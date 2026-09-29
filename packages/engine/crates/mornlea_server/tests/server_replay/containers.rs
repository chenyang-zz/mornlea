//! Replay cases for the container view provider.
//!
//! Every expected value mirrors a frozen Go oracle row, cited at each case:
//! the unified view bounds from `packages/shared/core/chest.go` (inventory
//! `0..35`, chest `36..62`) and `packages/shared/core/furnace.go` (inventory
//! `0..35`, input `36`, fuel `37`, output `38`), the value-copy settlement
//! from `moveChestStack` / `moveFurnaceStack` plus the partial variants in
//! `packages/server/sim/entity/container.go` and `furnace.go`, the quick-move
//! target order in `packages/server/sim/entity/quick_move.go` (pickup phases
//! into the backpack, chest ascending fit, furnace input-before-fuel), the
//! bench repack gate `canRepackCrafting` in
//! `packages/server/sim/entity/crafting.go`, the view and generation gate in
//! `applyContainerMove` (`session.viewContainer`, exact reference match), and
//! the runtime rows in `packages/server/sim/runtime/chest_inventory_test.go`
//! and `furnace_inventory_test.go`. No case chooses a value the oracle does
//! not pin.

use std::collections::BTreeMap;

use super::*;
use mornlea_domain::{
    BlockPos, ChunkPos, Command, CommandEnvelope, CommandEnvelopeParts, ContainerKind,
    ContainerMove, ContainerRef, Dimension, FiniteVec3, LookAngles, MotionState, MotionStateParts,
    PartialMove, PlayerId, StackSource, StackView, SurvivalState, SurvivalStateParts,
};
use mornlea_protocol::{LoginStart, admit_login};
use mornlea_server::contracts::{
    ActorBody, ActorLifecycle, ActorRecord, BlockObservation, ChunkKey, ContainerRecord,
    ContainerSlots, EnvironmentState, FixtureState, InventoryRecord, RuleEffect, RulePhase,
    RuleTunables, SessionKey, SleepState, TransportKind, WorkState,
};
use mornlea_server::rules::containers as provider;
use mornlea_storage::{ItemStack, PlayerLocation, PlayerSave};

// Stable block numbers, mirrored from the frozen const block in
// `packages/shared/core/block.go`.
const AIR: u16 = 0; // `core.AirID`
const FURNACE_BLOCK: u16 = 9; // `core.FurnaceID`
const CHEST_BLOCK: u16 = 11; // `core.ChestID`
const WORKBENCH_BLOCK: u16 = 45; // `core.WorkbenchID`

// Stable item numbers, mirrored from the frozen const block in
// `packages/shared/core/item.go`.
const ITEM_STONE: u16 = 1; // `core.ItemStone`
const ITEM_DIRT: u16 = 2; // `core.ItemDirt`
const ITEM_COAL: u16 = 5; // `core.ItemCoal`
const ITEM_RAW_IRON: u16 = 6; // `core.ItemRawIron`
const ITEM_IRON_INGOT: u16 = 7; // `core.ItemIronIngot`
const ITEM_SAND: u16 = 18; // `core.ItemSand`

/// Mints one session identity through the real admission path. Session keys
/// are process-local nonzero ids, so a key minted on a throwaway authority is
/// a valid fixture identity for the replay authority.
fn player_session(tag: u8, name: &str) -> SessionKey {
    let mut bytes = [0u8; 16];
    bytes[0] = tag.max(1);
    bytes[6] = 0x40;
    bytes[8] = 0x80;
    let id = PlayerId::try_from_bytes(bytes).expect("player id");
    let start = LoginStart::new(id, name, 8).expect("login start");
    let inbound = LoginStart::decode_inbound(&start.encode().expect("encoded")).expect("inbound");
    let login = admit_login(inbound).expect("admitted");
    let mut mint = AuthorityState::try_new(limits(), 0).expect("authority");
    mint.admit(login, TransportKind::Memory).expect("session")
}

fn stack(item: u16, count: u8) -> ItemStack {
    ItemStack {
        item,
        count,
        durability: 0,
    }
}

fn uuid(tag: u8) -> [u8; 16] {
    let mut bytes = [0u8; 16];
    bytes[0] = tag.max(1);
    bytes[6] = 0x40;
    bytes[8] = 0x80;
    bytes
}

fn environment() -> EnvironmentState {
    EnvironmentState {
        seed: 7,
        next_tick: 0,
        world_time: 0,
        day_phase_offset: 0,
        season_offset: 0,
        weather: mornlea_domain::Weather::Clear,
        weather_remaining: 0,
        difficulty: 0,
        tunables: RuleTunables::source_defaults(),
    }
}

fn player_body(position: [f32; 3]) -> PlayerSave {
    PlayerSave {
        player_id: mornlea_storage::PlayerId::from_bytes(uuid(1)),
        revision: 1,
        display_name: "Tester".to_owned(),
        current: PlayerLocation {
            dimension: 0,
            position,
        },
        yaw: 0.0,
        pitch: 0.0,
        safe: None,
        inventory: mornlea_storage::Inventory::default(),
        health: 20,
        hunger: 20,
        saturation_milli: 5_000,
        exhaustion_milli: 0,
        respawn_present: false,
        respawn_position: [0.0, 0.0, 0.0],
        respawn_dimension: 0,
        armor: [ItemStack::default(); 4],
    }
}

fn player_actor(session: SessionKey, position: [f32; 3], yaw: f32, pitch: f32) -> ActorRecord {
    ActorRecord::try_new(
        ActorKey::Player(session),
        ActorLifecycle::Active,
        Dimension::OVERWORLD,
        MotionState::new(MotionStateParts {
            position: FiniteVec3::try_new(position).expect("position"),
            velocity: FiniteVec3::try_new([0.0, 0.0, 0.0]).expect("velocity"),
            on_ground: true,
        }),
        LookAngles::try_new(yaw, pitch).expect("look"),
        SurvivalState::try_new(SurvivalStateParts {
            health: 20,
            oxygen: 300,
            hunger: 20,
            saturation_zero: false,
            armor_points: 0,
        })
        .expect("survival"),
        ActorBody::Player(player_body(position)),
    )
    .expect("player actor")
}

fn overworld_key(pos: BlockPos) -> ChunkKey {
    ChunkKey {
        dimension: Dimension::OVERWORLD,
        pos: ChunkPos::new(pos.x() >> 4, pos.z() >> 4),
    }
}

fn observation(pos: BlockPos, block: u16) -> BlockObservation {
    BlockObservation::try_new(overworld_key(pos), 1, 1, pos, block).expect("block observation")
}

fn chest_ref(slot: u8, generation: u32) -> ContainerRef {
    ContainerRef::try_new(ChunkPos::new(0, 0), ContainerKind::Chest, slot, generation)
        .expect("chest reference")
}

fn furnace_ref(slot: u8, generation: u32) -> ContainerRef {
    ContainerRef::try_new(
        ChunkPos::new(0, 0),
        ContainerKind::Furnace,
        slot,
        generation,
    )
    .expect("furnace reference")
}

fn chest_record(slot: u8, generation: u32, items: &[(usize, ItemStack)]) -> ContainerRecord {
    let mut cells = [ItemStack::default(); 27];
    for (index, held) in items {
        cells[*index] = *held;
    }
    ContainerRecord {
        reference: chest_ref(slot, generation),
        revision: 1,
        slots: ContainerSlots::Chest(cells),
    }
}

fn furnace_record(
    slot: u8,
    generation: u32,
    input: ItemStack,
    fuel: ItemStack,
    output: ItemStack,
    progress: u32,
) -> ContainerRecord {
    ContainerRecord {
        reference: furnace_ref(slot, generation),
        revision: 1,
        slots: ContainerSlots::Furnace {
            slots: [input, fuel, output],
            fuel: 0,
            progress,
        },
    }
}

fn envelope(session: SessionKey, sequence: u64, command: Command) -> CommandEnvelope {
    CommandEnvelope::try_new(CommandEnvelopeParts {
        tick: 0,
        session: session.get(),
        sequence,
        arrival_index: 0,
        command,
    })
    .expect("envelope")
}

/// Horizontal look toward negative Z, the open intent the envelope carries:
/// look only, never a target container or revision.
fn open_command() -> Command {
    Command::OpenContainer(LookAngles::try_new(0.0, 0.0).expect("look"))
}

fn admit(
    context: &mut TickContext<'_>,
    envelope: &CommandEnvelope,
) -> Result<PhaseReport, ServerError> {
    provider::run(
        context,
        RuleCall {
            phase: RulePhase::PlayerCommand,
            actor: None,
            command: Some(envelope),
            internal: None,
        },
    )
}

/// The single settlement pass over the deferred container queue, in admission
/// order. Production calls this once per tick with a fresh staging context.
fn drain(context: &mut TickContext<'_>) -> Result<PhaseReport, ServerError> {
    provider::run(
        context,
        RuleCall {
            phase: RulePhase::ContainerMove,
            actor: None,
            command: None,
            internal: None,
        },
    )
}

/// One viewer scene: an active player at `[0.5, 64.0, 0.5]` looking toward
/// negative Z with the eye cell staged as air and the aimed cell staged by
/// the caller, plus the actor's inventory.
fn viewer_scene(
    context: &mut TickContext<'_>,
    session: SessionKey,
    aimed: (BlockPos, u16),
    slots: &[(usize, ItemStack)],
) -> ActorKey {
    let actor = ActorKey::Player(session);
    context
        .stage(RuleEffect::Environment(environment()))
        .expect("environment");
    context
        .stage(RuleEffect::Actor(player_actor(
            session,
            [0.5, 64.0, 0.5],
            0.0,
            0.0,
        )))
        .expect("actor");
    let mut inventory = InventoryRecord::empty();
    for (index, held) in slots {
        inventory.slots[*index] = *held;
    }
    context.preload_inventory(actor, inventory);
    context.preload_block(observation(BlockPos::new(0, 65, 0), AIR));
    context.preload_block(observation(aimed.0, aimed.1));
    actor
}

fn inventory_of(context: &TickContext<'_>, actor: ActorKey) -> InventoryRecord {
    context
        .read()
        .inventory(actor)
        .copied()
        .expect("inventory staged")
}

fn container_of(context: &TickContext<'_>, reference: ContainerRef) -> ContainerRecord {
    context
        .read()
        .container(reference)
        .expect("container staged")
}

fn container_cells(container: &ContainerRecord) -> Vec<ItemStack> {
    match &container.slots {
        ContainerSlots::Chest(cells) => cells.to_vec(),
        ContainerSlots::Furnace { slots, .. } => slots.to_vec(),
    }
}

/// The conservation invariant: the multiset of item counts keyed by item,
/// count and durability over the player slots plus the container cells. A
/// settlement may regroup stacks but never mints or destroys a count; a
/// refusal keeps every cell identical.
fn item_totals(
    inventory: &InventoryRecord,
    container: &ContainerRecord,
) -> BTreeMap<(u16, u16), u64> {
    let mut totals: BTreeMap<(u16, u16), u64> = BTreeMap::new();
    for held in inventory
        .slots
        .iter()
        .chain(container_cells(container).iter())
    {
        if held.item != 0 {
            *totals.entry((held.item, held.durability)).or_insert(0) += u64::from(held.count);
        }
    }
    totals.into_iter().collect()
}

/// A no-settlement observation for the same fixture state, the baseline
/// `assert_no_effect` compares a refusal against.
fn baseline_observed(state: &FixtureState) -> Observed {
    Observed {
        events: Vec::new(),
        counters: TickCounters::default(),
        state_sha256: canonical_state_sha256(state),
        class: None,
    }
}

#[test]
fn late_close_and_generation() {
    let session = player_session(11, "late-close");
    let target = BlockPos::new(0, 65, -1);

    // A delayed move that arrives after the close settles against a cleared
    // view, so the drain refuses it with every cell unchanged
    // (`applyContainerMove` requires `session.viewContainer`, and the Go close
    // row `CommandCloseFurnace` in `packages/server/sim/entity/tick.go`
    // clears it).
    let mut state = AuthorityState::try_new(limits(), 7).expect("authority");
    let mut context = TickContext::harness(&mut state, TickBudget::full());
    let actor = viewer_scene(
        &mut context,
        session,
        (target, FURNACE_BLOCK),
        &[(0, stack(ITEM_DIRT, 10))],
    );
    let reference = furnace_ref(0, 1);
    context.preload_container(furnace_record(
        0,
        1,
        ItemStack::default(),
        ItemStack::default(),
        ItemStack::default(),
        0,
    ));
    let before_inventory = inventory_of(&context, actor);
    let before_container = container_of(&context, reference);
    let before_totals = item_totals(&before_inventory, &before_container);
    assert!(admit(&mut context, &envelope(session, 1, open_command())).is_ok());
    assert!(admit(&mut context, &envelope(session, 2, Command::CloseContainer),).is_ok());
    assert!(
        admit(
            &mut context,
            &envelope(
                session,
                3,
                Command::MoveContainer(
                    ContainerMove::try_new(reference.chunk(), ContainerKind::Furnace, 0, 1, 0, 36,)
                        .expect("move"),
                ),
            ),
        )
        .is_ok()
    );
    let report = drain(&mut context).expect("drain settles the queue");
    assert_eq!(report.examined, 3);
    assert_eq!(report.applied, 2);
    assert_eq!(report.rejected, 1);
    let after_inventory = inventory_of(&context, actor);
    let after_container = container_of(&context, reference);
    assert_eq!(after_inventory, before_inventory);
    assert_eq!(after_container, before_container);
    assert_eq!(
        item_totals(&after_inventory, &after_container),
        before_totals
    );
    assert!(context.events().is_empty());

    // The same deferred move against a replaced generation refuses: the slot
    // was reused and its generation advanced, so the old reference no longer
    // names it (`furnaceView` / `chestView` generation mismatch in
    // `packages/server/sim/entity/container.go` and `furnace.go`, pinned by
    // `TestMoveRejectsStaleFurnaceReference` and
    // `TestMoveRejectsStaleChestReference`).
    let mut state = AuthorityState::try_new(limits(), 7).expect("authority");
    let mut context = TickContext::harness(&mut state, TickBudget::full());
    let actor = viewer_scene(
        &mut context,
        session,
        (target, FURNACE_BLOCK),
        &[(0, stack(ITEM_DIRT, 10))],
    );
    context.preload_container(furnace_record(
        0,
        1,
        ItemStack::default(),
        ItemStack::default(),
        ItemStack::default(),
        0,
    ));
    let stale = ContainerMove::try_new(reference.chunk(), ContainerKind::Furnace, 0, 1, 0, 36)
        .expect("move");
    assert!(admit(&mut context, &envelope(session, 1, open_command())).is_ok());
    assert!(
        admit(
            &mut context,
            &envelope(session, 2, Command::MoveContainer(stale)),
        )
        .is_ok()
    );
    context.preload_container(furnace_record(
        0,
        2,
        ItemStack::default(),
        ItemStack::default(),
        ItemStack::default(),
        0,
    ));
    let report = drain(&mut context).expect("drain settles the queue");
    assert_eq!(report.examined, 2);
    assert_eq!(report.applied, 1);
    assert_eq!(report.rejected, 1);
    let after_inventory = inventory_of(&context, actor);
    assert_eq!(after_inventory.slots[0], stack(ITEM_DIRT, 10));
    let current = container_of(&context, furnace_ref(0, 2));
    assert_eq!(container_cells(&current)[0], ItemStack::default());
    assert_eq!(
        current,
        furnace_record(
            0,
            2,
            ItemStack::default(),
            ItemStack::default(),
            ItemStack::default(),
            0,
        )
    );
    assert_eq!(
        item_totals(&after_inventory, &current),
        item_totals(&before_inventory, &before_container)
    );

    // An open aimed at a workbench never reaches the container drain: the
    // workbench is an ordinary block rather than a container, so its opens
    // route through the crafting node (`openContainer` workbench arm in
    // `packages/server/sim/entity/container.go`).
    let mut state = AuthorityState::try_new(limits(), 7).expect("authority");
    let mut context = TickContext::harness(&mut state, TickBudget::full());
    viewer_scene(
        &mut context,
        session,
        (target, WORKBENCH_BLOCK),
        &[(0, stack(ITEM_DIRT, 10))],
    );
    assert!(admit(&mut context, &envelope(session, 1, open_command())).is_ok());
    let report = drain(&mut context).expect("drain settles the queue");
    assert_eq!(report.examined, 1);
    assert_eq!(report.applied, 0);
    assert_eq!(report.rejected, 1);

    // An open with no container in reach refuses the same way
    // (`RejectNoTarget` in the same Go open row).
    let mut state = AuthorityState::try_new(limits(), 7).expect("authority");
    let mut context = TickContext::harness(&mut state, TickBudget::full());
    viewer_scene(
        &mut context,
        session,
        (target, AIR),
        &[(0, stack(ITEM_DIRT, 10))],
    );
    assert!(admit(&mut context, &envelope(session, 1, open_command())).is_ok());
    let report = drain(&mut context).expect("drain settles the queue");
    assert_eq!(report.examined, 1);
    assert_eq!(report.applied, 0);
    assert_eq!(report.rejected, 1);

    // The lease survives drains through the staging overlay: open and drain,
    // then move and drain again, then close and drain a third time. The
    // second drain settles the move against the still-open view and binds
    // the exact reference into the overlay; the third drain replays the
    // settled move idempotently (empty source, nothing staged) and clears
    // the lease on close; a fourth move then reads as closed. The
    // authority-commit leg that carries the overlay across ticks belongs to
    // the serial reducer integration, so this proof stays inside one tick
    // context on purpose.
    let mut state = AuthorityState::try_new(limits(), 7).expect("authority");
    let mut context = TickContext::harness(&mut state, TickBudget::full());
    let actor = viewer_scene(
        &mut context,
        session,
        (target, FURNACE_BLOCK),
        &[(0, stack(ITEM_RAW_IRON, 4))],
    );
    let reference = furnace_ref(0, 1);
    context.preload_container(furnace_record(
        0,
        1,
        ItemStack::default(),
        ItemStack::default(),
        ItemStack::default(),
        0,
    ));
    assert!(admit(&mut context, &envelope(session, 1, open_command())).is_ok());
    let report = drain(&mut context).expect("first drain");
    assert_eq!(
        (report.examined, report.applied, report.rejected),
        (1, 1, 0)
    );
    // The open stages no lease: authorization lives in the settled queue
    // history, and only the first settled move binds the reference.
    assert!(context.read().viewer(session).is_none());
    assert!(
        admit(
            &mut context,
            &envelope(
                session,
                2,
                Command::MoveContainer(
                    ContainerMove::try_new(reference.chunk(), ContainerKind::Furnace, 0, 1, 0, 36,)
                        .expect("move"),
                ),
            ),
        )
        .is_ok()
    );
    let report = drain(&mut context).expect("second drain");
    assert_eq!(
        (report.examined, report.applied, report.rejected),
        (2, 2, 0)
    );
    assert_eq!(inventory_of(&context, actor).slots[0], ItemStack::default());
    let settled_container = container_of(&context, reference);
    assert_eq!(
        settled_container,
        furnace_record(
            0,
            1,
            stack(ITEM_RAW_IRON, 4),
            ItemStack::default(),
            ItemStack::default(),
            0,
        )
    );
    let lease = context.read().viewer(session).expect("lease bound");
    assert_eq!(lease.session(), session);
    assert_eq!(lease.reference(), reference);
    assert!(admit(&mut context, &envelope(session, 3, Command::CloseContainer)).is_ok());
    let report = drain(&mut context).expect("third drain");
    assert_eq!(
        (report.examined, report.applied, report.rejected),
        (3, 2, 1)
    );
    assert!(context.read().viewer(session).is_none());
    assert_eq!(container_of(&context, reference), settled_container);
    // A move after the settled close reads as closed even with a live
    // source: the input cell still holds the dirt, so only the cleared
    // view can refuse this transfer.
    assert!(
        admit(
            &mut context,
            &envelope(
                session,
                4,
                Command::MoveContainer(
                    ContainerMove::try_new(reference.chunk(), ContainerKind::Furnace, 0, 1, 36, 1,)
                        .expect("move"),
                ),
            ),
        )
        .is_ok()
    );
    let report = drain(&mut context).expect("fourth drain");
    assert_eq!(
        (report.examined, report.applied, report.rejected),
        (4, 2, 2)
    );
    assert_eq!(container_of(&context, reference), settled_container);
    assert_eq!(inventory_of(&context, actor).slots[1], ItemStack::default());
}

#[test]
fn furnace_output_and_priority() {
    let session = player_session(12, "furnace-priority");
    let target = BlockPos::new(0, 65, -1);

    // The output slot is never a whole-move destination: the domain boundary
    // already refuses it (`ContainerMove::try_new` furnace-output arm, the
    // `FurnaceOutputAsTarget` rule), so no envelope can carry it.
    assert!(
        ContainerMove::try_new(ChunkPos::new(0, 0), ContainerKind::Furnace, 0, 1, 0, 38,).is_err()
    );

    // The output slot is never a partial-move destination either, even though
    // the protocol admits the index and leaves the judgment to the authority
    // (`moveFurnaceStackAmount` output-target refusal in
    // `packages/server/sim/entity/furnace.go`, pinned by
    // `TestMoveIntoOutputSlotIsRejected`).
    let mut state = AuthorityState::try_new(limits(), 7).expect("authority");
    let mut context = TickContext::harness(&mut state, TickBudget::full());
    let actor = viewer_scene(
        &mut context,
        session,
        (target, FURNACE_BLOCK),
        &[(0, stack(ITEM_DIRT, 5))],
    );
    let reference = furnace_ref(0, 1);
    context.preload_container(furnace_record(
        0,
        1,
        ItemStack::default(),
        ItemStack::default(),
        stack(ITEM_IRON_INGOT, 3),
        0,
    ));
    let before_inventory = inventory_of(&context, actor);
    let before_container = container_of(&context, reference);
    let partial_into_output =
        PartialMove::try_new(StackView::Container(reference), 0, 38, false).expect("partial");
    assert!(admit(&mut context, &envelope(session, 1, open_command()),).is_ok());
    assert!(
        admit(
            &mut context,
            &envelope(session, 2, Command::MovePartial(partial_into_output)),
        )
        .is_ok()
    );
    let report = drain(&mut context).expect("drain settles the queue");
    assert_eq!(report.applied, 1);
    assert_eq!(report.rejected, 1);
    assert_eq!(inventory_of(&context, actor), before_inventory);
    assert_eq!(container_of(&context, reference), before_container);

    // Quick-move takes the smeltable input first: raw iron lands in the input
    // slot even with the fuel slot empty (`quickMoveFurnaceStack` input step
    // in `packages/server/sim/entity/quick_move.go`).
    let mut state = AuthorityState::try_new(limits(), 7).expect("authority");
    let mut context = TickContext::harness(&mut state, TickBudget::full());
    let actor = viewer_scene(
        &mut context,
        session,
        (target, FURNACE_BLOCK),
        &[(0, stack(ITEM_RAW_IRON, 4))],
    );
    let reference = furnace_ref(0, 1);
    context.preload_container(furnace_record(
        0,
        1,
        ItemStack::default(),
        ItemStack::default(),
        ItemStack::default(),
        0,
    ));
    let start = (
        inventory_of(&context, actor),
        container_of(&context, reference),
    );
    assert!(admit(&mut context, &envelope(session, 1, open_command())).is_ok());
    assert!(
        admit(
            &mut context,
            &envelope(
                session,
                2,
                Command::QuickMove(
                    StackSource::try_new(StackView::Container(reference), 0).expect("source"),
                ),
            ),
        )
        .is_ok()
    );
    let report = drain(&mut context).expect("drain settles the queue");
    assert_eq!((report.applied, report.rejected), (2, 0));
    let after_inventory = inventory_of(&context, actor);
    let after_container = container_of(&context, reference);
    assert_eq!(after_inventory.slots[0], ItemStack::default());
    assert_eq!(
        container_cells(&after_container)[0],
        stack(ITEM_RAW_IRON, 4)
    );
    assert_eq!(
        after_container,
        furnace_record(
            0,
            1,
            stack(ITEM_RAW_IRON, 4),
            ItemStack::default(),
            ItemStack::default(),
            0,
        )
    );
    assert_eq!(
        item_totals(&after_inventory, &after_container),
        item_totals(&start.0, &start.1)
    );

    // Quick-move takes coal fuel second: coal lands in the fuel slot, never
    // the output (`quickMoveFurnaceStack` fuel step in the same file).
    let mut state = AuthorityState::try_new(limits(), 7).expect("authority");
    let mut context = TickContext::harness(&mut state, TickBudget::full());
    let actor = viewer_scene(
        &mut context,
        session,
        (target, FURNACE_BLOCK),
        &[(0, stack(ITEM_COAL, 3))],
    );
    let reference = furnace_ref(0, 1);
    context.preload_container(furnace_record(
        0,
        1,
        ItemStack::default(),
        ItemStack::default(),
        ItemStack::default(),
        0,
    ));
    assert!(admit(&mut context, &envelope(session, 1, open_command())).is_ok());
    assert!(
        admit(
            &mut context,
            &envelope(
                session,
                2,
                Command::QuickMove(
                    StackSource::try_new(StackView::Container(reference), 0).expect("source"),
                ),
            ),
        )
        .is_ok()
    );
    let report = drain(&mut context).expect("drain settles the queue");
    assert_eq!((report.applied, report.rejected), (2, 0));
    let after_inventory = inventory_of(&context, actor);
    let after_container = container_of(&context, reference);
    assert_eq!(after_inventory.slots[0], ItemStack::default());
    assert_eq!(container_cells(&after_container)[1], stack(ITEM_COAL, 3));
    assert_eq!(container_cells(&after_container)[2], ItemStack::default());

    // The output is only ever a quick-move source: the smelted ingots credit
    // into the backpack with the remainder kept at the source.
    let mut state = AuthorityState::try_new(limits(), 7).expect("authority");
    let mut context = TickContext::harness(&mut state, TickBudget::full());
    let actor = viewer_scene(&mut context, session, (target, FURNACE_BLOCK), &[]);
    let reference = furnace_ref(0, 1);
    context.preload_container(furnace_record(
        0,
        1,
        ItemStack::default(),
        ItemStack::default(),
        stack(ITEM_IRON_INGOT, 3),
        0,
    ));
    assert!(admit(&mut context, &envelope(session, 1, open_command())).is_ok());
    assert!(
        admit(
            &mut context,
            &envelope(
                session,
                2,
                Command::QuickMove(
                    StackSource::try_new(StackView::Container(reference), 38).expect("source"),
                ),
            ),
        )
        .is_ok()
    );
    let report = drain(&mut context).expect("drain settles the queue");
    assert_eq!((report.applied, report.rejected), (2, 0));
    assert_eq!(
        inventory_of(&context, actor).slots[0],
        stack(ITEM_IRON_INGOT, 3)
    );
    assert_eq!(
        container_cells(&container_of(&context, reference))[2],
        ItemStack::default()
    );

    // Coal is not a smelting input, so a whole move into the input slot
    // refuses with both sides unchanged (`setFurnaceViewSlot` input arm in
    // `packages/server/sim/entity/furnace.go`, pinned by
    // `TestMoveIntoFurnaceMaterialsAcceptOnlyAllowedItems`).
    let mut state = AuthorityState::try_new(limits(), 7).expect("authority");
    let mut context = TickContext::harness(&mut state, TickBudget::full());
    let actor = viewer_scene(
        &mut context,
        session,
        (target, FURNACE_BLOCK),
        &[(0, stack(ITEM_COAL, 3))],
    );
    let reference = furnace_ref(0, 1);
    context.preload_container(furnace_record(
        0,
        1,
        ItemStack::default(),
        ItemStack::default(),
        ItemStack::default(),
        0,
    ));
    let before_inventory = inventory_of(&context, actor);
    let before_container = container_of(&context, reference);
    assert!(admit(&mut context, &envelope(session, 1, open_command())).is_ok());
    assert!(
        admit(
            &mut context,
            &envelope(
                session,
                2,
                Command::MoveContainer(
                    ContainerMove::try_new(reference.chunk(), ContainerKind::Furnace, 0, 1, 0, 36,)
                        .expect("move"),
                ),
            ),
        )
        .is_ok()
    );
    let report = drain(&mut context).expect("drain settles the queue");
    assert_eq!((report.applied, report.rejected), (1, 1));
    assert_eq!(inventory_of(&context, actor), before_inventory);
    assert_eq!(container_of(&context, reference), before_container);

    // Swapping the input kind resets the smelt progress: sand burns with
    // progress 7, raw iron swaps in whole, and the progress returns to zero
    // (`setFurnaceViewSlot` progress reset in the same file, pinned by
    // `TestFurnaceInputKindSwitchResetsProgress`).
    let mut state = AuthorityState::try_new(limits(), 7).expect("authority");
    let mut context = TickContext::harness(&mut state, TickBudget::full());
    let actor = viewer_scene(
        &mut context,
        session,
        (target, FURNACE_BLOCK),
        &[(0, stack(ITEM_RAW_IRON, 2))],
    );
    let reference = furnace_ref(0, 1);
    context.preload_container(furnace_record(
        0,
        1,
        stack(ITEM_SAND, 5),
        ItemStack::default(),
        ItemStack::default(),
        7,
    ));
    assert!(admit(&mut context, &envelope(session, 1, open_command())).is_ok());
    assert!(
        admit(
            &mut context,
            &envelope(
                session,
                2,
                Command::MoveContainer(
                    ContainerMove::try_new(reference.chunk(), ContainerKind::Furnace, 0, 1, 0, 36,)
                        .expect("move"),
                ),
            ),
        )
        .is_ok()
    );
    let report = drain(&mut context).expect("drain settles the queue");
    assert_eq!((report.applied, report.rejected), (2, 0));
    assert_eq!(inventory_of(&context, actor).slots[0], stack(ITEM_SAND, 5));
    let after_container = container_of(&context, reference);
    assert_eq!(
        container_cells(&after_container)[0],
        stack(ITEM_RAW_IRON, 2)
    );
    assert_eq!(
        after_container,
        furnace_record(
            0,
            1,
            stack(ITEM_RAW_IRON, 2),
            ItemStack::default(),
            ItemStack::default(),
            0,
        )
    );
    let ContainerSlots::Furnace { progress, .. } = after_container.slots else {
        panic!("furnace slots keep their shape");
    };
    assert_eq!(progress, 0);
}

#[test]
fn partial_absorb() {
    let session = player_session(13, "partial-absorb");
    let target = BlockPos::new(0, 65, -1);

    // Half of 7 is the ceiling half 4, leaving 3 behind: the amount derives
    // from the settlement-time source stack (`stackSplitAmount` in
    // `packages/server/sim/entity/container.go`, pinned by
    // `TestStackSplitChestHalfIntoChestRegion`).
    let mut state = AuthorityState::try_new(limits(), 7).expect("authority");
    let mut context = TickContext::harness(&mut state, TickBudget::full());
    let actor = viewer_scene(
        &mut context,
        session,
        (target, CHEST_BLOCK),
        &[(0, stack(ITEM_DIRT, 7))],
    );
    let reference = chest_ref(0, 1);
    context.preload_container(chest_record(0, 1, &[]));
    let start = (
        inventory_of(&context, actor),
        container_of(&context, reference),
    );
    assert!(admit(&mut context, &envelope(session, 1, open_command())).is_ok());
    assert!(
        admit(
            &mut context,
            &envelope(
                session,
                2,
                Command::MovePartial(
                    PartialMove::try_new(StackView::Container(reference), 0, 36, false)
                        .expect("partial"),
                ),
            ),
        )
        .is_ok()
    );
    let report = drain(&mut context).expect("drain settles the queue");
    assert_eq!((report.applied, report.rejected), (2, 0));
    let after_inventory = inventory_of(&context, actor);
    let after_container = container_of(&context, reference);
    assert_eq!(after_inventory.slots[0], stack(ITEM_DIRT, 3));
    assert_eq!(container_cells(&after_container)[0], stack(ITEM_DIRT, 4));
    assert_eq!(
        item_totals(&after_inventory, &after_container),
        item_totals(&start.0, &start.1)
    );

    // A half into a nearly-full same item truncates to the remaining capacity
    // and keeps the remainder at the source (pinned by
    // `TestStackSplitChestSingleIntoNearlyFullChestSlot`).
    let mut state = AuthorityState::try_new(limits(), 7).expect("authority");
    let mut context = TickContext::harness(&mut state, TickBudget::full());
    let actor = viewer_scene(
        &mut context,
        session,
        (target, CHEST_BLOCK),
        &[(0, stack(ITEM_DIRT, 7))],
    );
    let reference = chest_ref(0, 1);
    context.preload_container(chest_record(0, 1, &[(0, stack(ITEM_DIRT, 63))]));
    let start = (
        inventory_of(&context, actor),
        container_of(&context, reference),
    );
    assert!(admit(&mut context, &envelope(session, 1, open_command())).is_ok());
    assert!(
        admit(
            &mut context,
            &envelope(
                session,
                2,
                Command::MovePartial(
                    PartialMove::try_new(StackView::Container(reference), 0, 36, false)
                        .expect("partial"),
                ),
            ),
        )
        .is_ok()
    );
    let report = drain(&mut context).expect("drain settles the queue");
    assert_eq!((report.applied, report.rejected), (2, 0));
    let after_inventory = inventory_of(&context, actor);
    let after_container = container_of(&context, reference);
    assert_eq!(after_inventory.slots[0], stack(ITEM_DIRT, 6));
    assert_eq!(container_cells(&after_container)[0], stack(ITEM_DIRT, 64));
    assert_eq!(
        item_totals(&after_inventory, &after_container),
        item_totals(&start.0, &start.1)
    );

    // The single flag moves exactly one item out of the chest and keeps the
    // remainder in place (pinned by
    // `TestStackSplitChestPartialOutOfChestKeepsRemainder`).
    let mut state = AuthorityState::try_new(limits(), 7).expect("authority");
    let mut context = TickContext::harness(&mut state, TickBudget::full());
    let actor = viewer_scene(&mut context, session, (target, CHEST_BLOCK), &[]);
    let reference = chest_ref(0, 1);
    context.preload_container(chest_record(0, 1, &[(0, stack(ITEM_DIRT, 7))]));
    let start = (
        inventory_of(&context, actor),
        container_of(&context, reference),
    );
    assert!(admit(&mut context, &envelope(session, 1, open_command())).is_ok());
    assert!(
        admit(
            &mut context,
            &envelope(
                session,
                2,
                Command::MovePartial(
                    PartialMove::try_new(StackView::Container(reference), 36, 0, true)
                        .expect("partial"),
                ),
            ),
        )
        .is_ok()
    );
    let report = drain(&mut context).expect("drain settles the queue");
    assert_eq!((report.applied, report.rejected), (2, 0));
    let after_inventory = inventory_of(&context, actor);
    let after_container = container_of(&context, reference);
    assert_eq!(after_inventory.slots[0], stack(ITEM_DIRT, 1));
    assert_eq!(container_cells(&after_container)[0], stack(ITEM_DIRT, 6));
    assert_eq!(
        item_totals(&after_inventory, &after_container),
        item_totals(&start.0, &start.1)
    );

    // An unlike non-empty target refuses the whole partial move: a partial
    // move never swaps (`mergeStacksAmount` unlike refusal in
    // `packages/server/sim/entity/container.go`, pinned by
    // `TestStackSplitChestRejectsDifferentItemTargetWithoutSwap`).
    let mut state = AuthorityState::try_new(limits(), 7).expect("authority");
    let mut context = TickContext::harness(&mut state, TickBudget::full());
    let actor = viewer_scene(
        &mut context,
        session,
        (target, CHEST_BLOCK),
        &[(0, stack(ITEM_DIRT, 5))],
    );
    let reference = chest_ref(0, 1);
    context.preload_container(chest_record(0, 1, &[(0, stack(ITEM_STONE, 4))]));
    let before_inventory = inventory_of(&context, actor);
    let before_container = container_of(&context, reference);
    assert!(admit(&mut context, &envelope(session, 1, open_command())).is_ok());
    assert!(
        admit(
            &mut context,
            &envelope(
                session,
                2,
                Command::MovePartial(
                    PartialMove::try_new(StackView::Container(reference), 0, 36, false)
                        .expect("partial"),
                ),
            ),
        )
        .is_ok()
    );
    let report = drain(&mut context).expect("drain settles the queue");
    assert_eq!((report.applied, report.rejected), (1, 1));
    assert_eq!(inventory_of(&context, actor), before_inventory);
    assert_eq!(container_of(&context, reference), before_container);

    // An empty source derives no amount and refuses with zero change (pinned
    // by the empty-source rows of the same Go test family).
    let mut state = AuthorityState::try_new(limits(), 7).expect("authority");
    let mut context = TickContext::harness(&mut state, TickBudget::full());
    let actor = viewer_scene(&mut context, session, (target, CHEST_BLOCK), &[]);
    let reference = chest_ref(0, 1);
    context.preload_container(chest_record(0, 1, &[]));
    let before_inventory = inventory_of(&context, actor);
    let before_container = container_of(&context, reference);
    assert!(admit(&mut context, &envelope(session, 1, open_command())).is_ok());
    assert!(
        admit(
            &mut context,
            &envelope(
                session,
                2,
                Command::MovePartial(
                    PartialMove::try_new(StackView::Container(reference), 0, 36, false)
                        .expect("partial"),
                ),
            ),
        )
        .is_ok()
    );
    let report = drain(&mut context).expect("drain settles the queue");
    assert_eq!((report.applied, report.rejected), (1, 1));
    assert_eq!(inventory_of(&context, actor), before_inventory);
    assert_eq!(container_of(&context, reference), before_container);
}

#[test]
fn stale_view_and_unauthorized() {
    let session = player_session(14, "stale-view");
    let other = player_session(15, "other-viewer");
    let target = BlockPos::new(0, 65, -1);

    // A move naming a retired generation refuses with every cell unchanged
    // (the `chestView` generation mismatch row in
    // `packages/server/sim/entity/container.go`, pinned by
    // `TestMoveRejectsStaleChestReference`).
    let mut state = AuthorityState::try_new(limits(), 7).expect("authority");
    let mut context = TickContext::harness(&mut state, TickBudget::full());
    let actor = viewer_scene(
        &mut context,
        session,
        (target, CHEST_BLOCK),
        &[(0, stack(ITEM_DIRT, 5))],
    );
    let live = chest_ref(0, 1);
    context.preload_container(chest_record(0, 1, &[]));
    let before_inventory = inventory_of(&context, actor);
    let before_container = container_of(&context, live);
    let retired = chest_ref(0, 9);
    assert!(admit(&mut context, &envelope(session, 1, open_command())).is_ok());
    assert!(
        admit(
            &mut context,
            &envelope(
                session,
                2,
                Command::MoveContainer(
                    ContainerMove::try_new(retired.chunk(), ContainerKind::Chest, 0, 9, 0, 36,)
                        .expect("move"),
                ),
            ),
        )
        .is_ok()
    );
    let report = drain(&mut context).expect("drain settles the queue");
    assert_eq!((report.applied, report.rejected), (1, 1));
    let after_inventory = inventory_of(&context, actor);
    assert_eq!(after_inventory, before_inventory);
    assert_eq!(container_of(&context, live), before_container);
    assert_eq!(
        item_totals(&after_inventory, &before_container),
        item_totals(&before_inventory, &before_container)
    );

    // A move with no open view refuses: the session never established the
    // viewed container the move names (`!session.viewContainer` in
    // `applyContainerMove`, pinned by
    // `TestStackSplitChestRejectsWhenNotViewing`).
    let mut state = AuthorityState::try_new(limits(), 7).expect("authority");
    let mut context = TickContext::harness(&mut state, TickBudget::full());
    let actor = viewer_scene(
        &mut context,
        session,
        (target, CHEST_BLOCK),
        &[(0, stack(ITEM_DIRT, 5))],
    );
    let live = chest_ref(0, 1);
    context.preload_container(chest_record(0, 1, &[]));
    let before_inventory = inventory_of(&context, actor);
    let before_container = container_of(&context, live);
    assert!(
        admit(
            &mut context,
            &envelope(
                session,
                2,
                Command::MoveContainer(
                    ContainerMove::try_new(live.chunk(), ContainerKind::Chest, 0, 1, 0, 36,)
                        .expect("move"),
                ),
            ),
        )
        .is_ok()
    );
    let report = drain(&mut context).expect("drain settles the queue");
    assert_eq!((report.applied, report.rejected), (0, 1));
    assert_eq!(inventory_of(&context, actor), before_inventory);
    assert_eq!(container_of(&context, live), before_container);

    // A move naming a live container the session did not open refuses as
    // well: the first settled view binds the viewer, and a different
    // reference is not that view (`session.container != command.Furnace` in
    // the same Go gate).
    let mut state = AuthorityState::try_new(limits(), 7).expect("authority");
    let mut context = TickContext::harness(&mut state, TickBudget::full());
    let actor = viewer_scene(
        &mut context,
        session,
        (target, CHEST_BLOCK),
        &[(0, stack(ITEM_DIRT, 5))],
    );
    let first = chest_ref(0, 1);
    let second = chest_ref(1, 1);
    context.preload_container(chest_record(0, 1, &[]));
    context.preload_container(chest_record(1, 1, &[]));
    let before_inventory = inventory_of(&context, actor);
    let before_first = container_of(&context, first);
    assert!(admit(&mut context, &envelope(session, 1, open_command())).is_ok());
    assert!(
        admit(
            &mut context,
            &envelope(
                session,
                2,
                Command::MoveContainer(
                    ContainerMove::try_new(first.chunk(), ContainerKind::Chest, 0, 1, 0, 36,)
                        .expect("move"),
                ),
            ),
        )
        .is_ok()
    );
    assert!(
        admit(
            &mut context,
            &envelope(
                session,
                3,
                Command::MoveContainer(
                    ContainerMove::try_new(second.chunk(), ContainerKind::Chest, 1, 1, 0, 36,)
                        .expect("move"),
                ),
            ),
        )
        .is_ok()
    );
    let report = drain(&mut context).expect("drain settles the queue");
    assert_eq!(report.examined, 3);
    assert_eq!(report.applied, 2);
    assert_eq!(report.rejected, 1);
    let after_inventory = inventory_of(&context, actor);
    let after_first = container_of(&context, first);
    assert_eq!(after_inventory.slots[0], ItemStack::default());
    assert_eq!(container_cells(&after_first)[0], stack(ITEM_DIRT, 5));
    assert_eq!(after_first, chest_record(0, 1, &[(0, stack(ITEM_DIRT, 5))]));
    assert_eq!(
        container_cells(&container_of(&context, second))[0],
        ItemStack::default()
    );
    assert_eq!(after_inventory.slots[1], ItemStack::default());
    assert_eq!(
        item_totals(&after_inventory, &after_first),
        item_totals(&before_inventory, &before_first)
    );

    // A merge into a full destination refuses with both sides unchanged
    // (the at-cap refusal of `mergeStacks`, pinned by
    // `TestMoveIntoChestFailsToMergeAtStackLimit`); the same move into a
    // nearly-full cell merges and keeps the remainder.
    let mut state = AuthorityState::try_new(limits(), 7).expect("authority");
    let mut context = TickContext::harness(&mut state, TickBudget::full());
    let actor = viewer_scene(
        &mut context,
        session,
        (target, CHEST_BLOCK),
        &[(0, stack(ITEM_DIRT, 5))],
    );
    let live = chest_ref(0, 1);
    context.preload_container(chest_record(0, 1, &[(0, stack(ITEM_DIRT, 64))]));
    let before_inventory = inventory_of(&context, actor);
    let before_container = container_of(&context, live);
    assert!(admit(&mut context, &envelope(session, 1, open_command())).is_ok());
    assert!(
        admit(
            &mut context,
            &envelope(
                session,
                2,
                Command::MoveContainer(
                    ContainerMove::try_new(live.chunk(), ContainerKind::Chest, 0, 1, 0, 36,)
                        .expect("move"),
                ),
            ),
        )
        .is_ok()
    );
    let report = drain(&mut context).expect("drain settles the queue");
    assert_eq!((report.applied, report.rejected), (1, 1));
    assert_eq!(inventory_of(&context, actor), before_inventory);
    assert_eq!(container_of(&context, live), before_container);
    let mut state = AuthorityState::try_new(limits(), 7).expect("authority");
    let mut context = TickContext::harness(&mut state, TickBudget::full());
    let actor = viewer_scene(
        &mut context,
        session,
        (target, CHEST_BLOCK),
        &[(0, stack(ITEM_DIRT, 5))],
    );
    let live = chest_ref(0, 1);
    context.preload_container(chest_record(0, 1, &[(0, stack(ITEM_DIRT, 60))]));
    assert!(admit(&mut context, &envelope(session, 1, open_command())).is_ok());
    assert!(
        admit(
            &mut context,
            &envelope(
                session,
                2,
                Command::MoveContainer(
                    ContainerMove::try_new(live.chunk(), ContainerKind::Chest, 0, 1, 0, 36,)
                        .expect("move"),
                ),
            ),
        )
        .is_ok()
    );
    let report = drain(&mut context).expect("drain settles the queue");
    assert_eq!((report.applied, report.rejected), (2, 0));
    assert_eq!(inventory_of(&context, actor).slots[0], stack(ITEM_DIRT, 1));
    assert_eq!(
        container_cells(&container_of(&context, live))[0],
        stack(ITEM_DIRT, 64)
    );

    // A duplicate delivery settles once: the second identical envelope sees
    // the emptied source and refuses, so the multiset still balances
    // (duplicate-visible remainder of the ordering-layer skip the inventory
    // provider pins, applied here to the container view).
    let mut state = AuthorityState::try_new(limits(), 7).expect("authority");
    let mut context = TickContext::harness(&mut state, TickBudget::full());
    let actor = viewer_scene(
        &mut context,
        session,
        (target, CHEST_BLOCK),
        &[(0, stack(ITEM_DIRT, 5))],
    );
    let live = chest_ref(0, 1);
    context.preload_container(chest_record(0, 1, &[]));
    let start = (inventory_of(&context, actor), container_of(&context, live));
    let duplicate = envelope(
        session,
        2,
        Command::MoveContainer(
            ContainerMove::try_new(live.chunk(), ContainerKind::Chest, 0, 1, 0, 36).expect("move"),
        ),
    );
    assert!(admit(&mut context, &envelope(session, 1, open_command())).is_ok());
    assert!(admit(&mut context, &duplicate).is_ok());
    assert!(admit(&mut context, &duplicate).is_ok());
    let report = drain(&mut context).expect("drain settles the queue");
    assert_eq!(report.examined, 3);
    assert_eq!(report.applied, 2);
    assert_eq!(report.rejected, 1);
    let after_inventory = inventory_of(&context, actor);
    let after_container = container_of(&context, live);
    assert_eq!(after_inventory.slots[0], ItemStack::default());
    assert_eq!(container_cells(&after_container)[0], stack(ITEM_DIRT, 5));
    assert_eq!(
        item_totals(&after_inventory, &after_container),
        item_totals(&start.0, &start.1)
    );

    // Same-tick multi-player moves settle in admission order: the first
    // viewer takes the chest stack and the second sees an empty source
    // (stable session order in `TestMoveChestMultiPlayerSameTickStableOrder`).
    let mut state = AuthorityState::try_new(limits(), 7).expect("authority");
    let mut context = TickContext::harness(&mut state, TickBudget::full());
    let first_actor = viewer_scene(&mut context, session, (target, CHEST_BLOCK), &[]);
    let second_actor = viewer_scene(&mut context, other, (target, CHEST_BLOCK), &[]);
    let live = chest_ref(0, 1);
    context.preload_container(chest_record(0, 1, &[(0, stack(ITEM_DIRT, 10))]));
    assert!(admit(&mut context, &envelope(session, 1, open_command())).is_ok());
    assert!(admit(&mut context, &envelope(other, 1, open_command())).is_ok());
    assert!(
        admit(
            &mut context,
            &envelope(
                session,
                2,
                Command::MoveContainer(
                    ContainerMove::try_new(live.chunk(), ContainerKind::Chest, 0, 1, 36, 0,)
                        .expect("move"),
                ),
            ),
        )
        .is_ok()
    );
    assert!(
        admit(
            &mut context,
            &envelope(
                other,
                2,
                Command::MoveContainer(
                    ContainerMove::try_new(live.chunk(), ContainerKind::Chest, 0, 1, 36, 1,)
                        .expect("move"),
                ),
            ),
        )
        .is_ok()
    );
    let report = drain(&mut context).expect("drain settles the queue");
    assert_eq!(report.examined, 4);
    assert_eq!(report.applied, 3);
    assert_eq!(report.rejected, 1);
    assert_eq!(
        inventory_of(&context, first_actor).slots[0],
        stack(ITEM_DIRT, 10)
    );
    assert_eq!(
        inventory_of(&context, second_actor).slots[1],
        ItemStack::default()
    );
    assert_eq!(
        container_cells(&container_of(&context, live))[0],
        ItemStack::default()
    );

    // Foreign shapes refuse through the canonical harness without effect:
    // the provider owns only the container family on its two phases.
    let shape_fixture = Fixture {
        seed: 0,
        initial: FixtureState {
            runtime: Vec::new(),
            actors: Vec::new(),
            chunks: Vec::new(),
            inventories: Vec::new(),
            containers: Vec::new(),
            work: WorkState::default(),
            sleep: SleepState {
                beds: Vec::new(),
                day_phase_offset: 0,
                pending_offset: None,
            },
            projectiles: Vec::new(),
            drops: Vec::new(),
            world: mornlea_domain::WorldState::try_new(mornlea_domain::WorldStateParts {
                day_phase_offset: 0,
                world_time_ticks: 0,
                weather: mornlea_domain::Weather::Clear,
                season: mornlea_domain::Season::Spring,
                season_progress: 0,
                temperature: 0,
            })
            .expect("world"),
        },
        schedule: Vec::new(),
        expected: Expected {
            events: Vec::new(),
            state_sha256: [0; 32],
            class: None,
            counters: TickCounters::default(),
        },
        budgets: Vec::new(),
    };
    let before = baseline_observed(&shape_fixture.initial);
    let foreign = envelope(session, 9, Command::CloseContainer);
    let observed = run_phase(
        &shape_fixture,
        provider::run,
        RuleCall {
            phase: RulePhase::CompanionIntent,
            actor: None,
            command: Some(&foreign),
            internal: None,
        },
        TickBudget::full(),
    );
    assert_no_effect(&before, &observed, RuleReject::StaleObservation);
}

#[test]
fn exhausted_chunk_revision_cannot_debit_container_transfer() {
    use mornlea_domain::chunk_block_index;
    use mornlea_server::core::world::ReadyChunk;
    use mornlea_storage::{Chunk, ContainerSnapshot, FurnaceSlot, StorageKind};
    let session = player_session(61, "exhausted-transfer");
    let target = BlockPos::new(0, 65, -1);
    let mut authority = AuthorityState::try_new(limits(), 0).unwrap();
    let mut ctx = TickContext::harness(&mut authority, TickBudget::full());
    let actor = viewer_scene(
        &mut ctx,
        session,
        (target, FURNACE_BLOCK),
        &[(0, stack(ITEM_RAW_IRON, 3))],
    );
    let mut chunk = Chunk {
        sections: vec![
            ContainerSnapshot {
                kind: StorageKind::Single,
                bits: 0,
                single: 0,
                palette: vec![],
                packed: vec![]
            };
            24
        ],
        drops: vec![Default::default(); 32],
        furnaces: vec![Default::default(); 32],
        chests: vec![Default::default(); 16],
    };
    let index = chunk_block_index(target) as usize;
    let mut packed = vec![0; 1024];
    packed[(index % 4096) / 4] = u64::from(FURNACE_BLOCK) << ((index % 4) * 15);
    chunk.sections[index / 4096] = ContainerSnapshot {
        kind: StorageKind::Direct,
        bits: 15,
        single: 0,
        palette: vec![],
        packed,
    };
    chunk.furnaces[0] = FurnaceSlot {
        active: true,
        generation: 1,
        block_index: index as u32,
        ..Default::default()
    };
    let key = overworld_key(target);
    ctx.preload_ready_chunk(ReadyChunk::try_new(key, 1, u64::MAX, chunk).unwrap());
    let reference = ContainerRef::try_new(key.pos, ContainerKind::Furnace, 0, 1).unwrap();
    let before = inventory_of(&ctx, actor);
    let stored = container_of(&ctx, reference);
    admit(&mut ctx, &envelope(session, 1, open_command())).unwrap();
    admit(
        &mut ctx,
        &envelope(
            session,
            2,
            Command::MoveContainer(
                ContainerMove::try_new(key.pos, ContainerKind::Furnace, 0, 1, 0, 36).unwrap(),
            ),
        ),
    )
    .unwrap();
    drain(&mut ctx).unwrap();
    assert_eq!(inventory_of(&ctx, actor), before);
    assert_eq!(container_of(&ctx, reference), stored);
}
