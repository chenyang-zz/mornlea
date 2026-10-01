//! The bounded I/O provider cases: the Memory queue capability and the
//! numeric-IP TCP connector behind the frozen ticket/connector ports, the
//! complete-frames-only assembler, atomic bounded queue ownership and the
//! work-budget drain.
//!
//! The table drives the transport under test through the frozen `Connector`
//! surface only: raw receive bytes go in through the fixture peer, complete
//! prefix-inclusive v45 frames come out through `poll`, and every bound
//! admits exactly its frozen value and rejects the next record with the typed
//! `Capacity` error before any allocation or partial admission. The wrong
//! double artifact at the bottom keeps the replaced contract-double behavior
//! executable: it admits partial frames and enforces no queue bound at all.

use std::io::{ErrorKind, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::num::NonZeroU64;
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use crate::support::{DeterministicClock, MemoryConnectorDouble, frame_server_packet};
use mornlea_client_core::ClientEndpoint;
use mornlea_client_core::contracts::{
    ClientConfig, ClientError, ClientIdentity, ClientLimits, ClientWorkBudget, Connector,
    ConnectorRegistry, Endpoint, TransportPoll, TransportTicket,
};
use mornlea_client_core::presentation::FamilyRecords;
use mornlea_client_core::session::io::{MemoryConnector, MemoryPeer, TcpConnector, drain_frames};
use mornlea_client_core::session::login::LoginSession;
use mornlea_domain::{Identities, PlayerId};
use mornlea_protocol::{
    ClientHello, LoginStart, LoginSuccess, ServerHello, ServerPacket, write_frame,
};

/// Which transport implementation the table drives. The red run of this table
/// drove `Double` — the only transport behavior that existed at the contract
/// landing — and its recorded wrong behavior is kept executable by the
/// artifact case below.
enum Driver {
    Double,
    Provider,
}

/// The implementation the named cases run against.
const DRIVER: Driver = Driver::Provider;

/// The bounded wait every loopback fixture uses: generous against scheduling
/// jitter, but never an unbounded sleep.
const WAIT: Duration = Duration::from_secs(5);

/// One checked login identity for the transport cases. The connector layer
/// never reads it; the frozen port carries it and the login owner owns it.
fn identity() -> ClientIdentity {
    let player =
        PlayerId::try_from_bytes([0, 0, 0, 0, 0, 0, 0x40, 0, 0x80, 0, 0, 0, 0, 0, 0, 0x21])
            .expect("uuid v4");
    let login = LoginStart::new(player, "io-driver", 8).expect("login");
    ClientIdentity::try_new(login).expect("identity")
}

/// The memory endpoint the transport cases resolve through.
fn memory_endpoint() -> Endpoint {
    Endpoint::Memory {
        connector_id: NonZeroU64::new(1).expect("one"),
    }
}

/// A small complete prefix-inclusive frame with a recognizable body.
fn small_frame(seed: u8) -> Vec<u8> {
    let frame = write_frame(u32::from(seed), &[seed; 8]).expect("fixture frame");
    assert!(
        frame.len() >= 4,
        "the small fixture frame carries a visible body"
    );
    frame
}

/// `count` distinct small frames.
fn small_frames(count: usize) -> Vec<Vec<u8>> {
    (1..=count)
        .map(|index| small_frame((index % 255) as u8 + 1))
        .collect()
}

/// One frame whose declared body is exactly `body` bytes (packet-id varint
/// plus `body - 1` payload bytes), at or below the accepted 2 MiB body cap.
fn frame_with_body(body: usize) -> Vec<u8> {
    assert!(body >= 1 && body <= 2 << 20, "fixture body within the cap");
    write_frame(0, &vec![0xAA; body - 1]).expect("fixture frame")
}

/// One frame whose complete prefix-inclusive length is exactly `total` bytes.
/// The body is solved against the canonical uvarint prefix lengths, so
/// byte-cap fixtures land on their exact charge by construction.
fn frame_of_total(total: usize) -> Vec<u8> {
    for prefix in 1..=5usize {
        let Some(body) = total.checked_sub(prefix) else {
            break;
        };
        if body == 0 {
            continue;
        }
        let canonical = match body {
            1..=0x7f => 1,
            0x80..=0x3fff => 2,
            0x4000..=0x1f_ffff => 3,
            0x20_0000..=0xfff_ffff => 4,
            _ => 5,
        };
        if canonical == prefix {
            let frame = frame_with_body(body.min(2 << 20));
            assert_eq!(frame.len(), total, "the solved fixture is exact");
            return frame;
        }
    }
    panic!("no canonical frame of total length {total}");
}

/// A frame whose declared body is exactly one byte over the accepted 2 MiB
/// body cap: the canonical uvarint of 2 MiB + 1 followed by a stand-in body
/// byte. The landed framer refuses to build it, which is the point.
fn oversized_frame() -> Vec<u8> {
    vec![0x81, 0x80, 0x80, 0x01, 0x00]
}

/// The transport under test. Every operation goes through the frozen
/// `Connector` port; `feed` delivers raw receive bytes through the fixture
/// peer, `drain` is the bounded FIFO work drain, and the sent observation is
/// the peer-visible outbound record set.
struct UnderTest {
    limits: ClientLimits,
    identity: ClientIdentity,
    double: Option<Arc<MemoryConnectorDouble>>,
    provider: Option<Arc<MemoryConnector>>,
    peer: Option<MemoryPeer>,
    sent_seen: usize,
}

impl UnderTest {
    fn new() -> Self {
        match DRIVER {
            Driver::Double => Self::double(),
            Driver::Provider => Self::provider(),
        }
    }

    /// The contract landing's deterministic double, whose transport is the
    /// recorded wrong behavior the real I/O provider replaces.
    fn double() -> Self {
        Self {
            limits: ClientLimits::try_new().expect("frozen limits"),
            identity: identity(),
            double: Some(Arc::new(MemoryConnectorDouble::new())),
            provider: None,
            peer: None,
            sent_seen: 0,
        }
    }

    /// The real Memory queue capability over the same frozen ports.
    fn provider() -> Self {
        let limits = ClientLimits::try_new().expect("frozen limits");
        let (provider, peer) = MemoryConnector::pair(limits, NonZeroU64::new(1).expect("one"));
        Self {
            limits,
            identity: identity(),
            double: None,
            provider: Some(provider),
            peer: Some(peer),
            sent_seen: 0,
        }
    }

    /// Acquires one transport through the frozen connect port.
    fn connect(&self) -> TransportTicket {
        let endpoint = memory_endpoint();
        match (&self.double, &self.provider) {
            (Some(double), None) => double
                .try_connect(&endpoint, &self.identity)
                .expect("transport ticket"),
            (None, Some(provider)) => provider
                .try_connect(&endpoint, &self.identity)
                .expect("transport ticket"),
            _ => panic!("one driver"),
        }
    }

    /// Delivers raw receive bytes. A complete frame is admitted only when the
    /// whole prefix-inclusive body has arrived; a bounded queue refusal
    /// returns the typed `Capacity` error with the complete frame retained.
    fn feed(&mut self, bytes: &[u8]) -> Result<usize, ClientError> {
        match (&self.double, &self.peer) {
            (Some(double), None) => {
                double.feed_frame(bytes.to_vec());
                Ok(bytes.len())
            }
            (None, Some(peer)) => peer.feed(bytes),
            _ => panic!("one driver"),
        }
    }

    fn poll(&self, ticket: TransportTicket) -> TransportPoll {
        self.connector().poll(ticket)
    }

    fn try_send(&self, ticket: TransportTicket, frame: &[u8]) -> Result<(), ClientError> {
        self.connector().try_send(ticket, frame)
    }

    fn close(&self, ticket: TransportTicket) -> Result<(), ClientError> {
        self.connector().close(ticket)
    }

    fn connector(&self) -> &dyn Connector {
        match (&self.double, &self.provider) {
            (Some(double), None) => double.as_ref(),
            (None, Some(provider)) => provider.as_ref(),
            _ => panic!("one driver"),
        }
    }

    /// The outbound record count the peer can currently observe.
    fn sent_depth(&self) -> usize {
        match (&self.double, &self.peer) {
            (Some(double), None) => double.sent().len(),
            (None, Some(peer)) => peer.sent_depth(),
            _ => panic!("one driver"),
        }
    }

    /// Takes every peer-visible outbound record in FIFO order.
    fn take_sent(&mut self) -> Vec<Vec<u8>> {
        match (&self.double, &self.peer) {
            (Some(double), None) => {
                let sent = double.sent();
                let fresh = sent[self.sent_seen..].to_vec();
                self.sent_seen = sent.len();
                fresh
            }
            (None, Some(peer)) => peer.drain_sent(),
            _ => panic!("one driver"),
        }
    }

    /// The bounded FIFO work drain under the frozen work budget: at most
    /// `work.messages()` frames dequeue per call, idle stops the drain, and
    /// an over-budget demand rejects before the first dequeue.
    fn drain(
        &self,
        ticket: TransportTicket,
        work: ClientWorkBudget,
    ) -> Result<Vec<Vec<u8>>, ClientError> {
        match DRIVER {
            Driver::Provider => drain_frames(self.connector(), ticket, work),
            Driver::Double => {
                if usize::from(work.messages()) > usize::from(ClientWorkBudget::MAX_PER_STEP) {
                    return Err(ClientError::Capacity);
                }
                let mut drained = Vec::new();
                while drained.len() < usize::from(work.messages()) {
                    match self.connector().poll(ticket) {
                        TransportPoll::Frame(bytes) => drained.push(bytes),
                        TransportPoll::Pending | TransportPoll::Connected => break,
                        TransportPoll::Closed(error) => return Err(error),
                    }
                }
                Ok(drained)
            }
        }
    }
}

/// `io::work_budget_zero_retains_exactly`: establishment is reported exactly
/// once, a zero work budget drains nothing and every queued frame stays
/// retained in FIFO order for the next drain.
#[test]
fn work_budget_zero_retains_exactly() {
    let mut test = UnderTest::new();
    let ticket = test.connect();

    // Establishment is reported exactly once, then idle is `Pending`.
    assert_eq!(test.poll(ticket), TransportPoll::Connected);
    assert_eq!(test.poll(ticket), TransportPoll::Pending);

    let frames = small_frames(8);
    let transcript = frames.concat();
    test.feed(&transcript).expect("transcript admitted");

    let budget = ClientWorkBudget::try_new(0, 0).expect("zero budget is legal");
    let drained = test.drain(ticket, budget).expect("zero work drain");
    assert_eq!(drained.len(), 0, "zero work dequeues nothing");

    // Every frame stays retained: the head is still the first fed frame.
    assert_eq!(test.poll(ticket), TransportPoll::Frame(frames[0].clone()));
}

/// `io::work_budget_4096_drains_exactly_fifo`: the frozen 4096 message work
/// budget drains exactly 4096 frames in FIFO order from a fuller queue, and a
/// second drain picks up the retained remainder.
#[test]
fn work_budget_4096_drains_exactly_fifo() {
    let mut test = UnderTest::new();
    let ticket = test.connect();
    assert_eq!(test.poll(ticket), TransportPoll::Connected);
    let frames = small_frames(8192);
    test.feed(&frames.concat()).expect("transcript admitted");

    let budget = ClientWorkBudget::try_new(4096, 0).expect("full budget");
    let first = test.drain(ticket, budget).expect("first full drain");
    assert_eq!(first.len(), 4096, "exactly the budget drains");
    assert_eq!(first, frames[..4096].to_vec(), "FIFO order byte for byte");
    let second = test.drain(ticket, budget).expect("second full drain");
    assert_eq!(second, frames[4096..].to_vec(), "the remainder drains next");
    assert_eq!(
        test.poll(ticket),
        TransportPoll::Pending,
        "the drained queue is empty"
    );
}

/// `io::work_budget_4097_rejects_before_dequeue`: an over-budget demand is
/// refused by the checked budget constructor before any frame dequeues, and
/// the queue is unchanged afterwards.
#[test]
fn work_budget_4097_rejects_before_dequeue() {
    let mut test = UnderTest::new();
    let ticket = test.connect();
    let frames = small_frames(4);
    test.feed(&frames.concat()).expect("transcript admitted");

    assert_eq!(
        ClientWorkBudget::try_new(4097, 0),
        Err(ClientError::Capacity),
        "4097 message work rejects with the typed capacity error"
    );
    assert_eq!(
        test.poll(ticket),
        TransportPoll::Frame(frames[0].clone()),
        "nothing dequeued: the queue head is unchanged"
    );
}

/// `io::inbound_record_cap_admits_8192_rejects_plus_one`: the inbound receiver
/// admits exactly the frozen 8192 records, the next complete frame rejects
/// with the typed `Capacity` error before admission, and the rejected frame is
/// retained intact and surfaces only after room frees.
#[test]
fn inbound_record_cap_admits_8192_rejects_plus_one() {
    let mut test = UnderTest::new();
    let ticket = test.connect();
    let cap = test.limits.inbound_observations();
    assert_eq!(cap, 8192, "the frozen inbound receiver record bound");

    let frames = small_frames(cap + 1);
    assert_eq!(
        test.feed(&frames.concat()),
        Err(ClientError::Capacity),
        "the record beyond the bound rejects with nothing partially admitted"
    );

    // Everything surfaces in exact FIFO order: the admitted records first,
    // then the retained plus-one record once room frees, never a partial or
    // reordered frame.
    let budget = ClientWorkBudget::try_new(4096, 0).expect("full budget");
    let mut surfaced = test.drain(ticket, budget).expect("first drain");
    let mut rest = test.drain(ticket, budget).expect("second drain");
    surfaced.append(&mut rest);
    let mut next = test.drain(ticket, budget).expect("retained frame drain");
    surfaced.append(&mut next);
    assert_eq!(surfaced, frames, "every frame surfaces once, in order");
    assert_eq!(test.poll(ticket), TransportPoll::Pending);
}

/// `io::inbound_byte_cap_exact_admit_plus_one_unchanged`: the inbound byte
/// bound admits frames summing to exactly the frozen cap and rejects the next
/// frame unchanged; a drained frame's bytes free exactly its own charge.
#[test]
fn inbound_byte_cap_exact_admit_plus_one_unchanged() {
    let mut test = UnderTest::new();
    let ticket = test.connect();
    assert_eq!(test.poll(ticket), TransportPoll::Connected);
    let cap = test.limits.inbound_bytes();

    // Three maximum bodies plus one sized frame land the owned charge on
    // exactly the frozen inbound byte cap.
    let big = frame_with_body(2 << 20);
    let big_len = big.len();
    let tail = frame_of_total(cap - 3 * big_len);
    assert_eq!(
        3 * big_len + tail.len(),
        cap,
        "the admitted charge is exact"
    );
    for frame in [&big, &big, &big, &tail] {
        test.feed(frame).expect("charge frame admitted");
    }

    let extra = small_frame(9);
    assert_eq!(
        test.feed(&extra),
        Err(ClientError::Capacity),
        "one frame beyond the byte cap rejects"
    );

    // Unchanged: the drain returns exactly the four admitted frames — the
    // rejected frame contributed no bytes to the held charge.
    let budget = ClientWorkBudget::try_new(4, 0).expect("small budget");
    let drained = test.drain(ticket, budget).expect("drain the charge");
    assert_eq!(
        drained.iter().map(Vec::len).sum::<usize>(),
        cap,
        "the held charge is exactly the cap"
    );
    assert_eq!(
        drained,
        vec![big.clone(), big.clone(), big.clone(), tail],
        "only the admitted records are held"
    );

    // The rejected frame is intact and surfaces once its bytes are free.
    assert_eq!(
        test.poll(ticket),
        TransportPoll::Frame(extra),
        "the retained frame admits exactly when its charge frees"
    );
    assert_eq!(test.poll(ticket), TransportPoll::Pending);
}

/// `io::outbound_record_cap_admits_4104_rejects_plus_one`: the outbound queue
/// admits exactly the frozen 4104 records; the next complete frame rejects
/// with the typed `Capacity` error and the queue is unchanged, and the same
/// frame sends once room frees.
#[test]
fn outbound_record_cap_admits_4104_rejects_plus_one() {
    let mut test = UnderTest::new();
    let ticket = test.connect();
    let cap = test.limits.outbound_commands();
    assert_eq!(cap, 4104, "the frozen outbound queue record bound");

    let frames = small_frames(cap);
    for frame in &frames {
        test.try_send(ticket, frame).expect("record admitted");
    }
    assert_eq!(
        test.sent_depth(),
        cap,
        "every admitted record is observable"
    );

    let extra = small_frame(77);
    assert_eq!(
        test.try_send(ticket, &extra),
        Err(ClientError::Capacity),
        "the record beyond the bound rejects"
    );
    assert_eq!(
        test.sent_depth(),
        cap,
        "the rejection changes nothing in the queue"
    );

    let delivered = test.take_sent();
    assert_eq!(delivered, frames, "the admitted records deliver in order");
    test.try_send(ticket, &extra)
        .expect("the rejected frame sends once room frees");
    assert_eq!(
        test.take_sent(),
        vec![extra.clone()],
        "delivered exactly once"
    );
}

/// `io::outbound_byte_cap_exact_admit_plus_one_unchanged`: the outbound byte
/// bound admits frames summing to exactly the frozen cap, rejects the next
/// frame unchanged, and the same frame sends once its bytes are free.
#[test]
fn outbound_byte_cap_exact_admit_plus_one_unchanged() {
    let mut test = UnderTest::new();
    let ticket = test.connect();
    let cap = test.limits.outbound_bytes();

    let big = frame_with_body(2 << 20);
    let big_len = big.len();
    let tail = frame_of_total(cap - 3 * big_len);
    assert_eq!(
        3 * big_len + tail.len(),
        cap,
        "the reserved charge is exact"
    );
    for frame in [&big, &big, &big, &tail] {
        test.try_send(ticket, frame).expect("charge frame reserved");
    }
    assert_eq!(test.sent_depth(), 4);

    let extra = small_frame(5);
    assert_eq!(
        test.try_send(ticket, &extra),
        Err(ClientError::Capacity),
        "one frame beyond the byte cap rejects"
    );
    assert_eq!(test.sent_depth(), 4, "the rejection changes nothing");

    let delivered = test.take_sent();
    assert_eq!(
        delivered.iter().map(Vec::len).sum::<usize>(),
        cap,
        "the reserved charge is exactly the cap"
    );
    test.try_send(ticket, &extra)
        .expect("the rejected frame sends once its bytes are free");
    assert_eq!(test.take_sent(), vec![extra], "delivered exactly once");
}

/// `io::fragmented_prefix_and_body_admit_complete_frame_only`: a fragmented
/// length prefix and a fragmented body never surface anything; the complete
/// prefix-inclusive frame surfaces exactly once when its last byte arrives.
#[test]
fn fragmented_prefix_and_body_admit_complete_frame_only() {
    let mut test = UnderTest::new();
    let ticket = test.connect();
    assert_eq!(test.poll(ticket), TransportPoll::Connected);

    // A two-byte length prefix and a body split across three deliveries.
    let frame = frame_with_body(200);
    assert_eq!(frame[0] & 0x80, 0x80, "the fixture prefix is two bytes");
    test.feed(&frame[..1]).expect("partial prefix staged");
    assert_eq!(
        test.poll(ticket),
        TransportPoll::Pending,
        "a partial prefix never surfaces"
    );
    test.feed(&frame[1..2])
        .expect("prefix completed, body absent");
    assert_eq!(
        test.poll(ticket),
        TransportPoll::Pending,
        "a complete prefix without its body never surfaces"
    );
    test.feed(&frame[2..102]).expect("partial body staged");
    assert_eq!(
        test.poll(ticket),
        TransportPoll::Pending,
        "a partial body never surfaces"
    );
    test.feed(&frame[102..]).expect("body completed");
    assert_eq!(
        test.poll(ticket),
        TransportPoll::Frame(frame),
        "the complete frame surfaces exactly once"
    );
    assert_eq!(test.poll(ticket), TransportPoll::Pending);

    // Coalesced delivery: two complete frames plus a partial tail admit the
    // two and retain the tail without surfacing it.
    let one = small_frame(1);
    let two = small_frame(2);
    let three = frame_with_body(150);
    let mut transcript = one.clone();
    transcript.extend_from_slice(&two);
    transcript.extend_from_slice(&three[..60]);
    test.feed(&transcript).expect("coalesced transcript staged");
    assert_eq!(test.poll(ticket), TransportPoll::Frame(one));
    assert_eq!(test.poll(ticket), TransportPoll::Frame(two));
    assert_eq!(
        test.poll(ticket),
        TransportPoll::Pending,
        "the partial tail never surfaces"
    );
    test.feed(&three[60..]).expect("tail completed");
    assert_eq!(test.poll(ticket), TransportPoll::Frame(three));
}

/// `io::malformed_and_oversized_inbound_fail_sticky`: a declared empty frame,
/// a malformed length prefix and a declared body one byte over the accepted
/// 2 MiB cap each fail the receive stream with the typed error, stickily,
/// after the frames admitted before them have drained.
#[test]
fn malformed_and_oversized_inbound_fail_sticky() {
    // A declared empty frame is a permanent malformation.
    let mut test = UnderTest::new();
    let ticket = test.connect();
    assert_eq!(
        test.feed(&[0x00]),
        Err(ClientError::InvalidInput),
        "a zero length declaration is malformed"
    );
    assert_eq!(
        test.poll(ticket),
        TransportPoll::Closed(ClientError::InvalidInput)
    );
    assert_eq!(
        test.poll(ticket),
        TransportPoll::Closed(ClientError::InvalidInput),
        "the failure is sticky"
    );

    // Five continuation bytes can never complete a canonical uvarint.
    let mut test = UnderTest::new();
    let ticket = test.connect();
    assert_eq!(
        test.feed(&[0x80; 5]),
        Err(ClientError::InvalidInput),
        "an uncompletable length prefix is malformed"
    );
    assert_eq!(
        test.poll(ticket),
        TransportPoll::Closed(ClientError::InvalidInput)
    );

    // A declared body one byte over the accepted 2 MiB cap rejects with the
    // typed capacity error.
    let mut test = UnderTest::new();
    let ticket = test.connect();
    assert_eq!(
        test.feed(&oversized_frame()),
        Err(ClientError::Capacity),
        "a declared body over the cap rejects"
    );
    assert_eq!(
        test.poll(ticket),
        TransportPoll::Closed(ClientError::Capacity)
    );
    assert_eq!(
        test.poll(ticket),
        TransportPoll::Closed(ClientError::Capacity)
    );

    // Frames admitted before the failure drain first; the failure surfaces
    // only once the queue is empty.
    let mut test = UnderTest::new();
    let ticket = test.connect();
    let good = small_frame(3);
    test.feed(&good).expect("good frame admitted");
    assert_eq!(
        test.feed(&[0x00]),
        Err(ClientError::InvalidInput),
        "the malformed tail fails the stream"
    );
    assert_eq!(test.poll(ticket), TransportPoll::Frame(good));
    assert_eq!(
        test.poll(ticket),
        TransportPoll::Closed(ClientError::InvalidInput)
    );
}

/// `io::malformed_oversized_partial_try_send_rejects_unchanged`: the send port
/// admits exactly one complete prefix-inclusive frame per call; a partial
/// frame, an empty slice, trailing bytes after a complete frame and a declared
/// body over the cap each reject with the queue unchanged.
#[test]
fn malformed_oversized_partial_try_send_rejects_unchanged() {
    let mut test = UnderTest::new();
    let ticket = test.connect();
    let good = small_frame(4);

    let bad_inputs: [(&str, Vec<u8>, ClientError); 4] = [
        (
            "partial frame",
            good[..2].to_vec(),
            ClientError::InvalidInput,
        ),
        ("empty slice", Vec::new(), ClientError::InvalidInput),
        (
            "trailing bytes after a complete frame",
            {
                let mut bytes = good.clone();
                bytes.push(0);
                bytes
            },
            ClientError::InvalidInput,
        ),
        (
            "declared body one byte over the cap",
            oversized_frame(),
            ClientError::Capacity,
        ),
    ];
    for (name, bytes, error) in bad_inputs {
        assert_eq!(test.try_send(ticket, &bytes), Err(error), "{name}: rejects");
        assert_eq!(
            test.sent_depth(),
            0,
            "{name}: the rejection changes nothing"
        );
    }

    test.try_send(ticket, &good)
        .expect("a complete frame sends");
    assert_eq!(test.take_sent(), vec![good]);
}

/// `io::capacity_then_retry_sends_same_complete_head_once`: after a capacity
/// refusal the caller retains the complete head; the retry sends exactly that
/// byte-identical head once the bound frees room, never a partial record.
#[test]
fn capacity_then_retry_sends_same_complete_head_once() {
    let mut test = UnderTest::new();
    let ticket = test.connect();
    let cap = test.limits.outbound_commands();

    // Fill the outbound record bound so the head refusal is a capacity one.
    let frames = small_frames(cap);
    for frame in &frames {
        test.try_send(ticket, frame).expect("record admitted");
    }
    let head = small_frame(123);
    assert_eq!(
        test.try_send(ticket, &head),
        Err(ClientError::Capacity),
        "the head refusal is the typed capacity error"
    );

    // The caller retains the complete head; room frees and the identical
    // bytes send exactly once.
    let delivered = test.take_sent();
    assert_eq!(delivered, frames);
    let retained = head.clone();
    test.try_send(ticket, &retained).expect("the retry sends");
    assert_eq!(
        test.take_sent(),
        vec![head],
        "the same head once, not a partial"
    );
}

/// `io::reset_during_fragment_cannot_publish_old_packet`: a reset while a
/// frame is fragmented drops the partial receive and invalidates the old
/// ticket, so the old frame can never publish even when its completion bytes
/// arrive, and the new epoch starts from empty receive state.
#[test]
fn reset_during_fragment_cannot_publish_old_packet() {
    let mut test = UnderTest::new();
    let old = test.connect();
    assert_eq!(test.poll(old), TransportPoll::Connected);

    // Half of a frame whose body is continuation bytes, so the completion
    // half alone can never assemble a valid frame in the new epoch.
    let mut frame = vec![0xC8, 0x01, 0x00];
    frame.extend_from_slice(&[0x80; 199]);
    test.feed(&frame[..100]).expect("partial frame staged");
    assert_eq!(test.poll(old), TransportPoll::Pending);

    // Reset: the old transport is released and a fresh one is acquired.
    test.close(old).expect("the old transport is released");
    let fresh = test.connect();

    // The old ticket is invalid: it can neither publish, send nor release.
    assert_eq!(
        test.poll(old),
        TransportPoll::Closed(ClientError::InvalidState)
    );
    assert_eq!(
        test.try_send(old, &small_frame(6)),
        Err(ClientError::InvalidState)
    );
    assert_eq!(test.close(old), Err(ClientError::InvalidState));

    // The completion bytes arrive at the new epoch, whose receive state
    // starts empty: the orphaned half is a deterministic malformation, never
    // the old packet.
    assert_eq!(
        test.feed(&frame[100..]),
        Err(ClientError::InvalidInput),
        "the orphaned completion half fails the new stream"
    );
    let mut published = false;
    for _ in 0..3 {
        if let TransportPoll::Frame(bytes) = test.poll(fresh) {
            assert_ne!(bytes, frame, "no old-epoch packet can publish");
            published = true;
        }
    }
    assert!(!published, "the old fragmented packet never publishes");
    assert_eq!(
        test.poll(fresh),
        TransportPoll::Closed(ClientError::InvalidInput),
        "the malformed new stream is sticky"
    );

    // A subsequent epoch still works: the failed stream is released and the
    // next transport admits a fresh frame normally.
    test.close(fresh).expect("release the failed transport");
    let newest = test.connect();
    let after = small_frame(8);
    test.feed(&after).expect("fresh frame admitted");
    assert_eq!(test.poll(newest), TransportPoll::Frame(after));
}

/// `io::stale_ticket_operations_reject_after_close`: every operation on a
/// closed or unknown ticket rejects with the typed invalid-state error, so no
/// old-epoch packet can publish and no duplicate release can occur.
#[test]
fn stale_ticket_operations_reject_after_close() {
    let mut test = UnderTest::new();
    let ticket = test.connect();
    let good = small_frame(2);
    test.feed(&good).expect("frame admitted");
    test.close(ticket).expect("release once");

    assert_eq!(
        test.poll(ticket),
        TransportPoll::Closed(ClientError::InvalidState)
    );
    assert_eq!(test.try_send(ticket, &good), Err(ClientError::InvalidState));
    assert_eq!(test.close(ticket), Err(ClientError::InvalidState));

    // An unknown launch generation is the same rejection.
    let unknown = TransportTicket::try_new(
        NonZeroU64::new(1).expect("one"),
        NonZeroU64::new(9_999).expect("nonzero"),
    )
    .expect("ticket shape");
    assert_eq!(
        test.poll(unknown),
        TransportPoll::Closed(ClientError::InvalidState)
    );
    assert_eq!(
        test.try_send(unknown, &good),
        Err(ClientError::InvalidState)
    );
    assert_eq!(test.close(unknown), Err(ClientError::InvalidState));
}

/// Polls until a non-idle observation arrives or the bounded wait elapses;
/// the wait is the loopback fixture's only patience, never an unbounded
/// sleep.
fn poll_bounded(
    connector: &dyn Connector,
    ticket: TransportTicket,
    wait: Duration,
) -> TransportPoll {
    let deadline = Instant::now() + wait;
    loop {
        match connector.poll(ticket) {
            TransportPoll::Pending => {
                if Instant::now() >= deadline {
                    return TransportPoll::Pending;
                }
                thread::sleep(Duration::from_millis(1));
            }
            other => return other,
        }
    }
}

/// Collects exactly `count` admitted frames with the bounded wait, skipping
/// the one-time establishment report.
fn collect_frames(
    connector: &dyn Connector,
    ticket: TransportTicket,
    count: usize,
    wait: Duration,
) -> Vec<Vec<u8>> {
    let deadline = Instant::now() + wait;
    let mut frames = Vec::new();
    while frames.len() < count {
        assert!(Instant::now() < deadline, "timed out collecting frames");
        match connector.poll(ticket) {
            TransportPoll::Frame(bytes) => frames.push(bytes),
            TransportPoll::Connected => {}
            TransportPoll::Pending | TransportPoll::Closed(_) => {
                thread::sleep(Duration::from_millis(1))
            }
        }
    }
    frames
}

/// Accepts the loopback peer's connection within the bounded wait.
fn accept_bounded(listener: &TcpListener, wait: Duration) -> TcpStream {
    listener
        .set_nonblocking(true)
        .expect("nonblocking fixture listener");
    let deadline = Instant::now() + wait;
    loop {
        match listener.accept() {
            Ok((stream, _)) => return stream,
            Err(error) if error.kind() == ErrorKind::WouldBlock => {
                assert!(Instant::now() < deadline, "no peer connection arrived");
                thread::sleep(Duration::from_millis(1));
            }
            Err(error) => panic!("fixture listener error: {error}"),
        }
    }
}

/// Reads exactly `count` bytes from the fixture peer within the bounded wait.
fn read_exact_bounded(stream: &mut TcpStream, count: usize, wait: Duration) -> Vec<u8> {
    stream
        .set_read_timeout(Some(Duration::from_millis(50)))
        .expect("paced fixture read");
    let deadline = Instant::now() + wait;
    let mut buffer = Vec::with_capacity(count);
    let mut chunk = vec![0u8; count.max(1)];
    while buffer.len() < count {
        match stream.read(&mut chunk[..count - buffer.len()]) {
            Ok(0) => panic!("the fixture peer closed before the expected bytes"),
            Ok(read) => buffer.extend_from_slice(&chunk[..read]),
            Err(error)
                if error.kind() == ErrorKind::WouldBlock
                    || error.kind() == ErrorKind::TimedOut
                    || error.kind() == ErrorKind::Interrupted =>
            {
                assert!(
                    Instant::now() < deadline,
                    "timed out reading expected bytes"
                );
            }
            Err(error) => panic!("fixture read error: {error}"),
        }
    }
    buffer
}

/// The loopback fixture listener on an OS-assigned ephemeral port.
fn loopback() -> TcpListener {
    TcpListener::bind("127.0.0.1:0").expect("ephemeral loopback listener")
}

/// `io::memory_tcp_transcript_same_admitted_observations`: the same chunked
/// byte transcript through the Memory capability and through the real TCP
/// connector over a loopback socket pair admits the same complete frames in
/// the same order.
#[test]
fn memory_tcp_transcript_same_admitted_observations() {
    let frames = small_frames(5);
    // The transcript: one whole frame, one frame split across two chunks, a
    // coalesced pair, and one frame completed by its tail chunk.
    let chunks = vec![
        frames[0].clone(),
        frames[1][..4].to_vec(),
        frames[1][4..].to_vec(),
        {
            let mut coalesced = frames[2].clone();
            coalesced.extend_from_slice(&frames[3]);
            coalesced
        },
        frames[4][..3].to_vec(),
        frames[4][3..].to_vec(),
    ];

    // Memory capability.
    let mut memory = UnderTest::new();
    let memory_ticket = memory.connect();
    assert_eq!(memory.poll(memory_ticket), TransportPoll::Connected);
    for chunk in &chunks {
        let consumed = memory.feed(chunk).expect("chunk staged");
        assert_eq!(consumed, chunk.len(), "small chunks stage whole");
    }
    let memory_frames = collect_frames(memory.connector(), memory_ticket, 5, WAIT);
    assert_eq!(memory_frames, frames);
    assert_eq!(memory.poll(memory_ticket), TransportPoll::Pending);

    // The real TCP connector over a loopback socket pair. This is a fixture
    // peer on an ephemeral port, not a server-authority acceptance.
    let limits = ClientLimits::try_new().expect("frozen limits");
    let tcp = TcpConnector::new(
        NonZeroU64::new(2).expect("two"),
        limits,
        Duration::from_secs(2),
    )
    .expect("tcp capability");
    let listener = loopback();
    let address = listener.local_addr().expect("loopback address");
    let endpoint = Endpoint::Tcp(address);
    let ticket = tcp
        .try_connect(&endpoint, &identity())
        .expect("loopback ticket");
    let mut peer = accept_bounded(&listener, WAIT);
    assert_eq!(
        poll_bounded(&tcp, ticket, WAIT),
        TransportPoll::Connected,
        "the dial is reported exactly once"
    );
    for chunk in &chunks {
        peer.write_all(chunk).expect("fixture write");
        peer.flush().expect("fixture flush");
    }
    let tcp_frames = collect_frames(&tcp, ticket, 5, WAIT);
    assert_eq!(
        tcp_frames, memory_frames,
        "the same transcript admits the same observations"
    );
    assert_eq!(
        poll_bounded(&tcp, ticket, Duration::from_millis(150)),
        TransportPoll::Pending
    );
    tcp.close(ticket).expect("release the loopback transport");
}

/// `io::tcp_loopback_exchange_fragmented_receive`: the real TCP connector
/// carries a complete outbound frame to the fixture peer and assembles a
/// fragmented inbound frame into exactly one complete observation.
#[test]
fn tcp_loopback_exchange_fragmented_receive() {
    let limits = ClientLimits::try_new().expect("frozen limits");
    let tcp = TcpConnector::new(
        NonZeroU64::new(2).expect("two"),
        limits,
        Duration::from_secs(2),
    )
    .expect("tcp capability");
    let listener = loopback();
    let address = listener.local_addr().expect("loopback address");
    let ticket = tcp
        .try_connect(&Endpoint::Tcp(address), &identity())
        .expect("loopback ticket");
    let mut peer = accept_bounded(&listener, WAIT);
    assert_eq!(poll_bounded(&tcp, ticket, WAIT), TransportPoll::Connected);

    // One complete frame out: the peer reads exactly those bytes.
    let outbound = frame_with_body(300);
    tcp.try_send(ticket, &outbound)
        .expect("complete frame queued");
    let seen = read_exact_bounded(&mut peer, outbound.len(), WAIT);
    assert_eq!(seen, outbound, "the complete frame arrives byte for byte");

    // A fragmented frame in: three chunks, one complete observation.
    let inbound = frame_with_body(400);
    peer.write_all(&inbound[..5]).expect("fixture write");
    peer.flush().expect("fixture flush");
    assert_eq!(poll_bounded(&tcp, ticket, WAIT), TransportPoll::Pending);
    peer.write_all(&inbound[5..200]).expect("fixture write");
    peer.flush().expect("fixture flush");
    peer.write_all(&inbound[200..]).expect("fixture write");
    peer.flush().expect("fixture flush");
    assert_eq!(
        poll_bounded(&tcp, ticket, WAIT),
        TransportPoll::Frame(inbound),
        "the fragments assemble into exactly one complete frame"
    );
    tcp.close(ticket).expect("release the loopback transport");
}

/// `io::tcp_peer_close_and_connect_refusal_report_typed_close`: a peer that
/// closes its end reports the sticky typed disconnect, the refused dial of a
/// closed port reports the sticky typed I/O failure, and the released ticket
/// rejects every later operation.
#[test]
fn tcp_peer_close_and_connect_refusal_report_typed_close() {
    let limits = ClientLimits::try_new().expect("frozen limits");
    let tcp = TcpConnector::new(
        NonZeroU64::new(2).expect("two"),
        limits,
        Duration::from_secs(2),
    )
    .expect("tcp capability");

    // The peer closes: end of file is the typed transport disconnect.
    let listener = loopback();
    let address = listener.local_addr().expect("loopback address");
    let ticket = tcp
        .try_connect(&Endpoint::Tcp(address), &identity())
        .expect("loopback ticket");
    let peer = accept_bounded(&listener, WAIT);
    assert_eq!(poll_bounded(&tcp, ticket, WAIT), TransportPoll::Connected);
    drop(peer);
    assert_eq!(
        poll_bounded(&tcp, ticket, WAIT),
        TransportPoll::Closed(ClientError::Disconnected),
        "end of file is the remote disconnect"
    );
    assert_eq!(
        poll_bounded(&tcp, ticket, WAIT),
        TransportPoll::Closed(ClientError::Disconnected),
        "the close is sticky"
    );
    assert_eq!(
        tcp.try_send(ticket, &small_frame(3)),
        Err(ClientError::Disconnected),
        "a closed transport refuses sends with its typed error"
    );
    tcp.close(ticket).expect("release once");
    assert_eq!(
        tcp.poll(ticket),
        TransportPoll::Closed(ClientError::InvalidState),
        "the released ticket is invalid"
    );

    // A dial to a port with no listener is the typed I/O failure.
    let closed = loopback();
    let refused = closed.local_addr().expect("loopback address");
    drop(closed);
    let ticket = tcp
        .try_connect(&Endpoint::Tcp(refused), &identity())
        .expect("ticket before the dial resolves");
    assert_eq!(
        poll_bounded(&tcp, ticket, WAIT),
        TransportPoll::Closed(ClientError::Io),
        "the refused dial reports the typed I/O failure"
    );
    assert_eq!(
        poll_bounded(&tcp, ticket, Duration::from_millis(150)),
        TransportPoll::Closed(ClientError::Io),
        "the dial failure is sticky"
    );
    tcp.close(ticket).expect("release the failed transport");
}

/// The client's current-version hello frame through the landed encoder, the
/// exact bytes the login owner queues on the transport.
fn hello_frame() -> Vec<u8> {
    let hello = ClientHello::new(Identities::current().protocol).expect("current hello");
    write_frame(
        ClientHello::PACKET_ID,
        &hello.encode().expect("hello payload"),
    )
    .expect("hello frame")
}

/// The client's login start frame for one checked identity.
fn login_frame(for_identity: &ClientIdentity) -> Vec<u8> {
    write_frame(
        LoginStart::PACKET_ID,
        &for_identity.login().encode().expect("login payload"),
    )
    .expect("login frame")
}

/// The server's current-version hello frame.
fn server_hello_frame() -> Vec<u8> {
    frame_server_packet(&ServerPacket::ServerHello(
        ServerHello::new(Identities::current().protocol).expect("current server hello"),
    ))
}

/// The server's login success frame for one player.
fn login_success_frame(for_player: PlayerId) -> Vec<u8> {
    frame_server_packet(&ServerPacket::LoginSuccess(LoginSuccess::new(
        for_player, 7,
    )))
}

/// `io::login_session_over_memory_connector_capability`: the landed login
/// provider runs its complete hello/login exchange unchanged over the real
/// Memory capability through a real connector registry — the same frames out,
/// the same raw fragmented bytes in, the same admission at the end.
#[test]
fn login_session_over_memory_connector_capability() {
    let limits = ClientLimits::try_new().expect("frozen limits");
    let (connector, peer) = MemoryConnector::pair(limits, NonZeroU64::new(1).expect("one"));
    let mut registry = ConnectorRegistry::new();
    registry
        .register(NonZeroU64::new(1).expect("one"), connector)
        .expect("capability registered");
    let config = ClientConfig::try_new(
        limits,
        Duration::from_secs(5),
        Duration::from_secs(10),
        Arc::new(DeterministicClock::new()),
        Arc::new(registry),
    )
    .expect("checked config");
    let mut session = LoginSession::new(config).expect("login session over the real transport");
    let driver = identity();
    let epoch = session
        .connect(memory_endpoint(), driver.clone())
        .expect("pending epoch over the real capability");
    assert_eq!(
        peer.drain_sent(),
        vec![hello_frame()],
        "the hello frame is queued on the real outbound path"
    );

    // The server hello arrives fragmented: the real assembler admits it only
    // once complete, and the login start follows byte for byte.
    let server_hello = server_hello_frame();
    let split = server_hello.len() - 1;
    peer.feed(&server_hello[..split]).expect("fragment staged");
    let budget = ClientWorkBudget::try_new(4, 0).expect("step budget");
    let report = session.step(epoch, budget).expect("fragment-only step");
    assert_eq!(report.processed_messages(), 0, "no partial observation");
    peer.feed(&server_hello[split..])
        .expect("completion staged");
    session.step(epoch, budget).expect("hello step");
    assert_eq!(
        peer.drain_sent(),
        vec![login_frame(&driver)],
        "the login start follows the accepted hello"
    );

    // The login success admits the session over the real transport.
    let player = driver.login().player_id;
    peer.feed(&login_success_frame(player))
        .expect("success staged");
    session.step(epoch, budget).expect("login step");
    let visible = session.snapshot(epoch).expect("visible frame");
    let family = visible
        .families()
        .iter()
        .find(|family| family.key().logical_name == mornlea_client_core::contracts::FAMILY_SESSION)
        .expect("session family");
    let FamilyRecords::Session(records) = family.records() else {
        panic!("session records");
    };
    assert_eq!(records.len(), 1, "one session record per publication");
    assert_eq!(
        *records[0].phase(),
        mornlea_client_core::contracts::SessionPhase::Admitted,
        "the exchange admits over the real capability"
    );
}

/// The wrong-behavior artifact: the contract double's transport — the behavior
/// the real I/O provider replaces — admits a partial frame as a `Frame`
/// observation and enforces no queue bound, so neither complete-frames-only
/// assembly nor bounded ownership exists there.
#[test]
fn wrong_double_admits_partial_and_unbounded_transport() {
    let mut test = UnderTest::double();
    let ticket = test.connect();

    // Partial admission: half a frame surfaces as a frame.
    let frame = small_frame(7);
    test.feed(&frame[..3]).expect("the double stages any chunk");
    assert_eq!(
        test.poll(ticket),
        TransportPoll::Frame(frame[..3].to_vec()),
        "the double admits a partial frame — the replaced wrong behavior"
    );

    // No bound: feeding far beyond the frozen inbound record count never
    // reports the typed capacity error.
    let beyond = small_frames(10);
    assert_eq!(
        test.feed(&beyond.concat()),
        Ok(beyond.concat().len()),
        "the double has no queue bound — the replaced wrong behavior"
    );

    // No establishment signal: the double never reports `Connected`.
    let fresh = UnderTest::double();
    let ticket = fresh.connect();
    assert_eq!(fresh.poll(ticket), TransportPoll::Pending);
}
