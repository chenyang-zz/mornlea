// These cases reuse the actual background disk, native generator and Memory login owners.

use mornlea_server::core::source_acquisition::{SourceAcquisition, SourceAcquisitionPoll};
use mornlea_server::core::source_encoding::SourceSnapshotEncoding;

struct EncodedAcquisitionObservation {
    report: mornlea_server::core::source_acquisition::SourceAcquisitionTick,
    before: u64,
    held: u64,
    after: u64,
    first_pending: bool,
    peak_requests: usize,
}

fn encoded_acquisition_step(
    acquisition: &mut SourceAcquisition,
    state: &mut AuthorityState,
    fixture: &mut Fixture,
    encoding: &mut SourceSnapshotEncoding,
) -> EncodedAcquisitionObservation {
    let before = state.next_tick();
    let mut pending = acquisition.begin_encoded_tick(
        state, &mut fixture.store, &mut fixture.generation, encoding, TickBudget::full(),
    ).expect("actual encoded source begin");
    let held = pending.state().next_tick();
    let until = Instant::now() + SOURCE_ACQUISITION_BOUND;
    let mut first_pending = false;
    let mut peak_requests = 0;
    let mut polls = 0;
    let report = loop {
        let result = pending.poll(deadline()).expect("actual encoded source poll");
        polls += 1;
        peak_requests = peak_requests.max(pending.pending_snapshot_requests());
        match result {
            SourceAcquisitionPoll::Ready(report) => break report,
            SourceAcquisitionPoll::Pending => {
                first_pending |= polls == 1;
                assert!(Instant::now() < until, "actual encoded source CPU stalled");
                thread::yield_now();
            }
        }
    };
    drop(pending);
    EncodedAcquisitionObservation { report, before, held, after: state.next_tick(), first_pending, peak_requests }
}

fn encoded_acquisition_close(encoding: &mut SourceSnapshotEncoding) {
    encoding.stop_new().unwrap();
    encoding.wait(deadline()).unwrap();
    encoding.close(deadline()).unwrap();
}

fn encoded_acquisition_actual(saved: bool, workers: usize) {
    let target = key(Dimension::OVERWORLD, 0, 0);
    let (mut fixture, mut state) = source_acquisition_fixture(
        saved.then(saved_player), Dimension::OVERWORLD, ChunkPos::new(0, 0),
        if saved { vec![(target, floor()), (key(Dimension::OVERWORLD, -1, 0), floor())] } else { Vec::new() },
    );
    let (_login, _transport, _connection, session, _clock) = handshake(&mut fixture, &mut state);
    let mut acquisition = SourceAcquisition::new();
    let mut encoding = SourceSnapshotEncoding::try_new(workers).unwrap();
    let until = Instant::now() + SOURCE_ACQUISITION_BOUND;
    let mut observations = Vec::new();
    let mut snapshots = Vec::new();
    loop {
        observations.push(encoded_acquisition_step(&mut acquisition, &mut state, &mut fixture, &mut encoding));
        for prepared in MemoryTransport::drain_prepared_session(&mut state, session, 512, usize::MAX).unwrap() {
            if let ServerPacket::ChunkSnapshot(snapshot) = decode(prepared.as_bytes(), State::Play) {
                snapshots.push(snapshot);
            }
        }
        let active = state.settled_read().unwrap().actor(ActorKey::Player(session))
            .is_some_and(|actor| actor.lifecycle == ActorLifecycle::Active);
        let target_sent = snapshots.iter().any(|snapshot| (snapshot.chunk_x, snapshot.chunk_z) == (target.pos.x(), target.pos.z()));
        if active && target_sent { break; }
        assert!(Instant::now() < until, "actual automatic encoded acquisition stalled");
        thread::yield_now();
    }
    let materialized = source_materialized(&state, target);
    let expected = if saved { floor() } else {
        mornlea_server::core::generation::ChunkGenerator::try_new(42, false).unwrap().generate(target).unwrap()
    };
    let phase = state.session(session).unwrap().phase;
    let zero_requests = (encoding.retained_requests(), encoding.charged_requests());
    // Business observations precede the established test-only provider drain.
    source_acquisition_drain(&mut acquisition, &mut state, &mut fixture);
    encoded_acquisition_close(&mut encoding);
    fixture.close();
    assert_eq!(phase, SessionPhase::Active);
    assert_eq!(materialized, expected);
    assert_eq!(zero_requests, (0, 0));
    assert!(observations.iter().any(|o| o.first_pending && o.peak_requests > 0));
    for observation in observations {
        assert_eq!(observation.held, observation.before);
        assert_eq!(observation.report.publication.tick, observation.before);
        assert_eq!(observation.after, observation.before + 1);
        assert!(observation.peak_requests <= 1);
        assert_eq!(observation.report.first_error, None);
    }
    let snapshot = snapshots.iter().find(|snapshot| (snapshot.chunk_x, snapshot.chunk_z) == (target.pos.x(), target.pos.z())).unwrap();
    assert_eq!(snapshot.revision, if saved { 9 } else { 1 });
    assert_eq!(snapshot.dimension, Dimension::OVERWORLD);
    assert_eq!(snapshot.sections.len(), 24);
    // Translate the original fixture or off-tick native body directly, independently of the delivered capture.
    let expected_sections = expected.sections.into_iter().enumerate().map(|(index, section)| {
        let y = i32::try_from(index).unwrap();
        match section.kind {
            StorageKind::Single => mornlea_protocol::SectionData::single(y, section.single),
            StorageKind::Indexed => mornlea_protocol::SectionData::indexed(y, section.bits, section.palette, section.packed),
            StorageKind::Direct => mornlea_protocol::SectionData::direct(y, section.packed),
        }
    }).collect();
    let expected_wire = mornlea_protocol::ChunkSnapshot::new(Dimension::OVERWORLD, 0, 0, if saved { 9 } else { 1 }, expected_sections).unwrap();
    assert_eq!(snapshot, &expected_wire);
    let mut all_air = expected_wire.clone();
    all_air.sections = (0..24).map(|y| mornlea_protocol::SectionData::single(y, 0)).collect();
    assert_ne!(snapshot, &all_air);

}

#[test]
fn source_acquisition_encoded_saved_memory() {
    encoded_acquisition_actual(true, 1);
}

#[test]
fn source_acquisition_encoded_missing_native() {
    encoded_acquisition_actual(false, 2);
}

#[test]
fn source_acquisition_encoded_stopped_before_drive() {
    let (mut fixture, mut state) = source_acquisition_fixture(None, Dimension::OVERWORLD, ChunkPos::new(0, 0), Vec::new());
    let mut acquisition = SourceAcquisition::new();
    let mut encoding = SourceSnapshotEncoding::try_new(1).unwrap();
    encoding.stop_new().unwrap();
    let error = acquisition.begin_encoded_tick(&mut state, &mut fixture.store, &mut fixture.generation, &mut encoding, TickBudget::full()).err();
    let observed = (state.next_tick(), acquisition.pending_loads(), acquisition.pending_generations(), acquisition.pending_candidates());
    encoded_acquisition_close(&mut encoding);
    fixture.close();
    assert_eq!(error, Some(ServerError::InvalidState { phase: ServerPhase::Closing }));
    assert_eq!(observed, (0, 0, 0, 0));
}

#[test]
fn source_acquisition_encoded_expired_retry() {
    let (mut fixture, mut state) = source_acquisition_fixture(None, Dimension::OVERWORLD, ChunkPos::new(0, 0), Vec::new());
    let mut acquisition = SourceAcquisition::new();
    let mut encoding = SourceSnapshotEncoding::try_new(1).unwrap();
    let mut pending = acquisition.begin_encoded_tick(&mut state, &mut fixture.store, &mut fixture.generation, &mut encoding, TickBudget::full()).unwrap();
    let initial = pending.state().next_tick();
    let expired_pending = matches!(pending.poll(Deadline::at(Instant::now())), Ok(SourceAcquisitionPoll::Pending));
    let retained_tick = pending.state().next_tick();
    let retry_tick = match pending.poll(deadline()) {
        Ok(SourceAcquisitionPoll::Ready(report)) => Some(report.publication.tick),
        _ => None,
    };
    let repeated_error = pending.poll(deadline()).err();
    let terminal_tick = pending.state().next_tick();
    drop(pending);
    encoded_acquisition_close(&mut encoding);
    fixture.close();
    assert!(expired_pending);
    assert_eq!((initial, retained_tick, retry_tick, terminal_tick), (0, 0, Some(0), 1));
    assert_eq!(repeated_error, Some(ServerError::InvalidInput { field: "source_encoded_tick" }));
    assert_eq!(state.next_tick(), 1);
}
