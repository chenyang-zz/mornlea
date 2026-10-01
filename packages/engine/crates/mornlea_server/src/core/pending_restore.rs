//! Bounded restoration progress without actor, scheduling or publication ownership.

use super::actor_placement::{
    PlacementWorld, RestoreCandidate, SpawnColumn, SpawnSite, SpawnTier, candidate_chunks,
    scan_spawn_column, spawn_chunk_keys, spawn_columns, validate_restore,
};
use super::contracts::{ChunkKey, ServerError};
use mornlea_domain::{ChunkPos, Dimension};
use std::collections::BTreeSet;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RestoreKind {
    Player,
    Companion,
}
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RestoreActivation {
    pub dimension: Dimension,
    pub position: [f32; 3],
    pub on_ground: bool,
}
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum RestoreProgress {
    Waiting,
    Exhausted,
    Activated(RestoreActivation),
}

/// Retains bounded source scan state only; every world observation is borrowed.
#[derive(Debug)]
pub struct PendingRestore {
    kind: RestoreKind,
    anchor: ChunkPos,
    radius: u8,
    spawn_dimension: Dimension,
    candidates: Vec<RestoreCandidate>,
    next_restore: usize,
    restore_wanted: BTreeSet<ChunkKey>,
    columns: Vec<SpawnColumn>,
    column_chunk_positions: Vec<ChunkPos>,
    next_column: usize,
    spawn_wanted: BTreeSet<ChunkPos>,
    best: Option<SpawnSite>,
    exhausted_revisions: Option<Vec<u64>>,
    completed: bool,
}
impl PendingRestore {
    /// Checks candidate and square bounds before retaining any scan state.
    pub fn try_new(
        kind: RestoreKind,
        spawn_dimension: Dimension,
        anchor: ChunkPos,
        radius: u8,
        candidates: Vec<RestoreCandidate>,
    ) -> Result<Self, ServerError> {
        let maximum = match kind {
            RestoreKind::Player => 2,
            RestoreKind::Companion => 1,
        };
        if candidates.len() > maximum {
            return Err(ServerError::InvalidInput {
                field: "restore_candidates",
            });
        }
        if kind == RestoreKind::Companion && radius != 16 {
            return Err(ServerError::InvalidInput {
                field: "companion_spawn_radius",
            });
        }
        let columns = spawn_columns(anchor, radius)?;
        let keys = spawn_chunk_keys(spawn_dimension, &columns)?;
        // A consuming map can reuse the square-sized key buffer after deduplication.
        // Retain a fresh allocation sized only to the sorted unique positions.
        let mut column_chunk_positions = Vec::with_capacity(keys.len());
        for key in keys {
            column_chunk_positions.push(key.pos);
        }
        // Bound retained allocation independently of the caller's spare capacity.
        let mut owned_candidates = Vec::with_capacity(maximum);
        owned_candidates.extend_from_slice(&candidates);
        Ok(Self {
            kind,
            anchor,
            radius,
            spawn_dimension,
            candidates: owned_candidates,
            next_restore: 0,
            restore_wanted: BTreeSet::new(),
            columns,
            column_chunk_positions,
            next_column: 0,
            spawn_wanted: BTreeSet::from([anchor]),
            best: None,
            exhausted_revisions: None,
            completed: false,
        })
    }

    /// Reinterprets dimensionless spawn retention in the current actor dimension.
    pub fn pending_keys(&self) -> Vec<ChunkKey> {
        if self.completed {
            return Vec::new();
        }
        let mut keys = self.restore_wanted.clone();
        keys.extend(self.spawn_wanted.iter().map(|pos| ChunkKey {
            dimension: self.spawn_dimension,
            pos: *pos,
        }));
        keys.into_iter().collect()
    }

    /// Executes source same-call scan cadence; trusted errors retain prior progress.
    pub fn advance(
        &mut self,
        world: &impl PlacementWorld,
        eye_height: f32,
    ) -> Result<RestoreProgress, ServerError> {
        if self.completed {
            return Err(ServerError::InvalidInput {
                field: "restore_complete",
            });
        }
        while let Some(candidate) = self.candidates.get(self.next_restore).copied() {
            match candidate_chunks(candidate) {
                Ok(keys) => self.restore_wanted.extend(keys),
                // Unrepresentable horizontal geometry is an assessed invalid pose.
                Err(ServerError::InvalidInput {
                    field: "actor_geometry",
                }) => {}
                Err(error) => return Err(error),
            }
            let check = validate_restore(world, candidate)?;
            if !check.ready {
                return Ok(RestoreProgress::Waiting);
            }
            if check.valid {
                return Ok(self.activate(RestoreActivation {
                    dimension: candidate.dimension,
                    position: candidate.position,
                    on_ground: check.on_ground,
                }));
            }
            self.next_restore += 1;
        }
        if self.kind == RestoreKind::Companion {
            self.restore_wanted.clear();
        }
        if let Some(revisions) = &self.exhausted_revisions {
            let changed =
                self.column_chunk_positions
                    .iter()
                    .zip(revisions)
                    .any(|(pos, revision)| {
                        world.ready_revision(ChunkKey {
                            dimension: self.spawn_dimension,
                            pos: *pos,
                        }) != Some(*revision)
                    });
            if !changed {
                return Ok(RestoreProgress::Exhausted);
            }
            self.next_column = 0;
            self.best = None;
            self.exhausted_revisions = None;
        }
        while let Some(column) = self.columns.get(self.next_column).copied() {
            self.spawn_wanted
                .insert(ChunkPos::new(column.x >> 4, column.z >> 4));
            let result = scan_spawn_column(world, self.spawn_dimension, column, eye_height)?;
            if !result.ready {
                return Ok(RestoreProgress::Waiting);
            }
            if let Some(site) = result.site {
                if site.tier == SpawnTier::Dry {
                    return Ok(self.activate(RestoreActivation {
                        dimension: self.spawn_dimension,
                        position: site.position,
                        on_ground: true,
                    }));
                }
                if self
                    .best
                    .is_none_or(|best| tier_rank(site.tier) < tier_rank(best.tier))
                {
                    self.best = Some(site);
                }
            }
            self.next_column += 1;
        }
        // Source keeps the earlier fallback across a Ready gap without revalidation.
        if let Some(best) = self.best {
            return Ok(self.activate(RestoreActivation {
                dimension: self.spawn_dimension,
                position: best.position,
                on_ground: true,
            }));
        }
        let revisions = self
            .column_chunk_positions
            .iter()
            .map(|pos| {
                world
                    .ready_revision(ChunkKey {
                        dimension: self.spawn_dimension,
                        pos: *pos,
                    })
                    .ok_or(ServerError::Internal {
                        invariant: "restore exhausted chunk",
                    })
            })
            .collect::<Result<Vec<_>, _>>()?;
        self.exhausted_revisions = Some(revisions);
        Ok(RestoreProgress::Exhausted)
    }

    /// Restarts only a completed player, preserving captured geometry and wanted positions.
    pub fn restart_player(
        &mut self,
        spawn_dimension: Dimension,
        candidates: Vec<RestoreCandidate>,
    ) -> Result<(), ServerError> {
        if self.kind != RestoreKind::Player || !self.completed {
            return Err(ServerError::InvalidInput {
                field: "restore_restart",
            });
        }
        if candidates.len() > 1 {
            return Err(ServerError::InvalidInput {
                field: "restore_candidates",
            });
        }
        debug_assert_eq!(
            self.columns.len(),
            (usize::from(self.radius) * 2 + 1).pow(2)
        );
        let mut owned_candidates = Vec::with_capacity(1);
        owned_candidates.extend_from_slice(&candidates);
        self.spawn_dimension = spawn_dimension;
        self.candidates = owned_candidates;
        self.next_restore = 0;
        self.restore_wanted.clear();
        self.next_column = 0;
        self.best = None;
        self.exhausted_revisions = None;
        self.completed = false;
        self.spawn_wanted.insert(self.anchor);
        Ok(())
    }

    fn activate(&mut self, activation: RestoreActivation) -> RestoreProgress {
        self.completed = true;
        self.spawn_dimension = activation.dimension;
        self.candidates.clear();
        self.next_restore = 0;
        self.restore_wanted.clear();
        self.best = None;
        self.exhausted_revisions = None;
        RestoreProgress::Activated(activation)
    }
}
fn tier_rank(tier: SpawnTier) -> u8 {
    match tier {
        SpawnTier::Dry => 0,
        SpawnTier::EyeDry => 1,
        SpawnTier::Submerged => 2,
    }
}

#[cfg(test)]
mod tests {
    use super::super::contracts::{BlockWrite, ServerLimits, SystemRule, TickBudget};
    use super::super::state::{AuthorityReadView, AuthorityState, TickContext};
    use super::super::world::{self, ReadyChunk};
    use super::*;
    use mornlea_domain::BlockPos;
    use mornlea_storage::{Chunk, ContainerSnapshot, StorageKind};
    use std::cell::{Cell, RefCell};
    use std::collections::BTreeMap;
    const D: Dimension = Dimension::OVERWORLD;
    const DEPTHS: Dimension = Dimension::DEPTHS;
    fn key(d: Dimension, x: i32, z: i32) -> ChunkKey {
        ChunkKey {
            dimension: d,
            pos: ChunkPos::new(x, z),
        }
    }
    fn candidate(d: Dimension, p: [f32; 3], support: bool) -> RestoreCandidate {
        RestoreCandidate {
            dimension: d,
            position: p,
            require_support: support,
        }
    }
    fn activation(d: Dimension, p: [f32; 3], grounded: bool) -> RestoreProgress {
        RestoreProgress::Activated(RestoreActivation {
            dimension: d,
            position: p,
            on_ground: grounded,
        })
    }
    fn invalid(field: &'static str) -> ServerError {
        ServerError::InvalidInput { field }
    }
    fn player(candidates: Vec<RestoreCandidate>) -> PendingRestore {
        PendingRestore::try_new(RestoreKind::Player, D, ChunkPos::new(0, 0), 1, candidates).unwrap()
    }

    // Sparse test blocks are certified only when their whole Ready column is known.
    #[derive(Default)]
    struct CountingWorld {
        ready: BTreeMap<ChunkKey, u64>,
        blocks: BTreeMap<(Dimension, BlockPos), u16>,
        ready_calls: Cell<usize>,
        block_calls: Cell<usize>,
        height_calls: Cell<usize>,
        trace: RefCell<Vec<(Dimension, BlockPos)>>,
        heights: RefCell<Vec<(Dimension, i32, i32)>>,
    }
    impl CountingWorld {
        fn square() -> Self {
            let mut w = Self::default();
            for x in [-1, 0] {
                for z in [-1, 0] {
                    w.ready.insert(key(D, x, z), 9);
                }
            }
            w
        }
        fn put(&mut self, d: Dimension, x: i32, y: i32, z: i32, b: u16) {
            self.blocks.insert((d, BlockPos::new(x, y, z)), b);
        }
        fn reset(&self) {
            self.ready_calls.set(0);
            self.block_calls.set(0);
            self.height_calls.set(0);
            self.trace.borrow_mut().clear();
            self.heights.borrow_mut().clear();
        }
        fn counts(&self) -> (usize, usize, usize) {
            (
                self.ready_calls.get(),
                self.block_calls.get(),
                self.height_calls.get(),
            )
        }
    }
    impl PlacementWorld for CountingWorld {
        fn ready_revision(&self, k: ChunkKey) -> Option<u64> {
            self.ready_calls.set(self.ready_calls.get() + 1);
            self.ready.get(&k).copied()
        }
        fn block_at(&self, d: Dimension, p: BlockPos) -> Option<u16> {
            self.block_calls.set(self.block_calls.get() + 1);
            // Small fixtures alone retain traces, never the maximum water reader.
            self.trace.borrow_mut().push((d, p));
            if !(-64..320).contains(&p.y()) {
                return Some(0);
            }
            self.ready.get(&key(d, p.x() >> 4, p.z() >> 4))?;
            Some(self.blocks.get(&(d, p)).copied().unwrap_or(0))
        }
        fn ready_column_height(&self, d: Dimension, x: i32, z: i32) -> Option<i32> {
            self.height_calls.set(self.height_calls.get() + 1);
            self.heights.borrow_mut().push((d, x, z));
            self.ready.get(&key(d, x >> 4, z >> 4))?;
            Some(
                self.blocks
                    .iter()
                    .filter(|((bd, p), b)| {
                        *bd == d
                            && p.x() == x
                            && p.z() == z
                            && (-64..320).contains(&p.y())
                            && **b != 0
                    })
                    .map(|((_, p), _)| p.y())
                    .max()
                    .unwrap_or(-65),
            )
        }
    }
    #[test]
    fn initial_and_current_wait() {
        let mut p = player(vec![
            candidate(DEPTHS, [160.5, 64., 160.5], false),
            candidate(D, [8.5, 64., 8.5], true),
        ]);
        assert_eq!(p.pending_keys(), vec![key(D, 0, 0)]);
        let mut w = CountingWorld::default();
        w.ready.insert(key(D, 0, 0), 9);
        w.put(D, 8, 63, 8, 2);
        assert_eq!(p.advance(&w, 1.62), Ok(RestoreProgress::Waiting));
        assert_eq!(p.next_restore, 0);
        assert_eq!(w.counts(), (1, 0, 0));
        assert_eq!(p.pending_keys(), vec![key(D, 0, 0), key(DEPTHS, 10, 10)]);
        w.ready.insert(key(DEPTHS, 10, 10), 9);
        w.put(DEPTHS, 160, 64, 160, 2);
        assert_eq!(
            p.advance(&w, 1.62),
            Ok(activation(D, [8.5, 64., 8.5], true))
        );
        assert!(p.pending_keys().is_empty());
        assert!(p.candidates.is_empty());
        assert!(p.restore_wanted.is_empty());
        w.reset();
        assert_eq!(p.advance(&w, 1.62), Err(invalid("restore_complete")));
        assert_eq!(w.counts(), (0, 0, 0));
    }
    #[test]
    fn current_airborne_vs_safe() {
        let mut w = CountingWorld::square();
        w.put(D, 8, 63, 8, 2);
        let mut p = player(vec![
            candidate(D, [8.5, 65.25, 8.5], false),
            candidate(D, [8.5, 64., 8.5], true),
        ]);
        assert_eq!(
            p.advance(&w, 1.62),
            Ok(activation(D, [8.5, 65.25, 8.5], false))
        );
        assert_eq!(w.height_calls.get(), 0);
        let mut w = CountingWorld::default();
        w.ready.insert(key(D, 0, 0), 9);
        w.put(D, 8, 63, 8, 62);
        let mut p = player(vec![
            candidate(D, [8.5, 63.5, 8.9], false),
            candidate(D, [8.5, 64., 8.9], true),
        ]);
        let check = validate_restore(&w, p.candidates[1]).unwrap();
        assert!(check.on_ground);
        assert!(!check.valid);
        w.reset();
        assert_eq!(p.advance(&w, 1.62), Ok(RestoreProgress::Waiting));
        assert_eq!((p.next_restore, p.next_column), (2, 1));
    }
    #[test]
    fn retain_before_invalid_height() {
        let w = CountingWorld::default();
        let mut p = player(vec![
            candidate(DEPTHS, [160.5, f32::MAX, 160.5], false),
            candidate(D, [192.5, f32::MAX, 192.5], true),
        ]);
        assert_eq!(p.advance(&w, 1.62), Ok(RestoreProgress::Waiting));
        assert_eq!(
            p.pending_keys(),
            vec![key(D, 0, 0), key(D, 12, 12), key(DEPTHS, 10, 10)]
        );
        assert_eq!(w.counts(), (0, 1, 1));
        let mut p = PendingRestore::try_new(
            RestoreKind::Companion,
            D,
            ChunkPos::new(0, 0),
            16,
            vec![candidate(DEPTHS, [160.5, f32::MAX, 160.5], false)],
        )
        .unwrap();
        w.reset();
        assert_eq!(p.advance(&w, 1.62), Ok(RestoreProgress::Waiting));
        assert_eq!(p.pending_keys(), vec![key(D, 0, 0)]);
        assert_eq!(w.counts(), (0, 1, 1));
        let mut p = player(vec![
            candidate(DEPTHS, [f32::NAN, 64., 160.5], false),
            candidate(D, [192.5, f32::NAN, 192.5], true),
        ]);
        w.reset();
        assert_eq!(p.advance(&w, 1.62), Ok(RestoreProgress::Waiting));
        assert_eq!(p.pending_keys(), vec![key(D, 0, 0), key(D, 12, 12)]);
        assert_eq!(w.counts(), (0, 1, 1));
    }
    #[test]
    fn cross_column_fallback_without_revalidation() {
        let mut w = CountingWorld::default();
        w.ready.insert(key(D, 0, 0), 9);
        w.put(D, 0, 63, 0, 2);
        w.put(D, 0, 64, 0, 27);
        let mut p = player(vec![]);
        assert_eq!(p.advance(&w, 1.62), Ok(RestoreProgress::Waiting));
        assert_eq!(p.next_column, 1);
        assert_eq!(
            p.best,
            Some(SpawnSite {
                position: [0.5, 64., 0.5],
                tier: SpawnTier::EyeDry
            })
        );
        for x in [-1, 0] {
            for z in [-1, 0] {
                w.ready.insert(key(D, x, z), 9);
            }
        }
        w.put(D, 0, 64, 0, 2);
        w.reset();
        assert_eq!(
            p.advance(&w, 1.62),
            Ok(activation(D, [0.5, 64., 0.5], true))
        );
        assert_eq!(w.counts(), (0, 0, 8));
        assert!(!w.heights.borrow().contains(&(D, 0, 0)));
        assert!(w.trace.borrow().is_empty());
    }
    fn tiers() -> CountingWorld {
        let mut w = CountingWorld::square();
        for (x, y, z) in [(0, 63, 0), (-1, 30, 0), (0, 70, -1)] {
            w.put(D, x, y, z, 2);
            w.put(D, x, y + 1, z, 27);
        }
        w.put(D, 0, 65, 0, 27);
        w
    }
    #[test]
    fn across_column_tiers() {
        let w = tiers();
        let mut p = player(vec![]);
        assert_eq!(
            p.advance(&w, 1.62),
            Ok(activation(D, [-0.5, 31., 0.5], true))
        );
        assert_eq!(w.height_calls.get(), 9);
        let mut w = tiers();
        w.put(D, 0, 10, 1, 2);
        let mut p = player(vec![]);
        assert_eq!(
            p.advance(&w, 1.62),
            Ok(activation(D, [0.5, 11., 1.5], true))
        );
        assert_eq!(w.height_calls.get(), 4);
        assert_eq!(w.heights.borrow().last(), Some(&(D, 0, 1)));
    }
    #[test]
    fn exhausted_revision_gate() {
        let mut w = CountingWorld::square();
        let mut p = player(vec![]);
        assert_eq!(p.advance(&w, 1.62), Ok(RestoreProgress::Exhausted));
        assert_eq!(w.counts(), (4, 0, 9));
        let before = format!("{p:?}");
        w.reset();
        assert_eq!(p.advance(&w, 1.62), Ok(RestoreProgress::Exhausted));
        assert_eq!(w.counts(), (4, 0, 0));
        assert_eq!(format!("{p:?}"), before);
        w.ready.insert(key(D, 0, 0), 10);
        w.put(D, 0, 63, 0, 2);
        w.reset();
        assert_eq!(
            p.advance(&w, 1.62),
            Ok(activation(D, [0.5, 64., 0.5], true))
        );
        assert_eq!(w.counts(), (4, 7, 1));
        let mut w = CountingWorld::square();
        let mut p = player(vec![candidate(DEPTHS, [160.5, f32::MAX, 160.5], false)]);
        assert_eq!(p.advance(&w, 1.62), Ok(RestoreProgress::Exhausted));
        assert_eq!(p.next_restore, 1);
        w.ready.remove(&key(D, -1, 0));
        w.reset();
        assert_eq!(p.advance(&w, 1.62), Ok(RestoreProgress::Waiting));
        assert_eq!(p.next_restore, 1);
        assert_eq!(p.next_column, 1);
        assert_eq!(w.counts(), (2, 1, 2));
    }
    #[test]
    fn restart_current_dimension_and_original_anchor() {
        let w = tiers();
        let mut p = player(vec![]);
        p.advance(&w, 1.62).unwrap();
        let columns = p.columns.clone();
        let positions = p.column_chunk_positions.clone();
        let wanted = p.spawn_wanted.clone();
        assert_eq!(p.restart_player(DEPTHS, vec![]), Ok(()));
        assert_eq!(p.columns, columns);
        assert_eq!(p.column_chunk_positions, positions);
        assert_eq!(p.spawn_wanted, wanted);
        assert_eq!((p.anchor, p.radius), (ChunkPos::new(0, 0), 1));
        assert_eq!(
            p.pending_keys(),
            vec![
                key(DEPTHS, -1, -1),
                key(DEPTHS, -1, 0),
                key(DEPTHS, 0, -1),
                key(DEPTHS, 0, 0)
            ]
        );
        assert_eq!(
            (
                p.next_restore,
                p.next_column,
                p.best,
                p.exhausted_revisions.as_ref(),
                p.completed
            ),
            (0, 0, None, None, false)
        );
        let before = format!("{p:?}");
        assert_eq!(p.restart_player(D, vec![]), Err(invalid("restore_restart")));
        assert_eq!(format!("{p:?}"), before);
        let mut c = PendingRestore::try_new(
            RestoreKind::Companion,
            D,
            ChunkPos::new(0, 0),
            16,
            vec![candidate(D, [8.5, 65., 8.5], false)],
        )
        .unwrap();
        c.advance(&w, 1.62).unwrap();
        let before = format!("{c:?}");
        assert_eq!(
            c.restart_player(DEPTHS, vec![]),
            Err(invalid("restore_restart"))
        );
        assert_eq!(format!("{c:?}"), before);
        let mut p = player(vec![candidate(D, [8.5, 65., 8.5], false)]);
        p.advance(&w, 1.62).unwrap();
        let before = format!("{p:?}");
        let bed = candidate(DEPTHS, [8.5, 64., 8.5], false);
        assert_eq!(
            p.restart_player(DEPTHS, vec![bed; 2]),
            Err(invalid("restore_candidates"))
        );
        assert_eq!(format!("{p:?}"), before);
        p.restart_player(DEPTHS, vec![bed]).unwrap();
        let mut w = CountingWorld::default();
        w.ready.insert(key(DEPTHS, 0, 0), 9);
        w.put(DEPTHS, 8, 63, 8, 2);
        assert_eq!(
            p.advance(&w, 1.62),
            Ok(activation(DEPTHS, [8.5, 64., 8.5], true))
        );
    }
    #[test]
    fn checked_constructor_and_errors() {
        let c = candidate(D, [8.5, 64., 8.5], false);
        for (kind, count) in [(RestoreKind::Player, 3), (RestoreKind::Companion, 2)] {
            assert_eq!(
                PendingRestore::try_new(kind, D, ChunkPos::new(i32::MAX, 0), 0, vec![c; count])
                    .unwrap_err(),
                invalid("restore_candidates")
            );
        }
        assert_eq!(
            PendingRestore::try_new(
                RestoreKind::Companion,
                D,
                ChunkPos::new(i32::MAX, 0),
                1,
                vec![]
            )
            .unwrap_err(),
            invalid("companion_spawn_radius")
        );
        for radius in [0, 65] {
            assert_eq!(
                PendingRestore::try_new(
                    RestoreKind::Player,
                    D,
                    ChunkPos::new(0, 0),
                    radius,
                    vec![]
                )
                .unwrap_err(),
                invalid("spawn_radius")
            );
        }
        assert_eq!(
            PendingRestore::try_new(
                RestoreKind::Player,
                D,
                ChunkPos::new(i32::MAX, 0),
                1,
                vec![]
            )
            .unwrap_err(),
            invalid("spawn_anchor")
        );
        let w = CountingWorld::square();
        let mut p = player(vec![]);
        assert_eq!(p.advance(&w, f32::NAN), Err(invalid("eye_height")));
        assert_eq!(w.counts(), (0, 0, 0));
        let mut p = player(vec![c]);
        assert_eq!(
            p.advance(&w, f32::NAN),
            Ok(activation(D, c.position, false))
        );
        assert_eq!(w.height_calls.get(), 0);
        let mut p = player(vec![]);
        let before = format!("{p:?}");
        assert_eq!(
            p.restart_player(D, vec![c; 2]),
            Err(invalid("restore_restart"))
        );
        assert_eq!(format!("{p:?}"), before);
    }
    #[test]
    fn retained_spawn_positions_use_unique_count_allocation() {
        let cases = [
            (RestoreKind::Player, 1, 4),
            (RestoreKind::Player, 64, 81),
            (RestoreKind::Companion, 16, 9),
        ];
        let current = candidate(D, [8.5, 65.25, 8.5], false);
        let mut owners: Vec<_> = cases
            .iter()
            .map(|(kind, radius, count)| {
                let p =
                    PendingRestore::try_new(*kind, D, ChunkPos::new(0, 0), *radius, vec![current])
                        .unwrap();
                let expected: Vec<_> = spawn_chunk_keys(D, &p.columns)
                    .unwrap()
                    .iter()
                    .map(|key| key.pos)
                    .collect();
                assert_eq!(p.column_chunk_positions, expected);
                assert_eq!(p.column_chunk_positions.len(), *count);
                p
            })
            .collect();
        let actual: Vec<_> = owners
            .iter()
            .map(|p| {
                (
                    p.column_chunk_positions.len(),
                    p.column_chunk_positions.capacity(),
                )
            })
            .collect();
        assert_eq!(actual, vec![(4, 4), (81, 81), (9, 9)]);
        let w = CountingWorld::square();
        for p in &mut owners {
            let positions = p.column_chunk_positions.clone();
            let allocation = p.column_chunk_positions.as_ptr();
            let capacity = p.column_chunk_positions.capacity();
            assert_eq!(
                p.advance(&w, 1.62),
                Ok(activation(D, current.position, false))
            );
            assert_eq!(p.column_chunk_positions, positions);
            assert_eq!(p.column_chunk_positions.as_ptr(), allocation);
            assert_eq!(p.column_chunk_positions.capacity(), capacity);
            if p.kind == RestoreKind::Player {
                p.restart_player(DEPTHS, vec![]).unwrap();
                assert!(!p.completed);
            } else {
                assert_eq!(
                    p.restart_player(DEPTHS, vec![]),
                    Err(invalid("restore_restart"))
                );
                assert!(p.completed);
            }
            assert_eq!(p.column_chunk_positions, positions);
            assert_eq!(p.column_chunk_positions.as_ptr(), allocation);
            assert_eq!(p.column_chunk_positions.capacity(), capacity);
        }
    }

    #[test]
    fn constructor_discards_caller_spare_candidate_allocation() {
        for (kind, radius, capacity, count) in [
            (RestoreKind::Player, 1, 2, 2),
            (RestoreKind::Companion, 16, 1, 1),
        ] {
            let mut candidates = Vec::with_capacity(65_536);
            let current = candidate(D, [8.5, 65.25, 8.5], false);
            let safe = candidate(DEPTHS, [16.5, 64., 16.5], true);
            candidates.push(current);
            if count == 2 {
                candidates.push(safe);
            }
            let expected = candidates.clone();
            let p =
                PendingRestore::try_new(kind, D, ChunkPos::new(0, 0), radius, candidates).unwrap();
            assert_eq!(p.candidates, expected);
            assert_eq!(p.candidates.capacity(), capacity);
            let p = PendingRestore::try_new(
                kind,
                D,
                ChunkPos::new(0, 0),
                radius,
                Vec::with_capacity(65_536),
            )
            .unwrap();
            assert!(p.candidates.is_empty());
            assert_eq!(p.candidates.capacity(), capacity);
        }
    }
    #[test]
    fn restart_discards_caller_spare_candidate_allocation() {
        let w = CountingWorld::square();
        let current = candidate(D, [8.5, 65.25, 8.5], false);
        let bed = candidate(DEPTHS, [16.5, 64., 16.5], true);
        let mut p = player(vec![current]);
        assert_eq!(
            p.advance(&w, 1.62),
            Ok(activation(D, current.position, false))
        );
        let mut candidates = Vec::with_capacity(65_536);
        candidates.push(bed);
        p.restart_player(DEPTHS, candidates).unwrap();
        assert_eq!(p.candidates, vec![bed]);
        assert_eq!(p.candidates.capacity(), 1);
        let mut p = player(vec![current]);
        p.advance(&w, 1.62).unwrap();
        p.restart_player(DEPTHS, Vec::with_capacity(65_536))
            .unwrap();
        assert!(p.candidates.is_empty());
        assert_eq!(p.candidates.capacity(), 1);
    }
    #[test]
    fn refused_oversized_candidates_preserve_completed_owner() {
        let w = CountingWorld::square();
        let current = candidate(D, [8.5, 65.25, 8.5], false);
        let mut p = player(vec![current]);
        p.advance(&w, 1.62).unwrap();
        let before = format!("{p:?}");
        let capacity = p.candidates.capacity();
        let mut candidates = Vec::with_capacity(65_536);
        candidates.extend([current; 2]);
        assert_eq!(
            p.restart_player(DEPTHS, candidates),
            Err(invalid("restore_candidates"))
        );
        assert_eq!(format!("{p:?}"), before);
        assert_eq!(p.candidates.capacity(), capacity);
        let mut candidates = Vec::with_capacity(65_536);
        candidates.extend([current; 3]);
        assert_eq!(
            PendingRestore::try_new(
                RestoreKind::Player,
                D,
                ChunkPos::new(i32::MAX, 0),
                0,
                candidates
            )
            .unwrap_err(),
            invalid("restore_candidates")
        );
    }
    // Deliberately inconsistent certificates qualify trusted-reader failure only.
    struct BrokenHeight(i32);
    impl PlacementWorld for BrokenHeight {
        fn ready_revision(&self, _k: ChunkKey) -> Option<u64> {
            None
        }
        fn block_at(&self, _d: Dimension, _p: BlockPos) -> Option<u16> {
            panic!("certificate must avoid blocks")
        }
        fn ready_column_height(&self, _d: Dimension, _x: i32, _z: i32) -> Option<i32> {
            Some(self.0)
        }
    }
    #[test]
    fn certificate_and_inconsistent_capture_errors() {
        let mut p = player(vec![]);
        assert_eq!(
            p.advance(&BrokenHeight(320), 1.62),
            Err(ServerError::Internal {
                invariant: "actor placement height"
            })
        );
        let mut p = player(vec![]);
        assert_eq!(
            p.advance(&BrokenHeight(-65), 1.62),
            Err(ServerError::Internal {
                invariant: "restore exhausted chunk"
            })
        );
        assert!(p.exhausted_revisions.is_none());
    }
    #[derive(Default)]
    struct FakeActor {
        active: bool,
        position: [f32; 3],
        reset: bool,
    }
    impl FakeActor {
        fn consume(&mut self, p: &mut PendingRestore, w: &impl PlacementWorld) -> RestoreProgress {
            assert!(!self.active);
            let progress = p.advance(w, 1.62).unwrap();
            if let RestoreProgress::Activated(a) = progress {
                self.active = true;
                self.position = a.position;
                self.reset = false;
            }
            progress
        }
    }
    #[test]
    fn full_source_cadence_consumer() {
        let mut w = CountingWorld::square();
        w.put(D, 1, 63, 1, 2);
        let mut p = player(vec![]);
        let mut actor = FakeActor {
            reset: true,
            ..Default::default()
        };
        assert_eq!(
            actor.consume(&mut p, &w),
            activation(D, [1.5, 64., 1.5], true)
        );
        assert!(actor.active);
        assert!(!actor.reset);
        assert_eq!(actor.position, [1.5, 64., 1.5]);
        assert_eq!(w.height_calls.get(), 9);
        let mut p = player(vec![]);
        let mut actor = FakeActor {
            reset: true,
            ..Default::default()
        };
        let w = CountingWorld::default();
        assert_eq!(actor.consume(&mut p, &w), RestoreProgress::Waiting);
        assert!(!actor.active);
        assert!(actor.reset);
        let w = CountingWorld::square();
        assert_eq!(actor.consume(&mut p, &w), RestoreProgress::Exhausted);
        assert!(!actor.active);
        assert!(actor.reset);
    }

    // A generic reader counts calls without expanding or cloning actual Ready data.
    struct CountingReader<W> {
        inner: W,
        ready: Cell<usize>,
        blocks: Cell<usize>,
        heights: Cell<usize>,
    }
    impl<W> CountingReader<W> {
        fn new(inner: W) -> Self {
            Self {
                inner,
                ready: Cell::new(0),
                blocks: Cell::new(0),
                heights: Cell::new(0),
            }
        }
        fn counts(&self) -> (usize, usize, usize) {
            (self.ready.get(), self.blocks.get(), self.heights.get())
        }
    }
    impl<W: PlacementWorld> PlacementWorld for CountingReader<W> {
        fn ready_revision(&self, k: ChunkKey) -> Option<u64> {
            self.ready.set(self.ready.get() + 1);
            self.inner.ready_revision(k)
        }
        fn block_at(&self, d: Dimension, p: BlockPos) -> Option<u16> {
            self.blocks.set(self.blocks.get() + 1);
            self.inner.block_at(d, p)
        }
        fn ready_column_height(&self, d: Dimension, x: i32, z: i32) -> Option<i32> {
            self.heights.set(self.heights.get() + 1);
            self.inner.ready_column_height(d, x, z)
        }
    }
    fn empty_schedules(a: &AuthorityState) {
        assert_eq!(
            (
                a.fluid_schedule().pending_fluid(D),
                a.fluid_schedule().pending_fluid(DEPTHS),
                a.farmland_schedule().pending_candidates(D),
                a.farmland_schedule().pending_candidates(DEPTHS)
            ),
            (0, 0, 0, 0)
        );
        assert_eq!(a.fluid_schedule().pending_rescans(), 0);
    }
    fn reset_owners() {
        world::reset_ready_clones();
        world::reset_materializations();
    }
    fn unchanged_owners() {
        assert_eq!((world::ready_clones(), world::materializations()), (0, 0));
    }
    fn actual_probe(v: AuthorityReadView<'_>, keys: &[ChunkKey], anchor_revision: u64, block: u16) {
        for k in keys {
            assert_eq!(
                v.ready_chunk_revision(*k),
                Some(if *k == key(D, 0, 0) {
                    anchor_revision
                } else {
                    9
                })
            );
        }
        assert_eq!(
            v.observation(D, BlockPos::new(0, 63, 0)).map(|o| o.block),
            Some(block)
        );
        assert!(v.actors().is_empty());
    }
    #[test]
    fn actual_max_radius_air_and_current_write() {
        let mut a = AuthorityState::try_new(
            ServerLimits::try_new(8, 4096, 512, 64, 64, 1_048_576).unwrap(),
            42,
        )
        .unwrap();
        empty_schedules(&a);
        let mut ctx = TickContext::harness(&mut a, TickBudget::full());
        let columns = spawn_columns(ChunkPos::new(0, 0), 64).unwrap();
        let keys = spawn_chunk_keys(D, &columns).unwrap();
        assert_eq!((columns.len(), keys.len()), (16_641, 81));
        for k in &keys {
            ctx.preload_ready_chunk(
                ReadyChunk::try_new(
                    *k,
                    1,
                    9,
                    Chunk {
                        sections: vec![
                            ContainerSnapshot {
                                kind: StorageKind::Single,
                                bits: 0,
                                single: 0,
                                palette: vec![],
                                packed: vec![]
                            };
                            24
                        ],
                        drops: vec![Default::default(); 32],
                        furnaces: vec![Default::default(); 32],
                        chests: vec![Default::default(); 16],
                    },
                )
                .unwrap(),
            );
        }
        actual_probe(ctx.read(), &keys, 9, 0);
        reset_owners();
        let mut p =
            PendingRestore::try_new(RestoreKind::Player, D, ChunkPos::new(0, 0), 64, vec![])
                .unwrap();
        let reader = CountingReader::new(ctx.read());
        assert_eq!(p.advance(&reader, 1.62), Ok(RestoreProgress::Exhausted));
        assert_eq!(reader.counts(), (81, 0, 16_641));
        assert_eq!(p.pending_keys().len(), 81);
        unchanged_owners();
        actual_probe(ctx.read(), &keys, 9, 0);
        reset_owners();
        let reader = CountingReader::new(ctx.read());
        assert_eq!(p.advance(&reader, 1.62), Ok(RestoreProgress::Exhausted));
        assert_eq!(reader.counts(), (81, 0, 0));
        unchanged_owners();
        actual_probe(ctx.read(), &keys, 9, 0);
        let observed = ctx.read().observation(D, BlockPos::new(0, 63, 0)).unwrap();
        ctx.transaction()
            .try_system(
                SystemRule::Support,
                vec![BlockWrite::try_new(observed, 2).unwrap()],
            )
            .unwrap();
        actual_probe(ctx.read(), &keys, 10, 2);
        assert_eq!(ctx.read().highest_non_air(D, 0, 0), Some(63));
        reset_owners();
        let reader = CountingReader::new(ctx.read());
        assert_eq!(
            p.advance(&reader, 1.62),
            Ok(activation(D, [0.5, 64., 0.5], true))
        );
        assert_eq!(reader.counts(), (41, 7, 1));
        unchanged_owners();
        actual_probe(ctx.read(), &keys, 10, 2);
        drop(ctx);
        empty_schedules(&a);
    }
    // Complete synthetic water returns an exact certificate and keeps no query log.
    struct WaterWorld {
        ready: BTreeMap<ChunkKey, u64>,
    }
    impl PlacementWorld for WaterWorld {
        fn ready_revision(&self, k: ChunkKey) -> Option<u64> {
            self.ready.get(&k).copied()
        }
        fn block_at(&self, d: Dimension, p: BlockPos) -> Option<u16> {
            if !(-64..320).contains(&p.y()) {
                Some(0)
            } else {
                self.ready.get(&key(d, p.x() >> 4, p.z() >> 4)).map(|_| 27)
            }
        }
        fn ready_column_height(&self, d: Dimension, x: i32, z: i32) -> Option<i32> {
            self.ready.get(&key(d, x >> 4, z >> 4)).map(|_| 319)
        }
    }
    #[test]
    fn maximum_complete_water() {
        let keys = spawn_chunk_keys(D, &spawn_columns(ChunkPos::new(0, 0), 64).unwrap()).unwrap();
        let world = WaterWorld {
            ready: keys.into_iter().map(|k| (k, 9)).collect(),
        };
        let reader = CountingReader::new(world);
        let mut p =
            PendingRestore::try_new(RestoreKind::Player, D, ChunkPos::new(0, 0), 64, vec![])
                .unwrap();
        assert_eq!(p.advance(&reader, 1.62), Ok(RestoreProgress::Exhausted));
        assert_eq!(reader.counts(), (81, 6_390_144, 16_641));
        assert_eq!(p.pending_keys().len(), 81);
        let reader = CountingReader::new(reader.inner);
        assert_eq!(p.advance(&reader, 1.62), Ok(RestoreProgress::Exhausted));
        assert_eq!(reader.counts(), (81, 0, 0));
    }
    #[test]
    fn maximum_wanted_unions_and_companion_bounds() {
        let columns = spawn_columns(ChunkPos::new(0, 0), 16).unwrap();
        let keys = spawn_chunk_keys(D, &columns).unwrap();
        assert_eq!((columns.len(), keys.len()), (1089, 9));
        let mut c = PendingRestore::try_new(
            RestoreKind::Companion,
            D,
            ChunkPos::new(0, 0),
            16,
            vec![candidate(DEPTHS, [160., 64., 160.], false)],
        )
        .unwrap();
        let w = CountingWorld::default();
        assert_eq!(c.advance(&w, 1.62), Ok(RestoreProgress::Waiting));
        assert_eq!(c.pending_keys().len(), 5);
        let mut c = PendingRestore::try_new(
            RestoreKind::Companion,
            D,
            ChunkPos::new(0, 0),
            16,
            vec![candidate(DEPTHS, [160., f32::MAX, 160.], false)],
        )
        .unwrap();
        let w = WaterWorld {
            ready: keys.into_iter().map(|k| (k, 9)).collect(),
        };
        assert_eq!(c.advance(&w, 1.62), Ok(RestoreProgress::Exhausted));
        assert_eq!(c.pending_keys().len(), 9);
        assert!(c.restore_wanted.is_empty());
        let mut p = PendingRestore::try_new(
            RestoreKind::Player,
            D,
            ChunkPos::new(0, 0),
            64,
            vec![
                candidate(DEPTHS, [160., f32::MAX, 160.], false),
                candidate(D, [192., f32::MAX, 192.], true),
            ],
        )
        .unwrap();
        let keys = spawn_chunk_keys(D, &p.columns).unwrap();
        let w = WaterWorld {
            ready: keys.into_iter().map(|k| (k, 9)).collect(),
        };
        assert_eq!(p.advance(&w, 1.62), Ok(RestoreProgress::Exhausted));
        assert_eq!(p.restore_wanted.len(), 8);
        assert_eq!(p.pending_keys().len(), 89);
    }
}
