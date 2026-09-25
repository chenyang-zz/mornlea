use mornlea_engine::native::contracts::KernelError;
use mornlea_engine::native::contracts::world::{
    Materials, ProbeOp, ProbeQuery, ProbeValue, WorldgenParams,
};
use mornlea_engine::native::world_probe::NativeWorldProbe;

/// Material table and permutation matching the Go-side ABI observation
/// header: distinct entries 1..=15 in wire order and identity permutation.
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

fn canary() -> ProbeValue {
    ProbeValue::Block(0xA5A5)
}

#[test]
fn probe_mode_and_count() {
    let params = release_abi_params(0, false);
    let op = NativeWorldProbe;

    // The typed height query carries no Y field; constructing it by field
    // name pins that shape against any future reintroduction of one.
    let mut dst = [ProbeValue::default(); 1];
    let res = op.probe(&params, &[ProbeQuery::Height { x: 3, z: 5 }], &mut dst);
    assert_eq!(res, Ok(1));
    assert!(matches!(dst[0], ProbeValue::Height(_)));

    // The same height comes back when it is surrounded by queries that
    // carry explicit Y coordinates.
    let mut ctx = [ProbeValue::default(); 3];
    let res = op.probe(
        &params,
        &[
            ProbeQuery::Terrain {
                position: [3, -65, 5],
            },
            ProbeQuery::Height { x: 3, z: 5 },
            ProbeQuery::Terrain {
                position: [3, 320, 5],
            },
        ],
        &mut ctx,
    );
    assert_eq!(res, Ok(3));
    assert_eq!(ctx[1], dst[0]);

    // Below the surface no decoration applies, so terrain and base agree.
    let mut pair = [ProbeValue::default(); 2];
    let res = op.probe(
        &params,
        &[
            ProbeQuery::Terrain {
                position: [3, 0, 5],
            },
            ProbeQuery::Base {
                position: [3, 0, 5],
            },
        ],
        &mut pair,
    );
    assert_eq!(res, Ok(2));
    assert!(matches!(pair[0], ProbeValue::Block(_)));
    assert_eq!(pair[0], pair[1]);
}

#[test]
fn terrain_out_of_world_y_is_air() {
    let params = release_abi_params(0, false);
    let op = NativeWorldProbe;
    let air = params.materials().air;

    let mut dst = [ProbeValue::default(); 2];
    let res = op.probe(
        &params,
        &[
            ProbeQuery::Terrain {
                position: [3, -65, 5],
            },
            ProbeQuery::Terrain {
                position: [3, 320, 5],
            },
        ],
        &mut dst,
    );
    assert_eq!(res, Ok(2));
    assert_eq!(dst, [ProbeValue::Block(air), ProbeValue::Block(air)]);
}

#[test]
fn base_includes_seeded_decoration() {
    let params = release_abi_params(0, false);
    let op = NativeWorldProbe;

    // Pinned from the Go-side observation run: at (-64, 64, -64) the terrain
    // layer is air while the base layer fills sea water below sea level, so
    // the decoration layers are reachable through the probe provider.
    let mut pair = [ProbeValue::default(); 2];
    let res = op.probe(
        &params,
        &[
            ProbeQuery::Terrain {
                position: [-64, 64, -64],
            },
            ProbeQuery::Base {
                position: [-64, 64, -64],
            },
        ],
        &mut pair,
    );
    assert_eq!(res, Ok(2));
    assert_eq!(pair[0], ProbeValue::Block(1));
    assert_eq!(pair[1], ProbeValue::Block(14));
    assert_ne!(pair[0], pair[1]);
}

#[test]
fn count_bounds() {
    let params = release_abi_params(0, false);
    let op = NativeWorldProbe;

    let mut one = [ProbeValue::default(); 1];
    let res = op.probe(&params, &[ProbeQuery::Height { x: 3, z: 5 }], &mut one);
    assert_eq!(res, Ok(1));

    let full: Vec<ProbeQuery> = (0..64).map(|i| ProbeQuery::Height { x: i, z: i }).collect();
    let mut staged = [canary(); 64];
    let res = op.probe(&params, &full, &mut staged);
    assert_eq!(res, Ok(64));
    assert!(
        !staged.contains(&canary()),
        "all 64 destination slots must be written"
    );

    let over: Vec<ProbeQuery> = (0..65).map(|i| ProbeQuery::Height { x: i, z: i }).collect();
    let mut untouched = [canary(); 65];
    let before = untouched;
    let res = op.probe(&params, &over, &mut untouched);
    assert_eq!(res, Err(KernelError::InvalidInput));
    assert_eq!(untouched, before);

    let mut untouched = [canary(); 4];
    let before = untouched;
    let res = op.probe(&params, &[], &mut untouched);
    assert_eq!(res, Err(KernelError::InvalidInput));
    assert_eq!(untouched, before);
}

#[test]
fn short_output_canary() {
    let params = release_abi_params(0, false);
    let op = NativeWorldProbe;

    let full: Vec<ProbeQuery> = (0..64).map(|i| ProbeQuery::Height { x: i, z: i }).collect();
    let mut dst = [canary(); 63];
    let before = dst;
    let res = op.probe(&params, &full, &mut dst);
    assert_eq!(
        res,
        Err(KernelError::OutputTooSmall {
            needed: 64,
            available: 63
        })
    );
    assert_eq!(dst, before);
}

#[test]
fn extra_capacity_preserves_suffix() {
    let params = release_abi_params(0, false);
    let op = NativeWorldProbe;

    let queries = [
        ProbeQuery::Height { x: 3, z: 5 },
        ProbeQuery::Base {
            position: [-64, 64, -64],
        },
    ];
    let mut dst = [canary(); 4];
    let res = op.probe(&params, &queries, &mut dst);
    assert_eq!(res, Ok(2));
    assert!(matches!(dst[0], ProbeValue::Height(_)));
    assert!(matches!(dst[1], ProbeValue::Block(_)));
    assert_eq!(dst[2], canary());
    assert_eq!(dst[3], canary());
}

#[test]
fn query_order_preserved() {
    let params = release_abi_params(0, false);
    let op = NativeWorldProbe;

    let queries = [
        ProbeQuery::Height { x: 3, z: 5 },
        ProbeQuery::Terrain {
            position: [3, 0, 5],
        },
        ProbeQuery::Base {
            position: [-64, 64, -64],
        },
        ProbeQuery::Height { x: -7, z: -13 },
        ProbeQuery::Terrain {
            position: [-7, -64, -13],
        },
        ProbeQuery::Base {
            position: [-7, 319, -13],
        },
    ];
    let mut batch = [ProbeValue::default(); 6];
    let res = op.probe(&params, &queries, &mut batch);
    assert_eq!(res, Ok(6));

    for (index, query) in queries.iter().enumerate() {
        let mut single = [ProbeValue::default(); 1];
        let res = op.probe(&params, &[*query], &mut single);
        assert_eq!(res, Ok(1));
        assert_eq!(batch[index], single[0], "query {index} out of order");
    }
}

#[test]
fn scratch_free_reuse() {
    let op = NativeWorldProbe;

    // The provider owns no scratch state; repeated calls with different
    // parameters and query sets must all succeed unchanged.
    for (seed, mutate_perm) in [(0, false), (1, false), (0, true), (1, true)] {
        let params = release_abi_params(seed, mutate_perm);
        for count in [1usize, 3, 64] {
            let queries: Vec<ProbeQuery> = (0..count as i32)
                .map(|i| ProbeQuery::Base {
                    position: [i - 4, 64 + i, i * 3],
                })
                .collect();
            let mut dst = vec![ProbeValue::default(); count];
            let res = op.probe(&params, &queries, &mut dst);
            assert_eq!(res, Ok(count));
        }
    }
}
