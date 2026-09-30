//! Sessionless companion candidate ingress.
//!
//! This module owns validation and bounded queue reservation for the
//! separately typed `CompanionActionEnvelope`. Every candidate is checked
//! field by field against the companion's frozen active-task gate
//! (companion, request, run, snapshot, digest, generation, attempt), then
//! against the authoritative source tick, then as a whole payload, and
//! only a fully valid candidate reserves an inbox slot and an arrival
//! index. The expected values mirror the Go authority: the bounded
//! non-blocking companion inbox with `companion.MaxActive` slots
//! (`packages/server/sim/runtime/companion_action.go` and
//! `action_inbox_test.go`), the per-field planning-identity comparison in
//! `validCompanionPlanningIdentity`
//! (`packages/server/server/companion_manager.go`), and the plan target
//! vertical bound in `validPlanBlockY`
//! (`packages/shared/companion/plan_types.go`).
//!
//! A refusal leaves the pending queue, the arrival counter, and every
//! other companion's gate untouched: there is no partial reservation, so
//! an invalid candidate can never clear another in-flight gate. The module
//! performs no world mutation, no tick work, and no I/O; revalidating the
//! current world, target, range, and resources at the tick boundary stays
//! with the action executor. The endpoint rewires its intake scaffold to
//! this provider at the common transport integration node.

use mornlea_domain::CompanionId;

use super::contracts::{
    AgentRequestId, CompanionAction, CompanionActionEnvelope, CompanionReceipt, Resource, RunId,
    ServerError, SnapshotId,
};

/// World vertical bound `[MIN_WORLD_Y, MAX_WORLD_Y)` from the domain
/// section rule; the domain keeps the constants private, so the ingress
/// restates the frozen values it validates against.
const MIN_WORLD_Y: i32 = -64;
const MAX_WORLD_Y: i32 = 320;

/// Pending-candidate ceiling for the whole ingress, mirroring the Go
/// `companion.MaxActive` inbox bound: the fifth pending candidate is
/// refused without consuming an arrival index.
pub const INBOX_CAPACITY: usize = 4;

mod seal {
    /// Imposter barrier for [`super::SessionlessCandidate`]: outside
    /// crates cannot name this trait, so the sessionless implementer set
    /// is closed here.
    pub trait Seal {}
}

/// Type-level proof that a candidate carries no human-session handle.
///
/// The only implementer is the sessionless `CompanionActionEnvelope`, so
/// an admission path typed over this trait can never accept a
/// `SessionKey`: a forged human session is unconstructible by type rather
/// than refused by a runtime check.
pub trait SessionlessCandidate: seal::Seal {}

impl SessionlessCandidate for CompanionActionEnvelope {}
impl seal::Seal for CompanionActionEnvelope {}

/// The frozen provenance a dispatch holds for one companion's active task.
///
/// The fields stay private so a gate can only be built through
/// [`CompanionTaskGate::try_new`], which refuses the empty-digest and
/// zero-generation/attempt shapes the Go identity check refuses. At
/// admission the ingress compares every candidate provenance field against
/// this record, the way the Go tick compares a worker result against the
/// identity it froze at dispatch.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CompanionTaskGate {
    companion_id: CompanionId,
    request_id: AgentRequestId,
    run_id: RunId,
    snapshot_id: SnapshotId,
    snapshot_digest: [u8; 32],
    generation: u64,
    attempt: u64,
}

impl CompanionTaskGate {
    /// Freezes one active-task gate.
    ///
    /// The digest must be nonzero and the generation and attempt positive:
    /// these are the same preconditions the Go
    /// `validCompanionPlanningIdentity` applies to the frozen identity.
    #[allow(clippy::too_many_arguments)]
    pub fn try_new(
        companion_id: CompanionId,
        request_id: AgentRequestId,
        run_id: RunId,
        snapshot_id: SnapshotId,
        snapshot_digest: [u8; 32],
        generation: u64,
        attempt: u64,
    ) -> Result<Self, ServerError> {
        if generation == 0 || attempt == 0 {
            return Err(ServerError::InvalidInput {
                field: "companion_generation",
            });
        }
        if snapshot_digest == [0; 32] {
            return Err(ServerError::InvalidInput {
                field: "snapshot_digest",
            });
        }
        Ok(Self {
            companion_id,
            request_id,
            run_id,
            snapshot_id,
            snapshot_digest,
            generation,
            attempt,
        })
    }

    pub fn companion_id(&self) -> CompanionId {
        self.companion_id
    }

    pub fn request_id(&self) -> AgentRequestId {
        self.request_id
    }

    pub fn run_id(&self) -> RunId {
        self.run_id
    }

    pub fn snapshot_id(&self) -> SnapshotId {
        self.snapshot_id
    }

    pub fn snapshot_digest(&self) -> [u8; 32] {
        self.snapshot_digest
    }

    pub fn generation(&self) -> u64 {
        self.generation
    }

    pub fn attempt(&self) -> u64 {
        self.attempt
    }
}

/// One pending candidate and the arrival index its receipt returned.
///
/// The index is kept beside the envelope so the tick-bound consumer can
/// order and correlate candidates with the receipts already handed out.
/// Arrival ordering across companions is first-come in the pending vector.
#[derive(Clone, Debug, PartialEq)]
pub struct QueuedCandidate {
    pub arrival_index: u64,
    pub envelope: CompanionActionEnvelope,
}

/// Bounded sessionless candidate inbox with a server-owned arrival index.
///
/// The queue holds at most [`INBOX_CAPACITY`] candidates across all
/// companions, exactly like the Go companion inbox. Admission validates
/// provenance, source tick, and the whole payload before reserving
/// anything, and a refusal reserves neither slot nor index.
pub struct CompanionIngress {
    pending: Vec<QueuedCandidate>,
    next_arrival: u64,
}

impl CompanionIngress {
    /// Reserves the inbox storage up front so admission cannot hit an
    /// allocation failure after validation.
    pub fn try_new() -> Result<Self, ServerError> {
        let mut pending = Vec::new();
        pending
            .try_reserve_exact(INBOX_CAPACITY)
            .map_err(|_| ServerError::Capacity {
                resource: Resource::Commands,
                limit: INBOX_CAPACITY,
                observed: INBOX_CAPACITY,
            })?;
        Ok(Self {
            pending,
            next_arrival: 0,
        })
    }

    /// Number of pending candidates currently holding inbox slots.
    pub fn pending(&self) -> usize {
        self.pending.len()
    }

    /// Admits one candidate against a companion's active-task gate.
    ///
    /// The gate must be the one frozen for this candidate's companion;
    /// every provenance field, the source tick, and the whole payload are
    /// validated before the capacity rule, so an over-capacity-but-invalid
    /// candidate reports its real defect. A refusal changes nothing.
    pub fn admit(
        &mut self,
        gate: &CompanionTaskGate,
        tick: u64,
        candidate: CompanionActionEnvelope,
    ) -> Result<CompanionReceipt, ServerError> {
        self.validate_provenance(gate, tick, &candidate)?;
        validate_payload(&candidate)?;
        self.refuse_duplicate(&candidate)?;
        if self.pending.len() >= INBOX_CAPACITY {
            return Err(ServerError::Capacity {
                resource: Resource::Commands,
                limit: INBOX_CAPACITY,
                observed: self.pending.len() + 1,
            });
        }
        let arrival_index = self.next_arrival;
        // The counter is monotonic and never reuses an index; exhaustion is
        // refused before any state changes.
        let next = arrival_index.checked_add(1).ok_or(ServerError::Capacity {
            resource: Resource::Arrivals,
            limit: usize::MAX,
            observed: usize::MAX,
        })?;
        self.next_arrival = next;
        self.pending.push(QueuedCandidate {
            arrival_index,
            envelope: candidate,
        });
        Ok(CompanionReceipt::new(tick, arrival_index))
    }

    /// Takes every pending candidate in arrival order.
    ///
    /// The tick-bound consumer drains the inbox; draining keeps the
    /// arrival counter monotonic so indices are never reused.
    pub fn drain(&mut self) -> Vec<QueuedCandidate> {
        self.pending.drain(..).collect()
    }

    /// Compares every provenance field against the frozen gate.
    ///
    /// This is the Rust mirror of the Go per-field planning-identity
    /// check: a single boolean match cannot replace the individual field
    /// refusals, and a candidate whose source tick names a tick after the
    /// current authority tick is refused as mis-correlated.
    fn validate_provenance(
        &self,
        gate: &CompanionTaskGate,
        tick: u64,
        candidate: &CompanionActionEnvelope,
    ) -> Result<(), ServerError> {
        if candidate.companion_id != gate.companion_id {
            return Err(ServerError::InvalidInput {
                field: "companion_id",
            });
        }
        if candidate.generation != gate.generation {
            return Err(ServerError::InvalidInput {
                field: "companion_generation",
            });
        }
        if candidate.attempt != gate.attempt {
            return Err(ServerError::InvalidInput {
                field: "companion_attempt",
            });
        }
        if candidate.request_id != gate.request_id {
            return Err(ServerError::InvalidInput {
                field: "agent_request_id",
            });
        }
        if candidate.run_id != gate.run_id {
            return Err(ServerError::InvalidInput { field: "run_id" });
        }
        if candidate.snapshot_id != gate.snapshot_id {
            return Err(ServerError::InvalidInput {
                field: "snapshot_id",
            });
        }
        if candidate.snapshot_digest != gate.snapshot_digest {
            return Err(ServerError::InvalidInput {
                field: "snapshot_digest",
            });
        }
        if candidate.source_tick > tick {
            return Err(ServerError::InvalidInput {
                field: "source_tick",
            });
        }
        Ok(())
    }

    /// Refuses a repeated private request/generation/attempt tuple while
    /// the original candidate is pending.
    ///
    /// The duplicate check is the tuple, not the request id alone: a new
    /// request for the same task generation is a fresh candidate, while a
    /// repeated tuple is the same candidate arriving twice. The scan is
    /// bounded by the pending inbox; after a drain, identity is enforced
    /// again at the tick boundary, so no unbounded admission history is
    /// kept here.
    fn refuse_duplicate(&self, candidate: &CompanionActionEnvelope) -> Result<(), ServerError> {
        if self.pending.iter().any(|queued| {
            let envelope = &queued.envelope;
            envelope.request_id == candidate.request_id
                && envelope.generation == candidate.generation
                && envelope.attempt == candidate.attempt
        }) {
            return Err(ServerError::InvalidInput {
                field: "companion_action",
            });
        }
        Ok(())
    }
}

/// Validates the whole candidate payload.
///
/// The envelope fields are public, so a raw struct literal can bypass the
/// frozen `CompanionActionEnvelope::try_new`; the same constructor is
/// rerun here so admission enforces its structural rules on every path,
/// and the mine/place target is bound to the world vertical range the Go
/// plan validator applies.
fn validate_payload(candidate: &CompanionActionEnvelope) -> Result<(), ServerError> {
    let _ = CompanionActionEnvelope::try_new(
        candidate.companion_id,
        candidate.source_tick,
        candidate.request_id,
        candidate.run_id,
        candidate.snapshot_id,
        candidate.generation,
        candidate.attempt,
        candidate.snapshot_digest,
        candidate.action.clone(),
    )?;
    let target = match candidate.action {
        CompanionAction::MineHold { target } => Some(target),
        CompanionAction::Place { target, .. } => Some(target),
        CompanionAction::Move { .. } | CompanionAction::MineRelease => None,
    };
    if let Some(target) = target
        && (target.y() < MIN_WORLD_Y || target.y() >= MAX_WORLD_Y)
    {
        return Err(ServerError::InvalidInput { field: "target" });
    }
    Ok(())
}
