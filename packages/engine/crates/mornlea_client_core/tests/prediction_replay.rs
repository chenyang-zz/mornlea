//! The registered prediction replay target. The input and correction
//! provider test modules are reserved by the contract landing; the root
//! carries the registered harness smoke case that proves the deterministic
//! replay harness operations run on the checked constructors.

#![allow(dead_code)]

#[path = "support/mod.rs"]
mod support;

#[path = "prediction_replay/correction.rs"]
mod correction;
#[path = "prediction_replay/input.rs"]
mod input;

#[test]
fn replay_harness_roundtrip() {
    use mornlea_client_core::contracts::{ClientIdentity, ClientWorkBudget};
    use mornlea_protocol::{LoginStart, PlayerId};
    use support::{DoubleMode, ReplayHarness};

    let mut harness = ReplayHarness::new(DoubleMode::Contract).expect("harness");
    let identity = ClientIdentity::try_new(
        LoginStart::new(
            PlayerId::try_from_bytes([0, 0, 0, 0, 0, 0, 0x40, 0, 0x80, 0, 0, 0, 0, 0, 0, 1])
                .expect("uuid"),
            "harness",
            8,
        )
        .expect("login"),
    )
    .expect("identity");
    let epoch = harness.connect(identity).expect("pending epoch");
    assert_eq!(
        epoch.get(),
        1,
        "the first pending epoch is nonzero and first"
    );
    let report = harness
        .step(epoch, ClientWorkBudget::try_new(1, 1).expect("budget"))
        .expect("step publishes the pending frame");
    assert_eq!(
        report.confirmed_revision().get(),
        0,
        "pending revision zero"
    );
    assert_eq!(report.frame_index(), 1, "frame index advanced once");
}
