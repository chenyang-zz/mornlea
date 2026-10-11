//! The registered lifecycle contract target. The lifecycle provider test
//! module is reserved by the contract landing; the root carries the
//! registered transport release case that pins the checked ticket contract.

#![allow(dead_code)]

#[path = "support/mod.rs"]
mod support;

#[path = "lifecycle_contract/lifecycle.rs"]
mod lifecycle;

#[test]
fn transport_ticket_rejected_after_close() {
    use mornlea_client_core::contracts::{ClientIdentity, TransportPoll, TransportTicket};
    use mornlea_protocol::{LoginStart, PlayerId};
    use std::num::NonZeroU64;
    use support::{DoubleMode, ReplayHarness};

    let mut harness = ReplayHarness::new(DoubleMode::Contract).expect("harness");
    let identity = ClientIdentity::try_new(
        LoginStart::new(
            PlayerId::try_from_bytes([0, 0, 0, 0, 0, 0, 0x40, 0, 0x80, 0, 0, 0, 0, 0, 0, 2])
                .expect("uuid"),
            "lifecycle",
            8,
        )
        .expect("login"),
    )
    .expect("identity");
    let epoch = harness.connect(identity).expect("pending epoch");
    harness.double_close_checked(epoch).expect("close");
    assert_eq!(harness.connector.releases(), 1, "one transport release");
    // The old ticket cannot re-enter: a stale ticket is rejected by the
    // connector before any dereference.
    let stale = TransportTicket::try_new(
        NonZeroU64::new(1).expect("one"),
        NonZeroU64::new(1).expect("one"),
    )
    .expect("ticket shape");
    assert!(
        matches!(harness.connector_poll(stale), TransportPoll::Closed(_)),
        "old tickets are rejected after close"
    );
}
