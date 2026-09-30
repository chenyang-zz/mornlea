#![cfg(unix)]

//! Opt-in activation and rollback qualification.
//!
//! The four named cases drive the real opt-in script and a rebuilt Rust
//! binary against the real previous Go binary on disposable world copies.
//! Every flow asserts single-writer ownership through the OS world lock,
//! never through PID liveness alone, and no case launches a graphical
//! client. The previous binary arrives only through the explicit
//! `MORNLEA_PREVIOUS_SERVER_BIN` fixture; a missing fixture fails instead
//! of skipping.

use mornlea_domain::PlayerId;
use mornlea_protocol::{
    ClientHello, LoginStart, LoginSuccess, ServerHello, read_frame, write_frame,
};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fs::{self, File};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

/// Basename of the world lock file, excluded from every tree hash because
/// lock ownership churns across runtimes while durable bytes stay fixed.
const LOCK_BASENAME: &str = "world.lock";

/// Exact basename of the named-backup identity record. Crafting a backup
/// the script adopts must reproduce the record name its backup verification
/// checks, while assertion-side discovery stays suffix-based through
/// `backup_identity_name`.
const BACKUP_IDENTITY_BASENAME: &str = ".mcgo-world-backup-v1.json";

/// Discovers the named-backup identity record inside a backup directory by
/// its stable suffix and proves exactly one record exists.
fn backup_identity_name(backup: &Path) -> String {
    let mut found = Vec::new();
    for entry in fs::read_dir(backup).expect("list backup directory") {
        let name = entry
            .expect("backup entry")
            .file_name()
            .to_string_lossy()
            .into_owned();
        if name.ends_with("-world-backup-v1.json") {
            found.push(name);
        }
    }
    assert_eq!(found.len(), 1, "backup carries exactly one identity record");
    found.pop().expect("identity record")
}

/// Name of the manifest record inside an activation run directory.
const MANIFEST_BASENAME: &str = "activation-manifest.json";

static SCRATCH_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Fails the case with the exact missing-fixture identity instead of
/// skipping when the harness names no binary.
fn rust_bin() -> PathBuf {
    if let Some(built) = option_env!("CARGO_BIN_EXE_mornlea-server") {
        return PathBuf::from(built);
    }
    std::env::var("MORNLEA_RUST_SERVER_BIN")
        .map(PathBuf::from)
        .expect("explicit MORNLEA_RUST_SERVER_BIN fixture names the rebuilt Rust binary")
}

/// Fails the case instead of skipping when no previous binary is named.
/// The binary is configured explicitly and never discovered through `PATH`.
fn previous_bin() -> PathBuf {
    std::env::var("MORNLEA_PREVIOUS_SERVER_BIN")
        .map(PathBuf::from)
        .expect("explicit MORNLEA_PREVIOUS_SERVER_BIN fixture names the previous Go binary")
}

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("..")
        .join("..")
}

fn optin_script() -> PathBuf {
    repo_root().join("scripts").join("rust-server-opt-in.sh")
}

/// Disposable root for one case. Drop kills recorded processes and removes
/// the tree best-effort so a failed assertion never leaks a writer.
struct Scope {
    root: PathBuf,
}

impl Scope {
    fn fresh(case: &str) -> Self {
        let id = SCRATCH_COUNTER.fetch_add(1, Ordering::SeqCst);
        // Short names keep the local control socket under the OS path
        // ceiling on every supported Unix.
        let root = std::env::temp_dir().join(format!("ma-{}-{}-{}", std::process::id(), id, case));
        if root.exists() {
            fs::remove_dir_all(&root).expect("clear stale scratch root");
        }
        fs::create_dir_all(&root).expect("create scratch root");
        Self { root }
    }

    fn path(&self, name: &str) -> PathBuf {
        self.root.join(name)
    }
}

impl Drop for Scope {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn sha256_file(path: &Path) -> String {
    let bytes = fs::read(path).unwrap_or_else(|_| panic!("read {}", path.display()));
    hex_of(&bytes)
}

fn hex_of(bytes: &[u8]) -> String {
    let mut digest = Sha256::new();
    digest.update(bytes);
    format!("{:x}", digest.finalize())
}

/// Content hash over one world tree with the exact construction the opt-in
/// script implements: regular files sorted by `/`-joined relative path,
/// each contributing its relative path, one zero byte, its little-endian
/// length, then its bytes. Callers name basenames that describe ownership
/// rather than durable world content.
fn tree_hash(root: &Path, skip_basenames: &[&str]) -> String {
    let mut entries: BTreeMap<String, PathBuf> = BTreeMap::new();
    collect_files(root, root, &mut entries);
    let mut digest = Sha256::new();
    for (relative, path) in &entries {
        if skip_basenames.iter().any(|skip| {
            Path::new(relative)
                .file_name()
                .is_some_and(|base| base == *skip)
        }) {
            continue;
        }
        let bytes = fs::read(path).unwrap_or_else(|_| panic!("read {}", path.display()));
        digest.update(relative.as_bytes());
        digest.update([0u8]);
        digest.update((bytes.len() as u64).to_le_bytes());
        digest.update(&bytes);
    }
    format!("{:x}", digest.finalize())
}

fn collect_files(base: &Path, dir: &Path, entries: &mut BTreeMap<String, PathBuf>) {
    let mut children: Vec<PathBuf> = fs::read_dir(dir)
        .unwrap_or_else(|_| panic!("list {}", dir.display()))
        .map(|entry| entry.expect("read dir entry").path())
        .collect();
    children.sort();
    for child in children {
        let file_type = fs::symlink_metadata(&child)
            .expect("stat entry")
            .file_type();
        if file_type.is_symlink() {
            panic!("symlink alias inside hashed tree: {}", child.display());
        }
        if file_type.is_dir() {
            collect_files(base, &child, entries);
        } else if file_type.is_file() {
            let relative = child
                .strip_prefix(base)
                .expect("entry under tree root")
                .to_str()
                .expect("utf8 relative path")
                .replace('\\', "/");
            entries.insert(relative, child);
        }
    }
}

fn wait_until(timeout: Duration, mut ready: impl FnMut() -> bool) -> bool {
    let start = Instant::now();
    while start.elapsed() < timeout {
        if ready() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    ready()
}

fn kill_pid(pid: u32) {
    let _ = Command::new("kill")
        .args(["-9", &pid.to_string()])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
}

fn wait_pid_gone(pid: u32, timeout: Duration) -> bool {
    wait_until(timeout, || {
        Command::new("kill")
            .args(["-0", &pid.to_string()])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map(|status| !status.success())
            .unwrap_or(true)
    })
}

/// Proves no process survives under one pid: `kill -0` must exit nonzero.
fn assert_pid_gone(pid: u32) {
    let gone = Command::new("kill")
        .args(["-0", &pid.to_string()])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|status| !status.success())
        .unwrap_or(true);
    assert!(gone, "no writer starts on backup failure");
}

fn run_script(args: &[&str]) -> (i32, String) {
    let output = Command::new("bash")
        .arg(optin_script())
        .args(args)
        .output()
        .expect("spawn opt-in script");
    let code = output.status.code().unwrap_or(-1);
    let mut text = String::from_utf8_lossy(&output.stdout).into_owned();
    text.push_str(&String::from_utf8_lossy(&output.stderr));
    (code, text)
}

fn read_manifest(path: &Path) -> serde_json::Value {
    let text = fs::read_to_string(path).unwrap_or_else(|_| panic!("read {}", path.display()));
    serde_json::from_str(&text).unwrap_or_else(|_| panic!("parse {}", path.display()))
}

fn manifest_str(manifest: &serde_json::Value, field: &str) -> String {
    manifest
        .get(field)
        .and_then(|value| value.as_str())
        .unwrap_or_else(|| panic!("manifest field {field}"))
        .to_owned()
}

/// Reads one complete frame from a stream with a per-read deadline.
fn read_one_frame(stream: &mut TcpStream) -> (u32, Vec<u8>) {
    let mut prefix = Vec::new();
    loop {
        let mut byte = [0u8; 1];
        stream.read_exact(&mut byte).expect("read frame prefix");
        prefix.push(byte[0]);
        if byte[0] & 0x80 == 0 {
            break;
        }
        assert!(prefix.len() <= 10, "frame length prefix overflow");
    }
    let body_len = decode_uvarint(&prefix);
    let mut body = vec![0u8; body_len];
    stream.read_exact(&mut body).expect("read frame body");
    let mut wire = Vec::with_capacity(prefix.len() + body.len());
    wire.extend_from_slice(&prefix);
    wire.extend_from_slice(&body);
    let (packet_id, payload, _) = read_frame(&wire).expect("decode frame");
    (packet_id, payload)
}

fn decode_uvarint(bytes: &[u8]) -> usize {
    let mut value: usize = 0;
    for (index, byte) in bytes.iter().enumerate() {
        let chunk = (byte & 0x7f) as usize;
        value |= chunk << (7 * index);
        if byte & 0x80 == 0 {
            return value;
        }
    }
    panic!("truncated uvarint prefix");
}

/// Deterministic version-4 identity derived from one tag byte.
fn tagged_identity(tag: u8) -> [u8; 16] {
    let mut bytes = [0u8; 16];
    bytes[0] = tag;
    bytes[1] = 0x5a;
    bytes[6] = 0x40 | 0x0f;
    bytes[8] = 0x80;
    bytes[15] = tag ^ 0x3c;
    PlayerId::try_from_bytes(bytes).expect("tagged v4 identity");
    bytes
}

/// Performs the real client login handshake against a running server and
/// returns the echoed identity with the authoritative world seed.
fn go_login(addr: &str, identity: [u8; 16], name: &str) -> Result<([u8; 16], u64), String> {
    let step = |what: &str| format!("{what} against {addr}");
    let version = mornlea_domain::Identities::current().protocol;
    let socket_addr = addr
        .to_socket_addrs()
        .map_err(|_| step("resolve loopback"))?
        .next()
        .ok_or_else(|| step("resolve loopback"))?;
    let mut stream = TcpStream::connect_timeout(&socket_addr, Duration::from_secs(10))
        .map_err(|_| step("connect game listener"))?;
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .map_err(|_| step("read deadline"))?;
    stream
        .set_write_timeout(Some(Duration::from_secs(10)))
        .map_err(|_| step("write deadline"))?;
    let hello = ClientHello::new(version).map_err(|_| step("current hello"))?;
    let payload = hello.encode().map_err(|_| step("encode hello"))?;
    let frame = write_frame(ClientHello::PACKET_ID, &payload).map_err(|_| step("frame hello"))?;
    stream.write_all(&frame).map_err(|_| step("send hello"))?;
    let (id, payload) = read_one_frame(&mut stream);
    if id != ServerHello::PACKET_ID {
        return Err(step("handshake answers server hello"));
    }
    let answer = ServerHello::decode(&payload).map_err(|_| step("decode server hello"))?;
    if answer.protocol_version != version {
        return Err(step("server pins current protocol"));
    }
    let player = PlayerId::try_from_bytes(identity).map_err(|_| step("v4 login identity"))?;
    let start = LoginStart::new(player, name, 8).map_err(|_| step("login start"))?;
    let payload = start.encode().map_err(|_| step("encode start"))?;
    let frame =
        write_frame(LoginStart::PACKET_ID, &payload).map_err(|_| step("frame login start"))?;
    stream
        .write_all(&frame)
        .map_err(|_| step("send login start"))?;
    let (id, payload) = read_one_frame(&mut stream);
    if id != LoginSuccess::PACKET_ID {
        return Err(step("login answers success"));
    }
    let success = LoginSuccess::decode(&payload).map_err(|_| step("decode login success"))?;
    Ok((success.player_id.bytes(), success.world_seed))
}

/// Spawns the previous binary on a fresh world, waits for a real login,
/// and returns the authoritative seed with the running child. Port zero
/// removes the probe-then-bind race: the OS assigns the loopback port and
/// the server prints the bound address before serving.
fn prepare_go_world(previous: &Path, world: &Path, name: &str) -> (Child, String, u64, [u8; 16]) {
    fs::create_dir_all(world).expect("create world dir");
    let log_path = world.join("prepare-stdout.log");
    let log = File::create(&log_path).expect("prepare log");
    let errors = log.try_clone().expect("clone log handle");
    let mut child = Command::new(previous)
        .args([
            "--world",
            &world.to_string_lossy(),
            "--listen",
            "127.0.0.1:0",
        ])
        .stdout(Stdio::from(log))
        .stderr(Stdio::from(errors))
        .spawn()
        .expect("spawn previous binary");
    let deadline = Instant::now() + Duration::from_secs(60);
    let addr = loop {
        if Instant::now() > deadline {
            let _ = child.kill();
            panic!("previous binary never reported its listen address");
        }
        if child.try_wait().expect("poll previous binary").is_some() {
            panic!("previous binary exited before serving");
        }
        let text = fs::read_to_string(&log_path).unwrap_or_default();
        if let Some(addr) = bound_listen_addr(&text) {
            break addr;
        }
        std::thread::sleep(Duration::from_millis(200));
    };
    let identity = tagged_identity(0x21);
    let deadline = Instant::now() + Duration::from_secs(60);
    let seed = loop {
        if Instant::now() > deadline {
            let _ = child.kill();
            panic!("previous binary never admitted login on {addr}");
        }
        if child.try_wait().expect("poll previous binary").is_some() {
            panic!("previous binary exited before login on {addr}");
        }
        if TcpStream::connect_timeout(
            &addr
                .to_socket_addrs()
                .expect("resolve")
                .next()
                .expect("one addr"),
            Duration::from_millis(300),
        )
        .is_ok()
        {
            if let Ok((_, seed)) = go_login(&addr, identity, name) {
                break seed;
            }
            std::thread::sleep(Duration::from_secs(1));
        } else {
            std::thread::sleep(Duration::from_millis(200));
        }
    };
    (child, addr, seed, identity)
}

/// Reads the bound `listen=` address back from one startup log.
fn bound_listen_addr(log: &str) -> Option<String> {
    log.split_whitespace()
        .find(|token| token.starts_with("listen="))
        .map(|token| token["listen=".len()..].to_owned())
}

fn stop_child(child: &mut Child, what: &str) {
    let _ = child.kill();
    assert!(
        wait_until(Duration::from_secs(30), || child
            .try_wait()
            .expect("poll child")
            .is_some()),
        "{what} process never exited"
    );
}

/// Sends one control request over the local socket and parses the one-line
/// JSON reply.
fn control_request(socket: &Path, request: &str) -> serde_json::Value {
    let mut stream = UnixStream::connect(socket).expect("connect control socket");
    stream
        .set_read_timeout(Some(Duration::from_secs(15)))
        .expect("control deadline");
    stream
        .write_all(request.as_bytes())
        .expect("send control request");
    stream.write_all(b"\n").expect("send control line");
    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    reader.read_line(&mut line).expect("read control reply");
    serde_json::from_str(line.trim()).expect("parse control reply")
}

fn control_status(socket: &Path) -> serde_json::Value {
    control_request(socket, r#"{"op":"status"}"#)
}

/// Starts one activation through the script and returns the manifest path.
fn script_activate(
    scope: &Scope,
    world: &Path,
    backup: &Path,
    run_dir: &Path,
    extra: &[&str],
) -> PathBuf {
    let rust = rust_bin();
    let previous = previous_bin();
    let previous_hash = sha256_file(&previous);
    let world_text = world.to_string_lossy();
    let backup_text = backup.to_string_lossy();
    let run_dir_text = run_dir.to_string_lossy();
    let rust_path = rust.to_string_lossy();
    let previous_path = previous.to_string_lossy();
    let mut args: Vec<&str> = vec![
        "activate",
        "--world",
        &world_text,
        "--backup",
        &backup_text,
        "--run-dir",
        &run_dir_text,
        "--rust-bin",
        &rust_path,
        "--previous-bin",
        &previous_path,
        "--previous-sha256",
        &previous_hash,
    ];
    args.extend_from_slice(extra);
    let (code, output) = run_script(&args);
    assert_eq!(
        code,
        0,
        "script activate failed in {}: {output}",
        scope.root.display()
    );
    let manifest = run_dir.join(MANIFEST_BASENAME);
    assert!(manifest.is_file(), "activate writes manifest: {output}");
    manifest
}

fn assert_single_rust_owner(manifest: &serde_json::Value, socket: &Path) {
    let nonce = manifest_str(manifest, "start_nonce");
    let status = control_status(socket);
    assert_eq!(
        status.get("nonce").and_then(|value| value.as_str()),
        Some(nonce.as_str()),
        "control nonce matches the manifest owner"
    );
    assert_eq!(
        status.get("phase").and_then(|value| value.as_str()),
        Some("RustRunning"),
        "control reports the running phase"
    );
}

#[test]
fn manifest_fixture_shape() {
    let fixture = repo_root()
        .join("testdata")
        .join("runtime-migration")
        .join("server")
        .join("activation.json");
    let manifest = read_manifest(&fixture);
    assert_eq!(
        manifest.get("schema_version").and_then(|v| v.as_u64()),
        Some(1)
    );
    assert_eq!(
        manifest.get("runtime").and_then(|v| v.as_str()),
        Some("rust")
    );
    assert_eq!(
        manifest.get("protocol_version").and_then(|v| v.as_u64()),
        Some(45)
    );
    let schemas = manifest
        .get("save_schemas")
        .expect("save schema identities");
    for (family, version) in [
        ("player", 9),
        ("chunk", 9),
        ("world_metadata", 6),
        ("companions_ai", 5),
        ("hostile_mobs", 2),
        ("passive_mobs", 1),
    ] {
        assert_eq!(
            schemas.get(family).and_then(|v| v.as_u64()),
            Some(version),
            "save schema {family}"
        );
    }
    for field in [
        "source_sha",
        "executable_sha256",
        "previous_executable",
        "previous_sha256",
        "world_path",
        "backup_path",
        "control_socket",
        "start_nonce",
        "phase",
    ] {
        assert!(
            manifest.get(field).is_some(),
            "manifest fixture names {field}"
        );
    }
}

#[test]
fn default_startup_paths_untouched() {
    let output = Command::new("git")
        .current_dir(repo_root())
        .args([
            "status",
            "--porcelain",
            "--",
            "Makefile",
            "packages/server/cmd/mornlea-server",
            "packages/client",
        ])
        .output()
        .expect("git status of default startup paths");
    assert!(output.status.success(), "git status runs");
    let text = String::from_utf8_lossy(&output.stdout);
    assert!(
        text.trim().is_empty(),
        "default startup paths stay unmodified: {text}"
    );
}

#[test]
fn actual_activate_stop_compatible_rollback() {
    let scope = Scope::fresh("compatible");
    let run_dir = scope.path("run");
    let world = run_dir.join("world");
    let backup = run_dir.join("backup");
    let previous = previous_bin();

    let (mut go, _, seed_before, identity) = prepare_go_world(&previous, &world, "rollback-probe");
    stop_child(&mut go, "preparing previous binary");
    let world_before = tree_hash(&world, &[LOCK_BASENAME]);

    let manifest_path = script_activate(&scope, &world, &backup, &run_dir, &[]);
    let manifest = read_manifest(&manifest_path);
    assert_eq!(
        manifest.get("phase").and_then(|v| v.as_str()),
        Some("RustRunning")
    );
    assert_eq!(
        manifest.get("executable_sha256").and_then(|v| v.as_str()),
        Some(sha256_file(&rust_bin()).as_str())
    );
    assert_eq!(
        manifest.get("previous_sha256").and_then(|v| v.as_str()),
        Some(sha256_file(&previous).as_str())
    );
    assert_eq!(
        manifest.get("world_tree_sha256").and_then(|v| v.as_str()),
        Some(world_before.as_str()),
        "manifest pins the pre-activation world bytes"
    );
    assert_eq!(
        manifest.get("backup_tree_sha256").and_then(|v| v.as_str()),
        Some(world_before.as_str()),
        "named backup copies the pre-activation world bytes"
    );
    let socket = PathBuf::from(manifest_str(&manifest, "control_socket"));
    assert_single_rust_owner(&manifest, &socket);
    assert_eq!(
        tree_hash(&world, &[LOCK_BASENAME]),
        world_before,
        "Rust tenure writes no world bytes"
    );

    let (code, output) = run_script(&[
        "rollback",
        "--manifest",
        &manifest_path.to_string_lossy(),
        "--data-policy",
        "compatible",
    ]);
    assert_eq!(code, 0, "compatible rollback failed: {output}");
    let manifest = read_manifest(&manifest_path);
    assert_eq!(
        manifest.get("phase").and_then(|v| v.as_str()),
        Some("PreviousRunning")
    );

    let previous_pid = read_manifest(&manifest_path)
        .get("pid")
        .and_then(|v| v.as_u64())
        .expect("rollback records the previous pid") as u32;
    assert!(
        !wait_pid_gone(previous_pid, Duration::from_secs(1)),
        "previous runtime stays live after rollback"
    );
    let listen = last_go_listen(&run_dir);
    let (echo, seed_after) =
        go_login(&listen, identity, "rollback-probe").expect("login after rollback");
    assert_eq!(echo, identity, "previous runtime admits the same identity");
    assert_eq!(
        seed_after, seed_before,
        "authoritative seed survives the round trip"
    );

    let rust = rust_bin();
    let probe = Command::new(&rust)
        .args([
            "--world",
            &world.to_string_lossy(),
            "--listen",
            "127.0.0.1:0",
            "--activation-manifest",
            &manifest_path.to_string_lossy(),
            "--control-socket",
            &run_dir.join("probe.sock").to_string_lossy(),
            "--dry-run",
        ])
        .output()
        .expect("spawn rust dry-run probe");
    assert!(
        !probe.status.success(),
        "live previous writer refuses a second owner"
    );
    let probe_text = String::from_utf8_lossy(&probe.stderr);
    assert!(
        probe_text.contains("writer_live"),
        "refusal names the live writer: {probe_text}"
    );
    kill_pid(previous_pid);
    assert!(wait_pid_gone(previous_pid, Duration::from_secs(30)));
}

/// Reads the previous runtime listen address recorded by the script run.
fn last_go_listen(run_dir: &Path) -> String {
    let notes = fs::read_to_string(run_dir.join("previous-listen.addr"))
        .expect("script records the previous listen address");
    notes.trim().to_owned()
}

#[test]
fn actual_backup_restore() {
    let scope = Scope::fresh("restore");
    let run_dir = scope.path("run");
    let world = run_dir.join("world");
    let backup = run_dir.join("backup");
    let previous = previous_bin();

    let (mut go, _, seed_before, identity) = prepare_go_world(&previous, &world, "restore-probe");
    stop_child(&mut go, "preparing previous binary");
    let world_before = tree_hash(&world, &[LOCK_BASENAME]);

    let manifest_path = script_activate(&scope, &world, &backup, &run_dir, &[]);
    let manifest = read_manifest(&manifest_path);
    let rust_pid = manifest
        .get("pid")
        .and_then(|v| v.as_u64())
        .expect("rust pid") as u32;
    kill_pid(rust_pid);
    assert!(
        wait_pid_gone(rust_pid, Duration::from_secs(30)),
        "crashed rust exits"
    );

    let meta = world.join("world.meta");
    assert!(meta.is_file(), "prepared world carries metadata");
    let mut bytes = fs::read(&meta).expect("read metadata");
    let flip = bytes.len() / 2;
    bytes[flip] ^= 0xff;
    fs::write(&meta, &bytes).expect("corrupt metadata byte");
    assert_ne!(
        tree_hash(&world, &[LOCK_BASENAME]),
        world_before,
        "corruption diverges the world bytes"
    );

    let (code, output) = run_script(&[
        "rollback",
        "--manifest",
        &manifest_path.to_string_lossy(),
        "--data-policy",
        "compatible",
    ]);
    assert_ne!(
        code, 0,
        "compatible rollback must refuse diverged bytes: {output}"
    );
    assert!(
        output.contains("incompatible_save"),
        "refusal names the incompatible save: {output}"
    );

    let (code, output) = run_script(&[
        "rollback",
        "--manifest",
        &manifest_path.to_string_lossy(),
        "--data-policy",
        "restore-backup",
    ]);
    assert_eq!(code, 0, "restore rollback failed: {output}");
    let manifest = read_manifest(&manifest_path);
    assert_eq!(
        manifest.get("phase").and_then(|v| v.as_str()),
        Some("PreviousRunning")
    );
    assert_eq!(
        tree_hash(&world, &[LOCK_BASENAME]),
        world_before,
        "restore reinstalls the exact backup bytes"
    );
    let identity_name = backup_identity_name(&backup);
    assert_eq!(
        tree_hash(&backup, &[LOCK_BASENAME, identity_name.as_str()]),
        world_before,
        "restore never mutates the named backup"
    );
    assert!(
        backup.join(&identity_name).is_file(),
        "backup identity survives restore"
    );
    assert!(
        !world.join("world.meta.bak").exists(),
        "no stray files remain beside the restored world"
    );

    let listen = last_go_listen(&run_dir);
    let (_, seed_after) =
        go_login(&listen, identity, "restore-probe").expect("login after restore");
    assert_eq!(
        seed_after, seed_before,
        "restored world carries the prepared seed"
    );
    let previous_pid = read_manifest(&manifest_path)
        .get("pid")
        .and_then(|v| v.as_u64())
        .expect("rollback records the previous pid") as u32;
    kill_pid(previous_pid);
    assert!(wait_pid_gone(previous_pid, Duration::from_secs(30)));

    let scope = Scope::fresh("missing-backup");
    let run_dir = scope.path("run");
    let world = run_dir.join("world");
    let backup = run_dir.join("backup");
    let (mut go, _, _, _) = prepare_go_world(&previous, &world, "missing-probe");
    stop_child(&mut go, "preparing previous binary");
    let manifest_path = script_activate(&scope, &world, &backup, &run_dir, &[]);
    let manifest = read_manifest(&manifest_path);
    let rust_pid = manifest
        .get("pid")
        .and_then(|v| v.as_u64())
        .expect("rust pid") as u32;
    kill_pid(rust_pid);
    assert!(wait_pid_gone(rust_pid, Duration::from_secs(30)));
    fs::remove_dir_all(&backup).expect("drop the named backup");
    let (code, output) = run_script(&[
        "rollback",
        "--manifest",
        &manifest_path.to_string_lossy(),
        "--data-policy",
        "restore-backup",
    ]);
    assert_ne!(code, 0, "restore without a backup must fail: {output}");
    assert!(
        output.contains("backup_mismatch"),
        "refusal names the backup: {output}"
    );
    assert_pid_gone(rust_pid);
}

#[test]
fn interrupted_each_phase() {
    let previous = previous_bin();

    let scope = Scope::fresh("resume-prepared");
    let run_dir = scope.path("run");
    let world = run_dir.join("world");
    let backup = run_dir.join("backup");
    let (mut go, _, seed_before, identity) = prepare_go_world(&previous, &world, "resume-probe");
    stop_child(&mut go, "preparing previous binary");
    let manifest_path = script_activate(&scope, &world, &backup, &run_dir, &[]);
    let manifest = read_manifest(&manifest_path);
    let rust_pid = manifest
        .get("pid")
        .and_then(|v| v.as_u64())
        .expect("rust pid") as u32;
    kill_pid(rust_pid);
    assert!(
        wait_pid_gone(rust_pid, Duration::from_secs(30)),
        "crashed rust exits"
    );
    let (code, output) = run_script(&[
        "rollback",
        "--manifest",
        &manifest_path.to_string_lossy(),
        "--data-policy",
        "compatible",
    ]);
    assert_eq!(code, 0, "rollback resumes after a rust crash: {output}");
    let manifest = read_manifest(&manifest_path);
    assert_eq!(
        manifest.get("phase").and_then(|v| v.as_str()),
        Some("PreviousRunning")
    );
    let listen = last_go_listen(&run_dir);
    let (_, seed_after) =
        go_login(&listen, identity, "resume-probe").expect("login after crash resume");
    assert_eq!(seed_after, seed_before, "seed survives crash resume");
    let previous_pid = read_manifest(&manifest_path)
        .get("pid")
        .and_then(|v| v.as_u64())
        .expect("previous pid") as u32;
    kill_pid(previous_pid);
    assert!(wait_pid_gone(previous_pid, Duration::from_secs(30)));
    let (code, output) = run_script(&[
        "rollback",
        "--manifest",
        &manifest_path.to_string_lossy(),
        "--data-policy",
        "compatible",
    ]);
    assert_eq!(code, 0, "rollback resumes after a previous crash: {output}");
    let manifest = read_manifest(&manifest_path);
    assert_eq!(
        manifest.get("phase").and_then(|v| v.as_str()),
        Some("PreviousRunning")
    );
    let listen = last_go_listen(&run_dir);
    let (_, seed_restarted) =
        go_login(&listen, identity, "resume-probe").expect("login after restart");
    assert_eq!(
        seed_restarted, seed_before,
        "seed survives previous restart"
    );
    let restarted_pid = read_manifest(&manifest_path)
        .get("pid")
        .and_then(|v| v.as_u64())
        .expect("restarted pid") as u32;
    assert_ne!(
        restarted_pid, previous_pid,
        "resume starts a fresh previous process"
    );

    let manifest_text = fs::read_to_string(&manifest_path).expect("read manifest");
    let mut manifest_value: serde_json::Value =
        serde_json::from_str(&manifest_text).expect("parse manifest");
    manifest_value["pid"] = serde_json::Value::from(std::process::id() as u64);
    fs::write(
        &manifest_path,
        serde_json::to_string_pretty(&manifest_value).expect("render manifest"),
    )
    .expect("write recycled-pid manifest");
    kill_pid(restarted_pid);
    assert!(wait_pid_gone(restarted_pid, Duration::from_secs(30)));
    let (code, output) = run_script(&[
        "rollback",
        "--manifest",
        &manifest_path.to_string_lossy(),
        "--data-policy",
        "compatible",
    ]);
    assert_eq!(
        code, 0,
        "a live manifest pid never blocks lock-proven resume: {output}"
    );
    let fresh_pid = read_manifest(&manifest_path)
        .get("pid")
        .and_then(|v| v.as_u64())
        .expect("fresh pid") as u32;
    assert_ne!(
        fresh_pid,
        std::process::id(),
        "resume never signals the recycled pid"
    );
    kill_pid(fresh_pid);
    assert!(wait_pid_gone(fresh_pid, Duration::from_secs(30)));

    resume_from_quiescent(&previous);
    resume_from_stop_requested(&previous);
    resume_from_data_verified(&previous);
    resume_from_prepared(&previous);
    resume_restore_between_renames(&previous, true);
    resume_restore_between_renames(&previous, false);
}

/// Stops the named Rust owner through its own control plane and waits out
/// the process; every resume scenario below starts from a dead owner.
fn quiesce_rust(manifest_path: &Path) -> serde_json::Value {
    let manifest = read_manifest(manifest_path);
    let socket = PathBuf::from(manifest_str(&manifest, "control_socket"));
    let pid = manifest
        .get("pid")
        .and_then(|v| v.as_u64())
        .expect("rust pid") as u32;
    let reply = control_request(&socket, r#"{"op":"shutdown","deadline_ms":5000}"#);
    assert_eq!(
        reply.get("phase").and_then(|v| v.as_str()),
        Some("Quiescent"),
        "orderly shutdown quiesces"
    );
    assert!(
        wait_pid_gone(pid, Duration::from_secs(30)),
        "rust exits after shutdown"
    );
    read_manifest(manifest_path)
}

/// Overwrites one manifest field while keeping every other binding intact.
fn set_manifest_field(manifest_path: &Path, field: &str, value: serde_json::Value) {
    let text = fs::read_to_string(manifest_path).expect("read manifest");
    let mut manifest: serde_json::Value = serde_json::from_str(&text).expect("parse manifest");
    manifest[field] = value;
    fs::write(
        manifest_path,
        serde_json::to_string_pretty(&manifest).expect("render manifest"),
    )
    .expect("write manifest");
}

fn copy_dir(src: &Path, dst: &Path) {
    fs::create_dir_all(dst).expect("create copy dir");
    let mut children: Vec<PathBuf> = fs::read_dir(src)
        .expect("list source")
        .map(|entry| entry.expect("dir entry").path())
        .collect();
    children.sort();
    for child in children {
        let target = dst.join(child.file_name().expect("file name"));
        if child.is_dir() {
            copy_dir(&child, &target);
        } else {
            fs::copy(&child, &target).expect("copy file");
        }
    }
}

fn rollback_compatible(manifest_path: &Path, what: &str) -> serde_json::Value {
    let (code, output) = run_script(&[
        "rollback",
        "--manifest",
        &manifest_path.to_string_lossy(),
        "--data-policy",
        "compatible",
    ]);
    assert_eq!(code, 0, "{what}: {output}");
    let manifest = read_manifest(manifest_path);
    assert_eq!(
        manifest.get("phase").and_then(|v| v.as_str()),
        Some("PreviousRunning"),
        "{what} records its phase"
    );
    manifest
}

fn stop_previous(manifest: &serde_json::Value) {
    let pid = manifest
        .get("pid")
        .and_then(|v| v.as_u64())
        .expect("previous pid") as u32;
    kill_pid(pid);
    assert!(
        wait_pid_gone(pid, Duration::from_secs(30)),
        "previous exits"
    );
}

fn resume_from_quiescent(previous: &Path) {
    let scope = Scope::fresh("quiescent");
    let run_dir = scope.path("run");
    let world = run_dir.join("world");
    let backup = run_dir.join("backup");
    let (mut go, _, seed_before, identity) = prepare_go_world(previous, &world, "quiescent-probe");
    stop_child(&mut go, "preparing previous binary");
    let manifest_path = script_activate(&scope, &world, &backup, &run_dir, &[]);
    let manifest = quiesce_rust(&manifest_path);
    assert_eq!(
        manifest.get("phase").and_then(|v| v.as_str()),
        Some("Quiescent")
    );
    let manifest = rollback_compatible(&manifest_path, "rollback resumes after quiescence");
    let listen = last_go_listen(&run_dir);
    let (_, seed_after) =
        go_login(&listen, identity, "quiescent-probe").expect("login after resume");
    assert_eq!(seed_after, seed_before, "seed survives quiescent resume");
    stop_previous(&manifest);
}

fn resume_from_stop_requested(previous: &Path) {
    let scope = Scope::fresh("stopped");
    let run_dir = scope.path("run");
    let world = run_dir.join("world");
    let backup = run_dir.join("backup");
    let (mut go, _, seed_before, identity) = prepare_go_world(previous, &world, "stopped-probe");
    stop_child(&mut go, "preparing previous binary");
    let manifest_path = script_activate(&scope, &world, &backup, &run_dir, &[]);
    let manifest = read_manifest(&manifest_path);
    let rust_pid = manifest
        .get("pid")
        .and_then(|v| v.as_u64())
        .expect("rust pid") as u32;
    kill_pid(rust_pid);
    assert!(
        wait_pid_gone(rust_pid, Duration::from_secs(30)),
        "crashed rust exits"
    );
    set_manifest_field(
        &manifest_path,
        "phase",
        serde_json::Value::from("StopRequested"),
    );
    let manifest = rollback_compatible(&manifest_path, "rollback resumes after stop request");
    let listen = last_go_listen(&run_dir);
    let (_, seed_after) = go_login(&listen, identity, "stopped-probe").expect("login after resume");
    assert_eq!(seed_after, seed_before, "seed survives stop-request resume");
    stop_previous(&manifest);
}

fn resume_from_data_verified(previous: &Path) {
    let scope = Scope::fresh("verified");
    let run_dir = scope.path("run");
    let world = run_dir.join("world");
    let backup = run_dir.join("backup");
    let (mut go, _, seed_before, identity) = prepare_go_world(previous, &world, "verified-probe");
    stop_child(&mut go, "preparing previous binary");
    let manifest_path = script_activate(&scope, &world, &backup, &run_dir, &[]);
    let manifest = read_manifest(&manifest_path);
    let rust_pid = manifest
        .get("pid")
        .and_then(|v| v.as_u64())
        .expect("rust pid") as u32;
    kill_pid(rust_pid);
    assert!(
        wait_pid_gone(rust_pid, Duration::from_secs(30)),
        "crashed rust exits"
    );
    set_manifest_field(
        &manifest_path,
        "phase",
        serde_json::Value::from("DataVerified"),
    );
    let manifest = rollback_compatible(&manifest_path, "rollback resumes after data verify");
    let listen = last_go_listen(&run_dir);
    let (_, seed_after) =
        go_login(&listen, identity, "verified-probe").expect("login after resume");
    assert_eq!(seed_after, seed_before, "seed survives verified resume");
    stop_previous(&manifest);
}

fn resume_from_prepared(previous: &Path) {
    let scope = Scope::fresh("prepared");
    let run_dir = scope.path("run");
    let world = run_dir.join("world");
    let backup = run_dir.join("backup");
    let rust = rust_bin();
    let previous_hash = sha256_file(previous);
    let (mut go, _, seed_before, identity) = prepare_go_world(previous, &world, "prepared-probe");
    stop_child(&mut go, "preparing previous binary");
    fs::create_dir_all(&run_dir).expect("create run dir");
    let world_canon = world.canonicalize().expect("canonical world");
    let backup_canon = run_dir.join("backup");
    copy_dir(&world, &backup_canon);
    let world_tree = tree_hash(&world, &[LOCK_BASENAME]);
    let identity_text = serde_json::json!({
        "source": world_canon.to_string_lossy(),
        "seed": -1,
        "migration_version": 1,
        "created_by": "rust-server-opt-in",
        "tree_sha256": world_tree,
    });
    fs::write(
        backup_canon.join(BACKUP_IDENTITY_BASENAME),
        serde_json::to_string_pretty(&identity_text).expect("render identity"),
    )
    .expect("write backup identity");
    let manifest_path = run_dir.join(MANIFEST_BASENAME);
    let run_canon = run_dir.canonicalize().expect("canonical run dir");
    let crafted = serde_json::json!({
        "schema_version": 1,
        "runtime": "rust",
        "source_sha": "0000000000000000000000000000000000000000",
        "executable_sha256": sha256_file(&rust),
        "previous_executable": previous.to_string_lossy(),
        "previous_sha256": previous_hash,
        "protocol_version": 45,
        "save_schemas": {"player": 9, "chunk": 9, "world_metadata": 6,
                         "companions_ai": 5, "hostile_mobs": 2, "passive_mobs": 1},
        "world_path": world_canon.to_string_lossy(),
        "backup_path": backup_canon.canonicalize().expect("canonical backup").to_string_lossy(),
        "world_tree_sha256": world_tree,
        "backup_tree_sha256": world_tree,
        "control_socket": run_canon.join("control.sock").to_string_lossy(),
        "pid": null,
        "start_nonce": "abababababababababababababababab",
        "lease_identity": null,
        "phase": "Prepared",
        "restore_stage": null,
        "last_error": null,
        "created_unix": 0,
    });
    fs::write(
        &manifest_path,
        serde_json::to_string_pretty(&crafted).expect("render manifest"),
    )
    .expect("write prepared manifest");
    let manifest_path = script_activate(&scope, &world, &backup, &run_dir, &[]);
    let manifest = read_manifest(&manifest_path);
    assert_eq!(
        manifest.get("phase").and_then(|v| v.as_str()),
        Some("RustRunning"),
        "activate resumes a prepared manifest"
    );
    assert_single_rust_owner(
        &manifest,
        &PathBuf::from(manifest_str(&manifest, "control_socket")),
    );
    let manifest = rollback_compatible(&manifest_path, "rollback follows prepared resume");
    let listen = last_go_listen(&run_dir);
    let (_, seed_after) =
        go_login(&listen, identity, "prepared-probe").expect("login after resume");
    assert_eq!(seed_after, seed_before, "seed survives prepared resume");
    stop_previous(&manifest);
}

/// Replays a restore crash with the staged copy present. When `with_retired`
/// holds, the current world is already retired and only the install remains;
/// otherwise only the staging record exists and the retire still runs.
fn resume_restore_between_renames(previous: &Path, with_retired: bool) {
    let scope = Scope::fresh(if with_retired { "renames" } else { "staged" });
    let run_dir = scope.path("run");
    let world = run_dir.join("world");
    let backup = run_dir.join("backup");
    let (mut go, _, seed_before, identity) = prepare_go_world(previous, &world, "rename-probe");
    stop_child(&mut go, "preparing previous binary");
    let manifest_path = script_activate(&scope, &world, &backup, &run_dir, &[]);
    let manifest = read_manifest(&manifest_path);
    let rust_pid = manifest
        .get("pid")
        .and_then(|v| v.as_u64())
        .expect("rust pid") as u32;
    kill_pid(rust_pid);
    assert!(
        wait_pid_gone(rust_pid, Duration::from_secs(30)),
        "crashed rust exits"
    );
    let nonce = manifest_str(&manifest, "start_nonce");
    let staged = world.parent().expect("run dir").join(format!(
        "{}.restore.{}",
        world.file_name().expect("world name").to_string_lossy(),
        nonce
    ));
    let retired = world.parent().expect("run dir").join(format!(
        "{}.retired.{}",
        world.file_name().expect("world name").to_string_lossy(),
        nonce
    ));
    copy_dir(&backup, &staged);
    let staged_identity = backup_identity_name(&staged);
    fs::remove_file(staged.join(staged_identity)).expect("strip staged identity");
    if with_retired {
        fs::rename(&world, &retired).expect("retire world");
        set_manifest_field(
            &manifest_path,
            "restore_stage",
            serde_json::Value::from("old_retired"),
        );
    } else {
        set_manifest_field(
            &manifest_path,
            "restore_stage",
            serde_json::Value::from("staged"),
        );
    }
    let (code, output) = run_script(&[
        "rollback",
        "--manifest",
        &manifest_path.to_string_lossy(),
        "--data-policy",
        "restore-backup",
    ]);
    assert_eq!(code, 0, "restore resumes between renames: {output}");
    let manifest = read_manifest(&manifest_path);
    assert_eq!(
        manifest.get("phase").and_then(|v| v.as_str()),
        Some("PreviousRunning")
    );
    let backup_tree = manifest_str(&manifest, "backup_tree_sha256");
    assert_eq!(
        tree_hash(&world, &[LOCK_BASENAME]),
        backup_tree,
        "resumed restore installs the exact backup bytes"
    );
    let listen = last_go_listen(&run_dir);
    let (_, seed_after) = go_login(&listen, identity, "rename-probe").expect("login after resume");
    assert_eq!(seed_after, seed_before, "seed survives restore resume");
    stop_previous(&manifest);
}

#[test]
fn live_writer_and_bad_previous_identity() {
    let scope = Scope::fresh("writer");
    let run_dir = scope.path("run");
    let world = run_dir.join("world");
    let backup = run_dir.join("backup");
    let previous = previous_bin();
    let rust = rust_bin();
    let previous_hash = sha256_file(&previous);

    let (mut go, _, _, _) = prepare_go_world(&previous, &world, "writer-probe");
    stop_child(&mut go, "preparing previous binary");

    let manifest_path = script_activate(&scope, &world, &backup, &run_dir, &[]);
    let manifest = read_manifest(&manifest_path);
    let socket = PathBuf::from(manifest_str(&manifest, "control_socket"));
    let nonce = manifest_str(&manifest, "start_nonce");
    let rust_pid = manifest
        .get("pid")
        .and_then(|v| v.as_u64())
        .expect("rust pid") as u32;
    assert_single_rust_owner(&manifest, &socket);

    let (code, output) = run_script(&[
        "activate",
        "--world",
        &world.to_string_lossy(),
        "--backup",
        &backup.to_string_lossy(),
        "--run-dir",
        &run_dir.to_string_lossy(),
        "--rust-bin",
        &rust.to_string_lossy(),
        "--previous-bin",
        &previous.to_string_lossy(),
        "--previous-sha256",
        &previous_hash,
    ]);
    assert_eq!(code, 0, "repeat activate resumes idempotently: {output}");
    let manifest = read_manifest(&manifest_path);
    assert_eq!(
        manifest.get("pid").and_then(|v| v.as_u64()),
        Some(rust_pid as u64),
        "resume starts no second writer"
    );
    assert_single_rust_owner(&manifest, &socket);

    kill_pid(rust_pid);
    assert!(wait_pid_gone(rust_pid, Duration::from_secs(30)));
    let (mut holder, _, _, _) = prepare_go_world(&previous, &world, "holder-probe");
    let (code, output) = run_script(&[
        "activate",
        "--world",
        &world.to_string_lossy(),
        "--backup",
        &backup.to_string_lossy(),
        "--run-dir",
        &run_dir.to_string_lossy(),
        "--rust-bin",
        &rust.to_string_lossy(),
        "--previous-bin",
        &previous.to_string_lossy(),
        "--previous-sha256",
        &previous_hash,
    ]);
    assert_ne!(
        code, 0,
        "activate against a foreign writer must fail: {output}"
    );
    assert!(
        output.contains("writer_live"),
        "refusal names the writer: {output}"
    );
    let manifest = read_manifest(&manifest_path);
    assert_eq!(
        manifest.get("phase").and_then(|v| v.as_str()),
        Some("RustRunning"),
        "refusal retains the proven phase"
    );
    stop_child(&mut holder, "foreign previous binary");

    let manifest_path = script_activate(&scope, &world, &backup, &run_dir, &[]);
    let manifest = read_manifest(&manifest_path);
    let socket = PathBuf::from(manifest_str(&manifest, "control_socket"));
    assert_single_rust_owner(&manifest, &socket);

    let tampered = read_manifest(&manifest_path);
    let mut tampered_value = tampered.clone();
    tampered_value["previous_sha256"] = serde_json::Value::from("0".repeat(64));
    fs::write(
        &manifest_path,
        serde_json::to_string_pretty(&tampered_value).expect("render manifest"),
    )
    .expect("write tampered manifest");
    let (code, output) = run_script(&[
        "rollback",
        "--manifest",
        &manifest_path.to_string_lossy(),
        "--data-policy",
        "compatible",
    ]);
    assert_ne!(
        code, 0,
        "rollback with a forged previous hash must fail: {output}"
    );
    assert!(
        output.contains("identity_mismatch"),
        "refusal names identity: {output}"
    );
    let manifest = read_manifest(&manifest_path);
    assert_single_rust_owner(&manifest, &socket);
    assert_eq!(
        manifest_str(&manifest, "start_nonce"),
        nonce,
        "rust owner unchanged"
    );

    fs::write(
        &manifest_path,
        serde_json::to_string_pretty(&tampered).expect("render manifest"),
    )
    .expect("restore manifest");
    let rust_pid = read_manifest(&manifest_path)
        .get("pid")
        .and_then(|v| v.as_u64())
        .expect("rust pid") as u32;
    let shutdown = control_request(&socket, r#"{"op":"shutdown","deadline_ms":5000}"#);
    assert_eq!(
        shutdown.get("phase").and_then(|v| v.as_str()),
        Some("Quiescent"),
        "orderly shutdown quiesces: {shutdown}"
    );
    assert!(
        wait_pid_gone(rust_pid, Duration::from_secs(30)),
        "rust exits after shutdown"
    );
    let manifest = read_manifest(&manifest_path);
    assert_eq!(
        manifest.get("phase").and_then(|v| v.as_str()),
        Some("Quiescent")
    );

    let copy_bin = scope.path("rust-copy");
    fs::copy(&rust, &copy_bin).expect("copy rust binary");
    let mut bytes = fs::read(&copy_bin).expect("read rust copy");
    bytes.extend_from_slice(b"tamper");
    fs::write(&copy_bin, &bytes).expect("tamper rust copy");
    let (code, output) = run_script(&[
        "activate",
        "--world",
        &world.to_string_lossy(),
        "--backup",
        &backup.to_string_lossy(),
        "--run-dir",
        &run_dir.to_string_lossy(),
        "--rust-bin",
        &copy_bin.to_string_lossy(),
        "--previous-bin",
        &previous.to_string_lossy(),
        "--previous-sha256",
        &previous_hash,
    ]);
    assert_ne!(
        code, 0,
        "activate with a replaced rust binary must fail: {output}"
    );
    assert!(
        output.contains("identity_mismatch"),
        "refusal names identity: {output}"
    );
}

#[test]
fn control_rejects_world_actions() {
    let scope = Scope::fresh("control");
    let run_dir = scope.path("run");
    let world = run_dir.join("world");
    let backup = run_dir.join("backup");
    let previous = previous_bin();

    let (mut go, _, _, _) = prepare_go_world(&previous, &world, "control-probe");
    stop_child(&mut go, "preparing previous binary");
    let manifest_path = script_activate(&scope, &world, &backup, &run_dir, &[]);
    let manifest = read_manifest(&manifest_path);
    let socket = PathBuf::from(manifest_str(&manifest, "control_socket"));

    for request in [
        r#"{"op":"write_world"}"#,
        r#"{"op":"submit_command"}"#,
        r#"{"op":"shutdown","deadline_ms":31000}"#,
    ] {
        let reply = control_request(&socket, request);
        assert!(
            reply.get("error").is_some(),
            "control exposes no world action: {request} -> {reply}"
        );
    }
    let status = control_status(&socket);
    assert_eq!(
        status.get("phase").and_then(|value| value.as_str()),
        Some("RustRunning"),
        "rejected requests leave the phase untouched"
    );

    let rust_pid = read_manifest(&manifest_path)
        .get("pid")
        .and_then(|v| v.as_u64())
        .expect("rust pid") as u32;
    let shutdown = control_request(&socket, r#"{"op":"shutdown","deadline_ms":5000}"#);
    assert_eq!(
        shutdown.get("phase").and_then(|v| v.as_str()),
        Some("Quiescent")
    );
    assert!(wait_pid_gone(rust_pid, Duration::from_secs(30)));
}
