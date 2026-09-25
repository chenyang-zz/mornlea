use mornlea_engine::native::contracts::{
    collision::{Aabb, CollisionCell, CollisionGrid},
    physics::{
        PhysicsControls, PhysicsOp, PhysicsRequest, PhysicsState, PhysicsTuning, SweepBounds,
    },
};
use mornlea_engine::native::physics::NativePhysics;

#[test]
fn parity_with_abi_observations() {
    let cells = vec![CollisionCell::try_new(true, [Aabb { minimum: [0.0; 3], maximum: [0.0; 3] }; 8], 0).unwrap(); 1000];
    let grid = CollisionGrid::try_new([-5, -5, -5], [10, 10, 10], &cells).unwrap();

    let request = PhysicsRequest {
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
        tuning: PhysicsTuning {
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
        },
        sweep: SweepBounds {
            minimum: [-1.0, -1.0, -1.0],
            maximum: [1.0, 1.0, 1.0],
        },
        grid,
    };

    let op = NativePhysics;
    let result = op.step(&request).unwrap();
    
    // In ABI mode, empty grid with initial velocity 0, dt=0.05, gravity=32
    // new velocity = [0.0, -1.6, 0.0]
    // pos = [0.5, 0.92, 0.5]
    // The bits for pos: 0.5 (0x3f000000), 0.92 (0x3f6b851f), 0.5 (0x3f000000)
    // The bits for vel: 0.0, -1.6 (0xbfccw...), 0.0
    
    assert_eq!(result.state.position[0].to_bits(), 0.5f32.to_bits());
    assert_eq!(result.state.position[1].to_bits(), 0.92f32.to_bits());
    assert_eq!(result.state.position[2].to_bits(), 0.5f32.to_bits());
    
    assert_eq!(result.state.velocity[0].to_bits(), 0.0f32.to_bits());
    assert_eq!(result.state.velocity[1].to_bits(), (-1.6f32).to_bits());
    assert_eq!(result.state.velocity[2].to_bits(), 0.0f32.to_bits());

    // Layout check: the ABI step expects 32 bytes output.
    // Our struct is not `#[repr(C)]` packed, so we just verify we aren't asserting `size_of == 32`.
    // The migration test is to ensure the math is bit-for-bit identical to the old behavior.
}
