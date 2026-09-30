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
//! worker slots and reports the authority's save statistics. Encoding,
//! backend writes, sync and close wait for the off-tick
//! [`StoreMailbox::drive_workers`], `flush`, `sync` and `close` boundaries.

use std::collections::VecDeque;

use mornlea_storage::{
    CHUNK_CURRENT_SCHEMA, CHUNK_ENVELOPE_LENGTH, ChunkCodec, MAX_COMPRESSED_CHUNK, StorageError,
    chunk_logical_len, companions_encoded_len, hostile_mobs_encoded_len, passive_mobs_encoded_len,
    player_encoded_len, world_metadata_encoded_len,
};

use crate::core::contracts::{
    Clock, Deadline, DiskBackend, FlushReport, Operation, OwnedSnapshot, SaveAuthority, SaveBudget,
    SaveCompletion, SaveKey, SaveOccupancy, SavePoll, SaveRequest, SaveScheduleReport, SaveTicket,
    SaveValue, ServerError, ServerPhase, StoreHandle, StoreLimits, SubmitSaveError,
};

/// Reservation held for one owned chunk until encoding completes.
const CHUNK_MAX_RESERVATION: usize = MAX_COMPRESSED_CHUNK as usize + CHUNK_ENVELOPE_LENGTH;

/// One admitted save job: the caller's untouched request plus the store's
/// current reservation per snapshot, aligned by index.
struct Job {
    ticket: SaveTicket,
    request: SaveRequest,
    reservations: Vec<usize>,
}

/// A finished job the consumer has not polled yet. The store keeps owning the
/// snapshots and their reservations until the poll releases them.
struct StoredCompletion {
    ticket: SaveTicket,
    completion: SaveCompletion,
    reservations: Vec<usize>,
}

/// Bounded durable store mailbox over the frozen store surface.
///
/// The two workers are slots the tick path fills and the off-tick
/// [`StoreMailbox::drive_workers`] boundary executes; the final architecture
/// runs that boundary on the storage worker threads.
pub struct StoreMailbox<B: DiskBackend> {
    limits: StoreLimits,
    backend: B,
    codec: ChunkCodec,
    scratch: Vec<u8>,
    next_ticket: u64,
    queue: VecDeque<Job>,
    workers: Vec<Option<Job>>,
    completions: Vec<StoredCompletion>,
    occupancy: SaveOccupancy,
    closed: bool,
}

impl<B: DiskBackend> StoreMailbox<B> {
    /// Builds the mailbox over `limits` and `backend`, rejecting a limit set
    /// without a worker because no job could ever leave the queue.
    pub fn try_new(limits: StoreLimits, backend: B) -> Result<Self, ServerError> {
        if limits.workers() == 0 {
            return Err(ServerError::InvalidInput {
                field: "store_workers",
            });
        }
        let codec = ChunkCodec::try_new().map_err(|_| ServerError::Internal {
            invariant: "store chunk codec",
        })?;
        let workers = (0..limits.workers()).map(|_| None).collect();
        Ok(Self {
            limits,
            backend,
            codec,
            scratch: vec![0u8; CHUNK_MAX_RESERVATION],
            next_ticket: 0,
            queue: VecDeque::new(),
            workers,
            completions: Vec::new(),
            occupancy: SaveOccupancy::default(),
            closed: false,
        })
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

    /// Moves queued jobs into idle worker slots. Memory-only, so the tick
    /// path may call it; the write itself waits for [`Self::drive_workers`].
    fn dispatch(&mut self) {
        for slot in self.workers.iter_mut() {
            if slot.is_none() {
                let Some(job) = self.queue.pop_front() else {
                    break;
                };
                *slot = Some(job);
            }
        }
    }

    /// Executes every busy worker slot: encodes each chunk and shrinks its
    /// reservation to the actual encoded length, then hands the backend an
    /// equal copy of the request. The mailbox retains the original records so
    /// the completion always returns exactly what was submitted, whatever the
    /// backend echoes. Off-tick only.
    pub fn drive_workers(&mut self) -> usize {
        let mut completed = 0;
        for slot in self.workers.iter_mut() {
            let Some(mut job) = slot.take() else {
                continue;
            };
            let submitted: Vec<(SaveKey, u64)> = job
                .request
                .snapshots
                .iter()
                .map(|snapshot| (snapshot.key.clone(), snapshot.revision))
                .collect();
            let mut encode_error = None;
            for (index, snapshot) in job.request.snapshots.iter().enumerate() {
                let SaveValue::Chunk(save) = &snapshot.value else {
                    continue;
                };
                match self.codec.encode_into(save, &mut self.scratch) {
                    Ok(encoded) => {
                        let previous = job.reservations[index];
                        debug_assert!(previous >= encoded, "shrink below the actual length");
                        self.occupancy.encoded_bytes = self
                            .occupancy
                            .encoded_bytes
                            .saturating_sub(previous - encoded);
                        job.reservations[index] = encoded;
                    }
                    Err(error) => {
                        encode_error = Some(store_io_error(error));
                        break;
                    }
                }
            }
            let (committed, error) = match encode_error {
                // An encode failure persists nothing: the original snapshots
                // return through the completion for retry.
                Some(error) => (Vec::new(), Some(error)),
                None => {
                    let returned = self.backend.write(job.ticket, job.request.clone());
                    if returned.ticket == job.ticket {
                        (returned.committed, returned.error)
                    } else {
                        // Duplicate or stale ticket completion: report the
                        // mismatch and adopt none of its revisions.
                        (
                            Vec::new(),
                            Some(ServerError::Internal {
                                invariant: "store completion ticket",
                            }),
                        )
                    }
                }
            };
            let completion = SaveCompletion {
                ticket: job.ticket,
                snapshots: job.request.snapshots,
                submitted,
                committed,
                error,
            };
            self.completions.push(StoredCompletion {
                ticket: job.ticket,
                completion,
                reservations: job.reservations,
            });
            completed += 1;
        }
        completed
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
        // The admission copy carries the store's reservation bytes, while the
        // retained request keeps the caller's records untouched so a
        // completion returns exactly what was submitted.
        let mut admission = request.clone();
        for (snapshot, reservation) in admission.snapshots.iter_mut().zip(&reservations) {
            snapshot.estimated_bytes = *reservation;
        }
        let occupancy = match self.limits.try_admit(&self.occupancy, &admission) {
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
            request,
            reservations,
        });
        Ok(ticket)
    }

    fn poll(&mut self, ticket: SaveTicket) -> SavePoll {
        let Some(position) = self
            .completions
            .iter()
            .position(|stored| stored.ticket == ticket)
        else {
            return SavePoll::Pending;
        };
        let stored = self.completions.remove(position);
        let snapshots = stored.completion.snapshots.clone();
        self.release(&snapshots, &stored.reservations);
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
            cancelled.extend(request.snapshots);
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
        // Only flush blocks outside the tick: drive every accepted job to a
        // completion, then apply each one to the authority exactly once.
        while !self.queue.is_empty() || self.worker_jobs() > 0 {
            if deadline.expired(clock.monotonic()) {
                return Err(ServerError::Timeout {
                    operation: Operation::Flush,
                });
            }
            self.dispatch();
            self.drive_workers();
        }
        let mut report = FlushReport::default();
        let drained: Vec<StoredCompletion> = self.completions.drain(..).collect();
        for stored in drained {
            let failed = stored.completion.error.is_some();
            let snapshots = stored.completion.snapshots.clone();
            let _ack = authority.apply_completion(stored.completion);
            self.release(&snapshots, &stored.reservations);
            if failed {
                report.failed += 1;
            } else {
                report.durable += 1;
            }
        }
        report.outstanding = self.queued_jobs() + self.worker_jobs();
        Ok(report)
    }

    fn sync(&mut self, _deadline: Deadline) -> Result<(), ServerError> {
        if self.closed {
            return Err(ServerError::InvalidState {
                phase: ServerPhase::Closed,
            });
        }
        // The synchronous backend completes without blocking, so the caller
        // deadline cannot elapse here; real backends own deadline behavior.
        self.backend.sync()
    }

    fn close(&mut self, _deadline: Deadline) -> Result<(), ServerError> {
        if self.closed {
            return Err(ServerError::InvalidState {
                phase: ServerPhase::Closed,
            });
        }
        self.backend.close()?;
        self.closed = true;
        Ok(())
    }
}

/// Maps a codec rejection onto the store error space: a refused record is an
/// I/O-shaped write failure, and an undersized scratch is a store invariant
/// the mailbox sizes to the compression maximum.
fn store_io_error(error: StorageError) -> ServerError {
    match error {
        StorageError::OutputTooSmall { .. } => ServerError::Internal {
            invariant: "store chunk scratch",
        },
        StorageError::Corrupt(_) | StorageError::FutureVersion(_) => ServerError::Io {
            operation: Operation::WritePayload,
            kind: std::io::ErrorKind::InvalidData,
        },
    }
}
