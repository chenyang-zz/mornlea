//! Companion memory ownership: reconcile, commit, delete, and shutdown
//! finalization over the loopback Agent boundary.
//!
//! The owner consumes the accepted wire and lease provider through
//! [`AgentHandle`]. Reconcile runs on lease acquire and reacquire; a commit
//! must echo the epoch and operation with `revision == base + 1`, and an
//! unknown commit retries the same operation instead of opening a new
//! dialogue. Delete advances the epoch by exactly one and bars the old epoch
//! from resurrection. One companion's reconcile failure never stops the
//! later companions, and retries back off `1, 2, 4, 8, 16, 32` ticks capped.
//! Shutdown finalization uses a fresh context of at most 30 seconds per
//! attempt and retains every unresolved operation identity.

use std::collections::{BTreeMap, BTreeSet};
use std::num::NonZeroU64;
use std::sync::Arc;
use std::time::Duration;

use mornlea_domain::CompanionId;

use crate::contracts::{
    AgentHandle, AgentRequest, AgentRequestId, BaseIdentity, ClientInstanceId, Clock,
    CommitRequest, CommitResponse, Deadline, DeleteRequest, DeleteResponse, LeaseId,
    LeasedIdentity, MemoryFinalizationReport, MemoryFinalizer, MemoryState, NamespaceId, Operation,
    OperationId, ReconcileRequest, ReconcileResponse, Resource, ServerError,
};

/// Reconcile retry attempts before the wait saturates.
pub const RECONCILE_MAX_ATTEMPTS: u32 = 6;

/// Fresh memory-finalization context per shutdown attempt.
pub const FINALIZE_CONTEXT_TIMEOUT: Duration = Duration::from_secs(30);

/// Terminal proposal summary ceiling in bytes, matching the host.
pub const MAX_COMMIT_SUMMARY_BYTES: usize = 2048;

/// Dialogue line ceiling in bytes, matching the host.
pub const MAX_COMMIT_LINE_BYTES: usize = 256;

/// Reconcile wait in ticks after `attempts` consecutive failures: `1, 2, 4,
/// 8, 16, 32` capped. Zero attempts means no wait is armed.
pub fn reconcile_wait_ticks(attempts: u32) -> u64 {
    match attempts {
        0 => 0,
        1 => 1,
        2 => 2,
        3 => 4,
        4 => 8,
        5 => 16,
        _ => 32,
    }
}

/// Reports whether text fits the memory wire shape: byte-bounded with no
/// NUL, mirroring the accepted `memory_summary` and `persona_text` rules.
pub(crate) fn valid_memory_text(value: &str, maximum: usize) -> bool {
    value.len() <= maximum && !value.contains('\u{0000}')
}

/// Reports whether a dialogue line fits the wire shape: bounded nonempty
/// text with no NUL, no controls, and no edge whitespace, mirroring the
/// accepted `dialogue_line` codec rule.
pub(crate) fn valid_dialogue_line(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= MAX_COMMIT_LINE_BYTES
        && !value.contains('\u{0000}')
        && !value.chars().any(char::is_control)
        && value
            .chars()
            .next()
            .is_some_and(|first| !first.is_whitespace())
        && value
            .chars()
            .next_back()
            .is_some_and(|last| !last.is_whitespace())
}

/// Terminal dialogue reservation handed from the host to the memory owner.
///
/// The operation and base revision are reserved by the terminal proposal;
/// the mirror is never updated directly, only through a fenced commit.
#[derive(Clone, Debug, PartialEq)]
pub struct CommitReservation {
    /// Companion the memory belongs to.
    pub companion: CompanionId,
    /// Epoch the reservation was composed under.
    pub memory_epoch: u64,
    /// Reserved operation identity.
    pub operation: OperationId,
    /// Revision the commit must advance from.
    pub base_revision: u64,
    /// Proposed summary text.
    pub summary: String,
    /// Dialogue line the commit carries.
    pub line: String,
}

impl CommitReservation {
    /// Freezes one reservation; the summary and line stay within their wire
    /// shapes and the base revision must leave room for `base + 1`.
    pub fn try_new(
        companion: CompanionId,
        memory_epoch: u64,
        operation: OperationId,
        base_revision: u64,
        summary: String,
        line: String,
    ) -> Result<Self, ServerError> {
        if !valid_memory_text(&summary, MAX_COMMIT_SUMMARY_BYTES) || !valid_dialogue_line(&line) {
            return Err(ServerError::InvalidInput {
                field: "memory_proposal",
            });
        }
        if base_revision == u64::MAX {
            return Err(ServerError::InvalidInput {
                field: "base_revision",
            });
        }
        Ok(Self {
            companion,
            memory_epoch,
            operation,
            base_revision,
            summary,
            line,
        })
    }
}

/// Local memory mirror for one companion: the active record or the delete
/// tombstone, exactly like the durable lifecycle the tick side owns.
#[derive(Clone, Debug, PartialEq)]
pub struct MemoryMirror {
    /// Whether the mirror holds live memory rather than a tombstone.
    pub active: bool,
    /// Epoch of the mirror.
    pub epoch: u64,
    /// Committed revision, zero when memory is absent.
    pub revision: u64,
    /// Operation that wrote the revision, if any.
    pub operation: Option<OperationId>,
    /// Committed summary text.
    pub summary: String,
    /// Tombstone operation for an inactive mirror, if any.
    pub tombstone: Option<OperationId>,
}

impl MemoryMirror {
    /// Absent memory at the zero epoch.
    pub fn absent() -> Self {
        Self {
            active: true,
            epoch: 0,
            revision: 0,
            operation: None,
            summary: String::new(),
            tombstone: None,
        }
    }

    /// Renders the mirror as the reconcile wire state.
    pub fn state(&self) -> MemoryState {
        if !self.active || self.revision == 0 {
            return MemoryState::Absent;
        }
        let Some(operation) = self.operation else {
            return MemoryState::Absent;
        };
        let Some(revision) = NonZeroU64::new(self.revision) else {
            return MemoryState::Absent;
        };
        MemoryState::Present {
            revision,
            operation_id: operation,
            summary: self.summary.clone(),
        }
    }
}

/// Settled commit outcome.
#[derive(Clone, Debug, PartialEq)]
pub enum CommitSettled {
    /// Commit applied with the committed revision.
    Applied {
        /// Companion the commit belonged to.
        companion: CompanionId,
        /// Committed revision.
        revision: u64,
    },
    /// Response identities did not match the reservation; it is kept.
    Fenced {
        /// Companion the commit belonged to.
        companion: CompanionId,
    },
    /// Commit RPC failed; the same operation stays reserved for retry.
    Failed {
        /// Companion the commit belonged to.
        companion: CompanionId,
        /// Wire failure that keeps the reservation.
        error: ServerError,
    },
}

/// Settled reconcile outcome for one companion.
#[derive(Clone, Debug, PartialEq)]
pub enum ReconcileSettled {
    /// Mirror converged with the service; an optional fulfilled reservation
    /// carries the dialogue line the caller may now present.
    Ready {
        /// Companion the reconcile belonged to.
        companion: CompanionId,
        /// Reservation the remote state confirmed, if any.
        fulfilled: Option<CommitReservation>,
    },
    /// Mirror diverged or the RPC failed; the companion retries on backoff
    /// while later companions proceed.
    NotReady {
        /// Companion the reconcile belonged to.
        companion: CompanionId,
    },
    /// Response epoch did not match the request; nothing changed.
    Fenced {
        /// Companion the reconcile belonged to.
        companion: CompanionId,
    },
}

/// Settled delete outcome.
#[derive(Clone, Debug, PartialEq)]
pub enum DeleteSettled {
    /// Tombstone installed with the advanced epoch.
    Deleted {
        /// Companion the delete belonged to.
        companion: CompanionId,
        /// Epoch after the exact `+1` advance.
        epoch: u64,
    },
    /// Response identities did not match; the mirror is kept.
    Fenced {
        /// Companion the delete belonged to.
        companion: CompanionId,
    },
    /// Delete RPC failed; the mirror is kept.
    Failed {
        /// Companion the delete belonged to.
        companion: CompanionId,
        /// Wire failure that keeps the mirror.
        error: ServerError,
    },
}

/// Reconcile admission outcome: an armed backoff waits instead of erroring.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ReconcileAdmit {
    /// Request admitted.
    Admitted,
    /// Retry wait still armed; no request was sent.
    Waiting {
        /// Ticks still armed before the next attempt.
        ticks: u64,
    },
}

/// Per-companion reconcile retry state.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct ReconcileRetry {
    attempts: u32,
    wait_ticks: u64,
}

/// Admitted reconcile record for response binding.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct ReconcileInflight {
    request_id: AgentRequestId,
    epoch: u64,
}

/// Admitted commit record for response binding.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct CommitInflight {
    request_id: AgentRequestId,
}

/// Admitted delete record for response binding.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct DeleteInflight {
    request_id: AgentRequestId,
    new_epoch: u64,
    tombstone: OperationId,
}

/// A delete's semantic identity survives retirement of its HTTP attempt.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct DeleteIntent {
    new_epoch: u64,
    tombstone: OperationId,
}

/// Companion memory owner: mirrors, reservations, admitted RPCs, retry
/// backoff, and shutdown finalization for every companion.
pub struct MemoryOwner {
    agent: Box<dyn AgentHandle>,
    clock: Arc<dyn Clock + Send + Sync>,
    client: ClientInstanceId,
    namespace: NamespaceId,
    lease: LeaseId,
    mirrors: BTreeMap<CompanionId, MemoryMirror>,
    reservations: BTreeMap<CompanionId, CommitReservation>,
    commits: BTreeMap<CompanionId, CommitInflight>,
    reconciles: BTreeMap<CompanionId, ReconcileInflight>,
    deletes: BTreeMap<CompanionId, DeleteInflight>,
    delete_intents: BTreeMap<CompanionId, DeleteIntent>,
    pending_retirements: BTreeSet<AgentRequestId>,
    retries: BTreeMap<CompanionId, ReconcileRetry>,
    ready: BTreeMap<CompanionId, bool>,
    attempt: u64,
    attempt_deadline: Option<Deadline>,
}

impl MemoryOwner {
    /// Creates an owner over the Agent provider with the current lease.
    pub fn new(
        agent: Box<dyn AgentHandle>,
        clock: Arc<dyn Clock + Send + Sync>,
        client: ClientInstanceId,
        namespace: NamespaceId,
        lease: LeaseId,
    ) -> Self {
        Self {
            agent,
            clock,
            client,
            namespace,
            lease,
            mirrors: BTreeMap::new(),
            reservations: BTreeMap::new(),
            commits: BTreeMap::new(),
            reconciles: BTreeMap::new(),
            deletes: BTreeMap::new(),
            delete_intents: BTreeMap::new(),
            pending_retirements: BTreeSet::new(),
            retries: BTreeMap::new(),
            ready: BTreeMap::new(),
            attempt: 0,
            attempt_deadline: None,
        }
    }

    /// Failed reclamation remains charged without replaying its settlement.
    fn reap_retirements(&mut self) {
        let deadline = Deadline::at(self.clock.monotonic());
        self.pending_retirements
            .retain(|id| self.agent.cancel(*id, deadline).is_err());
    }

    fn retire_request(&mut self, id: AgentRequestId) {
        if self
            .agent
            .cancel(id, Deadline::at(self.clock.monotonic()))
            .is_err()
        {
            self.pending_retirements.insert(id);
        }
    }

    fn check_business_capacity(&self) -> Result<(), ServerError> {
        let owned = self.commits.len()
            + self.reconciles.len()
            + self.deletes.len()
            + self.pending_retirements.len();
        if owned >= 64 {
            return Err(ServerError::Capacity {
                resource: Resource::AgentRuns,
                limit: 64,
                observed: 65,
            });
        }
        Ok(())
    }

    /// Replaces the lease after acquire or reacquire; in-flight RPCs keep
    /// their frozen request lease and bind on their own identities.
    pub fn set_lease(&mut self, lease: LeaseId) {
        self.lease = lease;
    }

    /// Current shutdown attempt index, zero before the first attempt.
    pub fn attempt(&self) -> u64 {
        self.attempt
    }

    /// Fresh context deadline of the current shutdown attempt, if any.
    pub fn attempt_deadline(&self) -> Option<Deadline> {
        self.attempt_deadline
    }

    /// Working mirror for the companion, if seeded.
    pub fn mirror(&self, companion: CompanionId) -> Option<&MemoryMirror> {
        self.mirrors.get(&companion)
    }

    /// Seeds the working mirror from the tick-side durable lifecycle.
    pub fn set_mirror(&mut self, companion: CompanionId, mirror: MemoryMirror) {
        self.mirrors.insert(companion, mirror);
    }

    /// Pending terminal reservation for the companion, if any.
    pub fn reservation(&self, companion: CompanionId) -> Option<&CommitReservation> {
        self.reservations.get(&companion)
    }

    /// Whether the companion converged with the service.
    pub fn is_ready(&self, companion: CompanionId) -> bool {
        self.ready.get(&companion).copied().unwrap_or(false)
    }

    /// Retry attempts recorded for the companion.
    pub fn retry_attempts(&self, companion: CompanionId) -> u32 {
        self.retries
            .get(&companion)
            .copied()
            .unwrap_or_default()
            .attempts
    }

    /// Retry wait in ticks still armed for the companion.
    pub fn retry_wait(&self, companion: CompanionId) -> u64 {
        self.retries
            .get(&companion)
            .copied()
            .unwrap_or_default()
            .wait_ticks
    }

    /// Advances one tick: armed reconcile waits count down toward dispatch.
    pub fn tick(&mut self) {
        for retry in self.retries.values_mut() {
            retry.wait_ticks = retry.wait_ticks.saturating_sub(1);
        }
    }

    /// Reserves one terminal operation; a second reservation for the same
    /// companion is refused without touching the first.
    pub fn reserve(&mut self, reservation: CommitReservation) -> Result<(), ServerError> {
        if self.reservations.contains_key(&reservation.companion) {
            return Err(ServerError::InvalidInput {
                field: "memory_reservation",
            });
        }
        self.reservations.insert(reservation.companion, reservation);
        Ok(())
    }

    /// Submits the reserved commit; the request repeats the same operation,
    /// base revision, epoch, and summary on every retry, never a new one.
    pub fn commit(
        &mut self,
        companion: CompanionId,
        request_id: AgentRequestId,
    ) -> Result<(), ServerError> {
        self.reap_retirements();
        let mirror = self
            .mirrors
            .get(&companion)
            .ok_or(ServerError::InvalidInput {
                field: "memory_mirror",
            })?;
        if !mirror.active {
            return Err(ServerError::InvalidInput {
                field: "memory_mirror",
            });
        }
        let reservation = self
            .reservations
            .get(&companion)
            .ok_or(ServerError::InvalidInput {
                field: "memory_reservation",
            })?;
        if self.commits.contains_key(&companion) {
            return Err(ServerError::InvalidInput {
                field: "memory_commit",
            });
        }
        let request = commit_request_for(self.leased(request_id), companion, reservation);
        self.check_business_capacity()?;
        self.agent.submit(AgentRequest::Commit(request))?;
        self.commits
            .insert(companion, CommitInflight { request_id });
        Ok(())
    }

    /// Polls admitted commits once and applies fenced outcomes.
    pub fn poll_commits(&mut self) -> Vec<CommitSettled> {
        self.reap_retirements();
        let pending: Vec<(CompanionId, CommitInflight)> = self
            .commits
            .iter()
            .map(|(companion, record)| (*companion, *record))
            .collect();
        let mut settled = Vec::new();
        for (companion, record) in pending {
            let poll = self.agent.poll(record.request_id);
            let terminal = !matches!(poll, crate::contracts::AgentPoll::Pending);
            if terminal {
                self.commits.remove(&companion);
            }
            match poll {
                crate::contracts::AgentPoll::Pending => {}
                crate::contracts::AgentPoll::Completed(
                    crate::contracts::AgentResponse::Commit(response),
                ) => {
                    settled.push(self.apply_commit_response(companion, &response));
                }
                crate::contracts::AgentPoll::Completed(_) => {
                    self.commits.remove(&companion);
                    settled.push(CommitSettled::Failed {
                        companion,
                        error: unavailable(),
                    });
                }
                crate::contracts::AgentPoll::Failed(error) => {
                    self.commits.remove(&companion);
                    settled.push(CommitSettled::Failed { companion, error });
                }
            }
            if terminal {
                self.retire_request(record.request_id);
            }
        }
        settled
    }

    /// Submits one reconcile carrying the local mirror, active or tombstone,
    /// unless the retry wait is still armed.
    pub fn reconcile(
        &mut self,
        companion: CompanionId,
        request_id: AgentRequestId,
    ) -> Result<ReconcileAdmit, ServerError> {
        self.reap_retirements();
        if self.retry_wait(companion) > 0 {
            return Ok(ReconcileAdmit::Waiting {
                ticks: self.retry_wait(companion),
            });
        }
        let mirror = self
            .mirrors
            .get(&companion)
            .ok_or(ServerError::InvalidInput {
                field: "memory_mirror",
            })?;
        if self.reconciles.contains_key(&companion) {
            return Err(ServerError::InvalidInput {
                field: "memory_reconcile",
            });
        }
        let request = reconcile_request_for(self.leased(request_id), companion, mirror)?;
        let epoch = mirror.epoch;
        self.check_business_capacity()?;
        self.agent
            .submit(AgentRequest::Reconcile(request))
            .map_err(|_| unavailable())?;
        self.reconciles
            .insert(companion, ReconcileInflight { request_id, epoch });
        Ok(ReconcileAdmit::Admitted)
    }

    /// Polls admitted reconciles once; one companion's failure never stops
    /// the later companions and rearms its own backoff.
    pub fn poll_reconciles(&mut self) -> Vec<ReconcileSettled> {
        self.reap_retirements();
        let pending: Vec<(CompanionId, ReconcileInflight)> = self
            .reconciles
            .iter()
            .map(|(companion, record)| (*companion, *record))
            .collect();
        let mut settled = Vec::new();
        for (companion, record) in pending {
            let poll = self.agent.poll(record.request_id);
            let terminal = !matches!(poll, crate::contracts::AgentPoll::Pending);
            if terminal {
                self.reconciles.remove(&companion);
            }
            match poll {
                crate::contracts::AgentPoll::Pending => {}
                crate::contracts::AgentPoll::Completed(
                    crate::contracts::AgentResponse::Reconcile(response),
                ) => {
                    settled.push(self.apply_reconcile_response(companion, record.epoch, &response));
                }
                crate::contracts::AgentPoll::Completed(_) => {
                    self.reconciles.remove(&companion);
                    self.arm_retry(companion);
                    settled.push(ReconcileSettled::NotReady { companion });
                }
                crate::contracts::AgentPoll::Failed(_) => {
                    self.reconciles.remove(&companion);
                    self.arm_retry(companion);
                    settled.push(ReconcileSettled::NotReady { companion });
                }
            }
            if terminal {
                self.retire_request(record.request_id);
            }
        }
        settled
    }

    /// Submits one delete advancing the epoch by exactly one toward the
    /// tombstone.
    pub fn delete(
        &mut self,
        companion: CompanionId,
        request_id: AgentRequestId,
        tombstone: OperationId,
    ) -> Result<(), ServerError> {
        self.reap_retirements();
        let mirror = self
            .mirrors
            .get(&companion)
            .ok_or(ServerError::InvalidInput {
                field: "memory_mirror",
            })?;
        if !mirror.active {
            return Err(ServerError::InvalidInput {
                field: "memory_mirror",
            });
        }
        if self.deletes.contains_key(&companion) || self.commits.contains_key(&companion) {
            return Err(ServerError::InvalidInput {
                field: "memory_delete",
            });
        }
        let old_epoch = mirror.epoch;
        let new_epoch = old_epoch.checked_add(1).ok_or(ServerError::InvalidInput {
            field: "memory_epoch",
        })?;
        let request = delete_request_for(
            self.leased(request_id),
            companion,
            old_epoch,
            new_epoch,
            tombstone,
        );
        self.check_business_capacity()?;
        let intent = DeleteIntent {
            new_epoch,
            tombstone,
        };
        // A retry keeps the original tombstone identity after HTTP retirement.
        if let Some(retained) = self.delete_intents.get(&companion) {
            if *retained != intent {
                return Err(ServerError::InvalidInput {
                    field: "memory delete intent",
                });
            }
        } else if self.delete_intents.len() >= 64 {
            return Err(ServerError::Capacity {
                resource: Resource::AgentRuns,
                limit: 64,
                observed: 65,
            });
        }
        self.agent
            .submit(AgentRequest::Delete(request))
            .map_err(|_| unavailable())?;
        self.delete_intents.insert(companion, intent);
        self.deletes.insert(
            companion,
            DeleteInflight {
                request_id,
                new_epoch,
                tombstone,
            },
        );
        Ok(())
    }

    /// Polls admitted deletes once and installs matching tombstones.
    pub fn poll_deletes(&mut self) -> Vec<DeleteSettled> {
        self.reap_retirements();
        let pending: Vec<(CompanionId, DeleteInflight)> = self
            .deletes
            .iter()
            .map(|(companion, record)| (*companion, *record))
            .collect();
        let mut settled = Vec::new();
        for (companion, record) in pending {
            let poll = self.agent.poll(record.request_id);
            let terminal = !matches!(poll, crate::contracts::AgentPoll::Pending);
            if terminal {
                self.deletes.remove(&companion);
            }
            match poll {
                crate::contracts::AgentPoll::Pending => {}
                crate::contracts::AgentPoll::Completed(
                    crate::contracts::AgentResponse::Delete(response),
                ) => {
                    settled.push(self.apply_delete_response(&record, companion, &response));
                }
                crate::contracts::AgentPoll::Completed(_) => {
                    self.deletes.remove(&companion);
                    settled.push(DeleteSettled::Failed {
                        companion,
                        error: unavailable(),
                    });
                }
                crate::contracts::AgentPoll::Failed(error) => {
                    self.deletes.remove(&companion);
                    settled.push(DeleteSettled::Failed { companion, error });
                }
            }
            if terminal {
                self.retire_request(record.request_id);
            }
        }
        settled
    }

    /// Builds the leased identity every memory RPC names.
    fn leased(&self, request_id: AgentRequestId) -> LeasedIdentity {
        LeasedIdentity {
            base: BaseIdentity {
                request_id,
                client_instance_id: self.client,
                namespace_id: self.namespace,
            },
            lease_id: self.lease,
        }
    }

    /// Arms one more retry wait for the companion, capped at six attempts.
    fn arm_retry(&mut self, companion: CompanionId) {
        let retry = self.retries.entry(companion).or_default();
        retry.attempts = retry.attempts.saturating_add(1).min(RECONCILE_MAX_ATTEMPTS);
        retry.wait_ticks = reconcile_wait_ticks(retry.attempts);
        self.ready.insert(companion, false);
    }

    /// Clears the retry state after convergence.
    fn clear_retry(&mut self, companion: CompanionId) {
        self.retries.remove(&companion);
        self.ready.insert(companion, true);
    }

    /// Resolves a pending reservation against confirmed remote state: an
    /// exact epoch, revision, operation, and summary match fulfills it.
    fn resolve_reservation(
        &mut self,
        companion: CompanionId,
        epoch: u64,
        revision: u64,
        operation: OperationId,
        summary: &str,
    ) -> Option<CommitReservation> {
        let reservation = self.reservations.get(&companion)?;
        if reservation.memory_epoch != epoch
            || reservation.base_revision.checked_add(1)? != revision
            || reservation.operation != operation
            || reservation.summary != summary
        {
            return None;
        }
        self.reservations.remove(&companion)
    }

    /// Applies one commit response against the reservation fence: exact
    /// epoch, operation, and `revision == base + 1` apply, anything else
    /// keeps the reservation for the same-operation retry.
    fn apply_commit_response(
        &mut self,
        companion: CompanionId,
        response: &CommitResponse,
    ) -> CommitSettled {
        let Some(reservation) = self.reservations.get(&companion).cloned() else {
            self.commits.remove(&companion);
            return CommitSettled::Fenced { companion };
        };
        if response.memory_epoch != reservation.memory_epoch
            || response.operation_id != reservation.operation
            || response.committed_revision.get() != reservation.base_revision + 1
        {
            self.commits.remove(&companion);
            return CommitSettled::Fenced { companion };
        }
        let revision = response.committed_revision.get();
        if let Some(mirror) = self.mirrors.get_mut(&companion) {
            mirror.revision = revision;
            mirror.operation = Some(reservation.operation);
            mirror.summary = reservation.summary.clone();
        }
        self.commits.remove(&companion);
        self.reservations.remove(&companion);
        CommitSettled::Applied {
            companion,
            revision,
        }
    }

    /// Applies one reconcile response against the admitted epoch and the
    /// local mirror, converging, diverging, or fencing per companion.
    fn apply_reconcile_response(
        &mut self,
        companion: CompanionId,
        requested_epoch: u64,
        response: &ReconcileResponse,
    ) -> ReconcileSettled {
        match response {
            ReconcileResponse::Active {
                memory_epoch,
                memory,
                ..
            } => {
                if *memory_epoch != requested_epoch {
                    self.reconciles.remove(&companion);
                    return ReconcileSettled::Fenced { companion };
                }
                self.apply_active_reconcile(companion, *memory_epoch, memory)
            }
            ReconcileResponse::Inactive {
                memory_epoch,
                tombstone_operation_id,
                ..
            } => {
                if *memory_epoch != requested_epoch {
                    self.reconciles.remove(&companion);
                    return ReconcileSettled::Fenced { companion };
                }
                let mirror = self.mirrors.get(&companion).cloned();
                let Some(mirror) = mirror else {
                    self.reconciles.remove(&companion);
                    return ReconcileSettled::Fenced { companion };
                };
                if mirror.active || mirror.epoch != *memory_epoch {
                    self.reconciles.remove(&companion);
                    self.arm_retry(companion);
                    return ReconcileSettled::NotReady { companion };
                }
                self.reconciles.remove(&companion);
                if mirror.tombstone != Some(*tombstone_operation_id) {
                    self.arm_retry(companion);
                    return ReconcileSettled::NotReady { companion };
                }
                self.clear_retry(companion);
                ReconcileSettled::Ready {
                    companion,
                    fulfilled: None,
                }
            }
        }
    }

    /// Applies an active reconcile response against the local mirror.
    fn apply_active_reconcile(
        &mut self,
        companion: CompanionId,
        epoch: u64,
        memory: &MemoryState,
    ) -> ReconcileSettled {
        let mirror = self.mirrors.get(&companion).cloned();
        let Some(mirror) = mirror else {
            self.reconciles.remove(&companion);
            return ReconcileSettled::Fenced { companion };
        };
        if !mirror.active || mirror.epoch != epoch {
            self.reconciles.remove(&companion);
            self.arm_retry(companion);
            return ReconcileSettled::NotReady { companion };
        }
        match memory {
            MemoryState::Absent => {
                self.reconciles.remove(&companion);
                if mirror.revision != 0 {
                    self.arm_retry(companion);
                    return ReconcileSettled::NotReady { companion };
                }
                self.clear_retry(companion);
                ReconcileSettled::Ready {
                    companion,
                    fulfilled: None,
                }
            }
            MemoryState::Present {
                revision,
                operation_id,
                summary,
            } => {
                let revision = revision.get();
                if revision < mirror.revision
                    || (revision == mirror.revision
                        && (mirror.operation != Some(*operation_id) || mirror.summary != *summary))
                {
                    self.reconciles.remove(&companion);
                    self.arm_retry(companion);
                    return ReconcileSettled::NotReady { companion };
                }
                if let Some(live) = self.mirrors.get_mut(&companion) {
                    live.revision = revision;
                    live.operation = Some(*operation_id);
                    live.summary = summary.clone();
                }
                let fulfilled =
                    self.resolve_reservation(companion, epoch, revision, *operation_id, summary);
                self.reconciles.remove(&companion);
                self.clear_retry(companion);
                ReconcileSettled::Ready {
                    companion,
                    fulfilled,
                }
            }
        }
    }

    /// Applies one delete response against the admitted tombstone: exact new
    /// epoch and tombstone install, anything else keeps the mirror.
    fn apply_delete_response(
        &mut self,
        record: &DeleteInflight,
        companion: CompanionId,
        response: &DeleteResponse,
    ) -> DeleteSettled {
        if response.memory_epoch != record.new_epoch
            || response.tombstone_operation_id != record.tombstone
            || response.companion_id != companion
        {
            return DeleteSettled::Fenced { companion };
        }
        if let Some(mirror) = self.mirrors.get_mut(&companion) {
            mirror.active = false;
            mirror.epoch = record.new_epoch;
            mirror.tombstone = Some(record.tombstone);
        }
        self.deletes.remove(&companion);
        self.delete_intents.remove(&companion);
        self.reservations.remove(&companion);
        DeleteSettled::Deleted {
            companion,
            epoch: record.new_epoch,
        }
    }
}

impl MemoryFinalizer for MemoryOwner {
    fn pending(&self) -> MemoryFinalizationReport {
        let companions: BTreeSet<_> = self
            .reservations
            .keys()
            .chain(self.delete_intents.keys())
            .chain(self.commits.keys())
            .chain(self.reconciles.keys())
            .chain(self.deletes.keys())
            .copied()
            .collect();
        MemoryFinalizationReport {
            completed: 0,
            outstanding: companions.len() + self.pending_retirements.len(),
        }
    }

    /// Cancels the previous attempt and opens a fresh context of at most 30
    /// seconds; unresolved operation identities are retained.
    fn begin_attempt(&mut self, deadline: Deadline) -> Result<(), ServerError> {
        let in_flight: Vec<AgentRequestId> = self
            .commits
            .values()
            .map(|record| record.request_id)
            .chain(self.reconciles.values().map(|record| record.request_id))
            .chain(self.deletes.values().map(|record| record.request_id))
            .collect();
        for id in in_flight {
            let _ = self.agent.cancel(id, deadline);
        }
        self.attempt = self.attempt.saturating_add(1);
        let fresh =
            Deadline::after(self.clock.monotonic(), FINALIZE_CONTEXT_TIMEOUT).unwrap_or(deadline);
        self.attempt_deadline = Some(if fresh.instant() < deadline.instant() {
            fresh
        } else {
            deadline
        });
        Ok(())
    }

    /// Polls admitted memory RPCs once without blocking; a nonzero
    /// unresolved count is a typed shutdown failure. Callers repeat drain
    /// until it succeeds or the caller deadline passes.
    fn drain(&mut self, _deadline: Deadline) -> Result<MemoryFinalizationReport, ServerError> {
        let mut completed = 0usize;
        completed += self.poll_commits().len();
        completed += self.poll_reconciles().len();
        completed += self.poll_deletes().len();
        let outstanding = self.commits.len()
            + self.reconciles.len()
            + self.deletes.len()
            + self.reservations.len();
        if outstanding > 0 {
            return Err(ServerError::Timeout {
                operation: Operation::Shutdown,
            });
        }
        Ok(MemoryFinalizationReport {
            completed,
            outstanding: 0,
        })
    }
}

/// Builds one commit request from the reservation; kept beside the owner so
/// retries resubmit the identical operation, base revision, epoch, and
/// summary.
fn commit_request_for(
    leased: LeasedIdentity,
    companion: CompanionId,
    reservation: &CommitReservation,
) -> CommitRequest {
    CommitRequest {
        leased,
        companion_id: companion,
        memory_epoch: reservation.memory_epoch,
        base_revision: reservation.base_revision,
        operation_id: reservation.operation,
        summary: reservation.summary.clone(),
    }
}

/// Builds one delete request advancing exactly one epoch toward the
/// tombstone.
fn delete_request_for(
    leased: LeasedIdentity,
    companion: CompanionId,
    old_epoch: u64,
    new_epoch: u64,
    tombstone: OperationId,
) -> DeleteRequest {
    DeleteRequest {
        leased,
        companion_id: companion,
        old_memory_epoch: old_epoch,
        new_memory_epoch: new_epoch,
        tombstone_operation_id: tombstone,
    }
}

/// Builds one reconcile request from the local mirror; an inactive mirror
/// without a tombstone is refused before any side effect.
fn reconcile_request_for(
    leased: LeasedIdentity,
    companion: CompanionId,
    mirror: &MemoryMirror,
) -> Result<ReconcileRequest, ServerError> {
    if mirror.active {
        Ok(ReconcileRequest::Active {
            leased,
            companion_id: companion,
            memory_epoch: mirror.epoch,
            mirror: mirror.state(),
        })
    } else {
        let Some(tombstone) = mirror.tombstone else {
            return Err(ServerError::InvalidInput {
                field: "memory_tombstone",
            });
        };
        Ok(ReconcileRequest::Inactive {
            leased,
            companion_id: companion,
            memory_epoch: mirror.epoch,
            tombstone_operation_id: tombstone,
        })
    }
}

fn unavailable() -> ServerError {
    ServerError::Agent {
        code: crate::contracts::AgentErrorCode::AgentUnavailable,
        status: 503,
    }
}
