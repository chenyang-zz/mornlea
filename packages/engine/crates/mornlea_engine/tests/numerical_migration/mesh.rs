//! Mesh geometry migration parity: the typed provider must reproduce the raw
//! ABI's packed quad stream.
//!
//! The fixtures mirror the Go oracle scenes in
//! `packages/tools/cmd/runtime-oracle/kernel_mesh_test.go` field by field, and
//! the counts and FNV-1a digests are the release-ABI observations captured
//! from running `TestKernelMesh` against the engine dylib. Comparing packed
//! words proves the typed lane stages the identical per-quad bits.

use mornlea_engine::native::contracts::{
    MeshModel, MeshOp, MeshQuad, MeshRegistry, MeshRegistryEntry, MeshScratch, MeshView,
};
use mornlea_engine::native::mesh::{NativeMesh, try_new_registry};

const AIR: u16 = 0;
const BARRIER: u16 = 1;
const STONE: u16 = 2;
const WHEAT: u16 = 20;
const STANDING_TORCH: u16 = 71;
const WALL_TORCH_POS_X: u16 = 72;
const BED: u16 = 76;

const WHEAT_MATERIAL: u16 = 31;
const TORCH_MATERIAL: u16 = 90;
const BED_TOP_MATERIAL: u16 = 700;
const BED_SIDE_MATERIAL: u16 = 91;
const TORCH_WALL_TOP_NEAR_RAW: u8 = 8;
const TORCH_WALL_TOP_FAR_RAW: u8 = 13;
const BED_TOP_RAW: u8 = 8;

const BLOCKS: usize = 27 * 4096;
/// Typed staging capacity; the dense scene stays inside it.
const STAGE_QUADS: usize = 40960;

/// Pinned release-ABI observations shared with the Go `TestKernelMesh`:
/// identical scene inputs, counts and FNV-1a digests over the packed words.
const FACES_COUNT: usize = 6;
const FACES_DIGEST: u64 = 0x105f_552a_d3c4_31f5;
const ORDER_COUNT: usize = 16;
const ORDER_DIGEST: u64 = 0x7f01_74fc_3fdc_ee12;
const COMBINED_COUNT: usize = 8;
const DENSE_COUNT: usize = 32768;
const DENSE_DIGEST: u64 = 0x99aa_a0c3_8150_5725;

/// Compact ordered quad fields: position, face, back flag, material and
/// corner heights.
type QuadFields = (u8, u8, u8, u8, bool, u16, [u8; 4]);

fn entry(id: u16, opaque: bool, material: [u16; 6], model: MeshModel) -> MeshRegistryEntry {
    MeshRegistryEntry {
        id,
        opaque,
        emission: 0,
        material,
        fluid_height: 0,
        light_attenuation: 0,
        block_top_raw: 0,
        model,
    }
}

fn visibility(count: usize, visible: &[(usize, usize)]) -> Vec<u64> {
    let words_per_row = count.div_ceil(64);
    let mut words = vec![0_u64; count * words_per_row];
    for &(row, column) in visible {
        words[row * words_per_row + column / 64] |= 1_u64 << (column % 64);
    }
    words
}

fn registry(entries: &[MeshRegistryEntry], visible: &[(usize, usize)]) -> MeshRegistry {
    try_new_registry(entries, &visibility(entries.len(), visible), AIR, BARRIER).unwrap()
}

/// Owned section fixture backing one borrowed [`MeshView`].
struct Section {
    blocks: Box<[u16; BLOCKS]>,
    heights_present: Box<[bool; 9]>,
    heights: Box<[[i16; 256]; 9]>,
}

impl Section {
    fn air() -> Self {
        Self {
            blocks: Box::new([AIR; BLOCKS]),
            heights_present: Box::new([false; 9]),
            heights: Box::new([[0; 256]; 9]),
        }
    }

    fn set(&mut self, x: i32, y: i32, z: i32, id: u16) {
        self.blocks[cell(x, y, z)] = id;
    }

    fn fill(&mut self, id: u16) {
        self.blocks.fill(id);
    }

    fn view<'a>(&'a self, registry: &'a MeshRegistry) -> MeshView<'a> {
        MeshView {
            blocks: &self.blocks,
            heights_present: &self.heights_present,
            heights: &self.heights,
            section_origin_y: 0,
            registry,
        }
    }
}

fn cell(x: i32, y: i32, z: i32) -> usize {
    let shifted = |value: i32| value + 16;
    let section = (shifted(x) >> 4) * 3 + (shifted(y) >> 4);
    let section = (section * 3 + (shifted(z) >> 4)) as usize;
    let offset = ((shifted(y) & 15) << 8) | ((shifted(z) & 15) << 4) | (shifted(x) & 15);
    section * 4096 + offset as usize
}

/// The isolated six-face scene: one opaque stone visible toward air.
fn faces_scene() -> (Section, MeshRegistry) {
    let registry = registry(
        &[
            entry(AIR, false, [0; 6], MeshModel::Default),
            entry(BARRIER, true, [1; 6], MeshModel::Default),
            entry(STONE, true, [10, 11, 12, 13, 14, 15], MeshModel::Default),
        ],
        &[(2, 0)],
    );
    let mut section = Section::air();
    section.set(0, 0, 0, STONE);
    (section, registry)
}

/// The plant/torch/bed ordering scene: one plant, one standing torch, one wall
/// torch and one bed, with no axial faces anywhere.
fn order_scene() -> (Section, MeshRegistry) {
    let registry = registry(
        &[
            entry(AIR, false, [0; 6], MeshModel::Default),
            entry(BARRIER, true, [1; 6], MeshModel::Default),
            entry(WHEAT, false, [WHEAT_MATERIAL; 6], MeshModel::Default),
            entry(
                STANDING_TORCH,
                false,
                [TORCH_MATERIAL; 6],
                MeshModel::StandingTorch,
            ),
            entry(
                WALL_TORCH_POS_X,
                false,
                [TORCH_MATERIAL; 6],
                MeshModel::WallTorchPosX,
            ),
            entry(
                BED,
                false,
                [
                    BED_SIDE_MATERIAL,
                    BED_SIDE_MATERIAL,
                    BED_SIDE_MATERIAL,
                    BED_TOP_MATERIAL,
                    BED_SIDE_MATERIAL,
                    BED_SIDE_MATERIAL,
                ],
                MeshModel::Bed,
            ),
        ],
        &[],
    );
    let mut section = Section::air();
    section.set(1, 8, 8, WHEAT);
    section.set(2, 8, 8, STANDING_TORCH);
    section.set(3, 8, 8, WALL_TORCH_POS_X);
    section.set(4, 8, 8, BED);
    (section, registry)
}

/// The model+plant scene: a standing torch whose material is a plant layer, so
/// both emission paths publish four cross quads for the same cell.
fn combined_scene(dense: bool) -> (Section, MeshRegistry) {
    let registry = registry(
        &[
            entry(AIR, false, [0; 6], MeshModel::Default),
            entry(BARRIER, true, [1; 6], MeshModel::Default),
            entry(
                STANDING_TORCH,
                false,
                [WHEAT_MATERIAL; 6],
                MeshModel::StandingTorch,
            ),
        ],
        &[],
    );
    let mut section = Section::air();
    if dense {
        section.fill(STANDING_TORCH);
    } else {
        section.set(8, 8, 8, STANDING_TORCH);
    }
    (section, registry)
}

/// Runs the typed provider over one scene and returns the published prefix.
fn build(section: &Section, registry: &MeshRegistry, capacity: usize) -> Vec<MeshQuad> {
    let view = section.view(registry);
    let mut scratch = MeshScratch::try_new().unwrap();
    let mut dst = vec![MeshQuad::default(); capacity];
    let count = NativeMesh.mesh(&view, &mut scratch, &mut dst).unwrap();
    dst.truncate(count);
    dst
}

/// Encodes one quad into the 8-byte little-endian ABI word the Go oracle
/// digests, using the frozen packing of the typed contract.
fn encode_quad(quad: &MeshQuad) -> [u8; 8] {
    quad.packed().to_le_bytes()
}

/// FNV-1a 64-bit digest over the packed little-endian stream, matching the Go
/// oracle's `hash/fnv` New64a over the same bytes.
fn stream_digest(quads: &[MeshQuad]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for quad in quads {
        for byte in encode_quad(quad) {
            hash ^= u64::from(byte);
            hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        }
    }
    hash
}

#[test]
fn isolated_faces_match_the_raw_abi_quad_bits() {
    let (section, registry) = faces_scene();
    let quads = build(&section, &registry, FACES_COUNT);

    assert_eq!(quads.len(), FACES_COUNT, "six isolated axial faces");
    for (index, quad) in quads.iter().enumerate() {
        assert_eq!(
            (
                quad.x(),
                quad.y(),
                quad.z(),
                quad.face(),
                quad.back(),
                quad.material(),
                quad.corners(),
                quad.ao(),
                quad.light(),
            ),
            (
                0,
                0,
                0,
                index as u8,
                false,
                10 + index as u16,
                [0; 4],
                0xff,
                0,
            ),
            "quad {index} must match the ABI observation",
        );
    }
    assert_eq!(
        stream_digest(&quads),
        FACES_DIGEST,
        "packed face stream must match the Go oracle digest"
    );
}

#[test]
fn plant_torch_and_bed_stream_matches_the_raw_abi_order() {
    let (section, registry) = order_scene();
    let quads = build(&section, &registry, ORDER_COUNT);

    assert_eq!(quads.len(), ORDER_COUNT, "plant, torch, wall torch and bed");
    let near = TORCH_WALL_TOP_NEAR_RAW;
    let far = TORCH_WALL_TOP_FAR_RAW;
    let bed = BED_TOP_RAW;
    let mut want: Vec<QuadFields> = Vec::new();
    for (x, material) in [(1_u8, WHEAT_MATERIAL), (2_u8, TORCH_MATERIAL)] {
        want.extend([
            (x, 8, 8, 6, false, material, [0; 4]),
            (x, 8, 8, 6, true, material, [0; 4]),
            (x, 8, 8, 7, false, material, [0; 4]),
            (x, 8, 8, 7, true, material, [0; 4]),
        ]);
    }
    want.extend([
        (3, 8, 8, 4, false, TORCH_MATERIAL, [0, 0, far, near]),
        (3, 8, 8, 5, false, TORCH_MATERIAL, [0, 0, far, near]),
        (3, 8, 8, 0, false, TORCH_MATERIAL, [0; 4]),
        (4, 8, 8, 0, false, BED_SIDE_MATERIAL, [0, bed, bed, 0]),
        (4, 8, 8, 1, false, BED_SIDE_MATERIAL, [0, bed, bed, 0]),
        (4, 8, 8, 4, false, BED_SIDE_MATERIAL, [0, 0, bed, bed]),
        (4, 8, 8, 5, false, BED_SIDE_MATERIAL, [0, 0, bed, bed]),
        (4, 8, 8, 3, false, BED_TOP_MATERIAL, [bed; 4]),
    ]);

    for (index, (quad, want)) in quads.iter().zip(want.iter()).enumerate() {
        assert_eq!(
            (
                quad.x(),
                quad.y(),
                quad.z(),
                quad.face(),
                quad.back(),
                quad.material(),
                quad.corners(),
            ),
            *want,
            "quad {index} must match the ABI observation order",
        );
    }
    assert_eq!(
        stream_digest(&quads),
        ORDER_DIGEST,
        "ordered stream must match the Go oracle digest"
    );
}

#[test]
fn combined_model_plant_cells_reach_the_dense_abi_digest() {
    let (sparse_section, sparse_registry) = combined_scene(false);
    let sparse = build(&sparse_section, &sparse_registry, COMBINED_COUNT);
    assert_eq!(sparse.len(), COMBINED_COUNT, "one cell emits eight quads");
    assert!(
        sparse
            .iter()
            .all(|quad| quad.face() == 6 || quad.face() == 7),
        "every combined quad is a cross quad"
    );

    let (dense_section, dense_registry) = combined_scene(true);
    let dense = build(&dense_section, &dense_registry, STAGE_QUADS);
    assert_eq!(
        dense.len(),
        DENSE_COUNT,
        "a dense accepted registry must not be truncated"
    );
    assert_eq!(
        stream_digest(&dense),
        DENSE_DIGEST,
        "dense packed stream must match the Go oracle digest"
    );
    assert_eq!(
        (dense[0].x(), dense[0].y(), dense[0].z(), dense[0].face()),
        (0, 0, 0, 6),
        "the dense stream starts at the first plant pair of cell (0,0,0)"
    );
    assert_eq!(
        (
            dense[DENSE_COUNT - 1].x(),
            dense[DENSE_COUNT - 1].y(),
            dense[DENSE_COUNT - 1].z(),
            dense[DENSE_COUNT - 1].face(),
            dense[DENSE_COUNT - 1].back(),
        ),
        (15, 15, 15, 7, true),
        "the dense stream ends with the last model pair of cell (15,15,15)"
    );
}

unsafe extern "C" {
    fn mornlea_mesh_section(
        abi_version: u32,
        input: *const u8,
        input_len: usize,
        scratch: *mut u8,
        scratch_len: usize,
        output: *mut u64,
        output_capacity: usize,
        output_len: *mut usize,
    ) -> u32;
    fn mornlea_engine_abi_version() -> u32;
}

/// Legacy minimum output capacity in `u64` slots: the dense scene needs more,
/// which is exactly the late-overflow trigger.
const LEGACY_MIN_SLOTS: usize = 24576;
/// Legacy light scratch size in bytes, kept as the byte lane's workspace.
const LEGACY_SCRATCH_BYTES: usize = 552960;
/// Output canary; no packed quad of these scenes equals it.
const MESH_ABI_CANARY: u64 = 0xD15E_A5ED_F00D_CAFE;

const MESH_STATUS_OK: u32 = 0;
const MESH_STATUS_OUTPUT_OVERFLOW: u32 = 7;

/// One raw registry record in wire order; the remaining wire fields stay zero
/// for these scenes (no fluid, no attenuation, full cubes).
struct RawEntry {
    id: u16,
    opaque: bool,
    emission: u8,
    material: [u16; 6],
    model: u8,
}

fn push_raw_entry(bytes: &mut Vec<u8>, entry: &RawEntry) {
    bytes.extend_from_slice(&entry.id.to_le_bytes());
    bytes.push(u8::from(entry.opaque));
    bytes.push(entry.emission);
    for material in entry.material {
        bytes.extend_from_slice(&material.to_le_bytes());
    }
    bytes.extend_from_slice(&[0, 0, 0, entry.model]);
}

/// Encodes one raw `MGM1` input mirroring `combined_scene` structurally: the
/// same ids, materials, models and empty visibility table, but as the legacy
/// wire bytes the real exported symbol parses. Every neighborhood cell holds
/// `block`, and the torch entry carries `torch_emission` so the all-air case
/// can smuggle a semantic-only violation past the structural parse.
fn dense_raw_input(block: u16, torch_emission: u8) -> Vec<u8> {
    let entries = [
        RawEntry {
            id: AIR,
            opaque: false,
            emission: 0,
            material: [0; 6],
            model: 0,
        },
        RawEntry {
            id: BARRIER,
            opaque: true,
            emission: 0,
            material: [1; 6],
            model: 0,
        },
        RawEntry {
            id: STANDING_TORCH,
            opaque: false,
            emission: torch_emission,
            material: [WHEAT_MATERIAL; 6],
            model: 1,
        },
    ];
    let words_per_row = 1u16;
    let mut bytes = Vec::new();
    bytes.extend_from_slice(b"MGM1");
    bytes.extend_from_slice(&0i32.to_le_bytes());
    bytes.extend_from_slice(&(entries.len() as u16).to_le_bytes());
    bytes.extend_from_slice(&words_per_row.to_le_bytes());
    bytes.extend_from_slice(&AIR.to_le_bytes());
    bytes.extend_from_slice(&BARRIER.to_le_bytes());
    for _ in 0..BLOCKS {
        bytes.extend_from_slice(&block.to_le_bytes());
    }
    bytes.extend_from_slice(&[0u8; 9]);
    bytes.extend_from_slice(&[0u8; 9 * 256 * 2]);
    for entry in &entries {
        push_raw_entry(&mut bytes, entry);
    }
    for _ in 0..entries.len() * usize::from(words_per_row) {
        bytes.extend_from_slice(&0u64.to_le_bytes());
    }
    bytes
}

/// Calls the real exported `mornlea_mesh_section` symbol with a canary-filled
/// destination, returning the status, the metadata word and the payload. The
/// caller keeps no aliasing: input, scratch, output and metadata are disjoint
/// by construction.
fn call_mesh_abi(input: &[u8], slots: usize) -> (u32, usize, Vec<u64>) {
    let version = unsafe { mornlea_engine_abi_version() };
    let mut scratch = vec![0u64; LEGACY_SCRATCH_BYTES / 8];
    let mut output = vec![MESH_ABI_CANARY; slots];
    let mut written = usize::MAX;
    let status = unsafe {
        mornlea_mesh_section(
            version,
            input.as_ptr(),
            input.len(),
            scratch.as_mut_ptr().cast::<u8>(),
            LEGACY_SCRATCH_BYTES,
            output.as_mut_ptr(),
            slots,
            &mut written,
        )
    };
    (status, written, output)
}

#[test]
fn mesh_abi_late_overflow() {
    // The dense model+plant scene needs 32768 quads, past the legacy 24576
    // minimum: the legacy path writes a partial prefix before discovering the
    // short output, so the full-arena canary compare must fail before the fix.
    let dense = dense_raw_input(STANDING_TORCH, 0);
    let (status, written, output) = call_mesh_abi(&dense, LEGACY_MIN_SLOTS);
    assert_eq!(status, MESH_STATUS_OUTPUT_OVERFLOW);
    assert_eq!(written, 0, "overflow metadata must read zero");
    let first = output.iter().position(|word| *word != MESH_ABI_CANARY);
    let count = output
        .iter()
        .filter(|word| **word != MESH_ABI_CANARY)
        .count();
    assert_eq!(
        first,
        None,
        "late overflow wrote {count} payload slots starting at slot {}",
        first.unwrap_or(usize::MAX),
    );

    // The same scene with the full required capacity publishes every packed
    // word bit-identical to the typed provider's staged quads.
    let (status, written, output) = call_mesh_abi(&dense, DENSE_COUNT);
    assert_eq!(status, MESH_STATUS_OK);
    assert_eq!(written, DENSE_COUNT);
    let (section, registry) = combined_scene(true);
    let staged = build(&section, &registry, STAGE_QUADS);
    assert_eq!(staged.len(), DENSE_COUNT);
    assert_eq!(
        stream_digest(&staged),
        DENSE_DIGEST,
        "typed expectation must match the Go oracle digest"
    );
    let expected: Vec<u64> = staged.iter().map(|quad| quad.packed()).collect();
    assert_eq!(
        output, expected,
        "full-capacity words must equal the staged bits"
    );

    // Raw all-air with a semantically invalid but structurally valid unused
    // registry keeps the structural-only exemption: status 0, no payload.
    let air = dense_raw_input(AIR, 16);
    let (status, written, output) = call_mesh_abi(&air, LEGACY_MIN_SLOTS);
    assert_eq!(status, MESH_STATUS_OK);
    assert_eq!(written, 0);
    assert_eq!(
        output,
        vec![MESH_ABI_CANARY; LEGACY_MIN_SLOTS],
        "the all-air shortcut must not touch the payload"
    );
}

#[test]
fn all_air_custom_visibility_matches_the_abi_empty_stream() {
    let registry = registry(
        &[
            entry(AIR, false, [0; 6], MeshModel::Default),
            entry(BARRIER, true, [1; 6], MeshModel::Default),
            entry(
                STANDING_TORCH,
                false,
                [WHEAT_MATERIAL; 6],
                MeshModel::StandingTorch,
            ),
        ],
        &[(0, 0)],
    );
    let section = Section::air();
    let mut raw = dense_raw_input(AIR, 0);
    let visibility_offset = raw.len() - 3 * 8;
    raw[visibility_offset..visibility_offset + 8].copy_from_slice(&1_u64.to_le_bytes());
    let (status, written, output) = call_mesh_abi(&raw, LEGACY_MIN_SLOTS);
    assert_eq!(status, MESH_STATUS_OK);
    assert_eq!(written, 0);
    assert_eq!(output, vec![MESH_ABI_CANARY; LEGACY_MIN_SLOTS]);

    let mut scratch = MeshScratch::try_new().unwrap();
    let mut dst = vec![MeshQuad::default(); STAGE_QUADS];
    assert_eq!(
        NativeMesh.mesh(&section.view(&registry), &mut scratch, &mut dst),
        Ok(written)
    );
    assert!(dst.iter().all(|quad| *quad == MeshQuad::default()));
}
