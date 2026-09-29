//! Companion task host: gate fencing, world revalidation, and capacity.
//!
//! Expected values mirror the Go authority in
//! `packages/server/server/companion_manager.go` (`applyPlannerOutcome` drops
//! unmatched outcomes without touching the active gate and
//! `plannerOutcomeMatchesCurrentAuthority` rebuilds the current world before
//! accepting a plan) and the bounded companion inbox (`companion.MaxActive`).
//! One case runs against the real lease and HTTP providers with a
//! deterministic loopback server; the rest use a scripted `AgentHandle`
//! double with the real snapshot registry.

use std::collections::{BTreeMap, VecDeque};
use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};
use std::time::{Duration, Instant};

use mornlea_domain::{
    BlockPos, ChunkPos, CommandText, CompanionId, FiniteVec3, LookAngles, PlayerId,
};
use mornlea_server::agent::host::{
    CurrentWorld, DialogueAdmit, DialogueDispatch, InstallReject, MAX_PERSONA_BYTES, PlanDispatch,
    PlanFailKind, PlanHost,
};
use mornlea_server::agent::http::AgentHttpWire;
use mornlea_server::agent::lease::{LeaseConfig, LeaseController};
use mornlea_server::agent::snapshot::{SnapshotEntropy, SnapshotRegistry};
use mornlea_server::contracts::{
    AgentErrorCode, AgentHandle, AgentPlan, AgentPoll, AgentRequest, AgentRequestId, AgentResponse,
    CancelResponse, ClientInstanceId, Clock, Deadline, DialogueEnvironment, DialogueFact, LeaseId,
    NamespaceId, PlanRequest, PlanResponse, PlanStep, Resource, RunId, ServerError, SnapshotId,
    SnapshotPort,
};
use mornlea_server::core::companion_ingress::{CompanionIngress, CompanionTaskGate};
use mornlea_storage::ItemStack;

/// Step clock the harness holds fixed; the business RPC uses real loopback
/// I/O, so the clock only fences lease and snapshot lifetimes.
struct StepClock {
    now: Mutex<Instant>,
}

impl StepClock {
    fn start() -> (Instant, Arc<Self>) {
        let now = Instant::now();
        (
            now,
            Arc::new(Self {
                now: Mutex::new(now),
            }),
        )
    }
}

impl Clock for StepClock {
    fn monotonic(&self) -> Instant {
        *self.now.lock().unwrap()
    }

    fn unix_ms(&self) -> i64 {
        1_800_000_000_000
    }
}

/// Deterministic entropy cycling bytes `0, 1, ...`, mirroring the Go
/// `snapshotTestEntropy` cycle.
struct CycleEntropy {
    next: Mutex<u8>,
}

impl SnapshotEntropy for CycleEntropy {
    fn fill(&self, out: &mut [u8; 32]) -> Result<(), ServerError> {
        let mut next = self.next.lock().unwrap();
        for slot in out.iter_mut() {
            *slot = *next;
            *next = next.wrapping_add(1);
        }
        Ok(())
    }
}

fn uuid(tag: u8) -> [u8; 16] {
    let mut bytes = [0u8; 16];
    bytes[0] = tag;
    bytes[6] = 0x40;
    bytes[8] = 0x80;
    bytes
}

fn companion() -> CompanionId {
    CompanionId::try_from_bytes(uuid(20)).unwrap()
}

fn companion_at(tag: u8) -> CompanionId {
    let mut bytes = uuid(20);
    bytes[1] = tag;
    CompanionId::try_from_bytes(bytes).unwrap()
}

fn player() -> PlayerId {
    PlayerId::try_from_bytes(uuid(10)).unwrap()
}

fn lease() -> LeaseId {
    LeaseId::try_from_bytes(uuid(30)).unwrap()
}

fn position(values: [f32; 3]) -> FiniteVec3 {
    FiniteVec3::try_new(values).expect("fixture position")
}

fn look(yaw: f32, pitch: f32) -> LookAngles {
    LookAngles::try_new(yaw, pitch).expect("fixture look")
}

/// Mine target shared by the frozen snapshot and the current world: block
/// `(8, 63, -2)` inside the `(0, 56, -16)` projection.
fn target() -> BlockPos {
    BlockPos::new(8, 63, -2)
}

fn target_chunk() -> ChunkPos {
    // Go `BlockPos.Chunk` uses arithmetic shift: `8 >> 4 == 0`,
    // `-2 >> 4 == -1`.
    ChunkPos::new(0, -1)
}

/// Frozen planning snapshot with a chest at the mine target, chest revision
/// 12, and three oak planks in slot zero.
fn fixture_snapshot(source_tick: u64) -> mornlea_server::contracts::PlanningSnapshot {
    use mornlea_server::contracts::{
        PlanningSnapshot, SnapshotBlock, SnapshotChunkRevision, SnapshotCompanion, SnapshotIssuer,
        SnapshotTaskStatusText, SnapshotTerrain,
    };

    let mut ready = vec![0u8; 137];
    // Column `dx * 33 + dz` of the target inside the projection origin
    // `(-10, 57, -16)` derived from the companion floor `(6, 65, 0)`.
    ready[(18 * 33 + 14) / 8] |= 1 << ((18 * 33 + 14) % 8);
    let mut heights = vec![-65i16; 1089];
    heights[18 * 33 + 14] = 64;
    let mut blocks = vec![0u16; 18_513];
    // Voxel `(dx * 17 + dy) * 33 + dz` of the target.
    blocks[(18 * 17 + 6) * 33 + 14] = 11;
    let mut inventory = [ItemStack {
        item: 0,
        count: 0,
        durability: 0,
    }; 36];
    inventory[0] = ItemStack {
        item: 21,
        count: 3,
        durability: 0,
    };
    PlanningSnapshot::try_new(
        source_tick,
        6000,
        CommandText::try_from_canonical("采一块石头".to_owned()).unwrap(),
        SnapshotIssuer {
            player_id: player(),
            position: position([8.5, 65.0, -1.5]),
            look: look(0.25, -0.1),
            look_hit: Some(BlockPos::new(9, 64, -1)),
        },
        SnapshotCompanion {
            companion_id: companion(),
            position: position([6.5, 65.0, 0.5]),
            look: look(3.0, 0.0),
            task_status: SnapshotTaskStatusText::try_new("空闲".to_owned()).unwrap(),
            inventory,
        },
        vec![mornlea_server::contracts::SnapshotPlayer {
            player_id: player(),
            position: position([8.5, 65.0, -1.5]),
            look: look(0.25, -0.1),
            look_hit: Some(BlockPos::new(9, 64, -1)),
        }],
        vec![SnapshotChunkRevision {
            pos: target_chunk(),
            revision: 12,
        }],
        vec![SnapshotBlock {
            position: target(),
            block_id: 11,
        }],
        SnapshotTerrain::try_new(
            BlockPos::new(-10, 57, -16),
            [33, 17, 33],
            ready,
            heights,
            blocks,
        )
        .expect("fixture terrain"),
    )
    .expect("fixture snapshot")
}

/// Current world matching the frozen snapshot exactly.
fn matching_world(tick: u64) -> CurrentWorld {
    let mut blocks = BTreeMap::new();
    blocks.insert(target(), 11);
    let mut chunk_revisions = BTreeMap::new();
    chunk_revisions.insert(target_chunk(), 12);
    let mut inventory = [ItemStack {
        item: 0,
        count: 0,
        durability: 0,
    }; 36];
    inventory[0] = ItemStack {
        item: 21,
        count: 3,
        durability: 0,
    };
    let mut online_players = BTreeMap::new();
    online_players.insert(player(), [8.5, 65.0, -1.5]);
    CurrentWorld {
        tick,
        blocks,
        chunk_revisions,
        inventory,
        online_players,
    }
}

fn plan_request_at(script: &ScriptAgent, index: usize) -> PlanRequest {
    let submitted = script.submitted();
    assert!(
        submitted.len() > index,
        "plan request {index} submitted among {}",
        submitted.len()
    );
    match submitted.into_iter().nth(index).unwrap() {
        AgentRequest::Plan(request) => request,
        other => panic!("expected plan request, got {other:?}"),
    }
}

fn echo_plan(request: &PlanRequest, plan: AgentPlan) -> AgentResponse {
    echo_plan_generation(request, plan, request.generation)
}

fn echo_plan_generation(request: &PlanRequest, plan: AgentPlan, generation: u64) -> AgentResponse {
    AgentResponse::Plan(PlanResponse {
        leased: request.leased.clone(),
        run_id: request.run_id,
        companion_id: request.companion_id,
        generation,
        snapshot_id: request.snapshot_id,
        snapshot_digest: request.snapshot_digest,
        plan,
    })
}

fn mine_target_plan() -> AgentPlan {
    AgentPlan::try_new(
        "采集容器".to_owned(),
        vec![PlanStep::Mine { x: 8, y: 63, z: -2 }],
    )
    .unwrap()
}

/// Scripted `AgentHandle` double: FIFO poll script, observed submits, and a
/// cancel log. Request ids are echoed from the submitted business request,
/// exactly like the lease provider keys its slots.
struct ScriptAgent {
    polls: Mutex<VecDeque<AgentPoll>>,
    submitted: Mutex<Vec<AgentRequest>>,
    cancelled: Mutex<Vec<AgentRequestId>>,
}

impl ScriptAgent {
    fn new() -> Self {
        Self {
            polls: Mutex::new(VecDeque::new()),
            submitted: Mutex::new(Vec::new()),
            cancelled: Mutex::new(Vec::new()),
        }
    }

    fn push(&self, poll: AgentPoll) {
        self.polls.lock().unwrap().push_back(poll);
    }

    fn submitted(&self) -> Vec<AgentRequest> {
        self.submitted.lock().unwrap().clone()
    }

    fn cancelled(&self) -> Vec<AgentRequestId> {
        self.cancelled.lock().unwrap().clone()
    }
}

fn business_id(request: &AgentRequest) -> Result<AgentRequestId, ServerError> {
    match request {
        AgentRequest::Plan(plan) => Ok(plan.leased.base.request_id),
        AgentRequest::Cancel(cancel) => Ok(cancel.leased.base.request_id),
        AgentRequest::Dialogue(dialogue) => Ok(dialogue.leased.base.request_id),
        AgentRequest::Reconcile(
            mornlea_server::contracts::ReconcileRequest::Active { leased, .. }
            | mornlea_server::contracts::ReconcileRequest::Inactive { leased, .. },
        ) => Ok(leased.base.request_id),
        AgentRequest::Commit(commit) => Ok(commit.leased.base.request_id),
        AgentRequest::Delete(delete) => Ok(delete.leased.base.request_id),
        _ => Err(ServerError::Agent {
            code: mornlea_server::contracts::AgentErrorCode::AgentUnavailable,
            status: 503,
        }),
    }
}

impl AgentHandle for ScriptAgent {
    fn submit(&mut self, request: AgentRequest) -> Result<AgentRequestId, ServerError> {
        let id = business_id(&request)?;
        self.submitted.lock().unwrap().push(request);
        Ok(id)
    }

    fn poll(&mut self, id: AgentRequestId) -> AgentPoll {
        let _ = id;
        self.polls
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or(AgentPoll::Pending)
    }

    fn cancel(&mut self, id: AgentRequestId, _deadline: Deadline) -> Result<(), ServerError> {
        self.cancelled.lock().unwrap().push(id);
        Ok(())
    }

    fn freeze(&mut self, _clock: &dyn Clock) -> Option<mornlea_server::contracts::FrozenLease> {
        None
    }

    fn release(
        &mut self,
        _lease: &mornlea_server::contracts::FrozenLease,
        _deadline: Deadline,
    ) -> Result<(), ServerError> {
        Ok(())
    }

    fn close(&mut self, _deadline: Deadline) -> Result<(), ServerError> {
        Ok(())
    }
}

fn registry(clock: &Arc<StepClock>) -> SnapshotRegistry {
    SnapshotRegistry::try_new(
        clock.clone(),
        Arc::new(CycleEntropy {
            next: Mutex::new(0),
        }),
        "http://127.0.0.1:1/mcp".to_owned(),
    )
    .unwrap()
}

fn plan_dispatch(request_tag: u8, run_tag: u8, source_tick: u64) -> PlanDispatch {
    PlanDispatch {
        companion: companion(),
        generation: 7,
        request_id: AgentRequestId::try_from_bytes(uuid(request_tag)).unwrap(),
        run_id: RunId::try_from_bytes(uuid(run_tag)).unwrap(),
        client: ClientInstanceId::try_from_bytes(uuid(2)).unwrap(),
        namespace: NamespaceId::try_from_bytes(uuid(3)).unwrap(),
        lease: lease(),
        lease_fence: 9,
        snapshot: fixture_snapshot(source_tick),
        source_tick,
        deadline_unix_ms: 1_800_000_000_000,
    }
}

/// A late outcome from the previous attempt must not clear the new gate:
/// the host still holds the companion, installs the fresh outcome, and
/// emits nothing for the stale one.
#[test]
fn wrong_attempt_does_not_clear_gate() {
    let (_start, clock) = StepClock::start();
    let mut script = ScriptAgent::new();
    let mut snapshots = registry(&clock);
    let mut host = PlanHost::new();

    let first = host
        .dispatch_plan(
            &mut script,
            &mut snapshots,
            &*clock,
            plan_dispatch(40, 41, 100),
        )
        .expect("first dispatch admits");
    assert_eq!(first.attempt, 1);
    assert!(host.plan_inflight(companion()));

    let request = plan_request_at(&script, 0);
    script.push(AgentPoll::Completed(echo_plan(
        &request,
        mine_target_plan(),
    )));
    let drained = host
        .drain_outcomes(&mut script, &mut snapshots, &*clock)
        .expect("drain polls");
    assert_eq!(drained.completed, 1);
    let installed = host.install(100, 9, &matching_world(100));
    assert_eq!(installed.installed, 1);
    assert_eq!(installed.envelopes.len(), 1);
    assert!(installed.rejected.is_empty());
    assert!(!host.plan_inflight(companion()));

    let second = host
        .dispatch_plan(
            &mut script,
            &mut snapshots,
            &*clock,
            plan_dispatch(42, 43, 101),
        )
        .expect("second dispatch admits");
    assert_eq!(second.attempt, 2);

    // The previous attempt answers late with its own request identity: the
    // response echoes the first request, not the polled second one.
    script.push(AgentPoll::Completed(echo_plan(
        &request,
        mine_target_plan(),
    )));
    let drained = host
        .drain_outcomes(&mut script, &mut snapshots, &*clock)
        .expect("stale drain polls");
    assert_eq!(drained.completed, 0);
    assert_eq!(drained.failed, 0);
    let installed = host.install(101, 9, &matching_world(101));
    assert!(installed.envelopes.is_empty());
    assert!(installed.rejected.is_empty());
    assert!(
        host.plan_inflight(companion()),
        "stale previous-attempt outcome cleared the active gate"
    );

    let second_request = plan_request_at(&script, 1);
    assert_eq!(
        second_request.leased.base.request_id, second.request_id,
        "second dispatch kept its own request identity"
    );
    assert!(
        script.cancelled().is_empty(),
        "stale outcome sends no CancelRun"
    );

    // A response routed to the new request but naming another generation
    // queues at drain, then fences at install with the gate still held.
    script.push(AgentPoll::Completed(echo_plan_generation(
        &second_request,
        mine_target_plan(),
        8,
    )));
    let drained = host
        .drain_outcomes(&mut script, &mut snapshots, &*clock)
        .expect("mismatched drain polls");
    assert_eq!(drained.completed, 1);
    let installed = host.install(101, 9, &matching_world(101));
    assert!(installed.envelopes.is_empty());
    assert_eq!(
        installed.rejected,
        vec![mornlea_server::agent::host::InstallRejection {
            companion: companion(),
            attempt: 2,
            reason: InstallReject::StaleIdentities,
        }]
    );
    assert!(
        host.plan_inflight(companion()),
        "mismatched outcome cleared the active gate"
    );

    // The true outcome still installs afterwards.
    script.push(AgentPoll::Completed(echo_plan(
        &second_request,
        mine_target_plan(),
    )));
    let drained = host
        .drain_outcomes(&mut script, &mut snapshots, &*clock)
        .expect("true drain polls");
    assert_eq!(drained.completed, 1);
    let installed = host.install(102, 9, &matching_world(102));
    assert_eq!(installed.installed, 1);
    assert_eq!(installed.envelopes.len(), 1);
}

/// The tick install revalidates the arriving plan against the current world:
/// a changed dense target rejects the action with no staged world effect,
/// while the unchanged world installs and emits the first task action.
#[test]
fn plan_current_world_revalidation() {
    let (_start, clock) = StepClock::start();
    let server = EchoServer::serve();
    let wire = AgentHttpWire::try_new(&server.endpoint(), "test-agent-secret", clock.clone())
        .expect("loopback wire");
    let mut agent = LeaseController::try_new(
        LeaseConfig {
            client_instance_id: ClientInstanceId::try_from_bytes(uuid(2)).unwrap(),
            namespace_id: NamespaceId::try_from_bytes(uuid(3)).unwrap(),
        },
        Arc::new(wire),
        clock.clone(),
    )
    .expect("lease controller");
    agent.refresh();
    let (lease_id, fence) = agent.current_lease().expect("acquire admitted");
    let mut snapshots = registry(&clock);
    let mut host = PlanHost::new();

    let mut dispatch = plan_dispatch(40, 41, 100);
    dispatch.lease = lease_id;
    dispatch.lease_fence = fence;
    let ticket = host
        .dispatch_plan(&mut agent, &mut snapshots, &*clock, dispatch)
        .expect("dispatch over the real wire admits");
    assert_eq!(ticket.attempt, 1);

    let start = Instant::now();
    loop {
        let drained = host
            .drain_outcomes(&mut agent, &mut snapshots, &*clock)
            .expect("drain polls");
        if drained.completed == 1 || start.elapsed() > Duration::from_secs(5) {
            break;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(host.outcome_len(), 1);

    // The dense target changed from chest to furnace after the freeze.
    let mut changed = matching_world(101);
    changed.blocks.insert(target(), 9);
    let installed = host.install(101, fence, &changed);
    assert!(
        installed.envelopes.is_empty(),
        "changed target staged an action"
    );
    assert_eq!(installed.installed, 0);
    assert_eq!(
        installed.rejected,
        vec![mornlea_server::agent::host::InstallRejection {
            companion: companion(),
            attempt: 1,
            reason: InstallReject::WorldChanged,
        }]
    );

    // A fresh dispatch against the unchanged world installs and emits the
    // mine hold with the exact dispatch identities.
    let mut dispatch = plan_dispatch(44, 45, 102);
    dispatch.lease = lease_id;
    dispatch.lease_fence = fence;
    let ticket = host
        .dispatch_plan(&mut agent, &mut snapshots, &*clock, dispatch)
        .expect("second dispatch admits");
    let start = Instant::now();
    loop {
        let drained = host
            .drain_outcomes(&mut agent, &mut snapshots, &*clock)
            .expect("drain polls");
        if drained.completed == 1 || start.elapsed() > Duration::from_secs(5) {
            break;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    let installed = host.install(102, fence, &matching_world(102));
    assert_eq!(installed.installed, 1);
    assert_eq!(installed.envelopes.len(), 1);
    let envelope = installed.envelopes.into_iter().next().unwrap();
    assert_eq!(envelope.companion_id, companion());
    assert_eq!(envelope.source_tick, 102);
    assert_eq!(envelope.request_id, ticket.request_id);
    assert_eq!(envelope.generation, 7);
    assert_eq!(envelope.attempt, ticket.attempt);
    match envelope.action {
        mornlea_server::contracts::CompanionAction::MineHold { target: mined } => {
            assert_eq!(mined, target())
        }
        other => panic!("expected mine hold, got {other:?}"),
    }

    assert_eq!(
        server.routes(),
        vec![
            "/v1/namespaces/acquire".to_owned(),
            "/v1/plan".to_owned(),
            "/v1/plan".to_owned()
        ]
    );

    // The emitted envelope admits through the real sessionless ingress
    // against the gate frozen from the dispatch identities.
    let mut ingress = CompanionIngress::try_new().unwrap();
    let gate = CompanionTaskGate::try_new(
        envelope.companion_id,
        envelope.request_id,
        envelope.run_id,
        envelope.snapshot_id,
        envelope.snapshot_digest,
        envelope.generation,
        envelope.attempt,
    )
    .unwrap();
    let receipt = ingress
        .admit(&gate, 102, envelope)
        .expect("task runner emits admissible 2.9a actions");
    assert_eq!(receipt.tick(), 102);
}

/// Four workers fill the host; the fifth dispatch is refused before any
/// side effect, and the outcome queue never stages world effects by itself.
#[test]
fn four_then_five_no_world_effect() {
    let (_start, clock) = StepClock::start();
    let mut script = ScriptAgent::new();
    let mut snapshots = CountingSnapshots::new(registry(&clock));
    let mut host = PlanHost::new();

    for index in 0..4 {
        let mut dispatch = plan_dispatch(40 + 2 * index, 41 + 2 * index, 100);
        dispatch.companion = companion_at(index);
        dispatch.snapshot.companion.companion_id = companion_at(index);
        host.dispatch_plan(&mut script, &mut snapshots, &*clock, dispatch)
            .unwrap_or_else(|error| panic!("dispatch {index} admits: {error:?}"));
    }
    assert_eq!(snapshots.registers(), 4);
    assert_eq!(script.submitted().len(), 4);

    let mut fifth = plan_dispatch(48, 49, 100);
    fifth.companion = companion_at(9);
    fifth.snapshot.companion.companion_id = companion_at(9);
    let refused = host
        .dispatch_plan(&mut script, snapshots.inner(), &*clock, fifth)
        .expect_err("fifth dispatch refuses");
    assert_eq!(
        refused,
        ServerError::Capacity {
            resource: Resource::AgentRuns,
            limit: 4,
            observed: 5,
        }
    );
    assert_eq!(snapshots.registers(), 4, "refused dispatch registered");
    assert_eq!(script.submitted().len(), 4, "refused dispatch submitted");

    let installed = host.install(100, 9, &matching_world(100));
    assert!(installed.envelopes.is_empty());
    assert_eq!(installed.installed, 0);
    assert!(installed.rejected.is_empty());
}

/// Counting `SnapshotPort` wrapper proving a refused dispatch performs no
/// registry side effect.
struct CountingSnapshots {
    inner: SnapshotRegistry,
    registers: usize,
}

impl CountingSnapshots {
    fn new(inner: SnapshotRegistry) -> Self {
        Self {
            inner,
            registers: 0,
        }
    }

    fn inner(&mut self) -> &mut SnapshotRegistry {
        &mut self.inner
    }

    fn registers(&self) -> usize {
        self.registers
    }
}

impl SnapshotPort for CountingSnapshots {
    fn register(
        &mut self,
        namespace: NamespaceId,
        companion: mornlea_domain::CompanionId,
        generation: u64,
        snapshot: mornlea_server::contracts::PlanningSnapshot,
        deadline: Deadline,
    ) -> Result<mornlea_server::contracts::SnapshotRegistration, ServerError> {
        self.registers += 1;
        self.inner
            .register(namespace, companion, generation, snapshot, deadline)
    }

    fn complete(&mut self, id: SnapshotId) -> Result<(), ServerError> {
        self.inner.complete(id)
    }

    fn cancel(&mut self, id: SnapshotId) -> Result<(), ServerError> {
        self.inner.cancel(id)
    }

    fn close(&mut self) -> Result<(), ServerError> {
        self.inner.close()
    }
}

/// Observed loopback requests by route and raw body.
type ObservedRequests = Arc<Mutex<Vec<(String, Vec<u8>)>>>;

/// Deterministic loopback Agent server: echoes the observed request identity
/// into schema-exact Acquire and Plan replies with a fixed mine plan.
struct EchoServer {
    endpoint: String,
    observed: ObservedRequests,
    stop: Arc<AtomicBool>,
    join: Option<std::thread::JoinHandle<()>>,
}

impl EchoServer {
    fn serve() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let endpoint = format!("http://{}", listener.local_addr().unwrap());
        let observed = Arc::new(Mutex::new(Vec::new()));
        let stop = Arc::new(AtomicBool::new(false));
        let join = {
            let observed = observed.clone();
            let stop = stop.clone();
            std::thread::spawn(move || {
                listener.set_nonblocking(true).expect("nonblocking");
                while !stop.load(Ordering::SeqCst) {
                    let Ok((mut stream, _)) = listener.accept() else {
                        std::thread::sleep(Duration::from_millis(2));
                        continue;
                    };
                    stream.set_nonblocking(false).expect("blocking stream");
                    stream
                        .set_read_timeout(Some(Duration::from_secs(5)))
                        .expect("read timeout");
                    stream
                        .set_write_timeout(Some(Duration::from_secs(5)))
                        .expect("write timeout");
                    let Some((path, body)) = read_request(&mut stream) else {
                        continue;
                    };
                    observed.lock().unwrap().push((path.clone(), body.clone()));
                    let reply = reply_for(&path, &body);
                    let response = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                        reply.len(),
                        reply
                    );
                    let _ = stream.write_all(response.as_bytes());
                    let _ = stream.flush();
                    let _ = stream.shutdown(std::net::Shutdown::Both);
                }
            })
        };
        Self {
            endpoint,
            observed,
            stop,
            join: Some(join),
        }
    }

    fn endpoint(&self) -> String {
        self.endpoint.clone()
    }

    fn routes(&self) -> Vec<String> {
        self.observed
            .lock()
            .unwrap()
            .iter()
            .map(|(path, _)| path.clone())
            .collect()
    }
}

impl Drop for EchoServer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}

fn read_request(stream: &mut std::net::TcpStream) -> Option<(String, Vec<u8>)> {
    let mut buffer = Vec::new();
    let mut chunk = [0u8; 1024];
    let head_end = loop {
        if let Some(position) = buffer.windows(4).position(|w| w == b"\r\n\r\n") {
            break position;
        }
        let read = stream.read(&mut chunk).ok()?;
        if read == 0 {
            return None;
        }
        buffer.extend_from_slice(&chunk[..read]);
    };
    let head = String::from_utf8_lossy(&buffer[..head_end]).into_owned();
    let mut lines = head.split("\r\n");
    let path = lines
        .next()
        .unwrap_or_default()
        .split_whitespace()
        .nth(1)
        .unwrap_or_default()
        .to_owned();
    let mut content_length = 0usize;
    for line in lines {
        if let Some((name, value)) = line.split_once(':')
            && name.trim().eq_ignore_ascii_case("content-length")
        {
            content_length = value.trim().parse().unwrap_or(0);
        }
    }
    let mut body = buffer[head_end + 4..].to_vec();
    while body.len() < content_length {
        let read = stream.read(&mut chunk).ok()?;
        if read == 0 {
            break;
        }
        body.extend_from_slice(&chunk[..read]);
    }
    body.truncate(content_length);
    Some((path, body))
}

/// Extracts one string field from the flat request JSON the host encoder
/// writes; the encoder emits exact `"key":"value"` pairs.
fn field_text(body: &[u8], key: &str) -> String {
    let text = String::from_utf8_lossy(body).into_owned();
    let needle = format!("\"{key}\":\"");
    let start = text
        .find(&needle)
        .unwrap_or_else(|| panic!("field {key} present"))
        + needle.len();
    let end = text[start..].find('"').expect("field value closed") + start;
    text[start..end].to_owned()
}

/// Extracts one integer field from the flat request JSON.
fn field_number(body: &[u8], key: &str) -> String {
    let text = String::from_utf8_lossy(body).into_owned();
    let needle = format!("\"{key}\":");
    let start = text
        .find(&needle)
        .unwrap_or_else(|| panic!("field {key} present"))
        + needle.len();
    let end = text[start..].find([',', '}']).expect("field value closed") + start;
    text[start..end].to_owned()
}

fn reply_for(path: &str, body: &[u8]) -> String {
    match path {
        "/v1/namespaces/acquire" => format!(
            "{{\"contract_version\":\"v1\",\"request_id\":\"{}\",\"client_instance_id\":\"{}\",\"namespace_id\":\"{}\",\"lease_id\":\"{}\",\"lease_expires_in_ms\":15000}}",
            field_text(body, "request_id"),
            field_text(body, "client_instance_id"),
            field_text(body, "namespace_id"),
            "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa",
        ),
        "/v1/plan" => format!(
            "{{\"contract_version\":\"v1\",\"request_id\":\"{}\",\"client_instance_id\":\"{}\",\"namespace_id\":\"{}\",\"lease_id\":\"{}\",\"run_id\":\"{}\",\"companion_id\":\"{}\",\"generation\":{},\"snapshot_id\":\"{}\",\"snapshot_digest\":\"{}\",\"plan\":{{\"summary\":\"采集容器\",\"steps\":[{{\"kind\":\"mine\",\"x\":8,\"y\":63,\"z\":-2}}]}}}}",
            field_text(body, "request_id"),
            field_text(body, "client_instance_id"),
            field_text(body, "namespace_id"),
            field_text(body, "lease_id"),
            field_text(body, "run_id"),
            field_text(body, "companion_id"),
            field_number(body, "generation"),
            field_text(body, "snapshot_id"),
            field_text(body, "snapshot_digest"),
        ),
        other => panic!("unexpected route {other}"),
    }
}

/// Every failure path cancels its registry entry and sends an independent
/// `CancelRun` for the admitted run: an invalid model output fails the plan
/// while a transport failure only marks it unavailable.
#[test]
fn failed_outcome_cancels_registry_and_run() {
    let (_start, clock) = StepClock::start();
    let mut script = ScriptAgent::new();
    let mut snapshots = registry(&clock);
    let mut host = PlanHost::new();

    for (index, tags) in [(40u8, 41u8), (42, 43)].iter().enumerate() {
        let mut dispatch = plan_dispatch(tags.0, tags.1, 100);
        dispatch.companion = companion_at(index as u8);
        dispatch.snapshot.companion.companion_id = companion_at(index as u8);
        host.dispatch_plan(&mut script, &mut snapshots, &*clock, dispatch)
            .expect("dispatch admits");
    }
    let first = plan_request_at(&script, 0);
    let second = plan_request_at(&script, 1);

    script.push(AgentPoll::Failed(ServerError::Agent {
        code: AgentErrorCode::InvalidModelOutput,
        status: 422,
    }));
    // Each independent `CancelRun` answers at once so the drain never waits;
    // the cancel polls interleave with the failure polls in drain order.
    script.push(AgentPoll::Completed(AgentResponse::Cancel(
        CancelResponse {
            leased: first.leased.clone(),
            run_id: first.run_id,
            cancelled: true,
        },
    )));
    script.push(AgentPoll::Failed(ServerError::Agent {
        code: AgentErrorCode::AgentUnavailable,
        status: 503,
    }));
    script.push(AgentPoll::Completed(AgentResponse::Cancel(
        CancelResponse {
            leased: first.leased.clone(),
            run_id: first.run_id,
            cancelled: true,
        },
    )));
    let drained = host
        .drain_outcomes(&mut script, &mut snapshots, &*clock)
        .expect("drain polls");
    assert_eq!(drained.failed, 2);
    assert_eq!(drained.completed, 0);
    assert!(!host.plan_inflight(companion_at(0)));
    assert!(!host.plan_inflight(companion_at(1)));

    for request in [first, second] {
        assert!(
            snapshots.complete(request.snapshot_id).is_err(),
            "failed outcome leaves its registry entry"
        );
    }
    let cancels: Vec<_> = script
        .submitted()
        .into_iter()
        .filter_map(|request| match request {
            AgentRequest::Cancel(cancel) => Some(cancel),
            _ => None,
        })
        .collect();
    assert_eq!(cancels.len(), 2);
    assert_eq!(cancels[0].run_id, RunId::try_from_bytes(uuid(41)).unwrap());
    assert_eq!(cancels[1].run_id, RunId::try_from_bytes(uuid(43)).unwrap());

    let mut failures = host.take_failures();
    failures.sort_by_key(|failure| failure.companion.bytes()[1]);
    assert_eq!(failures.len(), 2);
    assert_eq!(failures[0].kind, PlanFailKind::InvalidPlan);
    assert_eq!(failures[1].kind, PlanFailKind::Unavailable);
}

/// Dialogue dispatch admits one request per companion, skips a busy
/// companion or full workers without queueing, and refuses oversized
/// persona text before any side effect.
#[test]
fn dialogue_persona_bound_and_skip() {
    let (_start, clock) = StepClock::start();
    let mut script = ScriptAgent::new();
    let mut host = PlanHost::new();

    let dispatch = |request_tag: u8, run_tag: u8, persona: String| DialogueDispatch {
        companion: companion(),
        generation: 7,
        memory_epoch: 1,
        request_id: AgentRequestId::try_from_bytes(uuid(request_tag)).unwrap(),
        run_id: RunId::try_from_bytes(uuid(run_tag)).unwrap(),
        client: ClientInstanceId::try_from_bytes(uuid(2)).unwrap(),
        namespace: NamespaceId::try_from_bytes(uuid(3)).unwrap(),
        lease: lease(),
        lease_fence: 9,
        persona,
        fact: DialogueFact::Start,
        environment: DialogueEnvironment {
            exposed_blocks: Vec::new(),
            heights: Vec::new(),
        },
        terminal: false,
        deadline_unix_ms: 1_800_000_000_000,
    };

    match host
        .dispatch_dialogue(&mut script, &*clock, dispatch(60, 61, "开场。".to_owned()))
        .expect("first dialogue admits")
    {
        DialogueAdmit::Admitted { attempt, .. } => assert_eq!(attempt, 1),
        DialogueAdmit::Skipped => panic!("first dialogue skipped"),
    }
    assert!(host.dialogue_inflight(companion()));
    assert_eq!(
        host.dispatch_dialogue(&mut script, &*clock, dispatch(62, 63, "后来。".to_owned())),
        Ok(DialogueAdmit::Skipped)
    );
    assert_eq!(script.submitted().len(), 1);

    let oversized = host.dispatch_dialogue(
        &mut script,
        &*clock,
        dispatch(64, 65, "x".repeat(MAX_PERSONA_BYTES + 1)),
    );
    assert_eq!(
        oversized,
        Err(ServerError::InvalidInput {
            field: "dialogue_persona",
        })
    );
    assert_eq!(script.submitted().len(), 1, "refused dialogue submitted");
}
