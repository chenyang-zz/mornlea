//! Actual MCP connection ownership over loopback TCP.

use super::*;
use mornlea_server::contracts::{McpLifecycle, Operation};
use std::sync::Condvar;

const WAIT: Duration = Duration::from_secs(5);

struct HeldTools {
    released: Mutex<bool>,
    wake: Condvar,
    entered: mpsc::Sender<()>,
    calls: AtomicUsize,
    panic_on_release: bool,
}

impl HeldTools {
    fn new() -> (Arc<Self>, mpsc::Receiver<()>) {
        Self::with_panic(false)
    }

    fn with_panic(panic_on_release: bool) -> (Arc<Self>, mpsc::Receiver<()>) {
        let (entered, receive) = mpsc::channel();
        (
            Arc::new(Self {
                released: Mutex::new(false),
                wake: Condvar::new(),
                entered,
                calls: AtomicUsize::new(0),
                panic_on_release,
            }),
            receive,
        )
    }

    fn release(&self) {
        *self.released.lock().unwrap() = true;
        self.wake.notify_all();
    }
}

impl PlanningTools for HeldTools {
    fn execute(
        &self,
        lease: &SnapshotLease,
        name: &str,
        arguments: &[u8],
    ) -> Result<ToolSuccess, ToolFault> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.entered.send(()).expect("signal actual tool entry");
        let mut released = self.released.lock().unwrap();
        while !*released {
            released = self.wake.wait(released).unwrap();
        }
        drop(released);
        assert!(!self.panic_on_release, "deliberate actual tool panic");
        FrozenTools.execute(lease, name, arguments)
    }
}

fn valid_request(addr: &str, bearer: &str) -> Vec<u8> {
    let auth = format!("Bearer {bearer}");
    let headers = [
        ("Host", addr),
        ("Authorization", auth.as_str()),
        ("Content-Type", "application/json"),
        ("Accept", "application/json, text/event-stream"),
        ("Mcp-Protocol-Version", MCP_PROTOCOL_VERSION),
    ];
    request(
        "POST",
        "/mcp",
        &headers,
        tool_call_body(1, "get_planning_context", "{}").as_bytes(),
    )
}

// Shutdown may interrupt HTTP before a response exists; EOF/reset is a valid
// transport observation, while read timeouts remain test failures.
fn read_closed(mut stream: TcpStream) -> std::io::Result<Vec<u8>> {
    stream.set_read_timeout(Some(WAIT))?;
    let mut raw = Vec::new();
    let mut chunk = [0; 8192];
    loop {
        match stream.read(&mut chunk) {
            Ok(0) => return Ok(raw),
            Ok(count) => raw.extend_from_slice(&chunk[..count]),
            Err(error) if error.kind() == std::io::ErrorKind::ConnectionReset => return Ok(raw),
            Err(error) => return Err(error),
        }
    }
}

fn client(addr: &str, bearer: &str) -> std::thread::JoinHandle<std::io::Result<Vec<u8>>> {
    let mut stream = TcpStream::connect(addr).expect("connect actual client");
    let bytes = valid_request(addr, bearer);
    std::thread::spawn(move || match stream.write_all(&bytes) {
        Ok(()) => read_closed(stream),
        Err(error)
            if matches!(
                error.kind(),
                std::io::ErrorKind::BrokenPipe | std::io::ErrorKind::ConnectionReset
            ) =>
        {
            Ok(Vec::new())
        }
        Err(error) => Err(error),
    })
}

#[test]
fn legacy_close_waits_for_actual_held_tool() {
    let fixture = harness(base_snapshot("planning", Vec::new(), &[]));
    let (tools, entered) = HeldTools::new();
    let live = live_service(&fixture, tools.clone());
    let peer = client(&live.addr, &fixture.bearer);
    let entry = entered.recv_timeout(WAIT);
    let service = Arc::new(live.service);
    let closing = service.clone();
    let (done, receive) = mpsc::channel();
    let closer = std::thread::spawn(move || {
        closing.close();
        done.send(()).unwrap();
    });
    let returned_while_held = receive.recv_timeout(Duration::from_millis(50)).is_ok();
    tools.release();
    let peer_result = peer.join().expect("join client");
    closer.join().expect("join closer");
    service.close();
    assert!(entry.is_ok(), "tool entry missing: {entry:?}");
    assert!(peer_result.is_ok(), "client failed: {peer_result:?}");
    assert!(
        !returned_while_held,
        "legacy close returned while an actual tool worker remained held"
    );
}

#[test]
fn seventeenth_connection_is_closed_without_tool_dispatch() {
    let fixture = harness(base_snapshot("planning", Vec::new(), &[]));
    let (tools, entered) = HeldTools::new();
    let live = live_service(&fixture, tools.clone());
    let mut peers = Vec::new();
    let mut entries = Vec::new();
    for _ in 0..16 {
        peers.push(client(&live.addr, &fixture.bearer));
        entries.push(entered.recv_timeout(WAIT));
    }
    let overflow = client(&live.addr, &fixture.bearer);
    let dispatched = entered.recv_timeout(Duration::from_millis(500)).is_ok();
    tools.release();
    let overflow_result = overflow.join().expect("join overflow client");
    let peer_results: Vec<_> = peers
        .into_iter()
        .map(|peer| peer.join().expect("join client"))
        .collect();
    live.service.close();
    assert!(
        entries.iter().all(Result::is_ok),
        "not all admitted tools entered: {entries:?}"
    );
    assert!(
        peer_results.iter().all(Result::is_ok),
        "admitted client failed"
    );
    assert!(
        overflow_result.is_ok(),
        "overflow did not close: {overflow_result:?}"
    );
    assert!(
        !dispatched,
        "seventeenth connection dispatched an actual tool"
    );
    assert_eq!(tools.calls.load(Ordering::SeqCst), 16);
    assert!(
        overflow_result.unwrap().is_empty(),
        "overflow received HTTP instead of peer closure"
    );
}

fn deadline(timeout: Duration) -> Deadline {
    Deadline::at(Instant::now() + timeout)
}

#[test]
fn actual_lifecycle_timeout_retains_tool_then_retries() {
    let fixture = harness(base_snapshot("planning", Vec::new(), &[]));
    let (tools, entered) = HeldTools::new();
    let live = live_service(&fixture, tools.clone());
    let peer = client(&live.addr, &fixture.bearer);
    let entry = entered.recv_timeout(WAIT);
    let mut service = live.service;
    let start = Instant::now();
    let timeout = McpLifecycle::close(&mut service, deadline(Duration::from_millis(5)));
    let elapsed = start.elapsed();
    let frozen = fixture.lease.checkpoint().is_err();
    let (observed, closed) = mpsc::channel();
    let reader = std::thread::spawn(move || observed.send(peer.join()).unwrap());
    let interrupted = closed.recv_timeout(Duration::from_millis(250));
    tools.release();
    reader.join().expect("join reader");
    let retry = McpLifecycle::close(&mut service, deadline(WAIT));
    let repeated = service.close_until(Deadline::at(Instant::now()));
    let settled = service.wait_done(WAIT);
    assert!(entry.is_ok(), "tool did not enter");
    assert_eq!(
        timeout,
        Err(ServerError::Timeout {
            operation: Operation::Close
        })
    );
    assert!(
        elapsed < Duration::from_millis(250),
        "close exceeded bounded deadline: {elapsed:?}"
    );
    assert!(frozen, "timeout failed to invalidate actual lease");
    assert!(
        matches!(interrupted, Ok(Ok(Ok(_)))),
        "socket was not interrupted while tool held: {interrupted:?}"
    );
    assert_eq!(retry, Ok(()));
    assert_eq!(repeated, Ok(()));
    assert_eq!(settled, Some(Ok(())));
    assert!(
        TcpStream::connect(&live.addr).is_err(),
        "successful close retained listener"
    );
}

#[test]
fn expired_close_freezes_and_retains_actual_worker() {
    let fixture = harness(base_snapshot("planning", Vec::new(), &[]));
    let (tools, entered) = HeldTools::new();
    let live = live_service(&fixture, tools.clone());
    let peer = client(&live.addr, &fixture.bearer);
    let entry = entered.recv_timeout(WAIT);
    let first = live.service.close_until(Deadline::at(Instant::now()));
    let still_held = tools.calls.load(Ordering::SeqCst);
    let frozen = fixture.registry.lookup(&fixture.bearer).is_err();
    tools.release();
    let peer_result = peer.join().expect("join client");
    let retry = live.service.close_until(deadline(WAIT));
    assert!(entry.is_ok(), "tool did not enter");
    assert_eq!(
        first,
        Err(ServerError::Timeout {
            operation: Operation::Close
        })
    );
    assert_eq!(still_held, 1);
    assert!(frozen);
    assert!(peer_result.is_ok());
    assert_eq!(retry, Ok(()));
}

#[test]
fn actual_tool_panic_is_reported_once_then_retired() {
    let fixture = harness(base_snapshot("planning", Vec::new(), &[]));
    let (tools, entered) = HeldTools::with_panic(true);
    let live = live_service(&fixture, tools.clone());
    let peer = client(&live.addr, &fixture.bearer);
    let entry = entered.recv_timeout(WAIT);
    tools.release();
    let peer_result = peer.join().expect("join client");
    let first = live.service.close_until(deadline(WAIT));
    let retry = live.service.close_until(Deadline::at(Instant::now()));
    assert!(entry.is_ok(), "panic tool did not enter");
    assert!(peer_result.is_ok());
    assert_eq!(
        first,
        Err(ServerError::Internal {
            invariant: "mcp connection join"
        })
    );
    assert_eq!(retry, Ok(()));
}

#[test]
fn partial_header_and_body_io_are_interrupted_before_close_succeeds() {
    let fixture = harness(base_snapshot("planning", Vec::new(), &[]));
    let (tools, entered) = HeldTools::new();
    let live = live_service(&fixture, tools.clone());
    let mut header = TcpStream::connect(&live.addr).expect("header client");
    header
        .write_all(b"POST /mcp HTTP/1.1\r\nHost:")
        .expect("partial header");
    let mut body = TcpStream::connect(&live.addr).expect("body client");
    let full = valid_request(&live.addr, &fixture.bearer);
    body.write_all(&full[..full.len() - 1])
        .expect("partial body");
    let held = client(&live.addr, &fixture.bearer);
    let entry = entered.recv_timeout(WAIT);
    let first = live.service.close_until(deadline(Duration::from_millis(5)));
    let header_result = read_closed(header);
    let body_result = read_closed(body);
    tools.release();
    let held_result = held.join().expect("join held client");
    let retry = live.service.close_until(deadline(WAIT));
    assert!(entry.is_ok(), "tool did not enter");
    assert_eq!(
        first,
        Err(ServerError::Timeout {
            operation: Operation::Close
        })
    );
    assert!(
        header_result.is_ok(),
        "partial header retained: {header_result:?}"
    );
    assert!(
        body_result.is_ok(),
        "partial body retained: {body_result:?}"
    );
    assert!(held_result.is_ok());
    assert_eq!(retry, Ok(()));
}

#[test]
fn completed_connections_release_capacity_without_history() {
    let fixture = harness(base_snapshot("planning", Vec::new(), &[]));
    let live = live_service(&fixture, Arc::new(FrozenTools));
    let mut responses = Vec::new();
    for _ in 0..64 {
        responses.push(roundtrip(
            &live.addr,
            &valid_request(&live.addr, &fixture.bearer),
        ));
    }
    responses.push(roundtrip(
        &live.addr,
        &valid_request(&live.addr, &fixture.bearer),
    ));
    let closed = live.service.close_until(deadline(WAIT));
    for response in responses {
        assert_eq!(response.status, 200);
        let value: serde_json::Value = serde_json::from_slice(&response.body).unwrap();
        assert!(
            value["result"].get("isError").is_none(),
            "success must omit isError"
        );
        assert!(value["result"].get("structuredContent").is_some());
    }
    assert_eq!(closed, Ok(()));
}

#[test]
fn concurrent_close_attempts_retain_and_retry_same_service() {
    let fixture = harness(base_snapshot("planning", Vec::new(), &[]));
    let (tools, entered) = HeldTools::new();
    let live = live_service(&fixture, tools.clone());
    let peer = client(&live.addr, &fixture.bearer);
    let entry = entered.recv_timeout(WAIT);
    let service = Arc::new(live.service);
    let attempts: Vec<_> = (0..2)
        .map(|_| {
            let service = service.clone();
            std::thread::spawn(move || service.close_until(deadline(Duration::from_millis(5))))
        })
        .collect();
    let results: Vec<_> = attempts
        .into_iter()
        .map(|attempt| attempt.join().expect("join close attempt"))
        .collect();
    let frozen = fixture.lease.checkpoint().is_err();
    tools.release();
    let peer_result = peer.join().expect("join client");
    let retries: Vec<_> = (0..2)
        .map(|_| {
            let service = service.clone();
            std::thread::spawn(move || service.close_until(deadline(WAIT)))
        })
        .collect();
    let retired: Vec<_> = retries
        .into_iter()
        .map(|retry| retry.join().expect("join close retry"))
        .collect();
    assert!(entry.is_ok());
    assert!(frozen);
    assert!(peer_result.is_ok());
    assert_eq!(
        results,
        vec![
            Err(ServerError::Timeout {
                operation: Operation::Close
            });
            2
        ]
    );
    assert_eq!(retired, vec![Ok(()); 2]);
    assert_eq!(service.close_until(Deadline::at(Instant::now())), Ok(()));
}
