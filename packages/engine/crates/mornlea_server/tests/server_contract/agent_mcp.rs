//! Frozen MCP service: contract matrix, cancellation, and byte limits.
//!
//! Expected values mirror the frozen contract sources: the MCP v1 manifest
//! and schema under `packages/contracts/companion-agent/mcp-v1/`, the goldens
//! beside them, and `packages/server/server/companion_mcp*.go` (outer
//! validation order, SDK dispatch shape, loopback service lifecycle). The
//! service is a stateless HTTP POST endpoint at loopback `/mcp` speaking
//! protocol `2025-11-25`; there are no sessions, SSE, batches, or extra
//! capabilities.

use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant};

use mornlea_domain::{
    BlockPos, ChunkPos, CommandText, CompanionId, FiniteVec3, LookAngles, PlayerId,
};
use mornlea_server::agent::http::parse_canonical_uuid;
use mornlea_server::agent::mcp::{
    FrozenTools, MAX_MCP_REQUEST_BYTES, MAX_MCP_RESPONSE_BYTES, MAX_PLAN_INPUT_BYTES,
    MCP_APP_VERSION, MCP_ENDPOINT_PATH, MCP_PROTOCOL_VERSION, McpService, PlanningTools, ToolFault,
    ToolSuccess, load_contract, tool_canonical_limit, tool_domain_codes, tool_max_calls_per_run,
    tool_model_visible,
};
use mornlea_server::agent::snapshot::{SnapshotEntropy, SnapshotLease, SnapshotRegistry};
use mornlea_server::contracts::{
    Clock, Deadline, NamespaceId, PlanningSnapshot, ServerError, SnapshotBlock,
    SnapshotChunkRevision, SnapshotCompanion, SnapshotIssuer, SnapshotPlayer, SnapshotPort,
    SnapshotTaskStatusText, SnapshotTerrain,
};
use mornlea_storage::ItemStack;

// ---------------------------------------------------------------------------
// Fixtures shared by this topic.
// ---------------------------------------------------------------------------

struct StepClock {
    now: Mutex<Instant>,
}

impl StepClock {
    fn start() -> (Instant, Arc<Self>) {
        let now = Instant::now();
        (
            now,
            Arc::new(Self {
                now: Mutex::new(now),
            }),
        )
    }

    fn advance(&self, delta: Duration) {
        let mut now = self.now.lock().unwrap();
        *now = now.checked_add(delta).expect("advance");
    }
}

impl Clock for StepClock {
    fn monotonic(&self) -> Instant {
        *self.now.lock().unwrap()
    }

    fn unix_ms(&self) -> i64 {
        1_800_000_000_000
    }
}

struct CycleEntropy {
    next: Mutex<u8>,
}

impl SnapshotEntropy for CycleEntropy {
    fn fill(&self, out: &mut [u8; 32]) -> Result<(), ServerError> {
        let mut next = self.next.lock().unwrap();
        for slot in out.iter_mut() {
            *slot = *next;
            *next = next.wrapping_add(1);
        }
        Ok(())
    }
}

fn player_id(text: &str) -> PlayerId {
    PlayerId::try_from_bytes(parse_canonical_uuid(text).expect("fixture uuid"))
        .expect("fixture player id")
}

fn companion_id(text: &str) -> CompanionId {
    CompanionId::try_from_bytes(parse_canonical_uuid(text).expect("fixture uuid"))
        .expect("fixture companion id")
}

const ISSUER_ID: &str = "99999999-9999-4999-8999-999999999999";
const COMPANION_ID: &str = "66666666-6666-4666-8666-666666666666";
const NAMESPACE_ID: &str = "4d5e6f70-8192-4aa3-8b4f-3a4b5c6d7e8f";

/// Inventory shared by every MCP fixture: stone x64 in slot 0, chest x1 in
/// slot 4, matching the checked-in inventory golden.
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

fn terrain_with(origin: BlockPos, blocks: &[(BlockPos, u16)], height: i16) -> SnapshotTerrain {
    let mut ready = vec![0u8; 137];
    for byte in ready.iter_mut().take(136) {
        *byte = 0xff;
    }
    ready[136] = 0x01;
    let heights = vec![height; 1089];
    let mut plane = vec![0u16; 18_513];
    for (position, block) in blocks {
        let dx = position.x() - origin.x();
        let dy = position.y() - origin.y();
        let dz = position.z() - origin.z();
        plane[(dx as usize * 17 + dy as usize) * 33 + dz as usize] = *block;
    }
    SnapshotTerrain::try_new(origin, [33, 17, 33], ready, heights, plane).expect("terrain")
}

/// Base snapshot both golden comparisons build on: issuer `9999…` at
/// `(4.5, 64, -1.5)` looking at `(8, 63, -2)`, companion `6666…` at
/// `(5.5, 64, -1.5)` with the planning-status fixture, world time 1200, one revision.
fn base_snapshot(
    instruction: &str,
    exposed: Vec<SnapshotBlock>,
    blocks: &[(BlockPos, u16)],
) -> PlanningSnapshot {
    PlanningSnapshot::try_new(
        99,
        1200,
        CommandText::try_from_canonical(instruction.to_owned()).unwrap(),
        SnapshotIssuer {
            player_id: player_id(ISSUER_ID),
            position: FiniteVec3::try_new([4.5, 64.0, -1.5]).unwrap(),
            look: LookAngles::try_new(90.0, 0.0).unwrap(),
            look_hit: Some(BlockPos::new(8, 63, -2)),
        },
        SnapshotCompanion {
            companion_id: companion_id(COMPANION_ID),
            position: FiniteVec3::try_new([5.5, 64.0, -1.5]).unwrap(),
            look: LookAngles::try_new(0.0, 0.0).unwrap(),
            task_status: SnapshotTaskStatusText::try_new("规划中".to_owned()).unwrap(),
            inventory: fixture_inventory(),
        },
        vec![SnapshotPlayer {
            player_id: player_id(ISSUER_ID),
            position: FiniteVec3::try_new([4.5, 64.0, -1.5]).unwrap(),
            look: LookAngles::try_new(90.0, 0.0).unwrap(),
            look_hit: Some(BlockPos::new(8, 63, -2)),
        }],
        vec![SnapshotChunkRevision {
            pos: ChunkPos::new(0, -1),
            revision: 17,
        }],
        exposed,
        terrain_with(BlockPos::new(-11, 56, -18), blocks, 63),
    )
    .expect("snapshot")
}

fn block_at(x: i32, y: i32, z: i32, block: u16) -> SnapshotBlock {
    SnapshotBlock {
        position: BlockPos::new(x, y, z),
        block_id: block,
    }
}

struct Harness {
    clock: Arc<StepClock>,
    registry: SnapshotRegistry,
    bearer: String,
    lease: SnapshotLease,
    digest: String,
}

fn harness(snapshot: PlanningSnapshot) -> Harness {
    let (start, clock) = StepClock::start();
    let entropy = Arc::new(CycleEntropy {
        next: Mutex::new(0),
    });
    let mut registry =
        SnapshotRegistry::try_new(clock.clone(), entropy, "http://127.0.0.1:9/mcp".to_owned())
            .expect("registry");
    let companion = companion_id(COMPANION_ID);
    let registration = registry
        .register(
            NamespaceId::try_from_bytes(parse_canonical_uuid(NAMESPACE_ID).unwrap()).unwrap(),
            companion,
            1,
            snapshot,
            Deadline::at(start + Duration::from_secs(60)),
        )
        .expect("register");
    let bearer = bearer(&registration.capability);
    let lease = registry.lookup(&bearer).expect("lookup");
    let digest = lease.digest_hex();
    Harness {
        clock,
        registry,
        bearer,
        lease,
        digest,
    }
}

/// Renders raw capability bytes as unpadded base64url, the HTTP/MCP bearer.
fn bearer(capability: &[u8]) -> String {
    const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut text = String::with_capacity(43);
    for chunk in capability.chunks(3) {
        let word = match chunk.len() {
            3 => ((chunk[0] as u32) << 16) | ((chunk[1] as u32) << 8) | chunk[2] as u32,
            2 => ((chunk[0] as u32) << 16) | ((chunk[1] as u32) << 8),
            _ => (chunk[0] as u32) << 16,
        };
        let width = match chunk.len() {
            3 => 4,
            2 => 3,
            _ => 2,
        };
        for index in 0..width {
            text.push(ALPHABET[((word >> (18 - 6 * index)) & 63) as usize] as char);
        }
    }
    text
}

fn contract_path(name: &str) -> String {
    format!(
        "{}/../../../../packages/contracts/companion-agent/mcp-v1/{}",
        env!("CARGO_MANIFEST_DIR"),
        name
    )
}

fn load_text(name: &str) -> Vec<u8> {
    std::fs::read(contract_path(name)).expect("contract file readable")
}

// ---------------------------------------------------------------------------
// Raw HTTP loopback client.
// ---------------------------------------------------------------------------

struct Response {
    status: u16,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

impl Response {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
    }
}

fn roundtrip(addr: &str, request: &[u8]) -> Response {
    let mut stream = TcpStream::connect(addr).expect("connect");
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .expect("timeout");
    stream.write_all(request).expect("write");
    // A refused connection may reset after the response instead of a clean
    // FIN; bytes already received still form the complete response.
    let mut raw = Vec::new();
    let mut chunk = [0u8; 8192];
    loop {
        match stream.read(&mut chunk) {
            Ok(0) => break,
            Ok(read) => raw.extend_from_slice(&chunk[..read]),
            Err(error)
                if !raw.is_empty()
                    && (error.kind() == std::io::ErrorKind::ConnectionReset
                        || error.kind() == std::io::ErrorKind::TimedOut) =>
            {
                break;
            }
            Err(error) => panic!("read: {error}"),
        }
    }
    let split = raw
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .expect("header end");
    let head = String::from_utf8(raw[..split].to_vec()).expect("header utf8");
    let mut lines = head.lines();
    let status_line = lines.next().expect("status");
    let status: u16 = status_line
        .split_whitespace()
        .nth(1)
        .expect("status code")
        .parse()
        .expect("status number");
    let mut headers = Vec::new();
    for line in lines {
        let (name, value) = line.split_once(':').expect("header colon");
        headers.push((name.trim().to_owned(), value.trim().to_owned()));
    }
    Response {
        status,
        headers,
        body: raw[split + 4..].to_vec(),
    }
}

fn request(method: &str, path: &str, headers: &[(&str, &str)], body: &[u8]) -> Vec<u8> {
    let mut request = format!("{method} {path} HTTP/1.1\r\n").into_bytes();
    for (name, value) in headers {
        request.extend_from_slice(format!("{name}: {value}\r\n").as_bytes());
    }
    request.extend_from_slice(format!("Content-Length: {}\r\n", body.len()).as_bytes());
    request.extend_from_slice(b"\r\n");
    request.extend_from_slice(body);
    request
}

struct LiveService {
    service: McpService,
    addr: String,
}

fn live_service(harness: &Harness, tools: Arc<dyn PlanningTools>) -> LiveService {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let addr = listener.local_addr().expect("addr").to_string();
    let service =
        McpService::try_new_with(harness.registry.clone(), tools, listener).expect("service");
    LiveService { service, addr }
}

fn standard_headers(
    service: &LiveService,
    harness: &Harness,
    protocol: bool,
) -> Vec<(String, String)> {
    let mut headers = vec![
        ("Host".to_owned(), service.addr.clone()),
        (
            "Authorization".to_owned(),
            format!("Bearer {}", harness.bearer),
        ),
        ("Content-Type".to_owned(), "application/json".to_owned()),
        (
            "Accept".to_owned(),
            "application/json, text/event-stream".to_owned(),
        ),
    ];
    if protocol {
        headers.push(("Mcp-Protocol-Version".to_owned(), "2025-11-25".to_owned()));
    }
    headers
}

fn call(
    service: &LiveService,
    harness: &Harness,
    protocol: bool,
    mutate: impl FnOnce(&mut String, &mut Vec<(String, String)>, &mut Vec<u8>, &mut String),
    body: &str,
) -> Response {
    let mut headers = standard_headers(service, harness, protocol);
    let mut owned = body.as_bytes().to_vec();
    let mut path = "/mcp".to_owned();
    let mut method = "POST".to_owned();
    mutate(&mut method, &mut headers, &mut owned, &mut path);
    let borrowed: Vec<(&str, &str)> = headers
        .iter()
        .map(|(name, value)| (name.as_str(), value.as_str()))
        .collect();
    roundtrip(&service.addr, &request(&method, &path, &borrowed, &owned))
}

fn remove_header(headers: &mut Vec<(String, String)>, name: &str) {
    headers.retain(|(key, _)| !key.eq_ignore_ascii_case(name));
}

fn set_header(headers: &mut Vec<(String, String)>, name: &str, value: &str) {
    remove_header(headers, name);
    headers.push((name.to_owned(), value.to_owned()));
}

fn envelope(id: u64, method: &str, params: &str) -> String {
    format!("{{\"jsonrpc\":\"2.0\",\"id\":{id},\"method\":\"{method}\",\"params\":{params}}}")
}

fn initialize_body() -> String {
    envelope(
        1,
        "initialize",
        "{\"protocolVersion\":\"2025-11-25\",\"capabilities\":{},\"clientInfo\":{\"name\":\"test\",\"version\":\"1\"}}",
    )
}

fn tool_call_body(id: u64, name: &str, arguments: &str) -> String {
    envelope(
        id,
        "tools/call",
        &format!("{{\"name\":\"{name}\",\"arguments\":{arguments}}}"),
    )
}

fn assert_outer_error(response: &Response) {
    assert!(
        response.status >= 400,
        "status {} not an error",
        response.status
    );
    assert_eq!(
        response.header("Content-Type"),
        Some("application/json"),
        "error content type"
    );
    let body = String::from_utf8(response.body.clone()).expect("error utf8");
    let parsed: serde_json::Value = serde_json::from_str(&body).expect("error json");
    let code = parsed
        .get("error")
        .and_then(|error| error.get("code"))
        .and_then(|code| code.as_str())
        .expect("error code");
    assert_eq!(
        body,
        format!("{{\"error\":{{\"code\":\"{code}\"}}}}"),
        "error wire shape"
    );
    assert!(
        !body.contains("LEAK-ME-NOT"),
        "error body leaked input: {body}"
    );
}

// ---------------------------------------------------------------------------
// Counting and gated tool doubles.
// ---------------------------------------------------------------------------

struct CountingTools {
    calls: AtomicUsize,
    inner: FrozenTools,
}

impl PlanningTools for CountingTools {
    fn execute(
        &self,
        lease: &SnapshotLease,
        tool: &str,
        input: &[u8],
    ) -> Result<ToolSuccess, ToolFault> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.inner.execute(lease, tool, input)
    }
}

struct GateTools {
    gate: Mutex<Option<mpsc::Receiver<()>>>,
}

impl PlanningTools for GateTools {
    fn execute(
        &self,
        _lease: &SnapshotLease,
        _tool: &str,
        _input: &[u8],
    ) -> Result<ToolSuccess, ToolFault> {
        let receiver = self.gate.lock().unwrap().take().expect("gate armed");
        receiver
            .recv_timeout(Duration::from_secs(10))
            .expect("gate released");
        Ok(ToolSuccess {
            canonical: b"{\"gated\":true}".to_vec(),
            domain_failure: false,
        })
    }
}

struct FixedTools {
    canonical: Vec<u8>,
}

impl PlanningTools for FixedTools {
    fn execute(
        &self,
        _lease: &SnapshotLease,
        _tool: &str,
        _input: &[u8],
    ) -> Result<ToolSuccess, ToolFault> {
        Ok(ToolSuccess {
            canonical: self.canonical.clone(),
            domain_failure: false,
        })
    }
}

// ---------------------------------------------------------------------------
// Tests.
// ---------------------------------------------------------------------------

#[test]
fn schema_tools_protocol_matrix() {
    let manifest: serde_json::Value =
        serde_json::from_slice(&load_text("manifest.json")).expect("manifest");
    let schema: serde_json::Value =
        serde_json::from_slice(&load_text("schema.json")).expect("schema");
    let contract = load_contract().expect("contract");
    assert_eq!(contract.protocol_version, MCP_PROTOCOL_VERSION);
    assert_eq!(contract.protocol_version, "2025-11-25");
    assert_eq!(contract.app_version, MCP_APP_VERSION);
    assert_eq!(contract.endpoint_path, MCP_ENDPOINT_PATH);
    assert_eq!(contract.endpoint_path, "/mcp");
    assert!(contract.stateless && contract.json_response && !contract.sse && !contract.sessions);
    assert_eq!(contract.request_limit, MAX_MCP_REQUEST_BYTES);
    assert_eq!(contract.request_limit, 262_144);
    assert_eq!(contract.response_limit, MAX_MCP_RESPONSE_BYTES);
    assert_eq!(contract.response_limit, 163_840);
    assert_eq!(contract.plan_limit, MAX_PLAN_INPUT_BYTES);
    assert_eq!(contract.plan_limit, 65_536);
    assert_eq!(
        manifest["mcp_protocol_version"],
        serde_json::Value::String(contract.protocol_version.clone())
    );

    let want_order = [
        "get_planning_context",
        "list_affordances",
        "inspect_inventory",
        "find_visible_blocks",
        "query_terrain",
        "validate_plan",
    ];
    assert_eq!(contract.tools.len(), want_order.len());
    let defs = schema.get("$defs").expect("defs");
    let manifest_tools = manifest["tools"].as_array().expect("manifest tools");
    for (index, tool) in contract.tools.iter().enumerate() {
        assert_eq!(tool.name, want_order[index], "tool order drift");
        assert_eq!(tool.name, manifest_tools[index]["name"].as_str().unwrap());
        assert_eq!(
            tool.canonical_limit,
            tool_canonical_limit(&tool.name).unwrap()
        );
        assert_eq!(tool.domain_codes, tool_domain_codes(&tool.name).unwrap());
        assert_eq!(tool.model_visible, tool_model_visible(&tool.name).unwrap());
        assert_eq!(
            tool.max_calls_per_run,
            tool_max_calls_per_run(&tool.name),
            "call budget drift for {}",
            tool.name
        );
        // Resolved schemas carry no `$ref` and decode to objects.
        assert!(
            !tool.input_schema.contains("$ref"),
            "unresolved input {}",
            tool.name
        );
        assert!(
            !tool.output_schema.contains("$ref"),
            "unresolved output {}",
            tool.name
        );
        let input: serde_json::Value =
            serde_json::from_str(&tool.input_schema).expect("input schema");
        let output: serde_json::Value =
            serde_json::from_str(&tool.output_schema).expect("output schema");
        assert_eq!(
            input.get("type").and_then(|kind| kind.as_str()),
            Some("object")
        );
        let _ = output;
        let input_def = manifest_tools[index]["input_schema"].as_str().unwrap();
        let output_def = manifest_tools[index]["result_schema"].as_str().unwrap();
        assert!(
            defs.get(input_def).is_some(),
            "missing input def {input_def}"
        );
        assert!(
            defs.get(output_def).is_some(),
            "missing output def {output_def}"
        );
    }
    assert_eq!(tool_max_calls_per_run("validate_plan"), Some(2));
    assert_eq!(tool_model_visible("get_planning_context"), Some(false));
    assert_eq!(tool_model_visible("list_affordances"), Some(true));
    assert_eq!(tool_model_visible("inspect_inventory"), Some(true));
    assert_eq!(tool_model_visible("find_visible_blocks"), Some(true));
    assert_eq!(tool_model_visible("query_terrain"), Some(true));
    assert_eq!(tool_model_visible("validate_plan"), Some(false));
    assert_eq!(
        tool_domain_codes("find_visible_blocks").unwrap(),
        &["unknown_block"]
    );
    assert_eq!(
        tool_domain_codes("query_terrain").unwrap(),
        &["out_of_bounds"]
    );
    assert_eq!(
        tool_domain_codes("validate_plan").unwrap(),
        &[
            "invalid_schema",
            "out_of_bounds",
            "unknown_player",
            "unmineable_target",
            "unknown_block",
            "missing_item",
            "snapshot_mismatch"
        ]
    );

    // The place enum is the fixed 23-name delivery set in manifest order.
    let place_names = [
        "brick",
        "chest",
        "clay",
        "cobblestone",
        "dirt",
        "furnace",
        "glass",
        "grass",
        "gravel",
        "iron_block",
        "leaves",
        "light_block",
        "mossy_cobblestone",
        "oak_log",
        "oak_planks",
        "roof_tile",
        "sand",
        "smooth_stone",
        "snow_block",
        "stone",
        "stone_brick",
        "white_wool",
        "workbench",
    ];
    let validate_input = contract
        .tools
        .iter()
        .find(|tool| tool.name == "validate_plan")
        .expect("validate tool");
    let parsed: serde_json::Value =
        serde_json::from_str(&validate_input.input_schema).expect("validate schema");
    let steps = &parsed["properties"]["plan"]["properties"]["steps"]["items"]["oneOf"];
    let mut names = Vec::new();
    for branch in steps.as_array().expect("oneOf") {
        if let Some(values) = branch["properties"]
            .get("block")
            .and_then(|block| block.get("enum"))
            .and_then(|values| values.as_array())
        {
            names.extend(values.iter().map(|name| name.as_str().unwrap().to_owned()));
        }
    }
    assert_eq!(names, place_names, "place enum drift");

    // Golden tool results equal live tool output on matching fixtures, modulo
    // the fake golden digest.
    let goldens: serde_json::Value =
        serde_json::from_slice(&load_text("golden/valid.json")).expect("valid goldens");
    let golden = |schema_name: &str, case: &str| -> serde_json::Value {
        goldens["cases"]
            .as_array()
            .unwrap()
            .iter()
            .find(|entry| entry["schema"] == schema_name && entry["name"] == case)
            .unwrap_or_else(|| panic!("golden {schema_name}/{case}"))["value"]
            .clone()
    };

    let context_harness = harness(base_snapshot("采一块石头后跟上玩家", Vec::new(), &[]));
    let tools = FrozenTools;
    let context = tools
        .execute(&context_harness.lease, "get_planning_context", b"{}")
        .expect("context");
    assert!(!context.domain_failure);
    let mut context_golden = golden(
        "get_planning_context_result",
        "planning context is a bounded projection",
    );
    context_golden["snapshot_digest"] = serde_json::Value::String(context_harness.digest.clone());
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&context.canonical).unwrap(),
        context_golden,
        "context output drifted from golden"
    );

    let affordance_harness = harness(base_snapshot(
        "采一块石头后跟上玩家",
        vec![block_at(8, 63, -2, 2), block_at(9, 63, -2, 11)],
        &[
            (BlockPos::new(8, 63, -2), 2),
            (BlockPos::new(9, 63, -2), 11),
        ],
    ));
    let affordances = tools
        .execute(&affordance_harness.lease, "list_affordances", b"{}")
        .expect("affordances");
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&affordances.canonical).unwrap(),
        golden(
            "list_affordances_result",
            "affordances include fixed steps and mine classifications"
        ),
        "affordance output drifted from golden"
    );
    let inventory = tools
        .execute(
            &affordance_harness.lease,
            "inspect_inventory",
            r#"{"offset":0,"limit":36}"#.as_bytes(),
        )
        .expect("inventory");
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&inventory.canonical).unwrap(),
        golden(
            "inspect_inventory_result",
            "inventory result contains bounded occupied slots"
        ),
        "inventory output drifted from golden"
    );

    let find_harness = harness(base_snapshot(
        "找箱子",
        vec![block_at(9, 63, -2, 11), block_at(10, 63, -2, 9)],
        &[
            (BlockPos::new(9, 63, -2), 11),
            (BlockPos::new(10, 63, -2), 9),
        ],
    ));
    let found = tools
        .execute(
            &find_harness.lease,
            "find_visible_blocks",
            r#"{"block_names":["chest","furnace"],"limit":64}"#.as_bytes(),
        )
        .expect("find");
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&found.canonical).unwrap(),
        golden(
            "find_visible_blocks_result",
            "visible block result is coordinate ordered"
        ),
        "find output drifted from golden"
    );

    let query = tools
        .execute(
            &context_harness.lease,
            "query_terrain",
            r#"{"positions":[{"x":4,"y":64,"z":-1},{"x":8,"y":64,"z":-2}]}"#.as_bytes(),
        )
        .expect("query");
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&query.canonical).unwrap(),
        golden(
            "query_terrain_result",
            "terrain result preserves input order"
        ),
        "query output drifted from golden"
    );

    // The delivered validate_plan golden input is accepted against a fixture
    // carrying its mine target, place stock, and follow target.
    let validate_harness = harness(base_snapshot(
        "采一块石头",
        vec![block_at(8, 63, -2, 2)],
        &[(BlockPos::new(8, 63, -2), 2)],
    ));
    let accepted_input = golden(
        "validate_plan_input",
        "validator accepts the complete delivered step set",
    );
    let accepted = tools
        .execute(
            &validate_harness.lease,
            "validate_plan",
            serde_json::to_string(&accepted_input).unwrap().as_bytes(),
        )
        .expect("validate");
    assert!(!accepted.domain_failure);
    let accepted_value: serde_json::Value = serde_json::from_slice(&accepted.canonical).unwrap();
    assert_eq!(accepted_value["accepted"], serde_json::Value::Bool(true));
    assert_eq!(
        accepted_value["snapshot_digest"],
        serde_json::Value::String(validate_harness.digest.clone())
    );
    assert_eq!(
        accepted_value["plan"]["steps"], accepted_input["plan"]["steps"],
        "canonical plan echo drift"
    );

    // Invalid goldens fail on the matching tool path.
    let invalid: serde_json::Value =
        serde_json::from_slice(&load_text("golden/invalid.json")).expect("invalid goldens");
    let invalid_cases: HashMap<(&str, &str), serde_json::Value> = invalid["cases"]
        .as_array()
        .unwrap()
        .iter()
        .map(|entry| {
            (
                (
                    entry["schema"].as_str().unwrap(),
                    entry["name"].as_str().unwrap(),
                ),
                entry["value"].clone(),
            )
        })
        .collect();
    let invalid_value = |schema_name: &str, case: &str| -> Vec<u8> {
        serde_json::to_vec(&invalid_cases[&(schema_name, case)]).unwrap()
    };
    let tools_ref = &tools;
    let rejects_input = |tool: &str, input: &[u8]| {
        assert!(
            matches!(
                tools_ref.execute(&validate_harness.lease, tool, input),
                Err(ToolFault::InvalidInput)
            ),
            "{tool} accepted invalid input {}",
            String::from_utf8_lossy(input)
        );
    };
    rejects_input(
        "inspect_inventory",
        &invalid_value("inspect_inventory_input", "inventory offset is bounded"),
    );
    rejects_input(
        "inspect_inventory",
        &invalid_value("inspect_inventory_input", "inventory limit is bounded"),
    );
    rejects_input(
        "find_visible_blocks",
        r#"{"block_names":["chest","chest"],"limit":2}"#.as_bytes(),
    );
    rejects_input(
        "find_visible_blocks",
        r#"{"block_names":[],"limit":1}"#.as_bytes(),
    );
    rejects_input(
        "query_terrain",
        &invalid_value("query_terrain_input", "terrain query cannot be empty"),
    );
    rejects_input(
        "query_terrain",
        &invalid_value(
            "query_terrain_input",
            "terrain query y is a world block coordinate",
        ),
    );
    for (tool, input) in [
        (
            "get_planning_context",
            r#"{"snapshot_id":"77777777-7777-4777-8777-777777777777"}"#.as_bytes(),
        ),
        (
            "list_affordances",
            r#"{"snapshot_id":"77777777-7777-4777-8777-777777777777"}"#.as_bytes(),
        ),
    ] {
        rejects_input(tool, input);
    }
    // Every `bounded_name` golden is rejected as a block name argument.
    let bounded: Vec<&serde_json::Value> = invalid["cases"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|entry| entry["schema"] == "bounded_name")
        .collect();
    assert!(bounded.len() >= 6, "bounded_name goldens missing");
    for entry in &bounded {
        let name = serde_json::to_string(&entry["value"]).unwrap();
        let input = format!("{{\"block_names\":[{name}],\"limit\":1}}");
        rejects_input("find_visible_blocks", input.as_bytes());
    }
    // A bad name never masks a later schema violation: unknown registered
    // names resolve first, so this is a domain failure, not invalid input.
    let domain = tools
        .execute(
            &validate_harness.lease,
            "find_visible_blocks",
            r#"{"block_names":["missing_block"],"limit":1}"#.as_bytes(),
        )
        .expect("domain failure");
    assert!(domain.domain_failure);
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&domain.canonical).unwrap()["code"],
        serde_json::Value::String("unknown_block".to_owned())
    );

    // Validator failures carry one stable code and a bounded hint, and never
    // the success fields.
    let failures = [
        (
            r#"{"plan":{"summary":"向前走","steps":[{"kind":"go_to","x":1,"y":64,"z":0}],"tool":"move"}}"#.as_bytes(),
            "invalid_schema",
        ),
        (
            r#"{"plan":{"summary":"x","steps":[{"kind":"attack","x":1,"y":64,"z":0}]}}"#.as_bytes(),
            "invalid_schema",
        ),
        (
            r#"{"plan":{"summary":"x","steps":[]}}"#.as_bytes(),
            "invalid_schema",
        ),
        (
            r#"{"plan":{"summary":"挖","steps":[{"kind":"mine","x":8,"y":63,"z":-2}]}}"#.as_bytes(),
            "unmineable_target",
        ),
    ];
    for (input, code) in failures {
        let outcome = tools
            .execute(&context_harness.lease, "validate_plan", input)
            .expect("validator failure");
        assert!(outcome.domain_failure, "validator error for {code}");
        let value: serde_json::Value = serde_json::from_slice(&outcome.canonical).unwrap();
        assert_eq!(value["accepted"], serde_json::Value::Bool(false));
        assert_eq!(value["code"], serde_json::Value::String(code.to_owned()));
        assert!(value.get("plan").is_none() && value.get("snapshot_digest").is_none());
        let hint = value["hint"].as_str().expect("hint text");
        assert!(!hint.is_empty() && hint.len() <= 256, "hint bound");
    }
    // The empty-terrain context has only air outside its frozen blocks: the
    // same mine one voxel past the projection edge is out of bounds there,
    // proving validation reads the frozen projection.
    let oob = tools
        .execute(
            &context_harness.lease,
            "validate_plan",
            r#"{"plan":{"summary":"挖","steps":[{"kind":"mine","x":1000,"y":63,"z":-2}]}}"#
                .as_bytes(),
        )
        .expect("oob failure");
    let oob_value: serde_json::Value = serde_json::from_slice(&oob.canonical).unwrap();
    assert_eq!(
        oob_value["code"],
        serde_json::Value::String("out_of_bounds".to_owned())
    );

    // Protocol matrix over raw HTTP: every malformed request is refused
    // before dispatch, and two valid origins each dispatch exactly once.
    // The closure receives method, headers, body, and path to mutate.
    let counting = Arc::new(CountingTools {
        calls: AtomicUsize::new(0),
        inner: FrozenTools,
    });
    let live = live_service(&context_harness, counting.clone());
    type Mutate = Box<dyn Fn(&mut String, &mut Vec<(String, String)>, &mut Vec<u8>, &mut String)>;
    let valid_initialize = initialize_body();
    let valid_list = envelope(2, "tools/list", "{}");
    let bad_version = valid_initialize.replace("2025-11-25", "2025-06-18");
    let oversize = "x".repeat(MAX_MCP_REQUEST_BYTES + 1);
    let invalid_utf8 = vec![b'{', 0xff, b'}'];
    let matrix: Vec<(&str, String, Mutate, bool)> = vec![
        (
            "get",
            valid_initialize.clone(),
            Box::new(|method, _, _, _| *method = "GET".to_owned()),
            true,
        ),
        (
            "get wrong path",
            valid_initialize.clone(),
            Box::new(|method, _, _, path| {
                *method = "GET".to_owned();
                *path = "/other".to_owned();
            }),
            true,
        ),
        (
            "wrong path",
            valid_initialize.clone(),
            Box::new(|_, _, _, path| *path = "/other".to_owned()),
            true,
        ),
        (
            "missing host",
            valid_initialize.clone(),
            Box::new(|_, headers, _, _| set_header(headers, "Host", "")),
            true,
        ),
        (
            "wrong host",
            valid_initialize.clone(),
            Box::new(|_, headers, _, _| set_header(headers, "Host", "127.0.0.1:9")),
            true,
        ),
        (
            "missing bearer",
            valid_initialize.clone(),
            Box::new(|_, headers, _, _| remove_header(headers, "Authorization")),
            true,
        ),
        (
            "malformed bearer",
            valid_initialize.clone(),
            Box::new(|_, headers, _, _| set_header(headers, "Authorization", "Basic abc")),
            true,
        ),
        (
            "wrong bearer",
            valid_initialize.clone(),
            Box::new(|_, headers, _, _| set_header(headers, "Authorization", "Bearer wrong")),
            true,
        ),
        (
            "duplicate bearer",
            valid_initialize.clone(),
            Box::new(|_, headers, _, _| {
                headers.push(("Authorization".to_owned(), "Bearer extra".to_owned()))
            }),
            true,
        ),
        (
            "wrong origin",
            valid_initialize.clone(),
            Box::new(|_, headers, _, _| set_header(headers, "Origin", "http://127.0.0.1:9")),
            true,
        ),
        (
            "https origin",
            valid_initialize.clone(),
            Box::new({
                let origin = format!("https://{}", live.addr);
                move |_, headers: &mut Vec<(String, String)>, _, _| {
                    set_header(headers, "Origin", &origin)
                }
            }),
            true,
        ),
        (
            "duplicate origin",
            valid_initialize.clone(),
            Box::new({
                let origin = format!("http://{}", live.addr);
                move |_, headers: &mut Vec<(String, String)>, _, _| {
                    headers.push(("Origin".to_owned(), origin.clone()));
                    headers.push(("Origin".to_owned(), origin.clone()));
                }
            }),
            true,
        ),
        (
            "missing content type",
            valid_initialize.clone(),
            Box::new(|_, headers, _, _| remove_header(headers, "Content-Type")),
            true,
        ),
        (
            "wrong content type",
            valid_initialize.clone(),
            Box::new(|_, headers, _, _| set_header(headers, "Content-Type", "text/plain")),
            true,
        ),
        (
            "duplicate content type",
            valid_initialize.clone(),
            Box::new(|_, headers, _, _| {
                headers.push(("Content-Type".to_owned(), "application/json".to_owned()))
            }),
            true,
        ),
        ("empty", String::new(), Box::new(|_, _, _, _| {}), true),
        (
            "trailing",
            valid_initialize.clone() + "{}",
            Box::new(|_, _, _, _| {}),
            true,
        ),
        (
            "array batch",
            format!("[{valid_initialize}]"),
            Box::new(|_, _, _, _| {}),
            true,
        ),
        (
            "ping",
            envelope(1, "ping", "{}"),
            Box::new(|_, _, _, _| {}),
            true,
        ),
        (
            "subscription",
            envelope(1, "subscriptions/listen", "{}"),
            Box::new(|_, _, _, _| {}),
            true,
        ),
        (
            "unknown method",
            envelope(1, "resources/list", "{}"),
            Box::new(|_, _, _, _| {}),
            true,
        ),
        (
            "bad jsonrpc",
            valid_initialize.clone().replace("\"2.0\"", "\"1.0\""),
            Box::new(|_, _, _, _| {}),
            true,
        ),
        (
            "missing request id",
            "{\"jsonrpc\":\"2.0\",\"method\":\"tools/list\",\"params\":{}}".to_owned(),
            Box::new(|_, _, _, _| {}),
            false,
        ),
        (
            "notification has id",
            envelope(1, "notifications/initialized", "{}"),
            Box::new(|_, _, _, _| {}),
            false,
        ),
        (
            "initialize wrong version",
            bad_version.clone(),
            Box::new(|_, _, _, _| {}),
            true,
        ),
        (
            "initialize wrong header",
            valid_initialize.clone(),
            Box::new(|_, headers, _, _| set_header(headers, "Mcp-Protocol-Version", "2025-06-18")),
            true,
        ),
        (
            "post-init missing header",
            valid_list.clone(),
            Box::new(|_, headers, _, _| remove_header(headers, "Mcp-Protocol-Version")),
            false,
        ),
        (
            "post-init wrong header",
            valid_list.clone(),
            Box::new(|_, headers, _, _| set_header(headers, "Mcp-Protocol-Version", "2025-06-18")),
            false,
        ),
        (
            "oversize",
            oversize.clone(),
            Box::new(|_, _, _, _| {}),
            true,
        ),
    ];
    for (name, body, mutate, protocol) in matrix {
        // `invalid_utf8` is handled separately below because it is not UTF-8.
        let response = call(&live, &context_harness, protocol, mutate, &body);
        assert_outer_error(&response);
        assert_eq!(
            counting.calls.load(Ordering::SeqCst),
            0,
            "{name} reached dispatch"
        );
        let _ = name;
    }
    {
        let response = call(
            &live,
            &context_harness,
            true,
            |_, _, body, _| *body = invalid_utf8.clone(),
            &valid_initialize,
        );
        assert_outer_error(&response);
    }
    assert_eq!(counting.calls.load(Ordering::SeqCst), 0);

    // A wrong capability yields the identical body for a valid and an
    // invalid envelope, so envelope validity never leaks through errors.
    let mut wrong_bodies = Vec::new();
    for body in [&valid_initialize, "{\"not\":\"an MCP envelope\"}"] {
        let response = call(
            &live,
            &context_harness,
            true,
            |_, headers, _, _| set_header(headers, "Authorization", "Bearer wrong-capability"),
            body,
        );
        assert_eq!(response.status, 401, "wrong capability status");
        wrong_bodies.push(String::from_utf8(response.body.clone()).expect("utf8"));
    }
    assert_eq!(
        wrong_bodies[0], wrong_bodies[1],
        "wrong capability leaked envelope validity"
    );
    assert_eq!(wrong_bodies[0], "{\"error\":{\"code\":\"unauthorized\"}}");

    // Both allowed origins dispatch exactly once. The probe is a real tool
    // call, since initialize and tools/list are answered by the dispatcher
    // without reaching the tool runner.
    for origin in [None, Some(format!("http://{}", live.addr))] {
        let response = call(
            &live,
            &context_harness,
            true,
            |_, headers, _, _| {
                remove_header(headers, "Origin");
                if let Some(origin) = &origin {
                    headers.push(("Origin".to_owned(), origin.clone()));
                }
            },
            &tool_call_body(31, "get_planning_context", "{}"),
        );
        assert_eq!(response.status, 200, "valid origin: {}", response.status);
        if origin.is_none() {
            let value: serde_json::Value =
                serde_json::from_slice(&response.body).expect("success call json");
            let result = value.get("result").expect("success result wrapper");
            assert!(result.get("isError").is_none(), "success omits isError");
            assert_eq!(result["content"].as_array().expect("content").len(), 1);
            assert!(result.get("structuredContent").is_some(), "success content");
        }
    }
    assert_eq!(counting.calls.load(Ordering::SeqCst), 2);

    // Initialize advertises exactly the frozen capability set and version,
    // notifications answer 202 with an empty body, and tools/list follows
    // the manifest order with identical schemas.
    let initialized = call(
        &live,
        &context_harness,
        true,
        |_, _, _, _| {},
        &valid_initialize,
    );
    let initialized_value: serde_json::Value =
        serde_json::from_slice(&initialized.body).expect("initialize json");
    assert_eq!(
        initialized_value["result"]["capabilities"],
        serde_json::json!({"tools": {"listChanged": false}})
    );
    assert_eq!(
        initialized_value["result"]["protocolVersion"],
        serde_json::json!("2025-11-25")
    );
    let notified = call(
        &live,
        &context_harness,
        true,
        |_, _, _, _| {},
        "{\"jsonrpc\":\"2.0\",\"method\":\"notifications/initialized\",\"params\":{}}",
    );
    assert_eq!(notified.status, 202, "notification status");
    assert!(notified.body.is_empty(), "notification body");
    let listed = call(&live, &context_harness, true, |_, _, _, _| {}, &valid_list);
    assert_eq!(listed.status, 200);
    let listed_value: serde_json::Value = serde_json::from_slice(&listed.body).expect("list json");
    let served = listed_value["result"]["tools"].as_array().expect("tools");
    assert_eq!(served.len(), contract.tools.len());
    for (index, served_tool) in served.iter().enumerate() {
        assert_eq!(
            served_tool["name"],
            serde_json::json!(contract.tools[index].name)
        );
        let served_input: serde_json::Value =
            serde_json::from_str(&contract.tools[index].input_schema).unwrap();
        let served_output: serde_json::Value =
            serde_json::from_str(&contract.tools[index].output_schema).unwrap();
        assert_eq!(served_tool["inputSchema"], served_input);
        assert_eq!(served_tool["outputSchema"], served_output);
    }

    // Unknown tools and transport failures are controlled errors: no input
    // echo, no structured content.
    let unknown = call(
        &live,
        &context_harness,
        true,
        |_, _, _, _| {},
        &tool_call_body(7, "LEAK-ME-NOT", "{}"),
    );
    assert_eq!(
        String::from_utf8(unknown.body.clone()).expect("utf8"),
        "{\"error\":{\"code\":-32603,\"message\":\"unavailable\"},\"id\":7,\"jsonrpc\":\"2.0\"}"
    );
    assert!(
        !String::from_utf8(unknown.body)
            .unwrap()
            .contains("LEAK-ME-NOT")
    );

    // A missing tool name is the same controlled method error.
    let nameless = call(
        &live,
        &context_harness,
        true,
        |_, _, _, _| {},
        &envelope(9, "tools/call", "{\"arguments\":{}}"),
    );
    assert_eq!(
        String::from_utf8(nameless.body.clone()).expect("utf8"),
        "{\"error\":{\"code\":-32603,\"message\":\"unavailable\"},\"id\":9,\"jsonrpc\":\"2.0\"}"
    );

    // Invalid tool input is a controlled tool error: `isError: true` with
    // the fixed text and no structured content.
    let invalid_call = call(
        &live,
        &context_harness,
        true,
        |_, _, _, _| {},
        &tool_call_body(8, "inspect_inventory", "{\"offset\":99,\"limit\":1}"),
    );
    assert_eq!(invalid_call.status, 200);
    let invalid_value: serde_json::Value =
        serde_json::from_slice(&invalid_call.body).expect("invalid call json");
    let result = &invalid_value["result"];
    assert_eq!(result["isError"], serde_json::json!(true));
    assert!(result.get("structuredContent").is_none());
    let content = result["content"].as_array().expect("content");
    assert_eq!(content.len(), 1);
    assert_eq!(
        content[0]["text"],
        serde_json::json!("{\"code\":\"unavailable\"}")
    );
    assert_eq!(content[0]["type"], serde_json::json!("text"));

    live.service.close();
}

/// Slow-arriving bodies wait instead of refusing: accepted sockets inherit
/// the nonblocking listener mode, and the server restores blocking reads so
/// an early read waits for bytes rather than surfacing `WouldBlock` as a
/// bogus 400 or 503.
#[test]
fn slow_body_waits_for_bytes() {
    let harness = harness(base_snapshot("采一块石头", Vec::new(), &[]));
    let live = live_service(&harness, Arc::new(FrozenTools));
    let headers = standard_headers(&live, &harness, true);
    let borrowed: Vec<(&str, &str)> = headers
        .iter()
        .map(|(name, value)| (name.as_str(), value.as_str()))
        .collect();
    let full = request(
        "POST",
        "/mcp",
        &borrowed,
        tool_call_body(60, "get_planning_context", "{}").as_bytes(),
    );
    let split = full
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .map(|index| index + 4)
        .expect("head");
    let mut stream = TcpStream::connect(&live.addr).expect("connect");
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .expect("timeout");
    stream.write_all(&full[..split]).expect("head");
    std::thread::sleep(Duration::from_millis(200));
    stream.write_all(&full[split..]).expect("body");
    let mut raw = Vec::new();
    let mut chunk = [0u8; 8192];
    loop {
        match stream.read(&mut chunk) {
            Ok(0) => break,
            Ok(read) => raw.extend_from_slice(&chunk[..read]),
            Err(error) => panic!("read: {error}"),
        }
    }
    let text = String::from_utf8(raw).expect("utf8");
    assert!(
        text.starts_with("HTTP/1.1 200"),
        "slow body refused: {text}"
    );
    let body = text.split("\r\n\r\n").nth(1).expect("body");
    let value: serde_json::Value = serde_json::from_str(body).expect("json");
    assert!(value.get("result").is_some(), "success wrapper missing");
    live.service.close();
}

#[test]
fn cancel_before_after_encode() {
    use mornlea_server::contracts::SnapshotPort;

    let pre_harness = harness(base_snapshot("采一块石头", Vec::new(), &[]));
    let counting = Arc::new(CountingTools {
        calls: AtomicUsize::new(0),
        inner: FrozenTools,
    });
    let live = live_service(&pre_harness, counting.clone());

    // A record cancelled before dispatch never reaches the tools: the outer
    // layer answers unauthorized (the capability no longer authorizes)
    // without running or encoding anything. Deeper 503/502 paths cover
    // races where the record dies after authorization.
    let victim = pre_harness
        .registry
        .lookup(&pre_harness.bearer)
        .expect("victim");
    let mut registry = pre_harness.registry.clone();
    SnapshotPort::cancel(&mut registry, victim.snapshot_id()).expect("cancel record");
    assert!(victim.checkpoint().is_err());
    let cancelled = call(
        &live,
        &pre_harness,
        true,
        |_, _, _, _| {},
        &tool_call_body(11, "get_planning_context", "{}"),
    );
    assert_eq!(cancelled.status, 401, "cancelled capability status");
    assert_outer_error(&cancelled);
    assert_eq!(
        counting.calls.load(Ordering::SeqCst),
        0,
        "cancelled request dispatched"
    );
    live.service.close();

    // Cancellation during tool execution is rechecked before the response is
    // committed: the gated run is released only after its lease dies.
    let gated_harness = harness(base_snapshot("采一块石头", Vec::new(), &[]));
    let (release, gate) = mpsc::channel::<()>();
    let gated = Arc::new(GateTools {
        gate: Mutex::new(Some(gate)),
    });
    let live = live_service(&gated_harness, gated);
    let bearer = gated_harness.bearer.clone();
    let addr = live.addr.clone();
    let handle = std::thread::spawn(move || {
        let auth = format!("Bearer {bearer}");
        let headers = [
            ("Host", addr.as_str()),
            ("Authorization", auth.as_str()),
            ("Content-Type", "application/json"),
            ("Accept", "application/json, text/event-stream"),
            ("Mcp-Protocol-Version", "2025-11-25"),
        ];
        let body = tool_call_body(12, "get_planning_context", "{}");
        roundtrip(&addr, &request("POST", "/mcp", &headers, body.as_bytes()))
    });
    // Wait until the gated tool is inside execution, then cancel the record.
    std::thread::sleep(Duration::from_millis(200));
    let victim = gated_harness
        .registry
        .lookup(&gated_harness.bearer)
        .expect("victim");
    let mut registry = gated_harness.registry.clone();
    SnapshotPort::cancel(&mut registry, victim.snapshot_id()).expect("cancel victim");
    release.send(()).expect("release gate");
    let response = handle.join().expect("handler joined");
    assert!(
        response.status >= 400,
        "cancelled run committed: {}",
        response.status
    );
    assert_outer_error(&response);
    assert!(
        !response
            .body
            .windows(7)
            .any(|window| window == b"\"gated\""),
        "cancelled run leaked tool output"
    );

    // An expired record no longer authorizes after the clock advances.
    gated_harness.clock.advance(Duration::from_secs(61));
    let expired = call(
        &live,
        &gated_harness,
        true,
        |_, _, _, _| {},
        &tool_call_body(13, "get_planning_context", "{}"),
    );
    assert_eq!(expired.status, 401, "expired capability status");
    assert_outer_error(&expired);
    live.service.close();
}

#[test]
fn exact_byte_limits() {
    assert_eq!(MAX_MCP_REQUEST_BYTES, 262_144);
    assert_eq!(MAX_MCP_RESPONSE_BYTES, 163_840);
    let harness = harness(base_snapshot("采一块石头", Vec::new(), &[]));
    let counting = Arc::new(CountingTools {
        calls: AtomicUsize::new(0),
        inner: FrozenTools,
    });
    let live = live_service(&harness, counting);

    // A request of exactly the limit is accepted; trailing spaces are valid
    // JSON whitespace. One byte more is refused before dispatch.
    let prefix = envelope(
        21,
        "tools/call",
        "{\"name\":\"get_planning_context\",\"arguments\":{}}",
    );
    let mut exact = prefix.clone();
    exact.push_str(&" ".repeat(MAX_MCP_REQUEST_BYTES - prefix.len()));
    assert_eq!(exact.len(), MAX_MCP_REQUEST_BYTES);
    let accepted = call(&live, &harness, true, |_, _, _, _| {}, &exact);
    assert_eq!(accepted.status, 200, "exact request refused");
    assert!(serde_json::from_slice::<serde_json::Value>(&accepted.body).is_ok());
    let over = exact + " ";
    let refused = call(&live, &harness, true, |_, _, _, _| {}, &over);
    assert_eq!(refused.status, 413, "oversize request accepted");
    assert_outer_error(&refused);

    // A response of exactly the limit is accepted; one byte more fails after
    // dispatch without committing the oversized bytes.
    let probe = FixedTools {
        canonical: b"{\"padding\":\"\"}".to_vec(),
    };
    let probe_live = live_service(&harness, Arc::new(probe));
    let probe_response = call(
        &probe_live,
        &harness,
        true,
        |_, _, _, _| {},
        &tool_call_body(22, "get_planning_context", "{}"),
    );
    assert_eq!(probe_response.status, 200);
    let growth_per_byte = 2;
    let padding = (MAX_MCP_RESPONSE_BYTES - probe_response.body.len()) / growth_per_byte;
    let exact_canonical = format!("{{\"padding\":\"{}\"}}", "r".repeat(padding));
    let exact_live = live_service(
        &harness,
        Arc::new(FixedTools {
            canonical: exact_canonical.into_bytes(),
        }),
    );
    let exact_response = call(
        &exact_live,
        &harness,
        true,
        |_, _, _, _| {},
        &tool_call_body(23, "get_planning_context", "{}"),
    );
    // Padding grows the body two bytes per character, so a parity miss lands
    // one byte short; a one-digit-longer id recovers the exact limit.
    let exact_response = if exact_response.body.len() == MAX_MCP_RESPONSE_BYTES - 1 {
        call(
            &exact_live,
            &harness,
            true,
            |_, _, _, _| {},
            &tool_call_body(233, "get_planning_context", "{}"),
        )
    } else {
        exact_response
    };
    assert_eq!(
        exact_response.body.len(),
        MAX_MCP_RESPONSE_BYTES,
        "response fixture missed the limit"
    );
    assert_eq!(exact_response.status, 200, "exact response refused");
    assert!(serde_json::from_slice::<serde_json::Value>(&exact_response.body).is_ok());
    let over_live = live_service(
        &harness,
        Arc::new(FixedTools {
            canonical: format!("{{\"padding\":\"{}\"}}", "r".repeat(padding + 1)).into_bytes(),
        }),
    );
    let over_response = call(
        &over_live,
        &harness,
        true,
        |_, _, _, _| {},
        &tool_call_body(24, "get_planning_context", "{}"),
    );
    assert!(over_response.status >= 400, "oversize response committed");
    assert_outer_error(&over_response);
    assert!(over_response.body.len() <= 1024);

    // A validate_plan candidate past the 64 KiB canonical bound is a domain
    // failure, not a transport error: the method contract still answers.
    let big_summary = "界".repeat(22_000);
    let big_input = format!(
        "{{\"plan\":{{\"summary\":\"{big_summary}\",\"steps\":[{{\"kind\":\"go_to\",\"x\":1,\"y\":64,\"z\":1}}]}}}}"
    );
    assert!(big_input.len() > MAX_PLAN_INPUT_BYTES);
    let tools = FrozenTools;
    let outcome = tools
        .execute(&harness.lease, "validate_plan", big_input.as_bytes())
        .expect("oversize plan answered");
    assert!(outcome.domain_failure);
    let value: serde_json::Value = serde_json::from_slice(&outcome.canonical).unwrap();
    assert_eq!(value["code"], serde_json::json!("invalid_schema"));

    live.service.close();
    probe_live.service.close();
    exact_live.service.close();
    over_live.service.close();
}

#[test]
fn service_loopback_close_and_serve_failure_isolation() {
    let main = harness(base_snapshot("采一块石头", Vec::new(), &[]));
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let addr = listener.local_addr().expect("addr").to_string();
    let service = McpService::try_new_with(main.registry.clone(), Arc::new(FrozenTools), listener)
        .expect("service");
    assert_eq!(service.authority(), addr);
    assert_eq!(service.endpoint(), format!("http://{addr}/mcp"));
    assert!(addr.starts_with("127.0.0.1:"));

    // The live endpoint answers tools/list over real TCP.
    let auth = format!("Bearer {}", main.bearer);
    let headers = [
        ("Host", addr.as_str()),
        ("Authorization", auth.as_str()),
        ("Content-Type", "application/json"),
        ("Accept", "application/json, text/event-stream"),
        ("Mcp-Protocol-Version", "2025-11-25"),
    ];
    let response = roundtrip(
        &addr,
        &request(
            "POST",
            "/mcp",
            &headers,
            envelope(2, "tools/list", "{}").as_bytes(),
        ),
    );
    assert_eq!(response.status, 200);

    // Close is idempotent, closes the registry first, then the listener.
    service.close();
    service.close();
    assert!(
        main.registry.lookup(&main.bearer).is_err(),
        "close left the registry open"
    );
    assert!(
        TcpStream::connect(&addr).is_err(),
        "close left the listener open"
    );
    assert!(
        service.wait_done(Duration::from_secs(5)).is_some(),
        "close did not settle serve"
    );

    // A dead listener settles the serve channel with an error while the
    // registry and the rest of the process stay usable.
    let fresh = harness(base_snapshot("采一块石头", Vec::new(), &[]));
    let poisoned = McpService::try_new_with(
        fresh.registry.clone(),
        Arc::new(FrozenTools),
        closed_listener(),
    )
    .expect("poisoned service");
    let settled = poisoned
        .wait_done(Duration::from_secs(5))
        .expect("serve did not settle");
    assert!(settled.is_err(), "serve failure swallowed");
    assert!(
        fresh.registry.lookup(&fresh.bearer).is_ok(),
        "serve failure killed the registry"
    );
    poisoned.close();
}

#[cfg(unix)]
fn closed_listener() -> TcpListener {
    use std::os::unix::io::FromRawFd;
    // A bound datagram socket has an address but refuses accept, which is
    // exactly the serve failure path without touching the network.
    let fd = unsafe { libc::socket(libc::AF_INET, libc::SOCK_DGRAM, 0) };
    assert!(fd >= 0, "udp socket");
    let addr = libc::sockaddr_in {
        sin_len: std::mem::size_of::<libc::sockaddr_in>() as u8,
        sin_family: libc::AF_INET as u8,
        sin_port: 0,
        sin_addr: libc::in_addr {
            s_addr: u32::from_ne_bytes([127, 0, 0, 1]),
        },
        sin_zero: [0; 8],
    };
    let bound = unsafe {
        libc::bind(
            fd,
            &addr as *const libc::sockaddr_in as *const libc::sockaddr,
            std::mem::size_of::<libc::sockaddr_in>() as u32,
        )
    };
    assert_eq!(bound, 0, "udp bind");
    unsafe { TcpListener::from_raw_fd(fd) }
}

#[cfg(not(unix))]
fn closed_listener() -> TcpListener {
    use std::os::windows::io::FromRawSocket;
    unsafe { TcpListener::from_raw_socket(u64::MAX as usize) }
}
