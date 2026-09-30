//! Loopback Agent HTTP v1 wire: schema goldens, echo binding, and byte limits.
//!
//! The expected values come from the frozen contract sources: the HTTP v1
//! schema and goldens under `packages/contracts/companion-agent/http-v1/`,
//! `packages/shared/companion/agent_client.go`, and
//! `packages/server/server/companion_agent.go`. Every byte ceiling is
//! inclusive at the limit and refused one byte past it, and every published
//! response echoes the exact request identity.

use std::collections::VecDeque;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use mornlea_domain::{CommandText, CompanionId};
use mornlea_server::agent::http::{
    AgentHttpWire, Json, MAX_HEADER_BYTES, MAX_REQUEST_BODY_BYTES, MAX_RESPONSE_BODY_BYTES,
    USER_AGENT, decode_success, error_failure, request_body, request_body_within_limit,
    response_body_within_limit, schema_decode,
};
use mornlea_server::agent::lease::{AgentWire, RpcCancellation};
use mornlea_server::contracts::{
    AgentErrorCode, AgentRequest, AgentRequestId, AgentResponse, BaseIdentity, ClientInstanceId,
    Clock, Deadline, LeaseId, LeasedIdentity, NamespaceId, Operation, PlanRequest, RunId,
    ServerError, SnapshotId,
};

/// Real monotonic clock for socket deadlines.
struct WallClock;

impl Clock for WallClock {
    fn monotonic(&self) -> Instant {
        Instant::now()
    }

    fn unix_ms(&self) -> i64 {
        1_800_000_000_000
    }
}

fn clock() -> Arc<dyn Clock + Send + Sync> {
    Arc::new(WallClock)
}

fn uuid(tag: u8) -> [u8; 16] {
    let mut bytes = [0u8; 16];
    bytes[0] = tag;
    bytes[6] = 0x40;
    bytes[8] = 0x80;
    bytes
}

/// A plan request whose MCP endpoint field is the given URL; construction is
/// typed-level only, so the wire's pre-send MCP rules are what refuse or pass.
fn plan_request_with_endpoint(endpoint: &str) -> PlanRequest {
    PlanRequest::try_new(
        LeasedIdentity {
            base: base_identity(4),
            lease_id: LeaseId::try_from_bytes(uuid(8)).unwrap(),
        },
        RunId::try_from_bytes(uuid(5)).unwrap(),
        CompanionId::try_from_bytes(uuid(6)).unwrap(),
        9,
        SnapshotId::try_from_bytes(uuid(7)).unwrap(),
        [0xaa; 32],
        1_800_000_000_000,
        endpoint.to_owned(),
        "test-capability".to_owned(),
        CommandText::try_from_canonical("采一块石头".to_owned()).unwrap(),
    )
    .unwrap()
}

fn uuid_text(bytes: [u8; 16]) -> String {
    let hex: String = bytes.iter().map(|byte| format!("{byte:02x}")).collect();
    format!(
        "{}-{}-{}-{}-{}",
        &hex[0..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..32]
    )
}

fn base_identity(request_tag: u8) -> BaseIdentity {
    BaseIdentity {
        request_id: AgentRequestId::try_from_bytes(uuid(request_tag)).unwrap(),
        client_instance_id: ClientInstanceId::try_from_bytes(uuid(2)).unwrap(),
        namespace_id: NamespaceId::try_from_bytes(uuid(3)).unwrap(),
    }
}

fn acquire_request() -> AgentRequest {
    AgentRequest::Acquire(base_identity(1))
}

/// One observed request on the loopback wire.
#[derive(Clone)]
struct Observed {
    path: String,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

/// Minimal raw HTTP/1.1 server: one scripted response per accepted
/// connection, plus connection counting so the test can prove that a refused
/// redirect is never followed.
struct TestServer {
    endpoint: String,
    observed: Arc<Mutex<Vec<Observed>>>,
    connections: Arc<AtomicUsize>,
    stop: Arc<AtomicBool>,
    join: Option<std::thread::JoinHandle<()>>,
}

impl TestServer {
    fn serve(script: Vec<Vec<u8>>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let endpoint = format!("http://{}", listener.local_addr().unwrap());
        let observed = Arc::new(Mutex::new(Vec::new()));
        let connections = Arc::new(AtomicUsize::new(0));
        let stop = Arc::new(AtomicBool::new(false));
        let script = Mutex::new(VecDeque::from(script));
        let join = {
            let observed = observed.clone();
            let connections = connections.clone();
            let stop = stop.clone();
            std::thread::spawn(move || {
                listener
                    .set_nonblocking(true)
                    .expect("listener nonblocking");
                while !stop.load(Ordering::SeqCst) {
                    let Ok((mut stream, _)) = listener.accept() else {
                        std::thread::sleep(Duration::from_millis(2));
                        continue;
                    };
                    // Accepted sockets inherit nonblocking mode on some platforms;
                    // the scripted peer must wait for the complete bounded request.
                    stream
                        .set_nonblocking(false)
                        .expect("blocking accepted stream");
                    stream
                        .set_write_timeout(Some(Duration::from_secs(5)))
                        .expect("write timeout");
                    connections.fetch_add(1, Ordering::SeqCst);
                    let scripted = script.lock().unwrap().pop_front();
                    let recorded = read_request(&mut stream);
                    if let Ok(observed_request) = recorded {
                        observed.lock().unwrap().push(observed_request);
                    }
                    if let Some(response) = scripted {
                        let _ = stream.write_all(&response);
                        let _ = stream.flush();
                    }
                    let _ = stream.shutdown(std::net::Shutdown::Both);
                }
            })
        };
        Self {
            endpoint,
            observed,
            connections,
            stop,
            join: Some(join),
        }
    }

    fn observed(&self) -> Vec<Observed> {
        self.observed.lock().unwrap().clone()
    }

    fn wire(&self) -> AgentHttpWire {
        AgentHttpWire::try_new(&self.endpoint, "test-agent-secret", clock()).unwrap()
    }
}

impl Drop for TestServer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}

fn read_request(stream: &mut std::net::TcpStream) -> std::io::Result<Observed> {
    stream.set_read_timeout(Some(Duration::from_secs(5)))?;
    let mut buffer = Vec::new();
    let mut chunk = [0u8; 1024];
    let head_end;
    loop {
        if let Some(position) = find_head_end(&buffer) {
            head_end = position;
            break;
        }
        let read = stream.read(&mut chunk)?;
        if read == 0 {
            return Err(std::io::Error::other("connection closed before head end"));
        }
        buffer.extend_from_slice(&chunk[..read]);
    }
    let head = String::from_utf8_lossy(&buffer[..head_end]).into_owned();
    let mut lines = head.split("\r\n");
    let request_line = lines.next().unwrap_or_default().to_owned();
    let mut headers = Vec::new();
    let mut content_length = 0usize;
    for line in lines {
        if line.is_empty() {
            continue;
        }
        if let Some((name, value)) = line.split_once(':') {
            let name = name.trim().to_ascii_lowercase();
            let value = value.trim().to_owned();
            if name == "content-length" {
                content_length = value.parse().unwrap_or(0);
            }
            headers.push((name, value));
        }
    }
    let path = request_line
        .split_whitespace()
        .nth(1)
        .unwrap_or_default()
        .to_owned();
    let mut body = buffer[head_end + 4..].to_vec();
    while body.len() < content_length {
        let read = stream.read(&mut chunk)?;
        if read == 0 {
            break;
        }
        body.extend_from_slice(&chunk[..read]);
    }
    body.truncate(content_length);
    Ok(Observed {
        path,
        headers,
        body,
    })
}

fn find_head_end(buffer: &[u8]) -> Option<usize> {
    buffer.windows(4).position(|window| window == b"\r\n\r\n")
}

fn header_value<'a>(observed: &'a Observed, name: &str) -> Option<&'a str> {
    observed
        .headers
        .iter()
        .find(|(key, _)| key == name)
        .map(|(_, value)| value.as_str())
}

/// Raw HTTP response bytes with the exact headers the test pins.
fn raw_response(status_line: &str, headers: &[(&str, String)], body: &[u8]) -> Vec<u8> {
    let mut bytes = Vec::new();
    bytes.extend_from_slice(status_line.as_bytes());
    bytes.extend_from_slice(b"\r\n");
    for (name, value) in headers {
        bytes.extend_from_slice(name.as_bytes());
        bytes.extend_from_slice(b": ");
        bytes.extend_from_slice(value.as_bytes());
        bytes.extend_from_slice(b"\r\n");
    }
    bytes.extend_from_slice(b"\r\n");
    bytes.extend_from_slice(body);
    bytes
}

fn golden_path(name: &str) -> String {
    format!(
        "{}/../../../../packages/contracts/companion-agent/http-v1/golden/{}",
        env!("CARGO_MANIFEST_DIR"),
        name
    )
}

fn load_golden(name: &str) -> Json {
    Json::parse(&std::fs::read(golden_path(name)).expect("golden file readable"))
        .expect("golden json")
}

fn golden_cases(document: &Json) -> Vec<(String, String, Json)> {
    let cases = document.get("cases").expect("golden cases");
    cases
        .as_array()
        .iter()
        .map(|case| {
            (
                case.get("name").unwrap().as_str().to_owned(),
                case.get("schema").unwrap().as_str().to_owned(),
                case.get("value").unwrap().clone(),
            )
        })
        .collect()
}

#[test]
fn goldens_and_echo_rejection() {
    // Every schema-identical golden value decodes, and the canonical
    // re-encode of the decoded value equals the golden value exactly.
    for (name, schema, value) in golden_cases(&load_golden("valid.json")) {
        let canonical = schema_decode(&schema, &value)
            .unwrap_or_else(|error| panic!("golden {name} rejected: {error:?}"));
        assert!(
            canonical.equivalent(&value),
            "golden {name} did not round-trip"
        );
        let rewritten = Json::parse(&canonical.write()).expect("canonical bytes repARSE");
        assert!(
            rewritten.equivalent(&canonical),
            "golden {name} write drift"
        );
    }
    // Every golden invalid value is rejected before publication.
    for (name, schema, value) in golden_cases(&load_golden("invalid.json")) {
        assert!(
            schema_decode(&schema, &value).is_err(),
            "golden invalid {name} accepted"
        );
    }

    // Response echo binding: a well-formed response whose identity does not
    // match the request is refused before publication.
    let request = acquire_request();
    let AgentRequest::Acquire(base) = &request else {
        panic!("acquire fixture");
    };
    let body = Json::parse(&format!(
            "{{\"contract_version\":\"v1\",\"request_id\":\"{}\",\"client_instance_id\":\"{}\",\"namespace_id\":\"{}\",\"lease_id\":\"{}\",\"lease_expires_in_ms\":15000}}",
            uuid_text(base.request_id.bytes()),
            uuid_text(base.client_instance_id.bytes()),
            uuid_text(base.namespace_id.bytes()),
            uuid_text(uuid(4)),
        )
        .into_bytes(),
    )
    .unwrap();
    assert!(decode_success(&request, &body.write()).is_ok());
    for (field, replacement) in [
        ("request_id", "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa"),
        ("client_instance_id", "bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb"),
        ("namespace_id", "cccccccc-cccc-4ccc-8ccc-cccccccccccc"),
    ] {
        let mutated = body.set(field, Json::Str(replacement.to_owned()));
        assert!(
            decode_success(&request, &mutated.write()).is_err(),
            "acquire response with mismatched {field} published"
        );
    }
    // The expiry constant is part of the closed schema: any other value is
    // refused.
    let mutated = body.set("lease_expires_in_ms", Json::U64(15_001));
    assert!(decode_success(&request, &mutated.write()).is_err());

    // Lease responses bind the exact leased identity: a heartbeat echoing a
    // different lease is refused before publication, while the exact echo
    // publishes.
    let leased = LeasedIdentity {
        base: BaseIdentity {
            request_id: AgentRequestId::try_from_bytes(uuid(9)).unwrap(),
            client_instance_id: ClientInstanceId::try_from_bytes(uuid(2)).unwrap(),
            namespace_id: NamespaceId::try_from_bytes(uuid(3)).unwrap(),
        },
        lease_id: LeaseId::try_from_bytes(uuid(4)).unwrap(),
    };
    let heartbeat = AgentRequest::Heartbeat(leased);
    let mut heartbeat_body = body;
    heartbeat_body = heartbeat_body.set(
        "request_id",
        Json::Str(uuid_text(
            AgentRequestId::try_from_bytes(uuid(9)).unwrap().bytes(),
        )),
    );
    heartbeat_body = heartbeat_body.set(
        "client_instance_id",
        Json::Str(uuid_text(
            ClientInstanceId::try_from_bytes(uuid(2)).unwrap().bytes(),
        )),
    );
    heartbeat_body = heartbeat_body.set(
        "namespace_id",
        Json::Str(uuid_text(
            NamespaceId::try_from_bytes(uuid(3)).unwrap().bytes(),
        )),
    );

    assert!(decode_success(&heartbeat, &heartbeat_body.write()).is_ok());
    let wrong_lease = heartbeat_body.set(
        "lease_id",
        Json::Str("dddddddd-dddd-4ddd-8ddd-dddddddddddd".to_owned()),
    );
    assert!(decode_success(&heartbeat, &wrong_lease.write()).is_err());

    // Error envelopes: only the exact route/status/code pair publishes a
    // typed Agent error; anything else is AgentUnavailable.
    let route = "/v1/namespaces/acquire";
    let envelope =
        br#"{"contract_version":"v1","request_id":null,"error":{"code":"namespace_conflict"}}"#;
    let failure = error_failure(route, 409, &base_identity(1).request_id, envelope);
    assert_eq!(
        failure,
        ServerError::Agent {
            code: AgentErrorCode::NamespaceConflict,
            status: 409,
        }
    );
    // Wrong status for the code, and a code the route never allows.
    let mismatched = error_failure(route, 500, &base_identity(1).request_id, envelope);
    assert_eq!(
        mismatched,
        ServerError::Agent {
            code: AgentErrorCode::AgentUnavailable,
            status: 503,
        }
    );
    let not_found = br#"{"contract_version":"v1","request_id":null,"error":{"code":"not_found"}}"#;
    let forbidden_on_route = error_failure(route, 404, &base_identity(1).request_id, not_found);
    assert_eq!(
        forbidden_on_route,
        ServerError::Agent {
            code: AgentErrorCode::AgentUnavailable,
            status: 503,
        }
    );

    // Exact byte boundaries are inclusive: the limit passes, limit + 1
    // is refused.
    assert!(request_body_within_limit(&vec![
        0u8;
        MAX_REQUEST_BODY_BYTES
    ]));
    assert!(!request_body_within_limit(&vec![
        0u8;
        MAX_REQUEST_BODY_BYTES + 1
    ]));
    assert!(response_body_within_limit(&vec![
        0u8;
        MAX_RESPONSE_BODY_BYTES
    ]));
    assert!(!response_body_within_limit(&vec![
        0u8;
        MAX_RESPONSE_BODY_BYTES
            + 1
    ]));
}

#[test]
fn limits_and_no_extra_keys() {
    // No DNS, no proxy, no non-loopback host: the endpoint constructor only
    // accepts an http loopback IP literal, so the wire can never resolve a
    // name or leave loopback.
    assert!(AgentHttpWire::try_new("http://localhost:9", "secret", clock()).is_err());
    assert!(AgentHttpWire::try_new("http://192.0.2.1:9", "secret", clock()).is_err());
    assert!(AgentHttpWire::try_new("https://127.0.0.1:9", "secret", clock()).is_err());
    assert!(AgentHttpWire::try_new("http://127.0.0.1:0", "", clock()).is_err());
    assert!(AgentHttpWire::try_new("http://127.0.0.1:9", "secret", clock()).is_ok());

    // The plan request's MCP endpoint field requires the /mcp path and an
    // explicit nonzero port: a portless loopback endpoint is refused pre-send
    // with zero connections, while the port-carrying form encodes.
    let portless = plan_request_with_endpoint("http://127.0.0.1/mcp");
    assert!(request_body(&AgentRequest::Plan(portless.clone())).is_err());
    let ported = plan_request_with_endpoint("http://127.0.0.1:45831/mcp");
    assert!(request_body(&AgentRequest::Plan(ported)).is_ok());
    let no_port_server = TestServer::serve(Vec::new());
    let portless_wire =
        AgentHttpWire::try_new(&no_port_server.endpoint, "test-agent-secret", clock()).unwrap();
    let deadline = Deadline::after(clock().monotonic(), Duration::from_secs(5)).unwrap();
    assert!(
        portless_wire
            .rpc(AgentRequest::Plan(portless), deadline)
            .is_err()
    );
    assert_eq!(
        no_port_server.connections.load(Ordering::SeqCst),
        0,
        "portless MCP endpoint reached the wire"
    );
    drop(no_port_server);

    // A valid acquire round-trips with the frozen headers and no retry.
    let request = acquire_request();
    let AgentRequest::Acquire(base) = &request else {
        panic!("acquire fixture");
    };
    let success_body = format!(
        "{{\"contract_version\":\"v1\",\"request_id\":\"{}\",\"client_instance_id\":\"{}\",\"namespace_id\":\"{}\",\"lease_id\":\"{}\",\"lease_expires_in_ms\":15000}}",
        uuid_text(base.request_id.bytes()),
        uuid_text(base.client_instance_id.bytes()),
        uuid_text(base.namespace_id.bytes()),
        uuid_text(uuid(4)),
    )
    .into_bytes();
    let success = raw_response(
        "HTTP/1.1 200 OK",
        &[
            ("Content-Type", "application/json".to_owned()),
            ("Content-Length", success_body.len().to_string()),
        ],
        &success_body,
    );
    let server = TestServer::serve(vec![success]);
    let wire = server.wire();
    let deadline = Deadline::after(clock().monotonic(), Duration::from_secs(5)).unwrap();
    let response = wire.rpc(request.clone(), deadline).expect("acquire ok");
    let AgentResponse::Acquire(lease) = &response else {
        panic!("unexpected response {response:?}");
    };
    assert_eq!(
        lease.leased.lease_id,
        LeaseId::try_from_bytes(uuid(4)).unwrap()
    );
    let observed = server.observed();
    assert_eq!(observed.len(), 1, "exactly one request on the wire");
    assert_eq!(observed[0].path, "/v1/namespaces/acquire");
    assert_eq!(
        header_value(&observed[0], "authorization"),
        Some("Bearer test-agent-secret")
    );
    assert_eq!(
        header_value(&observed[0], "content-type"),
        Some("application/json")
    );
    assert_eq!(header_value(&observed[0], "user-agent"), Some(USER_AGENT));
    assert_eq!(header_value(&observed[0], "connection"), Some("close"));
    assert_eq!(
        header_value(&observed[0], "accept-encoding"),
        Some("identity")
    );
    assert_eq!(
        observed[0].body,
        request_body(&request).expect("request body encodes")
    );
    drop(server);

    // Unknown keys are refused before publication, end to end.
    let extra_key = br#"{"contract_version":"v1","request_id":"11111111-1111-4111-8111-111111111111","client_instance_id":"22222222-2222-4222-8222-222222222222","namespace_id":"33333333-3333-4333-8333-333333333333","lease_id":"44444444-4444-4444-8444-444444444444","lease_expires_in_ms":15000,"attempt":1}"#;
    let server = TestServer::serve(vec![raw_response(
        "HTTP/1.1 200 OK",
        &[
            ("Content-Type", "application/json".to_owned()),
            ("Content-Length", extra_key.len().to_string()),
        ],
        extra_key,
    )]);
    assert!(
        server.wire().rpc(acquire_request(), deadline).is_err(),
        "response with unknown key published"
    );
    drop(server);

    // No redirects: a 302 is refused and no second connection is made.
    let redirect = raw_response(
        "HTTP/1.1 302 Found",
        &[("Location", "http://127.0.0.1:1/moved".to_owned())],
        b"",
    );
    let server = TestServer::serve(vec![redirect]);
    let _ = server.wire().rpc(acquire_request(), deadline);
    std::thread::sleep(Duration::from_millis(50));
    assert_eq!(
        server.connections.load(Ordering::SeqCst),
        1,
        "redirect was followed"
    );
    drop(server);

    // Response body limit: exactly at the limit passes, one byte over is
    // refused both when declared and when streamed.
    let padded = padded_success_body(MAX_RESPONSE_BODY_BYTES);
    let at_limit = server_for_body(&padded);
    assert!(at_limit.wire().rpc(acquire_request(), deadline).is_ok());
    drop(at_limit);
    let mut declared_over = padded_success_body(MAX_RESPONSE_BODY_BYTES + 1);
    let over = server_for_body(&declared_over);
    assert!(over.wire().rpc(acquire_request(), deadline).is_err());
    drop(over);
    declared_over.push(b' ');
    let streamed_over = server_for_body(&declared_over);
    assert!(
        streamed_over
            .wire()
            .rpc(acquire_request(), deadline)
            .is_err()
    );
    drop(streamed_over);

    // Header limit: a response whose header lines sum to exactly the ceiling
    // passes; one byte more is refused. The body is a valid echoing acquire
    // response so the at-limit case fails only on the header rule.
    let body = padded_success_body(300);
    for (target, expected_ok) in [(MAX_HEADER_BYTES, true), (MAX_HEADER_BYTES + 1, false)] {
        let response = response_with_header_budget(target, body.len(), &body);
        let server = TestServer::serve(vec![response]);
        let outcome = server.wire().rpc(acquire_request(), deadline);
        assert_eq!(
            outcome.is_ok(),
            expected_ok,
            "header budget {target}: {:?}",
            outcome.as_ref().err()
        );
        drop(server);
    }
}

/// Valid acquire response echoing the fixture request identity, padded with
/// trailing JSON whitespace to the exact byte size.
fn padded_success_body(total: usize) -> Vec<u8> {
    let base = base_identity(1);
    let mut body = format!(
        "{{\"contract_version\":\"v1\",\"request_id\":\"{}\",\"client_instance_id\":\"{}\",\"namespace_id\":\"{}\",\"lease_id\":\"{}\",\"lease_expires_in_ms\":15000}}",
        uuid_text(base.request_id.bytes()),
        uuid_text(base.client_instance_id.bytes()),
        uuid_text(base.namespace_id.bytes()),
        uuid_text(uuid(4)),
    )
    .into_bytes();
    assert!(body.len() <= total);
    body.resize(total, b' ');
    body
}

fn server_for_body(body: &[u8]) -> TestServer {
    TestServer::serve(vec![raw_response(
        "HTTP/1.1 200 OK",
        &[
            ("Content-Type", "application/json".to_owned()),
            ("Content-Length", body.len().to_string()),
        ],
        body,
    )])
}

/// Raw success response whose counted header bytes equal `target` exactly
/// (each line contributes name + value + 4, mirroring the Go client).
fn response_with_header_budget(target: usize, body_len: usize, body: &[u8]) -> Vec<u8> {
    let content_length = body_len.to_string();
    let counted_without_pad = "Content-Type".len()
        + "application/json".len()
        + 4
        + "Content-Length".len()
        + content_length.len()
        + 4
        + "X-Pad".len()
        + 4;
    assert!(counted_without_pad < target);
    let pad = target - counted_without_pad;
    raw_response(
        "HTTP/1.1 200 OK",
        &[
            ("Content-Type", "application/json".to_owned()),
            ("Content-Length", content_length),
            ("X-Pad", "p".repeat(pad)),
        ],
        body,
    )
}

#[test]
fn accepted_connection_waits_for_request() {
    let server = TestServer::serve(vec![raw_response(
        "HTTP/1.1 200 OK",
        &[("Content-Length", "0".to_owned())],
        &[],
    )]);
    let address = server.endpoint.strip_prefix("http://").unwrap();
    let mut client = std::net::TcpStream::connect(address).unwrap();
    client
        .set_read_timeout(Some(Duration::from_millis(100)))
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(2);
    while server.connections.load(Ordering::SeqCst) == 0 {
        assert!(Instant::now() < deadline, "server did not accept client");
        std::thread::yield_now();
    }
    // A server must not answer before this client has sent its request.
    // This also exercises platforms that inherit the listener's nonblocking mode.
    let mut first = [0u8; 1];
    let early = client.peek(&mut first);
    assert!(
        matches!(early, Err(ref error) if matches!(error.kind(),
        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut)),
        "server replied before receiving the request: {early:?}"
    );
    client
        .set_read_timeout(Some(Duration::from_secs(2)))
        .unwrap();
    client
        .write_all(b"POST /delayed HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Length: 4\r\n\r\nping")
        .unwrap();
    let mut response = Vec::new();
    client.read_to_end(&mut response).unwrap();
    assert!(response.starts_with(b"HTTP/1.1 200 OK"));
    let observed = server.observed();
    assert_eq!(observed.len(), 1);
    assert_eq!(observed[0].path, "/delayed");
    assert_eq!(observed[0].body, b"ping");
}

/// Progress on a socket cannot renew the total RPC deadline.
#[test]
fn response_drips_share_one_absolute_deadline() {
    for body_drip in [false, true] {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let endpoint = format!("http://{}", listener.local_addr().unwrap());
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(1)))
                .unwrap();
            stream
                .set_write_timeout(Some(Duration::from_secs(1)))
                .unwrap();
            read_request(&mut stream).unwrap();
            if body_drip {
                stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 20\r\n\r\n").unwrap();
            }
            let byte = if body_drip { b' ' } else { b'H' };
            for _ in 0..20 {
                if stream.write_all(&[byte]).is_err() {
                    break;
                }
                std::thread::sleep(Duration::from_millis(20));
            }
        });
        let wire = AgentHttpWire::try_new(&endpoint, "secret", clock()).unwrap();
        let started = Instant::now();
        let result = wire.rpc(
            acquire_request(),
            Deadline::at(started + Duration::from_millis(80)),
        );
        let elapsed = started.elapsed();
        server.join().unwrap();
        assert_eq!(
            result,
            Err(ServerError::Timeout {
                operation: Operation::AgentRpc
            }),
            "body drip={body_drip}"
        );
        assert!(
            elapsed < Duration::from_millis(250),
            "body drip={body_drip} renewed deadline: {elapsed:?}"
        );
    }
}

/// Closing the owner interrupts an admitted response without waiting for its
/// much longer business deadline.
#[test]
fn close_interrupts_blocked_response() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    let (accepted, waiting) = std::sync::mpsc::channel();
    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(1)))
            .unwrap();
        read_request(&mut stream).unwrap();
        accepted.send(()).unwrap();
        std::thread::sleep(Duration::from_millis(400));
    });
    let wire = Arc::new(AgentHttpWire::try_new(&endpoint, "secret", clock()).unwrap());
    let rpc_wire = wire.clone();
    let rpc = std::thread::spawn(move || {
        rpc_wire.rpc(
            acquire_request(),
            Deadline::at(Instant::now() + Duration::from_secs(2)),
        )
    });
    waiting.recv_timeout(Duration::from_secs(1)).unwrap();
    let closed = Instant::now();
    wire.close();
    let result = rpc.join().unwrap();
    let elapsed = closed.elapsed();
    server.join().unwrap();
    assert!(result.is_err());
    assert!(
        elapsed < Duration::from_millis(200),
        "closed wire retained the response socket: {elapsed:?}"
    );
}

#[test]
fn cancellation_token_is_shared_and_one_way() {
    let owner = RpcCancellation::default();
    let worker = owner.clone();
    assert!(!owner.is_cancelled());
    std::thread::spawn(move || worker.cancel()).join().unwrap();
    assert!(owner.is_cancelled());
    owner.cancel();
    assert!(owner.clone().is_cancelled());
}

#[test]
fn cancelled_request_opens_no_connection() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    let wire = AgentHttpWire::try_new(&endpoint, "secret", clock()).unwrap();
    let cancellation = RpcCancellation::default();
    cancellation.cancel();
    let outcome = wire.rpc_cancellable(
        acquire_request(),
        Deadline::at(Instant::now() + Duration::from_millis(80)),
        &cancellation,
    );
    assert_eq!(
        outcome,
        Err(ServerError::Agent {
            code: AgentErrorCode::AgentUnavailable,
            status: 503,
        })
    );
    assert_eq!(
        listener.accept().unwrap_err().kind(),
        std::io::ErrorKind::WouldBlock
    );
}

#[test]
fn cancellation_retires_only_its_response_socket() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    let (accepted, waiting) = std::sync::mpsc::channel();
    let server = std::thread::spawn(move || {
        let accept = || {
            let until = Instant::now() + Duration::from_secs(2);
            loop {
                match listener.accept() {
                    Ok((stream, _)) => {
                        stream.set_nonblocking(false).unwrap();
                        stream
                            .set_read_timeout(Some(Duration::from_secs(1)))
                            .unwrap();
                        stream
                            .set_write_timeout(Some(Duration::from_secs(1)))
                            .unwrap();
                        break stream;
                    }
                    Err(error)
                        if error.kind() == std::io::ErrorKind::WouldBlock
                            && Instant::now() < until =>
                    {
                        std::thread::sleep(Duration::from_millis(2));
                    }
                    Err(error) => panic!("bounded fixture accept: {error}"),
                }
            }
        };
        let mut first = accept();
        read_request(&mut first).unwrap();
        accepted.send(()).unwrap();
        let closed = matches!(first.read(&mut [0]), Ok(0));
        drop(first);
        let mut second = accept();
        read_request(&mut second).unwrap();
        let body = padded_success_body(1024);
        let response = raw_response(
            "HTTP/1.1 200 OK",
            &[
                ("Content-Type", "application/json".to_owned()),
                ("Content-Length", body.len().to_string()),
            ],
            &body,
        );
        second.write_all(&response).unwrap();
        closed
    });
    let wire = Arc::new(AgentHttpWire::try_new(&endpoint, "secret", clock()).unwrap());
    let cancellation = RpcCancellation::default();
    let worker_cancel = cancellation.clone();
    let worker_wire = wire.clone();
    let rpc = std::thread::spawn(move || {
        worker_wire.rpc_cancellable(
            acquire_request(),
            Deadline::at(Instant::now() + Duration::from_secs(2)),
            &worker_cancel,
        )
    });
    waiting.recv_timeout(Duration::from_secs(1)).unwrap();
    let started = Instant::now();
    cancellation.cancel();
    let result = rpc.join().unwrap();
    let elapsed = started.elapsed();
    let independent = wire.rpc(
        acquire_request(),
        Deadline::at(Instant::now() + Duration::from_secs(1)),
    );
    let first_closed = server.join().unwrap();
    assert_eq!(
        result,
        Err(ServerError::Agent {
            code: AgentErrorCode::AgentUnavailable,
            status: 503,
        })
    );
    assert!(
        elapsed < Duration::from_millis(200),
        "request cancellation retained socket: {elapsed:?}"
    );
    assert!(first_closed, "retired request must release its socket");
    assert!(matches!(independent, Ok(AgentResponse::Acquire(_))));
}
