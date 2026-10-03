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
use super::deferred_commands::DeferredCommands;
use super::drop_store::{self, DropState};
use super::login_seed::{SeededPlayer, seed_player};
use super::publication::{EnqueueOutcome, PreparedFrame, PreparedPublicationPort};
use super::source_player_restore::SourcePlayerBook;
use super::world::ReadyChunk;
use crate::rules::farmland::FarmlandSchedule;
use crate::rules::fluids::FluidSchedule;

const COMPANION_INBOX: usize = 4;

static SETTLED_PRE_STEP: BTreeMap<ActorKey, MotionState> = BTreeMap::new();

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
    /// A successful stored load, separate from the canonical body for Missing.
    loaded_current: bool,
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

/// Accepted residency transfers and the first refusal, retaining prefix progress.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ChunkRetirementReport {
    pub retired: usize,
    pub first_error: Option<ServerError>,
}

/// Private world, sessions, queues, tick, and publication owner.
pub struct AuthorityState {
    limits: ServerLimits,
    phase: ServerPhase,
    next_session: u64,
    ids_exhausted: bool,
    next_tick: u64,
    /// First hard execution failure permanently fences new reduction and save capture.
    tick_failure: Option<ServerError>,
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
    source_player_radius: Option<u8>,
    source_players: SourcePlayerBook,
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
            tick_failure: None,
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
            source_player_radius: None,
            source_players: SourcePlayerBook::default(),
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

    /// Borrows healthy committed owners without copying residents or pending ingress.
    /// The tick names the next executable endpoint; absolute time comes from climate.
    /// A retained hard failure fences new reads before terminal phase validation.
    ///
    /// The loan must end before an exclusive tick can execute:
    /// ```compile_fail,E0502
    /// use mornlea_server::contracts::{ServerLimits, TickBudget};
    /// use mornlea_server::state::AuthorityState;
    /// let limits = ServerLimits::try_new(8, 4096, 512, 64, 64, 1_048_576).unwrap();
    /// let mut authority = AuthorityState::try_new(limits, 42).unwrap();
    /// let view = authority.settled_read().unwrap();
    /// authority.advance_tick(TickBudget::full()).unwrap();
    /// assert_eq!(view.tick(), 0);
    /// ```
    pub fn settled_read(&self) -> Result<AuthorityReadView<'_>, ServerError> {
        if let Some(error) = self.tick_failure {
            return Err(error);
        }
        if self.phase == ServerPhase::Closed {
            return Err(ServerError::InvalidState { phase: self.phase });
        }
        Ok(AuthorityReadView {
            acquisition: self.acquisition.enabled().then_some(&self.acquisition),
            tick: self.next_tick,
            world: None,
            commands: &[],
            companions: &[],
            interactions: &[],
            inventories: &self.residents.inventories,
            blocks: &self.residents.blocks,
            ready: &self.residents.ready,
            actors: &self.residents.actors,
            runtimes: &self.residents.runtimes,
            mining: &self.residents.mining,
            pre_step: &SETTLED_PRE_STEP,
            environment: self.residents.environment.as_ref(),
            containers: &self.residents.containers,
            container_chunks: &self.residents.container_chunks,
            viewers: &self.views,
            drops: &self.residents.drops,
            projectiles: &self.residents.projectiles,
            damage_intents: &[],
            metadata: &self.metadata,
            observation_trace: None,
        })
    }

    /// Login seeds for Active sessions with a save body and no player actor,
    /// in ascending session order. Saves are validated at install, so a
    /// mapping refusal only means in-memory corruption; skipping leaves the
    /// session for a later tick instead of failing the tick on login staging.
    pub(crate) fn login_seeds(&self) -> Vec<SeededPlayer> {
        if self.source_player_radius.is_some() {
            return Vec::new();
        }
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

    pub(crate) fn tick_failure(&self) -> Option<ServerError> {
        self.tick_failure
    }

    /// Partial mutations retain ownership, but this authority cannot resume or save them.
    pub(crate) fn fail_tick(&mut self, error: ServerError) -> ServerError {
        let retained = *self.tick_failure.get_or_insert(error);
        if self.phase == ServerPhase::Running {
            self.phase = ServerPhase::Closing;
        }
        retained
    }

    pub fn advance_tick(&mut self, work: TickBudget) -> Result<TickPublication, ServerError> {
        if let Some(error) = self.tick_failure {
            return Err(error);
        }
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
        let publication = super::step::reduce_tick(self, work)?;
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

    /// Enables explicit source registration before any player owner exists.
    pub fn enable_source_player_restoration(&mut self, radius: u8) -> Result<(), ServerError> {
        if let Some(error) = self.tick_failure {
            return Err(error);
        }
        if self.phase != ServerPhase::Running {
            return Err(ServerError::InvalidState { phase: self.phase });
        }
        if let Some(enabled) = self.source_player_radius {
            return if enabled == radius {
                Ok(())
            } else {
                Err(ServerError::InvalidInput {
                    field: "source_player_radius",
                })
            };
        }
        if !self.sessions.is_empty()
            || !self.residents.actors.is_empty()
            || !self.residents.inventories.is_empty()
            || !self.residents.runtimes.is_empty()
            || !self.residents.mining.is_empty()
        {
            return Err(ServerError::InvalidInput {
                field: "source_player_restoration",
            });
        }
        Dimension::new(u8::try_from(self.metadata.spawn_dimension).map_err(|_| {
            ServerError::Internal {
                invariant: "source player spawn metadata",
            }
        })?)
        .map_err(|_| ServerError::Internal {
            invariant: "source player spawn metadata",
        })?;
        super::actor_placement::spawn_columns(
            mornlea_domain::ChunkPos::new(
                self.metadata.spawn_anchor.x,
                self.metadata.spawn_anchor.z,
            ),
            radius,
        )?;
        self.source_player_radius = Some(radius);
        Ok(())
    }

    pub(crate) fn source_players_mut(&mut self) -> &mut SourcePlayerBook {
        &mut self.source_players
    }

    pub(crate) fn prune_source_players(&mut self) {
        self.source_players.entries.retain(|key, _| {
            self.sessions
                .get(key)
                .is_some_and(|record| record.phase == SessionPhase::Active)
        });
    }

    pub fn allocate(
        &mut self,
        login: AdmittedLogin,
        kind: TransportKind,
    ) -> Result<SessionKey, ServerError> {
        let _ = kind;
        if self.source_player_radius.is_some() {
            return Err(ServerError::InvalidInput {
                field: "background_login_required",
            });
        }
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
        let loaded_current = loaded.is_some();
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
        record.loaded_current = loaded_current;
        Ok(())
    }

    pub fn activate(&mut self, session: SessionKey) -> Result<(), ServerError> {
        let record = self
            .sessions
            .get(&session)
            .ok_or(ServerError::StaleSession { session })?;
        if record.phase != SessionPhase::Prepared || record.body.is_none() {
            return Err(ServerError::InvalidState { phase: self.phase });
        }
        if let Some(radius) = self.source_player_radius {
            if let Some(error) = self.tick_failure {
                return Err(error);
            }
            if self.phase != ServerPhase::Running {
                return Err(ServerError::InvalidState { phase: self.phase });
            }
            let dimension = mornlea_domain::Dimension::new(
                u8::try_from(self.metadata.spawn_dimension).map_err(|_| ServerError::Internal {
                    invariant: "source player spawn metadata",
                })?,
            )
            .map_err(|_| ServerError::Internal {
                invariant: "source player spawn metadata",
            })?;
            let anchor = mornlea_domain::ChunkPos::new(
                self.metadata.spawn_anchor.x,
                self.metadata.spawn_anchor.z,
            );
            let prepared = super::source_player_restore::prepare(
                session,
                record.body.as_ref().unwrap(),
                record.loaded_current,
                dimension,
                anchor,
                radius,
                &self.settled_read()?,
            )?;
            let key = ActorKey::Player(session);
            if self.residents.actors.iter().any(|actor| actor.key == key)
                || self.residents.inventories.contains_key(&key)
                || self.residents.runtimes.contains_key(&key)
                || self.source_players.entries.contains_key(&session)
                || self.source_players.entries.len() >= 8
            {
                return Err(ServerError::Internal {
                    invariant: "source player registration",
                });
            }
            // All checked construction precedes the owner transfer and Active handoff.
            let slot = self.residents.actors.len();
            self.residents.actors.push(prepared.seeded.actor);
            self.residents
                .inventories
                .insert(key, prepared.seeded.inventory);
            self.residents.runtimes.insert(key, prepared.runtime);
            self.residents.player_slots.insert(session, slot);
            self.source_players.entries.insert(session, prepared.entry);
        }
        self.sessions.get_mut(&session).unwrap().phase = SessionPhase::Active;
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
        self.source_players.entries.remove(&key);
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
        if let Some(error) = self.tick_failure {
            return Err(error);
        }
        if !self.acquisition.enabled()
            || self.phase == ServerPhase::Closed
            || (running && self.phase != ServerPhase::Running)
        {
            Err(ServerError::InvalidState { phase: self.phase })
        } else {
            Ok(())
        }
    }
    /// Transfers clean unwanted bodies; admission is separate from CPU disposal.
    pub fn retire_unwanted_chunks(
        &mut self,
        port: &mut dyn super::chunk_retirement::ChunkRetirementPort,
        max_chunks: usize,
    ) -> Result<ChunkRetirementReport, ServerError> {
        use super::chunk_retirement::{MAX_CHUNK_RETIREMENTS, RetiredChunk};
        self.require_live_chunks(false)?;
        if !self.residents.dirty_chunks.is_empty() {
            return Err(ServerError::Internal {
                invariant: "uncommitted chunk retirement",
            });
        }
        if !self.residents.containers.is_empty() {
            return Err(ServerError::Internal {
                invariant: "managed sparse containers",
            });
        }
        let mut report = ChunkRetirementReport::default();
        for _ in 0..max_chunks.min(MAX_CHUNK_RETIREMENTS) {
            if port.occupied() >= MAX_CHUNK_RETIREMENTS {
                break;
            }
            let Some((key, generation)) = self.acquisition.next_retirement() else {
                break;
            };
            if !self
                .residents
                .ready
                .get(&key)
                .is_some_and(|r| r.key == key && r.generation == generation)
                || !self.residents.drops.contains_key(&key)
                || !self.residents.container_chunks.contains_key(&key)
            {
                report.first_error = Some(ServerError::Internal {
                    invariant: "managed chunk ownership",
                });
                break;
            }
            // Keep scalar eligibility intact until admission accepts all owners.
            let owner = RetiredChunk::new(
                self.residents
                    .ready
                    .remove(&key)
                    .expect("validated Ready owner"),
                self.residents
                    .drops
                    .remove(&key)
                    .expect("validated drop owner"),
                self.residents
                    .container_chunks
                    .remove(&key)
                    .expect("validated container owner"),
                self.residents.blocks.take_chunk(key),
            );
            match port.submit(owner) {
                Ok(()) => {
                    self.acquisition.retired(key, generation);
                    report.retired += 1;
                }
                Err(rejected) => {
                    // The port returns the same complete body, including tree nodes.
                    let (ready, drops, containers, observations) = rejected.owner.into_parts();
                    self.residents.ready.insert(key, ready);
                    self.residents.drops.insert(key, drops);
                    self.residents.container_chunks.insert(key, containers);
                    if let Some(owner) = observations {
                        self.residents.blocks.restore_chunk(key, owner);
                    }
                    report.first_error = Some(rejected.error);
                    break;
                }
            }
        }
        Ok(report)
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
        if let Some(error) = self.tick_failure {
            return Err(error);
        }
        if self.phase == ServerPhase::Closed || self.final_consumed {
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
        if self.tick_failure.is_some() {
            return None;
        }
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
        if self.tick_failure.is_some() || self.phase == ServerPhase::Closed {
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
        if let Some(error) = self.tick_failure {
            return Err(error);
        }
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
        report.retryable = self.tick_failure.is_none() && super::shutdown::is_transient(&error);
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
                loaded_current: false,
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
            .or_else(|| self.environment.map(|environment| environment.world_time))
            .unwrap_or(self.metadata.world_time_ticks)
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
    /// Managed collision reads normalize out-of-height air without a mutation address.
    /// In-height reads retain current observations and their trace refusal policy.
    pub(crate) fn live_collision_block(
        &self,
        dimension: Dimension,
        pos: mornlea_domain::BlockPos,
    ) -> Option<u16> {
        if self.acquisition.is_some() && !(-64..320).contains(&pos.y()) {
            return Some(0);
        }
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
    deferred: DeferredCommands,
    charges: Vec<(ActorKey, ActionKind)>,
    suppressed_mining: BTreeSet<ActorKey>,
    /// Pre-motion actor poses captured at construction. Only source recovery
    /// may rebase an existing pose after its positional lift, before motion.
    /// Jump takeoffs and swimming displacement consume this step-start owner
    /// instead of re-deriving physics. Compounds never touch it; no rollback entry.
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
/// provider or late source-player consumer to settle.
///
/// Mining and till receipts fire exactly on their successful completion forks
/// (refused or interrupted work notes nothing); melee receipts fire exactly on
/// a successful hit in direct fixtures. Source players settle late receipts after
/// Interaction and Mining; public direct fixtures retain next post-physics
/// settlement. The existing direct melee debit remains separate.
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
            deferred: DeferredCommands::default(),
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

    /// Settles only late action scalars through the indexed source player owners.
    /// All qualification and arithmetic precede writes; foreign receipts retain order.
    pub(crate) fn settle_source_player_action_costs(
        &mut self,
        session: SessionKey,
    ) -> Result<PhaseReport, ServerError> {
        let mut report = PhaseReport {
            examined: 0,
            applied: 0,
            carried: 0,
            rejected: 0,
        };
        if let Some(error) = self.authority.tick_failure {
            return Err(error);
        }
        if self.authority.phase == ServerPhase::Closed {
            return Err(ServerError::InvalidState {
                phase: ServerPhase::Closed,
            });
        }
        if self.authority.source_player_radius.is_none()
            || !self.source_player_session_active(session)
        {
            return Ok(report);
        }
        let invalid = ServerError::InvalidInput {
            field: "source_player_action_costs",
        };
        let slot = *self.player_slots.get(&session).ok_or(invalid)?;
        let actor = self.actors.get(slot).ok_or(invalid)?;
        let key = ActorKey::Player(session);
        if actor.key != key {
            return Err(invalid);
        }
        if actor.lifecycle != ActorLifecycle::Active {
            return Ok(report);
        }
        // Reject a corrupt lane before scanning, including foreign-only work.
        if self.charges.len() > 4096 {
            return Err(ServerError::Capacity {
                resource: Resource::Commands,
                limit: 4096,
                observed: self.charges.len(),
            });
        }
        if !self.charges.iter().any(|(owner, _)| *owner == key) {
            return Ok(report);
        }
        if !matches!(actor.body, ActorBody::Player(_)) {
            return Err(invalid);
        }
        let runtime = self.runtimes.get(&key).ok_or(invalid)?;
        if runtime.key != key || !matches!(runtime.aux, ActorAux::Player { .. }) {
            return Err(invalid);
        }
        if runtime.reset {
            return Ok(report);
        }
        let threshold = self
            .environment
            .as_ref()
            .ok_or(ServerError::Internal {
                invariant: "tick environment snapshot",
            })?
            .tunables
            .exhaustion_threshold_milli();
        // Match survival's compatibility narrowing before the shared wide loop.
        let mut hunger = actor.survival.hunger();
        let mut saturation = runtime.saturation_milli.min(u32::from(u16::MAX)) as u16;
        let mut exhaustion = runtime.exhaustion_milli.min(u32::from(u16::MAX)) as u16;
        for (_, kind) in self.charges.iter().filter(|(owner, _)| *owner == key) {
            (hunger, saturation, exhaustion) = crate::rules::player_survival::exhausted_state(
                hunger,
                saturation,
                exhaustion,
                crate::rules::player_survival::charge_milli(*kind),
                threshold,
            );
            report.examined += 1;
        }
        let survival = mornlea_domain::SurvivalState::try_new(mornlea_domain::SurvivalStateParts {
            health: actor.survival.health(),
            hunger,
            oxygen: actor.survival.oxygen(),
            saturation_zero: saturation == 0,
            armor_points: actor.survival.armor_points(),
        })
        .map_err(|_| ServerError::Internal {
            invariant: "source player action cost survival",
        })?;
        // No fallible work remains: the checked indexed pair stays exclusively borrowed.
        let actor = &mut self.actors[slot];
        let ActorBody::Player(body) = &mut actor.body else {
            unreachable!("checked player body")
        };
        body.hunger = hunger;
        body.saturation_milli = saturation;
        body.exhaustion_milli = exhaustion;
        actor.survival = survival;
        let runtime = self.runtimes.get_mut(&key).expect("checked player runtime");
        runtime.saturation_milli = u32::from(saturation);
        runtime.exhaustion_milli = u32::from(exhaustion);
        self.charges.retain(|(owner, _)| *owner != key);
        report.applied = report.examined;
        Ok(report)
    }

    /// Recovers one indexed Active source player before native motion.
    /// Geometry retains bounded read-trace bookkeeping; restart belongs to the book owner.
    pub(crate) fn recover_source_player(
        &mut self,
        session: SessionKey,
        restore: &super::pending_restore::PendingRestore,
    ) -> Result<Option<Dimension>, ServerError> {
        if let Some(error) = &self.authority.tick_failure {
            return Err(*error);
        }
        if self.authority.phase == ServerPhase::Closed {
            return Err(ServerError::InvalidState {
                phase: ServerPhase::Closed,
            });
        }
        let invalid = ServerError::InvalidInput {
            field: "source_player_reset",
        };
        if self.authority.source_player_radius.is_none()
            || !self.source_player_session_active(session)
        {
            return Err(invalid);
        }
        let slot = *self.player_slots.get(&session).ok_or(invalid)?;
        let actor = self.actors.get(slot).ok_or(invalid)?;
        let key = ActorKey::Player(session);
        if actor.key != key {
            return Err(invalid);
        }
        // Earlier legacy death may have settled this originally Active roster entry.
        if actor.lifecycle != ActorLifecycle::Active {
            return Ok(None);
        }
        if !matches!(actor.body, ActorBody::Player(_)) {
            return Err(invalid);
        }
        let runtime = self.runtimes.get(&key).ok_or(invalid)?;
        if runtime.key != key || !matches!(runtime.aux, ActorAux::Player { .. }) {
            return Err(invalid);
        }
        restore.player_reset_anchor()?;
        let dimension = actor.dimension;
        let original = actor.motion;
        let position = original.position().get();
        if position[1] < -80.0 {
            self.begin_source_player_reset(session, restore)?;
            return Ok(Some(dimension));
        }
        let space = super::actor_placement::body_space(&self.read(), dimension, position)?;
        if !space.ready {
            self.begin_source_player_reset(session, restore)?;
            return Ok(Some(dimension));
        }
        if space.free {
            return Ok(None);
        }
        for step in 1..=16 {
            // Every lift starts at the original pose, preserving source float arithmetic.
            let mut candidate = position;
            candidate[1] = position[1] + (step as f32) / 16.0;
            let space = super::actor_placement::body_space(&self.read(), dimension, candidate)?;
            if !space.ready {
                self.begin_source_player_reset(session, restore)?;
                return Ok(Some(dimension));
            }
            if space.free {
                let position = mornlea_domain::FiniteVec3::try_new(candidate).map_err(|_| {
                    ServerError::Internal {
                        invariant: "source player recovery",
                    }
                })?;
                let pre_step = self.pre_step.get_mut(&key).ok_or(ServerError::Internal {
                    invariant: "source player recovery",
                })?;
                let lifted = MotionState::new(mornlea_domain::MotionStateParts {
                    position,
                    velocity: original.velocity(),
                    on_ground: original.on_ground(),
                });
                // Rebase the existing owner so swimming never charges recovery displacement.
                self.actors[slot].motion = lifted;
                *pre_step = lifted;
                return Ok(None);
            }
        }
        self.begin_source_player_reset(session, restore)?;
        Ok(Some(dimension))
    }

    /// Borrows the original landing-edge certificate before later death can replace the pose.
    /// Qualification and geometry read no world owners and mutate no context state.
    pub(crate) fn capture_source_player_trample(
        &self,
        session: SessionKey,
    ) -> Result<Option<(Dimension, [mornlea_domain::BlockPos; 4], usize)>, ServerError> {
        if let Some(error) = self.authority.tick_failure {
            return Err(error);
        }
        if self.authority.phase == ServerPhase::Closed {
            return Err(ServerError::InvalidState {
                phase: ServerPhase::Closed,
            });
        }
        if self.authority.source_player_radius.is_none()
            || !self.source_player_session_active(session)
        {
            return Ok(None);
        }
        let invalid = ServerError::InvalidInput {
            field: "source_player_trample",
        };
        let slot = *self.player_slots.get(&session).ok_or(invalid)?;
        let actor = self.actors.get(slot).ok_or(invalid)?;
        let key = ActorKey::Player(session);
        if actor.key != key {
            return Err(invalid);
        }
        if actor.lifecycle != ActorLifecycle::Active || !actor.motion.on_ground() {
            return Ok(None);
        }
        if !matches!(actor.body, ActorBody::Player(_)) {
            return Err(invalid);
        }
        let runtime = self.runtimes.get(&key).ok_or(invalid)?;
        if runtime.key != key || !matches!(runtime.aux, ActorAux::Player { .. }) {
            return Err(invalid);
        }
        if runtime.reset {
            return Ok(None);
        }
        let Some(pre_step) = self.pre_step.get(&key) else {
            return Ok(None);
        };
        if pre_step.on_ground() {
            return Ok(None);
        }
        let dimension = actor.dimension;
        let position = actor.motion.position().get();
        let (cells, len) = super::actor_placement::trample_cells(position)?;
        Ok((len != 0).then_some((dimension, cells, len)))
    }

    /// Copies grounded source motion before Safe and death without reading world owners.
    // Keep the private copied-motion tuple at this single producer boundary.
    #[allow(clippy::type_complexity)]
    pub(crate) fn capture_source_player_snow(
        &self,
        session: SessionKey,
    ) -> Result<Option<(Dimension, [f32; 3], [f32; 3])>, ServerError> {
        if let Some(error) = self.authority.tick_failure {
            return Err(error);
        }
        if self.authority.phase == ServerPhase::Closed {
            return Err(ServerError::InvalidState {
                phase: ServerPhase::Closed,
            });
        }
        if self.authority.source_player_radius.is_none()
            || !self.source_player_session_active(session)
        {
            return Ok(None);
        }
        let invalid = ServerError::InvalidInput {
            field: "source_player_snow",
        };
        let slot = *self.player_slots.get(&session).ok_or(invalid)?;
        let actor = self.actors.get(slot).ok_or(invalid)?;
        let key = ActorKey::Player(session);
        if actor.key != key {
            return Err(invalid);
        }
        if actor.lifecycle != ActorLifecycle::Active || !actor.motion.on_ground() {
            return Ok(None);
        }
        if !matches!(actor.body, ActorBody::Player(_)) {
            return Err(invalid);
        }
        let runtime = self.runtimes.get(&key).ok_or(invalid)?;
        if runtime.key != key || !matches!(runtime.aux, ActorAux::Player { .. }) {
            return Err(invalid);
        }
        if runtime.reset {
            return Ok(None);
        }
        let Some(pre_step) = self.pre_step.get(&key) else {
            return Ok(None);
        };
        let dimension = actor.dimension;
        let position = actor.motion.position().get();
        Ok(Some((dimension, pre_step.position().get(), position)))
    }

    /// Updates only the indexed player's heap-free Safe value after ordinary native motion.
    /// Refusal preserves every owner, and an earlier player's accepted write survives abandonment.
    pub(crate) fn checkpoint_source_player_safe(
        &mut self,
        session: SessionKey,
    ) -> Result<(), ServerError> {
        if let Some(error) = self.authority.tick_failure {
            return Err(error);
        }
        if self.authority.phase == ServerPhase::Closed {
            return Err(ServerError::InvalidState {
                phase: ServerPhase::Closed,
            });
        }
        if self.authority.source_player_radius.is_none()
            || !self.source_player_session_active(session)
        {
            return Ok(());
        }
        let invalid = ServerError::InvalidInput {
            field: "source_player_safe",
        };
        let slot = *self.player_slots.get(&session).ok_or(invalid)?;
        let actor = self.actors.get(slot).ok_or(invalid)?;
        let key = ActorKey::Player(session);
        if actor.key != key {
            return Err(invalid);
        }
        if actor.lifecycle != ActorLifecycle::Active || !actor.motion.on_ground() {
            return Ok(());
        }
        if !matches!(actor.body, ActorBody::Player(_)) {
            return Err(invalid);
        }
        let runtime = self.runtimes.get(&key).ok_or(invalid)?;
        if runtime.key != key || !matches!(runtime.aux, ActorAux::Player { .. }) {
            return Err(invalid);
        }
        if runtime.reset {
            return Ok(());
        }
        let dimension = actor.dimension;
        let position = actor.motion.position().get();
        if !super::actor_placement::safe_location(&self.read(), dimension, position)? {
            return Ok(());
        }
        let ActorBody::Player(body) = &mut self.actors[slot].body else {
            return Err(invalid);
        };
        body.safe = Some(mornlea_storage::PlayerLocation {
            dimension: i32::from(dimension.get()),
            position,
        });
        Ok(())
    }

    /// Identifies source sessions whose death routing belongs to the serial consumer.
    pub(crate) fn source_player_death_deferred(&self, session: SessionKey) -> bool {
        self.authority.source_player_radius.is_some() && self.source_player_session_active(session)
    }

    /// Prepares one indexed death and returns fixed data for the caller's retained scan.
    /// The Ready guard bounds rehearsal; staging precedes fixed mapping and own cleanup.
    pub(crate) fn settle_source_player_death(
        &mut self,
        session: SessionKey,
        restore: &super::pending_restore::PendingRestore,
    ) -> Result<Option<super::source_player_death::SourceDeathReset>, ServerError> {
        if let Some(error) = self.authority.tick_failure {
            return Err(error);
        }
        if self.authority.phase == ServerPhase::Closed {
            return Err(ServerError::InvalidState {
                phase: ServerPhase::Closed,
            });
        }
        let invalid = ServerError::InvalidInput {
            field: "source_player_reset",
        };
        if self.authority.source_player_radius.is_none()
            || !self.source_player_session_active(session)
        {
            return Err(invalid);
        }
        let slot = *self.player_slots.get(&session).ok_or(invalid)?;
        let actor = self.actors.get(slot).ok_or(invalid)?;
        let key = ActorKey::Player(session);
        if actor.key != key {
            return Err(invalid);
        }
        if actor.lifecycle != ActorLifecycle::Active || actor.survival.health() > 0 {
            return Ok(None);
        }
        if !matches!(actor.body, ActorBody::Player(_)) {
            return Err(invalid);
        }
        let runtime = self.runtimes.get(&key).ok_or(invalid)?;
        if runtime.key != key {
            return Err(invalid);
        }
        let ActorAux::Player { respawn, .. } = runtime.aux else {
            return Err(invalid);
        };
        restore.player_reset_anchor()?;
        let refusal = ServerError::Internal {
            invariant: "source player death",
        };
        let before = *self.inventories.get(&key).ok_or(refusal)?;
        let pickup_delay = self
            .environment
            .as_ref()
            .ok_or(refusal)?
            .tunables
            .player_drop_pickup_delay_ticks();
        let dimension = actor.dimension;
        let position = actor.motion.position().get();
        if self.ready.len() > super::acquisition::MAX_MANAGED {
            return Err(ServerError::Capacity {
                resource: Resource::ResidentChunks,
                limit: super::acquisition::MAX_MANAGED,
                observed: self.ready.len(),
            });
        }
        let view = self.read();
        let prepared = super::source_player_death::prepare_inventory(
            &view,
            key,
            dimension,
            position,
            pickup_delay,
            before,
        )?;
        let bed = super::source_player_death::prepare_bed(&view, dimension, respawn)?;
        let survival = mornlea_domain::SurvivalState::try_new(mornlea_domain::SurvivalStateParts {
            health: 20,
            oxygen: 300,
            hunger: 20,
            saturation_zero: false,
            armor_points: crate::rules::inventory::armor_points(&prepared.after.armor),
        })
        .map_err(|_| refusal)?;
        let staging = ServerError::Internal {
            invariant: "source player death staging",
        };
        let patch = InventoryPatch::try_new(key, before, prepared.after).map_err(|_| staging)?;
        let mut effects = Vec::with_capacity(1 + prepared.drops.len());
        effects.push(RuleEffect::Inventory(patch));
        effects.extend(prepared.drops);
        self.stage(RuleEffect::Compound(effects))
            .map_err(|_| staging)?;

        self.actors[slot].survival = survival;
        let runtime = self.runtimes.get_mut(&key).ok_or(refusal)?;
        runtime.saturation_milli = 5000;
        runtime.exhaustion_milli = 0;
        runtime.since_damage_ticks = 0;
        runtime.starvation_ticks = 0;
        if let ActorAux::Player { respawn, .. } = &mut runtime.aux {
            *respawn = bed.respawn;
        }
        // Keep the other actors' receipt order and the existing bounded allocation.
        self.charges.retain(|(charged, _)| *charged != key);
        if let Err(error) = self.begin_source_player_reset(session, restore) {
            return Err(self.authority.fail_tick(error));
        }
        Ok(Some(super::source_player_death::SourceDeathReset {
            dimension,
            candidate: bed.candidate,
        }))
    }

    /// Resets a live source player in place; the caller selects its completed scan.
    /// Refusals precede mutation, and earned receipts stay with their settlement owner.
    /// Keyed lookups and removals are logarithmic; resident maps may retain history.
    pub(crate) fn begin_source_player_reset(
        &mut self,
        session: SessionKey,
        restore: &super::pending_restore::PendingRestore,
    ) -> Result<(), ServerError> {
        if let Some(error) = &self.authority.tick_failure {
            return Err(*error);
        }
        if self.authority.phase == ServerPhase::Closed {
            return Err(ServerError::InvalidState {
                phase: ServerPhase::Closed,
            });
        }
        let invalid = ServerError::InvalidInput {
            field: "source_player_reset",
        };
        if self.authority.source_player_radius.is_none()
            || !self.source_player_session_active(session)
        {
            return Err(invalid);
        }
        let slot = self.player_slots.get(&session).ok_or(invalid)?;
        let actor = self.actors.get_mut(*slot).ok_or(invalid)?;
        let key = ActorKey::Player(session);
        if actor.key != key {
            return Err(invalid);
        }
        let runtime = self.runtimes.get_mut(&key).ok_or(invalid)?;
        super::source_player_reset::begin_reset(actor, runtime, restore)?;

        // Cleanup cannot refuse after mapping, and leaves durable sleep/view owners intact.
        self.mining.remove(&key);
        self.sleeping.remove(&session);
        self.suppressed_mining.remove(&key);
        Ok(())
    }

    pub(crate) fn source_player_session_active(&self, session: SessionKey) -> bool {
        self.authority
            .sessions
            .get(&session)
            .is_some_and(|record| record.phase == SessionPhase::Active)
    }

    /// Pending source actors admit only the source's unconditional exceptions.
    pub(crate) fn source_player_command_ready(
        &self,
        envelope: &CommandEnvelope,
    ) -> Result<bool, ServerError> {
        if self.authority.source_player_radius.is_none() {
            return Ok(true);
        }
        if matches!(
            envelope.command(),
            mornlea_domain::Command::CloseContainer
                | mornlea_domain::Command::Resync(_)
                | mornlea_domain::Command::MoveContainer(_)
        ) {
            return Ok(true);
        }
        let session = SessionKey::from_raw(envelope.session()).ok_or(ServerError::Internal {
            invariant: "source player registration",
        })?;
        let actor = self
            .read()
            .actor(ActorKey::Player(session))
            .ok_or(ServerError::Internal {
                invariant: "source player registration",
            })?;
        Ok(actor.lifecycle == ActorLifecycle::Active)
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
        self.deferred.defer(command, phase)
    }

    pub fn deferred(&self, phase: RulePhase) -> Vec<CommandEnvelope> {
        self.deferred.for_phase(phase)
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
    /// survival pass or the late source-player scalar consumer. Writers own the firing rule (see
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
                    loaded_current: false,
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
    fn retirement_authority(count: i32, rewrite: bool) -> AuthorityState {
        retirement_authority_with_body(count, rewrite, chunk())
    }
    fn retirement_authority_with_body(
        count: i32,
        rewrite: bool,
        template: Chunk,
    ) -> AuthorityState {
        let mut a = live();
        a.replace_chunk_wants((0..count).map(save_key).collect())
            .unwrap();
        for x in 0..count {
            let k = save_key(x);
            let r = a.reserve_chunk_load(k).unwrap();
            let id = request(x as u64 + 1);
            a.bind_chunk_load(r, id).unwrap();
            let mut body = template.clone();
            body.drops[31].generation = 17;
            body.furnaces[31].generation = 20;
            body.chests[15].generation = 21;
            let prepared = PreparedChunk::try_new(
                k,
                r.generation(),
                RecoveredChunk {
                    chunk: body,
                    revision: 5,
                    persisted_revision: 5,
                    needs_rewrite: rewrite,
                    recovered: false,
                },
            )
            .unwrap();
            a.offer_acquired(AcquiredChunkEvent::Load {
                key: k,
                generation: r.generation(),
                request: id,
                result: Ok(Some(prepared)),
            })
            .unwrap();
            if x % 8 == 7 || x == count - 1 {
                let mut ctx = TickContext::for_tick(&mut a, TickBudget::full());
                ctx.apply_live_acquisition();
                ctx.commit_carried();
            }
        }
        a
    }
    // These prepared-event and scripted-port cases qualify authority ownership,
    // independently of the actual disk and CPU integration topic.
    struct RetirementDouble {
        held: Vec<super::super::chunk_retirement::RetiredChunk>,
        refusal_at: Option<usize>,
        occupied: usize,
        calls: usize,
    }
    impl RetirementDouble {
        fn accepting() -> Self {
            Self {
                held: vec![],
                refusal_at: None,
                occupied: 0,
                calls: 0,
            }
        }
    }
    impl super::super::chunk_retirement::ChunkRetirementPort for RetirementDouble {
        fn submit(
            &mut self,
            owner: super::super::chunk_retirement::RetiredChunk,
        ) -> Result<(), super::super::chunk_retirement::RejectedRetiredChunk> {
            self.calls += 1;
            if self.refusal_at == Some(self.calls) {
                return Err(super::super::chunk_retirement::RejectedRetiredChunk {
                    error: ServerError::Disconnected,
                    owner,
                });
            }
            self.held.push(owner);
            Ok(())
        }
        fn collect(
            &mut self,
            _: usize,
        ) -> Result<Vec<super::super::chunk_retirement::RetiredChunkId>, ServerError> {
            Ok(vec![])
        }
        fn occupied(&self) -> usize {
            self.occupied + self.held.len()
        }
        fn close(&mut self, _: Deadline) -> Result<(), ServerError> {
            Ok(())
        }
    }
    #[test]
    fn retirement_private_rewant_prefix_limits_and_idle_work() {
        let mut a = retirement_authority(1000, false);
        let mut port = RetirementDouble::accepting();
        a.acquisition.reset_retirement_work();
        assert_eq!(
            a.retire_unwanted_chunks(&mut port, usize::MAX)
                .unwrap()
                .retired,
            0
        );
        assert_eq!(a.acquisition.retirement_work(), (0, 0));
        let generation = a.live_chunk_facts(save_key(0)).unwrap().generation;
        a.replace_chunk_wants((1..1000).map(save_key).collect())
            .unwrap();
        a.replace_chunk_wants((0..1000).map(save_key).collect())
            .unwrap();
        assert_eq!(
            a.live_chunk_facts(save_key(0)).unwrap().generation,
            generation
        );
        assert_eq!(a.retire_unwanted_chunks(&mut port, 1).unwrap().retired, 0);
        a.replace_chunk_wants((10..1000).map(save_key).collect())
            .unwrap();
        assert_eq!(
            a.retire_unwanted_chunks(&mut port, 0).unwrap(),
            ChunkRetirementReport::default()
        );
        assert_eq!(port.calls, 0);
        port.occupied = 8;
        assert_eq!(a.retire_unwanted_chunks(&mut port, 1).unwrap().retired, 0);
        assert_eq!(port.calls, 0);
        port.occupied = 0;
        a.acquisition.reset_retirement_work();
        assert_eq!(a.retire_unwanted_chunks(&mut port, 1).unwrap().retired, 1);
        assert_eq!(a.acquisition.retirement_work(), (1, 1));
        assert!(a.live_chunk_facts(save_key(0)).is_none());
        assert!(!a.residents.ready.contains_key(&save_key(0)));
        port.held.clear();
        a.acquisition.reset_retirement_work();
        assert_eq!(
            a.retire_unwanted_chunks(&mut port, usize::MAX)
                .unwrap()
                .retired,
            8
        );
        assert_eq!(a.acquisition.retirement_work(), (8, 8));
        assert!(a.live_chunk_facts(save_key(9)).is_some());
        assert_eq!(a.residents.ready.len(), 991);
        let calls = port.calls;
        a.acquisition.reset_retirement_work();
        assert_eq!(
            a.retire_unwanted_chunks(&mut port, usize::MAX)
                .unwrap()
                .retired,
            0
        );
        assert_eq!(port.calls, calls);
        assert_eq!(a.acquisition.retirement_work(), (0, 0));

        let mut a = retirement_authority(2, false);
        a.replace_chunk_wants(BTreeSet::new()).unwrap();
        let mut port = RetirementDouble::accepting();
        port.refusal_at = Some(2);
        assert_eq!(
            a.retire_unwanted_chunks(&mut port, 8).unwrap(),
            ChunkRetirementReport {
                retired: 1,
                first_error: Some(ServerError::Disconnected),
            }
        );
        assert!(a.live_chunk_facts(save_key(0)).is_none());
        assert!(a.residents.ready.contains_key(&save_key(1)));
        assert_eq!(port.held.len(), 1);
    }
    #[test]
    fn retirement_private_refusal_restores_every_node_and_capture() {
        let mut a = retirement_authority(1, false);
        for index in 0..4096 {
            let pos = BlockPos::new(index % 16, -64 + index / 256, index / 16 % 16);
            a.residents.blocks.insert(
                (key(), pos),
                BlockObservation::try_new(key(), 1, 7 + index as u64, pos, 0).unwrap(),
            );
        }
        let addresses: BTreeMap<_, _> = (&a.residents.blocks)
            .into_iter()
            .map(|(k, v)| {
                (
                    *k,
                    (
                        std::ptr::from_ref(k) as usize,
                        std::ptr::from_ref(v) as usize,
                        *v,
                    ),
                )
            })
            .collect();
        let old = a
            .capture_chunk_snapshot(key(), SaveUrgency::Autosave)
            .unwrap();
        let old_body = match &old.value {
            SaveValue::ChunkView(v) => v.materialize(),
            _ => unreachable!(),
        };
        a.replace_chunk_wants(BTreeSet::new()).unwrap();
        let facts = a.live_chunk_facts(key()).unwrap();
        let slots = (
            a.residents.drops[&key()].slots,
            a.residents.container_chunks[&key()].furnaces,
            a.residents.container_chunks[&key()].chests,
        );
        let stats = a.save_stats();
        world::reset_ready_clones();
        let mut port = RetirementDouble::accepting();
        port.refusal_at = Some(1);
        assert_eq!(
            a.retire_unwanted_chunks(&mut port, 1).unwrap().first_error,
            Some(ServerError::Disconnected)
        );
        assert_eq!(world::ready_clones(), 0);
        assert_eq!(a.live_chunk_facts(key()), Some(facts));
        assert_eq!(a.residents.blocks.len(), 4096);
        assert_eq!(a.residents.drops[&key()].slots, slots.0);
        assert_eq!(a.residents.container_chunks[&key()].furnaces, slots.1);
        assert_eq!(a.residents.container_chunks[&key()].chests, slots.2);
        assert_eq!(a.save_stats(), stats);
        for (k, v) in &a.residents.blocks {
            assert_eq!(
                (
                    std::ptr::from_ref(k) as usize,
                    std::ptr::from_ref(v) as usize,
                    *v
                ),
                addresses[k]
            );
        }
        let new = a
            .capture_chunk_snapshot(key(), SaveUrgency::Unload)
            .unwrap();
        assert_ne!(new.value, old.value);
        let SaveValue::ChunkView(new_view) = new.value else {
            unreachable!()
        };
        assert_eq!(
            (new_view.generation(), new_view.revision()),
            (facts.generation, facts.revision)
        );
        assert_eq!(new_view.materialize(), old_body);
        let SaveValue::ChunkView(old_view) = old.value else {
            unreachable!()
        };
        assert_eq!(old_view.materialize(), old_body);
        a.replace_chunk_wants(BTreeSet::from([key()])).unwrap();
        assert_eq!(
            a.live_chunk_facts(key()).unwrap().generation,
            facts.generation
        );
        a.replace_chunk_wants(BTreeSet::new()).unwrap();
        port.refusal_at = None;
        assert_eq!(a.retire_unwanted_chunks(&mut port, 1).unwrap().retired, 1);
        assert_eq!(world::ready_clones(), 0);
        let (ready, drops, containers, nodes) = port.held.pop().unwrap().into_parts();
        assert_eq!(
            (ready.key, ready.generation, ready.revision),
            (key(), facts.generation, facts.revision)
        );
        assert_eq!(drops.slots, slots.0);
        assert_eq!(containers.furnaces, slots.1);
        assert_eq!(containers.chests, slots.2);
        for (k, v) in nodes.unwrap().iter() {
            assert_eq!(
                (
                    std::ptr::from_ref(k) as usize,
                    std::ptr::from_ref(v) as usize,
                    *v
                ),
                addresses[k]
            );
        }
    }
    #[test]
    fn retirement_private_latest_ack_flights_rewrite_and_error_diagnostic() {
        let mut a = retirement_authority(1, false);
        // Use genuine accepted transaction writes: cell CAS diverges from the
        // once-per-commit durable revision, without synthetic book mutation.
        let mut ctx = TickContext::for_tick(&mut a, TickBudget::full());
        for block in [1, 2] {
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
        }
        ctx.commit_carried();
        drop(ctx);
        assert_eq!(a.live_chunk_facts(key()).unwrap().revision, 6);
        assert_eq!(
            a.residents
                .blocks
                .get(&(key(), BlockPos::new(0, -64, 0)))
                .unwrap()
                .revision,
            7
        );
        a.replace_chunk_wants(BTreeSet::new()).unwrap();
        let old = a.select(SaveMode::Urgent, SaveBudget::default());
        // A distinct cell raises durability while preserving the first CAS.
        a.replace_chunk_wants(BTreeSet::from([key()])).unwrap();
        let mut ctx = TickContext::for_tick(&mut a, TickBudget::full());
        let observed = ctx
            .read()
            .observation(Dimension::OVERWORLD, BlockPos::new(1, -64, 0))
            .unwrap();
        ctx.transaction()
            .try_system(
                SystemRule::Support,
                vec![BlockWrite::try_new(observed, 1).unwrap()],
            )
            .unwrap();
        ctx.commit_carried();
        drop(ctx);
        a.replace_chunk_wants(BTreeSet::new()).unwrap();
        assert_eq!(a.live_chunk_facts(key()).unwrap().revision, 7);
        assert_eq!(
            a.residents
                .blocks
                .get(&(key(), BlockPos::new(0, -64, 0)))
                .unwrap()
                .revision,
            7
        );
        let mut port = RetirementDouble::accepting();
        assert_eq!(a.retire_unwanted_chunks(&mut port, 1).unwrap().retired, 0);
        assert_eq!(a.apply_completion(save_completion(old, true)).acked, 1);
        assert_eq!(a.retire_unwanted_chunks(&mut port, 1).unwrap().retired, 0);
        let current = a.select(SaveMode::Urgent, SaveBudget::default());
        let mut failed = save_completion(current.clone(), false);
        failed.error = Some(ServerError::Disconnected);
        a.apply_completion(failed);
        assert_eq!(a.save_stats().in_flight, 1);
        assert_eq!(a.retire_unwanted_chunks(&mut port, 1).unwrap().retired, 0);
        a.return_dirty(current.into_iter().next().unwrap());
        assert_eq!(a.save_stats().in_flight, 0);
        assert_eq!(a.retire_unwanted_chunks(&mut port, 1).unwrap().retired, 0);
        let latest = a.select(SaveMode::Urgent, SaveBudget::default());
        a.acquisition.save_error(key(), ServerError::Disconnected);
        assert_eq!(a.apply_completion(save_completion(latest, true)).acked, 1);
        assert_eq!(a.live_chunk_error(key()), Some(&ServerError::Disconnected));
        assert_eq!(a.retire_unwanted_chunks(&mut port, 1).unwrap().retired, 1);
        let mut a = retirement_authority(1, true);
        a.replace_chunk_wants(BTreeSet::new()).unwrap();
        let rewrite = a.live_chunk_facts(key()).unwrap();
        assert_eq!(
            (
                rewrite.revision,
                rewrite.persisted_revision,
                rewrite.needs_rewrite
            ),
            (5, 5, true)
        );
        assert_eq!(a.retire_unwanted_chunks(&mut port, 1).unwrap().retired, 0);
        let selected = a.select(SaveMode::Urgent, SaveBudget::default());
        a.apply_completion(save_completion(selected, true));
        assert_eq!(a.retire_unwanted_chunks(&mut port, 1).unwrap().retired, 1);
    }
    #[test]
    fn retirement_private_defensive_guards_and_phase() {
        let mut port = RetirementDouble::accepting();
        let mut legacy = authority();
        assert!(legacy.retire_unwanted_chunks(&mut port, 0).is_err());
        let mut a = retirement_authority(1, false);
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
        drop(ctx);
        assert_eq!(
            a.retire_unwanted_chunks(&mut port, 0),
            Err(ServerError::Internal {
                invariant: "uncommitted chunk retirement"
            })
        );
        assert_eq!(port.calls, 0);
        let mut sparse = retirement_authority(1, false);
        let reference =
            ContainerRef::try_new(key().pos, mornlea_domain::ContainerKind::Chest, 0, 1).unwrap();
        sparse.residents.containers.insert(
            reference,
            ContainerRecord {
                reference,
                revision: 5,
                slots: ContainerSlots::Chest([Default::default(); 27]),
            },
        );
        assert_eq!(
            sparse.retire_unwanted_chunks(&mut port, 0),
            Err(ServerError::Internal {
                invariant: "managed sparse containers"
            })
        );
        let mut closing = retirement_authority(1, false);
        closing.replace_chunk_wants(BTreeSet::new()).unwrap();
        closing.phase = ServerPhase::Closing;
        assert_eq!(
            closing
                .retire_unwanted_chunks(&mut port, 1)
                .unwrap()
                .retired,
            1
        );
        closing.phase = ServerPhase::Closed;
        assert_eq!(
            closing.retire_unwanted_chunks(&mut port, 0),
            Err(ServerError::InvalidState {
                phase: ServerPhase::Closed
            })
        );
        for missing in 0..5 {
            let mut a = retirement_authority(1, false);
            a.replace_chunk_wants(BTreeSet::new()).unwrap();
            match missing {
                0 => {
                    a.residents.ready.remove(&key());
                }
                1 => a.residents.ready.get_mut(&key()).unwrap().generation += 1,
                2 => {
                    a.residents.drops.remove(&key());
                }
                3 => {
                    a.residents.container_chunks.remove(&key());
                }
                _ => a.residents.ready.get_mut(&key()).unwrap().key = save_key(8),
            }
            let counts = (
                a.residents.ready.len(),
                a.residents.drops.len(),
                a.residents.container_chunks.len(),
            );
            assert_eq!(
                a.retire_unwanted_chunks(&mut port, 1).unwrap(),
                ChunkRetirementReport {
                    retired: 0,
                    first_error: Some(ServerError::Internal {
                        invariant: "managed chunk ownership"
                    })
                }
            );
            assert_eq!(
                (
                    a.residents.ready.len(),
                    a.residents.drops.len(),
                    a.residents.container_chunks.len()
                ),
                counts
            );
            assert!(a.live_chunk_facts(key()).is_some());
        }
    }

    #[test]
    fn retirement_private_scalar_schedule_view_and_anchor_survive_detach() {
        use mornlea_domain::ContainerKind;
        use mornlea_protocol::{LoginStart, admit_login};
        let mut body = chunk();
        body.sections[0].single = 45;
        body.sections[1].single = 11;
        body.chests[0].generation = 1;
        body.chests[0].active = true;
        body.chests[0].block_index = 4096;
        let mut a = retirement_authority_with_body(1, false, body);
        let player =
            PlayerId::try_from_bytes([1, 0, 0, 0, 0, 0, 64, 0, 128, 0, 0, 0, 0, 0, 0, 1]).unwrap();
        let login = LoginStart::new(player, "Ada", 8).unwrap();
        let admitted =
            admit_login(LoginStart::decode_inbound(&login.encode().unwrap()).unwrap()).unwrap();
        let session = a.admit(admitted, TransportKind::Memory).unwrap();
        let reference = ContainerRef::try_new(key().pos, ContainerKind::Chest, 0, 1).unwrap();
        // Explicit scalar fixtures preserve existing lifecycle validation; detach
        // does not eagerly purge references when central Ready reads disappear.
        assert!(
            a.residents.container_chunks[&key()]
                .record(key(), reference)
                .is_some()
        );
        let lease = ViewLease::new(session, reference);
        a.views.insert(session, lease);
        let pos = BlockPos::new(0, -64, 0);
        let other_player =
            PlayerId::try_from_bytes([2, 0, 0, 0, 0, 0, 64, 0, 128, 0, 0, 0, 0, 0, 0, 2]).unwrap();
        let other_login = LoginStart::new(other_player, "Bea", 8).unwrap();
        let other_admitted =
            admit_login(LoginStart::decode_inbound(&other_login.encode().unwrap()).unwrap())
                .unwrap();
        let anchor_session = a.admit(other_admitted, TransportKind::Memory).unwrap();
        let actor = ActorKey::Player(anchor_session);
        let mut inventory = InventoryRecord::empty();
        inventory.crafting_size = mornlea_domain::CraftingSize::Workbench;
        a.residents.inventories.insert(actor, inventory);
        let runtime = ActorRuntime {
            key: actor,
            controls: None,
            has_view: true,
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
                workbench: Some(pos),
            },
        };
        a.residents.runtimes.insert(actor, runtime.clone());
        a.fluid_schedule.enqueue_fluid(key(), pos, 100);
        a.farmland_schedule.enqueue_candidate(key(), pos, 100);
        a.replace_chunk_wants(BTreeSet::new()).unwrap();
        let mut port = RetirementDouble::accepting();
        assert_eq!(a.retire_unwanted_chunks(&mut port, 1).unwrap().retired, 1);
        assert_eq!(a.views.get(&session), Some(&lease));
        assert_eq!(a.residents.inventories.get(&actor), Some(&inventory));
        assert_eq!(a.residents.runtimes.get(&actor), Some(&runtime));
        assert_eq!(
            a.fluid_schedule.fluid_due(Dimension::OVERWORLD, pos),
            Some(100)
        );
        assert_eq!(a.fluid_schedule.pending_fluid(Dimension::OVERWORLD), 1);
        assert_eq!(
            a.farmland_schedule.candidate_due(Dimension::OVERWORLD, pos),
            Some(100)
        );
        assert_eq!(
            a.farmland_schedule.pending_candidates(Dimension::OVERWORLD),
            1
        );
        let ctx = TickContext::for_tick(&mut a, TickBudget::full());
        assert!(ctx.read().observation(Dimension::OVERWORLD, pos).is_none());
    }
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

#[cfg(test)]
mod failed_tick_tests {
    use super::super::step::{AuthoritativeFinalReducer, reduce_tick, set_dispatch_hook};
    use super::*;
    use mornlea_domain::{BlockPos, ChunkPos, Command, CommandRejection, Event};
    use mornlea_storage::{ContainerSnapshot, StorageKind};
    use std::cell::Cell;
    use std::time::{Duration, Instant};

    thread_local! { static HOOK_CALLS: Cell<usize> = const { Cell::new(0) }; }

    struct HookGuard;
    impl HookGuard {
        fn install(hook: fn(&mut TickContext<'_>) -> Result<(), ServerError>) -> Self {
            HOOK_CALLS.with(|calls| calls.set(0));
            set_dispatch_hook(Some(hook));
            Self
        }
    }
    impl Drop for HookGuard {
        fn drop(&mut self) {
            set_dispatch_hook(None);
        }
    }
    fn key() -> ChunkKey {
        ChunkKey {
            dimension: Dimension::OVERWORLD,
            pos: ChunkPos::new(0, 0),
        }
    }
    fn pos() -> BlockPos {
        BlockPos::new(0, -64, 0)
    }
    fn due_pos() -> BlockPos {
        BlockPos::new(15, 200, 15)
    }
    fn capacity() -> ServerError {
        ServerError::Capacity {
            resource: Resource::Commands,
            limit: 4096,
            observed: 4097,
        }
    }
    fn fixture() -> (AuthorityState, SessionKey, OwnedSnapshot) {
        let mut a = AuthorityState::try_new(
            ServerLimits::try_new(8, 4096, 512, 64, 64, 1_048_576).unwrap(),
            42,
        )
        .unwrap();
        let id =
            PlayerId::try_from_bytes([1, 0, 0, 0, 0, 0, 64, 0, 128, 0, 0, 0, 0, 0, 0, 1]).unwrap();
        let start = mornlea_protocol::LoginStart::new(id, "Ada", 8).unwrap();
        let session = a
            .prepare(
                mornlea_protocol::admit_login(
                    mornlea_protocol::LoginStart::decode_inbound(&start.encode().unwrap()).unwrap(),
                )
                .unwrap(),
                TransportKind::Memory,
            )
            .unwrap();
        a.install(session, None).unwrap();
        a.activate(session).unwrap();
        a.enable_live_chunks().unwrap();
        a.replace_chunk_wants(BTreeSet::from([key()])).unwrap();
        let r = a.reserve_chunk_load(key()).unwrap();
        let request = ChunkRequestId::try_new(1).unwrap();
        a.bind_chunk_load(r, request).unwrap();
        let prepared = super::super::world::PreparedChunk::try_new(
            key(),
            r.generation(),
            RecoveredChunk {
                chunk: Chunk {
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
                },
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
            request,
            result: Ok(Some(prepared)),
        })
        .unwrap();
        a.advance_tick(TickBudget::full()).unwrap();
        let installed = a.live_chunk_facts(key()).unwrap();
        assert_eq!(installed.generation, r.generation());
        assert_eq!(
            (
                installed.revision,
                installed.persisted_revision,
                installed.needs_rewrite,
                installed.recovered
            ),
            (9, 7, true, true)
        );
        assert_eq!(a.residents.ready[&key()].block(pos()), Some(0));
        assert!(a.residents.dirty_chunks.is_empty());
        a.sessions.get_mut(&session).unwrap().outbox.clear();
        let old = a
            .capture_chunk_snapshot(key(), SaveUrgency::Autosave)
            .unwrap();
        a.fluid_schedule_mut().enqueue_fluid(key(), due_pos(), 100);
        a.farmland_schedule_mut()
            .enqueue_candidate(key(), due_pos(), 100);
        (a, session, old)
    }
    fn write_and_emit(
        context: &mut TickContext<'_>,
        recipient: EventRecipient,
    ) -> Result<(), ServerError> {
        HOOK_CALLS.with(|calls| calls.set(calls.get() + 1));
        let observed = context
            .read()
            .observation(Dimension::OVERWORLD, pos())
            .unwrap();
        context
            .transaction()
            .try_system(
                SystemRule::Support,
                vec![BlockWrite::try_new(observed, 1).unwrap()],
            )
            .unwrap();
        context.emit(RoutedEvent::new(
            recipient,
            Event::CommandRejected(CommandRejection::new(1, RejectReason::InvalidRay)),
        ))
    }
    fn oversubscribe(context: &mut TickContext<'_>) -> Result<(), ServerError> {
        write_and_emit(context, EventRecipient::Broadcast)?;
        let session = context
            .read()
            .actors()
            .iter()
            .find_map(|actor| match actor.key {
                ActorKey::Player(session) => Some(session),
                _ => None,
            })
            .unwrap();
        // Explicit trusted-hook oversubscription exceeds ordinary supported admission.
        let envelope = CommandEnvelope::try_new(CommandEnvelopeParts {
            tick: context.authority.next_tick,
            session: session.get(),
            sequence: 4097,
            arrival_index: 4097,
            command: Command::CloseContainer,
        })
        .unwrap();
        context.defer(envelope, RulePhase::Interaction)
    }
    fn unwind(context: &mut TickContext<'_>) -> Result<(), ServerError> {
        write_and_emit(context, EventRecipient::Broadcast)?;
        panic!("intentional trusted dispatch unwind");
    }
    fn stale_delivery(context: &mut TickContext<'_>) -> Result<(), ServerError> {
        write_and_emit(context, EventRecipient::Session(999_999))
    }
    fn success(context: &mut TickContext<'_>) -> Result<(), ServerError> {
        write_and_emit(context, EventRecipient::Broadcast)
    }
    fn io_failure(context: &mut TickContext<'_>) -> Result<(), ServerError> {
        write_and_emit(context, EventRecipient::Broadcast)?;
        Err(ServerError::Io {
            operation: Operation::Shutdown,
            kind: std::io::ErrorKind::Other,
        })
    }
    fn fill(a: &mut AuthorityState, session: SessionKey) {
        for sequence in 1..=4096 {
            a.submit(
                session,
                PlayIntent::Sequenced {
                    sequence,
                    command: Command::CloseContainer,
                },
            )
            .unwrap();
        }
    }
    fn old_is_immutable(old: &OwnedSnapshot) {
        let SaveValue::ChunkView(view) = &old.value else {
            panic!("captured chunk");
        };
        assert_eq!(old.revision, 9);
        assert_eq!(view.materialize().chunk.sections[0].single, 0);
    }
    fn failed_facts(
        a: &mut AuthorityState,
        session: SessionKey,
        old: &OwnedSnapshot,
        tick: u64,
        environment: &Option<EnvironmentState>,
        error: ServerError,
        committed: bool,
    ) {
        assert_eq!(
            a.next_tick(),
            tick,
            "failed execution must not bump endpoint tick"
        );
        assert_eq!(a.phase(), ServerPhase::Closing);
        assert_eq!(a.tick_failure(), Some(error));
        assert_eq!(a.settled_read().err(), Some(error));
        assert_eq!(&a.residents.environment, environment);
        assert_eq!(a.residents.ready[&key()].block(pos()), Some(1));
        assert_eq!(
            a.residents.ready[&key()].revision,
            if committed { 10 } else { 9 }
        );
        assert_eq!(
            a.live_chunk_facts(key()).unwrap().revision,
            if committed { 10 } else { 9 }
        );
        let facts = a.live_chunk_facts(key()).unwrap();
        let SaveValue::ChunkView(old_view) = &old.value else {
            panic!("original capture");
        };
        assert_eq!(facts.generation, old_view.generation());
        assert_eq!(
            (
                facts.persisted_revision,
                facts.needs_rewrite,
                facts.recovered
            ),
            (7, true, true)
        );
        assert_eq!(facts.phase, LiveChunkPhase::Ready);
        assert_eq!(a.residents.dirty_chunks.contains(&key()), !committed);
        assert!(a.commands.is_empty());
        assert!(a.views.is_empty());
        assert!(!a.final_consumed);
        assert_eq!(
            a.fluid_schedule()
                .fluid_due(Dimension::OVERWORLD, due_pos()),
            Some(100)
        );
        assert_eq!(
            a.farmland_schedule()
                .candidate_due(Dimension::OVERWORLD, due_pos()),
            Some(100)
        );
        assert!(
            a.capture_chunk_snapshot(key(), SaveUrgency::Autosave)
                .is_none()
        );
        assert!(a.select(SaveMode::All, SaveBudget::default()).is_empty());
        let last_metadata = a.metadata_snapshot();
        assert_eq!(a.try_metadata_snapshot(), Err(error));
        assert_eq!(a.metadata_snapshot(), last_metadata);
        assert_eq!(a.replace_chunk_wants(BTreeSet::new()), Err(error));
        assert_eq!(
            a.sessions[&session].last_applied_sequence,
            if committed { 0 } else { 4096 }
        );
        old_is_immutable(old);
        let unchanged = a.residents.ready[&key()].revision;
        assert_eq!(a.advance_tick(TickBudget::full()), Err(error));
        assert_eq!(reduce_tick(a, TickBudget::full()), Err(error));
        let mut fallback = CountingFallback(Cell::new(0));
        assert_eq!(a.run_final(&mut fallback), Err(error));
        assert_eq!(fallback.0.get(), 0);
        assert_eq!(a.residents.ready[&key()].revision, unchanged);
        assert_eq!(a.next_tick(), tick);
        assert_eq!(
            a.fail_tick(ServerError::Disconnected),
            error,
            "later failure cannot replace first error"
        );
        assert_eq!(a.tick_failure(), Some(error));
        HOOK_CALLS.with(|calls| assert_eq!(calls.get(), 1));
    }
    #[test]
    fn dispatch_capacity_after_partial_write_stops_tick_and_fences_new_saves() {
        let (mut a, session, old) = fixture();
        let selected = a
            .select(SaveMode::All, SaveBudget::default())
            .pop()
            .unwrap();
        fill(&mut a, session);
        let tick = a.next_tick();
        let environment = a.residents.environment.clone();
        let _hook = HookGuard::install(oversubscribe);
        assert_eq!(
            a.advance_tick(TickBudget::full()),
            Err(capacity()),
            "hard dispatch failure must reach the caller"
        );
        failed_facts(&mut a, session, &old, tick, &environment, capacity(), false);
        assert!(a.sessions[&session].outbox.is_empty());
        assert_eq!(a.save_stats().in_flight, 1);
        a.return_dirty(selected);
        assert_eq!(a.save_stats().in_flight, 0);
        assert!(a.select(SaveMode::All, SaveBudget::default()).is_empty());
    }
    #[test]
    fn dispatch_unwind_returns_moved_schedules_and_retains_failure() {
        let (mut a, session, old) = fixture();
        let selected = a
            .select(SaveMode::All, SaveBudget::default())
            .pop()
            .unwrap();
        fill(&mut a, session);
        let tick = a.next_tick();
        let environment = a.residents.environment.clone();
        let _hook = HookGuard::install(unwind);
        let error = ServerError::Internal {
            invariant: "authoritative tick panic",
        };
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            a.advance_tick(TickBudget::full())
        }));
        assert_eq!(
            result.ok(),
            Some(Err(error)),
            "trusted unwind must become the retained error"
        );
        failed_facts(&mut a, session, &old, tick, &environment, error, false);
        assert!(a.sessions[&session].outbox.is_empty());
        assert_eq!(a.save_stats().in_flight, 1);
        a.return_dirty(selected);
        assert_eq!(a.save_stats().in_flight, 0);
        drop(_hook);
        let (mut healthy, _, _) = fixture();
        healthy.advance_tick(TickBudget::full()).unwrap();
        HOOK_CALLS.with(|calls| assert_eq!(calls.get(), 1));
    }
    #[test]
    fn publication_refusal_after_commit_is_sticky_without_undo() {
        let (mut a, session, old) = fixture();
        let selected = a
            .select(SaveMode::All, SaveBudget::default())
            .pop()
            .unwrap();
        a.publish(TickPublication {
            tick: 0,
            events: vec![RoutedEvent::new(
                EventRecipient::Broadcast,
                Event::CommandRejected(CommandRejection::new(99, RejectReason::InvalidRay)),
            )],
            control: vec![],
            counters: TickCounters::default(),
        })
        .unwrap();
        let frames: Vec<_> = a.sessions[&session]
            .outbox
            .iter()
            .map(|frame| frame.as_bytes().to_vec())
            .collect();
        let tick = a.next_tick();
        let _hook = HookGuard::install(stale_delivery);
        let error = ServerError::StaleSession {
            session: SessionKey::from_raw(999_999).unwrap(),
        };
        assert_eq!(
            a.advance_tick(TickBudget::full()),
            Err(error),
            "publication refusal must reach the caller after commit"
        );
        // Successful dispatch committed its environment before delivery refused.
        let environment = a.residents.environment.clone();
        assert_eq!(environment.as_ref().unwrap().world_time, tick + 1);
        failed_facts(&mut a, session, &old, tick, &environment, error, true);
        assert_eq!(
            a.sessions[&session]
                .outbox
                .iter()
                .map(|frame| frame.as_bytes().to_vec())
                .collect::<Vec<_>>(),
            frames
        );
        assert_eq!(a.save_stats().in_flight, 1);
        // This checked completion is synthetic ownership evidence, not a disk durability claim.
        let submitted = vec![(selected.key.clone(), selected.revision)];
        assert_eq!(
            a.apply_completion(SaveCompletion {
                ticket: SaveTicket::try_from_raw(1).unwrap(),
                snapshots: vec![selected],
                submitted: submitted.clone(),
                committed: submitted,
                error: None,
            })
            .acked,
            1
        );
        assert_eq!(a.save_stats().in_flight, 0);
        assert_eq!(a.live_chunk_facts(key()).unwrap().persisted_revision, 9);
        assert_eq!(a.live_chunk_facts(key()).unwrap().revision, 10);
        assert!(a.select(SaveMode::All, SaveBudget::default()).is_empty());
    }
    #[test]
    fn final_real_reducer_commits_same_write_once_without_frames() {
        let (mut a, session, old) = fixture();
        let tick = a.next_tick();
        let before = a.residents.environment.clone().unwrap();
        let views = a.views.clone();
        let _hook = HookGuard::install(success);
        a.begin_close();
        assert_eq!(a.run_final(&mut AuthoritativeFinalReducer), Ok(tick));
        assert!(
            a.sessions[&session].outbox.is_empty(),
            "actual final execution must suppress frames"
        );
        assert_eq!(a.next_tick(), tick + 1);
        let after = a.residents.environment.clone().unwrap();
        assert_eq!(after.world_time, before.world_time + 1);
        assert_eq!(after.next_tick, before.next_tick + 1);
        assert_eq!(a.residents.ready[&key()].revision, 10);
        assert_eq!(a.live_chunk_facts(key()).unwrap().revision, 10);
        assert!(!a.residents.dirty_chunks.contains(&key()));
        assert_eq!(a.views, views);
        assert_eq!(
            a.fluid_schedule()
                .fluid_due(Dimension::OVERWORLD, due_pos()),
            Some(100)
        );
        assert_eq!(
            a.farmland_schedule()
                .candidate_due(Dimension::OVERWORLD, due_pos()),
            Some(100)
        );
        let selected = a.select(SaveMode::All, SaveBudget::default());
        assert_eq!(selected.len(), 1);
        assert_eq!(selected[0].revision, 10);
        let SaveValue::ChunkView(view) = &selected[0].value else {
            panic!("selected body");
        };
        let body = view.materialize();
        assert_eq!(
            super::super::world::ReadyChunk::try_new(key(), 1, body.revision, body.chunk)
                .unwrap()
                .block(pos())
                .unwrap(),
            1
        );
        old_is_immutable(&old);
        assert_eq!(
            a.run_final(&mut AuthoritativeFinalReducer),
            Err(ServerError::InvalidState {
                phase: ServerPhase::Closing
            })
        );
        assert_eq!(
            a.advance_tick(TickBudget::full()),
            Err(ServerError::InvalidState {
                phase: ServerPhase::Closing
            })
        );
        assert_eq!(a.next_tick(), tick + 1);
        assert_eq!(a.residents.environment, Some(after));
        assert_eq!(a.residents.ready[&key()].revision, 10);
        HOOK_CALLS.with(|calls| assert_eq!(calls.get(), 1));
    }
    struct CountingFallback(Cell<usize>);
    impl FinalReducer for CountingFallback {
        fn reduce_final(&mut self, a: &mut AuthorityState) -> Result<u64, ServerError> {
            self.0.set(self.0.get() + 1);
            Ok(a.next_tick())
        }
    }
    struct FixedClock(Instant);
    impl Clock for FixedClock {
        fn monotonic(&self) -> Instant {
            self.0
        }
        fn unix_ms(&self) -> i64 {
            0
        }
    }
    struct ForbiddenIo;
    impl WorkerLifecycle for ForbiddenIo {
        fn stop_new(&mut self) -> Result<(), ServerError> {
            panic!("unexpected post-failure shutdown I/O")
        }
        fn cancel(&mut self) -> Result<(), ServerError> {
            panic!("unexpected post-failure shutdown I/O")
        }
        fn wait(&mut self, _: Deadline) -> Result<(), ServerError> {
            panic!("unexpected post-failure shutdown I/O")
        }
        fn close(&mut self, _: Deadline) -> Result<(), ServerError> {
            panic!("unexpected post-failure shutdown I/O")
        }
    }
    impl ActorPersistence for ForbiddenIo {
        fn flush(&mut self, _: SaveKey, _: Deadline) -> Result<FlushReport, ServerError> {
            panic!("unexpected post-failure shutdown I/O")
        }
    }
    impl MemoryFinalizer for ForbiddenIo {
        fn pending(&self) -> MemoryFinalizationReport {
            panic!("unexpected post-failure shutdown I/O")
        }
        fn begin_attempt(&mut self, _: Deadline) -> Result<(), ServerError> {
            panic!("unexpected post-failure shutdown I/O")
        }
        fn drain(&mut self, _: Deadline) -> Result<MemoryFinalizationReport, ServerError> {
            panic!("unexpected post-failure shutdown I/O")
        }
    }
    impl StoreHandle for ForbiddenIo {
        fn submit(&mut self, _: SaveRequest) -> Result<SaveTicket, SubmitSaveError> {
            panic!("unexpected post-failure shutdown I/O")
        }
        fn poll(&mut self, _: SaveTicket) -> SavePoll {
            panic!("unexpected post-failure shutdown I/O")
        }
        fn poll_tick(
            &mut self,
            _: u64,
            _: SaveBudget,
            _: &mut dyn SaveAuthority,
        ) -> Result<SaveScheduleReport, ServerError> {
            panic!("unexpected post-failure shutdown I/O")
        }
        fn cancel_pending(&mut self) -> Result<Vec<OwnedSnapshot>, ServerError> {
            panic!("unexpected post-failure shutdown I/O")
        }
        fn flush(
            &mut self,
            _: Deadline,
            _: &mut dyn SaveAuthority,
            _: &dyn Clock,
        ) -> Result<FlushReport, ServerError> {
            panic!("unexpected post-failure shutdown I/O")
        }
        fn sync(&mut self, _: Deadline) -> Result<(), ServerError> {
            panic!("unexpected post-failure shutdown I/O")
        }
        fn close(&mut self, _: Deadline) -> Result<(), ServerError> {
            panic!("unexpected post-failure shutdown I/O")
        }
    }
    impl AgentHandle for ForbiddenIo {
        fn submit(&mut self, _: AgentRequest) -> Result<AgentRequestId, ServerError> {
            panic!("unexpected post-failure shutdown I/O")
        }
        fn poll(&mut self, _: AgentRequestId) -> AgentPoll {
            panic!("unexpected post-failure shutdown I/O")
        }
        fn cancel(&mut self, _: AgentRequestId, _: Deadline) -> Result<(), ServerError> {
            panic!("unexpected post-failure shutdown I/O")
        }
        fn freeze(&mut self, _: &dyn Clock) -> Option<FrozenLease> {
            panic!("unexpected post-failure shutdown I/O")
        }
        fn release(&mut self, _: &FrozenLease, _: Deadline) -> Result<(), ServerError> {
            panic!("unexpected post-failure shutdown I/O")
        }
        fn close(&mut self, _: Deadline) -> Result<(), ServerError> {
            panic!("unexpected post-failure shutdown I/O")
        }
    }
    impl McpLifecycle for ForbiddenIo {
        fn close(&mut self, _: Deadline) -> Result<(), ServerError> {
            panic!("unexpected post-failure shutdown I/O")
        }
    }
    impl SnapshotPort for ForbiddenIo {
        fn register(
            &mut self,
            _: NamespaceId,
            _: mornlea_domain::CompanionId,
            _: u64,
            _: PlanningSnapshot,
            _: Deadline,
        ) -> Result<SnapshotRegistration, ServerError> {
            panic!("unexpected post-failure shutdown I/O")
        }
        fn complete(&mut self, _: SnapshotId) -> Result<(), ServerError> {
            panic!("unexpected post-failure shutdown I/O")
        }
        fn cancel(&mut self, _: SnapshotId) -> Result<(), ServerError> {
            panic!("unexpected post-failure shutdown I/O")
        }
        fn close(&mut self) -> Result<(), ServerError> {
            panic!("unexpected post-failure shutdown I/O")
        }
    }
    fn assert_no_shutdown_io(a: &mut AuthorityState, error: ServerError, scaffold: bool) {
        let tick = a.next_tick();
        let mut fallback = CountingFallback(Cell::new(0));
        let (mut workers, mut actors, mut memory, mut store, mut agent, mut mcp, mut snapshots) = (
            ForbiddenIo,
            ForbiddenIo,
            ForbiddenIo,
            ForbiddenIo,
            ForbiddenIo,
            ForbiddenIo,
            ForbiddenIo,
        );
        let clock = FixedClock(Instant::now());
        let deadline = Deadline::at(clock.0 + Duration::from_secs(5));
        for failed in 1..=2 {
            let result = if scaffold {
                a.drive_shutdown(
                    deadline,
                    &mut ShutdownIo {
                        reducer: &mut fallback,
                        workers: &mut workers,
                        persistence: &mut actors,
                        memory: &mut memory,
                        store: &mut store,
                        agent: &mut agent,
                        mcp: &mut mcp,
                        snapshots: &mut snapshots,
                        clock: &clock,
                    },
                )
            } else {
                super::super::shutdown::shutdown(
                    a,
                    &mut super::super::shutdown::ShutdownPorts {
                        reducer: &mut fallback,
                        workers: &mut workers,
                        actors: &mut actors,
                        memory: &mut memory,
                        store: &mut store,
                        agent: &mut agent,
                        mcp: &mut mcp,
                        clock: &clock,
                    },
                    deadline,
                )
            };
            let failure = result.unwrap_err();
            assert_eq!(failure.error, error);
            assert_eq!(failure.report.next, ShutdownPhase::FinalTick);
            assert!(!failure.report.retryable);
            assert_eq!(failure.report.completed, vec![ShutdownPhase::StopAdmission]);
            assert_eq!(failure.report.final_tick, None);
            assert_eq!(failure.report.failed, failed);
            assert_eq!(failure.report.durable, 0);
            assert_eq!(a.shutdown_progress(), failure.report);
            assert_eq!(fallback.0.get(), 0);
            assert_eq!(a.next_tick(), tick);
        }
    }
    #[test]
    fn actual_final_hard_error_cannot_replay_or_flush() {
        for scaffold in [false, true] {
            let (mut a, session, _) = fixture();
            fill(&mut a, session);
            let tick = a.next_tick();
            let _hook = HookGuard::install(oversubscribe);
            a.begin_close();
            assert_eq!(
                a.run_final(&mut AuthoritativeFinalReducer),
                Err(capacity()),
                "actual failed final must remain unconsumed"
            );
            assert!(!a.final_consumed);
            assert_eq!(a.next_tick(), tick);
            assert_no_shutdown_io(&mut a, capacity(), scaffold);
            HOOK_CALLS.with(|calls| assert_eq!(calls.get(), 1));
        }
    }
    #[test]
    fn retained_dispatch_io_is_permanent_in_both_shutdown_machines() {
        for scaffold in [false, true] {
            let (mut a, _, _) = fixture();
            let _hook = HookGuard::install(io_failure);
            let error = ServerError::Io {
                operation: Operation::Shutdown,
                kind: std::io::ErrorKind::Other,
            };
            assert_eq!(
                a.advance_tick(TickBudget::full()),
                Err(error),
                "normally transient I/O becomes a retained tick failure"
            );
            assert_no_shutdown_io(&mut a, error, scaffold);
            HOOK_CALLS.with(|calls| assert_eq!(calls.get(), 1));
        }
    }
}

#[cfg(test)]
mod settled_read_tests {
    use super::super::{step::AuthoritativeFinalReducer, world};
    use super::*;
    use mornlea_domain::{BlockPos, ChunkPos, Command, Season, WorldStateParts};
    use mornlea_storage::{ContainerSnapshot, StorageKind};

    fn authority(time: u64) -> AuthorityState {
        let limits = ServerLimits::try_new(8, 4096, 512, 64, 64, 1_048_576).unwrap();
        let mut metadata = AuthorityState::try_new(limits, 42).unwrap().metadata;
        metadata.world_time_ticks = time;
        AuthorityState::try_new_with_metadata(limits, metadata).unwrap()
    }

    fn key() -> ChunkKey {
        ChunkKey {
            dimension: Dimension::OVERWORLD,
            pos: ChunkPos::new(0, 0),
        }
    }

    fn session(a: &mut AuthorityState) -> (SessionKey, PlayerId) {
        let mut bytes = [0; 16];
        bytes[0] = 51;
        bytes[6] = 0x40;
        bytes[8] = 0x80;
        let player = PlayerId::try_from_bytes(bytes).unwrap();
        let start = mornlea_protocol::LoginStart::new(player, "Settled", 8).unwrap();
        let admitted = mornlea_protocol::admit_login(
            mornlea_protocol::LoginStart::decode_inbound(&start.encode().unwrap()).unwrap(),
        )
        .unwrap();
        let session = a.prepare(admitted, TransportKind::Memory).unwrap();
        a.install(session, None).unwrap();
        a.activate(session).unwrap();
        (session, player)
    }

    fn empty_lanes(view: AuthorityReadView<'_>) {
        assert!(view.commands().is_empty());
        assert!(view.companion_actions().is_empty());
        assert!(view.interactions().is_empty());
        assert!(view.damage_intents().is_empty());
        assert!(view.pre_step.is_empty());
        assert!(view.observation_trace.is_none());
    }

    #[test]
    fn cold_metadata_clock() {
        let mut a = authority(1200);
        assert!(a.residents.environment.is_none());
        let view = a.settled_read().unwrap();
        assert_eq!(view.tick(), 0);
        assert_eq!(view.world(), None);
        assert_eq!(view.world_time(), 1200);
        assert_eq!(
            view.spawn_anchor(),
            Some((Dimension::OVERWORLD, ChunkPos::new(0, 0)))
        );
        assert_eq!(view.metadata.seed, 42);
        empty_lanes(view);
        let context = TickContext::harness(&mut a, TickBudget::full());
        assert_eq!(context.read().world_time(), 1200);
    }

    fn owner_addresses(resident: &ResidentTickState, a: &AuthorityState) -> [usize; 12] {
        [
            std::ptr::from_ref(&resident.inventories) as usize,
            std::ptr::from_ref(&resident.blocks) as usize,
            std::ptr::from_ref(&resident.ready) as usize,
            std::ptr::from_ref(&resident.runtimes) as usize,
            std::ptr::from_ref(&resident.mining) as usize,
            std::ptr::from_ref(resident.environment.as_ref().unwrap()) as usize,
            std::ptr::from_ref(&resident.containers) as usize,
            std::ptr::from_ref(&resident.container_chunks) as usize,
            std::ptr::from_ref(&a.views) as usize,
            std::ptr::from_ref(&resident.drops) as usize,
            std::ptr::from_ref(&a.metadata) as usize,
            std::ptr::from_ref(&resident.ready[&key()]) as usize,
        ]
    }

    #[test]
    fn committed_borrow_identity_and_current_write() {
        let mut a = authority(0);
        let (session, player) = session(&mut a);
        let actor_key = ActorKey::Player(session);
        let pos = BlockPos::new(8, 63, 8);
        {
            let mut context = TickContext::for_tick(&mut a, TickBudget::full());
            let chunk = Chunk {
                sections: vec![
                    ContainerSnapshot {
                        kind: StorageKind::Single,
                        bits: 0,
                        single: 0,
                        palette: vec![],
                        packed: vec![],
                    };
                    24
                ],
                drops: vec![Default::default(); 32],
                furnaces: vec![Default::default(); 32],
                chests: vec![Default::default(); 16],
            };
            context.preload_ready_chunk(ReadyChunk::try_new(key(), 1, 9, chunk).unwrap());
            let mut save = canonical_player(player, "Settled").unwrap();
            save.current.position = [8.5, 64.0, 8.5];
            context.stage_login(seed_player(session, &save).unwrap());
            context.freeze_environment(0);
            let actor = context.read().actor(actor_key).unwrap().clone();
            let runtime =
                crate::rules::player_survival::merged_runtime(&context.read(), &actor).unwrap();
            context.stage(RuleEffect::Runtime(runtime)).unwrap();
            let observed = context
                .read()
                .observation(Dimension::OVERWORLD, pos)
                .unwrap();
            context
                .transaction()
                .try_system(
                    SystemRule::Support,
                    vec![BlockWrite::try_new(observed, 2).unwrap()],
                )
                .unwrap();
            context.commit_carried();
        }
        let resident = &a.residents;
        let owners_before = owner_addresses(resident, &a);
        let ready = &resident.ready[&key()];
        let ready_before = (
            ready.generation,
            ready.revision,
            ready.block(pos),
            ready.height(8, 8),
        );
        let actor_before = resident.actors[0].clone();
        let actors_ptr = resident.actors.as_ptr();
        let projectiles_ptr = resident.projectiles.as_ptr();
        let capacities = (resident.actors.capacity(), resident.projectiles.capacity());
        world::reset_ready_clones();
        world::reset_materializations();
        for _ in 0..8 {
            let view = a.settled_read().unwrap();
            assert!(view.acquisition.is_none());
            assert!(std::ptr::eq(view.inventories, &resident.inventories));
            assert!(std::ptr::eq(view.blocks, &resident.blocks));
            assert!(std::ptr::eq(view.ready, &resident.ready));
            assert!(std::ptr::eq(view.actors, resident.actors.as_slice()));
            assert_eq!(view.actors.as_ptr(), actors_ptr);
            assert!(std::ptr::eq(view.runtimes, &resident.runtimes));
            assert!(std::ptr::eq(view.mining, &resident.mining));
            assert!(std::ptr::eq(
                view.environment.unwrap(),
                resident.environment.as_ref().unwrap()
            ));
            assert!(std::ptr::eq(view.containers, &resident.containers));
            assert!(std::ptr::eq(
                view.container_chunks,
                &resident.container_chunks
            ));
            assert!(std::ptr::eq(view.viewers, &a.views));
            assert!(std::ptr::eq(view.drops, &resident.drops));
            assert!(std::ptr::eq(
                view.projectiles,
                resident.projectiles.as_slice()
            ));
            assert_eq!(view.projectiles.as_ptr(), projectiles_ptr);
            assert!(std::ptr::eq(view.metadata, &a.metadata));
            assert!(std::ptr::eq(
                view.inventory(actor_key).unwrap(),
                &resident.inventories[&actor_key]
            ));
            assert!(std::ptr::eq(
                view.runtime(actor_key).unwrap(),
                &resident.runtimes[&actor_key]
            ));
            assert!(std::ptr::eq(
                view.actor(actor_key).unwrap(),
                &resident.actors[0]
            ));
            assert_eq!(view.tick(), 0);
            assert_eq!(view.world(), None);
            assert_eq!(view.block(Dimension::OVERWORLD, pos), Some(2));
            assert_eq!(view.ready_chunk_revision(key()), Some(10));
            assert_eq!(view.highest_non_air(Dimension::OVERWORLD, 8, 8), Some(63));
            empty_lanes(view);
        }
        assert_eq!((world::ready_clones(), world::materializations()), (0, 0));
        assert_eq!(ready_before, (1, 10, Some(2), 63));
        assert_eq!(
            (
                ready.generation,
                ready.revision,
                ready.block(pos),
                ready.height(8, 8)
            ),
            ready_before
        );
        assert_eq!(owner_addresses(resident, &a), owners_before);
        assert_eq!(resident.actors[0], actor_before);
        assert_eq!(resident.actors.as_ptr(), actors_ptr);
        assert_eq!(resident.projectiles.as_ptr(), projectiles_ptr);
        assert_eq!(
            (resident.actors.capacity(), resident.projectiles.capacity()),
            capacities
        );
    }

    #[test]
    fn actual_clock_progress_and_final() {
        let mut a = authority(1200);
        {
            let mut context = TickContext::for_tick(&mut a, TickBudget::full());
            context.freeze_environment(0);
            assert_eq!(context.read().world(), None);
            assert_eq!(context.read().environment().unwrap().world_time, 1200);
            assert_eq!(context.read().world_time(), 1200);
        }
        assert_eq!(a.residents.environment.as_ref().unwrap().world_time, 1200);
        for tick in 0..2 {
            let publication = a.advance_tick(TickBudget::full()).unwrap();
            assert_eq!(publication.tick, tick);
            let view = a.settled_read().unwrap();
            assert_eq!(view.tick(), tick + 1);
            assert_eq!(view.world_time(), 1201 + tick);
            assert_eq!(view.world(), None);
        }
        let outbox_prefix: usize = a
            .sessions
            .values()
            .map(|session| session.outbox.len())
            .sum();
        assert!(a.begin_close());
        assert_eq!(a.run_final(&mut AuthoritativeFinalReducer).unwrap(), 2);
        assert_eq!(
            a.sessions
                .values()
                .map(|session| session.outbox.len())
                .sum::<usize>(),
            outbox_prefix
        );
        let view = a.settled_read().unwrap();
        assert_eq!(view.tick(), 3);
        assert_eq!(view.world_time(), 1203);
        assert_eq!(view.environment().unwrap().world_time, 1203);
        assert_eq!(view.world(), None);
    }

    #[test]
    fn fixture_world_override() {
        let mut a = authority(1200);
        let mut context = TickContext::harness(&mut a, TickBudget::full());
        context.freeze_environment(0);
        context.world = Some(
            WorldState::try_new(WorldStateParts {
                day_phase_offset: 0,
                world_time_ticks: 999,
                weather: Weather::Clear,
                season: Season::Spring,
                season_progress: 0,
                temperature: 11,
            })
            .unwrap(),
        );
        assert_eq!(context.read().world_time(), 999);
        context.world = None;
        assert_eq!(context.read().world_time(), 1200);
        crate::rules::environment::run(
            &mut context,
            RuleCall {
                phase: RulePhase::EnvironmentEnd,
                actor: None,
                command: None,
                internal: None,
            },
        )
        .unwrap();
        assert_eq!(context.read().world_time(), 1201);
        assert_eq!(context.read().tick(), 0);
    }

    #[test]
    fn phases_and_first_error() {
        let mut a = authority(0);
        assert!(a.settled_read().is_ok());
        assert!(a.begin_close());
        assert!(a.settled_read().is_ok());
        a.phase = ServerPhase::Closed;
        assert_eq!(
            a.settled_read().err(),
            Some(ServerError::InvalidState {
                phase: ServerPhase::Closed
            })
        );
        let mut failed = authority(1200);
        let error = ServerError::Internal {
            invariant: "settled read test failure",
        };
        failed.fail_tick(error);
        failed.fail_tick(ServerError::Disconnected);
        let before = (
            failed.next_tick,
            failed.metadata.clone(),
            failed.residents.environment.clone(),
        );
        assert_eq!(failed.settled_read().err(), Some(error));
        failed.phase = ServerPhase::Closed;
        assert_eq!(failed.settled_read().err(), Some(error));
        assert_eq!(
            (
                failed.next_tick,
                failed.metadata.clone(),
                failed.residents.environment.clone()
            ),
            before
        );
    }

    #[test]
    fn pending_ingress_is_not_settled() {
        let mut a = authority(0);
        let (session, _) = session(&mut a);
        a.submit(
            session,
            PlayIntent::Sequenced {
                sequence: 1,
                command: Command::CloseContainer,
            },
        )
        .unwrap();
        let pointer = a.commands.as_ptr();
        let queued = a.commands.clone();
        assert!(!queued.is_empty());
        empty_lanes(a.settled_read().unwrap());
        assert_eq!(a.commands.as_ptr(), pointer);
        assert_eq!(a.commands, queued);
    }
}

#[cfg(test)]
mod source_player_restore_tests {
    use super::super::{
        step::{AuthoritativeFinalReducer, set_dispatch_hook},
        world::PreparedChunk,
    };
    use super::*;
    use mornlea_domain::{BlockPos, ChunkPos, HotbarSlot};
    use mornlea_protocol::{LoginStart, PlayerInput, SelectHotbar, admit_login};
    use mornlea_storage::{ContainerSnapshot, StorageKind};
    use std::panic::{AssertUnwindSafe, catch_unwind};

    fn fresh() -> AuthorityState {
        AuthorityState::try_new(
            ServerLimits::try_new(8, 4096, 512, 64, 64, 1_048_576).unwrap(),
            42,
        )
        .unwrap()
    }
    fn login(tag: u8) -> AdmittedLogin {
        let mut id = [0; 16];
        id[0] = tag;
        id[6] = 64;
        id[8] = 128;
        let start = LoginStart::new(PlayerId::try_from_bytes(id).unwrap(), "Ada", 8).unwrap();
        admit_login(LoginStart::decode_inbound(&start.encode().unwrap()).unwrap()).unwrap()
    }
    fn saved(tag: u8) -> PlayerSave {
        let mut save = canonical_player(login(tag).player_id(), "Ada").unwrap();
        save.current = PlayerLocation {
            dimension: 0,
            position: [8.5, 65., 8.5],
        };
        save.safe = Some(PlayerLocation {
            dimension: 1,
            position: [56.5, 64., 8.5],
        });
        save.yaw = 0.1;
        save.pitch = 0.2;
        save.health = 15;
        save.hunger = 17;
        save.saturation_milli = 9000;
        save.exhaustion_milli = 250;
        save.inventory.hotbar.selected = 3;
        save.inventory.hotbar.slots[3] = mornlea_storage::ItemStack {
            item: 1,
            count: 7,
            durability: 0,
        };
        save.respawn_present = true;
        save.respawn_dimension = 1;
        save.respawn_position = [-0.5, 1.5, 0.49];
        save
    }
    fn stored(s: PlayerSave) -> StoredPlayer {
        StoredPlayer {
            player_id: s.player_id,
            revision: s.revision,
            display_name: s.display_name,
            current: s.current,
            yaw: s.yaw,
            pitch: s.pitch,
            safe: s.safe,
            inventory: s.inventory,
            health: s.health,
            hunger: s.hunger,
            saturation_milli: s.saturation_milli,
            exhaustion_milli: s.exhaustion_milli,
            respawn_present: s.respawn_present,
            respawn_position: s.respawn_position,
            respawn_dimension: s.respawn_dimension,
            armor: s.armor,
            needs_rewrite: false,
        }
    }
    fn register(a: &mut AuthorityState, tag: u8, save: Option<PlayerSave>) -> SessionKey {
        let s = a.prepare(login(tag), TransportKind::Memory).unwrap();
        a.install(s, save.map(stored)).unwrap();
        a.activate(s).unwrap();
        s
    }
    fn key(dimension: Dimension, x: i32, z: i32) -> ChunkKey {
        ChunkKey {
            dimension,
            pos: ChunkPos::new(x, z),
        }
    }
    fn compact(kind: u8) -> Chunk {
        let mut chunk = Chunk {
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
        };
        if kind == 1 {
            chunk.sections[7].single = 1;
        }
        if kind == 2 {
            for section in &mut chunk.sections {
                section.single = 1;
            }
        }
        if kind == 3 {
            chunk.sections[1].single = 11;
            chunk.chests[0].generation = 1;
            chunk.chests[0].active = true;
            chunk.chests[0].block_index = 4096;
            chunk.chests[0].items[0] = mornlea_storage::ItemStack {
                item: 1,
                count: 3,
                durability: 0,
            };
        }
        chunk
    }
    fn offer(a: &mut AuthorityState, key: ChunkKey, kind: u8) {
        let mut wants = a.residents.ready.keys().copied().collect::<BTreeSet<_>>();
        wants.extend(
            a.source_players
                .entries
                .values()
                .flat_map(|entry| entry.restore.pending_keys()),
        );
        wants.insert(key);
        a.replace_chunk_wants(wants).unwrap();
        let r = a.reserve_chunk_load(key).unwrap();
        let request = ChunkRequestId::try_new(r.generation()).unwrap();
        a.bind_chunk_load(r, request).unwrap();
        let prepared = PreparedChunk::try_new(
            key,
            r.generation(),
            RecoveredChunk {
                chunk: compact(kind),
                revision: 9,
                persisted_revision: 9,
                needs_rewrite: false,
                recovered: false,
            },
        )
        .unwrap();
        a.offer_acquired(AcquiredChunkEvent::Load {
            key,
            generation: r.generation(),
            request,
            result: Ok(Some(prepared)),
        })
        .unwrap();
    }
    fn fixture() -> (AuthorityState, SessionKey) {
        let mut a = fresh();
        a.metadata.spawn_anchor = mornlea_storage::MetadataChunkPos { x: 2, z: 0 };
        a.enable_source_player_restoration(1).unwrap();
        a.enable_live_chunks().unwrap();
        let s = register(&mut a, 1, Some(saved(1)));
        (a, s)
    }
    fn player(a: &AuthorityState, s: SessionKey) -> &ActorRecord {
        a.residents
            .actors
            .iter()
            .find(|a| a.key == ActorKey::Player(s))
            .unwrap()
    }
    fn local(p: &TickPublication) -> &mornlea_domain::PlayerState {
        p.events
            .iter()
            .find_map(|e| {
                if let mornlea_domain::Event::PlayerState(s) = e.event() {
                    Some(s)
                } else {
                    None
                }
            })
            .unwrap()
    }
    #[test]
    fn explicit_mode_radius_health_anchor_and_idempotence() {
        for radius in [1, 16, 64] {
            let mut a = fresh();
            a.enable_source_player_restoration(radius).unwrap();
            assert_eq!(a.source_player_radius, Some(radius));
        }
        for radius in [0, 65] {
            let mut a = fresh();
            assert_eq!(
                a.enable_source_player_restoration(radius),
                Err(ServerError::InvalidInput {
                    field: "spawn_radius"
                })
            );
            assert_eq!(a.source_player_radius, None);
        }
        for x in [-134_217_724, 134_217_723] {
            let mut a = fresh();
            a.metadata.spawn_anchor.x = x;
            a.enable_source_player_restoration(64).unwrap();
        }
        for x in [i32::MIN, i32::MAX] {
            let mut a = fresh();
            a.metadata.spawn_anchor.x = x;
            assert_eq!(
                a.enable_source_player_restoration(1),
                Err(ServerError::InvalidInput {
                    field: "spawn_anchor"
                })
            );
        }
        let mut a = fresh();
        a.enable_source_player_restoration(16).unwrap();
        a.prepare(login(1), TransportKind::Memory).unwrap();
        a.enable_source_player_restoration(16).unwrap();
        assert_eq!(
            a.enable_source_player_restoration(1),
            Err(ServerError::InvalidInput {
                field: "source_player_radius"
            })
        );
        for phase in [ServerPhase::Closing, ServerPhase::Closed] {
            let mut a = fresh();
            a.phase = phase;
            assert_eq!(
                a.enable_source_player_restoration(1),
                Err(ServerError::InvalidState { phase })
            );
            let error = ServerError::Internal {
                invariant: "injected failure",
            };
            a.tick_failure = Some(error);
            assert_eq!(a.enable_source_player_restoration(1), Err(error));
        }
        let mut a = fresh();
        a.metadata.spawn_dimension = 2;
        assert_eq!(
            a.enable_source_player_restoration(1),
            Err(ServerError::Internal {
                invariant: "source player spawn metadata"
            })
        );
    }
    #[test]
    fn enable_refuses_existing_player_owners_and_history_without_mutation() {
        for lane in 0..6 {
            let mut a = fresh();
            let s = a.prepare(login(1), TransportKind::Memory).unwrap();
            let seeded = seed_player(s, &saved(1)).unwrap();
            let runtime = crate::rules::player_survival::merged_runtime(
                &a.settled_read().unwrap(),
                &seeded.actor,
            )
            .unwrap();
            if lane == 1 {
                a.retire(s, CloseReason::PeerGone).unwrap();
            }
            if lane >= 2 {
                a.sessions.clear();
                a.current_sessions.clear();
                a.occupied = 0;
            }
            match lane {
                2 => a.residents.actors.push(seeded.actor),
                3 => {
                    a.residents
                        .inventories
                        .insert(ActorKey::Player(s), seeded.inventory);
                }
                4 => {
                    a.residents.runtimes.insert(ActorKey::Player(s), runtime);
                }
                5 => {
                    a.residents.mining.insert(
                        ActorKey::Player(s),
                        MiningProgress {
                            actor: ActorKey::Player(s),
                            dimension: Dimension::OVERWORLD,
                            target: BlockPos::new(0, 0, 0),
                            observed_block: 1,
                            tool_slot: HotbarSlot::new(0).unwrap(),
                            tool: Default::default(),
                            elapsed: 0,
                            required: 1,
                            last_tick: 0,
                        },
                    );
                }
                _ => {}
            }
            let before = (
                a.sessions.len(),
                a.residents.actors.len(),
                a.residents.inventories.len(),
                a.residents.runtimes.len(),
                a.residents.mining.len(),
                a.next_session,
            );
            assert_eq!(
                a.enable_source_player_restoration(1),
                Err(ServerError::InvalidInput {
                    field: "source_player_restoration"
                })
            );
            assert_eq!(a.source_player_radius, None);
            assert_eq!(
                before,
                (
                    a.sessions.len(),
                    a.residents.actors.len(),
                    a.residents.inventories.len(),
                    a.residents.runtimes.len(),
                    a.residents.mining.len(),
                    a.next_session
                )
            );
        }
    }
    #[test]
    fn source_direct_allocation_refuses_before_reservation() {
        let mut a = fresh();
        a.enable_source_player_restoration(1).unwrap();
        assert_eq!(
            a.allocate(login(1), TransportKind::Memory),
            Err(ServerError::InvalidInput {
                field: "background_login_required"
            })
        );
        assert_eq!((a.next_session, a.occupied, a.sessions.len()), (1, 0, 0));
    }
    #[test]
    fn missing_registration_separates_canonical_body_and_explicit_runtime() {
        let mut a = fresh();
        a.metadata.spawn_dimension = 1;
        a.metadata.spawn_anchor = mornlea_storage::MetadataChunkPos { x: -2, z: 3 };
        a.enable_source_player_restoration(1).unwrap();
        let s = register(&mut a, 1, None);
        let p = player(&a, s);
        assert_eq!(p.lifecycle, ActorLifecycle::Pending);
        assert_eq!(p.dimension, Dimension::DEPTHS);
        assert_eq!(p.motion.position().get(), [-31.5, 321., 48.5]);
        assert_eq!(p.motion.velocity().get(), [0.; 3]);
        assert!(!p.motion.on_ground());
        let ActorBody::Player(body) = &p.body else {
            panic!("body")
        };
        assert_eq!(body.current.position, [0., 64., 0.]);
        assert_eq!(body.current.dimension, 0);
        assert_eq!(
            (
                p.survival.health(),
                p.survival.hunger(),
                p.survival.oxygen()
            ),
            (20, 20, 300)
        );
        let runtime = &a.residents.runtimes[&p.key];
        assert_eq!(
            (
                runtime.saturation_milli,
                runtime.exhaustion_milli,
                runtime.oxygen,
                runtime.peak_y
            ),
            (5000, 0, 300, 321.)
        );
        assert_eq!(runtime.controls, None);
        assert!(!runtime.has_view && !runtime.reset);
        assert_eq!(
            runtime.aux,
            ActorAux::Player {
                respawn: None,
                workbench: None
            }
        );
        assert_eq!(
            a.residents.inventories[&p.key].slots,
            [Default::default(); 36]
        );
        assert_eq!(
            a.residents.inventories[&p.key].armor,
            [Default::default(); 4]
        );
        assert_eq!(
            a.residents.inventories[&p.key].crafting,
            [Default::default(); 9]
        );
        let entry = &a.source_players.entries[&s];
        assert!(!entry.ever_spawned);
        assert_eq!(
            entry.restore.pending_keys(),
            vec![key(Dimension::DEPTHS, -2, 3)]
        );
        assert!(!a.sessions[&s].loaded_current);
        assert!(a.login_seeds().is_empty());
    }
    #[test]
    fn loaded_registration_preserves_source_fields_and_rounds_bed() {
        let (a, s) = fixture();
        let p = player(&a, s);
        assert_eq!(p.lifecycle, ActorLifecycle::Pending);
        assert_eq!(p.motion.position().get(), [32.5, 321., 0.5]);
        assert_eq!(
            (
                p.look.yaw(),
                p.look.pitch(),
                p.survival.health(),
                p.survival.hunger()
            ),
            (0.1, 0.2, 15, 17)
        );
        let r = &a.residents.runtimes[&p.key];
        assert_eq!((r.saturation_milli, r.exhaustion_milli), (9000, 250));
        assert_eq!(
            r.aux,
            ActorAux::Player {
                respawn: Some((Dimension::DEPTHS, BlockPos::new(-1, 2, 0))),
                workbench: None
            }
        );
        assert_eq!(a.residents.inventories[&p.key].selected.get(), 3);
        assert_eq!(a.residents.inventories[&p.key].slots[3].count, 7);
        assert!(a.sessions[&s].loaded_current);
        assert!(
            format!("{:?}", a.source_players.entries[&s].restore)
                .contains("require_support: false")
        );
        assert!(
            format!("{:?}", a.source_players.entries[&s].restore).contains("require_support: true")
        );
    }
    #[test]
    fn failed_install_and_registration_leave_prepared_owners_intact() {
        let mut a = fresh();
        a.enable_source_player_restoration(1).unwrap();
        let s = a.prepare(login(1), TransportKind::Memory).unwrap();
        a.install(s, None).unwrap();
        let old = a.sessions[&s].body.clone();
        assert_eq!(
            a.install(s, Some(stored(saved(2)))),
            Err(ServerError::InvalidInput { field: "player_id" })
        );
        assert_eq!(a.sessions[&s].body, old);
        assert!(!a.sessions[&s].loaded_current);
        let mut invalid = saved(1);
        invalid.current.dimension = 2;
        assert!(a.install(s, Some(stored(invalid))).is_err());
        assert_eq!(a.sessions[&s].body, old);
        assert!(!a.sessions[&s].loaded_current);
        a.residents.inventories.insert(
            ActorKey::Player(s),
            seed_player(s, &saved(1)).unwrap().inventory,
        );
        assert_eq!(
            a.activate(s),
            Err(ServerError::Internal {
                invariant: "source player registration"
            })
        );
        assert_eq!(a.sessions[&s].phase, SessionPhase::Prepared);
        assert_eq!(a.sessions[&s].body, old);
        assert!(
            a.residents.actors.is_empty()
                && a.residents.runtimes.is_empty()
                && a.source_players.entries.is_empty()
        );
    }
    fn envelope(s: SessionKey, command: mornlea_domain::Command) -> CommandEnvelope {
        CommandEnvelope::try_new(CommandEnvelopeParts {
            tick: 0,
            session: s.get(),
            sequence: 1,
            arrival_index: 0,
            command,
        })
        .unwrap()
    }
    #[test]
    fn source_pending_gate_keeps_only_close_resync_and_whole_move_exceptions() {
        use mornlea_domain::{
            Command, ContainerKind, ContainerMove, PartialMove, ResyncIntent, StackSource,
            StackView,
        };
        let (mut a, s) = fixture();
        let reference =
            ContainerRef::try_new(ChunkPos::new(0, 0), ContainerKind::Chest, 0, 1).unwrap();
        let mut context = TickContext::for_tick(&mut a, TickBudget::full());
        for command in [
            Command::SelectHotbar(HotbarSlot::new(5).unwrap()),
            Command::DropSelectedItem,
            Command::TakeCraftingOutput,
            Command::EquipArmor,
            Command::MovePartial(
                PartialMove::try_new(StackView::Container(reference), 0, 1, false).unwrap(),
            ),
            Command::QuickMove(StackSource::try_new(StackView::Container(reference), 0).unwrap()),
            Command::DropStack(StackSource::try_new(StackView::Container(reference), 0).unwrap()),
        ] {
            assert_eq!(
                context.source_player_command_ready(&envelope(s, command)),
                Ok(false)
            );
        }
        for command in [
            Command::CloseContainer,
            Command::Resync(ResyncIntent::try_new(0, ChunkPos::new(0, 0), 0).unwrap()),
            Command::MoveContainer(
                ContainerMove::try_new(ChunkPos::new(0, 0), ContainerKind::Chest, 0, 1, 0, 1)
                    .unwrap(),
            ),
        ] {
            assert_eq!(
                context.source_player_command_ready(&envelope(s, command)),
                Ok(true)
            );
        }
        context.actors.clear();
        assert_eq!(
            context.source_player_command_ready(&envelope(s, Command::DropSelectedItem)),
            Err(ServerError::Internal {
                invariant: "source player registration"
            })
        );
    }
    #[test]
    fn current_acquire_activates_after_pending_admission_and_keeps_scan() {
        let (mut a, s) = fixture();
        offer(&mut a, key(Dimension::DEPTHS, 3, 0), 1);
        let waiting = a.advance_tick(TickBudget::full()).unwrap();
        assert!(!local(&waiting).ready());
        assert!(!local(&waiting).reset());
        let node = &a.source_players.entries[&s] as *const _;
        for packet in [
            mornlea_protocol::ClientPacket::PlayerInput(
                PlayerInput::new(1, 1, 0, false, 1.2, 0.3, false, false, false, false).unwrap(),
            ),
            mornlea_protocol::ClientPacket::SelectHotbar(SelectHotbar::new(2, 5).unwrap()),
        ] {
            a.accept(s, PlayIntent::try_from(packet).unwrap()).unwrap();
        }
        offer(&mut a, key(Dimension::OVERWORLD, 0, 0), 0);
        let p = a.advance_tick(TickBudget::full()).unwrap();
        assert!(local(&p).ready() && local(&p).reset());
        assert_eq!(local(&p).last_input_sequence(), 0);
        assert_eq!(player(&a, s).motion.position().get(), [8.5, 65., 8.5]);
        assert_eq!(player(&a, s).motion.velocity().get(), [0.; 3]);
        assert!(!player(&a, s).motion.on_ground());
        assert_eq!(
            a.residents.inventories[&ActorKey::Player(s)].selected.get(),
            3
        );
        assert_eq!(
            (player(&a, s).look.yaw(), player(&a, s).look.pitch()),
            (0.1, 0.2)
        );
        assert!(!a.residents.runtimes[&ActorKey::Player(s)].reset);
        assert_eq!(&a.source_players.entries[&s] as *const _, node);
        assert!(a.source_players.entries[&s].ever_spawned);
        assert!(
            a.source_players.entries[&s]
                .restore
                .pending_keys()
                .is_empty()
        );
        assert!(format!("{:?}", a.source_players.entries[&s].restore).contains("completed: true"));
        a.advance_tick(TickBudget::full()).unwrap();
    }
    #[test]
    fn solid_current_uses_safe_and_missing_uses_only_anchor() {
        let (mut a, s) = fixture();
        offer(&mut a, key(Dimension::OVERWORLD, 0, 0), 2);
        a.advance_tick(TickBudget::full()).unwrap();
        assert_eq!(player(&a, s).lifecycle, ActorLifecycle::Pending);
        offer(&mut a, key(Dimension::DEPTHS, 3, 0), 1);
        let p = a.advance_tick(TickBudget::full()).unwrap();
        assert!(local(&p).reset());
        assert_eq!(player(&a, s).dimension, Dimension::DEPTHS);
        assert_eq!(player(&a, s).motion.position().get(), [56.5, 64., 8.5]);
        let ActorBody::Player(body) = &player(&a, s).body else {
            panic!("body")
        };
        assert_eq!(body.safe, saved(1).safe);
        let mut a = fresh();
        a.metadata.spawn_dimension = 1;
        a.metadata.spawn_anchor = mornlea_storage::MetadataChunkPos { x: 2, z: 0 };
        a.enable_source_player_restoration(1).unwrap();
        a.enable_live_chunks().unwrap();
        for (x, z) in [(-1, -1), (-1, 0), (0, -1), (0, 0)] {
            offer(&mut a, key(Dimension::OVERWORLD, x, z), 0);
        }
        a.advance_tick(TickBudget::full()).unwrap();
        let s = register(&mut a, 1, None);
        let p = a.advance_tick(TickBudget::full()).unwrap();
        assert!(!local(&p).ready());
        offer(&mut a, key(Dimension::DEPTHS, 2, 0), 1);
        let p = a.advance_tick(TickBudget::full()).unwrap();
        assert!(local(&p).ready() && local(&p).reset());
        assert_eq!(player(&a, s).motion.position().get(), [32.5, 64., 0.5]);
        assert!(player(&a, s).motion.on_ground());
    }
    #[test]
    fn pending_whole_container_move_defers_but_initial_viewer_is_absent() {
        use mornlea_domain::{Command, ContainerKind, ContainerMove, ResyncIntent};
        let (mut a, s) = fixture();
        let movement =
            ContainerMove::try_new(ChunkPos::new(0, 0), ContainerKind::Chest, 0, 1, 0, 1).unwrap();
        let command = envelope(s, Command::MoveContainer(movement));
        {
            let mut context = TickContext::for_tick(&mut a, TickBudget::full());
            assert_eq!(context.source_player_command_ready(&command), Ok(true));
            crate::rules::containers::run(
                &mut context,
                RuleCall {
                    phase: RulePhase::PlayerCommand,
                    actor: None,
                    command: Some(&command),
                    internal: None,
                },
            )
            .unwrap();
            assert_eq!(context.deferred(RulePhase::ContainerMove), vec![command]);
            assert!(context.read().viewer(s).is_none());
        }
        a.accept(
            s,
            PlayIntent::Sequenced {
                sequence: 1,
                command: Command::MoveContainer(movement),
            },
        )
        .unwrap();
        a.accept(
            s,
            PlayIntent::Sequenced {
                sequence: 2,
                command: Command::Resync(ResyncIntent::try_new(0, ChunkPos::new(0, 0), 0).unwrap()),
            },
        )
        .unwrap();
        offer(&mut a, key(Dimension::OVERWORLD, 0, 0), 3);
        let p = a.advance_tick(TickBudget::full()).unwrap();
        assert!(local(&p).ready() && local(&p).reset());
        let view = a.settled_read().unwrap();
        let chest = view.container(movement.container()).unwrap();
        let ContainerSlots::Chest(slots) = chest.slots else {
            panic!("chest")
        };
        assert_eq!(slots[0].count, 3);
        assert_eq!(slots[1].count, 0);
        assert_eq!(
            view.inventory(ActorKey::Player(s)).unwrap().slots[3].count,
            7
        );
        assert!(view.viewer(s).is_none());
        let mut a = fresh();
        a.enable_source_player_restoration(1).unwrap();
        let s = register(&mut a, 1, None);
        // A private stale-view injection pins the existing unconditional close leg.
        a.views.insert(s, ViewLease::new(s, movement.container()));
        a.accept(
            s,
            PlayIntent::Sequenced {
                sequence: 1,
                command: Command::CloseContainer,
            },
        )
        .unwrap();
        a.advance_tick(TickBudget::full()).unwrap();
        assert!(a.views.is_empty());
    }
    #[test]
    fn source_book_is_live_bounded_and_retirement_preserves_durable_history() {
        let mut a = fresh();
        a.enable_source_player_restoration(1).unwrap();
        let sessions: Vec<_> = (1..=8).map(|tag| register(&mut a, tag, None)).collect();
        let next = a.next_session;
        assert!(matches!(
            a.prepare(login(9), TransportKind::Memory),
            Err(ServerError::Capacity {
                resource: Resource::Players,
                ..
            })
        ));
        assert_eq!(a.next_session, next);
        a.retire(sessions[0], CloseReason::PeerGone).unwrap();
        assert_eq!(a.source_players.entries.len(), 7);
        assert_eq!(a.residents.actors.len(), 8);
        assert_eq!(a.residents.runtimes.len(), 8);
        assert_eq!(a.sessions.len(), 8);
        register(&mut a, 9, None);
        assert_eq!(a.source_players.entries.len(), 8);
        assert_eq!(a.residents.actors.len(), 9);
        assert_eq!(a.sessions.len(), 9);
    }
    fn error() -> ServerError {
        ServerError::Capacity {
            resource: Resource::Commands,
            limit: 4096,
            observed: 4097,
        }
    }
    fn partial(context: &mut TickContext<'_>) -> Result<(), ServerError> {
        let old = context
            .read()
            .observation(Dimension::DEPTHS, BlockPos::new(56, 64, 8))
            .unwrap();
        context
            .transaction()
            .try_system(
                SystemRule::Support,
                vec![BlockWrite::try_new(old, 1).unwrap()],
            )
            .unwrap();
        Ok(())
    }
    fn fault(context: &mut TickContext<'_>) -> Result<(), ServerError> {
        partial(context)?;
        Err(error())
    }
    fn unwind(context: &mut TickContext<'_>) -> Result<(), ServerError> {
        partial(context)?;
        panic!("trusted source ownership fault")
    }
    fn moved_book_and_schedules(panic: bool) {
        let (mut a, s) = fixture();
        offer(&mut a, key(Dimension::DEPTHS, 3, 0), 1);
        a.advance_tick(TickBudget::full()).unwrap();
        let entry = &a.source_players.entries[&s];
        let node = entry as *const _;
        let progress = format!("{:?}", entry.restore);
        let keys = entry.restore.pending_keys();
        assert!(!keys.is_empty());
        assert_eq!(player(&a, s).lifecycle, ActorLifecycle::Pending);
        let key = key(Dimension::DEPTHS, 3, 0);
        let pos = BlockPos::new(63, 200, 15);
        a.fluid_schedule.enqueue_fluid(key, pos, 100);
        a.farmland_schedule.enqueue_candidate(key, pos, 100);
        let tick = a.next_tick();
        set_dispatch_hook(Some(if panic { unwind } else { fault }));
        let result = catch_unwind(AssertUnwindSafe(|| a.advance_tick(TickBudget::full())));
        if panic {
            assert_eq!(
                result.unwrap(),
                Err(ServerError::Internal {
                    invariant: "authoritative tick panic"
                })
            );
        } else {
            assert_eq!(result.unwrap(), Err(error()));
        }
        set_dispatch_hook(None);
        assert_eq!(&a.source_players.entries[&s] as *const _, node);
        assert_eq!(
            format!("{:?}", a.source_players.entries[&s].restore),
            progress
        );
        assert_eq!(a.source_players.entries[&s].restore.pending_keys(), keys);
        assert_eq!(a.next_tick(), tick);
        assert_eq!(
            a.residents.ready[&key].block(BlockPos::new(56, 64, 8)),
            Some(1)
        );
        assert!(a.settled_read().is_err());
        assert_eq!(a.phase, ServerPhase::Closing);
        assert_eq!(
            a.advance_tick(TickBudget::full()).unwrap_err(),
            a.tick_failure.unwrap()
        );
        assert_eq!(a.fluid_schedule.pending_fluid(Dimension::DEPTHS), 1);
        assert_eq!(a.farmland_schedule.pending_candidates(Dimension::DEPTHS), 1);
        assert_eq!(
            a.fluid_schedule.fluid_due(Dimension::DEPTHS, pos),
            Some(100)
        );
        assert_eq!(
            a.farmland_schedule.candidate_due(Dimension::DEPTHS, pos),
            Some(100)
        );
    }
    #[test]
    fn moved_book_and_schedules_return_after_error() {
        moved_book_and_schedules(false);
    }
    #[test]
    fn moved_book_and_schedules_return_after_unwind() {
        moved_book_and_schedules(true);
    }
    fn retire_during(context: &mut TickContext<'_>) -> Result<(), ServerError> {
        let s = context
            .read()
            .actors()
            .iter()
            .find_map(|a| {
                if let ActorKey::Player(s) = a.key {
                    Some(s)
                } else {
                    None
                }
            })
            .unwrap();
        context.authority.retire(s, CloseReason::PeerGone)
    }
    #[test]
    fn post_context_retirement_prunes_returned_book() {
        let (mut a, s) = fixture();
        a.advance_tick(TickBudget::full()).unwrap();
        set_dispatch_hook(Some(retire_during));
        a.advance_tick(TickBudget::full()).unwrap();
        set_dispatch_hook(None);
        assert_eq!(a.sessions[&s].phase, SessionPhase::Retired);
        assert!(a.source_players.entries.is_empty());
        assert!(player(&a, s).body == ActorBody::Player(saved(1)));
    }
    #[test]
    fn manual_session_actual_final_restores_once_without_delivery() {
        let (mut a, s) = fixture();
        offer(&mut a, key(Dimension::OVERWORLD, 0, 0), 0);
        let tick = a.next_tick();
        let frames = a.sessions[&s].outbox.len();
        a.begin_close();
        assert_eq!(a.run_final(&mut AuthoritativeFinalReducer).unwrap(), tick);
        assert_eq!(a.next_tick(), tick + 1);
        assert_eq!(a.sessions[&s].outbox.len(), frames);
        assert_eq!(player(&a, s).lifecycle, ActorLifecycle::Active);
        assert!(a.source_players.entries[&s].ever_spawned);
        assert!(format!("{:?}", a.source_players.entries[&s].restore).contains("completed: true"));
        assert!(!a.residents.runtimes[&ActorKey::Player(s)].reset);
        assert_eq!(
            a.run_final(&mut AuthoritativeFinalReducer),
            Err(ServerError::InvalidState {
                phase: ServerPhase::Closing
            })
        );
        assert_eq!(a.next_tick(), tick + 1);
    }
    // Actual acquisition qualifies successful scans; subsequent preparation is off tick.
    fn ctx_fixture(two: bool) -> (AuthorityState, SessionKey, Option<SessionKey>) {
        let (mut a, s) = fixture();
        let other = two.then(|| register(&mut a, 2, Some(saved(2))));
        offer(&mut a, key(Dimension::OVERWORLD, 0, 0), 0);
        a.advance_tick(TickBudget::full()).unwrap();
        assert_eq!(player(&a, s).lifecycle, ActorLifecycle::Active);
        assert!(a.source_players.entries[&s].ever_spawned);
        assert!(
            a.source_players.entries[&s]
                .restore
                .player_reset_anchor()
                .is_ok()
        );
        let slot = a.residents.player_slots[&s];
        let actor = &mut a.residents.actors[slot];
        actor.dimension = Dimension::DEPTHS;
        actor.motion = MotionState::new(mornlea_domain::MotionStateParts {
            position: mornlea_domain::FiniteVec3::try_new([6., 90., 7.]).unwrap(),
            velocity: mornlea_domain::FiniteVec3::try_new([1., 2., 3.]).unwrap(),
            on_ground: true,
        });
        actor.look = mornlea_domain::LookAngles::try_new(0.1, 0.2).unwrap();
        actor.survival = ctx_survival(7, 9, 3);
        let r = a.residents.runtimes.get_mut(&ActorKey::Player(s)).unwrap();
        r.controls = Some(mornlea_domain::PlayerControl::new(
            mornlea_domain::PlayerControlParts {
                movement: mornlea_domain::Movement {
                    move_x: 1,
                    move_z: -1,
                    jump: true,
                },
                look: actor.look,
                actions: mornlea_domain::HeldActions {
                    primary: true,
                    eating: true,
                    sprinting: true,
                    sneaking: true,
                },
            },
        ));
        r.reset = true;
        r.has_view = true;
        r.attack_cooldown = 11;
        r.hurt_cooldown = 12;
        r.burn_cooldown = 13;
        r.oxygen = 3;
        r.peak_y = 90.;
        r.exhaustion_milli = 250;
        r.saturation_milli = 9000;
        r.since_damage_ticks = 17;
        r.drown_ticks = 18;
        r.starvation_ticks = 19;
        r.eating = Some(EatingProgress {
            slot: HotbarSlot::new(2).unwrap(),
            item: 5,
            ticks: 6,
        });
        r.bow = Some(BowProgress {
            slot: HotbarSlot::new(3).unwrap(),
            ticks: 8,
        });
        r.aux = ActorAux::Player {
            respawn: Some((Dimension::DEPTHS, BlockPos::new(-1, 2, 0))),
            workbench: Some(BlockPos::new(2, 3, 4)),
        };
        (a, s, other)
    }
    fn ctx_survival(health: u8, hunger: u8, oxygen: u16) -> mornlea_domain::SurvivalState {
        mornlea_domain::SurvivalState::try_new(mornlea_domain::SurvivalStateParts {
            health,
            oxygen,
            hunger,
            saturation_zero: false,
            armor_points: 4,
        })
        .unwrap()
    }
    fn ctx_expected(c: &TickContext<'_>, s: SessionKey) -> (ActorRecord, ActorRuntime) {
        let mut actor = c.actors[c.player_slots[&s]].clone();
        let mut runtime = c.runtimes[&ActorKey::Player(s)].clone();
        actor.lifecycle = ActorLifecycle::Pending;
        actor.motion = MotionState::new(mornlea_domain::MotionStateParts {
            position: mornlea_domain::FiniteVec3::try_new([32.5, 321., 0.5]).unwrap(),
            velocity: mornlea_domain::FiniteVec3::try_new([0.; 3]).unwrap(),
            on_ground: false,
        });
        actor.survival = ctx_survival(actor.survival.health(), actor.survival.hunger(), 300);
        runtime.controls = None;
        runtime.reset = false;
        runtime.attack_cooldown = 0;
        runtime.hurt_cooldown = 0;
        runtime.oxygen = 300;
        runtime.peak_y = 321.;
        runtime.drown_ticks = 0;
        runtime.eating = None;
        runtime.bow = None;
        (actor, runtime)
    }
    fn ctx_assert_pair(c: &TickContext<'_>, s: SessionKey, want: &(ActorRecord, ActorRuntime)) {
        assert_eq!(
            (
                &c.actors[c.player_slots[&s]],
                &c.runtimes[&ActorKey::Player(s)]
            ),
            (&want.0, &want.1)
        );
    }
    fn ctx_seed_transients(c: &mut TickContext<'_>, sessions: &[SessionKey]) {
        for &s in sessions {
            let actor = ActorKey::Player(s);
            c.mining.insert(
                actor,
                MiningProgress {
                    actor,
                    dimension: Dimension::DEPTHS,
                    target: BlockPos::new(2, 3, 4),
                    observed_block: 1,
                    tool_slot: HotbarSlot::new(3).unwrap(),
                    tool: mornlea_storage::ItemStack::default(),
                    elapsed: 2,
                    required: 9,
                    last_tick: 7,
                },
            );
            c.sleeping.insert(s);
            c.suppressed_mining.insert(actor);
        }
    }
    fn ctx_snapshot(c: &TickContext<'_>) -> String {
        format!(
            "{:?}",
            (
                (&c.actors, &c.player_slots, &c.runtimes, &c.inventories),
                (
                    &c.mining,
                    &c.sleeping,
                    &c.suppressed_mining,
                    &c.sleep_record,
                    c.sleep_record_touched
                ),
                (
                    &c.viewers,
                    &c.charges,
                    &c.pre_step,
                    &c.damage_intents,
                    &c.events
                ),
                (
                    c.spent_commands,
                    c.spent_fluid,
                    c.spent_rescan,
                    c.spent_farmland_checks,
                    c.spent_farmland_reads,
                    c.spent_effects,
                    c.spent_snapshot_chunks,
                    c.spent_snapshot_bytes
                ),
                (
                    c.authority
                        .sessions
                        .iter()
                        .map(|(key, record)| (
                            *key,
                            record.player_id,
                            &record.display_name,
                            record.phase,
                            record.last_applied_sequence,
                            record.last_input_sequence,
                            record.next_arrival,
                            &record.body,
                            record.loaded_current,
                            &record.outbox,
                            record.outbox_closed
                        ))
                        .collect::<Vec<_>>(),
                    c.authority.source_player_radius,
                    c.authority.phase,
                    &c.authority.tick_failure
                ),
            )
        )
    }
    fn ctx_refuse(
        c: &mut TickContext<'_>,
        s: SessionKey,
        scan: &super::super::pending_restore::PendingRestore,
        error: ServerError,
    ) {
        let before = ctx_snapshot(c);
        let scan_before = format!("{scan:?}");
        assert_eq!(c.begin_source_player_reset(s, scan), Err(error));
        assert_eq!(ctx_snapshot(c), before);
        assert_eq!(format!("{scan:?}"), scan_before);
    }
    fn ctx_error() -> ServerError {
        ServerError::InvalidInput {
            field: "source_player_reset",
        }
    }
    #[test]
    fn ctx_reset_recovery_preserves_durable_owners() {
        let (mut a, s, _) = ctx_fixture(false);
        let book = std::mem::take(&mut a.source_players);
        let mut c = TickContext::for_tick(&mut a, TickBudget::full());
        let want = ctx_expected(&c, s);
        let inventories = c.inventories.clone();
        c.begin_source_player_reset(s, &book.entries[&s].restore)
            .unwrap();
        ctx_assert_pair(&c, s, &want);
        assert_eq!(c.inventories, inventories);
    }
    #[test]
    fn ctx_reset_death_prepared_pair_reuses_mapping() {
        let (mut a, s, _) = ctx_fixture(false);
        // Prepared death caller fills survival before the common context operation.
        a.residents.actors[a.residents.player_slots[&s]].survival = ctx_survival(20, 20, 3);
        let r = a.residents.runtimes.get_mut(&ActorKey::Player(s)).unwrap();
        r.saturation_milli = 5000;
        r.exhaustion_milli = 0;
        r.since_damage_ticks = 0;
        r.starvation_ticks = 0;
        let book = std::mem::take(&mut a.source_players);
        let mut c = TickContext::for_tick(&mut a, TickBudget::full());
        let want = ctx_expected(&c, s);
        let inventories = c.inventories.clone();
        c.begin_source_player_reset(s, &book.entries[&s].restore)
            .unwrap();
        ctx_assert_pair(&c, s, &want);
        assert_eq!(c.inventories, inventories);
    }
    #[test]
    fn ctx_reset_clears_only_owned_transients() {
        let (mut a, s, other) = ctx_fixture(true);
        let other = other.unwrap();
        let book = std::mem::take(&mut a.source_players);
        let mut c = TickContext::for_tick(&mut a, TickBudget::full());
        ctx_seed_transients(&mut c, &[s, other]);
        c.sleep_record = SleepState::try_new(
            vec![
                (s, Dimension::DEPTHS, BlockPos::new(-1, 2, 0)),
                (other, Dimension::OVERWORLD, BlockPos::new(2, 3, 4)),
            ],
            55,
            Some(66),
        )
        .unwrap();
        let reference = ContainerRef::try_new(
            ChunkPos::new(0, 0),
            mornlea_domain::ContainerKind::Chest,
            0,
            1,
        )
        .unwrap();
        c.viewers.insert(s, ViewLease::new(s, reference));
        c.viewers.insert(other, ViewLease::new(other, reference));
        let want = ctx_expected(&c, s);
        let before = (
            c.actors.clone(),
            c.runtimes.clone(),
            c.inventories.clone(),
            c.mining.clone(),
            c.sleeping.clone(),
            c.suppressed_mining.clone(),
            c.sleep_record.clone(),
            c.sleep_record_touched,
            c.viewers.clone(),
        );
        c.begin_source_player_reset(s, &book.entries[&s].restore)
            .unwrap();
        ctx_assert_pair(&c, s, &want);
        let mut expected_actors = before.0;
        expected_actors[c.player_slots[&s]] = want.0;
        let mut expected_runtimes = before.1;
        expected_runtimes.insert(ActorKey::Player(s), want.1);
        let mut mining = before.3;
        mining.remove(&ActorKey::Player(s));
        let mut sleeping = before.4;
        sleeping.remove(&s);
        let mut suppressed = before.5;
        suppressed.remove(&ActorKey::Player(s));
        assert_eq!(
            (
                c.actors.clone(),
                c.runtimes.clone(),
                c.inventories.clone(),
                c.mining.clone(),
                c.sleeping.clone(),
                c.suppressed_mining.clone(),
                c.sleep_record.clone(),
                c.sleep_record_touched,
                c.viewers.clone()
            ),
            (
                expected_actors,
                expected_runtimes,
                before.2,
                mining,
                sleeping,
                suppressed,
                before.6,
                before.7,
                before.8
            )
        );
    }
    #[test]
    fn ctx_reset_preserves_earned_charges_and_input_ack() {
        let (mut a, s, other) = ctx_fixture(true);
        let other = other.unwrap();
        a.sessions.get_mut(&s).unwrap().last_input_sequence = 3;
        let book = std::mem::take(&mut a.source_players);
        let mut c = TickContext::for_tick(&mut a, TickBudget::full());
        // Prepared receipts do not qualify actual mining or late action producers.
        c.charges = vec![
            (ActorKey::Player(s), ActionKind::Till),
            (ActorKey::Player(s), ActionKind::Mining),
            (ActorKey::Player(other), ActionKind::Mining),
            (ActorKey::Player(s), ActionKind::Melee),
            (ActorKey::Player(other), ActionKind::Melee),
        ];
        c.damage_intents.push(DamageIntent {
            source: ActorKey::Player(other),
            target: ActorKey::Player(s),
            dimension: Dimension::DEPTHS,
            amount: 2,
            cause: DamageCause::Melee,
            projectile: None,
            tick: 7,
        });
        let want = ctx_expected(&c, s);
        let before = (
            c.charges.clone(),
            c.pre_step.clone(),
            c.damage_intents.clone(),
            c.events.clone(),
            c.spent_commands,
            c.spent_fluid,
            c.spent_rescan,
            c.spent_farmland_checks,
            c.spent_farmland_reads,
            c.spent_effects,
            c.spent_snapshot_chunks,
            c.spent_snapshot_bytes,
        );
        c.begin_source_player_reset(s, &book.entries[&s].restore)
            .unwrap();
        ctx_assert_pair(&c, s, &want);
        assert_eq!(c.authority.sessions[&s].last_input_sequence, 3);
        assert_eq!(
            (
                c.charges.clone(),
                c.pre_step.clone(),
                c.damage_intents.clone(),
                c.events.clone(),
                c.spent_commands,
                c.spent_fluid,
                c.spent_rescan,
                c.spent_farmland_checks,
                c.spent_farmland_reads,
                c.spent_effects,
                c.spent_snapshot_chunks,
                c.spent_snapshot_bytes
            ),
            before
        );
    }
    fn ctx_add_allocations(a: &mut AuthorityState, s: SessionKey) {
        let ActorBody::Player(body) = &mut a.residents.actors[a.residents.player_slots[&s]].body
        else {
            unreachable!()
        };
        let mut name = String::with_capacity(65536);
        name.push_str("Ada");
        body.display_name = name;
        let mut revisions = Vec::with_capacity(65536);
        revisions.extend((0..9).map(|n| (key(Dimension::DEPTHS, n, -n), n as u64)));
        let mut waypoints = Vec::with_capacity(65536);
        waypoints.extend((0..17).map(|n| BlockPos::new(n, 64, -n)));
        a.residents
            .runtimes
            .get_mut(&ActorKey::Player(s))
            .unwrap()
            .path = Some(PathState {
            generation: 7,
            target: BlockPos::new(1, 2, 3),
            revisions,
            waypoints,
            cursor: 2,
            next_repath_tick: 900,
        });
    }
    fn ctx_allocations(
        actor: &ActorRecord,
        r: &ActorRuntime,
    ) -> (
        usize,
        usize,
        usize,
        usize,
        usize,
        usize,
        usize,
        usize,
        usize,
    ) {
        let ActorBody::Player(body) = &actor.body else {
            unreachable!()
        };
        let p = r.path.as_ref().unwrap();
        (
            body.display_name.as_ptr() as usize,
            body.display_name.len(),
            body.display_name.capacity(),
            p.revisions.as_ptr() as usize,
            p.revisions.len(),
            p.revisions.capacity(),
            p.waypoints.as_ptr() as usize,
            p.waypoints.len(),
            p.waypoints.capacity(),
        )
    }
    fn ctx_book_snapshot(book: &SourcePlayerBook, s: SessionKey) -> String {
        let entry = &book.entries[&s];
        format!(
            "{:p}/{:?}/{:?}/{:?}",
            entry,
            entry.restore.pending_keys(),
            entry.restore,
            entry.ever_spawned
        )
    }
    #[test]
    fn ctx_reset_preserves_public_allocations_and_book_scan() {
        let (mut a, s, _) = ctx_fixture(false);
        ctx_add_allocations(&mut a, s);
        let book = std::mem::take(&mut a.source_players);
        let mut c = TickContext::for_tick(&mut a, TickBudget::full());
        let want = ctx_expected(&c, s);
        let allocations = ctx_allocations(
            &c.actors[c.player_slots[&s]],
            &c.runtimes[&ActorKey::Player(s)],
        );
        let scan = ctx_book_snapshot(&book, s);
        c.begin_source_player_reset(s, &book.entries[&s].restore)
            .unwrap();
        ctx_assert_pair(&c, s, &want);
        assert_eq!(
            ctx_allocations(
                &c.actors[c.player_slots[&s]],
                &c.runtimes[&ActorKey::Player(s)]
            ),
            allocations
        );
        assert_eq!(ctx_book_snapshot(&book, s), scan);
    }
    #[test]
    fn ctx_reset_rejects_disabled_or_retired_owner() {
        for case in 0..3 {
            let (mut a, s, other) = ctx_fixture(true);
            let other = other.unwrap();
            match case {
                0 => a.source_player_radius = None,
                1 => {
                    a.sessions.remove(&s);
                }
                2 => a.sessions.get_mut(&s).unwrap().phase = SessionPhase::Retired,
                _ => unreachable!(),
            }
            let book = std::mem::take(&mut a.source_players);
            let before_book = ctx_book_snapshot(&book, s);
            let mut c = TickContext::for_tick(&mut a, TickBudget::full());
            ctx_seed_transients(&mut c, &[s, other]);
            ctx_refuse(&mut c, s, &book.entries[&s].restore, ctx_error());
            assert_eq!(ctx_book_snapshot(&book, s), before_book);
        }
    }
    #[test]
    fn ctx_reset_rejects_missing_or_mismatched_pair() {
        for case in 0..10 {
            let (mut a, s, other) = ctx_fixture(true);
            let other = other.unwrap();
            let book = std::mem::take(&mut a.source_players);
            let mut c = TickContext::for_tick(&mut a, TickBudget::full());
            ctx_seed_transients(&mut c, &[s, other]);
            let slot = c.player_slots[&s];
            match case {
                0 => {
                    c.player_slots.remove(&s);
                }
                1 => {
                    c.player_slots.insert(s, c.actors.len());
                }
                2 => c.actors[slot].key = ActorKey::Player(other),
                3 => {
                    c.runtimes.remove(&ActorKey::Player(s));
                }
                4 => {
                    c.runtimes.get_mut(&ActorKey::Player(s)).unwrap().key = ActorKey::Player(other)
                }
                5 => {
                    c.actors[slot].body = ActorBody::Passive(mornlea_storage::PassiveMob {
                        id: 1,
                        dimension: 0,
                        position: [0., 64., 0.],
                        velocity: [0.; 3],
                        on_ground: true,
                        yaw: 0.,
                        health: 7,
                    })
                }
                6 => {
                    c.runtimes.get_mut(&ActorKey::Player(s)).unwrap().aux = ActorAux::Hostile {
                        distant_ticks: 1,
                        shoot_cooldown: 2,
                        fresh: true,
                    }
                }
                7 => c.actors[slot].lifecycle = ActorLifecycle::Pending,
                8 => c.actors[slot].lifecycle = ActorLifecycle::Respawning,
                9 => c.actors[slot].lifecycle = ActorLifecycle::Dead,
                _ => unreachable!(),
            }
            let incomplete = ctx_scan(super::super::pending_restore::RestoreKind::Player, false);
            let scan = if case == 4 {
                &incomplete
            } else {
                &book.entries[&s].restore
            };
            let before_book = ctx_book_snapshot(&book, s);
            ctx_refuse(&mut c, s, scan, ctx_error());
            assert_eq!(ctx_book_snapshot(&book, s), before_book);
        }
    }
    // AIR/Ready is used only to construct scan-kind guard doubles.
    struct ContextAirReady;
    impl super::super::actor_placement::PlacementWorld for ContextAirReady {
        fn ready_revision(&self, k: ChunkKey) -> Option<u64> {
            (k == key(Dimension::OVERWORLD, 0, 0)).then_some(9)
        }
        fn block_at(&self, dimension: Dimension, pos: BlockPos) -> Option<u16> {
            self.ready_revision(key(dimension, pos.x() >> 4, pos.z() >> 4))?;
            Some(0)
        }
    }
    fn ctx_scan(
        kind: super::super::pending_restore::RestoreKind,
        complete: bool,
    ) -> super::super::pending_restore::PendingRestore {
        use super::super::pending_restore::{PendingRestore, RestoreKind, RestoreProgress};
        let mut scan = PendingRestore::try_new(
            kind,
            Dimension::DEPTHS,
            ChunkPos::new(2, 0),
            if kind == RestoreKind::Companion {
                16
            } else {
                1
            },
            vec![super::super::actor_placement::RestoreCandidate {
                dimension: Dimension::OVERWORLD,
                position: [8.5, 65., 8.5],
                require_support: false,
            }],
        )
        .unwrap();
        if complete {
            assert!(matches!(
                scan.advance(&ContextAirReady, 1.62).unwrap(),
                RestoreProgress::Activated(_)
            ));
        }
        scan
    }
    #[test]
    fn ctx_reset_rejects_incomplete_or_companion_scan() {
        use super::super::pending_restore::RestoreKind;
        for scan in [
            ctx_scan(RestoreKind::Player, false),
            ctx_scan(RestoreKind::Companion, true),
        ] {
            let (mut a, s, other) = ctx_fixture(true);
            let book = std::mem::take(&mut a.source_players);
            let before_book = ctx_book_snapshot(&book, s);
            let mut c = TickContext::for_tick(&mut a, TickBudget::full());
            ctx_seed_transients(&mut c, &[s, other.unwrap()]);
            ctx_refuse(
                &mut c,
                s,
                &scan,
                ServerError::InvalidInput {
                    field: "restore_restart",
                },
            );
            assert_eq!(ctx_book_snapshot(&book, s), before_book);
        }
    }
    #[test]
    fn ctx_reset_health_and_closed_guard_precedence() {
        for case in 0..4 {
            let (mut a, s, _) = ctx_fixture(false);
            let failure = ServerError::Internal {
                invariant: "retained context failure",
            };
            match case {
                0 => {
                    a.tick_failure = Some(failure);
                    a.phase = ServerPhase::Closed;
                }
                1 => {
                    a.tick_failure = Some(failure);
                    a.source_player_radius = None;
                }
                2 => {
                    a.phase = ServerPhase::Closed;
                    a.residents.player_slots.remove(&s);
                }
                3 => a.phase = ServerPhase::Closing,
                _ => unreachable!(),
            }
            let book = std::mem::take(&mut a.source_players);
            let before_book = ctx_book_snapshot(&book, s);
            let mut c = TickContext::for_tick(&mut a, TickBudget::full());
            ctx_seed_transients(&mut c, &[s]);
            if case == 3 {
                let want = ctx_expected(&c, s);
                c.begin_source_player_reset(s, &book.entries[&s].restore)
                    .unwrap();
                ctx_assert_pair(&c, s, &want);
                assert!(!c.mining.contains_key(&ActorKey::Player(s)));
                assert!(!c.sleeping.contains(&s));
                assert!(!c.suppressed_mining.contains(&ActorKey::Player(s)));
            } else {
                ctx_refuse(
                    &mut c,
                    s,
                    &book.entries[&s].restore,
                    if case < 2 {
                        failure
                    } else {
                        ServerError::InvalidState {
                            phase: ServerPhase::Closed,
                        }
                    },
                );
            }
            assert_eq!(ctx_book_snapshot(&book, s), before_book);
        }
    }
    #[test]
    fn ctx_reset_partial_mutation_returns_after_context_drop() {
        let (mut a, s, other) = ctx_fixture(true);
        let other = other.unwrap();
        ctx_add_allocations(&mut a, s);
        let book = std::mem::take(&mut a.source_players);
        let book_before = ctx_book_snapshot(&book, s);
        let allocations =
            ctx_allocations(player(&a, s), &a.residents.runtimes[&ActorKey::Player(s)]);
        let other_pair = (
            player(&a, other).clone(),
            a.residents.runtimes[&ActorKey::Player(other)].clone(),
        );
        let inventories = a.residents.inventories.clone();
        let want;
        let other_mining;
        {
            let mut c = TickContext::for_tick(&mut a, TickBudget::full());
            ctx_seed_transients(&mut c, &[s, other]);
            other_mining = c.mining[&ActorKey::Player(other)].clone();
            want = ctx_expected(&c, s);
            c.begin_source_player_reset(s, &book.entries[&s].restore)
                .unwrap();
            // Deliberately drop the resident loan without commit or publication.
        }
        a.source_players = book;
        assert_eq!(
            (player(&a, s), &a.residents.runtimes[&ActorKey::Player(s)]),
            (&want.0, &want.1)
        );
        assert_eq!(
            (
                player(&a, other),
                &a.residents.runtimes[&ActorKey::Player(other)]
            ),
            (&other_pair.0, &other_pair.1)
        );
        assert!(!a.residents.mining.contains_key(&ActorKey::Player(s)));
        assert!(!a.residents.sleeping.contains(&s));
        assert_eq!(a.residents.mining[&ActorKey::Player(other)], other_mining);
        assert!(a.residents.sleeping.contains(&other));
        assert_eq!(a.residents.inventories, inventories);
        assert_eq!(
            ctx_allocations(player(&a, s), &a.residents.runtimes[&ActorKey::Player(s)]),
            allocations
        );
        assert_eq!(ctx_book_snapshot(&a.source_players, s), book_before);
    }
    // These checked preparations are off tick; Ready AIR came from the actual fixture.
    fn ctx_recovery_pose(
        a: &mut AuthorityState,
        s: SessionKey,
        dimension: Dimension,
        position: [f32; 3],
    ) {
        let actor = &mut a.residents.actors[a.residents.player_slots[&s]];
        actor.dimension = dimension;
        actor.motion = MotionState::new(mornlea_domain::MotionStateParts {
            position: mornlea_domain::FiniteVec3::try_new(position).unwrap(),
            velocity: actor.motion.velocity(),
            on_ground: actor.motion.on_ground(),
        });
        a.residents
            .runtimes
            .get_mut(&ActorKey::Player(s))
            .unwrap()
            .reset = false;
    }
    fn ctx_recovery_stone(c: &mut TickContext<'_>, x: i32, ys: std::ops::RangeInclusive<i32>) {
        for y in ys {
            let observed = c
                .read()
                .observation(Dimension::OVERWORLD, BlockPos::new(x, y, 8))
                .unwrap();
            assert_eq!(observed.block, 0);
            c.transaction()
                .try_system(
                    SystemRule::Support,
                    vec![BlockWrite::try_new(observed, 2).unwrap()],
                )
                .unwrap();
        }
    }
    fn ctx_recovery_restart(book: &SourcePlayerBook, s: SessionKey, dimension: Dimension) {
        let entry = &book.entries[&s];
        assert!(entry.ever_spawned);
        assert_eq!(
            entry.restore.player_reset_anchor(),
            Err(ServerError::InvalidInput {
                field: "restore_restart"
            })
        );
        assert!(entry.restore.pending_keys().contains(&key(dimension, 2, 0)));
    }
    #[test]
    fn ctx_recover_below_world_restarts_same_captured_scan() {
        let (mut a, s, other) = ctx_fixture(true);
        let other = other.unwrap();
        ctx_add_allocations(&mut a, s);
        ctx_recovery_pose(&mut a, s, Dimension::DEPTHS, [6., -80.0625, 7.]);
        a.sessions.get_mut(&s).unwrap().last_input_sequence = 3;
        let mut book = std::mem::take(&mut a.source_players);
        let allocations =
            ctx_allocations(player(&a, s), &a.residents.runtimes[&ActorKey::Player(s)]);
        let want;
        {
            let mut c = TickContext::for_tick(&mut a, TickBudget::full());
            ctx_seed_transients(&mut c, &[s, other]);
            c.charges.push((ActorKey::Player(s), ActionKind::Mining));
            let retained = (
                c.pre_step.clone(),
                c.inventories.clone(),
                c.charges.clone(),
                c.viewers.clone(),
            );
            let other_pair = (
                c.actors[c.player_slots[&other]].clone(),
                c.runtimes[&ActorKey::Player(other)].clone(),
            );
            want = ctx_expected(&c, s);
            assert_eq!(
                super::super::source_player_restore::recover(&mut book, &mut c, s),
                Ok(true)
            );
            ctx_assert_pair(&c, s, &want);
            assert_eq!(
                (
                    c.pre_step.clone(),
                    c.inventories.clone(),
                    c.charges.clone(),
                    c.viewers.clone()
                ),
                retained
            );
            ctx_assert_pair(&c, other, &other_pair);
            assert!(
                !c.mining.contains_key(&ActorKey::Player(s))
                    && !c.sleeping.contains(&s)
                    && !c.suppressed_mining.contains(&ActorKey::Player(s))
            );
            assert!(
                c.mining.contains_key(&ActorKey::Player(other))
                    && c.sleeping.contains(&other)
                    && c.suppressed_mining.contains(&ActorKey::Player(other))
            );
            assert_eq!(c.authority.sessions[&s].last_input_sequence, 3);
            ctx_recovery_restart(&book, s, Dimension::DEPTHS);
            // Deliberately return mapped residents through Drop without commit.
        }
        a.source_players = book;
        assert_eq!(
            (player(&a, s), &a.residents.runtimes[&ActorKey::Player(s)]),
            (&want.0, &want.1)
        );
        assert_eq!(
            ctx_allocations(player(&a, s), &a.residents.runtimes[&ActorKey::Player(s)]),
            allocations
        );
        ctx_recovery_restart(&a.source_players, s, Dimension::DEPTHS);
        let (mut a, s, _) = ctx_fixture(false);
        ctx_recovery_pose(&mut a, s, Dimension::OVERWORLD, [8.5, -80., 8.5]);
        let mut book = std::mem::take(&mut a.source_players);
        let before_book = ctx_book_snapshot(&book, s);
        let mut c = TickContext::for_tick(&mut a, TickBudget::full());
        let before = ctx_snapshot(&c);
        assert_eq!(
            super::super::source_player_restore::recover(&mut book, &mut c, s),
            Ok(false)
        );
        assert_eq!(ctx_snapshot(&c), before);
        assert_eq!(ctx_book_snapshot(&book, s), before_book);
    }
    #[test]
    fn ctx_recover_lifts_first_free_sixteenth_and_rebases_pre_step() {
        for y in [64.75, 64.] {
            let (mut a, s, _) = ctx_fixture(false);
            ctx_add_allocations(&mut a, s);
            ctx_recovery_pose(&mut a, s, Dimension::OVERWORLD, [8.5, y, 8.5]);
            let mut book = std::mem::take(&mut a.source_players);
            let before_book = ctx_book_snapshot(&book, s);
            let mut c = TickContext::for_tick(&mut a, TickBudget::full());
            ctx_recovery_stone(&mut c, 8, 64..=64);
            ctx_seed_transients(&mut c, &[s]);
            let slot = c.player_slots[&s];
            let allocations = ctx_allocations(&c.actors[slot], &c.runtimes[&ActorKey::Player(s)]);
            let mut want = (
                c.actors[slot].clone(),
                c.runtimes[&ActorKey::Player(s)].clone(),
            );
            want.0.motion = MotionState::new(mornlea_domain::MotionStateParts {
                position: mornlea_domain::FiniteVec3::try_new([8.5, 65., 8.5]).unwrap(),
                velocity: want.0.motion.velocity(),
                on_ground: want.0.motion.on_ground(),
            });
            let before = ctx_snapshot(&c);
            assert_eq!(
                super::super::source_player_restore::recover(&mut book, &mut c, s),
                Ok(false)
            );
            ctx_assert_pair(&c, s, &want);
            assert_eq!(c.pre_step[&ActorKey::Player(s)], want.0.motion);
            assert_eq!(
                ctx_allocations(&c.actors[slot], &c.runtimes[&ActorKey::Player(s)]),
                allocations
            );
            assert_eq!(ctx_book_snapshot(&book, s), before_book);
            // Restore only the two expected differences to compare every other owned value.
            c.actors[slot].motion = MotionState::new(mornlea_domain::MotionStateParts {
                position: mornlea_domain::FiniteVec3::try_new([8.5, y, 8.5]).unwrap(),
                velocity: want.0.motion.velocity(),
                on_ground: want.0.motion.on_ground(),
            });
            *c.pre_step.get_mut(&ActorKey::Player(s)).unwrap() = c.actors[slot].motion;
            assert_eq!(ctx_snapshot(&c), before);
        }
    }
    #[test]
    fn ctx_recover_all_sixteenths_blocked_enter_pending() {
        let (mut a, s, _) = ctx_fixture(false);
        ctx_recovery_pose(&mut a, s, Dimension::OVERWORLD, [8.5, 64.75, 8.5]);
        let mut book = std::mem::take(&mut a.source_players);
        let mut c = TickContext::for_tick(&mut a, TickBudget::full());
        ctx_recovery_stone(&mut c, 8, 64..=67);
        let want = ctx_expected(&c, s);
        assert_eq!(
            super::super::source_player_restore::recover(&mut book, &mut c, s),
            Ok(true)
        );
        ctx_assert_pair(&c, s, &want);
        ctx_recovery_restart(&book, s, Dimension::OVERWORLD);
    }
    #[test]
    fn ctx_recover_unknown_stops_initial_and_later_attempts() {
        for blocked in [false, true] {
            let (mut a, s, _) = ctx_fixture(false);
            ctx_recovery_pose(&mut a, s, Dimension::OVERWORLD, [15.85, 64.75, 8.5]);
            let mut book = std::mem::take(&mut a.source_players);
            let mut c = TickContext::for_tick(&mut a, TickBudget::full());
            if blocked {
                ctx_recovery_stone(&mut c, 15, 64..=64);
            }
            let space = super::super::actor_placement::body_space(
                &c.read(),
                Dimension::OVERWORLD,
                [15.85, 64.75, 8.5],
            )
            .unwrap();
            assert_eq!(space.ready, blocked);
            assert!(!space.free);
            let want = ctx_expected(&c, s);
            assert_eq!(
                super::super::source_player_restore::recover(&mut book, &mut c, s),
                Ok(true)
            );
            ctx_assert_pair(&c, s, &want);
            ctx_recovery_restart(&book, s, Dimension::OVERWORLD);
        }
    }
    #[test]
    fn ctx_recover_free_inactive_and_absent_book_are_quiet() {
        for case in 0..8 {
            let (mut a, s, _) = ctx_fixture(false);
            ctx_recovery_pose(&mut a, s, Dimension::OVERWORLD, [8.5, 65., 8.5]);
            match case {
                1 => {
                    a.residents.actors[a.residents.player_slots[&s]].lifecycle =
                        ActorLifecycle::Pending
                }
                2 => {
                    a.residents.actors[a.residents.player_slots[&s]].lifecycle =
                        ActorLifecycle::Respawning
                }
                3 => {
                    a.residents.actors[a.residents.player_slots[&s]].lifecycle =
                        ActorLifecycle::Dead
                }
                4 => {
                    a.source_players.entries.remove(&s);
                }
                5 => a.sessions.get_mut(&s).unwrap().phase = SessionPhase::Retired,
                6 => {
                    a.source_player_radius = None;
                    a.source_players.entries.clear();
                }
                7 => {
                    a.sessions.get_mut(&s).unwrap().phase = SessionPhase::Retired;
                    a.residents.player_slots.remove(&s);
                }
                _ => {}
            }
            let mut book = std::mem::take(&mut a.source_players);
            let before_book = format!(
                "{:?}",
                book.entries
                    .iter()
                    .map(|(s, e)| (*s, format!("{:?}", e.restore), e.ever_spawned))
                    .collect::<Vec<_>>()
            );
            let mut c = TickContext::for_tick(&mut a, TickBudget::full());
            let before = ctx_snapshot(&c);
            assert_eq!(
                super::super::source_player_restore::recover(&mut book, &mut c, s),
                Ok(false)
            );
            assert_eq!(ctx_snapshot(&c), before);
            assert_eq!(
                format!(
                    "{:?}",
                    book.entries
                        .iter()
                        .map(|(s, e)| (*s, format!("{:?}", e.restore), e.ever_spawned))
                        .collect::<Vec<_>>()
                ),
                before_book
            );
        }
    }
    #[test]
    fn ctx_recover_refusals_preserve_owned_state() {
        for case in 0..14 {
            let (mut a, s, other) = ctx_fixture(true);
            let other = other.unwrap();
            ctx_recovery_pose(&mut a, s, Dimension::OVERWORLD, [8.5, 64.75, 8.5]);
            let failure = ServerError::Internal {
                invariant: "retained context failure",
            };
            if case == 0 {
                a.tick_failure = Some(failure);
                a.phase = ServerPhase::Closed;
            }
            if case == 1 {
                a.phase = ServerPhase::Closed;
            }
            if case == 2 {
                a.source_player_radius = None;
            }
            let mut book = std::mem::take(&mut a.source_players);
            let mut c = TickContext::for_tick(&mut a, TickBudget::full());
            ctx_seed_transients(&mut c, &[s, other]);
            let slot = c.player_slots[&s];
            match case {
                3 => {
                    c.player_slots.remove(&s);
                }
                4 => {
                    c.player_slots.insert(s, c.actors.len());
                }
                5 => c.actors[slot].key = ActorKey::Player(other),
                7 => {
                    c.runtimes.remove(&ActorKey::Player(s));
                }
                8 => {
                    c.runtimes.get_mut(&ActorKey::Player(s)).unwrap().key = ActorKey::Player(other);
                    book.entries.get_mut(&s).unwrap().restore =
                        ctx_scan(super::super::pending_restore::RestoreKind::Player, false);
                }
                9 => {
                    c.runtimes.get_mut(&ActorKey::Player(s)).unwrap().aux = ActorAux::Hostile {
                        distant_ticks: 1,
                        shoot_cooldown: 2,
                        fresh: true,
                    }
                }
                10 => {
                    book.entries.get_mut(&s).unwrap().restore =
                        ctx_scan(super::super::pending_restore::RestoreKind::Player, false)
                }
                11 => book.entries.get_mut(&s).unwrap().ever_spawned = false,
                12 => {
                    c.actors[slot].motion = MotionState::new(mornlea_domain::MotionStateParts {
                        position: mornlea_domain::FiniteVec3::try_new([1e30, 65., 8.5]).unwrap(),
                        velocity: c.actors[slot].motion.velocity(),
                        on_ground: true,
                    })
                }
                13 => {
                    ctx_recovery_stone(&mut c, 8, 64..=64);
                    c.pre_step.remove(&ActorKey::Player(s));
                }
                _ => {}
            }
            if case == 6 {
                c.actors[slot].body = ActorBody::Passive(mornlea_storage::PassiveMob {
                    id: 1,
                    dimension: 0,
                    position: [0., 64., 0.],
                    velocity: [0.; 3],
                    on_ground: true,
                    yaw: 0.,
                    health: 7,
                });
            }
            let expected = match case {
                0 => failure,
                1 => ServerError::InvalidState {
                    phase: ServerPhase::Closed,
                },
                10 => ServerError::InvalidInput {
                    field: "restore_restart",
                },
                11 => ServerError::Internal {
                    invariant: "source player registration",
                },
                12 => ServerError::InvalidInput {
                    field: "actor_geometry",
                },
                13 => ServerError::Internal {
                    invariant: "source player recovery",
                },
                _ => ctx_error(),
            };
            let before = ctx_snapshot(&c);
            let before_book = ctx_book_snapshot(&book, s);
            assert_eq!(
                super::super::source_player_restore::recover(&mut book, &mut c, s),
                Err(expected),
                "case {case}"
            );
            assert_eq!(ctx_snapshot(&c), before, "case {case}");
            assert_eq!(ctx_book_snapshot(&book, s), before_book, "case {case}");
        }
    }
    fn ctx_death_snapshot(c: &TickContext<'_>) -> String {
        format!(
            "{}/{:?}",
            ctx_snapshot(c),
            (
                c.drops
                    .iter()
                    .map(|(k, v)| (*k, v.slots, v.dirty, v.records()))
                    .collect::<Vec<_>>(),
                &c.dirty_chunks,
                c.ready
                    .iter()
                    .map(|(k, v)| (*k, v.key, v.generation, v.revision))
                    .collect::<Vec<_>>(),
                &c.environment,
            )
        )
    }
    fn ctx_death_zero(a: &mut AuthorityState, s: SessionKey) {
        a.residents.actors[a.residents.player_slots[&s]].survival = ctx_survival(0, 9, 3);
    }
    fn ctx_death_expected(c: &TickContext<'_>, s: SessionKey) -> (ActorRecord, ActorRuntime) {
        let mut want = ctx_expected(c, s);
        want.0.survival =
            mornlea_domain::SurvivalState::try_new(mornlea_domain::SurvivalStateParts {
                health: 20,
                hunger: 20,
                oxygen: 300,
                saturation_zero: false,
                armor_points: crate::rules::inventory::armor_points(
                    &c.inventories[&ActorKey::Player(s)].armor,
                ),
            })
            .unwrap();
        want.1.saturation_milli = 5000;
        want.1.exhaustion_milli = 0;
        want.1.since_damage_ticks = 0;
        want.1.starvation_ticks = 0;
        want
    }
    fn ctx_death_result(
        dimension: Dimension,
        candidate: Option<super::super::actor_placement::RestoreCandidate>,
    ) -> Option<super::super::source_player_death::SourceDeathReset> {
        Some(super::super::source_player_death::SourceDeathReset {
            dimension,
            candidate,
        })
    }
    fn ctx_death_refuse(
        c: &mut TickContext<'_>,
        s: SessionKey,
        scan: &super::super::pending_restore::PendingRestore,
        error: ServerError,
    ) {
        let before = ctx_death_snapshot(c);
        let scan_before = format!("{scan:?}");
        assert_eq!(c.settle_source_player_death(s, scan), Err(error));
        assert_eq!(ctx_death_snapshot(c), before);
        assert_eq!(format!("{scan:?}"), scan_before);
    }
    fn ctx_death_stack(count: u8) -> mornlea_storage::ItemStack {
        mornlea_storage::ItemStack {
            item: 2,
            count,
            durability: 0,
        }
    }
    fn ctx_death_write(c: &mut TickContext<'_>, pos: BlockPos, block: u16) {
        let observed = c.read().observation(Dimension::OVERWORLD, pos).unwrap();
        c.transaction()
            .try_system(
                SystemRule::Support,
                vec![BlockWrite::try_new(observed, block).unwrap()],
            )
            .unwrap();
    }
    fn ctx_death_live_bed(
        c: &mut TickContext<'_>,
        s: SessionKey,
        bed: Option<(Dimension, BlockPos)>,
    ) {
        let ActorAux::Player { respawn, .. } =
            &mut c.runtimes.get_mut(&ActorKey::Player(s)).unwrap().aux
        else {
            unreachable!()
        };
        *respawn = bed;
    }
    #[test]
    fn ctx_death_mode_eligibility_is_explicit() {
        for case in 0..6 {
            let (mut a, s, _) = ctx_fixture(false);
            let book = std::mem::take(&mut a.source_players);
            let mut c = TickContext::for_tick(&mut a, TickBudget::full());
            let absent = SessionKey::from_raw(999).unwrap();
            match case {
                1 => c.authority.source_player_radius = None,
                2 => c.authority.sessions.get_mut(&s).unwrap().phase = SessionPhase::Retired,
                4 => c.authority.phase = ServerPhase::Closing,
                5 => {
                    c.player_slots.remove(&s);
                }
                _ => {}
            }
            let before = ctx_death_snapshot(&c);
            let scan = ctx_book_snapshot(&book, s);
            assert_eq!(
                c.source_player_death_deferred(if case == 3 { absent } else { s }),
                !matches!(case, 1..=3),
                "case {case}"
            );
            assert_eq!(ctx_death_snapshot(&c), before);
            assert_eq!(ctx_book_snapshot(&book, s), scan);
        }
    }
    #[test]
    fn ctx_death_healthy_inactive_are_quiet() {
        for lifecycle in [
            ActorLifecycle::Active,
            ActorLifecycle::Pending,
            ActorLifecycle::Respawning,
            ActorLifecycle::Dead,
        ] {
            let (mut a, s, _) = ctx_fixture(false);
            if lifecycle != ActorLifecycle::Active {
                ctx_death_zero(&mut a, s);
            }
            a.residents.actors[a.residents.player_slots[&s]].lifecycle = lifecycle;
            a.residents.runtimes.remove(&ActorKey::Player(s));
            let book = std::mem::take(&mut a.source_players);
            let scan = ctx_scan(super::super::pending_restore::RestoreKind::Player, false);
            let before_book = ctx_book_snapshot(&book, s);
            let mut c = TickContext::for_tick(&mut a, TickBudget::full());
            let before = ctx_death_snapshot(&c);
            assert_eq!(c.settle_source_player_death(s, &scan), Ok(None));
            assert_eq!(ctx_death_snapshot(&c), before);
            assert_eq!(ctx_book_snapshot(&book, s), before_book);
        }
    }
    #[test]
    fn ctx_death_refusals_preserve_owners() {
        for case in 0..16 {
            let (mut a, s, other) = ctx_fixture(true);
            let other = other.unwrap();
            ctx_death_zero(&mut a, s);
            let book = std::mem::take(&mut a.source_players);
            let mut c = TickContext::for_tick(&mut a, TickBudget::full());
            ctx_seed_transients(&mut c, &[s, other]);
            let slot = c.player_slots[&s];
            let mut incomplete = None;
            match case {
                0 => {
                    c.authority.tick_failure = Some(ServerError::Disconnected);
                    c.authority.phase = ServerPhase::Closed;
                }
                1 => {
                    c.authority.phase = ServerPhase::Closed;
                    c.actors[slot].survival = ctx_survival(7, 9, 3);
                }
                2 => c.authority.source_player_radius = None,
                3 => c.authority.sessions.get_mut(&s).unwrap().phase = SessionPhase::Retired,
                4 => {
                    c.player_slots.remove(&s);
                }
                5 => {
                    c.player_slots.insert(s, c.actors.len());
                }
                6 => c.actors[slot].key = ActorKey::Player(other),
                7 => {
                    c.actors[slot].body = ActorBody::Passive(mornlea_storage::PassiveMob {
                        id: 1,
                        dimension: 0,
                        position: [0., 64., 0.],
                        velocity: [0.; 3],
                        on_ground: true,
                        yaw: 0.,
                        health: 7,
                    })
                }
                8 => {
                    c.runtimes.remove(&ActorKey::Player(s));
                }
                9 | 14 => {
                    c.runtimes.get_mut(&ActorKey::Player(s)).unwrap().key = ActorKey::Player(other);
                    if case == 14 {
                        incomplete = Some(ctx_scan(
                            super::super::pending_restore::RestoreKind::Player,
                            false,
                        ));
                    }
                }
                10 => {
                    c.runtimes.get_mut(&ActorKey::Player(s)).unwrap().aux = ActorAux::Hostile {
                        distant_ticks: 1,
                        shoot_cooldown: 2,
                        fresh: true,
                    }
                }
                11 => {
                    incomplete = Some(ctx_scan(
                        super::super::pending_restore::RestoreKind::Player,
                        false,
                    ))
                }
                12 => {
                    incomplete = Some(ctx_scan(
                        super::super::pending_restore::RestoreKind::Companion,
                        true,
                    ))
                }
                13 => {
                    c.inventories.remove(&ActorKey::Player(s));
                }
                15 => c.environment = None,
                _ => unreachable!(),
            }
            let error = match case {
                0 => ServerError::Disconnected,
                1 => ServerError::InvalidState {
                    phase: ServerPhase::Closed,
                },
                11 | 12 => ServerError::InvalidInput {
                    field: "restore_restart",
                },
                13 | 15 => ServerError::Internal {
                    invariant: "source player death",
                },
                _ => ctx_error(),
            };
            let scan = incomplete.as_ref().unwrap_or(&book.entries[&s].restore);
            let before_book = ctx_book_snapshot(&book, s);
            ctx_death_refuse(&mut c, s, scan, error);
            assert_eq!(ctx_book_snapshot(&book, s), before_book);
        }
    }
    #[test]
    fn ctx_death_fills_stats_and_cleans_only_owned_state() {
        let (mut a, s, other) = ctx_fixture(true);
        let other = other.unwrap();
        ctx_death_zero(&mut a, s);
        ctx_add_allocations(&mut a, s);
        a.sessions.get_mut(&s).unwrap().last_input_sequence = 3;
        let book = std::mem::take(&mut a.source_players);
        let mut c = TickContext::for_tick(&mut a, TickBudget::full());
        ctx_seed_transients(&mut c, &[s, other]);
        c.sleep_record = SleepState::try_new(
            vec![
                (s, Dimension::DEPTHS, BlockPos::new(-1, 2, 0)),
                (other, Dimension::OVERWORLD, BlockPos::new(2, 3, 4)),
            ],
            55,
            Some(66),
        )
        .unwrap();
        let reference = ContainerRef::try_new(
            ChunkPos::new(0, 0),
            mornlea_domain::ContainerKind::Chest,
            0,
            1,
        )
        .unwrap();
        c.viewers.insert(s, ViewLease::new(s, reference));
        c.viewers.insert(other, ViewLease::new(other, reference));
        c.charges = vec![
            (ActorKey::Player(s), ActionKind::Till),
            (ActorKey::Player(other), ActionKind::Mining),
            (ActorKey::Player(s), ActionKind::Mining),
            (ActorKey::Player(other), ActionKind::Melee),
            (ActorKey::Player(s), ActionKind::Melee),
        ];
        let want = ctx_death_expected(&c, s);
        let allocations = ctx_allocations(
            &c.actors[c.player_slots[&s]],
            &c.runtimes[&ActorKey::Player(s)],
        );
        let other_pair = (
            c.actors[c.player_slots[&other]].clone(),
            c.runtimes[&ActorKey::Player(other)].clone(),
        );
        let before = (
            c.inventories.clone(),
            c.pre_step.clone(),
            c.sleep_record.clone(),
            c.viewers.clone(),
            c.sleep_record_touched,
            c.mining[&ActorKey::Player(other)].clone(),
        );
        let book_before = ctx_book_snapshot(&book, s);
        assert_eq!(
            c.settle_source_player_death(s, &book.entries[&s].restore),
            Ok(ctx_death_result(Dimension::DEPTHS, None))
        );
        ctx_assert_pair(&c, s, &want);
        ctx_assert_pair(&c, other, &other_pair);
        assert_eq!(
            ctx_allocations(
                &c.actors[c.player_slots[&s]],
                &c.runtimes[&ActorKey::Player(s)]
            ),
            allocations
        );
        assert_eq!(
            (
                c.inventories.clone(),
                c.pre_step.clone(),
                c.sleep_record.clone(),
                c.viewers.clone(),
                c.sleep_record_touched,
                c.mining[&ActorKey::Player(other)].clone()
            ),
            before
        );
        assert!(!c.mining.contains_key(&ActorKey::Player(s)));
        assert!(!c.sleeping.contains(&s));
        assert!(c.sleeping.contains(&other));
        assert!(!c.suppressed_mining.contains(&ActorKey::Player(s)));
        assert!(c.suppressed_mining.contains(&ActorKey::Player(other)));
        assert_eq!(
            c.charges,
            vec![
                (ActorKey::Player(other), ActionKind::Mining),
                (ActorKey::Player(other), ActionKind::Melee)
            ]
        );
        assert_eq!(c.authority.sessions[&s].last_input_sequence, 3);
        assert_eq!(ctx_book_snapshot(&book, s), book_before);
        assert_eq!(
            book.entries[&s].restore.player_reset_anchor(),
            Ok(ChunkPos::new(2, 0))
        );
    }
    #[test]
    fn ctx_death_repack_is_lossless_or_hard_failure() {
        for impossible in [false, true] {
            let (mut a, s, _) = ctx_fixture(false);
            ctx_death_zero(&mut a, s);
            let mut inventory = InventoryRecord::empty();
            inventory.crafting_size = mornlea_domain::CraftingSize::Workbench;
            if impossible {
                inventory.slots = [ctx_death_stack(64); 36];
                inventory.crafting[0] = ctx_death_stack(1);
            } else {
                inventory.crafting = [ctx_death_stack(1); 9];
            }
            a.residents
                .inventories
                .insert(ActorKey::Player(s), inventory);
            let book = std::mem::take(&mut a.source_players);
            let mut c = TickContext::for_tick(&mut a, TickBudget::full());
            let body = c.actors[c.player_slots[&s]].body.clone();
            c.charges.push((ActorKey::Player(s), ActionKind::Mining));
            if impossible {
                ctx_death_refuse(
                    &mut c,
                    s,
                    &book.entries[&s].restore,
                    ServerError::Internal {
                        invariant: "source player death crafting",
                    },
                );
            } else {
                assert_eq!(
                    c.settle_source_player_death(s, &book.entries[&s].restore),
                    Ok(ctx_death_result(Dimension::DEPTHS, None))
                );
                let after = &c.inventories[&ActorKey::Player(s)];
                let mut expected = inventory;
                expected.slots[0] = ctx_death_stack(9);
                expected.crafting = [Default::default(); 9];
                expected.crafting_size = mornlea_domain::CraftingSize::Personal;
                assert_eq!(*after, expected);
                assert_eq!(
                    after
                        .slots
                        .iter()
                        .chain(after.armor.iter())
                        .map(|s| u32::from(s.count))
                        .sum::<u32>(),
                    9
                );
            }
            assert_eq!(c.actors[c.player_slots[&s]].body, body);
        }
    }
    fn ctx_death_full_drops(k: ChunkKey) -> super::super::drop_store::DropState {
        super::super::drop_store::DropState::new(
            k,
            [mornlea_storage::DropSlot {
                generation: 1,
                active: true,
                stack: ctx_death_stack(64),
                block_index: mornlea_domain::chunk_block_index(BlockPos::new(8, 65, 8)),
                age_ticks: 0,
                pickup_delay_ticks: 0,
            }; 32],
        )
    }
    fn ctx_death_total(c: &TickContext<'_>, s: SessionKey) -> u32 {
        let inv = &c.inventories[&ActorKey::Player(s)];
        inv.slots
            .iter()
            .chain(inv.armor.iter())
            .chain(inv.crafting.iter())
            .map(|s| u32::from(s.count))
            .sum::<u32>()
            + c.drops
                .values()
                .flat_map(|v| v.records())
                .map(|v| u32::from(v.stack.count))
                .sum::<u32>()
    }
    #[test]
    fn ctx_death_ring_previews_clear_only_accepted_slots() {
        for case in 0..3 {
            let (mut a, s, _) = ctx_fixture(false);
            if case != 2 {
                offer(&mut a, key(Dimension::OVERWORLD, 1, 0), 0);
                a.advance_tick(TickBudget::full()).unwrap();
            }
            ctx_recovery_pose(
                &mut a,
                s,
                if case == 2 {
                    Dimension::DEPTHS
                } else {
                    Dimension::OVERWORLD
                },
                [8.5, 65., 8.5],
            );
            ctx_death_zero(&mut a, s);
            let mut inventory = InventoryRecord::empty();
            inventory.selected = HotbarSlot::new(3).unwrap();
            inventory.crafting_size = mornlea_domain::CraftingSize::Workbench;
            inventory.slots[0] = ctx_death_stack(1);
            inventory.slots[1] = ctx_death_stack(2);
            inventory.armor[0] = ctx_death_stack(1);
            a.residents
                .inventories
                .insert(ActorKey::Player(s), inventory);
            let current = key(Dimension::OVERWORLD, 0, 0);
            let neighbor = key(Dimension::OVERWORLD, 1, 0);
            a.residents
                .drops
                .insert(current, ctx_death_full_drops(current));
            if case == 1 {
                a.residents
                    .drops
                    .insert(neighbor, ctx_death_full_drops(neighbor));
            }
            let book = std::mem::take(&mut a.source_players);
            let mut c = TickContext::for_tick(&mut a, TickBudget::full());
            let before = ctx_death_snapshot(&c);
            let total = ctx_death_total(&c, s);
            let full = c.drops[&current].slots;
            let delay = c
                .environment
                .as_ref()
                .unwrap()
                .tunables
                .player_drop_pickup_delay_ticks();
            let dimension = c.actors[c.player_slots[&s]].dimension;
            let prepared = super::super::source_player_death::prepare_inventory(
                &c.read(),
                ActorKey::Player(s),
                dimension,
                [8.5, 65., 8.5],
                delay,
                inventory,
            )
            .unwrap();
            assert_eq!(ctx_death_snapshot(&c), before);
            if case == 0 {
                assert_eq!(prepared.drops.len(), 3);
                for effect in &prepared.drops {
                    let RuleEffect::Drops(batch) = effect else {
                        unreachable!()
                    };
                    assert_eq!(
                        batch.source,
                        DropSource::Death {
                            actor: ActorKey::Player(s),
                            tick: c.read().tick()
                        }
                    );
                    assert_eq!(batch.dimension, dimension);
                    assert_eq!(batch.origin.get(), [16.5, 65.5, 8.5]);
                    assert_eq!(batch.pickup_delay, delay);
                }
            } else {
                assert!(prepared.drops.is_empty());
            }
            assert_eq!(
                c.settle_source_player_death(s, &book.entries[&s].restore),
                Ok(ctx_death_result(dimension, None))
            );
            assert_eq!(ctx_death_total(&c, s), total);
            assert_eq!(c.drops[&current].slots, full);
            let mut expected = inventory;
            expected.crafting_size = mornlea_domain::CraftingSize::Personal;
            if case == 0 {
                expected.slots[0] = Default::default();
                expected.slots[1] = Default::default();
                expected.armor[0] = Default::default();
                let records = c.drops[&neighbor].records();
                assert_eq!(records.len(), 1);
                assert_eq!(records[0].stack, ctx_death_stack(4));
                assert_eq!(records[0].position.get(), [16.5, 65.5, 8.5]);
                assert_eq!(records[0].pickup_delay, delay);
            }
            assert_eq!(c.inventories[&ActorKey::Player(s)], expected);
            assert_eq!(
                c.actors[c.player_slots[&s]].lifecycle,
                ActorLifecycle::Pending
            );
            assert_eq!(c.actors[c.player_slots[&s]].survival.health(), 20);
        }
    }
    #[test]
    fn ctx_death_uses_live_bed_and_delays_geometry() {
        let mut rows = vec![
            (None, None, None, false),
            (
                Some((Dimension::DEPTHS, BlockPos::new(8, 64, 8))),
                None,
                None,
                false,
            ),
            (
                Some((Dimension::OVERWORLD, BlockPos::new(1000, 64, 8))),
                None,
                None,
                false,
            ),
            (
                Some((Dimension::OVERWORLD, BlockPos::new(8, 64, 8))),
                None,
                None,
                true,
            ),
            (
                Some((Dimension::OVERWORLD, BlockPos::new(8, 64, 8))),
                Some(76),
                Some(80),
                false,
            ),
            (
                Some((Dimension::OVERWORLD, BlockPos::new(8, 64, 8))),
                Some(76),
                Some(81),
                true,
            ),
            (
                Some((Dimension::OVERWORLD, BlockPos::new(15, 64, 8))),
                Some(79),
                None,
                false,
            ),
            (
                Some((Dimension::OVERWORLD, BlockPos::new(8, 64, 8))),
                Some(80),
                None,
                true,
            ),
        ];
        for y in [-65, -64, 319, 320] {
            rows.push((
                Some((Dimension::OVERWORLD, BlockPos::new(1000, y, 8))),
                None,
                None,
                !(-64..320).contains(&y),
            ));
            if (-64..320).contains(&y) {
                rows.push((
                    Some((Dimension::OVERWORLD, BlockPos::new(8, y, 8))),
                    None,
                    None,
                    true,
                ));
            }
            rows.push((
                Some((Dimension::DEPTHS, BlockPos::new(1000, y, 8))),
                None,
                None,
                false,
            ));
        }
        for dir in 0..4 {
            rows.push((
                Some((Dimension::OVERWORLD, BlockPos::new(8, 64, 8))),
                Some(76 + dir),
                Some(80 + dir),
                false,
            ));
        }
        for (bed, foot, head, clear) in rows {
            let (mut a, s, _) = ctx_fixture(false);
            ctx_recovery_pose(&mut a, s, Dimension::OVERWORLD, [8.5, 65., 8.5]);
            ctx_death_zero(&mut a, s);
            let book = std::mem::take(&mut a.source_players);
            let book_before = ctx_book_snapshot(&book, s);
            let mut c = TickContext::for_tick(&mut a, TickBudget::full());
            if let (Some((_, pos)), Some(form)) = (bed, foot) {
                ctx_death_write(&mut c, pos, form);
                if let Some(head) = head {
                    let partner = crate::rules::sleep::bed_head_neighbor(
                        pos,
                        crate::rules::sleep::bed_dir(form).unwrap(),
                    )
                    .unwrap();
                    ctx_death_write(&mut c, partner, head);
                }
            }
            ctx_death_live_bed(&mut c, s, bed);
            let durable = (
                c.actors[c.player_slots[&s]].body.clone(),
                c.sleep_record.clone(),
            );
            let workbench = match c.runtimes[&ActorKey::Player(s)].aux {
                ActorAux::Player { workbench, .. } => workbench,
                _ => unreachable!(),
            };
            let valid = foot.is_some_and(|f| (76..80).contains(&f)) && head == foot.map(|f| f + 4);
            let candidate = if valid {
                let pos = bed.unwrap().1;
                Some(super::super::actor_placement::RestoreCandidate {
                    dimension: Dimension::OVERWORLD,
                    position: [
                        pos.x() as f32 + 0.5,
                        pos.y() as f32 + 0.5625,
                        pos.z() as f32 + 0.5,
                    ],
                    require_support: false,
                })
            } else {
                None
            };
            assert_eq!(
                c.settle_source_player_death(s, &book.entries[&s].restore),
                Ok(ctx_death_result(Dimension::OVERWORLD, candidate))
            );
            assert_eq!(
                c.runtimes[&ActorKey::Player(s)].aux,
                ActorAux::Player {
                    respawn: if clear { None } else { bed },
                    workbench
                }
            );
            assert_eq!(
                (
                    c.actors[c.player_slots[&s]].body.clone(),
                    c.sleep_record.clone()
                ),
                durable
            );
            assert_eq!(ctx_book_snapshot(&book, s), book_before);
        }
        // The source placement read owns height semantics even without acquisition.
        for y in [-65, -64, 319, 320] {
            for dimension in [Dimension::OVERWORLD, Dimension::DEPTHS] {
                for x in [8, 1000] {
                    let (mut a, s, _) = ctx_fixture(false);
                    a.acquisition = Default::default();
                    let book = std::mem::take(&mut a.source_players);
                    let c = TickContext::for_tick(&mut a, TickBudget::full());
                    let before = ctx_death_snapshot(&c);
                    let trace = RefCell::new(ObservationTrace::default());
                    let view = c.read();
                    let traced = view.with_observation_trace(&trace);
                    let bed = Some((dimension, BlockPos::new(x, y, 8)));
                    let prepared = super::super::source_player_death::prepare_bed(
                        &traced,
                        Dimension::OVERWORLD,
                        bed,
                    )
                    .unwrap();
                    let clear =
                        dimension == Dimension::OVERWORLD && (!(-64..320).contains(&y) || x == 8);
                    assert_eq!(prepared.respawn, if clear { None } else { bed });
                    assert_eq!(prepared.candidate, None);
                    if !(-64..320).contains(&y) || dimension != Dimension::OVERWORLD {
                        assert!(trace.borrow().cells.is_empty());
                    }
                    assert_eq!(ctx_death_snapshot(&c), before);
                    assert!(book.entries[&s].restore.player_reset_anchor().is_ok());
                }
            }
        }
        // One real acquired extreme foot proves checked neighbor overflow without a head read.
        let (mut a, s, _) = ctx_fixture(false);
        offer(&mut a, key(Dimension::OVERWORLD, i32::MAX >> 4, 0), 0);
        a.advance_tick(TickBudget::full()).unwrap();
        ctx_recovery_pose(&mut a, s, Dimension::OVERWORLD, [8.5, 65., 8.5]);
        ctx_death_zero(&mut a, s);
        let book = std::mem::take(&mut a.source_players);
        let before_book = ctx_book_snapshot(&book, s);
        let mut c = TickContext::for_tick(&mut a, TickBudget::full());
        let pos = BlockPos::new(i32::MAX, 64, 8);
        ctx_death_write(&mut c, pos, 79);
        let bed = Some((Dimension::OVERWORLD, pos));
        ctx_death_live_bed(&mut c, s, bed);
        let before = ctx_death_snapshot(&c);
        let trace = RefCell::new(ObservationTrace::default());
        {
            let view = c.read();
            let traced = view.with_observation_trace(&trace);
            let prepared =
                super::super::source_player_death::prepare_bed(&traced, Dimension::OVERWORLD, bed)
                    .unwrap();
            assert_eq!(prepared.respawn, bed);
            assert_eq!(prepared.candidate, None);
        }
        assert_eq!(trace.borrow().cells.len(), 1);
        assert!(
            trace
                .borrow()
                .cells
                .contains_key(&(Dimension::OVERWORLD, pos))
        );
        assert_eq!(ctx_death_snapshot(&c), before);
        assert_eq!(
            c.settle_source_player_death(s, &book.entries[&s].restore),
            Ok(ctx_death_result(Dimension::OVERWORLD, None))
        );
        assert_eq!(ctx_book_snapshot(&book, s), before_book);
    }
    #[test]
    fn ctx_death_ready_cap_precedes_enumeration_and_drop_returns() {
        for over in [false, true] {
            let (mut a, s, _) = ctx_fixture(false);
            ctx_death_zero(&mut a, s);
            a.residents
                .inventories
                .insert(ActorKey::Player(s), InventoryRecord::empty());
            let original = a.residents.ready[&key(Dimension::OVERWORLD, 0, 0)].clone();
            let count = super::super::acquisition::MAX_MANAGED + usize::from(over);
            for x in 1..count {
                let k = key(Dimension::OVERWORLD, x as i32, 0);
                let mut copy = original.clone();
                copy.key = k;
                a.residents.ready.insert(k, copy);
            }
            let book = std::mem::take(&mut a.source_players);
            let book_before = ctx_book_snapshot(&book, s);
            let mut c = TickContext::for_tick(&mut a, TickBudget::full());
            assert_eq!(c.ready.len(), count);
            if over {
                ctx_death_refuse(
                    &mut c,
                    s,
                    &book.entries[&s].restore,
                    ServerError::Capacity {
                        resource: Resource::ResidentChunks,
                        limit: super::super::acquisition::MAX_MANAGED,
                        observed: count,
                    },
                );
            } else {
                assert_eq!(
                    c.settle_source_player_death(s, &book.entries[&s].restore),
                    Ok(ctx_death_result(Dimension::DEPTHS, None))
                );
            }
            assert_eq!(ctx_book_snapshot(&book, s), book_before);
        }
        let (mut a, s, _) = ctx_fixture(false);
        ctx_recovery_pose(&mut a, s, Dimension::OVERWORLD, [8.5, 65., 8.5]);
        ctx_death_zero(&mut a, s);
        ctx_add_allocations(&mut a, s);
        let selected = a.residents.inventories[&ActorKey::Player(s)].selected;
        a.residents.inventories.insert(
            ActorKey::Player(s),
            InventoryRecord::empty().with_selected(selected),
        );
        let book = std::mem::take(&mut a.source_players);
        let before_book = ctx_book_snapshot(&book, s);
        let allocations =
            ctx_allocations(player(&a, s), &a.residents.runtimes[&ActorKey::Player(s)]);
        let inventory = a.residents.inventories[&ActorKey::Player(s)];
        let want;
        let reset;
        {
            let mut c = TickContext::for_tick(&mut a, TickBudget::full());
            ctx_seed_transients(&mut c, &[s]);
            ctx_death_write(&mut c, BlockPos::new(8, 64, 8), 76);
            ctx_death_write(&mut c, BlockPos::new(8, 64, 9), 80);
            ctx_death_live_bed(
                &mut c,
                s,
                Some((Dimension::OVERWORLD, BlockPos::new(8, 64, 8))),
            );
            want = ctx_death_expected(&c, s);
            reset = c
                .settle_source_player_death(s, &book.entries[&s].restore)
                .unwrap();
        }
        a.source_players = book;
        assert_eq!(
            reset,
            ctx_death_result(
                Dimension::OVERWORLD,
                Some(super::super::actor_placement::RestoreCandidate {
                    dimension: Dimension::OVERWORLD,
                    position: [8.5, 64.5625, 8.5],
                    require_support: false,
                })
            )
        );
        assert_eq!(
            (player(&a, s), &a.residents.runtimes[&ActorKey::Player(s)]),
            (&want.0, &want.1)
        );
        assert_eq!(a.residents.inventories[&ActorKey::Player(s)], inventory);
        assert_eq!(
            ctx_allocations(player(&a, s), &a.residents.runtimes[&ActorKey::Player(s)]),
            allocations
        );
        assert_eq!(ctx_book_snapshot(&a.source_players, s), before_book);
    }
    fn ctx_death_consumer_call(s: SessionKey, phase: RulePhase) -> RuleCall<'static> {
        RuleCall {
            phase,
            actor: Some(ActorKey::Player(s)),
            command: None,
            internal: None,
        }
    }
    #[test]
    fn ctx_death_consumer_defers_both_legacy_entries() {
        let (mut a, s, _) = ctx_fixture(false);
        ctx_death_zero(&mut a, s);
        ctx_add_allocations(&mut a, s);
        let book = std::mem::take(&mut a.source_players);
        let mut c = TickContext::for_tick(&mut a, TickBudget::full());
        c.environment = None;
        let runtime = c.runtimes.remove(&ActorKey::Player(s)).unwrap();
        let body_ptr = match &c.actors[c.player_slots[&s]].body {
            ActorBody::Player(b) => b.display_name.as_ptr(),
            _ => unreachable!(),
        };
        let before = ctx_death_snapshot(&c);
        let scan = ctx_book_snapshot(&book, s);
        for phase in [
            RulePhase::PlayerRegenStarvation,
            RulePhase::PlayerPrePhysicsOxygen,
            RulePhase::PlayerPostPhysics,
        ] {
            assert_eq!(
                crate::rules::player_survival::run(&mut c, ctx_death_consumer_call(s, phase)),
                Ok(PhaseReport {
                    examined: 1,
                    applied: 0,
                    carried: 0,
                    rejected: 0
                })
            );
            assert_eq!(ctx_death_snapshot(&c), before);
            assert_eq!(ctx_book_snapshot(&book, s), scan);
            let ActorBody::Player(body) = &c.actors[c.player_slots[&s]].body else {
                unreachable!()
            };
            assert_eq!(body.display_name.as_ptr(), body_ptr);
        }
        assert_eq!(
            crate::rules::hostile_outcomes::run(
                &mut c,
                RuleCall {
                    phase: RulePhase::HostilePlayerDeaths,
                    actor: None,
                    command: None,
                    internal: None
                }
            ),
            Ok(PhaseReport {
                examined: 0,
                applied: 0,
                carried: 0,
                rejected: 0
            })
        );
        assert_eq!(ctx_death_snapshot(&c), before);
        assert_eq!(ctx_book_snapshot(&book, s), scan);
        assert_eq!(
            c.actors[c.player_slots[&s]].lifecycle,
            ActorLifecycle::Active
        );
        assert_eq!(c.actors[c.player_slots[&s]].survival.health(), 0);
        let command =
            mornlea_domain::CommandEnvelope::try_new(mornlea_domain::CommandEnvelopeParts {
                tick: 0,
                session: s.get(),
                sequence: 1,
                arrival_index: 0,
                command: mornlea_domain::Command::CloseContainer,
            })
            .unwrap();
        let internal = AuthorityInteraction {
            session: s,
            look: c.actors[c.player_slots[&s]].look,
            kind: InteractionKind::Bed,
            sequence: 1,
        };
        for row in 0..4 {
            if row == 3 {
                c.actors[c.player_slots[&s]].lifecycle = ActorLifecycle::Pending;
            }
            let mut call = ctx_death_consumer_call(s, RulePhase::PlayerPostPhysics);
            match row {
                0 => call.phase = RulePhase::Publish,
                1 => call.command = Some(&command),
                2 => call.internal = Some(&internal),
                _ => {}
            }
            let before = ctx_death_snapshot(&c);
            assert_eq!(
                crate::rules::player_survival::run(&mut c, call),
                Err(ServerError::InvalidInput {
                    field: if row == 3 { "actor" } else { "phase" }
                })
            );
            assert_eq!(ctx_death_snapshot(&c), before);
            assert_eq!(ctx_book_snapshot(&book, s), scan);
        }
        assert!(runtime.path.is_some());
        drop(c);
        let (mut legacy, s, _) = ctx_fixture(false);
        ctx_death_zero(&mut legacy, s);
        legacy.source_player_radius = None;
        let mut c = TickContext::for_tick(&mut legacy, TickBudget::full());
        let result = crate::rules::hostile_outcomes::run(
            &mut c,
            RuleCall {
                phase: RulePhase::HostilePlayerDeaths,
                actor: None,
                command: None,
                internal: None,
            },
        )
        .unwrap();
        assert_eq!((result.examined, result.applied), (1, 1));
        assert_eq!(
            c.actors[c.player_slots[&s]].lifecycle,
            ActorLifecycle::Respawning
        );
        assert_eq!(c.actors[c.player_slots[&s]].survival.health(), 20);
    }
    #[test]
    fn ctx_death_consumer_sorted_restart_is_once() {
        let (mut a, s, other) = ctx_fixture(true);
        let other = other.unwrap();
        assert!(s < other);
        for session in [s, other] {
            ctx_death_zero(&mut a, session);
            ctx_add_allocations(&mut a, session);
            let actor = &mut a.residents.actors[a.residents.player_slots[&session]];
            actor.dimension = Dimension::OVERWORLD;
            actor.motion = MotionState::new(mornlea_domain::MotionStateParts {
                position: mornlea_domain::FiniteVec3::try_new([8.5, 65., 8.5]).unwrap(),
                velocity: mornlea_domain::FiniteVec3::try_new([0.; 3]).unwrap(),
                on_ground: false,
            });
            let mut inventory = InventoryRecord::empty();
            inventory.slots[0] = mornlea_storage::ItemStack {
                item: if session == s { 1 } else { 2 },
                count: 1,
                durability: 0,
            };
            a.residents
                .inventories
                .insert(ActorKey::Player(session), inventory);
        }
        let mut book = std::mem::take(&mut a.source_players);
        let addresses: Vec<_> = [s, other]
            .into_iter()
            .map(|s| &book.entries[&s] as *const _ as usize)
            .collect();
        let mut c = TickContext::for_tick(&mut a, TickBudget::full());
        for session in [s, other] {
            ctx_death_live_bed(&mut c, session, None);
        }
        ctx_seed_transients(&mut c, &[s, other]);
        let outsider = ActorKey::Hostile(mornlea_domain::HostileId::try_new(99).unwrap());
        c.charges = vec![
            (ActorKey::Player(s), ActionKind::Mining),
            (outsider, ActionKind::Melee),
            (ActorKey::Player(other), ActionKind::Till),
        ];
        let allocations: Vec<_> = [s, other]
            .into_iter()
            .map(|s| {
                ctx_allocations(
                    &c.actors[c.player_slots[&s]],
                    &c.runtimes[&ActorKey::Player(s)],
                )
            })
            .collect();
        let current = key(Dimension::OVERWORLD, 0, 0);
        let mut drops = ctx_death_full_drops(current);
        drops.slots[31] = mornlea_storage::DropSlot::default();
        c.drops.insert(current, drops);
        assert_eq!(
            super::super::source_player_restore::settle_deaths(&mut book, &mut c),
            Ok(())
        );
        for (index, session) in [s, other].into_iter().enumerate() {
            let actor = &c.actors[c.player_slots[&session]];
            assert_eq!(actor.lifecycle, ActorLifecycle::Pending);
            assert_eq!(
                (
                    actor.survival.health(),
                    actor.survival.hunger(),
                    actor.survival.oxygen()
                ),
                (20, 20, 300)
            );
            assert_eq!(
                ctx_allocations(actor, &c.runtimes[&actor.key]),
                allocations[index]
            );
            assert!(!c.mining.contains_key(&actor.key));
            assert!(!c.sleeping.contains(&session));
            assert!(!c.suppressed_mining.contains(&actor.key));
            assert_eq!(
                &book.entries[&session] as *const _ as usize,
                addresses[index]
            );
            assert!(book.entries[&session].ever_spawned);
            assert_eq!(
                book.entries[&session].restore.player_reset_anchor(),
                Err(ServerError::InvalidInput {
                    field: "restore_restart"
                })
            );
            assert!(book.entries[&session].restore.pending_keys().contains(&key(
                Dimension::OVERWORLD,
                2,
                0
            )));
            assert!(
                format!("{:?}", book.entries[&session].restore)
                    .contains("anchor: ChunkPos { x: 2, z: 0 }")
            );
        }
        assert_eq!(c.charges, vec![(outsider, ActionKind::Melee)]);
        assert_eq!(
            c.inventories[&ActorKey::Player(s)].slots[0],
            mornlea_storage::ItemStack::default()
        );
        assert_eq!(
            c.inventories[&ActorKey::Player(other)].slots[0],
            ctx_death_stack(1)
        );
        let ground: u32 = c
            .drops
            .values()
            .flat_map(|d| d.records())
            .map(|d| u32::from(d.stack.count))
            .sum();
        assert_eq!(ground, 31 * 64 + 1);
        assert_eq!(
            ctx_death_total(&c, s)
                + u32::from(c.inventories[&ActorKey::Player(other)].slots[0].count),
            31 * 64 + 2
        );
        let before = ctx_death_snapshot(&c);
        let scans = [ctx_book_snapshot(&book, s), ctx_book_snapshot(&book, other)];
        assert_eq!(
            super::super::source_player_restore::settle_deaths(&mut book, &mut c),
            Ok(())
        );
        assert_eq!(ctx_death_snapshot(&c), before);
        assert_eq!(
            [ctx_book_snapshot(&book, s), ctx_book_snapshot(&book, other)],
            scans
        );
    }
    #[test]
    fn ctx_death_consumer_ineligible_is_quiet() {
        for row in 0..7 {
            let (mut a, s) = if row == 3 {
                fixture()
            } else {
                let (a, s, _) = ctx_fixture(false);
                (a, s)
            };
            match row {
                0 => a.source_player_radius = None,
                1 => a.sessions.get_mut(&s).unwrap().phase = SessionPhase::Retired,
                2 => {
                    a.sessions.remove(&s);
                }
                4 => {
                    a.residents.actors[a.residents.player_slots[&s]].lifecycle =
                        ActorLifecycle::Pending
                }
                6 => a.source_players.entries.clear(),
                _ => {}
            }
            a.residents.runtimes.remove(&ActorKey::Player(s));
            let mut book = std::mem::take(&mut a.source_players);
            let scans: Vec<_> = book
                .entries
                .keys()
                .map(|s| ctx_book_snapshot(&book, *s))
                .collect();
            let mut c = TickContext::for_tick(&mut a, TickBudget::full());
            c.environment = None;
            let before = ctx_death_snapshot(&c);
            assert_eq!(
                super::super::source_player_restore::settle_deaths(&mut book, &mut c),
                Ok(()),
                "row {row}"
            );
            assert_eq!(ctx_death_snapshot(&c), before, "row {row}");
            assert_eq!(
                book.entries
                    .keys()
                    .map(|s| ctx_book_snapshot(&book, *s))
                    .collect::<Vec<_>>(),
                scans
            );
        }
    }

    fn ctx_safe_fixture(two: bool) -> (AuthorityState, SessionKey, Option<SessionKey>) {
        let (mut a, s, other) = ctx_fixture(two);
        offer(&mut a, key(Dimension::DEPTHS, 0, 0), 1);
        a.advance_tick(TickBudget::full()).unwrap();
        for session in std::iter::once(s).chain(other) {
            let actor = &mut a.residents.actors[a.residents.player_slots[&session]];
            actor.lifecycle = ActorLifecycle::Active;
            actor.dimension = Dimension::DEPTHS;
            actor.motion = MotionState::new(mornlea_domain::MotionStateParts {
                position: mornlea_domain::FiniteVec3::try_new([8.5, 64., 8.5]).unwrap(),
                velocity: mornlea_domain::FiniteVec3::try_new([0.; 3]).unwrap(),
                on_ground: true,
            });
            a.residents
                .runtimes
                .get_mut(&ActorKey::Player(session))
                .unwrap()
                .reset = false;
        }
        (a, s, other)
    }
    fn ctx_safe_value(c: &TickContext<'_>, s: SessionKey) -> Option<PlayerLocation> {
        let ActorBody::Player(body) = &c.actors[c.player_slots[&s]].body else {
            unreachable!()
        };
        body.safe.as_ref().map(|v| PlayerLocation {
            dimension: v.dimension,
            position: v.position,
        })
    }
    fn ctx_safe_set(c: &mut TickContext<'_>, s: SessionKey, safe: Option<PlayerLocation>) {
        let slot = c.player_slots[&s];
        let ActorBody::Player(body) = &mut c.actors[slot].body else {
            unreachable!()
        };
        body.safe = safe;
    }
    fn ctx_safe_error() -> ServerError {
        ServerError::InvalidInput {
            field: "source_player_safe",
        }
    }
    fn ctx_safe_refuse(c: &mut TickContext<'_>, s: SessionKey, error: ServerError) {
        let before = ctx_death_snapshot(c);
        assert_eq!(c.checkpoint_source_player_safe(s), Err(error));
        assert_eq!(ctx_death_snapshot(c), before);
    }

    #[test]
    fn ctx_safe_checkpoint_updates_in_place() {
        for row in 0..4 {
            let (mut a, s, _) = ctx_safe_fixture(false);
            ctx_add_allocations(&mut a, s);
            let book = std::mem::take(&mut a.source_players);
            let scan = ctx_book_snapshot(&book, s);
            let mut c = TickContext::for_tick(&mut a, TickBudget::full());
            c.environment = None;
            ctx_seed_transients(&mut c, &[s]);
            let want = Some(PlayerLocation {
                dimension: 1,
                position: [8.5, 64., 8.5],
            });
            let old = match row {
                0 | 3 => None,
                1 => Some(PlayerLocation {
                    dimension: 0,
                    position: [56.5, 65., 8.5],
                }),
                2 => want.clone(),
                _ => unreachable!(),
            };
            ctx_safe_set(&mut c, s, old.clone());
            if row == 3 {
                let slot = c.player_slots[&s];
                c.actors[slot].survival = ctx_survival(0, 9, 3);
                c.runtimes.get_mut(&ActorKey::Player(s)).unwrap().has_view = false;
            }
            let allocations = ctx_allocations(
                &c.actors[c.player_slots[&s]],
                &c.runtimes[&ActorKey::Player(s)],
            );
            let before = ctx_death_snapshot(&c);
            assert_eq!(
                super::super::source_player_restore::checkpoint_safe(&book, &mut c, s),
                Ok(())
            );
            let observed = ctx_safe_value(&c, s);
            assert_eq!(observed, want, "row {row}");
            assert_eq!(
                ctx_allocations(
                    &c.actors[c.player_slots[&s]],
                    &c.runtimes[&ActorKey::Player(s)]
                ),
                allocations
            );
            assert_eq!(ctx_book_snapshot(&book, s), scan);
            // Normalize only the checked fixed Safe field to compare every other owner.
            ctx_safe_set(&mut c, s, old);
            assert_eq!(ctx_death_snapshot(&c), before);
            ctx_safe_set(&mut c, s, observed);
            let accepted = ctx_death_snapshot(&c);
            assert_eq!(
                super::super::source_player_restore::checkpoint_safe(&book, &mut c, s),
                Ok(())
            );
            assert_eq!(ctx_death_snapshot(&c), accepted);
            assert_eq!(ctx_book_snapshot(&book, s), scan);
            assert_eq!(
                ctx_allocations(
                    &c.actors[c.player_slots[&s]],
                    &c.runtimes[&ActorKey::Player(s)]
                ),
                allocations
            );
        }
    }

    #[test]
    fn ctx_safe_checkpoint_ineligible_is_quiet() {
        for row in 0..10 {
            let (mut a, s) = if row == 1 {
                fixture()
            } else {
                let (a, s, _) = ctx_safe_fixture(false);
                (a, s)
            };
            if row == 2 {
                a.source_player_radius = None;
            }
            if row == 3 {
                a.sessions.get_mut(&s).unwrap().phase = SessionPhase::Retired;
            }
            if row == 4 {
                a.sessions.remove(&s);
            }
            let mut book = std::mem::take(&mut a.source_players);
            if row == 0 {
                book.entries.remove(&s);
            }
            if row == 1 {
                assert!(!book.entries[&s].ever_spawned);
            }
            let scans = book
                .entries
                .keys()
                .map(|s| ctx_book_snapshot(&book, *s))
                .collect::<Vec<_>>();
            let mut c = TickContext::for_tick(&mut a, TickBudget::full());
            let slot = c.player_slots[&s];
            match row {
                5 => c.actors[slot].lifecycle = ActorLifecycle::Pending,
                6 => c.actors[slot].lifecycle = ActorLifecycle::Respawning,
                7 => c.actors[slot].lifecycle = ActorLifecycle::Dead,
                8 => {
                    let m = c.actors[slot].motion;
                    c.actors[slot].motion = MotionState::new(mornlea_domain::MotionStateParts {
                        position: m.position(),
                        velocity: m.velocity(),
                        on_ground: false,
                    });
                }
                9 => c.runtimes.get_mut(&ActorKey::Player(s)).unwrap().reset = true,
                _ => {}
            }
            c.environment = None;
            if row != 9 {
                c.runtimes.remove(&ActorKey::Player(s));
            }
            let before = ctx_death_snapshot(&c);
            assert_eq!(
                super::super::source_player_restore::checkpoint_safe(&book, &mut c, s),
                Ok(()),
                "row {row}"
            );
            assert_eq!(ctx_death_snapshot(&c), before, "row {row}");
            assert_eq!(
                book.entries
                    .keys()
                    .map(|s| ctx_book_snapshot(&book, *s))
                    .collect::<Vec<_>>(),
                scans
            );
        }
    }

    #[test]
    fn ctx_safe_checkpoint_refusals_and_abandonment() {
        for row in 0..10 {
            let (mut a, s, other) = ctx_safe_fixture(true);
            let other = other.unwrap();
            let book = std::mem::take(&mut a.source_players);
            let scan = ctx_book_snapshot(&book, s);
            let mut c = TickContext::for_tick(&mut a, TickBudget::full());
            let slot = c.player_slots[&s];
            match row {
                0 => {
                    c.player_slots.remove(&s);
                }
                1 => {
                    c.player_slots.insert(s, c.actors.len());
                }
                2 => c.actors[slot].key = ActorKey::Player(other),
                3 => {
                    c.actors[slot].body = ActorBody::Passive(mornlea_storage::PassiveMob {
                        id: 1,
                        dimension: 0,
                        position: [0., 64., 0.],
                        velocity: [0.; 3],
                        on_ground: true,
                        yaw: 0.,
                        health: 7,
                    })
                }
                4 => {
                    c.runtimes.remove(&ActorKey::Player(s));
                }
                5 => {
                    c.runtimes.get_mut(&ActorKey::Player(s)).unwrap().key = ActorKey::Player(other)
                }
                6 => {
                    c.runtimes.get_mut(&ActorKey::Player(s)).unwrap().aux = ActorAux::Hostile {
                        distant_ticks: 1,
                        shoot_cooldown: 2,
                        fresh: true,
                    }
                }
                7 => {
                    c.actors[slot].motion = MotionState::new(mornlea_domain::MotionStateParts {
                        position: mornlea_domain::FiniteVec3::try_new([f32::MAX, 64., 8.5])
                            .unwrap(),
                        velocity: mornlea_domain::FiniteVec3::try_new([0.; 3]).unwrap(),
                        on_ground: true,
                    })
                }
                8 | 9 => {
                    c.authority.source_player_radius = None;
                    c.authority.phase = ServerPhase::Closed;
                    if row == 9 {
                        c.authority.tick_failure = Some(ServerError::Disconnected);
                    }
                }
                _ => unreachable!(),
            }
            let error = match row {
                7 => ServerError::InvalidInput {
                    field: "actor_geometry",
                },
                8 => ServerError::InvalidState {
                    phase: ServerPhase::Closed,
                },
                9 => ServerError::Disconnected,
                _ => ctx_safe_error(),
            };
            ctx_safe_refuse(&mut c, s, error);
            assert_eq!(ctx_book_snapshot(&book, s), scan);
        }
        for row in 0..3 {
            let (mut a, s, _) = ctx_safe_fixture(false);
            let book = std::mem::take(&mut a.source_players);
            let scan = ctx_book_snapshot(&book, s);
            let mut c = TickContext::for_tick(&mut a, TickBudget::full());
            let slot = c.player_slots[&s];
            let pose = match row {
                0 => [15.9, 64., 8.5],
                1 => [8.5, 63., 8.5],
                _ => [8.5, 65., 8.5],
            };
            c.actors[slot].motion = MotionState::new(mornlea_domain::MotionStateParts {
                position: mornlea_domain::FiniteVec3::try_new(pose).unwrap(),
                velocity: mornlea_domain::FiniteVec3::try_new([0.; 3]).unwrap(),
                on_ground: true,
            });
            let before = ctx_death_snapshot(&c);
            assert_eq!(c.checkpoint_source_player_safe(s), Ok(()));
            assert_eq!(ctx_death_snapshot(&c), before);
            assert_eq!(ctx_book_snapshot(&book, s), scan);
        }
        let (mut a, s, other) = ctx_safe_fixture(true);
        let other = other.unwrap();
        ctx_add_allocations(&mut a, s);
        let book = std::mem::take(&mut a.source_players);
        let scans = [ctx_book_snapshot(&book, s), ctx_book_snapshot(&book, other)];
        let allocations =
            ctx_allocations(player(&a, s), &a.residents.runtimes[&ActorKey::Player(s)]);
        let other_before;
        {
            let mut c = TickContext::for_tick(&mut a, TickBudget::full());
            c.runtimes.get_mut(&ActorKey::Player(other)).unwrap().key = ActorKey::Player(s);
            other_before = (
                c.actors[c.player_slots[&other]].clone(),
                c.runtimes[&ActorKey::Player(other)].clone(),
            );
            assert_eq!(
                super::super::source_player_restore::checkpoint_safe(&book, &mut c, s),
                Ok(())
            );
            assert_eq!(
                ctx_safe_value(&c, s),
                Some(PlayerLocation {
                    dimension: 1,
                    position: [8.5, 64., 8.5]
                })
            );
            let accepted = ctx_death_snapshot(&c);
            assert_eq!(
                super::super::source_player_restore::checkpoint_safe(&book, &mut c, other),
                Err(ctx_safe_error())
            );
            assert_eq!(ctx_death_snapshot(&c), accepted);
            // Abandon the actual exclusive loan without committing or publishing.
        }
        a.source_players = book;
        let ActorBody::Player(body) = &player(&a, s).body else {
            unreachable!()
        };
        assert_eq!(
            body.safe,
            Some(PlayerLocation {
                dimension: 1,
                position: [8.5, 64., 8.5]
            })
        );
        assert_eq!(
            (
                player(&a, other),
                &a.residents.runtimes[&ActorKey::Player(other)]
            ),
            (&other_before.0, &other_before.1)
        );
        assert_eq!(
            ctx_allocations(player(&a, s), &a.residents.runtimes[&ActorKey::Player(s)]),
            allocations
        );
        assert_eq!(
            [
                ctx_book_snapshot(&a.source_players, s),
                ctx_book_snapshot(&a.source_players, other)
            ],
            scans
        );
    }
    fn ctx_trample_motion(position: [f32; 3], grounded: bool) -> MotionState {
        MotionState::new(mornlea_domain::MotionStateParts {
            position: mornlea_domain::FiniteVec3::try_new(position).unwrap(),
            velocity: mornlea_domain::FiniteVec3::try_new([0.; 3]).unwrap(),
            on_ground: grounded,
        })
    }
    fn ctx_trample_airborne(a: &mut AuthorityState, sessions: &[SessionKey]) {
        for s in sessions {
            a.residents.actors[a.residents.player_slots[s]].motion =
                ctx_trample_motion([15.9, 67., 15.9], false);
        }
    }
    fn ctx_trample_edge(c: &mut TickContext<'_>, s: SessionKey) {
        c.actors[c.player_slots[&s]].motion = ctx_trample_motion([15.9, 64., 15.9], true);
    }
    fn ctx_trample_positions() -> [BlockPos; 4] {
        [
            BlockPos::new(15, 63, 15),
            BlockPos::new(15, 63, 16),
            BlockPos::new(16, 63, 15),
            BlockPos::new(16, 63, 16),
        ]
    }
    #[test]
    fn ctx_trample_borrowed_edge_and_legacy_exclusion() {
        for healthless in [false, true] {
            let (mut a, s, _) = ctx_safe_fixture(false);
            ctx_add_allocations(&mut a, s);
            ctx_trample_airborne(&mut a, &[s]);
            let mut book = std::mem::take(&mut a.source_players);
            let scan = ctx_book_snapshot(&book, s);
            let mut c = TickContext::for_tick(&mut a, TickBudget::full());
            ctx_trample_edge(&mut c, s);
            ctx_seed_transients(&mut c, &[s]);
            if healthless {
                c.actors[c.player_slots[&s]].survival = ctx_survival(0, 9, 3);
                c.runtimes.get_mut(&ActorKey::Player(s)).unwrap().has_view = false;
            }
            let env = c.environment.take();
            c.ready.clear();
            c.blocks = Default::default();
            let before = ctx_death_snapshot(&c);
            let pointers = ctx_allocations(
                &c.actors[c.player_slots[&s]],
                &c.runtimes[&ActorKey::Player(s)],
            );
            assert_eq!(
                c.capture_source_player_trample(s),
                Ok(Some((Dimension::DEPTHS, ctx_trample_positions(), 4)))
            );
            super::super::source_player_restore::capture_trample(&mut book, &c, s).unwrap();
            let (cells, len) = book.trample_test_snapshot();
            assert_eq!(len, 4);
            for (cell, pos) in cells[..4].iter().zip(ctx_trample_positions()) {
                assert_eq!(
                    *cell,
                    crate::rules::crops::FootprintCell {
                        dimension: Dimension::DEPTHS,
                        pos
                    }
                );
            }
            assert_eq!(ctx_death_snapshot(&c), before);
            assert_eq!(ctx_book_snapshot(&book, s), scan);
            assert_eq!(
                ctx_allocations(
                    &c.actors[c.player_slots[&s]],
                    &c.runtimes[&ActorKey::Player(s)]
                ),
                pointers
            );
            c.environment = env;
            let before = ctx_death_snapshot(&c);
            let batch = book.trample_test_snapshot();
            let mut legacy = crate::rules::crops::FootprintSchedule::new();
            let r = crate::rules::crops::settle_tramples(&mut legacy, &mut c).unwrap();
            assert_eq!((r.examined, r.applied), (0, 0));
            assert_eq!(book.trample_test_snapshot(), batch);
            assert_eq!(ctx_death_snapshot(&c), before);
            c.authority.source_player_radius = None;
            let r = crate::rules::crops::settle_tramples(&mut legacy, &mut c).unwrap();
            assert_eq!((r.examined, r.applied), (4, 0));
        }
    }
    #[test]
    fn ctx_trample_ineligible_is_quiet() {
        for row in 0..12 {
            let (mut a, s) = if row == 1 {
                fixture()
            } else {
                let (a, s, _) = ctx_safe_fixture(false);
                (a, s)
            };
            ctx_trample_airborne(&mut a, &[s]);
            if row == 2 {
                a.source_player_radius = None;
            }
            if row == 3 {
                a.sessions.get_mut(&s).unwrap().phase = SessionPhase::Retired;
            }
            if row == 4 {
                a.sessions.remove(&s);
            }
            let mut book = std::mem::take(&mut a.source_players);
            if row == 0 {
                book.entries.remove(&s);
            }
            if row == 1 {
                assert!(!book.entries[&s].ever_spawned);
            }
            let scans = book
                .entries
                .keys()
                .map(|s| ctx_book_snapshot(&book, *s))
                .collect::<Vec<_>>();
            let mut c = TickContext::for_tick(&mut a, TickBudget::full());
            ctx_trample_edge(&mut c, s);
            let slot = c.player_slots[&s];
            match row {
                5 => c.actors[slot].lifecycle = ActorLifecycle::Pending,
                6 => c.actors[slot].lifecycle = ActorLifecycle::Respawning,
                7 => c.actors[slot].lifecycle = ActorLifecycle::Dead,
                8 => c.actors[slot].motion = ctx_trample_motion([15.9, 64., 15.9], false),
                9 => c.runtimes.get_mut(&ActorKey::Player(s)).unwrap().reset = true,
                10 => {
                    c.pre_step.remove(&ActorKey::Player(s));
                }
                11 => {
                    c.pre_step.insert(
                        ActorKey::Player(s),
                        ctx_trample_motion([15.9, 64., 15.9], true),
                    );
                }
                _ => {}
            }
            c.environment = None;
            c.ready.clear();
            c.blocks = Default::default();
            if (2..=4).contains(&row) {
                c.actors.clear();
                c.player_slots.clear();
            }
            if row <= 8 {
                c.runtimes.remove(&ActorKey::Player(s));
            }
            let before = ctx_death_snapshot(&c);
            let batch = book.trample_test_snapshot();
            if row > 1 {
                assert_eq!(c.capture_source_player_trample(s), Ok(None), "row {row}");
            }
            assert_eq!(
                super::super::source_player_restore::capture_trample(&mut book, &c, s),
                Ok(()),
                "row {row}"
            );
            assert_eq!(ctx_death_snapshot(&c), before);
            assert_eq!(book.trample_test_snapshot(), batch);
            assert_eq!(
                book.entries
                    .keys()
                    .map(|s| ctx_book_snapshot(&book, *s))
                    .collect::<Vec<_>>(),
                scans
            );
        }
    }
    #[test]
    fn ctx_trample_refusal_and_prefix_drop() {
        for row in 0..10 {
            let (mut a, s, other) = ctx_safe_fixture(true);
            let other = other.unwrap();
            ctx_trample_airborne(&mut a, &[s, other]);
            let mut book = std::mem::take(&mut a.source_players);
            let scan = ctx_book_snapshot(&book, s);
            let mut c = TickContext::for_tick(&mut a, TickBudget::full());
            ctx_trample_edge(&mut c, s);
            let slot = c.player_slots[&s];
            match row {
                0 => {
                    c.player_slots.remove(&s);
                }
                1 => {
                    c.player_slots.insert(s, c.actors.len());
                }
                2 => c.actors[slot].key = ActorKey::Player(other),
                3 => {
                    c.actors[slot].body = ActorBody::Passive(mornlea_storage::PassiveMob {
                        id: 1,
                        dimension: 0,
                        position: [0., 64., 0.],
                        velocity: [0.; 3],
                        on_ground: true,
                        yaw: 0.,
                        health: 7,
                    })
                }
                4 => {
                    c.runtimes.remove(&ActorKey::Player(s));
                }
                5 => {
                    c.runtimes.get_mut(&ActorKey::Player(s)).unwrap().key = ActorKey::Player(other)
                }
                6 => {
                    c.runtimes.get_mut(&ActorKey::Player(s)).unwrap().aux = ActorAux::Hostile {
                        distant_ticks: 1,
                        shoot_cooldown: 2,
                        fresh: true,
                    }
                }
                7 => c.actors[slot].motion = ctx_trample_motion([f32::MAX, 64., 8.5], true),
                8 | 9 => {
                    c.authority.phase = ServerPhase::Closed;
                    c.authority.source_player_radius = None;
                    if row == 9 {
                        c.authority.tick_failure = Some(ServerError::Disconnected);
                    }
                }
                _ => unreachable!(),
            }
            let err = match row {
                7 => ServerError::InvalidInput {
                    field: "actor_geometry",
                },
                8 => ServerError::InvalidState {
                    phase: ServerPhase::Closed,
                },
                9 => ServerError::Disconnected,
                _ => ServerError::InvalidInput {
                    field: "source_player_trample",
                },
            };
            let before = ctx_death_snapshot(&c);
            let batch = book.trample_test_snapshot();
            assert_eq!(c.capture_source_player_trample(s), Err(err));
            assert_eq!(
                super::super::source_player_restore::capture_trample(&mut book, &c, s),
                Err(err)
            );
            assert_eq!(ctx_death_snapshot(&c), before);
            assert_eq!(book.trample_test_snapshot(), batch);
            assert_eq!(ctx_book_snapshot(&book, s), scan);
        }
        let (mut a, s, other) = ctx_safe_fixture(true);
        let other = other.unwrap();
        ctx_add_allocations(&mut a, s);
        ctx_trample_airborne(&mut a, &[s, other]);
        let mut book = std::mem::take(&mut a.source_players);
        let scans = [ctx_book_snapshot(&book, s), ctx_book_snapshot(&book, other)];
        let pointers = ctx_allocations(player(&a, s), &a.residents.runtimes[&ActorKey::Player(s)]);
        let batch;
        let environment = a.residents.environment.clone();
        {
            let mut c = TickContext::for_tick(&mut a, TickBudget::full());
            ctx_trample_edge(&mut c, s);
            ctx_trample_edge(&mut c, other);
            super::super::source_player_restore::capture_trample(&mut book, &c, s).unwrap();
            batch = book.trample_test_snapshot();
            assert_eq!(batch.1, 4);
            c.runtimes.get_mut(&ActorKey::Player(other)).unwrap().key = ActorKey::Player(s);
            let before = ctx_death_snapshot(&c);
            assert_eq!(
                super::super::source_player_restore::capture_trample(&mut book, &c, other),
                Err(ServerError::InvalidInput {
                    field: "source_player_trample"
                })
            );
            assert_eq!(ctx_death_snapshot(&c), before);
            assert_eq!(book.trample_test_snapshot(), batch);
            c.environment = None;
            assert_eq!(
                super::super::source_player_restore::settle_tramples(&mut book, &mut c),
                Err(ServerError::InvalidInput {
                    field: "environment"
                })
            );
            assert_eq!(book.trample_test_snapshot(), batch);
        }
        a.source_players = book;
        a.residents.environment = environment;
        assert_eq!(a.source_players.trample_test_snapshot(), batch);
        assert_eq!(
            [
                ctx_book_snapshot(&a.source_players, s),
                ctx_book_snapshot(&a.source_players, other)
            ],
            scans
        );
        assert_eq!(
            ctx_allocations(player(&a, s), &a.residents.runtimes[&ActorKey::Player(s)]),
            pointers
        );
        let mut book = std::mem::take(&mut a.source_players);
        let mut c = TickContext::for_tick(&mut a, TickBudget::full());
        c.ready.clear();
        c.blocks = Default::default();
        let before = ctx_death_snapshot(&c);
        let r = super::super::source_player_restore::settle_tramples(&mut book, &mut c).unwrap();
        assert_eq!((r.examined, r.applied, r.carried, r.rejected), (4, 0, 0, 0));
        let drained = book.trample_test_snapshot();
        assert_eq!(drained.1, 0);
        assert_eq!(drained.0, batch.0);
        assert_eq!(ctx_death_snapshot(&c), before);
    }
    fn ctx_snow_before(a: &mut AuthorityState, sessions: &[SessionKey]) {
        for s in sessions {
            a.residents.actors[a.residents.player_slots[s]].motion =
                ctx_trample_motion([8., 64., 8.], true);
        }
    }
    fn ctx_snow_after(c: &mut TickContext<'_>, s: SessionKey) {
        c.actors[c.player_slots[&s]].motion = ctx_trample_motion([8.7, 64., 8.], true);
    }
    fn ctx_snow_error() -> ServerError {
        ServerError::InvalidInput {
            field: "source_player_snow",
        }
    }
    #[test]
    fn ctx_snow_borrowed_motion_and_legacy_exclusion() {
        for healthless in [false, true] {
            let (mut a, s, _) = ctx_safe_fixture(false);
            ctx_add_allocations(&mut a, s);
            ctx_snow_before(&mut a, &[s]);
            let mut book = std::mem::take(&mut a.source_players);
            let scan = ctx_book_snapshot(&book, s);
            let mut c = TickContext::for_tick(&mut a, TickBudget::full());
            ctx_snow_after(&mut c, s);
            ctx_seed_transients(&mut c, &[s]);
            if healthless {
                c.actors[c.player_slots[&s]].survival = ctx_survival(0, 9, 3);
                c.runtimes.get_mut(&ActorKey::Player(s)).unwrap().has_view = false;
            }
            c.environment = None;
            c.ready.clear();
            c.blocks = Default::default();
            let before = ctx_death_snapshot(&c);
            let pointers = ctx_allocations(
                &c.actors[c.player_slots[&s]],
                &c.runtimes[&ActorKey::Player(s)],
            );
            assert_eq!(
                c.capture_source_player_snow(s),
                Ok(Some((Dimension::DEPTHS, [8., 64., 8.], [8.7, 64., 8.])))
            );
            assert_eq!(ctx_death_snapshot(&c), before);
            super::super::source_player_restore::capture_snow(&mut book, &c, s).unwrap();
            let batch = book.snow_test_snapshot();
            assert_eq!(batch.1, 1);
            assert_eq!(
                batch.0[0],
                crate::rules::crops::FootprintCell {
                    dimension: Dimension::DEPTHS,
                    pos: BlockPos::new(8, 64, 8)
                }
            );
            assert_eq!(
                book.snow_test_state(s),
                Some((0., BlockPos::new(8, 64, 8), true))
            );
            assert_eq!(ctx_death_snapshot(&c), before);
            assert_eq!(ctx_book_snapshot(&book, s), scan);
            assert_eq!(
                ctx_allocations(
                    &c.actors[c.player_slots[&s]],
                    &c.runtimes[&ActorKey::Player(s)]
                ),
                pointers
            );
            let mut legacy = crate::rules::crops::FootprintSchedule::new();
            let r = crate::rules::crops::settle_snow_footprints(&mut legacy, &mut c).unwrap();
            assert_eq!((r.examined, r.applied), (0, 0));
            assert_eq!(book.snow_test_snapshot(), batch);
            assert_eq!(ctx_death_snapshot(&c), before);
            c.actors[c.player_slots[&s]].dimension = Dimension::OVERWORLD;
            super::super::source_player_restore::capture_snow(&mut book, &c, s).unwrap();
            assert_eq!(book.snow_test_snapshot(), batch);
            c.authority.source_player_radius = None;
            let r = crate::rules::crops::settle_snow_footprints(&mut legacy, &mut c).unwrap();
            assert_eq!((r.examined, r.applied), (1, 0));
        }
    }
    #[test]
    fn ctx_snow_quiet_and_refusal_precedence() {
        for row in 0..11 {
            let (mut a, s) = if row == 1 {
                fixture()
            } else {
                let (a, s, _) = ctx_safe_fixture(false);
                (a, s)
            };
            ctx_snow_before(&mut a, &[s]);
            if row == 2 {
                a.source_player_radius = None;
            }
            if row == 3 {
                a.sessions.get_mut(&s).unwrap().phase = SessionPhase::Retired;
            }
            if row == 4 {
                a.sessions.remove(&s);
            }
            let mut book = std::mem::take(&mut a.source_players);
            if row == 0 {
                book.entries.remove(&s);
            }
            if row == 1 {
                assert!(!book.entries[&s].ever_spawned);
            }
            let scans = book
                .entries
                .keys()
                .map(|s| ctx_book_snapshot(&book, *s))
                .collect::<Vec<_>>();
            if book.entries.contains_key(&s) {
                book.snow_test_set_state(s, 0.4, BlockPos::new(3, 64, 8), true);
            }
            let tracker = book.snow_test_state(s);
            let mut c = TickContext::for_tick(&mut a, TickBudget::full());
            ctx_snow_after(&mut c, s);
            let slot = c.player_slots[&s];
            match row {
                5 => c.actors[slot].lifecycle = ActorLifecycle::Pending,
                6 => c.actors[slot].lifecycle = ActorLifecycle::Respawning,
                7 => c.actors[slot].lifecycle = ActorLifecycle::Dead,
                8 => c.actors[slot].motion = ctx_trample_motion([8.7, 64., 8.], false),
                9 => c.runtimes.get_mut(&ActorKey::Player(s)).unwrap().reset = true,
                10 => {
                    c.pre_step.remove(&ActorKey::Player(s));
                }
                _ => {}
            }
            c.environment = None;
            c.ready.clear();
            c.blocks = Default::default();
            if (2..=4).contains(&row) {
                c.actors.clear();
                c.player_slots.clear();
            }
            if row <= 8 {
                c.runtimes.remove(&ActorKey::Player(s));
            }
            let before = ctx_death_snapshot(&c);
            let batch = book.snow_test_snapshot();
            if row > 1 {
                assert_eq!(c.capture_source_player_snow(s), Ok(None), "row {row}");
            }
            assert_eq!(
                super::super::source_player_restore::capture_snow(&mut book, &c, s),
                Ok(()),
                "row {row}"
            );
            assert_eq!(ctx_death_snapshot(&c), before);
            assert_eq!(book.snow_test_snapshot(), batch);
            assert_eq!(book.snow_test_state(s), tracker);
            assert_eq!(
                book.entries
                    .keys()
                    .map(|s| ctx_book_snapshot(&book, *s))
                    .collect::<Vec<_>>(),
                scans
            );
        }

        for row in 0..9 {
            let (mut a, s, other) = ctx_safe_fixture(true);
            let other = other.unwrap();
            ctx_snow_before(&mut a, &[s, other]);
            let mut book = std::mem::take(&mut a.source_players);
            let scan = ctx_book_snapshot(&book, s);
            if book.entries.contains_key(&s) {
                book.snow_test_set_state(s, 0.4, BlockPos::new(3, 64, 8), true);
            }
            let tracker = book.snow_test_state(s);
            let mut c = TickContext::for_tick(&mut a, TickBudget::full());
            ctx_snow_after(&mut c, s);
            let slot = c.player_slots[&s];
            match row {
                0 => {
                    c.player_slots.remove(&s);
                }
                1 => {
                    c.player_slots.insert(s, c.actors.len());
                }
                2 => c.actors[slot].key = ActorKey::Player(other),
                3 => {
                    c.actors[slot].body = ActorBody::Passive(mornlea_storage::PassiveMob {
                        id: 1,
                        dimension: 0,
                        position: [0., 64., 0.],
                        velocity: [0.; 3],
                        on_ground: true,
                        yaw: 0.,
                        health: 7,
                    })
                }
                4 => {
                    c.runtimes.remove(&ActorKey::Player(s));
                }
                5 => {
                    c.runtimes.get_mut(&ActorKey::Player(s)).unwrap().key = ActorKey::Player(other)
                }
                6 => {
                    c.runtimes.get_mut(&ActorKey::Player(s)).unwrap().aux = ActorAux::Hostile {
                        distant_ticks: 1,
                        shoot_cooldown: 2,
                        fresh: true,
                    }
                }
                7 | 8 => {
                    c.authority.phase = ServerPhase::Closed;
                    c.authority.source_player_radius = None;
                    if row == 8 {
                        c.authority.tick_failure = Some(ServerError::Disconnected);
                    }
                }
                _ => unreachable!(),
            }
            let err = match row {
                7 => ServerError::InvalidState {
                    phase: ServerPhase::Closed,
                },
                8 => ServerError::Disconnected,
                _ => ServerError::InvalidInput {
                    field: "source_player_snow",
                },
            };
            let before = ctx_death_snapshot(&c);
            let batch = book.snow_test_snapshot();
            assert_eq!(c.capture_source_player_snow(s), Err(err));
            assert_eq!(
                super::super::source_player_restore::capture_snow(&mut book, &c, s),
                Err(err)
            );
            assert_eq!(ctx_death_snapshot(&c), before);
            assert_eq!(book.snow_test_snapshot(), batch);
            assert_eq!(book.snow_test_state(s), tracker);
            assert_eq!(ctx_book_snapshot(&book, s), scan);
        }
    }

    #[test]
    fn ctx_snow_prefix_drop_and_atomic_capacity() {
        // Prepared context evidence qualifies copied-prefix ownership independently of native motion.
        let (mut a, s, other) = ctx_safe_fixture(true);
        let other = other.unwrap();
        ctx_add_allocations(&mut a, s);
        ctx_snow_before(&mut a, &[s, other]);
        let mut book = std::mem::take(&mut a.source_players);
        let scans = [ctx_book_snapshot(&book, s), ctx_book_snapshot(&book, other)];
        let pointers = ctx_allocations(player(&a, s), &a.residents.runtimes[&ActorKey::Player(s)]);
        let batch;
        {
            let mut c = TickContext::for_tick(&mut a, TickBudget::full());
            ctx_snow_after(&mut c, s);
            ctx_snow_after(&mut c, other);
            super::super::source_player_restore::capture_snow(&mut book, &c, s).unwrap();
            batch = book.snow_test_snapshot();
            assert_eq!(batch.1, 1);
            c.runtimes.get_mut(&ActorKey::Player(other)).unwrap().key = ActorKey::Player(s);
            let before = ctx_death_snapshot(&c);
            let trackers = [book.snow_test_state(s), book.snow_test_state(other)];
            assert_eq!(
                super::super::source_player_restore::capture_snow(&mut book, &c, other),
                Err(ctx_snow_error())
            );
            assert_eq!(ctx_death_snapshot(&c), before);
            assert_eq!(book.snow_test_snapshot(), batch);
            assert_eq!(
                [book.snow_test_state(s), book.snow_test_state(other)],
                trackers
            );
        }
        a.source_players = book;
        assert_eq!(a.source_players.snow_test_snapshot(), batch);
        assert_eq!(
            [
                ctx_book_snapshot(&a.source_players, s),
                ctx_book_snapshot(&a.source_players, other)
            ],
            scans
        );
        assert_eq!(
            ctx_allocations(player(&a, s), &a.residents.runtimes[&ActorKey::Player(s)]),
            pointers
        );
        let mut book = std::mem::take(&mut a.source_players);
        let tracker = book.snow_test_state(s);
        let mut c = TickContext::for_tick(&mut a, TickBudget::full());
        c.environment = None;
        c.ready.clear();
        c.blocks = Default::default();
        let before = ctx_death_snapshot(&c);
        let r = super::super::source_player_restore::settle_snow(&mut book, &mut c).unwrap();
        assert_eq!((r.examined, r.applied, r.carried, r.rejected), (1, 0, 0, 0));
        assert_eq!(book.snow_test_snapshot(), (batch.0, 0));
        assert_eq!(book.snow_test_state(s), tracker);
        assert_eq!(ctx_death_snapshot(&c), before);
        for i in 0..8 {
            c.pre_step.insert(
                ActorKey::Player(s),
                ctx_trample_motion([i as f32 + 10., 64., 8.], true),
            );
            c.actors[c.player_slots[&s]].motion =
                ctx_trample_motion([i as f32 + 10.7, 64., 8.], true);
            super::super::source_player_restore::capture_snow(&mut book, &c, s).unwrap();
        }
        let prefix = book.snow_test_snapshot();
        assert_eq!(prefix.1, 8);
        let tracker = book.snow_test_state(s);
        c.pre_step.insert(
            ActorKey::Player(s),
            ctx_trample_motion([30., 64., 8.], true),
        );
        c.actors[c.player_slots[&s]].motion = ctx_trample_motion([30.7, 64., 8.], true);
        let before = ctx_death_snapshot(&c);
        assert_eq!(
            super::super::source_player_restore::capture_snow(&mut book, &c, s),
            Err(ServerError::Internal {
                invariant: "source player snow capacity"
            })
        );
        assert_eq!(book.snow_test_snapshot(), prefix);
        assert_eq!(book.snow_test_state(s), tracker);
        assert_eq!(ctx_death_snapshot(&c), before);
    }
    #[test]
    fn ctx_snow_reset_clears_tracker_keeps_candidates() {
        for death in [false, true] {
            let (mut a, s, other) = ctx_safe_fixture(true);
            let other = other.unwrap();
            ctx_snow_before(&mut a, &[s, other]);
            let mut book = std::mem::take(&mut a.source_players);
            book.snow_test_set_state(other, 0.4, BlockPos::new(3, 64, 8), true);
            let other_scan = ctx_book_snapshot(&book, other);
            let mut c = TickContext::for_tick(&mut a, TickBudget::full());
            ctx_snow_after(&mut c, s);
            super::super::source_player_restore::capture_snow(&mut book, &c, s).unwrap();
            let batch = book.snow_test_snapshot();
            assert_eq!(batch.1, 1);
            book.snow_test_set_state(s, 0.55, BlockPos::new(4, 64, 8), true);
            let quiet = book.snow_test_state(s);
            assert!(!super::super::source_player_restore::recover(&mut book, &mut c, s).unwrap());
            super::super::source_player_restore::settle_deaths(&mut book, &mut c).unwrap();
            assert_eq!(book.snow_test_state(s), quiet);
            let radius = c.authority.source_player_radius.take();
            assert_eq!(
                super::super::source_player_restore::recover(&mut book, &mut c, s),
                Err(ServerError::InvalidInput {
                    field: "source_player_reset"
                })
            );
            assert_eq!(book.snow_test_state(s), quiet);
            c.authority.source_player_radius = radius;
            if death {
                c.actors[c.player_slots[&s]].survival = ctx_survival(0, 20, 300);
                c.inventories
                    .get_mut(&ActorKey::Player(s))
                    .unwrap()
                    .slots
                    .fill(Default::default());
                c.inventories
                    .get_mut(&ActorKey::Player(s))
                    .unwrap()
                    .crafting
                    .fill(Default::default());
                super::super::source_player_restore::settle_deaths(&mut book, &mut c).unwrap();
            } else {
                c.actors[c.player_slots[&s]].motion = ctx_trample_motion([8.7, -81., 8.], true);
                assert!(
                    super::super::source_player_restore::recover(&mut book, &mut c, s).unwrap()
                );
            }
            assert_eq!(book.snow_test_state(s), Some((0., BlockPos::ORIGIN, false)));
            assert_eq!(book.snow_test_snapshot(), batch);
            assert_eq!(
                book.snow_test_state(other),
                Some((0.4, BlockPos::new(3, 64, 8), true))
            );
            assert_eq!(ctx_book_snapshot(&book, other), other_scan);
        }
    }
    fn ctx_action_seed(
        c: &mut TickContext<'_>,
        session: SessionKey,
        saturation: u32,
        exhaustion: u32,
    ) {
        let slot = c.player_slots[&session];
        c.actors[slot].survival = ctx_survival(20, 20, 300);
        let ActorBody::Player(body) = &mut c.actors[slot].body else {
            unreachable!()
        };
        body.hunger = 20;
        body.saturation_milli = saturation.min(65535) as u16;
        body.exhaustion_milli = exhaustion.min(65535) as u16;
        let r = c.runtimes.get_mut(&ActorKey::Player(session)).unwrap();
        r.saturation_milli = saturation;
        r.exhaustion_milli = exhaustion;
        r.reset = false;
    }
    fn ctx_action_lanes(c: &TickContext<'_>, session: SessionKey) -> (u8, u32, u32) {
        let a = &c.actors[c.player_slots[&session]];
        let r = &c.runtimes[&ActorKey::Player(session)];
        let ActorBody::Player(body) = &a.body else {
            unreachable!()
        };
        assert_eq!(
            (
                body.hunger,
                u32::from(body.saturation_milli),
                u32::from(body.exhaustion_milli)
            ),
            (a.survival.hunger(), r.saturation_milli, r.exhaustion_milli)
        );
        assert_eq!(a.survival.saturation_zero(), r.saturation_milli == 0);
        (a.survival.hunger(), r.saturation_milli, r.exhaustion_milli)
    }
    fn ctx_action_threshold(threshold: u16) -> RuleTunables {
        let d = RuleTunables::source_defaults();
        RuleTunables::try_new(
            d.physics(),
            d.regen_delay_ticks(),
            d.regen_interval_ticks(),
            d.drown_interval_ticks(),
            d.starvation_interval_ticks(),
            d.regen_hunger_threshold(),
            threshold,
            d.eating_ticks(),
            d.furnace_burn_ticks(),
            d.furnace_smelt_ticks(),
            d.fluid_delay(),
            d.random_attempts(),
            d.crop_growth_percent(),
            d.interaction_reach(),
            d.eye_height(),
            d.drop_pickup_delay_ticks(),
            d.player_drop_pickup_delay_ticks(),
            d.drop_lifetime_ticks(),
            d.drop_pickup_range(),
        )
        .unwrap()
    }
    #[test]
    fn ctx_action_costs_scalar_borrow_and_order() {
        let (mut a, s, other) = ctx_safe_fixture(true);
        let other = other.unwrap();
        ctx_add_allocations(&mut a, s);
        let book = std::mem::take(&mut a.source_players);
        let scans = [ctx_book_snapshot(&book, s), ctx_book_snapshot(&book, other)];
        let mut c = TickContext::for_tick(&mut a, TickBudget::full());
        c.freeze_environment(0);
        ctx_seed_transients(&mut c, &[s, other]);
        ctx_action_seed(&mut c, s, 500, 3999);
        c.charges = vec![
            (ActorKey::Player(s), ActionKind::Till),
            (ActorKey::Player(other), ActionKind::Melee),
            (ActorKey::Player(s), ActionKind::Mining),
            (ActorKey::Player(other), ActionKind::Till),
        ];
        let pointers = ctx_allocations(
            &c.actors[c.player_slots[&s]],
            &c.runtimes[&ActorKey::Player(s)],
        );
        let before = ctx_death_snapshot(&c);
        let old_actor = c.actors[c.player_slots[&s]].clone();
        let old_runtime = c.runtimes[&ActorKey::Player(s)].clone();
        let old_charges = c.charges.clone();
        let report = c.settle_source_player_action_costs(s).unwrap();
        assert_eq!((report.examined, report.applied), (2, 2));
        assert_eq!(ctx_action_lanes(&c, s), (20, 0, 9));
        assert_eq!(
            c.charges,
            vec![
                (ActorKey::Player(other), ActionKind::Melee),
                (ActorKey::Player(other), ActionKind::Till)
            ]
        );
        assert_eq!(
            ctx_allocations(
                &c.actors[c.player_slots[&s]],
                &c.runtimes[&ActorKey::Player(s)]
            ),
            pointers
        );
        let accepted_survival = c.actors[c.player_slots[&s]].survival;
        let accepted_charges = c.charges.clone();
        assert_eq!(
            (
                accepted_survival.health(),
                accepted_survival.oxygen(),
                accepted_survival.armor_points()
            ),
            (
                old_actor.survival.health(),
                old_actor.survival.oxygen(),
                old_actor.survival.armor_points()
            )
        );
        c.actors[c.player_slots[&s]].survival = old_actor.survival;
        let ActorBody::Player(body) = &mut c.actors[c.player_slots[&s]].body else {
            unreachable!()
        };
        let ActorBody::Player(old_body) = &old_actor.body else {
            unreachable!()
        };
        body.hunger = old_body.hunger;
        body.saturation_milli = old_body.saturation_milli;
        body.exhaustion_milli = old_body.exhaustion_milli;
        let runtime = c.runtimes.get_mut(&ActorKey::Player(s)).unwrap();
        runtime.saturation_milli = old_runtime.saturation_milli;
        runtime.exhaustion_milli = old_runtime.exhaustion_milli;
        c.charges = old_charges;
        assert_eq!(ctx_death_snapshot(&c), before);
        c.actors[c.player_slots[&s]].survival = accepted_survival;
        let ActorBody::Player(body) = &mut c.actors[c.player_slots[&s]].body else {
            unreachable!()
        };
        body.hunger = 20;
        body.saturation_milli = 0;
        body.exhaustion_milli = 9;
        let runtime = c.runtimes.get_mut(&ActorKey::Player(s)).unwrap();
        runtime.saturation_milli = 0;
        runtime.exhaustion_milli = 9;
        c.charges = accepted_charges;
        let accepted = ctx_death_snapshot(&c);
        assert_eq!(
            c.settle_source_player_action_costs(s),
            Ok(PhaseReport {
                examined: 0,
                applied: 0,
                carried: 0,
                rejected: 0
            })
        );
        assert_eq!(ctx_death_snapshot(&c), accepted);
        assert_eq!(
            [ctx_book_snapshot(&book, s), ctx_book_snapshot(&book, other)],
            scans
        );
    }
    #[test]
    fn ctx_action_costs_source_threshold_table() {
        for kind in [ActionKind::Till, ActionKind::Mining] {
            for (sat, exh, threshold, want) in [
                (500, 3999, 4000, (20, 0, 4)),
                (0, 3999, 4000, (19, 0, 4)),
                (500, 3999, 8000, (20, 500, 4004)),
                (500, 3999, 0, (0, 0, 0)),
                (500, 3999, 1, (0, 0, 0)),
                (500, 65535, 65535, (20, 0, 5)),
                (u32::MAX, u32::MAX, 4000, (20, 49535, 1540)),
            ] {
                let (mut a, s, _) = ctx_safe_fixture(false);
                let mut c = TickContext::for_tick(&mut a, TickBudget::full());
                c.freeze_environment(0);
                c.environment.as_mut().unwrap().tunables = ctx_action_threshold(threshold);
                ctx_action_seed(&mut c, s, sat, exh);
                c.charges = vec![(ActorKey::Player(s), kind)];
                let health = c.actors[c.player_slots[&s]].survival.health();
                let r = c.settle_source_player_action_costs(s).unwrap();
                assert_eq!((r.examined, r.applied), (1, 1));
                assert_eq!(
                    ctx_action_lanes(&c, s),
                    want,
                    "{kind:?} threshold {threshold}"
                );
                assert_eq!(c.actors[c.player_slots[&s]].survival.health(), health);
                assert!(c.charges.is_empty());
            }
        }
    }
    #[test]
    fn ctx_action_costs_quiet_and_refusal_precedence() {
        for row in 0..25 {
            let (mut a, s, other) = ctx_safe_fixture(true);
            let other = other.unwrap();
            let mut book = std::mem::take(&mut a.source_players);
            let mut c = TickContext::for_tick(&mut a, TickBudget::full());
            c.freeze_environment(0);
            ctx_action_seed(&mut c, s, 500, 3999);
            let slot = c.player_slots[&s];
            c.charges = vec![(ActorKey::Player(s), ActionKind::Till)];
            let invalid = ServerError::InvalidInput {
                field: "source_player_action_costs",
            };
            let expected = match row {
                0 => {
                    c.authority.source_player_radius = None;
                    c.player_slots.remove(&s);
                    Ok(PhaseReport {
                        examined: 0,
                        applied: 0,
                        carried: 0,
                        rejected: 0,
                    })
                }
                1 => {
                    c.authority.sessions.remove(&s);
                    c.player_slots.remove(&s);
                    Ok(PhaseReport {
                        examined: 0,
                        applied: 0,
                        carried: 0,
                        rejected: 0,
                    })
                }
                2 => {
                    c.authority.sessions.get_mut(&s).unwrap().phase = SessionPhase::Retired;
                    Ok(PhaseReport {
                        examined: 0,
                        applied: 0,
                        carried: 0,
                        rejected: 0,
                    })
                }
                3..=5 => {
                    c.actors[slot].lifecycle = match row {
                        3 => ActorLifecycle::Pending,
                        4 => ActorLifecycle::Respawning,
                        _ => ActorLifecycle::Dead,
                    };
                    c.charges
                        .resize(4097, (ActorKey::Player(s), ActionKind::Till));
                    Ok(PhaseReport {
                        examined: 0,
                        applied: 0,
                        carried: 0,
                        rejected: 0,
                    })
                }
                6 => {
                    c.charges.clear();
                    c.runtimes.remove(&ActorKey::Player(s));
                    c.environment = None;
                    Ok(PhaseReport {
                        examined: 0,
                        applied: 0,
                        carried: 0,
                        rejected: 0,
                    })
                }
                7 => {
                    c.runtimes.get_mut(&ActorKey::Player(s)).unwrap().reset = true;
                    c.environment = None;
                    Ok(PhaseReport {
                        examined: 0,
                        applied: 0,
                        carried: 0,
                        rejected: 0,
                    })
                }
                8 => {
                    book.entries.remove(&s);
                    book.entries.remove(&other);
                    Ok(PhaseReport {
                        examined: 0,
                        applied: 0,
                        carried: 0,
                        rejected: 0,
                    })
                }
                9 => {
                    for e in book.entries.values_mut() {
                        e.ever_spawned = false;
                    }
                    Ok(PhaseReport {
                        examined: 0,
                        applied: 0,
                        carried: 0,
                        rejected: 0,
                    })
                }
                10 => {
                    c.authority.tick_failure = Some(ServerError::Disconnected);
                    c.authority.phase = ServerPhase::Closed;
                    Err(ServerError::Disconnected)
                }
                11 => {
                    c.authority.phase = ServerPhase::Closed;
                    c.authority.source_player_radius = None;
                    Err(ServerError::InvalidState {
                        phase: ServerPhase::Closed,
                    })
                }
                12 => {
                    c.player_slots.remove(&s);
                    Err(invalid)
                }
                13 => {
                    c.player_slots.insert(s, c.actors.len());
                    Err(invalid)
                }
                14 => {
                    c.actors[slot].key = ActorKey::Player(other);
                    Err(invalid)
                }
                15..=19 => {
                    c.charges = vec![(ActorKey::Player(other), ActionKind::Till); 4097];
                    match row {
                        15 => {}
                        16 => {
                            c.actors[slot].body = c.actors[c.player_slots[&other]].body.clone();
                            if let ActorBody::Player(b) = &c.actors[slot].body {
                                c.actors[slot].body =
                                    ActorBody::Passive(mornlea_storage::PassiveMob {
                                        id: 1,
                                        dimension: 0,
                                        position: b.current.position,
                                        velocity: [0.; 3],
                                        on_ground: true,
                                        yaw: 0.,
                                        health: 7,
                                    });
                            }
                        }
                        17 => {
                            c.runtimes.remove(&ActorKey::Player(s));
                        }
                        18 => c.runtimes.get_mut(&ActorKey::Player(s)).unwrap().reset = true,
                        _ => c.environment = None,
                    };
                    Err(ServerError::Capacity {
                        resource: Resource::Commands,
                        limit: 4096,
                        observed: 4097,
                    })
                }
                20 => {
                    c.actors[slot].body = ActorBody::Passive(mornlea_storage::PassiveMob {
                        id: 1,
                        dimension: 0,
                        position: [0., 64., 0.],
                        velocity: [0.; 3],
                        on_ground: true,
                        yaw: 0.,
                        health: 7,
                    });
                    Err(invalid)
                }
                21 => {
                    c.runtimes.remove(&ActorKey::Player(s));
                    Err(invalid)
                }
                22 => {
                    c.runtimes.get_mut(&ActorKey::Player(s)).unwrap().key = ActorKey::Player(other);
                    Err(invalid)
                }
                23 => {
                    c.runtimes.get_mut(&ActorKey::Player(s)).unwrap().aux = ActorAux::Hostile {
                        distant_ticks: 1,
                        shoot_cooldown: 2,
                        fresh: true,
                    };
                    Err(invalid)
                }
                _ => {
                    c.environment = None;
                    Err(ServerError::Internal {
                        invariant: "tick environment snapshot",
                    })
                }
            };
            let book_before = book
                .entries
                .keys()
                .map(|s| ctx_book_snapshot(&book, *s))
                .collect::<Vec<_>>();
            let before = ctx_death_snapshot(&c);
            let result = if row == 8 || row == 9 {
                super::super::source_player_restore::settle_action_costs(&book, &mut c)
            } else {
                c.settle_source_player_action_costs(s)
            };
            assert_eq!(result, expected, "row {row}");
            assert_eq!(ctx_death_snapshot(&c), before, "row {row}");
            assert_eq!(
                book.entries
                    .keys()
                    .map(|s| ctx_book_snapshot(&book, *s))
                    .collect::<Vec<_>>(),
                book_before
            );
        }
    }
    #[test]
    fn ctx_action_costs_accepted_prefix_survives_drop() {
        let (mut a, s, other) = ctx_safe_fixture(true);
        let other = other.unwrap();
        ctx_add_allocations(&mut a, s);
        let book = std::mem::take(&mut a.source_players);
        let scans = [ctx_book_snapshot(&book, s), ctx_book_snapshot(&book, other)];
        let pointers = ctx_allocations(player(&a, s), &a.residents.runtimes[&ActorKey::Player(s)]);
        {
            let mut c = TickContext::for_tick(&mut a, TickBudget::full());
            c.freeze_environment(0);
            ctx_action_seed(&mut c, s, 500, 3999);
            ctx_action_seed(&mut c, other, 500, 3999);
            let foreign = ActorKey::Player(SessionKey::from_raw(99).unwrap());
            c.charges = vec![
                (foreign, ActionKind::Melee),
                (ActorKey::Player(s), ActionKind::Till),
                (ActorKey::Player(other), ActionKind::Mining),
                (foreign, ActionKind::Till),
                (ActorKey::Player(s), ActionKind::Mining),
            ];
            c.runtimes.get_mut(&ActorKey::Player(other)).unwrap().key = ActorKey::Player(s);
            assert_eq!(
                super::super::source_player_restore::settle_action_costs(&book, &mut c),
                Err(ServerError::InvalidInput {
                    field: "source_player_action_costs"
                })
            );
            assert_eq!(ctx_action_lanes(&c, s), (20, 0, 9));
            assert_eq!(
                c.charges,
                vec![
                    (foreign, ActionKind::Melee),
                    (ActorKey::Player(other), ActionKind::Mining),
                    (foreign, ActionKind::Till)
                ]
            );
        }
        a.source_players = book;
        let r = &a.residents.runtimes[&ActorKey::Player(s)];
        assert_eq!(
            (
                player(&a, s).survival.hunger(),
                r.saturation_milli,
                r.exhaustion_milli
            ),
            (20, 0, 9)
        );
        assert_eq!(ctx_allocations(player(&a, s), r), pointers);
        assert_eq!(
            [
                ctx_book_snapshot(&a.source_players, s),
                ctx_book_snapshot(&a.source_players, other)
            ],
            scans
        );
    }
}

#[cfg(test)]
mod live_collision_read_tests {
    use super::super::{acquisition::LiveChunkPhase, actor_placement, world};
    use super::*;
    use mornlea_domain::{BlockPos, ChunkPos};
    use mornlea_engine::native::contracts::collision::{CollisionCell, CollisionGrid};
    use mornlea_storage::{ContainerSnapshot, StorageKind};

    fn authority() -> AuthorityState {
        AuthorityState::try_new(
            ServerLimits::try_new(8, 4096, 512, 64, 64, 1_048_576).unwrap(),
            42,
        )
        .unwrap()
    }

    fn key(dimension: Dimension) -> ChunkKey {
        ChunkKey {
            dimension,
            pos: ChunkPos::new(0, 0),
        }
    }

    fn boundary_chunk(block: u16) -> Chunk {
        let mut chunk = Chunk {
            sections: vec![
                ContainerSnapshot {
                    kind: StorageKind::Single,
                    bits: 0,
                    single: 0,
                    palette: vec![],
                    packed: vec![],
                };
                24
            ],
            drops: vec![Default::default(); 32],
            furnaces: vec![Default::default(); 32],
            chests: vec![Default::default(); 16],
        };
        chunk.sections[0].single = block;
        chunk.sections[23].single = block;
        chunk
    }

    // Manual prepared offers qualify the read owner, not disk or driver integration.
    fn queue(a: &mut AuthorityState, dimension: Dimension, block: u16, request: u64) {
        let key = key(dimension);
        let reservation = a.reserve_chunk_load(key).unwrap();
        let request = ChunkRequestId::try_new(request).unwrap();
        a.bind_chunk_load(reservation, request).unwrap();
        let prepared = world::PreparedChunk::try_new(
            key,
            reservation.generation(),
            RecoveredChunk {
                chunk: boundary_chunk(block),
                revision: 9,
                persisted_revision: 9,
                needs_rewrite: false,
                recovered: false,
            },
        )
        .unwrap();
        a.offer_acquired(AcquiredChunkEvent::Load {
            key,
            generation: reservation.generation(),
            request,
            result: Ok(Some(prepared)),
        })
        .unwrap();
    }

    fn live(dimensions: &[Dimension]) -> AuthorityState {
        let mut a = authority();
        a.enable_live_chunks().unwrap();
        a.replace_chunk_wants(dimensions.iter().copied().map(key).collect())
            .unwrap();
        for (index, dimension) in dimensions.iter().copied().enumerate() {
            queue(&mut a, dimension, index as u16 + 1, index as u64 + 1);
        }
        a.advance_tick(TickBudget::full()).unwrap();
        a
    }

    fn sample(view: &AuthorityReadView<'_>, dimension: Dimension) -> [Option<u16>; 4] {
        [-65, -64, 319, 320].map(|y| view.live_collision_block(dimension, BlockPos::new(8, y, 8)))
    }

    #[test]
    fn managed_height_precedes_unknown_horizontal_without_changing_owners() {
        let a = live(&[Dimension::OVERWORLD]);
        let ready = &a.residents.ready[&key(Dimension::OVERWORLD)];
        let identity = (
            std::ptr::from_ref(ready),
            ready.generation,
            ready.revision,
            ready.block(BlockPos::new(8, 319, 8)),
            ready.height(8, 8),
        );
        let metadata = a.metadata.clone();
        let facts = a.live_chunk_facts(key(Dimension::OVERWORLD));
        let tick = a.next_tick();
        world::reset_ready_clones();
        world::reset_materializations();
        for _ in 0..50 {
            let view = a.settled_read().unwrap();
            for (x, z, known) in [(8, 8, true), (144, -48, false)] {
                for y in [-65, -64, 319, 320, i32::MIN, i32::MAX] {
                    let expected = if !(-64..320).contains(&y) {
                        Some(0)
                    } else if known {
                        Some(1)
                    } else {
                        None
                    };
                    assert_eq!(
                        view.live_collision_block(Dimension::OVERWORLD, BlockPos::new(x, y, z)),
                        expected,
                        "horizontal ({x},{z}), y {y}"
                    );
                }
            }
        }
        assert_eq!((world::ready_clones(), world::materializations()), (0, 0));
        let current = &a.residents.ready[&key(Dimension::OVERWORLD)];
        assert_eq!(
            (
                std::ptr::from_ref(current),
                current.generation,
                current.revision,
                current.block(BlockPos::new(8, 319, 8)),
                current.height(8, 8)
            ),
            identity
        );
        assert_eq!(a.metadata, metadata);
        assert_eq!(a.next_tick(), tick);
        assert_eq!(a.live_chunk_facts(key(Dimension::OVERWORLD)), facts);
    }

    #[test]
    fn managed_loading_ready_dimensions_and_retained_unloading() {
        let mut a = authority();
        a.enable_live_chunks().unwrap();
        a.replace_chunk_wants(BTreeSet::from([
            key(Dimension::OVERWORLD),
            key(Dimension::DEPTHS),
        ]))
        .unwrap();
        for (dimension, block, request) in [(Dimension::OVERWORLD, 1, 1), (Dimension::DEPTHS, 2, 2)]
        {
            queue(&mut a, dimension, block, request);
            assert_eq!(
                a.live_chunk_facts(key(dimension)).unwrap().phase,
                LiveChunkPhase::Loading
            );
            assert_eq!(
                sample(&a.settled_read().unwrap(), dimension),
                [Some(0), None, None, Some(0)]
            );
        }
        a.advance_tick(TickBudget::full()).unwrap();
        for (dimension, block) in [(Dimension::OVERWORLD, 1), (Dimension::DEPTHS, 2)] {
            assert_eq!(
                a.live_chunk_facts(key(dimension)).unwrap().phase,
                LiveChunkPhase::Ready
            );
            assert_eq!(
                sample(&a.settled_read().unwrap(), dimension),
                [Some(0), Some(block), Some(block), Some(0)]
            );
            assert_eq!(
                a.settled_read()
                    .unwrap()
                    .live_collision_block(dimension, BlockPos::new(144, 319, -48)),
                None
            );
        }
        let retained = &a.residents.ready[&key(Dimension::OVERWORLD)];
        let before = (
            std::ptr::from_ref(retained),
            retained.generation,
            retained.revision,
            retained.block(BlockPos::new(8, 319, 8)),
        );
        a.replace_chunk_wants(BTreeSet::from([key(Dimension::DEPTHS)]))
            .unwrap();
        assert_eq!(
            a.live_chunk_facts(key(Dimension::OVERWORLD)).unwrap().phase,
            LiveChunkPhase::Unloading
        );
        let retained = &a.residents.ready[&key(Dimension::OVERWORLD)];
        assert_eq!(
            (
                std::ptr::from_ref(retained),
                retained.generation,
                retained.revision,
                retained.block(BlockPos::new(8, 319, 8))
            ),
            before
        );
        assert_eq!(
            sample(&a.settled_read().unwrap(), Dimension::OVERWORLD),
            [Some(0), None, None, Some(0)]
        );
        assert_eq!(
            sample(&a.settled_read().unwrap(), Dimension::DEPTHS),
            [Some(0), Some(2), Some(2), Some(0)]
        );
    }

    fn bounded_current_reads(view: &AuthorityReadView<'_>, pos: BlockPos) {
        let key = key(Dimension::OVERWORLD);
        let ready = &view.ready[&key];
        let observed = view.blocks.get(&(key, pos)).unwrap();
        let ready_identity = (std::ptr::from_ref(ready), ready.generation, ready.revision);
        let node_identity = (std::ptr::from_ref(observed), *observed);
        world::reset_ready_clones();
        world::reset_materializations();
        for _ in 0..50 {
            assert_eq!(
                view.live_collision_block(Dimension::OVERWORLD, pos),
                Some(2)
            );
            assert_eq!(
                view.observation(Dimension::OVERWORLD, pos).unwrap().block,
                2
            );
            assert_eq!(view.ready_chunk_revision(key), Some(10));
            for y in [-65, 320, i32::MIN, i32::MAX] {
                let outside = BlockPos::new(8, y, 8);
                assert_eq!(
                    view.live_collision_block(Dimension::OVERWORLD, outside),
                    Some(0)
                );
                assert_eq!(view.block(Dimension::OVERWORLD, outside), None);
                assert_eq!(view.observation(Dimension::OVERWORLD, outside), None);
            }
        }
        assert_eq!((world::ready_clones(), world::materializations()), (0, 0));
        let current = &view.ready[&key];
        assert_eq!(
            (
                std::ptr::from_ref(current),
                current.generation,
                current.revision
            ),
            ready_identity
        );
        let observed = view.blocks.get(&(key, pos)).unwrap();
        assert_eq!((std::ptr::from_ref(observed), *observed), node_identity);
    }

    #[test]
    fn actual_mutation_is_current_before_and_after_retained_commit() {
        let mut a = live(&[Dimension::OVERWORLD]);
        let pos = BlockPos::new(8, 319, 8);
        {
            let mut context = TickContext::for_tick(&mut a, TickBudget::full());
            for y in [-65, 320] {
                let outside = BlockPos::new(8, y, 8);
                assert_eq!(
                    context
                        .read()
                        .live_collision_block(Dimension::OVERWORLD, outside),
                    Some(0)
                );
                assert_eq!(context.read().block(Dimension::OVERWORLD, outside), None);
                assert_eq!(
                    context.read().observation(Dimension::OVERWORLD, outside),
                    None
                );
            }
            let old = context
                .read()
                .observation(Dimension::OVERWORLD, pos)
                .unwrap();
            assert_eq!(old.block, 1);
            context
                .transaction()
                .try_system(
                    SystemRule::Support,
                    vec![BlockWrite::try_new(old, 2).unwrap()],
                )
                .unwrap();
            bounded_current_reads(&context.read(), pos);
            context.commit_carried();
        }
        assert_eq!(a.residents.ready[&key(Dimension::OVERWORLD)].revision, 10);
        bounded_current_reads(&a.settled_read().unwrap(), pos);
    }

    #[test]
    fn disabled_sparse_fixtures_and_source_radius_keep_observation_mode() {
        let mut a = authority();
        let pos = BlockPos::new(8, 64, 8);
        let observed = BlockObservation::try_new(key(Dimension::OVERWORLD), 7, 9, pos, 2).unwrap();
        a.residents.blocks.insert((observed.key, pos), observed);
        assert!(a.residents.ready.is_empty());
        let check = |view: AuthorityReadView<'_>| {
            assert!(view.acquisition.is_none());
            assert_eq!(view.observation(Dimension::OVERWORLD, pos), Some(observed));
            assert_eq!(
                view.live_collision_block(Dimension::OVERWORLD, pos),
                Some(2)
            );
            assert_eq!(
                view.live_collision_block(Dimension::OVERWORLD, BlockPos::new(9, 64, 8)),
                None
            );
            for y in [-65, 320, i32::MIN, i32::MAX] {
                assert_eq!(
                    view.live_collision_block(Dimension::OVERWORLD, BlockPos::new(8, y, 8)),
                    None
                );
            }
        };
        check(a.settled_read().unwrap());
        {
            let context = TickContext::for_tick(&mut a, TickBudget::full());
            check(context.read());
        }
        let mut radius_only = authority();
        radius_only.enable_source_player_restoration(1).unwrap();
        assert_eq!(radius_only.source_player_radius, Some(1));
        assert!(radius_only.settled_read().unwrap().acquisition.is_none());
        for y in [-65, 320, i32::MIN, i32::MAX] {
            assert_eq!(
                radius_only
                    .settled_read()
                    .unwrap()
                    .live_collision_block(Dimension::OVERWORLD, BlockPos::new(8, y, 8)),
                None
            );
        }
    }

    #[test]
    fn settled_and_context_reads_agree_with_healthy_closing() {
        let mut a = live(&[Dimension::OVERWORLD]);
        let tick = a.next_tick();
        let time = a.settled_read().unwrap().world_time();
        let settled = sample(&a.settled_read().unwrap(), Dimension::OVERWORLD);
        assert_eq!(settled, [Some(0), Some(1), Some(1), Some(0)]);
        {
            let context = TickContext::for_tick(&mut a, TickBudget::full());
            assert_eq!(sample(&context.read(), Dimension::OVERWORLD), settled);
            assert_eq!(context.read().tick(), tick);
            assert_eq!(context.read().world_time(), time);
        }
        a.phase = ServerPhase::Closing;
        let closing = a.settled_read().unwrap();
        assert_eq!(sample(&closing, Dimension::OVERWORLD), settled);
        assert_eq!(closing.tick(), tick);
        assert_eq!(closing.world_time(), time);
        assert_eq!(a.next_tick(), tick);
    }

    #[test]
    fn trace_delegation_preserves_entries_and_overflow_outside_bypasses_it() {
        let a = live(&[Dimension::OVERWORLD]);
        let view = a.settled_read().unwrap();
        let outside = BlockPos::new(144, 320, -48);
        let known = BlockPos::new(8, 319, 8);
        let missing = BlockPos::new(144, 319, -48);
        let observed = view.observation(Dimension::OVERWORLD, known).unwrap();
        let trace = RefCell::new(ObservationTrace::default());
        let traced = view.with_observation_trace(&trace);
        assert_eq!(
            traced.live_collision_block(Dimension::OVERWORLD, outside),
            Some(0)
        );
        assert!(trace.borrow().cells.is_empty());
        assert!(!trace.borrow().overflow);
        for _ in 0..2 {
            assert_eq!(
                traced.live_collision_block(Dimension::OVERWORLD, known),
                Some(1)
            );
            assert_eq!(
                traced.live_collision_block(Dimension::OVERWORLD, missing),
                None
            );
        }
        assert_eq!(
            trace.borrow().cells,
            BTreeMap::from([
                ((Dimension::OVERWORLD, known), Some(observed)),
                ((Dimension::OVERWORLD, missing), None),
            ])
        );
        assert!(!trace.borrow().overflow);
        let full = RefCell::new(ObservationTrace::default());
        let traced = view.with_observation_trace(&full);
        for x in 0..512 {
            let pos = BlockPos::new(x, 64, 0);
            assert_eq!(
                traced.live_collision_block(Dimension::OVERWORLD, pos),
                view.observation(Dimension::OVERWORLD, pos).map(|v| v.block)
            );
        }
        assert_eq!(full.borrow().cells.len(), 512);
        assert!(!full.borrow().overflow);
        // This new known cell would be stone absent the existing trace refusal.
        assert_eq!(
            traced.live_collision_block(Dimension::OVERWORLD, known),
            None
        );
        assert_eq!(full.borrow().cells.len(), 512);
        assert!(full.borrow().overflow);
        assert_eq!(
            traced.live_collision_block(Dimension::OVERWORLD, outside),
            Some(0)
        );
        assert_eq!(full.borrow().cells.len(), 512);
        assert!(full.borrow().overflow);
    }

    // Contract double: builds accepted native input, without executing actor motion.
    fn grid_builder_double(
        view: &AuthorityReadView<'_>,
        dimension: Dimension,
        origin: [i32; 3],
        dimensions: [u32; 3],
    ) -> Vec<CollisionCell> {
        let count = dimensions
            .into_iter()
            .try_fold(1u32, u32::checked_mul)
            .unwrap();
        assert!((1..=4).contains(&count));
        let mut cells = Vec::with_capacity(count as usize);
        for y in 0..dimensions[1] {
            for x in 0..dimensions[0] {
                for z in 0..dimensions[2] {
                    let pos = BlockPos::new(
                        origin[0].checked_add(x as i32).unwrap(),
                        origin[1].checked_add(y as i32).unwrap(),
                        origin[2].checked_add(z as i32).unwrap(),
                    );
                    cells.push(match view.live_collision_block(dimension, pos) {
                        Some(block) => crate::rules::player_motion::collision_cell(block).unwrap(),
                        None => CollisionCell::default(),
                    });
                }
            }
        }
        CollisionGrid::try_new(origin, dimensions, &cells).unwrap();
        cells
    }

    #[test]
    fn grid_builder_double_boundary_cells_use_native_input_contract() {
        let a = live(&[Dimension::OVERWORLD]);
        let view = a.settled_read().unwrap();
        for (origin, expected) in [
            ([144, 319, -48], [(false, 0), (true, 0)]),
            ([8, 319, 8], [(true, 1), (true, 0)]),
        ] {
            let cells = grid_builder_double(&view, Dimension::OVERWORLD, origin, [1, 2, 1]);
            let grid = CollisionGrid::try_new(origin, [1, 2, 1], &cells).unwrap();
            assert_eq!(grid.origin(), origin);
            assert_eq!(grid.dimensions(), [1, 2, 1]);
            assert_eq!(
                grid.cells()
                    .iter()
                    .map(|v| (v.loaded(), v.used()))
                    .collect::<Vec<_>>(),
                expected
            );
        }
        let disabled = authority();
        let cells = grid_builder_double(
            &disabled.settled_read().unwrap(),
            Dimension::OVERWORLD,
            [144, 320, -48],
            [1, 2, 1],
        );
        assert!(cells.iter().all(|v| !v.loaded()));
    }

    // Contract double: accepted geometry consumes the read adapter, not a motion provider.
    struct GeometryConsumerDouble<'a, 'b>(&'a AuthorityReadView<'b>);
    impl actor_placement::PlacementWorld for GeometryConsumerDouble<'_, '_> {
        fn ready_revision(&self, key: ChunkKey) -> Option<u64> {
            self.0.ready_chunk_revision(key)
        }
        fn block_at(&self, dimension: Dimension, pos: BlockPos) -> Option<u16> {
            self.0.live_collision_block(dimension, pos)
        }
    }

    #[test]
    fn geometry_consumer_double_body_space_observes_managed_boundaries() {
        let a = live(&[Dimension::OVERWORLD]);
        let view = a.settled_read().unwrap();
        let double = GeometryConsumerDouble(&view);
        for (pose, free, ready) in [
            ([144.5, 321., -47.5], true, true),
            ([144.5, 319., -47.5], false, false),
            ([8.5, 319., 8.5], false, true),
        ] {
            assert_eq!(
                actor_placement::body_space(&double, Dimension::OVERWORLD, pose).unwrap(),
                actor_placement::BodySpace { free, ready }
            );
        }
        let disabled = authority();
        let view = disabled.settled_read().unwrap();
        assert_eq!(
            actor_placement::body_space(
                &GeometryConsumerDouble(&view),
                Dimension::OVERWORLD,
                [144.5, 321., -47.5]
            )
            .unwrap(),
            actor_placement::BodySpace {
                free: false,
                ready: false
            }
        );
    }
}
