//! Shared harness for the cross-language Agent tests: fixture identities,
//! the disposable helper child, and readiness gates.
//!
//! Production server code never launches Python. This module spawns the
//! read-only helper with an explicit interpreter fixture, an inherited
//! listener on descriptor three, and exact standard input, then gates on the
//! ready line plus the live and ready probes before any business traffic.

use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::os::unix::io::AsRawFd;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use mornlea_domain::{
    BlockPos, ChunkPos, CommandText, CompanionId, FiniteVec3, LookAngles, PlayerId,
};
use mornlea_server::agent::host::{CurrentWorld, DrainReport, PlanHost};
use mornlea_server::agent::http::{AgentHttpWire, parse_canonical_uuid};
use mornlea_server::agent::lease::{LeaseConfig, LeaseController};
use mornlea_server::agent::mcp::{FrozenTools, MCP_ENDPOINT_PATH, McpService, PlanningTools};
use mornlea_server::agent::snapshot::{SnapshotEntropy, SnapshotRegistry};
use mornlea_server::contracts::{
    AgentHandle, AgentPoll, AgentRequestId, ClientInstanceId, Clock, NamespaceId, OperationId,
    PlanningSnapshot, RunId, ServerError, SnapshotBlock, SnapshotChunkRevision, SnapshotCompanion,
    SnapshotIssuer, SnapshotPlayer, SnapshotTaskStatusText, SnapshotTerrain,
};
use mornlea_storage::ItemStack;
use sha2::{Digest, Sha256};

/// Issuer identity shared with the cross-language fixtures.
pub const ISSUER_ID: &str = "99999999-9999-4999-8999-999999999999";
/// Companion identity shared with the cross-language fixtures.
pub const COMPANION_ID: &str = "66666666-6666-4666-8666-666666666666";
/// Client identity shared with the cross-language fixtures.
pub const CLIENT_ID: &str = "22222222-2222-4222-8222-222222222222";
/// Namespace identity shared with the cross-language fixtures.
pub const NAMESPACE_ID: &str = "4d5e6f70-8192-4aa3-8b4f-3a4b5c6d7e8f";
/// Ready line the helper prints once its inherited listener serves.
const READY_LINE: &str = r#"{"status":"ready"}"#;
/// Bound for the ready line plus both probe gates after spawn.
pub const READINESS_TIMEOUT: Duration = Duration::from_secs(15);
/// Bound for killing and reaping the helper on teardown.
pub const TEARDOWN_TIMEOUT: Duration = Duration::from_secs(5);

/// Step clock the harness holds fixed; business RPCs use real loopback I/O,
/// so the clock only fences lease and snapshot lifetimes.
pub struct StepClock {
    now: Mutex<Instant>,
}

impl StepClock {
    /// Freezes one clock at the current instant with its start returned for
    /// expiry arithmetic.
    pub fn start() -> (Instant, Arc<Self>) {
        let now = Instant::now();
        (
            now,
            Arc::new(Self {
                now: Mutex::new(now),
            }),
        )
    }

    /// Moves the frozen instant forward for expiry and timeout cases.
    pub fn set(&self, instant: Instant) {
        *self.now.lock().expect("clock") = instant;
    }
}

impl Clock for StepClock {
    fn monotonic(&self) -> Instant {
        *self.now.lock().expect("clock")
    }

    fn unix_ms(&self) -> i64 {
        1_800_000_000_000
    }
}

/// Deterministic entropy cycling bytes `0, 1, ...`, mirroring the other
/// snapshot harnesses.
pub struct CycleEntropy {
    next: Mutex<u8>,
}

impl CycleEntropy {
    /// Starts the byte cycle at zero.
    pub fn zero() -> Arc<Self> {
        Arc::new(Self {
            next: Mutex::new(0),
        })
    }
}

impl SnapshotEntropy for CycleEntropy {
    fn fill(&self, out: &mut [u8; 32]) -> Result<(), ServerError> {
        let mut next = self.next.lock().expect("entropy");
        for slot in out.iter_mut() {
            *slot = *next;
            *next = next.wrapping_add(1);
        }
        Ok(())
    }
}

fn uuid_bytes(text: &str, what: &str) -> [u8; 16] {
    parse_canonical_uuid(text).unwrap_or_else(|| panic!("fixture identity parses: {what}"))
}

/// Issuer player identity of the planning fixture.
pub fn issuer_id() -> PlayerId {
    PlayerId::try_from_bytes(uuid_bytes(ISSUER_ID, "issuer")).expect("issuer id")
}

/// Companion identity of the planning fixture.
pub fn companion_id() -> CompanionId {
    CompanionId::try_from_bytes(uuid_bytes(COMPANION_ID, "companion")).expect("companion id")
}

/// Client instance identity presented to the helper gateway.
pub fn client_id() -> ClientInstanceId {
    ClientInstanceId::try_from_bytes(uuid_bytes(CLIENT_ID, "client")).expect("client id")
}

/// Namespace identity presented to the helper gateway.
pub fn namespace_id() -> NamespaceId {
    NamespaceId::try_from_bytes(uuid_bytes(NAMESPACE_ID, "namespace")).expect("namespace id")
}

fn tag_bytes(tag: u8) -> [u8; 16] {
    let mut bytes = [0u8; 16];
    bytes[0] = tag;
    bytes[6] = 0x40;
    bytes[8] = 0x80;
    bytes
}

/// Caller-minted business request identity.
pub fn request_id(tag: u8) -> AgentRequestId {
    AgentRequestId::try_from_bytes(tag_bytes(tag)).expect("request id")
}

/// Caller-minted run identity.
pub fn run_id(tag: u8) -> RunId {
    RunId::try_from_bytes(tag_bytes(tag)).expect("run id")
}

/// Caller-minted memory operation identity.
pub fn operation(tag: u8) -> OperationId {
    OperationId::try_from_bytes(tag_bytes(tag)).expect("operation id")
}

/// Business deadline a minute past the real clock so the helper gateway
/// treats every integration request as live.
pub fn wall_deadline_ms() -> i64 {
    let now_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("wall clock")
        .as_millis();
    i64::try_from(now_ms).expect("wall millis") + 60_000
}

/// Disposable scratch directory removed on drop.
pub struct TempDir {
    path: PathBuf,
}

impl TempDir {
    /// Creates an empty scratch directory tagged for the calling case.
    pub fn new(tag: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "mornlea-agent-process-{}-{tag}",
            std::process::id()
        ));
        if path.exists() {
            std::fs::remove_dir_all(&path).expect("clear stale temp dir");
        }
        std::fs::create_dir_all(&path).expect("create temp dir");
        Self { path }
    }

    /// Joins one disposable file name inside the scratch directory.
    pub fn join(&self, name: &str) -> PathBuf {
        self.path.join(name)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

/// Escapes one JSON string value without pulling in a serializer.
fn json_string(value: &str) -> String {
    let mut text = String::with_capacity(value.len() + 2);
    text.push('"');
    for unit in value.encode_utf16() {
        match unit {
            0x22 => text.push_str("\\\""),
            0x5C => text.push_str("\\\\"),
            0x0A => text.push_str("\\n"),
            0x0D => text.push_str("\\r"),
            0x09 => text.push_str("\\t"),
            0x00..=0x1F => text.push_str(&format!("\\u{unit:04x}")),
            _ => text.push(char::from_u32(u32::from(unit)).expect("string fragment")),
        }
    }
    text.push('"');
    text
}

/// Hex digest of one file; hashing follows the interpreter symlink to the
/// executed bytes.
fn sha256_file(path: &Path, what: &str) -> String {
    let bytes = std::fs::read(path).unwrap_or_else(|_| panic!("rebuild identity reads {what}"));
    format!("{:x}", Sha256::digest(bytes))
}

/// Explicit interpreter fixture: a missing variable or executable fails the
/// case, never skips it.
fn python_fixture() -> PathBuf {
    let text = std::env::var("MORNLEA_AGENT_PYTHON")
        .expect("explicit MORNLEA_AGENT_PYTHON fixture names the helper interpreter");
    let path = PathBuf::from(text);
    assert!(
        path.is_file(),
        "helper interpreter is missing: {}",
        path.display()
    );
    path
}

fn repository_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(4)
        .expect("repository root")
        .to_path_buf()
}

/// Disposable helper child with its scratch directory, source identity, and
/// drained standard error.
pub struct ChildOwner {
    child: Child,
    pid: u32,
    port: u16,
    marker: PathBuf,
    stderr: Option<std::thread::JoinHandle<Vec<u8>>>,
    /// Pins the scratch directory for the child lifetime.
    _temp: TempDir,
    source_sha: String,
    executable_sha: String,
    started: Instant,
    reaped: bool,
}

impl ChildOwner {
    /// Loopback endpoint of the helper gateway.
    pub fn endpoint(&self) -> String {
        format!("http://127.0.0.1:{}", self.port)
    }

    /// Barrier file the helper model writes once a blocking call is admitted.
    pub fn marker(&self) -> &Path {
        &self.marker
    }

    /// Kills and reaps the child within the teardown bound, then reports the
    /// start, stop, and source identities.
    pub fn shutdown(&mut self, what: &str) {
        let stopwatch = Instant::now();
        let _ = self.child.kill();
        let mut status = None;
        while status.is_none() && stopwatch.elapsed() < TEARDOWN_TIMEOUT {
            match self.child.try_wait() {
                Ok(done) => status = done,
                Err(_) => break,
            }
            if status.is_none() {
                std::thread::sleep(Duration::from_millis(10));
            }
        }
        let stderr_bytes = if status.is_some() {
            self.stderr
                .take()
                .map(|reader| reader.join().unwrap_or_default())
                .unwrap_or_default()
        } else {
            Vec::new()
        };
        self.reaped = status.is_some();
        if !stderr_bytes.is_empty() {
            eprintln!(
                "agent child {what} stderr: {}",
                String::from_utf8_lossy(&stderr_bytes)
            );
        }
        println!(
            "agent child {what} stop: pid={} status={:?} stop_ms={} uptime_ms={} stderr_bytes={} source_sha256={} executable_sha256={}",
            self.pid,
            status,
            stopwatch.elapsed().as_millis(),
            self.started.elapsed().as_millis(),
            stderr_bytes.len(),
            self.source_sha,
            self.executable_sha,
        );
        assert!(
            status.is_some(),
            "helper child was not reaped within five seconds"
        );
    }
}

impl Drop for ChildOwner {
    fn drop(&mut self) {
        if !self.reaped {
            let _ = self.child.kill();
            // A panic skips `shutdown`, so report the child state here: a
            // helper that already exited explains refused business RPCs.
            match self.child.try_wait() {
                Ok(Some(status)) => eprintln!("agent child unreaped exit: {status}"),
                Ok(None) => eprintln!("agent child unreaped: still running"),
                Err(error) => eprintln!("agent child unreaped wait error: {error}"),
            }
            // A panic skips `shutdown`, so drain the helper log here: the
            // uvicorn request lines name the exact failing leg server-side.
            if let Some(reader) = self.stderr.take() {
                let bytes = reader.join().unwrap_or_default();
                if !bytes.is_empty() {
                    eprintln!(
                        "agent child unreaped stderr: {}",
                        String::from_utf8_lossy(&bytes)
                    );
                }
            }
        }
    }
}

fn parse_response(raw: &[u8]) -> Option<(u16, Vec<u8>)> {
    let split = raw.windows(4).position(|window| window == b"\r\n\r\n")?;
    let head = std::str::from_utf8(&raw[..split]).ok()?;
    let status = head
        .lines()
        .next()?
        .split_whitespace()
        .nth(1)?
        .parse::<u16>()
        .ok()?;
    Some((status, raw[split + 4..].to_vec()))
}

fn probe_once(addr: &str, path: &str, token: Option<&str>) -> Option<(u16, Vec<u8>)> {
    let mut stream = TcpStream::connect(addr).ok()?;
    stream.set_read_timeout(Some(Duration::from_secs(2))).ok()?;
    let mut request = format!("GET {path} HTTP/1.1\r\nHost: {addr}\r\n");
    if let Some(token) = token {
        request.push_str(&format!("Authorization: Bearer {token}\r\n"));
    }
    request.push_str("Connection: close\r\n\r\n");
    stream.write_all(request.as_bytes()).ok()?;
    let mut raw = Vec::new();
    let mut chunk = [0u8; 4096];
    loop {
        match stream.read(&mut chunk) {
            Ok(0) => break,
            Ok(read) => raw.extend_from_slice(&chunk[..read]),
            Err(_) => break,
        }
    }
    parse_response(&raw)
}

fn await_probe(
    deadline: Instant,
    addr: &str,
    path: &str,
    token: Option<&str>,
    marker: &str,
    want: &str,
) {
    loop {
        if let Some((status, body)) = probe_once(addr, path, token)
            && status == 200
            && String::from_utf8_lossy(&body).contains(want)
        {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "{marker} probe never settled on {addr}{path}"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// Spawns the helper with an inherited listener on descriptor three, exact
/// standard input, and scrubbed proxy state, then gates on the ready line
/// plus the live and authenticated ready probes.
pub fn spawn_helper(tag: &str, token: &str) -> ChildOwner {
    let started = Instant::now();
    let deadline = started + READINESS_TIMEOUT;
    let python = python_fixture();
    let root = repository_root();
    let helper = root.join("packages/agent/tests/integration/process.py");
    assert!(helper.is_file(), "helper source is missing");
    let source_sha = sha256_file(&helper, "helper source");
    let executable_sha = sha256_file(&python, "helper interpreter");

    let temp = TempDir::new(tag);
    let sqlite = temp.join("memory.sqlite3");
    let marker = temp.join("model-ready");
    let agent_dir = root.join("packages/agent");

    let gateway = TcpListener::bind("127.0.0.1:0").expect("gateway listener");
    let port = gateway.local_addr().expect("gateway addr").port();
    let stdin = format!(
        "{{\"block_ready_path\":{},\"http_bearer_token\":{},\"port\":{},\"sqlite_path\":{}}}",
        json_string(&marker.to_string_lossy()),
        json_string(token),
        port,
        json_string(&sqlite.to_string_lossy()),
    );

    let raw = gateway.as_raw_fd();
    let mut command = Command::new(&python);
    command
        .arg(&helper)
        .arg("http-server")
        .current_dir(&agent_dir)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .env("PYTHONUNBUFFERED", "1")
        .env("HTTP_PROXY", "")
        .env("HTTPS_PROXY", "")
        .env("ALL_PROXY", "")
        .env("NO_PROXY", "*");
    unsafe {
        command.pre_exec(move || {
            if raw != 3 {
                if libc::dup2(raw, 3) == -1 {
                    return Err(std::io::Error::last_os_error());
                }
                libc::close(raw);
            }
            // A no-op self duplication keeps the close-on-exec flag, so
            // clear it explicitly: descriptor three must survive `exec`.
            let flags = libc::fcntl(3, libc::F_GETFD);
            if flags == -1 {
                return Err(std::io::Error::last_os_error());
            }
            if libc::fcntl(3, libc::F_SETFD, flags & !libc::FD_CLOEXEC) == -1 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let mut child = command.spawn().expect("spawn helper child");
    drop(gateway);
    child
        .stdin
        .take()
        .expect("piped stdin")
        .write_all(stdin.as_bytes())
        .expect("helper stdin");
    let pid = child.id();

    let stdout = child.stdout.take().expect("piped stdout");
    let (ready_tx, ready_rx) = mpsc::channel();
    std::thread::spawn(move || {
        let mut reader = BufReader::new(stdout);
        let mut line = String::new();
        let result = reader.read_line(&mut line).map(|_| line);
        let _ = ready_tx.send(result);
    });
    let remaining = deadline.saturating_duration_since(Instant::now());
    let line = ready_rx
        .recv_timeout(remaining)
        .expect("helper ready line")
        .expect("helper stdout");
    assert_eq!(
        line.trim_end(),
        READY_LINE,
        "helper readiness line names the exact status"
    );

    let addr = format!("127.0.0.1:{port}");
    await_probe(
        deadline,
        &addr,
        "/livez",
        None,
        "live",
        r#""status":"live""#,
    );
    await_probe(
        deadline,
        &addr,
        "/readyz",
        Some(token),
        "ready",
        r#""status":"ready""#,
    );

    let stderr = child.stderr.take().expect("piped stderr");
    let stderr_reader = std::thread::spawn(move || {
        let mut bytes = Vec::new();
        let mut locked = stderr;
        let _ = locked.read_to_end(&mut bytes);
        bytes
    });

    println!(
        "agent child {tag} start: pid={pid} port={port} source_sha256={source_sha} executable_sha256={executable_sha}",
    );
    ChildOwner {
        child,
        pid,
        port,
        marker,
        stderr: Some(stderr_reader),
        _temp: temp,
        source_sha,
        executable_sha,
        started,
        reaped: false,
    }
}

/// Serves the frozen planning tools on a fresh loopback listener and freezes
/// a registry pointing at it, so the helper planner calls back into Rust.
pub fn serve_mcp(clock: &Arc<StepClock>) -> (SnapshotRegistry, McpService) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("mcp listener");
    let endpoint = format!(
        "http://{}{MCP_ENDPOINT_PATH}",
        listener.local_addr().expect("mcp addr")
    );
    let registry = SnapshotRegistry::try_new(clock.clone(), CycleEntropy::zero(), endpoint)
        .expect("snapshot registry");
    let tools: Arc<dyn PlanningTools> = Arc::new(FrozenTools);
    let service = McpService::try_new_with(registry.clone(), tools, listener).expect("mcp service");
    (registry, service)
}

/// Acquires one lease from the helper gateway over the real wire.
pub fn acquire_lease(clock: &Arc<StepClock>, endpoint: &str, token: &str) -> LeaseController {
    let wire = AgentHttpWire::try_new(endpoint, token, clock.clone()).expect("loopback wire");
    let agent = LeaseController::try_new(
        LeaseConfig {
            client_instance_id: client_id(),
            namespace_id: namespace_id(),
        },
        Arc::new(wire),
        clock.clone(),
    )
    .expect("lease controller");
    agent.refresh();
    assert!(
        agent.current_lease().is_some(),
        "helper gateway admits the acquire"
    );
    agent
}

/// Mine target shared by the frozen snapshot and the current world.
pub fn target_pos() -> BlockPos {
    BlockPos::new(8, 63, -2)
}

fn target_chunk() -> ChunkPos {
    ChunkPos::new(0, -1)
}

fn finite(values: [f32; 3]) -> FiniteVec3 {
    FiniteVec3::try_new(values).expect("fixture position")
}

fn angles(yaw: f32, pitch: f32) -> LookAngles {
    LookAngles::try_new(yaw, pitch).expect("fixture look")
}

/// Inventory shared by the fixture: stone in slot zero, chest in slot four.
fn fixture_inventory() -> [ItemStack; 36] {
    let mut inventory = [ItemStack {
        item: 0,
        count: 0,
        durability: 0,
    }; 36];
    inventory[0] = ItemStack {
        item: 1,
        count: 64,
        durability: 0,
    };
    inventory[4] = ItemStack {
        item: 14,
        count: 1,
        durability: 0,
    };
    inventory
}

/// Frozen planning snapshot carrying stone at the mine target with full
/// dense terrain, mirroring the cross-language contract fixture.
pub fn plan_snapshot(instruction: &str, source_tick: u64) -> PlanningSnapshot {
    let origin = BlockPos::new(-11, 56, -18);
    let mut ready = vec![0u8; 137];
    for bit in 0..1089 {
        ready[bit / 8] |= 1 << (bit % 8);
    }
    let heights = vec![63i16; 1089];
    let mut blocks = vec![0u16; 18_513];
    let dx = (target_pos().x() - origin.x()) as usize;
    let dy = (target_pos().y() - origin.y()) as usize;
    let dz = (target_pos().z() - origin.z()) as usize;
    blocks[(dx * 17 + dy) * 33 + dz] = 2;
    PlanningSnapshot::try_new(
        source_tick,
        1200,
        CommandText::try_from_canonical(instruction.to_owned()).expect("snapshot instruction"),
        SnapshotIssuer {
            player_id: issuer_id(),
            position: finite([4.5, 64.0, -1.5]),
            look: angles(90.0, 0.0),
            look_hit: Some(target_pos()),
        },
        SnapshotCompanion {
            companion_id: companion_id(),
            position: finite([5.5, 64.0, -1.5]),
            look: angles(0.0, 0.0),
            task_status: SnapshotTaskStatusText::try_new("规划中".to_owned()).expect("task status"),
            inventory: fixture_inventory(),
        },
        vec![SnapshotPlayer {
            player_id: issuer_id(),
            position: finite([4.5, 64.0, -1.5]),
            look: angles(90.0, 0.0),
            look_hit: Some(target_pos()),
        }],
        vec![SnapshotChunkRevision {
            pos: target_chunk(),
            revision: 17,
        }],
        vec![SnapshotBlock {
            position: target_pos(),
            block_id: 2,
        }],
        SnapshotTerrain::try_new(origin, [33, 17, 33], ready, heights, blocks)
            .expect("fixture terrain"),
    )
    .expect("fixture snapshot")
}

/// Current world matching the frozen snapshot exactly.
pub fn plan_world(tick: u64) -> CurrentWorld {
    let mut blocks = BTreeMap::new();
    blocks.insert(target_pos(), 2);
    let mut chunk_revisions = BTreeMap::new();
    chunk_revisions.insert(target_chunk(), 17);
    let mut online_players = BTreeMap::new();
    online_players.insert(issuer_id(), [4.5, 64.0, -1.5]);
    CurrentWorld {
        tick,
        blocks,
        chunk_revisions,
        inventory: fixture_inventory(),
        online_players,
    }
}

/// Waits for the blocking-model barrier while also watching the dispatched
/// plan: a helper that settles the run early (a fast rejection rather than
/// an admitted block) surfaces its report and raw poll at once instead of
/// burning the whole bound behind a blind barrier wait. A still-pending
/// plan drains without side effects, so the barrier semantics are unchanged.
/// The nonblocking serve-loop probe names a dead MCP listener, which would
/// refuse the helper's planning calls.
#[allow(clippy::too_many_arguments)]
pub fn await_block_marker(
    host: &mut PlanHost,
    agent: &mut LeaseController,
    snapshots: &mut SnapshotRegistry,
    mcp: &McpService,
    clock: &Arc<StepClock>,
    marker: &Path,
    request_id: AgentRequestId,
    timeout: Duration,
    what: &str,
) {
    let deadline = Instant::now() + timeout;
    loop {
        if marker.exists() {
            return;
        }
        let report = host
            .drain_outcomes(agent, snapshots, &**clock)
            .expect("barrier wait polls the dispatched plan");
        assert!(
            report.completed == 0 && report.failed == 0,
            "model barrier missed for {what}: plan settled early report={report:?} failures={:?} poll={:?} mcp_done={:?}",
            host.take_failures(),
            agent.poll(request_id),
            mcp.wait_done(Duration::from_millis(0)),
        );
        assert!(
            Instant::now() < deadline,
            "model barrier was never reached for {what}"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// Drains the host until one outcome settles or the bound expires.
pub fn drain_plan_until(
    host: &mut PlanHost,
    agent: &mut LeaseController,
    snapshots: &mut SnapshotRegistry,
    clock: &Arc<StepClock>,
    timeout: Duration,
) -> DrainReport {
    let deadline = Instant::now() + timeout;
    loop {
        let report = host
            .drain_outcomes(agent, snapshots, &**clock)
            .expect("drain polls");
        if report.completed > 0 || report.failed > 0 {
            return report;
        }
        assert!(Instant::now() < deadline, "plan outcome never settled");
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// Polls one admitted business request until it settles or the bound expires.
pub fn poll_request_until(
    agent: &mut LeaseController,
    id: AgentRequestId,
    timeout: Duration,
) -> AgentPoll {
    let deadline = Instant::now() + timeout;
    loop {
        let poll = agent.poll(id);
        if !matches!(poll, AgentPoll::Pending) {
            return poll;
        }
        assert!(Instant::now() < deadline, "business request never settled");
        std::thread::sleep(Duration::from_millis(5));
    }
}
