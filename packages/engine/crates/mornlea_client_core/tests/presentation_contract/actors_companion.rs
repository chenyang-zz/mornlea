//! Companion actor projection contract tests.
//!
//! The table pins the accepted companion semantics against the landed mirror
//! provider: spawn/state/remove/reuse over real committed observations, the
//! typed UUID identity, the canonical name on the spawn alone, the pose with
//! its vertical-range pitch on the spawn and state records, the reset bit on
//! the state records alone, no task or death field on any record, and
//! unchanged state for late, duplicate, malformed or old-epoch inputs. Order
//! keys are the actual observation keys with packet record ordinals; equal,
//! absent or backwards source ticks never reconstruct order.

use mornlea_client_core::contracts::{
    ClientError, ClientLimits, ConfirmedRevision, FamilyOperation, ObservationKey, SessionEpoch,
};
use mornlea_client_core::input::InputProjectionState;
use mornlea_client_core::prediction::PlayerProjectionState;
use mornlea_client_core::presentation::actors::companion::project_companion;
use mornlea_client_core::presentation::frame::ActorRecord;
use mornlea_client_core::presentation::{
    AcceptedObservation, ActorDetail, ActorDimension, ActorId, ActorKind, AudioProjectionState,
    DiagnosticProjectionState, ErrorClassCounters, LifecycleProjectionState, MovementIntent,
    OrderedRecord, Pose, ProducerIdentity, ProjectionOrder, ProjectionView, QueueCounters,
    ResolvedActor, StableRecordKey,
};
use mornlea_client_core::session::ConfirmedMirror;
use mornlea_client_core::session::mirror::MirrorProvider;
use mornlea_domain::{
    ChatBody, ChatEvent, ChatEventParts, CommandText, CompanionDespawn, CompanionId, CompanionName,
    CompanionSpawn, CompanionSpawnParts, CompanionSpeaker, CompanionState, CompanionStateParts,
    CompanionStates, CompanionStatesParts, Dimension, Event, FiniteVec3, LookAngles,
    PassiveDespawn, PassiveDespawnParts, PassiveDespawnReason, PassiveDespawnRecord, PassiveId,
    PassiveSpawn, PassiveSpawnParts, PassiveSpawnRecord, PassiveSpawnRecordParts, PlayerId,
    TaskFailure, TaskState,
};
use mornlea_protocol::{KeepAlive, ServerPacket};

/// The epoch every fixture mirror and observation key carries.
const EPOCH: u64 = 7;

/// A checked UUIDv4 companion identity whose raw byte order follows `last`,
/// so two distinct seeds are strictly ordered the way a state batch requires.
fn companion(last: u8) -> CompanionId {
    let mut bytes = [0u8; 16];
    bytes[0] = 0x12;
    bytes[6] = 0x40;
    bytes[8] = 0x80;
    bytes[15] = last;
    CompanionId::try_from_bytes(bytes).expect("checked uuid v4 companion identity")
}

/// A checked UUIDv4 player identity for the chat fixtures that name the
/// issuing player.
fn player_id(first: u8) -> PlayerId {
    let mut bytes = [0u8; 16];
    bytes[0] = first;
    bytes[6] = 0x40;
    bytes[8] = 0x80;
    PlayerId::try_from_bytes(bytes).expect("checked uuid v4 player identity")
}

fn companion_name(text: &str) -> CompanionName {
    CompanionName::try_from_canonical(text.to_owned()).expect("canonical companion name")
}

fn command_text(text: &str) -> CommandText {
    CommandText::try_from_canonical(text.to_owned()).expect("canonical command text")
}

fn finite_pos(components: [f32; 3]) -> FiniteVec3 {
    FiniteVec3::try_new(components).expect("finite position")
}

fn finite_look(yaw: f32, pitch: f32) -> LookAngles {
    LookAngles::try_new(yaw, pitch).expect("finite look angles")
}

/// One companion spawn event: the complete first-seen body with the canonical
/// name. The domain admits the overworld alone for a companion.
fn spawn_event(
    id: CompanionId,
    name: &str,
    tick: u64,
    position: [f32; 3],
    yaw: f32,
    pitch: f32,
) -> Event {
    Event::CompanionSpawn(
        CompanionSpawn::try_new(CompanionSpawnParts {
            id,
            name: companion_name(name),
            server_tick: tick,
            dimension: Dimension::OVERWORLD,
            position: finite_pos(position),
            look: finite_look(yaw, pitch),
        })
        .expect("checked companion spawn"),
    )
}

/// One companion state record: the per-tick body with the reset marker.
fn state_record(
    id: CompanionId,
    position: [f32; 3],
    yaw: f32,
    pitch: f32,
    reset: bool,
) -> CompanionState {
    CompanionState::try_new(CompanionStateParts {
        id,
        dimension: Dimension::OVERWORLD,
        position: finite_pos(position),
        look: finite_look(yaw, pitch),
        reset,
    })
    .expect("checked companion state record")
}

fn states_event(tick: u64, records: Vec<CompanionState>) -> Event {
    Event::CompanionStates(
        CompanionStates::try_new(CompanionStatesParts {
            server_tick: tick,
            states: records.into_boxed_slice(),
        })
        .expect("checked companion state batch"),
    )
}

fn despawn_event(id: CompanionId) -> Event {
    Event::CompanionDespawn(CompanionDespawn::new(id))
}

/// Task-shaped chat observations: the addressed command and the task
/// lifecycle facts name the same companion identity through its speaker, but
/// they are world-ui records — no companion actor field can come from them.
fn task_shaped_chat_events(id: CompanionId) -> Vec<Event> {
    let speaker = CompanionSpeaker::new(id, companion_name("阿木"));
    vec![
        Event::Chat(
            ChatEvent::try_new(ChatEventParts {
                event_id: 1,
                player_id: player_id(0x90),
                player_name: mornlea_domain::DisplayName::try_from_canonical("Chen".to_owned())
                    .expect("canonical player name"),
                body: ChatBody::Accepted {
                    companion: speaker.clone(),
                    command: command_text("挖石头"),
                },
            })
            .expect("checked chat event"),
        ),
        Event::Chat(
            ChatEvent::try_new(ChatEventParts {
                event_id: 2,
                player_id: player_id(0x90),
                player_name: mornlea_domain::DisplayName::try_from_canonical("Chen".to_owned())
                    .expect("canonical player name"),
                body: ChatBody::Task {
                    companion: speaker,
                    command: command_text("挖石头"),
                    state: TaskState::Failed(TaskFailure::PlannerUnavailable),
                },
            })
            .expect("checked chat event"),
        ),
    ]
}

/// Death-shaped foreign observations: a passive mob that spawns and dies
/// publishes health and a died reason no companion record may ever carry.
fn passive_died_events(tick: u64) -> Vec<Event> {
    let id = PassiveId::try_new(5).expect("passive identity");
    let spawn = Event::PassiveSpawn(
        PassiveSpawn::try_new(PassiveSpawnParts {
            server_tick: tick,
            spawns: vec![
                PassiveSpawnRecord::try_new(PassiveSpawnRecordParts {
                    id,
                    dimension: Dimension::OVERWORLD,
                    position: finite_pos([2.0, 64.0, 2.0]),
                    yaw: 0.5,
                    health: 20,
                })
                .expect("checked passive spawn record"),
            ]
            .into_boxed_slice(),
        })
        .expect("checked passive spawn batch"),
    );
    let despawn = Event::PassiveDespawn(
        PassiveDespawn::try_new(PassiveDespawnParts {
            server_tick: tick + 1,
            despawns: vec![PassiveDespawnRecord::new(id, PassiveDespawnReason::Died)]
                .into_boxed_slice(),
        })
        .expect("checked passive despawn batch"),
    );
    vec![spawn, despawn]
}

fn packet(event: Event) -> ServerPacket {
    ServerPacket::try_from(event).expect("publication converts to its packet")
}

/// An admitted real mirror provider with no observations yet.
fn admitted_mirror() -> MirrorProvider {
    let epoch = SessionEpoch::try_new(EPOCH).expect("epoch");
    let mut provider =
        MirrorProvider::new(epoch, ClientLimits::try_new().expect("limits")).expect("mirror");
    provider.admit().expect("admitted session");
    provider
}

/// Commits one publication through the real mirror provider, so the retained
/// observation queue carries the actual source keys and identity resolution.
fn commit(provider: &mut MirrorProvider, event: Event) -> ConfirmedRevision {
    let next = provider.mirror().revision().get() + 1;
    let key = ObservationKey::try_new(
        SessionEpoch::try_new(EPOCH).expect("epoch"),
        ConfirmedRevision::new(next),
        0,
    )
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
        let epoch = SessionEpoch::try_new(EPOCH).expect("epoch");
        let revision = ConfirmedRevision::new(provider.mirror().revision().get() + 1);
        self.view(provider.mirror(), provider.observations(), epoch, revision)
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

/// Asserts one entry is server-sourced with the exact observation identity
/// and packet record ordinal, never a reconstructed or local order.
fn assert_confirmed_order(entry: &OrderedRecord<ActorRecord>, revision: u64, ordinal: u32) {
    match entry.order() {
        ProjectionOrder::Confirmed {
            observation,
            record_ordinal,
        } => {
            assert_eq!(observation.epoch().get(), EPOCH);
            assert_eq!(observation.confirmed_revision().get(), revision);
            assert_eq!(*record_ordinal, ordinal);
        }
        other => panic!("companion record is server-sourced: {other:?}"),
    }
}

/// The companion detail payload, refusing a foreign detail tag: the closed
/// union has the name and the reset bit alone, and no other field — no task
/// state, no failure reason, no death marker — can appear on a companion
/// record.
fn companion_detail(entry: &OrderedRecord<ActorRecord>) -> (Option<String>, Option<bool>) {
    match entry.record().detail() {
        None => (None, None),
        Some(ActorDetail::Companion { name, reset }) => {
            (name.as_ref().map(|name| name.as_str().to_owned()), *reset)
        }
        Some(other) => panic!("companion record carries a foreign detail tag: {other:?}"),
    }
}

/// The shared stable-key assertion: kind, actual dimension and the typed
/// UUID identity.
fn assert_stable_key(entry: &OrderedRecord<ActorRecord>, id: CompanionId) {
    assert_eq!(
        *entry.stable_key(),
        StableRecordKey::Actor {
            kind: ActorKind::Companion,
            dimension: ActorDimension::Known(Dimension::OVERWORLD),
            id: ActorId::Companion(id),
        }
    );
}

/// `spawn`: one upsert with the typed UUID identity, the actual spawn
/// dimension, the exact finite pose with the vertical-range pitch widened
/// from f32, the canonical name detail — and no velocity and no reset bit.
#[test]
fn spawn_projects_typed_identity_name_pose_and_pitch() {
    let mut provider = admitted_mirror();
    let id = companion(0x11);
    commit(
        &mut provider,
        spawn_event(id, "阿木", 10, [1.5, 70.25, -2.5], 0.25, -0.5),
    );

    let fixture = view_fixture();
    let view = fixture.view_of(&provider);
    let records = project_companion(&view).expect("spawn projects one record");
    assert_eq!(records.len(), 1);

    let entry = &records[0];
    assert_eq!(entry.record().kind(), ActorKind::Companion);
    assert_eq!(*entry.record().id(), ActorId::Companion(id));
    assert_eq!(
        *entry.record().dimension(),
        ActorDimension::Known(Dimension::OVERWORLD),
        "the spawn names its own dimension"
    );
    assert_eq!(entry.record().position(), Some([1.5, 70.25, -2.5]));
    assert_eq!(entry.record().yaw(), Some(f64::from(0.25f32)));
    assert_eq!(
        entry.record().pitch(),
        Some(f64::from(-0.5f32)),
        "the companion pose carries the vertical-range pitch"
    );
    assert_eq!(
        entry.record().velocity(),
        None,
        "the companion publication carries no velocity"
    );
    assert_eq!(
        companion_detail(entry),
        (Some("阿木".to_owned()), None),
        "spawn detail is the canonical name alone"
    );
    assert_eq!(entry.record().header().operation(), FamilyOperation::Upsert);
    assert_eq!(entry.record().header().source_tick(), Some(10));
    assert_eq!(entry.record().header().epoch().get(), EPOCH);
    assert_eq!(
        entry.record().header().revision(),
        view.frame_revision(),
        "headers are rebased onto the coherent candidate revision"
    );
    assert_stable_key(entry, id);
    assert_confirmed_order(entry, 1, 0);
}

/// `state`: one upsert per batch record in published packet order, each with
/// its own actual dimension, the pose with pitch, the reset bit — and no
/// name — while equal source ticks never collapse the packet record order.
#[test]
fn states_batch_keeps_packet_order_pose_and_reset() {
    let mut provider = admitted_mirror();
    let first = companion(0x10);
    let second = companion(0x20);
    commit(
        &mut provider,
        spawn_event(first, "阿木", 10, [1.0, 70.0, 1.0], 0.0, 0.0),
    );
    commit(
        &mut provider,
        spawn_event(second, "Bram", 10, [2.0, 70.0, 2.0], 1.0, 0.125),
    );
    commit(
        &mut provider,
        states_event(
            42,
            vec![
                state_record(first, [1.5, 71.0, 1.5], 0.5, -0.25, false),
                state_record(second, [2.5, 71.0, 2.5], 1.25, 0.5, true),
            ],
        ),
    );

    let fixture = view_fixture();
    let records = project_companion(&fixture.view_of(&provider)).expect("state batch projects");
    assert_eq!(records.len(), 4, "two spawns then two state records");

    let expected = [
        (
            first,
            Some([1.5, 71.0, 1.5]),
            0.5f32,
            -0.25f32,
            Some(false),
            3u64,
            0u32,
        ),
        (second, Some([2.5, 71.0, 2.5]), 1.25, 0.5, Some(true), 3, 1),
    ];
    for (entry, (id, position, yaw, pitch, reset, revision, ordinal)) in
        records.iter().skip(2).zip(&expected)
    {
        assert_eq!(*entry.record().id(), ActorId::Companion(*id));
        assert_eq!(
            *entry.record().dimension(),
            ActorDimension::Known(Dimension::OVERWORLD),
            "the state record names the dimension the mirror confirmed"
        );
        assert_eq!(entry.record().position(), *position);
        assert_eq!(entry.record().yaw(), Some(f64::from(*yaw)));
        assert_eq!(entry.record().pitch(), Some(f64::from(*pitch)));
        assert_eq!(
            companion_detail(entry),
            (None, *reset),
            "state detail is the reset bit alone"
        );
        assert_eq!(entry.record().velocity(), None);
        assert_eq!(entry.record().header().operation(), FamilyOperation::Upsert);
        assert_eq!(entry.record().header().source_tick(), Some(42));
        assert_stable_key(entry, *id);
        assert_confirmed_order(entry, *revision, *ordinal);
    }
    assert!(
        records[2].order() < records[3].order(),
        "equal source ticks keep the packet record ordinal order"
    );
}

/// `remove`: one remove-only record with the dimension resolved from the
/// confirmed identity — no pose, no angle, no velocity and no detail survive
/// the identity, and the despawn publication carries no source tick, so none
/// is invented.
#[test]
fn despawn_is_remove_only_with_resolved_dimension() {
    let mut provider = admitted_mirror();
    let id = companion(0x30);
    commit(
        &mut provider,
        spawn_event(id, "阿木", 100, [1.0, 70.0, 1.0], 0.0, 0.0),
    );
    commit(&mut provider, despawn_event(id));

    let fixture = view_fixture();
    let records = project_companion(&fixture.view_of(&provider)).expect("despawn projects");
    assert_eq!(records.len(), 2);

    let remove = &records[1];
    assert_eq!(
        remove.record().header().operation(),
        FamilyOperation::Remove
    );
    assert_eq!(*remove.record().id(), ActorId::Companion(id));
    assert_eq!(
        *remove.record().dimension(),
        ActorDimension::Known(Dimension::OVERWORLD),
        "the dimensionless despawn resolves through the mirror's identity"
    );
    assert_eq!(remove.record().position(), None);
    assert_eq!(remove.record().yaw(), None);
    assert_eq!(remove.record().pitch(), None);
    assert_eq!(remove.record().velocity(), None);
    assert_eq!(
        companion_detail(remove),
        (None, None),
        "a removal publishes no name and no reset bit"
    );
    assert_eq!(
        remove.record().header().source_tick(),
        None,
        "the despawn wire carries no tick and none is invented"
    );
    assert_stable_key(remove, id);
    assert_confirmed_order(remove, 2, 0);
}

/// The full lifecycle table: spawn, reset-marked state, remove, then reuse of
/// the same typed identity after its removal, in actual source order with one
/// shared stable identity key and remove before reuse.
#[test]
fn spawn_state_remove_reuse_lifecycle() {
    let mut provider = admitted_mirror();
    let id = companion(0x40);
    commit(
        &mut provider,
        spawn_event(id, "阿木", 10, [4.0, 64.0, 4.0], 0.0, 0.0),
    );
    commit(
        &mut provider,
        states_event(
            11,
            vec![state_record(id, [4.5, 64.5, 4.5], 0.75, -0.125, true)],
        ),
    );
    commit(&mut provider, despawn_event(id));
    commit(
        &mut provider,
        spawn_event(id, "阿木Again", 13, [5.0, 64.0, 5.0], 0.0, 0.0),
    );

    let fixture = view_fixture();
    let records = project_companion(&fixture.view_of(&provider)).expect("lifecycle projects");
    assert_eq!(records.len(), 4);

    let expected = [
        (
            FamilyOperation::Upsert,
            Some(10u64),
            1u64,
            0u32,
            Some("阿木".to_owned()),
            None,
        ),
        (FamilyOperation::Upsert, Some(11), 2, 0, None, Some(true)),
        (FamilyOperation::Remove, None, 3, 0, None, None),
        (
            FamilyOperation::Upsert,
            Some(13),
            4,
            0,
            Some("阿木Again".to_owned()),
            None,
        ),
    ];
    for (entry, (operation, tick, revision, ordinal, name, reset)) in records.iter().zip(&expected)
    {
        assert_eq!(entry.record().header().operation(), *operation);
        assert_eq!(entry.record().header().source_tick(), *tick);
        assert_eq!(
            companion_detail(entry),
            (name.clone(), *reset),
            "each packet fills exactly its own fields"
        );
        assert_eq!(entry.record().velocity(), None);
        assert_stable_key(entry, id);
        assert_confirmed_order(entry, *revision, *ordinal);
    }

    // The remove record is a removal alone.
    let remove = &records[2];
    assert_eq!(remove.record().position(), None);
    assert_eq!(remove.record().yaw(), None);
    assert_eq!(remove.record().pitch(), None);
    assert!(remove.record().detail().is_none());

    assert!(
        records
            .windows(2)
            .all(|pair| pair[0].order() < pair[1].order()),
        "records keep actual source order across absent and present ticks"
    );
}

/// No task and no death fields: chat observations that address the same
/// companion identity and carry task lifecycle facts, and passive
/// observations that carry health and a died reason, produce no companion
/// records and no companion record ever carries a foreign payload.
#[test]
fn task_and_death_shaped_observations_never_become_companion_fields() {
    let mut provider = admitted_mirror();
    let id = companion(0x50);
    commit(
        &mut provider,
        spawn_event(id, "阿木", 10, [6.0, 64.0, 6.0], 0.0, 0.0),
    );
    for event in task_shaped_chat_events(id) {
        commit(&mut provider, event);
    }
    for event in passive_died_events(20) {
        commit(&mut provider, event);
    }

    let fixture = view_fixture();
    let records =
        project_companion(&fixture.view_of(&provider)).expect("foreign kinds are ignorable");
    assert_eq!(records.len(), 1, "only the companion observations project");
    for entry in &records {
        assert_eq!(entry.record().kind(), ActorKind::Companion);
        assert_eq!(*entry.record().id(), ActorId::Companion(id));
        assert_eq!(
            companion_detail(entry),
            (Some("阿木".to_owned()), None),
            "task-shaped and death-shaped source data never becomes a companion field"
        );
        assert_eq!(
            entry.record().velocity(),
            None,
            "the spawn carries no velocity"
        );
    }
}

/// Malformed prevalidation doubles reject the whole projection with no
/// partial output: a despawn without its resolved identity, a despawn whose
/// resolution names another identity, a despawn resolved live in two
/// dimensions at once, a packet that is not a checked publication, and an
/// old-epoch observation key.
#[test]
fn invalid_observations_reject_without_partial_output() {
    let epoch = SessionEpoch::try_new(EPOCH).expect("epoch");
    let fixture = view_fixture();
    let provider = admitted_mirror();
    let id = companion(0x60);
    let other = companion(0x61);
    let despawn = packet(despawn_event(id));

    let missing = AcceptedObservation::try_new(
        ObservationKey::try_new(epoch, ConfirmedRevision::new(1), 0).expect("key"),
        None,
        despawn.clone(),
        Vec::new(),
    )
    .expect("staged without resolution");
    assert_eq!(
        project_companion(&fixture.view(
            provider.mirror(),
            &[missing],
            epoch,
            ConfirmedRevision::new(1)
        )),
        Err(ClientError::InvalidInput),
        "a despawn with no resolved identity rejects"
    );

    let mismatched = AcceptedObservation::try_new(
        ObservationKey::try_new(epoch, ConfirmedRevision::new(1), 0).expect("key"),
        None,
        despawn.clone(),
        vec![
            ResolvedActor::try_new(
                ActorKind::Companion,
                ActorId::Companion(other),
                Dimension::OVERWORLD,
            )
            .expect("resolved other identity"),
        ],
    )
    .expect("staged with foreign resolution");
    assert_eq!(
        project_companion(&fixture.view(
            provider.mirror(),
            &[mismatched],
            epoch,
            ConfirmedRevision::new(1)
        )),
        Err(ClientError::InvalidInput),
        "a resolution naming another identity rejects"
    );

    let ambiguous = AcceptedObservation::try_new(
        ObservationKey::try_new(epoch, ConfirmedRevision::new(1), 0).expect("key"),
        None,
        despawn,
        vec![
            ResolvedActor::try_new(
                ActorKind::Companion,
                ActorId::Companion(id),
                Dimension::OVERWORLD,
            )
            .expect("resolved overworld identity"),
            ResolvedActor::try_new(
                ActorKind::Companion,
                ActorId::Companion(id),
                Dimension::OVERWORLD,
            )
            .expect("resolved second identity"),
        ],
    )
    .expect("staged with ambiguous resolution");
    assert_eq!(
        project_companion(&fixture.view(
            provider.mirror(),
            &[ambiguous],
            epoch,
            ConfirmedRevision::new(1)
        )),
        Err(ClientError::InvalidInput),
        "the same identity resolved twice at once rejects"
    );

    let non_publication = AcceptedObservation::try_new(
        ObservationKey::try_new(epoch, ConfirmedRevision::new(1), 0).expect("key"),
        None,
        ServerPacket::KeepAlive(KeepAlive::new(1).expect("checked keep-alive token")),
        Vec::new(),
    )
    .expect("staged with a non-publication packet");
    assert_eq!(
        project_companion(&fixture.view(
            provider.mirror(),
            &[non_publication],
            epoch,
            ConfirmedRevision::new(1)
        )),
        Err(ClientError::InvalidInput),
        "a packet that is not an event publication rejects without a record"
    );

    let stale_epoch = AcceptedObservation::try_new(
        ObservationKey::try_new(
            SessionEpoch::try_new(EPOCH + 1).expect("next epoch"),
            ConfirmedRevision::new(1),
            0,
        )
        .expect("key"),
        Some(10),
        packet(spawn_event(id, "阿木", 10, [0.0, 64.0, 0.0], 0.0, 0.0)),
        Vec::new(),
    )
    .expect("staged in another epoch");
    assert_eq!(
        project_companion(&fixture.view(
            provider.mirror(),
            &[stale_epoch],
            epoch,
            ConfirmedRevision::new(1)
        )),
        Err(ClientError::StaleEpoch),
        "an observation from another epoch rejects without output"
    );

    // The untouched baseline still projects exactly its committed records:
    // the rejected doubles changed no owner state.
    assert!(
        project_companion(&fixture.view_of(&provider))
            .expect("empty queue projects")
            .is_empty(),
        "an empty committed queue projects no records"
    );
}

/// A stale-ticked state batch is published in observation order, never
/// re-sorted by its older server tick: the record keeps its own tick and its
/// position after the newer batch, so backwards ticks never reconstruct
/// source order.
#[test]
fn backwards_tick_keeps_observation_order_not_tick_order() {
    let mut provider = admitted_mirror();
    let id = companion(0x70);
    commit(
        &mut provider,
        spawn_event(id, "阿木", 100, [1.0, 70.0, 1.0], 0.0, 0.0),
    );
    commit(
        &mut provider,
        states_event(
            101,
            vec![state_record(id, [1.5, 70.0, 1.5], 0.5, 0.0, false)],
        ),
    );
    commit(
        &mut provider,
        states_event(90, vec![state_record(id, [2.0, 70.0, 2.0], 0.5, 0.0, true)]),
    );

    let fixture = view_fixture();
    let records = project_companion(&fixture.view_of(&provider)).expect("stale batch projects");
    assert_eq!(records.len(), 3);

    let late = &records[2];
    assert_eq!(
        companion_detail(late),
        (None, Some(true)),
        "the stale-ticked record publishes its own reset value"
    );
    assert_eq!(late.record().header().source_tick(), Some(90));
    assert_eq!(
        late.record().position(),
        Some([2.0, 70.0, 2.0]),
        "the late record's own body is preserved, not the newer mirror value"
    );
    assert_confirmed_order(late, 3, 0);
    assert!(
        records[1].order() < late.order(),
        "an older server tick never reorders a later observation"
    );
    assert!(
        records
            .windows(2)
            .all(|pair| pair[0].order() < pair[1].order()),
        "order follows the observation keys alone"
    );
}

/// Late and duplicate inputs are refused by the accepted mirror schema
/// before this projection ever sees them: a duplicate spawn while the
/// identity is live, a late state for a removed identity and a duplicate
/// despawn all leave the committed revision, the observation queue and the
/// projected output unchanged, while a later reuse of the removed identity
/// still commits.
#[test]
fn late_or_duplicate_inputs_leave_committed_state_unchanged() {
    let mut provider = admitted_mirror();
    let id = companion(0x80);
    commit(
        &mut provider,
        spawn_event(id, "阿木", 10, [7.0, 64.0, 7.0], 0.0, 0.0),
    );
    commit(
        &mut provider,
        states_event(
            11,
            vec![state_record(id, [7.5, 64.5, 7.5], 0.25, 0.0, true)],
        ),
    );

    let fixture = view_fixture();
    let staged = |provider: &MirrorProvider, event: Event| {
        let next = provider.mirror().revision().get() + 1;
        AcceptedObservation::try_new(
            ObservationKey::try_new(
                SessionEpoch::try_new(EPOCH).expect("epoch"),
                ConfirmedRevision::new(next),
                0,
            )
            .expect("key"),
            None,
            packet(event),
            Vec::new(),
        )
        .expect("staged")
    };
    assert!(
        provider
            .commit(&staged(
                &provider,
                spawn_event(id, "阿木", 12, [9.0, 9.0, 9.0], 0.0, 0.0),
            ))
            .is_err(),
        "a spawn naming a live identity is a duplicate"
    );

    commit(&mut provider, despawn_event(id));
    let baseline = project_companion(&fixture.view_of(&provider)).expect("baseline projects");
    assert_eq!(baseline.len(), 3);
    assert_eq!(
        baseline
            .last()
            .expect("remove record")
            .record()
            .header()
            .operation(),
        FamilyOperation::Remove
    );

    assert!(
        provider
            .commit(&staged(
                &provider,
                states_event(
                    99,
                    vec![state_record(id, [9.0, 64.0, 9.0], 0.0, 0.0, false)]
                ),
            ))
            .is_err(),
        "a late state cannot resurrect a despawn"
    );
    assert!(
        provider
            .commit(&staged(&provider, despawn_event(id)))
            .is_err(),
        "a duplicate despawn is an orphan removal"
    );
    assert_eq!(provider.mirror().revision().get(), 3);
    assert_eq!(provider.observations().len(), 3);
    assert!(provider.mirror().actors().companions().is_empty());
    assert_eq!(
        project_companion(&fixture.view_of(&provider)).expect("projection still succeeds"),
        baseline,
        "the refused inputs leave the projection unchanged"
    );

    // Removal precedes reuse: the same identity spawns again after its
    // despawn and the projection gains exactly the new upsert.
    commit(
        &mut provider,
        spawn_event(id, "阿木", 14, [8.0, 64.0, 8.0], 0.0, 0.0),
    );
    let reused = project_companion(&fixture.view_of(&provider)).expect("reuse projects");
    assert_eq!(reused.len(), 4);
    assert_eq!(
        reused
            .last()
            .expect("respawn record")
            .record()
            .header()
            .operation(),
        FamilyOperation::Upsert
    );
}

/// Invalid companion values are refused at the checked domain boundary
/// before any observation exists: a non-UUIDv4 identity, a foreign dimension,
/// a pitch outside the inclusive vertical range and a name with embedded
/// whitespace never become a companion observation, so the projection never
/// sees one.
#[test]
fn invalid_values_are_refused_at_the_checked_domain_boundary() {
    assert!(
        CompanionId::try_from_bytes([0u8; 16]).is_err(),
        "a zero identity never becomes a checked companion"
    );
    assert!(
        CompanionSpawn::try_new(CompanionSpawnParts {
            id: companion(0x11),
            name: companion_name("阿木"),
            server_tick: 10,
            dimension: Dimension::DEPTHS,
            position: finite_pos([1.0, 64.0, 1.0]),
            look: finite_look(0.0, 0.0),
        })
        .is_err(),
        "a companion lives in the overworld alone"
    );
    assert!(
        CompanionState::try_new(CompanionStateParts {
            id: companion(0x11),
            dimension: Dimension::OVERWORLD,
            position: finite_pos([1.0, 64.0, 1.0]),
            look: finite_look(0.0, 2.0),
            reset: false,
        })
        .is_err(),
        "a pitch outside the inclusive vertical range never becomes a state record"
    );
    assert!(
        CompanionName::try_from_canonical("embedded space".to_owned()).is_err(),
        "a name with embedded whitespace never becomes a canonical companion name"
    );
    assert!(
        FiniteVec3::try_new([f32::NAN, 64.0, 0.0]).is_err(),
        "a non-finite position never becomes a checked body"
    );
}
