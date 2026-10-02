//! The survival (health, oxygen, hunger, saturation, armor) world UI
//! projection.
//!
//! This provider turns the survival scalars of the committed player-state
//! publications into ordered `world-ui@1` records: one confirmed publication
//! projects exactly one upsert carrying the complete checked `SurvivalState`
//! value the authority published — the health, oxygen and hunger beside the
//! saturation-zero presentation hint and the armor points — through the
//! survival topic tag with no event identity, because survival is a
//! singleton current-state topic whose identity is the tag itself. The
//! damage→death→respawn chain is therefore exactly the sequence of confirmed
//! publications: health descending under damage, the zero-health death the
//! accepted schema admits, and the respawn publication restoring the prior
//! scalars. The record is never a delta and never a locally derived value —
//! a confirmed combat hit against the player changes nothing, because the
//! authority publishes survival only through the player's own state
//! publication — and the checked domain boundary (health, hunger and armor
//! points at most 20, oxygen at most 300) has already refused every plus-one
//! before an observation could exist.
//!
//! Ownership boundaries: the projection is pure over the immutable
//! `ProjectionView` and owns no second mirror, no packet history and no
//! presentation state. The optional source tick is the retained
//! observation's own derived value, preserved verbatim — the player
//! publication always carries a server tick, the legal zero included, and no
//! tick is invented or reordered from, because equal ticks never decide
//! order. Observations of other kinds are ignored wholesale: they belong to
//! their own per-topic providers. A malformed observation identity — a
//! packet that is not a checked event publication or an observation from
//! another epoch — rejects the whole projection before any record is
//! returned, so no partial output exists.

use mornlea_domain::Event;

use crate::contracts::{ClientError, FamilyOperation, RecordHeader};
use crate::presentation::frame::WorldUiRecord;
use crate::presentation::{
    OrderedRecord, ProjectionOrder, ProjectionView, StableRecordKey, WorldTopic, WorldUiView,
};

/// The stable identity of one survival record: the survival topic tag alone.
/// Survival is a singleton current-state topic with no event identity of its
/// own, so the identity field is absent rather than fabricated from an
/// observation key or a source tick.
fn stable_key() -> StableRecordKey {
    StableRecordKey::World {
        topic: WorldTopic::Survival,
        identity: None,
    }
}

/// The rebased coherent candidate header of one record, preserving the
/// observation's own optional source tick exactly; no tick is invented here
/// and none is shifted onto another record.
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

/// Projects the survival records of one immutable view, in actual source
/// order: observation key first, then the packet record ordinal.
///
/// Every confirmed player-state publication projects exactly one upsert
/// carrying the complete published survival scalars; observations of other
/// kinds are ignored wholesale, because they belong to their own per-topic
/// providers. Order is never reconstructed from the source tick nor from the
/// rebased parent revision. The serial world-UI family assembler consumes
/// these envelopes and strips the envelope when the unchanged record vector
/// is emitted.
pub fn project_survival(
    view: &ProjectionView<'_>,
) -> Result<Vec<OrderedRecord<WorldUiRecord>>, ClientError> {
    let mut records: Vec<OrderedRecord<WorldUiRecord>> = Vec::new();
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
        if let Event::PlayerState(state) = event {
            // A complete publication is one upsert of the whole checked
            // value: no local derivation from combat hits or prediction
            // state, no defaults and no invented identity, because the
            // authority owns every survival fact.
            let record = WorldUiRecord::try_new(
                header(view, observation.source_tick())?,
                WorldUiView::Survival(state.survival()),
            )?;
            records.push(OrderedRecord::try_new(
                ProjectionOrder::Confirmed {
                    observation: key,
                    record_ordinal: 0,
                },
                stable_key(),
                record,
            )?);
        }
    }
    Ok(records)
}
