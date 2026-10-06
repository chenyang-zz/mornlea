//! Native lifecycle command ordering and real held eating advancement.
use super::*;

pub(super) fn scene(kind: ContainerKind) -> (AuthorityState, SessionKey, SessionKey, ContainerRef) {
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

fn hotbar_wire(selected: u8, bread: u8) -> Event {
    Event::InventoryState(InventoryState::new(InventoryStateParts {
        selected: HotbarSlot::new(selected).unwrap(),
        hotbar: item_array(&[(0, 36, bread)]),
        backpack: item_array(&[]),
    }))
}

#[test]
fn native_hotbar_drop_before_selection_uses_old_slot() {
    let (mut state, owner, other, _) = scene(ContainerKind::Chest);
    let mut expected = record(&state, owner);
    expected.selected = HotbarSlot::new(1).unwrap();
    expected.slots[0] = storage(36, 1);
    submit(&mut state, owner, 1, Command::DropSelectedItem);
    submit(
        &mut state,
        owner,
        2,
        Command::SelectHotbar(HotbarSlot::new(1).unwrap()),
    );
    let tick = state.advance_tick(TickBudget::full()).unwrap();
    assert_eq!(record(&state, owner), expected);
    let drops = state.residents().drop_records();
    assert_eq!(drops.len(), 1);
    assert_eq!(drops[0].stack, storage(36, 1));
    assert_eq!(
        projection_drop_wire_events(&events_for(&tick, owner)),
        vec![projection_drop_wire_upsert(
            tick.tick,
            0,
            BlockPos::new(0, 65, 0),
            36,
            1
        )]
    );
    assert_eq!(
        projection_inventory_states(&events_for(&tick, owner)),
        vec![hotbar_wire(1, 1)]
    );
    assert!(projection_inventory_states(&events_for(&tick, other)).is_empty());
    let quiet = state.advance_tick(TickBudget::full()).unwrap();
    assert!(projection_inventory_states(&events_for(&quiet, owner)).is_empty());
}

#[test]
fn native_hotbar_selection_after_eating_completion() {
    let (mut state, owner, other, _) = scene(ContainerKind::Chest);
    submit(&mut state, owner, 1, held_eating());
    let required = RuleTunables::source_defaults().eating_ticks();
    for _ in 0..required - 1 {
        state.advance_tick(TickBudget::full()).unwrap();
    }
    assert_eq!(eating_ticks(&state, owner), Some(required - 1));
    assert_eq!(record(&state, owner).slots[0], storage(36, 2));
    assert_eq!(
        state
            .settled_read()
            .unwrap()
            .actor(ActorKey::Player(owner))
            .unwrap()
            .survival
            .hunger(),
        10
    );
    let mut expected = record(&state, owner);
    expected.selected = HotbarSlot::new(1).unwrap();
    expected.slots[0] = storage(36, 1);
    submit(
        &mut state,
        owner,
        2,
        Command::SelectHotbar(HotbarSlot::new(1).unwrap()),
    );
    let tick = state.advance_tick(TickBudget::full()).unwrap();
    assert_eq!(record(&state, owner), expected);
    assert_eq!(
        state
            .settled_read()
            .unwrap()
            .actor(ActorKey::Player(owner))
            .unwrap()
            .survival
            .hunger(),
        15
    );
    assert_eq!(eating_ticks(&state, owner), None);
    assert_eq!(
        projection_inventory_states(&events_for(&tick, owner)),
        vec![hotbar_wire(1, 1)]
    );
    assert!(projection_inventory_states(&events_for(&tick, other)).is_empty());
    state.advance_tick(TickBudget::full()).unwrap();
    assert_eq!(eating_ticks(&state, owner), None);
}

#[test]
fn native_hotbar_selection_does_not_change_immediate_equip_basis() {
    let (mut state, owner, other, _) = scene(ContainerKind::Chest);
    // The saved pack cause is prepared; both commands use actual admission.
    stage(&mut state, |context| {
        let actor = ActorKey::Player(owner);
        let mut inventory = *context.read().inventory(actor).unwrap();
        inventory.slots[1] = StorageStack {
            item: 58,
            count: 1,
            durability: 165,
        };
        context.preload_inventory(actor, inventory);
    });
    let mut expected = record(&state, owner);
    expected.selected = HotbarSlot::new(1).unwrap();
    submit(
        &mut state,
        owner,
        1,
        Command::SelectHotbar(HotbarSlot::new(1).unwrap()),
    );
    submit(&mut state, owner, 2, Command::EquipArmor);
    let tick = state.advance_tick(TickBudget::full()).unwrap();
    assert_eq!(record(&state, owner), expected);
    let mut hotbar = item_array(&[(0, 36, 2)]);
    hotbar[1] = ItemStack::try_new(58, 1, 165).unwrap();
    let wire = Event::InventoryState(InventoryState::new(InventoryStateParts {
        selected: HotbarSlot::new(1).unwrap(),
        hotbar,
        backpack: item_array(&[]),
    }));
    assert_eq!(
        projection_inventory_states(&events_for(&tick, owner)),
        vec![wire]
    );
    assert!(projection_inventory_states(&events_for(&tick, other)).is_empty());
}

#[test]
fn native_hotbar_selection_before_drop_keeps_prefix_order() {
    let (mut state, owner, other, _) = scene(ContainerKind::Chest);
    let mut expected = record(&state, owner);
    expected.selected = HotbarSlot::new(1).unwrap();
    submit(
        &mut state,
        owner,
        1,
        Command::SelectHotbar(HotbarSlot::new(1).unwrap()),
    );
    submit(&mut state, owner, 2, Command::DropSelectedItem);
    let tick = state.advance_tick(TickBudget::full()).unwrap();
    assert_eq!(record(&state, owner), expected);
    assert!(state.residents().drop_records().is_empty());
    assert!(projection_drop_wire_events(&events_for(&tick, owner)).is_empty());
    assert_eq!(
        projection_inventory_states(&events_for(&tick, owner)),
        vec![hotbar_wire(1, 2)]
    );
    assert!(projection_inventory_states(&events_for(&tick, other)).is_empty());
}

fn placement_move_control(yaw: f32, pitch: f32) -> PlayerControl {
    PlayerControl::new(PlayerControlParts {
        movement: Movement {
            move_x: 0,
            move_z: 1,
            jump: false,
        },
        look: look(yaw, pitch),
        actions: HeldActions {
            primary: false,
            eating: false,
            sprinting: false,
            sneaking: false,
        },
    })
}

fn place_with_empty_slot(yaw: f32, pitch: f32) -> Command {
    Command::PlaceBlock(mornlea_domain::PlacementIntent::try_new(look(yaw, pitch), 8).unwrap())
}

fn native_input_ack(tick: &mornlea_server::contracts::TickPublication, owner: SessionKey) -> u64 {
    events_for(tick, owner)
        .iter()
        .find_map(|event| match event {
            Event::PlayerState(value) => Some(value.last_input_sequence()),
            _ => None,
        })
        .expect("native player state")
}

#[test]
fn native_placement_admission_keeps_look_after_refused_world_action() {
    let (mut state, owner, other, _) = scene(ContainerKind::Chest);
    let before = record(&state, owner);
    submit(&mut state, owner, 1, place_with_empty_slot(0.75, 0.25));
    let tick = state.advance_tick(TickBudget::full()).unwrap();
    assert_eq!(
        state
            .settled_read()
            .unwrap()
            .actor(ActorKey::Player(owner))
            .unwrap()
            .look,
        look(0.75, 0.25)
    );
    assert_eq!(record(&state, owner), before);
    assert_eq!(
        state.residents().runtimes[&ActorKey::Player(owner)].controls,
        None
    );
    assert_eq!(native_input_ack(&tick, owner), 0);
    assert_eq!(state.session(owner).unwrap().last_applied_sequence, 1);
    assert!(!events_for(&tick, owner).iter().any(|event| matches!(
        event,
        Event::PlaceBlockSucceeded(_) | Event::BlockChanges(_)
    )));
    assert!(projection_inventory_states(&events_for(&tick, owner)).is_empty());
    assert!(projection_inventory_states(&events_for(&tick, other)).is_empty());
}

#[test]
fn native_placement_after_same_tick_input_uses_current_motion_yaw() {
    let (mut state, owner, other, _) = scene(ContainerKind::Chest);
    let (mut reference, reference_owner, _, _) = scene(ContainerKind::Chest);
    let before = record(&state, owner);
    let new_yaw = std::f32::consts::FRAC_PI_2;
    submit(
        &mut state,
        owner,
        1,
        Command::PlayerInput(placement_move_control(0.0, 0.0)),
    );
    submit(&mut state, owner, 2, place_with_empty_slot(new_yaw, 0.25));
    submit(
        &mut reference,
        reference_owner,
        1,
        Command::PlayerInput(placement_move_control(new_yaw, 0.25)),
    );
    submit(
        &mut reference,
        reference_owner,
        2,
        place_with_empty_slot(new_yaw, 0.25),
    );
    let tick = state.advance_tick(TickBudget::full()).unwrap();
    reference.advance_tick(TickBudget::full()).unwrap();
    let view = state.settled_read().unwrap();
    let expected = reference.settled_read().unwrap();
    let actor = ActorKey::Player(owner);
    let reference_actor = ActorKey::Player(reference_owner);
    assert_eq!(
        view.actor(actor).unwrap().motion,
        expected.actor(reference_actor).unwrap().motion
    );
    assert_eq!(view.actor(actor).unwrap().look, look(new_yaw, 0.25));
    assert_eq!(
        view.runtime(actor).unwrap().controls,
        Some(placement_move_control(new_yaw, 0.25))
    );
    assert_eq!(native_input_ack(&tick, owner), 1);
    assert_eq!(record(&state, owner), before);
    assert_eq!(record(&reference, reference_owner), before);
    assert!(
        !events_for(&tick, owner)
            .iter()
            .any(|event| matches!(event, Event::PlaceBlockSucceeded(_)))
    );
    assert!(projection_inventory_states(&events_for(&tick, other)).is_empty());
}

#[test]
fn native_placement_changes_previously_held_motion_yaw() {
    let (mut state, owner, _, _) = scene(ContainerKind::Chest);
    let (mut reference, reference_owner, _, _) = scene(ContainerKind::Chest);
    for (authority, session) in [(&mut state, owner), (&mut reference, reference_owner)] {
        submit(
            authority,
            session,
            1,
            Command::PlayerInput(placement_move_control(0.0, 0.0)),
        );
        authority.advance_tick(TickBudget::full()).unwrap();
    }
    let new_yaw = std::f32::consts::FRAC_PI_2;
    submit(&mut state, owner, 2, place_with_empty_slot(new_yaw, 0.25));
    submit(
        &mut reference,
        reference_owner,
        2,
        Command::PlayerInput(placement_move_control(new_yaw, 0.25)),
    );
    submit(
        &mut reference,
        reference_owner,
        3,
        place_with_empty_slot(new_yaw, 0.25),
    );
    let tick = state.advance_tick(TickBudget::full()).unwrap();
    let reference_tick = reference.advance_tick(TickBudget::full()).unwrap();
    let view = state.settled_read().unwrap();
    let expected = reference.settled_read().unwrap();
    let actor = ActorKey::Player(owner);
    let reference_actor = ActorKey::Player(reference_owner);
    assert_eq!(
        view.actor(actor).unwrap().motion,
        expected.actor(reference_actor).unwrap().motion
    );
    assert_eq!(view.actor(actor).unwrap().look, look(new_yaw, 0.25));
    assert_eq!(view.runtime(actor), expected.runtime(reference_actor));
    assert_eq!(native_input_ack(&tick, owner), 1);
    assert_eq!(native_input_ack(&reference_tick, reference_owner), 2);
    assert_eq!(record(&state, owner), record(&reference, reference_owner));
}

#[test]
fn native_placement_before_input_keeps_later_input_authority() {
    let (mut state, owner, _, _) = scene(ContainerKind::Chest);
    let before = record(&state, owner);
    submit(&mut state, owner, 1, place_with_empty_slot(0.75, 0.25));
    let input = placement_move_control(0.0, 0.0);
    submit(&mut state, owner, 2, Command::PlayerInput(input));
    let tick = state.advance_tick(TickBudget::full()).unwrap();
    let view = state.settled_read().unwrap();
    assert_eq!(
        view.actor(ActorKey::Player(owner)).unwrap().look,
        input.look()
    );
    assert_eq!(
        view.runtime(ActorKey::Player(owner)).unwrap().controls,
        Some(input)
    );
    assert_eq!(native_input_ack(&tick, owner), 2);
    assert_eq!(record(&state, owner), before);
}

#[test]
fn native_placement_invalid_pitch_preserves_held_basis_and_rejects_first() {
    let (mut state, owner, other, _) = scene(ContainerKind::Chest);
    let (mut reference, reference_owner, _, _) = scene(ContainerKind::Chest);
    for (authority, session) in [(&mut state, owner), (&mut reference, reference_owner)] {
        submit(
            authority,
            session,
            1,
            Command::PlayerInput(placement_move_control(0.0, 0.0)),
        );
        authority.advance_tick(TickBudget::full()).unwrap();
    }
    submit(&mut state, owner, 2, place_with_empty_slot(0.75, 2.0));
    let tick = state.advance_tick(TickBudget::full()).unwrap();
    reference.advance_tick(TickBudget::full()).unwrap();
    let view = state.settled_read().unwrap();
    let expected = reference.settled_read().unwrap();
    assert_eq!(
        view.actor(ActorKey::Player(owner)),
        expected.actor(ActorKey::Player(reference_owner))
    );
    assert_eq!(
        view.runtime(ActorKey::Player(owner)),
        expected.runtime(ActorKey::Player(reference_owner))
    );
    assert_eq!(record(&state, owner), record(&reference, reference_owner));
    assert_eq!(native_input_ack(&tick, owner), 1);
    let refusal = Event::CommandRejected(mornlea_domain::CommandRejection::new(
        2,
        mornlea_domain::RejectReason::InvalidInput,
    ));
    let events = events_for(&tick, owner);
    assert_eq!(events.first(), Some(&refusal));
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(event, Event::CommandRejected(_)))
            .count(),
        1
    );
    assert!(
        !events_for(&tick, other)
            .iter()
            .any(|event| matches!(event, Event::CommandRejected(_)))
    );
    assert!(
        !events
            .iter()
            .any(|event| matches!(event, Event::PlaceBlockSucceeded(_)))
    );
}

#[test]
fn native_placement_pitch_boundary_and_normalized_yaw_match_source_bits() {
    // These bits come from the executed Go look oracle, including f32 rounding.
    let bound = f32::from_bits(0x3fc7c82d);
    for (yaw, pitch, normalized) in [
        (0.0, -bound, 0.0),
        (0.0, bound, 0.0),
        (f32::from_bits(0x40d10fdb), 0.0, f32::from_bits(0x3e800006)),
    ] {
        let (mut state, owner, _, _) = scene(ContainerKind::Chest);
        submit(&mut state, owner, 1, place_with_empty_slot(yaw, pitch));
        let tick = state.advance_tick(TickBudget::full()).unwrap();
        assert_eq!(
            state
                .settled_read()
                .unwrap()
                .actor(ActorKey::Player(owner))
                .unwrap()
                .look,
            look(normalized, pitch)
        );
        assert_eq!(native_input_ack(&tick, owner), 0);
        assert!(
            !events_for(&tick, owner)
                .iter()
                .any(|event| matches!(event, Event::CommandRejected(_)))
        );
    }
}

#[test]
fn native_placement_held_and_same_tick_yaw_normalizes_once_near_pi() {
    let raw = f32::from_bits(0x4116cbe4);
    let normalized = f32::from_bits(0xc0490fdb);
    for same_tick in [true, false] {
        let (mut state, owner, _, _) = scene(ContainerKind::Chest);
        let (mut reference, reference_owner, _, _) = scene(ContainerKind::Chest);
        if !same_tick {
            for (authority, session) in [(&mut state, owner), (&mut reference, reference_owner)] {
                submit(
                    authority,
                    session,
                    1,
                    Command::PlayerInput(placement_move_control(0.0, 0.0)),
                );
                authority.advance_tick(TickBudget::full()).unwrap();
            }
        } else {
            submit(
                &mut state,
                owner,
                1,
                Command::PlayerInput(placement_move_control(0.0, 0.0)),
            );
        }
        submit(&mut state, owner, 2, place_with_empty_slot(raw, 0.25));
        let reference_sequence = if same_tick { 1 } else { 2 };
        submit(
            &mut reference,
            reference_owner,
            reference_sequence,
            Command::PlayerInput(placement_move_control(raw, 0.25)),
        );
        let tick = state.advance_tick(TickBudget::full()).unwrap();
        let reference_tick = reference.advance_tick(TickBudget::full()).unwrap();
        assert_eq!(
            state
                .settled_read()
                .unwrap()
                .actor(ActorKey::Player(owner))
                .unwrap()
                .look,
            look(normalized, 0.25)
        );
        assert_eq!(
            state
                .settled_read()
                .unwrap()
                .actor(ActorKey::Player(owner))
                .unwrap()
                .motion,
            reference
                .settled_read()
                .unwrap()
                .actor(ActorKey::Player(reference_owner))
                .unwrap()
                .motion
        );
        assert_eq!(native_input_ack(&tick, owner), 1);
        assert_eq!(
            native_input_ack(&reference_tick, reference_owner),
            reference_sequence
        );
        assert_eq!(record(&state, owner), record(&reference, reference_owner));
        state.advance_tick(TickBudget::full()).unwrap();
        reference.advance_tick(TickBudget::full()).unwrap();
        assert_eq!(
            state
                .settled_read()
                .unwrap()
                .actor(ActorKey::Player(owner))
                .unwrap()
                .look,
            look(normalized, 0.25)
        );
        assert_eq!(
            state
                .settled_read()
                .unwrap()
                .actor(ActorKey::Player(owner))
                .unwrap()
                .motion,
            reference
                .settled_read()
                .unwrap()
                .actor(ActorKey::Player(reference_owner))
                .unwrap()
                .motion
        );
    }
}

fn input_refusal_control(move_x: i8, move_z: i8, pitch: f32) -> PlayerControl {
    PlayerControl::new(PlayerControlParts {
        movement: Movement {
            move_x,
            move_z,
            jump: true,
        },
        look: look(0.75, pitch),
        actions: HeldActions {
            primary: true,
            eating: true,
            sprinting: true,
            sneaking: true,
        },
    })
}

fn assert_input_refusals(
    tick: &mornlea_server::contracts::TickPublication,
    owner: SessionKey,
    other: SessionKey,
    sequences: &[u64],
) {
    let expected: Vec<_> = sequences
        .iter()
        .map(|sequence| {
            Event::CommandRejected(mornlea_domain::CommandRejection::new(
                *sequence,
                mornlea_domain::RejectReason::InvalidInput,
            ))
        })
        .collect();
    let events = events_for(tick, owner);
    let actual: Vec<_> = events
        .iter()
        .filter(|event| matches!(event, Event::CommandRejected(_)))
        .cloned()
        .collect();
    assert_eq!(actual, expected);
    assert_eq!(&events[..expected.len()], expected.as_slice());
    assert!(
        !events_for(tick, other)
            .iter()
            .any(|event| matches!(event, Event::CommandRejected(_)))
    );
}

#[test]
fn native_input_refusal_preserves_look_inventory_and_acknowledges() {
    for (move_x, move_z, pitch) in [
        (-2, 0, 0.0),
        (2, 0, 0.0),
        (0, -2, 0.0),
        (0, 2, 0.0),
        (0, 0, -2.0),
        (0, 0, 2.0),
    ] {
        let (mut state, owner, other, _) = scene(ContainerKind::Chest);
        let (mut reference, reference_owner, _, _) = scene(ContainerKind::Chest);
        for (authority, session) in [(&mut state, owner), (&mut reference, reference_owner)] {
            submit(
                authority,
                session,
                1,
                Command::PlayerInput(placement_move_control(0.25, 0.1)),
            );
            authority.advance_tick(TickBudget::full()).unwrap();
        }
        let before_inventory = record(&state, owner);
        let before_look = state
            .settled_read()
            .unwrap()
            .actor(ActorKey::Player(owner))
            .unwrap()
            .look;
        submit(
            &mut state,
            owner,
            2,
            Command::PlayerInput(input_refusal_control(move_x, move_z, pitch)),
        );
        let neutral = PlayerControl::new(PlayerControlParts {
            movement: Movement {
                move_x: 0,
                move_z: 0,
                jump: false,
            },
            look: before_look,
            actions: HeldActions {
                primary: false,
                eating: false,
                sprinting: false,
                sneaking: false,
            },
        });
        submit(
            &mut reference,
            reference_owner,
            2,
            Command::PlayerInput(neutral),
        );
        let tick = state.advance_tick(TickBudget::full()).unwrap();
        reference.advance_tick(TickBudget::full()).unwrap();
        let view = state.settled_read().unwrap();
        let actor = ActorKey::Player(owner);
        let runtime = view.runtime(actor).unwrap();
        assert_eq!(runtime.controls, None);
        assert_eq!(runtime.eating, None);
        assert_eq!(runtime.bow, None);
        assert_eq!(view.mining(actor), None);
        assert_eq!(view.actor(actor).unwrap().look, before_look);
        assert_eq!(
            view.actor(actor).unwrap().motion,
            reference
                .settled_read()
                .unwrap()
                .actor(ActorKey::Player(reference_owner))
                .unwrap()
                .motion
        );
        assert_eq!(record(&state, owner), before_inventory);
        assert_eq!(native_input_ack(&tick, owner), 2);
        assert_input_refusals(&tick, owner, other, &[2]);
        submit(
            &mut state,
            owner,
            2,
            Command::PlayerInput(input_refusal_control(move_x, move_z, pitch)),
        );
        let stale = state.advance_tick(TickBudget::full()).unwrap();
        assert_input_refusals(&stale, owner, other, &[]);
        assert_eq!(native_input_ack(&stale, owner), 2);
        assert_eq!(
            state
                .settled_read()
                .unwrap()
                .runtime(actor)
                .unwrap()
                .controls,
            None
        );
    }
}

#[test]
fn native_input_refusal_then_valid_keeps_prefix_identity() {
    let (mut state, owner, other, _) = scene(ContainerKind::Chest);
    let before = record(&state, owner);
    submit(
        &mut state,
        owner,
        1,
        Command::PlayerInput(input_refusal_control(2, 0, 0.0)),
    );
    submit(
        &mut state,
        owner,
        2,
        Command::PlayerInput(input_refusal_control(0, 0, 2.0)),
    );
    let valid = placement_move_control(0.25, 0.1);
    submit(&mut state, owner, 3, Command::PlayerInput(valid));
    let tick = state.advance_tick(TickBudget::full()).unwrap();
    assert_eq!(native_input_ack(&tick, owner), 3);
    let view = state.settled_read().unwrap();
    assert_eq!(
        view.runtime(ActorKey::Player(owner)).unwrap().controls,
        Some(valid)
    );
    assert_eq!(
        view.actor(ActorKey::Player(owner)).unwrap().look,
        valid.look()
    );
    assert_eq!(record(&state, owner), before);
    assert_input_refusals(&tick, owner, other, &[1, 2]);
    submit(
        &mut state,
        owner,
        2,
        Command::PlayerInput(input_refusal_control(2, 0, 0.0)),
    );
    let stale = state.advance_tick(TickBudget::full()).unwrap();
    assert_input_refusals(&stale, owner, other, &[]);
    assert_eq!(native_input_ack(&stale, owner), 3);
}

#[test]
fn native_input_refusal_interrupts_actual_eating_before_completion() {
    let (mut state, owner, other, _) = scene(ContainerKind::Chest);
    submit(&mut state, owner, 1, held_eating());
    let required = RuleTunables::source_defaults().eating_ticks();
    for _ in 0..required - 1 {
        state.advance_tick(TickBudget::full()).unwrap();
    }
    assert_eq!(eating_ticks(&state, owner), Some(required - 1));
    let before = record(&state, owner);
    let before_survival = state
        .settled_read()
        .unwrap()
        .actor(ActorKey::Player(owner))
        .unwrap()
        .survival;
    submit(
        &mut state,
        owner,
        2,
        Command::PlayerInput(input_refusal_control(2, 0, 0.0)),
    );
    let tick = state.advance_tick(TickBudget::full()).unwrap();
    assert_eq!(eating_ticks(&state, owner), None);
    assert_eq!(record(&state, owner), before);
    assert_eq!(
        state
            .settled_read()
            .unwrap()
            .actor(ActorKey::Player(owner))
            .unwrap()
            .survival
            .health(),
        before_survival.health()
    );
    assert_eq!(
        state
            .settled_read()
            .unwrap()
            .actor(ActorKey::Player(owner))
            .unwrap()
            .survival
            .hunger(),
        before_survival.hunger()
    );
    assert!(projection_inventory_states(&events_for(&tick, owner)).is_empty());
    assert_eq!(native_input_ack(&tick, owner), 2);
    assert_input_refusals(&tick, owner, other, &[2]);
    let idle = state.advance_tick(TickBudget::full()).unwrap();
    assert_eq!(record(&state, owner), before);
    assert_eq!(native_input_ack(&idle, owner), 2);
    assert_input_refusals(&idle, owner, other, &[]);
}

#[test]
fn native_input_refusal_clears_prepared_bow_and_mining_without_release() {
    let (mut state, owner, other, _) = scene(ContainerKind::Chest);
    let actor = ActorKey::Player(owner);
    // Action progress is a prepared cause; the refused input and consumers are real.
    stage(&mut state, |context| {
        let mut runtime = context.read().runtime(actor).unwrap().clone();
        runtime.controls = Some(input_refusal_control(0, 0, 0.0));
        runtime.bow = Some(mornlea_server::contracts::BowProgress {
            slot: HotbarSlot::new(3).unwrap(),
            ticks: 15,
        });
        context.stage(RuleEffect::Runtime(runtime)).unwrap();
        context
            .stage(RuleEffect::Mining {
                actor,
                progress: Some(mornlea_server::contracts::MiningProgress {
                    actor,
                    dimension: Dimension::OVERWORLD,
                    target: BlockPos::new(0, 64, 0),
                    observed_block: 1,
                    tool_slot: HotbarSlot::new(0).unwrap(),
                    tool: StorageStack::default(),
                    elapsed: 3,
                    required: 10,
                    last_tick: 0,
                }),
            })
            .unwrap();
    });
    let before = record(&state, owner);
    submit(
        &mut state,
        owner,
        1,
        Command::PlayerInput(input_refusal_control(2, 0, 0.0)),
    );
    let tick = state.advance_tick(TickBudget::full()).unwrap();
    let view = state.settled_read().unwrap();
    let runtime = view.runtime(actor).unwrap();
    assert_eq!(runtime.controls, None);
    assert_eq!(runtime.bow, None);
    assert_eq!(runtime.eating, None);
    assert_eq!(view.mining(actor), None);
    assert_eq!(record(&state, owner), before);
    assert!(view.projectiles().is_empty());
    for x in -1..=1 {
        for z in -1..=1 {
            assert!(view.drops(chunk_key(x, z)).is_empty());
        }
    }
    assert!(!events_for(&tick, owner).iter().any(|event| matches!(
        event,
        Event::BlockChanges(_) | Event::ProjectileSpawn(_) | Event::ItemDropUpserts(_)
    )));
    assert_eq!(native_input_ack(&tick, owner), 1);
    assert_input_refusals(&tick, owner, other, &[1]);
}

fn prepared_inventory_outcome(
    state: &mut AuthorityState,
    owner: SessionKey,
    edit: impl FnOnce(&mut mornlea_server::contracts::InventoryRecord),
) {
    // Inventory is prepared; the following command and publication consumers are real.
    stage(state, |context| {
        let actor = ActorKey::Player(owner);
        let mut inventory = *context.read().inventory(actor).unwrap();
        edit(&mut inventory);
        context.preload_inventory(actor, inventory);
    });
    state.advance_tick(TickBudget::full()).unwrap();
}

fn assert_inventory_outcome_refusal(
    tick: &mornlea_server::contracts::TickPublication,
    owner: SessionKey,
    other: SessionKey,
    sequence: u64,
    reason: mornlea_domain::RejectReason,
) {
    let expected = Event::CommandRejected(mornlea_domain::CommandRejection::new(sequence, reason));
    let events = events_for(tick, owner);
    assert_eq!(events.first(), Some(&expected));
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(event, Event::CommandRejected(_)))
            .count(),
        1
    );
    assert!(
        !events_for(tick, other)
            .iter()
            .any(|event| matches!(event, Event::CommandRejected(_)))
    );
    assert_eq!(native_input_ack(tick, owner), 0);
}

fn inventory_outcome_moves(from: u8, to: u8) -> [Command; 4] {
    [
        Command::MoveInventory(mornlea_domain::InventoryMove::try_new(from, to).unwrap()),
        Command::MovePartial(PartialMove::try_new(StackView::Inventory, from, to, false).unwrap()),
        Command::MovePartial(PartialMove::try_new(StackView::Inventory, from, to, true).unwrap()),
        Command::QuickMove(StackSource::try_new(StackView::Inventory, from).unwrap()),
    ]
}

#[test]
fn native_inventory_outcome_empty_sources() {
    for command in inventory_outcome_moves(5, 6) {
        let (mut state, owner, other, _) = scene(ContainerKind::Chest);
        let before = record(&state, owner);
        submit(&mut state, owner, 1, command);
        let tick = state.advance_tick(TickBudget::full()).unwrap();
        assert_eq!(record(&state, owner), before);
        assert!(projection_inventory_states(&events_for(&tick, owner)).is_empty());
        assert_inventory_outcome_refusal(
            &tick,
            owner,
            other,
            1,
            mornlea_domain::RejectReason::InvalidInput,
        );
    }
}

#[test]
fn native_inventory_outcome_full_destinations() {
    for command in inventory_outcome_moves(0, 9) {
        let (mut state, owner, other, _) = scene(ContainerKind::Chest);
        prepared_inventory_outcome(&mut state, owner, |inventory| {
            inventory.slots[0] = storage(ITEM_DIRT, 5);
            for slot in &mut inventory.slots[9..] {
                *slot = storage(ITEM_DIRT, 64);
            }
        });
        let before = record(&state, owner);
        submit(&mut state, owner, 1, command);
        let tick = state.advance_tick(TickBudget::full()).unwrap();
        assert_eq!(record(&state, owner), before);
        assert!(projection_inventory_states(&events_for(&tick, owner)).is_empty());
        assert_inventory_outcome_refusal(
            &tick,
            owner,
            other,
            1,
            mornlea_domain::RejectReason::InvalidInput,
        );
    }
}

#[test]
fn native_inventory_outcome_equip_not_armor() {
    for empty in [false, true] {
        let (mut state, owner, other, _) = scene(ContainerKind::Chest);
        if empty {
            prepared_inventory_outcome(&mut state, owner, |inventory| {
                inventory.selected = HotbarSlot::new(8).unwrap();
            });
        }
        let before = record(&state, owner);
        submit(&mut state, owner, 1, Command::EquipArmor);
        let tick = state.advance_tick(TickBudget::full()).unwrap();
        assert_eq!(record(&state, owner), before);
        assert!(projection_inventory_states(&events_for(&tick, owner)).is_empty());
        assert_inventory_outcome_refusal(
            &tick,
            owner,
            other,
            1,
            mornlea_domain::RejectReason::NotArmor,
        );
    }
}

#[test]
fn native_inventory_outcome_refusal_then_success_retains_prefix() {
    let (mut state, owner, other, _) = scene(ContainerKind::Chest);
    prepared_inventory_outcome(&mut state, owner, |inventory| {
        inventory.slots[0] = storage(ITEM_DIRT, 5);
    });
    let mut expected = record(&state, owner);
    expected.slots[0] = StorageStack::default();
    expected.slots[9] = storage(ITEM_DIRT, 5);
    submit(&mut state, owner, 1, inventory_outcome_moves(5, 6)[0]);
    submit(&mut state, owner, 2, inventory_outcome_moves(0, 9)[0]);
    let tick = state.advance_tick(TickBudget::full()).unwrap();
    assert_eq!(record(&state, owner), expected);
    assert_eq!(state.session(owner).unwrap().last_applied_sequence, 2);
    assert_eq!(
        projection_inventory_states(&events_for(&tick, owner)).len(),
        1
    );
    assert!(projection_inventory_states(&events_for(&tick, other)).is_empty());
    assert_inventory_outcome_refusal(
        &tick,
        owner,
        other,
        1,
        mornlea_domain::RejectReason::InvalidInput,
    );
    submit(&mut state, owner, 1, inventory_outcome_moves(5, 6)[0]);
    let stale = state.advance_tick(TickBudget::full()).unwrap();
    assert!(projection_inventory_states(&events_for(&stale, owner)).is_empty());
    assert!(
        !events_for(&stale, owner)
            .iter()
            .any(|event| matches!(event, Event::CommandRejected(_)))
    );
    assert_eq!(record(&state, owner), expected);
}

#[test]
fn native_inventory_outcome_partial_never_swaps() {
    for whole in [false, true] {
        let (mut state, owner, other, _) = scene(ContainerKind::Chest);
        prepared_inventory_outcome(&mut state, owner, |inventory| {
            inventory.slots[0] = storage(ITEM_DIRT, 5);
            inventory.slots[9] = storage(1, 3);
        });
        let mut expected = record(&state, owner);
        let command = inventory_outcome_moves(0, 9)[if whole { 0 } else { 1 }];
        submit(&mut state, owner, 1, command);
        let tick = state.advance_tick(TickBudget::full()).unwrap();
        if whole {
            expected.slots.swap(0, 9);
            assert_eq!(record(&state, owner), expected);
            assert_eq!(
                projection_inventory_states(&events_for(&tick, owner)).len(),
                1
            );
            assert!(
                !events_for(&tick, owner)
                    .iter()
                    .any(|event| matches!(event, Event::CommandRejected(_)))
            );
            assert!(projection_inventory_states(&events_for(&tick, other)).is_empty());
        } else {
            assert_eq!(record(&state, owner), expected);
            assert!(projection_inventory_states(&events_for(&tick, owner)).is_empty());
            assert_inventory_outcome_refusal(
                &tick,
                owner,
                other,
                1,
                mornlea_domain::RejectReason::InvalidInput,
            );
        }
    }
}

#[test]
fn native_inventory_outcome_absorption_and_equal_equip_succeed() {
    for mode in 0..3 {
        let (mut state, owner, other, _) = scene(ContainerKind::Chest);
        prepared_inventory_outcome(&mut state, owner, |inventory| {
            if mode == 2 {
                let helmet = StorageStack {
                    item: 58,
                    count: 1,
                    durability: 165,
                };
                inventory.slots[0] = helmet;
                inventory.armor[0] = helmet;
            } else {
                inventory.slots[0] = storage(ITEM_DIRT, 5);
                if mode == 0 {
                    for slot in &mut inventory.slots[9..] {
                        *slot = storage(1, 64);
                    }
                    inventory.slots[9] = storage(ITEM_DIRT, 63);
                }
            }
        });
        let mut expected = record(&state, owner);
        let command = match mode {
            0 => {
                expected.slots[0] = storage(ITEM_DIRT, 4);
                expected.slots[9] = storage(ITEM_DIRT, 64);
                inventory_outcome_moves(0, 9)[3]
            }
            1 => {
                expected.slots[0] = storage(ITEM_DIRT, 2);
                expected.slots[9] = storage(ITEM_DIRT, 3);
                inventory_outcome_moves(0, 9)[1]
            }
            _ => Command::EquipArmor,
        };
        submit(&mut state, owner, 1, command);
        let tick = state.advance_tick(TickBudget::full()).unwrap();
        assert_eq!(record(&state, owner), expected);
        assert_eq!(
            projection_inventory_states(&events_for(&tick, owner)).len(),
            1
        );
        assert!(projection_inventory_states(&events_for(&tick, other)).is_empty());
        assert!(
            !events_for(&tick, owner)
                .iter()
                .any(|event| matches!(event, Event::CommandRejected(_)))
        );
        assert_eq!(native_input_ack(&tick, owner), 0);
    }
}

fn crafting_outcome_moves(from: u8, to: u8) -> [Command; 4] {
    [
        Command::MoveCrafting(mornlea_domain::CraftingMove::try_new(from, to).unwrap()),
        Command::MovePartial(PartialMove::try_new(StackView::Crafting, from, to, false).unwrap()),
        Command::MovePartial(PartialMove::try_new(StackView::Crafting, from, to, true).unwrap()),
        Command::QuickMove(StackSource::try_new(StackView::Crafting, from).unwrap()),
    ]
}

fn assert_crafting_outcome_quiet(
    state: &AuthorityState,
    tick: &mornlea_server::contracts::TickPublication,
    owner: SessionKey,
    other: SessionKey,
    before: mornlea_server::contracts::InventoryRecord,
    reason: mornlea_domain::RejectReason,
) {
    assert_eq!(record(state, owner), before);
    assert!(projection_crafting_record_states(&events_for(tick, owner)).is_empty());
    assert!(projection_crafting_record_states(&events_for(tick, other)).is_empty());
    assert_inventory_outcome_refusal(tick, owner, other, 1, reason);
}

#[test]
fn native_crafting_outcome_personal_extent() {
    let commands = crafting_outcome_moves(4, 9)
        .into_iter()
        .chain(crafting_outcome_moves(9, 4).into_iter().take(3));
    for command in commands {
        let (mut state, owner, other, _) = scene(ContainerKind::Chest);
        let before = record(&state, owner);
        submit(&mut state, owner, 1, command);
        let tick = state.advance_tick(TickBudget::full()).unwrap();
        assert_crafting_outcome_quiet(
            &state,
            &tick,
            owner,
            other,
            before,
            mornlea_domain::RejectReason::InvalidSlot,
        );
    }
}

#[test]
fn native_crafting_outcome_partial_pack_pair() {
    for single in [false, true] {
        let (mut state, owner, other, _) = scene(ContainerKind::Chest);
        prepared_inventory_outcome(&mut state, owner, |inventory| {
            inventory.slots[0] = storage(ITEM_DIRT, 5);
        });
        let before = record(&state, owner);
        submit(
            &mut state,
            owner,
            1,
            Command::MovePartial(PartialMove::try_new(StackView::Crafting, 9, 10, single).unwrap()),
        );
        let tick = state.advance_tick(TickBudget::full()).unwrap();
        assert_crafting_outcome_quiet(
            &state,
            &tick,
            owner,
            other,
            before,
            mornlea_domain::RejectReason::InvalidInput,
        );
    }
}

#[test]
fn native_crafting_outcome_empty_and_no_recipe() {
    for command in crafting_outcome_moves(0, 9)
        .into_iter()
        .chain([Command::TakeCraftingOutput])
    {
        let (mut state, owner, other, _) = scene(ContainerKind::Chest);
        let before = record(&state, owner);
        submit(&mut state, owner, 1, command);
        let tick = state.advance_tick(TickBudget::full()).unwrap();
        assert_crafting_outcome_quiet(
            &state,
            &tick,
            owner,
            other,
            before,
            mornlea_domain::RejectReason::InvalidInput,
        );
    }
}

#[test]
fn native_crafting_outcome_no_absorption() {
    for command in crafting_outcome_moves(0, 9) {
        let (mut state, owner, other, _) = scene(ContainerKind::Chest);
        // This non-repackable inventory is a prepared consumer cause.
        prepared_inventory_outcome(&mut state, owner, |inventory| {
            inventory.slots = [storage(ITEM_DIRT, 64); 36];
            inventory.crafting[0] = storage(ITEM_DIRT, 5);
        });
        let before = record(&state, owner);
        submit(&mut state, owner, 1, command);
        let tick = state.advance_tick(TickBudget::full()).unwrap();
        assert_crafting_outcome_quiet(
            &state,
            &tick,
            owner,
            other,
            before,
            mornlea_domain::RejectReason::InvalidInput,
        );
    }
}

#[test]
fn native_crafting_outcome_output_full() {
    let (mut state, owner, other, _) = scene(ContainerKind::Chest);
    // A prepared full pack isolates output insertion, not upstream repack production.
    prepared_inventory_outcome(&mut state, owner, |inventory| {
        inventory.slots = [storage(1, 64); 36];
        inventory.crafting[0] = storage(20, 2);
    });
    let before = record(&state, owner);
    submit(&mut state, owner, 1, Command::TakeCraftingOutput);
    let tick = state.advance_tick(TickBudget::full()).unwrap();
    assert_crafting_outcome_quiet(
        &state,
        &tick,
        owner,
        other,
        before,
        mornlea_domain::RejectReason::InvalidInput,
    );
}

#[test]
fn native_crafting_outcome_extent_then_open_and_success() {
    let (mut state, owner, other, _) = scene(ContainerKind::Chest);
    prepared_inventory_outcome(&mut state, owner, |inventory| {
        inventory.slots[0] = storage(ITEM_DIRT, 5);
    });
    let mut expected = record(&state, owner);
    expected.crafting_size = CraftingSize::Workbench;
    expected.crafting[4] = storage(ITEM_DIRT, 5);
    expected.slots[0] = StorageStack::default();
    submit(&mut state, owner, 1, crafting_outcome_moves(9, 4)[0]);
    open_bench(&mut state, owner, 2);
    submit(&mut state, owner, 3, crafting_outcome_moves(9, 4)[0]);
    let tick = state.advance_tick(TickBudget::full()).unwrap();
    assert_eq!(record(&state, owner), expected);
    assert_eq!(state.session(owner).unwrap().last_applied_sequence, 3);
    assert_eq!(
        projection_inventory_states(&events_for(&tick, owner)).len(),
        1
    );
    assert_eq!(
        projection_crafting_record_states(&events_for(&tick, owner)).len(),
        2
    );
    assert!(projection_crafting_record_states(&events_for(&tick, other)).is_empty());
    assert_inventory_outcome_refusal(
        &tick,
        owner,
        other,
        1,
        mornlea_domain::RejectReason::InvalidSlot,
    );
    submit(&mut state, owner, 1, crafting_outcome_moves(9, 4)[0]);
    let quiet = state.advance_tick(TickBudget::full()).unwrap();
    assert_eq!(record(&state, owner), expected);
    assert!(projection_crafting_record_states(&events_for(&quiet, owner)).is_empty());
    assert!(
        !events_for(&quiet, owner)
            .iter()
            .any(|event| matches!(event, Event::CommandRejected(_)))
    );
}

#[test]
fn native_crafting_outcome_success_round_trips_and_output() {
    for mode in 0..4 {
        let (mut state, owner, other, _) = scene(ContainerKind::Chest);
        prepared_inventory_outcome(&mut state, owner, |inventory| {
            inventory.slots[0] = if mode == 3 {
                storage(20, 2)
            } else {
                storage(ITEM_DIRT, 5)
            };
        });
        let mut expected = record(&state, owner);
        let outward = match mode {
            1 => crafting_outcome_moves(9, 0)[1],
            2 => crafting_outcome_moves(9, 0)[3],
            _ => crafting_outcome_moves(9, 0)[0],
        };
        let inward = match mode {
            0 => crafting_outcome_moves(0, 9)[0],
            3 => {
                expected.slots[0] = storage(21, 4);
                expected.crafting[0] = storage(20, 1);
                Command::TakeCraftingOutput
            }
            _ => crafting_outcome_moves(0, 9)[3],
        };
        submit(&mut state, owner, 1, outward);
        submit(&mut state, owner, 2, inward);
        let tick = state.advance_tick(TickBudget::full()).unwrap();
        assert_eq!(record(&state, owner), expected);
        assert_eq!(
            projection_inventory_states(&events_for(&tick, owner)).len(),
            1
        );
        assert_eq!(
            projection_crafting_record_states(&events_for(&tick, owner)).len(),
            2
        );
        assert!(projection_crafting_record_states(&events_for(&tick, other)).is_empty());
        assert!(
            !events_for(&tick, owner)
                .iter()
                .any(|event| matches!(event, Event::CommandRejected(_)))
        );
        assert_eq!(native_input_ack(&tick, owner), 0);
        let quiet = state.advance_tick(TickBudget::full()).unwrap();
        assert_eq!(record(&state, owner), expected);
        assert!(projection_crafting_record_states(&events_for(&quiet, owner)).is_empty());
    }
}

fn lifecycle_sneak(sneaking: bool) -> Command {
    Command::PlayerInput(PlayerControl::new(PlayerControlParts {
        movement: Movement {
            move_x: 0,
            move_z: 0,
            jump: false,
        },
        look: look(0.0, 0.0),
        actions: HeldActions {
            primary: false,
            eating: false,
            sprinting: false,
            sneaking,
        },
    }))
}

fn lifecycle_refusal(
    tick: &mornlea_server::contracts::TickPublication,
    owner: SessionKey,
    other: SessionKey,
    sequence: u64,
    reason: mornlea_domain::RejectReason,
    ack: u64,
) {
    let expected = Event::CommandRejected(mornlea_domain::CommandRejection::new(sequence, reason));
    let events = events_for(tick, owner);
    assert_eq!(events.first(), Some(&expected));
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(event, Event::CommandRejected(_)))
            .count(),
        1
    );
    assert!(
        !events_for(tick, other)
            .iter()
            .any(|event| matches!(event, Event::CommandRejected(_)))
    );
    assert_eq!(native_input_ack(tick, owner), ack);
}

fn replace_forward_lifecycle_target(state: &mut AuthorityState, block: u16) {
    // Ready geometry and missing physical slots are prepared causes for real command consumers.
    stage(state, |context| {
        let mut front = ground_chunk();
        set_cell(&mut front, BlockPos::new(0, 66, -1), block);
        context.preload_ready_chunk(ReadyChunk::try_new(chunk_key(0, -1), 1, 1, front).unwrap());
    });
}

#[test]
fn native_lifecycle_outcome_physical_slot_sneak_precedence() {
    for (kind, block) in [(ContainerKind::Chest, 11), (ContainerKind::Furnace, 9)] {
        for sneaking in [false, true] {
            let (mut state, owner, other, _) = scene(kind);
            replace_forward_lifecycle_target(&mut state, block);
            let before = record(&state, owner);
            submit(&mut state, owner, 1, lifecycle_sneak(sneaking));
            submit(&mut state, owner, 2, Command::OpenContainer(look(0.0, 0.0)));
            let tick = state.advance_tick(TickBudget::full()).unwrap();
            assert_eq!(record(&state, owner), before);
            assert!(state.settled_read().unwrap().viewer(owner).is_none());
            assert!(projection_crafting_record_states(&events_for(&tick, owner)).is_empty());
            assert!(projection_crafting_record_states(&events_for(&tick, other)).is_empty());
            assert_eq!(state.session(owner).unwrap().last_applied_sequence, 2);
            lifecycle_refusal(
                &tick,
                owner,
                other,
                2,
                if sneaking {
                    mornlea_domain::RejectReason::InvalidInput
                } else {
                    mornlea_domain::RejectReason::NoTarget
                },
                1,
            );
        }
    }
}

#[test]
fn native_lifecycle_outcome_unrecognized_sneak() {
    for block in [0, 1] {
        let (mut state, owner, other, _) = scene(ContainerKind::Chest);
        replace_forward_lifecycle_target(&mut state, block);
        let before = record(&state, owner);
        submit(&mut state, owner, 1, lifecycle_sneak(true));
        submit(&mut state, owner, 2, Command::OpenContainer(look(0.0, 0.0)));
        let tick = state.advance_tick(TickBudget::full()).unwrap();
        assert_eq!(record(&state, owner), before);
        assert!(state.settled_read().unwrap().viewer(owner).is_none());
        assert!(projection_crafting_record_states(&events_for(&tick, owner)).is_empty());
        lifecycle_refusal(
            &tick,
            owner,
            other,
            2,
            mornlea_domain::RejectReason::NoTarget,
            1,
        );
    }
}

#[test]
fn native_lifecycle_outcome_unavailable_ray() {
    let (mut state, owner, other, _) = scene(ContainerKind::Chest);
    // The owner stays over Ready ground; only the forward ray crosses unavailable geometry.
    stage(&mut state, |context| {
        let mut actor = context
            .read()
            .actor(ActorKey::Player(owner))
            .cloned()
            .unwrap();
        actor.motion = MotionState::new(MotionStateParts {
            position: FiniteVec3::try_new([0.5, 65.0, 31.5]).unwrap(),
            velocity: FiniteVec3::try_new([0.0; 3]).unwrap(),
            on_ground: true,
        });
        context.stage(RuleEffect::Actor(actor)).unwrap();
    });
    let before = record(&state, owner);
    submit(
        &mut state,
        owner,
        1,
        Command::OpenContainer(look(std::f32::consts::PI, 0.0)),
    );
    let tick = state.advance_tick(TickBudget::full()).unwrap();
    assert_eq!(record(&state, owner), before);
    assert!(state.settled_read().unwrap().viewer(owner).is_none());
    assert!(projection_crafting_record_states(&events_for(&tick, owner)).is_empty());
    lifecycle_refusal(
        &tick,
        owner,
        other,
        1,
        mornlea_domain::RejectReason::ChunkNotReady,
        0,
    );
}

#[test]
fn native_lifecycle_outcome_bench_sneak_preserves_view() {
    let (mut state, owner, other, reference) = scene(ContainerKind::Chest);
    submit(&mut state, owner, 1, Command::OpenContainer(look(0.0, 0.0)));
    state.advance_tick(TickBudget::full()).unwrap();
    let before = record(&state, owner);
    submit(&mut state, owner, 2, lifecycle_sneak(true));
    open_bench(&mut state, owner, 3);
    let tick = state.advance_tick(TickBudget::full()).unwrap();
    assert_eq!(record(&state, owner), before);
    assert_eq!(
        state
            .settled_read()
            .unwrap()
            .viewer(owner)
            .unwrap()
            .reference(),
        reference
    );
    assert!(projection_crafting_record_states(&events_for(&tick, owner)).is_empty());
    lifecycle_refusal(
        &tick,
        owner,
        other,
        3,
        mornlea_domain::RejectReason::InvalidInput,
        2,
    );
}

#[test]
fn native_lifecycle_outcome_failed_close_preserves_all() {
    let (mut state, owner, other, reference) = scene(ContainerKind::Chest);
    open_bench(&mut state, owner, 1);
    submit(&mut state, owner, 2, Command::OpenContainer(look(0.0, 0.0)));
    state.advance_tick(TickBudget::full()).unwrap();
    // The impossible repack is prepared; the actual open established a valid bench anchor.
    prepared_inventory_outcome(&mut state, owner, |inventory| {
        inventory.slots = [storage(1, 64); 36];
        inventory.crafting[4] = storage(ITEM_DIRT, 5);
    });
    let before = record(&state, owner);
    let anchor = state.residents().runtimes[&ActorKey::Player(owner)]
        .aux
        .clone();
    submit(&mut state, owner, 3, Command::CloseContainer);
    let tick = state.advance_tick(TickBudget::full()).unwrap();
    assert_eq!(record(&state, owner), before);
    assert_eq!(
        state.residents().runtimes[&ActorKey::Player(owner)].aux,
        anchor
    );
    assert_eq!(
        state
            .settled_read()
            .unwrap()
            .viewer(owner)
            .unwrap()
            .reference(),
        reference
    );
    assert!(projection_crafting_record_states(&events_for(&tick, owner)).is_empty());
    assert!(projection_crafting_record_states(&events_for(&tick, other)).is_empty());
    lifecycle_refusal(
        &tick,
        owner,
        other,
        3,
        mornlea_domain::RejectReason::InvalidInput,
        0,
    );
}

#[test]
fn native_lifecycle_outcome_refusal_then_valid_open() {
    let (mut state, owner, other, reference) = scene(ContainerKind::Chest);
    let before = record(&state, owner);
    submit(
        &mut state,
        owner,
        1,
        Command::OpenContainer(look(std::f32::consts::FRAC_PI_2, 0.0)),
    );
    submit(&mut state, owner, 2, Command::OpenContainer(look(0.0, 0.0)));
    let tick = state.advance_tick(TickBudget::full()).unwrap();
    assert_eq!(record(&state, owner), before);
    assert_eq!(
        state
            .settled_read()
            .unwrap()
            .viewer(owner)
            .unwrap()
            .reference(),
        reference
    );
    assert!(
        events_for(&tick, owner)
            .iter()
            .any(|event| matches!(event, Event::ChestState(_)))
    );
    assert!(projection_crafting_record_states(&events_for(&tick, owner)).is_empty());
    assert_eq!(state.session(owner).unwrap().last_applied_sequence, 2);
    lifecycle_refusal(
        &tick,
        owner,
        other,
        1,
        mornlea_domain::RejectReason::NoTarget,
        0,
    );
    submit(
        &mut state,
        owner,
        1,
        Command::OpenContainer(look(std::f32::consts::FRAC_PI_2, 0.0)),
    );
    let quiet = state.advance_tick(TickBudget::full()).unwrap();
    assert_eq!(record(&state, owner), before);
    assert!(
        !events_for(&quiet, owner)
            .iter()
            .any(|event| matches!(event, Event::CommandRejected(_)))
    );
}

#[test]
fn native_lifecycle_outcome_success_close_intent() {
    let (mut state, owner, other, _) = scene(ContainerKind::Chest);
    let before = record(&state, owner);
    open_bench(&mut state, owner, 1);
    submit(&mut state, owner, 2, Command::CloseContainer);
    let tick = state.advance_tick(TickBudget::full()).unwrap();
    assert_eq!(record(&state, owner), before);
    assert_eq!(
        projection_inventory_states(&events_for(&tick, owner)).len(),
        1
    );
    assert_eq!(
        projection_crafting_record_states(&events_for(&tick, owner)).len(),
        2
    );
    assert!(projection_crafting_record_states(&events_for(&tick, other)).is_empty());
    assert!(
        !events_for(&tick, owner)
            .iter()
            .any(|event| matches!(event, Event::CommandRejected(_)))
    );
    assert_eq!(native_input_ack(&tick, owner), 0);
    submit(&mut state, owner, 3, Command::CloseContainer);
    let quiet = state.advance_tick(TickBudget::full()).unwrap();
    assert!(projection_crafting_record_states(&events_for(&quiet, owner)).is_empty());
    assert!(
        !events_for(&quiet, owner)
            .iter()
            .any(|event| matches!(event, Event::CommandRejected(_)))
    );
}

fn transfer_outcome_commands(reference: ContainerRef, from: u8, to: u8) -> [Command; 3] {
    [
        Command::MoveContainer(
            mornlea_domain::ContainerMove::try_new(
                reference.chunk(),
                reference.kind(),
                reference.slot(),
                reference.generation(),
                from,
                to,
            )
            .unwrap(),
        ),
        Command::MovePartial(
            PartialMove::try_new(StackView::Container(reference), from, to, false).unwrap(),
        ),
        Command::QuickMove(StackSource::try_new(StackView::Container(reference), from).unwrap()),
    ]
}

fn transfer_outcome_open(state: &mut AuthorityState, owner: SessionKey) {
    submit(state, owner, 1, Command::OpenContainer(look(0.0, 0.0)));
    let tick = state.advance_tick(TickBudget::full()).unwrap();
    assert!(
        !events_for(&tick, owner)
            .iter()
            .any(|e| matches!(e, Event::CommandRejected(_)))
    );
    assert!(state.settled_read().unwrap().viewer(owner).is_some());
}

fn transfer_outcome_quiet(
    state: &mut AuthorityState,
    owner: SessionKey,
    other: SessionKey,
    sequence: u64,
    command: Command,
) {
    let inventory = record(state, owner);
    let containers = state.residents().container_records();
    let lease = state.settled_read().unwrap().viewer(owner);
    submit(state, owner, sequence, command);
    let tick = state.advance_tick(TickBudget::full()).unwrap();
    assert_eq!(record(state, owner), inventory);
    assert_eq!(state.residents().container_records(), containers);
    assert_eq!(state.settled_read().unwrap().viewer(owner), lease);
    assert!(projection_crafting_record_states(&events_for(&tick, owner)).is_empty());
    assert!(projection_crafting_record_states(&events_for(&tick, other)).is_empty());
    assert_inventory_outcome_refusal(
        &tick,
        owner,
        other,
        sequence,
        mornlea_domain::RejectReason::InvalidInput,
    );
}

#[test]
fn native_transfer_outcome_no_view() {
    for index in 0..3 {
        let (mut state, owner, other, reference) = scene(ContainerKind::Chest);
        transfer_outcome_quiet(
            &mut state,
            owner,
            other,
            1,
            transfer_outcome_commands(reference, 36, 0)[index],
        );
    }
}

#[test]
fn native_transfer_outcome_empty_source() {
    for index in 0..3 {
        let (mut state, owner, other, reference) = scene(ContainerKind::Chest);
        transfer_outcome_open(&mut state, owner);
        transfer_outcome_quiet(
            &mut state,
            owner,
            other,
            2,
            transfer_outcome_commands(reference, 37, 0)[index],
        );
    }
}

#[test]
fn native_transfer_outcome_stale_reference() {
    for index in 0..3 {
        let (mut state, owner, other, reference) = scene(ContainerKind::Chest);
        transfer_outcome_open(&mut state, owner);
        let stale = ContainerRef::try_new(reference.chunk(), reference.kind(), reference.slot(), 2)
            .unwrap();
        transfer_outcome_quiet(
            &mut state,
            owner,
            other,
            2,
            transfer_outcome_commands(stale, 36, 0)[index],
        );
    }
}

#[test]
fn native_transfer_outcome_furnace_output_destination() {
    for single in [false, true] {
        let (mut state, owner, other, reference) = scene(ContainerKind::Furnace);
        transfer_outcome_open(&mut state, owner);
        prepared_inventory_outcome(&mut state, owner, |inv| {
            inv.slots[0] = storage(ITEM_RAW_IRON, 3)
        });
        let command = Command::MovePartial(
            PartialMove::try_new(StackView::Container(reference), 0, 38, single).unwrap(),
        );
        transfer_outcome_quiet(&mut state, owner, other, 2, command);
    }
}

#[test]
fn native_transfer_outcome_quick_no_absorption() {
    for source in [36, 0] {
        let (mut state, owner, other, reference) = scene(ContainerKind::Chest);
        transfer_outcome_open(&mut state, owner);
        prepared_inventory_outcome(&mut state, owner, |inv| {
            inv.slots.fill(storage(ITEM_STONE, 64))
        });
        if source == 0 {
            // Full physical storage is a prepared capacity cause for the real quick move.
            stage(&mut state, |context| {
                let mut front = ground_chunk();
                assert_eq!(
                    chest_in_chunk(
                        &mut front,
                        BlockPos::new(0, 66, -1),
                        [storage(ITEM_DIRT, 64); 27]
                    ),
                    reference
                );
                context.preload_ready_chunk(
                    ReadyChunk::try_new(chunk_key(0, -1), 1, 1, front).unwrap(),
                );
            });
            state.advance_tick(TickBudget::full()).unwrap();
        }
        let command = Command::QuickMove(
            StackSource::try_new(StackView::Container(reference), source).unwrap(),
        );
        transfer_outcome_quiet(&mut state, owner, other, 2, command);
    }
}

#[test]
fn native_transfer_outcome_repack() {
    for index in 0..3 {
        let (mut state, owner, other, reference) = scene(ContainerKind::Chest);
        transfer_outcome_open(&mut state, owner);
        prepared_inventory_outcome(&mut state, owner, |inv| {
            inv.slots.fill(storage(ITEM_STONE, 64));
            inv.slots[0] = StorageStack::default();
            inv.crafting[0] = storage(ITEM_DIRT, 64);
        });
        transfer_outcome_quiet(
            &mut state,
            owner,
            other,
            2,
            transfer_outcome_commands(reference, 36, 0)[index],
        );
    }
}

#[test]
fn native_transfer_outcome_admission_then_settlement_order() {
    let (mut state, owner, other, reference) = scene(ContainerKind::Chest);
    transfer_outcome_open(&mut state, owner);
    submit(
        &mut state,
        owner,
        2,
        transfer_outcome_commands(reference, 37, 0)[0],
    );
    submit(
        &mut state,
        owner,
        3,
        Command::OpenContainer(look(std::f32::consts::FRAC_PI_2, 0.0)),
    );
    submit(
        &mut state,
        owner,
        4,
        transfer_outcome_commands(reference, 36, 1)[0],
    );
    let tick = state.advance_tick(TickBudget::full()).unwrap();
    let events = events_for(&tick, owner);
    let expected = [
        Event::CommandRejected(mornlea_domain::CommandRejection::new(
            3,
            mornlea_domain::RejectReason::NoTarget,
        )),
        Event::CommandRejected(mornlea_domain::CommandRejection::new(
            2,
            mornlea_domain::RejectReason::InvalidInput,
        )),
    ];
    assert_eq!(&events[..2], &expected);
    assert_eq!(
        events
            .iter()
            .filter(|e| matches!(e, Event::CommandRejected(_)))
            .count(),
        2
    );
    assert_eq!(record(&state, owner).slots[1], storage(ITEM_DIRT, 5));
    assert!(
        matches!(&state.residents().container_records()[&reference].slots,
        mornlea_server::contracts::ContainerSlots::Chest(cells) if cells[0] == StorageStack::default())
    );
    assert_eq!(
        events
            .iter()
            .filter(|e| matches!(e, Event::InventoryState(_)))
            .count(),
        1
    );
    assert!(!events.iter().any(|e| matches!(e, Event::CraftingState(_))));
    assert!(projection_crafting_record_states(&events_for(&tick, other)).is_empty());
    assert!(
        !events_for(&tick, other)
            .iter()
            .any(|e| matches!(e, Event::CommandRejected(_)))
    );
    assert_eq!(state.session(owner).unwrap().last_applied_sequence, 4);
    assert_eq!(native_input_ack(&tick, owner), 0);
    submit(
        &mut state,
        owner,
        2,
        transfer_outcome_commands(reference, 37, 0)[0],
    );
    let next = state.advance_tick(TickBudget::full()).unwrap();
    assert!(
        !events_for(&next, owner)
            .iter()
            .any(|e| matches!(e, Event::CommandRejected(_)))
    );
}

#[test]
fn native_transfer_outcome_equal_roundtrip_intent() {
    let (mut state, owner, other, reference) = scene(ContainerKind::Chest);
    transfer_outcome_open(&mut state, owner);
    let inventory = record(&state, owner);
    let before = state.residents().container_records()[&reference].clone();
    submit(
        &mut state,
        owner,
        2,
        transfer_outcome_commands(reference, 36, 1)[0],
    );
    submit(
        &mut state,
        owner,
        3,
        transfer_outcome_commands(reference, 1, 36)[0],
    );
    let tick = state.advance_tick(TickBudget::full()).unwrap();
    assert_eq!(record(&state, owner), inventory);
    let after = state.residents().container_records()[&reference].clone();
    assert_eq!(after.slots, before.slots);
    assert_eq!(after.revision, before.revision + 1);
    let events = events_for(&tick, owner);
    assert_eq!(
        events
            .iter()
            .filter(|e| matches!(e, Event::InventoryState(_)))
            .count(),
        1
    );
    assert!(
        !events
            .iter()
            .any(|e| matches!(e, Event::CommandRejected(_) | Event::CraftingState(_)))
    );
    assert!(projection_crafting_record_states(&events_for(&tick, other)).is_empty());
}

#[test]
fn native_transfer_outcome_exhausted_revision_is_hard() {
    let (mut state, owner, _, reference) = scene(ContainerKind::Chest);
    transfer_outcome_open(&mut state, owner);
    // Exhausted durable revision is a prepared invariant cause, never client input.
    stage(&mut state, |context| {
        let mut front = ground_chunk();
        assert_eq!(
            chest_in_chunk(
                &mut front,
                BlockPos::new(0, 66, -1),
                storage_array(&[(0, ITEM_DIRT, 5)])
            ),
            reference
        );
        context.preload_ready_chunk(
            ReadyChunk::try_new(chunk_key(0, -1), 1, u64::MAX, front).unwrap(),
        );
    });
    let inventory = record(&state, owner);
    let containers = state.residents().container_records();
    let drops = state.residents().drop_records();
    submit(
        &mut state,
        owner,
        2,
        transfer_outcome_commands(reference, 36, 0)[0],
    );
    let error = state.advance_tick(TickBudget::full()).unwrap_err();
    assert_eq!(
        error,
        ServerError::Internal {
            invariant: "container transfer staging"
        }
    );
    assert_eq!(record(&state, owner), inventory);
    assert_eq!(state.residents().container_records(), containers);
    assert_eq!(state.residents().drop_records(), drops);
    assert_eq!(state.advance_tick(TickBudget::full()).unwrap_err(), error);
}

fn container_drop_command(reference: ContainerRef, slot: u8) -> Command {
    Command::DropStack(StackSource::try_new(StackView::Container(reference), slot).unwrap())
}

fn container_drop_scene(
    kind: ContainerKind,
) -> (AuthorityState, SessionKey, SessionKey, ContainerRef) {
    let (mut state, owner, other, reference) = scene(kind);
    if kind == ContainerKind::Furnace {
        // Valid fuel is prepared; opening and dropping use the actual command path.
        stage(&mut state, |context| {
            let mut front = ground_chunk();
            assert_eq!(
                furnace_in_chunk(
                    &mut front,
                    BlockPos::new(0, 66, -1),
                    StorageStack::default(),
                    storage(ITEM_COAL, 3),
                    StorageStack::default(),
                    0,
                    0
                ),
                reference
            );
            context
                .preload_ready_chunk(ReadyChunk::try_new(chunk_key(0, -1), 1, 1, front).unwrap());
        });
        state.advance_tick(TickBudget::full()).unwrap();
    }
    (state, owner, other, reference)
}

fn container_drop_quantities(state: &AuthorityState) -> Vec<(DropId, FiniteVec3, StorageStack)> {
    state
        .residents()
        .drop_records()
        .into_iter()
        .map(|drop| (drop.id, drop.position, drop.stack))
        .collect()
}

fn container_drop_refused(
    state: &mut AuthorityState,
    owner: SessionKey,
    other: SessionKey,
    sequence: u64,
    command: Command,
    reason: mornlea_domain::RejectReason,
) -> mornlea_server::contracts::TickPublication {
    let inventory = record(state, owner);
    let containers = state.residents().container_records();
    let quantities = container_drop_quantities(state);
    submit(state, owner, sequence, command);
    let tick = state.advance_tick(TickBudget::full()).unwrap();
    assert_eq!(record(state, owner), inventory);
    assert_eq!(state.residents().container_records(), containers);
    assert_eq!(container_drop_quantities(state), quantities);
    assert!(projection_crafting_record_states(&events_for(&tick, owner)).is_empty());
    assert!(projection_crafting_record_states(&events_for(&tick, other)).is_empty());
    assert_inventory_outcome_refusal(&tick, owner, other, sequence, reason);
    tick
}

#[test]
fn native_container_drop_outcome_no_view() {
    for kind in [ContainerKind::Chest, ContainerKind::Furnace] {
        let (mut state, owner, other, reference) = scene(kind);
        container_drop_refused(
            &mut state,
            owner,
            other,
            1,
            container_drop_command(reference, 36),
            mornlea_domain::RejectReason::InvalidInput,
        );
        assert!(state.settled_read().unwrap().viewer(owner).is_none());
    }
}

#[test]
fn native_container_drop_outcome_empty() {
    for (kind, slot) in [(ContainerKind::Chest, 37), (ContainerKind::Furnace, 36)] {
        let (mut state, owner, other, reference) = scene(kind);
        transfer_outcome_open(&mut state, owner);
        let lease = state.settled_read().unwrap().viewer(owner);
        container_drop_refused(
            &mut state,
            owner,
            other,
            2,
            container_drop_command(reference, slot),
            mornlea_domain::RejectReason::InvalidSlot,
        );
        assert_eq!(state.settled_read().unwrap().viewer(owner), lease);
    }
}

#[test]
fn native_container_drop_outcome_stale() {
    let (mut state, owner, other, reference) = scene(ContainerKind::Chest);
    transfer_outcome_open(&mut state, owner);
    let lease = state.settled_read().unwrap().viewer(owner);
    let stale =
        ContainerRef::try_new(reference.chunk(), reference.kind(), reference.slot(), 2).unwrap();
    container_drop_refused(
        &mut state,
        owner,
        other,
        2,
        container_drop_command(stale, 36),
        mornlea_domain::RejectReason::InvalidInput,
    );
    assert_eq!(state.settled_read().unwrap().viewer(owner), lease);
}

#[test]
fn native_container_drop_outcome_unavailable_feet() {
    let (mut state, owner, other, reference) = scene(ContainerKind::Chest);
    transfer_outcome_open(&mut state, owner);
    // The retained lease is real; the unavailable foot-column pose is prepared.
    stage(&mut state, |context| {
        let mut actor = context
            .read()
            .actor(ActorKey::Player(owner))
            .cloned()
            .unwrap();
        actor.motion = MotionState::new(MotionStateParts {
            position: FiniteVec3::try_new([32.5, 65.0, 0.5]).unwrap(),
            velocity: FiniteVec3::try_new([0.0; 3]).unwrap(),
            on_ground: true,
        });
        context.stage(RuleEffect::Actor(actor)).unwrap();
    });
    // Direct settlement precedes source recovery and separately qualifies the foot gate.
    stage(&mut state, |context| {
        let envelope =
            mornlea_domain::CommandEnvelope::try_new(mornlea_domain::CommandEnvelopeParts {
                tick: 2,
                session: owner.get(),
                sequence: 2,
                arrival_index: 1,
                command: container_drop_command(reference, 36),
            })
            .unwrap();
        let inventory = *context.read().inventory(ActorKey::Player(owner)).unwrap();
        let container = context.read().container(reference).unwrap();
        assert_eq!(
            mornlea_server::rules::containers::settle_command(context, &envelope),
            Err(mornlea_server::contracts::RuleReject::Wire(
                mornlea_domain::RejectReason::ChunkNotReady
            ))
        );
        assert_eq!(
            context.read().inventory(ActorKey::Player(owner)),
            Some(&inventory)
        );
        assert_eq!(context.read().container(reference), Some(container));
    });
    let tick = container_drop_refused(
        &mut state,
        owner,
        other,
        2,
        container_drop_command(reference, 36),
        mornlea_domain::RejectReason::PlayerNotReady,
    );
    assert_eq!(
        state
            .residents()
            .actors
            .iter()
            .find(|actor| actor.key == ActorKey::Player(owner))
            .unwrap()
            .lifecycle,
        ActorLifecycle::Pending
    );
    assert!(state.settled_read().unwrap().viewer(owner).is_none());
    assert!(
        events_for(&tick, owner)
            .iter()
            .any(|event| matches!(event, Event::ContainerClosed(_)))
    );
}

#[test]
fn native_container_drop_outcome_capacity() {
    for (kind, slot) in [(ContainerKind::Chest, 36), (ContainerKind::Furnace, 37)] {
        let (mut state, owner, other, reference) = container_drop_scene(kind);
        transfer_outcome_open(&mut state, owner);
        // All fixed physical slots are prepared full and remain pickup-delayed.
        stage(&mut state, |context| {
            let mut home = ground_chunk();
            home.drops.fill(mornlea_storage::DropSlot {
                generation: 1,
                active: true,
                stack: storage(ITEM_STONE, 64),
                block_index: mornlea_domain::chunk_block_index(BlockPos::new(0, 65, 0)) as u32,
                age_ticks: 0,
                pickup_delay_ticks: 200,
            });
            context.preload_ready_chunk(ReadyChunk::try_new(chunk_key(0, 0), 1, 1, home).unwrap());
        });
        container_drop_refused(
            &mut state,
            owner,
            other,
            2,
            container_drop_command(reference, slot),
            mornlea_domain::RejectReason::DropCapacity,
        );
        assert_eq!(state.residents().drop_records().len(), 32);
        assert!(
            state
                .residents()
                .drop_records()
                .iter()
                .all(|drop| drop.age == 1 && drop.pickup_delay == 199)
        );
    }
}

#[test]
fn native_container_drop_outcome_close_precedes_late_drop() {
    let (mut state, owner, other, reference) = scene(ContainerKind::Chest);
    transfer_outcome_open(&mut state, owner);
    let inventory = record(&state, owner);
    let containers = state.residents().container_records();
    submit(&mut state, owner, 2, container_drop_command(reference, 36));
    submit(&mut state, owner, 3, Command::CloseContainer);
    let tick = state.advance_tick(TickBudget::full()).unwrap();
    assert_eq!(record(&state, owner), inventory);
    assert_eq!(state.residents().container_records(), containers);
    assert!(state.residents().drop_records().is_empty());
    assert!(state.settled_read().unwrap().viewer(owner).is_none());
    assert!(projection_crafting_record_states(&events_for(&tick, owner)).is_empty());
    assert!(projection_crafting_record_states(&events_for(&tick, other)).is_empty());
    assert_inventory_outcome_refusal(
        &tick,
        owner,
        other,
        2,
        mornlea_domain::RejectReason::InvalidInput,
    );
}

#[test]
fn native_container_drop_outcome_exhausted_revision_hard() {
    for foot in [false, true] {
        let (mut state, owner, _, reference) = scene(ContainerKind::Chest);
        transfer_outcome_open(&mut state, owner);
        // Each exhausted revision independently exercises a trusted atomic boundary.
        stage(&mut state, |context| {
            let mut chunk = ground_chunk();
            let key = if foot {
                chunk_key(0, 0)
            } else {
                assert_eq!(
                    chest_in_chunk(
                        &mut chunk,
                        BlockPos::new(0, 66, -1),
                        storage_array(&[(0, ITEM_DIRT, 5)])
                    ),
                    reference
                );
                chunk_key(0, -1)
            };
            context.preload_ready_chunk(ReadyChunk::try_new(key, 1, u64::MAX, chunk).unwrap());
        });
        let inventory = record(&state, owner);
        let containers = state.residents().container_records();
        let drops = state.residents().drop_records();
        submit(&mut state, owner, 2, container_drop_command(reference, 36));
        let error = state.advance_tick(TickBudget::full()).unwrap_err();
        assert_eq!(
            error,
            ServerError::Internal {
                invariant: "container drop staging"
            }
        );
        assert_eq!(record(&state, owner), inventory);
        assert_eq!(state.residents().container_records(), containers);
        assert_eq!(state.residents().drop_records(), drops);
        assert_eq!(state.advance_tick(TickBudget::full()).unwrap_err(), error);
    }
}

#[test]
fn native_container_drop_outcome_success_intent() {
    for (kind, slot, item, count) in [
        (ContainerKind::Chest, 36, ITEM_DIRT, 5),
        (ContainerKind::Furnace, 37, ITEM_COAL, 3),
    ] {
        let (mut state, owner, other, reference) = container_drop_scene(kind);
        transfer_outcome_open(&mut state, owner);
        let inventory = record(&state, owner);
        submit(
            &mut state,
            owner,
            2,
            container_drop_command(reference, slot),
        );
        let tick = state.advance_tick(TickBudget::full()).unwrap();
        assert_eq!(record(&state, owner), inventory);
        let events = events_for(&tick, owner);
        assert_eq!(
            events
                .iter()
                .filter(|event| matches!(event, Event::InventoryState(_)))
                .count(),
            1
        );
        assert!(
            !events
                .iter()
                .any(|event| matches!(event, Event::CraftingState(_) | Event::CommandRejected(_)))
        );
        assert!(projection_crafting_record_states(&events_for(&tick, other)).is_empty());
        let drops = state.residents().drop_records();
        assert_eq!(drops.len(), 1);
        assert_eq!(drops[0].stack, storage(item, count));
        assert_eq!(drops[0].pickup_delay, 40);
        assert!(events.contains(&projection_drop_wire_upsert(
            tick.tick,
            0,
            BlockPos::new(0, 65, 0),
            item,
            count
        )));
        let stored = state.residents().container_records()[&reference].clone();
        match stored.slots {
            mornlea_server::contracts::ContainerSlots::Chest(cells) => {
                assert_eq!(cells[0], StorageStack::default())
            }
            mornlea_server::contracts::ContainerSlots::Furnace { slots, .. } => {
                assert_eq!(slots[1], StorageStack::default())
            }
        }
        assert_eq!(native_input_ack(&tick, owner), 0);
    }
}

fn inline_drop_commands() -> [Command; 4] {
    [
        Command::DropSelectedItem,
        Command::DropStack(StackSource::try_new(StackView::Inventory, 0).unwrap()),
        Command::DropStack(StackSource::try_new(StackView::Crafting, 0).unwrap()),
        Command::DropStack(StackSource::try_new(StackView::Crafting, 9).unwrap()),
    ]
}

fn inline_drop_scene(domain: usize) -> (AuthorityState, SessionKey, SessionKey) {
    let (mut state, owner, other, _) = scene(ContainerKind::Chest);
    if domain == 2 {
        prepared_inventory_outcome(&mut state, owner, |inventory| {
            inventory.crafting[0] = storage(ITEM_DIRT, 3);
        });
    }
    (state, owner, other)
}

#[test]
fn native_inline_drop_outcome_empty() {
    for (domain, command) in [
        (0, Command::DropSelectedItem),
        (
            1,
            Command::DropStack(StackSource::try_new(StackView::Inventory, 1).unwrap()),
        ),
        (
            2,
            Command::DropStack(StackSource::try_new(StackView::Crafting, 0).unwrap()),
        ),
        (
            3,
            Command::DropStack(StackSource::try_new(StackView::Crafting, 10).unwrap()),
        ),
    ] {
        let (mut state, owner, other, _) = scene(ContainerKind::Chest);
        if domain == 0 {
            prepared_inventory_outcome(&mut state, owner, |inventory| {
                inventory.slots[0] = StorageStack::default();
            });
        }
        container_drop_refused(
            &mut state,
            owner,
            other,
            1,
            command,
            mornlea_domain::RejectReason::InvalidSlot,
        );
    }
}

#[test]
fn native_inline_drop_outcome_capacity() {
    for (domain, command) in inline_drop_commands().into_iter().enumerate() {
        let (mut state, owner, other) = inline_drop_scene(domain);
        stage(&mut state, |context| {
            let mut chunk = ground_chunk();
            for slot in &mut chunk.drops {
                *slot = mornlea_storage::DropSlot {
                    generation: 1,
                    active: true,
                    block_index: mornlea_domain::chunk_block_index(BlockPos::new(0, 65, 0)) as u32,
                    stack: storage(ITEM_STONE, 64),
                    age_ticks: 0,
                    pickup_delay_ticks: 200,
                };
            }
            context.preload_ready_chunk(ReadyChunk::try_new(chunk_key(0, 0), 1, 1, chunk).unwrap());
        });
        container_drop_refused(
            &mut state,
            owner,
            other,
            1,
            command,
            mornlea_domain::RejectReason::DropCapacity,
        );
        for drop in state.residents().drop_records() {
            assert_eq!((drop.age, drop.pickup_delay), (1, 199));
        }
    }
}

#[test]
fn native_inline_drop_outcome_personal_extent_admission() {
    let (mut state, owner, other, _) = scene(ContainerKind::Chest);
    let before = record(&state, owner);
    submit(
        &mut state,
        owner,
        1,
        Command::DropStack(StackSource::try_new(StackView::Crafting, 5).unwrap()),
    );
    submit(
        &mut state,
        owner,
        2,
        Command::OpenContainer(LookAngles::try_new(0.0, -1.0).unwrap()),
    );
    let tick = state.advance_tick(TickBudget::full()).unwrap();
    let refusals: Vec<_> = events_for(&tick, owner)
        .into_iter()
        .filter(|e| matches!(e, Event::CommandRejected(_)))
        .collect();
    assert_eq!(
        refusals,
        vec![
            Event::CommandRejected(mornlea_domain::CommandRejection::new(
                1,
                mornlea_domain::RejectReason::InvalidSlot
            )),
            Event::CommandRejected(mornlea_domain::CommandRejection::new(
                2,
                mornlea_domain::RejectReason::NoTarget
            )),
        ]
    );
    assert_eq!(record(&state, owner), before);
    assert!(projection_crafting_record_states(&events_for(&tick, owner)).is_empty());
    assert!(projection_crafting_record_states(&events_for(&tick, other)).is_empty());
    assert_eq!(native_input_ack(&tick, owner), 0);
}

#[test]
fn native_inline_drop_outcome_unavailable_feet() {
    for (domain, command) in inline_drop_commands().into_iter().enumerate() {
        let (mut state, owner, other) = inline_drop_scene(domain);
        // Direct preparation and the full source recovery have separate acceptance boundaries.
        stage(&mut state, |context| {
            let mut actor = context
                .read()
                .actor(ActorKey::Player(owner))
                .cloned()
                .unwrap();
            actor.motion = MotionState::new(MotionStateParts {
                position: FiniteVec3::try_new([32.5, 65.0, 0.5]).unwrap(),
                velocity: FiniteVec3::try_new([0.0; 3]).unwrap(),
                on_ground: true,
            });
            context.stage(RuleEffect::Actor(actor)).unwrap();
            let envelope =
                mornlea_domain::CommandEnvelope::try_new(mornlea_domain::CommandEnvelopeParts {
                    tick: 2,
                    session: owner.get(),
                    sequence: 1,
                    arrival_index: 1,
                    command,
                })
                .unwrap();
            let inventory = *context.read().inventory(ActorKey::Player(owner)).unwrap();
            assert_eq!(
                mornlea_server::rules::drops::settle_command(context, &envelope),
                Err(mornlea_server::contracts::RuleReject::Wire(
                    mornlea_domain::RejectReason::ChunkNotReady
                ))
            );
            assert_eq!(
                context.read().inventory(ActorKey::Player(owner)),
                Some(&inventory)
            );
        });
        container_drop_refused(
            &mut state,
            owner,
            other,
            1,
            command,
            if domain == 0 {
                mornlea_domain::RejectReason::PlayerNotReady
            } else {
                mornlea_domain::RejectReason::ChunkNotReady
            },
        );
        assert_eq!(
            state
                .settled_read()
                .unwrap()
                .actor(ActorKey::Player(owner))
                .unwrap()
                .lifecycle,
            ActorLifecycle::Pending
        );
    }
    for domain in 1..=3 {
        let (mut state, owner, other) = inline_drop_scene(domain);
        // Empty source precedence survives a real reset before panel settlement.
        stage(&mut state, |context| {
            let key = ActorKey::Player(owner);
            let mut actor = context.read().actor(key).cloned().unwrap();
            actor.motion = MotionState::new(MotionStateParts {
                position: FiniteVec3::try_new([32.5, 65.0, 0.5]).unwrap(),
                velocity: FiniteVec3::try_new([0.0; 3]).unwrap(),
                on_ground: true,
            });
            context.stage(RuleEffect::Actor(actor)).unwrap();
            let mut inventory = *context.read().inventory(key).unwrap();
            if domain == 2 {
                inventory.crafting[0] = StorageStack::default();
            } else {
                inventory.slots[0] = StorageStack::default();
            }
            context.preload_inventory(key, inventory);
        });
        container_drop_refused(
            &mut state,
            owner,
            other,
            1,
            inline_drop_commands()[domain],
            mornlea_domain::RejectReason::InvalidSlot,
        );
    }
}

#[test]
fn native_inline_drop_outcome_refusal_then_success() {
    let (mut state, owner, other, _) = scene(ContainerKind::Chest);
    submit(
        &mut state,
        owner,
        1,
        Command::DropStack(StackSource::try_new(StackView::Inventory, 1).unwrap()),
    );
    submit(&mut state, owner, 2, Command::DropSelectedItem);
    let tick = state.advance_tick(TickBudget::full()).unwrap();
    assert_inventory_outcome_refusal(
        &tick,
        owner,
        other,
        1,
        mornlea_domain::RejectReason::InvalidSlot,
    );
    assert_eq!(record(&state, owner).slots[0], storage(36, 1));
    let drops = state.residents().drop_records();
    assert_eq!(drops.len(), 1);
    assert_eq!(
        (drops[0].stack, drops[0].pickup_delay),
        (storage(36, 1), 39)
    );
    assert_eq!(
        events_for(&tick, owner)
            .iter()
            .filter(|e| matches!(e, Event::InventoryState(_)))
            .count(),
        1
    );
    assert!(
        !events_for(&tick, owner)
            .iter()
            .any(|e| matches!(e, Event::CraftingState(_)))
    );
    assert!(projection_crafting_record_states(&events_for(&tick, other)).is_empty());
}

#[test]
fn native_inline_drop_outcome_admission_before_settlement() {
    let (mut state, owner, other, _) = scene(ContainerKind::Chest);
    prepared_inventory_outcome(&mut state, owner, |inventory| {
        inventory.slots[0] = StorageStack::default();
    });
    let before = record(&state, owner);
    submit(&mut state, owner, 1, Command::DropSelectedItem);
    submit(
        &mut state,
        owner,
        2,
        Command::DropStack(StackSource::try_new(StackView::Crafting, 5).unwrap()),
    );
    let tick = state.advance_tick(TickBudget::full()).unwrap();
    let refusals: Vec<_> = events_for(&tick, owner)
        .into_iter()
        .filter(|e| matches!(e, Event::CommandRejected(_)))
        .collect();
    assert_eq!(
        refusals,
        vec![
            Event::CommandRejected(mornlea_domain::CommandRejection::new(
                2,
                mornlea_domain::RejectReason::InvalidSlot
            )),
            Event::CommandRejected(mornlea_domain::CommandRejection::new(
                1,
                mornlea_domain::RejectReason::InvalidSlot
            )),
        ]
    );
    assert_eq!(record(&state, owner), before);
    assert!(container_drop_quantities(&state).is_empty());
    assert!(projection_crafting_record_states(&events_for(&tick, owner)).is_empty());
    assert!(projection_crafting_record_states(&events_for(&tick, other)).is_empty());
    assert_eq!(native_input_ack(&tick, owner), 0);
}

#[test]
fn native_inline_drop_outcome_exhausted_revision_hard() {
    for (domain, command) in inline_drop_commands().into_iter().enumerate() {
        let (mut state, owner, _) = inline_drop_scene(domain);
        // Revision exhaustion is a prepared trusted preflight cause, not a client command.
        stage(&mut state, |context| {
            context.preload_ready_chunk(
                ReadyChunk::try_new(chunk_key(0, 0), 1, u64::MAX, ground_chunk()).unwrap(),
            );
        });
        let before = record(&state, owner);
        let drops = state.residents().drop_records();
        submit(&mut state, owner, 1, command);
        let error = state.advance_tick(TickBudget::full()).unwrap_err();
        assert_eq!(
            error,
            ServerError::Internal {
                invariant: "inline drop staging"
            }
        );
        assert_eq!(record(&state, owner), before);
        assert_eq!(state.residents().drop_records(), drops);
        assert_eq!(state.advance_tick(TickBudget::full()).unwrap_err(), error);
    }
}

#[test]
fn native_inline_drop_outcome_success_intent() {
    for (domain, command) in inline_drop_commands().into_iter().enumerate() {
        let (mut state, owner, other) = inline_drop_scene(domain);
        submit(&mut state, owner, 1, command);
        let tick = state.advance_tick(TickBudget::full()).unwrap();
        let events = events_for(&tick, owner);
        assert!(
            !events
                .iter()
                .any(|e| matches!(e, Event::CommandRejected(_)))
        );
        assert_eq!(
            events
                .iter()
                .filter(|e| matches!(e, Event::InventoryState(_)))
                .count(),
            1
        );
        assert_eq!(
            events
                .iter()
                .filter(|e| matches!(e, Event::CraftingState(_)))
                .count(),
            usize::from(domain == 2)
        );
        let want = if domain == 2 {
            storage(ITEM_DIRT, 3)
        } else {
            storage(36, if domain == 0 { 1 } else { 2 })
        };
        let drops = state.residents().drop_records();
        assert_eq!(drops.len(), 1);
        assert_eq!(
            (drops[0].stack, drops[0].pickup_delay, drops[0].age),
            (want, 39, 1)
        );
        if domain == 2 {
            assert_eq!(record(&state, owner).crafting[0], StorageStack::default());
        } else {
            assert_eq!(
                record(&state, owner).slots[0],
                if domain == 0 {
                    storage(36, 1)
                } else {
                    StorageStack::default()
                }
            );
        }
        assert!(events.contains(&projection_drop_wire_upsert(
            tick.tick,
            0,
            BlockPos::new(0, 65, 0),
            want.item,
            want.count
        )));
        assert!(projection_crafting_record_states(&events_for(&tick, other)).is_empty());
        assert_eq!(native_input_ack(&tick, owner), 0);
    }
}
