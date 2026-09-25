//! One reusable 24-section chunk for aggregate admission tests.

use mornlea_storage::{
    ChestSlot, Chunk, ChunkKey, ChunkSave, ContainerSnapshot, DropSlot, FurnaceSlot, ItemStack,
    StorageKind,
};

#[allow(dead_code)]
pub const LAST_BLOCK_INDEX: u32 = 24 * 4096 - 1;

pub fn empty_save() -> ChunkSave {
    ChunkSave {
        key: ChunkKey {
            dimension: 0,
            x: 0,
            z: 0,
        },
        revision: 1,
        chunk: Chunk {
            sections: (0..24)
                .map(|_| ContainerSnapshot {
                    kind: StorageKind::Single,
                    bits: 0,
                    single: 0,
                    palette: Vec::new(),
                    packed: Vec::new(),
                })
                .collect(),
            drops: vec![DropSlot::default(); 32],
            furnaces: vec![FurnaceSlot::default(); 32],
            chests: vec![ChestSlot::default(); 16],
        },
    }
}

pub fn set_section_block(save: &mut ChunkSave, index: u32, block: u16) {
    let section = (index as usize) / 4096;
    save.chunk.sections[section].single = block;
}

pub fn activate_furnace(save: &mut ChunkSave, slot: usize, block_index: u32) {
    save.chunk.furnaces[slot] = FurnaceSlot {
        generation: 1,
        active: true,
        block_index,
        input: ItemStack::default(),
        fuel: ItemStack::default(),
        output: ItemStack::default(),
        progress_ticks: 0,
        burn_ticks: 0,
    };
}

pub fn activate_chest(save: &mut ChunkSave, slot: usize, block_index: u32) {
    save.chunk.chests[slot] = ChestSlot {
        generation: 1,
        active: true,
        block_index,
        items: [ItemStack::default(); 27],
    };
}
