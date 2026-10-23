//! Process-level smoke for `--serve`: the binary runs the 20 TPS host loop.
//!
//! Companion movement and client-visible state remain open (Owen/Lena 4.1
//! archive criteria); this case only proves the executable enters the paced
//! tick path and exits after `--max-ticks`.

#![cfg(unix)]

use std::process::Command;
use std::time::{Duration, Instant};

#[test]
fn serve_mode_runs_max_ticks_and_exits() {
    let bin = env!("CARGO_BIN_EXE_mornlea-server");
    let root = tempfile_world();
    let probe = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = probe.local_addr().unwrap().port();
    drop(probe);
    let listen = format!("127.0.0.1:{port}");
    let started = Instant::now();
    let output = Command::new(bin)
        .args([
            "--serve",
            "--world",
            root.to_str().unwrap(),
            "--listen",
            &listen,
            "--max-ticks",
            "3",
        ])
        .output()
        .expect("spawn mornlea-server --serve");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "status={} stdout={stdout} stderr={stderr}",
        output.status
    );
    assert!(
        stdout.contains("serve ready"),
        "missing ready banner: {stdout}"
    );
    assert!(
        stdout.contains("serve stopped ticks=3"),
        "missing stop line: {stdout}"
    );
    // Three ticks at 20 TPS need at least ~100 ms of pacing after first due.
    assert!(
        started.elapsed() >= Duration::from_millis(100),
        "finished too fast for 20 TPS pacing: {:?}",
        started.elapsed()
    );
}

fn tempfile_world() -> std::path::PathBuf {
    let mut path = std::env::temp_dir();
    path.push(format!(
        "mornlea-serve-smoke-{}-{}",
        std::process::id(),
        Instant::now().elapsed().as_nanos()
    ));
    std::fs::create_dir_all(&path).unwrap();
    path
}
