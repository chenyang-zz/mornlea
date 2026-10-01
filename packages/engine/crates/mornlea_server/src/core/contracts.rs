//! Checked server contracts.
//!
//! Field names and constructor ceilings come from the authoritative-server
//! execution and seam plans. A constructor that rejects a value does so before
//! it allocates. These types are not wire records and not save records.

use std::num::NonZeroU64;
use std::time::{Duration, Instant};

use mornlea_domain::{
    BlockPos, ChunkPos, CommandEnvelope, CommandText, CompanionId, ContainerRef, CraftingSize,
    Dimension, DropId, FiniteVec3, HostileId, HotbarSlot, LookAngles, MotionState, PassiveId,
    PlayerControl, PlayerId, ProjectileId, ProjectileKind, RejectReason, RoutedEvent,
    SurvivalState, Weather, WorldState, registered_block,
};
use mornlea_engine::native::contracts::PhysicsTuning;
use mornlea_protocol::{AdmittedLogin, PlayIntent, ServerPacket};
use mornlea_storage::{
    Chunk, ChunkSave, CompanionBody, CompanionSave, HostileMob, HostileMobs, HostileMobsSave,
    ItemStack, Metadata, PassiveMob, PassiveMobs, PassiveMobsSave, PlayerSave, StoredCompanionTask,
    StoredCompanions, StoredPlayer,
};

const MAX_PLAYERS: u8 = 8;
const MAX_QUEUED_COMMANDS: usize = 4096;
const MAX_SESSION_OUTBOX: usize = 512;
const MAX_READY_CHUNK_RESULTS: usize = 64;
const MAX_SNAPSHOT_CHUNKS: usize = 64;
const MAX_SNAPSHOT_BYTES: usize = 1_048_576;
const MAX_FLUID_UPDATES: usize = 512;
const MAX_FLUID_RESCAN_TARGET: usize = 65_536;
const MAX_FARMLAND: usize = 65_536;
const MAX_EFFECTS: usize = 4096;
const RESCAN_SECTION_CELLS: usize = 4096;
const RESCAN_OVERSHOOT: usize = 4095;
const MAX_STORE_WORKERS: usize = 2;
const MAX_PLAYER_SNAPSHOTS: usize = 16;
const MAX_COMPANION_SNAPSHOTS: usize = 3;
const MAX_HOSTILE_SNAPSHOTS: usize = 3;
const MAX_PASSIVE_SNAPSHOTS: usize = 3;
const MAX_METADATA_SNAPSHOTS: usize = 1;
const MAX_OWNED_CHUNKS: usize = 8;
const MAX_OWNED_ENCODED_BYTES: usize = 4_194_304;
const MAX_SAVE_JOBS: usize = 4;
const DEFAULT_SAVE_CHUNKS: usize = 8;
const DEFAULT_SAVE_BYTES: usize = 4_194_304;
const MAX_DROP_STACKS: usize = 36;
const MAX_PROJECTILES: usize = 128;
const MAX_SLEEP_BEDS: usize = 8;
const MAX_DROP_LIFETIME: u32 = 120_000;
const MAX_RANDOM_ATTEMPTS: u8 = 64;
const MAX_CROP_GROWTH_PERCENT: u8 = 100;
const MAX_FURNACE_SMELT: u8 = 200;
const MAX_FURNACE_BURN: u16 = 1600;
const TERRAIN_X: u8 = 33;
const TERRAIN_Y: u8 = 17;
const TERRAIN_Z: u8 = 33;
const TERRAIN_READY_COLUMNS: usize = 137;
const TERRAIN_HEIGHTS: usize = 1089;
const TERRAIN_BLOCKS: usize = 18_513;
const PLAN_STEP_LIMIT: usize = 5000;
const BLOCK_Y_MIN: i32 = -64;
const BLOCK_Y_MAX: i32 = 319;

/// Process-local session identity. Values are nonzero, monotonic, and never reused.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct SessionKey(NonZeroU64);

impl SessionKey {
    pub(crate) fn from_raw(raw: u64) -> Option<Self> {
        NonZeroU64::new(raw).map(Self)
    }

    pub fn get(self) -> u64 {
        self.0.get()
    }
}

/// Nonzero save-ticket identity allocated by the store mailbox.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct SaveTicket(NonZeroU64);

impl SaveTicket {
    pub fn try_from_raw(raw: u64) -> Result<Self, ServerError> {
        NonZeroU64::new(raw)
            .map(Self)
            .ok_or(ServerError::InvalidInput {
                field: "save_ticket",
            })
    }

    pub fn get(self) -> u64 {
        self.0.get()
    }
}

/// Nonzero chunk-request identity.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct ChunkRequestId(NonZeroU64);

impl ChunkRequestId {
    pub fn try_new(raw: u64) -> Result<Self, ServerError> {
        NonZeroU64::new(raw)
            .map(Self)
            .ok_or(ServerError::InvalidInput {
                field: "chunk_request",
            })
    }

    pub fn get(self) -> u64 {
        self.0.get()
    }
}

/// Nonzero connection identity reserved before login.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct ConnectionId(NonZeroU64);

impl ConnectionId {
    pub fn try_from_raw(raw: u64) -> Result<Self, ServerError> {
        NonZeroU64::new(raw)
            .map(Self)
            .ok_or(ServerError::InvalidInput {
                field: "connection_id",
            })
    }

    pub fn get(self) -> u64 {
        self.0.get()
    }
}

/// Login ticket pairing a prepared session with one load.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct LoginTicket(NonZeroU64);

impl LoginTicket {
    pub fn try_from_raw(raw: u64) -> Result<Self, ServerError> {
        NonZeroU64::new(raw)
            .map(Self)
            .ok_or(ServerError::InvalidInput {
                field: "login_ticket",
            })
    }

    pub fn get(self) -> u64 {
        self.0.get()
    }
}

macro_rules! uuid_id {
    ($name:ident, $field:literal) => {
        #[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
        pub struct $name([u8; 16]);

        impl $name {
            pub fn try_from_bytes(bytes: [u8; 16]) -> Result<Self, ServerError> {
                if bytes == [0; 16] || bytes[6] >> 4 != 4 || bytes[8] & 0xc0 != 0x80 {
                    return Err(ServerError::InvalidInput { field: $field });
                }
                Ok(Self(bytes))
            }

            pub fn bytes(self) -> [u8; 16] {
                self.0
            }
        }
    };
}

uuid_id!(AgentRequestId, "agent_request_id");
uuid_id!(RunId, "run_id");
uuid_id!(SnapshotId, "snapshot_id");
uuid_id!(LeaseId, "lease_id");
uuid_id!(NamespaceId, "namespace_id");
uuid_id!(ClientInstanceId, "client_instance_id");
uuid_id!(OperationId, "operation_id");

/// HTTP contract version written by the agent codec. It is not a stored field.
pub const AGENT_CONTRACT_VERSION: &str = "v1";
/// Lease TTL written by the agent codec. It is not a stored field.
pub const LEASE_EXPIRES_IN_MS: u64 = 15_000;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TransportKind {
    Memory,
    Tcp,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ServerPhase {
    Running,
    Closing,
    Closed,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SessionPhase {
    Prepared,
    Active,
    Retired,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Resource {
    Players,
    Commands,
    Arrivals,
    ChunkResults,
    Outbox,
    PendingLogins,
    SaveChunks,
    SaveBytes,
    AgentRuns,
    Snapshots,
    RuleEffects,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Operation {
    Load,
    WritePayload,
    SyncPayload,
    WriteBank,
    SyncBank,
    Replace,
    DirectorySync,
    Flush,
    Sync,
    ReleaseAgent,
    Close,
    Shutdown,
    AgentRpc,
    Transport,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AgentErrorCode {
    InvalidRequest,
    Unauthorized,
    UnsupportedVersion,
    NamespaceConflict,
    Overloaded,
    DeadlineExceeded,
    AgentUnavailable,
    InvalidModelOutput,
    MemoryConflict,
    NotFound,
    InternalError,
}

impl AgentErrorCode {
    pub fn status(self) -> u16 {
        match self {
            Self::InvalidRequest => 400,
            Self::Unauthorized => 401,
            Self::UnsupportedVersion => 426,
            Self::NamespaceConflict | Self::MemoryConflict => 409,
            Self::Overloaded => 429,
            Self::DeadlineExceeded => 504,
            Self::AgentUnavailable => 503,
            Self::InvalidModelOutput => 422,
            Self::NotFound => 404,
            Self::InternalError => 500,
        }
    }
}

/// Stable codec failure classes; descriptive codec text is not a wire version.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StorageFailure {
    Corrupt,
    FutureVersion,
    OutputTooSmall,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ServerError {
    InvalidInput {
        field: &'static str,
    },
    IncompatibleVersion {
        family: &'static str,
        found: u32,
    },
    InvalidState {
        phase: ServerPhase,
    },
    StaleSession {
        session: SessionKey,
    },
    Capacity {
        resource: Resource,
        limit: usize,
        observed: usize,
    },
    Timeout {
        operation: Operation,
    },
    Cancelled,
    Disconnected,
    Io {
        operation: Operation,
        kind: std::io::ErrorKind,
    },
    Storage {
        family: &'static str,
        kind: StorageFailure,
    },
    Agent {
        code: AgentErrorCode,
        status: u16,
    },
    Internal {
        invariant: &'static str,
    },
}

impl ServerError {
    pub fn agent(code: AgentErrorCode, status: u16) -> Result<Self, Self> {
        if code.status() != status {
            return Err(Self::InvalidInput {
                field: "agent_status",
            });
        }
        Ok(Self::Agent { code, status })
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RuleReject {
    Wire(RejectReason),
    StaleObservation,
    DuplicateProvenance,
    CancelledGeneration,
    ResourceFull(Resource),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CloseReason {
    PeerGone,
    InvalidPlay,
    HeartbeatMismatch,
    HeartbeatTimeout,
    SlowReceiver,
    Capacity,
    Shutdown,
}

/// Accepted constructor ceilings: 8 / 4096 / 512 / 64 / 64 / 1048576.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ServerLimits {
    max_players: u8,
    queued_commands: usize,
    session_outbox: usize,
    ready_chunk_results: usize,
    snapshot_chunks: usize,
    snapshot_bytes: usize,
}

impl ServerLimits {
    pub fn try_new(
        max_players: u8,
        queued_commands: usize,
        session_outbox: usize,
        ready_chunk_results: usize,
        snapshot_chunks: usize,
        snapshot_bytes: usize,
    ) -> Result<Self, ServerError> {
        if !(1..=MAX_PLAYERS).contains(&max_players) {
            return Err(capacity(
                Resource::Players,
                MAX_PLAYERS as usize,
                max_players as usize,
            ));
        }
        check_ceiling(Resource::Commands, queued_commands, MAX_QUEUED_COMMANDS)?;
        check_ceiling(Resource::Outbox, session_outbox, MAX_SESSION_OUTBOX)?;
        check_ceiling(
            Resource::ChunkResults,
            ready_chunk_results,
            MAX_READY_CHUNK_RESULTS,
        )?;
        check_ceiling(Resource::Snapshots, snapshot_chunks, MAX_SNAPSHOT_CHUNKS)?;
        check_ceiling(Resource::SaveBytes, snapshot_bytes, MAX_SNAPSHOT_BYTES)?;
        Ok(Self {
            max_players,
            queued_commands,
            session_outbox,
            ready_chunk_results,
            snapshot_chunks,
            snapshot_bytes,
        })
    }

    pub fn max_players(self) -> u8 {
        self.max_players
    }
    pub fn queued_commands(self) -> usize {
        self.queued_commands
    }
    pub fn session_outbox(self) -> usize {
        self.session_outbox
    }
    pub fn ready_chunk_results(self) -> usize {
        self.ready_chunk_results
    }
    pub fn snapshot_chunks(self) -> usize {
        self.snapshot_chunks
    }
    pub fn snapshot_bytes(self) -> usize {
        self.snapshot_bytes
    }
}

/// Zero work is legal. Ceilings are 4096 / 512 / 65536 / 65536 / 65536.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TickBudget {
    commands: usize,
    fluid_updates_per_dimension: usize,
    fluid_rescan_target_per_dimension: usize,
    farmland_checks: usize,
    farmland_block_reads: usize,
}

impl TickBudget {
    pub fn try_new(
        commands: usize,
        fluid_updates_per_dimension: usize,
        fluid_rescan_target_per_dimension: usize,
        farmland_checks: usize,
        farmland_block_reads: usize,
    ) -> Result<Self, ServerError> {
        check_ceiling(Resource::Commands, commands, MAX_QUEUED_COMMANDS)?;
        check_ceiling(
            Resource::Commands,
            fluid_updates_per_dimension,
            MAX_FLUID_UPDATES,
        )?;
        check_ceiling(
            Resource::Commands,
            fluid_rescan_target_per_dimension,
            MAX_FLUID_RESCAN_TARGET,
        )?;
        check_ceiling(Resource::Commands, farmland_checks, MAX_FARMLAND)?;
        check_ceiling(Resource::Commands, farmland_block_reads, MAX_FARMLAND)?;
        Ok(Self {
            commands,
            fluid_updates_per_dimension,
            fluid_rescan_target_per_dimension,
            farmland_checks,
            farmland_block_reads,
        })
    }

    pub fn full() -> Self {
        Self {
            commands: MAX_QUEUED_COMMANDS,
            fluid_updates_per_dimension: MAX_FLUID_UPDATES,
            fluid_rescan_target_per_dimension: MAX_FLUID_RESCAN_TARGET,
            farmland_checks: MAX_FARMLAND,
            farmland_block_reads: MAX_FARMLAND,
        }
    }

    pub fn commands(self) -> usize {
        self.commands
    }
    pub fn fluid_updates_per_dimension(self) -> usize {
        self.fluid_updates_per_dimension
    }
    pub fn fluid_rescan_target_per_dimension(self) -> usize {
        self.fluid_rescan_target_per_dimension
    }
    pub fn farmland_checks(self) -> usize {
        self.farmland_checks
    }
    pub fn farmland_block_reads(self) -> usize {
        self.farmland_block_reads
    }
}

/// Per-lane snapshot ceilings: workers 2, players 16, companions 3, hostiles 3,
/// passives 3, metadata 1, chunks 8, encoded bytes 4194304.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StoreLimits {
    workers: usize,
    player_snapshots: usize,
    companion_snapshots: usize,
    hostile_snapshots: usize,
    passive_snapshots: usize,
    metadata_snapshots: usize,
    max_owned_chunks: usize,
    max_owned_encoded_bytes: usize,
}

impl StoreLimits {
    // The eight lane ceilings are the frozen S1 store contract record; grouping
    // them would hide which ceiling each constructor position names.
    #[allow(clippy::too_many_arguments)]
    pub fn try_new(
        workers: usize,
        player_snapshots: usize,
        companion_snapshots: usize,
        hostile_snapshots: usize,
        passive_snapshots: usize,
        metadata_snapshots: usize,
        max_owned_chunks: usize,
        max_owned_encoded_bytes: usize,
    ) -> Result<Self, ServerError> {
        check_ceiling(Resource::Snapshots, workers, MAX_STORE_WORKERS)?;
        check_ceiling(Resource::Snapshots, player_snapshots, MAX_PLAYER_SNAPSHOTS)?;
        check_ceiling(
            Resource::Snapshots,
            companion_snapshots,
            MAX_COMPANION_SNAPSHOTS,
        )?;
        check_ceiling(
            Resource::Snapshots,
            hostile_snapshots,
            MAX_HOSTILE_SNAPSHOTS,
        )?;
        check_ceiling(
            Resource::Snapshots,
            passive_snapshots,
            MAX_PASSIVE_SNAPSHOTS,
        )?;
        check_ceiling(
            Resource::Snapshots,
            metadata_snapshots,
            MAX_METADATA_SNAPSHOTS,
        )?;
        check_ceiling(Resource::SaveChunks, max_owned_chunks, MAX_OWNED_CHUNKS)?;
        check_ceiling(
            Resource::SaveBytes,
            max_owned_encoded_bytes,
            MAX_OWNED_ENCODED_BYTES,
        )?;
        Ok(Self {
            workers,
            player_snapshots,
            companion_snapshots,
            hostile_snapshots,
            passive_snapshots,
            metadata_snapshots,
            max_owned_chunks,
            max_owned_encoded_bytes,
        })
    }

    pub fn workers(self) -> usize {
        self.workers
    }
    pub fn player_snapshots(self) -> usize {
        self.player_snapshots
    }
    pub fn companion_snapshots(self) -> usize {
        self.companion_snapshots
    }
    pub fn hostile_snapshots(self) -> usize {
        self.hostile_snapshots
    }
    pub fn passive_snapshots(self) -> usize {
        self.passive_snapshots
    }
    pub fn metadata_snapshots(self) -> usize {
        self.metadata_snapshots
    }
    pub fn max_owned_chunks(self) -> usize {
        self.max_owned_chunks
    }
    pub fn max_owned_encoded_bytes(self) -> usize {
        self.max_owned_encoded_bytes
    }

    pub fn lane_cap(self, key: &SaveKey) -> usize {
        match key {
            SaveKey::Player(_) => self.player_snapshots,
            SaveKey::Companions => self.companion_snapshots,
            SaveKey::Hostiles => self.hostile_snapshots,
            SaveKey::Passives => self.passive_snapshots,
            SaveKey::Metadata => self.metadata_snapshots,
            SaveKey::Chunk(_) => self.max_owned_chunks,
        }
    }

    /// Returns the occupancy after admitting every snapshot, or the original
    /// occupancy's error without changing counts. A non-chunk lane does not
    /// consume `max_owned_chunks`.
    pub fn try_admit(
        self,
        occupancy: &SaveOccupancy,
        request: &SaveRequest,
    ) -> Result<SaveOccupancy, ServerError> {
        self.admit_lengths(
            occupancy,
            request,
            request
                .snapshots
                .iter()
                .map(|snapshot| snapshot.estimated_bytes),
        )
    }

    /// Uses codec reservations without cloning snapshot bodies or changing caller estimates.
    pub(crate) fn try_admit_reserved(
        self,
        occupancy: &SaveOccupancy,
        request: &SaveRequest,
        reservations: &[usize],
    ) -> Result<SaveOccupancy, ServerError> {
        if request.snapshots.len() != reservations.len() {
            return Err(ServerError::Internal {
                invariant: "store reservation length",
            });
        }
        self.admit_lengths(occupancy, request, reservations.iter().copied())
    }

    fn admit_lengths(
        self,
        occupancy: &SaveOccupancy,
        request: &SaveRequest,
        lengths: impl Iterator<Item = usize>,
    ) -> Result<SaveOccupancy, ServerError> {
        if occupancy.jobs >= MAX_SAVE_JOBS {
            return Err(capacity(
                Resource::Snapshots,
                MAX_SAVE_JOBS,
                occupancy.jobs + 1,
            ));
        }
        let mut next = occupancy.clone();
        next.jobs += 1;
        for (snapshot, length) in request.snapshots.iter().zip(lengths) {
            let cap = self.lane_cap(&snapshot.key);
            let slot = next.lane_mut(&snapshot.key);
            let observed = slot.saturating_add(1);
            if observed > cap {
                let resource = if matches!(snapshot.key, SaveKey::Chunk(_)) {
                    Resource::SaveChunks
                } else {
                    Resource::Snapshots
                };
                return Err(capacity(resource, cap, observed));
            }
            *slot = observed;
            next.encoded_bytes =
                next.encoded_bytes
                    .checked_add(length)
                    .ok_or(ServerError::InvalidInput {
                        field: "estimated_bytes",
                    })?;
            if next.encoded_bytes > self.max_owned_encoded_bytes {
                return Err(capacity(
                    Resource::SaveBytes,
                    self.max_owned_encoded_bytes,
                    next.encoded_bytes,
                ));
            }
        }
        Ok(next)
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct SaveOccupancy {
    pub jobs: usize,
    pub players: usize,
    pub companions: usize,
    pub hostiles: usize,
    pub passives: usize,
    pub metadata: usize,
    pub chunks: usize,
    pub encoded_bytes: usize,
}

impl SaveOccupancy {
    fn lane_mut(&mut self, key: &SaveKey) -> &mut usize {
        match key {
            SaveKey::Player(_) => &mut self.players,
            SaveKey::Companions => &mut self.companions,
            SaveKey::Hostiles => &mut self.hostiles,
            SaveKey::Passives => &mut self.passives,
            SaveKey::Metadata => &mut self.metadata,
            SaveKey::Chunk(_) => &mut self.chunks,
        }
    }

    pub fn chunks(&self) -> usize {
        self.chunks
    }
}

fn check_ceiling(resource: Resource, observed: usize, limit: usize) -> Result<(), ServerError> {
    if observed > limit {
        Err(capacity(resource, limit, observed))
    } else {
        Ok(())
    }
}

fn capacity(resource: Resource, limit: usize, observed: usize) -> ServerError {
    ServerError::Capacity {
        resource,
        limit,
        observed,
    }
}

/// Monotonic deadline. Wall time cannot build or extend one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Deadline(Instant);

impl Deadline {
    pub fn at(instant: Instant) -> Self {
        Self(instant)
    }

    pub fn after(now: Instant, timeout: Duration) -> Result<Self, ServerError> {
        now.checked_add(timeout)
            .map(Self)
            .ok_or(ServerError::InvalidInput { field: "deadline" })
    }

    pub fn expired(self, now: Instant) -> bool {
        now >= self.0
    }

    pub fn instant(self) -> Instant {
        self.0
    }
}

pub trait Clock {
    fn monotonic(&self) -> Instant;
    fn unix_ms(&self) -> i64;
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SubmissionReceipt {
    QueuedForTick { tick: u64, arrival_index: u64 },
    ControlAccepted,
}

/// Admission receipt for one sessionless companion action.
///
/// The seam plans name this receipt and do not list a second field set, so it
/// carries the same earliest-tick and arrival identity as a queued command.
/// It is not a protocol record.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CompanionReceipt {
    tick: u64,
    arrival_index: u64,
}

impl CompanionReceipt {
    pub(crate) fn new(tick: u64, arrival_index: u64) -> Self {
        Self {
            tick,
            arrival_index,
        }
    }

    pub fn tick(self) -> u64 {
        self.tick
    }

    pub fn arrival_index(self) -> u64 {
        self.arrival_index
    }
}

pub trait ServerEndpoint {
    fn admit(
        &mut self,
        login: AdmittedLogin,
        transport: TransportKind,
    ) -> Result<SessionKey, ServerError>;
    fn submit(
        &mut self,
        session: SessionKey,
        intent: PlayIntent,
    ) -> Result<SubmissionReceipt, ServerError>;
    fn submit_companion(
        &mut self,
        candidate: CompanionActionEnvelope,
    ) -> Result<CompanionReceipt, ServerError>;
    fn advance_tick(&mut self, work: TickBudget) -> Result<TickPublication, ServerError>;
    fn close_session(
        &mut self,
        session: SessionKey,
        reason: CloseReason,
    ) -> Result<(), ServerError>;
    fn shutdown(&mut self, deadline: Deadline) -> Result<ShutdownReport, ShutdownFailure>;
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SessionFacts {
    pub player_id: PlayerId,
    pub phase: SessionPhase,
    pub last_applied_sequence: u64,
    pub next_arrival: u64,
}

/// Server-local chunk key. The storage region key is a different codec type.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct ChunkKey {
    pub dimension: Dimension,
    pub pos: ChunkPos,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ChunkResult {
    pub request: ChunkRequestId,
    pub key: ChunkKey,
    pub generation: u64,
    pub result: Result<Chunk, ServerError>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ControlReply {
    pub session: SessionKey,
    pub packet: ServerPacket,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct TickCounters {
    pub executed_tick: u64,
    pub commands: usize,
    pub fluid_by_dimension: Vec<(Dimension, usize)>,
    pub rescan_by_dimension: Vec<(Dimension, usize)>,
    pub farmland_checks: usize,
    pub farmland_reads: usize,
    pub carried: usize,
    pub stale: usize,
}

#[derive(Clone, Debug, PartialEq)]
pub struct TickPublication {
    pub tick: u64,
    pub events: Vec<RoutedEvent>,
    pub control: Vec<ControlReply>,
    pub counters: TickCounters,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SaveKey {
    Chunk(ChunkKey),
    Player(PlayerId),
    Companions,
    Hostiles,
    Passives,
    Metadata,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FrozenAuthority {
    pub final_tick: u64,
    pub save_keys: Vec<SaveKey>,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum ActorKey {
    Player(SessionKey),
    Companion(CompanionId),
    Hostile(HostileId),
    Passive(PassiveId),
}

impl ActorKey {
    pub fn inventory_actor(self) -> bool {
        matches!(self, Self::Player(_) | Self::Companion(_))
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ActorLifecycle {
    Pending,
    Active,
    Respawning,
    Dead,
}

#[derive(Clone, Debug, PartialEq)]
pub enum ActorBody {
    Player(PlayerSave),
    Companion(CompanionBody),
    Hostile(HostileMob),
    Passive(PassiveMob),
}

#[derive(Clone, Debug, PartialEq)]
pub struct ActorRecord {
    pub key: ActorKey,
    pub lifecycle: ActorLifecycle,
    pub dimension: Dimension,
    pub motion: mornlea_domain::MotionState,
    pub look: LookAngles,
    pub survival: SurvivalState,
    pub body: ActorBody,
}

impl ActorRecord {
    pub fn try_new(
        key: ActorKey,
        lifecycle: ActorLifecycle,
        dimension: Dimension,
        motion: mornlea_domain::MotionState,
        look: LookAngles,
        survival: SurvivalState,
        body: ActorBody,
    ) -> Result<Self, ServerError> {
        let matches = matches!(
            (&key, &body),
            (ActorKey::Player(_), ActorBody::Player(_))
                | (ActorKey::Companion(_), ActorBody::Companion(_))
                | (ActorKey::Hostile(_), ActorBody::Hostile(_))
                | (ActorKey::Passive(_), ActorBody::Passive(_))
        );
        if !matches {
            return Err(ServerError::InvalidInput { field: "actor" });
        }
        Ok(Self {
            key,
            lifecycle,
            dimension,
            motion,
            look,
            survival,
            body,
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InventoryRecord {
    pub slots: [ItemStack; 36],
    pub selected: HotbarSlot,
    pub armor: [ItemStack; 4],
    pub crafting: [ItemStack; 9],
    pub crafting_size: CraftingSize,
}

impl InventoryRecord {
    pub fn empty() -> Self {
        Self {
            slots: [ItemStack::default(); 36],
            selected: HotbarSlot::new(0).expect("slot zero"),
            armor: [ItemStack::default(); 4],
            crafting: [ItemStack::default(); 9],
            crafting_size: CraftingSize::Personal,
        }
    }

    pub fn try_new(
        slots: [ItemStack; 36],
        selected: HotbarSlot,
        armor: [ItemStack; 4],
        crafting: [ItemStack; 9],
        crafting_size: CraftingSize,
    ) -> Result<Self, ServerError> {
        if slots
            .iter()
            .chain(crafting.iter())
            .any(|stack| !stack.is_valid())
        {
            return Err(ServerError::InvalidInput { field: "inventory" });
        }
        Ok(Self {
            slots,
            selected,
            armor,
            crafting,
            crafting_size,
        })
    }

    pub fn with_selected(mut self, selected: HotbarSlot) -> Self {
        self.selected = selected;
        self
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InventoryPatch {
    pub actor: ActorKey,
    pub before: InventoryRecord,
    pub after: InventoryRecord,
}

impl InventoryPatch {
    pub fn try_new(
        actor: ActorKey,
        before: InventoryRecord,
        after: InventoryRecord,
    ) -> Result<Self, RuleReject> {
        if !actor.inventory_actor() {
            return Err(RuleReject::Wire(RejectReason::InvalidInput));
        }
        Ok(Self {
            actor,
            before,
            after,
        })
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum ContainerSlots {
    Chest([ItemStack; 27]),
    Furnace {
        slots: [ItemStack; 3],
        fuel: u32,
        progress: u32,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub struct ContainerRecord {
    pub reference: ContainerRef,
    pub revision: u64,
    pub slots: ContainerSlots,
}

/// Internal chunk ownership retains dimension independently of a wire view reference.
#[derive(Clone, Debug, PartialEq)]
pub struct CapturedContainer {
    pub key: ChunkKey,
    pub record: ContainerRecord,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ViewLease {
    session: SessionKey,
    reference: ContainerRef,
}

impl ViewLease {
    pub fn new(session: SessionKey, reference: ContainerRef) -> Self {
        Self { session, reference }
    }

    pub fn session(self) -> SessionKey {
        self.session
    }

    pub fn reference(self) -> ContainerRef {
        self.reference
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct MiningProgress {
    pub actor: ActorKey,
    pub dimension: Dimension,
    pub target: BlockPos,
    pub observed_block: u16,
    pub tool_slot: HotbarSlot,
    pub tool: ItemStack,
    pub elapsed: u32,
    pub required: u32,
    pub last_tick: u64,
}

/// A tick-local internal melee choice. Saved chase UUIDs cannot substitute
/// for this exact process-local target session.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct HostileMeleeAttack {
    attacker: HostileId,
    target: SessionKey,
}

impl HostileMeleeAttack {
    pub fn new(attacker: HostileId, target: SessionKey) -> Self {
        Self { attacker, target }
    }
    pub fn attacker(self) -> HostileId {
        self.attacker
    }
    pub fn target(self) -> SessionKey {
        self.target
    }
}

/// Immutable choices for one authority tick, bounded by the hostile population.
/// The producer resolves duplicate actions before construction; combat still
/// owns live identity, kind, dimension, cooldown and post-motion range checks.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HostileMeleeBatch {
    tick: u64,
    entries: Vec<HostileMeleeAttack>,
}

impl HostileMeleeBatch {
    pub fn try_new(tick: u64, entries: &[HostileMeleeAttack]) -> Result<Self, ServerError> {
        const LIMIT: usize = 64;
        if entries.len() > LIMIT {
            return Err(ServerError::Capacity {
                resource: Resource::RuleEffects,
                limit: LIMIT,
                observed: entries.len(),
            });
        }
        // The fixed population bound keeps this validation allocation-free;
        // rejected duplicate policies cannot leave a partially owned batch.
        for (index, entry) in entries.iter().enumerate() {
            if entries[..index]
                .iter()
                .any(|prior| prior.attacker == entry.attacker)
            {
                return Err(ServerError::InvalidInput {
                    field: "hostile_melee",
                });
            }
        }
        let mut owned = entries.to_vec();
        owned.sort_unstable_by_key(|entry| entry.attacker());
        Ok(Self {
            tick,
            entries: owned,
        })
    }
    pub fn tick(&self) -> u64 {
        self.tick
    }
    pub fn entries(&self) -> &[HostileMeleeAttack] {
        &self.entries
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DamageCause {
    Melee,
    Projectile,
    Fall,
    Drowning,
    Hunger,
    Fire,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DamageIntent {
    pub source: ActorKey,
    pub target: ActorKey,
    pub dimension: Dimension,
    pub amount: u8,
    pub cause: DamageCause,
    pub projectile: Option<ProjectileId>,
    pub tick: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DropSource {
    System {
        rule: SystemRule,
        tick: u64,
        target: BlockPos,
    },
    Mining {
        actor: ActorKey,
        target: BlockPos,
        tick: u64,
    },
    Death {
        actor: ActorKey,
        tick: u64,
    },
    Panel {
        session: SessionKey,
        sequence: u64,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub struct DropBatch {
    pub source: DropSource,
    pub dimension: Dimension,
    pub origin: FiniteVec3,
    pub stacks: Vec<ItemStack>,
    pub pickup_delay: u8,
}

impl DropBatch {
    pub fn try_new(
        source: DropSource,
        dimension: Dimension,
        origin: FiniteVec3,
        stacks: Vec<ItemStack>,
        pickup_delay: u8,
    ) -> Result<Self, RuleReject> {
        if stacks.len() > MAX_DROP_STACKS || stacks.iter().any(|stack| !stack.is_valid()) {
            return Err(RuleReject::ResourceFull(Resource::RuleEffects));
        }
        Ok(Self {
            source,
            dimension,
            origin,
            stacks,
            pickup_delay,
        })
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct DropRecord {
    pub id: DropId,
    pub position: FiniteVec3,
    pub stack: ItemStack,
    pub pickup_delay: u8,
    pub age: u32,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ProjectileRecord {
    pub id: ProjectileId,
    pub owner: ActorKey,
    pub dimension: Dimension,
    pub position: FiniteVec3,
    pub velocity: FiniteVec3,
    pub kind: ProjectileKind,
    pub damage: u8,
    pub age: u32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SleepState {
    pub beds: Vec<(SessionKey, Dimension, BlockPos)>,
    pub day_phase_offset: u64,
    pub pending_offset: Option<u64>,
}

impl SleepState {
    pub fn try_new(
        beds: Vec<(SessionKey, Dimension, BlockPos)>,
        day_phase_offset: u64,
        pending_offset: Option<u64>,
    ) -> Result<Self, RuleReject> {
        if beds.len() > MAX_SLEEP_BEDS {
            return Err(RuleReject::ResourceFull(Resource::Players));
        }
        Ok(Self {
            beds,
            day_phase_offset,
            pending_offset,
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FluidWork {
    pub key: ChunkKey,
    pub pos: BlockPos,
    pub due_tick: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RescanWork {
    pub key: ChunkKey,
    pub section_y: i32,
    pub next_cell: u16,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FarmlandWork {
    pub key: ChunkKey,
    pub pos: BlockPos,
    pub due_tick: u64,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct WorkState {
    pub fluid: Vec<FluidWork>,
    pub rescans: Vec<RescanWork>,
    pub farmland: Vec<FarmlandWork>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WorkKind {
    Commands,
    FluidUpdates(Dimension),
    RescanCells(Dimension),
    FarmlandChecks,
    FarmlandReads,
    Effects,
    SnapshotChunks,
    SnapshotBytes,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BlockObservation {
    pub key: ChunkKey,
    pub generation: u64,
    pub revision: u64,
    pub pos: BlockPos,
    pub block: u16,
}

impl BlockObservation {
    pub fn try_new(
        key: ChunkKey,
        generation: u64,
        revision: u64,
        pos: BlockPos,
        block: u16,
    ) -> Result<Self, RuleReject> {
        if !registered_block(block) {
            return Err(RuleReject::Wire(RejectReason::InvalidBlock));
        }
        Ok(Self {
            key,
            generation,
            revision,
            pos,
            block,
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BlockWrite {
    pub observed: BlockObservation,
    pub replacement: u16,
}

impl BlockWrite {
    pub fn try_new(observed: BlockObservation, replacement: u16) -> Result<Self, RuleReject> {
        if !registered_block(replacement) {
            return Err(RuleReject::Wire(RejectReason::InvalidBlock));
        }
        Ok(Self {
            observed,
            replacement,
        })
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SystemRule {
    Fluid,
    Farmland,
    RandomBlock,
    Support,
    Footprint,
    PassiveGraze(PassiveId),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MutationProducer {
    Actor(ActorKey),
    System(SystemRule),
}

/// Opaque resolver preimages include read-only geometry, not just written cells.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct MutationReadBasis {
    pub actor: ActorKey,
    pub dimension: Dimension,
    pub motion: MotionState,
    pub look: LookAngles,
    pub seed: i64,
    pub tunables: RuleTunables,
    pub cells: Vec<(Dimension, BlockPos, Option<BlockObservation>)>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct BlockTxn {
    pub(crate) producer: MutationProducer,
    pub(crate) tick: u64,
    pub(crate) read_basis: Option<MutationReadBasis>,
    pub(crate) writes: Vec<BlockWrite>,
    pub(crate) inventory: Option<InventoryPatch>,
    pub(crate) containers: Vec<CapturedContainer>,
    pub(crate) drops: Option<DropBatch>,
    pub(crate) mining: Option<MiningProgress>,
}

impl BlockTxn {
    pub(crate) fn system(producer: SystemRule, tick: u64, writes: Vec<BlockWrite>) -> Self {
        Self {
            producer: MutationProducer::System(producer),
            tick,
            read_basis: None,
            writes,
            inventory: None,
            containers: Vec::new(),
            drops: None,
            mining: None,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct ResolvedPlacement {
    pub(crate) txn: BlockTxn,
}

impl ResolvedPlacement {
    pub(crate) fn into_txn(self) -> BlockTxn {
        self.txn
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct ResolvedMining {
    pub(crate) txn: BlockTxn,
}

impl ResolvedMining {
    pub(crate) fn into_txn(self) -> BlockTxn {
        self.txn
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MutationOutcome {
    pub changed: Vec<BlockObservation>,
    pub inventory_changed: bool,
    pub drops_created: usize,
}

// The variant payloads are the pinned S1 rule-effect record; each variant owns
// its complete effect so staging stays a single move with no allocation.
#[allow(clippy::large_enum_variant)]
#[derive(Clone, Debug, PartialEq)]
pub enum RuleEffect {
    Actor(ActorRecord),
    Inventory(InventoryPatch),
    Container {
        before: ContainerRecord,
        after: ContainerRecord,
    },
    WorldContainer {
        dimension: Dimension,
        before: ContainerRecord,
        after: ContainerRecord,
    },
    Viewer {
        session: SessionKey,
        view: Option<ViewLease>,
    },
    Mining {
        actor: ActorKey,
        progress: Option<MiningProgress>,
    },
    Damage(DamageIntent),
    Drops(DropBatch),
    DropPatch {
        before: DropRecord,
        after: Option<DropRecord>,
    },
    Projectile {
        before: Option<ProjectileRecord>,
        after: Option<ProjectileRecord>,
    },
    Sleep(SleepState),
    Work(WorkState),
    World(WorldState),
    Blocks(BlockTxn),
    Compound(Vec<RuleEffect>),
    Runtime(ActorRuntime),
    Environment(EnvironmentState),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct EatingProgress {
    pub slot: HotbarSlot,
    pub item: u16,
    pub ticks: u16,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BowProgress {
    pub slot: HotbarSlot,
    pub ticks: u16,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PathState {
    pub generation: u64,
    pub target: BlockPos,
    pub revisions: Vec<(ChunkKey, u64)>,
    pub waypoints: Vec<BlockPos>,
    pub cursor: usize,
    pub next_repath_tick: u64,
}

#[derive(Clone, Debug, PartialEq)]
pub enum ActorAux {
    Player {
        respawn: Option<(Dimension, BlockPos)>,
        /// Transient workbench anchor: the hit block of the latest settled
        /// bench open, meaningful only while `crafting_size` is Workbench.
        /// The anchor is a runtime overlay staged beside the neutral fields,
        /// never serialized, and stale values stay inert because every read
        /// gates on the bench size first.
        workbench: Option<BlockPos>,
    },
    Companion {
        generation: u64,
        attempt: u64,
        task: StoredCompanionTask,
        /// Held target survives progress interruption until an explicit release.
        /// This is transient authority state and never enters companion saves.
        mining_target: Option<BlockPos>,
    },
    Hostile {
        distant_ticks: u16,
        shoot_cooldown: u32,
        fresh: bool,
    },
    Passive {
        home: BlockPos,
        flee_ticks: u16,
        flee_from: Option<FiniteVec3>,
        graze_ticks: u16,
        graze_at: Option<BlockPos>,
        fresh: bool,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub struct ActorRuntime {
    pub key: ActorKey,
    pub controls: Option<PlayerControl>,
    pub has_view: bool,
    pub reset: bool,
    pub attack_cooldown: u32,
    pub hurt_cooldown: u32,
    pub burn_cooldown: u32,
    pub oxygen: u16,
    pub peak_y: f32,
    pub exhaustion_milli: u32,
    pub saturation_milli: u32,
    pub since_damage_ticks: u32,
    pub drown_ticks: u32,
    pub starvation_ticks: u32,
    pub eating: Option<EatingProgress>,
    pub bow: Option<BowProgress>,
    pub path: Option<PathState>,
    pub aux: ActorAux,
}

/// Checked simulation snapshot. Eye height is stored beside physics because
/// the numerical tuning value has no eye-height field.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RuleTunables {
    physics: PhysicsTuning,
    regen_delay: u32,
    regen_interval: u32,
    drown_interval: u32,
    starvation_interval: u32,
    regen_hunger_threshold: u8,
    exhaustion_threshold: u16,
    eating_ticks: u16,
    furnace_burn_ticks: u16,
    furnace_smelt_ticks: u8,
    fluid_delay: u64,
    random_attempts: u8,
    crop_growth_percent: u8,
    interaction_reach: f32,
    eye_height: f32,
    drop_pickup_delay_ticks: u8,
    player_drop_pickup_delay_ticks: u8,
    drop_lifetime_ticks: u32,
    drop_pickup_range: f32,
}

impl RuleTunables {
    // The tunable fields mirror the authoritative Go snapshot one-to-one; a
    // grouped struct would fork the single checked parameter source.
    #[allow(clippy::too_many_arguments)]
    pub fn try_new(
        physics: PhysicsTuning,
        regen_delay: u32,
        regen_interval: u32,
        drown_interval: u32,
        starvation_interval: u32,
        regen_hunger_threshold: u8,
        exhaustion_threshold: u16,
        eating_ticks: u16,
        furnace_burn_ticks: u16,
        furnace_smelt_ticks: u8,
        fluid_delay: u64,
        random_attempts: u8,
        crop_growth_percent: u8,
        interaction_reach: f32,
        eye_height: f32,
        drop_pickup_delay_ticks: u8,
        player_drop_pickup_delay_ticks: u8,
        drop_lifetime_ticks: u32,
        drop_pickup_range: f32,
    ) -> Result<Self, ServerError> {
        if !physics_finite(physics)
            || !interaction_reach.is_finite()
            || !eye_height.is_finite()
            || !drop_pickup_range.is_finite()
        {
            return Err(ServerError::InvalidInput { field: "tunables" });
        }
        if drop_lifetime_ticks > MAX_DROP_LIFETIME
            || random_attempts > MAX_RANDOM_ATTEMPTS
            || crop_growth_percent > MAX_CROP_GROWTH_PERCENT
            || furnace_smelt_ticks > MAX_FURNACE_SMELT
            || furnace_burn_ticks > MAX_FURNACE_BURN
        {
            return Err(ServerError::InvalidInput { field: "tunables" });
        }
        Ok(Self {
            physics,
            regen_delay,
            regen_interval,
            drown_interval,
            starvation_interval,
            regen_hunger_threshold,
            exhaustion_threshold,
            eating_ticks,
            furnace_burn_ticks,
            furnace_smelt_ticks,
            fluid_delay,
            random_attempts,
            crop_growth_percent,
            interaction_reach,
            eye_height,
            drop_pickup_delay_ticks,
            player_drop_pickup_delay_ticks,
            drop_lifetime_ticks,
            drop_pickup_range,
        })
    }

    /// Source defaults. Eye height is the physics snapshot `1.62`. The paired
    /// drop delays are mining `10` and player drop `40`.
    pub fn source_defaults() -> Self {
        Self::try_new(
            source_physics(),
            100,
            40,
            20,
            80,
            18,
            4000,
            32,
            MAX_FURNACE_BURN,
            MAX_FURNACE_SMELT,
            5,
            3,
            50,
            6.0,
            1.62,
            10,
            40,
            6000,
            1.25,
        )
        .expect("source defaults are inside the checked ranges")
    }

    pub fn physics(self) -> PhysicsTuning {
        self.physics
    }
    /// Rule providers consume the stored tick snapshot, never source defaults.
    pub fn regen_delay_ticks(self) -> u32 {
        self.regen_delay
    }
    pub fn regen_interval_ticks(self) -> u32 {
        self.regen_interval
    }
    pub fn drown_interval_ticks(self) -> u32 {
        self.drown_interval
    }
    pub fn starvation_interval_ticks(self) -> u32 {
        self.starvation_interval
    }
    pub fn regen_hunger_threshold(self) -> u8 {
        self.regen_hunger_threshold
    }
    pub fn eating_ticks(self) -> u16 {
        self.eating_ticks
    }
    pub fn furnace_burn_ticks(self) -> u16 {
        self.furnace_burn_ticks
    }
    pub fn furnace_smelt_ticks(self) -> u8 {
        self.furnace_smelt_ticks
    }
    pub fn random_attempts(self) -> u8 {
        self.random_attempts
    }
    pub fn crop_growth_percent(self) -> u8 {
        self.crop_growth_percent
    }
    pub fn interaction_reach(self) -> f32 {
        self.interaction_reach
    }
    pub fn eye_height(self) -> f32 {
        self.eye_height
    }
    pub fn exhaustion_threshold_milli(self) -> u16 {
        self.exhaustion_threshold
    }
    pub fn fluid_delay(self) -> u64 {
        self.fluid_delay
    }
    pub fn drop_pickup_delay_ticks(self) -> u8 {
        self.drop_pickup_delay_ticks
    }
    pub fn player_drop_pickup_delay_ticks(self) -> u8 {
        self.player_drop_pickup_delay_ticks
    }
    pub fn drop_lifetime_ticks(self) -> u32 {
        self.drop_lifetime_ticks
    }
    pub fn drop_pickup_range(self) -> f32 {
        self.drop_pickup_range
    }
}

fn source_physics() -> PhysicsTuning {
    PhysicsTuning {
        fixed_delta_seconds: 0.05,
        step_height: 0.6,
        walk_speed: 4.3,
        ground_acceleration: 40.0,
        ground_deceleration: 50.0,
        air_acceleration: 8.0,
        jump_speed: 8.4,
        gravity: 32.0,
        terminal_fall_speed: 78.4,
        fluid_gravity: 6.4,
        fluid_sink_speed: 3.0,
        fluid_ascend_speed: 4.0,
        fluid_horizontal_drag: 0.8,
        sprint_speed_multiplier: 1.3,
        sneak_speed_multiplier: 0.3,
    }
}

fn physics_finite(tuning: PhysicsTuning) -> bool {
    [
        tuning.fixed_delta_seconds,
        tuning.step_height,
        tuning.walk_speed,
        tuning.ground_acceleration,
        tuning.ground_deceleration,
        tuning.air_acceleration,
        tuning.jump_speed,
        tuning.gravity,
        tuning.terminal_fall_speed,
        tuning.fluid_gravity,
        tuning.fluid_sink_speed,
        tuning.fluid_ascend_speed,
        tuning.fluid_horizontal_drag,
        tuning.sprint_speed_multiplier,
        tuning.sneak_speed_multiplier,
    ]
    .into_iter()
    .all(|value| value.is_finite())
}

#[derive(Clone, Debug, PartialEq)]
pub struct EnvironmentState {
    pub seed: i64,
    pub next_tick: u64,
    pub world_time: u64,
    pub day_phase_offset: u16,
    pub season_offset: u32,
    pub weather: Weather,
    pub weather_remaining: u32,
    pub difficulty: u8,
    pub tunables: RuleTunables,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RulePhase {
    PlayerCommand,
    CompanionIntent,
    Acquire,
    PlayerRegenStarvation,
    Eating,
    BowDraw,
    PlayerPrePhysicsOxygen,
    PlayerMotion,
    PlayerPostPhysics,
    CompanionMotion,
    HostileActions,
    HostileMotion,
    Combat,
    HostileBurnDistant,
    ProjectileStep,
    HostilePlayerDeaths,
    PassiveStepDeaths,
    CompanionPlacement,
    Interaction,
    SleepSettlement,
    DropStep,
    FurnaceStep,
    FluidRescan,
    FluidUpdate,
    Farmland,
    Trample,
    SnowFootprint,
    RandomBlock,
    ContainerMove,
    MiningStep,
    WorkbenchLifecycle,
    Support,
    Publish,
    EnvironmentEnd,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RuleCall<'a> {
    pub phase: RulePhase,
    pub actor: Option<ActorKey>,
    pub command: Option<&'a CommandEnvelope>,
    pub internal: Option<&'a AuthorityInteraction>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InteractionKind {
    Door,
    Bed,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AuthorityInteraction {
    pub session: SessionKey,
    pub look: LookAngles,
    pub kind: InteractionKind,
    pub sequence: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhaseReport {
    pub examined: usize,
    pub applied: usize,
    pub carried: usize,
    pub rejected: usize,
}

#[derive(Clone, Debug, PartialEq)]
pub enum CompanionAction {
    Move {
        move_x: i8,
        move_z: i8,
        jump: bool,
        yaw: f32,
    },
    MineHold {
        target: BlockPos,
    },
    MineRelease,
    Place {
        target: BlockPos,
        block: u16,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub struct CompanionActionEnvelope {
    pub companion_id: CompanionId,
    pub source_tick: u64,
    pub request_id: AgentRequestId,
    pub run_id: RunId,
    pub snapshot_id: SnapshotId,
    pub generation: u64,
    pub attempt: u64,
    pub snapshot_digest: [u8; 32],
    pub action: CompanionAction,
}

impl CompanionActionEnvelope {
    // Provenance identity is the frozen Agent admission record; every field is
    // individually validated before any state changes.
    #[allow(clippy::too_many_arguments)]
    pub fn try_new(
        companion_id: CompanionId,
        source_tick: u64,
        request_id: AgentRequestId,
        run_id: RunId,
        snapshot_id: SnapshotId,
        generation: u64,
        attempt: u64,
        snapshot_digest: [u8; 32],
        action: CompanionAction,
    ) -> Result<Self, ServerError> {
        if generation == 0 || attempt == 0 {
            return Err(ServerError::InvalidInput {
                field: "companion_generation",
            });
        }
        if let CompanionAction::Move { yaw, .. } = action
            && !yaw.is_finite()
        {
            return Err(ServerError::InvalidInput { field: "yaw" });
        }
        if let CompanionAction::Place { block, .. } = action
            && !registered_block(block)
        {
            return Err(ServerError::InvalidInput { field: "block" });
        }
        Ok(Self {
            companion_id,
            source_tick,
            request_id,
            run_id,
            snapshot_id,
            generation,
            attempt,
            snapshot_digest,
            action,
        })
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct ScheduledInput {
    pub earliest_tick: u64,
    pub session: SessionKey,
    pub intent: PlayIntent,
}

#[derive(Clone, Debug, PartialEq)]
pub enum FixtureInput {
    Human(ScheduledInput),
    Companion {
        earliest_tick: u64,
        value: CompanionActionEnvelope,
    },
    Internal {
        earliest_tick: u64,
        value: AuthorityInteraction,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub struct FixtureState {
    pub runtime: Vec<ActorRuntime>,
    pub actors: Vec<ActorRecord>,
    pub chunks: Vec<(ChunkKey, u64, u64, Chunk)>,
    pub inventories: Vec<(ActorKey, InventoryRecord)>,
    pub containers: Vec<ContainerRecord>,
    pub work: WorkState,
    pub sleep: SleepState,
    pub projectiles: Vec<ProjectileRecord>,
    pub drops: Vec<DropRecord>,
    pub world: WorldState,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Expected {
    pub events: Vec<RoutedEvent>,
    pub state_sha256: [u8; 32],
    pub class: Option<RuleReject>,
    pub counters: TickCounters,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Observed {
    pub events: Vec<RoutedEvent>,
    pub counters: TickCounters,
    pub state_sha256: [u8; 32],
    pub class: Option<RuleReject>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Fixture {
    pub seed: u64,
    pub initial: FixtureState,
    pub schedule: Vec<FixtureInput>,
    pub expected: Expected,
    pub budgets: Vec<(u64, TickBudget)>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SaveMode {
    Urgent,
    All,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SaveUrgency {
    Unload,
    Autosave,
    Shutdown,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SaveBudget {
    pub chunks: usize,
    pub estimated_bytes: usize,
}

impl Default for SaveBudget {
    fn default() -> Self {
        Self {
            chunks: DEFAULT_SAVE_CHUNKS,
            estimated_bytes: DEFAULT_SAVE_BYTES,
        }
    }
}

// Save payloads are the frozen storage value union; each lane is bounded by
// its own StoreLimits ceiling rather than by boxing the enum.
#[allow(clippy::large_enum_variant)]
#[derive(Clone, Debug, PartialEq)]
pub enum SaveValue {
    Chunk(ChunkSave),
    Player(PlayerSave),
    Companions(CompanionSave),
    Hostiles(HostileMobsSave),
    Passives(PassiveMobsSave),
    Metadata(Metadata),
}

#[derive(Clone, Debug, PartialEq)]
pub struct OwnedSnapshot {
    pub key: SaveKey,
    pub revision: u64,
    pub estimated_bytes: usize,
    pub urgency: SaveUrgency,
    pub value: SaveValue,
}

impl OwnedSnapshot {
    pub fn try_new(
        key: SaveKey,
        revision: u64,
        estimated_bytes: usize,
        urgency: SaveUrgency,
        value: SaveValue,
    ) -> Result<Self, ServerError> {
        if matches!(key, SaveKey::Metadata) && revision == 0 {
            return Err(ServerError::InvalidInput {
                field: "metadata_sequence",
            });
        }
        if !key_matches(&key, &value) {
            return Err(ServerError::InvalidInput {
                field: "save_value",
            });
        }
        Ok(Self {
            key,
            revision,
            estimated_bytes,
            urgency,
            value,
        })
    }
}

fn key_matches(key: &SaveKey, value: &SaveValue) -> bool {
    matches!(
        (key, value),
        (SaveKey::Chunk(_), SaveValue::Chunk(_))
            | (SaveKey::Player(_), SaveValue::Player(_))
            | (SaveKey::Companions, SaveValue::Companions(_))
            | (SaveKey::Hostiles, SaveValue::Hostiles(_))
            | (SaveKey::Passives, SaveValue::Passives(_))
            | (SaveKey::Metadata, SaveValue::Metadata(_))
    )
}

#[derive(Clone, Debug, PartialEq)]
pub struct SaveRequest {
    pub snapshots: Vec<OwnedSnapshot>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SubmitSaveError {
    pub error: ServerError,
    pub request: SaveRequest,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SaveCompletion {
    pub ticket: SaveTicket,
    pub snapshots: Vec<OwnedSnapshot>,
    pub submitted: Vec<(SaveKey, u64)>,
    pub committed: Vec<(SaveKey, u64)>,
    pub error: Option<ServerError>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum SavePoll {
    Pending,
    Completed(SaveCompletion),
}

#[derive(Clone, Debug, PartialEq)]
pub struct AckReport {
    pub acked: usize,
    pub released: usize,
    pub retry: Vec<OwnedSnapshot>,
    pub errors: Vec<ServerError>,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct SaveStats {
    pub dirty: usize,
    pub in_flight: usize,
    pub estimated_unsaved_bytes: usize,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SaveScheduleReport {
    pub urgent: usize,
    pub autosave: usize,
    pub retry: usize,
    pub stats: SaveStats,
    pub backpressured: bool,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct FlushReport {
    pub durable: usize,
    pub failed: usize,
    pub outstanding: usize,
}

pub trait SaveAuthority {
    fn select(&mut self, mode: SaveMode, budget: SaveBudget) -> Vec<OwnedSnapshot>;
    fn return_dirty(&mut self, snapshot: OwnedSnapshot);
    fn apply_completion(&mut self, completion: SaveCompletion) -> AckReport;
    fn save_stats(&self) -> SaveStats;
    fn metadata_snapshot(&self) -> OwnedSnapshot;
}

pub trait StoreHandle {
    fn submit(&mut self, request: SaveRequest) -> Result<SaveTicket, SubmitSaveError>;
    fn poll(&mut self, ticket: SaveTicket) -> SavePoll;
    fn poll_tick(
        &mut self,
        tick: u64,
        budget: SaveBudget,
        authority: &mut dyn SaveAuthority,
    ) -> Result<SaveScheduleReport, ServerError>;
    fn cancel_pending(&mut self) -> Result<Vec<OwnedSnapshot>, ServerError>;
    fn flush(
        &mut self,
        deadline: Deadline,
        authority: &mut dyn SaveAuthority,
        clock: &dyn Clock,
    ) -> Result<FlushReport, ServerError>;
    fn sync(&mut self, deadline: Deadline) -> Result<(), ServerError>;
    fn close(&mut self, deadline: Deadline) -> Result<(), ServerError>;
}

pub trait DiskBackend {
    fn write(&mut self, ticket: SaveTicket, request: SaveRequest) -> SaveCompletion;
    fn load(&mut self, key: SaveKey) -> Result<LoadedValue, ServerError>;
    fn sync(&mut self) -> Result<(), ServerError>;
    fn close(&mut self) -> Result<(), ServerError>;
}

/// Decoded storage facts retain migration/recovery information until the
/// authority decides when a normalized value can be saved.
#[allow(clippy::large_enum_variant)]
#[derive(Clone, Debug, PartialEq)]
pub enum LoadedValue {
    Chunk(RecoveredChunk),
    Player(StoredPlayer),
    Companions(StoredCompanions),
    Hostiles(HostileMobs),
    Passives(PassiveMobs),
    Metadata(Metadata),
}

/// A later failure must not erase revisions that are already durable.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DiskWriteOutcome {
    pub committed: Vec<(SaveKey, u64)>,
    pub error: Option<ServerError>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RecoveredChunk {
    pub chunk: Chunk,
    pub revision: u64,
    pub persisted_revision: u64,
    pub needs_rewrite: bool,
    pub recovered: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum IoFaultPoint {
    PayloadWrite,
    PayloadSync,
    BankWrite,
    BankSync,
    TempWrite,
    TempSync,
    Rename,
    DirectorySync,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BaseIdentity {
    pub request_id: AgentRequestId,
    pub client_instance_id: ClientInstanceId,
    pub namespace_id: NamespaceId,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LeasedIdentity {
    pub base: BaseIdentity,
    pub lease_id: LeaseId,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlanRequest {
    pub leased: LeasedIdentity,
    pub run_id: RunId,
    pub companion_id: CompanionId,
    pub generation: u64,
    pub snapshot_id: SnapshotId,
    pub snapshot_digest: [u8; 32],
    pub deadline_unix_ms: i64,
    pub mcp_endpoint: String,
    pub capability: String,
    pub instruction: CommandText,
}

impl PlanRequest {
    // The leased plan request mirrors the Agent HTTP contract record; fields
    // are validated as one unit before the request becomes visible.
    #[allow(clippy::too_many_arguments)]
    pub fn try_new(
        leased: LeasedIdentity,
        run_id: RunId,
        companion_id: CompanionId,
        generation: u64,
        snapshot_id: SnapshotId,
        snapshot_digest: [u8; 32],
        deadline_unix_ms: i64,
        mcp_endpoint: String,
        capability: String,
        instruction: CommandText,
    ) -> Result<Self, ServerError> {
        if generation == 0 || deadline_unix_ms < 1 {
            return Err(ServerError::InvalidInput { field: "plan" });
        }
        checked_text(&mcp_endpoint, 1, 256, true)?;
        checked_text(&capability, 1, 512, true)?;
        Ok(Self {
            leased,
            run_id,
            companion_id,
            generation,
            snapshot_id,
            snapshot_digest,
            deadline_unix_ms,
            mcp_endpoint,
            capability,
            instruction,
        })
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PlanBlock {
    Brick,
    Chest,
    Clay,
    Cobblestone,
    Dirt,
    Furnace,
    Glass,
    Grass,
    Gravel,
    IronBlock,
    Leaves,
    LightBlock,
    MossyCobblestone,
    OakLog,
    OakPlanks,
    RoofTile,
    Sand,
    SmoothStone,
    SnowBlock,
    Stone,
    StoneBrick,
    WhiteWool,
    Workbench,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PlanStep {
    GoTo {
        x: i32,
        y: i32,
        z: i32,
    },
    Mine {
        x: i32,
        y: i32,
        z: i32,
    },
    Place {
        x: i32,
        y: i32,
        z: i32,
        block: PlanBlock,
    },
    Follow {
        player_id: PlayerId,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentPlan {
    pub summary: String,
    pub steps: Vec<PlanStep>,
}

impl AgentPlan {
    pub fn try_new(summary: String, steps: Vec<PlanStep>) -> Result<Self, ServerError> {
        if steps.is_empty() || steps.len() > PLAN_STEP_LIMIT {
            return Err(ServerError::InvalidInput {
                field: "plan_steps",
            });
        }
        checked_text(&summary, 1, 512, true)?;
        for step in &steps {
            let y = match step {
                PlanStep::GoTo { y, .. } | PlanStep::Mine { y, .. } | PlanStep::Place { y, .. } => {
                    Some(*y)
                }
                PlanStep::Follow { .. } => None,
            };
            if let Some(y) = y
                && !(BLOCK_Y_MIN..=BLOCK_Y_MAX).contains(&y)
            {
                return Err(ServerError::InvalidInput { field: "plan_y" });
            }
        }
        if let Some(index) = steps
            .iter()
            .position(|step| matches!(step, PlanStep::Follow { .. }))
            && (index + 1 != steps.len()
                || steps
                    .iter()
                    .filter(|step| matches!(step, PlanStep::Follow { .. }))
                    .count()
                    != 1)
        {
            return Err(ServerError::InvalidInput {
                field: "plan_follow",
            });
        }
        Ok(Self { summary, steps })
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DialogueProgress {
    GoTo,
    Mine,
    Place,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DialogueFact {
    Start,
    Progress(DialogueProgress),
    FirstArrival,
    Idle,
    Terminal {
        failed: bool,
        reason: DialogueFailure,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DialogueFailure {
    None,
    PlannerUnavailable,
    InvalidPlan,
    PathUnreachable,
    WorldChanged,
    InventoryFull,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DialogueEnvironment {
    pub exposed_blocks: Vec<(BlockPos, u16)>,
    pub heights: Vec<(i32, i32, i32)>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DialogueRequest {
    pub leased: LeasedIdentity,
    pub run_id: RunId,
    pub companion_id: CompanionId,
    pub generation: u64,
    pub memory_epoch: u64,
    pub deadline_unix_ms: i64,
    pub persona: String,
    pub fact_node: DialogueFact,
    pub environment: DialogueEnvironment,
    pub terminal: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MemoryState {
    Absent,
    Present {
        revision: NonZeroU64,
        operation_id: OperationId,
        summary: String,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ReconcileRequest {
    Active {
        leased: LeasedIdentity,
        companion_id: CompanionId,
        memory_epoch: u64,
        mirror: MemoryState,
    },
    Inactive {
        leased: LeasedIdentity,
        companion_id: CompanionId,
        memory_epoch: u64,
        tombstone_operation_id: OperationId,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommitRequest {
    pub leased: LeasedIdentity,
    pub companion_id: CompanionId,
    pub memory_epoch: u64,
    pub base_revision: u64,
    pub operation_id: OperationId,
    pub summary: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeleteRequest {
    pub leased: LeasedIdentity,
    pub companion_id: CompanionId,
    pub old_memory_epoch: u64,
    pub new_memory_epoch: u64,
    pub tombstone_operation_id: OperationId,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CancelRequest {
    pub leased: LeasedIdentity,
    pub run_id: RunId,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AgentRequest {
    Acquire(BaseIdentity),
    Heartbeat(LeasedIdentity),
    Release(LeasedIdentity),
    Plan(PlanRequest),
    Cancel(CancelRequest),
    Dialogue(DialogueRequest),
    Reconcile(ReconcileRequest),
    Commit(CommitRequest),
    Delete(DeleteRequest),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlanResponse {
    pub leased: LeasedIdentity,
    pub run_id: RunId,
    pub companion_id: CompanionId,
    pub generation: u64,
    pub snapshot_id: SnapshotId,
    pub snapshot_digest: [u8; 32],
    pub plan: AgentPlan,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DialogueResponse {
    pub leased: LeasedIdentity,
    pub run_id: RunId,
    pub companion_id: CompanionId,
    pub generation: u64,
    pub memory_epoch: u64,
    pub line: String,
    pub memory_proposal: Option<MemoryProposal>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MemoryProposal {
    pub operation_id: OperationId,
    pub base_revision: u64,
    pub summary: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ReconcileResponse {
    Active {
        leased: LeasedIdentity,
        companion_id: CompanionId,
        memory_epoch: u64,
        memory: MemoryState,
    },
    Inactive {
        leased: LeasedIdentity,
        companion_id: CompanionId,
        memory_epoch: u64,
        tombstone_operation_id: OperationId,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommitResponse {
    pub leased: LeasedIdentity,
    pub companion_id: CompanionId,
    pub memory_epoch: u64,
    pub operation_id: OperationId,
    pub committed_revision: NonZeroU64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeleteResponse {
    pub leased: LeasedIdentity,
    pub companion_id: CompanionId,
    pub memory_epoch: u64,
    pub tombstone_operation_id: OperationId,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CancelResponse {
    pub leased: LeasedIdentity,
    pub run_id: RunId,
    pub cancelled: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LeaseResponse {
    pub leased: LeasedIdentity,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AgentResponse {
    Acquire(LeaseResponse),
    Heartbeat(LeaseResponse),
    Release(LeaseResponse),
    Plan(PlanResponse),
    Cancel(CancelResponse),
    Dialogue(DialogueResponse),
    Reconcile(ReconcileResponse),
    Commit(CommitResponse),
    Delete(DeleteResponse),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AgentPoll {
    Pending,
    Completed(AgentResponse),
    Failed(ServerError),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FrozenLease {
    pub client: ClientInstanceId,
    pub namespace: NamespaceId,
    pub lease: LeaseId,
    pub lease_fence: u64,
    pub expires_at: Instant,
}

pub trait AgentHandle {
    fn submit(&mut self, request: AgentRequest) -> Result<AgentRequestId, ServerError>;
    fn poll(&mut self, id: AgentRequestId) -> AgentPoll;
    /// Retires local request ownership. Already absent ownership succeeds without
    /// wire work; a first join error may be reported after ownership is reclaimed.
    fn cancel(&mut self, id: AgentRequestId, deadline: Deadline) -> Result<(), ServerError>;
    fn freeze(&mut self, clock: &dyn Clock) -> Option<FrozenLease>;
    fn release(&mut self, lease: &FrozenLease, deadline: Deadline) -> Result<(), ServerError>;
    fn close(&mut self, deadline: Deadline) -> Result<(), ServerError>;
}

/// Checked task-status text: at most 96 bytes and no Unicode controls.
/// The empty string is valid.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SnapshotTaskStatusText(String);

impl SnapshotTaskStatusText {
    pub fn try_new(text: impl Into<String>) -> Result<Self, ServerError> {
        let text = text.into();
        checked_text(&text, 0, 96, true)?;
        Ok(Self(text))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn queued() -> Self {
        Self("待执行".to_owned())
    }
    pub fn planning() -> Self {
        Self("规划中".to_owned())
    }
    pub fn validating() -> Self {
        Self("校验中".to_owned())
    }
    pub fn running() -> Self {
        Self("执行中".to_owned())
    }
    pub fn completed() -> Self {
        Self("已完成".to_owned())
    }
    pub fn failed() -> Self {
        Self("失败".to_owned())
    }
    pub fn timed_out() -> Self {
        Self("已超时".to_owned())
    }
    pub fn stopped() -> Self {
        Self("已停止".to_owned())
    }
    pub fn idle() -> Self {
        Self("空闲".to_owned())
    }

    pub fn from_durable_state(state: Option<u8>) -> Self {
        match state {
            Some(mornlea_storage::COMPANION_TASK_QUEUED) => Self::queued(),
            Some(mornlea_storage::COMPANION_TASK_PLANNING) => Self::planning(),
            Some(mornlea_storage::COMPANION_TASK_VALIDATING) => Self::validating(),
            Some(mornlea_storage::COMPANION_TASK_RUNNING) => Self::running(),
            Some(mornlea_storage::COMPANION_TASK_COMPLETED) => Self::completed(),
            Some(mornlea_storage::COMPANION_TASK_FAILED) => Self::failed(),
            Some(mornlea_storage::COMPANION_TASK_TIMED_OUT) => Self::timed_out(),
            Some(mornlea_storage::COMPANION_TASK_STOPPED) => Self::stopped(),
            _ => Self::idle(),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct SnapshotIssuer {
    pub player_id: PlayerId,
    pub position: FiniteVec3,
    pub look: LookAngles,
    pub look_hit: Option<BlockPos>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SnapshotCompanion {
    pub companion_id: CompanionId,
    pub position: FiniteVec3,
    pub look: LookAngles,
    pub task_status: SnapshotTaskStatusText,
    pub inventory: [ItemStack; 36],
}

#[derive(Clone, Debug, PartialEq)]
pub struct SnapshotPlayer {
    pub player_id: PlayerId,
    pub position: FiniteVec3,
    pub look: LookAngles,
    pub look_hit: Option<BlockPos>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SnapshotChunkRevision {
    pub pos: ChunkPos,
    pub revision: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SnapshotBlock {
    pub position: BlockPos,
    pub block_id: u16,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SnapshotTerrain {
    pub origin: BlockPos,
    pub dimensions: [u8; 3],
    pub ready_columns: Vec<u8>,
    pub heights: Vec<i16>,
    pub blocks: Vec<u16>,
}

impl SnapshotTerrain {
    pub fn try_new(
        origin: BlockPos,
        dimensions: [u8; 3],
        ready_columns: Vec<u8>,
        heights: Vec<i16>,
        blocks: Vec<u16>,
    ) -> Result<Self, ServerError> {
        if dimensions != [TERRAIN_X, TERRAIN_Y, TERRAIN_Z]
            || ready_columns.len() != TERRAIN_READY_COLUMNS
            || heights.len() != TERRAIN_HEIGHTS
            || blocks.len() != TERRAIN_BLOCKS
        {
            return Err(ServerError::InvalidInput { field: "terrain" });
        }
        Ok(Self {
            origin,
            dimensions,
            ready_columns,
            heights,
            blocks,
        })
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct PlanningSnapshot {
    source_tick: u64,
    pub world_time_ticks: u64,
    pub instruction: CommandText,
    pub issuer: SnapshotIssuer,
    pub companion: SnapshotCompanion,
    pub online_players: Vec<SnapshotPlayer>,
    pub chunk_revisions: Vec<SnapshotChunkRevision>,
    pub exposed_blocks: Vec<SnapshotBlock>,
    pub terrain: SnapshotTerrain,
}

impl PlanningSnapshot {
    // The snapshot sections mirror the frozen Agent MCP snapshot payload; each
    // position names one bounded section of the same record.
    #[allow(clippy::too_many_arguments)]
    pub fn try_new(
        source_tick: u64,
        world_time_ticks: u64,
        instruction: CommandText,
        issuer: SnapshotIssuer,
        companion: SnapshotCompanion,
        online_players: Vec<SnapshotPlayer>,
        chunk_revisions: Vec<SnapshotChunkRevision>,
        exposed_blocks: Vec<SnapshotBlock>,
        terrain: SnapshotTerrain,
    ) -> Result<Self, ServerError> {
        if online_players.len() > MAX_PLAYERS as usize || chunk_revisions.len() > 9 {
            return Err(ServerError::InvalidInput { field: "snapshot" });
        }
        Ok(Self {
            source_tick,
            world_time_ticks,
            instruction,
            issuer,
            companion,
            online_players,
            chunk_revisions,
            exposed_blocks,
            terrain,
        })
    }

    /// Correlation kept off the HTTP and MCP documents.
    pub fn source_tick(&self) -> u64 {
        self.source_tick
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SnapshotRegistration {
    pub id: SnapshotId,
    pub digest: [u8; 32],
    pub capability: Vec<u8>,
    pub mcp_endpoint: String,
}

impl SnapshotRegistration {
    pub fn try_new(
        id: SnapshotId,
        digest: [u8; 32],
        capability: Vec<u8>,
        mcp_endpoint: String,
    ) -> Result<Self, ServerError> {
        if capability.len() != 32 {
            return Err(ServerError::InvalidInput {
                field: "capability",
            });
        }
        checked_text(&mcp_endpoint, 1, 256, true)?;
        Ok(Self {
            id,
            digest,
            capability,
            mcp_endpoint,
        })
    }
}

pub trait SnapshotPort {
    fn register(
        &mut self,
        namespace: NamespaceId,
        companion: CompanionId,
        generation: u64,
        snapshot: PlanningSnapshot,
        deadline: Deadline,
    ) -> Result<SnapshotRegistration, ServerError>;
    fn complete(&mut self, id: SnapshotId) -> Result<(), ServerError>;
    fn cancel(&mut self, id: SnapshotId) -> Result<(), ServerError>;
    fn close(&mut self) -> Result<(), ServerError>;
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ShutdownPhase {
    StopAdmission,
    FinalTick,
    Freeze,
    WaitWorkers,
    FinalizeMemory,
    FlushPlayers,
    FlushCompanions,
    FlushHostiles,
    FlushPassives,
    FlushWorld,
    FlushMetadata,
    StoreSync,
    ReleaseAgent,
    AgentClose,
    McpClose,
    StoreClose,
    CloseWorkers,
    Closed,
}

impl ShutdownPhase {
    pub fn successors() -> &'static [Self] {
        &[
            Self::StopAdmission,
            Self::FinalTick,
            Self::Freeze,
            Self::WaitWorkers,
            Self::FinalizeMemory,
            Self::FlushPlayers,
            Self::FlushCompanions,
            Self::FlushHostiles,
            Self::FlushPassives,
            Self::FlushWorld,
            Self::FlushMetadata,
            Self::StoreSync,
            Self::ReleaseAgent,
            Self::AgentClose,
            Self::McpClose,
            Self::StoreClose,
            Self::CloseWorkers,
            Self::Closed,
        ]
    }

    pub fn next(self) -> Self {
        let phases = Self::successors();
        phases
            .iter()
            .position(|phase| *phase == self)
            .and_then(|index| phases.get(index + 1).copied())
            .unwrap_or(Self::Closed)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ShutdownReport {
    pub final_tick: Option<u64>,
    pub completed: Vec<ShutdownPhase>,
    pub next: ShutdownPhase,
    pub durable: usize,
    pub failed: usize,
    pub outstanding: usize,
    pub retryable: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ShutdownFailure {
    pub error: ServerError,
    pub report: ShutdownReport,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct MemoryFinalizationReport {
    pub completed: usize,
    pub outstanding: usize,
}

pub trait FinalReducer {
    fn reduce_final(
        &mut self,
        authority: &mut crate::core::state::AuthorityState,
    ) -> Result<u64, ServerError>;
}

pub trait WorkerLifecycle {
    fn stop_new(&mut self) -> Result<(), ServerError>;
    fn cancel(&mut self) -> Result<(), ServerError>;
    fn wait(&mut self, deadline: Deadline) -> Result<(), ServerError>;
    fn close(&mut self, deadline: Deadline) -> Result<(), ServerError>;
}

pub trait ActorPersistence {
    fn flush(&mut self, family: SaveKey, deadline: Deadline) -> Result<FlushReport, ServerError>;
}

pub trait McpLifecycle {
    fn close(&mut self, deadline: Deadline) -> Result<(), ServerError>;
}

pub trait MemoryFinalizer {
    /// Pure retained ownership snapshot, available even after an attempt fails.
    /// A semantic operation and its current RPC count once; cleanup joins remain owned.
    fn pending(&self) -> MemoryFinalizationReport;
    fn begin_attempt(&mut self, deadline: Deadline) -> Result<(), ServerError>;
    fn drain(&mut self, deadline: Deadline) -> Result<MemoryFinalizationReport, ServerError>;
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ConnectionProgress {
    AwaitMore,
    Advanced {
        frames: usize,
    },
    Closed {
        reason: CloseReason,
        class: Option<ServerError>,
    },
}

// The login success packet is carried whole so the poll hands the caller one
// owned record; login lanes are bounded by the pending-login ceiling.
#[allow(clippy::large_enum_variant)]
#[derive(Clone, Debug, PartialEq)]
pub enum LoginPoll {
    Pending,
    Ready {
        session: SessionKey,
        success: ServerPacket,
    },
    Failed {
        error: ServerError,
        reject: u8,
    },
}

// A missing player initializes a canonical new player; the loaded record is
// transferred whole to the session installer without a second copy.
#[allow(clippy::large_enum_variant)]
#[derive(Clone, Debug, PartialEq)]
pub enum LoadPoll {
    Pending,
    Loaded(Option<StoredPlayer>),
    Failed(ServerError),
}

pub trait PlayerLoadPort {
    fn start(&mut self, player: PlayerId, deadline: Deadline) -> Result<LoginTicket, ServerError>;
    fn poll(&mut self, ticket: LoginTicket) -> LoadPoll;
    fn cancel(&mut self, ticket: LoginTicket) -> Result<(), ServerError>;
}

/// A chunk load transfers its checked compact base and source durability facts.
/// Only a typed missing file is absence; failed decoding never means generation.
// The eight-entry chunk lane transfers prepared records whole; their large
// compact storage stays shared rather than adding a second payload allocation.
#[allow(clippy::large_enum_variant)]
pub enum ChunkLoadPoll {
    Pending,
    Loaded(Option<crate::core::world::PreparedChunk>),
    Failed(ServerError),
}

/// Nonblocking load ownership; the same disk owner also executes player loads.
/// Cancellation of started work retains its resources until its reply is drained.
pub trait ChunkLoadPort {
    fn start_chunk(
        &mut self,
        key: ChunkKey,
        generation: u64,
        deadline: Deadline,
    ) -> Result<ChunkRequestId, ServerError>;
    fn poll_chunk(&mut self, request: ChunkRequestId) -> ChunkLoadPoll;
    fn cancel_chunk(&mut self, request: ChunkRequestId) -> Result<(), ServerError>;
}

pub trait SessionPort {
    fn allocate(
        &mut self,
        login: AdmittedLogin,
        kind: TransportKind,
    ) -> Result<SessionKey, ServerError>;
    fn prepare(
        &mut self,
        login: AdmittedLogin,
        kind: TransportKind,
    ) -> Result<SessionKey, ServerError>;
    fn install(
        &mut self,
        session: SessionKey,
        loaded: Option<StoredPlayer>,
    ) -> Result<(), ServerError>;
    fn activate(&mut self, session: SessionKey) -> Result<(), ServerError>;
    fn session(&self, key: SessionKey) -> Option<SessionFacts>;
    fn retire(&mut self, key: SessionKey, reason: CloseReason) -> Result<(), ServerError>;
    fn accept(
        &mut self,
        session: SessionKey,
        intent: PlayIntent,
    ) -> Result<SubmissionReceipt, ServerError>;
    fn apply_sequence(&mut self, session: SessionKey, sequence: u64) -> bool;
}

pub trait MailboxPort {
    fn freeze_eligible(&mut self, tick: u64) -> Vec<CommandEnvelope>;
    fn carry(&mut self, batch: Vec<CommandEnvelope>) -> Result<(), ServerError>;
    // Refusal returns the original owned chunk result so the producer keeps
    // ownership; the mailbox never stores or copies a rejected record.
    #[allow(clippy::result_large_err)]
    fn admit_chunk(&mut self, result: ChunkResult) -> Result<(), ChunkResult>;
    fn drain_chunks(&mut self, max: usize) -> Vec<ChunkResult>;
    fn cancel_chunk(&mut self, request: ChunkRequestId);
    fn drain_companions(&mut self, max: usize) -> Vec<CompanionActionEnvelope>;
    fn enqueue_interaction(&mut self, value: AuthorityInteraction) -> Result<(), ServerError>;
}

pub trait PublicationPort {
    fn publish(&mut self, publication: TickPublication) -> Result<(), ServerError>;
    /// Transfers complete canonical wire frames, including packet identity.
    /// The byte budget counts the whole frame; adapters never reconstruct IDs.
    fn take_outbox(
        &mut self,
        session: SessionKey,
        max_frames: usize,
        max_bytes: usize,
    ) -> Result<Vec<Vec<u8>>, ServerError>;
    fn close_outbox(&mut self, session: SessionKey, reason: CloseReason);
}

pub trait LifecyclePort {
    fn begin_close(&mut self) -> bool;
    fn run_final(&mut self, reducer: &mut dyn FinalReducer) -> Result<u64, ServerError>;
    fn freeze(&mut self) -> FrozenAuthority;
    fn shutdown_progress(&self) -> ShutdownReport;
    fn record_progress(&mut self, report: ShutdownReport);
    fn mark_closed(&mut self);
}

fn checked_text(
    text: &str,
    min: usize,
    max: usize,
    reject_controls: bool,
) -> Result<(), ServerError> {
    if text.len() < min || text.len() > max {
        return Err(ServerError::InvalidInput { field: "text" });
    }
    if reject_controls && text.chars().any(|ch| ch.is_control()) {
        return Err(ServerError::InvalidInput { field: "text" });
    }
    Ok(())
}

pub const MAX_PROJECTILE_RECORDS: usize = MAX_PROJECTILES;
pub const RESCAN_SECTION: usize = RESCAN_SECTION_CELLS;
pub const RESCAN_MAX_OVERSHOOT: usize = RESCAN_OVERSHOOT;
pub const EFFECT_BUDGET: usize = MAX_EFFECTS;
