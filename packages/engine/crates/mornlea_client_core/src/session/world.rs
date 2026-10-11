//! Immutable compact block ownership shared by prediction, preparation and prompts.
//! Only mirror admission replaces a column; readers never maintain a second world.

use super::WorldConfirmed;
use crate::preparation::{OwnedMeshView, SectionKey};
use crate::{ClientError, ClientLimits};
use mornlea_domain::{
    BlockChanges, BlockPos, ChunkPos, ChunkSnapshot, Dimension, PalettedSection, chunk_block_index,
};
use mornlea_engine::native::contracts::MeshRegistry;
use mornlea_protocol::{MAX_Y, MIN_Y};
use std::sync::Arc;

#[derive(Clone, Debug, PartialEq)]
pub(super) struct ChunkContent {
    sections: Box<[Arc<PalettedSection>; 24]>,
    // Heights count every non-air block and publish with the immutable sections.
    heights: [i16; 256],
    // Metadata revisions cannot replace the identity of held block content.
    revision: u64,
    pub(super) bytes: usize,
}

fn section_bytes(section: &PalettedSection) -> usize {
    let dynamic = if let Some((_, palette, words)) = section.as_indexed() {
        std::mem::size_of_val(palette) + std::mem::size_of_val(words)
    } else if let Some(words) = section.as_direct() {
        std::mem::size_of_val(words)
    } else {
        0
    };
    std::mem::size_of::<PalettedSection>() + 2 * std::mem::size_of::<usize>() + dynamic
}

impl ChunkContent {
    fn column_top(sections: &[Arc<PalettedSection>; 24], column: usize) -> i16 {
        for y in (0..384).rev() {
            let index = (y % 16) * 256 + column;
            if sections[y / 16].block_at(index).expect("checked section") != 0 {
                return (MIN_Y + y as i32) as i16;
            }
        }
        (MIN_Y - 1) as i16
    }

    fn charge(sections: &[Arc<PalettedSection>; 24]) -> Result<usize, ClientError> {
        // Reserve a full sparse tree node per entry, including its key and edges.
        let mut bytes = 512 + std::mem::size_of::<Self>() + std::mem::size_of_val(sections);
        for section in sections {
            bytes = bytes
                .checked_add(section_bytes(section))
                .ok_or(ClientError::Capacity)?;
        }
        Ok(bytes)
    }

    fn snapshot_charge(snapshot: &ChunkSnapshot) -> Result<usize, ClientError> {
        let mut bytes =
            512 + std::mem::size_of::<Self>() + std::mem::size_of::<[Arc<PalettedSection>; 24]>();
        for section in snapshot.sections() {
            bytes = bytes
                .checked_add(section_bytes(section))
                .ok_or(ClientError::Capacity)?;
        }
        Ok(bytes)
    }
}

// Validate the entire horizontal neighborhood before allocating mesh input.
fn mesh_columns(dimension: Dimension, chunk: ChunkPos) -> Result<[(u8, i64, i64); 9], ClientError> {
    let mut keys = [(dimension.get(), 0, 0); 9];
    for cx in 0..3 {
        let x = chunk
            .x()
            .checked_add(cx as i32 - 1)
            .ok_or(ClientError::InvalidInput)?;
        for cz in 0..3 {
            let z = chunk
                .z()
                .checked_add(cz as i32 - 1)
                .ok_or(ClientError::InvalidInput)?;
            keys[cx * 3 + cz] = (dimension.get(), i64::from(x), i64::from(z));
        }
    }
    Ok(keys)
}

impl WorldConfirmed {
    /// Copies one native neighborhood; unknown columns and vertical edges stay barriers.
    pub(crate) fn mesh_view(
        &self,
        dimension: Dimension,
        section: SectionKey,
        registry: &Arc<MeshRegistry>,
    ) -> Result<OwnedMeshView, ClientError> {
        let chunk = section.chunk();
        if !self.content.contains_key(&(
            dimension.get(),
            i64::from(chunk.x()),
            i64::from(chunk.z()),
        )) {
            return Err(ClientError::InvalidInput);
        }
        let keys = mesh_columns(dimension, chunk)?;
        for &(_, x, z) in &keys {
            for coordinate in [x, z] {
                let origin = coordinate
                    .checked_mul(16)
                    .ok_or(ClientError::InvalidInput)?;
                i32::try_from(origin).map_err(|_| ClientError::InvalidInput)?;
                i32::try_from(origin.checked_add(15).ok_or(ClientError::InvalidInput)?)
                    .map_err(|_| ClientError::InvalidInput)?;
            }
        }
        // Allocate on the heap without constructing the native arrays on the stack.
        let mut blocks = Vec::new();
        blocks
            .try_reserve_exact(110592)
            .map_err(|_| ClientError::Capacity)?;
        blocks.resize(110592, registry.barrier());
        let mut heights = Vec::new();
        heights
            .try_reserve_exact(9)
            .map_err(|_| ClientError::Capacity)?;
        heights.resize(9, [0i16; 256]);
        let mut heights_present = [false; 9];
        for cx in 0..3 {
            for cz in 0..3 {
                let horizontal = cx * 3 + cz;
                let Some(content) = self.content.get(&keys[horizontal]) else {
                    continue;
                };
                heights_present[horizontal] = true;
                heights[horizontal] = content.heights;
                for cy in 0..3 {
                    let si = i32::from(section.section()) + cy as i32 - 1;
                    if !(0..24).contains(&si) {
                        continue;
                    }
                    let start = ((cx * 3 + cy) * 3 + cz) * 4096;
                    for local in 0..4096 {
                        blocks[start + local] = content.sections[si as usize]
                            .block_at(local)
                            .expect("checked section");
                    }
                }
            }
        }
        OwnedMeshView::try_new(
            blocks
                .into_boxed_slice()
                .try_into()
                .map_err(|_| ClientError::Internal)?,
            heights_present,
            heights
                .into_boxed_slice()
                .try_into()
                .map_err(|_| ClientError::Internal)?,
            MIN_Y + i32::from(section.section()) * 16,
            Arc::clone(registry),
        )
    }

    /// Metadata-only columns never enter the scheduler's content invalidation signature.
    pub(crate) fn mesh_stamps(
        &self,
        dimension: Dimension,
        chunk: ChunkPos,
    ) -> Result<[Option<u64>; 9], ClientError> {
        if !self.content.contains_key(&(
            dimension.get(),
            i64::from(chunk.x()),
            i64::from(chunk.z()),
        )) {
            return Err(ClientError::InvalidInput);
        }
        let keys = mesh_columns(dimension, chunk)?;
        Ok(std::array::from_fn(|index| {
            self.content
                .get(&keys[index])
                .map(|content| content.revision)
        }))
    }

    pub fn content_bytes(&self) -> usize {
        self.content_bytes
    }

    /// Returns a loaded block only; absent columns never become synthetic air.
    pub fn block_at(&self, dimension: Dimension, position: BlockPos) -> Option<u16> {
        if !(MIN_Y..MAX_Y).contains(&position.y()) {
            return None;
        }
        let key = (
            dimension.get(),
            i64::from(position.x() >> 4),
            i64::from(position.z() >> 4),
        );
        let content = self.content.get(&key)?;
        let index = chunk_block_index(position) as usize;
        content.sections[index / 4096].block_at(index % 4096)
    }

    fn replacement_charge(
        &self,
        key: &(u8, i64, i64),
        bytes: usize,
        limits: &ClientLimits,
    ) -> Result<usize, ClientError> {
        let count = self.content.len() + usize::from(!self.content.contains_key(key));
        if count > limits.inbound_observations() {
            return Err(ClientError::Capacity);
        }
        let previous = self.content.get(key).map_or(0, |column| column.bytes);
        let next = self
            .content_bytes
            .checked_sub(previous)
            .and_then(|held| held.checked_add(bytes))
            .ok_or(ClientError::Capacity)?;
        if next > limits.inbound_bytes() {
            return Err(ClientError::Capacity);
        }
        Ok(next)
    }

    /// Stages one validated compact column before replacing either content or revision.
    pub fn apply_snapshot(
        &mut self,
        snapshot: &ChunkSnapshot,
        limits: &ClientLimits,
    ) -> Result<(), ClientError> {
        let key = (
            snapshot.dimension().get(),
            i64::from(snapshot.chunk().x()),
            i64::from(snapshot.chunk().z()),
        );
        if self
            .chunks
            .get(&key)
            .is_some_and(|revision| snapshot.revision() <= *revision)
        {
            return Err(ClientError::InvalidInput);
        }
        let bytes = ChunkContent::snapshot_charge(snapshot)?;
        let next = self.replacement_charge(&key, bytes, limits)?;
        let sections = Box::new(std::array::from_fn(|index| {
            Arc::new(snapshot.sections()[index].clone())
        }));
        let heights = std::array::from_fn(|column| ChunkContent::column_top(&sections, column));
        self.content.insert(
            key,
            Arc::new(ChunkContent {
                sections,
                heights,
                revision: snapshot.revision(),
                bytes,
            }),
        );
        self.chunks.insert(key, snapshot.revision());
        self.content_bytes = next;
        Ok(())
    }

    /// Applies exact source writes to a staged column, preserving the old Arc on refusal.
    pub fn apply_changes(
        &mut self,
        changes: &BlockChanges,
        limits: &ClientLimits,
    ) -> Result<(), ClientError> {
        let key = (
            changes.dimension().get(),
            i64::from(changes.chunk().x()),
            i64::from(changes.chunk().z()),
        );
        if self.chunks.get(&key) != Some(&changes.base_revision()) {
            return Err(ClientError::InvalidInput);
        }
        let prior = self.content.get(&key).ok_or(ClientError::InvalidState)?;
        let mut sections = prior.sections.clone();
        let mut offset = 0;
        while offset < changes.changes().len() {
            let section = chunk_block_index(changes.changes()[offset].position()) as usize / 4096;
            let mut blocks: Vec<u16> = (0..4096)
                .map(|index| sections[section].block_at(index).expect("checked section"))
                .collect();
            while offset < changes.changes().len()
                && chunk_block_index(changes.changes()[offset].position()) as usize / 4096
                    == section
            {
                let change = changes.changes()[offset];
                blocks[chunk_block_index(change.position()) as usize % 4096] = change.block();
                offset += 1;
            }
            let compact = if blocks.iter().all(|block| *block == blocks[0]) {
                PalettedSection::single(blocks[0])
            } else {
                let mut words = vec![0u64; 1024];
                for (index, block) in blocks.iter().enumerate() {
                    words[index / 4] |= u64::from(*block) << ((index % 4) * 15);
                }
                PalettedSection::direct(words.into_boxed_slice())
            }
            .map_err(|_| ClientError::InvalidInput)?;
            sections[section] = Arc::new(compact);
        }
        let bytes = ChunkContent::charge(&sections)?;
        let next = self.replacement_charge(&key, bytes, limits)?;
        let mut heights = prior.heights;
        let mut touched = [false; 256];
        for change in changes.changes() {
            touched[chunk_block_index(change.position()) as usize % 256] = true;
        }
        for (column, changed) in touched.iter().enumerate() {
            if *changed {
                heights[column] = ChunkContent::column_top(&sections, column);
            }
        }
        self.content.insert(
            key,
            Arc::new(ChunkContent {
                sections,
                heights,
                revision: changes.new_revision(),
                bytes,
            }),
        );
        self.chunks.insert(key, changes.new_revision());
        self.content_bytes = next;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::preparation::SectionKey;
    use crate::runtime_data::mesh_registry;
    use mornlea_domain::{BlockChange, BlockChangesParts, ChunkPos, ChunkSnapshotParts};
    use mornlea_engine::native::contracts::{MeshOp, MeshQuad, MeshScratch};
    use mornlea_engine::native::mesh::NativeMesh;

    fn snapshot(
        dimension: Dimension,
        chunk: ChunkPos,
        revision: u64,
        fills: [u16; 24],
    ) -> ChunkSnapshot {
        ChunkSnapshot::try_new(ChunkSnapshotParts {
            dimension,
            chunk,
            revision,
            sections: Box::new(std::array::from_fn(|si| {
                PalettedSection::single(fills[si]).unwrap()
            })),
        })
        .unwrap()
    }

    fn delta(chunk: ChunkPos, base: u64, writes: &[(i32, i32, i32, u16)]) -> BlockChanges {
        let mut changes: Vec<_> = writes
            .iter()
            .map(|&(x, y, z, id)| {
                BlockChange::try_new(BlockPos::new(chunk.x() * 16 + x, y, chunk.z() * 16 + z), id)
                    .unwrap()
            })
            .collect();
        changes.sort_by_key(|change| chunk_block_index(change.position()));
        BlockChanges::try_new(BlockChangesParts {
            dimension: Dimension::OVERWORLD,
            chunk,
            base_revision: base,
            new_revision: base + 1,
            changes: changes.into_boxed_slice(),
        })
        .unwrap()
    }

    fn byte_limit(bytes: usize) -> ClientLimits {
        let base = ClientLimits::try_new().unwrap();
        ClientLimits::try_new_with(
            base.queued_input_events(),
            base.inbound_observations(),
            bytes,
            base.outbound_commands(),
            base.outbound_bytes(),
            base.prediction_journal(),
            base.message_work(),
            base.mesh_work(),
            base.preparation_results(),
            base.preparation_bytes(),
            base.family_records(),
            base.frame_bytes(),
        )
        .unwrap()
    }

    #[test]
    fn mesh_view_preserves_all_native_section_and_local_indexes_at_negative_coordinates() {
        let limits = ClientLimits::try_new().unwrap();
        let registry = mesh_registry().unwrap();
        let center = ChunkPos::new(-3, -7);
        let mut world = WorldConfirmed::default();
        for cx in 0..3 {
            for cz in 0..3 {
                let chunk = ChunkPos::new(center.x() + cx - 1, center.z() + cz - 1);
                let mut fills = [0; 24];
                for cy in 0..3 {
                    fills[9 + cy] = 2 + (cx * 9 + cy as i32 * 3 + cz) as u16;
                }
                world
                    .apply_snapshot(
                        &snapshot(Dimension::OVERWORLD, chunk, (cx * 3 + cz + 1) as u64, fills),
                        &limits,
                    )
                    .unwrap();
                let base = (cx * 3 + cz + 1) as u64;
                world
                    .apply_changes(
                        &delta(
                            chunk,
                            base,
                            &[(3, 85, 11, 84), (5, 128 + cx * 3 + cz, 9, 27)],
                        ),
                        &limits,
                    )
                    .unwrap();
            }
        }
        let section = SectionKey::try_new(center, 10).unwrap();
        let view = world
            .mesh_view(Dimension::OVERWORLD, section, &registry)
            .unwrap();
        assert_eq!(view.section_origin_y(), 96);
        assert!(Arc::ptr_eq(view.registry(), &registry));
        assert_eq!(view.heights_present(), &[true; 9]);
        for (row, top) in view.heights().iter().zip(128..=136) {
            assert_eq!(row[149], top);
            assert_eq!(row[0], 127);
        }
        assert_eq!(
            world.mesh_stamps(Dimension::OVERWORLD, center).unwrap(),
            [
                Some(2),
                Some(3),
                Some(4),
                Some(5),
                Some(6),
                Some(7),
                Some(8),
                Some(9),
                Some(10)
            ]
        );
        for cx in 0..3 {
            for cy in 0..3 {
                for cz in 0..3 {
                    for local in 0..4096 {
                        let expected = if cy == 0 && local == 1459 {
                            84
                        } else {
                            2 + (cx * 9 + cy * 3 + cz) as u16
                        };
                        assert_eq!(
                            view.blocks()[((cx * 3 + cy) * 3 + cz) * 4096 + local],
                            expected,
                            "neighbor {cx}/{cy}/{cz}, local {local}"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn mesh_heights_count_non_air_and_recompute_only_the_changed_columns() {
        let limits = ClientLimits::try_new().unwrap();
        let registry = mesh_registry().unwrap();
        let chunk = ChunkPos::new(-1, -2);
        let section = SectionKey::try_new(chunk, 23).unwrap();
        let mut world = WorldConfirmed::default();
        world
            .apply_snapshot(&snapshot(Dimension::OVERWORLD, chunk, 1, [0; 24]), &limits)
            .unwrap();
        assert_eq!(
            world
                .mesh_view(Dimension::OVERWORLD, section, &registry)
                .unwrap()
                .heights()[4],
            [-65; 256]
        );
        world
            .apply_changes(
                &delta(
                    chunk,
                    1,
                    &[
                        (2, 5, 3, 2),
                        (2, 319, 3, 27),
                        (7, 318, 11, 84),
                        (9, 317, 4, 25),
                    ],
                ),
                &limits,
            )
            .unwrap();
        let held = world
            .mesh_view(Dimension::OVERWORLD, section, &registry)
            .unwrap();
        assert_eq!(held.heights()[4][50], 319);
        assert_eq!(held.heights()[4][183], 318);
        assert_eq!(held.heights()[4][73], 317);
        assert_eq!(held.heights()[4][0], -65);
        world
            .apply_changes(
                &delta(chunk, 2, &[(2, 319, 3, 0), (7, 318, 11, 0)]),
                &limits,
            )
            .unwrap();
        let current = world
            .mesh_view(Dimension::OVERWORLD, section, &registry)
            .unwrap();
        assert_eq!(current.heights()[4][50], 5);
        assert_eq!(current.heights()[4][183], -65);
        assert_eq!(current.heights()[4][73], 317);
        assert_eq!(held.heights()[4][50], 319);
        assert_eq!(
            world.mesh_stamps(Dimension::OVERWORLD, chunk).unwrap()[4],
            Some(3)
        );
    }

    #[test]
    fn mesh_view_treats_unloaded_metadata_and_vertical_boundaries_as_barriers() {
        let limits = ClientLimits::try_new().unwrap();
        let registry = mesh_registry().unwrap();
        let chunk = ChunkPos::new(-2, -5);
        let mut world = WorldConfirmed::default().with_chunk(Dimension::OVERWORLD, chunk, 1);
        assert!(matches!(
            world.mesh_view(
                Dimension::OVERWORLD,
                SectionKey::try_new(chunk, 0).unwrap(),
                &registry
            ),
            Err(ClientError::InvalidInput)
        ));
        assert_eq!(
            world.mesh_stamps(Dimension::OVERWORLD, chunk),
            Err(ClientError::InvalidInput)
        );
        world
            .apply_snapshot(&snapshot(Dimension::OVERWORLD, chunk, 2, [0; 24]), &limits)
            .unwrap();
        world = world.with_chunk(Dimension::OVERWORLD, ChunkPos::new(-3, -6), 8);
        for (si, barrier_cy) in [(0, 0), (23, 2)] {
            let view = world
                .mesh_view(
                    Dimension::OVERWORLD,
                    SectionKey::try_new(chunk, si).unwrap(),
                    &registry,
                )
                .unwrap();
            assert_eq!(
                view.heights_present(),
                &[false, false, false, false, true, false, false, false, false]
            );
            for cx in 0..3 {
                for cy in 0..3 {
                    for cz in 0..3 {
                        let expected = if cx == 1 && cz == 1 && cy != barrier_cy {
                            0
                        } else {
                            1
                        };
                        assert!(
                            view.blocks()[((cx * 3 + cy) * 3 + cz) * 4096
                                ..((cx * 3 + cy) * 3 + cz + 1) * 4096]
                                .iter()
                                .all(|&id| id == expected)
                        );
                    }
                }
            }
        }
        assert_eq!(
            world.mesh_stamps(Dimension::OVERWORLD, chunk).unwrap(),
            [None, None, None, None, Some(2), None, None, None, None]
        );
        assert!(matches!(
            world.mesh_view(
                Dimension::DEPTHS,
                SectionKey::try_new(chunk, 0).unwrap(),
                &registry
            ),
            Err(ClientError::InvalidInput)
        ));
        world
            .apply_snapshot(&snapshot(Dimension::DEPTHS, chunk, 1, [2; 24]), &limits)
            .unwrap();
        let depth = world
            .mesh_view(
                Dimension::DEPTHS,
                SectionKey::try_new(chunk, 0).unwrap(),
                &registry,
            )
            .unwrap();
        assert_eq!(depth.blocks()[13 * 4096], 2);
        assert_eq!(depth.heights()[4], [319; 256]);
        assert_eq!(
            world.mesh_stamps(Dimension::DEPTHS, chunk).unwrap()[4],
            Some(1)
        );
    }

    #[test]
    fn mesh_coordinate_overflow_rejects_actual_content_without_wrapping() {
        let limits = ClientLimits::try_new().unwrap();
        let registry = mesh_registry().unwrap();
        for chunk in [
            ChunkPos::new(i32::MAX, 0),
            ChunkPos::new(0, i32::MIN),
            ChunkPos::new(i32::MAX / 16, 0),
            ChunkPos::new(0, i32::MIN / 16),
        ] {
            let mut world = WorldConfirmed::default();
            world
                .apply_snapshot(&snapshot(Dimension::OVERWORLD, chunk, 1, [0; 24]), &limits)
                .unwrap();
            assert!(matches!(
                world.mesh_view(
                    Dimension::OVERWORLD,
                    SectionKey::try_new(chunk, 1).unwrap(),
                    &registry
                ),
                Err(ClientError::InvalidInput)
            ));
            if chunk.x() == i32::MAX || chunk.z() == i32::MIN {
                assert_eq!(
                    world.mesh_stamps(Dimension::OVERWORLD, chunk),
                    Err(ClientError::InvalidInput)
                );
            }
        }
    }

    #[test]
    fn mesh_stamps_follow_immutable_content_after_metadata_changes_and_forget() {
        let limits = ClientLimits::try_new().unwrap();
        let chunk = ChunkPos::new(-2, -7);
        let neighbor = ChunkPos::new(-1, -8);
        let mut world = WorldConfirmed::default();
        world
            .apply_snapshot(&snapshot(Dimension::OVERWORLD, chunk, 1, [0; 24]), &limits)
            .unwrap();
        world
            .apply_snapshot(
                &snapshot(Dimension::OVERWORLD, neighbor, 5, [0; 24]),
                &limits,
            )
            .unwrap();
        world = world.with_chunk(Dimension::OVERWORLD, neighbor, 99);
        assert_eq!(
            world.mesh_stamps(Dimension::OVERWORLD, chunk).unwrap()[6],
            Some(5)
        );
        world.remove_chunk(Dimension::OVERWORLD, neighbor);
        assert_eq!(
            world.mesh_stamps(Dimension::OVERWORLD, chunk).unwrap()[6],
            None
        );
    }

    #[test]
    fn refused_snapshot_and_delta_preserve_mesh_content_heights_stamps_and_charge() {
        let limits = ClientLimits::try_new().unwrap();
        let registry = mesh_registry().unwrap();
        let chunk = ChunkPos::new(-3, -4);
        let section = SectionKey::try_new(chunk, 4).unwrap();
        let mut world = WorldConfirmed::default();
        world
            .apply_snapshot(&snapshot(Dimension::OVERWORLD, chunk, 1, [0; 24]), &limits)
            .unwrap();
        let before = world.clone();
        let old = world
            .mesh_view(Dimension::OVERWORLD, section, &registry)
            .unwrap();
        assert_eq!(
            world.apply_snapshot(
                &snapshot(Dimension::OVERWORLD, chunk, 2, [2; 24]),
                &byte_limit(world.content_bytes() - 1)
            ),
            Err(ClientError::Capacity)
        );
        assert_eq!(
            world.apply_changes(
                &delta(chunk, 1, &[(1, 0, 2, 27)]),
                &byte_limit(world.content_bytes())
            ),
            Err(ClientError::Capacity)
        );
        assert_eq!(world, before);
        let after = world
            .mesh_view(Dimension::OVERWORLD, section, &registry)
            .unwrap();
        assert_eq!(after.blocks(), old.blocks());
        assert_eq!(after.heights(), old.heights());
        assert_eq!(
            world.mesh_stamps(Dimension::OVERWORLD, chunk).unwrap()[4],
            Some(1)
        );
        assert_eq!(world.content_bytes(), before.content_bytes());
    }

    #[test]
    fn confirmed_mesh_view_runs_real_native_mesh_for_isolated_stone() {
        let limits = ClientLimits::try_new().unwrap();
        let registry = mesh_registry().unwrap();
        let chunk = ChunkPos::new(-2, -6);
        let mut world = WorldConfirmed::default();
        for dx in -1..=1 {
            for dz in -1..=1 {
                world
                    .apply_snapshot(
                        &snapshot(
                            Dimension::OVERWORLD,
                            ChunkPos::new(chunk.x() + dx, chunk.z() + dz),
                            1,
                            [0; 24],
                        ),
                        &limits,
                    )
                    .unwrap();
            }
        }
        world
            .apply_changes(&delta(chunk, 1, &[(3, 8, 11, 2)]), &limits)
            .unwrap();
        let view = world
            .mesh_view(
                Dimension::OVERWORLD,
                SectionKey::try_new(chunk, 4).unwrap(),
                &registry,
            )
            .unwrap();
        assert_eq!(view.heights()[4][179], 8);
        assert_eq!(view.heights()[0][179], -65);
        let mut scratch = MeshScratch::try_new().unwrap();
        let mut quads = [MeshQuad::default(); 64];
        let count = NativeMesh
            .mesh(&view.as_mesh_view(), &mut scratch, &mut quads)
            .unwrap();
        assert_eq!(count, 6);
        assert!(quads[..count].iter().all(|quad| quad.material() == 0));
    }
}
