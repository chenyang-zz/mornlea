use std::cell::Cell;

use crate::native::contracts::{
    KernelError,
    physics::{PhysicsOp, PhysicsRequest, PhysicsResult, PhysicsState},
};
use crate::step::{IntegrationParams, integrate};

pub struct NativePhysics;

impl PhysicsOp for NativePhysics {
    fn step(&self, request: &PhysicsRequest<'_>) -> Result<PhysicsResult, KernelError> {
        step_physics(request)
    }
}

pub fn step_physics(request: &PhysicsRequest<'_>) -> Result<PhysicsResult, KernelError> {
    if !(-1..=1).contains(&request.controls.move_x) || !(-1..=1).contains(&request.controls.move_z)
    {
        return Err(KernelError::InvalidInput);
    }

    let floats = [
        request.state.position[0],
        request.state.position[1],
        request.state.position[2],
        request.state.velocity[0],
        request.state.velocity[1],
        request.state.velocity[2],
        request.controls.yaw_sin,
        request.controls.yaw_cos,
        request.tuning.fixed_delta_seconds,
        request.tuning.step_height,
        request.tuning.walk_speed,
        request.tuning.ground_acceleration,
        request.tuning.ground_deceleration,
        request.tuning.air_acceleration,
        request.tuning.jump_speed,
        request.tuning.gravity,
        request.tuning.terminal_fall_speed,
        request.tuning.fluid_gravity,
        request.tuning.fluid_sink_speed,
        request.tuning.fluid_ascend_speed,
        request.tuning.fluid_horizontal_drag,
        request.tuning.sprint_speed_multiplier,
        request.tuning.sneak_speed_multiplier,
        request.sweep.minimum[0],
        request.sweep.minimum[1],
        request.sweep.minimum[2],
        request.sweep.maximum[0],
        request.sweep.maximum[1],
        request.sweep.maximum[2],
    ];
    for f in floats {
        if !f.is_finite() {
            return Err(KernelError::InvalidInput);
        }
    }

    for i in 0..3 {
        if request.sweep.minimum[i] > request.sweep.maximum[i] {
            return Err(KernelError::InvalidInput);
        }
    }

    let params = IntegrationParams {
        velocity: request.state.velocity,
        on_ground: request.state.on_ground,
        jump: request.controls.jump,
        move_x: request.controls.move_x,
        move_z: request.controls.move_z,
        yaw_sin: request.controls.yaw_sin,
        yaw_cos: request.controls.yaw_cos,
        fixed_delta_seconds: request.tuning.fixed_delta_seconds,
        walk_speed: request.tuning.walk_speed,
        ground_acceleration: request.tuning.ground_acceleration,
        ground_deceleration: request.tuning.ground_deceleration,
        air_acceleration: request.tuning.air_acceleration,
        jump_speed: request.tuning.jump_speed,
        gravity: request.tuning.gravity,
        terminal_fall_speed: request.tuning.terminal_fall_speed,
        body_in_fluid: request.controls.body_in_fluid,
        sprinting: request.controls.sprinting,
        sneaking: request.controls.sneaking,
        fluid_gravity: request.tuning.fluid_gravity,
        fluid_sink_speed: request.tuning.fluid_sink_speed,
        fluid_ascend_speed: request.tuning.fluid_ascend_speed,
        fluid_horizontal_drag: request.tuning.fluid_horizontal_drag,
        sprint_speed_multiplier: request.tuning.sprint_speed_multiplier,
        sneak_speed_multiplier: request.tuning.sneak_speed_multiplier,
    };

    let (mut velocity, displacement) = integrate(&params);

    for (i, &disp) in displacement.iter().enumerate() {
        let min = request.sweep.minimum[i].next_down();
        let max = request.sweep.maximum[i].next_up();
        if !(min <= disp && disp <= max) {
            return Err(KernelError::DisplacementOutOfBounds);
        }
    }

    // The raw ABI keeps its historical overflow behavior. Native callers must
    // receive a typed failure before derived nonfinite state can be published.
    if velocity
        .iter()
        .chain(&displacement)
        .any(|value| !value.is_finite())
    {
        return Err(KernelError::InvalidInput);
    }

    let wrapper = PhysicsGridWrapper {
        grid: request.grid,
        failed: Cell::new(false),
    };
    let collision_input = crate::collision::CollisionInputData {
        cells: &wrapper,
        position: request.state.position,
        displacement,
        began_grounded: request.state.on_ground,
        step_height: request.tuning.step_height,
    };

    let collision_result = crate::collision::resolve_move_and_step(&collision_input);
    if wrapper.failed.get()
        || collision_result
            .position
            .iter()
            .any(|value| !value.is_finite())
    {
        return Err(KernelError::InvalidInput);
    }

    let mut clipped = [false; 3];
    for (i, &clip) in collision_result.clipped.iter().enumerate() {
        if clip {
            clipped[i] = true;
            velocity[i] = 0.0;
        }
    }

    Ok(PhysicsResult {
        state: PhysicsState {
            position: collision_result.position,
            velocity,
            on_ground: collision_result.on_ground,
        },
        clipped,
        used_step: collision_result.used_step,
        hit_unknown: collision_result.hit_unknown,
    })
}

/// Actual-scan admission preserves physics's weaker prism policy: unused step
/// alternatives need no cells. A rejection poisons this call so later solver
/// phases do no reads, and the staged collision result is never published.
struct PhysicsGridWrapper<'a> {
    grid: crate::native::contracts::collision::CollisionGrid<'a>,
    failed: Cell<bool>,
}

impl crate::collision::CollisionCells for PhysicsGridWrapper<'_> {
    fn scan_is_admitted(&self, minimum: [f32; 3], maximum: [f32; 3]) -> bool {
        if self.failed.get() {
            return false;
        }
        let origin = self.grid.origin();
        let dimensions = self.grid.dimensions();
        let admitted = (0..3).all(|axis| {
            if !minimum[axis].is_finite()
                || !maximum[axis].is_finite()
                || minimum[axis] > maximum[axis]
            {
                return false;
            }
            // Widen before flooring/range checks; f32 casts to i32 saturate and
            // would otherwise let an out-of-world scan alias a boundary cell.
            let low = f64::from(minimum[axis]).floor();
            let high = f64::from(maximum[axis]).floor();
            if low < f64::from(i32::MIN) || high > f64::from(i32::MAX) {
                return false;
            }
            let grid_low = i64::from(origin[axis]);
            let grid_high = grid_low + i64::from(dimensions[axis]) - 1;
            low as i64 >= grid_low && high as i64 <= grid_high
        });
        if !admitted {
            self.failed.set(true);
        }
        admitted
    }

    fn loaded(&self, position: [i32; 3]) -> bool {
        self.cell(position).is_some_and(|cell| cell.loaded())
    }
    fn count(&self, position: [i32; 3]) -> usize {
        self.cell(position).map_or(0, |cell| cell.used() as usize)
    }
    fn bounds(&self, position: [i32; 3], index: usize) -> crate::collision::Bounds {
        let Some(aabb) = self.cell(position).and_then(|cell| cell.boxes().get(index)) else {
            self.failed.set(true);
            return crate::collision::Bounds::default();
        };
        crate::collision::Bounds {
            minimum: std::array::from_fn(|axis| aabb.minimum[axis] + position[axis] as f32),
            maximum: std::array::from_fn(|axis| aabb.maximum[axis] + position[axis] as f32),
        }
    }
}

impl PhysicsGridWrapper<'_> {
    fn cell(
        &self,
        position: [i32; 3],
    ) -> Option<&crate::native::contracts::collision::CollisionCell> {
        if self.failed.get() {
            return None;
        }
        let origin = self.grid.origin();
        let dims = self.grid.dimensions();
        let relative = std::array::from_fn::<_, 3, _>(|axis| {
            i64::from(position[axis]) - i64::from(origin[axis])
        });
        if (0..3).any(|axis| relative[axis] < 0 || relative[axis] >= i64::from(dims[axis])) {
            self.failed.set(true);
            return None;
        }
        let index = ((relative[1] as usize * dims[0] as usize) + relative[0] as usize)
            * dims[2] as usize
            + relative[2] as usize;
        let cell = self.grid.cells().get(index);
        if cell.is_none() {
            self.failed.set(true);
        }
        cell
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::collision::CollisionCells;
    use crate::native::contracts::collision::{CollisionCell, CollisionGrid};

    #[test]
    fn physics_scan_endpoints_and_defensive_reads_use_wide_arithmetic() {
        let cells = [CollisionCell::default(); 128];
        for origin_x in [i32::MIN, i32::MAX - 127] {
            let grid = CollisionGrid::try_new([origin_x, 0, 0], [128, 1, 1], &cells).unwrap();
            let wrapper = PhysicsGridWrapper {
                grid,
                failed: Cell::new(false),
            };
            let inside_scan = [origin_x as f32, 0.0, 0.0];
            assert!(wrapper.scan_is_admitted(inside_scan, inside_scan));
            assert!(wrapper.cell([origin_x + 1, 0, 0]).is_some());
            let outside = if origin_x == i32::MIN {
                i32::MAX
            } else {
                i32::MIN
            };
            let outside_scan = [outside as f32, 0.0, 0.0];
            assert!(!wrapper.scan_is_admitted(outside_scan, outside_scan));
            assert!(wrapper.failed.get());
            assert!(!wrapper.scan_is_admitted(inside_scan, inside_scan));
            assert!(wrapper.cell([outside, 0, 0]).is_none());

            let defensive = PhysicsGridWrapper {
                grid,
                failed: Cell::new(false),
            };
            assert!(defensive.cell([outside, 0, 0]).is_none());
            assert!(defensive.failed.get());
        }
    }
}
