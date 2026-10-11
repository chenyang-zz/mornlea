//! The client-side I/O provider: the bounded Memory queue capability and the
//! numeric-IP TCP connector behind the frozen ticket/connector ports.
//!
//! Both capabilities expose exactly the landed `Connector` substitution port
//! — `try_connect`/`poll`/`try_send`/`close` over `TransportTicket` — and add
//! no new port, clock or decoder: inbound bytes are framed only through the
//! landed `mornlea_protocol` framer, and the injected `MonotonicClock`
//! remains the only clock the core step sees. The provider owns one worker
//! thread per TCP launch; the worker's bounded waits (the dial wait, then
//! socket read/write pacing) live entirely outside the core step, which never
//! blocks on the network.
//!
//! Ownership and bounds (the frozen capability-inventory rows, verbatim
//! through `ClientLimits`): the inbound receiver queue admits at most 8192
//! records and 8 MiB of owned frame bytes; the outbound queue admits at most
//! 4104 records and 8 MiB; a declared frame body over the accepted 2 MiB
//! `mornlea_protocol` body cap rejects with the typed `Capacity` error; the
//! per-step drain honors the 4096 message work budget. Every bound admits
//! exactly its frozen value and rejects the next record with
//! `ClientError::Capacity` before any allocation or partial admission: the
//! record and byte charge is reserved before a frame enters a queue, and a
//! refusal leaves both queues and the retained frame unchanged.
//!
//! Receive semantics: raw bytes stage in a partial-receive buffer bounded by
//! one incomplete frame plus one complete-but-unadmitted frame, and a frame
//! surfaces through `poll` only when its complete prefix-inclusive body has
//! arrived — no partial observation ever reaches the session. A framing-level
//! malformation (an empty length declaration, an uncompletable length prefix)
//! or an oversized declared body fails the receive stream with the typed
//! error, stickily: a length-prefixed stream cannot resynchronize, so the
//! stream closes instead of silently dropping bytes mid-frame.
//!
//! Send semantics: `try_send` admits exactly one complete prefix-inclusive
//! frame per call — a partial or trailing-byte slice is invalid input and an
//! oversized declared body is the capacity error — and on `Capacity` the
//! caller retains the entire head for retry, never a partial record.
//!
//! Ticket lifecycle: `try_connect` issues one nonzero launch generation per
//! transport; `close` releases exactly once, drops that generation's queues
//! and partial receive state, and every later operation on the old ticket
//! rejects with `ClientError::InvalidState`, so no old-epoch packet can
//! publish after a reset. Full reset/close lifecycle conformance belongs to
//! the later lifecycle node.

use std::collections::{BTreeMap, VecDeque};
use std::io::{ErrorKind, Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::num::NonZeroU64;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::time::Duration;

use mornlea_protocol::{MAX_FRAME_BYTES, ProtocolError, read_frame_ref};

use crate::contracts::{
    ClientError, ClientIdentity, ClientLimits, ClientWorkBudget, Connector, Endpoint,
    TransportLaunch, TransportPoll, TransportTicket,
};

/// The accepted protocol body cap, as the staged-byte unit it is here. The
/// value is the landed framer's own cap, not a second ceiling.
const MAX_BODY: usize = MAX_FRAME_BYTES as usize;

/// The receive staging ceiling: one incomplete frame plus one complete but
/// unadmitted frame, each at most a maximum body plus its five-byte prefix.
/// Reads stop when staging is full, so a peer cannot grow the buffer without
/// bound while the inbound queue sits at its record or byte budget.
const RECEIVE_STAGING_MAX: usize = 2 * (5 + MAX_BODY);

/// The TCP worker's socket pacing: the read and write wait per iteration and
/// the condvar park between them. Every wait in the worker is bounded by this
/// value, so cancellation (close, connector drop) is observed within one
/// pacing interval.
const TCP_PARK: Duration = Duration::from_millis(5);

/// The largest socket read chunk per worker iteration, capped by the receive
/// staging headroom so a single read cannot push staging past its ceiling.
const TCP_READ_CHUNK: usize = 64 * 1024;

/// What the landed framer says about the head of the receive staging buffer.
enum Scan {
    /// A complete prefix-inclusive frame of exactly this many bytes is ready.
    Complete(usize),
    /// The buffer holds a fragment; more bytes are required.
    NeedMore,
    /// The stream is permanently malformed or oversized; the typed error is
    /// the sticky receive failure from now on.
    Failed(ClientError),
}

/// Scans one frame from the head of the receive staging through the landed
/// framer. There is no second wire decoder here: `read_frame_ref` already
/// classifies an incomplete prefix or body as `Truncated`, a declared body
/// over the accepted 2 MiB cap as `FrameTooLarge`, and an empty or
/// uncanonical length declaration as a protocol violation.
fn scan_complete_frame(buffer: &[u8]) -> Scan {
    match read_frame_ref(buffer) {
        Ok(frame) => Scan::Complete(frame.consumed),
        Err(ProtocolError::Truncated) => Scan::NeedMore,
        // The declared body is over the accepted frame cap: the byte bound
        // rejects, exactly one byte beyond the admitted maximum.
        Err(ProtocolError::FrameTooLarge) => Scan::Failed(ClientError::Capacity),
        Err(_) => Scan::Failed(ClientError::InvalidInput),
    }
}

/// One live transport connection: the assembled inbound queue, the bounded
/// outbound queue and the partial receive state. All fields sit behind one
/// lock, so every port operation observes a consistent queue pair and the
/// record/byte charges cannot drift from the queues they describe.
struct ConnectionState {
    connected: bool,
    connected_reported: bool,
    inbound: VecDeque<Vec<u8>>,
    inbound_bytes: usize,
    outbound: VecDeque<Vec<u8>>,
    outbound_bytes: usize,
    /// The write cursor inside the outbound head for the TCP worker: the
    /// bytes before it are already on the socket, while the head itself stays
    /// whole until its last byte is written.
    outbound_cursor: usize,
    receive: Vec<u8>,
    /// The sticky receive failure; once set, every poll reports it.
    failure: Option<ClientError>,
    /// The one-way release flag: a closed connection's worker stops and its
    /// queues and partial receive state are gone with the ticket.
    closed: bool,
}

impl ConnectionState {
    fn new() -> Self {
        Self {
            connected: false,
            connected_reported: false,
            inbound: VecDeque::new(),
            inbound_bytes: 0,
            outbound: VecDeque::new(),
            outbound_bytes: 0,
            outbound_cursor: 0,
            receive: Vec::new(),
            failure: None,
            closed: false,
        }
    }

    /// Moves complete frames from the receive staging into the bounded
    /// inbound queue, one reservation at a time. A complete frame that cannot
    /// reserve its record or byte charge stays retained, intact, at the head
    /// of the staging; a framing-level failure becomes the sticky receive
    /// failure. This is the only path a frame enters the inbound queue, so
    /// the Memory capability and the TCP worker admit through identical
    /// rules.
    fn admit_staged(&mut self, limits: &ClientLimits) -> Result<(), ClientError> {
        loop {
            match scan_complete_frame(&self.receive) {
                Scan::NeedMore => return Ok(()),
                Scan::Failed(error) => {
                    self.failure = Some(error);
                    return Err(error);
                }
                Scan::Complete(total) => {
                    if self.inbound.len() + 1 > limits.inbound_observations()
                        || self.inbound_bytes + total > limits.inbound_bytes()
                    {
                        // The bound rejects before any allocation or partial
                        // admission; the complete frame is retained for the
                        // drain that frees its charge.
                        return Err(ClientError::Capacity);
                    }
                    let frame: Vec<u8> = self.receive.drain(..total).collect();
                    self.inbound_bytes += total;
                    self.inbound.push_back(frame);
                }
            }
        }
    }

    /// Reserves the record and byte charge for one complete frame and
    /// enqueues it on the outbound queue. The reservation runs before the
    /// copy, so a refusal leaves the queue and its charges unchanged.
    fn reserve_outbound(&mut self, limits: &ClientLimits, frame: &[u8]) -> Result<(), ClientError> {
        if self.outbound.len() + 1 > limits.outbound_commands()
            || self.outbound_bytes + frame.len() > limits.outbound_bytes()
        {
            return Err(ClientError::Capacity);
        }
        self.outbound_bytes += frame.len();
        self.outbound.push_back(frame.to_vec());
        Ok(())
    }
}

/// Feeds raw receive bytes into one connection's staging and admits every
/// resolvable complete frame.
///
/// `Ok(consumed)` reports how many input bytes were staged; a remainder means
/// the staging ceiling was reached and the caller re-feeds exactly the
/// unconsumed tail. `Err(Capacity)` means every input byte was staged and at
/// least one complete frame is retained unadmitted — the recovery is draining
/// `poll`, which admits retained frames as their charge frees, never
/// re-feeding. Any other error is the sticky receive failure.
fn feed_receive(
    state: &mut ConnectionState,
    limits: &ClientLimits,
    input: &[u8],
) -> Result<usize, ClientError> {
    if let Some(error) = state.failure {
        return Err(error);
    }
    let staged = RECEIVE_STAGING_MAX
        .saturating_sub(state.receive.len())
        .min(input.len());
    state.receive.extend_from_slice(&input[..staged]);
    state.admit_staged(limits).map(|()| staged)
}

/// One polled observation, applying the shared result order: admitted frames
/// first (they are real observations), then the sticky failure, then the
/// one-time establishment report, then idle.
fn poll_state(state: &mut ConnectionState, limits: ClientLimits) -> TransportPoll {
    // Retained frames admit as their charge frees; a framing-level failure
    // becomes sticky here exactly as it does at feed time.
    let _ = state.admit_staged(&limits);
    if let Some(frame) = state.inbound.pop_front() {
        state.inbound_bytes -= frame.len();
        return TransportPoll::Frame(frame);
    }
    if let Some(error) = state.failure {
        return TransportPoll::Closed(error);
    }
    if state.connected && !state.connected_reported {
        state.connected_reported = true;
        return TransportPoll::Connected;
    }
    TransportPoll::Pending
}

/// The bounded FIFO work drain the session wiring drives once per step.
///
/// At most `work.messages()` complete frames dequeue per call, an idle or
/// connected transport stops the drain instead of spinning, and a transport
/// that reports closed surfaces its typed error. The work budget is rechecked
/// against the frozen per-step ceiling before the first dequeue, so an
/// over-budget demand rejects with the typed `Capacity` error before any
/// frame leaves its queue.
pub fn drain_frames(
    connector: &dyn Connector,
    ticket: TransportTicket,
    work: ClientWorkBudget,
) -> Result<Vec<Vec<u8>>, ClientError> {
    if usize::from(work.messages()) > usize::from(ClientWorkBudget::MAX_PER_STEP) {
        return Err(ClientError::Capacity);
    }
    let mut drained = Vec::new();
    while drained.len() < usize::from(work.messages()) {
        match connector.poll(ticket) {
            TransportPoll::Frame(bytes) => drained.push(bytes),
            TransportPoll::Pending | TransportPoll::Connected => break,
            TransportPoll::Closed(error) => return Err(error),
        }
    }
    Ok(drained)
}

/// The shared half of the bounded Memory queue capability: the launch
/// counter, one slot per live launch generation, and the generation the
/// fixture peer currently feeds. Closing a slot drops it from the map, which
/// is exactly the old-ticket and partial-receive invalidation.
struct MemoryShared {
    limits: ClientLimits,
    launch: Mutex<TransportLaunch>,
    slots: Mutex<BTreeMap<u64, ConnectionState>>,
    current: Mutex<u64>,
}

/// The bounded in-process Memory transport capability.
///
/// `try_connect` establishes immediately and issues a fresh launch
/// generation; the fixture peer delivers raw receive bytes (the same partial
/// framing rules as TCP) and observes the outbound queue, so a Memory
/// transcript and a TCP transcript of the same bytes admit the same frames
/// through the same admission code.
pub struct MemoryConnector {
    connector_id: NonZeroU64,
    shared: Arc<MemoryShared>,
}

/// The fixture peer of one Memory capability: the far end of the current
/// launch generation's transport.
pub struct MemoryPeer {
    shared: Arc<MemoryShared>,
}

impl MemoryConnector {
    /// Builds the capability and its peer under one connector id. The pair is
    /// the registration unit: the returned connector goes into the injected
    /// `ConnectorRegistry` and the peer stays with the test or fixture that
    /// drives the far end.
    pub fn pair(limits: ClientLimits, connector_id: NonZeroU64) -> (Arc<Self>, MemoryPeer) {
        let connector = Arc::new(Self {
            connector_id,
            shared: Arc::new(MemoryShared {
                limits,
                launch: Mutex::new(TransportLaunch::new()),
                slots: Mutex::new(BTreeMap::new()),
                current: Mutex::new(0),
            }),
        });
        let peer = MemoryPeer {
            shared: Arc::clone(&connector.shared),
        };
        (connector, peer)
    }

    /// The live state of one ticket's launch generation, or the typed
    /// invalid-state rejection for a foreign or released ticket.
    fn with_state<T>(
        &self,
        ticket: TransportTicket,
        on_state: impl FnOnce(&mut ConnectionState, ClientLimits) -> T,
    ) -> Result<T, ClientError> {
        if ticket.connector_id() != self.connector_id {
            return Err(ClientError::InvalidState);
        }
        let mut slots = self.shared.slots.lock().expect("memory slot mutex");
        let Some(state) = slots.get_mut(&ticket.generation().get()) else {
            return Err(ClientError::InvalidState);
        };
        Ok(on_state(state, self.shared.limits))
    }
}

impl Connector for MemoryConnector {
    fn try_connect(
        &self,
        endpoint: &Endpoint,
        _identity: &ClientIdentity,
    ) -> Result<TransportTicket, ClientError> {
        // The registry already resolved this capability's id; a TCP endpoint
        // is not this capability's transport and is refused rather than
        // invented. The login identity is the login owner's concern: the
        // transport carries it nowhere.
        match endpoint {
            Endpoint::Memory { .. } => {}
            Endpoint::Tcp(_) => return Err(ClientError::InvalidInput),
        }
        let generation = self
            .shared
            .launch
            .lock()
            .expect("memory launch mutex")
            .advance()?;
        // The in-process transport is established the moment its slot exists;
        // the first poll reports the establishment exactly once.
        let mut state = ConnectionState::new();
        state.connected = true;
        let mut slots = self.shared.slots.lock().expect("memory slot mutex");
        slots.insert(generation.get(), state);
        drop(slots);
        *self.shared.current.lock().expect("memory current mutex") = generation.get();
        TransportTicket::try_new(self.connector_id, generation)
    }

    fn poll(&self, ticket: TransportTicket) -> TransportPoll {
        self.with_state(ticket, poll_state)
            .unwrap_or(TransportPoll::Closed(ClientError::InvalidState))
    }

    fn try_send(&self, ticket: TransportTicket, frame: &[u8]) -> Result<(), ClientError> {
        self.with_state(ticket, |state, limits| {
            if let Some(error) = state.failure {
                return Err(error);
            }
            match scan_complete_frame(frame) {
                // Exactly one complete frame per call: trailing bytes after a
                // complete frame are as invalid as a partial encode output.
                Scan::Complete(total) if total == frame.len() => {}
                Scan::Failed(error) => return Err(error),
                Scan::Complete(_) | Scan::NeedMore => return Err(ClientError::InvalidInput),
            }
            state.reserve_outbound(&limits, frame)
        })?
    }

    fn close(&self, ticket: TransportTicket) -> Result<(), ClientError> {
        // The release drops the generation's queues and partial receive
        // state; the old ticket can never resolve again, and a second release
        // of the same ticket is the typed invalid state.
        if ticket.connector_id() != self.connector_id {
            return Err(ClientError::InvalidState);
        }
        self.shared
            .slots
            .lock()
            .expect("memory slot mutex")
            .remove(&ticket.generation().get())
            .ok_or(ClientError::InvalidState)?;
        Ok(())
    }
}

impl MemoryPeer {
    /// Delivers raw receive bytes to the current launch generation. The
    /// return contract is `feed_receive`'s: `Ok(consumed)` with a possible
    /// unconsumed tail at the staging ceiling, `Err(Capacity)` when a
    /// complete frame is retained unadmitted, and any other error as the
    /// sticky receive failure.
    pub fn feed(&self, bytes: &[u8]) -> Result<usize, ClientError> {
        let current = *self.shared.current.lock().expect("memory current mutex");
        let mut slots = self.shared.slots.lock().expect("memory slot mutex");
        let Some(state) = slots.get_mut(&current) else {
            return Err(ClientError::InvalidState);
        };
        let limits = self.shared.limits;
        feed_receive(state, &limits, bytes)
    }

    /// The outbound record count waiting for the peer.
    pub fn sent_depth(&self) -> usize {
        let current = *self.shared.current.lock().expect("memory current mutex");
        let slots = self.shared.slots.lock().expect("memory slot mutex");
        slots.get(&current).map_or(0, |state| state.outbound.len())
    }

    /// Takes every queued outbound record in FIFO order, releasing its charge
    /// the way a far-end socket read does.
    pub fn drain_sent(&self) -> Vec<Vec<u8>> {
        let current = *self.shared.current.lock().expect("memory current mutex");
        let mut slots = self.shared.slots.lock().expect("memory slot mutex");
        let Some(state) = slots.get_mut(&current) else {
            return Vec::new();
        };
        let drained: Vec<Vec<u8>> = state.outbound.drain(..).collect();
        state.outbound_bytes = 0;
        state.outbound_cursor = 0;
        drained
    }
}

/// The shared half of one TCP launch: its connection state and the wake
/// signal the port operations use to cancel the worker's bounded park.
struct TcpShared {
    state: Mutex<ConnectionState>,
    wake: Condvar,
}

/// The numeric-IP TCP transport capability.
///
/// `Endpoint::Tcp` carries an already-resolved `SocketAddr`, so no DNS lookup
/// can block anywhere in this capability. Each `try_connect` issues one
/// launch generation and one worker thread; the worker owns the socket and
/// performs every blocking network wait (a bounded dial, then bounded
/// read/write pacing), while the port operations touch only the bounded
/// queues. `try_send` enqueues complete frames for the worker; a socket error
/// fails the transport with the typed `Io` error and the peer's end-of-file
/// with `Disconnected`, both sticky, with the not-yet-written head retained.
pub struct TcpConnector {
    connector_id: NonZeroU64,
    limits: ClientLimits,
    connect_wait: Duration,
    inner: Mutex<TcpInner>,
    /// Cancels every worker when the capability goes away, so no parking
    /// worker outlives its connector.
    alive: Arc<AtomicBool>,
}

struct TcpInner {
    launch: TransportLaunch,
    connections: BTreeMap<u64, Arc<TcpShared>>,
}

impl TcpConnector {
    /// Builds the capability under one connector id with the bounded dial
    /// wait its workers use. The wait bounds each connection attempt; it
    /// never bounds or delays a core step, which sees only the queued state.
    pub fn new(
        connector_id: NonZeroU64,
        limits: ClientLimits,
        connect_wait: Duration,
    ) -> Result<Self, ClientError> {
        if connect_wait.is_zero() {
            return Err(ClientError::InvalidInput);
        }
        Ok(Self {
            connector_id,
            limits,
            connect_wait,
            inner: Mutex::new(TcpInner {
                launch: TransportLaunch::new(),
                connections: BTreeMap::new(),
            }),
            alive: Arc::new(AtomicBool::new(true)),
        })
    }

    /// The shared state of one ticket's launch generation, or the typed
    /// invalid-state rejection for a foreign or released ticket.
    fn lookup(&self, ticket: TransportTicket) -> Result<Arc<TcpShared>, ClientError> {
        if ticket.connector_id() != self.connector_id {
            return Err(ClientError::InvalidState);
        }
        self.inner
            .lock()
            .expect("tcp inner mutex")
            .connections
            .get(&ticket.generation().get())
            .cloned()
            .ok_or(ClientError::InvalidState)
    }
}

impl Connector for TcpConnector {
    fn try_connect(
        &self,
        endpoint: &Endpoint,
        _identity: &ClientIdentity,
    ) -> Result<TransportTicket, ClientError> {
        // Only the resolved numeric-IP endpoint is this capability's
        // transport; a memory endpoint belongs to the registry's memory
        // connector and is refused here rather than misrouted.
        let Endpoint::Tcp(address) = endpoint else {
            return Err(ClientError::InvalidInput);
        };
        let mut inner = self.inner.lock().expect("tcp inner mutex");
        let generation = inner.launch.advance()?;
        let shared = Arc::new(TcpShared {
            state: Mutex::new(ConnectionState::new()),
            wake: Condvar::new(),
        });
        inner
            .connections
            .insert(generation.get(), Arc::clone(&shared));
        drop(inner);
        // The worker owns every network wait; the ticket returns before the
        // dial completes, which is the frozen nonblocking connect contract.
        let spawned = thread::Builder::new()
            .name(format!("mornlea-client-tcp-{}", generation.get()))
            .spawn({
                let shared = Arc::clone(&shared);
                let alive = Arc::clone(&self.alive);
                let limits = self.limits;
                let connect_wait = self.connect_wait;
                let address = *address;
                move || tcp_worker(address, shared, limits, connect_wait, alive)
            });
        if spawned.is_err() {
            // A worker that never launched is an internal resource failure:
            // the ticket is withdrawn before it is ever returned.
            self.inner
                .lock()
                .expect("tcp inner mutex")
                .connections
                .remove(&generation.get());
            return Err(ClientError::Internal);
        }
        TransportTicket::try_new(self.connector_id, generation)
    }

    fn poll(&self, ticket: TransportTicket) -> TransportPoll {
        let Ok(shared) = self.lookup(ticket) else {
            return TransportPoll::Closed(ClientError::InvalidState);
        };
        let mut state = shared.state.lock().expect("tcp state mutex");
        poll_state(&mut state, self.limits)
    }

    fn try_send(&self, ticket: TransportTicket, frame: &[u8]) -> Result<(), ClientError> {
        let shared = self.lookup(ticket)?;
        let reserved = {
            let mut state = shared.state.lock().expect("tcp state mutex");
            if let Some(error) = state.failure {
                return Err(error);
            }
            match scan_complete_frame(frame) {
                Scan::Complete(total) if total == frame.len() => {}
                Scan::Failed(error) => return Err(error),
                Scan::Complete(_) | Scan::NeedMore => return Err(ClientError::InvalidInput),
            }
            state.reserve_outbound(&self.limits, frame)
        };
        // Wake the worker so the queued head is written without waiting out
        // the current bounded park.
        shared.wake.notify_all();
        reserved
    }

    fn close(&self, ticket: TransportTicket) -> Result<(), ClientError> {
        let shared = self.lookup(ticket)?;
        // Cancellation outranks queued work: the outbound queue and the
        // partial receive state are dropped, the worker observes the release
        // through the shared flag, and a second release is invalid state.
        self.inner
            .lock()
            .expect("tcp inner mutex")
            .connections
            .remove(&ticket.generation().get())
            .ok_or(ClientError::InvalidState)?;
        {
            let mut state = shared.state.lock().expect("tcp state mutex");
            state.closed = true;
            state.outbound.clear();
            state.outbound_bytes = 0;
            state.outbound_cursor = 0;
            state.receive.clear();
        }
        shared.wake.notify_all();
        Ok(())
    }
}

impl Drop for TcpConnector {
    fn drop(&mut self) {
        // Detached cancellation: workers observe the flag within one bounded
        // park and exit; no join ever waits on their network I/O.
        self.alive.store(false, Ordering::Release);
        let inner = self.inner.lock().expect("tcp inner mutex");
        for shared in inner.connections.values() {
            shared.wake.notify_all();
        }
    }
}

/// One TCP worker: dial with the bounded wait, then pace reads, writes and
/// cancellation under the same bounded wait. The worker is the only owner of
/// the socket; it shares only the locked connection state, and no network
/// I/O, decoding or callback ever runs under that lock.
fn tcp_worker(
    address: SocketAddr,
    shared: Arc<TcpShared>,
    limits: ClientLimits,
    connect_wait: Duration,
    alive: Arc<AtomicBool>,
) {
    let mut stream = match TcpStream::connect_timeout(&address, connect_wait) {
        Ok(stream) => stream,
        // A refused or unreachable dial is the transport I/O failure; the
        // ticket's polls report it stickily.
        Err(_) => {
            set_failure(&shared, ClientError::Io);
            return;
        }
    };
    let paced = stream
        .set_read_timeout(Some(TCP_PARK))
        .and_then(|()| stream.set_write_timeout(Some(TCP_PARK)))
        .is_ok();
    if !paced {
        set_failure(&shared, ClientError::Io);
        return;
    }
    {
        let mut state = shared.state.lock().expect("tcp state mutex");
        // The establishment is reported once through poll; a cancellation
        // that arrived during the dial reports nothing and stops the worker.
        if state.closed {
            return;
        }
        state.connected = true;
    }
    let mut chunk = vec![0u8; TCP_READ_CHUNK];
    loop {
        if !alive.load(Ordering::Acquire) {
            return;
        }
        let (writable_head, read_budget) = {
            let state = shared.state.lock().expect("tcp state mutex");
            if state.closed || state.failure.is_some() {
                return;
            }
            // Backpressure: read only while the inbound queue has a record
            // slot and staging has headroom; the peer's kernel buffer holds
            // the rest, exactly as a full socket does.
            let read_budget =
                TCP_READ_CHUNK.min(RECEIVE_STAGING_MAX.saturating_sub(state.receive.len()));
            (
                state
                    .outbound
                    .front()
                    .and_then(|head| head.get(state.outbound_cursor..))
                    .map(<[u8]>::to_vec),
                read_budget,
            )
        };
        // Write the head first so cancellation and inbound backpressure
        // never delay the already-accepted outbound records. The slice is a
        // bounded copy taken outside the lock; the head itself stays queued
        // until its last byte is written, so a refused or failed write
        // retains the complete record.
        let mut progressed = false;
        if let Some(bytes) = writable_head {
            match stream.write(&bytes) {
                Ok(0) => {
                    set_failure(&shared, ClientError::Io);
                    return;
                }
                Ok(written) => {
                    progressed = true;
                    let mut state = shared.state.lock().expect("tcp state mutex");
                    state.outbound_cursor += written;
                    if let Some(head) = state.outbound.front()
                        && state.outbound_cursor >= head.len()
                    {
                        state.outbound_bytes -= head.len();
                        state.outbound.pop_front();
                        state.outbound_cursor = 0;
                    }
                }
                Err(error) if is_pacing(error.kind()) => {}
                Err(_) => {
                    set_failure(&shared, ClientError::Io);
                    return;
                }
            }
        }
        if read_budget > 0 {
            match stream.read(&mut chunk[..read_budget]) {
                // End of file: the peer closed its end; the typed error is
                // the transport disconnect the session maps to a remote
                // disconnect terminal.
                Ok(0) => {
                    set_failure(&shared, ClientError::Disconnected);
                    return;
                }
                Ok(read) => {
                    progressed = true;
                    let mut state = shared.state.lock().expect("tcp state mutex");
                    // A framing-level failure or an at-bound queue keeps the
                    // staged bytes; both surface through poll.
                    let _ = feed_receive(&mut state, &limits, &chunk[..read]);
                }
                Err(error) if is_pacing(error.kind()) => {}
                Err(_) => {
                    set_failure(&shared, ClientError::Io);
                    return;
                }
            }
        }
        if !progressed {
            // Idle iteration: park on the wake signal so a queued head, a
            // close or a connector drop resumes the worker immediately.
            let guard = shared.state.lock().expect("tcp state mutex");
            let _ = shared.wake.wait_timeout(guard, TCP_PARK);
        }
    }
}

/// The pacing error kinds: a bounded wait elapsed, the socket would block, or
/// a signal interrupted the call. All three mean "try again on the next
/// iteration", never a transport failure.
fn is_pacing(kind: ErrorKind) -> bool {
    matches!(
        kind,
        ErrorKind::WouldBlock | ErrorKind::TimedOut | ErrorKind::Interrupted
    )
}

/// Sets the sticky transport failure under the lock.
fn set_failure(shared: &Arc<TcpShared>, error: ClientError) {
    let mut state = shared.state.lock().expect("tcp state mutex");
    if state.failure.is_none() {
        state.failure = Some(error);
    }
}
