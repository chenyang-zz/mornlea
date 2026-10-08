// Prepared terrain qualifies the retained tick and actual CPU consumer boundaries.

use crate::core::source_encoding::SourceSnapshotEncoding;
use std::time::{Duration, Instant};

struct EncodedTickObservation {
    publication: Result<TickPublication, ServerError>,
    first_ready: bool,
    peak_requests: usize,
    polls: usize,
}

fn encoded_tick_budget() -> TickBudget {
    TickBudget::try_new(0, 0, 0, 0, 0).unwrap()
}

fn encoded_tick_deadline() -> Deadline {
    Deadline::after(Instant::now(), Duration::from_secs(10)).unwrap()
}

fn encoded_tick_fixture(players: u8) -> (AuthorityState, Vec<SessionKey>) {
    let (mut state, sessions) = fixture(512, players);
    state.limits = state.limits.with_view_radius(2);
    state.enable_live_chunks().unwrap();
    let (mut terrain, _) = snapshot_fixture(512, 0);
    let ready = terrain.residents.ready.remove(&snapshot_key()).unwrap();
    let mut context = TickContext::for_tick(&mut state, encoded_tick_budget());
    context.preload_ready_chunk(ready);
    context.commit_carried();
    drop(context);
    (state, sessions)
}

fn encoded_tick_managed_goals(state: &mut AuthorityState) -> SourceGoals {
    use crate::core::world::PreparedChunk;
    // Prepared completions establish matching book/body identity; actual disk callers are separate.
    let wanted: BTreeSet<_> = state.residents.ready.keys().copied().collect();
    state.replace_chunk_wants(wanted.clone()).unwrap();
    let missing: Vec<_> = wanted.iter().copied().filter(|key| state.live_chunk_facts(*key).is_none()).collect();
    for key in missing {
        let ready = &state.residents.ready[&key];
        let revision = ready.revision;
        let chunk = ready.capture(state.residents.drops.get(&key), state.residents.container_chunks.get(&key)).materialize().chunk;
        let reservation = state.reserve_chunk_load(key).unwrap();
        let request = ChunkRequestId::try_new(reservation.generation()).unwrap();
        state.bind_chunk_load(reservation, request).unwrap();
        let body = PreparedChunk::try_new(key, reservation.generation(), RecoveredChunk {
            chunk, revision, persisted_revision: revision, needs_rewrite: false, recovered: false,
        }).unwrap();
        state.offer_acquired(AcquiredChunkEvent::Load { key, generation: reservation.generation(), request, result: Ok(Some(body)) }).unwrap();
    }
    let mut context = TickContext::for_tick(state, encoded_tick_budget());
    let applied = context.apply_live_acquisition();
    assert_eq!(applied.rejected, 0);
    context.commit_carried();
    drop(context);
    crate::core::source_acquisition::goals_tests::prepared_projection_goals(wanted)
}

fn encoded_tick_finish(
    mut pending: super::source_tick::SourceTickContinuation<'_>,
    encoding: &mut SourceSnapshotEncoding,
) -> EncodedTickObservation {
    pending.prepare_encoding().unwrap();
    let until = Instant::now() + Duration::from_secs(10);
    let first = pending.poll_encoding(encoding);
    let first_ready = matches!(first, Ok(true));
    let mut peak_requests = encoding.charged_requests();
    let mut polls = 1;
    let mut result = first;
    while matches!(result, Ok(false)) {
        assert!(Instant::now() < until, "actual snapshot CPU stalled");
        std::thread::yield_now();
        result = pending.poll_encoding(encoding);
        peak_requests = peak_requests.max(encoding.charged_requests());
        polls += 1;
    }
    let publication = match result {
        Ok(true) => pending.complete_encoded_with(|_, _, publication| Ok(publication)),
        Ok(false) => unreachable!(),
        Err(error) => Err(pending.abort(error)),
    };
    EncodedTickObservation {
        publication,
        first_ready,
        peak_requests,
        polls,
    }
}

fn encoded_tick_run(
    state: &mut AuthorityState,
    encoding: &mut SourceSnapshotEncoding,
) -> EncodedTickObservation {
    let mut goals = encoded_tick_managed_goals(state);
    let pending = state.begin_source_tick(encoded_tick_budget(), &mut goals).unwrap();
    encoded_tick_finish(pending, encoding)
}

fn encoded_tick_join(encoding: &mut SourceSnapshotEncoding) {
    encoding.stop_new().unwrap();
    encoding.wait(encoded_tick_deadline()).unwrap();
    encoding.close(encoded_tick_deadline()).unwrap();
}

fn encoded_tick_snapshots(publication: &TickPublication, session: SessionKey) -> Vec<ChunkKey> {
    publication.events.iter().filter_map(|event| {
        if event.recipient() != EventRecipient::Session(session.get()) {
            return None;
        }
        match event.event() {
            Event::ChunkSnapshot(snapshot) => {
                assert_eq!(snapshot.revision(), 9);
                Some(ChunkKey { dimension: snapshot.dimension(), pos: snapshot.chunk() })
            }
            _ => None,
        }
    }).collect()
}

fn encoded_tick_snapshot_frames(state: &mut AuthorityState, session: SessionKey) -> Vec<Vec<u8>> {
    state.take_outbox(session, 512, usize::MAX).unwrap().into_iter().filter(|bytes| {
        mornlea_protocol::read_frame_ref(bytes).unwrap().packet_id == 0
    }).collect()
}

#[test]
fn source_encoded_tick_held_reduction() {
    let (mut state, sessions) = encoded_tick_fixture(1);
    let owner = sessions[0];
    let expected = frame(&Event::ChunkSnapshot(state.chunk_snapshot_event(snapshot_key()).unwrap()));
    let mut goals = encoded_tick_managed_goals(&mut state);
    let mut encoding = SourceSnapshotEncoding::try_new(1).unwrap();
    let pending = state.begin_source_tick(encoded_tick_budget(), &mut goals).unwrap();
    let before = pending.state().next_tick();
    let pending_frames = pending.state().sessions[&owner].outbox.len();
    let settled_time = pending.state().residents.environment.as_ref().unwrap().world_time;
    let observed = encoded_tick_finish(pending, &mut encoding);
    encoded_tick_join(&mut encoding);
    assert_eq!((before, pending_frames, settled_time), (0, 0, 1));
    assert!(!observed.first_ready);
    assert!(observed.peak_requests > 0);
    assert!(observed.polls > 1);
    let publication = observed.publication.unwrap();
    assert_eq!((publication.tick, state.next_tick()), (0, 1));
    assert_eq!(state.residents.environment.as_ref().unwrap().world_time, 1);
    assert_eq!(encoded_tick_snapshots(&publication, owner), vec![snapshot_key()]);
    assert_eq!(encoded_tick_snapshot_frames(&mut state, owner), vec![expected]);
    sent(&state, owner);
}

#[test]
fn source_encoded_tick_whole_pass_cohorts() {
    let (mut state, sessions) = encoded_tick_fixture(2);
    let keys = [selection_key(0, 0), selection_key(-1, 0), selection_key(0, -1), selection_key(0, 1), selection_key(1, 0)];
    selection_ready(&mut state, &keys, None);
    selection_limits(&mut state, 5, 1_048_576);
    let mut encoding = SourceSnapshotEncoding::try_new(2).unwrap();
    let observed = encoded_tick_run(&mut state, &mut encoding);
    encoded_tick_join(&mut encoding);
    assert!(!observed.first_ready);
    assert_eq!(observed.peak_requests, 2);
    let publication = observed.publication.unwrap();
    for owner in sessions {
        assert_eq!(encoded_tick_snapshots(&publication, owner), keys);
        assert_eq!(encoded_tick_snapshot_frames(&mut state, owner).len(), 5);
        for key in keys {
            let mirror = state.session_views[&owner].chunks[&key];
            assert!(mirror.snapshot_sent);
            assert_eq!(mirror.last_revision, 9);
        }
    }
    assert_eq!(state.next_tick(), 1);
}

#[test]
fn source_encoded_tick_resync_priority() {
    let (mut state, sessions) = encoded_tick_fixture(1);
    let owner = sessions[0];
    let far = selection_key(1, 0);
    selection_ready(&mut state, &[far], None);
    snapshot_project(&mut state, None, false);
    state.session_views.get_mut(&owner).unwrap().chunks.get_mut(&far).unwrap().resync_queued = true;
    selection_limits(&mut state, 1, 1_048_576);
    let mut encoding = SourceSnapshotEncoding::try_new(1).unwrap();
    let observed = encoded_tick_run(&mut state, &mut encoding);
    encoded_tick_join(&mut encoding);
    assert_eq!(encoded_tick_snapshots(&observed.publication.unwrap(), owner), vec![far]);
    assert!(!state.session_views[&owner].chunks[&snapshot_key()].snapshot_sent);
    assert!(!state.session_views[&owner].chunks[&far].resync_queued);
    assert_eq!(state.session_views[&owner].chunks[&far].last_revision, 9);
}

#[test]
fn source_encoded_tick_section_boundary() {
    let (mut state, sessions) = encoded_tick_fixture(1);
    let keys = [selection_key(0, 0), selection_key(-1, 0), selection_key(0, -1)];
    selection_ready(&mut state, &keys, None);
    selection_limits(&mut state, 3, 96);
    let mut encoding = SourceSnapshotEncoding::try_new(1).unwrap();
    let observed = encoded_tick_run(&mut state, &mut encoding);
    encoded_tick_join(&mut encoding);
    assert_eq!(encoded_tick_snapshots(&observed.publication.unwrap(), sessions[0]), keys[..2]);
    assert!(!state.session_views[&sessions[0]].chunks[&keys[2]].snapshot_sent);
}

#[test]
fn source_encoded_tick_later_oversize() {
    let (mut state, sessions) = encoded_tick_fixture(1);
    let keys = [selection_key(0, 0), selection_key(-1, 0), selection_key(0, -1)];
    selection_ready(&mut state, &keys, Some(keys[1]));
    selection_limits(&mut state, 3, 96);
    let mut encoding = SourceSnapshotEncoding::try_new(1).unwrap();
    let observed = encoded_tick_run(&mut state, &mut encoding);
    encoded_tick_join(&mut encoding);
    assert_eq!(encoded_tick_snapshots(&observed.publication.unwrap(), sessions[0]), keys[..1]);
    for key in &keys[1..] {
        assert!(!state.session_views[&sessions[0]].chunks[key].snapshot_sent);
    }
}

#[test]
fn source_encoded_tick_first_oversize() {
    let (mut state, sessions) = encoded_tick_fixture(1);
    selection_ready(&mut state, &[selection_key(-1, 0)], None);
    selection_limits(&mut state, 2, 0);
    let mut encoding = SourceSnapshotEncoding::try_new(1).unwrap();
    let observed = encoded_tick_run(&mut state, &mut encoding);
    encoded_tick_join(&mut encoding);
    assert_eq!(encoded_tick_snapshots(&observed.publication.unwrap(), sessions[0]), vec![snapshot_key()]);
}

#[test]
fn source_encoded_tick_bounded_missing() {
    let (mut state, sessions) = encoded_tick_fixture(1);
    let owner = sessions[0];
    state.limits = state.limits.with_view_radius(3);
    let far = selection_key(3, 3);
    selection_ready(&mut state, &[far], None);
    state.residents.ready.remove(&snapshot_key());
    snapshot_project(&mut state, None, false);
    let missing = selection_key(-1, 0);
    state.session_views.get_mut(&owner).unwrap().chunks.get_mut(&missing).unwrap().resync_queued = true;
    selection_limits(&mut state, 1, 1_048_576);
    let mut encoding = SourceSnapshotEncoding::try_new(1).unwrap();
    let observed = encoded_tick_run(&mut state, &mut encoding);
    encoded_tick_join(&mut encoding);
    assert!(!observed.first_ready);
    assert!(observed.polls >= 4);
    assert_eq!(encoded_tick_snapshots(&observed.publication.unwrap(), owner), vec![far]);
    assert!(!state.session_views[&owner].chunks[&missing].resync_queued);
    assert!(!state.session_views[&owner].chunks[&missing].snapshot_sent);
}

#[test]
fn source_encoded_tick_malformed_prefix_peer() {
    let (mut state, sessions) = encoded_tick_fixture(2);
    let (owner, peer) = (sessions[0], sessions[1]);
    let good = [selection_key(0, 0), selection_key(1, 0), selection_key(2, 0), selection_key(5, 0)];
    selection_ready(&mut state, &good, None);
    let _prepared_goals = encoded_tick_managed_goals(&mut state);
    refusal_bad_ready(&mut state, good[1]);
    let bad = &state.residents.ready[&good[1]];
    state.acquisition.committed(good[1], bad.generation, bad.revision, None);
    let mut peer_actor = state.residents.actors.iter().find(|actor| actor.key == ActorKey::Player(peer)).unwrap().clone();
    peer_actor.motion = MotionState::new(mornlea_domain::MotionStateParts { position: FiniteVec3::try_new([80.5, 65.0, 0.5]).unwrap(), velocity: FiniteVec3::try_new([0.0; 3]).unwrap(), on_ground: true });
    let mut context = TickContext::for_tick(&mut state, encoded_tick_budget());
    context.stage(RuleEffect::Actor(peer_actor)).unwrap();
    context.commit_carried();
    drop(context);
    let mut encoding = SourceSnapshotEncoding::try_new(2).unwrap();
    let observed = encoded_tick_run(&mut state, &mut encoding);
    encoded_tick_join(&mut encoding);
    let publication = observed.publication.unwrap();
    assert_eq!(encoded_tick_snapshots(&publication, owner), vec![good[0]]);
    assert_eq!(encoded_tick_snapshots(&publication, peer), vec![good[3]]);
    assert_eq!(encoded_tick_snapshot_frames(&mut state, owner).len(), 1);
    assert_eq!(encoded_tick_snapshot_frames(&mut state, peer).len(), 1);
    assert_eq!(state.session(owner).unwrap().phase, SessionPhase::Retired);
    assert_eq!(state.session(peer).unwrap().phase, SessionPhase::Active);
    assert_eq!(state.tick_failure(), None);
}

#[test]
fn source_encoded_tick_abandoned_owner() {
    let (mut state, sessions) = encoded_tick_fixture(1);
    let owner = sessions[0];
    snapshot_project(&mut state, None, false);
    let previous = state.session_views[&owner].clone();
    let mut goals = encoded_tick_managed_goals(&mut state);
    let mut encoding = SourceSnapshotEncoding::try_new(1).unwrap();
    let mut pending = state.begin_source_tick(encoded_tick_budget(), &mut goals).unwrap();
    pending.prepare_encoding().unwrap();
    let ready = pending.poll_encoding(&mut encoding).unwrap();
    let charged = encoding.charged_requests();
    drop(pending);
    encoding.retain_current(|_, _, _, _| false).unwrap();
    let retained = encoding.retained_requests();
    encoded_tick_join(&mut encoding);
    assert!(!ready);
    assert_eq!(charged, 1);
    assert_eq!(retained, 0);
    assert_eq!(state.tick_failure(), Some(ServerError::Internal { invariant: "source tick continuation abandoned" }));
    assert_eq!(state.next_tick(), 0);
    assert!(state.sessions[&owner].outbox.is_empty());
    assert_eq!(state.session_views[&owner].chunks, previous.chunks);
}

#[test]
fn source_encoded_tick_late_preflight_failure() {
    use crate::core::step::set_dispatch_hook;
    struct RestoreHook;
    impl Drop for RestoreHook {
        fn drop(&mut self) { set_dispatch_hook(None); }
    }
    fn stale(context: &mut TickContext<'_>) -> Result<(), ServerError> {
        context.emit(marker_rejection(SessionKey::from_raw(u64::MAX).unwrap(), 19))
    }
    let (mut state, sessions) = encoded_tick_fixture(1);
    let owner = sessions[0];
    let mut encoding = SourceSnapshotEncoding::try_new(1).unwrap();
    set_dispatch_hook(Some(stale));
    let _hook = RestoreHook;
    let observed = encoded_tick_run(&mut state, &mut encoding);
    encoded_tick_join(&mut encoding);
    assert!(!observed.first_ready);
    assert!(observed.peak_requests > 0);
    let error = ServerError::StaleSession { session: SessionKey::from_raw(u64::MAX).unwrap() };
    assert_eq!(observed.publication, Err(error));
    assert_eq!(state.tick_failure(), Some(error));
    assert_eq!(state.next_tick(), 0);
    assert!(state.sessions[&owner].outbox.is_empty());
    assert!(!state.session_views[&owner].chunks[&snapshot_key()].snapshot_sent);
}

#[test]
fn source_encoded_tick_zero_count() {
    let (mut state, sessions) = encoded_tick_fixture(1);
    selection_limits(&mut state, 0, 0);
    let mut encoding = SourceSnapshotEncoding::try_new(1).unwrap();
    let observed = encoded_tick_run(&mut state, &mut encoding);
    encoded_tick_join(&mut encoding);
    assert!(observed.first_ready);
    assert_eq!(observed.peak_requests, 0);
    assert!(encoded_tick_snapshots(&observed.publication.unwrap(), sessions[0]).is_empty());
    assert_eq!(state.next_tick(), 1);
    assert!(!state.session_views[&sessions[0]].chunks[&snapshot_key()].snapshot_sent);
}
