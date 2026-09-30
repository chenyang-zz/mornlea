//! Owned tick publication and bounded slow-receiver outboxes.
//!
//! This module owns the delivery semantics the endpoint exports for one
//! tick's owned output: publishing the encoded events and control replies a
//! tick produced into per-session frame outboxes, draining a receiver's
//! outbox under explicit frame and byte budgets, and closing a receiver.
//! Publication never waits on a receiver: an outbox that reaches its frame
//! limit is closed and its session retired with the frozen slow-receiver
//! reason inside the state port, silently, without appending a Disconnect
//! frame, so one slow receiver can neither block the tick nor disturb any
//! peer's ordered delivery. World rules, tick reduction, and transport stay
//! outside this module.

use super::contracts::{CloseReason, ServerError, SessionKey, TickPublication};
use super::state::AuthorityState;

/// Publishes one tick's owned events and control replies.
///
/// Events are encoded through the protocol conversion before any append;
/// a session-directed frame lands only on its recipient and a broadcast
/// frame expands once over the sessions present at publish time. A receiver
/// whose outbox is already closed keeps nothing. When an append would exceed
/// the per-session frame limit, the overflowing frame is dropped and that
/// receiver alone is retired with the slow-receiver reason, so publishing
/// succeeds without blocking or erroring on a slow receiver. Delegates to
/// `AuthorityState::publish`.
pub fn publish_tick(
    state: &mut AuthorityState,
    publication: TickPublication,
) -> Result<(), ServerError> {
    state.publish(publication)
}

/// Drains one receiver's outbox under explicit budgets.
///
/// Complete protocol frames retain their packet IDs and leave in publication
/// order; the drain stops at whichever budget
/// bites first, always delivering at least one frame when the outbox is
/// nonempty, and the undelivered suffix stays queued for the next drain.
/// Draining a retired receiver returns the frames it already held. Delegates
/// to `AuthorityState::take_outbox`.
pub fn drain_outbox(
    state: &mut AuthorityState,
    session: SessionKey,
    max_frames: usize,
    max_bytes: usize,
) -> Result<Vec<Vec<u8>>, ServerError> {
    state.take_outbox(session, max_frames, max_bytes)
}

/// Closes one receiver's outbox without retiring the session.
///
/// Closing is idempotent and stops further appends only; the frames already
/// queued remain drainable. This is the publication-side control for a
/// receiver the endpoint has decided to stop feeding; full session
/// retirement stays on the session port. Delegates to
/// `AuthorityState::close_outbox`.
pub fn close_receiver(state: &mut AuthorityState, session: SessionKey, reason: CloseReason) {
    state.close_outbox(session, reason)
}
