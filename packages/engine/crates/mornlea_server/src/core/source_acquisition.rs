//! Temporary consumer-only source acquisition caller.
//!
//! Plan 109 tests/API checkpoint only: this struct is an intentionally
//! unaccepted consumer-only borrowed caller. It owns one existing
//! [`ChunkDriver`] and borrows the background `AutosaveScheduler` and
//! `GenerationPool` without taking, closing or cancelling those owners.
//! `advance` drives the real providers once, polls the driver once and runs
//! the ordinary `AuthorityState::advance_tick`; it derives no source goals,
//! starts no load or generation and replaces no wants, so the appended
//! primary cases fail on actual automatic behavior rather than on fixture
//! declarations. Never read this stub as completed goal integration: the
//! producer phase owns the frozen private goal book and admission seams. The
//! frozen goal book structure and its pure priority selection live below;
//! reducer threading, whole-want reconcile and provider admission follow.

use super::chunk_driver::{ChunkDriver, ChunkPollReport};
use super::contracts::{ChunkKey, Deadline, DiskBackend, ServerError, TickBudget, TickPublication};
use super::generation_worker::GenerationPool;
use super::state::AuthorityState;
use crate::store::scheduler::AutosaveScheduler;
use mornlea_domain::{ChunkPos, Dimension};
use std::collections::{BTreeMap, BTreeSet, VecDeque};

/// Which real provider kind one successful start used.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SourceChunkKind {
    Load,
    Generate,
}

/// One successful actual provider start, never a merely selected job.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SourceChunkStart {
    pub key: ChunkKey,
    pub kind: SourceChunkKind,
}

/// One nonblocking pass over the borrowed owners plus the ordinary tick.
pub struct SourceAcquisitionTick {
    pub publication: TickPublication,
    pub poll: ChunkPollReport,
    pub started: Vec<SourceChunkStart>,
    pub queued: usize,
    pub first_error: Option<ServerError>,
}

/// Owner of one chunk driver plus the frozen automatic source goal book.
#[derive(Default)]
pub struct SourceAcquisition {
    driver: ChunkDriver,
    goals: SourceGoals,
}

impl SourceAcquisition {
    pub fn new() -> Self {
        Self::default()
    }
    /// Forwards the driver's load ownership count.
    pub fn pending_loads(&self) -> usize {
        self.driver.pending_loads()
    }
    /// Forwards the driver's CPU ownership count.
    pub fn pending_generations(&self) -> usize {
        self.driver.pending_generations()
    }
    /// Retained unstarted FIFO candidate count.
    pub fn pending_candidates(&self) -> usize {
        self.goals.pending.len()
    }
    /// Drives the borrowed providers once, polls the driver once, then runs
    /// the ordinary tick. The unused deadline is reserved for the producer
    /// phase's bounded admission; nothing here blocks on it.
    pub fn advance<B: DiskBackend>(
        &mut self,
        state: &mut AuthorityState,
        store: &mut AutosaveScheduler<B>,
        generations: &mut GenerationPool,
        budget: TickBudget,
        _deadline: Deadline,
    ) -> Result<SourceAcquisitionTick, ServerError> {
        store.drive_workers();
        generations.drive();
        let poll = self.driver.poll(state, store, generations);
        let publication = state.advance_tick(budget)?;
        Ok(SourceAcquisitionTick {
            publication,
            poll,
            started: Vec::new(),
            queued: 0,
            first_error: poll.first_error,
        })
    }
}

/// One retained source owner fact: lifecycle admission, dimension,
/// foot-center column and effective radius only, plus the bounded pending
/// keys the owner still wants with their retained squared chunk distances.
/// No actor, inventory or book state is ever cloned.
#[derive(Clone, Debug, Eq, PartialEq)]
struct SourceOwner {
    player: bool,
    active: bool,
    dimension: Dimension,
    center: ChunkPos,
    radius: u8,
    pending: BTreeMap<ChunkKey, i64>,
}

/// The bounded per-tick source owner facts: at most eight players and four
/// registered companions, kept sorted by actor key at collection time.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
struct SourceInputs {
    owners: Vec<SourceOwner>,
}

/// Whole-want and FIFO queue ceiling shared by the goal book.
#[allow(dead_code)] // wired by the reducer threading followup
const MAX_SOURCE_KEYS: usize = 36660;

/// Squared distances beyond this delta cannot affect owner priority.
#[allow(dead_code)] // wired by the reducer threading followup
const RELEVANT_DISTANCE: i64 = 268435489;

/// The frozen effective session radius: a declared zero selects the full
/// server bound, any other declaration keeps the existing one-plus-one clamp.
#[allow(dead_code)] // wired by the reducer threading followup
fn effective_radius(declared: u8, server_bound: u8) -> u8 {
    if declared == 0 {
        server_bound
    } else {
        declared.saturating_add(1).min(server_bound)
    }
}

/// Squared horizontal distance between two chunk columns.
#[allow(dead_code)] // wired by the reducer threading followup
fn column_distance_squared(a: ChunkPos, b: ChunkPos) -> i64 {
    let dx = i64::from(a.x()) - i64::from(b.x());
    let dz = i64::from(a.z()) - i64::from(b.z());
    dx * dx + dz * dz
}

/// The smallest relevant distance one owner holds toward one key, or `None`
/// when this owner does not want the key: wanting means inside its own
/// square, players measure their center without a dimension filter, active
/// companions match only their own dimension, and retained pending keys
/// keep their captured distance.
#[allow(dead_code)] // wired by the reducer threading followup
fn owner_distance(owner: &SourceOwner, key: &ChunkKey) -> Option<i64> {
    if let Some(distance) = owner.pending.get(key) {
        return (*distance <= RELEVANT_DISTANCE).then_some(*distance);
    }
    if !owner.active && !owner.player {
        // Pending companions want their retained pending keys only.
        return None;
    }
    if !owner.player && key.dimension != owner.dimension {
        return None;
    }
    let distance = column_distance_squared(owner.center, key.pos);
    let bound = i64::from(owner.radius) * i64::from(owner.radius);
    (distance <= bound && distance <= RELEVANT_DISTANCE).then_some(distance)
}

/// Sorts one whole-want union by priority: each key ranks by the minimum
/// squared distance any wanting owner holds, then by chunk key order.
#[allow(dead_code)] // wired by the reducer threading followup
fn select_by_priority(inputs: &SourceInputs, wanted: &BTreeSet<ChunkKey>) -> Vec<ChunkKey> {
    let mut ranked: Vec<(i64, ChunkKey)> = wanted
        .iter()
        .filter_map(|key| {
            inputs
                .owners
                .iter()
                .filter_map(|owner| owner_distance(owner, key))
                .min()
                .map(|distance| (distance, *key))
        })
        .collect();
    ranked.sort_unstable();
    ranked.into_iter().map(|(_, key)| key).collect()
}

/// The frozen automatic goal book: the last retained small inputs, the
/// current whole want union and the FIFO admission queue with per-key dedup.
/// The initial force flag guarantees one whole-want reconcile before any
/// quiet pass can look settled.
struct SourceGoals {
    last_inputs: Option<SourceInputs>,
    wanted: BTreeSet<ChunkKey>,
    pending: VecDeque<ChunkKey>,
    queued: BTreeSet<ChunkKey>,
    force: bool,
}

impl Default for SourceGoals {
    fn default() -> Self {
        Self {
            last_inputs: None,
            wanted: BTreeSet::new(),
            pending: VecDeque::new(),
            queued: BTreeSet::new(),
            force: true,
        }
    }
}

impl SourceGoals {
    /// FIFO admission retains each key once and never resorts older jobs.
    #[allow(dead_code)] // wired by the reducer threading followup
    fn enqueue(&mut self, key: ChunkKey) {
        if self.pending.len() >= MAX_SOURCE_KEYS {
            return;
        }
        if self.queued.insert(key) {
            self.pending.push_back(key);
        }
    }

    /// Drops queued keys the current whole want union no longer names;
    /// started jobs are owned by the driver and never cancelled here.
    #[allow(dead_code)] // wired by the reducer threading followup
    fn retain_wanted(&mut self) {
        self.queued.retain(|key| self.wanted.contains(key));
        self.pending.retain(|key| self.wanted.contains(key));
    }
}

#[cfg(test)]
mod goals_tests {
    use super::super::publication_project::wanted_square;
    use super::*;

    fn key(dimension: Dimension, x: i32, z: i32) -> ChunkKey {
        ChunkKey {
            dimension,
            pos: ChunkPos::new(x, z),
        }
    }

    fn owner(
        player: bool,
        active: bool,
        dimension: Dimension,
        center: (i32, i32),
        radius: u8,
        pending: &[(ChunkKey, i64)],
    ) -> SourceOwner {
        SourceOwner {
            player,
            active,
            dimension,
            center: ChunkPos::new(center.0, center.1),
            radius,
            pending: pending.iter().copied().collect(),
        }
    }

    #[test]
    fn declared_zero_radius_selects_the_server_bound() {
        assert_eq!(effective_radius(0, 8), 8);
        assert_eq!(effective_radius(3, 8), 4);
        assert_eq!(effective_radius(200, 8), 8);
    }

    #[test]
    fn shared_keys_rank_by_the_closest_wanting_owner() {
        let dimension = Dimension::OVERWORLD;
        let inputs = SourceInputs {
            owners: vec![
                owner(true, true, dimension, (10, 10), 8, &[]),
                owner(true, true, dimension, (0, 0), 8, &[]),
            ],
        };
        let wanted = BTreeSet::from([key(dimension, 1, 0), key(dimension, 9, 9)]);
        assert_eq!(
            select_by_priority(&inputs, &wanted),
            vec![key(dimension, 1, 0), key(dimension, 9, 9)]
        );
    }

    #[test]
    fn pending_companions_rank_only_retained_keys() {
        let dimension = Dimension::OVERWORLD;
        let retained = key(dimension, 2, 0);
        let inputs = SourceInputs {
            owners: vec![owner(false, false, dimension, (0, 0), 1, &[(retained, 4)])],
        };
        let wanted = BTreeSet::from([retained, key(dimension, 1, 0)]);
        assert_eq!(select_by_priority(&inputs, &wanted), vec![retained]);
    }

    #[test]
    fn shared_checked_square_bounds_and_refusals() {
        let dimension = Dimension::OVERWORLD;
        let full = wanted_square(dimension, ChunkPos::new(0, 0), 33).expect("physical square");
        assert_eq!(full.len(), 4489);
        assert_eq!(
            full.range(key(dimension, -1, -1)..=key(dimension, 1, 1))
                .count(),
            9
        );
        assert!(wanted_square(dimension, ChunkPos::new(i32::MAX, 0), 33).is_err());
        assert!(wanted_square(dimension, ChunkPos::new(i32::MIN, i32::MIN), 1).is_err());
    }

    #[test]
    fn fifo_queue_dedups_and_drops_now_unwanted_keys() {
        let dimension = Dimension::OVERWORLD;
        let mut goals = SourceGoals::default();
        assert!(goals.force);
        goals.enqueue(key(dimension, 1, 0));
        goals.enqueue(key(dimension, 1, 0));
        goals.enqueue(key(dimension, 2, 0));
        assert_eq!(goals.pending.len(), 2);
        assert_eq!(goals.queued.len(), 2);
        goals.wanted.insert(key(dimension, 2, 0));
        goals.retain_wanted();
        assert_eq!(goals.pending.len(), 1);
        assert_eq!(goals.queued.len(), 1);
    }
}
