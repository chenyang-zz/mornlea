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
