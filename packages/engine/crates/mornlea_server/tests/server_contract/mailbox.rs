//! Bounded tick mailbox, chunk-result admission, and cancellation behavior.
//!
//! These cases drive the mailbox provider through the frozen state ports:
//! a zero command budget executes nothing and carries the whole batch,
//! small budgets apply the sorted prefix while carried envelopes keep their
//! original intake identity, the tick budget ceiling accepts the cap and
//! refuses one above it, a cancellation tombstone rejects exactly one late
//! chunk completion before repeats count as duplicates, and global command
//! saturation refuses either session's new submission without consuming any
//! accepted record.

use mornlea_domain::{ChunkPos, Command, Dimension, PlayerId};
use mornlea_protocol::{LoginStart, PlayIntent, admit_login};
use mornlea_server::contracts::{
    ChunkKey, ChunkRequestId, ChunkResult, Resource, ServerError, ServerLimits, SubmissionReceipt,
    TickBudget, TransportKind,
};
use mornlea_server::core::mailbox::{self, ChunkAdmission};
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

/// The smallest legal completion record: the error arm keeps the payload
/// small while the request identity and generation stay observable.
fn chunk_result(request: u64, generation: u64) -> ChunkResult {
    ChunkResult {
        request: ChunkRequestId::try_new(request).unwrap(),
        key: ChunkKey {
            dimension: Dimension::OVERWORLD,
            pos: ChunkPos::new(0, 0),
        },
        generation,
        result: Err(ServerError::Cancelled),
    }
}

#[test]
fn zero_and_prefix_carry() {
    let mut state = authority();
    for _ in 0..7 {
        state
            .advance_tick(TickBudget::try_new(0, 0, 0, 0, 0).unwrap())
            .unwrap();
    }
    let key = state
        .admit(login(21, "Carry"), TransportKind::Memory)
        .unwrap();
    let mut receipts = Vec::new();
    for sequence in [9u64, 9, 8] {
        let receipt = state.submit(key, sequenced(sequence)).unwrap();
        let SubmissionReceipt::QueuedForTick {
            tick,
            arrival_index,
        } = receipt
        else {
            panic!("a sequenced intent is queued for a tick");
        };
        assert_eq!(tick, 7, "the earliest eligible tick is the submit tick");
        receipts.push((sequence, arrival_index));
    }
    assert_eq!(receipts, vec![(9, 0), (9, 1), (8, 2)]);

    // Zero budget: nothing executes and the whole batch is carried back.
    let applied = mailbox::budgeted_batch(&mut state, 7, 0).unwrap();
    assert!(applied.is_empty(), "a zero budget applies nothing");

    // Budget one at tick 8: the sorted prefix is sequence 8, and the two
    // sequence-9 envelopes were carried by the previous pass yet keep the
    // tick-7 identity their receipts recorded.
    let applied = mailbox::budgeted_batch(&mut state, 8, 1).unwrap();
    assert_eq!(applied.len(), 1);
    assert_eq!(
        (
            applied[0].sequence(),
            applied[0].arrival_index(),
            applied[0].tick()
        ),
        (8, 2, 7),
        "sequence 8 applies with its original arrival and tick-7 identity"
    );

    // Budget one at tick 9: the first-arriving 9 applies. It was carried by
    // the previous pass, so its fields prove the suffix kept its identity.
    let applied = mailbox::budgeted_batch(&mut state, 9, 1).unwrap();
    assert_eq!(applied.len(), 1);
    assert_eq!(
        (
            applied[0].sequence(),
            applied[0].arrival_index(),
            applied[0].tick()
        ),
        (9, 0, 7),
        "the first-arriving 9 applies with its original arrival and tick-7 identity"
    );

    // Inspect the carried duplicate directly before it is walked, so the
    // last envelope's identity triple is observed at full strength rather
    // than only through its staleness.
    let carried = state.freeze_eligible(u64::MAX);
    assert_eq!(carried.len(), 1, "only the stale duplicate is still queued");
    assert_eq!(
        (
            carried[0].sequence(),
            carried[0].arrival_index(),
            carried[0].tick()
        ),
        (9, 1, 7),
        "the carried duplicate keeps its original arrival and tick-7 identity"
    );
    state.carry(carried).unwrap();

    // One more pass: the second 9 is walked, found at or below the
    // watermark, and stale-discarded rather than carried or returned.
    let applied = mailbox::budgeted_batch(&mut state, 10, 1).unwrap();
    assert!(applied.is_empty(), "the stale duplicate applies nothing");

    assert!(
        state.freeze_eligible(u64::MAX).is_empty(),
        "the queue is fully drained after the stale discard"
    );
    let facts = state.session(key).unwrap();
    assert_eq!(facts.last_applied_sequence, 9);
    assert_eq!(facts.next_arrival, 3);
}

#[test]
fn budget_ceiling_accepts_cap_and_refuses_plus_one() {
    assert!(TickBudget::try_new(4096, 0, 0, 0, 0).is_ok());
    assert!(matches!(
        TickBudget::try_new(4097, 0, 0, 0, 0),
        Err(ServerError::Capacity {
            resource: Resource::Commands,
            limit: 4096,
            observed: 4097,
        })
    ));
}

#[test]
fn late_chunk_once() {
    let mut state = authority();
    let request = ChunkRequestId::try_new(1).unwrap();
    mailbox::cancel_chunk_request(&mut state, request);
    let first = mailbox::admit_chunk_result(&mut state, chunk_result(1, 3)).unwrap();
    assert_eq!(first, ChunkAdmission::CancelledDiscarded);
    let second = mailbox::admit_chunk_result(&mut state, chunk_result(1, 3)).unwrap();
    assert_eq!(
        second,
        ChunkAdmission::DuplicateDiscarded,
        "the consumed tombstone turns the repeat into a duplicate"
    );
    assert_eq!(state.chunk_discard_counts(), (1, 1));
    let drained = state.drain_chunks(64);
    assert!(
        drained.is_empty(),
        "neither discarded completion is ever installed"
    );
}

#[test]
fn duplicate_result_without_cancellation() {
    let mut state = authority();
    let first = mailbox::admit_chunk_result(&mut state, chunk_result(2, 5)).unwrap();
    assert_eq!(first, ChunkAdmission::Admitted);
    let second = mailbox::admit_chunk_result(&mut state, chunk_result(2, 5)).unwrap();
    assert_eq!(second, ChunkAdmission::DuplicateDiscarded);
    assert_eq!(state.chunk_discard_counts(), (0, 1));
    let drained = state.drain_chunks(64);
    assert_eq!(drained.len(), 1, "exactly one record is queued");
    assert_eq!(drained[0].request.get(), 2);
    assert_eq!(drained[0].generation, 5);
}

#[test]
fn full_global_queue_rejects_either_session_intact() {
    let mut state = authority();
    let first = state
        .admit(login(31, "One"), TransportKind::Memory)
        .unwrap();
    let second = state
        .admit(login(32, "Two"), TransportKind::Memory)
        .unwrap();
    let cap = state.limits().queued_commands();
    for index in 0..cap {
        let (session, sequence) = if index % 2 == 0 {
            (first, index as u64 / 2 + 1)
        } else {
            (second, index as u64 / 2 + 1)
        };
        state
            .submit(session, sequenced(sequence))
            .expect("submissions alternate until the global cap is filled");
    }
    for session in [first, second] {
        assert!(matches!(
            state.submit(session, sequenced(u64::MAX)),
            Err(ServerError::Capacity {
                resource: Resource::Commands,
                ..
            })
        ));
    }
    for session in [first, second] {
        let facts = state.session(session).unwrap();
        assert_eq!(
            facts.next_arrival,
            cap as u64 / 2,
            "the global refusal consumes no arrival index"
        );
        assert_eq!(
            facts.last_applied_sequence, 0,
            "the global refusal consumes no sequence watermark"
        );
    }
    let queued = state.freeze_eligible(u64::MAX);
    assert_eq!(
        queued.len(),
        cap,
        "both sessions' accepted records survive the rejections intact"
    );
    state.carry(queued).unwrap();
}
