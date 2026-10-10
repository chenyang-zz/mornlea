//! Agent service endpoint: Go `AgentServiceSettings.Validate` acceptance and
//! the exact request Go's `AgentClient` derives from an accepted endpoint.
//!
//! Acceptance follows Go 1.26 `url.Parse` with strict colons: scheme `http`
//! in any case, no userinfo, an empty query and fragment, valid path
//! escapes, an optional port in 1..65535 (leading zeros allowed), and a
//! loopback IP literal host (IPv4-mapped loopback included, zones
//! excluded). Text before a `[` in the host is dropped, as Go does.
//!
//! The request matches Go's `AgentClient.post`: trailing slashes are
//! trimmed, the route is appended, and the joined text is parsed again, so a
//! trailing `?` moves the route into the query and an empty `#` drops it.
//! The request target is Go `URL.RequestURI`, the `Host` header is the URL
//! host without an empty port, and the dial address is the host IP (IPv4
//! for an IPv4-mapped literal) with the port or 80.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};

use crate::contracts::ServerError;

/// A Go-accepted Agent service endpoint.
#[derive(Clone, Debug)]
pub struct AgentEndpoint {
    addr: SocketAddr,
    host: String,
    tail: String,
}

impl AgentEndpoint {
    /// Accepts exactly the endpoints Go `AgentServiceSettings.Validate`
    /// accepts; every other text is `InvalidInput`.
    pub fn parse(endpoint: &str) -> Result<Self, ServerError> {
        accepted(endpoint).ok_or(ServerError::InvalidInput {
            field: "agent_endpoint",
        })
    }

    /// Address Go dials.
    pub fn socket_address(&self) -> SocketAddr {
        self.addr
    }

    /// `Host` header value Go sends.
    pub fn host_header(&self) -> &str {
        &self.host
    }

    /// Go `URL.RequestURI` of the trimmed endpoint with `route` appended.
    pub fn request_target(&self, route: &str) -> String {
        let joined = format!("{}{route}", self.tail);
        let before_fragment = joined
            .split_once('#')
            .map_or(joined.as_str(), |(head, _)| head);
        let (path, query) = match before_fragment.split_once('?') {
            Some((path, query)) => (path, Some(query)),
            None => (before_fragment, None),
        };
        let mut target = if path.is_empty() {
            "/".to_owned()
        } else {
            escaped_path(path)
        };
        if let Some(query) = query {
            target.push('?');
            target.push_str(query);
        }
        target
    }
}

fn accepted(endpoint: &str) -> Option<AgentEndpoint> {
    if endpoint.is_empty() {
        return None;
    }
    let (url, fragment) = endpoint.split_once('#').unwrap_or((endpoint, ""));
    if !fragment.is_empty() || url.bytes().any(|byte| byte < 0x20 || byte == 0x7f) {
        return None;
    }
    let (scheme, mut rest) = url.split_once(':')?;
    if !scheme.eq_ignore_ascii_case("http") {
        return None;
    }
    if rest.ends_with('?') && rest.matches('?').count() == 1 {
        rest = &rest[..rest.len() - 1];
    } else if let Some((before, query)) = rest.split_once('?') {
        if !query.is_empty() {
            return None;
        }
        rest = before;
    }
    let after = rest.strip_prefix("//")?;
    let (authority, path) = after.find('/').map_or((after, ""), |i| after.split_at(i));
    if authority.contains('@') || !valid_path_escapes(path) {
        return None;
    }
    let (host, ip, port) = if let Some(open) = authority.rfind('[') {
        let close = authority.rfind(']')?;
        let colon_port = &authority[close + 1..];
        if !valid_optional_port(colon_port) || close < open {
            return None;
        }
        let inner = &authority[open + 1..close];
        if inner.contains('%') {
            return None;
        }
        let ip = inner.parse::<Ipv6Addr>().ok()?;
        (
            &authority[open..],
            IpAddr::V6(ip),
            colon_port.get(1..).unwrap_or(""),
        )
    } else {
        let (name, colon_port) = authority
            .find(':')
            .map_or((authority, ""), |i| authority.split_at(i));
        if !valid_optional_port(colon_port) || name.contains('%') {
            return None;
        }
        let ip = name.parse::<Ipv4Addr>().ok()?;
        (authority, IpAddr::V4(ip), colon_port.get(1..).unwrap_or(""))
    };
    let port = if port.is_empty() {
        80
    } else {
        match port.parse::<u16>() {
            Ok(value) if value != 0 => value,
            _ => return None,
        }
    };
    let ip = ip.to_canonical();
    if !ip.is_loopback() {
        return None;
    }
    // Go `removeEmptyPort` on the request host.
    let host = host.strip_suffix(':').unwrap_or(host).to_owned();
    // The authority holds no `/`, so trimming never reaches it.
    let trimmed = endpoint.trim_end_matches('/');
    let tail = trimmed
        .get(scheme.len() + 3 + authority.len()..)
        .unwrap_or("")
        .to_owned();
    Some(AgentEndpoint {
        addr: SocketAddr::new(ip, port),
        host,
        tail,
    })
}

fn valid_optional_port(colon_port: &str) -> bool {
    colon_port.is_empty()
        || colon_port
            .strip_prefix(':')
            .is_some_and(|digits| digits.bytes().all(|byte| byte.is_ascii_digit()))
}

fn valid_path_escapes(path: &str) -> bool {
    let bytes = path.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            let hex = |offset: usize| bytes.get(index + offset).is_some_and(u8::is_ascii_hexdigit);
            if !hex(1) || !hex(2) {
                return false;
            }
            index += 3;
        } else {
            index += 1;
        }
    }
    true
}

/// Go `shouldEscape(c, encodePath)`.
fn path_byte_needs_escape(byte: u8) -> bool {
    !(byte.is_ascii_alphanumeric()
        || matches!(
            byte,
            b'-' | b'_'
                | b'.'
                | b'~'
                | b'$'
                | b'&'
                | b'+'
                | b','
                | b'/'
                | b':'
                | b';'
                | b'='
                | b'@'
        ))
}

/// Go `URL.EscapedPath` for a parsed path: the raw text when it is a valid
/// encoding, otherwise the decoded path escaped again.
fn escaped_path(raw: &str) -> String {
    let valid_encoded = raw.bytes().all(|byte| {
        matches!(
            byte,
            b'!' | b'$'
                | b'&'
                | b'\''
                | b'('
                | b')'
                | b'*'
                | b'+'
                | b','
                | b';'
                | b'='
                | b':'
                | b'@'
                | b'['
                | b']'
                | b'%'
        ) || !path_byte_needs_escape(byte)
    });
    if valid_encoded {
        return raw.to_owned();
    }
    let bytes = raw.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        let pair = bytes
            .get(index + 1..index + 3)
            .and_then(|pair| std::str::from_utf8(pair).ok())
            .and_then(|pair| u8::from_str_radix(pair, 16).ok());
        match (bytes[index], pair) {
            (b'%', Some(value)) => {
                decoded.push(value);
                index += 3;
            }
            (byte, _) => {
                decoded.push(byte);
                index += 1;
            }
        }
    }
    let mut escaped = String::with_capacity(decoded.len() * 3);
    for byte in decoded {
        if path_byte_needs_escape(byte) {
            escaped.push_str(&format!("%{byte:02X}"));
        } else {
            escaped.push(char::from(byte));
        }
    }
    escaped
}
