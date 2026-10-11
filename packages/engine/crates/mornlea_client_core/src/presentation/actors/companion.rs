//! The companion actor projection.
//!
//! This provider turns the companion publications of the committed
//! observation queue into ordered `actors@1` records: a spawn is one upsert
//! carrying the typed UUID identity, the actual spawn dimension, the exact
//! finite pose with its vertical-range pitch and the canonical name detail; a
//! state batch is one upsert per record in published packet order carrying
//! the pose with its pitch and the reset detail; a despawn is one remove-only
//! record whose dimension comes from the identity resolution the confirmed
//! mirror recorded. The same identity may spawn again after its removal, so
//! repeated stable keys across observations are the legal reuse lifecycle,
//! not a duplicate.
//!
//! Ownership boundaries: the projection is pure over the immutable
//! `ProjectionView` and owns no second mirror, no packet history and no
//! presentation state. A companion's task lifecycle is a chat observation
//! the world-ui family owns, and a companion removal publishes no death
//! fact, so no task state, failure reason, death marker, health or velocity
//! is ever invented here: the companion wire carries the identity, the name,
//! the pose and the reset bit alone. Observations of other kinds are ignored
//! wholesale: they belong to their own per-kind providers. A malformed
//! observation identity — a despawn whose prevalidated resolution is missing,
//! ambiguous or names another identity, a packet that is not a checked
//! publication, or an observation from another epoch — rejects the whole
//! projection before any record is returned, so no partial output exists; the
//! confirmed mirror and the serial family assembler remain the identity and
//! publication owners.

use mornlea_domain::{Dimension, Event};

use crate::contracts::{ClientError, FamilyOperation, RecordHeader};
use crate::presentation::frame::{ActorDetail, ActorDimension, ActorId, ActorKind, ActorRecord};
use crate::presentation::{
    AcceptedObservation, BoundedText, OrderedRecord, ProjectionOrder, ProjectionView,
    StableRecordKey, TextKind,
};

/// Widens one finite f32 publication vector exactly; f32-to-f64 conversion is
/// exact, so no world coordinate is rounded or invented.
fn widened(components: [f32; 3]) -> [f64; 3] {
    [
        f64::from(components[0]),
        f64::from(components[1]),
        f64::from(components[2]),
    ]
}

/// The stable identity of one companion record: kind, actual dimension and
/// the typed UUID identity, never a numeric conversion of the identity.
fn stable_key(id: mornlea_domain::CompanionId, dimension: Dimension) -> StableRecordKey {
    StableRecordKey::Actor {
        kind: ActorKind::Companion,
        dimension: ActorDimension::Known(dimension),
        id: ActorId::Companion(id),
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

/// Resolves the confirmed dimension of one dimensionless despawn against the
/// observation's prevalidated identity resolution.
///
/// Exactly one matching live resolution must name the despawned identity:
/// none is an orphan, more than one is ambiguous, and both reject the whole
/// projection instead of guessing a dimension the mirror never confirmed.
fn resolved_dimension(
    observation: &AcceptedObservation,
    id: mornlea_domain::CompanionId,
) -> Result<Dimension, ClientError> {
    let wanted = ActorId::Companion(id);
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

/// Projects the companion records of one immutable view, in actual source
/// order: observation key first, then the packet record ordinal.
///
/// Order is never reconstructed from the optional source tick — equal,
/// absent or backwards ticks keep the observation and packet order — nor from
/// the rebased parent revision. The serial actor family assembler consumes
/// these envelopes, validates remove-before-reuse and strips the envelope
/// when the unchanged record vector is emitted.
pub fn project_companion(
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
            Event::CompanionSpawn(spawn) => {
                let record = ActorRecord::try_new(
                    header(view, observation.source_tick(), FamilyOperation::Upsert)?,
                    ActorKind::Companion,
                    ActorId::Companion(spawn.id()),
                    ActorDimension::Known(spawn.dimension()),
                    Some(widened(spawn.position().get())),
                    Some(f64::from(spawn.look().yaw())),
                    Some(f64::from(spawn.look().pitch())),
                    // The companion publication carries no velocity.
                    None,
                    Some(ActorDetail::Companion {
                        name: Some(BoundedText::try_new(
                            spawn.name().as_str().to_owned(),
                            TextKind::Name,
                        )?),
                        // A new body has no replay marker yet.
                        reset: None,
                    }),
                )?;
                records.push(OrderedRecord::try_new(
                    ProjectionOrder::Confirmed {
                        observation: key,
                        record_ordinal: 0,
                    },
                    stable_key(spawn.id(), spawn.dimension()),
                    record,
                )?);
            }
            Event::CompanionStates(states) => {
                for (ordinal, state) in states.states().iter().enumerate() {
                    // A packet record ordinal beyond u32 cannot occur for the
                    // accepted batch cap, but a checked conversion never
                    // truncates silently.
                    let record_ordinal =
                        u32::try_from(ordinal).map_err(|_| ClientError::Capacity)?;
                    let record = ActorRecord::try_new(
                        header(view, observation.source_tick(), FamilyOperation::Upsert)?,
                        ActorKind::Companion,
                        ActorId::Companion(state.id()),
                        ActorDimension::Known(state.dimension()),
                        Some(widened(state.position().get())),
                        Some(f64::from(state.look().yaw())),
                        Some(f64::from(state.look().pitch())),
                        None,
                        Some(ActorDetail::Companion {
                            // The state batch repeats no name: the spawn
                            // alone carries it.
                            name: None,
                            reset: Some(state.reset()),
                        }),
                    )?;
                    records.push(OrderedRecord::try_new(
                        ProjectionOrder::Confirmed {
                            observation: key,
                            record_ordinal,
                        },
                        stable_key(state.id(), state.dimension()),
                        record,
                    )?);
                }
            }
            Event::CompanionDespawn(despawn) => {
                let dimension = resolved_dimension(observation, despawn.id())?;
                // A despawn is a removal alone: no pose, no angle, no
                // velocity, no name and no reset bit survive the identity,
                // and no death fact exists on this wire to publish.
                let record = ActorRecord::try_new(
                    header(view, observation.source_tick(), FamilyOperation::Remove)?,
                    ActorKind::Companion,
                    ActorId::Companion(despawn.id()),
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
                        record_ordinal: 0,
                    },
                    stable_key(despawn.id(), dimension),
                    record,
                )?);
            }
            // Every other publication belongs to another per-kind provider;
            // the companion task lifecycle arrives as chat and the world-ui
            // family owns it.
            _ => {}
        }
    }
    Ok(records)
}
