//! Hostile actor projection contract tests.
//!
//! The table pins the accepted hostile semantics against the landed mirror
//! provider: a spawn batch is one yaw-only upsert per record carrying the
//! archetype and health detail and no pitch; a state batch advances position,
//! velocity and health together with the dimension resolved from the
//! confirmed identity; a `CombatHit` observation contributes no attacker
//! association and no pitch; an old-epoch observation rejects the whole
//! projection; and the same identity may spawn again after its removal.
//! Late, duplicate or malformed inputs leave the committed mirror, the
//! observation queue and the projected output unchanged. Order keys are the
//! actual observation keys with packet record ordinals; equal or absent
//! source ticks never reconstruct order.

use mornlea_client_core::contracts::{
    ClientError, ClientLimits, ConfirmedRevision, FamilyOperation, ObservationKey, SessionEpoch,
};
use mornlea_client_core::input::InputProjectionState;
use mornlea_client_core::prediction::PlayerProjectionState;
use mornlea_client_core::presentation::actors::hostile::project_hostile;
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
    CombatHit, CombatTarget, Dimension, Event, FiniteVec3, HostileDespawn, HostileDespawnParts,
    HostileId, HostileKind, HostileSpawn, HostileSpawnParts, HostileSpawnRecord,
    HostileSpawnRecordParts, HostileState, HostileStateParts, HostileStateRecord,
    HostileStateRecordParts,
};
use mornlea_protocol::{HOSTILE_KIND_BONE_THROWER, HOSTILE_KIND_NIGHTWALKER, ServerPacket};

/// The epoch every fixture mirror and observation key carries.
const EPOCH: u64 = 7;

fn hostile_id(value: u64) -> HostileId {
    HostileId::try_new(value).expect("checked hostile identity")
}

fn finite_pos(components: [f32; 3]) -> FiniteVec3 {
    FiniteVec3::try_new(components).expect("finite position")
}

fn spawn_record(
    id: HostileId,
    position: [f32; 3],
    yaw: f32,
    health: u8,
    kind: HostileKind,
) -> HostileSpawnRecord {
    HostileSpawnRecord::try_new(HostileSpawnRecordParts {
        id,
        dimension: Dimension::OVERWORLD,
        position: finite_pos(position),
        yaw,
        health,
        kind,
    })
    .expect("checked hostile spawn record")
}

fn spawn_event(tick: u64, records: Vec<HostileSpawnRecord>) -> Event {
    Event::HostileSpawn(
        HostileSpawn::try_new(HostileSpawnParts {
            server_tick: tick,
            spawns: records.into_boxed_slice(),
        })
        .expect("checked hostile spawn batch"),
    )
}

fn state_record(
    id: HostileId,
    position: [f32; 3],
    velocity: [f32; 3],
    yaw: f32,
    health: u8,
    kind: HostileKind,
) -> HostileStateRecord {
    HostileStateRecord::try_new(HostileStateRecordParts {
        id,
        position: finite_pos(position),
        velocity: finite_pos(velocity),
        yaw,
        health,
        kind,
    })
    .expect("checked hostile state record")
}

fn state_event(tick: u64, records: Vec<HostileStateRecord>) -> Event {
    Event::HostileState(
        HostileState::try_new(HostileStateParts {
            server_tick: tick,
            states: records.into_boxed_slice(),
        })
        .expect("checked hostile state batch"),
    )
}

fn despawn_event(tick: u64, ids: Vec<HostileId>) -> Event {
    Event::HostileDespawn(
        HostileDespawn::try_new(HostileDespawnParts {
            server_tick: tick,
            ids: ids.into_boxed_slice(),
        })
        .expect("checked hostile despawn batch"),
    )
}

/// A combat hit against a hostile-kind target: the publication names a target
/// category alone, never a hostile identity, so no association can be drawn.
fn combat_hit_event(tick: u64, damage: u8) -> Event {
    Event::CombatHit(
        CombatHit::try_new(tick, damage, CombatTarget::Hostile).expect("checked combat hit"),
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

/// Stages one observation without committing it, for the malformed doubles
/// the prevalidated identity resolution would never carry.
fn staged(event: Event, revision: u64, resolved: Vec<ResolvedActor>) -> AcceptedObservation {
    AcceptedObservation::try_new(
        ObservationKey::try_new(
            SessionEpoch::try_new(EPOCH).expect("epoch"),
            ConfirmedRevision::new(revision),
            0,
        )
        .expect("staged key"),
        None,
        packet(event),
        resolved,
    )
    .expect("staged")
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
        other => panic!("hostile record is server-sourced: {other:?}"),
    }
}

/// The hostile detail payload, refusing a foreign detail tag: a remove-only
/// record carries no detail at all.
fn hostile_detail(entry: &OrderedRecord<ActorRecord>) -> Option<(u8, u8)> {
    match entry.record().detail() {
        None => None,
        Some(ActorDetail::Hostile { archetype, health }) => Some((*archetype, *health)),
        Some(other) => panic!("hostile record carries a foreign detail tag: {other:?}"),
    }
}

/// Asserts every record of one projection keeps the publication's yaw-only
/// shape: the hostile packets carry no pitch, so none is ever published.
fn assert_no_pitch(records: &[OrderedRecord<ActorRecord>]) {
    for entry in records {
        assert_eq!(
            entry.record().pitch(),
            None,
            "the hostile publications carry yaw alone"
        );
    }
}

/// `spawn`: one yaw-only upsert per spawn record, carrying the typed identity,
/// the actual spawn dimension, the exact finite position widened from f32, the
/// archetype and health detail, and no velocity and no pitch.
#[test]
fn yaw_only_spawn_carries_health_without_pitch() {
    let mut provider = admitted_mirror();
    let id = hostile_id(7);
    commit(
        &mut provider,
        spawn_event(
            100,
            vec![spawn_record(
                id,
                [1.0, 2.0, 3.0],
                0.25,
                13,
                HostileKind::Nightwalker,
            )],
        ),
    );

    let fixture = view_fixture();
    let records = project_hostile(&fixture.view_of(&provider)).expect("spawn projects one record");
    assert_eq!(records.len(), 1);

    let entry = &records[0];
    assert_eq!(entry.record().kind(), ActorKind::Hostile);
    assert_eq!(*entry.record().id(), ActorId::Hostile(id));
    assert_eq!(
        *entry.record().dimension(),
        ActorDimension::Known(Dimension::OVERWORLD)
    );
    assert_eq!(entry.record().position(), Some([1.0, 2.0, 3.0]));
    assert_eq!(entry.record().yaw(), Some(f64::from(0.25f32)));
    assert_eq!(
        entry.record().velocity(),
        None,
        "a spawn carries no velocity"
    );
    assert_eq!(hostile_detail(entry), Some((HOSTILE_KIND_NIGHTWALKER, 13)));
    assert_eq!(entry.record().header().operation(), FamilyOperation::Upsert);
    assert_eq!(entry.record().header().source_tick(), Some(100));
    assert_eq!(entry.record().header().epoch().get(), EPOCH);
    assert_eq!(
        entry.record().header().revision(),
        fixture.view_of(&provider).frame_revision(),
        "headers are rebased onto the coherent candidate revision"
    );
    assert_eq!(
        *entry.stable_key(),
        StableRecordKey::Actor {
            kind: ActorKind::Hostile,
            dimension: ActorDimension::Known(Dimension::OVERWORLD),
            id: ActorId::Hostile(id),
        }
    );
    assert_confirmed_order(entry, 1, 0);
    assert_no_pitch(&records);
}

/// `state`: position, velocity and health advance together, one record per
/// batch entry in published packet order, with the dimension resolved from
/// the confirmed identity because the state packet carries none. Each
/// published sample keeps its own exact position and source tick — the
/// projection never merges, interpolates or collapses to the newest sample.
#[test]
fn state_batch_advances_health_velocity_and_position_together() {
    let mut provider = admitted_mirror();
    let walker = hostile_id(7);
    let thrower = hostile_id(9);
    commit(
        &mut provider,
        spawn_event(
            100,
            vec![
                spawn_record(walker, [1.0, 2.0, 3.0], 0.25, 20, HostileKind::Nightwalker),
                spawn_record(thrower, [4.0, 5.0, 6.0], 0.5, 19, HostileKind::BoneThrower),
            ],
        ),
    );
    commit(
        &mut provider,
        state_event(
            101,
            vec![
                state_record(
                    walker,
                    [2.0, 2.0, 3.0],
                    [0.5, 0.0, 0.0],
                    0.5,
                    11,
                    HostileKind::Nightwalker,
                ),
                state_record(
                    thrower,
                    [4.0, 5.5, 6.0],
                    [0.0, 1.0, 0.0],
                    0.75,
                    17,
                    HostileKind::BoneThrower,
                ),
            ],
        ),
    );
    commit(
        &mut provider,
        state_event(
            102,
            vec![state_record(
                walker,
                [3.0, 2.0, 3.0],
                [0.5, 0.0, 0.0],
                0.5,
                9,
                HostileKind::Nightwalker,
            )],
        ),
    );

    let fixture = view_fixture();
    let records =
        project_hostile(&fixture.view_of(&provider)).expect("state batches project records");
    assert_eq!(records.len(), 5, "two spawns, two states, one state");

    let expected = [
        (
            walker,
            Some([2.0, 2.0, 3.0]),
            Some([0.5, 0.0, 0.0]),
            Some(f64::from(0.5f32)),
            HOSTILE_KIND_NIGHTWALKER,
            11u8,
            101u64,
            2u64,
            0u32,
        ),
        (
            thrower,
            Some([4.0, 5.5, 6.0]),
            Some([0.0, 1.0, 0.0]),
            Some(f64::from(0.75f32)),
            HOSTILE_KIND_BONE_THROWER,
            17,
            101,
            2,
            1,
        ),
    ];
    for (entry, (id, position, velocity, yaw, archetype, health, tick, revision, ordinal)) in
        records.iter().skip(2).zip(&expected)
    {
        assert_eq!(*entry.record().id(), ActorId::Hostile(*id));
        assert_eq!(
            *entry.record().dimension(),
            ActorDimension::Known(Dimension::OVERWORLD),
            "the dimensionless state resolves against the confirmed identity"
        );
        assert_eq!(entry.record().position(), *position);
        assert_eq!(entry.record().velocity(), *velocity);
        assert_eq!(entry.record().yaw(), *yaw);
        assert_eq!(hostile_detail(entry), Some((*archetype, *health)));
        assert_eq!(entry.record().header().source_tick(), Some(*tick));
        assert_eq!(entry.record().header().operation(), FamilyOperation::Upsert);
        assert_confirmed_order(entry, *revision, *ordinal);
    }
    assert!(
        records[2].order() < records[3].order(),
        "one batch's records keep the packet record ordinal order"
    );

    // The later sample publishes its own exact position and tick, never a
    // merged or newest-only value; earlier samples stay exactly as published.
    let later = &records[4];
    assert_eq!(*later.record().id(), ActorId::Hostile(walker));
    assert_eq!(later.record().position(), Some([3.0, 2.0, 3.0]));
    assert_eq!(later.record().velocity(), Some([0.5, 0.0, 0.0]));
    assert_eq!(hostile_detail(later), Some((HOSTILE_KIND_NIGHTWALKER, 9)));
    assert_eq!(later.record().header().source_tick(), Some(102));
    assert_confirmed_order(later, 3, 0);
    assert!(
        records
            .windows(2)
            .all(|pair| pair[0].order() < pair[1].order()),
        "records keep actual source order across the batches"
    );
    assert_no_pitch(&records);
}

/// A `CombatHit` against a hostile-kind target contributes no attacker
/// association: it adds no record, changes no health, and never publishes a
/// pitch. The hostile publications stay the only source of hostile state.
#[test]
fn combat_hit_adds_no_attacker_association() {
    let mut provider = admitted_mirror();
    let id = hostile_id(7);
    commit(
        &mut provider,
        spawn_event(
            100,
            vec![spawn_record(
                id,
                [1.0, 1.0, 1.0],
                0.0,
                20,
                HostileKind::Nightwalker,
            )],
        ),
    );
    commit(
        &mut provider,
        state_event(
            101,
            vec![state_record(
                id,
                [1.5, 1.0, 1.0],
                [0.0, 0.0, 0.0],
                0.25,
                18,
                HostileKind::Nightwalker,
            )],
        ),
    );
    commit(&mut provider, combat_hit_event(102, 2));

    let fixture = view_fixture();
    let records = project_hostile(&fixture.view_of(&provider)).expect("projection succeeds");
    assert_eq!(
        records.len(),
        2,
        "the combat hit publishes no hostile record"
    );
    for entry in &records {
        assert_eq!(*entry.record().id(), ActorId::Hostile(id));
    }
    // The published health values stay the publications' own: the hit neither
    // decrements a body nor attributes an attacker to the identity.
    assert_eq!(
        hostile_detail(&records[0]),
        Some((HOSTILE_KIND_NIGHTWALKER, 20))
    );
    assert_eq!(
        hostile_detail(&records[1]),
        Some((HOSTILE_KIND_NIGHTWALKER, 18))
    );
    assert_confirmed_order(&records[0], 1, 0);
    assert_confirmed_order(&records[1], 2, 0);
    assert_no_pitch(&records);
}

/// Old epoch and reuse: an observation key from another epoch rejects the
/// whole projection with no partial output, and the same identity may spawn
/// again after its removal, keeping its stable key through the reuse.
#[test]
fn old_epoch_rejects_and_removed_identity_reuses() {
    let epoch = SessionEpoch::try_new(EPOCH).expect("epoch");
    let fixture = view_fixture();
    let mut provider = admitted_mirror();
    let id = hostile_id(7);
    commit(
        &mut provider,
        spawn_event(
            10,
            vec![spawn_record(
                id,
                [1.0, 64.0, 1.0],
                0.0,
                20,
                HostileKind::Nightwalker,
            )],
        ),
    );
    commit(
        &mut provider,
        state_event(
            11,
            vec![state_record(
                id,
                [1.5, 64.0, 1.0],
                [0.25, 0.0, 0.0],
                0.0,
                19,
                HostileKind::Nightwalker,
            )],
        ),
    );
    commit(&mut provider, despawn_event(12, vec![id]));

    let stale_epoch = AcceptedObservation::try_new(
        ObservationKey::try_new(
            SessionEpoch::try_new(EPOCH + 1).expect("next epoch"),
            ConfirmedRevision::new(4),
            0,
        )
        .expect("key"),
        None,
        packet(state_event(
            99,
            vec![state_record(
                id,
                [9.0, 64.0, 9.0],
                [0.0, 0.0, 0.0],
                0.0,
                1,
                HostileKind::Nightwalker,
            )],
        )),
        Vec::new(),
    )
    .expect("staged in another epoch");
    assert_eq!(
        project_hostile(&fixture.view(
            provider.mirror(),
            &[stale_epoch],
            epoch,
            ConfirmedRevision::new(4)
        )),
        Err(ClientError::StaleEpoch),
        "an observation from another epoch rejects without output"
    );

    let records = project_hostile(&fixture.view_of(&provider)).expect("lifecycle projects");
    assert_eq!(records.len(), 3);
    let stable = StableRecordKey::Actor {
        kind: ActorKind::Hostile,
        dimension: ActorDimension::Known(Dimension::OVERWORLD),
        id: ActorId::Hostile(id),
    };
    for entry in &records {
        assert_eq!(*entry.stable_key(), stable);
    }

    // The despawn is a removal alone: no pose, no angle, no velocity and no
    // detail survive the identity, and the dimension comes from the
    // resolution the confirmed mirror recorded.
    let remove = &records[2];
    assert_eq!(
        remove.record().header().operation(),
        FamilyOperation::Remove
    );
    assert_eq!(remove.record().header().source_tick(), Some(12));
    assert_eq!(remove.record().position(), None);
    assert_eq!(remove.record().yaw(), None);
    assert_eq!(remove.record().velocity(), None);
    assert!(remove.record().detail().is_none());
    assert_confirmed_order(remove, 3, 0);

    // Reuse: the same identity spawns again after its removal.
    commit(
        &mut provider,
        spawn_event(
            13,
            vec![spawn_record(
                id,
                [2.0, 64.0, 2.0],
                0.0,
                20,
                HostileKind::Nightwalker,
            )],
        ),
    );
    let reused = project_hostile(&fixture.view_of(&provider)).expect("reuse projects");
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
    assert_eq!(
        *reused.last().expect("respawn record").stable_key(),
        stable,
        "reuse keeps the typed identity"
    );
    assert_no_pitch(&reused);
}

/// Malformed prevalidation doubles reject the whole projection with no
/// partial output: a dimensionless despawn or state without its resolved
/// identity, a resolution naming another identity, and a resolution that is
/// ambiguous between two dimensions.
#[test]
fn invalid_identity_doubles_reject_without_partial_output() {
    let epoch = SessionEpoch::try_new(EPOCH).expect("epoch");
    let fixture = view_fixture();
    let provider = admitted_mirror();
    let id = hostile_id(7);
    let other = hostile_id(8);

    let missing = staged(despawn_event(10, vec![id]), 1, Vec::new());
    assert_eq!(
        project_hostile(&fixture.view(
            provider.mirror(),
            &[missing],
            epoch,
            ConfirmedRevision::new(1)
        )),
        Err(ClientError::InvalidInput),
        "a despawn with no resolved identity rejects"
    );

    let mismatched = staged(
        despawn_event(10, vec![id]),
        1,
        vec![
            ResolvedActor::try_new(
                ActorKind::Hostile,
                ActorId::Hostile(other),
                Dimension::OVERWORLD,
            )
            .expect("resolved other identity"),
        ],
    );
    assert_eq!(
        project_hostile(&fixture.view(
            provider.mirror(),
            &[mismatched],
            epoch,
            ConfirmedRevision::new(1)
        )),
        Err(ClientError::InvalidInput),
        "a resolution naming another identity rejects"
    );

    let ambiguous = staged(
        state_event(
            10,
            vec![state_record(
                id,
                [1.0, 1.0, 1.0],
                [0.0, 0.0, 0.0],
                0.0,
                20,
                HostileKind::Nightwalker,
            )],
        ),
        1,
        vec![
            ResolvedActor::try_new(
                ActorKind::Hostile,
                ActorId::Hostile(id),
                Dimension::OVERWORLD,
            )
            .expect("resolved identity"),
            ResolvedActor::try_new(ActorKind::Hostile, ActorId::Hostile(id), Dimension::DEPTHS)
                .expect("resolved identity again"),
        ],
    );
    assert_eq!(
        project_hostile(&fixture.view(
            provider.mirror(),
            &[ambiguous],
            epoch,
            ConfirmedRevision::new(1)
        )),
        Err(ClientError::InvalidInput),
        "an ambiguous resolution rejects"
    );

    // The untouched baseline still projects exactly its committed records:
    // the rejected doubles changed no owner state.
    assert!(
        project_hostile(&fixture.view_of(&provider))
            .expect("empty queue projects")
            .is_empty(),
        "an empty committed queue projects no records"
    );
}

/// Late and duplicate inputs are refused by the accepted mirror schema
/// before this projection ever sees them: a duplicate spawn of a live
/// identity, a state for an unknown identity, a state for a removed
/// identity and a duplicate despawn all leave the committed revision, the
/// observation queue and the projected output unchanged, while a valid state
/// advances position and health together and a later reuse still commits.
#[test]
fn late_or_duplicate_inputs_leave_committed_state_unchanged() {
    let mut provider = admitted_mirror();
    let id = hostile_id(7);
    commit(
        &mut provider,
        spawn_event(
            100,
            vec![spawn_record(
                id,
                [1.0, 2.0, 3.0],
                0.25,
                13,
                HostileKind::Nightwalker,
            )],
        ),
    );

    let fixture = view_fixture();
    let baseline = project_hostile(&fixture.view_of(&provider)).expect("baseline projects");
    assert_eq!(baseline.len(), 1);
    assert_eq!(baseline[0].record().position(), Some([1.0, 2.0, 3.0]));
    assert_eq!(
        hostile_detail(&baseline[0]),
        Some((HOSTILE_KIND_NIGHTWALKER, 13))
    );

    let duplicate_spawn = spawn_event(
        999,
        vec![spawn_record(
            id,
            [9.0, 9.0, 9.0],
            0.0,
            20,
            HostileKind::Nightwalker,
        )],
    );
    assert!(
        provider
            .commit(&staged(duplicate_spawn, 2, Vec::new()))
            .is_err(),
        "a spawn naming a live identity is a duplicate"
    );
    let unknown_state = state_event(
        101,
        vec![state_record(
            hostile_id(8),
            [5.0, 5.0, 5.0],
            [0.0, 0.0, 0.0],
            0.0,
            10,
            HostileKind::Nightwalker,
        )],
    );
    assert!(
        provider
            .commit(&staged(unknown_state, 2, Vec::new()))
            .is_err(),
        "a state for an identity the mirror never confirmed creates no entity"
    );

    assert_eq!(provider.mirror().revision().get(), 1);
    assert_eq!(provider.observations().len(), 1);
    assert_eq!(
        project_hostile(&fixture.view_of(&provider)).expect("projection still succeeds"),
        baseline,
        "the refused inputs leave the projection unchanged"
    );

    // A valid state advances the position and the health together.
    commit(
        &mut provider,
        state_event(
            101,
            vec![state_record(
                id,
                [2.0, 2.0, 3.0],
                [0.0, 0.0, 0.0],
                0.25,
                11,
                HostileKind::Nightwalker,
            )],
        ),
    );
    let advanced = project_hostile(&fixture.view_of(&provider)).expect("state projects");
    assert_eq!(advanced.len(), 2);
    assert_eq!(advanced[1].record().position(), Some([2.0, 2.0, 3.0]));
    assert_eq!(
        hostile_detail(&advanced[1]),
        Some((HOSTILE_KIND_NIGHTWALKER, 11))
    );

    commit(&mut provider, despawn_event(102, vec![id]));
    let removed = project_hostile(&fixture.view_of(&provider)).expect("despawn projects");
    assert_eq!(removed.len(), 3);
    assert_eq!(
        removed
            .last()
            .expect("remove record")
            .record()
            .header()
            .operation(),
        FamilyOperation::Remove
    );

    // A late state cannot resurrect the despawned identity and a duplicate
    // despawn is an orphan removal; both leave everything unchanged.
    let late_state = state_event(
        99,
        vec![state_record(
            id,
            [6.0, 6.0, 6.0],
            [0.0, 0.0, 0.0],
            0.0,
            1,
            HostileKind::Nightwalker,
        )],
    );
    assert!(
        provider.commit(&staged(late_state, 4, Vec::new())).is_err(),
        "a late state cannot resurrect a despawn"
    );
    assert!(
        provider
            .commit(&staged(despawn_event(103, vec![id]), 4, Vec::new()))
            .is_err(),
        "a duplicate despawn is an orphan removal"
    );
    assert_eq!(provider.mirror().revision().get(), 3);
    assert_eq!(provider.observations().len(), 3);
    assert!(provider.mirror().actors().hostiles().is_empty());
    assert_eq!(
        project_hostile(&fixture.view_of(&provider)).expect("projection still succeeds"),
        removed,
        "the refused late inputs leave the projection unchanged"
    );

    // Reuse after removal still commits.
    commit(
        &mut provider,
        spawn_event(
            105,
            vec![spawn_record(
                id,
                [2.0, 2.0, 2.0],
                0.0,
                18,
                HostileKind::Nightwalker,
            )],
        ),
    );
    let reused = project_hostile(&fixture.view_of(&provider)).expect("reuse projects");
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
