//! Full-corpus integration: inventory digests, logical replay parity, and the
//! live Agent admission.
//!
//! The first case replays the complete provider inventory at the digest
//! level: every row of the sealed capability inventory names a Go source byte
//! range with a recorded SHA-256, and the case recomputes each distinct range
//! over the current tree. A drifted Go fixture fails here, not in a provider.
//! The second case replays one two-player transcript twice through the real
//! reducer and compares only logical state and ordered events, never a
//! private Rust layout. The third case admits one real Rust-to-Python/MCP
//! candidate: it spawns the read-only helper with the explicit
//! `MORNLEA_AGENT_PYTHON` fixture (a missing executable fails, never skips),
//! plans through the real gateway, planner, and model SDK against real
//! snapshot tools, installs the validated step through the task runner and
//! the sessionless ingress, submits it to a real authority, and advances a
//! real tick. Presenting the same candidate twice and presenting it under a
//! superseded generation are both refused with their reported outcomes.

use std::collections::{BTreeMap, BTreeSet};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::os::unix::io::AsRawFd;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use mornlea_domain::{
    BlockPos, ChunkPos, CommandText, CompanionId, FiniteVec3, LookAngles, PlayerId,
};
use mornlea_protocol::{LoginStart, PlayIntent, admit_login};
use mornlea_server::agent::host::{CurrentWorld, DrainReport, PlanDispatch, PlanHost};
use mornlea_server::agent::http::{AgentHttpWire, parse_canonical_uuid};
use mornlea_server::agent::lease::{ControlPhase, LeaseConfig, LeaseController};
use mornlea_server::agent::mcp::{FrozenTools, MCP_ENDPOINT_PATH, McpService, PlanningTools};
use mornlea_server::agent::snapshot::{SnapshotEntropy, SnapshotRegistry};
use mornlea_server::contracts::{
    AgentHandle, Clock, CompanionAction, Deadline, ServerError, ServerLimits, SessionKey,
    SnapshotPort, TickBudget, TransportKind,
};
use mornlea_server::core::companion_ingress::{CompanionIngress, CompanionTaskGate};
use mornlea_server::state::AuthorityState;
use mornlea_storage::ItemStack;
use sha2::{Digest, Sha256};

/// Sealed inventory rows must stay exactly this many; the digest case below
/// checks every row, so a silent row drop cannot shrink the corpus.
const INVENTORY_ROWS: usize = 78;

/// Slices the top-level rows array out of the inventory JSON without a
/// serializer: rows are flat string objects, so bracket matching outside
/// strings finds the array exactly.
fn rows_array(text: &str) -> &str {
    let rows_at = text.find("\"rows\"").expect("rows");
    let start = text[rows_at..].find('[').expect("rows array") + rows_at;
    let mut depth = 0usize;
    let mut in_string = false;
    let mut escape = false;
    for (offset, ch) in text[start..].char_indices() {
        if in_string {
            if escape {
                escape = false;
            } else if ch == '\\' {
                escape = true;
            } else if ch == '"' {
                in_string = false;
            }
            continue;
        }
        match ch {
            '"' => in_string = true,
            '[' => depth += 1,
            ']' => {
                depth -= 1;
                if depth == 0 {
                    return &text[start..=start + offset];
                }
            }
            _ => {}
        }
    }
    panic!("unclosed rows array");
}

/// Reads one `"field": "value"` string out of a flat row object.
fn row_field<'a>(row: &'a str, field: &str) -> &'a str {
    let needle = format!("\"{field}\": \"");
    let start = row
        .find(&needle)
        .unwrap_or_else(|| panic!("row is missing {field}"))
        + needle.len();
    let end = row[start..].find('"').expect("field end");
    &row[start..start + end]
}

/// Splits the rows array into its flat row objects by brace matching outside
/// strings.
fn row_objects(rows: &str) -> Vec<&str> {
    let mut objects = Vec::new();
    let mut depth = 0usize;
    let mut start = 0usize;
    let mut in_string = false;
    let mut escape = false;
    for (offset, ch) in rows.char_indices() {
        if in_string {
            if escape {
                escape = false;
            } else if ch == '\\' {
                escape = true;
            } else if ch == '"' {
                in_string = false;
            }
            continue;
        }
        match ch {
            '"' => in_string = true,
            '{' => {
                if depth == 0 {
                    start = offset;
                }
                depth += 1;
            }
            '}' => {
                depth -= 1;
                if depth == 0 {
                    objects.push(&rows[start..=offset]);
                }
            }
            _ => {}
        }
    }
    objects
}

fn repository_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(4)
        .expect("repository root")
        .to_path_buf()
}

/// Recomputes every distinct Go source byte range named by the sealed
/// inventory and compares it with the recorded digest, then proves every
/// named fixture file exists. The digests pin the Go side of the contract;
/// no Rust layout participates.
#[test]
fn inventory_source_digests_match_go_fixtures() {
    let root = repository_root();
    let path = root.join("testdata/runtime-migration/server/capability-inventory.json");
    let text = std::fs::read_to_string(&path).expect("capability inventory");
    let rows = row_objects(rows_array(&text));
    assert_eq!(rows.len(), INVENTORY_ROWS, "no inventory row is dropped");

    let mut ids = BTreeSet::new();
    let mut ranges = BTreeMap::new();
    for row in &rows {
        let id = row_field(row, "id");
        assert!(!id.is_empty(), "empty capability id");
        assert!(ids.insert(id.to_owned()), "duplicate capability {id}");
        for field in [
            "source",
            "source_sha256",
            "fixture",
            "rust_node",
            "rust_test",
        ] {
            let _ = row_field(row, field);
        }
        let source = row_field(row, "source");
        let (file, range) = source.split_once("#bytes[").expect("byte range source");
        let range = range.strip_suffix(']').expect("range end");
        let (start, end) = range.split_once(':').expect("range bounds");
        ranges.insert(
            (
                file.to_owned(),
                start.parse::<usize>().expect("range start"),
                end.parse::<usize>().expect("range end"),
            ),
            row_field(row, "source_sha256").to_owned(),
        );
        let fixture = row_field(row, "fixture");
        for part in fixture.split(';') {
            let file = part.split_once('#').map(|(file, _)| file).unwrap_or(part);
            assert!(root.join(file).is_file(), "fixture file is missing: {file}");
        }
    }

    let mut checked = 0usize;
    for ((file, start, end), digest) in &ranges {
        let bytes = std::fs::read(root.join(file))
            .unwrap_or_else(|_| panic!("source file is missing: {file}"));
        let slice = bytes.get(*start..*end).expect("byte range in file");
        let expected = digest.strip_prefix("sha256:").expect("digest prefix");
        assert_eq!(
            format!("{:x}", Sha256::digest(slice)),
            expected,
            "Go source drifted: {file}[{start}:{end}]"
        );
        checked += 1;
    }
    assert!(checked > 0, "the corpus checks at least one Go range");
    println!("inventory digest: {INVENTORY_ROWS} rows, {checked} distinct Go ranges match");
}

fn authority(seed: i64) -> AuthorityState {
    AuthorityState::try_new(
        ServerLimits::try_new(8, 4096, 512, 64, 64, 1_048_576).unwrap(),
        seed,
    )
    .unwrap()
}

fn admitted(tag: u8, name: &str) -> mornlea_protocol::AdmittedLogin {
    let mut bytes = [0u8; 16];
    bytes[0] = tag;
    bytes[6] = 0x40;
    bytes[8] = 0x80;
    let id = PlayerId::try_from_bytes(bytes).expect("player id");
    let start = LoginStart::new(id, name, 8).expect("login start");
    let inbound = LoginStart::decode_inbound(&start.encode().expect("encoded")).expect("inbound");
    admit_login(inbound).expect("admitted login")
}

fn sequenced(sequence: u64) -> PlayIntent {
    PlayIntent::Sequenced {
        sequence,
        command: mornlea_domain::Command::CloseContainer,
    }
}

/// Canonical digest over logical replay outputs only: the executed tick,
/// per-session applied watermarks and arrival indexes, ordered event text,
/// and counters. Private Rust layouts never enter the hash.
fn logical_digest(
    state: &AuthorityState,
    sessions: &[SessionKey],
    events: &[Vec<String>],
    tick: u64,
) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(tick.to_be_bytes());
    for session in sessions {
        let facts = state.session(*session).expect("transcript session");
        hasher.update(session.get().to_be_bytes());
        hasher.update([facts.phase as u8]);
        hasher.update(facts.next_arrival.to_be_bytes());
        hasher.update(facts.last_applied_sequence.to_be_bytes());
    }
    for tick_events in events {
        hasher.update((tick_events.len() as u64).to_be_bytes());
        for event in tick_events {
            hasher.update((event.len() as u64).to_be_bytes());
            hasher.update(event.as_bytes());
        }
    }
    hasher.finalize().into()
}

/// Replays one two-player transcript through the real reducer and returns the
/// logical digest with the ordered per-tick events.
fn replay_transcript() -> ([u8; 32], Vec<Vec<String>>) {
    let mut state = authority(7);
    let ada = state
        .admit(admitted(1, "Ada"), TransportKind::Memory)
        .unwrap();
    let bea = state
        .admit(admitted(2, "Bea"), TransportKind::Memory)
        .unwrap();
    state.submit(ada, sequenced(5)).unwrap();
    state.submit(ada, sequenced(6)).unwrap();
    state.submit(bea, sequenced(1)).unwrap();
    let first = state.advance_tick(TickBudget::full()).unwrap();
    assert_eq!(first.tick, 0);
    assert_eq!(first.counters.stale, 0);
    let mut events = vec![
        first
            .events
            .iter()
            .map(|event| format!("{event:?}"))
            .collect::<Vec<_>>(),
    ];
    state.publish(first).unwrap();
    state.submit(ada, sequenced(4)).unwrap();
    state.submit(bea, sequenced(2)).unwrap();
    let second = state.advance_tick(TickBudget::full()).unwrap();
    assert_eq!(second.tick, 1);
    assert_eq!(
        (second.counters.commands, second.counters.stale),
        (2, 1),
        "the older sequence refuses below the applied watermark"
    );
    events.push(
        second
            .events
            .iter()
            .map(|event| format!("{event:?}"))
            .collect::<Vec<_>>(),
    );
    state.publish(second).unwrap();
    let digest = logical_digest(&state, &[ada, bea], &events, state.next_tick());
    assert_eq!(state.next_tick(), 2);
    (digest, events)
}

/// The same logical transcript replays to the same logical state and ordered
/// events on every run: watermarks, arrivals, ticks, and event text match
/// without comparing any private Rust layout.
#[test]
fn logical_state_and_events_match_across_runs() {
    let (first_digest, first_events) = replay_transcript();
    let (second_digest, second_events) = replay_transcript();
    assert_eq!(
        first_digest, second_digest,
        "logical replay is deterministic"
    );
    assert_eq!(
        first_events, second_events,
        "ordered events replay identically"
    );
    println!(
        "logical replay: 2 ticks, {} + {} ordered events, digest {:x?}",
        first_events[0].len(),
        first_events[1].len(),
        &first_digest[..8]
    );
}

/// Issuer identity shared with the cross-language fixtures.
const ISSUER_ID: &str = "99999999-9999-4999-8999-999999999999";
/// Companion identity shared with the cross-language fixtures.
const COMPANION_ID: &str = "66666666-6666-4666-8666-666666666666";
/// Client identity shared with the cross-language fixtures.
const CLIENT_ID: &str = "22222222-2222-4222-8222-222222222222";
/// Namespace identity shared with the cross-language fixtures.
const NAMESPACE_ID: &str = "4d5e6f70-8192-4aa3-8b4f-3a4b5c6d7e8f";
/// Ready line the helper prints once its inherited listener serves.
const READY_LINE: &str = r#"{"status":"ready"}"#;
/// Bound for the ready line plus both probe gates after spawn.
const READINESS_TIMEOUT: Duration = Duration::from_secs(15);
/// Bound for killing and reaping the helper on teardown.
const TEARDOWN_TIMEOUT: Duration = Duration::from_secs(5);

/// Step clock the harness holds fixed; business RPCs use real loopback I/O,
/// so the clock only fences lease and snapshot lifetimes.
struct StepClock {
    now: Mutex<Instant>,
}

impl StepClock {
    /// Freezes one clock at the current instant with its start returned for
    /// expiry arithmetic.
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
        *self.now.lock().expect("clock")
    }

    fn unix_ms(&self) -> i64 {
        1_800_000_000_000
    }
}

/// Deterministic entropy cycling bytes `0, 1, ...`, mirroring the other
/// snapshot harnesses.
struct CycleEntropy {
    next: Mutex<u8>,
}

impl CycleEntropy {
    /// Starts the byte cycle at zero.
    fn zero() -> Arc<Self> {
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
fn issuer_id() -> PlayerId {
    PlayerId::try_from_bytes(uuid_bytes(ISSUER_ID, "issuer")).expect("issuer id")
}

/// Companion identity of the planning fixture.
fn companion_id() -> CompanionId {
    CompanionId::try_from_bytes(uuid_bytes(COMPANION_ID, "companion")).expect("companion id")
}

/// Client instance identity presented to the helper gateway.
fn client_id() -> mornlea_server::contracts::ClientInstanceId {
    mornlea_server::contracts::ClientInstanceId::try_from_bytes(uuid_bytes(CLIENT_ID, "client"))
        .expect("client id")
}

/// Namespace identity presented to the helper gateway.
fn namespace_id() -> mornlea_server::contracts::NamespaceId {
    mornlea_server::contracts::NamespaceId::try_from_bytes(uuid_bytes(NAMESPACE_ID, "namespace"))
        .expect("namespace id")
}

fn tag_bytes(tag: u8) -> [u8; 16] {
    let mut bytes = [0u8; 16];
    bytes[0] = tag;
    bytes[6] = 0x40;
    bytes[8] = 0x80;
    bytes
}

/// Caller-minted business request identity.
fn request_id(tag: u8) -> mornlea_server::contracts::AgentRequestId {
    mornlea_server::contracts::AgentRequestId::try_from_bytes(tag_bytes(tag)).expect("request id")
}

/// Caller-minted run identity.
fn run_id(tag: u8) -> mornlea_server::contracts::RunId {
    mornlea_server::contracts::RunId::try_from_bytes(tag_bytes(tag)).expect("run id")
}

/// Business deadline a minute past the real clock so the helper gateway
/// treats every integration request as live.
fn wall_deadline_ms() -> i64 {
    let now_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("wall clock")
        .as_millis();
    i64::try_from(now_ms).expect("wall millis") + 60_000
}

/// Disposable scratch directory removed on drop.
struct TempDir {
    path: PathBuf,
}

impl TempDir {
    /// Creates an empty scratch directory tagged for the calling case.
    fn new(tag: &str) -> Self {
        let path =
            std::env::temp_dir().join(format!("mornlea-full-corpus-{}-{tag}", std::process::id()));
        if path.exists() {
            std::fs::remove_dir_all(&path).expect("clear stale temp dir");
        }
        std::fs::create_dir_all(&path).expect("create temp dir");
        Self { path }
    }

    /// Joins one disposable file name inside the scratch directory.
    fn join(&self, name: &str) -> PathBuf {
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

/// Disposable helper child with its scratch directory, source identity, and
/// drained standard error. Production server code never launches Python;
/// this test-only owner reuses the cross-language fixture pattern.
struct ChildOwner {
    child: Child,
    pid: u32,
    port: u16,
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
    fn endpoint(&self) -> String {
        format!("http://127.0.0.1:{}", self.port)
    }

    /// Kills and reaps the child within the teardown bound, then reports the
    /// start, stop, and source identities.
    fn shutdown(&mut self, what: &str) {
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
            let _ = self.child.try_wait();
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
    let mut stream = std::net::TcpStream::connect(addr).ok()?;
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
fn spawn_helper(tag: &str, token: &str) -> ChildOwner {
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
fn serve_mcp(clock: &Arc<StepClock>) -> (SnapshotRegistry, McpService) {
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
fn acquire_lease(clock: &Arc<StepClock>, endpoint: &str, token: &str) -> LeaseController {
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
fn target_pos() -> BlockPos {
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
fn plan_snapshot(
    instruction: &str,
    source_tick: u64,
) -> mornlea_server::contracts::PlanningSnapshot {
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
    mornlea_server::contracts::PlanningSnapshot::try_new(
        source_tick,
        1200,
        CommandText::try_from_canonical(instruction.to_owned()).expect("snapshot instruction"),
        mornlea_server::contracts::SnapshotIssuer {
            player_id: issuer_id(),
            position: finite([4.5, 64.0, -1.5]),
            look: angles(90.0, 0.0),
            look_hit: Some(target_pos()),
        },
        mornlea_server::contracts::SnapshotCompanion {
            companion_id: companion_id(),
            position: finite([5.5, 64.0, -1.5]),
            look: angles(0.0, 0.0),
            task_status: mornlea_server::contracts::SnapshotTaskStatusText::try_new(
                "规划中".to_owned(),
            )
            .expect("task status"),
            inventory: fixture_inventory(),
        },
        vec![mornlea_server::contracts::SnapshotPlayer {
            player_id: issuer_id(),
            position: finite([4.5, 64.0, -1.5]),
            look: angles(90.0, 0.0),
            look_hit: Some(target_pos()),
        }],
        vec![mornlea_server::contracts::SnapshotChunkRevision {
            pos: target_chunk(),
            revision: 17,
        }],
        vec![mornlea_server::contracts::SnapshotBlock {
            position: target_pos(),
            block_id: 2,
        }],
        mornlea_server::contracts::SnapshotTerrain::try_new(
            origin,
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
fn plan_world(tick: u64) -> CurrentWorld {
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

/// Drains the host until one outcome settles or the bound expires.
fn drain_plan_until(
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

/// Plans through the real gateway, planner, and model SDK against the real
/// snapshot tools, installs the validated mine step through the task runner,
/// the sessionless ingress, and a real authority tick, then proves the stale
/// replays are refused: the same candidate twice, and the same candidate
/// under a superseded generation.
#[test]
fn real_agent_candidate_admitted_and_stale_refused() {
    let token = "corpus-child-token";
    let mut child = spawn_helper("corpus-agent", token);
    let (_start, clock) = StepClock::start();
    let (mut snapshots, mcp) = serve_mcp(&clock);
    let mut agent = acquire_lease(&clock, &child.endpoint(), token);
    let (lease_id, fence) = agent.current_lease().expect("acquire admitted");
    let mut host = PlanHost::new();

    let ticket = host
        .dispatch_plan(
            &mut agent,
            &mut snapshots,
            &*clock,
            PlanDispatch {
                companion: companion_id(),
                generation: 7,
                request_id: request_id(40),
                run_id: run_id(41),
                client: client_id(),
                namespace: namespace_id(),
                lease: lease_id,
                lease_fence: fence,
                snapshot: plan_snapshot("采一块石头", 99),
                source_tick: 99,
                deadline_unix_ms: wall_deadline_ms(),
            },
        )
        .expect("dispatch admits");
    assert_eq!(ticket.attempt, 1);

    let drained = drain_plan_until(
        &mut host,
        &mut agent,
        &mut snapshots,
        &clock,
        Duration::from_secs(20),
    );
    if drained.completed != 1 {
        panic!(
            "plan outcome missing: report={drained:?} failures={:?}",
            host.take_failures()
        );
    }
    assert_eq!(drained.failed, 0);

    let installed = host.install(99, fence, &plan_world(99));
    assert_eq!(installed.installed, 1);
    assert_eq!(installed.envelopes.len(), 1);
    assert!(installed.rejected.is_empty());
    let envelope = installed
        .envelopes
        .into_iter()
        .next()
        .expect("one permitted effect");
    assert_eq!(envelope.companion_id, companion_id());
    assert_eq!(envelope.source_tick, 99);
    assert_ne!(envelope.snapshot_digest, [0u8; 32]);
    match envelope.action {
        CompanionAction::MineHold { target } => assert_eq!(target, target_pos()),
        other => panic!("expected mine hold, got {other:?}"),
    }

    // The live candidate enters through the sessionless ingress authority at
    // the planned tick, then reaches a real authority and a real tick.
    let mut ingress = CompanionIngress::try_new().expect("ingress");
    let gate = CompanionTaskGate::try_new(
        envelope.companion_id,
        envelope.request_id,
        envelope.run_id,
        envelope.snapshot_id,
        envelope.snapshot_digest,
        envelope.generation,
        envelope.attempt,
    )
    .expect("task gate");
    let receipt = ingress
        .admit(&gate, 99, envelope.clone())
        .expect("live candidate admits");
    assert_eq!(receipt.tick(), 99);
    let mut state = authority(7);
    let authority_receipt = state
        .submit_companion(envelope.clone())
        .expect("authority queues");
    assert_eq!(authority_receipt.tick(), state.next_tick());
    // The same envelope submitted twice while still queued is refused with
    // its duplicate outcome; the tick below drains the queue.
    assert_eq!(
        state.submit_companion(envelope.clone()),
        Err(ServerError::InvalidInput {
            field: "companion_action"
        }),
        "duplicate authority submission reports its refusal"
    );
    let publication = state.advance_tick(TickBudget::full()).unwrap();
    assert_eq!(publication.tick, state.next_tick() - 1);
    state.publish(publication).unwrap();

    // The same candidate presented twice to the ingress is refused with its
    // duplicate outcome, and the pending admission is unchanged.
    assert_eq!(
        ingress.admit(&gate, 99, envelope.clone()),
        Err(ServerError::InvalidInput {
            field: "companion_action"
        }),
        "duplicate candidate reports its refusal"
    );

    // The same candidate under a superseded generation is stale: the frozen
    // gate moved on, so the old generation reports its mismatch.
    let superseded = CompanionTaskGate::try_new(
        envelope.companion_id,
        envelope.request_id,
        envelope.run_id,
        envelope.snapshot_id,
        envelope.snapshot_digest,
        envelope.generation + 1,
        envelope.attempt,
    )
    .expect("superseding gate");
    assert_eq!(
        ingress.admit(&superseded, 99, envelope),
        Err(ServerError::InvalidInput {
            field: "companion_generation"
        }),
        "stale generation reports its refusal"
    );

    snapshots.close().expect("registry closes");
    mcp.close();
    assert!(
        mcp.wait_done(Duration::from_secs(5)).is_some(),
        "model serve loop settles"
    );
    agent
        .close(Deadline::after(clock.monotonic(), Duration::from_secs(5)).expect("close deadline"))
        .expect("agent closes");
    assert_eq!(agent.control_phase(), ControlPhase::Closed);
    child.shutdown("corpus-agent");
}
