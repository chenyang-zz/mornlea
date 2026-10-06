//! Prepared lifecycle fences qualify the private owner without claiming a shutdown composer.
use super::*;

fn raw(tag: u8) -> [u8; 16] {
    let mut bytes = [0; 16];
    bytes[0] = tag;
    bytes[6] = 0x40;
    bytes[8] = 0x80;
    bytes
}
fn state() -> AuthorityState {
    let mut state = AuthorityState::try_new(
        ServerLimits::try_new(8, 4096, 512, 64, 64, 1_048_576).unwrap(),
        42,
    )
    .unwrap();
    state.enable_source_player_restoration(1).unwrap();
    state.enable_live_chunks().unwrap();
    state.enable_actor_saves().unwrap();
    state.enable_player_persistence().unwrap();
    let saved = StoredPlayerId::from_bytes(raw(1));
    let loaded = StoredCompanions {
        source_schema: 5,
        revision: 9,
        agent_namespace_id: StoredPlayerId::from_bytes(raw(240)),
        records: vec![CompanionBody {
            id: saved,
            dimension: 0,
            position: [8.5, 65.0, 8.5],
            yaw: 0.0,
            pitch: 0.0,
            inventory: Default::default(),
        }],
        lifecycles: vec![StoredCompanionLifecycle {
            id: saved,
            active: true,
            memory_epoch: 7,
            memory_revision: 2,
            memory_operation_id: StoredPlayerId::from_bytes(raw(180)),
            summary: "old".into(),
            tombstone_operation_id: StoredPlayerId::default(),
        }],
        queues: Vec::new(),
    };
    state
        .enable_companion_persistence(
            &[(
                CompanionId::try_from_bytes(raw(1)).unwrap(),
                CompanionName::try_from_canonical("Nova".into()).unwrap(),
            )],
            loaded,
        )
        .unwrap();
    state
}
fn replace(state: &mut AuthorityState) -> Result<(), ServerError> {
    state.replace_companion_memory(
        CompanionId::try_from_bytes(raw(1)).unwrap(),
        7,
        1,
        2,
        OperationId::try_from_bytes(raw(180)).unwrap(),
        "old".into(),
    )
}

#[test]
fn frozen_ledger_refuses_even_exact_installed_memory_before_idempotence() {
    let mut state = state();
    let old = state.companion_memory_lifecycles().unwrap().to_vec();
    state.actor_saves.as_mut().unwrap().freeze();
    assert_eq!(
        replace(&mut state),
        Err(ServerError::InvalidState {
            phase: ServerPhase::Closing
        })
    );
    assert_eq!(state.companion_memory_lifecycles().unwrap(), old);
}
#[test]
fn closed_owner_remains_readable_and_refuses_replacement_without_mutation() {
    let mut state = state();
    let old = state.companion_memory_lifecycles().unwrap().to_vec();
    state.phase = ServerPhase::Closed;
    assert_eq!(
        replace(&mut state),
        Err(ServerError::InvalidState {
            phase: ServerPhase::Closed
        })
    );
    assert_eq!(state.companion_memory_lifecycles().unwrap(), old);
}
#[test]
fn sticky_tick_failure_preserves_memory_and_returns_the_same_retained_error() {
    let mut state = state();
    let old = state.companion_memory_lifecycles().unwrap().to_vec();
    let error = ServerError::Internal {
        invariant: "prepared sticky memory fence",
    };
    state.fail_tick(error);
    assert_eq!(replace(&mut state), Err(error));
    assert_eq!(state.companion_memory_lifecycles().unwrap(), old);
}
