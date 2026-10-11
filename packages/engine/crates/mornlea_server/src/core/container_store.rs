//! Fixed chunk-owned containers, separate from the overworld-only wire view.

use mornlea_domain::{BlockPos, ContainerKind, ContainerRef, RejectReason, chunk_block_index};
use mornlea_storage::{ChestSlot, Chunk, FurnaceSlot};

use super::contracts::{
    BlockWrite, CapturedContainer, ChunkKey, ContainerRecord, ContainerSlots, RuleReject,
};

/// Chunk dimension belongs to the map key, never to a wire view reference.
/// A rehearsal owns only these fixed arrays; source records retain the base
/// revision while every durable change shares the chunk's tick increment.
#[derive(Clone)]
pub(crate) struct ContainerState {
    pub(crate) furnaces: [FurnaceSlot; 32],
    pub(crate) chests: [ChestSlot; 16],
    pub(crate) dirty: bool,
    revision: u64,
}

impl ContainerState {
    pub(crate) fn new(chunk: &Chunk, revision: u64) -> Self {
        Self {
            furnaces: chunk
                .furnaces
                .as_slice()
                .try_into()
                .expect("validated furnace slots"),
            chests: chunk
                .chests
                .as_slice()
                .try_into()
                .expect("validated chest slots"),
            dirty: false,
            revision,
        }
    }

    /// The resident commit advances observation identity with the owning
    /// chunk and clears only this tick's durable mutation marker.
    pub(crate) fn finish_tick(&mut self, revision: u64) {
        self.revision = revision;
        self.dirty = false;
    }

    pub(crate) fn record(&self, key: ChunkKey, reference: ContainerRef) -> Option<ContainerRecord> {
        if reference.chunk() != key.pos {
            return None;
        }
        let index = usize::from(reference.slot());
        let slots = match reference.kind() {
            ContainerKind::Chest => {
                let slot = self.chests[index];
                if !slot.active || slot.generation != reference.generation() {
                    return None;
                }
                ContainerSlots::Chest(slot.items)
            }
            ContainerKind::Furnace => {
                let slot = self.furnaces[index];
                if !slot.active || slot.generation != reference.generation() {
                    return None;
                }
                ContainerSlots::Furnace {
                    slots: [slot.input, slot.fuel, slot.output],
                    fuel: u32::from(slot.burn_ticks),
                    progress: u32::from(slot.progress_ticks),
                }
            }
        };
        Some(ContainerRecord {
            reference,
            revision: self.revision,
            slots,
        })
    }

    /// Recover the physical cell only after the exact live slot is proven.
    /// Wire references carry no dimension; the owning chunk key supplies it.
    pub(crate) fn position(&self, key: ChunkKey, reference: ContainerRef) -> Option<BlockPos> {
        self.record(key, reference)?;
        let index = match reference.kind() {
            ContainerKind::Chest => self.chests[usize::from(reference.slot())].block_index,
            ContainerKind::Furnace => self.furnaces[usize::from(reference.slot())].block_index,
        };
        if index >= 16 * 16 * 384 {
            return None;
        }
        let x = i64::from(key.pos.x()) * 16 + i64::from(index & 15);
        let z = i64::from(key.pos.z()) * 16 + i64::from((index >> 4) & 15);
        Some(BlockPos::new(
            i32::try_from(x).ok()?,
            i32::try_from(index >> 8).ok()? - 64,
            i32::try_from(z).ok()?,
        ))
    }

    pub(crate) fn at(
        &self,
        key: ChunkKey,
        pos: BlockPos,
        kind: ContainerKind,
    ) -> Option<ContainerRecord> {
        if pos.x() >> 4 != key.pos.x()
            || pos.z() >> 4 != key.pos.z()
            || !(-64..320).contains(&pos.y())
        {
            return None;
        }
        let block_index = chunk_block_index(pos);
        let (index, generation) = match kind {
            ContainerKind::Chest => self
                .chests
                .iter()
                .enumerate()
                .find(|(_, slot)| slot.active && slot.block_index == block_index)
                .map(|(i, s)| (i, s.generation))?,
            ContainerKind::Furnace => self
                .furnaces
                .iter()
                .enumerate()
                .find(|(_, slot)| slot.active && slot.block_index == block_index)
                .map(|(i, s)| (i, s.generation))?,
        };
        self.record(
            key,
            ContainerRef::try_new(key.pos, kind, index as u8, generation).ok()?,
        )
    }

    pub(crate) fn references(&self, key: ChunkKey) -> Vec<ContainerRef> {
        let mut refs = Vec::new();
        for (kind, slots) in [(ContainerKind::Furnace, 32), (ContainerKind::Chest, 16)] {
            for index in 0..slots {
                let (active, generation) = match kind {
                    ContainerKind::Furnace => {
                        (self.furnaces[index].active, self.furnaces[index].generation)
                    }
                    ContainerKind::Chest => {
                        (self.chests[index].active, self.chests[index].generation)
                    }
                };
                if active {
                    refs.push(
                        ContainerRef::try_new(key.pos, kind, index as u8, generation)
                            .expect("validated active container"),
                    );
                }
            }
        }
        refs
    }

    pub(crate) fn patch(
        &mut self,
        key: ChunkKey,
        before: &ContainerRecord,
        after: &ContainerRecord,
    ) -> Result<(), RuleReject> {
        if self.record(key, before.reference).as_ref() != Some(before) {
            return Err(RuleReject::StaleObservation);
        }
        if before.reference != after.reference || before.revision != after.revision {
            return Err(invalid());
        }
        let index = usize::from(before.reference.slot());
        match (&after.slots, before.reference.kind()) {
            (ContainerSlots::Chest(items), ContainerKind::Chest) => {
                let next = ChestSlot {
                    items: *items,
                    ..self.chests[index]
                };
                if !next.is_valid() {
                    return Err(invalid());
                }
                // An accepted patch is a durable source touch even when its slots match.
                self.dirty = true;
                self.chests[index] = next;
            }
            (
                ContainerSlots::Furnace {
                    slots,
                    fuel,
                    progress,
                },
                ContainerKind::Furnace,
            ) => {
                let next = FurnaceSlot {
                    input: slots[0],
                    fuel: slots[1],
                    output: slots[2],
                    burn_ticks: u16::try_from(*fuel).map_err(|_| invalid())?,
                    progress_ticks: u8::try_from(*progress).map_err(|_| invalid())?,
                    ..self.furnaces[index]
                };
                if !next.is_valid() {
                    return Err(invalid());
                }
                // Preserve the source revision barrier for inventory-only transfers.
                self.dirty = true;
                self.furnaces[index] = next;
            }
            _ => return Err(invalid()),
        }
        Ok(())
    }

    pub(crate) fn transition(
        &mut self,
        key: ChunkKey,
        write: &BlockWrite,
        captures: &[CapturedContainer],
    ) -> Result<(), RuleReject> {
        if write.observed.block == write.replacement {
            return Ok(());
        }
        if let Some(kind) = kind(write.observed.block) {
            let current = self
                .at(key, write.observed.pos, kind)
                .ok_or(RuleReject::StaleObservation)?;
            if !captures
                .iter()
                .any(|capture| capture.key == key && capture.record == current)
            {
                return Err(RuleReject::StaleObservation);
            }
            let index = usize::from(current.reference.slot());
            match kind {
                ContainerKind::Chest => {
                    self.chests[index] = ChestSlot {
                        generation: current.reference.generation(),
                        ..Default::default()
                    }
                }
                ContainerKind::Furnace => {
                    self.furnaces[index] = FurnaceSlot {
                        generation: current.reference.generation(),
                        ..Default::default()
                    }
                }
            };
            self.dirty = true;
        }
        if let Some(kind) = kind(write.replacement) {
            let block_index = chunk_block_index(write.observed.pos);
            if self.at(key, write.observed.pos, kind).is_some() {
                return Err(RuleReject::Wire(RejectReason::ContainerCapacity));
            }
            match kind {
                ContainerKind::Chest => {
                    let slot = self
                        .chests
                        .iter_mut()
                        .find(|slot| !slot.active && slot.generation != u32::MAX)
                        .ok_or(RuleReject::Wire(RejectReason::ContainerCapacity))?;
                    *slot = ChestSlot {
                        generation: slot.generation + 1,
                        active: true,
                        block_index,
                        ..Default::default()
                    };
                }
                ContainerKind::Furnace => {
                    let slot = self
                        .furnaces
                        .iter_mut()
                        .find(|slot| !slot.active && slot.generation != u32::MAX)
                        .ok_or(RuleReject::Wire(RejectReason::ContainerCapacity))?;
                    *slot = FurnaceSlot {
                        generation: slot.generation + 1,
                        active: true,
                        block_index,
                        ..Default::default()
                    };
                }
            }
            self.dirty = true;
        }
        Ok(())
    }
}

/// Sparse harness records pass the same storage payload boundary as Ready slots.
pub(crate) fn validate_slots(record: &ContainerRecord) -> Result<(), RuleReject> {
    let valid = match (&record.slots, record.reference.kind()) {
        (ContainerSlots::Chest(items), ContainerKind::Chest) => ChestSlot {
            generation: record.reference.generation(),
            active: true,
            items: *items,
            ..Default::default()
        }
        .is_valid(),
        (
            ContainerSlots::Furnace {
                slots,
                fuel,
                progress,
            },
            ContainerKind::Furnace,
        ) => FurnaceSlot {
            generation: record.reference.generation(),
            active: true,
            input: slots[0],
            fuel: slots[1],
            output: slots[2],
            burn_ticks: u16::try_from(*fuel).map_err(|_| invalid())?,
            progress_ticks: u8::try_from(*progress).map_err(|_| invalid())?,
            ..Default::default()
        }
        .is_valid(),
        _ => false,
    };
    if valid { Ok(()) } else { Err(invalid()) }
}

pub(crate) fn kind(block: u16) -> Option<ContainerKind> {
    match block {
        9 => Some(ContainerKind::Furnace),
        11 => Some(ContainerKind::Chest),
        _ => None,
    }
}
fn invalid() -> RuleReject {
    RuleReject::Wire(RejectReason::InvalidInput)
}
