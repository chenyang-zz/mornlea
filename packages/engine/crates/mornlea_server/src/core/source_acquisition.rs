//! Temporary consumer-only source acquisition caller.
//!
//! Plan 109 tests/API checkpoint only: this struct is an intentionally
//! unaccepted consumer-only borrowed caller. It owns one existing
//! [`ChunkDriver`] and borrows the background `AutosaveScheduler` and
//! `GenerationPool` without taking, closing or cancelling those owners.
//! `advance` drives the real providers once, polls the driver once and runs
//! the ordinary `AuthorityState::advance_tick`; it derives no source goals,
//! starts no load or generation and replaces no wants, so the appended
//! primary cases fail on actual automatic behavior rather than on fixture
//! declarations. Never read this stub as completed goal integration: the
//! producer phase owns the frozen private goal book and admission seams.

use super::chunk_driver::{ChunkDriver, ChunkPollReport};
use super::contracts::{ChunkKey, Deadline, DiskBackend, ServerError, TickBudget, TickPublication};
use super::generation_worker::GenerationPool;
use super::state::AuthorityState;
use crate::store::scheduler::AutosaveScheduler;

/// Which real provider kind one successful start used.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SourceChunkKind {
    Load,
    Generate,
}

/// One successful actual provider start, never a merely selected job.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SourceChunkStart {
    pub key: ChunkKey,
    pub kind: SourceChunkKind,
}

/// One nonblocking pass over the borrowed owners plus the ordinary tick.
pub struct SourceAcquisitionTick {
    pub publication: TickPublication,
    pub poll: ChunkPollReport,
    pub started: Vec<SourceChunkStart>,
    pub queued: usize,
    pub first_error: Option<ServerError>,
}

/// Consumer-only owner of one chunk driver; no goal book exists yet.
#[derive(Default)]
pub struct SourceAcquisition {
    driver: ChunkDriver,
}

impl SourceAcquisition {
    pub fn new() -> Self {
        Self::default()
    }
    /// Forwards the driver's load ownership count.
    pub fn pending_loads(&self) -> usize {
        self.driver.pending_loads()
    }
    /// Forwards the driver's CPU ownership count.
    pub fn pending_generations(&self) -> usize {
        self.driver.pending_generations()
    }
    /// Retained unstarted candidate count; the consumer stub retains none.
    pub fn pending_candidates(&self) -> usize {
        0
    }
    /// Drives the borrowed providers once, polls the driver once, then runs
    /// the ordinary tick. The unused deadline is reserved for the producer
    /// phase's bounded admission; nothing here blocks on it.
    pub fn advance<B: DiskBackend>(
        &mut self,
        state: &mut AuthorityState,
        store: &mut AutosaveScheduler<B>,
        generations: &mut GenerationPool,
        budget: TickBudget,
        _deadline: Deadline,
    ) -> Result<SourceAcquisitionTick, ServerError> {
        store.drive_workers();
        generations.drive();
        let poll = self.driver.poll(state, store, generations);
        let publication = state.advance_tick(budget)?;
        Ok(SourceAcquisitionTick {
            publication,
            poll,
            started: Vec::new(),
            queued: 0,
            first_error: poll.first_error,
        })
    }
}
