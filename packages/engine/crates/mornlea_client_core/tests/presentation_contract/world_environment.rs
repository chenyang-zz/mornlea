//! Environment (day, weather, season, temperature) projection contract
//! tests.
//!
//! The table pins the accepted environment semantics against the landed
//! mirror provider: one confirmed player-state publication projects exactly
//! one upsert carrying the complete published world scalars — the absolute
//! world time beside the display day-phase offset, the weather and season
//! kinds, the season progress and the temperature — with every boundary
//! value admitted exactly and each plus-one refused at the checked domain
//! boundary before any observation exists. A restore publication returns the
//! exact prior values, never defaults. The optional source tick is preserved
//! verbatim from the retained observation, including the legal zero, and the
//! order never comes from equal ticks but from the actual observation keys.
//! Late, duplicate and old-epoch inputs leave the committed state
//! unchanged, and a queue that mixes a valid observation with a stale one
//! rejects whole without partial output.

use mornlea_client_core::contracts::{
    ClientError, ClientLimits, ConfirmedRevision, FamilyOperation, ObservationKey, SessionEpoch,
};
use mornlea_client_core::input::InputProjectionState;
use mornlea_client_core::prediction::PlayerProjectionState;
use mornlea_client_core::presentation::frame::WorldUiRecord;
use mornlea_client_core::presentation::world_ui::environment::project_environment;
use mornlea_client_core::presentation::{
    AcceptedObservation, AudioProjectionState, DiagnosticProjectionState, ErrorClassCounters,
    LifecycleProjectionState, MovementIntent, OrderedRecord, Pose, ProducerIdentity,
    ProjectionOrder, ProjectionView, QueueCounters, StableRecordKey, WorldTopic, WorldUiView,
};
use mornlea_client_core::session::ConfirmedMirror;
use mornlea_client_core::session::mirror::MirrorProvider;
use mornlea_domain::{
    ChunkPos, ContainerClosed, ContainerKind, ContainerRef, Dimension, DomainError, Event,
    FiniteVec3, LookAngles, MiningState, MotionState, MotionStateParts, PlayerState,
    PlayerStateParts, Season, SurvivalState, SurvivalStateParts, Weather, WorldState,
    WorldStateParts,
};
use mornlea_protocol::{KeepAlive, ServerPacket};

/// The epoch every fixture mirror and observation key carries.
const EPOCH: u64 = 21;

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

/// One overworld player publication at the caller's server tick and world
/// scalars: ready, unreset, idle mining and a neutral pose, so the world
/// record alone distinguishes two fixtures.
fn player_event(tick: u64, world: WorldState) -> Event {
    player_event_at(tick, world, false)
}

/// One player publication whose reset flag the caller names, the shape an
/// authoritative restore replays with.
fn player_event_at(tick: u64, world: WorldState, reset: bool) -> Event {
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
        reset,
        mining: MiningState::Idle,
        survival: survival(),
        world,
    }))
}

/// One filler observation of a foreign world-UI topic, used beside the
/// environment publication without contributing environment facts.
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

/// The environment payload of one record, refusing a foreign view tag: this
/// projection publishes the environment view alone.
fn environment_view(entry: &OrderedRecord<WorldUiRecord>) -> WorldState {
    match entry.record().view() {
        WorldUiView::Environment(state) => *state,
        other => panic!("environment record carries a foreign view tag: {other:?}"),
    }
}

/// The shared stable-key assertion: the environment topic tag with no event
/// identity, because the environment is a singleton topic whose identity is
/// the tag itself, never a fabricated observation or tick identity.
fn assert_stable_key(entry: &OrderedRecord<WorldUiRecord>) {
    assert_eq!(
        *entry.stable_key(),
        StableRecordKey::World {
            topic: WorldTopic::Environment,
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
        other => panic!("environment record is server-sourced: {other:?}"),
    }
}

/// `day/weather/season/temperature boundary`: the Go oracle's boundary
/// snapshot — the last in-cycle day offset, the highest weather and season
/// kinds, the full-range season progress and a sub-zero temperature beside
/// the absolute world time — projects exactly one upsert with every scalar
/// preserved, the actual source tick, the singleton stable key and the
/// confirmed observation order.
#[test]
fn boundary_values_project_exactly() {
    let boundary = world_state(23_999, 48_000, Weather::Thunder, Season::Winter, 255, -40);
    let mut provider = admitted_mirror(EPOCH);
    commit(&mut provider, player_event(17, boundary));

    let fixture = view_fixture();
    let view = fixture.view_of(&provider);
    let records = project_environment(&view).expect("one environment record projects");
    assert_eq!(records.len(), 1);

    let entry = &records[0];
    let projected = environment_view(entry);
    assert_eq!(projected, boundary, "the complete published scalars");
    assert_eq!(
        projected.day_phase_offset(),
        23_999,
        "the last in-cycle offset"
    );
    assert_eq!(projected.world_time_ticks(), 48_000);
    assert_eq!(
        projected.weather(),
        Weather::Thunder,
        "the highest weather kind"
    );
    assert_eq!(
        projected.season(),
        Season::Winter,
        "the highest season kind"
    );
    assert_eq!(projected.season_progress(), 255, "the full-range progress");
    assert_eq!(projected.temperature(), -40);
    assert_eq!(entry.record().header().operation(), FamilyOperation::Upsert);
    assert_eq!(
        entry.record().header().source_tick(),
        Some(17),
        "the actual server tick the publication carried"
    );
    assert_eq!(entry.record().header().epoch().get(), EPOCH);
    assert_eq!(
        entry.record().header().revision(),
        view.frame_revision(),
        "headers are rebased onto the coherent candidate revision"
    );
    assert_stable_key(entry);
    assert_confirmed_order(entry, 1, 0);
}

/// `day/weather/season/temperature boundary`, the refusing half: each plus-one
/// refuses at the checked domain boundary before any observation exists, and
/// the extremes the accepted schema admits — an offset of zero, the full i8
/// temperature range on both ends — project exactly.
#[test]
fn domain_bounds_admit_extremes_and_reject_plus_one() {
    let day_length = 24_000u16;
    assert_eq!(
        WorldState::try_new(WorldStateParts {
            day_phase_offset: day_length,
            world_time_ticks: 0,
            weather: Weather::Clear,
            season: Season::Spring,
            season_progress: 0,
            temperature: 0,
        }),
        Err(DomainError::InvalidDayPhaseOffset),
        "an offset at the cycle length no longer names a phase inside it"
    );
    assert_eq!(
        Weather::try_new(3),
        Err(DomainError::InvalidWeather),
        "one above the highest published weather kind"
    );
    assert_eq!(
        Season::try_new(4),
        Err(DomainError::InvalidSeason),
        "one above the highest published season"
    );
    assert_eq!(Weather::try_new(2), Ok(Weather::Thunder));
    assert_eq!(Season::try_new(3), Ok(Season::Winter));

    let mut provider = admitted_mirror(EPOCH);
    let zero_offset = world_state(0, 0, Weather::Clear, Season::Spring, 0, i8::MIN);
    let hottest = world_state(1, u64::MAX, Weather::Clear, Season::Spring, 0, i8::MAX);
    commit(&mut provider, player_event(1, zero_offset));
    commit(&mut provider, player_event(2, hottest));

    let fixture = view_fixture();
    let records = project_environment(&fixture.view_of(&provider)).expect("the extremes project");
    assert_eq!(records.len(), 2);
    assert_eq!(environment_view(&records[0]), zero_offset);
    assert_eq!(environment_view(&records[0]).temperature(), i8::MIN);
    assert_eq!(environment_view(&records[1]), hottest);
    assert_eq!(environment_view(&records[1]).temperature(), i8::MAX);
    assert_eq!(environment_view(&records[1]).world_time_ticks(), u64::MAX);
}

/// `restore`: after the environment changes, a publication that restores the
/// exact prior scalars — the authoritative reset replay shape — projects the
/// exact prior values, never defaults and never the displaced values, with
/// its own observation identity beside the earlier records.
#[test]
fn restore_returns_exact_prior_values_never_defaults() {
    let prior = world_state(1_000, 20_000, Weather::Clear, Season::Summer, 10, 5);
    let changed = world_state(2_000, 30_000, Weather::Rain, Season::Autumn, 200, -10);
    let mut provider = admitted_mirror(EPOCH);
    commit(&mut provider, player_event(50, prior));
    commit(&mut provider, player_event(60, changed));
    commit(&mut provider, player_event_at(70, prior, true));

    let fixture = view_fixture();
    let records = project_environment(&fixture.view_of(&provider)).expect("three records project");
    assert_eq!(records.len(), 3, "one record per confirmed publication");

    let restored = environment_view(&records[2]);
    assert_eq!(
        restored, prior,
        "a restore returns the exact prior values, never defaults"
    );
    assert_ne!(
        restored,
        WorldState::try_new(WorldStateParts {
            day_phase_offset: 0,
            world_time_ticks: 0,
            weather: Weather::Clear,
            season: Season::Spring,
            season_progress: 0,
            temperature: 0,
        })
        .expect("the zero world state")
    );
    assert_ne!(restored, changed, "the displaced values do not survive");
    assert_eq!(environment_view(&records[0]), prior);
    assert_eq!(environment_view(&records[1]), changed);
    assert!(records[0].order() < records[1].order());
    assert!(records[1].order() < records[2].order());
    for entry in &records {
        assert_stable_key(entry);
    }
    assert_confirmed_order(&records[0], 1, 0);
    assert_confirmed_order(&records[1], 2, 0);
    assert_confirmed_order(&records[2], 3, 0);
    assert_eq!(records[2].record().header().source_tick(), Some(70));
}

/// `optional source tick preserved`: the retained observation tick passes
/// through verbatim — including the legal zero the accepted packet admits —
/// equal ticks never decide order (the observation keys alone do), and an
/// observation the queue retained without a tick projects none, because no
/// tick is ever invented for the environment topic.
#[test]
fn optional_source_tick_preserved_verbatim() {
    let morning = world_state(0, 100, Weather::Clear, Season::Spring, 0, 0);
    let evening = world_state(12_000, 100, Weather::Clear, Season::Spring, 0, 0);
    let mut provider = admitted_mirror(EPOCH);
    commit(&mut provider, player_event(0, morning));
    commit(&mut provider, player_event(0, evening));

    let fixture = view_fixture();
    let records = project_environment(&fixture.view_of(&provider)).expect("two records project");
    assert_eq!(records.len(), 2);
    assert_eq!(
        records[0].record().header().source_tick(),
        Some(0),
        "the legal zero tick is preserved, not dropped or shifted"
    );
    assert_eq!(records[1].record().header().source_tick(), Some(0));
    assert_eq!(environment_view(&records[0]), morning);
    assert_eq!(environment_view(&records[1]), evening);
    assert!(
        records[0].order() < records[1].order(),
        "equal ticks never decide order; the observation keys alone do"
    );

    // A retained observation without a tick is not constructible through the
    // real mirror for a player publication, which always derives one, so the
    // queue-contract case is pinned directly: the absent tick stays absent.
    let epoch = SessionEpoch::try_new(EPOCH).expect("epoch");
    let tickless = AcceptedObservation::try_new(
        ObservationKey::try_new(epoch, ConfirmedRevision::new(3), 0).expect("key"),
        None,
        packet(player_event(9, morning)),
        Vec::new(),
    )
    .expect("staged without a retained tick");
    let direct = project_environment(&fixture.view(
        provider.mirror(),
        &[tickless],
        epoch,
        ConfirmedRevision::new(3),
    ))
    .expect("the tickless observation projects");
    assert_eq!(direct.len(), 1, "no tick is required to project");
    assert_eq!(
        direct[0].record().header().source_tick(),
        None,
        "an absent tick stays absent; none is fabricated"
    );
}

/// Late and duplicate observation identities are refused by the accepted
/// mirror before this projection ever sees them: a duplicate revision key
/// and a backward key both fail without changing the committed revision, the
/// observation queue or the projected output.
#[test]
fn late_or_duplicate_inputs_leave_committed_state_unchanged() {
    let confirmed = world_state(600, 6_000, Weather::Thunder, Season::Winter, 128, -40);
    let mut provider = admitted_mirror(EPOCH);
    commit(&mut provider, player_event(30, confirmed));

    let fixture = view_fixture();
    let baseline = project_environment(&fixture.view_of(&provider)).expect("baseline projects");
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
    let later = world_state(7_000, 7_000, Weather::Clear, Season::Spring, 0, 0);
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
    assert_eq!(provider.mirror().revision().get(), 1);
    assert_eq!(provider.observations().len(), 1);
    assert_eq!(
        project_environment(&fixture.view_of(&provider)).expect("projection still succeeds"),
        baseline,
        "the refused inputs leave the projection unchanged"
    );
}

/// A fresh epoch's mirror starts with an empty queue and projects no
/// environment record, an observation from the old epoch rejects the whole
/// projection under the new frame epoch, and a queue that mixes one valid
/// observation with an old-epoch one rejects without partial output.
#[test]
fn reset_clears_projection_and_old_epoch_rejects_whole() {
    let mut provider = admitted_mirror(EPOCH);
    commit(
        &mut provider,
        player_event(1, world_state(0, 0, Weather::Clear, Season::Spring, 0, 0)),
    );

    let next_epoch_value = EPOCH + 1;
    let reset_provider = admitted_mirror(next_epoch_value);
    let fixture = view_fixture();
    assert!(
        project_environment(&fixture.view_of(&reset_provider))
            .expect("empty queue projects")
            .is_empty(),
        "a reset mirror carries no previous-session environment"
    );

    let next_epoch = SessionEpoch::try_new(next_epoch_value).expect("next epoch");
    let old_epoch_observation = provider.observations()[0].clone();
    assert_eq!(
        project_environment(&fixture.view(
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
        player_event(2, world_state(0, 0, Weather::Clear, Season::Spring, 0, 0)),
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
        project_environment(&fixture.view(
            fresh.mirror(),
            &mixed,
            next_epoch,
            ConfirmedRevision::new(2)
        )),
        Err(ClientError::StaleEpoch),
        "a mixed queue rejects whole, never a partial prefix"
    );
}

/// Observations of other kinds are ignored wholesale — they belong to their
/// own per-topic providers — and a packet that is not a checked event
/// publication rejects the whole projection without partial output, exactly
/// as the mirror refuses to commit one.
#[test]
fn other_kinds_are_ignored_and_non_event_rejects_whole() {
    let mut provider = admitted_mirror(EPOCH);
    commit(&mut provider, closed_event(1));
    commit(&mut provider, closed_event(2));

    let fixture = view_fixture();
    assert!(
        project_environment(&fixture.view_of(&provider))
            .expect("foreign topics project no environment records")
            .is_empty(),
        "observations of other kinds are ignored wholesale"
    );

    let epoch = SessionEpoch::try_new(EPOCH).expect("epoch");
    let valid = provider.observations()[0].clone();
    let non_event = AcceptedObservation::try_new(
        ObservationKey::try_new(epoch, ConfirmedRevision::new(3), 0).expect("key"),
        None,
        ServerPacket::KeepAlive(KeepAlive::new(1).expect("checked keep-alive token")),
        Vec::new(),
    )
    .expect("staged with a non-publication packet");
    assert_eq!(
        project_environment(&fixture.view(
            provider.mirror(),
            &[valid, non_event],
            epoch,
            ConfirmedRevision::new(3)
        )),
        Err(ClientError::InvalidInput),
        "a packet that is not an event publication rejects without a partial prefix"
    );
}
