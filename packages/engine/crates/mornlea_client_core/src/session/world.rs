//! Immutable compact block ownership shared by prediction, preparation and prompts.
//! Only mirror admission replaces a column; readers never maintain a second world.

use super::WorldConfirmed;
use crate::{ClientError, ClientLimits};
use mornlea_domain::{
    BlockChanges, BlockPos, ChunkSnapshot, Dimension, PalettedSection, chunk_block_index,
};
use mornlea_protocol::{MAX_Y, MIN_Y};
use std::sync::Arc;

#[derive(Clone, Debug, PartialEq)]
pub(super) struct ChunkContent {
    sections: Box<[Arc<PalettedSection>; 24]>,
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

impl WorldConfirmed {
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
        self.content
            .insert(key, Arc::new(ChunkContent { sections, bytes }));
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
        self.content
            .insert(key, Arc::new(ChunkContent { sections, bytes }));
        self.chunks.insert(key, changes.new_revision());
        self.content_bytes = next;
        Ok(())
    }
}
