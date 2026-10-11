//! The item-drop actor projection.
//!
//! This provider turns the item-drop publications of the committed
//! observation queue into ordered `actors@1` records: an upserts batch is one
//! upsert per drop in published packet order carrying the typed identity with
//! its raw dimension, chunk, slot and generation preserved exactly, the exact
//! finite world position the accepted geometry helper resolves from the
//! identity's chunk and the record's chunk-local block index, and the exact
//! item/count/durability stack detail — an upsert adds or wholly replaces one
//! stack by identity, so the same identity may be republished with a smaller
//! count after a partial pickup and still stays exactly once in the mirror's
//! identity store; a removes batch is one remove-only record per identity —
//! no position, no stack and no cause, because the publication names none —
//! and the same identity may be upserted again after its removal, so repeated
//! stable keys across observations are the legal reuse lifecycle, not a
//! duplicate. The raw drop dimension is never coerced into a known world: it
//! stays the open value the identity carried, and the identity resolution
//! the mirror records for dimensionless kinds is never consulted because the
//! drop identity names its own dimension.
//!
//! Ownership boundaries: the projection is pure over the immutable
//! `ProjectionView` and owns no second mirror, no packet history and no
//! drop lifecycle simulation — no pickup timer, despawn countdown or
//! velocity exists on the wire record, so none is invented here. A
//! `CombatHit` publication names a target category alone, so it is ignored
//! wholesale — no pickup attribution is drawn from it; observations of other
//! kinds belong to their own per-kind providers. A packet that is not a
//! checked publication, or an observation from another epoch, rejects the
//! whole projection before any record is returned, so no partial output
//! exists; the confirmed mirror and the serial family assembler remain the
//! identity and publication owners.

use mornlea_domain::{DropId, Event};

use crate::contracts::{ClientError, FamilyOperation, RecordHeader};
use crate::presentation::frame::{ActorDetail, ActorDimension, ActorId, ActorKind, ActorRecord};
use crate::presentation::geometry::drop_position;
use crate::presentation::{OrderedRecord, ProjectionOrder, ProjectionView, StableRecordKey};

/// The stable identity of one drop record: kind, the raw dimension of the
/// identity and the typed identity itself, never a normalization of the raw
/// dimension into a known world.
fn stable_key(id: DropId) -> StableRecordKey {
    StableRecordKey::Actor {
        kind: ActorKind::Drop,
        dimension: ActorDimension::DropRaw(id.dimension()),
        id: ActorId::Drop(id),
    }
}

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

/// The checked packet record ordinal of one batch entry; a checked
/// conversion never truncates silently.
fn ordinal(index: usize) -> Result<u32, ClientError> {
    u32::try_from(index).map_err(|_| ClientError::Capacity)
}

/// Projects the drop records of one immutable view, in actual source order:
/// observation key first, then the packet record ordinal.
///
/// Order is never reconstructed from the optional source tick — equal,
/// absent or backwards ticks keep the observation and packet order — nor
/// from the rebased parent revision. The serial actor family assembler
/// consumes these envelopes, validates remove-before-reuse and strips the
/// envelope when the unchanged record vector is emitted.
pub fn project_drop(
    view: &ProjectionView<'_>,
) -> Result<Vec<OrderedRecord<ActorRecord>>, ClientError> {
    let mut records: Vec<OrderedRecord<ActorRecord>> = Vec::new();
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
            Event::ItemDropUpserts(upserts) => {
                for (index, drop) in upserts.drops().iter().enumerate() {
                    let record_ordinal = ordinal(index)?;
                    let id = drop.id();
                    // The world position is the geometry helper's exact value
                    // of the identity's chunk and this record's block index;
                    // no velocity and no angle exists on a drop publication.
                    let entry = ActorRecord::try_new(
                        header(view, observation.source_tick(), FamilyOperation::Upsert)?,
                        ActorKind::Drop,
                        ActorId::Drop(id),
                        ActorDimension::DropRaw(id.dimension()),
                        Some(drop_position(id, drop.block_index())?),
                        None,
                        None,
                        None,
                        Some(ActorDetail::Drop {
                            block_index: drop.block_index(),
                            stack: drop.stack(),
                        }),
                    )?;
                    records.push(OrderedRecord::try_new(
                        ProjectionOrder::Confirmed {
                            observation: key,
                            record_ordinal,
                        },
                        stable_key(id),
                        entry,
                    )?);
                }
            }
            Event::ItemDropRemoves(removes) => {
                for (index, id) in removes.ids().iter().enumerate() {
                    let record_ordinal = ordinal(index)?;
                    // A removal is a removal alone: the publication names the
                    // identity and nothing else, so no cell, no stack and no
                    // cause survives it.
                    let entry = ActorRecord::try_new(
                        header(view, observation.source_tick(), FamilyOperation::Remove)?,
                        ActorKind::Drop,
                        ActorId::Drop(*id),
                        ActorDimension::DropRaw(id.dimension()),
                        None,
                        None,
                        None,
                        None,
                        None,
                    )?;
                    records.push(OrderedRecord::try_new(
                        ProjectionOrder::Confirmed {
                            observation: key,
                            record_ordinal,
                        },
                        stable_key(*id),
                        entry,
                    )?);
                }
            }
            // Every other publication belongs to another per-kind provider.
            // A CombatHit names a target category alone, so no pickup or
            // despawn attribution is ever drawn from it.
            _ => {}
        }
    }
    Ok(records)
}
