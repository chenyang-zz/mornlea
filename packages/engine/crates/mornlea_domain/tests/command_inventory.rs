//! Inventory, container and chat command payload contracts for `mornlea_domain`.
//!
//! The Go baseline is the `protocol.MoveInventoryStack` /
//! `protocol.MoveCraftingStack` / `protocol.MoveContainerStack` /
//! `protocol.MoveStackPartial` / `protocol.QuickMoveStack` /
//! `protocol.DropStack` wire records plus the `protocol.ChatCommand` text rule.
//! Every bound below is the static value rule the protocol layer publishes: the
//! item, capacity and slot-content decisions stay with the authority, so this
//! crate refuses to publish a payload the wire cannot name and refuses nothing
//! the wire still admits.

use mornlea_domain::{
    ChatIntent, ChunkPos, Command, CommandText, ContainerKind, ContainerMove, ContainerRef,
    CraftingMove, DomainError, HeldActions, HotbarSlot, InventoryMove, LookAngles, Movement,
    PartialMove, PlacementIntent, PlayerControl, PlayerControlParts, ResyncIntent, StackSource,
    StackView,
};

/// Legal furnace reference used by the container cases.
fn furnace_ref() -> ContainerRef {
    ContainerRef::try_new(ChunkPos::new(-2, 5), ContainerKind::Furnace, 7, 3)
        .expect("a furnace reference inside its fixed array is legal")
}

/// Legal chest reference used by the container cases.
fn chest_ref() -> ContainerRef {
    ContainerRef::try_new(ChunkPos::new(4, -9), ContainerKind::Chest, 5, 11)
        .expect("a chest reference inside its fixed array is legal")
}

#[test]
fn command_inventory_move_accepts_last_slot_and_rejects_above_array_and_same_slot() {
    let last_to_first = InventoryMove::try_new(35, 0).expect("slot 35 is the last inventory slot");
    assert_eq!(last_to_first.from(), 35);
    assert_eq!(last_to_first.to(), 0);

    let first_to_last = InventoryMove::try_new(0, 35).expect("slot 35 is a legal destination");
    assert_eq!(first_to_last.from(), 0);
    assert_eq!(first_to_last.to(), 35);

    // The inventory view is the fixed 36-slot array, so index 36 names nothing.
    assert_eq!(InventoryMove::try_new(36, 0), Err(DomainError::InvalidSlot));
    assert_eq!(InventoryMove::try_new(0, 36), Err(DomainError::InvalidSlot));
    assert_eq!(
        InventoryMove::try_new(u8::MAX, 0),
        Err(DomainError::InvalidSlot)
    );
    // A move onto its own slot has no effect, so the protocol rejects it.
    assert_eq!(
        InventoryMove::try_new(7, 7),
        Err(DomainError::SourceEqualsTarget)
    );
}

#[test]
fn command_inventory_crafting_move_requires_one_grid_end() {
    let grid_to_backpack =
        CraftingMove::try_new(8, 44).expect("grid slot eight to the last backpack slot is legal");
    assert_eq!(grid_to_backpack.from(), 8);
    assert_eq!(grid_to_backpack.to(), 44);

    let backpack_to_grid =
        CraftingMove::try_new(9, 0).expect("the first backpack slot to grid zero is legal");
    assert_eq!(backpack_to_grid.from(), 9);
    assert_eq!(backpack_to_grid.to(), 0);

    // Both ends inside the inventory region is the inventory move's job, so the
    // crafting command refuses it rather than accepting a second spelling.
    assert_eq!(
        CraftingMove::try_new(9, 10),
        Err(DomainError::CraftingMoveInsideInventory)
    );
    assert_eq!(
        CraftingMove::try_new(44, 9),
        Err(DomainError::CraftingMoveInsideInventory)
    );
    // The unified crafting view ends at 44.
    assert_eq!(CraftingMove::try_new(45, 0), Err(DomainError::InvalidSlot));
    assert_eq!(CraftingMove::try_new(0, 45), Err(DomainError::InvalidSlot));
    assert_eq!(
        CraftingMove::try_new(3, 3),
        Err(DomainError::SourceEqualsTarget)
    );
}

#[test]
fn command_inventory_partial_move_keeps_the_looser_crafting_rule() {
    // The protocol publishes no both-ends-in-inventory rule for a partial move,
    // so the domain must not borrow the stricter crafting validator here: the
    // authority applies the item and slot rules later.
    let both_in_backpack = PartialMove::try_new(StackView::Crafting, 9, 10, false)
        .expect("two inventory-region crafting indices are protocol-legal for a partial move");
    assert_eq!(both_in_backpack.from(), 9);
    assert_eq!(both_in_backpack.to(), 10);
    assert!(!both_in_backpack.single());

    // The view bounds themselves still hold for a partial move.
    assert_eq!(
        PartialMove::try_new(StackView::Inventory, 35, 36, false),
        Err(DomainError::InvalidSlot)
    );
    assert_eq!(
        PartialMove::try_new(StackView::Inventory, 36, 0, false),
        Err(DomainError::InvalidSlot)
    );
    assert_eq!(
        PartialMove::try_new(StackView::Crafting, 44, 45, false),
        Err(DomainError::InvalidSlot)
    );
    // A partial move still cannot name one slot as both ends.
    assert_eq!(
        PartialMove::try_new(StackView::Crafting, 9, 9, true),
        Err(DomainError::SourceEqualsTarget)
    );
}

#[test]
fn command_inventory_container_move_treats_furnace_output_as_source_only() {
    let take_output =
        ContainerMove::try_new(ChunkPos::new(-2, 5), ContainerKind::Furnace, 7, 3, 38, 0)
            .expect("the furnace output slot is a legal move source");
    assert_eq!(take_output.container(), furnace_ref());
    assert_eq!(take_output.from(), 38);
    assert_eq!(take_output.to(), 0);

    // The output slot is reserved for taking the smelting product, so a whole
    // container move may not name it as a destination.
    assert_eq!(
        ContainerMove::try_new(ChunkPos::new(-2, 5), ContainerKind::Furnace, 7, 3, 0, 38),
        Err(DomainError::FurnaceOutputAsTarget)
    );
    // The unified furnace view ends at 38.
    assert_eq!(
        ContainerMove::try_new(ChunkPos::new(-2, 5), ContainerKind::Furnace, 7, 3, 38, 39),
        Err(DomainError::InvalidSlot)
    );
    assert_eq!(
        ContainerMove::try_new(ChunkPos::new(-2, 5), ContainerKind::Furnace, 7, 3, 39, 0),
        Err(DomainError::InvalidSlot)
    );
    assert_eq!(
        ContainerMove::try_new(ChunkPos::new(-2, 5), ContainerKind::Furnace, 7, 3, 5, 5),
        Err(DomainError::SourceEqualsTarget)
    );
}

#[test]
fn command_inventory_partial_move_may_target_the_furnace_output() {
    // The protocol publishes no output-slot rule for a partial move either, so
    // slot 38 stays a legal destination here and only the authority decides
    // whether the item fits.
    let into_output = PartialMove::try_new(StackView::Container(furnace_ref()), 0, 38, true)
        .expect("a partial move may name the furnace output slot as its target");
    assert_eq!(into_output.view().container(), Some(furnace_ref()));
    assert_eq!(into_output.from(), 0);
    assert_eq!(into_output.to(), 38);
    assert!(into_output.single());

    // The furnace view bound still applies to a partial move.
    assert_eq!(
        PartialMove::try_new(StackView::Container(furnace_ref()), 38, 39, false),
        Err(DomainError::InvalidSlot)
    );
}

#[test]
fn command_inventory_container_move_chest_bounds() {
    let last_chest_slot =
        ContainerMove::try_new(ChunkPos::new(4, -9), ContainerKind::Chest, 5, 11, 62, 0)
            .expect("slot 62 is the last chest view slot");
    assert_eq!(last_chest_slot.container(), chest_ref());
    assert_eq!(last_chest_slot.from(), 62);
    assert_eq!(last_chest_slot.to(), 0);

    // A chest reserves no slot, so every view index is a legal destination.
    let into_last =
        ContainerMove::try_new(ChunkPos::new(4, -9), ContainerKind::Chest, 5, 11, 0, 62)
            .expect("the chest view has no reserved output slot");
    assert_eq!(into_last.to(), 62);

    assert_eq!(
        ContainerMove::try_new(ChunkPos::new(4, -9), ContainerKind::Chest, 5, 11, 63, 0),
        Err(DomainError::InvalidSlot)
    );
    assert_eq!(
        ContainerMove::try_new(ChunkPos::new(4, -9), ContainerKind::Chest, 5, 11, 0, 63),
        Err(DomainError::InvalidSlot)
    );
}

#[test]
fn command_inventory_partial_move_single_flag_is_preserved() {
    let half = PartialMove::try_new(StackView::Inventory, 0, 1, false)
        .expect("a legal inventory partial move");
    let single = PartialMove::try_new(StackView::Inventory, 0, 1, true)
        .expect("a legal inventory partial move");
    // The flag is the whole difference between the two server-derived counts,
    // so it must survive construction unchanged in both directions.
    assert!(!half.single());
    assert!(single.single());
    assert_ne!(half, single);

    // A stack source carries no split flag: the moved amount is the whole stack.
    let source = StackSource::try_new(StackView::Inventory, 35).expect("slot 35 is legal");
    assert_eq!(source.slot(), 35);
}

#[test]
fn command_inventory_malformed_container_ref_is_rejected_before_slot_checks() {
    // The reference names a chest array slot that does not exist, and both move
    // indices are far outside every view: the reference rejection has to win,
    // which proves the reference is validated before the slot bounds.
    assert_eq!(
        ContainerMove::try_new(ChunkPos::new(0, 0), ContainerKind::Chest, 16, 7, 200, 201),
        Err(DomainError::InvalidContainerSlot)
    );
    assert_eq!(
        ContainerMove::try_new(ChunkPos::new(0, 0), ContainerKind::Furnace, 0, 0, 200, 201),
        Err(DomainError::InvalidContainerGeneration)
    );

    // The partial path cannot carry a malformed reference at all: the container
    // view holds an already-validated `ContainerRef`, so the rejection happens
    // when the reference is built rather than inside the move.
    assert_eq!(
        ContainerRef::try_new(ChunkPos::new(0, 0), ContainerKind::Chest, 16, 7),
        Err(DomainError::InvalidContainerSlot)
    );
    let legal = ContainerRef::try_new(ChunkPos::new(0, 0), ContainerKind::Chest, 15, 7)
        .expect("slot 15 is the last chest array slot");
    assert_eq!(StackView::Container(legal).container(), Some(legal));
    // The absence of a container is the variant itself, never a sentinel
    // reference, so the two non-container views publish none.
    assert_eq!(StackView::Inventory.container(), None);
    assert_eq!(StackView::Crafting.container(), None);
}

#[test]
fn command_inventory_stack_source_shares_view_bounds_and_carries_no_destination() {
    assert_eq!(
        StackSource::try_new(StackView::Inventory, 35)
            .expect("slot 35 is the last inventory slot")
            .slot(),
        35
    );
    assert_eq!(
        StackSource::try_new(StackView::Inventory, 36),
        Err(DomainError::InvalidSlot)
    );
    assert_eq!(
        StackSource::try_new(StackView::Crafting, 44)
            .expect("slot 44 is the last crafting view slot")
            .slot(),
        44
    );
    assert_eq!(
        StackSource::try_new(StackView::Crafting, 45),
        Err(DomainError::InvalidSlot)
    );
    // The container views keep their own bounds, including the furnace output
    // slot, which is a legal source for a whole-stack command.
    assert_eq!(
        StackSource::try_new(StackView::Container(furnace_ref()), 38)
            .expect("the furnace output slot is a legal source")
            .slot(),
        38
    );
    assert_eq!(
        StackSource::try_new(StackView::Container(furnace_ref()), 39),
        Err(DomainError::InvalidSlot)
    );
    assert_eq!(
        StackSource::try_new(StackView::Container(chest_ref()), 62)
            .expect("slot 62 is the last chest view slot")
            .slot(),
        62
    );
    assert_eq!(
        StackSource::try_new(StackView::Container(chest_ref()), 63),
        Err(DomainError::InvalidSlot)
    );
}

#[test]
fn command_inventory_command_carries_every_variant_and_stays_distinct() {
    let look = LookAngles::try_new(0.5, -0.25).expect("finite angles");
    let control = PlayerControl::new(PlayerControlParts {
        movement: Movement {
            move_x: 1,
            move_z: -1,
            jump: true,
        },
        look,
        actions: HeldActions {
            primary: true,
            eating: false,
            sprinting: false,
            sneaking: false,
        },
    });

    let commands = [
        Command::PlayerInput(control),
        Command::PlaceBlock(PlacementIntent::try_new(look, 8).expect("slot eight")),
        Command::Resync(ResyncIntent::try_new(1, ChunkPos::new(-1, 2), 0).expect("legal resync")),
        Command::SelectHotbar(HotbarSlot::new(3).expect("slot three")),
        Command::OpenContainer(look),
        Command::TillSoil(look),
        Command::BoneMeal(look),
        Command::CollectWater(look),
        Command::PlaceWater(look),
        Command::MoveInventory(InventoryMove::try_new(0, 1).expect("legal inventory move")),
        Command::MoveCrafting(CraftingMove::try_new(0, 9).expect("legal crafting move")),
        Command::MoveContainer(
            ContainerMove::try_new(ChunkPos::new(0, 0), ContainerKind::Furnace, 0, 1, 0, 1)
                .expect("legal container move"),
        ),
        Command::CloseContainer,
        Command::DropSelectedItem,
        Command::TakeCraftingOutput,
        Command::EquipArmor,
        Command::MovePartial(
            PartialMove::try_new(StackView::Inventory, 0, 1, false).expect("legal partial move"),
        ),
        Command::QuickMove(
            StackSource::try_new(StackView::Inventory, 0).expect("legal quick move source"),
        ),
        Command::DropStack(
            StackSource::try_new(StackView::Crafting, 44).expect("legal drop stack source"),
        ),
    ];
    assert_eq!(
        commands.len(),
        19,
        "the command enum has exactly 19 variants"
    );

    for left in 0..commands.len() {
        for right in (left + 1)..commands.len() {
            assert_ne!(
                commands[left], commands[right],
                "variants {left} and {right} collapsed onto one another"
            );
        }
    }
}

#[test]
fn command_inventory_chat_intent_retains_text_verbatim() {
    let intent = ChatIntent::try_new(
        CommandText::try_from_canonical("@Bob 停止".to_string())
            .expect("a bounded, trimmed command text is publishable"),
    )
    .unwrap();
    // The mention is retained byte for byte: this payload performs no
    // addressing, warp, stop or queue policy, and it carries no sequence, so a
    // replay sees exactly the text the player typed.
    assert_eq!(intent.text().as_str(), "@Bob 停止");

    // The text rule itself is the chat command's only payload bound.
    assert_eq!(
        CommandText::try_from_canonical(String::new()),
        Err(DomainError::InvalidText)
    );
    assert_eq!(
        CommandText::try_from_canonical(" mine ".to_string()),
        Err(DomainError::InvalidText)
    );
    assert_eq!(
        CommandText::try_from_canonical("mi\u{0001}ne".to_string()),
        Err(DomainError::InvalidText)
    );
    let oversized = "x".repeat(1025);
    assert_eq!(
        CommandText::try_from_canonical(oversized),
        Err(DomainError::InvalidText)
    );
}
