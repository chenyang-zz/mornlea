//! Pure prompt derivation from recorded ray targets and confirmed block content.
//! Missing world context emits no fact; an unknown or invalid hit clears the prompt.

use mornlea_domain::Event;

use crate::contracts::{ClientError, FamilyOperation, RecordHeader, registered_label_text};
use crate::presentation::frame::WorldUiRecord;
use crate::presentation::{
    OrderedRecord, ProjectionOrder, ProjectionView, PromptView, StableRecordKey, WorldTopic,
    WorldUiView,
};
use crate::runtime_data;

/// Projects the selected pose's fresh hit without recomputing rays or inventing a source tick.
pub fn project_prompt(
    view: &ProjectionView<'_>,
) -> Result<Vec<OrderedRecord<WorldUiRecord>>, ClientError> {
    // Validate the entire retained queue before returning any derived fact.
    for observation in view.observations() {
        let key = *observation.key();
        if key.epoch() != view.frame_epoch() {
            return Err(ClientError::StaleEpoch);
        }
        Event::try_from(observation.packet().clone()).map_err(|_| ClientError::InvalidInput)?;
    }
    let Some(dimension) = view.player().world_dimension() else {
        return Ok(Vec::new());
    };
    // A predicted pose with no fresh hit must not fall back to another pose's target.
    let target = if view.player().predicted_pose().is_some() {
        view.player().predicted_target()
    } else {
        view.player().confirmed_target()
    }
    .filter(|target| target.source_revision() == view.frame_revision());
    let mut prompt = None;
    if let Some(target) = target
        && let Some(block) = view.mirror().world().block_at(dimension, target.position())
        // Barrier is registered but has no player-visible prompt.
        && block != 1
        && runtime_data::interaction_target(block)
        && let Some(label) = registered_label_text(block)?
    {
        prompt = Some(PromptView::try_new(target.position(), label)?);
    }
    let record = WorldUiRecord::try_new(
        RecordHeader::try_new(
            view.frame_epoch(),
            view.frame_revision(),
            None,
            FamilyOperation::Upsert,
        )?,
        WorldUiView::Prompt(prompt),
    )?;
    Ok(vec![OrderedRecord::try_new(
        ProjectionOrder::AfterConfirmed {
            epoch: view.frame_epoch(),
            revision: view.frame_revision(),
            local_sequence: view.input().sequence_hint(),
            record_ordinal: 0,
        },
        StableRecordKey::World {
            topic: WorldTopic::Prompt,
            identity: None,
        },
        record,
    )?])
}
