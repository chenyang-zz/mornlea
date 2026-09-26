use mornlea_engine::native::collision::resolve_collision;
use mornlea_engine::native::contracts::collision::{
    CollisionCell, CollisionGrid, CollisionRequest,
};

unsafe extern "C" {
    fn mornlea_collision_resolve(
        abi_version: u32,
        input: *const u8,
        input_len: usize,
        output: *mut u8,
        output_len: usize,
    ) -> u32;
    fn mornlea_engine_abi_version() -> u32;
}

const HEADER_BYTES: usize = 64;
const CELL_BYTES: usize = 196;
const OUTPUT_BYTES: usize = 16;
const CANARY: u8 = 0xA5;

/// Minimal raw `MGC1` request encoder: header plus Y/X/Z ordered cells.
/// Every cell starts loaded-but-empty; individual cells are then armed as
/// full cubes or unknown cells by the scenarios below.
struct RawCollision {
    bytes: Vec<u8>,
    origin: [i32; 3],
    dimensions: [u32; 3],
}

impl RawCollision {
    fn named(
        position: [f32; 3],
        displacement: [f32; 3],
        began_grounded: bool,
        step_height: f32,
        origin: [i32; 3],
        dimensions: [u32; 3],
    ) -> Self {
        let cells = dimensions
            .iter()
            .map(|value| *value as usize)
            .product::<usize>();
        let mut bytes = vec![0_u8; HEADER_BYTES + cells * CELL_BYTES];
        bytes[0..4].copy_from_slice(b"MGC1");
        bytes[4..8].copy_from_slice(&1_u32.to_le_bytes());
        for (index, value) in position.into_iter().enumerate() {
            bytes[8 + index * 4..12 + index * 4].copy_from_slice(&value.to_bits().to_le_bytes());
        }
        for (index, value) in displacement.into_iter().enumerate() {
            bytes[20 + index * 4..24 + index * 4].copy_from_slice(&value.to_bits().to_le_bytes());
        }
        bytes[32] = u8::from(began_grounded);
        bytes[36..40].copy_from_slice(&step_height.to_bits().to_le_bytes());
        for (index, value) in origin.into_iter().enumerate() {
            bytes[40 + index * 4..44 + index * 4].copy_from_slice(&value.to_le_bytes());
        }
        for (index, value) in dimensions.into_iter().enumerate() {
            bytes[52 + index * 4..56 + index * 4].copy_from_slice(&value.to_le_bytes());
        }
        for cell in bytes[HEADER_BYTES..].chunks_exact_mut(CELL_BYTES) {
            cell[0] = 1;
        }
        Self {
            bytes,
            origin,
            dimensions,
        }
    }

    fn cell_offset(&self, position: [i32; 3]) -> usize {
        let x = (position[0] - self.origin[0]) as usize;
        let y = (position[1] - self.origin[1]) as usize;
        let z = (position[2] - self.origin[2]) as usize;
        let dx = self.dimensions[0] as usize;
        let dz = self.dimensions[2] as usize;
        HEADER_BYTES + ((y * dx + x) * dz + z) * CELL_BYTES
    }

    fn put_f32(&mut self, offset: usize, value: f32) {
        self.bytes[offset..offset + 4].copy_from_slice(&value.to_bits().to_le_bytes());
    }

    fn set_full_cube(&mut self, position: [i32; 3]) {
        let offset = self.cell_offset(position);
        self.bytes[offset] = 1;
        self.bytes[offset + 1] = 1;
        for (index, value) in [0.0_f32, 0.0, 0.0, 1.0, 1.0, 1.0].into_iter().enumerate() {
            self.put_f32(offset + 4 + index * 4, value);
        }
    }

    fn set_unknown(&mut self, position: [i32; 3]) {
        let offset = self.cell_offset(position);
        self.bytes[offset] = 0;
        self.bytes[offset + 1] = 0;
    }
}

/// Calls the real exported `mornlea_collision_resolve` symbol with a
/// canary-filled destination, returning the status and the destination bytes.
fn call_abi(input: &[u8], output_len: usize) -> (u32, Vec<u8>) {
    let version = unsafe { mornlea_engine_abi_version() };
    let mut output = vec![CANARY; output_len];
    let status = unsafe {
        mornlea_collision_resolve(
            version,
            input.as_ptr(),
            input.len(),
            output.as_mut_ptr(),
            output.len(),
        )
    };
    (status, output)
}

fn read_f32(output: &[u8], offset: usize) -> f32 {
    f32::from_bits(u32::from_le_bytes(
        output[offset..offset + 4].try_into().unwrap(),
    ))
}

#[test]
fn parity_test() {
    let cells = [CollisionCell::default(); 4];
    let grid = CollisionGrid::try_new([0, -1, 0], [1, 4, 1], &cells).unwrap();

    let req = CollisionRequest {
        position: [0.5, 0.5, 0.5],
        displacement: [0.0, -0.5, 0.0],
        began_grounded: false,
        step_height: 0.6,
        grid,
    };

    let res = resolve_collision(&req).unwrap();
    assert_eq!(res.position[1].to_bits(), 0.0f32.to_bits());
    assert!(res.hit_unknown);
}

#[test]
fn collision_abi_native_bits() {
    // Loaded floor: falls onto the full cube at the grid base and lands on it.
    let mut floor = RawCollision::named(
        [0.5, 1.2, 0.5],
        [0.0, -0.5, 0.0],
        false,
        0.6,
        [0, 0, 0],
        [2, 4, 1],
    );
    floor.set_full_cube([0, 0, 0]);
    let (status, output) = call_abi(&floor.bytes, OUTPUT_BYTES);
    assert_eq!(status, 0, "floor case status");
    assert_eq!(
        output,
        vec![
            0x00, 0x00, 0x00, 0x3f, 0x00, 0x00, 0x80, 0x3f, 0x00, 0x00, 0x00, 0x3f, 0x02, 0x01,
            0x00, 0x00
        ],
        "floor full 16-byte parity"
    );
    assert_eq!(read_f32(&output, 4).to_bits(), 1.0_f32.to_bits());
    assert_eq!(output[12] & 0b010, 0b010, "floor clips the Y axis");
    assert_eq!(output[13], 1, "floor ends on the ground");
    assert_eq!(output[15], 0, "floor sees no unknown cell");
    println!("floor bytes: {output:02x?}");

    // Loaded wall: slides into the full cube on the +X side and clips there.
    let mut wall = RawCollision::named(
        [0.5, 1.0, 0.5],
        [0.5, 0.0, 0.0],
        true,
        0.6,
        [0, 0, 0],
        [2, 4, 1],
    );
    wall.set_full_cube([0, 0, 0]);
    wall.set_full_cube([1, 1, 0]);
    let (status, output) = call_abi(&wall.bytes, OUTPUT_BYTES);
    assert_eq!(status, 0, "wall case status");
    assert_eq!(
        output,
        vec![
            0x33, 0x33, 0x33, 0x3f, 0x00, 0x00, 0x80, 0x3f, 0x00, 0x00, 0x00, 0x3f, 0x01, 0x01,
            0x00, 0x00
        ],
        "wall full 16-byte parity"
    );
    assert_eq!(output[12] & 0b001, 0b001, "wall clips the X axis");
    assert_eq!(output[15], 0, "wall sees no unknown cell");
    println!("wall bytes: {output:02x?}");

    // Unknown cell: the same wall cell unloaded, so the move reports `hit_unknown`.
    let mut unknown = RawCollision::named(
        [0.5, 1.0, 0.5],
        [0.5, 0.0, 0.0],
        true,
        0.6,
        [0, 0, 0],
        [2, 4, 1],
    );
    unknown.set_full_cube([0, 0, 0]);
    unknown.set_unknown([1, 1, 0]);
    let (status, output) = call_abi(&unknown.bytes, OUTPUT_BYTES);
    assert_eq!(status, 0, "unknown case status");
    assert_eq!(
        output,
        vec![
            0x33, 0x33, 0x33, 0x3f, 0x00, 0x00, 0x80, 0x3f, 0x00, 0x00, 0x00, 0x3f, 0x01, 0x01,
            0x00, 0x01
        ],
        "unknown full 16-byte parity"
    );
    assert_eq!(output[15], 1, "unknown cell sets the flag byte");
    println!("unknown bytes: {output:02x?}");

    // Short destination: a 15-byte output is rejected and left untouched.
    let (status, output) = call_abi(&floor.bytes, OUTPUT_BYTES - 1);
    assert_eq!(status, 7, "short output status");
    assert_eq!(
        output,
        vec![CANARY; OUTPUT_BYTES - 1],
        "short output keeps the canary"
    );

    // Rejected input: a corrupt magic value reports status 3 without touching output.
    let mut rejected = floor.bytes.clone();
    rejected[0] = b'X';
    let (status, output) = call_abi(&rejected, OUTPUT_BYTES);
    assert_eq!(status, 3, "rejected input status");
    assert_eq!(
        output,
        vec![CANARY; OUTPUT_BYTES],
        "rejected input keeps the canary"
    );
}
