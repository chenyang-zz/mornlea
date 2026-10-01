//! The registered contract double cases.
//!
//! The named cases pin the checked constructors, the real frame validator,
//! the real whole-batch input admission, the local view-validity overlay and
//! the private source-order envelopes through the deterministic consumer
//! double. `MODE` is the double behavior under test: the behavioral reds are
//! captured by running this module with the deliberately wrong double
//! (`DoubleMode::Broken`), which publishes mixed-revision frames without
//! validation and advances the sequence on rejected batches; the real
//! validator and the contract double reject that behavior.

use std::sync::Arc;

use crate::support::{DoubleMode, ReplayHarness};
use mornlea_client_core::ClientIdentity;
use mornlea_client_core::contracts::{
    ClientError, ClientLimits, ClientWorkBudget, ConfirmedRevision, FAMILY_SESSION, FamilyKey,
    RecordHeader, SessionEpoch, SessionPhase,
};
use mornlea_client_core::input::{
    ClientIntent, ClientIntentKind, ContainerOperation, ContainerToken, CraftingViewToken,
    InputAction, InputBatch,
};
use mornlea_client_core::presentation::frame::ActorRecord;
use mornlea_client_core::presentation::frame::{
    FamilyFrame, FamilyRecords, PresentationFrame, SessionRecord,
};
use mornlea_client_core::presentation::{
    ActorDetail, ActorDimension, ActorId, ActorKind, AudioDedupDelta, AudioDedupKey, BoundedText,
    CueId, FinitePositive, FiniteRay, FiniteUnit, OrderedRecord, Pose, ProjectionOrder,
    StableRecordKey, TaskView, TextKind, UiOutcome, WorldUiRecord, WorldUiView,
};
use mornlea_domain::{
    ChunkPos, CommandText, CompanionId, CompanionName, CompanionSpeaker, ContainerKind,
    ContainerRef, CraftingMove, CraftingSize, HotbarSlot, InventoryMove, LookAngles, PartialMove,
    PlacementIntent, PlayerControl, PlayerControlParts, PlayerId, ResyncIntent, StackSource,
    StackView,
};
use mornlea_protocol::{
    CONTAINER_KIND_CHEST, ChestState, ContainerRef as WireContainerRef, ForgetChunks, ItemStack,
    LoginStart, ServerPacket,
};

/// The double behavior under test. Contract mode is the accepted behavior;
/// the behavioral reds were captured with `DoubleMode::Broken`.
const MODE: DoubleMode = DoubleMode::Contract;

fn identity(name: &str, byte: u8) -> ClientIdentity {
    let login = LoginStart::new(
        PlayerId::try_from_bytes([0, 0, 0, 0, 0, 0, 0x40, 0, 0x80, 0, 0, 0, 0, 0, 0, byte])
            .expect("uuid v4"),
        name,
        8,
    )
    .expect("login");
    ClientIdentity::try_new(login).expect("identity")
}

fn look(yaw: f32, pitch: f32) -> LookAngles {
    LookAngles::try_new(yaw, pitch).expect("finite look")
}

fn control() -> PlayerControl {
    PlayerControl::new(PlayerControlParts {
        movement: mornlea_domain::Movement {
            move_x: 1,
            move_z: 0,
            jump: false,
        },
        look: look(0.0, 0.0),
        actions: mornlea_domain::HeldActions {
            primary: false,
            eating: false,
            sprinting: false,
            sneaking: false,
        },
    })
}

fn action(intent: ClientIntent) -> InputAction {
    InputAction {
        intent,
        container: None,
        crafting: None,
    }
}

fn chat_action(text: &str) -> InputAction {
    action(ClientIntent::Chat(mornlea_domain::ChatIntent::new(
        CommandText::try_from_canonical(text.to_string()).expect("text"),
    )))
}

fn batch(epoch: SessionEpoch, actions: Vec<InputAction>) -> InputBatch {
    InputBatch::try_new(epoch, actions).expect("batch shape")
}

fn connected() -> (ReplayHarness, SessionEpoch) {
    let mut harness = ReplayHarness::new(MODE).expect("harness");
    let epoch = harness
        .connect(identity("double", 3))
        .expect("pending epoch");
    harness.double.admit().expect("admitted");
    (harness, epoch)
}

fn step_once(harness: &mut ReplayHarness, epoch: SessionEpoch) {
    harness
        .step(epoch, ClientWorkBudget::try_new(4, 0).expect("budget"))
        .expect("step publishes");
}

/// `contract_double::pending_and_first_observation`: the pending epoch is
/// nonzero, the revision is zero before any server observation, the first
/// complete observation moves the revision to exactly one, and no world state
/// is invented along the way.
#[test]
fn pending_and_first_observation() {
    let mut harness = ReplayHarness::new(MODE).expect("harness");
    let epoch = harness
        .connect(identity("pending", 4))
        .expect("pending epoch");
    assert_eq!(epoch.get(), 1, "the pending epoch is nonzero");

    // The pending frame: revision zero, only session and lifecycle records.
    let pending = harness.snapshot(epoch).expect("pending frame");
    assert_eq!(pending.confirmed_revision().get(), 0);
    assert_eq!(pending.session_epoch(), epoch);
    for family in pending.families() {
        assert!(
            family.records().record_count() == 0
                || matches!(
                    family.records(),
                    FamilyRecords::Session(_) | FamilyRecords::Lifecycle(_)
                ),
            "only session/lifecycle records exist at revision zero"
        );
    }

    // One complete observation: revision one, and no invented world state.
    harness
        .double
        .stage_observation(
            epoch,
            ServerPacket::ForgetChunks(
                ForgetChunks::new(mornlea_domain::Dimension::OVERWORLD, vec![(5, 5)])
                    .expect("forget"),
            ),
        )
        .expect("staged");
    let report = harness
        .step(epoch, ClientWorkBudget::try_new(1, 0).expect("budget"))
        .expect("step commits");
    assert_eq!(
        report.confirmed_revision().get(),
        1,
        "first complete observation"
    );
    let mirror = harness.double.mirror().expect("mirror");
    assert_eq!(mirror.revision().get(), 1);
    assert_eq!(
        mirror.world().chunk_count(),
        0,
        "a forget invents no world state"
    );
    assert_eq!(
        harness.double.pending_observations(),
        0,
        "nothing is retained uncharged"
    );
}

/// `contract_double::whole_input_failure_atomic`: an invalid middle action,
/// the 129th action, a stale token and a full prediction journal each leave
/// the sequence, queue, journal and mirror unchanged.
#[test]
fn whole_input_failure_atomic() {
    let (mut harness, epoch) = connected();

    // A healthy baseline batch commits one sequence.
    harness
        .submit_input(
            epoch,
            batch(epoch, vec![action(ClientIntent::PlayerInput(control()))]),
        )
        .expect("baseline batch");
    let sequence = harness.double.sequence_witness();
    let records = harness
        .double
        .admission()
        .expect("admission")
        .outbound_records();
    let journal = harness
        .double
        .admission()
        .expect("admission")
        .journal_entries();
    let mirror_revision = harness.double.mirror().expect("mirror").revision();

    // Invalid middle action: the whole batch rejects.
    let missing_token = InputAction {
        intent: ClientIntent::CloseContainer,
        container: None,
        crafting: None,
    };
    let error = harness
        .submit_input(
            epoch,
            batch(
                epoch,
                vec![
                    action(ClientIntent::PlayerInput(control())),
                    missing_token,
                    action(ClientIntent::SelectHotbar(
                        HotbarSlot::new(1).expect("slot"),
                    )),
                ],
            ),
        )
        .expect_err("invalid middle action rejects the whole batch");
    assert_eq!(error, ClientError::InvalidInput);
    assert_eq!(
        harness.double.sequence_witness(),
        sequence,
        "sequence unchanged"
    );
    assert_eq!(
        harness
            .double
            .admission()
            .expect("admission")
            .outbound_records(),
        records,
        "queue unchanged"
    );
    assert_eq!(
        harness
            .double
            .admission()
            .expect("admission")
            .journal_entries(),
        journal,
        "journal unchanged"
    );
    assert_eq!(
        harness.double.mirror().expect("mirror").revision(),
        mirror_revision,
        "mirror unchanged"
    );

    // The 129th action rejects before any scan.
    let oversized_batch = InputBatch::try_new(
        epoch,
        vec![action(ClientIntent::PlayerInput(control())); 129],
    );
    assert_eq!(oversized_batch, Err(ClientError::Capacity));
    assert_eq!(harness.double.sequence_witness(), sequence);

    // A stale token rejects without mutating anything.
    let reference =
        ContainerRef::try_new(ChunkPos::new(0, 0), ContainerKind::Chest, 0, 1).expect("reference");
    let stale =
        ContainerToken::try_new(epoch, reference, ConfirmedRevision::new(9)).expect("token shape");
    let error = harness
        .submit_input(
            epoch,
            batch(
                epoch,
                vec![InputAction {
                    intent: ClientIntent::MoveContainer(
                        mornlea_domain::ContainerMove::try_new(
                            ChunkPos::new(0, 0),
                            ContainerKind::Chest,
                            0,
                            1,
                            0,
                            1,
                        )
                        .expect("move"),
                    ),
                    container: Some(stale),
                    crafting: None,
                }],
            ),
        )
        .expect_err("stale token rejects");
    assert_eq!(error, ClientError::StaleEpoch);
    assert_eq!(harness.double.sequence_witness(), sequence);
    assert_eq!(
        harness
            .double
            .admission()
            .expect("admission")
            .outbound_records(),
        records
    );

    // A full prediction journal rejects before outbound admission.
    let tight = ClientLimits::try_new_with(
        128,
        8192,
        8 << 20,
        4104,
        8 << 20,
        1,
        4096,
        4096,
        4096,
        64 << 20,
        4096,
        8 << 20,
    )
    .expect("one journal slot");
    let mut tight_harness = ReplayHarness::with_limits(MODE, tight).expect("harness");
    let tight_epoch = tight_harness
        .connect(identity("journal", 5))
        .expect("pending epoch");
    tight_harness.double.admit().expect("admitted");
    tight_harness
        .submit_input(
            tight_epoch,
            batch(
                tight_epoch,
                vec![action(ClientIntent::PlayerInput(control()))],
            ),
        )
        .expect("first batch fills the journal");
    let tight_sequence = tight_harness.double.sequence_witness();
    let tight_records = tight_harness
        .double
        .admission()
        .expect("admission")
        .outbound_records();
    let error = tight_harness
        .submit_input(
            tight_epoch,
            batch(
                tight_epoch,
                vec![action(ClientIntent::PlayerInput(control()))],
            ),
        )
        .expect_err("full journal rejects");
    assert_eq!(error, ClientError::Capacity);
    assert_eq!(tight_harness.double.sequence_witness(), tight_sequence);
    assert_eq!(
        tight_harness
            .double
            .admission()
            .expect("admission")
            .outbound_records(),
        tight_records,
        "queue unchanged on journal capacity"
    );
}

/// `contract_double::mixed_input_chat`: an empty semantic batch is a noop,
/// chat-only carries no first sequence, and input/chat/input commits exactly
/// two contiguous sequences and one chat.
#[test]
fn mixed_input_chat() {
    let (mut harness, epoch) = connected();

    let receipt = harness
        .submit_input(epoch, batch(epoch, vec![]))
        .expect("empty batch");
    assert_eq!(receipt, mornlea_client_core::contracts::InputReceipt::Noop);
    assert_eq!(harness.double.sequence_witness(), 1, "no sequence reserved");

    let receipt = harness
        .submit_input(epoch, batch(epoch, vec![chat_action("hello world")]))
        .expect("chat batch");
    let mornlea_client_core::contracts::InputReceipt::Queued {
        first_sequence,
        sequenced_count,
        chat_count,
        ..
    } = receipt
    else {
        panic!("queued receipt");
    };
    assert_eq!(first_sequence, None, "chat-only has no first sequence");
    assert_eq!(sequenced_count, 0);
    assert_eq!(chat_count, 1);
    assert_eq!(
        harness.double.sequence_witness(),
        1,
        "chat takes no sequence"
    );

    let receipt = harness
        .submit_input(
            epoch,
            batch(
                epoch,
                vec![
                    action(ClientIntent::PlayerInput(control())),
                    chat_action("second"),
                    action(ClientIntent::SelectHotbar(
                        HotbarSlot::new(0).expect("slot"),
                    )),
                ],
            ),
        )
        .expect("mixed batch");
    let mornlea_client_core::contracts::InputReceipt::Queued {
        first_sequence,
        sequenced_count,
        chat_count,
        ..
    } = receipt
    else {
        panic!("queued receipt");
    };
    let first = first_sequence.expect("sequenced actions exist");
    assert_eq!(sequenced_count, 2);
    assert_eq!(chat_count, 1);
    assert_eq!(
        harness.double.sequence_witness(),
        first + 2,
        "exactly two contiguous sequences"
    );
}

/// `contract_double::frame_failure_atomic`: a mixed epoch or revision and a
/// byte cap plus one each preserve the old visible Arc and frame index, and
/// a tickless `ForgetChunks` carries no source tick and orders a remove.
#[test]
fn frame_failure_atomic() {
    let (mut harness, epoch) = connected();
    step_once(&mut harness, epoch);
    let old = harness.snapshot(epoch).expect("visible frame");
    let old_index = harness.double.frame_index();

    // Cap plus one: the configured frame cap is one byte under the next
    // candidate, so the publication fails and nothing changes.
    harness
        .double
        .stage_observation(
            epoch,
            ServerPacket::ForgetChunks(
                ForgetChunks::new(mornlea_domain::Dimension::OVERWORLD, vec![(7, 7)])
                    .expect("forget"),
            ),
        )
        .expect("staged");
    let hint = harness.double.candidate_size_hint();
    harness.double.set_frame_cap_for_test(hint - 1);
    let error = harness
        .step(epoch, ClientWorkBudget::try_new(4, 0).expect("budget"))
        .expect_err("cap plus one rejects");
    assert_eq!(error, ClientError::Capacity);
    let after = harness.snapshot(epoch).expect("old frame retained");
    assert!(Arc::ptr_eq(&old, &after), "the old Arc is preserved");
    assert_eq!(
        harness.double.frame_index(),
        old_index,
        "frame index preserved"
    );

    // Mixed revision: a family header that disagrees with the frame parent
    // rejects. The broken double publishes exactly such a frame unchecked;
    // the real validator rejects it.
    let mixed = broken_mixed_frame(epoch);
    assert_eq!(
        mixed.validate(harness.double.limits()),
        Err(ClientError::InvalidInput),
        "the real validator rejects the mixed frame"
    );

    // Mixed epoch: a header naming a foreign epoch rejects.
    let foreign = SessionEpoch::try_new(epoch.get() + 1).expect("foreign epoch");
    let header = RecordHeader::try_new(
        foreign,
        ConfirmedRevision::new(1),
        None,
        mornlea_client_core::contracts::FamilyOperation::Upsert,
    )
    .expect("header");
    let record =
        SessionRecord::try_new(header, SessionPhase::Admitted, None, None).expect("record");
    let frame = PresentationFrame::try_new(
        epoch,
        ConfirmedRevision::new(1),
        1,
        vec![
            FamilyFrame::try_new(
                FamilyKey::try_new(FAMILY_SESSION).expect("key"),
                FamilyRecords::Session(vec![record]),
            )
            .expect("family"),
        ],
    )
    .expect("candidate");
    assert_eq!(
        frame.validate(harness.double.limits()),
        Err(ClientError::InvalidInput)
    );

    // Tickless ForgetChunks: no source tick, ordered remove.
    let normal = ClientLimits::try_new().expect("frozen limits");
    let mut fresh = ReplayHarness::with_limits(MODE, normal).expect("harness");
    let fresh_epoch = fresh
        .connect(identity("tickless", 6))
        .expect("pending epoch");
    fresh.double.admit().expect("admitted");
    fresh
        .double
        .stage_observation(
            fresh_epoch,
            ServerPacket::ForgetChunks(
                ForgetChunks::new(mornlea_domain::Dimension::OVERWORLD, vec![(1, 2)])
                    .expect("forget"),
            ),
        )
        .expect("staged");
    step_once(&mut fresh, fresh_epoch);
    assert_eq!(
        fresh.double.pending_removals().len(),
        0,
        "the removal is consumed"
    );
    let visible = fresh.snapshot(fresh_epoch).expect("published frame");
    let terrain = visible
        .families()
        .iter()
        .find(|family| family.key().logical_name == "terrain")
        .expect("terrain family published");
    let FamilyRecords::Terrain(records) = terrain.records() else {
        panic!("terrain records");
    };
    assert_eq!(records.len(), 1);
    assert_eq!(
        records[0].header().source_tick(),
        None,
        "ForgetChunks is tickless"
    );
    assert_eq!(
        records[0].header().operation(),
        mornlea_client_core::contracts::FamilyOperation::Remove,
        "the forget orders a remove"
    );
}

fn broken_mixed_frame(epoch: SessionEpoch) -> PresentationFrame {
    let header = RecordHeader::try_new(
        epoch,
        ConfirmedRevision::new(2),
        None,
        mornlea_client_core::contracts::FamilyOperation::Upsert,
    )
    .expect("header");
    let record =
        SessionRecord::try_new(header, SessionPhase::Admitted, None, None).expect("record");
    PresentationFrame::try_new(
        epoch,
        ConfirmedRevision::new(1),
        1,
        vec![
            FamilyFrame::try_new(
                FamilyKey::try_new(FAMILY_SESSION).expect("key"),
                FamilyRecords::Session(vec![record]),
            )
            .expect("family"),
        ],
    )
    .expect("candidate")
}

/// `contract_double::ordered_projection_and_commit`: interleaved equal and
/// tickless observations retain their private source order across topic
/// vectors, and a failed publication consumes no event, removal or dedup
/// proposal.
#[test]
fn ordered_projection_and_commit() {
    let (mut harness, epoch) = connected();
    let revision = ConfirmedRevision::new(1);

    // Three observations with equal source ticks and one tickless: the
    // private order keys carry the actual ordinals.
    let mut orders = Vec::new();
    for (ordinal, tick) in [(0u32, Some(7u64)), (1, Some(7)), (2, None)] {
        let key = mornlea_client_core::contracts::ObservationKey::try_new(epoch, revision, ordinal)
            .expect("key");
        orders.push((key, tick));
    }

    // Two topic vectors interleaved by source order: remote player from
    // observation 0, hostile from observation 1, remote player from the
    // tickless observation 2.
    let stable_a = StableRecordKey::Actor {
        kind: ActorKind::RemotePlayer,
        dimension: ActorDimension::Known(mornlea_domain::Dimension::OVERWORLD),
        id: ActorId::RemotePlayer(
            PlayerId::try_from_bytes([0, 0, 0, 0, 0, 0, 0x40, 0, 0x80, 0, 0, 0, 0, 0, 0, 10])
                .expect("uuid"),
        ),
    };
    let stable_b = StableRecordKey::Actor {
        kind: ActorKind::Hostile,
        dimension: ActorDimension::Known(mornlea_domain::Dimension::OVERWORLD),
        id: ActorId::Hostile(mornlea_domain::HostileId::try_new(11).expect("id")),
    };
    let stable_c = StableRecordKey::Actor {
        kind: ActorKind::RemotePlayer,
        dimension: ActorDimension::Known(mornlea_domain::Dimension::OVERWORLD),
        id: ActorId::RemotePlayer(
            PlayerId::try_from_bytes([0, 0, 0, 0, 0, 0, 0x40, 0, 0x80, 0, 0, 0, 0, 0, 0, 11])
                .expect("uuid"),
        ),
    };
    let topic_one = vec![
        OrderedRecord::try_new(
            ProjectionOrder::Confirmed {
                observation: orders[0].0,
                record_ordinal: 0,
            },
            stable_a,
            0u8,
        )
        .expect("envelope"),
        OrderedRecord::try_new(
            ProjectionOrder::Confirmed {
                observation: orders[2].0,
                record_ordinal: 0,
            },
            stable_c,
            2u8,
        )
        .expect("envelope"),
    ];
    let topic_two = vec![
        OrderedRecord::try_new(
            ProjectionOrder::Confirmed {
                observation: orders[1].0,
                record_ordinal: 0,
            },
            stable_b,
            1u8,
        )
        .expect("envelope"),
    ];
    let merged = mornlea_client_core::presentation::merge_ordered(vec![topic_one, topic_two], 4096)
        .expect("merge");
    let payloads: Vec<u8> = merged.iter().map(|entry| *entry.record()).collect();
    assert_eq!(
        payloads,
        vec![0, 1, 2],
        "private source order across topics"
    );

    // A failed publication consumes nothing: the pending observation, the
    // pending removal and the proposed dedup keys all stay retry-owned.
    harness
        .double
        .stage_observation(
            epoch,
            ServerPacket::ForgetChunks(
                ForgetChunks::new(mornlea_domain::Dimension::OVERWORLD, vec![(3, 3)])
                    .expect("forget"),
            ),
        )
        .expect("staged");
    let pending_before = harness.double.pending_observations();
    assert_eq!(pending_before, 1);

    let key = AudioDedupKey::Confirmed {
        epoch,
        observation: orders[0].0,
        authoritative_event_id: None,
        cue: CueId::try_new(5).expect("cue"),
    };
    let delta = AudioDedupDelta::try_new(vec![key], Vec::new()).expect("delta");
    harness.double.stage_dedup_proposal(delta);
    assert_eq!(harness.double.committed_dedup().len(), 0);
    assert_eq!(
        harness.double.pending_dedup_proposal().insertions().len(),
        1
    );

    // Drive a real failed publication through the double: the configured
    // frame cap is one byte under the next candidate, so `step` returns
    // `Err(Capacity)` and every owner stays unchanged.
    let hint = harness.double.candidate_size_hint();
    harness.double.set_frame_cap_for_test(hint - 1);
    let error = harness
        .step(epoch, ClientWorkBudget::try_new(4, 0).expect("budget"))
        .expect_err("failed publication");
    assert_eq!(error, ClientError::Capacity);
    assert_eq!(
        harness.double.pending_observations(),
        pending_before,
        "event retained"
    );
    assert_eq!(
        harness.double.pending_removals().len(),
        0,
        "removal retained: the proposal stays inside the retained observation"
    );
    assert_eq!(
        harness.double.committed_dedup().len(),
        0,
        "dedup not committed"
    );
    assert_eq!(
        harness.double.pending_dedup_proposal().insertions().len(),
        1,
        "the dedup proposal stays retry-owned"
    );

    // A retry publishes once and then commits the dedup proposal exactly once.
    harness
        .double
        .set_frame_cap_for_test(ClientLimits::MAX_FRAME_BYTES);
    let report = harness
        .step(epoch, ClientWorkBudget::try_new(4, 0).expect("budget"))
        .expect("retry publishes");
    assert_eq!(report.confirmed_revision().get(), 1);
    assert_eq!(
        harness.double.pending_observations(),
        0,
        "the event is consumed once"
    );
    assert_eq!(
        harness.double.pending_removals().len(),
        0,
        "the removal is consumed once"
    );
    assert_eq!(
        harness.double.committed_dedup(),
        &[key],
        "the dedup proposal commits exactly once"
    );
    assert_eq!(
        harness.double.pending_dedup_proposal().insertions().len(),
        0,
        "the proposal is consumed with the publication"
    );
}

/// `contract_double::local_close_overlay`: a close followed by a later
/// external move rejects the whole batch; a committed close blocks a later
/// batch until the confirmed close/reopen; and the mirror contents remain
/// authoritative and unchanged.
#[test]
fn local_close_overlay() {
    let (mut harness, epoch) = connected();

    // A confirmed chest view exists at revision one.
    let wire_reference = WireContainerRef {
        dimension: 0,
        chunk_x: 2,
        chunk_z: 3,
        kind: CONTAINER_KIND_CHEST,
        slot: 1,
        generation: 4,
    };
    let mut items: [ItemStack; 27] =
        core::array::from_fn(|_| ItemStack::try_new(0, 0, 0).expect("empty"));
    items[0] = ItemStack::try_new(1, 3, 0).expect("stack");
    let chest = ChestState::new(wire_reference, items).expect("chest state");
    harness
        .double
        .stage_observation(epoch, ServerPacket::ChestState(chest))
        .expect("staged");
    step_once(&mut harness, epoch);
    let reference = ContainerRef::try_new(ChunkPos::new(2, 3), ContainerKind::Chest, 1, 4)
        .expect("domain reference");
    let revision = harness
        .double
        .mirror()
        .expect("mirror")
        .container_revision(&reference)
        .expect("confirmed view");
    assert_eq!(revision.get(), 1);
    let token = ContainerToken::try_new(epoch, reference, revision).expect("token");

    // Close + later external move in one batch: the whole batch rejects and
    // the overlay is not mutated.
    let move_action = || InputAction {
        intent: ClientIntent::MoveContainer(
            mornlea_domain::ContainerMove::try_new(
                ChunkPos::new(2, 3),
                ContainerKind::Chest,
                1,
                4,
                0,
                2,
            )
            .expect("move"),
        ),
        container: Some(token),
        crafting: None,
    };
    let close_action = InputAction {
        intent: ClientIntent::CloseContainer,
        container: Some(token),
        crafting: None,
    };
    let error = harness
        .submit_input(epoch, batch(epoch, vec![close_action, move_action()]))
        .expect_err("close then external move rejects the whole batch");
    assert_eq!(error, ClientError::InvalidState);
    assert!(
        !harness
            .double
            .admission()
            .expect("admission")
            .overlay()
            .is_closed(&reference),
        "the overlay is unchanged by the rejected batch"
    );
    assert_eq!(
        harness.double.sequence_witness(),
        1,
        "no sequence was consumed"
    );

    // The close alone commits and tombstones the view locally; the mirror
    // contents stay authoritative and unchanged.
    harness
        .submit_input(
            epoch,
            batch(
                epoch,
                vec![InputAction {
                    intent: ClientIntent::CloseContainer,
                    container: Some(token),
                    crafting: None,
                }],
            ),
        )
        .expect("close commits");
    assert!(
        harness
            .double
            .admission()
            .expect("admission")
            .overlay()
            .is_closed(&reference),
        "the committed close tombstones the view"
    );
    assert_eq!(
        harness
            .double
            .mirror()
            .expect("mirror")
            .container_revision(&reference),
        Some(revision),
        "the mirror still owns the confirmed view"
    );

    // A later batch addressing the locally closed view is blocked.
    let error = harness
        .submit_input(epoch, batch(epoch, vec![move_action()]))
        .expect_err("locally closed view blocks the move");
    assert_eq!(error, ClientError::InvalidState);
    assert_eq!(
        harness
            .double
            .mirror()
            .expect("mirror")
            .container_revision(&reference),
        Some(revision),
        "the rejected batch did not touch the mirror"
    );

    // The confirmed close removes the view; a fresh reopened view supplies
    // fresh attribution.
    let wire_closed = mornlea_protocol::ContainerClosed::new(wire_reference).expect("closed");
    harness
        .double
        .stage_observation(epoch, ServerPacket::ContainerClosed(wire_closed))
        .expect("staged");
    step_once(&mut harness, epoch);
    let reopened_revision = ConfirmedRevision::new(harness.double.revision());
    let inventory = harness
        .double
        .mirror()
        .expect("mirror")
        .inventory()
        .clone()
        .with_container_view(reference, reopened_revision);
    harness.double.stage_inventory_for_test(inventory);
    let fresh_token = ContainerToken::try_new(epoch, reference, reopened_revision).expect("token");
    harness
        .submit_input(
            epoch,
            batch(
                epoch,
                vec![InputAction {
                    intent: ClientIntent::MoveContainer(
                        mornlea_domain::ContainerMove::try_new(
                            ChunkPos::new(2, 3),
                            ContainerKind::Chest,
                            1,
                            4,
                            0,
                            1,
                        )
                        .expect("move"),
                    ),
                    container: Some(fresh_token),
                    crafting: None,
                }],
            ),
        )
        .expect("the fresh view permits its fresh token");
}

/// `contract_double::exact_source_mapping`: every action and region, the
/// hostile yaw-only rule, absent associations, the real task states, the raw
/// drop dimension, and the queued receipt that is not a confirmation.
#[test]
fn exact_source_mapping() {
    let (mut harness, epoch) = connected();

    // Every action maps to exactly one F1 client packet and round-trips.
    let reference =
        ContainerRef::try_new(ChunkPos::new(0, 0), ContainerKind::Chest, 0, 1).expect("reference");
    let expected_packets: [(ClientIntentKind, u32); 20] = [
        (ClientIntentKind::PlayerInput, 0),
        (ClientIntentKind::PlaceBlock, 2),
        (ClientIntentKind::Resync, 3),
        (ClientIntentKind::SelectHotbar, 5),
        (ClientIntentKind::OpenContainer, 8),
        (ClientIntentKind::TillSoil, 13),
        (ClientIntentKind::BoneMeal, 14),
        (ClientIntentKind::CollectWater, 16),
        (ClientIntentKind::PlaceWater, 17),
        (ClientIntentKind::MoveInventory, 6),
        (ClientIntentKind::MoveCrafting, 7),
        (ClientIntentKind::MoveContainer, 9),
        (ClientIntentKind::CloseContainer, 10),
        (ClientIntentKind::DropSelectedItem, 11),
        (ClientIntentKind::TakeCraftingOutput, 15),
        (ClientIntentKind::EquipArmor, 18),
        (ClientIntentKind::MovePartial, 19),
        (ClientIntentKind::QuickMove, 20),
        (ClientIntentKind::DropStack, 21),
        (ClientIntentKind::Chat, 12),
    ];
    let intents = all_intents();
    for (intent, (kind, packet_id)) in intents.iter().zip(expected_packets.iter()) {
        assert_eq!(
            intent.kind(),
            *kind,
            "kind order matches the contract order"
        );
        let play = intent.to_play_intent(1);
        let packet = mornlea_protocol::ClientPacket::try_from(play).expect("wire admits");
        assert_eq!(packet.key().id, *packet_id, "the F1 registry id matches");
        let round = ClientIntent::from_play_intent(&intent.to_play_intent(1));
        match (&round, intent) {
            (Some(back), original) => assert_eq!(back, original, "the mapping round-trips"),
            (None, _) => panic!("play intent lost"),
        }
    }

    // Every container region: inventory-only intents need no token and reject
    // an irrelevant one; crafting and container regions need their own token.
    let inventory_only = action(ClientIntent::MoveInventory(
        InventoryMove::try_new(0, 1).expect("move"),
    ));
    assert!(
        harness
            .submit_input(epoch, batch(epoch, vec![inventory_only]))
            .is_ok(),
        "inventory-only needs no token"
    );
    let with_irrelevant = InputAction {
        intent: ClientIntent::MoveInventory(InventoryMove::try_new(0, 1).expect("move")),
        container: Some(
            ContainerToken::try_new(epoch, reference, ConfirmedRevision::new(0)).expect("token"),
        ),
        crafting: None,
    };
    assert_eq!(
        harness
            .submit_input(epoch, batch(epoch, vec![with_irrelevant]))
            .expect_err("an irrelevant token rejects"),
        ClientError::InvalidInput
    );
    let crafting_missing = action(ClientIntent::MoveCrafting(
        CraftingMove::try_new(0, 9).expect("crafting move"),
    ));
    assert_eq!(
        harness
            .submit_input(epoch, batch(epoch, vec![crafting_missing]))
            .expect_err("a crafting move requires the crafting token"),
        ClientError::InvalidInput
    );
    // A confirmed crafting view exists at the current revision.
    let crafting_state = mornlea_protocol::CraftingState::new(
        mornlea_protocol::CRAFTING_GRID_SIZE_PERSONAL,
        [ItemStack::try_new(0, 0, 0).expect("empty"); 9],
        ItemStack::try_new(0, 0, 0).expect("empty"),
    )
    .expect("crafting state");
    harness
        .double
        .stage_observation(epoch, ServerPacket::CraftingState(crafting_state))
        .expect("staged");
    step_once(&mut harness, epoch);
    let crafting_revision = harness
        .double
        .mirror()
        .expect("mirror")
        .crafting_revision(CraftingSize::Personal)
        .expect("confirmed crafting view");
    let crafting_token =
        CraftingViewToken::try_new(epoch, crafting_revision, CraftingSize::Personal)
            .expect("token");
    let crafting_ok = InputAction {
        intent: ClientIntent::MoveCrafting(CraftingMove::try_new(0, 9).expect("crafting move")),
        container: None,
        crafting: Some(crafting_token),
    };
    harness
        .submit_input(epoch, batch(epoch, vec![crafting_ok]))
        .expect("a crafting move with its token admits");
    let container_partial = action(ClientIntent::MovePartial(
        PartialMove::try_new(StackView::Inventory, 0, 1, true).expect("partial"),
    ));
    harness
        .submit_input(epoch, batch(epoch, vec![container_partial]))
        .expect("an inventory-region partial move needs no token");
    let container_region_missing = action(ClientIntent::QuickMove(
        StackSource::try_new(StackView::Container(reference), 0).expect("source"),
    ));
    assert_eq!(
        harness
            .submit_input(epoch, batch(epoch, vec![container_region_missing]))
            .expect_err("a container-region quick move requires the container token"),
        ClientError::InvalidInput
    );

    // The classification is not a second wire command.
    assert!(matches!(
        ContainerOperation::classify(&ClientIntent::CloseContainer),
        Some(ContainerOperation::Close)
    ));
    assert!(ContainerOperation::classify(&ClientIntent::OpenContainer(look(0.0, 0.0))).is_none());

    // Hostile yaw-only: the spawn carries yaw and no pitch, so the projected
    // actor record keeps yaw and leaves pitch absent, with no velocity.
    let hostile_id = mornlea_domain::HostileId::try_new(77).expect("id");
    let record = ActorRecord::try_new(
        RecordHeader::try_new(
            epoch,
            ConfirmedRevision::new(1),
            Some(9),
            mornlea_client_core::contracts::FamilyOperation::Upsert,
        )
        .expect("header"),
        ActorKind::Hostile,
        ActorId::Hostile(hostile_id),
        ActorDimension::Known(mornlea_domain::Dimension::OVERWORLD),
        Some([1.0, 65.0, 2.0]),
        Some(1.5),
        None,
        None,
        Some(ActorDetail::Hostile {
            archetype: 2,
            health: 20,
        }),
    )
    .expect("hostile record");
    assert_eq!(record.yaw(), Some(1.5));
    assert_eq!(record.pitch(), None, "hostiles are yaw-only");
    assert_eq!(
        record.velocity(),
        None,
        "a hostile spawn invents no velocity"
    );

    // Absent associations: a CombatHit observation projects no actor record
    // and the hostile detail carries no attacker association by construction.
    let hit = mornlea_domain::CombatHit::try_new(9, 5, mornlea_domain::CombatTarget::Hostile)
        .expect("hit");
    assert_eq!(hit.target(), mornlea_domain::CombatTarget::Hostile);
    assert!(
        !matches!(record.detail(), Some(ActorDetail::Passive { .. })),
        "no lure or drop marker is invented for a hostile"
    );

    // Real task states: Started through Failed map exactly, with no invented
    // id, generation or percentage.
    let companion = CompanionSpeaker::new(
        CompanionId::try_from_bytes([0, 0, 0, 0, 0, 0, 0x40, 0, 0x80, 0, 0, 0, 0, 0, 0, 20])
            .expect("uuid"),
        CompanionName::try_from_canonical("spec".to_string()).expect("name"),
    );
    let command = CommandText::try_from_canonical("mine stone".to_string()).expect("command");
    let observation = mornlea_client_core::contracts::ObservationKey::try_new(
        epoch,
        ConfirmedRevision::new(1),
        0,
    )
    .expect("key");
    let states = [
        mornlea_domain::TaskState::Started,
        mornlea_domain::TaskState::Progress,
        mornlea_domain::TaskState::Completed,
        mornlea_domain::TaskState::TimedOut,
        mornlea_domain::TaskState::Stopped,
        mornlea_domain::TaskState::Failed(mornlea_domain::TaskFailure::InventoryFull),
    ];
    for state in states {
        let view = TaskView::try_new(observation, companion.clone(), command.clone(), state)
            .expect("task view");
        assert_eq!(*view.state(), state, "the real task state is preserved");
    }

    // Raw drop dimension: the open raw dimension is preserved, never coerced.
    let drop = mornlea_domain::DropId::try_new(190, ChunkPos::new(-3, 4), 5, 2).expect("drop id");
    let drop_record = ActorRecord::try_new(
        RecordHeader::try_new(
            epoch,
            ConfirmedRevision::new(1),
            Some(9),
            mornlea_client_core::contracts::FamilyOperation::Upsert,
        )
        .expect("header"),
        ActorKind::Drop,
        ActorId::Drop(drop),
        ActorDimension::DropRaw(190),
        Some([1.0, 2.0, 3.0]),
        None,
        None,
        None,
        Some(ActorDetail::Drop {
            block_index: 42,
            stack: mornlea_domain::ItemStack::try_new(1, 3, 0).expect("stack"),
        }),
    )
    .expect("drop record");
    assert_eq!(
        *drop_record.dimension(),
        ActorDimension::DropRaw(190),
        "the raw drop dimension is preserved"
    );

    // A queued receipt is not a confirmation: the double's input record keeps
    // the queued state and chat is never labeled confirmed.
    harness
        .submit_input(epoch, batch(epoch, vec![chat_action("receipt check")]))
        .expect("chat queued");
    assert!(
        harness
            .double
            .admission()
            .expect("admission")
            .outbound_records()
            > 0,
        "the chat record is admitted"
    );
    let queued = mornlea_client_core::presentation::InputReceiptState::Queued;
    assert!(
        queued
            != mornlea_client_core::presentation::InputReceiptState::Confirmed {
                server_tick: None
            }
    );

    // Rejections stay sequence-bound observations of their own.
    let rejection = UiOutcome::Rejected {
        sequence: 1,
        reason: mornlea_domain::RejectReason::Occupied,
    };
    let accepted = UiOutcome::PlacementAccepted { sequence: 2 };
    assert_ne!(
        rejection, accepted,
        "rejection and placement remain distinct"
    );
}

fn all_intents() -> Vec<ClientIntent> {
    vec![
        ClientIntent::PlayerInput(control()),
        ClientIntent::PlaceBlock(PlacementIntent::try_new(look(0.0, 0.0), 0).expect("placement")),
        ClientIntent::Resync(ResyncIntent::try_new(0, ChunkPos::new(0, 0), 0).expect("resync")),
        ClientIntent::SelectHotbar(HotbarSlot::new(0).expect("slot")),
        ClientIntent::OpenContainer(look(0.0, 0.0)),
        ClientIntent::TillSoil(look(0.0, 0.0)),
        ClientIntent::BoneMeal(look(0.0, 0.0)),
        ClientIntent::CollectWater(look(0.0, 0.0)),
        ClientIntent::PlaceWater(look(0.0, 0.0)),
        ClientIntent::MoveInventory(InventoryMove::try_new(0, 1).expect("move")),
        ClientIntent::MoveCrafting(CraftingMove::try_new(0, 9).expect("crafting move")),
        ClientIntent::MoveContainer(
            mornlea_domain::ContainerMove::try_new(
                ChunkPos::new(0, 0),
                ContainerKind::Chest,
                0,
                1,
                0,
                1,
            )
            .expect("move"),
        ),
        ClientIntent::CloseContainer,
        ClientIntent::DropSelectedItem,
        ClientIntent::TakeCraftingOutput,
        ClientIntent::EquipArmor,
        ClientIntent::MovePartial(
            PartialMove::try_new(StackView::Inventory, 0, 1, true).expect("partial"),
        ),
        ClientIntent::QuickMove(StackSource::try_new(StackView::Inventory, 0).expect("source")),
        ClientIntent::DropStack(StackSource::try_new(StackView::Inventory, 0).expect("source")),
        ClientIntent::Chat(mornlea_domain::ChatIntent::new(
            CommandText::try_from_canonical("mapping".to_string()).expect("text"),
        )),
    ]
}

/// Constructor and validator bounds: measured limits, every numeric and text
/// bound plus one, family-key major/minor compatibility and the
/// 4,325,408-byte world aggregate under the accepted frame cap.
#[test]
fn limits_bounds_and_layout_compat() {
    // The measured frozen limits equal the inventory values.
    let limits = ClientLimits::try_new().expect("frozen limits");
    assert_eq!(limits.queued_input_events(), 128);
    assert_eq!(
        limits.inbound_observations(),
        8192,
        "the measured revision of 4104"
    );
    assert_eq!(limits.inbound_bytes(), 8 << 20);
    assert_eq!(limits.outbound_commands(), 4104);
    assert_eq!(limits.outbound_bytes(), 8 << 20);
    assert_eq!(limits.prediction_journal(), 256);
    assert_eq!(limits.message_work(), 4096);
    assert_eq!(limits.mesh_work(), 4096);
    assert_eq!(limits.preparation_results(), 4096);
    assert_eq!(limits.preparation_bytes(), 64 << 20);
    assert_eq!(limits.family_records(), 4096);
    assert_eq!(
        limits.frame_bytes(),
        8 << 20,
        "4 MiB is invalid for the aggregate"
    );

    // Injected over-limit values and zeros reject before any state change.
    assert_eq!(
        ClientLimits::try_new_with(
            129,
            8192,
            8 << 20,
            4104,
            8 << 20,
            256,
            4096,
            4096,
            4096,
            64 << 20,
            4096,
            8 << 20,
        ),
        Err(ClientError::InvalidInput)
    );
    assert_eq!(
        ClientLimits::try_new_with(
            128,
            8193,
            8 << 20,
            4104,
            8 << 20,
            256,
            4096,
            4096,
            4096,
            64 << 20,
            4096,
            8 << 20,
        ),
        Err(ClientError::InvalidInput)
    );
    assert_eq!(
        ClientLimits::try_new_with(
            128,
            8192,
            8 << 20,
            4105,
            8 << 20,
            256,
            4096,
            4096,
            4096,
            64 << 20,
            4096,
            8 << 20,
        ),
        Err(ClientError::InvalidInput)
    );
    assert_eq!(
        ClientLimits::try_new_with(
            128,
            8192,
            8 << 20,
            4104,
            8 << 20,
            257,
            4096,
            4096,
            4096,
            64 << 20,
            4096,
            8 << 20,
        ),
        Err(ClientError::InvalidInput)
    );
    assert_eq!(
        ClientLimits::try_new_with(
            128,
            8192,
            8 << 20,
            4104,
            8 << 20,
            256,
            4097,
            4096,
            4096,
            64 << 20,
            4096,
            8 << 20,
        ),
        Err(ClientError::InvalidInput)
    );
    assert_eq!(
        ClientLimits::try_new_with(
            128,
            8192,
            8 << 20,
            4104,
            8 << 20,
            256,
            4096,
            4097,
            4096,
            64 << 20,
            4096,
            8 << 20,
        ),
        Err(ClientError::InvalidInput)
    );
    assert_eq!(
        ClientLimits::try_new_with(
            128,
            8192,
            8 << 20,
            4104,
            8 << 20,
            256,
            4096,
            4096,
            4097,
            64 << 20,
            4096,
            8 << 20,
        ),
        Err(ClientError::InvalidInput)
    );
    assert_eq!(
        ClientLimits::try_new_with(
            128,
            8192,
            8 << 20,
            4104,
            8 << 20,
            256,
            4096,
            4096,
            4096,
            64 << 20 + 1,
            4096,
            8 << 20,
        ),
        Err(ClientError::InvalidInput)
    );
    assert_eq!(
        ClientLimits::try_new_with(
            128,
            8192,
            8 << 20,
            4104,
            8 << 20,
            256,
            4096,
            4096,
            4096,
            64 << 20,
            4097,
            8 << 20,
        ),
        Err(ClientError::InvalidInput)
    );
    assert_eq!(
        ClientLimits::try_new_with(
            128,
            8192,
            8 << 20,
            4104,
            8 << 20,
            256,
            4096,
            4096,
            4096,
            64 << 20,
            4096,
            8 << 20 + 1,
        ),
        Err(ClientError::InvalidInput)
    );
    assert_eq!(
        ClientLimits::try_new_with(
            0,
            8192,
            8 << 20,
            4104,
            8 << 20,
            256,
            4096,
            4096,
            4096,
            64 << 20,
            4096,
            8 << 20,
        ),
        Err(ClientError::InvalidInput)
    );

    // The work budget allows zero and caps each field at 4096.
    assert!(ClientWorkBudget::try_new(0, 0).is_ok());
    assert_eq!(
        ClientWorkBudget::try_new(4097, 0),
        Err(ClientError::Capacity)
    );
    assert_eq!(
        ClientWorkBudget::try_new(0, 4097),
        Err(ClientError::Capacity)
    );

    // Text bounds plus one, and the control-set rules.
    let name_33 = BoundedText::try_new("a".repeat(33), TextKind::Name);
    assert_eq!(name_33, Err(ClientError::InvalidInput));
    let name_bytes = BoundedText::try_new("é".repeat(65), TextKind::Name);
    assert_eq!(
        name_bytes,
        Err(ClientError::InvalidInput),
        "129 bytes over 128"
    );
    let command = BoundedText::try_new("c".repeat(1024), TextKind::Command);
    assert!(command.is_ok());
    assert_eq!(
        BoundedText::try_new("c".repeat(1025), TextKind::Command),
        Err(ClientError::InvalidInput)
    );
    assert!(BoundedText::try_new("s".repeat(256), TextKind::Speech).is_ok());
    assert_eq!(
        BoundedText::try_new("s".repeat(257), TextKind::Speech),
        Err(ClientError::InvalidInput)
    );
    assert!(
        BoundedText::try_new(String::new(), TextKind::Target).is_ok(),
        "an empty target clears"
    );
    assert_eq!(
        BoundedText::try_new("t".repeat(65), TextKind::Target),
        Err(ClientError::InvalidInput)
    );
    assert!(BoundedText::try_new(String::new(), TextKind::Control).is_ok());
    assert!(BoundedText::try_new("  \u{0007}".to_string(), TextKind::Control).is_ok());
    assert_eq!(
        BoundedText::try_new("x".repeat(257), TextKind::Control),
        Err(ClientError::InvalidInput)
    );

    // Numeric supporting-value bounds plus one.
    assert!(
        CueId::try_new(6).is_ok(),
        "the registered pilot cue ids are 0..=6"
    );
    assert_eq!(CueId::try_new(7), Err(ClientError::InvalidInput));
    assert!(FiniteUnit::try_new(1.0).is_ok());
    assert_eq!(FiniteUnit::try_new(1.1), Err(ClientError::InvalidInput));
    assert!(FinitePositive::try_new(0.25).is_ok());
    assert_eq!(FinitePositive::try_new(0.0), Err(ClientError::InvalidInput));
    assert!(Pose::try_new([0.0, 64.0, 0.0], 0.0, 0.0).is_ok());
    assert_eq!(
        Pose::try_new([0.0, f64::NAN, 0.0], 0.0, 0.0),
        Err(ClientError::InvalidInput)
    );
    assert!(
        FiniteRay::try_new([0.0; 3], look(0.0, 0.0), 6.0).is_ok(),
        "the accepted interaction distance"
    );
    assert_eq!(
        FiniteRay::try_new([0.0; 3], look(0.0, 0.0), 6.1),
        Err(ClientError::InvalidInput)
    );
    assert_eq!(
        FiniteRay::try_new([0.0; 3], look(0.0, 0.0), 0.0),
        Err(ClientError::InvalidInput)
    );

    // Family-key major/minor compatibility: 1.0 only, known names only.
    assert!(FamilyKey::try_new(FAMILY_SESSION).is_ok());
    assert_eq!(FamilyKey::try_new(FAMILY_SESSION).expect("key").major, 1);
    assert_eq!(FamilyKey::try_new(FAMILY_SESSION).expect("key").minor, 0);
    assert_eq!(
        FamilyKey::try_new("unknown"),
        Err(ClientError::InvalidInput)
    );

    // The 4,325,408-byte pilot world aggregate is a valid frame below the
    // accepted 8 MiB cap; one byte over the accepted cap rejects.
    let epoch = SessionEpoch::try_new(1).expect("epoch");
    let aggregate = task_frame(epoch, 4096, Some(4_325_408)).expect("aggregate frame");
    assert_eq!(
        aggregate.validated_size().expect("size"),
        4_325_408,
        "the exact measured aggregate"
    );
    assert!(
        aggregate.validate(&limits).is_ok(),
        "below the accepted cap"
    );
    let aggregate_limits = ClientLimits::try_new_with(
        128,
        8192,
        8 << 20,
        4104,
        8 << 20,
        256,
        4096,
        4096,
        4096,
        64 << 20,
        4096,
        4_325_408,
    )
    .expect("aggregate-sized cap");
    assert!(aggregate.validate(&aggregate_limits).is_ok());
    let over = task_frame(epoch, 4096, Some(4_325_409)).expect("one byte over");
    assert_eq!(
        over.validate(&aggregate_limits),
        Err(ClientError::Capacity),
        "cap plus one rejects"
    );

    // A family count plus one rejects before publication.
    let epoch = SessionEpoch::try_new(2).expect("epoch");
    let over_count = task_frame(epoch, 4097, None).expect("over-count frame");
    assert_eq!(
        over_count.validate(&limits),
        Err(ClientError::Capacity),
        "family records plus one rejects"
    );
}

/// Builds a world-UI task frame with `count` records whose exact owned size
/// is `target` bytes, using only valid records.
fn task_frame(
    epoch: SessionEpoch,
    count: usize,
    target: Option<usize>,
) -> Result<PresentationFrame, ClientError> {
    let revision = ConfirmedRevision::new(1);
    let companion = CompanionSpeaker::new(
        CompanionId::try_from_bytes([0, 0, 0, 0, 0, 0, 0x40, 0, 0x80, 0, 0, 0, 0, 0, 0, 30])
            .expect("uuid"),
        CompanionName::try_from_canonical("spec".to_string()).expect("name"),
    );
    let observation =
        mornlea_client_core::contracts::ObservationKey::try_new(epoch, revision, 0).expect("key");

    // Start from a uniform text length and distribute the deficit over the
    // records, so the total is exact without hand-computed constants.
    let mut lengths = vec![512usize; count];
    let frame_of = |lengths: &[usize]| -> Result<PresentationFrame, ClientError> {
        let header = RecordHeader::try_new(
            epoch,
            revision,
            Some(1),
            mornlea_client_core::contracts::FamilyOperation::Upsert,
        )?;
        let mut records = Vec::with_capacity(count);
        for length in lengths.iter() {
            let command = CommandText::try_from_canonical("a".repeat(*length))
                .map_err(|_| ClientError::InvalidInput)?;
            let view = TaskView::try_new(
                observation,
                companion.clone(),
                command,
                mornlea_domain::TaskState::Progress,
            )
            .map_err(|_| ClientError::InvalidInput)?;
            records.push(WorldUiRecord::try_new(header, WorldUiView::Task(view))?);
        }
        PresentationFrame::try_new(
            epoch,
            revision,
            1,
            vec![FamilyFrame::try_new(
                FamilyKey::try_new(mornlea_client_core::contracts::FAMILY_WORLD_UI)?,
                FamilyRecords::WorldUi(records),
            )?],
        )
    };
    let Some(target) = target else {
        return frame_of(&lengths);
    };
    let mut frame = frame_of(&lengths)?;
    let mut size = frame.validated_size()?;
    if size > target {
        return Err(ClientError::InvalidInput);
    }
    let mut deficit = target - size;
    // Each record can absorb up to 1024 - 512 = 512 more bytes.
    for length in lengths.iter_mut() {
        let take = deficit.min(1024 - *length);
        *length += take;
        deficit -= take;
        if deficit == 0 {
            break;
        }
    }
    if deficit != 0 {
        return Err(ClientError::InvalidInput);
    }
    frame = frame_of(&lengths)?;
    size = frame.validated_size()?;
    assert_eq!(size, target, "the aggregate construction is exact");
    Ok(frame)
}

/// The behavioral red artifact: the deliberately wrong double's mixed frame
/// is rejected by the real validator, and its non-atomic admission is visible
/// in the sequence witness.
#[test]
fn wrong_double_rejected() {
    let (mut harness, epoch) = connected();

    // Non-atomic admission: the broken double advances the sequence witness
    // on a rejected batch, the contract double never does.
    let mut broken = ReplayHarness::new(DoubleMode::Broken).expect("harness");
    let broken_epoch = broken
        .connect(identity("broken", 7))
        .expect("pending epoch");
    broken.double.admit().expect("admitted");
    let missing_close = InputAction {
        intent: ClientIntent::CloseContainer,
        container: None,
        crafting: None,
    };
    broken
        .submit_input(broken_epoch, batch(broken_epoch, vec![missing_close]))
        .expect_err("the broken double rejects the same batch");
    assert_eq!(
        broken.double.sequence_witness(),
        2,
        "the broken double consumed a sequence anyway"
    );

    // The contract double's rejected batch leaves the witness unchanged.
    let missing_token = InputAction {
        intent: ClientIntent::CloseContainer,
        container: None,
        crafting: None,
    };
    let error = harness
        .submit_input(epoch, batch(epoch, vec![missing_token]))
        .expect_err("rejected");
    assert_eq!(error, ClientError::InvalidInput);
    assert_eq!(
        harness.double.sequence_witness(),
        1,
        "the contract double consumed nothing"
    );

    // The broken double's mixed frame is rejected by the real validator even
    // though the broken double publishes it unchecked.
    let mixed = broken_mixed_frame(epoch);
    assert!(matches!(
        mixed.validate(&ClientLimits::try_new().expect("limits")),
        Err(ClientError::InvalidInput)
    ));
}
