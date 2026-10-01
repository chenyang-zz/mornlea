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
    ContainerMove, ContainerRef, CraftingSize, Dimension, FiniteVec3, HeldActions, LookAngles,
    MotionState, MotionStateParts, Movement, PartialMove, PlayerControl, PlayerControlParts,
    PlayerId, StackSource, StackView, SurvivalState, SurvivalStateParts,
};
use mornlea_protocol::{LoginStart, admit_login};
use mornlea_server::contracts::{
    ActorAux, ActorBody, ActorLifecycle, ActorRecord, ActorRuntime, BlockObservation, ChunkKey,
    ContainerRecord, ContainerSlots, EnvironmentState, FixtureState, InventoryRecord, RuleEffect,
    RulePhase, RuleTunables, SessionKey, SleepState, TransportKind, WorkState,
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

fn player_login(tag: u8, name: &str) -> mornlea_protocol::AdmittedLogin {
    let mut bytes = [0u8; 16];
    bytes[0] = tag.max(1);
    bytes[6] = 0x40;
    bytes[8] = 0x80;
    let id = PlayerId::try_from_bytes(bytes).expect("player id");
    let start = LoginStart::new(id, name, 8).expect("login start");
    let inbound = LoginStart::decode_inbound(&start.encode().expect("encoded")).expect("inbound");
    admit_login(inbound).expect("admitted")
}

/// Mints one session identity through the real admission path. Session keys
/// are process-local nonzero ids, so a key minted on a throwaway authority is
/// a valid fixture identity for the replay authority.
fn player_session(tag: u8, name: &str) -> SessionKey {
    let mut mint = AuthorityState::try_new(limits(), 0).expect("authority");
    mint.admit(player_login(tag, name), TransportKind::Memory)
        .expect("session")
}

/// Multi-viewer fixtures share one mint so their process-local keys differ.
fn player_sessions(first: (u8, &str), second: (u8, &str)) -> (SessionKey, SessionKey) {
    let pair_limits = ServerLimits::try_new(2, 1, 1, 1, 1, 1).expect("two-player limits");
    let mut mint = AuthorityState::try_new(pair_limits, 0).expect("authority");
    let first = mint
        .admit(player_login(first.0, first.1), TransportKind::Memory)
        .expect("first session");
    let second = mint
        .admit(player_login(second.0, second.1), TransportKind::Memory)
        .expect("second session");
    (first, second)
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
    ContainerRef::try_new(ChunkPos::new(0, -1), ContainerKind::Chest, slot, generation)
        .expect("chest reference")
}

fn furnace_ref(slot: u8, generation: u32) -> ContainerRef {
    ContainerRef::try_new(
        ChunkPos::new(0, -1),
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

/// Installs the fixture's exact block and fixed slot in a compact Ready chunk.
/// Reinstallation preserves other live slots in the same chunk and replaces
/// a reused slot's generation before the command drains.
fn install_container(context: &mut TickContext<'_>, record: ContainerRecord) {
    use mornlea_domain::chunk_block_index;
    use mornlea_server::core::world::ReadyChunk;
    use mornlea_storage::{ChestSlot, Chunk, ContainerSnapshot, FurnaceSlot, StorageKind};

    let key = ChunkKey {
        dimension: Dimension::OVERWORLD,
        pos: record.reference.chunk(),
    };
    let mut records: Vec<_> = context
        .read()
        .container_refs(key)
        .into_iter()
        .filter(|reference| {
            reference.kind() != record.reference.kind()
                || reference.slot() != record.reference.slot()
        })
        .filter_map(|reference| context.read().container(reference))
        .collect();
    records.push(record);
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
    for record in records {
        let slot = usize::from(record.reference.slot());
        let pos = BlockPos::new(slot as i32, 65, -1);
        let index = chunk_block_index(pos) as usize;
        let section = &mut chunk.sections[index / 4096];
        if section.kind == StorageKind::Single {
            *section = ContainerSnapshot {
                kind: StorageKind::Direct,
                bits: 15,
                single: 0,
                palette: vec![],
                packed: vec![0; 1024],
            };
        }
        let block = match record.slots {
            ContainerSlots::Chest(items) => {
                chunk.chests[slot] = ChestSlot {
                    active: true,
                    generation: record.reference.generation(),
                    block_index: index as u32,
                    items,
                };
                CHEST_BLOCK
            }
            ContainerSlots::Furnace {
                slots,
                fuel,
                progress,
            } => {
                chunk.furnaces[slot] = FurnaceSlot {
                    active: true,
                    generation: record.reference.generation(),
                    block_index: index as u32,
                    input: slots[0],
                    fuel: slots[1],
                    output: slots[2],
                    burn_ticks: fuel as u16,
                    progress_ticks: progress as u8,
                };
                FURNACE_BLOCK
            }
        };
        section.packed[(index % 4096) / 4] |= u64::from(block) << ((index % 4) * 15);
    }
    context.preload_ready_chunk(ReadyChunk::try_new(key, 1, 1, chunk).unwrap());
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

fn air_chunk(context: &mut TickContext<'_>, key: ChunkKey) {
    use mornlea_server::core::world::ReadyChunk;
    use mornlea_storage::{Chunk, ContainerSnapshot, StorageKind};
    let chunk = Chunk {
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
    context.preload_ready_chunk(ReadyChunk::try_new(key, 1, 1, chunk).unwrap());
}

fn fixture_world() -> mornlea_domain::WorldState {
    mornlea_domain::WorldState::try_new(mornlea_domain::WorldStateParts {
        day_phase_offset: 0,
        world_time_ticks: 0,
        weather: mornlea_domain::Weather::Clear,
        season: mornlea_domain::Season::Spring,
        season_progress: 0,
        temperature: 0,
    })
    .unwrap()
}

fn drop_from(
    session: SessionKey,
    sequence: u64,
    reference: ContainerRef,
    slot: u8,
) -> CommandEnvelope {
    envelope(
        session,
        sequence,
        Command::DropStack(StackSource::try_new(StackView::Container(reference), slot).unwrap()),
    )
}

#[test]
fn lease_close_and_commit_do_not_resurrect_previous_view() {
    let session = player_session(91, "lease-commit");
    let target = BlockPos::new(0, 65, -1);
    let reference = chest_ref(0, 1);
    let mut authority = AuthorityState::try_new(limits(), 7).unwrap();
    {
        let mut ctx = TickContext::harness(&mut authority, TickBudget::full());
        viewer_scene(&mut ctx, session, (target, CHEST_BLOCK), &[]);
        install_container(&mut ctx, chest_record(0, 1, &[(0, stack(ITEM_DIRT, 2))]));
        provider::settle_command(&mut ctx, &envelope(session, 1, open_command())).unwrap();
        let leases = ctx.viewer_leases();
        assert_eq!(leases[&session].reference(), reference);
        drop(ctx);
        authority.commit_viewers(leases);
    }
    {
        let mut ctx = TickContext::harness(&mut authority, TickBudget::full());
        viewer_scene(&mut ctx, session, (target, CHEST_BLOCK), &[]);
        install_container(&mut ctx, chest_record(0, 1, &[(0, stack(ITEM_DIRT, 2))]));
        assert_eq!(ctx.read().viewer(session).unwrap().reference(), reference);
        provider::settle_command(&mut ctx, &envelope(session, 2, Command::CloseContainer)).unwrap();
        assert!(ctx.read().viewer(session).is_none());
        assert!(
            provider::settle_command(
                &mut ctx,
                &envelope(
                    session,
                    3,
                    Command::MoveContainer(
                        ContainerMove::try_new(
                            reference.chunk(),
                            ContainerKind::Chest,
                            0,
                            1,
                            36,
                            0
                        )
                        .unwrap()
                    )
                )
            )
            .is_err()
        );
        let leases = ctx.viewer_leases();
        assert!(leases.is_empty());
        drop(ctx);
        authority.commit_viewers(leases);
    }
    assert!(
        TickContext::harness(&mut authority, TickBudget::full())
            .read()
            .viewer(session)
            .is_none()
    );
}

#[test]
fn container_drop_is_whole_atomic_and_ages_only_on_next_step() {
    use mornlea_server::rules::drops;
    let session = player_session(92, "panel-drop");
    let target = BlockPos::new(0, 65, -1);
    let foot = overworld_key(BlockPos::new(0, 64, 0));
    let mut authority = AuthorityState::try_new(limits(), 7).unwrap();
    let mut ctx = TickContext::harness(&mut authority, TickBudget::full());
    let actor = viewer_scene(&mut ctx, session, (target, CHEST_BLOCK), &[]);
    install_container(&mut ctx, chest_record(0, 1, &[(26, stack(ITEM_STONE, 4))]));
    air_chunk(&mut ctx, foot);
    let reference = chest_ref(0, 1);
    provider::settle_command(&mut ctx, &envelope(session, 1, open_command())).unwrap();
    drops::advance(&mut ctx, &[foot]).unwrap();
    provider::settle_command(&mut ctx, &drop_from(session, 2, reference, 62)).unwrap();
    assert_eq!(
        container_cells(&container_of(&ctx, reference))[26],
        ItemStack::default()
    );
    assert_eq!(inventory_of(&ctx, actor).slots[0], ItemStack::default());
    assert_eq!(ctx.read().drops(foot)[0].stack, stack(ITEM_STONE, 4));
    assert_eq!(
        (
            ctx.read().drops(foot)[0].age,
            ctx.read().drops(foot)[0].pickup_delay
        ),
        (0, 40)
    );
    drops::advance(&mut ctx, &[foot]).unwrap();
    assert_eq!(
        (
            ctx.read().drops(foot)[0].age,
            ctx.read().drops(foot)[0].pickup_delay
        ),
        (1, 39)
    );
    assert!(ctx.events().is_empty());
    let before = ctx.snapshot_state(fixture_world());
    assert_eq!(
        provider::settle_command(&mut ctx, &drop_from(session, 3, reference, 62)),
        Err(mornlea_server::contracts::RuleReject::Wire(
            mornlea_domain::RejectReason::InvalidSlot
        ))
    );
    assert_eq!(ctx.snapshot_state(fixture_world()), before);
}

#[test]
fn close_previews_only_extended_grid_and_clears_missing_actor_view() {
    let session = player_session(93, "close-preview");
    let target = BlockPos::new(0, 65, -1);
    let mut authority = AuthorityState::try_new(limits(), 7).unwrap();
    let mut ctx = TickContext::harness(&mut authority, TickBudget::full());
    let actor = viewer_scene(&mut ctx, session, (target, CHEST_BLOCK), &[]);
    install_container(&mut ctx, chest_record(0, 1, &[]));
    provider::settle_command(&mut ctx, &envelope(session, 1, open_command())).unwrap();
    let mut before = InventoryRecord::empty();
    before.crafting_size = CraftingSize::Workbench;
    before.crafting[0] = stack(ITEM_STONE, 1);
    before.crafting[4] = stack(ITEM_DIRT, 2);
    before.crafting[8] = stack(ITEM_DIRT, 3);
    ctx.preload_inventory(actor, before);
    provider::settle_command(&mut ctx, &envelope(session, 2, Command::CloseContainer)).unwrap();
    let after = inventory_of(&ctx, actor);
    assert_eq!(after.crafting_size, CraftingSize::Personal);
    assert_eq!(after.crafting[0], stack(ITEM_STONE, 1));
    assert_eq!(after.crafting[4], ItemStack::default());
    assert_eq!(after.crafting[8], ItemStack::default());
    assert_eq!(after.slots[0], stack(ITEM_DIRT, 5));
    assert!(ctx.read().viewer(session).is_none());

    provider::settle_command(&mut ctx, &envelope(session, 3, open_command())).unwrap();
    let mut full = InventoryRecord::empty();
    full.crafting_size = CraftingSize::Workbench;
    full.slots.fill(stack(ITEM_STONE, 64));
    full.crafting[4] = stack(ITEM_DIRT, 1);
    ctx.preload_inventory(actor, full);
    assert_eq!(
        provider::settle_command(&mut ctx, &envelope(session, 4, Command::CloseContainer)),
        Err(mornlea_server::contracts::RuleReject::Wire(
            mornlea_domain::RejectReason::InvalidInput
        ))
    );
    assert_eq!(inventory_of(&ctx, actor), full);
    assert!(ctx.read().viewer(session).is_some());
    full.crafting_size = CraftingSize::Personal;
    full.crafting[4] = ItemStack::default();
    full.crafting[0] = stack(ITEM_DIRT, 1);
    ctx.preload_inventory(actor, full);
    provider::settle_command(&mut ctx, &envelope(session, 5, Command::CloseContainer)).unwrap();
    assert_eq!(inventory_of(&ctx, actor), full);
    assert!(ctx.read().viewer(session).is_none());

    let absent = player_session(94, "absent-close");
    ctx.stage(RuleEffect::Viewer {
        session: absent,
        view: Some(mornlea_server::contracts::ViewLease::new(
            absent,
            chest_ref(0, 1),
        )),
    })
    .unwrap();
    provider::settle_command(&mut ctx, &envelope(absent, 1, Command::CloseContainer)).unwrap();
    assert!(ctx.read().viewer(absent).is_none());
}

#[test]
fn inventory_only_container_commands_touch_exact_referenced_chunk() {
    let session = player_session(95, "source-touch");
    let target = BlockPos::new(0, 65, -1);
    let foot = overworld_key(BlockPos::new(0, 64, 0));
    let source = overworld_key(target);
    let mut authority = AuthorityState::try_new(limits(), 7).unwrap();
    let mut ctx = TickContext::harness(&mut authority, TickBudget::full());
    let actor = viewer_scene(
        &mut ctx,
        session,
        (target, CHEST_BLOCK),
        &[(0, stack(ITEM_DIRT, 2))],
    );
    install_container(&mut ctx, chest_record(0, 1, &[]));
    air_chunk(&mut ctx, foot);
    let reference = chest_ref(0, 1);
    provider::settle_command(&mut ctx, &envelope(session, 1, open_command())).unwrap();
    provider::settle_command(
        &mut ctx,
        &envelope(
            session,
            2,
            Command::MoveContainer(
                ContainerMove::try_new(reference.chunk(), ContainerKind::Chest, 0, 1, 0, 1)
                    .unwrap(),
            ),
        ),
    )
    .unwrap();
    assert_eq!(inventory_of(&ctx, actor).slots[1], stack(ITEM_DIRT, 2));
    let state = ctx.snapshot_state(fixture_world());
    assert_eq!(
        state.chunks.iter().find(|row| row.0 == source).unwrap().2,
        2
    );
    assert_eq!(state.chunks.iter().find(|row| row.0 == foot).unwrap().2, 1);
    provider::settle_command(&mut ctx, &drop_from(session, 3, reference, 1)).unwrap();
    let state = ctx.snapshot_state(fixture_world());
    assert_eq!(
        state.chunks.iter().find(|row| row.0 == source).unwrap().2,
        2
    );
    assert_eq!(state.chunks.iter().find(|row| row.0 == foot).unwrap().2, 2);
    assert_eq!(ctx.read().drops(foot)[0].stack, stack(ITEM_DIRT, 2));
}

#[test]
fn furnace_panel_drop_debits_input_fuel_and_output_without_repack_veto() {
    let target = BlockPos::new(0, 65, -1);
    let foot = overworld_key(BlockPos::new(0, 64, 0));
    for (slot, expected) in [
        (36, stack(ITEM_RAW_IRON, 2)),
        (37, stack(ITEM_COAL, 3)),
        (38, stack(ITEM_IRON_INGOT, 4)),
    ] {
        let session = player_session(96 + (slot - 36), "furnace-panel");
        let mut authority = AuthorityState::try_new(limits(), 7).unwrap();
        let mut ctx = TickContext::harness(&mut authority, TickBudget::full());
        let actor = viewer_scene(&mut ctx, session, (target, FURNACE_BLOCK), &[]);
        install_container(
            &mut ctx,
            furnace_record(
                0,
                1,
                stack(ITEM_RAW_IRON, 2),
                stack(ITEM_COAL, 3),
                stack(ITEM_IRON_INGOT, 4),
                3,
            ),
        );
        air_chunk(&mut ctx, foot);
        let mut full = InventoryRecord::empty();
        full.slots.fill(stack(ITEM_STONE, 64));
        full.crafting_size = CraftingSize::Workbench;
        full.crafting[4] = stack(ITEM_DIRT, 1);
        ctx.preload_inventory(actor, full);
        let reference = furnace_ref(0, 1);
        provider::settle_command(&mut ctx, &envelope(session, 1, open_command())).unwrap();
        provider::settle_command(&mut ctx, &drop_from(session, 2, reference, slot)).unwrap();
        assert_eq!(ctx.read().drops(foot)[0].stack, expected);
        assert_eq!(inventory_of(&ctx, actor), full);
        let stored = container_of(&ctx, reference);
        let ContainerSlots::Furnace {
            slots, progress, ..
        } = stored.slots
        else {
            panic!("furnace")
        };
        assert_eq!(slots[usize::from(slot - 36)], ItemStack::default());
        assert_eq!(progress, if slot == 36 { 0 } else { 3 });
    }
}

#[test]
fn full_drop_slots_refuse_then_same_cell_merge_preserves_source_atomicity() {
    use mornlea_domain::chunk_block_index;
    use mornlea_server::core::world::ReadyChunk;
    use mornlea_storage::{Chunk, ContainerSnapshot, DropSlot, StorageKind};
    let session = player_session(99, "drop-capacity");
    let target = BlockPos::new(0, 65, -1);
    let foot_pos = BlockPos::new(0, 64, 0);
    let foot = overworld_key(foot_pos);
    let reference = chest_ref(0, 1);
    let mut authority = AuthorityState::try_new(limits(), 7).unwrap();
    let mut ctx = TickContext::harness(&mut authority, TickBudget::full());
    let actor = viewer_scene(&mut ctx, session, (target, CHEST_BLOCK), &[]);
    install_container(&mut ctx, chest_record(0, 1, &[(0, stack(ITEM_DIRT, 1))]));
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
    for slot in &mut chunk.drops {
        *slot = DropSlot {
            active: true,
            generation: 1,
            stack: stack(ITEM_STONE, 64),
            block_index: chunk_block_index(foot_pos),
            ..Default::default()
        };
    }
    ctx.preload_ready_chunk(ReadyChunk::try_new(foot, 1, 1, chunk.clone()).unwrap());
    provider::settle_command(&mut ctx, &envelope(session, 1, open_command())).unwrap();
    let before = ctx.snapshot_state(fixture_world());
    assert_eq!(
        provider::settle_command(&mut ctx, &drop_from(session, 2, reference, 36)),
        Err(mornlea_server::contracts::RuleReject::Wire(
            mornlea_domain::RejectReason::DropCapacity
        ))
    );
    assert_eq!(ctx.snapshot_state(fixture_world()), before);
    assert_eq!(inventory_of(&ctx, actor).slots[0], ItemStack::default());
    chunk.drops[0].stack = stack(ITEM_DIRT, 63);
    ctx.preload_ready_chunk(ReadyChunk::try_new(foot, 1, 1, chunk).unwrap());
    provider::settle_command(&mut ctx, &drop_from(session, 3, reference, 36)).unwrap();
    assert_eq!(
        container_cells(&container_of(&ctx, reference))[0],
        ItemStack::default()
    );
    assert_eq!(ctx.read().drops(foot)[0].stack, stack(ITEM_DIRT, 64));
}

#[test]
fn ready_open_binds_exact_slot_before_first_move() {
    use mornlea_domain::chunk_block_index;
    use mornlea_server::core::world::ReadyChunk;
    use mornlea_storage::{ChestSlot, Chunk, ContainerSnapshot, StorageKind};

    let session = player_session(90, "exact-open");
    let target = BlockPos::new(0, 65, -1);
    let mut authority = AuthorityState::try_new(limits(), 7).unwrap();
    let mut ctx = TickContext::harness(&mut authority, TickBudget::full());
    let actor = viewer_scene(
        &mut ctx,
        session,
        (target, CHEST_BLOCK),
        &[(0, stack(ITEM_DIRT, 3))],
    );
    assert_eq!(
        provider::settle_command(&mut ctx, &envelope(session, 1, open_command())),
        Err(mornlea_server::contracts::RuleReject::Wire(
            mornlea_domain::RejectReason::ChunkNotReady
        ))
    );
    assert!(ctx.read().viewer(session).is_none());
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
    packed[(index % 4096) / 4] = u64::from(CHEST_BLOCK) << ((index % 4) * 15);
    chunk.sections[index / 4096] = ContainerSnapshot {
        kind: StorageKind::Direct,
        bits: 15,
        single: 0,
        palette: vec![],
        packed,
    };
    chunk.chests[7] = ChestSlot {
        active: true,
        generation: 9,
        block_index: index as u32,
        ..Default::default()
    };
    let key = overworld_key(target);
    let mut without_slot = chunk.clone();
    without_slot.chests[7] = ChestSlot::default();
    ctx.preload_ready_chunk(ReadyChunk::try_new(key, 1, 1, without_slot).unwrap());
    assert_eq!(
        provider::settle_command(&mut ctx, &envelope(session, 1, open_command())),
        Err(mornlea_server::contracts::RuleReject::Wire(
            mornlea_domain::RejectReason::NoTarget
        ))
    );
    assert!(ctx.read().viewer(session).is_none());
    ctx.preload_ready_chunk(ReadyChunk::try_new(key, 1, 1, chunk).unwrap());
    let exact = ContainerRef::try_new(key.pos, ContainerKind::Chest, 7, 9).unwrap();
    assert_eq!(
        provider::settle_command(&mut ctx, &envelope(session, 1, open_command()))
            .unwrap()
            .applied,
        1
    );
    assert_eq!(ctx.read().viewer(session).unwrap().reference(), exact);
    assert_eq!(
        provider::settle_command(
            &mut ctx,
            &envelope(
                session,
                2,
                Command::OpenContainer(LookAngles::try_new(std::f32::consts::PI, 0.0).unwrap())
            )
        ),
        Err(mornlea_server::contracts::RuleReject::Wire(
            mornlea_domain::RejectReason::ChunkNotReady
        ))
    );
    assert_eq!(ctx.read().viewer(session).unwrap().reference(), exact);
    let wrong = ContainerRef::try_new(key.pos, ContainerKind::Chest, 0, 1).unwrap();
    let before = inventory_of(&ctx, actor);
    assert!(
        provider::settle_command(
            &mut ctx,
            &envelope(
                session,
                2,
                Command::MoveContainer(
                    ContainerMove::try_new(
                        wrong.chunk(),
                        wrong.kind(),
                        wrong.slot(),
                        wrong.generation(),
                        0,
                        36
                    )
                    .unwrap()
                )
            )
        )
        .is_err()
    );
    assert_eq!(inventory_of(&ctx, actor), before);
    assert_eq!(ctx.read().viewer(session).unwrap().reference(), exact);
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
    install_container(
        &mut context,
        furnace_record(
            0,
            1,
            ItemStack::default(),
            ItemStack::default(),
            ItemStack::default(),
            0,
        ),
    );
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
    install_container(
        &mut context,
        furnace_record(
            0,
            1,
            ItemStack::default(),
            ItemStack::default(),
            ItemStack::default(),
            0,
        ),
    );
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
    install_container(
        &mut context,
        furnace_record(
            0,
            2,
            ItemStack::default(),
            ItemStack::default(),
            ItemStack::default(),
            0,
        ),
    );
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

    // Open binds immediately, and a later close removes the same lease.
    // Replaying an already drained queue is not a new command admission.
    let mut state = AuthorityState::try_new(limits(), 7).expect("authority");
    let mut context = TickContext::harness(&mut state, TickBudget::full());
    let actor = viewer_scene(
        &mut context,
        session,
        (target, FURNACE_BLOCK),
        &[(0, stack(ITEM_RAW_IRON, 4))],
    );
    let reference = furnace_ref(0, 1);
    install_container(
        &mut context,
        furnace_record(
            0,
            1,
            ItemStack::default(),
            ItemStack::default(),
            ItemStack::default(),
            0,
        ),
    );
    provider::settle_command(&mut context, &envelope(session, 1, open_command())).unwrap();
    assert_eq!(
        context.read().viewer(session).unwrap().reference(),
        reference
    );
    provider::settle_command(
        &mut context,
        &envelope(
            session,
            2,
            Command::MoveContainer(
                ContainerMove::try_new(reference.chunk(), ContainerKind::Furnace, 0, 1, 0, 36)
                    .unwrap(),
            ),
        ),
    )
    .unwrap();
    assert_eq!(inventory_of(&context, actor).slots[0], ItemStack::default());
    let settled_container = container_of(&context, reference);
    assert_eq!(
        container_cells(&settled_container)[0],
        stack(ITEM_RAW_IRON, 4)
    );
    provider::settle_command(&mut context, &envelope(session, 3, Command::CloseContainer)).unwrap();
    assert!(context.read().viewer(session).is_none());
    assert!(
        provider::settle_command(
            &mut context,
            &envelope(
                session,
                4,
                Command::MoveContainer(
                    ContainerMove::try_new(reference.chunk(), ContainerKind::Furnace, 0, 1, 36, 1)
                        .unwrap(),
                ),
            )
        )
        .is_err()
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
    install_container(
        &mut context,
        furnace_record(
            0,
            1,
            ItemStack::default(),
            ItemStack::default(),
            stack(ITEM_IRON_INGOT, 3),
            0,
        ),
    );
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
    install_container(
        &mut context,
        furnace_record(
            0,
            1,
            ItemStack::default(),
            ItemStack::default(),
            ItemStack::default(),
            0,
        ),
    );
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
    install_container(
        &mut context,
        furnace_record(
            0,
            1,
            ItemStack::default(),
            ItemStack::default(),
            ItemStack::default(),
            0,
        ),
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
    assert_eq!(container_cells(&after_container)[1], stack(ITEM_COAL, 3));
    assert_eq!(container_cells(&after_container)[2], ItemStack::default());

    // The output is only ever a quick-move source: the smelted ingots credit
    // into the backpack with the remainder kept at the source.
    let mut state = AuthorityState::try_new(limits(), 7).expect("authority");
    let mut context = TickContext::harness(&mut state, TickBudget::full());
    let actor = viewer_scene(&mut context, session, (target, FURNACE_BLOCK), &[]);
    let reference = furnace_ref(0, 1);
    install_container(
        &mut context,
        furnace_record(
            0,
            1,
            ItemStack::default(),
            ItemStack::default(),
            stack(ITEM_IRON_INGOT, 3),
            0,
        ),
    );
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
    install_container(
        &mut context,
        furnace_record(
            0,
            1,
            ItemStack::default(),
            ItemStack::default(),
            ItemStack::default(),
            0,
        ),
    );
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
    install_container(
        &mut context,
        furnace_record(
            0,
            1,
            stack(ITEM_SAND, 5),
            ItemStack::default(),
            ItemStack::default(),
            7,
        ),
    );
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
    install_container(&mut context, chest_record(0, 1, &[]));
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
    install_container(
        &mut context,
        chest_record(0, 1, &[(0, stack(ITEM_DIRT, 63))]),
    );
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
    install_container(
        &mut context,
        chest_record(0, 1, &[(0, stack(ITEM_DIRT, 7))]),
    );
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
    install_container(
        &mut context,
        chest_record(0, 1, &[(0, stack(ITEM_STONE, 4))]),
    );
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
    install_container(&mut context, chest_record(0, 1, &[]));
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
    let (session, other) = player_sessions((14, "stale-view"), (15, "other-viewer"));
    assert_ne!(session, other);
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
    install_container(&mut context, chest_record(0, 1, &[]));
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
    install_container(&mut context, chest_record(0, 1, &[]));
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
    install_container(&mut context, chest_record(0, 1, &[]));
    install_container(&mut context, chest_record(1, 1, &[]));
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
    install_container(
        &mut context,
        chest_record(0, 1, &[(0, stack(ITEM_DIRT, 64))]),
    );
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
    install_container(
        &mut context,
        chest_record(0, 1, &[(0, stack(ITEM_DIRT, 60))]),
    );
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
    install_container(&mut context, chest_record(0, 1, &[]));
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
    assert_ne!(first_actor, second_actor);
    let live = chest_ref(0, 1);
    install_container(
        &mut context,
        chest_record(0, 1, &[(0, stack(ITEM_DIRT, 10))]),
    );
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
                ContainerMove::try_new(key.pos, ContainerKind::Furnace, 0, 1, 0, 1).unwrap(),
            ),
        ),
    )
    .unwrap();
    assert_eq!(drain(&mut ctx).unwrap().rejected, 1);
    assert_eq!(
        provider::settle_command(
            &mut ctx,
            &envelope(
                session,
                3,
                Command::MoveContainer(
                    ContainerMove::try_new(key.pos, ContainerKind::Furnace, 0, 1, 0, 1).unwrap()
                )
            )
        ),
        Err(mornlea_server::contracts::RuleReject::StaleObservation)
    );
    assert_eq!(inventory_of(&ctx, actor), before);
    assert_eq!(container_of(&ctx, reference), stored);
}

#[test]
fn sneaking_open_preserves_previous_exact_lease() {
    let session = player_session(100, "sneak-open");
    let target = BlockPos::new(0, 65, -1);
    let mut authority = AuthorityState::try_new(limits(), 7).unwrap();
    let mut ctx = TickContext::harness(&mut authority, TickBudget::full());
    let actor = viewer_scene(&mut ctx, session, (target, CHEST_BLOCK), &[]);
    install_container(&mut ctx, chest_record(0, 1, &[]));
    provider::settle_command(&mut ctx, &envelope(session, 1, open_command())).unwrap();
    let lease = ctx.read().viewer(session).unwrap();
    ctx.stage(RuleEffect::Runtime(ActorRuntime {
        key: actor,
        controls: Some(PlayerControl::new(PlayerControlParts {
            movement: Movement {
                move_x: 0,
                move_z: 0,
                jump: false,
            },
            look: LookAngles::try_new(0.0, 0.0).unwrap(),
            actions: HeldActions {
                primary: false,
                eating: false,
                sprinting: false,
                sneaking: true,
            },
        })),
        has_view: false,
        reset: false,
        attack_cooldown: 0,
        hurt_cooldown: 0,
        burn_cooldown: 0,
        oxygen: 300,
        peak_y: 64.0,
        exhaustion_milli: 0,
        saturation_milli: 5_000,
        since_damage_ticks: 0,
        drown_ticks: 0,
        starvation_ticks: 0,
        eating: None,
        bow: None,
        path: None,
        aux: ActorAux::Player {
            respawn: None,
            workbench: None,
        },
    }))
    .unwrap();
    assert_eq!(
        provider::settle_command(&mut ctx, &envelope(session, 2, open_command())),
        Err(mornlea_server::contracts::RuleReject::Wire(
            mornlea_domain::RejectReason::InvalidInput
        ))
    );
    assert_eq!(ctx.read().viewer(session), Some(lease));
}

#[test]
fn actual_furnace_open_replaces_chest_lease() {
    let session = player_session(101, "reopen-kind");
    let target = BlockPos::new(0, 65, -1);
    let mut authority = AuthorityState::try_new(limits(), 7).unwrap();
    let mut ctx = TickContext::harness(&mut authority, TickBudget::full());
    viewer_scene(&mut ctx, session, (target, CHEST_BLOCK), &[]);
    install_container(&mut ctx, chest_record(0, 1, &[]));
    install_container(
        &mut ctx,
        furnace_record(
            1,
            3,
            ItemStack::default(),
            ItemStack::default(),
            ItemStack::default(),
            0,
        ),
    );
    air_chunk(&mut ctx, overworld_key(BlockPos::new(0, 65, 0)));
    provider::settle_command(&mut ctx, &envelope(session, 1, open_command())).unwrap();
    assert_eq!(
        ctx.read().viewer(session).unwrap().reference(),
        chest_ref(0, 1)
    );
    let look = LookAngles::try_new(-0.8, 0.0).unwrap();
    provider::settle_command(
        &mut ctx,
        &envelope(session, 2, Command::OpenContainer(look)),
    )
    .unwrap();
    assert_eq!(
        ctx.read().viewer(session).unwrap().reference(),
        furnace_ref(1, 3)
    );
}

#[test]
fn container_inventory_region_drop_preserves_tool_durability() {
    let session = player_session(102, "durable-panel");
    let target = BlockPos::new(0, 65, -1);
    let foot = overworld_key(BlockPos::new(0, 64, 0));
    let tool = ItemStack {
        item: 10,
        count: 1,
        durability: 17,
    };
    let mut authority = AuthorityState::try_new(limits(), 7).unwrap();
    let mut ctx = TickContext::harness(&mut authority, TickBudget::full());
    let actor = viewer_scene(&mut ctx, session, (target, CHEST_BLOCK), &[(35, tool)]);
    install_container(&mut ctx, chest_record(0, 1, &[]));
    let reference = chest_ref(0, 1);
    provider::settle_command(&mut ctx, &envelope(session, 1, open_command())).unwrap();
    let before = ctx.snapshot_state(fixture_world());
    assert_eq!(
        provider::settle_command(&mut ctx, &drop_from(session, 2, reference, 35)),
        Err(mornlea_server::contracts::RuleReject::Wire(
            mornlea_domain::RejectReason::ChunkNotReady
        ))
    );
    assert_eq!(ctx.snapshot_state(fixture_world()), before);
    air_chunk(&mut ctx, foot);
    provider::settle_command(&mut ctx, &drop_from(session, 2, reference, 35)).unwrap();
    assert_eq!(inventory_of(&ctx, actor).slots[35], ItemStack::default());
    assert_eq!(ctx.read().drops(foot)[0].stack, tool);
}

#[test]
fn two_viewers_cannot_drop_the_same_container_source_twice() {
    let (first, second) = player_sessions((103, "first-dropper"), (104, "second-dropper"));
    assert_ne!(first, second);
    let target = BlockPos::new(0, 65, -1);
    let foot = overworld_key(BlockPos::new(0, 64, 0));
    let mut authority = AuthorityState::try_new(limits(), 7).unwrap();
    let mut ctx = TickContext::harness(&mut authority, TickBudget::full());
    viewer_scene(&mut ctx, first, (target, CHEST_BLOCK), &[]);
    viewer_scene(&mut ctx, second, (target, CHEST_BLOCK), &[]);
    install_container(&mut ctx, chest_record(0, 1, &[(0, stack(ITEM_DIRT, 7))]));
    air_chunk(&mut ctx, foot);
    let reference = chest_ref(0, 1);
    provider::settle_command(&mut ctx, &envelope(first, 1, open_command())).unwrap();
    provider::settle_command(&mut ctx, &envelope(second, 1, open_command())).unwrap();
    provider::settle_command(&mut ctx, &drop_from(first, 2, reference, 36)).unwrap();
    let after = ctx.snapshot_state(fixture_world());
    assert_eq!(
        provider::settle_command(&mut ctx, &drop_from(second, 2, reference, 36)),
        Err(mornlea_server::contracts::RuleReject::Wire(
            mornlea_domain::RejectReason::InvalidSlot
        ))
    );
    assert_eq!(ctx.snapshot_state(fixture_world()), after);
    assert_eq!(ctx.read().drops(foot).len(), 1);
    assert_eq!(ctx.read().drops(foot)[0].stack, stack(ITEM_DIRT, 7));
}

#[test]
fn exhausted_referenced_chunk_refuses_inventory_region_drop_atomically() {
    use mornlea_server::core::world::ReadyChunk;
    let session = player_session(105, "exhausted-drop");
    let target = BlockPos::new(0, 65, -1);
    let source = overworld_key(target);
    let foot = overworld_key(BlockPos::new(0, 64, 0));
    let mut authority = AuthorityState::try_new(limits(), 7).unwrap();
    let mut ctx = TickContext::harness(&mut authority, TickBudget::full());
    viewer_scene(
        &mut ctx,
        session,
        (target, CHEST_BLOCK),
        &[(0, stack(ITEM_DIRT, 3))],
    );
    install_container(&mut ctx, chest_record(0, 1, &[]));
    let raw = ctx
        .snapshot_state(fixture_world())
        .chunks
        .into_iter()
        .find(|row| row.0 == source)
        .unwrap()
        .3;
    ctx.preload_ready_chunk(ReadyChunk::try_new(source, 1, u64::MAX, raw).unwrap());
    air_chunk(&mut ctx, foot);
    let reference = chest_ref(0, 1);
    provider::settle_command(&mut ctx, &envelope(session, 1, open_command())).unwrap();
    let before = ctx.snapshot_state(fixture_world());
    assert_eq!(
        provider::settle_command(&mut ctx, &drop_from(session, 2, reference, 0)),
        Err(mornlea_server::contracts::RuleReject::StaleObservation)
    );
    assert_eq!(ctx.snapshot_state(fixture_world()), before);
    assert!(ctx.read().drops(foot).is_empty());
}

#[test]
fn transfer_uses_live_lease_without_per_command_reach_veto() {
    let session = player_session(106, "reach-publication");
    let target = BlockPos::new(0, 65, -1);
    let reference = chest_ref(0, 1);
    let mut authority = AuthorityState::try_new(limits(), 7).unwrap();
    let mut ctx = TickContext::harness(&mut authority, TickBudget::full());
    let actor = viewer_scene(
        &mut ctx,
        session,
        (target, CHEST_BLOCK),
        &[(0, stack(ITEM_DIRT, 2))],
    );
    install_container(&mut ctx, chest_record(0, 1, &[]));
    provider::settle_command(&mut ctx, &envelope(session, 1, open_command())).unwrap();
    ctx.stage(RuleEffect::Actor(player_actor(
        session,
        [40.5, 64.0, 40.5],
        0.0,
        0.0,
    )))
    .unwrap();
    provider::settle_command(
        &mut ctx,
        &envelope(
            session,
            2,
            Command::MoveContainer(
                ContainerMove::try_new(reference.chunk(), ContainerKind::Chest, 0, 1, 0, 36)
                    .unwrap(),
            ),
        ),
    )
    .unwrap();
    assert_eq!(inventory_of(&ctx, actor).slots[0], ItemStack::default());
    assert_eq!(
        container_cells(&container_of(&ctx, reference))[0],
        stack(ITEM_DIRT, 2)
    );
}

#[test]
fn ready_container_ray_passes_fluids_and_open_door_halves() {
    use mornlea_domain::chunk_block_index;
    use mornlea_server::core::world::ReadyChunk;
    use mornlea_storage::{ChestSlot, Chunk, ContainerSnapshot, StorageKind};

    for obstruction in [27, 28, 34, 63, 70, 62] {
        let session = player_session(96, "ray-target");
        let target = BlockPos::new(0, 65, -3);
        let key = overworld_key(target);
        let mut authority = AuthorityState::try_new(limits(), 7).unwrap();
        let mut ctx = TickContext::harness(&mut authority, TickBudget::full());
        viewer_scene(&mut ctx, session, (target, CHEST_BLOCK), &[]);
        ctx.stage(RuleEffect::Actor(player_actor(
            session,
            [0.5, 64.0, -0.5],
            0.0,
            0.0,
        )))
        .unwrap();
        let mut chunk = Chunk {
            sections: vec![
                ContainerSnapshot {
                    kind: StorageKind::Single,
                    bits: 0,
                    single: 0,
                    palette: vec![],
                    packed: vec![],
                };
                24
            ],
            drops: vec![Default::default(); 32],
            furnaces: vec![Default::default(); 32],
            chests: vec![Default::default(); 16],
        };
        for (pos, block) in [
            (target, CHEST_BLOCK),
            (BlockPos::new(0, 65, -2), obstruction),
            (BlockPos::new(0, 64, -2), 63),
        ] {
            let index = chunk_block_index(pos) as usize;
            let section = &mut chunk.sections[index / 4096];
            if section.kind == StorageKind::Single {
                *section = ContainerSnapshot {
                    kind: StorageKind::Direct,
                    bits: 15,
                    single: 0,
                    palette: vec![],
                    packed: vec![0; 1024],
                };
            }
            section.packed[(index % 4096) / 4] |= u64::from(block) << ((index % 4) * 15);
        }
        chunk.chests[7] = ChestSlot {
            active: true,
            generation: 9,
            block_index: chunk_block_index(target),
            ..Default::default()
        };
        ctx.preload_ready_chunk(ReadyChunk::try_new(key, 1, 1, chunk).unwrap());
        let result = provider::settle_command(&mut ctx, &envelope(session, 1, open_command()));
        if obstruction == 62 {
            assert!(result.is_err(), "a closed door still blocks the container");
            assert!(ctx.read().viewer(session).is_none());
        } else {
            assert!(
                result.is_ok(),
                "transparent interaction block {obstruction}: {result:?}"
            );
            assert_eq!(
                ctx.read().viewer(session).unwrap().reference(),
                chest_ref(7, 9)
            );
        }
    }
}
