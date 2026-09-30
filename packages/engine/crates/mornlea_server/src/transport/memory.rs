//! In-process Memory transport adapter over the shared connection core.
//!
//! This module owns the local peer's side of one in-process connection: it
//! moves owned frames into [`ConnectionCore`](super::common::ConnectionCore),
//! hands queued control frames back to the peer, and drains session frames
//! out of the S1 outbox. It owns no session, world, or publication state and
//! duplicates none of the admission logic; every decode, hello check, login
//! lifecycle step, and submit runs inside the common core.
//!
//! Wire split: the core encodes and queues its own control frames
//! (`ServerHello`, `LoginSuccess`, rejects), so the adapter never re-encodes
//! them. The adapter frames the peer's own envelopes — hello, login, and play
//! records — with the protocol codec in [`MemoryTransport::encode_frame`].
//!
//! Lifecycle: `connect` reserves one prelogin slot, `send`/`poll` advance the
//! state machine, `receive` takes queued control frames in order,
//! `acknowledge` marks the frames the peer actually consumed (the queued
//! success commits only on this acknowledgment), and `close` retires through
//! the ordinary session lane. Session frames leave separately through
//! `drain_session`, which calls only the publication port, so delivery can
//! never mutate world state. Liveness polling stays with the caller: one
//! `poll` per known connection also applies that connection's deadlines.

use std::time::Instant;

use mornlea_protocol::{ClientPacket, ProtocolError, encode_client_into, write_frame};

use crate::contracts::{
    Clock, CloseReason, ConnectionId, ConnectionProgress, ServerError, SessionKey, TransportKind,
};

use super::common::{ConnectionCore, HandshakeLimits, TransportAuthority};

/// One in-process transport endpoint over the shared connection core.
///
/// The struct holds the core and nothing else: connection handles stay with
/// the caller and authority state stays behind the per-call endpoint seam, so
/// a local caller cannot reach past owned frames.
pub struct MemoryTransport {
    core: ConnectionCore,
}

impl MemoryTransport {
    /// Opens the endpoint with the frozen source handshake limits.
    pub fn new() -> Self {
        Self {
            core: ConnectionCore::new(HandshakeLimits::source()),
        }
    }

    /// Reserves one prelogin slot for a local peer.
    pub fn connect(&mut self, now: Instant) -> Result<ConnectionId, ServerError> {
        self.core.open(TransportKind::Memory, now)
    }

    /// Offers one owned peer chunk to the shared admission path.
    pub fn send(
        &mut self,
        id: ConnectionId,
        bytes: Vec<u8>,
        endpoint: &mut dyn TransportAuthority,
        clock: &dyn Clock,
    ) -> ConnectionProgress {
        self.core.ingest(id, bytes, false, endpoint, clock)
    }

    /// Advances one connection without new bytes: login progress, retained
    /// input, and deadline expiry.
    pub fn poll(
        &mut self,
        id: ConnectionId,
        endpoint: &mut dyn TransportAuthority,
        clock: &dyn Clock,
    ) -> ConnectionProgress {
        self.core.poll(id, endpoint, clock)
    }

    /// Hands the peer up to `max_frames` queued control frames in queue order.
    /// Ownership transfers to the caller; the send ledger keeps the order for
    /// a later `acknowledge`.
    pub fn receive(
        &mut self,
        id: ConnectionId,
        max_frames: usize,
        max_bytes: usize,
    ) -> Vec<Vec<u8>> {
        self.core.take_frames(id, max_frames, max_bytes)
    }

    /// Marks `frames` taken frames as consumed by the peer. A mark covering
    /// the queued success commits the handoff exactly once and admits play.
    pub fn acknowledge(
        &mut self,
        id: ConnectionId,
        frames: usize,
        endpoint: &mut dyn TransportAuthority,
    ) -> ConnectionProgress {
        self.core.ack_sent(id, frames, endpoint)
    }

    /// Closes one connection. Queued control frames stay drainable; an
    /// unacknowledged login cancels while a committed session closes through
    /// the ordinary session lane.
    pub fn close(
        &mut self,
        id: ConnectionId,
        reason: CloseReason,
        endpoint: &mut dyn TransportAuthority,
    ) {
        self.core.close(id, reason, endpoint)
    }

    /// Retained unconsumed inbound bytes for one connection.
    pub fn retained_len(&self, id: ConnectionId) -> Option<usize> {
        self.core.retained_len(id)
    }

    /// Drains complete canonical protocol frames under explicit budgets in
    /// publication order; the undelivered suffix stays queued for the next drain. The only
    /// authority surface touched is the publication port, so delivery observes
    /// frames without mutating world state.
    pub fn drain_session(
        endpoint: &mut dyn TransportAuthority,
        session: SessionKey,
        max_frames: usize,
        max_bytes: usize,
    ) -> Result<Vec<Vec<u8>>, ServerError> {
        endpoint.take_outbox(session, max_frames, max_bytes)
    }

    /// Frames one owned client packet into its length-prefixed wire envelope:
    /// the packet's registry key names the packet id and the protocol codec
    /// writes the payload, matching the control-frame composition the core
    /// uses for its own answers.
    pub fn encode_frame(packet: &ClientPacket) -> Result<Vec<u8>, ServerError> {
        let mut buffer = vec![0u8; 64];
        let payload = loop {
            match encode_client_into(packet, &mut buffer) {
                Ok(written) => break buffer[..written].to_vec(),
                Err(ProtocolError::OutputTooSmall { needed, .. }) if needed > buffer.len() => {
                    buffer.resize(needed, 0);
                }
                Err(_) => return Err(ServerError::InvalidInput { field: "packet" }),
            }
        };
        write_frame(packet.key().id, &payload)
            .map_err(|_| ServerError::InvalidInput { field: "packet" })
    }
}

impl Default for MemoryTransport {
    fn default() -> Self {
        Self::new()
    }
}
