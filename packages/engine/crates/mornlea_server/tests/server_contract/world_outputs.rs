//! Fixed container slots participate in actual authoritative world transactions.
use mornlea_domain::{
    BlockPos, ChunkPos, ContainerKind, ContainerRef, Dimension, Season, Weather, WorldState,
    WorldStateParts, chunk_block_index,
};
use mornlea_server::{
    contracts::*,
    core::world::ReadyChunk,
    state::{AuthorityState, TickContext},
};
use mornlea_storage::{ChestSlot, Chunk, ContainerSnapshot, FurnaceSlot, ItemStack, StorageKind};

fn key(dimension: Dimension) -> ChunkKey {
    ChunkKey {
        dimension,
        pos: ChunkPos::new(0, 0),
    }
}
pub(super) fn empty() -> Chunk {
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
        drops: vec![Default::default(); 32],
        furnaces: vec![Default::default(); 32],
        chests: vec![Default::default(); 16],
    }
}
pub(super) fn put(chunk: &mut Chunk, pos: BlockPos, block: u16) {
    let index = chunk_block_index(pos) as usize;
    let section = &mut chunk.sections[index / 4096];
    if section.kind != StorageKind::Direct {
        *section = ContainerSnapshot {
            kind: StorageKind::Direct,
            bits: 15,
            single: 0,
            palette: vec![],
            packed: vec![0; 1024],
        };
    }
    let cell = index % 4096;
    section.packed[cell / 4] |= u64::from(block) << ((cell % 4) * 15);
}
fn stack(item: u16, count: u8) -> ItemStack {
    ItemStack {
        item,
        count,
        durability: 0,
    }
}
fn reference(kind: ContainerKind, slot: u8, generation: u32) -> ContainerRef {
    ContainerRef::try_new(key(Dimension::OVERWORLD).pos, kind, slot, generation).unwrap()
}
fn state() -> AuthorityState {
    AuthorityState::try_new(
        ServerLimits::try_new(8, 4096, 512, 64, 64, 1_048_576).unwrap(),
        7,
    )
    .unwrap()
}
pub(super) fn world() -> WorldState {
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
fn install(ctx: &mut TickContext<'_>, dimension: Dimension, chunk: Chunk, revision: u64) {
    ctx.preload_ready_chunk(ReadyChunk::try_new(key(dimension), 1, revision, chunk).unwrap());
}
pub(super) fn with_chest() -> Chunk {
    let mut data = empty();
    let pos = BlockPos::new(0, 65, 2);
    put(&mut data, pos, 11);
    data.chests[5] = ChestSlot {
        generation: 7,
        active: true,
        block_index: chunk_block_index(pos),
        items: [Default::default(); 27],
    };
    data.chests[5].items[0] = stack(1, 4);
    data
}

#[test]
fn ready_container_lookup_uses_actual_slot_and_generation() {
    let mut data = with_chest();
    let pos = BlockPos::new(1, 65, 2);
    put(&mut data, pos, 9);
    data.furnaces[3] = FurnaceSlot {
        generation: 9,
        active: true,
        block_index: chunk_block_index(pos),
        input: stack(6, 2),
        fuel: stack(5, 1),
        output: Default::default(),
        progress_ticks: 10,
        burn_ticks: 1500,
    };
    let mut authority = state();
    let mut ctx = TickContext::harness(&mut authority, TickBudget::full());
    install(&mut ctx, Dimension::OVERWORLD, data, 8);
    assert!(
        ctx.read()
            .container(reference(ContainerKind::Chest, 5, 7))
            .is_some()
    );
    assert!(
        ctx.read()
            .container(reference(ContainerKind::Furnace, 3, 9))
            .is_some()
    );
    assert!(
        ctx.read()
            .container(reference(ContainerKind::Chest, 0, 1))
            .is_none()
    );
}

#[test]
fn container_patch_survives_real_chunk_save() {
    let mut authority = state();
    let mut ctx = TickContext::harness(&mut authority, TickBudget::full());
    install(&mut ctx, Dimension::OVERWORLD, with_chest(), 8);
    let before = ctx
        .read()
        .container(reference(ContainerKind::Chest, 5, 7))
        .unwrap();
    let mut after = before.clone();
    let ContainerSlots::Chest(ref mut cells) = after.slots else {
        panic!("chest")
    };
    cells[0] = stack(1, 2);
    ctx.stage(RuleEffect::Container { before, after }).unwrap();
    let snapshot = ctx.snapshot_state(world());
    assert_eq!(snapshot.chunks[0].2, 9);
    assert_eq!(snapshot.chunks[0].3.chests[5].items[0], stack(1, 2));
    let save = mornlea_storage::ChunkSave {
        key: mornlea_storage::ChunkKey {
            dimension: 0,
            x: 0,
            z: 0,
        },
        revision: 9,
        chunk: snapshot.chunks[0].3.clone(),
    };
    let bytes = mornlea_storage::encode_chunk(&save).unwrap();
    assert!(!bytes.is_empty());
}

#[test]
fn mined_drop_retains_exact_integer_cell_beyond_float_precision() {
    let owner = ChunkKey {
        dimension: Dimension::OVERWORLD,
        pos: ChunkPos::new(1_048_576, 0),
    };
    let mut authority = state();
    let mut ctx = TickContext::harness(&mut authority, TickBudget::full());
    ctx.preload_ready_chunk(ReadyChunk::try_new(owner, 1, 7, empty()).unwrap());
    let target = BlockPos::new(16_777_217, 64, 0);
    let source = DropSource::Mining {
        actor: ActorKey::Companion(
            mornlea_domain::CompanionId::try_from_bytes([
                1, 0, 0, 0, 0, 0, 64, 0, 128, 0, 0, 0, 0, 0, 0, 1,
            ])
            .unwrap(),
        ),
        target,
        tick: 0,
    };
    let batch = DropBatch::try_new(
        source,
        Dimension::OVERWORLD,
        mornlea_domain::FiniteVec3::try_new([target.x() as f32 + 0.5, 64.5, 0.5]).unwrap(),
        vec![stack(1, 1)],
        10,
    )
    .unwrap();
    ctx.stage(RuleEffect::Drops(batch)).unwrap();
    assert_eq!(
        ctx.snapshot_state(world()).chunks[0].3.drops[0].block_index,
        chunk_block_index(target)
    );
}

fn furnace_chunk() -> Chunk {
    let mut chunk = empty();
    let pos = BlockPos::new(0, 65, 2);
    put(&mut chunk, pos, 9);
    chunk.furnaces[3] = FurnaceSlot {
        active: true,
        generation: 9,
        block_index: chunk_block_index(pos),
        input: stack(6, 2),
        fuel: stack(5, 1),
        ..Default::default()
    };
    chunk
}

#[test]
fn dimensions_own_distinct_slots_and_ready_miss_never_uses_sparse_fixture() {
    let mut authority = state();
    let mut ctx = TickContext::harness(&mut authority, TickBudget::full());
    let stale = ContainerRecord {
        reference: reference(ContainerKind::Chest, 0, 1),
        revision: 8,
        slots: ContainerSlots::Chest([stack(1, 1); 27]),
    };
    ctx.preload_container(stale.clone());
    install(&mut ctx, Dimension::OVERWORLD, with_chest(), 8);
    let mut depths = with_chest();
    depths.chests[5].items[0] = stack(5, 6);
    install(&mut ctx, Dimension::DEPTHS, depths, 10);
    let reference = reference(ContainerKind::Chest, 5, 7);
    let overworld = ctx
        .read()
        .world_container(Dimension::OVERWORLD, reference)
        .unwrap();
    let before = ctx
        .read()
        .world_container(Dimension::DEPTHS, reference)
        .unwrap();
    assert_ne!(overworld.slots, before.slots);
    assert_eq!(
        ctx.read().container_at(
            Dimension::DEPTHS,
            BlockPos::new(0, 65, 2),
            ContainerKind::Chest
        ),
        Some(before.clone())
    );
    assert!(ctx.read().container(stale.reference).is_none());
    let mut after = before.clone();
    after.slots = ContainerSlots::Chest([Default::default(); 27]);
    ctx.stage(RuleEffect::WorldContainer {
        dimension: Dimension::DEPTHS,
        before,
        after: after.clone(),
    })
    .unwrap();
    assert_eq!(ctx.read().container(reference), Some(overworld));
    assert_eq!(
        ctx.read().world_container(Dimension::DEPTHS, reference),
        Some(after)
    );
    assert_eq!(
        ctx.read().container_refs(key(Dimension::DEPTHS)),
        vec![reference]
    );
    let saved = ctx.snapshot_state(world());
    assert_eq!(
        saved
            .chunks
            .iter()
            .find(|entry| entry.0.dimension == Dimension::OVERWORLD)
            .unwrap()
            .2,
        8
    );
    assert_eq!(
        saved
            .chunks
            .iter()
            .find(|entry| entry.0.dimension == Dimension::DEPTHS)
            .unwrap()
            .2,
        11
    );
    assert_eq!(saved.containers.len(), 1);
}

#[test]
fn furnace_counters_and_multiple_lanes_share_one_durable_revision() {
    let mut authority = state();
    let mut ctx = TickContext::harness(&mut authority, TickBudget::full());
    let mut data = furnace_chunk();
    data.furnaces[8].generation = 19;
    install(&mut ctx, Dimension::OVERWORLD, data, 8);
    let before = ctx
        .read()
        .container(reference(ContainerKind::Furnace, 3, 9))
        .unwrap();
    let mut after = before.clone();
    if let ContainerSlots::Furnace { fuel, progress, .. } = &mut after.slots {
        *fuel = 1599;
        *progress = 199;
    }
    let next = after.clone();
    ctx.stage(RuleEffect::Container { before, after }).unwrap();
    let target = BlockPos::new(3, 65, 3);
    let observed = ctx
        .read()
        .observation(Dimension::OVERWORLD, target)
        .unwrap();
    ctx.transaction()
        .try_system_with_drops(
            SystemRule::Support,
            vec![BlockWrite {
                observed,
                replacement: 2,
            }],
            system_drop(target, 1),
        )
        .unwrap();
    let snapshot = ctx.snapshot_state(world());
    assert_eq!(snapshot.chunks[0].2, 9);
    assert_eq!(snapshot.chunks[0].3.furnaces[8].generation, 19);
    let save = mornlea_storage::ChunkSave {
        key: mornlea_storage::ChunkKey {
            dimension: 0,
            x: 0,
            z: 0,
        },
        revision: 9,
        chunk: snapshot.chunks[0].3.clone(),
    };
    let bytes = mornlea_storage::encode_chunk(&save).unwrap();
    let decoded = mornlea_storage::decode_chunk(save.key, save.revision, &bytes).unwrap();
    assert_eq!(decoded.chunk, save.chunk);
    assert_eq!(ctx.read().container(next.reference), Some(next));
}

#[test]
fn invalid_or_stale_container_compound_preserves_every_output_lane() {
    for variant in 0..8 {
        let mut authority = state();
        let mut ctx = TickContext::harness(&mut authority, TickBudget::full());
        install(&mut ctx, Dimension::OVERWORLD, furnace_chunk(), 8);
        let before = ctx
            .read()
            .container(reference(ContainerKind::Furnace, 3, 9))
            .unwrap();
        let mut after = before.clone();
        match variant {
            0 => after.reference = reference(ContainerKind::Furnace, 3, 10),
            1 => after.revision += 1,
            2 => {
                if let ContainerSlots::Furnace { slots, .. } = &mut after.slots {
                    slots[1] = stack(1, 1)
                }
            }
            3 => {
                if let ContainerSlots::Furnace { progress, .. } = &mut after.slots {
                    *progress = 200
                }
            }
            4 => {
                if let ContainerSlots::Furnace { fuel, .. } = &mut after.slots {
                    *fuel = 1601
                }
            }
            5 => {
                if let ContainerSlots::Furnace { progress, .. } = &mut after.slots {
                    *progress = 65536
                }
            }
            6 => {
                if let ContainerSlots::Furnace { fuel, .. } = &mut after.slots {
                    *fuel = 65536
                }
            }
            _ => {
                if let ContainerSlots::Furnace { slots, .. } = &mut after.slots {
                    slots[0].count = 1
                }
            }
        }
        let snapshot = ctx.snapshot_state(world());
        let mut effects = vec![
            RuleEffect::Drops(system_drop(BlockPos::new(3, 65, 3), 1)),
            RuleEffect::Container {
                before: before.clone(),
                after,
            },
        ];
        if variant == 7 {
            effects.push(RuleEffect::Container {
                before: before.clone(),
                after: before.clone(),
            });
        }
        assert!(
            ctx.stage(RuleEffect::Compound(effects)).is_err(),
            "variant {variant}"
        );
        assert_eq!(ctx.snapshot_state(world()), snapshot, "variant {variant}");
    }
}

#[test]
fn ordered_container_patches_preserve_explicit_durable_touch() {
    for revision in [8, u64::MAX] {
        let mut authority = state();
        let mut ctx = TickContext::harness(&mut authority, TickBudget::full());
        install(&mut ctx, Dimension::OVERWORLD, with_chest(), revision);
        let before = ctx
            .read()
            .container(reference(ContainerKind::Chest, 5, 7))
            .unwrap();
        let snapshot = ctx.snapshot_state(world());
        let equal = RuleEffect::Container {
            before: before.clone(),
            after: before.clone(),
        };
        // Equal slots still mean an explicit successful container write; an
        // idle furnace avoids staging this patch in the first place.
        if revision == u64::MAX {
            assert_eq!(ctx.stage(equal), Err(RuleReject::StaleObservation));
            assert_eq!(ctx.snapshot_state(world()), snapshot);
        } else {
            ctx.stage(equal).unwrap();
            assert_eq!(ctx.snapshot_state(world()).chunks[0].2, 9);
        }
        let mut after = before.clone();
        after.slots = ContainerSlots::Chest([Default::default(); 27]);
        let effects = RuleEffect::Compound(vec![
            RuleEffect::Container {
                before: before.clone(),
                after: after.clone(),
            },
            RuleEffect::Container {
                before: after,
                after: before,
            },
        ]);
        if revision == u64::MAX {
            assert!(ctx.stage(effects).is_err());
            assert_eq!(ctx.snapshot_state(world()), snapshot);
        } else {
            ctx.stage(effects).unwrap();
            assert_eq!(ctx.snapshot_state(world()).chunks[0].2, 9);
        }
    }
}

fn system_drop(target: BlockPos, count: u8) -> DropBatch {
    DropBatch::try_new(
        DropSource::System {
            rule: SystemRule::Support,
            tick: 0,
            target,
        },
        Dimension::OVERWORLD,
        mornlea_domain::FiniteVec3::try_new([
            target.x() as f32 + 0.5,
            target.y() as f32 + 0.5,
            target.z() as f32 + 0.5,
        ])
        .unwrap(),
        vec![stack(1, count)],
        10,
    )
    .unwrap()
}

#[test]
fn stale_or_full_system_output_preserves_blocks_and_slots() {
    for full in [false, true] {
        let mut authority = state();
        let mut ctx = TickContext::harness(&mut authority, TickBudget::full());
        let mut data = empty();
        if full {
            for (index, drop) in data.drops.iter_mut().enumerate() {
                *drop = mornlea_storage::DropSlot {
                    active: true,
                    generation: 1,
                    block_index: index as u32,
                    stack: stack(2, 64),
                    ..Default::default()
                };
            }
        }
        install(&mut ctx, Dimension::OVERWORLD, data, 8);
        let target = BlockPos::new(3, 65, 3);
        let mut observed = ctx
            .read()
            .observation(Dimension::OVERWORLD, target)
            .unwrap();
        if !full {
            observed.block = 2;
        }
        let before = ctx.snapshot_state(world());
        assert!(
            ctx.transaction()
                .try_system_with_drops(
                    SystemRule::Support,
                    vec![BlockWrite {
                        observed,
                        replacement: 3
                    }],
                    system_drop(target, 1)
                )
                .is_err()
        );
        assert_eq!(ctx.snapshot_state(world()), before);
    }
}

#[test]
fn dirty_ready_containers_replay_with_emitted_chunk_revision() {
    for lane in 0..3 {
        let mut authority = state();
        let mut ctx = TickContext::harness(&mut authority, TickBudget::full());
        install(&mut ctx, Dimension::OVERWORLD, with_chest(), 8);
        let before = ctx
            .read()
            .container(reference(ContainerKind::Chest, 5, 7))
            .unwrap();
        match lane {
            0 => {
                let mut after = before.clone();
                after.slots = ContainerSlots::Chest([Default::default(); 27]);
                ctx.stage(RuleEffect::Container { before, after }).unwrap();
            }
            1 => {
                let observed = ctx
                    .read()
                    .observation(Dimension::OVERWORLD, BlockPos::new(3, 65, 3))
                    .unwrap();
                ctx.transaction()
                    .try_system(
                        SystemRule::Support,
                        vec![BlockWrite {
                            observed,
                            replacement: 2,
                        }],
                    )
                    .unwrap();
            }
            _ => ctx
                .stage(RuleEffect::Drops(system_drop(BlockPos::new(3, 65, 3), 1)))
                .unwrap(),
        }
        let saved = ctx.snapshot_state(world());
        let mut restarted = state();
        let restored = TickContext::from_fixture(&mut restarted, &saved, TickBudget::full());
        assert_eq!(
            restored
                .read()
                .container(reference(ContainerKind::Chest, 5, 7))
                .unwrap()
                .revision,
            9
        );
        assert_eq!(restored.snapshot_state(world()), saved);
    }
}

#[test]
fn sparse_container_patches_use_the_same_checked_payload_boundary() {
    for kind in [ContainerKind::Chest, ContainerKind::Furnace] {
        let mut authority = state();
        let mut ctx = TickContext::harness(&mut authority, TickBudget::full());
        let slots = match kind {
            ContainerKind::Chest => ContainerSlots::Chest([Default::default(); 27]),
            ContainerKind::Furnace => ContainerSlots::Furnace {
                slots: [Default::default(); 3],
                fuel: 0,
                progress: 0,
            },
        };
        let before = ContainerRecord {
            reference: reference(kind, 0, 1),
            revision: 8,
            slots,
        };
        ctx.preload_container(before.clone());
        let mut after = before.clone();
        match &mut after.slots {
            ContainerSlots::Chest(items) => items[0] = stack(u16::MAX, 1),
            ContainerSlots::Furnace { slots, .. } => slots[1] = stack(1, 1),
        };
        let saved = ctx.snapshot_state(world());
        assert!(ctx.stage(RuleEffect::Container { before, after }).is_err());
        assert_eq!(ctx.snapshot_state(world()), saved);
    }
}
