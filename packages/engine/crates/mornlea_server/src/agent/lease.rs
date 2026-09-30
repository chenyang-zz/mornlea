//! Namespace lease provider over the Agent control wire.
//!
//! The frozen machine is `Absent -> AcquirePending -> Active{lease, fence,
//! expiry} -> HeartbeatPending or Frozen -> Closed`, mirroring the Go
//! `companionAgentLeaseController` in
//! `packages/server/server/companion_agent.go`. Two counters have distinct
//! jobs: a checked `control_revision` increments before every control RPC so
//! a late outcome can never install, and a `lease_fence` changes only on
//! acquire, so a successful heartbeat preserves the fencing identity business
//! plans are admitted under. TTL 15000 ms and heartbeat cadence 5000 ms are
//! frozen; each control RPC runs under `min(now + 5 s, expiry)`.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use crate::contracts::{
    AgentErrorCode, AgentHandle, AgentPoll, AgentRequest, AgentRequestId, AgentResponse,
    BaseIdentity, CancelRequest, ClientInstanceId, Clock, CommitRequest, Deadline, DeleteRequest,
    DialogueRequest, FrozenLease, LeaseId, LeasedIdentity, NamespaceId, Operation, PlanRequest,
    ReconcileRequest, Resource, ServerError,
};

/// Frozen heartbeat cadence in milliseconds.
pub const HEARTBEAT_EVERY_MS: u64 = 5_000;
/// Frozen lease TTL in milliseconds; the wire also pins it per response.
pub const LEASE_TTL: Duration = Duration::from_millis(crate::contracts::LEASE_EXPIRES_IN_MS);
/// Upper bound of one control RPC: `min(now + 5 s, lease expiry)`.
pub const CONTROL_RPC_TIMEOUT: Duration = Duration::from_secs(5);
/// Caller-owned release bound, mirroring the Go `companionAgentReleaseTimeout`.
pub const RELEASE_TIMEOUT: Duration = Duration::from_secs(5);

/// Transport seam behind the lease controller and business workers. Methods
/// take `&self` with interior mutability so concurrent RPCs stay independent,
/// like Go's per-call goroutines over one shared client.
pub trait AgentWire: Send + Sync {
    fn rpc(&self, request: AgentRequest, deadline: Deadline) -> Result<AgentResponse, ServerError> {
        self.rpc_cancellable(request, deadline, &RpcCancellation::default())
    }
    fn rpc_cancellable(
        &self,
        request: AgentRequest,
        deadline: Deadline,
        cancellation: &RpcCancellation,
    ) -> Result<AgentResponse, ServerError>;
    fn close(&self);
}

/// One request's retirement signal. Clones share a one-way flag; cancelling
/// an owner must not close the shared wire or another request's connection.
#[derive(Clone, Default)]
pub struct RpcCancellation(Arc<AtomicBool>);

impl RpcCancellation {
    pub fn cancel(&self) {
        self.0.store(true, Ordering::Release);
    }

    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Acquire)
    }
}

/// Control machine phase. `AcquirePending` and `HeartbeatPending` are the
/// in-flight windows between a control round starting and its outcome
/// settling; a frozen or closed machine discards every late outcome.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ControlPhase {
    Absent,
    AcquirePending,
    HeartbeatPending,
    Active,
    Frozen,
    Closed,
}

/// Immutable identity this controller acquires and heartbeats for.
#[derive(Clone, Copy, Debug)]
pub struct LeaseConfig {
    pub client_instance_id: ClientInstanceId,
    pub namespace_id: NamespaceId,
}

/// One admitted control round. `revision` is the `control_revision` value the
/// round consumed; settling an outcome against any other revision discards it.
#[derive(Clone, Debug)]
pub struct ControlRound {
    pub revision: u64,
    pub request: AgentRequest,
    pub deadline: Deadline,
}

struct ActiveLease {
    id: LeaseId,
    fence: u64,
    expires_at: Instant,
}

struct InflightControl {
    revision: u64,
}

enum BusinessState {
    Pending,
    Done(AgentPoll),
}

// Admission installs the join before releasing the core lock. Terminal results
// retain this ownership and capacity until their consumer explicitly retires them.
struct BusinessSlot {
    cancel: RpcCancellation,
    state: Arc<Mutex<BusinessState>>,
    join: Option<JoinHandle<()>>,
}

struct WorkerHandle {
    cancel: RpcCancellation,
    join: Option<JoinHandle<()>>,
}

struct Core {
    phase: ControlPhase,
    active: Option<ActiveLease>,
    control_revision: u64,
    fence_counter: u64,
    inflight: Option<InflightControl>,
    frozen: Option<FrozenLease>,
    released: bool,
    closed: bool,
    close_started: bool,
    business: HashMap<AgentRequestId, BusinessSlot>,
    worker: Option<WorkerHandle>,
}

struct Shared {
    config: LeaseConfig,
    core: Mutex<Core>,
    wire: Arc<dyn AgentWire>,
    clock: Arc<dyn Clock + Send + Sync>,
    next_id: AtomicU64,
    seed: u64,
}

fn unavailable() -> ServerError {
    ServerError::Agent {
        code: AgentErrorCode::AgentUnavailable,
        status: AgentErrorCode::AgentUnavailable.status(),
    }
}

/// The lease controller implementing the frozen `AgentHandle` port. The
/// control machine is driven by `refresh` (the spawned worker's loop body) or
/// by the explicit `start_control_round`/`settle_control` pair, which lets a
/// host drive control RPCs from its own scheduler and keeps the machine
/// deterministic under a test clock.
pub struct LeaseController {
    shared: Arc<Shared>,
}

impl LeaseController {
    pub fn try_new(
        config: LeaseConfig,
        wire: Arc<dyn AgentWire>,
        clock: Arc<dyn Clock + Send + Sync>,
    ) -> Result<Self, ServerError> {
        let seed = (clock.unix_ms() as u64).rotate_left(17)
            ^ (Arc::as_ptr(&wire) as *const u8 as usize as u64);
        Ok(Self {
            shared: Arc::new(Shared {
                config,
                core: Mutex::new(Core {
                    phase: ControlPhase::Absent,
                    active: None,
                    control_revision: 0,
                    fence_counter: 0,
                    inflight: None,
                    frozen: None,
                    released: false,
                    closed: false,
                    close_started: false,
                    business: HashMap::new(),
                    worker: None,
                }),
                wire,
                clock,
                next_id: AtomicU64::new(0),
                seed,
            }),
        })
    }

    /// Spawns the pacing control worker: an immediate refresh, then one
    /// refresh per heartbeat interval measured on the controller clock.
    /// Freeze signals cancellation while retaining the join; close reaps it.
    pub fn spawn_control_worker(&self) -> Result<(), ServerError> {
        let mut core = self.shared.core.lock().unwrap();
        if core.closed || matches!(core.phase, ControlPhase::Frozen | ControlPhase::Closed) {
            return Err(unavailable());
        }
        if core.worker.is_some() {
            return Ok(());
        }
        let cancel = RpcCancellation::default();
        let shared = self.shared.clone();
        let worker_cancel = cancel.clone();
        // Install actual producer ownership before it can acquire the core lock.
        let join = std::thread::Builder::new()
            .spawn(move || {
                loop {
                    if worker_cancel.is_cancelled() {
                        return;
                    }
                    refresh_shared(&shared, &worker_cancel);
                    if worker_cancel.is_cancelled()
                        || matches!(
                            shared.core.lock().unwrap().phase,
                            ControlPhase::Frozen | ControlPhase::Closed
                        )
                    {
                        return;
                    }
                    let due = shared
                        .clock
                        .monotonic()
                        .checked_add(Duration::from_millis(HEARTBEAT_EVERY_MS));
                    let Some(due) = due else { return };
                    while shared.clock.monotonic() < due {
                        if worker_cancel.is_cancelled() {
                            return;
                        }
                        std::thread::sleep(Duration::from_millis(5));
                    }
                }
            })
            .map_err(|_| unavailable())?;
        core.worker = Some(WorkerHandle {
            cancel,
            join: Some(join),
        });
        Ok(())
    }

    /// One synchronous control cycle: start a round, send it on the wire
    /// under the computed deadline, and settle the outcome. This is exactly
    /// the spawned worker's loop body.
    pub fn refresh(&self) {
        refresh_shared(&self.shared, &RpcCancellation::default());
    }

    /// Starts one control round: bumps `control_revision`, computes the
    /// `min(now + 5 s, expiry)` deadline, chooses acquire or heartbeat from
    /// the live lease, and parks the machine in the matching pending phase.
    /// Returns `None` on a closed or frozen machine, or when no deadline can
    /// be built because the current lease already expired (which clears it,
    /// so the next round reacquires).
    pub fn start_control_round(&self) -> Option<ControlRound> {
        let mut core = self.shared.core.lock().unwrap();
        if core.closed || core.phase == ControlPhase::Frozen || core.phase == ControlPhase::Closed {
            return None;
        }
        let now = self.shared.clock.monotonic();
        let revision = match core.control_revision.checked_add(1) {
            Some(revision) => revision,
            None => {
                core.closed = true;
                core.phase = ControlPhase::Closed;
                return None;
            }
        };
        core.control_revision = revision;
        let timeout = now.checked_add(CONTROL_RPC_TIMEOUT)?;
        if core
            .active
            .as_ref()
            .is_some_and(|active| active.expires_at <= now)
        {
            // The lease lapsed between refreshes: drop it here so this
            // round reacquires instead of heartbeating a dead identity.
            core.active = None;
        }
        let deadline = match &core.active {
            Some(active) if active.expires_at < timeout => Deadline::at(active.expires_at),
            _ => Deadline::at(timeout),
        };
        let active_id = core.active.as_ref().map(|active| active.id);
        let request = match active_id {
            None => {
                core.phase = ControlPhase::AcquirePending;
                AgentRequest::Acquire(BaseIdentity {
                    request_id: mint_request_id(&self.shared),
                    client_instance_id: self.shared.config.client_instance_id,
                    namespace_id: self.shared.config.namespace_id,
                })
            }
            Some(lease_id) => {
                core.phase = ControlPhase::HeartbeatPending;
                AgentRequest::Heartbeat(LeasedIdentity {
                    base: BaseIdentity {
                        request_id: mint_request_id(&self.shared),
                        client_instance_id: self.shared.config.client_instance_id,
                        namespace_id: self.shared.config.namespace_id,
                    },
                    lease_id,
                })
            }
        };
        core.inflight = Some(InflightControl { revision });
        Some(ControlRound {
            revision,
            request,
            deadline,
        })
    }

    /// Settles one control outcome. Only a matching-revision outcome that
    /// arrives before its deadline and echoes the exact round identity can
    /// install; everything else clears the active lease so the next round
    /// reacquires, and a late success never revives a cleared fence.
    pub fn settle_control(&self, round: ControlRound, outcome: Result<AgentResponse, ServerError>) {
        let mut core = self.shared.core.lock().unwrap();
        let matches = core
            .inflight
            .as_ref()
            .map(|inflight| inflight.revision == round.revision)
            .unwrap_or(false)
            && core.control_revision == round.revision;
        if !matches {
            // Late outcome from a superseded or fenced-off round.
            return;
        }
        core.inflight = None;
        if core.closed {
            return;
        }
        let expired = self.shared.clock.monotonic() >= round.deadline.instant();
        let response = outcome
            .ok()
            .filter(|response| control_echo_matches(&round.request, response));
        let (Some(response), false) = (response, expired) else {
            core.active = None;
            core.phase = ControlPhase::Absent;
            return;
        };
        let now = self.shared.clock.monotonic();
        match (&round.request, response) {
            (AgentRequest::Acquire(_), AgentResponse::Acquire(lease)) => {
                let fence = core.fence_counter.wrapping_add(1);
                core.fence_counter = fence;
                core.active = Some(ActiveLease {
                    id: lease.leased.lease_id,
                    fence,
                    expires_at: now + LEASE_TTL,
                });
                core.phase = ControlPhase::Active;
            }
            (AgentRequest::Heartbeat(leased), AgentResponse::Heartbeat(_)) => {
                // A successful heartbeat preserves the lease fence.
                let fence = core.active.as_ref().map(|active| active.fence);
                let Some(fence) = fence else {
                    core.active = None;
                    core.phase = ControlPhase::Absent;
                    return;
                };
                core.active = Some(ActiveLease {
                    id: leased.lease_id,
                    fence,
                    expires_at: now + LEASE_TTL,
                });
                core.phase = ControlPhase::Active;
            }
            _ => {
                core.active = None;
                core.phase = ControlPhase::Absent;
            }
        }
    }

    /// The live machine phase.
    pub fn control_phase(&self) -> ControlPhase {
        self.shared.core.lock().unwrap().phase
    }

    /// The current unexpired lease identity and its fence, if any.
    pub fn current_lease(&self) -> Option<(LeaseId, u64)> {
        let core = self.shared.core.lock().unwrap();
        eligible_business_lease(&core, false, self.shared.clock.monotonic())
    }

    /// Requests remain charged until the consumer retires their owned outcome.
    pub fn retained_requests(&self) -> usize {
        self.shared.core.lock().unwrap().business.len()
    }

    /// Counts unfinished business producers, excluding retained terminal outcomes.
    pub fn pending_business_workers(&self) -> usize {
        self.shared
            .core
            .lock()
            .unwrap()
            .business
            .values()
            .filter(|slot| slot.join.as_ref().is_some_and(|join| !join.is_finished()))
            .count()
    }

    /// Control ownership remains retained until close reaps its producer.
    pub fn retained_control_workers(&self) -> usize {
        usize::from(self.shared.core.lock().unwrap().worker.is_some())
    }

    /// Reports whether the retained control producer has finished.
    pub fn pending_control_worker(&self) -> bool {
        self.shared
            .core
            .lock()
            .unwrap()
            .worker
            .as_ref()
            .is_some_and(|worker| worker.join.as_ref().is_some_and(|join| !join.is_finished()))
    }

    fn mint_request_id(&self) -> AgentRequestId {
        mint_request_id(&self.shared)
    }
}

/// Refresh body shared by the convenience method and the spawned worker.
fn refresh_shared(shared: &Arc<Shared>, cancellation: &RpcCancellation) {
    let controller = LeaseController {
        shared: shared.clone(),
    };
    if let Some(round) = controller.start_control_round() {
        let outcome =
            shared
                .wire
                .rpc_cancellable(round.request.clone(), round.deadline, cancellation);
        controller.settle_control(round, outcome);
    }
}

/// A control outcome may only install when the response variant matches the
/// round and every echoed identity field is the exact request value.
fn control_echo_matches(request: &AgentRequest, response: &AgentResponse) -> bool {
    match (request, response) {
        (AgentRequest::Acquire(base), AgentResponse::Acquire(lease)) => {
            base_matches(&lease.leased.base, base)
        }
        (AgentRequest::Heartbeat(leased), AgentResponse::Heartbeat(echo)) => {
            leased_matches(&echo.leased, leased)
        }
        _ => false,
    }
}

fn base_matches(echo: &BaseIdentity, base: &BaseIdentity) -> bool {
    echo.request_id == base.request_id
        && echo.client_instance_id == base.client_instance_id
        && echo.namespace_id == base.namespace_id
}

fn leased_matches(echo: &LeasedIdentity, leased: &LeasedIdentity) -> bool {
    base_matches(&echo.base, &leased.base) && echo.lease_id == leased.lease_id
}

/// Mints a fresh canonical UUIDv4 request id. Only uniqueness and the v4
/// layout are contractual; the SplitMix64 stream is seeded per controller
/// from the wall clock and the wire identity so concurrent controllers never
/// share a sequence.
fn mint_request_id(shared: &Arc<Shared>) -> AgentRequestId {
    loop {
        let counter = shared.next_id.fetch_add(1, Ordering::SeqCst);
        let mut state = shared
            .seed
            .wrapping_add(counter.wrapping_mul(0x9e37_79b9_7f4a_7c15));
        let mut bytes = [0u8; 16];
        for chunk in bytes.chunks_mut(8) {
            state = state.wrapping_add(0x9e37_79b9_7f4a_7c15);
            state = (state ^ (state >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
            state = (state ^ (state >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
            let mixed = state ^ (state >> 31);
            for (index, byte) in chunk.iter_mut().enumerate() {
                *byte = (mixed >> (index * 8)) as u8;
            }
        }
        bytes[6] = (bytes[6] & 0x0f) | 0x40;
        bytes[8] = (bytes[8] & 0x3f) | 0x80;
        if let Ok(id) = AgentRequestId::try_from_bytes(bytes) {
            return id;
        }
    }
}

fn business_identity(
    request: &AgentRequest,
) -> Result<(&LeasedIdentity, AgentRequestId), ServerError> {
    match request {
        AgentRequest::Acquire(_) | AgentRequest::Heartbeat(_) | AgentRequest::Release(_) => {
            // Control-plane kinds belong to the lease machine, not to
            // business admission.
            Err(ServerError::InvalidInput {
                field: "agent_request_kind",
            })
        }
        AgentRequest::Plan(PlanRequest { leased, .. })
        | AgentRequest::Cancel(CancelRequest { leased, .. })
        | AgentRequest::Dialogue(DialogueRequest { leased, .. })
        | AgentRequest::Reconcile(ReconcileRequest::Active { leased, .. })
        | AgentRequest::Reconcile(ReconcileRequest::Inactive { leased, .. })
        | AgentRequest::Commit(CommitRequest { leased, .. })
        | AgentRequest::Delete(DeleteRequest { leased, .. }) => {
            Ok((leased, leased.base.request_id))
        }
    }
}

/// Finalization uses the retained release identity after planner admission
/// closes. New planning and late planning results cannot cross freeze.
fn is_finalizer_request(request: &AgentRequest) -> bool {
    matches!(
        request,
        AgentRequest::Commit(_)
            | AgentRequest::Reconcile(_)
            | AgentRequest::Delete(_)
            | AgentRequest::Cancel(_)
    )
}

/// Admission and completion share one lease policy so release or expiry
/// fences a finalizer result just as freeze fences a planner result.
fn eligible_business_lease(core: &Core, finalizer: bool, now: Instant) -> Option<(LeaseId, u64)> {
    if core.closed || core.released {
        return None;
    }
    if core.phase == ControlPhase::Frozen {
        return core
            .frozen
            .as_ref()
            .filter(|lease| finalizer && lease.expires_at > now)
            .map(|lease| (lease.lease, lease.lease_fence));
    }
    core.active
        .as_ref()
        .filter(|lease| lease.expires_at > now)
        .map(|lease| (lease.id, lease.fence))
}

impl AgentHandle for LeaseController {
    fn submit(&mut self, request: AgentRequest) -> Result<AgentRequestId, ServerError> {
        let (leased, id) = business_identity(&request)?;
        let mut core = self.shared.core.lock().unwrap();
        let finalizer = is_finalizer_request(&request);
        if core.closed || (core.phase == ControlPhase::Frozen && !finalizer) {
            return Err(unavailable());
        }
        if core.business.contains_key(&id) {
            return Err(ServerError::InvalidInput {
                field: "agent_request_id",
            });
        }
        // Freeze closes planner work while finalization keeps the retained
        // identity until release. A caller cannot borrow another namespace.
        let now = self.shared.clock.monotonic();
        let (lease_snapshot, fence_snapshot) =
            eligible_business_lease(&core, finalizer, now).ok_or_else(unavailable)?;
        if leased.lease_id != lease_snapshot
            || leased.base.client_instance_id != self.shared.config.client_instance_id
            || leased.base.namespace_id != self.shared.config.namespace_id
        {
            return Err(unavailable());
        }
        if core.business.len() >= 64 {
            return Err(ServerError::Capacity {
                resource: Resource::AgentRuns,
                limit: 64,
                observed: 65,
            });
        }
        let deadline = Deadline::after(now, crate::agent::http::BUSINESS_RPC_TIMEOUT)?;
        let cancel = RpcCancellation::default();
        let state = Arc::new(Mutex::new(BusinessState::Pending));
        let worker_cancel = cancel.clone();
        let worker_state = state.clone();
        let shared = self.shared.clone();
        // One RPC per admission. The correlation lock also prevents the worker
        // from publishing before its actual join is installed in the owner map.
        let join = std::thread::Builder::new()
            .spawn(move || {
                let outcome = shared
                    .wire
                    .rpc_cancellable(request, deadline, &worker_cancel);
                let poll = {
                    let core = shared.core.lock().unwrap();
                    if worker_cancel.is_cancelled() || core.closed {
                        AgentPoll::Failed(unavailable())
                    } else {
                        let correlated =
                            eligible_business_lease(&core, finalizer, shared.clock.monotonic())
                                == Some((lease_snapshot, fence_snapshot));
                        match (outcome, correlated) {
                            (Ok(response), true) => AgentPoll::Completed(response),
                            (Ok(_), false) => AgentPoll::Failed(unavailable()),
                            (Err(error), _) => AgentPoll::Failed(error),
                        }
                    }
                };
                *worker_state.lock().unwrap() = BusinessState::Done(poll);
            })
            .map_err(|_| unavailable())?;
        core.business.insert(
            id,
            BusinessSlot {
                cancel,
                state,
                join: Some(join),
            },
        );
        Ok(id)
    }

    fn poll(&mut self, id: AgentRequestId) -> AgentPoll {
        let core = self.shared.core.lock().unwrap();
        let Some(slot) = core.business.get(&id) else {
            return AgentPoll::Failed(unavailable());
        };
        if slot.join.as_ref().is_some_and(|join| !join.is_finished()) {
            return AgentPoll::Pending;
        }
        match &*slot.state.lock().unwrap() {
            BusinessState::Pending => AgentPoll::Failed(ServerError::Internal {
                invariant: "agent business worker",
            }),
            BusinessState::Done(poll) => poll.clone(),
        }
    }

    fn cancel(&mut self, id: AgentRequestId, deadline: Deadline) -> Result<(), ServerError> {
        let cancel = {
            let core = self.shared.core.lock().unwrap();
            core.business
                .get(&id)
                .ok_or_else(unavailable)?
                .cancel
                .clone()
        };
        cancel.cancel();
        let mut wall_expiry = None;
        loop {
            let retired = {
                let mut core = self.shared.core.lock().unwrap();
                let slot = core.business.get(&id).ok_or_else(unavailable)?;
                if slot.join.as_ref().is_some_and(|join| join.is_finished()) {
                    core.business.remove(&id)
                } else {
                    None
                }
            };
            if let Some(mut slot) = retired {
                // Only a finished producer can relinquish its charged slot;
                // joining outside the core lock consumes its outcome exactly once.
                return slot
                    .join
                    .take()
                    .ok_or(ServerError::Internal {
                        invariant: "agent business worker",
                    })?
                    .join()
                    .map_err(|_| ServerError::Internal {
                        invariant: "agent business worker",
                    });
            }
            let now = self.shared.clock.monotonic();
            let expiry = match wall_expiry {
                Some(expiry) => expiry,
                None => {
                    // A paused controller clock cannot extend caller ownership
                    // waits. The real clock only bounds this retirement attempt.
                    let expiry = Instant::now()
                        .checked_add(deadline.instant().saturating_duration_since(now))
                        .ok_or(ServerError::InvalidInput { field: "deadline" })?;
                    wall_expiry = Some(expiry);
                    expiry
                }
            };
            let wall_now = Instant::now();
            if deadline.expired(now) || wall_now >= expiry {
                return Err(ServerError::Timeout {
                    operation: Operation::AgentRpc,
                });
            }
            std::thread::sleep(
                Duration::from_millis(2)
                    .min(deadline.instant().saturating_duration_since(now))
                    .min(expiry.saturating_duration_since(wall_now)),
            );
        }
    }

    fn freeze(&mut self, clock: &dyn Clock) -> Option<FrozenLease> {
        let mut core = self.shared.core.lock().unwrap();
        if !core.closed {
            // Fence every late outcome without relinquishing its producer handle.
            match core.control_revision.checked_add(1) {
                Some(revision) => {
                    core.control_revision = revision;
                    core.phase = ControlPhase::Frozen;
                }
                None => {
                    core.closed = true;
                    core.phase = ControlPhase::Closed;
                }
            }
            core.inflight = None;
        }
        if let Some(worker) = &core.worker {
            // Freeze has no caller deadline, so it can only signal the producer.
            // Close still owns the join if fencing overflowed or an RPC hangs.
            worker.cancel.cancel();
        }
        if core.closed {
            return None;
        }
        if core.frozen.is_none() && !core.released {
            let retained = core
                .active
                .as_ref()
                .filter(|active| active.expires_at > clock.monotonic())
                .map(|active| (active.id, active.fence, active.expires_at));
            if let Some((id, fence, expires_at)) = retained {
                core.frozen = Some(FrozenLease {
                    client: self.shared.config.client_instance_id,
                    namespace: self.shared.config.namespace_id,
                    lease: id,
                    lease_fence: fence,
                    expires_at,
                });
            }
        }
        // Release retries retain the same frozen identity until success/expiry.
        core.frozen
            .clone()
            .filter(|lease| lease.expires_at > clock.monotonic())
    }

    fn release(&mut self, lease: &FrozenLease, deadline: Deadline) -> Result<(), ServerError> {
        let request = {
            let mut core = self.shared.core.lock().unwrap();
            if core.closed {
                return Err(unavailable());
            }
            if core.released {
                return Ok(());
            }
            let Some(frozen) = &core.frozen else {
                // Nothing retained: the machine phase records the skip
                // instead of inventing a receipt.
                core.released = true;
                return Ok(());
            };
            if frozen.lease != lease.lease
                || frozen.lease_fence != lease.lease_fence
                || frozen.client != lease.client
                || frozen.namespace != lease.namespace
            {
                return Err(ServerError::InvalidInput { field: "lease" });
            }
            if frozen.expires_at <= self.shared.clock.monotonic() {
                // Expiry permits the skip.
                core.released = true;
                return Ok(());
            }
            AgentRequest::Release(LeasedIdentity {
                base: BaseIdentity {
                    request_id: self.mint_request_id(),
                    client_instance_id: lease.client,
                    namespace_id: lease.namespace,
                },
                lease_id: lease.lease,
            })
        };
        let outcome = self.shared.wire.rpc(request, deadline);
        let mut core = self.shared.core.lock().unwrap();
        match outcome {
            Ok(AgentResponse::Release(echo))
                if echo.leased.lease_id == lease.lease
                    && echo.leased.base.client_instance_id == lease.client
                    && echo.leased.base.namespace_id == lease.namespace =>
            {
                core.released = true;
                core.frozen = None;
                Ok(())
            }
            Ok(_) => Err(unavailable()),
            // A failed release keeps the frozen identity for the retry.
            Err(error) => Err(error),
        }
    }

    fn close(&mut self, deadline: Deadline) -> Result<(), ServerError> {
        let first_close = {
            let mut core = self.shared.core.lock().unwrap();
            // Closed phase alone may reflect revision overflow, not wire close.
            // The separate marker makes retries resume all retained ownership.
            let first_close = !core.close_started;
            core.close_started = true;
            core.closed = true;
            core.phase = ControlPhase::Closed;
            if first_close && let Some(revision) = core.control_revision.checked_add(1) {
                core.control_revision = revision;
            }
            core.active = None;
            core.inflight = None;
            core.frozen = None;
            if let Some(worker) = &core.worker {
                worker.cancel.cancel();
            }
            for slot in core.business.values() {
                slot.cancel.cancel();
            }
            first_close
        };
        if first_close {
            // Interrupt request sockets before any producer wait. Unfinished
            // joins remain retained if a provider cannot yet observe the signal.
            self.shared.wire.close();
        }
        let mut wall_expiry = None;
        loop {
            let (finished, outstanding) = {
                let mut core = self.shared.core.lock().unwrap();
                let mut finished = Vec::with_capacity(65);
                if core.worker.as_ref().is_some_and(|worker| {
                    worker.join.as_ref().is_some_and(|join| join.is_finished())
                }) {
                    let mut worker = core.worker.take().unwrap();
                    finished.push((worker.join.take().unwrap(), "agent control worker"));
                }
                let retired: Vec<_> = core
                    .business
                    .iter()
                    .filter_map(|(id, slot)| {
                        slot.join
                            .as_ref()
                            .is_some_and(|join| join.is_finished())
                            .then_some(*id)
                    })
                    .collect();
                for id in retired {
                    let mut slot = core.business.remove(&id).unwrap();
                    finished.push((slot.join.take().unwrap(), "agent business worker"));
                }
                let outstanding = core.worker.is_some() || !core.business.is_empty();
                (finished, outstanding)
            };
            let mut error = None;
            for (join, invariant) in finished {
                if join.join().is_err() && error.is_none() {
                    error = Some(ServerError::Internal { invariant });
                }
            }
            if let Some(error) = error {
                return Err(error);
            }
            if !outstanding {
                return Ok(());
            }
            let now = self.shared.clock.monotonic();
            let expiry = match wall_expiry {
                Some(expiry) => expiry,
                None => {
                    // A fixed authority clock cannot extend a caller's close
                    // budget; wall time bounds only this ownership wait.
                    let expiry = Instant::now()
                        .checked_add(deadline.instant().saturating_duration_since(now))
                        .ok_or(ServerError::InvalidInput { field: "deadline" })?;
                    wall_expiry = Some(expiry);
                    expiry
                }
            };
            let wall_now = Instant::now();
            if deadline.expired(now) || wall_now >= expiry {
                return Err(ServerError::Timeout {
                    operation: Operation::Close,
                });
            }
            std::thread::sleep(
                Duration::from_millis(2)
                    .min(deadline.instant().saturating_duration_since(now))
                    .min(expiry.saturating_duration_since(wall_now)),
            );
        }
    }
}
