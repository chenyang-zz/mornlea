//! Remote-player actor projection contract tests.
//!
//! The table pins the accepted remote-player semantics against the landed
//! mirror provider: spawn/state/remove/reuse over real committed
//! observations, the typed UUID and the actual dimension as part of the
//! record identity, no velocity field on any record, and unchanged state for
//! late, duplicate, malformed or old-epoch inputs. Order keys are the actual
//! observation keys with packet record ordinals; equal or absent source
//! ticks never reconstruct order.

use mornlea_client_core::contracts::{
    ClientError, ClientLimits, ConfirmedRevision, FamilyOperation, ObservationKey, SessionEpoch,
};
use mornlea_client_core::input::InputProjectionState;
use mornlea_client_core::prediction::PlayerProjectionState;
use mornlea_client_core::presentation::actors::remote_player::project_remote_player;
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
    Dimension, DisplayName, Event, FiniteVec3, HostileId, HostileKind, HostileSpawn,
    HostileSpawnParts, HostileSpawnRecord, HostileSpawnRecordParts, HostileState,
    HostileStateParts, HostileStateRecord, HostileStateRecordParts, LookAngles, PlayerId,
    RemotePlayerDespawn, RemotePlayerSpawn, RemotePlayerSpawnParts, RemotePlayerState,
    RemotePlayerStateParts, RemotePlayerStates, RemotePlayerStatesParts,
};
use mornlea_protocol::{KeepAlive, ServerPacket};

/// The epoch every fixture mirror and observation key carries.
const EPOCH: u64 = 7;

/// A checked UUIDv4 identity whose raw byte order follows `first`, so two
/// distinct seeds are strictly ordered the way a state batch requires.
fn player_id(first: u8) -> PlayerId {
    let mut bytes = [0u8; 16];
    bytes[0] = first;
    bytes[6] = 0x40;
    bytes[8] = 0x80;
    PlayerId::try_from_bytes(bytes).expect("checked uuid v4 identity")
}

fn display_name(text: &str) -> DisplayName {
    DisplayName::try_from_canonical(text.to_owned()).expect("canonical display name")
}

fn finite_pos(components: [f32; 3]) -> FiniteVec3 {
    FiniteVec3::try_new(components).expect("finite position")
}

fn finite_look(yaw: f32, pitch: f32) -> LookAngles {
    LookAngles::try_new(yaw, pitch).expect("finite look angles")
}

fn spawn_event(
    id: PlayerId,
    name: &str,
    tick: u64,
    dimension: Dimension,
    position: [f32; 3],
    yaw: f32,
    pitch: f32,
) -> Event {
    Event::RemotePlayerSpawn(RemotePlayerSpawn::new(RemotePlayerSpawnParts {
        player_id: id,
        display_name: display_name(name),
        server_tick: tick,
        dimension,
        position: finite_pos(position),
        look: finite_look(yaw, pitch),
    }))
}

fn state_record(
    id: PlayerId,
    dimension: Dimension,
    position: [f32; 3],
    yaw: f32,
    pitch: f32,
    reset: bool,
) -> RemotePlayerState {
    RemotePlayerState::new(RemotePlayerStateParts {
        player_id: id,
        dimension,
        position: finite_pos(position),
        look: finite_look(yaw, pitch),
        reset,
    })
}

fn states_event(tick: u64, records: Vec<RemotePlayerState>) -> Event {
    Event::RemotePlayerStates(
        RemotePlayerStates::try_new(RemotePlayerStatesParts {
            server_tick: tick,
            states: records.into_boxed_slice(),
        })
        .expect("checked remote-player state batch"),
    )
}

fn despawn_event(id: PlayerId) -> Event {
    Event::RemotePlayerDespawn(RemotePlayerDespawn::new(id))
}

/// A hostile state observation is the velocity-carrying foreign kind: the
/// projection must ignore it and never map its velocity anywhere.
fn hostile_velocity_events(tick: u64, id: HostileId) -> Vec<Event> {
    let spawn = Event::HostileSpawn(
        HostileSpawn::try_new(HostileSpawnParts {
            server_tick: tick,
            spawns: vec![
                HostileSpawnRecord::try_new(HostileSpawnRecordParts {
                    id,
                    dimension: Dimension::OVERWORLD,
                    position: finite_pos([3.0, 65.0, 3.0]),
                    yaw: 0.5,
                    health: 20,
                    kind: HostileKind::Nightwalker,
                })
                .expect("checked hostile spawn record"),
            ]
            .into_boxed_slice(),
        })
        .expect("checked hostile spawn batch"),
    );
    let state = Event::HostileState(
        HostileState::try_new(HostileStateParts {
            server_tick: tick + 1,
            states: vec![
                HostileStateRecord::try_new(HostileStateRecordParts {
                    id,
                    position: finite_pos([3.5, 65.5, 3.5]),
                    velocity: finite_pos([1.0, 0.0, 0.0]),
                    yaw: 0.6,
                    health: 18,
                    kind: HostileKind::Nightwalker,
                })
                .expect("checked hostile state record"),
            ]
            .into_boxed_slice(),
        })
        .expect("checked hostile state batch"),
    );
    vec![spawn, state]
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
        other => panic!("remote-player record is server-sourced: {other:?}"),
    }
}

/// The remote-player detail payload, refusing a foreign detail tag: a
/// remove-only record carries no detail at all.
fn remote_detail(entry: &OrderedRecord<ActorRecord>) -> (Option<String>, Option<bool>) {
    match entry.record().detail() {
        None => (None, None),
        Some(ActorDetail::RemotePlayer {
            display_name,
            reset,
        }) => (
            display_name.as_ref().map(|name| name.as_str().to_owned()),
            *reset,
        ),
        Some(other) => panic!("remote-player record carries a foreign detail tag: {other:?}"),
    }
}

/// `spawn`: one upsert with the typed UUID identity, the actual spawn
/// dimension, the exact finite pose widened from f32, the display-name detail
/// and no velocity.
#[test]
fn spawn_projects_typed_identity_name_and_actual_dimension() {
    let mut provider = admitted_mirror();
    let id = player_id(0x11);
    commit(
        &mut provider,
        spawn_event(
            id,
            "Aria",
            10,
            Dimension::OVERWORLD,
            [1.5, 70.25, -2.5],
            0.25,
            -0.5,
        ),
    );

    let fixture = view_fixture();
    let view = fixture.view_of(&provider);
    let records = project_remote_player(&view).expect("spawn projects one record");
    assert_eq!(records.len(), 1);

    let entry = &records[0];
    assert_eq!(entry.record().kind(), ActorKind::RemotePlayer);
    assert_eq!(*entry.record().id(), ActorId::RemotePlayer(id));
    assert_eq!(
        *entry.record().dimension(),
        ActorDimension::Known(Dimension::OVERWORLD)
    );
    assert_eq!(entry.record().position(), Some([1.5, 70.25, -2.5]));
    assert_eq!(entry.record().yaw(), Some(f64::from(0.25f32)));
    assert_eq!(entry.record().pitch(), Some(f64::from(-0.5f32)));
    assert_eq!(entry.record().velocity(), None, "no remote-player velocity");
    assert_eq!(
        remote_detail(entry),
        (Some("Aria".to_owned()), None),
        "spawn detail is the display name alone"
    );
    assert_eq!(entry.record().header().operation(), FamilyOperation::Upsert);
    assert_eq!(entry.record().header().source_tick(), Some(10));
    assert_eq!(entry.record().header().epoch().get(), EPOCH);
    assert_eq!(
        entry.record().header().revision(),
        view.frame_revision(),
        "headers are rebased onto the coherent candidate revision"
    );
    assert_eq!(
        *entry.stable_key(),
        StableRecordKey::Actor {
            kind: ActorKind::RemotePlayer,
            dimension: ActorDimension::Known(Dimension::OVERWORLD),
            id: ActorId::RemotePlayer(id),
        }
    );
    assert_confirmed_order(entry, 1, 0);
}

/// `state`: one record per batch entry in published packet order, each with
/// its own actual dimension, the reset detail and no display name, and equal
/// source ticks never collapse the packet record order.
#[test]
fn states_batch_keeps_packet_order_and_per_record_dimension() {
    let mut provider = admitted_mirror();
    let overworld_player = player_id(0x10);
    let depths_player = player_id(0x20);
    commit(
        &mut provider,
        spawn_event(
            overworld_player,
            "Aria",
            10,
            Dimension::OVERWORLD,
            [1.0, 70.0, 1.0],
            0.0,
            0.0,
        ),
    );
    commit(
        &mut provider,
        spawn_event(
            depths_player,
            "Bram",
            10,
            Dimension::DEPTHS,
            [2.0, 40.0, 2.0],
            1.0,
            0.125,
        ),
    );
    commit(
        &mut provider,
        states_event(
            42,
            vec![
                state_record(
                    overworld_player,
                    Dimension::OVERWORLD,
                    [1.5, 71.0, 1.5],
                    0.5,
                    -0.25,
                    false,
                ),
                state_record(
                    depths_player,
                    Dimension::DEPTHS,
                    [2.5, 41.0, 2.5],
                    1.25,
                    0.5,
                    true,
                ),
            ],
        ),
    );

    let fixture = view_fixture();
    let records = project_remote_player(&fixture.view_of(&provider)).expect("state batch projects");
    assert_eq!(records.len(), 4, "two spawns then two state records");

    let expected = [
        (
            overworld_player,
            Dimension::OVERWORLD,
            Some([1.5, 71.0, 1.5]),
            Some(false),
            3u64,
            0u32,
        ),
        (
            depths_player,
            Dimension::DEPTHS,
            Some([2.5, 41.0, 2.5]),
            Some(true),
            3,
            1,
        ),
    ];
    for (entry, (id, dimension, position, reset, revision, ordinal)) in
        records.iter().skip(2).zip(&expected)
    {
        assert_eq!(*entry.record().id(), ActorId::RemotePlayer(*id));
        assert_eq!(
            *entry.record().dimension(),
            ActorDimension::Known(*dimension)
        );
        assert_eq!(entry.record().position(), *position);
        assert_eq!(
            remote_detail(entry),
            (None, *reset),
            "state detail is the reset bit alone"
        );
        assert_eq!(entry.record().velocity(), None);
        assert_eq!(entry.record().header().operation(), FamilyOperation::Upsert);
        assert_eq!(entry.record().header().source_tick(), Some(42));
        assert_confirmed_order(entry, *revision, *ordinal);
    }
    assert!(
        records[2].order() < records[3].order(),
        "equal source ticks keep the packet record ordinal order"
    );
}

/// The full lifecycle table: spawn, state, remove, then reuse of the same
/// typed identity after its removal, in actual source order with one shared
/// stable identity key.
#[test]
fn spawn_state_remove_reuse_lifecycle() {
    let mut provider = admitted_mirror();
    let id = player_id(0x30);
    commit(
        &mut provider,
        spawn_event(
            id,
            "Bram",
            10,
            Dimension::OVERWORLD,
            [4.0, 64.0, 4.0],
            0.0,
            0.0,
        ),
    );
    commit(
        &mut provider,
        states_event(
            11,
            vec![state_record(
                id,
                Dimension::OVERWORLD,
                [4.5, 64.5, 4.5],
                0.75,
                0.0,
                false,
            )],
        ),
    );
    commit(&mut provider, despawn_event(id));
    commit(
        &mut provider,
        spawn_event(
            id,
            "BramAgain",
            12,
            Dimension::OVERWORLD,
            [5.0, 64.0, 5.0],
            0.0,
            0.0,
        ),
    );

    let fixture = view_fixture();
    let records = project_remote_player(&fixture.view_of(&provider)).expect("lifecycle projects");
    assert_eq!(records.len(), 4);

    let expected = [
        (
            FamilyOperation::Upsert,
            Some(10u64),
            1u64,
            0u32,
            Some("Bram".to_owned()),
        ),
        (FamilyOperation::Upsert, Some(11), 2, 0, None),
        (FamilyOperation::Remove, None, 3, 0, None),
        (
            FamilyOperation::Upsert,
            Some(12),
            4,
            0,
            Some("BramAgain".to_owned()),
        ),
    ];
    let stable = StableRecordKey::Actor {
        kind: ActorKind::RemotePlayer,
        dimension: ActorDimension::Known(Dimension::OVERWORLD),
        id: ActorId::RemotePlayer(id),
    };
    for (entry, (operation, tick, revision, ordinal, name)) in records.iter().zip(&expected) {
        assert_eq!(entry.record().header().operation(), *operation);
        assert_eq!(entry.record().header().source_tick(), *tick);
        assert_eq!(
            *entry.stable_key(),
            stable,
            "reuse keeps the typed identity"
        );
        assert_confirmed_order(entry, *revision, *ordinal);
        let (detail_name, _) = remote_detail(entry);
        assert_eq!(detail_name, name.clone());
        assert_eq!(entry.record().velocity(), None);
    }

    // The remove record is a removal alone: no pose, no angle, no velocity
    // and no detail, with the dimension resolved from the confirmed identity.
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

/// Foreign actor kinds are not this projection's input: hostile observations
/// carrying velocity-shaped data produce no remote-player records and no
/// remote-player record ever carries a velocity.
#[test]
fn foreign_velocity_kinds_are_ignored() {
    let mut provider = admitted_mirror();
    let id = player_id(0x40);
    commit(
        &mut provider,
        spawn_event(
            id,
            "Aria",
            10,
            Dimension::OVERWORLD,
            [6.0, 64.0, 6.0],
            0.0,
            0.0,
        ),
    );
    for event in hostile_velocity_events(20, HostileId::try_new(9).expect("hostile identity")) {
        commit(&mut provider, event);
    }
    commit(
        &mut provider,
        states_event(
            30,
            vec![state_record(
                id,
                Dimension::OVERWORLD,
                [6.5, 64.5, 6.5],
                0.0,
                0.0,
                false,
            )],
        ),
    );

    let fixture = view_fixture();
    let records =
        project_remote_player(&fixture.view_of(&provider)).expect("foreign kinds are ignorable");
    assert_eq!(
        records.len(),
        2,
        "only the remote-player observations project"
    );
    for entry in &records {
        assert_eq!(entry.record().kind(), ActorKind::RemotePlayer);
        assert_eq!(*entry.record().id(), ActorId::RemotePlayer(id));
        assert_eq!(
            entry.record().velocity(),
            None,
            "velocity-shaped foreign data never becomes a remote-player velocity"
        );
    }
}

/// Malformed prevalidation doubles reject the whole projection with no
/// partial output: a despawn without its resolved identity, a despawn whose
/// resolution names another identity, a despawn resolved live in two
/// dimensions at once, a packet that is not an event publication, and an
/// old-epoch observation key.
#[test]
fn invalid_observations_reject_without_partial_output() {
    let epoch = SessionEpoch::try_new(EPOCH).expect("epoch");
    let fixture = view_fixture();
    let provider = admitted_mirror();
    let id = player_id(0x50);
    let other = player_id(0x60);
    let despawn = packet(despawn_event(id));

    let missing = AcceptedObservation::try_new(
        ObservationKey::try_new(epoch, ConfirmedRevision::new(1), 0).expect("key"),
        None,
        despawn.clone(),
        Vec::new(),
    )
    .expect("staged without resolution");
    assert_eq!(
        project_remote_player(&fixture.view(
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
                ActorKind::RemotePlayer,
                ActorId::RemotePlayer(other),
                Dimension::DEPTHS,
            )
            .expect("resolved other identity"),
        ],
    )
    .expect("staged with foreign resolution");
    assert_eq!(
        project_remote_player(&fixture.view(
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
                ActorKind::RemotePlayer,
                ActorId::RemotePlayer(id),
                Dimension::OVERWORLD,
            )
            .expect("resolved overworld identity"),
            ResolvedActor::try_new(
                ActorKind::RemotePlayer,
                ActorId::RemotePlayer(id),
                Dimension::DEPTHS,
            )
            .expect("resolved depths identity"),
        ],
    )
    .expect("staged with ambiguous resolution");
    assert_eq!(
        project_remote_player(&fixture.view(
            provider.mirror(),
            &[ambiguous],
            epoch,
            ConfirmedRevision::new(1)
        )),
        Err(ClientError::InvalidInput),
        "the same identity resolved live in two dimensions at once rejects"
    );

    let non_publication = AcceptedObservation::try_new(
        ObservationKey::try_new(epoch, ConfirmedRevision::new(1), 0).expect("key"),
        None,
        ServerPacket::KeepAlive(KeepAlive::new(1).expect("checked keep-alive token")),
        Vec::new(),
    )
    .expect("staged with a non-publication packet");
    assert_eq!(
        project_remote_player(&fixture.view(
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
        None,
        packet(spawn_event(
            id,
            "Aria",
            10,
            Dimension::OVERWORLD,
            [0.0, 64.0, 0.0],
            0.0,
            0.0,
        )),
        Vec::new(),
    )
    .expect("staged in another epoch");
    assert_eq!(
        project_remote_player(&fixture.view(
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
        project_remote_player(&fixture.view_of(&provider))
            .expect("empty queue projects")
            .is_empty(),
        "an empty committed queue projects no records"
    );
}

/// Late and duplicate inputs are refused by the accepted mirror schema
/// before this projection ever sees them: the committed revision, the
/// observation queue and the projected output all stay unchanged, while a
/// later reuse of the removed identity still commits.
#[test]
fn late_or_duplicate_inputs_leave_committed_state_unchanged() {
    let mut provider = admitted_mirror();
    let id = player_id(0x70);
    commit(
        &mut provider,
        spawn_event(
            id,
            "Aria",
            10,
            Dimension::OVERWORLD,
            [7.0, 64.0, 7.0],
            0.0,
            0.0,
        ),
    );
    commit(
        &mut provider,
        states_event(
            11,
            vec![state_record(
                id,
                Dimension::OVERWORLD,
                [7.5, 64.5, 7.5],
                0.0,
                0.0,
                false,
            )],
        ),
    );
    commit(&mut provider, despawn_event(id));

    let fixture = view_fixture();
    let baseline = project_remote_player(&fixture.view_of(&provider)).expect("baseline projects");
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

    let late_state = states_event(
        99,
        vec![state_record(
            id,
            Dimension::OVERWORLD,
            [9.0, 64.0, 9.0],
            0.0,
            0.0,
            false,
        )],
    );
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
        provider.commit(&staged(&provider, late_state)).is_err(),
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
    assert!(provider.mirror().actors().remote_players().is_empty());
    assert_eq!(
        project_remote_player(&fixture.view_of(&provider)).expect("projection still succeeds"),
        baseline,
        "the refused inputs leave the projection unchanged"
    );

    // Removal precedes reuse: the same identity spawns again after its
    // despawn and the projection gains exactly the new upsert.
    commit(
        &mut provider,
        spawn_event(
            id,
            "Aria",
            12,
            Dimension::OVERWORLD,
            [8.0, 64.0, 8.0],
            0.0,
            0.0,
        ),
    );
    let reused = project_remote_player(&fixture.view_of(&provider)).expect("reuse projects");
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
