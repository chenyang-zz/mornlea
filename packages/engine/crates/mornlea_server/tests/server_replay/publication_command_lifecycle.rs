//! Native lifecycle command ordering and real held eating advancement.
use super::*;

fn scene(kind: ContainerKind) -> (AuthorityState, SessionKey, SessionKey, ContainerRef) {
    let mut state = authority();
    state.enable_source_player_restoration(1).unwrap();
    seed_world(&mut state);
    let mut home = ground_chunk();
    set_cell(&mut home, BlockPos::new(0, 66, 1), 45);
    let mut front = ground_chunk();
    let reference = match kind {
        ContainerKind::Chest => chest_in_chunk(
            &mut front,
            BlockPos::new(0, 66, -1),
            storage_array(&[(0, ITEM_DIRT, 5)]),
        ),
        ContainerKind::Furnace => furnace_in_chunk(
            &mut front,
            BlockPos::new(0, 66, -1),
            StorageStack::default(),
            StorageStack::default(),
            StorageStack::default(),
            0,
            0,
        ),
    };
    stage(&mut state, |context| {
        for x in -1..=1 {
            for z in -1..=1 {
                context.preload_ready_chunk(
                    ReadyChunk::try_new(chunk_key(x, z), 1, 1, ground_chunk()).unwrap(),
                );
            }
        }
        context.preload_ready_chunk(ReadyChunk::try_new(chunk_key(0, 0), 1, 1, home).unwrap());
        context.preload_ready_chunk(ReadyChunk::try_new(chunk_key(0, -1), 1, 1, front).unwrap());
    });
    let owner = login_with(&mut state, 1, "Ada", [0.5, 65.0, 0.5], 0.0, 0.0, |p| {
        p.inventory.hotbar.slots[0] = storage(36, 2);
        p.hunger = 10;
    });
    let other = login(&mut state, 2, "Ben", [4.5, 65.0, 4.5], 0.0, 0.0);
    state.advance_tick(TickBudget::full()).unwrap();
    assert!(state.residents().runtimes[&ActorKey::Player(owner)].has_view);
    (state, owner, other, reference)
}

fn open_bench(state: &mut AuthorityState, owner: SessionKey, sequence: u64) {
    submit(
        state,
        owner,
        sequence,
        Command::OpenContainer(look(std::f32::consts::PI, 0.0)),
    );
}

fn held_eating() -> Command {
    Command::PlayerInput(PlayerControl::new(PlayerControlParts {
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
    }))
}

fn eating_ticks(state: &AuthorityState, owner: SessionKey) -> Option<u16> {
    state.residents().runtimes[&ActorKey::Player(owner)]
        .eating
        .map(|p| p.ticks)
}

fn record(state: &AuthorityState, owner: SessionKey) -> mornlea_server::contracts::InventoryRecord {
    state.residents().inventories[&ActorKey::Player(owner)]
}

#[test]
fn native_bench_open_then_close_ends_personal_with_final_open_intent() {
    let (mut state, owner, other, _) = scene(ContainerKind::Chest);
    open_bench(&mut state, owner, 1);
    submit(&mut state, owner, 2, Command::CloseContainer);
    let tick = state.advance_tick(TickBudget::full()).unwrap();
    assert_eq!(record(&state, owner).crafting_size, CraftingSize::Personal);
    assert!(state.settled_read().unwrap().viewer(owner).is_none());
    let events = events_for(&tick, owner);
    let grids: Vec<_> = events
        .iter()
        .filter_map(|e| {
            if let Event::CraftingState(v) = e {
                Some(v)
            } else {
                None
            }
        })
        .collect();
    assert_eq!(grids.len(), 1);
    assert_eq!(grids[0].size(), CraftingSize::Personal);
    assert!(
        !events
            .iter()
            .any(|e| matches!(e, Event::ContainerClosed(_)))
    );
    assert!(projection_crafting_record_states(&events_for(&tick, other)).is_empty());
}

#[test]
fn native_bench_open_then_extended_move_observes_widened_grid() {
    let (mut state, owner, other, _) = scene(ContainerKind::Chest);
    stage(&mut state, |context| {
        let actor = ActorKey::Player(owner);
        let mut inventory = *context.read().inventory(actor).unwrap();
        inventory.slots[0] = storage(ITEM_DIRT, 2);
        context.preload_inventory(actor, inventory);
    });
    open_bench(&mut state, owner, 1);
    submit(
        &mut state,
        owner,
        2,
        Command::MoveCrafting(mornlea_domain::CraftingMove::try_new(9, 4).unwrap()),
    );
    let tick = state.advance_tick(TickBudget::full()).unwrap();
    let inventory = record(&state, owner);
    assert_eq!(inventory.crafting_size, CraftingSize::Workbench);
    assert_eq!(inventory.crafting[4], storage(ITEM_DIRT, 2));
    assert_eq!(inventory.slots[0], StorageStack::default());
    assert_eq!(
        projection_crafting_record_states(&events_for(&tick, owner)).len(),
        2
    );
    assert!(projection_crafting_record_states(&events_for(&tick, other)).is_empty());
}

#[test]
fn native_bench_open_then_chest_open_keeps_later_chest_view() {
    let (mut state, owner, _, reference) = scene(ContainerKind::Chest);
    open_bench(&mut state, owner, 1);
    submit(&mut state, owner, 2, Command::OpenContainer(look(0.0, 0.0)));
    let tick = state.advance_tick(TickBudget::full()).unwrap();
    assert_eq!(
        state
            .settled_read()
            .unwrap()
            .viewer(owner)
            .unwrap()
            .reference(),
        reference
    );
    assert_eq!(record(&state, owner).crafting_size, CraftingSize::Workbench);
    let expected = mornlea_domain::ChestState::try_new(mornlea_domain::ChestStateParts {
        container: reference,
        items: item_array(&[(0, ITEM_DIRT, 5)]),
    })
    .unwrap();
    assert!(events_for(&tick, owner).contains(&Event::ChestState(expected)));
    assert!(
        !events_for(&tick, owner)
            .iter()
            .any(|e| matches!(e, Event::ContainerClosed(_)))
    );
}

fn open_interrupts_completion(kind: ContainerKind) {
    let (mut state, owner, other, reference) = scene(kind);
    submit(&mut state, owner, 1, held_eating());
    let ticks = RuleTunables::source_defaults().eating_ticks();
    for _ in 0..ticks - 1 {
        state.advance_tick(TickBudget::full()).unwrap();
    }
    assert_eq!(eating_ticks(&state, owner), Some(ticks - 1));
    let before = record(&state, owner);
    let survival = state
        .settled_read()
        .unwrap()
        .actor(ActorKey::Player(owner))
        .unwrap()
        .survival;
    submit(&mut state, owner, 2, Command::OpenContainer(look(0.0, 0.0)));
    let tick = state.advance_tick(TickBudget::full()).unwrap();
    assert_eq!(record(&state, owner), before);
    assert_eq!(
        state
            .settled_read()
            .unwrap()
            .actor(ActorKey::Player(owner))
            .unwrap()
            .survival,
        survival
    );
    assert_eq!(eating_ticks(&state, owner), None);
    assert_eq!(
        state
            .settled_read()
            .unwrap()
            .viewer(owner)
            .unwrap()
            .reference(),
        reference
    );
    assert!(projection_inventory_states(&events_for(&tick, owner)).is_empty());
    assert!(projection_inventory_states(&events_for(&tick, other)).is_empty());
}

#[test]
fn native_chest_open_interrupts_actual_eating_completion() {
    open_interrupts_completion(ContainerKind::Chest);
}

#[test]
fn native_furnace_open_interrupts_actual_eating_completion() {
    open_interrupts_completion(ContainerKind::Furnace);
}

fn blocked_eating_scene() -> (AuthorityState, SessionKey, SessionKey) {
    let (mut state, owner, other, _) = scene(ContainerKind::Chest);
    submit(&mut state, owner, 1, Command::OpenContainer(look(0.0, 0.0)));
    state.advance_tick(TickBudget::full()).unwrap();
    submit(&mut state, owner, 2, held_eating());
    state.advance_tick(TickBudget::full()).unwrap();
    assert_eq!(eating_ticks(&state, owner), None);
    (state, owner, other)
}

#[test]
fn native_close_starts_actual_held_eating_in_same_tick() {
    let (mut state, owner, other) = blocked_eating_scene();
    let before = record(&state, owner);
    submit(&mut state, owner, 3, Command::CloseContainer);
    let tick = state.advance_tick(TickBudget::full()).unwrap();
    assert_eq!(eating_ticks(&state, owner), Some(1));
    assert_eq!(record(&state, owner), before);
    assert!(state.settled_read().unwrap().viewer(owner).is_none());
    assert!(
        !events_for(&tick, owner)
            .iter()
            .any(|e| matches!(e, Event::ContainerClosed(_)))
    );
    assert!(projection_inventory_states(&events_for(&tick, other)).is_empty());
}

#[test]
fn native_workbench_replacement_starts_actual_held_eating_in_same_tick() {
    let (mut state, owner, other) = blocked_eating_scene();
    let food = record(&state, owner).slots[0];
    open_bench(&mut state, owner, 3);
    let tick = state.advance_tick(TickBudget::full()).unwrap();
    assert_eq!(eating_ticks(&state, owner), Some(1));
    assert_eq!(record(&state, owner).slots[0], food);
    assert_eq!(record(&state, owner).crafting_size, CraftingSize::Workbench);
    assert!(state.settled_read().unwrap().viewer(owner).is_none());
    assert!(projection_crafting_record_states(&events_for(&tick, other)).is_empty());
}

#[test]
fn native_chest_open_then_bench_open_keeps_later_bench_view() {
    let (mut state, owner, _, _) = scene(ContainerKind::Chest);
    submit(&mut state, owner, 1, Command::OpenContainer(look(0.0, 0.0)));
    open_bench(&mut state, owner, 2);
    let tick = state.advance_tick(TickBudget::full()).unwrap();
    assert!(state.settled_read().unwrap().viewer(owner).is_none());
    assert_eq!(record(&state, owner).crafting_size, CraftingSize::Workbench);
    assert_eq!(
        projection_crafting_record_states(&events_for(&tick, owner)).len(),
        1
    );
    assert!(
        !events_for(&tick, owner)
            .iter()
            .any(|e| matches!(e, Event::ChestState(_) | Event::ContainerClosed(_)))
    );
}

#[test]
fn source_registration_owns_view_marker_before_any_ready_chunk() {
    let mut state = authority();
    state.enable_source_player_restoration(1).unwrap();
    let owner = login_with(&mut state, 1, "Ada", [0.5, 65.0, 0.5], 0.0, 0.0, |p| {
        p.hunger = 10;
        p.inventory.hotbar.slots[0] = storage(36, 2);
    });
    let read = state.settled_read().unwrap();
    assert!(!read.ready_chunk(chunk_key(0, 0)));
    assert_eq!(
        read.actor(ActorKey::Player(owner)).unwrap().lifecycle,
        ActorLifecycle::Pending
    );
    assert!(read.runtime(ActorKey::Player(owner)).unwrap().has_view);
}
