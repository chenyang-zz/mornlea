use mornlea_engine::native::contracts::KernelError;
use mornlea_engine::native::contracts::world::{
    Materials, WorldgenOp, WorldgenParams, WorldgenScratch,
};
use mornlea_engine::native::worldgen::NativeWorldgen;

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

#[test]
fn exact_capacity_succeeds() {
    let params = dummy_params();
    let op = NativeWorldgen;
    let mut scratch = WorldgenScratch::try_new().unwrap();
    let mut dst = [0u16; 98304];

    let res = op.generate_chunk(&params, [0, 0], &mut scratch, &mut dst);
    assert_eq!(res, Ok(98304));
}

#[test]
fn short_capacity_fails_without_mutating() {
    let params = dummy_params();
    let op = NativeWorldgen;
    let mut scratch = WorldgenScratch::try_new().unwrap();
    let mut dst = [0xABCDu16; 98303];

    let res = op.generate_chunk(&params, [0, 0], &mut scratch, &mut dst);
    assert_eq!(
        res,
        Err(KernelError::OutputTooSmall {
            needed: 98304,
            available: 98303
        })
    );
    assert!(
        dst.iter().all(|&cell| cell == 0xABCD),
        "destination canary must stay unchanged"
    );
}

#[test]
fn extra_capacity_succeeds_retains_canary() {
    let params = dummy_params();
    let op = NativeWorldgen;
    let mut scratch = WorldgenScratch::try_new().unwrap();
    let mut dst = [0u16; 98305];
    dst[98304] = 0xABCD;

    let res = op.generate_chunk(&params, [0, 0], &mut scratch, &mut dst);
    assert_eq!(res, Ok(98304));
    assert_eq!(dst[98304], 0xABCD);
}

#[test]
fn scratch_is_reused() {
    let params = dummy_params();
    let op = NativeWorldgen;
    let mut scratch = WorldgenScratch::try_new().unwrap();
    let mut dst = [0u16; 98304];

    let res1 = op.generate_chunk(&params, [0, 0], &mut scratch, &mut dst);
    assert_eq!(res1, Ok(98304));
    let res2 = op.generate_chunk(&params, [1, 0], &mut scratch, &mut dst);
    assert_eq!(res2, Ok(98304));
}

#[test]
fn duplicate_material_rejected_unless_water_air() {
    let mut perm = [0u8; 512];
    for (i, entry) in perm.iter_mut().enumerate() {
        *entry = (i % 256) as u8;
    }

    let mut m = dummy_materials();

    // Valid: water == air
    m.water = 0;
    assert!(WorldgenParams::try_new(1, m, perm).is_ok());

    // Invalid: stone == dirt
    m.water = 13;
    m.dirt = 1;
    assert!(WorldgenParams::try_new(1, m, perm).is_err());

    // Invalid: short_grass == air
    m.dirt = 2;
    m.short_grass = 0;
    assert!(WorldgenParams::try_new(1, m, perm).is_err());
}

#[test]
fn success_overwrites_every_destination_cell() {
    let params = dummy_params();
    let op = NativeWorldgen;
    let mut scratch = WorldgenScratch::try_new().unwrap();
    let mut dst = [0xABCDu16; 98304];

    let res = op.generate_chunk(&params, [0, 0], &mut scratch, &mut dst);
    assert_eq!(res, Ok(98304));
    // Every dense cell must be written: a success that leaves canary cells
    // behind would silently under-generate the chunk.
    assert!(
        !dst.contains(&0xABCD),
        "unwritten canary cell left in destination"
    );
}

#[test]
fn seeded_params_matches_all_independent_go_headers() {
    let fixture: serde_json::Value = serde_json::from_str(include_str!(
        "../../../../../../testdata/runtime-migration/server/worldgen-seeding.json"
    ))
    .unwrap();
    let rows = fixture["headers"].as_array().unwrap();
    assert_eq!(rows.len(), 24);
    for row in rows {
        let expected = row["header_hex"].as_str().unwrap();
        let bytes: Vec<u8> = (0..expected.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&expected[i..i + 2], 16).unwrap())
            .collect();
        assert_eq!(bytes.len(), 566);
        let ids: Vec<u16> = bytes[24..54]
            .chunks_exact(2)
            .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
            .collect();
        let materials = Materials {
            air: ids[0],
            stone: ids[1],
            dirt: ids[2],
            grass: ids[3],
            bedrock: ids[4],
            snow: ids[5],
            sand: ids[6],
            clay: ids[7],
            gravel: ids[8],
            iron_ore: ids[9],
            coal_ore: ids[10],
            oak_log: ids[11],
            leaves: ids[12],
            water: ids[13],
            short_grass: ids[14],
        };
        let seed = i64::from_le_bytes(bytes[8..16].try_into().unwrap());
        let params = mornlea_engine::native::worldgen::seeded_params(seed, materials).unwrap();
        let mut actual = b"MGW1".to_vec();
        actual.extend(3u32.to_le_bytes());
        actual.extend(params.seed().to_le_bytes());
        actual.extend((-64i32).to_le_bytes());
        actual.extend(320i32.to_le_bytes());
        for id in params.materials().as_slice() {
            actual.extend(id.to_le_bytes());
        }
        actual.extend(params.perm());
        assert_eq!(actual, bytes, "seed {seed}, row {row}");
        assert_eq!(&params.perm()[..256], &params.perm()[256..]);
        let mut counts = [0u16; 256];
        for byte in &params.perm()[..256] {
            counts[usize::from(*byte)] += 1;
        }
        assert_eq!(counts, [1; 256]);
    }
}

#[test]
fn seeded_params_preserves_material_rejection_and_water_alias() {
    let mut materials = dummy_materials();
    materials.water = materials.air;
    assert!(mornlea_engine::native::worldgen::seeded_params(42, materials).is_ok());
    materials.dirt = materials.stone;
    assert_eq!(
        mornlea_engine::native::worldgen::seeded_params(42, materials),
        Err(KernelError::InvalidInput)
    );
}
