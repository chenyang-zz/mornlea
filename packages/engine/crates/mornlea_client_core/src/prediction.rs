//! The prediction and correction seam's declared state owner.
//!
//! The player prediction state is the immutable projection input the
//! `player-view` projection borrows: the checked confirmed pose, the
//! explicitly attributed predicted pose, the last correction, the finite
//! look ray, the movement intent and the accepted source mining state. It
//! carries no elapsed local time, because authoritative mining progress is
//! never reconstructed from it. The replay logic itself (journal replay by
//! sequence, acknowledgement and rejection) belongs to the correction
//! provider; this contract landing declares and checks the shape.

use crate::contracts::{ClientError, ConfirmedRevision, SessionEpoch};
use crate::presentation::{Correction, FiniteRay, MovementIntent, Pose};

/// One accepted prediction journal observation, keyed by epoch and local
/// sequence. A rejected input removes its pending entry; an acknowledged one
/// confirms exactly once.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct JournalEntry {
    epoch: SessionEpoch,
    sequence: u64,
    acknowledged: bool,
}

impl JournalEntry {
    pub fn try_new(
        epoch: SessionEpoch,
        sequence: u64,
        acknowledged: bool,
    ) -> Result<Self, ClientError> {
        if sequence == 0 {
            return Err(ClientError::InvalidInput);
        }
        Ok(Self {
            epoch,
            sequence,
            acknowledged,
        })
    }

    pub fn epoch(&self) -> SessionEpoch {
        self.epoch
    }

    pub fn sequence(&self) -> u64 {
        self.sequence
    }

    pub fn acknowledged(&self) -> bool {
        self.acknowledged
    }
}

/// The checked player prediction state: the confirmed pose beside its
/// explicitly attributed predicted pose, the correction that caused the last
/// replay, the finite look ray, the movement intent and the source mining
/// state. The predicted pose is always tagged; a correction never relabels
/// it confirmed.
#[derive(Clone, Debug)]
pub struct PlayerProjectionState {
    epoch: SessionEpoch,
    revision: ConfirmedRevision,
    confirmed_pose: Pose,
    predicted_pose: Option<Pose>,
    journal: Vec<JournalEntry>,
    correction: Option<Correction>,
    look_ray: Option<FiniteRay>,
    movement: MovementIntent,
}

impl PlayerProjectionState {
    pub fn try_new(
        epoch: SessionEpoch,
        revision: ConfirmedRevision,
        confirmed_pose: Pose,
        predicted_pose: Option<Pose>,
        journal: Vec<JournalEntry>,
        correction: Option<Correction>,
        look_ray: Option<FiniteRay>,
        movement: MovementIntent,
    ) -> Result<Self, ClientError> {
        Ok(Self {
            epoch,
            revision,
            confirmed_pose,
            predicted_pose,
            journal,
            correction,
            look_ray,
            movement,
        })
    }

    pub fn epoch(&self) -> SessionEpoch {
        self.epoch
    }

    pub fn revision(&self) -> ConfirmedRevision {
        self.revision
    }

    pub fn confirmed_pose(&self) -> &Pose {
        &self.confirmed_pose
    }

    pub fn predicted_pose(&self) -> Option<&Pose> {
        self.predicted_pose.as_ref()
    }

    pub fn journal(&self) -> &[JournalEntry] {
        &self.journal
    }

    pub fn correction(&self) -> Option<&Correction> {
        self.correction.as_ref()
    }

    pub fn look_ray(&self) -> Option<&FiniteRay> {
        self.look_ray.as_ref()
    }

    pub fn movement(&self) -> &MovementIntent {
        &self.movement
    }
}
