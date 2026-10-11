use mornlea_client_core::session::WorldConfirmed;
use mornlea_client_core::{ClientError, ClientLimits};
use mornlea_domain::{
    BlockChange, BlockChanges, BlockChangesParts, BlockPos, ChunkPos, ChunkSnapshot,
    ChunkSnapshotParts, Dimension, PalettedSection,
};

fn snapshot(chunk: ChunkPos, revision: u64, block: u16) -> ChunkSnapshot {
    ChunkSnapshot::try_new(ChunkSnapshotParts {
        dimension: Dimension::OVERWORLD,
        chunk,
        revision,
        sections: Box::new(std::array::from_fn(|_| {
            PalettedSection::single(block).unwrap()
        })),
    })
    .unwrap()
}

#[test]
fn confirmed_blocks_follow_snapshot_delta_and_forget_at_negative_coordinates() {
    let limits = ClientLimits::try_new().unwrap();
    let chunk = ChunkPos::new(-1, -1);
    let position = BlockPos::new(-1, -64, -1);
    let mut world = WorldConfirmed::default();
    world
        .apply_snapshot(&snapshot(chunk, 1, 2), &limits)
        .unwrap();
    assert_eq!(world.block_at(Dimension::OVERWORLD, position), Some(2));
    let before = world.clone();
    let stale = snapshot(chunk, 1, 3);
    assert_eq!(
        world.apply_snapshot(&stale, &limits),
        Err(ClientError::InvalidInput)
    );
    assert_eq!(world, before);
    let delta = BlockChanges::try_new(BlockChangesParts {
        dimension: Dimension::OVERWORLD,
        chunk,
        base_revision: 1,
        new_revision: 2,
        changes: vec![BlockChange::try_new(position, 3).unwrap()].into_boxed_slice(),
    })
    .unwrap();
    world.apply_changes(&delta, &limits).unwrap();
    assert_eq!(world.block_at(Dimension::OVERWORLD, position), Some(3));
    assert_eq!(
        world.block_at(Dimension::OVERWORLD, BlockPos::new(-2, -64, -1)),
        Some(2)
    );
    assert_eq!(world.block_at(Dimension::DEPTHS, position), None);
    assert_eq!(
        world.block_at(Dimension::OVERWORLD, BlockPos::new(-1, 320, -1)),
        None
    );
    world.remove_chunk(Dimension::OVERWORLD, chunk);
    assert_eq!(world.block_at(Dimension::OVERWORLD, position), None);
    assert_eq!(world.content_bytes(), 0);
}

#[test]
fn confirmed_block_capacity_rejects_without_retaining_a_partial_column() {
    let base = ClientLimits::try_new().unwrap();
    let limits = ClientLimits::try_new_with(
        base.queued_input_events(),
        base.inbound_observations(),
        1,
        base.outbound_commands(),
        base.outbound_bytes(),
        base.prediction_journal(),
        base.message_work(),
        base.mesh_work(),
        base.preparation_results(),
        base.preparation_bytes(),
        base.family_records(),
        base.frame_bytes(),
    )
    .unwrap();
    let mut world = WorldConfirmed::default();
    assert_eq!(
        world.apply_snapshot(&snapshot(ChunkPos::new(0, 0), 1, 2), &limits),
        Err(ClientError::Capacity)
    );
    assert_eq!(world.chunk_count(), 0);
    assert_eq!(world.content_bytes(), 0);
}
