//! Actual store ownership and borrowed login evidence. Gates release before joins.

use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, mpsc};
use std::thread;
use std::time::{Duration, Instant};

use mornlea_domain::{Identities, PlayerId};
use mornlea_protocol::{ClientHello, ClientPacket, LoginStart, encode_uvarint};
use mornlea_server::contracts::*;
use mornlea_server::state::AuthorityState;
use mornlea_server::store::disk::{DiskOptions, DiskStore};
use mornlea_server::store::mailbox::StoreMailbox;
use mornlea_server::store::scheduler::{AutosaveScheduler, SchedulerConfig};

use mornlea_server::transport::live::LoginDriver;
use mornlea_server::transport::memory::MemoryTransport;
use mornlea_storage::{
    Inventory, ItemStack, METADATA_CURRENT_VERSION, Metadata, MetadataChunkPos, PlayerLocation,
    PlayerSave,
};

const BOUND: Duration = Duration::from_secs(5);
static ROOT_COUNTER: AtomicU64 = AtomicU64::new(0);
struct Root(PathBuf);
impl Root {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "mornlea-live-login-{}-{}",
            std::process::id(),
            ROOT_COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        Self(path)
    }
}
impl Drop for Root {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn deadline() -> Deadline {
    Deadline::after(Instant::now(), BOUND).unwrap()
}
fn limits() -> ServerLimits {
    ServerLimits::try_new(8, 4096, 512, 64, 64, 1_048_576).unwrap()
}
fn player(tag: u8) -> PlayerId {
    let mut bytes = [0; 16];
    bytes[0] = tag;
    bytes[6] = 0x40;
    bytes[8] = 0x80;
    PlayerId::try_from_bytes(bytes).unwrap()
}
fn metadata() -> Metadata {
    Metadata {
        format_version: METADATA_CURRENT_VERSION,
        seed: -57,
        spawn_dimension: 0,
        spawn_anchor: MetadataChunkPos { x: 0, z: 0 },
        world_time_ticks: 1200,
        day_phase_offset: 0,
        weather_kind: 0,
        weather_ticks_remaining: 200,
        depths_spawn_anchor: MetadataChunkPos { x: 0, z: 0 },
        depths_seed_salt: 1,
        difficulty: 0,
    }
}
fn options() -> DiskOptions {
    DiskOptions {
        create: metadata(),
        region_handle_cap: 1,
    }
}
fn saved_player() -> PlayerSave {
    PlayerSave {
        player_id: mornlea_storage::PlayerId::from_bytes(player(1).bytes()),
        revision: 9,
        display_name: "Ada".to_owned(),
        current: PlayerLocation {
            dimension: 0,
            position: [10.5, 65.0, -3.25],
        },
        yaw: 1.2,
        pitch: 0.3,
        safe: None,
        inventory: saved_inventory(),
        health: 15,
        hunger: 17,
        saturation_milli: 9000,
        exhaustion_milli: 250,
        respawn_present: false,
        respawn_position: [0.0; 3],
        respawn_dimension: 0,
        armor: [ItemStack::default(); 4],
    }
}
fn seed(root: &Root) {
    let mut disk = DiskStore::open(&root.0, options()).unwrap();
    let save = saved_player();
    let completion = disk.write(
        SaveTicket::try_from_raw(1).unwrap(),
        SaveRequest {
            snapshots: vec![
                OwnedSnapshot::try_new(
                    SaveKey::Player(player(1)),
                    save.revision,
                    1,
                    SaveUrgency::Autosave,
                    SaveValue::Player(save),
                )
                .unwrap(),
            ],
        },
    );
    assert!(completion.error.is_none());
    assert_eq!(completion.committed, vec![(SaveKey::Player(player(1)), 9)]);
    disk.close().unwrap();
}
struct StepClock(Instant);
impl Clock for StepClock {
    fn monotonic(&self) -> Instant {
        self.0
    }
    fn unix_ms(&self) -> i64 {
        1000
    }
}
fn hello() -> Vec<u8> {
    MemoryTransport::encode_frame(&ClientPacket::ClientHello(
        ClientHello::decode_inbound(&encode_uvarint(Identities::current().protocol)).unwrap(),
    ))
    .unwrap()
}
fn login(tag: u8) -> Vec<u8> {
    let start = LoginStart::new(player(tag), "Ada", 8).unwrap();
    MemoryTransport::encode_frame(&ClientPacket::LoginStart(
        LoginStart::decode_inbound(&start.encode().unwrap()).unwrap(),
    ))
    .unwrap()
}
#[derive(Clone)]
struct Gate {
    entered: mpsc::SyncSender<()>,
    opened: Arc<(Mutex<bool>, Condvar)>,
}
struct Release(Arc<(Mutex<bool>, Condvar)>);
impl Release {
    fn open(&self) {
        let (l, c) = &*self.0;
        *l.lock().unwrap() = true;
        c.notify_all();
    }
}
impl Drop for Release {
    fn drop(&mut self) {
        self.open();
    }
}
fn gate() -> (Gate, mpsc::Receiver<()>, Release) {
    let (tx, rx) = mpsc::sync_channel(1);
    let opened = Arc::new((Mutex::new(false), Condvar::new()));
    (
        Gate {
            entered: tx,
            opened: opened.clone(),
        },
        rx,
        Release(opened),
    )
}
struct GatedDisk {
    disk: DiskStore,
    gate: Option<Gate>,
}
impl DiskBackend for GatedDisk {
    fn load(&mut self, key: SaveKey) -> Result<LoadedValue, ServerError> {
        if let Some(gate) = self.gate.take() {
            gate.entered.send(()).unwrap();
            let (l, c) = &*gate.opened;
            drop(c.wait_while(l.lock().unwrap(), |open| !*open).unwrap());
        }
        self.disk.load(key)
    }
    fn write(&mut self, t: SaveTicket, r: SaveRequest) -> SaveCompletion {
        self.disk.write(t, r)
    }
    fn sync(&mut self) -> Result<(), ServerError> {
        self.disk.sync()
    }
    fn close(&mut self) -> Result<(), ServerError> {
        self.disk.close()
    }
}

#[test]
fn held_actual_player_read_returns_login_pump_before_release() {
    held_read(TransportKind::Memory);
}
#[test]
fn held_actual_player_read_returns_tcp_login_pump_before_release() {
    held_read(TransportKind::Tcp);
}
fn held_read(kind: TransportKind) {
    let root = Root::new();
    seed(&root);
    let (g, entered, release) = gate();
    let disk = DiskStore::open(&root.0, options()).unwrap();
    let state = AuthorityState::try_new_with_metadata(limits(), disk.metadata().clone()).unwrap();
    let (tx, rx) = mpsc::sync_channel(1);
    let owner = thread::spawn(move || {
        let mut state = state;
        let mut driver = LoginDriver::new();
        let mut scheduler = scheduler(GatedDisk {
            disk,
            gate: Some(g),
        });
        let clock = StepClock(Instant::now());
        let mut transport = Wire::open(kind, &mut driver.bind(&mut state, &mut scheduler), &clock);
        begin_wire(
            &mut transport,
            &mut driver.bind(&mut state, &mut scheduler),
            &clock,
            1,
        );
        scheduler.drive_workers();
        transport.poll(&mut driver.bind(&mut state, &mut scheduler), &clock);
        tx.send((driver, state, scheduler, transport)).unwrap();
    });
    let entered_result = entered.recv_timeout(BOUND);
    let observed = rx.recv_timeout(BOUND);
    let returned = observed.is_ok();
    release.open();
    let result = observed.or_else(|_| rx.recv_timeout(BOUND));
    let joined = owner.join();
    assert!(entered_result.is_ok());
    assert!(joined.is_ok());
    let (mut driver, mut state, mut scheduler, _) = result.unwrap();
    assert_eq!(driver.pending(), 1);
    assert_eq!(driver.cancel_pending(&mut state, &mut scheduler), Ok(1));
    close_scheduler(&mut scheduler);
    let mut reopened = DiskStore::open(&root.0, options()).unwrap();
    reopened.close().unwrap();
    assert!(
        returned,
        "login pump must return while the actual player read is held"
    );
}

fn store_limits() -> StoreLimits {
    StoreLimits::try_new(2, 16, 3, 3, 3, 1, 8, 4_194_304).unwrap()
}
fn scheduler<B: DiskBackend + Send + 'static>(backend: B) -> AutosaveScheduler<B> {
    AutosaveScheduler::try_new(
        SchedulerConfig::default(),
        StoreMailbox::try_new_background(store_limits(), backend).unwrap(),
    )
    .unwrap()
}

fn saved_inventory() -> Inventory {
    let mut inventory = Inventory::default();
    inventory.hotbar.selected = 3;
    inventory.hotbar.slots[3] = ItemStack {
        item: 1,
        count: 17,
        durability: 0,
    };
    inventory.backpack[26] = ItemStack {
        item: 10,
        count: 1,
        durability: 80,
    };
    inventory
}
fn close_scheduler<B: DiskBackend>(scheduler: &mut AutosaveScheduler<B>) {
    let until = Instant::now() + BOUND;
    loop {
        scheduler.drive_workers();
        match scheduler.close(deadline()) {
            Ok(()) => return,
            Err(ServerError::InvalidState {
                phase: ServerPhase::Closing,
            }) => {
                assert!(Instant::now() < until);
                thread::yield_now();
            }
            Err(error) => panic!("store close refused: {error:?}"),
        }
    }
}

use mornlea_protocol::{
    CloseContainer, LOGIN_PLAYER_DATA_CORRUPT, LoginSuccess, ProtocolCodec, ServerPacket, State,
    read_frame_ref, write_frame,
};
use mornlea_server::transport::common::{TransportAuthority, TransportSessionPort};
use mornlea_server::transport::tcp::TcpTransport;
use std::io::{self, Read, Write};
use std::net::TcpStream;

/// The test peer only moves real protocol frames; authority stays per-call borrowed.
enum Wire {
    Memory {
        transport: MemoryTransport,
        id: ConnectionId,
    },
    Tcp {
        transport: TcpTransport,
        id: ConnectionId,
        peer: TcpStream,
        received: Vec<u8>,
    },
}
impl Wire {
    fn open(kind: TransportKind, endpoint: &mut dyn TransportAuthority, clock: &StepClock) -> Self {
        match kind {
            TransportKind::Memory => {
                let mut transport = MemoryTransport::new();
                let id = transport.connect(clock.monotonic()).unwrap();
                Self::Memory { transport, id }
            }
            TransportKind::Tcp => {
                let mut transport = TcpTransport::bind_loopback().unwrap();
                let peer =
                    TcpStream::connect_timeout(&transport.local_addr().unwrap(), BOUND).unwrap();
                peer.set_nodelay(true).unwrap();
                peer.set_write_timeout(Some(BOUND)).unwrap();
                let until = Instant::now() + BOUND;
                let id = loop {
                    if let Some(id) = transport.accept_one(endpoint, clock).unwrap() {
                        break id;
                    }
                    assert!(Instant::now() < until);
                    thread::yield_now();
                };
                peer.set_nonblocking(true).unwrap();
                Self::Tcp {
                    transport,
                    id,
                    peer,
                    received: Vec::new(),
                }
            }
        }
    }
    fn send(&mut self, bytes: Vec<u8>, endpoint: &mut dyn TransportAuthority, clock: &StepClock) {
        match self {
            Self::Memory { transport, id } => {
                transport.send(*id, bytes, endpoint, clock);
            }
            Self::Tcp {
                transport,
                id,
                peer,
                ..
            } => {
                // Test-peer writes are small and deadline bounded, outside authority.
                peer.set_nonblocking(false).unwrap();
                peer.write_all(&bytes).unwrap();
                peer.set_nonblocking(true).unwrap();
                transport.pump_in(*id, endpoint, clock);
            }
        }
    }
    fn poll(
        &mut self,
        endpoint: &mut dyn TransportAuthority,
        clock: &StepClock,
    ) -> ConnectionProgress {
        match self {
            Self::Memory { transport, id } => transport.poll(*id, endpoint, clock),
            Self::Tcp { transport, id, .. } => {
                transport.pump_in(*id, endpoint, clock);
                transport.poll(*id, endpoint, clock)
            }
        }
    }
    fn flush(&mut self, endpoint: &mut dyn TransportAuthority) {
        if let Self::Tcp { transport, id, .. } = self {
            transport.flush_out(*id, endpoint);
        }
    }
    fn receive(&mut self) -> Vec<Vec<u8>> {
        match self {
            Self::Memory { transport, id } => transport.receive(*id, 64, 1 << 20),
            Self::Tcp { peer, received, .. } => {
                let mut buf = [0; 4096];
                for _ in 0..16 {
                    match peer.read(&mut buf) {
                        Ok(0) => break,
                        Ok(n) => received.extend_from_slice(&buf[..n]),
                        Err(error) if error.kind() == io::ErrorKind::WouldBlock => break,
                        Err(error) => panic!("peer read: {error}"),
                    }
                }
                let mut frames = Vec::new();
                while let Ok(frame) = read_frame_ref(received) {
                    let consumed = frame.consumed;
                    frames.push(received.drain(..consumed).collect());
                }
                frames
            }
        }
    }
    fn acknowledge(&mut self, count: usize, endpoint: &mut dyn TransportAuthority) {
        match self {
            Self::Memory { transport, id } => {
                transport.acknowledge(*id, count, endpoint);
            }
            Self::Tcp { .. } => self.flush(endpoint),
        }
    }
    fn drain(
        &mut self,
        endpoint: &mut dyn TransportAuthority,
        session: SessionKey,
    ) -> Vec<Vec<u8>> {
        match self {
            Self::Memory { .. } => {
                MemoryTransport::drain_session(endpoint, session, 64, 1 << 20).unwrap()
            }
            Self::Tcp { transport, id, .. } => {
                transport
                    .forward_outbox(*id, session, endpoint, 64, 1 << 20)
                    .unwrap();
                Vec::new()
            }
        }
    }
    fn close(&mut self, endpoint: &mut dyn TransportAuthority) {
        match self {
            Self::Memory { transport, id } => transport.close(*id, CloseReason::PeerGone, endpoint),
            Self::Tcp { transport, id, .. } => {
                transport.close(*id, CloseReason::PeerGone, endpoint)
            }
        }
    }
}
fn decode(frame: &[u8]) -> ServerPacket {
    let parsed = read_frame_ref(frame).unwrap();
    ProtocolCodec::new()
        .unwrap()
        .decode_server(State::Login, parsed.packet_id, parsed.payload)
        .unwrap()
}
fn server_frame(packet: &ServerPacket) -> Vec<u8> {
    let mut bytes = vec![0; 4096];
    let written = ProtocolCodec::new()
        .unwrap()
        .encode_server_into(packet, &mut bytes)
        .unwrap();
    write_frame(packet.key().id, &bytes[..written]).unwrap()
}
fn wait_frames(
    wire: &mut Wire,
    expected: usize,
    endpoint: &mut dyn TransportAuthority,
) -> Vec<Vec<u8>> {
    let until = Instant::now() + BOUND;
    let mut frames = Vec::new();
    while frames.len() < expected {
        wire.flush(endpoint);
        frames.extend(wire.receive());
        assert!(Instant::now() < until, "peer frame delivery stalled");
        thread::yield_now();
    }
    assert_eq!(frames.len(), expected);
    frames
}
fn begin_wire(wire: &mut Wire, endpoint: &mut dyn TransportAuthority, clock: &StepClock, tag: u8) {
    wire.send(hello(), endpoint, clock);
    let frames = wait_frames(wire, 1, endpoint);
    assert_eq!(
        frames,
        vec![server_frame(&ServerPacket::ServerHello(
            mornlea_protocol::ServerHello::new(Identities::current().protocol).unwrap()
        ))]
    );
    wire.acknowledge(1, endpoint);
    wire.send(login(tag), endpoint, clock);
}
fn wait_ready<B: DiskBackend>(
    wire: &mut Wire,
    driver: &mut LoginDriver,
    state: &mut AuthorityState,
    scheduler: &mut AutosaveScheduler<B>,
    clock: &StepClock,
    ticket: LoginTicket,
) -> (SessionKey, ServerPacket) {
    let until = Instant::now() + BOUND;
    loop {
        scheduler.drive_workers();
        wire.poll(&mut driver.bind(state, scheduler), clock);
        match driver.bind(state, scheduler).poll_login(ticket) {
            LoginPoll::Ready { session, success } => {
                wire.poll(&mut driver.bind(state, scheduler), clock);
                return (session, success);
            }
            LoginPoll::Failed { error, .. } => panic!("login load failed: {error:?}"),
            LoginPoll::Pending => {
                assert!(Instant::now() < until);
                thread::yield_now();
            }
        }
    }
}
fn play(sequence: u64) -> Vec<u8> {
    MemoryTransport::encode_frame(&ClientPacket::CloseContainer(CloseContainer::new(sequence)))
        .unwrap()
}
fn wait_arrival<B: DiskBackend>(
    wire: &mut Wire,
    driver: &mut LoginDriver,
    state: &mut AuthorityState,
    scheduler: &mut AutosaveScheduler<B>,
    clock: &StepClock,
    session: SessionKey,
    count: u64,
) {
    let until = Instant::now() + BOUND;
    while state.session(session).unwrap().next_arrival < count {
        wire.poll(&mut driver.bind(state, scheduler), clock);
        assert!(Instant::now() < until);
        thread::yield_now();
    }
}
fn delivered_publication<B: DiskBackend>(
    wire: &mut Wire,
    driver: &mut LoginDriver,
    state: &mut AuthorityState,
    scheduler: &mut AutosaveScheduler<B>,
    session: SessionKey,
    expected: usize,
) -> Vec<Vec<u8>> {
    let mut endpoint = driver.bind(state, scheduler);
    let frames = wire.drain(&mut endpoint, session);
    if matches!(wire, Wire::Tcp { .. }) {
        wait_frames(wire, expected, &mut endpoint)
    } else {
        assert_eq!(frames.len(), expected);
        frames
    }
}

#[derive(Debug, PartialEq)]
struct Transcript {
    sessions: Vec<u64>,
    receipts: Vec<SubmissionReceipt>,
    counters: Vec<(usize, usize)>,
    frames: Vec<Vec<Vec<u8>>>,
}
fn real_transcript(kind: TransportKind) -> Transcript {
    let root = Root::new();
    seed(&root);
    let disk = DiskStore::open(&root.0, options()).unwrap();
    let mut state =
        AuthorityState::try_new_with_metadata(limits(), disk.metadata().clone()).unwrap();
    let mut scheduler = scheduler(disk);
    let mut driver = LoginDriver::new();
    let clock = StepClock(Instant::now());
    let mut peers = Vec::new();
    let mut sessions = Vec::new();
    let mut transcript = Transcript {
        sessions: Vec::new(),
        receipts: Vec::new(),
        counters: Vec::new(),
        frames: Vec::new(),
    };
    for tag in 1..=2 {
        let mut wire = Wire::open(kind, &mut driver.bind(&mut state, &mut scheduler), &clock);
        begin_wire(
            &mut wire,
            &mut driver.bind(&mut state, &mut scheduler),
            &clock,
            tag,
        );
        let ticket = LoginTicket::try_from_raw(tag as u64).unwrap();
        let (session, success) = wait_ready(
            &mut wire,
            &mut driver,
            &mut state,
            &mut scheduler,
            &clock,
            ticket,
        );
        assert_eq!(
            success,
            ServerPacket::LoginSuccess(LoginSuccess::new(player(tag), metadata().seed as u64))
        );
        assert_eq!(
            state.session(session).unwrap().phase,
            SessionPhase::Prepared
        );
        assert!(
            driver
                .bind(&mut state, &mut scheduler)
                .submit(
                    session,
                    mornlea_protocol::PlayIntent::try_from(ClientPacket::CloseContainer(
                        CloseContainer::new(1)
                    ))
                    .unwrap()
                )
                .is_err()
        );
        assert!(state.residents().actors.is_empty());
        let success_bytes = wait_frames(&mut wire, 1, &mut driver.bind(&mut state, &mut scheduler));
        assert_eq!(success_bytes, vec![server_frame(&success)]);
        wire.acknowledge(1, &mut driver.bind(&mut state, &mut scheduler));
        assert_eq!(state.session(session).unwrap().phase, SessionPhase::Active);
        assert_eq!(driver.pending(), 0);
        wire.send(
            play(1),
            &mut driver.bind(&mut state, &mut scheduler),
            &clock,
        );
        wait_arrival(
            &mut wire,
            &mut driver,
            &mut state,
            &mut scheduler,
            &clock,
            session,
            1,
        );
        let receipt = driver
            .bind(&mut state, &mut scheduler)
            .submit(
                session,
                mornlea_protocol::PlayIntent::try_from(ClientPacket::CloseContainer(
                    CloseContainer::new(2),
                ))
                .unwrap(),
            )
            .unwrap();
        assert_eq!(
            receipt,
            SubmissionReceipt::QueuedForTick {
                tick: 0,
                arrival_index: 1
            }
        );
        transcript.receipts.push(receipt);
        transcript.sessions.push(session.get());
        sessions.push(session);
        peers.push(wire);
    }
    let publication = state.advance_tick(TickBudget::full()).unwrap();
    assert_eq!(publication.counters.commands, 4);
    assert_eq!(publication.counters.stale, 0);
    let residents = state.residents();
    for (index, session) in sessions.iter().enumerate() {
        let actor = residents
            .actors
            .iter()
            .find(|actor| actor.key == ActorKey::Player(*session))
            .unwrap();
        let ActorBody::Player(body) = &actor.body else {
            panic!("player body")
        };
        if index == 0 {
            assert_eq!(body.inventory, saved_inventory());
            assert_eq!((actor.look.yaw(), actor.look.pitch()), (1.2, 0.3));
            assert_eq!(body.current.position, saved_player().current.position);
            assert_eq!(
                actor.motion.position().get(),
                saved_player().current.position
            );
        } else {
            assert_eq!(body.inventory, Inventory::default());
            assert_eq!(body.current.position, [0.0, 64.0, 0.0]);
            assert_eq!(actor.motion.position().get(), [0.0, 64.0, 0.0]);
            assert_eq!((actor.look.yaw(), actor.look.pitch()), (0.0, 0.0));
        }
    }
    transcript
        .counters
        .push((publication.counters.commands, publication.counters.stale));
    for (wire, session) in peers.iter_mut().zip(&sessions) {
        let expected = publication
            .events
            .iter()
            .filter(|event| {
                matches!(event.recipient(), mornlea_domain::EventRecipient::Broadcast)
                    || event.recipient() == mornlea_domain::EventRecipient::Session(session.get())
            })
            .count()
            + publication
                .control
                .iter()
                .filter(|reply| reply.session == *session)
                .count();
        transcript.frames.push(delivered_publication(
            wire,
            &mut driver,
            &mut state,
            &mut scheduler,
            *session,
            expected,
        ));
    }
    peers[0].send(
        play(1),
        &mut driver.bind(&mut state, &mut scheduler),
        &clock,
    );
    wait_arrival(
        &mut peers[0],
        &mut driver,
        &mut state,
        &mut scheduler,
        &clock,
        sessions[0],
        3,
    );
    let publication = state.advance_tick(TickBudget::full()).unwrap();
    assert_eq!(publication.counters.stale, 1);
    transcript
        .counters
        .push((publication.counters.commands, publication.counters.stale));
    for (wire, session) in peers.iter_mut().zip(&sessions) {
        let expected = publication
            .events
            .iter()
            .filter(|event| {
                matches!(event.recipient(), mornlea_domain::EventRecipient::Broadcast)
                    || event.recipient() == mornlea_domain::EventRecipient::Session(session.get())
            })
            .count()
            + publication
                .control
                .iter()
                .filter(|reply| reply.session == *session)
                .count();
        transcript.frames.push(delivered_publication(
            wire,
            &mut driver,
            &mut state,
            &mut scheduler,
            *session,
            expected,
        ));
        wire.close(&mut driver.bind(&mut state, &mut scheduler));
    }
    assert_eq!(driver.cancel_pending(&mut state, &mut scheduler), Ok(0));
    close_scheduler(&mut scheduler);
    let mut reopened = DiskStore::open(&root.0, options()).unwrap();
    reopened.close().unwrap();
    transcript
}
#[test]
fn actual_saved_and_missing_players_share_memory_tcp_login_tick_and_bytes() {
    assert_eq!(
        real_transcript(TransportKind::Memory),
        real_transcript(TransportKind::Tcp)
    );
}

#[test]
fn actual_corrupt_and_future_players_reject_across_memory_and_tcp() {
    for kind in [TransportKind::Memory, TransportKind::Tcp] {
        for failure in [StorageFailure::Corrupt, StorageFailure::FutureVersion] {
            let root = Root::new();
            seed(&root);
            let path = root
                .0
                .join("players/01000000-0000-4000-8000-000000000000.player");
            let mut bytes = fs::read(&path).unwrap();
            match failure {
                StorageFailure::Corrupt => bytes[0] ^= 0xff,
                StorageFailure::FutureVersion => {
                    bytes[8..12].copy_from_slice(&u32::MAX.to_le_bytes())
                }
                _ => unreachable!(),
            }
            fs::write(&path, &bytes).unwrap();
            let disk = DiskStore::open(&root.0, options()).unwrap();
            let mut state =
                AuthorityState::try_new_with_metadata(limits(), disk.metadata().clone()).unwrap();
            let mut scheduler = scheduler(disk);
            let mut driver = LoginDriver::new();
            let clock = StepClock(Instant::now());
            let mut wire = Wire::open(kind, &mut driver.bind(&mut state, &mut scheduler), &clock);
            begin_wire(
                &mut wire,
                &mut driver.bind(&mut state, &mut scheduler),
                &clock,
                1,
            );
            let until = Instant::now() + BOUND;
            let progress = loop {
                scheduler.drive_workers();
                let progress = wire.poll(&mut driver.bind(&mut state, &mut scheduler), &clock);
                if matches!(progress, ConnectionProgress::Closed { .. }) {
                    break progress;
                }
                assert!(Instant::now() < until);
                thread::yield_now();
            };
            assert!(
                matches!(progress,ConnectionProgress::Closed { class:Some(ServerError::Storage { family:"player",kind:observed }),.. } if observed == failure)
            );
            let frames = wait_frames(&mut wire, 1, &mut driver.bind(&mut state, &mut scheduler));
            let ServerPacket::LoginReject(reject) = decode(&frames[0]) else {
                panic!("expected login rejection")
            };
            assert_eq!(reject.code, LOGIN_PLAYER_DATA_CORRUPT);
            assert_eq!(driver.pending(), 0);
            state.advance_tick(TickBudget::full()).unwrap();
            assert!(state.residents().actors.is_empty());
            assert_eq!(fs::read(&path).unwrap(), bytes);
            assert_eq!(driver.cancel_pending(&mut state, &mut scheduler), Ok(0));
            close_scheduler(&mut scheduler);
            let mut reopened = DiskStore::open(&root.0, options()).unwrap();
            reopened.close().unwrap();
        }
    }
}
#[test]
fn actual_handoff_timeout_cancels_before_activation_across_transports() {
    for kind in [TransportKind::Memory, TransportKind::Tcp] {
        let root = Root::new();
        seed(&root);
        let disk = DiskStore::open(&root.0, options()).unwrap();
        let mut state =
            AuthorityState::try_new_with_metadata(limits(), disk.metadata().clone()).unwrap();
        let mut scheduler = scheduler(disk);
        let mut driver = LoginDriver::new();
        let mut clock = StepClock(Instant::now());
        let mut wire = Wire::open(kind, &mut driver.bind(&mut state, &mut scheduler), &clock);
        begin_wire(
            &mut wire,
            &mut driver.bind(&mut state, &mut scheduler),
            &clock,
            1,
        );
        let (session, _) = wait_ready(
            &mut wire,
            &mut driver,
            &mut state,
            &mut scheduler,
            &clock,
            LoginTicket::try_from_raw(1).unwrap(),
        );
        assert_eq!(
            state.session(session).unwrap().phase,
            SessionPhase::Prepared
        );
        assert_eq!(driver.pending(), 1);
        clock.0 += Duration::from_secs(11);
        let progress = wire.poll(&mut driver.bind(&mut state, &mut scheduler), &clock);
        assert!(matches!(
            progress,
            ConnectionProgress::Closed {
                class: Some(ServerError::Timeout {
                    operation: Operation::Transport
                }),
                ..
            }
        ));
        assert_eq!(driver.pending(), 0);
        assert_eq!(state.session(session).unwrap().phase, SessionPhase::Retired);
        wire.acknowledge(2, &mut driver.bind(&mut state, &mut scheduler));
        assert_eq!(state.session(session).unwrap().phase, SessionPhase::Retired);
        state.advance_tick(TickBudget::full()).unwrap();
        assert!(state.residents().actors.is_empty());
        assert_eq!(driver.cancel_pending(&mut state, &mut scheduler), Ok(0));
        close_scheduler(&mut scheduler);
    }
}
#[test]
fn actual_reconnects_release_committed_driver_history_across_transports() {
    for kind in [TransportKind::Memory, TransportKind::Tcp] {
        let root = Root::new();
        seed(&root);
        let disk = DiskStore::open(&root.0, options()).unwrap();
        let mut state =
            AuthorityState::try_new_with_metadata(limits(), disk.metadata().clone()).unwrap();
        let mut scheduler = scheduler(disk);
        let mut driver = LoginDriver::new();
        let clock = StepClock(Instant::now());
        for index in 1..=40 {
            let mut wire = Wire::open(kind, &mut driver.bind(&mut state, &mut scheduler), &clock);
            begin_wire(
                &mut wire,
                &mut driver.bind(&mut state, &mut scheduler),
                &clock,
                1,
            );
            let (session, success) = wait_ready(
                &mut wire,
                &mut driver,
                &mut state,
                &mut scheduler,
                &clock,
                LoginTicket::try_from_raw(index).unwrap(),
            );
            let bytes = wait_frames(&mut wire, 1, &mut driver.bind(&mut state, &mut scheduler));
            assert_eq!(bytes, vec![server_frame(&success)]);
            wire.acknowledge(1, &mut driver.bind(&mut state, &mut scheduler));
            assert_eq!(driver.pending(), 0);
            assert_eq!(state.session(session).unwrap().phase, SessionPhase::Active);
            wire.close(&mut driver.bind(&mut state, &mut scheduler));
            assert_eq!(driver.pending(), 0);
            assert_eq!(state.session(session).unwrap().phase, SessionPhase::Retired);
        }
        assert_eq!(driver.cancel_pending(&mut state, &mut scheduler), Ok(0));
        close_scheduler(&mut scheduler);
        let mut reopened = DiskStore::open(&root.0, options()).unwrap();
        reopened.close().unwrap();
    }
}
#[test]
fn scheduler_delegates_chunk_loads_to_same_actual_owner() {
    let root = Root::new();
    let disk = DiskStore::open(&root.0, options()).unwrap();
    let mut scheduler = scheduler(disk);
    let key = ChunkKey {
        dimension: mornlea_domain::Dimension::OVERWORLD,
        pos: mornlea_domain::ChunkPos::new(0, 0),
    };
    let request = scheduler.start_chunk(key, 3, deadline()).unwrap();
    let until = Instant::now() + BOUND;
    loop {
        scheduler.drive_workers();
        match scheduler.poll_chunk(request) {
            ChunkLoadPoll::Pending => {
                assert!(Instant::now() < until);
                thread::yield_now();
            }
            ChunkLoadPoll::Loaded(None) => break,
            _ => panic!("missing actual chunk must stay typed absence"),
        }
    }
    assert!(matches!(
        scheduler.poll_chunk(request),
        ChunkLoadPoll::Pending
    ));
    let request = scheduler.start_chunk(key, 4, deadline()).unwrap();
    scheduler.cancel_chunk(request).unwrap();
    close_scheduler(&mut scheduler);
    let mut reopened = DiskStore::open(&root.0, options()).unwrap();
    reopened.close().unwrap();
}

/// Explicit failure injection executes on the real background owner's thread.
struct FailingLoad {
    disk: DiskStore,
    error: Option<ServerError>,
    panic_once: bool,
}
impl DiskBackend for FailingLoad {
    fn load(&mut self, key: SaveKey) -> Result<LoadedValue, ServerError> {
        if std::mem::take(&mut self.panic_once) {
            panic!("explicit login load panic")
        }
        if let Some(error) = self.error.take() {
            return Err(error);
        }
        self.disk.load(key)
    }
    fn write(&mut self, ticket: SaveTicket, request: SaveRequest) -> SaveCompletion {
        self.disk.write(ticket, request)
    }
    fn sync(&mut self) -> Result<(), ServerError> {
        self.disk.sync()
    }
    fn close(&mut self) -> Result<(), ServerError> {
        self.disk.close()
    }
}
#[test]
fn background_owner_failures_keep_exact_error_and_wire_class() {
    for kind in [TransportKind::Memory, TransportKind::Tcp] {
        let io = ServerError::Io {
            operation: Operation::Load,
            kind: io::ErrorKind::PermissionDenied,
        };
        let panic = ServerError::Internal {
            invariant: "store load panic",
        };
        for error in [io, panic] {
            let root = Root::new();
            seed(&root);
            let disk = DiskStore::open(&root.0, options()).unwrap();
            let mut state =
                AuthorityState::try_new_with_metadata(limits(), disk.metadata().clone()).unwrap();
            let mut scheduler = scheduler(FailingLoad {
                disk,
                error: if error == io { Some(io) } else { None },
                panic_once: error == panic,
            });
            let mut driver = LoginDriver::new();
            let clock = StepClock(Instant::now());
            let mut wire = Wire::open(kind, &mut driver.bind(&mut state, &mut scheduler), &clock);
            begin_wire(
                &mut wire,
                &mut driver.bind(&mut state, &mut scheduler),
                &clock,
                1,
            );
            let until = Instant::now() + BOUND;
            let progress = loop {
                scheduler.drive_workers();
                let progress = wire.poll(&mut driver.bind(&mut state, &mut scheduler), &clock);
                if matches!(progress, ConnectionProgress::Closed { .. }) {
                    break progress;
                }
                assert!(Instant::now() < until);
                thread::yield_now();
            };
            assert!(
                matches!(progress,ConnectionProgress::Closed { class:Some(observed),.. } if observed == error)
            );
            let frames = wait_frames(&mut wire, 1, &mut driver.bind(&mut state, &mut scheduler));
            let ServerPacket::LoginReject(reject) = decode(&frames[0]) else {
                panic!("login failure")
            };
            assert_eq!(
                reject.code,
                if error == io {
                    mornlea_protocol::LOGIN_STORE_UNAVAILABLE
                } else {
                    mornlea_protocol::LOGIN_INTERNAL_ERROR
                }
            );
            assert_eq!(driver.pending(), 0);
            state.advance_tick(TickBudget::full()).unwrap();
            assert!(state.residents().actors.is_empty());
            assert_eq!(driver.cancel_pending(&mut state, &mut scheduler), Ok(0));
            close_scheduler(&mut scheduler);
            let mut reopened = DiskStore::open(&root.0, options()).unwrap();
            reopened.close().unwrap();
        }
    }
}
