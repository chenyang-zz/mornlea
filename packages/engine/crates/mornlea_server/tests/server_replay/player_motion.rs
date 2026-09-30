//! Player motion replay: validated intake, kernel advance, and held staging.
//!
//! Every scene below mirrors a frozen Go oracle row, cited at each case:
//!
//! - Input validation from `packages/server/sim/entity/placement.go`
//!   (`validPlayerInput`: move axes in -1..=1; `validPlayerLook`: finite yaw and
//!   pitch inside +/-(pi/2 - 0.01)) and the intake clearing in
//!   `packages/server/sim/entity/tick.go` (`ApplyPlayerCommands`: an invalid
//!   input zeroes movement, keeps yaw, and clears held bits).
//! - Latest-wins and the neutral-held follow-up from
//!   `packages/server/sim/entity/movement_test.go`
//!   (`TestInvalidLatestInputIsAckedAndNeutral`: the invalid latest input leaves
//!   position untouched with zero velocity, and the next tick without input
//!   stays neutral; `TestEngineReusesHeldPlayerInputWithoutNewCommand` pins the
//!   held-input idea the reducer owns across ticks).
//! - The advance itself runs the accepted F1 kernels; the bit-exact vectors were
//!   collected from the production path (`physics.StepWithTunables` over the Rust
//!   kernel at `DefaultTunables`) the same way
//!   `packages/shared/physics/step_golden_vectors_test.go` pins its vectors. The
//!   fluid vector below reproduces that file's "fluid sink from rest" case
//!   exactly; the wall, fall, walk and yaw cases use the same collection method
//!   against a throwaway probe that was removed before commit.
//!
//! No case chooses a value the oracle does not pin. Refusals compare a
//! before/after probe of actor records, staged cells and events, not a bare
//! `is_err`.

use mornlea_domain::{
    BlockPos, ChunkPos, Command, CommandEnvelope, CommandEnvelopeParts, Dimension, FiniteVec3,
    HeldActions, HotbarSlot, LookAngles, MotionState, MotionStateParts, Movement, PlayerControl,
    PlayerControlParts, SurvivalState, SurvivalStateParts, Weather,
};
use mornlea_protocol::{LoginStart, admit_login};
use mornlea_server::contracts::*;
use mornlea_server::rules::player_motion as provider;
use mornlea_server::state::{AuthorityState, TickContext};
use mornlea_storage::{ItemStack, PlayerLocation, PlayerSave};

// Stable block numbers, mirrored from the frozen const block in
// `packages/shared/core/block.go`: `AirID` 0, `StoneID` 2, `GrassID` 4, and the
// fluid range `WaterSourceID` 27 through `WaterLevel7ID` 34 (`IsFluid` in
// `packages/shared/core/fluid.go`).
const AIR: u16 = 0;
const STONE: u16 = 2;
const GRASS: u16 = 4;
const WATER: u16 = 27;

// Pitch bound mirror of `validPlayerLook` in
// `packages/server/sim/entity/placement.go`: `float32(math.Pi/2 - 0.01)`,
// computed in float64 before narrowing exactly like the Go row.
fn max_pitch() -> f32 {
    (std::f64::consts::PI / 2.0 - 0.01) as f32
}

fn authority() -> AuthorityState {
    AuthorityState::try_new(
        ServerLimits::try_new(8, 4096, 512, 64, 64, 1_048_576).expect("limits"),
        7,
    )
    .expect("authority")
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

fn environment() -> EnvironmentState {
    EnvironmentState {
        seed: 7,
        next_tick: 0,
        world_time: 0,
        day_phase_offset: 0,
        season_offset: 0,
        weather: Weather::Clear,
        weather_remaining: 0,
        difficulty: 0,
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
    lifecycle: ActorLifecycle,
) -> ActorRecord {
    ActorRecord::try_new(
        ActorKey::Player(session),
        lifecycle,
        Dimension::OVERWORLD,
        MotionState::new(MotionStateParts {
            position: FiniteVec3::try_new(position).expect("position"),
            velocity: FiniteVec3::try_new(velocity).expect("velocity"),
            on_ground,
        }),
        LookAngles::try_new(0.0, 0.0).expect("look"),
        SurvivalState::try_new(SurvivalStateParts {
            health: 20,
            oxygen: 300,
            hunger: 20,
            saturation_zero: false,
            armor_points: 0,
        })
        .expect("survival"),
        ActorBody::Player(player_body(position)),
    )
    .expect("player actor")
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

fn intake_call(envelope: &CommandEnvelope) -> RuleCall<'_> {
    RuleCall {
        phase: RulePhase::PlayerCommand,
        actor: None,
        command: Some(envelope),
        internal: None,
    }
}

fn motion_call(actor: ActorKey) -> RuleCall<'static> {
    RuleCall {
        phase: RulePhase::PlayerMotion,
        actor: Some(actor),
        command: None,
        internal: None,
    }
}

/// One open room of staged air plus a grass floor, the movement-flat-world
/// shape from `movementFlatChunk` in
/// `packages/server/sim/entity/movement_test.go` (floor at y 0, spawn at
/// y 1). Callers override cells for wall and water legs afterwards.
fn motion_scene(
    context: &mut TickContext<'_>,
    session: SessionKey,
    position: [f32; 3],
    velocity: [f32; 3],
    on_ground: bool,
) -> ActorKey {
    let actor = ActorKey::Player(session);
    context
        .stage(RuleEffect::Environment(environment()))
        .expect("environment");
    context
        .stage(RuleEffect::Actor(player_actor(
            session,
            position,
            velocity,
            on_ground,
            ActorLifecycle::Active,
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
    actor
}

/// Bit-exact motion assertion: the provider must reproduce the oracle vectors
/// bitwise, including negative zero lanes.
fn assert_motion(actual: MotionState, pos: [u32; 3], vel: [u32; 3], ground: bool) {
    assert_eq!(
        actual.position().get().map(f32::to_bits),
        pos,
        "position bits"
    );
    assert_eq!(
        actual.velocity().get().map(f32::to_bits),
        vel,
        "velocity bits"
    );
    assert_eq!(actual.on_ground(), ground, "grounded");
}

/// Observable authority state a refusal must leave untouched: the actor record,
/// the probed cells with their full observations, and the event count.
#[derive(Clone, Debug, PartialEq)]
struct Probe {
    actor: Option<ActorRecord>,
    cells: Vec<(BlockPos, Option<BlockObservation>)>,
    events: usize,
}

fn probe(context: &TickContext<'_>, actor: ActorKey, cells: &[BlockPos]) -> Probe {
    let view = context.read();
    Probe {
        actor: view.actor(actor).cloned(),
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

/// Movement owns controls while survival, combat and action providers retain
/// their independent lanes through the same whole-record staging surface.
#[test]
fn motion_preserves_sibling_runtime() {
    let prior_control = control(-1, 0, false, 0.0, 0.0);
    let replacement = control(1, 0, false, 3.0 * std::f32::consts::PI, 0.2);
    let neutral = control(0, 0, false, 0.0, 0.0);
    let invalid = control(2, 0, false, 0.0, 0.0);
    for (name, incoming, expected_control) in [
        ("valid replacement", Some(replacement), Some(replacement)),
        ("held without packet", None, Some(prior_control)),
        ("explicit neutral", Some(neutral), Some(neutral)),
        ("invalid clears", Some(invalid), None),
    ] {
        let mut state = authority();
        let session = state
            .admit(admitted(1, "Ada"), TransportKind::Memory)
            .expect("session");
        let mut context = harness_context(&mut state);
        let actor = motion_scene(&mut context, session, [0.5, 1.0, 0.5], [0.0; 3], true);
        let before = ActorRuntime {
            key: actor,
            controls: Some(prior_control),
            has_view: true,
            reset: false,
            attack_cooldown: 13,
            hurt_cooldown: 17,
            burn_cooldown: 19,
            oxygen: 211,
            peak_y: 9.75,
            exhaustion_milli: 1_250,
            saturation_milli: 8_500,
            since_damage_ticks: 71,
            drown_ticks: 23,
            starvation_ticks: 31,
            eating: Some(EatingProgress {
                slot: HotbarSlot::new(2).expect("eating slot"),
                item: 36,
                ticks: 11,
            }),
            bow: Some(BowProgress {
                slot: HotbarSlot::new(3).expect("bow slot"),
                ticks: 15,
            }),
            path: Some(PathState {
                generation: 2,
                target: BlockPos::new(4, 1, 0),
                revisions: vec![(overworld_key(BlockPos::new(0, 1, 0)), 3)],
                waypoints: vec![BlockPos::new(2, 1, 0)],
                cursor: 0,
                next_repath_tick: 29,
            }),
            aux: ActorAux::Player {
                respawn: Some((Dimension::DEPTHS, BlockPos::new(16, 64, 32))),
                workbench: Some(BlockPos::new(4, 1, 0)),
            },
        };
        context
            .stage(RuleEffect::Runtime(before.clone()))
            .expect("runtime preimage");
        let mining = MiningProgress {
            actor,
            dimension: Dimension::OVERWORLD,
            target: BlockPos::new(2, 1, 0),
            observed_block: STONE,
            tool_slot: HotbarSlot::new(0).unwrap(),
            tool: ItemStack::default(),
            elapsed: 3,
            required: 10,
            last_tick: 0,
        };
        context
            .stage(RuleEffect::Mining {
                actor,
                progress: Some(mining.clone()),
            })
            .unwrap();
        let mut expected_actor = context.read().actor(actor).unwrap().clone();
        let interrupted = incoming == Some(invalid);
        let expected = ActorRuntime {
            controls: expected_control,
            eating: if interrupted { None } else { before.eating },
            bow: if interrupted { None } else { before.bow },
            ..before
        };
        if let Some(input) = incoming {
            let envelope = envelope(session, 1, Command::PlayerInput(input));
            let admitted = provider::run(&mut context, intake_call(&envelope));
            assert_eq!(admitted.is_ok(), input != invalid, "{name} admission");
        }
        if incoming == Some(replacement) {
            expected_actor.look = LookAngles::try_new(-std::f32::consts::PI, 0.2).unwrap();
        } else if incoming == Some(neutral) {
            expected_actor.look = neutral.look();
        }
        assert_eq!(
            context.read().actor(actor),
            Some(&expected_actor),
            "{name}: intake preserves pose and body"
        );
        assert_eq!(
            context.read().runtime(actor),
            Some(&expected),
            "{name}: intake publishes complete runtime before actions"
        );
        assert_eq!(
            context.read().mining(actor),
            if interrupted { None } else { Some(&mining) },
            "{name}: only invalid input interrupts mining"
        );
        provider::run(&mut context, motion_call(actor)).expect("motion advance");
        assert_eq!(context.read().runtime(actor), Some(&expected), "{name}");
    }
}

/// A missing packet retains held movement; an explicit neutral packet clears
/// the movement intent, matching the Go held-input replay.
#[test]
fn live_tick_reuses_held_input_until_neutral_packet() {
    let mut state = authority();
    let session = state
        .admit(admitted(1, "Ada"), TransportKind::Memory)
        .expect("session");
    let mut context = harness_context(&mut state);
    let actor = motion_scene(&mut context, session, [0.5, 1.0, 0.5], [0.0; 3], true);
    context.preload_inventory(actor, InventoryRecord::empty());
    let residents = context.resident_snapshot();
    drop(context);
    state.commit_residents(residents);
    let held = control(1, 0, false, 0.0, 0.0);
    state
        .submit(
            session,
            mornlea_protocol::PlayIntent::Sequenced {
                sequence: 1,
                command: Command::PlayerInput(held),
            },
        )
        .expect("held input");
    state.advance_tick(TickBudget::full()).expect("first tick");
    let first = state.residents();
    let first_x = first.actors[0].motion.position().get()[0];
    assert!(first_x > 0.5, "held input moves on its first tick");
    state
        .advance_tick(TickBudget::full())
        .expect("packet-free tick");
    let second = state.residents();
    let second_x = second.actors[0].motion.position().get()[0];
    assert!(
        second_x > first_x,
        "the next tick keeps moving without a packet"
    );
    assert_eq!(second.runtimes[&actor].controls, Some(held));
    let neutral = control(0, 0, false, 0.0, 0.0);
    state
        .submit(
            session,
            mornlea_protocol::PlayIntent::Sequenced {
                sequence: 2,
                command: Command::PlayerInput(neutral),
            },
        )
        .expect("neutral input");
    state
        .advance_tick(TickBudget::full())
        .expect("neutral tick");
    let third = state.residents();
    let third_x = third.actors[0].motion.position().get()[0];
    assert!(
        third_x - second_x < second_x - first_x,
        "neutral input reduces the carried motion"
    );
    assert_eq!(third.runtimes[&actor].controls, Some(neutral));
}

/// Wall collision, fluid and fall behavior with latest-input-wins, each leg
/// compared against the source fixture's clipped pose and grounded state.
#[test]
fn wall_fluid_fall_latest_input() {
    // Wall plus latest-wins: an older westward input loses to the latest
    // eastward input, and the winner clips against the staged wall exactly like
    // the oracle: feet stop at x 0.7 (one half-width off the wall face), the
    // blocked axis velocity zeroes, and the actor stays grounded.
    let mut state = authority();
    let session = state
        .admit(admitted(1, "Ada"), TransportKind::Memory)
        .expect("session");
    let mut context = harness_context(&mut state);
    let actor = motion_scene(
        &mut context,
        session,
        [0.5, 1.0, 0.5],
        [4.3, 0.0, 0.0],
        true,
    );
    for y in 1..=2 {
        context.preload_block(observation(BlockPos::new(1, y, 0), STONE));
    }
    let older = envelope(
        session,
        1,
        Command::PlayerInput(control(-1, 0, false, 0.5, 0.0)),
    );
    assert!(
        applied(provider::run(&mut context, intake_call(&older)).expect("older intake staged")),
        "older intake reports applied"
    );
    let latest = envelope(
        session,
        2,
        Command::PlayerInput(control(1, 0, false, 0.0, 0.0)),
    );
    assert!(
        applied(provider::run(&mut context, intake_call(&latest)).expect("latest intake staged")),
        "latest intake reports applied"
    );
    let report = provider::run(&mut context, motion_call(actor)).expect("motion advanced");
    assert!(applied(report), "motion reports applied, got {report:?}");
    let staged = context.read().actor(actor).expect("actor").clone();
    assert_motion(
        staged.motion,
        [0x3f333333, 0x3f800000, 0x3f000000],
        [0x00000000, 0x00000000, 0x80000000],
        true,
    );
    assert_eq!(
        staged.look.yaw().to_bits(),
        0.0f32.to_bits(),
        "the staged look carries the latest input yaw"
    );
    assert!(context.events().is_empty());
    // Held-controls staging: the advance records the latest validated input
    // on the runtime record for the Interaction-phase sneak gate.
    let want = match latest.command() {
        Command::PlayerInput(control) => control,
        _ => panic!("latest envelope carries the winning input"),
    };
    assert_eq!(
        context.read().runtime(actor).expect("runtime").controls,
        Some(want),
        "the staged runtime carries the latest validated held controls"
    );

    // Fluid: the input carries no fluid flags, so the provider must derive body
    // submersion from the staged water itself (the Go caller computes
    // `SubmersionFlagsWithTunables` before stepping). The pose matches the
    // golden "fluid sink from rest" vector bitwise.
    let mut state = authority();
    let session = state
        .admit(admitted(2, "Bea"), TransportKind::Memory)
        .expect("session");
    let mut context = harness_context(&mut state);
    let actor = motion_scene(
        &mut context,
        session,
        [0.5, 8.0, 0.5],
        [0.0, 0.0, 0.0],
        false,
    );
    for y in 7..=9 {
        context.preload_block(observation(BlockPos::new(0, y, 0), WATER));
    }
    let neutral = envelope(
        session,
        1,
        Command::PlayerInput(control(0, 0, false, 0.0, 0.0)),
    );
    provider::run(&mut context, intake_call(&neutral)).expect("fluid intake staged");
    provider::run(&mut context, motion_call(actor)).expect("fluid advance");
    let staged = context.read().actor(actor).expect("actor").clone();
    assert_motion(
        staged.motion,
        [0x3f000000, 0x40ff7cee, 0x3f000000],
        [0x00000000, 0xbea3d70b, 0x00000000],
        false,
    );
    assert!(context.events().is_empty());

    // Fall: with no intake at all the advance uses neutral controls and gravity
    // applies for exactly one fixed step.
    let mut state = authority();
    let session = state
        .admit(admitted(3, "Cara"), TransportKind::Memory)
        .expect("session");
    let mut context = harness_context(&mut state);
    let actor = motion_scene(
        &mut context,
        session,
        [0.5, 5.0, 0.5],
        [0.0, 0.0, 0.0],
        false,
    );
    provider::run(&mut context, motion_call(actor)).expect("fall advance");
    let staged = context.read().actor(actor).expect("actor").clone();
    assert_motion(
        staged.motion,
        [0x3f000000, 0x409d70a4, 0x3f000000],
        [0x00000000, 0xbfcccccd, 0x00000000],
        false,
    );
    assert!(context.events().is_empty());
    // No intake means nothing held: the runtime stages cleared controls.
    assert_eq!(
        context.read().runtime(actor).expect("runtime").controls,
        None,
        "an advance without intake stages no held controls"
    );

    // No tunables snapshot, no advance: without a staged environment the
    // provider cannot snapshot `RuleTunables` and refuses with nothing staged.
    let mut state = authority();
    let session = state
        .admit(admitted(4, "Dan"), TransportKind::Memory)
        .expect("session");
    let mut context = harness_context(&mut state);
    let actor = ActorKey::Player(session);
    context
        .stage(RuleEffect::Actor(player_actor(
            session,
            [0.5, 1.0, 0.5],
            [0.0, 0.0, 0.0],
            true,
            ActorLifecycle::Active,
        )))
        .expect("actor");
    let before = probe(&context, actor, &[]);
    assert!(provider::run(&mut context, motion_call(actor)).is_err());
    assert_eq!(probe(&context, actor, &[]), before);
}

/// An invalid latest input clears previous movement and held controls while
/// pose and observations stay unchanged during intake, mirroring
/// `TestInvalidLatestInputIsAckedAndNeutral`: position kept, velocity zeroed,
/// and the following tick without input stays neutral.
#[test]
fn invalid_clears_held() {
    let cells = [BlockPos::new(0, 0, 0), BlockPos::new(0, 1, 0)];
    let mut state = authority();
    let session = state
        .admit(admitted(1, "Ada"), TransportKind::Memory)
        .expect("session");
    let mut context = harness_context(&mut state);
    let actor = motion_scene(
        &mut context,
        session,
        [0.5, 1.0, 0.5],
        [1.0, 0.0, 0.0],
        true,
    );
    let valid = envelope(
        session,
        1,
        Command::PlayerInput(control(1, 0, false, 0.0, 0.0)),
    );
    provider::run(&mut context, intake_call(&valid)).expect("valid intake staged");

    // The invalid intake interrupts action progress while retaining the pose,
    // cells and events. Invalid input is an admitted control tombstone.
    let before = probe(&context, actor, &cells);
    let invalid = envelope(
        session,
        2,
        Command::PlayerInput(control(2, 0, false, 0.0, 0.0)),
    );
    assert!(provider::run(&mut context, intake_call(&invalid)).is_err());
    assert_eq!(
        probe(&context, actor, &cells),
        before,
        "invalid intake leaves staged state unchanged"
    );

    // The advance then runs on the controls already cleared at intake: the staged velocity zeroes
    // while the position is kept, and the next advance without new input stays
    // neutral.
    provider::run(&mut context, motion_call(actor)).expect("cleared advance");
    let staged = context.read().actor(actor).expect("actor").clone();
    assert_motion(
        staged.motion,
        [0x3f000000, 0x3f800000, 0x3f000000],
        [0x00000000, 0x00000000, 0x00000000],
        true,
    );
    assert!(context.events().is_empty());
    // The invalid latest cleared the held controls, so the runtime stages no
    // control even though an older valid envelope was deferred first.
    assert_eq!(
        context.read().runtime(actor).expect("runtime").controls,
        None,
        "an invalid latest clears the staged held controls"
    );
    provider::run(&mut context, motion_call(actor)).expect("held advance");
    let held = context.read().actor(actor).expect("actor").clone();
    assert_motion(
        held.motion,
        [0x3f000000, 0x3f800000, 0x3f000000],
        [0x00000000, 0x00000000, 0x00000000],
        true,
    );

    // Validation boundary matrix from `TestPlayerInputValidationBoundaries`:
    // axes at -1/+1 and pitch at exactly +/-max stay valid, one step beyond on
    // any lane refuses. Yaw needs no intake gate: non-finite rotations are
    // unrepresentable in `LookAngles`, and yaw wraps through normalization.
    let mut state = authority();
    let session = state
        .admit(admitted(2, "Bea"), TransportKind::Memory)
        .expect("session");
    let mut context = harness_context(&mut state);
    let actor = motion_scene(
        &mut context,
        session,
        [0.5, 1.0, 0.5],
        [0.0, 0.0, 0.0],
        true,
    );
    let limit = max_pitch();
    let mut sequence = 1;
    for (name, input, valid) in [
        ("move_x low", control(-1, 0, false, 0.0, 0.0), true),
        ("move_z high", control(0, 1, false, 0.0, 0.0), true),
        ("pitch low", control(0, 0, false, 0.0, -limit), true),
        ("pitch high", control(0, 0, false, 0.0, limit), true),
        ("move_x below", control(-2, 0, false, 0.0, 0.0), false),
        ("move_x above", control(2, 0, false, 0.0, 0.0), false),
        ("move_z below", control(0, -2, false, 0.0, 0.0), false),
        ("move_z above", control(0, 2, false, 0.0, 0.0), false),
        (
            "pitch below",
            control(0, 0, false, 0.0, (-limit).next_down()),
            false,
        ),
        (
            "pitch above",
            control(0, 0, false, 0.0, limit.next_up()),
            false,
        ),
    ] {
        let intake = envelope(session, sequence, Command::PlayerInput(input));
        sequence += 1;
        let before = probe(&context, actor, &cells);
        let outcome = provider::run(&mut context, intake_call(&intake));
        assert_eq!(outcome.is_ok(), valid, "{name} validity");
        if !valid {
            assert_eq!(
                probe(&context, actor, &cells),
                before,
                "{name} leaves staged state unchanged"
            );
        }
    }

    // Yaw wraps through normalization (`normalizeYaw`): 3pi stages as -pi, and
    // the westward walk matches the oracle bitwise.
    let wrapped = envelope(
        session,
        sequence,
        Command::PlayerInput(control(1, 0, false, 3.0 * std::f32::consts::PI, 0.0)),
    );
    provider::run(&mut context, intake_call(&wrapped)).expect("wrapped yaw staged");
    provider::run(&mut context, motion_call(actor)).expect("wrapped advance");
    let staged = context.read().actor(actor).expect("actor").clone();
    assert_eq!(
        staged.look.yaw().to_bits(),
        (-std::f32::consts::PI).to_bits(),
        "yaw normalizes into [-pi, pi)"
    );
    assert_motion(
        staged.motion,
        [0x3ecccccd, 0x3f800000, 0x3f000000],
        [0xc0000000, 0x00000000, 0xb43bbd2e],
        true,
    );

    // Foreign shapes refuse without effect: a non-input command, an unknown
    // session, a missing actor, a wrong phase, and a companion actor key.
    let before = probe(&context, actor, &cells);
    let foreign = envelope(
        session,
        sequence + 1,
        Command::SelectHotbar(HotbarSlot::new(4).expect("slot")),
    );
    assert!(provider::run(&mut context, intake_call(&foreign)).is_err());
    let ghost = CommandEnvelope::try_new(CommandEnvelopeParts {
        tick: 0,
        session: 0,
        sequence: 1,
        arrival_index: 0,
        command: Command::PlayerInput(control(1, 0, false, 0.0, 0.0)),
    })
    .expect("ghost envelope");
    assert!(provider::run(&mut context, intake_call(&ghost)).is_err());
    assert!(
        provider::run(
            &mut context,
            RuleCall {
                phase: RulePhase::PlayerMotion,
                actor: None,
                command: None,
                internal: None,
            }
        )
        .is_err()
    );
    let companion = ActorKey::Companion(
        mornlea_domain::CompanionId::try_from_bytes(uuid(9)).expect("companion id"),
    );
    assert!(provider::run(&mut context, motion_call(companion)).is_err());
    let misplaced = envelope(
        session,
        sequence + 2,
        Command::PlayerInput(control(1, 0, false, 0.0, 0.0)),
    );
    assert!(
        provider::run(
            &mut context,
            RuleCall {
                phase: RulePhase::Publish,
                actor: Some(actor),
                command: Some(&misplaced),
                internal: None,
            }
        )
        .is_err()
    );
    assert_eq!(probe(&context, actor, &cells), before);

    // int32 edge mirror of the Go prism guard (`collisionCheckedFloor` in
    // `packages/shared/physics/collision.go`): a pose at 2^31 floors outside
    // the int32 span, so the advance refuses with staged state unchanged,
    // while the largest exactly representable f32 below it still advances.
    let mut state = authority();
    let session = state
        .admit(admitted(3, "Cara"), TransportKind::Memory)
        .expect("session");
    let mut context = harness_context(&mut state);
    let actor = motion_scene(
        &mut context,
        session,
        [2_147_483_648.0, 1.0, 0.5],
        [0.0, 0.0, 0.0],
        true,
    );
    let before = probe(&context, actor, &cells);
    assert_eq!(
        provider::run(&mut context, motion_call(actor)),
        Err(ServerError::InvalidInput { field: "actor" }),
        "an unrepresentable pose refuses as invalid input"
    );
    assert_eq!(
        probe(&context, actor, &cells),
        before,
        "an unrepresentable pose leaves staged state unchanged"
    );

    let mut state = authority();
    let session = state
        .admit(admitted(4, "Dan"), TransportKind::Memory)
        .expect("session");
    let mut context = harness_context(&mut state);
    let actor = motion_scene(
        &mut context,
        session,
        [2_147_483_392.0, 1.0, 0.5],
        [0.0, 0.0, 0.0],
        true,
    );
    provider::run(&mut context, motion_call(actor)).expect("edge pose advances");
    assert_eq!(
        context.read().runtime(actor).expect("runtime").controls,
        None,
        "an edge advance without intake stages no held controls"
    );
}

/// Two sessions settle in arrival order and deterministically: swapping the
/// intake arrival order while advancing actors in ascending session order
/// produces bitwise-identical poses.
#[test]
fn two_player_arrival_order() {
    let run_arrival = |first_b: bool| {
        let mut state = authority();
        let session_a = state
            .admit(admitted(1, "Ada"), TransportKind::Memory)
            .expect("session a");
        let session_b = state
            .admit(admitted(2, "Bea"), TransportKind::Memory)
            .expect("session b");
        assert!(
            session_a < session_b,
            "admission order names the sort order"
        );
        let mut context = harness_context(&mut state);
        let actor_a = motion_scene(
            &mut context,
            session_a,
            [0.5, 1.0, 0.5],
            [0.0, 0.0, 0.0],
            true,
        );
        context
            .stage(RuleEffect::Actor(player_actor(
                session_b,
                [2.5, 1.0, 0.5],
                [0.0, 0.0, 0.0],
                true,
                ActorLifecycle::Active,
            )))
            .expect("actor b");
        let actor_b = ActorKey::Player(session_b);
        let intake_a = envelope(
            session_a,
            1,
            Command::PlayerInput(control(1, 0, false, 0.0, 0.0)),
        );
        let intake_b = envelope(
            session_b,
            1,
            Command::PlayerInput(control(-1, 0, false, 0.0, 0.0)),
        );
        if first_b {
            provider::run(&mut context, intake_call(&intake_b)).expect("b intake");
            provider::run(&mut context, intake_call(&intake_a)).expect("a intake");
        } else {
            provider::run(&mut context, intake_call(&intake_a)).expect("a intake");
            provider::run(&mut context, intake_call(&intake_b)).expect("b intake");
        }
        let mut order = [actor_a, actor_b];
        order.sort_by_key(|key| match key {
            ActorKey::Player(session) => session.get(),
            _ => u64::MAX,
        });
        for actor in order {
            provider::run(&mut context, motion_call(actor)).expect("motion advanced");
        }
        assert!(context.events().is_empty());
        let view = context.read();
        (
            view.actor(actor_a).expect("a").motion,
            view.actor(actor_b).expect("b").motion,
        )
    };

    let (a_first, b_first) = run_arrival(false);
    assert_motion(
        a_first,
        [0x3f19999a, 0x3f800000, 0x3f000000],
        [0x40000000, 0x00000000, 0x00000000],
        true,
    );
    assert_motion(
        b_first,
        [0x4019999a, 0x3f800000, 0x3f000000],
        [0xc0000000, 0x00000000, 0x00000000],
        true,
    );
    let (a_second, b_second) = run_arrival(true);
    assert_eq!(a_second, a_first, "a is arrival-order independent");
    assert_eq!(b_second, b_first, "b is arrival-order independent");
}

/// A stale-sequence intent changes nothing: an older sequence and a duplicate
/// redelivery report rejected with staged state untouched, and the advance
/// still runs on the latest input.
#[test]
fn stale_sequence_no_effect() {
    let cells = [BlockPos::new(0, 0, 0), BlockPos::new(0, 1, 0)];
    let mut state = authority();
    let session = state
        .admit(admitted(1, "Ada"), TransportKind::Memory)
        .expect("session");
    let mut context = harness_context(&mut state);
    let actor = motion_scene(
        &mut context,
        session,
        [0.5, 1.0, 0.5],
        [0.0, 0.0, 0.0],
        true,
    );
    let fresh = envelope(
        session,
        5,
        Command::PlayerInput(control(1, 0, false, 0.0, 0.0)),
    );
    assert!(
        applied(provider::run(&mut context, intake_call(&fresh)).expect("fresh staged")),
        "fresh intake reports applied"
    );

    let before = probe(&context, actor, &cells);
    let stale = envelope(
        session,
        3,
        Command::PlayerInput(control(-1, 0, false, 0.0, 0.0)),
    );
    let report = provider::run(&mut context, intake_call(&stale)).expect("stale skipped");
    assert_eq!(
        report,
        PhaseReport {
            examined: 1,
            applied: 0,
            carried: 0,
            rejected: 1,
        },
        "stale intake reports rejected"
    );
    assert_eq!(probe(&context, actor, &cells), before);
    let duplicate = envelope(
        session,
        5,
        Command::PlayerInput(control(1, 0, false, 0.0, 0.0)),
    );
    let report = provider::run(&mut context, intake_call(&duplicate)).expect("duplicate skipped");
    assert_eq!(report.rejected, 1, "duplicate reports rejected");
    assert_eq!(probe(&context, actor, &cells), before);

    provider::run(&mut context, motion_call(actor)).expect("motion advanced");
    let staged = context.read().actor(actor).expect("actor").clone();
    assert_motion(
        staged.motion,
        [0x3f19999a, 0x3f800000, 0x3f000000],
        [0x40000000, 0x00000000, 0x00000000],
        true,
    );

    // A player that is not active owns no motion: intake clears without effect
    // and the advance refuses, both leaving staged state untouched.
    let mut state = authority();
    let session = state
        .admit(admitted(2, "Bea"), TransportKind::Memory)
        .expect("session");
    let mut context = harness_context(&mut state);
    let actor = ActorKey::Player(session);
    context
        .stage(RuleEffect::Environment(environment()))
        .expect("environment");
    context
        .stage(RuleEffect::Actor(player_actor(
            session,
            [0.5, 1.0, 0.5],
            [0.0, 0.0, 0.0],
            true,
            ActorLifecycle::Pending,
        )))
        .expect("actor");
    context.preload_block(observation(BlockPos::new(0, 0, 0), GRASS));
    let before = probe(&context, actor, &cells);
    let pending = envelope(
        session,
        1,
        Command::PlayerInput(control(1, 0, false, 0.0, 0.0)),
    );
    assert!(provider::run(&mut context, intake_call(&pending)).is_err());
    assert_eq!(probe(&context, actor, &cells), before);
    assert!(provider::run(&mut context, motion_call(actor)).is_err());
    assert_eq!(probe(&context, actor, &cells), before);
}

#[test]
fn motion_fluid_minimum_refuses_without_effect_and_nearby_succeeds() {
    for (x, refused) in [
        (i32::MIN as f32, true),
        (i32::MIN as f32 + 256.0, false),
        (0.3, false),
    ] {
        let mut state = authority();
        let session = state
            .admit(admitted(1, "Ada"), TransportKind::Memory)
            .unwrap();
        let mut context = harness_context(&mut state);
        let actor = motion_scene(&mut context, session, [x, 1.0, 0.5], [0.0; 3], true);
        let before = {
            let resident = context.resident_snapshot();
            (
                resident.actors,
                resident.runtimes,
                resident.inventories,
                resident.projectiles,
                resident.blocks,
            )
        };
        let events = context.events().to_vec();
        let result = provider::run(&mut context, motion_call(actor));
        if refused {
            assert_eq!(result, Err(ServerError::InvalidInput { field: "actor" }));
            assert_eq!(
                {
                    let resident = context.resident_snapshot();
                    (
                        resident.actors,
                        resident.runtimes,
                        resident.inventories,
                        resident.projectiles,
                        resident.blocks,
                    )
                },
                before
            );
            assert_eq!(context.events(), events);
        } else {
            assert!(applied(result.unwrap()));
        }
    }
}

#[test]
fn malformed_intake_and_missing_session_leave_state_unchanged() {
    let mut state = authority();
    let session = state
        .admit(admitted(1, "Ada"), TransportKind::Memory)
        .unwrap();
    let mut context = harness_context(&mut state);
    let actor = motion_scene(&mut context, session, [0.5, 1.0, 0.5], [0.0; 3], true);
    provider::run(&mut context, motion_call(actor)).unwrap();
    let valid = envelope(
        session,
        1,
        Command::PlayerInput(control(0, 0, false, 0.0, 0.0)),
    );
    let foreign = envelope(
        session,
        1,
        Command::SelectHotbar(HotbarSlot::new(0).unwrap()),
    );
    let absent = CommandEnvelope::try_new(CommandEnvelopeParts {
        tick: 0,
        session: 999,
        sequence: 1,
        arrival_index: 0,
        command: valid.command(),
    })
    .unwrap();
    let zero = CommandEnvelope::try_new(CommandEnvelopeParts {
        tick: 0,
        session: 0,
        sequence: 1,
        arrival_index: 0,
        command: valid.command(),
    })
    .unwrap();
    for call in [
        RuleCall {
            phase: RulePhase::PlayerCommand,
            actor: None,
            command: None,
            internal: None,
        },
        RuleCall {
            phase: RulePhase::PlayerCommand,
            actor: Some(actor),
            command: Some(&valid),
            internal: None,
        },
        intake_call(&foreign),
        intake_call(&absent),
        intake_call(&zero),
    ] {
        let before = context.resident_snapshot();
        let events = context.events().to_vec();
        let deferred = context.deferred(RulePhase::PlayerMotion);
        assert!(provider::run(&mut context, call).is_err());
        let after = context.resident_snapshot();
        assert_eq!(after.actors, before.actors);
        assert_eq!(after.runtimes, before.runtimes);
        assert_eq!(after.inventories, before.inventories);
        assert_eq!(after.mining, before.mining);
        assert_eq!(after.projectiles, before.projectiles);
        assert_eq!(after.blocks, before.blocks);
        assert_eq!(context.events(), events);
        assert_eq!(context.deferred(RulePhase::PlayerMotion), deferred);
    }
}

fn with_hunger(mut actor: ActorRecord, hunger: u8) -> ActorRecord {
    actor.survival = SurvivalState::try_new(SurvivalStateParts {
        health: actor.survival.health(),
        oxygen: actor.survival.oxygen(),
        hunger,
        saturation_zero: actor.survival.saturation_zero(),
        armor_points: actor.survival.armor_points(),
    })
    .unwrap();
    let ActorBody::Player(body) = &mut actor.body else {
        unreachable!()
    };
    body.hunger = hunger;
    actor
}

fn sprint_control(sneaking: bool) -> PlayerControl {
    let ordinary = control(0, 1, false, 0.0, 0.0);
    PlayerControl::new(PlayerControlParts {
        movement: ordinary.movement(),
        look: ordinary.look(),
        actions: HeldActions {
            sprinting: true,
            sneaking,
            ..ordinary.actions()
        },
    })
}

#[test]
fn fresh_sprint_packet_obeys_hunger_and_sneak_gates() {
    // Start at walking speed so one fixed step can reach either target;
    // acceleration from rest would hide both targets behind the same cap.
    let mut actual = Vec::new();
    let mut expected = Vec::new();
    for (hunger, sneaking, speed) in [
        (5, false, 4.3f32),
        (6, false, 4.3f32 * 1.3),
        (20, true, 4.3f32 * 0.3),
    ] {
        let mut state = authority();
        let session = state
            .admit(admitted(1, "Ada"), TransportKind::Memory)
            .unwrap();
        let mut context = harness_context(&mut state);
        let actor = motion_scene(
            &mut context,
            session,
            [0.5, 1.0, 0.5],
            if sneaking { [0.0; 3] } else { [0.0, 0.0, -4.3] },
            true,
        );
        let record = with_hunger(context.read().actor(actor).unwrap().clone(), hunger);
        context.stage(RuleEffect::Actor(record)).unwrap();
        let raw = sprint_control(sneaking);
        let input = envelope(session, 1, Command::PlayerInput(raw));
        provider::run(&mut context, intake_call(&input)).unwrap();
        let before = context.read().runtime(actor).unwrap().clone();
        provider::run(&mut context, motion_call(actor)).unwrap();
        actual.push((
            hunger,
            sneaking,
            context.read().actor(actor).unwrap().motion.velocity().get()[2].to_bits(),
        ));
        expected.push((hunger, sneaking, (-speed).to_bits()));
        assert_eq!(
            context.read().runtime(actor),
            Some(&before),
            "motion retains raw intent and sibling lanes"
        );
    }
    assert_eq!(actual, expected);
}

#[test]
fn live_sprint_intent_resumes_when_hunger_recovers_without_packet() {
    let mut state = authority();
    let session = state
        .admit(admitted(1, "Ada"), TransportKind::Memory)
        .unwrap();
    let mut context = harness_context(&mut state);
    let actor = motion_scene(
        &mut context,
        session,
        [0.5, 1.0, 0.5],
        [0.0, 0.0, -4.3],
        true,
    );
    let record = with_hunger(context.read().actor(actor).unwrap().clone(), 5);
    context.stage(RuleEffect::Actor(record)).unwrap();
    context.preload_inventory(actor, InventoryRecord::empty());
    let residents = context.resident_snapshot();
    drop(context);
    state.commit_residents(residents);
    let raw = sprint_control(false);
    state
        .submit(
            session,
            mornlea_protocol::PlayIntent::Sequenced {
                sequence: 1,
                command: Command::PlayerInput(raw),
            },
        )
        .unwrap();
    state.advance_tick(TickBudget::full()).unwrap();
    let first_speed = state.residents().actors[0].motion.velocity().get()[2];
    let first_controls = state.residents().runtimes.get(&actor).unwrap().controls;
    let mut recovered = state.residents().clone();
    recovered.actors[0] = with_hunger(recovered.actors[0].clone(), 6);
    state.commit_residents(recovered);
    state.advance_tick(TickBudget::full()).unwrap();
    let second_speed = state.residents().actors[0].motion.velocity().get()[2];
    let second_controls = state.residents().runtimes.get(&actor).unwrap().controls;
    assert_eq!(
        (
            first_speed.to_bits(),
            second_speed.to_bits(),
            first_controls,
            second_controls
        ),
        (
            (-4.3f32).to_bits(),
            (-(4.3f32 * 1.3)).to_bits(),
            Some(raw),
            Some(raw)
        )
    );
}

#[test]
fn reset_motion_preserves_pose_and_all_sibling_runtime_lanes() {
    let mut state = authority();
    let session = state
        .admit(admitted(1, "Ada"), TransportKind::Memory)
        .unwrap();
    let mut context = harness_context(&mut state);
    let actor = motion_scene(
        &mut context,
        session,
        [i32::MIN as f32, 1.0, 0.5],
        [4.0, -3.0, 2.0],
        false,
    );
    let before_actor = context.read().actor(actor).unwrap().clone();
    let before = ActorRuntime {
        key: actor,
        controls: Some(sprint_control(false)),
        has_view: true,
        reset: true,
        attack_cooldown: 13,
        hurt_cooldown: 17,
        burn_cooldown: 19,
        oxygen: 211,
        peak_y: 77.0,
        exhaustion_milli: 1_250,
        saturation_milli: 8_500,
        since_damage_ticks: 71,
        drown_ticks: 23,
        starvation_ticks: 31,
        eating: Some(EatingProgress {
            slot: HotbarSlot::new(2).unwrap(),
            item: 36,
            ticks: 11,
        }),
        bow: Some(BowProgress {
            slot: HotbarSlot::new(3).unwrap(),
            ticks: 15,
        }),
        path: None,
        aux: ActorAux::Player {
            respawn: Some((Dimension::DEPTHS, BlockPos::new(16, 64, 32))),
            workbench: Some(BlockPos::new(4, 1, 0)),
        },
    };
    context.stage(RuleEffect::Runtime(before.clone())).unwrap();
    let fresh = control(-1, 0, true, 1.234, 0.321);
    context
        .defer(
            envelope(session, 1, Command::PlayerInput(fresh)),
            RulePhase::PlayerMotion,
        )
        .unwrap();
    provider::run(&mut context, motion_call(actor)).unwrap();
    assert_eq!(
        context.read().actor(actor),
        Some(&before_actor),
        "reset bypasses even unrepresentable geometry without updating look"
    );
    assert_eq!(
        context.read().runtime(actor),
        Some(&ActorRuntime {
            controls: Some(fresh),
            ..before
        })
    );
    assert!(context.events().is_empty());
}
