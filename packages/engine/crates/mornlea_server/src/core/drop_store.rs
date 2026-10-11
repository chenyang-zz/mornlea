//! Fixed persistent drop slots and bounded tick-local rehearsal.

use mornlea_domain::{
    BlockPos, ChunkPos, Dimension, DropId, FiniteVec3, RejectReason, chunk_block_index,
};
use mornlea_storage::{DropSlot, ItemStack, item_stack_limit};

use super::contracts::{ChunkKey, DropBatch, DropRecord, DropSource, Resource, RuleReject};

/// Inactive generations remain owned even when the active observation is empty.
/// Counter-only aging is intentionally not a durable mutation, matching the
/// source dirty selector; any later dirty save includes the latest counters.
#[derive(Clone)]
pub(crate) struct DropState {
    pub(crate) slots: [DropSlot; 32],
    pub(crate) dirty: bool,
    records: Vec<DropRecord>,
}

impl DropState {
    pub(crate) fn new(key: ChunkKey, slots: [DropSlot; 32]) -> Self {
        let mut state = Self {
            slots,
            dirty: false,
            records: Vec::new(),
        };
        state.refresh(key);
        state
    }

    pub(crate) fn records(&self) -> &[DropRecord] {
        &self.records
    }

    fn refresh(&mut self, key: ChunkKey) {
        self.records.clear();
        for (index, slot) in self.slots.iter().enumerate().filter(|(_, s)| s.active) {
            self.records.push(DropRecord {
                id: DropId::try_new(
                    i32::from(key.dimension.get()),
                    key.pos,
                    index as u8,
                    slot.generation,
                )
                .expect("validated active slot has a nonzero generation"),
                position: center(key, slot.block_index),
                stack: slot.stack,
                pickup_delay: slot.pickup_delay_ticks,
                age: slot.age_ticks,
            });
        }
    }

    /// Fixture seeding cannot alias two generations into one physical slot.
    pub(crate) fn seed(&mut self, key: ChunkKey, record: DropRecord) -> Result<(), RuleReject> {
        // A compact save already owns the exact block index. Never reconstruct
        // it from the lossy float center when replay repeats that observation.
        if self.records.iter().any(|current| current == &record) {
            return Ok(());
        }
        let (owner, index) = location(key.dimension, record.position)?;
        if owner != key
            || drop_key(&record)? != key
            || !record.stack.is_valid()
            || record.stack.count == 0
        {
            return Err(invalid());
        }
        let slot = &mut self.slots[usize::from(record.id.slot())];
        let value = DropSlot {
            generation: record.id.generation(),
            active: true,
            stack: record.stack,
            block_index: index,
            age_ticks: record.age,
            pickup_delay_ticks: record.pickup_delay,
        };
        if (slot.active && *slot != value) || (!slot.active && slot.generation != 0) {
            return Err(RuleReject::StaleObservation);
        }
        *slot = value;
        self.refresh(key);
        Ok(())
    }

    /// All changes stay on this bounded rehearsal until the caller accepts it.
    pub(crate) fn insert(&mut self, key: ChunkKey, batch: &DropBatch) -> Result<(), RuleReject> {
        validate_batch(batch)?;
        let (owner, index) = batch_location(batch)?;
        if owner != key {
            return Err(invalid());
        }
        for stack in &batch.stacks {
            if *stack == ItemStack::default() {
                continue;
            }
            let limit = item_stack_limit(stack.item).ok_or_else(invalid)?;
            let mut remaining = stack.count;
            while remaining != 0 {
                let merged = self.slots.iter().position(|slot| {
                    slot.active
                        && slot.stack.item == stack.item
                        && slot.block_index == index
                        && slot.stack.count < limit
                });
                let position = merged
                    .or_else(|| {
                        self.slots
                            .iter()
                            .position(|slot| !slot.active && slot.generation != u32::MAX)
                    })
                    .ok_or(RuleReject::Wire(RejectReason::DropCapacity))?;
                let slot = &mut self.slots[position];
                if slot.active {
                    let count = remaining.min(limit - slot.stack.count);
                    slot.stack.count += count;
                    slot.pickup_delay_ticks = slot.pickup_delay_ticks.max(batch.pickup_delay);
                    remaining -= count;
                } else {
                    let count = remaining.min(limit);
                    *slot = DropSlot {
                        generation: slot.generation + 1,
                        active: true,
                        stack: ItemStack { count, ..*stack },
                        block_index: index,
                        age_ticks: 0,
                        pickup_delay_ticks: batch.pickup_delay,
                    };
                    remaining -= count;
                }
                self.dirty = true;
            }
        }
        self.refresh(key);
        Ok(())
    }

    pub(crate) fn patch(
        &mut self,
        key: ChunkKey,
        before: &DropRecord,
        after: Option<&DropRecord>,
    ) -> Result<(), RuleReject> {
        if self.records.iter().find(|record| record.id == before.id) != Some(before) {
            return Err(RuleReject::StaleObservation);
        }
        let slot = &mut self.slots[usize::from(before.id.slot())];
        match after {
            Some(after) => {
                if after.id != before.id
                    || after.position != before.position
                    || after.stack.item != before.stack.item
                    || after.stack.durability != before.stack.durability
                    || !after.stack.is_valid()
                    || after.stack.count == 0
                    || after.stack.count > before.stack.count
                {
                    return Err(invalid());
                }
                self.dirty |= after.stack.count != before.stack.count;
                slot.stack = after.stack;
                slot.age_ticks = after.age;
                slot.pickup_delay_ticks = after.pickup_delay;
            }
            None => {
                *slot = DropSlot {
                    generation: slot.generation,
                    ..Default::default()
                };
                self.dirty = true;
            }
        }
        self.refresh(key);
        Ok(())
    }
}

pub(crate) fn validate_batch(batch: &DropBatch) -> Result<(), RuleReject> {
    if batch.stacks.len() > 36 {
        return Err(RuleReject::ResourceFull(Resource::RuleEffects));
    }
    if batch.stacks.iter().any(|stack| !stack.is_valid()) {
        return Err(invalid());
    }
    batch_location(batch)?;
    Ok(())
}

/// Block producers retain their integer authority target; actor-origin drops
/// floor a pose. A rendered float center must never become a new block index.
pub(crate) fn batch_location(batch: &DropBatch) -> Result<(ChunkKey, u32), RuleReject> {
    let target = match batch.source {
        DropSource::Mining { target, .. } | DropSource::System { target, .. } => target,
        _ => return location(batch.dimension, batch.origin),
    };
    if !(-64..320).contains(&target.y()) {
        return Err(invalid());
    }
    let expected = [
        target.x() as f32 + 0.5,
        target.y() as f32 + 0.5,
        target.z() as f32 + 0.5,
    ];
    if batch.origin.get() != expected {
        return Err(invalid());
    }
    Ok((
        ChunkKey {
            dimension: batch.dimension,
            pos: ChunkPos::new(target.x() >> 4, target.z() >> 4),
        },
        chunk_block_index(target),
    ))
}

pub(crate) fn drop_key(record: &DropRecord) -> Result<ChunkKey, RuleReject> {
    let raw = u8::try_from(record.id.dimension()).map_err(|_| invalid())?;
    Ok(ChunkKey {
        dimension: Dimension::new(raw).map_err(|_| invalid())?,
        pos: record.id.chunk(),
    })
}

pub(crate) fn location(
    dimension: Dimension,
    position: FiniteVec3,
) -> Result<(ChunkKey, u32), RuleReject> {
    let raw = position.get().map(|value| f64::from(value).floor());
    if raw
        .iter()
        .any(|value| *value < f64::from(i32::MIN) || *value > f64::from(i32::MAX))
        || !(-64.0..320.0).contains(&raw[1])
    {
        return Err(invalid());
    }
    let pos = BlockPos::new(raw[0] as i32, raw[1] as i32, raw[2] as i32);
    Ok((
        ChunkKey {
            dimension,
            pos: ChunkPos::new(pos.x() >> 4, pos.z() >> 4),
        },
        chunk_block_index(pos),
    ))
}

fn center(key: ChunkKey, index: u32) -> FiniteVec3 {
    // The source inverse index wraps in int32 before converting to float32;
    // adding the half cell in float64 would also change large-coordinate rounding.
    let x = key
        .pos
        .x()
        .wrapping_shl(4)
        .wrapping_add((index % 16) as i32);
    let z = key
        .pos
        .z()
        .wrapping_shl(4)
        .wrapping_add(((index / 16) % 16) as i32);
    FiniteVec3::try_new([
        x as f32 + 0.5,
        (index / 256) as f32 - 64.0 + 0.5,
        z as f32 + 0.5,
    ])
    .expect("bounded slot coordinates have a finite center")
}

fn invalid() -> RuleReject {
    RuleReject::Wire(RejectReason::InvalidInput)
}
