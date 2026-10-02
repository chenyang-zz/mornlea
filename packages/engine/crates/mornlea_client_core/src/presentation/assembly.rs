//! The serial whole-frame assembly and the atomic publication transaction.
//!
//! This module is the single serial publication owner of the presentation
//! seam. [`assemble_frame`] checks a complete candidate against the current
//! visible frame and returns it as a prepared immutable `Arc` without
//! touching the current frame, so a snapshot taken from the old Arc stays
//! safe across every later failure. [`prepare_publication`] is the checked
//! reservation step: it validates the whole candidate (the accepted frame
//! validator under the configured limits), the next frame index, every
//! consumption cursor as a FIFO prefix of the live queues, and the dequeued
//! entries' actual source keys against the attributions the candidate and
//! the dedup delta publish — all before any owner mutates, because the
//! borrowed bundle is immutable here by construction. [`commit_publication`]
//! is the final single-owner critical section: it applies the staged dedup
//! delta, consumes the published queue prefixes, writes the input
//! projection's halves and then swaps the visible Arc exactly once. No
//! callbacks, waits or fallible operations exist on that path, and a failure
//! or panic before it changes no visible frame, index, dedup state or
//! consumption cursor: events, removals, cancellations and the dedup
//! proposal stay retry-owned and a valid retry commits once.
//!
//! Staging surfaces owned here: the audio commit applies the proposed
//! delta — cancelled keys leave the committed set, insertions join it — and
//! stages the applied cancellations as the audio state's pending predicted
//! cancellations, the suppression that outlives the consumed rejection
//! metadata; the input commit consumes the published metadata prefix
//! (admitted entries first, then rejected) and the published local cue
//! prefix, and advances the sequence hint past the sequences the frame
//! attests. The pending local-cue ceiling stays the admission-owned
//! `queued_input_events` bound: publication only drains it.

use std::sync::Arc;

use crate::contracts::{ClientError, ClientLimits, ObservationKey};
use crate::input::{ClientIntentKind, InputProjectionState};
use crate::presentation::frame::{FamilyRecords, InputReceiptState, PresentationFrame};
use crate::presentation::{
    AcceptedObservation, AudioDedupDelta, AudioDedupKey, AudioProjectionState, CueProvenance,
    InputAdmissionOwner, LifecycleProjectionState, PublicationConsumption, PublicationOwners,
    PublicationReservation,
};

impl<'a> PublicationOwners<'a> {
    /// The controller's exclusive owners bundle: the visible Arc beside the
    /// observation queue, the pending input state, the committed audio dedup
    /// state and the pending lifecycle state. The bundle grants no authority
    /// over the confirmed mirror; every owner is lent by the controller and
    /// mutated only inside the publication transaction's commit section.
    pub fn try_new(
        visible: &'a mut Arc<PresentationFrame>,
        observations: &'a mut Vec<AcceptedObservation>,
        input: &'a mut InputAdmissionOwner,
        audio: &'a mut AudioProjectionState,
        lifecycle: &'a mut LifecycleProjectionState,
    ) -> Result<Self, ClientError> {
        Ok(Self {
            visible,
            observations,
            input,
            audio,
            lifecycle,
        })
    }
}

/// Checks one complete candidate against the current visible frame: the
/// accepted frame validator decides every family-level and whole-frame
/// bound, and the candidate must carry exactly the next frame index. Any
/// rejection is typed and leaves the current frame untouched.
fn check_candidate(
    current: &PresentationFrame,
    candidate: &PresentationFrame,
    limits: &ClientLimits,
) -> Result<(), ClientError> {
    candidate.validate(limits)?;
    let next = current
        .frame_index()
        .checked_add(1)
        .ok_or(ClientError::Capacity)?;
    if candidate.frame_index() != next {
        return Err(ClientError::InvalidInput);
    }
    Ok(())
}

/// Checks one complete candidate and returns it as a prepared immutable
/// `Arc` without changing the current frame. The old Arc stays fully
/// readable, and a snapshot taken from it stays safe across every later
/// failure of the same publication attempt.
pub fn assemble_frame(
    current: &Arc<PresentationFrame>,
    candidate: PresentationFrame,
    limits: &ClientLimits,
) -> Result<Arc<PresentationFrame>, ClientError> {
    check_candidate(current, &candidate, limits)?;
    Ok(Arc::new(candidate))
}

/// Reserves one publication transaction: the complete candidate, every
/// byte and count, and the dequeued entries' actual source keys are
/// validated before any owner mutates — the borrowed bundle is immutable
/// here by construction — and the checked reservation carries the validated
/// new Arc, the preallocated proposed audio dedup state and the consumption
/// cursors. A rejection is typed and consumes nothing: events, removals,
/// cancellations and the dedup proposal stay retry-owned.
pub fn prepare_publication(
    owners: &PublicationOwners<'_>,
    candidate: PresentationFrame,
    audio: AudioDedupDelta,
    consume: PublicationConsumption,
    limits: &ClientLimits,
) -> Result<PublicationReservation, ClientError> {
    // The candidate must be complete and exactly the next frame.
    check_candidate(owners.visible, &candidate, limits)?;

    // Cursor legality: every count is a FIFO prefix of its live queue, never
    // an arbitrary removal index. The pending input metadata queue is the
    // admitted entries followed by the rejected entries, the frozen drain
    // order of the publication owner.
    let observations = owners
        .observations
        .get(..consume.observations())
        .ok_or(ClientError::InvalidInput)?;
    let projection = &owners.input.projection;
    let input_count = consume.input_records();
    let admitted_count = input_count.min(projection.admitted().len());
    let rejected_count = input_count - admitted_count;
    if rejected_count > projection.rejected().len() {
        return Err(ClientError::InvalidInput);
    }
    let dequeued_admitted = &projection.admitted()[..admitted_count];
    let dequeued_rejected = &projection.rejected()[..rejected_count];
    let dequeued_cues = projection
        .pending_local_cues()
        .get(..consume.local_cues())
        .ok_or(ClientError::InvalidInput)?;
    if consume.lifecycle_records() > owners.lifecycle.pending().len() {
        return Err(ClientError::InvalidInput);
    }

    // The dequeued entries' actual source keys: every confirmed or local
    // attribution the candidate publishes and every confirmed insertion the
    // delta commits must name a source inside the dequeued prefix, and every
    // dequeued input metadata entry must be attested by a record the
    // candidate publishes — no publication without its consumed source, no
    // consumed source without its publication.
    let attests_observation =
        |wanted: &ObservationKey| observations.iter().any(|entry| entry.key() == wanted);
    for key in audio.insertions() {
        match key {
            AudioDedupKey::Confirmed { observation, .. } => {
                if !attests_observation(observation) {
                    return Err(ClientError::InvalidInput);
                }
            }
            AudioDedupKey::Local {
                local_event_sequence,
                ..
            } => {
                if !dequeued_cues
                    .iter()
                    .any(|source| source.local_event_sequence == *local_event_sequence)
                {
                    return Err(ClientError::InvalidInput);
                }
            }
            AudioDedupKey::Predicted { .. } => {}
        }
    }

    let mut input_attested_admitted: Vec<(u64, ClientIntentKind)> = Vec::new();
    let mut input_attested_rejected: Vec<u64> = Vec::new();
    for family in candidate.families() {
        match family.records() {
            FamilyRecords::AudioCues(records) => {
                for record in records {
                    match record.provenance() {
                        CueProvenance::Confirmed { observation, .. } => {
                            if !attests_observation(observation) {
                                return Err(ClientError::InvalidInput);
                            }
                        }
                        CueProvenance::Local {
                            local_event_sequence,
                        } => {
                            if !dequeued_cues
                                .iter()
                                .any(|source| source.local_event_sequence == *local_event_sequence)
                            {
                                return Err(ClientError::InvalidInput);
                            }
                        }
                        CueProvenance::Predicted { .. } => {}
                    }
                }
            }
            FamilyRecords::Input(records) => {
                for record in records {
                    match record.receipt() {
                        InputReceiptState::Queued => {
                            let sequence =
                                record.local_sequence().ok_or(ClientError::InvalidInput)?;
                            if !dequeued_admitted.iter().any(|(held, kind)| {
                                *held == sequence && *kind == record.intent_kind()
                            }) {
                                return Err(ClientError::InvalidInput);
                            }
                            input_attested_admitted.push((sequence, record.intent_kind()));
                        }
                        InputReceiptState::Rejected { .. } => {
                            let sequence =
                                record.local_sequence().ok_or(ClientError::InvalidInput)?;
                            if !dequeued_rejected.iter().any(|(held, _)| *held == sequence) {
                                return Err(ClientError::InvalidInput);
                            }
                            input_attested_rejected.push(sequence);
                        }
                        // A confirmed receipt's source is the acknowledgment
                        // observation, not the pending admission metadata.
                        InputReceiptState::Confirmed { .. } => {}
                    }
                }
            }
            _ => {}
        }
    }
    for (sequence, kind) in dequeued_admitted {
        if !input_attested_admitted
            .iter()
            .any(|(held, attested)| held == sequence && attested == kind)
        {
            return Err(ClientError::InvalidInput);
        }
    }
    for (sequence, _) in dequeued_rejected {
        if !input_attested_rejected.contains(sequence) {
            return Err(ClientError::InvalidInput);
        }
    }

    // Everything checked; the reservation stages the validated state. No
    // owner has mutated, so this point is unreachable on any failure path.
    PublicationReservation::try_new(Arc::new(candidate), audio, consume)
}

/// Commits one reserved publication: the final single-owner critical
/// section. Every operation here is an infallible owner swap or bounded
/// vector move over state the reservation already checked — no callbacks,
/// waits, new staging allocations or fallible operations exist on the path —
/// and the visible Arc swaps exactly once, last. The frame index advances
/// only because the reservation's Arc carries the checked next index.
pub fn commit_publication(
    owners: &mut PublicationOwners<'_>,
    reservation: PublicationReservation,
) -> Arc<PresentationFrame> {
    // The audio state: cancelled keys leave the committed set, insertions
    // join it, and the applied cancellations stage as the pending predicted
    // cancellations — the suppression that outlives the rejection metadata
    // this same commit consumes.
    for key in reservation.audio.cancellations() {
        owners.audio.committed.retain(|held| held != key);
        if !owners.audio.pending_cancellations.contains(key) {
            owners.audio.pending_cancellations.push(*key);
        }
    }
    owners
        .audio
        .committed
        .extend(reservation.audio.insertions().iter().copied());

    // The input projection: consume the published metadata prefix (admitted
    // entries first, then rejected) and the published local cue prefix, and
    // advance the sequence hint past the sequences the frame attests. The
    // frozen state exposes checked append and stage surfaces only, so the
    // survivors move through them into the replacement state.
    let input_count = reservation.consume.input_records();
    let taken = std::mem::take(&mut owners.input.projection);
    let admitted_offset = input_count.min(taken.admitted().len());
    let rejected_offset = input_count - admitted_offset;
    let cue_offset = reservation.consume.local_cues();
    let mut attested_hint = 0u64;
    for family in reservation.frame.families() {
        if let FamilyRecords::Input(records) = family.records() {
            for record in records {
                if let Some(sequence) = record.local_sequence() {
                    attested_hint = attested_hint.max(sequence);
                }
            }
        }
    }
    let hint = taken.sequence_hint().max(attested_hint.saturating_add(1));
    let mut survivors = InputProjectionState::try_new(hint).expect("checked input projection");
    for (sequence, kind) in &taken.admitted()[admitted_offset..] {
        survivors.record_admitted(*sequence, *kind);
    }
    for (sequence, class) in &taken.rejected()[rejected_offset..] {
        survivors.record_rejected(*sequence, *class);
    }
    survivors.stage_local_cues(&taken.pending_local_cues()[cue_offset..]);
    owners.input.projection = survivors;

    // The observation and lifecycle queues: consume exactly the published
    // prefixes.
    owners
        .observations
        .drain(..reservation.consume.observations());
    owners
        .lifecycle
        .pending
        .drain(..reservation.consume.lifecycle_records());

    // The visible Arc swaps exactly once, last.
    let frame = Arc::clone(&reservation.frame);
    *owners.visible = reservation.frame;
    frame
}
