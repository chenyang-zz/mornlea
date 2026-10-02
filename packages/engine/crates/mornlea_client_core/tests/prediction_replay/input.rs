//! The semantic typed input cases: the twenty closed actions, whole-batch
//! token rules, the local view-validity overlay and the single-owner atomic
//! admission commit.
//!
//! Every row drives the frozen two-phase surface directly — the read-only
//! `InputTranslator::validate_batch` over a confirmed mirror, then the
//! two-argument `InputTranslator::commit(ValidatedInputBatch, &mut
//! InputAdmissionState)` — so the assertions pin the exact receipt, the
//! overlay behavior and the unchanged authoritative mirror on both the
//! accepted and every rejected row. The wrong-behavior artifact at the bottom
//! keeps the contract double's deliberately non-atomic mode executable.

use crate::support::{DoubleMode, ReplayHarness};
use mornlea_client_core::contracts::{
    ClientError, ClientLimits, ConfirmedRevision, InputReceipt, SessionEpoch, SessionPhase,
};
use mornlea_client_core::input::{
    ClientIntent, ClientIntentKind, ContainerToken, CraftingViewToken, InputAction,
    InputAdmissionState, InputBatch, InputTranslator, LocalCueSource,
};
use mornlea_client_core::session::{ConfirmedMirror, ConfirmedMirrorParts, InventoryConfirmed};
use mornlea_domain::{
    ChatIntent, ChunkPos, CommandText, ContainerKind, ContainerMove, ContainerRef, CraftingMove,
    CraftingSize, HeldActions, InventoryMove, LookAngles, Movement, PartialMove, PlacementIntent,
    PlayerControl, PlayerControlParts, ResyncIntent, StackSource, StackView,
};

/// A finite look fixture every ray-style intent shares; the constructor is
/// the finiteness gate under test.
fn look(yaw: f32, pitch: f32) -> LookAngles {
    LookAngles::try_new(yaw, pitch).expect("finite fixture look")
}

/// The shared fixture look pair.
fn fixture_look() -> LookAngles {
    look(0.5, -0.25)
}

/// One neutral finite player-control payload with the given raw move axes.
fn control(move_x: i8, move_z: i8) -> PlayerControl {
    PlayerControl::new(PlayerControlParts {
        movement: Movement {
            move_x,
            move_z,
            jump: false,
        },
        look: fixture_look(),
        actions: HeldActions {
            primary: false,
            eating: false,
            sprinting: false,
            sneaking: false,
        },
    })
}

/// One canonical chat fixture of exactly `len` ASCII bytes.
fn chat_text(len: usize) -> CommandText {
    CommandText::try_from_canonical("a".repeat(len)).expect("canonical command text")
}

fn epoch(value: u64) -> SessionEpoch {
    SessionEpoch::try_new(value).expect("nonzero epoch")
}

fn limits() -> ClientLimits {
    ClientLimits::try_new().expect("frozen limits")
}

/// The frozen limits with only the named bounds tightened, so every bound
/// admits N and rejects N + 1 without driving a whole measured run to its
/// natural ceiling.
fn tightened(
    queued_input_events: usize,
    outbound_commands: usize,
    outbound_bytes: usize,
    prediction_journal: usize,
) -> ClientLimits {
    let frozen = limits();
    ClientLimits::try_new_with(
        queued_input_events,
        frozen.inbound_observations(),
        frozen.inbound_bytes(),
        outbound_commands,
        outbound_bytes,
        prediction_journal,
        frozen.message_work(),
        frozen.mesh_work(),
        frozen.preparation_results(),
        frozen.preparation_bytes(),
        frozen.family_records(),
        frozen.frame_bytes(),
    )
    .expect("legal tightened limits")
}

/// One admitted confirmed mirror with the staged inventory views the token
/// cases address, at the given revision.
fn admitted_mirror(
    epoch: SessionEpoch,
    revision: u64,
    inventory: InventoryConfirmed,
) -> ConfirmedMirror {
    ConfirmedMirror::try_new(ConfirmedMirrorParts {
        epoch,
        revision: ConfirmedRevision::new(revision),
        phase: SessionPhase::Admitted,
        world: None,
        actors: None,
        inventory: Some(inventory),
        world_ui: None,
    })
    .expect("checked admitted mirror")
}

fn empty_inventory() -> InventoryConfirmed {
    InventoryConfirmed::try_new().expect("empty confirmed inventory")
}

/// One chest reference: chunk (3, 4), slot 0, the given generation.
fn chest(generation: u32) -> ContainerRef {
    ContainerRef::try_new(ChunkPos::new(3, 4), ContainerKind::Chest, 0, generation)
        .expect("chest reference")
}

/// One furnace reference: chunk (5, -6), slot 1, the given generation.
fn furnace(generation: u32) -> ContainerRef {
    ContainerRef::try_new(ChunkPos::new(5, -6), ContainerKind::Furnace, 1, generation)
        .expect("furnace reference")
}

fn container_token(epoch: SessionEpoch, reference: ContainerRef, revision: u64) -> ContainerToken {
    ContainerToken::try_new(epoch, reference, ConfirmedRevision::new(revision))
        .expect("container token")
}

fn crafting_token(epoch: SessionEpoch, revision: u64, size: CraftingSize) -> CraftingViewToken {
    CraftingViewToken::try_new(epoch, ConfirmedRevision::new(revision), size)
        .expect("crafting token")
}

/// The confirmed inventory holding the chest, the furnace and both crafting
/// views, each attributed to its own revision.
fn full_inventory() -> InventoryConfirmed {
    empty_inventory()
        .with_container_view(chest(1), ConfirmedRevision::new(11))
        .with_container_view(furnace(1), ConfirmedRevision::new(12))
        .with_crafting_view(CraftingSize::Personal, ConfirmedRevision::new(13))
        .with_crafting_view(CraftingSize::Workbench, ConfirmedRevision::new(14))
}

/// One admitted mirror holding every fixture view.
fn full_mirror(epoch_value: u64) -> ConfirmedMirror {
    admitted_mirror(epoch(epoch_value), 20, full_inventory())
}

/// One fresh admission owner for the epoch.
fn owner(epoch_value: u64) -> InputAdmissionState {
    InputAdmissionState::try_new(epoch(epoch_value), limits()).expect("admission owner")
}

/// A plain action helper: an intent with both token fields `None`.
fn plain(intent: ClientIntent) -> InputAction {
    InputAction {
        intent,
        container: None,
        crafting: None,
    }
}

/// A chest-addressed action with its current token.
fn chest_action(intent: ClientIntent, epoch_value: u64, revision: u64) -> InputAction {
    InputAction {
        intent,
        container: Some(container_token(epoch(epoch_value), chest(1), revision)),
        crafting: None,
    }
}

/// The whole-batch submit under test: read-only validation over the mirror,
/// then the frozen two-argument commit.
fn submit(
    actions: Vec<InputAction>,
    mirror: &ConfirmedMirror,
    admission: &mut InputAdmissionState,
) -> Result<InputReceipt, ClientError> {
    let batch = InputBatch::try_new(mirror.epoch(), actions)?;
    let validated = InputTranslator::validate_batch(&batch, mirror, admission.limits())?;
    InputTranslator::commit(validated, admission)
}

/// Asserts every admission owner is exactly at its fresh-epoch state.
fn assert_untouched(admission: &InputAdmissionState) {
    assert_eq!(admission.next_sequence(), 1, "no sequence consumed");
    assert_eq!(admission.outbound_records(), 0, "no partial send");
    assert_eq!(admission.outbound_bytes(), 0, "no queued bytes");
    assert_eq!(admission.journal_entries(), 0, "no journal entry");
    assert!(
        admission.overlay().tombstones().is_empty(),
        "no overlay tombstone"
    );
    assert!(
        admission.pending_local_cues().is_empty(),
        "no local cue attribution"
    );
    assert!(
        admission.projection().pending_local_cues().is_empty(),
        "no projection-side cue attribution"
    );
}

/// One move inside the fixed player inventory.
fn inventory_move(from: u8, to: u8) -> ClientIntent {
    ClientIntent::MoveInventory(InventoryMove::try_new(from, to).expect("inventory move"))
}

/// One whole-stack move between the crafting grid and the backpack.
fn crafting_move(from: u8, to: u8) -> ClientIntent {
    ClientIntent::MoveCrafting(CraftingMove::try_new(from, to).expect("crafting move"))
}

/// One whole-stack move inside a container's unified view.
fn container_move(reference: ContainerRef, from: u8, to: u8) -> ClientIntent {
    ClientIntent::MoveContainer(
        ContainerMove::try_new(
            reference.chunk(),
            reference.kind(),
            reference.slot(),
            reference.generation(),
            from,
            to,
        )
        .expect("container move"),
    )
}

/// One partial move inside one unified view.
fn partial(view: StackView, from: u8, to: u8) -> ClientIntent {
    ClientIntent::MovePartial(PartialMove::try_new(view, from, to, false).expect("partial move"))
}

/// One whole-stack source-addressed command.
fn source(view: StackView, slot: u8, quick: bool) -> ClientIntent {
    let source = StackSource::try_new(view, slot).expect("stack source");
    if quick {
        ClientIntent::QuickMove(source)
    } else {
        ClientIntent::DropStack(source)
    }
}

/// All twenty variants in one ordered batch: every container-region action
/// precedes the close that locally dismisses the chest view, so the ordered
/// close validity admits the whole batch atomically. The exact contract order
/// is pinned by `f1_mapping_round_trips_every_variant`.
fn all_twenty(epoch_value: u64) -> Vec<InputAction> {
    let epoch = epoch(epoch_value);
    vec![
        plain(ClientIntent::PlayerInput(control(1, -1))),
        plain(ClientIntent::PlaceBlock(
            PlacementIntent::try_new(fixture_look(), 3).expect("placement"),
        )),
        plain(ClientIntent::Resync(
            ResyncIntent::try_new(0, ChunkPos::new(0, 0), 0).expect("resync"),
        )),
        plain(ClientIntent::SelectHotbar(
            mornlea_domain::HotbarSlot::new(8).expect("hotbar slot"),
        )),
        plain(ClientIntent::OpenContainer(fixture_look())),
        plain(ClientIntent::TillSoil(fixture_look())),
        plain(ClientIntent::BoneMeal(fixture_look())),
        plain(ClientIntent::CollectWater(fixture_look())),
        plain(ClientIntent::PlaceWater(fixture_look())),
        plain(inventory_move(0, 35)),
        InputAction {
            intent: crafting_move(0, 40),
            container: None,
            crafting: Some(crafting_token(epoch, 13, CraftingSize::Personal)),
        },
        InputAction {
            intent: container_move(chest(1), 36, 37),
            container: Some(container_token(epoch, chest(1), 11)),
            crafting: None,
        },
        InputAction {
            intent: partial(StackView::Container(chest(1)), 36, 37),
            container: Some(container_token(epoch, chest(1), 11)),
            crafting: None,
        },
        InputAction {
            intent: source(StackView::Crafting, 2, true),
            container: None,
            crafting: Some(crafting_token(epoch, 13, CraftingSize::Personal)),
        },
        plain(source(StackView::Inventory, 35, false)),
        plain(ClientIntent::DropSelectedItem),
        InputAction {
            intent: ClientIntent::TakeCraftingOutput,
            container: None,
            crafting: Some(crafting_token(epoch, 14, CraftingSize::Workbench)),
        },
        plain(ClientIntent::EquipArmor),
        chest_action(ClientIntent::CloseContainer, epoch_value, 11),
        plain(ClientIntent::Chat(ChatIntent::new(chat_text(8)))),
    ]
}

/// Every variant in the exact `ClientIntent` contract order, for
/// registry-drift checks. The list is not a batch: a whole-batch close must
/// come after the container-region actions it dismisses.
fn contract_intents() -> Vec<ClientIntent> {
    vec![
        ClientIntent::PlayerInput(control(1, -1)),
        ClientIntent::PlaceBlock(PlacementIntent::try_new(fixture_look(), 3).expect("placement")),
        ClientIntent::Resync(ResyncIntent::try_new(0, ChunkPos::new(0, 0), 0).expect("resync")),
        ClientIntent::SelectHotbar(mornlea_domain::HotbarSlot::new(8).expect("hotbar slot")),
        ClientIntent::OpenContainer(fixture_look()),
        ClientIntent::TillSoil(fixture_look()),
        ClientIntent::BoneMeal(fixture_look()),
        ClientIntent::CollectWater(fixture_look()),
        ClientIntent::PlaceWater(fixture_look()),
        inventory_move(0, 35),
        crafting_move(0, 40),
        container_move(chest(1), 36, 37),
        ClientIntent::CloseContainer,
        ClientIntent::DropSelectedItem,
        ClientIntent::TakeCraftingOutput,
        ClientIntent::EquipArmor,
        partial(StackView::Container(chest(1)), 36, 37),
        source(StackView::Crafting, 2, true),
        source(StackView::Inventory, 35, false),
        ClientIntent::Chat(ChatIntent::new(chat_text(8))),
    ]
}

#[test]
fn all_twenty_variants_admit_atomically_with_exact_receipt() {
    let mirror = full_mirror(1);
    let before = mirror.clone();
    let mut admission = owner(1);
    let receipt = submit(all_twenty(1), &mirror, &mut admission).expect("whole batch admits");
    assert_eq!(
        receipt,
        InputReceipt::Queued {
            epoch: epoch(1),
            first_sequence: Some(1),
            sequenced_count: 19,
            chat_count: 1,
        },
        "the exact local admission receipt"
    );
    assert_eq!(admission.next_sequence(), 20, "one sequence per command");
    assert_eq!(admission.outbound_records(), 20, "one record per action");
    assert_eq!(admission.journal_entries(), 19, "chat is never journaled");
    assert_eq!(
        admission.overlay().tombstones(),
        &[(chest(1), ConfirmedRevision::new(11))],
        "the committed close tombstones the chest attribution"
    );
    assert_eq!(
        admission.pending_local_cues(),
        &[LocalCueSource {
            local_event_sequence: 1,
            kind: ClientIntentKind::CollectWater,
        }],
        "the accepted bucket action emits exactly one local cue source"
    );
    assert_eq!(mirror, before, "the authoritative mirror is untouched");
    assert_eq!(
        mirror.container_revision(&chest(1)),
        Some(ConfirmedRevision::new(11)),
        "a local close never edits confirmed attribution"
    );
}

#[test]
fn f1_mapping_round_trips_every_variant() {
    let expected = [
        ClientIntentKind::PlayerInput,
        ClientIntentKind::PlaceBlock,
        ClientIntentKind::Resync,
        ClientIntentKind::SelectHotbar,
        ClientIntentKind::OpenContainer,
        ClientIntentKind::TillSoil,
        ClientIntentKind::BoneMeal,
        ClientIntentKind::CollectWater,
        ClientIntentKind::PlaceWater,
        ClientIntentKind::MoveInventory,
        ClientIntentKind::MoveCrafting,
        ClientIntentKind::MoveContainer,
        ClientIntentKind::CloseContainer,
        ClientIntentKind::DropSelectedItem,
        ClientIntentKind::TakeCraftingOutput,
        ClientIntentKind::EquipArmor,
        ClientIntentKind::MovePartial,
        ClientIntentKind::QuickMove,
        ClientIntentKind::DropStack,
        ClientIntentKind::Chat,
    ];
    for (index, intent) in contract_intents().into_iter().enumerate() {
        assert_eq!(intent.kind(), expected[index], "kind registry order");
        let played = intent.to_play_intent(7);
        assert_eq!(
            ClientIntent::from_play_intent(&played),
            Some(intent),
            "the F1 mapping round-trips variant {index}"
        );
    }
    assert_eq!(
        ClientIntent::from_play_intent(&mornlea_protocol::PlayIntent::KeepAliveReply { token: 0 }),
        None,
        "the keep-alive reply is session control, never a batch action"
    );
    let mut scratch = Vec::new();
    for intent in contract_intents() {
        let sequence = if matches!(intent, ClientIntent::Chat(_)) {
            0
        } else {
            1
        };
        assert!(
            intent.encode_frame(sequence, &mut scratch).is_ok(),
            "every variant encodes as one complete F1 frame"
        );
    }
}

#[test]
fn every_container_and_crafting_region_takes_exactly_its_token() {
    let cases: Vec<(&str, InputAction, bool, bool)> = vec![
        (
            "chest move",
            InputAction {
                intent: container_move(chest(1), 36, 40),
                container: Some(container_token(epoch(1), chest(1), 11)),
                crafting: None,
            },
            true,
            false,
        ),
        (
            "furnace move",
            InputAction {
                intent: container_move(furnace(1), 0, 36),
                container: Some(container_token(epoch(1), furnace(1), 12)),
                crafting: None,
            },
            true,
            false,
        ),
        (
            "personal crafting move",
            InputAction {
                intent: crafting_move(0, 40),
                container: None,
                crafting: Some(crafting_token(epoch(1), 13, CraftingSize::Personal)),
            },
            false,
            true,
        ),
        (
            "workbench crafting move",
            InputAction {
                intent: crafting_move(8, 44),
                container: None,
                crafting: Some(crafting_token(epoch(1), 14, CraftingSize::Workbench)),
            },
            false,
            true,
        ),
        (
            "personal take-output",
            InputAction {
                intent: ClientIntent::TakeCraftingOutput,
                container: None,
                crafting: Some(crafting_token(epoch(1), 13, CraftingSize::Personal)),
            },
            false,
            true,
        ),
        (
            "workbench take-output",
            InputAction {
                intent: ClientIntent::TakeCraftingOutput,
                container: None,
                crafting: Some(crafting_token(epoch(1), 14, CraftingSize::Workbench)),
            },
            false,
            true,
        ),
        (
            "partial inside inventory",
            plain(partial(StackView::Inventory, 0, 35)),
            false,
            false,
        ),
        (
            "partial inside crafting",
            InputAction {
                intent: partial(StackView::Crafting, 0, 40),
                container: None,
                crafting: Some(crafting_token(epoch(1), 13, CraftingSize::Personal)),
            },
            false,
            true,
        ),
        (
            "partial inside chest",
            InputAction {
                intent: partial(StackView::Container(chest(1)), 36, 40),
                container: Some(container_token(epoch(1), chest(1), 11)),
                crafting: None,
            },
            true,
            false,
        ),
        (
            "quick move inside inventory",
            plain(source(StackView::Inventory, 35, true)),
            false,
            false,
        ),
        (
            "quick move inside crafting",
            InputAction {
                intent: source(StackView::Crafting, 40, true),
                container: None,
                crafting: Some(crafting_token(epoch(1), 13, CraftingSize::Personal)),
            },
            false,
            true,
        ),
        (
            "quick move inside chest",
            InputAction {
                intent: source(StackView::Container(chest(1)), 36, true),
                container: Some(container_token(epoch(1), chest(1), 11)),
                crafting: None,
            },
            true,
            false,
        ),
        (
            "drop stack from inventory",
            plain(source(StackView::Inventory, 35, false)),
            false,
            false,
        ),
        (
            "drop stack from crafting",
            InputAction {
                intent: source(StackView::Crafting, 41, false),
                container: None,
                crafting: Some(crafting_token(epoch(1), 13, CraftingSize::Personal)),
            },
            false,
            true,
        ),
        (
            "drop stack from chest",
            InputAction {
                intent: source(StackView::Container(chest(1)), 40, false),
                container: Some(container_token(epoch(1), chest(1), 11)),
                crafting: None,
            },
            true,
            false,
        ),
    ];
    for (name, action, container_required, crafting_required) in cases {
        let mut admission = owner(1);
        let mirror = full_mirror(1);
        if container_required {
            let without = InputAction {
                container: None,
                ..action.clone()
            };
            assert_eq!(
                submit(vec![without], &mirror, &mut admission),
                Err(ClientError::InvalidInput),
                "{name}: a missing container token rejects the whole batch"
            );
        }
        if crafting_required {
            let without = InputAction {
                crafting: None,
                ..action.clone()
            };
            assert_eq!(
                submit(vec![without], &mirror, &mut admission),
                Err(ClientError::InvalidInput),
                "{name}: a missing crafting token rejects the whole batch"
            );
        }
        assert_eq!(
            admission.next_sequence(),
            1,
            "{name}: the token rejections consumed no sequence"
        );
        let receipt = submit(vec![action], &mirror, &mut admission)
            .unwrap_or_else(|error| panic!("{name}: the matched token admits: {error:?}"));
        assert_eq!(
            receipt,
            InputReceipt::Queued {
                epoch: epoch(1),
                first_sequence: Some(1),
                sequenced_count: 1,
                chat_count: 0,
            },
            "{name}: exact receipt"
        );
        assert_eq!(admission.journal_entries(), 1, "{name}: one entry");
    }
}

#[test]
fn missing_irrelevant_mismatched_and_stale_tokens_reject_whole_batch() {
    let mut admission = owner(1);
    let mirror = full_mirror(1);
    let cases: Vec<(&str, Vec<InputAction>, ClientError)> = vec![
        (
            "close without a container token",
            vec![InputAction {
                intent: ClientIntent::CloseContainer,
                container: None,
                crafting: None,
            }],
            ClientError::InvalidInput,
        ),
        (
            "movement with an irrelevant container token",
            vec![InputAction {
                intent: ClientIntent::PlayerInput(control(1, 0)),
                container: Some(container_token(epoch(1), chest(1), 11)),
                crafting: None,
            }],
            ClientError::InvalidInput,
        ),
        (
            "chat with an irrelevant crafting token",
            vec![InputAction {
                intent: ClientIntent::Chat(ChatIntent::new(chat_text(4))),
                container: None,
                crafting: Some(crafting_token(epoch(1), 13, CraftingSize::Personal)),
            }],
            ClientError::InvalidInput,
        ),
        (
            "crafting move with a fabricated workbench container token",
            vec![InputAction {
                intent: crafting_move(0, 40),
                container: Some(container_token(epoch(1), chest(1), 11)),
                crafting: Some(crafting_token(epoch(1), 13, CraftingSize::Personal)),
            }],
            ClientError::InvalidInput,
        ),
        (
            "close with a crafting token",
            vec![InputAction {
                intent: ClientIntent::CloseContainer,
                container: Some(container_token(epoch(1), chest(1), 11)),
                crafting: Some(crafting_token(epoch(1), 13, CraftingSize::Personal)),
            }],
            ClientError::InvalidInput,
        ),
        (
            "container move naming another reference than its token",
            vec![InputAction {
                intent: container_move(chest(1), 36, 40),
                container: Some(container_token(epoch(1), furnace(1), 12)),
                crafting: None,
            }],
            ClientError::InvalidInput,
        ),
        (
            "container move with a stale revision",
            vec![InputAction {
                intent: container_move(chest(1), 36, 40),
                container: Some(container_token(epoch(1), chest(1), 10)),
                crafting: None,
            }],
            ClientError::StaleEpoch,
        ),
        (
            "container move with a previous-epoch token",
            vec![InputAction {
                intent: container_move(chest(1), 36, 40),
                container: Some(container_token(epoch(9), chest(1), 11)),
                crafting: None,
            }],
            ClientError::StaleEpoch,
        ),
        (
            "crafting move with a stale revision",
            vec![InputAction {
                intent: crafting_move(0, 40),
                container: None,
                crafting: Some(crafting_token(epoch(1), 12, CraftingSize::Personal)),
            }],
            ClientError::StaleEpoch,
        ),
        (
            "one invalid action rejects the whole batch",
            vec![
                plain(ClientIntent::PlayerInput(control(1, 0))),
                plain(ClientIntent::DropSelectedItem),
                InputAction {
                    intent: ClientIntent::PlaceBlock(
                        PlacementIntent::try_new(fixture_look(), 3).expect("placement"),
                    ),
                    container: Some(container_token(epoch(1), chest(1), 11)),
                    crafting: None,
                },
            ],
            ClientError::InvalidInput,
        ),
    ];
    for (name, actions, error) in cases {
        assert_eq!(
            submit(actions, &mirror, &mut admission),
            Err(error),
            "{name}"
        );
    }
    assert_untouched(&admission);
    assert_eq!(mirror, full_mirror(1), "validate never mutates the mirror");
    // The owner stays usable: a correctly tokened action still admits.
    let receipt = submit(
        vec![chest_action(
            ClientIntent::MoveContainer(
                ContainerMove::try_new(
                    chest(1).chunk(),
                    chest(1).kind(),
                    chest(1).slot(),
                    chest(1).generation(),
                    36,
                    40,
                )
                .expect("container move"),
            ),
            1,
            11,
        )],
        &mirror,
        &mut admission,
    )
    .expect("a matched action still admits after rejections");
    assert_eq!(
        receipt,
        InputReceipt::Queued {
            epoch: epoch(1),
            first_sequence: Some(1),
            sequenced_count: 1,
            chat_count: 0,
        }
    );
}

#[test]
fn nonfinite_and_out_of_range_payloads_reject_at_checked_types() {
    // The payload constructors are the batch's finiteness and range gates: a
    // NaN rotation or an out-of-range slot is unrepresentable in a batch.
    assert!(LookAngles::try_new(f32::NAN, 0.0).is_err(), "NaN yaw");
    assert!(
        LookAngles::try_new(0.0, f32::INFINITY).is_err(),
        "infinite pitch"
    );
    assert!(
        mornlea_domain::HotbarSlot::new(9).is_err(),
        "hotbar outside 0..=8"
    );
    assert!(
        PlacementIntent::try_new(fixture_look(), 9).is_err(),
        "placement slot outside the hotbar"
    );
    assert!(InventoryMove::try_new(36, 0).is_err(), "slot at the bound");
    assert!(InventoryMove::try_new(1, 1).is_err(), "same-slot move");
    assert!(
        CommandText::try_from_canonical("a".repeat(1025)).is_err(),
        "chat text above 1024 bytes"
    );
    assert!(
        CommandText::try_from_canonical("a".repeat(1024)).is_ok(),
        "chat text at exactly 1024 bytes"
    );
    // The movement axes keep their full i8 range by contract, so both
    // extremes ride a valid batch.
    let mut admission = owner(1);
    let mirror = full_mirror(1);
    for (expected_sequence, move_x) in (1u64..).zip([i8::MIN, i8::MAX]) {
        let receipt = submit(
            vec![plain(ClientIntent::PlayerInput(control(move_x, 0)))],
            &mirror,
            &mut admission,
        )
        .expect("full i8 axis range is admitted");
        assert_eq!(
            receipt,
            InputReceipt::Queued {
                epoch: epoch(1),
                first_sequence: Some(expected_sequence),
                sequenced_count: 1,
                chat_count: 0,
            },
            "exact receipt at the axis extreme {move_x}"
        );
    }
    // The 1024-byte bound is a whole-batch bound too: the maximal text is
    // admitted in full and never partially sent.
    let receipt = submit(
        vec![plain(ClientIntent::Chat(ChatIntent::new(chat_text(1024))))],
        &mirror,
        &mut admission,
    )
    .expect("the maximal chat text admits");
    assert_eq!(
        receipt,
        InputReceipt::Queued {
            epoch: epoch(1),
            first_sequence: None,
            sequenced_count: 0,
            chat_count: 1,
        }
    );
}

#[test]
fn empty_chat_only_and_mixed_batches_sequence_exactly() {
    let mut admission = owner(1);
    let mirror = full_mirror(1);
    assert_eq!(
        submit(Vec::new(), &mirror, &mut admission),
        Ok(InputReceipt::Noop),
        "a zero-action batch is a valid noop"
    );
    assert_untouched(&admission);

    let receipt = submit(
        vec![
            plain(ClientIntent::Chat(ChatIntent::new(chat_text(4)))),
            plain(ClientIntent::Chat(ChatIntent::new(chat_text(6)))),
        ],
        &mirror,
        &mut admission,
    )
    .expect("chat-only batch admits");
    assert_eq!(
        receipt,
        InputReceipt::Queued {
            epoch: epoch(1),
            first_sequence: None,
            sequenced_count: 0,
            chat_count: 2,
        },
        "chat is never sequenced"
    );
    assert_eq!(admission.next_sequence(), 1, "chat consumes no sequence");
    assert_eq!(admission.journal_entries(), 0, "chat is never journaled");
    assert_eq!(
        admission.outbound_records(),
        2,
        "chat still queues once each"
    );

    let receipt = submit(
        vec![
            plain(ClientIntent::PlayerInput(control(1, 0))),
            plain(ClientIntent::Chat(ChatIntent::new(chat_text(2)))),
            plain(ClientIntent::SelectHotbar(
                mornlea_domain::HotbarSlot::new(0).expect("slot"),
            )),
        ],
        &mirror,
        &mut admission,
    )
    .expect("mixed batch admits");
    assert_eq!(
        receipt,
        InputReceipt::Queued {
            epoch: epoch(1),
            first_sequence: Some(1),
            sequenced_count: 2,
            chat_count: 1,
        },
        "the interleaved chat neither takes nor shifts a sequence"
    );
    assert_eq!(admission.next_sequence(), 3, "two commands consumed");
    assert_eq!(admission.journal_entries(), 2, "two journal entries");
    assert_eq!(admission.outbound_records(), 5, "three more queued records");
}

#[test]
fn validate_rejects_wrong_phase_and_epoch_read_only() {
    let admission = owner(1);
    let pending =
        ConfirmedMirror::try_new(ConfirmedMirrorParts::pending(epoch(1))).expect("pending mirror");
    let pending_before = pending.clone();
    let batch =
        InputBatch::try_new(epoch(1), vec![plain(ClientIntent::DropSelectedItem)]).expect("batch");
    assert_eq!(
        InputTranslator::validate_batch(&batch, &pending, admission.limits()),
        Err(ClientError::InvalidState),
        "nothing validates before the session is admitted"
    );
    let other_epoch = admitted_mirror(epoch(2), 1, empty_inventory());
    assert_eq!(
        InputTranslator::validate_batch(&batch, &other_epoch, admission.limits()),
        Err(ClientError::StaleEpoch),
        "a batch of another epoch is stale"
    );
    assert_eq!(pending, pending_before, "validate never mutates the mirror");
    assert_untouched(&admission);
}

#[test]
fn batch_action_ceiling_admits_128_rejects_129_atomically() {
    let mut admission = owner(1);
    let mirror = full_mirror(1);
    let one = plain(ClientIntent::PlayerInput(control(1, 0)));
    let admitted: Vec<InputAction> = std::iter::repeat_n(one.clone(), 128).collect();
    let receipt = submit(admitted, &mirror, &mut admission)
        .expect("128 actions, the measured per-batch ceiling, admit");
    assert_eq!(
        receipt,
        InputReceipt::Queued {
            epoch: epoch(1),
            first_sequence: Some(1),
            sequenced_count: 128,
            chat_count: 0,
        }
    );
    assert_eq!(admission.next_sequence(), 129);
    assert_eq!(admission.journal_entries(), 128);

    let rejected: Vec<InputAction> = std::iter::repeat_n(one.clone(), 129).collect();
    assert_eq!(
        InputBatch::try_new(mirror.epoch(), rejected),
        Err(ClientError::Capacity),
        "129 actions reject the whole batch at construction"
    );
    let tight_owner = InputAdmissionState::try_new(epoch(1), tightened(4, 4104, 8 << 20, 256))
        .expect("tightened owner");
    let five: Vec<InputAction> = std::iter::repeat_n(one, 5).collect();
    let batch = InputBatch::try_new(mirror.epoch(), five).expect("five actions fit the hard cap");
    assert_eq!(
        InputTranslator::validate_batch(&batch, &mirror, tight_owner.limits()),
        Err(ClientError::Capacity),
        "a configured tighter ceiling rejects the whole batch"
    );
    assert_untouched(&tight_owner);
    assert_eq!(
        admission.outbound_records(),
        128,
        "unchanged by the rejections"
    );
}

#[test]
fn outbound_record_and_byte_bounds_admit_n_reject_n_plus_one() {
    let mirror = full_mirror(1);
    let action = || plain(ClientIntent::PlayerInput(control(1, 0)));

    // Records: a configured two-record queue admits exactly two.
    let mut records =
        InputAdmissionState::try_new(epoch(1), tightened(128, 2, 8 << 20, 256)).expect("owner");
    assert!(submit(vec![action()], &mirror, &mut records).is_ok());
    assert!(submit(vec![action()], &mirror, &mut records).is_ok());
    let before = records.next_sequence();
    assert_eq!(
        submit(vec![action()], &mirror, &mut records),
        Err(ClientError::Capacity),
        "the third record is a valid over-limit demand"
    );
    assert_eq!(records.next_sequence(), before, "no sequence consumed");
    assert_eq!(records.outbound_records(), 2, "no partial send");

    // Bytes: a queue sized to exactly two frames admits two and rejects the
    // third whole batch.
    let mut probe = Vec::new();
    let frame = action()
        .intent
        .encode_frame(1, &mut probe)
        .expect("probe frame");
    let mut bytes = InputAdmissionState::try_new(epoch(1), tightened(128, 4104, 2 * frame, 256))
        .expect("owner");
    assert!(submit(vec![action()], &mirror, &mut bytes).is_ok());
    assert!(submit(vec![action()], &mirror, &mut bytes).is_ok());
    let before = bytes.next_sequence();
    assert_eq!(
        submit(vec![action()], &mirror, &mut bytes),
        Err(ClientError::Capacity),
        "the third frame's bytes are a valid over-limit demand"
    );
    assert_eq!(bytes.next_sequence(), before, "no sequence consumed");
    assert_eq!(bytes.outbound_records(), 2, "no partial send");
    assert_eq!(bytes.outbound_bytes(), 2 * frame, "exact queued bytes");
}

#[test]
fn full_journal_rejects_sequenced_batches_but_admits_chat() {
    let mut admission = owner(1);
    let mirror = full_mirror(1);
    let one = plain(ClientIntent::PlayerInput(control(1, 0)));
    for _ in 0..2 {
        let batch: Vec<InputAction> = std::iter::repeat_n(one.clone(), 128).collect();
        assert!(submit(batch, &mirror, &mut admission).is_ok());
    }
    assert_eq!(
        admission.journal_entries(),
        256,
        "the measured 256-entry journal is exactly full"
    );
    let before = admission.next_sequence();
    assert_eq!(
        submit(vec![one], &mirror, &mut admission),
        Err(ClientError::Capacity),
        "one more sequenced command is a valid over-limit demand"
    );
    assert_eq!(admission.next_sequence(), before, "no sequence consumed");
    assert_eq!(admission.outbound_records(), 256, "no partial send");
    assert!(
        submit(
            vec![plain(ClientIntent::Chat(ChatIntent::new(chat_text(4))))],
            &mirror,
            &mut admission
        )
        .is_ok(),
        "chat demands no journal entry and still admits"
    );
    assert_eq!(admission.journal_entries(), 256);
}

#[test]
fn close_then_move_in_one_batch_rejects_without_tombstone() {
    let mut admission = owner(1);
    let mirror = full_mirror(1);
    let actions = vec![
        chest_action(ClientIntent::CloseContainer, 1, 11),
        chest_action(container_move(chest(1), 36, 40), 1, 11),
    ];
    assert_eq!(
        submit(actions, &mirror, &mut admission),
        Err(ClientError::InvalidState),
        "a later external-view action after the close in one batch rejects"
    );
    assert_untouched(&admission);
    assert!(
        admission.overlay().tombstones().is_empty(),
        "the rejected close must not leave a tombstone"
    );
    assert_eq!(mirror, full_mirror(1), "the mirror is untouched");
}

#[test]
fn committed_close_blocks_move_while_mirror_still_confirms() {
    let mut admission = owner(1);
    let mirror = full_mirror(1);
    let receipt = submit(
        vec![chest_action(ClientIntent::CloseContainer, 1, 11)],
        &mirror,
        &mut admission,
    )
    .expect("the close admits");
    assert_eq!(
        receipt,
        InputReceipt::Queued {
            epoch: epoch(1),
            first_sequence: Some(1),
            sequenced_count: 1,
            chat_count: 0,
        }
    );
    assert_eq!(
        admission.overlay().tombstones(),
        &[(chest(1), ConfirmedRevision::new(11))]
    );
    // The confirmed mirror still holds the container, so the move's token
    // stays current and validates — the block is the local overlay's.
    let before = admission.next_sequence();
    assert_eq!(
        submit(
            vec![chest_action(container_move(chest(1), 36, 40), 1, 11)],
            &mirror,
            &mut admission
        ),
        Err(ClientError::InvalidState),
        "the locally dismissed view cannot take a further move"
    );
    assert_eq!(admission.next_sequence(), before, "no sequence consumed");
    assert_eq!(
        admission.overlay().tombstones(),
        &[(chest(1), ConfirmedRevision::new(11))],
        "the tombstone survives the rejection"
    );
    // Opening a view alone does not revive the old token either.
    assert_eq!(
        submit(
            vec![
                plain(ClientIntent::OpenContainer(fixture_look())),
                plain(ClientIntent::PlayerInput(control(1, 0)))
            ],
            &mirror,
            &mut admission
        ),
        Ok(InputReceipt::Queued {
            epoch: epoch(1),
            first_sequence: Some(2),
            sequenced_count: 2,
            chat_count: 0,
        }),
        "unrelated actions still admit while the view is dismissed"
    );
    assert_eq!(
        admission.overlay().tombstones(),
        &[(chest(1), ConfirmedRevision::new(11))],
        "OpenContainer admission never retires the tombstone"
    );
}

#[test]
fn unrelated_observation_cannot_reopen_locally_closed_view() {
    let mut admission = owner(1);
    let mirror = full_mirror(1);
    assert!(
        submit(
            vec![chest_action(ClientIntent::CloseContainer, 1, 11)],
            &mirror,
            &mut admission
        )
        .is_ok()
    );
    // An unrelated confirmed observation lands: the crafting view advances
    // and the revision moves, but the chest view is untouched at its closing
    // revision.
    let unrelated = admitted_mirror(
        epoch(1),
        21,
        full_inventory().with_crafting_view(CraftingSize::Personal, ConfirmedRevision::new(21)),
    );
    let before = admission.next_sequence();
    assert_eq!(
        submit(
            vec![chest_action(container_move(chest(1), 36, 40), 1, 11)],
            &unrelated,
            &mut admission
        ),
        Err(ClientError::InvalidState),
        "an unrelated observation leaves the view locally closed"
    );
    assert_eq!(admission.next_sequence(), before, "no sequence consumed");
}

#[test]
fn confirmed_close_or_replacement_retires_tombstone_and_admits_fresh_token() {
    let mut admission = owner(1);
    let mirror = full_mirror(1);
    assert!(
        submit(
            vec![chest_action(ClientIntent::CloseContainer, 1, 11)],
            &mirror,
            &mut admission
        )
        .is_ok()
    );
    assert_eq!(admission.overlay().tombstones().len(), 1);

    // The authoritative close is confirmed: the confirmed view is removed, so
    // the old token is stale, and a re-placed view at a fresh revision
    // supplies fresh attribution.
    let confirmed_closed = admitted_mirror(epoch(1), 21, {
        let mut inventory = full_inventory();
        inventory.retire_container(&chest(1));
        inventory
    });
    assert_eq!(
        submit(
            vec![chest_action(container_move(chest(1), 36, 40), 1, 11)],
            &confirmed_closed,
            &mut admission
        ),
        Err(ClientError::StaleEpoch),
        "the retired view's old token no longer validates"
    );
    assert_eq!(
        admission.overlay().tombstones().len(),
        1,
        "a rejected batch cannot retire the overlay"
    );

    let reopened = admitted_mirror(
        epoch(1),
        22,
        full_inventory().with_container_view(chest(1), ConfirmedRevision::new(22)),
    );
    let receipt = submit(
        vec![chest_action(container_move(chest(1), 36, 40), 1, 22)],
        &reopened,
        &mut admission,
    )
    .expect("the fresh token of the new view admits");
    assert_eq!(
        receipt,
        InputReceipt::Queued {
            epoch: epoch(1),
            first_sequence: Some(2),
            sequenced_count: 1,
            chat_count: 0,
        }
    );
    assert!(
        admission.overlay().tombstones().is_empty(),
        "the replaced view retires the tombstone"
    );

    // A locally closed furnace generation stays closed until its position's
    // view is replaced by a newer generation, which retires it the same way.
    assert!(
        submit(
            vec![InputAction {
                intent: ClientIntent::CloseContainer,
                container: Some(container_token(epoch(1), furnace(1), 12)),
                crafting: None,
            }],
            &reopened,
            &mut admission
        )
        .is_ok()
    );
    let replaced = admitted_mirror(epoch(1), 23, {
        let mut inventory = full_inventory();
        inventory.retire_container(&furnace(1));
        inventory
            .with_container_view(furnace(2), ConfirmedRevision::new(23))
            .with_container_view(chest(1), ConfirmedRevision::new(22))
    });
    let fresh = ContainerMove::try_new(
        furnace(2).chunk(),
        furnace(2).kind(),
        furnace(2).slot(),
        furnace(2).generation(),
        0,
        36,
    )
    .expect("fresh-generation move");
    assert!(
        submit(
            vec![InputAction {
                intent: ClientIntent::MoveContainer(fresh),
                container: Some(container_token(epoch(1), furnace(2), 23)),
                crafting: None,
            }],
            &replaced,
            &mut admission
        )
        .is_ok()
    );
    assert!(
        admission.overlay().tombstones().is_empty(),
        "the newer-generation replacement retires the old-generation tombstone"
    );
}

#[test]
fn rejected_close_neither_closes_nor_reopens_silently() {
    let mut admission = owner(1);
    let mirror = full_mirror(1);
    // Without any committed close, a rejected close batch leaves no tombstone
    // and the view stays usable.
    assert_eq!(
        submit(
            vec![
                chest_action(ClientIntent::CloseContainer, 1, 11),
                InputAction {
                    intent: ClientIntent::PlaceBlock(
                        PlacementIntent::try_new(fixture_look(), 3).expect("placement"),
                    ),
                    container: Some(container_token(epoch(1), chest(1), 11)),
                    crafting: None,
                },
            ],
            &mirror,
            &mut admission
        ),
        Err(ClientError::InvalidInput),
        "the trailing invalid action rejects the whole batch, close included"
    );
    assert!(
        admission.overlay().tombstones().is_empty(),
        "a rejected close never closes the view"
    );
    assert!(
        submit(
            vec![chest_action(container_move(chest(1), 36, 40), 1, 11)],
            &mirror,
            &mut admission
        )
        .is_ok()
    );

    // With a committed close, a later rejected close batch cannot silently
    // reopen the dismissed view either.
    assert!(
        submit(
            vec![chest_action(ClientIntent::CloseContainer, 1, 11)],
            &mirror,
            &mut admission
        )
        .is_ok()
    );
    assert_eq!(admission.overlay().tombstones().len(), 1);
    assert_eq!(
        submit(
            vec![
                chest_action(ClientIntent::CloseContainer, 1, 11),
                InputAction {
                    intent: ClientIntent::PlaceBlock(
                        PlacementIntent::try_new(fixture_look(), 3).expect("placement"),
                    ),
                    container: Some(container_token(epoch(1), chest(1), 11)),
                    crafting: None,
                },
            ],
            &mirror,
            &mut admission
        ),
        Err(ClientError::InvalidInput)
    );
    assert_eq!(
        admission.overlay().tombstones().len(),
        1,
        "the rejected re-close leaves the dismissal in place"
    );
    assert_eq!(
        submit(
            vec![chest_action(container_move(chest(1), 36, 40), 1, 11)],
            &mirror,
            &mut admission
        ),
        Err(ClientError::InvalidState),
        "the view is still locally closed"
    );
}

#[test]
fn reset_clears_overlay_sequence_and_local_cue_attribution() {
    let mut admission = owner(1);
    let mirror = full_mirror(1);
    assert!(
        submit(
            vec![
                chest_action(ClientIntent::CloseContainer, 1, 11),
                plain(ClientIntent::CollectWater(fixture_look())),
            ],
            &mirror,
            &mut admission
        )
        .is_ok()
    );
    assert_eq!(admission.overlay().tombstones().len(), 1);
    assert_eq!(admission.pending_local_cues().len(), 1);
    assert_eq!(admission.next_sequence(), 3);

    admission
        .reset_epoch(epoch(2), limits())
        .expect("reset to a fresh epoch");
    assert_untouched(&admission);
    assert_eq!(admission.overlay().epoch(), epoch(2));

    // The fresh epoch restarts every sequence space: its first command takes
    // sequence 1 and its first local cue takes native local sequence 1.
    let fresh_mirror = admitted_mirror(
        epoch(2),
        21,
        full_inventory()
            .with_container_view(chest(1), ConfirmedRevision::new(21))
            .with_crafting_view(CraftingSize::Personal, ConfirmedRevision::new(13))
            .with_crafting_view(CraftingSize::Workbench, ConfirmedRevision::new(14)),
    );
    let receipt = submit(
        vec![
            chest_action(container_move(chest(1), 36, 40), 2, 21),
            plain(ClientIntent::CollectWater(fixture_look())),
        ],
        &fresh_mirror,
        &mut admission,
    )
    .expect("the fresh epoch's fresh token admits after reset");
    assert_eq!(
        receipt,
        InputReceipt::Queued {
            epoch: epoch(2),
            first_sequence: Some(1),
            sequenced_count: 2,
            chat_count: 0,
        }
    );
    assert!(
        admission.overlay().tombstones().is_empty(),
        "the reset cleared the old epoch's dismissal"
    );
    assert_eq!(
        admission.pending_local_cues(),
        &[LocalCueSource {
            local_event_sequence: 1,
            kind: ClientIntentKind::CollectWater,
        }],
        "the native local cue sequence restarts with the epoch"
    );
}

#[test]
fn stale_epoch_validated_batch_cannot_enter_reset_owner() {
    // The binding ledger pointer from the contract re-review: a batch
    // validated against one epoch's mirror must not commit into an owner that
    // a reset moved to another epoch, or epoch-N journal entries would ride
    // epoch-M's restarted sequence space.
    let first = full_mirror(1);
    let batch = InputBatch::try_new(
        epoch(1),
        vec![plain(ClientIntent::PlayerInput(control(1, 0)))],
    )
    .expect("batch");
    let validated = InputTranslator::validate_batch(&batch, &first, &limits())
        .expect("validated against the first epoch's mirror");

    let mut reset_owner = InputAdmissionState::try_new(epoch(2), limits()).expect("owner");
    assert_eq!(
        InputTranslator::commit(validated, &mut reset_owner),
        Err(ClientError::StaleEpoch),
        "the owner's current epoch governs, not the validate-time snapshot's"
    );
    assert_untouched(&reset_owner);
}

#[test]
fn accepted_collect_water_emits_bounded_native_local_cue_sources() {
    let mut admission = owner(1);
    let mirror = full_mirror(1);
    let bucket = || plain(ClientIntent::CollectWater(fixture_look()));
    let receipt = submit(vec![bucket(), bucket(), bucket()], &mirror, &mut admission)
        .expect("three accepted bucket actions");
    assert_eq!(
        receipt,
        InputReceipt::Queued {
            epoch: epoch(1),
            first_sequence: Some(1),
            sequenced_count: 3,
            chat_count: 0,
        }
    );
    assert_eq!(
        admission.pending_local_cues(),
        &[
            LocalCueSource {
                local_event_sequence: 1,
                kind: ClientIntentKind::CollectWater,
            },
            LocalCueSource {
                local_event_sequence: 2,
                kind: ClientIntentKind::CollectWater,
            },
            LocalCueSource {
                local_event_sequence: 3,
                kind: ClientIntentKind::CollectWater,
            },
        ],
        "each accepted cue-source action emits one native local event in order"
    );

    // Actions outside the source cue inventory emit nothing.
    assert!(
        submit(
            vec![
                plain(ClientIntent::PlayerInput(control(1, 0))),
                plain(ClientIntent::Chat(ChatIntent::new(chat_text(4)))),
                plain(ClientIntent::PlaceWater(fixture_look())),
            ],
            &mirror,
            &mut admission
        )
        .is_ok()
    );
    assert_eq!(
        admission.pending_local_cues().len(),
        3,
        "no other admitted action emits a cue source"
    );

    // A rejected batch emits nothing and leaves the pending attribution
    // exactly as it was.
    assert_eq!(
        submit(
            vec![InputAction {
                intent: ClientIntent::PlaceBlock(
                    PlacementIntent::try_new(fixture_look(), 3).expect("placement"),
                ),
                container: Some(container_token(epoch(1), chest(1), 11)),
                crafting: None,
            }],
            &mirror,
            &mut admission
        ),
        Err(ClientError::InvalidInput)
    );
    assert_eq!(admission.pending_local_cues().len(), 3);

    // The pending cue queue is bounded like every input-side bound: it admits
    // the measured per-batch event count and rejects one more with Capacity
    // before any sequence advance.
    while admission.pending_local_cues().len() < 128 {
        assert!(submit(vec![bucket()], &mirror, &mut admission).is_ok());
    }
    let before = admission.next_sequence();
    assert_eq!(
        submit(vec![bucket()], &mirror, &mut admission),
        Err(ClientError::Capacity),
        "the cue-source bound rejects the next demand"
    );
    assert_eq!(admission.next_sequence(), before, "no sequence consumed");
    assert_eq!(admission.pending_local_cues().len(), 128, "no partial cue");
}

/// The input projection state carries the pending local cue-source events the
/// admission commit emits: a real admitted semantic UI action populates the
/// projection-side copy with the same native local events, a rejected batch
/// populates nothing, a read never consumes either copy, and reset clears the
/// projection side with the rest.
#[test]
fn projection_state_carries_pending_local_cues() {
    let mut admission = owner(1);
    let mirror = full_mirror(1);
    let bucket = || plain(ClientIntent::CollectWater(fixture_look()));

    // Before anything is admitted both copies are empty.
    assert!(
        admission.projection().pending_local_cues().is_empty(),
        "a fresh owner's projection state carries no cue events"
    );

    // A real admitted semantic UI action: the projection state the owner
    // exposes carries exactly the emitted events, native sequence and kind.
    assert!(submit(vec![bucket(), bucket()], &mirror, &mut admission).is_ok());
    let expected = [
        LocalCueSource {
            local_event_sequence: 1,
            kind: ClientIntentKind::CollectWater,
        },
        LocalCueSource {
            local_event_sequence: 2,
            kind: ClientIntentKind::CollectWater,
        },
    ];
    assert_eq!(
        admission.projection().pending_local_cues(),
        &expected,
        "the projection state carries the emitted native local events"
    );
    assert_eq!(
        admission.pending_local_cues(),
        &expected,
        "the owner's own queue holds the same unconsumed events"
    );

    // Reads never consume: a second read of either copy returns the same
    // events.
    assert_eq!(admission.projection().pending_local_cues(), &expected);
    assert_eq!(admission.pending_local_cues(), &expected);

    // A rejected batch populates nothing on the projection side.
    assert_eq!(
        submit(
            vec![InputAction {
                intent: ClientIntent::PlaceBlock(
                    PlacementIntent::try_new(fixture_look(), 3).expect("placement"),
                ),
                container: Some(container_token(epoch(1), chest(1), 11)),
                crafting: None,
            }],
            &mirror,
            &mut admission
        ),
        Err(ClientError::InvalidInput)
    );
    assert_eq!(
        admission.projection().pending_local_cues(),
        &expected,
        "a rejected batch stages no projection-side cue"
    );

    // An admitted action outside the source cue inventory populates nothing
    // either.
    assert!(
        submit(
            vec![plain(ClientIntent::PlayerInput(control(1, 0)))],
            &mirror,
            &mut admission
        )
        .is_ok()
    );
    assert_eq!(
        admission.projection().pending_local_cues(),
        &expected,
        "only cue-source actions populate the projection side"
    );

    // Reset clears the projection-side cues with the rest, and the new
    // epoch's first cue restarts at native local sequence one on both
    // copies.
    admission.reset_epoch(epoch(2), limits()).expect("reset");
    assert!(
        admission.projection().pending_local_cues().is_empty(),
        "reset clears the projection-side cue events"
    );
    let fresh_mirror = full_mirror(2);
    assert!(submit(vec![bucket()], &fresh_mirror, &mut admission).is_ok());
    let fresh = [LocalCueSource {
        local_event_sequence: 1,
        kind: ClientIntentKind::CollectWater,
    }];
    assert_eq!(admission.projection().pending_local_cues(), &fresh);
    assert_eq!(admission.pending_local_cues(), &fresh);
}

#[test]
fn wrong_double_sequence_consumption_rejected() {
    // The deliberately wrong artifact: in `Broken` mode the contract double
    // consumes a sequence even when a batch is rejected. The real admission
    // owner — driven through the same harness surface in contract mode —
    // never advances on a rejection.
    let mut broken = ReplayHarness::new(DoubleMode::Broken).expect("broken harness");
    let epoch = broken.connect(identity("input", 1)).expect("pending epoch");
    broken.double.admit().expect("admitted double");
    let rejected = InputBatch::try_new(
        epoch,
        vec![InputAction {
            intent: ClientIntent::PlaceBlock(
                PlacementIntent::try_new(fixture_look(), 3).expect("placement"),
            ),
            container: Some(container_token(epoch, chest(1), 11)),
            crafting: None,
        }],
    )
    .expect("batch");
    assert!(broken.submit_input(epoch, rejected).is_err());
    assert_eq!(
        broken.double.sequence_witness(),
        2,
        "the wrong double consumed a sequence on rejection"
    );

    let mut harness = ReplayHarness::new(DoubleMode::Contract).expect("harness");
    let epoch = harness.connect(identity("input", 2)).expect("epoch");
    harness.double.admit().expect("admitted");
    let rejected = InputBatch::try_new(
        epoch,
        vec![InputAction {
            intent: ClientIntent::PlaceBlock(
                PlacementIntent::try_new(fixture_look(), 3).expect("placement"),
            ),
            container: Some(container_token(epoch, chest(1), 11)),
            crafting: None,
        }],
    )
    .expect("batch");
    assert_eq!(
        harness.submit_input(epoch, rejected),
        Err(ClientError::InvalidInput)
    );
    assert_eq!(
        harness.double.sequence_witness(),
        1,
        "the real admission path consumes no sequence on rejection"
    );
    let accepted =
        InputBatch::try_new(epoch, vec![plain(ClientIntent::PlayerInput(control(1, 0)))])
            .expect("batch");
    assert!(harness.submit_input(epoch, accepted).is_ok());
    assert_eq!(
        harness.double.sequence_witness(),
        2,
        "only the accepted batch advances"
    );
}

/// One checked login identity for the harness cases, reusing the harness
/// smoke fixture's valid versioned uuid with a per-call distinct tail.
fn identity(name: &str, salt: u8) -> mornlea_client_core::ClientIdentity {
    let mut bytes = [0u8; 16];
    bytes[6] = 0x40;
    bytes[8] = 0x80;
    bytes[15] = salt;
    mornlea_client_core::ClientIdentity::try_new(
        mornlea_protocol::LoginStart::new(
            mornlea_protocol::PlayerId::try_from_bytes(bytes).expect("uuid"),
            name,
            8,
        )
        .expect("login start"),
    )
    .expect("identity")
}
