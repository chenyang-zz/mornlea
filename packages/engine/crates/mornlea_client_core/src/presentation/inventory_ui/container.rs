//! The external-container UI projection: chest views and closed
//! notifications.
//!
//! This provider turns the container publications of the committed
//! observation queue into ordered `inventory-ui@1` records. An authoritative
//! chest state is one complete upsert record carrying the exact checked
//! `ChestState` payload — the twenty-seven slots and the typed
//! kind/chunk/slot/generation reference preserved verbatim, with no rebuilt
//! copy and no invented field — and a closed notification is one remove-only
//! `Closed` record naming the exact container identity the publication
//! carried, because the close packet names the reference and nothing else.
//!
//! Token ownership: the token on a chest record is the local view
//! attribution the accepted `ContainerToken` semantics give this epoch, the view's
//! reference and the confirmed revision the mirror staged the view at, which
//! is exactly the observation's own confirmed revision. It is projected
//! mirror attribution, never a wire field: the chest publication carries no
//! revision, so none is fabricated, and the record's rebased header revision
//! (the coherent candidate frame revision) stays deliberately distinct from
//! the token's view revision. A closed record carries no token, because a
//! retired view has no mirror attribution left to project and reconstructing
//! the pre-close revision would invent one. Neither publication carries a
//! command outcome, so `outcome` stays `None` — acceptance and rejection
//! belong to their own sequence-bound observations.
//!
//! Tolerance boundaries: the provider projects what the mirror accepted, so
//! the accepted close tolerance stays visible — a close naming an
//! already-replaced generation or another kind's reference still publishes
//! its own `Closed` record under its own actual identity (a distinct
//! generation or kind is a distinct reference, hence a distinct stable key)
//! while the mirror's live views stay exactly as they were, and a late chest
//! state at an already-replaced generation was refused whole by the mirror,
//! so it never reaches the queue at all. Crafting keeps its separate token
//! through its own provider; furnace views, personal inventory and command
//! outcomes belong to their own per-topic providers and are ignored
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

/// Projects the container records of one immutable view, in actual source
/// order: observation key first, then the packet record ordinal.
///
/// A chest or closed publication carries exactly one container view, so
/// every record's packet ordinal is zero; order is never reconstructed from
/// the optional source tick — both publications carry none — nor from the
/// rebased parent revision. Repeated stable keys across observations are the
/// legal replace lifecycle of one container position, resolved by the serial
/// family assembler, never a provider-side duplicate. The stable key is the
/// inventory topic tag plus the actual container identity: the chest topic
/// for a view, the closed topic for a removal, so a view and its own close
/// never collide.
pub fn project_container(
    view: &ProjectionView<'_>,
) -> Result<Vec<OrderedRecord<InventoryUiRecord>>, ClientError> {
    let mut records: Vec<OrderedRecord<InventoryUiRecord>> = Vec::new();
    for observation in view.observations() {
        let key = *observation.key();
        if key.epoch() != view.frame_epoch() {
            // An observation from another epoch never enters this frame,
            // exactly as the confirmed mirror refuses old epochs.
            return Err(ClientError::StaleEpoch);
        }
        let event =
            Event::try_from(observation.packet().clone()).map_err(|_| ClientError::InvalidInput)?;
        match event {
            Event::ChestState(state) => {
                let reference = state.container();
                // The local attribution this view actually carries: the
                // observation's own confirmed revision is the revision the
                // mirror staged the view at, so the token is projected
                // mirror attribution, never a fabricated wire revision.
                let token = ContainerToken::try_new(
                    view.frame_epoch(),
                    reference,
                    key.confirmed_revision(),
                )?;
                let record = InventoryUiRecord::try_new(
                    header(view, observation.source_tick(), FamilyOperation::Upsert)?,
                    InventoryUiView::Chest(state),
                    Some(token),
                    None,
                )?;
                records.push(OrderedRecord::try_new(
                    ProjectionOrder::Confirmed {
                        observation: key,
                        record_ordinal: 0,
                    },
                    StableRecordKey::Inventory {
                        topic: InventoryTopic::Chest,
                        identity: Some(reference),
                    },
                    record,
                )?);
            }
            Event::ContainerClosed(closed) => {
                let reference = closed.container();
                // A close publishes the identity alone: no view payload, no
                // token — a retired view has no mirror attribution left —
                // and no outcome, exactly the fields the packet carries.
                let record = InventoryUiRecord::try_new(
                    header(view, observation.source_tick(), FamilyOperation::Remove)?,
                    InventoryUiView::Closed(reference),
                    None,
                    None,
                )?;
                records.push(OrderedRecord::try_new(
                    ProjectionOrder::Confirmed {
                        observation: key,
                        record_ordinal: 0,
                    },
                    StableRecordKey::Inventory {
                        topic: InventoryTopic::Closed,
                        identity: Some(reference),
                    },
                    record,
                )?);
            }
            // Every other publication belongs to another per-topic provider;
            // no container record or attribution is drawn from it.
            _ => {}
        }
    }
    Ok(records)
}
