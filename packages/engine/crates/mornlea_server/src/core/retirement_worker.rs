//! Bounded CPU ownership of whole detached chunk destruction.

use std::collections::VecDeque;
use std::sync::mpsc::{self, Receiver, Sender, SyncSender, TryRecvError, TrySendError};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use super::chunk_retirement::{
    ChunkRetirementPort, MAX_CHUNK_RETIREMENTS, RejectedRetiredChunk, RetiredChunk, RetiredChunkId,
};
use super::contracts::{Deadline, Operation, Resource, ServerError, ServerPhase};

// Caller metadata never owns a body, including completed but uncollected jobs.
struct Entry {
    id: RetiredChunkId,
    complete: bool,
}

/// Transfers at most eight whole retired bodies to one CPU owner.
/// Only explicit successful close proves disposal, collection and actual join;
/// caller Drop disconnects without moving queued destructors onto the caller.
pub struct BackgroundChunkRetirement {
    entries: VecDeque<Entry>,
    sender: Option<SyncSender<RetiredChunk>>,
    replies: Receiver<RetiredChunkId>,
    join: Option<JoinHandle<()>>,
    closing: bool,
    joined: bool,
    failed: bool,
}

impl BackgroundChunkRetirement {
    /// Starts exactly one destructor owner; no authority or I/O state crosses it.
    pub fn try_new() -> Result<Self, ServerError> {
        Self::spawn(|commands, replies| {
            run(
                commands,
                replies,
                #[cfg(test)]
                None,
            );
        })
    }

    fn spawn(
        work: impl FnOnce(Receiver<RetiredChunk>, Sender<RetiredChunkId>) + Send + 'static,
    ) -> Result<Self, ServerError> {
        let (sender, commands) = mpsc::sync_channel(MAX_CHUNK_RETIREMENTS);
        let (completions, replies) = mpsc::channel();
        let join = thread::Builder::new()
            .name("chunk-retirement".into())
            .spawn(move || work(commands, completions))
            .map_err(|error| ServerError::Io {
                operation: Operation::Retire,
                kind: error.kind(),
            })?;
        Ok(Self {
            entries: VecDeque::with_capacity(MAX_CHUNK_RETIREMENTS),
            sender: Some(sender),
            replies,
            join: Some(join),
            closing: false,
            joined: false,
            failed: false,
        })
    }

    // Failure never frees a lost obligation or claims a successful close. A
    // running handle remains retryable; a finished handle is explicitly joined.
    fn failed_close(&mut self) -> Result<(), ServerError> {
        drop(self.sender.take());
        if self.join.as_ref().is_some_and(JoinHandle::is_finished) {
            let _ = self
                .join
                .take()
                .expect("finished owner retains its handle")
                .join();
        }
        Err(worker_failure())
    }

    #[cfg(test)]
    fn from_hooks(hooks: Hooks) -> Result<Self, ServerError> {
        Self::spawn(move |commands, replies| run(commands, replies, Some(hooks)))
    }
}

impl ChunkRetirementPort for BackgroundChunkRetirement {
    fn submit(&mut self, owner: RetiredChunk) -> Result<(), RejectedRetiredChunk> {
        let error = if self.closing || self.joined {
            Some(ServerError::InvalidState {
                phase: ServerPhase::Closing,
            })
        } else if self.entries.len() == MAX_CHUNK_RETIREMENTS {
            Some(full())
        } else if self.entries.iter().any(|entry| entry.id == owner.id()) {
            Some(ServerError::InvalidInput {
                field: "chunk_retirement_identity",
            })
        } else {
            None
        };
        if let Some(error) = error {
            return Err(RejectedRetiredChunk { error, owner });
        }
        let Some(sender) = self.sender.as_ref() else {
            return Err(RejectedRetiredChunk {
                error: ServerError::Disconnected,
                owner,
            });
        };
        let id = owner.id();
        match sender.try_send(owner) {
            Ok(()) => {
                // Admission cannot refuse after the irreversible whole-body
                // transfer. The preallocated scalar lane retains its charge.
                self.entries.push_back(Entry {
                    id,
                    complete: false,
                });
                Ok(())
            }
            Err(TrySendError::Full(owner)) => Err(RejectedRetiredChunk {
                error: full(),
                owner,
            }),
            Err(TrySendError::Disconnected(owner)) => Err(RejectedRetiredChunk {
                error: ServerError::Disconnected,
                owner,
            }),
        }
    }

    fn collect(&mut self, max_reports: usize) -> Result<Vec<RetiredChunkId>, ServerError> {
        if self.failed {
            return Err(worker_failure());
        }
        // At most eight jobs can produce replies. Matching and polling remain
        // bounded even for zero reports, and malformed private replies stick.
        for _ in 0..MAX_CHUNK_RETIREMENTS {
            match self.replies.try_recv() {
                Ok(id) => {
                    if let Some(entry) = self
                        .entries
                        .iter_mut()
                        .find(|entry| entry.id == id && !entry.complete)
                    {
                        entry.complete = true;
                    } else {
                        self.failed = true;
                        break;
                    }
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    if !self.entries.is_empty() || self.sender.is_some() {
                        self.failed = true;
                    }
                    break;
                }
            }
        }
        if self.sender.is_some() && self.join.as_ref().is_some_and(JoinHandle::is_finished) {
            self.failed = true;
        }
        if self.failed {
            return Err(worker_failure());
        }
        let mut reports = Vec::with_capacity(max_reports.min(MAX_CHUNK_RETIREMENTS));
        while reports.len() < max_reports.min(MAX_CHUNK_RETIREMENTS)
            && self.entries.front().is_some_and(|entry| entry.complete)
        {
            reports.push(
                self.entries
                    .pop_front()
                    .expect("completed FIFO head exists")
                    .id,
            );
        }
        Ok(reports)
    }

    fn occupied(&self) -> usize {
        self.entries.len()
    }

    fn close(&mut self, deadline: Deadline) -> Result<(), ServerError> {
        self.closing = true;
        if self.failed {
            return self.failed_close();
        }
        if self.joined {
            return Ok(());
        }
        loop {
            if deadline.expired(Instant::now()) {
                return Err(retirement_timeout());
            }
            if let Err(error) = self.collect(MAX_CHUNK_RETIREMENTS) {
                debug_assert_eq!(error, worker_failure());
                return self.failed_close();
            }
            if self.entries.is_empty() {
                break;
            }
            pause(deadline)?;
        }
        // Disconnect only after collecting all successful disposal charges.
        // A join-only timeout retains this same handle after disconnection.
        drop(self.sender.take());
        loop {
            if deadline.expired(Instant::now()) {
                return Err(retirement_timeout());
            }
            if self.join.as_ref().is_some_and(JoinHandle::is_finished) {
                if self
                    .join
                    .take()
                    .expect("finished owner retains its handle")
                    .join()
                    .is_err()
                {
                    self.failed = true;
                    return Err(worker_failure());
                }
                self.joined = true;
                return Ok(());
            }
            pause(deadline)?;
        }
    }
}

impl Drop for BackgroundChunkRetirement {
    fn drop(&mut self) {
        // The sole receiver and every queued body stay on the CPU owner. The
        // handle detaches naturally; Drop makes no explicit-join assertion.
        drop(self.sender.take());
    }
}

fn run(
    commands: Receiver<RetiredChunk>,
    replies: Sender<RetiredChunkId>,
    #[cfg(test)] hooks: Option<Hooks>,
) {
    while let Ok(owner) = commands.recv() {
        let id = owner.id();
        #[cfg(not(test))]
        drop(owner);
        #[cfg(test)]
        if let Some(hooks) = &hooks {
            dispose_with_hooks(owner, hooks);
        } else {
            drop(owner);
        }
        // Replies contain no body. Their logical ceiling is the eight retained
        // charges; caller Drop may lose reports but cannot interrupt drainage.
        let _ = replies.send(id);
    }
    #[cfg(test)]
    if let Some(hooks) = hooks {
        hooks.finish();
    }
    // Unwinding also destroys the receiver's queued owners on this CPU thread.
}

fn full() -> ServerError {
    ServerError::Capacity {
        resource: Resource::ChunkRetirements,
        limit: MAX_CHUNK_RETIREMENTS,
        observed: MAX_CHUNK_RETIREMENTS + 1,
    }
}
fn worker_failure() -> ServerError {
    ServerError::Internal {
        invariant: "chunk retirement worker",
    }
}
fn retirement_timeout() -> ServerError {
    ServerError::Timeout {
        operation: Operation::Retire,
    }
}
fn pause(deadline: Deadline) -> Result<(), ServerError> {
    let now = Instant::now();
    if deadline.expired(now) {
        return Err(retirement_timeout());
    }
    thread::sleep(
        deadline
            .instant()
            .saturating_duration_since(now)
            .min(Duration::from_millis(1)),
    );
    Ok(())
}

// Causal hooks exist only in tests. Every successful or unwinding operation
// still executes the actual whole-owner destructor before its disposal probe.
#[cfg(test)]
#[derive(Debug, PartialEq)]
enum Event {
    Before {
        id: RetiredChunkId,
        thread: thread::ThreadId,
        name: Option<String>,
    },
    Disposed {
        id: RetiredChunkId,
        thread: thread::ThreadId,
        unwinding: bool,
    },
    Teardown {
        thread: thread::ThreadId,
    },
    Exit {
        thread: thread::ThreadId,
    },
}
#[cfg(test)]
struct Hooks {
    probe: Sender<Event>,
    permits: Option<Receiver<()>>,
    teardown: Option<Receiver<()>>,
    panic_before: bool,
    panic_teardown: bool,
    deadline: Deadline,
}
#[cfg(test)]
impl Hooks {
    fn finish(self) {
        let thread = thread::current().id();
        if let Some(gate) = self.teardown {
            let _ = self.probe.send(Event::Teardown { thread });
            gate.recv_timeout(
                self.deadline
                    .instant()
                    .saturating_duration_since(Instant::now()),
            )
            .expect("teardown permit before harness deadline");
        }
        assert!(
            !self.panic_teardown,
            "injected destructor-owner teardown panic"
        );
        let _ = self.probe.send(Event::Exit { thread });
    }
}
#[cfg(test)]
fn dispose_with_hooks(owner: RetiredChunk, hooks: &Hooks) {
    struct ObservedOwner<'a> {
        owner: Option<RetiredChunk>,
        id: RetiredChunkId,
        hooks: &'a Hooks,
    }
    impl Drop for ObservedOwner<'_> {
        fn drop(&mut self) {
            if let Some(owner) = self.owner.take() {
                drop(owner);
            }
            let _ = self.hooks.probe.send(Event::Disposed {
                id: self.id,
                thread: thread::current().id(),
                unwinding: thread::panicking(),
            });
        }
    }
    let guarded = ObservedOwner {
        id: owner.id(),
        owner: Some(owner),
        hooks,
    };
    let _ = hooks.probe.send(Event::Before {
        id: guarded.id,
        thread: thread::current().id(),
        name: thread::current().name().map(str::to_owned),
    });
    if let Some(gate) = &hooks.permits {
        gate.recv_timeout(
            hooks
                .deadline
                .instant()
                .saturating_duration_since(Instant::now()),
        )
        .expect("destructor permit before harness deadline");
    }
    assert!(
        !hooks.panic_before,
        "injected actual disposal operation panic"
    );
    drop(guarded);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::{
        block_observations::ChunkBlockObservations,
        container_store::ContainerState,
        contracts::{BlockObservation, ChunkKey},
        drop_store::DropState,
        world::{ReadyChunk, ready_clones, reset_ready_clones},
    };
    use mornlea_domain::{BlockPos, ChunkPos, Dimension};
    use mornlea_storage::{Chunk, ContainerSnapshot, StorageKind};
    use std::{collections::BTreeMap, sync::mpsc::Sender, thread::ThreadId};

    type Addresses = BTreeMap<(ChunkKey, BlockPos), (usize, usize)>;

    fn key(x: i32) -> ChunkKey {
        ChunkKey {
            dimension: Dimension::OVERWORLD,
            pos: ChunkPos::new(x, 0),
        }
    }

    fn fixture(x: i32, generation: u64, cells: bool) -> (RetiredChunk, Addresses) {
        let key = key(x);
        let chunk = Chunk {
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
        };
        let containers = ContainerState::new(&chunk, 9);
        let drops = DropState::new(key, chunk.drops.as_slice().try_into().unwrap());
        let ready = ReadyChunk::try_new(key, generation, 9, chunk).unwrap();
        let mut observations = ChunkBlockObservations::new();
        if cells {
            for index in 0..4096 {
                let pos = BlockPos::new(x * 16 + index % 16, -64 + index / 256, index / 16 % 16);
                observations.insert(
                    (key, pos),
                    BlockObservation::try_new(key, generation, 9 + index as u64, pos, 4).unwrap(),
                );
            }
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

    // Restoration checks the original nodes and scalar CAS/fixed-slot values;
    // rewrapping may replace only the small outer box allocation.
    fn preserved(owner: RetiredChunk, addresses: &Addresses) -> RetiredChunk {
        let id = owner.id();
        let (ready, drops, containers, observations) = owner.into_parts();
        assert_eq!(
            (ready.key, ready.generation, ready.revision),
            (id.key(), id.generation(), 9)
        );
        assert_eq!(drops.slots, [Default::default(); 32]);
        assert!(drops.records().is_empty());
        assert!(!drops.dirty);
        assert_eq!(containers.furnaces, [Default::default(); 32]);
        assert_eq!(containers.chests, [Default::default(); 16]);
        assert!(!containers.dirty);
        let tree = observations.as_ref().unwrap();
        assert_eq!(tree.len(), 4096);
        for (cell, value) in tree {
            assert_eq!(
                (
                    std::ptr::from_ref(cell) as usize,
                    std::ptr::from_ref(value) as usize
                ),
                addresses[cell]
            );
            let index =
                (cell.1.y() + 64) * 256 + cell.1.z() * 16 + cell.1.x() - id.key().pos.x() * 16;
            assert_eq!(
                *value,
                BlockObservation::try_new(id.key(), id.generation(), 9 + index as u64, cell.1, 4)
                    .unwrap()
            );
        }
        assert_eq!(ready_clones(), 0);
        RetiredChunk::new(ready, drops, containers, observations)
    }

    fn deadline() -> Deadline {
        Deadline::at(Instant::now() + Duration::from_secs(5))
    }
    fn timeout() -> ServerError {
        ServerError::Timeout {
            operation: Operation::Retire,
        }
    }
    fn failure() -> ServerError {
        ServerError::Internal {
            invariant: "chunk retirement worker",
        }
    }
    fn full() -> ServerError {
        ServerError::Capacity {
            resource: Resource::ChunkRetirements,
            limit: 8,
            observed: 9,
        }
    }
    fn closing() -> ServerError {
        ServerError::InvalidState {
            phase: ServerPhase::Closing,
        }
    }

    // Every wait shares the harness deadline. Unwinding releases remaining
    // permits, and disconnected probes never strand an accepted worker body.
    struct Harness {
        permits: Sender<()>,
        teardown: Sender<()>,
        events: Receiver<Event>,
        deadline: Deadline,
    }
    impl Drop for Harness {
        fn drop(&mut self) {
            for _ in 0..8 {
                let _ = self.permits.send(());
            }
            let _ = self.teardown.send(());
        }
    }
    impl Harness {
        fn new(
            gated: bool,
            teardown_gated: bool,
            panic_before: bool,
            panic_teardown: bool,
        ) -> (BackgroundChunkRetirement, Self) {
            let (permits, permit_receiver) = mpsc::channel();
            let (teardown, teardown_receiver) = mpsc::channel();
            let (probe, events) = mpsc::channel();
            let deadline = deadline();
            let hooks = Hooks {
                probe,
                permits: gated.then_some(permit_receiver),
                teardown: teardown_gated.then_some(teardown_receiver),
                panic_before,
                panic_teardown,
                deadline,
            };
            (
                BackgroundChunkRetirement::from_hooks(hooks).unwrap(),
                Self {
                    permits,
                    teardown,
                    events,
                    deadline,
                },
            )
        }
        fn event(&self) -> Event {
            self.events
                .recv_timeout(
                    self.deadline
                        .instant()
                        .saturating_duration_since(Instant::now()),
                )
                .expect("actual worker event before harness deadline")
        }
        fn before(&self, expected: RetiredChunkId) -> ThreadId {
            match self.event() {
                Event::Before { id, thread, name } => {
                    assert_eq!(id, expected);
                    assert_eq!(name.as_deref(), Some("chunk-retirement"));
                    assert_ne!(thread, thread::current().id());
                    thread
                }
                other => panic!("expected actual before-drop gate, got {other:?}"),
            }
        }
        fn disposed(&self, expected: RetiredChunkId, expected_thread: ThreadId, unwinding: bool) {
            assert_eq!(
                self.event(),
                Event::Disposed {
                    id: expected,
                    thread: expected_thread,
                    unwinding
                }
            );
        }
        fn release(&self) {
            self.permits.send(()).unwrap();
        }
        fn drain(&self, pool: &mut BackgroundChunkRetirement, count: usize) -> Vec<RetiredChunkId> {
            drain(pool, count, self.deadline)
        }
    }
    fn drain(
        pool: &mut BackgroundChunkRetirement,
        count: usize,
        deadline: Deadline,
    ) -> Vec<RetiredChunkId> {
        let mut reports = vec![];
        while reports.len() < count {
            assert!(
                !deadline.expired(Instant::now()),
                "actual retirement completion before harness deadline"
            );
            reports.extend(pool.collect(count - reports.len()).unwrap());
            thread::yield_now();
        }
        reports
    }
    fn finished(pool: &BackgroundChunkRetirement, deadline: Deadline) {
        while !pool.join.as_ref().unwrap().is_finished() {
            assert!(
                !deadline.expired(Instant::now()),
                "actual worker exit before harness deadline"
            );
            thread::yield_now();
        }
    }
    fn recognized(pool: &mut BackgroundChunkRetirement, count: usize, deadline: Deadline) {
        while pool.entries.iter().filter(|entry| entry.complete).count() < count {
            assert!(
                !deadline.expired(Instant::now()),
                "scalar acknowledgment before harness deadline"
            );
            assert!(pool.collect(0).unwrap().is_empty());
            thread::yield_now();
        }
    }

    #[test]
    fn production_default_disposes_sixteen_whole_owners_in_fifo_cohorts() {
        let mut pool = BackgroundChunkRetirement::try_new().unwrap();
        let deadline = deadline();
        let mut ids = vec![];
        for cohort in 0..2 {
            for x in cohort * 8..cohort * 8 + 8 {
                let (owner, _) = fixture(x, 7, true);
                ids.push(owner.id());
                pool.submit(owner).unwrap();
            }
            assert_eq!(pool.occupied(), 8);
            assert_eq!(ready_clones(), 0);
            assert_eq!(
                drain(&mut pool, 8, deadline),
                ids[cohort as usize * 8..cohort as usize * 8 + 8]
            );
            assert_eq!(pool.occupied(), 0);
            assert!(pool.entries.is_empty());
            assert!(pool.collect(usize::MAX).unwrap().is_empty());
        }
        pool.close(deadline).unwrap();
        assert!(pool.joined);
        assert!(pool.join.is_none());
        pool.close(Deadline::at(Instant::now())).unwrap();
    }

    #[test]
    fn actual_drop_wrapper_records_sixteen_disposals_on_one_distinct_cpu_thread() {
        let (mut pool, harness) = Harness::new(false, false, false, false);
        let mut worker = None;
        for cohort in 0..2 {
            let mut ids = vec![];
            for x in cohort * 8..cohort * 8 + 8 {
                let (owner, _) = fixture(x, 7, true);
                ids.push(owner.id());
                pool.submit(owner).unwrap();
            }
            for id in &ids {
                let observed = harness.before(*id);
                assert_eq!(*worker.get_or_insert(observed), observed);
                harness.disposed(*id, observed, false);
            }
            assert_eq!(pool.occupied(), 8);
            assert_eq!(harness.drain(&mut pool, 8), ids);
            assert_eq!(ready_clones(), 0);
        }
        pool.close(harness.deadline).unwrap();
        assert_eq!(
            harness.event(),
            Event::Exit {
                thread: worker.unwrap()
            }
        );
    }

    #[test]
    fn causal_eight_slots_include_actual_started_held_and_queued_whole_owners() {
        let (mut pool, harness) = Harness::new(true, false, false, false);
        let mut ids = vec![];
        for x in 0..8 {
            let (owner, _) = fixture(x, 7, true);
            ids.push(owner.id());
            pool.submit(owner).unwrap();
        }
        let worker = harness.before(ids[0]);
        let (ninth, addresses) = fixture(8, 7, true);
        let rejected = pool.submit(ninth).unwrap_err();
        assert_eq!(rejected.error, full());
        let ninth = preserved(rejected.owner, &addresses);
        harness.release();
        harness.disposed(ids[0], worker, false);
        assert_eq!(harness.before(ids[1]), worker);
        recognized(&mut pool, 1, harness.deadline);
        assert_eq!(pool.occupied(), 8);
        assert!(pool.collect(0).unwrap().is_empty());
        let rejected = pool.submit(ninth).unwrap_err();
        assert_eq!(rejected.error, full());
        let ninth = preserved(rejected.owner, &addresses);
        assert_eq!(pool.collect(1).unwrap(), ids[..1]);
        assert_eq!(pool.occupied(), 7);
        ids.push(ninth.id());
        pool.submit(ninth).unwrap();
        assert_eq!(pool.occupied(), 8);
        for index in 1..9 {
            harness.release();
            harness.disposed(ids[index], worker, false);
            if index < 8 {
                assert_eq!(harness.before(ids[index + 1]), worker);
            }
        }
        assert_eq!(harness.drain(&mut pool, 8), ids[1..]);
        pool.close(harness.deadline).unwrap();
        assert!(pool.joined);
        assert_eq!(harness.event(), Event::Exit { thread: worker });
    }

    #[test]
    fn duplicate_refusal_preserves_body_and_fresh_generation_is_distinct() {
        let (mut pool, harness) = Harness::new(true, false, false, false);
        let (original, _) = fixture(0, 7, true);
        let id = original.id();
        pool.submit(original).unwrap();
        let worker = harness.before(id);
        let (incoming, addresses) = fixture(0, 7, true);
        let rejected = pool.submit(incoming).unwrap_err();
        assert_eq!(
            rejected.error,
            ServerError::InvalidInput {
                field: "chunk_retirement_identity"
            }
        );
        assert_eq!(pool.occupied(), 1);
        drop(preserved(rejected.owner, &addresses));
        let (fresh, _) = fixture(0, 8, true);
        let fresh_id = fresh.id();
        assert_ne!(id, fresh_id);
        pool.submit(fresh).unwrap();
        harness.release();
        harness.disposed(id, worker, false);
        assert_eq!(harness.before(fresh_id), worker);
        harness.release();
        harness.disposed(fresh_id, worker, false);
        assert_eq!(harness.drain(&mut pool, 2), vec![id, fresh_id]);
        pool.close(harness.deadline).unwrap();
    }

    #[test]
    fn collection_zero_partial_maximum_and_repeated_empty_free_exact_prefix() {
        let (mut pool, harness) = Harness::new(true, false, false, false);
        let mut ids = vec![];
        for x in 0..8 {
            let (owner, _) = fixture(x, 7, false);
            ids.push(owner.id());
            pool.submit(owner).unwrap();
        }
        let worker = harness.before(ids[0]);
        assert!(pool.collect(usize::MAX).unwrap().is_empty());
        for index in 0..3 {
            harness.release();
            harness.disposed(ids[index], worker, false);
            assert_eq!(harness.before(ids[index + 1]), worker);
        }
        recognized(&mut pool, 3, harness.deadline);
        assert_eq!(pool.occupied(), 8);
        assert_eq!(pool.collect(1).unwrap(), ids[..1]);
        assert_eq!(pool.occupied(), 7);
        assert_eq!(pool.collect(usize::MAX).unwrap(), ids[1..3]);
        assert_eq!(pool.occupied(), 5);
        assert!(pool.collect(8).unwrap().is_empty());
        for index in 3..8 {
            harness.release();
            harness.disposed(ids[index], worker, false);
            if index < 7 {
                assert_eq!(harness.before(ids[index + 1]), worker);
            }
        }
        assert_eq!(harness.drain(&mut pool, 5), ids[3..]);
        assert!(pool.collect(0).unwrap().is_empty());
        assert!(pool.collect(usize::MAX).unwrap().is_empty());
        assert_eq!(pool.occupied(), 0);
        pool.close(harness.deadline).unwrap();
    }

    #[test]
    fn drain_timeout_retains_owner_sender_and_join_for_same_pool_retry() {
        let (mut pool, harness) = Harness::new(true, false, false, false);
        let (owner, _) = fixture(0, 7, true);
        let id = owner.id();
        pool.submit(owner).unwrap();
        let worker = harness.before(id);
        assert_eq!(
            pool.close(Deadline::at(Instant::now() + Duration::from_millis(10))),
            Err(timeout())
        );
        assert_eq!(pool.occupied(), 1);
        assert!(pool.sender.is_some());
        assert!(pool.join.is_some());
        assert!(!pool.joined);
        let (incoming, addresses) = fixture(1, 7, true);
        let rejected = pool.submit(incoming).unwrap_err();
        assert_eq!(rejected.error, closing());
        drop(preserved(rejected.owner, &addresses));
        harness.release();
        harness.disposed(id, worker, false);
        pool.close(harness.deadline).unwrap();
        assert_eq!(pool.occupied(), 0);
        assert!(pool.joined);
        assert!(pool.join.is_none());
        assert_eq!(harness.event(), Event::Exit { thread: worker });
        assert!(matches!(
            harness.events.try_recv(),
            Err(TryRecvError::Disconnected)
        ));
    }

    #[test]
    fn expired_deadline_stops_before_collecting_held_completion() {
        let (mut pool, harness) = Harness::new(false, false, false, false);
        let (owner, _) = fixture(0, 7, false);
        let id = owner.id();
        pool.submit(owner).unwrap();
        let worker = harness.before(id);
        harness.disposed(id, worker, false);
        recognized(&mut pool, 1, harness.deadline);
        assert_eq!(pool.close(Deadline::at(Instant::now())), Err(timeout()));
        assert_eq!(pool.occupied(), 1);
        assert!(pool.sender.is_some());
        pool.close(harness.deadline).unwrap();
    }

    #[test]
    fn join_only_timeout_retains_zero_charge_handle_then_explicitly_joins() {
        let (mut pool, harness) = Harness::new(false, true, false, false);
        assert_eq!(
            pool.close(Deadline::at(Instant::now() + Duration::from_millis(10))),
            Err(timeout())
        );
        let worker = match harness.event() {
            Event::Teardown { thread } => thread,
            other => panic!("expected teardown gate: {other:?}"),
        };
        assert_eq!(pool.occupied(), 0);
        assert!(pool.sender.is_none());
        assert!(pool.join.is_some());
        assert!(!pool.joined);
        harness.teardown.send(()).unwrap();
        pool.close(harness.deadline).unwrap();
        assert!(pool.joined);
        assert!(pool.join.is_none());
        assert_eq!(harness.event(), Event::Exit { thread: worker });
        let (incoming, addresses) = fixture(0, 7, true);
        let rejected = pool.submit(incoming).unwrap_err();
        assert_eq!(rejected.error, closing());
        drop(preserved(rejected.owner, &addresses));
    }

    #[test]
    fn actual_disposal_then_teardown_panic_is_sticky_even_after_join_consumption() {
        let (mut pool, harness) = Harness::new(false, false, false, true);
        let (owner, _) = fixture(0, 7, true);
        let id = owner.id();
        pool.submit(owner).unwrap();
        let worker = harness.before(id);
        harness.disposed(id, worker, false);
        assert_eq!(harness.drain(&mut pool, 1), vec![id]);
        assert_eq!(pool.close(harness.deadline), Err(failure()));
        assert!(pool.failed);
        assert!(!pool.joined);
        assert!(pool.join.is_none());
        assert_eq!(pool.close(harness.deadline), Err(failure()));
        assert_eq!(pool.collect(0), Err(failure()));
        assert_eq!(pool.occupied(), 0);
    }

    #[test]
    fn operation_panic_unwinds_actual_body_on_cpu_without_successful_report() {
        let (mut pool, harness) = Harness::new(false, false, true, false);
        let (owner, _) = fixture(0, 7, true);
        let id = owner.id();
        pool.submit(owner).unwrap();
        let worker = harness.before(id);
        harness.disposed(id, worker, true);
        finished(&pool, harness.deadline);
        let (incoming, addresses) = fixture(1, 7, true);
        let rejected = pool.submit(incoming).unwrap_err();
        assert_eq!(rejected.error, ServerError::Disconnected);
        drop(preserved(rejected.owner, &addresses));
        assert_eq!(pool.occupied(), 1);
        assert_eq!(pool.collect(usize::MAX), Err(failure()));
        assert_eq!(pool.occupied(), 1);
        assert_eq!(pool.close(harness.deadline), Err(failure()));
        assert_eq!(pool.occupied(), 1);
        assert!(pool.join.is_none());
        assert!(!pool.joined);
        assert_eq!(pool.close(harness.deadline), Err(failure()));
    }

    #[test]
    fn caller_drop_disconnects_and_actual_cpu_drains_started_and_queued_bodies() {
        let (mut pool, harness) = Harness::new(true, false, false, false);
        let (first, _) = fixture(0, 7, true);
        let first_id = first.id();
        pool.submit(first).unwrap();
        let worker = harness.before(first_id);
        let (second, _) = fixture(1, 7, true);
        let second_id = second.id();
        pool.submit(second).unwrap();
        drop(pool);
        harness.release();
        harness.disposed(first_id, worker, false);
        assert_eq!(harness.before(second_id), worker);
        harness.release();
        harness.disposed(second_id, worker, false);
        assert_eq!(harness.event(), Event::Exit { thread: worker });
        // Drop detaches; an exit probe establishes disposal, not explicit join.
    }
    #[test]
    fn full_precedes_duplicate_and_closing_precedes_full_preserving_incoming_nodes() {
        let (mut pool, harness) = Harness::new(true, false, false, false);
        let mut ids = vec![];
        for x in 0..8 {
            let (owner, _) = fixture(x, 7, false);
            ids.push(owner.id());
            pool.submit(owner).unwrap();
        }
        let worker = harness.before(ids[0]);
        let (incoming, addresses) = fixture(0, 7, true);
        let rejected = pool.submit(incoming).unwrap_err();
        assert_eq!(rejected.error, full());
        let incoming = preserved(rejected.owner, &addresses);
        assert_eq!(pool.close(Deadline::at(Instant::now())), Err(timeout()));
        let rejected = pool.submit(incoming).unwrap_err();
        assert_eq!(rejected.error, closing());
        drop(preserved(rejected.owner, &addresses));
        for index in 0..8 {
            harness.release();
            harness.disposed(ids[index], worker, false);
            if index < 7 {
                assert_eq!(harness.before(ids[index + 1]), worker);
            }
        }
        pool.close(harness.deadline).unwrap();
    }

    #[test]
    fn unknown_scalar_fault_retains_charges_and_running_join_until_same_pool_retry() {
        let (mut pool, harness) = Harness::new(true, false, false, false);
        let (owner, _) = fixture(0, 7, true);
        let id = owner.id();
        pool.submit(owner).unwrap();
        let worker = harness.before(id);
        let (incoming, _) = fixture(1, 7, false);
        let unknown = incoming.id();
        drop(incoming);
        // Corrupt only the private scalar channel; this is fault evidence and
        // cannot acknowledge successful disposal of the held body.
        let (inject, replies) = mpsc::channel();
        pool.replies = replies;
        inject.send(unknown).unwrap();
        assert_eq!(pool.collect(usize::MAX), Err(failure()));
        assert_eq!(pool.occupied(), 1);
        assert_eq!(pool.close(harness.deadline), Err(failure()));
        assert!(pool.sender.is_none());
        assert!(pool.join.is_some());
        assert!(!pool.joined);
        harness.release();
        harness.disposed(id, worker, false);
        assert_eq!(harness.event(), Event::Exit { thread: worker });
        if pool.join.is_some() {
            finished(&pool, harness.deadline);
        }
        assert_eq!(pool.close(harness.deadline), Err(failure()));
        assert!(pool.join.is_none());
        assert_eq!(pool.occupied(), 1);
        assert_eq!(pool.collect(usize::MAX), Err(failure()));
    }

    #[test]
    fn duplicate_scalar_fault_after_actual_disposal_is_sticky_without_releasing_charge() {
        let (mut pool, harness) = Harness::new(false, false, false, false);
        let (owner, _) = fixture(0, 7, true);
        let id = owner.id();
        pool.submit(owner).unwrap();
        let worker = harness.before(id);
        harness.disposed(id, worker, false);
        recognized(&mut pool, 1, harness.deadline);
        let (inject, replies) = mpsc::channel();
        pool.replies = replies;
        inject.send(id).unwrap();
        assert_eq!(pool.collect(1), Err(failure()));
        assert_eq!(pool.occupied(), 1);
        assert_eq!(pool.close(harness.deadline), Err(failure()));
        assert_eq!(harness.event(), Event::Exit { thread: worker });
        if pool.join.is_some() {
            finished(&pool, harness.deadline);
        }
        assert_eq!(pool.close(harness.deadline), Err(failure()));
        assert!(pool.join.is_none());
        assert!(!pool.joined);
    }

    #[test]
    fn unexpected_scalar_disconnect_with_live_sender_is_sticky_even_without_charges() {
        let (mut pool, harness) = Harness::new(false, false, false, false);
        let (inject, replies) = mpsc::channel();
        pool.replies = replies;
        drop(inject);
        assert_eq!(pool.collect(0), Err(failure()));
        assert_eq!(pool.occupied(), 0);
        assert!(pool.sender.is_some());
        assert_eq!(pool.close(harness.deadline), Err(failure()));
        assert!(matches!(harness.event(), Event::Exit { .. }));
        if pool.join.is_some() {
            finished(&pool, harness.deadline);
        }
        assert_eq!(pool.close(harness.deadline), Err(failure()));
        assert!(pool.join.is_none());
        assert!(!pool.joined);
    }
}
