//! Opaque single-threaded authority.
//!
//! Sibling modules do not read these fields. They call the ports below.
//! Staging a compound effect rehearses ordered inventory, container, drop and
//! projectile edits on private scratch state, and applies the whole effect
//! only after every component validates. A later rejection restores
//! the overlay. Publication encodes control packets and routed events through
//! the existing protocol conversion before it appends any frame.

use std::collections::{BTreeMap, BTreeSet};

use mornlea_domain::{
    CommandEnvelope, CommandEnvelopeParts, ContainerRef, Dimension, EventRecipient, MotionState,
    PlayerId, RejectReason, RoutedEvent, Weather, WorldState,
};
use mornlea_protocol::{AdmittedLogin, PlayIntent, ProtocolCodec, ProtocolError, ServerPacket};
use mornlea_storage::{Chunk, Metadata, PlayerLocation, PlayerSave, StoredPlayer};

use super::container_store::ContainerState;
use super::contracts::*;
use super::drop_store::{self, DropState};
use super::login_seed::{SeededPlayer, seed_player};
use super::world::ReadyChunk;
use crate::rules::farmland::FarmlandSchedule;
use crate::rules::fluids::FluidSchedule;

const COMPANION_INBOX: usize = 4;

struct SessionRecord {
    player_id: PlayerId,
    display_name: String,
    phase: SessionPhase,
    last_applied_sequence: u64,
    next_arrival: u64,
    body: Option<PlayerSave>,
    outbox: Vec<Vec<u8>>,
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
/// The serial reducer seeds the tick overlay from these maps at tick start
/// and commits the worked overlay back with a full replace at tick end, the
/// same clone-out/replace shape as the viewer commit with no merge logic.
/// Costs stay inside the existing caps because every map populates through
/// the same bounded staging the overlay already enforces.
#[derive(Clone, Default)]
pub struct ResidentTickState {
    pub actors: Vec<ActorRecord>,
    pub runtimes: BTreeMap<ActorKey, ActorRuntime>,
    pub inventories: BTreeMap<ActorKey, InventoryRecord>,
    pub mining: BTreeMap<ActorKey, MiningProgress>,
    pub projectiles: Vec<ProjectileRecord>,
    pub environment: Option<EnvironmentState>,
    pub sleep_record: Option<SleepState>,
    pub sleeping: BTreeSet<SessionKey>,
    pub blocks: BTreeMap<(ChunkKey, mornlea_domain::BlockPos), BlockObservation>,
    ready: BTreeMap<ChunkKey, ReadyChunk>,
    drops: BTreeMap<ChunkKey, DropState>,
    containers: BTreeMap<ContainerRef, ContainerRecord>,
    container_chunks: BTreeMap<ChunkKey, ContainerState>,
}

impl ResidentTickState {
    /// Ready chunks as keyed base snapshots in chunk-key order. Overlay
    /// writes and staged drops stay out: this names the carried chunk
    /// content, not the tick-local overlay around it.
    pub fn ready_snapshot(&self) -> Vec<(ChunkKey, u64, u64, Chunk)> {
        self.ready
            .values()
            .map(|chunk| {
                let (key, generation, revision, base) =
                    chunk.snapshot(std::iter::empty(), None, None);
                (key, generation, revision, base)
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
    occupied: usize,
    commands: Vec<CommandEnvelope>,
    companions: Vec<QueuedCompanion>,
    companion_arrival: BTreeMap<mornlea_domain::CompanionId, u64>,
    interactions: Vec<AuthorityInteraction>,
    chunk_results: Vec<ChunkResult>,
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
            depths_seed_salt: 0,
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
            occupied: 0,
            commands,
            companions: Vec::new(),
            companion_arrival: BTreeMap::new(),
            interactions: Vec::new(),
            chunk_results: Vec::new(),
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

    /// Replaces committed leases with the tick's complete net viewer set.
    /// The reducer owns this commit and retired-session pruning.
    pub fn commit_viewers(&mut self, overlay: BTreeMap<SessionKey, ViewLease>) {
        self.views = overlay;
    }

    /// Replaces every resident map with the tick's complete net overlay. The
    /// serial reducer owns the only call; providers only stage the overlay.
    pub fn commit_residents(&mut self, next: ResidentTickState) {
        self.residents = next;
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
        self.occupied = self.occupied.saturating_sub(1);
        // Sleep participation belongs to the live session. Durable respawn
        // anchors and the other resident lanes keep their persistence owner.
        if let Some(sleep) = &mut self.residents.sleep_record {
            sleep.beds.retain(|(session, _, _)| *session != key);
        }
        self.residents.sleeping.remove(&key);
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

    // Refusal returns the original owned chunk result so the producer keeps
    // ownership; the mailbox never stores or copies a rejected record.
    #[allow(clippy::result_large_err)]
    pub fn admit_chunk(&mut self, result: ChunkResult) -> Result<(), ChunkResult> {
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
            let frame = encode_packet(&mut codec, &packet)?;
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
            let frame = encode_packet(&mut codec, &reply.packet)?;
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
                    let sessions: Vec<SessionKey> = self.sessions.keys().copied().collect();
                    for session in sessions {
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
    fn append_frame(&mut self, session: SessionKey, frame: Vec<u8>) -> bool {
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
        record.outbox.push(frame);
        false
    }

    pub fn take_outbox(
        &mut self,
        session: SessionKey,
        max_frames: usize,
        max_bytes: usize,
    ) -> Result<Vec<Vec<u8>>, ServerError> {
        let record = self
            .sessions
            .get_mut(&session)
            .ok_or(ServerError::StaleSession { session })?;
        let mut taken = Vec::new();
        let mut bytes = 0usize;
        while taken.len() < max_frames {
            let Some(frame) = record.outbox.first() else {
                break;
            };
            let next = bytes.saturating_add(frame.len());
            if !taken.is_empty() && next > max_bytes {
                break;
            }
            bytes = next;
            taken.push(record.outbox.remove(0));
        }
        Ok(taken)
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
        let mut save_keys = Vec::new();
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

    pub fn select(&mut self, _mode: SaveMode, budget: SaveBudget) -> Vec<OwnedSnapshot> {
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

    pub fn return_dirty(&mut self, snapshot: OwnedSnapshot) {
        self.in_flight
            .retain(|held| !(held.key == snapshot.key && held.revision == snapshot.revision));
        self.dirty.push(snapshot);
    }

    pub fn apply_completion(&mut self, completion: SaveCompletion) -> AckReport {
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
                self.in_flight
                    .iter()
                    .filter(|held| held.key == snapshot.key && held.revision == snapshot.revision)
                    .all(|held| held == snapshot)
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
            let position = self
                .in_flight
                .iter()
                .position(|held| held.key == snapshot.key && held.revision == snapshot.revision);
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

    pub fn save_stats(&self) -> SaveStats {
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

    pub fn remember_dirty(&mut self, snapshot: OwnedSnapshot) {
        self.dirty.push(snapshot);
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
                Ok(())
            }
            ShutdownPhase::WaitWorkers => io.workers.wait(deadline),
            ShutdownPhase::FinalizeMemory => {
                io.memory.begin_attempt(deadline)?;
                let memory = io.memory.drain(deadline)?;
                report.outstanding = memory.outstanding;
                if memory.outstanding != 0 {
                    return Err(ServerError::Internal {
                        invariant: "memory finalization",
                    });
                }
                Ok(())
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
        if self.sessions.values().any(|record| {
            record.phase != SessionPhase::Retired && record.player_id == login.player_id()
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
                next_arrival: 0,
                body: None,
                outbox: Vec::new(),
                outbox_closed: false,
            },
        );
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
    One { session: SessionKey, frame: Vec<u8> },
    Broadcast { frame: Vec<u8> },
}

/// Encodes one packet with the protocol codec. A short buffer is resized to
/// the length the codec reports. Any other codec refusal is the existing
/// packet input error; this function does not choose a wire layout.
fn encode_packet(codec: &mut ProtocolCodec, packet: &ServerPacket) -> Result<Vec<u8>, ServerError> {
    let mut buffer = vec![0u8; 64];
    loop {
        match codec.encode_server_into(packet, &mut buffer) {
            Ok(written) => {
                buffer.truncate(written);
                return Ok(buffer);
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
}

/// Borrowed read surface. `None` means the value is unavailable, not air.
pub struct AuthorityReadView<'a> {
    tick: u64,
    world: Option<WorldState>,
    commands: &'a [CommandEnvelope],
    companions: &'a [CompanionActionEnvelope],
    interactions: &'a [AuthorityInteraction],
    inventories: &'a BTreeMap<ActorKey, InventoryRecord>,
    blocks: &'a BTreeMap<(ChunkKey, mornlea_domain::BlockPos), BlockObservation>,
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
}

/// Private cumulative output preview: failed attempts never consume capacity
/// or mutate the immutable authority base. One player death admits at most
/// forty slot batches; final compound staging remains the publication owner.
pub(crate) struct DropRehearsal<'a> {
    drops: &'a BTreeMap<ChunkKey, DropState>,
    ready: &'a BTreeMap<ChunkKey, ReadyChunk>,
    pending: BTreeMap<ChunkKey, DropState>,
}

impl DropRehearsal<'_> {
    pub(crate) fn try_insert(&mut self, batch: &DropBatch) -> Result<(), RuleReject> {
        drop_store::validate_batch(batch)?;
        let (key, _) = drop_store::batch_location(batch)?;
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
    /// Sparse fixture cells alone do not establish a Ready chunk.
    pub fn ready_chunk(&self, key: ChunkKey) -> bool {
        self.ready.contains_key(&key)
    }
    /// Ready-chunk keys in deterministic key order for ring-ordered scans
    /// such as death drops. The set is bounded by chunk-result caps, so
    /// collecting it never scans the world.
    pub fn ready_chunk_keys(&self) -> Vec<ChunkKey> {
        self.ready.keys().copied().collect()
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
        self.ready
            .get(&block_key(
                dimension,
                mornlea_domain::BlockPos::new(x, 0, z),
            ))
            .map(|chunk| chunk.height(x, z))
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
        if !(-64..320).contains(&pos.y()) {
            return None;
        }
        let key = block_key(dimension, pos);
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
        if let Some(chunk) = self.container_chunks.get(&key) {
            return chunk.at(key, pos, kind);
        }
        // Sparse fixtures retain their explicit historical first-slot convention.
        let reference = ContainerRef::try_new(key.pos, kind, 0, 1).ok()?;
        self.world_container(dimension, reference)
    }

    /// Fixed array enumeration is bounded independently of total loaded chunks.
    pub fn container_refs(&self, key: ChunkKey) -> Vec<ContainerRef> {
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
        self.drops.get(&key).map(DropState::records).unwrap_or(&[])
    }
    /// Cumulative preview borrows the base and retains only successful chunk copies.
    pub(crate) fn drop_rehearsal(&self) -> DropRehearsal<'a> {
        DropRehearsal {
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
    inventories: BTreeMap<ActorKey, InventoryRecord>,
    blocks: BTreeMap<(ChunkKey, mornlea_domain::BlockPos), BlockObservation>,
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
    /// Installs already validated compact data; preparation belongs off the tick.
    pub fn preload_ready_chunk(&mut self, chunk: ReadyChunk) {
        self.blocks.retain(|(key, _), _| *key != chunk.key);
        self.changed.retain(|(key, _), _| *key != chunk.key);
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
        // Tick-start seeding clones every resident map into the overlay, so
        // providers read last tick's committed state. The pre-step snapshot
        // covers the seeded actors by the same construction rule as fixtures.
        context.actors = context.authority.residents.actors.clone();
        context.runtimes = context.authority.residents.runtimes.clone();
        context.inventories = context.authority.residents.inventories.clone();
        context.mining = context.authority.residents.mining.clone();
        context.projectiles = context.authority.residents.projectiles.clone();
        context.environment = context.authority.residents.environment.clone();
        if let Some(record) = &context.authority.residents.sleep_record {
            context.sleep_record = record.clone();
        }
        context.sleeping = context.authority.residents.sleeping.clone();
        context.blocks = context.authority.residents.blocks.clone();
        context.ready = context.authority.residents.ready.clone();
        context.drops = context.authority.residents.drops.clone();
        context.containers = context.authority.residents.containers.clone();
        context.container_chunks = context.authority.residents.container_chunks.clone();
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
            inventories: BTreeMap::new(),
            blocks: BTreeMap::new(),
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
        }
    }

    /// Returns the complete net set for the reducer's full replacement commit.
    pub fn viewer_leases(&self) -> BTreeMap<SessionKey, ViewLease> {
        self.viewers.clone()
    }

    /// Clones the complete net overlay for the reducer's full-replacement
    /// resident commit. Login staging lands before this read, so seeded
    /// actors commit exactly like carried ones.
    pub fn resident_snapshot(&self) -> ResidentTickState {
        ResidentTickState {
            actors: self.actors.clone(),
            runtimes: self.runtimes.clone(),
            inventories: self.inventories.clone(),
            mining: self.mining.clone(),
            projectiles: self.projectiles.clone(),
            environment: self.environment.clone(),
            sleep_record: Some(self.sleep_record.clone()),
            sleeping: self.sleeping.clone(),
            blocks: self.blocks.clone(),
            ready: self.ready.clone(),
            drops: self.drops.clone(),
            containers: self.containers.clone(),
            container_chunks: self.container_chunks.clone(),
        }
    }

    /// Stages one login seed into the overlay. Latest-wins on a repeated key
    /// preserves the no-duplicate-actor invariant even against a carried
    /// record; the scan itself only emits sessions without one.
    pub fn stage_login(&mut self, seeded: SeededPlayer) {
        let SeededPlayer { actor, inventory } = seeded;
        match self
            .actors
            .iter()
            .position(|staged| staged.key == actor.key)
        {
            Some(index) => self.actors[index] = actor.clone(),
            None => self.actors.push(actor.clone()),
        }
        self.pre_step.insert(actor.key, actor.motion);
        self.inventories.insert(actor.key, inventory);
    }

    /// Reducer-carried sleep record under settlement.
    pub fn sleep_record(&self) -> &SleepState {
        &self.sleep_record
    }

    /// Persists the threaded sleep record: the entered copy after each bed
    /// entry, the settled copy after the settlement batch.
    pub fn set_sleep_record(&mut self, record: SleepState) {
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
                if txn.writes.len() > EFFECT_BUDGET {
                    return Err(RuleReject::ResourceFull(Resource::RuleEffects));
                }
                self.validate_writes(&txn.writes)?;
                if let Some(patch) = &txn.inventory {
                    self.validate_inventory_patch(patch, pending_inventories)?;
                }
                for capture in &txn.containers {
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
                    let next = pending_drop_state(pending_drops, self.drops.get(&key), key)?;
                    next.insert(key, batch)?;
                    self.check_drop_revision(key, next)?;
                }
                Ok(())
            }
            RuleEffect::Drops(batch) => {
                drop_store::validate_batch(batch)?;
                let (key, _) = drop_store::batch_location(batch)?;
                let next = pending_drop_state(pending_drops, self.drops.get(&key), key)?;
                next.insert(key, batch)?;
                self.check_drop_revision(key, next)
            }
            RuleEffect::DropPatch { before, after } => {
                let key = drop_store::drop_key(before)?;
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
        match effect {
            RuleEffect::Compound(parts) => {
                let inventories = self.inventories.clone();
                let world = self.world;
                let blocks = self.blocks.clone();
                let changed = self.changed.clone();
                let ready = self.ready.clone();
                let containers = self.containers.clone();
                let viewers = self.viewers.clone();
                let projectiles = self.projectiles.clone();
                let damage_len = self.damage_intents.len();
                let environment = self.environment.clone();
                let sleep_record = self.sleep_record.clone();
                // Defensive enforcement of the seam rule that a rejected
                // atomic effect leaves all components unchanged: actor
                // records became effect-mutable with the `Actor` staging arm,
                // so they join the rollback snapshot, and container records
                // and viewer leases join it with their own staging arms. No
                // public path reaches this restore today because `stage`
                // validates every component before applying any. Drop copies
                // publish only after this entire operation succeeds.
                let actors = self.actors.clone();
                let runtimes = self.runtimes.clone();
                let mining = self.mining.clone();
                for part in parts {
                    if let Err(error) = self.apply_effect(part) {
                        self.inventories = inventories;
                        self.world = world;
                        self.blocks = blocks;
                        self.changed = changed;
                        self.ready = ready;
                        self.containers = containers;
                        self.viewers = viewers;
                        self.projectiles = projectiles;
                        self.damage_intents.truncate(damage_len);
                        self.environment = environment;
                        self.sleep_record = sleep_record;
                        self.actors = actors;
                        self.runtimes = runtimes;
                        self.mining = mining;
                        return Err(error);
                    }
                }
                Ok(())
            }
            RuleEffect::Actor(record) => {
                // Latest-wins overlay replace, mirroring the inventory arm: a
                // staged actor snapshot supersedes the earlier record with
                // the same key instead of accumulating duplicates.
                match self.actors.iter().position(|actor| actor.key == record.key) {
                    Some(index) => self.actors[index] = record,
                    None => self.actors.push(record),
                }
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
            self.ready
                .get_mut(&observed.key)
                .expect("Ready ownership cannot change during a write")
                .set_height(pos.x(), pos.z(), next);
        }
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
