//! Session admission and bounded sequenced intake behavior.
//!
//! These cases drive the session provider module through the frozen state
//! ports: active-player capacity, command-queue saturation that refuses
//! without advancing session watermarks, sorted reduction that applies one
//! envelope per sequence and silently discards the stale duplicate,
//! retire-once semantics, and the control plane that never enqueues a domain
//! command.

use mornlea_domain::{Command, CommandText, PlayerId};
use mornlea_protocol::{LoginStart, PlayIntent, admit_login};
use mornlea_server::contracts::{
    CloseReason, Resource, ServerError, SessionPhase, SubmissionReceipt, TransportKind,
};
use mornlea_server::core::session;
use mornlea_server::state::AuthorityState;

fn authority() -> AuthorityState {
    AuthorityState::try_new(
        mornlea_server::contracts::ServerLimits::try_new(8, 4096, 512, 64, 64, 1_048_576).unwrap(),
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

#[test]
fn eighth_player_admits_and_ninth_hits_capacity() {
    let mut state = authority();
    let mut previous = None;
    for tag in 1..=8u8 {
        let name = format!("P{tag}");
        let key = session::admit(&mut state, login(tag, &name), TransportKind::Memory)
            .expect("eight distinct players admit");
        assert_eq!(state.session(key).unwrap().phase, SessionPhase::Active);
        let raw = key.get();
        assert!(raw > 0, "session keys are nonzero");
        if let Some(last) = previous {
            assert!(key > last, "session keys are monotonic");
        }
        previous = Some(key);
    }
    let ninth = session::admit(&mut state, login(9, "Ivy"), TransportKind::Tcp);
    assert!(matches!(
        ninth,
        Err(ServerError::Capacity {
            resource: Resource::Players,
            ..
        })
    ));
}

#[test]
fn duplicate_live_identity_is_refused_until_retirement() {
    let mut state = authority();
    let first = session::admit(&mut state, login(30, "Ada"), TransportKind::Memory)
        .expect("the first admission succeeds");
    let duplicate = session::admit(&mut state, login(30, "Ada"), TransportKind::Tcp);
    assert!(matches!(
        duplicate,
        Err(ServerError::InvalidInput { field }) if field == "player_id"
    ));
    session::close_session(&mut state, first, CloseReason::PeerGone).unwrap();
    let readmitted = session::admit(&mut state, login(30, "Ada"), TransportKind::Memory);
    assert!(
        readmitted.is_ok(),
        "a retired identity frees its player slot"
    );
}

#[test]
fn command_cap_plus_one_rejects_without_advancing_state() {
    let mut state = authority();
    let key = session::admit(&mut state, login(20, "Cap"), TransportKind::Memory).unwrap();
    let cap = state.limits().queued_commands();
    for sequence in 1..=cap as u64 {
        session::submit(&mut state, key, sequenced(sequence))
            .expect("submissions up to the cap are queued");
    }
    let before = state.session(key).unwrap();
    assert_eq!(before.last_applied_sequence, 0);
    assert_eq!(before.next_arrival, cap as u64);
    let overflow = session::submit(&mut state, key, sequenced(cap as u64 + 1));
    assert!(matches!(
        overflow,
        Err(ServerError::Capacity {
            resource: Resource::Commands,
            ..
        })
    ));
    let after = state.session(key).unwrap();
    assert_eq!(after.last_applied_sequence, before.last_applied_sequence);
    assert_eq!(after.next_arrival, before.next_arrival);
}

#[test]
fn sorted_batch_applies_eight_first_nine_and_discards_duplicate() {
    let mut state = authority();
    let key = session::admit(&mut state, login(21, "Dup"), TransportKind::Memory).unwrap();
    for (sequence, arrival) in [(9u64, 0u64), (9, 1), (8, 2)] {
        let receipt = session::submit(&mut state, key, sequenced(sequence)).unwrap();
        assert_eq!(
            receipt,
            SubmissionReceipt::QueuedForTick {
                tick: 0,
                arrival_index: arrival,
            }
        );
    }
    let applied = session::apply_sorted_batch(&mut state, 0)
        .expect("the sorted walk of an accepted batch never refuses");
    assert_eq!(applied.len(), 2);
    assert_eq!(
        (applied[0].sequence(), applied[0].arrival_index()),
        (8, 2),
        "sequence 8 applies first"
    );
    assert_eq!(
        (applied[1].sequence(), applied[1].arrival_index()),
        (9, 0),
        "the first-arriving 9 applies second; the second 9 is absent"
    );
    let facts = state.session(key).unwrap();
    assert_eq!(facts.last_applied_sequence, 9);
    let drained = session::apply_sorted_batch(&mut state, 0).unwrap();
    assert!(drained.is_empty(), "the frozen batch was consumed once");
}

#[test]
fn retired_key_refuses_submit() {
    let mut state = authority();
    let key = session::admit(&mut state, login(22, "Gone"), TransportKind::Memory).unwrap();
    session::close_session(&mut state, key, CloseReason::PeerGone)
        .expect("the first retirement succeeds");
    assert!(matches!(
        session::submit(&mut state, key, sequenced(1)),
        Err(ServerError::StaleSession { .. })
    ));
    assert!(matches!(
        session::close_session(&mut state, key, CloseReason::PeerGone),
        Err(ServerError::StaleSession { .. })
    ));
}

#[test]
fn chat_and_keepalive_stay_control_plane() {
    let mut state = authority();
    let key = session::admit(&mut state, login(23, "Chatty"), TransportKind::Memory).unwrap();
    let chat = PlayIntent::Chat(
        mornlea_domain::ChatIntent::try_new(
            CommandText::try_from_canonical("hello".to_owned()).unwrap(),
        )
        .unwrap(),
    );
    assert_eq!(
        session::submit(&mut state, key, chat).unwrap(),
        SubmissionReceipt::ControlAccepted
    );
    assert_eq!(
        session::submit(&mut state, key, PlayIntent::KeepAliveReply { token: 1 }).unwrap(),
        SubmissionReceipt::ControlAccepted
    );
    let applied = session::apply_sorted_batch(&mut state, 0).unwrap();
    assert!(applied.is_empty(), "control traffic enqueues no command");
    let facts = state.session(key).unwrap();
    assert_eq!(facts.next_arrival, 0, "control traffic consumes no arrival");
    assert_eq!(facts.last_applied_sequence, 0);
}
