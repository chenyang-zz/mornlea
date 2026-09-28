//! Sessionless companion candidate ingress: provenance and admission.
//!
//! These cases drive the ingress provider directly. The expected values
//! mirror the Go authority: the bounded non-blocking companion inbox and
//! its full-inbox rejection in
//! `packages/server/sim/runtime/action_inbox_test.go`, the per-field
//! planning-identity comparison in `validCompanionPlanningIdentity`
//! (`packages/server/server/companion_manager.go`), and the plan target
//! vertical bound `validPlanBlockY`
//! (`packages/shared/companion/plan_types.go`). Every refusal must leave
//! the queue and the arrival counter untouched.

use mornlea_domain::{BlockPos, CompanionId};
use mornlea_server::contracts::{
    AgentRequestId, CompanionAction, CompanionActionEnvelope, Resource, RunId, ServerError,
    SnapshotId,
};
use mornlea_server::core::companion_ingress::{
    CompanionIngress, CompanionTaskGate, SessionlessCandidate,
};

/// Compile-time proof that the candidate envelope is inside the sealed
/// sessionless set. The bound has exactly one implementer and the seal
/// module makes any further implementation unnameable, so a human-session
/// handle such as `SessionKey` can never satisfy it: forging a human
/// session is a type error, not a runtime refusal.
fn assert_sessionless<C: SessionlessCandidate>() {}

/// Canonical UUIDv4 test identity: version nibble 4 at byte 6, RFC 4122
/// variant at byte 8, unique tag byte at byte 0.
fn uuid(tag: u8) -> [u8; 16] {
    let mut bytes = [0u8; 16];
    bytes[0] = tag;
    bytes[6] = 0x40;
    bytes[8] = 0x80;
    bytes
}

fn companion(tag: u8) -> CompanionId {
    CompanionId::try_from_bytes(uuid(tag)).unwrap()
}

/// The frozen active-task gate a dispatch would hold for one companion:
/// registered request identity, run and snapshot identity, digest, and the
/// positive task generation and attempt. The identity bytes derive from the
/// companion tag, mirroring the unique request identity each dispatch
/// registers.
fn gate_for(id: CompanionId) -> CompanionTaskGate {
    let tag = id.bytes()[0];
    CompanionTaskGate::try_new(
        id,
        AgentRequestId::try_from_bytes(uuid(tag.wrapping_add(0xA0))).unwrap(),
        RunId::try_from_bytes(uuid(tag.wrapping_add(0xB0))).unwrap(),
        SnapshotId::try_from_bytes(uuid(tag.wrapping_add(0xC0))).unwrap(),
        [0xAB; 32],
        5,
        7,
    )
    .unwrap()
}

/// A structurally valid candidate bound to the gate: a release action with
/// no payload, a source tick at or before the admission tick, and the
/// gate's full provenance.
fn envelope_for(id: CompanionId, gate: &CompanionTaskGate) -> CompanionActionEnvelope {
    CompanionActionEnvelope::try_new(
        id,
        // A frozen source tick at or below every admission tick used here.
        10,
        gate.request_id(),
        gate.run_id(),
        gate.snapshot_id(),
        gate.generation(),
        gate.attempt(),
        gate.snapshot_digest(),
        CompanionAction::MineRelease,
    )
    .unwrap()
}

fn expect_invalid(error: ServerError, field: &'static str) {
    assert_eq!(error, ServerError::InvalidInput { field });
}

/// The frozen ingress row: a wrong attempt and a wrong digest are each
/// refused with the arrival index unchanged, four valid candidates fill
/// the inbox, and the fifth returns `Capacity` with the index and the four
/// stored candidates untouched.
#[test]
fn wrong_attempt_digest_and_cap() {
    let mut ingress = CompanionIngress::try_new().unwrap();
    let tick = 100;
    let gate = gate_for(companion(0x11));

    let mut wrong_attempt = envelope_for(companion(0x11), &gate);
    wrong_attempt.attempt += 1;
    expect_invalid(
        ingress.admit(&gate, tick, wrong_attempt).unwrap_err(),
        "companion_attempt",
    );
    let mut wrong_digest = envelope_for(companion(0x11), &gate);
    wrong_digest.snapshot_digest = [0xCD; 32];
    expect_invalid(
        ingress.admit(&gate, tick, wrong_digest).unwrap_err(),
        "snapshot_digest",
    );
    assert_eq!(ingress.pending(), 0);

    for tag in 1..=4u8 {
        let id = companion(tag);
        let candidate_gate = gate_for(id);
        let receipt = ingress
            .admit(&candidate_gate, tick, envelope_for(id, &candidate_gate))
            .unwrap();
        assert_eq!(receipt.tick(), tick);
        assert_eq!(receipt.arrival_index(), u64::from(tag - 1));
    }
    assert_eq!(ingress.pending(), 4);

    let overflow_gate = gate_for(companion(9));
    let fifth = envelope_for(companion(9), &overflow_gate);
    assert_eq!(
        ingress.admit(&overflow_gate, tick, fifth).unwrap_err(),
        ServerError::Capacity {
            resource: Resource::Commands,
            limit: 4,
            observed: 5,
        }
    );
    assert_eq!(ingress.pending(), 4);

    // No refusal reserved an index: the stored candidates keep their
    // original indices and the next admission continues at index 4.
    let drained = ingress.drain();
    assert_eq!(drained.len(), 4);
    for (position, queued) in drained.iter().enumerate() {
        assert_eq!(queued.arrival_index, position as u64);
    }
    let next_gate = gate_for(companion(0x21));
    let next = envelope_for(companion(0x21), &next_gate);
    assert_eq!(
        ingress
            .admit(&next_gate, tick, next)
            .unwrap()
            .arrival_index(),
        4
    );
}

/// A valid candidate is admitted with its full provenance intact, and a
/// repeated request identity is refused as a duplicate without consuming
/// queue space or an arrival index.
#[test]
fn valid_and_duplicate() {
    let mut ingress = CompanionIngress::try_new().unwrap();
    let tick = 40;
    let gate = gate_for(companion(0x31));
    let candidate = envelope_for(companion(0x31), &gate);

    let receipt = ingress.admit(&gate, tick, candidate.clone()).unwrap();
    assert_eq!(receipt.tick(), tick);
    assert_eq!(receipt.arrival_index(), 0);
    assert_eq!(ingress.pending(), 1);

    // A repeated request identity is refused while the original is still
    // pending, without consuming queue space.
    let duplicate = envelope_for(companion(0x31), &gate);
    expect_invalid(
        ingress.admit(&gate, tick, duplicate).unwrap_err(),
        "companion_action",
    );
    assert_eq!(ingress.pending(), 1);

    // The stored candidate keeps its full provenance and its index, and
    // the refused duplicate consumed no arrival index.
    let drained = ingress.drain();
    assert_eq!(drained.len(), 1);
    assert_eq!(drained[0].arrival_index, 0);
    assert_eq!(drained[0].envelope, candidate);
    let next_gate = gate_for(companion(0x32));
    let next = envelope_for(companion(0x32), &next_gate);
    assert_eq!(
        ingress
            .admit(&next_gate, tick, next)
            .unwrap()
            .arrival_index(),
        1
    );
}

/// A candidate from a cancelled task generation, one with a non-finite
/// yaw, and one with an out-of-world target are each refused whole, and
/// the sessionless envelope cannot name a human session by type.
#[test]
fn cancelled_generation_nan_invalid_target() {
    assert_sessionless::<CompanionActionEnvelope>();

    let mut ingress = CompanionIngress::try_new().unwrap();
    let tick = 60;
    let gate = gate_for(companion(0x41));

    // Cancelled generation: the active task has moved past this candidate.
    let mut cancelled = envelope_for(companion(0x41), &gate);
    cancelled.generation = gate.generation() - 1;
    expect_invalid(
        ingress.admit(&gate, tick, cancelled).unwrap_err(),
        "companion_generation",
    );
    assert_eq!(ingress.pending(), 0);

    // The whole payload is revalidated on admission even though the
    // envelope constructor already rejects this value.
    let mut nan = envelope_for(companion(0x41), &gate);
    nan.action = CompanionAction::Move {
        move_x: 1,
        move_z: 0,
        jump: false,
        yaw: f32::NAN,
    };
    expect_invalid(ingress.admit(&gate, tick, nan).unwrap_err(), "yaw");
    assert_eq!(ingress.pending(), 0);

    // Targets outside the world vertical bound are structurally invalid
    // before any queue reservation could matter.
    let mut above = envelope_for(companion(0x41), &gate);
    above.action = CompanionAction::MineHold {
        target: BlockPos::new(0, 320, 0),
    };
    expect_invalid(ingress.admit(&gate, tick, above).unwrap_err(), "target");
    let mut below = envelope_for(companion(0x41), &gate);
    below.action = CompanionAction::Place {
        target: BlockPos::new(0, -65, 0),
        block: 1,
    };
    expect_invalid(ingress.admit(&gate, tick, below).unwrap_err(), "target");
    assert_eq!(ingress.pending(), 0);

    // No refusal consumed an arrival index.
    let valid = envelope_for(companion(0x41), &gate);
    assert_eq!(
        ingress.admit(&gate, tick, valid).unwrap().arrival_index(),
        0
    );
}
