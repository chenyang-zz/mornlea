//! Final physical container lifecycle through actual native commands.
use super::*;

fn container_events(events: &[Event]) -> Vec<Event> {
    events
        .iter()
        .filter(|e| {
            matches!(
                e,
                Event::ChestState(_) | Event::FurnaceState(_) | Event::ContainerClosed(_)
            )
        })
        .cloned()
        .collect()
}

fn opened(kind: ContainerKind) -> (AuthorityState, SessionKey, SessionKey, ContainerRef, Event) {
    let (mut state, owner, other, reference, _, _) = projection_container_dirty_fixture(kind);
    submit(&mut state, owner, 1, Command::OpenContainer(look(0.0, 0.0)));
    let tick = state.advance_tick(TickBudget::full()).unwrap();
    let events = container_events(&events_for(&tick, owner));
    assert_eq!(events.len(), 1);
    assert!(container_events(&events_for(&tick, other)).is_empty());
    assert_eq!(
        state
            .settled_read()
            .unwrap()
            .viewer(owner)
            .unwrap()
            .reference(),
        reference
    );
    (state, owner, other, reference, events[0].clone())
}

fn prepare_pose(
    state: &mut AuthorityState,
    owner: SessionKey,
    position: [f32; 3],
    lifecycle: ActorLifecycle,
    dimension: Dimension,
) {
    stage(state, |context| {
        let mut actor = context
            .read()
            .actor(ActorKey::Player(owner))
            .unwrap()
            .clone();
        actor.lifecycle = lifecycle;
        actor.dimension = dimension;
        actor.motion = MotionState::new(MotionStateParts {
            position: FiniteVec3::try_new(position).unwrap(),
            velocity: FiniteVec3::try_new([0.0; 3]).unwrap(),
            on_ground: true,
        });
        context.stage(RuleEffect::Actor(actor)).unwrap();
    });
}

fn assert_invalidated(
    state: &mut AuthorityState,
    owner: SessionKey,
    other: SessionKey,
    reference: ContainerRef,
) {
    let tick = state.advance_tick(TickBudget::full()).unwrap();
    let events = events_for(&tick, owner);
    assert_eq!(
        container_events(&events),
        vec![Event::ContainerClosed(
            mornlea_domain::ContainerClosed::new(reference)
        )]
    );
    assert!(events.iter().any(|e| matches!(e, Event::PlayerState(_))));
    assert!(state.settled_read().unwrap().viewer(owner).is_none());
    assert!(container_events(&events_for(&tick, other)).is_empty());
    let quiet = state.advance_tick(TickBudget::full()).unwrap();
    assert!(container_events(&events_for(&quiet, owner)).is_empty());
}

#[test]
fn unchanged_chest_publishes_full_state_every_tick_to_owner() {
    let (mut state, owner, other, _, expected) = opened(ContainerKind::Chest);
    for _ in 0..2 {
        let tick = state.advance_tick(TickBudget::full()).unwrap();
        assert_eq!(
            container_events(&events_for(&tick, owner)),
            vec![expected.clone()]
        );
        assert!(container_events(&events_for(&tick, other)).is_empty());
    }
}

#[test]
fn unchanged_furnace_publishes_full_state_every_tick_to_owner() {
    let (mut state, owner, other, _, expected) = opened(ContainerKind::Furnace);
    for _ in 0..2 {
        let tick = state.advance_tick(TickBudget::full()).unwrap();
        assert_eq!(
            container_events(&events_for(&tick, owner)),
            vec![expected.clone()]
        );
        assert!(container_events(&events_for(&tick, other)).is_empty());
    }
}

#[test]
fn explicit_close_has_no_invalidation_notice() {
    let (mut state, owner, other, _, _) = opened(ContainerKind::Chest);
    submit(&mut state, owner, 2, Command::CloseContainer);
    let tick = state.advance_tick(TickBudget::full()).unwrap();
    assert!(container_events(&events_for(&tick, owner)).is_empty());
    assert!(container_events(&events_for(&tick, other)).is_empty());
    assert!(state.settled_read().unwrap().viewer(owner).is_none());
    let quiet = state.advance_tick(TickBudget::full()).unwrap();
    assert!(container_events(&events_for(&quiet, owner)).is_empty());
}

#[test]
fn workbench_replacement_has_no_invalidation_notice() {
    let (mut state, owner, _, _, _) = opened(ContainerKind::Chest);
    let mut home = ground_chunk();
    set_cell(&mut home, BlockPos::new(0, 66, 1), 45);
    stage(&mut state, |context| {
        context.preload_ready_chunk(ReadyChunk::try_new(chunk_key(0, 0), 1, 1, home).unwrap())
    });
    submit(
        &mut state,
        owner,
        2,
        Command::OpenContainer(look(std::f32::consts::PI, 0.0)),
    );
    let tick = state.advance_tick(TickBudget::full()).unwrap();
    assert_eq!(
        state.residents().inventories[&ActorKey::Player(owner)].crafting_size,
        CraftingSize::Workbench
    );
    assert_eq!(
        projection_crafting_record_states(&events_for(&tick, owner)).len(),
        1
    );
    assert!(container_events(&events_for(&tick, owner)).is_empty());
    assert!(state.settled_read().unwrap().viewer(owner).is_none());
}

#[test]
fn final_out_of_reach_pose_closes_once() {
    let (mut state, owner, other, reference, _) = opened(ContainerKind::Chest);
    prepare_pose(
        &mut state,
        owner,
        [7.5, 65.0, 0.5],
        ActorLifecycle::Active,
        Dimension::OVERWORLD,
    );
    assert_invalidated(&mut state, owner, other, reference);
}

#[test]
fn final_pending_actor_closes_once_and_keeps_private_state() {
    let (mut state, owner, other, reference, _) = opened(ContainerKind::Chest);
    prepare_pose(
        &mut state,
        owner,
        [0.5, 65.0, 0.5],
        ActorLifecycle::Pending,
        Dimension::OVERWORLD,
    );
    assert_invalidated(&mut state, owner, other, reference);
}

#[test]
fn final_dimension_mismatch_closes_once() {
    let (mut state, owner, other, reference, _) = opened(ContainerKind::Chest);
    prepare_pose(
        &mut state,
        owner,
        [0.5, 65.0, 0.5],
        ActorLifecycle::Active,
        Dimension::DEPTHS,
    );
    assert_invalidated(&mut state, owner, other, reference);
}

#[test]
fn physical_missing_and_replaced_generation_close_exact_old_reference() {
    for replacement in [false, true] {
        let (mut state, owner, other, reference, _) = opened(ContainerKind::Chest);
        let mut front = ground_chunk();
        if replacement {
            chest_in_chunk(
                &mut front,
                BlockPos::new(0, 66, -1),
                storage_array(&[(0, ITEM_DIRT, 5)]),
            );
            front.chests[0].generation = 2;
        }
        stage(&mut state, |context| {
            context.preload_ready_chunk(ReadyChunk::try_new(chunk_key(0, -1), 1, 2, front).unwrap())
        });
        assert_invalidated(&mut state, owner, other, reference);
    }
}

#[test]
fn native_move_settles_before_final_reach_invalidation() {
    let (mut state, owner, other, reference, _) = opened(ContainerKind::Chest);
    prepare_pose(
        &mut state,
        owner,
        [7.5, 65.0, 0.5],
        ActorLifecycle::Active,
        Dimension::OVERWORLD,
    );
    submit(
        &mut state,
        owner,
        2,
        Command::MoveContainer(
            mornlea_domain::ContainerMove::try_new(
                reference.chunk(),
                ContainerKind::Chest,
                reference.slot(),
                reference.generation(),
                36,
                0,
            )
            .unwrap(),
        ),
    );
    let tick = state.advance_tick(TickBudget::full()).unwrap();
    assert_eq!(
        state.residents().inventories[&ActorKey::Player(owner)].slots[0],
        storage(ITEM_DIRT, 5)
    );
    let events = events_for(&tick, owner);
    assert_eq!(
        container_events(&events),
        vec![Event::ContainerClosed(
            mornlea_domain::ContainerClosed::new(reference)
        )]
    );
    let inventory = events
        .iter()
        .position(|e| matches!(e, Event::InventoryState(_)))
        .unwrap();
    let closed = events
        .iter()
        .position(|e| matches!(e, Event::ContainerClosed(_)))
        .unwrap();
    let player = events
        .iter()
        .position(|e| matches!(e, Event::PlayerState(_)))
        .unwrap();
    assert!(inventory < closed && closed < player);
    assert!(container_events(&events_for(&tick, other)).is_empty());
}

#[test]
fn native_crafting_move_precedes_complete_chest_state() {
    let (mut state, owner, _, _, expected) = opened(ContainerKind::Chest);
    stage(&mut state, |context| {
        let actor = ActorKey::Player(owner);
        let mut record = *context.read().inventory(actor).unwrap();
        record.slots[0] = storage(ITEM_DIRT, 1);
        context.preload_inventory(actor, record);
    });
    submit(
        &mut state,
        owner,
        2,
        Command::MoveCrafting(mornlea_domain::CraftingMove::try_new(9, 0).unwrap()),
    );
    let tick = state.advance_tick(TickBudget::full()).unwrap();
    let events = events_for(&tick, owner);
    assert_eq!(
        state.residents().inventories[&ActorKey::Player(owner)].crafting[0],
        storage(ITEM_DIRT, 1)
    );
    assert_eq!(container_events(&events), vec![expected]);
    let family: Vec<_> = events
        .iter()
        .filter_map(|e| match e {
            Event::InventoryState(_) => Some(0),
            Event::CraftingState(_) => Some(1),
            Event::ChestState(_) => Some(2),
            Event::PlayerState(_) => Some(3),
            _ => None,
        })
        .collect();
    assert_eq!(family, vec![0, 1, 2, 3]);
}

#[test]
fn furnace_replacement_keeps_one_current_view_without_old_close() {
    let (mut state, owner, _, _, _) = opened(ContainerKind::Chest);
    let mut home = ground_chunk();
    let reference = furnace_in_chunk(
        &mut home,
        BlockPos::new(0, 66, 1),
        StorageStack::default(),
        StorageStack::default(),
        StorageStack::default(),
        0,
        0,
    );
    stage(&mut state, |context| {
        context.preload_ready_chunk(ReadyChunk::try_new(chunk_key(0, 0), 1, 1, home).unwrap())
    });
    submit(
        &mut state,
        owner,
        2,
        Command::OpenContainer(look(std::f32::consts::PI, 0.0)),
    );
    let tick = state.advance_tick(TickBudget::full()).unwrap();
    let events = container_events(&events_for(&tick, owner));
    assert_eq!(events.len(), 1);
    assert!(matches!(&events[0],Event::FurnaceState(value) if value.container()==reference));
    assert_eq!(
        state
            .settled_read()
            .unwrap()
            .viewer(owner)
            .unwrap()
            .reference(),
        reference
    );
}

#[test]
fn invalid_prepared_view_closes_without_any_prior_container_mirror() {
    let (mut state, owner, other, reference, _, _) =
        projection_container_dirty_fixture(ContainerKind::Chest);
    state.commit_viewers(std::collections::BTreeMap::from([(
        owner,
        mornlea_server::contracts::ViewLease::new(owner, reference),
    )]));
    let front = ground_chunk();
    stage(&mut state, |context| {
        context.preload_ready_chunk(ReadyChunk::try_new(chunk_key(0, -1), 1, 2, front).unwrap())
    });
    assert_invalidated(&mut state, owner, other, reference);
}
