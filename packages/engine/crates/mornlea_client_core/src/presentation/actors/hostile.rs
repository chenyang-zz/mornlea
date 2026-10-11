//! The hostile actor projection.
//!
//! This provider turns the hostile publications of the committed observation
//! queue into ordered `actors@1` records: a spawn batch is one upsert per
//! record in published packet order carrying the typed identity, the actual
//! spawn dimension, the exact finite position, the yaw alone — the hostile
//! publication carries no pitch, so no record ever publishes one — and the
//! archetype and health detail, with no velocity; a state batch is one upsert
//! per record advancing the position, the velocity, the yaw, the archetype
//! and the health together, its dimension resolved from the prevalidated
//! identity resolution the confirmed mirror recorded because the state packet
//! itself carries none; a despawn batch is one remove-only record per
//! identity, again with the dimension from the identity resolution. The same
//! identity may spawn again after its removal, so repeated stable keys across
//! observations are the legal reuse lifecycle, not a duplicate.
//!
//! Ownership boundaries: the projection is pure over the immutable
//! `ProjectionView` and owns no second mirror, no packet history and no
//! simulation state. A `CombatHit` publication names a target category, never
//! a hostile identity, so it is ignored wholesale here — no attacker
//! association, hit marker or death attribution is invented, and a hostile's
//! health comes from the hostile publications alone. Observations of other
//! kinds belong to their own per-kind providers. A malformed observation
//! identity — a dimensionless state or despawn whose prevalidated resolution
//! is missing, ambiguous or names another identity, a packet that is not a
//! checked publication, or an observation from another epoch — rejects the
//! whole projection before any record is returned, so no partial output
//! exists; the confirmed mirror and the serial family assembler remain the
//! identity and publication owners.

use mornlea_domain::{Dimension, Event, HostileId, HostileKind};

use mornlea_protocol::{HOSTILE_KIND_BONE_THROWER, HOSTILE_KIND_NIGHTWALKER};

use crate::contracts::{ClientError, FamilyOperation, RecordHeader};
use crate::presentation::frame::{ActorDetail, ActorDimension, ActorId, ActorKind, ActorRecord};
use crate::presentation::{
    AcceptedObservation, OrderedRecord, ProjectionOrder, ProjectionView, StableRecordKey,
};

/// Widens one finite f32 publication vector exactly; f32-to-f64 conversion
/// is exact, so no world coordinate or per-tick delta is rounded or invented.
fn widened(components: [f32; 3]) -> [f64; 3] {
    [
        f64::from(components[0]),
        f64::from(components[1]),
        f64::from(components[2]),
    ]
}

/// The wire archetype byte of the closed domain kind, preserving the accepted
/// kind values instead of inventing a second mapping.
fn archetype(kind: HostileKind) -> u8 {
    match kind {
        HostileKind::Nightwalker => HOSTILE_KIND_NIGHTWALKER,
        HostileKind::BoneThrower => HOSTILE_KIND_BONE_THROWER,
    }
}

/// The stable identity of one hostile record: kind, actual dimension and the
/// typed identity, never a numeric conversion of the identity.
fn stable_key(id: HostileId, dimension: Dimension) -> StableRecordKey {
    StableRecordKey::Actor {
        kind: ActorKind::Hostile,
        dimension: ActorDimension::Known(dimension),
        id: ActorId::Hostile(id),
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
/// Exactly one matching live resolution must name the record's identity:
/// none is an orphan, more than one is ambiguous, and both reject the whole
/// projection instead of guessing a dimension the mirror never confirmed.
fn resolved_dimension(
    observation: &AcceptedObservation,
    id: HostileId,
) -> Result<Dimension, ClientError> {
    let wanted = ActorId::Hostile(id);
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

/// Projects the hostile records of one immutable view, in actual source
/// order: observation key first, then the packet record ordinal.
///
/// Order is never reconstructed from the optional source tick — equal or
/// absent ticks keep the observation and packet order — nor from the rebased
/// parent revision. The serial actor family assembler consumes these
/// envelopes, validates remove-before-reuse and strips the envelope when the
/// unchanged record vector is emitted.
pub fn project_hostile(
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
            Event::HostileSpawn(spawn) => {
                for (ordinal, record) in spawn.spawns().iter().enumerate() {
                    // A packet record ordinal beyond u32 cannot occur for the
                    // accepted batch cap, but a checked conversion never
                    // truncates silently.
                    let record_ordinal =
                        u32::try_from(ordinal).map_err(|_| ClientError::Capacity)?;
                    let actor = ActorRecord::try_new(
                        header(view, observation.source_tick(), FamilyOperation::Upsert)?,
                        ActorKind::Hostile,
                        ActorId::Hostile(record.id()),
                        ActorDimension::Known(record.dimension()),
                        Some(widened(record.position().get())),
                        Some(f64::from(record.yaw())),
                        // The spawn publication carries yaw alone: no pitch
                        // exists on any hostile record to publish.
                        None,
                        None,
                        Some(ActorDetail::Hostile {
                            archetype: archetype(record.kind()),
                            health: record.health(),
                        }),
                    )?;
                    records.push(OrderedRecord::try_new(
                        ProjectionOrder::Confirmed {
                            observation: key,
                            record_ordinal,
                        },
                        stable_key(record.id(), record.dimension()),
                        actor,
                    )?);
                }
            }
            Event::HostileState(states) => {
                for (ordinal, record) in states.states().iter().enumerate() {
                    let record_ordinal =
                        u32::try_from(ordinal).map_err(|_| ClientError::Capacity)?;
                    let dimension = resolved_dimension(observation, record.id())?;
                    let actor = ActorRecord::try_new(
                        header(view, observation.source_tick(), FamilyOperation::Upsert)?,
                        ActorKind::Hostile,
                        ActorId::Hostile(record.id()),
                        ActorDimension::Known(dimension),
                        Some(widened(record.position().get())),
                        Some(f64::from(record.yaw())),
                        // Still yaw-only: the per-tick state publication adds
                        // the velocity and the health, never a pitch.
                        None,
                        Some(widened(record.velocity().get())),
                        Some(ActorDetail::Hostile {
                            archetype: archetype(record.kind()),
                            health: record.health(),
                        }),
                    )?;
                    records.push(OrderedRecord::try_new(
                        ProjectionOrder::Confirmed {
                            observation: key,
                            record_ordinal,
                        },
                        stable_key(record.id(), dimension),
                        actor,
                    )?);
                }
            }
            Event::HostileDespawn(despawn) => {
                for (ordinal, id) in despawn.ids().iter().enumerate() {
                    let record_ordinal =
                        u32::try_from(ordinal).map_err(|_| ClientError::Capacity)?;
                    let dimension = resolved_dimension(observation, *id)?;
                    // A despawn is a removal alone: no pose, no angle, no
                    // velocity and no detail survive the identity.
                    let actor = ActorRecord::try_new(
                        header(view, observation.source_tick(), FamilyOperation::Remove)?,
                        ActorKind::Hostile,
                        ActorId::Hostile(*id),
                        ActorDimension::Known(dimension),
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
                        stable_key(*id, dimension),
                        actor,
                    )?);
                }
            }
            // Every other publication belongs to another per-kind provider.
            // A CombatHit names a target category, never a hostile identity,
            // so no attacker association or health change is drawn from it.
            _ => {}
        }
    }
    Ok(records)
}
