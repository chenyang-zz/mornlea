//! Validated compact chunk bases prepared away from the authoritative tick.

use std::collections::BTreeSet;
use std::sync::Arc;

use mornlea_domain::{BlockPos, chunk_block_index};
use mornlea_storage::{Chunk, ChunkSave, ContainerSnapshot, StorageKind};

use super::contracts::{BlockObservation, ChunkKey, ServerError};

/// Immutable compact storage with a derived, tick-local non-air height cache.
/// Construction validates save associations and builds heights off the tick;
/// installing or cloning the value never expands or decodes its block storage.
#[derive(Clone)]
pub struct ReadyChunk {
    pub(crate) key: ChunkKey,
    pub(crate) generation: u64,
    pub(crate) revision: u64,
    base: Arc<Chunk>,
    heights: [i16; 256],
    blocks_dirty: bool,
}

impl ReadyChunk {
    pub fn try_new(
        key: ChunkKey,
        generation: u64,
        revision: u64,
        chunk: Chunk,
    ) -> Result<Self, ServerError> {
        let save = ChunkSave {
            key: mornlea_storage::ChunkKey {
                dimension: i32::from(key.dimension.get()),
                x: key.pos.x(),
                z: key.pos.z(),
            },
            revision,
            chunk,
        };
        mornlea_storage::chunk_logical_len(&save, mornlea_storage::CHUNK_CURRENT_SCHEMA).map_err(
            |_| ServerError::InvalidInput {
                field: "ready_chunk",
            },
        )?;
        let mut ready = Self {
            key,
            generation,
            revision,
            base: Arc::new(save.chunk),
            heights: [-65; 256],
            blocks_dirty: false,
        };
        for z in 0..16 {
            for x in 0..16 {
                for y in (-64..320).rev() {
                    let index = ((y + 64) * 256 + z * 16 + x) as usize;
                    if ready.block_index(index) != 0 {
                        ready.heights[(z * 16 + x) as usize] = y as i16;
                        break;
                    }
                }
            }
        }
        Ok(ready)
    }

    pub(crate) fn block(&self, position: BlockPos) -> Option<u16> {
        if !(-64..320).contains(&position.y())
            || position.x() >> 4 != self.key.pos.x()
            || position.z() >> 4 != self.key.pos.z()
        {
            return None;
        }
        Some(self.block_index(chunk_block_index(position) as usize))
    }

    fn block_index(&self, index: usize) -> u16 {
        let section = &self.base.sections[index / 4096];
        section_block(section, index % 4096)
    }

    pub(crate) fn height(&self, x: i32, z: i32) -> i32 {
        i32::from(self.heights[((z & 15) * 16 + (x & 15)) as usize])
    }

    pub(crate) fn set_height(&mut self, x: i32, z: i32, y: i32) {
        self.heights[((z & 15) * 16 + (x & 15)) as usize] = y as i16;
    }

    pub(crate) fn mark_blocks_dirty(&mut self) {
        self.blocks_dirty = true;
    }

    /// One durable identity covers every accepted mutation in this tick.
    /// Carried cell overlays alone never create another revision.
    pub(crate) fn pending_revision(&self, slots_dirty: bool) -> u64 {
        if self.blocks_dirty || slots_dirty {
            self.revision
                .checked_add(1)
                .expect("write preflight rejects exhausted durable revision")
        } else {
            self.revision
        }
    }

    /// Commits only revision metadata; compact data stays shared and overlays
    /// remain owned by the resident maps until an explicit save materializes them.
    pub(crate) fn finish_tick(&mut self, slots_dirty: bool) {
        self.revision = self.pending_revision(slots_dirty);
        self.blocks_dirty = false;
    }

    pub(crate) fn container_state(&self) -> super::container_store::ContainerState {
        super::container_store::ContainerState::new(&self.base, self.revision)
    }

    pub(crate) fn drop_slots(&self) -> &[mornlea_storage::DropSlot] {
        &self.base.drops
    }

    /// Materializes only explicit replay/save snapshots. A changed chunk gets
    /// one durable revision; per-cell overlay CAS counters are never persisted.
    pub(crate) fn snapshot<'a>(
        &self,
        writes: impl Iterator<Item = &'a BlockObservation>,
        drops: Option<&super::drop_store::DropState>,
        containers: Option<&super::container_store::ContainerState>,
    ) -> (ChunkKey, u64, u64, Chunk) {
        let mut chunk = (*self.base).clone();
        let mut converted = BTreeSet::new();
        for write in writes {
            let index = chunk_block_index(write.pos) as usize;
            let section_index = index / 4096;
            if converted.insert(section_index) {
                let old = &chunk.sections[section_index];
                let mut packed = vec![0u64; 1024];
                for cell in 0..4096 {
                    packed[cell / 4] |= u64::from(section_block(old, cell)) << ((cell % 4) * 15);
                }
                chunk.sections[section_index] = ContainerSnapshot {
                    kind: StorageKind::Direct,
                    bits: 15,
                    single: 0,
                    palette: Vec::new(),
                    packed,
                };
            }
            let cell = index % 4096;
            let shift = (cell % 4) * 15;
            let word = &mut chunk.sections[section_index].packed[cell / 4];
            *word = (*word & !(0x7fffu64 << shift)) | (u64::from(write.block) << shift);
        }
        if let Some(drops) = drops {
            chunk.drops = drops.slots.to_vec();
        }
        if let Some(containers) = containers {
            chunk.furnaces = containers.furnaces.to_vec();
            chunk.chests = containers.chests.to_vec();
        }
        let revision = self.pending_revision(
            drops.is_some_and(|state| state.dirty) || containers.is_some_and(|state| state.dirty),
        );
        (self.key, self.generation, revision, chunk)
    }
}

// The F1 validator has checked all widths, lengths, palettes and indices before
// this read path is installed. Packed slots do not cross word boundaries.
fn section_block(section: &ContainerSnapshot, index: usize) -> u16 {
    match section.kind {
        StorageKind::Single => section.single,
        StorageKind::Indexed | StorageKind::Direct => {
            let per_word = 64 / usize::from(section.bits);
            let value = ((section.packed[index / per_word]
                >> ((index % per_word) * usize::from(section.bits)))
                & ((1 << section.bits) - 1)) as u16;
            if section.kind == StorageKind::Indexed {
                section.palette[usize::from(value)]
            } else {
                value
            }
        }
    }
}
