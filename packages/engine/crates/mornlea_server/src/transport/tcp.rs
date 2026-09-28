//! Loopback TCP adapter over the shared transport admission core.
//!
//! This module owns the socket half of the remote transport: a loopback
//! listener with ephemeral ports, one nonblocking stream per accepted peer,
//! and the pump calls that move bytes between those streams and the single
//! shared [`ConnectionCore`]. Every decoded frame travels through the core,
//! so hello validation, the prelogin reservation, login admission, and play
//! conversion are never duplicated here; the adapter moves opaque envelopes
//! in and out.
//!
//! The serving model is synchronous and caller-driven: the owner calls
//! `accept_one`, `pump_in`, `flush_out`, and `poll` as its loop schedules
//! them. No call blocks and no call sleeps; each does bounded work per
//! connection. All deadlines come from the injected clock the caller passes
//! through, so virtual time alone drives the hello and login expiries.
//!
//! Outbound traffic has two lanes. Control frames the core queues (hello
//! answers, rejects, the login success record) are already complete
//! envelopes and leave through `flush_out`, which reports them sent through
//! the core acknowledgment ledger; only the acknowledged success handoff
//! admits play. Publication payloads the authority holds per session carry
//! no packet identity on the frozen surface, so the serving loop supplies
//! one packet id per drained payload from its own publication record and
//! `forward_outbox` frames the envelopes with the protocol codec directly.
//! Both lanes share one per-connection send queue with the same frame bound
//! the authority applies per session, so a peer that stops reading cannot
//! grow the adapter without limit.
//!
//! Saturation detection stays with the publication owner: when the
//! authority retires a receiver, the serving loop reaps the connection
//! through `close_slow_receiver`. The adapter owns the close mechanics for
//! that reap and never appends a disconnect frame of its own.

use std::collections::{BTreeMap, VecDeque};
use std::io::{self, Read, Write};
use std::net::{Shutdown, SocketAddr, TcpListener, TcpStream};

use crate::contracts::{
    Clock, CloseReason, ConnectionId, ConnectionProgress, SessionKey, TransportKind,
};
use crate::transport::common::{ConnectionCore, HandshakeLimits, TransportAuthority};

/// Bytes read per nonblocking socket read.
const READ_CHUNK: usize = 4096;
/// Reads attempted per `pump_in` call, bounding one pump's socket work.
const READS_PER_PUMP: usize = 16;
/// Core frames moved per `flush_out` call, matching the core poll budget.
const FLUSH_FRAME_BUDGET: usize = 64;
/// Envelope bytes moved per `flush_out` call, matching the core poll budget.
const FLUSH_BYTE_BUDGET: usize = 1 << 20;
/// Per-connection queued send frames, mirroring the per-session outbox bound
/// the authority enforces. A peer that stops reading trips this ceiling
/// instead of growing the adapter without limit.
const MAX_QUEUED_SEND_FRAMES: usize = 512;

/// One queued outbound envelope. The lane tag keeps the core acknowledgment
/// ledger exact: only frames taken from the core consume send acknowledgment
/// slots, while publication envelopes bypass that ledger entirely.
enum Queued {
    Core(Vec<u8>),
    Outbox(Vec<u8>),
}

impl Queued {
    fn bytes(&self) -> &[u8] {
        match self {
            Queued::Core(frame) | Queued::Outbox(frame) => frame,
        }
    }

    fn is_core(&self) -> bool {
        matches!(self, Queued::Core(_))
    }
}

/// One accepted peer: its nonblocking stream, the ordered send queue shared
/// by both outbound lanes, and the written prefix of the queue head.
struct TcpConn {
    stream: TcpStream,
    queue: VecDeque<Queued>,
    head: usize,
}

impl TcpConn {
    /// Writes queued envelopes in order until the socket blocks or the queue
    /// empties. Returns the count of fully written core frames so the caller
    /// can acknowledge exactly those against the core ledger; a partial head
    /// keeps its offset for the next call.
    fn drain_queue(&mut self) -> usize {
        let mut sent_core = 0;
        while let Some(front) = self.queue.front() {
            let written = match self.stream.write(&front.bytes()[self.head..]) {
                Ok(written) => written,
                Err(_) => break,
            };
            // A zero write on a nonblocking stream means no progress is
            // possible now; stopping avoids a hot spin.
            if written == 0 {
                break;
            }
            self.head += written;
            if self.head == front.bytes().len() {
                if front.is_core() {
                    sent_core += 1;
                }
                self.queue.pop_front();
                self.head = 0;
            }
        }
        sent_core
    }
}

/// Loopback TCP transport over the shared admission core.
pub struct TcpTransport {
    listener: TcpListener,
    core: ConnectionCore,
    streams: BTreeMap<u64, TcpConn>,
}

impl TcpTransport {
    /// Binds a nonblocking loopback listener on an ephemeral port with the
    /// frozen source handshake limits.
    pub fn bind_loopback() -> io::Result<Self> {
        let listener = TcpListener::bind(("127.0.0.1", 0))?;
        listener.set_nonblocking(true)?;
        Ok(Self {
            listener,
            core: ConnectionCore::new(HandshakeLimits::source()),
            streams: BTreeMap::new(),
        })
    }

    /// The loopback address peers connect to.
    pub fn local_addr(&self) -> io::Result<SocketAddr> {
        self.listener.local_addr()
    }

    /// Accepts one pending peer and reserves its prelogin slot. Returns
    /// `None` when no peer is waiting; a refused reservation drops the peer
    /// at once and also reports `None`, since the refusal itself is the
    /// observable answer.
    pub fn accept_one(
        &mut self,
        endpoint: &mut dyn TransportAuthority,
        clock: &dyn Clock,
    ) -> io::Result<Option<ConnectionId>> {
        let (stream, _) = match self.listener.accept() {
            Ok(pair) => pair,
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => return Ok(None),
            Err(error) => return Err(error),
        };
        stream.set_nonblocking(true)?;
        let _ = endpoint;
        let id = match self.core.open(TransportKind::Tcp, clock.monotonic()) {
            Ok(id) => id,
            Err(_) => return Ok(None),
        };
        self.streams.insert(
            id.get(),
            TcpConn {
                stream,
                queue: VecDeque::new(),
                head: 0,
            },
        );
        Ok(Some(id))
    }

    /// Reads available bytes from one peer and offers them to the core. Each
    /// read chunk runs one bounded core poll, so a coalesced burst processes
    /// under the same frame and byte budgets as any other ingress. A socket
    /// without a live stream delegates to the core poll so timeouts and
    /// replays still report.
    pub fn pump_in(
        &mut self,
        id: ConnectionId,
        endpoint: &mut dyn TransportAuthority,
        clock: &dyn Clock,
    ) -> ConnectionProgress {
        if !self.streams.contains_key(&id.get()) {
            return self.core.poll(id, endpoint, clock);
        }
        enum Outcome {
            Data(Vec<u8>),
            Eof,
            Wait,
            Reset,
        }
        let mut last = ConnectionProgress::AwaitMore;
        for _ in 0..READS_PER_PUMP {
            let outcome = {
                let conn = self
                    .streams
                    .get_mut(&id.get())
                    .expect("stream presence checked above");
                let mut chunk = [0u8; READ_CHUNK];
                match conn.stream.read(&mut chunk) {
                    Ok(0) => Outcome::Eof,
                    Ok(read) => Outcome::Data(chunk[..read].to_vec()),
                    Err(error) if error.kind() == io::ErrorKind::WouldBlock => Outcome::Wait,
                    Err(_) => Outcome::Reset,
                }
            };
            match outcome {
                Outcome::Data(bytes) => {
                    let progress = self.core.ingest(id, bytes, false, endpoint, clock);
                    if matches!(progress, ConnectionProgress::Closed { .. }) {
                        // The socket stays: a closing rejection queued by the
                        // core must still reach the peer through flush_out.
                        return progress;
                    }
                    last = progress;
                }
                Outcome::Eof => {
                    // End of stream is terminal evidence: a truncated tail
                    // can never complete, so the core closes it silently.
                    return self.core.ingest(id, Vec::new(), true, endpoint, clock);
                }
                Outcome::Wait => break,
                Outcome::Reset => {
                    // Abrupt peer loss closes the core connection through
                    // the ordinary lane, cancelling an unacknowledged login
                    // or retiring the session, and drops the dead socket so
                    // no half-open reservation survives.
                    self.core.close(id, CloseReason::PeerGone, endpoint);
                    self.drop_stream(id);
                    return ConnectionProgress::Closed {
                        reason: CloseReason::PeerGone,
                        class: None,
                    };
                }
            }
        }
        last
    }

    /// Moves newly queued core frames onto the send queue, writes what the
    /// socket accepts, and acknowledges exactly the fully written core frames
    /// against the core ledger. Returns the fully sent core frame count with
    /// the acknowledgment progress. A send queue past its bound closes the
    /// connection as a slow receiver instead of growing without limit.
    pub fn flush_out(
        &mut self,
        id: ConnectionId,
        endpoint: &mut dyn TransportAuthority,
    ) -> (usize, ConnectionProgress) {
        let fresh = self
            .core
            .take_frames(id, FLUSH_FRAME_BUDGET, FLUSH_BYTE_BUDGET);
        let Some(conn) = self.streams.get_mut(&id.get()) else {
            return (0, self.core.ack_sent(id, 0, endpoint));
        };
        for frame in fresh {
            conn.queue.push_back(Queued::Core(frame));
        }
        if conn.queue.len() > MAX_QUEUED_SEND_FRAMES {
            return (0, self.shut_locked(id, CloseReason::SlowReceiver, endpoint));
        }
        let sent_core = conn.drain_queue();
        let progress = self.core.ack_sent(id, sent_core, endpoint);
        (sent_core, progress)
    }

    /// Drives one connection without new bytes: an in-flight login advances
    /// by one load poll and deadline expiry closes the reservation. The
    /// socket stays so queued answers remain drainable.
    pub fn poll(
        &mut self,
        id: ConnectionId,
        endpoint: &mut dyn TransportAuthority,
        clock: &dyn Clock,
    ) -> ConnectionProgress {
        self.core.poll(id, endpoint, clock)
    }

    /// Closes one connection through the ordinary lane and releases its
    /// socket. Unsent queued bytes are dropped with the socket; previously
    /// flushed bytes are already with the peer.
    pub fn close(
        &mut self,
        id: ConnectionId,
        reason: CloseReason,
        endpoint: &mut dyn TransportAuthority,
    ) {
        let _ = self.shut_locked(id, reason, endpoint);
    }

    /// Reaps a connection whose session the publication path already retired
    /// as a slow receiver. The core close tolerates the already-retired
    /// session lane, and no disconnect frame is queued: the peer observes
    /// the held frames already flushed, then end of stream. Saturation
    /// detection itself stays with the publication owner; this call owns
    /// only the close mechanics.
    pub fn close_slow_receiver(
        &mut self,
        id: ConnectionId,
        endpoint: &mut dyn TransportAuthority,
    ) -> ConnectionProgress {
        self.shut_locked(id, CloseReason::SlowReceiver, endpoint)
    }

    /// Forwards one session outbox drain to its connection socket. The drain
    /// yields raw payloads without packet identity, so the caller supplies
    /// one packet id per payload from its own publication record; the drain
    /// takes at most one payload per identity, and a short outbox against
    /// longer identities reports invalid input without queueing anything.
    /// Each envelope is framed with the protocol codec directly and bypasses
    /// the core acknowledgment ledger. The returned envelopes are the exact
    /// bytes queued for the peer. A drain the session no longer owns, or a
    /// connection without a socket, reports the peer as gone.
    pub fn forward_outbox(
        &mut self,
        id: ConnectionId,
        session: SessionKey,
        endpoint: &mut dyn TransportAuthority,
        packet_ids: &[u32],
        max_frames: usize,
        max_bytes: usize,
    ) -> io::Result<Vec<Vec<u8>>> {
        let payloads = endpoint
            .take_outbox(session, max_frames.min(packet_ids.len()), max_bytes)
            .map_err(|_| io::Error::new(io::ErrorKind::NotConnected, "session outbox is gone"))?;
        if packet_ids.len() != payloads.len() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "packet identities must match the drained payloads",
            ));
        }
        let mut envelopes = Vec::with_capacity(payloads.len());
        for (packet_id, payload) in packet_ids.iter().zip(payloads.iter()) {
            envelopes.push(
                mornlea_protocol::write_frame(*packet_id, payload).map_err(|_| {
                    io::Error::new(io::ErrorKind::InvalidInput, "payload refuses framing")
                })?,
            );
        }
        let Some(conn) = self.streams.get_mut(&id.get()) else {
            return Err(io::Error::new(
                io::ErrorKind::NotConnected,
                "connection has no socket",
            ));
        };
        for envelope in &envelopes {
            conn.queue.push_back(Queued::Outbox(envelope.clone()));
        }
        if conn.queue.len() > MAX_QUEUED_SEND_FRAMES {
            self.shut_locked(id, CloseReason::SlowReceiver, endpoint);
            return Err(io::Error::new(
                io::ErrorKind::WriteZero,
                "send queue saturated",
            ));
        }
        conn.drain_queue();
        Ok(envelopes)
    }

    /// Retained unconsumed inbound bytes for one connection, mirroring the
    /// core count. Cases use it to prove a split frame arrived before it
    /// could complete.
    pub fn retained_len(&self, id: ConnectionId) -> Option<usize> {
        self.core.retained_len(id)
    }

    /// Drains queued core frames without acknowledging them. This is an
    /// assertion seam for cases that must observe the queue after a close;
    /// it must not interleave with `flush_out` on a live connection, since
    /// taken-but-unacknowledged frames would skew the send ledger.
    pub fn pending_control_frames(&mut self, id: ConnectionId) -> Vec<Vec<u8>> {
        self.core
            .take_frames(id, FLUSH_FRAME_BUDGET, FLUSH_BYTE_BUDGET)
    }

    /// Closes the core connection and releases its socket, reporting the
    /// terminal view.
    fn shut_locked(
        &mut self,
        id: ConnectionId,
        reason: CloseReason,
        endpoint: &mut dyn TransportAuthority,
    ) -> ConnectionProgress {
        self.core.close(id, reason, endpoint);
        self.drop_stream(id);
        ConnectionProgress::Closed {
            reason,
            class: None,
        }
    }

    /// Shuts a socket down and forgets it. Shutdown errors are ignored: a
    /// dead peer needs no graceful goodbye.
    fn drop_stream(&mut self, id: ConnectionId) {
        if let Some(conn) = self.streams.remove(&id.get()) {
            let _ = conn.stream.shutdown(Shutdown::Both);
        }
    }
}
