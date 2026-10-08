//! Exclusive source tick ownership while the dedicated actual codec progresses.

use super::*;
use crate::core::source_encoding::SourceSnapshotEncoding;
use crate::core::state::SourceTickContinuation;
use std::time::Instant;

/// Nonblocking progress of the same retained reduction.
// One retained tick transfers its bounded complete report without a completion allocation.
#[allow(clippy::large_enum_variant)]
pub enum SourceAcquisitionPoll {
    Pending,
    Ready(SourceAcquisitionTick),
}

/// Borrows every mutable provider and authority owner until its original tick settles.
pub struct SourceAcquisitionPending<'a, B: DiskBackend> {
    driver: &'a mut ChunkDriver,
    continuation: SourceTickContinuation<'a>,
    store: &'a mut AutosaveScheduler<B>,
    generations: &'a mut GenerationPool,
    encoding: &'a mut SourceSnapshotEncoding,
    poll: ChunkPollReport,
    terminal: bool,
}

impl SourceAcquisition {
    /// Admission validates all owners before driving providers or committing simulation.
    pub fn begin_encoded_tick<'a, B: DiskBackend>(
        &'a mut self,
        state: &'a mut AuthorityState,
        store: &'a mut AutosaveScheduler<B>,
        generations: &'a mut GenerationPool,
        encoding: &'a mut SourceSnapshotEncoding,
        budget: TickBudget,
    ) -> Result<SourceAcquisitionPending<'a, B>, ServerError> {
        state.check_source_tick(budget)?;
        store.source_chunk_slots()?;
        if let Some(error) = self.driver.last_error() {
            return Err(error);
        }
        encoding.check_source_pass()?;
        store.drive_workers();
        generations.drive();
        let poll = self.driver.poll(state, store, generations);
        let mut continuation = state.begin_source_tick(budget, &mut self.goals)?;
        if let Err(error) = continuation.prepare_encoding() {
            return Err(continuation.fail_encoding(error));
        }
        Ok(SourceAcquisitionPending {
            driver: &mut self.driver,
            continuation,
            store,
            generations,
            encoding,
            poll,
            terminal: false,
        })
    }
}

impl<B: DiskBackend> SourceAcquisitionPending<'_, B> {
    pub fn state(&self) -> &AuthorityState {
        self.continuation.state()
    }
    pub fn pending_snapshot_requests(&self) -> usize {
        self.encoding.charged_requests()
    }
    /// An expired attempt retains the original tick without collecting, starting or finishing.
    pub fn poll(&mut self, deadline: Deadline) -> Result<SourceAcquisitionPoll, ServerError> {
        if self.terminal {
            return Err(ServerError::InvalidInput {
                field: "source_encoded_tick",
            });
        }
        if deadline.expired(Instant::now()) {
            return Ok(SourceAcquisitionPoll::Pending);
        }
        match self.continuation.poll_encoding(self.encoding) {
            Ok(false) => Ok(SourceAcquisitionPoll::Pending),
            Ok(true) => {
                self.terminal = true;
                let driver = &mut *self.driver;
                let store = &mut *self.store;
                let generations = &mut *self.generations;
                let poll = self.poll;
                self.continuation
                    .complete_encoded_with(|state, goals, publication| {
                        admit_source_starts(
                            driver,
                            goals,
                            state,
                            store,
                            generations,
                            publication,
                            poll,
                            deadline,
                        )
                    })
                    .map(SourceAcquisitionPoll::Ready)
            }
            Err(error) => {
                self.terminal = true;
                let _ = self.encoding.retain_current(|_, _, _, _| false);
                Err(self.continuation.fail_encoding(error))
            }
        }
    }
}
impl<B: DiskBackend> Drop for SourceAcquisitionPending<'_, B> {
    fn drop(&mut self) {
        if !self.terminal {
            // Cancel routing only; started CPU ownership remains charged until its reply is collected.
            let _ = self.encoding.retain_current(|_, _, _, _| false);
        }
    }
}
