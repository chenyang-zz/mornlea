//! Continuous mining progression replay.
//!
//! Every scene below mirrors a frozen Go oracle row, cited at each case:
//!
//! - Progress accumulation from `packages/server/sim/entity/mining.go`
//!   (`stepMiningProgress`): the target/block/held key unchanged increments once
//!   per tick to saturation, a changed key restarts at 1, and
//!   released/suspended/unready/required-zero clears. Saturation completes in
//!   the same tick the counter reaches the required value.
//! - Required ticks and harvestability from `miningRule` in the same file:
//!   door/bed 15, crop/wild grass/sapling 1, snow layer 1, soil and related 5,
//!   log/planks/workbench 15, the stone and stonebrick tool tiers, and
//!   iron block 20/10/40. Unlisted blocks (bedrock) never progress.
//! - Completion through the accepted authority transaction (`resolve_mine` /
//!   `resolve_companion_mine` plus `MutationTxn::try_mine`): human output stages
//!   world drops under the per-chunk capacity, companion output credits the
//!   companion inventory all-or-nothing, and selected-tool wear follows
//!   `consumeMiningToolDurability` with the crop-plus-intact-hoe, wild grass,
//!   sapling and intact-sword exemptions.
//! - Human/completion outcomes from `advanceMining`: a completed human attempt
//!   clears progress even when the output is rejected, while a companion with a
//!   full inventory keeps saturated progress for retry.
//! - Snow layers clear with no drops and no capacity gate (the Go snow branch);
//!   the human-only clear-only settlement is the authorized narrow addition to
//!   the accepted resolver. Companion snow mining stays refusing.
//! - Short grass completes in one tick through the position-stable seed roll;
//!   its output may be empty and it never wears the selected tool.
//! - Active bow draw suppresses mining resolution (combat precedence), read
//!   through the landed motion-owned runtime lane.
//!
//! No case chooses a value the oracle does not pin. Refusals compare a
//! before/after probe of cells, full inventory records, progress and events,
//! not a bare `is_err`.

use mornlea_domain::{
    BlockPos, ChunkPos, CompanionId, ContainerKind, ContainerRef, Dimension, DropId, FiniteVec3,
    HeldActions, HotbarSlot, LookAngles, MotionState, MotionStateParts, Movement, PlayerControl,
    PlayerControlParts, SurvivalState, SurvivalStateParts, Weather,
};
use mornlea_server::contracts::*;
use mornlea_server::rules::mining as provider;
use mornlea_server::state::{AuthorityState, TickContext};
use mornlea_storage::{CompanionBody, ItemStack, PlayerLocation, PlayerSave};

// Stable block numbers, mirrored from the frozen const block in
// `packages/shared/core/block.go`.
const AIR: u16 = 0; // `core.AirID`
const STONE: u16 = 2; // `core.StoneID`
const DIRT: u16 = 3; // `core.DirtID`
const CHEST: u16 = 11; // `core.ChestID`
const FURNACE: u16 = 9; // `core.FurnaceID`
const DOOR_LOWER_SOUTH_CLOSED: u16 = 62; // `core.DoorLowerSouthClosed`
const DOOR_UPPER: u16 = 70; // `core.DoorUpper`
const WHEAT_STAGE0: u16 = 37; // `core.WheatStage0ID`
const SHORT_GRASS: u16 = 84; // `core.ShortGrassID`
const SNOW_LAYER1: u16 = 85; // `core.SnowLayer1BlockID`
const SAPLING: u16 = 89; // `core.SaplingID`
const BEDROCK: u16 = 5; // `core.BedrockID`
// Stable item numbers, mirrored from the frozen const block in
// `packages/shared/core/item.go`.
const ITEM_STONE: u16 = 1; // `core.ItemStone`
const ITEM_DIRT: u16 = 2; // `core.ItemDirt`
const ITEM_CHEST: u16 = 14; // `core.ItemChest`
const ITEM_STONE_PICKAXE: u16 = 10; // `core.ItemStonePickaxe`
const ITEM_IRON_PICKAXE: u16 = 11; // `core.ItemIronPickaxe`
const ITEM_BROKEN_STONE_PICKAXE: u16 = 12; // `core.ItemBrokenStonePickaxe`
const ITEM_STONE_HOE: u16 = 30; // `core.ItemStoneHoe`
const ITEM_WOODEN_SWORD: u16 = 47; // `core.ItemWoodenSword`

fn authority() -> AuthorityState {
    AuthorityState::try_new(
        ServerLimits::try_new(8, 4096, 512, 64, 64, 1_048_576).expect("limits"),
        7,
    )
    .expect("authority")
}

fn admitted(tag: u8, name: &str) -> mornlea_protocol::AdmittedLogin {
    let mut bytes = [0u8; 16];
    bytes[0] = tag;
    bytes[6] = 0x40;
    bytes[8] = 0x80;
    let id = mornlea_domain::PlayerId::try_from_bytes(bytes).expect("player id");
    let start = mornlea_protocol::LoginStart::new(id, name, 8).expect("login start");
    let inbound = mornlea_protocol::LoginStart::decode_inbound(&start.encode().expect("encoded"))
        .expect("inbound");
    mornlea_protocol::admit_login(inbound).expect("admitted login")
}

fn harness_context(authority: &mut AuthorityState) -> TickContext<'_> {
    TickContext::harness(authority, TickBudget::full())
}

fn environment() -> EnvironmentState {
    EnvironmentState {
        seed: 7,
        next_tick: 0,
        world_time: 0,
        day_phase_offset: 0,
        season_offset: 0,
        weather: Weather::Clear,
        weather_remaining: 0,
        difficulty: 0,
        tunables: RuleTunables::source_defaults(),
    }
}

fn uuid(tag: u8) -> [u8; 16] {
    let mut bytes = [0u8; 16];
    bytes[0] = tag.max(1);
    bytes[6] = 0x40;
    bytes[8] = 0x80;
    bytes
}

fn companion_id() -> CompanionId {
    CompanionId::try_from_bytes(uuid(9)).expect("companion id")
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

fn companion_actor(id: CompanionId, position: [f32; 3]) -> ActorRecord {
    ActorRecord::try_new(
        ActorKey::Companion(id),
        ActorLifecycle::Active,
        Dimension::OVERWORLD,
        MotionState::new(MotionStateParts {
            position: FiniteVec3::try_new(position).expect("position"),
            velocity: FiniteVec3::try_new([0.0, 0.0, 0.0]).expect("velocity"),
            on_ground: true,
        }),
        LookAngles::try_new(0.0, 0.0).expect("look"),
        SurvivalState::try_new(SurvivalStateParts {
            health: 20,
            oxygen: 300,
            hunger: 20,
            saturation_zero: false,
            armor_points: 0,
        })
        .expect("survival"),
        ActorBody::Companion(CompanionBody {
            id: mornlea_storage::PlayerId::from_bytes(id.bytes()),
            dimension: 0,
            position,
            yaw: 0.0,
            pitch: 0.0,
            inventory: mornlea_storage::Inventory::default(),
        }),
    )
    .expect("companion actor")
}

/// Held-primary control aimed by an explicit look, the motion-owned intake the
/// mining provider reads back through the runtime lane.
fn control(primary: bool, yaw: f32, pitch: f32) -> PlayerControl {
    PlayerControl::new(PlayerControlParts {
        movement: Movement {
            move_x: 0,
            move_z: 0,
            jump: false,
        },
        look: LookAngles::try_new(yaw, pitch).expect("look"),
        actions: HeldActions {
            primary,
            eating: false,
            sprinting: false,
            sneaking: false,
        },
    })
}

/// Motion-owned runtime record carrying the held controls under test, with an
/// optional active bow draw for the combat-precedence suppression.
fn human_runtime(key: ActorKey, held: PlayerControl, bow: bool) -> ActorRuntime {
    ActorRuntime {
        key,
        controls: Some(held),
        has_view: false,
        reset: false,
        attack_cooldown: 0,
        hurt_cooldown: 0,
        burn_cooldown: 0,
        oxygen: 300,
        peak_y: 64.0,
        exhaustion_milli: 0,
        saturation_milli: 0,
        since_damage_ticks: 0,
        drown_ticks: 0,
        starvation_ticks: 0,
        eating: None,
        bow: bow.then(|| BowProgress {
            slot: HotbarSlot::new(0).expect("slot"),
            ticks: 7,
        }),
        path: None,
        aux: ActorAux::Player { respawn: None },
    }
}

fn mine_call(actor: ActorKey) -> RuleCall<'static> {
    RuleCall {
        phase: RulePhase::MiningStep,
        actor: Some(actor),
        command: None,
        internal: None,
    }
}

fn companion_envelope(
    id: CompanionId,
    tag: u8,
    action: CompanionAction,
) -> CompanionActionEnvelope {
    CompanionActionEnvelope::try_new(
        id,
        0,
        AgentRequestId::try_from_bytes(uuid(tag)).expect("request id"),
        RunId::try_from_bytes(uuid(tag + 1)).expect("run id"),
        SnapshotId::try_from_bytes(uuid(tag + 2)).expect("snapshot id"),
        1,
        1,
        [0u8; 32],
        action,
    )
    .expect("companion envelope")
}

/// One aimed-south human mining scene: an active player at `[0.5, 64.0, 0.5]`
/// looking south with `held` in hotbar slot zero and staged held controls, the
/// eye cell preloaded as air alongside the caller-staged cells.
fn south_scene(
    context: &mut TickContext<'_>,
    session: SessionKey,
    held: ItemStack,
    cells: &[(BlockPos, u16)],
) -> ActorKey {
    let actor = ActorKey::Player(session);
    context
        .stage(RuleEffect::Environment(environment()))
        .expect("environment");
    context
        .stage(RuleEffect::Actor(player_actor(
            session,
            [0.5, 64.0, 0.5],
            std::f32::consts::PI,
            0.0,
        )))
        .expect("actor");
    context
        .stage(RuleEffect::Runtime(human_runtime(
            actor,
            control(true, std::f32::consts::PI, 0.0),
            false,
        )))
        .expect("runtime");
    let mut inventory = InventoryRecord::empty();
    inventory.slots[0] = held;
    context.preload_inventory(actor, inventory);
    context.preload_block(observation(BlockPos::new(0, 65, 0), AIR));
    for (pos, block) in cells {
        context.preload_block(observation(*pos, *block));
    }
    actor
}

/// One companion mining scene: an active companion at `[2.5, 64.0, 0.5]` with a
/// `MineHold` intent on `target`, the two cells between eye and target staged
/// as air alongside the caller-staged cells.
fn companion_scene(
    context: &mut TickContext<'_>,
    id: CompanionId,
    target: BlockPos,
    inventory: InventoryRecord,
    cells: &[(BlockPos, u16)],
) -> ActorKey {
    let actor = ActorKey::Companion(id);
    context
        .stage(RuleEffect::Environment(environment()))
        .expect("environment");
    context
        .stage(RuleEffect::Actor(companion_actor(id, [2.5, 64.0, 0.5])))
        .expect("actor");
    context.preload_companion_action(companion_envelope(
        id,
        30,
        CompanionAction::MineHold { target },
    ));
    context.preload_inventory(actor, inventory);
    context.preload_block(observation(BlockPos::new(2, 65, 0), AIR));
    context.preload_block(observation(BlockPos::new(2, 65, 1), AIR));
    for (pos, block) in cells {
        context.preload_block(observation(*pos, *block));
    }
    actor
}

fn chest_record(pos: BlockPos, contents: &[ItemStack]) -> ContainerRecord {
    let mut slots = [ItemStack::default(); 27];
    for (index, stack) in contents.iter().enumerate() {
        slots[index] = *stack;
    }
    ContainerRecord {
        reference: ContainerRef::try_new(
            ChunkPos::new(pos.x() >> 4, pos.z() >> 4),
            ContainerKind::Chest,
            0,
            1,
        )
        .expect("container reference"),
        revision: 1,
        slots: ContainerSlots::Chest(slots),
    }
}

/// Fills every drop slot of the target's chunk so the human output preflight
/// refuses with `DropCapacity`.
fn fill_drop_table(context: &mut TickContext<'_>, pos: BlockPos) {
    let chunk = ChunkPos::new(pos.x() >> 4, pos.z() >> 4);
    for slot in 0..32u8 {
        context.preload_drop(DropRecord {
            id: DropId::try_new(0, chunk, slot, 1).expect("drop id"),
            position: FiniteVec3::try_new([0.5, 65.5, 1.5]).expect("position"),
            stack: ItemStack {
                item: ITEM_STONE,
                count: 1,
                durability: 0,
            },
            pickup_delay: 0,
            age: 0,
        });
    }
}

/// Observable authority state a refusal must leave untouched: probed cells with
/// their full observations, actor inventories, mining progress and events.
#[derive(Clone, Debug, PartialEq)]
struct Probe {
    cells: Vec<(BlockPos, Option<BlockObservation>)>,
    inventories: Vec<(ActorKey, Option<InventoryRecord>)>,
    progress: Vec<(ActorKey, Option<MiningProgress>)>,
    events: usize,
}

fn probe(context: &TickContext<'_>, cells: &[BlockPos], actors: &[ActorKey]) -> Probe {
    let view = context.read();
    Probe {
        cells: cells
            .iter()
            .map(|pos| (*pos, view.observation(Dimension::OVERWORLD, *pos)))
            .collect(),
        inventories: actors
            .iter()
            .map(|key| (*key, view.inventory(*key).copied()))
            .collect(),
        progress: actors
            .iter()
            .map(|key| (*key, view.mining(*key).cloned()))
            .collect(),
        events: context.events().len(),
    }
}

fn tool(item: u16, durability: u16) -> ItemStack {
    ItemStack {
        item,
        count: 1,
        durability,
    }
}

#[test]
fn workbench_uses_wood_ticks_and_companion_can_mine_it() {
    let target = BlockPos::new(0, 65, 1);
    let mut state = authority();
    let session = state
        .admit(admitted(21, "bench"), TransportKind::Memory)
        .unwrap();
    let mut context = harness_context(&mut state);
    let actor = south_scene(&mut context, session, ItemStack::default(), &[(target, 45)]);
    let call = mine_call(actor);
    for elapsed in 1..15 {
        provider::run(&mut context, call).unwrap();
        let progress = context.read().mining(actor).unwrap();
        assert_eq!((progress.elapsed, progress.required), (elapsed, 15));
    }
    provider::run(&mut context, call).unwrap();
    assert_eq!(
        context
            .read()
            .observation(Dimension::OVERWORLD, target)
            .unwrap()
            .block,
        AIR
    );
    assert_eq!(context.read().mining(actor), None);

    let mut state = authority();
    let mut context = harness_context(&mut state);
    let id = companion_id();
    let target = BlockPos::new(2, 65, 2);
    let actor = companion_scene(
        &mut context,
        id,
        target,
        InventoryRecord::empty(),
        &[(target, 45)],
    );
    let call = mine_call(actor);
    for _ in 0..15 {
        provider::run(&mut context, call).unwrap();
    }
    assert_eq!(
        context
            .read()
            .observation(Dimension::OVERWORLD, target)
            .unwrap()
            .block,
        AIR
    );
    assert!(
        context
            .read()
            .inventory(actor)
            .unwrap()
            .slots
            .iter()
            .any(|stack| stack.item == 38 && stack.count == 1)
    );
}

#[test]
fn companion_rejects_plants_and_keeps_leaves_body_only() {
    for block in [WHEAT_STAGE0, SHORT_GRASS] {
        let mut state = authority();
        let mut context = harness_context(&mut state);
        let id = companion_id();
        let target = BlockPos::new(2, 65, 2);
        let actor = companion_scene(
            &mut context,
            id,
            target,
            InventoryRecord::empty(),
            &[(target, block)],
        );
        provider::run(&mut context, mine_call(actor)).unwrap();
        assert_eq!(
            context
                .read()
                .observation(Dimension::OVERWORLD, target)
                .unwrap()
                .block,
            block
        );
        assert_eq!(context.read().mining(actor), None);
        assert!(context.read().drops(overworld_key(target)).is_empty());
    }
    let mut state = authority();
    let mut context = harness_context(&mut state);
    let id = companion_id();
    let target = BlockPos::new(2, 65, 2);
    let actor = companion_scene(
        &mut context,
        id,
        target,
        InventoryRecord::empty(),
        &[(target, 19)],
    );
    let mut env = environment();
    env.seed = 11;
    context.stage(RuleEffect::Environment(env)).unwrap();
    for _ in 0..5 {
        provider::run(&mut context, mine_call(actor)).unwrap();
    }
    assert_eq!(
        context
            .read()
            .observation(Dimension::OVERWORLD, target)
            .unwrap()
            .block,
        AIR
    );
    let slots = context.read().inventory(actor).unwrap().slots;
    assert_eq!(slots.iter().filter(|stack| stack.item == 22).count(), 1);
    assert!(slots.iter().all(|stack| stack.item != 57));
    assert!(context.read().drops(overworld_key(target)).is_empty());
}

#[test]
fn companion_wrong_tool_body_is_suppressed_and_sapling_spares_tool() {
    let target = BlockPos::new(2, 65, 2);
    let id = companion_id();
    let mut state = authority();
    let mut context = harness_context(&mut state);
    let actor = companion_scene(
        &mut context,
        id,
        target,
        InventoryRecord::empty(),
        &[(target, 12)],
    );
    for _ in 0..30 {
        provider::run(&mut context, mine_call(actor)).unwrap();
    }
    assert_eq!(
        context
            .read()
            .observation(Dimension::OVERWORLD, target)
            .unwrap()
            .block,
        AIR
    );
    assert!(
        context
            .read()
            .inventory(actor)
            .unwrap()
            .slots
            .iter()
            .all(|stack| stack.item == 0)
    );
    assert!(context.read().drops(overworld_key(target)).is_empty());

    let mut state = authority();
    let mut context = harness_context(&mut state);
    let mut inventory = InventoryRecord::empty();
    inventory.slots[0] = tool(ITEM_STONE_PICKAXE, 18);
    let actor = companion_scene(&mut context, id, target, inventory, &[(target, SAPLING)]);
    provider::run(&mut context, mine_call(actor)).unwrap();
    let slots = context.read().inventory(actor).unwrap().slots;
    assert_eq!(slots[0], tool(ITEM_STONE_PICKAXE, 18));
    assert!(
        slots
            .iter()
            .any(|stack| stack.item == 57 && stack.count == 1)
    );
    assert!(context.read().drops(overworld_key(target)).is_empty());
}

/// The frozen progress key rule (`stepMiningProgress` in
/// `packages/server/sim/entity/mining.go`): an unchanged target/block/tool key
/// increments once per tick, a changed key restarts at 1, and wrong shapes
/// refuse without effect.
#[test]
fn key_switch_restarts_at_one() {
    let target = BlockPos::new(0, 65, 1);
    let mut state = authority();
    let session = state
        .admit(admitted(1, "Ada"), TransportKind::Memory)
        .expect("session");
    let mut context = harness_context(&mut state);
    let actor = south_scene(
        &mut context,
        session,
        tool(ITEM_STONE_PICKAXE, 131),
        &[(target, STONE)],
    );
    let call = mine_call(actor);
    for tick in 1..=4u32 {
        let report = provider::run(&mut context, call).expect("progress");
        assert_eq!(
            report,
            PhaseReport {
                examined: 1,
                applied: 1,
                carried: 0,
                rejected: 0
            }
        );
        assert_eq!(
            context.read().mining(actor).expect("progress").elapsed,
            tick,
            "unchanged key increments once per tick"
        );
        assert_eq!(
            context.read().mining(actor).expect("progress").required,
            15,
            "stone pick on stone needs 15 ticks"
        );
    }
    // Switching the held tool changes the progress key: the counter restarts
    // at 1 against the new tool's required value.
    let mut switched = InventoryRecord::empty();
    switched.slots[0] = tool(ITEM_IRON_PICKAXE, 50);
    context.preload_inventory(actor, switched);
    provider::run(&mut context, call).expect("restarted progress");
    let progress = context.read().mining(actor).expect("progress");
    assert_eq!(progress.elapsed, 1, "changed key restarts at one");
    assert_eq!(progress.required, 8, "iron pick on stone needs 8 ticks");
    assert_eq!(progress.target, target);
    assert_eq!(progress.observed_block, STONE);
    assert_eq!(progress.tool.item, ITEM_IRON_PICKAXE);
    assert_eq!(
        context
            .read()
            .observation(Dimension::OVERWORLD, target)
            .expect("cell")
            .block,
        STONE
    );

    // Wrong shapes refuse without effect: the staged progress is untouched.
    let before = probe(&context, &[target], &[actor]);
    assert!(
        provider::run(
            &mut context,
            RuleCall {
                phase: RulePhase::Interaction,
                actor: Some(actor),
                command: None,
                internal: None
            }
        )
        .is_err()
    );
    assert!(
        provider::run(
            &mut context,
            RuleCall {
                phase: RulePhase::MiningStep,
                actor: None,
                command: None,
                internal: None
            }
        )
        .is_err()
    );
    assert_eq!(probe(&context, &[target], &[actor]), before);
}

/// The frozen tool-tier rows (`miningRule`): a fixture stone pick on stone
/// completes exactly on the 15th tick with one wear point, an iron pick on the
/// 8th, and a refused container-body output preserves the body, the tool and
/// the contents while still clearing the human attempt.
#[test]
fn stone_pick_15_iron_8() {
    let target = BlockPos::new(0, 65, 1);
    let mut state = authority();
    let session = state
        .admit(admitted(1, "Ada"), TransportKind::Memory)
        .expect("session");
    let mut context = harness_context(&mut state);
    let actor = south_scene(
        &mut context,
        session,
        tool(ITEM_STONE_PICKAXE, 131),
        &[(target, STONE)],
    );
    let call = mine_call(actor);
    for tick in 1..15u32 {
        provider::run(&mut context, call).expect("progress");
        assert_eq!(
            context.read().mining(actor).expect("progress").elapsed,
            tick
        );
    }
    let report = provider::run(&mut context, call).expect("completion on the 15th tick");
    assert_eq!(
        report,
        PhaseReport {
            examined: 1,
            applied: 1,
            carried: 0,
            rejected: 0
        }
    );
    let cleared = context
        .read()
        .observation(Dimension::OVERWORLD, target)
        .expect("cell");
    assert_eq!(cleared.block, AIR);
    assert_eq!(cleared.revision, 2);
    assert_eq!(
        context.read().inventory(actor).expect("inventory").slots[0],
        tool(ITEM_STONE_PICKAXE, 130),
        "successful tool use wears exactly one point"
    );
    assert_eq!(
        context.read().mining(actor),
        None,
        "completion clears progress"
    );
    assert_eq!(context.events().len(), 0);

    // Iron pick on stone completes exactly on the 8th tick.
    let mut state = authority();
    let session = state
        .admit(admitted(2, "Bea"), TransportKind::Memory)
        .expect("session");
    let mut context = harness_context(&mut state);
    let actor = south_scene(
        &mut context,
        session,
        tool(ITEM_IRON_PICKAXE, 200),
        &[(target, STONE)],
    );
    let call = mine_call(actor);
    for _ in 1..8u32 {
        provider::run(&mut context, call).expect("progress");
    }
    assert_eq!(context.read().mining(actor).expect("progress").elapsed, 7);
    provider::run(&mut context, call).expect("completion on the 8th tick");
    assert_eq!(
        context
            .read()
            .observation(Dimension::OVERWORLD, target)
            .expect("cell")
            .block,
        AIR
    );
    assert_eq!(
        context.read().inventory(actor).expect("inventory").slots[0],
        tool(ITEM_IRON_PICKAXE, 199)
    );
    assert_eq!(context.read().mining(actor), None);

    // A refused container-body output preserves the body, the contents and the
    // tool: the chest, its staged record and the pick are unchanged, while the
    // completed human attempt still clears its progress.
    let mut state = authority();
    let session = state
        .admit(admitted(3, "Cyd"), TransportKind::Memory)
        .expect("session");
    let mut context = harness_context(&mut state);
    let actor = south_scene(
        &mut context,
        session,
        tool(ITEM_STONE_PICKAXE, 131),
        &[(target, CHEST)],
    );
    context.preload_container(chest_record(
        target,
        &[
            ItemStack {
                item: ITEM_DIRT,
                count: 3,
                durability: 0,
            },
            ItemStack {
                item: ITEM_STONE,
                count: 2,
                durability: 0,
            },
        ],
    ));
    fill_drop_table(&mut context, target);
    let call = mine_call(actor);
    for _ in 1..15u32 {
        provider::run(&mut context, call).expect("progress");
    }
    let before = probe(&context, &[target], &[actor]);
    assert!(
        provider::run(&mut context, call).is_err(),
        "full output refuses"
    );
    let after = probe(&context, &[target], &[actor]);
    assert_eq!(after.cells, before.cells, "refused body keeps its block");
    assert_eq!(
        after.inventories, before.inventories,
        "refused attempt never wears the tool"
    );
    assert_eq!(after.events, 0);
    assert_eq!(
        context.read().mining(actor),
        None,
        "human attempt clears even on refusal"
    );
    assert_eq!(
        context
            .read()
            .observation(Dimension::OVERWORLD, target)
            .expect("cell")
            .block,
        CHEST
    );
}

/// Completion outcome split (`advanceMining` versus `completeCompanionMining`
/// in `packages/server/sim/entity/mining.go`): a refused human attempt resets
/// to no progress, while a companion with a full inventory keeps saturated
/// progress for retry and settles exactly one completion after a slot frees.
#[test]
fn human_failure_reset_companion_saturate() {
    // Human dirt completion against a full drop table refuses and resets.
    let target = BlockPos::new(0, 65, 1);
    let mut state = authority();
    let session = state
        .admit(admitted(1, "Ada"), TransportKind::Memory)
        .expect("session");
    let mut context = harness_context(&mut state);
    let actor = south_scene(
        &mut context,
        session,
        ItemStack {
            item: 0,
            count: 0,
            durability: 0,
        },
        &[(target, DIRT)],
    );
    fill_drop_table(&mut context, target);
    let call = mine_call(actor);
    for _ in 1..5u32 {
        provider::run(&mut context, call).expect("progress");
    }
    assert_eq!(context.read().mining(actor).expect("progress").elapsed, 4);
    assert!(
        provider::run(&mut context, call).is_err(),
        "full output refuses"
    );
    assert_eq!(
        context.read().mining(actor),
        None,
        "human failure resets progress"
    );
    assert_eq!(
        context
            .read()
            .observation(Dimension::OVERWORLD, target)
            .expect("cell")
            .block,
        DIRT
    );

    // Companion dirt completion with a full backpack keeps saturated progress.
    let mut state = authority();
    let id = companion_id();
    let mut full = InventoryRecord::empty();
    for slot in full.slots.iter_mut() {
        *slot = ItemStack {
            item: ITEM_STONE,
            count: 64,
            durability: 0,
        };
    }
    let target = BlockPos::new(2, 65, 2);
    let mut context = harness_context(&mut state);
    let actor = companion_scene(&mut context, id, target, full, &[(target, DIRT)]);
    let call = mine_call(actor);
    for _ in 1..5u32 {
        provider::run(&mut context, call).expect("progress");
    }
    assert!(
        provider::run(&mut context, call).is_err(),
        "full inventory refuses"
    );
    let kept = context
        .read()
        .mining(actor)
        .expect("saturated progress is kept");
    assert_eq!((kept.elapsed, kept.required), (5, 5));
    assert_eq!(
        context
            .read()
            .observation(Dimension::OVERWORLD, target)
            .expect("cell")
            .block,
        DIRT,
        "refused companion output keeps its block"
    );
    // Freeing one slot lets the retry settle exactly one completion: the dirt
    // clears into the freed slot and progress resets. The freed slot sits
    // outside the selected held slot so the progress key is unchanged.
    let mut freed = full;
    freed.slots[5] = ItemStack::default();
    context.preload_inventory(actor, freed);
    let report = provider::run(&mut context, call).expect("retry completes");
    assert_eq!(report.applied, 1);
    assert_eq!(
        context
            .read()
            .observation(Dimension::OVERWORLD, target)
            .expect("cell")
            .block,
        AIR
    );
    assert_eq!(
        context.read().inventory(actor).expect("inventory").slots[5],
        ItemStack {
            item: ITEM_DIRT,
            count: 1,
            durability: 0
        }
    );
    assert_eq!(context.read().mining(actor), None);
}

/// Atomic completion across container bodies and paired blocks: the accepted
/// transaction commits the block write, the captured contents and the tool wear
/// together, and a refused output commits nothing. The resolver footprint
/// covers the hit cell; container read-back and slot lifecycle stay with the
/// container and drop providers.
#[test]
fn containers_and_paired_blocks_atomic() {
    // Companion chest mining credits the body plus both contents atomically
    // with the wear: one completion moves every stack or none.
    let target = BlockPos::new(2, 65, 2);
    let mut state = authority();
    let id = companion_id();
    let mut context = harness_context(&mut state);
    let mut inventory = InventoryRecord::empty();
    inventory.slots[0] = tool(ITEM_STONE_PICKAXE, 100);
    let actor = companion_scene(&mut context, id, target, inventory, &[(target, CHEST)]);
    context.preload_container(chest_record(
        target,
        &[
            ItemStack {
                item: ITEM_DIRT,
                count: 3,
                durability: 0,
            },
            ItemStack {
                item: ITEM_STONE,
                count: 2,
                durability: 0,
            },
        ],
    ));
    let call = mine_call(actor);
    for _ in 1..15u32 {
        provider::run(&mut context, call).expect("progress");
    }
    provider::run(&mut context, call).expect("container completion");
    assert_eq!(
        context
            .read()
            .observation(Dimension::OVERWORLD, target)
            .expect("cell")
            .block,
        AIR
    );
    let after = context.read().inventory(actor).expect("inventory").slots;
    // Credit precedes one selected-slot wear in the accepted companion mine.
    assert_eq!(
        after[0],
        tool(ITEM_STONE_PICKAXE, 99),
        "companion tool wears once"
    );
    assert_eq!(
        after[1],
        ItemStack {
            item: ITEM_CHEST,
            count: 1,
            durability: 0
        },
        "body first"
    );
    assert_eq!(
        after[2],
        ItemStack {
            item: ITEM_DIRT,
            count: 3,
            durability: 0
        }
    );
    assert_eq!(
        after[3],
        ItemStack {
            item: ITEM_STONE,
            count: 2,
            durability: 0
        }
    );
    assert_eq!(context.read().mining(actor), None);

    // Human door mining clears both observed halves in one settlement.
    let target = BlockPos::new(0, 65, 1);
    let upper = BlockPos::new(0, 66, 1);
    let mut state = authority();
    let session = state
        .admit(admitted(2, "Bea"), TransportKind::Memory)
        .expect("session");
    let mut context = harness_context(&mut state);
    let actor = south_scene(
        &mut context,
        session,
        tool(ITEM_STONE_PICKAXE, 77),
        &[(target, DOOR_LOWER_SOUTH_CLOSED), (upper, DOOR_UPPER)],
    );
    let call = mine_call(actor);
    for _ in 1..15u32 {
        provider::run(&mut context, call).expect("progress");
    }
    provider::run(&mut context, call).expect("paired completion");
    assert_eq!(
        context
            .read()
            .observation(Dimension::OVERWORLD, target)
            .expect("cell")
            .block,
        AIR
    );
    assert_eq!(
        context
            .read()
            .observation(Dimension::OVERWORLD, upper)
            .expect("partner")
            .block,
        AIR,
        "partner half clears atomically"
    );
    assert_eq!(
        context.read().inventory(actor).expect("inventory").slots[0],
        tool(ITEM_STONE_PICKAXE, 76)
    );
    assert_eq!(context.read().mining(actor), None);

    // Human furnace mining against a full drop table refuses atomically: the
    // furnace, its staged record and the tool are all unchanged.
    let mut state = authority();
    let session = state
        .admit(admitted(3, "Cyd"), TransportKind::Memory)
        .expect("session");
    let mut context = harness_context(&mut state);
    let actor = south_scene(
        &mut context,
        session,
        tool(ITEM_STONE_PICKAXE, 60),
        &[(target, FURNACE)],
    );
    context.preload_container(ContainerRecord {
        reference: ContainerRef::try_new(
            ChunkPos::new(target.x() >> 4, target.z() >> 4),
            ContainerKind::Furnace,
            0,
            1,
        )
        .expect("container reference"),
        revision: 1,
        slots: ContainerSlots::Furnace {
            slots: [
                ItemStack {
                    item: ITEM_DIRT,
                    count: 1,
                    durability: 0,
                },
                ItemStack::default(),
                ItemStack::default(),
            ],
            fuel: 0,
            progress: 0,
        },
    });
    fill_drop_table(&mut context, target);
    let call = mine_call(actor);
    for _ in 1..15u32 {
        provider::run(&mut context, call).expect("progress");
    }
    assert!(
        provider::run(&mut context, call).is_err(),
        "full output refuses"
    );
    assert_eq!(
        context
            .read()
            .observation(Dimension::OVERWORLD, target)
            .expect("cell")
            .block,
        FURNACE
    );
    assert_eq!(
        context.read().inventory(actor).expect("inventory").slots[0],
        tool(ITEM_STONE_PICKAXE, 60),
        "refused furnace mining never wears the tool"
    );
    assert_eq!(context.read().mining(actor), None);
}

/// Tool end-of-life and wear exemptions (`consumeMiningToolDurability` in
/// `packages/server/sim/entity/mining.go`): durability one becomes the
/// registered broken form, while crop with an intact hoe, saplings of any
/// held item and intact swords never wear. Snow clears with no drops and no
/// capacity gate; wild grass completes in one tick and only a successful
/// position-stable seed roll requires output capacity.
#[test]
fn last_durability_and_exemptions() {
    // Durability one becomes the registered broken form on completion.
    let target = BlockPos::new(0, 65, 1);
    let mut state = authority();
    let session = state
        .admit(admitted(1, "Ada"), TransportKind::Memory)
        .expect("session");
    let mut context = harness_context(&mut state);
    let actor = south_scene(
        &mut context,
        session,
        tool(ITEM_STONE_PICKAXE, 1),
        &[(target, STONE)],
    );
    let call = mine_call(actor);
    for _ in 1..15u32 {
        provider::run(&mut context, call).expect("progress");
    }
    provider::run(&mut context, call).expect("last-point completion");
    assert_eq!(
        context.read().inventory(actor).expect("inventory").slots[0],
        ItemStack {
            item: ITEM_BROKEN_STONE_PICKAXE,
            count: 1,
            durability: 0
        }
    );
    assert_eq!(
        context
            .read()
            .observation(Dimension::OVERWORLD, target)
            .expect("cell")
            .block,
        AIR
    );

    // Crop with an intact hoe is exempt: the block clears, the hoe keeps its
    // durability.
    let mut state = authority();
    let session = state
        .admit(admitted(2, "Bea"), TransportKind::Memory)
        .expect("session");
    let mut context = harness_context(&mut state);
    let actor = south_scene(
        &mut context,
        session,
        tool(ITEM_STONE_HOE, 40),
        &[(target, WHEAT_STAGE0)],
    );
    let call = mine_call(actor);
    provider::run(&mut context, call).expect("one-tick crop completion");
    assert_eq!(
        context
            .read()
            .observation(Dimension::OVERWORLD, target)
            .expect("cell")
            .block,
        AIR
    );
    assert_eq!(
        context.read().inventory(actor).expect("inventory").slots[0],
        tool(ITEM_STONE_HOE, 40),
        "crop with an intact hoe never wears"
    );

    // Sapling of any held item is exempt.
    let mut state = authority();
    let session = state
        .admit(admitted(3, "Cyd"), TransportKind::Memory)
        .expect("session");
    let mut context = harness_context(&mut state);
    let actor = south_scene(
        &mut context,
        session,
        tool(ITEM_STONE_PICKAXE, 77),
        &[(target, SAPLING)],
    );
    let call = mine_call(actor);
    provider::run(&mut context, call).expect("one-tick sapling completion");
    assert_eq!(
        context
            .read()
            .observation(Dimension::OVERWORLD, target)
            .expect("cell")
            .block,
        AIR
    );
    assert_eq!(
        context.read().inventory(actor).expect("inventory").slots[0],
        tool(ITEM_STONE_PICKAXE, 77),
        "sapling mining never wears"
    );

    // An intact sword on any block is exempt.
    let mut state = authority();
    let session = state
        .admit(admitted(4, "Dee"), TransportKind::Memory)
        .expect("session");
    let mut context = harness_context(&mut state);
    let actor = south_scene(
        &mut context,
        session,
        tool(ITEM_WOODEN_SWORD, 60),
        &[(target, DIRT)],
    );
    let call = mine_call(actor);
    for _ in 1..5u32 {
        provider::run(&mut context, call).expect("progress");
    }
    provider::run(&mut context, call).expect("sword completion");
    assert_eq!(
        context
            .read()
            .observation(Dimension::OVERWORLD, target)
            .expect("cell")
            .block,
        AIR
    );
    assert_eq!(
        context.read().inventory(actor).expect("inventory").slots[0],
        tool(ITEM_WOODEN_SWORD, 60),
        "intact sword mining never wears"
    );

    // Snow clears with no drops and no capacity gate, even against a full
    // drop table, through the human-only clear-only settlement.
    let mut state = authority();
    let session = state
        .admit(admitted(5, "Eli"), TransportKind::Memory)
        .expect("session");
    let mut context = harness_context(&mut state);
    let actor = south_scene(
        &mut context,
        session,
        ItemStack {
            item: 0,
            count: 0,
            durability: 0,
        },
        &[(target, SNOW_LAYER1)],
    );
    fill_drop_table(&mut context, target);
    let call = mine_call(actor);
    provider::run(&mut context, call).expect("one-tick snow clear");
    assert_eq!(
        context
            .read()
            .observation(Dimension::OVERWORLD, target)
            .expect("cell")
            .block,
        AIR
    );
    assert_eq!(
        context.read().drops(overworld_key(target)).len(),
        32,
        "snow stages no drops"
    );
    assert_eq!(context.read().mining(actor), None);

    // Short grass completes in one tick and never wears the selected tool.
    let mut state = authority();
    let session = state
        .admit(admitted(6, "Fay"), TransportKind::Memory)
        .expect("session");
    let mut context = harness_context(&mut state);
    let actor = south_scene(
        &mut context,
        session,
        tool(ITEM_STONE_PICKAXE, 90),
        &[(target, SHORT_GRASS)],
    );
    let call = mine_call(actor);
    provider::run(&mut context, call).expect("grass completes in one tick");
    assert_eq!(
        context
            .read()
            .observation(Dimension::OVERWORLD, target)
            .expect("cell")
            .block,
        AIR,
        "grass clears on completion"
    );
    assert_eq!(
        context.read().inventory(actor).expect("inventory").slots[0],
        tool(ITEM_STONE_PICKAXE, 90),
        "grass never wears"
    );
    assert_eq!(context.read().mining(actor), None);

    // Unlisted blocks never progress: bedrock clears any attempt immediately.
    let mut state = authority();
    let session = state
        .admit(admitted(7, "Gus"), TransportKind::Memory)
        .expect("session");
    let mut context = harness_context(&mut state);
    let actor = south_scene(
        &mut context,
        session,
        tool(ITEM_STONE_PICKAXE, 90),
        &[(target, BEDROCK)],
    );
    let call = mine_call(actor);
    let report = provider::run(&mut context, call).expect("bedrock clears");
    assert_eq!(report.applied, 0);
    assert_eq!(context.read().mining(actor), None);
    assert_eq!(
        context
            .read()
            .observation(Dimension::OVERWORLD, target)
            .expect("cell")
            .block,
        BEDROCK
    );

    // Active bow draw suppresses mining resolution: progress clears and the
    // block is untouched.
    let mut state = authority();
    let session = state
        .admit(admitted(8, "Hal"), TransportKind::Memory)
        .expect("session");
    let mut context = harness_context(&mut state);
    let actor = ActorKey::Player(session);
    context
        .stage(RuleEffect::Environment(environment()))
        .expect("environment");
    context
        .stage(RuleEffect::Actor(player_actor(
            session,
            [0.5, 64.0, 0.5],
            std::f32::consts::PI,
            0.0,
        )))
        .expect("actor");
    context
        .stage(RuleEffect::Runtime(human_runtime(
            actor,
            control(true, std::f32::consts::PI, 0.0),
            true,
        )))
        .expect("runtime");
    let mut inventory = InventoryRecord::empty();
    inventory.slots[0] = tool(ITEM_STONE_PICKAXE, 90);
    context.preload_inventory(actor, inventory);
    context.preload_block(observation(BlockPos::new(0, 65, 0), AIR));
    context.preload_block(observation(target, STONE));
    context.preload_mining(MiningProgress {
        actor,
        dimension: Dimension::OVERWORLD,
        target,
        observed_block: STONE,
        tool_slot: HotbarSlot::new(0).expect("slot"),
        tool: tool(ITEM_STONE_PICKAXE, 90),
        elapsed: 6,
        required: 15,
        last_tick: 0,
    });
    provider::run(&mut context, mine_call(actor)).expect("bow suppresses");
    assert_eq!(
        context.read().mining(actor),
        None,
        "bow draw clears progress"
    );
    assert_eq!(
        context
            .read()
            .observation(Dimension::OVERWORLD, target)
            .expect("cell")
            .block,
        STONE
    );
}

#[test]
fn bucket_suppression_is_tick_local() {
    let mut state = authority();
    let session = state
        .admit(admitted(31, "Bucket"), TransportKind::Memory)
        .unwrap();
    let target = BlockPos::new(0, 65, 1);
    {
        let mut context = harness_context(&mut state);
        let actor = south_scene(
            &mut context,
            session,
            tool(ITEM_STONE_PICKAXE, 90),
            &[(target, STONE)],
        );
        provider::run(&mut context, mine_call(actor)).unwrap();
        assert_eq!(context.read().mining(actor).unwrap().elapsed, 1);
        let inventory = *context.read().inventory(actor).unwrap();
        context.check_mining_suppression(actor).unwrap();
        context.suppress_mining(actor).unwrap();
        provider::run(&mut context, mine_call(actor)).unwrap();
        assert_eq!(context.read().mining(actor), None);
        assert_eq!(
            context.read().block(Dimension::OVERWORLD, target),
            Some(STONE)
        );
        assert_eq!(context.read().inventory(actor), Some(&inventory));
    }
    let mut context = harness_context(&mut state);
    let actor = south_scene(
        &mut context,
        session,
        tool(ITEM_STONE_PICKAXE, 90),
        &[(target, STONE)],
    );
    assert!(!context.mining_suppressed(actor));
    provider::run(&mut context, mine_call(actor)).unwrap();
    assert_eq!(context.read().mining(actor).unwrap().elapsed, 1);
}

/// Progress pins the solid behind transparent cells: the tracker and the
/// completion resolver agree through water and open doors, the front cells
/// stay untouched, and the saturated tick clears the target with one wear
/// point and one output batch.
#[test]
fn progress_pins_target_behind_transparent_cells() {
    const WATER_SOURCE: u16 = 27; // `core.WaterSourceID`
    const WATER_FLOWING: u16 = 34; // `core.WaterLevel7ID`
    const DOOR_LOWER_SOUTH_OPEN: u16 = 63; // `core.DoorLowerSouthOpen`
    for corridor in [WATER_SOURCE, WATER_FLOWING, DOOR_LOWER_SOUTH_OPEN] {
        let front = BlockPos::new(0, 65, 1);
        let target = BlockPos::new(0, 65, 2);
        let mut state = authority();
        let session = state
            .admit(admitted(1, "Ada"), TransportKind::Memory)
            .expect("session");
        let mut context = harness_context(&mut state);
        let actor = south_scene(
            &mut context,
            session,
            tool(ITEM_STONE_PICKAXE, 131),
            &[(front, corridor), (target, STONE)],
        );
        let call = mine_call(actor);
        for tick in 1..15u32 {
            provider::run(&mut context, call).expect("progress");
            let progress = context.read().mining(actor).expect("pinned progress");
            assert_eq!(
                progress.target, target,
                "tick {tick} still tracks the solid behind corridor {corridor}"
            );
            assert_eq!(progress.observed_block, STONE);
        }
        provider::run(&mut context, call).expect("completion");
        assert_eq!(context.read().mining(actor), None);
        let view = context.read();
        assert_eq!(
            view.observation(Dimension::OVERWORLD, target)
                .expect("cell")
                .block,
            AIR,
            "the solid behind corridor {corridor} completes at its required ticks"
        );
        let front_after = view
            .observation(Dimension::OVERWORLD, front)
            .expect("front cell");
        assert_eq!(front_after.block, corridor);
        assert_eq!(front_after.revision, 1);
        assert_eq!(
            view.inventory(actor).expect("inventory").slots[0].durability,
            130,
            "one wear point for the completed mine"
        );
        let drops = view.drops(overworld_key(target));
        assert_eq!(drops.len(), 1);
        assert_eq!(drops[0].stack.item, ITEM_STONE);
    }

    // A corridor the authority never observed clears progress instead of
    // tracking air: the walk cannot certify the missing geometry.
    let target = BlockPos::new(0, 65, 2);
    let mut state = authority();
    let session = state
        .admit(admitted(2, "Bo"), TransportKind::Memory)
        .expect("session");
    let mut context = harness_context(&mut state);
    let actor = south_scene(
        &mut context,
        session,
        tool(ITEM_STONE_PICKAXE, 131),
        &[(target, STONE)],
    );
    let report = provider::run(&mut context, mine_call(actor)).expect("unready clears");
    assert_eq!(report.applied, 0);
    assert_eq!(context.read().mining(actor), None);
}
