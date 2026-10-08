//! Tick reducer mailbox discipline through the endpoint surface.
//!
//! These cases drive the serial reducer, not an isolated provider: a drained
//! mailbox counts every envelope once across the mailbox-exact counters, a
//! retired session never executes, watermark duplicates drop stale without a
//! carry, and delegation keeps the endpoint validations. Order pins live in
//! the replay order suite; this target owns the mailbox counters.

use mornlea_domain::{Command, PlayerId};
use mornlea_protocol::{LoginStart, PlayIntent, admit_login};
use mornlea_server::contracts::{ServerLimits, TickBudget, TransportKind};
use mornlea_server::core::step::reduce_tick;
use mornlea_server::state::AuthorityState;

fn authority() -> AuthorityState {
    AuthorityState::try_new(
        ServerLimits::try_new(8, 4096, 512, 64, 64, 1_048_576).unwrap(),
        7,
    )
    .unwrap()
}

fn login(tag: u8, name: &str) -> mornlea_protocol::AdmittedLogin {
    let mut bytes = [0u8; 16];
    bytes[0] = tag;
    bytes[6] = 0x40;
    bytes[8] = 0x80;
    let id = PlayerId::try_from_bytes(bytes).unwrap();
    let start = LoginStart::new(id, name, 8).unwrap();
    let inbound = LoginStart::decode_inbound(&start.encode().unwrap()).unwrap();
    admit_login(inbound).unwrap()
}

fn sequenced(sequence: u64) -> PlayIntent {
    PlayIntent::Sequenced {
        sequence,
        command: Command::CloseContainer,
    }
}

fn full() -> TickBudget {
    TickBudget::full()
}

/// An authority whose shared command queue holds at most `cap` envelopes.
fn authority_with_queue(cap: usize) -> AuthorityState {
    AuthorityState::try_new(
        ServerLimits::try_new(8, cap, 512, 64, 64, 1_048_576).unwrap(),
        7,
    )
    .unwrap()
}

/// Reads the queued envelopes as `(sequence, arrival_index)` pairs and puts
/// them back unchanged, so a test can observe the queue between ticks.
fn queued(state: &mut AuthorityState) -> Vec<(u64, u64)> {
    let batch = state.freeze_eligible(u64::MAX);
    let pairs = batch
        .iter()
        .map(|envelope| (envelope.sequence(), envelope.arrival_index()))
        .collect();
    state.carry(batch).unwrap();
    pairs
}

#[test]
fn empty_dispatch_runs_no_provider_with_empty_publication() {
    let mut state = authority();
    let publication = reduce_tick(&mut state, full()).unwrap();
    assert_eq!(publication.tick, 0, "the first tick executes");
    assert!(publication.events.is_empty(), "no provider runs");
    assert!(
        publication.control.is_empty(),
        "control stays handshake-owned"
    );
    assert_eq!(publication.counters.executed_tick, 0);
    assert_eq!(publication.counters.commands, 0);
    assert_eq!(publication.counters.carried, 0);
    assert_eq!(publication.counters.stale, 0);
    assert!(publication.counters.fluid_by_dimension.is_empty());
    assert!(publication.counters.rescan_by_dimension.is_empty());
    assert_eq!(publication.counters.farmland_checks, 0);
    assert_eq!(publication.counters.farmland_reads, 0);
}

#[test]
fn retired_session_commands_drop_stale_never_carried() {
    let mut state = authority();
    let key = state
        .admit(login(21, "Retired"), TransportKind::Memory)
        .unwrap();
    state.submit(key, sequenced(1)).unwrap();
    state.submit(key, sequenced(2)).unwrap();
    state
        .close_session(key, mornlea_server::contracts::CloseReason::PeerGone)
        .unwrap();
    let publication = reduce_tick(&mut state, full()).unwrap();
    assert_eq!(publication.counters.commands, 2, "both envelopes drain");
    assert_eq!(publication.counters.stale, 2, "retired commands drop stale");
    assert_eq!(publication.counters.carried, 0);
    assert!(
        publication.events.is_empty(),
        "a retired session executes nothing"
    );
    // A retired session never returns: the next tick drains nothing.
    let again = reduce_tick(&mut state, full()).unwrap();
    assert_eq!(again.counters.commands, 0);
    assert_eq!(again.counters.stale, 0);
}

#[test]
fn stale_batch_ordering_counts_duplicates() {
    let mut state = authority();
    let key = state
        .admit(login(22, "Stale"), TransportKind::Memory)
        .unwrap();
    for sequence in [9u64, 9, 8] {
        state.submit(key, sequenced(sequence)).unwrap();
    }
    let publication = reduce_tick(&mut state, full()).unwrap();
    assert_eq!(
        publication.counters.commands, 3,
        "every envelope drains once"
    );
    assert_eq!(
        publication.counters.stale, 1,
        "the carried duplicate past the watermark drops stale"
    );
    assert_eq!(publication.counters.carried, 0);
}

#[test]
fn advance_tick_delegation_keeps_validations_and_drains() {
    let mut state = authority();
    let key = state
        .admit(login(23, "Delegate"), TransportKind::Memory)
        .unwrap();
    state.submit(key, sequenced(1)).unwrap();
    let publication = state.advance_tick(full()).unwrap();
    assert_eq!(publication.tick, 0);
    assert_eq!(publication.counters.commands, 1);
    assert_eq!(
        state.next_tick(),
        1,
        "the endpoint still owns the tick bump"
    );
}

/// Providers execute with the pre-bump tick: the first publication carries
/// tick zero onto the wire (pose and damage events name the same tick the
/// publication carries), and the counter bumps only after the reducer
/// returns. The mailbox flows at the same tick in both advances.
#[test]
fn advance_tick_publishes_pre_bump_tick() {
    let mut state = authority();
    let key = state
        .admit(login(24, "Tick"), TransportKind::Memory)
        .unwrap();
    state.submit(key, sequenced(1)).unwrap();
    let first = state.advance_tick(full()).unwrap();
    assert_eq!(first.tick, 0);
    assert_eq!(first.counters.executed_tick, 0);
    assert_eq!(first.counters.commands, 1);
    assert_eq!(state.next_tick(), 1);
    let second = state.advance_tick(full()).unwrap();
    assert_eq!(second.tick, 1);
    assert_eq!(second.counters.executed_tick, 1);
    assert_eq!(state.next_tick(), 2);
}

/// Cross-tick schedule carry: work queued due in the future waits out its
/// tick on the authority and fires when its due tick executes. The fluid
/// item and the farmland candidate below are seeded directly into the
/// carried schedules; nothing is due on the first tick, and both settle on
/// the second, charging exactly one unit of their own budget each.
#[test]
fn future_dues_fire_next_tick() {
    use mornlea_domain::{BlockPos, ChunkPos, Dimension};
    use mornlea_server::contracts::ChunkKey;

    let mut state = authority();
    let key = ChunkKey {
        dimension: Dimension::OVERWORLD,
        pos: ChunkPos::new(0, 0),
    };
    state
        .fluid_schedule_mut()
        .enqueue_fluid(key, BlockPos::new(0, 64, 0), 1);
    state
        .farmland_schedule_mut()
        .enqueue_candidate(key, BlockPos::new(0, 64, 0), 1);
    let first = state.advance_tick(full()).unwrap();
    assert_eq!(first.tick, 0);
    assert!(
        first.counters.fluid_by_dimension.is_empty(),
        "nothing is due on the first tick"
    );
    assert_eq!(first.counters.farmland_checks, 0);
    assert_eq!(
        state.fluid_schedule().pending_fluid(Dimension::OVERWORLD),
        1,
        "the future item waits on the authority"
    );
    assert_eq!(
        state
            .farmland_schedule()
            .pending_candidates(Dimension::OVERWORLD),
        1
    );
    let second = state.advance_tick(full()).unwrap();
    assert_eq!(second.tick, 1);
    assert_eq!(
        second.counters.fluid_by_dimension,
        vec![(Dimension::OVERWORLD, 1)],
        "the fluid item fires on its due tick"
    );
    assert_eq!(second.counters.farmland_checks, 1);
    assert_eq!(second.counters.farmland_reads, 0);
    assert_eq!(
        state.fluid_schedule().pending_fluid(Dimension::OVERWORLD),
        0,
        "a fired item never runs twice"
    );
    assert_eq!(
        state
            .farmland_schedule()
            .pending_candidates(Dimension::OVERWORLD),
        0
    );
}

#[test]
fn actual_final_reduces_accepted_commands_once_without_publication() {
    use mornlea_server::contracts::{ServerError, ServerPhase};
    use mornlea_server::core::step::AuthoritativeFinalReducer;
    let mut state = authority();
    let key = state
        .prepare(login(25, "Final"), TransportKind::Memory)
        .unwrap();
    state.install(key, None).unwrap();
    state.activate(key).unwrap();
    state.advance_tick(full()).unwrap();
    state.take_outbox(key, 512, usize::MAX).unwrap();
    state.submit(key, sequenced(7)).unwrap();
    let tick = state.next_tick();
    let before = state.residents().environment.unwrap();
    state.begin_close();
    assert_eq!(state.run_final(&mut AuthoritativeFinalReducer), Ok(tick));
    assert!(
        state.take_outbox(key, 512, usize::MAX).unwrap().is_empty(),
        "final engine appends no frames"
    );
    assert_eq!(state.session(key).unwrap().last_applied_sequence, 7);
    assert_eq!(state.next_tick(), tick + 1);
    let after = state.residents().environment.unwrap();
    assert_eq!(after.world_time, before.world_time + 1);
    assert_eq!(after.next_tick, before.next_tick + 1);
    assert_eq!(
        state.run_final(&mut AuthoritativeFinalReducer),
        Err(ServerError::InvalidState {
            phase: ServerPhase::Closing
        })
    );
    assert_eq!(
        state.advance_tick(full()),
        Err(ServerError::InvalidState {
            phase: ServerPhase::Closing
        })
    );
    assert_eq!(state.next_tick(), tick + 1);
    assert_eq!(state.residents().environment, Some(after));
}

/// A queue filled to its limit and drained under a partial command budget
/// carries the remainder back in submission order without growing past the
/// limit, and later ticks drain every carried envelope exactly once.
#[test]
fn full_queue_partial_budget_carries_remainder_within_limit() {
    const CAP: usize = 6;
    let mut state = authority_with_queue(CAP);
    let key = state
        .admit(login(26, "FullCarry"), TransportKind::Memory)
        .unwrap();
    for sequence in 1..=CAP as u64 {
        state.submit(key, sequenced(sequence)).unwrap();
    }
    let partial = TickBudget::try_new(2, 0, 0, 0, 0).unwrap();

    let first = state.advance_tick(partial).unwrap();
    assert_eq!(
        (
            first.counters.commands,
            first.counters.carried,
            first.counters.stale
        ),
        (CAP, 4, 0),
        "the whole queue drains, two execute and four carry back"
    );
    assert_eq!(state.session(key).unwrap().last_applied_sequence, 2);
    let remainder = queued(&mut state);
    assert!(remainder.len() <= CAP, "the carry never exceeds the limit");
    assert_eq!(
        remainder,
        vec![(3, 2), (4, 3), (5, 4), (6, 5)],
        "the remainder keeps submission order and arrival identity"
    );

    let second = state.advance_tick(partial).unwrap();
    assert_eq!(
        (
            second.counters.commands,
            second.counters.carried,
            second.counters.stale
        ),
        (4, 2, 0)
    );
    assert_eq!(state.session(key).unwrap().last_applied_sequence, 4);
    assert_eq!(queued(&mut state), vec![(5, 4), (6, 5)]);

    let third = state.advance_tick(partial).unwrap();
    assert_eq!(
        (
            third.counters.commands,
            third.counters.carried,
            third.counters.stale
        ),
        (2, 0, 0)
    );
    assert_eq!(
        state.session(key).unwrap().last_applied_sequence,
        CAP as u64,
        "every submitted command executed once and none was lost"
    );
    assert!(queued(&mut state).is_empty());
}

/// After a partial-budget tick carries work back, intake admits exactly the
/// space the executed prefix freed and then refuses with the command limit.
#[test]
fn intake_after_partial_carry_admits_freed_space_then_refuses() {
    use mornlea_server::contracts::{Resource, ServerError, SubmissionReceipt};
    const CAP: usize = 6;
    let mut state = authority_with_queue(CAP);
    let key = state
        .admit(login(27, "FreedSpace"), TransportKind::Memory)
        .unwrap();
    for sequence in 1..=CAP as u64 {
        state.submit(key, sequenced(sequence)).unwrap();
    }
    state
        .advance_tick(TickBudget::try_new(2, 0, 0, 0, 0).unwrap())
        .unwrap();

    for sequence in [7u64, 8] {
        assert!(matches!(
            state.submit(key, sequenced(sequence)),
            Ok(SubmissionReceipt::QueuedForTick { .. })
        ));
    }
    assert_eq!(
        state.submit(key, sequenced(9)),
        Err(ServerError::Capacity {
            resource: Resource::Commands,
            limit: CAP,
            observed: CAP + 1,
        }),
        "the queue is full again once the freed space is used"
    );
    assert_eq!(
        queued(&mut state),
        vec![(3, 2), (4, 3), (5, 4), (6, 5), (7, 6), (8, 7)],
        "the refusal consumes no arrival index and drops nothing"
    );

    let drained = state.advance_tick(full()).unwrap();
    assert_eq!(
        (
            drained.counters.commands,
            drained.counters.carried,
            drained.counters.stale
        ),
        (CAP, 0, 0)
    );
    assert_eq!(state.session(key).unwrap().last_applied_sequence, 8);
}
