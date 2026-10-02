//! Survival (health, oxygen, hunger, saturation, armor) projection contract
//! tests.
//!
//! The table pins the accepted survival semantics against the landed mirror
//! provider: the damage→death→respawn chain is a sequence of confirmed
//! player-state publications — health descending under damage, the zero the
//! accepted schema admits as death, and the respawn publication restoring the
//! exact spawn scalars — and every confirmed publication projects exactly one
//! upsert carrying the complete published value. Health, hunger and armor
//! admit at most 20 and oxygen at most 300; each plus-one refuses typed at the
//! checked domain boundary before any observation exists, never clamped back
//! into range. Survival facts come from the authority's own publications
//! alone: a combat-hit observation never changes the projected scalars, local
//! input and prediction state never leaks into the records, and late,
//! duplicate, old-epoch and non-publication inputs leave the committed state
//! unchanged or reject whole without partial output.

use mornlea_client_core::contracts::{
    ClientError, ClientLimits, ConfirmedRevision, FamilyOperation, ObservationKey, SessionEpoch,
};
use mornlea_client_core::input::InputProjectionState;
use mornlea_client_core::prediction::PlayerProjectionState;
use mornlea_client_core::presentation::frame::WorldUiRecord;
use mornlea_client_core::presentation::world_ui::survival::project_survival;
use mornlea_client_core::presentation::{
    AcceptedObservation, AudioProjectionState, DiagnosticProjectionState, ErrorClassCounters,
    LifecycleProjectionState, MovementIntent, OrderedRecord, Pose, ProducerIdentity,
    ProjectionOrder, ProjectionView, QueueCounters, StableRecordKey, WorldTopic, WorldUiView,
};
use mornlea_client_core::session::ConfirmedMirror;
use mornlea_client_core::session::mirror::MirrorProvider;
use mornlea_domain::{
    ChunkPos, CombatHit, CombatTarget, ContainerClosed, ContainerKind, ContainerRef, Dimension,
    DomainError, Event, FiniteVec3, LookAngles, MiningState, MotionState, MotionStateParts,
    PlayerState, PlayerStateParts, SurvivalState, SurvivalStateParts, Weather, WorldState,
    WorldStateParts,
};
use mornlea_protocol::{KeepAlive, ServerPacket};

/// The epoch every fixture mirror and observation key carries.
const EPOCH: u64 = 21;

/// One neutral world record, the non-survival half of a player publication
/// the fixtures keep constant.
fn world_state() -> WorldState {
    WorldState::try_new(WorldStateParts {
        day_phase_offset: 1_000,
        world_time_ticks: 20_000,
        weather: Weather::Clear,
        season: mornlea_domain::Season::Summer,
        season_progress: 10,
        temperature: 5,
    })
    .expect("checked world fixture")
}

/// One checked survival record at the caller's exact scalars, refusing
/// nothing a real authority could publish inside the accepted bounds.
fn survival(
    health: u8,
    oxygen: u16,
    hunger: u8,
    saturation_zero: bool,
    armor_points: u8,
) -> SurvivalState {
    SurvivalState::try_new(SurvivalStateParts {
        health,
        oxygen,
        hunger,
        saturation_zero,
        armor_points,
    })
    .expect("checked survival fixture")
}

/// One overworld player publication at the caller's server tick and survival
/// scalars: ready, unreset, idle mining and a neutral pose, so the survival
/// record alone distinguishes two fixtures.
fn player_event(tick: u64, state: SurvivalState) -> Event {
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
        world: world_state(),
    }))
}

/// One combat-hit observation against the player: a confirmed damage fact the
/// survival projection must not fold into the projected scalars, because the
/// authority publishes survival only through the player-state publication.
fn combat_hit_event(tick: u64, damage: u8) -> Event {
    Event::CombatHit(
        CombatHit::try_new(tick, damage, CombatTarget::Player).expect("checked combat hit"),
    )
}

/// One filler observation of a foreign world-UI topic, used beside the
/// survival publication without contributing survival facts.
fn closed_event(generation: u32) -> Event {
    Event::ContainerClosed(ContainerClosed::new(
        ContainerRef::try_new(ChunkPos::new(-3, 7), ContainerKind::Chest, 5, generation)
            .expect("checked container reference"),
    ))
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
    /// mirror facts, pinning that no local owner feeds the survival records.
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

/// The survival payload of one record, refusing a foreign view tag: this
/// projection publishes the survival view alone.
fn survival_view(entry: &OrderedRecord<WorldUiRecord>) -> SurvivalState {
    match entry.record().view() {
        WorldUiView::Survival(state) => *state,
        other => panic!("survival record carries a foreign view tag: {other:?}"),
    }
}

/// The shared stable-key assertion: the survival topic tag with no event
/// identity, because survival is a singleton current-state topic whose
/// identity is the tag itself, never a fabricated observation or tick
/// identity.
fn assert_stable_key(entry: &OrderedRecord<WorldUiRecord>) {
    assert_eq!(
        *entry.stable_key(),
        StableRecordKey::World {
            topic: WorldTopic::Survival,
            identity: None,
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
        other => panic!("survival record is server-sourced: {other:?}"),
    }
}

/// `damage→death→respawn`: the chain is exactly four confirmed publications
/// — spawn at full health, damage lowering it, the zero-health death the
/// accepted schema admits, and the respawn publication restoring the exact
/// spawn scalars — and each projects one complete upsert in actual source
/// order, with the actual server ticks, the singleton stable key and the
/// confirmed observation order. The client never derives a transition: the
/// respawn value appears only when the authority publishes it.
#[test]
fn damage_death_respawn_chain_projects_exactly() {
    let spawn = survival(20, 300, 20, false, 15);
    let damaged = survival(12, 300, 18, false, 15);
    let death = survival(0, 300, 18, false, 15);
    let respawn = survival(20, 300, 20, false, 15);
    let mut provider = admitted_mirror(EPOCH);
    commit(&mut provider, player_event(101, spawn));
    commit(&mut provider, player_event(102, damaged));
    commit(&mut provider, player_event(103, death));
    commit(&mut provider, player_event(104, respawn));

    let fixture = view_fixture();
    let view = fixture.view_of(&provider);
    let records = project_survival(&view).expect("four survival records project");
    assert_eq!(records.len(), 4, "one record per confirmed publication");

    let projected: Vec<SurvivalState> = records.iter().map(survival_view).collect();
    assert_eq!(projected, vec![spawn, damaged, death, respawn]);
    assert_eq!(
        projected[3], spawn,
        "respawn restores the exact spawn scalars"
    );
    assert_ne!(
        projected[3],
        survival(0, 0, 0, false, 0),
        "respawn never substitutes defaults for the published value"
    );

    for (index, entry) in records.iter().enumerate() {
        assert_eq!(entry.record().header().operation(), FamilyOperation::Upsert);
        assert_eq!(
            entry.record().header().source_tick(),
            Some(101 + index as u64),
            "the actual server tick the publication carried"
        );
        assert_eq!(entry.record().header().epoch().get(), EPOCH);
        assert_eq!(
            entry.record().header().revision(),
            view.frame_revision(),
            "headers are rebased onto the coherent candidate revision"
        );
        assert_stable_key(entry);
        assert_confirmed_order(entry, 1 + index as u64, 0);
    }
    assert!(records[0].order() < records[1].order());
    assert!(records[1].order() < records[2].order());
    assert!(records[2].order() < records[3].order());
}

/// `health/hunger/armor up to 20, oxygen up to 300`, the refusing half: each
/// plus-one refuses typed at the checked domain boundary before any
/// observation exists — the exact boundary the Go HUD oracle pins, never
/// clamped back into range — while the maxima and the legal zeros project
/// exactly, including the saturation-zero presentation hint both ways.
#[test]
fn bounds_admit_maxima_and_zeros_and_reject_plus_one() {
    assert_eq!(
        SurvivalState::try_new(SurvivalStateParts {
            health: 21,
            oxygen: 300,
            hunger: 20,
            saturation_zero: false,
            armor_points: 20,
        }),
        Err(DomainError::InvalidSurvivalValue),
        "health plus-one at 21 rejects typed"
    );
    assert_eq!(
        SurvivalState::try_new(SurvivalStateParts {
            health: 20,
            oxygen: 300,
            hunger: 21,
            saturation_zero: false,
            armor_points: 20,
        }),
        Err(DomainError::InvalidSurvivalValue),
        "hunger plus-one at 21 rejects typed"
    );
    assert_eq!(
        SurvivalState::try_new(SurvivalStateParts {
            health: 20,
            oxygen: 300,
            hunger: 20,
            saturation_zero: false,
            armor_points: 21,
        }),
        Err(DomainError::InvalidSurvivalValue),
        "armor plus-one at 21 rejects typed"
    );
    assert_eq!(
        SurvivalState::try_new(SurvivalStateParts {
            health: 20,
            oxygen: 301,
            hunger: 20,
            saturation_zero: false,
            armor_points: 20,
        }),
        Err(DomainError::InvalidSurvivalValue),
        "oxygen plus-one at 301 rejects typed"
    );

    let maxima = survival(20, 300, 20, true, 20);
    let zeros = survival(0, 0, 0, true, 0);
    let mut provider = admitted_mirror(EPOCH);
    commit(&mut provider, player_event(7, maxima));
    commit(&mut provider, player_event(0, zeros));

    let fixture = view_fixture();
    let records = project_survival(&fixture.view_of(&provider)).expect("the bounds project");
    assert_eq!(records.len(), 2);
    let full = survival_view(&records[0]);
    assert_eq!(full, maxima);
    assert_eq!(full.health(), 20, "the health maximum");
    assert_eq!(full.hunger(), 20, "the hunger maximum");
    assert_eq!(full.armor_points(), 20, "the armor maximum");
    assert_eq!(full.oxygen(), 300, "the oxygen maximum");
    assert!(full.saturation_zero(), "the hint carries true verbatim");
    let empty = survival_view(&records[1]);
    assert_eq!(empty, zeros, "the legal zero values project exactly");
    assert_eq!(empty.health(), 0);
    assert_eq!(empty.oxygen(), 0);
    assert_eq!(empty.hunger(), 0);
    assert_eq!(empty.armor_points(), 0);
    assert_eq!(
        records[1].record().header().source_tick(),
        Some(0),
        "the legal zero server tick is preserved, never dropped"
    );
}

/// `stale update`: late, duplicate and ahead-of-sequence observation
/// identities are refused by the accepted mirror before this projection ever
/// sees them — the committed revision, the observation queue and the
/// projected output all stay unchanged — and an old-epoch observation rejects
/// the whole projection under a fresh frame epoch, never a partial prefix.
#[test]
fn stale_updates_leave_committed_state_unchanged() {
    let confirmed = survival(20, 300, 20, false, 15);
    let mut provider = admitted_mirror(EPOCH);
    commit(&mut provider, player_event(30, confirmed));

    let fixture = view_fixture();
    let baseline = project_survival(&fixture.view_of(&provider)).expect("baseline projects");
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
    let later = survival(18, 250, 16, false, 15);
    assert!(
        provider
            .commit(&staged(&provider, 1, player_event(31, later)))
            .is_err(),
        "a revision the mirror already issued is a duplicate"
    );
    assert!(
        provider
            .commit(&staged(&provider, 0, player_event(31, later)))
            .is_err(),
        "a backward revision is a malformed observation identity"
    );
    assert!(
        provider
            .commit(&staged(&provider, 3, player_event(31, later)))
            .is_err(),
        "an ahead-of-sequence revision is a malformed observation identity"
    );
    assert_eq!(provider.mirror().revision().get(), 1);
    assert_eq!(provider.observations().len(), 1);
    assert_eq!(
        project_survival(&fixture.view_of(&provider)).expect("projection still succeeds"),
        baseline,
        "the refused inputs leave the projection unchanged"
    );

    let next_epoch_value = EPOCH + 1;
    let reset_provider = admitted_mirror(next_epoch_value);
    assert!(
        project_survival(&fixture.view_of(&reset_provider))
            .expect("empty queue projects")
            .is_empty(),
        "a reset mirror carries no previous-session survival"
    );

    let next_epoch = SessionEpoch::try_new(next_epoch_value).expect("next epoch");
    let old_epoch_observation = provider.observations()[0].clone();
    assert_eq!(
        project_survival(&fixture.view(
            reset_provider.mirror(),
            &[old_epoch_observation],
            next_epoch,
            ConfirmedRevision::new(1)
        )),
        Err(ClientError::StaleEpoch),
        "an old-epoch observation never enters the new epoch's frame"
    );

    let mut fresh = admitted_mirror(next_epoch_value);
    let valid = commit(
        &mut fresh,
        player_event(2, survival(20, 300, 20, false, 15)),
    );
    assert_eq!(valid.get(), 1);
    let stale = provider.observations()[0].clone();
    assert_ne!(
        stale.key().epoch().get(),
        next_epoch_value,
        "the stale observation belongs to the reset-away epoch"
    );
    let mixed = vec![fresh.observations()[0].clone(), stale];
    assert_eq!(
        project_survival(&fixture.view(
            fresh.mirror(),
            &mixed,
            next_epoch,
            ConfirmedRevision::new(2)
        )),
        Err(ClientError::StaleEpoch),
        "a mixed queue rejects whole, never a partial prefix"
    );
}

/// `no client authority`: the survival scalars come from the authority's own
/// confirmed publications alone — a confirmed combat hit against the player
/// never changes them, foreign topics are ignored wholesale, differing local
/// input and prediction owners project identical records, and a packet that
/// is not a checked event publication rejects the whole projection.
#[test]
fn survival_values_come_from_server_confirmations_only() {
    let published = survival(20, 300, 20, false, 15);
    let mut provider = admitted_mirror(EPOCH);
    commit(&mut provider, player_event(40, published));
    commit(&mut provider, combat_hit_event(41, 5));
    commit(&mut provider, closed_event(1));

    let fixture = view_fixture();
    let view = fixture.view_of(&provider);
    let records = project_survival(&view).expect("only the publication projects");
    assert_eq!(
        records.len(),
        1,
        "a combat hit and a foreign topic contribute nothing"
    );
    assert_eq!(survival_view(&records[0]), published);
    assert_eq!(
        records[0].record().header().source_tick(),
        Some(40),
        "the hit's tick never shifts onto the survival record"
    );
    assert_eq!(
        project_survival(&view).expect("repeat projection"),
        records,
        "the projection is pure: no local state accumulates between calls"
    );

    let divergent = ViewFixture::with_divergent_local_state(
        SessionEpoch::try_new(EPOCH).expect("epoch"),
        ConfirmedRevision::new(1),
    );
    assert_eq!(
        project_survival(&divergent.view_of(&provider)).expect("local owners stay irrelevant"),
        records,
        "local input, prediction and lifecycle owners never feed the survival records"
    );

    let epoch = SessionEpoch::try_new(EPOCH).expect("epoch");
    let valid = provider.observations()[0].clone();
    let non_event = AcceptedObservation::try_new(
        ObservationKey::try_new(epoch, ConfirmedRevision::new(4), 0).expect("key"),
        None,
        ServerPacket::KeepAlive(KeepAlive::new(1).expect("checked keep-alive token")),
        Vec::new(),
    )
    .expect("staged with a non-publication packet");
    assert_eq!(
        project_survival(&fixture.view(
            provider.mirror(),
            &[valid, non_event],
            epoch,
            ConfirmedRevision::new(4)
        )),
        Err(ClientError::InvalidInput),
        "a packet that is not an event publication rejects without a partial prefix"
    );
}
