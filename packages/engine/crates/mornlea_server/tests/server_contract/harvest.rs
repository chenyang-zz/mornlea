//! Literal Go sampler vectors and callable output-consumer examples.
use mornlea_domain::{BlockPos, CompanionId, Dimension, FiniteVec3};
use mornlea_server::{contracts::*, rules::harvest};
use mornlea_storage::ItemStack;

#[test]
fn crop_known_answers_match_frozen_go_sampler() {
    // Literal rows from updates/sampler_test.go, including unsigned casts of negative inputs.
    for (seed, tick, dim, pos, wheat, seeds, potato, carrot) in [
        (
            8_675_309,
            1u64 << 40,
            0,
            BlockPos::new(300, -64, -300),
            1,
            3,
            1,
            4,
        ),
        (-1, 0, 255, BlockPos::new(-1, 320, 1), 2, 1, 3, 2),
        (24_680, 13_579, 3, BlockPos::new(4096, -4096, 0), 1, 1, 3, 1),
    ] {
        assert_eq!(harvest::wheat(seed, tick, dim, pos), (wheat, seeds));
        assert_eq!(harvest::potato(seed, tick, dim, pos), potato);
        assert_eq!(harvest::carrot(seed, tick, dim, pos), carrot);
        assert!(!harvest::poison_potato(seed, tick, dim, pos));
    }
    assert!(harvest::poison_potato(11, 22, 3, BlockPos::new(1, 64, -1)));
}

#[test]
fn short_grass_known_answers_omit_tick() {
    // Literal source vectors from updates/sampler_entity_test.go.
    for (seed, dim, pos, want) in [
        (0, 0, BlockPos::new(0, 1, 5), false),
        (-42, 7, BlockPos::new(-100, 64, 32000), false),
        (8_675_309, 0, BlockPos::new(300, -64, -300), false),
        (5, 9, BlockPos::new(-2, -3, -4), false),
        (11, 3, BlockPos::new(1, 64, -1), true),
    ] {
        assert_eq!(harvest::short_grass(seed, dim, pos), want);
    }
}

#[test]
fn leaves_known_answers_omit_tick_and_have_distinct_salt() {
    for (seed, dim, pos, want) in [
        (0, 0, BlockPos::new(4, 1, 5), false),
        (-42, 7, BlockPos::new(-100, 64, 32000), false),
        (8_675_309, 0, BlockPos::new(300, -64, -300), false),
        (5, 9, BlockPos::new(-2, -3, -4), true),
        (11, 3, BlockPos::new(10, 64, -10), false),
    ] {
        assert_eq!(harvest::leaf_sapling(seed, dim, pos), want);
    }
}

#[test]
fn counts_remain_bounded_at_integer_extremes() {
    for seed in [i64::MIN, -1, 0, i64::MAX] {
        for tick in [0, 1, u64::MAX] {
            for pos in [
                BlockPos::new(i32::MIN, -64, i32::MAX),
                BlockPos::new(i32::MAX, 319, i32::MIN),
            ] {
                let (w, s) = harvest::wheat(seed, tick, u32::MAX, pos);
                assert!((1..=3).contains(&w) && (1..=3).contains(&s));
                assert!((1..=4).contains(&harvest::potato(seed, tick, u32::MAX, pos)));
                assert!((1..=4).contains(&harvest::carrot(seed, tick, u32::MAX, pos)));
                assert_eq!(harvest::wheat(seed, tick, u32::MAX, pos), (w, s));
            }
        }
    }
}

fn wheat_batch(source: DropSource) -> DropBatch {
    let pos = BlockPos::new(300, -64, -300);
    let (w, s) = harvest::wheat(8_675_309, 1u64 << 40, 0, pos);
    DropBatch::try_new(
        source,
        Dimension::OVERWORLD,
        FiniteVec3::try_new([300.5, -63.5, -299.5]).unwrap(),
        vec![
            ItemStack {
                item: 35,
                count: w,
                durability: 0,
            },
            ItemStack {
                item: 34,
                count: s,
                durability: 0,
            },
        ],
        10,
    )
    .unwrap()
}

#[test]
fn both_consumer_examples_preserve_wheat_then_seed_order() {
    let target = BlockPos::new(300, -64, -300);
    let actor = ActorKey::Companion(
        CompanionId::try_from_bytes([1, 0, 0, 0, 0, 0, 64, 0, 128, 0, 0, 0, 0, 0, 0, 1]).unwrap(),
    );
    for source in [
        DropSource::Mining {
            actor,
            target,
            tick: 1u64 << 40,
        },
        DropSource::System {
            rule: SystemRule::Fluid,
            target,
            tick: 1u64 << 40,
        },
    ] {
        let batch = wheat_batch(source);
        assert_eq!(
            batch.stacks,
            vec![
                ItemStack {
                    item: 35,
                    count: 1,
                    durability: 0
                },
                ItemStack {
                    item: 34,
                    count: 3,
                    durability: 0
                }
            ]
        );
    }
}
