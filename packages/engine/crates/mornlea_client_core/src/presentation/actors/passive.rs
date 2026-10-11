//! The passive-mob actor projection.
//!
//! This provider turns the passive publications of the committed observation
//! queue into ordered `actors@1` records: a spawn batch is one upsert per
//! record in published packet order carrying the typed nonzero identity, the
//! actual spawn dimension, the exact finite pose, the spawn health and no
//! transient fields; a state batch is one upsert per record carrying the
//! pose, the per-tick velocity, the health and the transient grazing bit,
//! with the dimension resolved from the prevalidated identity resolution the
//! confirmed mirror recorded; a despawn batch is one remove-only record per
//! entry whose only surviving payload is the exact vanished/died reason from
//! the closed wire union. The same identity may spawn again after its
//! removal, so repeated stable keys across observations are the legal reuse
//! lifecycle, not a duplicate.
//!
//! Ownership boundaries: the projection is pure over the immutable
//! `ProjectionView` and owns no second mirror, no packet history and no
//! presentation state. A spawn carries no pitch, no velocity and no grazing
//! or despawn reason because the wire record carries none; a state record
//! carries no despawn reason; and no lure, drop, attack, hit or death
//! attribution beyond the wire's own reason byte is ever invented here, so
//! foreign-shaped observations of other kinds are ignored wholesale: they
//! belong to their own per-kind providers. A malformed observation identity
//! — a dimensionless state or despawn record whose prevalidated resolution
//! is missing, ambiguous or names another identity, a packet that is not a
//! checked publication, or an observation from another epoch — rejects the
//! whole projection before any record is returned, so no partial output
//! exists; the confirmed mirror and the serial family assembler remain the
//! identity and publication owners.

use mornlea_domain::{Dimension, Event, PassiveId};

use crate::contracts::{ClientError, FamilyOperation, RecordHeader};
use crate::presentation::frame::{ActorDetail, ActorDimension, ActorId, ActorKind, ActorRecord};
use crate::presentation::{
    AcceptedObservation, OrderedRecord, ProjectionOrder, ProjectionView, StableRecordKey,
};

/// Widens one finite f32 publication vector exactly; f32-to-f64 conversion is
/// exact, so no world coordinate or per-tick delta is rounded or invented.
fn widened(components: [f32; 3]) -> [f64; 3] {
    [
        f64::from(components[0]),
        f64::from(components[1]),
        f64::from(components[2]),
    ]
}

/// The stable identity of one passive record: kind, actual dimension and the
/// typed nonzero identity, never a numeric conversion of the identity.
fn stable_key(id: PassiveId, dimension: Dimension) -> StableRecordKey {
    StableRecordKey::Actor {
        kind: ActorKind::Passive,
        dimension: ActorDimension::Known(dimension),
        id: ActorId::Passive(id),
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

/// Resolves the confirmed dimension of one dimensionless state or despawn
/// record against the observation's prevalidated identity resolution.
///
/// Exactly one matching live resolution must name the passive identity:
/// none is an orphan, more than one is ambiguous, and both reject the whole
/// projection instead of guessing a dimension the mirror never confirmed.
fn resolved_dimension(
    observation: &AcceptedObservation,
    id: PassiveId,
) -> Result<Dimension, ClientError> {
    let wanted = ActorId::Passive(id);
    let mut matches = observation
        .resolved()
        .iter()
        .filter(|resolved| *resolved.id() == wanted);
    let dimension = matches.next().ok_or(ClientError::InvalidInput)?.dimension();
    if matches.next().is_some() {
        return Err(ClientError::InvalidInput);
    }
    Ok(dimension)
}

/// The checked packet record ordinal of one batch entry; a checked
/// conversion never truncates silently.
fn ordinal(index: usize) -> Result<u32, ClientError> {
    u32::try_from(index).map_err(|_| ClientError::Capacity)
}

/// Projects the passive records of one immutable view, in actual source
/// order: observation key first, then the packet record ordinal.
///
/// Order is never reconstructed from the optional source tick — equal,
/// absent or backwards ticks keep the observation and packet order — nor
/// from the rebased parent revision. The serial actor family assembler
/// consumes these envelopes, validates remove-before-reuse and strips the
/// envelope when the unchanged record vector is emitted.
pub fn project_passive(
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
            Event::PassiveSpawn(spawn) => {
                for (index, record) in spawn.spawns().iter().enumerate() {
                    let record_ordinal = ordinal(index)?;
                    let entry = ActorRecord::try_new(
                        header(view, observation.source_tick(), FamilyOperation::Upsert)?,
                        ActorKind::Passive,
                        ActorId::Passive(record.id()),
                        ActorDimension::Known(record.dimension()),
                        Some(widened(record.position().get())),
                        Some(f64::from(record.yaw())),
                        // The passive spawn publication carries no pitch and
                        // no velocity.
                        None,
                        None,
                        Some(ActorDetail::Passive {
                            health: Some(record.health()),
                            // Grazing is a transient state observation; a
                            // new body has none yet.
                            grazing: None,
                            despawn_reason: None,
                        }),
                    )?;
                    records.push(OrderedRecord::try_new(
                        ProjectionOrder::Confirmed {
                            observation: key,
                            record_ordinal,
                        },
                        stable_key(record.id(), record.dimension()),
                        entry,
                    )?);
                }
            }
            Event::PassiveState(states) => {
                for (index, record) in states.states().iter().enumerate() {
                    let record_ordinal = ordinal(index)?;
                    let dimension = resolved_dimension(observation, record.id())?;
                    let entry = ActorRecord::try_new(
                        header(view, observation.source_tick(), FamilyOperation::Upsert)?,
                        ActorKind::Passive,
                        ActorId::Passive(record.id()),
                        ActorDimension::Known(dimension),
                        Some(widened(record.position().get())),
                        Some(f64::from(record.yaw())),
                        // The passive publication carries no pitch.
                        None,
                        Some(widened(record.velocity().get())),
                        Some(ActorDetail::Passive {
                            health: Some(record.health()),
                            // The grazing bit is the wire's exact 0/1 value.
                            grazing: Some(u8::from(record.grazing())),
                            despawn_reason: None,
                        }),
                    )?;
                    records.push(OrderedRecord::try_new(
                        ProjectionOrder::Confirmed {
                            observation: key,
                            record_ordinal,
                        },
                        stable_key(record.id(), dimension),
                        entry,
                    )?);
                }
            }
            Event::PassiveDespawn(despawn) => {
                for (index, record) in despawn.despawns().iter().enumerate() {
                    let record_ordinal = ordinal(index)?;
                    let dimension = resolved_dimension(observation, record.id())?;
                    // A despawn is a removal beside its reason: no pose, no
                    // angle, no velocity, no health and no grazing survive
                    // the identity, and the reason is the wire's own closed
                    // variant, never an inference.
                    let entry = ActorRecord::try_new(
                        header(view, observation.source_tick(), FamilyOperation::Remove)?,
                        ActorKind::Passive,
                        ActorId::Passive(record.id()),
                        ActorDimension::Known(dimension),
                        None,
                        None,
                        None,
                        None,
                        Some(ActorDetail::Passive {
                            health: None,
                            grazing: None,
                            despawn_reason: Some(record.reason()),
                        }),
                    )?;
                    records.push(OrderedRecord::try_new(
                        ProjectionOrder::Confirmed {
                            observation: key,
                            record_ordinal,
                        },
                        stable_key(record.id(), dimension),
                        entry,
                    )?);
                }
            }
            // Every other publication belongs to another per-kind provider.
            _ => {}
        }
    }
    Ok(records)
}
