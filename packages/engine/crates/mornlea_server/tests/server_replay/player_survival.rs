//! Player survival replay: regen/starvation, oxygen, exhaustion, fall and respawn.
//!
//! Every scene below mirrors a frozen Go oracle row, cited at each case:
//!
//! - Health regen (`advanceHealthRegen`, `regenHungerThreshold`,
//!   `restoreFullHunger` in `packages/server/sim/entity/health_regen.go`):
//!   below-max health increments the since-damage counter every tick; the
//!   hunger gate (18, peaceful 0) sits after the increment; a counter above 100
//!   hitting `(counter - 100) % 40 == 0` heals 1 and charges 6000 exhaustion;
//!   peaceful restores hunger 20 and saturation 20000 after the shared charge,
//!   keeping the exhaustion remainder.
//! - Starvation (`advanceStarvation` in
//!   `packages/server/sim/entity/hunger.go`): hunger above zero clears the
//!   timer; peaceful never damages; non-hard at health 1 or below freezes the
//!   timer without incrementing; otherwise every 80th tick deals 1 damage and
//!   resets the timer.
//! - Exhaustion settlement (`applyExhaustion` in
//!   `packages/server/sim/entity/hunger.go`): wide accumulation, one threshold
//!   loop at 4000 spending 1000 saturation, else clearing partial saturation,
//!   else one hunger — a single crossing consumes exactly one resource.
//! - Oxygen (`advanceOxygen` in `packages/server/sim/entity/oxygen.go`):
//!   out of water restores oxygen 300 and drown 0 immediately; submerged ticks
//!   decrement oxygen to zero first, then every 20th tick deals 1 damage.
//! - Fall (`applyFallDamage` in `packages/server/sim/entity/player.go`):
//!   `max(0, floor(peak - land) - 3)` through the shared damage entry, which
//!   clears the since-damage counter and eating progress and clamps at zero.
//! - Death and respawn (`settleDeath` in
//!   `packages/server/sim/entity/death.go` with `beginReset` in
//!   `packages/server/sim/entity/player.go`): full-health and fixed-hunger
//!   restore with transients cleared, settled exactly once, keeping the
//!   unverified bed record.
//! - Observations: the post-step call confirms the staged pose with the
//!   existing tick and input sequence after a real collision, and every real
//!   damage emits one victim-routed combat hit naming the existing tick, the
//!   applied damage and the player kind (the frozen `CombatHit` shape carries
//!   no target identity to invent; the sleep node consumes these to wake
//!   sleepers).
//!
//! No case chooses a value the oracle does not pin. Refusals compare a
//! before/after probe of actor records, staged cells and events, not a bare
//! `is_err`.

use mornlea_domain::{
    BlockPos, ChunkPos, CombatTarget, Command, CommandEnvelope, CommandEnvelopeParts, Dimension,
    Event, EventRecipient, FiniteVec3, HeldActions, HotbarSlot, LookAngles, MotionState,
    MotionStateParts, Movement, PlayerControl, PlayerControlParts, Season, SurvivalState,
    SurvivalStateParts, Weather, WorldState, WorldStateParts,
};
use mornlea_protocol::{LoginStart, admit_login};
use mornlea_server::contracts::*;
use mornlea_server::rules::player_motion;
use mornlea_server::rules::player_survival as provider;
use mornlea_server::state::{ActionKind, AuthorityState, TickContext};
use mornlea_storage::{ItemStack, PlayerLocation, PlayerSave};

const AIR: u16 = 0;
const STONE: u16 = 2;
const GRASS: u16 = 4;
const WATER: u16 = 27;

fn authority() -> AuthorityState {
    let mut state = AuthorityState::try_new(
        ServerLimits::try_new(8, 4096, 512, 64, 64, 1_048_576).expect("limits"),
        7,
    )
    .expect("authority");
    // Past the opening tick: the frozen combat-hit wire rule bars tick zero,
    // so damage observations are pinnable in every scene below.
    state.advance_tick(TickBudget::full()).expect("advance");
    state
}

fn admitted(tag: u8, name: &str) -> mornlea_protocol::AdmittedLogin {
    let mut bytes = [0u8; 16];
    bytes[0] = tag;
    bytes[6] = 0x40;
    bytes[8] = 0x80;
    let id = mornlea_domain::PlayerId::try_from_bytes(bytes).expect("player id");
    let start = LoginStart::new(id, name, 8).expect("login start");
    let inbound = LoginStart::decode_inbound(&start.encode().expect("encoded")).expect("inbound");
    admit_login(inbound).expect("admitted login")
}

fn harness_context(authority: &mut AuthorityState) -> TickContext<'_> {
    TickContext::harness(authority, TickBudget::full())
}

fn environment_with(difficulty: u8) -> EnvironmentState {
    EnvironmentState {
        seed: 7,
        next_tick: 0,
        world_time: 0,
        day_phase_offset: 0,
        season_offset: 0,
        weather: Weather::Clear,
        weather_remaining: 0,
        difficulty,
        tunables: RuleTunables::source_defaults(),
    }
}

fn uuid(tag: u8) -> [u8; 16] {
    let mut bytes = [0u8; 16];
    bytes[0] = tag.max(1);
    bytes[6] = 0x40;
    bytes[8] = 0x80;
    bytes
}

fn overworld_key(pos: BlockPos) -> ChunkKey {
    ChunkKey {
        dimension: Dimension::OVERWORLD,
        pos: ChunkPos::new(pos.x() >> 4, pos.z() >> 4),
    }
}

fn observation(pos: BlockPos, block: u16) -> BlockObservation {
    BlockObservation::try_new(overworld_key(pos), 1, 1, pos, block).expect("block observation")
}

fn player_body(position: [f32; 3]) -> PlayerSave {
    PlayerSave {
        player_id: mornlea_storage::PlayerId::from_bytes(uuid(1)),
        revision: 1,
        display_name: "Tester".to_owned(),
        current: PlayerLocation {
            dimension: 0,
            position,
        },
        yaw: 0.0,
        pitch: 0.0,
        safe: None,
        inventory: mornlea_storage::Inventory::default(),
        health: 20,
        hunger: 20,
        saturation_milli: 5_000,
        exhaustion_milli: 0,
        respawn_present: false,
        respawn_position: [0.0, 0.0, 0.0],
        respawn_dimension: 0,
        armor: [ItemStack::default(); 4],
    }
}

fn player_actor(
    session: SessionKey,
    position: [f32; 3],
    velocity: [f32; 3],
    on_ground: bool,
    health: u8,
    oxygen: u16,
    hunger: u8,
) -> ActorRecord {
    let mut body = player_body(position);
    body.health = health;
    body.hunger = hunger;
    ActorRecord::try_new(
        ActorKey::Player(session),
        ActorLifecycle::Active,
        Dimension::OVERWORLD,
        MotionState::new(MotionStateParts {
            position: FiniteVec3::try_new(position).expect("position"),
            velocity: FiniteVec3::try_new(velocity).expect("velocity"),
            on_ground,
        }),
        LookAngles::try_new(0.0, 0.0).expect("look"),
        SurvivalState::try_new(SurvivalStateParts {
            health,
            oxygen,
            hunger,
            saturation_zero: false,
            armor_points: 0,
        })
        .expect("survival"),
        ActorBody::Player(body),
    )
    .expect("player actor")
}

/// Staged transient record: the reducer-owned lanes this provider settles.
#[allow(clippy::too_many_arguments)]
fn stage_runtime(
    context: &mut TickContext<'_>,
    actor: ActorKey,
    oxygen: u16,
    peak_y: f32,
    exhaustion_milli: u32,
    saturation_milli: u32,
    since_damage_ticks: u32,
    drown_ticks: u32,
    starvation_ticks: u32,
    eating: Option<EatingProgress>,
    controls: Option<PlayerControl>,
    respawn: Option<(Dimension, BlockPos)>,
) {
    context
        .stage(RuleEffect::Runtime(ActorRuntime {
            key: actor,
            controls,
            has_view: false,
            reset: false,
            attack_cooldown: 0,
            hurt_cooldown: 0,
            burn_cooldown: 0,
            oxygen,
            peak_y,
            exhaustion_milli,
            saturation_milli,
            since_damage_ticks,
            drown_ticks,
            starvation_ticks,
            eating,
            bow: None,
            path: None,
            aux: ActorAux::Player { respawn },
        }))
        .expect("runtime");
}

fn control(move_x: i8, move_z: i8, jump: bool, yaw: f32, pitch: f32) -> PlayerControl {
    PlayerControl::new(PlayerControlParts {
        movement: Movement {
            move_x,
            move_z,
            jump,
        },
        look: LookAngles::try_new(yaw, pitch).expect("look"),
        actions: HeldActions {
            primary: false,
            eating: false,
            sprinting: false,
            sneaking: false,
        },
    })
}

fn action_control(
    move_x: i8,
    move_z: i8,
    jump: bool,
    sprinting: bool,
    sneaking: bool,
) -> PlayerControl {
    PlayerControl::new(PlayerControlParts {
        movement: Movement {
            move_x,
            move_z,
            jump,
        },
        look: LookAngles::try_new(0.0, 0.0).expect("look"),
        actions: HeldActions {
            primary: false,
            eating: false,
            sprinting,
            sneaking,
        },
    })
}

fn envelope(session: SessionKey, sequence: u64, command: Command) -> CommandEnvelope {
    CommandEnvelope::try_new(CommandEnvelopeParts {
        tick: 0,
        session: session.get(),
        sequence,
        arrival_index: 0,
        command,
    })
    .expect("envelope")
}

fn regen_call(actor: ActorKey) -> RuleCall<'static> {
    RuleCall {
        phase: RulePhase::PlayerRegenStarvation,
        actor: Some(actor),
        command: None,
        internal: None,
    }
}

fn oxygen_call(actor: ActorKey) -> RuleCall<'static> {
    RuleCall {
        phase: RulePhase::PlayerPrePhysicsOxygen,
        actor: Some(actor),
        command: None,
        internal: None,
    }
}

fn post_call(actor: ActorKey) -> RuleCall<'static> {
    RuleCall {
        phase: RulePhase::PlayerPostPhysics,
        actor: Some(actor),
        command: None,
        internal: None,
    }
}

fn world_record() -> WorldState {
    WorldState::try_new(WorldStateParts {
        day_phase_offset: 0,
        world_time_ticks: 0,
        weather: Weather::Clear,
        season: Season::Spring,
        season_progress: 0,
        temperature: 0,
    })
    .expect("world record")
}

/// Observable authority state a refusal must leave untouched: the actor record,
/// the runtime record, the probed cells with their full observations, and the
/// event count.
#[derive(Clone, Debug, PartialEq)]
struct Probe {
    actor: Option<ActorRecord>,
    runtime: Option<ActorRuntime>,
    cells: Vec<(BlockPos, Option<BlockObservation>)>,
    events: usize,
}

fn probe(context: &TickContext<'_>, actor: ActorKey, cells: &[BlockPos]) -> Probe {
    let view = context.read();
    Probe {
        actor: view.actor(actor).cloned(),
        runtime: view.runtime(actor).cloned(),
        cells: cells
            .iter()
            .map(|pos| (*pos, view.observation(Dimension::OVERWORLD, *pos)))
            .collect(),
        events: context.events().len(),
    }
}

fn applied(report: PhaseReport) -> bool {
    report
        == (PhaseReport {
            examined: 1,
            applied: 1,
            carried: 0,
            rejected: 0,
        })
}

/// The single damage observation a real-damage call emits: victim-routed,
/// existing tick, applied damage, player kind.
fn assert_combat_hit(context: &TickContext<'_>, session: SessionKey, tick: u64, damage: u8) {
    assert_eq!(context.events().len(), 1, "one damage observation");
    let event = &context.events()[0];
    assert_eq!(event.recipient(), EventRecipient::Session(session.get()));
    match event.event() {
        Event::CombatHit(hit) => {
            assert_eq!(hit.server_tick(), tick, "observation carries the tick");
            assert_eq!(hit.damage(), damage, "observation carries the damage");
            assert_eq!(hit.target(), CombatTarget::Player);
        }
        other => panic!("damage emits a combat hit, got {other:?}"),
    }
}

fn survival_scene(
    context: &mut TickContext<'_>,
    session: SessionKey,
    position: [f32; 3],
    health: u8,
    oxygen: u16,
    hunger: u8,
    difficulty: u8,
) -> ActorKey {
    let actor = ActorKey::Player(session);
    context
        .stage(RuleEffect::Environment(environment_with(difficulty)))
        .expect("environment");
    context
        .stage(RuleEffect::Actor(player_actor(
            session,
            position,
            [0.0, 0.0, 0.0],
            true,
            health,
            oxygen,
            hunger,
        )))
        .expect("actor");
    actor
}

/// Fixture-built scene: the initial actor doubles as the pre-step snapshot the
/// context takes at construction, so post-step tests stage the moved actor on
/// top and the provider compares against the true step start.
fn fixture_state(actor: ActorRecord, world: WorldState) -> FixtureState {
    FixtureState {
        runtime: Vec::new(),
        actors: vec![actor],
        chunks: Vec::new(),
        inventories: Vec::new(),
        containers: Vec::new(),
        work: WorkState::default(),
        sleep: SleepState::try_new(Vec::new(), 0, None).expect("sleep"),
        projectiles: Vec::new(),
        drops: Vec::new(),
        world,
    }
}

fn fixture_context<'a>(
    authority: &'a mut AuthorityState,
    initial: &FixtureState,
) -> TickContext<'a> {
    TickContext::from_fixture(authority, initial, TickBudget::full())
}

/// Oxygen budget from `advanceOxygen`: 300 submerged ticks drain oxygen to zero
/// with no damage, 20 more ticks deal the first drowning damage, and leaving
/// the water restores oxygen 300 with drown 0 immediately.
#[test]
fn oxygen_300_then_20() {
    let mut state = authority();
    let session = state
        .admit(admitted(1, "Ada"), TransportKind::Memory)
        .expect("session");
    let mut context = harness_context(&mut state);
    let actor = survival_scene(&mut context, session, [0.5, 8.0, 0.5], 20, 300, 20, 0);
    stage_runtime(
        &mut context,
        actor,
        300,
        8.0,
        0,
        5_000,
        0,
        0,
        0,
        None,
        None,
        None,
    );
    let eye = BlockPos::new(0, 9, 0);
    context.preload_block(observation(eye, WATER));

    for _ in 0..300 {
        let report = provider::run(&mut context, oxygen_call(actor)).expect("oxygen tick");
        assert!(applied(report), "oxygen reports applied, got {report:?}");
    }
    let staged = context.read().actor(actor).expect("actor").clone();
    assert_eq!(
        staged.survival.oxygen(),
        0,
        "300 submerged ticks drain oxygen"
    );
    assert_eq!(staged.survival.health(), 20, "no damage while oxygen lasts");
    let runtime = context.read().runtime(actor).expect("runtime").clone();
    assert_eq!(runtime.oxygen, 0);
    assert_eq!(runtime.drown_ticks, 0);

    for _ in 0..19 {
        provider::run(&mut context, oxygen_call(actor)).expect("drown tick");
    }
    let runtime = context.read().runtime(actor).expect("runtime").clone();
    assert_eq!(runtime.drown_ticks, 19, "drown timer counts at zero oxygen");
    assert_eq!(
        context
            .read()
            .actor(actor)
            .expect("actor")
            .survival
            .health(),
        20,
        "no damage before the 20th drown tick"
    );
    provider::run(&mut context, oxygen_call(actor)).expect("drowning damage");
    let staged = context.read().actor(actor).expect("actor").clone();
    assert_eq!(staged.survival.health(), 19, "20 more ticks deal 1 damage");
    let runtime = context.read().runtime(actor).expect("runtime").clone();
    assert_eq!(runtime.drown_ticks, 0, "the damage tick resets the timer");
    assert_eq!(runtime.since_damage_ticks, 0);
    let tick = context.read().tick();
    assert_combat_hit(&context, session, tick, 1);

    context.preload_block(observation(eye, AIR));
    provider::run(&mut context, oxygen_call(actor)).expect("surface");
    let staged = context.read().actor(actor).expect("actor").clone();
    assert_eq!(
        staged.survival.oxygen(),
        300,
        "exit restores oxygen at once"
    );
    assert_eq!(
        context.read().runtime(actor).expect("runtime").drown_ticks,
        0,
        "exit clears the drown timer"
    );
    assert_eq!(
        context.events().len(),
        1,
        "surfacing emits no second observation"
    );
}

/// Regen rhythm and starvation floors from `advanceHealthRegen`,
/// `regenHungerThreshold` and `advanceStarvation`: the heal lands at counter
/// 140, never 139; normal at hunger 0 and health 1 freezes the starvation
/// timer; hard at interval 80 reaches zero without settling death early.
#[test]
fn regen_140_and_hunger_floor() {
    // Heal at exactly counter 140 with the shared 6000 exhaustion charge: one
    // threshold spends 1000 saturation, leaving exhaustion 2000.
    let mut state = authority();
    let session = state
        .admit(admitted(1, "Ada"), TransportKind::Memory)
        .expect("session");
    let mut context = harness_context(&mut state);
    let actor = survival_scene(&mut context, session, [0.5, 1.0, 0.5], 19, 300, 18, 0);
    stage_runtime(
        &mut context,
        actor,
        300,
        1.0,
        0,
        5_000,
        139,
        0,
        0,
        None,
        None,
        None,
    );
    let report = provider::run(&mut context, regen_call(actor)).expect("regen tick");
    assert!(applied(report), "regen reports applied, got {report:?}");
    let staged = context.read().actor(actor).expect("actor").clone();
    assert_eq!(staged.survival.health(), 20, "counter 140 heals");
    let runtime = context.read().runtime(actor).expect("runtime").clone();
    assert_eq!(runtime.since_damage_ticks, 140);
    assert_eq!(runtime.exhaustion_milli, 2_000);
    assert_eq!(runtime.saturation_milli, 4_000);

    // Counter 139 does not heal yet.
    let mut state = authority();
    let session = state
        .admit(admitted(2, "Bea"), TransportKind::Memory)
        .expect("session");
    let mut context = harness_context(&mut state);
    let actor = survival_scene(&mut context, session, [0.5, 1.0, 0.5], 19, 300, 18, 0);
    stage_runtime(
        &mut context,
        actor,
        300,
        1.0,
        0,
        5_000,
        138,
        0,
        0,
        None,
        None,
        None,
    );
    provider::run(&mut context, regen_call(actor)).expect("regen tick");
    let staged = context.read().actor(actor).expect("actor").clone();
    assert_eq!(staged.survival.health(), 19, "counter 139 never heals");
    assert_eq!(
        context
            .read()
            .runtime(actor)
            .expect("runtime")
            .since_damage_ticks,
        139
    );

    // Normal at hunger 0 and health 1 freezes: no damage and no timer advance.
    let mut state = authority();
    let session = state
        .admit(admitted(3, "Cara"), TransportKind::Memory)
        .expect("session");
    let mut context = harness_context(&mut state);
    let actor = survival_scene(&mut context, session, [0.5, 1.0, 0.5], 1, 300, 0, 0);
    stage_runtime(
        &mut context,
        actor,
        300,
        1.0,
        0,
        0,
        10,
        0,
        5,
        None,
        None,
        None,
    );
    provider::run(&mut context, regen_call(actor)).expect("starvation tick");
    let staged = context.read().actor(actor).expect("actor").clone();
    assert_eq!(staged.survival.health(), 1, "normal keeps one health");
    let runtime = context.read().runtime(actor).expect("runtime").clone();
    assert_eq!(
        runtime.starvation_ticks, 5,
        "the frozen timer never advances"
    );

    // Hard at interval 80 reaches zero: damage lands and death stays unsettled
    // until a later call.
    let mut state = authority();
    let session = state
        .admit(admitted(4, "Dan"), TransportKind::Memory)
        .expect("session");
    let mut context = harness_context(&mut state);
    let actor = survival_scene(&mut context, session, [0.5, 1.0, 0.5], 1, 300, 0, 2);
    stage_runtime(
        &mut context,
        actor,
        300,
        1.0,
        0,
        0,
        10,
        0,
        79,
        None,
        None,
        None,
    );
    provider::run(&mut context, regen_call(actor)).expect("starvation tick");
    let staged = context.read().actor(actor).expect("actor").clone();
    assert_eq!(staged.survival.health(), 0, "hard interval 80 reaches zero");
    assert_eq!(staged.lifecycle, ActorLifecycle::Active);
    let runtime = context.read().runtime(actor).expect("runtime").clone();
    assert_eq!(runtime.starvation_ticks, 0);
    assert_eq!(runtime.since_damage_ticks, 0);
    let tick = context.read().tick();
    assert_combat_hit(&context, session, tick, 1);

    // Peaceful heals through hunger 0, then restores hunger 20 and saturation
    // 20000 while the exhaustion remainder survives the restore.
    let mut state = authority();
    let session = state
        .admit(admitted(5, "Eli"), TransportKind::Memory)
        .expect("session");
    let mut context = harness_context(&mut state);
    let actor = survival_scene(&mut context, session, [0.5, 1.0, 0.5], 19, 300, 0, 1);
    stage_runtime(
        &mut context,
        actor,
        300,
        1.0,
        0,
        5_000,
        139,
        0,
        0,
        None,
        None,
        None,
    );
    provider::run(&mut context, regen_call(actor)).expect("peaceful regen");
    let staged = context.read().actor(actor).expect("actor").clone();
    assert_eq!(staged.survival.health(), 20);
    assert_eq!(staged.survival.hunger(), 20, "peaceful restores hunger");
    let runtime = context.read().runtime(actor).expect("runtime").clone();
    assert_eq!(runtime.saturation_milli, 20_000);
    assert_eq!(
        runtime.exhaustion_milli, 2_000,
        "the restore keeps the exhaustion remainder"
    );
    assert!(context.events().is_empty());
}

/// Double threshold crossing from `applyExhaustion`: exhaustion 3999 with
/// partial saturation 500 plus the 6000 regen charge crosses twice, consuming
/// the partial 500 first, then one hunger, retaining exhaustion 1999.
#[test]
fn exhaustion_crosses_twice() {
    let mut state = authority();
    let session = state
        .admit(admitted(1, "Ada"), TransportKind::Memory)
        .expect("session");
    let mut context = harness_context(&mut state);
    let actor = survival_scene(&mut context, session, [0.5, 1.0, 0.5], 19, 300, 20, 0);
    stage_runtime(
        &mut context,
        actor,
        300,
        1.0,
        3_999,
        500,
        139,
        0,
        0,
        None,
        None,
        None,
    );
    let report = provider::run(&mut context, regen_call(actor)).expect("regen tick");
    assert!(applied(report), "regen reports applied, got {report:?}");
    let staged = context.read().actor(actor).expect("actor").clone();
    assert_eq!(staged.survival.health(), 20);
    assert_eq!(
        staged.survival.hunger(),
        19,
        "second crossing spends hunger"
    );
    let runtime = context.read().runtime(actor).expect("runtime").clone();
    assert_eq!(runtime.saturation_milli, 0, "first crossing clears partial");
    assert!(
        staged.survival.saturation_zero(),
        "the zero hint follows saturation"
    );
    assert_eq!(runtime.exhaustion_milli, 1_999, "two thresholds retained");
    assert!(context.events().is_empty());
}

/// Fall curve from `applyFallDamage`: height 3 deals no damage, height 4 deals
/// 1 through the shared damage entry, clearing eating and resetting the peak.
/// Motion charges settle against the pre-step snapshot on the same pass: a
/// real-ground takeoff charges 50 exactly once, submerged displacement charges
/// the fixed-point table, and an accelerated sprint charges 80.
/// Foreign shapes refuse without effect.
#[test]
fn fall_curve_3_4() {
    let cells = [BlockPos::new(0, 10, 0), BlockPos::new(0, 9, 0)];
    let eating = Some(EatingProgress {
        slot: HotbarSlot::new(0).expect("slot"),
        item: 1,
        ticks: 3,
    });

    let mut state = authority();
    let session = state
        .admit(admitted(1, "Ada"), TransportKind::Memory)
        .expect("session");
    let mut context = harness_context(&mut state);
    let actor = survival_scene(&mut context, session, [0.5, 10.0, 0.5], 20, 300, 20, 0);
    stage_runtime(
        &mut context,
        actor,
        300,
        13.0,
        0,
        5_000,
        50,
        0,
        0,
        eating,
        None,
        None,
    );
    let report = provider::run(&mut context, post_call(actor)).expect("landing");
    assert!(applied(report), "landing reports applied, got {report:?}");
    let staged = context.read().actor(actor).expect("actor").clone();
    assert_eq!(staged.survival.health(), 20, "height 3 deals no damage");
    let runtime = context.read().runtime(actor).expect("runtime").clone();
    assert_eq!(runtime.peak_y, 10.0, "landing resets the peak");
    assert_eq!(
        runtime.since_damage_ticks, 50,
        "no damage keeps the counter"
    );
    assert!(context.events().is_empty());

    let mut state = authority();
    let session = state
        .admit(admitted(2, "Bea"), TransportKind::Memory)
        .expect("session");
    let mut context = harness_context(&mut state);
    let actor = survival_scene(&mut context, session, [0.5, 10.0, 0.5], 20, 300, 20, 0);
    stage_runtime(
        &mut context,
        actor,
        300,
        14.0,
        0,
        5_000,
        50,
        0,
        0,
        eating,
        None,
        None,
    );
    provider::run(&mut context, post_call(actor)).expect("landing");
    let staged = context.read().actor(actor).expect("actor").clone();
    assert_eq!(staged.survival.health(), 19, "height 4 deals 1 damage");
    let runtime = context.read().runtime(actor).expect("runtime").clone();
    assert_eq!(runtime.eating, None, "real damage clears eating");
    assert_eq!(runtime.since_damage_ticks, 0);
    assert_eq!(runtime.peak_y, 10.0);
    let tick = context.read().tick();
    assert_combat_hit(&context, session, tick, 1);

    // Takeoff charges exactly once: pre-step grounded, jump held, post-step
    // airborne and dry. A grounded post-step (probe tolerance) and a mid-air
    // hold charge nothing, mirroring the takeoff trio in
    // `TestJumpAccumulatesExhaustionExactlyOncePerTakeoff` and
    // `TestJumpDisplacementWithinGroundProbeToleranceDoesNotAccumulateExhaustion`.
    for (name, pre_ground, post, want) in [
        ("takeoff", true, [0.5, 10.5, 0.5], 50),
        ("probe tolerance", true, [0.5, 10.0, 0.5], 0),
        ("mid-air hold", false, [0.5, 10.2, 0.5], 0),
    ] {
        let mut state = authority();
        let session = state
            .admit(admitted(6, "Fay"), TransportKind::Memory)
            .expect("session");
        let pre_y = if pre_ground { 10.0 } else { 10.5 };
        let initial = fixture_state(
            player_actor(
                session,
                [0.5, pre_y, 0.5],
                [0.0, 0.0, 0.0],
                pre_ground,
                20,
                300,
                20,
            ),
            world_record(),
        );
        let mut context = fixture_context(&mut state, &initial);
        let actor = ActorKey::Player(session);
        context
            .stage(RuleEffect::Environment(environment_with(0)))
            .expect("environment");
        let input = envelope(
            session,
            1,
            Command::PlayerInput(action_control(0, 0, true, false, false)),
        );
        context
            .defer(input, RulePhase::PlayerMotion)
            .expect("deferred input");
        let airborne = post[1] > 10.0;
        context
            .stage(RuleEffect::Actor(player_actor(
                session,
                post,
                [0.0, 0.0, 0.0],
                !airborne,
                20,
                300,
                20,
            )))
            .expect("post-step actor");
        provider::run(&mut context, post_call(actor)).expect("post physics");
        assert_eq!(
            context
                .read()
                .runtime(actor)
                .expect("runtime")
                .exhaustion_milli,
            want,
            "{name} charges exactly"
        );
        assert_eq!(
            context
                .read()
                .actor(actor)
                .expect("actor")
                .survival
                .health(),
            20
        );
        assert_eq!(context.events().len(), 1, "{name} emits only the pose");
        assert!(matches!(context.events()[0].event(), Event::PlayerState(_)));
    }

    // Swim charges the exact fixed-point displacement while submerged: one
    // block along X converts to 10, still water to 0, dry movement to 0,
    // mirroring `TestSwimExhaustionMilliFixedPointRounding` through the
    // post-step pass.
    for (name, water, post, want) in [
        ("swim one block", true, [1.5, 8.0, 0.5], 10),
        ("still water", true, [0.5, 8.0, 0.5], 0),
        ("dry movement", false, [1.5, 8.0, 0.5], 0),
    ] {
        let mut state = authority();
        let session = state
            .admit(admitted(7, "Gus"), TransportKind::Memory)
            .expect("session");
        let initial = fixture_state(
            player_actor(session, [0.5, 8.0, 0.5], [0.0, 0.0, 0.0], true, 20, 300, 20),
            world_record(),
        );
        let mut context = fixture_context(&mut state, &initial);
        let actor = ActorKey::Player(session);
        context
            .stage(RuleEffect::Environment(environment_with(0)))
            .expect("environment");
        if water {
            context.preload_block(observation(BlockPos::new(0, 8, 0), WATER));
            context.preload_block(observation(BlockPos::new(0, 9, 0), WATER));
        }
        context
            .stage(RuleEffect::Actor(player_actor(
                session,
                post,
                [0.0, 0.0, 0.0],
                true,
                20,
                300,
                20,
            )))
            .expect("post-step actor");
        provider::run(&mut context, post_call(actor)).expect("post physics");
        assert_eq!(
            context
                .read()
                .runtime(actor)
                .expect("runtime")
                .exhaustion_milli,
            want,
            "{name} charges exactly"
        );
        assert_eq!(context.events().len(), 1, "{name} emits only the pose");
        assert!(matches!(context.events()[0].event(), Event::PlayerState(_)));
    }

    // Sprint actual: held sprint with forward intent on dry pre-step ground
    // charges 80; hunger below 6 suppresses the staged sprint bit and charges
    // nothing; sneaking and mid-air sprints charge nothing either.
    for (name, hunger, sneaking, pre_ground, want) in [
        ("sprint", 20, false, true, 80),
        ("hunger gate", 5, false, true, 0),
        ("sneak gate", 20, true, true, 0),
        ("air sprint", 20, false, false, 0),
    ] {
        let mut state = authority();
        let session = state
            .admit(admitted(8, "Hal"), TransportKind::Memory)
            .expect("session");
        let pre_y = if pre_ground { 10.0 } else { 10.5 };
        let initial = fixture_state(
            player_actor(
                session,
                [0.5, pre_y, 0.5],
                [0.0, 0.0, 0.0],
                pre_ground,
                20,
                300,
                hunger,
            ),
            world_record(),
        );
        let mut context = fixture_context(&mut state, &initial);
        let actor = ActorKey::Player(session);
        context
            .stage(RuleEffect::Environment(environment_with(0)))
            .expect("environment");
        let input = envelope(
            session,
            1,
            Command::PlayerInput(action_control(0, 1, false, true, sneaking)),
        );
        context
            .defer(input, RulePhase::PlayerMotion)
            .expect("deferred input");
        context
            .stage(RuleEffect::Actor(player_actor(
                session,
                [0.7, 10.0, 0.5],
                [0.0, 0.0, 0.0],
                true,
                20,
                300,
                hunger,
            )))
            .expect("post-step actor");
        provider::run(&mut context, post_call(actor)).expect("post physics");
        let runtime = context.read().runtime(actor).expect("runtime").clone();
        assert_eq!(runtime.exhaustion_milli, want, "{name} charges exactly");
        if want == 0 && (hunger < 6 || sneaking) {
            assert_eq!(
                runtime.controls.map(|held| held.actions().sprinting),
                Some(false),
                "{name} clears the staged sprint bit"
            );
        }
        assert_eq!(context.events().len(), 1, "{name} emits only the pose");
        assert!(matches!(context.events()[0].event(), Event::PlayerState(_)));
    }

    // Foreign shapes refuse without effect: a wrong phase, a companion actor,
    // and a call carrying a command all leave staged state untouched.
    let before = probe(&context, actor, &cells);
    assert!(
        provider::run(
            &mut context,
            RuleCall {
                phase: RulePhase::Publish,
                actor: Some(actor),
                command: None,
                internal: None,
            }
        )
        .is_err()
    );
    let companion = ActorKey::Companion(
        mornlea_domain::CompanionId::try_from_bytes(uuid(9)).expect("companion id"),
    );
    assert!(provider::run(&mut context, post_call(companion)).is_err());
    let misplaced = envelope(
        session,
        9,
        Command::PlayerInput(control(1, 0, false, 0.0, 0.0)),
    );
    assert!(
        provider::run(
            &mut context,
            RuleCall {
                phase: RulePhase::PlayerPostPhysics,
                actor: Some(actor),
                command: Some(&misplaced),
                internal: None,
            }
        )
        .is_err()
    );
    assert_eq!(probe(&context, actor, &cells), before);
}

/// Combined settlement with source-pinned checkpoints: drowning damage at the
/// exact drown tick, a fall to zero settled once by a later call that keeps
/// the unverified bed record, and one confirmed pose after a real collision
/// carrying the existing tick and sequence with no combat hit.
#[test]
fn oxygen_fall_respawn_and_correction() {
    // Drowning checkpoint: oxygen 1 drains silently, then the drown timer at
    // 19 deals damage on the next submerged tick.
    let mut state = authority();
    let session = state
        .admit(admitted(1, "Ada"), TransportKind::Memory)
        .expect("session");
    let mut context = harness_context(&mut state);
    let actor = survival_scene(&mut context, session, [0.5, 8.0, 0.5], 20, 1, 20, 0);
    stage_runtime(
        &mut context,
        actor,
        1,
        8.0,
        0,
        5_000,
        0,
        0,
        0,
        None,
        None,
        None,
    );
    context.preload_block(observation(BlockPos::new(0, 9, 0), WATER));
    provider::run(&mut context, oxygen_call(actor)).expect("last oxygen");
    let staged = context.read().actor(actor).expect("actor").clone();
    assert_eq!(staged.survival.oxygen(), 0);
    assert_eq!(staged.survival.health(), 20);
    let mut runtime = context.read().runtime(actor).expect("runtime").clone();
    runtime.drown_ticks = 19;
    context
        .stage(RuleEffect::Runtime(runtime))
        .expect("drown timer");
    provider::run(&mut context, oxygen_call(actor)).expect("drowning damage");
    let staged = context.read().actor(actor).expect("actor").clone();
    assert_eq!(
        staged.survival.health(),
        19,
        "drown timer 20 deals 1 damage"
    );
    assert_eq!(
        context.read().runtime(actor).expect("runtime").drown_ticks,
        0
    );
    let tick = context.read().tick();
    assert_combat_hit(&context, session, tick, 1);

    // A fall to zero stays unsettled in its own call, then one later call
    // settles death exactly once: full health, fixed hunger, cleared
    // transients, kept bed record, and no further settlement afterwards.
    let mut state = authority();
    let session = state
        .admit(admitted(2, "Bea"), TransportKind::Memory)
        .expect("session");
    let mut context = harness_context(&mut state);
    let actor = survival_scene(&mut context, session, [0.5, 10.0, 0.5], 5, 300, 20, 0);
    let bed = BlockPos::new(3, 4, 5);
    stage_runtime(
        &mut context,
        actor,
        300,
        100.0,
        0,
        5_000,
        7,
        0,
        0,
        None,
        None,
        Some((Dimension::OVERWORLD, bed)),
    );
    provider::run(&mut context, post_call(actor)).expect("fatal fall");
    let staged = context.read().actor(actor).expect("actor").clone();
    assert_eq!(staged.survival.health(), 0, "the fall reaches zero");
    assert_eq!(staged.lifecycle, ActorLifecycle::Active);
    let tick = context.read().tick();
    assert_combat_hit(&context, session, tick, 5);
    provider::run(&mut context, regen_call(actor)).expect("death settles");
    let staged = context.read().actor(actor).expect("actor").clone();
    assert_eq!(staged.lifecycle, ActorLifecycle::Respawning);
    assert_eq!(staged.survival.health(), 20, "respawn restores health");
    assert_eq!(staged.survival.hunger(), 20, "respawn restores hunger");
    let runtime = context.read().runtime(actor).expect("runtime").clone();
    assert_eq!(runtime.oxygen, 300);
    assert_eq!(runtime.exhaustion_milli, 0);
    assert_eq!(runtime.saturation_milli, 5_000);
    assert_eq!(runtime.since_damage_ticks, 0);
    assert_eq!(runtime.drown_ticks, 0);
    assert_eq!(runtime.starvation_ticks, 0);
    assert_eq!(
        runtime.aux,
        ActorAux::Player {
            respawn: Some((Dimension::OVERWORLD, bed))
        },
        "respawn keeps the unverified bed record"
    );
    let body = match &staged.body {
        ActorBody::Player(save) => save.clone(),
        _ => panic!("player keeps a player body"),
    };
    assert_eq!(body.health, 20);
    assert_eq!(body.hunger, 20);
    let before = probe(&context, actor, &[]);
    assert!(
        provider::run(&mut context, regen_call(actor)).is_err(),
        "death settles exactly once"
    );
    assert_eq!(probe(&context, actor, &[]), before);

    // One correction after a real collision: the accepted motion provider clips
    // against the staged wall, and the post-step call confirms that exact pose
    // with the existing tick and input sequence and no combat hit.
    let mut state = authority();
    let session = state
        .admit(admitted(3, "Cara"), TransportKind::Memory)
        .expect("session");
    let mut context = harness_context(&mut state);
    let actor = ActorKey::Player(session);
    context
        .stage(RuleEffect::Environment(environment_with(0)))
        .expect("environment");
    context
        .stage(RuleEffect::Actor(player_actor(
            session,
            [0.5, 1.0, 0.5],
            [4.3, 0.0, 0.0],
            true,
            20,
            300,
            20,
        )))
        .expect("actor");
    for x in -4..=6 {
        for y in -1..=10 {
            for z in -4..=4 {
                context.preload_block(observation(BlockPos::new(x, y, z), AIR));
            }
        }
    }
    for x in -4..=6 {
        for z in -4..=4 {
            context.preload_block(observation(BlockPos::new(x, 0, z), GRASS));
        }
    }
    for y in 1..=2 {
        context.preload_block(observation(BlockPos::new(1, y, 0), STONE));
    }
    let input = envelope(
        session,
        7,
        Command::PlayerInput(control(1, 0, false, 0.0, 0.0)),
    );
    context
        .defer(input, RulePhase::PlayerMotion)
        .expect("deferred input");
    let motion_call = RuleCall {
        phase: RulePhase::PlayerMotion,
        actor: Some(actor),
        command: None,
        internal: None,
    };
    player_motion::run(&mut context, motion_call).expect("motion advances");
    let clipped = context.read().actor(actor).expect("actor").motion;
    assert_eq!(
        clipped.position().get().map(f32::to_bits),
        [0x3f33_3333, 0x3f80_0000, 0x3f00_0000],
        "the wall clips the pose first"
    );
    context
        .stage(RuleEffect::World(world_record()))
        .expect("world record");
    let tick = context.read().tick();
    provider::run(&mut context, post_call(actor)).expect("post physics");
    let confirmed = context.read().actor(actor).expect("actor").clone();
    assert_eq!(confirmed.motion, clipped, "the pose confirms the clip");
    assert_eq!(confirmed.survival.health(), 20);
    assert_eq!(context.events().len(), 1, "one correction");
    let event = &context.events()[0];
    assert_eq!(event.recipient(), EventRecipient::Session(session.get()));
    match event.event() {
        Event::PlayerState(pose) => {
            assert_eq!(pose.server_tick(), tick, "correction carries the tick");
            assert_eq!(
                pose.last_input_sequence(),
                7,
                "correction carries the sequence"
            );
            assert_eq!(pose.motion(), clipped);
            assert_eq!(pose.survival().health(), 20);
        }
        other => panic!("correction emits the confirmed pose, got {other:?}"),
    }
    assert!(
        !context
            .events()
            .iter()
            .any(|event| matches!(event.event(), Event::CombatHit(_))),
        "a damage-free correction emits no combat hit"
    );
}

/// Action-receipt settlement: noted mining/till/melee charges settle through
/// the shared threshold loop at the post-physics pass; other actors' receipts
/// survive the drain; death drops pending receipts with the hunger reset.
#[test]
fn exhaustion_charge_receipts_settle() {
    let mut state = authority();
    let session = state
        .admit(admitted(1, "Ada"), TransportKind::Memory)
        .expect("session");
    let session_b = state
        .admit(admitted(2, "Bea"), TransportKind::Memory)
        .expect("session b");
    let mut context = harness_context(&mut state);
    let actor = survival_scene(&mut context, session, [0.5, 10.0, 0.5], 20, 300, 20, 0);
    stage_runtime(
        &mut context,
        actor,
        300,
        10.0,
        3_990,
        5_000,
        0,
        0,
        0,
        None,
        None,
        None,
    );
    context
        .note_charge(actor, ActionKind::Melee)
        .expect("melee receipt");
    let report = provider::run(&mut context, post_call(actor)).expect("post physics");
    assert!(applied(report), "post reports applied, got {report:?}");
    let runtime = context.read().runtime(actor).expect("runtime").clone();
    assert_eq!(runtime.exhaustion_milli, 90, "melee crosses one threshold");
    assert_eq!(runtime.saturation_milli, 4_000);
    assert_eq!(
        context
            .read()
            .actor(actor)
            .expect("actor")
            .survival
            .hunger(),
        20
    );

    context
        .note_charge(actor, ActionKind::Mining)
        .expect("mining receipt");
    context
        .note_charge(actor, ActionKind::Till)
        .expect("till receipt");
    provider::run(&mut context, post_call(actor)).expect("post physics");
    let runtime = context.read().runtime(actor).expect("runtime").clone();
    assert_eq!(
        runtime.exhaustion_milli, 100,
        "mining and till add without crossing"
    );
    assert_eq!(runtime.saturation_milli, 4_000);
    assert!(context.events().is_empty());

    // Another actor's receipt survives this actor's drain untouched.
    let actor_b = ActorKey::Player(session_b);
    context
        .stage(RuleEffect::Actor(player_actor(
            session_b,
            [2.5, 10.0, 0.5],
            [0.0, 0.0, 0.0],
            true,
            20,
            300,
            20,
        )))
        .expect("actor b");
    context
        .note_charge(actor_b, ActionKind::Melee)
        .expect("foreign receipt");
    provider::run(&mut context, post_call(actor)).expect("post physics");
    assert_eq!(
        context.take_charges(),
        vec![(actor_b, ActionKind::Melee)],
        "foreign receipts survive the drain"
    );

    // Death drops pending receipts with the hunger reset instead of settling
    // them: the fixed respawn values leave no room for banked charges.
    context
        .note_charge(actor, ActionKind::Melee)
        .expect("doomed receipt");
    let mut staged = context.read().actor(actor).expect("actor").clone();
    let mut body = match &staged.body {
        ActorBody::Player(save) => save.clone(),
        _ => panic!("player keeps a player body"),
    };
    body.health = 0;
    staged = ActorRecord::try_new(
        staged.key,
        staged.lifecycle,
        staged.dimension,
        staged.motion,
        staged.look,
        SurvivalState::try_new(SurvivalStateParts {
            health: 0,
            oxygen: 300,
            hunger: 20,
            saturation_zero: false,
            armor_points: 0,
        })
        .expect("survival"),
        ActorBody::Player(body),
    )
    .expect("actor");
    context
        .stage(RuleEffect::Actor(staged))
        .expect("fatal record");
    provider::run(&mut context, regen_call(actor)).expect("death settles");
    assert_eq!(
        context.take_charges(),
        Vec::new(),
        "death drops pending receipts"
    );
}
