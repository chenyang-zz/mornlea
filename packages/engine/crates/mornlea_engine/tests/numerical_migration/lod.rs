use mornlea_engine::native::contracts::KernelError;
use mornlea_engine::native::contracts::world::{
    LodFace, LodOp, LodQuad, LodRequest, LodScratch, LodStep, Materials, WorldgenParams,
};
use mornlea_engine::native::lod::NativeLod;

/// Stage ceiling of the caller-owned scratch, used only to size destinations.
const STAGE_QUADS: usize = 3136;

const SEA_LEVEL_Y: i32 = 64;

/// Material table and permutation matching the Go-side ABI observation
/// header: distinct entries 1..=15 in wire order and identity permutation.
/// `gated_fluid` applies the fluid-disabled gate by reusing the air id for the
/// water entry.
fn release_abi_params(gated_fluid: bool) -> WorldgenParams {
    let materials = Materials {
        air: 1,
        stone: 2,
        dirt: 3,
        grass: 4,
        bedrock: 5,
        snow: 6,
        sand: 7,
        clay: 8,
        gravel: 9,
        iron_ore: 10,
        coal_ore: 11,
        oak_log: 12,
        leaves: 13,
        water: if gated_fluid { 1 } else { 14 },
        short_grass: 15,
    };
    let mut perm = [0u8; 512];
    for (i, entry) in perm.iter_mut().enumerate() {
        *entry = (i % 256) as u8;
    }
    WorldgenParams::try_new(0, materials, perm).unwrap()
}

/// Ordered quad fields in the observation tuple order.
type QuadFields = (i32, i32, i32, u16, u16, LodFace, u16, u8);

fn fields(quad: &LodQuad) -> QuadFields {
    (
        quad.x(),
        quad.z(),
        quad.y(),
        quad.w(),
        quad.d(),
        quad.face(),
        quad.material(),
        quad.shade(),
    )
}

/// Encodes one quad into the 20-byte little-endian ABI record so parity is
/// checked on the exact wire bytes the Go oracle digests.
fn encode_quad(quad: &LodQuad) -> [u8; 20] {
    let mut record = [0u8; 20];
    record[0..4].copy_from_slice(&quad.x().to_le_bytes());
    record[4..8].copy_from_slice(&quad.z().to_le_bytes());
    record[8..12].copy_from_slice(&quad.y().to_le_bytes());
    record[12..14].copy_from_slice(&quad.w().to_le_bytes());
    record[14..16].copy_from_slice(&quad.d().to_le_bytes());
    record[16] = quad.face() as u8;
    record[17..19].copy_from_slice(&quad.material().to_le_bytes());
    record[19] = quad.shade();
    record
}

fn encode_shell(quads: &[LodQuad]) -> Vec<u8> {
    let mut out = Vec::with_capacity(quads.len() * 20);
    for quad in quads {
        out.extend_from_slice(&encode_quad(quad));
    }
    out
}

/// FNV-1a 64-bit digest over raw bytes, matching the Go oracle's `hash/fnv`
/// New64a over the same stream.
fn fnv1a64(bytes: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

/// FNV-1a 64-bit digest over the encoded shell, matching the Go oracle's
/// `hash/fnv` New64a over the same bytes.
fn shell_digest(quads: &[LodQuad]) -> u64 {
    fnv1a64(&encode_shell(quads))
}

fn build(gated_fluid: bool, tile: [i32; 2], step: LodStep) -> Vec<LodQuad> {
    let params = release_abi_params(gated_fluid);
    let mut scratch = LodScratch::try_new().unwrap();
    let mut dst = [LodQuad::default(); STAGE_QUADS];
    let count = NativeLod
        .build(
            &LodRequest {
                params: &params,
                tile,
                step,
            },
            &mut scratch,
            &mut dst,
        )
        .unwrap();
    dst[..count].to_vec()
}

/// Pinned release-ABI observations shared with `TestLodShell` in the Go
/// runtime oracle: identical case inputs, counts, stream digests and spot
/// fields. Literals are captured from the Go-oracle observation run.
struct Observation {
    name: &'static str,
    tile: [i32; 2],
    step: LodStep,
    gated_fluid: bool,
    want_count: usize,
    want_digest: u64,
    want_first: QuadFields,
    want_last: QuadFields,
}

fn observations() -> Vec<Observation> {
    vec![
        Observation {
            name: "step two at tile (0,0)",
            tile: [0, 0],
            step: LodStep::Two,
            gated_fluid: false,
            want_count: 1284,
            want_digest: 0xd849_30ff_4cee_9f89,
            want_first: (0, 0, 64, 2, 2, LodFace::Top, 4, 255),
            want_last: (62, 61, 77, 2, 1, LodFace::PosZ, 4, 204),
        },
        Observation {
            name: "step four at tile (-3,2)",
            tile: [-3, 2],
            step: LodStep::Four,
            gated_fluid: false,
            want_count: 112,
            want_digest: 0xaddb_a620_c8cb_5bd8,
            want_first: (-192, 128, 64, 64, 24, LodFace::Top, 14, 255),
            want_last: (-132, 168, 66, 4, 2, LodFace::NegZ, 4, 204),
        },
        Observation {
            name: "step eight at tile (7,-5)",
            tile: [7, -5],
            step: LodStep::Eight,
            gated_fluid: false,
            want_count: 82,
            want_digest: 0x2371_4f96_9c54_b4f7,
            want_first: (448, -320, 71, 8, 8, LodFace::Top, 4, 255),
            want_last: (504, -289, 65, 8, 1, LodFace::PosZ, 4, 204),
        },
        Observation {
            name: "sea clamp gated off at tile (-3,2)",
            tile: [-3, 2],
            step: LodStep::Four,
            gated_fluid: true,
            want_count: 548,
            want_digest: 0x4f33_d86b_c6a1_6f8b,
            want_first: (-192, 128, 47, 4, 8, LodFace::Top, 7, 255),
            want_last: (-132, 168, 66, 4, 2, LodFace::NegZ, 4, 204),
        },
        Observation {
            name: "uniform single top at tile (-4,1)",
            tile: [-4, 1],
            step: LodStep::Four,
            gated_fluid: false,
            want_count: 1,
            want_digest: 0x94ed_4971_f2c9_89c9,
            want_first: (-256, 64, 64, 64, 64, LodFace::Top, 14, 255),
            want_last: (-256, 64, 64, 64, 64, LodFace::Top, 14, 255),
        },
    ]
}

#[test]
fn release_abi_params_match_wire_order() {
    let materials = release_abi_params(false).materials();
    assert_eq!(
        materials.as_slice(),
        [1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15]
    );
    let perm = *release_abi_params(false).perm();
    for (i, entry) in perm.iter().enumerate() {
        assert_eq!(*entry, (i % 256) as u8);
    }
    let gated = release_abi_params(true).materials();
    assert_eq!(gated.air, 1);
    assert_eq!(gated.water, 1);
}

#[test]
fn ordered_quads_match_go_abi_observations() {
    for case in observations() {
        let quads = build(case.gated_fluid, case.tile, case.step);
        assert_eq!(quads.len(), case.want_count, "{}", case.name);
        assert_eq!(shell_digest(&quads), case.want_digest, "{}", case.name);
        assert_eq!(fields(&quads[0]), case.want_first, "{}", case.name);
        assert_eq!(
            fields(&quads[quads.len() - 1]),
            case.want_last,
            "{}",
            case.name
        );
    }
}

#[test]
fn skirt_case_matches_go_abi_observations() {
    // The first skirt of the step-eight case, pinned field by field against
    // the Go-oracle observation run.
    let quads = build(false, [7, -5], LodStep::Eight);
    let skirt = quads
        .iter()
        .find(|quad| quad.face() != LodFace::Top)
        .expect("the observation tile must contain a height step");
    assert_eq!(
        fields(skirt),
        (448, -320, 69, 8, 3, LodFace::NegX, 4, 153),
        "first skirt fields must match the observation pin"
    );
}

#[test]
fn sea_clamp_case_matches_go_abi_observations() {
    // The clamped water surface of the basin tile and the unclamped terrain
    // top covering the same window once the fluid gate is off, pinned against
    // the Go-oracle observation run.
    let clamped = build(false, [-3, 2], LodStep::Four);
    let water_top = clamped
        .iter()
        .find(|quad| {
            quad.face() == LodFace::Top && quad.y() == SEA_LEVEL_Y && quad.material() == 14
        })
        .expect("the basin tile must contain a clamped water surface");
    assert_eq!(
        fields(water_top),
        (-192, 128, 64, 64, 24, LodFace::Top, 14, 255),
        "clamped water top fields must match the observation pin"
    );

    let unclamped = build(true, [-3, 2], LodStep::Four);
    let terrain_top = unclamped
        .iter()
        .find(|quad| {
            quad.face() == LodFace::Top
                && quad.x() <= water_top.x()
                && water_top.x() < quad.x() + i32::from(quad.w())
                && quad.z() <= water_top.z()
                && water_top.z() < quad.z() + i32::from(quad.d())
        })
        .expect("the gate-off shell must cover the same window");
    assert!(terrain_top.y() < SEA_LEVEL_Y);
    assert_eq!(
        fields(terrain_top),
        (-192, 128, 47, 4, 8, LodFace::Top, 7, 255),
        "unclamped terrain top fields must match the observation pin"
    );
}

#[test]
fn ordered_quads_are_deterministic() {
    for case in observations() {
        let first = build(case.gated_fluid, case.tile, case.step);
        let second = build(case.gated_fluid, case.tile, case.step);
        assert_eq!(first, second, "{}", case.name);
    }
}

#[test]
fn required_counts_match_go_abi_observations() {
    // The count reported on a short destination is exactly the count observed
    // on the Go side for the same request; the stage ceiling never leaks into
    // the required-size report.
    for case in observations() {
        let params = release_abi_params(case.gated_fluid);
        let mut scratch = LodScratch::try_new().unwrap();
        let mut dst = [LodQuad::default(); STAGE_QUADS];
        let result = NativeLod.build(
            &LodRequest {
                params: &params,
                tile: case.tile,
                step: case.step,
            },
            &mut scratch,
            &mut dst[..case.want_count - 1],
        );
        assert_eq!(
            result,
            Err(KernelError::OutputTooSmall {
                needed: case.want_count,
                available: case.want_count - 1
            }),
            "{}",
            case.name
        );
    }
}

unsafe extern "C" {
    fn mornlea_lod_shell(
        abi_version: u32,
        input: *const u8,
        input_len: usize,
        output: *mut u8,
        output_capacity: usize,
        output_len: *mut usize,
    ) -> u32;
    fn mornlea_engine_abi_version() -> u32;
}

const ABI_CANARY: u8 = 0xA5;

/// ABI status codes mirrored from the engine exports: only success and
/// output-overflow appear in this adapter test.
const LOD_STATUS_OK: u32 = 0;
const LOD_STATUS_OUTPUT_OVERFLOW: u32 = 7;

/// LOD shell ABI framing: the shared 566-byte `MGW1` worldgen header plus
/// tile_x/tile_z i32, columns u32 (fixed 64) and step u32.
const LOD_ABI_HEADER_BYTES: usize = 566;
const LOD_ABI_INPUT_BYTES: usize = 582;
const LOD_ABI_TILE_COLUMNS: u32 = 64;
const LOD_ABI_QUAD_BYTES: usize = 20;

/// Builds the raw 582-byte `MGW1` LOD shell request: layout 3, material
/// table ids 1..=15 in wire order, identity permutation, then the tile
/// coordinates with columns fixed at 64 and the requested step.
fn lod_abi_input(tile: [i32; 2], step: u32) -> Vec<u8> {
    let mut bytes = vec![0u8; LOD_ABI_INPUT_BYTES];
    bytes[0..4].copy_from_slice(b"MGW1");
    bytes[4..8].copy_from_slice(&3u32.to_le_bytes());
    bytes[8..16].copy_from_slice(&0i64.to_le_bytes());
    bytes[16..20].copy_from_slice(&(-64i32).to_le_bytes());
    bytes[20..24].copy_from_slice(&320i32.to_le_bytes());
    for (index, id) in (1u16..=15).enumerate() {
        bytes[24 + index * 2..26 + index * 2].copy_from_slice(&id.to_le_bytes());
    }
    for (index, entry) in bytes[54..LOD_ABI_HEADER_BYTES].iter_mut().enumerate() {
        *entry = (index & 255) as u8;
    }
    bytes[LOD_ABI_HEADER_BYTES..LOD_ABI_HEADER_BYTES + 4].copy_from_slice(&tile[0].to_le_bytes());
    bytes[LOD_ABI_HEADER_BYTES + 4..LOD_ABI_HEADER_BYTES + 8]
        .copy_from_slice(&tile[1].to_le_bytes());
    bytes[LOD_ABI_HEADER_BYTES + 8..LOD_ABI_HEADER_BYTES + 12]
        .copy_from_slice(&LOD_ABI_TILE_COLUMNS.to_le_bytes());
    bytes[LOD_ABI_HEADER_BYTES + 12..LOD_ABI_HEADER_BYTES + 16]
        .copy_from_slice(&step.to_le_bytes());
    bytes
}

/// Calls the real exported `mornlea_lod_shell` symbol with a canary-filled
/// destination, returning the status, the metadata word, and the destination
/// bytes. The caller keeps no aliasing: input, output and metadata are
/// disjoint by construction.
fn call_lod_abi(input: &[u8], output_len: usize) -> (u32, usize, Vec<u8>) {
    let version = unsafe { mornlea_engine_abi_version() };
    let mut output = vec![ABI_CANARY; output_len];
    let mut written = usize::MAX;
    let status = unsafe {
        mornlea_lod_shell(
            version,
            input.as_ptr(),
            input.len(),
            output.as_mut_ptr(),
            output.len(),
            &mut written,
        )
    };
    (status, written, output)
}

struct AbiCase {
    name: &'static str,
    tile: [i32; 2],
    step: LodStep,
    raw_step: u32,
    want_count: usize,
    want_digest: u64,
}

fn abi_cases() -> Vec<AbiCase> {
    vec![
        AbiCase {
            name: "step two at tile (0,0)",
            tile: [0, 0],
            step: LodStep::Two,
            raw_step: 2,
            want_count: 1284,
            want_digest: 0xd849_30ff_4cee_9f89,
        },
        AbiCase {
            name: "step four at tile (-3,2)",
            tile: [-3, 2],
            step: LodStep::Four,
            raw_step: 4,
            want_count: 112,
            want_digest: 0xaddb_a620_c8cb_5bd8,
        },
        AbiCase {
            name: "step eight at tile (7,-5)",
            tile: [7, -5],
            step: LodStep::Eight,
            raw_step: 8,
            want_count: 82,
            want_digest: 0x2371_4f96_9c54_b4f7,
        },
    ]
}

#[test]
fn lod_abi_exact_needed() {
    // Each adapter case pins the full ordered 20-byte stream through the real
    // exported symbol: byte-identical to the typed provider's encoded quads,
    // with the frozen count and stream digest. The exact-needed retry per
    // step probes `needed - 1` bytes first (status 7, canary untouched,
    // metadata exactly `needed`) and retries with exact capacity.
    for case in abi_cases() {
        let input = lod_abi_input(case.tile, case.raw_step);
        let expected = encode_shell(&build(false, case.tile, case.step));
        let needed = case.want_count * LOD_ABI_QUAD_BYTES;
        assert_eq!(needed, expected.len(), "{} staged bytes", case.name);
        assert_eq!(fnv1a64(&expected), case.want_digest, "{}", case.name);

        let (status, written, output) = call_lod_abi(&input, needed);
        assert_eq!(status, LOD_STATUS_OK, "{} status", case.name);
        assert_eq!(written, needed, "{} metadata", case.name);
        assert_eq!(output, expected, "{} full ordered quad bytes", case.name);

        let short_before = vec![ABI_CANARY; needed - 1];
        let (status, written, short) = call_lod_abi(&input, needed - 1);
        assert_eq!(
            status, LOD_STATUS_OUTPUT_OVERFLOW,
            "{} short status",
            case.name
        );
        assert_eq!(written, needed, "{} short metadata", case.name);
        assert_eq!(short, short_before, "{} short payload untouched", case.name);

        let (status, written, retry) = call_lod_abi(&input, needed);
        assert_eq!(status, LOD_STATUS_OK, "{} retry status", case.name);
        assert_eq!(written, needed, "{} retry metadata", case.name);
        assert_eq!(retry, expected, "{} retry bytes", case.name);
    }
}
