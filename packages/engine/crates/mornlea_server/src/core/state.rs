//! Opaque single-threaded authority.
//!
//! Sibling modules do not read these fields. They call the ports below.
//! Staging a compound effect rehearses ordered inventory, container, drop and
//! projectile edits on private scratch state, and applies the whole effect
//! only after every component validates. A later rejection restores
//! the overlay. Publication encodes control packets and routed events through
//! the existing protocol conversion before it appends any frame.

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet, VecDeque};

use mornlea_domain::{
    CommandEnvelope, CommandEnvelopeParts, ContainerRef, Dimension, EventRecipient, MotionState,
    PlayerId, RejectReason, RoutedEvent, Weather, WorldState,
};
use mornlea_protocol::{
    AdmittedLogin, Direction, PlayIntent, ProtocolCodec, ProtocolError, ServerPacket, State,
};
use mornlea_storage::{Chunk, Metadata, PlayerLocation, PlayerSave, StoredPlayer};

use super::acquisition::{
    AcquiredChunkEvent, AcquisitionState, ChunkGenerationReservation, ChunkLoadReservation,
    LiveChunkFacts, LiveChunkPhase, RejectedAcquiredChunk,
};
use super::block_observations::ChunkBlockObservations;
use super::container_store::ContainerState;
use super::contracts::*;
use super::drop_store::{self, DropState};
use super::login_seed::{SeededPlayer, seed_player};
use super::publication::{EnqueueOutcome, PreparedFrame, PreparedPublicationPort};
use super::world::ReadyChunk;
use crate::rules::farmland::FarmlandSchedule;
use crate::rules::fluids::FluidSchedule;

const COMPANION_INBOX: usize = 4;

#[cfg(test)]
thread_local! {
    // Private work observations count current-key lookups without changing decisions.
    static CURRENT_PUBLICATION_VISITS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    static CURRENT_DUPLICATE_VISITS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    // One tick's bounded acquisition observation; no production hook is exposed.
    static ACQUIRE_AVAILABILITY: std::cell::Cell<Option<(ChunkKey,bool,bool)>> = const { std::cell::Cell::new(None) };
}

struct SessionRecord {
    player_id: PlayerId,
    display_name: String,
    phase: SessionPhase,
    last_applied_sequence: u64,
    last_input_sequence: u64,
    next_arrival: u64,
    body: Option<PlayerSave>,
    outbox: VecDeque<PreparedFrame>,
    outbox_closed: bool,
}

struct QueuedCompanion {
    /// Kept with the queued action so a later drain can report the same index
    /// the receipt already returned. Intake does not read it again.
    #[allow(dead_code)]
    arrival_index: u64,
    envelope: CompanionActionEnvelope,
}

/// Resident tick state carried across production ticks.
///
/// The serial reducer exclusively moves these maps into its tick context and
/// returns them after finalizing accepted dirty keys. Explicit replay and save
/// observations may clone residents off the tick; production never clones the
/// complete resident set.
#[derive(Clone, Default)]
pub struct ResidentTickState {
    pub actors: Vec<ActorRecord>,
    /// Ordinals for at most eight live recipients, never historical actors.
    player_slots: BTreeMap<SessionKey, usize>,
    pub runtimes: BTreeMap<ActorKey, ActorRuntime>,
    pub inventories: BTreeMap<ActorKey, InventoryRecord>,
    pub mining: BTreeMap<ActorKey, MiningProgress>,
    pub projectiles: Vec<ProjectileRecord>,
    pub environment: Option<EnvironmentState>,
    pub sleep_record: Option<SleepState>,
    pub sleeping: BTreeSet<SessionKey>,
    pub blocks: ChunkBlockObservations,
    ready: BTreeMap<ChunkKey, ReadyChunk>,
    drops: BTreeMap<ChunkKey, DropState>,
    containers: BTreeMap<ContainerRef, ContainerRecord>,
    container_chunks: BTreeMap<ChunkKey, ContainerState>,
    /// Accepted writes retained until a successful tick finalizes their keys.
    dirty_chunks: BTreeSet<ChunkKey>,
}

impl ResidentTickState {
    /// Explicit durable snapshots in chunk-key order, including all committed
    /// overlays and physical slots. Materialization belongs off the tick.
    pub fn ready_snapshot(&self) -> Vec<(ChunkKey, u64, u64, Chunk)> {
        self.ready
            .values()
            .map(|chunk| {
                chunk.snapshot(
                    self.blocks
                        .values()
                        .filter(|observed| observed.key == chunk.key),
                    self.drops.get(&chunk.key),
                    self.container_chunks.get(&chunk.key),
                )
            })
            .collect()
    }

    /// Carried drops in chunk-key then physical-slot order.
    pub fn drop_records(&self) -> Vec<DropRecord> {
        self.drops
            .values()
            .flat_map(|state| state.records().iter().cloned())
            .collect()
    }

    /// Carried containers from sparse records and Ready-chunk states merged
    /// by reference. The two sources are disjoint by construction: sparse
    /// staging refuses keys a Ready chunk already owns.
    pub fn container_records(&self) -> BTreeMap<ContainerRef, ContainerRecord> {
        let mut merged: BTreeMap<ContainerRef, ContainerRecord> = self.containers.clone();
        for (key, state) in &self.container_chunks {
            for reference in state.references(*key) {
                if let Some(record) = state.record(*key, reference) {
                    merged.insert(reference, record);
                }
            }
        }
        merged
    }
}

/// Private world, sessions, queues, tick, and publication owner.
pub struct AuthorityState {
    limits: ServerLimits,
    phase: ServerPhase,
    next_session: u64,
    ids_exhausted: bool,
    next_tick: u64,
    world_seed: i64,
    sessions: BTreeMap<SessionKey, SessionRecord>,
    /// Prepared and Active membership is bounded independently of retained history.
    current_sessions: BTreeSet<SessionKey>,
    occupied: usize,
    commands: Vec<CommandEnvelope>,
    companions: Vec<QueuedCompanion>,
    companion_arrival: BTreeMap<mornlea_domain::CompanionId, u64>,
    interactions: Vec<AuthorityInteraction>,
    chunk_results: Vec<ChunkResult>,
    acquisition: AcquisitionState,
    cancelled_chunks: BTreeSet<ChunkRequestId>,
    chunk_cancel_discards: usize,
    chunk_duplicate_discards: usize,
    chunk_consumed_cancels: BTreeSet<ChunkRequestId>,
    metadata: Metadata,
    metadata_sequence: u64,
    dirty: Vec<OwnedSnapshot>,
    in_flight: Vec<OwnedSnapshot>,
    shutdown: ShutdownReport,
    final_consumed: bool,
    /// Committed container viewer leases by session. The serial reducer owns
    /// the overlay-commit leg that writes this store; providers only stage
    /// viewer overlays on the tick context.
    views: BTreeMap<SessionKey, ViewLease>,
    /// Carried fluid update and rescan schedule. The serial reducer takes
    /// this value for its world row and returns it after, so future-due
    /// requeues and unstarted sections resume next tick with their original
    /// dues and cursors.
    fluid_schedule: FluidSchedule,
    /// Carried farmland candidate and rescan schedule, owned the same way.
    farmland_schedule: FarmlandSchedule,
    /// Resident tick state the reducer seeds and commits each tick.
    residents: ResidentTickState,
}

impl AuthorityState {
    pub fn try_new(limits: ServerLimits, world_seed: i64) -> Result<Self, ServerError> {
        let mut commands = Vec::new();
        commands
            .try_reserve_exact(limits.queued_commands())
            .map_err(|_| ServerError::Capacity {
                resource: Resource::Commands,
                limit: limits.queued_commands(),
                observed: limits.queued_commands(),
            })?;
        let metadata = Metadata {
            format_version: mornlea_storage::METADATA_CURRENT_VERSION,
            seed: world_seed,
            spawn_dimension: 0,
            spawn_anchor: mornlea_storage::MetadataChunkPos { x: 0, z: 0 },
            world_time_ticks: 0,
            day_phase_offset: 0,
            weather_kind: 0,
            weather_ticks_remaining: 0,
            depths_spawn_anchor: mornlea_storage::MetadataChunkPos { x: 0, z: 0 },
            depths_seed_salt: 0x9e3779b97f4a7c15,
            difficulty: 0,
        };
        mornlea_storage::world_metadata_encoded_len(&metadata)
            .map_err(|_| ServerError::InvalidInput { field: "metadata" })?;
        Ok(Self {
            limits,
            phase: ServerPhase::Running,
            next_session: 1,
            ids_exhausted: false,
            next_tick: 0,
            world_seed,
            sessions: BTreeMap::new(),
            current_sessions: BTreeSet::new(),
            occupied: 0,
            commands,
            companions: Vec::new(),
            companion_arrival: BTreeMap::new(),
            interactions: Vec::new(),
            chunk_results: Vec::new(),
            acquisition: AcquisitionState::default(),
            cancelled_chunks: BTreeSet::new(),
            chunk_cancel_discards: 0,
            chunk_duplicate_discards: 0,
            chunk_consumed_cancels: BTreeSet::new(),
            metadata,
            metadata_sequence: 1,
            dirty: Vec::new(),
            in_flight: Vec::new(),
            shutdown: ShutdownReport {
                final_tick: None,
                completed: Vec::new(),
                next: ShutdownPhase::StopAdmission,
                durable: 0,
                failed: 0,
                outstanding: 0,
                retryable: true,
            },
            final_consumed: false,
            views: BTreeMap::new(),
            fluid_schedule: FluidSchedule::new(),
            farmland_schedule: FarmlandSchedule::new(),
            residents: ResidentTickState::default(),
        })
    }

    /// Restores a checked fixed record before any tick or provider starts.
    /// Raw codec fields retain their source fidelity until live climate capture.
    pub fn try_new_with_metadata(
        limits: ServerLimits,
        metadata: Metadata,
    ) -> Result<Self, ServerError> {
        mornlea_storage::world_metadata_encoded_len(&metadata)
            .map_err(|_| ServerError::InvalidInput { field: "metadata" })?;
        let mut authority = Self::try_new(limits, metadata.seed)?;
        authority.metadata = metadata;
        Ok(authority)
    }

    /// Replaces committed leases with the tick's complete net viewer set.
    /// The reducer owns this commit and retired-session pruning.
    pub fn commit_viewers(&mut self, overlay: BTreeMap<SessionKey, ViewLease>) {
        self.views = overlay;
    }

    /// Installs a detached replay or harness snapshot. Production resident
    /// ownership returns through the exclusive tick context instead.
    pub fn commit_residents(&mut self, next: ResidentTickState) {
        self.residents = next;
    }

    /// Captures the final private state before consuming its one-tick reset.
    pub(crate) fn project_player_updates(&mut self, tick: u64) -> Vec<RoutedEvent> {
        let Some(environment) = self.residents.environment.as_ref() else {
            return Vec::new();
        };
        let mut events = Vec::new();
        let mut projected = Vec::new();
        let mut refused = Vec::new();
        // Retired records retain durable and wire ownership. Their number
        // cannot enlarge this live-recipient projection or its census work.
        for (&session, &slot) in &self.residents.player_slots {
            let Some(record) = self
                .sessions
                .get(&session)
                .filter(|record| record.phase == SessionPhase::Active)
            else {
                continue;
            };
            let key = ActorKey::Player(session);
            let Some(actor) = self
                .residents
                .actors
                .get(slot)
                .filter(|actor| actor.key == key)
            else {
                refused.push(session);
                continue;
            };
            match super::player_publication::project(
                tick,
                record.last_input_sequence,
                actor,
                self.residents.inventories.get(&key),
                self.residents.runtimes.get(&key),
                self.residents.mining.get(&key),
                environment,
            ) {
                Ok(state) => {
                    events.push(RoutedEvent::new(
                        EventRecipient::Session(session.get()),
                        mornlea_domain::Event::PlayerState(state),
                    ));
                    projected.push(key);
                }
                Err(_) => refused.push(session),
            }
        }
        for key in projected {
            if let Some(runtime) = self.residents.runtimes.get_mut(&key) {
                runtime.reset = false;
            }
        }
        for session in refused {
            let _ = self.retire(session, CloseReason::InvalidPlay);
        }
        events
    }

    /// Cloned resident tick state for inspection and replay assertions.
    pub fn residents(&self) -> ResidentTickState {
        self.residents.clone()
    }

    /// Login seeds for Active sessions with a save body and no player actor,
    /// in ascending session order. Saves are validated at install, so a
    /// mapping refusal only means in-memory corruption; skipping leaves the
    /// session for a later tick instead of failing the tick on login staging.
    pub(crate) fn login_seeds(&self) -> Vec<SeededPlayer> {
        let mut seeds = Vec::new();
        for (session, record) in &self.sessions {
            if record.phase != SessionPhase::Active {
                continue;
            }
            let Some(save) = record.body.as_ref() else {
                continue;
            };
            if self
                .residents
                .actors
                .iter()
                .any(|actor| actor.key == ActorKey::Player(*session))
            {
                continue;
            }
            if let Ok(seeded) = seed_player(*session, save) {
                seeds.push(seeded);
            }
        }
        seeds
    }

    /// Carried fluid schedule for inspection and seeding. The reducer takes
    /// exclusive ownership for its world row with a replace below.
    pub fn fluid_schedule(&self) -> &FluidSchedule {
        &self.fluid_schedule
    }

    /// Exclusive carried fluid schedule. The reducer replaces it out before
    /// constructing the tick context and writes the worked value back after
    /// the context drops, so no borrowed schedule ever outlives the tick.
    pub fn fluid_schedule_mut(&mut self) -> &mut FluidSchedule {
        &mut self.fluid_schedule
    }

    /// Carried farmland schedule for inspection and seeding.
    pub fn farmland_schedule(&self) -> &FarmlandSchedule {
        &self.farmland_schedule
    }

    /// Exclusive carried farmland schedule, owned like the fluid one.
    pub fn farmland_schedule_mut(&mut self) -> &mut FarmlandSchedule {
        &mut self.farmland_schedule
    }

    pub fn phase(&self) -> ServerPhase {
        self.phase
    }

    pub fn next_tick(&self) -> u64 {
        self.next_tick
    }

    pub fn world_seed(&self) -> i64 {
        self.world_seed
    }

    pub fn limits(&self) -> ServerLimits {
        self.limits
    }

    pub fn admit(
        &mut self,
        login: AdmittedLogin,
        transport: TransportKind,
    ) -> Result<SessionKey, ServerError> {
        self.allocate(login, transport)
    }

    pub fn submit(
        &mut self,
        session: SessionKey,
        intent: PlayIntent,
    ) -> Result<SubmissionReceipt, ServerError> {
        self.accept(session, intent)
    }

    pub fn submit_companion(
        &mut self,
        candidate: CompanionActionEnvelope,
    ) -> Result<CompanionReceipt, ServerError> {
        if self.phase != ServerPhase::Running {
            return Err(ServerError::InvalidState { phase: self.phase });
        }
        let duplicate = self.companions.iter().any(|queued| {
            queued.envelope.request_id == candidate.request_id
                && queued.envelope.generation == candidate.generation
                && queued.envelope.attempt == candidate.attempt
        });
        if duplicate {
            return Err(ServerError::InvalidInput {
                field: "companion_action",
            });
        }
        let queued = self
            .companions
            .iter()
            .filter(|queued| queued.envelope.companion_id == candidate.companion_id)
            .count();
        if queued >= COMPANION_INBOX {
            return Err(ServerError::Capacity {
                resource: Resource::Commands,
                limit: COMPANION_INBOX,
                observed: queued + 1,
            });
        }
        let next = self
            .companion_arrival
            .get(&candidate.companion_id)
            .copied()
            .unwrap_or(0);
        if next == u64::MAX {
            return Err(ServerError::Capacity {
                resource: Resource::Arrivals,
                limit: usize::MAX,
                observed: usize::MAX,
            });
        }
        self.companion_arrival
            .insert(candidate.companion_id, next.saturating_add(1));
        self.companions.push(QueuedCompanion {
            arrival_index: next,
            envelope: candidate,
        });
        Ok(CompanionReceipt::new(self.next_tick, next))
    }

    pub fn advance_tick(&mut self, work: TickBudget) -> Result<TickPublication, ServerError> {
        if self.phase != ServerPhase::Running {
            return Err(ServerError::InvalidState { phase: self.phase });
        }
        TickBudget::try_new(
            work.commands(),
            work.fluid_updates_per_dimension(),
            work.fluid_rescan_target_per_dimension(),
            work.farmland_checks(),
            work.farmland_block_reads(),
        )?;
        // Providers execute with the pre-bump tick: the reducer reads the
        // current counter as the executing tick, and the bump lands only
        // after it returns, so staged event ticks name the tick the
        // publication carries.
        let publication = super::step::reduce_tick(self, work);
        self.next_tick = self.next_tick.saturating_add(1);
        Ok(publication)
    }

    pub fn close_session(
        &mut self,
        session: SessionKey,
        reason: CloseReason,
    ) -> Result<(), ServerError> {
        self.retire(session, reason)
    }

    pub fn allocate(
        &mut self,
        login: AdmittedLogin,
        kind: TransportKind,
    ) -> Result<SessionKey, ServerError> {
        let _ = kind;
        self.reserve(login, SessionPhase::Active)
    }

    pub fn prepare(
        &mut self,
        login: AdmittedLogin,
        kind: TransportKind,
    ) -> Result<SessionKey, ServerError> {
        let _ = kind;
        self.reserve(login, SessionPhase::Prepared)
    }

    pub fn install(
        &mut self,
        session: SessionKey,
        loaded: Option<StoredPlayer>,
    ) -> Result<(), ServerError> {
        let player_id = self
            .sessions
            .get(&session)
            .filter(|record| record.phase == SessionPhase::Prepared)
            .map(|record| record.player_id)
            .ok_or(ServerError::StaleSession { session })?;
        let display_name = self
            .sessions
            .get(&session)
            .map(|record| record.display_name.clone())
            .unwrap_or_default();
        let save = match loaded {
            None => canonical_player(player_id, &display_name)?,
            Some(stored) => {
                if stored.player_id.to_bytes() != player_id.bytes() {
                    return Err(ServerError::InvalidInput { field: "player_id" });
                }
                save_from_stored(stored)?
            }
        };
        let record = self
            .sessions
            .get_mut(&session)
            .ok_or(ServerError::StaleSession { session })?;
        if record.phase != SessionPhase::Prepared {
            return Err(ServerError::InvalidState { phase: self.phase });
        }
        record.body = Some(save);
        Ok(())
    }

    pub fn activate(&mut self, session: SessionKey) -> Result<(), ServerError> {
        let record = self
            .sessions
            .get_mut(&session)
            .ok_or(ServerError::StaleSession { session })?;
        if record.phase != SessionPhase::Prepared || record.body.is_none() {
            return Err(ServerError::InvalidState { phase: self.phase });
        }
        record.phase = SessionPhase::Active;
        Ok(())
    }

    pub fn session(&self, key: SessionKey) -> Option<SessionFacts> {
        self.sessions.get(&key).map(|record| SessionFacts {
            player_id: record.player_id,
            phase: record.phase,
            last_applied_sequence: record.last_applied_sequence,
            next_arrival: record.next_arrival,
        })
    }

    pub fn retire(&mut self, key: SessionKey, _reason: CloseReason) -> Result<(), ServerError> {
        let record = self
            .sessions
            .get_mut(&key)
            .ok_or(ServerError::StaleSession { session: key })?;
        if record.phase == SessionPhase::Retired {
            return Err(ServerError::StaleSession { session: key });
        }
        record.phase = SessionPhase::Retired;
        record.outbox_closed = true;
        self.current_sessions.remove(&key);
        self.occupied = self.occupied.saturating_sub(1);
        // Sleep participation belongs to the live session. Durable respawn
        // anchors and the other resident lanes keep their persistence owner.
        if let Some(sleep) = &mut self.residents.sleep_record {
            sleep.beds.retain(|(session, _, _)| *session != key);
        }
        self.residents.sleeping.remove(&key);
        self.residents.player_slots.remove(&key);
        Ok(())
    }

    pub fn accept(
        &mut self,
        session: SessionKey,
        intent: PlayIntent,
    ) -> Result<SubmissionReceipt, ServerError> {
        if self.phase != ServerPhase::Running {
            return Err(ServerError::InvalidState { phase: self.phase });
        }
        let (phase, next_arrival) = {
            let record = self
                .sessions
                .get(&session)
                .ok_or(ServerError::StaleSession { session })?;
            if record.phase == SessionPhase::Retired {
                return Err(ServerError::StaleSession { session });
            }
            if record.phase != SessionPhase::Active {
                return Err(ServerError::InvalidInput {
                    field: "session_phase",
                });
            }
            (record.phase, record.next_arrival)
        };
        let _ = phase;
        match intent {
            PlayIntent::Chat(_) | PlayIntent::KeepAliveReply { .. } => {
                Ok(SubmissionReceipt::ControlAccepted)
            }
            PlayIntent::Sequenced { sequence, command } => {
                // Intake order is seam-fixed: validate the whole payload before
                // queue or arrival capacity, so a combined violation reports
                // the payload boundary and never consumes capacity state.
                let envelope = CommandEnvelope::try_new(CommandEnvelopeParts {
                    tick: self.next_tick,
                    session: session.get(),
                    sequence,
                    arrival_index: next_arrival,
                    command,
                })
                .map_err(|_| ServerError::InvalidInput { field: "command" })?;
                if self.commands.len() >= self.limits.queued_commands() {
                    return Err(ServerError::Capacity {
                        resource: Resource::Commands,
                        limit: self.limits.queued_commands(),
                        observed: self.commands.len() + 1,
                    });
                }
                if next_arrival == u64::MAX {
                    return Err(ServerError::Capacity {
                        resource: Resource::Arrivals,
                        limit: usize::MAX,
                        observed: usize::MAX,
                    });
                }
                self.commands.push(envelope);
                if let Some(record) = self.sessions.get_mut(&session) {
                    record.next_arrival = next_arrival.saturating_add(1);
                }
                Ok(SubmissionReceipt::QueuedForTick {
                    tick: self.next_tick,
                    arrival_index: next_arrival,
                })
            }
        }
    }

    pub fn apply_sequence(&mut self, session: SessionKey, sequence: u64) -> bool {
        let Some(record) = self.sessions.get_mut(&session) else {
            return false;
        };
        if sequence <= record.last_applied_sequence {
            return false;
        }
        record.last_applied_sequence = sequence;
        true
    }

    pub fn freeze_eligible(&mut self, tick: u64) -> Vec<CommandEnvelope> {
        let mut kept = Vec::new();
        let mut frozen = Vec::new();
        for command in self.commands.drain(..) {
            if command.tick() <= tick {
                frozen.push(command);
            } else {
                kept.push(command);
            }
        }
        self.commands = kept;
        frozen
    }

    pub fn carry(&mut self, batch: Vec<CommandEnvelope>) -> Result<(), ServerError> {
        let observed = self.commands.len().saturating_add(batch.len());
        if observed > self.limits.queued_commands() {
            self.commands.extend(batch);
            return Err(ServerError::Capacity {
                resource: Resource::Commands,
                limit: self.limits.queued_commands(),
                observed,
            });
        }
        self.commands.extend(batch);
        Ok(())
    }

    /// Starts managed acquisition only on an empty world; actor state may exist.
    pub fn enable_live_chunks(&mut self) -> Result<(), ServerError> {
        if self.acquisition.enabled() {
            return Ok(());
        }
        if self.phase != ServerPhase::Running
            || !self.residents.blocks.is_empty()
            || !self.residents.ready.is_empty()
            || !self.residents.drops.is_empty()
            || !self.residents.containers.is_empty()
            || !self.residents.container_chunks.is_empty()
            || !self.residents.dirty_chunks.is_empty()
            || !self.chunk_results.is_empty()
            || !self.dirty.is_empty()
            || !self.in_flight.is_empty()
        {
            return Err(ServerError::InvalidState { phase: self.phase });
        }
        self.acquisition.enable();
        Ok(())
    }
    fn require_live_chunks(&self, running: bool) -> Result<(), ServerError> {
        if !self.acquisition.enabled()
            || self.phase == ServerPhase::Closed
            || (running && self.phase != ServerPhase::Running)
        {
            Err(ServerError::InvalidState { phase: self.phase })
        } else {
            Ok(())
        }
    }
    pub fn replace_chunk_wants(&mut self, wants: BTreeSet<ChunkKey>) -> Result<(), ServerError> {
        self.require_live_chunks(true)?;
        self.acquisition.replace_wants(wants)
    }
    pub fn live_chunk_facts(&self, key: ChunkKey) -> Option<LiveChunkFacts> {
        self.acquisition.facts(key)
    }
    pub fn live_chunk_error(&self, key: ChunkKey) -> Option<&ServerError> {
        self.acquisition.error(key)
    }
    pub fn reserve_chunk_load(
        &mut self,
        key: ChunkKey,
    ) -> Result<ChunkLoadReservation, ServerError> {
        self.require_live_chunks(true)?;
        self.acquisition.reserve_load(key)
    }
    pub fn bind_chunk_load(
        &mut self,
        r: ChunkLoadReservation,
        request: ChunkRequestId,
    ) -> Result<(), ServerError> {
        self.require_live_chunks(false)?;
        self.acquisition.bind_load(r, request)
    }
    pub fn abort_chunk_load(
        &mut self,
        r: ChunkLoadReservation,
        error: ServerError,
    ) -> Result<(), ServerError> {
        self.require_live_chunks(false)?;
        self.acquisition.abort_load(r, error)
    }
    pub fn reserve_chunk_generation(
        &mut self,
        key: ChunkKey,
    ) -> Result<ChunkGenerationReservation, ServerError> {
        self.require_live_chunks(true)?;
        self.acquisition.reserve_generation(key)
    }
    pub fn bind_chunk_generation(
        &mut self,
        r: ChunkGenerationReservation,
        request: ChunkRequestId,
    ) -> Result<(), ServerError> {
        self.require_live_chunks(false)?;
        self.acquisition.bind_generation(r, request)
    }
    pub fn abort_chunk_generation(
        &mut self,
        r: ChunkGenerationReservation,
        error: ServerError,
    ) -> Result<(), ServerError> {
        self.require_live_chunks(false)?;
        self.acquisition.abort_generation(r, error)
    }
    // Return the whole refused event; its prepared owner must never be cloned.
    #[allow(clippy::result_large_err)]
    pub fn offer_acquired(
        &mut self,
        event: AcquiredChunkEvent,
    ) -> Result<(), RejectedAcquiredChunk> {
        if let Err(error) = self.require_live_chunks(false) {
            return Err(RejectedAcquiredChunk { error, event });
        }
        self.acquisition.offer(event)
    }

    // Refusal returns the original owned chunk result, including in managed
    // mode; the mailbox never stores or copies a rejected record.
    #[allow(clippy::result_large_err)]
    pub fn admit_chunk(&mut self, result: ChunkResult) -> Result<(), ChunkResult> {
        if self.acquisition.enabled() {
            return Err(result);
        }
        if self.cancelled_chunks.remove(&result.request) {
            // The tombstone rejects exactly one late completion. The consumed
            // request stays recorded so one further repeat classifies as a
            // duplicate instead of entering the queue.
            self.chunk_cancel_discards += 1;
            self.chunk_consumed_cancels.insert(result.request);
            return Ok(());
        }
        if self
            .chunk_results
            .iter()
            .any(|queued| queued.request == result.request)
            || self.chunk_consumed_cancels.contains(&result.request)
        {
            // A repeated completion for an already-queued or already-discarded
            // request is silently discarded; ownership is released here.
            self.chunk_duplicate_discards += 1;
            return Ok(());
        }
        if self.chunk_results.len() >= self.limits.ready_chunk_results() {
            return Err(result);
        }
        self.chunk_results.push(result);
        Ok(())
    }

    pub fn drain_chunks(&mut self, max: usize) -> Vec<ChunkResult> {
        let count = max.min(self.chunk_results.len());
        self.chunk_results.drain(..count).collect()
    }

    pub fn cancel_chunk(&mut self, request: ChunkRequestId) {
        if self.acquisition.enabled() {
            return;
        }
        self.chunk_results
            .retain(|result| result.request != request);
        self.cancelled_chunks.insert(request);
    }

    /// Counted chunk-result discards: completions rejected by a cancellation
    /// tombstone first, repeats second. Neither is ever installed.
    pub fn chunk_discard_counts(&self) -> (usize, usize) {
        (self.chunk_cancel_discards, self.chunk_duplicate_discards)
    }

    pub fn drain_companions(&mut self, max: usize) -> Vec<CompanionActionEnvelope> {
        let count = max.min(self.companions.len());
        self.companions
            .drain(..count)
            .map(|queued| queued.envelope)
            .collect()
    }

    pub fn enqueue_interaction(&mut self, value: AuthorityInteraction) -> Result<(), ServerError> {
        let observed = self.commands.len().saturating_add(self.interactions.len());
        if observed >= self.limits.queued_commands() {
            return Err(ServerError::Capacity {
                resource: Resource::Commands,
                limit: self.limits.queued_commands(),
                observed: observed + 1,
            });
        }
        self.interactions.push(value);
        Ok(())
    }

    pub fn publish(&mut self, publication: TickPublication) -> Result<(), ServerError> {
        let mut codec =
            ProtocolCodec::new().map_err(|_| ServerError::InvalidInput { field: "packet" })?;
        let mut pending = Vec::new();
        for event in &publication.events {
            let packet = ServerPacket::try_from(event.event().clone())
                .map_err(|_| ServerError::InvalidInput { field: "packet" })?;
            let frame = PreparedFrame::encode(&mut codec, &packet)?;
            match event.recipient() {
                EventRecipient::Session(raw) => {
                    let session = SessionKey::from_raw(raw)
                        .ok_or(ServerError::InvalidInput { field: "packet" })?;
                    if !self.sessions.contains_key(&session) {
                        return Err(ServerError::StaleSession { session });
                    }
                    pending.push(PendingFrame::One { session, frame });
                }
                EventRecipient::Broadcast => pending.push(PendingFrame::Broadcast { frame }),
            }
        }
        for reply in &publication.control {
            if !self.sessions.contains_key(&reply.session) {
                return Err(ServerError::StaleSession {
                    session: reply.session,
                });
            }
            let frame = PreparedFrame::encode(&mut codec, &reply.packet)?;
            pending.push(PendingFrame::One {
                session: reply.session,
                frame,
            });
        }
        let mut slow = Vec::new();
        for item in pending {
            match item {
                PendingFrame::One { session, frame } => {
                    if self.append_frame(session, frame) {
                        slow.push(session);
                    }
                }
                PendingFrame::Broadcast { frame } => {
                    let sessions: Vec<SessionKey> = self.current_sessions.iter().copied().collect();
                    for session in sessions {
                        #[cfg(test)]
                        CURRENT_PUBLICATION_VISITS.with(|visits| visits.set(visits.get() + 1));
                        if self.append_frame(session, frame.clone()) {
                            slow.push(session);
                        }
                    }
                }
            }
        }
        // A saturated receiver is retired only after the append loop so one
        // session flips at most once per publication. Retirement frees the
        // player slot exactly like a peer-gone close; the overflowing frame
        // was dropped by the append and no Disconnect frame is appended.
        for session in slow {
            let _ = self.retire(session, CloseReason::SlowReceiver);
        }
        Ok(())
    }

    /// Appends one frame to a receiver's outbox. Returns `true` only when
    /// this append saturated the outbox and flipped it closed; the overflowing
    /// frame is dropped and the caller owns the slow-receiver retirement.
    fn append_frame(&mut self, session: SessionKey, frame: PreparedFrame) -> bool {
        let Some(record) = self.sessions.get_mut(&session) else {
            return false;
        };
        if record.outbox_closed {
            return false;
        }
        if record.outbox.len() >= self.limits.session_outbox() {
            record.outbox_closed = true;
            return true;
        }
        record.outbox.push_back(frame);
        false
    }

    /// Admits an immutable Play owner; the receipt certifies queue admission only.
    /// Legacy control publication keeps its existing permissive phase policy.
    pub fn enqueue_prepared(
        &mut self,
        session: SessionKey,
        frame: PreparedFrame,
    ) -> Result<EnqueueOutcome, ServerError> {
        let key = frame.packet_key();
        if key.direction != Direction::ServerToClient || key.state != State::Play {
            return Err(ServerError::InvalidInput { field: "packet" });
        }
        let record = self
            .sessions
            .get(&session)
            .ok_or(ServerError::StaleSession { session })?;
        if record.phase != SessionPhase::Active || record.outbox_closed {
            return Ok(EnqueueOutcome::Closed);
        }
        if self.append_frame(session, frame) {
            self.retire(session, CloseReason::SlowReceiver)?;
            return Ok(EnqueueOutcome::Closed);
        }
        Ok(EnqueueOutcome::Queued)
    }

    /// Moves whole immutable owners from the one FIFO, including retained prefixes.
    /// The first frame is exempt from the byte budget; later nonfits stay queued.
    pub fn take_prepared_outbox(
        &mut self,
        session: SessionKey,
        max_frames: usize,
        max_bytes: usize,
    ) -> Result<Vec<PreparedFrame>, ServerError> {
        let record = self
            .sessions
            .get_mut(&session)
            .ok_or(ServerError::StaleSession { session })?;
        let mut taken = Vec::new();
        let mut bytes = 0usize;
        while taken.len() < max_frames.min(512) {
            let Some(frame) = record.outbox.front() else {
                break;
            };
            let next = bytes.saturating_add(frame.byte_len());
            if !taken.is_empty() && next > max_bytes {
                break;
            }
            bytes = next;
            taken.push(record.outbox.pop_front().expect("inspected FIFO head"));
        }
        Ok(taken)
    }

    /// Legacy byte observation converts the same FIFO owners off tick.
    pub fn take_outbox(
        &mut self,
        session: SessionKey,
        max_frames: usize,
        max_bytes: usize,
    ) -> Result<Vec<Vec<u8>>, ServerError> {
        self.take_prepared_outbox(session, max_frames, max_bytes)
            .map(|frames| {
                frames
                    .into_iter()
                    .map(PreparedFrame::into_legacy_bytes)
                    .collect()
            })
    }

    pub fn close_outbox(&mut self, session: SessionKey, _reason: CloseReason) {
        if let Some(record) = self.sessions.get_mut(&session) {
            record.outbox_closed = true;
        }
    }

    pub fn begin_close(&mut self) -> bool {
        if self.phase != ServerPhase::Running {
            return false;
        }
        self.phase = ServerPhase::Closing;
        true
    }

    pub fn run_final(&mut self, reducer: &mut dyn FinalReducer) -> Result<u64, ServerError> {
        if self.final_consumed {
            return Err(ServerError::InvalidState { phase: self.phase });
        }
        let tick = reducer.reduce_final(self)?;
        self.final_consumed = true;
        self.next_tick = self.next_tick.saturating_add(1);
        Ok(tick)
    }

    pub fn freeze(&mut self) -> FrozenAuthority {
        let mut save_keys = if self.acquisition.enabled() {
            self.acquisition.save_keys()
        } else {
            Vec::new()
        };
        for snapshot in self.dirty.iter().chain(self.in_flight.iter()) {
            if !save_keys.contains(&snapshot.key) {
                save_keys.push(snapshot.key.clone());
            }
        }
        FrozenAuthority {
            final_tick: self.shutdown.final_tick.unwrap_or(self.next_tick),
            save_keys,
        }
    }

    pub fn shutdown_progress(&self) -> ShutdownReport {
        self.shutdown.clone()
    }

    pub fn record_progress(&mut self, report: ShutdownReport) {
        self.shutdown = report;
    }

    pub fn mark_closed(&mut self) {
        self.phase = ServerPhase::Closed;
        self.shutdown.next = ShutdownPhase::Closed;
        self.shutdown.retryable = false;
    }

    /// Captures the current Ready target without materialization or scheduling.
    /// Call after live commit; physical counter-only slot values remain current
    /// even when they share a durable revision with an earlier capture.
    pub fn capture_chunk_snapshot(
        &self,
        key: ChunkKey,
        urgency: SaveUrgency,
    ) -> Option<OwnedSnapshot> {
        if self.acquisition.enabled()
            && !self.acquisition.facts(key).is_some_and(|f| {
                matches!(f.phase, LiveChunkPhase::Ready | LiveChunkPhase::Unloading)
            })
        {
            return None;
        }
        let chunk = self.residents.ready.get(&key)?;
        // Live captures use the source scheduling metric; mailbox admission
        // independently reserves its compression maximum. Legacy fixtures keep it.
        let estimate = if self.acquisition.enabled() {
            chunk.payload_estimate()?
        } else {
            crate::store::mailbox::CHUNK_MAX_RESERVATION
        };
        let view = chunk.capture(
            self.residents.drops.get(&key),
            self.residents.container_chunks.get(&key),
        );
        OwnedSnapshot::try_new(
            SaveKey::Chunk(key),
            view.revision(),
            estimate,
            urgency,
            SaveValue::ChunkView(view),
        )
        .ok()
    }

    pub fn select(&mut self, mode: SaveMode, budget: SaveBudget) -> Vec<OwnedSnapshot> {
        if self.phase == ServerPhase::Closed {
            return Vec::new();
        }
        if self.acquisition.enabled() {
            return self.select_live(mode, budget);
        }
        let mut chosen = Vec::new();
        let mut rest = Vec::new();
        let mut bytes = 0usize;
        for snapshot in self.dirty.drain(..) {
            let first = chosen.is_empty();
            let fits_count = chosen.len() < budget.chunks;
            let fits_bytes =
                bytes.saturating_add(snapshot.estimated_bytes) <= budget.estimated_bytes;
            if first || (fits_count && fits_bytes) {
                bytes = bytes.saturating_add(snapshot.estimated_bytes);
                self.in_flight.push(snapshot.clone());
                chosen.push(snapshot);
            } else {
                rest.push(snapshot);
            }
        }
        self.dirty = rest;
        chosen
    }

    // Selection consults only the ordered dirty index and captures admitted keys.
    fn select_live(&mut self, mode: SaveMode, budget: SaveBudget) -> Vec<OwnedSnapshot> {
        let mut chosen = Vec::new();
        let mut bytes = 0usize;
        while self.acquisition.stats().in_flight < 8 && chosen.len() < 8 {
            let Some((key, estimate, urgency)) = self.acquisition.next_save(mode) else {
                break;
            };
            if !chosen.is_empty()
                && (chosen.len() >= budget.chunks
                    || bytes.saturating_add(estimate) > budget.estimated_bytes)
            {
                break;
            }
            let Some(snapshot) = self.capture_chunk_snapshot(key, urgency) else {
                self.acquisition.save_error(
                    key,
                    ServerError::Internal {
                        invariant: "chunk payload estimate",
                    },
                );
                break;
            };
            if let Err(error) = self.acquisition.selected(snapshot.clone()) {
                self.acquisition.save_error(key, error);
                break;
            }
            bytes = bytes.saturating_add(estimate);
            chosen.push(snapshot);
        }
        chosen
    }

    pub fn return_dirty(&mut self, snapshot: OwnedSnapshot) {
        if self.acquisition.enabled() {
            self.acquisition.release(&snapshot);
            return;
        }
        if let Some(position) = self.in_flight.iter().position(|held| held == &snapshot) {
            self.in_flight.remove(position);
        }
        self.dirty.push(snapshot);
    }

    pub fn apply_completion(&mut self, completion: SaveCompletion) -> AckReport {
        if self.acquisition.enabled() {
            return self.apply_live_completion(completion);
        }
        // The scheduler owns ticket correlation; this owner verifies the
        // echoed immutable preimages before touching any selected token.
        let identities_match = completion.submitted.len() == completion.snapshots.len()
            && completion.submitted.iter().zip(&completion.snapshots).all(
                |((key, revision), snapshot)| {
                    key == &snapshot.key && *revision == snapshot.revision
                },
            )
            && completion
                .committed
                .iter()
                .all(|identity| completion.submitted.contains(identity))
            && completion.snapshots.iter().all(|snapshot| {
                // Independent captures can share a revision; only one exact
                // held preimage must match. Unknown identities retain their lanes.
                let mut candidates = self
                    .in_flight
                    .iter()
                    .filter(|held| held.key == snapshot.key && held.revision == snapshot.revision);
                match candidates.next() {
                    None => true,
                    Some(held) => held == snapshot || candidates.any(|held| held == snapshot),
                }
            });
        let mut errors: Vec<_> = completion.error.into_iter().collect();
        if !identities_match {
            errors.push(ServerError::Internal {
                invariant: "save completion identity",
            });
            return AckReport {
                acked: 0,
                released: 0,
                retry: Vec::new(),
                errors,
            };
        }
        let mut acked = 0;
        let mut released = 0;
        let mut retry = Vec::new();
        for snapshot in completion.snapshots {
            let position = self.in_flight.iter().position(|held| held == &snapshot);
            // Metadata has an explicit direct-scheduler lane: unlike actor
            // and chunk saves, metadata_snapshot is submitted without select.
            if position.is_none() && !matches!(snapshot.key, SaveKey::Metadata) {
                continue;
            }
            let listed = completion
                .committed
                .contains(&(snapshot.key.clone(), snapshot.revision));
            if listed {
                acked += 1;
                if let Some(position) = position {
                    self.in_flight.remove(position);
                }
            } else {
                released += 1;
                // Backoff owns a payload copy while this token remains
                // charged; return_dirty or its retry ack releases the token.
                retry.push(snapshot);
            }
        }
        if !retry.is_empty() && errors.is_empty() {
            errors.push(ServerError::Internal {
                invariant: "incomplete save completion",
            });
        }
        AckReport {
            acked,
            released,
            retry,
            errors,
        }
    }

    fn apply_live_completion(&mut self, completion: SaveCompletion) -> AckReport {
        let mut errors: Vec<_> = completion.error.into_iter().collect();
        let observed = completion
            .snapshots
            .len()
            .max(completion.submitted.len())
            .max(completion.committed.len());
        if observed > 8 {
            errors.push(ServerError::Capacity {
                resource: Resource::SaveChunks,
                limit: 8,
                observed,
            });
            return AckReport {
                acked: 0,
                released: 0,
                retry: Vec::new(),
                errors,
            };
        }
        let mut keys = BTreeSet::new();
        let mut committed_keys = BTreeSet::new();
        let identity = completion.committed.iter().all(|(key, _)| match key {
            SaveKey::Chunk(key) => committed_keys.insert(*key),
            _ => true,
        }) && completion.submitted.len() == completion.snapshots.len()
            && completion
                .submitted
                .iter()
                .zip(&completion.snapshots)
                .all(|((key, revision), s)| key == &s.key && *revision == s.revision)
            && completion
                .committed
                .iter()
                .all(|id| completion.submitted.contains(id))
            && completion.snapshots.iter().all(|s| match s.key {
                SaveKey::Chunk(key) => {
                    keys.insert(key)
                        && (!self.acquisition.has_target(key, s.revision)
                            || self.acquisition.matches(s))
                }
                _ => true,
            });
        let validation = if !identity {
            Err(ServerError::Internal {
                invariant: "save completion identity",
            })
        } else {
            completion
                .snapshots
                .iter()
                .try_for_each(|s| self.acquisition.validate_saved(s))
        };
        if let Err(error) = validation {
            errors.push(error);
            return AckReport {
                acked: 0,
                released: 0,
                retry: Vec::new(),
                errors,
            };
        }
        let mut acked = 0;
        let mut released = 0;
        let mut retry = Vec::new();
        for snapshot in completion.snapshots {
            let held = self.acquisition.matches(&snapshot);
            if !held && !matches!(snapshot.key, SaveKey::Metadata) {
                continue;
            }
            if completion
                .committed
                .contains(&(snapshot.key.clone(), snapshot.revision))
            {
                if held {
                    // Whole-batch validation above makes this key-local update infallible.
                    self.acquisition
                        .saved(&snapshot)
                        .expect("validated live save preimage");
                }
                acked += 1;
            } else {
                released += 1;
                // The scheduler retries this exact capture while the flight stays charged.
                retry.push(snapshot);
            }
        }
        if !retry.is_empty() && errors.is_empty() {
            errors.push(ServerError::Internal {
                invariant: "incomplete save completion",
            });
        }
        AckReport {
            acked,
            released,
            retry,
            errors,
        }
    }

    pub fn save_stats(&self) -> SaveStats {
        if self.acquisition.enabled() {
            return self.acquisition.stats();
        }
        let estimated_unsaved_bytes = self
            .dirty
            .iter()
            .chain(self.in_flight.iter())
            .fold(0usize, |sum, snapshot| {
                sum.saturating_add(snapshot.estimated_bytes)
            });
        SaveStats {
            dirty: self.dirty.len(),
            in_flight: self.in_flight.len(),
            estimated_unsaved_bytes,
        }
    }

    /// Captures only committed climate fields; anchors, salt and difficulty
    /// remain the startup configuration. Distinct targets advance one checked
    /// process-local sequence, and refusal leaves the last captured target intact.
    pub fn try_metadata_snapshot(&mut self) -> Result<OwnedSnapshot, ServerError> {
        let mut target = self.metadata.clone();
        if let Some(environment) = &self.residents.environment {
            target.world_time_ticks = environment.world_time;
            target.day_phase_offset = u64::from(environment.day_phase_offset);
            target.weather_kind = environment.weather.wire_id();
            target.weather_ticks_remaining = environment.weather_remaining;
        }
        let estimated_bytes = mornlea_storage::world_metadata_encoded_len(&target)
            .map_err(|_| ServerError::InvalidInput { field: "metadata" })?;
        if target != self.metadata {
            let sequence = self
                .metadata_sequence
                .checked_add(1)
                .ok_or(ServerError::Internal {
                    invariant: "metadata sequence space",
                })?;
            self.metadata = target;
            self.metadata_sequence = sequence;
        }
        Ok(OwnedSnapshot {
            key: SaveKey::Metadata,
            revision: self.metadata_sequence,
            estimated_bytes,
            urgency: SaveUrgency::Autosave,
            value: SaveValue::Metadata(self.metadata.clone()),
        })
    }

    /// Returns the last captured target. Live persistence producers use
    /// `try_metadata_snapshot` so current climate and capture failures reach storage.
    pub fn metadata_snapshot(&self) -> OwnedSnapshot {
        let estimated_bytes =
            mornlea_storage::world_metadata_encoded_len(&self.metadata).unwrap_or(0);
        OwnedSnapshot {
            key: SaveKey::Metadata,
            revision: self.metadata_sequence,
            estimated_bytes,
            urgency: SaveUrgency::Autosave,
            value: SaveValue::Metadata(self.metadata.clone()),
        }
    }

    pub fn remember_dirty(&mut self, snapshot: OwnedSnapshot) -> Result<(), ServerError> {
        if self.acquisition.enabled() {
            return Err(ServerError::InvalidInput {
                field: "manual_live_snapshot",
            });
        }
        self.dirty.push(snapshot);
        Ok(())
    }

    pub fn drive_shutdown(
        &mut self,
        deadline: Deadline,
        io: &mut ShutdownIo<'_>,
    ) -> Result<ShutdownReport, ShutdownFailure> {
        if self.phase == ServerPhase::Closed {
            return Ok(self.shutdown.clone());
        }
        let mut report = self.shutdown.clone();
        for phase in ShutdownPhase::successors() {
            if *phase == ShutdownPhase::Closed {
                break;
            }
            if report.completed.contains(phase) {
                continue;
            }
            if deadline.expired(io.clock.monotonic()) {
                if *phase == ShutdownPhase::FinalizeMemory {
                    report.outstanding = io.memory.pending().outstanding;
                }
                return self.fail_shutdown(
                    report,
                    *phase,
                    ServerError::Timeout {
                        operation: Operation::Shutdown,
                    },
                );
            }
            if let Err(error) = self.run_shutdown_phase(*phase, &mut report, io, deadline) {
                return self.fail_shutdown(report, *phase, error);
            }
            report.completed.push(*phase);
            report.next = phase.next();
            self.shutdown = report.clone();
        }
        self.mark_closed();
        self.shutdown.final_tick = report.final_tick;
        self.shutdown.completed = report.completed.clone();
        self.shutdown.durable = report.durable;
        Ok(self.shutdown.clone())
    }

    fn fail_shutdown(
        &mut self,
        mut report: ShutdownReport,
        phase: ShutdownPhase,
        error: ServerError,
    ) -> Result<ShutdownReport, ShutdownFailure> {
        report.next = phase;
        report.retryable = true;
        report.failed = report.failed.saturating_add(1);
        self.shutdown = report.clone();
        if self.phase == ServerPhase::Running {
            self.phase = ServerPhase::Closing;
        }
        Err(ShutdownFailure { error, report })
    }

    fn run_shutdown_phase(
        &mut self,
        phase: ShutdownPhase,
        report: &mut ShutdownReport,
        io: &mut ShutdownIo<'_>,
        deadline: Deadline,
    ) -> Result<(), ServerError> {
        match phase {
            ShutdownPhase::StopAdmission => {
                self.begin_close();
                Ok(())
            }
            ShutdownPhase::FinalTick => {
                let tick = self.run_final(io.reducer)?;
                report.final_tick = Some(tick);
                Ok(())
            }
            ShutdownPhase::Freeze => {
                let _ = self.freeze();
                // Retain the lease while refusing planner work throughout
                // memory finalization, including a timed-out attempt.
                let _ = io.agent.freeze(io.clock);
                Ok(())
            }
            ShutdownPhase::WaitWorkers => {
                io.workers.stop_new()?;
                io.workers.cancel()?;
                io.workers.wait(deadline)
            }
            ShutdownPhase::FinalizeMemory => {
                super::shutdown::finalize_memory(io.memory, io.clock, deadline, report)
            }
            ShutdownPhase::FlushPlayers => {
                for (key, record) in &self.sessions {
                    if record.phase == SessionPhase::Retired {
                        continue;
                    }
                    let flushed = io
                        .persistence
                        .flush(SaveKey::Player(record.player_id), deadline)?;
                    report.durable = report.durable.saturating_add(flushed.durable);
                    let _ = key;
                }
                Ok(())
            }
            ShutdownPhase::FlushCompanions => {
                flush_family(io, SaveKey::Companions, deadline, report)
            }
            ShutdownPhase::FlushHostiles => flush_family(io, SaveKey::Hostiles, deadline, report),
            ShutdownPhase::FlushPassives => flush_family(io, SaveKey::Passives, deadline, report),
            ShutdownPhase::FlushWorld => {
                let chunks: Vec<SaveKey> = self
                    .dirty
                    .iter()
                    .filter(|snapshot| matches!(snapshot.key, SaveKey::Chunk(_)))
                    .map(|snapshot| snapshot.key.clone())
                    .collect();
                if chunks.is_empty() {
                    return Ok(());
                }
                for key in chunks {
                    let flushed = io.persistence.flush(key, deadline)?;
                    report.durable = report.durable.saturating_add(flushed.durable);
                }
                Ok(())
            }
            ShutdownPhase::FlushMetadata => flush_family(io, SaveKey::Metadata, deadline, report),
            ShutdownPhase::StoreSync => io.store.sync(deadline),
            ShutdownPhase::ReleaseAgent => {
                if let Some(lease) = io.agent.freeze(io.clock) {
                    io.agent.release(&lease, deadline)?;
                }
                Ok(())
            }
            ShutdownPhase::AgentClose => io.agent.close(deadline),
            ShutdownPhase::McpClose => io.mcp.close(deadline),
            ShutdownPhase::StoreClose => io.store.close(deadline),
            ShutdownPhase::CloseWorkers => io.workers.close(deadline),
            ShutdownPhase::Closed => Ok(()),
        }
    }

    fn reserve(
        &mut self,
        login: AdmittedLogin,
        phase: SessionPhase,
    ) -> Result<SessionKey, ServerError> {
        if self.phase != ServerPhase::Running {
            return Err(ServerError::InvalidState { phase: self.phase });
        }
        if self.occupied >= usize::from(self.limits.max_players()) {
            return Err(ServerError::Capacity {
                resource: Resource::Players,
                limit: usize::from(self.limits.max_players()),
                observed: self.occupied + 1,
            });
        }
        if self.current_sessions.iter().any(|key| {
            #[cfg(test)]
            CURRENT_DUPLICATE_VISITS.with(|visits| visits.set(visits.get() + 1));
            self.sessions[key].player_id == login.player_id()
        }) {
            return Err(ServerError::InvalidInput { field: "player_id" });
        }
        if self.ids_exhausted {
            return Err(ServerError::Capacity {
                resource: Resource::Players,
                limit: usize::from(self.limits.max_players()),
                observed: self.occupied + 1,
            });
        }
        let raw = self.next_session;
        let key = SessionKey::from_raw(raw).ok_or(ServerError::Internal {
            invariant: "session id",
        })?;
        if raw == u64::MAX {
            self.ids_exhausted = true;
        } else {
            self.next_session = raw + 1;
        }
        self.sessions.insert(
            key,
            SessionRecord {
                player_id: login.player_id(),
                display_name: login.display_name().as_str().to_owned(),
                phase,
                last_applied_sequence: 0,
                last_input_sequence: 0,
                next_arrival: 0,
                body: None,
                outbox: VecDeque::new(),
                outbox_closed: false,
            },
        );
        self.current_sessions.insert(key);
        self.occupied += 1;
        Ok(key)
    }
}

fn flush_family(
    io: &mut ShutdownIo<'_>,
    key: SaveKey,
    deadline: Deadline,
    report: &mut ShutdownReport,
) -> Result<(), ServerError> {
    let flushed = io.persistence.flush(key, deadline)?;
    report.durable = report.durable.saturating_add(flushed.durable);
    report.failed = report.failed.saturating_add(flushed.failed);
    report.outstanding = flushed.outstanding;
    Ok(())
}

/// Ports used by one shutdown attempt. Reports do not own snapshots.
pub struct ShutdownIo<'a> {
    pub reducer: &'a mut dyn FinalReducer,
    pub store: &'a mut dyn StoreHandle,
    pub agent: &'a mut dyn AgentHandle,
    pub snapshots: &'a mut dyn SnapshotPort,
    pub clock: &'a dyn Clock,
    pub workers: &'a mut dyn WorkerLifecycle,
    pub persistence: &'a mut dyn ActorPersistence,
    pub mcp: &'a mut dyn McpLifecycle,
    pub memory: &'a mut dyn MemoryFinalizer,
}

fn canonical_player(player_id: PlayerId, display_name: &str) -> Result<PlayerSave, ServerError> {
    let save = PlayerSave {
        player_id: mornlea_storage::PlayerId::from_bytes(player_id.bytes()),
        revision: 1,
        display_name: display_name.to_owned(),
        current: PlayerLocation {
            dimension: 0,
            position: [0.0, 64.0, 0.0],
        },
        yaw: 0.0,
        pitch: 0.0,
        safe: None,
        inventory: mornlea_storage::Inventory::default(),
        health: 20,
        hunger: 20,
        saturation_milli: 5_000,
        exhaustion_milli: 0,
        respawn_present: false,
        respawn_position: [0.0, 0.0, 0.0],
        respawn_dimension: 0,
        armor: [mornlea_storage::ItemStack::default(); 4],
    };
    mornlea_storage::player_encoded_len(&save)
        .map_err(|_| ServerError::InvalidInput { field: "player" })?;
    Ok(save)
}

fn save_from_stored(stored: StoredPlayer) -> Result<PlayerSave, ServerError> {
    let save = PlayerSave {
        player_id: stored.player_id,
        revision: stored.revision,
        display_name: stored.display_name,
        current: stored.current,
        yaw: stored.yaw,
        pitch: stored.pitch,
        safe: stored.safe,
        inventory: stored.inventory,
        health: stored.health,
        hunger: stored.hunger,
        saturation_milli: stored.saturation_milli,
        exhaustion_milli: stored.exhaustion_milli,
        respawn_present: stored.respawn_present,
        respawn_position: stored.respawn_position,
        respawn_dimension: stored.respawn_dimension,
        armor: stored.armor,
    };
    mornlea_storage::player_encoded_len(&save)
        .map_err(|_| ServerError::InvalidInput { field: "player" })?;
    Ok(save)
}

enum PendingFrame {
    One {
        session: SessionKey,
        frame: PreparedFrame,
    },
    Broadcast {
        frame: PreparedFrame,
    },
}

/// Encodes one packet with the protocol codec. A short buffer is resized to
/// the length the codec reports. Any other codec refusal is the existing
/// packet input error; this function does not choose a wire layout.
pub(crate) fn encode_packet(
    codec: &mut ProtocolCodec,
    packet: &ServerPacket,
) -> Result<Vec<u8>, ServerError> {
    let mut buffer = vec![0u8; 64];
    loop {
        match codec.encode_server_into(packet, &mut buffer) {
            Ok(written) => {
                buffer.truncate(written);
                // Identity travels with the immutable publication bytes so
                // every transport consumes the same canonical wire record.
                return mornlea_protocol::write_frame(packet.key().id, &buffer)
                    .map_err(|_| ServerError::InvalidInput { field: "packet" });
            }
            Err(ProtocolError::OutputTooSmall { needed, .. }) if needed > buffer.len() => {
                buffer.resize(needed, 0);
            }
            Err(_) => return Err(ServerError::InvalidInput { field: "packet" }),
        }
    }
}

impl SessionPort for AuthorityState {
    fn allocate(
        &mut self,
        login: AdmittedLogin,
        kind: TransportKind,
    ) -> Result<SessionKey, ServerError> {
        AuthorityState::allocate(self, login, kind)
    }
    fn prepare(
        &mut self,
        login: AdmittedLogin,
        kind: TransportKind,
    ) -> Result<SessionKey, ServerError> {
        AuthorityState::prepare(self, login, kind)
    }
    fn install(
        &mut self,
        session: SessionKey,
        loaded: Option<StoredPlayer>,
    ) -> Result<(), ServerError> {
        AuthorityState::install(self, session, loaded)
    }
    fn activate(&mut self, session: SessionKey) -> Result<(), ServerError> {
        AuthorityState::activate(self, session)
    }
    fn session(&self, key: SessionKey) -> Option<SessionFacts> {
        AuthorityState::session(self, key)
    }
    fn retire(&mut self, key: SessionKey, reason: CloseReason) -> Result<(), ServerError> {
        AuthorityState::retire(self, key, reason)
    }
    fn accept(
        &mut self,
        session: SessionKey,
        intent: PlayIntent,
    ) -> Result<SubmissionReceipt, ServerError> {
        AuthorityState::accept(self, session, intent)
    }
    fn apply_sequence(&mut self, session: SessionKey, sequence: u64) -> bool {
        AuthorityState::apply_sequence(self, session, sequence)
    }
}

impl MailboxPort for AuthorityState {
    fn freeze_eligible(&mut self, tick: u64) -> Vec<CommandEnvelope> {
        AuthorityState::freeze_eligible(self, tick)
    }
    fn carry(&mut self, batch: Vec<CommandEnvelope>) -> Result<(), ServerError> {
        AuthorityState::carry(self, batch)
    }
    fn admit_chunk(&mut self, result: ChunkResult) -> Result<(), ChunkResult> {
        AuthorityState::admit_chunk(self, result)
    }
    fn drain_chunks(&mut self, max: usize) -> Vec<ChunkResult> {
        AuthorityState::drain_chunks(self, max)
    }
    fn cancel_chunk(&mut self, request: ChunkRequestId) {
        AuthorityState::cancel_chunk(self, request)
    }
    fn drain_companions(&mut self, max: usize) -> Vec<CompanionActionEnvelope> {
        AuthorityState::drain_companions(self, max)
    }
    fn enqueue_interaction(&mut self, value: AuthorityInteraction) -> Result<(), ServerError> {
        AuthorityState::enqueue_interaction(self, value)
    }
}

impl PreparedPublicationPort for AuthorityState {
    fn enqueue_prepared(
        &mut self,
        session: SessionKey,
        frame: PreparedFrame,
    ) -> Result<EnqueueOutcome, ServerError> {
        AuthorityState::enqueue_prepared(self, session, frame)
    }
    fn take_prepared_outbox(
        &mut self,
        session: SessionKey,
        max_frames: usize,
        max_bytes: usize,
    ) -> Result<Vec<PreparedFrame>, ServerError> {
        AuthorityState::take_prepared_outbox(self, session, max_frames, max_bytes)
    }
}

impl PublicationPort for AuthorityState {
    fn publish(&mut self, publication: TickPublication) -> Result<(), ServerError> {
        AuthorityState::publish(self, publication)
    }
    fn take_outbox(
        &mut self,
        session: SessionKey,
        max_frames: usize,
        max_bytes: usize,
    ) -> Result<Vec<Vec<u8>>, ServerError> {
        AuthorityState::take_outbox(self, session, max_frames, max_bytes)
    }
    fn close_outbox(&mut self, session: SessionKey, reason: CloseReason) {
        AuthorityState::close_outbox(self, session, reason)
    }
}

impl LifecyclePort for AuthorityState {
    fn begin_close(&mut self) -> bool {
        AuthorityState::begin_close(self)
    }
    fn run_final(&mut self, reducer: &mut dyn FinalReducer) -> Result<u64, ServerError> {
        AuthorityState::run_final(self, reducer)
    }
    fn freeze(&mut self) -> FrozenAuthority {
        AuthorityState::freeze(self)
    }
    fn shutdown_progress(&self) -> ShutdownReport {
        AuthorityState::shutdown_progress(self)
    }
    fn record_progress(&mut self, report: ShutdownReport) {
        AuthorityState::record_progress(self, report)
    }
    fn mark_closed(&mut self) {
        AuthorityState::mark_closed(self)
    }
}

impl SaveAuthority for AuthorityState {
    fn select(&mut self, mode: SaveMode, budget: SaveBudget) -> Vec<OwnedSnapshot> {
        AuthorityState::select(self, mode, budget)
    }
    fn return_dirty(&mut self, snapshot: OwnedSnapshot) {
        AuthorityState::return_dirty(self, snapshot)
    }
    fn apply_completion(&mut self, completion: SaveCompletion) -> AckReport {
        AuthorityState::apply_completion(self, completion)
    }
    fn save_stats(&self) -> SaveStats {
        AuthorityState::save_stats(self)
    }
    fn metadata_snapshot(&self) -> OwnedSnapshot {
        AuthorityState::metadata_snapshot(self)
    }
    fn try_metadata_snapshot(&mut self) -> Result<OwnedSnapshot, ServerError> {
        AuthorityState::try_metadata_snapshot(self)
    }
}

/// Borrowed read surface. `None` means the value is unavailable, not air.
#[derive(Clone, Copy)]
pub struct AuthorityReadView<'a> {
    acquisition: Option<&'a AcquisitionState>,
    tick: u64,
    world: Option<WorldState>,
    commands: &'a [CommandEnvelope],
    companions: &'a [CompanionActionEnvelope],
    interactions: &'a [AuthorityInteraction],
    inventories: &'a BTreeMap<ActorKey, InventoryRecord>,
    blocks: &'a ChunkBlockObservations,
    ready: &'a BTreeMap<ChunkKey, ReadyChunk>,
    actors: &'a [ActorRecord],
    runtimes: &'a BTreeMap<ActorKey, ActorRuntime>,
    mining: &'a BTreeMap<ActorKey, MiningProgress>,
    pre_step: &'a BTreeMap<ActorKey, MotionState>,
    environment: Option<&'a EnvironmentState>,
    containers: &'a BTreeMap<ContainerRef, ContainerRecord>,
    container_chunks: &'a BTreeMap<ChunkKey, ContainerState>,
    viewers: &'a BTreeMap<SessionKey, ViewLease>,
    drops: &'a BTreeMap<ChunkKey, DropState>,
    projectiles: &'a [ProjectileRecord],
    damage_intents: &'a [DamageIntent],
    metadata: &'a mornlea_storage::Metadata,
    observation_trace: Option<&'a RefCell<ObservationTrace>>,
}

/// One resolver owns this bounded trace; ordinary authority reads do not record.
#[derive(Default)]
pub(crate) struct ObservationTrace {
    cells: BTreeMap<(Dimension, mornlea_domain::BlockPos), Option<BlockObservation>>,
    overflow: bool,
}

impl ObservationTrace {
    fn record(
        &mut self,
        dimension: Dimension,
        pos: mornlea_domain::BlockPos,
        value: Option<BlockObservation>,
    ) -> bool {
        if self.overflow {
            return false;
        }
        if !self.cells.contains_key(&(dimension, pos)) && self.cells.len() >= 512 {
            self.overflow = true;
            return false;
        }
        self.cells.entry((dimension, pos)).or_insert(value);
        true
    }

    pub(crate) fn check_capacity(&self) -> Result<(), RuleReject> {
        if self.overflow {
            Err(RuleReject::ResourceFull(Resource::RuleEffects))
        } else {
            Ok(())
        }
    }
}

/// Private cumulative output preview: failed attempts never consume capacity
/// or mutate the immutable authority base. One player death admits at most
/// forty slot batches; final compound staging remains the publication owner.
pub(crate) struct DropRehearsal<'a> {
    acquisition: Option<&'a AcquisitionState>,
    drops: &'a BTreeMap<ChunkKey, DropState>,
    ready: &'a BTreeMap<ChunkKey, ReadyChunk>,
    pending: BTreeMap<ChunkKey, DropState>,
}

impl DropRehearsal<'_> {
    pub(crate) fn try_insert(&mut self, batch: &DropBatch) -> Result<(), RuleReject> {
        drop_store::validate_batch(batch)?;
        let (key, _) = drop_store::batch_location(batch)?;
        if self.acquisition.is_some_and(|book| !book.available(key)) {
            return Err(RuleReject::StaleObservation);
        }
        let mut next = self
            .pending
            .get(&key)
            .or_else(|| self.drops.get(&key))
            .ok_or(RuleReject::StaleObservation)?
            .clone();
        next.insert(key, batch)?;
        if next.dirty
            && self
                .ready
                .get(&key)
                .is_some_and(|chunk| chunk.revision == u64::MAX)
        {
            return Err(RuleReject::StaleObservation);
        }
        // Install only a complete successful preview, including revision admission.
        self.pending.insert(key, next);
        Ok(())
    }
}

impl<'a> AuthorityReadView<'a> {
    fn available(&self, key: ChunkKey) -> bool {
        self.acquisition.is_none_or(|book| book.available(key))
    }
    /// A shorter borrowed view keeps the collector local to one resolution.
    pub(crate) fn with_observation_trace<'b>(
        &'b self,
        trace: &'b RefCell<ObservationTrace>,
    ) -> AuthorityReadView<'b> {
        let mut view = *self;
        view.observation_trace = Some(trace);
        view
    }

    pub(crate) fn mutation_basis(
        &self,
        actor: ActorKey,
        trace: &ObservationTrace,
    ) -> Result<MutationReadBasis, RuleReject> {
        trace.check_capacity()?;
        let record = self.actor(actor).ok_or(RuleReject::StaleObservation)?;
        if record.lifecycle != ActorLifecycle::Active {
            return Err(RuleReject::StaleObservation);
        }
        let environment = self.environment().ok_or(RuleReject::StaleObservation)?;
        Ok(MutationReadBasis {
            actor,
            dimension: record.dimension,
            motion: record.motion,
            look: record.look,
            seed: environment.seed,
            tunables: environment.tunables,
            cells: trace
                .cells
                .iter()
                .map(|((dimension, pos), observed)| (*dimension, *pos, *observed))
                .collect(),
        })
    }
    /// Sparse fixture cells alone do not establish a Ready chunk.
    pub fn ready_chunk(&self, key: ChunkKey) -> bool {
        self.available(key) && self.ready.contains_key(&key)
    }
    /// Exact Ready identity including accepted work in this tick. Sparse
    /// observations cannot supply a revision for an unavailable chunk.
    pub fn ready_chunk_revision(&self, key: ChunkKey) -> Option<u64> {
        if !self.available(key) {
            return None;
        }
        let chunk = self.ready.get(&key)?;
        Some(
            chunk.pending_revision(
                self.drops.get(&key).is_some_and(|state| state.dirty)
                    || self
                        .container_chunks
                        .get(&key)
                        .is_some_and(|state| state.dirty),
            ),
        )
    }
    /// Ready-chunk keys in deterministic key order for ring-ordered scans
    /// such as death drops. The set is bounded by chunk-result caps, so
    /// collecting it never scans the world.
    pub fn ready_chunk_keys(&self) -> Vec<ChunkKey> {
        self.ready
            .keys()
            .copied()
            .filter(|key| self.available(*key))
            .collect()
    }
    /// World spawn anchor for death reset teleport when the actor carries no
    /// bed respawn. `None` only on corrupt metadata, which providers refuse.
    pub fn spawn_anchor(&self) -> Option<(Dimension, mornlea_domain::ChunkPos)> {
        let dimension = Dimension::new(u8::try_from(self.metadata.spawn_dimension).ok()?).ok()?;
        Some((
            dimension,
            mornlea_domain::ChunkPos::new(
                self.metadata.spawn_anchor.x,
                self.metadata.spawn_anchor.z,
            ),
        ))
    }
    /// The source height map counts every non-air cell, including transparent blocks.
    pub fn highest_non_air(&self, dimension: Dimension, x: i32, z: i32) -> Option<i32> {
        let key = block_key(dimension, mornlea_domain::BlockPos::new(x, 0, z));
        if !self.available(key) {
            return None;
        }
        self.ready.get(&key).map(|chunk| chunk.height(x, z))
    }
    pub fn tick(&self) -> u64 {
        self.tick
    }
    pub fn world_time(&self) -> u64 {
        self.world
            .map(|world| world.world_time_ticks())
            .unwrap_or(0)
    }
    pub fn world(&self) -> Option<WorldState> {
        self.world
    }
    pub fn commands(&self) -> &'a [CommandEnvelope] {
        self.commands
    }
    pub fn companion_actions(&self) -> &'a [CompanionActionEnvelope] {
        self.companions
    }
    pub fn interactions(&self) -> &'a [AuthorityInteraction] {
        self.interactions
    }
    pub fn inventory(&self, key: ActorKey) -> Option<&'a InventoryRecord> {
        self.inventories.get(&key)
    }
    pub fn block(&self, dimension: Dimension, pos: mornlea_domain::BlockPos) -> Option<u16> {
        self.observation(dimension, pos)
            .map(|observed| observed.block)
    }
    /// Exact chunk/cell indexing observes this tick's writes before the compact
    /// immutable base. Missing data remains unavailable rather than inferred air.
    pub fn observation(
        &self,
        dimension: Dimension,
        pos: mornlea_domain::BlockPos,
    ) -> Option<BlockObservation> {
        let observed = self.untracked_observation(dimension, pos);
        if let Some(trace) = self.observation_trace
            && !trace.borrow_mut().record(dimension, pos, observed)
        {
            return None;
        }
        observed
    }

    fn untracked_observation(
        &self,
        dimension: Dimension,
        pos: mornlea_domain::BlockPos,
    ) -> Option<BlockObservation> {
        if !(-64..320).contains(&pos.y()) {
            return None;
        }
        let key = block_key(dimension, pos);
        if !self.available(key) {
            return None;
        }
        if let Some(observed) = self.blocks.get(&(key, pos)) {
            return Some(*observed);
        }
        let chunk = self.ready.get(&key)?;
        Some(BlockObservation {
            key,
            generation: chunk.generation,
            revision: chunk.revision,
            pos,
            block: chunk.block(pos)?,
        })
    }
    /// Fixture-backed container lookup by exact reference. Real container
    /// records, generation validation and viewer leases belong to the
    /// container provider; this surface only lets a resolver capture the
    /// staged slots of a mined container.
    pub fn container(&self, reference: ContainerRef) -> Option<ContainerRecord> {
        self.world_container(Dimension::OVERWORLD, reference)
    }
    /// Internal simulation keeps the chunk dimension even when wire views cannot.
    pub fn world_container(
        &self,
        dimension: Dimension,
        reference: ContainerRef,
    ) -> Option<ContainerRecord> {
        let key = ChunkKey {
            dimension,
            pos: reference.chunk(),
        };
        if !self.available(key) {
            return None;
        }
        if let Some(chunk) = self.container_chunks.get(&key) {
            return chunk.record(key, reference);
        }
        if dimension == Dimension::OVERWORLD {
            self.containers.get(&reference).cloned()
        } else {
            None
        }
    }

    /// Ready records resolve the actual stored position, slot and generation.
    pub fn container_at(
        &self,
        dimension: Dimension,
        pos: mornlea_domain::BlockPos,
        kind: mornlea_domain::ContainerKind,
    ) -> Option<ContainerRecord> {
        let key = block_key(dimension, pos);
        if !self.available(key) {
            return None;
        }
        if let Some(chunk) = self.container_chunks.get(&key) {
            return chunk.at(key, pos, kind);
        }
        // Sparse fixtures retain their explicit historical first-slot convention.
        let reference = ContainerRef::try_new(key.pos, kind, 0, 1).ok()?;
        self.world_container(dimension, reference)
    }

    /// Fixed array enumeration is bounded independently of total loaded chunks.
    pub fn container_refs(&self, key: ChunkKey) -> Vec<ContainerRef> {
        if !self.available(key) {
            return Vec::new();
        }
        self.container_chunks
            .get(&key)
            .map(|chunk| chunk.references(key))
            .unwrap_or_default()
    }

    /// The context's complete net set is the sole viewer authority this tick.
    pub fn viewer(&self, session: SessionKey) -> Option<ViewLease> {
        self.viewers.get(&session).copied()
    }
    /// Immutable active slots in physical slot order. Fixed inactive generations
    /// stay private to the owner and survive empty active observations.
    pub fn drops(&self, key: ChunkKey) -> &[DropRecord] {
        if !self.available(key) {
            return &[];
        }
        self.drops.get(&key).map(DropState::records).unwrap_or(&[])
    }
    /// Cumulative preview borrows the base and retains only successful chunk copies.
    pub(crate) fn drop_rehearsal(&self) -> DropRehearsal<'a> {
        DropRehearsal {
            acquisition: self.acquisition,
            drops: self.drops,
            ready: self.ready,
            pending: BTreeMap::new(),
        }
    }
    /// Rehearse the entire output on one fixed slot copy; commit must recheck it.
    pub fn check_drop_batch(&self, batch: &DropBatch) -> Result<(), RuleReject> {
        self.drop_rehearsal().try_insert(batch)
    }
    /// Immutable staged projectile records; providers cannot bypass compare-and-replace.
    pub fn projectiles(&self) -> &'a [ProjectileRecord] {
        self.projectiles
    }
    /// Ordered hit intents retained until the damage settlement phase.
    pub fn damage_intents(&self) -> &'a [DamageIntent] {
        self.damage_intents
    }
    pub fn actors(&self) -> &'a [ActorRecord] {
        self.actors
    }
    pub fn actor(&self, key: ActorKey) -> Option<&'a ActorRecord> {
        self.actors.iter().find(|actor| actor.key == key)
    }
    /// Provider-staged per-actor runtime record, if any. The motion provider
    /// is the single writer of the held-controls lane; every other lane
    /// belongs to its own provider and the serial reducer merges per field.
    pub fn runtime(&self, key: ActorKey) -> Option<&'a ActorRuntime> {
        self.runtimes.get(&key)
    }
    /// Provider-staged mining progress of one actor, if any. The mining
    /// provider is the single writer through the `Mining` effect; progress is
    /// transient tick state and never a save record.
    pub fn mining(&self, key: ActorKey) -> Option<&'a MiningProgress> {
        self.mining.get(&key)
    }
    /// Pre-motion pose snapshotted at construction, if this actor was loaded
    /// then. Motion providers overwrite the live record in place, so only
    /// this snapshot names the step-start pose a post-step pass may compare
    /// against.
    pub fn pre_step_motion(&self, key: ActorKey) -> Option<MotionState> {
        self.pre_step.get(&key).copied()
    }
    pub fn environment(&self) -> Option<&'a EnvironmentState> {
        self.environment
    }
}

/// Isolated staging context. Only the step module and the test harness construct it.
pub struct TickContext<'a> {
    authority: &'a mut AuthorityState,
    /// Some records an exclusive resident loan and its original sleep presence.
    resident_loan: Option<bool>,
    sleep_record_touched: bool,
    dirty_chunks: BTreeSet<ChunkKey>,
    inventories: BTreeMap<ActorKey, InventoryRecord>,
    blocks: ChunkBlockObservations,
    /// Successful block mutations only, keyed in source chunk/index order.
    changed: BTreeMap<(ChunkKey, u32), BlockObservation>,
    ready: BTreeMap<ChunkKey, ReadyChunk>,
    containers: BTreeMap<ContainerRef, ContainerRecord>,
    container_chunks: BTreeMap<ChunkKey, ContainerState>,
    /// Staged viewer leases by session. The container provider is the single
    /// writer through the viewer staging arm; the serial reducer commits the
    /// net overlay into the authority store at overlay commit.
    viewers: BTreeMap<SessionKey, ViewLease>,
    drops: BTreeMap<ChunkKey, DropState>,
    world: Option<WorldState>,
    commands: Vec<CommandEnvelope>,
    companions: Vec<CompanionActionEnvelope>,
    interactions: Vec<AuthorityInteraction>,
    actors: Vec<ActorRecord>,
    player_slots: BTreeMap<SessionKey, usize>,
    runtimes: BTreeMap<ActorKey, ActorRuntime>,
    mining: BTreeMap<ActorKey, MiningProgress>,
    environment: Option<EnvironmentState>,
    /// Reducer-carried sleep record: the bed anchors plus the staged display
    /// offset. The sleep provider is the single writer through the `Sleep`
    /// staging arm; the serial reducer threads the record across the entry
    /// and settlement calls and persists the settled copy here.
    sleep_record: SleepState,
    /// Reducer-owned sleeping sessions in ascending order. Settlement shrinks
    /// the set on wakes and the transition; a successful bed entry grows it.
    sleeping: BTreeSet<SessionKey>,
    budget: TickBudget,
    spent_commands: usize,
    spent_fluid: [usize; 2],
    spent_rescan: [usize; 2],
    spent_farmland_checks: usize,
    spent_farmland_reads: usize,
    spent_effects: usize,
    spent_snapshot_chunks: usize,
    spent_snapshot_bytes: usize,
    events: Vec<RoutedEvent>,
    projectiles: Vec<ProjectileRecord>,
    damage_intents: Vec<DamageIntent>,
    deferred: Vec<(RulePhase, CommandEnvelope)>,
    charges: Vec<(ActorKey, ActionKind)>,
    suppressed_mining: BTreeSet<ActorKey>,
    /// Pre-motion actor poses, snapshotted once at construction from the
    /// loaded actors and never written after. The reducer constructs one
    /// context per tick from pre-motion authority, so the snapshot is
    /// pre-step by construction; providers that need the step-start pose
    /// (jump takeoffs, swim displacement) read it here instead of
    /// re-deriving physics. No rollback entry: compounds never touch it.
    pre_step: BTreeMap<ActorKey, MotionState>,
}

/// Compound-entry preimages own only affected keys. Fixed slot rehearsals stay
/// outside this journal and publish only after the complete apply succeeds.
#[derive(Default)]
struct CompoundUndo {
    inventories: BTreeMap<ActorKey, Option<InventoryRecord>>,
    blocks: BTreeMap<(ChunkKey, mornlea_domain::BlockPos), Option<BlockObservation>>,
    changed: BTreeMap<(ChunkKey, u32), Option<BlockObservation>>,
    ready: BTreeMap<ChunkKey, Option<ReadyChunk>>,
    containers: BTreeMap<ContainerRef, Option<ContainerRecord>>,
    viewers: BTreeMap<SessionKey, Option<ViewLease>>,
    runtimes: BTreeMap<ActorKey, Option<ActorRuntime>>,
    mining: BTreeMap<ActorKey, Option<MiningProgress>>,
    player_slots: BTreeMap<SessionKey, Option<usize>>,
    dirty_chunks: BTreeMap<ChunkKey, bool>,
    actors_len: usize,
    actors: BTreeMap<ActorKey, Option<(usize, ActorRecord)>>,
    world: Option<Option<WorldState>>,
    environment: Option<Option<EnvironmentState>>,
    sleep: Option<(SleepState, bool)>,
    projectiles: Option<Vec<ProjectileRecord>>,
    damage_len: Option<usize>,
}

impl CompoundUndo {
    fn capture(context: &TickContext<'_>, parts: &[RuleEffect]) -> Self {
        let mut undo = Self {
            actors_len: context.actors.len(),
            ..Self::default()
        };
        for part in parts {
            match part {
                RuleEffect::Actor(record) => {
                    undo.actors.entry(record.key).or_insert_with(|| {
                        context
                            .actors
                            .iter()
                            .enumerate()
                            .find(|(_, actor)| actor.key == record.key)
                            .map(|(index, actor)| (index, actor.clone()))
                    });
                    if let ActorKey::Player(session) = record.key {
                        undo.player_slots
                            .entry(session)
                            .or_insert_with(|| context.player_slots.get(&session).copied());
                    }
                }
                RuleEffect::Runtime(record) => {
                    undo.runtimes
                        .entry(record.key)
                        .or_insert_with(|| context.runtimes.get(&record.key).cloned());
                }
                RuleEffect::Mining { actor, .. } => {
                    undo.mining
                        .entry(*actor)
                        .or_insert_with(|| context.mining.get(actor).cloned());
                }
                RuleEffect::Inventory(patch) => undo.capture_inventory(context, patch.actor),
                RuleEffect::Container { after, .. } => {
                    undo.capture_container(context, Dimension::OVERWORLD, after.reference)
                }
                RuleEffect::WorldContainer {
                    dimension, after, ..
                } => undo.capture_container(context, *dimension, after.reference),
                RuleEffect::Viewer { session, .. } => {
                    undo.viewers
                        .entry(*session)
                        .or_insert_with(|| context.viewers.get(session).copied());
                }
                RuleEffect::World(_) => {
                    undo.world.get_or_insert(context.world);
                }
                RuleEffect::Blocks(txn) => {
                    if let Some(patch) = &txn.inventory {
                        undo.capture_inventory(context, patch.actor);
                    }
                    for capture in &txn.containers {
                        if capture.key.dimension == Dimension::OVERWORLD
                            && !context.container_chunks.contains_key(&capture.key)
                        {
                            undo.containers
                                .entry(capture.record.reference)
                                .or_insert_with(|| {
                                    context.containers.get(&capture.record.reference).cloned()
                                });
                        }
                    }
                    for write in &txn.writes {
                        if write.replacement == write.observed.block {
                            continue;
                        }
                        let key = write.observed.key;
                        let cell = (key, write.observed.pos);
                        let changed = (key, mornlea_domain::chunk_block_index(write.observed.pos));
                        undo.blocks
                            .entry(cell)
                            .or_insert_with(|| context.blocks.get(&cell).copied());
                        undo.changed
                            .entry(changed)
                            .or_insert_with(|| context.changed.get(&changed).copied());
                        undo.ready
                            .entry(key)
                            .or_insert_with(|| context.ready.get(&key).cloned());
                        undo.dirty_chunks
                            .entry(key)
                            .or_insert_with(|| context.dirty_chunks.contains(&key));
                    }
                }
                RuleEffect::Damage(_) => {
                    undo.damage_len.get_or_insert(context.damage_intents.len());
                }
                RuleEffect::Projectile { .. } => {
                    undo.projectiles
                        .get_or_insert_with(|| context.projectiles.clone());
                }
                RuleEffect::Environment(_) => {
                    undo.environment
                        .get_or_insert_with(|| context.environment.clone());
                }
                RuleEffect::Sleep(_) => {
                    undo.sleep.get_or_insert_with(|| {
                        (context.sleep_record.clone(), context.sleep_record_touched)
                    });
                }
                RuleEffect::Drops(_) | RuleEffect::DropPatch { .. } | RuleEffect::Work(_) => {}
                RuleEffect::Compound(_) => unreachable!("compound shape checked before capture"),
            }
        }
        undo
    }

    fn capture_inventory(&mut self, context: &TickContext<'_>, actor: ActorKey) {
        self.inventories
            .entry(actor)
            .or_insert_with(|| context.inventories.get(&actor).copied());
    }

    fn capture_container(
        &mut self,
        context: &TickContext<'_>,
        dimension: Dimension,
        reference: ContainerRef,
    ) {
        let key = ChunkKey {
            dimension,
            pos: reference.chunk(),
        };
        if dimension == Dimension::OVERWORLD && !context.container_chunks.contains_key(&key) {
            self.containers
                .entry(reference)
                .or_insert_with(|| context.containers.get(&reference).cloned());
        }
    }

    fn restore(self, context: &mut TickContext<'_>) {
        restore_preimages(&mut context.inventories, self.inventories);
        // Restore only journaled cells; removal also retires an empty sparse owner.
        for (cell, original) in self.blocks {
            if let Some(original) = original {
                context.blocks.insert(cell, original);
            } else {
                context.blocks.remove(&cell);
            }
        }
        restore_preimages(&mut context.changed, self.changed);
        restore_preimages(&mut context.ready, self.ready);
        restore_preimages(&mut context.containers, self.containers);
        restore_preimages(&mut context.viewers, self.viewers);
        restore_preimages(&mut context.runtimes, self.runtimes);
        restore_preimages(&mut context.mining, self.mining);
        // Actor application replaces in place or appends, never removes.
        context.actors.truncate(self.actors_len);
        for (index, record) in self.actors.into_values().flatten() {
            context.actors[index] = record;
        }
        restore_preimages(&mut context.player_slots, self.player_slots);
        for (key, dirty) in self.dirty_chunks {
            if dirty {
                context.dirty_chunks.insert(key);
            } else {
                context.dirty_chunks.remove(&key);
            }
        }
        if let Some(world) = self.world {
            context.world = world;
        }
        if let Some(environment) = self.environment {
            context.environment = environment;
        }
        if let Some((record, touched)) = self.sleep {
            context.sleep_record = record;
            context.sleep_record_touched = touched;
        }
        if let Some(projectiles) = self.projectiles {
            context.projectiles = projectiles;
        }
        if let Some(length) = self.damage_len {
            context.damage_intents.truncate(length);
        }
    }
}

fn restore_preimages<K: Ord, V>(target: &mut BTreeMap<K, V>, preimages: BTreeMap<K, Option<V>>) {
    for (key, value) in preimages {
        if let Some(value) = value {
            target.insert(key, value);
        } else {
            target.remove(&key);
        }
    }
}

/// Bound independent private producer vectors before rehearsal or journal capture.
fn check_effect_work(effect: &RuleEffect) -> Result<(), RuleReject> {
    let rejection = RuleReject::ResourceFull(Resource::RuleEffects);
    let parts = match effect {
        RuleEffect::Compound(parts) if parts.len() <= EFFECT_BUDGET => parts.as_slice(),
        RuleEffect::Compound(_) => return Err(rejection),
        _ => std::slice::from_ref(effect),
    };
    let mut totals = [0usize; 3];
    for part in parts {
        if matches!(part, RuleEffect::Compound(_)) {
            return Err(rejection);
        }
        if let RuleEffect::Blocks(txn) = part {
            let lengths = [
                txn.writes.len(),
                txn.containers.len(),
                txn.read_basis.as_ref().map_or(0, |basis| basis.cells.len()),
            ];
            for (total, length) in totals.iter_mut().zip(lengths) {
                *total = total
                    .checked_add(length)
                    .filter(|sum| *sum <= EFFECT_BUDGET)
                    .ok_or(rejection)?;
            }
        }
    }
    Ok(())
}

/// Exhaustion charge receipt one action provider notes for the survival
/// provider to settle.
///
/// Mining and till receipts fire exactly on their successful completion forks
/// (refused or interrupted work notes nothing); melee receipts fire exactly on
/// a successful hit. Each receipt settles through the shared threshold loop at
/// the next post-physics pass, so writers never touch hunger state directly.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ActionKind {
    Mining,
    Till,
    Melee,
}

impl<'a> TickContext<'a> {
    /// Consumes at most sixteen prepared owners at the existing Acquire row.
    pub(crate) fn apply_live_acquisition(&mut self) -> PhaseReport {
        let events = self.authority.acquisition.drain();
        #[cfg(test)]
        let observed_key = events.first().map(|e| match e {
            AcquiredChunkEvent::Load { key, .. } | AcquiredChunkEvent::Generated { key, .. } => {
                *key
            }
        });
        #[cfg(test)]
        let before = observed_key.is_some_and(|key| self.read().ready_chunk(key));
        let mut report = PhaseReport {
            examined: events.len(),
            applied: 0,
            rejected: 0,
            carried: 0,
        };
        for event in events {
            if !self.authority.acquisition.settle(&event) {
                report.rejected += 1;
                continue;
            }
            let (key, result) = match event {
                AcquiredChunkEvent::Load { key, result, .. } => (key, result),
                AcquiredChunkEvent::Generated { key, result, .. } => (key, result.map(Some)),
            };
            match result {
                Ok(None) => {
                    self.authority.acquisition.missing(key);
                    report.applied += 1;
                }
                Ok(Some(prepared)) => {
                    let (ready, drops, containers, persisted, rewrite, recovered) =
                        prepared.into_live_parts();
                    self.authority.acquisition.installed(
                        key,
                        ready.revision,
                        persisted,
                        rewrite,
                        recovered,
                        ready.payload_estimate(),
                    );
                    self.ready.insert(key, ready);
                    self.drops.insert(key, drops);
                    self.container_chunks.insert(key, containers);
                    report.applied += 1;
                }
                Err(error) => {
                    self.authority.acquisition.failed(key, error);
                    report.rejected += 1;
                }
            }
        }
        #[cfg(test)]
        if let Some(key) = observed_key {
            ACQUIRE_AVAILABILITY
                .with(|value| value.set(Some((key, before, self.read().ready_chunk(key)))));
        }
        report
    }

    /// Installs already validated compact data; preparation belongs off the tick.
    pub fn preload_ready_chunk(&mut self, chunk: ReadyChunk) {
        // Fixture replacement drops only this chunk's sparse owner off tick.
        self.blocks.take_chunk(chunk.key);
        self.changed.retain(|(key, _), _| *key != chunk.key);
        self.dirty_chunks.remove(&chunk.key);
        self.drops.insert(
            chunk.key,
            DropState::new(
                chunk.key,
                chunk
                    .drop_slots()
                    .try_into()
                    .expect("Ready chunks own exactly 32 drop slots"),
            ),
        );
        self.container_chunks
            .insert(chunk.key, chunk.container_state());
        self.ready.insert(chunk.key, chunk);
    }
    pub fn harness(authority: &'a mut AuthorityState, budget: TickBudget) -> Self {
        Self::from_parts(authority, budget)
    }

    /// Production tick context. The serial reducer owns the only call; the
    /// frozen tick inputs (mailbox batch, companion feed, environment
    /// snapshot) arrive through the narrow ports below, never through this
    /// constructor.
    pub(crate) fn for_tick(authority: &'a mut AuthorityState, budget: TickBudget) -> Self {
        let mut context = Self::from_parts(authority, budget);
        // The exclusive authority borrow prevents observers from reading an
        // incomplete resident set while providers own and mutate these maps.
        let residents = std::mem::take(&mut context.authority.residents);
        context.resident_loan = Some(residents.sleep_record.is_some());
        context.actors = residents.actors;
        context.player_slots = residents.player_slots;
        context.runtimes = residents.runtimes;
        context.inventories = residents.inventories;
        context.mining = residents.mining;
        context.projectiles = residents.projectiles;
        context.environment = residents.environment;
        context.sleeping = residents.sleeping;
        context.blocks = residents.blocks;
        context.ready = residents.ready;
        context.drops = residents.drops;
        context.containers = residents.containers;
        context.container_chunks = residents.container_chunks;
        context.dirty_chunks = residents.dirty_chunks;
        if let Some(record) = residents.sleep_record {
            context.sleep_record = record;
        }
        for actor in &context.actors {
            context.pre_step.insert(actor.key, actor.motion);
        }
        context
    }

    /// Freezes the tick-start climate snapshot every provider consumes.
    /// Metadata seeds the first snapshot; later ticks retain committed climate
    /// progression and sleep display offsets. Only the executing tick and
    /// checked source tunables refresh at this boundary, before any provider
    /// runs. The providers share that frozen record without durability reads.
    pub(crate) fn freeze_environment(&mut self, tick: u64) {
        let mut frozen = self.environment.clone().unwrap_or_else(|| {
            let metadata = &self.authority.metadata;
            let seed = self.authority.world_seed;
            EnvironmentState {
                seed,
                next_tick: tick,
                world_time: metadata.world_time_ticks,
                // The stored offset is the metadata u64; the snapshot carries
                // the u16 display range with the same wrapping conversion the
                // source restore boundary applies.
                day_phase_offset: metadata.day_phase_offset as u16,
                season_offset: season_offset_from_seed(seed),
                weather: Weather::try_new(metadata.weather_kind).unwrap_or(Weather::Clear),
                weather_remaining: metadata.weather_ticks_remaining,
                difficulty: metadata.difficulty,
                tunables: RuleTunables::source_defaults(),
            }
        });
        frozen.next_tick = tick;
        frozen.tunables = RuleTunables::source_defaults();
        self.environment = Some(frozen);
    }

    pub fn from_fixture(
        authority: &'a mut AuthorityState,
        initial: &FixtureState,
        budget: TickBudget,
    ) -> Self {
        let mut context = Self::from_parts(authority, budget);
        context.world = Some(initial.world);
        for (key, generation, revision, chunk) in &initial.chunks {
            context.preload_ready_chunk(
                ReadyChunk::try_new(*key, *generation, *revision, chunk.clone())
                    .expect("fixture chunks must satisfy the storage contract"),
            );
        }
        for record in &initial.containers {
            context.preload_container(record.clone());
        }
        for record in &initial.drops {
            context.preload_drop(record.clone());
        }
        context.actors = initial.actors.clone();
        context.projectiles = initial.projectiles.clone();
        context.runtimes.extend(
            initial
                .runtime
                .iter()
                .cloned()
                .map(|record| (record.key, record)),
        );
        // The fixture's initial actors are the pre-motion poses, so the
        // snapshot taken here is pre-step by the same construction rule.
        for actor in &context.actors {
            context.pre_step.insert(actor.key, actor.motion);
        }
        context
            .inventories
            .extend(initial.inventories.iter().cloned());
        context
    }

    fn from_parts(authority: &'a mut AuthorityState, budget: TickBudget) -> Self {
        let viewers = authority.views.clone();
        Self {
            authority,
            resident_loan: None,
            sleep_record_touched: false,
            dirty_chunks: BTreeSet::new(),
            inventories: BTreeMap::new(),
            blocks: ChunkBlockObservations::new(),
            changed: BTreeMap::new(),
            ready: BTreeMap::new(),
            containers: BTreeMap::new(),
            container_chunks: BTreeMap::new(),
            viewers,
            drops: BTreeMap::new(),
            world: None,
            commands: Vec::new(),
            companions: Vec::new(),
            interactions: Vec::new(),
            actors: Vec::new(),
            player_slots: BTreeMap::new(),
            runtimes: BTreeMap::new(),
            mining: BTreeMap::new(),
            environment: None,
            sleep_record: SleepState::try_new(Vec::new(), 0, None)
                .expect("an empty sleep record satisfies the bed ceiling"),
            sleeping: BTreeSet::new(),
            budget,
            spent_commands: 0,
            spent_fluid: [0, 0],
            spent_rescan: [0, 0],
            spent_farmland_checks: 0,
            spent_farmland_reads: 0,
            spent_effects: 0,
            spent_snapshot_chunks: 0,
            spent_snapshot_bytes: 0,
            events: Vec::new(),
            projectiles: Vec::new(),
            damage_intents: Vec::new(),
            deferred: Vec::new(),
            charges: Vec::new(),
            suppressed_mining: BTreeSet::new(),
            pre_step: BTreeMap::new(),
        }
    }

    pub fn preload_inventory(&mut self, actor: ActorKey, record: InventoryRecord) {
        self.inventories.insert(actor, record);
    }

    /// Seeds a sparse-only fixture. Complete Ready fixtures use their compact
    /// chunk input so cell setup cannot bypass the derived height cache.
    pub fn preload_block(&mut self, observed: BlockObservation) {
        assert!(
            !self.ready.contains_key(&observed.key),
            "sparse fixture cells cannot override Ready chunks"
        );
        self.drops
            .entry(observed.key)
            .or_insert_with(|| DropState::new(observed.key, [Default::default(); 32]));
        self.blocks.insert((observed.key, observed.pos), observed);
    }

    /// Snapshots only committed changed cells; support passes each take their
    /// own snapshot so their writes cannot recursively extend that pass.
    pub fn changed_blocks(&self) -> Vec<BlockObservation> {
        self.changed.values().copied().collect()
    }

    /// Stages one container record for fixture-driven resolver preflight,
    /// replacing any earlier record with the same reference. Production
    /// container commits stage container deltas through the container
    /// staging arm instead; this entry point stays test setup only.
    pub fn preload_container(&mut self, record: ContainerRecord) {
        let key = ChunkKey {
            dimension: Dimension::OVERWORLD,
            pos: record.reference.chunk(),
        };
        if let Some(chunk) = self.container_chunks.get(&key) {
            assert_eq!(
                chunk.record(key, record.reference).as_ref(),
                Some(&record),
                "fixture record cannot override Ready container slots"
            );
        } else {
            self.containers.insert(record.reference, record);
        }
    }

    /// Stages one occupied drop slot for fixture-driven capacity preflight,
    /// filed under the chunk its drop identity names. A drop identity naming
    /// a dimension outside the supported pair has no chunk key and is
    /// ignored rather than silently re-homed.
    pub fn preload_drop(&mut self, record: DropRecord) {
        if let Some(dimension) = u8::try_from(record.id.dimension())
            .ok()
            .and_then(|raw| Dimension::new(raw).ok())
        {
            let key = ChunkKey {
                dimension,
                pos: record.id.chunk(),
            };
            self.drops
                .entry(key)
                .or_insert_with(|| DropState::new(key, [Default::default(); 32]))
                .seed(key, record)
                .expect("fixture drop must have a unique valid slot");
        }
    }

    /// Stages one mining progress record for fixture-driven progression,
    /// replacing any earlier record for the same actor. Progress is transient
    /// tick state owned by the mining provider; it never reaches a save.
    pub fn preload_mining(&mut self, progress: MiningProgress) {
        self.mining.insert(progress.actor, progress);
    }

    /// Stages one companion action envelope for fixture-driven intent reads,
    /// appended in arrival order. Intake validation and admission stay with
    /// the companion ingress owner; the tick reducer owns queue ordering.
    pub fn preload_companion_action(&mut self, action: CompanionActionEnvelope) {
        self.companions.push(action);
    }

    /// Production companion feed. The serial reducer pushes exactly the
    /// drained intake queue in drain order, so admission and bounds stay with
    /// the ingress owner and this mirrors the fixture preload with no second
    /// bound of its own.
    pub fn push_companion_action(&mut self, action: CompanionActionEnvelope) {
        self.companions.push(action);
    }

    pub fn read(&self) -> AuthorityReadView<'_> {
        AuthorityReadView {
            acquisition: self
                .authority
                .acquisition
                .enabled()
                .then_some(&self.authority.acquisition),
            tick: self.authority.next_tick,
            world: self.world,
            commands: &self.commands,
            companions: &self.companions,
            interactions: &self.interactions,
            inventories: &self.inventories,
            blocks: &self.blocks,
            ready: &self.ready,
            actors: &self.actors,
            runtimes: &self.runtimes,
            mining: &self.mining,
            pre_step: &self.pre_step,
            environment: self.environment.as_ref(),
            containers: &self.containers,
            container_chunks: &self.container_chunks,
            viewers: &self.viewers,
            drops: &self.drops,
            projectiles: &self.projectiles,
            damage_intents: &self.damage_intents,
            metadata: &self.authority.metadata,
            observation_trace: None,
        }
    }

    /// Returns the complete net set for the reducer's full replacement commit.
    pub fn viewer_leases(&self) -> BTreeMap<SessionKey, ViewLease> {
        self.viewers.clone()
    }

    /// Clones and finalizes a detached replay or harness observation.
    /// Production uses `commit_carried` to return ownership without a census.
    pub fn resident_snapshot(&self) -> ResidentTickState {
        let mut ready = self.ready.clone();
        let mut drops = self.drops.clone();
        let mut container_chunks = self.container_chunks.clone();
        // Commit revision metadata once without expanding compact storage.
        // Reset markers on the carried copies so a no-op next tick stays clean.
        for (key, chunk) in &mut ready {
            chunk.finish_tick(
                drops.get(key).is_some_and(|state| state.dirty)
                    || container_chunks.get(key).is_some_and(|state| state.dirty),
            );
            if let Some(state) = drops.get_mut(key) {
                state.dirty = false;
            }
            if let Some(state) = container_chunks.get_mut(key) {
                state.finish_tick(chunk.revision);
            }
        }
        ResidentTickState {
            actors: self.actors.clone(),
            player_slots: self.player_slots.clone(),
            runtimes: self.runtimes.clone(),
            inventories: self.inventories.clone(),
            mining: self.mining.clone(),
            projectiles: self.projectiles.clone(),
            environment: self.environment.clone(),
            sleep_record: Some(self.sleep_record.clone()),
            sleeping: self.sleeping.clone(),
            blocks: self.blocks.clone(),
            ready,
            drops,
            containers: self.containers.clone(),
            container_chunks,
            dirty_chunks: BTreeSet::new(),
        }
    }

    /// Finalizes only accepted dirty keys before returning the exclusive loan.
    /// Explicit commit preserves the reducer's successful Some sleep record.
    pub(crate) fn commit_carried(&mut self) {
        if self.resident_loan.is_none() {
            return;
        }
        for key in &self.dirty_chunks {
            let Some(chunk) = self.ready.get_mut(key) else {
                continue;
            };
            chunk.finish_tick(
                self.drops.get(key).is_some_and(|state| state.dirty)
                    || self
                        .container_chunks
                        .get(key)
                        .is_some_and(|state| state.dirty),
            );
            if let Some(state) = self.drops.get_mut(key) {
                state.dirty = false;
            }
            if let Some(state) = self.container_chunks.get_mut(key) {
                state.finish_tick(chunk.revision);
            }
        }
        for key in &self.dirty_chunks {
            if let Some(chunk) = self.ready.get(key) {
                self.authority.acquisition.committed(
                    *key,
                    chunk.generation,
                    chunk.revision,
                    chunk.payload_estimate(),
                );
            }
        }
        self.dirty_chunks.clear();
        self.return_carried(true);
    }

    /// Ownership recovery performs no finalization or provider work. Accepted
    /// writes and dirty identity survive abandonment for a later successful tick.
    fn return_carried(&mut self, committed: bool) {
        let Some(original_sleep) = self.resident_loan.take() else {
            return;
        };
        let sleep_record = if committed || original_sleep || self.sleep_record_touched {
            Some(std::mem::replace(
                &mut self.sleep_record,
                SleepState {
                    beds: Vec::new(),
                    day_phase_offset: 0,
                    pending_offset: None,
                },
            ))
        } else {
            None
        };
        self.authority.residents = ResidentTickState {
            actors: std::mem::take(&mut self.actors),
            player_slots: std::mem::take(&mut self.player_slots),
            runtimes: std::mem::take(&mut self.runtimes),
            inventories: std::mem::take(&mut self.inventories),
            mining: std::mem::take(&mut self.mining),
            projectiles: std::mem::take(&mut self.projectiles),
            environment: std::mem::take(&mut self.environment),
            sleeping: std::mem::take(&mut self.sleeping),
            blocks: std::mem::take(&mut self.blocks),
            ready: std::mem::take(&mut self.ready),
            drops: std::mem::take(&mut self.drops),
            containers: std::mem::take(&mut self.containers),
            container_chunks: std::mem::take(&mut self.container_chunks),
            dirty_chunks: std::mem::take(&mut self.dirty_chunks),
            sleep_record,
        };
    }

    /// Input acknowledgment precedes semantic validation and survives idle ticks.
    pub(crate) fn record_player_input(&mut self, envelope: &CommandEnvelope) {
        if !matches!(envelope.command(), mornlea_domain::Command::PlayerInput(_)) {
            return;
        }
        let Some(session) = SessionKey::from_raw(envelope.session()) else {
            return;
        };
        if !self
            .player_slots
            .get(&session)
            .and_then(|slot| self.actors.get(*slot))
            .is_some_and(|actor| {
                actor.key == ActorKey::Player(session) && actor.lifecycle == ActorLifecycle::Active
            })
        {
            return;
        }
        if let Some(record) = self.authority.sessions.get_mut(&session)
            && record.phase == SessionPhase::Active
        {
            record.last_input_sequence = envelope.sequence();
        }
    }

    /// Stages one login seed into the overlay. Latest-wins on a repeated key
    /// preserves the no-duplicate-actor invariant even against a carried
    /// record; the scan itself only emits sessions without one.
    pub fn stage_login(&mut self, seeded: SeededPlayer) {
        let SeededPlayer { actor, inventory } = seeded;
        let slot = match self
            .actors
            .iter()
            .position(|staged| staged.key == actor.key)
        {
            Some(index) => {
                self.actors[index] = actor.clone();
                index
            }
            None => {
                let index = self.actors.len();
                self.actors.push(actor.clone());
                index
            }
        };
        self.index_player_actor(actor.key, slot);
        self.pre_step.insert(actor.key, actor.motion);
        self.inventories.insert(actor.key, inventory);
    }

    /// Existing actor writes retain ordinals; only owned active sessions enter.
    fn index_player_actor(&mut self, key: ActorKey, slot: usize) {
        if let ActorKey::Player(session) = key
            && self
                .authority
                .sessions
                .get(&session)
                .is_some_and(|record| record.phase == SessionPhase::Active)
        {
            self.player_slots.insert(session, slot);
        }
    }

    /// Reducer-carried sleep record under settlement.
    pub fn sleep_record(&self) -> &SleepState {
        &self.sleep_record
    }

    /// Persists the threaded sleep record: the entered copy after each bed
    /// entry, the settled copy after the settlement batch.
    pub fn set_sleep_record(&mut self, record: SleepState) {
        self.sleep_record_touched = true;
        self.sleep_record = record;
    }

    /// Reducer-owned sleeping sessions in ascending order.
    pub fn sleeping(&self) -> Vec<SessionKey> {
        self.sleeping.iter().copied().collect()
    }

    /// Persists the sleeping set: grown on bed entry, shrunk by settlement
    /// wakes and the morning transition. Collection restores the ascending
    /// order the settlement scans rely on.
    pub fn set_sleeping(&mut self, sessions: Vec<SessionKey>) {
        self.sleeping = sessions.into_iter().collect();
    }

    pub fn charge(&mut self, kind: WorkKind, units: usize) -> Result<(), ServerError> {
        match kind {
            WorkKind::Commands => charge_ceiling(
                &mut self.spent_commands,
                units,
                self.budget.commands(),
                Resource::Commands,
            ),
            WorkKind::FluidUpdates(dimension) => {
                let index = dimension_index(dimension)?;
                charge_ceiling(
                    &mut self.spent_fluid[index],
                    units,
                    self.budget.fluid_updates_per_dimension(),
                    Resource::Commands,
                )
            }
            WorkKind::RescanCells(dimension) => {
                let index = dimension_index(dimension)?;
                let target = self.budget.fluid_rescan_target_per_dimension();
                let spent = self.spent_rescan[index];
                if target == 0 || spent >= target || units > RESCAN_SECTION {
                    return Err(ServerError::Capacity {
                        resource: Resource::Commands,
                        limit: target.saturating_add(RESCAN_MAX_OVERSHOOT),
                        observed: spent.saturating_add(units),
                    });
                }
                let next = spent
                    .checked_add(units)
                    .ok_or(ServerError::InvalidInput { field: "rescan" })?;
                if next > target + RESCAN_MAX_OVERSHOOT {
                    return Err(ServerError::Capacity {
                        resource: Resource::Commands,
                        limit: target + RESCAN_MAX_OVERSHOOT,
                        observed: next,
                    });
                }
                self.spent_rescan[index] = next;
                Ok(())
            }
            WorkKind::FarmlandChecks => charge_ceiling(
                &mut self.spent_farmland_checks,
                units,
                self.budget.farmland_checks(),
                Resource::Commands,
            ),
            WorkKind::FarmlandReads => charge_ceiling(
                &mut self.spent_farmland_reads,
                units,
                self.budget.farmland_block_reads(),
                Resource::Commands,
            ),
            WorkKind::Effects => charge_ceiling(
                &mut self.spent_effects,
                units,
                EFFECT_BUDGET,
                Resource::RuleEffects,
            ),
            WorkKind::SnapshotChunks => charge_ceiling(
                &mut self.spent_snapshot_chunks,
                units,
                self.authority.limits.snapshot_chunks(),
                Resource::Snapshots,
            ),
            WorkKind::SnapshotBytes => charge_ceiling(
                &mut self.spent_snapshot_bytes,
                units,
                self.authority.limits.snapshot_bytes(),
                Resource::SaveBytes,
            ),
        }
    }

    /// Tick-charged fluid updates for one dimension. The reducer attributes
    /// the per-dimension counters from these readers after the last row.
    pub fn spent_fluid(&self, dimension: Dimension) -> usize {
        dimension_index(dimension)
            .map(|index| self.spent_fluid[index])
            .unwrap_or(0)
    }

    /// Tick-charged rescan cells for one dimension, with the shared overshoot
    /// already applied by the charging path.
    pub fn spent_rescan(&self, dimension: Dimension) -> usize {
        dimension_index(dimension)
            .map(|index| self.spent_rescan[index])
            .unwrap_or(0)
    }

    /// Tick-charged farmland candidates.
    pub fn spent_farmland_checks(&self) -> usize {
        self.spent_farmland_checks
    }

    /// Tick-charged farmland block reads.
    pub fn spent_farmland_reads(&self) -> usize {
        self.spent_farmland_reads
    }

    pub fn stage(&mut self, effect: RuleEffect) -> Result<(), RuleReject> {
        check_effect_work(&effect)?;
        let mut pending_projectiles = None;
        let mut pending_damage = 0;
        let mut pending_drops = BTreeMap::new();
        let mut pending_containers = BTreeMap::new();
        let mut pending_inventories = BTreeMap::new();
        self.validate_effect(
            &effect,
            false,
            &mut pending_projectiles,
            &mut pending_damage,
            &mut pending_drops,
            &mut pending_containers,
            &mut pending_inventories,
        )?;
        self.apply_effect(effect)?;
        // Publish only the affected fixed slot copies after every other arm succeeds.
        self.dirty_chunks.extend(
            pending_drops
                .iter()
                .filter_map(|(key, state)| state.dirty.then_some(*key)),
        );
        self.dirty_chunks.extend(
            pending_containers
                .iter()
                .filter_map(|(key, state)| state.dirty.then_some(*key)),
        );
        self.drops.extend(pending_drops);
        self.container_chunks.extend(pending_containers);
        Ok(())
    }

    pub fn projectile_len(&self) -> usize {
        self.projectiles.len()
    }

    pub fn emit(&mut self, event: mornlea_domain::RoutedEvent) -> Result<(), ServerError> {
        self.events.push(event);
        Ok(())
    }

    pub fn events(&self) -> &[mornlea_domain::RoutedEvent] {
        &self.events
    }

    /// Copies the staged overlay into a fixture state for the replay helper.
    /// The hash input is these logical fields, not the process layout.
    pub fn snapshot_state(&self, fallback_world: WorldState) -> FixtureState {
        let chunks: Vec<_> = self
            .ready
            .values()
            .map(|chunk| {
                chunk.snapshot(
                    self.blocks
                        .values()
                        .filter(|observed| observed.key == chunk.key),
                    self.drops.get(&chunk.key),
                    self.container_chunks.get(&chunk.key),
                )
            })
            .collect();
        let revisions: BTreeMap<_, _> = chunks
            .iter()
            .map(|(key, _, revision, _)| (*key, *revision))
            .collect();
        FixtureState {
            runtime: self.runtimes.values().cloned().collect(),
            actors: self.actors.clone(),
            chunks,
            inventories: self
                .inventories
                .iter()
                .map(|(key, inventory)| (*key, *inventory))
                .collect(),
            containers: self
                .container_chunks
                .iter()
                .filter(|(key, _)| key.dimension == Dimension::OVERWORLD)
                .flat_map(|(key, owner)| {
                    owner
                        .references(*key)
                        .into_iter()
                        .filter_map(|reference| owner.record(*key, reference))
                        .map(|mut record| {
                            // Replay aliases name the materialized chunk, not the old tick basis.
                            record.revision = revisions[key];
                            record
                        })
                })
                .chain(
                    self.containers
                        .values()
                        .filter(|record| {
                            !self.ready.contains_key(&ChunkKey {
                                dimension: Dimension::OVERWORLD,
                                pos: record.reference.chunk(),
                            })
                        })
                        .cloned(),
                )
                .collect(),
            work: WorkState::default(),
            sleep: SleepState {
                beds: Vec::new(),
                day_phase_offset: 0,
                pending_offset: None,
            },
            projectiles: self.projectiles.clone(),
            drops: self
                .drops
                .values()
                .flat_map(|state| state.records().iter().cloned())
                .collect(),
            world: self.world.unwrap_or(fallback_world),
        }
    }

    pub fn transaction(&mut self) -> MutationTxn<'_, 'a> {
        MutationTxn { context: self }
    }

    pub fn defer(&mut self, command: CommandEnvelope, phase: RulePhase) -> Result<(), ServerError> {
        if self.deferred.len() >= EFFECT_BUDGET {
            return Err(ServerError::Capacity {
                resource: Resource::Commands,
                limit: EFFECT_BUDGET,
                observed: self.deferred.len() + 1,
            });
        }
        self.deferred.push((phase, command));
        Ok(())
    }

    pub fn deferred(&self, phase: RulePhase) -> Vec<CommandEnvelope> {
        self.deferred
            .iter()
            .filter(|(listed, _)| *listed == phase)
            .map(|(_, command)| *command)
            .collect()
    }

    /// Preflight receipt capacity before a transaction under the same exclusive context.
    pub fn check_charge_capacity(&self) -> Result<(), ServerError> {
        if self.charges.len() >= EFFECT_BUDGET {
            return Err(ServerError::Capacity {
                resource: Resource::Commands,
                limit: EFFECT_BUDGET,
                observed: self.charges.len() + 1,
            });
        }
        Ok(())
    }

    /// Preflight the bounded, tick-local successful-bucket receipt.
    pub fn check_mining_suppression(&self, actor: ActorKey) -> Result<(), ServerError> {
        if !matches!(actor, ActorKey::Player(_)) {
            return Err(ServerError::InvalidInput { field: "actor" });
        }
        if !self.suppressed_mining.contains(&actor) && self.suppressed_mining.len() >= 8 {
            return Err(ServerError::Capacity {
                resource: Resource::Players,
                limit: 8,
                observed: 9,
            });
        }
        Ok(())
    }

    /// Record successful bucket use after the checked transaction commits.
    pub fn suppress_mining(&mut self, actor: ActorKey) -> Result<(), ServerError> {
        self.check_mining_suppression(actor)?;
        self.suppressed_mining.insert(actor);
        Ok(())
    }

    /// Suppression is transient and never part of a saved actor record.
    pub fn mining_suppressed(&self, actor: ActorKey) -> bool {
        self.suppressed_mining.contains(&actor)
    }

    /// Notes one exhaustion charge receipt for an actor, to be settled by the
    /// survival provider's post-physics pass. Writers own the firing rule (see
    /// the `ActionKind` contract); the log itself only bounds memory.
    pub fn note_charge(&mut self, actor: ActorKey, kind: ActionKind) -> Result<(), ServerError> {
        self.check_charge_capacity()?;
        self.charges.push((actor, kind));
        Ok(())
    }

    /// Drains every noted charge receipt. The consumer settles its own actor's
    /// receipts and re-notes the rest, so one drain never starves a later
    /// per-actor pass in the same tick.
    pub fn take_charges(&mut self) -> Vec<(ActorKey, ActionKind)> {
        std::mem::take(&mut self.charges)
    }

    // Each effect lane retains its own private rehearsal until the complete
    // compound validates; keep their ownership explicit at this boundary.
    #[allow(clippy::too_many_arguments)]
    fn validate_effect(
        &self,
        effect: &RuleEffect,
        nested: bool,
        pending_projectiles: &mut Option<Vec<ProjectileRecord>>,
        pending_damage: &mut usize,
        pending_drops: &mut BTreeMap<ChunkKey, DropState>,
        pending_containers: &mut BTreeMap<ChunkKey, ContainerState>,
        pending_inventories: &mut BTreeMap<ActorKey, InventoryRecord>,
    ) -> Result<(), RuleReject> {
        match effect {
            RuleEffect::Compound(parts) => {
                if nested || parts.len() > EFFECT_BUDGET {
                    return Err(RuleReject::ResourceFull(Resource::RuleEffects));
                }
                for part in parts {
                    self.validate_effect(
                        part,
                        true,
                        pending_projectiles,
                        pending_damage,
                        pending_drops,
                        pending_containers,
                        pending_inventories,
                    )?;
                }
                Ok(())
            }
            RuleEffect::Actor(record) => {
                // The record's only own shape rule is the key/body pairing
                // `ActorRecord::try_new` enforces; restating it here keeps a
                // compound actor component from staging a mismatched body.
                let paired = matches!(
                    (&record.key, &record.body),
                    (ActorKey::Player(_), ActorBody::Player(_))
                        | (ActorKey::Companion(_), ActorBody::Companion(_))
                        | (ActorKey::Hostile(_), ActorBody::Hostile(_))
                        | (ActorKey::Passive(_), ActorBody::Passive(_))
                );
                if paired {
                    Ok(())
                } else {
                    Err(RuleReject::Wire(RejectReason::InvalidInput))
                }
            }
            RuleEffect::Inventory(patch) => {
                self.validate_inventory_patch(patch, pending_inventories)
            }
            RuleEffect::Container { before, after } => self.validate_container_patch(
                Dimension::OVERWORLD,
                before,
                after,
                pending_containers,
            ),
            RuleEffect::WorldContainer {
                dimension,
                before,
                after,
            } => self.validate_container_patch(*dimension, before, after, pending_containers),
            RuleEffect::Viewer { session, view } => match view {
                Some(lease) if lease.session() == *session => Ok(()),
                None => Ok(()),
                _ => Err(RuleReject::Wire(RejectReason::InvalidInput)),
            },
            RuleEffect::Blocks(txn) => {
                self.validate_mutation_basis(txn)?;
                if txn.writes.len() > EFFECT_BUDGET {
                    return Err(RuleReject::ResourceFull(Resource::RuleEffects));
                }
                self.validate_writes(&txn.writes)?;
                if let Some(patch) = &txn.inventory {
                    self.validate_inventory_patch(patch, pending_inventories)?;
                }
                for capture in &txn.containers {
                    if !self.read().available(capture.key) {
                        return Err(RuleReject::StaleObservation);
                    }
                    let current = if let Some(owner) = pending_containers.get(&capture.key) {
                        owner.record(capture.key, capture.record.reference)
                    } else {
                        self.read()
                            .world_container(capture.key.dimension, capture.record.reference)
                    };
                    if current.as_ref() != Some(&capture.record) {
                        return Err(RuleReject::StaleObservation);
                    }
                }
                for write in &txn.writes {
                    let key = write.observed.key;
                    if let Some(current) = self.container_chunks.get(&key) {
                        let next = pending_containers
                            .entry(key)
                            .or_insert_with(|| current.clone());
                        next.transition(key, write, &txn.containers)?;
                        self.check_container_revision(key, next)?;
                    }
                }
                if let Some(batch) = &txn.drops {
                    drop_store::validate_batch(batch)?;
                    let (key, _) = drop_store::batch_location(batch)?;
                    if !self.read().available(key) {
                        return Err(RuleReject::StaleObservation);
                    }
                    let next = pending_drop_state(pending_drops, self.drops.get(&key), key)?;
                    next.insert(key, batch)?;
                    self.check_drop_revision(key, next)?;
                }
                Ok(())
            }
            RuleEffect::Drops(batch) => {
                drop_store::validate_batch(batch)?;
                let (key, _) = drop_store::batch_location(batch)?;
                if !self.read().available(key) {
                    return Err(RuleReject::StaleObservation);
                }
                let next = pending_drop_state(pending_drops, self.drops.get(&key), key)?;
                next.insert(key, batch)?;
                self.check_drop_revision(key, next)
            }
            RuleEffect::DropPatch { before, after } => {
                let key = drop_store::drop_key(before)?;
                if !self.read().available(key) {
                    return Err(RuleReject::StaleObservation);
                }
                let next = pending_drop_state(pending_drops, self.drops.get(&key), key)?;
                next.patch(key, before, after.as_ref())?;
                self.check_drop_revision(key, next)
            }
            RuleEffect::Damage(_) => {
                if self.damage_intents.len().saturating_add(*pending_damage) >= EFFECT_BUDGET {
                    return Err(RuleReject::ResourceFull(Resource::RuleEffects));
                }
                *pending_damage += 1;
                Ok(())
            }
            RuleEffect::Projectile { before, after } => {
                // Only projectile effects allocate this bounded preview. Ordered
                // validation permits eviction before insertion without publishing it.
                let records = pending_projectiles.get_or_insert_with(|| self.projectiles.clone());
                apply_projectile(records, before.as_ref(), after.as_ref())
            }
            _ => Ok(()),
        }
    }

    /// Inventory preimages compare with the latest accepted arm in this
    /// rehearsal. Refusal drops the scratch map before any overlay publication.
    fn validate_inventory_patch(
        &self,
        patch: &InventoryPatch,
        pending: &mut BTreeMap<ActorKey, InventoryRecord>,
    ) -> Result<(), RuleReject> {
        let current = pending
            .get(&patch.actor)
            .or_else(|| self.inventories.get(&patch.actor));
        if current != Some(&patch.before) {
            return Err(RuleReject::StaleObservation);
        }
        pending.insert(patch.actor, patch.after);
        Ok(())
    }

    /// All resolver preimages are checked before compound publication begins.
    fn validate_mutation_basis(&self, txn: &BlockTxn) -> Result<(), RuleReject> {
        if txn.tick != self.authority.next_tick {
            return Err(RuleReject::StaleObservation);
        }
        match (&txn.producer, &txn.read_basis) {
            (MutationProducer::System(_), None) => Ok(()),
            (MutationProducer::Actor(actor), Some(basis)) if *actor == basis.actor => {
                let view = self.read();
                let record = view.actor(*actor).ok_or(RuleReject::StaleObservation)?;
                let environment = view.environment().ok_or(RuleReject::StaleObservation)?;
                if record.lifecycle != ActorLifecycle::Active
                    || record.dimension != basis.dimension
                    || record.motion != basis.motion
                    || record.look != basis.look
                    || environment.seed != basis.seed
                    || environment.tunables != basis.tunables
                    || basis.cells.iter().any(|(dimension, pos, observed)| {
                        view.observation(*dimension, *pos) != *observed
                    })
                {
                    return Err(RuleReject::StaleObservation);
                }
                Ok(())
            }
            _ => Err(RuleReject::StaleObservation),
        }
    }

    fn check_container_revision(
        &self,
        key: ChunkKey,
        next: &ContainerState,
    ) -> Result<(), RuleReject> {
        if next.dirty
            && self
                .ready
                .get(&key)
                .is_some_and(|chunk| chunk.revision == u64::MAX)
        {
            Err(RuleReject::StaleObservation)
        } else {
            Ok(())
        }
    }

    fn validate_container_patch(
        &self,
        dimension: Dimension,
        before: &ContainerRecord,
        after: &ContainerRecord,
        pending: &mut BTreeMap<ChunkKey, ContainerState>,
    ) -> Result<(), RuleReject> {
        let key = ChunkKey {
            dimension,
            pos: before.reference.chunk(),
        };
        if !self.read().available(key) {
            return Err(RuleReject::StaleObservation);
        }
        if let Some(current) = self.container_chunks.get(&key) {
            let next = pending.entry(key).or_insert_with(|| current.clone());
            next.patch(key, before, after)?;
            self.check_container_revision(key, next)
        } else if dimension == Dimension::OVERWORLD
            && self.containers.get(&before.reference) == Some(before)
            && before.reference == after.reference
            && before.revision == after.revision
        {
            super::container_store::validate_slots(after)
        } else {
            Err(RuleReject::StaleObservation)
        }
    }

    fn check_drop_revision(&self, key: ChunkKey, next: &DropState) -> Result<(), RuleReject> {
        if next.dirty
            && self
                .ready
                .get(&key)
                .is_some_and(|chunk| chunk.revision == u64::MAX)
        {
            Err(RuleReject::StaleObservation)
        } else {
            Ok(())
        }
    }

    fn validate_writes(&self, writes: &[BlockWrite]) -> Result<(), RuleReject> {
        if writes.len() > EFFECT_BUDGET {
            return Err(RuleReject::ResourceFull(Resource::RuleEffects));
        }
        for write in writes {
            if write.observed.key != block_key(write.observed.key.dimension, write.observed.pos)
                || (write.replacement != write.observed.block
                    && (write.observed.revision == u64::MAX
                        || self
                            .ready
                            .get(&write.observed.key)
                            .is_some_and(|chunk| chunk.revision == u64::MAX)))
            {
                return Err(RuleReject::StaleObservation);
            }
            match self
                .read()
                .observation(write.observed.key.dimension, write.observed.pos)
            {
                Some(current)
                    if current.generation == write.observed.generation
                        && current.revision == write.observed.revision
                        && current.block == write.observed.block => {}
                _ => return Err(RuleReject::StaleObservation),
            }
        }
        Ok(())
    }

    fn apply_effect(&mut self, effect: RuleEffect) -> Result<(), RuleReject> {
        check_effect_work(&effect)?;
        match effect {
            RuleEffect::Compound(parts) => {
                let undo = CompoundUndo::capture(self, &parts);
                for part in parts {
                    if let Err(error) = self.apply_effect(part) {
                        undo.restore(self);
                        return Err(error);
                    }
                }
                Ok(())
            }
            RuleEffect::Actor(record) => {
                // Latest-wins overlay replace, mirroring the inventory arm: a
                // staged actor snapshot supersedes the earlier record with
                // the same key instead of accumulating duplicates.
                let key = record.key;
                let slot = match self.actors.iter().position(|actor| actor.key == key) {
                    Some(index) => {
                        self.actors[index] = record;
                        index
                    }
                    None => {
                        let index = self.actors.len();
                        self.actors.push(record);
                        index
                    }
                };
                self.index_player_actor(key, slot);
                Ok(())
            }
            RuleEffect::Runtime(record) => {
                // Latest-wins overlay replace keyed by actor, mirroring the
                // actor arm: one staged runtime record per actor.
                self.runtimes.insert(record.key, record);
                Ok(())
            }
            RuleEffect::Mining { actor, progress } => {
                // Latest-wins overlay replace keyed by actor, mirroring the
                // runtime arm: `Some` records progress, `None` clears it.
                // The mining provider is the single writer of this lane.
                match progress {
                    Some(record) => {
                        self.mining.insert(actor, record);
                    }
                    None => {
                        self.mining.remove(&actor);
                    }
                }
                Ok(())
            }
            RuleEffect::Inventory(patch) => {
                self.inventories.insert(patch.actor, patch.after);
                Ok(())
            }
            RuleEffect::Container { after, .. } => {
                let key = ChunkKey {
                    dimension: Dimension::OVERWORLD,
                    pos: after.reference.chunk(),
                };
                if !self.container_chunks.contains_key(&key) {
                    self.containers.insert(after.reference, after);
                }
                Ok(())
            }
            RuleEffect::WorldContainer {
                dimension, after, ..
            } => {
                let key = ChunkKey {
                    dimension,
                    pos: after.reference.chunk(),
                };
                if dimension == Dimension::OVERWORLD && !self.container_chunks.contains_key(&key) {
                    self.containers.insert(after.reference, after);
                }
                Ok(())
            }
            RuleEffect::Viewer { session, view } => {
                match view {
                    Some(lease) => {
                        self.viewers.insert(session, lease);
                    }
                    None => {
                        self.viewers.remove(&session);
                    }
                }
                Ok(())
            }
            RuleEffect::World(world) => {
                self.world = Some(world);
                Ok(())
            }
            RuleEffect::Blocks(txn) => {
                for capture in &txn.containers {
                    if capture.key.dimension == Dimension::OVERWORLD
                        && !self.container_chunks.contains_key(&capture.key)
                    {
                        self.containers.remove(&capture.record.reference);
                    }
                }
                self.apply_writes(&txn.writes);
                if let Some(patch) = txn.inventory {
                    self.inventories.insert(patch.actor, patch.after);
                }
                Ok(())
            }
            RuleEffect::Damage(intent) => {
                if self.damage_intents.len() >= EFFECT_BUDGET {
                    return Err(RuleReject::ResourceFull(Resource::RuleEffects));
                }
                self.damage_intents.push(intent);
                Ok(())
            }
            RuleEffect::Projectile { before, after } => {
                apply_projectile(&mut self.projectiles, before.as_ref(), after.as_ref())
            }
            RuleEffect::Environment(environment) => {
                self.environment = Some(environment);
                Ok(())
            }
            RuleEffect::Sleep(record) => {
                // Latest-wins overlay replace, mirroring the environment
                // arm: the settlement stages the threaded record and the
                // reducer persists the settled copy after the batch.
                self.sleep_record_touched = true;
                self.sleep_record = record;
                Ok(())
            }
            _ => Ok(()),
        }
    }

    fn apply_writes(&mut self, writes: &[BlockWrite]) {
        for write in writes {
            if write.replacement == write.observed.block {
                continue;
            }
            let mut observed = write.observed;
            observed.block = write.replacement;
            observed.revision = observed.revision.saturating_add(1);
            self.blocks.insert((observed.key, observed.pos), observed);
            self.dirty_chunks.insert(observed.key);
            self.changed.insert(
                (
                    observed.key,
                    mornlea_domain::chunk_block_index(observed.pos),
                ),
                observed,
            );
            let Some(chunk) = self.ready.get(&observed.key) else {
                continue;
            };
            let pos = observed.pos;
            let current = chunk.height(pos.x(), pos.z());
            let next = if observed.block != 0 && pos.y() > current {
                pos.y()
            } else if observed.block == 0 && pos.y() == current {
                (-64..pos.y())
                    .rev()
                    .find(|y| {
                        self.read()
                            .block(
                                observed.key.dimension,
                                mornlea_domain::BlockPos::new(pos.x(), *y, pos.z()),
                            )
                            .is_some_and(|block| block != 0)
                    })
                    .unwrap_or(-65)
            } else {
                current
            };
            let chunk = self
                .ready
                .get_mut(&observed.key)
                .expect("Ready ownership cannot change during a write");
            chunk.set_block(pos, observed.block);
            chunk.mark_blocks_dirty();
            chunk.set_height(pos.x(), pos.z(), next);
        }
    }
}

impl Drop for TickContext<'_> {
    fn drop(&mut self) {
        self.return_carried(false);
    }
}

fn pending_drop_state<'a>(
    pending: &'a mut BTreeMap<ChunkKey, DropState>,
    current: Option<&DropState>,
    key: ChunkKey,
) -> Result<&'a mut DropState, RuleReject> {
    match pending.entry(key) {
        std::collections::btree_map::Entry::Occupied(entry) => Ok(entry.into_mut()),
        std::collections::btree_map::Entry::Vacant(entry) => {
            Ok(entry.insert(current.ok_or(RuleReject::StaleObservation)?.clone()))
        }
    }
}

fn block_key(dimension: Dimension, position: mornlea_domain::BlockPos) -> ChunkKey {
    ChunkKey {
        dimension,
        pos: mornlea_domain::ChunkPos::new(position.x() >> 4, position.z() >> 4),
    }
}

/// Salt isolating the season-offset hash stream from the weather dice and
/// entity streams (`seasonOffsetSalt`, shared/core/season.go).
const SEASON_OFFSET_SALT: u64 = 0x005e_a50e_51ab_0001;
/// Ticks per year the season offset ranges over (`YearTicks`).
const SEASON_YEAR_TICKS: u64 = 288_000;

/// Mirrors `SeasonOffsetFromSeed` (shared/core/season.go): one SplitMix64
/// pass over the salted seed modulo the year length. A negative seed takes
/// the same two's-complement bit pattern the source conversion produces, so
/// restarts derive the identical offset.
fn season_offset_from_seed(seed: i64) -> u32 {
    (season_splitmix((seed as u64) ^ SEASON_OFFSET_SALT) % SEASON_YEAR_TICKS) as u32
}

/// The shared SplitMix64 terminator (Steele/Lea/Flood), copied from the
/// source dice the season derivation cites.
fn season_splitmix(mut x: u64) -> u64 {
    x = x.wrapping_add(0x9e37_79b9_7f4a_7c15);
    x = (x ^ (x >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    x = (x ^ (x >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    x ^ (x >> 31)
}

fn dimension_index(dimension: Dimension) -> Result<usize, ServerError> {
    match dimension.get() {
        0 => Ok(0),
        1 => Ok(1),
        _ => Err(ServerError::InvalidInput { field: "dimension" }),
    }
}

fn charge_ceiling(
    spent: &mut usize,
    units: usize,
    limit: usize,
    resource: Resource,
) -> Result<(), ServerError> {
    let next = spent
        .checked_add(units)
        .ok_or(ServerError::InvalidInput { field: "charge" })?;
    if next > limit {
        return Err(ServerError::Capacity {
            resource,
            limit,
            observed: next,
        });
    }
    *spent = next;
    Ok(())
}

pub struct MutationTxn<'ctx, 'auth> {
    context: &'ctx mut TickContext<'auth>,
}

impl<'ctx, 'auth> MutationTxn<'ctx, 'auth> {
    pub fn try_place(
        &mut self,
        resolved: ResolvedPlacement,
    ) -> Result<MutationOutcome, RuleReject> {
        self.commit(resolved.into_txn())
    }

    pub fn try_mine(&mut self, resolved: ResolvedMining) -> Result<MutationOutcome, RuleReject> {
        self.commit(resolved.into_txn())
    }

    pub fn try_system(
        &mut self,
        producer: SystemRule,
        writes: Vec<BlockWrite>,
    ) -> Result<MutationOutcome, RuleReject> {
        let tick = self.context.authority.next_tick;
        self.commit(BlockTxn::system(producer, tick, writes))
    }

    /// System block outputs and writes share the same bounded transaction.
    pub fn try_system_with_drops(
        &mut self,
        producer: SystemRule,
        writes: Vec<BlockWrite>,
        drops: DropBatch,
    ) -> Result<MutationOutcome, RuleReject> {
        let tick = self.context.authority.next_tick;
        if !matches!(drops.source,DropSource::System{rule,tick:source_tick,..} if rule==producer && source_tick==tick)
        {
            return Err(RuleReject::Wire(RejectReason::InvalidInput));
        }
        let mut txn = BlockTxn::system(producer, tick, writes);
        txn.drops = Some(drops);
        self.commit(txn)
    }

    fn commit(&mut self, txn: BlockTxn) -> Result<MutationOutcome, RuleReject> {
        let changed = txn
            .writes
            .iter()
            .filter(|write| write.replacement != write.observed.block)
            .map(|write| {
                let mut observed = write.observed;
                observed.block = write.replacement;
                observed.revision = observed.revision.saturating_add(1);
                observed
            })
            .collect();
        let drops_created = txn
            .drops
            .as_ref()
            .map(|drops| drops.stacks.iter().filter(|stack| stack.count != 0).count())
            .unwrap_or(0);
        let inventory_changed = txn.inventory.is_some();
        self.context.stage(RuleEffect::Blocks(txn))?;
        Ok(MutationOutcome {
            changed,
            inventory_changed,
            drops_created,
        })
    }
}

pub fn resolve_place(
    _actor: ActorKey,
    _intent: &mornlea_domain::PlacementIntent,
    _view: &AuthorityReadView<'_>,
) -> Result<ResolvedPlacement, RuleReject> {
    Err(RuleReject::StaleObservation)
}

pub fn resolve_mine(
    _actor: ActorKey,
    _control: &mornlea_domain::PlayerControl,
    _view: &AuthorityReadView<'_>,
) -> Result<Option<ResolvedMining>, RuleReject> {
    Err(RuleReject::StaleObservation)
}

pub fn resolve_companion_place(
    _actor: mornlea_domain::CompanionId,
    _target: mornlea_domain::BlockPos,
    _block: u16,
    _view: &AuthorityReadView<'_>,
) -> Result<ResolvedPlacement, RuleReject> {
    Err(RuleReject::StaleObservation)
}

pub fn resolve_companion_mine(
    _actor: mornlea_domain::CompanionId,
    _target: mornlea_domain::BlockPos,
    _view: &AuthorityReadView<'_>,
) -> Result<ResolvedMining, RuleReject> {
    Err(RuleReject::StaleObservation)
}

/// Applies one checked identity-preserving edit to either a preview or the overlay.
/// Exact preimages reject late hits and repeated removals before any state changes.
fn apply_projectile(
    records: &mut Vec<ProjectileRecord>,
    before: Option<&ProjectileRecord>,
    after: Option<&ProjectileRecord>,
) -> Result<(), RuleReject> {
    match (before, after) {
        (None, None) => Err(RuleReject::Wire(RejectReason::InvalidInput)),
        (None, Some(next)) => {
            if records.iter().any(|record| record.id == next.id) {
                return Err(RuleReject::StaleObservation);
            }
            if records.len() >= MAX_PROJECTILE_RECORDS {
                return Err(RuleReject::ResourceFull(Resource::RuleEffects));
            }
            records.push(next.clone());
            Ok(())
        }
        (Some(previous), next) => {
            if next.is_some_and(|record| record.id != previous.id) {
                return Err(RuleReject::Wire(RejectReason::InvalidInput));
            }
            let index = records
                .iter()
                .position(|record| record == previous)
                .ok_or(RuleReject::StaleObservation)?;
            match next {
                Some(record) => records[index] = record.clone(),
                None => {
                    records.remove(index);
                }
            }
            Ok(())
        }
    }
}

#[cfg(test)]
mod metadata_capture_tests {
    use super::*;

    #[test]
    fn exhausted_metadata_sequence_preserves_last_captured_target() {
        let mut authority = AuthorityState::try_new(
            ServerLimits::try_new(8, 4096, 512, 64, 64, 1_048_576).unwrap(),
            42,
        )
        .unwrap();
        authority.metadata_sequence = u64::MAX;
        let before = authority.metadata_snapshot();
        assert_eq!(authority.try_metadata_snapshot().unwrap(), before);
        authority.advance_tick(TickBudget::full()).unwrap();
        assert_eq!(
            authority.try_metadata_snapshot(),
            Err(ServerError::Internal {
                invariant: "metadata sequence space",
            })
        );
        assert_eq!(authority.metadata_snapshot(), before);
        assert_eq!(authority.metadata_sequence, u64::MAX);
    }
}

#[cfg(test)]
mod owned_resident_tests {
    use super::super::world::{reset_tick_finishes, tick_finishes};
    use super::*;
    use mornlea_domain::{BlockPos, ChunkPos};
    use mornlea_storage::{ContainerSnapshot, StorageKind};

    fn key(x: i32) -> ChunkKey {
        ChunkKey {
            dimension: Dimension::OVERWORLD,
            pos: ChunkPos::new(x, 0),
        }
    }

    fn authority(chunks: i32, actor: bool) -> AuthorityState {
        let mut authority = AuthorityState::try_new(
            ServerLimits::try_new(8, 4096, 512, 64, 64, 1_048_576).unwrap(),
            0,
        )
        .unwrap();
        let mut context = TickContext::harness(&mut authority, TickBudget::full());
        for x in 0..chunks {
            let chunk = Chunk {
                sections: vec![
                    ContainerSnapshot {
                        kind: StorageKind::Single,
                        bits: 0,
                        single: 0,
                        palette: Vec::new(),
                        packed: Vec::new()
                    };
                    24
                ],
                drops: vec![Default::default(); 32],
                furnaces: vec![Default::default(); 32],
                chests: vec![Default::default(); 16],
            };
            context.preload_ready_chunk(ReadyChunk::try_new(key(x), 7, 5, chunk).unwrap());
        }
        if actor {
            let player =
                PlayerId::try_from_bytes([1, 0, 0, 0, 0, 0, 64, 0, 128, 0, 0, 0, 0, 0, 0, 1])
                    .unwrap();
            let mut seeded = seed_player(
                SessionKey::from_raw(1).unwrap(),
                &canonical_player(player, "Ada").unwrap(),
            )
            .unwrap();
            seeded.actor.lifecycle = ActorLifecycle::Pending;
            context.stage_login(seeded);
        }
        let residents = context.resident_snapshot();
        drop(context);
        authority.commit_residents(residents);
        authority.residents.sleep_record = None;
        authority
    }

    fn addresses(residents: &ResidentTickState) -> [usize; 5] {
        [
            residents.actors.as_ptr() as usize,
            std::ptr::from_ref(residents.inventories.values().next().unwrap()) as usize,
            std::ptr::from_ref(residents.ready.values().next().unwrap()) as usize,
            std::ptr::from_ref(residents.drops.values().next().unwrap()) as usize,
            std::ptr::from_ref(residents.container_chunks.values().next().unwrap()) as usize,
        ]
    }

    #[test]
    fn live_commit_captures_all_slots_once_and_pins_old_blocks() {
        use super::super::world::{materializations, reset_materializations};
        use mornlea_domain::FiniteVec3;
        use mornlea_storage::ItemStack;
        let mut authority = authority(1, false);
        let old = authority
            .capture_chunk_snapshot(key(0), SaveUrgency::Autosave)
            .unwrap();
        let mut ctx = TickContext::for_tick(&mut authority, TickBudget::full());
        for (pos, block) in [
            (BlockPos::new(1, -64, 1), 4),
            (BlockPos::new(2, 319, 2), 5),
            (BlockPos::new(3, 64, 3), 11),
            (BlockPos::new(4, 80, 4), 9),
        ] {
            let observed = ctx.read().observation(Dimension::OVERWORLD, pos).unwrap();
            ctx.transaction()
                .try_system(
                    SystemRule::Support,
                    vec![BlockWrite::try_new(observed, block).unwrap()],
                )
                .unwrap();
        }
        for reference in ctx.container_chunks[&key(0)].references(key(0)) {
            let before = ctx.read().container(reference).unwrap();
            let mut after = before.clone();
            match &mut after.slots {
                ContainerSlots::Chest(items) => {
                    items[0] = ItemStack {
                        item: 2,
                        count: 3,
                        durability: 0,
                    }
                }
                ContainerSlots::Furnace {
                    slots,
                    fuel,
                    progress,
                } => {
                    slots[0] = ItemStack {
                        item: 6,
                        count: 2,
                        durability: 0,
                    };
                    *fuel = 20;
                    *progress = 3;
                }
            }
            ctx.stage(RuleEffect::Container { before, after }).unwrap();
        }
        let drops = DropBatch::try_new(
            DropSource::System {
                rule: SystemRule::Support,
                tick: ctx.read().tick(),
                target: BlockPos::new(5, 64, 5),
            },
            Dimension::OVERWORLD,
            FiniteVec3::try_new([5.5, 64.5, 5.5]).unwrap(),
            vec![ItemStack {
                item: 2,
                count: 4,
                durability: 0,
            }],
            5,
        )
        .unwrap();
        ctx.transaction()
            .try_system_with_drops(SystemRule::Support, Vec::new(), drops)
            .unwrap();
        reset_tick_finishes();
        reset_materializations();
        ctx.commit_carried();
        drop(ctx);
        let new = authority
            .capture_chunk_snapshot(key(0), SaveUrgency::Autosave)
            .unwrap();
        assert_eq!(materializations(), 0);
        assert_eq!(tick_finishes(), 1);
        assert_eq!(new.revision, 6);
        let SaveValue::ChunkView(old_view) = &old.value else {
            unreachable!()
        };
        let SaveValue::ChunkView(new_view) = &new.value else {
            unreachable!()
        };
        assert_ne!(old_view, new_view);
        let saved = new_view.materialize();
        assert!(saved.chunk.drops[0].active);
        assert_eq!(saved.chunk.drops[0].stack.count, 4);
        assert_eq!(saved.chunk.chests[0].items[0].count, 3);
        assert_eq!(saved.chunk.furnaces[0].input.item, 6);
        assert_eq!(saved.chunk.furnaces[0].progress_ticks, 3);
        assert_eq!(saved.chunk.furnaces[0].burn_ticks, 20);
        assert_eq!(authority.residents.ready_snapshot()[0].3, saved.chunk);
        assert!(
            old_view
                .materialize()
                .chunk
                .sections
                .iter()
                .all(|section| section.single == 0)
        );
        authority.remember_dirty(old.clone()).unwrap();
        let selected = authority.select(SaveMode::All, SaveBudget::default());
        let equal_new = authority
            .capture_chunk_snapshot(key(0), SaveUrgency::Autosave)
            .unwrap();
        authority.remember_dirty(equal_new.clone()).unwrap();
        let ack = authority.apply_completion(SaveCompletion {
            ticket: SaveTicket::try_from_raw(1).unwrap(),
            snapshots: selected,
            submitted: vec![(old.key.clone(), old.revision)],
            committed: vec![(old.key.clone(), old.revision)],
            error: None,
        });
        assert_eq!(ack.acked, 1);
        assert_eq!(authority.dirty, vec![equal_new]);
        assert_eq!(new_view.materialize(), saved);
    }

    #[test]
    fn chunk_capture_recipe_does_not_materialize_on_authority() {
        use super::super::world::{materializations, reset_materializations};
        let authority = authority(1, false);
        reset_materializations();
        let captured = authority.residents.ready_snapshot();
        assert_eq!(captured.len(), 1);
        assert_eq!(
            materializations(),
            1,
            "explicit off-tick recipe clones the actual Ready base"
        );
        reset_materializations();
        let captured = authority
            .capture_chunk_snapshot(key(0), SaveUrgency::Autosave)
            .unwrap();
        let SaveValue::ChunkView(view) = captured.value else {
            unreachable!()
        };
        assert_eq!(view, view.clone());
        assert_eq!(materializations(), 0);
    }

    #[test]
    fn disabled_legacy_payload_capture_retains_maximum_reservation() {
        let authority = authority(1, false);
        assert_eq!(
            authority
                .capture_chunk_snapshot(key(0), SaveUrgency::Autosave)
                .unwrap()
                .estimated_bytes,
            crate::store::mailbox::CHUNK_MAX_RESERVATION
        );
    }

    #[test]
    fn disabled_legacy_payload_capture_retains_private_unregistered_body() {
        let mut authority = authority(1, false);
        let ready = authority.residents.ready.get_mut(&key(0)).unwrap();
        ready.set_block(BlockPos::new(0, -64, 0), 90);
        ready.mark_blocks_dirty();
        ready.finish_tick(false);
        assert_eq!(ready.payload_estimate(), None);
        let captured = authority
            .capture_chunk_snapshot(key(0), SaveUrgency::Autosave)
            .unwrap();
        assert_eq!(captured.revision, 6);
        assert_eq!(
            captured.estimated_bytes,
            crate::store::mailbox::CHUNK_MAX_RESERVATION
        );
    }

    #[test]
    fn chunk_select_recipe_shares_inflight_body() {
        let mut authority = authority(1, false);
        let mut context = TickContext::for_tick(&mut authority, TickBudget::full());
        write(&mut context, 1, 4);
        context.commit_carried();
        drop(context);
        use super::super::world::{materializations, reset_materializations};
        let captured = authority
            .capture_chunk_snapshot(key(0), SaveUrgency::Autosave)
            .unwrap();
        reset_materializations();
        authority.remember_dirty(captured).unwrap();
        let selected = authority.select(SaveMode::All, SaveBudget::default());
        let SaveValue::ChunkView(chosen) = &selected[0].value else {
            unreachable!()
        };
        let SaveValue::ChunkView(held) = &authority.in_flight[0].value else {
            unreachable!()
        };
        assert_eq!(chosen, held);
        assert_eq!(materializations(), 0);
    }

    #[test]
    fn compound_clones_only_touched_ready_owners() {
        use super::super::world::{ready_clones, reset_ready_clones};
        let mut authority = authority(128, true);
        let mut ctx = TickContext::harness(&mut authority, TickBudget::full());
        let chunks: Vec<_> = ctx.authority.residents.ready.values().cloned().collect();
        for chunk in chunks {
            ctx.preload_ready_chunk(chunk);
        }
        ctx.inventories = ctx.authority.residents.inventories.clone();
        let observed = ctx
            .read()
            .observation(Dimension::OVERWORLD, BlockPos::new(1, 64, 1))
            .unwrap();
        reset_ready_clones();
        ctx.stage(RuleEffect::Compound(vec![RuleEffect::Blocks(
            BlockTxn::system(
                SystemRule::Support,
                ctx.read().tick(),
                vec![BlockWrite::try_new(observed, 4).unwrap()],
            ),
        )]))
        .unwrap();
        assert_eq!(ready_clones(), 1);
        let (&actor, &before) = ctx.inventories.iter().next().unwrap();
        reset_ready_clones();
        ctx.stage(RuleEffect::Compound(vec![RuleEffect::Inventory(
            InventoryPatch {
                actor,
                before,
                after: before,
            },
        )]))
        .unwrap();
        assert_eq!(ready_clones(), 0);
    }

    #[test]
    fn compound_failure_preserves_unrelated_allocations_and_prior_dirty_work() {
        let mut authority = authority(2, true);
        let mut ctx = TickContext::for_tick(&mut authority, TickBudget::full());
        write(&mut ctx, 1, 4);
        let changed = ctx.changed.clone();
        let inventory_pointer =
            std::ptr::from_ref(ctx.inventories.values().next().unwrap()) as usize;
        let ready_pointer = std::ptr::from_ref(&ctx.ready[&key(1)]) as usize;
        let observed = ctx
            .read()
            .observation(Dimension::OVERWORLD, BlockPos::new(2, 65, 1))
            .unwrap();
        assert_eq!(
            ctx.apply_effect(RuleEffect::Compound(vec![
                RuleEffect::Blocks(BlockTxn::system(
                    SystemRule::Support,
                    ctx.read().tick(),
                    vec![BlockWrite::try_new(observed, 4).unwrap()]
                )),
                RuleEffect::Projectile {
                    before: None,
                    after: None
                },
            ])),
            Err(RuleReject::Wire(RejectReason::InvalidInput))
        );
        assert_eq!(
            std::ptr::from_ref(ctx.inventories.values().next().unwrap()) as usize,
            inventory_pointer
        );
        assert_eq!(
            std::ptr::from_ref(&ctx.ready[&key(1)]) as usize,
            ready_pointer
        );
        assert_eq!(
            ctx.read().block(Dimension::OVERWORLD, observed.pos),
            Some(0)
        );
        assert_eq!(ctx.ready[&key(0)].height(1, 1), 64);
        assert_eq!(ctx.ready[&key(0)].height(2, 1), -65);
        assert_eq!(ctx.ready[&key(0)].revision, 5);
        assert_eq!(ctx.changed, changed);
        assert_eq!(ctx.dirty_chunks, BTreeSet::from([key(0)]));
        ctx.commit_carried();
        drop(ctx);
        assert_eq!(authority.residents.ready[&key(0)].revision, 6);
        assert_eq!(authority.residents.ready[&key(1)].revision, 5);
    }

    fn rejected_apply(ctx: &mut TickContext<'_>, mut parts: Vec<RuleEffect>) {
        parts.push(RuleEffect::Projectile {
            before: None,
            after: None,
        });
        assert_eq!(
            ctx.apply_effect(RuleEffect::Compound(parts)),
            Err(RuleReject::Wire(RejectReason::InvalidInput))
        );
    }

    #[test]
    fn chunk_cell_cas_survives_multiwrite_commit_and_unrelated_dirty_ticks() {
        let mut authority = authority(1, false);
        let original_capture = authority
            .capture_chunk_snapshot(key(0), SaveUrgency::Autosave)
            .unwrap();
        let SaveValue::ChunkView(original_view) = &original_capture.value else {
            unreachable!()
        };
        let original_body = original_view.materialize();
        assert_eq!(original_capture.revision, 5);
        let pos = BlockPos::new(1, 64, 1);
        let mut ctx = TickContext::for_tick(&mut authority, TickBudget::full());
        let initial = ctx.read().observation(Dimension::OVERWORLD, pos).unwrap();
        assert_eq!(
            (initial.generation, initial.revision, initial.block),
            (7, 5, 0)
        );
        write(&mut ctx, 1, 4);
        let first = ctx.read().observation(Dimension::OVERWORLD, pos).unwrap();
        assert_eq!((first.generation, first.revision, first.block), (7, 6, 4));
        write(&mut ctx, 1, 5);
        let twice = ctx.read().observation(Dimension::OVERWORLD, pos).unwrap();
        assert_eq!((twice.generation, twice.revision, twice.block), (7, 7, 5));
        ctx.commit_carried();
        drop(ctx);
        assert_eq!(authority.residents.ready[&key(0)].revision, 6);
        assert_eq!(authority.residents.blocks[&(key(0), pos)], twice);
        for (x, durable) in [(2, 7), (3, 8)] {
            let mut ctx = TickContext::for_tick(&mut authority, TickBudget::full());
            assert_eq!(
                ctx.read().observation(Dimension::OVERWORLD, pos),
                Some(twice)
            );
            assert_eq!(
                ctx.transaction().try_system(
                    SystemRule::Support,
                    vec![BlockWrite::try_new(first, 4).unwrap()],
                ),
                Err(RuleReject::StaleObservation)
            );
            write(&mut ctx, x, 4);
            ctx.commit_carried();
            drop(ctx);
            assert_eq!(authority.residents.ready[&key(0)].revision, durable);
            assert_eq!(authority.residents.blocks[&(key(0), pos)], twice);
            assert_eq!(original_view.materialize(), original_body);
        }
    }

    #[test]
    fn chunk_local_observation_compound_failure_restores_empty_owner() {
        let mut authority = authority(1, true);
        let mut ctx = TickContext::for_tick(&mut authority, TickBudget::full());
        let pos = BlockPos::new(1, 64, 1);
        let original = ctx.read().observation(Dimension::OVERWORLD, pos).unwrap();
        let original_height = ctx.ready[&key(0)].height(1, 1);
        assert!(ctx.blocks.is_empty());
        assert!(ctx.changed.is_empty());
        assert!(ctx.dirty_chunks.is_empty());
        let tick = ctx.read().tick();
        // Inject the existing invalid projectile arm after a real accepted block arm.
        rejected_apply(
            &mut ctx,
            vec![RuleEffect::Blocks(BlockTxn::system(
                SystemRule::Support,
                tick,
                vec![BlockWrite::try_new(original, 4).unwrap()],
            ))],
        );
        assert_eq!(ctx.blocks.len(), 0);
        assert!(ctx.blocks.is_empty());
        assert_eq!(ctx.blocks.take_chunk(key(0)), None);
        assert_eq!(
            ctx.read().observation(Dimension::OVERWORLD, pos),
            Some(original)
        );
        assert_eq!(ctx.ready[&key(0)].height(1, 1), original_height);
        assert!(ctx.changed.is_empty());
        assert!(ctx.dirty_chunks.is_empty());
        write(&mut ctx, 1, 4);
        let accepted = ctx.read().observation(Dimension::OVERWORLD, pos).unwrap();
        assert_eq!(ctx.blocks.len(), 1);
        assert_eq!(ctx.blocks[&(key(0), pos)], accepted);
        assert_eq!(
            (accepted.generation, accepted.revision, accepted.block),
            (7, 6, 4)
        );
        assert_eq!(ctx.changed.len(), 1);
        assert_eq!(ctx.dirty_chunks, BTreeSet::from([key(0)]));
    }

    #[test]
    fn compound_restores_first_actor_inventory_runtime_and_mining_preimages() {
        use mornlea_domain::HotbarSlot;
        let mut authority = authority(1, true);
        let mut ctx = TickContext::for_tick(&mut authority, TickBudget::full());
        let actor = ctx.actors[0].key;
        let new_session = SessionKey::from_raw(2).unwrap();
        let new_key = ActorKey::Player(new_session);
        let ActorKey::Player(session) = actor else {
            unreachable!()
        };
        for session in [session, new_session] {
            ctx.authority.sessions.insert(
                session,
                SessionRecord {
                    player_id: PlayerId::try_from_bytes([
                        1, 0, 0, 0, 0, 0, 64, 0, 128, 0, 0, 0, 0, 0, 0, 1,
                    ])
                    .unwrap(),
                    display_name: "Ada".into(),
                    phase: SessionPhase::Active,
                    last_applied_sequence: 0,
                    last_input_sequence: 0,
                    next_arrival: 0,
                    body: None,
                    outbox: VecDeque::new(),
                    outbox_closed: false,
                },
            );
        }
        ctx.authority
            .current_sessions
            .extend([session, new_session]);
        ctx.player_slots.insert(session, 0);
        let mut unrelated = ctx.actors[0].clone();
        unrelated.key = ActorKey::Player(SessionKey::from_raw(3).unwrap());
        ctx.actors.push(unrelated);
        let actors = ctx.actors.clone();
        let slots = ctx.player_slots.clone();
        let inventory = ctx.inventories[&actor];
        let mut after_inventory = inventory;
        after_inventory.selected = HotbarSlot::new(1).unwrap();
        let runtime = ActorRuntime {
            key: actor,
            controls: None,
            has_view: false,
            reset: false,
            attack_cooldown: 0,
            hurt_cooldown: 0,
            burn_cooldown: 0,
            oxygen: 300,
            peak_y: 64.0,
            exhaustion_milli: 0,
            saturation_milli: 0,
            since_damage_ticks: 0,
            drown_ticks: 0,
            starvation_ticks: 0,
            eating: None,
            bow: None,
            path: None,
            aux: ActorAux::Player {
                respawn: None,
                workbench: None,
            },
        };
        ctx.runtimes.insert(actor, runtime.clone());
        let mut last_inventory = after_inventory;
        last_inventory.selected = HotbarSlot::new(2).unwrap();
        let mut after_runtime = runtime.clone();
        after_runtime.reset = !runtime.reset;
        let progress = MiningProgress {
            actor,
            dimension: Dimension::OVERWORLD,
            target: BlockPos::new(1, 64, 1),
            observed_block: 4,
            tool_slot: HotbarSlot::new(0).unwrap(),
            tool: Default::default(),
            elapsed: 1,
            required: 3,
            last_tick: ctx.read().tick(),
        };
        ctx.mining.insert(actor, progress.clone());
        let mut later_progress = progress.clone();
        later_progress.elapsed = 2;
        let mut replacement = actors[0].clone();
        replacement.lifecycle = ActorLifecycle::Dead;
        let mut appended = actors[0].clone();
        appended.key = new_key;
        let mut new_runtime = runtime.clone();
        new_runtime.key = new_key;
        rejected_apply(
            &mut ctx,
            vec![
                RuleEffect::Actor(replacement.clone()),
                RuleEffect::Actor(appended.clone()),
                RuleEffect::Actor(replacement),
                RuleEffect::Actor(appended),
                RuleEffect::Inventory(InventoryPatch {
                    actor,
                    before: inventory,
                    after: after_inventory,
                }),
                RuleEffect::Inventory(InventoryPatch {
                    actor,
                    before: after_inventory,
                    after: last_inventory,
                }),
                RuleEffect::Inventory(InventoryPatch {
                    actor: new_key,
                    before: inventory,
                    after: inventory,
                }),
                RuleEffect::Runtime(after_runtime.clone()),
                RuleEffect::Runtime(after_runtime),
                RuleEffect::Runtime(new_runtime),
                RuleEffect::Mining {
                    actor,
                    progress: None,
                },
                RuleEffect::Mining {
                    actor,
                    progress: Some(later_progress),
                },
                RuleEffect::Mining {
                    actor: new_key,
                    progress: Some(progress.clone()),
                },
            ],
        );
        assert_eq!(ctx.actors, actors);
        assert_eq!(ctx.player_slots, slots);
        assert_eq!(ctx.inventories[&actor], inventory);
        assert!(!ctx.inventories.contains_key(&new_key));
        assert_eq!(ctx.runtimes[&actor], runtime);
        assert!(!ctx.runtimes.contains_key(&new_key));
        assert_eq!(ctx.mining[&actor], progress);
        assert!(!ctx.mining.contains_key(&new_key));
    }

    #[test]
    fn compound_restores_sparse_container_and_viewer_presence() {
        use mornlea_domain::ContainerKind;
        let mut authority = authority(1, true);
        let mut ctx = TickContext::for_tick(&mut authority, TickBudget::full());
        let reference = ContainerRef::try_new(key(2).pos, ContainerKind::Chest, 0, 1).unwrap();
        let missing = ContainerRef::try_new(key(3).pos, ContainerKind::Chest, 0, 1).unwrap();
        let record = ContainerRecord {
            reference,
            revision: 5,
            slots: ContainerSlots::Chest([Default::default(); 27]),
        };
        let mut after = record.clone();
        let ContainerSlots::Chest(items) = &mut after.slots else {
            unreachable!()
        };
        items[0].item = 2;
        items[0].count = 1;
        let mut absent = record.clone();
        absent.reference = missing;
        ctx.containers.insert(reference, record.clone());
        let session = SessionKey::from_raw(1).unwrap();
        let missing_session = SessionKey::from_raw(2).unwrap();
        let lease = ViewLease::new(session, reference);
        ctx.viewers.insert(session, lease);
        let mut removal = BlockTxn::system(SystemRule::Support, ctx.read().tick(), Vec::new());
        removal.containers.push(CapturedContainer {
            key: key(2),
            record: record.clone(),
        });
        rejected_apply(
            &mut ctx,
            vec![
                RuleEffect::Blocks(removal),
                RuleEffect::Container {
                    before: record.clone(),
                    after,
                },
                RuleEffect::WorldContainer {
                    dimension: Dimension::OVERWORLD,
                    before: absent.clone(),
                    after: absent,
                },
                RuleEffect::Viewer {
                    session,
                    view: None,
                },
                RuleEffect::Viewer {
                    session,
                    view: Some(ViewLease::new(session, missing)),
                },
                RuleEffect::Viewer {
                    session: missing_session,
                    view: Some(ViewLease::new(missing_session, missing)),
                },
            ],
        );
        assert_eq!(ctx.containers.get(&reference), Some(&record));
        assert!(!ctx.containers.contains_key(&missing));
        assert_eq!(ctx.viewers.get(&session), Some(&lease));
        assert!(!ctx.viewers.contains_key(&missing_session));
    }

    #[test]
    fn compound_restores_scalar_presence_sleep_bit_projectile_order_and_damage_length() {
        use mornlea_domain::{FiniteVec3, ProjectileId, ProjectileKind, Season, WorldStateParts};
        for initially_present in [false, true] {
            let mut authority = authority(1, true);
            let mut ctx = TickContext::for_tick(&mut authority, TickBudget::full());
            let actor = ctx.actors[0].key;
            let world = WorldState::try_new(WorldStateParts {
                day_phase_offset: 0,
                world_time_ticks: 7,
                weather: Weather::Clear,
                season: Season::Spring,
                season_progress: 0,
                temperature: 0,
            })
            .unwrap();
            ctx.freeze_environment(1);
            let environment = ctx.environment.clone().unwrap();
            if initially_present {
                ctx.world = Some(world);
                ctx.sleep_record_touched = true;
            } else {
                ctx.environment = None;
            }
            let old_world = ctx.world;
            let old_environment = ctx.environment.clone();
            let old_sleep = ctx.sleep_record.clone();
            let mut after_environment = environment;
            after_environment.world_time += 99;
            let after_world = WorldState::try_new(WorldStateParts {
                day_phase_offset: 0,
                world_time_ticks: 11,
                weather: Weather::Clear,
                season: Season::Spring,
                season_progress: 0,
                temperature: 0,
            })
            .unwrap();
            let projectile = |id| ProjectileRecord {
                id: ProjectileId::try_new(id).unwrap(),
                owner: actor,
                dimension: Dimension::OVERWORLD,
                position: FiniteVec3::try_new([0.0, 64.0, 0.0]).unwrap(),
                velocity: FiniteVec3::try_new([0.0, 0.0, 1.0]).unwrap(),
                kind: ProjectileKind::Arrow,
                damage: 1,
                age: 0,
            };
            ctx.projectiles = vec![projectile(8), projectile(2), projectile(5)];
            let projectiles = ctx.projectiles.clone();
            let damage = DamageIntent {
                source: actor,
                target: actor,
                dimension: Dimension::OVERWORLD,
                amount: 1,
                cause: DamageCause::Fall,
                projectile: None,
                tick: ctx.read().tick(),
            };
            ctx.damage_intents.push(damage);
            rejected_apply(
                &mut ctx,
                vec![
                    RuleEffect::World(after_world),
                    RuleEffect::Environment(after_environment),
                    RuleEffect::Sleep(SleepState::try_new(Vec::new(), 23, None).unwrap()),
                    RuleEffect::Sleep(SleepState::try_new(Vec::new(), 24, None).unwrap()),
                    RuleEffect::Projectile {
                        before: Some(projectile(2)),
                        after: None,
                    },
                    RuleEffect::Projectile {
                        before: None,
                        after: Some(projectile(7)),
                    },
                    RuleEffect::Damage(damage),
                    RuleEffect::Damage(damage),
                ],
            );
            assert_eq!(ctx.world, old_world);
            assert_eq!(ctx.environment, old_environment);
            assert_eq!(ctx.sleep_record, old_sleep);
            assert_eq!(ctx.sleep_record_touched, initially_present);
            assert_eq!(ctx.projectiles, projectiles);
            assert_eq!(ctx.damage_intents, vec![damage]);
        }
    }

    #[test]
    fn compound_repeated_cells_restore_initial_absence_and_height_column() {
        let mut authority = authority(1, true);
        let mut ctx = TickContext::for_tick(&mut authority, TickBudget::full());
        write(&mut ctx, 1, 4);
        let blocks = ctx.blocks.clone();
        let changed = ctx.changed.clone();
        let low = ctx
            .read()
            .observation(Dimension::OVERWORLD, BlockPos::new(1, 64, 1))
            .unwrap();
        let high = ctx
            .read()
            .observation(Dimension::OVERWORLD, BlockPos::new(1, 65, 1))
            .unwrap();
        let mut updated_high = high;
        updated_high.block = 4;
        updated_high.revision += 1;
        let tick = ctx.read().tick();
        rejected_apply(
            &mut ctx,
            vec![
                RuleEffect::Blocks(BlockTxn::system(
                    SystemRule::Support,
                    tick,
                    vec![
                        BlockWrite::try_new(low, 0).unwrap(),
                        BlockWrite::try_new(high, 4).unwrap(),
                    ],
                )),
                RuleEffect::Blocks(BlockTxn::system(
                    SystemRule::Support,
                    tick,
                    vec![BlockWrite::try_new(updated_high, 0).unwrap()],
                )),
            ],
        );
        assert_eq!(ctx.blocks, blocks);
        assert_eq!(ctx.changed, changed);
        assert_eq!(ctx.ready[&key(0)].height(1, 1), 64);
        assert_eq!(ctx.dirty_chunks, BTreeSet::from([key(0)]));
    }

    #[test]
    fn compound_work_bounds_precede_validation_and_mutation() {
        use mornlea_domain::ContainerKind;
        let mut authority = authority(1, true);
        let mut ctx = TickContext::for_tick(&mut authority, TickBudget::full());
        let observed = ctx
            .read()
            .observation(Dimension::OVERWORLD, BlockPos::new(1, 64, 1))
            .unwrap();
        let txn = |count| {
            BlockTxn::system(
                SystemRule::Support,
                0,
                vec![BlockWrite::try_new(observed, 4).unwrap(); count],
            )
        };
        ctx.stage(RuleEffect::Compound(Vec::new())).unwrap();
        ctx.apply_effect(RuleEffect::Compound(Vec::new())).unwrap();
        let rejection = Err(RuleReject::ResourceFull(Resource::RuleEffects));
        let actor = ctx.actors[0].clone();
        let mut captures = txn(0);
        captures.containers = vec![
            CapturedContainer {
                key: key(2),
                record: ContainerRecord {
                    reference: ContainerRef::try_new(key(2).pos, ContainerKind::Chest, 0, 1)
                        .unwrap(),
                    revision: 5,
                    slots: ContainerSlots::Chest([Default::default(); 27]),
                },
            };
            4097
        ];
        ctx.freeze_environment(1);
        let mut basis = txn(0);
        basis.read_basis = Some(MutationReadBasis {
            actor: actor.key,
            dimension: actor.dimension,
            motion: actor.motion,
            look: actor.look,
            seed: 0,
            tunables: RuleTunables::source_defaults(),
            cells: vec![(Dimension::OVERWORLD, observed.pos, Some(observed)); 4097],
        });
        let mut first_captures = captures.clone();
        first_captures.containers.truncate(2048);
        let mut second_captures = captures.clone();
        second_captures.containers.truncate(2049);
        let mut first_basis = basis.clone();
        first_basis
            .read_basis
            .as_mut()
            .unwrap()
            .cells
            .truncate(2048);
        let mut second_basis = basis.clone();
        second_basis
            .read_basis
            .as_mut()
            .unwrap()
            .cells
            .truncate(2049);
        let effects = vec![
            RuleEffect::Compound(vec![RuleEffect::Compound(Vec::new())]),
            RuleEffect::Compound(vec![RuleEffect::Actor(actor); 4097]),
            RuleEffect::Compound(vec![
                RuleEffect::Blocks(txn(2048)),
                RuleEffect::Blocks(txn(2049)),
            ]),
            RuleEffect::Blocks(captures.clone()),
            RuleEffect::Blocks(basis.clone()),
            RuleEffect::Compound(vec![
                RuleEffect::Blocks(first_captures),
                RuleEffect::Blocks(second_captures),
            ]),
            RuleEffect::Compound(vec![
                RuleEffect::Blocks(first_basis),
                RuleEffect::Blocks(second_basis),
            ]),
            RuleEffect::Compound(vec![RuleEffect::Blocks(captures)]),
            RuleEffect::Compound(vec![RuleEffect::Blocks(basis)]),
        ];
        for effect in effects {
            assert_eq!(ctx.stage(effect.clone()), rejection);
            assert_eq!(ctx.apply_effect(effect), rejection);
            assert!(ctx.blocks.is_empty());
            assert!(ctx.changed.is_empty());
            assert!(ctx.dirty_chunks.is_empty());
        }
        let mut accepted = txn(4096);
        accepted.tick = ctx.read().tick();
        let mut independent = accepted.clone();
        independent.containers = vec![
            CapturedContainer {
                key: key(2),
                record: ContainerRecord {
                    reference: ContainerRef::try_new(key(2).pos, ContainerKind::Chest, 0, 1)
                        .unwrap(),
                    revision: 5,
                    slots: ContainerSlots::Chest([Default::default(); 27]),
                },
            };
            4096
        ];
        independent.read_basis = Some(MutationReadBasis {
            actor: ctx.actors[0].key,
            dimension: ctx.actors[0].dimension,
            motion: ctx.actors[0].motion,
            look: ctx.actors[0].look,
            seed: 0,
            tunables: RuleTunables::source_defaults(),
            cells: vec![(Dimension::OVERWORLD, observed.pos, Some(observed)); 4096],
        });
        assert_eq!(
            check_effect_work(&RuleEffect::Compound(vec![RuleEffect::Blocks(independent)])),
            Ok(())
        );
        ctx.stage(RuleEffect::Compound(vec![RuleEffect::Blocks(accepted)]))
            .unwrap();
        assert_eq!(
            ctx.read().block(Dimension::OVERWORLD, observed.pos),
            Some(4)
        );
    }

    #[test]
    fn for_tick_carries_existing_resident_allocations() {
        let mut authority = authority(1, true);
        let pointers = addresses(&authority.residents);
        let actors = authority.residents.actors.clone();
        let inventories = authority.residents.inventories.clone();
        let context = TickContext::for_tick(&mut authority, TickBudget::full());
        assert_eq!(context.actors.as_ptr() as usize, pointers[0]);
        assert_eq!(
            std::ptr::from_ref(context.inventories.values().next().unwrap()) as usize,
            pointers[1]
        );
        assert_eq!(
            std::ptr::from_ref(context.ready.values().next().unwrap()) as usize,
            pointers[2]
        );
        assert_eq!(
            std::ptr::from_ref(context.drops.values().next().unwrap()) as usize,
            pointers[3]
        );
        assert_eq!(
            std::ptr::from_ref(context.container_chunks.values().next().unwrap()) as usize,
            pointers[4]
        );
        assert_eq!(context.actors, actors);
        assert_eq!(context.inventories, inventories);
    }

    #[test]
    fn live_idle_tick_preserves_resident_allocations_and_values() {
        let mut authority = authority(1, true);
        let pointers = addresses(&authority.residents);
        let actors = authority.residents.actors.clone();
        let inventories = authority.residents.inventories.clone();
        let chunks = authority.residents.ready_snapshot();
        authority.advance_tick(TickBudget::full()).unwrap();
        assert_eq!(addresses(&authority.residents), pointers);
        assert_eq!(authority.residents.actors, actors);
        assert_eq!(authority.residents.inventories, inventories);
        assert_eq!(authority.residents.ready_snapshot(), chunks);
    }

    fn write(context: &mut TickContext<'_>, x: i32, block: u16) {
        let observed = context
            .read()
            .observation(Dimension::OVERWORLD, BlockPos::new(x, 64, 1))
            .unwrap();
        context
            .transaction()
            .try_system(
                SystemRule::Support,
                vec![BlockWrite::try_new(observed, block).unwrap()],
            )
            .unwrap();
    }

    #[test]
    fn abandoned_read_only_loan_restores_allocations_values_and_absent_sleep() {
        let mut authority = authority(1, true);
        let pointers = addresses(&authority.residents);
        let actors = authority.residents.actors.clone();
        let inventories = authority.residents.inventories.clone();
        let chunks = authority.residents.ready_snapshot();
        reset_tick_finishes();
        drop(TickContext::for_tick(&mut authority, TickBudget::full()));
        assert_eq!(addresses(&authority.residents), pointers);
        assert_eq!(authority.residents.actors, actors);
        assert_eq!(authority.residents.inventories, inventories);
        assert_eq!(authority.residents.ready_snapshot(), chunks);
        assert!(authority.residents.sleep_record.is_none());
        assert_eq!(tick_finishes(), 0);
    }

    #[test]
    fn detached_harness_drop_preserves_authority_residents() {
        let mut authority = authority(1, true);
        let pointers = addresses(&authority.residents);
        let context = TickContext::harness(&mut authority, TickBudget::full());
        drop(context);
        assert_eq!(addresses(&authority.residents), pointers);
        assert!(authority.residents.sleep_record.is_none());
    }

    #[test]
    fn unwind_recovers_accepted_write_for_next_live_commit() {
        let mut authority = authority(1, true);
        let pointers = addresses(&authority.residents);
        reset_tick_finishes();
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let mut context = TickContext::for_tick(&mut authority, TickBudget::full());
            write(&mut context, 1, 4);
            panic!("abandon the exclusive resident loan");
        }));
        assert!(result.is_err());
        assert_eq!(addresses(&authority.residents), pointers);
        assert_eq!(authority.residents.ready[&key(0)].revision, 5);
        assert_eq!(
            authority.residents.blocks[&(key(0), BlockPos::new(1, 64, 1))].block,
            4
        );
        assert_eq!(tick_finishes(), 0);
        authority.advance_tick(TickBudget::full()).unwrap();
        assert_eq!(tick_finishes(), 1);
        assert_eq!(authority.residents.ready[&key(0)].revision, 6);
        reset_tick_finishes();
        authority.advance_tick(TickBudget::full()).unwrap();
        assert_eq!(tick_finishes(), 0);
    }

    #[test]
    fn abandoned_sleep_writes_restore_some_record() {
        for staged in [false, true] {
            let mut authority = authority(1, false);
            let record = SleepState::try_new(Vec::new(), 23, None).unwrap();
            let mut context = TickContext::for_tick(&mut authority, TickBudget::full());
            if staged {
                context.stage(RuleEffect::Sleep(record.clone())).unwrap();
            } else {
                context.set_sleep_record(record.clone());
            }
            drop(context);
            assert_eq!(authority.residents.sleep_record, Some(record));
        }
    }

    #[test]
    fn one_changed_chunk_finishes_once_among_many_ready_chunks() {
        let mut authority = authority(128, false);
        reset_tick_finishes();
        let mut context = TickContext::for_tick(&mut authority, TickBudget::full());
        write(&mut context, 1, 4);
        write(&mut context, 2, 4);
        assert_eq!(context.dirty_chunks, BTreeSet::from([key(0)]));
        context.commit_carried();
        drop(context);
        assert_eq!(tick_finishes(), 1);
        assert_eq!(authority.residents.ready[&key(0)].revision, 6);
        assert!(
            authority
                .residents
                .ready
                .iter()
                .filter(|(key, _)| **key != self::key(0))
                .all(|(_, chunk)| chunk.revision == 5)
        );
        assert!(authority.residents.dirty_chunks.is_empty());
        reset_tick_finishes();
        authority.advance_tick(TickBudget::full()).unwrap();
        assert_eq!(tick_finishes(), 0);
    }

    #[test]
    fn explicit_commit_repeated_commit_and_drop_preserve_returned_state() {
        let mut authority = authority(1, true);
        let pointers = addresses(&authority.residents);
        let mut context = TickContext::for_tick(&mut authority, TickBudget::full());
        write(&mut context, 1, 4);
        reset_tick_finishes();
        context.commit_carried();
        assert_eq!(tick_finishes(), 1);
        assert_eq!(addresses(&context.authority.residents), pointers);
        assert!(context.authority.residents.sleep_record.is_some());
        context.authority.residents.actors[0].lifecycle = ActorLifecycle::Dead;
        context.commit_carried();
        drop(context);
        assert_eq!(tick_finishes(), 1);
        assert_eq!(
            authority.residents.actors[0].lifecycle,
            ActorLifecycle::Dead
        );
        assert_eq!(authority.residents.ready[&key(0)].revision, 6);
    }

    #[test]
    fn abandoned_read_only_loan_preserves_existing_sleep_allocation() {
        let mut authority = authority(1, false);
        let record = SleepState::try_new(
            vec![(
                SessionKey::from_raw(1).unwrap(),
                Dimension::OVERWORLD,
                BlockPos::new(1, 64, 1),
            )],
            23,
            None,
        )
        .unwrap();
        authority.residents.sleep_record = Some(record.clone());
        let pointer = authority
            .residents
            .sleep_record
            .as_ref()
            .unwrap()
            .beds
            .as_ptr() as usize;
        let context = TickContext::for_tick(&mut authority, TickBudget::full());
        assert_eq!(context.sleep_record.beds.as_ptr() as usize, pointer);
        drop(context);
        assert_eq!(authority.residents.sleep_record, Some(record));
        assert_eq!(
            authority
                .residents
                .sleep_record
                .as_ref()
                .unwrap()
                .beds
                .as_ptr() as usize,
            pointer
        );
    }

    #[test]
    fn no_op_block_write_finishes_no_chunks() {
        let mut authority = authority(1, false);
        reset_tick_finishes();
        let mut context = TickContext::for_tick(&mut authority, TickBudget::full());
        write(&mut context, 1, 0);
        assert!(context.dirty_chunks.is_empty());
        context.commit_carried();
        drop(context);
        assert_eq!(tick_finishes(), 0);
        assert_eq!(authority.residents.ready[&key(0)].revision, 5);
    }

    #[test]
    fn ready_fixture_replacement_clears_pending_dirty_key() {
        let mut authority = authority(1, false);
        let chunk = authority.residents.ready_snapshot()[0].3.clone();
        reset_tick_finishes();
        let mut context = TickContext::for_tick(&mut authority, TickBudget::full());
        write(&mut context, 1, 4);
        assert_eq!(context.dirty_chunks, BTreeSet::from([key(0)]));
        context.preload_ready_chunk(ReadyChunk::try_new(key(0), 8, 17, chunk).unwrap());
        assert!(context.dirty_chunks.is_empty());
        context.commit_carried();
        drop(context);
        assert_eq!(tick_finishes(), 0);
        assert_eq!(authority.residents.ready[&key(0)].revision, 17);
    }

    #[test]
    fn counter_only_drop_patch_preserves_revision_without_finish() {
        let mut authority = authority(1, false);
        let mut chunk = authority.residents.ready_snapshot()[0].3.clone();
        chunk.drops[0] = mornlea_storage::DropSlot {
            generation: 1,
            active: true,
            stack: mornlea_storage::ItemStack {
                item: 2,
                count: 3,
                durability: 0,
            },
            block_index: mornlea_domain::chunk_block_index(BlockPos::new(1, 64, 1)),
            age_ticks: 10,
            pickup_delay_ticks: 5,
        };
        let mut context = TickContext::for_tick(&mut authority, TickBudget::full());
        context.preload_ready_chunk(ReadyChunk::try_new(key(0), 7, 5, chunk).unwrap());
        let old_view = context.ready[&key(0)].capture(
            context.drops.get(&key(0)),
            context.container_chunks.get(&key(0)),
        );
        let before = context.drops[&key(0)].records()[0].clone();
        let mut after = before.clone();
        after.age += 1;
        after.pickup_delay -= 1;
        context
            .stage(RuleEffect::DropPatch {
                before,
                after: Some(after.clone()),
            })
            .unwrap();
        assert!(context.dirty_chunks.is_empty());
        assert!(!context.drops[&key(0)].dirty);
        reset_tick_finishes();
        context.commit_carried();
        drop(context);
        assert_eq!(tick_finishes(), 0);
        assert_eq!(authority.residents.ready[&key(0)].revision, 5);
        assert_eq!(authority.residents.drops[&key(0)].records(), &[after]);
        let latest = authority
            .capture_chunk_snapshot(key(0), SaveUrgency::Autosave)
            .unwrap();
        let SaveValue::ChunkView(latest) = latest.value else {
            unreachable!()
        };
        assert_eq!(latest.revision(), old_view.revision());
        assert_ne!(latest, old_view);
        assert_eq!(old_view.materialize().chunk.drops[0].age_ticks, 10);
        assert_eq!(latest.materialize().chunk.drops[0].age_ticks, 11);
    }

    #[test]
    fn live_idle_tick_finishes_no_ready_chunks() {
        let mut authority = authority(128, false);
        reset_tick_finishes();
        authority.advance_tick(TickBudget::full()).unwrap();
        assert_eq!(tick_finishes(), 0);
        assert!(
            authority
                .residents
                .ready
                .values()
                .all(|chunk| chunk.revision == 5)
        );
    }
}

#[cfg(test)]
mod ready_commit_tests {
    use super::super::world::{reset_tick_finishes, tick_finishes};
    use super::*;
    use mornlea_domain::{BlockPos, ChunkPos, FiniteVec3};
    use mornlea_storage::{ContainerSnapshot, ItemStack, StorageKind};

    fn key() -> ChunkKey {
        ChunkKey {
            dimension: Dimension::OVERWORLD,
            pos: ChunkPos::new(0, 0),
        }
    }

    fn authority(revision: u64) -> AuthorityState {
        let mut authority = AuthorityState::try_new(
            ServerLimits::try_new(8, 4096, 512, 64, 64, 1_048_576).unwrap(),
            0,
        )
        .unwrap();
        let chunk = Chunk {
            sections: vec![
                ContainerSnapshot {
                    kind: StorageKind::Single,
                    bits: 0,
                    single: 0,
                    palette: Vec::new(),
                    packed: Vec::new(),
                };
                24
            ],
            drops: vec![Default::default(); 32],
            furnaces: vec![Default::default(); 32],
            chests: vec![Default::default(); 16],
        };
        let mut ctx = TickContext::harness(&mut authority, TickBudget::full());
        ctx.preload_ready_chunk(ReadyChunk::try_new(key(), 7, revision, chunk).unwrap());
        let residents = ctx.resident_snapshot();
        drop(ctx);
        authority.commit_residents(residents);
        authority
    }

    fn write(
        ctx: &mut TickContext<'_>,
        pos: BlockPos,
        block: u16,
    ) -> Result<MutationOutcome, RuleReject> {
        let observed = ctx.read().observation(Dimension::OVERWORLD, pos).unwrap();
        ctx.transaction().try_system(
            SystemRule::Support,
            vec![BlockWrite::try_new(observed, block).unwrap()],
        )
    }

    fn commit(mut ctx: TickContext<'_>) {
        ctx.commit_carried();
    }

    #[test]
    fn resident_save_materializes_all_carried_block_writes() {
        let mut authority = authority(5);
        let mut ctx = TickContext::for_tick(&mut authority, TickBudget::full());
        for x in [1, 2] {
            write(&mut ctx, BlockPos::new(x, 64, 1), 4).unwrap();
        }
        commit(ctx);
        let snapshots = authority.residents().ready_snapshot();
        let saved = &snapshots[0];
        let restored = ReadyChunk::try_new(saved.0, saved.1, saved.2, saved.3.clone()).unwrap();
        for x in [1, 2] {
            assert_eq!(restored.block(BlockPos::new(x, 64, 1)), Some(4));
        }
        assert_eq!(saved.2, 6);
    }

    #[test]
    fn later_dirty_ticks_advance_once_and_noop_live_ticks_preserve_revision() {
        let mut authority = authority(5);
        let mut ctx = TickContext::for_tick(&mut authority, TickBudget::full());
        write(&mut ctx, BlockPos::new(1, 64, 1), 4).unwrap();
        write(&mut ctx, BlockPos::new(2, 64, 1), 4).unwrap();
        commit(ctx);
        authority.advance_tick(TickBudget::full()).unwrap();
        let mut ctx = TickContext::for_tick(&mut authority, TickBudget::full());
        write(&mut ctx, BlockPos::new(3, 64, 1), 4).unwrap();
        commit(ctx);
        assert_eq!(authority.residents().ready_snapshot()[0].2, 7);
        let saved = &authority.residents().ready_snapshot()[0];
        let restored = ReadyChunk::try_new(saved.0, saved.1, saved.2, saved.3.clone()).unwrap();
        for x in [1, 2, 3] {
            assert_eq!(restored.block(BlockPos::new(x, 64, 1)), Some(4));
        }
        authority.advance_tick(TickBudget::full()).unwrap();
        assert_eq!(authority.residents().ready_snapshot()[0].2, 7);
    }

    #[test]
    fn ready_query_uses_pending_chunk_identity_and_refuses_sparse_coverage() {
        let mut authority = authority(5);
        let empty = authority.residents().ready_snapshot()[0].3.clone();
        let mut ctx = TickContext::for_tick(&mut authority, TickBudget::full());
        let missing = ChunkKey {
            dimension: Dimension::OVERWORLD,
            pos: ChunkPos::new(1, 0),
        };
        let other_dimension = ChunkKey {
            dimension: Dimension::DEPTHS,
            pos: key().pos,
        };
        assert_eq!(ctx.read().ready_chunk_revision(key()), Some(5));
        assert_eq!(ctx.read().ready_chunk_revision(other_dimension), None);
        ctx.preload_block(
            BlockObservation::try_new(missing, 1, 99, BlockPos::new(16, 64, 0), 4).unwrap(),
        );
        assert_eq!(ctx.read().ready_chunk_revision(missing), None);
        ctx.preload_ready_chunk(
            ReadyChunk::try_new(other_dimension, 7, 17, empty.clone()).unwrap(),
        );
        ctx.preload_ready_chunk(ReadyChunk::try_new(missing, 7, 23, empty).unwrap());
        write(&mut ctx, BlockPos::new(1, 64, 1), 0).unwrap();
        assert_eq!(ctx.read().ready_chunk_revision(key()), Some(5));
        write(&mut ctx, BlockPos::new(1, 64, 1), 4).unwrap();
        write(&mut ctx, BlockPos::new(2, 64, 1), 4).unwrap();
        assert_eq!(ctx.read().ready_chunk_revision(key()), Some(6));
        assert_eq!(ctx.read().ready_chunk_revision(other_dimension), Some(17));
        assert_eq!(ctx.read().ready_chunk_revision(missing), Some(23));
        assert_eq!(
            ctx.resident_snapshot().ready_snapshot(),
            ctx.resident_snapshot().ready_snapshot()
        );
        assert_eq!(ctx.read().ready_chunk_revision(key()), Some(6));
        commit(ctx);
        let ctx = TickContext::for_tick(&mut authority, TickBudget::full());
        assert_eq!(ctx.read().ready_chunk_revision(key()), Some(6));
        assert_eq!(ctx.read().ready_chunk_revision(other_dimension), Some(17));
        assert_eq!(ctx.read().ready_chunk_revision(missing), Some(23));
    }

    #[test]
    fn exhausted_chunk_revision_refuses_container_touch_before_publication() {
        let mut authority = authority(5);
        let mut ctx = TickContext::for_tick(&mut authority, TickBudget::full());
        write(&mut ctx, BlockPos::new(1, 64, 1), 11).unwrap();
        commit(ctx);
        let chunk = authority.residents().ready_snapshot()[0].3.clone();
        let mut ctx = TickContext::for_tick(&mut authority, TickBudget::full());
        ctx.preload_ready_chunk(ReadyChunk::try_new(key(), 7, u64::MAX, chunk).unwrap());
        let reference = ctx.container_chunks[&key()].references(key())[0];
        let before = ctx.read().container(reference).unwrap();
        let snapshot = ctx.resident_snapshot().ready_snapshot();
        assert_eq!(
            ctx.stage(RuleEffect::Container {
                before: before.clone(),
                after: before.clone()
            }),
            Err(RuleReject::StaleObservation)
        );
        assert_eq!(ctx.read().container(reference), Some(before));
        assert!(ctx.dirty_chunks.is_empty());
        assert_eq!(ctx.resident_snapshot().ready_snapshot(), snapshot);
        assert_eq!(ctx.read().ready_chunk_revision(key()), Some(u64::MAX));
    }

    #[test]
    fn exhausted_chunk_revision_refuses_changed_effects_and_admits_equal_writes() {
        let mut authority = authority(u64::MAX);
        let mut ctx = TickContext::for_tick(&mut authority, TickBudget::full());
        let pos = BlockPos::new(1, 64, 1);
        let before = ctx.resident_snapshot().ready_snapshot();
        write(&mut ctx, pos, 0).unwrap();
        assert_eq!(write(&mut ctx, pos, 4), Err(RuleReject::StaleObservation));
        let drops = DropBatch::try_new(
            DropSource::System {
                rule: SystemRule::Support,
                tick: ctx.read().tick(),
                target: pos,
            },
            Dimension::OVERWORLD,
            FiniteVec3::try_new([1.5, 64.5, 1.5]).unwrap(),
            vec![ItemStack {
                item: 2,
                count: 1,
                durability: 0,
            }],
            5,
        )
        .unwrap();
        assert_eq!(
            ctx.transaction()
                .try_system_with_drops(SystemRule::Support, Vec::new(), drops),
            Err(RuleReject::StaleObservation)
        );
        assert_eq!(ctx.read().ready_chunk_revision(key()), Some(u64::MAX));
        assert_eq!(ctx.resident_snapshot().ready_snapshot(), before);
        assert!(ctx.dirty_chunks.is_empty());
    }

    #[test]
    fn defensive_compound_rollback_restores_chunk_dirty_identity() {
        let mut authority = authority(5);
        let mut ctx = TickContext::for_tick(&mut authority, TickBudget::full());
        let pos = BlockPos::new(1, 64, 1);
        let observed = ctx.read().observation(Dimension::OVERWORLD, pos).unwrap();
        let compound = RuleEffect::Compound(vec![
            RuleEffect::Sleep(SleepState::try_new(Vec::new(), 23, None).unwrap()),
            RuleEffect::Blocks(BlockTxn::system(
                SystemRule::Support,
                ctx.read().tick(),
                vec![BlockWrite::try_new(observed, 4).unwrap()],
            )),
            RuleEffect::Projectile {
                before: None,
                after: None,
            },
        ]);
        assert_eq!(
            ctx.apply_effect(compound),
            Err(RuleReject::Wire(RejectReason::InvalidInput))
        );
        assert_eq!(ctx.read().block(Dimension::OVERWORLD, pos), Some(0));
        assert_eq!(ctx.read().ready_chunk_revision(key()), Some(5));
        assert!(ctx.changed_blocks().is_empty());
        assert!(ctx.dirty_chunks.is_empty());
        let captured =
            ctx.ready[&key()].capture(ctx.drops.get(&key()), ctx.container_chunks.get(&key()));
        assert!(
            captured
                .materialize()
                .chunk
                .sections
                .iter()
                .all(|section| section.single == 0)
        );
        assert!(!ctx.sleep_record_touched);
        assert_eq!(ctx.sleep_record.day_phase_offset, 0);
        reset_tick_finishes();
        commit(ctx);
        assert_eq!(tick_finishes(), 0);
    }

    #[test]
    fn container_and_drop_work_share_revision_and_clear_committed_dirty_flags() {
        let mut authority = authority(5);
        let mut ctx = TickContext::for_tick(&mut authority, TickBudget::full());
        write(&mut ctx, BlockPos::new(1, 64, 1), 11).unwrap();
        let before = ctx.container_chunks[&key()]
            .record(key(), ctx.container_chunks[&key()].references(key())[0])
            .unwrap();
        let mut after = before.clone();
        let ContainerSlots::Chest(items) = &mut after.slots else {
            panic!("expected chest")
        };
        items[0] = ItemStack {
            item: 2,
            count: 3,
            durability: 0,
        };
        ctx.stage(RuleEffect::Container { before, after }).unwrap();
        let pos = BlockPos::new(2, 64, 1);
        let drops = DropBatch::try_new(
            DropSource::System {
                rule: SystemRule::Support,
                tick: ctx.read().tick(),
                target: pos,
            },
            Dimension::OVERWORLD,
            FiniteVec3::try_new([2.5, 64.5, 1.5]).unwrap(),
            vec![ItemStack {
                item: 2,
                count: 1,
                durability: 0,
            }],
            5,
        )
        .unwrap();
        ctx.transaction()
            .try_system_with_drops(SystemRule::Support, Vec::new(), drops)
            .unwrap();
        assert_eq!(ctx.dirty_chunks, BTreeSet::from([key()]));
        reset_tick_finishes();
        commit(ctx);
        assert_eq!(tick_finishes(), 1);
        let residents = authority.residents();
        assert!(residents.dirty_chunks.is_empty());
        let saved = &residents.ready_snapshot()[0];
        assert_eq!(saved.2, 6);
        assert_eq!(saved.3.chests[0].items[0].count, 3);
        assert!(saved.3.drops[0].active);
        assert!(!residents.drops[&key()].dirty);
        assert!(!residents.container_chunks[&key()].dirty);
        assert_eq!(
            residents
                .container_records()
                .values()
                .next()
                .unwrap()
                .revision,
            6
        );
        authority.advance_tick(TickBudget::full()).unwrap();
        assert_eq!(authority.residents().ready_snapshot()[0].2, 6);
        let mut ctx = TickContext::for_tick(&mut authority, TickBudget::full());
        let before = ctx
            .read()
            .container(
                residents
                    .container_records()
                    .keys()
                    .next()
                    .copied()
                    .unwrap(),
            )
            .unwrap()
            .clone();
        ctx.stage(RuleEffect::Container {
            before: before.clone(),
            after: before,
        })
        .unwrap();
        commit(ctx);
        assert_eq!(authority.residents().ready_snapshot()[0].2, 7);
    }
}

#[cfg(test)]
mod support_changed_rollback_tests {
    use super::*;
    use mornlea_domain::{BlockPos, ChunkPos};

    /// A defensive compound failure must restore the mutation fact along
    /// with the staged block, even when an earlier change already existed.
    #[test]
    fn later_apply_failure_restores_changed_cells() {
        let mut authority = AuthorityState::try_new(
            ServerLimits::try_new(8, 4096, 512, 64, 64, 1_048_576).unwrap(),
            0,
        )
        .unwrap();
        let mut ctx = TickContext::harness(&mut authority, TickBudget::full());
        let key = ChunkKey {
            dimension: Dimension::OVERWORLD,
            pos: ChunkPos::new(0, 0),
        };
        let first = BlockPos::new(1, 64, 1);
        let later = BlockPos::new(2, 64, 1);
        for pos in [first, later] {
            ctx.preload_block(BlockObservation::try_new(key, 1, 1, pos, 4).unwrap());
        }
        let initial = ctx.read().observation(Dimension::OVERWORLD, first).unwrap();
        ctx.transaction()
            .try_system(
                SystemRule::Support,
                vec![BlockWrite::try_new(initial, 0).unwrap()],
            )
            .unwrap();
        let before = ctx.changed_blocks();
        assert_eq!(before.len(), 1);
        let next = ctx.read().observation(Dimension::OVERWORLD, later).unwrap();
        let compound = RuleEffect::Compound(vec![
            RuleEffect::Blocks(BlockTxn::system(
                SystemRule::Support,
                ctx.read().tick(),
                vec![BlockWrite::try_new(next, 0).unwrap()],
            )),
            RuleEffect::Projectile {
                before: None,
                after: None,
            },
        ]);
        // Directly exercise the defensive apply rollback: public `stage`
        // validates every arm before applying any of them.
        assert_eq!(
            ctx.apply_effect(compound),
            Err(RuleReject::Wire(RejectReason::InvalidInput))
        );
        assert_eq!(ctx.read().block(Dimension::OVERWORLD, first), Some(0));
        assert_eq!(ctx.read().block(Dimension::OVERWORLD, later), Some(4));
        assert_eq!(ctx.changed_blocks(), before);
    }
}

#[cfg(test)]
mod observation_trace_tests {
    use super::*;
    use mornlea_domain::BlockPos;

    #[test]
    fn trace_retains_missing_preimages_and_caps_distinct_cells() {
        let mut trace = ObservationTrace::default();
        let missing = BlockPos::new(0, 65, 0);
        for _ in 0..600 {
            assert!(trace.record(Dimension::OVERWORLD, missing, None));
        }
        assert_eq!(trace.cells.len(), 1);
        assert_eq!(trace.cells[&(Dimension::OVERWORLD, missing)], None);
        for x in 1..512 {
            assert!(trace.record(Dimension::OVERWORLD, BlockPos::new(x, 65, 0), None));
        }
        assert_eq!(trace.check_capacity(), Ok(()));
        assert!(!trace.record(Dimension::OVERWORLD, BlockPos::new(512, 65, 0), None));
        assert_eq!(trace.cells.len(), 512);
        assert_eq!(
            trace.check_capacity(),
            Err(RuleReject::ResourceFull(Resource::RuleEffects))
        );
        assert!(!trace.record(Dimension::OVERWORLD, missing, None));
    }
}

#[cfg(test)]
mod compound_inventory_tests {
    use super::*;
    use crate::core::mutation::resolve_place as resolve_player_placement;
    use mornlea_domain::{
        BlockPos, ChunkPos, LookAngles, PlacementIntent, Season, WorldStateParts,
    };
    use mornlea_protocol::{LoginStart, admit_login};
    use mornlea_storage::ItemStack;

    fn world() -> WorldState {
        WorldState::try_new(WorldStateParts {
            day_phase_offset: 0,
            world_time_ticks: 0,
            weather: Weather::Clear,
            season: Season::Spring,
            season_progress: 0,
            temperature: 0,
        })
        .unwrap()
    }

    #[test]
    fn mixed_resolved_placement_and_inventory_chain_is_atomic() {
        for ordinary_first in [false, true] {
            for stale in [false, true] {
                let mut authority = AuthorityState::try_new(
                    ServerLimits::try_new(8, 4096, 512, 64, 64, 1_048_576).unwrap(),
                    7,
                )
                .unwrap();
                let player =
                    PlayerId::try_from_bytes([1, 0, 0, 0, 0, 0, 64, 0, 128, 0, 0, 0, 0, 0, 0, 1])
                        .unwrap();
                let start = LoginStart::new(player, "Compound", 8).unwrap();
                let inbound = LoginStart::decode_inbound(&start.encode().unwrap()).unwrap();
                let session = authority
                    .admit(admit_login(inbound).unwrap(), TransportKind::Memory)
                    .unwrap();
                let actor = ActorKey::Player(session);
                let mut save = canonical_player(player, "Compound").unwrap();
                save.current.position = [0.5, 64.0, 0.5];
                save.yaw = std::f32::consts::PI;
                save.inventory.hotbar.slots[0] = ItemStack {
                    item: 2,
                    count: 2,
                    durability: 0,
                };
                let mut ctx = TickContext::harness(&mut authority, TickBudget::full());
                ctx.stage_login(seed_player(session, &save).unwrap());
                ctx.stage(RuleEffect::Environment(EnvironmentState {
                    seed: 7,
                    next_tick: 0,
                    world_time: 0,
                    day_phase_offset: 0,
                    season_offset: 0,
                    weather: Weather::Clear,
                    weather_remaining: 0,
                    difficulty: 0,
                    tunables: RuleTunables::source_defaults(),
                }))
                .unwrap();
                let key = ChunkKey {
                    dimension: Dimension::OVERWORLD,
                    pos: ChunkPos::new(0, 0),
                };
                for (z, block) in [(0, 0), (1, 0), (2, 2)] {
                    ctx.preload_block(
                        BlockObservation::try_new(key, 1, 1, BlockPos::new(0, 65, z), block)
                            .unwrap(),
                    );
                }
                let before = *ctx.read().inventory(actor).unwrap();
                let mut middle = before;
                middle.slots[0].count = 1;
                let mut after = middle;
                after.slots[0] = ItemStack::default();
                // Resolve against the actual inventory basis consumed at this
                // position in the compound, then restore its initial preimage.
                if ordinary_first && !stale {
                    ctx.preload_inventory(actor, middle);
                }
                let intent = PlacementIntent::try_new(
                    LookAngles::try_new(std::f32::consts::PI, 0.0).unwrap(),
                    0,
                )
                .unwrap();
                let resolved = resolve_player_placement(actor, &intent, &ctx.read()).unwrap();
                ctx.preload_inventory(actor, before);
                let block_effect = RuleEffect::Blocks(resolved.into_txn());
                let inventory_effect = if ordinary_first {
                    RuleEffect::Inventory(InventoryPatch::try_new(actor, before, middle).unwrap())
                } else {
                    RuleEffect::Inventory(
                        InventoryPatch::try_new(actor, if stale { before } else { middle }, after)
                            .unwrap(),
                    )
                };
                let effects = if ordinary_first {
                    vec![inventory_effect, block_effect]
                } else {
                    vec![block_effect, inventory_effect]
                };
                let snapshot = ctx.snapshot_state(world());
                let blocks = ctx.blocks.clone();
                let changed = ctx.changed_blocks();
                let result = ctx.stage(RuleEffect::Compound(effects));
                if stale {
                    assert_eq!(
                        result,
                        Err(RuleReject::StaleObservation),
                        "ordinary_first={ordinary_first}"
                    );
                    assert_eq!(ctx.snapshot_state(world()), snapshot);
                    assert_eq!(ctx.blocks, blocks);
                    assert_eq!(ctx.changed_blocks(), changed);
                } else {
                    result.unwrap();
                    assert_eq!(ctx.read().inventory(actor), Some(&after));
                    assert_eq!(
                        ctx.read()
                            .block(Dimension::OVERWORLD, BlockPos::new(0, 65, 1)),
                        Some(3)
                    );
                    assert_eq!(ctx.changed_blocks().len(), 1);
                }
                assert!(ctx.read().drops(key).is_empty());
                assert!(ctx.events().is_empty());
            }
        }
    }
}

#[cfg(test)]
mod player_publication_tests {
    use super::*;
    use mornlea_domain::{BlockPos, FiniteVec3, HotbarSlot, MiningState, MotionStateParts};
    use mornlea_storage::ItemStack;

    fn fixture() -> (SeededPlayer, EnvironmentState) {
        let player =
            PlayerId::try_from_bytes([1, 0, 0, 0, 0, 0, 64, 0, 128, 0, 0, 0, 0, 0, 0, 1]).unwrap();
        let seeded = seed_player(
            SessionKey::from_raw(1).unwrap(),
            &canonical_player(player, "Ada").unwrap(),
        )
        .unwrap();
        let environment = EnvironmentState {
            seed: 0,
            next_tick: 1,
            world_time: 72_000,
            day_phase_offset: 7_800,
            season_offset: 0,
            weather: Weather::Clear,
            weather_remaining: 5_000,
            difficulty: 0,
            tunables: RuleTunables::source_defaults(),
        };
        (seeded, environment)
    }

    fn login(authority: &mut AuthorityState, tag: u8) -> SessionKey {
        let player =
            PlayerId::try_from_bytes([tag, 0, 0, 0, 0, 0, 64, 0, 128, 0, 0, 0, 0, 0, 0, 1])
                .unwrap();
        let start = mornlea_protocol::LoginStart::new(player, "Ada", 8).unwrap();
        let inbound =
            mornlea_protocol::LoginStart::decode_inbound(&start.encode().unwrap()).unwrap();
        let session = authority
            .admit(
                mornlea_protocol::admit_login(inbound).unwrap(),
                TransportKind::Memory,
            )
            .unwrap();
        let mut context = TickContext::for_tick(authority, TickBudget::full());
        context
            .stage_login(seed_player(session, &canonical_player(player, "Ada").unwrap()).unwrap());
        context.stage(RuleEffect::Environment(fixture().1)).unwrap();
        context.commit_carried();
        drop(context);
        session
    }

    #[test]
    fn reconnect_history_never_enters_live_projection_index() {
        let mut authority = AuthorityState::try_new(
            ServerLimits::try_new(8, 4096, 512, 64, 64, 1_048_576).unwrap(),
            0,
        )
        .unwrap();
        for _ in 0..32 {
            let session = login(&mut authority, 1);
            assert_eq!(authority.residents.player_slots.len(), 1);
            authority.retire(session, CloseReason::PeerGone).unwrap();
            assert!(authority.residents.player_slots.is_empty());
        }
        let current = login(&mut authority, 1);
        assert_eq!(authority.residents.actors.len(), 33);
        assert_eq!(authority.residents.player_slots.len(), 1);
        let events = authority.project_player_updates(5);
        assert_eq!(events.len(), 1);
        assert_eq!(
            events[0].recipient(),
            EventRecipient::Session(current.get())
        );
    }

    #[test]
    fn invalid_private_projection_retires_only_its_recipient() {
        let mut authority = AuthorityState::try_new(
            ServerLimits::try_new(8, 4096, 512, 64, 64, 1_048_576).unwrap(),
            0,
        )
        .unwrap();
        let broken = login(&mut authority, 1);
        let healthy = login(&mut authority, 2);
        authority.residents.mining.insert(
            ActorKey::Player(broken),
            MiningProgress {
                actor: ActorKey::Player(broken),
                dimension: Dimension::OVERWORLD,
                target: BlockPos::new(1, 65, 0),
                observed_block: 17,
                tool_slot: HotbarSlot::new(0).unwrap(),
                tool: ItemStack::default(),
                elapsed: 4,
                required: u32::MAX,
                last_tick: 5,
            },
        );
        let events = authority.project_player_updates(5);
        assert_eq!(events.len(), 1);
        assert_eq!(
            events[0].recipient(),
            EventRecipient::Session(healthy.get())
        );
        assert_eq!(
            authority.session(broken).unwrap().phase,
            SessionPhase::Retired
        );
        assert_eq!(
            authority.session(healthy).unwrap().phase,
            SessionPhase::Active
        );
        assert!(!authority.residents.player_slots.contains_key(&broken));
    }

    #[test]
    fn final_temperature_matches_source_solstice_and_altitude_anchors() {
        for (time, offset, y, weather, expected) in [
            (72_000, 7_800, 64.0, Weather::Clear, 30),
            (72_000, 7_800, 88.0, Weather::Clear, 0),
            (72_000, 7_800, 88.0, Weather::Rain, -4),
            (72_000, 7_800, 64.0, Weather::Rain, 26),
            (216_000, 4_200, 64.0, Weather::Clear, -8),
            (216_000, 4_200, 319.0, Weather::Thunder, -40),
        ] {
            let (mut seeded, mut environment) = fixture();
            seeded.actor.motion = MotionState::new(MotionStateParts {
                position: FiniteVec3::try_new([0.5, y, 0.5]).unwrap(),
                velocity: seeded.actor.motion.velocity(),
                on_ground: true,
            });
            environment.world_time = time;
            environment.day_phase_offset = offset;
            environment.weather = weather;
            let state = super::super::player_publication::project(
                9,
                7,
                &seeded.actor,
                Some(&seeded.inventory),
                None,
                None,
                &environment,
            )
            .unwrap();
            assert_eq!(
                state.world().temperature(),
                expected,
                "time={time} y={y} weather={weather:?}"
            );
        }
    }

    #[test]
    fn final_mining_and_armor_observe_latest_resident_values() {
        let (mut seeded, environment) = fixture();
        seeded.inventory.armor[1] = ItemStack {
            item: 59,
            count: 1,
            durability: 240,
        };
        let ActorBody::Player(body) = &mut seeded.actor.body else {
            unreachable!()
        };
        body.saturation_milli = 0;
        let mut progress = MiningProgress {
            actor: seeded.actor.key,
            dimension: seeded.actor.dimension,
            target: BlockPos::new(1, 65, 0),
            observed_block: 17,
            tool_slot: HotbarSlot::new(0).unwrap(),
            tool: ItemStack::default(),
            elapsed: 4,
            required: 15,
            last_tick: 9,
        };
        let state = super::super::player_publication::project(
            9,
            7,
            &seeded.actor,
            Some(&seeded.inventory),
            None,
            Some(&progress),
            &environment,
        )
        .unwrap();
        assert_eq!(state.survival().armor_points(), 6);
        assert!(state.survival().saturation_zero());
        let MiningState::Active(mining) = state.mining() else {
            panic!("unfinished mining must publish")
        };
        assert_eq!(mining.target(), progress.target);
        assert_eq!(mining.progress(), 4);
        assert_eq!(mining.required(), 15);
        assert!(mining.harvestable());
        progress.required = u32::MAX;
        assert_eq!(
            super::super::player_publication::project(
                9,
                7,
                &seeded.actor,
                None,
                None,
                Some(&progress),
                &environment
            ),
            Err(ServerError::InvalidInput {
                field: "mining_publication"
            })
        );
    }
}

#[cfg(test)]
mod drop_rehearsal_tests {
    use super::*;
    use mornlea_domain::{BlockPos, ChunkPos, FiniteVec3, HostileId, chunk_block_index};
    use mornlea_storage::{DropSlot, ItemStack};

    fn stack(item: u16, count: u8) -> ItemStack {
        ItemStack {
            item,
            count,
            durability: 0,
        }
    }

    fn batch(stacks: Vec<ItemStack>) -> DropBatch {
        DropBatch::try_new(
            DropSource::Death {
                actor: ActorKey::Hostile(HostileId::try_new(1).unwrap()),
                tick: 0,
            },
            Dimension::OVERWORLD,
            FiniteVec3::try_new([0.5, 1.5, 0.5]).unwrap(),
            stacks,
            40,
        )
        .unwrap()
    }

    #[test]
    fn failed_merge_and_split_leave_cumulative_scratch_and_base_unchanged() {
        let key = ChunkKey {
            dimension: Dimension::OVERWORLD,
            pos: ChunkPos::new(0, 0),
        };
        let mut slots = [DropSlot::default(); 32];
        for (index, slot) in slots.iter_mut().take(31).enumerate() {
            *slot = DropSlot {
                generation: index as u32 + 1,
                active: true,
                stack: stack(1, 64),
                block_index: chunk_block_index(BlockPos::new(0, 70, 0)),
                age_ticks: 100,
                pickup_delay_ticks: 2,
            };
        }
        slots[0].stack = stack(3, 62);
        slots[0].block_index = chunk_block_index(BlockPos::new(0, 1, 0));
        slots[31].generation = 40;
        let base = BTreeMap::from([(key, DropState::new(key, slots))]);
        let ready = BTreeMap::new();
        let mut rehearsal = DropRehearsal {
            acquisition: None,
            drops: &base,
            ready: &ready,
            pending: BTreeMap::new(),
        };
        rehearsal.try_insert(&batch(vec![stack(3, 1)])).unwrap();
        let accepted = rehearsal.pending[&key].slots;
        let accepted_records = rehearsal.pending[&key].records().to_vec();
        assert_eq!(
            rehearsal.try_insert(&batch(vec![stack(3, 2), stack(4, 1)])),
            Err(RuleReject::Wire(RejectReason::DropCapacity)),
        );
        assert_eq!(rehearsal.pending[&key].slots, accepted);
        assert_eq!(rehearsal.pending[&key].records(), accepted_records);
        assert!(rehearsal.pending[&key].dirty);
        rehearsal.try_insert(&batch(vec![stack(4, 1)])).unwrap();
        let final_slots = &rehearsal.pending[&key].slots;
        assert_eq!(final_slots[0].stack, stack(3, 63));
        assert_eq!(final_slots[0].age_ticks, 100);
        assert_eq!(final_slots[31].generation, 41);
        assert_eq!(final_slots[31].stack, stack(4, 1));
        assert_eq!(final_slots[31].age_ticks, 0);
        assert_eq!(base[&key].slots, slots);
        assert!(!base[&key].dirty);
        assert_eq!(rehearsal.pending.len(), 1);
    }
}

#[cfg(test)]
mod live_acquisition_tests {
    #[test]
    fn live_save_actual_owned_commit_old_ack_fresh_refusal_and_settled_cycles() {
        use crate::core::{chunk_driver::ChunkDriver, generation_worker::GenerationPool};
        use crate::store::{
            disk::{DiskOptions, DiskStore},
            mailbox::StoreMailbox,
            scheduler::{AutosaveScheduler, SchedulerConfig},
        };
        use mornlea_storage::{
            BANK_A_START_SECTOR, BANK_B_START_SECTOR, BANK_SIZE, RegionKey, SECTOR_SIZE,
            decode_region_bank,
        };
        use std::{
            fs, thread,
            time::{Duration, Instant},
        };
        struct Root(std::path::PathBuf);
        impl Drop for Root {
            fn drop(&mut self) {
                let _ = fs::remove_dir_all(&self.0);
            }
        }
        fn deadline() -> Deadline {
            Deadline::after(Instant::now(), Duration::from_secs(10)).unwrap()
        }
        fn complete(
            store: &mut AutosaveScheduler<DiskStore>,
            ticket: SaveTicket,
        ) -> SaveCompletion {
            let until = deadline();
            loop {
                store.drive_workers();
                if let SavePoll::Completed(c) = StoreHandle::poll(store, ticket) {
                    return c;
                }
                assert!(!until.expired(Instant::now()));
                thread::yield_now();
            }
        }
        fn scheduler(disk: DiskStore, bytes: usize) -> AutosaveScheduler<DiskStore> {
            AutosaveScheduler::try_new(
                SchedulerConfig::default(),
                StoreMailbox::try_new_background(
                    StoreLimits::try_new(2, 16, 3, 3, 3, 1, 8, bytes).unwrap(),
                    disk,
                )
                .unwrap(),
            )
            .unwrap()
        }
        let root = Root(std::env::temp_dir().join(
            format!("mornlea-owned-live-save-{}-{}",std::process::id(),
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()),
        ));
        let mut a = authority();
        let metadata = a.metadata.clone();
        let options = || DiskOptions {
            region_handle_cap: 1,
            create: metadata.clone(),
        };
        let mut disk = DiskStore::open(&root.0, options()).unwrap();
        let expected = chunk();
        for revision in [7, 8] {
            let mut value = expected.clone();
            if revision == 8 {
                value.sections[0].single = 3;
            }
            let snapshot = OwnedSnapshot::try_new(
                SaveKey::Chunk(key()),
                revision,
                4096,
                SaveUrgency::Autosave,
                SaveValue::Chunk(mornlea_storage::ChunkSave {
                    key: mornlea_storage::ChunkKey {
                        dimension: 0,
                        x: 0,
                        z: 0,
                    },
                    revision,
                    chunk: value,
                }),
            )
            .unwrap();
            let c = disk.write(
                SaveTicket::try_from_raw(revision).unwrap(),
                SaveRequest {
                    snapshots: vec![snapshot],
                },
            );
            assert!(c.error.is_none());
        }
        disk.close().unwrap();
        let path = root.0.join("dimensions/0/regions/r.0.0.region");
        let mut bytes = fs::read(&path).unwrap();
        let rk = RegionKey {
            dimension: 0,
            x: 0,
            z: 0,
        };
        let banks: Vec<_> = [BANK_A_START_SECTOR, BANK_B_START_SECTOR]
            .into_iter()
            .map(|sector| {
                let at = sector as usize * SECTOR_SIZE as usize;
                decode_region_bank(rk, &bytes[at..at + BANK_SIZE], bytes.len() as i64).unwrap()
            })
            .collect();
        let active = banks.iter().max_by_key(|b| b.generation).unwrap();
        bytes[active.entries[0].offset_sector as usize * SECTOR_SIZE as usize] ^= 0xff;
        fs::write(path, bytes).unwrap();
        let mut store = scheduler(DiskStore::open(&root.0, options()).unwrap(), 4_194_304);
        a.enable_live_chunks().unwrap();
        a.replace_chunk_wants(BTreeSet::from([key()])).unwrap();
        let mut driver = ChunkDriver::new();
        let mut pool = GenerationPool::try_new(42, false, 1).unwrap();
        driver
            .start_load(&mut a, &mut store, key(), deadline())
            .unwrap();
        let until = deadline();
        loop {
            store.drive_workers();
            let r = driver.poll(&mut a, &mut store, &mut pool);
            if r.retained == 0 {
                assert!(r.first_error.is_none());
                break;
            }
            assert!(!until.expired(Instant::now()));
            thread::yield_now();
        }
        a.advance_tick(TickBudget::full()).unwrap();
        pool.close(deadline()).unwrap();
        let old = a.select(SaveMode::All, SaveBudget::default());
        let SaveValue::ChunkView(old_view) = &old[0].value else {
            panic!("view");
        };
        assert_eq!(old_view.materialize().chunk, expected);
        let old_ticket = store.submit(SaveRequest { snapshots: old }).unwrap();
        save_mutate(&mut a);
        let now = a
            .capture_chunk_snapshot(key(), SaveUrgency::Autosave)
            .unwrap();
        assert_eq!((now.revision, now.estimated_bytes), (10, 6164));
        world::reset_payload_work();
        world::reset_ready_clones();
        world::reset_materializations();
        assert_eq!(
            a.apply_completion(complete(&mut store, old_ticket)).acked,
            1
        );
        assert_eq!(
            (
                world::payload_work(),
                world::ready_clones(),
                world::materializations()
            ),
            ((0, 0, 0, 0), 0, 0)
        );
        let f = a.live_chunk_facts(key()).unwrap();
        assert_eq!(
            (
                f.revision,
                f.persisted_revision,
                f.needs_rewrite,
                f.recovered
            ),
            (10, 9, true, true)
        );
        store.close(deadline()).unwrap();
        let mut refusing = scheduler(DiskStore::open(&root.0, options()).unwrap(), 1);
        let selected = a.select(SaveMode::All, SaveBudget::default());
        let exact = selected[0].clone();
        let refusal = refusing
            .submit(SaveRequest {
                snapshots: selected,
            })
            .unwrap_err();
        assert_eq!(refusal.request.snapshots, vec![exact]);
        save_write(&mut a, 2);
        a.return_dirty(refusal.request.snapshots.into_iter().next().unwrap());
        let latest = a.select(SaveMode::All, SaveBudget::default());
        assert_eq!(latest[0].revision, 11);
        let SaveValue::ChunkView(view) = &latest[0].value else {
            panic!("view");
        };
        let latest_body = view.materialize().chunk;
        refusing.close(deadline()).unwrap();
        use crate::store::io::{DiskIo, IoPhase};
        use std::sync::{
            Arc,
            atomic::{AtomicBool, Ordering},
        };
        struct FailWrite(Arc<AtomicBool>);
        impl DiskIo for FailWrite {
            fn boundary(&mut self, point: IoFaultPoint, phase: IoPhase) -> std::io::Result<()> {
                if point == IoFaultPoint::PayloadWrite
                    && phase == IoPhase::Before
                    && self.0.swap(false, Ordering::SeqCst)
                {
                    return Err(std::io::ErrorKind::Other.into());
                }
                Ok(())
            }
        }
        let failure = Arc::new(AtomicBool::new(false));
        let hook_failure = failure.clone();
        let disk = DiskStore::with_io(
            &root.0,
            options(),
            Box::new(move || Box::new(FailWrite(hook_failure.clone()))),
        )
        .unwrap();
        let mut store = scheduler(disk, 4_194_304);
        a.return_dirty(latest.into_iter().next().unwrap());
        failure.store(true, Ordering::SeqCst);
        store
            .poll_tick(6000, SaveBudget::default(), &mut a)
            .unwrap();
        save_write(&mut a, 3);
        let until = deadline();
        while store.pending_retry_jobs() == 0 {
            store.drive_workers();
            store
                .poll_tick(6001, SaveBudget::default(), &mut a)
                .unwrap();
            assert!(!until.expired(Instant::now()));
            thread::yield_now();
        }
        assert_eq!(store.pending_retry_state(), vec![(1, 6021)]);
        assert_eq!(a.save_stats().in_flight, 1);
        assert_eq!(a.save_stats().dirty, 1);
        assert_eq!(a.live_chunk_facts(key()).unwrap().persisted_revision, 9);
        assert_eq!(a.live_chunk_facts(key()).unwrap().revision, 12);
        assert!(a.select(SaveMode::All, SaveBudget::default()).is_empty());
        struct RealClock;
        impl Clock for RealClock {
            fn monotonic(&self) -> Instant {
                Instant::now()
            }
            fn unix_ms(&self) -> i64 {
                0
            }
        }
        store.flush(deadline(), &mut a, &RealClock).unwrap();
        assert_eq!(a.save_stats(), SaveStats::default());
        assert_eq!(a.live_chunk_facts(key()).unwrap().persisted_revision, 12);
        // Every cycle commits a real owned write and settles an actual disk reply.
        for cycle in 0..64 {
            save_write(&mut a, if cycle % 2 == 0 { 1 } else { 2 });
            world::reset_payload_work();
            world::reset_ready_clones();
            world::reset_materializations();
            let target = a
                .select(SaveMode::All, SaveBudget::default())
                .pop()
                .unwrap();
            a.return_dirty(target);
            let selected = a.select(SaveMode::All, SaveBudget::default());
            let t = store
                .submit(SaveRequest {
                    snapshots: selected,
                })
                .unwrap();
            assert_eq!(a.apply_completion(complete(&mut store, t)).acked, 1);
            assert_eq!(a.save_stats(), SaveStats::default());
            assert_eq!(
                (
                    world::payload_work(),
                    world::ready_clones(),
                    world::materializations()
                ),
                ((0, 0, 0, 0), 0, 0)
            );
        }
        store.close(deadline()).unwrap();
        let mut reopened = DiskStore::open(&root.0, options()).unwrap();
        let LoadedValue::Chunk(reopened_body) = reopened.load(SaveKey::Chunk(key())).unwrap()
        else {
            panic!("chunk");
        };
        reopened.close().unwrap();
        assert_eq!(reopened_body.revision, 76);
        assert_eq!(reopened_body.chunk, latest_body);
        assert!(a.freeze().save_keys.is_empty());
        assert_eq!(a.acquisition.ownership_counts().0, 1);
        assert!(a.live_chunk_facts(key()).unwrap().recovered);
    }
    #[test]
    fn live_save_defensive_capture_correlation_preserves_identity_refusal() {
        let mut a = save_authority(1, 1);
        let facts = a.live_chunk_facts(key()).unwrap();
        let SaveValue::ChunkView(before) = a
            .capture_chunk_snapshot(key(), SaveUrgency::Autosave)
            .unwrap()
            .value
        else {
            panic!("immutable resident view");
        };
        // Defensive correlation evidence: the private cache contradicts a real
        // capture; ordinary checked source commits keep these values aligned.
        a.acquisition
            .committed(key(), facts.generation, facts.revision, Some(6164));
        world::reset_payload_work();
        world::reset_ready_clones();
        world::reset_materializations();
        assert!(a.select(SaveMode::All, SaveBudget::default()).is_empty());
        assert_eq!(
            (
                world::payload_work(),
                world::ready_clones(),
                world::materializations()
            ),
            ((0, 0, 0, 0), 0, 0)
        );
        assert_eq!(a.live_chunk_facts(key()), Some(facts));
        assert_eq!(
            a.save_stats(),
            SaveStats {
                dirty: 1,
                in_flight: 0,
                estimated_unsaved_bytes: crate::store::mailbox::CHUNK_MAX_RESERVATION,
            }
        );
        assert_eq!(a.acquisition.ownership_counts().0, 1);
        assert_eq!(a.freeze().save_keys, vec![SaveKey::Chunk(key())]);
        let SaveValue::ChunkView(after) = a
            .capture_chunk_snapshot(key(), SaveUrgency::Autosave)
            .unwrap()
            .value
        else {
            panic!("retained resident view");
        };
        assert_eq!(after.materialize(), before.materialize());
        let original = ServerError::Internal {
            invariant: "chunk save identity",
        };
        assert_eq!(a.live_chunk_error(key()), Some(&original));
        a.replace_chunk_wants(BTreeSet::new()).unwrap();
        assert!(a.select(SaveMode::Urgent, SaveBudget::default()).is_empty());
        assert_eq!(a.live_chunk_error(key()), Some(&original));
        assert_eq!(a.save_stats().in_flight, 0);
    }
    #[test]
    fn live_save_invalid_private_estimate_is_conservative_and_never_eligible() {
        let mut a = save_authority(1, 1);
        let r = a.residents.ready.get_mut(&key()).unwrap();
        r.set_block(BlockPos::new(0, -64, 0), 90);
        r.finish_tick(false);
        assert!(a.select(SaveMode::All, SaveBudget::default()).is_empty());
        assert_eq!(
            a.live_chunk_error(key()),
            Some(&ServerError::Internal {
                invariant: "chunk payload estimate"
            })
        );
        assert_eq!(
            a.save_stats(),
            SaveStats {
                dirty: 1,
                in_flight: 0,
                estimated_unsaved_bytes: crate::store::mailbox::CHUNK_MAX_RESERVATION
            }
        );
        a.acquisition.reset_save_work();
        assert!(a.select(SaveMode::All, SaveBudget::default()).is_empty());
        assert_eq!(a.acquisition.save_work(), (0, 0));
        a.replace_chunk_wants(BTreeSet::new()).unwrap();
        assert_eq!(a.save_stats().dirty, 1);
        assert_eq!(a.freeze().save_keys, vec![SaveKey::Chunk(key())]);
    }
    fn save_key(x: i32) -> ChunkKey {
        ChunkKey {
            pos: ChunkPos::new(x, 0),
            ..key()
        }
    }
    fn save_authority(n: i32, dirty: i32) -> AuthorityState {
        let mut a = live();
        a.replace_chunk_wants((0..n).map(save_key).collect())
            .unwrap();
        for x in 0..n {
            let k = save_key(x);
            let r = a.reserve_chunk_load(k).unwrap();
            let id = request(x as u64 + 1);
            a.bind_chunk_load(r, id).unwrap();
            let p = PreparedChunk::try_new(
                k,
                r.generation(),
                RecoveredChunk {
                    chunk: chunk(),
                    revision: 9,
                    persisted_revision: if x < dirty { 7 } else { 9 },
                    needs_rewrite: x < dirty,
                    recovered: x < dirty,
                },
            )
            .unwrap();
            a.offer_acquired(AcquiredChunkEvent::Load {
                key: k,
                generation: r.generation(),
                request: id,
                result: Ok(Some(p)),
            })
            .unwrap();
            if x % 8 == 7 || x == n - 1 {
                a.advance_tick(TickBudget::full()).unwrap();
            }
        }
        a
    }
    // Synthetic completions exercise contract defenses, not disk durability.
    fn save_completion(snapshots: Vec<OwnedSnapshot>, committed: bool) -> SaveCompletion {
        let submitted: Vec<_> = snapshots
            .iter()
            .map(|s| (s.key.clone(), s.revision))
            .collect();
        SaveCompletion {
            ticket: SaveTicket::try_from_raw(1).unwrap(),
            snapshots,
            committed: if committed { submitted.clone() } else { vec![] },
            submitted,
            error: None,
        }
    }
    fn save_mutate(a: &mut AuthorityState) {
        save_write(a, 1);
    }
    fn save_write(a: &mut AuthorityState, block: u16) {
        let mut ctx = TickContext::for_tick(a, TickBudget::full());
        let observed = ctx
            .read()
            .observation(Dimension::OVERWORLD, BlockPos::new(0, -64, 0))
            .unwrap();
        ctx.transaction()
            .try_system(
                SystemRule::Support,
                vec![BlockWrite::try_new(observed, block).unwrap()],
            )
            .unwrap();
        ctx.commit_carried();
    }
    #[test]
    fn live_save_priority_budget_and_eight_flights_span_repeated_selections() {
        let mut a = save_authority(10, 10);
        a.replace_chunk_wants((0..8).map(save_key).collect())
            .unwrap();
        let urgent = a.select(
            SaveMode::Urgent,
            SaveBudget {
                chunks: 0,
                estimated_bytes: 0,
            },
        );
        assert_eq!(urgent.len(), 1);
        assert_eq!(urgent[0].key, SaveKey::Chunk(save_key(8)));
        assert_eq!(urgent[0].urgency, SaveUrgency::Unload);
        let urgent2 = a.select(SaveMode::Urgent, SaveBudget::default());
        assert_eq!(urgent2[0].key, SaveKey::Chunk(save_key(9)));
        assert!(a.select(SaveMode::Urgent, SaveBudget::default()).is_empty());
        let selected = a.select(SaveMode::All, SaveBudget::default());
        assert_eq!(selected.len(), 6);
        assert_eq!(selected[0].key, SaveKey::Chunk(save_key(0)));
        assert_eq!(
            a.save_stats(),
            SaveStats {
                dirty: 10,
                in_flight: 8,
                estimated_unsaved_bytes: 18 * 4096
            }
        );
        assert!(a.select(SaveMode::All, SaveBudget::default()).is_empty());
        a.return_dirty(urgent[0].clone());
        let again = a.select(
            SaveMode::All,
            SaveBudget {
                chunks: 3,
                estimated_bytes: 1,
            },
        );
        assert_eq!(again.len(), 1);
        assert_eq!(again[0].key, urgent[0].key);
        assert_ne!(again[0], urgent[0]);
        let frozen = a.freeze();
        assert_eq!(frozen.save_keys.len(), 10);
        a.begin_close();
        a.return_dirty(again[0].clone());
        assert_eq!(a.select(SaveMode::All, SaveBudget::default()).len(), 1);
        a.mark_closed();
        assert!(a.select(SaveMode::All, SaveBudget::default()).is_empty());
    }
    #[test]
    fn live_save_stops_at_first_nonfit_without_skipping_a_smaller_later_key() {
        let mut a = save_authority(3, 3);
        let mut ctx = TickContext::for_tick(&mut a, TickBudget::full());
        let observed = ctx
            .read()
            .observation(Dimension::OVERWORLD, BlockPos::new(16, -64, 0))
            .unwrap();
        ctx.transaction()
            .try_system(
                SystemRule::Support,
                vec![BlockWrite::try_new(observed, 1).unwrap()],
            )
            .unwrap();
        ctx.commit_carried();
        drop(ctx);
        let selected = a.select(
            SaveMode::All,
            SaveBudget {
                chunks: 3,
                estimated_bytes: 8192,
            },
        );
        assert_eq!(selected.len(), 1);
        assert_eq!(selected[0].key, SaveKey::Chunk(save_key(0)));
        assert_eq!(a.save_stats().in_flight, 1);
        a.return_dirty(selected.into_iter().next().unwrap());
        let counted = a.select(
            SaveMode::All,
            SaveBudget {
                chunks: 2,
                estimated_bytes: usize::MAX,
            },
        );
        assert_eq!(counted.len(), 2);
        assert_eq!(counted[1].key, SaveKey::Chunk(save_key(1)));
        assert_eq!(counted[1].estimated_bytes, 6164);
        assert_eq!(a.save_stats().in_flight, 2);
    }
    #[test]
    fn live_save_current_estimate_old_ack_and_exact_fresh_refusal() {
        let mut a = save_authority(1, 1);
        let old = a
            .select(SaveMode::All, SaveBudget::default())
            .pop()
            .unwrap();
        save_mutate(&mut a);
        assert_eq!(a.save_stats().estimated_unsaved_bytes, 4096 + 6164);
        let sibling = a
            .capture_chunk_snapshot(key(), SaveUrgency::Autosave)
            .unwrap();
        a.return_dirty(sibling);
        assert_eq!(a.save_stats().in_flight, 1);
        assert!(a.select(SaveMode::All, SaveBudget::default()).is_empty());
        let ack = a.apply_completion(save_completion(vec![old.clone()], true));
        assert_eq!(ack.acked, 1);
        let f = a.live_chunk_facts(key()).unwrap();
        assert_eq!(
            (
                f.revision,
                f.persisted_revision,
                f.needs_rewrite,
                f.recovered
            ),
            (10, 9, true, true)
        );
        assert_eq!(a.save_stats().estimated_unsaved_bytes, 6164);
        let current = a
            .select(SaveMode::All, SaveBudget::default())
            .pop()
            .unwrap();
        a.return_dirty(old);
        assert_eq!(a.save_stats().in_flight, 1);
        let report = a.apply_completion(save_completion(vec![current], true));
        assert_eq!(report.acked, 1);
        assert_eq!(a.save_stats(), SaveStats::default());
        assert!(a.live_chunk_facts(key()).unwrap().recovered);
        assert!(!a.live_chunk_facts(key()).unwrap().needs_rewrite);
    }
    #[test]
    fn live_save_failure_keeps_exact_capture_and_current_target_charged() {
        let mut a = save_authority(1, 1);
        let old = a
            .select(SaveMode::All, SaveBudget::default())
            .pop()
            .unwrap();
        save_mutate(&mut a);
        let before = a.save_stats();
        let mut c = save_completion(vec![old.clone()], false);
        c.error = Some(ServerError::Cancelled);
        let report = a.apply_completion(c);
        assert_eq!((report.acked, report.released), (0, 1));
        assert_eq!(report.retry, vec![old.clone()]);
        assert_eq!(report.errors, vec![ServerError::Cancelled]);
        assert_eq!(a.save_stats(), before);
        a.return_dirty(old);
        let latest = a
            .select(SaveMode::All, SaveBudget::default())
            .pop()
            .unwrap();
        assert_eq!((latest.revision, latest.estimated_bytes), (10, 6164));
    }
    #[test]
    fn live_save_completion_defenses_validate_whole_batch_before_mutation() {
        let mut a = save_authority(2, 2);
        let selected = a.select(SaveMode::All, SaveBudget::default());
        let before = a.save_stats();
        let sibling = a
            .capture_chunk_snapshot(save_key(1), SaveUrgency::Autosave)
            .unwrap();
        let mut wrong = selected.clone();
        wrong[1] = sibling;
        let mut duplicate_committed = save_completion(selected.clone(), true);
        duplicate_committed.committed = vec![(selected[0].key.clone(), selected[0].revision); 2];
        for c in [
            duplicate_committed,
            save_completion(wrong, true),
            save_completion(vec![selected[0].clone(), selected[0].clone()], true),
        ] {
            let report = a.apply_completion(c);
            assert_eq!(
                report.errors,
                vec![ServerError::Internal {
                    invariant: "save completion identity"
                }]
            );
            assert_eq!((report.acked, report.released), (0, 0));
            assert_eq!(a.save_stats(), before);
            assert_eq!(a.live_chunk_facts(key()).unwrap().persisted_revision, 7);
        }
        for lane in 0..3 {
            let mut c = save_completion(selected.clone(), true);
            match lane {
                0 => c.snapshots = vec![selected[0].clone(); 9],
                1 => c.submitted = vec![(selected[0].key.clone(), 9); 10],
                _ => c.committed = vec![(selected[0].key.clone(), 9); 11],
            }
            assert_eq!(
                a.apply_completion(c).errors,
                vec![ServerError::Capacity {
                    resource: Resource::SaveChunks,
                    limit: 8,
                    observed: 9 + lane
                }]
            );
            assert_eq!(a.save_stats(), before);
        }
        a.acquisition.records_test_generation(save_key(1), 99);
        assert_eq!(
            a.apply_completion(save_completion(selected.clone(), true))
                .errors,
            vec![ServerError::Internal {
                invariant: "chunk save identity"
            }]
        );
        assert_eq!(a.save_stats(), before);
        a.acquisition.records_test_generation(save_key(1), 2);
        a.acquisition.committed(save_key(1), 2, 8, Some(4096));
        assert_eq!(
            a.apply_completion(save_completion(selected, true)).errors,
            vec![ServerError::Internal {
                invariant: "chunk save identity"
            }]
        );
        assert_eq!(a.live_chunk_facts(key()).unwrap().persisted_revision, 7);
    }
    #[test]
    fn live_save_unknown_target_metadata_lane_and_manual_isolation() {
        let mut a = save_authority(1, 1);
        let unknown = a
            .capture_chunk_snapshot(key(), SaveUrgency::Autosave)
            .unwrap();
        assert_eq!(
            a.remember_dirty(unknown.clone()),
            Err(ServerError::InvalidInput {
                field: "manual_live_snapshot"
            })
        );
        assert_eq!(
            a.apply_completion(save_completion(vec![unknown], true))
                .acked,
            0
        );
        let metadata = a.try_metadata_snapshot().unwrap();
        assert_eq!(
            a.apply_completion(save_completion(vec![metadata], true))
                .acked,
            1
        );
        assert_eq!(a.save_stats().dirty, 1);
        let mut legacy = authority();
        let metadata = legacy.metadata_snapshot();
        legacy.remember_dirty(metadata).unwrap();
        assert!(legacy.enable_live_chunks().is_err());
        let _ = legacy.select(SaveMode::All, SaveBudget::default());
        assert!(legacy.enable_live_chunks().is_err());
    }
    #[test]
    fn live_save_thousand_clean_records_have_bounded_work() {
        let mut a = save_authority(1000, 1);
        world::reset_payload_work();
        world::reset_ready_clones();
        world::reset_materializations();
        a.acquisition.reset_save_work();
        assert_eq!(a.save_stats().dirty, 1);
        assert_eq!(a.acquisition.save_work(), (0, 0));
        let first = a.select(SaveMode::All, SaveBudget::default());
        assert_eq!(first.len(), 1);
        assert_eq!(a.acquisition.save_work(), (1, 0));
        assert_eq!(a.apply_completion(save_completion(first, true)).acked, 1);
        assert_eq!(a.acquisition.save_work(), (1, 2));
        assert_eq!(
            (
                world::ready_clones(),
                world::materializations(),
                world::payload_work()
            ),
            (0, 0, (0, 0, 0, 0))
        );
        assert_eq!(a.acquisition.ownership_counts().0, 1000);
        assert!(a.freeze().save_keys.is_empty());
    }

    use super::super::world::{self, PreparedChunk};
    use super::*;
    use mornlea_domain::{BlockPos, ChunkPos, FiniteVec3};
    use mornlea_storage::{ContainerSnapshot, ItemStack, StorageKind};
    fn key() -> ChunkKey {
        ChunkKey {
            dimension: Dimension::OVERWORLD,
            pos: ChunkPos::new(0, 0),
        }
    }
    fn request(n: u64) -> ChunkRequestId {
        ChunkRequestId::try_new(n).unwrap()
    }
    fn authority() -> AuthorityState {
        AuthorityState::try_new(
            ServerLimits::try_new(8, 4096, 512, 64, 64, 1_048_576).unwrap(),
            42,
        )
        .unwrap()
    }
    fn chunk() -> Chunk {
        Chunk {
            sections: vec![
                ContainerSnapshot {
                    kind: StorageKind::Single,
                    bits: 0,
                    single: 0,
                    palette: vec![],
                    packed: vec![]
                };
                24
            ],
            drops: vec![Default::default(); 32],
            furnaces: vec![Default::default(); 32],
            chests: vec![Default::default(); 16],
        }
    }
    fn prepared(generation: u64) -> PreparedChunk {
        PreparedChunk::try_new(
            key(),
            generation,
            RecoveredChunk {
                chunk: chunk(),
                revision: 9,
                persisted_revision: 7,
                needs_rewrite: true,
                recovered: true,
            },
        )
        .unwrap()
    }
    fn live() -> AuthorityState {
        let mut a = authority();
        a.enable_live_chunks().unwrap();
        a.replace_chunk_wants(BTreeSet::from([key()])).unwrap();
        a
    }
    fn queue(a: &mut AuthorityState, chunk: Chunk) {
        let r = a.reserve_chunk_load(key()).unwrap();
        a.bind_chunk_load(r, request(1)).unwrap();
        let p = PreparedChunk::try_new(
            key(),
            r.generation(),
            RecoveredChunk {
                chunk,
                revision: 9,
                persisted_revision: 7,
                needs_rewrite: true,
                recovered: true,
            },
        )
        .unwrap();
        a.offer_acquired(AcquiredChunkEvent::Load {
            key: key(),
            generation: r.generation(),
            request: request(1),
            result: Ok(Some(p)),
        })
        .unwrap();
    }
    fn drop_batch() -> DropBatch {
        DropBatch::try_new(
            DropSource::System {
                rule: SystemRule::Support,
                tick: 0,
                target: BlockPos::new(3, 1, 3),
            },
            Dimension::OVERWORLD,
            FiniteVec3::try_new([3.5, 1.5, 3.5]).unwrap(),
            vec![ItemStack {
                item: 2,
                count: 1,
                durability: 0,
            }],
            5,
        )
        .unwrap()
    }
    #[test]
    fn live_capture_refuses_unavailable_private_payload_estimate() {
        let mut a = live();
        queue(&mut a, chunk());
        a.advance_tick(TickBudget::full()).unwrap();
        let r = a.residents.ready.get_mut(&key()).unwrap();
        r.set_block(BlockPos::new(0, -64, 0), 90);
        r.finish_tick(false);
        assert!(
            a.capture_chunk_snapshot(key(), SaveUrgency::Autosave)
                .is_none()
        );
    }

    #[test]
    fn live_payload_estimate_commit_and_capture_use_only_fixed_metadata() {
        let mut a = live();
        queue(&mut a, chunk());
        world::reset_payload_work();
        world::reset_ready_clones();
        world::reset_materializations();
        a.advance_tick(TickBudget::full()).unwrap();
        let old = a
            .capture_chunk_snapshot(key(), SaveUrgency::Autosave)
            .unwrap();
        assert_eq!(world::payload_work(), (0, 0, 0, 0));
        let mut ctx = TickContext::for_tick(&mut a, TickBudget::full());
        let observed = ctx
            .read()
            .observation(Dimension::OVERWORLD, BlockPos::new(0, -64, 0))
            .unwrap();
        ctx.transaction()
            .try_system(
                SystemRule::Support,
                vec![BlockWrite::try_new(observed, 1).unwrap()],
            )
            .unwrap();
        ctx.commit_carried();
        drop(ctx);
        let changed = a
            .capture_chunk_snapshot(key(), SaveUrgency::Autosave)
            .unwrap();
        assert_eq!(changed.estimated_bytes, 6164);
        assert_eq!(old.estimated_bytes, 4096);
        assert_eq!(changed.revision, old.revision + 1);
        assert_eq!(world::payload_work(), (0, 1, 0, 90));
        assert_eq!((world::ready_clones(), world::materializations()), (0, 0));
        let SaveValue::ChunkView(old_view) = old.value else {
            unreachable!()
        };
        let SaveValue::ChunkView(new_view) = changed.value else {
            unreachable!()
        };
        assert_eq!(world::payload_work(), (0, 1, 0, 90));
        assert_ne!(old_view, new_view);
        assert_eq!(old_view.materialize().chunk.sections[0].single, 0);
        assert_eq!(new_view.materialize().revision, old.revision + 1);
    }

    #[test]
    fn live_payload_estimate_uses_source_air_metric_and_retains_unloading_identity() {
        let mut a = live();
        queue(&mut a, chunk());
        a.advance_tick(TickBudget::full()).unwrap();
        let before = a.live_chunk_facts(key()).unwrap();
        let first = a
            .capture_chunk_snapshot(key(), SaveUrgency::Autosave)
            .unwrap();
        let second = a
            .capture_chunk_snapshot(key(), SaveUrgency::Autosave)
            .unwrap();
        assert_eq!(first.estimated_bytes, 4096);
        assert_eq!(second.estimated_bytes, 4096);
        assert_eq!(first.revision, before.revision);
        assert_ne!(first.value, second.value);
        a.replace_chunk_wants(BTreeSet::new()).unwrap();
        let unloading = a
            .capture_chunk_snapshot(key(), SaveUrgency::Unload)
            .unwrap();
        assert_eq!(unloading.estimated_bytes, 4096);
        assert_eq!(unloading.revision, before.revision);
        assert_eq!(
            a.live_chunk_facts(key()).unwrap().persisted_revision,
            before.persisted_revision
        );
    }

    #[test]
    fn offer_and_actual_tick_installation_clone_and_materialize_no_ready_body() {
        let mut a = live();
        let r = a.reserve_chunk_load(key()).unwrap();
        use crate::store::{
            disk::{DiskOptions, DiskStore},
            mailbox::StoreMailbox,
        };
        use std::{
            fs, thread,
            time::{Duration, Instant, SystemTime, UNIX_EPOCH},
        };
        struct Root(std::path::PathBuf);
        impl Drop for Root {
            fn drop(&mut self) {
                let _ = fs::remove_dir_all(&self.0);
            }
        }
        let root = Root(std::env::temp_dir().join(format!(
                "mornlea-live-counter-{}-{}",
                std::process::id(),
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            )));
        fs::create_dir(&root.0).unwrap();
        let mut saved = chunk();
        saved.drops[31].generation = 19;
        saved.furnaces[31].generation = 20;
        saved.chests[15].generation = 21;
        let mut setup = authority();
        let mut fixture = TickContext::harness(&mut setup, TickBudget::full());
        fixture.preload_ready_chunk(ReadyChunk::try_new(key(), 1, 8, saved).unwrap());
        for (pos, block) in [(BlockPos::new(1, 1, 1), 11), (BlockPos::new(2, 1, 2), 9)] {
            let observed = fixture
                .read()
                .observation(Dimension::OVERWORLD, pos)
                .unwrap();
            fixture
                .transaction()
                .try_system(
                    SystemRule::Support,
                    vec![BlockWrite::try_new(observed, block).unwrap()],
                )
                .unwrap();
        }
        fixture.stage(RuleEffect::Drops(drop_batch())).unwrap();
        let expected = fixture.resident_snapshot().ready_snapshot().remove(0).3;
        drop(fixture);
        let deadline = || Deadline::after(Instant::now(), Duration::from_secs(5)).unwrap();
        let mut store = StoreMailbox::try_new_background(
            StoreLimits::try_new(2, 16, 3, 3, 3, 1, 8, 4_194_304).unwrap(),
            DiskStore::open(
                &root.0,
                DiskOptions {
                    create: a.metadata.clone(),
                    region_handle_cap: 1,
                },
            )
            .unwrap(),
        )
        .unwrap();
        let ticket = store
            .submit(SaveRequest {
                snapshots: vec![
                    OwnedSnapshot::try_new(
                        SaveKey::Chunk(key()),
                        9,
                        1,
                        SaveUrgency::Autosave,
                        SaveValue::Chunk(mornlea_storage::ChunkSave {
                            key: mornlea_storage::ChunkKey {
                                dimension: 0,
                                x: 0,
                                z: 0,
                            },
                            revision: 9,
                            chunk: expected.clone(),
                        }),
                    )
                    .unwrap(),
                ],
            })
            .unwrap();
        let until = deadline();
        loop {
            store.drive_workers();
            if let SavePoll::Completed(completion) = StoreHandle::poll(&mut store, ticket) {
                assert!(completion.error.is_none());
                break;
            }
            assert!(!until.expired(Instant::now()));
            thread::yield_now();
        }
        let ticket = store
            .start_chunk(key(), r.generation(), deadline())
            .unwrap();
        a.bind_chunk_load(r, ticket).unwrap();
        let until = deadline();
        let p = loop {
            store.drive_workers();
            match store.poll_chunk(ticket) {
                ChunkLoadPoll::Loaded(Some(prepared)) => break prepared,
                ChunkLoadPoll::Pending => {
                    assert!(!until.expired(Instant::now()));
                    thread::yield_now();
                }
                ChunkLoadPoll::Failed(error) => panic!("real saved counter load: {error:?}"),
                ChunkLoadPoll::Loaded(None) => panic!("saved counter load was absent"),
            }
        };
        store.close(deadline()).unwrap();
        world::reset_ready_clones();
        world::reset_materializations();
        a.offer_acquired(AcquiredChunkEvent::Load {
            key: key(),
            generation: r.generation(),
            request: ticket,
            result: Ok(Some(p)),
        })
        .unwrap();
        a.advance_tick(TickBudget::full()).unwrap();
        assert_eq!(world::ready_clones(), 0);
        assert_eq!(world::materializations(), 0);
        let facts = a.live_chunk_facts(key()).unwrap();
        assert_eq!(
            (
                facts.revision,
                facts.persisted_revision,
                facts.needs_rewrite,
                facts.recovered
            ),
            (9, 9, false, false)
        );
        assert_eq!(
            ACQUIRE_AVAILABILITY.with(std::cell::Cell::get),
            Some((key(), false, true))
        );
        let ctx = TickContext::for_tick(&mut a, TickBudget::full());
        let view = ctx.read();
        assert_eq!(
            view.block(Dimension::OVERWORLD, BlockPos::new(1, 1, 1)),
            Some(11)
        );
        assert_eq!(view.highest_non_air(Dimension::OVERWORLD, 1, 1), Some(1));
        assert_eq!(
            view.container_at(
                Dimension::OVERWORLD,
                BlockPos::new(1, 1, 1),
                mornlea_domain::ContainerKind::Chest
            )
            .unwrap()
            .reference
            .slot(),
            0
        );
        assert_eq!(
            view.container_at(
                Dimension::OVERWORLD,
                BlockPos::new(2, 1, 2),
                mornlea_domain::ContainerKind::Furnace
            )
            .unwrap()
            .reference
            .slot(),
            0
        );
        assert_eq!(view.drops(key()).len(), 1);
        assert_eq!(
            ctx.drops.get(&key()).unwrap().slots.as_slice(),
            expected.drops
        );
        assert_eq!(
            ctx.container_chunks
                .get(&key())
                .unwrap()
                .furnaces
                .as_slice(),
            expected.furnaces
        );
        assert_eq!(
            ctx.container_chunks.get(&key()).unwrap().chests.as_slice(),
            expected.chests
        );
    }
    #[test]
    fn retained_unloading_gates_every_read_rehearsal_and_compound_preflight() {
        let mut a = live();
        queue(&mut a, chunk());
        a.advance_tick(TickBudget::full()).unwrap();
        let (observation, container, drop_record) = {
            let mut ctx = TickContext::for_tick(&mut a, TickBudget::full());
            let observed = ctx
                .read()
                .observation(Dimension::OVERWORLD, BlockPos::new(1, 1, 1))
                .unwrap();
            ctx.transaction()
                .try_system(
                    SystemRule::Support,
                    vec![BlockWrite::try_new(observed, 11).unwrap()],
                )
                .unwrap();
            ctx.stage(RuleEffect::Drops(drop_batch())).unwrap();
            let o = ctx
                .read()
                .observation(Dimension::OVERWORLD, BlockPos::new(1, 1, 1))
                .unwrap();
            let mut c = ctx
                .read()
                .container_at(
                    Dimension::OVERWORLD,
                    o.pos,
                    mornlea_domain::ContainerKind::Chest,
                )
                .unwrap();
            let d = ctx.read().drops(key())[0].clone();
            ctx.commit_carried();
            c.revision = ctx.authority.live_chunk_facts(key()).unwrap().revision;
            (o, c, d)
        };
        let before = a.live_chunk_facts(key()).unwrap();
        assert_eq!((before.revision, before.persisted_revision), (10, 7));
        a.replace_chunk_wants(BTreeSet::new()).unwrap();
        let unloading = a.live_chunk_facts(key()).unwrap();
        assert_eq!(unloading.phase, LiveChunkPhase::Unloading);
        assert!(
            a.capture_chunk_snapshot(key(), SaveUrgency::Unload)
                .is_some()
        );
        {
            let mut ctx = TickContext::for_tick(&mut a, TickBudget::full());
            // A stale sparse observation must not outrank the managed phase gate.
            ctx.blocks.insert((key(), observation.pos), observation);
            let view = ctx.read();
            assert!(!view.ready_chunk(key()));
            assert!(view.ready_chunk_revision(key()).is_none());
            assert!(view.ready_chunk_keys().is_empty());
            assert!(view.highest_non_air(Dimension::OVERWORLD, 1, 1).is_none());
            assert!(
                view.observation(Dimension::OVERWORLD, observation.pos)
                    .is_none()
            );
            assert!(view.block(Dimension::OVERWORLD, observation.pos).is_none());
            assert!(view.container(container.reference).is_none());
            assert!(
                view.world_container(Dimension::OVERWORLD, container.reference)
                    .is_none()
            );
            assert!(
                view.container_at(
                    Dimension::OVERWORLD,
                    observation.pos,
                    mornlea_domain::ContainerKind::Chest
                )
                .is_none()
            );
            assert!(view.container_refs(key()).is_empty());
            assert!(view.drops(key()).is_empty());
            assert_eq!(
                view.check_drop_batch(&drop_batch()),
                Err(RuleReject::StaleObservation)
            );
            assert_eq!(
                view.drop_rehearsal().try_insert(&drop_batch()),
                Err(RuleReject::StaleObservation)
            );
            let replacement = container.clone();
            let effects = vec![
                RuleEffect::Drops(drop_batch()),
                RuleEffect::DropPatch {
                    before: drop_record.clone(),
                    after: None,
                },
                RuleEffect::Container {
                    before: container.clone(),
                    after: replacement.clone(),
                },
                RuleEffect::WorldContainer {
                    dimension: Dimension::OVERWORLD,
                    before: container.clone(),
                    after: replacement,
                },
                RuleEffect::Blocks(BlockTxn::system(
                    SystemRule::Support,
                    ctx.read().tick(),
                    vec![BlockWrite::try_new(observation, 2).unwrap()],
                )),
            ];
            for effect in effects {
                assert_eq!(
                    ctx.stage(RuleEffect::Compound(vec![
                        RuleEffect::World(
                            WorldState::try_new(mornlea_domain::WorldStateParts {
                                day_phase_offset: 0,
                                world_time_ticks: 999,
                                weather: Weather::Clear,
                                season: mornlea_domain::Season::Spring,
                                season_progress: 0,
                                temperature: 0
                            })
                            .unwrap()
                        ),
                        effect
                    ])),
                    Err(RuleReject::StaleObservation)
                );
                assert!(ctx.world.is_none());
            }
            assert_eq!(ctx.drops.get(&key()).unwrap().records(), &[drop_record]);
            assert_eq!(
                ctx.container_chunks
                    .get(&key())
                    .unwrap()
                    .record(key(), container.reference),
                Some(container.clone())
            );
            ctx.commit_carried();
        }
        a.replace_chunk_wants(BTreeSet::from([key()])).unwrap();
        let ready = a.live_chunk_facts(key()).unwrap();
        assert_eq!(
            (
                ready.phase,
                ready.generation,
                ready.revision,
                ready.persisted_revision,
                ready.needs_rewrite,
                ready.recovered
            ),
            (
                LiveChunkPhase::Ready,
                before.generation,
                before.revision,
                7,
                true,
                true
            )
        );
        let ctx = TickContext::for_tick(&mut a, TickBudget::full());
        assert!(ctx.read().ready_chunk(key()));
        assert_eq!(ctx.read().container(container.reference), Some(container));
        assert_eq!(ctx.read().drops(key()).len(), 1);
    }
    #[test]
    fn sixty_four_failed_attempts_tick_forget_retain_only_global_generation() {
        let mut a = live();
        for n in 1..=64 {
            a.replace_chunk_wants(BTreeSet::from([key()])).unwrap();
            let r = a.reserve_chunk_load(key()).unwrap();
            assert_eq!(r.generation(), n);
            a.bind_chunk_load(r, request(n)).unwrap();
            a.offer_acquired(AcquiredChunkEvent::Load {
                key: key(),
                generation: r.generation(),
                request: request(n),
                result: Err(ServerError::Cancelled),
            })
            .unwrap();
            a.advance_tick(TickBudget::full()).unwrap();
            assert_eq!(
                a.live_chunk_facts(key()).unwrap().phase,
                LiveChunkPhase::Failed
            );
            a.replace_chunk_wants(BTreeSet::new()).unwrap();
            assert_eq!(a.acquisition.ownership_counts(), (0, 0, 0, 0, 0, 0));
        }
        a.replace_chunk_wants(BTreeSet::from([key()])).unwrap();
        assert_eq!(a.reserve_chunk_load(key()).unwrap().generation(), 65);
    }
    #[test]
    fn enable_refuses_sparse_ready_and_queued_legacy_before_changing_mode() {
        for ready in [false, true] {
            let mut a = authority();
            let mut ctx = TickContext::harness(&mut a, TickBudget::full());
            if ready {
                ctx.preload_ready_chunk(prepared(1).into_parts().0);
            } else {
                ctx.preload_block(
                    BlockObservation::try_new(key(), 1, 1, BlockPos::new(0, 0, 0), 2).unwrap(),
                );
            }
            let residents = ctx.resident_snapshot();
            drop(ctx);
            a.commit_residents(residents);
            assert_eq!(
                a.enable_live_chunks(),
                Err(ServerError::InvalidState {
                    phase: ServerPhase::Running
                })
            );
            assert!(!a.acquisition.enabled());
            let ctx = TickContext::for_tick(&mut a, TickBudget::full());
            assert!(
                ctx.read()
                    .observation(Dimension::OVERWORLD, BlockPos::new(0, 0, 0))
                    .is_some()
            );
        }
        let mut a = authority();
        a.admit_chunk(ChunkResult {
            key: key(),
            generation: 1,
            request: request(1),
            result: Ok(chunk()),
        })
        .unwrap();
        assert!(a.enable_live_chunks().is_err());
        assert_eq!(a.drain_chunks(1).len(), 1);
        a.enable_live_chunks().unwrap();
    }
    #[test]
    fn ordinary_tick_selects_companion_before_acquire_and_moves_player_after_installation() {
        use mornlea_domain::{
            Command, CompanionId, HeldActions, LookAngles, MotionStateParts, Movement,
            PlayerControl, PlayerControlParts, SurvivalState, SurvivalStateParts,
        };
        use mornlea_protocol::{LoginStart, admit_login};
        fn uuid(n: u8) -> [u8; 16] {
            let mut b = [0; 16];
            b[0] = n;
            b[6] = 0x40;
            b[8] = 0x80;
            b
        }
        let mut a = live();
        let id = PlayerId::try_from_bytes(uuid(1)).unwrap();
        let start = LoginStart::new(id, "Ada", 8).unwrap();
        let login =
            admit_login(LoginStart::decode_inbound(&start.encode().unwrap()).unwrap()).unwrap();
        let session = a.admit(login, TransportKind::Memory).unwrap();
        let save = PlayerSave {
            player_id: mornlea_storage::PlayerId::from_bytes(uuid(1)),
            revision: 1,
            display_name: "Ada".into(),
            current: PlayerLocation {
                dimension: 0,
                position: [8.65, 0.0, 8.0],
            },
            yaw: 0.0,
            pitch: 0.0,
            safe: None,
            inventory: Default::default(),
            health: 20,
            hunger: 20,
            saturation_milli: 5000,
            exhaustion_milli: 0,
            respawn_present: false,
            respawn_position: [0.0; 3],
            respawn_dimension: 0,
            armor: [Default::default(); 4],
        };
        let companion = CompanionId::try_from_bytes(uuid(2)).unwrap();
        let mut seeded = seed_player(session, &save).unwrap();
        seeded.actor.motion = MotionState::new(MotionStateParts {
            position: FiniteVec3::try_new([8.65, 0.0, 8.0]).unwrap(),
            velocity: FiniteVec3::try_new([0.0; 3]).unwrap(),
            on_ground: true,
        });
        let companion_actor = ActorRecord::try_new(
            ActorKey::Companion(companion),
            ActorLifecycle::Active,
            Dimension::OVERWORLD,
            MotionState::new(MotionStateParts {
                position: FiniteVec3::try_new([6.0, 0.0, 6.0]).unwrap(),
                velocity: FiniteVec3::try_new([0.0; 3]).unwrap(),
                on_ground: true,
            }),
            LookAngles::try_new(0.0, 0.0).unwrap(),
            SurvivalState::try_new(SurvivalStateParts {
                health: 20,
                oxygen: 300,
                hunger: 20,
                saturation_zero: false,
                armor_points: 0,
            })
            .unwrap(),
            ActorBody::Companion(mornlea_storage::CompanionBody {
                id: mornlea_storage::PlayerId::from_bytes(uuid(2)),
                dimension: 0,
                position: [6.0, 0.0, 6.0],
                yaw: 0.0,
                pitch: 0.0,
                inventory: Default::default(),
            }),
        )
        .unwrap();
        {
            let mut ctx = TickContext::for_tick(&mut a, TickBudget::full());
            ctx.stage_login(seeded);
            ctx.stage(RuleEffect::Actor(companion_actor)).unwrap();
            ctx.preload_inventory(ActorKey::Companion(companion), InventoryRecord::empty());
            ctx.commit_carried();
        }
        let control = PlayerControl::new(PlayerControlParts {
            movement: Movement {
                move_x: 1,
                move_z: 0,
                jump: false,
            },
            look: LookAngles::try_new(0.0, 0.0).unwrap(),
            actions: HeldActions {
                primary: false,
                eating: false,
                sprinting: false,
                sneaking: false,
            },
        });
        a.submit(
            session,
            PlayIntent::Sequenced {
                sequence: 1,
                command: Command::PlayerInput(control),
            },
        )
        .unwrap();
        let mut floor = chunk();
        for section in &mut floor.sections[..4] {
            section.single = 2;
        }
        let mut packed = vec![0u64; 1024];
        for y in [0usize, 1] {
            let cell = y * 256 + 8 * 16 + 9;
            packed[cell / 4] |= 2u64 << ((cell % 4) * 15);
        }
        floor.sections[4] = ContainerSnapshot {
            kind: StorageKind::Direct,
            bits: 15,
            single: 0,
            palette: vec![],
            packed,
        };
        queue(&mut a, floor);
        let action = CompanionActionEnvelope::try_new(
            companion,
            a.next_tick(),
            AgentRequestId::try_from_bytes(uuid(3)).unwrap(),
            RunId::try_from_bytes(uuid(4)).unwrap(),
            SnapshotId::try_from_bytes(uuid(5)).unwrap(),
            1,
            1,
            [0; 32],
            CompanionAction::MineHold {
                target: BlockPos::new(9, 0, 8),
            },
        )
        .unwrap();
        a.submit_companion(action).unwrap();
        world::reset_ready_clones();
        world::reset_materializations();
        a.advance_tick(TickBudget::full()).unwrap();
        assert_eq!(
            ACQUIRE_AVAILABILITY.with(std::cell::Cell::get),
            Some((key(), false, true))
        );
        let installed = a
            .residents
            .actors
            .iter()
            .find(|v| v.key == ActorKey::Player(session))
            .unwrap()
            .motion
            .position()
            .get();
        assert_eq!(
            installed[0], 8.7,
            "later physics collides with the installed wall"
        );
        assert!(
            a.residents
                .actors
                .iter()
                .find(|v| v.key == ActorKey::Player(session))
                .unwrap()
                .motion
                .on_ground()
        );
        assert_eq!(world::ready_clones(), 0);
        assert_eq!(world::materializations(), 0);
    }
    #[test]
    fn acquire_shape_and_batch_report_preserve_then_settle_separate_lanes() {
        let mut a = live();
        let k = |x| ChunkKey {
            dimension: Dimension::OVERWORLD,
            pos: ChunkPos::new(x, 0),
        };
        a.replace_chunk_wants((0..16).map(k).collect()).unwrap();
        for x in 0..8 {
            let r = a.reserve_chunk_load(k(x)).unwrap();
            a.bind_chunk_load(r, request(x as u64 + 1)).unwrap();
            a.offer_acquired(AcquiredChunkEvent::Load {
                key: k(x),
                generation: r.generation(),
                request: request(x as u64 + 1),
                result: Ok(None),
            })
            .unwrap();
        }
        a.advance_tick(TickBudget::full()).unwrap();
        for x in 0..8 {
            let r = a.reserve_chunk_generation(k(x)).unwrap();
            a.bind_chunk_generation(r, request(x as u64 + 1)).unwrap();
            a.offer_acquired(AcquiredChunkEvent::Generated {
                key: k(x),
                generation: r.generation(),
                request: request(x as u64 + 1),
                result: Err(ServerError::Cancelled),
            })
            .unwrap();
        }
        for x in 8..16 {
            let r = a.reserve_chunk_load(k(x)).unwrap();
            a.bind_chunk_load(r, request(x as u64 - 7)).unwrap();
            a.offer_acquired(AcquiredChunkEvent::Load {
                key: k(x),
                generation: r.generation(),
                request: request(x as u64 - 7),
                result: Ok(None),
            })
            .unwrap();
        }
        let mut ctx = TickContext::for_tick(&mut a, TickBudget::full());
        let wrong = RuleCall {
            phase: RulePhase::CompanionIntent,
            actor: None,
            command: None,
            internal: None,
        };
        assert_eq!(
            crate::rules::world_acquisition::run(&mut ctx, wrong).unwrap(),
            PhaseReport {
                examined: 0,
                applied: 0,
                carried: 0,
                rejected: 0
            }
        );
        assert_eq!(
            ctx.authority.acquisition.ownership_counts(),
            (16, 8, 8, 16, 8, 8)
        );
        let call = RuleCall {
            phase: RulePhase::Acquire,
            actor: None,
            command: None,
            internal: None,
        };
        let report = crate::rules::world_acquisition::run(&mut ctx, call).unwrap();
        assert_eq!(
            report,
            PhaseReport {
                examined: 16,
                applied: 8,
                carried: 0,
                rejected: 8
            }
        );
        assert_eq!(
            ctx.authority.acquisition.ownership_counts(),
            (16, 0, 0, 0, 0, 0)
        );
    }
}

#[cfg(test)]
mod prepared_current_index_tests {
    use super::*;
    use crate::core::publication::{EnqueueOutcome, PreparedFrame};
    use mornlea_domain::{CommandRejection, Event};
    use mornlea_protocol::{LoginStart, admit_login};

    #[test]
    fn sequential_reservations_keep_one_current_key_and_skip_retained_history() {
        let mut state = AuthorityState::try_new(
            ServerLimits::try_new(1, 4096, 2, 64, 64, 1_048_576).unwrap(),
            7,
        )
        .unwrap();
        let mut history = Vec::new();
        for ordinal in 0u16..256 {
            CURRENT_PUBLICATION_VISITS.with(|visits| visits.set(0));
            CURRENT_DUPLICATE_VISITS.with(|visits| visits.set(0));
            let mut bytes = [0u8; 16];
            bytes[..2].copy_from_slice(&ordinal.to_le_bytes());
            bytes[6] = 0x40;
            bytes[8] = 0x80;
            let player = PlayerId::try_from_bytes(bytes).unwrap();
            let start = LoginStart::new(player, "Ada", 8).unwrap();
            let login =
                admit_login(LoginStart::decode_inbound(&start.encode().unwrap()).unwrap()).unwrap();
            let key = if ordinal % 2 == 0 {
                state.prepare(login, TransportKind::Memory).unwrap()
            } else {
                state.admit(login, TransportKind::Memory).unwrap()
            };
            assert_eq!(
                state.current_sessions.iter().copied().collect::<Vec<_>>(),
                vec![key]
            );
            let packet = ServerPacket::CommandRejected(
                mornlea_protocol::CommandRejected::new(u64::from(ordinal), 1).unwrap(),
            );
            if ordinal % 2 == 0 {
                assert_eq!(
                    state
                        .enqueue_prepared(
                            key,
                            PreparedFrame::encode(&mut ProtocolCodec::new().unwrap(), &packet)
                                .unwrap()
                        )
                        .unwrap(),
                    EnqueueOutcome::Closed
                );
            }
            state
                .publish(TickPublication {
                    tick: 0,
                    events: vec![RoutedEvent::new(
                        EventRecipient::Broadcast,
                        Event::CommandRejected(CommandRejection::new(
                            u64::from(ordinal),
                            RejectReason::InvalidRay,
                        )),
                    )],
                    control: vec![],
                    counters: TickCounters::default(),
                })
                .unwrap();
            assert_eq!(state.sessions[&key].outbox.len(), 1);
            CURRENT_PUBLICATION_VISITS.with(|visits| assert_eq!(visits.get(), 1));
            CURRENT_DUPLICATE_VISITS.with(|visits| assert_eq!(visits.get(), 0));
            for old in &history {
                assert_eq!(state.sessions[old].phase, SessionPhase::Retired);
                assert_eq!(state.sessions[old].outbox.len(), 1);
            }
            state.retire(key, CloseReason::PeerGone).unwrap();
            assert!(state.current_sessions.is_empty());
            assert_eq!(state.occupied, 0);
            history.push(key);
        }
        assert_eq!(state.sessions.len(), 256);
    }
    #[test]
    fn duplicate_player_lookup_uses_only_current_prepared_membership() {
        let mut state = AuthorityState::try_new(
            ServerLimits::try_new(8, 4096, 2, 64, 64, 1_048_576).unwrap(),
            7,
        )
        .unwrap();
        let player =
            PlayerId::try_from_bytes([1, 0, 0, 0, 0, 0, 64, 0, 128, 0, 0, 0, 0, 0, 0, 1]).unwrap();
        let login = || {
            let start = LoginStart::new(player, "Ada", 8).unwrap();
            admit_login(LoginStart::decode_inbound(&start.encode().unwrap()).unwrap()).unwrap()
        };
        for _ in 0..256 {
            let key = state.prepare(login(), TransportKind::Memory).unwrap();
            state.retire(key, CloseReason::PeerGone).unwrap();
        }
        CURRENT_DUPLICATE_VISITS.with(|visits| visits.set(0));
        let key = state.prepare(login(), TransportKind::Memory).unwrap();
        CURRENT_DUPLICATE_VISITS.with(|visits| assert_eq!(visits.get(), 0));
        assert_eq!(
            state.prepare(login(), TransportKind::Memory),
            Err(ServerError::InvalidInput { field: "player_id" })
        );
        CURRENT_DUPLICATE_VISITS.with(|visits| assert_eq!(visits.get(), 1));
        assert_eq!(
            state.current_sessions.iter().copied().collect::<Vec<_>>(),
            vec![key]
        );
        assert_eq!(state.sessions.len(), 257);
    }
}
