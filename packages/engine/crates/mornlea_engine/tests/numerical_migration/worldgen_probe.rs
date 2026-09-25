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
