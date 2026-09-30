//! Bounded tick mailbox, chunk-result admission, and cancellation.
//!
//! This module owns the reduction-side mailbox discipline the tick step
//! consumes: freezing one tick's eligible command batch, ordering it with
//! the accepted domain comparator, applying at most the budgeted prefix
//! while the carried suffix keeps its original intake identity, and
//! classifying chunk-result admission so a late completion after
//! cancellation, and a repeated completion, are discarded with a counted
//! reason instead of installing. Cancellation retires the generation's
//! queued work; it never publishes a partial result. World rules, tick
//! reduction, and transport stay outside this module.

use mornlea_domain::{CommandEnvelope, CommandOrderScratch, order_commands};

use super::contracts::{ChunkRequestId, ChunkResult, Resource, ServerError, SessionKey};
use super::state::AuthorityState;

/// Classified outcome of one chunk-result admission.
///
/// `CancelledDiscarded` and `DuplicateDiscarded` both release the owned
/// completion buffers exactly once and never install a record; the
/// distinction is the counted discard reason the state exposes.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ChunkAdmission {
    Admitted,
    CancelledDiscarded,
    DuplicateDiscarded,
}

/// Freezes, orders, and budget-applies one tick's sequenced batch.
///
/// The eligible batch is frozen, ordered in place by the domain's
/// `(tick, session, sequence, arrival_index)` rule, and split at the
/// budget: the positional prefix is walked so the per-session sequence
/// watermark decides admission, and the suffix is carried back unchanged
/// with its original earliest tick and arrival identity. An envelope in the
/// prefix whose sequence is at or below the watermark is a stale duplicate:
/// it has been walked and counted stale by absence, so it is silently
/// dropped rather than returned or carried. The returned vector holds the
/// applied envelopes in applied order. A refused sort returns the frozen
/// work to the queue before the typed error, so no accepted command is
/// lost.
pub fn budgeted_batch(
    state: &mut AuthorityState,
    tick: u64,
    budget: usize,
) -> Result<Vec<CommandEnvelope>, ServerError> {
    let mut batch = state.freeze_eligible(tick);
    if let Err(error) = sort_batch(&mut batch) {
        // The refusal must not drop accepted work. Carrying the drained
        // batch back cannot exceed the command limit these same envelopes
        // already occupied before the freeze.
        let _ = state.carry(batch);
        return Err(error);
    }
    let prefix_len = budget.min(batch.len());
    let suffix = batch.split_off(prefix_len);
    // The suffix reoccupies the queue slots these same envelopes held
    // before the freeze, so this carry cannot exceed the command limit.
    let _ = state.carry(suffix);
    let mut applied = Vec::new();
    for envelope in batch {
        let Some(key) = SessionKey::from_raw(envelope.session()) else {
            continue;
        };
        if state.apply_sequence(key, envelope.sequence()) {
            applied.push(envelope);
        }
    }
    Ok(applied)
}

/// Classifies one chunk-result admission against the counted discards.
///
/// A full mailbox refuses without loss by returning the owned record, so
/// the producer keeps its buffers. Admission otherwise reports which
/// counted reason consumed the completion: a cancellation tombstone rejects
/// exactly one late completion, and a repeat after that (or of an
/// already-queued request) counts as a duplicate. Either discard releases
/// the owned buffers exactly once and never installs a record.
// Refusal hands the producer's owned record straight back; no copy is made.
#[allow(clippy::result_large_err)]
pub fn admit_chunk_result(
    state: &mut AuthorityState,
    result: ChunkResult,
) -> Result<ChunkAdmission, ChunkResult> {
    let (cancel_before, duplicate_before) = state.chunk_discard_counts();
    match state.admit_chunk(result) {
        Err(owned) => Err(owned),
        Ok(()) => {
            let (cancel_after, duplicate_after) = state.chunk_discard_counts();
            if cancel_after > cancel_before {
                Ok(ChunkAdmission::CancelledDiscarded)
            } else if duplicate_after > duplicate_before {
                Ok(ChunkAdmission::DuplicateDiscarded)
            } else {
                Ok(ChunkAdmission::Admitted)
            }
        }
    }
}

/// Retires one chunk request's generation work.
///
/// Any queued result for the request is dropped and the request is armed
/// with a tombstone that rejects exactly one further late completion with
/// a counted cancellation. Delegates to `AuthorityState::cancel_chunk`.
pub fn cancel_chunk_request(state: &mut AuthorityState, request: ChunkRequestId) {
    state.cancel_chunk(request)
}

/// Sorts one frozen batch with reusable key scratch.
///
/// The scratch is sized for exactly the batch, so an unreservable request is
/// a commands-plane capacity refusal for that many envelopes, and a domain
/// ordering rejection (only a synthetic carried envelope can trigger it) is
/// the packet input error for the arrival key. This mirrors the session
/// provider's private helper: the pattern stays local so the two reduction
/// examples remain independently replaceable.
fn sort_batch(batch: &mut [CommandEnvelope]) -> Result<(), ServerError> {
    let mut scratch =
        CommandOrderScratch::try_with_capacity(batch.len()).map_err(|_| ServerError::Capacity {
            resource: Resource::Commands,
            limit: batch.len(),
            observed: batch.len(),
        })?;
    order_commands(batch, &mut scratch).map_err(|_| ServerError::InvalidInput { field: "arrival" })
}
