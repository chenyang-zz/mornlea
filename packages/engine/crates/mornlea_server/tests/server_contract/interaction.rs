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

#[test]
fn source_look_components_match_go_float_bits() {
    use mornlea_server::core::interaction::look_direction;
    // Literals come from Go math.Float32bits after the source operation order.
    for (yaw, pitch, bits) in [
        (0.0, 0.0, [0x80000000, 0, 0xbf800000]),
        (0.1, 0.2, [0xbdc8621f, 0x3e4b6ff9, 0xbf79a4c4]),
        (1.234, -0.456, [0xbf58ede4, 0xbee176ea, 0xbe97e8e0]),
        (
            f32::from_bits(0x40490fdb),
            f32::from_bits(0x3fc90fdb),
            [0xa789aded, 0x3f800000, 0xb33bbd2e],
        ),
    ] {
        assert_eq!(look_direction(yaw, pitch).map(f32::to_bits), bits);
    }
}

#[test]
fn source_ray_normalization_matches_go_without_large_vector_overflow() {
    use mornlea_server::core::interaction::normalized_direction;
    for (vector, bits) in [
        ([1.0, 2.0, 3.0], [0x3e88d677, 0x3f08d677, 0x3f4d41b2]),
        ([3.0, 4.0, 0.0], [0x3f19999a, 0x3f4ccccd, 0]),
        ([1e30; 3], [0x3f13cd3a; 3]),
        ([-2.8, 0.123, 9.5], [0xbe90bce4, 0x3c4b75c8, 0x3f758996]),
    ] {
        let original = vector;
        assert_eq!(
            normalized_direction(vector).unwrap().map(f32::to_bits),
            bits
        );
        assert_eq!(vector, original);
    }
}

#[test]
fn ray_normalization_refuses_nonfinite_and_source_tiny_boundary() {
    use mornlea_server::core::interaction::normalized_direction;
    for vector in [
        [0.0; 3],
        [1e-7, 0.0, 0.0],
        [1e-6, 0.0, 0.0],
        [f32::NAN, 1.0, 0.0],
        [0.0, f32::INFINITY, 0.0],
        [0.0, 0.0, f32::NEG_INFINITY],
    ] {
        assert_eq!(normalized_direction(vector), None);
    }
    let above = f32::from_bits(1e-6f32.to_bits() + 1);
    assert!(normalized_direction([above, 0.0, 0.0]).is_some());
}
