//! Copied nonplayer speed tuning over one borrowed raw foot-cell observation.

use super::actor_placement::{PlacementWorld, snow_cell};
use super::contracts::ServerError;
use mornlea_domain::Dimension;
use mornlea_engine::native::contracts::physics::PhysicsTuning;

/// Applies source thick-Snow speed semantics without retaining or changing actor state.
/// Quiet actors require neither valid geometry nor a world observation.
pub fn apply_snow_slowdown(
    world: &impl PlacementWorld,
    dimension: Dimension,
    position: [f32; 3],
    on_ground: bool,
    move_intent: [i8; 2],
    tuning: PhysicsTuning,
) -> Result<PhysicsTuning, ServerError> {
    if !on_ground || move_intent == [0, 0] {
        return Ok(tuning);
    }
    let cell = snow_cell(position).map_err(|_| ServerError::InvalidInput { field: "actor" })?;
    if !(-64..320).contains(&cell.y()) {
        return Ok(tuning);
    }
    let mut result = tuning;
    if matches!(world.block_at(dimension, cell), Some(87 | 88)) {
        result.walk_speed *= 0.7_f32;
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::contracts::ChunkKey;
    use mornlea_domain::BlockPos;
    use std::cell::{Cell, RefCell};

    struct CountingWorld {
        response: Cell<Option<u16>>,
        trace: RefCell<Vec<(Dimension, BlockPos)>>,
    }

    impl CountingWorld {
        fn new(response: Option<u16>) -> Self {
            Self {
                response: Cell::new(response),
                trace: RefCell::new(Vec::new()),
            }
        }
    }

    impl PlacementWorld for CountingWorld {
        fn ready_revision(&self, _key: ChunkKey) -> Option<u64> {
            panic!("scalar slowdown must not query readiness")
        }

        fn block_at(&self, dimension: Dimension, pos: BlockPos) -> Option<u16> {
            self.trace.borrow_mut().push((dimension, pos));
            self.response.get()
        }

        fn ready_column_height(&self, _dimension: Dimension, _x: i32, _z: i32) -> Option<i32> {
            panic!("scalar slowdown must not query column height")
        }
    }

    fn tuning() -> PhysicsTuning {
        PhysicsTuning {
            fixed_delta_seconds: 0.05,
            step_height: 0.6,
            walk_speed: 4.3,
            ground_acceleration: 25.0,
            ground_deceleration: 30.0,
            air_acceleration: 5.0,
            jump_speed: 8.0,
            gravity: 24.0,
            terminal_fall_speed: 56.0,
            fluid_gravity: 4.0,
            fluid_sink_speed: 3.0,
            fluid_ascend_speed: 4.0,
            fluid_horizontal_drag: 2.0,
            sprint_speed_multiplier: 1.3,
            sneak_speed_multiplier: 0.3,
        }
    }

    fn bits(value: PhysicsTuning) -> [u32; 15] {
        [
            value.fixed_delta_seconds.to_bits(),
            value.step_height.to_bits(),
            value.walk_speed.to_bits(),
            value.ground_acceleration.to_bits(),
            value.ground_deceleration.to_bits(),
            value.air_acceleration.to_bits(),
            value.jump_speed.to_bits(),
            value.gravity.to_bits(),
            value.terminal_fall_speed.to_bits(),
            value.fluid_gravity.to_bits(),
            value.fluid_sink_speed.to_bits(),
            value.fluid_ascend_speed.to_bits(),
            value.fluid_horizontal_drag.to_bits(),
            value.sprint_speed_multiplier.to_bits(),
            value.sneak_speed_multiplier.to_bits(),
        ]
    }

    fn assert_tuning(result: PhysicsTuning, input: PhysicsTuning, before: [u32; 15], walk: u32) {
        let mut expected = before;
        expected[2] = walk;
        assert_eq!(bits(result), expected);
        assert_eq!(bits(input), before);
    }

    fn assert_read(world: &CountingWorld, dimension: Dimension, cell: [i32; 3]) {
        assert_eq!(
            *world.trace.borrow(),
            [(dimension, BlockPos::new(cell[0], cell[1], cell[2]))]
        );
    }

    #[test]
    fn slowdown_raw_tiers_preserve_tuning() {
        let input = tuning();
        let before = bits(input);
        for response in [
            None,
            Some(0),
            Some(1),
            Some(3),
            Some(35),
            Some(85),
            Some(86),
            Some(87),
            Some(88),
            Some(65535),
        ] {
            let world = CountingWorld::new(response);
            let result = apply_snow_slowdown(
                &world,
                Dimension::OVERWORLD,
                [2.5, 64.0, 3.5],
                true,
                [1, 0],
                input,
            )
            .unwrap();
            let walk = if matches!(response, Some(87 | 88)) {
                0x4040a3d7
            } else {
                0x4089999a
            };
            assert_tuning(result, input, before, walk);
            assert_read(&world, Dimension::OVERWORLD, [2, 64, 3]);
        }
        let input = PhysicsTuning {
            walk_speed: 17.25,
            ..input
        };
        let before = bits(input);
        assert_eq!(before[2], 0x418a0000);
        for response in [87, 88] {
            let world = CountingWorld::new(Some(response));
            let result = apply_snow_slowdown(
                &world,
                Dimension::OVERWORLD,
                [2.5, 64.0, 3.5],
                true,
                [1, 0],
                input,
            )
            .unwrap();
            assert_tuning(result, input, before, 0x41413333);
            assert_read(&world, Dimension::OVERWORLD, [2, 64, 3]);
        }
    }

    #[test]
    fn slowdown_quiet_without_geometry_or_reads() {
        let input = tuning();
        let before = bits(input);
        let mut positions = vec![[2.5, 64.0, 3.5], [2147483648.0, 64.0, 3.5]];
        for axis in 0..3 {
            for value in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY, f32::MAX] {
                let mut position = [2.5, 64.0, 3.5];
                position[axis] = value;
                positions.push(position);
            }
        }
        for position in positions {
            for (grounded, intent) in [
                (false, [0, 0]),
                (false, [1, 0]),
                (false, [0, -1]),
                (false, [1, 1]),
                (true, [0, 0]),
            ] {
                let world = CountingWorld::new(Some(87));
                let result = apply_snow_slowdown(
                    &world,
                    Dimension::OVERWORLD,
                    position,
                    grounded,
                    intent,
                    input,
                )
                .unwrap();
                assert_tuning(result, input, before, 0x4089999a);
                assert!(world.trace.borrow().is_empty());
            }
        }
    }

    #[test]
    fn slowdown_checked_geometry_refuses_without_reads() {
        let input = tuning();
        let before = bits(input);
        let mut positions = Vec::new();
        for axis in 0..3 {
            for value in [
                f32::NAN,
                f32::INFINITY,
                f32::NEG_INFINITY,
                f32::MAX,
                -f32::MAX,
                2147483648.0,
                -2147483904.0,
            ] {
                let mut position = [2.5, 64.0, 3.5];
                position[axis] = value;
                positions.push(position);
            }
        }
        positions.extend([
            [f32::NAN, 320.0, f32::INFINITY],
            [f32::NAN, -65.0, f32::NAN],
            [2.5, f32::NAN, 3.5],
        ]);
        for position in positions {
            let world = CountingWorld::new(Some(87));
            assert_eq!(
                apply_snow_slowdown(&world, Dimension::OVERWORLD, position, true, [1, 0], input),
                Err(ServerError::InvalidInput { field: "actor" })
            );
            assert!(world.trace.borrow().is_empty());
            assert_eq!(bits(input), before);
        }
    }

    #[test]
    fn slowdown_foot_cell_dimension_and_height() {
        let input = tuning();
        let before = bits(input);
        for (dimension, position, cell) in [
            (Dimension::OVERWORLD, [2.5, 64.0, 3.5], [2, 64, 3]),
            (Dimension::DEPTHS, [-1.25, 64.75, -16.125], [-2, 64, -17]),
            (
                Dimension::OVERWORLD,
                [-2147483648.0, 64.0, 2147483520.0],
                [i32::MIN, 64, 2147483520],
            ),
            (Dimension::OVERWORLD, [2.5, -64.0, 3.5], [2, -64, 3]),
            (Dimension::DEPTHS, [2.5, 319.99, 3.5], [2, 319, 3]),
        ] {
            let world = CountingWorld::new(Some(87));
            let result =
                apply_snow_slowdown(&world, dimension, position, true, [0, -1], input).unwrap();
            assert_tuning(result, input, before, 0x4040a3d7);
            assert_read(&world, dimension, cell);
        }
        for y in [-64.125, 320.0, 2147483520.0, -2147483648.0] {
            let world = CountingWorld::new(Some(87));
            let result = apply_snow_slowdown(
                &world,
                Dimension::OVERWORLD,
                [2.5, y, 3.5],
                true,
                [0, -1],
                input,
            )
            .unwrap();
            assert_tuning(result, input, before, 0x4089999a);
            assert!(world.trace.borrow().is_empty());
        }
        let world = CountingWorld::new(Some(87));
        assert_eq!(
            apply_snow_slowdown(
                &world,
                Dimension::OVERWORLD,
                [2.5, 320.0, f32::NAN],
                true,
                [0, -1],
                input
            ),
            Err(ServerError::InvalidInput { field: "actor" })
        );
        assert!(world.trace.borrow().is_empty());
        assert_eq!(bits(input), before);
    }

    #[test]
    fn slowdown_current_unknown_observations() {
        let world = CountingWorld::new(None);
        let input = tuning();
        let before = bits(input);
        for (index, (response, walk)) in [
            (None, 0x4089999a),
            (Some(87), 0x4040a3d7),
            (Some(86), 0x4089999a),
            (Some(88), 0x4040a3d7),
            (Some(0), 0x4089999a),
        ]
        .into_iter()
        .enumerate()
        {
            world.response.set(response);
            let result = apply_snow_slowdown(
                &world,
                Dimension::OVERWORLD,
                [2.5, 64.0, 3.5],
                true,
                [1, -1],
                input,
            )
            .unwrap();
            assert_tuning(result, input, before, walk);
            let trace = world.trace.borrow();
            assert_eq!(trace.len(), index + 1);
            assert!(
                trace
                    .iter()
                    .all(|entry| *entry == (Dimension::OVERWORLD, BlockPos::new(2, 64, 3)))
            );
        }
    }

    #[test]
    fn slowdown_independent_consumer_values() {
        // Callable examples qualify copied values, independently of native consumer wiring.
        type ConsumerExample = (&'static str, Dimension, [f32; 3], [i8; 2], [i32; 3]);
        let examples: [ConsumerExample; 3] = [
            (
                "companion",
                Dimension::OVERWORLD,
                [2.5, 64.0, 3.5],
                [1, 0],
                [2, 64, 3],
            ),
            (
                "commonwalker-hurler",
                Dimension::DEPTHS,
                [-1.25, 64.75, -16.125],
                [0, -1],
                [-2, 64, -17],
            ),
            (
                "passive",
                Dimension::OVERWORLD,
                [8.5, 1.0, 8.5],
                [-1, 1],
                [8, 1, 8],
            ),
        ];
        let input = tuning();
        let before = bits(input);
        for (label, dimension, position, intent, cell) in examples {
            for (response, walk) in [(88, 0x4040a3d7), (0, 0x4089999a)] {
                let world = CountingWorld::new(Some(response));
                let result =
                    apply_snow_slowdown(&world, dimension, position, true, intent, input).unwrap();
                assert_tuning(result, input, before, walk);
                assert_read(&world, dimension, cell);
                assert_eq!(bits(input), before, "{label}");
            }
        }
    }
}
