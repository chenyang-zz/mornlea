//! Source owner-record publication through the actual reducer.

use super::*;
use mornlea_server::contracts::EatingProgress;

fn bench_scene() -> (AuthorityState, SessionKey, SessionKey) {
    let mut state = authority();
    seed_world(&mut state);
    stage(&mut state, |context| {
        let mut front = ground_chunk();
        set_cell(&mut front, BlockPos::new(0, 66, -1), 45);
        context.preload_ready_chunk(ReadyChunk::try_new(chunk_key(0, -1), 1, 1, front).unwrap());
        let mut home = ground_chunk();
        set_cell(&mut home, BlockPos::new(0, 66, 1), 45);
        context.preload_ready_chunk(ReadyChunk::try_new(chunk_key(0, 0), 1, 1, home).unwrap());
    });
    let owner = login(&mut state, 1, "Ada", [0.5, 65.0, 0.5], 0.0, 0.0);
    let other = login(&mut state, 2, "Ben", [4.5, 65.0, 4.5], 0.0, 0.0);
    state.advance_tick(TickBudget::full()).unwrap();
    (state, owner, other)
}

fn crafting_states(events: &[Event]) -> Vec<Event> {
    events
        .iter()
        .filter(|e| matches!(e, Event::CraftingState(_)))
        .cloned()
        .collect()
}

#[test]
fn first_workbench_open_publishes_only_the_grid() {
    let (mut state, owner, other) = bench_scene();
    submit(&mut state, owner, 1, Command::OpenContainer(look(0.0, 0.0)));
    let opened = state.advance_tick(TickBudget::full()).unwrap();
    assert_eq!(
        state.residents().inventories[&ActorKey::Player(owner)].crafting_size,
        CraftingSize::Workbench
    );
    assert_eq!(crafting_states(&events_for(&opened, owner)).len(), 1);
    assert!(projection_inventory_states(&events_for(&opened, owner)).is_empty());
    assert!(projection_crafting_record_states(&events_for(&opened, other)).is_empty());
    let quiet = state.advance_tick(TickBudget::full()).unwrap();
    assert!(projection_crafting_record_states(&events_for(&quiet, owner)).is_empty());
    submit(
        &mut state,
        owner,
        2,
        Command::OpenContainer(look(std::f32::consts::FRAC_PI_2, 0.0)),
    );
    let refused = state.advance_tick(TickBudget::full()).unwrap();
    assert!(projection_crafting_record_states(&events_for(&refused, owner)).is_empty());
}

#[test]
fn same_anchor_workbench_reopen_publishes_one_equal_grid() {
    let (mut state, owner, other) = bench_scene();
    submit(&mut state, owner, 1, Command::OpenContainer(look(0.0, 0.0)));
    let opened = state.advance_tick(TickBudget::full()).unwrap();
    let expected = crafting_states(&events_for(&opened, owner));
    assert_eq!(expected.len(), 1);
    let before = state.residents().inventories[&ActorKey::Player(owner)];
    submit(&mut state, owner, 2, Command::OpenContainer(look(0.0, 0.0)));
    let reopened = state.advance_tick(TickBudget::full()).unwrap();
    assert_eq!(
        state.residents().inventories[&ActorKey::Player(owner)],
        before
    );
    assert_eq!(crafting_states(&events_for(&reopened, owner)), expected);
    assert!(projection_inventory_states(&events_for(&reopened, owner)).is_empty());
    assert!(projection_crafting_record_states(&events_for(&reopened, other)).is_empty());
}

#[test]
fn reanchored_workbench_publishes_one_equal_grid() {
    let (mut state, owner, other) = bench_scene();
    submit(&mut state, owner, 1, Command::OpenContainer(look(0.0, 0.0)));
    let opened = state.advance_tick(TickBudget::full()).unwrap();
    let expected = crafting_states(&events_for(&opened, owner));
    submit(
        &mut state,
        owner,
        2,
        Command::OpenContainer(look(std::f32::consts::PI, 0.0)),
    );
    let reanchored = state.advance_tick(TickBudget::full()).unwrap();
    let residents = state.residents();
    assert!(
        matches!(residents.runtimes[&ActorKey::Player(owner)].aux, ActorAux::Player { workbench: Some(cell), .. } if cell == BlockPos::new(0,66,1))
    );
    assert_eq!(crafting_states(&events_for(&reanchored, owner)), expected);
    assert!(projection_inventory_states(&events_for(&reanchored, owner)).is_empty());
    assert!(projection_crafting_record_states(&events_for(&reanchored, other)).is_empty());
}

#[test]
fn pending_owner_retains_changed_records_until_active() {
    let mut state = authority();
    seed_world(&mut state);
    let owner = login(&mut state, 1, "Ada", [0.5, 65.0, 0.5], 0.0, 0.0);
    state.advance_tick(TickBudget::full()).unwrap();
    let residents = state.residents();
    let mut actor = residents
        .actors
        .iter()
        .find(|a| a.key == ActorKey::Player(owner))
        .unwrap()
        .clone();
    actor.lifecycle = ActorLifecycle::Pending;
    let mut inventory = residents.inventories[&actor.key];
    inventory.slots[0] = storage(ITEM_DIRT, 3);
    inventory.crafting[0] = storage(ITEM_STICK, 1);
    stage(&mut state, |context| {
        context.stage(RuleEffect::Actor(actor.clone())).unwrap();
        context.preload_inventory(actor.key, inventory);
    });
    let pending = state.advance_tick(TickBudget::full()).unwrap();
    assert_eq!(
        state
            .residents()
            .actors
            .iter()
            .find(|a| a.key == actor.key)
            .unwrap()
            .lifecycle,
        ActorLifecycle::Pending
    );
    assert!(
        events_for(&pending, owner)
            .iter()
            .any(|e| matches!(e, Event::PlayerState(_)))
    );
    assert!(projection_crafting_record_states(&events_for(&pending, owner)).is_empty());
    actor.lifecycle = ActorLifecycle::Active;
    stage(&mut state, |context| {
        context.stage(RuleEffect::Actor(actor)).unwrap();
    });
    let active = state.advance_tick(TickBudget::full()).unwrap();
    assert_eq!(
        projection_inventory_states(&events_for(&active, owner)).len(),
        1
    );
    assert_eq!(crafting_states(&events_for(&active, owner)).len(), 1);
}

#[test]
fn eating_and_identical_pickup_publish_the_equal_final_inventory_once() {
    let mut state = authority();
    seed_world(&mut state);
    let owner = login_with(&mut state, 1, "Ada", [0.5, 65.0, 0.5], 0.0, 0.0, |p| {
        p.inventory.hotbar.slots[0] = storage(36, 2);
        p.hunger = 10;
    });
    let other = login(&mut state, 2, "Ben", [4.5, 65.0, 4.5], 0.0, 0.0);
    let joined = state.advance_tick(TickBudget::full()).unwrap();
    let expected = projection_inventory_states(&events_for(&joined, owner));
    let residents = state.residents();
    let before = residents.inventories[&ActorKey::Player(owner)];
    let mut runtime = residents.runtimes[&ActorKey::Player(owner)].clone();
    runtime.reset = false;
    runtime.has_view = true;
    runtime.controls = Some(PlayerControl::new(PlayerControlParts {
        movement: Movement {
            move_x: 0,
            move_z: 0,
            jump: false,
        },
        look: look(0.0, 0.0),
        actions: HeldActions {
            primary: false,
            eating: true,
            sprinting: false,
            sneaking: false,
        },
    }));
    runtime.eating = Some(EatingProgress {
        slot: HotbarSlot::new(0).unwrap(),
        item: 36,
        ticks: RuleTunables::source_defaults().eating_ticks() - 1,
    });
    stage(&mut state, |context| {
        context.stage(RuleEffect::Runtime(runtime)).unwrap();
        let mut drop = drop_record(3, 0, [0.5, 65.5, 0.5]);
        drop.stack = storage(36, 1);
        drop.pickup_delay = 1;
        context.preload_drop(drop);
    });
    let settled = state.advance_tick(TickBudget::full()).unwrap();
    let residents = state.residents();
    assert_eq!(residents.inventories[&ActorKey::Player(owner)], before);
    assert!(
        residents
            .actors
            .iter()
            .find(|a| a.key == ActorKey::Player(owner))
            .unwrap()
            .survival
            .hunger()
            > 10
    );
    assert!(residents.drop_records().is_empty());
    assert_eq!(
        projection_inventory_states(&events_for(&settled, owner)),
        expected
    );
    assert!(projection_inventory_states(&events_for(&settled, other)).is_empty());
}

#[test]
fn initial_pending_owner_has_no_record_mirrors_before_activation() {
    let mut state = authority();
    seed_world(&mut state);
    let owner = login(&mut state, 1, "Ada", [0.5, 65.0, 0.5], 0.0, 0.0);
    let mut seed = authority();
    seed_world(&mut seed);
    let seed_owner = login(&mut seed, 1, "Ada", [0.5, 65.0, 0.5], 0.0, 0.0);
    seed.advance_tick(TickBudget::full()).unwrap();
    let body = seed
        .residents()
        .actors
        .iter()
        .find(|actor| actor.key == ActorKey::Player(seed_owner))
        .unwrap()
        .body
        .clone();
    let mut actor = ActorRecord::try_new(
        ActorKey::Player(owner),
        ActorLifecycle::Pending,
        Dimension::OVERWORLD,
        MotionState::new(MotionStateParts {
            position: FiniteVec3::try_new([0.5, 65.0, 0.5]).unwrap(),
            velocity: FiniteVec3::try_new([0.0; 3]).unwrap(),
            on_ground: true,
        }),
        look(0.0, 0.0),
        survival(),
        body,
    )
    .unwrap();
    stage(&mut state, |context| {
        context.stage(RuleEffect::Actor(actor.clone())).unwrap();
        context.preload_inventory(
            actor.key,
            mornlea_server::contracts::InventoryRecord::empty(),
        );
    });
    let pending = state.advance_tick(TickBudget::full()).unwrap();
    assert_eq!(
        state
            .residents()
            .actors
            .iter()
            .find(|a| a.key == actor.key)
            .unwrap()
            .lifecycle,
        ActorLifecycle::Pending
    );
    assert!(projection_crafting_record_states(&events_for(&pending, owner)).is_empty());
    assert!(
        events_for(&pending, owner)
            .iter()
            .any(|e| matches!(e, Event::PlayerState(_)))
    );
    actor.lifecycle = ActorLifecycle::Active;
    stage(&mut state, |context| {
        context.stage(RuleEffect::Actor(actor)).unwrap();
    });
    let active = state.advance_tick(TickBudget::full()).unwrap();
    assert_eq!(
        projection_inventory_states(&events_for(&active, owner)).len(),
        1
    );
    assert_eq!(crafting_states(&events_for(&active, owner)).len(), 1);
}
