//! The diagnostics family projection: bounded owner-maintained counters and
//! identity correlation only.
//!
//! This provider publishes exactly one `diagnostics@1` record per projection:
//! the producer/source/contract identity the diagnostics state owner holds,
//! the frame's epoch and confirmed revision as the record header, the frame
//! index as the explicit correlation field, and the owner-maintained
//! `QueueCounters`/`ErrorClassCounters` snapshots copied value-for-value.
//! The record is a local publication, so its header carries no source tick —
//! no server tick is fabricated for a locally generated fact — and the closed
//! record schema admits no raw packet bytes, no command text and no timing
//! field beyond the declared ones.
//!
//! Ownership boundaries: the projection is pure over the immutable
//! `ProjectionView` and owns no counters of its own. The input, I/O,
//! preparation, publication and lifetime owners maintain the counters; one
//! saturation event increments a counter exactly once at the owner and a poll
//! of this projection never increments anything, so repeated projections of
//! one view return the identical record. A counter at `u64::MAX` is the
//! contract's incomplete-evidence report and passes through verbatim — never
//! wrapped and never silently lowered. A producer identity that names no
//! source or no contract is the unset identity: a provenance record cannot
//! publish without provenance, so it rejects typed before any record exists.
//!
//! Bounds and failure policy: the candidate record is checked through the
//! accepted frame validator's own accounting over a minimal diagnostics-only
//! frame before it is returned, so a record that cannot fit the configured
//! family-record or frame byte caps rejects with the typed capacity error and
//! no partial vector exists. The projection owns no visible state, so a
//! rejected publication leaves the caller's previously visible frame — the
//! old publication — untouched by construction.

use crate::contracts::{ClientError, FAMILY_DIAGNOSTICS, FamilyKey, FamilyOperation, RecordHeader};
use crate::presentation::ProducerIdentity;
use crate::presentation::ProjectionView;
use crate::presentation::frame::{DiagnosticRecord, FamilyFrame, FamilyRecords, PresentationFrame};

/// Whether the identity names no producer: the unset all-zero source digest
/// or the unset all-zero contract digest. A diagnostics record exists to
/// attribute its counters, so an identity that attributes them to nothing is
/// rejected instead of published.
fn names_no_producer(identity: &ProducerIdentity) -> bool {
    identity.source_sha() == [0u8; 20] || identity.contract_sha() == [0u8; 32]
}

/// Projects the diagnostics record of one immutable view.
///
/// The counters are the owner-supplied snapshot exactly: nothing here
/// increments, wraps or clamps them, so polling is free of side effects and a
/// saturated counter stays at its incomplete-evidence maximum. The returned
/// vector holds exactly one checked record that already fits the view's
/// family-record and frame byte caps, or the typed rejection names why none
/// exists.
pub fn project_diagnostics(
    view: &ProjectionView<'_>,
) -> Result<Vec<DiagnosticRecord>, ClientError> {
    let state = view.diagnostics();
    if names_no_producer(state.producer()) {
        return Err(ClientError::InvalidInput);
    }
    // The local correlation header: the frame's epoch and revision, no
    // fabricated server tick; the frame index travels as the record's own
    // declared field below.
    let header = RecordHeader::try_new(
        view.frame_epoch(),
        view.frame_revision(),
        None,
        FamilyOperation::Upsert,
    )?;
    let record = DiagnosticRecord::try_new(
        header,
        *state.producer(),
        view.frame_index(),
        *state.queues(),
        *state.rejected(),
    )?;
    // The bounded-publication check: one record through the accepted frame
    // validator over a minimal diagnostics-only candidate, so the frozen
    // family-record count and the configured frame byte bound decide exactly
    // as they will at publication. A rejection is typed and returns no
    // partial vector; this pure projection never touches the caller's
    // previously visible frame.
    let candidate = PresentationFrame::try_new(
        view.frame_epoch(),
        view.frame_revision(),
        view.frame_index(),
        vec![FamilyFrame::try_new(
            FamilyKey::try_new(FAMILY_DIAGNOSTICS)?,
            FamilyRecords::Diagnostics(vec![record]),
        )?],
    )?;
    candidate.validate(view.limits())?;
    Ok(vec![record])
}
