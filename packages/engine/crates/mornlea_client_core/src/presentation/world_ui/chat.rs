//! The chat world UI projection.
//!
//! This provider turns the confirmed chat facts of the committed observation
//! queue into ordered `world-ui@1` records: one confirmed chat publication
//! projects exactly one upsert carrying the exact checked `ChatEvent` value
//! the authority published — the actual sender identity and canonical name
//! beside the closed body union — through the chat topic tag with the actual
//! observation identity the mirror retained for the carrying publication.
//! The record is never a delta and never a locally derived value: the
//! projection carries the checked domain value verbatim, and every text
//! bound (the player and companion names at most 32 scalars and 128 bytes,
//! the command 1024 bytes, the speech line 256 bytes) was already refused
//! plus-one typed at the checked domain constructors, while invalid UTF-8 is
//! refused at the checked wire decoder before any domain value can exist.
//! Chat carries no server tick, so the retained observation's absent tick
//! stays absent and none is invented here.
//!
//! The bounded window: the pilot client's chat event ring keeps the most
//! recent 32 confirmed events in event order, and this projection exposes
//! the same window of the retained confirmed facts — at most the most recent
//! ring capacity in source order, nothing dropped below the capacity and the
//! oldest evicted above it. The window admits only a strictly newer event
//! identity: a duplicate resend of an already-accepted identity and a stale
//! identity that is not newer than the last accepted one never double-emit
//! and leave the projected output unchanged, exactly as the pilot ring
//! refuses an id not newer than its last accepted one. The confirmed
//! chat store behind the mirror accumulates one entry per accepted chat
//! publication for the session lifetime; whether that store is trimmed is
//! the mirror owner's concern, and this provider owns only the window its
//! output exposes.
//!
//! Ownership boundaries: the projection is pure over the immutable
//! `ProjectionView` and owns no second mirror, no packet history and no
//! presentation state. A sequence-bound command rejection is not a chat
//! fact and projects nowhere here — the crafting provider owns the
//! family-wide rejected record — and every other publication belongs to its
//! own per-topic provider and is ignored wholesale. A malformed observation
//! identity — a packet that is not a checked event publication or an
//! observation from another epoch — rejects the whole projection before any
//! record is returned, so no partial output exists.

use mornlea_domain::Event;

use crate::contracts::{ClientError, FamilyOperation, ObservationKey, RecordHeader};
use crate::presentation::frame::WorldUiRecord;
use crate::presentation::{
    OrderedRecord, ProjectionOrder, ProjectionView, StableRecordKey, WorldTopic, WorldUiView,
};

/// The chat window capacity: the pilot client's chat event ring keeps the
/// most recent 32 confirmed events, the value the measured client inventory
/// freezes for this family.
const CHAT_RING_CAPACITY: usize = 32;

/// The stable identity of one chat record: the chat topic tag beside the
/// actual observation identity the mirror retained for the publication that
/// carried the fact — never a fabricated event id, generation or tick
/// identity.
fn stable_key(observation: ObservationKey) -> StableRecordKey {
    StableRecordKey::World {
        topic: WorldTopic::Chat,
        identity: Some(observation),
    }
}

/// The rebased coherent candidate header of one record, preserving the
/// observation's own optional source tick exactly; chat carries none, so the
/// absent tick stays absent and none is invented here.
fn header(
    view: &ProjectionView<'_>,
    source_tick: Option<u64>,
) -> Result<RecordHeader, ClientError> {
    RecordHeader::try_new(
        view.frame_epoch(),
        view.frame_revision(),
        source_tick,
        FamilyOperation::Upsert,
    )
}

/// Projects the chat records of one immutable view, in actual source order:
/// observation key first, then the packet record ordinal — one chat fact per
/// publication, so every packet ordinal is zero.
///
/// Every confirmed chat publication projects exactly one upsert of the exact
/// checked fact under the bounded window rule; observations of other kinds
/// are ignored wholesale, because they belong to their own per-topic
/// providers. Order is never reconstructed from the source tick nor from the
/// rebased parent revision. The serial world-UI family assembler consumes
/// these envelopes and strips the envelope when the unchanged record vector
/// is emitted.
pub fn project_chat(
    view: &ProjectionView<'_>,
) -> Result<Vec<OrderedRecord<WorldUiRecord>>, ClientError> {
    let mut records: Vec<OrderedRecord<WorldUiRecord>> = Vec::new();
    let mut last_accepted_id: Option<u64> = None;
    for observation in view.observations() {
        let key = *observation.key();
        if key.epoch() != view.frame_epoch() {
            // An observation from another epoch never enters this frame,
            // exactly as the confirmed mirror refuses old epochs; the whole
            // projection rejects rather than publishing a partial prefix.
            return Err(ClientError::StaleEpoch);
        }
        let event =
            Event::try_from(observation.packet().clone()).map_err(|_| ClientError::InvalidInput)?;
        let fact = match event {
            Event::Chat(fact) => fact,
            // Every other publication belongs to another per-topic provider:
            // a command rejection is a sequence-bound outcome the crafting
            // provider owns, never a chat record here.
            _ => continue,
        };
        // The window admits only a strictly newer event identity: a
        // duplicate or stale resend never double-emits and leaves the
        // projected output unchanged, exactly as the pilot ring refuses an
        // id not newer than its last accepted one.
        if let Some(last) = last_accepted_id {
            if fact.event_id() <= last {
                continue;
            }
        }
        last_accepted_id = Some(fact.event_id());
        let record = WorldUiRecord::try_new(
            header(view, observation.source_tick())?,
            WorldUiView::Chat(fact),
        )?;
        records.push(OrderedRecord::try_new(
            ProjectionOrder::Confirmed {
                observation: key,
                record_ordinal: 0,
            },
            stable_key(key),
            record,
        )?);
    }
    // The bounded window: at most the most recent ring capacity in source
    // order — nothing is dropped below the capacity, and above it the oldest
    // evict exactly as the ring overwrites them.
    if records.len() > CHAT_RING_CAPACITY {
        records.drain(..records.len() - CHAT_RING_CAPACITY);
    }
    Ok(records)
}
