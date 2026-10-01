//! Prepared live acquisition and legacy fixture identity checks.
//!
//! The managed entry transfers prepared owners at the actual acquisition row.
//! The legacy fixture helpers below retain their identity-only ledger and
//! completion gate. The managed source behavior is:
//!
//! - `packages/server/sim/runtime/engine_subscription.go` (`applyAcquired`,
//!   `applyGenerated`): an expected successful body installs even after its
//!   key is forgotten, then requests unload without a Ready publication. A
//!   forgotten missing load removes its loading record without generation.
//!   The legacy `ChunkWants` helper keeps its historical NotWanted refusal;
//!   that identity-only fixture is not the managed installation consumer.
//! - `packages/server/sim/runtime/engine_step.go` (`StepWithTunables`): the
//!   acquire work sits after companion actions and before physics, the seam
//!   position `RulePhase::Acquire` holds in the frozen phase order.
//! - `packages/server/sim/realm/state.go` (`BeginLoading`, `MarkGenerating`,
//!   `ApplyLoaded`, `ApplyGenerated`, `MarkLoadFailed`): the record state
//!   machine silently skips every completion whose record left the expected
//!   state, and a failed completion consumes the attempt while the key stays
//!   available for a re-issued load. The frozen `ChunkResult`
//!   request/generation identity sharpens those skips into the counted
//!   stale-generation and consumed refusals below.
//!
//! Legacy fixture ledgers below retain their original identity-only behavior.
//! Enabled managed worlds instead transfer prepared owners through the actual
//! Acquire entry, after companion intent and before player physics.

use std::collections::{BTreeMap, BTreeSet};

use crate::core::contracts::{
    ChunkKey, ChunkRequestId, ChunkResult, PhaseReport, RuleCall, RulePhase, ServerError,
};
use crate::core::state::TickContext;

/// One issued want: the generation the want was made at and the one request
/// identity that may answer it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct WantRecord {
    generation: u64,
    request: ChunkRequestId,
}

/// The caller-owned want union, the counterpart of the Go engine's `wanted`
/// map that `applyAcquired`/`applyGenerated` receive as a parameter.
///
/// Keys are the frozen `ChunkKey` (dimension plus chunk coordinate); the
/// generation and request identity complete the frozen `ChunkResult` key.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ChunkWants {
    wanted: BTreeMap<ChunkKey, WantRecord>,
    consumed: BTreeSet<ChunkRequestId>,
}

impl ChunkWants {
    pub fn new() -> Self {
        Self::default()
    }

    /// Issues one want for a key at a generation under one request identity.
    ///
    /// A later want for the same key supersedes the earlier one, exactly like
    /// the Go record replacement in `realm/state.go`: the superseded request
    /// keeps no ledger entry, so its late completion refuses through the
    /// generation gate below.
    pub fn want(&mut self, key: ChunkKey, generation: u64, request: ChunkRequestId) {
        self.wanted.insert(
            key,
            WantRecord {
                generation,
                request,
            },
        );
    }

    /// Drops the want and returns its issued request so the caller can arm
    /// the mailbox tombstone (`MailboxPort::cancel_chunk`), the cancellation
    /// half of the Go reconcile path that unloads forgotten keys.
    pub fn forget(&mut self, key: ChunkKey) -> Option<ChunkRequestId> {
        self.wanted.remove(&key).map(|want| want.request)
    }

    /// The current want generation and issued request for one key.
    pub fn want_at(&self, key: ChunkKey) -> Option<(u64, ChunkRequestId)> {
        self.wanted
            .get(&key)
            .map(|want| (want.generation, want.request))
    }
}

/// Outcome of one completion at the install gate.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AcquireOutcome {
    Installed,
    Refused(AcquireRefusal),
}

/// Counted refusal reasons for completions the mailbox already admitted.
///
/// `NotWanted`, `StaleGeneration`, `AlreadyConsumed` and `Failed` have no
/// state counter; each refusal surfaces through `PhaseReport.rejected`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AcquireRefusal {
    /// No current want for the key: the Go forgotten-key path that drops or
    /// unloads and never publishes.
    NotWanted,
    /// The key is wanted, but at a different generation or under a different
    /// request identity than the completion carries: the sharpened form of
    /// the Go state gate that skips a record no longer in the expected state.
    StaleGeneration { wanted: u64, found: u64 },
    /// This request was already answered (installed or failed). A drained
    /// completion the mailbox re-admits closes here instead of installing a
    /// second time.
    AlreadyConsumed,
    /// The producer reported a completion error: nothing installs, the
    /// request is consumed, and the want survives for the reconcile retry.
    Failed,
}

/// The install gate for one admitted completion.
///
/// Order matters: the consumed check runs first so a replayed completion
/// refuses as consumed even when its key is still wanted at the matching
/// generation. Refusals consume nothing and change no state, so repeated
/// examinations re-refuse identically; acceptance consumes the request so no
/// second completion of the same request can ever install.
pub fn apply(wants: &mut ChunkWants, result: &ChunkResult) -> AcquireOutcome {
    if wants.consumed.contains(&result.request) {
        return AcquireOutcome::Refused(AcquireRefusal::AlreadyConsumed);
    }
    let Some(want) = wants.wanted.get(&result.key) else {
        return AcquireOutcome::Refused(AcquireRefusal::NotWanted);
    };
    if want.request != result.request || want.generation != result.generation {
        return AcquireOutcome::Refused(AcquireRefusal::StaleGeneration {
            wanted: want.generation,
            found: result.generation,
        });
    }
    if result.result.is_err() {
        wants.consumed.insert(result.request);
        return AcquireOutcome::Refused(AcquireRefusal::Failed);
    }
    wants.consumed.insert(result.request);
    AcquireOutcome::Installed
}

/// Applies one drained mailbox batch and reports the phase counts.
///
/// This is the batch body the serial tick reducer calls after
/// `drain_chunks`: `examined` counts the drained completions, `applied` the
/// installs, and `rejected` every counted refusal.
pub fn apply_drained(wants: &mut ChunkWants, drained: &[ChunkResult]) -> PhaseReport {
    let mut report = PhaseReport {
        examined: drained.len(),
        applied: 0,
        carried: 0,
        rejected: 0,
    };
    for result in drained {
        match apply(wants, result) {
            AcquireOutcome::Installed => report.applied += 1,
            AcquireOutcome::Refused(_) => report.rejected += 1,
        }
    }
    report
}

/// Applies managed prepared completions only for the exact acquisition call.
pub fn run(ctx: &mut TickContext<'_>, call: RuleCall<'_>) -> Result<PhaseReport, ServerError> {
    if call.phase != RulePhase::Acquire
        || call.actor.is_some()
        || call.command.is_some()
        || call.internal.is_some()
    {
        return Ok(PhaseReport {
            examined: 0,
            applied: 0,
            carried: 0,
            rejected: 0,
        });
    }
    Ok(ctx.apply_live_acquisition())
}
