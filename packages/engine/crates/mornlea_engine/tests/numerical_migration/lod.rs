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

/// FNV-1a 64-bit digest over the encoded shell, matching the Go oracle's
/// `hash/fnv` New64a over the same bytes.
fn shell_digest(quads: &[LodQuad]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in encode_shell(quads) {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
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
