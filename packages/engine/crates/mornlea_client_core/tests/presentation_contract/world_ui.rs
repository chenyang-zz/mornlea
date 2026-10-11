//! Complete world UI family assembly contract tests.
//!
//! The table pins the serial world-UI family assembly against the five
//! accepted real providers over real committed observations: the five checked
//! vectors combine into one coherent `world-ui@1` vector where retained
//! order keys — never the parts order and never equal or absent source
//! ticks — determine the interleaving, so weather, task and chat records
//! assemble together in actual source order. The bounded chat window the
//! chat provider exposes is merged exactly as emitted, never re-derived,
//! extended or re-filtered here. The derived prompt topic invents no source
//! event: the landed prompt provider publishes the schema's empty prompt
//! state, and a schema-admitted derived prompt envelope under the
//! `AfterConfirmed` order validates under the same rules and merges after
//! every confirmed entry at its sampled revision. Repeated stable keys
//! across observations are the accepted latest-wins lifecycles of the
//! singleton current-state topics, schema-bound and resolved by retained
//! source order alone. A foreign epoch or revision, a stale derived order
//! key, an ambiguous duplicate order slot, an envelope disagreeing with its
//! record, and a family count or frame byte cap plus one each reject the
//! whole family typed with the previous output retained. The assembler is
//! pure per frame: the family reflects the retained-observation window of
//! the sampled revision alone, and whether the presentation consumer keeps
//! the last window visible across empty frames is that consumer's concern,
//! never state built here.

use mornlea_client_core::contracts::{
    ClientError, ClientLimits, ConfirmedRevision, FAMILY_WORLD_UI, FamilyKey, FamilyOperation,
    ObservationKey, RecordHeader, SessionEpoch,
};
use mornlea_client_core::input::InputProjectionState;
use mornlea_client_core::prediction::PlayerProjectionState;
use mornlea_client_core::presentation::family_world_ui::assemble_world_ui;
use mornlea_client_core::presentation::frame::{
    FamilyFrame, FamilyRecords, PresentationFrame, WorldUiRecord,
};
use mornlea_client_core::presentation::world_ui::chat::project_chat;
use mornlea_client_core::presentation::world_ui::environment::project_environment;
use mornlea_client_core::presentation::world_ui::prompt::project_prompt;
use mornlea_client_core::presentation::world_ui::survival::project_survival;
use mornlea_client_core::presentation::world_ui::task::project_task;
use mornlea_client_core::presentation::{
    AcceptedObservation, AudioProjectionState, BoundedText, DiagnosticProjectionState,
    ErrorClassCounters, LifecycleProjectionState, MovementIntent, OrderedRecord, Pose,
    ProducerIdentity, ProjectionOrder, ProjectionView, PromptView, QueueCounters, StableRecordKey,
    TaskView, TextKind, WorldTopic, WorldUiView,
};
use mornlea_client_core::session::ConfirmedMirror;
use mornlea_client_core::session::mirror::MirrorProvider;
use mornlea_domain::{
    BlockPos, ChatBody, ChatEvent, ChatEventParts, CommandText, CompanionId, CompanionName,
    CompanionSpeaker, Dimension, DisplayName, Event, FiniteVec3, LookAngles, MiningState,
    MotionState, MotionStateParts, PlayerId, PlayerState, PlayerStateParts, Season, SurvivalState,
    SurvivalStateParts, TaskState, Weather, WorldState, WorldStateParts,
};
use mornlea_protocol::ServerPacket;

/// The epoch every fixture mirror and observation key carries.
const EPOCH: u64 = 57;

/// The candidate frame revision the hand-built envelope fixtures rebase onto.
const FRAME_REVISION: u64 = 10;

fn epoch() -> SessionEpoch {
    SessionEpoch::try_new(EPOCH).expect("epoch")
}

/// One checked world state at the caller's exact scalars, refusing nothing a
/// real authority could publish inside the accepted domains.
fn world_state(
    day_phase_offset: u16,
    world_time_ticks: u64,
    weather: Weather,
    season: Season,
    season_progress: u8,
    temperature: i8,
) -> WorldState {
    WorldState::try_new(WorldStateParts {
        day_phase_offset,
        world_time_ticks,
        weather,
        season,
        season_progress,
        temperature,
    })
    .expect("checked world state")
}

/// One checked survival record at the caller's health and armor, the rest of
/// the scalars the fixtures keep constant.
fn survival(health: u8, armor_points: u8) -> SurvivalState {
    SurvivalState::try_new(SurvivalStateParts {
        health,
        oxygen: 300,
        hunger: 20,
        saturation_zero: false,
        armor_points,
    })
    .expect("checked survival state")
}

/// One overworld player publication at the caller's server tick, world and
/// survival scalars: one publication is one environment record and one
/// survival record beside each other.
fn player_event(tick: u64, world: WorldState, state: SurvivalState) -> Event {
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
        survival: state,
        world,
    }))
}

/// A checked UUIDv4 player identity whose raw byte order follows `first`, so
/// two fixture senders are distinct actual identities.
fn player_id(first: u8) -> PlayerId {
    let mut bytes = [0u8; 16];
    bytes[0] = first;
    bytes[6] = 0x40;
    bytes[8] = 0x80;
    PlayerId::try_from_bytes(bytes).expect("checked uuid v4 player identity")
}

fn player_name() -> DisplayName {
    DisplayName::try_from_canonical("Chen".to_owned()).expect("canonical player name")
}

/// A checked UUIDv4 companion identity for the bodies that name one.
fn companion_id(last: u8) -> CompanionId {
    let mut bytes = [0u8; 16];
    bytes[0] = 0x12;
    bytes[6] = 0x40;
    bytes[8] = 0x80;
    bytes[15] = last;
    CompanionId::try_from_bytes(bytes).expect("checked uuid v4 companion identity")
}

fn companion_name() -> CompanionName {
    CompanionName::try_from_canonical("阿木".to_owned()).expect("canonical companion name")
}

fn speaker(last: u8) -> CompanionSpeaker {
    CompanionSpeaker::new(companion_id(last), companion_name())
}

fn command_text(text: &str) -> CommandText {
    CommandText::try_from_canonical(text.to_owned()).expect("canonical command text")
}

/// One addressing-accepted chat fact, the chat topic's own record shape.
fn accepted_fact(event_id: u64, sender: PlayerId) -> ChatEvent {
    ChatEvent::try_new(ChatEventParts {
        event_id,
        player_id: sender,
        player_name: player_name(),
        body: ChatBody::Accepted {
            companion: speaker(1),
            command: command_text("挖石头"),
        },
    })
    .expect("checked chat event")
}

/// One task lifecycle chat fact: a chat publication whose body is the task
/// branch, so one observation projects the chat record and the task record
/// beside each other.
fn task_fact(event_id: u64, sender: PlayerId, state: TaskState) -> ChatEvent {
    ChatEvent::try_new(ChatEventParts {
        event_id,
        player_id: sender,
        player_name: player_name(),
        body: ChatBody::Task {
            companion: speaker(2),
            command: command_text("挖石头"),
            state,
        },
    })
    .expect("checked chat event")
}

fn chat_event(fact: ChatEvent) -> Event {
    Event::Chat(fact)
}

fn packet(event: Event) -> ServerPacket {
    ServerPacket::try_from(event).expect("publication converts to its packet")
}

/// An admitted real mirror provider with no observations yet.
fn admitted_mirror() -> MirrorProvider {
    let mut provider =
        MirrorProvider::new(epoch(), ClientLimits::try_new().expect("limits")).expect("mirror");
    provider.admit().expect("admitted session");
    provider
}

/// Commits one publication through the real mirror provider, so the retained
/// observation queue carries the actual source keys and the derived tick.
fn commit(provider: &mut MirrorProvider, event: Event) -> ConfirmedRevision {
    let next = provider.mirror().revision().get() + 1;
    let key = ObservationKey::try_new(provider.mirror().epoch(), ConfirmedRevision::new(next), 0)
        .expect("staged key");
    let staged =
        AcceptedObservation::try_new(key, None, packet(event), Vec::new()).expect("staged");
    provider.commit(&staged).expect("committed observation")
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
        Self {
            input: InputProjectionState::try_new(1).expect("input projection state"),
            player: PlayerProjectionState::try_new(
                epoch(),
                ConfirmedRevision::new(1),
                Pose::try_new([0.0, 64.0, 0.0], 0.0, 0.0).expect("finite pose"),
                None,
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

    /// Builds one immutable projection view over the supplied mirror and
    /// observation queue.
    fn view<'a>(
        &'a self,
        mirror: &'a ConfirmedMirror,
        observations: &'a [AcceptedObservation],
        frame_epoch: SessionEpoch,
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
            frame_epoch,
            revision,
            5,
            &self.limits,
        )
        .expect("projection view")
    }

    /// The candidate view of the provider's committed state: the next
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

fn view_fixture() -> ViewFixture {
    ViewFixture::new()
}

/// The five provider vectors of one view, in the packet's provider order:
/// environment, survival, chat, task, prompt.
fn provider_parts(view: &ProjectionView<'_>) -> [Vec<OrderedRecord<WorldUiRecord>>; 5] {
    [
        project_environment(view).expect("environment projection"),
        project_survival(view).expect("survival projection"),
        project_chat(view).expect("chat projection"),
        project_task(view).expect("task projection"),
        project_prompt(view).expect("prompt projection"),
    ]
}

/// Assembles the five real provider vectors of one committed fixture state
/// under the supplied limits.
fn assemble(
    fixture: &ViewFixture,
    provider: &MirrorProvider,
    limits: &ClientLimits,
) -> Result<Vec<WorldUiRecord>, ClientError> {
    let view = fixture.view_of(provider);
    let parts = provider_parts(&view);
    assemble_world_ui(view.frame_epoch(), view.frame_revision(), parts, limits)
}

/// Tightened frozen-ceiling limits: every bound at its frozen value except
/// the family record count and the frame byte cap.
fn limits_with(family_records: usize, frame_bytes: usize) -> ClientLimits {
    let frozen = ClientLimits::try_new().expect("frozen limits");
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
    .expect("tightened limits")
}

/// The exact minimal world-ui-only frame size of one assembled vector at
/// the assembly identity, through the accepted frame accounting the
/// assembler itself uses.
fn minimal_frame_size(
    frame_epoch: SessionEpoch,
    revision: ConfirmedRevision,
    records: &[WorldUiRecord],
) -> usize {
    PresentationFrame::try_new(
        frame_epoch,
        revision,
        0,
        vec![
            FamilyFrame::try_new(
                FamilyKey::try_new(FAMILY_WORLD_UI).expect("world-ui family key"),
                FamilyRecords::WorldUi(records.to_vec()),
            )
            .expect("world-ui family entry"),
        ],
    )
    .expect("minimal world-ui-only candidate frame")
    .validated_size()
    .expect("checked size")
}

/// The topic tag one assembled record carries, for interleaving assertions
/// that never depend on payload details.
fn view_tag(record: &WorldUiRecord) -> WorldTopic {
    match record.view() {
        WorldUiView::Environment(_) => WorldTopic::Environment,
        WorldUiView::Survival(_) => WorldTopic::Survival,
        WorldUiView::Chat(_) => WorldTopic::Chat,
        WorldUiView::Task(_) => WorldTopic::Task,
        WorldUiView::Prompt(_) => WorldTopic::Prompt,
    }
}

// --- hand-built envelope fixtures for shapes the landed providers never emit ---

/// The rebased coherent candidate header of one hand-built record.
fn hand_header(operation: FamilyOperation) -> RecordHeader {
    RecordHeader::try_new(
        epoch(),
        ConfirmedRevision::new(FRAME_REVISION),
        None,
        operation,
    )
    .expect("checked header")
}

fn world_key(topic: WorldTopic, identity: Option<ObservationKey>) -> StableRecordKey {
    StableRecordKey::World { topic, identity }
}

/// One confirmed envelope at the named observation revision and packet record
/// ordinal, wrapping the given record under the given stable identity.
fn envelope(
    observation_revision: u64,
    record_ordinal: u32,
    stable_key: StableRecordKey,
    record: WorldUiRecord,
) -> OrderedRecord<WorldUiRecord> {
    OrderedRecord::try_new(
        ProjectionOrder::Confirmed {
            observation: ObservationKey::try_new(
                epoch(),
                ConfirmedRevision::new(observation_revision),
                0,
            )
            .expect("staged key"),
            record_ordinal,
        },
        stable_key,
        record,
    )
    .expect("checked envelope")
}

/// One locally derived envelope at the named epoch, sampled revision, local
/// sequence and packet record ordinal, wrapping the given record under the
/// given stable identity. No landed world-UI provider emits this order
/// variant today; the envelope schema admits it and the derived prompt is
/// its natural kind, so the assembler's rules for it stay pinned here.
fn after_envelope(
    epoch_value: u64,
    sampled_revision: u64,
    local_sequence: u64,
    record_ordinal: u32,
    stable_key: StableRecordKey,
    record: WorldUiRecord,
) -> OrderedRecord<WorldUiRecord> {
    OrderedRecord::try_new(
        ProjectionOrder::AfterConfirmed {
            epoch: SessionEpoch::try_new(epoch_value).expect("epoch"),
            revision: ConfirmedRevision::new(sampled_revision),
            local_sequence,
            record_ordinal,
        },
        stable_key,
        record,
    )
    .expect("checked envelope")
}

/// A minimal checked environment upsert record for the hand-built sequences.
fn environment_record(world: WorldState) -> WorldUiRecord {
    WorldUiRecord::try_new(
        hand_header(FamilyOperation::Upsert),
        WorldUiView::Environment(world),
    )
    .expect("checked environment record")
}

/// A minimal checked chat upsert record for the hand-built sequences.
fn chat_record(fact: &ChatEvent) -> WorldUiRecord {
    WorldUiRecord::try_new(
        hand_header(FamilyOperation::Upsert),
        WorldUiView::Chat(fact.clone()),
    )
    .expect("checked chat record")
}

/// A minimal checked derived prompt record: the mirror-derived ray target
/// beside its registered display label, the kind the landed prompt seam
/// would project once its position-to-id link lands.
fn prompt_record(target: PromptView) -> WorldUiRecord {
    WorldUiRecord::try_new(
        hand_header(FamilyOperation::Upsert),
        WorldUiView::Prompt(Some(target)),
    )
    .expect("checked prompt record")
}

/// One checked derived prompt view at the named position and registered
/// label.
fn prompt_view(x: i32, y: i32, z: i32, label: &str) -> PromptView {
    PromptView::try_new(
        BlockPos::new(x, y, z),
        BoundedText::try_new(label.to_owned(), TextKind::Target).expect("checked target label"),
    )
    .expect("checked prompt view")
}

fn empty_parts() -> [Vec<OrderedRecord<WorldUiRecord>>; 5] {
    [Vec::new(), Vec::new(), Vec::new(), Vec::new(), Vec::new()]
}

fn assemble_hand_built(
    parts: [Vec<OrderedRecord<WorldUiRecord>>; 5],
) -> Result<Vec<WorldUiRecord>, ClientError> {
    assemble_world_ui(
        epoch(),
        ConfirmedRevision::new(FRAME_REVISION),
        parts,
        &ClientLimits::try_new().expect("limits"),
    )
}

// --- the table ---

/// `weather/task/chat together`: the five real providers' vectors of one
/// committed view assemble into one family vector in actual observation
/// order — weather and survival scalars interleaved with task and chat
/// facts — and the parts order is free: a rotated parts array yields the
/// identical vector. One task publication is one chat observation, so its
/// chat and task records share one order slot under distinct stable
/// identities and interleave by the stable-key tiebreak. Every record is
/// emitted unchanged with the envelope stripped after validation, and the
/// bounded chat window merges exactly as the provider emitted it, never
/// re-derived, extended or re-filtered here.
#[test]
fn weather_task_and_chat_assemble_together() {
    let morning = world_state(1_000, 20_000, Weather::Clear, Season::Spring, 10, 5);
    let storm = world_state(12_000, 34_000, Weather::Thunder, Season::Winter, 200, -30);
    let healthy = survival(20, 0);
    let hurt = survival(9, 4);
    let sender = player_id(0xA0);
    let mut provider = admitted_mirror();
    commit(&mut provider, player_event(17, morning, healthy));
    commit(
        &mut provider,
        chat_event(task_fact(10, sender, TaskState::Started)),
    );
    commit(&mut provider, chat_event(accepted_fact(11, sender)));
    commit(&mut provider, player_event(18, storm, hurt));

    let fixture = view_fixture();
    let view = fixture.view_of(&provider);
    let parts = provider_parts(&view);
    let limits = ClientLimits::try_new().expect("limits");
    let assembled = assemble_world_ui(
        view.frame_epoch(),
        view.frame_revision(),
        parts.clone(),
        &limits,
    )
    .expect("the five providers assemble");

    let expected = [
        WorldTopic::Environment,
        WorldTopic::Survival,
        WorldTopic::Chat,
        WorldTopic::Task,
        WorldTopic::Chat,
        WorldTopic::Environment,
        WorldTopic::Survival,
    ];
    assert_eq!(assembled.len(), expected.len());
    for (record, topic) in assembled.iter().zip(&expected) {
        assert_eq!(
            view_tag(record),
            *topic,
            "actual source order, not topic batching"
        );
        assert_eq!(record.header().epoch(), view.frame_epoch());
        assert_eq!(
            record.header().revision(),
            view.frame_revision(),
            "one coherent candidate revision across the merged family"
        );
    }
    // The ticked publications carry their own ticks; the chat-family records
    // carry none, and no record carries an invented tick.
    let ticks = [Some(17), Some(17), None, None, None, Some(18), Some(18)];
    for (record, tick) in assembled.iter().zip(ticks) {
        assert_eq!(record.header().source_tick(), tick);
    }

    // The task publication's chat and task records share one order slot under
    // distinct stable identities: the stable-key tiebreak resolves the tie in
    // topic variant order, chat before task.
    assert_eq!(view_tag(&assembled[2]), WorldTopic::Chat);
    assert_eq!(view_tag(&assembled[3]), WorldTopic::Task);

    // The emitted vector is the unchanged record payload: the envelopes are
    // stripped after validation and nothing else about a record changes.
    let mut expected_records: Vec<OrderedRecord<WorldUiRecord>> = parts.concat();
    expected_records.sort_by(|left, right| {
        left.order()
            .cmp(right.order())
            .then_with(|| left.stable_key().cmp(right.stable_key()))
    });
    let expected_records: Vec<WorldUiRecord> = expected_records
        .into_iter()
        .map(|entry| entry.into_record())
        .collect();
    assert_eq!(assembled, expected_records, "records are emitted unchanged");

    // The bounded chat window is merged exactly as the chat provider emitted
    // it: never re-derived, extended or filtered here.
    let emitted_chat = parts[2].len();
    let merged_chat = assembled
        .iter()
        .filter(|record| view_tag(record) == WorldTopic::Chat)
        .count();
    assert_eq!(
        merged_chat, emitted_chat,
        "the bounded chat window merges as emitted"
    );

    // A rotated parts array — the prompt vector first — yields the identical
    // output because retained order keys, not parts order, interleave.
    let rotated = [
        parts[4].clone(),
        parts[3].clone(),
        parts[2].clone(),
        parts[1].clone(),
        parts[0].clone(),
    ];
    let rotated = assemble_world_ui(view.frame_epoch(), view.frame_revision(), rotated, &limits)
        .expect("rotated parts assemble");
    assert_eq!(assembled, rotated, "parts order never reorders the family");
}

/// `interleaved equal/absent ticks`: three player publications at the same
/// server tick — the legal zero included — and a tickless chat publication
/// between the first two keep their actual source order after the envelope
/// merge: equal or absent ticks never decide anything, the retained
/// observation keys alone do, and every record preserves its own
/// observation's tick verbatim.
#[test]
fn equal_or_absent_ticks_retain_actual_source_order() {
    let first = world_state(0, 100, Weather::Clear, Season::Spring, 0, 0);
    let second = world_state(12_000, 100, Weather::Rain, Season::Summer, 128, 20);
    let third = world_state(23_000, 100, Weather::Thunder, Season::Autumn, 255, -5);
    let neutral = survival(20, 0);
    let sender = player_id(0xA1);
    let mut provider = admitted_mirror();
    commit(&mut provider, player_event(0, first, neutral));
    commit(&mut provider, chat_event(accepted_fact(1, sender)));
    commit(&mut provider, player_event(0, second, neutral));
    commit(&mut provider, player_event(0, third, neutral));

    let fixture = view_fixture();
    let assembled = assemble(
        &fixture,
        &provider,
        &ClientLimits::try_new().expect("limits"),
    )
    .expect("equal and absent ticks assemble in retained order");

    let expected = [
        WorldTopic::Environment,
        WorldTopic::Survival,
        WorldTopic::Chat,
        WorldTopic::Environment,
        WorldTopic::Survival,
        WorldTopic::Environment,
        WorldTopic::Survival,
    ];
    assert_eq!(
        assembled.iter().map(view_tag).collect::<Vec<_>>(),
        expected,
        "the tickless chat keeps its source position between the equal-tick publications"
    );
    let ticks = [Some(0), Some(0), None, Some(0), Some(0), Some(0), Some(0)];
    for (record, tick) in assembled.iter().zip(ticks) {
        assert_eq!(record.header().source_tick(), tick, "no tick is shifted");
    }
    // The interleaved weather scalars prove the order is the observations',
    // never a tick comparison: the second and third publications share tick
    // zero with the first yet follow it in committed order.
    assert_eq!(assembled[0].view(), &WorldUiView::Environment(first));
    assert_eq!(assembled[3].view(), &WorldUiView::Environment(second));
    assert_eq!(assembled[5].view(), &WorldUiView::Environment(third));
}

/// `derived prompt follows the sampled confirmed revision`: the prompt is a
/// derived local value with no source event of its own — the landed provider
/// publishes the schema's empty prompt state even beside a current recorded
/// ray target, and the assembled family carries no invented prompt record;
/// and a schema-admitted derived prompt envelope under the `AfterConfirmed`
/// order validates under the confirmed rules and merges after every
/// confirmed entry at its sampled revision, carrying no source tick and no
/// fabricated observation identity.
#[test]
fn derived_prompt_merges_after_confirmed_without_invented_event() {
    // The real half: a target-present player projection state contributes no
    // prompt record and no invented source event to the assembled family.
    let mut provider = admitted_mirror();
    let world = world_state(500, 6_000, Weather::Clear, Season::Spring, 0, 0);
    commit(&mut provider, player_event(7, world, survival(20, 0)));
    let fixture = view_fixture();
    let view = fixture.view_of(&provider);
    let parts = provider_parts(&view);
    assert!(
        parts[4].is_empty(),
        "the landed prompt provider publishes the empty prompt state"
    );
    let assembled = assemble_world_ui(
        view.frame_epoch(),
        view.frame_revision(),
        parts,
        &ClientLimits::try_new().expect("limits"),
    )
    .expect("the real providers assemble");
    assert!(
        !assembled
            .iter()
            .any(|record| view_tag(record) == WorldTopic::Prompt),
        "no prompt record is invented from a recorded position alone"
    );

    // The schema-admitted derived shape: two confirmed environment
    // republications and one confirmed chat fact beside two derived prompt
    // envelopes. Every derived entry sorts after every confirmed entry —
    // even after the confirmed revision 5 that exceeds the sampled 4 — and
    // the derived entries order among themselves by sampled revision, then
    // local sequence and record ordinal.
    let mut accepted = empty_parts();
    accepted[0] = vec![
        envelope(
            2,
            0,
            world_key(WorldTopic::Environment, None),
            environment_record(world_state(0, 0, Weather::Clear, Season::Spring, 0, 0)),
        ),
        envelope(
            5,
            0,
            world_key(WorldTopic::Environment, None),
            environment_record(world_state(1, 1, Weather::Rain, Season::Summer, 1, 1)),
        ),
    ];
    accepted[2] = vec![envelope(
        3,
        0,
        world_key(
            WorldTopic::Chat,
            Some(
                ObservationKey::try_new(epoch(), ConfirmedRevision::new(3), 0).expect("staged key"),
            ),
        ),
        chat_record(&accepted_fact(2, player_id(0xA2))),
    )];
    accepted[4] = vec![
        after_envelope(
            EPOCH,
            4,
            1,
            0,
            world_key(WorldTopic::Prompt, None),
            prompt_record(prompt_view(1, 64, -2, "石头")),
        ),
        after_envelope(
            EPOCH,
            9,
            2,
            0,
            world_key(WorldTopic::Prompt, None),
            prompt_record(prompt_view(2, 65, -3, "泥土")),
        ),
    ];
    let derived = assemble_hand_built(accepted).expect("derived entries validate and assemble");
    assert_eq!(
        derived.iter().map(view_tag).collect::<Vec<_>>(),
        vec![
            WorldTopic::Environment,
            WorldTopic::Chat,
            WorldTopic::Environment,
            WorldTopic::Prompt,
            WorldTopic::Prompt,
        ],
        "derived entries merge after every confirmed entry, then by sampled revision"
    );
    assert_eq!(
        derived[3].header().source_tick(),
        None,
        "a derived prompt record invents no source tick"
    );
    assert_eq!(derived[4].header().source_tick(), None);
    assert_eq!(
        derived[3].view(),
        &WorldUiView::Prompt(Some(prompt_view(1, 64, -2, "石头"))),
        "the derived prompt payload is emitted unchanged"
    );
}

/// `stale prompt`: a derived order key from another epoch is stale, a
/// derived record rebased onto a revision other than the coherent candidate
/// is incoherent, and a chat or task envelope under the derived order is
/// incoherent too — a chat or task identity is the observation of its
/// carrying publication, and a derived envelope has no carrying publication
/// to name, so admitting one would fabricate the identity. Each rejects the
/// whole family beside otherwise valid parts, without partial output.
#[test]
fn stale_or_misrebased_derived_records_reject_typed() {
    // A foreign epoch inside the derived prompt order key is stale, and the
    // whole family rejects beside an otherwise valid confirmed part.
    let mut stale = empty_parts();
    stale[0] = vec![envelope(
        1,
        0,
        world_key(WorldTopic::Environment, None),
        environment_record(world_state(0, 0, Weather::Clear, Season::Spring, 0, 0)),
    )];
    stale[4] = vec![after_envelope(
        EPOCH + 1,
        2,
        1,
        0,
        world_key(WorldTopic::Prompt, None),
        prompt_record(prompt_view(1, 64, -2, "石头")),
    )];
    assert_eq!(
        assemble_hand_built(stale).expect_err("a derived order key from another epoch is stale"),
        ClientError::StaleEpoch
    );

    // A derived record rebased onto its sampled revision instead of the
    // frame's coherent candidate disagrees with the frame it rides.
    let mut misrebased = empty_parts();
    misrebased[4] = vec![
        OrderedRecord::try_new(
            ProjectionOrder::AfterConfirmed {
                epoch: epoch(),
                revision: ConfirmedRevision::new(4),
                local_sequence: 1,
                record_ordinal: 0,
            },
            world_key(WorldTopic::Prompt, None),
            WorldUiRecord::try_new(
                RecordHeader::try_new(
                    epoch(),
                    ConfirmedRevision::new(4),
                    None,
                    FamilyOperation::Upsert,
                )
                .expect("checked header"),
                WorldUiView::Prompt(Some(prompt_view(1, 64, -2, "石头"))),
            )
            .expect("checked prompt record"),
        )
        .expect("checked envelope"),
    ];
    assert_eq!(
        assemble_hand_built(misrebased).expect_err(
            "a derived record at its sampled revision instead of the frame's is incoherent"
        ),
        ClientError::InvalidInput
    );

    // A chat envelope under the derived order cannot name the observation
    // identity its record carries.
    let mut derived_chat = empty_parts();
    derived_chat[2] = vec![after_envelope(
        EPOCH,
        4,
        1,
        0,
        world_key(WorldTopic::Chat, None),
        chat_record(&accepted_fact(3, player_id(0xA3))),
    )];
    assert_eq!(
        assemble_hand_built(derived_chat)
            .expect_err("a derived chat envelope has no carrying publication to name"),
        ClientError::InvalidInput
    );
}

/// `mixed revision`: parts projected for one epoch and candidate revision
/// assemble only under exactly that identity — a foreign epoch is stale and
/// a foreign revision is incoherent, both without any partial output.
#[test]
fn foreign_epoch_or_revision_rejects_without_partial_output() {
    let mut provider = admitted_mirror();
    commit(
        &mut provider,
        player_event(
            5,
            world_state(100, 1_000, Weather::Clear, Season::Spring, 0, 0),
            survival(20, 0),
        ),
    );

    let fixture = view_fixture();
    let view = fixture.view_of(&provider);
    let parts = provider_parts(&view);
    let limits = ClientLimits::try_new().expect("limits");

    let stale = assemble_world_ui(
        SessionEpoch::try_new(EPOCH + 1).expect("foreign epoch"),
        view.frame_revision(),
        parts.clone(),
        &limits,
    )
    .expect_err("a part from another epoch is stale");
    assert_eq!(stale, ClientError::StaleEpoch);

    let incoherent = assemble_world_ui(
        view.frame_epoch(),
        ConfirmedRevision::new(view.frame_revision().get() + 1),
        parts,
        &limits,
    )
    .expect_err("a part rebased onto another revision is incoherent");
    assert_eq!(incoherent, ClientError::InvalidInput);
}

/// `duplicate, ambiguous and disagreeing envelopes`: two envelopes claiming
/// one retained order slot for one stable identity — in one part or across
/// two parts — an envelope whose stable identity disagrees with the record
/// it wraps or with its own order key, and a record under a removal tag
/// each reject the whole family typed. Distinct identities never reject:
/// two equal chat facts under distinct observation identities are the
/// cross-consumption duplicate admission shape — the resend of a consumed
/// observation enters as a fresh record under a new identity — and both
/// merge in retained order.
#[test]
fn duplicate_ambiguous_or_disagreeing_envelopes_reject_typed() {
    let observation = |revision: u64| {
        ObservationKey::try_new(epoch(), ConfirmedRevision::new(revision), 0).expect("staged key")
    };

    // Two environment envelopes claiming one order slot for the singleton
    // identity, in one part.
    let mut duplicate_in_part = empty_parts();
    duplicate_in_part[0] = vec![
        envelope(
            1,
            0,
            world_key(WorldTopic::Environment, None),
            environment_record(world_state(0, 0, Weather::Clear, Season::Spring, 0, 0)),
        ),
        envelope(
            1,
            0,
            world_key(WorldTopic::Environment, None),
            environment_record(world_state(0, 0, Weather::Clear, Season::Spring, 0, 0)),
        ),
    ];
    assert_eq!(
        assemble_hand_built(duplicate_in_part)
            .expect_err("two envelopes claiming one order slot reject"),
        ClientError::InvalidInput
    );

    // The same duplicated slot spread across two parts is the same
    // malformation: the parts order must never decide which record wins.
    let fact = accepted_fact(1, player_id(0xA4));
    let chat_key = world_key(WorldTopic::Chat, Some(observation(1)));
    let mut duplicate_across_parts = empty_parts();
    duplicate_across_parts[2] = vec![envelope(1, 0, chat_key, chat_record(&fact))];
    duplicate_across_parts[3] = vec![envelope(1, 0, chat_key, chat_record(&fact))];
    assert_eq!(
        assemble_hand_built(duplicate_across_parts)
            .expect_err("a duplicated order slot across parts rejects"),
        ClientError::InvalidInput
    );

    // A chat envelope whose stable identity names another observation than
    // the one its own order key retained.
    let mut foreign_identity = empty_parts();
    foreign_identity[2] = vec![envelope(
        1,
        0,
        world_key(WorldTopic::Chat, Some(observation(2))),
        chat_record(&fact),
    )];
    assert_eq!(
        assemble_hand_built(foreign_identity)
            .expect_err("an envelope naming a foreign observation identity rejects"),
        ClientError::InvalidInput
    );

    // A task view wrapped under a chat identity disagrees with the record.
    let mut foreign_topic = empty_parts();
    foreign_topic[2] = vec![envelope(
        1,
        0,
        world_key(WorldTopic::Chat, Some(observation(1))),
        WorldUiRecord::try_new(
            hand_header(FamilyOperation::Upsert),
            WorldUiView::Task(
                TaskView::try_new(
                    observation(1),
                    speaker(2),
                    command_text("挖石头"),
                    TaskState::Started,
                )
                .expect("checked task view"),
            ),
        )
        .expect("checked task record"),
    )];
    assert_eq!(
        assemble_hand_built(foreign_topic).expect_err("an envelope naming another topic rejects"),
        ClientError::InvalidInput
    );

    // A world record under a removal tag: no accepted provider emits one.
    let mut removed = empty_parts();
    removed[0] = vec![envelope(
        1,
        0,
        world_key(WorldTopic::Environment, None),
        WorldUiRecord::try_new(
            hand_header(FamilyOperation::Remove),
            WorldUiView::Environment(world_state(0, 0, Weather::Clear, Season::Spring, 0, 0)),
        )
        .expect("checked removal record"),
    )];
    assert_eq!(
        assemble_hand_built(removed).expect_err("a record under a removal tag rejects"),
        ClientError::InvalidInput
    );

    // The cross-consumption duplicate admission: two equal chat facts under
    // distinct observation identities at distinct order slots are fresh
    // records, never a malformation — the resent duplicate of a consumed
    // observation enters under its new identity and both merge in retained
    // order.
    let mut admission = empty_parts();
    admission[2] = vec![
        envelope(
            1,
            0,
            world_key(WorldTopic::Chat, Some(observation(1))),
            chat_record(&fact),
        ),
        envelope(
            2,
            0,
            world_key(WorldTopic::Chat, Some(observation(2))),
            chat_record(&fact),
        ),
    ];
    let admitted = assemble_hand_built(admission).expect("distinct-identity duplicates merge");
    assert_eq!(
        admitted.len(),
        2,
        "the resend is a fresh record, not a duplicate slot"
    );
    assert_eq!(
        admitted[0].view(),
        admitted[1].view(),
        "the equal payloads ride distinct identities"
    );

    // The real-provider shape of the same ruling: after the original
    // observation was consumed between frames, the resend alone in the
    // retained queue projects as one fresh record under its new identity.
    let mut provider = admitted_mirror();
    commit(&mut provider, chat_event(fact.clone()));
    let consumed_resend =
        AcceptedObservation::try_new(observation(2), None, packet(chat_event(fact)), Vec::new())
            .expect("staged resend");
    let fixture = view_fixture();
    let resend_queue = [consumed_resend];
    let resend_view = fixture.view(
        provider.mirror(),
        &resend_queue,
        epoch(),
        ConfirmedRevision::new(3),
    );
    let resend_parts = provider_parts(&resend_view);
    let fresh = assemble_world_ui(
        resend_view.frame_epoch(),
        resend_view.frame_revision(),
        resend_parts,
        &ClientLimits::try_new().expect("limits"),
    )
    .expect("the consumed observation's resend assembles as a fresh record");
    assert_eq!(fresh.len(), 1, "one fresh record under the new identity");
    assert_eq!(view_tag(&fresh[0]), WorldTopic::Chat);
}

/// `family count cap plus one`: an assembly at exactly the tightened family
/// record count publishes, one record more is a typed capacity rejection,
/// and the caller's retained previous output stays exactly as it was.
#[test]
fn family_count_cap_plus_one_retains_old_output() {
    let sender = player_id(0xA5);
    let mut provider = admitted_mirror();
    commit(&mut provider, chat_event(accepted_fact(1, sender)));
    commit(&mut provider, chat_event(accepted_fact(2, sender)));
    commit(&mut provider, chat_event(accepted_fact(3, sender)));

    let fixture = view_fixture();
    let frozen = ClientLimits::try_new().expect("frozen limits");
    let at_cap = limits_with(3, frozen.frame_bytes());
    let old = assemble(&fixture, &provider, &at_cap)
        .expect("exactly three records fit the family count cap");
    assert_eq!(old.len(), 3);

    commit(&mut provider, chat_event(accepted_fact(4, sender)));
    let error = assemble(&fixture, &provider, &at_cap)
        .expect_err("one record past the family count cap rejects");
    assert_eq!(error, ClientError::Capacity);
    assert_eq!(old.len(), 3, "the retained previous output is unchanged");
    assert_eq!(
        old[2].view(),
        &WorldUiView::Chat(accepted_fact(3, sender)),
        "the retained previous output keeps its newest record"
    );
}

/// `frame byte cap plus one`: an assembly whose minimal world-ui-only frame
/// exactly fits the frame byte cap publishes, one record more is a typed
/// capacity rejection, and the retained previous output stays intact.
#[test]
fn frame_byte_cap_plus_one_retains_old_output() {
    let sender = player_id(0xA6);
    let mut provider = admitted_mirror();
    commit(&mut provider, chat_event(accepted_fact(1, sender)));
    commit(&mut provider, chat_event(accepted_fact(2, sender)));
    commit(&mut provider, chat_event(accepted_fact(3, sender)));

    let fixture = view_fixture();
    let frozen = ClientLimits::try_new().expect("frozen limits");
    let old = assemble(&fixture, &provider, &frozen)
        .expect("three chat records assemble under the frozen caps");
    assert_eq!(old.len(), 3);

    let view = fixture.view_of(&provider);
    let exact_cap = minimal_frame_size(view.frame_epoch(), view.frame_revision(), &old);
    let at_cap = limits_with(frozen.family_records(), exact_cap);
    let fitted = assemble(&fixture, &provider, &at_cap)
        .expect("the exact minimal world-ui frame fits its own byte cap");
    assert_eq!(fitted.len(), 3);

    commit(&mut provider, chat_event(accepted_fact(4, sender)));
    let error = assemble(&fixture, &provider, &at_cap)
        .expect_err("one record past the frame byte cap rejects");
    assert_eq!(error, ClientError::Capacity);
    assert_eq!(fitted.len(), 3, "the retained previous output is unchanged");
    assert_eq!(
        fitted[2].view(),
        &WorldUiView::Chat(accepted_fact(3, sender))
    );
}

/// `empty retained window`: an assembly over empty parts publishes the empty
/// family and repeated calls agree — the assembler is pure per frame and
/// builds no HUD persistence state: the family reflects the
/// retained-observation window of the sampled revision alone, and whether
/// the presentation consumer keeps the last window visible across empty
/// frames is that consumer's concern, never a state this family owns.
#[test]
fn empty_parts_assemble_empty_without_invented_state() {
    let first = assemble_hand_built(empty_parts()).expect("empty parts assemble empty");
    let second = assemble_hand_built(empty_parts()).expect("repeat assembly agrees");
    assert!(
        first.is_empty(),
        "an empty retained window publishes no record"
    );
    assert_eq!(
        first, second,
        "the assembler builds no state between frames"
    );
}
