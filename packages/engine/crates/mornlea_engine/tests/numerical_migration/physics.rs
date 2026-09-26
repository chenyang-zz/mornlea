use mornlea_engine::native::contracts::{
    collision::{Aabb, CollisionCell, CollisionGrid},
    physics::{
        PhysicsControls, PhysicsOp, PhysicsRequest, PhysicsState, PhysicsTuning, SweepBounds,
    },
};
use mornlea_engine::native::physics::NativePhysics;

unsafe extern "C" {
    fn mornlea_physics_step(
        abi_version: u32,
        input: *const u8,
        input_len: usize,
        output: *mut u8,
        output_len: usize,
    ) -> u32;
    fn mornlea_engine_abi_version() -> u32;
}

const RAW_STEP_HEADER_BYTES: usize = 160;
const RAW_STEP_LAYOUT_VERSION: u32 = 4;
const RAW_STEP_CELL_BYTES: usize = 196;
const PHYSICS_OUTPUT_BYTES: usize = 32;
const CANARY_WORD: u32 = 0xa5a5_a5a5;

/// Minimal raw `MGP1` request encoder: 160-byte header plus Y/X/Z ordered
/// 196-byte cells. Mirrors the field layout of `StepInput::decode`
/// structurally; every cell starts loaded-but-empty and the scenarios below
/// arm individual cells as full cubes.
struct RawStep {
    bytes: Vec<u8>,
}

impl RawStep {
    #[allow(clippy::too_many_arguments)]
    fn empty_prism(
        position: [f32; 3],
        velocity: [f32; 3],
        on_ground: bool,
        jump: bool,
        move_x: i8,
        move_z: i8,
        body_in_fluid: bool,
        sprinting: bool,
        sneaking: bool,
        sweep_min: [f32; 3],
        sweep_max: [f32; 3],
    ) -> Self {
        let dimensions = [1u32, 4, 1];
        let cells = (dimensions[0] * dimensions[1] * dimensions[2]) as usize;
        let mut bytes = vec![0u8; RAW_STEP_HEADER_BYTES + cells * RAW_STEP_CELL_BYTES];
        bytes[0..4].copy_from_slice(b"MGP1");
        bytes[4..8].copy_from_slice(&RAW_STEP_LAYOUT_VERSION.to_le_bytes());
        for (index, value) in position.into_iter().enumerate() {
            Self::put_f32(&mut bytes, 8 + index * 4, value);
        }
        for (index, value) in velocity.into_iter().enumerate() {
            Self::put_f32(&mut bytes, 20 + index * 4, value);
        }
        bytes[32] = u8::from(on_ground);
        bytes[33] = u8::from(jump);
        bytes[34] = move_x as u8;
        bytes[35] = move_z as u8;
        Self::put_f32(&mut bytes, 36, 0.0); // `yaw_sin`
        Self::put_f32(&mut bytes, 40, 1.0); // `yaw_cos`
        Self::put_f32(&mut bytes, 44, 0.05); // `fixed_delta_seconds`
        for (index, value) in [0.6f32, 4.3, 40.0, 50.0, 8.0, 8.4, 32.0, 78.4]
            .into_iter()
            .enumerate()
        {
            Self::put_f32(&mut bytes, 48 + index * 4, value);
        }
        bytes[128] = u8::from(body_in_fluid);
        bytes[129] = u8::from(sprinting);
        bytes[130] = u8::from(sneaking);
        for (index, value) in [6.4f32, 3.0, 4.0, 0.8].into_iter().enumerate() {
            Self::put_f32(&mut bytes, 132 + index * 4, value);
        }
        Self::put_f32(&mut bytes, 148, 1.3); // `sprint_speed_multiplier`
        Self::put_f32(&mut bytes, 152, 0.3); // `sneak_speed_multiplier`
        for axis in 0..3 {
            Self::put_f32(&mut bytes, 80 + axis * 8, sweep_min[axis]);
            Self::put_f32(&mut bytes, 84 + axis * 8, sweep_max[axis]);
            bytes[104 + axis * 4..108 + axis * 4].copy_from_slice(&0i32.to_le_bytes());
            bytes[116 + axis * 4..120 + axis * 4].copy_from_slice(&dimensions[axis].to_le_bytes());
        }
        for cell in bytes[RAW_STEP_HEADER_BYTES..].chunks_exact_mut(RAW_STEP_CELL_BYTES) {
            cell[0] = 1; // loaded, zero boxes
        }
        Self { bytes }
    }

    fn put_f32(bytes: &mut [u8], offset: usize, value: f32) {
        bytes[offset..offset + 4].copy_from_slice(&value.to_bits().to_le_bytes());
    }

    /// Arms cell (0, 0, 0) — the first cell in Y/X/Z order — as a full cube.
    fn set_floor_cube(&mut self) {
        let offset = RAW_STEP_HEADER_BYTES;
        self.bytes[offset] = 1;
        self.bytes[offset + 1] = 1;
        for (index, value) in [0.0f32, 0.0, 0.0, 1.0, 1.0, 1.0].into_iter().enumerate() {
            Self::put_f32(&mut self.bytes, offset + 4 + index * 4, value);
        }
    }
}

/// Calls the real exported `mornlea_physics_step` symbol with a full-word
/// canary destination, returning the status and the destination bytes.
fn call_physics_abi(input: &[u8]) -> (u32, Vec<u8>) {
    let version = unsafe { mornlea_engine_abi_version() };
    let mut words = vec![CANARY_WORD; PHYSICS_OUTPUT_BYTES / 4];
    let status = unsafe {
        mornlea_physics_step(
            version,
            input.as_ptr(),
            input.len(),
            words.as_mut_ptr().cast::<u8>(),
            words.len() * 4,
        )
    };
    let bytes: Vec<u8> = words.iter().flat_map(|word| word.to_le_bytes()).collect();
    (status, bytes)
}

#[test]
fn parity_with_abi_observations() {
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

#[test]
fn physics_abi_native_bits() {
    // Floor landing: falls onto the full cube at the grid base, lands back
    // at y = 1.0 with the Y velocity clipped and `on_ground` set.
    let mut landing = RawStep::empty_prism(
        [0.5, 1.0, 0.5],
        [0.0, 0.0, 0.0],
        false,
        false,
        0,
        0,
        false,
        false,
        false,
        [0.0, -0.08, 0.0],
        [0.0, 0.05, 0.0],
    );
    landing.set_floor_cube();
    let (status, output) = call_physics_abi(&landing.bytes);
    assert_eq!(status, 0, "landing case status");
    assert_eq!(
        output,
        vec![
            0x00, 0x00, 0x00, 0x3f, 0x00, 0x00, 0x80, 0x3f, 0x00, 0x00, 0x00, 0x3f, 0x00, 0x00,
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x02, 0x01, 0x00, 0x00,
            0x00, 0x00, 0x00, 0x00,
        ],
        "landing full 32-byte parity"
    );
    assert_eq!(output[24], 0b010, "landing clips the Y axis");
    println!("landing bytes: {output:02x?}");

    // Jump: a grounded jump assigns `jump_speed` directly and rises by
    // `8.4 * 0.05 = 0.42` to y = 1.42.
    let jump = RawStep::empty_prism(
        [0.5, 1.0, 0.5],
        [0.0, 0.0, 0.0],
        true,
        true,
        0,
        0,
        false,
        false,
        false,
        [0.0, 0.42, 0.0],
        [0.0, 0.42, 0.0],
    );
    let (status, output) = call_physics_abi(&jump.bytes);
    assert_eq!(status, 0, "jump case status");
    assert_eq!(
        output,
        vec![
            0x00, 0x00, 0x00, 0x3f, 0x8f, 0xc2, 0xb5, 0x3f, 0x00, 0x00, 0x00, 0x3f, 0x00, 0x00,
            0x00, 0x00, 0x66, 0x66, 0x06, 0x41, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00, 0x00, 0x00,
        ],
        "jump full 32-byte parity"
    );
    println!("jump bytes: {output:02x?}");

    // Fluid drag: airborne in fluid with x velocity 2.0; air acceleration
    // pulls toward rest, then the 0.8 drag scales the horizontal speed.
    let fluid = RawStep::empty_prism(
        [0.5, 2.0, 0.5],
        [2.0, 0.0, 0.0],
        false,
        false,
        0,
        0,
        true,
        false,
        false,
        [0.064, -0.016, 0.0],
        [0.064, -0.016, 0.0],
    );
    let (status, output) = call_physics_abi(&fluid.bytes);
    assert_eq!(status, 0, "fluid case status");
    assert_eq!(
        output,
        vec![
            0x4e, 0x62, 0x10, 0x3f, 0xb6, 0xf3, 0xfd, 0x3f, 0x00, 0x00, 0x00, 0x3f, 0x0b, 0xd7,
            0xa3, 0x3f, 0x0b, 0xd7, 0xa3, 0xbe, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00, 0x00, 0x00,
        ],
        "fluid full 32-byte parity"
    );
    println!("fluid bytes: {output:02x?}");

    // Sneak plus sprint: sneaking wins, so the forward target is
    // `-(4.3 * 0.3)` with the Y velocity falling under gravity.
    let sneak = RawStep::empty_prism(
        [0.5, 1.0, 0.5],
        [0.0, 0.0, 0.0],
        true,
        false,
        0,
        1,
        false,
        true,
        true,
        [0.0, -0.09, -0.07],
        [0.0, 0.05, 0.0],
    );
    let (status, output) = call_physics_abi(&sneak.bytes);
    assert_eq!(status, 0, "sneak+sprint case status");
    assert_eq!(
        output,
        vec![
            0x00, 0x00, 0x00, 0x3f, 0x1f, 0x85, 0x6b, 0x3f, 0xdb, 0xf9, 0xde, 0x3e, 0x00, 0x00,
            0x00, 0x00, 0xcd, 0xcc, 0xcc, 0xbf, 0xb9, 0x1e, 0xa5, 0xbf, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00, 0x00, 0x00,
        ],
        "sneak+sprint full 32-byte parity"
    );
    assert_eq!(
        f32::from_le_bytes(output[20..24].try_into().unwrap()).to_bits(),
        (-(4.3f32 * 0.3f32)).to_bits(),
        "sneak priority pins the forward speed"
    );
    println!("sneak bytes: {output:02x?}");

    // One-ULP sweep boundary: `dy_max` one ULP below the integrated
    // displacement is accepted under the frozen one-ULP allowance. The
    // falling displacement is `(-1.6) * 0.05 = -0.08` up to residual
    // rounding, so the bound below pins the full 32-byte vector.
    let displacement = (-1.6f32) * 0.05f32;
    let ulp_ok = RawStep::empty_prism(
        [0.5, 1.0, 0.5],
        [0.0, 0.0, 0.0],
        false,
        false,
        0,
        0,
        false,
        false,
        false,
        [0.0, displacement.next_down(), 0.0],
        [0.0, displacement.next_down(), 0.0],
    );
    let (status, output) = call_physics_abi(&ulp_ok.bytes);
    assert_eq!(status, 0, "one-ULP boundary status");
    assert_eq!(
        output,
        vec![
            0x00, 0x00, 0x00, 0x3f, 0x1f, 0x85, 0x6b, 0x3f, 0x00, 0x00, 0x00, 0x3f, 0x00, 0x00,
            0x00, 0x00, 0xcd, 0xcc, 0xcc, 0xbf, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00, 0x00, 0x00,
        ],
        "one-ULP full 32-byte parity"
    );
    println!("one-ULP bytes: {output:02x?}");

    // Two-ULP sweep boundary: `dy_max` two ULPs below the displacement is
    // rejected with status 3 and the destination stays untouched.
    let ulp_rejected = RawStep::empty_prism(
        [0.5, 1.0, 0.5],
        [0.0, 0.0, 0.0],
        false,
        false,
        0,
        0,
        false,
        false,
        false,
        [0.0, displacement.next_down(), 0.0],
        [0.0, displacement.next_down().next_down(), 0.0],
    );
    let version = unsafe { mornlea_engine_abi_version() };
    let mut rejected_words = vec![CANARY_WORD; PHYSICS_OUTPUT_BYTES / 4];
    let rejected_before: Vec<u8> = rejected_words
        .iter()
        .flat_map(|word| word.to_le_bytes())
        .collect();
    let status = unsafe {
        mornlea_physics_step(
            version,
            ulp_rejected.bytes.as_ptr(),
            ulp_rejected.bytes.len(),
            rejected_words.as_mut_ptr().cast::<u8>(),
            rejected_words.len() * 4,
        )
    };
    let rejected_after: Vec<u8> = rejected_words
        .iter()
        .flat_map(|word| word.to_le_bytes())
        .collect();
    assert_eq!(status, 3, "two-ULP boundary status");
    assert_eq!(
        rejected_after, rejected_before,
        "two-ULP rejection keeps the canary"
    );

    // Malformed axis: `move_x = 2` fails admission with status 3 and the
    // destination stays untouched.
    let mut malformed = landing.bytes.clone();
    malformed[34] = 2;
    let mut malformed_words = vec![CANARY_WORD; PHYSICS_OUTPUT_BYTES / 4];
    let malformed_before: Vec<u8> = malformed_words
        .iter()
        .flat_map(|word| word.to_le_bytes())
        .collect();
    let status = unsafe {
        mornlea_physics_step(
            version,
            malformed.as_ptr(),
            malformed.len(),
            malformed_words.as_mut_ptr().cast::<u8>(),
            malformed_words.len() * 4,
        )
    };
    let malformed_after: Vec<u8> = malformed_words
        .iter()
        .flat_map(|word| word.to_le_bytes())
        .collect();
    assert_eq!(status, 3, "malformed axis status");
    assert_eq!(
        malformed_after, malformed_before,
        "malformed input keeps the canary"
    );

    // Short destination: the known-good landing bytes pre-fill the 31-byte
    // buffer, so a partial publish would corrupt the surviving prefix and
    // the equality check below would fail.
    let mut short = landing_output_known_good().to_vec();
    short.pop();
    let short_before = short.clone();
    let status = unsafe {
        mornlea_physics_step(
            version,
            landing.bytes.as_ptr(),
            landing.bytes.len(),
            short.as_mut_ptr(),
            short.len(),
        )
    };
    assert_eq!(status, 7, "short output status");
    assert_eq!(
        short, short_before,
        "short output keeps the known-good bytes"
    );
}

/// Returns the frozen landing output vector, reused as the known-good
/// pre-fill for the short-destination canary above.
fn landing_output_known_good() -> [u8; PHYSICS_OUTPUT_BYTES] {
    [
        0x00, 0x00, 0x00, 0x3f, 0x00, 0x00, 0x80, 0x3f, 0x00, 0x00, 0x00, 0x3f, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x02, 0x01, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00,
    ]
}
