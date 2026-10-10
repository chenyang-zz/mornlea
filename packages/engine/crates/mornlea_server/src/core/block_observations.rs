//! Sparse cell observations grouped by exact chunk ownership.

use std::collections::{BTreeMap, btree_map};
use std::iter::Flatten;
use std::ops::Index;

use mornlea_domain::BlockPos;

use super::contracts::{BlockObservation, ChunkKey};

type InnerMap = BTreeMap<(ChunkKey, BlockPos), BlockObservation>;

/// Retains cell CAS history independently of durable chunk revisions.
///
/// Production tick loans move this owner; cloning is an explicit off-tick
/// observation. Inner trees retain full keys so borrowed traversal allocates
/// nothing and preserves the flat map's chunk then cell order.
#[derive(Clone, Default, Debug, PartialEq)]
pub struct ChunkBlockObservations {
    chunks: BTreeMap<ChunkKey, InnerMap>,
    len: usize,
}

impl ChunkBlockObservations {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub fn get(&self, cell: &(ChunkKey, BlockPos)) -> Option<&BlockObservation> {
        self.chunks.get(&cell.0)?.get(cell)
    }

    pub fn insert(
        &mut self,
        cell: (ChunkKey, BlockPos),
        value: BlockObservation,
    ) -> Option<BlockObservation> {
        let original = self.chunks.entry(cell.0).or_default().insert(cell, value);
        if original.is_none() {
            self.len += 1;
        }
        original
    }

    pub fn remove(&mut self, cell: &(ChunkKey, BlockPos)) -> Option<BlockObservation> {
        let owner = self.chunks.get_mut(&cell.0)?;
        let original = owner.remove(cell)?;
        self.len -= 1;
        if owner.is_empty() {
            self.chunks.remove(&cell.0);
        }
        Some(original)
    }

    pub fn values(&self) -> impl DoubleEndedIterator<Item = &BlockObservation> {
        self.chunks.values().flat_map(BTreeMap::values)
    }

    pub fn into_values(self) -> impl DoubleEndedIterator<Item = BlockObservation> {
        self.chunks.into_values().flat_map(BTreeMap::into_values)
    }

    /// Refusal returns the original whole tree; exclusive ownership prevents overlap.
    pub(crate) fn restore_chunk(&mut self, key: ChunkKey, owner: InnerMap) {
        debug_assert!(!self.chunks.contains_key(&key));
        if owner.is_empty() {
            return;
        }
        self.len += owner.len();
        self.chunks.insert(key, owner);
    }

    /// Transfers existing tree nodes without visiting cells. The receiving
    /// retirement policy owns their eventual destruction.
    pub(crate) fn take_chunk(&mut self, key: ChunkKey) -> Option<InnerMap> {
        let owner = self.chunks.remove(&key)?;
        self.len -= owner.len();
        Some(owner)
    }
}

impl Index<&(ChunkKey, BlockPos)> for ChunkBlockObservations {
    type Output = BlockObservation;

    fn index(&self, cell: &(ChunkKey, BlockPos)) -> &Self::Output {
        self.get(cell).expect("no entry found for key")
    }
}

impl<'a> IntoIterator for &'a ChunkBlockObservations {
    type Item = (&'a (ChunkKey, BlockPos), &'a BlockObservation);
    type IntoIter = Flatten<btree_map::Values<'a, ChunkKey, InnerMap>>;

    fn into_iter(self) -> Self::IntoIter {
        self.chunks.values().flatten()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mornlea_domain::{BlockPos, ChunkPos, Dimension};
    use std::collections::BTreeMap;

    fn key(dimension: Dimension, x: i32) -> ChunkKey {
        ChunkKey {
            dimension,
            pos: ChunkPos::new(x, 0),
        }
    }

    fn observed(key: ChunkKey, pos: BlockPos, revision: u64) -> BlockObservation {
        BlockObservation::try_new(key, 7, revision, pos, 4).unwrap()
    }

    #[test]
    fn traversals_preserve_full_key_order_and_fixture_values() {
        let mut observations = ChunkBlockObservations::new();
        let mut expected = BTreeMap::new();
        for dimension in [Dimension::DEPTHS, Dimension::OVERWORLD] {
            for x in [2, -1] {
                for pos in [
                    BlockPos::new(3, 2, 1),
                    BlockPos::new(1, 3, 2),
                    BlockPos::new(1, 2, 3),
                    BlockPos::new(1, 2, 1),
                ] {
                    let cell = (key(dimension, x), pos);
                    // Detached fixtures retain supplied keys even when the value names another cell.
                    let value = observed(
                        key(Dimension::OVERWORLD, 99),
                        BlockPos::new(8, 9, 10),
                        expected.len() as u64,
                    );
                    assert_eq!(
                        observations.insert(cell, value),
                        expected.insert(cell, value)
                    );
                }
            }
        }
        assert_eq!(observations.len(), expected.len());
        assert_eq!(
            (&observations).into_iter().collect::<Vec<_>>(),
            expected.iter().collect::<Vec<_>>()
        );
        assert_eq!(
            (&observations).into_iter().rev().collect::<Vec<_>>(),
            expected.iter().rev().collect::<Vec<_>>()
        );
        assert_eq!(
            observations.values().collect::<Vec<_>>(),
            expected.values().collect::<Vec<_>>()
        );
        assert_eq!(
            observations.values().rev().collect::<Vec<_>>(),
            expected.values().rev().collect::<Vec<_>>()
        );
        assert_eq!(
            observations.clone().into_values().collect::<Vec<_>>(),
            expected.clone().into_values().collect::<Vec<_>>()
        );
        assert_eq!(
            observations.into_values().rev().collect::<Vec<_>>(),
            expected.into_values().rev().collect::<Vec<_>>()
        );
    }

    #[test]
    fn replacement_removal_and_empty_owner_pruning_preserve_len() {
        let mut observations = ChunkBlockObservations::new();
        let first = (key(Dimension::OVERWORLD, 0), BlockPos::new(1, 64, 1));
        let second = (key(Dimension::DEPTHS, 0), first.1);
        let original = observed(first.0, first.1, 5);
        let replacement = observed(first.0, first.1, 6);
        assert!(observations.is_empty());
        assert_eq!(observations.remove(&first), None);
        assert_eq!(observations.insert(first, original), None);
        assert_eq!(observations.insert(first, replacement), Some(original));
        assert_eq!(observations.len(), 1);
        assert_eq!(observations.get(&first), Some(&replacement));
        assert_eq!(observations[&first], replacement);
        assert_eq!(observations.insert(second, original), None);
        assert_eq!(observations.len(), 2);
        assert_eq!(observations.remove(&first), Some(replacement));
        assert_eq!(observations.len(), 1);
        assert!(!observations.chunks.contains_key(&first.0));
        assert_eq!(observations.remove(&first), None);
        assert_eq!(observations.remove(&second), Some(original));
        assert_eq!(observations.len(), 0);
        assert!(observations.is_empty());
        assert!(observations.chunks.is_empty());
    }

    #[test]
    #[should_panic(expected = "no entry found for key")]
    fn missing_index_preserves_map_panic() {
        let observations = ChunkBlockObservations::new();
        let _ = observations[&(key(Dimension::OVERWORLD, 0), BlockPos::new(0, 0, 0))];
    }

    #[test]
    fn restore_chunk_preserves_all_addresses_and_totals() {
        let mut observations = ChunkBlockObservations::new();
        let target = key(Dimension::OVERWORLD, 0);
        for owner in [target, key(Dimension::DEPTHS, 0)] {
            for i in 0..4096 {
                let pos = BlockPos::new(i % 16, -64 + i / 256, i / 16 % 16);
                observations.insert((owner, pos), observed(owner, pos, i as u64 + 5));
            }
        }
        let addresses: BTreeMap<_, _> = (&observations)
            .into_iter()
            .map(|(k, v)| {
                (
                    *k,
                    (
                        std::ptr::from_ref(k) as usize,
                        std::ptr::from_ref(v) as usize,
                        *v,
                    ),
                )
            })
            .collect();
        let nodes = observations.take_chunk(target).unwrap();
        observations.restore_chunk(target, nodes);
        assert_eq!(observations.len(), 8192);
        assert_eq!(
            (&observations)
                .into_iter()
                .map(|(k, _)| *k)
                .collect::<Vec<_>>(),
            addresses.keys().copied().collect::<Vec<_>>()
        );
        for (k, v) in &observations {
            assert_eq!(
                (
                    std::ptr::from_ref(k) as usize,
                    std::ptr::from_ref(v) as usize,
                    *v
                ),
                addresses[k]
            );
        }
        observations.restore_chunk(key(Dimension::DEPTHS, 9), BTreeMap::new());
        assert_eq!(observations.chunks.len(), 2);
    }

    #[test]
    fn take_chunk_moves_all_nodes_without_changing_other_owners() {
        let target = key(Dimension::OVERWORLD, 0);
        let mut observations = ChunkBlockObservations::new();
        for owner in [
            target,
            key(Dimension::OVERWORLD, -1),
            key(Dimension::DEPTHS, 0),
            key(Dimension::DEPTHS, 1),
        ] {
            for index in 0..4096 {
                let pos = BlockPos::new(
                    owner.pos.x() * 16 + index % 16,
                    -64 + index / 256,
                    (index / 16) % 16,
                );
                observations.insert((owner, pos), observed(owner, pos, 5 + index as u64));
            }
        }
        let addresses: BTreeMap<_, _> = (&observations)
            .into_iter()
            .map(|(cell, value)| {
                (
                    *cell,
                    (
                        std::ptr::from_ref(cell) as usize,
                        std::ptr::from_ref(value) as usize,
                    ),
                )
            })
            .collect();
        let expected: BTreeMap<_, _> = (&observations)
            .into_iter()
            .filter(|(cell, _)| cell.0 == target)
            .map(|(cell, value)| (*cell, *value))
            .collect();
        let detached = observations
            .take_chunk(target)
            .expect("existing chunk owner must transfer");
        assert_eq!(detached.len(), 4096);
        assert_eq!(detached, expected);
        assert_eq!(
            detached.keys().collect::<Vec<_>>(),
            expected.keys().collect::<Vec<_>>()
        );
        for (cell, value) in &detached {
            assert_eq!(
                (
                    std::ptr::from_ref(cell) as usize,
                    std::ptr::from_ref(value) as usize
                ),
                addresses[cell]
            );
        }
        assert_eq!(observations.len(), 3 * 4096);
        assert_eq!(observations.chunks.len(), 3);
        for (cell, value) in &observations {
            assert_ne!(cell.0, target);
            assert_eq!(
                (
                    std::ptr::from_ref(cell) as usize,
                    std::ptr::from_ref(value) as usize
                ),
                addresses[cell]
            );
            assert_eq!(
                *value,
                observed(
                    cell.0,
                    cell.1,
                    5 + (cell.1.y() + 64) as u64 * 256
                        + cell.1.z() as u64 * 16
                        + cell.1.x().rem_euclid(16) as u64
                )
            );
        }
        let before = observations.clone();
        assert_eq!(observations.take_chunk(target), None);
        assert_eq!(observations.take_chunk(key(Dimension::DEPTHS, 99)), None);
        assert_eq!(observations, before);
        for (cell, value) in &observations {
            assert_eq!(
                (
                    std::ptr::from_ref(cell) as usize,
                    std::ptr::from_ref(value) as usize
                ),
                addresses[cell]
            );
        }
    }
}
