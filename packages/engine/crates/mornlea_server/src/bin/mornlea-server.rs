#![deny(unsafe_code)]

//! Opt-in Rust server binary for activation qualification.
//!
//! The binary proves one claim: this exact process exclusively owns the
//! named world and answers the local control plane. It parses the frozen
//! activation flags, binds the loopback game port as a reservation, matches
//! the manifest nonce and world path, acquires the OS world lock, loads the
//! stored metadata through the real standalone reader, then serves `status`
//! and `shutdown` on the local control socket. It runs no ticks, admits no
//! sessions, and exposes no world action endpoint; gameplay serving stays a
//! recorded opt-in limitation. Orderly shutdown closes the store, reports
//! the quiescent phase, and exits so the lock releases exactly once.

use mornlea_server::core::contracts::{LoadedValue, SaveKey, ServerError};
use mornlea_server::store::atomic_file::AtomicFiles;
use mornlea_server::store::lease::WorldLease;
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::net::SocketAddr;
use std::path::{Path, PathBuf};

/// Usage failures exit here; operational failures print one `FAIL <code>`
/// line and exit `1` so the opt-in script can match the failure vocabulary.
const USAGE_EXIT: i32 = 2;

/// Second argument of every control `shutdown` request; larger values are
/// refused before any state changes.
const MAX_SHUTDOWN_DEADLINE_MS: u64 = 30_000;

struct Args {
    world: PathBuf,
    listen: SocketAddr,
    manifest: PathBuf,
    control_socket: PathBuf,
    dry_run: bool,
}

fn usage_error(message: &str) -> ! {
    eprintln!(
        "usage: mornlea-server --world <path> --listen <127.0.0.1:port> --activation-manifest <path> --control-socket <path> [--dry-run]"
    );
    eprintln!("error: {message}");
    std::process::exit(USAGE_EXIT);
}

fn fail(code: &str, detail: &str) -> ! {
    eprintln!("FAIL {code} {detail}");
    std::process::exit(1);
}

/// Takes the value following a flag; a missing value is a usage error.
fn take_value(argv: &[String], index: &mut usize, flag: &str) -> String {
    *index += 1;
    argv.get(*index)
        .cloned()
        .unwrap_or_else(|| usage_error(&format!("missing value for {flag}")))
}

/// Parses the frozen flag set; any other flag or a missing value is a usage
/// error rather than an activation failure code.
fn parse_args(argv: &[String]) -> Args {
    let mut world: Option<PathBuf> = None;
    let mut listen: Option<SocketAddr> = None;
    let mut manifest: Option<PathBuf> = None;
    let mut control_socket: Option<PathBuf> = None;
    let mut dry_run = false;
    let mut index = 1;
    while index < argv.len() {
        match argv[index].as_str() {
            "--world" => {
                world = Some(PathBuf::from(take_value(argv, &mut index, "--world")));
            }
            "--listen" => {
                let value = take_value(argv, &mut index, "--listen");
                match value.parse::<SocketAddr>() {
                    Ok(addr) if addr.ip().is_loopback() => listen = Some(addr),
                    _ => usage_error("--listen must name a loopback socket address"),
                }
            }
            "--activation-manifest" => {
                manifest = Some(PathBuf::from(take_value(
                    argv,
                    &mut index,
                    "--activation-manifest",
                )));
            }
            "--control-socket" => {
                control_socket = Some(PathBuf::from(take_value(
                    argv,
                    &mut index,
                    "--control-socket",
                )));
            }
            "--dry-run" => dry_run = true,
            other => usage_error(&format!("unknown flag {other}")),
        }
        index += 1;
    }
    Args {
        world: world.unwrap_or_else(|| usage_error("missing --world")),
        listen: listen.unwrap_or_else(|| usage_error("missing --listen")),
        manifest: manifest.unwrap_or_else(|| usage_error("missing --activation-manifest")),
        control_socket: control_socket.unwrap_or_else(|| usage_error("missing --control-socket")),
        dry_run,
    }
}

/// Loads the manifest record and checks the schema version plus the fields
/// this binary binds before touching the world.
fn load_manifest(path: &Path) -> serde_json::Value {
    let text = fs::read_to_string(path).unwrap_or_else(|_| fail("invalid_manifest", "unreadable"));
    let manifest: serde_json::Value =
        serde_json::from_str(&text).unwrap_or_else(|_| fail("invalid_manifest", "malformed json"));
    if manifest
        .get("schema_version")
        .and_then(|value| value.as_u64())
        != Some(1)
    {
        fail("invalid_manifest", "unsupported schema_version");
    }
    for field in ["start_nonce", "world_path", "phase"] {
        if manifest
            .get(field)
            .and_then(|value| value.as_str())
            .is_none()
        {
            fail("invalid_manifest", &format!("missing {field}"));
        }
    }
    manifest
}

fn manifest_field<'a>(manifest: &'a serde_json::Value, field: &str) -> &'a str {
    manifest
        .get(field)
        .and_then(|value| value.as_str())
        .unwrap_or_else(|| fail("invalid_manifest", &format!("missing {field}")))
}

/// Compares the canonical CLI world against the manifest binding; a mismatch
/// proves the manifest names a different world, never a hash drift.
fn check_world_binding(args: &Args, manifest: &serde_json::Value) -> PathBuf {
    let cli_world = args.world.canonicalize().unwrap_or_else(|_| {
        fail("invalid_manifest", "world path does not resolve");
    });
    let manifest_world = Path::new(manifest_field(manifest, "world_path"));
    let manifest_canonical = manifest_world.canonicalize().unwrap_or_else(|_| {
        fail("invalid_manifest", "manifest world path does not resolve");
    });
    if cli_world != manifest_canonical {
        fail(
            "identity_mismatch",
            "world path differs from the manifest binding",
        );
    }
    if manifest_field(manifest, "start_nonce").is_empty() {
        fail("invalid_manifest", "empty start_nonce");
    }
    cli_world
}

/// Records the stable file identity behind the acquired lock so a later
/// resume can prove the lock file was never replaced mid-flight.
fn lease_identity(world: &Path) -> String {
    let metadata = fs::metadata(world.join("world.lock"))
        .unwrap_or_else(|_| fail("writer_live", "lock file vanished after acquire"));
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        format!("world.lock:{}:{}", metadata.dev(), metadata.ino())
    }
    #[cfg(not(unix))]
    {
        let _ = metadata;
        format!("world.lock:{}", world.join("world.lock").to_string_lossy())
    }
}

/// Atomically replaces the manifest record; the binary owns these fields
/// only while it holds the world lock, the script owns them otherwise.
fn store_manifest(path: &Path, manifest: &serde_json::Value) {
    let staged = path.with_extension(format!("tmp.{}", std::process::id()));
    let text = serde_json::to_string_pretty(manifest)
        .unwrap_or_else(|_| fail("invalid_manifest", "manifest does not render"));
    fs::write(&staged, text).unwrap_or_else(|_| fail("invalid_manifest", "manifest not writable"));
    fs::rename(&staged, path).unwrap_or_else(|_| fail("invalid_manifest", "manifest not replaced"));
}

fn reply_status(stream: &mut impl Write, nonce: &str, phase: &str, world_closed: bool) {
    let line = serde_json::json!({
        "nonce": nonce,
        "phase": phase,
        "final_tick": 0,
        "world_closed": world_closed,
    });
    let mut text = serde_json::to_string(&line).expect("status renders");
    text.push('\n');
    let _ = stream.write_all(text.as_bytes());
    let _ = stream.flush();
}

fn reply_error(stream: &mut impl Write, code: &str) {
    let line = serde_json::json!({ "error": code });
    let mut text = serde_json::to_string(&line).expect("error renders");
    text.push('\n');
    let _ = stream.write_all(text.as_bytes());
    let _ = stream.flush();
}

fn main() {
    let argv: Vec<String> = std::env::args().collect();
    let args = parse_args(&argv);
    let mut manifest = load_manifest(&args.manifest);
    let world = check_world_binding(&args, &manifest);
    let nonce = manifest_field(&manifest, "start_nonce").to_owned();
    match manifest_field(&manifest, "phase") {
        "Prepared" | "RustRunning" | "StopRequested" | "Quiescent" => {}
        "DataVerified" | "PreviousRunning" => fail(
            "writer_live",
            "manifest already belongs to the previous runtime",
        ),
        _ => fail("invalid_manifest", "unknown phase"),
    }

    if !world.join("world.meta").is_file() {
        fail("invalid_manifest", "world carries no stored metadata");
    }
    // The game port is a reservation held for the process lifetime, never a
    // served endpoint; gameplay transport stays a recorded limitation.
    let _game_port: std::net::TcpListener = match std::net::TcpListener::bind(args.listen) {
        Ok(listener) => listener,
        Err(_) => fail("activation_failed", "loopback game port is not bindable"),
    };
    let _lease = match WorldLease::acquire(&world) {
        Ok(lease) => lease,
        Err(error) => {
            // Contention refuses as a live writer; a lock alias refuses as
            // an invalid world; any other I/O failure aborts activation.
            let code = match error {
                ServerError::Io {
                    kind: std::io::ErrorKind::WouldBlock,
                    ..
                } => "writer_live",
                ServerError::InvalidInput { .. } => "invalid_manifest",
                _ => "activation_failed",
            };
            fail(code, "world lock is not acquirable");
        }
    };
    let identity = lease_identity(&world);
    let mut files = match AtomicFiles::open(&world) {
        Ok(files) => files,
        Err(_) => fail("incompatible_save", "stored world does not open"),
    };
    match files.load(SaveKey::Metadata) {
        Ok(LoadedValue::Metadata(_)) => {}
        Ok(_) => fail(
            "incompatible_save",
            "metadata key decodes to another family",
        ),
        Err(_) => fail("incompatible_save", "stored metadata does not decode"),
    }

    if args.dry_run {
        println!(
            "DRYRUN OK manifest={} world={} nonce={} lock=free store=loaded",
            args.manifest.display(),
            world.display(),
            nonce,
        );
        return;
    }

    manifest["phase"] = serde_json::Value::from("RustRunning");
    manifest["pid"] = serde_json::Value::from(std::process::id() as u64);
    manifest["lease_identity"] = serde_json::Value::from(identity);
    manifest["last_error"] = serde_json::Value::Null;
    store_manifest(&args.manifest, &manifest);
    println!(
        "mornlea-server ready world={} control={}",
        world.display(),
        args.control_socket.display()
    );
    let _ = std::io::stdout().flush();

    serve_control(&args, &nonce, &mut files);
}

/// Accepts local control connections until an orderly `shutdown` quiesces
/// the store and exits the process, releasing the world lock exactly once.
fn serve_control(args: &Args, nonce: &str, files: &mut AtomicFiles) {
    #[cfg(unix)]
    {
        use std::os::unix::net::UnixListener;
        if let Some(parent) = args.control_socket.parent()
            && !parent.as_os_str().is_empty()
            && fs::create_dir_all(parent).is_err()
        {
            fail("invalid_manifest", "control socket parent not writable");
        }
        let _ = fs::remove_file(&args.control_socket);
        let listener = UnixListener::bind(&args.control_socket)
            .unwrap_or_else(|_| fail("activation_failed", "control socket is not bindable"));
        for incoming in listener.incoming() {
            let stream = match incoming {
                Ok(stream) => stream,
                Err(_) => continue,
            };
            if handle_connection(stream, args, nonce, files) {
                return;
            }
        }
    }
    #[cfg(not(unix))]
    {
        serve_control_tcp(args, nonce, files);
    }
}

/// Returns true once the process must exit after an orderly shutdown.
fn handle_connection(
    stream: impl std::io::Read + Write,
    args: &Args,
    nonce: &str,
    files: &mut AtomicFiles,
) -> bool {
    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    if reader.read_line(&mut line).is_err() {
        return false;
    }
    let request: serde_json::Value = match serde_json::from_str(line.trim()) {
        Ok(request) => request,
        Err(_) => {
            reply_error(reader.get_mut(), "malformed");
            return false;
        }
    };
    match request.get("op").and_then(|op| op.as_str()) {
        Some("status") => {
            reply_status(reader.get_mut(), nonce, current_phase(args), false);
            false
        }
        Some("shutdown") => {
            let deadline = request.get("deadline_ms").and_then(|value| value.as_u64());
            match deadline {
                Some(ms) if (1..=MAX_SHUTDOWN_DEADLINE_MS).contains(&ms) => {
                    orderly_shutdown(args, nonce, files, reader.get_mut())
                }
                _ => {
                    reply_error(reader.get_mut(), "deadline_exceeds_max");
                    false
                }
            }
        }
        _ => {
            reply_error(reader.get_mut(), "unknown_op");
            false
        }
    }
}

fn current_phase(args: &Args) -> &'static str {
    let text = fs::read_to_string(&args.manifest).unwrap_or_default();
    let manifest: serde_json::Value = serde_json::from_str(&text).unwrap_or_default();
    match manifest.get("phase").and_then(|phase| phase.as_str()) {
        Some("StopRequested") => "StopRequested",
        _ => "RustRunning",
    }
}

/// Marks `StopRequested`, closes the store without releasing the process
/// lock, reports `Quiescent`, records the manifest, and exits. A close
/// failure retains `StopRequested` with the named error and a nonzero exit
/// so rollback refuses to start the previous writer.
fn orderly_shutdown(
    args: &Args,
    nonce: &str,
    files: &mut AtomicFiles,
    stream: &mut impl Write,
) -> bool {
    let mut manifest = load_manifest(&args.manifest);
    manifest["phase"] = serde_json::Value::from("StopRequested");
    store_manifest(&args.manifest, &manifest);
    match files.close() {
        Ok(()) => {
            reply_status(stream, nonce, "Quiescent", true);
            manifest["phase"] = serde_json::Value::from("Quiescent");
            manifest["last_error"] = serde_json::Value::Null;
            store_manifest(&args.manifest, &manifest);
            std::process::exit(0);
        }
        Err(_) => {
            manifest["last_error"] =
                serde_json::json!({ "code": "shutdown_failed", "detail": "store close refused" });
            store_manifest(&args.manifest, &manifest);
            reply_error(stream, "shutdown_failed");
            eprintln!("FAIL shutdown_failed store close refused");
            std::process::exit(1);
        }
    }
}

#[cfg(not(unix))]
fn serve_control_tcp(args: &Args, nonce: &str, files: &mut AtomicFiles) {
    let listener = std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap_or_else(|_| fail("writer_live", "control socket is already owned"));
    let port = listener.local_addr().expect("control addr").port();
    fs::write(&args.control_socket, port.to_string())
        .unwrap_or_else(|_| fail("invalid_manifest", "control socket path not writable"));
    for incoming in listener.incoming() {
        let stream = match incoming {
            Ok(stream) => stream,
            Err(_) => continue,
        };
        if handle_connection(stream, args, nonce, files) {
            return;
        }
    }
}
