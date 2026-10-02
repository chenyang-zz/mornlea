//! The world UI family serial assembler: the merge of the five accepted
//! per-topic provider vectors into one coherent `world-ui@1` record
//! vector.
//!
//! The five providers each publish a checked vector of source-ordered
//! envelopes for their own topic; this assembler is the single serial owner
//! that combines them. The parts array follows the packet's provider order
//! (environment, survival, chat, task, prompt), but that order carries no
//! semantics here: retained `ProjectionOrder` keys — the actual observation
//! revision and ordinal, then the packet record ordinal — interleave the
//! topics by true source order, with the stable key as the final tiebreak,
//! so equal or absent source ticks never reconstruct order and the output is
//! never batched by topic. Weather, task and chat facts therefore assemble
//! together in exactly the order the authority published them, and one task
//! publication's chat and task records share one order slot under distinct
//! stable identities, resolved by the stable-key tiebreak.
//!
//! Identity and attribution: the envelope's stable key must be exactly the
//! key derived from the record it wraps. The environment, survival and
//! prompt topics are singleton current-state topics whose identity is the
//! tag itself; a chat or task record names the actual observation identity
//! of the carrying publication, which only a confirmed order key retains —
//! a derived chat or task envelope has no carrying publication to name, so
//! admitting one would fabricate the identity and it is incoherent. Every
//! world record the accepted providers emit is an upsert of a complete
//! checked value, so a record under a removal tag is malformed; the record
//! carries no token or outcome field, so the union of the parts survives
//! unchanged.
//!
//! Ordering tolerance: repeated stable keys across observations are the
//! legal replace and latest-wins lifecycles of the singleton current-state
//! topics — each publication restates the complete value and the retained
//! source order resolves them, a schema binding this assembler records
//! rather than a finer identity it invents. Two envelopes claiming the same
//! retained order slot for one stable identity are indistinguishable source
//! observations and reject the whole family; two envelopes at one order
//! slot under distinct stable identities interleave by the stable-key
//! tiebreak, the frozen deterministic resolution. Distinct identities never
//! reject: a chat duplicate resent after its original observation was
//! consumed enters as a fresh record under its new identity, and the
//! assembler keeps no cross-frame memory that could treat it as malformed.
//!
//! Bounded inputs: the chat provider exposes only its most-recent confirmed
//! window (the pilot chat ring's 32) and this assembler merges that bounded
//! output exactly as emitted — it never re-derives, extends or re-filters
//! it. The family is pure per frame: it reflects the retained-observation
//! window of the sampled revision alone, and whether the presentation
//! consumer keeps the last window visible across empty frames is that
//! consumer's concern, never state this assembler owns.
//!
//! Failure policy: the function is pure — it owns no state, and every
//! rejection is typed (`StaleEpoch` for a part header or order key from
//! another epoch, `InvalidInput` for incoherent identity, operation tag or
//! rebased revision and for an ambiguous order slot, `Capacity` for the
//! full-family record count or owned-byte bound) and returns before any
//! record is emitted, so the caller retains its previous output unchanged.
//! The family byte bound is measured through the accepted frame accounting
//! over a minimal world-ui-only frame, never a second accounting. The
//! `OrderedRecord` envelope is stripped only after every check that
//! inspects the envelopes passes — the family byte bound is measured over
//! the stripped records in the candidate frame — and the wrapped records
//! are emitted unchanged.

use crate::contracts::{
    ClientError, ClientLimits, ConfirmedRevision, FAMILY_WORLD_UI, FamilyKey, FamilyOperation,
    SessionEpoch,
};
use crate::presentation::frame::{FamilyFrame, FamilyRecords, PresentationFrame, WorldUiRecord};
use crate::presentation::{
    OrderedRecord, ProjectionOrder, StableRecordKey, WorldTopic, WorldUiView,
};

/// Assembles the five checked per-topic provider vectors into the complete
/// world UI family record vector, in retained source order.
///
/// The parts array is the packet's provider order (environment, survival,
/// chat, task, prompt); the emitted interleaving follows the retained order
/// keys alone. Validation covers the envelope/record identity agreement
/// (topic tag and the actual observation identity a chat or task record
/// names), the upsert operation tag of every view, the frame epoch and
/// candidate revision coherence of every header and order key, the absence
/// of ambiguous duplicate order slots, and the full-family record count and
/// owned-byte bounds.
pub fn assemble_world_ui(
    epoch: SessionEpoch,
    revision: ConfirmedRevision,
    parts: [Vec<OrderedRecord<WorldUiRecord>>; 5],
    limits: &ClientLimits,
) -> Result<Vec<WorldUiRecord>, ClientError> {
    let mut merged: Vec<OrderedRecord<WorldUiRecord>> = parts.into_iter().flatten().collect();

    // Identity and frame coherence: the envelope's stable key must be exactly
    // the key derived from the record it wraps, the record's operation tag
    // must be the upsert every accepted provider emits, and every header and
    // retained order key must carry exactly this frame's epoch and candidate
    // revision.
    let mut slots: Vec<(ProjectionOrder, StableRecordKey)> = Vec::with_capacity(merged.len());
    for entry in &merged {
        let record = entry.record();
        let topic = match record.view() {
            WorldUiView::Environment(_) => WorldTopic::Environment,
            WorldUiView::Survival(_) => WorldTopic::Survival,
            WorldUiView::Chat(_) => WorldTopic::Chat,
            WorldUiView::Task(_) => WorldTopic::Task,
            WorldUiView::Prompt(_) => WorldTopic::Prompt,
        };
        // The stable identity: a chat or task record names the observation of
        // its carrying publication, which only a confirmed order key retains;
        // the singleton topics name no identity, and none is fabricated.
        let identity = match (topic, entry.order()) {
            (
                WorldTopic::Chat | WorldTopic::Task,
                ProjectionOrder::Confirmed { observation, .. },
            ) => Some(*observation),
            (WorldTopic::Chat | WorldTopic::Task, ProjectionOrder::AfterConfirmed { .. }) => {
                return Err(ClientError::InvalidInput);
            }
            _ => None,
        };
        let derived = StableRecordKey::World { topic, identity };
        if *entry.stable_key() != derived {
            return Err(ClientError::InvalidInput);
        }
        if record.header().operation() != FamilyOperation::Upsert {
            return Err(ClientError::InvalidInput);
        }

        // An order key from another epoch never enters this frame, and every
        // header is rebased onto exactly this frame's coherent candidate
        // revision.
        match entry.order() {
            ProjectionOrder::Confirmed { observation, .. } => {
                if observation.epoch() != epoch {
                    return Err(ClientError::StaleEpoch);
                }
            }
            ProjectionOrder::AfterConfirmed {
                epoch: order_epoch, ..
            } => {
                if *order_epoch != epoch {
                    return Err(ClientError::StaleEpoch);
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

    // Ambiguous ordering: two envelopes claiming the same retained order slot
    // for the same stable identity are indistinguishable source observations,
    // so the whole family rejects instead of letting the parts order silently
    // decide. Distinct stable identities never reject: they interleave by the
    // stable-key tiebreak below.
    slots.sort();
    for pair in slots.windows(2) {
        if pair[0] == pair[1] {
            return Err(ClientError::InvalidInput);
        }
    }

    // The frozen merge: retained order keys interleave the five parts by
    // actual source order, with the stable key as the final tiebreak. Equal
    // or absent source ticks play no part in this comparison.
    merged.sort_by(|left, right| {
        left.order()
            .cmp(right.order())
            .then_with(|| left.stable_key().cmp(right.stable_key()))
    });

    // The full-family bounds: the record count against the frozen per-family
    // cap, then the owned bytes through the accepted frame accounting over a
    // minimal world-ui-only frame. Both reject before any record is emitted,
    // so the caller's retained previous output survives an over-limit
    // assembly attempt unchanged.
    if merged.len() > limits.family_records() {
        return Err(ClientError::Capacity);
    }
    let records: Vec<WorldUiRecord> = merged
        .into_iter()
        .map(|entry| entry.into_record())
        .collect();
    let candidate = PresentationFrame::try_new(
        epoch,
        revision,
        0,
        vec![FamilyFrame::try_new(
            FamilyKey::try_new(FAMILY_WORLD_UI)?,
            FamilyRecords::WorldUi(records.clone()),
        )?],
    )?;
    if candidate.validated_size()? > limits.frame_bytes() {
        return Err(ClientError::Capacity);
    }
    Ok(records)
}
