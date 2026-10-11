//! Disposable real TCP authority for the external native session qualification.
#[path = "../tests/support/rust_server.rs"]
mod rust_server;
use std::path::PathBuf;
use std::time::{Duration, Instant};

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let mut listen = None;
    let mut ready: Option<PathBuf> = None;
    let mut stop: Option<PathBuf> = None;
    let mut report: Option<PathBuf> = None;
    let mut seconds = None;
    while let Some(key) = args.next() {
        let value = args.next().ok_or("missing option value")?;
        match key.as_str() {
            "--listen" => listen = Some(value),
            "--ready-file" => ready = Some(value.into()),
            "--stop-file" => stop = Some(value.into()),
            "--report-file" => report = Some(value.into()),
            "--max-seconds" => seconds = Some(value.parse::<u64>()?),
            _ => return Err(format!("unknown option: {key}").into()),
        }
    }
    if listen.as_deref() != Some("127.0.0.1:0") {
        return Err("listen must be 127.0.0.1:0".into());
    }
    let ready = ready.ok_or("missing ready-file")?;
    let stop = stop.ok_or("missing stop-file")?;
    let report = report.ok_or("missing report-file")?;
    let seconds = seconds
        .filter(|seconds| *seconds > 0)
        .ok_or("max-seconds must be positive")?;
    let mut server = rust_server::TcpServer::new();
    std::fs::write(ready, format!("{}\n", server.address()))?;
    let start = Instant::now();
    let deadline = start
        .checked_add(Duration::from_secs(seconds))
        .ok_or("deadline overflow")?;
    let mut next_tick = start;
    let mut stopped = false;
    while Instant::now() < deadline {
        if stop.try_exists()? {
            stopped = true;
            break;
        }
        server.pump();
        if Instant::now() >= next_tick {
            server.server.tick();
            next_tick = Instant::now() + Duration::from_millis(50);
        }
        std::thread::yield_now();
    }
    server.close_connections();
    let record = serde_json::json!({
        "listen":server.address().to_string(),
        "sessions":server.admitted.len(),
        "packets":server.server.packets,
        "ticks":server.server.ticks,
        "source_sessions":server.admitted.len(),
        "retired_connections":server.retired,
        "source_ticks":server.server.ticks,
        "source_packets":server.server.packets,
        "retained_sockets":server.transport.retained_sockets(),
        "stop_requested":stopped,
        "deadline_reached":!stopped,
        "errors":server.errors,
    });
    std::fs::write(report, serde_json::to_vec_pretty(&record)?)?;
    if !server.errors.is_empty() {
        return Err("real server reported errors".into());
    }
    Ok(())
}
fn main() {
    if let Err(error) = run() {
        eprintln!("rust session server: {error}");
        std::process::exit(1);
    }
}
