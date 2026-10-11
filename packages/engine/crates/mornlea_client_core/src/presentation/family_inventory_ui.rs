//! The inventory UI family serial assembler: the merge of the four accepted
//! per-topic provider vectors into one coherent `inventory-ui@1` record
//! vector.
//!
//! The four providers each publish a checked vector of source-ordered
//! envelopes for their own topic; this assembler is the single serial owner
//! that combines them. The parts array follows the packet's provider order
//! (inventory, container, crafting, furnace), but that order carries no
//! semantics here: retained `ProjectionOrder` keys — the actual observation
//! revision and ordinal, then the packet record ordinal — interleave the
//! topics by true source order, with the stable key as the final tiebreak,
//! so equal or absent source ticks never reconstruct order and the output is
//! never batched by topic. Every inventory-ui publication carries no source
//! tick, which is exactly why the envelopes, never the ticks, own the order
//! here.
//!
//! Identity and attribution: the envelope's stable key must be exactly the
//! key derived from the record it wraps — the topic tag of the view plus the
//! actual container identity a chest, furnace or closed record names, and no
//! identity for the singleton topics. The token and outcome union of the
//! parts survives unchanged and is validated against the shapes the four
//! accepted providers emit: a chest or furnace view carries exactly one
//! local attribution naming this epoch, its own view's reference and the
//! revision its own observation staged the view at; a closed record, a
//! singleton record and a rejection carry no token; a rejection carries
//! exactly the sequence-bound outcome its payload names; and no contents
//! record carries an outcome. A stale, missing or irrelevant attribution is
//! therefore malformed before any record is emitted. The rejection records
//! of the family-wide rejection projector — the crafting provider — are
//! merged as-is: this assembler never produces, filters or duplicates them.
//!
//! Ordering tolerance: repeated stable keys across observations are the
//! legal replace and latest-wins lifecycles of one container position and
//! one crafting grid, resolved by the retained source order, never
//! provider-side duplicates. A close ordered before its own open and a
//! rejection ordered before any crafting publication keep their actual
//! source order, because the accepted close tolerance and the standalone
//! rejection are provider semantics this assembler restates rather than
//! polices. Two envelopes claiming the same retained order slot for one
//! stable identity are indistinguishable source observations and reject the
//! whole family; two envelopes at one order slot under distinct stable
//! identities interleave by the stable-key tiebreak, the frozen
//! deterministic resolution.
//!
//! Failure policy: the function is pure — it owns no state, and every
//! rejection is typed (`StaleEpoch` for a part, header or attribution from
//! another epoch, `InvalidInput` for incoherent identity, revision,
//! operation tag, token or outcome agreement, and an ambiguous order slot,
//! `Capacity` for the full-family record count or owned-byte bound) and
//! returns before any record is emitted, so the caller retains its previous
//! output unchanged. The family byte bound is measured through the accepted
//! frame accounting over a minimal inventory-ui-only frame, never a second
//! accounting. The `OrderedRecord` envelope is stripped only after every
//! check that inspects the envelopes passes — the family byte bound is
//! measured over the stripped records in the candidate frame — and the
//! wrapped records are emitted unchanged.

use crate::contracts::{
    ClientError, ClientLimits, ConfirmedRevision, FAMILY_INVENTORY_UI, FamilyKey, FamilyOperation,
    SessionEpoch,
};
use crate::presentation::frame::{
    FamilyFrame, FamilyRecords, InventoryUiRecord, PresentationFrame,
};
use crate::presentation::{
    InventoryTopic, InventoryUiView, OrderedRecord, ProjectionOrder, StableRecordKey, UiOutcome,
};

/// Assembles the four checked per-topic provider vectors into the complete
/// inventory UI family record vector, in retained source order.
///
/// The parts array is the packet's provider order (inventory, container,
/// crafting, furnace); the emitted interleaving follows the retained order
/// keys alone. Validation covers the envelope/record identity agreement
/// (topic tag and actual container identity), the operation tag of each
/// view, the token and outcome agreement of the union, the frame epoch and
/// candidate revision coherence of every header and order key, the absence
/// of ambiguous duplicate order slots, and the full-family record count and
/// owned-byte bounds.
pub fn assemble_inventory_ui(
    epoch: SessionEpoch,
    revision: ConfirmedRevision,
    parts: [Vec<OrderedRecord<InventoryUiRecord>>; 4],
    limits: &ClientLimits,
) -> Result<Vec<InventoryUiRecord>, ClientError> {
    let mut merged: Vec<OrderedRecord<InventoryUiRecord>> = parts.into_iter().flatten().collect();

    // Identity, attribution and frame coherence: the envelope's stable key
    // must be exactly the key derived from the record it wraps, the record's
    // operation tag must match its view, the token and outcome union must
    // agree with the shapes the accepted providers emit, and every header and
    // retained order key must carry exactly this frame's epoch and candidate
    // revision.
    let mut slots: Vec<(ProjectionOrder, StableRecordKey)> = Vec::with_capacity(merged.len());
    for entry in &merged {
        let record = entry.record();
        // The per-view contract row: the derived topic tag and identity, the
        // operation tag the view publishes under, the container reference a
        // token must name, and the outcome the payload itself carries.
        let (topic, identity, operation, attributed, outcome) = match record.view() {
            InventoryUiView::Inventory(_) => (
                InventoryTopic::Inventory,
                None,
                FamilyOperation::Upsert,
                None,
                None,
            ),
            InventoryUiView::Crafting(_) => (
                InventoryTopic::Crafting,
                None,
                FamilyOperation::Upsert,
                None,
                None,
            ),
            InventoryUiView::Furnace(state) => (
                InventoryTopic::Furnace,
                Some(state.container()),
                FamilyOperation::Upsert,
                Some(state.container()),
                None,
            ),
            InventoryUiView::Chest(state) => (
                InventoryTopic::Chest,
                Some(state.container()),
                FamilyOperation::Upsert,
                Some(state.container()),
                None,
            ),
            InventoryUiView::Closed(reference) => (
                InventoryTopic::Closed,
                Some(*reference),
                FamilyOperation::Remove,
                None,
                None,
            ),
            InventoryUiView::Rejected(rejection) => (
                InventoryTopic::Rejected,
                None,
                FamilyOperation::Upsert,
                None,
                Some(UiOutcome::Rejected {
                    sequence: rejection.sequence(),
                    reason: rejection.reason(),
                }),
            ),
        };
        let derived = StableRecordKey::Inventory { topic, identity };
        if *entry.stable_key() != derived {
            return Err(ClientError::InvalidInput);
        }
        if record.header().operation() != operation {
            return Err(ClientError::InvalidInput);
        }
        if record.outcome() != outcome.as_ref() {
            return Err(ClientError::InvalidInput);
        }

        // The source revision the retained order key names: the revision its
        // own observation committed at, which is also the revision a staged
        // view's attribution must name. An order key from another epoch never
        // enters this frame.
        let source_revision = match entry.order() {
            ProjectionOrder::Confirmed { observation, .. } => {
                if observation.epoch() != epoch {
                    return Err(ClientError::StaleEpoch);
                }
                observation.confirmed_revision()
            }
            ProjectionOrder::AfterConfirmed {
                epoch: order_epoch,
                revision: sampled,
                ..
            } => {
                if *order_epoch != epoch {
                    return Err(ClientError::StaleEpoch);
                }
                *sampled
            }
        };

        // The token agreement: an external view carries exactly its own local
        // attribution — this epoch, its own reference, its own observation's
        // staged revision — and nothing else may carry one, because a
        // singleton, a retired view and a rejection have no container
        // attribution to project.
        match attributed {
            Some(reference) => {
                let token = record.token().ok_or(ClientError::InvalidInput)?;
                if token.epoch() != epoch {
                    return Err(ClientError::StaleEpoch);
                }
                if token.reference() != reference {
                    return Err(ClientError::InvalidInput);
                }
                if token.confirmed_revision() != source_revision {
                    return Err(ClientError::InvalidInput);
                }
            }
            None => {
                if record.token().is_some() {
                    return Err(ClientError::InvalidInput);
                }
            }
        }

        if record.header().epoch() != epoch {
            return Err(ClientError::StaleEpoch);
        }
        if record.header().revision() != revision {
            return Err(ClientError::InvalidInput);
        }
        slots.push((*entry.order(), *entry.stable_key()));
    }

    // Ambiguous ordering: two envelopes claiming the same retained order
    // slot for the same stable identity are indistinguishable source
    // observations, so the whole family rejects instead of letting the parts
    // order silently decide.
    slots.sort();
    for pair in slots.windows(2) {
        if pair[0] == pair[1] {
            return Err(ClientError::InvalidInput);
        }
    }

    // The frozen merge: retained order keys interleave the four parts by
    // actual source order, with the stable key as the final tiebreak. Equal
    // or absent source ticks play no part in this comparison.
    merged.sort_by(|left, right| {
        left.order()
            .cmp(right.order())
            .then_with(|| left.stable_key().cmp(right.stable_key()))
    });

    // The full-family bounds: the record count against the frozen per-family
    // cap, then the owned bytes through the accepted frame accounting over a
    // minimal inventory-ui-only frame. Both reject before any record is
    // emitted, so the caller's retained previous output survives an
    // over-limit assembly attempt unchanged.
    if merged.len() > limits.family_records() {
        return Err(ClientError::Capacity);
    }
    let records: Vec<InventoryUiRecord> = merged
        .into_iter()
        .map(|entry| entry.into_record())
        .collect();
    let candidate = PresentationFrame::try_new(
        epoch,
        revision,
        0,
        vec![FamilyFrame::try_new(
            FamilyKey::try_new(FAMILY_INVENTORY_UI)?,
            FamilyRecords::InventoryUi(records.clone()),
        )?],
    )?;
    if candidate.validated_size()? > limits.frame_bytes() {
        return Err(ClientError::Capacity);
    }
    Ok(records)
}
