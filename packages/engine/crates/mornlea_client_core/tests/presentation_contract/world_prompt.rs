//! Prompt projection tests over real confirmed block content and recorded targets.

use mornlea_client_core::contracts::{
    ClientError, ClientLimits, ConfirmedRevision, ObservationKey, SessionEpoch,
    registered_label_text,
};
use mornlea_client_core::input::InputProjectionState;
use mornlea_client_core::prediction::{PlayerProjectionState, RayTarget};
use mornlea_client_core::presentation::frame::WorldUiRecord;
use mornlea_client_core::presentation::world_ui::prompt::project_prompt;
use mornlea_client_core::presentation::{
    AcceptedObservation, AudioProjectionState, BoundedText, DiagnosticProjectionState,
    ErrorClassCounters, LifecycleProjectionState, MovementIntent, OrderedRecord, Pose,
    ProducerIdentity, ProjectionOrder, ProjectionView, PromptView, QueueCounters, StableRecordKey,
    TextKind, WorldTopic, WorldUiView,
};
use mornlea_client_core::session::ConfirmedMirror;
use mornlea_client_core::session::mirror::MirrorProvider;
use mornlea_domain::{
    BlockPos, ChunkPos, ChunkSnapshot, ChunkSnapshotParts, Dimension, Event, FiniteVec3,
    ForgetChunks, ForgetChunksParts, LookAngles, MiningState, MotionState, MotionStateParts,
    PalettedSection, PlayerState, PlayerStateParts, Season, SurvivalState, SurvivalStateParts,
    Weather, WorldState, WorldStateParts,
};
use mornlea_protocol::{KeepAlive, ServerPacket};

/// The epoch every fixture mirror and observation key carries.
const EPOCH: u64 = 21;

/// One checked world state fixture, the environment half of a player
/// publication the fixtures keep constant.
fn world_state() -> WorldState {
    WorldState::try_new(WorldStateParts {
        day_phase_offset: 0,
        world_time_ticks: 0,
        weather: Weather::Clear,
        season: Season::Spring,
        season_progress: 0,
        temperature: 0,
    })
    .expect("checked world state")
}

/// One neutral survival record, the non-environment half of a player
/// publication the fixtures keep constant.
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

/// One overworld player publication at the caller's server tick: ready,
/// unreset, idle mining and a neutral pose, so the queue fixtures differ
/// only where a row names a difference.
fn player_event(tick: u64) -> Event {
    Event::PlayerState(PlayerState::new(PlayerStateParts {
        server_tick: tick,
        last_input_sequence: 0,
        dimension: Dimension::OVERWORLD,
        motion: MotionState::new(MotionStateParts {
            position: FiniteVec3::try_new([0.0, 64.0, 0.0]).expect("finite fixture position"),
            velocity: FiniteVec3::try_new([0.0, 0.0, 0.0]).expect("finite fixture velocity"),
            on_ground: true,
        }),
        look: LookAngles::try_new(0.0, 0.0).expect("finite fixture look"),
        ready: true,
        reset: false,
        mining: MiningState::Idle,
        survival: survival(),
        world: world_state(),
    }))
}

fn packet(event: Event) -> ServerPacket {
    ServerPacket::try_from(event).expect("publication converts to its packet")
}

/// An admitted real mirror provider with no observations yet.
fn admitted_mirror(epoch: u64) -> MirrorProvider {
    let epoch = SessionEpoch::try_new(epoch).expect("epoch");
    let mut provider =
        MirrorProvider::new(epoch, ClientLimits::try_new().expect("limits")).expect("mirror");
    provider.admit().expect("admitted session");
    provider
}

/// Commits one publication through the real mirror provider, so the retained
/// observation queue carries the actual source keys and the derived tick.
fn commit(provider: &mut MirrorProvider, event: Event) {
    let next = provider.mirror().revision().get() + 1;
    let key = ObservationKey::try_new(provider.mirror().epoch(), ConfirmedRevision::new(next), 0)
        .expect("staged key");
    let staged =
        AcceptedObservation::try_new(key, None, packet(event), Vec::new()).expect("staged");
    provider.commit(&staged).expect("committed observation");
}

/// One checked recorded ray target at the caller's position and source
/// revision — the checked constructor surface the replay owner publishes
/// through, so the fixtures carry exactly the seam the projection consumes.
fn recorded_target(x: i32, y: i32, z: i32, revision: u64) -> RayTarget {
    RayTarget::try_new(BlockPos::new(x, y, z), ConfirmedRevision::new(revision))
        .expect("checked recorded target")
}

/// The projection-view state owners a fixture keeps alive beside the
/// borrowed mirror and observation queue.
struct ViewFixture {
    input: InputProjectionState,
    player: PlayerProjectionState,
    audio: AudioProjectionState,
    lifecycle: LifecycleProjectionState,
    diagnostics: DiagnosticProjectionState,
    limits: ClientLimits,
}

impl ViewFixture {
    fn new() -> Self {
        Self::with_pose(None)
    }

    fn with_pose(predicted: Option<Pose>) -> Self {
        Self {
            input: InputProjectionState::try_new(1).expect("input projection state"),
            player: PlayerProjectionState::try_new(
                SessionEpoch::try_new(EPOCH).expect("epoch"),
                ConfirmedRevision::new(1),
                Pose::try_new([0.0, 64.0, 0.0], 0.0, 0.0).expect("finite pose"),
                predicted,
                Vec::new(),
                None,
                None,
                MovementIntent::try_new(None, false).expect("movement intent"),
            )
            .expect("player projection state"),
            audio: AudioProjectionState::try_new().expect("audio projection state"),
            lifecycle: LifecycleProjectionState::try_new(1).expect("lifecycle projection state"),
            diagnostics: DiagnosticProjectionState::try_new(
                ProducerIdentity::try_new([0u8; 20], [0u8; 32]).expect("producer identity"),
                QueueCounters::default(),
                ErrorClassCounters::default(),
            )
            .expect("diagnostics projection state"),
            limits: ClientLimits::try_new().expect("limits"),
        }
    }

    fn with_dimension(mut self, dimension: Dimension) -> Self {
        self.player = self.player.with_world_dimension(dimension);
        self
    }

    /// Publishes the recorded target of the confirmed pose's look ray
    /// through its checked constructor surface.
    fn with_confirmed_target(mut self, target: Option<RayTarget>) -> Self {
        self.player = self.player.with_confirmed_target(target);
        self
    }

    /// Publishes the recorded target of the predicted pose's own look ray
    /// through its checked constructor surface.
    fn with_predicted_target(mut self, target: Option<RayTarget>) -> Self {
        self.player = self.player.with_predicted_target(target);
        self
    }

    /// Builds one immutable projection view over the supplied mirror and
    /// observation queue.
    fn view<'a>(
        &'a self,
        mirror: &'a ConfirmedMirror,
        observations: &'a [AcceptedObservation],
        epoch: SessionEpoch,
        revision: ConfirmedRevision,
    ) -> ProjectionView<'a> {
        ProjectionView::try_new(
            mirror,
            observations,
            &self.input,
            &self.player,
            &self.audio,
            &self.lifecycle,
            &self.diagnostics,
            epoch,
            revision,
            5,
            &self.limits,
        )
        .expect("projection view")
    }

    /// The candidate view of one provider's committed state: the next
    /// revision the coherent frame would carry.
    fn view_of<'a>(&'a self, provider: &'a MirrorProvider) -> ProjectionView<'a> {
        let revision = ConfirmedRevision::new(provider.mirror().revision().get() + 1);
        self.view(
            provider.mirror(),
            provider.observations(),
            provider.mirror().epoch(),
            revision,
        )
    }
}

/// The shared fixture owners; the per-view epoch and revision are supplied
/// when the view itself is built.
fn view_fixture() -> ViewFixture {
    ViewFixture::new()
}

/// An absent world context emits no prompt fact, even with an empty queue.
#[test]
fn missing_world_context_projects_no_record() {
    let provider = admitted_mirror(EPOCH);
    let fixture = view_fixture();
    let records = project_prompt(&fixture.view_of(&provider)).expect("no target projects empty");
    assert!(
        records.is_empty(),
        "an absent world context fabricates no prompt fact"
    );
}

fn load(provider: &mut MirrorProvider, dimension: Dimension, block: u16) {
    commit(
        provider,
        Event::ChunkSnapshot(
            ChunkSnapshot::try_new(ChunkSnapshotParts {
                dimension,
                chunk: ChunkPos::new(0, 0),
                revision: 1,
                sections: Box::new(std::array::from_fn(|_| {
                    PalettedSection::single(block).unwrap()
                })),
            })
            .unwrap(),
        ),
    );
}

fn assert_prompt(
    records: &[OrderedRecord<WorldUiRecord>],
    revision: u64,
    expected: Option<PromptView>,
) {
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].record().view(), &WorldUiView::Prompt(expected));
    assert_eq!(records[0].record().header().source_tick(), None);
    assert_eq!(
        records[0].record().header().revision(),
        ConfirmedRevision::new(revision)
    );
    assert_eq!(
        records[0].stable_key(),
        &StableRecordKey::World {
            topic: WorldTopic::Prompt,
            identity: None
        }
    );
    assert_eq!(
        records[0].order(),
        &ProjectionOrder::AfterConfirmed {
            epoch: SessionEpoch::try_new(EPOCH).unwrap(),
            revision: ConfirmedRevision::new(revision),
            local_sequence: 1,
            record_ordinal: 0,
        }
    );
}

#[test]
fn loaded_targets_use_actual_dimension_and_registered_label() {
    let mut provider = admitted_mirror(EPOCH);
    load(&mut provider, Dimension::OVERWORLD, 2);
    load(&mut provider, Dimension::DEPTHS, 3);
    let revision = provider.mirror().revision().get() + 1;
    for (dimension, label) in [(Dimension::OVERWORLD, "石头"), (Dimension::DEPTHS, "泥土")] {
        let fixture = view_fixture()
            .with_dimension(dimension)
            .with_confirmed_target(Some(recorded_target(1, 64, 2, revision)));
        assert_prompt(
            &project_prompt(&fixture.view_of(&provider)).unwrap(),
            revision,
            Some(
                PromptView::try_new(
                    BlockPos::new(1, 64, 2),
                    BoundedText::try_new(label.to_owned(), TextKind::Target).unwrap(),
                )
                .unwrap(),
            ),
        );
    }
    let missing_dimension =
        view_fixture().with_confirmed_target(Some(recorded_target(1, 64, 2, revision)));
    assert!(
        project_prompt(&missing_dimension.view_of(&provider))
            .unwrap()
            .is_empty()
    );
}

#[test]
fn absent_stale_future_and_unknown_targets_clear() {
    let mut provider = admitted_mirror(EPOCH);
    load(&mut provider, Dimension::OVERWORLD, 2);
    let revision = provider.mirror().revision().get() + 1;
    for target in [
        None,
        Some(recorded_target(1, 64, 2, revision - 1)),
        Some(recorded_target(1, 64, 2, revision + 1)),
        Some(recorded_target(32, 64, 2, revision)),
    ] {
        let fixture = view_fixture()
            .with_dimension(Dimension::OVERWORLD)
            .with_confirmed_target(target);
        assert_prompt(
            &project_prompt(&fixture.view_of(&provider)).unwrap(),
            revision,
            None,
        );
    }
    commit(
        &mut provider,
        Event::ForgetChunks(
            ForgetChunks::try_new(ForgetChunksParts {
                dimension: Dimension::OVERWORLD,
                chunks: vec![ChunkPos::new(0, 0)].into_boxed_slice(),
            })
            .unwrap(),
        ),
    );
    let revision = provider.mirror().revision().get() + 1;
    let fixture = view_fixture()
        .with_dimension(Dimension::OVERWORLD)
        .with_confirmed_target(Some(recorded_target(1, 64, 2, revision)));
    assert_prompt(
        &project_prompt(&fixture.view_of(&provider)).unwrap(),
        revision,
        None,
    );
}

#[test]
fn non_interaction_blocks_clear() {
    for block in [0, 1, 27, 28, 29, 30, 31, 32, 33, 34, 63, 65, 67, 69] {
        let mut provider = admitted_mirror(EPOCH);
        load(&mut provider, Dimension::OVERWORLD, block);
        let revision = provider.mirror().revision().get() + 1;
        let fixture = view_fixture()
            .with_dimension(Dimension::OVERWORLD)
            .with_confirmed_target(Some(recorded_target(1, 64, 2, revision)));
        assert_prompt(
            &project_prompt(&fixture.view_of(&provider)).unwrap(),
            revision,
            None,
        );
    }
}

#[test]
fn predicted_pose_selects_its_target_without_confirmed_fallback() {
    let mut provider = admitted_mirror(EPOCH);
    load(&mut provider, Dimension::OVERWORLD, 2);
    let revision = provider.mirror().revision().get() + 1;
    let predicted = Some(Pose::try_new([1.0, 64.0, 0.0], 0.0, 0.0).unwrap());
    for target in [
        None,
        Some(recorded_target(2, 64, 3, revision - 1)),
        Some(recorded_target(2, 64, 3, revision + 1)),
        Some(recorded_target(2, 64, 3, revision)),
    ] {
        let fixture = ViewFixture::with_pose(predicted)
            .with_dimension(Dimension::OVERWORLD)
            .with_confirmed_target(Some(recorded_target(1, 64, 2, revision)))
            .with_predicted_target(target);
        let expected = target
            .filter(|target| target.source_revision().get() == revision)
            .map(|target| {
                PromptView::try_new(
                    target.position(),
                    registered_label_text(2).unwrap().unwrap(),
                )
                .unwrap()
            });
        assert_prompt(
            &project_prompt(&fixture.view_of(&provider)).unwrap(),
            revision,
            expected,
        );
    }
    let fixture = view_fixture()
        .with_dimension(Dimension::OVERWORLD)
        .with_confirmed_target(Some(recorded_target(1, 64, 2, revision)))
        .with_predicted_target(Some(recorded_target(2, 64, 3, revision)));
    assert_prompt(
        &project_prompt(&fixture.view_of(&provider)).unwrap(),
        revision,
        Some(
            PromptView::try_new(
                BlockPos::new(1, 64, 2),
                registered_label_text(2).unwrap().unwrap(),
            )
            .unwrap(),
        ),
    );
}

/// `purity`: the projection is a pure function of the immutable view —
/// repeated calls over the same view return equal records, target-present
/// and target-absent states alike.
#[test]
fn projection_is_pure() {
    let mut provider = admitted_mirror(EPOCH);
    load(&mut provider, Dimension::OVERWORLD, 2);
    let fixture = view_fixture();
    let view = fixture.view_of(&provider);
    let first = project_prompt(&view).expect("first projection succeeds");
    let second = project_prompt(&view).expect("second projection succeeds");
    assert_eq!(first, second, "repeated calls over one view agree");

    let targeted = view_fixture()
        .with_dimension(Dimension::OVERWORLD)
        .with_confirmed_target(Some(recorded_target(1, 64, 2, 2)))
        .with_predicted_target(Some(recorded_target(2, 65, 3, 2)));
    let view = targeted.view_of(&provider);
    let first = project_prompt(&view).expect("third projection succeeds");
    let second = project_prompt(&view).expect("fourth projection succeeds");
    assert_eq!(first, second, "repeated calls over a targeted view agree");
}

/// `old epoch`: an observation from another epoch rejects the whole
/// projection with `StaleEpoch`, exactly as every sibling world-UI provider
/// refuses a queue the frame epoch does not own — never a partial prefix.
#[test]
fn old_epoch_observation_rejects_whole() {
    let mut provider = admitted_mirror(EPOCH);
    commit(&mut provider, player_event(1));
    let next_epoch = SessionEpoch::try_new(EPOCH + 1).expect("next epoch");
    let fixture = view_fixture().with_dimension(Dimension::OVERWORLD);
    assert_eq!(
        project_prompt(&fixture.view(
            provider.mirror(),
            provider.observations(),
            next_epoch,
            ConfirmedRevision::new(1),
        )),
        Err(ClientError::StaleEpoch),
        "an old-epoch observation rejects the whole projection"
    );
}

/// `non-event packet`: a queue that mixes one valid publication with a
/// packet that is not a checked event publication rejects the whole
/// projection with `InvalidInput` — never a partial prefix.
#[test]
fn non_event_packet_rejects_whole() {
    let mut provider = admitted_mirror(EPOCH);
    commit(&mut provider, player_event(1));
    let valid = provider.observations()[0].clone();
    let epoch = SessionEpoch::try_new(EPOCH).expect("epoch");
    let non_event = AcceptedObservation::try_new(
        ObservationKey::try_new(epoch, ConfirmedRevision::new(2), 0).expect("key"),
        None,
        ServerPacket::KeepAlive(KeepAlive::new(1).expect("checked keep-alive token")),
        Vec::new(),
    )
    .expect("staged with a non-publication packet");
    let fixture = view_fixture().with_dimension(Dimension::OVERWORLD);
    assert_eq!(
        project_prompt(&fixture.view(
            provider.mirror(),
            &[valid, non_event],
            epoch,
            ConfirmedRevision::new(2),
        )),
        Err(ClientError::InvalidInput),
        "a non-event packet rejects the whole projection"
    );
}

/// `label boundary pins`: the checked registered-text constructor the
/// prompt label chain consumes. A registered id checks through the
/// `Target` bound with its registered kind, the first unregistered id and
/// the far-outside id stay absent (None, never invented text), and at the
/// checked byte boundary 64 bytes admit while 65 reject typed.
#[test]
fn registered_label_boundary_pins() {
    let registered = registered_label_text(2)
        .expect("a registered id checks")
        .expect("its registered label");
    assert_eq!(registered.kind(), &TextKind::Target);
    assert!(
        registered.as_str().len() <= BoundedText::TARGET_MAX_BYTES,
        "every registered label checks through the Target bound"
    );

    assert_eq!(
        registered_label_text(90),
        Ok(None),
        "the first unregistered id has no label and none is invented"
    );
    assert_eq!(
        registered_label_text(u16::MAX),
        Ok(None),
        "an id far outside the registry has no label and none is invented"
    );

    let at_bound =
        BoundedText::try_new("x".repeat(BoundedText::TARGET_MAX_BYTES), TextKind::Target)
            .expect("64 bytes admit at the Target bound");
    assert_eq!(at_bound.as_str().len(), BoundedText::TARGET_MAX_BYTES);
    assert_eq!(
        BoundedText::try_new(
            "x".repeat(BoundedText::TARGET_MAX_BYTES + 1),
            TextKind::Target
        ),
        Err(ClientError::InvalidInput),
        "65 bytes reject typed at the checked boundary"
    );
}

#[test]
fn registered_non_collision_targets_keep_their_labels() {
    for block in [37, 70, 84, 89] {
        let mut provider = admitted_mirror(EPOCH);
        load(&mut provider, Dimension::OVERWORLD, block);
        let revision = provider.mirror().revision().get() + 1;
        let fixture = view_fixture()
            .with_dimension(Dimension::OVERWORLD)
            .with_confirmed_target(Some(recorded_target(1, 64, 2, revision)));
        assert_prompt(
            &project_prompt(&fixture.view_of(&provider)).unwrap(),
            revision,
            Some(
                PromptView::try_new(
                    BlockPos::new(1, 64, 2),
                    registered_label_text(block).unwrap().unwrap(),
                )
                .unwrap(),
            ),
        );
    }
}
