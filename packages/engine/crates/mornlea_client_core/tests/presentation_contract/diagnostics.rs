//! Diagnostics family projection contract tests.
//!
//! The table pins the accepted bounded-diagnostics semantics against the
//! owner-supplied projection state: one record per publication carrying the
//! accepted producer/source/contract identity, the frame's epoch/revision
//! correlation with no fabricated tick, the explicit frame index and the two
//! immutable counter snapshots — nothing else, so no raw packet payload, no
//! command text and no undeclared timing field can appear. Counter events
//! count exactly once per owner event and never per poll; a counter at its
//! maximum publishes the incomplete-evidence value verbatim instead of
//! wrapping; a reset zeroes the epoch-scoped counters under the new epoch; an
//! unset producer identity rejects typed; a record that cannot fit a legal
//! frame byte cap rejects typed and the previous publication stands.

use mornlea_client_core::contracts::{
    ClientError, ClientLimits, ConfirmedRevision, FamilyOperation, ObservationKey, RecordHeader,
    SessionEpoch,
};
use mornlea_client_core::input::InputProjectionState;
use mornlea_client_core::prediction::PlayerProjectionState;
use mornlea_client_core::presentation::family_diagnostics::project_diagnostics;
use mornlea_client_core::presentation::frame::DiagnosticRecord;
use mornlea_client_core::presentation::{
    AcceptedObservation, AudioProjectionState, DiagnosticProjectionState, ErrorClassCounters,
    LifecycleProjectionState, MovementIntent, Pose, ProducerIdentity, ProjectionView,
    QueueCounters,
};
use mornlea_client_core::session::{ConfirmedMirror, ConfirmedMirrorParts};
use mornlea_protocol::{KeepAlive, ServerPacket};

/// The epoch the fixtures start from; a reset issues the next one.
const EPOCH: u64 = 7;

/// A real attributed producer identity: both the source digest and the
/// contract digest are nonzero, so the record names a producer and the
/// contract it was built against.
fn producer(source: u8, contract: u8) -> ProducerIdentity {
    ProducerIdentity::try_new([source; 20], [contract; 32]).expect("checked producer identity")
}

/// One queue counter snapshot from the maintained event counts.
fn queues(input: u64, inbound_records: u64, preparation: u64) -> QueueCounters {
    QueueCounters::try_new(input, inbound_records, 0, 0, 0, 0, preparation)
        .expect("checked queue snapshot")
}

/// One rejection-class snapshot from the maintained event counts.
fn rejected(capacity: u64, io: u64) -> ErrorClassCounters {
    ErrorClassCounters::try_new(0, capacity, 0, 0, io, 0).expect("checked rejection snapshot")
}

fn epoch(value: u64) -> SessionEpoch {
    SessionEpoch::try_new(value).expect("checked epoch")
}

/// The counter-owner double: the stand-in for the input, I/O, preparation,
/// publication and lifetime owners that maintain the epoch-scoped counters.
/// One saturation or rejection event bumps exactly one counter exactly once,
/// saturating at the maximum rather than wrapping; a reset issues a fresh
/// epoch with every epoch-scoped counter back at zero; polling the snapshot
/// changes nothing because only events do.
struct CounterOwnerDouble {
    epoch: SessionEpoch,
    input_events: u64,
    inbound_events: u64,
    preparation_events: u64,
    capacity_events: u64,
    io_events: u64,
}

impl CounterOwnerDouble {
    fn admitted(epoch_value: u64) -> Self {
        Self {
            epoch: epoch(epoch_value),
            input_events: 0,
            inbound_events: 0,
            preparation_events: 0,
            capacity_events: 0,
            io_events: 0,
        }
    }

    /// One input queue saturation event: the bound was hit once.
    fn saturate_input_once(&mut self) {
        self.input_events = self.input_events.saturating_add(1);
    }

    /// One capacity-class rejection event.
    fn reject_on_capacity_once(&mut self) {
        self.capacity_events = self.capacity_events.saturating_add(1);
    }

    /// The epoch reset: a fresh epoch with every epoch-scoped counter zeroed,
    /// so no old-epoch count crosses the boundary.
    fn reset(&mut self) {
        self.epoch = epoch(self.epoch.get() + 1);
        self.input_events = 0;
        self.inbound_events = 0;
        self.preparation_events = 0;
        self.capacity_events = 0;
        self.io_events = 0;
    }

    /// The immutable snapshot the state owner hands one projection.
    fn state(&self, producer: ProducerIdentity) -> DiagnosticProjectionState {
        DiagnosticProjectionState::try_new(
            producer,
            queues(
                self.input_events,
                self.inbound_events,
                self.preparation_events,
            ),
            rejected(self.capacity_events, self.io_events),
        )
        .expect("checked diagnostics projection state")
    }
}

/// The projection-view state owners a fixture keeps alive beside the borrowed
/// mirror and observation queue; the diagnostics state and limits arrive per
/// view because every table row varies them.
struct ViewFixture {
    input: InputProjectionState,
    player: PlayerProjectionState,
    audio: AudioProjectionState,
    lifecycle: LifecycleProjectionState,
}

impl ViewFixture {
    fn new(epoch_value: u64, revision: u64) -> Self {
        Self {
            input: InputProjectionState::try_new(1).expect("input projection state"),
            player: PlayerProjectionState::try_new(
                epoch(epoch_value),
                ConfirmedRevision::new(revision),
                Pose::try_new([0.0, 64.0, 0.0], 0.0, 0.0).expect("finite pose"),
                None,
                Vec::new(),
                None,
                None,
                MovementIntent::try_new(None, false).expect("movement intent"),
            )
            .expect("player projection state"),
            audio: AudioProjectionState::try_new().expect("audio projection state"),
            lifecycle: LifecycleProjectionState::try_new(1).expect("lifecycle projection state"),
        }
    }

    /// One immutable projection view over the supplied diagnostics state,
    /// mirror, observation queue and limits at the named frame identity.
    #[allow(clippy::too_many_arguments)]
    fn view<'a>(
        &'a self,
        diagnostics: &'a DiagnosticProjectionState,
        mirror: &'a ConfirmedMirror,
        observations: &'a [AcceptedObservation],
        frame_epoch: SessionEpoch,
        frame_revision: ConfirmedRevision,
        frame_index: u64,
        limits: &'a ClientLimits,
    ) -> ProjectionView<'a> {
        ProjectionView::try_new(
            mirror,
            observations,
            &self.input,
            &self.player,
            &self.audio,
            &self.lifecycle,
            diagnostics,
            frame_epoch,
            frame_revision,
            frame_index,
            limits,
        )
        .expect("projection view")
    }
}

/// A pending-connection mirror of one epoch; diagnostics never reads it, the
/// view only borrows it.
fn pending_mirror(epoch_value: u64) -> ConfirmedMirror {
    ConfirmedMirror::try_new(ConfirmedMirrorParts::pending(epoch(epoch_value)))
        .expect("pending mirror")
}

/// One committed server observation beside the queue, carrying a raw packet.
/// A diagnostics record must stay independent of it.
fn keep_alive_observation(revision: u64) -> AcceptedObservation {
    let key = ObservationKey::try_new(epoch(EPOCH), ConfirmedRevision::new(revision), 0)
        .expect("observation key");
    AcceptedObservation::try_new(
        key,
        Some(revision),
        ServerPacket::KeepAlive(KeepAlive::new(revision).expect("checked keep-alive token")),
        Vec::new(),
    )
    .expect("accepted observation")
}

/// `identity`: one record carrying the accepted producer identity, the exact
/// frame correlation and the two immutable counter snapshots — and nothing
/// else. The full-record equality against a locally built record over the
/// same closed schema is the pin that no raw packet bytes, no command text
/// and no undeclared timing field can appear, even with a raw observation
/// sitting in the borrowed queue.
#[test]
fn record_carries_identity_correlation_and_counter_snapshots_only() {
    let mut owner = CounterOwnerDouble::admitted(EPOCH);
    for _ in 0..3 {
        owner.saturate_input_once();
    }
    owner.inbound_events = 2;
    owner.preparation_events = 1;
    owner.reject_on_capacity_once();
    let identity = producer(0x11, 0x22);
    let state = owner.state(identity);

    let fixture = ViewFixture::new(EPOCH, 9);
    let mirror = pending_mirror(EPOCH);
    let observations = [keep_alive_observation(9)];
    let limits = ClientLimits::try_new().expect("frozen limits");
    let view = fixture.view(
        &state,
        &mirror,
        &observations,
        epoch(EPOCH),
        ConfirmedRevision::new(9),
        5,
        &limits,
    );
    let records = project_diagnostics(&view).expect("one diagnostics record");
    assert_eq!(records.len(), 1);

    let expected = DiagnosticRecord::try_new(
        RecordHeader::try_new(
            epoch(EPOCH),
            ConfirmedRevision::new(9),
            None,
            FamilyOperation::Upsert,
        )
        .expect("checked header"),
        identity,
        5,
        queues(3, 2, 1),
        rejected(1, 0),
    )
    .expect("checked expected record");
    assert_eq!(records[0], expected);
    assert_eq!(
        records[0].header().source_tick(),
        None,
        "a local diagnostics record fabricates no server tick"
    );
    assert_eq!(records[0].producer().source_sha(), [0x11; 20]);
    assert_eq!(records[0].producer().contract_sha(), [0x22; 32]);
    assert_eq!(records[0].frame_index(), 5);
}

/// `poll`: a counter event counts exactly once per owner event and never per
/// poll. Repeated projections of one immutable view return the identical
/// record, and one further event advances the published value by exactly one.
#[test]
fn queue_saturation_counts_once_per_event_not_per_poll() {
    let mut owner = CounterOwnerDouble::admitted(EPOCH);
    for _ in 0..3 {
        owner.saturate_input_once();
    }
    let state = owner.state(producer(0x11, 0x22));

    let fixture = ViewFixture::new(EPOCH, 4);
    let mirror = pending_mirror(EPOCH);
    let limits = ClientLimits::try_new().expect("frozen limits");
    let view = fixture.view(
        &state,
        &mirror,
        &[],
        epoch(EPOCH),
        ConfirmedRevision::new(4),
        12,
        &limits,
    );
    let first = project_diagnostics(&view).expect("first poll projects");
    assert_eq!(first.len(), 1);
    assert_eq!(first[0].queue_high_water(), &queues(3, 0, 0));

    let second = project_diagnostics(&view).expect("second poll projects");
    assert_eq!(
        first, second,
        "a poll is not a saturation event: the published count never grows by projecting"
    );

    owner.saturate_input_once();
    let next_state = owner.state(producer(0x11, 0x22));
    let next_view = fixture.view(
        &next_state,
        &mirror,
        &[],
        epoch(EPOCH),
        ConfirmedRevision::new(4),
        13,
        &limits,
    );
    let third = project_diagnostics(&next_view).expect("projection after one event");
    assert_eq!(
        third[0].queue_high_water(),
        &queues(4, 0, 0),
        "one saturation event advances the published high-water by exactly one"
    );
}

/// `reset`: a reset zeroes the epoch-scoped counters under the new epoch, and
/// no old-epoch count crosses the boundary.
#[test]
fn reset_clears_epoch_scoped_counters_under_the_new_epoch() {
    let mut owner = CounterOwnerDouble::admitted(EPOCH);
    owner.saturate_input_once();
    owner.reject_on_capacity_once();
    let state = owner.state(producer(0x11, 0x22));

    let fixture = ViewFixture::new(EPOCH, 3);
    let limits = ClientLimits::try_new().expect("frozen limits");
    let old_mirror = pending_mirror(EPOCH);
    let old_view = fixture.view(
        &state,
        &old_mirror,
        &[],
        epoch(EPOCH),
        ConfirmedRevision::new(3),
        2,
        &limits,
    );
    let old_records = project_diagnostics(&old_view).expect("pre-reset publication");
    assert_eq!(old_records[0].header().epoch(), epoch(EPOCH));
    assert_eq!(old_records[0].queue_high_water(), &queues(1, 0, 0));
    assert_eq!(old_records[0].rejected(), &rejected(1, 0));

    owner.reset();
    assert_eq!(owner.epoch.get(), EPOCH + 1);
    let reset_state = owner.state(producer(0x11, 0x22));
    let new_mirror = pending_mirror(EPOCH + 1);
    let new_view = fixture.view(
        &reset_state,
        &new_mirror,
        &[],
        owner.epoch,
        ConfirmedRevision::new(0),
        3,
        &limits,
    );
    let new_records = project_diagnostics(&new_view).expect("post-reset publication");
    assert_eq!(
        new_records[0].header().epoch(),
        epoch(EPOCH + 1),
        "the reset record carries the new epoch"
    );
    assert_eq!(
        new_records[0].queue_high_water(),
        &QueueCounters::default(),
        "reset clears every epoch-scoped queue counter"
    );
    assert_eq!(
        new_records[0].rejected(),
        &ErrorClassCounters::default(),
        "reset clears every epoch-scoped rejection counter"
    );
    assert_eq!(new_records[0].frame_index(), 3);
}

/// `unknown`: an identity that names no producer — an unset source digest, an
/// unset contract digest or both — rejects typed before any record exists,
/// because a provenance record cannot publish without provenance. An
/// attributed identity still projects.
#[test]
fn unset_producer_identity_rejects_typed() {
    let fixture = ViewFixture::new(EPOCH, 2);
    let mirror = pending_mirror(EPOCH);
    let limits = ClientLimits::try_new().expect("frozen limits");
    let unset = [
        ProducerIdentity::try_new([0u8; 20], [0u8; 32]).expect("checked identity"),
        ProducerIdentity::try_new([1u8; 20], [0u8; 32]).expect("checked identity"),
        ProducerIdentity::try_new([0u8; 20], [1u8; 32]).expect("checked identity"),
    ];
    for identity in unset {
        let state =
            DiagnosticProjectionState::try_new(identity, QueueCounters::default(), rejected(1, 0))
                .expect("checked state");
        let view = fixture.view(
            &state,
            &mirror,
            &[],
            epoch(EPOCH),
            ConfirmedRevision::new(2),
            1,
            &limits,
        );
        assert_eq!(
            project_diagnostics(&view),
            Err(ClientError::InvalidInput),
            "an identity naming no producer or no contract rejects typed"
        );
    }

    let attributed = producer(0x11, 0x22);
    let state =
        DiagnosticProjectionState::try_new(attributed, QueueCounters::default(), rejected(1, 0))
            .expect("checked state");
    let view = fixture.view(
        &state,
        &mirror,
        &[],
        epoch(EPOCH),
        ConfirmedRevision::new(2),
        1,
        &limits,
    );
    assert!(
        project_diagnostics(&view).is_ok(),
        "an attributed identity projects"
    );
}

/// `saturation`: a counter at its maximum publishes the incomplete-evidence
/// value verbatim — never a wrapped count and never a silently frozen lower
/// value — for both the queue and the rejection-class snapshots, and one
/// further event changes nothing because the maximum is the report.
#[test]
fn saturated_counter_publishes_incomplete_evidence_without_wrap() {
    let mut owner = CounterOwnerDouble::admitted(EPOCH);
    owner.input_events = u64::MAX - 1;
    owner.capacity_events = u64::MAX;
    owner.saturate_input_once();

    let state = owner.state(producer(0x11, 0x22));
    let fixture = ViewFixture::new(EPOCH, 6);
    let mirror = pending_mirror(EPOCH);
    let limits = ClientLimits::try_new().expect("frozen limits");
    let view = fixture.view(
        &state,
        &mirror,
        &[],
        epoch(EPOCH),
        ConfirmedRevision::new(6),
        9,
        &limits,
    );
    let records = project_diagnostics(&view).expect("a saturated snapshot still publishes");
    assert_eq!(records.len(), 1);
    assert_eq!(
        records[0].queue_high_water(),
        &queues(u64::MAX, 0, 0),
        "the maximum queue value is the incomplete-evidence report"
    );
    assert_eq!(
        records[0].rejected(),
        &rejected(u64::MAX, 0),
        "the maximum rejection value is the incomplete-evidence report"
    );

    owner.saturate_input_once();
    let next_state = owner.state(producer(0x11, 0x22));
    let next_view = fixture.view(
        &next_state,
        &mirror,
        &[],
        epoch(EPOCH),
        ConfirmedRevision::new(6),
        10,
        &limits,
    );
    let next_records = project_diagnostics(&next_view).expect("saturation stays reportable");
    assert_eq!(
        next_records[0].queue_high_water(),
        &queues(u64::MAX, 0, 0),
        "one event past the maximum neither wraps nor lowers the report"
    );
}

/// `oversized`: a record that cannot fit a legal frame byte cap rejects typed
/// with no partial vector, and the previous publication stands unchanged —
/// the same state under the frozen limits still projects the identical record.
#[test]
fn oversized_record_rejects_and_keeps_the_previous_publication() {
    let mut owner = CounterOwnerDouble::admitted(EPOCH);
    owner.saturate_input_once();
    let state = owner.state(producer(0x11, 0x22));

    let fixture = ViewFixture::new(EPOCH, 8);
    let mirror = pending_mirror(EPOCH);
    let limits = ClientLimits::try_new().expect("frozen limits");
    let view = fixture.view(
        &state,
        &mirror,
        &[],
        epoch(EPOCH),
        ConfirmedRevision::new(8),
        4,
        &limits,
    );
    let accepted = project_diagnostics(&view).expect("the frozen limits admit the record");
    assert_eq!(accepted.len(), 1);

    // Every frozen ceiling with a one-byte frame cap: a legal configuration
    // no diagnostics record fits.
    let tight = ClientLimits::try_new_with(
        ClientLimits::MAX_QUEUED_INPUT_EVENTS,
        ClientLimits::MAX_INBOUND_OBSERVATIONS,
        ClientLimits::MAX_INBOUND_BYTES,
        ClientLimits::MAX_OUTBOUND_COMMANDS,
        ClientLimits::MAX_OUTBOUND_BYTES,
        ClientLimits::MAX_PREDICTION_JOURNAL,
        ClientLimits::MAX_MESSAGE_WORK,
        ClientLimits::MAX_MESH_WORK,
        ClientLimits::MAX_PREPARATION_RESULTS,
        ClientLimits::MAX_PREPARATION_BYTES,
        ClientLimits::MAX_FAMILY_RECORDS,
        1,
    )
    .expect("tight but legal limits");
    let oversize_view = fixture.view(
        &state,
        &mirror,
        &[],
        epoch(EPOCH),
        ConfirmedRevision::new(8),
        5,
        &tight,
    );
    assert_eq!(
        project_diagnostics(&oversize_view),
        Err(ClientError::Capacity),
        "an oversized record rejects typed with no partial vector"
    );

    let after = project_diagnostics(&view).expect("the old publication still projects");
    assert_eq!(
        accepted, after,
        "the rejected projection leaves the previous publication intact"
    );
}

/// `pending`: revision zero is the pending-connection local frame, where a
/// diagnostics record is legal; its header carries the zero revision exactly.
#[test]
fn pending_revision_zero_projects_the_local_record() {
    let state = CounterOwnerDouble::admitted(EPOCH).state(producer(0x11, 0x22));
    let fixture = ViewFixture::new(EPOCH, 0);
    let mirror = pending_mirror(EPOCH);
    let limits = ClientLimits::try_new().expect("frozen limits");
    let view = fixture.view(
        &state,
        &mirror,
        &[],
        epoch(EPOCH),
        ConfirmedRevision::new(0),
        0,
        &limits,
    );
    let records = project_diagnostics(&view).expect("revision zero admits diagnostics");
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].header().revision(), ConfirmedRevision::new(0));
    assert_eq!(records[0].header().epoch(), epoch(EPOCH));
    assert_eq!(records[0].frame_index(), 0);
    assert_eq!(records[0].queue_high_water(), &QueueCounters::default());
}
