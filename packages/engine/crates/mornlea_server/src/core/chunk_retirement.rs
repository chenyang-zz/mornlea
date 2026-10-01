//! Whole detached chunk ownership for bounded off-tick disposal.

use std::{collections::BTreeMap, fmt};

use mornlea_domain::BlockPos;

use super::{
    container_store::ContainerState,
    contracts::{BlockObservation, ChunkKey, Deadline, ServerError},
    drop_store::DropState,
    world::ReadyChunk,
};

/// Total accepted owners, including queued, started and held completions.
pub const MAX_CHUNK_RETIREMENTS: usize = 8;

/// Physical managed-body identity; revisions do not identify a body.
#[derive(Copy, Clone, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub struct RetiredChunkId {
    key: ChunkKey,
    generation: u64,
}

impl RetiredChunkId {
    pub fn key(self) -> ChunkKey {
        self.key
    }

    pub fn generation(self) -> u64 {
        self.generation
    }
}

/// Opaque whole owner. Transfer neither clones Ready roots nor visits tree cells.
/// Immutable captures may still share allocations after disposal.
///
/// ```compile_fail
/// use mornlea_server::core::chunk_retirement::RetiredChunk;
/// fn duplicate(owner: &RetiredChunk) -> RetiredChunk {
///     RetiredChunk::clone(owner)
/// }
/// ```
pub struct RetiredChunk {
    id: RetiredChunkId,
    // The later live consumer supplies production body access without exposing it.
    #[allow(dead_code)]
    body: Box<RetiredChunkBody>,
}

#[allow(dead_code)]
struct RetiredChunkBody {
    ready: ReadyChunk,
    drops: DropState,
    containers: ContainerState,
    observations: Option<BTreeMap<(ChunkKey, BlockPos), BlockObservation>>,
}

impl RetiredChunk {
    pub fn id(&self) -> RetiredChunkId {
        self.id
    }

    /// Trusted managed preparation supplies a nonzero generation. All existing
    /// owners move into one box; no detached cell traversal runs on the caller.
    #[allow(dead_code)]
    pub(crate) fn new(
        ready: ReadyChunk,
        drops: DropState,
        containers: ContainerState,
        observations: Option<BTreeMap<(ChunkKey, BlockPos), BlockObservation>>,
    ) -> Self {
        debug_assert_ne!(ready.generation, 0);
        Self {
            id: RetiredChunkId {
                key: ready.key,
                generation: ready.generation,
            },
            body: Box::new(RetiredChunkBody {
                ready,
                drops,
                containers,
                observations,
            }),
        }
    }

    /// Restores all incoming owners on refusal. Only the fixed box allocation
    /// ends on the caller; returned tree nodes remain owned by the caller.
    // The explicit tuple keeps every whole owner visible at the restoration seam.
    #[allow(clippy::type_complexity)]
    #[allow(dead_code)]
    pub(crate) fn into_parts(
        self,
    ) -> (
        ReadyChunk,
        DropState,
        ContainerState,
        Option<BTreeMap<(ChunkKey, BlockPos), BlockObservation>>,
    ) {
        let RetiredChunkBody {
            ready,
            drops,
            containers,
            observations,
        } = *self.body;
        (ready, drops, containers, observations)
    }
}

impl fmt::Debug for RetiredChunk {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RetiredChunk")
            .field("id", &self.id)
            .finish()
    }
}

/// A refusal returns the exact incoming owner without disposing its body.
pub struct RejectedRetiredChunk {
    pub error: ServerError,
    pub owner: RetiredChunk,
}

impl fmt::Debug for RejectedRetiredChunk {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RejectedRetiredChunk")
            .field("error", &self.error)
            .field("id", &self.owner.id())
            .finish()
    }
}

/// Bounded CPU disposal ownership, separate from authoritative unload policy.
/// Successful admission is irreversible ownership transfer, not destruction
/// acknowledgment. Queued, started and held completions share eight charges;
/// each charge lasts until actual disposal and scalar report collection.
///
/// Providers retain accepted bodies on their worker-side channel: Drop only
/// disconnects and must not destroy queued bodies on the caller or claim joins.
pub trait ChunkRetirementPort {
    /// Full capacity returns Capacity/ChunkRetirements with limit eight and
    /// observed nine. Closing returns InvalidState/Closing; retained duplicate
    /// identity returns InvalidInput/chunk_retirement_identity. Every refusal
    /// preserves the whole incoming owner and changes no charge.
    fn submit(&mut self, owner: RetiredChunk) -> Result<(), RejectedRetiredChunk>;

    /// Moves at most min(max_reports, eight) scalar completions in admission
    /// order, freeing exactly those charges. Zero and repeated empty collection
    /// transfer nothing; no unbounded scan or retained identity history exists.
    fn collect(&mut self, max_reports: usize) -> Result<Vec<RetiredChunkId>, ServerError>;

    fn occupied(&self) -> usize;

    /// Stops admission, finishes every accepted disposal, collects charges,
    /// disconnects and explicitly joins the CPU owner. Timeout/Retire retains
    /// charge and handles for same-owner retry without cancellation or replay.
    /// Teardown panic stays Internal/"chunk retirement worker" and can never
    /// become joined success. A declaration alone proves no actual OS join.
    fn close(&mut self, deadline: Deadline) -> Result<(), ServerError>;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::{
        block_observations::ChunkBlockObservations,
        contracts::{Operation, Resource, ServerPhase},
        world::{ready_clones, reset_ready_clones},
    };
    use mornlea_domain::{ChunkPos, Dimension};
    use mornlea_storage::{Chunk, ContainerSnapshot, StorageKind};
    use std::{collections::VecDeque, time::Instant};

    type Cell = (ChunkKey, BlockPos);
    type Addresses = BTreeMap<Cell, (usize, usize)>;

    fn key(x: i32) -> ChunkKey {
        ChunkKey {
            dimension: Dimension::OVERWORLD,
            pos: ChunkPos::new(x, 0),
        }
    }

    fn owner(x: i32) -> (RetiredChunk, Addresses) {
        let key = key(x);
        let chunk = Chunk {
            sections: vec![
                ContainerSnapshot {
                    kind: StorageKind::Single,
                    bits: 0,
                    single: 0,
                    palette: vec![],
                    packed: vec![],
                };
                24
            ],
            drops: vec![Default::default(); 32],
            furnaces: vec![Default::default(); 32],
            chests: vec![Default::default(); 16],
        };
        let containers = ContainerState::new(&chunk, 9);
        let drops = DropState::new(key, chunk.drops.as_slice().try_into().unwrap());
        let ready = ReadyChunk::try_new(key, 7, 9, chunk).unwrap();
        let mut observations = ChunkBlockObservations::new();
        for index in 0..4096 {
            let pos = BlockPos::new(x * 16 + index % 16, -64 + index / 256, index / 16 % 16);
            observations.insert(
                (key, pos),
                BlockObservation::try_new(key, 7, 9 + index as u64, pos, 4).unwrap(),
            );
        }
        let addresses = (&observations)
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
        reset_ready_clones();
        let owner = RetiredChunk::new(ready, drops, containers, observations.take_chunk(key));
        assert!(observations.is_empty());
        assert_eq!(ready_clones(), 0);
        (owner, addresses)
    }

    fn preserved(owner: RetiredChunk, x: i32, addresses: &Addresses) {
        assert_eq!(owner.id().key(), key(x));
        assert_eq!(owner.id().generation(), 7);
        let (ready, drops, containers, observations) = owner.into_parts();
        assert_eq!(
            (ready.key, ready.generation, ready.revision),
            (key(x), 7, 9)
        );
        assert_eq!(drops.slots, [Default::default(); 32]);
        assert!(drops.records().is_empty());
        assert!(!drops.dirty);
        assert_eq!(containers.furnaces, [Default::default(); 32]);
        assert_eq!(containers.chests, [Default::default(); 16]);
        assert!(!containers.dirty);
        let observations = observations.unwrap();
        assert_eq!(observations.len(), 4096);
        for (cell, value) in &observations {
            assert_eq!(
                (
                    std::ptr::from_ref(cell) as usize,
                    std::ptr::from_ref(value) as usize
                ),
                addresses[cell],
            );
            let index = (cell.1.y() + 64) * 256 + cell.1.z() * 16 + cell.1.x() - x * 16;
            assert_eq!(
                *value,
                BlockObservation::try_new(key(x), 7, 9 + index as u64, cell.1, 4).unwrap()
            );
        }
        assert_eq!(ready_clones(), 0);
    }

    struct Record {
        id: RetiredChunkId,
        owner: Option<RetiredChunk>,
        complete: bool,
    }

    /// This deterministic double executes ownership obligations without a CPU
    /// thread. Its explicit_joined flag is only a simulated lifecycle result.
    #[derive(Default)]
    struct OwnershipDouble {
        records: VecDeque<Record>,
        closing: bool,
        explicit_joined: bool,
    }

    impl OwnershipDouble {
        fn finish_next(&mut self) -> Option<RetiredChunkId> {
            let record = self.records.iter_mut().find(|record| !record.complete)?;
            drop(record.owner.take());
            record.complete = true;
            Some(record.id)
        }
    }

    impl ChunkRetirementPort for OwnershipDouble {
        fn submit(&mut self, owner: RetiredChunk) -> Result<(), RejectedRetiredChunk> {
            let error = if self.closing {
                Some(ServerError::InvalidState {
                    phase: ServerPhase::Closing,
                })
            } else if self.occupied() == MAX_CHUNK_RETIREMENTS {
                Some(ServerError::Capacity {
                    resource: Resource::ChunkRetirements,
                    limit: 8,
                    observed: 9,
                })
            } else if self.records.iter().any(|record| record.id == owner.id()) {
                Some(ServerError::InvalidInput {
                    field: "chunk_retirement_identity",
                })
            } else {
                None
            };
            if let Some(error) = error {
                return Err(RejectedRetiredChunk { error, owner });
            }
            self.records.push_back(Record {
                id: owner.id(),
                owner: Some(owner),
                complete: false,
            });
            Ok(())
        }

        fn collect(&mut self, max_reports: usize) -> Result<Vec<RetiredChunkId>, ServerError> {
            let mut reports = Vec::new();
            while reports.len() < max_reports.min(MAX_CHUNK_RETIREMENTS)
                && self.records.front().is_some_and(|record| record.complete)
            {
                reports.push(self.records.pop_front().unwrap().id);
            }
            Ok(reports)
        }

        fn occupied(&self) -> usize {
            self.records.len()
        }

        fn close(&mut self, _deadline: Deadline) -> Result<(), ServerError> {
            self.closing = true;
            if self.records.iter().any(|record| !record.complete) {
                return Err(ServerError::Timeout {
                    operation: Operation::Retire,
                });
            }
            self.collect(MAX_CHUNK_RETIREMENTS)?;
            self.explicit_joined = true;
            Ok(())
        }
    }

    #[test]
    fn factory_moves_every_owner_and_debug_exposes_only_identity() {
        let (owner, addresses) = owner(3);
        assert_eq!(
            format!("{owner:?}"),
            format!("RetiredChunk {{ id: {:?} }}", owner.id())
        );
        preserved(owner, 3, &addresses);
    }

    #[test]
    fn eight_charges_include_started_held_and_queued_and_refusal_preserves_nodes() {
        let mut double = OwnershipDouble::default();
        let mut ids = vec![];
        for x in 0..8 {
            let (owner, _) = owner(x);
            ids.push(owner.id());
            double.submit(owner).unwrap();
        }
        // One started label and six queued bodies accompany one held report.
        assert_eq!(double.finish_next(), Some(ids[0]));
        assert!(double.records[1].owner.is_some());
        assert_eq!(
            double.records.iter().filter(|r| r.owner.is_some()).count(),
            7
        );
        assert_eq!(double.occupied(), 8);
        let (incoming, addresses) = owner(8);
        let rejected = double.submit(incoming).unwrap_err();
        assert_eq!(
            rejected.error,
            ServerError::Capacity {
                resource: Resource::ChunkRetirements,
                limit: 8,
                observed: 9
            }
        );
        assert_eq!(double.occupied(), 8);
        preserved(rejected.owner, 8, &addresses);
        let (incoming, addresses) = owner(8);
        let rejected = double.submit(incoming).unwrap_err();
        assert_eq!(double.collect(1).unwrap(), vec![ids[0]]);
        assert_eq!(double.occupied(), 7);
        assert_eq!(ready_clones(), 0);
        double.submit(rejected.owner).unwrap();
        assert_eq!(double.occupied(), 8);
        let retained = double.records.back().unwrap().owner.as_ref().unwrap();
        assert_eq!(
            retained.body.observations.as_ref().unwrap().len(),
            addresses.len()
        );
        for (cell, value) in retained.body.observations.as_ref().unwrap() {
            assert_eq!(
                (
                    std::ptr::from_ref(cell) as usize,
                    std::ptr::from_ref(value) as usize
                ),
                addresses[cell]
            );
        }
    }

    #[test]
    fn collection_is_bounded_fifo_and_finish_skips_held_reports() {
        let mut double = OwnershipDouble::default();
        let mut ids = vec![];
        for x in 0..8 {
            let (owner, _) = owner(x);
            ids.push(owner.id());
            double.submit(owner).unwrap();
        }
        assert!(double.collect(8).unwrap().is_empty());
        assert_eq!(double.occupied(), 8);
        for id in &ids[..3] {
            assert_eq!(double.finish_next(), Some(*id));
        }
        assert!(double.collect(0).unwrap().is_empty());
        assert_eq!(double.occupied(), 8);
        assert_eq!(double.collect(1).unwrap(), ids[..1]);
        assert_eq!(double.occupied(), 7);
        assert_eq!(double.collect(usize::MAX).unwrap(), ids[1..3]);
        assert_eq!(double.occupied(), 5);
        assert!(double.collect(8).unwrap().is_empty());
        for id in &ids[3..] {
            assert_eq!(double.finish_next(), Some(*id));
        }
        assert_eq!(double.finish_next(), None);
        assert_eq!(double.occupied(), 5);
        assert_eq!(double.collect(usize::MAX).unwrap(), ids[3..]);
        assert!(double.collect(8).unwrap().is_empty());
        assert_eq!(double.finish_next(), None);
        assert_eq!(double.occupied(), 0);
    }

    #[test]
    fn duplicate_checked_identity_returns_incoming_body_and_retains_original() {
        let mut double = OwnershipDouble::default();
        let (original, original_addresses) = owner(0);
        let id = original.id();
        double.submit(original).unwrap();
        let (incoming, addresses) = owner(0);
        assert_eq!(incoming.id(), id);
        let rejected = double.submit(incoming).unwrap_err();
        assert_eq!(
            rejected.error,
            ServerError::InvalidInput {
                field: "chunk_retirement_identity"
            }
        );
        assert_eq!(double.occupied(), 1);
        assert_eq!(
            format!("{rejected:?}"),
            format!(
                "RejectedRetiredChunk {{ error: {:?}, id: {:?} }}",
                rejected.error, id
            )
        );
        preserved(rejected.owner, 0, &addresses);
        let original = double.records.pop_front().unwrap().owner.unwrap();
        reset_ready_clones();
        preserved(original, 0, &original_addresses);
    }

    #[test]
    fn closed_submission_returns_the_whole_owner() {
        let mut double = OwnershipDouble::default();
        double.close(Deadline::at(Instant::now())).unwrap();
        assert!(double.explicit_joined);
        let (incoming, addresses) = owner(4);
        let rejected = double.submit(incoming).unwrap_err();
        assert_eq!(
            rejected.error,
            ServerError::InvalidState {
                phase: ServerPhase::Closing
            }
        );
        assert_eq!(double.occupied(), 0);
        preserved(rejected.owner, 4, &addresses);
    }

    #[test]
    fn timeout_retains_owners_and_held_reports_for_same_double_retry() {
        let mut double = OwnershipDouble::default();
        let (first, _) = owner(0);
        let first_id = first.id();
        double.submit(first).unwrap();
        let (second, addresses) = owner(1);
        let second_id = second.id();
        double.submit(second).unwrap();
        assert_eq!(double.finish_next(), Some(first_id));
        let deadline = Deadline::at(Instant::now());
        assert_eq!(
            double.close(deadline),
            Err(ServerError::Timeout {
                operation: Operation::Retire
            })
        );
        assert!(!double.explicit_joined);
        assert_eq!(double.occupied(), 2);
        assert_eq!(double.records[0].id, first_id);
        assert!(double.records[0].owner.is_none());
        assert_eq!(double.records[1].id, second_id);
        let retained = double.records[1].owner.as_ref().unwrap();
        for (cell, value) in retained.body.observations.as_ref().unwrap() {
            assert_eq!(
                (
                    std::ptr::from_ref(cell) as usize,
                    std::ptr::from_ref(value) as usize
                ),
                addresses[cell]
            );
        }
        let (incoming, incoming_addresses) = owner(2);
        let rejected = double.submit(incoming).unwrap_err();
        assert_eq!(
            rejected.error,
            ServerError::InvalidState {
                phase: ServerPhase::Closing
            }
        );
        preserved(rejected.owner, 2, &incoming_addresses);
        assert_eq!(double.finish_next(), Some(second_id));
        assert_eq!(double.occupied(), 2);
        double.close(deadline).unwrap();
        assert!(double.explicit_joined);
        assert_eq!(double.occupied(), 0);
        assert!(double.collect(8).unwrap().is_empty());
        double.close(deadline).unwrap();
    }
}
