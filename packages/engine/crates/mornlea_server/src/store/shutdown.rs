//! Shutdown ports over one autosave scheduler.
//!
//! The core shutdown machine borrows the store twice at once: as the
//! [`ActorPersistence`] that flushes one save family at a time and as the
//! [`StoreHandle`] that syncs and closes. Both ports here point to the same
//! [`AutosaveScheduler`], held in a single-threaded `RefCell`. Every port
//! method borrows the scheduler for the duration of that one call and
//! releases it before returning; no borrow outlives a call, so the two ports
//! never observe each other mid-operation. A reentrant borrow is reported as
//! a typed internal error, never a panic.
//!
//! The actor port overrides `flush_authority`: it selects the requested
//! family's snapshot from the authority and returns only after the store
//! reports it durable. The authority-free `flush` refuses, because without
//! the authority it could only write nothing.

use std::cell::{RefCell, RefMut};

use crate::core::contracts::{
    ActorPersistence, Clock, Deadline, DiskBackend, FlushReport, OwnedSnapshot, SaveAuthority,
    SaveBudget, SaveKey, SavePoll, SaveRequest, SaveScheduleReport, SaveTicket, ServerError,
    StoreHandle, SubmitSaveError,
};
use crate::core::state::AuthorityState;
use crate::store::scheduler::AutosaveScheduler;

const REENTERED: ServerError = ServerError::Internal {
    invariant: "shutdown store port reentered",
};

/// One autosave scheduler shared by the shutdown actor and store ports.
pub struct SharedScheduler<B: DiskBackend> {
    scheduler: RefCell<AutosaveScheduler<B>>,
}

impl<B: DiskBackend> SharedScheduler<B> {
    pub fn new(scheduler: AutosaveScheduler<B>) -> Self {
        Self {
            scheduler: RefCell::new(scheduler),
        }
    }

    /// Returns the scheduler once both ports are gone.
    pub fn into_inner(self) -> AutosaveScheduler<B> {
        self.scheduler.into_inner()
    }

    /// The per-family flush port; `clock` bounds its waits.
    pub fn actors<'a>(&'a self, clock: &'a dyn Clock) -> SchedulerActors<'a, B> {
        SchedulerActors {
            scheduler: &self.scheduler,
            clock,
        }
    }

    /// The store lifecycle port over the same scheduler.
    pub fn store(&self) -> SchedulerStore<'_, B> {
        SchedulerStore {
            scheduler: &self.scheduler,
        }
    }
}

fn borrow<B: DiskBackend>(
    scheduler: &RefCell<AutosaveScheduler<B>>,
) -> Result<RefMut<'_, AutosaveScheduler<B>>, ServerError> {
    scheduler.try_borrow_mut().map_err(|_| REENTERED)
}

/// Per-family flush port of a [`SharedScheduler`].
pub struct SchedulerActors<'a, B: DiskBackend> {
    scheduler: &'a RefCell<AutosaveScheduler<B>>,
    clock: &'a dyn Clock,
}

impl<B: DiskBackend> ActorPersistence for SchedulerActors<'_, B> {
    fn flush(&mut self, _family: SaveKey, _deadline: Deadline) -> Result<FlushReport, ServerError> {
        Err(ServerError::Internal {
            invariant: "actor flush requires the authority",
        })
    }

    fn flush_authority(
        &mut self,
        authority: &mut AuthorityState,
        family: SaveKey,
        deadline: Deadline,
    ) -> Result<FlushReport, ServerError> {
        borrow(self.scheduler)?.flush_family(deadline, authority, &family, self.clock)
    }
}

/// Store lifecycle port of a [`SharedScheduler`].
pub struct SchedulerStore<'a, B: DiskBackend> {
    scheduler: &'a RefCell<AutosaveScheduler<B>>,
}

impl<B: DiskBackend> StoreHandle for SchedulerStore<'_, B> {
    fn submit(&mut self, request: SaveRequest) -> Result<SaveTicket, SubmitSaveError> {
        match self.scheduler.try_borrow_mut() {
            Ok(mut scheduler) => scheduler.submit(request),
            Err(_) => Err(SubmitSaveError {
                error: REENTERED,
                request,
            }),
        }
    }

    fn poll(&mut self, ticket: SaveTicket) -> SavePoll {
        match self.scheduler.try_borrow_mut() {
            Ok(mut scheduler) => scheduler.poll(ticket),
            Err(_) => SavePoll::Pending,
        }
    }

    fn poll_tick(
        &mut self,
        tick: u64,
        budget: SaveBudget,
        authority: &mut dyn SaveAuthority,
    ) -> Result<SaveScheduleReport, ServerError> {
        borrow(self.scheduler)?.poll_tick(tick, budget, authority)
    }

    fn cancel_pending(&mut self) -> Result<Vec<OwnedSnapshot>, ServerError> {
        borrow(self.scheduler)?.cancel_pending()
    }

    fn flush(
        &mut self,
        deadline: Deadline,
        authority: &mut dyn SaveAuthority,
        clock: &dyn Clock,
    ) -> Result<FlushReport, ServerError> {
        borrow(self.scheduler)?.flush(deadline, authority, clock)
    }

    fn sync(&mut self, deadline: Deadline) -> Result<(), ServerError> {
        borrow(self.scheduler)?.sync(deadline)
    }

    fn close(&mut self, deadline: Deadline) -> Result<(), ServerError> {
        borrow(self.scheduler)?.close(deadline)
    }
}
