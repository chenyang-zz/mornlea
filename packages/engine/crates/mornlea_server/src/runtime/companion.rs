//! Configured companion startup: the durable save bootstrap, the authority
//! handoff, and Agent service startup.
//!
//! This mirrors the Go `NewHost` companion bootstrap. With no configured
//! companion an existing aggregate is still merged so retired companions
//! become inactive tombstones, and nothing else starts. With companions the
//! credential is checked before any save I/O, the aggregate is loaded,
//! merged with the configured anchor bodies, and saved durably when the merge
//! changed it, then the Agent wire, namespace lease controller, snapshot
//! registry, and MCP service start, and only then does the authority receive
//! the complete aggregate. Every failure stops startup and retires whatever
//! this function started; the Python Agent process is never launched here.
//!
//! The per-tick plan loop (planning snapshot, outcome install, task runner,
//! task timeout) is not assembled here.

use std::fmt;
use std::net::TcpListener;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use mornlea_domain::{ChunkPos, CompanionId, CompanionName};
use mornlea_storage::{CompanionMergeError, CompanionSave, PlayerId, StoredCompanions};

use super::config::AiConfig;
use crate::agent::http::AgentHttpWire;
use crate::agent::lease::{ControlPhase, LeaseConfig, LeaseController};
use crate::agent::mcp::{FrozenTools, MCP_ENDPOINT_PATH, McpService};
use crate::agent::snapshot::{SnapshotRegistry, SystemEntropy};
use crate::contracts::{
    AgentHandle, ClientInstanceId, Clock, Deadline, DiskBackend, LoadedValue, NamespaceId,
    Operation, OwnedSnapshot, SaveKey, SaveRequest, SaveTicket, SaveUrgency, SaveValue,
    ServerError,
};
use crate::core::anchor_body;
use crate::state::AuthorityState;

/// Bound for retiring Agent services after a failed startup or on drop.
pub const AGENT_CLOSE_TIMEOUT: Duration = Duration::from_secs(5);

/// Injected startup capabilities. Production passes [`SystemClock`],
/// [`env_credential`], and [`system_identity`]; tests inject deterministic
/// doubles.
pub struct CompanionStartupPorts<'a> {
    /// Clock shared by the Agent wire, lease controller, and snapshot registry.
    pub clock: Arc<dyn Clock + Send + Sync>,
    /// Reads the credential named by `ai.agentService.apiKeyEnv`.
    pub credential: &'a dyn Fn(&str) -> Option<String>,
    /// Mints raw UUIDv4 bytes for save lifecycle identities and the Agent
    /// client instance.
    pub identity: &'a mut dyn FnMut() -> Result<[u8; 16], ServerError>,
}

/// Process clock: real monotonic time and wall-clock unix milliseconds.
pub struct SystemClock;

impl Clock for SystemClock {
    fn monotonic(&self) -> Instant {
        Instant::now()
    }

    fn unix_ms(&self) -> i64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .ok()
            .and_then(|elapsed| i64::try_from(elapsed.as_millis()).ok())
            .unwrap_or(0)
    }
}

/// Go `os.Getenv`: an unset or empty variable yields no credential.
pub fn env_credential(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|value| !value.is_empty())
}

/// Operating-system entropy shaped into a random UUIDv4.
pub fn system_identity() -> Result<[u8; 16], ServerError> {
    let mut bytes = [0u8; 16];
    getrandom::fill(&mut bytes).map_err(|_| ServerError::Internal {
        invariant: "companion identity entropy",
    })?;
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    Ok(bytes)
}

/// Typed startup refusal. No variant carries the credential value.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CompanionStartError {
    /// The credential variable is unset or empty; nothing was read or written.
    MissingCredential,
    /// The companion aggregate could not be loaded (corrupt, future, I/O).
    Load(ServerError),
    /// The configured merge refused the stored aggregate.
    Merge(String),
    /// Identity entropy failed.
    Identity(ServerError),
    /// The merged aggregate could not be saved durably.
    Save(ServerError),
    /// An Agent service failed to start.
    Agent(ServerError),
    /// The authority refused the complete aggregate.
    Authority(ServerError),
}

impl fmt::Display for CompanionStartError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingCredential => {
                write!(f, "companion startup: Agent credential variable is empty")
            }
            Self::Load(error) => write!(f, "companion startup: load companions: {error:?}"),
            Self::Merge(detail) => write!(f, "companion startup: merge companions: {detail}"),
            Self::Identity(error) => write!(f, "companion startup: identity: {error:?}"),
            Self::Save(error) => write!(f, "companion startup: save companions: {error:?}"),
            Self::Agent(error) => write!(f, "companion startup: start Agent services: {error:?}"),
            Self::Authority(error) => write!(f, "companion startup: authority handoff: {error:?}"),
        }
    }
}

impl std::error::Error for CompanionStartError {}

/// Started Agent services owned by the companion runtime.
pub struct AgentServices {
    client: ClientInstanceId,
    namespace: NamespaceId,
    lease: LeaseController,
    snapshots: SnapshotRegistry,
    mcp: McpService,
    closed: bool,
}

impl AgentServices {
    /// Client instance identity presented on every Agent request.
    pub fn client(&self) -> ClientInstanceId {
        self.client
    }

    /// Agent namespace taken from the durable companion aggregate.
    pub fn namespace(&self) -> NamespaceId {
        self.namespace
    }

    /// Current namespace lease and fence, if the control worker holds one.
    pub fn current_lease(&self) -> Option<(crate::contracts::LeaseId, u64)> {
        self.lease.current_lease()
    }

    /// Lease control machine phase.
    pub fn lease_phase(&self) -> ControlPhase {
        self.lease.control_phase()
    }

    /// Whether the lease control worker is still running.
    pub fn lease_worker_running(&self) -> bool {
        self.lease.pending_control_worker()
    }

    /// Loopback MCP endpoint the Agent calls back into.
    pub fn mcp_endpoint(&self) -> &str {
        self.mcp.endpoint()
    }

    /// Snapshot registry shared with the MCP service.
    pub fn snapshots(&self) -> &SnapshotRegistry {
        &self.snapshots
    }

    /// Stops MCP admission and the snapshot registry, then closes the lease
    /// controller and its control worker, all within one absolute deadline.
    pub fn close_until(&mut self, deadline: Deadline) -> Result<(), ServerError> {
        self.mcp.close_until(deadline)?;
        self.lease.close(deadline)?;
        self.closed = true;
        Ok(())
    }
}

impl Drop for AgentServices {
    fn drop(&mut self) {
        if !self.closed {
            let _ = self.close_until(close_deadline());
        }
    }
}

/// Configured companion runtime after a successful startup.
pub struct CompanionRuntime {
    definitions: Vec<(CompanionId, CompanionName)>,
    task_timeout_minutes: u32,
    agent: AgentServices,
}

impl CompanionRuntime {
    /// Configured companions handed to the authority, in file order.
    pub fn definitions(&self) -> &[(CompanionId, CompanionName)] {
        &self.definitions
    }

    /// Frozen task timeout in minutes; no task runner consumes it yet.
    pub fn task_timeout_minutes(&self) -> u32 {
        self.task_timeout_minutes
    }

    /// Started Agent services.
    pub fn agent(&self) -> &AgentServices {
        &self.agent
    }

    /// Mutable Agent services, for an explicit close.
    pub fn agent_mut(&mut self) -> &mut AgentServices {
        &mut self.agent
    }
}

/// Runs the configured companion startup against an unstarted authority.
///
/// `disk` must be the world store before it moves into the save scheduler,
/// and `state` must already own live chunks, actor saves, and player
/// persistence with no tick run. Returns `Ok(None)` when no companion is
/// configured.
pub fn start_companions(
    ai: Option<&AiConfig>,
    disk: &mut dyn DiskBackend,
    state: &mut AuthorityState,
    ports: CompanionStartupPorts<'_>,
) -> Result<Option<CompanionRuntime>, CompanionStartError> {
    let CompanionStartupPorts {
        clock,
        credential,
        identity,
    } = ports;
    let Some(ai) = ai.filter(|ai| !ai.companions().is_empty()) else {
        retire_unconfigured(disk, identity)?;
        return Ok(None);
    };
    // The static Agent boundary precedes any companion save read, so a bad
    // credential or an undialable endpoint never triggers I/O.
    let credential = credential(ai.api_key_env())
        .filter(|value| !value.is_empty())
        .ok_or(CompanionStartError::MissingCredential)?;
    let wire = AgentHttpWire::try_new(ai.endpoint(), &credential, clock.clone())
        .map_err(CompanionStartError::Agent)?;

    let loaded = load_companions(disk)?.unwrap_or_default();
    let anchor = spawn_anchor(state)?;
    let active: Vec<_> = ai
        .companions()
        .iter()
        .map(|(id, _)| anchor_body(*id, anchor))
        .collect();
    let durable = merge_and_save(disk, &loaded, &active, identity)?;

    let client =
        ClientInstanceId::try_from_bytes(identity().map_err(CompanionStartError::Identity)?)
            .map_err(CompanionStartError::Identity)?;
    let namespace = NamespaceId::try_from_bytes(durable.agent_namespace_id.to_bytes())
        .map_err(CompanionStartError::Agent)?;
    let mut agent = start_agent(wire, client, namespace, clock)?;
    if let Err(error) = state.enable_companion_persistence(ai.companions(), durable) {
        let _ = agent.close_until(close_deadline());
        return Err(CompanionStartError::Authority(error));
    }
    Ok(Some(CompanionRuntime {
        definitions: ai.companions().to_vec(),
        task_timeout_minutes: ai.task_timeout_minutes(),
        agent,
    }))
}

/// Go `bootstrapCompanionPersistence` with no configured companion: an
/// existing aggregate is merged with no active body so formerly configured
/// companions become tombstones, saved only when that changed it.
fn retire_unconfigured(
    disk: &mut dyn DiskBackend,
    identity: &mut dyn FnMut() -> Result<[u8; 16], ServerError>,
) -> Result<(), CompanionStartError> {
    if let Some(loaded) = load_companions(disk)? {
        merge_and_save(disk, &loaded, &[], identity)?;
    }
    Ok(())
}

/// Loads the aggregate; a missing file is `None`, every other failure stops.
fn load_companions(
    disk: &mut dyn DiskBackend,
) -> Result<Option<StoredCompanions>, CompanionStartError> {
    match disk.load(SaveKey::Companions) {
        Ok(LoadedValue::Companions(loaded)) => Ok(Some(loaded)),
        Ok(_) => Err(CompanionStartError::Load(ServerError::Internal {
            invariant: "companion load family",
        })),
        Err(ServerError::Io {
            operation: Operation::Load,
            kind: std::io::ErrorKind::NotFound,
        }) => Ok(None),
        Err(error) => Err(CompanionStartError::Load(error)),
    }
}

/// World spawn anchor column from the authority's frozen metadata.
fn spawn_anchor(state: &AuthorityState) -> Result<ChunkPos, CompanionStartError> {
    match &state.metadata_snapshot().value {
        SaveValue::Metadata(metadata) => Ok(ChunkPos::new(
            metadata.spawn_anchor.x,
            metadata.spawn_anchor.z,
        )),
        _ => Err(CompanionStartError::Authority(ServerError::Internal {
            invariant: "companion spawn anchor",
        })),
    }
}

/// Merges the configured bodies into the stored aggregate and, when the
/// merge changed it, saves the result durably before returning it.
fn merge_and_save(
    disk: &mut dyn DiskBackend,
    loaded: &StoredCompanions,
    active: &[mornlea_storage::CompanionBody],
    identity: &mut dyn FnMut() -> Result<[u8; 16], ServerError>,
) -> Result<StoredCompanions, CompanionStartError> {
    let mut generate = || identity().map(PlayerId::from_bytes);
    let (merged, changed) = mornlea_storage::merge_companions_v5(
        loaded,
        active,
        Some(&mut generate),
    )
    .map_err(|error| match error {
        CompanionMergeError::Identity(error) => CompanionStartError::Identity(error),
        CompanionMergeError::Storage(error) => CompanionStartError::Merge(error.to_string()),
    })?;
    if changed {
        save_companions(disk, &merged)?;
    }
    Ok(merged)
}

fn save_companions(
    disk: &mut dyn DiskBackend,
    merged: &StoredCompanions,
) -> Result<(), CompanionStartError> {
    let save = CompanionSave {
        revision: merged.revision,
        agent_namespace_id: merged.agent_namespace_id,
        records: merged.records.clone(),
        lifecycles: merged.lifecycles.clone(),
        queues: merged.queues.clone(),
    };
    let bytes = mornlea_storage::companions_encoded_len(&save).map_err(|_| {
        CompanionStartError::Save(ServerError::InvalidInput {
            field: "companions",
        })
    })?;
    let snapshot = OwnedSnapshot::try_new(
        SaveKey::Companions,
        merged.revision,
        bytes,
        SaveUrgency::Autosave,
        SaveValue::Companions(save),
    )
    .map_err(CompanionStartError::Save)?;
    let ticket = SaveTicket::try_from_raw(1).map_err(CompanionStartError::Save)?;
    let completion = disk.write(
        ticket,
        SaveRequest {
            snapshots: vec![snapshot],
        },
    );
    if let Some(error) = completion.error {
        return Err(CompanionStartError::Save(error));
    }
    if completion.committed != [(SaveKey::Companions, merged.revision)] {
        return Err(CompanionStartError::Save(ServerError::Internal {
            invariant: "companion bootstrap commit",
        }));
    }
    Ok(())
}

/// Go `NewHost` Agent wiring: the namespace lease controller starts its
/// control worker immediately, then the snapshot registry and the loopback
/// MCP service. A refused acquire stays on the worker and never surfaces
/// here; only constructor failures stop startup, after retiring what
/// already started.
fn start_agent(
    wire: AgentHttpWire,
    client: ClientInstanceId,
    namespace: NamespaceId,
    clock: Arc<dyn Clock + Send + Sync>,
) -> Result<AgentServices, CompanionStartError> {
    let mut lease = LeaseController::try_new(
        LeaseConfig {
            client_instance_id: client,
            namespace_id: namespace,
        },
        Arc::new(wire),
        clock.clone(),
    )
    .map_err(CompanionStartError::Agent)?;
    let close_lease = |lease: &mut LeaseController, error: ServerError| {
        let _ = lease.close(close_deadline());
        CompanionStartError::Agent(error)
    };
    if let Err(error) = lease.spawn_control_worker() {
        return Err(close_lease(&mut lease, error));
    }
    let listener = match TcpListener::bind("127.0.0.1:0") {
        Ok(listener) => listener,
        Err(_) => {
            return Err(close_lease(
                &mut lease,
                ServerError::InvalidInput {
                    field: "mcp_listen",
                },
            ));
        }
    };
    let endpoint = match listener.local_addr() {
        Ok(address) => format!("http://{address}{MCP_ENDPOINT_PATH}"),
        Err(_) => {
            return Err(close_lease(
                &mut lease,
                ServerError::InvalidInput {
                    field: "mcp_listen",
                },
            ));
        }
    };
    let snapshots = match SnapshotRegistry::try_new(clock, Arc::new(SystemEntropy::new()), endpoint)
    {
        Ok(snapshots) => snapshots,
        Err(error) => return Err(close_lease(&mut lease, error)),
    };
    let mcp = match McpService::try_new_with(snapshots.clone(), Arc::new(FrozenTools), listener) {
        Ok(mcp) => mcp,
        Err(error) => {
            snapshots.close_shared();
            return Err(close_lease(&mut lease, error));
        }
    };
    Ok(AgentServices {
        client,
        namespace,
        lease,
        snapshots,
        mcp,
        closed: false,
    })
}

fn close_deadline() -> Deadline {
    let now = Instant::now();
    Deadline::after(now, AGENT_CLOSE_TIMEOUT).unwrap_or_else(|_| Deadline::at(now))
}
