use mornlea_engine::native::contracts::world::{
    Materials, ProbeOp, ProbeQuery, ProbeValue, WorldgenParams,
};
use mornlea_engine::native::world_probe::NativeWorldProbe;

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

/// The shared ordinary-coordinate query batch, in the same order as the Go
/// oracle's observation records: all modes at positive and negative X/Z, a
/// pair of height lookups at one column, a terrain/base pair at a seeded
/// decoration column, and a seed-sensitive terrain cell.
fn ordinary_queries() -> Vec<ProbeQuery> {
    vec![
        ProbeQuery::Height { x: 3, z: 5 },
        ProbeQuery::Terrain {
            position: [3, 0, 5],
        },
        ProbeQuery::Base {
            position: [3, 0, 5],
        },
        ProbeQuery::Height { x: -7, z: -13 },
        ProbeQuery::Terrain {
            position: [-7, -64, -13],
        },
        ProbeQuery::Base {
            position: [-7, -64, -13],
        },
        ProbeQuery::Terrain {
            position: [-7, 319, -13],
        },
        ProbeQuery::Base {
            position: [-7, 319, -13],
        },
        ProbeQuery::Height { x: 20, z: -40 },
        ProbeQuery::Height { x: 20, z: -40 },
        ProbeQuery::Terrain {
            position: [-64, 64, -64],
        },
        ProbeQuery::Base {
            position: [-64, 64, -64],
        },
        ProbeQuery::Terrain {
            position: [39, 0, -57],
        },
    ]
}

fn probe_values(params: &WorldgenParams, queries: &[ProbeQuery]) -> Vec<ProbeValue> {
    let op = NativeWorldProbe;
    let mut dst = vec![ProbeValue::default(); queries.len()];
    let written = op.probe(params, queries, &mut dst).unwrap();
    assert_eq!(written, queries.len());
    dst
}

/// FNV-1a 64-bit digest over the 8-byte ABI output records (height LE +
/// block LE + two zero reserved bytes), matching the Go oracle's `hash/fnv`
/// New64a over the same output buffer.
fn probe_digest(values: &[ProbeValue]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for value in values {
        let (height, block) = match *value {
            ProbeValue::Height(height) => (height, 0u16),
            ProbeValue::Block(block) => (0i32, block),
        };
        for byte in height
            .to_le_bytes()
            .into_iter()
            .chain(block.to_le_bytes())
            .chain([0u8; 2])
        {
            hash ^= u64::from(byte);
            hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        }
    }
    hash
}

#[test]
fn release_abi_fixture_matches_go_observation_header() {
    let params = release_abi_params(0, false);
    assert_eq!(
        params.materials().as_slice(),
        [1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15]
    );
    for (index, entry) in params.perm().iter().enumerate() {
        assert_eq!(*entry, (index % 256) as u8, "perm[{index}]");
    }
    let mutated = release_abi_params(0, true);
    assert_eq!(mutated.perm()[1], 0x20);
}

#[test]
fn seed_0_values_match_release_abi() {
    let values = probe_values(&release_abi_params(0, false), &ordinary_queries());
    assert_eq!(
        values,
        vec![
            ProbeValue::Height(67),
            ProbeValue::Block(2),
            ProbeValue::Block(2),
            ProbeValue::Height(56),
            ProbeValue::Block(5),
            ProbeValue::Block(5),
            ProbeValue::Block(1),
            ProbeValue::Block(1),
            ProbeValue::Height(53),
            ProbeValue::Height(53),
            ProbeValue::Block(1),
            ProbeValue::Block(14),
            ProbeValue::Block(11),
        ]
    );
    assert_eq!(probe_digest(&values), 0x57d1_f3de_f502_2eba);
}

#[test]
fn seed_1_values_match_release_abi() {
    let values = probe_values(&release_abi_params(1, false), &ordinary_queries());
    assert_eq!(
        values,
        vec![
            ProbeValue::Height(67),
            ProbeValue::Block(2),
            ProbeValue::Block(2),
            ProbeValue::Height(56),
            ProbeValue::Block(5),
            ProbeValue::Block(5),
            ProbeValue::Block(1),
            ProbeValue::Block(1),
            ProbeValue::Height(53),
            ProbeValue::Height(53),
            ProbeValue::Block(1),
            ProbeValue::Block(14),
            ProbeValue::Block(2),
        ]
    );
    assert_eq!(probe_digest(&values), 0xf7f7_3fa4_998a_06c3);
}

#[test]
fn height_ignores_caller_y() {
    let params = release_abi_params(0, false);
    let op = NativeWorldProbe;

    // The typed height query has no Y field at all, so equality across
    // differently positioned query contexts is structural; pin it plus the
    // Go-observed value.
    let mut low = [ProbeValue::default(); 2];
    let res = op.probe(
        &params,
        &[
            ProbeQuery::Terrain {
                position: [20, -65, -40],
            },
            ProbeQuery::Height { x: 20, z: -40 },
        ],
        &mut low,
    );
    assert_eq!(res, Ok(2));
    let mut high = [ProbeValue::default(); 3];
    let res = op.probe(
        &params,
        &[
            ProbeQuery::Terrain {
                position: [20, 320, -40],
            },
            ProbeQuery::Height { x: 20, z: -40 },
            ProbeQuery::Base {
                position: [20, 320, -40],
            },
        ],
        &mut high,
    );
    assert_eq!(res, Ok(3));
    assert_eq!(low[1], high[1]);
    assert_eq!(low[1], ProbeValue::Height(53));
}

#[test]
fn extreme_coordinate_probes_are_deterministic() {
    let params = release_abi_params(0, false);

    // Includes base probes whose oak-tree fringe crosses i32::MIN and
    // i32::MAX. Full debug/release ABI parity at signed extremes is deferred
    // to the ABI adapter layer; here the frozen contract is no panic and
    // call-to-call determinism.
    let queries = [
        ProbeQuery::Height {
            x: i32::MIN,
            z: i32::MIN,
        },
        ProbeQuery::Height {
            x: i32::MAX,
            z: i32::MAX,
        },
        ProbeQuery::Terrain {
            position: [i32::MIN, 0, i32::MIN],
        },
        ProbeQuery::Base {
            position: [i32::MIN, 64, i32::MIN],
        },
        ProbeQuery::Base {
            position: [i32::MIN + 2, 64, i32::MIN + 2],
        },
        ProbeQuery::Base {
            position: [i32::MAX - 1, 64, i32::MAX - 1],
        },
        ProbeQuery::Terrain {
            position: [i32::MAX, 0, i32::MAX],
        },
    ];
    let first = probe_values(&params, &queries);
    let second = probe_values(&params, &queries);
    assert_eq!(first, second, "extreme probes must be deterministic");
}

unsafe extern "C" {
    fn mornlea_worldgen_probe(
        abi_version: u32,
        input: *const u8,
        input_len: usize,
        output: *mut u8,
        output_len: usize,
    ) -> u32;
    fn mornlea_engine_abi_version() -> u32;
}

const PROBE_HEADER_BYTES: usize = 566;
const PROBE_RECORD_BYTES: usize = 16;
const PROBE_OUTPUT_RECORD_BYTES: usize = 8;
const PROBE_MAX_RECORDS: usize = 64;
const ABI_CANARY: u8 = 0xA5;

/// Raw ABI records matching `ordinary_queries` order; the two height lookups
/// at (20,-40) carry raw Y 12345 and -12345 to pin mode-0 Y-independence.
fn ordinary_raw_records() -> Vec<(u32, i32, i32, i32)> {
    vec![
        (0, 3, 0, 5),
        (1, 3, 0, 5),
        (2, 3, 0, 5),
        (0, -7, 0, -13),
        (1, -7, -64, -13),
        (2, -7, -64, -13),
        (1, -7, 319, -13),
        (2, -7, 319, -13),
        (0, 20, 12345, -40),
        (0, 20, -12345, -40),
        (1, -64, 64, -64),
        (2, -64, 64, -64),
        (1, 39, 0, -57),
    ]
}

/// Raw ABI records for the signed-extreme batch, in typed-test order.
fn extreme_raw_records() -> Vec<(u32, i32, i32, i32)> {
    vec![
        (0, i32::MIN, 0, i32::MIN),
        (0, i32::MAX, 0, i32::MAX),
        (1, i32::MIN, 0, i32::MIN),
        (2, i32::MIN, 64, i32::MIN),
        (2, i32::MIN + 2, 64, i32::MIN + 2),
        (2, i32::MAX - 1, 64, i32::MAX - 1),
        (1, i32::MAX, 0, i32::MAX),
    ]
}

/// Builds the raw `MGW1` layout-3 probe request from the typed params: the
/// 566-byte header with the material table in wire order, the 512-byte
/// permutation, then the u32 count and the 16-byte records.
fn probe_abi_input(params: &WorldgenParams, records: &[(u32, i32, i32, i32)]) -> Vec<u8> {
    let mut bytes = vec![0u8; PROBE_HEADER_BYTES + 4 + records.len() * PROBE_RECORD_BYTES];
    bytes[0..4].copy_from_slice(b"MGW1");
    bytes[4..8].copy_from_slice(&3u32.to_le_bytes());
    bytes[8..16].copy_from_slice(&params.seed().to_le_bytes());
    bytes[16..20].copy_from_slice(&(-64i32).to_le_bytes());
    bytes[20..24].copy_from_slice(&320i32.to_le_bytes());
    for (index, id) in params.materials().as_slice().into_iter().enumerate() {
        bytes[24 + index * 2..26 + index * 2].copy_from_slice(&id.to_le_bytes());
    }
    bytes[54..PROBE_HEADER_BYTES].copy_from_slice(params.perm());
    bytes[PROBE_HEADER_BYTES..PROBE_HEADER_BYTES + 4]
        .copy_from_slice(&(records.len() as u32).to_le_bytes());
    for (index, (mode, x, y, z)) in records.iter().enumerate() {
        let offset = PROBE_HEADER_BYTES + 4 + index * PROBE_RECORD_BYTES;
        bytes[offset..offset + 4].copy_from_slice(&mode.to_le_bytes());
        bytes[offset + 4..offset + 8].copy_from_slice(&x.to_le_bytes());
        bytes[offset + 8..offset + 12].copy_from_slice(&y.to_le_bytes());
        bytes[offset + 12..offset + 16].copy_from_slice(&z.to_le_bytes());
    }
    bytes
}

/// Encodes typed `ProbeValue` results as 8-byte ABI output records (height
/// LE + block LE + two zero reserved bytes).
fn probe_encoded(values: &[ProbeValue]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(values.len() * PROBE_OUTPUT_RECORD_BYTES);
    for value in values {
        let (height, block) = match *value {
            ProbeValue::Height(height) => (height, 0u16),
            ProbeValue::Block(block) => (0i32, block),
        };
        bytes.extend_from_slice(&height.to_le_bytes());
        bytes.extend_from_slice(&block.to_le_bytes());
        bytes.extend_from_slice(&[0u8; 2]);
    }
    bytes
}

/// FNV-1a 64-bit digest over the raw ABI output bytes, matching the Go
/// oracle's digest over the same buffer.
fn abi_bytes_digest(bytes: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

/// Calls the real exported `mornlea_worldgen_probe` symbol with a
/// canary-filled destination, returning the status and the destination bytes.
fn call_probe_abi(input: &[u8], output_len: usize) -> (u32, Vec<u8>) {
    let version = unsafe { mornlea_engine_abi_version() };
    let mut output = vec![ABI_CANARY; output_len];
    let status = unsafe {
        mornlea_worldgen_probe(
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
fn worldgen_probe_abi_modes() {
    // Seed-0 ordinary batch: full output bytes must equal the typed
    // provider's encoded bytes, with the release digest pinned.
    let params = release_abi_params(0, false);
    let records = ordinary_raw_records();
    let input = probe_abi_input(&params, &records);
    let (status, output) = call_probe_abi(&input, records.len() * PROBE_OUTPUT_RECORD_BYTES);
    assert_eq!(status, 0, "ordinary batch status");
    let expected = probe_encoded(&probe_values(&params, &ordinary_queries()));
    assert_eq!(output, expected, "ordinary batch full output bytes");
    assert_eq!(
        abi_bytes_digest(&output),
        0x57d1_f3de_f502_2eba,
        "ordinary batch digest"
    );
    // Mode 0 ignores the raw Y field: the two height records at (20,-40)
    // with raw Y 12345 and -12345 must produce identical 8-byte outputs.
    assert_eq!(
        output[8 * 8..8 * 9],
        output[8 * 9..8 * 10],
        "mode-0 Y-independence bytes"
    );

    // Signed extremes: replay the release observations against the merged
    // provider.
    let extreme = extreme_raw_records();
    let extreme_input = probe_abi_input(&release_abi_params(0, false), &extreme);
    let (status, extreme_output) =
        call_probe_abi(&extreme_input, extreme.len() * PROBE_OUTPUT_RECORD_BYTES);
    assert_eq!(status, 0, "extreme batch status");
    assert_eq!(
        abi_bytes_digest(&extreme_output),
        0x7211_9860_a6d5_d143,
        "extreme batch digest"
    );

    // Mode 3 record: input rejection with the output canary untouched.
    let mut bad_mode = ordinary_raw_records();
    bad_mode[0].0 = 3;
    let bad_input = probe_abi_input(&release_abi_params(0, false), &bad_mode);
    let before = vec![ABI_CANARY; bad_mode.len() * PROBE_OUTPUT_RECORD_BYTES];
    let (status, output) = call_probe_abi(&bad_input, before.len());
    assert_eq!(status, 3, "mode-3 record status");
    assert_eq!(output, before, "mode-3 rejection keeps the canary");

    // 65-query input: the parse rejects before sizing, so status 3.
    let wide: Vec<(u32, i32, i32, i32)> = vec![(1, 3, 0, 5); PROBE_MAX_RECORDS + 1];
    let wide_input = probe_abi_input(&release_abi_params(0, false), &wide);
    let (status, _) = call_probe_abi(&wide_input, wide.len() * PROBE_OUTPUT_RECORD_BYTES);
    assert_eq!(status, 3, "65-query input status");

    // 64-query boundary: exact success, then one byte short gives status 7
    // with the full canary untouched; the exact retry matches byte-for-byte.
    let full: Vec<(u32, i32, i32, i32)> = vec![(1, 3, 0, 5); PROBE_MAX_RECORDS];
    let full_input = probe_abi_input(&release_abi_params(0, false), &full);
    let exact_len = full.len() * PROBE_OUTPUT_RECORD_BYTES;
    let (status, exact) = call_probe_abi(&full_input, exact_len);
    assert_eq!(status, 0, "64-query exact status");
    let short_before = vec![ABI_CANARY; exact_len - 1];
    let (status, short) = call_probe_abi(&full_input, exact_len - 1);
    assert_eq!(status, 7, "short output status");
    assert_eq!(short, short_before, "short output keeps the canary");
    let (status, retry) = call_probe_abi(&full_input, exact_len);
    assert_eq!(status, 0, "exact retry status");
    assert_eq!(retry, exact, "exact retry bytes");
}

#[test]
fn mutated_permutation_changes_probe_values() {
    let plain = probe_values(&release_abi_params(0, false), &ordinary_queries());
    let mutated = probe_values(&release_abi_params(0, true), &ordinary_queries());
    assert_eq!(probe_digest(&mutated), 0x254b_4817_5549_7a7a);
    assert_ne!(
        probe_digest(&mutated),
        probe_digest(&plain),
        "permutation change must alter the probed values",
    );
}
