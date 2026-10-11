//! Authoritative memory adapters preserve terminal proposal ownership through ledger acceptance.
use super::*;
use crate::contracts::{SaveKey, SaveValue};
use crate::core::state::AuthorityState;

/// One terminal result with a whole reservation available for a later checked presentation effect.
#[derive(Clone, Debug, PartialEq)]
pub struct CommitSettlement {
    /// The remote operation's fenced acceptance result.
    pub outcome: CommitSettled,
    /// The complete proposal, supplied once after acceptance.
    pub fulfilled: Option<CommitReservation>,
}

impl MemoryOwner {
    // Public reservation fields remain mutable, so admission boundaries recheck their bounds.
    pub(super) fn valid_authority_proposal(proposal: &CommitReservation) -> bool {
        proposal.memory_epoch != 0
            && proposal.base_revision != u64::MAX
            && valid_memory_text(&proposal.summary, MAX_COMMIT_SUMMARY_BYTES)
            && valid_dialogue_line(&proposal.line)
    }

    pub(super) fn matches_authority_response(
        &self,
        leased: &LeasedIdentity,
        request: AgentRequestId,
        response_companion: CompanionId,
        companion: CompanionId,
    ) -> bool {
        leased.base.request_id == request
            && leased.base.client_instance_id == self.client
            && leased.base.namespace_id == self.namespace
            && response_companion == companion
    }

    /// Accepts complete terminal proposals through authority before releasing reservations.
    pub fn poll_commits_authoritative(
        &mut self,
        authority: &mut AuthorityState,
    ) -> Vec<CommitSettlement> {
        self.poll_commits_inner(Some(authority))
    }

    /// Converges against latest authoritative metadata before granting readiness or fulfillment.
    pub fn poll_reconciles_authoritative(
        &mut self,
        authority: &mut AuthorityState,
    ) -> Vec<ReconcileSettled> {
        self.poll_reconciles_inner(Some(authority))
    }

    /// Confirms deletion only after the exact inactive transition already exists in authority.
    pub fn poll_deletes_authoritative(
        &mut self,
        authority: &mut AuthorityState,
    ) -> Vec<DeleteSettled> {
        self.poll_deletes_inner(Some(authority))
    }

    /// Drains remote semantic work into latest authority; subsequent aggregate flush owns disk I/O.
    pub fn drain_authoritative(
        &mut self,
        authority: &mut AuthorityState,
        deadline: Deadline,
    ) -> Result<MemoryFinalizationReport, ServerError> {
        self.ensure_authority(authority)?;
        if self
            .reservations
            .values()
            .any(|proposal| !Self::valid_authority_proposal(proposal))
        {
            return Err(ServerError::InvalidInput {
                field: "memory_proposal",
            });
        }
        self.drain_inner(Some(authority), deadline)
    }

    /// Borrows this provider for the shutdown machine's explicit authority-aware boundary.
    pub fn authoritative_finalizer(&mut self) -> AuthoritativeMemoryFinalizer<'_> {
        AuthoritativeMemoryFinalizer { owner: self }
    }

    fn ensure_authority(&self, authority: &AuthorityState) -> Result<(), ServerError> {
        let invalid = ServerError::InvalidInput {
            field: "memory_authority",
        };
        if let Some(error) = authority.tick_failure() {
            return Err(error);
        }
        if authority.phase() == crate::contracts::ServerPhase::Closed {
            return Err(ServerError::InvalidState {
                phase: authority.phase(),
            });
        }
        if !authority.companion_persistence_enabled() {
            return Err(invalid);
        }
        let Some((_, SaveValue::Companions(save))) =
            authority.actor_save_current(&SaveKey::Companions)
        else {
            return Err(invalid);
        };
        if save.agent_namespace_id.to_bytes() != self.namespace.bytes() {
            return Err(ServerError::InvalidInput {
                field: "memory_namespace",
            });
        }
        Ok(())
    }

    fn authoritative_mirror(
        &self,
        authority: &AuthorityState,
        companion: CompanionId,
    ) -> Result<MemoryMirror, ServerError> {
        self.ensure_authority(authority)?;
        let invalid = ServerError::InvalidInput {
            field: "memory_authority",
        };
        let lifecycle = authority
            .companion_memory_lifecycle(companion)
            .ok_or(invalid)?;
        let optional = |bytes: [u8; 16]| {
            if bytes == [0; 16] {
                Ok(None)
            } else {
                OperationId::try_from_bytes(bytes)
                    .map(Some)
                    .map_err(|_| invalid)
            }
        };
        Ok(MemoryMirror {
            active: lifecycle.active,
            epoch: lifecycle.memory_epoch,
            revision: lifecycle.memory_revision,
            operation: optional(lifecycle.memory_operation_id.to_bytes())?,
            summary: lifecycle.summary.clone(),
            tombstone: optional(lifecycle.tombstone_operation_id.to_bytes())?,
        })
    }

    pub(super) fn accept_commit_authority(
        &mut self,
        authority: &mut AuthorityState,
        reservation: &CommitReservation,
        revision: u64,
    ) -> Result<(), ServerError> {
        self.authoritative_mirror(authority, reservation.companion)?;
        authority.replace_companion_memory(
            reservation.companion,
            reservation.memory_epoch,
            reservation.base_revision,
            revision,
            reservation.operation,
            reservation.summary.clone(),
        )?;
        let mirror = self.authoritative_mirror(authority, reservation.companion)?;
        self.mirrors.insert(reservation.companion, mirror);
        Ok(())
    }

    pub(super) fn apply_active_authority(
        &mut self,
        authority: &mut AuthorityState,
        companion: CompanionId,
        epoch: u64,
        memory: &MemoryState,
    ) -> ReconcileSettled {
        self.reconciles.remove(&companion);
        if self
            .reservations
            .get(&companion)
            .is_some_and(|proposal| !Self::valid_authority_proposal(proposal))
        {
            self.arm_retry(companion);
            return ReconcileSettled::NotReady { companion };
        }
        let mirror = match self.authoritative_mirror(authority, companion) {
            Ok(value) if value.active && value.epoch == epoch => value,
            _ => {
                self.arm_retry(companion);
                return ReconcileSettled::NotReady { companion };
            }
        };
        let confirmation = match memory {
            MemoryState::Absent => {
                if mirror.revision != 0 {
                    self.arm_retry(companion);
                    return ReconcileSettled::NotReady { companion };
                }
                None
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
                    self.arm_retry(companion);
                    return ReconcileSettled::NotReady { companion };
                }
                if revision > mirror.revision
                    && authority
                        .replace_companion_memory(
                            companion,
                            epoch,
                            mirror.revision,
                            revision,
                            *operation_id,
                            summary.clone(),
                        )
                        .is_err()
                {
                    self.arm_retry(companion);
                    return ReconcileSettled::NotReady { companion };
                }
                Some((revision, *operation_id, summary.as_str()))
            }
        };
        // Every fallible authority operation precedes readiness and semantic release.
        let mirror = match self.authoritative_mirror(authority, companion) {
            Ok(value) => value,
            Err(_) => {
                self.arm_retry(companion);
                return ReconcileSettled::NotReady { companion };
            }
        };
        self.mirrors.insert(companion, mirror);
        let fulfilled = confirmation.and_then(|(revision, operation, summary)| {
            self.resolve_reservation(companion, epoch, revision, operation, summary)
        });
        self.clear_retry(companion);
        ReconcileSettled::Ready {
            companion,
            fulfilled,
        }
    }

    pub(super) fn apply_inactive_authority(
        &mut self,
        authority: &AuthorityState,
        companion: CompanionId,
        epoch: u64,
        tombstone: OperationId,
    ) -> ReconcileSettled {
        self.reconciles.remove(&companion);
        let mirror = match self.authoritative_mirror(authority, companion) {
            Ok(value)
                if !value.active && value.epoch == epoch && value.tombstone == Some(tombstone) =>
            {
                value
            }
            _ => {
                self.arm_retry(companion);
                return ReconcileSettled::NotReady { companion };
            }
        };
        self.mirrors.insert(companion, mirror);
        self.clear_retry(companion);
        ReconcileSettled::Ready {
            companion,
            fulfilled: None,
        }
    }

    pub(super) fn confirm_delete_authority(
        &mut self,
        authority: &AuthorityState,
        companion: CompanionId,
        record: &DeleteInflight,
        response: &DeleteResponse,
    ) -> Result<(), ServerError> {
        if response.companion_id != companion
            || response.memory_epoch != record.new_epoch
            || response.tombstone_operation_id != record.tombstone
        {
            return Err(ServerError::InvalidInput {
                field: "memory_authority",
            });
        }
        let mirror = self.authoritative_mirror(authority, companion)?;
        if mirror.active
            || mirror.epoch != record.new_epoch
            || mirror.tombstone != Some(record.tombstone)
        {
            self.ready.insert(companion, false);
            return Err(ServerError::InvalidInput {
                field: "memory_authority",
            });
        }
        self.mirrors.insert(companion, mirror);
        Ok(())
    }
}

/// Shutdown borrower that requires the same mutable authority used by the phase machine.
pub struct AuthoritativeMemoryFinalizer<'a> {
    owner: &'a mut MemoryOwner,
}

impl MemoryFinalizer for AuthoritativeMemoryFinalizer<'_> {
    fn pending(&self) -> MemoryFinalizationReport {
        self.owner.pending()
    }
    fn begin_attempt(&mut self, deadline: Deadline) -> Result<(), ServerError> {
        self.owner.begin_attempt(deadline)
    }
    fn drain(&mut self, _deadline: Deadline) -> Result<MemoryFinalizationReport, ServerError> {
        Err(ServerError::InvalidInput {
            field: "memory_authority",
        })
    }
    fn drain_authority(
        &mut self,
        authority: &mut AuthorityState,
        deadline: Deadline,
    ) -> Result<MemoryFinalizationReport, ServerError> {
        self.owner.drain_authoritative(authority, deadline)
    }
}
