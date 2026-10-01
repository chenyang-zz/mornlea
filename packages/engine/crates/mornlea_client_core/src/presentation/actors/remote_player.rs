//! The remote-player actor projection.
//!
//! This provider turns the remote-player publications of the committed
//! observation queue into ordered `actors@1` records: a spawn is one upsert
//! carrying the typed identity, the actual spawn dimension, the exact finite
//! pose and the display-name detail; a state batch is one upsert per record
//! in published packet order carrying the pose and the reset detail; a
//! despawn is one remove-only record whose dimension comes from the identity
//! resolution the confirmed mirror recorded. The same identity may spawn
//! again after its removal, so repeated stable keys across observations are
//! the legal reuse lifecycle, not a duplicate.
//!
//! Ownership boundaries: the projection is pure over the immutable
//! `ProjectionView` and owns no second mirror, no packet history and no
//! display state. The remote-player wire publication carries no velocity, so
//! no record ever carries one; no attack, lure, hit or death attribution is
//! invented here. Observations of other kinds are ignored wholesale: they
//! belong to their own per-kind providers. A malformed observation identity
//! — a despawn whose prevalidated resolution is missing, ambiguous or names
//! another identity, a packet that is not a checked publication, or an
//! observation from another epoch — rejects the whole projection before any
//! record is returned, so no partial output exists; the confirmed mirror and
//! the serial family assembler remain the identity and publication owners.

use mornlea_domain::{Dimension, Event, PlayerId};

use crate::contracts::{ClientError, FamilyOperation, RecordHeader};
use crate::presentation::frame::{ActorDetail, ActorDimension, ActorId, ActorKind, ActorRecord};
use crate::presentation::{
    AcceptedObservation, BoundedText, OrderedRecord, ProjectionOrder, ProjectionView,
    StableRecordKey, TextKind,
};

/// Widens one finite f32 publication position exactly; f32-to-f64 conversion
/// is exact, so no world coordinate is rounded or invented.
fn widened(components: [f32; 3]) -> [f64; 3] {
    [
        f64::from(components[0]),
        f64::from(components[1]),
        f64::from(components[2]),
    ]
}

/// The stable identity of one remote-player record: kind, actual dimension
/// and the typed UUID, never a numeric conversion of the identity.
fn stable_key(id: PlayerId, dimension: Dimension) -> StableRecordKey {
    StableRecordKey::Actor {
        kind: ActorKind::RemotePlayer,
        dimension: ActorDimension::Known(dimension),
        id: ActorId::RemotePlayer(id),
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
    id: PlayerId,
) -> Result<Dimension, ClientError> {
    let wanted = ActorId::RemotePlayer(id);
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

/// Projects the remote-player records of one immutable view, in actual
/// source order: observation key first, then the packet record ordinal.
///
/// Order is never reconstructed from the optional source tick — equal or
/// absent ticks keep the observation and packet order — nor from the rebased
/// parent revision. The serial actor family assembler consumes these
/// envelopes, validates remove-before-reuse and strips the envelope when the
/// unchanged record vector is emitted.
pub fn project_remote_player(
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
            Event::RemotePlayerSpawn(spawn) => {
                let record = ActorRecord::try_new(
                    header(view, observation.source_tick(), FamilyOperation::Upsert)?,
                    ActorKind::RemotePlayer,
                    ActorId::RemotePlayer(spawn.player_id()),
                    ActorDimension::Known(spawn.dimension()),
                    Some(widened(spawn.position().get())),
                    Some(f64::from(spawn.look().yaw())),
                    Some(f64::from(spawn.look().pitch())),
                    // The remote-player publication carries no velocity.
                    None,
                    Some(ActorDetail::RemotePlayer {
                        display_name: Some(BoundedText::try_new(
                            spawn.display_name().as_str().to_owned(),
                            TextKind::Name,
                        )?),
                        reset: None,
                    }),
                )?;
                records.push(OrderedRecord::try_new(
                    ProjectionOrder::Confirmed {
                        observation: key,
                        record_ordinal: 0,
                    },
                    stable_key(spawn.player_id(), spawn.dimension()),
                    record,
                )?);
            }
            Event::RemotePlayerStates(batch) => {
                for (ordinal, state) in batch.states().iter().enumerate() {
                    // A packet record ordinal beyond u32 cannot occur for the
                    // accepted batch cap, but a checked conversion never
                    // truncates silently.
                    let record_ordinal =
                        u32::try_from(ordinal).map_err(|_| ClientError::Capacity)?;
                    let record = ActorRecord::try_new(
                        header(view, observation.source_tick(), FamilyOperation::Upsert)?,
                        ActorKind::RemotePlayer,
                        ActorId::RemotePlayer(state.player_id()),
                        ActorDimension::Known(state.dimension()),
                        Some(widened(state.position().get())),
                        Some(f64::from(state.look().yaw())),
                        Some(f64::from(state.look().pitch())),
                        None,
                        Some(ActorDetail::RemotePlayer {
                            display_name: None,
                            reset: Some(state.reset()),
                        }),
                    )?;
                    records.push(OrderedRecord::try_new(
                        ProjectionOrder::Confirmed {
                            observation: key,
                            record_ordinal,
                        },
                        stable_key(state.player_id(), state.dimension()),
                        record,
                    )?);
                }
            }
            Event::RemotePlayerDespawn(despawn) => {
                let dimension = resolved_dimension(observation, despawn.player_id())?;
                // A despawn is a removal alone: no pose, no angle, no
                // velocity and no detail survive the identity.
                let record = ActorRecord::try_new(
                    header(view, observation.source_tick(), FamilyOperation::Remove)?,
                    ActorKind::RemotePlayer,
                    ActorId::RemotePlayer(despawn.player_id()),
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
                    stable_key(despawn.player_id(), dimension),
                    record,
                )?);
            }
            // Every other publication belongs to another per-kind provider.
            _ => {}
        }
    }
    Ok(records)
}
