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
    use std::sync::Arc;

    use mornlea_client_core::contracts::{ClientIdentity, ClientWorkBudget};
    use mornlea_client_core::preparation::{
        PreparationJob, PreparationPayload, PreparedResourceKey, SectionKey, TerrainKey,
    };
    use mornlea_domain::{ChunkPos, Dimension};
    use mornlea_engine::native::contracts::mesh::{MeshModel, MeshRegistry, MeshRegistryEntry};
    use mornlea_protocol::{LoginStart, MIN_Y, PlayerId};
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

    // Delayed preparation completion: an admitted job has no result until
    // the harness advances it, and the FIFO result carries its identity.
    let entries = [MeshRegistryEntry {
        id: 1,
        opaque: true,
        emission: 0,
        material: [1; 6],
        fluid_height: 0,
        light_attenuation: 0,
        block_top_raw: 0,
        model: MeshModel::Default,
    }];
    let registry = Arc::new(MeshRegistry::try_new(&entries, &[0], 0, 1).expect("checked registry"));
    let job = PreparationJob::try_new(
        PreparedResourceKey::try_new(
            epoch,
            Dimension::OVERWORLD,
            TerrainKey::Section(SectionKey::try_new(ChunkPos::new(0, 0), 0).expect("section")),
            1,
            1,
            1,
        )
        .expect("checked key"),
        PreparationPayload::Near(
            mornlea_client_core::preparation::OwnedMeshView::try_new(
                Box::new([0u16; 110592]),
                [false; 9],
                Box::new([[0i16; 256]; 9]),
                MIN_Y,
                registry,
            )
            .expect("checked mesh view"),
        ),
    )
    .expect("paired job");
    let ticket = harness.submit_preparation(job).expect("admitted");
    assert_eq!(harness.pending_preparations(), 1, "admitted, not completed");
    assert_eq!(harness.completed_preparations(), 0, "nothing advanced yet");
    assert!(
        harness.poll_preparation().is_none(),
        "delayed: no result before advance"
    );
    assert_eq!(
        harness.advance_preparations(1),
        1,
        "the scripted advance completes it"
    );
    assert_eq!(harness.pending_preparations(), 0);
    let result = harness
        .poll_preparation()
        .expect("FIFO result after advance");
    assert_eq!(result.ticket(), ticket);
    assert!(result.outcome().is_ok(), "the completed job succeeds");
    assert!(
        harness.poll_preparation().is_none(),
        "the queue drains exactly"
    );
}
