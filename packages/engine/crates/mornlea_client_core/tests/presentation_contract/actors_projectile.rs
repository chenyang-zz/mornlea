//! Projectile actor projection contract tests.
//!
//! The table pins the accepted projectile semantics against the landed mirror
//! provider, following the measured Go projectile mirror transcript: a launch
//! is one upsert carrying the position, the velocity and the archetype exactly
//! and no orientation; a state batch is one upsert per record carrying the
//! position alone — the launch velocity is never re-estimated or re-attributed
//! from a position sample, and no archetype is republished; a despawn is a
//! remove-only record naming no cause. A `CombatHit` observation names a
//! target category and cannot even name a projectile identity, so it
//! contributes no hit marker and no projectile link; an old-epoch observation
//! rejects the whole projection; and duplicate spawn, duplicate despawn and
//! late unknown-identity inputs are refused by the real mirror, leaving the
//! committed revision, the observation queue and the projected output
//! unchanged. Order keys are the actual observation keys with packet record
//! ordinals; equal or absent source ticks never reconstruct order.

use mornlea_client_core::contracts::{
    ClientError, ClientLimits, ConfirmedRevision, FamilyOperation, ObservationKey, SessionEpoch,
};
use mornlea_client_core::input::InputProjectionState;
use mornlea_client_core::prediction::PlayerProjectionState;
use mornlea_client_core::presentation::actors::projectile::project_projectile;
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
    CombatHit, CombatTarget, Dimension, Event, FiniteVec3, ProjectileDespawn,
    ProjectileDespawnParts, ProjectileId, ProjectileKind, ProjectileSpawn, ProjectileSpawnParts,
    ProjectileSpawnRecord, ProjectileSpawnRecordParts, ProjectileState, ProjectileStateParts,
    ProjectileStateRecord, ProjectileStateRecordParts,
};
use mornlea_protocol::{PROJECTILE_KIND_ARROW, PROJECTILE_KIND_SHARD, ServerPacket};

/// The epoch every fixture mirror and observation key carries.
const EPOCH: u64 = 7;

fn projectile_id(value: u64) -> ProjectileId {
    ProjectileId::try_new(value).expect("checked projectile identity")
}

fn finite_pos(components: [f32; 3]) -> FiniteVec3 {
    FiniteVec3::try_new(components).expect("finite position")
}

fn spawn_record(
    id: ProjectileId,
    kind: ProjectileKind,
    dimension: Dimension,
    position: [f32; 3],
    velocity: [f32; 3],
) -> ProjectileSpawnRecord {
    ProjectileSpawnRecord::new(ProjectileSpawnRecordParts {
        id,
        kind,
        dimension,
        position: finite_pos(position),
        velocity: finite_pos(velocity),
    })
}

fn spawn_event(tick: u64, records: Vec<ProjectileSpawnRecord>) -> Event {
    Event::ProjectileSpawn(
        ProjectileSpawn::try_new(ProjectileSpawnParts {
            server_tick: tick,
            spawns: records.into_boxed_slice(),
        })
        .expect("checked projectile spawn batch"),
    )
}

fn state_record(id: ProjectileId, position: [f32; 3]) -> ProjectileStateRecord {
    ProjectileStateRecord::new(ProjectileStateRecordParts {
        id,
        position: finite_pos(position),
    })
}

fn state_event(tick: u64, records: Vec<ProjectileStateRecord>) -> Event {
    Event::ProjectileState(
        ProjectileState::try_new(ProjectileStateParts {
            server_tick: tick,
            states: records.into_boxed_slice(),
        })
        .expect("checked projectile state batch"),
    )
}

fn despawn_event(tick: u64, ids: Vec<ProjectileId>) -> Event {
    Event::ProjectileDespawn(
        ProjectileDespawn::try_new(ProjectileDespawnParts {
            server_tick: tick,
            ids: ids.into_boxed_slice(),
        })
        .expect("checked projectile despawn batch"),
    )
}

/// A combat hit against a hostile-kind target: the publication names a target
/// category alone and the closed target union has no projectile variant at
/// all, so no projectile identity or link can ever be drawn from it.
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
        other => panic!("projectile record is server-sourced: {other:?}"),
    }
}

/// The projectile archetype detail, refusing a foreign detail tag: `None`
/// means the record carries no detail at all, which is exactly the shape a
/// position-only state or a causeless despawn must keep.
fn archetype_detail(entry: &OrderedRecord<ActorRecord>) -> Option<u8> {
    match entry.record().detail() {
        None => None,
        Some(ActorDetail::Projectile { archetype }) => *archetype,
        Some(other) => panic!("projectile record carries a foreign detail tag: {other:?}"),
    }
}

/// Asserts every record of one projection keeps the launch's orientation-free
/// shape: the projectile publications carry no yaw and no pitch, because the
/// client derives orientation from the launch velocity, so none is invented.
fn assert_no_angles(records: &[OrderedRecord<ActorRecord>]) {
    for entry in records {
        assert_eq!(entry.record().yaw(), None, "projectiles carry no yaw");
        assert_eq!(entry.record().pitch(), None, "projectiles carry no pitch");
    }
}

/// `launch`: one upsert per spawn record carrying the typed identity, the
/// actual spawn dimension, the exact finite position and velocity widened
/// from f32, and the wire archetype detail — the launch values and nothing
/// else, exactly as the measured transcript's first row publishes them.
#[test]
fn launch_carries_position_velocity_and_archetype_exactly() {
    let mut provider = admitted_mirror();
    let id = projectile_id(7);
    commit(
        &mut provider,
        spawn_event(
            100,
            vec![spawn_record(
                id,
                ProjectileKind::Shard,
                Dimension::OVERWORLD,
                [1.0, 2.0, 3.0],
                [0.0, -0.5, -1.0],
            )],
        ),
    );

    let fixture = view_fixture();
    let records =
        project_projectile(&fixture.view_of(&provider)).expect("launch projects one record");
    assert_eq!(records.len(), 1);

    let entry = &records[0];
    assert_eq!(entry.record().kind(), ActorKind::Projectile);
    assert_eq!(*entry.record().id(), ActorId::Projectile(id));
    assert_eq!(
        *entry.record().dimension(),
        ActorDimension::Known(Dimension::OVERWORLD)
    );
    assert_eq!(entry.record().position(), Some([1.0, 2.0, 3.0]));
    assert_eq!(entry.record().velocity(), Some([0.0, -0.5, -1.0]));
    assert_eq!(archetype_detail(entry), Some(PROJECTILE_KIND_SHARD));
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
            kind: ActorKind::Projectile,
            dimension: ActorDimension::Known(Dimension::OVERWORLD),
            id: ActorId::Projectile(id),
        }
    );
    assert_confirmed_order(entry, 1, 0);
    assert_no_angles(&records);
}

/// `state`: one upsert per batch entry in published packet order carrying the
/// position alone. The launch velocity is never re-attributed — the legacy
/// pilot re-estimates it by position diffing, and the packet-02 contract
/// forbids exactly that — and no archetype is republished because the state
/// record has no additional actor fields. The dimension comes from the
/// prevalidated identity resolution, and every sample keeps its own exact
/// position and source tick; the projection never merges, interpolates or
/// collapses to the newest sample.
#[test]
fn state_updates_carry_position_only_without_velocity_re_attribution() {
    let mut provider = admitted_mirror();
    let shard = projectile_id(7);
    let arrow = projectile_id(9);
    commit(
        &mut provider,
        spawn_event(
            100,
            vec![
                spawn_record(
                    shard,
                    ProjectileKind::Shard,
                    Dimension::OVERWORLD,
                    [1.0, 2.0, 3.0],
                    [0.0, -0.5, -1.0],
                ),
                spawn_record(
                    arrow,
                    ProjectileKind::Arrow,
                    Dimension::DEPTHS,
                    [0.0, 8.0, 0.0],
                    [0.0, 0.0, -2.0],
                ),
            ],
        ),
    );
    commit(
        &mut provider,
        state_event(
            101,
            vec![
                state_record(shard, [1.0, 1.0, 1.0]),
                state_record(arrow, [0.0, 8.0, -2.0]),
            ],
        ),
    );
    // A zero-displacement sample still publishes position only: the legacy
    // pilot would keep its last velocity estimate, this contract re-publishes
    // nothing the wire does not carry.
    commit(
        &mut provider,
        state_event(102, vec![state_record(shard, [1.0, 1.0, 1.0])]),
    );

    let fixture = view_fixture();
    let records =
        project_projectile(&fixture.view_of(&provider)).expect("state batches project records");
    assert_eq!(records.len(), 5, "two launches, two states, one state");

    let expected = [
        (
            shard,
            Some([1.0, 1.0, 1.0]),
            Dimension::OVERWORLD,
            101u64,
            2u64,
            0u32,
        ),
        (arrow, Some([0.0, 8.0, -2.0]), Dimension::DEPTHS, 101, 2, 1),
        (
            shard,
            Some([1.0, 1.0, 1.0]),
            Dimension::OVERWORLD,
            102,
            3,
            0,
        ),
    ];
    for (entry, (id, position, dimension, tick, revision, ordinal)) in
        records.iter().skip(2).zip(&expected)
    {
        assert_eq!(*entry.record().id(), ActorId::Projectile(*id));
        assert_eq!(
            *entry.record().dimension(),
            ActorDimension::Known(*dimension),
            "the dimensionless state resolves against the confirmed identity"
        );
        assert_eq!(entry.record().position(), *position);
        assert_eq!(
            entry.record().velocity(),
            None,
            "a state update carries position only; the launch velocity is never re-attributed"
        );
        assert_eq!(
            archetype_detail(entry),
            None,
            "a state record republishes no archetype"
        );
        assert_eq!(entry.record().header().source_tick(), Some(*tick));
        assert_eq!(entry.record().header().operation(), FamilyOperation::Upsert);
        assert_confirmed_order(entry, *revision, *ordinal);
    }
    assert!(
        records[2].order() < records[3].order(),
        "one batch's records keep the packet record ordinal order"
    );
    assert!(
        records
            .windows(2)
            .all(|pair| pair[0].order() < pair[1].order()),
        "records keep actual source order across the batches"
    );
    assert_no_angles(&records);
}

/// A `CombatHit` publishes no projectile link: the closed target union has no
/// projectile variant, so the observation contributes no record, marks no hit
/// on any live projectile and draws no cause onto a despawn — the despawn
/// remains a removal alone with no detail, and a hit after the removal
/// publishes nothing at all.
#[test]
fn combat_hit_publishes_no_projectile_link_or_hit_marker() {
    let mut provider = admitted_mirror();
    let id = projectile_id(7);
    commit(
        &mut provider,
        spawn_event(
            100,
            vec![spawn_record(
                id,
                ProjectileKind::Arrow,
                Dimension::OVERWORLD,
                [0.0, 64.0, 0.0],
                [0.0, -1.0, -2.0],
            )],
        ),
    );
    commit(
        &mut provider,
        state_event(101, vec![state_record(id, [0.0, 63.0, -2.0])]),
    );
    commit(&mut provider, combat_hit_event(102, 2));

    let fixture = view_fixture();
    let records = project_projectile(&fixture.view_of(&provider)).expect("projection succeeds");
    assert_eq!(
        records.len(),
        2,
        "the combat hit publishes no projectile record"
    );
    for entry in &records {
        assert_eq!(*entry.record().id(), ActorId::Projectile(id));
    }
    // The published values stay the publications' own: no hit marker, no
    // attacker and no projectile-to-projectile link exists on any record.
    assert_eq!(records[0].record().position(), Some([0.0, 64.0, 0.0]));
    assert_eq!(records[0].record().velocity(), Some([0.0, -1.0, -2.0]));
    assert_eq!(archetype_detail(&records[0]), Some(PROJECTILE_KIND_ARROW));
    assert_eq!(records[1].record().position(), Some([0.0, 63.0, -2.0]));
    assert_eq!(records[1].record().velocity(), None);
    assert_eq!(archetype_detail(&records[1]), None);

    commit(&mut provider, despawn_event(103, vec![id]));
    // A hit after the removal still names no projectile identity.
    commit(&mut provider, combat_hit_event(104, 3));
    let after = project_projectile(&fixture.view_of(&provider)).expect("projection succeeds");
    assert_eq!(after.len(), 3);
    let remove = &after[2];
    assert_eq!(
        remove.record().header().operation(),
        FamilyOperation::Remove
    );
    assert_eq!(remove.record().header().source_tick(), Some(103));
    assert_eq!(remove.record().position(), None);
    assert_eq!(remove.record().velocity(), None);
    assert!(
        remove.record().detail().is_none(),
        "a projectile despawn names no cause and invents none"
    );
    assert_eq!(
        *remove.stable_key(),
        StableRecordKey::Actor {
            kind: ActorKind::Projectile,
            dimension: ActorDimension::Known(Dimension::OVERWORLD),
            id: ActorId::Projectile(id),
        }
    );
    assert_confirmed_order(remove, 4, 0);
    assert_no_angles(&after);
}

/// An observation key from another epoch rejects the whole projection with no
/// partial output — even when a valid projectile observation precedes it —
/// and the same identity may launch again after its despawn, keeping its
/// stable key through the reuse.
#[test]
fn old_epoch_rejects_whole_projection_without_partial_output() {
    let epoch = SessionEpoch::try_new(EPOCH).expect("epoch");
    let fixture = view_fixture();
    let mut provider = admitted_mirror();
    let id = projectile_id(7);
    commit(
        &mut provider,
        spawn_event(
            10,
            vec![spawn_record(
                id,
                ProjectileKind::Shard,
                Dimension::OVERWORLD,
                [1.0, 64.0, 1.0],
                [0.0, -1.0, 0.0],
            )],
        ),
    );
    commit(
        &mut provider,
        state_event(11, vec![state_record(id, [1.0, 63.0, 1.0])]),
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
        packet(state_event(99, vec![state_record(id, [9.0, 64.0, 9.0])])),
        Vec::new(),
    )
    .expect("staged in another epoch");
    // The valid committed observation precedes the stale one: the rejection
    // still swallows the whole candidate, never a partial prefix.
    let mut queue: Vec<AcceptedObservation> = provider.observations().to_vec();
    queue.push(stale_epoch);
    assert_eq!(
        project_projectile(&fixture.view(
            provider.mirror(),
            &queue,
            epoch,
            ConfirmedRevision::new(4)
        )),
        Err(ClientError::StaleEpoch),
        "an observation from another epoch rejects without partial output"
    );

    let records = project_projectile(&fixture.view_of(&provider)).expect("lifecycle projects");
    assert_eq!(records.len(), 3);
    let stable = StableRecordKey::Actor {
        kind: ActorKind::Projectile,
        dimension: ActorDimension::Known(Dimension::OVERWORLD),
        id: ActorId::Projectile(id),
    };
    for entry in &records {
        assert_eq!(*entry.stable_key(), stable);
    }

    // Reuse: the same identity launches again after its removal.
    commit(
        &mut provider,
        spawn_event(
            13,
            vec![spawn_record(
                id,
                ProjectileKind::Arrow,
                Dimension::OVERWORLD,
                [2.0, 64.0, 2.0],
                [1.0, 0.0, 0.0],
            )],
        ),
    );
    let reused = project_projectile(&fixture.view_of(&provider)).expect("reuse projects");
    assert_eq!(reused.len(), 4);
    let respawn = reused.last().expect("relaunch record");
    assert_eq!(
        respawn.record().header().operation(),
        FamilyOperation::Upsert
    );
    assert_eq!(
        *respawn.stable_key(),
        stable,
        "reuse keeps the typed identity"
    );
    assert_eq!(archetype_detail(respawn), Some(PROJECTILE_KIND_ARROW));
    assert_no_angles(&reused);
}

/// The measured transcript's unchanged-state rows through the real mirror: a
/// duplicate launch of a live identity, a state or despawn for an identity
/// the mirror never confirmed, a late state that cannot resurrect a despawn
/// and a duplicate despawn all refuse to commit, leaving the revision, the
/// observation queue and the projected output exactly as they were, while a
/// valid state advances position only and a fresh epoch starts empty and
/// works again.
#[test]
fn duplicate_and_late_inputs_leave_committed_state_unchanged() {
    let mut provider = admitted_mirror();
    let id = projectile_id(7);
    commit(
        &mut provider,
        spawn_event(
            100,
            vec![spawn_record(
                id,
                ProjectileKind::Shard,
                Dimension::OVERWORLD,
                [1.0, 2.0, 3.0],
                [0.0, -0.5, -1.0],
            )],
        ),
    );

    let fixture = view_fixture();
    let baseline = project_projectile(&fixture.view_of(&provider)).expect("baseline projects");
    assert_eq!(baseline.len(), 1);
    assert_eq!(baseline[0].record().position(), Some([1.0, 2.0, 3.0]));
    assert_eq!(baseline[0].record().velocity(), Some([0.0, -0.5, -1.0]));
    assert_eq!(archetype_detail(&baseline[0]), Some(PROJECTILE_KIND_SHARD));

    let duplicate_launch = spawn_event(
        999,
        vec![spawn_record(
            id,
            ProjectileKind::Arrow,
            Dimension::OVERWORLD,
            [9.0, 9.0, 9.0],
            [0.0, 30.0, 0.0],
        )],
    );
    assert!(
        provider
            .commit(&staged(duplicate_launch, 2, Vec::new()))
            .is_err(),
        "a launch naming a live identity is a duplicate"
    );
    let unknown_state = state_event(101, vec![state_record(projectile_id(8), [5.0, 5.0, 5.0])]);
    assert!(
        provider
            .commit(&staged(unknown_state, 2, Vec::new()))
            .is_err(),
        "a state for an identity the mirror never confirmed creates no entity"
    );
    let unknown_despawn = despawn_event(103, vec![projectile_id(8)]);
    assert!(
        provider
            .commit(&staged(unknown_despawn, 2, Vec::new()))
            .is_err(),
        "a despawn for an unknown identity is an orphan removal"
    );

    assert_eq!(provider.mirror().revision().get(), 1);
    assert_eq!(provider.observations().len(), 1);
    assert_eq!(
        project_projectile(&fixture.view_of(&provider)).expect("projection still succeeds"),
        baseline,
        "the refused inputs leave the projection unchanged"
    );

    // A valid state advances the position alone; the launch velocity is not
    // re-estimated from the sample the way the legacy mirror diffed it.
    commit(
        &mut provider,
        state_event(101, vec![state_record(id, [1.0, 1.0, 1.0])]),
    );
    let advanced = project_projectile(&fixture.view_of(&provider)).expect("state projects");
    assert_eq!(advanced.len(), 2);
    assert_eq!(advanced[1].record().position(), Some([1.0, 1.0, 1.0]));
    assert_eq!(advanced[1].record().velocity(), None);

    commit(&mut provider, despawn_event(103, vec![id]));
    let removed = project_projectile(&fixture.view_of(&provider)).expect("despawn projects");
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
    let late_state = state_event(99, vec![state_record(id, [6.0, 6.0, 6.0])]);
    assert!(
        provider.commit(&staged(late_state, 4, Vec::new())).is_err(),
        "a late state cannot resurrect a despawn"
    );
    assert!(
        provider
            .commit(&staged(despawn_event(104, vec![id]), 4, Vec::new()))
            .is_err(),
        "a duplicate despawn is an orphan removal"
    );
    assert_eq!(provider.mirror().revision().get(), 3);
    assert_eq!(provider.observations().len(), 3);
    assert!(provider.mirror().actors().projectiles().is_empty());
    assert_eq!(
        project_projectile(&fixture.view_of(&provider)).expect("projection still succeeds"),
        removed,
        "the refused late inputs leave the projection unchanged"
    );

    // The reset row of the transcript: a fresh epoch's mirror starts empty
    // and accepts a launch again.
    let mut fresh = MirrorProvider::new(
        SessionEpoch::try_new(EPOCH + 1).expect("next epoch"),
        ClientLimits::try_new().expect("limits"),
    )
    .expect("fresh mirror");
    fresh.admit().expect("admitted session");
    let relaunched = AcceptedObservation::try_new(
        ObservationKey::try_new(
            SessionEpoch::try_new(EPOCH + 1).expect("next epoch"),
            ConfirmedRevision::new(1),
            0,
        )
        .expect("staged key"),
        None,
        packet(spawn_event(
            2,
            vec![spawn_record(
                projectile_id(8),
                ProjectileKind::Arrow,
                Dimension::OVERWORLD,
                [2.0, 1.0, 1.0],
                [1.0, 0.0, 0.0],
            )],
        )),
        Vec::new(),
    )
    .expect("staged");
    fresh.commit(&relaunched).expect("fresh epoch commits");
    let fresh_epoch = SessionEpoch::try_new(EPOCH + 1).expect("next epoch");
    let records = project_projectile(&fixture.view(
        fresh.mirror(),
        fresh.observations(),
        fresh_epoch,
        ConfirmedRevision::new(2),
    ))
    .expect("fresh epoch projects");
    assert_eq!(records.len(), 1);
    assert_eq!(
        records[0].record().header().epoch().get(),
        EPOCH + 1,
        "the fresh epoch's records carry their own epoch"
    );
}

/// Malformed prevalidation doubles reject the whole projection with no
/// partial output: a dimensionless state or despawn without its resolved
/// identity, a resolution naming another identity, and a resolution that is
/// ambiguous between two dimensions.
#[test]
fn invalid_identity_doubles_reject_without_partial_output() {
    let epoch = SessionEpoch::try_new(EPOCH).expect("epoch");
    let fixture = view_fixture();
    let provider = admitted_mirror();
    let id = projectile_id(7);
    let other = projectile_id(8);

    let missing = staged(despawn_event(10, vec![id]), 1, Vec::new());
    assert_eq!(
        project_projectile(&fixture.view(
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
                ActorKind::Projectile,
                ActorId::Projectile(other),
                Dimension::OVERWORLD,
            )
            .expect("resolved other identity"),
        ],
    );
    assert_eq!(
        project_projectile(&fixture.view(
            provider.mirror(),
            &[mismatched],
            epoch,
            ConfirmedRevision::new(1)
        )),
        Err(ClientError::InvalidInput),
        "a resolution naming another identity rejects"
    );

    let ambiguous = staged(
        state_event(10, vec![state_record(id, [1.0, 1.0, 1.0])]),
        1,
        vec![
            ResolvedActor::try_new(
                ActorKind::Projectile,
                ActorId::Projectile(id),
                Dimension::OVERWORLD,
            )
            .expect("resolved identity"),
            ResolvedActor::try_new(
                ActorKind::Projectile,
                ActorId::Projectile(id),
                Dimension::DEPTHS,
            )
            .expect("resolved identity again"),
        ],
    );
    assert_eq!(
        project_projectile(&fixture.view(
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
        project_projectile(&fixture.view_of(&provider))
            .expect("empty queue projects")
            .is_empty(),
        "an empty committed queue projects no records"
    );
}
