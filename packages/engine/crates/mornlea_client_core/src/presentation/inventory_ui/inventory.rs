//! The personal inventory (hotbar and backpack) UI projection.
//!
//! This provider turns the personal-inventory publications of the committed
//! observation queue into ordered `inventory-ui@1` records: one confirmed
//! `InventoryState` observation projects exactly one upsert carrying the
//! complete published value — the selected hotbar index, the nine hotbar
//! slots and the twenty-seven backpack slots — through the inventory topic
//! tag with no container or event identity, because the personal inventory
//! is a singleton with no external identity of its own.
//!
//! Ownership boundaries: the projection is pure over the immutable
//! `ProjectionView` and owns no second mirror, no packet history and no
//! presentation state. The contents are server-authoritative alone — a
//! server-rejected selection, a locally admitted unconfirmed input and a
//! placement confirmation that carries no contents never change the projected
//! stacks, because only the next complete authoritative publication does —
//! and the record carries no container token and no outcome, which the serial
//! family owner unions. Observations of other kinds are ignored wholesale:
//! they belong to their own per-topic providers. A malformed observation
//! identity — a packet that is not a checked publication or an observation
//! from another epoch — rejects the whole projection before any record is
//! returned, so no partial output exists.

use mornlea_domain::Event;

use crate::contracts::{ClientError, FamilyOperation, RecordHeader};
use crate::presentation::frame::InventoryUiRecord;
use crate::presentation::{
    InventoryTopic, InventoryUiView, OrderedRecord, ProjectionOrder, ProjectionView,
    StableRecordKey,
};

/// The stable identity of one personal-inventory record: the inventory topic
/// tag alone. The personal inventory is a singleton with no container or
/// event identity, so the identity field is absent rather than fabricated.
fn stable_key() -> StableRecordKey {
    StableRecordKey::Inventory {
        topic: InventoryTopic::Inventory,
        identity: None,
    }
}

/// The rebased coherent candidate header of one record, preserving the
/// observation's own optional source tick exactly; the inventory publication
/// carries none, so none is ever invented here.
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

/// Projects the personal-inventory records of one immutable view, in actual
/// source order: observation key first, then the packet record ordinal.
///
/// Every confirmed `InventoryState` observation projects exactly one upsert
/// carrying the complete published value; observations of other kinds are
/// ignored wholesale, because they belong to their own per-topic providers.
/// Order is never reconstructed from the absent source tick nor from the
/// rebased parent revision. The serial inventory-UI family assembler consumes
/// these envelopes and strips the envelope when the unchanged record vector
/// is emitted.
pub fn project_inventory(
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
        if let Event::InventoryState(state) = event {
            // A complete publication is one upsert of the whole value: no
            // container token, because the personal inventory is not an
            // external view, and no outcome, which the serial family owner
            // unions from the sequence-bound outcome observations.
            let record = InventoryUiRecord::try_new(
                header(view, observation.source_tick(), FamilyOperation::Upsert)?,
                InventoryUiView::Inventory(state),
                None,
                None,
            )?;
            records.push(OrderedRecord::try_new(
                ProjectionOrder::Confirmed {
                    observation: key,
                    record_ordinal: 0,
                },
                stable_key(),
                record,
            )?);
        }
    }
    Ok(records)
}
