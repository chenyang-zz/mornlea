//! Automatic source acquisition producer.
//!
//! [`SourceAcquisition`] exclusively owns one existing [`ChunkDriver`] plus
//! the frozen private [`SourceGoals`] book, and borrows the background
//! `AutosaveScheduler` and `GenerationPool` without taking, closing or
//! cancelling those owners. Only the explicit `advance` call derives
//! automatic goals, drives each borrowed provider once, polls the driver,
//! runs the real source tick through `AuthorityState::advance_source_tick`
//! and admits front FIFO provider starts; ordinary `advance_tick` semantics
//! stay untouched.

use super::acquisition::LiveChunkPhase;
use super::chunk_driver::{ChunkDriver, ChunkPollReport};
use super::contracts::{
    ActorKey, ActorLifecycle, ChunkKey, Deadline, DiskBackend, Resource, ServerError, SessionKey,
    TickBudget, TickPublication,
};
use super::generation_worker::GenerationPool;
use super::pending_restore::PendingRestore;
use super::publication_project::{position_chunk, wanted_square};
use super::source_companion_restore::SourceCompanionBook;
use super::source_player_restore::SourcePlayerBook;
use super::state::{AuthorityReadView, AuthorityState, TickContext};
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
    /// One nonblocking automatic pass: validation precedes every provider,
    /// the inline/frozen store admission query refuses before any drive, a
    /// permanent driver fault refuses the caller, each borrowed provider
    /// drives once, the driver polls once and the real source tick runs.
    /// Backpressure retains the whole FIFO; otherwise strict front admission
    /// attempts at most sixteen actual provider starts.
    pub fn advance<B: DiskBackend>(
        &mut self,
        state: &mut AuthorityState,
        store: &mut AutosaveScheduler<B>,
        generations: &mut GenerationPool,
        budget: TickBudget,
        deadline: Deadline,
    ) -> Result<SourceAcquisitionTick, ServerError> {
        state.check_source_tick(budget)?;
        // An inline or frozen store refuses before any provider drive.
        store.source_chunk_slots()?;
        if let Some(error) = self.driver.last_error() {
            return Err(error);
        }
        store.drive_workers();
        generations.drive();
        let poll = self.driver.poll(state, store, generations);
        let publication = state.advance_source_tick(budget, &mut self.goals)?;
        let mut started = Vec::new();
        let mut first_start_error = None;
        // The backpressure latch is externally poll-driven: retain every
        // queued job with zero starts instead of forcing providers.
        if !store.backpressured() {
            for _ in 0..MAX_ADMISSION_ATTEMPTS {
                let Some(&key) = self.goals.pending.front() else {
                    break;
                };
                if !self.goals.wanted.contains(&key) {
                    self.goals.pop_front();
                    continue;
                }
                let kind = match state.live_chunk_facts(key).map(|facts| facts.phase) {
                    // Missing and Failed records load; NeedsGeneration
                    // generates; every owned record stays with its owner.
                    None | Some(LiveChunkPhase::Failed) => SourceChunkKind::Load,
                    Some(LiveChunkPhase::NeedsGeneration) => SourceChunkKind::Generate,
                    Some(_) => {
                        self.goals.pop_front();
                        continue;
                    }
                };
                match kind {
                    SourceChunkKind::Load => {
                        if self.driver.pending_loads() >= MAX_DRIVER_LANES
                            || store.source_chunk_slots()? == 0
                        {
                            break;
                        }
                    }
                    SourceChunkKind::Generate => {
                        if self.driver.pending_generations() >= MAX_DRIVER_LANES
                            || generations.owned_jobs() >= MAX_DRIVER_LANES
                        {
                            break;
                        }
                    }
                }
                let result = match kind {
                    SourceChunkKind::Load => self.driver.start_load(state, store, key, deadline),
                    SourceChunkKind::Generate => {
                        self.driver.start_generation(state, generations, key)
                    }
                };
                match result {
                    Ok(_) => {
                        self.goals.pop_front();
                        started.push(SourceChunkStart { key, kind });
                    }
                    Err(error) => {
                        let capacity = matches!(error, ServerError::Capacity { .. });
                        if first_start_error.is_none() {
                            first_start_error = Some(error);
                        }
                        if capacity {
                            // The provider-aborted record stays Failed: keep
                            // the front candidate as a future load; never
                            // forge NeedsGeneration or roll back an owner.
                            break;
                        }
                        // A typed refusal consumes this job without poisoning
                        // the healthy authority; a disk error never becomes a
                        // generation start for the same key.
                        self.goals.pop_front();
                        if self.goals.companion_owned(&key) {
                            self.goals.force = true;
                        }
                    }
                }
                // A permanent driver correlation fault halts further starts.
                if self.driver.last_error().is_some() {
                    break;
                }
            }
        }
        Ok(SourceAcquisitionTick {
            publication,
            poll,
            started,
            queued: self.goals.pending.len(),
            first_error: poll.first_error.or(first_start_error),
        })
    }
}

/// Strict front admission attempts per advance call.
const MAX_ADMISSION_ATTEMPTS: usize = 16;

/// Driver lanes and provider admission share the frozen eight-slot bound.
const MAX_DRIVER_LANES: usize = 8;

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

/// Owner bounds: eight players plus four registered companions, checked
/// against the trusted registration ceiling instead of truncated.
const MAX_SOURCE_PLAYERS: usize = 8;
const MAX_SOURCE_COMPANIONS: usize = 4;
const MAX_SOURCE_OWNERS: usize = MAX_SOURCE_PLAYERS + MAX_SOURCE_COMPANIONS;

/// Whole-want and FIFO queue ceiling shared by the goal book.
const MAX_SOURCE_KEYS: usize = 36660;

/// Pre-Acquire companion fact copies the goal book retains.
const MAX_COMPANION_FACTS: usize = 36;

/// Freshly missing keys one tick may queue for generation.
const MAX_FRESH_MISSING: usize = 8;

/// The frozen effective session radius: a declared zero selects the full
/// server bound, any other declaration keeps the existing one-plus-one clamp.
pub(crate) fn effective_radius(declared: u8, server_bound: u8) -> u8 {
    if declared == 0 {
        server_bound
    } else {
        declared.saturating_add(1).min(server_bound)
    }
}

/// Retained player pending keys: every key measures dimensionlessly against
/// the owner's captured or current center, so a cross-dimension restore key
/// keeps a real squared distance instead of a manufactured zero.
fn player_pending_facts(scan: &PendingRestore, center: ChunkPos) -> BTreeMap<ChunkKey, i64> {
    scan.pending_keys()
        .into_iter()
        .map(|key| {
            let dx = i64::from(key.pos.x()) - i64::from(center.x());
            let dz = i64::from(key.pos.z()) - i64::from(center.z());
            (key, dx * dx + dz * dz)
        })
        .collect()
}

/// Retained companion pending keys with their retained scan distances; keys
/// without a valid retained distance contribute no manufactured value.
fn companion_pending_facts(scan: &PendingRestore) -> BTreeMap<ChunkKey, i64> {
    scan.pending_keys()
        .into_iter()
        .filter_map(|key| scan.pending_distance_squared(key).map(|d| (key, d)))
        .collect()
}

/// The squared distance one owner holds toward one queried key, or `None`
/// when this owner does not want the key. Membership is the owner's own
/// inclusive square (both axis deltas within the radius) or an exact
/// retained pending key; owners never want other dimensions' squares, and
/// pending companions want their retained pending keys only.
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
        let mut player_owners = 0usize;
        for (session, entry) in players.entries.iter() {
            // A registered session with an Active-scoped radius admits one
            // owner; non-Active sessions keep no automatic goals.
            let Some(radius) = session_radius(*session) else {
                continue;
            };
            let actor = read.actor(ActorKey::Player(*session));
            let owner = match actor {
                Some(record) if record.lifecycle == ActorLifecycle::Active => {
                    let center =
                        position_chunk(record.dimension, record.motion.position().get()).pos;
                    SourceOwner {
                        actor: ActorKey::Player(*session),
                        player: true,
                        active: true,
                        dimension: record.dimension,
                        center,
                        radius,
                        pending: player_pending_facts(&entry.restore, center),
                    }
                }
                Some(record) if record.lifecycle == ActorLifecycle::Pending => {
                    // Pending registered player: the exact captured anchor
                    // keeps the full radius square in the actor dimension,
                    // plus the retained pending keys including cross-D.
                    let anchor = entry.restore.subscription_anchor();
                    SourceOwner {
                        actor: ActorKey::Player(*session),
                        player: true,
                        active: false,
                        dimension: record.dimension,
                        center: anchor,
                        radius,
                        pending: player_pending_facts(&entry.restore, anchor),
                    }
                }
                // Dead, respawning or absent actors keep no automatic goals.
                _ => continue,
            };
            player_owners += 1;
            owners.push(owner);
        }
        let mut companion_owners = 0usize;
        for (id, scan) in companions.entries.iter() {
            let actor = ActorKey::Companion(*id);
            let owner = match read.actor(actor) {
                Some(record) if record.lifecycle == ActorLifecycle::Active => {
                    let center =
                        position_chunk(record.dimension, record.motion.position().get()).pos;
                    SourceOwner {
                        actor,
                        player: false,
                        active: true,
                        dimension: record.dimension,
                        center,
                        radius: 1,
                        pending: companion_pending_facts(scan),
                    }
                }
                Some(record) if record.lifecycle == ActorLifecycle::Pending => {
                    // Pending registered companions want their retained
                    // pending keys only.
                    SourceOwner {
                        actor,
                        player: false,
                        active: false,
                        dimension: record.dimension,
                        center: scan.subscription_anchor(),
                        radius: 1,
                        pending: companion_pending_facts(scan),
                    }
                }
                // Completed, dead, respawning or absent actors contribute
                // nothing even while the book retains their entry.
                _ => continue,
            };
            companion_owners += 1;
            owners.push(owner);
        }
        // The books bound registration, so an exceeded trusted bound is an
        // invariant failure rather than a silent truncation.
        if player_owners > MAX_SOURCE_PLAYERS || companion_owners > MAX_SOURCE_COMPANIONS {
            return Err(ServerError::Internal {
                invariant: "source owner bound",
            });
        }
        owners.sort_unstable_by(|a, b| a.actor.cmp(&b.actor));
        Ok(Self { owners })
    }

    /// The full whole-want union: every player or active owner contributes
    /// its own-dimension checked square, and every owner contributes its
    /// retained pending keys. One aggregate ceiling refuses overflow.
    pub(crate) fn wanted(&self) -> Result<BTreeSet<ChunkKey>, ServerError> {
        let mut union = BTreeSet::new();
        for owner in &self.owners {
            if owner.player || owner.active {
                union.extend(wanted_square(owner.dimension, owner.center, owner.radius)?);
            }
            union.extend(owner.pending.keys().copied());
        }
        if union.len() > MAX_SOURCE_KEYS {
            return Err(ServerError::Capacity {
                resource: Resource::ChunkWants,
                limit: MAX_SOURCE_KEYS,
                observed: union.len(),
            });
        }
        Ok(union)
    }

    /// The keys companion owners want this tick: an active radius-one square
    /// plus retained pending keys, bounded by the four-owner registration.
    pub(crate) fn companion_keys(&self) -> Result<Vec<ChunkKey>, ServerError> {
        let mut keys = BTreeSet::new();
        for owner in &self.owners {
            if owner.player {
                continue;
            }
            if owner.active {
                keys.extend(wanted_square(owner.dimension, owner.center, owner.radius)?);
            }
            keys.extend(owner.pending.keys().copied());
        }
        Ok(keys.into_iter().collect())
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
    /// Pre-Acquire phase/generation copies for current companion wanted keys.
    prior_companion_facts: BTreeMap<ChunkKey, (LiveChunkPhase, u64)>,
    /// Freshly missing keys the last Acquire produced, queued as generation
    /// candidates after this tick's conditional load batch.
    fresh_missing: Vec<ChunkKey>,
}

impl Default for SourceGoals {
    fn default() -> Self {
        Self {
            last_inputs: None,
            wanted: BTreeSet::new(),
            pending: VecDeque::new(),
            queued: BTreeSet::new(),
            force: true,
            prior_companion_facts: BTreeMap::new(),
            fresh_missing: Vec::new(),
        }
    }
}

impl SourceGoals {
    /// FIFO admission retains each key once and never resorts older jobs.
    fn enqueue(&mut self, key: ChunkKey) {
        if self.pending.len() >= MAX_SOURCE_KEYS {
            return;
        }
        if self.queued.insert(key) {
            self.pending.push_back(key);
        }
    }

    /// Removes the front FIFO candidate and its dedup entry.
    fn pop_front(&mut self) {
        if let Some(key) = self.pending.pop_front() {
            self.queued.remove(&key);
        }
    }

    /// Drops queued keys the current whole want union no longer names;
    /// started jobs are owned by the driver and never cancelled here.
    fn retain_wanted(&mut self) {
        self.queued.retain(|key| self.wanted.contains(key));
        self.pending.retain(|key| self.wanted.contains(key));
    }

    /// Whether one companion owner still wants the given key, for carrying
    /// force after a companion-related typed admission failure.
    fn companion_owned(&self, key: &ChunkKey) -> bool {
        self.last_inputs.as_ref().is_some_and(|inputs| {
            inputs
                .owners
                .iter()
                .any(|owner| !owner.player && owner_distance(owner, key).is_some())
        })
    }

    /// Before the Acquire row and only when staged completions exist: derive
    /// the current union into the tick-local completion override and capture
    /// the pre-Acquire phase/generation of current companion wanted keys.
    /// Never replaces the whole want set here; that would unload unrelated
    /// Ready chunks before physics runs.
    pub(crate) fn before_acquire(
        &mut self,
        context: &mut TickContext<'_>,
        players: &SourcePlayerBook,
        companions: &SourceCompanionBook,
    ) -> Result<(), ServerError> {
        self.prior_companion_facts.clear();
        if !context.source_staged_present() {
            return Ok(());
        }
        let inputs = context.source_inputs(players, companions)?;
        let union = inputs.wanted()?;
        for key in inputs.companion_keys()? {
            if self.prior_companion_facts.len() >= MAX_COMPANION_FACTS {
                break;
            }
            if let Some(facts) = context.source_chunk_facts(key) {
                self.prior_companion_facts
                    .insert(key, (facts.phase, facts.generation));
            }
        }
        context.set_source_completion_wants(union);
        Ok(())
    }

    /// After the Acquire row: drain the tick's freshly missing keys without
    /// dirtying subscriptions, and mark force only when a current companion
    /// key newly failed relative to the pre-Acquire facts. An old Failed key
    /// never re-dirties every quiet tick.
    pub(crate) fn after_acquire(
        &mut self,
        context: &mut TickContext<'_>,
    ) -> Result<(), ServerError> {
        for key in context.take_source_generation_keys() {
            if self.fresh_missing.len() < MAX_FRESH_MISSING && !self.fresh_missing.contains(&key) {
                self.fresh_missing.push(key);
            }
        }
        let prior = std::mem::take(&mut self.prior_companion_facts);
        for (key, (phase, generation)) in prior {
            if let Some(facts) = context.source_chunk_facts(key)
                && facts.phase == LiveChunkPhase::Failed
                && (phase != LiveChunkPhase::Failed || facts.generation != generation)
            {
                self.force = true;
            }
        }
        Ok(())
    }

    /// The sole whole-want reconcile, after CompanionMotion and before the
    /// hostile plan. Reconciles only on force, changed inputs, new companion
    /// ingress failure, or a pending registered scan whose spawn wait column
    /// is absent or Failed; an unchanged saved-restore candidate failure is
    /// quiet. Fresh missing keys queue generation candidates after the
    /// conditional load batch without dirtying anything.
    pub(crate) fn reconcile(
        &mut self,
        context: &mut TickContext<'_>,
        players: &SourcePlayerBook,
        companions: &SourceCompanionBook,
    ) -> Result<(), ServerError> {
        let inputs = context.source_inputs(players, companions)?;
        let mut dirty = self.force || self.last_inputs.as_ref() != Some(&inputs);
        if !dirty {
            dirty = players
                .entries
                .values()
                .filter_map(|entry| entry.restore.spawn_wait_key())
                .chain(
                    companions
                        .entries
                        .values()
                        .filter_map(|scan| scan.spawn_wait_key()),
                )
                .any(|key| {
                    context
                        .source_chunk_facts(key)
                        .is_none_or(|facts| facts.phase == LiveChunkPhase::Failed)
                });
        }
        if dirty {
            let next_union = inputs.wanted()?;
            // Only this tick's newly emitted batch sorts by priority; older
            // queued jobs keep their accepted FIFO order.
            let selected: BTreeSet<ChunkKey> = next_union
                .iter()
                .copied()
                .filter(|key| {
                    !self.wanted.contains(key)
                        || context
                            .source_chunk_facts(*key)
                            .is_some_and(|facts| facts.phase == LiveChunkPhase::Failed)
                })
                .collect();
            context.replace_source_chunk_wants(next_union.clone())?;
            self.wanted = next_union;
            self.retain_wanted();
            for key in select_by_priority(&inputs, &selected) {
                if self.pending.len() >= MAX_SOURCE_KEYS {
                    break;
                }
                if self.queued.contains(&key) {
                    continue;
                }
                match context.source_chunk_facts(key).map(|facts| facts.phase) {
                    // Only absent or Failed records admit a new load;
                    // Loading/Generating/Ready/Unloading records stay owned.
                    None | Some(LiveChunkPhase::Failed) => self.enqueue(key),
                    _ => (),
                }
            }
            self.last_inputs = Some(inputs);
            self.force = false;
        }
        for key in std::mem::take(&mut self.fresh_missing) {
            if self.pending.len() >= MAX_SOURCE_KEYS {
                break;
            }
            // Missing alone never dirties the pass or retries an unrelated
            // Failed player load: only a still-wanted NeedsGeneration record
            // queues its generation candidate here.
            if self.queued.contains(&key) || !self.wanted.contains(&key) {
                continue;
            }
            if context
                .source_chunk_facts(key)
                .is_some_and(|facts| facts.phase == LiveChunkPhase::NeedsGeneration)
            {
                self.enqueue(key);
            }
        }
        Ok(())
    }

    /// After the full reducer and publication: capture the final small
    /// inputs and carry force into the next tick when late actor
    /// death/reset/reactivation changed them after the reconcile snapshot.
    /// The final union only prunes unstarted candidates; the reconcile
    /// captured `wanted` stays so the next tick still compares new keys.
    pub(crate) fn finish_tick(&mut self, state: &AuthorityState) -> Result<(), ServerError> {
        let inputs = state.source_inputs_settled()?;
        self.carry_force(&inputs);
        let final_union = inputs.wanted()?;
        self.queued.retain(|key| final_union.contains(key));
        self.pending.retain(|key| final_union.contains(key));
        Ok(())
    }

    /// Flags one force reconcile when the final inputs differ from the
    /// post-Actor reconcile snapshot; no second reconcile runs this tick.
    fn carry_force(&mut self, inputs: &SourceInputs) {
        if self.last_inputs.as_ref() != Some(inputs) {
            self.force = true;
        }
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

    fn player_owner(
        player: bool,
        active: bool,
        dimension: Dimension,
        center: (i32, i32),
        radius: u8,
        pending: &[(ChunkKey, i64)],
    ) -> SourceOwner {
        SourceOwner {
            actor: ActorKey::Player(SessionKey::from_raw(1).expect("session key")),
            player,
            active,
            dimension,
            center: ChunkPos::new(center.0, center.1),
            radius,
            pending: pending.iter().copied().collect(),
        }
    }

    #[test]
    fn whole_want_union_respects_the_aggregate_ceiling() {
        let dimension = Dimension::OVERWORLD;
        let bounded = SourceInputs {
            owners: (0..8)
                .map(|i| player_owner(true, true, dimension, (i * 100, 0), 33, &[]))
                .collect(),
        };
        assert_eq!(bounded.wanted().expect("bounded union").len(), 8 * 4489);
        let excessive = SourceInputs {
            owners: (0..9)
                .map(|i| player_owner(true, true, dimension, (i * 100, 0), 33, &[]))
                .collect(),
        };
        assert!(matches!(
            excessive.wanted(),
            Err(ServerError::Capacity {
                resource: Resource::ChunkWants,
                limit: 36_660,
                ..
            })
        ));
    }

    #[test]
    fn pending_players_keep_the_anchor_square_and_cross_dimension_keys() {
        let overworld = Dimension::OVERWORLD;
        let depths = Dimension::DEPTHS;
        let cross = key(depths, 10, 0);
        let inputs = SourceInputs {
            owners: vec![player_owner(
                true,
                false,
                overworld,
                (0, 0),
                1,
                &[(cross, 100)],
            )],
        };
        let wanted = inputs.wanted().expect("union");
        // The own-dimension radius square plus the cross-dimension pending
        // key with its owner-centered distance.
        assert_eq!(wanted.len(), 10);
        assert!(wanted.contains(&cross));
        assert!(wanted.contains(&key(overworld, 1, 1)));
    }

    #[test]
    fn unloading_rewant_keeps_the_resident_generation() {
        use super::super::acquisition::AcquisitionState;
        let dimension = Dimension::OVERWORLD;
        let target = key(dimension, 3, 4);
        let mut acquisition = AcquisitionState::default();
        acquisition.enable();
        acquisition
            .replace_wants([target].into())
            .expect("initial want");
        acquisition.reserve_load(target).expect("reserve");
        acquisition.installed(target, 9, 9, false, false, None);
        let ready = acquisition.facts(target).expect("ready record");
        assert_eq!(ready.phase, LiveChunkPhase::Ready);
        acquisition.replace_wants(BTreeSet::new()).expect("unwant");
        let unloading = acquisition.facts(target).expect("retained body");
        assert_eq!(unloading.phase, LiveChunkPhase::Unloading);
        assert_eq!(unloading.generation, ready.generation);
        assert_eq!(unloading.revision, 9);
        acquisition.replace_wants([target].into()).expect("rewant");
        let rewanted = acquisition.facts(target).expect("rewanted record");
        assert_eq!(rewanted.phase, LiveChunkPhase::Ready);
        assert_eq!(rewanted.generation, ready.generation);
    }

    #[test]
    fn late_input_change_carries_force_without_a_second_reconcile() {
        let dimension = Dimension::OVERWORLD;
        let mut goals = SourceGoals::default();
        goals.force = false;
        goals.wanted.insert(key(dimension, 0, 0));
        let settled = SourceInputs::default();
        goals.last_inputs = Some(settled.clone());
        let changed = SourceInputs {
            owners: vec![player_owner(true, true, dimension, (0, 0), 1, &[])],
        };
        goals.carry_force(&changed);
        assert!(goals.force);
        // The tick-end capture only flags the next tick; the reconcile union
        // stays exactly what the mid-tick reconcile captured.
        assert_eq!(goals.wanted.len(), 1);
        goals.force = false;
        goals.carry_force(&settled);
        assert!(!goals.force);
    }

    #[test]
    fn quiet_fresh_missing_never_reconciles_or_retries_failed_keys() {
        use super::super::contracts::{Operation, ServerLimits};
        let dimension = Dimension::OVERWORLD;
        let failed_key = key(dimension, 2, 0);
        let missing_key = key(dimension, 5, 0);
        let mut state = AuthorityState::try_new(
            ServerLimits::try_new(8, 128, 64, 16, 32, 1_048_576).expect("limits"),
            42,
        )
        .expect("authority");
        state.enable_live_chunks().expect("live chunks");
        state
            .replace_chunk_wants([failed_key].into())
            .expect("want");
        let reservation = state.reserve_chunk_load(failed_key).expect("reserve");
        state
            .abort_chunk_load(
                reservation,
                ServerError::Io {
                    operation: Operation::Load,
                    kind: std::io::ErrorKind::NotFound,
                },
            )
            .expect("abort");
        assert_eq!(
            state.live_chunk_facts(failed_key).map(|facts| facts.phase),
            Some(LiveChunkPhase::Failed)
        );
        let mut goals = SourceGoals::default();
        goals.force = false;
        goals.last_inputs = Some(SourceInputs::default());
        goals.wanted = [failed_key, missing_key].into();
        goals.fresh_missing.push(missing_key);
        goals.enqueue(failed_key);
        let players = SourcePlayerBook::default();
        let companions = SourceCompanionBook::default();
        let mut context = TickContext::for_tick(&mut state, TickBudget::full());
        goals
            .reconcile(&mut context, &players, &companions)
            .expect("quiet reconcile");
        // No whole-want replacement, no dirty selection of the Failed player
        // key, and the fresh missing key queues nothing without a
        // NeedsGeneration record.
        assert!(!goals.force);
        assert_eq!(goals.pending.len(), 1);
        assert_eq!(goals.queued.len(), 1);
        assert_eq!(
            state.live_chunk_facts(failed_key).map(|facts| facts.phase),
            Some(LiveChunkPhase::Failed)
        );
    }
}
