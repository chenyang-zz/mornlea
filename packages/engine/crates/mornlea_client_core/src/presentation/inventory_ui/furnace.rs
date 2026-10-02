//! The furnace UI projection.
//!
//! This provider turns the furnace publications of the committed observation
//! queue into ordered `inventory-ui@1` records. An authoritative furnace
//! state is one complete upsert record carrying the exact checked
//! `FurnaceState` payload — the input, fuel and output slots, the typed
//! kind/chunk/slot/generation reference and the two authoritative timers,
//! preserved verbatim with no rebuilt copy and no invented field — because
//! the checked bounds (a progress strictly below the smelt requirement, a
//! burn time at or below its maximum, the slot whitelists) were enforced
//! before the value existed, so the provider restates none of them.
//!
//! Token ownership: the token on a furnace record is the local view
//! attribution the accepted `ContainerToken` semantics give this epoch, the
//! view's reference and the confirmed revision the mirror staged the view
//! at, which is exactly the observation's own confirmed revision. It is
//! projected mirror attribution, never a wire field: the furnace publication
//! carries no revision, so none is fabricated, and the record's rebased
//! header revision (the coherent candidate frame revision) stays deliberately
//! distinct from the token's view revision. The publication carries no
//! command outcome, so `outcome` stays `None` — acceptance and rejection
//! belong to their own sequence-bound observations.
//!
//! Tolerance boundaries: the provider projects what the mirror accepted. A
//! republication at a new generation is the legal replace lifecycle of one
//! furnace position, so repeated distinct-generation references are distinct
//! stable keys while the mirror retires the replaced view; a late state at an
//! already-replaced generation was refused whole by the mirror, so it never
//! reaches the queue at all; a closed notification belongs to the container
//! provider's closed topic and contributes no furnace record, whatever
//! generation or kind it names, while the mirror alone decides whether the
//! close retired the view. Personal inventory, crafting views, chests and
//! command outcomes belong to their own per-topic providers and are ignored
//! wholesale here. A packet that is not a checked publication, or an
//! observation from another epoch, rejects the whole projection before any
//! record is returned, so no partial output exists; the confirmed mirror
//! remains the sole attribution and state owner and the serial inventory
//! family assembler remains the publication owner.

use mornlea_domain::Event;

use crate::contracts::{ClientError, FamilyOperation, RecordHeader};
use crate::input::ContainerToken;
use crate::presentation::frame::InventoryUiRecord;
use crate::presentation::{
    InventoryTopic, InventoryUiView, OrderedRecord, ProjectionOrder, ProjectionView,
    StableRecordKey,
};

/// The rebased coherent candidate header of one record, preserving the
/// observation's own optional source tick exactly.
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

/// Projects the furnace records of one immutable view, in actual source
/// order: observation key first, then the packet record ordinal.
///
/// A furnace publication carries exactly one container view, so every
/// record's packet ordinal is zero; order is never reconstructed from the
/// optional source tick — the publication carries none — nor from the
/// rebased parent revision. Repeated stable keys across observations are the
/// legal replace lifecycle of one furnace position, resolved by the serial
/// family assembler, never a provider-side duplicate. The stable key is the
/// inventory furnace topic tag plus the actual furnace identity the
/// publication carried, never the absent source tick.
pub fn project_furnace(
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
        if let Event::FurnaceState(state) = event {
            let reference = state.container();
            // The local attribution this view actually carries: the
            // observation's own confirmed revision is the revision the
            // mirror staged the view at, so the token is projected
            // mirror attribution, never a fabricated wire revision.
            let token =
                ContainerToken::try_new(view.frame_epoch(), reference, key.confirmed_revision())?;
            // A complete publication is one upsert of the whole value: no
            // outcome, because acceptance and rejection belong to their own
            // sequence-bound observations the serial family owner unions.
            let record = InventoryUiRecord::try_new(
                header(view, observation.source_tick(), FamilyOperation::Upsert)?,
                InventoryUiView::Furnace(state),
                Some(token),
                None,
            )?;
            records.push(OrderedRecord::try_new(
                ProjectionOrder::Confirmed {
                    observation: key,
                    record_ordinal: 0,
                },
                StableRecordKey::Inventory {
                    topic: InventoryTopic::Furnace,
                    identity: Some(reference),
                },
                record,
            )?);
        }
        // Every other publication belongs to another per-topic provider; no
        // furnace record or attribution is drawn from it.
    }
    Ok(records)
}
