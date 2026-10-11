//! Real authority FIFO ownership and actual borrowed Memory delivery.

use mornlea_domain::{
    CommandRejection, Event, EventRecipient, Identities, PlayerId, RejectReason, RoutedEvent,
};
use mornlea_protocol::{
    ClientHello, ClientPacket, CommandRejected, LoginStart, LoginSuccess, ProtocolCodec,
    ServerPacket, State, admit_login, encode_uvarint, read_frame_ref,
};
use mornlea_server::contracts::*;
use mornlea_server::core::publication::{EnqueueOutcome, PreparedFrame, PreparedPublicationPort};
use mornlea_server::state::AuthorityState;
use mornlea_server::store::disk::{DiskOptions, DiskStore};
use mornlea_server::store::mailbox::StoreMailbox;
use mornlea_server::store::scheduler::{AutosaveScheduler, SchedulerConfig};
use mornlea_server::transport::{live::LoginDriver, memory::MemoryTransport};
use mornlea_storage::{METADATA_CURRENT_VERSION, Metadata, MetadataChunkPos};
use std::{
    fs,
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
    thread,
    time::{Duration, Instant},
};

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
fn authority(capacity: usize) -> AuthorityState {
    AuthorityState::try_new(
        ServerLimits::try_new(8, 4096, capacity, 64, 64, 1_048_576).unwrap(),
        7,
    )
    .unwrap()
}
fn foreign_session(count: u8) -> SessionKey {
    let mut state = authority(8);
    (1..=count)
        .map(|tag| state.admit(login(tag), TransportKind::Memory).unwrap())
        .last()
        .unwrap()
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
fn publish(state: &mut AuthorityState, recipient: EventRecipient, sequence: u64) {
    state
        .publish(TickPublication {
            tick: 0,
            events: vec![RoutedEvent::new(
                recipient,
                Event::CommandRejected(CommandRejection::new(sequence, RejectReason::InvalidRay)),
            )],
            control: vec![],
            counters: TickCounters::default(),
        })
        .unwrap();
}

#[test]
fn actual_capacity_receipts_retire_once_preserve_prefix_and_isolate_peer() {
    let mut state = authority(2);
    let slow = state.admit(login(1), TransportKind::Memory).unwrap();
    let peer = state.admit(login(2), TransportKind::Memory).unwrap();
    let mut mirror = 0;
    for sequence in 1..=3 {
        let receipt = state.enqueue_prepared(slow, frame(sequence)).unwrap();
        if receipt == EnqueueOutcome::Queued {
            mirror = sequence;
        }
        assert_eq!(
            receipt,
            if sequence <= 2 {
                EnqueueOutcome::Queued
            } else {
                EnqueueOutcome::Closed
            }
        );
    }
    assert_eq!(mirror, 2);
    assert_eq!(state.session(slow).unwrap().phase, SessionPhase::Retired);
    assert_eq!(
        state.enqueue_prepared(slow, frame(4)).unwrap(),
        EnqueueOutcome::Closed
    );
    assert_eq!(
        state.retire(slow, CloseReason::SlowReceiver),
        Err(ServerError::StaleSession { session: slow })
    );
    assert_eq!(
        state.enqueue_prepared(peer, frame(5)).unwrap(),
        EnqueueOutcome::Queued
    );
    let prefix = state
        .take_prepared_outbox(slow, usize::MAX, usize::MAX)
        .unwrap();
    assert_eq!(prefix.len(), 2);
    for (owner, sequence) in prefix.iter().zip(1..=2) {
        assert_eq!(decode(owner.as_bytes(), State::Play), packet(sequence));
    }
    assert!(
        state
            .take_prepared_outbox(slow, 8, usize::MAX)
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        decode(
            state.take_prepared_outbox(peer, 1, 0).unwrap()[0].as_bytes(),
            State::Play
        ),
        packet(5)
    );
    // A replacement proves that saturation releases exactly the retired slot.
    assert!(state.admit(login(1), TransportKind::Memory).is_ok());
}

#[test]
fn actual_phase_packet_precedence_unknown_identity_and_explicit_close() {
    let mut state = authority(2);
    let prepared = state.prepare(login(1), TransportKind::Memory).unwrap();
    let active = state.admit(login(2), TransportKind::Memory).unwrap();
    let unknown = foreign_session(3);
    assert_eq!(
        state.enqueue_prepared(prepared, frame(1)).unwrap(),
        EnqueueOutcome::Closed
    );
    assert_eq!(
        state.enqueue_prepared(unknown, frame(1)),
        Err(ServerError::StaleSession { session: unknown })
    );
    assert!(
        matches!(state.take_prepared_outbox(unknown, 0, 0), Err(ServerError::StaleSession { session }) if session == unknown)
    );
    for session in [prepared, active, unknown] {
        let control = PreparedFrame::encode(
            &mut ProtocolCodec::new().unwrap(),
            &ServerPacket::LoginSuccess(LoginSuccess::new(player(1), 7)),
        )
        .unwrap();
        assert_eq!(
            state.enqueue_prepared(session, control),
            Err(ServerError::InvalidInput { field: "packet" })
        );
    }
    assert_eq!(
        state.session(prepared).unwrap().phase,
        SessionPhase::Prepared
    );
    assert!(
        state
            .take_prepared_outbox(prepared, 8, usize::MAX)
            .unwrap()
            .is_empty()
    );
    assert!(
        state
            .take_prepared_outbox(active, 8, usize::MAX)
            .unwrap()
            .is_empty()
    );
    let control = ServerPacket::LoginSuccess(LoginSuccess::new(player(1), 7));
    state
        .publish(TickPublication {
            tick: 0,
            events: vec![],
            control: vec![ControlReply {
                session: prepared,
                packet: control.clone(),
            }],
            counters: TickCounters::default(),
        })
        .unwrap();
    let legacy_control = state.take_prepared_outbox(prepared, 1, 0).unwrap();
    assert_eq!(decode(legacy_control[0].as_bytes(), State::Login), control);
    assert_eq!(
        state.session(prepared).unwrap().phase,
        SessionPhase::Prepared
    );
    assert_eq!(
        state.enqueue_prepared(active, frame(2)).unwrap(),
        EnqueueOutcome::Queued
    );
    state.close_outbox(active, CloseReason::PeerGone);
    state.close_outbox(active, CloseReason::PeerGone);
    assert_eq!(
        state.enqueue_prepared(active, frame(3)).unwrap(),
        EnqueueOutcome::Closed
    );
    assert_eq!(state.session(active).unwrap().phase, SessionPhase::Active);
    assert_eq!(
        decode(
            state.take_prepared_outbox(active, 8, 0).unwrap()[0].as_bytes(),
            State::Play
        ),
        packet(2)
    );
}

#[test]
fn mixed_legacy_prepared_fifo_count_byte_and_nonfit_budgets() {
    let mut state = authority(8);
    let session = state.admit(login(1), TransportKind::Memory).unwrap();
    let first = frame(1);
    let second = PreparedFrame::encode(
        &mut ProtocolCodec::new().unwrap(),
        &ServerPacket::try_from(Event::CommandRejected(CommandRejection::new(
            2,
            RejectReason::InvalidRay,
        )))
        .unwrap(),
    )
    .unwrap();
    let third = frame(3);
    let control = ServerPacket::LoginSuccess(LoginSuccess::new(player(1), 7));
    let fourth = PreparedFrame::encode(&mut ProtocolCodec::new().unwrap(), &control).unwrap();
    state.enqueue_prepared(session, first.clone()).unwrap();
    publish(&mut state, EventRecipient::Session(session.get()), 2);
    state.enqueue_prepared(session, third.clone()).unwrap();
    state
        .publish(TickPublication {
            tick: 0,
            events: vec![],
            control: vec![ControlReply {
                session,
                packet: control.clone(),
            }],
            counters: TickCounters::default(),
        })
        .unwrap();
    assert!(
        state
            .take_prepared_outbox(session, 0, usize::MAX)
            .unwrap()
            .is_empty()
    );
    let prefix = state
        .take_prepared_outbox(session, 8, first.byte_len() + second.byte_len())
        .unwrap();
    assert_eq!(
        prefix
            .iter()
            .map(PreparedFrame::as_bytes)
            .collect::<Vec<_>>(),
        vec![first.as_bytes(), second.as_bytes()]
    );
    assert_eq!(prefix[0].as_bytes().as_ptr(), first.as_bytes().as_ptr());
    // Legacy observation consumes the same next owner even with a zero byte budget.
    assert_eq!(
        state.take_outbox(session, 8, 0).unwrap(),
        vec![third.as_bytes().to_vec()]
    );
    let suffix = state.take_prepared_outbox(session, 1, 0).unwrap();
    assert_eq!(suffix[0].as_bytes(), fourth.as_bytes());
    assert_eq!(decode(suffix[0].as_bytes(), State::Login), control);
    assert!(
        state
            .take_outbox(session, 8, usize::MAX)
            .unwrap()
            .is_empty()
    );
    state.enqueue_prepared(session, first.clone()).unwrap();
    state.enqueue_prepared(session, third.clone()).unwrap();
    assert_eq!(
        state
            .take_prepared_outbox(session, 1, usize::MAX)
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        state.take_prepared_outbox(session, 8, 0).unwrap()[0].as_bytes(),
        third.as_bytes()
    );
}

#[test]
fn memory_moves_shared_owner_after_packet_codec_drop_and_broadcast_shares_bytes() {
    let mut state = authority(8);
    let first = state.admit(login(1), TransportKind::Memory).unwrap();
    let second = state.admit(login(2), TransportKind::Memory).unwrap();
    let owner = {
        let packet = packet(8);
        let mut codec = ProtocolCodec::new().unwrap();
        PreparedFrame::encode(&mut codec, &packet).unwrap()
    };
    let retained = owner.clone();
    let pointer = retained.as_bytes().as_ptr();
    state.enqueue_prepared(first, owner).unwrap();
    let port: &mut dyn PreparedPublicationPort = &mut state;
    let drained = MemoryTransport::drain_prepared_session(port, first, 8, 0).unwrap();
    assert_eq!(drained[0].as_bytes().as_ptr(), pointer);
    assert_eq!(drained[0].packet_key(), retained.packet_key());
    assert_eq!(drained[0].as_bytes(), retained.as_bytes());
    publish(&mut state, EventRecipient::Broadcast, 9);
    let left = state.take_prepared_outbox(first, 1, 0).unwrap();
    let right = state.take_prepared_outbox(second, 1, 0).unwrap();
    assert_eq!(left[0].as_bytes().as_ptr(), right[0].as_bytes().as_ptr());
    assert_eq!(
        decode(left[0].as_bytes(), State::Play),
        decode(right[0].as_bytes(), State::Play)
    );
    publish(&mut state, EventRecipient::Broadcast, 10);
    let mut legacy = state.take_outbox(first, 1, 0).unwrap();
    let other = state.take_prepared_outbox(second, 1, 0).unwrap();
    assert_ne!(legacy[0].as_ptr(), other[0].as_bytes().as_ptr());
    assert_eq!(legacy[0], other[0].as_bytes());
    legacy[0][0] ^= 0xff;
    assert_ne!(legacy[0], other[0].as_bytes());
}

const BOUND: Duration = Duration::from_secs(5);
static ROOT_COUNT: AtomicU64 = AtomicU64::new(0);
struct Root(PathBuf);
impl Root {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "mornlea-prepared-delivery-{}-{}",
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

#[test]
fn actual_disk_login_acknowledgment_live_port_memory_transfer_and_lease_release() {
    let root = Root::new();
    let disk = DiskStore::open(&root.0, options()).unwrap();
    let mut authority = AuthorityState::try_new_with_metadata(
        ServerLimits::try_new(8, 4096, 512, 64, 64, 1_048_576).unwrap(),
        disk.metadata().clone(),
    )
    .unwrap();
    let mailbox = StoreMailbox::try_new_background(
        StoreLimits::try_new(2, 16, 3, 3, 3, 1, 8, 4_194_304).unwrap(),
        disk,
    )
    .unwrap();
    let mut scheduler = AutosaveScheduler::try_new(SchedulerConfig::default(), mailbox).unwrap();
    let mut driver = LoginDriver::new();
    let clock = StepClock(Instant::now());
    let mut transport = MemoryTransport::new();
    let id = transport.connect(clock.monotonic()).unwrap();
    // Catch assertions until after explicit cancellation and bounded owner close.
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let hello = ClientPacket::ClientHello(
            ClientHello::decode_inbound(&encode_uvarint(Identities::current().protocol)).unwrap(),
        );
        transport.send(
            id,
            MemoryTransport::encode_frame(&hello).unwrap(),
            &mut driver.bind(&mut authority, &mut scheduler),
            &clock,
        );
        let hello_frames = transport.receive(id, 1, usize::MAX);
        assert_eq!(hello_frames.len(), 1);
        assert!(matches!(
            decode(&hello_frames[0], State::Handshake),
            ServerPacket::ServerHello(_)
        ));
        transport.acknowledge(id, 1, &mut driver.bind(&mut authority, &mut scheduler));
        let start = LoginStart::new(player(1), "Ada", 8).unwrap();
        let request =
            ClientPacket::LoginStart(LoginStart::decode_inbound(&start.encode().unwrap()).unwrap());
        transport.send(
            id,
            MemoryTransport::encode_frame(&request).unwrap(),
            &mut driver.bind(&mut authority, &mut scheduler),
            &clock,
        );
        let until = Instant::now() + BOUND;
        let success = loop {
            scheduler.drive_workers();
            transport.poll(id, &mut driver.bind(&mut authority, &mut scheduler), &clock);
            let frames = transport.receive(id, 1, usize::MAX);
            if let Some(success) = frames.into_iter().next() {
                break success;
            }
            assert!(Instant::now() < until, "actual login deadline");
            thread::yield_now();
        };
        assert_eq!(
            decode(&success, State::Login),
            ServerPacket::LoginSuccess(LoginSuccess::new(player(1), metadata().seed as u64))
        );
        let session = foreign_session(1);
        assert_eq!(
            authority.session(session).unwrap().phase,
            SessionPhase::Prepared
        );
        assert_eq!(
            driver
                .bind(&mut authority, &mut scheduler)
                .enqueue_prepared(session, frame(1))
                .unwrap(),
            EnqueueOutcome::Closed
        );
        transport.acknowledge(id, 1, &mut driver.bind(&mut authority, &mut scheduler));
        assert_eq!(
            authority.session(session).unwrap().phase,
            SessionPhase::Active
        );
        assert_eq!(driver.pending(), 0);
        let owner = frame(88);
        let retained = owner.clone();
        let mut endpoint = driver.bind(&mut authority, &mut scheduler);
        assert_eq!(
            endpoint.enqueue_prepared(session, owner).unwrap(),
            EnqueueOutcome::Queued
        );
        let narrow: &mut dyn PreparedPublicationPort = &mut endpoint;
        let frames = MemoryTransport::drain_prepared_session(narrow, session, 1, 0).unwrap();
        assert_eq!(frames[0].as_bytes().as_ptr(), retained.as_bytes().as_ptr());
        assert_eq!(frames[0].packet_key(), retained.packet_key());
        assert_eq!(decode(frames[0].as_bytes(), State::Play), packet(88));
    }));
    transport.close(
        id,
        CloseReason::PeerGone,
        &mut driver.bind(&mut authority, &mut scheduler),
    );
    let cancellation = driver.cancel_pending(&mut authority, &mut scheduler);
    let until = Instant::now() + BOUND;
    loop {
        scheduler.drive_workers();
        match scheduler.close(deadline()) {
            Ok(()) => break,
            Err(ServerError::InvalidState {
                phase: ServerPhase::Closing,
            }) if Instant::now() < until => thread::yield_now(),
            Err(error) => panic!("actual store close refused with owner retained: {error:?}"),
        }
    }
    cancellation.unwrap();
    let mut reopened = DiskStore::open(&root.0, options()).unwrap();
    reopened.close().unwrap();
    if let Err(panic) = result {
        std::panic::resume_unwind(panic);
    }
    assert_eq!(
        authority.session(foreign_session(1)).unwrap().phase,
        SessionPhase::Retired
    );
}
