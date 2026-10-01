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
//! depends on. Their full-fixture behavioral layer (real kills settling
//! between projectile and passive phases, real hunger settling before motion)
//! needs actor and world seeding the frozen reducer surface does not supply;
//! that layer is recorded for the integration owner, not invented here.
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

/// The intake admits in envelope order: motion, inventory, containers,
/// crafting. An open lands in both the container and the workbench bags.
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
    "source_player_restore::advance",
    "player_survival::run",
    "eating::run",
    "projectiles::run",
    "source_player_restore::recover",
    "player_survival::run",
    "player_motion::run",
    "player_survival::run",
    "companions::run",
    "hostile_actions::plan",
    "hostile_actions::apply",
    "hostile_actors::run",
    "hostile_outcomes::advance",
    "hostile_actors::run",
    "projectiles::advance",
    "hostile_outcomes::run",
    "passives::run",
    "companions::run",
    "route_interaction",
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
    "crops::settle_tramples",
    "crops::run",
    "crops::settle_snow_footprints",
    "random_blocks::run",
    "random_blocks::advance",
    "containers::run",
    "mining::run",
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
        &["random_blocks::advance", "containers::run", "mining::run"],
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
