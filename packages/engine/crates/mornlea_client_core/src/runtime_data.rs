//! Session-owned native inputs derived from the compatibility registry.

use std::sync::Arc;

use mornlea_domain::Dimension;
use mornlea_engine::native::contracts::{CollisionCell, Materials, MeshRegistry, WorldgenParams};
use mornlea_engine::native::{mesh::try_new_registry, worldgen::seeded_params};

use crate::ClientError;

#[path = "runtime_data_generated.rs"]
mod generated;

/// The caller caches this owned registry once per session, outside numerical work.
pub(crate) fn mesh_registry() -> Result<Arc<MeshRegistry>, ClientError> {
    try_new_registry(&generated::MESH_ENTRIES, &generated::MESH_VISIBILITY, 0, 1)
        .map(Arc::new)
        .map_err(|_| ClientError::Internal)
}

/// Missing mirror data is unknown; an invalid registered ID is rejected.
pub(crate) fn collision_cell(block: Option<u16>) -> Result<CollisionCell, ClientError> {
    let Some(block) = block else {
        return Ok(CollisionCell::default());
    };
    let (used, boxes) = generated::COLLISION
        .get(usize::from(block))
        .ok_or(ClientError::InvalidInput)?;
    CollisionCell::try_new(true, *boxes, *used).map_err(|_| ClientError::Internal)
}

/// Dimension selection stays in the client domain; the engine receives only numerical inputs.
pub(crate) fn worldgen_params(
    seed: i64,
    dimension: Dimension,
    fluid_enabled: bool,
) -> Result<Arc<WorldgenParams>, ClientError> {
    let seed = match dimension.get() {
        0 => seed,
        1 => (seed as u64 ^ 0x9e3779b97f4a7c15) as i64,
        _ => return Err(ClientError::InvalidInput),
    };
    let materials = Materials {
        air: 0,
        stone: 2,
        dirt: 3,
        grass: 4,
        bedrock: 5,
        snow: 25,
        sand: 15,
        clay: 24,
        gravel: 16,
        iron_ore: 8,
        coal_ore: 7,
        oak_log: 17,
        leaves: 19,
        water: if fluid_enabled { 27 } else { 0 },
        short_grass: 84,
    };
    seeded_params(seed, materials)
        .map(Arc::new)
        .map_err(|_| ClientError::Internal)
}

pub(crate) fn is_water(block: u16) -> bool {
    (27..=34).contains(&block)
}

#[cfg(test)]
mod tests {
    use super::*;
    use mornlea_engine::native::contracts::{
        Aabb, CollisionGrid, CollisionOp, CollisionRequest, MeshOp, MeshQuad, MeshScratch, MeshView,
    };
    use mornlea_engine::native::{collision::NativeCollision, mesh::NativeMesh};

    #[test]
    fn collision_shapes_preserve_loaded_empty_and_registered_bounds() {
        assert_eq!(collision_cell(None).unwrap(), CollisionCell::default());
        assert_eq!(collision_cell(Some(90)), Err(ClientError::InvalidInput));
        assert_eq!(
            collision_cell(Some(u16::MAX)),
            Err(ClientError::InvalidInput)
        );
        for id in 0..90 {
            assert!(collision_cell(Some(id)).unwrap().loaded(), "id {id}");
        }
        for id in [0, 27, 34, 37, 44, 46, 61, 70, 71, 75, 84, 85, 88, 89] {
            assert!(
                collision_cell(Some(id)).unwrap().boxes().is_empty(),
                "id {id}"
            );
        }
        for id in [1, 2, 19, 26] {
            assert_eq!(
                collision_cell(Some(id)).unwrap().boxes(),
                &[Aabb {
                    minimum: [0.0; 3],
                    maximum: [1.0; 3]
                }]
            );
        }
        for (ids, height) in [
            (&[35, 36][..], 15.0 / 16.0),
            (&[76, 77, 78, 79, 80, 81, 82, 83][..], 9.0 / 16.0),
        ] {
            for &id in ids {
                assert_eq!(
                    collision_cell(Some(id)).unwrap().boxes(),
                    &[Aabb {
                        minimum: [0.0; 3],
                        maximum: [1.0, height, 1.0]
                    }]
                );
            }
        }
        for (ids, minimum, maximum) in [
            ([62, 65], [0.0, 0.0, 13.0 / 16.0], [1.0; 3]),
            ([64, 67], [0.0; 3], [3.0 / 16.0, 1.0, 1.0]),
            ([66, 69], [0.0; 3], [1.0, 1.0, 3.0 / 16.0]),
            ([68, 63], [13.0 / 16.0, 0.0, 0.0], [1.0; 3]),
        ] {
            for id in ids {
                assert_eq!(
                    collision_cell(Some(id)).unwrap().boxes(),
                    &[Aabb { minimum, maximum }]
                );
            }
        }
    }

    #[test]
    fn native_collision_lands_on_real_stone_bed_and_farmland() {
        for (id, height) in [(2, 1.0), (35, 15.0 / 16.0), (76, 9.0 / 16.0)] {
            let mut cells = vec![collision_cell(Some(0)).unwrap(); 6 * 6 * 6];
            // The native grid indexes YXZ, independently of chunk YZX storage.
            cells[(2 * 6 + 2) * 6 + 2] = collision_cell(Some(id)).unwrap();
            let grid = CollisionGrid::try_new([-2, -2, -2], [6, 6, 6], &cells).unwrap();
            let result = NativeCollision
                .resolve(&CollisionRequest {
                    position: [0.5, 2.0, 0.5],
                    displacement: [0.0, -2.0, 0.0],
                    began_grounded: false,
                    step_height: 0.0,
                    grid,
                })
                .unwrap();
            assert_eq!(result.position, [0.5, height, 0.5], "id {id}");
            assert!(result.on_ground);
            assert!(!result.hit_unknown);
        }
    }

    #[test]
    fn native_mesh_uses_real_stone_water_plant_snow_and_bed() {
        let registry = mesh_registry().unwrap();
        assert_eq!(registry.entries().len(), 90);
        assert_eq!(registry.visibility().len(), 180);
        assert_eq!((registry.air(), registry.barrier()), (0, 1));
        assert_eq!(registry.entries()[89].material, [164; 6]);
        let mut scratch = MeshScratch::try_new().unwrap();
        for (id, count) in [(2, 6), (27, 6), (84, 4), (85, 6), (76, 5)] {
            let mut blocks = vec![0; 110592].into_boxed_slice();
            blocks[13 * 4096 + (8 * 16 + 8) * 16 + 8] = id;
            let blocks: &[u16; 110592] = blocks.as_ref().try_into().unwrap();
            let view = MeshView {
                blocks,
                heights_present: &[false; 9],
                heights: &[[0; 256]; 9],
                section_origin_y: 0,
                registry: &registry,
            };
            let mut output = [MeshQuad::default(); 64];
            let actual = NativeMesh.mesh(&view, &mut scratch, &mut output).unwrap();
            assert_eq!(actual, count, "id {id}");
            assert!(output[..actual].iter().all(|q| if id == 2 {
                q.material() == 0
            } else {
                q.material() != 0
            }));
            if id == 84 {
                assert!(
                    output[..actual]
                        .iter()
                        .all(|q| q.face() == 6 || q.face() == 7)
                );
            }
            if id == 85 {
                assert!(output[..actual].iter().any(|q| q.corners() == [1; 4]));
            }
            if id == 76 {
                assert!(output[..actual].iter().any(|q| q.corners() == [8; 4]));
            }
        }
    }

    #[test]
    fn seeded_world_inputs_preserve_dimension_fluid_and_water_identity() {
        for (dimension, expected) in [
            (
                Dimension::OVERWORLD,
                "4d475731030000002a00000000000000c0ffffff400100000000020003000400050019000f0018001000080007001100130000005400cabe96547767ae059c8957483d42f88f7287596fb0c4c36e270a254bcc39b68ee5944452d8fdf532fa9d4793147a8dd3047ca6cd2886c10151497bbde4b5ac0d4a6a03c53eec3f192b178280e9691363643b7de3f31b9091462aef9222b79e3006c92965c2d9f0fe2c62d4d15c4d8c9bad404eee7e5612832ed01eb23c45760e3708b8eb5b79f918cfdd00a56875a716f7f45dbc31ea8aa415f69802df41e7e8b16c1fa9db2d84231ac0d24c7fb9dadc9a5511506174bae181c8d6a86d3ac6fb8b2085b33307385ae26ba1bf2426a37371af78950c36edf18821bbced70fdec7a2532f66f2d50997fc434f1ce6e0abaa70a01d5835b49f5effcb600b3499105fcabe96547767ae059c8957483d42f88f7287596fb0c4c36e270a254bcc39b68ee5944452d8fdf532fa9d4793147a8dd3047ca6cd2886c10151497bbde4b5ac0d4a6a03c53eec3f192b178280e9691363643b7de3f31b9091462aef9222b79e3006c92965c2d9f0fe2c62d4d15c4d8c9bad404eee7e5612832ed01eb23c45760e3708b8eb5b79f918cfdd00a56875a716f7f45dbc31ea8aa415f69802df41e7e8b16c1fa9db2d84231ac0d24c7fb9dadc9a5511506174bae181c8d6a86d3ac6fb8b2085b33307385ae26ba1bf2426a37371af78950c36edf18821bbced70fdec7a2532f66f2d50997fc434f1ce6e0abaa70a01d5835b49f5effcb600b3499105f",
            ),
            (
                Dimension::DEPTHS,
                "4d475731030000003f7c4a7fb979379ec0ffffff400100000000020003000400050019000f00180010000800070011001300000054004f76481a3f6f9b290367f630c74da7057e59baaf1880c3bd3aacbeff024039757860ad5dcb9d1f9e1e08318f2dc1b7d5dedc94717cc5f3814557680a2fa232d28daaf0386a5a0fed365ec26e9cb398d6dae0f73766fdfe6133d7b2432e9fb6d0a10c26043e094955f88306b9d9442751998b419a2ba98a4ef472e35fe2c63d9170a4cf86f5e1d8017d56e728bfb42c1393a34685105b52128cf9e9f2a684f1a5c8ca6382e56447cedb530777b019e6d4fbb8ab627935226d166987ebeffa96eabca8d39790237b501d20fc920bb13b006bb58874cd1cdd650ecc7a6c1454e4c034581b89c4ec0de8ee157f4a4bdfc9215c11ae8e73bb2442254c95d1172a3ca04f76481a3f6f9b290367f630c74da7057e59baaf1880c3bd3aacbeff024039757860ad5dcb9d1f9e1e08318f2dc1b7d5dedc94717cc5f3814557680a2fa232d28daaf0386a5a0fed365ec26e9cb398d6dae0f73766fdfe6133d7b2432e9fb6d0a10c26043e094955f88306b9d9442751998b419a2ba98a4ef472e35fe2c63d9170a4cf86f5e1d8017d56e728bfb42c1393a34685105b52128cf9e9f2a684f1a5c8ca6382e56447cedb530777b019e6d4fbb8ab627935226d166987ebeffa96eabca8d39790237b501d20fc920bb13b006bb58874cd1cdd650ecc7a6c1454e4c034581b89c4ec0de8ee157f4a4bdfc9215c11ae8e73bb2442254c95d1172a3ca0",
            ),
        ] {
            let params = worldgen_params(42, dimension, false).unwrap();
            let mut bytes = b"MGW1".to_vec();
            bytes.extend(3u32.to_le_bytes());
            bytes.extend(params.seed().to_le_bytes());
            bytes.extend((-64i32).to_le_bytes());
            bytes.extend(320i32.to_le_bytes());
            for id in params.materials().as_slice() {
                bytes.extend(id.to_le_bytes());
            }
            bytes.extend(params.perm());
            let actual: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
            assert_eq!(actual, expected);
            assert_eq!(
                worldgen_params(42, dimension, true)
                    .unwrap()
                    .materials()
                    .water,
                27
            );
        }
        for id in 0..=90 {
            assert_eq!(is_water(id), (27..=34).contains(&id));
        }
        assert!(!is_water(u16::MAX));
        assert!(Dimension::new(2).is_err());
    }
}
