//! Task lifecycle projection contract tests.
//!
//! The table pins the accepted task semantics against the landed mirror
//! provider: the Started→Progress→Completed chain plus the TimedOut, Stopped
//! and Failed facts arrive as confirmed chat observations whose body is the
//! accepted task branch, and each projects exactly one upsert carrying the
//! checked companion speaker, the restated command and the closed lifecycle
//! state. The identity of every record is the actual observation key the
//! mirror issued — never an invented task id, generation, percentage,
//! Pending/Running state or fabricated tick — because the accepted schema has
//! no such wire field at all. Duplicate observation identities never
//! double-emit, observations from another epoch reject the whole projection,
//! and every foreign topic (accepted chat, speech, player publications,
//! combat hits) contributes nothing.

use mornlea_client_core::contracts::{
    ClientError, ClientLimits, ConfirmedRevision, FamilyOperation, ObservationKey, SessionEpoch,
};
use mornlea_client_core::input::InputProjectionState;
use mornlea_client_core::prediction::PlayerProjectionState;
use mornlea_client_core::presentation::frame::WorldUiRecord;
use mornlea_client_core::presentation::world_ui::task::project_task;
use mornlea_client_core::presentation::{
    AcceptedObservation, AudioProjectionState, DiagnosticProjectionState, ErrorClassCounters,
    LifecycleProjectionState, MovementIntent, OrderedRecord, Pose, ProducerIdentity,
    ProjectionOrder, ProjectionView, QueueCounters, StableRecordKey, WorldTopic, WorldUiView,
};
use mornlea_client_core::session::ConfirmedMirror;
use mornlea_client_core::session::mirror::MirrorProvider;
use mornlea_domain::{
    ChatBody, ChatEvent, ChatEventParts, CombatHit, CombatTarget, CommandText, CompanionId,
    CompanionName, CompanionSpeaker, DisplayName, DomainError, Event, FiniteVec3, LookAngles,
    MiningState, MotionState, MotionStateParts, PlayerId, PlayerState, PlayerStateParts,
    SurvivalState, SurvivalStateParts, TaskFailure, TaskState, Weather, WorldState,
    WorldStateParts,
};
use mornlea_protocol::{KeepAlive, ServerPacket};

/// The epoch every fixture mirror and observation key carries.
const EPOCH: u64 = 33;

/// A checked UUIDv4 companion identity, the addressed executor of the task.
fn companion_id() -> CompanionId {
    let mut bytes = [0u8; 16];
    bytes[0] = 0x22;
    bytes[6] = 0x40;
    bytes[8] = 0x80;
    bytes[15] = 0x07;
    CompanionId::try_from_bytes(bytes).expect("checked uuid v4 companion identity")
}

/// A checked UUIDv4 player identity for the chat fixtures that name the
/// issuing player.
fn player_id() -> PlayerId {
    let mut bytes = [0u8; 16];
    bytes[0] = 0x11;
    bytes[6] = 0x40;
    bytes[8] = 0x80;
    bytes[15] = 0x01;
    PlayerId::try_from_bytes(bytes).expect("checked uuid v4 player identity")
}

fn player_name() -> DisplayName {
    DisplayName::try_from_canonical("Chen".to_owned()).expect("canonical player name")
}

fn companion_name() -> CompanionName {
    CompanionName::try_from_canonical("阿木".to_owned()).expect("canonical companion name")
}

fn speaker() -> CompanionSpeaker {
    CompanionSpeaker::new(companion_id(), companion_name())
}

fn command_text(text: &str) -> CommandText {
    CommandText::try_from_canonical(text.to_owned()).expect("canonical command text")
}

/// One task lifecycle chat observation for the addressed command: the
/// authority restates the original instruction and names one closed state.
fn task_event(event_id: u64, state: TaskState) -> Event {
    Event::Chat(
        ChatEvent::try_new(ChatEventParts {
            event_id,
            player_id: player_id(),
            player_name: player_name(),
            body: ChatBody::Task {
                companion: speaker(),
                command: command_text("挖石头"),
                state,
            },
        })
        .expect("checked chat event"),
    )
}

/// One addressing-accepted chat observation: a foreign world-ui topic the
/// chat provider owns, never a task record.
fn accepted_chat_event(event_id: u64) -> Event {
    Event::Chat(
        ChatEvent::try_new(ChatEventParts {
            event_id,
            player_id: player_id(),
            player_name: player_name(),
            body: ChatBody::Accepted {
                companion: speaker(),
                command: command_text("挖石头"),
            },
        })
        .expect("checked chat event"),
    )
}

/// One companion speech observation: the only branch that never restates the
/// command and never carries a task state.
fn speech_chat_event(event_id: u64) -> Event {
    Event::Chat(
        ChatEvent::try_new(ChatEventParts {
            event_id,
            player_id: player_id(),
            player_name: player_name(),
            body: ChatBody::Speech {
                companion: speaker(),
                text: mornlea_domain::SpeechText::try_from_canonical("好的，这就去。".to_owned())
                    .expect("bounded speech"),
            },
        })
        .expect("checked chat event"),
    )
}

/// One overworld player publication at the caller's server tick: a foreign
/// topic whose server tick must never shift onto a task record.
fn player_event(tick: u64) -> Event {
    Event::PlayerState(PlayerState::new(PlayerStateParts {
        server_tick: tick,
        last_input_sequence: 0,
        dimension: mornlea_domain::Dimension::OVERWORLD,
        motion: MotionState::new(MotionStateParts {
            position: FiniteVec3::try_new([0.0, 64.0, 0.0]).expect("finite fixture position"),
            velocity: FiniteVec3::try_new([0.0, 0.0, 0.0]).expect("finite fixture velocity"),
            on_ground: true,
        }),
        look: LookAngles::try_new(0.0, 0.0).expect("finite fixture look"),
        ready: true,
        reset: false,
        mining: MiningState::Idle,
        survival: SurvivalState::try_new(SurvivalStateParts {
            health: 20,
            oxygen: 300,
            hunger: 20,
            saturation_zero: false,
            armor_points: 15,
        })
        .expect("checked survival fixture"),
        world: WorldState::try_new(WorldStateParts {
            day_phase_offset: 1_000,
            world_time_ticks: 20_000,
            weather: Weather::Clear,
            season: mornlea_domain::Season::Summer,
            season_progress: 10,
            temperature: 5,
        })
        .expect("checked world fixture"),
    }))
}

/// One combat-hit observation against the player: a confirmed damage fact
/// that carries a server tick but no task fact.
fn combat_hit_event(tick: u64) -> Event {
    Event::CombatHit(CombatHit::try_new(tick, 5, CombatTarget::Player).expect("checked combat hit"))
}

fn packet(event: Event) -> ServerPacket {
    ServerPacket::try_from(event).expect("observation converts to its packet")
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
    fn new(epoch: SessionEpoch, revision: ConfirmedRevision) -> Self {
        Self {
            input: InputProjectionState::try_new(1).expect("input projection state"),
            player: PlayerProjectionState::try_new(
                epoch,
                revision,
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

    /// A deliberately different local-state fixture: another input sequence,
    /// predicted pose and lifecycle generation beside the same confirmed
    /// mirror facts, pinning that no local owner feeds the task records.
    fn with_divergent_local_state(epoch: SessionEpoch, revision: ConfirmedRevision) -> Self {
        let mut fixture = Self::new(epoch, revision);
        fixture.input = InputProjectionState::try_new(77).expect("input projection state");
        fixture.player = PlayerProjectionState::try_new(
            epoch,
            revision,
            Pose::try_new([12.0, 70.0, -4.0], 1.25, -0.5).expect("finite divergent pose"),
            Some(Pose::try_new([12.5, 70.0, -4.0], 1.25, -0.5).expect("finite predicted pose")),
            Vec::new(),
            None,
            None,
            MovementIntent::try_new(None, true).expect("movement intent"),
        )
        .expect("player projection state");
        fixture.lifecycle = LifecycleProjectionState::try_new(9).expect("lifecycle state");
        fixture
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
    ViewFixture::new(
        SessionEpoch::try_new(EPOCH).expect("epoch"),
        ConfirmedRevision::new(1),
    )
}

/// The task payload of one record, refusing a foreign view tag: this
/// projection publishes the task view alone.
fn task_view(entry: &OrderedRecord<WorldUiRecord>) -> &mornlea_client_core::presentation::TaskView {
    match entry.record().view() {
        WorldUiView::Task(view) => view,
        other => panic!("task record carries a foreign view tag: {other:?}"),
    }
}

/// The shared stable-key assertion: the task topic tag beside the actual
/// observation identity, because each task fact is its own record and the
/// identity comes from the mirror-issued key, never an invented task id or
/// the chat event's acknowledgment id.
fn assert_stable_key(entry: &OrderedRecord<WorldUiRecord>, key: &ObservationKey) {
    assert_eq!(
        *entry.stable_key(),
        StableRecordKey::World {
            topic: WorldTopic::Task,
            identity: Some(*key),
        }
    );
}

/// Asserts one entry is server-sourced with the exact observation identity
/// and packet record ordinal, never a reconstructed or local order.
fn assert_confirmed_order(entry: &OrderedRecord<WorldUiRecord>, revision: u64, ordinal: u32) {
    match entry.order() {
        ProjectionOrder::Confirmed {
            observation,
            record_ordinal,
        } => {
            assert_eq!(observation.epoch().get(), EPOCH);
            assert_eq!(observation.confirmed_revision().get(), revision);
            assert_eq!(*record_ordinal, ordinal);
        }
        other => panic!("task record is server-sourced: {other:?}"),
    }
}

/// The closed task lifecycle set, matched exhaustively without a wildcard so
/// a state or failure reason added later fails to compile here: there is no
/// Pending or Running member and no percentage payload to project.
fn assert_closed_state(state: TaskState) {
    match state {
        TaskState::Started | TaskState::Progress | TaskState::Completed => {}
        TaskState::TimedOut | TaskState::Stopped => {}
        TaskState::Failed(reason) => match reason {
            TaskFailure::PlannerUnavailable
            | TaskFailure::InvalidPlan
            | TaskFailure::PathUnreachable
            | TaskFailure::WorldChanged
            | TaskFailure::InventoryFull => {}
        },
    }
}

/// `Started→Progress→Completed` plus `TimedOut/Stopped/Failed`: the chain is
/// six confirmed chat observations, and each projects exactly one upsert
/// carrying the checked speaker, the restated command and the closed state in
/// actual source order, with the actual observation identity as the stable
/// identity and no invented source tick — chat observations carry none.
#[test]
fn task_lifecycle_chain_projects_in_accepted_order() {
    let chain = [
        TaskState::Started,
        TaskState::Progress,
        TaskState::Completed,
        TaskState::TimedOut,
        TaskState::Stopped,
        TaskState::Failed(TaskFailure::PlannerUnavailable),
    ];
    let mut provider = admitted_mirror(EPOCH);
    for (index, state) in chain.iter().enumerate() {
        commit(&mut provider, task_event(10 + index as u64, *state));
    }

    let fixture = view_fixture();
    let view = fixture.view_of(&provider);
    let records = project_task(&view).expect("six task records project");
    assert_eq!(records.len(), 6, "one record per confirmed task fact");

    for (index, entry) in records.iter().enumerate() {
        let state = chain[index];
        assert_closed_state(state);
        let key = *provider.observations()[index].key();
        let expected = mornlea_client_core::presentation::TaskView::try_new(
            key,
            speaker(),
            command_text("挖石头"),
            state,
        )
        .expect("checked task view");
        assert_eq!(task_view(entry), &expected, "fact {index} projects exactly");
        assert_eq!(entry.record().header().operation(), FamilyOperation::Upsert);
        assert_eq!(
            entry.record().header().source_tick(),
            None,
            "chat observations carry no server tick and none is invented"
        );
        assert_eq!(entry.record().header().epoch().get(), EPOCH);
        assert_eq!(
            entry.record().header().revision(),
            view.frame_revision(),
            "headers are rebased onto the coherent candidate revision"
        );
        assert_stable_key(entry, &key);
        assert_confirmed_order(entry, 1 + index as u64, 0);
    }
    assert!(records[0].order() < records[1].order());
    assert!(records[1].order() < records[2].order());
    assert!(records[2].order() < records[3].order());
    assert!(records[3].order() < records[4].order());
    assert!(records[4].order() < records[5].order());
}

/// The failed fact's reason payload is the closed five-reason set the Go
/// oracle sweeps, and each reason projects exactly as published, never
/// coalesced or renumbered.
#[test]
fn failed_reasons_are_the_closed_five() {
    let reasons = [
        TaskFailure::PlannerUnavailable,
        TaskFailure::InvalidPlan,
        TaskFailure::PathUnreachable,
        TaskFailure::WorldChanged,
        TaskFailure::InventoryFull,
    ];
    let mut provider = admitted_mirror(EPOCH);
    for (index, reason) in reasons.iter().enumerate() {
        commit(
            &mut provider,
            task_event(20 + index as u64, TaskState::Failed(*reason)),
        );
    }

    let fixture = view_fixture();
    let records = project_task(&fixture.view_of(&provider)).expect("five failures project");
    assert_eq!(records.len(), 5, "one record per failed fact");
    for (index, entry) in records.iter().enumerate() {
        assert_eq!(task_view(entry).state(), &TaskState::Failed(reasons[index]));
        assert_closed_state(*task_view(entry).state());
    }
}

/// `no invented ID/generation/%/Pending/Running`: the task record's every
/// fact is the authority's own — the identity is the actual observation key,
/// the command and speaker are the restated payload, no progress percentage
/// or task generation exists to project, a command text plus-one over its
/// accepted cap refuses typed at the checked domain boundary before any
/// observation exists, foreign topics contribute nothing, a ticked
/// publication's tick never shifts onto a task record, local owners never
/// feed the records, and the projection is pure. A packet that is not a
/// checked event publication rejects the whole projection.
#[test]
fn task_records_carry_no_invented_facts() {
    let mut provider = admitted_mirror(EPOCH);
    commit(&mut provider, task_event(30, TaskState::Started));
    commit(&mut provider, player_event(31));
    commit(&mut provider, combat_hit_event(32));
    commit(&mut provider, accepted_chat_event(33));
    commit(&mut provider, speech_chat_event(34));
    commit(&mut provider, task_event(35, TaskState::Completed));

    let fixture = view_fixture();
    let view = fixture.view_of(&provider);
    let records = project_task(&view).expect("only the task facts project");
    assert_eq!(
        records.len(),
        2,
        "accepted chat, speech, player and combat topics contribute nothing"
    );
    assert_eq!(
        task_view(&records[0]).state(),
        &TaskState::Started,
        "the facts project in actual source order"
    );
    assert_eq!(task_view(&records[1]).state(), &TaskState::Completed);
    for entry in &records {
        assert_eq!(
            entry.record().header().source_tick(),
            None,
            "the ticked player publication and combat hit never shift a tick here"
        );
        assert_stable_key(entry, task_view(entry).observation());
    }
    assert_eq!(
        project_task(&view).expect("repeat projection"),
        records,
        "the projection is pure: no local state accumulates between calls"
    );

    let divergent = ViewFixture::with_divergent_local_state(
        SessionEpoch::try_new(EPOCH).expect("epoch"),
        ConfirmedRevision::new(1),
    );
    assert_eq!(
        project_task(&divergent.view_of(&provider)).expect("local owners stay irrelevant"),
        records,
        "local input, prediction and lifecycle owners never feed the task records"
    );

    // The command cap is the checked domain boundary: 1024 bytes admit and
    // the 1025th byte refuses typed before any observation could exist.
    let at_cap = "a".repeat(1024);
    assert_eq!(at_cap.len(), 1024);
    assert!(
        CommandText::try_from_canonical(at_cap).is_ok(),
        "the cap admits N"
    );
    let over_cap = "a".repeat(1025);
    assert_eq!(over_cap.len(), 1025);
    assert_eq!(
        CommandText::try_from_canonical(over_cap),
        Err(DomainError::InvalidText),
        "the cap rejects N plus one typed"
    );

    let epoch = SessionEpoch::try_new(EPOCH).expect("epoch");
    let valid = provider.observations()[0].clone();
    let non_event = AcceptedObservation::try_new(
        ObservationKey::try_new(epoch, ConfirmedRevision::new(7), 0).expect("key"),
        None,
        ServerPacket::KeepAlive(KeepAlive::new(1).expect("checked keep-alive token")),
        Vec::new(),
    )
    .expect("staged with a non-publication packet");
    assert_eq!(
        project_task(&fixture.view(
            provider.mirror(),
            &[valid, non_event],
            epoch,
            ConfirmedRevision::new(7)
        )),
        Err(ClientError::InvalidInput),
        "a packet that is not an event publication rejects without a partial prefix"
    );
}

/// `duplicate/old epoch`: a duplicate observation identity is refused by the
/// accepted mirror before this projection ever sees it — the committed
/// revision, the observation queue and the projected output all stay
/// unchanged — and an identity handed to the projection twice never
/// double-emits. An old-epoch observation rejects the whole projection under
/// a fresh frame epoch, and a mixed queue rejects whole, never a partial
/// prefix.
#[test]
fn duplicate_and_old_epoch_inputs_reject_whole() {
    let mut provider = admitted_mirror(EPOCH);
    commit(&mut provider, task_event(40, TaskState::Progress));

    let fixture = view_fixture();
    let baseline = project_task(&fixture.view_of(&provider)).expect("baseline projects");
    assert_eq!(baseline.len(), 1);

    let staged = |provider: &MirrorProvider, revision: u64, event: Event| {
        AcceptedObservation::try_new(
            ObservationKey::try_new(
                provider.mirror().epoch(),
                ConfirmedRevision::new(revision),
                0,
            )
            .expect("key"),
            None,
            packet(event),
            Vec::new(),
        )
        .expect("staged")
    };
    let later = task_event(41, TaskState::Completed);
    assert!(
        provider
            .commit(&staged(&provider, 1, later.clone()))
            .is_err(),
        "a revision the mirror already issued is a duplicate"
    );
    assert!(
        provider
            .commit(&staged(&provider, 0, later.clone()))
            .is_err(),
        "a backward revision is a malformed observation identity"
    );
    assert!(
        provider.commit(&staged(&provider, 3, later)).is_err(),
        "an ahead-of-sequence revision is a malformed observation identity"
    );
    assert_eq!(provider.mirror().revision().get(), 1);
    assert_eq!(provider.observations().len(), 1);
    assert_eq!(
        project_task(&fixture.view_of(&provider)).expect("projection still succeeds"),
        baseline,
        "the refused duplicates leave the projection unchanged"
    );

    // The provider-level duplicate rule: the same observation identity
    // handed to the projection twice emits exactly one record, never two.
    let duplicated = vec![
        provider.observations()[0].clone(),
        provider.observations()[0].clone(),
    ];
    let deduped = project_task(&fixture.view(
        provider.mirror(),
        &duplicated,
        provider.mirror().epoch(),
        ConfirmedRevision::new(2),
    ))
    .expect("duplicated queue projects once");
    assert_eq!(
        deduped.len(),
        1,
        "duplicate events never double-emit one identity"
    );
    assert_eq!(deduped, baseline);

    let next_epoch_value = EPOCH + 1;
    let reset_provider = admitted_mirror(next_epoch_value);
    assert!(
        project_task(&fixture.view_of(&reset_provider))
            .expect("empty queue projects")
            .is_empty(),
        "a reset mirror carries no previous-session task facts"
    );

    let next_epoch = SessionEpoch::try_new(next_epoch_value).expect("next epoch");
    let old_epoch_observation = provider.observations()[0].clone();
    assert_eq!(
        project_task(&fixture.view(
            reset_provider.mirror(),
            &[old_epoch_observation],
            next_epoch,
            ConfirmedRevision::new(1)
        )),
        Err(ClientError::StaleEpoch),
        "an old-epoch observation never enters the new epoch's frame"
    );

    let mut fresh = admitted_mirror(next_epoch_value);
    commit(&mut fresh, task_event(2, TaskState::Started));
    let stale = provider.observations()[0].clone();
    assert_ne!(
        stale.key().epoch().get(),
        next_epoch_value,
        "the stale observation belongs to the reset-away epoch"
    );
    let mixed = vec![fresh.observations()[0].clone(), stale];
    assert_eq!(
        project_task(&fixture.view(
            fresh.mirror(),
            &mixed,
            next_epoch,
            ConfirmedRevision::new(2)
        )),
        Err(ClientError::StaleEpoch),
        "a mixed queue rejects whole, never a partial prefix"
    );
}
