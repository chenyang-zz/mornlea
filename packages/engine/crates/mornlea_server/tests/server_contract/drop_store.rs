//! Real staging preserves slot identity, bounded capacity and durable revisions.
use mornlea_domain::{
    BlockPos, ChunkPos, Dimension, FiniteVec3, Season, Weather, WorldState, WorldStateParts,
};
use mornlea_server::{
    contracts::*,
    core::world::ReadyChunk,
    state::{AuthorityState, TickContext},
};
use mornlea_storage::{Chunk, ContainerSnapshot, DropSlot, ItemStack, StorageKind};

fn key() -> ChunkKey {
    ChunkKey {
        dimension: Dimension::OVERWORLD,
        pos: ChunkPos::new(0, 0),
    }
}
fn chunk() -> Chunk {
    Chunk {
        sections: vec![
            ContainerSnapshot {
                kind: StorageKind::Single,
                bits: 0,
                single: 0,
                palette: vec![],
                packed: vec![]
            };
            24
        ],
        drops: vec![DropSlot::default(); 32],
        furnaces: vec![Default::default(); 32],
        chests: vec![Default::default(); 16],
    }
}
fn state() -> AuthorityState {
    AuthorityState::try_new(
        ServerLimits::try_new(8, 4096, 512, 64, 64, 1_048_576).unwrap(),
        0,
    )
    .unwrap()
}
fn budget() -> TickBudget {
    TickBudget::try_new(0, 0, 0, 0, 0).unwrap()
}
fn world() -> WorldState {
    WorldState::try_new(WorldStateParts {
        day_phase_offset: 0,
        world_time_ticks: 0,
        weather: Weather::Clear,
        season: Season::Spring,
        season_progress: 0,
        temperature: 0,
    })
    .unwrap()
}
fn stack(item: u16, count: u8) -> ItemStack {
    ItemStack {
        item,
        count,
        durability: 0,
    }
}
fn batch(stacks: Vec<ItemStack>, delay: u8) -> DropBatch {
    DropBatch::try_new(
        DropSource::Death {
            actor: ActorKey::Companion(
                mornlea_domain::CompanionId::try_from_bytes([
                    1, 0, 0, 0, 0, 0, 64, 0, 128, 0, 0, 0, 0, 0, 0, 1,
                ])
                .unwrap(),
            ),
            tick: 1,
        },
        Dimension::OVERWORLD,
        FiniteVec3::try_new([0.1, 64.9, 0.2]).unwrap(),
        stacks,
        delay,
    )
    .unwrap()
}
fn setup(ctx: &mut TickContext<'_>, data: Chunk, revision: u64) {
    ctx.preload_ready_chunk(ReadyChunk::try_new(key(), 1, revision, data).unwrap());
}

#[test]
fn input_stack_bound_is_distinct_from_physical_slots() {
    let mut authority = state();
    let mut ctx = TickContext::harness(&mut authority, budget());
    setup(&mut ctx, chunk(), 1);
    let mut inputs = vec![stack(1, 64); 28];
    inputs.extend(vec![stack(5, 8); 8]);
    ctx.stage(RuleEffect::Drops(batch(inputs, 20))).unwrap();
    let drops = ctx.read().drops(key()).to_vec();
    assert_eq!(drops.len(), 29);
    assert_eq!(drops[28].stack, stack(5, 64));
    let mut invalid = batch(vec![], 0);
    invalid.stacks = vec![stack(1, 1); 37];
    assert!(ctx.stage(RuleEffect::Drops(invalid)).is_err());
    assert_eq!(ctx.read().drops(key()), drops);
}

#[test]
fn merge_retains_age_identity_and_longest_delay() {
    let mut authority = state();
    let mut ctx = TickContext::harness(&mut authority, budget());
    setup(&mut ctx, chunk(), 7);
    ctx.stage(RuleEffect::Drops(batch(vec![stack(1, 60)], 5)))
        .unwrap();
    let before = ctx.read().drops(key())[0].clone();
    let mut aged = before.clone();
    aged.age = 9;
    aged.pickup_delay = 2;
    ctx.stage(RuleEffect::DropPatch {
        before,
        after: Some(aged.clone()),
    })
    .unwrap();
    ctx.stage(RuleEffect::Drops(batch(vec![stack(1, 8)], 10)))
        .unwrap();
    let read = ctx.read();
    let records = read.drops(key());
    assert_eq!(records.len(), 2);
    assert_eq!(records[0].id, aged.id);
    assert_eq!(records[0].age, 9);
    assert_eq!(records[0].pickup_delay, 10);
    assert_eq!(records[0].stack.count, 64);
    assert_eq!(records[1].stack.count, 4);
    assert_eq!(records[1].age, 0);
    assert_eq!(records[0].position.get(), [0.5, 64.5, 0.5]);
    assert_eq!(ctx.snapshot_state(world()).chunks[0].2, 8);
}

#[test]
fn clear_rebirth_preserves_generation_and_compound_order() {
    let mut authority = state();
    let mut ctx = TickContext::harness(&mut authority, budget());
    let mut data = chunk();
    data.drops[0].generation = u32::MAX;
    setup(&mut ctx, data, 8);
    ctx.stage(RuleEffect::Drops(batch(vec![stack(1, 1)], 0)))
        .unwrap();
    let old = ctx.read().drops(key())[0].clone();
    assert_eq!(old.id.slot(), 1);
    ctx.stage(RuleEffect::Compound(vec![
        RuleEffect::DropPatch {
            before: old.clone(),
            after: None,
        },
        RuleEffect::Drops(batch(vec![stack(1, 1)], 0)),
    ]))
    .unwrap();
    let current = ctx.read().drops(key())[0].clone();
    assert_eq!(current.id.slot(), 1);
    assert_eq!(current.id.generation(), 2);
    assert!(
        ctx.stage(RuleEffect::DropPatch {
            before: old,
            after: None
        })
        .is_err()
    );
    let snap = ctx.snapshot_state(world());
    assert_eq!(snap.chunks[0].3.drops[0].generation, u32::MAX);
    let mut next = state();
    let restored = TickContext::from_fixture(&mut next, &snap, budget());
    assert_eq!(restored.read().drops(key()), [current]);
}

#[test]
fn failed_compound_and_capacity_preserve_all_slots() {
    let mut authority = state();
    let mut ctx = TickContext::harness(&mut authority, budget());
    setup(&mut ctx, chunk(), 3);
    let before = ctx.snapshot_state(world());
    let full = batch(vec![stack(1, 64); 32], 0);
    assert!(
        ctx.stage(RuleEffect::Compound(vec![
            RuleEffect::Drops(full),
            RuleEffect::Drops(batch(vec![stack(5, 1)], 0))
        ]))
        .is_err()
    );
    assert_eq!(ctx.snapshot_state(world()), before);
    ctx.stage(RuleEffect::Drops(batch(vec![stack(1, 64); 32], 0)))
        .unwrap();
    let before = ctx.snapshot_state(world());
    assert!(
        ctx.stage(RuleEffect::Drops(batch(vec![stack(1, 1)], 0)))
            .is_err()
    );
    assert_eq!(ctx.snapshot_state(world()), before);
}

#[test]
fn durable_drop_changes_share_block_revision_but_aging_does_not() {
    let mut data = chunk();
    data.drops[0] = DropSlot {
        generation: 4,
        active: true,
        stack: stack(1, 3),
        block_index: 128 * 256,
        age_ticks: 0,
        pickup_delay_ticks: 40,
    };
    let mut authority = state();
    let mut ctx = TickContext::harness(&mut authority, budget());
    setup(&mut ctx, data, 10);
    let old = ctx.read().drops(key())[0].clone();
    let mut after = old.clone();
    after.age = 10;
    after.pickup_delay = 30;
    ctx.stage(RuleEffect::DropPatch {
        before: old,
        after: Some(after.clone()),
    })
    .unwrap();
    let snap = ctx.snapshot_state(world());
    assert_eq!(snap.chunks[0].2, 10);
    assert_eq!(snap.chunks[0].3.drops[0].age_ticks, 10);
    let mut partial = after.clone();
    partial.stack.count = 2;
    ctx.stage(RuleEffect::DropPatch {
        before: after,
        after: Some(partial),
    })
    .unwrap();
    let observed = ctx
        .read()
        .observation(Dimension::OVERWORLD, BlockPos::new(0, 0, 0))
        .unwrap();
    ctx.transaction()
        .try_system(
            SystemRule::Support,
            vec![BlockWrite::try_new(observed, 1).unwrap()],
        )
        .unwrap();
    assert_eq!(ctx.snapshot_state(world()).chunks[0].2, 11);
}

#[test]
fn patches_refuse_identity_position_item_and_count_forgery() {
    let mut authority = state();
    let mut ctx = TickContext::harness(&mut authority, budget());
    setup(&mut ctx, chunk(), 3);
    ctx.stage(RuleEffect::Drops(batch(vec![stack(1, 2)], 0)))
        .unwrap();
    let old = ctx.read().drops(key())[0].clone();
    for mode in 0..5 {
        let mut bad = old.clone();
        match mode {
            0 => bad.stack.count = 3,
            1 => bad.stack.item = 5,
            2 => bad.position = FiniteVec3::try_new([1.5, 64.5, 0.5]).unwrap(),
            3 => bad.stack.count = 0,
            _ => bad.id = mornlea_domain::DropId::try_new(0, key().pos, 1, 1).unwrap(),
        };
        assert!(
            ctx.stage(RuleEffect::DropPatch {
                before: old.clone(),
                after: Some(bad)
            })
            .is_err()
        );
        assert_eq!(ctx.read().drops(key()), std::slice::from_ref(&old));
    }
}

#[test]
fn missing_invalid_origin_and_exhausted_revision_are_state_preserving() {
    let mut authority = state();
    let mut ctx = TickContext::harness(&mut authority, budget());
    assert!(
        ctx.stage(RuleEffect::Drops(batch(vec![stack(1, 1)], 0)))
            .is_err()
    );
    setup(&mut ctx, chunk(), u64::MAX);
    assert!(
        ctx.stage(RuleEffect::Drops(batch(vec![stack(1, 1)], 0)))
            .is_err()
    );
    for pos in [[0., 320., 0.], [2147483648., 64., 0.], [0., -65., 0.]] {
        let mut bad = batch(vec![stack(1, 1)], 0);
        bad.origin = FiniteVec3::try_new(pos).unwrap();
        assert!(ctx.stage(RuleEffect::Drops(bad)).is_err());
    }
    assert!(ctx.read().drops(key()).is_empty());
}

#[test]
fn restored_slots_do_not_round_trip_through_float_coordinates() {
    let far = ChunkKey {
        dimension: Dimension::OVERWORLD,
        pos: ChunkPos::new(i32::MAX, 0),
    };
    let mut data = chunk();
    data.drops[0] = DropSlot {
        generation: 1,
        active: true,
        stack: stack(1, 1),
        block_index: 128 * 256 + 15,
        ..Default::default()
    };
    let mut authority = state();
    let mut ctx = TickContext::harness(&mut authority, budget());
    ctx.preload_ready_chunk(ReadyChunk::try_new(far, 1, 9, data).unwrap());
    let snapshot = ctx.snapshot_state(world());
    let mut next = state();
    let restored = TickContext::from_fixture(&mut next, &snapshot, budget());
    assert_eq!(restored.snapshot_state(world()), snapshot);
}

#[test]
fn exhausted_revision_allows_only_counter_changes() {
    let mut data = chunk();
    data.drops[0] = DropSlot {
        generation: 1,
        active: true,
        stack: stack(1, 1),
        block_index: 128 * 256,
        pickup_delay_ticks: 40,
        ..Default::default()
    };
    let mut authority = state();
    let mut ctx = TickContext::harness(&mut authority, budget());
    setup(&mut ctx, data, u64::MAX);
    let before = ctx.read().drops(key())[0].clone();
    let mut after = before.clone();
    after.age = 10;
    after.pickup_delay = 30;
    ctx.stage(RuleEffect::DropPatch {
        before,
        after: Some(after.clone()),
    })
    .unwrap();
    assert_eq!(ctx.snapshot_state(world()).chunks[0].2, u64::MAX);
    assert!(
        ctx.stage(RuleEffect::DropPatch {
            before: after.clone(),
            after: None
        })
        .is_err()
    );
    assert_eq!(ctx.read().drops(key()), [after]);
}

#[test]
fn preflight_is_pure_and_negative_chunk_center_is_exact() {
    let negative = ChunkKey {
        dimension: Dimension::DEPTHS,
        pos: ChunkPos::new(-1, -1),
    };
    let mut authority = state();
    let mut ctx = TickContext::harness(&mut authority, budget());
    ctx.preload_ready_chunk(ReadyChunk::try_new(negative, 1, 2, chunk()).unwrap());
    let mut input = batch(vec![stack(1, 1)], 4);
    input.dimension = Dimension::DEPTHS;
    input.origin = FiniteVec3::try_new([-0.1, -63.2, -15.9]).unwrap();
    ctx.read().check_drop_batch(&input).unwrap();
    assert!(ctx.read().drops(negative).is_empty());
    ctx.stage(RuleEffect::Drops(input)).unwrap();
    let read = ctx.read();
    let item = &read.drops(negative)[0];
    assert_eq!(item.position.get(), [-0.5, -63.5, -15.5]);
    assert_eq!(item.id.chunk(), negative.pos);
}

#[test]
fn fixture_overlay_cannot_resurrect_an_inactive_generation() {
    let mut data = chunk();
    data.drops[0].generation = 7;
    let mut authority = state();
    let mut ctx = TickContext::harness(&mut authority, budget());
    setup(&mut ctx, data, 3);
    let before = ctx.snapshot_state(world());
    let record = DropRecord {
        id: mornlea_domain::DropId::try_new(0, key().pos, 0, 1).unwrap(),
        position: FiniteVec3::try_new([0.5, 64.5, 0.5]).unwrap(),
        stack: stack(1, 1),
        pickup_delay: 0,
        age: 0,
    };
    let result =
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| ctx.preload_drop(record)));
    assert!(result.is_err());
    assert_eq!(ctx.snapshot_state(world()), before);
}

#[test]
fn loaded_center_preserves_source_wrapping_and_float_rounding() {
    for (x, local, want) in [
        (134_217_728, 0, -2_147_483_648.0f32),
        (1_048_576, 1, 16_777_216.0f32),
    ] {
        let owner = ChunkKey {
            dimension: Dimension::OVERWORLD,
            pos: ChunkPos::new(x, 0),
        };
        let mut data = chunk();
        data.drops[0] = DropSlot {
            generation: 1,
            active: true,
            stack: stack(1, 1),
            block_index: 128 * 256 + local,
            ..Default::default()
        };
        let mut authority = state();
        let mut ctx = TickContext::harness(&mut authority, budget());
        ctx.preload_ready_chunk(ReadyChunk::try_new(owner, 1, 2, data).unwrap());
        assert_eq!(ctx.read().drops(owner)[0].position.get()[0], want);
    }
}

#[test]
fn mutable_invalid_stack_and_other_lane_refusal_publish_no_drops() {
    let mut authority = state();
    let mut ctx = TickContext::harness(&mut authority, budget());
    setup(&mut ctx, chunk(), 3);
    let before = ctx.snapshot_state(world());
    let mut bad = batch(vec![stack(1, 1)], 0);
    bad.stacks[0].count = 65;
    assert!(ctx.stage(RuleEffect::Drops(bad)).is_err());
    assert_eq!(ctx.snapshot_state(world()), before);
    assert!(
        ctx.stage(RuleEffect::Compound(vec![
            RuleEffect::Drops(batch(vec![stack(1, 1)], 0)),
            RuleEffect::Projectile {
                before: None,
                after: None
            }
        ]))
        .is_err()
    );
    assert_eq!(ctx.snapshot_state(world()), before);
}

#[test]
fn identical_patch_and_empty_batch_keep_snapshot_exact() {
    let mut data = chunk();
    data.drops[0] = DropSlot {
        generation: 7,
        active: true,
        stack: stack(1, 3),
        block_index: 128 * 256,
        age_ticks: 19,
        pickup_delay_ticks: 5,
    };
    let mut authority = state();
    let mut ctx = TickContext::harness(&mut authority, budget());
    setup(&mut ctx, data, 5);
    let before = ctx.snapshot_state(world());
    let current = ctx.read().drops(key())[0].clone();
    ctx.stage(RuleEffect::DropPatch {
        before: current.clone(),
        after: Some(current),
    })
    .unwrap();
    ctx.stage(RuleEffect::Drops(batch(vec![ItemStack::default()], 0)))
        .unwrap();
    assert_eq!(ctx.snapshot_state(world()), before);
}
