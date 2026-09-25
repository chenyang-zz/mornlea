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

    struct PhysicsGridWrapper<'a> {
        grid: crate::native::contracts::collision::CollisionGrid<'a>,
    }

    impl<'a> crate::collision::CollisionCells for PhysicsGridWrapper<'a> {
        fn loaded(&self, position: [i32; 3]) -> bool {
            let idx = self.index(position);
            self.grid.cells()[idx].loaded()
        }
        fn count(&self, position: [i32; 3]) -> usize {
            let idx = self.index(position);
            self.grid.cells()[idx].used() as usize
        }
        fn bounds(&self, position: [i32; 3], index: usize) -> crate::collision::Bounds {
            let idx = self.index(position);
            let aabb = self.grid.cells()[idx].boxes()[index];
            crate::collision::Bounds {
                minimum: [
                    aabb.minimum[0] + position[0] as f32,
                    aabb.minimum[1] + position[1] as f32,
                    aabb.minimum[2] + position[2] as f32,
                ],
                maximum: [
                    aabb.maximum[0] + position[0] as f32,
                    aabb.maximum[1] + position[1] as f32,
                    aabb.maximum[2] + position[2] as f32,
                ],
            }
        }
    }

    impl<'a> PhysicsGridWrapper<'a> {
        fn index(&self, position: [i32; 3]) -> usize {
            let origin = self.grid.origin();
            let dims = self.grid.dimensions();
            let x = (position[0] - origin[0]) as usize;
            let y = (position[1] - origin[1]) as usize;
            let z = (position[2] - origin[2]) as usize;
            (y * dims[0] as usize + x) * dims[2] as usize + z
        }
    }

    let wrapper = PhysicsGridWrapper { grid: request.grid };
    let collision_input = crate::collision::CollisionInputData {
        cells: &wrapper,
        position: request.state.position,
        displacement,
        began_grounded: request.state.on_ground,
        step_height: request.tuning.step_height,
    };

    let collision_result = crate::collision::resolve_move_and_step(&collision_input);

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
