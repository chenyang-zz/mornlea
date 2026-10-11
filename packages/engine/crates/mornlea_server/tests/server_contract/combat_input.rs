//! Bounded tick-local hostile choices shared by action and combat providers.
use mornlea_domain::{HostileId, PlayerId};
use mornlea_protocol::{LoginStart, admit_login};
use mornlea_server::{contracts::*, state::AuthorityState};

fn targets() -> [SessionKey; 2] {
    let mut state = AuthorityState::try_new(
        ServerLimits::try_new(8, 4096, 512, 64, 64, 1_048_576).unwrap(),
        0,
    )
    .unwrap();
    [1, 2].map(|tag| {
        let mut bytes = [0u8; 16];
        bytes[0] = tag;
        bytes[6] = 0x40;
        bytes[8] = 0x80;
        let start = LoginStart::new(
            PlayerId::try_from_bytes(bytes).unwrap(),
            format!("P{tag}"),
            8,
        )
        .unwrap();
        let login =
            admit_login(LoginStart::decode_inbound(&start.encode().unwrap()).unwrap()).unwrap();
        state.admit(login, TransportKind::Memory).unwrap()
    })
}

#[test]
fn empty_batch_preserves_scalar_tick() {
    for tick in [0, 17, u64::MAX] {
        let batch = HostileMeleeBatch::try_new(tick, &[]).unwrap();
        assert_eq!(batch.tick(), tick);
        assert!(batch.entries().is_empty());
    }
}

#[test]
fn maximum_batch_is_canonical_and_does_not_rewrite_source() {
    let targets = targets();
    let source: Vec<_> = (1..=64)
        .rev()
        .map(|id| {
            HostileMeleeAttack::new(HostileId::try_new(id).unwrap(), targets[(id % 2) as usize])
        })
        .collect();
    let unchanged = source.clone();
    let batch = HostileMeleeBatch::try_new(29, &source).unwrap();
    assert_eq!(source, unchanged);
    assert_eq!(batch.entries().len(), 64);
    for (index, entry) in batch.entries().iter().enumerate() {
        assert_eq!(entry.attacker().get(), index as u64 + 1);
        assert_eq!(entry.target(), targets[(index + 1) % 2]);
    }
    let mut too_many = source.clone();
    too_many.push(HostileMeleeAttack::new(
        HostileId::try_new(65).unwrap(),
        targets[0],
    ));
    assert_eq!(
        HostileMeleeBatch::try_new(29, &too_many),
        Err(ServerError::Capacity {
            resource: Resource::RuleEffects,
            limit: 64,
            observed: 65,
        })
    );
    assert_eq!(batch.entries().len(), 64);
}

#[test]
fn duplicate_attacker_refuses_same_or_different_targets() {
    let targets = targets();
    for second in targets {
        let first = HostileMeleeAttack::new(HostileId::try_new(7).unwrap(), targets[0]);
        let source = [first, HostileMeleeAttack::new(first.attacker(), second)];
        let unchanged = source;
        assert_eq!(
            HostileMeleeBatch::try_new(9, &source),
            Err(ServerError::InvalidInput {
                field: "hostile_melee"
            })
        );
        assert_eq!(source, unchanged);
    }
}

// These doubles prove cross-provider value flow only; neither grants a hit.
fn action_double(tick: u64, chosen: SessionKey) -> HostileMeleeBatch {
    HostileMeleeBatch::try_new(
        tick,
        &[HostileMeleeAttack::new(
            HostileId::try_new(41).unwrap(),
            chosen,
        )],
    )
    .unwrap()
}
fn combat_double(batch: &HostileMeleeBatch) -> (u64, u64, SessionKey) {
    let entry = batch.entries()[0];
    (batch.tick(), entry.attacker().get(), entry.target())
}

#[test]
fn callable_consumers_preserve_target_session_and_tick() {
    let targets = targets();
    let old = action_double(300, targets[1]);
    let new = action_double(301, targets[0]);
    assert_eq!(combat_double(&old), (300, 41, targets[1]));
    assert_eq!(combat_double(&new), (301, 41, targets[0]));
    assert_eq!(combat_double(&old), (300, 41, targets[1]));
}
