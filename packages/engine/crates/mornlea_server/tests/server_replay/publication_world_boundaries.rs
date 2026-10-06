//! Physical chunk barriers and source drop interest through the actual reducer.
use super::*;

fn barriers(events: &[Event]) -> Vec<mornlea_domain::BlockChanges> {
    events
        .iter()
        .filter_map(|event| match event {
            Event::BlockChanges(value) => Some(value.clone()),
            _ => None,
        })
        .collect()
}

fn empty_barrier(chunk: ChunkPos, base: u64) -> mornlea_domain::BlockChanges {
    mornlea_domain::BlockChanges::try_new(mornlea_domain::BlockChangesParts {
        dimension: Dimension::OVERWORLD,
        chunk,
        base_revision: base,
        new_revision: base + 1,
        changes: Box::new([]),
    })
    .unwrap()
}

#[test]
fn selected_drop_emits_empty_revision_before_the_next_block_delta() {
    let mut state = authority();
    seed_world(&mut state);
    let owner = login_with(&mut state, 1, "Ada", [0.5, 65.0, 0.5], 0.0, 0.0, |p| {
        p.inventory.hotbar.slots[0] = storage(ITEM_DIRT, 3);
    });
    state.advance_tick(TickBudget::full()).unwrap();
    submit(&mut state, owner, 1, Command::DropSelectedItem);
    let dropped = state.advance_tick(TickBudget::full()).unwrap();
    assert_eq!(state.residents().ready_snapshot()[0].2, 2);
    assert_eq!(
        barriers(&events_for(&dropped, owner)),
        vec![empty_barrier(ChunkPos::new(0, 0), 1)]
    );
    assert!(snapshot_positions(&events_for(&dropped, owner)).is_empty());
    submit(
        &mut state,
        owner,
        2,
        Command::PlaceBlock(
            mornlea_domain::PlacementIntent::try_new(
                look(std::f32::consts::PI, -std::f32::consts::FRAC_PI_4),
                0,
            )
            .unwrap(),
        ),
    );
    let placed = state.advance_tick(TickBudget::full()).unwrap();
    let deltas = barriers(&events_for(&placed, owner));
    assert_eq!(deltas.len(), 1);
    assert_eq!(deltas[0].base_revision(), 2);
    assert_eq!(deltas[0].new_revision(), 3);
    assert_eq!(
        deltas[0].changes(),
        &[BlockChange::try_new(BlockPos::new(0, 65, 2), DIRT_BLOCK).unwrap()]
    );
    assert!(snapshot_positions(&events_for(&placed, owner)).is_empty());
}

#[test]
fn chest_panel_drop_emits_both_slot_barriers_and_late_join_uses_snapshot() {
    let (mut state, owner, _, reference, _, _) =
        projection_container_dirty_fixture(ContainerKind::Chest);
    submit(&mut state, owner, 1, Command::OpenContainer(look(0.0, 0.0)));
    state.advance_tick(TickBudget::full()).unwrap();
    submit(
        &mut state,
        owner,
        2,
        Command::DropStack(StackSource::try_new(StackView::Container(reference), 36).unwrap()),
    );
    let late = login(&mut state, 3, "Cy", [1.5, 65.0, 1.5], 0.0, 0.0);
    let dropped = state.advance_tick(TickBudget::full()).unwrap();
    assert_eq!(
        barriers(&events_for(&dropped, owner)),
        vec![
            empty_barrier(ChunkPos::new(0, -1), 1),
            empty_barrier(ChunkPos::new(0, 0), 1)
        ]
    );
    assert!(barriers(&events_for(&dropped, late)).is_empty());
    assert!(snapshot_positions(&events_for(&dropped, late)).contains(&ChunkPos::new(0, -1)));
    let quiet = state.advance_tick(TickBudget::full()).unwrap();
    assert!(barriers(&events_for(&quiet, owner)).is_empty());
}

fn drop_interest(cap: usize) {
    let mut state = authority_with_view_radius(cap);
    seed_world(&mut state);
    preload_keys(&mut state, &[(2, 0), (3, 0)]);
    stage(&mut state, |context| {
        for x in [2, 3] {
            let mut drop = drop_record(1, 0, [16.0 * x as f32 + 0.5, 65.5, 0.5]);
            drop.id = DropId::try_new(0, ChunkPos::new(x, 0), 1, 1).unwrap();
            context.preload_drop(drop);
        }
    });
    let owner = login_declared(&mut state, 1, "Ada", [0.5, 65.0, 0.5], 0.0, 0.0, 8);
    let tick = state.advance_tick(TickBudget::full()).unwrap();
    let ids: Vec<_> = events_for(&tick, owner)
        .iter()
        .filter_map(|e| match e {
            Event::ItemDropUpserts(batch) => Some(batch.drops()),
            _ => None,
        })
        .flatten()
        .map(|drop| drop.id())
        .collect();
    assert_eq!(
        ids,
        vec![DropId::try_new(0, ChunkPos::new(2, 0), 1, 1).unwrap()]
    );
    if cap == 0 {
        assert!(!snapshot_positions(&events_for(&tick, owner)).contains(&ChunkPos::new(2, 0)));
    }
    let quiet = state.advance_tick(TickBudget::full()).unwrap();
    assert!(projection_drop_wire_events(&events_for(&quiet, owner)).is_empty());
}

#[test]
fn narrow_snapshot_interest_still_receives_ready_radius_two_drops() {
    drop_interest(0);
}
#[test]
fn wide_snapshot_interest_excludes_ready_radius_three_drops() {
    drop_interest(8);
}

#[test]
fn sparse_unready_drop_stays_absent_until_ready_physical_ownership() {
    let mut state = authority();
    seed_world(&mut state);
    stage(&mut state, |context| {
        let mut drop = drop_record(1, 0, [16.5, 65.5, 0.5]);
        drop.id = DropId::try_new(0, ChunkPos::new(1, 0), 1, 1).unwrap();
        context.preload_drop(drop);
    });
    let owner = login(&mut state, 1, "Ada", [0.5, 65.0, 0.5], 0.0, 0.0);
    let unready = state.advance_tick(TickBudget::full()).unwrap();
    assert!(projection_drop_wire_events(&events_for(&unready, owner)).is_empty());
    let mut chunk = ground_chunk();
    chunk.drops[1] = mornlea_storage::DropSlot {
        generation: 1,
        active: true,
        stack: storage(ITEM_COAL, 2),
        block_index: mornlea_domain::chunk_block_index(BlockPos::new(16, 65, 0)) as u32,
        age_ticks: 0,
        pickup_delay_ticks: 5,
    };
    stage(&mut state, |context| {
        context.preload_ready_chunk(ReadyChunk::try_new(chunk_key(1, 0), 1, 1, chunk).unwrap());
    });
    let ready = state.advance_tick(TickBudget::full()).unwrap();
    assert!(
        matches!(projection_drop_wire_events(&events_for(&ready,owner)).as_slice(),[Event::ItemDropUpserts(batch)] if batch.drops()[0].id()==DropId::try_new(0,ChunkPos::new(1,0),1,1).unwrap())
    );
}

#[test]
fn same_tick_slot_and_block_work_share_one_nonempty_revision() {
    let mut state = authority();
    seed_world(&mut state);
    let owner = login_with(&mut state, 1, "Ada", [0.5, 65.0, 0.5], 0.0, 0.0, |p| {
        p.inventory.hotbar.slots[0] = storage(ITEM_DIRT, 3);
    });
    state.advance_tick(TickBudget::full()).unwrap();
    submit(&mut state, owner, 1, Command::DropSelectedItem);
    submit(
        &mut state,
        owner,
        2,
        Command::PlaceBlock(
            mornlea_domain::PlacementIntent::try_new(
                look(std::f32::consts::PI, -std::f32::consts::FRAC_PI_4),
                0,
            )
            .unwrap(),
        ),
    );
    let tick = state.advance_tick(TickBudget::full()).unwrap();
    let deltas = barriers(&events_for(&tick, owner));
    assert_eq!(deltas.len(), 1);
    assert_eq!(deltas[0].base_revision(), 1);
    assert_eq!(deltas[0].new_revision(), 2);
    assert_eq!(deltas[0].changes().len(), 1);
    let quiet = state.advance_tick(TickBudget::full()).unwrap();
    assert!(barriers(&events_for(&quiet, owner)).is_empty());
}
