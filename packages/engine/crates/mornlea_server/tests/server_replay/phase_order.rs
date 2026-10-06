//! Serial reducer order pins.
//!
//! The reducer's contract is call order: every provider runs in the frozen Go
//! order with retired-session filtering, explicit damage-victim routing and
//! bounded scopes. Two layers pin it here. The source guard below asserts the
//! exact dispatch call sequence in the reducer source the way the Go
//! step-sequence oracle pins its own call order, so a swapped row fails
//! loudly; the negative control proves the guard bites. The behavioral cases
//! prove the sorted mailbox walk the order depends on: swapped arrivals still
//! dispatch in sequence and budget pressure carries across ticks.
//!
//! The four named row tests assert the exact provider subsequence each row
//! depends on. Actual source-player death recipes in persistence failure
//! execute five damage producers and the consumer between projectile and
//! passive advancement. Actual native landing and top-floor Safe recipes qualify
//! per-player checkpoint after fall settlement and before late death. Trample markers
//! pin capture before Safe and copied-cell settlement before legacy collection. These
//! Snow markers pin its retained player capture and bounded copied-cell settlement. These
//! source guards separately qualify call order; other provider integration
//! coverage retains its existing owners.
//!
//! The closing case is different: a placement and a tilling stage real dirt
//! through providers the test drives directly on a seeded context, and the
//! reducer's own dirty-driven derivation plus the fluid and support bodies
//! prove the same-tick cascade genuinely. No endpoint seeding is involved;
//! the row order around it stays with the source guard above.

use std::collections::BTreeSet;

use mornlea_domain::{
    BlockPos, ChunkPos, Command, CommandEnvelope, CommandEnvelopeParts, Dimension, FiniteVec3,
    LookAngles, MotionState, MotionStateParts, PlacementIntent, PlayerId, SurvivalState,
    SurvivalStateParts, Weather,
};
use mornlea_protocol::{LoginStart, PlayIntent, admit_login};
use mornlea_server::contracts::{
    ActorBody, ActorKey, ActorLifecycle, ActorRecord, BlockObservation, ChunkKey, EnvironmentState,
    InventoryRecord, RuleCall, RuleEffect, RulePhase, RuleTunables, ServerLimits, SessionKey,
    TickBudget, TransportKind,
};
use mornlea_server::core::step;
use mornlea_server::rules::{farmland, fluids, supports, tools, world_mutation};
use mornlea_server::state::{AuthorityState, TickContext};
use mornlea_storage::{ItemStack, PlayerLocation, PlayerSave};

fn authority() -> AuthorityState {
    AuthorityState::try_new(
        ServerLimits::try_new(8, 4096, 512, 64, 64, 1_048_576).unwrap(),
        7,
    )
    .unwrap()
}

fn login(tag: u8, name: &str) -> mornlea_protocol::AdmittedLogin {
    let mut bytes = [0u8; 16];
    bytes[0] = tag;
    bytes[6] = 0x40;
    bytes[8] = 0x80;
    let id = PlayerId::try_from_bytes(bytes).unwrap();
    let start = LoginStart::new(id, name, 8).unwrap();
    let inbound = LoginStart::decode_inbound(&start.encode().unwrap()).unwrap();
    admit_login(inbound).unwrap()
}

fn sequenced(sequence: u64) -> PlayIntent {
    PlayIntent::Sequenced {
        sequence,
        command: Command::CloseContainer,
    }
}

fn tiny_commands() -> TickBudget {
    TickBudget::try_new(1, 0, 0, 0, 0).unwrap()
}

/// Reducer source with line comments and import statements stripped. The
/// guard below asserts dispatch call order, and imports carry no dispatch
/// semantics; both are noise the scanner drops. The reducer carries no web
/// addresses or comment-like sequences in literals, so the plain cuts keep
/// every real call site while silencing prose.
fn code_without_comments(source: &str) -> String {
    let mut code = String::with_capacity(source.len());
    let mut in_import = false;
    for line in source.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("use ") {
            in_import = !trimmed.contains(';');
            continue;
        }
        if in_import {
            in_import = !trimmed.contains(';');
            continue;
        }
        let cut = line
            .find("//")
            .map(|i| line[..i].trim_end())
            .unwrap_or(line);
        code.push_str(cut);
        code.push('\n');
    }
    code
}

fn step_source() -> String {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/src/core/step.rs");
    let source = std::fs::read_to_string(path).expect("reducer source");
    code_without_comments(&source)
}

/// Body of one reducer function by brace balance. Each dispatch chain below
/// is scoped to the function that owns it, so helper placement elsewhere in
/// the file can never disturb the order pins.
fn fn_body<'a>(code: &'a str, name: &str) -> &'a str {
    let head = code
        .find(name)
        .unwrap_or_else(|| panic!("reducer fn missing: {name}"));
    let open = code[head..].find('{').expect("fn body") + head;
    let mut depth = 0;
    for (offset, grain) in code[open..].char_indices() {
        if grain == '{' {
            depth += 1;
        } else if grain == '}' {
            depth -= 1;
            if depth == 0 {
                return &code[open..open + offset + 1];
            }
        }
    }
    panic!("unbalanced fn body: {name}");
}

/// Positions of one ordered marker chain inside the code. Every marker must
/// appear after the previous one; a missing or swapped marker fails with the
/// exact break point.
fn chain_positions(code: &str, markers: &[&str]) -> Vec<usize> {
    let mut positions = Vec::with_capacity(markers.len());
    let mut cursor = 0;
    for marker in markers {
        let rest = &code[cursor..];
        let offset = rest
            .find(marker)
            .unwrap_or_else(|| panic!("dispatch marker missing: {marker}"));
        cursor += offset + marker.len();
        positions.push(cursor);
    }
    positions
}

/// Counts of one marker inside the code.
fn marker_count(code: &str, marker: &str) -> usize {
    code.match_indices(marker).count()
}

/// The endpoint wrapper order: mailbox drain, companion feed, environment
/// freeze, the row dispatch, then the viewer commit and the single
/// publication. The closing commit and publication follow the climate close
/// because both need the authority borrow the dispatch context holds.
const WRAPPER_CHAIN: &[&str] = &[
    "drain_mailbox",
    "drain_companions",
    "freeze_environment",
    "push_companion_action",
    "dispatch_rows",
    "commit_viewers",
    "publish_tick",
];

/// The mailbox composition: eligible freeze then the domain sort. The
/// budget split and watermark walk have no provider call sites of their
/// own; the behavioral cases below pin their accounting.
const MAILBOX_CHAIN: &[&str] = &["freeze_eligible", "order_commands"];

/// Lifecycle commands settle before the ordinary intake chain below.
/// Other envelopes retain motion, inventory, container and crafting admission.
const ADMIT_CHAIN: &[&str] = &[
    "source_player_command_ready",
    "record_player_input",
    "player_motion::run",
    "inventory::run",
    "containers::run",
    "crafting::run",
];

/// The interaction gates in loop order: placement and door geometry, tools
/// and buckets, then panel drops.
const GATE_CHAIN: &[&str] = &["world_mutation::run", "tools::run", "drops::run"];

/// The full frozen dispatch chain in source order: intake loop with bed
/// collection, intent, acquire, the per-actor survival line, companion
/// motion, hostile planning through player deaths, passive deaths, companion
/// placement, the single ordered interaction loop with its door arm and
/// sequence-ordered bed entries, sleep settlement, drops, furnaces, the world
/// sweep row, container moves, mining, workbench lifecycle, support and the
/// single climate close.
const DISPATCH_CHAIN: &[&str] = &[
    "admit_command",
    "companions::run",
    "world_acquisition::run",
    "source_companion_restore::advance",
    "source_player_restore::advance",
    "player_survival::run",
    "eating::run",
    "projectiles::run",
    "source_player_restore::recover",
    "player_survival::run",
    "player_motion::run",
    "player_survival::run",
    "source_player_restore::capture_trample",
    "source_player_restore::capture_snow",
    "source_player_restore::checkpoint_safe",
    "companions::run",
    "hostile_actions::plan",
    "hostile_actions::apply",
    "hostile_actors::run",
    "hostile_outcomes::advance",
    "hostile_actors::run",
    "projectiles::advance",
    "hostile_outcomes::run",
    "source_player_restore::settle_deaths",
    "passives::run",
    "companions::run",
    "route_interaction",
    "source_player_restore::settle_action_costs",
    "route_door",
    "sleep::enter",
    "sleep::settle",
    "drops::run",
    "drops::advance",
    "furnaces::run",
    "furnaces::advance",
    "fluids::run",
    "fluids::rescan",
    "fluids::run",
    "fluids::update",
    "farmland::run",
    "farmland::advance",
    "crops::run",
    "source_player_restore::settle_tramples",
    "crops::settle_tramples",
    "crops::run",
    "source_player_restore::settle_snow",
    "crops::settle_snow_footprints",
    "random_blocks::run",
    "random_blocks::advance",
    "containers::drain_commands",
    "mining::run",
    "source_player_restore::settle_action_costs",
    "crafting::run",
    "supports::run",
    "environment::run",
];

#[test]
fn dispatch_chain_runs_in_frozen_order() {
    let code = step_source();
    chain_positions(fn_body(&code, "fn reduce_tick_inner"), WRAPPER_CHAIN);
    chain_positions(fn_body(&code, "fn drain_mailbox"), MAILBOX_CHAIN);
    chain_positions(fn_body(&code, "fn admit_command"), ADMIT_CHAIN);
    chain_positions(fn_body(&code, "fn route_interaction"), GATE_CHAIN);
    chain_positions(fn_body(&code, "fn dispatch_rows"), DISPATCH_CHAIN);
}

/// Swaps the first occurrences of two markers, keeping both present so the
/// control probes order sensitivity rather than mere presence.
fn swap_markers(code: &str, first: &str, second: &str) -> String {
    let early = code.find(first).expect("early marker");
    let late = code.find(second).expect("late marker");
    assert!(early < late, "the real source is not swapped");
    let mut swapped = code.to_owned();
    swapped.replace_range(late..late + second.len(), first);
    swapped.replace_range(early..early + first.len(), second);
    swapped
}

#[test]
fn dispatch_guard_rejects_a_swapped_row() {
    // The negative control mirrors the Go swapped-order oracle: a dispatch
    // body with the random sweep ahead of the moisture row must not satisfy
    // the moisture-before-random probe, proving the guard above actually
    // bites instead of passing any source.
    let code = step_source();
    let body = fn_body(&code, "fn dispatch_rows").to_owned();
    let swapped = swap_markers(&body, "farmland::advance", "random_blocks::advance");
    let probe = ["farmland::advance", "random_blocks::advance"];
    let outcome = std::panic::catch_unwind(|| chain_positions(&swapped, &probe));
    assert!(
        outcome.is_err(),
        "a swapped row must break the frozen chain"
    );
}

#[test]
fn environment_close_and_publication_run_exactly_once() {
    let code = step_source();
    assert_eq!(
        marker_count(&code, "environment::run"),
        1,
        "the climate close runs once per tick"
    );
    assert_eq!(
        marker_count(&code, "publish_tick"),
        1,
        "the tick publishes once"
    );
    assert_eq!(
        marker_count(&code, "commit_viewers"),
        1,
        "the viewer overlay commits once at the close"
    );
}

#[test]
fn combat_projectile_death_sandwich() {
    // A projectile kill settles after the projectile phase and before the
    // passive phase, with hostile player deaths closed in between: combat,
    // then flight, then player deaths, then passives.
    let code = step_source();
    let body = fn_body(&code, "fn dispatch_rows");
    chain_positions(
        body,
        &[
            "hostile_outcomes::advance",
            "projectiles::advance",
            "hostile_outcomes::run",
            "passives::run",
        ],
    );
}

#[test]
fn eating_before_motion() {
    // Hunger settles before the physics step every tick: the eating call
    // precedes the motion call on the per-actor line, and the whole intake
    // loop precedes that line.
    let code = step_source();
    let body = fn_body(&code, "fn dispatch_rows");
    chain_positions(
        body,
        &["admit_command", "eating::run", "player_motion::run"],
    );
}

#[test]
fn deferred_interactions_keep_order() {
    // One deferred loop merges every interaction bag by envelope sequence in
    // the original sorted order, routing each envelope to the single
    // provider whose gate accepts it.
    let code = step_source();
    assert_eq!(
        marker_count(&code, "deferred(RulePhase::Interaction)"),
        1,
        "exactly one loop drains the interaction bag"
    );
    let body = fn_body(&code, "fn dispatch_rows");
    chain_positions(
        body,
        &[
            "companions::run",
            "route_interaction",
            "route_door",
            "sleep::enter",
            "sleep::settle",
        ],
    );
    chain_positions(
        fn_body(&code, "fn route_interaction"),
        &["world_mutation::run", "tools::run", "drops::run"],
    );
    // Door toggles settle through placement geometry in the row loop.
    chain_positions(fn_body(&code, "fn route_door"), &["world_mutation::run"]);
}

#[test]
fn containers_before_mining_after_random() {
    // The container drain settles before mining progress, and both run after
    // the random-block sweep closed the world row.
    let code = step_source();
    let body = fn_body(&code, "fn dispatch_rows");
    chain_positions(
        body,
        &[
            "random_blocks::advance",
            "containers::drain_commands",
            "mining::run",
        ],
    );
}

#[test]
fn swapped_arrival_order_still_dispatches_in_sequence() {
    let mut state = authority();
    let key = state
        .admit(login(31, "Swapped"), TransportKind::Memory)
        .unwrap();
    state
        .submit(
            key,
            PlayIntent::Sequenced {
                sequence: 2,
                command: Command::CloseContainer,
            },
        )
        .unwrap();
    state.submit(key, sequenced(1)).unwrap();
    let first = state.advance_tick(tiny_commands()).unwrap();
    assert_eq!(first.counters.commands, 2, "both envelopes drain");
    assert_eq!(
        first.counters.carried, 1,
        "sequence 2 carries past its budget"
    );
    assert_eq!(first.counters.stale, 0);
    let second = state.advance_tick(tiny_commands()).unwrap();
    assert_eq!(second.counters.commands, 1);
    assert_eq!(second.counters.carried, 0);
    assert_eq!(
        second.counters.stale, 0,
        "sequence 1 is not stale: the sorted walk admitted it first"
    );
}

#[test]
fn budget_carry_over_two_ticks() {
    let mut state = authority();
    let key = state
        .admit(login(32, "Carry"), TransportKind::Memory)
        .unwrap();
    for sequence in [1u64, 2, 3] {
        state.submit(key, sequenced(sequence)).unwrap();
    }
    let first = state.advance_tick(tiny_commands()).unwrap();
    assert_eq!(first.counters.commands, 3, "every envelope drains once");
    assert_eq!(first.counters.carried, 2);
    let second = state.advance_tick(tiny_commands()).unwrap();
    assert_eq!(second.counters.commands, 2, "the carried pair drains again");
    assert_eq!(second.counters.carried, 1);
    assert_eq!(second.counters.stale, 0);
    let third = state.advance_tick(tiny_commands()).unwrap();
    assert_eq!(third.counters.commands, 1);
    assert_eq!(third.counters.carried, 0);
    assert_eq!(third.counters.stale, 0);
}

// Stable block numbers mirrored from the frozen const block in
// `packages/shared/core/block.go`.
const AIR: u16 = 0;
const STONE: u16 = 2;
const DIRT: u16 = 3;
const FARMLAND_DRY: u16 = 35;
const SHORT_GRASS: u16 = 84;
// Stable item numbers mirrored from `packages/shared/core/item.go`.
const ITEM_DIRT: u16 = 2;
const ITEM_STONE_HOE: u16 = 30;

fn uuid(tag: u8) -> [u8; 16] {
    let mut bytes = [0u8; 16];
    bytes[0] = tag.max(1);
    bytes[6] = 0x40;
    bytes[8] = 0x80;
    bytes
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

fn player_actor(session: SessionKey, position: [f32; 3], yaw: f32, pitch: f32) -> ActorRecord {
    ActorRecord::try_new(
        ActorKey::Player(session),
        ActorLifecycle::Active,
        Dimension::OVERWORLD,
        MotionState::new(MotionStateParts {
            position: FiniteVec3::try_new(position).expect("position"),
            velocity: FiniteVec3::try_new([0.0, 0.0, 0.0]).expect("velocity"),
            on_ground: true,
        }),
        LookAngles::try_new(yaw, pitch).expect("look"),
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

fn overworld_key(pos: BlockPos) -> ChunkKey {
    ChunkKey {
        dimension: Dimension::OVERWORLD,
        pos: ChunkPos::new(pos.x() >> 4, pos.z() >> 4),
    }
}

fn observation(pos: BlockPos, block: u16) -> BlockObservation {
    BlockObservation::try_new(overworld_key(pos), 1, 1, pos, block).expect("block observation")
}

fn south_look() -> LookAngles {
    LookAngles::try_new(std::f32::consts::PI, 0.0).expect("look")
}

fn east_look() -> LookAngles {
    LookAngles::try_new(-std::f32::consts::PI / 2.0, 0.0).expect("look")
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

fn interaction_call(envelope: &CommandEnvelope) -> RuleCall<'_> {
    RuleCall {
        phase: RulePhase::Interaction,
        actor: None,
        command: Some(envelope),
        internal: None,
    }
}

fn support_call() -> RuleCall<'static> {
    RuleCall {
        phase: RulePhase::Support,
        actor: None,
        command: None,
        internal: None,
    }
}

/// A placed block reaches fluid evaluation and support sweeping in the same
/// tick: the placement stages dirt, the reducer's dirty-driven derivation
/// queues the cell plus its six neighbors for fluid update while firing no
/// moisture arm for plain dirt, the update examines every queued cell
/// without touching the solid, and the support sweep removes the wild grass
/// the fresh dirt no longer holds. A tilled cell in the same scene proves
/// the moisture arm genuinely: new farmland queues one candidate the
/// moisture pass examines.
#[test]
fn place_reaches_fluid_and_support_same_tick() {
    let placed = BlockPos::new(0, 65, 1);
    let tilled = BlockPos::new(2, 65, 0);
    let mut state = authority();
    let session = state
        .admit(login(33, "Place"), TransportKind::Memory)
        .unwrap();
    let mut context = TickContext::harness(&mut state, TickBudget::full());
    let actor = ActorKey::Player(session);
    context
        .stage(RuleEffect::Environment(environment()))
        .expect("environment");
    context
        .stage(RuleEffect::Actor(player_actor(
            session,
            [0.5, 64.0, 0.5],
            std::f32::consts::PI,
            0.0,
        )))
        .expect("actor");
    let mut inventory = InventoryRecord::empty();
    inventory.slots[0] = ItemStack {
        item: ITEM_STONE_HOE,
        count: 1,
        durability: 2,
    };
    inventory.slots[1] = ItemStack {
        item: ITEM_DIRT,
        count: 1,
        durability: 0,
    };
    context.preload_inventory(actor, inventory);
    // Placement ray south: air, air, dirt face with stone below; the grass
    // above the target waits for the support sweep.
    for (pos, block) in [
        (BlockPos::new(0, 65, 0), AIR),
        (placed, AIR),
        (BlockPos::new(0, 66, 1), SHORT_GRASS),
        (BlockPos::new(0, 64, 1), STONE),
        (BlockPos::new(0, 65, 2), DIRT),
        // Tilling ray east, clear of the placement lane.
        (BlockPos::new(1, 65, 0), AIR),
        (tilled, DIRT),
        (BlockPos::new(2, 66, 0), AIR),
    ] {
        context.preload_block(observation(pos, block));
    }
    let place = envelope(
        session,
        1,
        Command::PlaceBlock(PlacementIntent::try_new(south_look(), 1).expect("placement intent")),
    );
    world_mutation::run(&mut context, interaction_call(&place)).expect("placement");
    let till = envelope(session, 2, Command::TillSoil(east_look()));
    tools::run(&mut context, interaction_call(&till)).expect("till");
    let changed = context.changed_blocks();
    assert_eq!(changed.len(), 2, "placement and tilling both stage dirt");
    assert!(
        changed
            .iter()
            .any(|cell| cell.pos == placed && cell.block == DIRT)
    );
    assert!(
        changed
            .iter()
            .any(|cell| cell.pos == tilled && cell.block == FARMLAND_DRY)
    );
    // Dirty-driven fluid queue: each changed cell plus its six neighbors,
    // sharing no cell here, all due now.
    let mut fluid = fluids::FluidSchedule::new();
    step::feed_fluid_schedule(&mut fluid, &context, 0);
    assert_eq!(
        fluid.pending_fluid(Dimension::OVERWORLD),
        14,
        "both dirt cells reach fluid evaluation with their neighborhoods"
    );
    // Only the fresh farmland fires a moisture arm; plain dirt queues
    // nothing, matching the source facade's two arms.
    let mut farm = farmland::FarmlandSchedule::new();
    step::feed_farmland_schedule(&mut farm, &context, 0);
    assert_eq!(farm.pending_candidates(Dimension::OVERWORLD), 1);
    let scope: BTreeSet<ChunkKey> = changed.iter().map(|cell| cell.key).collect();
    let update = fluids::update(&mut fluid, &mut context, 0, 5).expect("fluid update");
    assert_eq!(update.examined, 14);
    assert_eq!(update.applied, 0, "no fluid is present to flow");
    assert_eq!(
        context.read().block(Dimension::OVERWORLD, placed),
        Some(DIRT),
        "fluid evaluation leaves the solid placement untouched"
    );
    let moisture = farmland::advance(&mut farm, &mut context, &scope, 0).expect("moisture check");
    assert_eq!(moisture.examined, 1, "the fresh farmland is rechecked");
    let support = supports::run(&mut context, support_call()).expect("support sweep");
    assert_eq!(support.applied, 1, "the fresh dirt drops its grass");
    assert_eq!(
        context
            .read()
            .block(Dimension::OVERWORLD, BlockPos::new(0, 66, 1)),
        Some(AIR)
    );
    assert_eq!(
        context.read().block(Dimension::OVERWORLD, placed),
        Some(DIRT)
    );
}

#[test]
fn dispatch_guard_rejects_pending_restore_before_acquisition() {
    let code = step_source();
    let body = fn_body(&code, "fn dispatch_rows");
    let swapped = swap_markers(
        body,
        "world_acquisition::run",
        "source_player_restore::advance",
    );
    assert_eq!(marker_count(&swapped, "world_acquisition::run"), 1);
    assert_eq!(marker_count(&swapped, "source_player_restore::advance"), 1);
    let result = std::panic::catch_unwind(|| chain_positions(&swapped, DISPATCH_CHAIN));
    assert!(
        result.is_err(),
        "both markers survive but the acquire-before-restore guard must refuse"
    );
}

#[test]
fn recovery_precedes_oxygen_and_swapped_order_fails() {
    let code = step_source();
    let body = fn_body(&code, "fn dispatch_rows");
    let probe = [
        "projectiles::run",
        "source_player_restore::recover",
        "RulePhase::PlayerPrePhysicsOxygen",
        "player_motion::run",
    ];
    chain_positions(body, &probe);
    let swapped = swap_markers(
        body,
        "source_player_restore::recover",
        "RulePhase::PlayerPrePhysicsOxygen",
    );
    assert!(std::panic::catch_unwind(|| chain_positions(&swapped, &probe)).is_err());
}

#[test]
fn death_consumer_rejects_early_or_repeated_dispatch() {
    let code = step_source();
    let body = fn_body(&code, "fn dispatch_rows");
    let marker = "source_player_restore::settle_deaths";
    assert_eq!(marker_count(body, marker), 1);
    let chain = [
        "projectiles::advance",
        "hostile_outcomes::run",
        marker,
        "passives::run",
    ];
    chain_positions(body, &chain);
    for swap in ["projectiles::advance", "passives::run"] {
        let swapped = if swap == "projectiles::advance" {
            swap_markers(body, swap, marker)
        } else {
            swap_markers(body, marker, swap)
        };
        assert!(std::panic::catch_unwind(|| chain_positions(&swapped, &chain)).is_err());
    }
    let repeated = format!("{body}\n{marker}");
    assert!(std::panic::catch_unwind(|| assert_eq!(marker_count(&repeated, marker), 1)).is_err());
}

#[test]
fn safe_checkpoint_rejects_early_or_repeated_dispatch() {
    let code = step_source();
    let body = fn_body(&code, "fn dispatch_rows");
    let marker = "source_player_restore::checkpoint_safe";
    assert_eq!(marker_count(body, marker), 1);
    let chain = [
        "player_motion::run",
        "RulePhase::PlayerPostPhysics",
        marker,
        "RulePhase::CompanionMotion",
        "source_player_restore::settle_deaths",
    ];
    chain_positions(body, &chain);
    for other in ["player_motion::run", "RulePhase::CompanionMotion"] {
        let swapped = if other == "player_motion::run" {
            swap_markers(body, other, marker)
        } else {
            swap_markers(body, marker, other)
        };
        assert!(std::panic::catch_unwind(|| chain_positions(&swapped, &chain)).is_err());
    }
    let repeated = format!("{body}\n{marker}");
    assert!(std::panic::catch_unwind(|| assert_eq!(marker_count(&repeated, marker), 1)).is_err());
}

#[test]
fn trample_capture_order() {
    let code = step_source();
    let body = fn_body(&code, "fn dispatch_rows");
    let capture = "source_player_restore::capture_trample";
    let settle = "source_player_restore::settle_tramples";
    let chain = [
        "player_motion::run",
        "RulePhase::PlayerPostPhysics",
        capture,
        "source_player_restore::checkpoint_safe",
        "RulePhase::CompanionMotion",
        "source_player_restore::settle_deaths",
        "RulePhase::Trample",
        settle,
        "crops::settle_tramples",
        "RulePhase::SnowFootprint",
        "random_blocks::advance",
    ];
    chain_positions(body, &chain);
    for marker in [capture, settle] {
        assert_eq!(marker_count(body, marker), 1);
    }
    for (first, last) in [
        ("player_motion::run", capture),
        (capture, "RulePhase::CompanionMotion"),
        (settle, "random_blocks::advance"),
    ] {
        let swapped = swap_markers(body, first, last);
        assert!(std::panic::catch_unwind(|| chain_positions(&swapped, &chain)).is_err());
    }
    for marker in [capture, settle] {
        let repeat = format!("{body}\n{marker}");
        assert!(std::panic::catch_unwind(|| assert_eq!(marker_count(&repeat, marker), 1)).is_err());
    }
}

#[test]
fn snow_capture_order() {
    let code = step_source();
    let body = fn_body(&code, "fn dispatch_rows");
    let capture = "source_player_restore::capture_snow";
    let settle = "source_player_restore::settle_snow";
    // The passive capture rides the PassiveStepDeaths dispatch after the
    // source-player deaths, and its copied cells settle between the
    // source-player and legacy generic Snow regions (Go players-then-passives).
    let passive_capture = "passives::run_with_snow";
    let passive_settle = "passives::settle_snow";
    let chain = [
        "RulePhase::PlayerPostPhysics",
        "source_player_restore::capture_trample",
        capture,
        "source_player_restore::checkpoint_safe",
        "source_player_restore::settle_deaths",
        passive_capture,
        "RulePhase::Trample",
        "source_player_restore::settle_tramples",
        "crops::settle_tramples",
        "RulePhase::SnowFootprint",
        settle,
        passive_settle,
        "crops::settle_snow_footprints",
        "RulePhase::RandomBlock",
    ];
    chain_positions(body, &chain);
    for marker in [capture, settle, passive_capture, passive_settle] {
        assert_eq!(marker_count(body, marker), 1);
    }
    for (first, last) in [
        ("source_player_restore::capture_trample", capture),
        (capture, "source_player_restore::checkpoint_safe"),
        (passive_capture, "RulePhase::Trample"),
        ("RulePhase::SnowFootprint", settle),
        (settle, passive_settle),
        (passive_settle, "crops::settle_snow_footprints"),
        (settle, "crops::settle_snow_footprints"),
    ] {
        let swapped = swap_markers(body, first, last);
        assert!(std::panic::catch_unwind(|| chain_positions(&swapped, &chain)).is_err());
    }
    for marker in [capture, settle, passive_capture, passive_settle] {
        let repeat = format!("{body}\n{marker}");
        assert!(std::panic::catch_unwind(|| assert_eq!(marker_count(&repeat, marker), 1)).is_err());
    }
}

#[test]
fn action_costs_source_regions_order() {
    let code = step_source();
    let body = fn_body(&code, "fn dispatch_rows");
    let marker = "source_player_restore::settle_action_costs";
    assert_eq!(marker_count(body, marker), 2);
    let chain = [
        "source_player_restore::settle_deaths",
        "for envelope in context.deferred(RulePhase::Interaction)",
        "route_interaction",
        marker,
        "route_door",
        "sleep::enter",
        "sleep::settle",
        "mining::run",
        marker,
        "RulePhase::WorkbenchLifecycle",
    ];
    chain_positions(body, &chain);
    let swapped = body
        .replacen("route_interaction", "costs_swap", 1)
        .replacen(marker, "route_interaction", 1)
        .replacen("costs_swap", marker, 1);
    assert!(std::panic::catch_unwind(|| chain_positions(&swapped, &chain)).is_err());
    let duplicate = body.replacen(marker, &format!("{marker} {marker}"), 1);
    assert_ne!(marker_count(&duplicate, marker), 2);
}

#[test]
fn dispatch_guard_rejects_player_restore_before_companion() {
    // Companion pending advancement must precede pending players: the swap
    // keeps both markers present, so only the order probe can refuse.
    let code = step_source();
    let body = fn_body(&code, "fn dispatch_rows");
    let swapped = swap_markers(
        body,
        "source_companion_restore::advance",
        "source_player_restore::advance",
    );
    assert_eq!(
        marker_count(&swapped, "source_companion_restore::advance"),
        1
    );
    assert_eq!(marker_count(&swapped, "source_player_restore::advance"), 1);
    let result = std::panic::catch_unwind(|| chain_positions(&swapped, DISPATCH_CHAIN));
    assert!(
        result.is_err(),
        "both markers survive but the companion-before-player-restore guard must refuse"
    );
}

#[test]
fn source_acquisition_completion_phase_order() {
    // The independently bounded acquisition phase closes the dispatch goals
    // before the world acquisition runs and reopens them before companion
    // restoration, so no provider call site can straddle the boundary.
    let code = step_source();
    let body = fn_body(&code, "fn dispatch_rows");
    let chain = [
        "goals.before_acquire(",
        "world_acquisition::run(",
        "goals.after_acquire(",
        "source_companion_restore::advance(",
    ];
    chain_positions(body, &chain);
    // Negative control: swapping two of this guard's own markers must break
    // the order probe, proving the chain actually bites.
    let swapped = swap_markers(body, "goals.before_acquire(", "world_acquisition::run(");
    assert!(
        std::panic::catch_unwind(|| chain_positions(&swapped, &chain)).is_err(),
        "a swapped acquisition phase must break the frozen chain"
    );
}

#[test]
fn source_acquisition_reconcile_phase_order() {
    // Companion motion goals reconcile before hostile planning, so the
    // bounded phase always settles its own goals ahead of hostile intent.
    let code = step_source();
    let body = fn_body(&code, "fn dispatch_rows");
    let chain = [
        "RulePhase::CompanionMotion",
        "goals.reconcile(",
        "hostile_actions::plan(",
    ];
    chain_positions(body, &chain);
    let swapped = swap_markers(body, "goals.reconcile(", "hostile_actions::plan(");
    assert!(
        std::panic::catch_unwind(|| chain_positions(&swapped, &chain)).is_err(),
        "a swapped reconcile phase must break the frozen chain"
    );
}
