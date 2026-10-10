//! Source workbench close intent and real reducer hard-failure fencing.
use super::*;
use mornlea_server::contracts::{SaveBudget, SaveMode, SaveUrgency, ServerPhase};
use mornlea_server::core::step::AuthoritativeFinalReducer;

fn unchanged_owner_inventory_event() -> Event {
    Event::InventoryState(InventoryState::new(InventoryStateParts {
        hotbar: item_array(&[(0, 36, 2)]),
        backpack: [ItemStack::EMPTY; 27],
        selected: HotbarSlot::new(0).unwrap(),
    }))
}

fn empty_personal_grid_event() -> Event {
    Event::CraftingState(
        CraftingState::try_new(CraftingStateParts {
            size: CraftingSize::Personal,
            slots: [ItemStack::EMPTY; 9],
            output: ItemStack::EMPTY,
        })
        .unwrap(),
    )
}

fn open_bench(state: &mut AuthorityState, owner: SessionKey) {
    submit(
        state,
        owner,
        1,
        Command::OpenContainer(look(std::f32::consts::PI, 0.0)),
    );
    state.advance_tick(TickBudget::full()).unwrap();
}

#[test]
fn native_empty_bench_close_restates_owner_inventory_and_personal_close_is_quiet() {
    let (mut state, owner, other, _) = command_lifecycle::scene(ContainerKind::Chest);
    open_bench(&mut state, owner);
    let actor = ActorKey::Player(owner);
    let before = unchanged_owner_inventory_event();
    submit(&mut state, owner, 2, Command::CloseContainer);
    let tick = state.advance_tick(TickBudget::full()).unwrap();
    assert_eq!(
        projection_inventory_states(&events_for(&tick, owner)),
        vec![before]
    );
    assert_eq!(
        projection_crafting_record_states(&events_for(&tick, owner)),
        vec![
            unchanged_owner_inventory_event(),
            empty_personal_grid_event()
        ]
    );
    assert_eq!(
        state.residents().inventories[&actor].crafting_size,
        CraftingSize::Personal
    );
    assert!(projection_inventory_states(&events_for(&tick, other)).is_empty());
    submit(&mut state, owner, 3, Command::CloseContainer);
    let tick = state.advance_tick(TickBudget::full()).unwrap();
    assert!(projection_inventory_states(&events_for(&tick, owner)).is_empty());
    assert!(projection_crafting_record_states(&events_for(&tick, owner)).is_empty());
}

#[test]
fn native_mined_empty_bench_close_retains_owner_inventory_intent() {
    let (mut state, owner, other, _) = command_lifecycle::scene(ContainerKind::Chest);
    open_bench(&mut state, owner);
    let actor = ActorKey::Player(owner);
    let before = unchanged_owner_inventory_event();
    submit(
        &mut state,
        owner,
        2,
        Command::PlayerInput(PlayerControl::new(PlayerControlParts {
            movement: Movement {
                move_x: 0,
                move_z: 0,
                jump: false,
            },
            look: look(std::f32::consts::PI, 0.0),
            actions: HeldActions {
                primary: true,
                eating: false,
                sprinting: false,
                sneaking: false,
            },
        })),
    );
    for elapsed in 1..=14 {
        state.advance_tick(TickBudget::full()).unwrap();
        assert_eq!(
            state.settled_read().unwrap().mining(actor).unwrap().elapsed,
            elapsed
        );
    }
    let tick = state.advance_tick(TickBudget::full()).unwrap();
    let read = state.settled_read().unwrap();
    assert_eq!(
        read.observation(Dimension::OVERWORLD, BlockPos::new(0, 66, 1))
            .unwrap()
            .block,
        0
    );
    assert_eq!(
        read.inventory(actor).unwrap().crafting_size,
        CraftingSize::Personal
    );
    assert_eq!(
        projection_inventory_states(&events_for(&tick, owner)),
        vec![before]
    );
    assert_eq!(
        projection_crafting_record_states(&events_for(&tick, owner)),
        vec![
            unchanged_owner_inventory_event(),
            empty_personal_grid_event()
        ]
    );
    assert!(projection_inventory_states(&events_for(&tick, other)).is_empty());
}

fn invariant_fault_scene() -> (AuthorityState, SessionKey, SessionKey) {
    let (mut state, owner, other, _) = command_lifecycle::scene(ContainerKind::Chest);
    open_bench(&mut state, owner);
    // Trusted preparation corrupts the repack invariant and invalidates the anchor.
    // Actual admission cannot construct this state; the reducer must fence it.
    stage(&mut state, |context| {
        let actor = ActorKey::Player(owner);
        let mut inventory = *context.read().inventory(actor).unwrap();
        inventory.slots.fill(storage(ITEM_STONE, 64));
        inventory.crafting[4] = storage(ITEM_DIRT, 64);
        context.preload_inventory(actor, inventory);
        context.preload_ready_chunk(
            ReadyChunk::try_new(chunk_key(0, 0), 1, 1, ground_chunk()).unwrap(),
        );
    });
    for session in [owner, other] {
        state.take_outbox(session, 1024, 1_048_576).unwrap();
        assert!(
            state
                .take_outbox(session, 1024, 1_048_576)
                .unwrap()
                .is_empty()
        );
    }
    (state, owner, other)
}

fn assert_fenced(state: &mut AuthorityState, owner: SessionKey, other: SessionKey, tick: u64) {
    let error = ServerError::Internal {
        invariant: "automatic workbench repack",
    };
    assert_eq!(state.phase(), ServerPhase::Closing);
    assert_eq!(state.next_tick(), tick);
    assert_eq!(state.settled_read().err(), Some(error));
    assert!(
        state
            .capture_chunk_snapshot(chunk_key(0, 0), SaveUrgency::Shutdown)
            .is_none()
    );
    assert!(
        state
            .select(
                SaveMode::All,
                SaveBudget {
                    chunks: 8,
                    estimated_bytes: 1_048_576
                }
            )
            .is_empty()
    );
    for session in [owner, other] {
        assert!(
            state
                .take_outbox(session, 1024, 1_048_576)
                .unwrap()
                .is_empty()
        );
    }
    for _ in 0..2 {
        assert_eq!(state.advance_tick(TickBudget::full()), Err(error));
        assert_eq!(state.run_final(&mut AuthoritativeFinalReducer), Err(error));
        assert_eq!(state.next_tick(), tick);
    }
}

#[test]
fn actual_automatic_close_failure_fences_normal_tick_without_frames_or_capture() {
    let (mut state, owner, other) = invariant_fault_scene();
    let tick = state.next_tick();
    assert_eq!(
        state.advance_tick(TickBudget::full()),
        Err(ServerError::Internal {
            invariant: "automatic workbench repack",
        })
    );
    assert_fenced(&mut state, owner, other, tick);
}

#[test]
fn actual_automatic_close_failure_fences_unpublished_final_and_replay() {
    let (mut state, owner, other) = invariant_fault_scene();
    let tick = state.next_tick();
    assert!(state.begin_close());
    assert_eq!(
        state.run_final(&mut AuthoritativeFinalReducer),
        Err(ServerError::Internal {
            invariant: "automatic workbench repack",
        })
    );
    assert_fenced(&mut state, owner, other, tick);
}
