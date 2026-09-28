//! Opaque single-threaded authority.
//!
//! Sibling modules do not read these fields. They call the ports below.
//! Staging a compound effect checks every component against the pre-effect
//! overlay, counting projectile inserts in that compound, and applies the
//! whole effect only after that check succeeds. A later rejection restores
//! the overlay. Publication encodes control packets and routed events through
//! the existing protocol conversion before it appends any frame.

use std::collections::{BTreeMap, BTreeSet};

use mornlea_domain::{
    CommandEnvelope, CommandEnvelopeParts, Dimension, EventRecipient, PlayerId, RoutedEvent,
    WorldState,
};
use mornlea_protocol::{AdmittedLogin, PlayIntent, ProtocolCodec, ProtocolError, ServerPacket};
use mornlea_storage::{Metadata, PlayerLocation, PlayerSave, StoredPlayer};

use super::contracts::*;

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
    metadata: Metadata,
    metadata_sequence: u64,
    dirty: Vec<OwnedSnapshot>,
    in_flight: Vec<OwnedSnapshot>,
    shutdown: ShutdownReport,
    final_consumed: bool,
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
        })
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
        let tick = self.next_tick;
        self.next_tick = self.next_tick.saturating_add(1);
        Ok(TickPublication {
            tick,
            events: Vec::new(),
            control: Vec::new(),
            counters: TickCounters {
                executed_tick: tick,
                ..TickCounters::default()
            },
        })
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
        for item in pending {
            match item {
                PendingFrame::One { session, frame } => self.append_frame(session, frame),
                PendingFrame::Broadcast { frame } => {
                    let sessions: Vec<SessionKey> = self.sessions.keys().copied().collect();
                    for session in sessions {
                        self.append_frame(session, frame.clone());
                    }
                }
            }
        }
        Ok(())
    }

    fn append_frame(&mut self, session: SessionKey, frame: Vec<u8>) {
        let Some(record) = self.sessions.get_mut(&session) else {
            return;
        };
        if record.outbox_closed {
            return;
        }
        if record.outbox.len() >= self.limits.session_outbox() {
            record.outbox_closed = true;
            return;
        }
        record.outbox.push(frame);
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
        let mut acked = 0;
        let mut released = 0;
        let mut retry = Vec::new();
        for held in self.in_flight.drain(..) {
            let listed = completion
                .committed
                .iter()
                .any(|(key, revision)| key == &held.key && *revision == held.revision);
            if listed {
                acked += 1;
                if matches!(held.key, SaveKey::Metadata) {
                    self.metadata_sequence = held.revision;
                }
            } else {
                released += 1;
                retry.push(held);
            }
        }
        self.dirty.extend(retry.iter().cloned());
        AckReport {
            acked,
            released,
            retry,
            errors: completion.error.into_iter().collect(),
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
    actors: &'a [ActorRecord],
    environment: Option<&'a EnvironmentState>,
}

impl<'a> AuthorityReadView<'a> {
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
        self.blocks
            .iter()
            .find(|((key, block_pos), _)| key.dimension == dimension && *block_pos == pos)
            .map(|(_, observed)| observed.block)
    }
    pub fn actors(&self) -> &'a [ActorRecord] {
        self.actors
    }
    pub fn actor(&self, key: ActorKey) -> Option<&'a ActorRecord> {
        self.actors.iter().find(|actor| actor.key == key)
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
    world: Option<WorldState>,
    commands: Vec<CommandEnvelope>,
    companions: Vec<CompanionActionEnvelope>,
    interactions: Vec<AuthorityInteraction>,
    actors: Vec<ActorRecord>,
    environment: Option<EnvironmentState>,
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
    deferred: Vec<(RulePhase, CommandEnvelope)>,
}

impl<'a> TickContext<'a> {
    pub fn harness(authority: &'a mut AuthorityState, budget: TickBudget) -> Self {
        Self::from_parts(authority, budget)
    }

    pub fn from_fixture(
        authority: &'a mut AuthorityState,
        initial: &FixtureState,
        budget: TickBudget,
    ) -> Self {
        let mut context = Self::from_parts(authority, budget);
        context.world = Some(initial.world);
        context.actors = initial.actors.clone();
        context
            .inventories
            .extend(initial.inventories.iter().cloned());
        context
    }

    fn from_parts(authority: &'a mut AuthorityState, budget: TickBudget) -> Self {
        Self {
            authority,
            inventories: BTreeMap::new(),
            blocks: BTreeMap::new(),
            world: None,
            commands: Vec::new(),
            companions: Vec::new(),
            interactions: Vec::new(),
            actors: Vec::new(),
            environment: None,
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
            deferred: Vec::new(),
        }
    }

    pub fn preload_inventory(&mut self, actor: ActorKey, record: InventoryRecord) {
        self.inventories.insert(actor, record);
    }

    pub fn preload_block(&mut self, observed: BlockObservation) {
        self.blocks.insert((observed.key, observed.pos), observed);
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
            actors: &self.actors,
            environment: self.environment.as_ref(),
        }
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

    pub fn stage(&mut self, effect: RuleEffect) -> Result<(), RuleReject> {
        let mut pending_projectiles = 0usize;
        self.validate_effect(&effect, false, &mut pending_projectiles)?;
        self.apply_effect(effect)
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
        FixtureState {
            runtime: Vec::new(),
            actors: self.actors.clone(),
            chunks: Vec::new(),
            inventories: self
                .inventories
                .iter()
                .map(|(key, inventory)| (*key, *inventory))
                .collect(),
            containers: Vec::new(),
            work: WorkState::default(),
            sleep: SleepState {
                beds: Vec::new(),
                day_phase_offset: 0,
                pending_offset: None,
            },
            projectiles: self.projectiles.clone(),
            drops: Vec::new(),
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

    fn validate_effect(
        &self,
        effect: &RuleEffect,
        nested: bool,
        pending_projectiles: &mut usize,
    ) -> Result<(), RuleReject> {
        match effect {
            RuleEffect::Compound(parts) => {
                if nested || parts.len() > EFFECT_BUDGET {
                    return Err(RuleReject::ResourceFull(Resource::RuleEffects));
                }
                for part in parts {
                    self.validate_effect(part, true, pending_projectiles)?;
                }
                Ok(())
            }
            RuleEffect::Inventory(patch) => match self.inventories.get(&patch.actor) {
                Some(current) if current == &patch.before => Ok(()),
                _ => Err(RuleReject::StaleObservation),
            },
            RuleEffect::Blocks(txn) => {
                if txn.writes.len() > EFFECT_BUDGET {
                    return Err(RuleReject::ResourceFull(Resource::RuleEffects));
                }
                self.validate_writes(&txn.writes)
            }
            RuleEffect::Projectile { after: Some(_), .. } => {
                let occupied = self.projectiles.len().saturating_add(*pending_projectiles);
                if occupied >= MAX_PROJECTILE_RECORDS {
                    return Err(RuleReject::ResourceFull(Resource::RuleEffects));
                }
                *pending_projectiles = pending_projectiles.saturating_add(1);
                Ok(())
            }
            _ => Ok(()),
        }
    }

    fn validate_writes(&self, writes: &[BlockWrite]) -> Result<(), RuleReject> {
        if writes.len() > EFFECT_BUDGET {
            return Err(RuleReject::ResourceFull(Resource::RuleEffects));
        }
        for write in writes {
            match self.blocks.get(&(write.observed.key, write.observed.pos)) {
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
                let projectiles = self.projectiles.clone();
                let environment = self.environment.clone();
                for part in parts {
                    if let Err(error) = self.apply_effect(part) {
                        self.inventories = inventories;
                        self.world = world;
                        self.blocks = blocks;
                        self.projectiles = projectiles;
                        self.environment = environment;
                        return Err(error);
                    }
                }
                Ok(())
            }
            RuleEffect::Inventory(patch) => {
                self.inventories.insert(patch.actor, patch.after);
                Ok(())
            }
            RuleEffect::World(world) => {
                self.world = Some(world);
                Ok(())
            }
            RuleEffect::Blocks(txn) => {
                self.apply_writes(&txn.writes);
                if let Some(patch) = txn.inventory {
                    self.inventories.insert(patch.actor, patch.after);
                }
                Ok(())
            }
            RuleEffect::Projectile { after, .. } => {
                if let Some(projectile) = after {
                    if self.projectiles.len() >= MAX_PROJECTILE_RECORDS {
                        return Err(RuleReject::ResourceFull(Resource::RuleEffects));
                    }
                    self.projectiles.push(projectile);
                }
                Ok(())
            }
            RuleEffect::Environment(environment) => {
                self.environment = Some(environment);
                Ok(())
            }
            _ => Ok(()),
        }
    }

    fn apply_writes(&mut self, writes: &[BlockWrite]) {
        for write in writes {
            let mut observed = write.observed;
            observed.block = write.replacement;
            observed.revision = observed.revision.saturating_add(1);
            self.blocks.insert((observed.key, observed.pos), observed);
        }
    }
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

    fn commit(&mut self, txn: BlockTxn) -> Result<MutationOutcome, RuleReject> {
        self.context.validate_writes(&txn.writes)?;
        if let Some(patch) = &txn.inventory {
            match self.context.inventories.get(&patch.actor) {
                Some(current) if current == &patch.before => {}
                _ => return Err(RuleReject::StaleObservation),
            }
        }
        let changed = txn
            .writes
            .iter()
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
            .map(|drops| drops.stacks.len())
            .unwrap_or(0);
        let inventory_changed = txn.inventory.is_some();
        self.context.apply_writes(&txn.writes);
        if let Some(patch) = txn.inventory {
            self.context.inventories.insert(patch.actor, patch.after);
        }
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
