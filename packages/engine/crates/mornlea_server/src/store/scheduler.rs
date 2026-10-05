//! Autosave scheduling layer over the durable store mailbox.
//!
//! [`AutosaveScheduler`] owns selection cadence, retry with backoff,
//! backpressure hysteresis, unload retention and the deadline-bounded final
//! flush. It composes with the accepted [`StoreMailbox`](super::mailbox::StoreMailbox):
//! the scheduler selects snapshots through the frozen [`SaveAuthority`] seams
//! and submits them, while the mailbox owns admission, tickets and the backend
//! commit. The scheduler never encodes records and never touches disk.
//!
//! Tick discipline mirrors the Go persistence scheduler in
//! `packages/server/server/persistence/world.go`. Each nonblocking
//! [`StoreHandle::poll_tick`] drains ready completions first, then dispatches
//! due retries, then urgent snapshots, then cadence-triggered autosave, and
//! finally the metadata schedule; autosave stays active until dirty and
//! in-flight both clear. Background saves execute on their owner as soon as
//! dispatched. Flush waits for actual results; sync and close share that same
//! serialized owner, and all three waits remain outside the tick.
//!
//! Ownership across the layers: the authority retains its dirty and in-flight
//! accounting while submitted copies travel through the mailbox. A failed
//! completion hands its originals back through [`AckReport::retry`], and the
//! scheduler holds them as backoff cohorts; the authority entry stays
//! in-flight meanwhile, so selection cannot submit a duplicate while the retry
//! waits. A refused fresh submission returns through
//! [`SaveAuthority::return_dirty`], matching the Go full-queue release. A
//! refused retry dispatch keeps its cohort untouched with no attempt advance.

use mornlea_domain::PlayerId;
use std::collections::HashMap;

use crate::core::contracts::{
    AckReport, ChunkKey, ChunkLoadPoll, ChunkLoadPort, ChunkRequestId, Clock, Deadline,
    DiskBackend, FlushReport, LoadPoll, LoginTicket, OwnedSnapshot, SaveAuthority, SaveBudget,
    SaveCompletion, SaveMode, SavePoll, SaveRequest, SaveScheduleReport, SaveTicket, ServerError,
    StoreHandle, SubmitSaveError,
};
use crate::store::mailbox::StoreMailbox;

/// Checked scheduler tuning. Defaults follow the Go server configuration:
/// autosave every six thousand ticks, retry base twenty ticks capped at twelve
/// hundred, and backpressure entering at five hundred twelve mebibytes of
/// estimated unsaved bytes.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SchedulerConfig {
    autosave_interval: u64,
    retry_base_ticks: u64,
    retry_max_ticks: u64,
    unsaved_byte_ceiling: usize,
}

impl SchedulerConfig {
    /// Checks the tuning before the scheduler allocates any retry state. A
    /// zero cadence would fault the tick remainder, and an inverted retry range
    /// would never back off, so both reject up front.
    pub fn try_new(
        autosave_interval: u64,
        retry_base_ticks: u64,
        retry_max_ticks: u64,
        unsaved_byte_ceiling: usize,
    ) -> Result<Self, ServerError> {
        if autosave_interval == 0 {
            return Err(ServerError::InvalidInput {
                field: "autosave_interval",
            });
        }
        if retry_base_ticks == 0 || retry_max_ticks == 0 {
            return Err(ServerError::InvalidInput {
                field: "retry_base_ticks",
            });
        }
        if retry_max_ticks < retry_base_ticks {
            return Err(ServerError::InvalidInput {
                field: "retry_max_ticks",
            });
        }
        if unsaved_byte_ceiling < 1 {
            return Err(ServerError::InvalidInput {
                field: "unsaved_bytes",
            });
        }
        Ok(Self {
            autosave_interval,
            retry_base_ticks,
            retry_max_ticks,
            unsaved_byte_ceiling,
        })
    }
}

impl Default for SchedulerConfig {
    fn default() -> Self {
        Self {
            autosave_interval: 6000,
            retry_base_ticks: 20,
            retry_max_ticks: 1200,
            unsaved_byte_ceiling: 512 << 20,
        }
    }
}

/// Retry backoff in ticks: the base doubles per attempt with a cap-before-double
/// ceiling, mirroring the Go retry delay. The cap check precedes the doubling
/// so a large base can never overflow past the maximum.
pub fn retry_delay(base: u64, maximum: u64, attempt: u32) -> u64 {
    let mut delay = base;
    let mut step: u32 = 1;
    while step < attempt && delay < maximum {
        if delay > maximum / 2 {
            return maximum;
        }
        delay *= 2;
        step += 1;
    }
    delay.min(maximum)
}

/// Backpressure hysteresis over estimated unsaved bytes, mirroring the Go
/// boundary: enter at or above the ceiling, then stay until strictly below
/// ninety percent. The exit threshold rounds up, so a ceiling of one hundred
/// stays at ninety and exits at eighty-nine.
pub fn next_backpressure(current: bool, estimated_bytes: usize, ceiling: usize) -> bool {
    if !current {
        return estimated_bytes >= ceiling;
    }
    let threshold = ceiling - ceiling / 10;
    estimated_bytes >= threshold
}

/// One failed submission waiting for its backoff tick. Cohorts merge only with
/// an equal attempt count and an equal deadline, so unrelated failures never
/// share a dispatch.
#[derive(Clone, Debug, PartialEq)]
struct PendingRetry {
    id: u64,
    attempts: u32,
    next_tick: u64,
    snapshots: Vec<OwnedSnapshot>,
}

/// Fixed metadata schedule: the committed sequence, the pending flag, at most
/// one in-flight submission, and the failure count with its next retry tick.
/// The target always comes from [`SaveAuthority::try_metadata_snapshot`], never
/// from a wall clock.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct MetadataSchedule {
    committed: u64,
    pending: bool,
    in_flight: bool,
    attempts: u32,
    next_retry: u64,
}

/// How one tracked submission was dispatched, so its completion routes to the
/// retry cohorts or the metadata schedule.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum TrackedKind {
    Fresh,
    Retry { id: u64, attempt: u32 },
    Metadata { sequence: u64 },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct TrackedSubmit {
    ticket: SaveTicket,
    kind: TrackedKind,
}

/// Scheduling layer over the durable store mailbox. See the module docs for
/// the tick order and the ownership split with the authority.
pub struct AutosaveScheduler<B: DiskBackend> {
    store: StoreMailbox<B>,
    config: SchedulerConfig,
    autosave_active: bool,
    backpressured: bool,
    next_retry_id: u64,
    pending: Vec<PendingRetry>,
    dispatched: HashMap<u64, PendingRetry>,
    tracked: Vec<TrackedSubmit>,
    metadata: MetadataSchedule,
    last_error: Option<ServerError>,
}

impl<B: DiskBackend> AutosaveScheduler<B> {
    /// Builds the scheduler over an accepted mailbox. The configuration check
    /// runs before any retry state allocates.
    pub fn try_new(config: SchedulerConfig, store: StoreMailbox<B>) -> Result<Self, ServerError> {
        let _ = SchedulerConfig::try_new(
            config.autosave_interval,
            config.retry_base_ticks,
            config.retry_max_ticks,
            config.unsaved_byte_ceiling,
        )?;
        Ok(Self {
            store,
            config,
            autosave_active: false,
            backpressured: false,
            next_retry_id: 0,
            pending: Vec::new(),
            dispatched: HashMap::new(),
            tracked: Vec::new(),
            metadata: MetadataSchedule::default(),
            last_error: None,
        })
    }

    /// Drives inline doubles synchronously; a background mailbox only hands off
    /// immutable work and collects available completion facts.
    pub fn drive_workers(&mut self) -> usize {
        self.store.drive_workers()
    }

    /// Whether the cadence-triggered autosave still has work to drain.
    pub fn autosave_active(&self) -> bool {
        self.autosave_active
    }

    /// Whether estimated unsaved bytes hold the backpressure latch.
    pub fn backpressured(&self) -> bool {
        self.backpressured
    }

    /// Chunk-load admission slots for the automatic source acquisition caller.
    pub(crate) fn source_chunk_slots(&self) -> Result<usize, ServerError> {
        self.store.source_chunk_slots()
    }

    /// Retry cohorts waiting for their backoff tick.
    pub fn pending_retry_jobs(&self) -> usize {
        self.pending.len()
    }

    /// Attempt count and deadline of each waiting cohort, in dispatch order.
    pub fn pending_retry_state(&self) -> Vec<(u32, u64)> {
        let mut order: Vec<&PendingRetry> = self.pending.iter().collect();
        order.sort_by_key(|cohort| (cohort.next_tick, cohort.id));
        order
            .into_iter()
            .map(|cohort| (cohort.attempts, cohort.next_tick))
            .collect()
    }

    /// Submissions the scheduler dispatched and has not drained yet.
    pub fn tracked_submits(&self) -> usize {
        self.tracked.len()
    }

    /// Most recent scheduling or completion failure, if any.
    pub fn last_error(&self) -> Option<ServerError> {
        self.last_error
    }

    /// Whether a metadata target waits for dispatch or retry.
    pub fn metadata_pending(&self) -> bool {
        self.metadata.pending
    }

    /// Routes one ready completion through the authority. Chunk kinds convert
    /// uncommitted originals into backoff cohorts; the metadata kind drives
    /// the fixed schedule. Returns the completion error, if any.
    fn apply_completion(
        &mut self,
        tracked: TrackedSubmit,
        completion: SaveCompletion,
        authority: &mut dyn SaveAuthority,
        tick: u64,
    ) -> Option<ServerError> {
        match tracked.kind {
            TrackedKind::Metadata { sequence } => {
                let ack = authority.apply_completion(completion);
                let failed = !ack.errors.is_empty();
                let error = ack.errors.first().copied();
                self.apply_metadata_completion(sequence, ack, tick);
                if failed { error } else { None }
            }
            TrackedKind::Fresh => {
                let ack = authority.apply_completion(completion);
                self.absorb_chunk_ack(ack, 1, tick)
            }
            TrackedKind::Retry { id, attempt } => {
                // The dispatched cohort resolves here, success or failure; a
                // failure enqueues a fresh cohort below instead of reviving it.
                self.dispatched.remove(&id);
                let ack = authority.apply_completion(completion);
                self.absorb_chunk_ack(ack, attempt, tick)
            }
        }
    }

    /// Absorbs a chunk acknowledgment: clean acks vanish, failures record and
    /// cohort their uncommitted originals under the failed attempt's backoff.
    fn absorb_chunk_ack(&mut self, ack: AckReport, attempt: u32, tick: u64) -> Option<ServerError> {
        if ack.errors.is_empty() && ack.retry.is_empty() {
            return None;
        }
        if let Some(error) = ack.errors.first() {
            self.last_error = Some(*error);
        }
        let error = ack.errors.first().copied();
        self.enqueue_retry(ack.retry, attempt, tick);
        error
    }

    /// Applies the metadata acknowledgment: success commits the sequence and
    /// clears the schedule, failure preserves the frozen target with a bounded
    /// backoff for the next attempt.
    fn apply_metadata_completion(
        &mut self,
        sequence: u64,
        ack: AckReport,
        tick: u64,
    ) -> Option<ServerError> {
        self.metadata.in_flight = false;
        if ack.errors.is_empty() {
            self.metadata.committed = self.metadata.committed.max(sequence);
            self.metadata.attempts = 0;
            self.metadata.next_retry = 0;
            self.metadata.pending = false;
            return None;
        }
        self.metadata.attempts = self.metadata.attempts.saturating_add(1);
        self.metadata.pending = true;
        self.metadata.next_retry = tick.saturating_add(retry_delay(
            self.config.retry_base_ticks,
            self.config.retry_max_ticks,
            self.metadata.attempts,
        ));
        let error = ack.errors.first().copied();
        self.last_error = error;
        error
    }

    /// Holds failed originals for retry with the failed attempt's backoff. An
    /// empty handoff keeps no cohort; snapshots the scheduler already owns are
    /// never duplicated, and one clone survives per key with the higher
    /// revision winning.
    fn enqueue_retry(&mut self, snapshots: Vec<OwnedSnapshot>, failed_attempt: u32, now: u64) {
        if snapshots.is_empty() {
            return;
        }
        let attempts = failed_attempt.max(1);
        let next_tick = now.saturating_add(retry_delay(
            self.config.retry_base_ticks,
            self.config.retry_max_ticks,
            attempts,
        ));
        let mut fresh: Vec<OwnedSnapshot> = Vec::with_capacity(snapshots.len());
        for snapshot in snapshots {
            if self.owns_retry_snapshot(&snapshot) {
                continue;
            }
            if let Some(held) = fresh.iter_mut().find(|held| held.key == snapshot.key) {
                if snapshot.revision > held.revision {
                    *held = snapshot;
                }
                continue;
            }
            fresh.push(snapshot);
        }
        if fresh.is_empty() {
            return;
        }
        if let Some(cohort) = self
            .pending
            .iter_mut()
            .find(|cohort| cohort.attempts == attempts && cohort.next_tick == next_tick)
        {
            for snapshot in fresh {
                if let Some(held) = cohort
                    .snapshots
                    .iter_mut()
                    .find(|held| held.key == snapshot.key)
                {
                    if snapshot.revision > held.revision {
                        *held = snapshot;
                    }
                } else {
                    cohort.snapshots.push(snapshot);
                }
            }
            return;
        }
        let id = self.allocate_retry_id();
        self.pending.push(PendingRetry {
            id,
            attempts,
            next_tick,
            snapshots: fresh,
        });
    }

    fn owns_retry_snapshot(&self, snapshot: &OwnedSnapshot) -> bool {
        self.pending
            .iter()
            .flat_map(|cohort| &cohort.snapshots)
            .chain(
                self.dispatched
                    .values()
                    .flat_map(|cohort| &cohort.snapshots),
            )
            .any(|held| held.key == snapshot.key && held.revision == snapshot.revision)
    }

    fn allocate_retry_id(&mut self) -> u64 {
        loop {
            self.next_retry_id = self.next_retry_id.wrapping_add(1);
            if self.next_retry_id == 0 {
                self.next_retry_id += 1;
            }
            let id = self.next_retry_id;
            if self.dispatched.contains_key(&id)
                || self.pending.iter().any(|cohort| cohort.id == id)
            {
                continue;
            }
            return id;
        }
    }

    /// Submits one fresh selection. A refused request returns whole through
    /// the authority, preserving the dirty retention the selection marked.
    fn submit_fresh(
        &mut self,
        snapshots: Vec<OwnedSnapshot>,
        authority: &mut dyn SaveAuthority,
    ) -> usize {
        if snapshots.is_empty() {
            return 0;
        }
        let count = snapshots.len();
        match self.store.submit(SaveRequest { snapshots }) {
            Ok(ticket) => {
                self.tracked.push(TrackedSubmit {
                    ticket,
                    kind: TrackedKind::Fresh,
                });
                count
            }
            Err(refused) => {
                self.last_error = Some(refused.error);
                for snapshot in refused.request.snapshots {
                    authority.return_dirty(snapshot);
                }
                0
            }
        }
    }

    /// Dispatches due cohorts oldest first. A refused dispatch stops the drain
    /// with the cohort preserved and no attempt advanced; the refusal surfaces
    /// as the latest scheduling error.
    fn dispatch_due_retries(&mut self, tick: u64) -> usize {
        let mut due: Vec<(u64, u64)> = self
            .pending
            .iter()
            .filter(|cohort| cohort.next_tick <= tick)
            .map(|cohort| (cohort.next_tick, cohort.id))
            .collect();
        due.sort();
        let mut submitted = 0;
        for (_, id) in due {
            let Some(position) = self.pending.iter().position(|cohort| cohort.id == id) else {
                continue;
            };
            let cohort = self.pending.remove(position);
            let attempt = cohort.attempts.saturating_add(1);
            let count = cohort.snapshots.len();
            let request = SaveRequest {
                snapshots: cohort.snapshots.clone(),
            };
            match self.store.submit(request) {
                Ok(ticket) => {
                    self.tracked.push(TrackedSubmit {
                        ticket,
                        kind: TrackedKind::Retry { id, attempt },
                    });
                    self.dispatched.insert(id, cohort);
                    submitted += count;
                }
                Err(refused) => {
                    // Queue-full preserves the cohort: attempt, deadline and
                    // snapshots return untouched for the next tick.
                    self.last_error = Some(refused.error);
                    self.pending.push(cohort);
                    break;
                }
            }
        }
        submitted
    }

    /// Dispatches every waiting cohort regardless of deadline, oldest first,
    /// for the final flush. A refusal stops with the remainder preserved.
    fn dispatch_all_retries(&mut self) -> bool {
        let mut order: Vec<(u64, u64)> = self
            .pending
            .iter()
            .map(|cohort| (cohort.next_tick, cohort.id))
            .collect();
        order.sort();
        let mut submitted = false;
        for (_, id) in order {
            let Some(position) = self.pending.iter().position(|cohort| cohort.id == id) else {
                continue;
            };
            let cohort = self.pending.remove(position);
            let attempt = cohort.attempts.saturating_add(1);
            let request = SaveRequest {
                snapshots: cohort.snapshots.clone(),
            };
            match self.store.submit(request) {
                Ok(ticket) => {
                    self.tracked.push(TrackedSubmit {
                        ticket,
                        kind: TrackedKind::Retry { id, attempt },
                    });
                    self.dispatched.insert(id, cohort);
                    submitted = true;
                }
                Err(refused) => {
                    self.last_error = Some(refused.error);
                    self.pending.push(cohort);
                    break;
                }
            }
        }
        submitted
    }

    /// Polls every tracked ticket once and applies the ready completions,
    /// counting durable and failed jobs. Returns the first completion error.
    fn drain_ready(
        &mut self,
        authority: &mut dyn SaveAuthority,
        tick: u64,
        durable: &mut usize,
        failed: &mut usize,
    ) -> Option<ServerError> {
        let mut ready = Vec::new();
        let mut waiting = Vec::new();
        for tracked in self.tracked.drain(..) {
            match self.store.poll(tracked.ticket) {
                SavePoll::Pending => waiting.push(tracked),
                SavePoll::Completed(completion) => ready.push((tracked, completion)),
            }
        }
        self.tracked = waiting;
        let mut first_error = None;
        for (tracked, completion) in ready {
            if let Some(error) = self.apply_completion(tracked, completion, authority, tick) {
                *failed += 1;
                if first_error.is_none() {
                    first_error = Some(error);
                }
            } else {
                *durable += 1;
            }
        }
        first_error
    }

    /// Runs the fixed metadata schedule: cadence or backoff marks the target
    /// pending, and at most one submission stays in flight with the latest
    /// frozen value. A refused dispatch keeps the pending target untouched.
    fn schedule_metadata(
        &mut self,
        tick: u64,
        authority: &mut dyn SaveAuthority,
    ) -> Result<(), ServerError> {
        if tick.is_multiple_of(self.config.autosave_interval) {
            self.metadata.pending = true;
        }
        if self.metadata.attempts != 0 && tick >= self.metadata.next_retry {
            self.metadata.pending = true;
        }
        if !self.metadata.pending || self.metadata.in_flight {
            return Ok(());
        }
        // Capture refusal retains the pending target without consuming a
        // submission or advancing the backend retry attempt.
        let snapshot = authority.try_metadata_snapshot().inspect_err(|error| {
            self.last_error = Some(*error);
        })?;
        if snapshot.revision <= self.metadata.committed {
            self.metadata.pending = false;
            return Ok(());
        }
        let sequence = snapshot.revision;
        match self.store.submit(SaveRequest {
            snapshots: vec![snapshot],
        }) {
            Ok(ticket) => {
                self.metadata.pending = false;
                self.metadata.in_flight = true;
                self.tracked.push(TrackedSubmit {
                    ticket,
                    kind: TrackedKind::Metadata { sequence },
                });
            }
            Err(_) => {
                // The fresh clone was never marked in flight, so nothing
                // returns dirty; the pending target retries next tick.
            }
        }
        Ok(())
    }
}

impl<B: DiskBackend> StoreHandle for AutosaveScheduler<B> {
    fn submit(&mut self, request: SaveRequest) -> Result<SaveTicket, SubmitSaveError> {
        // Direct submissions bypass the scheduling layer: the caller owns the
        // ticket and polls it, exactly as with the mailbox itself.
        self.store.submit(request)
    }

    fn poll(&mut self, ticket: SaveTicket) -> SavePoll {
        self.store.poll(ticket)
    }

    fn poll_tick(
        &mut self,
        tick: u64,
        budget: SaveBudget,
        authority: &mut dyn SaveAuthority,
    ) -> Result<SaveScheduleReport, ServerError> {
        let mut urgent = 0usize;
        let mut autosave = 0usize;
        // Completions settle at the next tick start, before any new dispatch.
        let mut durable = 0usize;
        let mut failed = 0usize;
        let _ = self.drain_ready(authority, tick, &mut durable, &mut failed);
        let retry = self.dispatch_due_retries(tick);
        urgent += self.submit_fresh(authority.select(SaveMode::Urgent, budget), authority);
        if tick.is_multiple_of(self.config.autosave_interval) {
            self.autosave_active = true;
        }
        if self.autosave_active {
            autosave += self.submit_fresh(authority.select(SaveMode::All, budget), authority);
            let stats = authority.save_stats();
            if stats.dirty == 0 && stats.in_flight == 0 {
                self.autosave_active = false;
            }
        }
        self.schedule_metadata(tick, authority)?;
        // Hand queued jobs to idle slots without waiting for the background owner.
        self.store.poll_tick(tick, budget, authority)?;
        let stats = authority.save_stats();
        self.backpressured = next_backpressure(
            self.backpressured,
            stats.estimated_unsaved_bytes,
            self.config.unsaved_byte_ceiling,
        );
        Ok(SaveScheduleReport {
            urgent,
            autosave,
            retry,
            stats,
            backpressured: self.backpressured,
        })
    }

    fn cancel_pending(&mut self) -> Result<Vec<OwnedSnapshot>, ServerError> {
        // Only unstarted mailbox snapshots return here; scheduler cohorts and
        // tracked tickets stay owned until their completions drain.
        self.store.cancel_pending()
    }

    fn flush(
        &mut self,
        deadline: Deadline,
        authority: &mut dyn SaveAuthority,
        clock: &dyn Clock,
    ) -> Result<FlushReport, ServerError> {
        let mut report = FlushReport::default();
        let mut last_refusal: Option<ServerError> = None;
        // Dirty, pending and retry work drains first, in that order.
        loop {
            self.store.check_flush_deadline(deadline, clock)?;
            let selected = authority.select(SaveMode::All, SaveBudget::default());
            let mut submitted = false;
            if !selected.is_empty() {
                match self.store.submit(SaveRequest {
                    snapshots: selected,
                }) {
                    Ok(ticket) => {
                        self.tracked.push(TrackedSubmit {
                            ticket,
                            kind: TrackedKind::Fresh,
                        });
                        submitted = true;
                    }
                    Err(refused) => {
                        last_refusal = Some(refused.error);
                        self.last_error = last_refusal;
                        for snapshot in refused.request.snapshots {
                            authority.return_dirty(snapshot);
                        }
                    }
                }
            }
            if self.dispatch_all_retries() {
                submitted = true;
            }
            // Hand queued jobs to idle worker slots; the tick argument is
            // unused by the mailbox, so the flush passes zero.
            self.store.poll_tick(0, SaveBudget::default(), authority)?;
            self.store.drive_workers();
            let durable_before_drain = report.durable;
            if let Some(error) =
                self.drain_ready(authority, 0, &mut report.durable, &mut report.failed)
            {
                return Err(error);
            }
            if self.tracked.is_empty() && self.pending.is_empty() {
                let stats = authority.save_stats();
                if stats.dirty == 0 && stats.in_flight == 0 {
                    break;
                }
                // A qualified ACK can make a newer dirty target eligible;
                // that progress permits another selection before a stall check.
                if !submitted && report.durable == durable_before_drain {
                    // The store refuses everything while nothing is in
                    // flight, so no progress is possible; report the refusal
                    // instead of spinning.
                    return Err(last_refusal.unwrap_or(ServerError::Internal {
                        invariant: "flush stalled",
                    }));
                }
            }
            if self.store.is_background() {
                self.store.wait_flush(deadline, clock)?;
            }
        }
        // The final metadata barrier runs after chunk work drains.
        let snapshot = authority.try_metadata_snapshot().inspect_err(|error| {
            self.metadata.pending = true;
            self.last_error = Some(*error);
        })?;
        if snapshot.revision > self.metadata.committed {
            self.store.check_flush_deadline(deadline, clock)?;
            let sequence = snapshot.revision;
            let ticket = match self.store.submit(SaveRequest {
                snapshots: vec![snapshot],
            }) {
                Ok(ticket) => ticket,
                Err(refused) => {
                    self.metadata.pending = true;
                    self.last_error = Some(refused.error);
                    return Err(refused.error);
                }
            };
            // Record ownership before any wait. A timed-out flush retries by
            // draining this ticket before selecting another metadata target.
            self.metadata.in_flight = true;
            self.metadata.pending = false;
            self.tracked.push(TrackedSubmit {
                ticket,
                kind: TrackedKind::Metadata { sequence },
            });
            loop {
                self.store.check_flush_deadline(deadline, clock)?;
                self.store.poll_tick(0, SaveBudget::default(), authority)?;
                self.store.drive_workers();
                if let Some(error) =
                    self.drain_ready(authority, 0, &mut report.durable, &mut report.failed)
                {
                    return Err(error);
                }
                if !self.tracked.iter().any(|tracked| tracked.ticket == ticket) {
                    break;
                }
                self.store.wait_flush(deadline, clock)?;
            }
        }
        report.outstanding = self.tracked.len() + self.pending.len();
        Ok(report)
    }

    fn sync(&mut self, deadline: Deadline) -> Result<(), ServerError> {
        self.store.sync(deadline)
    }

    fn close(&mut self, deadline: Deadline) -> Result<(), ServerError> {
        self.store.close(deadline)
    }
}

/// Load operations preserve the mailbox's sole backend and bounded ledgers.
impl<B: DiskBackend> crate::core::contracts::PlayerLoadPort for AutosaveScheduler<B> {
    fn start(&mut self, player: PlayerId, deadline: Deadline) -> Result<LoginTicket, ServerError> {
        crate::core::contracts::PlayerLoadPort::start(&mut self.store, player, deadline)
    }
    fn poll(&mut self, ticket: LoginTicket) -> LoadPoll {
        crate::core::contracts::PlayerLoadPort::poll(&mut self.store, ticket)
    }
    fn cancel(&mut self, ticket: LoginTicket) -> Result<(), ServerError> {
        crate::core::contracts::PlayerLoadPort::cancel(&mut self.store, ticket)
    }
}
impl<B: DiskBackend> ChunkLoadPort for AutosaveScheduler<B> {
    fn start_chunk(
        &mut self,
        key: ChunkKey,
        generation: u64,
        deadline: Deadline,
    ) -> Result<ChunkRequestId, ServerError> {
        ChunkLoadPort::start_chunk(&mut self.store, key, generation, deadline)
    }
    fn poll_chunk(&mut self, request: ChunkRequestId) -> ChunkLoadPoll {
        ChunkLoadPort::poll_chunk(&mut self.store, request)
    }
    fn cancel_chunk(&mut self, request: ChunkRequestId) -> Result<(), ServerError> {
        ChunkLoadPort::cancel_chunk(&mut self.store, request)
    }
}
