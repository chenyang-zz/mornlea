//! Session admission, bounded sequenced intake, and the control plane.
//!
//! This module owns the session lifecycle semantics the endpoint exports:
//! admitting a protocol-checked login into the active session plane, routing
//! an intent into the sequenced or control plane, retiring a session exactly
//! once, and the reduction-side example that orders one tick's frozen batch
//! and advances the per-session sequence watermark. Every entry point reaches
//! the authority only through the frozen state ports; world rules, tick
//! reduction, and transport stay outside this module.

use mornlea_domain::{CommandEnvelope, CommandOrderScratch, order_commands};
use mornlea_protocol::{AdmittedLogin, PlayIntent};

use super::contracts::{
    CloseReason, Resource, ServerError, SessionKey, SubmissionReceipt, TransportKind,
};
use super::state::AuthorityState;

/// Admits a protocol-checked login into the active session plane.
///
/// Protocol version negotiation belongs to the upstream admission that
/// produced the `AdmittedLogin`; this boundary rechecks running phase, live
/// player capacity, and duplicate live identity, and publishes a monotonic
/// nonzero session key. Delegates to `AuthorityState::allocate`.
pub fn admit(
    state: &mut AuthorityState,
    login: AdmittedLogin,
    transport: TransportKind,
) -> Result<SessionKey, ServerError> {
    state.allocate(login, transport)
}

/// Routes one intent into its intake plane.
///
/// Chat and keepalive replies are control traffic: they are acknowledged
/// without enqueueing a domain command and without consuming an arrival
/// index. A sequenced intent enters the bounded command queue and receives
/// the earliest eligible tick plus the session's next arrival index.
/// Out-of-order and duplicate sequences are accepted here; the sequence
/// watermark advances only inside the sorted reduction. Delegates to
/// `AuthorityState::accept`.
pub fn submit(
    state: &mut AuthorityState,
    session: SessionKey,
    intent: PlayIntent,
) -> Result<SubmissionReceipt, ServerError> {
    state.accept(session, intent)
}

/// Retires a session once and closes it to further intake.
///
/// Retiring the same key a second time reports the session as stale, so a
/// retired key can be neither re-closed nor reused for submissions.
/// Delegates to `AuthorityState::retire`.
pub fn close_session(
    state: &mut AuthorityState,
    session: SessionKey,
    reason: CloseReason,
) -> Result<(), ServerError> {
    state.retire(session, reason)
}

/// Freezes, orders, and applies one tick's sequenced batch.
///
/// This is the reduction-side example of the intake contract: the eligible
/// batch is frozen, ordered in place by the domain's
/// `(tick, session, sequence, arrival_index)` rule, and then walked in that
/// order so the per-session sequence watermark decides admission. An envelope
/// whose sequence is at or below the watermark is a stale duplicate and is
/// silently discarded; the returned vector holds the applied envelopes in
/// applied order. A refused sort returns the frozen work to the queue before
/// the typed error, so no accepted command is lost.
pub fn apply_sorted_batch(
    state: &mut AuthorityState,
    tick: u64,
) -> Result<Vec<CommandEnvelope>, ServerError> {
    let mut batch = state.freeze_eligible(tick);
    if let Err(error) = sort_batch(&mut batch) {
        // The refusal must not drop accepted work. Carrying the drained batch
        // back cannot exceed the command limit these same envelopes already
        // occupied before the freeze.
        let _ = state.carry(batch);
        return Err(error);
    }
    let mut applied = Vec::new();
    for envelope in &batch {
        let Some(key) = SessionKey::from_raw(envelope.session()) else {
            continue;
        };
        if state.apply_sequence(key, envelope.sequence()) {
            applied.push(*envelope);
        }
    }
    Ok(applied)
}

/// Sorts one frozen batch with reusable key scratch.
///
/// The scratch is sized for exactly the batch, so an unreservable request is
/// a commands-plane capacity refusal for that many envelopes, and a domain
/// ordering rejection (only a synthetic carried envelope can trigger it) is
/// the packet input error for the arrival key.
fn sort_batch(batch: &mut [CommandEnvelope]) -> Result<(), ServerError> {
    let mut scratch =
        CommandOrderScratch::try_with_capacity(batch.len()).map_err(|_| ServerError::Capacity {
            resource: Resource::Commands,
            limit: batch.len(),
            observed: batch.len(),
        })?;
    order_commands(batch, &mut scratch).map_err(|_| ServerError::InvalidInput { field: "arrival" })
}
