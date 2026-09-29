//! Loopback Agent HTTP v1 client wire.
//!
//! The expected values are frozen by the HTTP v1 contract sources: the schema
//! and goldens under `packages/contracts/companion-agent/http-v1/` and
//! `packages/shared/companion/agent_client.go`. The wire is one request per
//! connection against a loopback IP literal: no DNS, no proxy, no redirect
//! follow, and no automatic retry; the fixed bearer credential is the only
//! authentication. Every byte ceiling is inclusive at the limit and refused
//! one byte past it, and a response is published only after the
//! closed-schema decode and the exact request echo binding both pass.

use std::io::{Read, Write};
use std::net::{IpAddr, SocketAddr, TcpStream};
use std::num::NonZeroU64;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use mornlea_domain::PlayerId;

use crate::agent::lease::AgentWire;
use crate::contracts::{
    AGENT_CONTRACT_VERSION, AgentErrorCode, AgentPlan, AgentRequest, AgentRequestId, AgentResponse,
    BaseIdentity, CancelRequest, CancelResponse, ClientInstanceId, Clock, CommitRequest,
    CommitResponse, Deadline, DeleteRequest, DeleteResponse, DialogueEnvironment, DialogueFact,
    DialogueFailure, DialogueProgress, DialogueRequest, DialogueResponse, LEASE_EXPIRES_IN_MS,
    LeaseId, LeaseResponse, LeasedIdentity, MemoryProposal, MemoryState, NamespaceId, Operation,
    OperationId, PlanBlock, PlanRequest, PlanResponse, PlanStep, ReconcileRequest,
    ReconcileResponse, ServerError,
};

/// Outbound JSON body ceiling (256 KiB), mirroring the Go client's
/// `AgentMaxRequestBodyBytes`.
pub const MAX_REQUEST_BODY_BYTES: usize = 262_144;
/// Inbound JSON body ceiling (64 KiB), mirroring `AgentMaxResponseBodyBytes`.
pub const MAX_RESPONSE_BODY_BYTES: usize = 65_536;
/// HTTP header ceiling (16 KiB) in both directions, mirroring
/// `AgentMaxHeaderBytes`.
pub const MAX_HEADER_BYTES: usize = 16_384;
/// Fixed wire user agent so the header budget cannot drift with a default.
pub const USER_AGENT: &str = "mornlea-companion-agent-http-v1";

/// Business RPC bound, mirroring the Go planner's default 30 s timeout.
pub const BUSINESS_RPC_TIMEOUT: Duration = Duration::from_secs(30);

// ---------------------------------------------------------------------------
// Strict JSON value
// ---------------------------------------------------------------------------

/// Parsed JSON value restricted to the shapes the HTTP v1 contract uses: no
/// floating-point numbers and no duplicate object keys.
#[derive(Clone, Debug, PartialEq)]
pub enum Json {
    Null,
    Bool(bool),
    U64(u64),
    I64(i64),
    Str(String),
    Array(Vec<Json>),
    Object(Vec<(String, Json)>),
}

impl Json {
    /// Strict parse: valid UTF-8, no duplicate keys, no unpaired surrogates,
    /// no trailing content after the single top-level value.
    pub fn parse(bytes: &[u8]) -> Result<Json, ServerError> {
        let invalid = || invalid_json();
        let text = std::str::from_utf8(bytes).map_err(|_| invalid())?;
        let mut parser = Parser {
            input: text,
            pos: 0,
            depth: 0,
        };
        let value = parser.parse_value()?;
        parser.skip_whitespace();
        if parser.pos != text.len() {
            return Err(invalid());
        }
        Ok(value)
    }

    /// Serializes the value back to JSON bytes.
    pub fn write(&self) -> Vec<u8> {
        let mut out = Vec::new();
        self.write_into(&mut out);
        out
    }

    fn write_into(&self, out: &mut Vec<u8>) {
        match self {
            Json::Null => out.extend_from_slice(b"null"),
            Json::Bool(true) => out.extend_from_slice(b"true"),
            Json::Bool(false) => out.extend_from_slice(b"false"),
            Json::U64(value) => out.extend_from_slice(value.to_string().as_bytes()),
            Json::I64(value) => out.extend_from_slice(value.to_string().as_bytes()),
            Json::Str(text) => write_json_string(text, out),
            Json::Array(items) => {
                out.push(b'[');
                for (index, item) in items.iter().enumerate() {
                    if index > 0 {
                        out.push(b',');
                    }
                    item.write_into(out);
                }
                out.push(b']');
            }
            Json::Object(entries) => {
                out.push(b'{');
                for (index, (key, value)) in entries.iter().enumerate() {
                    if index > 0 {
                        out.push(b',');
                    }
                    write_json_string(key, out);
                    out.push(b':');
                    value.write_into(out);
                }
                out.push(b'}');
            }
        }
    }

    /// Order-insensitive structural equality for objects, so a canonical
    /// re-encode can be compared against a golden value.
    pub fn equivalent(&self, other: &Json) -> bool {
        match (self, other) {
            (Json::Object(left), Json::Object(right)) => {
                left.len() == right.len()
                    && left.iter().all(|(key, value)| {
                        right.iter().any(|(other_key, other_value)| {
                            key == other_key && value.equivalent(other_value)
                        })
                    })
            }
            (Json::Array(left), Json::Array(right)) => {
                left.len() == right.len()
                    && left
                        .iter()
                        .zip(right.iter())
                        .all(|(one, other)| one.equivalent(other))
            }
            _ => self == other,
        }
    }

    pub fn get(&self, key: &str) -> Option<&Json> {
        match self {
            Json::Object(entries) => entries
                .iter()
                .find(|(name, _)| name == key)
                .map(|(_, value)| value),
            _ => None,
        }
    }

    /// Clones the object with one key's value replaced, preserving order.
    pub fn set(&self, key: &str, value: Json) -> Json {
        match self {
            Json::Object(entries) => Json::Object(
                entries
                    .iter()
                    .map(|(name, existing)| {
                        if name == key {
                            (name.clone(), value.clone())
                        } else {
                            (name.clone(), existing.clone())
                        }
                    })
                    .collect(),
            ),
            _ => self.clone(),
        }
    }

    pub fn as_str(&self) -> &str {
        match self {
            Json::Str(text) => text,
            _ => "",
        }
    }

    pub fn as_array(&self) -> &[Json] {
        match self {
            Json::Array(items) => items,
            _ => &[],
        }
    }

    pub fn as_u64(&self) -> Option<u64> {
        match self {
            Json::U64(value) => Some(*value),
            _ => None,
        }
    }

    fn extend_object(&mut self, entries: &[(&str, Json)]) {
        if let Json::Object(existing) = self {
            existing.extend(
                entries
                    .iter()
                    .map(|(key, value)| ((*key).to_owned(), value.clone())),
            );
        }
    }
}

fn write_json_string(text: &str, out: &mut Vec<u8>) {
    out.push(b'"');
    for character in text.chars() {
        match character {
            '"' => out.extend_from_slice(b"\\\""),
            '\\' => out.extend_from_slice(b"\\\\"),
            '\u{0008}' => out.extend_from_slice(b"\\b"),
            '\u{000c}' => out.extend_from_slice(b"\\f"),
            '\n' => out.extend_from_slice(b"\\n"),
            '\r' => out.extend_from_slice(b"\\r"),
            '\t' => out.extend_from_slice(b"\\t"),
            character if (character as u32) < 0x20 => {
                out.extend_from_slice(format!("\\u{:04x}", character as u32).as_bytes());
            }
            character => {
                let mut buffer = [0u8; 4];
                out.extend_from_slice(character.encode_utf8(&mut buffer).as_bytes());
            }
        }
    }
    out.push(b'"');
}

fn invalid_json() -> ServerError {
    ServerError::InvalidInput {
        field: "agent_json",
    }
}

struct Parser<'a> {
    input: &'a str,
    pos: usize,
    depth: usize,
}

/// Bounds recursive descent so a hostile body cannot exhaust the stack.
const MAX_JSON_DEPTH: usize = 128;

impl<'a> Parser<'a> {
    fn invalid(&self) -> ServerError {
        invalid_json()
    }

    fn skip_whitespace(&mut self) {
        while let Some(byte) = self.input.as_bytes().get(self.pos) {
            match byte {
                b' ' | b'\t' | b'\n' | b'\r' => self.pos += 1,
                _ => break,
            }
        }
    }

    fn peek(&self) -> Option<u8> {
        self.input.as_bytes().get(self.pos).copied()
    }

    fn parse_value(&mut self) -> Result<Json, ServerError> {
        self.skip_whitespace();
        match self.peek() {
            Some(b'{') => self.parse_object(),
            Some(b'[') => self.parse_array(),
            Some(b'"') => Ok(Json::Str(self.parse_string()?)),
            Some(b't') => self.parse_literal("true", Json::Bool(true)),
            Some(b'f') => self.parse_literal("false", Json::Bool(false)),
            Some(b'n') => self.parse_literal("null", Json::Null),
            Some(byte) if byte == b'-' || byte.is_ascii_digit() => self.parse_number(),
            _ => Err(self.invalid()),
        }
    }

    fn parse_literal(&mut self, literal: &str, value: Json) -> Result<Json, ServerError> {
        if self.input[self.pos..].starts_with(literal) {
            self.pos += literal.len();
            Ok(value)
        } else {
            Err(self.invalid())
        }
    }

    fn parse_object(&mut self) -> Result<Json, ServerError> {
        self.depth += 1;
        if self.depth > MAX_JSON_DEPTH {
            return Err(self.invalid());
        }
        self.pos += 1;
        let mut entries: Vec<(String, Json)> = Vec::new();
        self.skip_whitespace();
        if self.peek() == Some(b'}') {
            self.pos += 1;
            self.depth -= 1;
            return Ok(Json::Object(entries));
        }
        loop {
            self.skip_whitespace();
            if self.peek() != Some(b'"') {
                return Err(self.invalid());
            }
            let key = self.parse_string()?;
            self.skip_whitespace();
            if self.peek() != Some(b':') {
                return Err(self.invalid());
            }
            self.pos += 1;
            let value = self.parse_value()?;
            if entries.iter().any(|(name, _)| *name == key) {
                return Err(self.invalid());
            }
            entries.push((key, value));
            self.skip_whitespace();
            match self.peek() {
                Some(b',') => self.pos += 1,
                Some(b'}') => {
                    self.pos += 1;
                    self.depth -= 1;
                    return Ok(Json::Object(entries));
                }
                _ => return Err(self.invalid()),
            }
        }
    }

    fn parse_array(&mut self) -> Result<Json, ServerError> {
        self.depth += 1;
        if self.depth > MAX_JSON_DEPTH {
            return Err(self.invalid());
        }
        self.pos += 1;
        let mut items = Vec::new();
        self.skip_whitespace();
        if self.peek() == Some(b']') {
            self.pos += 1;
            self.depth -= 1;
            return Ok(Json::Array(items));
        }
        loop {
            let value = self.parse_value()?;
            items.push(value);
            self.skip_whitespace();
            match self.peek() {
                Some(b',') => self.pos += 1,
                Some(b']') => {
                    self.pos += 1;
                    self.depth -= 1;
                    return Ok(Json::Array(items));
                }
                _ => return Err(self.invalid()),
            }
        }
    }

    fn parse_string(&mut self) -> Result<String, ServerError> {
        self.pos += 1;
        let mut text = String::new();
        loop {
            let byte = self.peek().ok_or_else(|| self.invalid())?;
            match byte {
                b'"' => {
                    self.pos += 1;
                    return Ok(text);
                }
                b'\\' => {
                    self.pos += 1;
                    let escape = self.peek().ok_or_else(|| self.invalid())?;
                    self.pos += 1;
                    match escape {
                        b'"' => text.push('"'),
                        b'\\' => text.push('\\'),
                        b'/' => text.push('/'),
                        b'b' => text.push('\u{0008}'),
                        b'f' => text.push('\u{000c}'),
                        b'n' => text.push('\n'),
                        b'r' => text.push('\r'),
                        b't' => text.push('\t'),
                        b'u' => {
                            let high = self.parse_hex4()?;
                            let character = if (0xd800..0xdc00).contains(&high) {
                                // A high surrogate must pair with a low one.
                                if self.peek() != Some(b'\\')
                                    || self.input.as_bytes().get(self.pos + 1) != Some(&b'u')
                                {
                                    return Err(self.invalid());
                                }
                                self.pos += 2;
                                let low = self.parse_hex4()?;
                                if !(0xdc00..0xe000).contains(&low) {
                                    return Err(self.invalid());
                                }
                                let combined = 0x1_0000 + ((high - 0xd800) << 10) + (low - 0xdc00);
                                char::from_u32(combined).ok_or_else(|| self.invalid())?
                            } else if (0xdc00..0xe000).contains(&high) {
                                return Err(self.invalid());
                            } else {
                                char::from_u32(high).ok_or_else(|| self.invalid())?
                            };
                            text.push(character);
                        }
                        _ => return Err(self.invalid()),
                    }
                }
                byte if byte < 0x20 => return Err(self.invalid()),
                _ => {
                    let character = self.input[self.pos..]
                        .chars()
                        .next()
                        .ok_or_else(|| self.invalid())?;
                    text.push(character);
                    self.pos += character.len_utf8();
                }
            }
        }
    }

    fn parse_hex4(&mut self) -> Result<u32, ServerError> {
        let slice = self
            .input
            .get(self.pos..self.pos + 4)
            .ok_or_else(|| self.invalid())?;
        let value = u32::from_str_radix(slice, 16).map_err(|_| self.invalid())?;
        self.pos += 4;
        Ok(value)
    }

    fn parse_number(&mut self) -> Result<Json, ServerError> {
        let start = self.pos;
        if self.peek() == Some(b'-') {
            self.pos += 1;
        }
        let digits_start = self.pos;
        while matches!(self.peek(), Some(byte) if byte.is_ascii_digit()) {
            self.pos += 1;
        }
        let digit_count = self.pos - digits_start;
        if digit_count == 0 {
            return Err(self.invalid());
        }
        if digit_count > 1 && self.input.as_bytes()[digits_start] == b'0' {
            return Err(self.invalid());
        }
        // The HTTP v1 contract has no floating-point fields, so a fractional
        // or exponent form is refused instead of rounded.
        if matches!(self.peek(), Some(b'.') | Some(b'e') | Some(b'E')) {
            return Err(self.invalid());
        }
        let digits = &self.input[digits_start..self.pos];
        if self.input.as_bytes()[start] == b'-' {
            let magnitude: u64 = digits.parse().map_err(|_| self.invalid())?;
            if magnitude > i64::MAX as u64 {
                return Err(self.invalid());
            }
            Ok(Json::I64(-(magnitude as i64)))
        } else {
            let value: u64 = digits.parse().map_err(|_| self.invalid())?;
            Ok(Json::U64(value))
        }
    }
}

// ---------------------------------------------------------------------------
// Canonical identifiers and text rules
// ---------------------------------------------------------------------------

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

/// Parses the canonical lowercase UUIDv4 text form into bytes.
pub fn parse_canonical_uuid(text: &str) -> Option<[u8; 16]> {
    if text.len() != 36 {
        return None;
    }
    let hex: String = text.chars().filter(|character| *character != '-').collect();
    if hex.len() != 32 {
        return None;
    }
    let raw: [u8; 16] = (0..16)
        .map(|index| u8::from_str_radix(&hex[index * 2..index * 2 + 2], 16).ok())
        .collect::<Option<Vec<u8>>>()?
        .try_into()
        .ok()?;
    // Round-trip through the canonical text enforces the lowercase v4 layout
    // the schema pattern pins.
    if text != uuid_text(raw) {
        return None;
    }
    Some(raw)
}

fn invalid_schema() -> ServerError {
    ServerError::InvalidInput {
        field: "agent_schema",
    }
}

fn unavailable() -> ServerError {
    ServerError::Agent {
        code: AgentErrorCode::AgentUnavailable,
        status: AgentErrorCode::AgentUnavailable.status(),
    }
}

/// The Go `validAgentText` rule: byte-bounded, no NUL, no Unicode controls.
fn valid_text(value: &str, maximum: usize, required: bool) -> bool {
    !(required && value.is_empty())
        && value.len() <= maximum
        && !value.contains('\u{0000}')
        && !value.chars().any(char::is_control)
}

/// The Go `validAgentNonBlankText` rule.
fn valid_non_blank_text(value: &str, maximum: usize) -> bool {
    valid_text(value, maximum, true) && !value.trim().is_empty()
}

/// The Go `validAgentMemoryText` rule: byte-bounded, no NUL, controls allowed.
fn valid_memory_text(value: &str, maximum: usize) -> bool {
    value.len() <= maximum && !value.contains('\u{0000}')
}

/// The Go `validDialogueLine` rule: bounded text with no edge whitespace.
fn valid_dialogue_line(value: &str) -> bool {
    valid_text(value, 256, true)
        && value
            .chars()
            .next()
            .is_some_and(|first| !first.is_whitespace())
        && value
            .chars()
            .next_back()
            .is_some_and(|last| !last.is_whitespace())
}

fn valid_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

/// The Go `validMCPEndpoint` rule: `http://loopback-literal:port/mcp` with a
/// required port and no userinfo, query, or fragment.
fn valid_mcp_endpoint(value: &str) -> bool {
    valid_text(value, 256, true) && parse_http_loopback_url(value, true).is_some()
}

/// Parses `http://<loopback-ip-literal>[:port][/path]` into a socket address.
/// With `require_mcp_path` (the plan-request MCP endpoint field) the exact
/// `/mcp` path and an explicit nonzero port are required, mirroring the Go
/// `validMCPEndpoint` rejection of `parsed.Port() == ""`; without it (the
/// service endpoint) any or no path is allowed and a missing port defaults to
/// 80, mirroring `AgentServiceSettings.Validate`.
fn parse_http_loopback_url(value: &str, require_mcp_path: bool) -> Option<SocketAddr> {
    let rest = value.strip_prefix("http://")?;
    if value.contains('?') || value.contains('#') {
        return None;
    }
    let (authority, path) = match rest.split_once('/') {
        Some((authority, path)) => (authority, format!("/{path}")),
        None => (rest, String::new()),
    };
    if authority.contains('@') {
        return None;
    }
    let (host, port) = split_host_port(authority)?;
    if require_mcp_path {
        if path != "/mcp" {
            return None;
        }
        // The MCP gate has no port default: the Go contract refuses a
        // portless MCP endpoint before send.
        port?;
    }
    let ip: IpAddr = if host.starts_with('[') {
        host.strip_prefix('[')?.strip_suffix(']')?.parse().ok()?
    } else {
        host.parse().ok()?
    };
    if !ip.is_loopback() {
        return None;
    }
    Some(SocketAddr::new(ip, port.unwrap_or(80)))
}

/// Splits the authority into host and explicit port. A missing port is
/// reported as `None` so each caller applies its own default; an explicit
/// port must be nonzero in both endpoint contexts.
fn split_host_port(authority: &str) -> Option<(&str, Option<u16>)> {
    if let Some(rest) = authority.strip_prefix('[') {
        let (host, tail) = rest.split_once(']')?;
        return match tail.strip_prefix(':') {
            Some(port) => {
                let port: u16 = port.parse().ok()?;
                if port == 0 {
                    return None;
                }
                Some((host, Some(port)))
            }
            None => Some((host, None)),
        };
    }
    match authority.rsplit_once(':') {
        Some((host, port)) => {
            let port: u16 = port.parse().ok()?;
            if port == 0 {
                return None;
            }
            Some((host, Some(port)))
        }
        None => Some((authority, None)),
    }
}

// ---------------------------------------------------------------------------
// Endpoint and wire
// ---------------------------------------------------------------------------

/// A validated Agent service endpoint: an http loopback IP literal with an
/// explicit or defaulted port. Hostnames never construct a wire, so the
/// transport cannot resolve names, and no proxy surface exists.
#[derive(Clone, Copy, Debug)]
pub struct AgentEndpoint {
    addr: SocketAddr,
}

impl AgentEndpoint {
    pub fn parse(endpoint: &str) -> Result<Self, ServerError> {
        // The Go settings trim trailing slashes before validation.
        let trimmed = endpoint.trim_end_matches('/');
        parse_http_loopback_url(trimmed, false)
            .map(|addr| Self { addr })
            .ok_or(ServerError::InvalidInput {
                field: "agent_endpoint",
            })
    }

    pub fn socket_address(&self) -> SocketAddr {
        self.addr
    }

    fn host_header(&self) -> String {
        match self.addr {
            SocketAddr::V4(address) => address.to_string(),
            SocketAddr::V6(address) => format!("[{}]:{}", address.ip(), address.port()),
        }
    }
}

/// The loopback HTTP client wire. One short-lived connection per RPC with
/// `Connection: close`; failures surface as typed errors instead of a second
/// attempt, so no business request is ever retried automatically.
pub struct AgentHttpWire {
    endpoint: AgentEndpoint,
    credential: String,
    clock: Arc<dyn Clock + Send + Sync>,
    closed: Arc<AtomicBool>,
}

impl AgentHttpWire {
    pub fn try_new(
        endpoint: &str,
        credential: &str,
        clock: Arc<dyn Clock + Send + Sync>,
    ) -> Result<Self, ServerError> {
        if credential.is_empty() {
            return Err(ServerError::InvalidInput {
                field: "agent_credential",
            });
        }
        Ok(Self {
            endpoint: AgentEndpoint::parse(endpoint)?,
            credential: credential.to_owned(),
            clock,
            closed: Arc::new(AtomicBool::new(false)),
        })
    }

    /// Builds the request head with the frozen header set and refuses heads
    /// over the header ceiling.
    fn request_head(&self, route: &str, body_len: usize) -> Result<String, ServerError> {
        let content_length = body_len.to_string();
        let headers = [
            ("Host", self.endpoint.host_header()),
            ("Authorization", format!("Bearer {}", self.credential)),
            ("Content-Type", "application/json".to_owned()),
            ("Accept-Encoding", "identity".to_owned()),
            ("User-Agent", USER_AGENT.to_owned()),
            ("Connection", "close".to_owned()),
            ("Content-Length", content_length),
        ];
        let counted: usize = headers
            .iter()
            .map(|(name, value)| name.len() + value.len() + 4)
            .sum();
        if counted > MAX_HEADER_BYTES {
            return Err(unavailable());
        }
        let mut head = format!("POST {route} HTTP/1.1\r\n");
        for (name, value) in &headers {
            head.push_str(name);
            head.push_str(": ");
            head.push_str(value);
            head.push_str("\r\n");
        }
        head.push_str("\r\n");
        Ok(head)
    }

    /// Remaining real time until the deadline, measured on this wire's clock.
    fn timeout_until(&self, deadline: Deadline) -> Result<Duration, ServerError> {
        deadline
            .instant()
            .checked_duration_since(self.clock.monotonic())
            .ok_or(ServerError::Timeout {
                operation: Operation::AgentRpc,
            })
    }
}

impl AgentWire for AgentHttpWire {
    fn rpc(&self, request: AgentRequest, deadline: Deadline) -> Result<AgentResponse, ServerError> {
        if self.closed.load(Ordering::SeqCst) {
            return Err(unavailable());
        }
        let route = route_of(&request)?;
        let body = request_body(&request)?;
        let head = self.request_head(route, body.len())?;
        let timeout = self.timeout_until(deadline)?;
        let mut stream = TcpStream::connect_timeout(&self.endpoint.socket_address(), timeout)
            .map_err(|_| unavailable())?;
        stream
            .set_read_timeout(Some(timeout))
            .map_err(|_| unavailable())?;
        stream
            .set_write_timeout(Some(timeout))
            .map_err(|_| unavailable())?;
        let mut outbound = head.into_bytes();
        outbound.extend_from_slice(&body);
        stream.write_all(&outbound).map_err(|_| unavailable())?;
        let response = read_http_response(&mut stream)?;
        let status = response.status;
        // A 3xx never redirects: the boundary forbids following, so a
        // redirect status is a wire failure.
        if (300..400).contains(&status) {
            return Err(unavailable());
        }
        // Exactly one Content-Type value, exactly application/json.
        let content_types: Vec<&str> = response
            .headers
            .iter()
            .filter(|(name, _)| name == "content-type")
            .map(|(_, value)| value.as_str())
            .collect();
        if content_types.len() != 1 || content_types[0] != "application/json" {
            return Err(unavailable());
        }
        if response
            .declared_content_length
            .is_some_and(|declared| declared > MAX_RESPONSE_BODY_BYTES)
        {
            return Err(plan_success_body_failure(route, status));
        }
        if response.body.len() > MAX_RESPONSE_BODY_BYTES {
            return Err(plan_success_body_failure(route, status));
        }
        if status == 200 {
            return decode_success(&request, &response.body);
        }
        let expected = identity_request_id(&request);
        Err(error_failure(route, status, &expected, &response.body))
    }

    fn close(&self) {
        self.closed.store(true, Ordering::SeqCst);
    }
}

struct RawResponse {
    status: u16,
    headers: Vec<(String, String)>,
    declared_content_length: Option<usize>,
    body: Vec<u8>,
}

/// Reads one HTTP/1.1 response: the head bounded well past the header
/// ceiling, then the body capped one byte past the response ceiling.
fn read_http_response(stream: &mut TcpStream) -> Result<RawResponse, ServerError> {
    let mut head = Vec::new();
    let mut buffer = [0u8; 1024];
    let head_end = loop {
        if let Some(position) = find_head_end(&head) {
            break position;
        }
        if head.len() > MAX_HEADER_BYTES * 4 {
            return Err(unavailable());
        }
        let read = stream.read(&mut buffer).map_err(|_| unavailable())?;
        if read == 0 {
            return Err(unavailable());
        }
        head.extend_from_slice(&buffer[..read]);
    };
    let head_text = std::str::from_utf8(&head[..head_end]).map_err(|_| unavailable())?;
    let mut lines = head_text.split("\r\n");
    let status_line = lines.next().ok_or_else(unavailable)?;
    let status = status_line
        .split_whitespace()
        .nth(1)
        .and_then(|code| code.parse::<u16>().ok())
        .ok_or_else(unavailable)?;
    let mut headers = Vec::new();
    let mut declared_content_length = None;
    let mut counted = 0usize;
    for line in lines {
        if line.is_empty() {
            continue;
        }
        let Some((name, value)) = line.split_once(':') else {
            return Err(unavailable());
        };
        // The Go client counts the parsed header map, so trim first.
        let name = name.trim().to_ascii_lowercase();
        let value = value.trim().to_owned();
        counted += name.len() + value.len() + 4;
        if name == "content-length" {
            declared_content_length = value.parse::<usize>().ok();
        }
        headers.push((name, value));
    }
    if counted > MAX_HEADER_BYTES {
        return Err(unavailable());
    }
    let mut body = head[head_end + 4..].to_vec();
    let cap = MAX_RESPONSE_BODY_BYTES + 1;
    while body.len() < cap {
        let read = stream.read(&mut buffer).map_err(|_| unavailable())?;
        if read == 0 {
            break;
        }
        body.extend_from_slice(&buffer[..read]);
    }
    body.truncate(cap);
    Ok(RawResponse {
        status,
        headers,
        declared_content_length,
        body,
    })
}

fn find_head_end(buffer: &[u8]) -> Option<usize> {
    buffer.windows(4).position(|window| window == b"\r\n\r\n")
}

fn identity_request_id(request: &AgentRequest) -> AgentRequestId {
    match request {
        AgentRequest::Acquire(base) => base.request_id,
        AgentRequest::Heartbeat(leased)
        | AgentRequest::Release(leased)
        | AgentRequest::Plan(PlanRequest { leased, .. })
        | AgentRequest::Cancel(CancelRequest { leased, .. })
        | AgentRequest::Dialogue(DialogueRequest { leased, .. })
        | AgentRequest::Reconcile(ReconcileRequest::Active { leased, .. })
        | AgentRequest::Reconcile(ReconcileRequest::Inactive { leased, .. })
        | AgentRequest::Commit(CommitRequest { leased, .. })
        | AgentRequest::Delete(DeleteRequest { leased, .. }) => leased.base.request_id,
    }
}

// ---------------------------------------------------------------------------
// Route table and request encoding
// ---------------------------------------------------------------------------

/// The POST route of each request kind, mirroring the manifest route table.
pub fn route_of(request: &AgentRequest) -> Result<&'static str, ServerError> {
    Ok(match request {
        AgentRequest::Acquire(_) => "/v1/namespaces/acquire",
        AgentRequest::Heartbeat(_) => "/v1/namespaces/heartbeat",
        AgentRequest::Release(_) => "/v1/namespaces/release",
        AgentRequest::Plan(_) => "/v1/plan",
        AgentRequest::Cancel(_) => "/v1/runs/cancel",
        AgentRequest::Dialogue(_) => "/v1/dialogue",
        AgentRequest::Reconcile(_) => "/v1/memory/reconcile",
        AgentRequest::Commit(_) => "/v1/memory/commit",
        AgentRequest::Delete(_) => "/v1/memory/delete",
    })
}

/// Inclusive request body budget check, mirroring the Go boundary helper.
pub fn request_body_within_limit(body: &[u8]) -> bool {
    body.len() <= MAX_REQUEST_BODY_BYTES
}

/// Inclusive response body budget check.
pub fn response_body_within_limit(body: &[u8]) -> bool {
    body.len() <= MAX_RESPONSE_BODY_BYTES
}

/// Encodes the request JSON document with the schema key order and the
/// pre-send text rules of the Go `validAgentRequest` gate.
pub fn request_json(request: &AgentRequest) -> Result<Json, ServerError> {
    fn base_json(base: &BaseIdentity) -> Json {
        Json::Object(vec![
            (
                "contract_version".to_owned(),
                Json::Str(AGENT_CONTRACT_VERSION.to_owned()),
            ),
            (
                "request_id".to_owned(),
                Json::Str(uuid_text(base.request_id.bytes())),
            ),
            (
                "client_instance_id".to_owned(),
                Json::Str(uuid_text(base.client_instance_id.bytes())),
            ),
            (
                "namespace_id".to_owned(),
                Json::Str(uuid_text(base.namespace_id.bytes())),
            ),
        ])
    }
    fn leased_json(leased: &LeasedIdentity) -> Result<Json, ServerError> {
        let mut value = base_json(&leased.base);
        value.extend_object(&[("lease_id", Json::Str(uuid_text(leased.lease_id.bytes())))]);
        Ok(value)
    }
    let value = match request {
        AgentRequest::Acquire(base) => base_json(base),
        AgentRequest::Heartbeat(leased) | AgentRequest::Release(leased) => leased_json(leased)?,
        AgentRequest::Plan(request) => {
            if !valid_mcp_endpoint(&request.mcp_endpoint)
                || !valid_text(&request.capability, 512, true)
                || !valid_non_blank_text(request.instruction.as_str(), 1024)
            {
                return Err(ServerError::InvalidInput {
                    field: "plan_request",
                });
            }
            let mut value = leased_json(&request.leased)?;
            value.extend_object(&[
                ("run_id", Json::Str(uuid_text(request.run_id.bytes()))),
                (
                    "companion_id",
                    Json::Str(uuid_text(request.companion_id.bytes())),
                ),
                ("generation", Json::U64(request.generation)),
                (
                    "snapshot_id",
                    Json::Str(uuid_text(request.snapshot_id.bytes())),
                ),
                (
                    "snapshot_digest",
                    Json::Str(hex_lower(&request.snapshot_digest)),
                ),
                ("deadline_unix_ms", Json::I64(request.deadline_unix_ms)),
                ("mcp_endpoint", Json::Str(request.mcp_endpoint.clone())),
                ("mcp_capability", Json::Str(request.capability.clone())),
                (
                    "instruction",
                    Json::Str(request.instruction.as_str().to_owned()),
                ),
            ]);
            value
        }
        AgentRequest::Cancel(request) => {
            let mut value = leased_json(&request.leased)?;
            value.extend_object(&[("run_id", Json::Str(uuid_text(request.run_id.bytes())))]);
            value
        }
        AgentRequest::Dialogue(request) => {
            if !valid_memory_text(&request.persona, 4096) {
                return Err(ServerError::InvalidInput {
                    field: "dialogue_request",
                });
            }
            let mut value = leased_json(&request.leased)?;
            value.extend_object(&[
                ("run_id", Json::Str(uuid_text(request.run_id.bytes()))),
                (
                    "companion_id",
                    Json::Str(uuid_text(request.companion_id.bytes())),
                ),
                ("generation", Json::U64(request.generation)),
                ("memory_epoch", Json::U64(request.memory_epoch)),
                ("deadline_unix_ms", Json::I64(request.deadline_unix_ms)),
                ("persona", Json::Str(request.persona.clone())),
                ("fact_node", fact_node_json(&request.fact_node)),
                ("environment", environment_json(&request.environment)),
                ("terminal", Json::Bool(request.terminal)),
            ]);
            value
        }
        AgentRequest::Reconcile(request) => {
            let (leased, companion_id, memory_epoch) = match request {
                ReconcileRequest::Active {
                    leased,
                    companion_id,
                    memory_epoch,
                    ..
                }
                | ReconcileRequest::Inactive {
                    leased,
                    companion_id,
                    memory_epoch,
                    ..
                } => (leased, companion_id, memory_epoch),
            };
            let mut value = leased_json(leased)?;
            let (active, tombstone, mirror) = match request {
                ReconcileRequest::Active { mirror, .. } => {
                    (Json::Bool(true), Json::Null, memory_state_json(mirror))
                }
                ReconcileRequest::Inactive {
                    tombstone_operation_id,
                    ..
                } => (
                    Json::Bool(false),
                    Json::Str(uuid_text(tombstone_operation_id.bytes())),
                    Json::Null,
                ),
            };
            value.extend_object(&[
                ("companion_id", Json::Str(uuid_text(companion_id.bytes()))),
                ("memory_epoch", Json::U64(*memory_epoch)),
                ("active", active),
                ("tombstone_operation_id", tombstone),
                ("mirror", mirror),
            ]);
            value
        }
        AgentRequest::Commit(request) => {
            if !valid_memory_text(&request.summary, 2048) {
                return Err(ServerError::InvalidInput {
                    field: "commit_request",
                });
            }
            let mut value = leased_json(&request.leased)?;
            value.extend_object(&[
                (
                    "companion_id",
                    Json::Str(uuid_text(request.companion_id.bytes())),
                ),
                ("memory_epoch", Json::U64(request.memory_epoch)),
                ("base_revision", Json::U64(request.base_revision)),
                (
                    "operation_id",
                    Json::Str(uuid_text(request.operation_id.bytes())),
                ),
                ("summary", Json::Str(request.summary.clone())),
            ]);
            value
        }
        AgentRequest::Delete(request) => {
            let mut value = leased_json(&request.leased)?;
            value.extend_object(&[
                (
                    "companion_id",
                    Json::Str(uuid_text(request.companion_id.bytes())),
                ),
                ("old_memory_epoch", Json::U64(request.old_memory_epoch)),
                ("new_memory_epoch", Json::U64(request.new_memory_epoch)),
                (
                    "tombstone_operation_id",
                    Json::Str(uuid_text(request.tombstone_operation_id.bytes())),
                ),
            ]);
            value
        }
    };
    Ok(value)
}

/// Encodes and applies the inclusive outbound body ceiling.
pub fn request_body(request: &AgentRequest) -> Result<Vec<u8>, ServerError> {
    let bytes = request_json(request)?.write();
    if !request_body_within_limit(&bytes) {
        return Err(unavailable());
    }
    Ok(bytes)
}

fn hex_lower(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn object(entries: &[(&str, Json)]) -> Json {
    Json::Object(
        entries
            .iter()
            .map(|(key, value)| ((*key).to_owned(), value.clone()))
            .collect(),
    )
}

fn fact_node_json(fact: &DialogueFact) -> Json {
    match fact {
        DialogueFact::Start => object(&[("kind", Json::Str("start".to_owned()))]),
        DialogueFact::FirstArrival => object(&[("kind", Json::Str("first_arrival".to_owned()))]),
        DialogueFact::Idle => object(&[("kind", Json::Str("idle".to_owned()))]),
        DialogueFact::Progress(step) => object(&[
            ("kind", Json::Str("progress".to_owned())),
            (
                "step_kind",
                Json::Str(
                    match step {
                        DialogueProgress::GoTo => "go_to",
                        DialogueProgress::Mine => "mine",
                        DialogueProgress::Place => "place",
                    }
                    .to_owned(),
                ),
            ),
        ]),
        DialogueFact::Terminal { failed, reason } => {
            let state = if *failed { "failed" } else { "completed" };
            object(&[
                ("kind", Json::Str("terminal".to_owned())),
                ("state", Json::Str(state.to_owned())),
                ("reason", Json::Str(dialogue_failure_text(*reason))),
            ])
        }
    }
}

fn dialogue_failure_text(reason: DialogueFailure) -> String {
    match reason {
        DialogueFailure::None => "none".to_owned(),
        DialogueFailure::PlannerUnavailable => "planner_unavailable".to_owned(),
        DialogueFailure::InvalidPlan => "invalid_plan".to_owned(),
        DialogueFailure::PathUnreachable => "path_unreachable".to_owned(),
        DialogueFailure::WorldChanged => "world_changed".to_owned(),
        DialogueFailure::InventoryFull => "inventory_full".to_owned(),
    }
}

fn environment_json(environment: &DialogueEnvironment) -> Json {
    let blocks = Json::Array(
        environment
            .exposed_blocks
            .iter()
            .map(|(position, block_id)| {
                object(&[
                    (
                        "position",
                        object(&[
                            ("x", Json::I64(position.x() as i64)),
                            ("y", Json::I64(position.y() as i64)),
                            ("z", Json::I64(position.z() as i64)),
                        ]),
                    ),
                    ("block_id", Json::U64(*block_id as u64)),
                ])
            })
            .collect(),
    );
    let heights = Json::Array(
        environment
            .heights
            .iter()
            .map(|(x, z, height)| {
                object(&[
                    ("x", Json::I64(*x as i64)),
                    ("z", Json::I64(*z as i64)),
                    ("height", Json::I64(*height as i64)),
                ])
            })
            .collect(),
    );
    object(&[("exposed_blocks", blocks), ("heights", heights)])
}

fn memory_state_json(state: &MemoryState) -> Json {
    match state {
        MemoryState::Absent => object(&[
            ("revision", Json::U64(0)),
            ("operation_id", Json::Null),
            ("summary", Json::Str(String::new())),
        ]),
        MemoryState::Present {
            revision,
            operation_id,
            summary,
        } => object(&[
            ("revision", Json::U64(revision.get())),
            ("operation_id", Json::Str(uuid_text(operation_id.bytes()))),
            ("summary", Json::Str(summary.clone())),
        ]),
    }
}

// ---------------------------------------------------------------------------
// Closed-schema decode
// ---------------------------------------------------------------------------

/// Checks the exact closed key set and returns the fields in schema order.
fn object_keys<'a>(
    value: &'a Json,
    expected: &[&str],
) -> Result<Vec<(&'a str, &'a Json)>, ServerError> {
    let Json::Object(entries) = value else {
        return Err(invalid_schema());
    };
    if entries.len() != expected.len()
        || entries
            .iter()
            .any(|(key, _)| !expected.contains(&key.as_str()))
    {
        return Err(invalid_schema());
    }
    expected
        .iter()
        .map(|name| {
            entries
                .iter()
                .find(|(key, _)| key == name)
                .map(|(key, value)| (key.as_str(), value))
                .ok_or_else(invalid_schema)
        })
        .collect()
}

fn field<'a>(fields: &[(&'a str, &'a Json)], name: &str) -> Result<&'a Json, ServerError> {
    fields
        .iter()
        .find(|(key, _)| *key == name)
        .map(|(_, value)| *value)
        .ok_or_else(invalid_schema)
}

fn uuid_value(value: &Json) -> Result<[u8; 16], ServerError> {
    parse_canonical_uuid(value.as_str()).ok_or_else(invalid_schema)
}

fn canonical(entries: Vec<(&str, Json)>) -> Json {
    Json::Object(
        entries
            .into_iter()
            .map(|(key, value)| (key.to_owned(), value))
            .collect(),
    )
}

fn positive_u64(value: &Json) -> Result<u64, ServerError> {
    match value {
        Json::U64(value) if *value > 0 => Ok(*value),
        _ => Err(invalid_schema()),
    }
}

/// Decodes one schema value with the closed-schema rules and returns the
/// canonical form: keys in schema order with every leaf validated. Unknown
/// keys, missing keys, and value-rule violations are refused.
pub fn schema_decode(schema: &str, value: &Json) -> Result<Json, ServerError> {
    const BASE: &[&str] = &[
        "contract_version",
        "request_id",
        "client_instance_id",
        "namespace_id",
    ];
    const BASE_WITH_LEASE: &[&str] = &[
        "contract_version",
        "request_id",
        "client_instance_id",
        "namespace_id",
        "lease_id",
    ];
    let result = match schema {
        "live_response" => {
            let fields = object_keys(value, &["status"])?;
            if field(&fields, "status")?.as_str() != "live" {
                return Err(invalid_schema());
            }
            canonical(vec![("status", field(&fields, "status")?.clone())])
        }
        "ready_response" => {
            let fields = object_keys(value, &["status"])?;
            let status = field(&fields, "status")?.as_str();
            if status != "ready" && status != "not_ready" {
                return Err(invalid_schema());
            }
            canonical(vec![("status", field(&fields, "status")?.clone())])
        }
        "acquire_request" => {
            let fields = object_keys(value, BASE)?;
            decode_base_ref(&fields)?
        }
        "lease_request" => {
            let fields = object_keys(value, BASE_WITH_LEASE)?;
            decode_leased_with(&fields, "lease_id")?
        }
        "cancel_request" => {
            let fields = object_keys(
                value,
                &[
                    "contract_version",
                    "request_id",
                    "client_instance_id",
                    "namespace_id",
                    "lease_id",
                    "run_id",
                ],
            )?;
            let mut decoded = decode_leased_with(&fields, "lease_id")?;
            uuid_value(field(&fields, "run_id")?)?;
            decoded.extend_object(&[("run_id", field(&fields, "run_id")?.clone())]);
            decoded
        }
        "acquire_response" | "heartbeat_response" => {
            let fields = object_keys(
                value,
                &[
                    "contract_version",
                    "request_id",
                    "client_instance_id",
                    "namespace_id",
                    "lease_id",
                    "lease_expires_in_ms",
                ],
            )?;
            let mut decoded = decode_base_ref(&fields)?;
            uuid_value(field(&fields, "lease_id")?)?;
            if field(&fields, "lease_expires_in_ms")?.as_u64() != Some(LEASE_EXPIRES_IN_MS) {
                return Err(invalid_schema());
            }
            decoded.extend_object(&[
                ("lease_id", field(&fields, "lease_id")?.clone()),
                (
                    "lease_expires_in_ms",
                    field(&fields, "lease_expires_in_ms")?.clone(),
                ),
            ]);
            decoded
        }
        "release_response" => {
            let fields = object_keys(
                value,
                &[
                    "contract_version",
                    "request_id",
                    "client_instance_id",
                    "namespace_id",
                    "lease_id",
                    "released",
                ],
            )?;
            let mut decoded = decode_base_ref(&fields)?;
            uuid_value(field(&fields, "lease_id")?)?;
            if field(&fields, "released")? != &Json::Bool(true) {
                return Err(invalid_schema());
            }
            decoded.extend_object(&[
                ("lease_id", field(&fields, "lease_id")?.clone()),
                ("released", Json::Bool(true)),
            ]);
            decoded
        }
        "plan_request" => {
            let fields = object_keys(
                value,
                &[
                    "contract_version",
                    "request_id",
                    "client_instance_id",
                    "namespace_id",
                    "lease_id",
                    "run_id",
                    "companion_id",
                    "generation",
                    "snapshot_id",
                    "snapshot_digest",
                    "deadline_unix_ms",
                    "mcp_endpoint",
                    "mcp_capability",
                    "instruction",
                ],
            )?;
            let mut decoded = decode_plan_identity(&fields)?;
            match field(&fields, "deadline_unix_ms")? {
                Json::I64(value) if *value >= 1 => {}
                Json::U64(value) if *value >= 1 && *value <= i64::MAX as u64 => {}
                _ => return Err(invalid_schema()),
            }
            if !valid_mcp_endpoint(field(&fields, "mcp_endpoint")?.as_str())
                || !valid_text(field(&fields, "mcp_capability")?.as_str(), 512, true)
                || !valid_non_blank_text(field(&fields, "instruction")?.as_str(), 1024)
            {
                return Err(invalid_schema());
            }
            for name in [
                "deadline_unix_ms",
                "mcp_endpoint",
                "mcp_capability",
                "instruction",
            ] {
                decoded.extend_object(&[(name, field(&fields, name)?.clone())]);
            }
            decoded
        }
        "plan_response" => {
            let fields = object_keys(
                value,
                &[
                    "contract_version",
                    "request_id",
                    "client_instance_id",
                    "namespace_id",
                    "lease_id",
                    "run_id",
                    "companion_id",
                    "generation",
                    "snapshot_id",
                    "snapshot_digest",
                    "plan",
                ],
            )?;
            let mut decoded = decode_plan_identity(&fields)?;
            let plan = decode_plan(field(&fields, "plan")?)?;
            decoded.extend_object(&[("plan", plan)]);
            decoded
        }
        "plan_summary" => {
            if !valid_non_blank_text(value.as_str(), 512) {
                return Err(invalid_schema());
            }
            value.clone()
        }
        "dialogue_nonterminal_fact_node" => decode_nonterminal_fact(value)?,
        "dialogue_terminal_fact_node"
        | "dialogue_terminal_nonfailed_fact_node"
        | "dialogue_terminal_failed_fact_node" => decode_terminal_fact(value)?,
        "dialogue_fact_node" => {
            decode_nonterminal_fact(value).or_else(|_| decode_terminal_fact(value))?
        }
        "dialogue_request" => decode_dialogue_request(value)?,
        "dialogue_response" => decode_dialogue_response(value)?,
        "dialogue_line" => {
            if !valid_dialogue_line(value.as_str()) {
                return Err(invalid_schema());
            }
            value.clone()
        }
        "persona_text" => {
            if !valid_memory_text(value.as_str(), 4096) {
                return Err(invalid_schema());
            }
            value.clone()
        }
        "memory_summary" => {
            if !valid_memory_text(value.as_str(), 2048) {
                return Err(invalid_schema());
            }
            value.clone()
        }
        "mcp_endpoint" => {
            if !valid_mcp_endpoint(value.as_str()) {
                return Err(invalid_schema());
            }
            value.clone()
        }
        "instruction_text" => {
            if !valid_non_blank_text(value.as_str(), 1024) {
                return Err(invalid_schema());
            }
            value.clone()
        }
        "mcp_capability" => {
            if !valid_text(value.as_str(), 512, true) {
                return Err(invalid_schema());
            }
            value.clone()
        }
        "memory_state" | "memory_state_zero" | "memory_state_nonzero" => {
            decode_memory_state(value)?
        }
        "memory_reconcile_request" => decode_reconcile(
            value,
            "mirror",
            &[
                "contract_version",
                "request_id",
                "client_instance_id",
                "namespace_id",
                "lease_id",
                "companion_id",
                "memory_epoch",
                "active",
                "tombstone_operation_id",
                "mirror",
            ],
        )?,
        "memory_reconcile_response" => decode_reconcile(
            value,
            "memory",
            &[
                "contract_version",
                "request_id",
                "client_instance_id",
                "namespace_id",
                "lease_id",
                "companion_id",
                "memory_epoch",
                "active",
                "tombstone_operation_id",
                "memory",
            ],
        )?,
        "memory_commit_request" => {
            let fields = object_keys(
                value,
                &[
                    "contract_version",
                    "request_id",
                    "client_instance_id",
                    "namespace_id",
                    "lease_id",
                    "companion_id",
                    "memory_epoch",
                    "base_revision",
                    "operation_id",
                    "summary",
                ],
            )?;
            let mut decoded = decode_leased_companion(&fields)?;
            field(&fields, "base_revision")?
                .as_u64()
                .ok_or_else(invalid_schema)?;
            uuid_value(field(&fields, "operation_id")?)?;
            if !valid_memory_text(field(&fields, "summary")?.as_str(), 2048) {
                return Err(invalid_schema());
            }
            for name in ["base_revision", "operation_id", "summary"] {
                decoded.extend_object(&[(name, field(&fields, name)?.clone())]);
            }
            decoded
        }
        "memory_commit_response" => {
            let fields = object_keys(
                value,
                &[
                    "contract_version",
                    "request_id",
                    "client_instance_id",
                    "namespace_id",
                    "lease_id",
                    "companion_id",
                    "memory_epoch",
                    "operation_id",
                    "committed_revision",
                ],
            )?;
            let mut decoded = decode_leased_companion(&fields)?;
            uuid_value(field(&fields, "operation_id")?)?;
            positive_u64(field(&fields, "committed_revision")?)?;
            for name in ["operation_id", "committed_revision"] {
                decoded.extend_object(&[(name, field(&fields, name)?.clone())]);
            }
            decoded
        }
        "memory_delete_request" => {
            let fields = object_keys(
                value,
                &[
                    "contract_version",
                    "request_id",
                    "client_instance_id",
                    "namespace_id",
                    "lease_id",
                    "companion_id",
                    "old_memory_epoch",
                    "new_memory_epoch",
                    "tombstone_operation_id",
                ],
            )?;
            let mut decoded = decode_leased_with(&fields, "lease_id")?;
            uuid_value(field(&fields, "companion_id")?)?;
            positive_u64(field(&fields, "old_memory_epoch")?)?;
            positive_u64(field(&fields, "new_memory_epoch")?)?;
            uuid_value(field(&fields, "tombstone_operation_id")?)?;
            for name in [
                "companion_id",
                "old_memory_epoch",
                "new_memory_epoch",
                "tombstone_operation_id",
            ] {
                decoded.extend_object(&[(name, field(&fields, name)?.clone())]);
            }
            decoded
        }
        "memory_delete_response" => {
            let fields = object_keys(
                value,
                &[
                    "contract_version",
                    "request_id",
                    "client_instance_id",
                    "namespace_id",
                    "lease_id",
                    "companion_id",
                    "memory_epoch",
                    "tombstone_operation_id",
                ],
            )?;
            let mut decoded = decode_leased_companion(&fields)?;
            uuid_value(field(&fields, "tombstone_operation_id")?)?;
            decoded.extend_object(&[(
                "tombstone_operation_id",
                field(&fields, "tombstone_operation_id")?.clone(),
            )]);
            decoded
        }
        "cancel_response" => {
            let fields = object_keys(
                value,
                &[
                    "contract_version",
                    "request_id",
                    "client_instance_id",
                    "namespace_id",
                    "lease_id",
                    "run_id",
                    "cancelled",
                ],
            )?;
            let mut decoded = decode_leased_with(&fields, "lease_id")?;
            uuid_value(field(&fields, "run_id")?)?;
            if !matches!(field(&fields, "cancelled")?, Json::Bool(_)) {
                return Err(invalid_schema());
            }
            decoded.extend_object(&[
                ("run_id", field(&fields, "run_id")?.clone()),
                ("cancelled", field(&fields, "cancelled")?.clone()),
            ]);
            decoded
        }
        "error_response" => {
            let fields = object_keys(value, &["contract_version", "request_id", "error"])?;
            if field(&fields, "contract_version")?.as_str() != AGENT_CONTRACT_VERSION {
                return Err(invalid_schema());
            }
            let request_id = field(&fields, "request_id")?;
            if request_id != &Json::Null {
                uuid_value(request_id)?;
            }
            let error = field(&fields, "error")?;
            let error_fields = object_keys(error, &["code"])?;
            if !stable_error_code(field(&error_fields, "code")?.as_str()) {
                return Err(invalid_schema());
            }
            canonical(vec![
                (
                    "contract_version",
                    field(&fields, "contract_version")?.clone(),
                ),
                ("request_id", request_id.clone()),
                (
                    "error",
                    canonical(vec![("code", field(&error_fields, "code")?.clone())]),
                ),
            ])
        }
        _ => return Err(invalid_schema()),
    };
    Ok(result)
}

fn decode_leased_with(fields: &[(&str, &Json)], lease_key: &str) -> Result<Json, ServerError> {
    let mut decoded = decode_base_ref(fields)?;
    uuid_value(field(fields, lease_key)?)?;
    decoded.extend_object(&[(lease_key, field(fields, lease_key)?.clone())]);
    Ok(decoded)
}

fn decode_base_ref(fields: &[(&str, &Json)]) -> Result<Json, ServerError> {
    if field(fields, "contract_version")?.as_str() != AGENT_CONTRACT_VERSION {
        return Err(invalid_schema());
    }
    for name in ["request_id", "client_instance_id", "namespace_id"] {
        uuid_value(field(fields, name)?)?;
    }
    Ok(canonical(vec![
        (
            "contract_version",
            field(fields, "contract_version")?.clone(),
        ),
        ("request_id", field(fields, "request_id")?.clone()),
        (
            "client_instance_id",
            field(fields, "client_instance_id")?.clone(),
        ),
        ("namespace_id", field(fields, "namespace_id")?.clone()),
    ]))
}

fn decode_leased_companion(fields: &[(&str, &Json)]) -> Result<Json, ServerError> {
    let mut decoded = decode_leased_with(fields, "lease_id")?;
    uuid_value(field(fields, "companion_id")?)?;
    positive_u64(field(fields, "memory_epoch")?)?;
    decoded.extend_object(&[
        ("companion_id", field(fields, "companion_id")?.clone()),
        ("memory_epoch", field(fields, "memory_epoch")?.clone()),
    ]);
    Ok(decoded)
}

fn decode_plan_identity(fields: &[(&str, &Json)]) -> Result<Json, ServerError> {
    let mut decoded = decode_leased_with(fields, "lease_id")?;
    for name in ["run_id", "companion_id", "snapshot_id"] {
        uuid_value(field(fields, name)?)?;
    }
    positive_u64(field(fields, "generation")?)?;
    if !valid_sha256(field(fields, "snapshot_digest")?.as_str()) {
        return Err(invalid_schema());
    }
    for name in [
        "run_id",
        "companion_id",
        "generation",
        "snapshot_id",
        "snapshot_digest",
    ] {
        decoded.extend_object(&[(name, field(fields, name)?.clone())]);
    }
    Ok(decoded)
}

fn decode_nonterminal_fact(value: &Json) -> Result<Json, ServerError> {
    let kind = value.get("kind").map(Json::as_str).unwrap_or_default();
    match kind {
        "start" | "first_arrival" | "idle" => {
            object_keys(value, &["kind"])?;
            Ok(value.clone())
        }
        "progress" => {
            object_keys(value, &["kind", "step_kind"])?;
            let step = value.get("step_kind").map(Json::as_str).unwrap_or_default();
            if step == "go_to" || step == "mine" || step == "place" {
                Ok(value.clone())
            } else {
                Err(invalid_schema())
            }
        }
        _ => Err(invalid_schema()),
    }
}

fn decode_terminal_fact(value: &Json) -> Result<Json, ServerError> {
    let fields = object_keys(value, &["kind", "state", "reason"])?;
    if field(&fields, "kind")?.as_str() != "terminal" {
        return Err(invalid_schema());
    }
    let state = field(&fields, "state")?.as_str();
    let reason = field(&fields, "reason")?.as_str();
    let valid = match state {
        "completed" | "timed_out" | "stopped" => reason == "none",
        "failed" => matches!(
            reason,
            "planner_unavailable"
                | "invalid_plan"
                | "path_unreachable"
                | "world_changed"
                | "inventory_full"
        ),
        _ => false,
    };
    if valid {
        Ok(value.clone())
    } else {
        Err(invalid_schema())
    }
}

const DIALOGUE_REQUEST_KEYS: &[&str] = &[
    "contract_version",
    "request_id",
    "client_instance_id",
    "namespace_id",
    "lease_id",
    "run_id",
    "companion_id",
    "generation",
    "memory_epoch",
    "deadline_unix_ms",
    "persona",
    "fact_node",
    "environment",
    "terminal",
];

fn decode_dialogue_request(value: &Json) -> Result<Json, ServerError> {
    let fields = object_keys(value, DIALOGUE_REQUEST_KEYS)?;
    let mut decoded = decode_leased_with(&fields, "lease_id")?;
    for name in ["run_id", "companion_id"] {
        uuid_value(field(&fields, name)?)?;
    }
    positive_u64(field(&fields, "generation")?)?;
    positive_u64(field(&fields, "memory_epoch")?)?;
    match field(&fields, "deadline_unix_ms")? {
        Json::I64(value) if *value >= 1 => {}
        Json::U64(value) if *value >= 1 && *value <= i64::MAX as u64 => {}
        _ => return Err(invalid_schema()),
    }
    if !valid_memory_text(field(&fields, "persona")?.as_str(), 4096) {
        return Err(invalid_schema());
    }
    let terminal = match field(&fields, "terminal")? {
        Json::Bool(flag) => *flag,
        _ => return Err(invalid_schema()),
    };
    decode_environment(field(&fields, "environment")?)?;
    let fact = field(&fields, "fact_node")?;
    if terminal {
        decode_terminal_fact(fact)?;
    } else {
        decode_nonterminal_fact(fact)?;
    }
    for name in [
        "run_id",
        "companion_id",
        "generation",
        "memory_epoch",
        "deadline_unix_ms",
        "persona",
        "fact_node",
        "environment",
        "terminal",
    ] {
        decoded.extend_object(&[(name, field(&fields, name)?.clone())]);
    }
    Ok(decoded)
}

fn decode_dialogue_response(value: &Json) -> Result<Json, ServerError> {
    let proposal = value.get("memory_proposal");
    let keys: &[&str] = if proposal.is_some() {
        &[
            "contract_version",
            "request_id",
            "client_instance_id",
            "namespace_id",
            "lease_id",
            "run_id",
            "companion_id",
            "generation",
            "memory_epoch",
            "line",
            "memory_proposal",
        ]
    } else {
        &[
            "contract_version",
            "request_id",
            "client_instance_id",
            "namespace_id",
            "lease_id",
            "run_id",
            "companion_id",
            "generation",
            "memory_epoch",
            "line",
        ]
    };
    let fields = object_keys(value, keys)?;
    let mut decoded = decode_leased_with(&fields, "lease_id")?;
    for name in ["run_id", "companion_id"] {
        uuid_value(field(&fields, name)?)?;
    }
    positive_u64(field(&fields, "generation")?)?;
    positive_u64(field(&fields, "memory_epoch")?)?;
    if !valid_dialogue_line(field(&fields, "line")?.as_str()) {
        return Err(invalid_schema());
    }
    for name in [
        "run_id",
        "companion_id",
        "generation",
        "memory_epoch",
        "line",
    ] {
        decoded.extend_object(&[(name, field(&fields, name)?.clone())]);
    }
    if let Some(proposal) = proposal {
        let proposal_fields = object_keys(proposal, &["operation_id", "base_revision", "summary"])?;
        uuid_value(field(&proposal_fields, "operation_id")?)?;
        field(&proposal_fields, "base_revision")?
            .as_u64()
            .ok_or_else(invalid_schema)?;
        if !valid_memory_text(field(&proposal_fields, "summary")?.as_str(), 2048) {
            return Err(invalid_schema());
        }
        decoded.extend_object(&[("memory_proposal", proposal.clone())]);
    }
    Ok(decoded)
}

fn decode_environment(value: &Json) -> Result<(), ServerError> {
    let fields = object_keys(value, &["exposed_blocks", "heights"])?;
    if !matches!(field(&fields, "exposed_blocks")?, Json::Array(_))
        || !matches!(field(&fields, "heights")?, Json::Array(_))
    {
        return Err(invalid_schema());
    }
    let blocks = field(&fields, "exposed_blocks")?.as_array();
    let heights = field(&fields, "heights")?.as_array();
    if blocks.len() > 256 || heights.len() > 1089 {
        return Err(invalid_schema());
    }
    for block in blocks {
        let block_fields = object_keys(block, &["position", "block_id"])?;
        let position = field(&block_fields, "position")?;
        let position_fields = object_keys(position, &["x", "y", "z"])?;
        let y = position_integer(field(&position_fields, "y")?)?;
        if !(-64..=319).contains(&y) {
            return Err(invalid_schema());
        }
        position_integer(field(&position_fields, "x")?)?;
        position_integer(field(&position_fields, "z")?)?;
        let block_id = field(&block_fields, "block_id")?
            .as_u64()
            .ok_or_else(invalid_schema)?;
        if block_id == 0 || block_id == 65535 || block_id > 65534 {
            return Err(invalid_schema());
        }
    }
    for height in heights {
        let height_fields = object_keys(height, &["x", "z", "height"])?;
        position_integer(field(&height_fields, "x")?)?;
        position_integer(field(&height_fields, "z")?)?;
        let value = match field(&height_fields, "height")? {
            Json::I64(value) => *value,
            Json::U64(value) => *value as i64,
            _ => return Err(invalid_schema()),
        };
        if !(-65..=319).contains(&value) {
            return Err(invalid_schema());
        }
    }
    Ok(())
}

fn position_integer(value: &Json) -> Result<i64, ServerError> {
    match value {
        Json::I64(value) if *value >= i32::MIN as i64 && *value <= i32::MAX as i64 => Ok(*value),
        Json::U64(value) if *value <= i32::MAX as u64 => Ok(*value as i64),
        _ => Err(invalid_schema()),
    }
}

fn decode_memory_state(value: &Json) -> Result<Json, ServerError> {
    let fields = object_keys(value, &["revision", "operation_id", "summary"])?;
    match field(&fields, "revision")? {
        Json::U64(0) => {
            if field(&fields, "operation_id")? != &Json::Null
                || field(&fields, "summary")?.as_str() != ""
            {
                return Err(invalid_schema());
            }
        }
        Json::U64(value) if *value > 0 => {
            uuid_value(field(&fields, "operation_id")?)?;
            if !valid_memory_text(field(&fields, "summary")?.as_str(), 2048) {
                return Err(invalid_schema());
            }
        }
        _ => return Err(invalid_schema()),
    }
    Ok(value.clone())
}

fn decode_reconcile(value: &Json, mirror_key: &str, keys: &[&str]) -> Result<Json, ServerError> {
    let fields = object_keys(value, keys)?;
    let mut decoded = decode_leased_with(&fields, "lease_id")?;
    uuid_value(field(&fields, "companion_id")?)?;
    positive_u64(field(&fields, "memory_epoch")?)?;
    let active = match field(&fields, "active")? {
        Json::Bool(flag) => *flag,
        _ => return Err(invalid_schema()),
    };
    let tombstone = field(&fields, "tombstone_operation_id")?;
    let mirror = field(&fields, mirror_key)?;
    if active {
        if tombstone != &Json::Null {
            return Err(invalid_schema());
        }
        decode_memory_state(mirror)?;
    } else {
        uuid_value(tombstone)?;
        if mirror != &Json::Null {
            return Err(invalid_schema());
        }
    }
    for name in [
        "companion_id",
        "memory_epoch",
        "active",
        "tombstone_operation_id",
        mirror_key,
    ] {
        decoded.extend_object(&[(name, field(&fields, name)?.clone())]);
    }
    Ok(decoded)
}

/// The plan place-block table, mirroring the MCP schema enum.
const PLAN_BLOCKS: &[(&str, PlanBlock)] = &[
    ("brick", PlanBlock::Brick),
    ("chest", PlanBlock::Chest),
    ("clay", PlanBlock::Clay),
    ("cobblestone", PlanBlock::Cobblestone),
    ("dirt", PlanBlock::Dirt),
    ("furnace", PlanBlock::Furnace),
    ("glass", PlanBlock::Glass),
    ("grass", PlanBlock::Grass),
    ("gravel", PlanBlock::Gravel),
    ("iron_block", PlanBlock::IronBlock),
    ("leaves", PlanBlock::Leaves),
    ("light_block", PlanBlock::LightBlock),
    ("mossy_cobblestone", PlanBlock::MossyCobblestone),
    ("oak_log", PlanBlock::OakLog),
    ("oak_planks", PlanBlock::OakPlanks),
    ("roof_tile", PlanBlock::RoofTile),
    ("sand", PlanBlock::Sand),
    ("smooth_stone", PlanBlock::SmoothStone),
    ("snow_block", PlanBlock::SnowBlock),
    ("stone", PlanBlock::Stone),
    ("stone_brick", PlanBlock::StoneBrick),
    ("white_wool", PlanBlock::WhiteWool),
    ("workbench", PlanBlock::Workbench),
];

/// Decodes the MCP plan object: bounded summary, 1..=5000 steps, the step
/// oneOf, and a follow that may only close the plan.
fn decode_plan(value: &Json) -> Result<Json, ServerError> {
    let fields = object_keys(value, &["summary", "steps"])?;
    if !valid_non_blank_text(field(&fields, "summary")?.as_str(), 512) {
        return Err(invalid_schema());
    }
    if !matches!(field(&fields, "steps")?, Json::Array(_)) {
        return Err(invalid_schema());
    }
    let steps = field(&fields, "steps")?.as_array();
    if steps.is_empty() || steps.len() > 5000 {
        return Err(invalid_schema());
    }
    for (index, step) in steps.iter().enumerate() {
        let Json::Object(entries) = step else {
            return Err(invalid_schema());
        };
        let keys: Vec<&str> = entries.iter().map(|(key, _)| key.as_str()).collect();
        let kind = step.get("kind").map(Json::as_str).unwrap_or_default();
        match kind {
            "go_to" | "mine" => {
                if keys.len() != 4
                    || !keys.contains(&"x")
                    || !keys.contains(&"y")
                    || !keys.contains(&"z")
                {
                    return Err(invalid_schema());
                }
                let y = position_integer(step.get("y").unwrap_or(&Json::Null))?;
                if !(-64..=319).contains(&y) {
                    return Err(invalid_schema());
                }
                position_integer(step.get("x").unwrap_or(&Json::Null))?;
                position_integer(step.get("z").unwrap_or(&Json::Null))?;
            }
            "place" => {
                if keys.len() != 5 || !keys.contains(&"block") {
                    return Err(invalid_schema());
                }
                let y = position_integer(step.get("y").unwrap_or(&Json::Null))?;
                if !(-64..=319).contains(&y) {
                    return Err(invalid_schema());
                }
                position_integer(step.get("x").unwrap_or(&Json::Null))?;
                position_integer(step.get("z").unwrap_or(&Json::Null))?;
                let block = step.get("block").map(Json::as_str).unwrap_or_default();
                if !PLAN_BLOCKS.iter().any(|(name, _)| *name == block) {
                    return Err(invalid_schema());
                }
            }
            "follow" => {
                if keys.len() != 2 || index + 1 != steps.len() {
                    return Err(invalid_schema());
                }
                parse_canonical_uuid(step.get("player_id").map(Json::as_str).unwrap_or_default())
                    .ok_or_else(invalid_schema)?;
            }
            _ => return Err(invalid_schema()),
        }
    }
    Ok(value.clone())
}

// ---------------------------------------------------------------------------
// Typed response decode and echo binding
// ---------------------------------------------------------------------------

/// Decodes a success body into the typed response, binding every echoed
/// identity field to the exact request values before publication.
pub fn decode_success(request: &AgentRequest, body: &[u8]) -> Result<AgentResponse, ServerError> {
    let value = Json::parse(body)?;
    let uuid_of = |value: &Json| -> Result<[u8; 16], ServerError> {
        parse_canonical_uuid(value.as_str()).ok_or_else(unavailable)
    };
    let same_uuid = |value: &Json, expected: [u8; 16]| -> Result<(), ServerError> {
        if uuid_of(value)? == expected {
            Ok(())
        } else {
            Err(unavailable())
        }
    };
    let base_matches = |decoded: &Json, base: &BaseIdentity| -> Result<(), ServerError> {
        if decoded.get("contract_version").map(Json::as_str) != Some(AGENT_CONTRACT_VERSION) {
            return Err(unavailable());
        }
        same_uuid(
            decoded.get("request_id").unwrap_or(&Json::Null),
            base.request_id.bytes(),
        )?;
        same_uuid(
            decoded.get("client_instance_id").unwrap_or(&Json::Null),
            base.client_instance_id.bytes(),
        )?;
        same_uuid(
            decoded.get("namespace_id").unwrap_or(&Json::Null),
            base.namespace_id.bytes(),
        )
    };
    let leased_matches = |decoded: &Json, leased: &LeasedIdentity| -> Result<(), ServerError> {
        base_matches(decoded, &leased.base)?;
        same_uuid(
            decoded.get("lease_id").unwrap_or(&Json::Null),
            leased.lease_id.bytes(),
        )
    };
    let identity = |decoded: &Json| -> Result<LeasedIdentity, ServerError> {
        Ok(LeasedIdentity {
            base: BaseIdentity {
                request_id: AgentRequestId::try_from_bytes(uuid_of(
                    decoded.get("request_id").unwrap_or(&Json::Null),
                )?)
                .map_err(|_| unavailable())?,
                client_instance_id: ClientInstanceId::try_from_bytes(uuid_of(
                    decoded.get("client_instance_id").unwrap_or(&Json::Null),
                )?)
                .map_err(|_| unavailable())?,
                namespace_id: NamespaceId::try_from_bytes(uuid_of(
                    decoded.get("namespace_id").unwrap_or(&Json::Null),
                )?)
                .map_err(|_| unavailable())?,
            },
            lease_id: LeaseId::try_from_bytes(uuid_of(
                decoded.get("lease_id").unwrap_or(&Json::Null),
            )?)
            .map_err(|_| unavailable())?,
        })
    };
    match request {
        AgentRequest::Acquire(base) => {
            let decoded = schema_decode("acquire_response", &value).map_err(|_| unavailable())?;
            base_matches(&decoded, base)?;
            let lease_id = LeaseId::try_from_bytes(uuid_of(decoded.get("lease_id").unwrap())?)
                .map_err(|_| unavailable())?;
            Ok(AgentResponse::Acquire(LeaseResponse {
                leased: LeasedIdentity {
                    base: base.clone(),
                    lease_id,
                },
            }))
        }
        AgentRequest::Heartbeat(leased) => {
            let decoded = schema_decode("heartbeat_response", &value).map_err(|_| unavailable())?;
            leased_matches(&decoded, leased)?;
            Ok(AgentResponse::Heartbeat(LeaseResponse {
                leased: identity(&decoded)?,
            }))
        }
        AgentRequest::Release(leased) => {
            let decoded = schema_decode("release_response", &value).map_err(|_| unavailable())?;
            leased_matches(&decoded, leased)?;
            Ok(AgentResponse::Release(LeaseResponse {
                leased: identity(&decoded)?,
            }))
        }
        AgentRequest::Plan(plan) => {
            let decoded = schema_decode("plan_response", &value).map_err(|_| unavailable())?;
            leased_matches(&decoded, &plan.leased)?;
            same_uuid(
                decoded.get("run_id").unwrap_or(&Json::Null),
                plan.run_id.bytes(),
            )?;
            same_uuid(
                decoded.get("companion_id").unwrap_or(&Json::Null),
                plan.companion_id.bytes(),
            )?;
            if decoded.get("generation").and_then(Json::as_u64) != Some(plan.generation) {
                return Err(unavailable());
            }
            same_uuid(
                decoded.get("snapshot_id").unwrap_or(&Json::Null),
                plan.snapshot_id.bytes(),
            )?;
            if decoded.get("snapshot_digest").map(Json::as_str)
                != Some(hex_lower(&plan.snapshot_digest).as_str())
            {
                return Err(unavailable());
            }
            let typed_plan = typed_plan(decoded.get("plan").unwrap_or(&Json::Null))?;
            Ok(AgentResponse::Plan(PlanResponse {
                leased: identity(&decoded)?,
                run_id: plan.run_id,
                companion_id: plan.companion_id,
                generation: plan.generation,
                snapshot_id: plan.snapshot_id,
                snapshot_digest: plan.snapshot_digest,
                plan: typed_plan,
            }))
        }
        AgentRequest::Cancel(cancel) => {
            let decoded = schema_decode("cancel_response", &value).map_err(|_| unavailable())?;
            leased_matches(&decoded, &cancel.leased)?;
            same_uuid(
                decoded.get("run_id").unwrap_or(&Json::Null),
                cancel.run_id.bytes(),
            )?;
            let cancelled = match decoded.get("cancelled") {
                Some(Json::Bool(flag)) => *flag,
                _ => return Err(unavailable()),
            };
            Ok(AgentResponse::Cancel(CancelResponse {
                leased: identity(&decoded)?,
                run_id: cancel.run_id,
                cancelled,
            }))
        }
        AgentRequest::Dialogue(dialogue) => {
            let decoded = schema_decode("dialogue_response", &value).map_err(|_| unavailable())?;
            leased_matches(&decoded, &dialogue.leased)?;
            same_uuid(
                decoded.get("run_id").unwrap_or(&Json::Null),
                dialogue.run_id.bytes(),
            )?;
            same_uuid(
                decoded.get("companion_id").unwrap_or(&Json::Null),
                dialogue.companion_id.bytes(),
            )?;
            if decoded.get("generation").and_then(Json::as_u64) != Some(dialogue.generation)
                || decoded.get("memory_epoch").and_then(Json::as_u64) != Some(dialogue.memory_epoch)
            {
                return Err(unavailable());
            }
            let memory_proposal = match decoded.get("memory_proposal") {
                Some(proposal) => Some(MemoryProposal {
                    operation_id: OperationId::try_from_bytes(uuid_of(
                        proposal.get("operation_id").unwrap_or(&Json::Null),
                    )?)
                    .map_err(|_| unavailable())?,
                    base_revision: proposal
                        .get("base_revision")
                        .and_then(Json::as_u64)
                        .ok_or_else(unavailable)?,
                    summary: proposal
                        .get("summary")
                        .map(Json::as_str)
                        .unwrap_or_default()
                        .to_owned(),
                }),
                None => None,
            };
            Ok(AgentResponse::Dialogue(DialogueResponse {
                leased: identity(&decoded)?,
                run_id: dialogue.run_id,
                companion_id: dialogue.companion_id,
                generation: dialogue.generation,
                memory_epoch: dialogue.memory_epoch,
                line: decoded
                    .get("line")
                    .map(Json::as_str)
                    .unwrap_or_default()
                    .to_owned(),
                memory_proposal,
            }))
        }
        AgentRequest::Reconcile(reconcile) => {
            let decoded =
                schema_decode("memory_reconcile_response", &value).map_err(|_| unavailable())?;
            let (leased, companion_id, memory_epoch) = match reconcile {
                ReconcileRequest::Active {
                    leased,
                    companion_id,
                    memory_epoch,
                    ..
                }
                | ReconcileRequest::Inactive {
                    leased,
                    companion_id,
                    memory_epoch,
                    ..
                } => (leased, companion_id, memory_epoch),
            };
            leased_matches(&decoded, leased)?;
            same_uuid(
                decoded.get("companion_id").unwrap_or(&Json::Null),
                companion_id.bytes(),
            )?;
            if decoded.get("memory_epoch").and_then(Json::as_u64) != Some(*memory_epoch) {
                return Err(unavailable());
            }
            let active = matches!(decoded.get("active"), Some(Json::Bool(true)));
            let memory = typed_memory_state(decoded.get("memory").unwrap_or(&Json::Null))?;
            let tombstone = match decoded.get("tombstone_operation_id") {
                Some(Json::Str(text)) => Some(
                    OperationId::try_from_bytes(
                        parse_canonical_uuid(text).ok_or_else(unavailable)?,
                    )
                    .map_err(|_| unavailable())?,
                ),
                _ => None,
            };
            let response = if active {
                ReconcileResponse::Active {
                    leased: identity(&decoded)?,
                    companion_id: *companion_id,
                    memory_epoch: *memory_epoch,
                    memory,
                }
            } else {
                ReconcileResponse::Inactive {
                    leased: identity(&decoded)?,
                    companion_id: *companion_id,
                    memory_epoch: *memory_epoch,
                    tombstone_operation_id: tombstone.ok_or_else(unavailable)?,
                }
            };
            Ok(AgentResponse::Reconcile(response))
        }
        AgentRequest::Commit(commit) => {
            let decoded =
                schema_decode("memory_commit_response", &value).map_err(|_| unavailable())?;
            leased_matches(&decoded, &commit.leased)?;
            same_uuid(
                decoded.get("companion_id").unwrap_or(&Json::Null),
                commit.companion_id.bytes(),
            )?;
            if decoded.get("memory_epoch").and_then(Json::as_u64) != Some(commit.memory_epoch) {
                return Err(unavailable());
            }
            Ok(AgentResponse::Commit(CommitResponse {
                leased: identity(&decoded)?,
                companion_id: commit.companion_id,
                memory_epoch: commit.memory_epoch,
                operation_id: OperationId::try_from_bytes(uuid_of(
                    decoded.get("operation_id").unwrap_or(&Json::Null),
                )?)
                .map_err(|_| unavailable())?,
                committed_revision: NonZeroU64::new(
                    decoded
                        .get("committed_revision")
                        .and_then(Json::as_u64)
                        .ok_or_else(unavailable)?,
                )
                .ok_or_else(unavailable)?,
            }))
        }
        AgentRequest::Delete(delete) => {
            let decoded =
                schema_decode("memory_delete_response", &value).map_err(|_| unavailable())?;
            leased_matches(&decoded, &delete.leased)?;
            same_uuid(
                decoded.get("companion_id").unwrap_or(&Json::Null),
                delete.companion_id.bytes(),
            )?;
            Ok(AgentResponse::Delete(DeleteResponse {
                leased: identity(&decoded)?,
                companion_id: delete.companion_id,
                memory_epoch: decoded
                    .get("memory_epoch")
                    .and_then(Json::as_u64)
                    .ok_or_else(unavailable)?,
                tombstone_operation_id: OperationId::try_from_bytes(uuid_of(
                    decoded.get("tombstone_operation_id").unwrap_or(&Json::Null),
                )?)
                .map_err(|_| unavailable())?,
            }))
        }
    }
}

fn typed_memory_state(value: &Json) -> Result<MemoryState, ServerError> {
    let revision = value.get("revision").and_then(Json::as_u64).unwrap_or(0);
    if revision == 0 {
        return Ok(MemoryState::Absent);
    }
    Ok(MemoryState::Present {
        revision: NonZeroU64::new(revision).ok_or_else(invalid_schema)?,
        operation_id: OperationId::try_from_bytes(
            parse_canonical_uuid(
                value
                    .get("operation_id")
                    .map(Json::as_str)
                    .unwrap_or_default(),
            )
            .ok_or_else(invalid_schema)?,
        )
        .map_err(|_| invalid_schema())?,
        summary: value
            .get("summary")
            .map(Json::as_str)
            .unwrap_or_default()
            .to_owned(),
    })
}

/// Builds the typed plan from a decoded plan object.
fn typed_plan(value: &Json) -> Result<AgentPlan, ServerError> {
    let summary = value
        .get("summary")
        .map(Json::as_str)
        .unwrap_or_default()
        .to_owned();
    let mut steps = Vec::new();
    for step in value.get("steps").map(Json::as_array).unwrap_or(&[]) {
        let kind = step.get("kind").map(Json::as_str).unwrap_or_default();
        let integer = |name: &str| -> Result<i32, ServerError> {
            Ok(position_integer(step.get(name).unwrap_or(&Json::Null))? as i32)
        };
        match kind {
            "go_to" => steps.push(PlanStep::GoTo {
                x: integer("x")?,
                y: integer("y")?,
                z: integer("z")?,
            }),
            "mine" => steps.push(PlanStep::Mine {
                x: integer("x")?,
                y: integer("y")?,
                z: integer("z")?,
            }),
            "place" => {
                let block = step.get("block").map(Json::as_str).unwrap_or_default();
                let found = PLAN_BLOCKS
                    .iter()
                    .find(|(name, _)| *name == block)
                    .map(|(_, kind)| *kind)
                    .ok_or_else(invalid_schema)?;
                steps.push(PlanStep::Place {
                    x: integer("x")?,
                    y: integer("y")?,
                    z: integer("z")?,
                    block: found,
                })
            }
            "follow" => steps.push(PlanStep::Follow {
                player_id: PlayerId::try_from_bytes(
                    parse_canonical_uuid(
                        step.get("player_id").map(Json::as_str).unwrap_or_default(),
                    )
                    .ok_or_else(invalid_schema)?,
                )
                .map_err(|_| invalid_schema())?,
            }),
            _ => return Err(invalid_schema()),
        }
    }
    AgentPlan::try_new(summary, steps).map_err(|_| unavailable())
}

/// Maps a non-200 response to the typed Agent error when the envelope, the
/// stable code, the route/status pair, and the optional request echo all
/// validate; any other failure is AgentUnavailable.
pub fn error_failure(
    route: &str,
    status: u16,
    expected_request_id: &AgentRequestId,
    body: &[u8],
) -> ServerError {
    let value = match Json::parse(body) {
        Ok(value) => value,
        Err(_) => return unavailable(),
    };
    let decoded = match schema_decode("error_response", &value) {
        Ok(decoded) => decoded,
        Err(_) => return unavailable(),
    };
    let code_text = decoded
        .get("error")
        .and_then(|error| error.get("code"))
        .map(Json::as_str)
        .unwrap_or_default();
    let Some(code) = stable_error_code_value(code_text) else {
        return unavailable();
    };
    if !route_allows_error(route, code) || code.status() != status {
        return unavailable();
    }
    // A present request id must echo the exact expected value.
    let request_id = decoded.get("request_id").unwrap_or(&Json::Null);
    if *request_id != Json::Null {
        match parse_canonical_uuid(request_id.as_str()) {
            Some(bytes) if bytes == expected_request_id.bytes() => {}
            _ => return unavailable(),
        }
    }
    ServerError::Agent {
        code,
        status: code.status(),
    }
}

fn stable_error_code(code: &str) -> bool {
    stable_error_code_value(code).is_some()
}

fn stable_error_code_value(code: &str) -> Option<AgentErrorCode> {
    Some(match code {
        "invalid_request" => AgentErrorCode::InvalidRequest,
        "unauthorized" => AgentErrorCode::Unauthorized,
        "unsupported_version" => AgentErrorCode::UnsupportedVersion,
        "namespace_conflict" => AgentErrorCode::NamespaceConflict,
        "overloaded" => AgentErrorCode::Overloaded,
        "deadline_exceeded" => AgentErrorCode::DeadlineExceeded,
        "agent_unavailable" => AgentErrorCode::AgentUnavailable,
        "invalid_model_output" => AgentErrorCode::InvalidModelOutput,
        "memory_conflict" => AgentErrorCode::MemoryConflict,
        "not_found" => AgentErrorCode::NotFound,
        "internal_error" => AgentErrorCode::InternalError,
        _ => return None,
    })
}

/// The manifest route error tables: only these route/code pairs exist.
fn route_allows_error(route: &str, code: AgentErrorCode) -> bool {
    let allowed: &[AgentErrorCode] = match route {
        "/v1/namespaces/acquire" => &[
            AgentErrorCode::InvalidRequest,
            AgentErrorCode::Unauthorized,
            AgentErrorCode::UnsupportedVersion,
            AgentErrorCode::NamespaceConflict,
            AgentErrorCode::InternalError,
        ],
        "/v1/namespaces/heartbeat" | "/v1/namespaces/release" | "/v1/runs/cancel" => &[
            AgentErrorCode::InvalidRequest,
            AgentErrorCode::Unauthorized,
            AgentErrorCode::UnsupportedVersion,
            AgentErrorCode::NotFound,
            AgentErrorCode::InternalError,
        ],
        "/v1/plan" => &[
            AgentErrorCode::InvalidRequest,
            AgentErrorCode::Unauthorized,
            AgentErrorCode::UnsupportedVersion,
            AgentErrorCode::Overloaded,
            AgentErrorCode::DeadlineExceeded,
            AgentErrorCode::AgentUnavailable,
            AgentErrorCode::InvalidModelOutput,
            AgentErrorCode::NotFound,
            AgentErrorCode::InternalError,
        ],
        "/v1/dialogue" => &[
            AgentErrorCode::InvalidRequest,
            AgentErrorCode::Unauthorized,
            AgentErrorCode::UnsupportedVersion,
            AgentErrorCode::Overloaded,
            AgentErrorCode::DeadlineExceeded,
            AgentErrorCode::AgentUnavailable,
            AgentErrorCode::InvalidModelOutput,
            AgentErrorCode::MemoryConflict,
            AgentErrorCode::NotFound,
            AgentErrorCode::InternalError,
        ],
        "/v1/memory/reconcile" | "/v1/memory/commit" | "/v1/memory/delete" => &[
            AgentErrorCode::InvalidRequest,
            AgentErrorCode::Unauthorized,
            AgentErrorCode::UnsupportedVersion,
            AgentErrorCode::MemoryConflict,
            AgentErrorCode::NotFound,
            AgentErrorCode::InternalError,
        ],
        _ => return false,
    };
    allowed.contains(&code)
}

/// A success body that is oversized or undecodable is invalid model output
/// only for an accepted `/v1/plan` success; every other route reports the
/// Agent protocol as unavailable.
fn plan_success_body_failure(route: &str, status: u16) -> ServerError {
    if route == "/v1/plan" && status == 200 {
        ServerError::Agent {
            code: AgentErrorCode::InvalidModelOutput,
            status: AgentErrorCode::InvalidModelOutput.status(),
        }
    } else {
        unavailable()
    }
}
