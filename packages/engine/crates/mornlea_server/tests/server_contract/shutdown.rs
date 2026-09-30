//! Retryable shutdown: one final unpublished tick and ordered closure.
//!
//! These cases drive the shutdown provider's resumable phase machine with
//! scriptable port doubles. The pins are the phase-boundary ownership rules:
//! the final tick consumes accepted work exactly once and publishes nothing,
//! a timed-out attempt resumes at the failed phase without a second final
//! tick, a failed store sync keeps the frozen lease with the Agent and MCP
//! services open, release and close retries never repeat a succeeded step,
//! an expired frozen lease skips the release instead of inventing a receipt,
//! and a completed shutdown is idempotent with zero further port calls. No
//! double here is production behavior; the real reducer and store join this
//! machine only at integration time.

use std::cell::Cell;
use std::time::{Duration, Instant};

use mornlea_domain::{Command, PlayerId};
use mornlea_protocol::{LoginStart, PlayIntent, admit_login};
use mornlea_server::contracts::{
    ActorPersistence, AgentErrorCode, AgentHandle, AgentPoll, AgentRequest, AgentRequestId,
    ClientInstanceId, Clock, Deadline, FinalReducer, FlushReport, FrozenLease, LeaseId,
    McpLifecycle, MemoryFinalizationReport, MemoryFinalizer, NamespaceId, Operation, OwnedSnapshot,
    SaveAuthority, SaveBudget, SaveKey, SavePoll, SaveRequest, SaveScheduleReport, SaveTicket,
    ServerError, ServerLimits, ServerPhase, SessionKey, ShutdownPhase, StoreHandle,
    SubmitSaveError, TransportKind, WorkerLifecycle,
};
use mornlea_server::core::session;
use mornlea_server::core::shutdown as provider;
use mornlea_server::state::AuthorityState;

fn authority() -> AuthorityState {
    AuthorityState::try_new(
        ServerLimits::try_new(8, 4096, 512, 64, 64, 1_048_576).unwrap(),
        7,
    )
    .unwrap()
}

fn uuid(tag: u8) -> [u8; 16] {
    let mut bytes = [0u8; 16];
    bytes[0] = tag;
    bytes[6] = 0x40;
    bytes[8] = 0x80;
    bytes
}

fn login(tag: u8, name: &str) -> mornlea_protocol::AdmittedLogin {
    let id = PlayerId::try_from_bytes(uuid(tag)).unwrap();
    let start = LoginStart::new(id, name, 8).unwrap();
    let inbound = LoginStart::decode_inbound(&start.encode().unwrap()).unwrap();
    admit_login(inbound).unwrap()
}

fn sequenced(sequence: u64) -> PlayIntent {
    PlayIntent::Sequenced {
        sequence,
        command: Command::CloseContainer,
    }
}

/// Doubles answer ports the shutdown path never exercises with a typed
/// internal error rather than a silent default.
fn unused() -> ServerError {
    ServerError::Internal {
        invariant: "shutdown double: unused port",
    }
}

/// Clock double the harness can advance mid-attempt: lease expiry is a
/// monotonic boundary, so a test crosses it by stepping time forward without
/// rebuilding the harness or its doubles.
struct StepClock {
    now: Cell<Instant>,
}

impl StepClock {
    fn new(now: Instant) -> Self {
        Self {
            now: Cell::new(now),
        }
    }

    fn set(&self, instant: Instant) {
        self.now.set(instant);
    }
}

impl Clock for StepClock {
    fn monotonic(&self) -> Instant {
        self.now.get()
    }

    fn unix_ms(&self) -> i64 {
        0
    }
}

/// Final-tick double: drains every eligible accepted command through the
/// sequence watermark in one reduction and publishes nothing.
struct CountingReducer {
    calls: usize,
    session: Option<SessionKey>,
}

impl FinalReducer for CountingReducer {
    fn reduce_final(&mut self, state: &mut AuthorityState) -> Result<u64, ServerError> {
        self.calls += 1;
        let tick = state.next_tick();
        let batch = state.freeze_eligible(tick);
        for envelope in &batch {
            if let Some(key) = self.session {
                let _ = state.apply_sequence(key, envelope.sequence());
            }
        }
        Ok(tick)
    }
}

struct WorkerDouble {
    stop_new: usize,
    cancel: usize,
    wait: usize,
    close_calls: usize,
    fail_wait_once: Option<ServerError>,
    fail_close_once: Option<ServerError>,
}

impl WorkerLifecycle for WorkerDouble {
    fn stop_new(&mut self) -> Result<(), ServerError> {
        self.stop_new += 1;
        Ok(())
    }

    fn cancel(&mut self) -> Result<(), ServerError> {
        self.cancel += 1;
        Ok(())
    }

    fn wait(&mut self, _deadline: Deadline) -> Result<(), ServerError> {
        self.wait += 1;
        if let Some(error) = self.fail_wait_once.take() {
            return Err(error);
        }
        Ok(())
    }

    fn close(&mut self, _deadline: Deadline) -> Result<(), ServerError> {
        self.close_calls += 1;
        if let Some(error) = self.fail_close_once.take() {
            return Err(error);
        }
        Ok(())
    }
}

struct MemoryDouble {
    attempts: usize,
    drains: usize,
}

impl MemoryFinalizer for MemoryDouble {
    fn pending(&self) -> MemoryFinalizationReport {
        MemoryFinalizationReport::default()
    }
    fn begin_attempt(&mut self, _deadline: Deadline) -> Result<(), ServerError> {
        self.attempts += 1;
        Ok(())
    }

    fn drain(&mut self, _deadline: Deadline) -> Result<MemoryFinalizationReport, ServerError> {
        self.drains += 1;
        Ok(MemoryFinalizationReport {
            completed: 1,
            outstanding: 0,
        })
    }
}

struct ActorDouble {
    flushes: Vec<&'static str>,
    outstanding: usize,
}

fn family_name(key: &SaveKey) -> &'static str {
    match key {
        SaveKey::Player(_) => "player",
        SaveKey::Companions => "companions",
        SaveKey::Hostiles => "hostiles",
        SaveKey::Passives => "passives",
        SaveKey::Metadata => "metadata",
        SaveKey::Chunk(_) => "chunk",
    }
}

impl ActorPersistence for ActorDouble {
    fn flush(&mut self, family: SaveKey, _deadline: Deadline) -> Result<FlushReport, ServerError> {
        self.flushes.push(family_name(&family));
        Ok(FlushReport {
            durable: 1,
            failed: 0,
            outstanding: self.outstanding,
        })
    }
}

struct StoreDouble {
    sync_calls: usize,
    close_calls: usize,
    fail_sync_once: Option<ServerError>,
    fail_close_once: Option<ServerError>,
}

impl StoreHandle for StoreDouble {
    fn submit(&mut self, request: SaveRequest) -> Result<SaveTicket, SubmitSaveError> {
        Err(SubmitSaveError {
            error: unused(),
            request,
        })
    }

    fn poll(&mut self, _ticket: SaveTicket) -> SavePoll {
        SavePoll::Pending
    }

    fn poll_tick(
        &mut self,
        _tick: u64,
        _budget: SaveBudget,
        _authority: &mut dyn SaveAuthority,
    ) -> Result<SaveScheduleReport, ServerError> {
        Err(unused())
    }

    fn cancel_pending(&mut self) -> Result<Vec<OwnedSnapshot>, ServerError> {
        Err(unused())
    }

    fn flush(
        &mut self,
        _deadline: Deadline,
        _authority: &mut dyn SaveAuthority,
        _clock: &dyn Clock,
    ) -> Result<FlushReport, ServerError> {
        Ok(FlushReport::default())
    }

    fn sync(&mut self, _deadline: Deadline) -> Result<(), ServerError> {
        self.sync_calls += 1;
        if let Some(error) = self.fail_sync_once.take() {
            return Err(error);
        }
        Ok(())
    }

    fn close(&mut self, _deadline: Deadline) -> Result<(), ServerError> {
        self.close_calls += 1;
        if let Some(error) = self.fail_close_once.take() {
            return Err(error);
        }
        Ok(())
    }
}

struct AgentDouble {
    lease: FrozenLease,
    freeze_calls: usize,
    release_calls: usize,
    release_ok: usize,
    release_failures: Vec<ServerError>,
    fail_close_once: Option<ServerError>,
    close_calls: usize,
    released: Vec<FrozenLease>,
}

impl AgentHandle for AgentDouble {
    fn submit(&mut self, _request: AgentRequest) -> Result<AgentRequestId, ServerError> {
        Err(unused())
    }

    fn poll(&mut self, _id: AgentRequestId) -> AgentPoll {
        AgentPoll::Pending
    }

    fn cancel(&mut self, _id: AgentRequestId, _deadline: Deadline) -> Result<(), ServerError> {
        Err(unused())
    }

    fn freeze(&mut self, clock: &dyn Clock) -> Option<FrozenLease> {
        // A frozen handle keeps returning the same retained lease until a
        // successful release retires it or the lease expires, which is what
        // makes a release retry reuse the lease identity the freeze captured
        // while an expired lease is never released again.
        self.freeze_calls += 1;
        if clock.monotonic() >= self.lease.expires_at {
            return None;
        }
        Some(self.lease.clone())
    }

    fn release(&mut self, lease: &FrozenLease, _deadline: Deadline) -> Result<(), ServerError> {
        self.release_calls += 1;
        self.released.push(lease.clone());
        // Failures are consumed in order so one harness can script a lost
        // response followed by a differently-typed retry answer.
        if !self.release_failures.is_empty() {
            return Err(self.release_failures.remove(0));
        }
        self.release_ok += 1;
        Ok(())
    }

    fn close(&mut self, _deadline: Deadline) -> Result<(), ServerError> {
        self.close_calls += 1;
        if let Some(error) = self.fail_close_once.take() {
            return Err(error);
        }
        Ok(())
    }
}

struct McpDouble {
    close_calls: usize,
    fail_close_once: Option<ServerError>,
}

impl McpLifecycle for McpDouble {
    fn close(&mut self, _deadline: Deadline) -> Result<(), ServerError> {
        self.close_calls += 1;
        if let Some(error) = self.fail_close_once.take() {
            return Err(error);
        }
        Ok(())
    }
}

/// One shutdown harness: the same doubles survive across attempts so call
/// counters observe retry behavior, and fail-once switches spend themselves.
struct Harness {
    reducer: CountingReducer,
    workers: WorkerDouble,
    actors: ActorDouble,
    memory: MemoryDouble,
    store: StoreDouble,
    agent: AgentDouble,
    mcp: McpDouble,
    clock: StepClock,
    deadline: Deadline,
}

impl Harness {
    fn new(now: Instant) -> Self {
        Self {
            reducer: CountingReducer {
                calls: 0,
                session: None,
            },
            workers: WorkerDouble {
                stop_new: 0,
                cancel: 0,
                wait: 0,
                close_calls: 0,
                fail_wait_once: None,
                fail_close_once: None,
            },
            actors: ActorDouble {
                flushes: Vec::new(),
                outstanding: 0,
            },
            memory: MemoryDouble {
                attempts: 0,
                drains: 0,
            },
            store: StoreDouble {
                sync_calls: 0,
                close_calls: 0,
                fail_sync_once: None,
                fail_close_once: None,
            },
            agent: AgentDouble {
                lease: FrozenLease {
                    client: ClientInstanceId::try_from_bytes(uuid(9)).unwrap(),
                    namespace: NamespaceId::try_from_bytes(uuid(10)).unwrap(),
                    lease: LeaseId::try_from_bytes(uuid(11)).unwrap(),
                    lease_fence: 7,
                    expires_at: now + Duration::from_secs(60),
                },
                freeze_calls: 0,
                release_calls: 0,
                release_ok: 0,
                release_failures: Vec::new(),
                fail_close_once: None,
                close_calls: 0,
                released: Vec::new(),
            },
            mcp: McpDouble {
                close_calls: 0,
                fail_close_once: None,
            },
            clock: StepClock::new(now),
            deadline: Deadline::after(now, Duration::from_secs(30)).unwrap(),
        }
    }

    fn ports(&mut self) -> provider::ShutdownPorts<'_> {
        provider::ShutdownPorts {
            reducer: &mut self.reducer,
            workers: &mut self.workers,
            actors: &mut self.actors,
            memory: &mut self.memory,
            store: &mut self.store,
            agent: &mut self.agent,
            mcp: &mut self.mcp,
            clock: &self.clock,
        }
    }
}

#[test]
fn buffered_command_once_no_publication() {
    let mut state = authority();
    let key = session::admit(&mut state, login(1, "Ada"), TransportKind::Memory).unwrap();
    session::submit(&mut state, key, sequenced(1)).unwrap();
    let mut harness = Harness::new(Instant::now());
    harness.reducer.session = Some(key);
    let deadline = harness.deadline;
    let report = provider::shutdown(&mut state, &mut harness.ports(), deadline).unwrap();

    assert_eq!(report.final_tick, Some(0));
    assert_eq!(report.next, ShutdownPhase::Closed);
    assert!(!report.retryable);
    assert_eq!(state.phase(), ServerPhase::Closed);
    assert_eq!(harness.reducer.calls, 1);
    assert_eq!(
        state.session(key).unwrap().last_applied_sequence,
        1,
        "the final tick consumed the buffered command exactly once"
    );
    assert!(
        state.take_outbox(key, 64, 4096).unwrap().is_empty(),
        "the final tick is unpublished: no outbox frame is appended"
    );
    assert_eq!(
        harness.actors.flushes,
        ["companions", "hostiles", "passives", "metadata"],
        "aggregate families flush in the fixed order after the final tick"
    );
    assert_eq!(report.durable, 4);
}

#[test]
fn timeout_resume_without_second_tick() {
    let mut state = authority();
    let key = session::admit(&mut state, login(2, "Bea"), TransportKind::Memory).unwrap();
    session::submit(&mut state, key, sequenced(4)).unwrap();
    let mut harness = Harness::new(Instant::now());
    harness.reducer.session = Some(key);
    harness.workers.fail_wait_once = Some(ServerError::Timeout {
        operation: Operation::Shutdown,
    });
    let deadline = harness.deadline;
    let failure = provider::shutdown(&mut state, &mut harness.ports(), deadline).unwrap_err();

    assert_eq!(failure.report.next, ShutdownPhase::WaitWorkers);
    assert!(failure.report.retryable);
    assert!(matches!(failure.error, ServerError::Timeout { .. }));
    assert!(failure.report.completed.contains(&ShutdownPhase::FinalTick));
    assert_eq!(failure.report.final_tick, Some(0));
    assert_eq!(state.phase(), ServerPhase::Closing);
    assert_eq!(harness.reducer.calls, 1);

    let resumed = provider::shutdown(&mut state, &mut harness.ports(), deadline).unwrap();
    assert_eq!(resumed.next, ShutdownPhase::Closed);
    assert_eq!(state.phase(), ServerPhase::Closed);
    assert_eq!(
        harness.reducer.calls, 1,
        "a resumed attempt never executes a second final tick"
    );
    assert_eq!(resumed.final_tick, Some(0));
    assert_eq!(
        harness.workers.wait, 2,
        "the retry re-runs the failed phase itself, not the whole machine"
    );
}

#[test]
fn failed_sync_retains_lease() {
    let mut state = authority();
    let key = session::admit(&mut state, login(3, "Cara"), TransportKind::Memory).unwrap();
    session::submit(&mut state, key, sequenced(2)).unwrap();
    let mut harness = Harness::new(Instant::now());
    harness.reducer.session = Some(key);
    harness.actors.outstanding = 2;
    harness.store.fail_sync_once = Some(ServerError::Io {
        operation: Operation::Sync,
        kind: std::io::ErrorKind::TimedOut,
    });
    let deadline = harness.deadline;
    let failure = provider::shutdown(&mut state, &mut harness.ports(), deadline).unwrap_err();

    assert_eq!(failure.report.next, ShutdownPhase::StoreSync);
    assert!(failure.report.retryable);
    assert_eq!(
        failure.report.outstanding, 2,
        "the failure reports the flush work that has not reached durability"
    );
    assert!(!failure.report.completed.contains(&ShutdownPhase::StoreSync));
    assert_eq!(harness.agent.freeze_calls, 1);
    assert_eq!(harness.agent.release_calls, 0);
    assert_eq!(harness.agent.close_calls, 0);
    assert_eq!(harness.mcp.close_calls, 0);
    assert_eq!(harness.store.close_calls, 0);
    assert!(
        harness.agent.lease.expires_at > harness.clock.monotonic(),
        "the retained lease is still valid at the failure boundary"
    );

    let resumed = provider::shutdown(&mut state, &mut harness.ports(), deadline).unwrap();
    assert_eq!(resumed.next, ShutdownPhase::Closed);
    assert_eq!(resumed.outstanding, 0);
    assert_eq!(harness.agent.freeze_calls, 2);
    assert_eq!(harness.agent.release_calls, 1);
    assert_eq!(harness.agent.released.len(), 1);
    assert_eq!(
        harness.agent.released[0], harness.agent.lease,
        "the retry releases the same lease the freeze retained"
    );
    assert_eq!(
        harness.store.sync_calls, 2,
        "the failed sync re-runs on the retry, unlike a completed sync"
    );
}

#[test]
fn release_and_close_retry_skip_sync() {
    let mut state = authority();
    let key = session::admit(&mut state, login(4, "Dora"), TransportKind::Memory).unwrap();
    session::submit(&mut state, key, sequenced(3)).unwrap();
    let mut harness = Harness::new(Instant::now());
    harness.reducer.session = Some(key);
    harness.agent.release_failures = vec![ServerError::Agent {
        code: AgentErrorCode::AgentUnavailable,
        status: 503,
    }];
    let deadline = harness.deadline;

    // First attempt: the store sync succeeds, the lease release fails once.
    let release_failure =
        provider::shutdown(&mut state, &mut harness.ports(), deadline).unwrap_err();
    assert_eq!(release_failure.report.next, ShutdownPhase::ReleaseAgent);
    assert!(
        release_failure
            .report
            .completed
            .contains(&ShutdownPhase::StoreSync)
    );
    assert_eq!(harness.store.sync_calls, 1);
    assert_eq!(harness.agent.release_calls, 1);
    assert_eq!(harness.agent.release_ok, 0);
    assert_eq!(harness.agent.close_calls, 0);
    assert_eq!(harness.mcp.close_calls, 0);
    assert_eq!(harness.store.close_calls, 0);

    // Second attempt: the release succeeds, the store close fails once.
    harness.store.fail_close_once = Some(ServerError::Io {
        operation: Operation::Close,
        kind: std::io::ErrorKind::Other,
    });
    let close_failure = provider::shutdown(&mut state, &mut harness.ports(), deadline).unwrap_err();
    assert_eq!(close_failure.report.next, ShutdownPhase::StoreClose);
    assert_eq!(harness.agent.release_calls, 2);
    assert_eq!(harness.agent.release_ok, 1);
    assert_eq!(harness.agent.close_calls, 1);
    assert_eq!(
        harness.mcp.close_calls, 1,
        "MCP closes only after the release succeeded"
    );
    assert_eq!(
        harness.store.sync_calls, 1,
        "a completed sync is never replayed"
    );
    assert_eq!(harness.store.close_calls, 1);
    assert!(
        harness
            .agent
            .released
            .iter()
            .all(|lease| *lease == harness.agent.lease),
        "every release attempt carries the same frozen lease"
    );

    // Third attempt: the store close succeeds and the server reaches Closed.
    let done = provider::shutdown(&mut state, &mut harness.ports(), deadline).unwrap();
    assert_eq!(done.next, ShutdownPhase::Closed);
    assert_eq!(state.phase(), ServerPhase::Closed);
    assert_eq!(done.final_tick, Some(0));
    assert_eq!(harness.reducer.calls, 1);
    assert_eq!(harness.store.sync_calls, 1);
    assert_eq!(harness.agent.release_calls, 2);
    assert_eq!(
        harness.agent.release_ok, 1,
        "the close retry does not release again"
    );
    assert_eq!(harness.mcp.close_calls, 1);
    assert_eq!(harness.store.close_calls, 2);
    assert_eq!(harness.workers.close_calls, 1);
    assert!(
        state.take_outbox(key, 64, 4096).unwrap().is_empty(),
        "the final buffered command published nothing across all attempts"
    );
}

#[test]
fn closed_idempotent() {
    let mut state = authority();
    let key = session::admit(&mut state, login(5, "Eve"), TransportKind::Memory).unwrap();
    session::submit(&mut state, key, sequenced(6)).unwrap();
    let mut harness = Harness::new(Instant::now());
    harness.reducer.session = Some(key);
    let deadline = harness.deadline;
    let first = provider::shutdown(&mut state, &mut harness.ports(), deadline).unwrap();
    assert_eq!(first.next, ShutdownPhase::Closed);

    let counters = (
        harness.reducer.calls,
        harness.workers.stop_new,
        harness.workers.close_calls,
        harness.actors.flushes.len(),
        harness.memory.attempts,
        harness.store.sync_calls,
        harness.store.close_calls,
        harness.agent.freeze_calls,
        harness.agent.release_calls,
        harness.agent.close_calls,
        harness.mcp.close_calls,
    );

    let second = provider::shutdown(&mut state, &mut harness.ports(), deadline).unwrap();
    assert_eq!(second, first);
    assert_eq!(second.final_tick, first.final_tick);
    let after = (
        harness.reducer.calls,
        harness.workers.stop_new,
        harness.workers.close_calls,
        harness.actors.flushes.len(),
        harness.memory.attempts,
        harness.store.sync_calls,
        harness.store.close_calls,
        harness.agent.freeze_calls,
        harness.agent.release_calls,
        harness.agent.close_calls,
        harness.mcp.close_calls,
    );
    assert_eq!(after, counters, "a completed shutdown replays no port");
}

#[test]
fn release_lost_response_expiry() {
    let mut state = authority();
    let key = session::admit(&mut state, login(6, "Faye"), TransportKind::Memory).unwrap();
    session::submit(&mut state, key, sequenced(7)).unwrap();
    let mut harness = Harness::new(Instant::now());
    harness.reducer.session = Some(key);
    // The lease expires well inside the attempt budget, so the expiry fires
    // while the deadline is still far away.
    let now = harness.clock.monotonic();
    harness.agent.lease.expires_at = now + Duration::from_secs(15);
    harness.agent.release_failures = vec![
        ServerError::Io {
            operation: Operation::ReleaseAgent,
            kind: std::io::ErrorKind::TimedOut,
        },
        ServerError::Agent {
            code: AgentErrorCode::NotFound,
            status: 404,
        },
    ];
    let deadline = harness.deadline;

    // The release response was lost locally: the server cannot know whether
    // the remote honored it, so the lease stays retained and every resource
    // behind it stays open.
    let lost = provider::shutdown(&mut state, &mut harness.ports(), deadline).unwrap_err();
    assert_eq!(lost.report.next, ShutdownPhase::ReleaseAgent);
    assert!(lost.report.retryable);
    assert_eq!(harness.agent.release_calls, 1);
    assert_eq!(harness.agent.release_ok, 0);
    assert_eq!(harness.agent.close_calls, 0);
    assert_eq!(harness.mcp.close_calls, 0);
    assert_eq!(harness.store.close_calls, 0);

    // A not_found answer to the retry is no release receipt: without proof
    // the lease is gone the failure stays and nothing closes.
    let not_found = provider::shutdown(&mut state, &mut harness.ports(), deadline).unwrap_err();
    assert_eq!(not_found.report.next, ShutdownPhase::ReleaseAgent);
    assert!(not_found.report.retryable);
    assert!(matches!(
        not_found.error,
        ServerError::Agent {
            code: AgentErrorCode::NotFound,
            ..
        }
    ));
    assert!(
        !not_found
            .report
            .completed
            .contains(&ShutdownPhase::ReleaseAgent),
        "no invented release receipt"
    );
    assert_eq!(harness.agent.release_calls, 2);
    assert_eq!(harness.agent.release_ok, 0);
    assert_eq!(harness.agent.close_calls, 0);
    assert_eq!(harness.mcp.close_calls, 0);
    assert_eq!(harness.store.close_calls, 0);

    // Past the local lease expiry the release is skipped entirely and the
    // close chain runs instead, so the authority still reaches Closed.
    harness.clock.set(now + Duration::from_secs(16));
    let done = provider::shutdown(&mut state, &mut harness.ports(), deadline).unwrap();
    assert_eq!(done.next, ShutdownPhase::Closed);
    assert_eq!(state.phase(), ServerPhase::Closed);
    assert_eq!(
        harness.agent.release_calls, 2,
        "an expired lease is never released again"
    );
    assert_eq!(harness.agent.release_ok, 0, "no release ever succeeded");
    assert_eq!(harness.agent.close_calls, 1);
    assert_eq!(harness.mcp.close_calls, 1);
    assert_eq!(harness.store.close_calls, 1);
    assert_eq!(harness.workers.close_calls, 1);
}

#[test]
fn agent_close_failure_no_release_repeat() {
    let mut state = authority();
    let key = session::admit(&mut state, login(7, "Gus"), TransportKind::Memory).unwrap();
    session::submit(&mut state, key, sequenced(8)).unwrap();
    let mut harness = Harness::new(Instant::now());
    harness.reducer.session = Some(key);
    harness.agent.fail_close_once = Some(ServerError::Io {
        operation: Operation::Close,
        kind: std::io::ErrorKind::ConnectionAborted,
    });
    let deadline = harness.deadline;

    // The release succeeded, so the failure boundary is the Agent close:
    // the lease is gone and the MCP and store services are still open.
    let failure = provider::shutdown(&mut state, &mut harness.ports(), deadline).unwrap_err();
    assert_eq!(failure.report.next, ShutdownPhase::AgentClose);
    assert!(failure.report.retryable);
    assert!(
        failure
            .report
            .completed
            .contains(&ShutdownPhase::ReleaseAgent)
    );
    assert_eq!(harness.agent.release_calls, 1);
    assert_eq!(harness.agent.release_ok, 1);
    assert_eq!(harness.agent.close_calls, 1);
    assert_eq!(harness.mcp.close_calls, 0);
    assert_eq!(harness.store.close_calls, 0);

    // The retry re-runs the Agent close without any second release, and the
    // MCP and store closes follow only once the Agent close succeeds.
    let done = provider::shutdown(&mut state, &mut harness.ports(), deadline).unwrap();
    assert_eq!(done.next, ShutdownPhase::Closed);
    assert_eq!(state.phase(), ServerPhase::Closed);
    assert_eq!(
        harness.agent.release_calls, 1,
        "a completed release is never repeated by a close retry"
    );
    assert_eq!(
        harness.agent.close_calls, 2,
        "the failed Agent close re-runs on the retry"
    );
    assert_eq!(harness.mcp.close_calls, 1);
    assert_eq!(harness.store.close_calls, 1);
    assert_eq!(harness.workers.close_calls, 1);
}

#[test]
fn mcp_close_failure_no_agent_repeat() {
    let mut state = authority();
    let key = session::admit(&mut state, login(8, "Hana"), TransportKind::Memory).unwrap();
    session::submit(&mut state, key, sequenced(9)).unwrap();
    let mut harness = Harness::new(Instant::now());
    harness.reducer.session = Some(key);
    harness.mcp.fail_close_once = Some(ServerError::Io {
        operation: Operation::Close,
        kind: std::io::ErrorKind::ConnectionReset,
    });
    let deadline = harness.deadline;

    // Release and Agent close succeeded, so the failure boundary is the MCP
    // service close and the store stays open behind it.
    let failure = provider::shutdown(&mut state, &mut harness.ports(), deadline).unwrap_err();
    assert_eq!(failure.report.next, ShutdownPhase::McpClose);
    assert!(failure.report.retryable);
    assert!(
        failure
            .report
            .completed
            .contains(&ShutdownPhase::AgentClose)
    );
    assert_eq!(harness.agent.release_ok, 1);
    assert_eq!(harness.agent.close_calls, 1);
    assert_eq!(harness.mcp.close_calls, 1);
    assert_eq!(harness.store.close_calls, 0);

    // The retry re-runs only the MCP close; the Agent close that already
    // succeeded is never repeated, and the store close follows.
    let done = provider::shutdown(&mut state, &mut harness.ports(), deadline).unwrap();
    assert_eq!(done.next, ShutdownPhase::Closed);
    assert_eq!(state.phase(), ServerPhase::Closed);
    assert_eq!(harness.agent.release_calls, 1);
    assert_eq!(
        harness.agent.close_calls, 1,
        "a completed Agent close is never repeated by an MCP close retry"
    );
    assert_eq!(
        harness.mcp.close_calls, 2,
        "the failed MCP close re-runs on the retry"
    );
    assert_eq!(harness.store.close_calls, 1);
    assert_eq!(harness.workers.close_calls, 1);
}
