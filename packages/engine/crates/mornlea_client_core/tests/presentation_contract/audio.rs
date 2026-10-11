//! Audio-cue provenance contract tests.
//!
//! The table pins the provenance-aware audio family against the real mirror
//! provider, the real input admission path and the real prediction replay
//! owner. A confirmed chat acknowledgment carries its actual authoritative
//! event id and duplicates on epoch + id + cue, while a combat hit or a
//! placement success — packets the source never gives a universal event id —
//! deduplicates on the complete `ObservationKey` alone, so two distinct combat
//! observations both sound and no combat, projectile or attacker correlation
//! absent from the source is ever fabricated. A predicted snow step is
//! attributed to its input sequence, emits once, and a correction that
//! replays the journal never replays the sound; a rejected input cancels its
//! pending predicted cue through the proposed dedup delta. A local cue exists
//! only as the native local event a real admitted semantic UI action staged,
//! never fabricated from admitted actions alone. Dedup keys are epoch-scoped,
//! so a reset that clears the owner's keys — or even an owner that wrongly
//! retained old-epoch keys — cannot silence the fresh epoch. No audio device
//! concept exists on the projection input at all, so an absent device still
//! generates cue records. The closed cue set, the neutral checked gain/pitch
//! and the family-record and frame-byte caps reject typed with no partial
//! output. The failed-publication case drives the deterministic publication
//! double: a frame that fails consumes no source event and no cancellation,
//! commits no dedup key, a valid retry emits once, and only the successful
//! publication commits the key so later projections stay silent.

use std::num::NonZeroU64;

use super::support::{ContractDouble, DoubleMode};
use mornlea_client_core::input::InputProjectionState;
use mornlea_client_core::prediction::{PredictionReplay, StepEnvironment};
use mornlea_client_core::presentation::family_audio::project_audio;
use mornlea_client_core::presentation::{Pose, ProducerIdentity};
use mornlea_client_core::session::mirror::MirrorProvider;
use mornlea_client_core::{
    AcceptedObservation, AudioDedupDelta, AudioDedupKey, AudioProjectionState, ClientError,
    ClientIdentity, ClientIntentKind, ClientLimits, ClientWorkBudget, ConfirmedMirror,
    ConfirmedRevision, CorrectionReason, CueId, CueProvenance, DiagnosticProjectionState, Endpoint,
    ErrorClassCounters, FinitePositive, FiniteUnit, InputAction, InputAdmissionState, InputBatch,
    InputTranslator, LifecycleProjectionState, MovementIntent, ObservationKey, ProjectionView,
    QueueCounters, SessionEpoch,
};
use mornlea_domain::{
    ChatBody, ChatEvent, ChatEventParts, CombatHit, CombatTarget, Dimension, DisplayName, Event,
    FiniteVec3, HeldActions, LookAngles, MiningState, MotionState, MotionStateParts, Movement,
    PlacementSuccess, PlayerControl, PlayerControlParts, PlayerId, PlayerState, PlayerStateParts,
    Season, SurvivalState, SurvivalStateParts, Weather, WorldState, WorldStateParts,
};
use mornlea_engine::native::contracts::{Aabb, CollisionCell, CollisionGrid};
use mornlea_protocol::{LoginStart, ServerPacket};

/// The epoch every fixture mirror, replay owner and observation key carries.
const EPOCH: u64 = 7;

// The walking fixture grid, the same shape the accepted prediction replay
// cases use: one full-cube floor layer at the grid's lowest y (one block
// below the walking plane) and air above it. Cells are laid out in the
// native physics wrapper's order: y-major, then x, then z.
const WALK_ORIGIN: [i32; 3] = [-4, -1, -8];
const WALK_DIMS: [u32; 3] = [9, 6, 17];

fn fixture_cells(dims: [u32; 3]) -> Vec<CollisionCell> {
    let cube = Aabb {
        minimum: [0.0; 3],
        maximum: [1.0, 1.0, 1.0],
    };
    let empty = Aabb {
        minimum: [0.0; 3],
        maximum: [0.0; 3],
    };
    let mut cells = Vec::new();
    for y in 0..dims[1] {
        for _x in 0..dims[0] {
            for _z in 0..dims[2] {
                let floor = y == 0;
                let mut boxes = [empty; 8];
                let used = if floor {
                    boxes[0] = cube;
                    1u8
                } else {
                    0u8
                };
                cells.push(CollisionCell::try_new(true, boxes, used).expect("checked cell"));
            }
        }
    }
    cells
}

fn walking_grid(cells: &[CollisionCell]) -> CollisionGrid<'_> {
    CollisionGrid::try_new(WALK_ORIGIN, WALK_DIMS, cells).expect("walking fixture grid")
}

fn epoch(value: u64) -> SessionEpoch {
    SessionEpoch::try_new(value).expect("nonzero epoch")
}

fn limits() -> ClientLimits {
    ClientLimits::try_new().expect("frozen limits")
}

/// The frozen limits with one configured family-record or frame-byte cap, the
/// accepted tighter-configuration proof technique for the plus-one rows.
fn limits_with(family_records: usize, frame_bytes: usize) -> ClientLimits {
    let frozen = limits();
    ClientLimits::try_new_with(
        frozen.queued_input_events(),
        frozen.inbound_observations(),
        frozen.inbound_bytes(),
        frozen.outbound_commands(),
        frozen.outbound_bytes(),
        frozen.prediction_journal(),
        frozen.message_work(),
        frozen.mesh_work(),
        frozen.preparation_results(),
        frozen.preparation_bytes(),
        family_records,
        frame_bytes,
    )
    .expect("cap at or below the frozen ceiling")
}

fn look(yaw: f32, pitch: f32) -> LookAngles {
    LookAngles::try_new(yaw, pitch).expect("finite fixture look")
}

/// One neutral forward-walking control, the shape the accepted replay cases
/// step with.
fn forward(yaw: f32) -> PlayerControl {
    PlayerControl::new(PlayerControlParts {
        movement: Movement {
            move_x: 0,
            move_z: 1,
            jump: false,
        },
        look: look(yaw, 0.0),
        actions: HeldActions {
            primary: false,
            eating: false,
            sprinting: false,
            sneaking: false,
        },
    })
}

fn player(byte: u8) -> PlayerId {
    PlayerId::try_from_bytes([0, 0, 0, 0, 0, 0, 0x40, 0, 0x80, 0, 0, 0, 0, 0, 0, byte])
        .expect("uuid v4")
}

/// One confirmed chat acknowledgment with its actual event id, the branch
/// that addresses no companion.
fn chat(event_id: u64) -> Event {
    Event::Chat(
        ChatEvent::try_new(ChatEventParts {
            event_id,
            player_id: player(3),
            player_name: DisplayName::try_from_canonical("pilot".to_string())
                .expect("canonical fixture name"),
            body: ChatBody::InvalidFormat,
        })
        .expect("chat event"),
    )
}

fn combat(server_tick: u64) -> Event {
    Event::CombatHit(CombatHit::try_new(server_tick, 3, CombatTarget::Hostile).expect("combat hit"))
}

fn placement(sequence: u64) -> Event {
    Event::PlaceBlockSucceeded(PlacementSuccess::new(sequence))
}

fn packet(event: Event) -> ServerPacket {
    ServerPacket::try_from(event).expect("publication converts to its packet")
}

fn survival() -> SurvivalState {
    SurvivalState::try_new(SurvivalStateParts {
        health: 20,
        oxygen: 300,
        hunger: 20,
        saturation_zero: false,
        armor_points: 0,
    })
    .expect("checked survival fixture")
}

fn world() -> WorldState {
    WorldState::try_new(WorldStateParts {
        day_phase_offset: 0,
        world_time_ticks: 0,
        weather: Weather::Clear,
        season: Season::Spring,
        season_progress: 0,
        temperature: 0,
    })
    .expect("checked world fixture")
}

/// One overworld authority record at the caller's tick, sequence and motion.
#[allow(clippy::too_many_arguments)]
fn authority(tick: u64, last_sequence: u64, position: [f32; 3], ready: bool) -> PlayerState {
    PlayerState::new(PlayerStateParts {
        server_tick: tick,
        last_input_sequence: last_sequence,
        dimension: Dimension::OVERWORLD,
        motion: MotionState::new(MotionStateParts {
            position: FiniteVec3::try_new(position).expect("finite fixture position"),
            velocity: FiniteVec3::try_new([0.0, 0.0, 0.0]).expect("finite fixture velocity"),
            on_ground: true,
        }),
        look: look(0.2, -0.1),
        ready,
        reset: false,
        mining: MiningState::Idle,
        survival: survival(),
        world: world(),
    })
}

/// An admitted real mirror provider with no observations yet.
fn admitted_mirror(value: u64) -> MirrorProvider {
    let mut provider =
        MirrorProvider::new(epoch(value), limits()).expect("admitted mirror provider");
    provider.admit().expect("admitted session");
    provider
}

/// Commits one publication through the real mirror provider and returns the
/// confirmed revision it issued beside the actual source key it retained.
fn commit(
    provider: &mut MirrorProvider,
    value: u64,
    event: Event,
    ordinal: u32,
) -> (ConfirmedRevision, ObservationKey) {
    let next = provider.mirror().revision().get() + 1;
    let key = ObservationKey::try_new(epoch(value), ConfirmedRevision::new(next), ordinal)
        .expect("staged key");
    let staged = AcceptedObservation::try_new(key, None, packet(event), Vec::new())
        .expect("staged observation");
    let revision = provider.commit(&staged).expect("committed observation");
    (revision, key)
}

/// The projection-view state owners a fixture keeps alive beside the
/// borrowed mirror, observation queue and player projection state.
struct ViewFixture {
    input: InputProjectionState,
    audio: AudioProjectionState,
    lifecycle: LifecycleProjectionState,
    diagnostics: DiagnosticProjectionState,
    limits: ClientLimits,
}

impl ViewFixture {
    fn new() -> Self {
        Self {
            input: InputProjectionState::try_new(1).expect("input projection state"),
            audio: AudioProjectionState::try_new().expect("audio projection state"),
            lifecycle: LifecycleProjectionState::try_new(1).expect("lifecycle state"),
            diagnostics: DiagnosticProjectionState::try_new(
                ProducerIdentity::try_new([0u8; 20], [0u8; 32]).expect("producer identity"),
                QueueCounters::default(),
                ErrorClassCounters::default(),
            )
            .expect("diagnostics projection state"),
            limits: limits(),
        }
    }

    fn with_limits(limits: ClientLimits) -> Self {
        Self {
            limits,
            ..Self::new()
        }
    }

    fn with_audio(audio: AudioProjectionState) -> Self {
        Self {
            audio,
            ..Self::new()
        }
    }

    /// Builds one immutable projection view over the supplied player state,
    /// mirror and observation queue, at the coherent candidate revision.
    #[allow(clippy::too_many_arguments)]
    fn view<'a>(
        &'a self,
        player: &'a mornlea_client_core::prediction::PlayerProjectionState,
        mirror: &'a ConfirmedMirror,
        observations: &'a [AcceptedObservation],
        value: u64,
        revision: ConfirmedRevision,
    ) -> ProjectionView<'a> {
        self.view_with_input(&self.input, player, mirror, observations, value, revision)
    }

    /// Builds one immutable projection view over an externally owned input
    /// projection state — the admission owner's exposed state — beside this
    /// fixture's other owners.
    #[allow(clippy::too_many_arguments)]
    fn view_with_input<'a>(
        &'a self,
        input: &'a InputProjectionState,
        player: &'a mornlea_client_core::prediction::PlayerProjectionState,
        mirror: &'a ConfirmedMirror,
        observations: &'a [AcceptedObservation],
        value: u64,
        revision: ConfirmedRevision,
    ) -> ProjectionView<'a> {
        ProjectionView::try_new(
            mirror,
            observations,
            input,
            player,
            &self.audio,
            &self.lifecycle,
            &self.diagnostics,
            epoch(value),
            revision,
            5,
            &self.limits,
        )
        .expect("projection view")
    }

    /// The candidate view of one provider's committed state: the next
    /// revision the coherent frame would carry.
    fn view_of<'a>(
        &'a self,
        player: &'a mornlea_client_core::prediction::PlayerProjectionState,
        provider: &'a MirrorProvider,
        value: u64,
    ) -> ProjectionView<'a> {
        let revision = ConfirmedRevision::new(provider.mirror().revision().get() + 1);
        self.view(
            player,
            provider.mirror(),
            provider.observations(),
            value,
            revision,
        )
    }
}

/// One directly constructed checked player state with an empty journal: the
/// frame-coherent stand-in for tests that project confirmed or local cues
/// only.
fn idle_player(value: u64) -> mornlea_client_core::prediction::PlayerProjectionState {
    use mornlea_client_core::prediction::PlayerProjectionState;
    PlayerProjectionState::try_new(
        epoch(value),
        ConfirmedRevision::new(1),
        Pose::try_new([0.5, 0.0, 0.5], 0.2, -0.1).expect("finite fixture pose"),
        None,
        Vec::new(),
        None,
        None,
        MovementIntent::try_new(None, true).expect("checked movement intent"),
    )
    .expect("checked player projection state")
}

/// The first row: a confirmed chat acknowledgment carries its actual
/// authoritative event id, the first occurrence in source order emits exactly
/// one record, and a second observation of the same actual id is the same
/// acknowledged sound — suppressed inside one projection and across
/// publications by the committed key — while a different actual id is a
/// different acknowledgment that still sounds.
#[test]
fn confirmed_chat_event_duplicates_on_actual_id() {
    let mut provider = admitted_mirror(EPOCH);
    let (_, first_key) = commit(&mut provider, EPOCH, chat(99), 0);
    let (_, second_key) = commit(&mut provider, EPOCH, chat(99), 1);

    let player = idle_player(EPOCH);
    let fixture = ViewFixture::new();
    let view = fixture.view_of(&player, &provider, EPOCH);
    let projection = project_audio(&view).expect("the chat acknowledgment projects its cue");
    assert_eq!(
        projection.records().len(),
        1,
        "the duplicate id sounds once"
    );
    let record = &projection.records()[0];
    assert_eq!(record.cue_id(), CueId::try_new(0).expect("ui click cue"));
    assert_eq!(
        record.provenance(),
        &CueProvenance::Confirmed {
            observation: first_key,
            authoritative_event_id: Some(99),
        },
        "the emitted record is the first occurrence in source order"
    );
    assert_ne!(
        *record.provenance(),
        CueProvenance::Confirmed {
            observation: second_key,
            authoritative_event_id: Some(99),
        },
        "the suppressed duplicate is never the emitted attribution"
    );
    let key = AudioDedupKey::Confirmed {
        epoch: epoch(EPOCH),
        observation: first_key,
        authoritative_event_id: Some(99),
        cue: CueId::try_new(0).expect("ui click cue"),
    };
    assert_eq!(projection.proposed_dedup().insertions(), &[key]);

    // Once the publication committed the key, replaying both observations
    // through the same owner stays silent.
    let committed = ViewFixture::with_audio(
        AudioProjectionState::try_new()
            .expect("audio state")
            .with_committed(vec![key]),
    );
    let silent = committed.view_of(&player, &provider, EPOCH);
    let projection = project_audio(&silent).expect("still a valid projection");
    assert!(
        projection.records().is_empty(),
        "the committed actual id suppresses both occurrences"
    );
    assert!(projection.proposed_dedup().insertions().is_empty());

    // A different actual id is a different acknowledged sound.
    let mut provider = admitted_mirror(EPOCH);
    commit(&mut provider, EPOCH, chat(99), 0);
    commit(&mut provider, EPOCH, chat(100), 1);
    let view = committed.view_of(&player, &provider, EPOCH);
    let projection = project_audio(&view).expect("valid projection");
    assert_eq!(projection.records().len(), 1);
    assert_eq!(
        projection.records()[0].provenance(),
        &CueProvenance::Confirmed {
            observation: *provider.observations()[1].key(),
            authoritative_event_id: Some(100),
        }
    );
}

/// Combat hits and placement successes never carry a universal event id, so
/// each deduplicates on its complete `ObservationKey` alone: a replayed
/// observation is silent while two distinct combat observations both sound —
/// no combat, projectile or attacker correlation absent from the source is
/// fabricated.
#[test]
fn combat_and_place_without_id_dedup_by_observation_key() {
    let mut provider = admitted_mirror(EPOCH);
    let (_, combat_first) = commit(&mut provider, EPOCH, combat(5), 0);
    let (_, place_key) = commit(&mut provider, EPOCH, placement(4), 1);
    let (_, combat_second) = commit(&mut provider, EPOCH, combat(6), 2);

    let player = idle_player(EPOCH);
    let fixture = ViewFixture::new();
    let view = fixture.view_of(&player, &provider, EPOCH);
    let projection = project_audio(&view).expect("each observation carries its cue");
    assert_eq!(
        projection.records().len(),
        3,
        "distinct observations all sound; nothing correlates them"
    );

    let hit = &projection.records()[0];
    assert_eq!(hit.cue_id(), CueId::try_new(5).expect("combat hit cue"));
    assert_eq!(
        hit.category(),
        &mornlea_client_core::AudioCueCategory::Combat
    );
    assert_eq!(
        hit.provenance(),
        &CueProvenance::Confirmed {
            observation: combat_first,
            authoritative_event_id: None,
        }
    );
    assert_eq!(
        hit.header().source_tick(),
        Some(5),
        "the combat observation's own server tick survives"
    );

    let placed = &projection.records()[1];
    assert_eq!(placed.cue_id(), CueId::try_new(0).expect("ui click cue"));
    assert_eq!(
        placed.category(),
        &mornlea_client_core::AudioCueCategory::Ui
    );
    assert_eq!(
        placed.provenance(),
        &CueProvenance::Confirmed {
            observation: place_key,
            authoritative_event_id: None,
        }
    );
    assert_eq!(placed.header().source_tick(), None);
    assert_eq!(
        projection.records()[2].provenance(),
        &CueProvenance::Confirmed {
            observation: combat_second,
            authoritative_event_id: None,
        }
    );

    // Committing one observation's key silences exactly that observation.
    let first_key = AudioDedupKey::Confirmed {
        epoch: epoch(EPOCH),
        observation: combat_first,
        authoritative_event_id: None,
        cue: CueId::try_new(5).expect("combat hit cue"),
    };
    let committed = ViewFixture::with_audio(
        AudioProjectionState::try_new()
            .expect("audio state")
            .with_committed(vec![first_key]),
    );
    let view = committed.view_of(&player, &provider, EPOCH);
    let projection = project_audio(&view).expect("valid projection");
    assert_eq!(
        projection.records().len(),
        2,
        "the replayed combat observation is silent; the unrelated cues still sound"
    );

    // Committing every key silences the whole set.
    let all = projection
        .proposed_dedup()
        .insertions()
        .iter()
        .chain(std::iter::once(&first_key))
        .copied()
        .collect::<Vec<_>>();
    let silent = ViewFixture::with_audio(
        AudioProjectionState::try_new()
            .expect("audio state")
            .with_committed(all),
    );
    let view = silent.view_of(&player, &provider, EPOCH);
    let projection = project_audio(&view).expect("valid projection");
    assert!(projection.records().is_empty());
}

/// The predicted snow step is attributed to its input sequence through the
/// real prediction journal: it emits once, a correction that replays the
/// still-unconfirmed journal never replays the sound, and only a fresh
/// sequence sounds again.
#[test]
fn predicted_snow_step_emits_once_across_correction_replay() {
    let cells = fixture_cells(WALK_DIMS);
    let grid = walking_grid(&cells);
    let environment = StepEnvironment::new(grid, false);
    let mut provider = admitted_mirror(EPOCH);
    let mut replay = PredictionReplay::try_new(epoch(EPOCH), limits()).expect("replay owner");

    let begin = authority(1, 0, [0.5, 0.0, 0.5], true);
    let (_, key) = commit(&mut provider, EPOCH, Event::PlayerState(begin), 0);
    replay
        .apply_confirmed(&key, &begin, &environment)
        .expect("the first authority begins prediction");
    replay.begin_frame();
    replay
        .predict_step(1, forward(0.4), &environment)
        .expect("journaled prediction step");

    let player = replay.projection().expect("attributable state");
    let fixture = ViewFixture::new();
    let view = fixture.view_of(&player, &provider, EPOCH);
    let projection = project_audio(&view).expect("the pending step carries its cue");
    assert_eq!(projection.records().len(), 1);
    let record = &projection.records()[0];
    assert_eq!(record.cue_id(), CueId::try_new(6).expect("snow step cue"));
    assert_eq!(
        record.category(),
        &mornlea_client_core::AudioCueCategory::Footstep
    );
    assert_eq!(
        record.provenance(),
        &CueProvenance::Predicted { input_sequence: 1 }
    );
    assert_eq!(record.header().source_tick(), None);
    let step_key = AudioDedupKey::Predicted {
        epoch: epoch(EPOCH),
        input_sequence: 1,
        cue: CueId::try_new(6).expect("snow step cue"),
    };
    assert_eq!(projection.proposed_dedup().insertions(), &[step_key]);

    // A successful publication commits the sequence key.
    let committed = ViewFixture::with_audio(
        AudioProjectionState::try_new()
            .expect("audio state")
            .with_committed(vec![step_key]),
    );

    // A newer authority that acknowledges nothing replays the journal: the
    // replayed step keeps its sequence and its cue stays silent.
    let corrected = authority(2, 0, [0.6, 0.0, 0.6], true);
    let (_, second) = commit(&mut provider, EPOCH, Event::PlayerState(corrected), 1);
    let witness = replay
        .apply_confirmed(&second, &corrected, &environment)
        .expect("the correction replays the journal");
    assert_eq!(witness.acknowledged(), 0);
    assert_eq!(witness.replayed(), 1);

    let player = replay.projection().expect("attributable state");
    let view = committed.view_of(&player, &provider, EPOCH);
    let projection = project_audio(&view).expect("valid projection");
    assert!(
        projection.records().is_empty(),
        "the correction replay never replays the sound"
    );

    // Only a fresh sequence sounds again.
    replay.begin_frame();
    replay
        .predict_step(2, forward(0.4), &environment)
        .expect("fresh journaled step");
    let player = replay.projection().expect("attributable state");
    let view = committed.view_of(&player, &provider, EPOCH);
    let projection = project_audio(&view).expect("valid projection");
    assert_eq!(projection.records().len(), 1);
    assert_eq!(
        projection.records()[0].provenance(),
        &CueProvenance::Predicted { input_sequence: 2 }
    );
}

/// A rejected input cancels its pending predicted cue: the real rejection
/// removal publishes the attribution key, the input projection state records
/// the refused sequence, and the next projection proposes the cancellation —
/// never a new record for the refused step.
#[test]
fn rejected_input_cancels_its_pending_predicted_cue() {
    let cells = fixture_cells(WALK_DIMS);
    let grid = walking_grid(&cells);
    let environment = StepEnvironment::new(grid, false);
    let mut provider = admitted_mirror(EPOCH);
    let mut replay = PredictionReplay::try_new(epoch(EPOCH), limits()).expect("replay owner");

    let begin = authority(1, 0, [0.5, 0.0, 0.5], true);
    let (_, key) = commit(&mut provider, EPOCH, Event::PlayerState(begin), 0);
    replay
        .apply_confirmed(&key, &begin, &environment)
        .expect("begins");
    replay.begin_frame();
    replay
        .predict_step(1, forward(0.4), &environment)
        .expect("journaled prediction step");

    let player = replay.projection().expect("attributable state");
    let fixture = ViewFixture::new();
    let view = fixture.view_of(&player, &provider, EPOCH);
    let projection = project_audio(&view).expect("the pending step carries its cue");
    let step_key = AudioDedupKey::Predicted {
        epoch: epoch(EPOCH),
        input_sequence: 1,
        cue: CueId::try_new(6).expect("snow step cue"),
    };
    assert_eq!(projection.proposed_dedup().insertions(), &[step_key]);

    // The publication committed the sequence key, then the authority refused
    // the input: the replay owner removes the pending prediction and the
    // input projection state records the refused sequence.
    let rejected = replay
        .reject(1, &environment)
        .expect("the rejection removes the pending prediction");
    assert_eq!(
        rejected,
        Some(
            mornlea_client_core::prediction::RejectedPrediction::try_new(epoch(EPOCH), 1)
                .expect("attribution key")
        )
    );
    let player = replay.projection().expect("attributable state");
    assert_eq!(
        player
            .correction()
            .expect("the rejection published a correction")
            .reason(),
        CorrectionReason::RejectedInput
    );

    let mut input = InputProjectionState::try_new(1).expect("input projection state");
    input.record_rejected(1, ClientError::InvalidInput);
    let committed = AudioProjectionState::try_new()
        .expect("audio state")
        .with_committed(vec![step_key]);
    let committed_fixture = ViewFixture {
        input,
        audio: committed,
        ..ViewFixture::new()
    };
    let view = committed_fixture.view_of(&player, &provider, EPOCH);
    let projection = project_audio(&view).expect("valid projection");
    assert!(
        projection.records().is_empty(),
        "the refused step never sounds again"
    );
    assert_eq!(
        projection.proposed_dedup().cancellations(),
        &[step_key],
        "the rejection cancels the committed cue attribution exactly once"
    );
    assert!(
        projection.proposed_dedup().insertions().is_empty(),
        "a cancellation is never also an insertion"
    );
}

/// A local cue exists only as the native local event a real admitted
/// semantic UI action staged: the real admission path emits exactly one
/// `CollectWater` cue source, the projection reads the input projection
/// state's pending local events without consuming the admission owner's
/// queue, and it fabricates no local cue from admitted actions alone when no
/// native local event is staged.
#[test]
fn local_cue_requires_a_real_native_local_event() {
    let provider = admitted_mirror(EPOCH);
    let mut admission = InputAdmissionState::try_new(epoch(EPOCH), limits()).expect("owner");

    let action = InputAction {
        intent: mornlea_client_core::ClientIntent::CollectWater(look(0.0, 0.0)),
        container: None,
        crafting: None,
    };
    let batch = InputBatch::try_new(epoch(EPOCH), vec![action]).expect("batch");
    let validated =
        InputTranslator::validate_batch(&batch, provider.mirror(), &limits()).expect("validated");
    InputTranslator::commit(validated, &mut admission).expect("admitted");
    assert_eq!(
        admission.pending_local_cues(),
        &[mornlea_client_core::LocalCueSource {
            local_event_sequence: 1,
            kind: ClientIntentKind::CollectWater,
        }],
        "the real admitted semantic UI action stages its native local event"
    );

    // The projection reads the input projection state's pending local cue
    // events. An admitted action alone is not a local cue: without the
    // native local event staged on the projection input, nothing sounds and
    // no local dedup key is proposed.
    let mut input = InputProjectionState::try_new(1).expect("input projection state");
    input.record_admitted(1, ClientIntentKind::CollectWater);
    let fixture = ViewFixture {
        input,
        ..ViewFixture::new()
    };
    let player = idle_player(EPOCH);
    let view = fixture.view_of(&player, &provider, EPOCH);
    let projection = project_audio(&view).expect("valid projection");
    assert!(
        projection.records().is_empty(),
        "no local cue is fabricated from the admitted action alone"
    );
    assert!(projection.proposed_dedup().insertions().is_empty());
    assert!(projection.proposed_dedup().cancellations().is_empty());

    // The projection is pure: reading it consumed nothing of the admission
    // owner's pending native local event queue.
    assert_eq!(admission.pending_local_cues().len(), 1);
}

/// The end-to-end local-provenance row: one real admitted `CollectWater`
/// semantic UI action drives the admission owner's exposed projection
/// state, and the projection built from that owner emits exactly one
/// `Local`-provenance water-splash record keyed by the native local event
/// sequence — never the input sequence, never a server confirmation. Reads
/// consume nothing, so a second projection before any commit still sees the
/// pending cue, and once the Local key is committed the same pending cue is
/// a duplicate that stays silent.
#[test]
fn local_cue_projects_end_to_end_from_real_admission() {
    let provider = admitted_mirror(EPOCH);
    let mut admission = InputAdmissionState::try_new(epoch(EPOCH), limits()).expect("owner");

    let action = InputAction {
        intent: mornlea_client_core::ClientIntent::CollectWater(look(0.0, 0.0)),
        container: None,
        crafting: None,
    };
    let batch = InputBatch::try_new(epoch(EPOCH), vec![action]).expect("batch");
    let validated =
        InputTranslator::validate_batch(&batch, provider.mirror(), &limits()).expect("validated");
    InputTranslator::commit(validated, &mut admission).expect("admitted");
    assert_eq!(
        admission.projection().pending_local_cues(),
        &[mornlea_client_core::LocalCueSource {
            local_event_sequence: 1,
            kind: ClientIntentKind::CollectWater,
        }],
        "the admission owner's exposed projection state carries the native event"
    );

    // The view borrows the admission owner's exposed projection state — the
    // same input surface the serial assembler borrows at publication time.
    let player = idle_player(EPOCH);
    let fixture = ViewFixture::new();
    let view = fixture.view_with_input(
        admission.projection(),
        &player,
        provider.mirror(),
        provider.observations(),
        EPOCH,
        ConfirmedRevision::new(1),
    );
    let projection = project_audio(&view).expect("the admitted action's native cue projects");
    assert_eq!(projection.records().len(), 1);
    let record = &projection.records()[0];
    assert_eq!(
        record.cue_id(),
        CueId::try_new(4).expect("water splash cue"),
        "the bucket action's measured local fixture is the water splash"
    );
    assert_eq!(
        record.category(),
        &mornlea_client_core::AudioCueCategory::World
    );
    assert_eq!(
        record.provenance(),
        &CueProvenance::Local {
            local_event_sequence: 1,
        }
    );
    assert_eq!(record.header().source_tick(), None);
    let local_key = AudioDedupKey::Local {
        epoch: epoch(EPOCH),
        local_event_sequence: 1,
        cue: CueId::try_new(4).expect("water splash cue"),
    };
    assert_eq!(projection.proposed_dedup().insertions(), &[local_key]);
    assert!(projection.proposed_dedup().cancellations().is_empty());

    // Reads never consume: the exposed cue queue is unchanged and a second
    // projection before any commit still sees the pending cue.
    assert_eq!(admission.projection().pending_local_cues().len(), 1);
    let second = project_audio(&view).expect("valid projection");
    assert_eq!(
        second.records().len(),
        1,
        "the unconsumed pending cue still projects"
    );
    assert_eq!(
        admission.projection().pending_local_cues().len(),
        1,
        "projecting consumed nothing of the owner's exposed queue"
    );

    // A committed Local key silences the same cue: the native local event
    // sequence is its whole identity, so the still-pending event is a
    // duplicate after the publication committed the key.
    let committed = ViewFixture::with_audio(
        AudioProjectionState::try_new()
            .expect("audio state")
            .with_committed(vec![local_key]),
    );
    let view = committed.view_with_input(
        admission.projection(),
        &player,
        provider.mirror(),
        provider.observations(),
        EPOCH,
        ConfirmedRevision::new(1),
    );
    let projection = project_audio(&view).expect("valid projection");
    assert!(
        projection.records().is_empty(),
        "the committed local key silences the cue"
    );
    assert!(projection.proposed_dedup().insertions().is_empty());
    assert_eq!(
        admission.projection().pending_local_cues().len(),
        1,
        "the silencing read consumed nothing either"
    );
}

/// Dedup keys are epoch-scoped: after a reset the fresh epoch's owner starts
/// with cleared keys, and even an owner that wrongly retained the old
/// epoch's committed keys cannot silence the fresh epoch's cue, because the
/// same actual event id in a new epoch is a new acknowledgment.
#[test]
fn reset_clears_epoch_scoped_dedup_keys() {
    let mut first = admitted_mirror(EPOCH);
    let (_, old_key_value) = commit(&mut first, EPOCH, chat(99), 0);
    let player = idle_player(EPOCH);
    let fixture = ViewFixture::new();
    let view = fixture.view_of(&player, &first, EPOCH);
    let projection = project_audio(&view).expect("the epoch emits its cue");
    assert_eq!(projection.records().len(), 1);
    let old_key = AudioDedupKey::Confirmed {
        epoch: epoch(EPOCH),
        observation: old_key_value,
        authoritative_event_id: Some(99),
        cue: CueId::try_new(0).expect("ui click cue"),
    };
    assert_eq!(projection.proposed_dedup().insertions(), &[old_key]);

    // Reset: the fresh epoch restarts at a new identity. The reset owner's
    // cleared keys emit again, and an owner that wrongly retained the old
    // keys still emits, because the retained key's epoch never matches.
    const NEXT: u64 = EPOCH + 1;
    let mut fresh = admitted_mirror(NEXT);
    commit(&mut fresh, NEXT, chat(99), 0);
    let player = idle_player(NEXT);

    let cleared = ViewFixture::new();
    let view = cleared.view_of(&player, &fresh, NEXT);
    let projection = project_audio(&view).expect("valid projection");
    assert_eq!(
        projection.records().len(),
        1,
        "the reset owner's cleared keys cannot silence the fresh epoch"
    );
    assert_eq!(
        projection.proposed_dedup().insertions()[0],
        AudioDedupKey::Confirmed {
            epoch: epoch(NEXT),
            observation: *fresh.observations()[0].key(),
            authoritative_event_id: Some(99),
            cue: CueId::try_new(0).expect("ui click cue"),
        },
        "the fresh key carries the fresh epoch"
    );

    let retained = ViewFixture::with_audio(
        AudioProjectionState::try_new()
            .expect("audio state")
            .with_committed(vec![old_key]),
    );
    let view = retained.view_of(&player, &fresh, NEXT);
    let projection = project_audio(&view).expect("valid projection");
    assert_eq!(
        projection.records().len(),
        1,
        "an old-epoch key never suppresses a fresh-epoch cue"
    );
}

/// The projection input owns no audio device concept at all, so an absent
/// device still generates the same semantic cue record.
#[test]
fn absent_device_still_generates_cue_records() {
    let mut provider = admitted_mirror(EPOCH);
    commit(&mut provider, EPOCH, combat(9), 0);
    let player = idle_player(EPOCH);
    let fixture = ViewFixture::new();
    let view = fixture.view_of(&player, &provider, EPOCH);
    let projection = project_audio(&view).expect("device absence changes nothing");
    assert_eq!(projection.records().len(), 1);
    assert_eq!(
        projection.records()[0].cue_id(),
        CueId::try_new(5).expect("combat hit cue")
    );
}

/// The closed cue set, the checked gain and pitch domains and the
/// family-record and frame-byte caps all reject typed, with no partial
/// output. The cue ids are exactly the measured inventory's registered set
/// and every emitted record carries the neutral checked playback values.
#[test]
fn unknown_cue_invalid_gain_pitch_and_caps_reject() {
    for id in 0..=CueId::MAX_REGISTERED {
        assert!(CueId::try_new(id).is_ok(), "registered cue id {id}");
    }
    assert_eq!(
        CueId::try_new(CueId::MAX_REGISTERED + 1),
        Err(ClientError::InvalidInput),
        "the cue set is closed at the registered ceiling"
    );
    assert_eq!(CueId::try_new(u16::MAX), Err(ClientError::InvalidInput));

    for bad in [f32::NAN, f32::INFINITY, -0.5, 1.5] {
        assert_eq!(FiniteUnit::try_new(bad), Err(ClientError::InvalidInput));
    }
    assert!(FiniteUnit::try_new(0.0).is_ok());
    assert!(FiniteUnit::try_new(1.0).is_ok());
    for bad in [f32::NAN, f32::INFINITY, 0.0, -1.0] {
        assert_eq!(
            FinitePositive::try_new(bad),
            Err(ClientError::InvalidInput),
            "pitch is strictly positive and finite"
        );
    }
    assert!(FinitePositive::try_new(1.0).is_ok());

    // Every emitted record carries the neutral checked playback values the
    // measured pilot plays its pre-synthesized cues at.
    let mut provider = admitted_mirror(EPOCH);
    commit(&mut provider, EPOCH, combat(9), 0);
    let player = idle_player(EPOCH);
    let fixture = ViewFixture::new();
    let view = fixture.view_of(&player, &provider, EPOCH);
    let projection = project_audio(&view).expect("valid projection");
    let record = &projection.records()[0];
    assert_eq!(record.gain(), FiniteUnit::try_new(1.0).expect("unit gain"));
    assert_eq!(
        record.pitch(),
        FinitePositive::try_new(1.0).expect("unit pitch")
    );

    // The family-record cap plus one rejects with the typed capacity error
    // and no partial vector; at the cap the same source projects cleanly.
    let mut provider = admitted_mirror(EPOCH);
    commit(&mut provider, EPOCH, combat(9), 0);
    commit(&mut provider, EPOCH, combat(10), 1);
    let capped = ViewFixture::with_limits(limits_with(1, ClientLimits::MAX_FRAME_BYTES));
    let view = capped.view_of(&player, &provider, EPOCH);
    assert_eq!(
        project_audio(&view),
        Err(ClientError::Capacity),
        "two cue records above a one-record family cap reject whole"
    );
    let fitted = ViewFixture::with_limits(limits_with(2, ClientLimits::MAX_FRAME_BYTES));
    let view = fitted.view_of(&player, &provider, EPOCH);
    assert_eq!(
        project_audio(&view)
            .expect("the cap admits both records")
            .records()
            .len(),
        2
    );

    // The whole-frame byte cap rejects the same way: one configured byte
    // cannot carry a single cue record.
    let starved = ViewFixture::with_limits(limits_with(ClientLimits::MAX_FAMILY_RECORDS, 1));
    let view = starved.view_of(&player, &provider, EPOCH);
    assert_eq!(
        project_audio(&view),
        Err(ClientError::Capacity),
        "the frame byte cap rejects before any record is published"
    );
}

/// An observation from another epoch — or a player projection state from
/// another epoch — rejects the whole projection with the typed stale-epoch
/// error before any record exists.
#[test]
fn stale_epoch_rejects_whole_projection() {
    let provider = admitted_mirror(EPOCH);
    let player = idle_player(EPOCH);
    let fixture = ViewFixture::new();

    let foreign_key = ObservationKey::try_new(epoch(EPOCH - 1), ConfirmedRevision::new(1), 0)
        .expect("staged key");
    let staged = AcceptedObservation::try_new(foreign_key, None, packet(combat(9)), Vec::new())
        .expect("staged observation");
    let observations = vec![staged];
    let view = fixture.view(
        &player,
        provider.mirror(),
        &observations,
        EPOCH,
        ConfirmedRevision::new(1),
    );
    assert_eq!(
        project_audio(&view),
        Err(ClientError::StaleEpoch),
        "an old-epoch observation never enters this frame"
    );

    let foreign_player = idle_player(EPOCH - 1);
    let view = fixture.view_of(&foreign_player, &provider, EPOCH);
    assert_eq!(
        project_audio(&view),
        Err(ClientError::StaleEpoch),
        "an old-epoch player state never enters this frame"
    );
}

/// The failed-publication row: a proposed cue and dedup delta are built from
/// the real committed state, then the publication transaction fails on the
/// frame cap. No source observation is consumed, the committed key stays
/// uncancelled and unextended, the proposal stays retry-owned, a valid retry
/// emits the cue exactly once, and only the successful publication commits
/// the key so the projection after it stays silent.
#[test]
fn failed_publication_retains_dedup() {
    let mut double = ContractDouble::new(DoubleMode::Contract, limits());
    let identity = ClientIdentity::try_new(LoginStart::new(player(3), "pilot", 8).expect("login"))
        .expect("identity");
    let endpoint = Endpoint::Memory {
        connector_id: NonZeroU64::new(1).expect("one"),
    };
    let session = double.connect(endpoint, identity).expect("connected");
    double.admit().expect("admitted");
    let budget = ClientWorkBudget::try_new(16, 0).expect("budget");

    // One already-committed predicted key the retry's rejection cancels.
    let committed_step_key = AudioDedupKey::Predicted {
        epoch: session,
        input_sequence: 5,
        cue: CueId::try_new(6).expect("snow step cue"),
    };
    double.stage_dedup_proposal(
        AudioDedupDelta::try_new(vec![committed_step_key], Vec::new()).expect("checked delta"),
    );
    double.step(budget).expect("the preparatory publication");
    assert_eq!(double.committed_dedup(), &[committed_step_key]);

    // The source event: one confirmed chat acknowledgment staged into the
    // publication owner beside the refused input sequence.
    double
        .stage_observation(session, packet(chat(99)))
        .expect("staged");
    let key = ObservationKey::try_new(session, ConfirmedRevision::new(1), 0).expect("staged key");
    let staged = AcceptedObservation::try_new(key, None, packet(chat(99)), Vec::new())
        .expect("staged observation");
    let observations = vec![staged];
    let chat_key = AudioDedupKey::Confirmed {
        epoch: session,
        observation: key,
        authoritative_event_id: Some(99),
        cue: CueId::try_new(0).expect("ui click cue"),
    };

    let mut input = InputProjectionState::try_new(1).expect("input projection state");
    input.record_rejected(5, ClientError::InvalidInput);
    let player = idle_player(session.get());
    let audio = AudioProjectionState::try_new()
        .expect("audio state")
        .with_committed(double.committed_dedup().to_vec());
    let fixture = ViewFixture {
        input,
        audio,
        ..ViewFixture::new()
    };
    let view = fixture.view(
        &player,
        double.mirror().expect("connected"),
        &observations,
        session.get(),
        ConfirmedRevision::new(1),
    );
    let proposal = project_audio(&view).expect("the cue and its delta are proposed");
    assert_eq!(proposal.records().len(), 1);
    assert_eq!(proposal.proposed_dedup().insertions(), &[chat_key]);
    assert_eq!(
        proposal.proposed_dedup().cancellations(),
        &[committed_step_key]
    );

    // Force the publication failure on the frame byte cap.
    double.stage_dedup_proposal(
        AudioDedupDelta::try_new(
            proposal.proposed_dedup().insertions().to_vec(),
            proposal.proposed_dedup().cancellations().to_vec(),
        )
        .expect("checked delta"),
    );
    double.set_frame_cap_for_test(double.candidate_size_hint() - 1);
    assert_eq!(
        double.step(budget),
        Err(ClientError::Capacity),
        "the frame failure is typed"
    );
    assert_eq!(
        double.pending_observations(),
        1,
        "no source event is consumed by the failed publication"
    );
    assert_eq!(
        double.committed_dedup(),
        &[committed_step_key],
        "no dedup key commits and no cancellation is consumed"
    );
    assert_eq!(
        double.pending_dedup_proposal().insertions(),
        &[chat_key],
        "the proposal stays retry-owned"
    );
    assert_eq!(
        double.pending_dedup_proposal().cancellations(),
        &[committed_step_key]
    );

    // The valid retry emits the cue exactly once — the source observation is
    // still retained and the key is still uncommitted.
    double.set_frame_cap_for_test(ClientLimits::MAX_FRAME_BYTES);
    let mut retry_input = InputProjectionState::try_new(1).expect("input projection state");
    retry_input.record_rejected(5, ClientError::InvalidInput);
    let fixture = ViewFixture {
        input: retry_input,
        audio: AudioProjectionState::try_new()
            .expect("audio state")
            .with_committed(double.committed_dedup().to_vec()),
        ..ViewFixture::new()
    };
    let view = fixture.view(
        &player,
        double.mirror().expect("connected"),
        &observations,
        session.get(),
        ConfirmedRevision::new(1),
    );
    let retry = project_audio(&view).expect("the retry still proposes its cue");
    assert_eq!(retry.records().len(), 1, "the retry emits once");
    assert_eq!(retry.proposed_dedup().insertions(), &[chat_key]);
    assert_eq!(
        retry.proposed_dedup().cancellations(),
        &[committed_step_key]
    );

    double.stage_dedup_proposal(
        AudioDedupDelta::try_new(
            retry.proposed_dedup().insertions().to_vec(),
            retry.proposed_dedup().cancellations().to_vec(),
        )
        .expect("checked delta"),
    );
    double.step(budget).expect("the retry publishes");
    assert_eq!(
        double.committed_dedup(),
        &[chat_key],
        "the successful publication commits the insertion and applies the cancellation"
    );
    assert_eq!(
        double.pending_observations(),
        0,
        "the event is consumed once"
    );

    // After the successful publication the committed key silences the cue.
    let audio = AudioProjectionState::try_new()
        .expect("audio state")
        .with_committed(double.committed_dedup().to_vec());
    let fixture = ViewFixture::with_audio(audio);
    let view = fixture.view(
        &player,
        double.mirror().expect("connected"),
        &observations,
        session.get(),
        ConfirmedRevision::new(2),
    );
    let projection = project_audio(&view).expect("valid projection");
    assert!(
        projection.records().is_empty(),
        "the committed key suppresses the already-published cue"
    );
}
