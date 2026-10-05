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
use super::contracts::{
    ActorKey, ActorLifecycle, ChunkKey, Deadline, DiskBackend, ServerError, SessionKey, TickBudget,
    TickPublication,
};
use super::generation_worker::GenerationPool;
use super::pending_restore::PendingRestore;
use super::publication_project::position_chunk;
use super::source_companion_restore::SourceCompanionBook;
use super::source_player_restore::SourcePlayerBook;
use super::state::{AuthorityReadView, AuthorityState};
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

/// One retained source owner fact: the owner key plus copied lifecycle,
/// dimension, foot-center column and effective radius scalars, and the
/// bounded pending keys this owner still wants with their retained squared
/// chunk distances. No actor, inventory or book state is ever cloned.
#[derive(Clone, Debug, Eq, PartialEq)]
struct SourceOwner {
    actor: ActorKey,
    player: bool,
    active: bool,
    dimension: Dimension,
    center: ChunkPos,
    radius: u8,
    pending: BTreeMap<ChunkKey, i64>,
}

/// The bounded per-tick source owner facts: at most eight players and four
/// registered companions, kept sorted by actor key.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct SourceInputs {
    owners: Vec<SourceOwner>,
}

/// Owner bounds: eight players plus four registered companions.
const MAX_SOURCE_OWNERS: usize = 12;

/// Whole-want and FIFO queue ceiling shared by the goal book.
#[allow(dead_code)] // wired by the reducer threading followup
const MAX_SOURCE_KEYS: usize = 36660;

/// The frozen effective session radius: a declared zero selects the full
/// server bound, any other declaration keeps the existing one-plus-one clamp.
pub(crate) fn effective_radius(declared: u8, server_bound: u8) -> u8 {
    if declared == 0 {
        server_bound
    } else {
        declared.saturating_add(1).min(server_bound)
    }
}

/// Retained pending keys with their per-key squared distances.
fn pending_facts(scan: &PendingRestore) -> BTreeMap<ChunkKey, i64> {
    scan.pending_keys()
        .into_iter()
        .map(|key| {
            let distance = scan.pending_distance_squared(key).unwrap_or(0);
            (key, distance)
        })
        .collect()
}

/// The squared distance one owner holds toward one queried key, or `None`
/// when this owner does not want the key. Membership is the owner's own
/// inclusive square (both axis deltas within the radius) or an exact
/// retained pending key; owners never want other dimensions' squares, and
/// pending companions want their retained pending keys only.
#[allow(dead_code)] // wired by the reducer threading followup
fn owner_distance(owner: &SourceOwner, key: &ChunkKey) -> Option<i64> {
    owner_want_distance(
        owner.player,
        owner.active,
        owner.dimension,
        owner.center,
        owner.radius,
        &owner.pending,
        key,
    )
}

/// Owner membership and distance core over copied scalar facts: pending
/// keys keep their retained distance, squares include every column with
/// both axis deltas within the radius in the owner's own dimension, and
/// distances widen before subtraction with no clipping.
fn owner_want_distance(
    player: bool,
    active: bool,
    owner_dimension: Dimension,
    center: ChunkPos,
    radius: u8,
    pending: &BTreeMap<ChunkKey, i64>,
    key: &ChunkKey,
) -> Option<i64> {
    if let Some(distance) = pending.get(key) {
        return Some(*distance);
    }
    if !active && !player {
        // Pending companions want their retained pending keys only.
        return None;
    }
    let dx = (i64::from(key.pos.x()) - i64::from(center.x())).abs();
    let dz = (i64::from(key.pos.z()) - i64::from(center.z())).abs();
    let bound = i64::from(radius);
    if dx > bound || dz > bound || key.dimension != owner_dimension {
        return None;
    }
    Some(dx * dx + dz * dz)
}

/// Sorts candidate rankings by distance, then by chunk key order.
fn rank_keys(candidates: impl Iterator<Item = (i64, ChunkKey)>) -> Vec<ChunkKey> {
    let mut ranked: Vec<(i64, ChunkKey)> = candidates.collect();
    ranked.sort_unstable();
    ranked.into_iter().map(|(_, key)| key).collect()
}

/// Sorts one whole-want union by priority: each key ranks by the minimum
/// squared distance any wanting owner holds, then by chunk key order.
#[allow(dead_code)] // wired by the reducer threading followup
fn select_by_priority(inputs: &SourceInputs, wanted: &BTreeSet<ChunkKey>) -> Vec<ChunkKey> {
    rank_keys(wanted.iter().filter_map(|key| {
        inputs
            .owners
            .iter()
            .filter_map(|owner| owner_distance(owner, key))
            .min()
            .map(|distance| (distance, *key))
    }))
}

impl SourceInputs {
    /// Collects the bounded owner facts over the borrowed read view and the
    /// reducer's real retained source books: at most eight players and four
    /// registered companions, sorted by actor key. Removed actors, non-Active
    /// sessions and unregistered scans contribute nothing; no fabricated
    /// scan facts are assigned.
    pub(crate) fn collect(
        read: &AuthorityReadView<'_>,
        players: &SourcePlayerBook,
        companions: &SourceCompanionBook,
        session_radius: impl Fn(SessionKey) -> Option<u8>,
    ) -> Result<Self, ServerError> {
        let mut owners: Vec<SourceOwner> = Vec::with_capacity(MAX_SOURCE_OWNERS);
        for (session, entry) in players.entries.iter() {
            // A registered session with an Active-scoped radius admits one
            // owner; non-Active sessions keep no automatic goals.
            let Some(radius) = session_radius(*session) else {
                continue;
            };
            let actor = read.actor(ActorKey::Player(*session));
            let pending = pending_facts(&entry.restore);
            let owner = match actor {
                Some(record) if record.lifecycle == ActorLifecycle::Active => SourceOwner {
                    actor: ActorKey::Player(*session),
                    player: true,
                    active: true,
                    dimension: record.dimension,
                    center: position_chunk(record.dimension, record.motion.position().get()).pos,
                    radius,
                    pending,
                },
                Some(record) => {
                    // Pending registered player: the exact captured anchor
                    // keeps the full radius square in the actor dimension,
                    // plus the retained pending keys including cross-D.
                    SourceOwner {
                        actor: ActorKey::Player(*session),
                        player: true,
                        active: false,
                        dimension: record.dimension,
                        center: entry.restore.subscription_anchor(),
                        radius,
                        pending,
                    }
                }
                None => continue,
            };
            owners.push(owner);
        }
        for (id, scan) in companions.entries.iter() {
            let actor = ActorKey::Companion(*id);
            let pending = pending_facts(scan);
            let owner = match read.actor(actor) {
                Some(record) if record.lifecycle == ActorLifecycle::Active => SourceOwner {
                    actor,
                    player: false,
                    active: true,
                    dimension: record.dimension,
                    center: position_chunk(record.dimension, record.motion.position().get()).pos,
                    radius: 1,
                    pending,
                },
                Some(record) => {
                    // Pending registered companions want their retained
                    // pending keys only.
                    SourceOwner {
                        actor,
                        player: false,
                        active: false,
                        dimension: record.dimension,
                        center: scan.subscription_anchor(),
                        radius: 1,
                        pending,
                    }
                }
                None => continue,
            };
            owners.push(owner);
        }
        owners.sort_unstable_by(|a, b| a.actor.cmp(&b.actor));
        owners.truncate(MAX_SOURCE_OWNERS);
        Ok(Self { owners })
    }
}

/// The frozen automatic goal book: the last retained small inputs, the
/// current whole want union and the FIFO admission queue with per-key dedup.
/// The initial force flag guarantees one whole-want reconcile before any
/// quiet pass can look settled.
pub(crate) struct SourceGoals {
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

    type Facts = (bool, bool, Dimension, ChunkPos, u8, BTreeMap<ChunkKey, i64>);

    fn facts(
        player: bool,
        active: bool,
        dimension: Dimension,
        center: (i32, i32),
        radius: u8,
        pending: &[(ChunkKey, i64)],
    ) -> Facts {
        (
            player,
            active,
            dimension,
            ChunkPos::new(center.0, center.1),
            radius,
            pending.iter().copied().collect(),
        )
    }

    fn want(owner: &Facts, key: ChunkKey) -> Option<i64> {
        owner_want_distance(owner.0, owner.1, owner.2, owner.3, owner.4, &owner.5, &key)
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
        let near = facts(true, true, dimension, (0, 0), 8, &[]);
        let far = facts(true, true, dimension, (10, 10), 8, &[]);
        let first = key(dimension, 1, 0);
        let corner = key(dimension, 9, 9);
        let ranked = rank_keys([first, corner].into_iter().map(|key| {
            let distance = [&near, &far]
                .into_iter()
                .filter_map(|owner| want(owner, key))
                .min()
                .expect("one wanting owner");
            (distance, key)
        }));
        assert_eq!(ranked, vec![first, corner]);
    }

    #[test]
    fn pending_companions_rank_only_retained_keys() {
        let dimension = Dimension::OVERWORLD;
        let retained = key(dimension, 2, 0);
        let pending = facts(false, false, dimension, (0, 0), 1, &[(retained, 4)]);
        assert_eq!(want(&pending, retained), Some(4));
        assert_eq!(want(&pending, key(dimension, 1, 0)), None);
    }

    #[test]
    fn square_membership_is_inclusive_at_corners() {
        let dimension = Dimension::OVERWORLD;
        let owner = facts(true, true, dimension, (0, 0), 1, &[]);
        assert_eq!(want(&owner, key(dimension, 1, 1)), Some(2));
        assert_eq!(want(&owner, key(dimension, 1, 2)), None);
    }

    #[test]
    fn other_dimension_keys_need_retained_pending_entries() {
        let overworld = Dimension::OVERWORLD;
        let depths = Dimension::DEPTHS;
        let owner = facts(true, true, overworld, (0, 0), 4, &[]);
        assert_eq!(want(&owner, key(depths, 0, 0)), None);
        let cross = key(depths, 30, 40);
        let pending = facts(true, false, overworld, (0, 0), 4, &[(cross, 9)]);
        assert_eq!(want(&pending, cross), Some(9));
    }

    #[test]
    fn full_radius_distances_rank_without_clipping() {
        let dimension = Dimension::OVERWORLD;
        let owner = facts(true, true, dimension, (0, 0), 33, &[]);
        assert_eq!(want(&owner, key(dimension, 33, 0)), Some(1089));
        assert_eq!(want(&owner, key(dimension, 33, 33)), Some(2178));
        assert_eq!(want(&owner, key(dimension, 34, 0)), None);
    }

    #[test]
    fn shared_checked_square_bounds_and_refusals() {
        let dimension = Dimension::OVERWORLD;
        let full = wanted_square(dimension, ChunkPos::new(0, 0), 33).expect("physical square");
        assert_eq!(full.len(), 4489);
        let near = full
            .iter()
            .filter(|key| key.pos.x().abs() <= 1 && key.pos.z().abs() <= 1)
            .count();
        assert_eq!(near, 9);
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
