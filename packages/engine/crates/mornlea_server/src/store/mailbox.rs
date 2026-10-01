//! Durable bounded store mailbox.
//!
//! [`StoreMailbox`] owns the submit/poll/worker-completion save pipeline over
//! the frozen [`StoreHandle`] surface and a caller-supplied [`DiskBackend`].
//! The backend is a dependency, never an assumption: this module performs no
//! disk persistence itself.
//!
//! Ownership and limits:
//!
//! * Admission reserves the exact codec encoded length for every standalone
//!   family record and the compression maximum (`MAX_COMPRESSED_CHUNK` plus
//!   `CHUNK_ENVELOPE_LENGTH`) for every chunk, because a chunk's compressed
//!   size is unknowable before encoding. The chunk worker shrinks each chunk
//!   reservation to the actual encoded length right after encoding; an encode
//!   failure persists nothing and returns the original snapshots through the
//!   completion. Admission rejects a record the codec refuses, so the store
//!   never holds an unencodable value and never invents a length.
//! * One occupancy counter covers the queue, both worker slots and unconsumed
//!   completions, so each allocation is charged exactly once wherever it
//!   sits. A completed completion stays charged until its consumer polls it,
//!   and a full lane or job returns the whole request instead of a partial
//!   admission. Retry ownership returns through the completion itself; the
//!   scheduler that owns autosave and retry cadence re-submits it as a fresh
//!   allocation.
//! * A duplicate or stale ticket completion — a backend answer wearing a
//!   ticket other than the dispatched job's — is reported as an error on the
//!   job's completion and none of its revisions are adopted, so it cannot
//!   clear a newer in-flight snapshot.
//!
//! Tick safety: [`StoreMailbox::poll_tick`] only hands queued jobs to idle
//! worker slots and reports the authority's save statistics. Background
//! construction performs encoding and filesystem work on its sole owner thread;
//! inline construction reserves synchronous drive for deterministic doubles.
//! Only flush, sync and close may wait off the tick.

use std::collections::VecDeque;
use std::sync::{Arc, mpsc};

use super::background::{Background, Handoff, LifecycleKind, SaveExecutor, SaveResult, internal};

use mornlea_storage::{
    CHUNK_CURRENT_SCHEMA, CHUNK_ENVELOPE_LENGTH, MAX_COMPRESSED_CHUNK, chunk_logical_len,
    companions_encoded_len, hostile_mobs_encoded_len, passive_mobs_encoded_len, player_encoded_len,
    world_metadata_encoded_len,
};

use crate::core::contracts::{
    Clock, Deadline, DiskBackend, FlushReport, Operation, OwnedSnapshot, SaveAuthority, SaveBudget,
    SaveCompletion, SaveKey, SaveOccupancy, SavePoll, SaveRequest, SaveScheduleReport, SaveTicket,
    SaveValue, ServerError, ServerPhase, StoreHandle, StoreLimits, SubmitSaveError,
};

/// Reservation held for one owned chunk until encoding completes.
pub(super) const CHUNK_MAX_RESERVATION: usize =
    MAX_COMPRESSED_CHUNK as usize + CHUNK_ENVELOPE_LENGTH;

/// One admitted save job: the caller's untouched request plus the store's
/// current reservation per snapshot, aligned by index.
struct Job {
    ticket: SaveTicket,
    request: Arc<SaveRequest>,
    reservations: Vec<usize>,
}

/// A finished job the consumer has not polled yet. The store keeps owning the
/// snapshots and their reservations until the poll releases them.
struct StoredCompletion {
    ticket: SaveTicket,
    completion: SaveCompletion,
    reservations: Vec<usize>,
}

/// Inline doubles keep their original execution semantics. The background
/// variant transfers the backend and its codec to one filesystem owner.
enum Owner<B> {
    Inline { backend: B, executor: SaveExecutor },
    Background(Background),
}
struct Dispatched {
    job: Job,
    reply: Option<mpsc::Receiver<SaveResult>>,
}

/// Bounded durable store mailbox. The authority retains immutable original
/// snapshots and charged occupancy while the background owner performs I/O.
pub struct StoreMailbox<B: DiskBackend> {
    limits: StoreLimits,
    owner: Owner<B>,
    next_ticket: u64,
    queue: VecDeque<Job>,
    workers: Vec<Option<Dispatched>>,
    completions: Vec<StoredCompletion>,
    occupancy: SaveOccupancy,
    closed: bool,
}

impl<B: DiskBackend> StoreMailbox<B> {
    /// Inline compatibility boundary for deterministic, non-Send doubles.
    pub fn try_new(limits: StoreLimits, backend: B) -> Result<Self, ServerError> {
        Self::validate_workers(limits)?;
        let executor = SaveExecutor::try_new()?;
        Ok(Self::with_owner(
            limits,
            Owner::Inline { backend, executor },
        ))
    }

    /// Moves the backend and its sole codec into one bounded background owner.
    /// Startup opens and reads remain the caller's off-tick responsibility.
    pub fn try_new_background(limits: StoreLimits, backend: B) -> Result<Self, ServerError>
    where
        B: Send + 'static,
    {
        Self::validate_workers(limits)?;
        let executor = SaveExecutor::try_new()?;
        let owner = Background::spawn(limits.workers() + 1, backend, executor)?;
        Ok(Self::with_owner(limits, Owner::Background(owner)))
    }

    fn validate_workers(limits: StoreLimits) -> Result<(), ServerError> {
        if limits.workers() == 0 {
            return Err(ServerError::InvalidInput {
                field: "store_workers",
            });
        }
        Ok(())
    }
    fn with_owner(limits: StoreLimits, owner: Owner<B>) -> Self {
        Self {
            limits,
            owner,
            next_ticket: 0,
            queue: VecDeque::new(),
            workers: (0..limits.workers()).map(|_| None).collect(),
            completions: Vec::new(),
            occupancy: SaveOccupancy::default(),
            closed: false,
        }
    }

    pub(crate) fn is_background(&self) -> bool {
        matches!(self.owner, Owner::Background(_))
    }

    /// Caller clocks govern inline doubles. Real host time additionally bounds
    /// waits for the background owner, whose operation outlives a timed-out caller.
    pub(crate) fn check_flush_deadline(
        &self,
        deadline: Deadline,
        clock: &dyn Clock,
    ) -> Result<(), ServerError> {
        if deadline.expired(clock.monotonic())
            || (self.is_background() && deadline.expired(std::time::Instant::now()))
        {
            return Err(ServerError::Timeout {
                operation: Operation::Flush,
            });
        }
        Ok(())
    }
    pub(crate) fn wait_flush(
        &self,
        deadline: Deadline,
        clock: &dyn Clock,
    ) -> Result<(), ServerError> {
        self.check_flush_deadline(deadline, clock)?;
        if self.is_background() {
            super::background::wait(deadline, Operation::Flush)?;
        }
        Ok(())
    }

    /// Current owned occupancy: jobs and per-lane counts across the queue,
    /// both workers and unconsumed completions, with reservation bytes.
    pub fn occupancy(&self) -> SaveOccupancy {
        self.occupancy.clone()
    }

    /// Jobs accepted but not yet handed to a worker slot.
    pub fn queued_jobs(&self) -> usize {
        self.queue.len()
    }

    /// Jobs currently held by a worker slot.
    pub fn worker_jobs(&self) -> usize {
        self.workers.iter().filter(|slot| slot.is_some()).count()
    }

    /// Finished jobs waiting for their consumer to poll them.
    pub fn held_completions(&self) -> usize {
        self.completions.len()
    }

    /// Nonblocking handoff leaves a full channel's original job queued.
    fn dispatch(&mut self) {
        for index in 0..self.workers.len() {
            if self.workers[index].is_some() {
                continue;
            }
            let Some(job) = self.queue.front() else {
                break;
            };
            let reply = match &self.owner {
                Owner::Inline { .. } => None,
                Owner::Background(owner) => {
                    match owner.try_save(job.ticket, &job.request, &job.reservations) {
                        Handoff::Sent(reply) => Some(reply),
                        Handoff::Full => break,
                        Handoff::Disconnected => {
                            let job = self.queue.pop_front().expect("queued owner");
                            let result = SaveResult {
                                ticket: job.ticket,
                                committed: Vec::new(),
                                error: Some(internal("store owner disconnected")),
                                reservations: job.reservations.clone(),
                            };
                            self.finish(job, result);
                            continue;
                        }
                    }
                }
            };
            self.workers[index] = Some(Dispatched {
                job: self.queue.pop_front().expect("queued owner"),
                reply,
            });
        }
    }

    fn finish(&mut self, job: Job, result: SaveResult) {
        let request =
            Arc::try_unwrap(job.request).expect("worker drops immutable request before reply");
        let submitted = request
            .snapshots
            .iter()
            .map(|snapshot| (snapshot.key.clone(), snapshot.revision))
            .collect();
        for (previous, actual) in job.reservations.iter().zip(&result.reservations) {
            debug_assert!(previous >= actual, "shrink below actual length");
            self.occupancy.encoded_bytes = self
                .occupancy
                .encoded_bytes
                .saturating_sub(previous - actual);
        }
        let (committed, error) = if result.ticket == job.ticket {
            (result.committed, result.error)
        } else {
            (Vec::new(), Some(internal("store completion ticket")))
        };
        self.completions.push(StoredCompletion {
            ticket: job.ticket,
            completion: SaveCompletion {
                ticket: job.ticket,
                snapshots: request.snapshots,
                submitted,
                committed,
                error,
            },
            reservations: result.reservations,
        });
    }

    /// Adopts facts only. The worker has released its Arc before publication,
    /// so original snapshots move into completion without cloning their bodies.
    fn collect_ready(&mut self) -> usize {
        let mut completed = 0;
        for index in 0..self.workers.len() {
            let Some(slot) = &self.workers[index] else {
                continue;
            };
            let Some(reply) = &slot.reply else {
                continue;
            };
            let result = match reply.try_recv() {
                Ok(result) => result,
                Err(mpsc::TryRecvError::Empty) => continue,
                Err(mpsc::TryRecvError::Disconnected) => SaveResult {
                    ticket: slot.job.ticket,
                    committed: Vec::new(),
                    error: Some(internal("store owner disconnected")),
                    reservations: slot.job.reservations.clone(),
                },
            };
            let slot = self.workers[index].take().expect("dispatched owner");
            self.finish(slot.job, result);
            completed += 1;
        }
        completed
    }

    /// Inline construction executes slots synchronously. Background construction
    /// only hands off jobs and collects available facts, returning while I/O runs.
    pub fn drive_workers(&mut self) -> usize {
        if self.is_background() {
            let completed = self.collect_ready();
            self.dispatch();
            return completed + self.collect_ready();
        }
        let mut completed = 0;
        for index in 0..self.workers.len() {
            let Some(slot) = self.workers[index].take() else {
                continue;
            };
            let Owner::Inline { backend, executor } = &mut self.owner else {
                unreachable!()
            };
            let result = executor.save(
                backend,
                slot.job.ticket,
                &slot.job.request,
                slot.job.reservations.clone(),
            );
            self.finish(slot.job, result);
            completed += 1;
        }
        completed
    }

    fn apply_ready(&mut self, authority: &mut dyn SaveAuthority, report: &mut FlushReport) {
        let drained: Vec<StoredCompletion> = self.completions.drain(..).collect();
        for stored in drained {
            let failed = stored.completion.error.is_some();
            self.release(&stored.completion.snapshots, &stored.reservations);
            let _ack = authority.apply_completion(stored.completion);
            if failed {
                report.failed += 1;
            } else {
                report.durable += 1;
            }
        }
    }

    /// Reservation the store holds for one snapshot: the exact codec encoded
    /// length for the standalone families, the compression maximum for a
    /// chunk. A record the codec refuses is rejected here, before enqueue.
    fn reservation(snapshot: &OwnedSnapshot) -> Result<usize, ServerError> {
        match &snapshot.value {
            SaveValue::Chunk(save) => {
                chunk_logical_len(save, CHUNK_CURRENT_SCHEMA).map_err(|_| {
                    ServerError::InvalidInput {
                        field: "save_value",
                    }
                })?;
                Ok(CHUNK_MAX_RESERVATION)
            }
            SaveValue::Player(save) => {
                player_encoded_len(save).map_err(|_| ServerError::InvalidInput {
                    field: "save_value",
                })
            }
            SaveValue::Companions(save) => {
                companions_encoded_len(save).map_err(|_| ServerError::InvalidInput {
                    field: "save_value",
                })
            }
            SaveValue::Hostiles(save) => {
                hostile_mobs_encoded_len(save).map_err(|_| ServerError::InvalidInput {
                    field: "save_value",
                })
            }
            SaveValue::Passives(save) => {
                passive_mobs_encoded_len(save).map_err(|_| ServerError::InvalidInput {
                    field: "save_value",
                })
            }
            SaveValue::Metadata(save) => {
                world_metadata_encoded_len(save).map_err(|_| ServerError::InvalidInput {
                    field: "save_value",
                })
            }
        }
    }

    fn next_ticket(&mut self) -> Result<SaveTicket, ServerError> {
        self.next_ticket = self
            .next_ticket
            .checked_add(1)
            .ok_or(ServerError::Internal {
                invariant: "store ticket space",
            })?;
        SaveTicket::try_from_raw(self.next_ticket)
    }

    /// Releases one job's occupancy: every allocation is dropped from its
    /// lane, its reservation bytes leave the ceiling and the job slot frees.
    fn release(&mut self, snapshots: &[OwnedSnapshot], reservations: &[usize]) {
        for (snapshot, reservation) in snapshots.iter().zip(reservations) {
            let lane = match &snapshot.key {
                SaveKey::Player(_) => &mut self.occupancy.players,
                SaveKey::Companions => &mut self.occupancy.companions,
                SaveKey::Hostiles => &mut self.occupancy.hostiles,
                SaveKey::Passives => &mut self.occupancy.passives,
                SaveKey::Metadata => &mut self.occupancy.metadata,
                SaveKey::Chunk(_) => &mut self.occupancy.chunks,
            };
            debug_assert!(*lane >= 1, "lane underflow on release");
            *lane = lane.saturating_sub(1);
            debug_assert!(
                self.occupancy.encoded_bytes >= *reservation,
                "reservation underflow on release"
            );
            self.occupancy.encoded_bytes =
                self.occupancy.encoded_bytes.saturating_sub(*reservation);
        }
        debug_assert!(self.occupancy.jobs >= 1, "job underflow on release");
        self.occupancy.jobs = self.occupancy.jobs.saturating_sub(1);
    }
}

impl<B: DiskBackend> StoreHandle for StoreMailbox<B> {
    fn submit(&mut self, request: SaveRequest) -> Result<SaveTicket, SubmitSaveError> {
        if self.closed {
            return Err(SubmitSaveError {
                error: ServerError::InvalidState {
                    phase: ServerPhase::Closed,
                },
                request,
            });
        }
        let mut reservations = Vec::with_capacity(request.snapshots.len());
        for snapshot in &request.snapshots {
            match Self::reservation(snapshot) {
                Ok(reservation) => reservations.push(reservation),
                Err(error) => return Err(SubmitSaveError { error, request }),
            }
        }
        let occupancy =
            match self
                .limits
                .try_admit_reserved(&self.occupancy, &request, &reservations)
            {
                Ok(occupancy) => occupancy,
                Err(error) => return Err(SubmitSaveError { error, request }),
            };
        let ticket = match self.next_ticket() {
            Ok(ticket) => ticket,
            Err(error) => return Err(SubmitSaveError { error, request }),
        };
        self.occupancy = occupancy;
        self.queue.push_back(Job {
            ticket,
            request: Arc::new(request),
            reservations,
        });
        Ok(ticket)
    }

    fn poll(&mut self, ticket: SaveTicket) -> SavePoll {
        self.collect_ready();
        let Some(position) = self
            .completions
            .iter()
            .position(|stored| stored.ticket == ticket)
        else {
            return SavePoll::Pending;
        };
        let stored = self.completions.remove(position);
        self.release(&stored.completion.snapshots, &stored.reservations);
        SavePoll::Completed(stored.completion)
    }

    fn poll_tick(
        &mut self,
        _tick: u64,
        _budget: SaveBudget,
        authority: &mut dyn SaveAuthority,
    ) -> Result<SaveScheduleReport, ServerError> {
        if self.closed {
            return Err(ServerError::InvalidState {
                phase: ServerPhase::Closed,
            });
        }
        // Tick-path work is a memory-only handoff; selection cadence and
        // backpressure belong to the scheduler node.
        self.collect_ready();
        self.dispatch();
        let stats = authority.save_stats();
        Ok(SaveScheduleReport {
            urgent: 0,
            autosave: 0,
            retry: 0,
            stats,
            backpressured: false,
        })
    }

    fn cancel_pending(&mut self) -> Result<Vec<OwnedSnapshot>, ServerError> {
        let mut cancelled = Vec::new();
        while let Some(job) = self.queue.pop_front() {
            let Job {
                ticket: _,
                request,
                reservations,
            } = job;
            self.release(&request.snapshots, &reservations);
            cancelled.extend(
                Arc::try_unwrap(request)
                    .expect("unstarted original owner")
                    .snapshots,
            );
        }
        // Worker-held and completed ownership stays store-held until poll;
        // only unstarted snapshots come back here.
        Ok(cancelled)
    }

    fn flush(
        &mut self,
        deadline: Deadline,
        authority: &mut dyn SaveAuthority,
        clock: &dyn Clock,
    ) -> Result<FlushReport, ServerError> {
        if self.closed {
            return Err(ServerError::InvalidState {
                phase: ServerPhase::Closed,
            });
        }
        let mut report = FlushReport::default();
        if !self.is_background() {
            // Inline doubles keep virtual-clock ordering: all jobs execute
            // before completions are applied, with no host-time wait.
            while !self.queue.is_empty() || self.worker_jobs() > 0 {
                self.check_flush_deadline(deadline, clock)?;
                self.dispatch();
                self.drive_workers();
            }
            self.apply_ready(authority, &mut report);
            return Ok(report);
        }
        loop {
            self.check_flush_deadline(deadline, clock)?;
            self.dispatch();
            self.drive_workers();
            self.apply_ready(authority, &mut report);
            if self.queue.is_empty() && self.worker_jobs() == 0 {
                break;
            }
            self.wait_flush(deadline, clock)?;
        }
        report.outstanding = self.queued_jobs() + self.worker_jobs();
        Ok(report)
    }

    fn sync(&mut self, deadline: Deadline) -> Result<(), ServerError> {
        if self.closed {
            return Err(ServerError::InvalidState {
                phase: ServerPhase::Closed,
            });
        }
        match &mut self.owner {
            Owner::Inline { backend, .. } => backend.sync(),
            Owner::Background(owner) => owner.lifecycle(LifecycleKind::Sync, deadline),
        }
    }

    fn close(&mut self, deadline: Deadline) -> Result<(), ServerError> {
        if self.closed {
            return Err(ServerError::InvalidState {
                phase: ServerPhase::Closed,
            });
        }
        match &mut self.owner {
            Owner::Inline { backend, .. } => backend.close()?,
            Owner::Background(owner) => owner.lifecycle(LifecycleKind::Close, deadline)?,
        }
        self.closed = true;
        Ok(())
    }
}
