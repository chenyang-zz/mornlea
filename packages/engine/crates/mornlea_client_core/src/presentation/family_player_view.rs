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
//! Mining ownership: the authoritative mining progress is the persisted
//! source `MiningState` of the latest confirmed authority, carried by the
//! prediction projection state itself — the same value the owner's
//! `source_mining()` reports — not by any retained observation. It therefore
//! survives a full observation-queue drain unchanged (no idle flap), always
//! pairs with the confirmed base's own pose, and only an authoritative
//! confirmation replaces it; a prediction, a rejection, a duplicate delivery
//! or an advancing frame revision never touches it, and nothing is ever
//! reconstructed from elapsed local time. Because the record's source is the
//! owner-persisted summary rather than one retained packet, the header
//! carries no source tick: none is invented from an observation the
//! publication cursor may already have consumed.
//!
//! Failure policy: a player state or an observation from another epoch
//! rejects the whole projection with `StaleEpoch` before any record is
//! returned — an epoch reset clears the projection owner, and nothing of an
//! old epoch may enter a fresh frame. The emitted look ray is re-derived
//! through the typed `FiniteRay` constructor, whose zero, NaN and over-range
//! rejections are the boundary gate of every ray this family publishes. No
//! prediction math runs here: the prediction owner's checked values are
//! read-only inputs, and the confirmed mirror remains the sole attribution
//! and state owner.

use crate::contracts::{ClientError, FamilyOperation, RecordHeader};
use crate::presentation::frame::PlayerViewRecord;
use crate::presentation::{FiniteRay, MovementIntent, ProjectionView};

/// Projects the player-view records of one immutable view: exactly one
/// record summarizing the current player projection state, rebased onto the
/// coherent candidate frame identity.
///
/// The singleton record is a whole-view summary, never one record per
/// observation: the player view a frame publishes is the single checked
/// state the prediction owner currently attests, including the persisted
/// source mining of its confirmed base. The retained observation queue is
/// read only for epoch coherence, never for player facts: the persisted
/// owner state owns them.
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
    for observation in view.observations() {
        if observation.key().epoch() != view.frame_epoch() {
            // An observation from another epoch never enters this frame,
            // however valid its packet is; old epochs never resurrect. The
            // mirror would have refused its commit, so a foreign entry is
            // a queue-contract violation, not player content to project.
            return Err(ClientError::StaleEpoch);
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
            // The summary's source is the owner-persisted state, which
            // carries no tick; no tick is invented from a retained
            // observation the drain may already have consumed.
            None,
            FamilyOperation::Upsert,
        )?,
        *player.confirmed_pose(),
        player.predicted_pose().copied(),
        look_ray,
        movement,
        player.correction().copied(),
        *player.mining(),
    )?;
    Ok(vec![record])
}
