//! Owned tick publication and bounded slow-receiver outboxes.
//!
//! This module owns the delivery semantics the endpoint exports for one
//! tick's owned output: publishing the encoded events and control replies a
//! tick produced into per-session frame outboxes, draining a receiver's
//! outbox under explicit frame and byte budgets, and closing a receiver.
//! Publication never waits on a receiver: an outbox that reaches its frame
//! limit is closed and its session retired with the frozen slow-receiver
//! reason inside the state port, silently, without appending a Disconnect
//! frame, so one slow receiver can neither block the tick nor disturb any
//! peer's ordered delivery. World rules, tick reduction, and transport stay
//! outside this module.

use super::contracts::{CloseReason, MAX_PLAYERS, ServerError, SessionKey, TickPublication};
use super::state::{AuthorityState, encode_packet};
use mornlea_protocol::{PacketKey, ProtocolCodec, ServerPacket};
use std::{fmt, sync::Arc};

/// Publishes one tick's owned events and control replies.
///
/// Events are encoded through the protocol conversion before any append;
/// a session-directed frame lands only on its recipient and a broadcast
/// frame expands once over the sessions present at publish time. A receiver
/// whose outbox is already closed keeps nothing. When an append would exceed
/// the per-session frame limit, the overflowing frame is dropped and that
/// receiver alone is retired with the slow-receiver reason, so publishing
/// succeeds without blocking or erroring on a slow receiver. Delegates to
/// `AuthorityState::publish`.
pub fn publish_tick(
    state: &mut AuthorityState,
    publication: TickPublication,
) -> Result<(), ServerError> {
    state.publish(publication)
}

/// Drains one receiver's outbox under explicit budgets.
///
/// Complete protocol frames retain their packet IDs and leave in publication
/// order; the drain stops at whichever budget
/// bites first, always delivering at least one frame when the outbox is
/// nonempty, and the undelivered suffix stays queued for the next drain.
/// Draining a retired receiver returns the frames it already held. Delegates
/// to `AuthorityState::take_outbox`.
pub fn drain_outbox(
    state: &mut AuthorityState,
    session: SessionKey,
    max_frames: usize,
    max_bytes: usize,
) -> Result<Vec<Vec<u8>>, ServerError> {
    state.take_outbox(session, max_frames, max_bytes)
}

/// Closes one receiver's outbox without retiring the session.
///
/// Closing is idempotent and stops further appends only; the frames already
/// queued remain drainable. This is the publication-side control for a
/// receiver the endpoint has decided to stop feeding; full session
/// retirement stays on the session port. Delegates to
/// `AuthorityState::close_outbox`.
pub fn close_receiver(state: &mut AuthorityState, session: SessionKey, reason: CloseReason) {
    state.close_outbox(session, reason)
}

/// Ordered delivery positions for at most one refusal per online recipient.
pub(crate) type SourceRefusals = Vec<(usize, SessionKey)>;

/// Refusal markers are unique per recipient, so their ceiling is the player limit.
pub(crate) const MAX_SOURCE_REFUSALS: usize = MAX_PLAYERS as usize;

/// Snapshot frames the plain builder accepts. This counts encoded frames in
/// one independent CPU batch, not players: whole source passes use
/// `for_source_tick`, which multiplies the checked player and per-session
/// snapshot limits.
const PLAIN_SNAPSHOT_FRAMES: usize = 8;

/// Owns immutable semantic output paired with a bounded set of CPU snapshot frames.
/// Private fields and append-only construction prevent event/frame replacement.
pub struct PreparedSourcePublication {
    publication: TickPublication,
    snapshots: Vec<(usize, PreparedFrame)>,
    snapshot_limit: usize,
    refusals: SourceRefusals,
}

impl PreparedSourcePublication {
    pub fn new(publication: TickPublication) -> Self {
        Self {
            publication,
            snapshots: Vec::new(),
            snapshot_limit: PLAIN_SNAPSHOT_FRAMES,
            refusals: Vec::new(),
        }
    }

    /// Bounds a whole pass by checked recipient and per-recipient counts.
    /// Selection bytes and actual CPU admission retain their separate owners.
    pub fn for_source_tick(
        publication: TickPublication,
        limits: super::contracts::ServerLimits,
    ) -> Self {
        // Checked limits cap this product at eight recipients times sixty-four snapshots.
        Self {
            publication,
            snapshots: Vec::new(),
            snapshot_limit: usize::from(limits.max_players()) * limits.snapshot_chunks(),
            refusals: Vec::new(),
        }
    }

    /// Closes one source recipient after the existing immutable event prefix.
    /// Delivery preflights this marker with every frame before any append.
    pub fn append_source_refusal(&mut self, session: SessionKey) -> Result<(), ServerError> {
        self.refuse_at(self.publication.events.len(), session)
    }

    /// Translates a projector position without replacing any event or CPU frame.
    pub(crate) fn refuse_at(
        &mut self,
        before: usize,
        session: SessionKey,
    ) -> Result<(), ServerError> {
        if before > self.publication.events.len()
            || self.refusals.len() >= MAX_SOURCE_REFUSALS
            || self.refusals.iter().any(|(_, owner)| *owner == session)
            || self
                .refusals
                .last()
                .is_some_and(|(position, _)| *position > before)
        {
            return Err(ServerError::InvalidInput {
                field: "source_publication",
            });
        }
        self.refusals.push((before, session));
        Ok(())
    }

    pub fn publication(&self) -> &TickPublication {
        &self.publication
    }

    pub fn append_event(&mut self, event: mornlea_domain::RoutedEvent) {
        self.publication.events.push(event);
    }

    pub fn append_encoded_snapshot(
        &mut self,
        session: SessionKey,
        expected: &super::world::ChunkSaveView,
        encoded: super::chunk_encoding::EncodedChunkSnapshot,
    ) -> Result<(), ServerError> {
        // Token refusal precedes capacity and neither can change the batch.
        // The caller separately owns current source relevance and selection.
        if encoded.capture() != expected {
            return Err(ServerError::InvalidInput {
                field: "chunk_encode_capture",
            });
        }
        if self.snapshots.len() >= self.snapshot_limit {
            return Err(ServerError::Capacity {
                resource: super::contracts::Resource::Snapshots,
                limit: self.snapshot_limit,
                observed: self.snapshots.len() + 1,
            });
        }
        let index = self.publication.events.len();
        let (_, snapshot, _, frame) = encoded.into_source_parts();
        self.publication
            .events
            .push(mornlea_domain::RoutedEvent::new(
                mornlea_domain::EventRecipient::Session(session.get()),
                mornlea_domain::Event::ChunkSnapshot(snapshot),
            ));
        self.snapshots.push((index, frame));
        Ok(())
    }

    /// Moves semantic output without delivering frames or refusals, including final reduction.
    pub fn into_publication(self) -> TickPublication {
        self.publication
    }

    pub(crate) fn into_parts(
        self,
    ) -> (TickPublication, Vec<(usize, PreparedFrame)>, SourceRefusals) {
        (self.publication, self.snapshots, self.refusals)
    }
}

/// Immutable canonical frame prepared off tick; clones share its owned bytes.
///
/// A frame retains no codec, decoded packet or authority borrow. Construction
/// validates through the existing protocol encoder before publishing an owner.
#[derive(Clone)]
pub struct PreparedFrame(Arc<PreparedFrameInner>);

struct PreparedFrameInner {
    key: PacketKey,
    bytes: Vec<u8>,
}

impl PreparedFrame {
    /// Encodes and frames one checked packet off tick using the caller's codec.
    ///
    /// Codec or framing refusal returns the existing packet input error without
    /// returning a partial owner. Negotiation packets are valid factory inputs;
    /// the publication port admits only server-to-client Play frames.
    pub fn encode(codec: &mut ProtocolCodec, packet: &ServerPacket) -> Result<Self, ServerError> {
        let bytes = encode_packet(codec, packet)?;
        Ok(Self(Arc::new(PreparedFrameInner {
            key: packet.key(),
            bytes,
        })))
    }

    pub fn packet_key(&self) -> PacketKey {
        self.0.key
    }

    pub fn byte_len(&self) -> usize {
        self.0.bytes.len()
    }

    pub fn as_bytes(&self) -> &[u8] {
        &self.0.bytes
    }

    /// Observes legacy bytes off tick, transferring a unique allocation or
    /// explicitly copying a shared one. Actual prepared publication and transport
    /// paths must transfer frame owners and must never call this compatibility
    /// observation; it grants no on-tick cloning exemption.
    pub(crate) fn into_legacy_bytes(self) -> Vec<u8> {
        match Arc::try_unwrap(self.0) {
            Ok(inner) => inner.bytes,
            Err(shared) => shared.bytes.clone(),
        }
    }
}

// Diagnostics expose identity and size without scanning or disclosing bodies.
impl fmt::Debug for PreparedFrame {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PreparedFrame")
            .field("packet_key", &self.packet_key())
            .field("byte_len", &self.byte_len())
            .finish()
    }
}

/// Receipt for this exact owner, not socket flush, decode or durable success.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EnqueueOutcome {
    Queued,
    Closed,
}

/// Transfers immutable frame owners through one per-session FIFO.
///
/// AuthorityState owns queue capacity, current membership and retirement.
/// Consumer doubles prove caller semantics; actual provider and transport
/// acceptance require their separate real-owner evidence.
pub trait PreparedPublicationPort {
    /// Rejects non-server Play keys as packet input errors before queue mutation.
    /// An unknown session is stale. Only an exact Active session with an open
    /// outbox appends and returns Queued; mirror revisions advance only then.
    /// Closed or nonactive retained sessions return Closed without mutation.
    /// Saturation retires only that receiver with SlowReceiver, drops the
    /// overflowing owner and returns Closed, retaining previously queued frames.
    fn enqueue_prepared(
        &mut self,
        session: SessionKey,
        frame: PreparedFrame,
    ) -> Result<EnqueueOutcome, ServerError>;

    /// Transfers whole frames in FIFO order without encoding or copying bytes.
    /// Zero max_frames takes none; otherwise the first whole frame may exceed
    /// max_bytes, including zero. A later first nonfit stops without skipping.
    /// Unknown sessions are stale; closed or retired receivers may still drain.
    fn take_prepared_outbox(
        &mut self,
        session: SessionKey,
        max_frames: usize,
        max_bytes: usize,
    ) -> Result<Vec<PreparedFrame>, ServerError>;
}

#[cfg(test)]
mod prepared_frame_tests {
    use super::*;
    use mornlea_protocol::CommandRejected;

    fn frame() -> PreparedFrame {
        PreparedFrame::encode(
            &mut ProtocolCodec::new().unwrap(),
            &ServerPacket::CommandRejected(CommandRejected::new(808, 1).unwrap()),
        )
        .unwrap()
    }

    #[test]
    fn legacy_observation_transfers_unique_allocation_off_tick() {
        let frame = frame();
        let pointer = frame.as_bytes().as_ptr();
        let expected = frame.as_bytes().to_vec();
        let bytes = frame.into_legacy_bytes();
        assert_eq!(bytes.as_ptr(), pointer);
        assert_eq!(bytes, expected);
    }

    #[test]
    fn legacy_observation_explicitly_copies_shared_allocation_off_tick() {
        let frame = frame();
        let retained = frame.clone();
        let pointer = retained.as_bytes().as_ptr();
        let key = retained.packet_key();
        let bytes = frame.into_legacy_bytes();
        assert_ne!(bytes.as_ptr(), pointer);
        assert_eq!(bytes, retained.as_bytes());
        assert_eq!(retained.as_bytes().as_ptr(), pointer);
        assert_eq!(retained.packet_key(), key);
    }
}

#[cfg(test)]
#[path = "publication_source_budget.rs"]
mod source_budget_tests;
