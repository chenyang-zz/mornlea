use mornlea_engine::native::contracts::world::{
    Materials, WorldgenOp, WorldgenParams, WorldgenScratch,
};
use mornlea_engine::native::worldgen::NativeWorldgen;

const CHUNK_CELLS: usize = 98304;

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
