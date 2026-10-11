//! The environment (day, weather, season, temperature) world UI projection.
//!
//! This provider turns the world scalars of the committed player-state
//! publications into ordered `world-ui@1` records: one confirmed publication
//! projects exactly one upsert carrying the complete checked `WorldState`
//! value the authority published — the absolute world time beside the
//! display day-phase offset, the weather and season kinds, the season
//! progress and the temperature — through the environment topic tag with no
//! event identity, because the environment is a singleton topic whose
//! identity is the tag itself. The environment record is never a delta and
//! never a locally derived value: a restored publication returns the exact
//! prior scalars the authority named, never defaults, and the checked domain
//! boundary (the day-phase offset strictly below the 24,000-tick display
//! cycle, the closed weather and season kind sets, the full-range progress
//! and temperature scalars) has already refused every plus-one before an
//! observation could exist.
//!
//! Ownership boundaries: the projection is pure over the immutable
//! `ProjectionView` and owns no second mirror, no packet history and no
//! presentation state. The optional source tick is the retained
//! observation's own derived value, preserved verbatim — the player
//! publication always carries a server tick, the legal zero included, and
//! no tick is invented or reordered from, because equal ticks never decide
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

/// The stable identity of one environment record: the environment topic tag
/// alone. The environment is a singleton current-state topic with no event
/// identity of its own, so the identity field is absent rather than
/// fabricated from an observation key or a source tick.
fn stable_key() -> StableRecordKey {
    StableRecordKey::World {
        topic: WorldTopic::Environment,
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

/// Projects the environment records of one immutable view, in actual source
/// order: observation key first, then the packet record ordinal.
///
/// Every confirmed player-state publication projects exactly one upsert
/// carrying the complete published world scalars; observations of other
/// kinds are ignored wholesale, because they belong to their own per-topic
/// providers. Order is never reconstructed from the source tick nor from
/// the rebased parent revision. The serial world-UI family assembler
/// consumes these envelopes and strips the envelope when the unchanged
/// record vector is emitted.
pub fn project_environment(
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
            // value: no local derivation, no defaults and no invented
            // identity, because the authority owns every environment fact.
            let record = WorldUiRecord::try_new(
                header(view, observation.source_tick())?,
                WorldUiView::Environment(state.world()),
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
