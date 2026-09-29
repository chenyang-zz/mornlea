//! Companion plan and dialogue task host over the loopback Agent boundary.
//!
//! The host consumes the accepted wire and lease provider through
//! [`AgentHandle`] and the accepted frozen-snapshot provider through
//! [`SnapshotPort`]; it never touches the Python Agent process, embeds no
//! interpreter, and opens no channel other than the loopback HTTP and MCP
//! contracts. Dispatch freezes the authoritative snapshot, the private
//! `source_tick`, and a `u64` attempt, registers the snapshot before the
//! request, and records the exact dispatch identities. Outcomes are polled
//! off the tick and installed at the tick boundary, where the active slot,
//! generation, attempt, fence, run, snapshot, and digest are checked and the
//! current world is revalidated before any action is emitted. The task runner
//! alone emits [`CompanionActionEnvelope`] values for the sessionless
//! ingress; a stale outcome can never clear another in-flight gate. Worker,
//! queue, and outcome capacity are bounded; admission refuses before any side
//! effect once a bound is reached.

use std::collections::{BTreeMap, VecDeque};
use std::time::{Duration, Instant};

use mornlea_domain::{BlockPos, ChunkPos, CompanionId, PlayerId};
use mornlea_storage::ItemStack;

use super::memory::{CommitReservation, valid_dialogue_line, valid_memory_text};
use crate::contracts::{
    AgentErrorCode, AgentHandle, AgentPlan, AgentPoll, AgentRequest, AgentRequestId, AgentResponse,
    BaseIdentity, CancelRequest, ClientInstanceId, Clock, CompanionAction, CompanionActionEnvelope,
    Deadline, DialogueEnvironment, DialogueFact, DialogueRequest, DialogueResponse, LeaseId,
    LeasedIdentity, NamespaceId, PlanBlock, PlanRequest, PlanResponse, PlanStep, Resource, RunId,
    ServerError, SnapshotId, SnapshotPort,
};
use crate::core::companion_ingress::CompanionTaskGate;

/// Shared bound for plan and dialogue workers, matching the Go companion
/// `MaxActive` model slots.
pub const WORKER_CAPACITY: usize = 4;

/// Valid plans waiting for tick install, at most one queue entry per worker.
pub const TASK_CAPACITY: usize = 4;

/// Polled outcomes waiting for tick install.
pub const OUTCOME_CAPACITY: usize = 4;

/// Independent `CancelRun` budget on failure paths; the run exists only after
/// the request was admitted.
pub const CANCEL_RUN_TIMEOUT: Duration = Duration::from_millis(100);

/// Dialogue RPC timeout and snapshot registration horizon.
pub const DIALOGUE_TIMEOUT: Duration = Duration::from_secs(30);

/// Snapshot registration horizon for plan dispatch; same value as the
/// dialogue timeout by packet, named separately so the plan path does not
/// read as dialogue-coupled.
pub const PLAN_REGISTER_TIMEOUT: Duration = Duration::from_secs(30);

/// Hard ceiling for a dialogue RPC deadline.
pub const DIALOGUE_TIMEOUT_MAX: Duration = Duration::from_secs(60);

/// Persona text ceiling in bytes.
pub const MAX_PERSONA_BYTES: usize = 4096;

/// Dialogue line ceiling in bytes.
pub const MAX_LINE_BYTES: usize = 256;

/// Terminal proposal summary ceiling in bytes.
pub const MAX_PROPOSAL_SUMMARY_BYTES: usize = 2048;

/// Unpadded base64url alphabet for the snapshot capability bearer.
const BASE64URL_ALPHABET: &[u8; 64] =
    b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";

/// Salt mixing the host cancel counter into distinct v4 correlation ids.
const CANCEL_ID_SALT: u64 = 0x9e37_79b9_7f4a_7c15;

/// Renders raw capability bytes as the unpadded base64url bearer the
/// snapshot registry authorizes.
fn base64url_unpadded(input: &[u8]) -> String {
    let mut out = String::with_capacity(input.len().div_ceil(3) * 4);
    for chunk in input.chunks(3) {
        let word = match chunk {
            [a, b, c] => (*a as u32) << 16 | (*b as u32) << 8 | *c as u32,
            [a, b] => (*a as u32) << 16 | (*b as u32) << 8,
            [a] => (*a as u32) << 16,
            _ => 0,
        };
        let width = match chunk.len() {
            3 => 4,
            2 => 3,
            _ => 2,
        };
        for index in 0..width {
            let sextet = ((word >> (18 - 6 * index)) & 0x3f) as usize;
            out.push(BASE64URL_ALPHABET[sextet] as char);
        }
    }
    out
}

/// Reports whether a persona fits the wire ceiling, sharing the memory
/// text rule the codec enforces.
fn valid_persona(value: &str) -> bool {
    valid_memory_text(value, MAX_PERSONA_BYTES)
}

/// Reports whether a dialogue line fits the wire shape, sharing the line
/// rule the codec enforces.
fn valid_line(value: &str) -> bool {
    value.len() <= MAX_LINE_BYTES && valid_dialogue_line(value)
}

/// Reports whether a block id is a planner-legal mine target.
///
/// The rule mirrors the Go `planMineableBlock`: crops, farmland, torches, and
/// wild grass are explicitly refused, and every other block needs a single
/// registered drop, so air, barrier, bedrock, fluids, snow layers, and any
/// unregistered id stay refused.
fn plan_mineable(block: u16) -> bool {
    if (35..=36).contains(&block)
        || (37..=44).contains(&block)
        || (46..=53).contains(&block)
        || (54..=61).contains(&block)
        || (71..=75).contains(&block)
        || block == 84
    {
        return false;
    }
    matches!(
        block,
        2 | 3 | 4 | 6 | 7 | 8 | 9 | 10 | 11 | 12 | 13 | 14 | 15 | 16 | 17 | 18 | 19 | 20 | 21
            | 22 | 23 | 24 | 25 | 26 | 45 | 62..=70 | 76..=83 | 89
    )
}

/// Maps a plan block to the item id the companion must hold.
///
/// The table mirrors the Go `planPlaceBlocks` reverse index over
/// `ItemPlacement`: each placeable block names its placing item.
fn plan_block_item(block: PlanBlock) -> u16 {
    match block {
        PlanBlock::Brick => 24,
        PlanBlock::Chest => 14,
        PlanBlock::Clay => 27,
        PlanBlock::Cobblestone => 16,
        PlanBlock::Dirt => 2,
        PlanBlock::Furnace => 8,
        PlanBlock::Glass => 23,
        PlanBlock::Grass => 3,
        PlanBlock::Gravel => 19,
        PlanBlock::IronBlock => 9,
        PlanBlock::Leaves => 22,
        PlanBlock::LightBlock => 15,
        PlanBlock::MossyCobblestone => 29,
        PlanBlock::OakLog => 20,
        PlanBlock::OakPlanks => 21,
        PlanBlock::RoofTile => 26,
        PlanBlock::Sand => 18,
        PlanBlock::SmoothStone => 17,
        PlanBlock::SnowBlock => 28,
        PlanBlock::Stone => 1,
        PlanBlock::StoneBrick => 4,
        PlanBlock::WhiteWool => 25,
        PlanBlock::Workbench => 38,
    }
}

/// Maps a plan block to its protocol block id.
///
/// The numbers are the Go `core` stable block ids in `block.go` declaration
/// order, the same registry the wire names decode into.
fn plan_block_id(block: PlanBlock) -> u16 {
    match block {
        PlanBlock::Brick => 21,
        PlanBlock::Chest => 11,
        PlanBlock::Clay => 24,
        PlanBlock::Cobblestone => 13,
        PlanBlock::Dirt => 3,
        PlanBlock::Furnace => 9,
        PlanBlock::Glass => 20,
        PlanBlock::Grass => 4,
        PlanBlock::Gravel => 16,
        PlanBlock::IronBlock => 10,
        PlanBlock::Leaves => 19,
        PlanBlock::LightBlock => 12,
        PlanBlock::MossyCobblestone => 26,
        PlanBlock::OakLog => 17,
        PlanBlock::OakPlanks => 18,
        PlanBlock::RoofTile => 23,
        PlanBlock::Sand => 15,
        PlanBlock::SmoothStone => 14,
        PlanBlock::SnowBlock => 25,
        PlanBlock::Stone => 2,
        PlanBlock::StoneBrick => 6,
        PlanBlock::WhiteWool => 22,
        PlanBlock::Workbench => 45,
    }
}

/// Derives the chunk column of a block position with arithmetic shift,
/// mirroring the Go `BlockPos.Chunk` rule for negative coordinates.
fn target_chunk(target: BlockPos) -> ChunkPos {
    ChunkPos::new(target.x() >> 4, target.z() >> 4)
}

/// Reads one dense terrain cell: the position must sit inside the frozen
/// `[33, 17, 33]` projection and its column must be marked ready.
fn terrain_lookup(
    origin: BlockPos,
    ready_columns: &[u8],
    blocks: &[u16],
    target: BlockPos,
) -> Option<u16> {
    let dx = target.x().checked_sub(origin.x())?;
    let dy = target.y().checked_sub(origin.y())?;
    let dz = target.z().checked_sub(origin.z())?;
    if !(0..33).contains(&dx) || !(0..17).contains(&dy) || !(0..33).contains(&dz) {
        return None;
    }
    let column = (dx * 33 + dz) as usize;
    if ready_columns.get(column / 8)? & (1 << (column % 8)) == 0 {
        return None;
    }
    blocks.get(((dx * 17 + dy) * 33 + dz) as usize).copied()
}

/// Current-world projection the tick side rebuilds for outcome install.
///
/// The host never reads authority state directly; the reducer supplies this
/// bounded view at the tick boundary and install revalidates the arriving
/// plan against it, the way the Go authority rebuilds the plan snapshot
/// before accepting a planner outcome.
#[derive(Clone, Debug, PartialEq)]
pub struct CurrentWorld {
    /// Authority tick the view was taken at.
    pub tick: u64,
    /// Dense current blocks for the planned region; absence means the cell
    /// is not ready, so lookup fails closed.
    pub blocks: BTreeMap<BlockPos, u16>,
    /// Current revision per planned chunk; a missing entry fails closed.
    pub chunk_revisions: BTreeMap<ChunkPos, u64>,
    /// Companion inventory, all 36 slots.
    pub inventory: [ItemStack; 36],
    /// Online players by id with their current positions.
    pub online_players: BTreeMap<PlayerId, [f32; 3]>,
}

/// Plan dispatch frozen by the caller: the snapshot carries the instruction
/// and the frozen context, while the surrounding identities are separate
/// correlation records that never travel on the wire.
pub struct PlanDispatch {
    /// Companion the plan is for.
    pub companion: CompanionId,
    /// Task generation the plan belongs to.
    pub generation: u64,
    /// Caller-minted business request id.
    pub request_id: AgentRequestId,
    /// Caller-minted run id.
    pub run_id: RunId,
    /// Client instance owning the lease.
    pub client: ClientInstanceId,
    /// Namespace owning the lease.
    pub namespace: NamespaceId,
    /// Lease the business request names.
    pub lease: LeaseId,
    /// Fence observed with the lease.
    pub lease_fence: u64,
    /// Authoritative snapshot to freeze and register.
    pub snapshot: crate::contracts::PlanningSnapshot,
    /// Private authority tick the snapshot was taken at.
    pub source_tick: u64,
    /// Business deadline as unix milliseconds.
    pub deadline_unix_ms: i64,
}

/// Dialogue dispatch frozen by the caller.
pub struct DialogueDispatch {
    /// Companion the line is for.
    pub companion: CompanionId,
    /// Task generation the dialogue belongs to.
    pub generation: u64,
    /// Memory epoch the dialogue was composed under.
    pub memory_epoch: u64,
    /// Caller-minted business request id.
    pub request_id: AgentRequestId,
    /// Caller-minted run id.
    pub run_id: RunId,
    /// Client instance owning the lease.
    pub client: ClientInstanceId,
    /// Namespace owning the lease.
    pub namespace: NamespaceId,
    /// Lease the business request names.
    pub lease: LeaseId,
    /// Fence observed with the lease.
    pub lease_fence: u64,
    /// Persona text, at most [`MAX_PERSONA_BYTES`] bytes with no NUL.
    pub persona: String,
    /// Dialogue fact node driving the line.
    pub fact: DialogueFact,
    /// Frozen environment facts for the line.
    pub environment: DialogueEnvironment,
    /// Whether this is the terminal node of the dialogue.
    pub terminal: bool,
    /// Business deadline as unix milliseconds, at most 60 seconds out.
    pub deadline_unix_ms: i64,
}

/// Admission receipt for one plan dispatch.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PlanTicket {
    /// Companion the plan was dispatched for.
    pub companion: CompanionId,
    /// Attempt allocated to this dispatch.
    pub attempt: u64,
    /// Business request id carrying the plan RPC.
    pub request_id: AgentRequestId,
}

/// Dialogue admission outcome: over-capacity dialogue is skipped, never
/// queued, while invalid input is a hard refusal before any side effect.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DialogueAdmit {
    /// Request admitted with the allocated attempt.
    Admitted {
        /// Companion the dialogue was dispatched for.
        companion: CompanionId,
        /// Attempt allocated to this dispatch.
        attempt: u64,
        /// Business request id carrying the dialogue RPC.
        request_id: AgentRequestId,
    },
    /// Skipped because a dialogue is already in flight or workers are full.
    Skipped,
}

/// Tick install rejection cause.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InstallReject {
    /// Outcome identities do not match the active slot; the slot is kept.
    StaleIdentities,
    /// Current world drifted from the frozen plan; the slot is released.
    WorldChanged,
    /// Plan or dialogue payload is invalid; the slot is released.
    InvalidPlan,
    /// Task queue is full; the validated plan is dropped and the slot freed.
    TaskFull,
}

/// One rejected outcome.
#[derive(Clone, Debug, PartialEq)]
pub struct InstallRejection {
    /// Companion the outcome named.
    pub companion: CompanionId,
    /// Attempt the outcome named.
    pub attempt: u64,
    /// Why the outcome was rejected.
    pub reason: InstallReject,
}

/// Non-terminal speech effect for the caller to present.
#[derive(Clone, Debug, PartialEq)]
pub struct DialogueLine {
    /// Companion speaking the line.
    pub companion: CompanionId,
    /// Line text, within [`MAX_LINE_BYTES`] bytes.
    pub line: String,
}

/// Valid plan admitted to the task queue for the downstream task runner.
#[derive(Clone, Debug, PartialEq)]
pub struct InstalledPlan {
    /// Companion the plan belongs to.
    pub companion: CompanionId,
    /// Attempt that produced the plan.
    pub attempt: u64,
    /// Validated plan steps.
    pub plan: AgentPlan,
}

/// Tick install summary: installed plans with their first actions, speech
/// lines, terminal memory reservations, and rejections.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct InstallReport {
    /// Valid plans moved into the task queue.
    pub installed: usize,
    /// First actions emitted by the task runner, in install order.
    pub envelopes: Vec<CompanionActionEnvelope>,
    /// Speech lines to present, in install order.
    pub lines: Vec<DialogueLine>,
    /// Terminal reservations for the memory owner, in install order.
    pub reservations: Vec<CommitReservation>,
    /// Rejected outcomes, in install order.
    pub rejected: Vec<InstallRejection>,
}

/// Drain summary: polled business outcomes now awaiting tick install.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct DrainReport {
    /// Newly queued outcomes.
    pub completed: usize,
    /// Cleaned failure paths.
    pub failed: usize,
}

/// Failure classification for a business outcome: an invalid model output
/// fails the plan, while malformed, protocol, lease, or transport failures
/// only mark the plan unavailable.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PlanFailKind {
    /// Model output refused by the contract.
    InvalidPlan,
    /// Request never produced a usable answer.
    Unavailable,
}

/// One cleaned business failure for the tick-side task lanes.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PlanFailure {
    /// Companion the failed request belonged to.
    pub companion: CompanionId,
    /// Attempt the failed request belonged to.
    pub attempt: u64,
    /// How the failure classifies.
    pub kind: PlanFailKind,
}

fn classify_failure(error: &ServerError) -> PlanFailKind {
    match error {
        ServerError::Agent {
            code: AgentErrorCode::InvalidModelOutput,
            ..
        } => PlanFailKind::InvalidPlan,
        _ => PlanFailKind::Unavailable,
    }
}

/// One active plan slot: the frozen dispatch identities plus the frozen
/// snapshot the install revalidation reads.
struct PlanSlot {
    companion: CompanionId,
    generation: u64,
    attempt: u64,
    request_id: AgentRequestId,
    run_id: RunId,
    snapshot_id: SnapshotId,
    digest: [u8; 32],
    lease: LeaseId,
    fence: u64,
    client: ClientInstanceId,
    namespace: NamespaceId,
    frozen: crate::contracts::PlanningSnapshot,
}

/// One active dialogue: the frozen dispatch identities for outcome binding.
struct DialogueSlot {
    companion: CompanionId,
    generation: u64,
    memory_epoch: u64,
    attempt: u64,
    request_id: AgentRequestId,
    run_id: RunId,
    lease: LeaseId,
    fence: u64,
    client: ClientInstanceId,
    namespace: NamespaceId,
    terminal: bool,
}

/// Queued successful outcome awaiting tick install.
enum QueuedOutcome {
    Plan {
        companion: CompanionId,
        attempt: u64,
        response: PlanResponse,
    },
    Dialogue {
        companion: CompanionId,
        attempt: u64,
        response: DialogueResponse,
    },
}

/// Queued valid plan awaiting the task runner.
struct QueuedPlan {
    companion: CompanionId,
    attempt: u64,
    plan: AgentPlan,
}

/// Companion plan and dialogue task host.
///
/// The host owns at most [`WORKER_CAPACITY`] combined plan and dialogue
/// workers, one plan plus one dialogue slot per companion, a task queue of
/// at most [`TASK_CAPACITY`] valid plans, and an outcome queue of at most
/// [`OUTCOME_CAPACITY`] polled outcomes. Polling and cleanup run off the
/// tick; only [`PlanHost::install`] runs at the tick boundary and it performs
/// no I/O.
pub struct PlanHost {
    plans: BTreeMap<CompanionId, PlanSlot>,
    dialogues: BTreeMap<CompanionId, DialogueSlot>,
    attempts: BTreeMap<CompanionId, u64>,
    dialogue_attempts: BTreeMap<CompanionId, u64>,
    tasks: VecDeque<QueuedPlan>,
    outcomes: VecDeque<QueuedOutcome>,
    failures: Vec<PlanFailure>,
    cancel_counter: u64,
}

impl PlanHost {
    /// Creates an empty host; no workers exist until the first dispatch.
    pub fn new() -> Self {
        Self {
            plans: BTreeMap::new(),
            dialogues: BTreeMap::new(),
            attempts: BTreeMap::new(),
            dialogue_attempts: BTreeMap::new(),
            tasks: VecDeque::new(),
            outcomes: VecDeque::new(),
            failures: Vec::new(),
            cancel_counter: 0,
        }
    }

    /// Whether the companion holds an in-flight plan gate.
    pub fn plan_inflight(&self, companion: CompanionId) -> bool {
        self.plans.contains_key(&companion)
    }

    /// Whether the companion holds an in-flight dialogue request.
    pub fn dialogue_inflight(&self, companion: CompanionId) -> bool {
        self.dialogues.contains_key(&companion)
    }

    /// Number of valid plans waiting for the task runner.
    pub fn task_len(&self) -> usize {
        self.tasks.len()
    }

    /// Number of polled outcomes waiting for tick install.
    pub fn outcome_len(&self) -> usize {
        self.outcomes.len()
    }

    /// Attempt most recently allocated for the companion, if any.
    pub fn attempt(&self, companion: CompanionId) -> Option<u64> {
        self.attempts.get(&companion).copied()
    }

    /// Takes cleaned business failures for the tick-side task lanes.
    pub fn take_failures(&mut self) -> Vec<PlanFailure> {
        std::mem::take(&mut self.failures)
    }

    /// Takes validated plans for the task runner, in install order.
    pub fn take_installed(&mut self) -> Vec<InstalledPlan> {
        self.tasks
            .drain(..)
            .map(|queued| InstalledPlan {
                companion: queued.companion,
                attempt: queued.attempt,
                plan: queued.plan,
            })
            .collect()
    }

    /// Mints a fresh v4 correlation id for a `CancelRun` request.
    ///
    /// Business request and run ids stay caller-minted; only the cancel
    /// envelope needs a new identity, and a deterministic `SplitMix64`
    /// stream over the host counter keeps every dispatch reproducible.
    fn mint_cancel_id(&mut self) -> AgentRequestId {
        loop {
            self.cancel_counter = self.cancel_counter.wrapping_add(1);
            let mut state = CANCEL_ID_SALT
                .wrapping_add(self.cancel_counter.wrapping_mul(0x9e37_79b9_7f4a_7c15));
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

    /// Dispatches one plan: validates input, freezes the snapshot through
    /// the registry, submits the business request, and records the slot.
    ///
    /// Every refusal happens before the registry call, so a refused dispatch
    /// registers nothing and submits nothing; a failed submit cancels the
    /// registration it just created.
    pub fn dispatch_plan(
        &mut self,
        agent: &mut dyn AgentHandle,
        snapshots: &mut dyn SnapshotPort,
        clock: &dyn Clock,
        dispatch: PlanDispatch,
    ) -> Result<PlanTicket, ServerError> {
        if dispatch.generation == 0 {
            return Err(ServerError::InvalidInput {
                field: "companion_generation",
            });
        }
        if dispatch.deadline_unix_ms < 1 {
            return Err(ServerError::InvalidInput { field: "plan" });
        }
        if dispatch.source_tick != dispatch.snapshot.source_tick() {
            return Err(ServerError::InvalidInput {
                field: "source_tick",
            });
        }
        if self.plans.contains_key(&dispatch.companion) {
            return Err(ServerError::Capacity {
                resource: Resource::AgentRuns,
                limit: 1,
                observed: 2,
            });
        }
        if self.plans.len() + self.dialogues.len() >= WORKER_CAPACITY {
            return Err(ServerError::Capacity {
                resource: Resource::AgentRuns,
                limit: WORKER_CAPACITY,
                observed: WORKER_CAPACITY + 1,
            });
        }
        let attempt = self.attempts.get(&dispatch.companion).copied().unwrap_or(0);
        let attempt = attempt.checked_add(1).ok_or(ServerError::InvalidInput {
            field: "companion_attempt",
        })?;
        let instruction = dispatch.snapshot.instruction.clone();
        let frozen = dispatch.snapshot.clone();
        let deadline = Deadline::after(clock.monotonic(), PLAN_REGISTER_TIMEOUT).map_err(|_| {
            ServerError::Agent {
                code: AgentErrorCode::AgentUnavailable,
                status: 503,
            }
        })?;
        let registration = snapshots.register(
            dispatch.namespace,
            dispatch.companion,
            dispatch.generation,
            dispatch.snapshot,
            deadline,
        )?;
        // The gate constructor re-checks the frozen identities before the
        // request becomes visible; a refusal cancels the registration.
        let _gate = CompanionTaskGate::try_new(
            dispatch.companion,
            dispatch.request_id,
            dispatch.run_id,
            registration.id,
            registration.digest,
            dispatch.generation,
            attempt,
        )
        .inspect_err(|_| {
            let _ = snapshots.cancel(registration.id);
        })?;
        let leased = LeasedIdentity {
            base: BaseIdentity {
                request_id: dispatch.request_id,
                client_instance_id: dispatch.client,
                namespace_id: dispatch.namespace,
            },
            lease_id: dispatch.lease,
        };
        let request = PlanRequest::try_new(
            leased,
            dispatch.run_id,
            dispatch.companion,
            dispatch.generation,
            registration.id,
            registration.digest,
            dispatch.deadline_unix_ms,
            registration.mcp_endpoint.clone(),
            base64url_unpadded(&registration.capability),
            instruction,
        )
        .inspect_err(|_| {
            let _ = snapshots.cancel(registration.id);
        })?;
        if let Err(submit_error) = agent.submit(AgentRequest::Plan(request)) {
            let _ = snapshots.cancel(registration.id);
            return Err(submit_error);
        }
        self.attempts.insert(dispatch.companion, attempt);
        self.plans.insert(
            dispatch.companion,
            PlanSlot {
                companion: dispatch.companion,
                generation: dispatch.generation,
                attempt,
                request_id: dispatch.request_id,
                run_id: dispatch.run_id,
                snapshot_id: registration.id,
                digest: registration.digest,
                lease: dispatch.lease,
                fence: dispatch.lease_fence,
                client: dispatch.client,
                namespace: dispatch.namespace,
                frozen,
            },
        );
        Ok(PlanTicket {
            companion: dispatch.companion,
            attempt,
            request_id: dispatch.request_id,
        })
    }

    /// Dispatches one dialogue line request, or skips it when the companion
    /// already holds one or workers are full; oversized persona text and an
    /// out-of-range deadline are refused before any side effect.
    pub fn dispatch_dialogue(
        &mut self,
        agent: &mut dyn AgentHandle,
        clock: &dyn Clock,
        dispatch: DialogueDispatch,
    ) -> Result<DialogueAdmit, ServerError> {
        if dispatch.generation == 0 {
            return Err(ServerError::InvalidInput {
                field: "companion_generation",
            });
        }
        if !valid_persona(&dispatch.persona) {
            return Err(ServerError::InvalidInput {
                field: "dialogue_persona",
            });
        }
        if dispatch.deadline_unix_ms < 1
            || dispatch.deadline_unix_ms
                > clock
                    .unix_ms()
                    .saturating_add(DIALOGUE_TIMEOUT_MAX.as_millis() as i64)
        {
            return Err(ServerError::InvalidInput {
                field: "dialogue_deadline",
            });
        }
        if self.dialogues.contains_key(&dispatch.companion) {
            return Ok(DialogueAdmit::Skipped);
        }
        if self.plans.len() + self.dialogues.len() >= WORKER_CAPACITY {
            return Ok(DialogueAdmit::Skipped);
        }
        let attempt = self
            .dialogue_attempts
            .get(&dispatch.companion)
            .copied()
            .unwrap_or(0);
        let attempt = attempt.checked_add(1).ok_or(ServerError::InvalidInput {
            field: "dialogue_attempt",
        })?;
        let leased = LeasedIdentity {
            base: BaseIdentity {
                request_id: dispatch.request_id,
                client_instance_id: dispatch.client,
                namespace_id: dispatch.namespace,
            },
            lease_id: dispatch.lease,
        };
        let request = DialogueRequest {
            leased,
            run_id: dispatch.run_id,
            companion_id: dispatch.companion,
            generation: dispatch.generation,
            memory_epoch: dispatch.memory_epoch,
            deadline_unix_ms: dispatch.deadline_unix_ms,
            persona: dispatch.persona,
            fact_node: dispatch.fact,
            environment: dispatch.environment,
            terminal: dispatch.terminal,
        };
        agent.submit(AgentRequest::Dialogue(request))?;
        self.dialogue_attempts.insert(dispatch.companion, attempt);
        self.dialogues.insert(
            dispatch.companion,
            DialogueSlot {
                companion: dispatch.companion,
                generation: dispatch.generation,
                memory_epoch: dispatch.memory_epoch,
                attempt,
                request_id: dispatch.request_id,
                run_id: dispatch.run_id,
                lease: dispatch.lease,
                fence: dispatch.lease_fence,
                client: dispatch.client,
                namespace: dispatch.namespace,
                terminal: dispatch.terminal,
            },
        );
        Ok(DialogueAdmit::Admitted {
            companion: dispatch.companion,
            attempt,
            request_id: dispatch.request_id,
        })
    }

    /// Polls admitted business requests once; successful responses wait in
    /// the outcome queue while every failure path cancels its registry entry
    /// and sends an independent `CancelRun` for the admitted run.
    ///
    /// A response echoing another request id is dropped without touching any
    /// gate; a full outcome queue leaves the remaining slots awaiting the
    /// next drain.
    pub fn drain_outcomes(
        &mut self,
        agent: &mut dyn AgentHandle,
        snapshots: &mut dyn SnapshotPort,
        clock: &dyn Clock,
    ) -> Result<DrainReport, ServerError> {
        let mut report = DrainReport::default();
        let companions: Vec<CompanionId> = self.plans.keys().copied().collect();
        for companion in companions {
            let Some(slot) = self.plans.get(&companion) else {
                continue;
            };
            match agent.poll(slot.request_id) {
                AgentPoll::Pending => {}
                AgentPoll::Completed(AgentResponse::Plan(response)) => {
                    if response.leased.base.request_id != slot.request_id {
                        continue;
                    }
                    if self.outcomes.len() >= OUTCOME_CAPACITY {
                        continue;
                    }
                    let _ = snapshots.complete(slot.snapshot_id);
                    self.outcomes.push_back(QueuedOutcome::Plan {
                        companion,
                        attempt: slot.attempt,
                        response,
                    });
                    report.completed += 1;
                }
                AgentPoll::Completed(_) => {
                    self.fail_plan_slot(
                        agent,
                        snapshots,
                        clock,
                        companion,
                        &ServerError::Agent {
                            code: AgentErrorCode::InvalidModelOutput,
                            status: 422,
                        },
                    );
                    report.failed += 1;
                }
                AgentPoll::Failed(error) => {
                    self.fail_plan_slot(agent, snapshots, clock, companion, &error);
                    report.failed += 1;
                }
            }
        }
        let speakers: Vec<CompanionId> = self.dialogues.keys().copied().collect();
        for companion in speakers {
            let Some(slot) = self.dialogues.get(&companion) else {
                continue;
            };
            match agent.poll(slot.request_id) {
                AgentPoll::Pending => {}
                AgentPoll::Completed(AgentResponse::Dialogue(response)) => {
                    if response.leased.base.request_id != slot.request_id {
                        continue;
                    }
                    if self.outcomes.len() >= OUTCOME_CAPACITY {
                        continue;
                    }
                    self.outcomes.push_back(QueuedOutcome::Dialogue {
                        companion,
                        attempt: slot.attempt,
                        response,
                    });
                    report.completed += 1;
                }
                AgentPoll::Completed(_) => {
                    self.fail_dialogue_slot(
                        agent,
                        clock,
                        companion,
                        &ServerError::Agent {
                            code: AgentErrorCode::InvalidModelOutput,
                            status: 422,
                        },
                    );
                    report.failed += 1;
                }
                AgentPoll::Failed(error) => {
                    self.fail_dialogue_slot(agent, clock, companion, &error);
                    report.failed += 1;
                }
            }
        }
        Ok(report)
    }

    /// Cleans one failed plan slot: releases the gate, cancels the registry
    /// entry, sends the independent `CancelRun`, and records the failure.
    fn fail_plan_slot(
        &mut self,
        agent: &mut dyn AgentHandle,
        snapshots: &mut dyn SnapshotPort,
        clock: &dyn Clock,
        companion: CompanionId,
        error: &ServerError,
    ) {
        let Some(slot) = self.plans.remove(&companion) else {
            return;
        };
        let _ = snapshots.cancel(slot.snapshot_id);
        let leased = LeasedIdentity {
            base: BaseIdentity {
                request_id: self.mint_cancel_id(),
                client_instance_id: slot.client,
                namespace_id: slot.namespace,
            },
            lease_id: slot.lease,
        };
        cancel_admitted_run(agent, clock, leased, slot.run_id);
        self.failures.push(PlanFailure {
            companion,
            attempt: slot.attempt,
            kind: classify_failure(error),
        });
    }

    /// Cleans one failed dialogue slot: releases the gate, sends the
    /// independent `CancelRun`, and records the failure.
    fn fail_dialogue_slot(
        &mut self,
        agent: &mut dyn AgentHandle,
        clock: &dyn Clock,
        companion: CompanionId,
        error: &ServerError,
    ) {
        let Some(slot) = self.dialogues.remove(&companion) else {
            return;
        };
        let leased = LeasedIdentity {
            base: BaseIdentity {
                request_id: self.mint_cancel_id(),
                client_instance_id: slot.client,
                namespace_id: slot.namespace,
            },
            lease_id: slot.lease,
        };
        cancel_admitted_run(agent, clock, leased, slot.run_id);
        self.failures.push(PlanFailure {
            companion,
            attempt: slot.attempt,
            kind: classify_failure(error),
        });
    }

    /// Installs queued outcomes at the tick boundary: checks the active
    /// slot, generation, attempt, fence, run, snapshot, and digest, then
    /// revalidates the plan against the current world before the task queue
    /// admits it and the runner emits the first action. Identity mismatches
    /// keep the slot; world and payload failures release it.
    pub fn install(&mut self, tick: u64, lease_fence: u64, world: &CurrentWorld) -> InstallReport {
        let mut report = InstallReport::default();
        let outcomes: Vec<QueuedOutcome> = self.outcomes.drain(..).collect();
        for outcome in outcomes {
            match outcome {
                QueuedOutcome::Plan {
                    companion,
                    attempt,
                    response,
                } => self.install_plan(
                    &mut report,
                    tick,
                    lease_fence,
                    world,
                    companion,
                    attempt,
                    &response,
                ),
                QueuedOutcome::Dialogue {
                    companion,
                    attempt,
                    response,
                } => self.install_dialogue(&mut report, lease_fence, companion, attempt, &response),
            }
        }
        report
    }

    /// Installs one plan outcome against its slot and the current world.
    #[allow(clippy::too_many_arguments)]
    fn install_plan(
        &mut self,
        report: &mut InstallReport,
        tick: u64,
        lease_fence: u64,
        world: &CurrentWorld,
        companion: CompanionId,
        attempt: u64,
        response: &PlanResponse,
    ) {
        let Some(slot) = self.plans.get(&companion) else {
            return;
        };
        if slot.attempt != attempt
            || slot.fence != lease_fence
            || response.leased.base.request_id != slot.request_id
            || response.leased.lease_id != slot.lease
            || response.run_id != slot.run_id
            || response.companion_id != slot.companion
            || response.generation != slot.generation
            || response.snapshot_id != slot.snapshot_id
            || response.snapshot_digest != slot.digest
        {
            report.rejected.push(InstallRejection {
                companion,
                attempt,
                reason: InstallReject::StaleIdentities,
            });
            return;
        }
        if validate_against_current(slot, &response.plan, world).is_err() {
            self.plans.remove(&companion);
            report.rejected.push(InstallRejection {
                companion,
                attempt,
                reason: InstallReject::WorldChanged,
            });
            return;
        }
        if self.tasks.len() >= TASK_CAPACITY {
            self.plans.remove(&companion);
            report.rejected.push(InstallRejection {
                companion,
                attempt,
                reason: InstallReject::TaskFull,
            });
            return;
        }
        let Some(envelope) = first_action(slot, tick, &response.plan) else {
            self.plans.remove(&companion);
            report.rejected.push(InstallRejection {
                companion,
                attempt,
                reason: InstallReject::InvalidPlan,
            });
            return;
        };
        self.plans.remove(&companion);
        self.tasks.push_back(QueuedPlan {
            companion,
            attempt,
            plan: response.plan.clone(),
        });
        report.installed += 1;
        report.envelopes.push(envelope);
    }

    /// Installs one dialogue outcome against its slot.
    fn install_dialogue(
        &mut self,
        report: &mut InstallReport,
        lease_fence: u64,
        companion: CompanionId,
        attempt: u64,
        response: &DialogueResponse,
    ) {
        let Some(slot) = self.dialogues.get(&companion) else {
            return;
        };
        if slot.attempt != attempt
            || slot.fence != lease_fence
            || response.leased.base.request_id != slot.request_id
            || response.leased.lease_id != slot.lease
            || response.run_id != slot.run_id
            || response.companion_id != slot.companion
            || response.generation != slot.generation
            || response.memory_epoch != slot.memory_epoch
        {
            report.rejected.push(InstallRejection {
                companion,
                attempt,
                reason: InstallReject::StaleIdentities,
            });
            return;
        }
        if !valid_line(&response.line) {
            self.dialogues.remove(&companion);
            report.rejected.push(InstallRejection {
                companion,
                attempt,
                reason: InstallReject::InvalidPlan,
            });
            return;
        }
        if slot.terminal {
            let Some(proposal) = &response.memory_proposal else {
                self.dialogues.remove(&companion);
                report.rejected.push(InstallRejection {
                    companion,
                    attempt,
                    reason: InstallReject::InvalidPlan,
                });
                return;
            };
            let Ok(reservation) = CommitReservation::try_new(
                companion,
                response.memory_epoch,
                proposal.operation_id,
                proposal.base_revision,
                proposal.summary.clone(),
                response.line.clone(),
            ) else {
                self.dialogues.remove(&companion);
                report.rejected.push(InstallRejection {
                    companion,
                    attempt,
                    reason: InstallReject::InvalidPlan,
                });
                return;
            };
            self.dialogues.remove(&companion);
            report.reservations.push(reservation);
            return;
        }
        self.dialogues.remove(&companion);
        report.lines.push(DialogueLine {
            companion,
            line: response.line.clone(),
        });
    }
}

impl Default for PlanHost {
    fn default() -> Self {
        Self::new()
    }
}

/// Revalidates every plan step against the current world.
///
/// Mine and place targets must read the same dense value frozen and now,
/// mine targets must stay minable, place blocks must stay held, follow
/// targets must stay online at the same position, and every mine or place
/// chunk revision must be present and unchanged on both sides. A missing
/// cell or revision fails closed, mirroring the Go authority rebuild.
fn validate_against_current(
    slot: &PlanSlot,
    plan: &AgentPlan,
    world: &CurrentWorld,
) -> Result<(), InstallReject> {
    let terrain = &slot.frozen.terrain;
    for step in &plan.steps {
        match step {
            PlanStep::GoTo { .. } => {}
            PlanStep::Mine { x, y, z } => {
                let target = BlockPos::new(*x, *y, *z);
                let frozen = terrain_lookup(
                    terrain.origin,
                    &terrain.ready_columns,
                    &terrain.blocks,
                    target,
                )
                .ok_or(InstallReject::WorldChanged)?;
                let current = world
                    .blocks
                    .get(&target)
                    .copied()
                    .ok_or(InstallReject::WorldChanged)?;
                if frozen != current || !plan_mineable(current) {
                    return Err(InstallReject::WorldChanged);
                }
                check_revision(slot, world, target)?;
            }
            PlanStep::Place { x, y, z, block } => {
                let target = BlockPos::new(*x, *y, *z);
                let frozen = terrain_lookup(
                    terrain.origin,
                    &terrain.ready_columns,
                    &terrain.blocks,
                    target,
                )
                .ok_or(InstallReject::WorldChanged)?;
                let current = world
                    .blocks
                    .get(&target)
                    .copied()
                    .ok_or(InstallReject::WorldChanged)?;
                if frozen != current {
                    return Err(InstallReject::WorldChanged);
                }
                let item = plan_block_item(*block);
                if !world
                    .inventory
                    .iter()
                    .any(|held| held.item == item && held.count >= 1)
                {
                    return Err(InstallReject::WorldChanged);
                }
                check_revision(slot, world, target)?;
            }
            PlanStep::Follow { player_id } => {
                let frozen = slot
                    .frozen
                    .online_players
                    .iter()
                    .find(|player| player.player_id == *player_id)
                    .map(|player| player.position.get())
                    .ok_or(InstallReject::WorldChanged)?;
                let current = world
                    .online_players
                    .get(player_id)
                    .copied()
                    .ok_or(InstallReject::WorldChanged)?;
                if frozen != current {
                    return Err(InstallReject::WorldChanged);
                }
            }
        }
    }
    Ok(())
}

/// Checks the planned target chunk revision is present and unchanged on both
/// sides; a missing entry fails closed.
fn check_revision(
    slot: &PlanSlot,
    world: &CurrentWorld,
    target: BlockPos,
) -> Result<(), InstallReject> {
    let chunk = target_chunk(target);
    let frozen = slot
        .frozen
        .chunk_revisions
        .iter()
        .find(|revision| revision.pos == chunk)
        .map(|revision| revision.revision)
        .ok_or(InstallReject::WorldChanged)?;
    let current = world
        .chunk_revisions
        .get(&chunk)
        .copied()
        .ok_or(InstallReject::WorldChanged)?;
    if frozen != current {
        return Err(InstallReject::WorldChanged);
    }
    Ok(())
}

/// Emits the first-step action for a validated plan: mine holds, places,
/// and clamped single-step moves toward go-to and follow targets. This is
/// the task runner's emission site; the envelopes it builds are what the
/// companion action provider admits.
fn first_action(slot: &PlanSlot, tick: u64, plan: &AgentPlan) -> Option<CompanionActionEnvelope> {
    let first = plan.steps.first()?;
    let action = match first {
        PlanStep::Mine { x, y, z } => CompanionAction::MineHold {
            target: BlockPos::new(*x, *y, *z),
        },
        PlanStep::Place { x, y, z, block } => CompanionAction::Place {
            target: BlockPos::new(*x, *y, *z),
            block: plan_block_id(*block),
        },
        PlanStep::GoTo { x, y: _, z } => {
            let position = slot.frozen.companion.position.get();
            CompanionAction::Move {
                move_x: (*x as f32 - position[0]).signum() as i8,
                move_z: (*z as f32 - position[2]).signum() as i8,
                jump: false,
                yaw: 0.0,
            }
        }
        PlanStep::Follow { player_id } => {
            let target = slot
                .frozen
                .online_players
                .iter()
                .find(|player| player.player_id == *player_id)?;
            let goal = target.position.get();
            let position = slot.frozen.companion.position.get();
            CompanionAction::Move {
                move_x: (goal[0] - position[0]).signum() as i8,
                move_z: (goal[2] - position[2]).signum() as i8,
                jump: false,
                yaw: 0.0,
            }
        }
    };
    CompanionActionEnvelope::try_new(
        slot.companion,
        tick,
        slot.request_id,
        slot.run_id,
        slot.snapshot_id,
        slot.generation,
        slot.attempt,
        slot.digest,
        action,
    )
    .ok()
}

/// Cancels one admitted run within the independent failure budget.
///
/// The cancel carries the frozen lease of the failed request; a best-effort
/// attempt that never blocks the caller past the budget.
pub(crate) fn cancel_admitted_run(
    agent: &mut dyn AgentHandle,
    clock: &dyn Clock,
    leased: LeasedIdentity,
    run_id: RunId,
) {
    let budget_until = clock.monotonic() + CANCEL_RUN_TIMEOUT;
    let Ok(request_id) = agent.submit(AgentRequest::Cancel(CancelRequest { leased, run_id }))
    else {
        return;
    };
    let deadline = Deadline::at(budget_until);
    loop {
        match agent.poll(request_id) {
            AgentPoll::Completed(_) | AgentPoll::Failed(_) => return,
            AgentPoll::Pending => {}
        }
        if clock.monotonic() >= deadline.instant() || Instant::now() >= budget_until {
            return;
        }
        std::thread::sleep(Duration::from_millis(1));
    }
}
