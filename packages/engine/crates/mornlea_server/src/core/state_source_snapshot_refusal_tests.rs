// These boundary cases reuse the prepared session and real CPU owners of the parent test module.

fn refusal_input() -> TickOutcome {
    let mut outcome = prepared_delta(vec![]);
    outcome.block_batches.clear();
    outcome
}

fn refusal_bad_ready(state: &mut AuthorityState, key: ChunkKey) {
    // Unchecked high terrain qualifies the network consumer, not native gameplay production.
    let ready = state.residents.ready.get_mut(&key).unwrap();
    ready.set_block(
        BlockPos::new(key.pos.x() * 16, 200, key.pos.z() * 16),
        32767,
    );
    ready.mark_blocks_dirty();
    ready.finish_tick(false);
}

fn refusal_packet() -> ServerError {
    ServerError::InvalidInput {
        field: "source_publication",
    }
}

#[test]
fn source_snapshot_refusal_prepared_prefix() {
    use crate::transport::memory::MemoryTransport;
    let (mut state, sessions) = snapshot_fixture(512, 2);
    let (owner, peer) = (sessions[0], sessions[1]);
    snapshot_project(&mut state, None, false);
    let mut batch = PreparedSourcePublication::new(publication(vec![]));
    let (_, owner_ptr, owner_bytes) = prepared_source_append(&mut batch, &state, owner);
    batch.append_source_refusal(owner).unwrap();
    batch.append_event(marker_rejection(owner, 19));
    let (_, peer_ptr, peer_bytes) = prepared_source_append(&mut batch, &state, peer);
    state.publish_prepared(batch).unwrap();
    validation_retired(&state, owner, 1);
    let prefix = MemoryTransport::drain_prepared_session(&mut state, owner, 8, usize::MAX).unwrap();
    assert_eq!(prefix.len(), 1);
    assert_eq!(prefix[0].as_bytes().as_ptr(), owner_ptr);
    assert_eq!(prefix[0].as_bytes(), owner_bytes);
    assert_eq!(state.session(peer).unwrap().phase, SessionPhase::Active);
    sent(&state, peer);
    encoded_admission_drain(&mut state, peer, peer_ptr, &peer_bytes);
}

#[test]
fn source_snapshot_refusal_terminal_marker() {
    let (mut state, sessions) = snapshot_fixture(512, 1);
    let owner = sessions[0];
    let prefix = marker_rejection(owner, 11);
    state.publish(publication(vec![prefix.clone()])).unwrap();
    let capture = encoded_admission_capture(&state);
    let encoded = encoded_admission_cpu(&capture);
    let wire = mornlea_protocol::read_frame_ref(encoded.frame().as_bytes()).unwrap();
    let snapshot = ProtocolCodec::new()
        .unwrap()
        .decode_snapshot(wire.payload)
        .unwrap();
    let mut output = publication(vec![]);
    output.control.push(ControlReply {
        session: owner,
        packet: ServerPacket::ChunkSnapshot(snapshot),
    });
    let mut batch = PreparedSourcePublication::new(output);
    batch.append_source_refusal(owner).unwrap();
    state.publish_prepared(batch).unwrap();
    validation_retired(&state, owner, 0);
    assert_eq!(
        state.take_outbox(owner, 8, usize::MAX).unwrap(),
        vec![frame(prefix.event())]
    );
}

#[test]
fn source_snapshot_refusal_broadcast_registry() {
    let (mut state, sessions) = snapshot_fixture(512, 2);
    let (owner, peer) = (sessions[0], sessions[1]);
    let broadcast = RoutedEvent::new(
        EventRecipient::Broadcast,
        marker_rejection(peer, 17).event().clone(),
    );
    let mut batch = PreparedSourcePublication::new(publication(vec![]));
    batch.append_source_refusal(owner).unwrap();
    batch.append_event(broadcast.clone());
    CURRENT_PUBLICATION_VISITS.with(|n| n.set(0));
    state.publish_prepared(batch).unwrap();
    assert_eq!(CURRENT_PUBLICATION_VISITS.with(|n| n.get()), 1);
    validation_retired(&state, owner, 1);
    assert!(state.take_outbox(owner, 8, usize::MAX).unwrap().is_empty());
    assert_eq!(
        state.take_outbox(peer, 8, usize::MAX).unwrap(),
        vec![frame(broadcast.event())]
    );
}

#[test]
fn source_snapshot_refusal_stale_marker_atomic() {
    let (mut state, sessions) = snapshot_fixture(512, 1);
    let owner = sessions[0];
    snapshot_project(&mut state, None, false);
    let previous = chunk_mirror(&state, owner);
    let mut batch = PreparedSourcePublication::new(publication(vec![]));
    prepared_source_append(&mut batch, &state, owner);
    let unknown = SessionKey::from_raw(u64::MAX).unwrap();
    batch.append_source_refusal(unknown).unwrap();
    assert_eq!(
        state.publish_prepared(batch),
        Err(ServerError::StaleSession { session: unknown })
    );
    assert_eq!(state.session(owner).unwrap().phase, SessionPhase::Active);
    assert_eq!(chunk_mirror(&state, owner), previous);
    assert!(state.take_outbox(owner, 8, usize::MAX).unwrap().is_empty());
}

#[test]
fn source_snapshot_refusal_duplicate_atomic() {
    let (mut state, sessions) = snapshot_fixture(512, 2);
    let (owner, peer) = (sessions[0], sessions[1]);
    let prefix = marker_rejection(owner, 11);
    let mut batch = PreparedSourcePublication::new(publication(vec![prefix.clone()]));
    let previous = batch.publication().clone();
    assert_eq!(batch.refuse_at(2, owner), Err(refusal_packet()));
    batch.append_source_refusal(owner).unwrap();
    assert_eq!(batch.append_source_refusal(owner), Err(refusal_packet()));
    assert_eq!(batch.refuse_at(0, peer), Err(refusal_packet()));
    assert_eq!(batch.publication(), &previous);
    let peer_event = marker_rejection(peer, 13);
    batch.append_event(peer_event.clone());
    state.publish_prepared(batch).unwrap();
    validation_retired(&state, owner, 1);
    assert_eq!(
        state.take_outbox(owner, 8, usize::MAX).unwrap(),
        vec![frame(prefix.event())]
    );
    assert_eq!(state.session(peer).unwrap().phase, SessionPhase::Active);
    assert_eq!(
        state.take_outbox(peer, 8, usize::MAX).unwrap(),
        vec![frame(peer_event.event())]
    );
}

#[test]
fn source_snapshot_refusal_eight_bound() {
    let (mut state, sessions) = snapshot_fixture(512, 8);
    let mut batch = PreparedSourcePublication::new(publication(vec![]));
    for owner in &sessions {
        batch.append_source_refusal(*owner).unwrap();
    }
    assert_eq!(
        batch.append_source_refusal(SessionKey::from_raw(9).unwrap()),
        Err(refusal_packet())
    );
    state.publish_prepared(batch).unwrap();
    assert_eq!(state.occupied, 0);
    assert!(state.current_sessions.is_empty());
    for owner in sessions {
        assert_eq!(state.session(owner).unwrap().phase, SessionPhase::Retired);
        assert!(state.take_outbox(owner, 8, usize::MAX).unwrap().is_empty());
    }
    assert_eq!(state.phase(), ServerPhase::Running);
}

#[test]
fn source_snapshot_refusal_late_control_atomic() {
    let (mut state, sessions) = snapshot_fixture(512, 1);
    let owner = sessions[0];
    snapshot_project(&mut state, None, false);
    let previous = chunk_mirror(&state, owner);
    let capture = encoded_admission_capture(&state);
    let encoded = encoded_admission_cpu(&capture);
    let wire = mornlea_protocol::read_frame_ref(encoded.frame().as_bytes()).unwrap();
    let mut invalid = ProtocolCodec::new()
        .unwrap()
        .decode_snapshot(wire.payload)
        .unwrap();
    invalid.revision = 0;
    let mut output = publication(vec![]);
    output.control.push(ControlReply {
        session: owner,
        packet: ServerPacket::ChunkSnapshot(invalid),
    });
    let mut batch = PreparedSourcePublication::new(output);
    batch
        .append_encoded_snapshot(owner, &capture, encoded)
        .unwrap();
    batch.append_source_refusal(owner).unwrap();
    assert_eq!(
        state.publish_prepared(batch),
        Err(ServerError::InvalidInput { field: "packet" })
    );
    assert_eq!(state.session(owner).unwrap().phase, SessionPhase::Active);
    assert_eq!(chunk_mirror(&state, owner), previous);
    assert!(state.take_outbox(owner, 8, usize::MAX).unwrap().is_empty());
}

#[test]
fn source_snapshot_refusal_legacy_boundary() {
    let (mut state, sessions) = snapshot_fixture(512, 2);
    let (owner, peer) = (sessions[0], sessions[1]);
    snapshot_project(&mut state, None, false);
    let prefix = marker_rejection(owner, 11);
    let mut batch = PreparedSourcePublication::new(publication(vec![prefix.clone()]));
    prepared_source_append(&mut batch, &state, owner);
    let (_, pointer, bytes) = prepared_source_append(&mut batch, &state, peer);
    state
        .publish_prepared_source(batch, 1, vec![owner])
        .unwrap();
    validation_retired(&state, owner, 1);
    assert_eq!(
        state.take_outbox(owner, 8, usize::MAX).unwrap(),
        vec![frame(prefix.event())]
    );
    sent(&state, peer);
    encoded_admission_drain(&mut state, peer, pointer, &bytes);
}

#[test]
fn source_snapshot_refusal_malformed_first() {
    let (mut state, sessions) = snapshot_fixture(512, 2);
    let (owner, peer) = (sessions[0], sessions[1]);
    state.limits = state.limits.with_view_radius(2);
    let good = selection_key(5, 0);
    selection_ready(&mut state, &[good], None);
    let mut actor = state
        .residents
        .actors
        .iter()
        .find(|a| a.key == ActorKey::Player(peer))
        .unwrap()
        .clone();
    actor.motion = MotionState::new(mornlea_domain::MotionStateParts {
        position: FiniteVec3::try_new([80.5, 65.0, 0.5]).unwrap(),
        velocity: FiniteVec3::try_new([0.0; 3]).unwrap(),
        on_ground: true,
    });
    let mut context = TickContext::for_tick(&mut state, TickBudget::full());
    context.stage(RuleEffect::Actor(actor)).unwrap();
    context.commit_carried();
    drop(context);
    let expected = frame(&Event::ChunkSnapshot(
        state.chunk_snapshot_event(good).unwrap(),
    ));
    refusal_bad_ready(&mut state, snapshot_key());
    source_validation_publish(&mut state, &refusal_input(), vec![]);
    validation_retired(&state, owner, 1);
    assert!(state.take_outbox(owner, 8, usize::MAX).unwrap().is_empty());
    assert_eq!(state.session(peer).unwrap().phase, SessionPhase::Active);
    let received = state.take_outbox(peer, 16, usize::MAX).unwrap();
    let snapshots: Vec<_> = received
        .iter()
        .filter(|bytes| mornlea_protocol::read_frame_ref(bytes).unwrap().packet_id == 0)
        .collect();
    assert_eq!(snapshots, vec![&expected]);
    assert!(state.session_views[&peer].chunks[&good].snapshot_sent);
    assert_eq!(state.session_views[&peer].chunks[&good].last_revision, 9);
    assert_eq!(state.tick_failure(), None);
}

#[test]
fn source_snapshot_refusal_malformed_after_prefix() {
    let (mut state, sessions) = snapshot_fixture(512, 1);
    let owner = sessions[0];
    state.limits = state.limits.with_view_radius(2);
    let bad = selection_key(1, 0);
    let later = selection_key(2, 0);
    selection_ready(&mut state, &[bad, later], None);
    let expected = frame(&Event::ChunkSnapshot(
        state.chunk_snapshot_event(snapshot_key()).unwrap(),
    ));
    refusal_bad_ready(&mut state, bad);
    let prefix = marker_rejection(owner, 11);
    source_validation_publish(&mut state, &refusal_input(), vec![prefix.clone()]);
    validation_retired(&state, owner, 0);
    assert_eq!(
        state.take_outbox(owner, 16, usize::MAX).unwrap(),
        vec![frame(prefix.event()), expected]
    );
    assert!(state.residents.ready.contains_key(&later));
    assert_eq!(state.tick_failure(), None);
}

#[test]
fn source_snapshot_refusal_missing_is_pending() {
    let (mut state, sessions) = snapshot_fixture(512, 1);
    let owner = sessions[0];
    state.limits = state.limits.with_view_radius(2);
    snapshot_project(&mut state, None, false);
    let missing = selection_key(-1, 0);
    state
        .session_views
        .get_mut(&owner)
        .unwrap()
        .chunks
        .get_mut(&missing)
        .unwrap()
        .resync_queued = true;
    selection_limits(&mut state, 1, 1_048_576);
    let expected = frame(&Event::ChunkSnapshot(
        state.chunk_snapshot_event(snapshot_key()).unwrap(),
    ));
    source_validation_publish(&mut state, &refusal_input(), vec![]);
    assert_eq!(state.session(owner).unwrap().phase, SessionPhase::Active);
    assert!(!state.session_views[&owner].chunks[&missing].resync_queued);
    assert!(!state.session_views[&owner].chunks[&missing].snapshot_sent);
    sent(&state, owner);
    let received = state.take_outbox(owner, 16, usize::MAX).unwrap();
    let snapshots: Vec<_> = received
        .iter()
        .filter(|bytes| mornlea_protocol::read_frame_ref(bytes).unwrap().packet_id == 0)
        .collect();
    assert_eq!(snapshots, vec![&expected]);
}

#[test]
fn source_snapshot_refusal_actual_tick_endpoint() {
    let (mut state, sessions) = snapshot_fixture(512, 1);
    let owner = sessions[0];
    state.limits = state.limits.with_view_radius(2);
    refusal_bad_ready(&mut state, snapshot_key());
    assert_eq!(state.next_tick(), 0);
    let output = state
        .advance_tick(TickBudget::try_new(0, 0, 0, 0, 0).unwrap())
        .unwrap();
    assert_eq!(output.tick, 0);
    assert_eq!(state.next_tick(), 1);
    validation_retired(&state, owner, 0);
    assert!(state.take_outbox(owner, 16, usize::MAX).unwrap().is_empty());
    assert_eq!(state.tick_failure(), None);
}

#[test]
fn source_snapshot_refusal_mixed_duplicate_atomic() {
    let (mut state, sessions) = snapshot_fixture(512, 1);
    let owner = sessions[0];
    snapshot_project(&mut state, None, false);
    let previous = chunk_mirror(&state, owner);
    let mut batch = PreparedSourcePublication::new(publication(vec![]));
    prepared_source_append(&mut batch, &state, owner);
    batch.append_source_refusal(owner).unwrap();
    assert_eq!(
        state.publish_prepared_source(batch, 0, vec![owner]),
        Err(refusal_packet())
    );
    assert_eq!(state.session(owner).unwrap().phase, SessionPhase::Active);
    assert_eq!(chunk_mirror(&state, owner), previous);
    assert!(state.take_outbox(owner, 8, usize::MAX).unwrap().is_empty());
}
