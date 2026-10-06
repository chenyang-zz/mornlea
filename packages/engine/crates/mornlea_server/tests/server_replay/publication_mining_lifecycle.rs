//! Source mining suspension through actual held input and lifecycle commands.
use super::*;

fn held_mining() -> Command {
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
    }))
}

fn native_open_interrupts_mining(kind: ContainerKind) {
    let (mut state, owner, other, reference) = command_lifecycle::scene(kind);
    let actor = ActorKey::Player(owner);
    let target = BlockPos::new(0, 66, 1);
    submit(&mut state, owner, 1, held_mining());
    for elapsed in 1..=14 {
        state.advance_tick(TickBudget::full()).unwrap();
        let progress = state.settled_read().unwrap().mining(actor).unwrap().clone();
        assert_eq!(
            (progress.elapsed, progress.required, progress.target),
            (elapsed, 15, target)
        );
    }
    let inventory = *state.settled_read().unwrap().inventory(actor).unwrap();
    let drops = state
        .settled_read()
        .unwrap()
        .drops(chunk_key(0, 0))
        .to_vec();
    submit(&mut state, owner, 2, Command::OpenContainer(look(0.0, 0.0)));
    for _ in 0..4 {
        let tick = state.advance_tick(TickBudget::full()).unwrap();
        let read = state.settled_read().unwrap();
        assert_eq!(
            read.observation(Dimension::OVERWORLD, target)
                .unwrap()
                .block,
            45
        );
        assert_eq!(read.mining(actor), None);
        assert_eq!(read.inventory(actor), Some(&inventory));
        assert_eq!(read.drops(chunk_key(0, 0)), drops.as_slice());
        assert_eq!(read.viewer(owner).unwrap().reference(), reference);
        assert!(
            read.runtime(actor)
                .unwrap()
                .controls
                .unwrap()
                .actions()
                .primary
        );
        assert!(projection_inventory_states(&events_for(&tick, owner)).is_empty());
        assert!(projection_inventory_states(&events_for(&tick, other)).is_empty());
        assert!(
            !events_for(&tick, owner)
                .iter()
                .any(|event| matches!(event, Event::CommandRejected(_)))
        );
    }
    submit(&mut state, owner, 3, Command::CloseContainer);
    let tick = state.advance_tick(TickBudget::full()).unwrap();
    let read = state.settled_read().unwrap();
    assert!(read.viewer(owner).is_none());
    assert_eq!(read.mining(actor).unwrap().elapsed, 1);
    assert_eq!(
        read.observation(Dimension::OVERWORLD, target)
            .unwrap()
            .block,
        45
    );
    assert_eq!(read.inventory(actor), Some(&inventory));
    assert_eq!(read.drops(chunk_key(0, 0)), drops.as_slice());
    assert!(projection_inventory_states(&events_for(&tick, owner)).is_empty());
}

#[test]
fn native_chest_open_prevents_actual_mining_completion_and_close_restarts() {
    native_open_interrupts_mining(ContainerKind::Chest);
}

#[test]
fn native_furnace_open_prevents_actual_mining_completion_and_close_restarts() {
    native_open_interrupts_mining(ContainerKind::Furnace);
}
