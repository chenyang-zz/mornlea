//! Bounded live acquisition identities; resident bodies belong to the authority.
//!
//! Only reservations allocate records. Separate source aliases retain started
//! ownership until Acquire consumes a whole completion, even after forgetting.
//! Unloading preserves resident facts for a later durable/reclamation owner;
//! this book never performs I/O, prepares bodies or records request history.
use super::{
    contracts::{
        ChunkKey, ChunkRequestId, OwnedSnapshot, Resource, SaveKey, SaveMode, SaveStats,
        SaveUrgency, SaveValue, ServerError,
    },
    world::PreparedChunk,
};
use std::collections::{BTreeMap, BTreeSet};

const MAX_WANTS: usize = 36_660;
const MAX_MANAGED: usize = 36_676;
const MAX_ATTEMPTS: usize = 8;
const MAX_STAGED: usize = 16;

/// Gameplay availability requires Ready; Unloading retains a body privately.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LiveChunkPhase {
    Loading,
    NeedsGeneration,
    Generating,
    Failed,
    Ready,
    Unloading,
}
/// Current identity and source durability facts, independent of save scheduling.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LiveChunkFacts {
    pub generation: u64,
    pub revision: u64,
    pub persisted_revision: u64,
    pub needs_rewrite: bool,
    pub recovered: bool,
    pub wanted: bool,
    pub phase: LiveChunkPhase,
}

/// Opaque unbound load attempt; every use rechecks the current book record.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ChunkLoadReservation {
    key: ChunkKey,
    generation: u64,
}
impl ChunkLoadReservation {
    pub fn key(self) -> ChunkKey {
        self.key
    }
    pub fn generation(self) -> u64 {
        self.generation
    }
}
/// Opaque unbound CPU attempt reusing the generation of its missing disk load.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ChunkGenerationReservation {
    key: ChunkKey,
    generation: u64,
}
impl ChunkGenerationReservation {
    pub fn key(self) -> ChunkKey {
        self.key
    }
    pub fn generation(self) -> u64 {
        self.generation
    }
}

// A capped lane transfers the whole prepared owner, never a cloned body.
#[allow(clippy::large_enum_variant)]
pub enum AcquiredChunkEvent {
    Load {
        key: ChunkKey,
        generation: u64,
        request: ChunkRequestId,
        result: Result<Option<PreparedChunk>, ServerError>,
    },
    Generated {
        key: ChunkKey,
        generation: u64,
        request: ChunkRequestId,
        result: Result<PreparedChunk, ServerError>,
    },
}
impl AcquiredChunkEvent {
    fn identity(&self) -> (bool, ChunkKey, u64, ChunkRequestId) {
        match self {
            Self::Load {
                key,
                generation,
                request,
                ..
            } => (false, *key, *generation, *request),
            Self::Generated {
                key,
                generation,
                request,
                ..
            } => (true, *key, *generation, *request),
        }
    }
    fn prepared_matches(&self) -> bool {
        let (_, key, generation, _) = self.identity();
        let prepared = match self {
            Self::Load {
                result: Ok(Some(v)),
                ..
            }
            | Self::Generated { result: Ok(v), .. } => Some(v),
            _ => None,
        };
        prepared.is_none_or(|v| v.key() == key && v.generation() == generation)
    }
}
/// A refusal returns the exact event owner; the expected alias stays charged.
pub struct RejectedAcquiredChunk {
    pub error: ServerError,
    pub event: AcquiredChunkEvent,
}
impl std::fmt::Debug for RejectedAcquiredChunk {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RejectedAcquiredChunk")
            .field("error", &self.error)
            .finish_non_exhaustive()
    }
}
struct Record {
    facts: LiveChunkFacts,
    request: Option<ChunkRequestId>,
    error: Option<ServerError>,
    estimate: Option<usize>,
}
#[derive(Default)]
pub(crate) struct AcquisitionState {
    enabled: bool,
    wants: BTreeSet<ChunkKey>,
    records: BTreeMap<ChunkKey, Record>,
    loads: BTreeMap<ChunkRequestId, (ChunkKey, u64)>,
    generations: BTreeMap<ChunkRequestId, (ChunkKey, u64)>,
    load_count: usize,
    generation_count: usize,
    staged: Vec<AcquiredChunkEvent>,
    last_generation: u64,
    // Current facts and retained captures have separate exact byte charges.
    dirty: BTreeSet<ChunkKey>,
    retireable: BTreeSet<ChunkKey>,
    eligible: BTreeSet<(bool, ChunkKey)>,
    flights: BTreeMap<ChunkKey, OwnedSnapshot>,
    dirty_bytes: u64,
    flight_bytes: u64,
    #[cfg(test)]
    save_work: std::cell::Cell<(usize, usize)>,
    #[cfg(test)]
    retirement_work: std::cell::Cell<(usize, usize)>,
}
fn invalid(field: &'static str) -> ServerError {
    ServerError::InvalidInput { field }
}
fn capacity(resource: Resource, observed: usize, limit: usize) -> Result<(), ServerError> {
    if observed > limit {
        Err(ServerError::Capacity {
            resource,
            limit,
            observed,
        })
    } else {
        Ok(())
    }
}
impl AcquisitionState {
    pub(crate) fn enabled(&self) -> bool {
        self.enabled
    }
    pub(crate) fn enable(&mut self) {
        self.enabled = true;
    }
    pub(crate) fn facts(&self, key: ChunkKey) -> Option<LiveChunkFacts> {
        self.records.get(&key).map(|r| r.facts)
    }
    pub(crate) fn error(&self, key: ChunkKey) -> Option<&ServerError> {
        self.records.get(&key)?.error.as_ref()
    }
    pub(crate) fn available(&self, key: ChunkKey) -> bool {
        !self.enabled
            || self
                .facts(key)
                .is_some_and(|f| f.phase == LiveChunkPhase::Ready)
    }
    pub(crate) fn replace_wants(&mut self, wants: BTreeSet<ChunkKey>) -> Result<(), ServerError> {
        capacity(Resource::ChunkWants, wants.len(), MAX_WANTS)?;
        let keys: Vec<_> = self.records.keys().copied().collect();
        for key in keys {
            self.refresh(key, |r| {
                r.facts.wanted = wants.contains(&key);
                match (r.facts.phase, r.facts.wanted) {
                    (LiveChunkPhase::Ready, false) => r.facts.phase = LiveChunkPhase::Unloading,
                    (LiveChunkPhase::Unloading, true) => r.facts.phase = LiveChunkPhase::Ready,
                    _ => (),
                }
            });
            if self.records.get(&key).is_some_and(|r| {
                !r.facts.wanted
                    && matches!(
                        r.facts.phase,
                        LiveChunkPhase::Failed | LiveChunkPhase::NeedsGeneration
                    )
            }) {
                self.records.remove(&key);
            }
        }
        self.wants = wants;
        Ok(())
    }
    pub(crate) fn reserve_load(
        &mut self,
        key: ChunkKey,
    ) -> Result<ChunkLoadReservation, ServerError> {
        if !self.wants.contains(&key)
            || self
                .records
                .get(&key)
                .is_some_and(|r| r.facts.phase != LiveChunkPhase::Failed)
        {
            return Err(invalid("chunk_reservation"));
        }
        capacity(Resource::ChunkRequests, self.load_count + 1, MAX_ATTEMPTS)?;
        capacity(
            Resource::ResidentChunks,
            self.records.len() + usize::from(!self.records.contains_key(&key)),
            MAX_MANAGED,
        )?;
        let generation = self
            .last_generation
            .checked_add(1)
            .ok_or(invalid("chunk_generation"))?;
        self.records.insert(
            key,
            Record {
                facts: LiveChunkFacts {
                    generation,
                    revision: 0,
                    persisted_revision: 0,
                    needs_rewrite: false,
                    recovered: false,
                    wanted: true,
                    phase: LiveChunkPhase::Loading,
                },
                request: None,
                error: None,
                estimate: None,
            },
        );
        self.last_generation = generation;
        self.load_count += 1;
        Ok(ChunkLoadReservation { key, generation })
    }
    pub(crate) fn reserve_generation(
        &mut self,
        key: ChunkKey,
    ) -> Result<ChunkGenerationReservation, ServerError> {
        let r = self.records.get(&key).ok_or(invalid("chunk_reservation"))?;
        if !r.facts.wanted || r.facts.phase != LiveChunkPhase::NeedsGeneration {
            return Err(invalid("chunk_reservation"));
        }
        capacity(
            Resource::ChunkRequests,
            self.generation_count + 1,
            MAX_ATTEMPTS,
        )?;
        let r = self.records.get_mut(&key).expect("validated record");
        r.facts.phase = LiveChunkPhase::Generating;
        self.generation_count += 1;
        Ok(ChunkGenerationReservation {
            key,
            generation: r.facts.generation,
        })
    }
    fn unbound(&self, key: ChunkKey, generation: u64, generated: bool) -> Result<(), ServerError> {
        let phase = if generated {
            LiveChunkPhase::Generating
        } else {
            LiveChunkPhase::Loading
        };
        if self.records.get(&key).is_some_and(|r| {
            r.facts.generation == generation && r.facts.phase == phase && r.request.is_none()
        }) {
            Ok(())
        } else {
            Err(invalid("chunk_reservation"))
        }
    }
    pub(crate) fn bind_load(
        &mut self,
        r: ChunkLoadReservation,
        request: ChunkRequestId,
    ) -> Result<(), ServerError> {
        self.bind(r.key, r.generation, false, request)
    }
    pub(crate) fn bind_generation(
        &mut self,
        r: ChunkGenerationReservation,
        request: ChunkRequestId,
    ) -> Result<(), ServerError> {
        self.bind(r.key, r.generation, true, request)
    }
    fn bind(
        &mut self,
        key: ChunkKey,
        generation: u64,
        generated: bool,
        request: ChunkRequestId,
    ) -> Result<(), ServerError> {
        self.unbound(key, generation, generated)?;
        let aliases = if generated {
            &mut self.generations
        } else {
            &mut self.loads
        };
        if aliases.contains_key(&request) {
            return Err(invalid("chunk_request_identity"));
        }
        aliases.insert(request, (key, generation));
        self.records
            .get_mut(&key)
            .expect("validated record")
            .request = Some(request);
        Ok(())
    }
    pub(crate) fn abort_load(
        &mut self,
        r: ChunkLoadReservation,
        error: ServerError,
    ) -> Result<(), ServerError> {
        self.abort(r.key, r.generation, false, error)
    }
    pub(crate) fn abort_generation(
        &mut self,
        r: ChunkGenerationReservation,
        error: ServerError,
    ) -> Result<(), ServerError> {
        self.abort(r.key, r.generation, true, error)
    }
    fn abort(
        &mut self,
        key: ChunkKey,
        generation: u64,
        generated: bool,
        error: ServerError,
    ) -> Result<(), ServerError> {
        self.unbound(key, generation, generated)?;
        if generated {
            self.generation_count -= 1;
        } else {
            self.load_count -= 1;
        }
        self.failed(key, error);
        Ok(())
    }
    fn expected(&self, e: &AcquiredChunkEvent) -> bool {
        let (generated, key, generation, request) = e.identity();
        let aliases = if generated {
            &self.generations
        } else {
            &self.loads
        };
        aliases.get(&request) == Some(&(key, generation))
            && self.records.get(&key).is_some_and(|r| {
                r.facts.generation == generation
                    && r.request == Some(request)
                    && r.facts.phase
                        == if generated {
                            LiveChunkPhase::Generating
                        } else {
                            LiveChunkPhase::Loading
                        }
            })
    }
    // Refusal preserves ownership of the exact prepared event for the caller.
    #[allow(clippy::result_large_err)]
    pub(crate) fn offer(&mut self, event: AcquiredChunkEvent) -> Result<(), RejectedAcquiredChunk> {
        let error = if self.staged.len() >= MAX_STAGED {
            Some(ServerError::Capacity {
                resource: Resource::ChunkResults,
                limit: MAX_STAGED,
                observed: self.staged.len() + 1,
            })
        } else if !self.expected(&event)
            || !event.prepared_matches()
            || self.staged.iter().any(|v| v.identity() == event.identity())
        {
            Some(invalid("chunk_completion_identity"))
        } else {
            None
        };
        if let Some(error) = error {
            return Err(RejectedAcquiredChunk { error, event });
        }
        self.staged.push(event);
        Ok(())
    }
    pub(crate) fn drain(&mut self) -> Vec<AcquiredChunkEvent> {
        let mut events = std::mem::take(&mut self.staged);
        events.sort_by_key(|e| {
            let (source, key, _, _) = e.identity();
            (source, key)
        });
        events
    }
    pub(crate) fn settle(&mut self, e: &AcquiredChunkEvent) -> bool {
        if !self.expected(e) {
            return false;
        }
        let (generated, key, _, request) = e.identity();
        if generated {
            self.generations.remove(&request);
            self.generation_count -= 1;
        } else {
            self.loads.remove(&request);
            self.load_count -= 1;
        }
        self.records
            .get_mut(&key)
            .expect("validated record")
            .request = None;
        true
    }
    pub(crate) fn missing(&mut self, key: ChunkKey) {
        if let Some(r) = self.records.get_mut(&key)
            && r.facts.wanted
        {
            r.facts.phase = LiveChunkPhase::NeedsGeneration;
            return;
        }
        self.records.remove(&key);
    }
    pub(crate) fn failed(&mut self, key: ChunkKey, error: ServerError) {
        if let Some(r) = self.records.get_mut(&key)
            && r.facts.wanted
        {
            r.facts.phase = LiveChunkPhase::Failed;
            r.error = Some(error);
            return;
        }
        self.records.remove(&key);
    }
    pub(crate) fn installed(
        &mut self,
        key: ChunkKey,
        revision: u64,
        persisted_revision: u64,
        needs_rewrite: bool,
        recovered: bool,
        estimate: Option<usize>,
    ) {
        self.refresh(key, |r| {
            r.facts.revision = revision;
            r.facts.persisted_revision = persisted_revision;
            r.facts.needs_rewrite = needs_rewrite;
            r.facts.recovered = recovered;
            r.estimate = estimate;
            r.facts.phase = if r.facts.wanted {
                LiveChunkPhase::Ready
            } else {
                LiveChunkPhase::Unloading
            };
        });
    }
    pub(crate) fn committed(
        &mut self,
        key: ChunkKey,
        generation: u64,
        revision: u64,
        estimate: Option<usize>,
    ) {
        self.refresh(key, |r| {
            debug_assert_eq!(r.facts.generation, generation);
            r.facts.revision = revision;
            r.estimate = estimate;
        });
    }
    fn contribution(r: &Record) -> Option<u64> {
        (matches!(
            r.facts.phase,
            LiveChunkPhase::Ready | LiveChunkPhase::Unloading
        ) && (r.facts.revision > r.facts.persisted_revision || r.facts.needs_rewrite))
            .then_some(
                r.estimate
                    .unwrap_or(crate::store::mailbox::CHUNK_MAX_RESERVATION) as u64,
            )
    }
    // Key-local refresh removes old contributions before changing current facts.
    fn refresh(&mut self, key: ChunkKey, update: impl FnOnce(&mut Record)) {
        self.retireable.remove(&key);
        let Some(r) = self.records.get_mut(&key) else {
            return;
        };
        if let Some(bytes) = Self::contribution(r) {
            self.dirty.remove(&key);
            self.eligible
                .remove(&(r.facts.phase == LiveChunkPhase::Ready, key));
            self.dirty_bytes -= bytes;
        }
        update(r);
        if let Some(bytes) = Self::contribution(r) {
            self.dirty.insert(key);
            self.dirty_bytes += bytes;
            if r.estimate.is_none() {
                // Preserve a specific refusal when its invalid estimate is retained.
                r.error.get_or_insert(ServerError::Internal {
                    invariant: "chunk payload estimate",
                });
            } else if !self.flights.contains_key(&key) {
                self.eligible
                    .insert((r.facts.phase == LiveChunkPhase::Ready, key));
            }
        }
        if Self::can_retire(r) && !self.flights.contains_key(&key) {
            self.retireable.insert(key);
        }
    }
    fn can_retire(r: &Record) -> bool {
        !r.facts.wanted
            && r.facts.phase == LiveChunkPhase::Unloading
            && r.request.is_none()
            && r.facts.revision <= r.facts.persisted_revision
            && !r.facts.needs_rewrite
    }
    /// The ordered index avoids visiting wanted or dirty resident bodies.
    pub(crate) fn next_retirement(&self) -> Option<(ChunkKey, u64)> {
        let key = *self.retireable.first()?;
        #[cfg(test)]
        self.retirement_work.set((
            self.retirement_work.get().0 + 1,
            self.retirement_work.get().1,
        ));
        Some((
            key,
            self.records
                .get(&key)
                .expect("indexed record")
                .facts
                .generation,
        ))
    }
    /// Authority holds the exclusive book borrow across whole-owner admission.
    pub(crate) fn retired(&mut self, key: ChunkKey, generation: u64) {
        debug_assert!(self.retireable.contains(&key));
        debug_assert!(
            self.records
                .get(&key)
                .is_some_and(|r| r.facts.generation == generation && Self::can_retire(r))
        );
        debug_assert!(!self.flights.contains_key(&key));
        self.retireable.remove(&key);
        self.records.remove(&key);
        #[cfg(test)]
        self.retirement_work.set((
            self.retirement_work.get().0,
            self.retirement_work.get().1 + 1,
        ));
    }
    pub(crate) fn save_error(&mut self, key: ChunkKey, error: ServerError) {
        self.refresh(key, |r| {
            r.estimate = None;
            r.error = Some(error);
        });
    }
    pub(crate) fn next_save(&self, mode: SaveMode) -> Option<(ChunkKey, usize, SaveUrgency)> {
        let &(ready, key) = self.eligible.first()?;
        if mode == SaveMode::Urgent && ready {
            return None;
        }
        Some((
            key,
            self.records.get(&key)?.estimate?,
            if ready {
                SaveUrgency::Autosave
            } else {
                SaveUrgency::Unload
            },
        ))
    }
    pub(crate) fn selected(&mut self, snapshot: OwnedSnapshot) -> Result<(), ServerError> {
        capacity(Resource::SaveChunks, self.flights.len() + 1, MAX_ATTEMPTS)?;
        let SaveKey::Chunk(key) = snapshot.key else {
            return Err(invalid("chunk_save_target"));
        };
        let Some(r) = self.records.get(&key) else {
            return Err(invalid("chunk_save_target"));
        };
        let ready = r.facts.phase == LiveChunkPhase::Ready;
        let valid_view = matches!(&snapshot.value, SaveValue::ChunkView(v)
            if v.key() == key && v.generation() == r.facts.generation && v.revision() == r.facts.revision);
        if !valid_view
            || snapshot.revision != r.facts.revision
            || Some(snapshot.estimated_bytes) != r.estimate
            || snapshot.urgency
                != if ready {
                    SaveUrgency::Autosave
                } else {
                    SaveUrgency::Unload
                }
            || !self.eligible.contains(&(ready, key))
            || self.flights.contains_key(&key)
        {
            return Err(ServerError::Internal {
                invariant: "chunk save identity",
            });
        }
        #[cfg(test)]
        self.save_work
            .set((self.save_work.get().0 + 1, self.save_work.get().1));
        self.flight_bytes += snapshot.estimated_bytes as u64;
        self.flights.insert(key, snapshot);
        self.retireable.remove(&key);
        self.eligible.remove(&(ready, key));
        Ok(())
    }
    pub(crate) fn has_target(&self, key: ChunkKey, revision: u64) -> bool {
        self.flights
            .get(&key)
            .is_some_and(|s| s.revision == revision)
    }
    pub(crate) fn matches(&self, snapshot: &OwnedSnapshot) -> bool {
        let SaveKey::Chunk(key) = snapshot.key else {
            return false;
        };
        self.flights.get(&key) == Some(snapshot)
    }
    // Validate every exact preimage before a batch mutates any durability fact.
    pub(crate) fn validate_saved(&self, snapshot: &OwnedSnapshot) -> Result<(), ServerError> {
        #[cfg(test)]
        self.save_work
            .set((self.save_work.get().0, self.save_work.get().1 + 1));
        if !self.matches(snapshot) {
            return Ok(());
        }
        let SaveKey::Chunk(key) = snapshot.key else {
            unreachable!();
        };
        let SaveValue::ChunkView(view) = &snapshot.value else {
            unreachable!();
        };
        if !self.records.get(&key).is_some_and(|r| {
            r.facts.generation == view.generation() && snapshot.revision <= r.facts.revision
        }) {
            return Err(ServerError::Internal {
                invariant: "chunk save identity",
            });
        }
        Ok(())
    }
    pub(crate) fn saved(&mut self, snapshot: &OwnedSnapshot) -> Result<bool, ServerError> {
        self.validate_saved(snapshot)?;
        if !self.matches(snapshot) {
            return Ok(false);
        }
        let SaveKey::Chunk(key) = snapshot.key else {
            unreachable!();
        };
        self.release(snapshot);
        self.refresh(key, |r| {
            r.facts.persisted_revision = r.facts.persisted_revision.max(snapshot.revision);
            if snapshot.revision == r.facts.revision {
                r.facts.needs_rewrite = false;
            }
        });
        Ok(true)
    }
    pub(crate) fn release(&mut self, snapshot: &OwnedSnapshot) -> bool {
        if !self.matches(snapshot) {
            return false;
        }
        let SaveKey::Chunk(key) = snapshot.key else {
            unreachable!();
        };
        self.flights.remove(&key);
        self.flight_bytes -= snapshot.estimated_bytes as u64;
        self.refresh(key, |_| {});
        true
    }
    pub(crate) fn stats(&self) -> SaveStats {
        SaveStats {
            dirty: self.dirty.len(),
            in_flight: self.flights.len(),
            estimated_unsaved_bytes: usize::try_from(self.dirty_bytes + self.flight_bytes)
                .unwrap_or(usize::MAX),
        }
    }
    // This explicit enumeration belongs to off-tick freeze, never selection.
    pub(crate) fn save_keys(&self) -> Vec<SaveKey> {
        self.dirty
            .iter()
            .copied()
            .chain(self.flights.keys().copied())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .map(SaveKey::Chunk)
            .collect()
    }
}

#[cfg(test)]
impl AcquisitionState {
    pub(crate) fn reset_retirement_work(&self) {
        self.retirement_work.set((0, 0));
    }
    pub(crate) fn retirement_work(&self) -> (usize, usize) {
        self.retirement_work.get()
    }
    pub(crate) fn records_test_generation(&mut self, key: ChunkKey, generation: u64) {
        self.records.get_mut(&key).unwrap().facts.generation = generation;
    }
    pub(crate) fn reset_save_work(&self) {
        self.save_work.set((0, 0));
    }
    pub(crate) fn save_work(&self) -> (usize, usize) {
        self.save_work.get()
    }
    pub(crate) fn ownership_counts(&self) -> (usize, usize, usize, usize, usize, usize) {
        (
            self.records.len(),
            self.loads.len(),
            self.generations.len(),
            self.staged.len(),
            self.load_count,
            self.generation_count,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mornlea_domain::{ChunkPos, Dimension};
    fn key(x: i32) -> ChunkKey {
        ChunkKey {
            dimension: Dimension::OVERWORLD,
            pos: ChunkPos::new(x, 0),
        }
    }
    fn request(id: u64) -> ChunkRequestId {
        ChunkRequestId::try_new(id).unwrap()
    }
    fn book(n: i32) -> AcquisitionState {
        let mut b = AcquisitionState::default();
        b.enable();
        b.replace_wants((0..n).map(key).collect()).unwrap();
        b
    }
    fn missing(key: ChunkKey, generation: u64, request: ChunkRequestId) -> AcquiredChunkEvent {
        AcquiredChunkEvent::Load {
            key,
            generation,
            request,
            result: Ok(None),
        }
    }
    #[test]
    fn load_bound_abort_alias_and_scalar_overflow_leave_original_ownership() {
        let mut b = book(9);
        let reservations: Vec<_> = (0..8).map(|x| b.reserve_load(key(x)).unwrap()).collect();
        assert_eq!(
            b.reserve_load(key(8)),
            Err(ServerError::Capacity {
                resource: Resource::ChunkRequests,
                limit: 8,
                observed: 9
            })
        );
        b.bind_load(reservations[0], request(1)).unwrap();
        assert_eq!(
            b.bind_load(reservations[1], request(1)),
            Err(invalid("chunk_request_identity"))
        );
        assert_eq!(
            b.abort_load(reservations[0], ServerError::Cancelled),
            Err(invalid("chunk_reservation"))
        );
        b.abort_load(reservations[1], ServerError::Cancelled)
            .unwrap();
        let retry = b.reserve_load(key(1)).unwrap();
        assert!(retry.generation() > reservations[1].generation());
        assert_eq!(
            b.bind_load(reservations[1], request(2)),
            Err(invalid("chunk_reservation"))
        );
        b.abort_load(retry, ServerError::Cancelled).unwrap();
        let before = b.ownership_counts();
        b.last_generation = u64::MAX;
        assert_eq!(b.reserve_load(key(1)), Err(invalid("chunk_generation")));
        assert_eq!(b.ownership_counts(), before);
        assert_eq!(b.loads.get(&request(1)), Some(&(key(0), 1)));
    }
    #[test]
    fn separate_aliases_eight_each_and_sixteen_staged_with_no_history() {
        let mut b = book(17);
        for x in 0..8 {
            let r = b.reserve_load(key(x)).unwrap();
            let id = request(x as u64 + 1);
            b.bind_load(r, id).unwrap();
            b.offer(missing(key(x), r.generation(), id)).unwrap();
        }
        for event in b.drain() {
            assert!(b.settle(&event));
            let (_, key, _, _) = event.identity();
            b.missing(key);
        }
        let mut generations = Vec::new();
        for x in 0..8 {
            let r = b.reserve_generation(key(x)).unwrap();
            b.bind_generation(r, request(x as u64 + 1)).unwrap();
            generations.push(r);
        }
        let r = b.reserve_load(key(16)).unwrap();
        b.bind_load(r, request(99)).unwrap();
        let event = missing(key(16), r.generation(), request(99));
        b.offer(event).unwrap();
        let event = b.drain().pop().unwrap();
        assert!(b.settle(&event));
        b.missing(key(16));
        assert_eq!(
            b.reserve_generation(key(16)),
            Err(ServerError::Capacity {
                resource: Resource::ChunkRequests,
                limit: 8,
                observed: 9
            })
        );
        for x in 8..16 {
            let r = b.reserve_load(key(x)).unwrap();
            let id = request(x as u64 - 7);
            b.bind_load(r, id).unwrap();
            b.offer(missing(key(x), r.generation(), id)).unwrap();
        }
        for (x, r) in generations.iter().enumerate() {
            b.offer(AcquiredChunkEvent::Generated {
                key: r.key(),
                generation: r.generation(),
                request: request(x as u64 + 1),
                result: Err(ServerError::Cancelled),
            })
            .unwrap();
        }
        assert_eq!(b.ownership_counts(), (17, 8, 8, 16, 8, 8));
        let refused = b
            .offer(missing(
                key(8),
                b.facts(key(8)).unwrap().generation,
                request(1),
            ))
            .unwrap_err();
        assert_eq!(
            refused.error,
            ServerError::Capacity {
                resource: Resource::ChunkResults,
                limit: 16,
                observed: 17
            }
        );
        b.replace_wants(BTreeSet::new()).unwrap();
        for event in b.drain() {
            assert!(b.settle(&event));
            let (_, key, _, _) = event.identity();
            b.failed(key, ServerError::Cancelled);
        }
        assert_eq!(b.ownership_counts(), (0, 0, 0, 0, 0, 0));
    }
    #[test]
    fn wanted_and_managed_limits_refuse_before_mutation() {
        let mut b = book(1);
        assert_eq!(
            b.replace_wants((0..36_661).map(key).collect()),
            Err(ServerError::Capacity {
                resource: Resource::ChunkWants,
                limit: 36_660,
                observed: 36_661
            })
        );
        assert_eq!(b.wants, BTreeSet::from([key(0)]));
        for x in 1..=36_676 {
            b.records.insert(
                key(x),
                Record {
                    facts: LiveChunkFacts {
                        generation: 1,
                        revision: 1,
                        persisted_revision: 0,
                        needs_rewrite: false,
                        recovered: false,
                        wanted: false,
                        phase: LiveChunkPhase::Unloading,
                    },
                    request: None,
                    error: None,
                    estimate: Some(4096),
                },
            );
        }
        assert_eq!(
            b.reserve_load(key(0)),
            Err(ServerError::Capacity {
                resource: Resource::ResidentChunks,
                limit: 36_676,
                observed: 36_677
            })
        );
        assert_eq!(b.last_generation, 0);
        assert_eq!(b.load_count, 0);
    }
    #[test]
    fn wrong_outer_source_request_and_duplicate_preserve_expected_alias() {
        let mut b = book(2);
        let r = b.reserve_load(key(0)).unwrap();
        b.bind_load(r, request(1)).unwrap();
        for event in [
            missing(key(1), r.generation(), request(1)),
            missing(key(0), r.generation() + 1, request(1)),
            missing(key(0), r.generation(), request(2)),
            AcquiredChunkEvent::Generated {
                key: key(0),
                generation: r.generation(),
                request: request(1),
                result: Err(ServerError::Cancelled),
            },
        ] {
            let identity = event.identity();
            let rejected = b.offer(event).unwrap_err();
            assert_eq!(rejected.error, invalid("chunk_completion_identity"));
            assert_eq!(rejected.event.identity(), identity);
        }
        b.offer(missing(key(0), r.generation(), request(1)))
            .unwrap();
        assert_eq!(
            b.offer(missing(key(0), r.generation(), request(1)))
                .unwrap_err()
                .error,
            invalid("chunk_completion_identity")
        );
        assert_eq!(b.loads.get(&request(1)), Some(&(key(0), r.generation())));
        let stale = missing(key(0), r.generation() + 1, request(1));
        assert!(!b.settle(&stale));
        assert_eq!(b.load_count, 1);
    }
}
