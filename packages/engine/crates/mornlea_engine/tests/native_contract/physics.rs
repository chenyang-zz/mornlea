use mornlea_engine::native::contracts::{
    KernelError,
    collision::{Aabb, CollisionCell, CollisionGrid},
    physics::{
        PhysicsControls, PhysicsOp, PhysicsRequest, PhysicsState, PhysicsTuning, SweepBounds,
    },
};
use mornlea_engine::native::physics::NativePhysics;

fn base_tuning() -> PhysicsTuning {
    PhysicsTuning {
        fixed_delta_seconds: 0.05,
        step_height: 0.6,
        walk_speed: 4.3,
        ground_acceleration: 40.0,
        ground_deceleration: 50.0,
        air_acceleration: 8.0,
        jump_speed: 8.4,
        gravity: 32.0,
        terminal_fall_speed: 78.4,
        fluid_gravity: 6.4,
        fluid_sink_speed: 3.0,
        fluid_ascend_speed: 4.0,
        fluid_horizontal_drag: 0.8,
        sprint_speed_multiplier: 1.3,
        sneak_speed_multiplier: 0.3,
    }
}

#[test]
fn floor_landing_clips_vertical_velocity() {
    let mut cells = vec![
        CollisionCell::try_new(
            true,
            [Aabb {
                minimum: [0.0; 3],
                maximum: [0.0; 3]
            }; 8],
            0
        )
        .unwrap();
        1000
    ];
    let floor_cell = CollisionCell::try_new(
        true,
        [
            Aabb {
                minimum: [0.0, 0.0, 0.0],
                maximum: [1.0, 1.0, 1.0],
            },
            Aabb {
                minimum: [0.0; 3],
                maximum: [0.0; 3],
            },
            Aabb {
                minimum: [0.0; 3],
                maximum: [0.0; 3],
            },
            Aabb {
                minimum: [0.0; 3],
                maximum: [0.0; 3],
            },
            Aabb {
                minimum: [0.0; 3],
                maximum: [0.0; 3],
            },
            Aabb {
                minimum: [0.0; 3],
                maximum: [0.0; 3],
            },
            Aabb {
                minimum: [0.0; 3],
                maximum: [0.0; 3],
            },
            Aabb {
                minimum: [0.0; 3],
                maximum: [0.0; 3],
            },
        ],
        1,
    )
    .unwrap();
    cells[555] = floor_cell;
    let grid = CollisionGrid::try_new([-5, -5, -5], [10, 10, 10], &cells).unwrap();

    let request = PhysicsRequest {
        state: PhysicsState {
            position: [0.5, 1.1, 0.5],
            velocity: [0.0, -10.0, 0.0],
            on_ground: false,
        },
        controls: PhysicsControls {
            move_x: 0,
            move_z: 0,
            jump: false,
            yaw_sin: 0.0,
            yaw_cos: 1.0,
            body_in_fluid: false,
            sprinting: false,
            sneaking: false,
        },
        tuning: base_tuning(),
        sweep: SweepBounds {
            minimum: [-1.0, -1.0, -1.0],
            maximum: [1.0, 1.0, 1.0],
        },
        grid,
    };

    let op = NativePhysics;
    let result = op.step(&request).unwrap();

    assert!(result.state.on_ground);
    assert_eq!(result.state.velocity[1], 0.0);
    assert!(result.clipped[1]);
}

#[test]
fn jump_speed_applied() {
    let cells = vec![
        CollisionCell::try_new(
            true,
            [Aabb {
                minimum: [0.0; 3],
                maximum: [0.0; 3]
            }; 8],
            0
        )
        .unwrap();
        1000
    ];
    let grid = CollisionGrid::try_new([-5, -5, -5], [10, 10, 10], &cells).unwrap();

    let request = PhysicsRequest {
        state: PhysicsState {
            position: [0.5, 1.0, 0.5],
            velocity: [10.0, 0.0, 0.0],
            on_ground: true,
        },
        controls: PhysicsControls {
            move_x: 0,
            move_z: 0,
            jump: true,
            yaw_sin: 0.0,
            yaw_cos: 1.0,
            body_in_fluid: false,
            sprinting: false,
            sneaking: false,
        },
        tuning: base_tuning(),
        sweep: SweepBounds {
            minimum: [-1.0, -1.0, -1.0],
            maximum: [1.0, 1.0, 1.0],
        },
        grid,
    };

    let op = NativePhysics;
    let result = op.step(&request).unwrap();
    assert_eq!(result.state.velocity[1].to_bits(), 8.4f32.to_bits());
}

#[test]
fn fluid_drag_and_gravity() {
    let cells = vec![
        CollisionCell::try_new(
            true,
            [Aabb {
                minimum: [0.0; 3],
                maximum: [0.0; 3]
            }; 8],
            0
        )
        .unwrap();
        1000
    ];
    let grid = CollisionGrid::try_new([-5, -5, -5], [10, 10, 10], &cells).unwrap();

    let request = PhysicsRequest {
        state: PhysicsState {
            position: [0.5, 1.0, 0.5],
            velocity: [10.0, 0.0, 0.0],
            on_ground: false,
        },
        controls: PhysicsControls {
            move_x: 0,
            move_z: 0,
            jump: false,
            yaw_sin: 0.0,
            yaw_cos: 1.0,
            body_in_fluid: true,
            sprinting: false,
            sneaking: false,
        },
        tuning: base_tuning(),
        sweep: SweepBounds {
            minimum: [-2.0, -2.0, -2.0],
            maximum: [2.0, 2.0, 2.0],
        },
        grid,
    };

    let op = NativePhysics;
    let result = op.step(&request).unwrap();

    assert_eq!(
        result.state.velocity[1].to_bits(),
        (-6.4f32 * 0.05).to_bits()
    );
}

#[test]
fn sneak_overrides_sprint() {
    let cells = vec![
        CollisionCell::try_new(
            true,
            [Aabb {
                minimum: [0.0; 3],
                maximum: [0.0; 3]
            }; 8],
            0
        )
        .unwrap();
        1000
    ];
    let grid = CollisionGrid::try_new([-5, -5, -5], [10, 10, 10], &cells).unwrap();

    let request = PhysicsRequest {
        state: PhysicsState {
            position: [0.5, 1.0, 0.5],
            velocity: [10.0, 0.0, 0.0],
            on_ground: true,
        },
        controls: PhysicsControls {
            move_x: 0,
            move_z: 1,
            jump: false,
            yaw_sin: 0.0,
            yaw_cos: 1.0,
            body_in_fluid: false,
            sprinting: true,
            sneaking: true,
        },
        tuning: base_tuning(),
        sweep: SweepBounds {
            minimum: [-2.0, -2.0, -2.0],
            maximum: [2.0, 2.0, 2.0],
        },
        grid,
    };

    let op = NativePhysics;
    let result = op.step(&request).unwrap();

    assert!(result.state.velocity[2] < 0.0);
}

#[test]
fn sweep_bounds_ulp_allowances() {
    let cells = vec![
        CollisionCell::try_new(
            true,
            [Aabb {
                minimum: [0.0; 3],
                maximum: [0.0; 3]
            }; 8],
            0
        )
        .unwrap();
        1000
    ];
    let grid = CollisionGrid::try_new([-5, -5, -5], [10, 10, 10], &cells).unwrap();

    let mut request = PhysicsRequest {
        state: PhysicsState {
            position: [0.5, 1.0, 0.5],
            velocity: [0.0, 0.0, 0.0],
            on_ground: false,
        },
        controls: PhysicsControls {
            move_x: 0,
            move_z: 0,
            jump: false,
            yaw_sin: 0.0,
            yaw_cos: 1.0,
            body_in_fluid: false,
            sprinting: false,
            sneaking: false,
        },
        tuning: base_tuning(),
        sweep: SweepBounds {
            minimum: [-10.0, -10.0, -10.0],
            maximum: [10.0, 10.0, 10.0],
        },
        grid,
    };

    let op = NativePhysics;

    // figure out exact displacement
    let _res = op.step(&request).unwrap();
    let exact_dy = (0.0_f32 - 32.0_f32 * 0.05_f32).max(-78.4_f32) * 0.05_f32;

    // Set 1 ULP outside
    request.sweep.minimum[1] = exact_dy.next_up();
    request.sweep.maximum[1] = 10.0;

    let res1 = op.step(&request);
    assert!(
        res1.is_ok(),
        "1 ULP should be ok, but got {:?} for min={}, exact_dy={}",
        res1,
        request.sweep.minimum[1],
        exact_dy
    );

    // 2 ULPs should fail
    request.sweep.minimum[1] = exact_dy.next_up().next_up();
    assert_eq!(op.step(&request), Err(KernelError::DisplacementOutOfBounds));
}
#[test]
fn input_validation_failures() {
    let cells = vec![
        CollisionCell::try_new(
            true,
            [Aabb {
                minimum: [0.0; 3],
                maximum: [0.0; 3]
            }; 8],
            0
        )
        .unwrap();
        1000
    ];
    let grid = CollisionGrid::try_new([-5, -5, -5], [10, 10, 10], &cells).unwrap();

    let base_req = PhysicsRequest {
        state: PhysicsState {
            position: [0.5, 1.0, 0.5],
            velocity: [10.0, 0.0, 0.0],
            on_ground: false,
        },
        controls: PhysicsControls {
            move_x: 0,
            move_z: 0,
            jump: false,
            yaw_sin: 0.0,
            yaw_cos: 1.0,
            body_in_fluid: false,
            sprinting: false,
            sneaking: false,
        },
        tuning: base_tuning(),
        sweep: SweepBounds {
            minimum: [-1.0, -1.0, -1.0],
            maximum: [1.0, 1.0, 1.0],
        },
        grid,
    };

    let op = NativePhysics;

    let mut req = base_req;
    req.sweep.minimum[1] = 2.0; // max < min
    assert_eq!(op.step(&req), Err(KernelError::InvalidInput));

    let mut req = base_req;
    req.controls.move_x = 2;
    assert_eq!(op.step(&req), Err(KernelError::InvalidInput));

    let mut req = base_req;
    req.controls.yaw_sin = f32::NAN;
    assert_eq!(op.step(&req), Err(KernelError::InvalidInput));
}

#[test]
fn negative_dt_tuning_accepted() {
    let cells = vec![
        CollisionCell::try_new(
            true,
            [Aabb {
                minimum: [0.0; 3],
                maximum: [0.0; 3]
            }; 8],
            0
        )
        .unwrap();
        1000
    ];
    let grid = CollisionGrid::try_new([-5, -5, -5], [10, 10, 10], &cells).unwrap();

    let mut request = PhysicsRequest {
        state: PhysicsState {
            position: [0.5, 1.0, 0.5],
            velocity: [10.0, 0.0, 0.0],
            on_ground: false,
        },
        controls: PhysicsControls {
            move_x: 0,
            move_z: 0,
            jump: false,
            yaw_sin: 0.0,
            yaw_cos: 1.0,
            body_in_fluid: false,
            sprinting: false,
            sneaking: false,
        },
        tuning: base_tuning(),
        sweep: SweepBounds {
            // dt = -0.05, gravity 32. vel[1] = 0 - 32 * (-0.05) = 1.6
            // dy = 1.6 * -0.05 = -0.08
            minimum: [-1.0, -1.0, -1.0],
            maximum: [1.0, 1.0, 1.0],
        },
        grid,
    };
    request.tuning.fixed_delta_seconds = -0.05;

    let op = NativePhysics;

    let expected_disp = (0.0 - 32.0_f32 * 0.05_f32).max(-78.4_f32) * 0.05_f32;
    request.sweep.minimum[1] = expected_disp.next_up();
    let res = op.step(&request);
    assert!(res.is_ok(), "{:?}", res);
}
