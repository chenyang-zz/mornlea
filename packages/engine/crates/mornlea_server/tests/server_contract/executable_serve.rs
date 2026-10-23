//! Process-level smoke for `--serve`: paced host loop, and companion movement
//! reaching a real TCP client (Owen/Lena executable-entry gate).

#![cfg(unix)]

use std::fs;
use std::io::{Read, Write};
use std::net::TcpStream;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use mornlea_domain::{CompanionId, Identities};
use mornlea_protocol::{
    encode_uvarint, read_frame_ref, ClientHello, ClientPacket, LoginStart, LoginSuccess,
    ProtocolCodec, ServerPacket, State,
};
use mornlea_server::contracts::{
    DiskBackend, OwnedSnapshot, SaveKey, SaveRequest, SaveTicket, SaveUrgency, SaveValue,
    ServerError,
};
use mornlea_server::store::disk::{DiskOptions, DiskStore};
use mornlea_server::transport::memory::MemoryTransport;
use mornlea_storage::{
    CompanionBody, Inventory, Metadata, MetadataChunkPos, PlanStep, PlayerId, StoredCompanionQueue,
    StoredCompanionTask, StoredCompanions, COMPANION_PLAN_STEP_GO_TO, COMPANION_TASK_RUNNING,
    METADATA_CURRENT_VERSION,
};

#[test]
fn serve_mode_runs_max_ticks_and_exits() {
    let bin = env!("CARGO_BIN_EXE_mornlea-server");
    let root = tempfile_world("max-ticks");
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
    assert!(
        started.elapsed() >= Duration::from_millis(100),
        "finished too fast for 20 TPS pacing: {:?}",
        started.elapsed()
    );
}

/// Start the binary with a configured companion that already holds a Running
/// go_to, connect a real TCP client, and require CompanionStates whose
/// position changes (Owen: companion actually moves + state reaches client).
#[test]
fn serve_companion_moves_and_state_reaches_client() {
    let bin = env!("CARGO_BIN_EXE_mornlea-server");
    let root = tempfile_world("companion-client");
    let config_path = root.join("server.json");
    let endpoint = dead_endpoint();
    let mira = uuid_text(1);
    fs::write(
        &config_path,
        format!(
            r#"{{"ai":{{"agentService":{{"endpoint":"{endpoint}","apiKeyEnv":"PATH"}},"companions":[{{"id":"{mira}","name":"Mira"}}],"taskTimeoutMinutes":10}}}}"#
        ),
    )
    .unwrap();
    seed_companion_running_goto(&root);

    let probe = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = probe.local_addr().unwrap().port();
    drop(probe);
    let listen = format!("127.0.0.1:{port}");

    let mut child = Command::new(bin)
        .args([
            "--serve",
            "--world",
            root.to_str().unwrap(),
            "--listen",
            &listen,
            "--config",
            config_path.to_str().unwrap(),
            "--max-ticks",
            "160",
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn mornlea-server --serve with companions");

    // Drain stdout on a helper thread so "serve ready" is observed without a
    // probe TCP connect (a probe would sit in the accept backlog ahead of us).
    let stdout_pipe = child.stdout.take().expect("stdout pipe");
    let ready_flag = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let ready_flag_bg = std::sync::Arc::clone(&ready_flag);
    let stdout_buf = std::sync::Arc::new(std::sync::Mutex::new(String::new()));
    let stdout_buf_bg = std::sync::Arc::clone(&stdout_buf);
    std::thread::spawn(move || {
        let mut reader = stdout_pipe;
        let mut chunk = [0u8; 512];
        loop {
            match reader.read(&mut chunk) {
                Ok(0) => break,
                Ok(n) => {
                    let text = String::from_utf8_lossy(&chunk[..n]);
                    let mut guard = stdout_buf_bg.lock().unwrap();
                    guard.push_str(&text);
                    if guard.contains("serve ready") {
                        ready_flag_bg.store(true, std::sync::atomic::Ordering::SeqCst);
                    }
                }
                Err(_) => break,
            }
        }
    });

    let ready_deadline = Instant::now() + Duration::from_secs(60);
    loop {
        if Instant::now() > ready_deadline {
            let _ = child.kill();
            let captured = stdout_buf.lock().unwrap().clone();
            panic!("server never printed serve ready; stdout={captured}");
        }
        if let Some(status) = child.try_wait().unwrap() {
            let captured = stdout_buf.lock().unwrap().clone();
            let mut stderr = String::new();
            if let Some(mut err) = child.stderr.take() {
                let _ = err.read_to_string(&mut stderr);
            }
            panic!("server exited before ready: {status} stdout={captured} stderr={stderr}");
        }
        if ready_flag.load(std::sync::atomic::Ordering::SeqCst) {
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }

    let mut stream = TcpStream::connect_timeout(&listen.parse().unwrap(), Duration::from_secs(5))
        .expect("connect gameplay listener");
    stream
        .set_read_timeout(Some(Duration::from_millis(200)))
        .unwrap();
    stream
        .set_write_timeout(Some(Duration::from_secs(5)))
        .unwrap();

    handshake_login(&mut stream);

    let mira_id = CompanionId::try_from_bytes(raw_id(1)).unwrap();
    let mut buffer = Vec::new();
    let mut saw_spawn = false;
    let mut positions: Vec<[f32; 3]> = Vec::new();
    let observe_until = Instant::now() + Duration::from_secs(12);
    while Instant::now() < observe_until {
        if let Some(status) = child.try_wait().unwrap() {
            // Drain remaining bytes then finish assertions below.
            let _ = status;
            break;
        }
        let mut chunk = [0u8; 4096];
        match stream.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => buffer.extend_from_slice(&chunk[..n]),
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::sleep(Duration::from_millis(10));
            }
            Err(error) if error.kind() == std::io::ErrorKind::TimedOut => {}
            Err(error) => panic!("client read: {error}"),
        }
        while let Ok(frame) = read_frame_ref(&buffer) {
            let consumed = frame.consumed;
            let packet = ProtocolCodec::new().unwrap().decode_server(
                State::Play,
                frame.packet_id,
                frame.payload,
            );
            buffer.drain(..consumed);
            let Ok(packet) = packet else {
                continue;
            };
            match packet {
                ServerPacket::CompanionSpawn(spawn) if spawn.companion_id == mira_id => {
                    saw_spawn = true;
                    positions.push(spawn.position);
                }
                ServerPacket::CompanionStates(batch) => {
                    for record in batch.states {
                        if record.companion_id == mira_id {
                            positions.push(record.position);
                        }
                    }
                }
                _ => {}
            }
        }
        if saw_spawn && positions_moved(&positions) {
            break;
        }
    }

    let _ = child.kill();
    let _ = child.wait();
    let stdout = stdout_buf.lock().unwrap().clone();
    let mut stderr = String::new();
    if let Some(mut err) = child.stderr.take() {
        let _ = err.read_to_string(&mut stderr);
    }

    assert!(
        saw_spawn,
        "client never saw CompanionSpawn for Mira; positions={positions:?} stdout={stdout} stderr={stderr}"
    );
    assert!(
        positions_moved(&positions),
        "companion position never changed on the wire; positions={positions:?} stdout={stdout} stderr={stderr}"
    );
}

fn positions_moved(positions: &[[f32; 3]]) -> bool {
    let Some(first) = positions.first() else {
        return false;
    };
    positions.iter().any(|p| {
        (p[0] - first[0]).abs() > 0.05
            || (p[1] - first[1]).abs() > 0.05
            || (p[2] - first[2]).abs() > 0.05
    })
}

fn handshake_login(stream: &mut TcpStream) {
    let version = Identities::current().protocol;
    let hello =
        ClientPacket::ClientHello(ClientHello::decode_inbound(&encode_uvarint(version)).unwrap());
    stream
        .write_all(&MemoryTransport::encode_frame(&hello).unwrap())
        .unwrap();
    let frame = read_one_frame(stream, Duration::from_secs(5));
    let packet = decode_server(State::Handshake, &frame);
    assert!(
        matches!(packet, ServerPacket::ServerHello(_)),
        "expected ServerHello, got {packet:?}"
    );

    let player = mornlea_domain::PlayerId::try_from_bytes(raw_id(0x21)).unwrap();
    let start = ClientPacket::LoginStart(
        LoginStart::decode_inbound(&LoginStart::new(player, "Ada", 8).unwrap().encode().unwrap())
            .unwrap(),
    );
    stream
        .write_all(&MemoryTransport::encode_frame(&start).unwrap())
        .unwrap();
    let frame = read_one_frame(stream, Duration::from_secs(10));
    let packet = decode_server(State::Login, &frame);
    assert!(
        matches!(packet, ServerPacket::LoginSuccess(LoginSuccess { .. })),
        "expected LoginSuccess, got {packet:?}"
    );
}

fn decode_server(state: State, frame: &[u8]) -> ServerPacket {
    let wire = read_frame_ref(frame).unwrap();
    ProtocolCodec::new()
        .unwrap()
        .decode_server(state, wire.packet_id, wire.payload)
        .unwrap()
}

fn read_one_frame(stream: &mut TcpStream, bound: Duration) -> Vec<u8> {
    let until = Instant::now() + bound;
    let mut buffer = Vec::new();
    loop {
        assert!(Instant::now() < until, "frame deadline");
        let mut chunk = [0u8; 4096];
        match stream.read(&mut chunk) {
            Ok(0) => panic!("eof before frame"),
            Ok(n) => buffer.extend_from_slice(&chunk[..n]),
            Err(error)
                if error.kind() == std::io::ErrorKind::WouldBlock
                    || error.kind() == std::io::ErrorKind::TimedOut =>
            {
                std::thread::sleep(Duration::from_millis(5));
                continue;
            }
            Err(error) => panic!("read: {error}"),
        }
        if let Ok(frame) = read_frame_ref(&buffer) {
            return buffer[..frame.consumed].to_vec();
        }
    }
}

fn seed_companion_running_goto(world: &PathBuf) {
    let mut disk = DiskStore::open(
        world,
        DiskOptions {
            region_handle_cap: 1,
            create: Metadata {
                format_version: METADATA_CURRENT_VERSION,
                seed: 42,
                spawn_dimension: 0,
                spawn_anchor: MetadataChunkPos { x: 0, z: 0 },
                world_time_ticks: 1000,
                day_phase_offset: 0,
                weather_kind: 0,
                weather_ticks_remaining: 0,
                depths_spawn_anchor: MetadataChunkPos { x: 0, z: 0 },
                depths_seed_salt: 0,
                difficulty: 0,
            },
        },
    )
    .unwrap();
    let body = CompanionBody {
        id: PlayerId::from_bytes(raw_id(1)),
        dimension: 0,
        position: [8.5, 72.0, 8.5],
        yaw: 0.0,
        pitch: 0.0,
        inventory: Inventory::default(),
    };
    let mut generate = || Ok::<_, ServerError>(PlayerId::from_bytes(raw_id(0xa0)));
    let (mut aggregate, _) = mornlea_storage::merge_companions_v5(
        &StoredCompanions::default(),
        &[body],
        Some(&mut generate),
    )
    .unwrap();
    aggregate.queues = vec![StoredCompanionQueue {
        id: PlayerId::from_bytes(raw_id(1)),
        has_current: true,
        current: StoredCompanionTask {
            command: "walk".into(),
            state: COMPANION_TASK_RUNNING,
            plan_steps: vec![PlanStep {
                kind: COMPANION_PLAN_STEP_GO_TO,
                x: 14,
                y: 72,
                z: 8,
                ..Default::default()
            }],
            start_tick: 1,
            deadline_ticks: 50_000,
            ..Default::default()
        },
        pending: Vec::new(),
        summary: String::new(),
    }];
    let save = mornlea_storage::CompanionSave {
        revision: aggregate.revision.max(1),
        agent_namespace_id: aggregate.agent_namespace_id,
        records: aggregate.records,
        lifecycles: aggregate.lifecycles,
        queues: aggregate.queues,
    };
    let bytes = mornlea_storage::companions_encoded_len(&save).unwrap();
    let snapshot = OwnedSnapshot::try_new(
        SaveKey::Companions,
        save.revision,
        bytes,
        SaveUrgency::Autosave,
        SaveValue::Companions(save),
    )
    .unwrap();
    let completion = disk.write(
        SaveTicket::try_from_raw(1).unwrap(),
        SaveRequest {
            snapshots: vec![snapshot],
        },
    );
    assert_eq!(completion.error, None);
    disk.close().unwrap();
}

fn dead_endpoint() -> String {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    format!("http://127.0.0.1:{port}")
}

fn raw_id(tag: u8) -> [u8; 16] {
    let mut bytes = [0u8; 16];
    bytes[0] = tag;
    bytes[6] = 0x40;
    bytes[8] = 0x80;
    bytes
}

fn uuid_text(tag: u8) -> String {
    let hex: String = raw_id(tag).iter().map(|b| format!("{b:02x}")).collect();
    format!(
        "{}-{}-{}-{}-{}",
        &hex[0..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..32]
    )
}

fn tempfile_world(tag: &str) -> PathBuf {
    let mut path = std::env::temp_dir();
    path.push(format!(
        "mornlea-serve-smoke-{tag}-{}-{}",
        std::process::id(),
        Instant::now().elapsed().as_nanos()
    ));
    fs::create_dir_all(&path).unwrap();
    path
}
