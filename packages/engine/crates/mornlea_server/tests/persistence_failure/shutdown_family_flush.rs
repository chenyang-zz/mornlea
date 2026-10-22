//! Production per-family shutdown flush over the real autosave scheduler.
//!
//! The shutdown machine hands the store one save family at a time in the
//! fixed core order players, companions, hostiles, passives, world chunks and
//! metadata, mirroring Go `host_shutdown.go:83` and `shutdown.go:152-193`
//! (world chunks then metadata at `persistence/world.go:196-199`). These
//! cases drive the `store::shutdown` ports over a background `DiskStore`
//! wrapped in a recording backend, so each assertion reads what actually
//! reached the backend rather than what a double claims.

use std::sync::{Arc, Mutex};
use std::time::Instant;

use mornlea_server::contracts::{
    ActorPersistence, AgentHandle, AgentPoll, AgentRequest, AgentRequestId, Clock, Deadline,
    DiskBackend, FinalReducer, FrozenLease, LoadedValue, McpLifecycle, MemoryFinalizationReport,
    MemoryFinalizer, Operation, SaveCompletion, SaveKey, SaveRequest, SaveTicket, ServerError,
    ShutdownPhase, StoreHandle, WorkerLifecycle,
};
use mornlea_server::core::shutdown::{ShutdownPorts, shutdown};
use mornlea_server::state::AuthorityState;
use mornlea_server::store::disk::DiskStore;
use mornlea_server::store::shutdown::SharedScheduler;

use super::authority_actor_saves::{RealClock, Root, deadline, id, key, real_setup_on, reopen};

#[derive(Clone, Default)]
struct Log {
    /// Every key/revision the backend committed, in write order.
    written: Arc<Mutex<Vec<(SaveKey, u64)>>>,
    /// Every key the backend was asked to write, including faulted writes.
    attempted: Arc<Mutex<Vec<SaveKey>>>,
    /// One family whose next write fails with a transient I/O error.
    fail_once: Arc<Mutex<Option<SaveKey>>>,
}

impl Log {
    fn written_keys(&self) -> Vec<SaveKey> {
        self.written
            .lock()
            .unwrap()
            .iter()
            .map(|(key, _)| key.clone())
            .collect()
    }
}

struct Recording {
    inner: DiskStore,
    log: Log,
}

impl DiskBackend for Recording {
    fn write(&mut self, ticket: SaveTicket, request: SaveRequest) -> SaveCompletion {
        let keys: Vec<SaveKey> = request.snapshots.iter().map(|s| s.key.clone()).collect();
        self.log
            .attempted
            .lock()
            .unwrap()
            .extend(keys.iter().cloned());
        let mut fault = self.log.fail_once.lock().unwrap();
        if fault.as_ref().is_some_and(|family| keys.contains(family)) {
            *fault = None;
            let submitted = request
                .snapshots
                .iter()
                .map(|s| (s.key.clone(), s.revision))
                .collect();
            return SaveCompletion {
                ticket,
                snapshots: request.snapshots,
                submitted,
                committed: Vec::new(),
                error: Some(ServerError::Io {
                    operation: Operation::Replace,
                    kind: std::io::ErrorKind::Other,
                }),
            };
        }
        drop(fault);
        let completion = self.inner.write(ticket, request);
        self.log
            .written
            .lock()
            .unwrap()
            .extend(completion.committed.iter().cloned());
        completion
    }
    fn load(&mut self, key: SaveKey) -> Result<LoadedValue, ServerError> {
        self.inner.load(key)
    }
    fn sync(&mut self) -> Result<(), ServerError> {
        self.inner.sync()
    }
    fn close(&mut self) -> Result<(), ServerError> {
        self.inner.close()
    }
}

fn player() -> SaveKey {
    SaveKey::Player(id(1))
}

fn family_order() -> Vec<SaveKey> {
    vec![
        player(),
        SaveKey::Companions,
        SaveKey::Hostiles,
        SaveKey::Passives,
        SaveKey::Chunk(key(0)),
        SaveKey::Metadata,
    ]
}

fn pending(s: &mut AuthorityState) -> Vec<SaveKey> {
    s.freeze().save_keys
}

#[test]
fn production_family_flush_writes_each_family_snapshot_before_the_next() {
    let root = Root::new();
    let log = Log::default();
    let recorder = log.clone();
    let (mut s, store) = real_setup_on(&root, move |inner| Recording {
        inner,
        log: recorder,
    });
    s.begin_close();
    let shared = SharedScheduler::new(store);
    let clock = RealClock;
    let mut actors = shared.actors(&clock);
    let order = family_order();
    for (index, family) in order.iter().enumerate() {
        let before = log.written.lock().unwrap().len();
        let report = actors
            .flush_authority(&mut s, family.clone(), deadline())
            .unwrap();
        let written: Vec<(SaveKey, u64)> = log.written.lock().unwrap()[before..].to_vec();
        assert!(
            !written.is_empty(),
            "{family:?}: the store must receive the family's authority snapshot"
        );
        assert!(
            written.iter().all(|(key, _)| key == family),
            "{family:?} wrote {written:?}: only the requested family may reach the store"
        );
        assert!(report.durable >= 1, "{family:?}: {report:?}");
        let left = pending(&mut s);
        assert!(
            !left.contains(family),
            "{family:?} must be durable on return"
        );
        for later in &order[index + 1..] {
            if *later != SaveKey::Metadata {
                assert!(
                    left.contains(later),
                    "{later:?} must stay unflushed until its own phase"
                );
            }
        }
    }
    // The aggregate revisions are the authority's own next revisions.
    let written = log.written.lock().unwrap().clone();
    for (family, revision) in [
        (player(), 10),
        (SaveKey::Companions, 6),
        (SaveKey::Hostiles, 6),
        (SaveKey::Passives, 6),
    ] {
        assert!(written.contains(&(family.clone(), revision)), "{family:?}");
    }
    let mut store = shared.store();
    store.sync(deadline()).unwrap();
    store.close(deadline()).unwrap();
    reopen(&root, &s);
}

#[test]
fn production_authority_free_flush_refuses_instead_of_writing_nothing() {
    let root = Root::new();
    let log = Log::default();
    let recorder = log.clone();
    let (mut s, store) = real_setup_on(&root, move |inner| Recording {
        inner,
        log: recorder,
    });
    s.begin_close();
    let shared = SharedScheduler::new(store);
    let clock = RealClock;
    let mut actors = shared.actors(&clock);
    let refused = actors.flush(SaveKey::Companions, deadline()).unwrap_err();
    assert!(
        matches!(refused, ServerError::Internal { .. }),
        "{refused:?}"
    );
    assert!(log.attempted.lock().unwrap().is_empty());
    assert!(pending(&mut s).contains(&SaveKey::Companions));
}

struct FinalOnce;
impl FinalReducer for FinalOnce {
    fn reduce_final(&mut self, authority: &mut AuthorityState) -> Result<u64, ServerError> {
        Ok(authority.next_tick())
    }
}

struct IdleLifecycle;
impl WorkerLifecycle for IdleLifecycle {
    fn stop_new(&mut self) -> Result<(), ServerError> {
        Ok(())
    }
    fn cancel(&mut self) -> Result<(), ServerError> {
        Ok(())
    }
    fn wait(&mut self, _: Deadline) -> Result<(), ServerError> {
        Ok(())
    }
    fn close(&mut self, _: Deadline) -> Result<(), ServerError> {
        Ok(())
    }
}
impl MemoryFinalizer for IdleLifecycle {
    fn pending(&self) -> MemoryFinalizationReport {
        MemoryFinalizationReport::default()
    }
    fn begin_attempt(&mut self, _: Deadline) -> Result<(), ServerError> {
        Ok(())
    }
    fn drain(&mut self, _: Deadline) -> Result<MemoryFinalizationReport, ServerError> {
        Ok(MemoryFinalizationReport::default())
    }
}
impl AgentHandle for IdleLifecycle {
    fn submit(&mut self, _: AgentRequest) -> Result<AgentRequestId, ServerError> {
        Err(ServerError::Internal {
            invariant: "unused agent submit",
        })
    }
    fn poll(&mut self, _: AgentRequestId) -> AgentPoll {
        AgentPoll::Pending
    }
    fn cancel(&mut self, _: AgentRequestId, _: Deadline) -> Result<(), ServerError> {
        Ok(())
    }
    fn freeze(&mut self, _: &dyn Clock) -> Option<FrozenLease> {
        None
    }
    fn release(&mut self, _: &FrozenLease, _: Deadline) -> Result<(), ServerError> {
        Ok(())
    }
    fn close(&mut self, _: Deadline) -> Result<(), ServerError> {
        Ok(())
    }
}
impl McpLifecycle for IdleLifecycle {
    fn close(&mut self, _: Deadline) -> Result<(), ServerError> {
        Ok(())
    }
}

#[test]
fn production_shutdown_flushes_families_in_go_order_and_stops_at_a_failed_family() {
    let root = Root::new();
    let log = Log::default();
    *log.fail_once.lock().unwrap() = Some(SaveKey::Hostiles);
    let recorder = log.clone();
    let (mut s, store) = real_setup_on(&root, move |inner| Recording {
        inner,
        log: recorder,
    });
    let shared = SharedScheduler::new(store);
    let clock = RealClock;
    let mut actors = shared.actors(&clock);
    let mut store = shared.store();
    let (mut reducer, mut workers, mut memory, mut agent, mut mcp) = (
        FinalOnce,
        IdleLifecycle,
        IdleLifecycle,
        IdleLifecycle,
        IdleLifecycle,
    );
    let until = Deadline::after(Instant::now(), std::time::Duration::from_secs(20)).unwrap();
    let mut ports = ShutdownPorts {
        reducer: &mut reducer,
        workers: &mut workers,
        actors: &mut actors,
        memory: &mut memory,
        store: &mut store,
        agent: &mut agent,
        mcp: &mut mcp,
        clock: &clock,
    };
    let failure = shutdown(&mut s, &mut ports, until).unwrap_err();
    assert_eq!(failure.report.next, ShutdownPhase::FlushHostiles);
    assert!(failure.report.retryable, "{:?}", failure.error);
    assert_eq!(
        log.written_keys(),
        vec![player(), SaveKey::Companions],
        "a failed family stops the flush before any later family reaches the store"
    );
    assert_eq!(
        log.attempted.lock().unwrap().last(),
        Some(&SaveKey::Hostiles)
    );

    let report = shutdown(&mut s, &mut ports, until).unwrap();
    assert_eq!(report.next, ShutdownPhase::Closed);
    assert_eq!(
        log.written_keys(),
        family_order(),
        "the retry resumes at the failed family and keeps the fixed family order"
    );
    reopen(&root, &s);
}

#[test]
fn production_family_flush_splits_a_mixed_retry_cohort_by_family() {
    // An autosave writes every dirty actor family in one ticket. When that
    // write fails, its originals wait as one retry cohort; the shutdown flush
    // must still write them one family at a time.
    let root = Root::new();
    let log = Log::default();
    *log.fail_once.lock().unwrap() = Some(player());
    let recorder = log.clone();
    let (mut s, mut scheduler) = real_setup_on(&root, move |inner| Recording {
        inner,
        log: recorder,
    });
    let until = deadline();
    let mut tick = 0;
    // Settle every write the autosave already dispatched, so the flush below
    // is the only source of new writes.
    while scheduler.pending_retry_jobs() == 0 || scheduler.tracked_submits() != 0 {
        scheduler.drive_workers();
        scheduler
            .poll_tick(
                tick,
                mornlea_server::contracts::SaveBudget::default(),
                &mut s,
            )
            .unwrap();
        assert!(
            !until.expired(Instant::now()),
            "the faulted autosave never settled"
        );
        // Once the cohort waits, the tick stays put so its backoff never
        // comes due here; only already-dispatched writes keep settling.
        if scheduler.pending_retry_jobs() == 0 {
            tick += 1;
        }
        std::thread::yield_now();
    }
    let attempted = log.attempted.lock().unwrap().clone();
    for family in [
        player(),
        SaveKey::Companions,
        SaveKey::Hostiles,
        SaveKey::Passives,
    ] {
        assert!(
            attempted.contains(&family),
            "one mixed actor ticket: {attempted:?}"
        );
    }
    assert!(
        log.written_keys()
            .iter()
            .all(|key| matches!(key, SaveKey::Chunk(_) | SaveKey::Metadata))
    );
    let before = log.written.lock().unwrap().len();
    s.begin_close();
    let shared = SharedScheduler::new(scheduler);
    let clock = RealClock;
    let mut actors = shared.actors(&clock);
    for family in [
        player(),
        SaveKey::Companions,
        SaveKey::Hostiles,
        SaveKey::Passives,
    ] {
        let start = log.written.lock().unwrap().len();
        actors
            .flush_authority(&mut s, family.clone(), deadline())
            .unwrap();
        let written: Vec<SaveKey> = log.written.lock().unwrap()[start..]
            .iter()
            .map(|(key, _)| key.clone())
            .collect();
        assert_eq!(written, vec![family.clone()], "{family:?}");
    }
    assert!(log.written.lock().unwrap().len() >= before + 4);
}
