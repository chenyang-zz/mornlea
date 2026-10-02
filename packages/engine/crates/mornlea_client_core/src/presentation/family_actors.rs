//! The actor family serial assembler: the merge of the six accepted
//! per-kind provider vectors into one coherent `actors@1` record vector.
//!
//! The six providers each publish a checked vector of source-ordered
//! envelopes for their own kind; this assembler is the single serial owner
//! that combines them. The parts array follows the packet's provider order,
//! but that order carries no semantics here: retained `ProjectionOrder`
//! keys — the actual observation revision and ordinal, then the packet
//! record ordinal — interleave the kinds by true source order, with the
//! stable key as the final tiebreak, so equal or absent source ticks never
//! reconstruct order and the output is never batched by topic.
//!
//! One stable identity legally carries several records: a spawn, its state
//! samples, its removal and a later reuse spawn are distinct source
//! observations of one actor, not duplicates. Validation therefore walks the
//! merged retained order per identity instead of rejecting repeated keys: an
//! upsert publishes or republishes a live identity, a removal is legal only
//! while its identity is live, and an orphan removal, a duplicated removal
//! or two envelopes claiming the same retained order slot for one identity
//! are malformed and reject the whole family.
//!
//! Failure policy: the function is pure — it owns no state, and every
//! rejection is typed (`StaleEpoch` for a part from another epoch,
//! `InvalidInput` for incoherent identity, revision, slot or reuse order,
//! `Capacity` for the full-family record count or owned-byte bound) and
//! returns before any record is emitted, so the caller retains its previous
//! output unchanged. The family byte bound is measured through the accepted
//! frame accounting over a minimal actors-only frame, never a second
//! accounting. The `OrderedRecord` envelope is stripped only after every
//! check that inspects the envelopes passes — the family byte bound is
//! measured over the stripped records in the candidate frame — and the
//! wrapped records are emitted unchanged.

use crate::contracts::{
    ClientError, ClientLimits, ConfirmedRevision, FAMILY_ACTORS, FamilyKey, FamilyOperation,
    SessionEpoch,
};
use crate::presentation::frame::{ActorRecord, FamilyFrame, FamilyRecords, PresentationFrame};
use crate::presentation::{OrderedRecord, ProjectionOrder, StableRecordKey};

/// Assembles the six checked per-kind provider vectors into the complete
/// actor family record vector, in retained source order.
///
/// The parts array is the packet's provider order (remote player, hostile,
/// passive, projectile, companion, drop); the emitted interleaving follows
/// the retained order keys alone. Validation covers the envelope/record
/// identity agreement (kind, typed id and dimension), the frame epoch and
/// candidate revision coherence of every header and order key, the absence
/// of ambiguous duplicate order slots, remove-before-reuse against the
/// retained order, and the full-family record count and owned-byte bounds.
pub fn assemble_actors(
    epoch: SessionEpoch,
    revision: ConfirmedRevision,
    parts: [Vec<OrderedRecord<ActorRecord>>; 6],
    limits: &ClientLimits,
) -> Result<Vec<ActorRecord>, ClientError> {
    let mut merged: Vec<OrderedRecord<ActorRecord>> = parts.into_iter().flatten().collect();

    // Identity and frame coherence: the envelope's stable key must be exactly
    // the key derived from the record it wraps, the record's kind and typed
    // identity tags must agree, and every header and retained order key must
    // carry exactly this frame's epoch and candidate revision.
    let mut slots: Vec<(ProjectionOrder, StableRecordKey)> = Vec::with_capacity(merged.len());
    for entry in &merged {
        let record = entry.record();
        if record.id().kind() != record.kind() {
            return Err(ClientError::InvalidInput);
        }
        let derived = StableRecordKey::Actor {
            kind: record.kind(),
            dimension: *record.dimension(),
            id: *record.id(),
        };
        if *entry.stable_key() != derived {
            return Err(ClientError::InvalidInput);
        }
        if record.header().epoch() != epoch {
            return Err(ClientError::StaleEpoch);
        }
        if record.header().revision() != revision {
            return Err(ClientError::InvalidInput);
        }
        let order_epoch = match entry.order() {
            ProjectionOrder::Confirmed { observation, .. } => observation.epoch(),
            ProjectionOrder::AfterConfirmed { epoch, .. } => *epoch,
        };
        if order_epoch != epoch {
            return Err(ClientError::StaleEpoch);
        }
        slots.push((*entry.order(), *entry.stable_key()));
    }

    // Ambiguous ordering: two envelopes claiming the same retained order
    // slot for the same stable identity are indistinguishable source
    // observations, so the whole family rejects instead of guessing.
    slots.sort();
    for pair in slots.windows(2) {
        if pair[0] == pair[1] {
            return Err(ClientError::InvalidInput);
        }
    }

    // The frozen merge: retained order keys interleave the six parts by
    // actual source order, with the stable key as the final tiebreak. Equal
    // or absent source ticks play no part in this comparison.
    merged.sort_by(|left, right| {
        left.order()
            .cmp(right.order())
            .then_with(|| left.stable_key().cmp(right.stable_key()))
    });

    // Remove-before-reuse against the retained order: repeated upserts of a
    // live identity are legal state samples and stack replacements, a
    // removal retires exactly a live identity, and a reuse upsert is legal
    // only because its removal came first.
    let mut live: Vec<StableRecordKey> = Vec::new();
    for entry in &merged {
        match entry.record().header().operation() {
            FamilyOperation::Upsert => {
                if !live.contains(entry.stable_key()) {
                    live.push(*entry.stable_key());
                }
            }
            FamilyOperation::Remove => {
                let Some(index) = live.iter().position(|key| key == entry.stable_key()) else {
                    return Err(ClientError::InvalidInput);
                };
                live.remove(index);
            }
        }
    }

    // The full-family bounds: the record count against the frozen per-family
    // cap, then the owned bytes through the accepted frame accounting over a
    // minimal actors-only frame. Both reject before any record is emitted,
    // so the caller's retained previous output survives an over-limit
    // assembly attempt unchanged.
    if merged.len() > limits.family_records() {
        return Err(ClientError::Capacity);
    }
    let records: Vec<ActorRecord> = merged
        .into_iter()
        .map(|entry| entry.into_record())
        .collect();
    let candidate = PresentationFrame::try_new(
        epoch,
        revision,
        0,
        vec![FamilyFrame::try_new(
            FamilyKey::try_new(FAMILY_ACTORS)?,
            FamilyRecords::Actors(records.clone()),
        )?],
    )?;
    if candidate.validated_size()? > limits.frame_bytes() {
        return Err(ClientError::Capacity);
    }
    Ok(records)
}
