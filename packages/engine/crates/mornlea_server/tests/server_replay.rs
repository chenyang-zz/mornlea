//! Replay helpers for isolated phase providers.
//!
//! The canonical hash writes explicit logical fields. It does not mint the
//! expected hashes a provider must match; those stay with the source fixture.

#[path = "server_replay/companions.rs"]
mod companions;
#[path = "server_replay/containers.rs"]
mod containers;
#[path = "server_replay/crafting.rs"]
mod crafting;
#[path = "server_replay/crops.rs"]
mod crops;
#[path = "server_replay/drops.rs"]
mod drops;
#[path = "server_replay/eating.rs"]
mod eating;
#[path = "server_replay/environment.rs"]
mod environment;
#[path = "server_replay/farmland.rs"]
mod farmland;
#[path = "server_replay/fluids.rs"]
mod fluids;
#[path = "server_replay/full_corpus.rs"]
mod full_corpus;
#[path = "server_replay/furnaces.rs"]
mod furnaces;
#[path = "server_replay/hostile_actors.rs"]
mod hostile_actors;
#[path = "server_replay/hostile_outcomes.rs"]
mod hostile_outcomes;
#[path = "server_replay/inventory.rs"]
mod inventory;
#[path = "server_replay/mining.rs"]
mod mining;
#[path = "server_replay/passives.rs"]
mod passives;
#[path = "server_replay/phase_order.rs"]
mod phase_order;
#[path = "server_replay/player_motion.rs"]
mod player_motion;
#[path = "server_replay/player_survival.rs"]
mod player_survival;
#[path = "server_replay/projectiles.rs"]
mod projectiles;
#[path = "server_replay/random_blocks.rs"]
mod random_blocks;
#[path = "server_replay/sleep.rs"]
mod sleep;
#[path = "server_replay/supports.rs"]
mod supports;
#[path = "server_replay/tools.rs"]
mod tools;
#[path = "server_replay/world_acquisition.rs"]
mod world_acquisition;
#[path = "server_replay/world_mutation.rs"]
mod world_mutation;

use std::time::Instant;

use mornlea_domain::WorldState;
use mornlea_server::contracts::{
    ActorKey, CompanionActionEnvelope, Expected, Fixture, FixtureInput, Observed, PhaseReport,
    RuleCall, RuleReject, ScheduledInput, ServerEndpoint, ServerError, ServerLimits, TickBudget,
    TickCounters,
};
use mornlea_server::state::{AuthorityState, TickContext};
use sha2::{Digest, Sha256};

#[allow(dead_code)]
fn run_phase(
    fixture: &Fixture,
    provider: fn(&mut TickContext<'_>, RuleCall<'_>) -> Result<PhaseReport, ServerError>,
    call: RuleCall<'_>,
    budget: TickBudget,
) -> Observed {
    let mut authority = AuthorityState::try_new(limits(), fixture.seed as i64).expect("authority");
    let mut context = TickContext::from_fixture(&mut authority, &fixture.initial, budget);
    let failed = provider(&mut context, call).is_err();
    let state = context.snapshot_state(fixture.initial.world);
    Observed {
        events: context.events().to_vec(),
        counters: TickCounters {
            executed_tick: context.read().tick(),
            ..TickCounters::default()
        },
        state_sha256: canonical_state_sha256(&state),
        class: if failed {
            Some(RuleReject::StaleObservation)
        } else {
            None
        },
    }
}

#[allow(dead_code)]
fn replay(endpoint: &mut dyn ServerEndpoint, fixture: &Fixture) -> Observed {
    for input in &fixture.schedule {
        match input {
            FixtureInput::Human(ScheduledInput {
                earliest_tick,
                session,
                intent,
            }) => {
                let _ = earliest_tick;
                let _ = endpoint.submit(*session, intent.clone());
            }
            FixtureInput::Companion { value, .. } => {
                let _ = endpoint.submit_companion(value.clone());
            }
            FixtureInput::Internal { .. } => {}
        }
    }
    let budget = fixture
        .budgets
        .first()
        .map(|(_, budget)| *budget)
        .unwrap_or_else(TickBudget::full);
    let _ = endpoint.advance_tick(budget);
    Observed {
        events: Vec::new(),
        counters: TickCounters::default(),
        state_sha256: canonical_state_sha256_world(fixture.initial.world),
        class: None,
    }
}

#[allow(dead_code)]
fn assert_expected(actual: &Observed, expected: &Expected) {
    assert_eq!(actual.events, expected.events);
    assert_eq!(actual.state_sha256, expected.state_sha256);
    assert_eq!(actual.class, expected.class);
    assert_eq!(actual.counters, expected.counters);
}

#[allow(dead_code)]
fn assert_no_effect(before: &Observed, after: &Observed, class: RuleReject) {
    assert_eq!(before.state_sha256, after.state_sha256);
    assert!(after.events.is_empty());
    assert_eq!(after.class, Some(class));
}

fn canonical_state_sha256(state: &mornlea_server::contracts::FixtureState) -> [u8; 32] {
    let mut hasher = Sha256::new();
    write_world(&mut hasher, state.world);
    hasher.update((state.actors.len() as u64).to_be_bytes());
    let mut inventories = state.inventories.clone();
    inventories.sort_by(|left, right| actor_tag(left.0).cmp(&actor_tag(right.0)));
    for (key, inventory) in inventories {
        hasher.update(actor_tag(key));
        hasher.update([inventory.selected.get()]);
    }
    hasher.finalize().into()
}

fn canonical_state_sha256_world(world: WorldState) -> [u8; 32] {
    let mut hasher = Sha256::new();
    write_world(&mut hasher, world);
    hasher.finalize().into()
}

fn write_world(hasher: &mut Sha256, world: WorldState) {
    hasher.update(world.world_time_ticks().to_be_bytes());
    hasher.update(world.day_phase_offset().to_be_bytes());
    hasher.update([world.weather().wire_id(), world.season().wire_id()]);
}

fn actor_tag(key: ActorKey) -> [u8; 17] {
    let mut tag = [0u8; 17];
    match key {
        ActorKey::Player(session) => {
            tag[0] = 1;
            tag[1..9].copy_from_slice(&session.get().to_be_bytes());
        }
        ActorKey::Companion(id) => {
            tag[0] = 2;
            tag[1..].copy_from_slice(&id.bytes());
        }
        ActorKey::Hostile(id) => {
            tag[0] = 3;
            tag[1..9].copy_from_slice(&id.get().to_be_bytes());
        }
        ActorKey::Passive(id) => {
            tag[0] = 4;
            tag[1..9].copy_from_slice(&id.get().to_be_bytes());
        }
    }
    tag
}

fn limits() -> ServerLimits {
    ServerLimits::try_new(1, 1, 1, 1, 1, 1).expect("limits")
}

#[allow(dead_code)]
fn clock_anchor() -> Instant {
    Instant::now()
}

#[allow(dead_code)]
fn companion_value_is_send(value: CompanionActionEnvelope) -> CompanionActionEnvelope {
    value
}
