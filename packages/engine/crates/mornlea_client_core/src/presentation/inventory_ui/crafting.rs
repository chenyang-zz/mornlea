//! The crafting UI projection: grid publications and sequence-bound
//! rejections.
//!
//! This provider turns the crafting publications of the committed
//! observation queue into ordered `inventory-ui@1` records. One confirmed
//! `CraftingState` observation projects exactly one upsert carrying the
//! complete published value — the personal or workbench size, the nine grid
//! slots with the personal grid's five unused cells published empty, and the
//! output the authority derived from the grid — through the crafting topic
//! tag with no container identity, because a crafting view is a session
//! singleton that never occupies a container reference: the workbench raises
//! the grid's usable size without naming one, so none is fabricated here.
//! Repeated crafting keys across observations are the legal latest-wins
//! lifecycle of the one grid, resolved by the serial family assembler from
//! the retained source order, never a provider-side duplicate.
//!
//! Token ownership: the crafting view's local attribution is the separate
//! crafting token of the accepted input rules — epoch, confirmed revision
//! and size — which the record schema has no field for and which this
//! projection therefore never rebuilds as a `ContainerToken`. The confirmed
//! mirror remains the sole attribution owner: a container publication or
//! close is another topic's input and changes no crafting attribution, no
//! crafting revision and no record here.
//!
//! Recipe ownership: the output is derived by the authority from the grid
//! and published inside the state. No recipe table exists in the checked
//! domain and none is invented here, so a valid recipe's output passes
//! through verbatim and an unavailable recipe publishes the empty output
//! exactly as the authority published it — never fabricated into a valid
//! one, never recomputed.
//!
//! Rejections: one confirmed `CommandRejected` observation projects one
//! separate upsert `Rejected` record carrying the exact checked payload and
//! the sequence-bound `UiOutcome::Rejected` outcome with the refused
//! action's actual sequence. A rejection is an outcome, never a contents
//! change: the confirmed crafting view's revision and records stay exactly
//! as they were, and a locally admitted unconfirmed crafting action debits
//! nothing, because only the next complete authoritative publication
//! replaces the projected grid. Observations of other kinds are ignored
//! wholesale: they belong to their own per-topic providers. A malformed
//! observation identity — a packet that is not a checked publication or an
//! observation from another epoch — rejects the whole projection before any
//! record is returned, so no partial output exists.

use mornlea_domain::Event;

use crate::contracts::{ClientError, FamilyOperation, RecordHeader};
use crate::presentation::frame::InventoryUiRecord;
use crate::presentation::{
    InventoryTopic, InventoryUiView, OrderedRecord, ProjectionOrder, ProjectionView,
    StableRecordKey, UiOutcome,
};

/// The rebased coherent candidate header of one record, preserving the
/// observation's own optional source tick exactly; neither the crafting nor
/// the rejection publication carries one, so none is ever invented here.
fn header(
    view: &ProjectionView<'_>,
    source_tick: Option<u64>,
    operation: FamilyOperation,
) -> Result<RecordHeader, ClientError> {
    RecordHeader::try_new(
        view.frame_epoch(),
        view.frame_revision(),
        source_tick,
        operation,
    )
}

/// Projects the crafting records of one immutable view, in actual source
/// order: observation key first, then the packet record ordinal.
///
/// Every confirmed `CraftingState` observation projects exactly one upsert
/// carrying the complete published value, and every confirmed
/// `CommandRejected` observation projects exactly one separate
/// sequence-bound `Rejected` record that never mutates them. Order is never
/// reconstructed from the absent source tick nor from the rebased parent
/// revision. The serial inventory-UI family assembler consumes these
/// envelopes and strips the envelope when the unchanged record vector is
/// emitted.
pub fn project_crafting(
    view: &ProjectionView<'_>,
) -> Result<Vec<OrderedRecord<InventoryUiRecord>>, ClientError> {
    let mut records: Vec<OrderedRecord<InventoryUiRecord>> = Vec::new();
    for observation in view.observations() {
        let key = *observation.key();
        if key.epoch() != view.frame_epoch() {
            // An observation from another epoch never enters this frame,
            // exactly as the confirmed mirror refuses old epochs; the whole
            // projection rejects rather than publishing a partial prefix.
            return Err(ClientError::StaleEpoch);
        }
        let event =
            Event::try_from(observation.packet().clone()).map_err(|_| ClientError::InvalidInput)?;
        match event {
            Event::CraftingState(state) => {
                // A complete publication is one upsert of the whole value:
                // no container token, because the crafting view is no
                // external container and its separate crafting attribution
                // stays with the mirror and the input rules, and no outcome,
                // which the serial family owner unions from the
                // sequence-bound outcome records.
                let record = InventoryUiRecord::try_new(
                    header(view, observation.source_tick(), FamilyOperation::Upsert)?,
                    InventoryUiView::Crafting(state),
                    None,
                    None,
                )?;
                records.push(OrderedRecord::try_new(
                    ProjectionOrder::Confirmed {
                        observation: key,
                        record_ordinal: 0,
                    },
                    StableRecordKey::Inventory {
                        topic: InventoryTopic::Crafting,
                        identity: None,
                    },
                    record,
                )?);
            }
            Event::CommandRejected(rejection) => {
                // The refusal is its own sequence-bound outcome record under
                // the rejected topic tag: it names the refused action's
                // actual sequence and reason, never a crafting contents
                // change, so the confirmed crafting view's records and
                // revision stay exactly as they were.
                let record = InventoryUiRecord::try_new(
                    header(view, observation.source_tick(), FamilyOperation::Upsert)?,
                    InventoryUiView::Rejected(rejection),
                    None,
                    Some(UiOutcome::Rejected {
                        sequence: rejection.sequence(),
                        reason: rejection.reason(),
                    }),
                )?;
                records.push(OrderedRecord::try_new(
                    ProjectionOrder::Confirmed {
                        observation: key,
                        record_ordinal: 0,
                    },
                    StableRecordKey::Inventory {
                        topic: InventoryTopic::Rejected,
                        identity: None,
                    },
                    record,
                )?);
            }
            // Every other publication belongs to another per-topic provider;
            // no crafting record or attribution is drawn from it.
            _ => {}
        }
    }
    Ok(records)
}
