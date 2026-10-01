//! Actual background store, borrowed authority and loopback prepared delivery.

use mornlea_domain::{Identities, PlayerId};
use mornlea_protocol::{
    ClientHello, ClientPacket, CommandRejected, LoginStart, LoginSuccess, ProtocolCodec,
    ServerPacket, State, admit_login, encode_uvarint, read_frame_ref,
};
use mornlea_server::contracts::*;
use mornlea_server::core::publication::{EnqueueOutcome, PreparedFrame, PreparedPublicationPort};
use mornlea_server::state::AuthorityState;
use mornlea_server::store::{
    disk::{DiskOptions, DiskStore},
    mailbox::StoreMailbox,
    scheduler::{AutosaveScheduler, SchedulerConfig},
};
use mornlea_server::transport::common::TransportAuthority;
use mornlea_server::transport::{live::LoginDriver, memory::MemoryTransport, tcp::TcpTransport};
use mornlea_storage::{METADATA_CURRENT_VERSION, Metadata, MetadataChunkPos};
use std::{
    fs,
    io::{self, Read, Write},
    net::TcpStream,
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
    thread,
    time::{Duration, Instant},
};

const BOUND: Duration = Duration::from_secs(5);
static ROOT_COUNT: AtomicU64 = AtomicU64::new(0);
struct Root(PathBuf);
impl Root {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "mornlea-prepared-tcp-{}-{}",
            std::process::id(),
            ROOT_COUNT.fetch_add(1, Ordering::Relaxed)
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
fn player(tag: u8) -> PlayerId {
    let mut bytes = [0; 16];
    bytes[0] = tag;
    bytes[6] = 0x40;
    bytes[8] = 0x80;
    PlayerId::try_from_bytes(bytes).unwrap()
}
fn login(tag: u8) -> mornlea_protocol::AdmittedLogin {
    let start = LoginStart::new(player(tag), "Ada", 8).unwrap();
    admit_login(LoginStart::decode_inbound(&start.encode().unwrap()).unwrap()).unwrap()
}
fn options() -> DiskOptions {
    DiskOptions {
        create: Metadata {
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
        },
        region_handle_cap: 1,
    }
}
fn deadline() -> Deadline {
    Deadline::after(Instant::now(), BOUND).unwrap()
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
fn packet(sequence: u64) -> ServerPacket {
    ServerPacket::CommandRejected(CommandRejected::new(sequence, 1).unwrap())
}
fn frame(sequence: u64) -> PreparedFrame {
    PreparedFrame::encode(&mut ProtocolCodec::new().unwrap(), &packet(sequence)).unwrap()
}
fn decode(bytes: &[u8], state: State) -> ServerPacket {
    let wire = read_frame_ref(bytes).unwrap();
    assert_eq!(wire.consumed, bytes.len());
    ProtocolCodec::new()
        .unwrap()
        .decode_server(state, wire.packet_id, wire.payload)
        .unwrap()
}
// Own and explicitly close every actual socket, load alias and background store.
struct Fixture {
    root: Root,
    authority: AuthorityState,
    driver: LoginDriver,
    scheduler: AutosaveScheduler<DiskStore>,
    transport: TcpTransport,
    peer: TcpStream,
    id: ConnectionId,
    clock: StepClock,
    received: Vec<u8>,
    other: SessionKey,
}
impl Fixture {
    fn new() -> Self {
        let root = Root::new();
        let disk = DiskStore::open(&root.0, options()).unwrap();
        let mut authority = AuthorityState::try_new_with_metadata(
            ServerLimits::try_new(8, 4096, 512, 64, 64, 1_048_576).unwrap(),
            disk.metadata().clone(),
        )
        .unwrap();
        let other = authority.admit(login(2), TransportKind::Memory).unwrap();
        let mailbox = StoreMailbox::try_new_background(
            StoreLimits::try_new(2, 16, 3, 3, 3, 1, 8, 4_194_304).unwrap(),
            disk,
        )
        .unwrap();
        let mut scheduler =
            AutosaveScheduler::try_new(SchedulerConfig::default(), mailbox).unwrap();
        let mut driver = LoginDriver::new();
        let clock = StepClock(Instant::now());
        let mut transport = TcpTransport::bind_loopback().unwrap();
        let peer = TcpStream::connect_timeout(&transport.local_addr().unwrap(), BOUND).unwrap();
        peer.set_write_timeout(Some(BOUND)).unwrap();
        let until = Instant::now() + BOUND;
        let id = loop {
            if let Some(id) = transport
                .accept_one(&mut driver.bind(&mut authority, &mut scheduler), &clock)
                .unwrap()
            {
                break id;
            }
            assert!(Instant::now() < until);
            thread::yield_now();
        };
        peer.set_nonblocking(true).unwrap();
        Self {
            root,
            authority,
            driver,
            scheduler,
            transport,
            peer,
            id,
            clock,
            received: Vec::new(),
            other,
        }
    }
    fn send(&mut self, packet: ClientPacket) {
        self.peer.set_nonblocking(false).unwrap();
        self.peer
            .write_all(&MemoryTransport::encode_frame(&packet).unwrap())
            .unwrap();
        self.peer.set_nonblocking(true).unwrap();
        self.transport.pump_in(
            self.id,
            &mut self.driver.bind(&mut self.authority, &mut self.scheduler),
            &self.clock,
        );
    }
    fn receive(&mut self, count: usize) -> Vec<Vec<u8>> {
        let until = Instant::now() + BOUND;
        let mut frames = Vec::new();
        while frames.len() < count {
            self.transport.flush_out(
                self.id,
                &mut self.driver.bind(&mut self.authority, &mut self.scheduler),
            );
            let mut buf = [0; 4096];
            match self.peer.read(&mut buf) {
                Ok(0) => panic!("peer closed before delivery"),
                Ok(n) => self.received.extend_from_slice(&buf[..n]),
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => {}
                Err(error) => panic!("peer read: {error}"),
            }
            while let Ok(wire) = read_frame_ref(&self.received) {
                let consumed = wire.consumed;
                frames.push(self.received.drain(..consumed).collect());
            }
            assert!(Instant::now() < until, "actual socket delivery deadline");
            thread::yield_now();
        }
        assert_eq!(frames.len(), count);
        frames
    }
    fn handoff(&mut self) -> SessionKey {
        self.send(ClientPacket::ClientHello(
            ClientHello::decode_inbound(&encode_uvarint(Identities::current().protocol)).unwrap(),
        ));
        assert!(matches!(
            decode(&self.receive(1)[0], State::Handshake),
            ServerPacket::ServerHello(_)
        ));
        let start = LoginStart::new(player(1), "Ada", 8).unwrap();
        self.send(ClientPacket::LoginStart(
            LoginStart::decode_inbound(&start.encode().unwrap()).unwrap(),
        ));
        let until = Instant::now() + BOUND;
        loop {
            self.scheduler.drive_workers();
            self.transport.pump_in(
                self.id,
                &mut self.driver.bind(&mut self.authority, &mut self.scheduler),
                &self.clock,
            );
            self.transport.poll(
                self.id,
                &mut self.driver.bind(&mut self.authority, &mut self.scheduler),
                &self.clock,
            );
            if let LoginPoll::Ready { session, .. } = self
                .driver
                .bind(&mut self.authority, &mut self.scheduler)
                .poll_login(LoginTicket::try_from_raw(1).unwrap())
            {
                assert_eq!(
                    self.authority.session(session).unwrap().phase,
                    SessionPhase::Prepared
                );
                return session;
            }
            assert!(Instant::now() < until, "actual background login deadline");
            thread::yield_now();
        }
    }
    fn active(&mut self) -> SessionKey {
        let session = self.handoff();
        assert_eq!(
            decode(&self.receive(1)[0], State::Login),
            ServerPacket::LoginSuccess(LoginSuccess::new(player(1), options().create.seed as u64))
        );
        assert_eq!(
            self.authority.session(session).unwrap().phase,
            SessionPhase::Active
        );
        assert_eq!(self.driver.pending(), 0);
        session
    }
    fn transfer(
        &mut self,
        id: ConnectionId,
        session: SessionKey,
        count: usize,
        bytes: usize,
    ) -> io::Result<usize> {
        self.transport.forward_prepared_outbox(
            id,
            session,
            &mut self.driver.bind(&mut self.authority, &mut self.scheduler),
            count,
            bytes,
        )
    }
    fn close(&mut self) {
        self.transport.close(
            self.id,
            CloseReason::PeerGone,
            &mut self.driver.bind(&mut self.authority, &mut self.scheduler),
        );
    }
    fn finish(&mut self) {
        self.close();
        self.driver
            .cancel_pending(&mut self.authority, &mut self.scheduler)
            .unwrap();
        let until = Instant::now() + BOUND;
        loop {
            self.scheduler.drive_workers();
            match self.scheduler.close(deadline()) {
                Ok(()) => break,
                Err(ServerError::InvalidState {
                    phase: ServerPhase::Closing,
                }) if Instant::now() < until => thread::yield_now(),
                Err(error) => panic!("actual store close refused: {error:?}"),
            }
        }
        let mut reopened = DiskStore::open(&self.root.0, options()).unwrap();
        reopened.close().unwrap();
        assert_eq!(self.transport.retained_sockets(), 0);
        assert_eq!(self.driver.pending(), 0);
    }
}
fn with_fixture(test: impl FnOnce(&mut Fixture)) {
    let mut fixture = Fixture::new();
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| test(&mut fixture)));
    fixture.finish();
    if let Err(panic) = result {
        std::panic::resume_unwind(panic);
    }
}
#[test]
fn actual_background_store_tcp_and_memory_canonical_fifo_and_budgets() {
    with_fixture(|f| {
        let session = f.active();
        // Factory execution is off the authority/transport call stack.
        let owners = thread::spawn(|| [frame(1), frame(2), frame(3), frame(4)])
            .join()
            .unwrap();
        for owner in &owners {
            let mut endpoint = f.driver.bind(&mut f.authority, &mut f.scheduler);
            assert_eq!(
                endpoint.enqueue_prepared(session, owner.clone()).unwrap(),
                EnqueueOutcome::Queued
            );
            assert_eq!(
                endpoint.enqueue_prepared(f.other, owner.clone()).unwrap(),
                EnqueueOutcome::Queued
            );
        }
        assert_eq!(f.transfer(f.id, session, 0, 0).unwrap(), 0);
        assert_eq!(f.transfer(f.id, session, 4, 0).unwrap(), 1);
        let first = f.receive(1);
        assert_eq!(first[0], owners[0].as_bytes());
        let exact = owners[1].byte_len() + owners[2].byte_len();
        assert_eq!(f.transfer(f.id, session, 4, exact).unwrap(), 2);
        let middle = f.receive(2);
        assert_eq!(f.transfer(f.id, session, 4, usize::MAX).unwrap(), 1);
        let last = f.receive(1);
        assert_eq!(f.transfer(f.id, session, 4, usize::MAX).unwrap(), 0);
        let memory = MemoryTransport::drain_prepared_session(
            &mut f.driver.bind(&mut f.authority, &mut f.scheduler),
            f.other,
            4,
            usize::MAX,
        )
        .unwrap();
        let wire = first
            .into_iter()
            .chain(middle)
            .chain(last)
            .collect::<Vec<_>>();
        for (index, ((bytes, local), retained)) in wire.iter().zip(&memory).zip(&owners).enumerate()
        {
            assert_eq!(bytes, local.as_bytes());
            assert_eq!(bytes, retained.as_bytes());
            assert_eq!(local.packet_key(), retained.packet_key());
            assert_eq!(local.as_bytes().as_ptr(), retained.as_bytes().as_ptr());
            assert_eq!(decode(bytes, State::Play), packet(index as u64 + 1));
        }
        assert_eq!(
            f.authority.session(session).unwrap().phase,
            SessionPhase::Active
        );
    });
}
fn refusal(f: &mut Fixture, id: ConnectionId, session: SessionKey) {
    let error = f.transfer(id, session, 8, usize::MAX).unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
    assert_eq!(error.to_string(), "connection does not own the session");
}
#[test]
fn actual_identity_refusals_preserve_other_active_fifo_before_and_after_login_close() {
    with_fixture(|f| {
        let retained = [frame(77), frame(78)];
        for owner in &retained {
            f.authority
                .enqueue_prepared(f.other, owner.clone())
                .unwrap();
        }
        refusal(f, f.id, f.other);
        refusal(f, ConnectionId::try_from_raw(999).unwrap(), f.other);
        let prepared = f.handoff();
        refusal(f, f.id, f.other);
        refusal(f, f.id, prepared);
        assert_eq!(
            f.authority.enqueue_prepared(prepared, frame(99)).unwrap(),
            EnqueueOutcome::Closed
        );
        assert!(
            f.authority
                .take_prepared_outbox(prepared, 8, usize::MAX)
                .unwrap()
                .is_empty()
        );
        f.receive(1);
        refusal(f, f.id, f.other);
        f.close();
        refusal(f, f.id, f.other);
        refusal(f, f.id, prepared);
        let held = f
            .authority
            .take_prepared_outbox(f.other, 8, usize::MAX)
            .unwrap();
        assert_eq!(held.len(), 2);
        for (actual, original) in held.iter().zip(&retained) {
            assert_eq!(actual.as_bytes().as_ptr(), original.as_bytes().as_ptr());
            assert_eq!(actual.packet_key(), original.packet_key());
        }
    });
}
#[test]
fn actual_closed_admission_does_not_transfer_an_owner() {
    with_fixture(|f| {
        let session = f.active();
        f.authority.close_outbox(session, CloseReason::PeerGone);
        assert_eq!(
            f.driver
                .bind(&mut f.authority, &mut f.scheduler)
                .enqueue_prepared(session, frame(99))
                .unwrap(),
            EnqueueOutcome::Closed
        );
        assert_eq!(f.transfer(f.id, session, 8, usize::MAX).unwrap(), 0);
        assert!(
            f.authority
                .take_prepared_outbox(session, 8, usize::MAX)
                .unwrap()
                .is_empty()
        );
    });
}
