use mornlea_engine::native::contracts::world::{
    Materials, WorldgenOp, WorldgenParams, WorldgenScratch,
};
use mornlea_engine::native::worldgen::NativeWorldgen;

unsafe extern "C" {
    fn mornlea_worldgen_chunk(
        abi_version: u32,
        input: *const u8,
        input_len: usize,
        output: *mut u8,
        output_len: usize,
    ) -> u32;
    fn mornlea_engine_abi_version() -> u32;
}

const CHUNK_CELLS: usize = 98304;
const CHUNK_HEADER_BYTES: usize = 566;
const CHUNK_INPUT_BYTES: usize = 574;
const CHUNK_OUTPUT_BYTES: usize = 196608;
const ABI_CANARY: u8 = 0xA5;

fn dummy_materials() -> Materials {
    Materials {
        air: 0,
        stone: 1,
        dirt: 2,
        grass: 3,
        bedrock: 4,
        snow: 5,
        sand: 6,
        clay: 7,
        gravel: 8,
        iron_ore: 9,
        coal_ore: 10,
        oak_log: 11,
        leaves: 12,
        water: 13,
        short_grass: 14,
    }
}

fn dummy_params() -> WorldgenParams {
    let mut perm = [0u8; 512];
    for (i, entry) in perm.iter_mut().enumerate() {
        *entry = (i % 256) as u8;
    }
    WorldgenParams::try_new(1, dummy_materials(), perm).unwrap()
}

/// Material table and permutation matching the Go-side ABI observation
/// header: distinct entries 1..=15 in wire order and identity permutation.
/// `mutate_perm` applies the same one-entry perturbation as the Go oracle's
/// modified-permutation case.
fn release_abi_params(seed: i64, mutate_perm: bool) -> WorldgenParams {
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
        water: 14,
        short_grass: 15,
    };
    let mut perm = [0u8; 512];
    for (i, entry) in perm.iter_mut().enumerate() {
        *entry = (i % 256) as u8;
    }
    if mutate_perm {
        perm[1] = 0x20;
    }
    WorldgenParams::try_new(seed, materials, perm).unwrap()
}

fn generate(params: &WorldgenParams, chunk: [i32; 2]) -> Vec<u16> {
    let op = NativeWorldgen;
    let mut scratch = WorldgenScratch::try_new().unwrap();
    let mut dst = vec![0u16; CHUNK_CELLS];
    let written = op
        .generate_chunk(params, chunk, &mut scratch, &mut dst)
        .unwrap();
    assert_eq!(written, CHUNK_CELLS);
    dst
}

/// FNV-1a 64-bit digest over the little-endian dense cells, matching the Go
/// oracle's `hash/fnv` New64a over the same 196608-byte buffer.
fn dense_digest(cells: &[u16]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for cell in cells {
        for byte in cell.to_le_bytes() {
            hash ^= u64::from(byte);
            hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        }
    }
    hash
}

// Digest literals pinned from real Go-side `nativeabi.WorldgenChunk` runs; the
// Go oracle test asserts the same values against the release ABI.

const DIGEST_SEED0_CHUNK_ORIGIN: u64 = 0x6f67_4aa8_baa4_ea79;
const DIGEST_SEED0_CHUNK_NEG_ONE_TWO: u64 = 0xcfd0_4d93_2a54_cb6b;
const DIGEST_SEED0_EXTREME_LOW: u64 = 0x8d3e_20fa_7599_b5c6;
const DIGEST_SEED0_EXTREME_HIGH: u64 = 0x7f88_06f3_505d_fbce;
const DIGEST_SEED0_MUTATED_PERM: u64 = 0x072b_63b2_bdf3_922b;

#[test]
fn generates_extreme_coordinate_chunk_without_panic() {
    let params = dummy_params();
    let op = NativeWorldgen;
    let mut scratch = WorldgenScratch::try_new().unwrap();
    let mut dst = vec![0u16; 98304];

    // Near i32::MIN
    let res = op.generate_chunk(
        &params,
        [i32::MIN / 16, i32::MIN / 16],
        &mut scratch,
        &mut dst,
    );
    assert!(res.is_ok());

    // Near i32::MAX - 15
    let res = op.generate_chunk(
        &params,
        [(i32::MAX - 15) / 16, (i32::MAX - 15) / 16],
        &mut scratch,
        &mut dst,
    );
    assert!(res.is_ok());
}

#[test]
fn chunk_cells_match_release_abi_observations() {
    let cells = generate(&release_abi_params(0, false), [0, 0]);
    // Independent generation contract: the bottom Y layer is entirely
    // bedrock, material table entry 5 in the observation header.
    for &cell in &cells[..256] {
        assert_eq!(cell, 5, "bottom layer must be entirely bedrock");
    }
    // Bitwise cell parity with the Go-side run at a spread of dense indices.
    assert_eq!(cells[16467], 2);
    assert_eq!(cells[32768], 4);
    assert_eq!(cells[98303], 1);
    assert_eq!(dense_digest(&cells), DIGEST_SEED0_CHUNK_ORIGIN);
}

#[test]
fn extreme_coordinate_digest_matches_release_abi() {
    for (chunk, want) in [
        ([i32::MIN / 16, i32::MIN / 16], DIGEST_SEED0_EXTREME_LOW),
        (
            [(i32::MAX - 15) / 16, (i32::MAX - 15) / 16],
            DIGEST_SEED0_EXTREME_HIGH,
        ),
    ] {
        let params = release_abi_params(0, false);
        let first = generate(&params, chunk);
        let second = generate(&params, chunk);
        assert_eq!(first, second, "chunk {chunk:?} must be deterministic");
        assert_eq!(dense_digest(&first), want, "chunk {chunk:?}");
    }
}

#[test]
fn chunk_digests_match_release_abi_across_chunks_and_permutation() {
    assert_eq!(
        dense_digest(&generate(&release_abi_params(0, false), [-1, 2])),
        DIGEST_SEED0_CHUNK_NEG_ONE_TWO,
    );
    let mutated = generate(&release_abi_params(0, true), [0, 0]);
    assert_eq!(dense_digest(&mutated), DIGEST_SEED0_MUTATED_PERM);
    assert_ne!(
        dense_digest(&mutated),
        DIGEST_SEED0_CHUNK_ORIGIN,
        "permutation change must alter the generated chunk",
    );
}

/// Builds the raw 574-byte `MGW1` chunk request from the typed params:
/// layout-3 header with the material table in wire order, the 512-byte
/// permutation, and the chunk coordinates.
fn chunk_abi_input(params: &WorldgenParams, chunk: [i32; 2]) -> Vec<u8> {
    assert_eq!(CHUNK_INPUT_BYTES, CHUNK_HEADER_BYTES + 8);
    let mut bytes = vec![0u8; CHUNK_INPUT_BYTES];
    bytes[0..4].copy_from_slice(b"MGW1");
    bytes[4..8].copy_from_slice(&3u32.to_le_bytes());
    bytes[8..16].copy_from_slice(&params.seed().to_le_bytes());
    bytes[16..20].copy_from_slice(&(-64i32).to_le_bytes());
    bytes[20..24].copy_from_slice(&320i32.to_le_bytes());
    for (index, id) in params.materials().as_slice().into_iter().enumerate() {
        bytes[24 + index * 2..26 + index * 2].copy_from_slice(&id.to_le_bytes());
    }
    bytes[54..CHUNK_HEADER_BYTES].copy_from_slice(params.perm());
    bytes[CHUNK_HEADER_BYTES..CHUNK_HEADER_BYTES + 4].copy_from_slice(&chunk[0].to_le_bytes());
    bytes[CHUNK_HEADER_BYTES + 4..CHUNK_INPUT_BYTES].copy_from_slice(&chunk[1].to_le_bytes());
    bytes
}

/// FNV-1a 64-bit digest over the raw 196608-byte little-endian output,
/// matching the Go oracle's digest over the same ABI buffer.
fn abi_bytes_digest(bytes: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

/// Reads the dense cells out of the raw little-endian ABI output.
fn abi_bytes_cells(bytes: &[u8]) -> Vec<u16> {
    bytes
        .chunks_exact(2)
        .map(|chunk| u16::from_le_bytes([chunk[0], chunk[1]]))
        .collect()
}

/// Calls the real exported `mornlea_worldgen_chunk` symbol with a
/// canary-filled destination, returning the status and the destination bytes.
fn call_chunk_abi(input: &[u8], output_len: usize) -> (u32, Vec<u8>) {
    let version = unsafe { mornlea_engine_abi_version() };
    let mut output = vec![ABI_CANARY; output_len];
    let status = unsafe {
        mornlea_worldgen_chunk(
            version,
            input.as_ptr(),
            input.len(),
            output.as_mut_ptr(),
            output.len(),
        )
    };
    (status, output)
}

#[test]
fn worldgen_chunk_abi_bytes() {
    // Seed 0 at chunk (0,0): the full-output digest plus spot cells must
    // match the typed provider's cells, pinning full release-ABI parity.
    let params = release_abi_params(0, false);
    let input = chunk_abi_input(&params, [0, 0]);
    let (status, output) = call_chunk_abi(&input, CHUNK_OUTPUT_BYTES);
    assert_eq!(status, 0, "seed 0 chunk (0,0) status");
    assert_eq!(
        abi_bytes_digest(&output),
        DIGEST_SEED0_CHUNK_ORIGIN,
        "seed 0 chunk (0,0) full-output digest"
    );
    let expected = generate(&params, [0, 0]);
    let cells = abi_bytes_cells(&output);
    assert_eq!(cells, expected, "seed 0 chunk (0,0) full cell parity");
    for index in (0..8).chain(49152..49160) {
        assert_eq!(
            cells[index], expected[index],
            "seed 0 chunk (0,0) cell {index}"
        );
    }

    // Signed extremes: the wrapping fix must survive the ABI adapter.
    for (chunk, want) in [
        ([i32::MIN / 16, i32::MIN / 16], DIGEST_SEED0_EXTREME_LOW),
        (
            [(i32::MAX - 15) / 16, (i32::MAX - 15) / 16],
            DIGEST_SEED0_EXTREME_HIGH,
        ),
    ] {
        let input = chunk_abi_input(&release_abi_params(0, false), chunk);
        let (status, output) = call_chunk_abi(&input, CHUNK_OUTPUT_BYTES);
        assert_eq!(status, 0, "extreme chunk {chunk:?} status");
        assert_eq!(
            abi_bytes_digest(&output),
            want,
            "extreme chunk {chunk:?} digest"
        );
    }

    // Mutated permutation: the caller-supplied permutation is a real input.
    let mutated_input = chunk_abi_input(&release_abi_params(0, true), [0, 0]);
    let (status, mutated) = call_chunk_abi(&mutated_input, CHUNK_OUTPUT_BYTES);
    assert_eq!(status, 0, "mutated permutation status");
    assert_eq!(
        abi_bytes_digest(&mutated),
        DIGEST_SEED0_MUTATED_PERM,
        "mutated permutation digest"
    );
    assert_ne!(
        mutated, output,
        "permutation change must alter the ABI output"
    );

    // Short destination: status 7 with the full destination canary untouched.
    let short_before = vec![ABI_CANARY; CHUNK_OUTPUT_BYTES - 1];
    let (status, short) = call_chunk_abi(&input, CHUNK_OUTPUT_BYTES - 1);
    assert_eq!(status, 7, "short output status");
    assert_eq!(
        short, short_before,
        "short output keeps the destination canary"
    );
}
