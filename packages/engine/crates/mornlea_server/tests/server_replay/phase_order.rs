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

use mornlea_domain::{Command, PlayerId};
use mornlea_protocol::{LoginStart, PlayIntent, admit_login};
use mornlea_server::contracts::{ServerLimits, TickBudget, TransportKind};
use mornlea_server::state::AuthorityState;

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
    "player_motion::run",
    "inventory::run",
    "containers::run",
    "crafting::run",
];

/// The interaction gates in loop order: placement and door geometry, tools
/// and buckets, then panel drops.
const GATE_CHAIN: &[&str] = &["world_mutation::run", "tools::run", "drops::run"];

/// The full frozen dispatch chain in source order: intake loop, intent,
/// acquire, the per-actor survival line, companion motion, hostile planning
/// through player deaths, passive deaths, companion placement, the single
/// ordered interaction loop with its internal arm, sleep settlement, drops,
/// furnaces, the world sweep row, container moves, mining, workbench
/// lifecycle, support and the single climate close.
const DISPATCH_CHAIN: &[&str] = &[
    "admit_command",
    "companions::run",
    "world_acquisition::run",
    "player_survival::run",
    "eating::run",
    "projectiles::run",
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
    "route_internal",
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
    chain_positions(fn_body(&code, "fn reduce_tick"), WRAPPER_CHAIN);
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
        &["companions::run", "route_interaction", "sleep::settle"],
    );
    chain_positions(
        fn_body(&code, "fn route_interaction"),
        &["world_mutation::run", "tools::run", "drops::run"],
    );
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
