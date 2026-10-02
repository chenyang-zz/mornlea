//! The player-view projection: one checked summary record of the local
//! player's confirmed and predicted state.
//!
//! This provider turns the prediction owner's checked projection state —
//! the confirmed pose, the explicitly attributed predicted pose, the last
//! correction, the finite look ray and the movement intent — into one
//! `player-view@1` record per projection. The confirmed pose is always the
//! authority's own pose and the predicted pose stays in its attributed
//! optional field, so an unconfirmed pose is never labeled confirmed: not
//! while steps are pending, not after a correction replays them, and not
//! after a rejection removes one. A correction is republished verbatim with
//! its exact reason and last input sequence, or not at all when the
//! authority left none — a not-ready authority publishes no correction and
//! no prediction.
//!
//! Mining ownership: the authoritative mining progress is the exact
//! `MiningState` the latest retained player observation carried, in actual
//! source order, and the idle value when no player observation is retained.
//! It is never reconstructed from elapsed local time or an advancing frame
//! revision; the private body observation carries no mirror-store identity,
//! so this family projects it from the retained observation queue exactly
//! like the other body families.
//!
//! Failure policy: a player state or an observation from another epoch
//! rejects the whole projection with `StaleEpoch` before any record is
//! returned — an epoch reset clears the projection owner, and nothing of an
//! old epoch may enter a fresh frame — and a packet that is not a checked
//! semantic publication rejects with `InvalidInput`. The emitted look ray is
//! re-derived through the typed `FiniteRay` constructor, whose zero, NaN and
//! over-range rejections are the boundary gate of every ray this family
//! publishes. No prediction math runs here: the prediction owner's checked
//! values are read-only inputs, and the confirmed mirror remains the sole
//! attribution and state owner.

use mornlea_domain::{Event, MiningState};

use crate::contracts::{ClientError, FamilyOperation, RecordHeader};
use crate::presentation::frame::PlayerViewRecord;
use crate::presentation::{FiniteRay, MovementIntent, ProjectionView};

/// Projects the player-view records of one immutable view: exactly one
/// record summarizing the current player projection state, rebased onto the
/// coherent candidate frame identity.
///
/// The singleton record is a whole-view summary, never one record per
/// observation: the player view a frame publishes is the single checked
/// state the prediction owner currently attests, beside the mining of the
/// latest authoritative player observation the frame retained.
pub fn project_player_view(
    view: &ProjectionView<'_>,
) -> Result<Vec<PlayerViewRecord>, ClientError> {
    let player = view.player();
    if player.epoch() != view.frame_epoch() {
        // A player state from another epoch never enters this frame,
        // exactly as the confirmed mirror refuses old epochs; the reset
        // path cleared the owner, so nothing stale may republish.
        return Err(ClientError::StaleEpoch);
    }
    let mut mining = MiningState::Idle;
    let mut source_tick = None;
    for observation in view.observations() {
        let key = *observation.key();
        if key.epoch() != view.frame_epoch() {
            // An observation from another epoch never enters this frame,
            // however valid its packet is; old epochs never resurrect.
            return Err(ClientError::StaleEpoch);
        }
        let event =
            Event::try_from(observation.packet().clone()).map_err(|_| ClientError::InvalidInput)?;
        if let Event::PlayerState(state) = event {
            // The exact authoritative source value, in actual source order:
            // the last retained player observation wins, and nothing is
            // ever advanced from elapsed revisions or local time.
            mining = state.mining();
            source_tick = observation.source_tick();
        }
    }
    // The emitted ray passes through the typed constructor again, so a
    // zero, NaN or over-range value rejects this projection with a typed
    // error instead of reaching a record.
    let look_ray = match player.look_ray() {
        Some(ray) => Some(FiniteRay::try_new(ray.origin(), ray.look(), ray.reach())?),
        None => None,
    };
    let movement = MovementIntent::try_new(
        player.movement().control().copied(),
        player.movement().on_ground(),
    )?;
    let record = PlayerViewRecord::try_new(
        RecordHeader::try_new(
            view.frame_epoch(),
            view.frame_revision(),
            source_tick,
            FamilyOperation::Upsert,
        )?,
        *player.confirmed_pose(),
        player.predicted_pose().copied(),
        look_ray,
        movement,
        player.correction().copied(),
        mining,
    )?;
    Ok(vec![record])
}
