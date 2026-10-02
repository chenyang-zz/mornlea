//! The audio-cue provenance family projection: semantic cue records beside
//! the dedup delta a successful publication would commit.
//!
//! This provider derives `audio-cues@1` records from exactly three real
//! source identities and never invents a fourth. Confirmed cues come from
//! the committed observation queue: the chat acknowledgment is the pilot's
//! UI-confirmation click class and the one cue packet carrying an actual
//! authoritative event id; the combat hit and the placement success carry no
//! universal event id in the source, so they attribute to their complete
//! `ObservationKey` and nothing else — two distinct combat observations are
//! two sounds, and no combat, projectile or attacker correlation absent from
//! the source is ever fabricated. Predicted cues are the pending steps of
//! the accepted prediction journal, each attributed to its input sequence
//! through the snow-step cue, so a correction that replays the journal keeps
//! the sequence and never replays the sound. Local cues are the native local
//! events the input projection state holds — read here without consuming
//! them, because only a successful publication consumes that queue.
//!
//! Dedup is epoch- and provenance-scoped over the proposed key families.
//! When a confirmed candidate and a held key both carry an actual event id,
//! epoch + id + cue decide and the observation key is attribution only;
//! otherwise the complete `ObservationKey` + cue decides, because no
//! universal id exists to correlate with. Predicted and local keys are
//! wholly salient, so equality is the rule and no cross-class match exists.
//! A rejected input cancels its pending predicted cue: the refused sequences
//! the input projection state records — and a retained rejection correction
//! names — are proposed back as cancellations together with the audio
//! owner's staged pending cancellations, and a cancelled candidate never
//! emits.
//!
//! The returned `AudioProjection` is a checked proposal, not a mutation: the
//! committed keys, the observation queue and the pending local cue queue are
//! immutable inputs, so a failed or repeated publication changes nothing and
//! the serial assembler alone commits the delta. Every record carries the
//! neutral checked playback gain and pitch — the measured pilot plays its
//! pre-synthesized cue PCM unscaled, and the device owner applies any later
//! attenuation — and no position, because no cue source in this core carries
//! world-space coordinates. No audio device concept exists on the projection
//! input at all, so an absent device still generates records. The candidate
//! records are checked through the accepted frame validator over a minimal
//! audio-only candidate before they are returned, so the family-record count
//! and frame byte caps reject typed with no partial vector.

use mornlea_domain::Event;

use crate::contracts::{ClientError, FAMILY_AUDIO_CUES, FamilyKey, FamilyOperation, RecordHeader};
use crate::input::ClientIntentKind;
use crate::presentation::frame::{
    AudioCueCategory, AudioCueRecord, FamilyFrame, FamilyRecords, PresentationFrame,
};
use crate::presentation::{
    AudioDedupDelta, AudioDedupKey, AudioProjection, CorrectionReason, CueId, CueProvenance,
    FinitePositive, FiniteUnit, ProjectionView,
};

/// The UI-confirmation click, the measured pilot's cue id zero: the click
/// class the pilot plays for command confirmations, which the chat
/// acknowledgment and the placement success publish as confirmed cues.
const CUE_UI_CLICK: u16 = 0;
/// The water-splash cue, the measured pilot's cue id four: the local fixture
/// the measured source cue inventory binds to the single admitted semantic
/// UI action with a native local audio source.
const CUE_WATER_SPLASH: u16 = 4;
/// The combat-hit cue, the measured pilot's cue id five.
const CUE_COMBAT_HIT: u16 = 5;
/// The snow-step cue, the measured pilot's cue id six: the movement footstep
/// class, the only predicted-provenance cue.
const CUE_SNOW_STEP: u16 = 6;

/// The neutral playback gain and pitch: the measured pilot plays its
/// pre-synthesized cue PCM unscaled, so the semantic record carries the
/// unit values and attenuation belongs to the device owner.
const PLAYBACK_GAIN: f32 = 1.0;
const PLAYBACK_PITCH: f32 = 1.0;

/// Wraps one registered cue id, rejecting anything outside the measured
/// inventory's registered set at the typed boundary this family owns.
fn cue(value: u16) -> Result<CueId, ClientError> {
    CueId::try_new(value)
}

/// One source-attributed cue candidate before the dedup rules decide
/// emission: its typed record payload beside the dedup key it would commit
/// and the observation's own optional source tick.
struct CueCandidate {
    cue: CueId,
    category: AudioCueCategory,
    provenance: CueProvenance,
    key: AudioDedupKey,
    source_tick: Option<u64>,
}

/// Whether two confirmed keys name the same acknowledged sound under the
/// salient-identity rule. When both carry an actual authoritative event id,
/// epoch + id + cue decide — the observation key is the attribution the
/// first emission carried, not the identity of the sound — otherwise the
/// complete `ObservationKey` + cue decides, because no universal id exists
/// to correlate id-less packets across observations.
fn confirmed_duplicate(candidate: &AudioDedupKey, held: &AudioDedupKey) -> bool {
    match (candidate, held) {
        (
            AudioDedupKey::Confirmed {
                epoch,
                authoritative_event_id: Some(id),
                cue,
                ..
            },
            AudioDedupKey::Confirmed {
                epoch: held_epoch,
                authoritative_event_id: Some(held_id),
                cue: held_cue,
                ..
            },
        ) => epoch == held_epoch && id == held_id && cue == held_cue,
        (
            AudioDedupKey::Confirmed {
                epoch,
                observation,
                authoritative_event_id: None,
                cue,
            },
            AudioDedupKey::Confirmed {
                epoch: held_epoch,
                observation: held_observation,
                authoritative_event_id: None,
                cue: held_cue,
            },
        ) => epoch == held_epoch && observation == held_observation && cue == held_cue,
        _ => false,
    }
}

/// Whether `candidate` duplicates any held key. Predicted and local keys are
/// wholly salient — every field is real identity — so structural equality is
/// their rule, and keys of different provenance classes never match, because
/// an unsupported cross-class correlation is exactly what this family must
/// not fabricate.
fn duplicates(candidate: &AudioDedupKey, held: &[AudioDedupKey]) -> bool {
    held.iter().any(|key| match (candidate, key) {
        (AudioDedupKey::Confirmed { .. }, AudioDedupKey::Confirmed { .. }) => {
            confirmed_duplicate(candidate, key)
        }
        _ => candidate == key,
    })
}

/// Projects the audio cue records and dedup delta of one immutable view, in
/// actual source order: confirmed observations first in observation order,
/// then the predicted journal in sequence order, then the pending local cue
/// events in native local event order.
///
/// Every emission passes the checked record constructor and the whole
/// candidate vector the accepted frame validator, so an over-cap proposal
/// rejects typed with no partial output, and the proposal never mutates the
/// committed audio state — the serial assembler commits the returned delta
/// only inside a successful publication.
pub fn project_audio(view: &ProjectionView<'_>) -> Result<AudioProjection, ClientError> {
    let epoch = view.frame_epoch();
    let player = view.player();
    if player.epoch() != epoch {
        // A player state from another epoch never enters this frame; the
        // reset path cleared the owner, so nothing stale may republish.
        return Err(ClientError::StaleEpoch);
    }

    let mut candidates: Vec<CueCandidate> = Vec::new();
    for observation in view.observations() {
        let key = *observation.key();
        if key.epoch() != epoch {
            // An observation from another epoch never enters this frame,
            // however valid its packet is; the whole projection rejects
            // rather than publishing a partial prefix.
            return Err(ClientError::StaleEpoch);
        }
        let event =
            Event::try_from(observation.packet().clone()).map_err(|_| ClientError::InvalidInput)?;
        let (cue_value, category, event_id) = match &event {
            // The chat acknowledgment is the confirmed UI-confirmation click
            // and the one cue packet with an actual authoritative event id;
            // the id names the acknowledged sound itself.
            Event::Chat(event) => (CUE_UI_CLICK, AudioCueCategory::Ui, Some(event.event_id())),
            // A combat hit carries no universal event id and no target
            // identity, so it attributes to its observation alone.
            Event::CombatHit(_) => (CUE_COMBAT_HIT, AudioCueCategory::Combat, None),
            // A placement success publishes the pilot's confirmation click;
            // the bucket-versus-block distinction needs a selected-stack
            // baseline this core's audio owner does not stage, so the
            // id-less click is the value that never fabricates a splash.
            Event::PlaceBlockSucceeded(_) => (CUE_UI_CLICK, AudioCueCategory::Ui, None),
            // Every other publication belongs to another family; no cue is
            // drawn from it.
            _ => continue,
        };
        let cue_id = cue(cue_value)?;
        candidates.push(CueCandidate {
            cue: cue_id,
            category,
            provenance: CueProvenance::Confirmed {
                observation: key,
                authoritative_event_id: event_id,
            },
            key: AudioDedupKey::Confirmed {
                epoch,
                observation: key,
                authoritative_event_id: event_id,
                cue: cue_id,
            },
            source_tick: observation.source_tick(),
        });
    }

    // The predicted movement cues: every still-pending journal step is a
    // snow-step candidate attributed to the input sequence the admission
    // owner issued for it, so the sequence — never a pose or a replay
    // instant — is the identity a correction replay cannot duplicate.
    for entry in player.journal() {
        let cue_id = cue(CUE_SNOW_STEP)?;
        candidates.push(CueCandidate {
            cue: cue_id,
            category: AudioCueCategory::Footstep,
            provenance: CueProvenance::Predicted {
                input_sequence: entry.sequence(),
            },
            key: AudioDedupKey::Predicted {
                epoch,
                input_sequence: entry.sequence(),
                cue: cue_id,
            },
            source_tick: None,
        });
    }

    // The local cues: the native local cue events the input projection state
    // holds, read without consuming them. The closed local cue-source set
    // admits exactly the bucket action, whose measured fixture is the water
    // splash; a cue-source kind outside that set is contract drift and
    // rejects typed instead of silently dropping a sound.
    for source in view.input().pending_local_cues() {
        let cue_value = match source.kind {
            ClientIntentKind::CollectWater => CUE_WATER_SPLASH,
            _ => return Err(ClientError::InvalidInput),
        };
        let cue_id = cue(cue_value)?;
        candidates.push(CueCandidate {
            cue: cue_id,
            category: AudioCueCategory::World,
            provenance: CueProvenance::Local {
                local_event_sequence: source.local_event_sequence,
            },
            key: AudioDedupKey::Local {
                epoch,
                local_event_sequence: source.local_event_sequence,
                cue: cue_id,
            },
            source_tick: None,
        });
    }

    // The cancellation set: the audio owner's staged pending cancellations,
    // plus one predicted cancellation per refused sequence the input
    // projection state records or a retained rejection correction names.
    // Snow step is the only predicted cue class, so the refused sequence's
    // cancelled attribution is exactly that key.
    let mut cancellations: Vec<AudioDedupKey> = view.audio().pending_cancellations().to_vec();
    let mut refused: Vec<u64> = view
        .input()
        .rejected()
        .iter()
        .map(|(sequence, _)| *sequence)
        .collect();
    if let Some(correction) = player.correction() {
        if correction.reason() == CorrectionReason::RejectedInput {
            refused.push(correction.last_input_sequence());
        }
    }
    refused.sort_unstable();
    refused.dedup();
    for sequence in refused {
        let key = AudioDedupKey::Predicted {
            epoch,
            input_sequence: sequence,
            cue: cue(CUE_SNOW_STEP)?,
        };
        if !cancellations.contains(&key) {
            cancellations.push(key);
        }
    }

    // Emission under the dedup rules: a candidate that duplicates a
    // committed key, an insertion already proposed inside this projection,
    // or a cancellation in this same delta never sounds and never commits a
    // second key.
    let mut records: Vec<AudioCueRecord> = Vec::new();
    let mut insertions: Vec<AudioDedupKey> = Vec::new();
    for candidate in candidates {
        if duplicates(&candidate.key, view.audio().committed())
            || duplicates(&candidate.key, &insertions)
            || cancellations.contains(&candidate.key)
        {
            continue;
        }
        let header = RecordHeader::try_new(
            epoch,
            view.frame_revision(),
            candidate.source_tick,
            FamilyOperation::Upsert,
        )?;
        records.push(AudioCueRecord::try_new(
            header,
            candidate.cue,
            candidate.provenance,
            candidate.category,
            None,
            FiniteUnit::try_new(PLAYBACK_GAIN)?,
            FinitePositive::try_new(PLAYBACK_PITCH)?,
        )?);
        insertions.push(candidate.key);
    }

    // The bounded-publication check: the whole candidate vector through the
    // accepted frame validator over a minimal audio-only candidate, so the
    // frozen family-record count and the configured frame byte bound decide
    // exactly as they will at publication. A rejection is typed and returns
    // no partial proposal.
    if !records.is_empty() {
        let candidate = PresentationFrame::try_new(
            epoch,
            view.frame_revision(),
            view.frame_index(),
            vec![FamilyFrame::try_new(
                FamilyKey::try_new(FAMILY_AUDIO_CUES)?,
                FamilyRecords::AudioCues(records.clone()),
            )?],
        )?;
        candidate.validate(view.limits())?;
    }

    AudioProjection::try_new(
        records,
        AudioDedupDelta::try_new(insertions, cancellations)?,
    )
}
