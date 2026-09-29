//! Source target predicates stay independent of collision and output rules.
use mornlea_domain::{BlockPos, ChunkPos, Dimension};
use mornlea_server::{
    contracts::*,
    core::interaction::target_block,
    state::{AuthorityState, TickContext},
};

fn authority() -> AuthorityState {
    AuthorityState::try_new(
        ServerLimits::try_new(8, 4096, 512, 64, 64, 1_048_576).unwrap(),
        0,
    )
    .unwrap()
}
fn observe(ctx: &mut TickContext<'_>, dimension: Dimension, pos: BlockPos, block: u16) {
    ctx.preload_block(BlockObservation {
        key: ChunkKey {
            dimension,
            pos: ChunkPos::new(pos.x() >> 4, pos.z() >> 4),
        },
        generation: 1,
        revision: 1,
        pos,
        block,
    });
}

#[test]
fn literal_source_target_matrix_keeps_plants_and_glass() {
    let mut authority = authority();
    let ctx = TickContext::harness(&mut authority, TickBudget::full());
    let non_targets = [0, 27, 28, 29, 30, 31, 32, 33, 34, 63, 65, 67, 69];
    for block in (0..=89).chain([u16::MAX]) {
        assert_eq!(
            target_block(
                &ctx.read(),
                Dimension::OVERWORLD,
                BlockPos::new(0, 65, 0),
                block
            ),
            !non_targets.contains(&block),
            "block {block}"
        );
    }
    assert!(ctx.events().is_empty());
}

#[test]
fn upper_door_follows_exact_lower_in_its_own_dimension() {
    let upper = BlockPos::new(-1, 65, -1);
    let lower = BlockPos::new(-1, 64, -1);
    for block in 62..=69 {
        let mut authority = authority();
        let mut ctx = TickContext::harness(&mut authority, TickBudget::full());
        observe(&mut ctx, Dimension::OVERWORLD, lower, block);
        observe(
            &mut ctx,
            Dimension::DEPTHS,
            lower,
            if block % 2 == 0 { 63 } else { 62 },
        );
        assert_eq!(
            target_block(&ctx.read(), Dimension::OVERWORLD, upper, 70),
            block % 2 == 0
        );
        assert_eq!(
            target_block(&ctx.read(), Dimension::DEPTHS, upper, 70),
            block % 2 != 0
        );
        assert_eq!(ctx.read().block(Dimension::OVERWORLD, lower), Some(block));
        assert!(ctx.events().is_empty());
    }
}

#[test]
fn unready_missing_or_nonlower_support_keeps_upper_targetable() {
    for lower in [None, Some(0), Some(70), Some(11), Some(27)] {
        let mut authority = authority();
        let mut ctx = TickContext::harness(&mut authority, TickBudget::full());
        if let Some(block) = lower {
            observe(
                &mut ctx,
                Dimension::OVERWORLD,
                BlockPos::new(0, 64, 0),
                block,
            );
        }
        assert!(target_block(
            &ctx.read(),
            Dimension::OVERWORLD,
            BlockPos::new(0, 65, 0),
            70
        ));
        assert!(target_block(
            &ctx.read(),
            Dimension::OVERWORLD,
            BlockPos::new(0, -64, 0),
            70
        ));
        assert!(ctx.events().is_empty());
    }
}
