//! The prompt (current ray target and registered display name) world UI
//! projection.
//!
//! The prompt is the mirror-derived presentation of the current checked ray
//! target and its registered display name — a derived local value, explicitly
//! not an authoritative prompt packet — carried by the
//! `WorldUiView::Prompt(Option<PromptView>)` view, where a visible prompt is
//! a `PromptView { target, label }`: the recorded target position beside the
//! registered display name of the block that position names.
//!
//! Partial landing under the controller's deferral ruling. The prompt fact
//! needs three links. The id→name link is landed: the crate contracts carry
//! the verbatim registered block display-name registry behind the checked
//! `registered_label_text` lookup. The ray→position link is landed: the
//! player projection state carries the recorded ray targets of the confirmed
//! and predicted look rays, derived at replay-step time through the accepted
//! engine raycast kernel and stamped with their source revision. The middle
//! position→block-id link does not exist on any surface the frozen
//! `ProjectionView` grants: the confirmed mirror carries chunk content
//! revisions only and the step-time collision grid carries collision
//! occupancy only, so no projection input can name the block at a recorded
//! position (the crate's block data lives in the preparation seam, which is
//! not a projection input and is frozen to this row). While the middle link
//! is absent no truthful `PromptView` can exist — `label` is a required
//! registered text, and an invented or empty label under a visible target
//! would fabricate exactly what this family forbids — so this provider
//! projects the schema's empty prompt state, no prompt record, for
//! target-present and target-absent states alike, exactly as the accepted Go
//! source returns no target when the block path is unknown. The repair's
//! natural owner is the real F2 integration that owns block data or a future
//! contract revision; the checked target reads in `project_prompt` are the
//! seam point the label chain completes.
//!
//! Ownership boundaries: the projection is pure over the immutable
//! `ProjectionView` and owns no second mirror, no packet history and no
//! presentation state. The shared observation queue is validated exactly
//! like every sibling world-UI provider — an observation from another epoch
//! rejects with `StaleEpoch` and a packet that is not a checked event
//! publication rejects with `InvalidInput`, the whole projection before any
//! record could exist, so no partial output ever exists — while the prompt
//! facts themselves come only from the player projection state: the stale
//! gate admits a confirmed recorded target only at or above the frame's
//! sampled revision, so an older target never projects as current.

use mornlea_domain::Event;

use crate::contracts::ClientError;
use crate::presentation::frame::WorldUiRecord;
use crate::presentation::{OrderedRecord, ProjectionView};

/// Projects the prompt records of one immutable view.
///
/// The prompt candidates of the frame are the player projection state's
/// recorded ray targets: the confirmed target at or above the frame's
/// sampled revision, beside the attributed predicted target. Under the
/// landed seams each candidate lacks the registered label its record
/// requires (see the module docs for the recorded deferral), so the
/// projection publishes the schema's empty prompt state — no record — and
/// never a fabricated prompt.
pub fn project_prompt(
    view: &ProjectionView<'_>,
) -> Result<Vec<OrderedRecord<WorldUiRecord>>, ClientError> {
    // The shared observation queue is validated exactly like every sibling
    // world-UI provider, so a malformed queue rejects every provider of the
    // frame whole: an observation from another epoch never enters this
    // frame, and a packet that is not a checked event publication is not a
    // publication at all.
    for observation in view.observations() {
        let key = *observation.key();
        if key.epoch() != view.frame_epoch() {
            return Err(ClientError::StaleEpoch);
        }
        Event::try_from(observation.packet().clone()).map_err(|_| ClientError::InvalidInput)?;
    }
    // The current prompt candidates: the stale gate admits the confirmed
    // recorded target only at or above the frame's sampled revision, and the
    // predicted target rides beside it as explicitly unconfirmed data.
    let confirmed = view
        .player()
        .confirmed_target()
        .filter(|target| target.source_revision() >= view.frame_revision());
    let predicted = view.player().predicted_target();
    // The deferral bites here: both candidates would project only as
    // `PromptView { target, label }`, and the registered label of the block
    // a recorded position names cannot be derived while the position→id link
    // is absent, so the candidates stay unprojected and the frame publishes
    // the schema's empty prompt state. No record is fabricated from a
    // position alone and no label text is invented.
    let _unlabelable_candidates = (confirmed, predicted);
    Ok(Vec::new())
}
