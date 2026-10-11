//! The task lifecycle world UI projection.
//!
//! This provider turns the accepted task facts of confirmed chat
//! observations into ordered `world-ui@1` records: one confirmed chat
//! observation whose body is the accepted task branch projects exactly one
//! upsert carrying the checked `TaskView` the authority published — the
//! companion speaker, the restated original command and one closed lifecycle
//! state (`Started`, `Progress`, `Completed`, `TimedOut`, `Stopped` or
//! `Failed` with one of its five closed reasons) — under the task topic tag
//! beside the actual observation identity. The accepted schema has no task
//! id, no task generation, no progress percentage and no Pending/Running
//! wire state, so none is displayed and none can be invented here: the only
//! identity a record carries is the observation key the mirror issued when
//! it confirmed the chat fact, never the chat event's acknowledgment id and
//! never a locally fabricated identifier. Chat observations carry no server
//! tick, so the record header preserves that absence verbatim and no tick is
//! invented or shifted from a neighboring observation.
//!
//! Ownership boundaries: the projection is pure over the immutable
//! `ProjectionView` and owns no second mirror, no packet history and no
//! presentation state. The lifecycle progression itself is the authority's
//! sequence of confirmed facts — the client never derives a transition, so
//! `Completed` appears only when the authority publishes it. Observations of
//! other kinds, and chat bodies of the addressing, rejection and speech
//! branches, are ignored wholesale: they belong to their own per-topic
//! providers. A duplicate observation identity emits exactly one record —
//! the same identity never enters the mirror twice, and the serial family
//! assembler rejects duplicate stable keys, so the projection deduplicates
//! by identity instead of emitting a record the assembler would refuse. A
//! malformed observation identity — a packet that is not a checked event
//! publication or an observation from another epoch — rejects the whole
//! projection before any record is returned, so no partial output exists.

use mornlea_domain::{ChatBody, Event};

use crate::contracts::{ClientError, FamilyOperation, ObservationKey, RecordHeader};
use crate::presentation::frame::WorldUiRecord;
use crate::presentation::{
    OrderedRecord, ProjectionOrder, ProjectionView, StableRecordKey, TaskView, WorldTopic,
    WorldUiView,
};

/// The stable identity of one task record: the task topic tag beside the
/// actual observation key of the confirmed fact it carries. Each task fact is
/// its own record, so the identity is present rather than fabricated from a
/// task id, a generation or a source tick.
fn stable_key(key: ObservationKey) -> StableRecordKey {
    StableRecordKey::World {
        topic: WorldTopic::Task,
        identity: Some(key),
    }
}

/// The rebased coherent candidate header of one record, preserving the
/// observation's own optional source tick exactly; chat observations carry
/// none, and no tick is invented here or shifted onto another record.
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

/// Projects the task records of one immutable view, in actual source order:
/// observation key first, then the packet record ordinal.
///
/// Every confirmed chat observation whose body is the accepted task branch
/// projects exactly one upsert carrying the checked companion speaker, the
/// restated command and the closed lifecycle state; observations of other
/// kinds and other chat bodies are ignored wholesale, because they belong to
/// their own per-topic providers. Order is never reconstructed from the
/// source tick nor from the rebased parent revision. Duplicate observation
/// identities emit their first occurrence alone. The serial world-UI family
/// assembler consumes these envelopes and strips the envelope when the
/// unchanged record vector is emitted.
pub fn project_task(
    view: &ProjectionView<'_>,
) -> Result<Vec<OrderedRecord<WorldUiRecord>>, ClientError> {
    let mut records: Vec<OrderedRecord<WorldUiRecord>> = Vec::new();
    let mut seen: Vec<ObservationKey> = Vec::new();
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
        let Event::Chat(event) = event else {
            continue;
        };
        let ChatBody::Task {
            companion,
            command,
            state,
        } = event.body()
        else {
            // The addressing, rejection and speech branches belong to the
            // chat topic provider, never to the task records.
            continue;
        };
        if seen.contains(&key) {
            // One observation identity projects at most once: a duplicated
            // queue entry is not a second fact, and a second record would
            // collide with the first in the family assembler's stable keys.
            continue;
        }
        seen.push(key);
        // A complete task fact is one upsert of the whole checked value: no
        // local derivation, no defaults, no invented id, generation or
        // percentage, because the authority owns every task fact.
        let task = TaskView::try_new(key, companion.clone(), command.clone(), *state)?;
        let record = WorldUiRecord::try_new(
            header(view, observation.source_tick())?,
            WorldUiView::Task(task),
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
    Ok(records)
}
