//! Native workbench sneaking refusals preserve owner state and intent.
use super::*;

fn held_sneaking(sneaking: bool) -> Command {
    Command::PlayerInput(PlayerControl::new(PlayerControlParts {
        movement: Movement {
            move_x: 0,
            move_z: 0,
            jump: false,
        },
        look: look(std::f32::consts::PI, 0.0),
        actions: HeldActions {
            primary: false,
            eating: false,
            sprinting: false,
            sneaking,
        },
    }))
}

fn bench_open() -> Command {
    Command::OpenContainer(look(std::f32::consts::PI, 0.0))
}

fn anchor(state: &AuthorityState, owner: SessionKey) -> Option<BlockPos> {
    match &state
        .settled_read()
        .unwrap()
        .runtime(ActorKey::Player(owner))
        .unwrap()
        .aux
    {
        ActorAux::Player { workbench, .. } => *workbench,
        _ => panic!("expected player auxiliary"),
    }
}

fn no_grid_or_inventory(
    tick: &mornlea_server::contracts::TickPublication,
    owner: SessionKey,
    other: SessionKey,
) {
    for session in [owner, other] {
        assert!(projection_inventory_states(&events_for(tick, session)).is_empty());
        assert!(projection_crafting_record_states(&events_for(tick, session)).is_empty());
    }
}

#[test]
fn native_sneaking_bench_open_preserves_personal_grid_and_release_allows_open() {
    let (mut state, owner, other, _) = command_lifecycle::scene(ContainerKind::Chest);
    let actor = ActorKey::Player(owner);
    let before = *state.settled_read().unwrap().inventory(actor).unwrap();
    submit(&mut state, owner, 1, held_sneaking(true));
    submit(&mut state, owner, 2, bench_open());
    let tick = state.advance_tick(TickBudget::full()).unwrap();
    assert_eq!(
        state.settled_read().unwrap().inventory(actor),
        Some(&before)
    );
    assert_eq!(anchor(&state, owner), None);
    assert!(state.settled_read().unwrap().viewer(owner).is_none());
    no_grid_or_inventory(&tick, owner, other);
    submit(&mut state, owner, 3, held_sneaking(false));
    submit(&mut state, owner, 4, bench_open());
    let tick = state.advance_tick(TickBudget::full()).unwrap();
    assert_eq!(
        state
            .settled_read()
            .unwrap()
            .inventory(actor)
            .unwrap()
            .crafting_size,
        CraftingSize::Workbench
    );
    assert_eq!(anchor(&state, owner), Some(BlockPos::new(0, 66, 1)));
    assert_eq!(
        projection_crafting_record_states(&events_for(&tick, owner)).len(),
        1
    );
}

#[test]
fn native_sneaking_bench_open_preserves_existing_chest_lease() {
    let (mut state, owner, other, reference) = command_lifecycle::scene(ContainerKind::Chest);
    submit(&mut state, owner, 1, Command::OpenContainer(look(0.0, 0.0)));
    state.advance_tick(TickBudget::full()).unwrap();
    let actor = ActorKey::Player(owner);
    let before = *state.settled_read().unwrap().inventory(actor).unwrap();
    submit(&mut state, owner, 2, held_sneaking(true));
    submit(&mut state, owner, 3, bench_open());
    let tick = state.advance_tick(TickBudget::full()).unwrap();
    assert_eq!(
        state.settled_read().unwrap().inventory(actor),
        Some(&before)
    );
    assert_eq!(anchor(&state, owner), None);
    assert_eq!(
        state
            .settled_read()
            .unwrap()
            .viewer(owner)
            .unwrap()
            .reference(),
        reference
    );
    no_grid_or_inventory(&tick, owner, other);
    assert!(
        events_for(&tick, owner)
            .iter()
            .any(|event| matches!(event, Event::ChestState(_)))
    );
}

#[test]
fn native_sneaking_bench_reopen_preserves_anchor_without_grid_intent() {
    let (mut state, owner, other, _) = command_lifecycle::scene(ContainerKind::Chest);
    submit(&mut state, owner, 1, bench_open());
    state.advance_tick(TickBudget::full()).unwrap();
    let actor = ActorKey::Player(owner);
    let before = *state.settled_read().unwrap().inventory(actor).unwrap();
    let before_anchor = anchor(&state, owner);
    submit(&mut state, owner, 2, held_sneaking(true));
    submit(&mut state, owner, 3, bench_open());
    let tick = state.advance_tick(TickBudget::full()).unwrap();
    assert_eq!(
        state.settled_read().unwrap().inventory(actor),
        Some(&before)
    );
    assert_eq!(anchor(&state, owner), before_anchor);
    assert!(state.settled_read().unwrap().viewer(owner).is_none());
    no_grid_or_inventory(&tick, owner, other);
}
