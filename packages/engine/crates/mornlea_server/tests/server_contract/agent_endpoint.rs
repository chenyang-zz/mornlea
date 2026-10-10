//! Agent service endpoint spellings follow Go's real handling.
//!
//! The reference is the actual Go code, run as a helper: Go
//! `AgentServiceSettings.Validate` decides acceptance, then `net/http`
//! builds the request exactly like `AgentClient.post` (trailing slashes
//! trimmed, the route appended, the joined text parsed by `url.Parse`) and
//! writes it through an `http.Transport`. The helper reports the resolved
//! dial address, the request line, and the `Host` header. Every spelling Go
//! accepts must construct the Rust wire with the same three values, so no
//! accepted spelling can stop startup. A spelling whose route Go moves into
//! the query or drops with the fragment reaches the Agent at that same
//! wrong target, where it fails like an unreachable Agent.

use std::fs;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use mornlea_server::agent::http::{AgentEndpoint, AgentHttpWire};
use mornlea_server::agent::lease::{AgentWire, RpcCancellation};
use mornlea_server::contracts::{
    AgentRequest, AgentRequestId, BaseIdentity, ClientInstanceId, Clock, Deadline, NamespaceId,
};

const ROUTE: &str = "/v1/namespaces/acquire";

/// Spellings Go accepts. `{P}` is a live loopback port owned by the test.
const GO_ACCEPTED: &[&str] = &[
    "http://127.0.0.1:{P}",
    "HTTP://127.0.0.1:{P}",
    "hTtP://[::1]:{P}/",
    "http://127.0.0.1:0{P}",
    "http://127.0.0.1:00000{P}",
    "HTTP://[::1]:0{P}/agent/v1?",
    "http://127.0.0.1:{P}/agent/v1/",
    "http://127.0.0.1:{P}///",
    "HTTP://127.0.0.1:{P}/AGENT",
    "http://127.0.0.1:{P}?",
    "http://127.0.0.1:{P}/a?",
    "http://127.0.0.1:{P}/a/?",
    "http://127.0.0.1:{P}/#",
    "http://[::1]:{P}#",
    "http://127.0.0.1:{P}?#",
    "http://[::1]:{P}/agent/v1?#",
    "http://x[::1]:{P}",
    "http://abc[::1]:{P}/agent",
    "http://[0:0:0:0:0:0:0:1]:{P}",
    "http://[::ffff:127.0.0.1]:{P}",
    "http://127.0.0.1:{P}/agent%2Fv1",
    "http://127.0.0.1:{P}/a%2f",
    "http://127.0.0.1:{P}/a%41",
    "http://127.0.0.1:{P}/a%20b",
    "http://127.0.0.1:{P}/a b",
    "http://127.0.0.1:{P}/a%41 b",
    "http://127.0.0.1:{P}/ü",
    "http://127.0.0.1:{P}//double//",
    "http://127.0.0.1:{P}/a!$&'()*+,;=:@[]",
    "http://127.0.0.1:{P}/a\"b",
    "http://127.0.0.1:{P}/a<b>{}|\\^`",
    "http://127.0.0.1:/",
    "http://127.0.0.1",
    "http://127.9.9.9",
    "http://[::ffff:127.0.0.2]/#",
    "http://x[::1]:80",
];

/// Spellings Go refuses; the Rust wire refuses them too.
const GO_REFUSED: &[&str] = &[
    "",
    "https://127.0.0.1",
    "http://localhost:1",
    "http://192.168.0.1:1",
    "http://user@127.0.0.1",
    "http://127.0.0.1/?a",
    "http://127.0.0.1/#a",
    "http://127.0.0.1:{P}/#/",
    "http://127.0.0.1:0",
    "http://127.0.0.1:65536",
    "http://127.0.0.1/%zz",
    "http://::1:80",
    "http://[127.0.0.1]:80",
    "http://[::1%25lo]:80",
    "http://127.0.0.01",
    "http:127.0.0.1",
    "http:/127.0.0.1",
];

const GO_HELPER: &str = r#"package main

import (
	"bufio"
	"context"
	"fmt"
	"net"
	"net/http"
	"os"
	"strings"
	"time"

	"github.com/channing771/mornlea/packages/shared/companion"
)

// Reports, per endpoint argument, Go's acceptance and the request the
// production AgentClient would send: trailing slashes trimmed, the route
// appended, the joined text parsed by net/http and written by a Transport.
// The dial is redirected to a local capture listener after recording it.
func main() {
	listener, err := net.Listen("tcp", "127.0.0.1:0")
	if err != nil {
		panic(err)
	}
	for _, endpoint := range os.Args[1:] {
		settings := companion.AgentServiceSettings{Endpoint: endpoint, APIKeyEnv: "K"}
		if settings.Validate() != nil {
			fmt.Println("REFUSE")
			continue
		}
		trimmed := endpoint
		for strings.HasSuffix(trimmed, "/") {
			trimmed = trimmed[:len(trimmed)-1]
		}
		request, err := http.NewRequest(http.MethodPost, trimmed+"/v1/namespaces/acquire", strings.NewReader("{}"))
		if err != nil {
			fmt.Println("REQUEST_ERROR")
			continue
		}
		dialed := make(chan string, 1)
		transport := &http.Transport{
			DisableKeepAlives:  true,
			DisableCompression: true,
			DialContext: func(ctx context.Context, network, address string) (net.Conn, error) {
				resolved, err := net.ResolveTCPAddr(network, address)
				if err != nil {
					return nil, err
				}
				dialed <- resolved.String()
				return (&net.Dialer{}).DialContext(ctx, "tcp", listener.Addr().String())
			},
		}
		captured := make(chan [2]string, 1)
		go func() {
			conn, err := listener.Accept()
			if err != nil {
				captured <- [2]string{}
				return
			}
			reader := bufio.NewReader(conn)
			line, _ := reader.ReadString('\n')
			host := ""
			for {
				header, err := reader.ReadString('\n')
				if err != nil || header == "\r\n" {
					break
				}
				if name, value, ok := strings.Cut(header, ":"); ok && strings.EqualFold(name, "Host") {
					host = strings.TrimSpace(value)
				}
			}
			conn.Write([]byte("HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"))
			conn.Close()
			captured <- [2]string{strings.TrimSuffix(line, "\r\n"), host}
		}()
		client := &http.Client{Transport: transport, Timeout: 5 * time.Second}
		if response, err := client.Do(request); err == nil {
			response.Body.Close()
		}
		select {
		case address := <-dialed:
			got := <-captured
			fmt.Printf("ACCEPT\t%s\t%s\t%s\n", address, got[0], got[1])
		case <-time.After(5 * time.Second):
			fmt.Println("NO_DIAL")
		}
	}
}
"#;

static ROOTS: AtomicU64 = AtomicU64::new(0);

struct Root(PathBuf);

impl Drop for Root {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

struct OwnedChild(Child);

impl Drop for OwnedChild {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

struct WallClock;

impl Clock for WallClock {
    fn monotonic(&self) -> Instant {
        Instant::now()
    }

    fn unix_ms(&self) -> i64 {
        1_800_000_000_000
    }
}

fn uuid(tag: u8) -> [u8; 16] {
    let mut bytes = [0u8; 16];
    bytes[0] = tag;
    bytes[6] = 0x40;
    bytes[8] = 0x80;
    bytes
}

fn acquire() -> AgentRequest {
    AgentRequest::Acquire(BaseIdentity {
        request_id: AgentRequestId::try_from_bytes(uuid(1)).unwrap(),
        client_instance_id: ClientInstanceId::try_from_bytes(uuid(2)).unwrap(),
        namespace_id: NamespaceId::try_from_bytes(uuid(3)).unwrap(),
    })
}

/// One loopback port listening on both IPv4 and IPv6 loopback.
struct DualListener {
    port: u16,
    v4: TcpListener,
    v6: TcpListener,
}

impl DualListener {
    fn bind() -> Self {
        for _ in 0..64 {
            let v4 = TcpListener::bind("127.0.0.1:0").unwrap();
            let port = v4.local_addr().unwrap().port();
            if let Ok(v6) = TcpListener::bind(("::1", port)) {
                return Self { port, v4, v6 };
            }
        }
        panic!("no loopback port free on both families");
    }

    fn for_address(&self, address: SocketAddr) -> &TcpListener {
        if address.is_ipv4() {
            &self.v4
        } else {
            &self.v6
        }
    }
}

fn with_port(form: &str, port: u16) -> String {
    form.replace("{P}", &port.to_string())
}

/// Runs the Go helper over `endpoints` and returns one report per endpoint.
fn go_reports(endpoints: &[String]) -> Vec<String> {
    let root = Root(std::env::temp_dir().join(format!(
        "mornlea-agent-endpoint-go-{}-{}",
        std::process::id(),
        ROOTS.fetch_add(1, Ordering::Relaxed)
    )));
    fs::create_dir_all(&root.0).unwrap();
    let source = root.0.join("helper.go");
    fs::write(&source, GO_HELPER).unwrap();
    let repository = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../..");
    let stdout = root.0.join("stdout");
    let stderr = root.0.join("stderr");
    let mut child = OwnedChild(
        Command::new("go")
            .arg("run")
            .arg(&source)
            .args(endpoints)
            .current_dir(repository)
            .stdin(Stdio::null())
            .stdout(fs::File::create(&stdout).unwrap())
            .stderr(fs::File::create(&stderr).unwrap())
            .spawn()
            .expect("actual Go compiler is required"),
    );
    let deadline = Instant::now() + Duration::from_secs(180);
    let status = loop {
        if let Some(status) = child.0.try_wait().unwrap() {
            break status;
        }
        assert!(Instant::now() < deadline, "Go helper timed out");
        thread::sleep(Duration::from_millis(10));
    };
    let bounded = |path: &Path| {
        let mut text = String::new();
        fs::File::open(path)
            .unwrap()
            .take(65_537)
            .read_to_string(&mut text)
            .unwrap();
        assert!(text.len() <= 65_536, "Go report exceeded its bound");
        text
    };
    assert!(status.success(), "Go helper failed: {}", bounded(&stderr));
    let reports: Vec<String> = bounded(&stdout).lines().map(str::to_owned).collect();
    assert_eq!(reports.len(), endpoints.len(), "one Go report per endpoint");
    reports
}

/// Sends one acquire through the real Rust wire and returns the request line
/// and `Host` header the listener received.
fn rust_capture(wire: &AgentHttpWire, listener: &TcpListener) -> (String, String) {
    let listener = listener.try_clone().unwrap();
    let peer = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        let mut head = Vec::new();
        let mut byte = [0u8; 1];
        while !head.ends_with(b"\r\n\r\n") {
            assert_eq!(stream.read(&mut byte).unwrap(), 1, "request head ended");
            head.push(byte[0]);
            assert!(head.len() <= 16_384, "request head bounded");
        }
        let _ = stream
            .write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
        let text = String::from_utf8(head).unwrap();
        let mut lines = text.split("\r\n");
        let line = lines.next().unwrap().to_owned();
        let host = lines
            .filter_map(|header| header.split_once(':'))
            .find(|(name, _)| name.eq_ignore_ascii_case("host"))
            .map(|(_, value)| value.trim().to_owned())
            .unwrap_or_default();
        (line, host)
    });
    let deadline = Deadline::after(Instant::now(), Duration::from_secs(5)).unwrap();
    assert!(
        wire.rpc_cancellable(acquire(), deadline, &RpcCancellation::default())
            .is_err(),
        "a 404 never publishes a response"
    );
    peer.join().unwrap()
}

/// Every Go-accepted spelling yields Go's dial address, request target, and
/// `Host` header; the live-port spellings are also checked on the real wire.
#[test]
fn go_accepted_endpoint_spellings_send_go_request() {
    let listener = DualListener::bind();
    let endpoints: Vec<String> = GO_ACCEPTED
        .iter()
        .map(|form| with_port(form, listener.port))
        .collect();
    let reports = go_reports(&endpoints);
    let clock: Arc<dyn Clock + Send + Sync> = Arc::new(WallClock);
    for ((form, endpoint), report) in GO_ACCEPTED.iter().zip(&endpoints).zip(&reports) {
        let fields: Vec<&str> = report.split('\t').collect();
        assert_eq!(fields.len(), 4, "{form}: Go must accept: {report}");
        assert_eq!(fields[0], "ACCEPT", "{form}");
        let parsed = AgentEndpoint::parse(endpoint).unwrap_or_else(|error| {
            panic!("{form}: Rust refused a Go-accepted endpoint: {error:?}")
        });
        let target = parsed.request_target(ROUTE);
        assert_eq!(
            (
                parsed.socket_address().to_string(),
                format!("POST {target} HTTP/1.1"),
                parsed.host_header().to_owned(),
            ),
            (
                fields[1].to_owned(),
                fields[2].to_owned(),
                fields[3].to_owned()
            ),
            "{form}"
        );
        if form.contains("{P}") {
            let wire = AgentHttpWire::try_new(endpoint, "secret", clock.clone())
                .unwrap_or_else(|error| panic!("{form}: wire refused: {error:?}"));
            let captured = rust_capture(&wire, listener.for_address(parsed.socket_address()));
            assert_eq!(
                captured,
                (fields[2].to_owned(), fields[3].to_owned()),
                "{form}: real wire bytes"
            );
        }
    }
}

/// Go-refused spellings construct no Rust endpoint and no wire.
#[test]
fn go_refused_endpoint_spellings_refuse_typed() {
    let port = TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let endpoints: Vec<String> = GO_REFUSED
        .iter()
        .map(|form| with_port(form, port))
        .collect();
    let reports = go_reports(&endpoints);
    let clock: Arc<dyn Clock + Send + Sync> = Arc::new(WallClock);
    for ((form, endpoint), report) in GO_REFUSED.iter().zip(&endpoints).zip(&reports) {
        assert_eq!(report, "REFUSE", "{form}: Go must refuse");
        assert!(AgentEndpoint::parse(endpoint).is_err(), "{form}");
        assert!(
            AgentHttpWire::try_new(endpoint, "secret", clock.clone()).is_err(),
            "{form}"
        );
    }
}
