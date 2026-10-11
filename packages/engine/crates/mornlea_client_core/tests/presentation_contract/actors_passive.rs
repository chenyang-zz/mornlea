//! Passive-mob actor projection contract tests.
//!
//! The table pins the accepted passive semantics against the landed mirror
//! provider: spawn/state/despawn over real committed observations, the typed
//! nonzero identity and the resolved dimension as part of the record
//! identity, health and the grazing bit on the records whose packets carry
//! them, the exact vanished/died despawn reason from the closed wire union,
//! no lure or drop inference from foreign-shaped data, and unchanged state
//! for late, duplicate, malformed or old-epoch inputs. Order keys are the
//! actual observation keys with packet record ordinals; equal, absent or
//! backwards source ticks never reconstruct order.

use mornlea_client_core::contracts::{
    ClientError, ClientLimits, ConfirmedRevision, FamilyOperation, ObservationKey, SessionEpoch,
};
use mornlea_client_core::input::InputProjectionState;
use mornlea_client_core::prediction::PlayerProjectionState;
use mornlea_client_core::presentation::actors::passive::project_passive;
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
    ChunkPos, Dimension, DropId, Event, FiniteVec3, HostileId, HostileKind, HostileSpawn,
    HostileSpawnParts, HostileSpawnRecord, HostileSpawnRecordParts, HostileState,
    HostileStateParts, HostileStateRecord, HostileStateRecordParts, ItemDrop, ItemDropParts,
    ItemDropUpserts, ItemDropUpsertsParts, ItemStack, PassiveDespawn, PassiveDespawnParts,
    PassiveDespawnReason, PassiveDespawnRecord, PassiveId, PassiveSpawn, PassiveSpawnParts,
    PassiveSpawnRecord, PassiveSpawnRecordParts, PassiveState, PassiveStateParts,
    PassiveStateRecord, PassiveStateRecordParts,
};
use mornlea_protocol::{KeepAlive, ServerPacket};

/// The epoch every fixture mirror and observation key carries.
const EPOCH: u64 = 7;

/// A checked nonzero passive identity; distinct seeds are strictly ordered
/// the way a batch's ascending identity order requires.
fn passive_id(seed: u64) -> PassiveId {
    PassiveId::try_new(seed).expect("checked passive identity")
}

fn finite_pos(components: [f32; 3]) -> FiniteVec3 {
    FiniteVec3::try_new(components).expect("finite position")
}

/// One passive spawn record; the domain admits the overworld alone, so every
/// fixture body lives there.
fn spawn_record(id: PassiveId, position: [f32; 3], yaw: f32, health: u8) -> PassiveSpawnRecord {
    PassiveSpawnRecord::try_new(PassiveSpawnRecordParts {
        id,
        dimension: Dimension::OVERWORLD,
        position: finite_pos(position),
        yaw,
        health,
    })
    .expect("checked passive spawn record")
}

fn spawn_event(tick: u64, records: Vec<PassiveSpawnRecord>) -> Event {
    Event::PassiveSpawn(
        PassiveSpawn::try_new(PassiveSpawnParts {
            server_tick: tick,
            spawns: records.into_boxed_slice(),
        })
        .expect("checked passive spawn batch"),
    )
}

/// One passive state record: the dimensionless body with the transient
/// grazing observation.
fn state_record(
    id: PassiveId,
    position: [f32; 3],
    velocity: [f32; 3],
    yaw: f32,
    health: u8,
    grazing: bool,
) -> PassiveStateRecord {
    PassiveStateRecord::try_new(PassiveStateRecordParts {
        id,
        position: finite_pos(position),
        velocity: finite_pos(velocity),
        yaw,
        health,
        grazing,
    })
    .expect("checked passive state record")
}

fn state_event(tick: u64, records: Vec<PassiveStateRecord>) -> Event {
    Event::PassiveState(
        PassiveState::try_new(PassiveStateParts {
            server_tick: tick,
            states: records.into_boxed_slice(),
        })
        .expect("checked passive state batch"),
    )
}

fn despawn_event(tick: u64, records: Vec<PassiveDespawnRecord>) -> Event {
    Event::PassiveDespawn(
        PassiveDespawn::try_new(PassiveDespawnParts {
            server_tick: tick,
            despawns: records.into_boxed_slice(),
        })
        .expect("checked passive despawn batch"),
    )
}

/// One hostile spawn/state pair: the foreign velocity-carrying kind whose
/// records must never become passive ones.
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

/// One item-drop upsert observation: the foreign block-index/stack-carrying
/// kind, whose shape must not leak into any passive record.
fn drop_upsert_event(tick: u64) -> Event {
    Event::ItemDropUpserts(
        ItemDropUpserts::try_new(ItemDropUpsertsParts {
            server_tick: tick,
            drops: vec![
                ItemDrop::try_new(ItemDropParts {
                    id: DropId::try_new(0, ChunkPos::new(1, 1), 5, 1).expect("drop identity"),
                    block_index: 4,
                    stack: ItemStack::try_new(1, 3, 0).expect("valid stack"),
                })
                .expect("checked drop record"),
            ]
            .into_boxed_slice(),
        })
        .expect("checked drop upsert batch"),
    )
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
        other => panic!("passive record is server-sourced: {other:?}"),
    }
}

/// The passive detail payload, refusing a foreign detail tag: the closed
/// union has health, grazing and the despawn reason alone, and no other
/// field can appear on a passive record.
fn passive_detail(
    entry: &OrderedRecord<ActorRecord>,
) -> (Option<u8>, Option<u8>, Option<PassiveDespawnReason>) {
    match entry.record().detail() {
        None => (None, None, None),
        Some(ActorDetail::Passive {
            health,
            grazing,
            despawn_reason,
        }) => (*health, *grazing, *despawn_reason),
        Some(other) => panic!("passive record carries a foreign detail tag: {other:?}"),
    }
}

/// The shared stable-key assertion: kind, actual dimension and the typed
/// nonzero identity.
fn assert_stable_key(entry: &OrderedRecord<ActorRecord>, id: PassiveId) {
    assert_eq!(
        *entry.stable_key(),
        StableRecordKey::Actor {
            kind: ActorKind::Passive,
            dimension: ActorDimension::Known(Dimension::OVERWORLD),
            id: ActorId::Passive(id),
        }
    );
}

/// `spawn`: one upsert per batch record in published packet order, each with
/// the typed identity, the actual spawn dimension, the exact finite pose
/// widened from f32, yaw, the spawn health — and no velocity, no pitch, no
/// grazing bit and no despawn reason.
#[test]
fn spawn_projects_health_pose_and_typed_identity() {
    let mut provider = admitted_mirror();
    let first = passive_id(3);
    let second = passive_id(9);
    commit(
        &mut provider,
        spawn_event(
            10,
            vec![
                spawn_record(first, [1.5, 70.25, -2.5], 0.25, 9),
                spawn_record(second, [2.5, 71.0, 2.5], 0.5, 20),
            ],
        ),
    );

    let fixture = view_fixture();
    let view = fixture.view_of(&provider);
    let records = project_passive(&view).expect("spawn batch projects its records");
    assert_eq!(records.len(), 2, "one record per spawn in packet order");

    let expected = [
        (first, [1.5, 70.25, -2.5], 0.25f32, 9u8, 0u32),
        (second, [2.5, 71.0, 2.5], 0.5, 20, 1),
    ];
    for (entry, (id, position, yaw, health, ordinal)) in records.iter().zip(&expected) {
        assert_eq!(entry.record().kind(), ActorKind::Passive);
        assert_eq!(*entry.record().id(), ActorId::Passive(*id));
        assert_eq!(
            *entry.record().dimension(),
            ActorDimension::Known(Dimension::OVERWORLD),
            "the spawn names its own dimension"
        );
        assert_eq!(entry.record().position(), Some(*position));
        assert_eq!(entry.record().yaw(), Some(f64::from(*yaw)));
        assert_eq!(
            entry.record().pitch(),
            None,
            "the passive spawn carries no pitch"
        );
        assert_eq!(
            entry.record().velocity(),
            None,
            "the passive spawn carries no velocity"
        );
        assert_eq!(
            passive_detail(entry),
            (Some(*health), None, None),
            "spawn detail is the health alone"
        );
        assert_eq!(entry.record().header().operation(), FamilyOperation::Upsert);
        assert_eq!(entry.record().header().source_tick(), Some(10));
        assert_eq!(entry.record().header().epoch().get(), EPOCH);
        assert_eq!(
            entry.record().header().revision(),
            view.frame_revision(),
            "headers are rebased onto the coherent candidate revision"
        );
        assert_stable_key(entry, *id);
        assert_confirmed_order(entry, 1, *ordinal);
    }
    assert!(
        records[0].order() < records[1].order(),
        "batch records keep their published packet order"
    );
}

/// `state`: one upsert per batch record in published packet order, each with
/// the dimension resolved from the mirror's prevalidated identity, the
/// grazing bit as the exact 0/1 wire value, health, the widened velocity —
/// and no pitch and no despawn reason.
#[test]
fn state_batch_keeps_packet_order_graze_and_resolved_dimension() {
    let mut provider = admitted_mirror();
    let first = passive_id(3);
    let second = passive_id(9);
    commit(
        &mut provider,
        spawn_event(
            100,
            vec![
                spawn_record(first, [1.0, 70.0, 1.0], 0.5, 12),
                spawn_record(second, [2.0, 70.0, 2.0], 0.5, 20),
            ],
        ),
    );
    commit(
        &mut provider,
        state_event(
            101,
            vec![
                state_record(first, [1.5, 70.5, 1.5], [0.25, 0.0, 0.0], 0.75, 7, true),
                state_record(second, [2.5, 70.5, 2.5], [0.0, 0.0, -0.25], 0.9, 20, false),
            ],
        ),
    );

    let fixture = view_fixture();
    let records = project_passive(&fixture.view_of(&provider)).expect("state batch projects");
    assert_eq!(records.len(), 4, "two spawns then two state records");

    let expected = [
        (
            first,
            [1.5, 70.5, 1.5],
            [0.25, 0.0, 0.0],
            7u8,
            1u8,
            2u64,
            0u32,
        ),
        (second, [2.5, 70.5, 2.5], [0.0, 0.0, -0.25], 20, 0, 2, 1),
    ];
    for (entry, (id, position, velocity, health, grazing, revision, ordinal)) in
        records.iter().skip(2).zip(&expected)
    {
        assert_eq!(*entry.record().id(), ActorId::Passive(*id));
        assert_eq!(
            *entry.record().dimension(),
            ActorDimension::Known(Dimension::OVERWORLD),
            "the dimensionless state resolves through the mirror's identity"
        );
        assert_eq!(entry.record().position(), Some(*position));
        assert_eq!(entry.record().velocity(), Some(*velocity));
        assert_eq!(entry.record().pitch(), None);
        assert_eq!(
            passive_detail(entry),
            (Some(*health), Some(*grazing), None),
            "state detail is health beside the grazing bit"
        );
        assert_eq!(entry.record().header().source_tick(), Some(101));
        assert_stable_key(entry, *id);
        assert_confirmed_order(entry, *revision, *ordinal);
    }
    assert!(
        records[2].order() < records[3].order(),
        "records of one batch keep the packet record ordinal order"
    );
}

/// `remove`: one remove-only record per despawn entry with the exact
/// vanished/died reason from the closed wire union — no pose, no angle, no
/// velocity, no health and no grazing survive the identity — and the
/// dimension resolved from the confirmed identity.
#[test]
fn despawn_publishes_exact_vanished_and_died_reasons() {
    let mut provider = admitted_mirror();
    let vanished = passive_id(3);
    let died = passive_id(9);
    commit(
        &mut provider,
        spawn_event(
            100,
            vec![
                spawn_record(vanished, [1.0, 70.0, 1.0], 0.5, 12),
                spawn_record(died, [2.0, 70.0, 2.0], 0.5, 3),
            ],
        ),
    );
    commit(
        &mut provider,
        despawn_event(
            110,
            vec![
                PassiveDespawnRecord::new(vanished, PassiveDespawnReason::Vanished),
                PassiveDespawnRecord::new(died, PassiveDespawnReason::Died),
            ],
        ),
    );

    let fixture = view_fixture();
    let records = project_passive(&fixture.view_of(&provider)).expect("despawn batch projects");
    assert_eq!(records.len(), 4);

    let expected = [
        (vanished, PassiveDespawnReason::Vanished, 0u32),
        (died, PassiveDespawnReason::Died, 1),
    ];
    for (entry, (id, reason, ordinal)) in records.iter().skip(2).zip(&expected) {
        assert_eq!(entry.record().header().operation(), FamilyOperation::Remove);
        assert_eq!(*entry.record().id(), ActorId::Passive(*id));
        assert_eq!(
            *entry.record().dimension(),
            ActorDimension::Known(Dimension::OVERWORLD)
        );
        assert_eq!(entry.record().position(), None);
        assert_eq!(entry.record().yaw(), None);
        assert_eq!(entry.record().pitch(), None);
        assert_eq!(entry.record().velocity(), None);
        assert_eq!(
            passive_detail(entry),
            (None, None, Some(*reason)),
            "the despawn detail is the exact wire reason alone"
        );
        assert_eq!(entry.record().header().source_tick(), Some(110));
        assert_stable_key(entry, *id);
        assert_confirmed_order(entry, 2, *ordinal);
    }
}

/// The full lifecycle table: spawn, grazing state, died removal, then reuse
/// of the same typed identity after its removal, in actual source order with
/// one shared stable identity key and remove before reuse.
#[test]
fn spawn_state_remove_reuse_lifecycle() {
    let mut provider = admitted_mirror();
    let id = passive_id(21);
    commit(
        &mut provider,
        spawn_event(10, vec![spawn_record(id, [4.0, 64.0, 4.0], 0.5, 20)]),
    );
    commit(
        &mut provider,
        state_event(
            11,
            vec![state_record(
                id,
                [4.5, 64.5, 4.5],
                [0.5, 0.0, 0.0],
                0.6,
                19,
                true,
            )],
        ),
    );
    commit(
        &mut provider,
        despawn_event(
            12,
            vec![PassiveDespawnRecord::new(id, PassiveDespawnReason::Died)],
        ),
    );
    commit(
        &mut provider,
        spawn_event(13, vec![spawn_record(id, [5.0, 64.0, 5.0], 0.5, 20)]),
    );

    let fixture = view_fixture();
    let records = project_passive(&fixture.view_of(&provider)).expect("lifecycle projects");
    assert_eq!(records.len(), 4);

    let expected = [
        (
            FamilyOperation::Upsert,
            Some(10u64),
            1u64,
            0u32,
            Some(20u8),
            None,
            None,
        ),
        (
            FamilyOperation::Upsert,
            Some(11),
            2,
            0,
            Some(19),
            Some(1),
            None,
        ),
        (
            FamilyOperation::Remove,
            Some(12),
            3,
            0,
            None,
            None,
            Some(PassiveDespawnReason::Died),
        ),
        (
            FamilyOperation::Upsert,
            Some(13),
            4,
            0,
            Some(20),
            None,
            None,
        ),
    ];
    for (entry, (operation, tick, revision, ordinal, health, grazing, reason)) in
        records.iter().zip(&expected)
    {
        assert_eq!(entry.record().header().operation(), *operation);
        assert_eq!(entry.record().header().source_tick(), *tick);
        assert_eq!(
            passive_detail(entry),
            (*health, *grazing, *reason),
            "each packet fills exactly its own fields"
        );
        assert_stable_key(entry, id);
        assert_confirmed_order(entry, *revision, *ordinal);
    }

    // The remove record is a removal alone beside its reason.
    let remove = &records[2];
    assert_eq!(remove.record().position(), None);
    assert_eq!(remove.record().yaw(), None);
    assert_eq!(remove.record().pitch(), None);
    assert_eq!(remove.record().velocity(), None);

    assert!(
        records
            .windows(2)
            .all(|pair| pair[0].order() < pair[1].order()),
        "records keep actual source order across the lifecycle"
    );
}

/// No lure or drop inference: hostile observations carrying velocity- and
/// health-shaped data and item-drop observations carrying block-index and
/// stack-shaped data produce no passive records, and no passive record ever
/// carries a foreign detail payload.
#[test]
fn foreign_drop_and_lure_shaped_kinds_are_ignored() {
    let mut provider = admitted_mirror();
    let id = passive_id(5);
    commit(
        &mut provider,
        spawn_event(10, vec![spawn_record(id, [6.0, 64.0, 6.0], 0.5, 20)]),
    );
    for event in hostile_velocity_events(20, HostileId::try_new(9).expect("hostile identity")) {
        commit(&mut provider, event);
    }
    commit(&mut provider, drop_upsert_event(22));

    let fixture = view_fixture();
    let records =
        project_passive(&fixture.view_of(&provider)).expect("foreign kinds are ignorable");
    assert_eq!(records.len(), 1, "only the passive observations project");
    for entry in &records {
        assert_eq!(entry.record().kind(), ActorKind::Passive);
        assert_eq!(*entry.record().id(), ActorId::Passive(id));
        assert_eq!(
            passive_detail(entry),
            (Some(20), None, None),
            "drop-shaped and lure-shaped source data never becomes a passive field"
        );
        assert_eq!(
            entry.record().velocity(),
            None,
            "the spawn carries no velocity"
        );
    }
}

/// Malformed prevalidation doubles reject the whole projection with no
/// partial output: a state record whose prevalidated resolution is missing,
/// a despawn without its resolved identity, a despawn whose resolution names
/// another identity, a despawn resolved live in two dimensions at once, a
/// packet that is not a checked publication, and an old-epoch observation
/// key.
#[test]
fn invalid_observations_reject_without_partial_output() {
    let epoch = SessionEpoch::try_new(EPOCH).expect("epoch");
    let fixture = view_fixture();
    let provider = admitted_mirror();
    let id = passive_id(7);
    let other = passive_id(8);
    let state = packet(state_event(
        101,
        vec![state_record(
            id,
            [1.0, 70.0, 1.0],
            [0.0, 0.0, 0.0],
            0.5,
            9,
            false,
        )],
    ));
    let despawn = packet(despawn_event(
        102,
        vec![PassiveDespawnRecord::new(
            id,
            PassiveDespawnReason::Vanished,
        )],
    ));

    let unresolved_state = AcceptedObservation::try_new(
        ObservationKey::try_new(epoch, ConfirmedRevision::new(1), 0).expect("key"),
        Some(101),
        state,
        Vec::new(),
    )
    .expect("staged state without resolution");
    assert_eq!(
        project_passive(&fixture.view(
            provider.mirror(),
            &[unresolved_state],
            epoch,
            ConfirmedRevision::new(1)
        )),
        Err(ClientError::InvalidInput),
        "a dimensionless state with no resolved identity rejects"
    );

    let missing = AcceptedObservation::try_new(
        ObservationKey::try_new(epoch, ConfirmedRevision::new(1), 0).expect("key"),
        Some(102),
        despawn.clone(),
        Vec::new(),
    )
    .expect("staged despawn without resolution");
    assert_eq!(
        project_passive(&fixture.view(
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
        Some(102),
        despawn.clone(),
        vec![
            ResolvedActor::try_new(
                ActorKind::Passive,
                ActorId::Passive(other),
                Dimension::OVERWORLD,
            )
            .expect("resolved other identity"),
        ],
    )
    .expect("staged with foreign resolution");
    assert_eq!(
        project_passive(&fixture.view(
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
        Some(102),
        despawn,
        vec![
            ResolvedActor::try_new(
                ActorKind::Passive,
                ActorId::Passive(id),
                Dimension::OVERWORLD,
            )
            .expect("resolved overworld identity"),
            ResolvedActor::try_new(
                ActorKind::Passive,
                ActorId::Passive(id),
                Dimension::OVERWORLD,
            )
            .expect("resolved second identity"),
        ],
    )
    .expect("staged with ambiguous resolution");
    assert_eq!(
        project_passive(&fixture.view(
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
        project_passive(&fixture.view(
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
        packet(spawn_event(
            10,
            vec![spawn_record(id, [0.0, 64.0, 0.0], 0.5, 20)],
        )),
        Vec::new(),
    )
    .expect("staged in another epoch");
    assert_eq!(
        project_passive(&fixture.view(
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
        project_passive(&fixture.view_of(&provider))
            .expect("empty queue projects")
            .is_empty(),
        "an empty committed queue projects no records"
    );
}

/// A late graze is published in observation order, never re-sorted by its
/// older server tick: the record keeps its actual tick and its position
/// after the newer batch, so equal or backwards ticks never reconstruct
/// source order.
#[test]
fn late_graze_keeps_observation_order_not_tick_order() {
    let mut provider = admitted_mirror();
    let id = passive_id(4);
    commit(
        &mut provider,
        spawn_event(100, vec![spawn_record(id, [1.0, 70.0, 1.0], 0.5, 20)]),
    );
    commit(
        &mut provider,
        state_event(
            101,
            vec![state_record(
                id,
                [1.5, 70.0, 1.5],
                [0.5, 0.0, 0.0],
                0.5,
                20,
                true,
            )],
        ),
    );
    commit(
        &mut provider,
        state_event(
            90,
            vec![state_record(
                id,
                [2.0, 70.0, 2.0],
                [0.5, 0.0, 0.0],
                0.5,
                20,
                false,
            )],
        ),
    );

    let fixture = view_fixture();
    let records = project_passive(&fixture.view_of(&provider)).expect("late graze projects");
    assert_eq!(records.len(), 3);

    let late = &records[2];
    assert_eq!(
        passive_detail(late),
        (Some(20), Some(0), None),
        "the stale-ticked clear publishes its own graze value"
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
/// before this projection ever sees them: the committed revision, the
/// observation queue and the projected output all stay unchanged, while a
/// later reuse of the removed identity still commits.
#[test]
fn late_or_duplicate_inputs_leave_committed_state_unchanged() {
    let mut provider = admitted_mirror();
    let id = passive_id(11);
    commit(
        &mut provider,
        spawn_event(10, vec![spawn_record(id, [7.0, 64.0, 7.0], 0.5, 20)]),
    );
    commit(
        &mut provider,
        state_event(
            11,
            vec![state_record(
                id,
                [7.5, 64.5, 7.5],
                [0.5, 0.0, 0.0],
                0.5,
                18,
                true,
            )],
        ),
    );
    commit(
        &mut provider,
        despawn_event(
            12,
            vec![PassiveDespawnRecord::new(
                id,
                PassiveDespawnReason::Vanished,
            )],
        ),
    );

    let fixture = view_fixture();
    let baseline = project_passive(&fixture.view_of(&provider)).expect("baseline projects");
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
                state_event(
                    99,
                    vec![state_record(
                        id,
                        [9.0, 64.0, 9.0],
                        [0.0, 0.0, 0.0],
                        0.5,
                        5,
                        false
                    )],
                ),
            ))
            .is_err(),
        "a late state cannot resurrect a despawn"
    );
    assert!(
        provider
            .commit(&staged(
                &provider,
                despawn_event(
                    13,
                    vec![PassiveDespawnRecord::new(id, PassiveDespawnReason::Died)],
                ),
            ))
            .is_err(),
        "a duplicate despawn is an orphan removal"
    );
    assert_eq!(provider.mirror().revision().get(), 3);
    assert_eq!(provider.observations().len(), 3);
    assert!(provider.mirror().actors().passives().is_empty());
    assert_eq!(
        project_passive(&fixture.view_of(&provider)).expect("projection still succeeds"),
        baseline,
        "the refused inputs leave the projection unchanged"
    );

    // Removal precedes reuse: the same identity spawns again after its
    // despawn and the projection gains exactly the new upsert.
    commit(
        &mut provider,
        spawn_event(14, vec![spawn_record(id, [8.0, 64.0, 8.0], 0.5, 20)]),
    );
    let reused = project_passive(&fixture.view_of(&provider)).expect("reuse projects");
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

/// A duplicate spawn of a live identity is refused by the mirror before this
/// projection ever sees it: the committed state and the projection stay
/// unchanged, and no second upsert appears.
#[test]
fn duplicate_spawn_leaves_committed_state_unchanged() {
    let mut provider = admitted_mirror();
    let id = passive_id(12);
    commit(
        &mut provider,
        spawn_event(10, vec![spawn_record(id, [1.0, 64.0, 1.0], 0.5, 20)]),
    );

    let fixture = view_fixture();
    let baseline = project_passive(&fixture.view_of(&provider)).expect("baseline projects");
    assert_eq!(baseline.len(), 1);

    let next = provider.mirror().revision().get() + 1;
    let staged = AcceptedObservation::try_new(
        ObservationKey::try_new(
            SessionEpoch::try_new(EPOCH).expect("epoch"),
            ConfirmedRevision::new(next),
            0,
        )
        .expect("key"),
        None,
        packet(spawn_event(
            11,
            vec![spawn_record(id, [9.0, 9.0, 9.0], 0.5, 20)],
        )),
        Vec::new(),
    )
    .expect("staged");
    assert!(
        provider.commit(&staged).is_err(),
        "a spawn naming a live identity is a duplicate"
    );
    assert_eq!(provider.mirror().revision().get(), 1);
    assert_eq!(provider.observations().len(), 1);
    assert_eq!(
        project_passive(&fixture.view_of(&provider)).expect("projection still succeeds"),
        baseline,
        "the refused duplicate leaves the projection unchanged"
    );
}

/// An invalid pose is refused at the checked domain boundary before any
/// observation exists: a non-finite position or yaw never becomes a passive
/// observation, so the projection never sees one.
#[test]
fn invalid_pose_is_refused_at_the_checked_domain_boundary() {
    assert!(
        FiniteVec3::try_new([f32::NAN, 64.0, 0.0]).is_err(),
        "a non-finite position never becomes a checked body"
    );
    assert!(
        PassiveStateRecord::try_new(PassiveStateRecordParts {
            id: passive_id(3),
            position: finite_pos([1.0, 70.0, 1.0]),
            velocity: finite_pos([0.0, 0.0, 0.0]),
            yaw: f32::NAN,
            health: 20,
            grazing: false,
        })
        .is_err(),
        "a non-finite yaw never becomes a passive state record"
    );
    assert!(
        PassiveSpawnRecord::try_new(PassiveSpawnRecordParts {
            id: passive_id(4),
            dimension: Dimension::OVERWORLD,
            position: finite_pos([1.0, 70.0, 1.0]),
            yaw: 0.5,
            health: 0,
        })
        .is_err(),
        "a zero-health body is removed rather than published"
    );
}
