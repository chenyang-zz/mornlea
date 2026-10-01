//! The projectile actor projection.
//!
//! This provider turns the projectile publications of the committed
//! observation queue into ordered `actors@1` records: a launch batch is one
//! upsert per record in published packet order carrying the typed identity,
//! the actual launch dimension, the exact finite position, the launch
//! velocity and the wire archetype detail, with no yaw and no pitch because
//! a projectile publication carries no orientation — the client derives the
//! orientation from the launch velocity; a state batch is one upsert per
//! record carrying the position alone, its dimension resolved from the
//! prevalidated identity resolution the confirmed mirror recorded because
//! the state packet carries none — the launch velocity is never
//! re-estimated from a position sample the way the legacy mirror diffed it,
//! and no archetype is republished because the state record has no
//! additional actor fields; a despawn batch is one remove-only record per
//! identity, again with the dimension from the identity resolution, and no
//! detail at all because the despawn publication names no cause. The same
//! identity may launch again after its removal, so repeated stable keys
//! across observations are the legal reuse lifecycle, not a duplicate.
//!
//! Ownership boundaries: the projection is pure over the immutable
//! `ProjectionView` and owns no second mirror, no packet history and no
//! ballistic simulation. A `CombatHit` publication names a target category
//! whose closed union has no projectile variant, so it is ignored wholesale
//! here — no hit marker, no attacker association and no
//! projectile-to-projectile link is invented, and a projectile's position
//! and velocity come from the projectile publications alone. Observations of
//! other kinds belong to their own per-kind providers. A malformed
//! observation identity — a dimensionless state or despawn whose
//! prevalidated resolution is missing, ambiguous or names another identity,
//! a packet that is not a checked publication, or an observation from
//! another epoch — rejects the whole projection before any record is
//! returned, so no partial output exists; the confirmed mirror and the
//! serial family assembler remain the identity and publication owners.

use mornlea_domain::{Dimension, Event, ProjectileId, ProjectileKind};

use mornlea_protocol::{PROJECTILE_KIND_ARROW, PROJECTILE_KIND_SHARD};

use crate::contracts::{ClientError, FamilyOperation, RecordHeader};
use crate::presentation::frame::{ActorDetail, ActorDimension, ActorId, ActorKind, ActorRecord};
use crate::presentation::{
    AcceptedObservation, OrderedRecord, ProjectionOrder, ProjectionView, StableRecordKey,
};

/// Widens one finite f32 publication vector exactly; f32-to-f64 conversion is
/// exact, so no world coordinate or launch velocity is rounded or invented.
fn widened(components: [f32; 3]) -> [f64; 3] {
    [
        f64::from(components[0]),
        f64::from(components[1]),
        f64::from(components[2]),
    ]
}

/// The wire archetype byte of the closed domain kind, preserving the accepted
/// kind values instead of inventing a second mapping.
fn archetype(kind: ProjectileKind) -> u8 {
    match kind {
        ProjectileKind::Shard => PROJECTILE_KIND_SHARD,
        ProjectileKind::Arrow => PROJECTILE_KIND_ARROW,
    }
}

/// The stable identity of one projectile record: kind, actual dimension and
/// the typed identity, never a numeric conversion of the identity.
fn stable_key(id: ProjectileId, dimension: Dimension) -> StableRecordKey {
    StableRecordKey::Actor {
        kind: ActorKind::Projectile,
        dimension: ActorDimension::Known(dimension),
        id: ActorId::Projectile(id),
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
    id: ProjectileId,
) -> Result<Dimension, ClientError> {
    let wanted = ActorId::Projectile(id);
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

/// Projects the projectile records of one immutable view, in actual source
/// order: observation key first, then the packet record ordinal.
///
/// Order is never reconstructed from the optional source tick — equal,
/// absent or backwards ticks keep the observation and packet order — nor
/// from the rebased parent revision. The serial actor family assembler
/// consumes these envelopes, validates remove-before-reuse and strips the
/// envelope when the unchanged record vector is emitted.
pub fn project_projectile(
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
            Event::ProjectileSpawn(spawn) => {
                for (index, record) in spawn.spawns().iter().enumerate() {
                    let record_ordinal = ordinal(index)?;
                    let entry = ActorRecord::try_new(
                        header(view, observation.source_tick(), FamilyOperation::Upsert)?,
                        ActorKind::Projectile,
                        ActorId::Projectile(record.id()),
                        ActorDimension::Known(record.dimension()),
                        Some(widened(record.position().get())),
                        // The publication carries no orientation: no yaw and
                        // no pitch exists on any projectile record.
                        None,
                        None,
                        Some(widened(record.velocity().get())),
                        Some(ActorDetail::Projectile {
                            archetype: Some(archetype(record.kind())),
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
            Event::ProjectileState(states) => {
                for (index, record) in states.states().iter().enumerate() {
                    let record_ordinal = ordinal(index)?;
                    let dimension = resolved_dimension(observation, record.id())?;
                    // Position only: the state record carries no velocity and
                    // no kind, so the launch velocity is kept — never
                    // re-attributed from the sample — and no archetype is
                    // republished.
                    let entry = ActorRecord::try_new(
                        header(view, observation.source_tick(), FamilyOperation::Upsert)?,
                        ActorKind::Projectile,
                        ActorId::Projectile(record.id()),
                        ActorDimension::Known(dimension),
                        Some(widened(record.position().get())),
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
                        stable_key(record.id(), dimension),
                        entry,
                    )?);
                }
            }
            Event::ProjectileDespawn(despawn) => {
                for (index, id) in despawn.ids().iter().enumerate() {
                    let record_ordinal = ordinal(index)?;
                    let dimension = resolved_dimension(observation, *id)?;
                    // A despawn is a removal alone: no pose, no angle, no
                    // velocity and no cause detail survive the identity,
                    // because the publication names none.
                    let entry = ActorRecord::try_new(
                        header(view, observation.source_tick(), FamilyOperation::Remove)?,
                        ActorKind::Projectile,
                        ActorId::Projectile(*id),
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
                        entry,
                    )?);
                }
            }
            // Every other publication belongs to another per-kind provider.
            // A CombatHit names a target category whose closed union has no
            // projectile variant, so no hit marker or projectile link is
            // ever drawn from it.
            _ => {}
        }
    }
    Ok(records)
}
