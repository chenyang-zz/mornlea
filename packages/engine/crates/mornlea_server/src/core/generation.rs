//! Off-tick terrain preparation over the checked native worldgen provider.

use mornlea_domain::Dimension;
use mornlea_engine::native::contracts::world::{
    Materials, WorldgenOp, WorldgenParams, WorldgenScratch,
};
use mornlea_engine::native::worldgen::NativeWorldgen;

use super::contracts::{ChunkKey, ServerError};
use super::go_random::permutation;
use mornlea_storage::{ChestSlot, Chunk, ContainerSnapshot, DropSlot, FurnaceSlot, StorageKind};

/// One off-tick owner reuses numerical scratch across both dimensions.
/// Acquisition owns request identities, Ready preparation and publication.
pub struct ChunkGenerator {
    parameters: [WorldgenParams; 2],
    scratch: WorldgenScratch,
    dense: Box<[u16]>,
}

impl ChunkGenerator {
    /// Prepares canonical materials and independent Go-compatible permutations.
    pub fn try_new(seed: i64, fluid_enabled: bool) -> Result<Self, ServerError> {
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
        let make = |dimension_seed| {
            WorldgenParams::try_new(dimension_seed, materials, permutation(dimension_seed)).map_err(
                |_| ServerError::Internal {
                    invariant: "worldgen parameters",
                },
            )
        };
        Ok(Self {
            parameters: [
                make(seed)?,
                make((seed as u64 ^ 0x9e3779b97f4a7c15) as i64)?,
            ],
            scratch: WorldgenScratch::try_new().map_err(|_| ServerError::Internal {
                invariant: "worldgen scratch",
            })?,
            dense: vec![0; 98_304].into_boxed_slice(),
        })
    }

    /// Returns the checked parameters for a supported dimension.
    pub fn parameters(&self, dimension: Dimension) -> &WorldgenParams {
        &self.parameters[usize::from(dimension.get())]
    }
}

impl ChunkGenerator {
    /// Produces one owned compact chunk without acquiring authoritative state.
    pub fn generate(&mut self, key: ChunkKey) -> Result<Chunk, ServerError> {
        let used = NativeWorldgen
            .generate_chunk(
                &self.parameters[usize::from(key.dimension.get())],
                [key.pos.x(), key.pos.z()],
                &mut self.scratch,
                &mut self.dense,
            )
            .map_err(|_| ServerError::Internal {
                invariant: "worldgen kernel",
            })?;
        if used != 98_304 {
            return Err(ServerError::Internal {
                invariant: "worldgen output length",
            });
        }
        let mut sections = Vec::with_capacity(24);
        for dense in self.dense.chunks_exact(4096) {
            let dense = dense.try_into().map_err(|_| ServerError::Internal {
                invariant: "worldgen output length",
            })?;
            sections.push(compact_section(dense)?);
        }
        Ok(Chunk {
            sections,
            drops: vec![DropSlot::default(); 32],
            furnaces: vec![FurnaceSlot::default(); 32],
            chests: vec![ChestSlot::default(); 16],
        })
    }
}

// First appearance matches Go Compact; collection stops at the direct-mode
// threshold while ID validation still examines every cell. Packing never
// crosses word boundaries and starts from zero so high padding stays canonical.
fn compact_section(dense: &[u16; 4096]) -> Result<ContainerSnapshot, ServerError> {
    let mut palette = Vec::with_capacity(257);
    for &id in dense {
        if id > 32767 {
            return Err(ServerError::Internal {
                invariant: "worldgen block id",
            });
        }
        if palette.len() <= 256 && !palette.contains(&id) {
            palette.push(id);
        }
    }
    if palette.len() == 1 {
        return Ok(ContainerSnapshot {
            kind: StorageKind::Single,
            bits: 0,
            single: palette[0],
            palette: Vec::new(),
            packed: Vec::new(),
        });
    }
    let (kind, bits) = match palette.len() {
        2..=16 => (StorageKind::Indexed, 4),
        17..=256 => (StorageKind::Indexed, 8),
        _ => (StorageKind::Direct, 15),
    };
    let per_word = 64 / usize::from(bits);
    let mut packed = vec![0u64; 4096usize.div_ceil(per_word)];
    for (index, &id) in dense.iter().enumerate() {
        let value = if kind == StorageKind::Direct {
            u64::from(id)
        } else {
            palette
                .iter()
                .position(|&entry| entry == id)
                .ok_or(ServerError::Internal {
                    invariant: "worldgen block id",
                })? as u64
        };
        packed[index / per_word] |= value << ((index % per_word) * usize::from(bits));
    }
    if kind == StorageKind::Direct {
        palette = Vec::new();
    }
    Ok(ContainerSnapshot {
        kind,
        bits,
        single: 0,
        palette,
        packed,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cell(section: &ContainerSnapshot, index: usize) -> u16 {
        if section.kind == StorageKind::Single {
            return section.single;
        }
        let per_word = 64 / usize::from(section.bits);
        let raw = ((section.packed[index / per_word]
            >> ((index % per_word) * usize::from(section.bits)))
            & ((1 << section.bits) - 1)) as u16;
        if section.kind == StorageKind::Indexed {
            section.palette[usize::from(raw)]
        } else {
            raw
        }
    }

    #[test]
    fn compact_uniform_air() {
        assert_eq!(
            compact_section(&[0; 4096]).expect("section"),
            ContainerSnapshot {
                kind: StorageKind::Single,
                bits: 0,
                single: 0,
                palette: Vec::new(),
                packed: Vec::new(),
            }
        );
    }

    #[test]
    fn compact_palette_follows_first_appearance() {
        let dense: [u16; 4096] = std::array::from_fn(|i| [19, 2, 19, 84][i % 4]);
        let section = compact_section(&dense).expect("section");
        assert_eq!(section.kind, StorageKind::Indexed);
        assert_eq!(section.bits, 4);
        assert_eq!(section.single, 0);
        assert_eq!(section.palette, [19, 2, 84]);
        assert_eq!(section.packed[0], 0x2010201020102010);
        for (i, value) in dense.iter().enumerate() {
            assert_eq!(cell(&section, i), *value);
        }
    }

    #[test]
    fn compact_mode_transitions_and_noncrossing_padding() {
        for (distinct, kind, bits) in [
            (16, StorageKind::Indexed, 4),
            (17, StorageKind::Indexed, 8),
            (256, StorageKind::Indexed, 8),
            (257, StorageKind::Direct, 15),
        ] {
            let dense: [u16; 4096] = std::array::from_fn(|i| (i % distinct) as u16);
            let section = compact_section(&dense).expect("section");
            assert_eq!(
                (section.kind, section.bits, section.single),
                (kind, bits, 0)
            );
            if kind == StorageKind::Indexed {
                assert_eq!(section.palette, (0..distinct as u16).collect::<Vec<_>>());
            } else {
                assert!(section.palette.is_empty());
            }
            let per_word = 64 / usize::from(bits);
            assert_eq!(section.packed.len(), 4096usize.div_ceil(per_word));
            if bits == 15 {
                assert!(section.packed.iter().all(|word| word >> 60 == 0));
            }
            for (i, value) in dense.iter().enumerate() {
                assert_eq!(cell(&section, i), *value, "distinct {distinct}, cell {i}");
            }
        }
    }

    #[test]
    fn compact_rejects_out_of_domain_ids_even_after_direct_transition() {
        for index in [0, 4095] {
            let mut dense: [u16; 4096] = std::array::from_fn(|i| (i % 257) as u16);
            dense[index] = 32768;
            assert_eq!(
                compact_section(&dense),
                Err(ServerError::Internal {
                    invariant: "worldgen block id"
                })
            );
        }
    }
}
