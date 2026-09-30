//! Authority-resolved atomic world transaction cases.
//!
//! Every case drives the real resolvers and the real transaction through the
//! harness surfaces: actors staged via `RuleEffect::Actor`, inventories via
//! `preload_inventory`, blocks via `preload_block`, containers via
//! `preload_container`, drop occupancy via `preload_drop`, and the environment
//! via `RuleEffect::Environment`. Each refusal case proves the failure leaves
//! world cells, inventories, containers, drop occupancy and events unchanged
//! by comparing a before/after probe, not just that an `Err` returns.

use mornlea_domain::{
    BlockPos, ChunkPos, CompanionId, ContainerKind, ContainerRef, Dimension, DropId, FiniteVec3,
    HeldActions, LookAngles, MotionState, MotionStateParts, Movement, PlacementIntent,
    PlayerControl, PlayerControlParts, RejectReason, SurvivalState, SurvivalStateParts, Weather,
};
use mornlea_protocol::{AdmittedLogin, LoginStart, admit_login};
use mornlea_server::contracts::*;
use mornlea_server::core::mutation::{
    resolve_companion_mine, resolve_companion_place, resolve_mine, resolve_place,
};
use mornlea_server::state::{AuthorityState, TickContext};
use mornlea_storage::{CompanionBody, ItemStack, PlayerLocation, PlayerSave};

// Stable block numbers mirrored from the frozen Go table
// (`packages/shared/core/block.go` const block).
const AIR: u16 = 0; // `core.AirID`
const STONE: u16 = 2; // `core.StoneID`
const DIRT: u16 = 3; // `core.DirtID`
const BEDROCK: u16 = 5; // `core.BedrockID`
const CHEST: u16 = 11; // `core.ChestID`
// Stable item numbers mirrored from the frozen Go table
// (`packages/shared/core/item.go` const block).
const ITEM_STONE: u16 = 1; // `core.ItemStone`
const ITEM_DIRT: u16 = 2; // `core.ItemDirt`
const ITEM_COAL: u16 = 5; // `core.ItemCoal`
const ITEM_PICKAXE: u16 = 10; // `core.ItemStonePickaxe`
const ITEM_CHEST: u16 = 14; // `core.ItemChest`
const ITEM_BED: u16 = 46; // `core.ItemBed`
const ITEM_WOODEN_SWORD: u16 = 47; // `core.ItemWoodenSword`
// Stone pickaxe durability budget (`core.ItemMaxDurability` via the domain
// durability table, mirrored from `packages/shared/core/item.go`).
const PICKAXE_DURABILITY: u16 = 131;
// Wooden sword durability budget (`core.ItemMaxDurability`).
const SWORD_DURABILITY: u16 = 59;
// East-facing bed forms (`core.BedFootEastID`, `core.BedHeadEastID` in
// `packages/shared/core/block.go`; head is foot + 4 per `bedHeadIDs`).
const BED_FOOT_EAST: u16 = 79;
const BED_HEAD_EAST: u16 = 83;
// One chest holds 27 item slots (`core.ChestSlots`,
// `packages/shared/core/chest.go`).
const CHEST_SLOTS: usize = 27;

fn authority() -> AuthorityState {
    AuthorityState::try_new(
        ServerLimits::try_new(8, 4096, 512, 64, 64, 1_048_576).expect("limits"),
        7,
    )
    .expect("authority")
}

fn admitted(tag: u8, name: &str) -> AdmittedLogin {
    let mut bytes = [0u8; 16];
    bytes[0] = tag;
    bytes[6] = 0x40;
    bytes[8] = 0x80;
    let id = mornlea_domain::PlayerId::try_from_bytes(bytes).expect("player id");
    let start = LoginStart::new(id, name, 8).expect("login start");
    let inbound = LoginStart::decode_inbound(&start.encode().expect("encoded")).expect("inbound");
    admit_login(inbound).expect("admitted login")
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

fn chunk(pos: BlockPos) -> ChunkPos {
    ChunkPos::new(pos.x() >> 4, pos.z() >> 4)
}

fn overworld_key(pos: BlockPos) -> ChunkKey {
    ChunkKey {
        dimension: Dimension::OVERWORLD,
        pos: chunk(pos),
    }
}

fn observation(pos: BlockPos, block: u16) -> BlockObservation {
    observation_at(pos, block, 1, 1)
}

fn observation_at(pos: BlockPos, block: u16, generation: u64, revision: u64) -> BlockObservation {
    BlockObservation::try_new(overworld_key(pos), generation, revision, pos, block)
        .expect("block observation")
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

fn hotbar_inventory(slot: usize, item: u16, count: u8, durability: u16) -> InventoryRecord {
    let mut record = InventoryRecord::empty();
    record.slots[slot] = ItemStack {
        item,
        count,
        durability,
    };
    record
}

fn south_intent() -> PlacementIntent {
    PlacementIntent::try_new(
        LookAngles::try_new(std::f32::consts::PI, 0.0).expect("look"),
        0,
    )
    .expect("placement intent")
}

fn primary_control(yaw: f32, pitch: f32) -> PlayerControl {
    control(yaw, pitch, true)
}

fn idle_control(yaw: f32, pitch: f32) -> PlayerControl {
    control(yaw, pitch, false)
}

fn control(yaw: f32, pitch: f32, primary: bool) -> PlayerControl {
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

/// Observable authority state a refusal must leave untouched: probed cells
/// with their full observations, actor inventories, staged container records,
/// per-chunk drop occupancy and the event count.
#[derive(Clone, Debug, PartialEq)]
struct Probe {
    cells: Vec<(BlockPos, Option<BlockObservation>)>,
    inventories: Vec<(ActorKey, Option<InventoryRecord>)>,
    containers: Vec<(ContainerRef, Option<ContainerRecord>)>,
    drops: Vec<(ChunkKey, usize)>,
    events: usize,
}

fn probe(
    context: &TickContext<'_>,
    cells: &[BlockPos],
    actors: &[ActorKey],
    references: &[ContainerRef],
    chunks: &[ChunkKey],
) -> Probe {
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
        containers: references
            .iter()
            .map(|reference| (*reference, view.container(*reference)))
            .collect(),
        drops: chunks
            .iter()
            .map(|key| (*key, view.drops(*key).len()))
            .collect(),
        events: context.events().len(),
    }
}

fn chest_reference() -> ContainerRef {
    ContainerRef::try_new(ChunkPos::new(0, 0), ContainerKind::Chest, 0, 1).expect("chest reference")
}

/// A chest record whose 27 slots are all full, the Go worst-case mining batch
/// (1 container body + 27 contents, `packages/shared/world/chest_test.go`).
fn full_chest_record() -> ContainerRecord {
    let slots = [ItemStack {
        item: ITEM_STONE,
        count: 64,
        durability: 0,
    }; CHEST_SLOTS];
    ContainerRecord {
        reference: chest_reference(),
        revision: 1,
        slots: ContainerSlots::Chest(slots),
    }
}

/// One occupied drop slot in chunk (0, 0): coal at 64, unmergeable with the
/// stone batch, mirroring the pre-occupy pattern of the Go oracle
/// (`packages/shared/world/chest_test.go`
/// `TestPrepareDropBatchWorstCaseFailureLeavesBytesUnchanged`).
fn occupied_drop(slot: u8) -> DropRecord {
    DropRecord {
        id: DropId::try_new(0, ChunkPos::new(0, 0), slot, 1).expect("drop id"),
        position: FiniteVec3::try_new([0.5, 65.5, 2.5]).expect("drop position"),
        stack: ItemStack {
            item: ITEM_COAL,
            count: 64,
            durability: 0,
        },
        pickup_delay: 0,
        age: 0,
    }
}

#[test]
fn valid_place_debit_and_delta() {
    let mut state = authority();
    let session = state
        .admit(admitted(1, "Ada"), TransportKind::Memory)
        .expect("session");
    let actor = ActorKey::Player(session);
    let mut context = harness_context(&mut state);
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
    context.preload_inventory(actor, hotbar_inventory(0, ITEM_DIRT, 3, 0));
    context.preload_block(observation(BlockPos::new(0, 65, 0), AIR));
    context.preload_block(observation(BlockPos::new(0, 65, 1), AIR));
    context.preload_block(observation(BlockPos::new(0, 65, 2), STONE));

    let resolved = resolve_place(actor, &south_intent(), &context.read()).expect("resolved place");
    let outcome = context
        .transaction()
        .try_place(resolved)
        .expect("committed place");

    assert_eq!(outcome.changed.len(), 1);
    assert_eq!(outcome.changed[0].pos, BlockPos::new(0, 65, 1));
    assert_eq!(outcome.changed[0].block, DIRT);
    assert_eq!(outcome.changed[0].generation, 1);
    assert_eq!(outcome.changed[0].revision, 2);
    assert!(outcome.inventory_changed);
    assert_eq!(outcome.drops_created, 0);
    let inventory = context.read().inventory(actor).expect("inventory");
    assert_eq!(inventory.slots[0].count, 2);
    let placed = context
        .read()
        .observation(Dimension::OVERWORLD, BlockPos::new(0, 65, 1))
        .expect("placed cell");
    assert_eq!(placed.block, DIRT);
    assert_eq!(placed.revision, 2);
    let hit = context
        .read()
        .observation(Dimension::OVERWORLD, BlockPos::new(0, 65, 2))
        .expect("hit cell");
    assert_eq!(hit.block, STONE);
    assert_eq!(hit.revision, 1);
    assert_eq!(context.events().len(), 0);
}

#[test]
fn two_chunk_door_atomic() {
    // The two-cell footprint crossing a chunk boundary is the bed form: a
    // bed's halves lie horizontally (foot at x=15 in chunk 0, east-facing
    // head at x=16 in chunk 1), while a door's halves stack vertically and
    // stay in one column. The atomicity under test is the same transaction.
    let foot = BlockPos::new(15, 65, 0);
    let head = BlockPos::new(16, 65, 0);
    let drop_chunks = [overworld_key(foot), overworld_key(head)];
    let cells = [
        BlockPos::new(15, 67, 0),
        BlockPos::new(15, 66, 0),
        foot,
        head,
        BlockPos::new(15, 64, 0),
        BlockPos::new(16, 64, 0),
    ];
    let bed_look = LookAngles::try_new(-std::f32::consts::FRAC_PI_2, -1.4).expect("look");
    let intent = PlacementIntent::try_new(bed_look, 0).expect("intent");

    // Stale leg: the head cell's revision advances between resolve and commit.
    let mut state = authority();
    let session = state
        .admit(admitted(2, "Bea"), TransportKind::Memory)
        .expect("session");
    let actor = ActorKey::Player(session);
    let mut context = harness_context(&mut state);
    context
        .stage(RuleEffect::Environment(environment()))
        .expect("environment");
    context
        .stage(RuleEffect::Actor(player_actor(
            session,
            [15.5, 66.0, 0.5],
            bed_look.yaw(),
            bed_look.pitch(),
        )))
        .expect("actor");
    context.preload_inventory(actor, hotbar_inventory(0, ITEM_BED, 2, 0));
    for (pos, block) in [
        (BlockPos::new(15, 67, 0), AIR),
        (BlockPos::new(15, 66, 0), AIR),
        (foot, AIR),
        (head, AIR),
        (BlockPos::new(15, 64, 0), STONE),
        (BlockPos::new(16, 64, 0), STONE),
    ] {
        context.preload_block(observation(pos, block));
    }
    let before = probe(&context, &cells, &[actor], &[], &drop_chunks);

    let resolved = resolve_place(actor, &intent, &context.read()).expect("resolved bed place");
    for block in [STONE, AIR] {
        let current = context
            .read()
            .observation(Dimension::OVERWORLD, head)
            .expect("head");
        let unrelated = BlockWrite::try_new(current, block).expect("revision advance");
        context
            .transaction()
            .try_system(SystemRule::Support, vec![unrelated])
            .expect("unrelated write");
    }
    let refused = context.transaction().try_place(resolved);
    assert_eq!(refused, Err(RuleReject::StaleObservation));

    let after = probe(&context, &cells, &[actor], &[], &drop_chunks);
    // Both footprint cells stay air; only the unrelated revision advance on
    // the head cell separates the probes, and nothing else moved.
    let mut expected = before.clone();
    expected.cells[3].1 = Some(observation_at(head, AIR, 1, 3));
    assert_eq!(after, expected);

    // Clean leg: the same placement commits both cells exactly once.
    let mut state = authority();
    let session = state
        .admit(admitted(2, "Bea"), TransportKind::Memory)
        .expect("session");
    let actor = ActorKey::Player(session);
    let mut context = harness_context(&mut state);
    context
        .stage(RuleEffect::Environment(environment()))
        .expect("environment");
    context
        .stage(RuleEffect::Actor(player_actor(
            session,
            [15.5, 66.0, 0.5],
            bed_look.yaw(),
            bed_look.pitch(),
        )))
        .expect("actor");
    context.preload_inventory(actor, hotbar_inventory(0, ITEM_BED, 2, 0));
    for (pos, block) in [
        (BlockPos::new(15, 67, 0), AIR),
        (BlockPos::new(15, 66, 0), AIR),
        (foot, AIR),
        (head, AIR),
        (BlockPos::new(15, 64, 0), STONE),
        (BlockPos::new(16, 64, 0), STONE),
    ] {
        context.preload_block(observation(pos, block));
    }
    let resolved = resolve_place(actor, &intent, &context.read()).expect("resolved bed place");
    let outcome = context
        .transaction()
        .try_place(resolved)
        .expect("committed bed place");

    assert_eq!(outcome.changed.len(), 2);
    let foot_change = outcome
        .changed
        .iter()
        .find(|change| change.pos == foot)
        .expect("foot change");
    assert_eq!(foot_change.block, BED_FOOT_EAST);
    assert_eq!(foot_change.revision, 2);
    let head_change = outcome
        .changed
        .iter()
        .find(|change| change.pos == head)
        .expect("head change");
    assert_eq!(head_change.block, BED_HEAD_EAST);
    assert_eq!(head_change.revision, 2);
    assert!(outcome.inventory_changed);
    assert_eq!(
        context.read().inventory(actor).expect("inventory").slots[0].count,
        1
    );
    assert_eq!(context.events().len(), 0);
}

#[test]
fn human_drop_companion_credit() {
    let companion = companion_id();
    let player_cells = [
        BlockPos::new(0, 65, 0),
        BlockPos::new(0, 65, 1),
        BlockPos::new(0, 65, 2),
    ];
    let chest_pos = BlockPos::new(0, 65, 2);
    let drop_chunk = overworld_key(chest_pos);

    // Human leg, full drop capacity: five occupied drop slots plus the
    // worst-case 28-stack batch exceed the 32 per-chunk slots.
    let mut state = authority();
    let session = state
        .admit(admitted(3, "Cara"), TransportKind::Memory)
        .expect("session");
    let player = ActorKey::Player(session);
    let mut context = harness_context(&mut state);
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
    for (pos, block) in [
        (BlockPos::new(0, 65, 0), AIR),
        (BlockPos::new(0, 65, 1), AIR),
        (chest_pos, CHEST),
    ] {
        context.preload_block(observation(pos, block));
    }
    context.preload_container(full_chest_record());
    for slot in 0..5u8 {
        context.preload_drop(occupied_drop(slot));
    }
    // Preflight-order corner: the drop capacity is already full, but with no
    // inventory record staged the tool basis refusal wins — item/tool before
    // output capacity in the frozen order.
    let corner = resolve_mine(
        player,
        &primary_control(std::f32::consts::PI, 0.0),
        &context.read(),
    );
    assert_eq!(corner, Err(RuleReject::StaleObservation));
    context.preload_inventory(
        player,
        hotbar_inventory(0, ITEM_PICKAXE, 1, PICKAXE_DURABILITY),
    );
    let before = probe(
        &context,
        &player_cells,
        &[player],
        &[chest_reference()],
        &[drop_chunk],
    );

    let refused = resolve_mine(
        player,
        &primary_control(std::f32::consts::PI, 0.0),
        &context.read(),
    );
    assert_eq!(refused, Err(RuleReject::Wire(RejectReason::DropCapacity)));
    assert_eq!(
        probe(
            &context,
            &player_cells,
            &[player],
            &[chest_reference()],
            &[drop_chunk],
        ),
        before
    );

    // Human leg, capacity available: the chest clears once, the batch is
    // staged and the tool wears by one.
    let mut state = authority();
    let session = state
        .admit(admitted(3, "Cara"), TransportKind::Memory)
        .expect("session");
    let player = ActorKey::Player(session);
    let mut context = harness_context(&mut state);
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
    context.preload_inventory(
        player,
        hotbar_inventory(0, ITEM_PICKAXE, 1, PICKAXE_DURABILITY),
    );
    for (pos, block) in [
        (BlockPos::new(0, 65, 0), AIR),
        (BlockPos::new(0, 65, 1), AIR),
        (chest_pos, CHEST),
    ] {
        context.preload_block(observation(pos, block));
    }
    context.preload_container(full_chest_record());
    let resolved = resolve_mine(
        player,
        &primary_control(std::f32::consts::PI, 0.0),
        &context.read(),
    )
    .expect("resolved chest mining")
    .expect("completed mining");
    let outcome = context
        .transaction()
        .try_mine(resolved)
        .expect("mined chest");

    assert_eq!(outcome.changed.len(), 1);
    assert_eq!(outcome.changed[0].pos, chest_pos);
    assert_eq!(outcome.changed[0].block, AIR);
    assert_eq!(outcome.changed[0].revision, 2);
    // Container slots plus the chest body: the staged multiset is conserved.
    assert_eq!(outcome.drops_created, 1 + CHEST_SLOTS);
    assert!(outcome.inventory_changed);
    let tool = context.read().inventory(player).expect("inventory").slots[0];
    assert_eq!(tool.item, ITEM_PICKAXE);
    assert_eq!(tool.durability, PICKAXE_DURABILITY - 1);
    assert!(context.read().container(chest_reference()).is_none());
    assert_eq!(
        context.read().drops(overworld_key(chest_pos)).len(),
        1 + CHEST_SLOTS
    );
    assert_eq!(context.events().len(), 0);

    // Companion leg, full inventory: no slot can take the batch.
    let mut state = authority();
    let companion_key = ActorKey::Companion(companion);
    let mut context = harness_context(&mut state);
    context
        .stage(RuleEffect::Environment(environment()))
        .expect("environment");
    context
        .stage(RuleEffect::Actor(companion_actor(
            companion,
            [0.5, 64.0, 0.5],
        )))
        .expect("actor");
    let mut full = InventoryRecord::empty();
    for slot in &mut full.slots {
        *slot = ItemStack {
            item: ITEM_DIRT,
            count: 64,
            durability: 0,
        };
    }
    context.preload_inventory(companion_key, full);
    for (pos, block) in [
        (BlockPos::new(0, 65, 0), AIR),
        (BlockPos::new(0, 65, 1), AIR),
        (chest_pos, CHEST),
    ] {
        context.preload_block(observation(pos, block));
    }
    context.preload_container(full_chest_record());
    let before = probe(
        &context,
        &player_cells,
        &[companion_key],
        &[chest_reference()],
        &[drop_chunk],
    );

    let refused = resolve_companion_mine(companion, chest_pos, &context.read());
    assert_eq!(refused, Err(RuleReject::Wire(RejectReason::HotbarFull)));
    assert_eq!(
        probe(
            &context,
            &player_cells,
            &[companion_key],
            &[chest_reference()],
            &[drop_chunk],
        ),
        before
    );

    // Companion leg, capacity available: the batch credits into the
    // inventory and nothing is staged as a world drop.
    let mut state = authority();
    let mut context = harness_context(&mut state);
    context
        .stage(RuleEffect::Environment(environment()))
        .expect("environment");
    context
        .stage(RuleEffect::Actor(companion_actor(
            companion,
            [0.5, 64.0, 0.5],
        )))
        .expect("actor");
    context.preload_inventory(companion_key, InventoryRecord::empty());
    for (pos, block) in [
        (BlockPos::new(0, 65, 0), AIR),
        (BlockPos::new(0, 65, 1), AIR),
        (chest_pos, CHEST),
    ] {
        context.preload_block(observation(pos, block));
    }
    context.preload_container(full_chest_record());
    let resolved = resolve_companion_mine(companion, chest_pos, &context.read())
        .expect("resolved companion mining");
    let outcome = context
        .transaction()
        .try_mine(resolved)
        .expect("companion mined chest");

    assert_eq!(outcome.changed.len(), 1);
    assert_eq!(outcome.changed[0].pos, chest_pos);
    assert_eq!(outcome.changed[0].block, AIR);
    assert_eq!(outcome.changed[0].revision, 2);
    assert!(outcome.inventory_changed);
    assert_eq!(outcome.drops_created, 0);
    let credited = *context
        .read()
        .inventory(companion_key)
        .expect("credited inventory");
    let mut expected = InventoryRecord::empty();
    // An empty selected hand cannot harvest the chest body; its contents
    // still transfer in fixed slot order.
    for slot in 0..CHEST_SLOTS {
        expected.slots[slot] = ItemStack {
            item: ITEM_STONE,
            count: 64,
            durability: 0,
        };
    }
    assert_eq!(credited, expected);
    assert_eq!(context.events().len(), 0);

    // Exempt-tool leg: an intact sword never wears on a completed mine
    // (`IsIntactSword` exemption in `consumeToolDurability`,
    // `packages/server/sim/entity/mining.go`).
    let mut state = authority();
    let session = state
        .admit(admitted(3, "Cara"), TransportKind::Memory)
        .expect("session");
    let player = ActorKey::Player(session);
    let mut context = harness_context(&mut state);
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
    context.preload_inventory(
        player,
        hotbar_inventory(0, ITEM_WOODEN_SWORD, 1, SWORD_DURABILITY),
    );
    context.preload_block(observation(BlockPos::new(0, 65, 0), AIR));
    context.preload_block(observation(BlockPos::new(0, 65, 1), AIR));
    context.preload_block(observation(BlockPos::new(0, 65, 2), DIRT));
    let resolved = resolve_mine(
        player,
        &primary_control(std::f32::consts::PI, 0.0),
        &context.read(),
    )
    .expect("resolved sword mining")
    .expect("completed mining");
    let outcome = context
        .transaction()
        .try_mine(resolved)
        .expect("sword mined dirt");
    assert_eq!(outcome.changed.len(), 1);
    assert_eq!(outcome.changed[0].block, AIR);
    assert_eq!(outcome.drops_created, 1);
    assert!(!outcome.inventory_changed);
    assert_eq!(
        context.read().inventory(player).expect("inventory").slots[0],
        ItemStack {
            item: ITEM_WOODEN_SWORD,
            count: 1,
            durability: SWORD_DURABILITY,
        }
    );
    assert_eq!(context.events().len(), 0);
}

#[test]
fn stale_generation_and_revision_refused() {
    let target = BlockPos::new(0, 65, 1);

    // Generation leg.
    let mut state = authority();
    let session = state
        .admit(admitted(4, "Dan"), TransportKind::Memory)
        .expect("session");
    let actor = ActorKey::Player(session);
    let mut context = harness_context(&mut state);
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
    context.preload_inventory(actor, hotbar_inventory(0, ITEM_DIRT, 2, 0));
    context.preload_block(observation(BlockPos::new(0, 65, 0), AIR));
    context.preload_block(observation(target, AIR));
    context.preload_block(observation(BlockPos::new(0, 65, 2), STONE));

    let resolved = resolve_place(actor, &south_intent(), &context.read()).expect("resolved place");
    context.preload_block(observation_at(target, AIR, 2, 1));
    let refused = context.transaction().try_place(resolved);
    assert_eq!(refused, Err(RuleReject::StaleObservation));
    let observed = context
        .read()
        .observation(Dimension::OVERWORLD, target)
        .expect("target cell");
    assert_eq!(observed.block, AIR);
    assert_eq!(observed.revision, 1);
    assert_eq!(
        context.read().inventory(actor).expect("inventory").slots[0].count,
        2
    );
    assert_eq!(context.events().len(), 0);

    // Revision leg against the refreshed generation.
    let resolved = resolve_place(actor, &south_intent(), &context.read()).expect("resolved place");
    for block in [STONE, AIR] {
        let current = context
            .read()
            .observation(Dimension::OVERWORLD, target)
            .expect("target");
        let advance = BlockWrite::try_new(current, block).expect("revision advance");
        context
            .transaction()
            .try_system(SystemRule::Support, vec![advance])
            .expect("unrelated write");
    }
    let refused = context.transaction().try_place(resolved);
    assert_eq!(refused, Err(RuleReject::StaleObservation));
    let observed = context
        .read()
        .observation(Dimension::OVERWORLD, target)
        .expect("target cell");
    assert_eq!(observed.block, AIR);
    assert_eq!(observed.revision, 3);
    assert_eq!(
        context.read().inventory(actor).expect("inventory").slots[0].count,
        2
    );
    assert_eq!(context.events().len(), 0);
}

#[test]
fn missing_item_occupied_protected_refused() {
    // Empty selected slot: the Go hotbar oracle pins the empty-slot placement
    // reject as `RejectInvalidBlock` (`packages/server/sim/runtime/hotbar_test.go`
    // `TestHotbarPlaceRejectsEmptyOrInvalidSlot`).
    fixture_case(AIR, AIR, STONE, ItemStack::default(), |context, session| {
        let actor = ActorKey::Player(session);
        let cells = [
            BlockPos::new(0, 65, 0),
            BlockPos::new(0, 65, 1),
            BlockPos::new(0, 65, 2),
        ];
        let before = probe(context, &cells, &[actor], &[], &[]);
        let refused = resolve_place(actor, &south_intent(), &context.read());
        assert_eq!(refused, Err(RuleReject::Wire(RejectReason::InvalidBlock)));
        assert_eq!(probe(context, &cells, &[actor], &[], &[]), before);
    });

    // Non-air aim cell: the eye sits inside a solid block, so the ray hit
    // carries no entry face and there is no air cell to place into — the
    // faceless-hit rule Go pins as `RejectOccupied`
    // (`packages/server/sim/entity/placement.go`, `BlockFaceNone` arm).
    fixture_case(
        STONE,
        AIR,
        STONE,
        ItemStack {
            item: ITEM_DIRT,
            count: 2,
            durability: 0,
        },
        |context, session| {
            let actor = ActorKey::Player(session);
            let cells = [
                BlockPos::new(0, 65, 0),
                BlockPos::new(0, 65, 1),
                BlockPos::new(0, 65, 2),
            ];
            let before = probe(context, &cells, &[actor], &[], &[]);
            let refused = resolve_place(actor, &south_intent(), &context.read());
            assert_eq!(refused, Err(RuleReject::Wire(RejectReason::Occupied)));
            assert_eq!(probe(context, &cells, &[actor], &[], &[]), before);
        },
    );

    // Protected target: bedrock has no `core.BlockDrop` entry
    // (`packages/shared/core/item.go`), so completed mining refuses
    // `RejectProtectedBlock` (`packages/server/sim/entity/mining.go`).
    fixture_case(
        AIR,
        AIR,
        BEDROCK,
        ItemStack::default(),
        |context, session| {
            let actor = ActorKey::Player(session);
            let cells = [
                BlockPos::new(0, 65, 0),
                BlockPos::new(0, 65, 1),
                BlockPos::new(0, 65, 2),
            ];
            let before = probe(context, &cells, &[actor], &[], &[]);
            let refused = resolve_mine(
                actor,
                &primary_control(std::f32::consts::PI, 0.0),
                &context.read(),
            );
            assert_eq!(refused, Err(RuleReject::Wire(RejectReason::ProtectedBlock)));
            assert_eq!(probe(context, &cells, &[actor], &[], &[]), before);
        },
    );
}

/// One aimed placement/mining fixture: an active player at `[0.5, 64.0, 0.5]`
/// looking south, `held` in hotbar slot zero, an origin cell (the eye cell)
/// carrying `origin_block`, a target cell carrying `target_block`, and a hit
/// cell carrying `hit_block`.
fn fixture_case(
    origin_block: u16,
    target_block: u16,
    hit_block: u16,
    held: ItemStack,
    case: impl FnOnce(&mut TickContext<'_>, SessionKey),
) {
    let mut state = authority();
    let session = state
        .admit(admitted(5, "Eve"), TransportKind::Memory)
        .expect("session");
    let mut context = harness_context(&mut state);
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
    let mut inventory = InventoryRecord::empty();
    inventory.slots[0] = held;
    context.preload_inventory(ActorKey::Player(session), inventory);
    context.preload_block(observation(BlockPos::new(0, 65, 0), origin_block));
    context.preload_block(observation(BlockPos::new(0, 65, 1), target_block));
    context.preload_block(observation(BlockPos::new(0, 65, 2), hit_block));
    case(&mut context, session);
}

#[test]
fn wrong_tool_clears_without_body_or_drop_capacity() {
    for block in [12, 10] {
        fixture_case(AIR, AIR, block, ItemStack::default(), |context, session| {
            let actor = ActorKey::Player(session);
            let target = BlockPos::new(0, 65, 2);
            for slot in 0..32 {
                context.preload_drop(occupied_drop(slot));
            }
            let resolved = resolve_mine(
                actor,
                &primary_control(std::f32::consts::PI, 0.0),
                &context.read(),
            )
            .expect("wrong tool still resolves")
            .expect("target exists");
            let outcome = context.transaction().try_mine(resolved).expect("clear");
            assert_eq!(outcome.changed.len(), 1);
            assert_eq!(outcome.drops_created, 0);
            assert_eq!(
                context
                    .read()
                    .observation(Dimension::OVERWORLD, target)
                    .unwrap()
                    .block,
                AIR
            );
            assert_eq!(context.read().drops(overworld_key(target)).len(), 32);
        });
    }
}

#[test]
fn upper_door_mining_clears_pair_and_anchors_lower_drop() {
    fixture_case(AIR, AIR, 70, ItemStack::default(), |context, session| {
        let actor = ActorKey::Player(session);
        let upper = BlockPos::new(0, 65, 2);
        let lower = BlockPos::new(0, 64, 2);
        context.preload_block(observation(lower, 62));
        let resolved = resolve_mine(
            actor,
            &primary_control(std::f32::consts::PI, 0.0),
            &context.read(),
        )
        .expect("door resolve")
        .expect("door target");
        let outcome = context
            .transaction()
            .try_mine(resolved)
            .expect("door clear");
        assert_eq!(outcome.changed.len(), 2);
        assert_eq!(outcome.drops_created, 1);
        assert_eq!(
            context
                .read()
                .observation(Dimension::OVERWORLD, upper)
                .unwrap()
                .block,
            AIR
        );
        assert_eq!(
            context
                .read()
                .observation(Dimension::OVERWORLD, lower)
                .unwrap()
                .block,
            AIR
        );
        let view = context.read();
        let drops = view.drops(overworld_key(lower));
        assert_eq!(drops[0].stack.item, 43);
        assert_eq!(drops[0].position.get(), [0.5, 64.5, 2.5]);
    });
}

#[test]
fn source_harvest_outputs_settle_in_fixed_order() {
    let target = BlockPos::new(0, 65, 2);
    // Expected stacks are literal outputs from the Go sampler at tick zero.
    for (block, seed, expected) in [
        (37, 7, vec![(34, 1)]),
        (43, 7, vec![(34, 1)]),
        (46, 7, vec![(40, 1)]),
        (52, 7, vec![(40, 1)]),
        (54, 7, vec![(41, 1)]),
        (60, 7, vec![(41, 1)]),
        (44, 7, vec![(35, 2), (34, 2)]),
        (53, 7, vec![(40, 1)]),
        (53, 14, vec![(40, 1), (42, 1)]),
        (61, 7, vec![(41, 1)]),
        (19, 7, vec![(22, 1)]),
        (19, 11, vec![(22, 1), (57, 1)]),
        (84, 4, vec![(34, 1)]),
        (84, 7, vec![]),
    ] {
        fixture_case(AIR, AIR, block, ItemStack::default(), |context, session| {
            let mut env = environment();
            env.seed = seed;
            context.stage(RuleEffect::Environment(env)).expect("seed");
            let actor = ActorKey::Player(session);
            let resolved = resolve_mine(
                actor,
                &primary_control(std::f32::consts::PI, 0.0),
                &context.read(),
            )
            .expect("resolve")
            .expect("target");
            let outcome = context.transaction().try_mine(resolved).expect("mine");
            assert_eq!(outcome.changed.len(), 1, "block {block}");
            let view = context.read();
            let actual: Vec<_> = view
                .drops(overworld_key(target))
                .iter()
                .map(|drop| (drop.stack.item, drop.stack.count))
                .collect();
            assert_eq!(actual, expected, "block {block}, seed {seed}");
        });
    }
}

#[test]
fn grass_miss_ignores_full_capacity_but_hit_retries_stably() {
    let target = BlockPos::new(0, 65, 2);
    for (seed, hit) in [(7, false), (4, true)] {
        fixture_case(AIR, AIR, 84, ItemStack::default(), |context, session| {
            let mut env = environment();
            env.seed = seed;
            context.stage(RuleEffect::Environment(env)).unwrap();
            for slot in 0..32 {
                context.preload_drop(occupied_drop(slot));
            }
            let actor = ActorKey::Player(session);
            let before = probe(context, &[target], &[actor], &[], &[overworld_key(target)]);
            let resolve = || {
                resolve_mine(
                    actor,
                    &primary_control(std::f32::consts::PI, 0.0),
                    &context.read(),
                )
            };
            if hit {
                assert_eq!(resolve(), Err(RuleReject::Wire(RejectReason::DropCapacity)));
                assert_eq!(resolve(), Err(RuleReject::Wire(RejectReason::DropCapacity)));
                assert_eq!(
                    probe(context, &[target], &[actor], &[], &[overworld_key(target)]),
                    before
                );
            } else {
                let resolved = resolve().unwrap().unwrap();
                let outcome = context.transaction().try_mine(resolved).unwrap();
                assert_eq!(outcome.drops_created, 0);
                assert_eq!(
                    context
                        .read()
                        .observation(Dimension::OVERWORLD, target)
                        .unwrap()
                        .block,
                    AIR
                );
            }
        });
    }
}

#[test]
fn leaf_second_output_capacity_refuses_whole_mine() {
    let target = BlockPos::new(0, 65, 2);
    fixture_case(AIR, AIR, 19, ItemStack::default(), |context, session| {
        let mut env = environment();
        env.seed = 11;
        context.stage(RuleEffect::Environment(env)).unwrap();
        for slot in 0..31 {
            context.preload_drop(occupied_drop(slot));
        }
        let actor = ActorKey::Player(session);
        let before = probe(context, &[target], &[actor], &[], &[overworld_key(target)]);
        assert_eq!(
            resolve_mine(
                actor,
                &primary_control(std::f32::consts::PI, 0.0),
                &context.read()
            ),
            Err(RuleReject::Wire(RejectReason::DropCapacity))
        );
        assert_eq!(
            probe(context, &[target], &[actor], &[], &[overworld_key(target)]),
            before
        );
    });
}

#[test]
fn companion_credits_container_then_wears_newly_selected_tool() {
    let mut state = authority();
    let mut context = harness_context(&mut state);
    let id = companion_id();
    let actor = ActorKey::Companion(id);
    let target = BlockPos::new(0, 65, 2);
    context
        .stage(RuleEffect::Environment(environment()))
        .unwrap();
    context
        .stage(RuleEffect::Actor(companion_actor(id, [0.5, 64.0, 0.5])))
        .unwrap();
    context.preload_inventory(actor, InventoryRecord::empty());
    context.preload_block(observation(BlockPos::new(0, 65, 0), AIR));
    context.preload_block(observation(BlockPos::new(0, 65, 1), AIR));
    context.preload_block(observation(target, CHEST));
    let mut contents = [ItemStack::default(); CHEST_SLOTS];
    contents[0] = ItemStack {
        item: ITEM_PICKAXE,
        count: 1,
        durability: 10,
    };
    context.preload_container(ContainerRecord {
        reference: chest_reference(),
        revision: 1,
        slots: ContainerSlots::Chest(contents),
    });
    let resolved = resolve_companion_mine(id, target, &context.read()).unwrap();
    let outcome = context.transaction().try_mine(resolved).unwrap();
    assert_eq!(outcome.changed.len(), 1);
    assert_eq!(outcome.drops_created, 0);
    let after = context.read().inventory(actor).unwrap().slots;
    assert_eq!(
        after[0],
        ItemStack {
            item: ITEM_PICKAXE,
            count: 1,
            durability: 9
        }
    );
    assert!(after.iter().all(|stack| stack.item != ITEM_CHEST));
}

#[test]
fn cross_chunk_bed_head_requires_current_foot_and_anchors_hit_drop() {
    let foot = BlockPos::new(15, 65, 2);
    let head = BlockPos::new(16, 65, 2);
    let mut state = authority();
    let session = state
        .admit(admitted(72, "bed-mine"), TransportKind::Memory)
        .unwrap();
    let actor = ActorKey::Player(session);
    let mut context = harness_context(&mut state);
    context
        .stage(RuleEffect::Environment(environment()))
        .unwrap();
    context
        .stage(RuleEffect::Actor(player_actor(
            session,
            [16.5, 64.0, 0.5],
            std::f32::consts::PI,
            0.0,
        )))
        .unwrap();
    context.preload_inventory(actor, hotbar_inventory(0, ITEM_PICKAXE, 1, 50));
    context.preload_block(observation(BlockPos::new(16, 65, 0), AIR));
    context.preload_block(observation(BlockPos::new(16, 65, 1), AIR));
    context.preload_block(observation(head, BED_HEAD_EAST));
    let control = primary_control(std::f32::consts::PI, 0.0);
    let missing = probe(&context, &[head], &[actor], &[], &[overworld_key(head)]);
    assert_eq!(
        resolve_mine(actor, &control, &context.read()),
        Err(RuleReject::Wire(RejectReason::ChunkNotReady))
    );
    assert_eq!(
        probe(&context, &[head], &[actor], &[], &[overworld_key(head)]),
        missing
    );

    context.preload_block(observation(foot, BED_FOOT_EAST));
    let stale = resolve_mine(actor, &control, &context.read())
        .unwrap()
        .unwrap();
    for block in [AIR, BED_FOOT_EAST] {
        let current = context
            .read()
            .observation(Dimension::OVERWORLD, foot)
            .unwrap();
        context
            .transaction()
            .try_system(
                SystemRule::Support,
                vec![BlockWrite::try_new(current, block).unwrap()],
            )
            .unwrap();
    }
    let before = probe(
        &context,
        &[foot, head],
        &[actor],
        &[],
        &[overworld_key(foot), overworld_key(head)],
    );
    assert_eq!(
        context.transaction().try_mine(stale),
        Err(RuleReject::StaleObservation)
    );
    assert_eq!(
        probe(
            &context,
            &[foot, head],
            &[actor],
            &[],
            &[overworld_key(foot), overworld_key(head)]
        ),
        before
    );

    let resolved = resolve_mine(actor, &control, &context.read())
        .unwrap()
        .unwrap();
    let outcome = context.transaction().try_mine(resolved).unwrap();
    assert_eq!(outcome.changed.len(), 2);
    assert_eq!(outcome.drops_created, 1);
    assert_eq!(
        context
            .read()
            .observation(Dimension::OVERWORLD, foot)
            .unwrap()
            .block,
        AIR
    );
    assert_eq!(
        context
            .read()
            .observation(Dimension::OVERWORLD, head)
            .unwrap()
            .block,
        AIR
    );
    let view = context.read();
    let drops = view.drops(overworld_key(head));
    assert_eq!(drops[0].stack.item, ITEM_BED);
    assert_eq!(drops[0].position.get(), [16.5, 65.5, 2.5]);
    assert_eq!(view.inventory(actor).unwrap().slots[0].durability, 49);
}

#[test]
fn companion_bed_clears_pair_with_one_credit_and_one_wear() {
    let mut state = authority();
    let mut context = harness_context(&mut state);
    let id = companion_id();
    let actor = ActorKey::Companion(id);
    let foot = BlockPos::new(0, 65, 2);
    let head = BlockPos::new(1, 65, 2);
    context
        .stage(RuleEffect::Environment(environment()))
        .unwrap();
    context
        .stage(RuleEffect::Actor(companion_actor(id, [0.5, 64.0, 0.5])))
        .unwrap();
    context.preload_inventory(actor, hotbar_inventory(0, ITEM_PICKAXE, 1, 12));
    context.preload_block(observation(BlockPos::new(0, 65, 0), AIR));
    context.preload_block(observation(BlockPos::new(0, 65, 1), AIR));
    context.preload_block(observation(foot, BED_FOOT_EAST));
    context.preload_block(observation(head, BED_HEAD_EAST));
    let resolved = resolve_companion_mine(id, foot, &context.read()).unwrap();
    let outcome = context.transaction().try_mine(resolved).unwrap();
    assert_eq!(outcome.changed.len(), 2);
    assert_eq!(outcome.drops_created, 0);
    let view = context.read();
    assert_eq!(
        view.observation(Dimension::OVERWORLD, foot).unwrap().block,
        AIR
    );
    assert_eq!(
        view.observation(Dimension::OVERWORLD, head).unwrap().block,
        AIR
    );
    let slots = view.inventory(actor).unwrap().slots;
    assert_eq!(slots[0].durability, 11);
    assert_eq!(
        slots
            .iter()
            .filter(|stack| stack.item == ITEM_BED)
            .map(|stack| stack.count)
            .sum::<u8>(),
        1
    );
    assert!(view.drops(overworld_key(foot)).is_empty());
}

#[test]
fn companion_door_keeps_source_single_cell_branch() {
    let mut state = authority();
    let mut context = harness_context(&mut state);
    let id = companion_id();
    let actor = ActorKey::Companion(id);
    let lower = BlockPos::new(0, 65, 2);
    let upper = BlockPos::new(0, 66, 2);
    context
        .stage(RuleEffect::Environment(environment()))
        .unwrap();
    context
        .stage(RuleEffect::Actor(companion_actor(id, [0.5, 64.0, 0.5])))
        .unwrap();
    context.preload_inventory(actor, InventoryRecord::empty());
    context.preload_block(observation(BlockPos::new(0, 65, 0), AIR));
    context.preload_block(observation(BlockPos::new(0, 65, 1), AIR));
    context.preload_block(observation(lower, 62));
    context.preload_block(observation(upper, 70));
    let resolved = resolve_companion_mine(id, lower, &context.read()).unwrap();
    let outcome = context.transaction().try_mine(resolved).unwrap();
    assert_eq!(outcome.changed.len(), 1);
    let view = context.read();
    assert_eq!(
        view.observation(Dimension::OVERWORLD, lower).unwrap().block,
        AIR
    );
    assert_eq!(
        view.observation(Dimension::OVERWORLD, upper).unwrap().block,
        70
    );
    assert_eq!(view.inventory(actor).unwrap().slots[0].item, 43);
}

#[test]
fn door_at_world_ceiling_refuses_unavailable_upper_without_wear() {
    let mut state = authority();
    let session = state
        .admit(admitted(73, "ceiling"), TransportKind::Memory)
        .unwrap();
    let actor = ActorKey::Player(session);
    let mut context = harness_context(&mut state);
    context
        .stage(RuleEffect::Environment(environment()))
        .unwrap();
    context
        .stage(RuleEffect::Actor(player_actor(
            session,
            [0.5, 317.0, 0.5],
            std::f32::consts::PI,
            0.2,
        )))
        .unwrap();
    context.preload_inventory(actor, hotbar_inventory(0, ITEM_PICKAXE, 1, 20));
    for pos in [
        BlockPos::new(0, 318, 0),
        BlockPos::new(0, 318, 1),
        BlockPos::new(0, 318, 2),
    ] {
        context.preload_block(observation(pos, AIR));
    }
    let target = BlockPos::new(0, 319, 2);
    context.preload_block(observation(target, 62));
    let before = probe(&context, &[target], &[actor], &[], &[overworld_key(target)]);
    assert_eq!(
        resolve_mine(
            actor,
            &primary_control(std::f32::consts::PI, 0.2),
            &context.read()
        ),
        Err(RuleReject::Wire(RejectReason::ChunkNotReady))
    );
    assert_eq!(
        probe(&context, &[target], &[actor], &[], &[overworld_key(target)]),
        before
    );
}

#[test]
fn wrong_tool_chest_contents_are_atomic_without_body() {
    for full in [false, true] {
        fixture_case(AIR, AIR, CHEST, ItemStack::default(), |context, session| {
            let actor = ActorKey::Player(session);
            let target = BlockPos::new(0, 65, 2);
            let mut contents = [ItemStack::default(); CHEST_SLOTS];
            contents[0] = ItemStack {
                item: ITEM_DIRT,
                count: 1,
                durability: 0,
            };
            contents[1] = ItemStack {
                item: ITEM_STONE,
                count: 1,
                durability: 0,
            };
            context.preload_container(ContainerRecord {
                reference: chest_reference(),
                revision: 1,
                slots: ContainerSlots::Chest(contents),
            });
            if full {
                for slot in 0..31 {
                    context.preload_drop(occupied_drop(slot));
                }
            }
            let before = probe(
                context,
                &[target],
                &[actor],
                &[chest_reference()],
                &[overworld_key(target)],
            );
            let resolved = resolve_mine(
                actor,
                &primary_control(std::f32::consts::PI, 0.0),
                &context.read(),
            );
            if full {
                assert_eq!(resolved, Err(RuleReject::Wire(RejectReason::DropCapacity)));
                assert_eq!(
                    probe(
                        context,
                        &[target],
                        &[actor],
                        &[chest_reference()],
                        &[overworld_key(target)]
                    ),
                    before
                );
            } else {
                let outcome = context
                    .transaction()
                    .try_mine(resolved.unwrap().unwrap())
                    .unwrap();
                assert_eq!(outcome.drops_created, 2);
                let view = context.read();
                let actual: Vec<_> = view
                    .drops(overworld_key(target))
                    .iter()
                    .map(|drop| drop.stack.item)
                    .collect();
                assert_eq!(actual, vec![ITEM_DIRT, ITEM_STONE]);
                assert!(view.container(chest_reference()).is_none());
            }
        });
    }
}

#[test]
fn effect_budget_refused() {
    let mut state = authority();
    let mut context = harness_context(&mut state);

    // A valid system batch commits atomically.
    let single = BlockPos::new(0, 64, 0);
    context.preload_block(observation(single, AIR));
    let write = BlockWrite::try_new(observation(single, AIR), STONE).expect("write");
    let outcome = context
        .transaction()
        .try_system(SystemRule::Support, vec![write])
        .expect("valid system batch");
    assert_eq!(outcome.changed.len(), 1);
    assert_eq!(outcome.changed[0].block, STONE);
    assert_eq!(outcome.changed[0].revision, 2);

    // An unobserved cell inside a batch refuses before any write applies.
    let first = BlockPos::new(1, 64, 0);
    let second = BlockPos::new(2, 64, 0);
    context.preload_block(observation(first, AIR));
    let writes = vec![
        BlockWrite::try_new(observation(first, AIR), STONE).expect("first write"),
        BlockWrite::try_new(observation(second, STONE), STONE).expect("second write"),
    ];
    let refused = context
        .transaction()
        .try_system(SystemRule::Support, writes);
    assert_eq!(refused, Err(RuleReject::StaleObservation));
    assert_eq!(
        context
            .read()
            .observation(Dimension::OVERWORLD, first)
            .expect("first cell")
            .block,
        AIR
    );

    // A batch over the effect budget refuses with no partial application.
    let mut over: Vec<BlockWrite> = Vec::with_capacity(EFFECT_BUDGET + 1);
    for index in 0..=(EFFECT_BUDGET as i32) {
        let pos = BlockPos::new(100 + index, 64, 0);
        context.preload_block(observation(pos, AIR));
        over.push(BlockWrite::try_new(observation(pos, AIR), STONE).expect("write"));
    }
    let refused = context.transaction().try_system(SystemRule::Support, over);
    assert_eq!(
        refused,
        Err(RuleReject::ResourceFull(Resource::RuleEffects))
    );
    for index in 0..=(EFFECT_BUDGET as i32) {
        let observed = context
            .read()
            .observation(Dimension::OVERWORLD, BlockPos::new(100 + index, 64, 0))
            .expect("batch cell");
        assert_eq!(observed.block, AIR, "cell {index} was partially written");
        assert_eq!(observed.revision, 1, "cell {index} advanced a revision");
    }
    assert_eq!(context.events().len(), 0);
}

#[test]
fn primary_release_and_no_target_none() {
    let mut state = authority();
    let session = state
        .admit(admitted(6, "Fay"), TransportKind::Memory)
        .expect("session");
    let actor = ActorKey::Player(session);
    let mut context = harness_context(&mut state);
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
    context.preload_inventory(actor, hotbar_inventory(0, ITEM_DIRT, 2, 0));
    let cells: Vec<BlockPos> = (0..=6).map(|z| BlockPos::new(0, 65, z)).collect();
    for pos in &cells {
        context.preload_block(observation(*pos, AIR));
    }
    let before = probe(&context, &cells, &[actor], &[], &[]);

    let released = resolve_mine(
        actor,
        &idle_control(std::f32::consts::PI, 0.0),
        &context.read(),
    );
    assert_eq!(released, Ok(None));
    assert_eq!(probe(&context, &cells, &[actor], &[], &[]), before);

    let no_target = resolve_mine(
        actor,
        &primary_control(std::f32::consts::PI, 0.0),
        &context.read(),
    );
    assert_eq!(no_target, Ok(None));
    assert_eq!(probe(&context, &cells, &[actor], &[], &[]), before);
}

#[test]
fn competing_target_first_wins() {
    let companion = companion_id();
    let contested = BlockPos::new(0, 65, 1);
    let free = BlockPos::new(0, 65, 3);
    let mut state = authority();
    let session = state
        .admit(admitted(7, "Gia"), TransportKind::Memory)
        .expect("session");
    let player = ActorKey::Player(session);
    let companion_key = ActorKey::Companion(companion);
    let mut context = harness_context(&mut state);
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
        .stage(RuleEffect::Actor(companion_actor(
            companion,
            [4.5, 64.0, 0.5],
        )))
        .expect("actor");
    context.preload_inventory(player, hotbar_inventory(0, ITEM_DIRT, 2, 0));
    context.preload_inventory(companion_key, hotbar_inventory(3, ITEM_DIRT, 1, 0));
    for (pos, block) in [
        (BlockPos::new(0, 65, 0), AIR),
        (contested, AIR),
        (BlockPos::new(0, 65, 2), STONE),
        (free, AIR),
    ] {
        context.preload_block(observation(pos, block));
    }

    // The human placement commits first.
    let human = resolve_place(player, &south_intent(), &context.read()).expect("human place");
    context
        .transaction()
        .try_place(human)
        .expect("human committed");
    let human_cell = context
        .read()
        .observation(Dimension::OVERWORLD, contested)
        .expect("contested cell");
    assert_eq!(human_cell.block, DIRT);
    assert_eq!(human_cell.revision, 2);

    // The companion proposal against the refreshed view refuses.
    let refused = resolve_companion_place(companion, contested, DIRT, &context.read());
    assert_eq!(refused, Err(RuleReject::Wire(RejectReason::Occupied)));

    // The companion commits its own placement at a free cell.
    let proposal =
        resolve_companion_place(companion, free, DIRT, &context.read()).expect("companion place");
    context
        .transaction()
        .try_place(proposal)
        .expect("companion committed");
    let companion_cell = context
        .read()
        .observation(Dimension::OVERWORLD, free)
        .expect("companion cell");
    assert_eq!(companion_cell.block, DIRT);
    assert_eq!(companion_cell.revision, 2);
    assert_eq!(
        context
            .read()
            .inventory(companion_key)
            .expect("companion inventory")
            .slots[3],
        ItemStack::default()
    );

    // A repeated proposal after its own committed write refuses identically.
    let repeat = resolve_companion_place(companion, free, DIRT, &context.read());
    assert_eq!(repeat, Err(RuleReject::Wire(RejectReason::Occupied)));

    // The human's committed state is untouched.
    let human_cell = context
        .read()
        .observation(Dimension::OVERWORLD, contested)
        .expect("contested cell");
    assert_eq!(human_cell.block, DIRT);
    assert_eq!(human_cell.revision, 2);
    assert_eq!(
        context.read().inventory(player).expect("inventory").slots[0].count,
        1
    );
    assert_eq!(context.events().len(), 0);
}

fn loaded_context(
    state: &mut AuthorityState,
    session: SessionKey,
    data: mornlea_storage::Chunk,
    item: u16,
) -> TickContext<'_> {
    let mut ctx = harness_context(state);
    ctx.stage(RuleEffect::Environment(environment())).unwrap();
    ctx.stage(RuleEffect::Actor(player_actor(
        session,
        [0.5, 64.0, 0.5],
        std::f32::consts::PI,
        0.0,
    )))
    .unwrap();
    ctx.preload_inventory(
        ActorKey::Player(session),
        hotbar_inventory(
            0,
            item,
            if item == ITEM_PICKAXE { 1 } else { 3 },
            if item == ITEM_PICKAXE {
                PICKAXE_DURABILITY
            } else {
                0
            },
        ),
    );
    ctx.preload_ready_chunk(
        mornlea_server::core::world::ReadyChunk::try_new(
            overworld_key(BlockPos::new(0, 65, 2)),
            1,
            8,
            data,
        )
        .unwrap(),
    );
    ctx
}

#[test]
fn loaded_placement_allocates_lowest_reusable_slot_and_refuses_capacity_atomically() {
    use super::world_outputs::{empty, put, world};
    for (block, item, kind, capacity) in [
        (CHEST, ITEM_CHEST, ContainerKind::Chest, 16),
        (9, 8, ContainerKind::Furnace, 32),
    ] {
        for full in [false, true] {
            let mut state = authority();
            let session = state
                .admit(admitted(71, "slot-place"), TransportKind::Memory)
                .unwrap();
            let actor = ActorKey::Player(session);
            let mut data = empty();
            put(&mut data, BlockPos::new(0, 65, 2), STONE);
            match kind {
                ContainerKind::Chest => {
                    for (i, s) in data.chests.iter_mut().enumerate() {
                        s.generation = if full || i == 0 { u32::MAX } else { 11 };
                    }
                }
                ContainerKind::Furnace => {
                    for (i, s) in data.furnaces.iter_mut().enumerate() {
                        s.generation = if full || i == 0 { u32::MAX } else { 11 };
                    }
                }
            }
            let mut ctx = loaded_context(&mut state, session, data, item);
            let before = ctx.snapshot_state(world());
            let resolved = resolve_place(actor, &south_intent(), &ctx.read()).unwrap();
            let outcome = ctx.transaction().try_place(resolved);
            if full {
                assert_eq!(
                    outcome,
                    Err(RuleReject::Wire(RejectReason::ContainerCapacity))
                );
                assert_eq!(ctx.snapshot_state(world()), before);
            } else {
                outcome.unwrap();
                let record = ctx
                    .read()
                    .container_at(Dimension::OVERWORLD, BlockPos::new(0, 65, 1), kind)
                    .unwrap();
                assert_eq!(record.reference.slot(), 1);
                assert_eq!(record.reference.generation(), 12);
                assert_eq!(
                    ctx.read()
                        .observation(Dimension::OVERWORLD, BlockPos::new(0, 65, 1))
                        .unwrap()
                        .block,
                    block
                );
                assert_eq!(ctx.read().inventory(actor).unwrap().slots[0].count, 2);
                assert_eq!(
                    ctx.read()
                        .container_refs(overworld_key(BlockPos::new(0, 65, 1)))
                        .len(),
                    1
                );
                assert!(usize::from(record.reference.slot()) < capacity);
            }
        }
    }
}

#[test]
fn loaded_container_mining_removes_nonzero_slot_and_preserves_generation() {
    use super::world_outputs::{empty, put, with_chest, world};
    for furnace in [false, true] {
        let mut state = authority();
        let session = state
            .admit(admitted(72, "slot-mine"), TransportKind::Memory)
            .unwrap();
        let actor = ActorKey::Player(session);
        let target = BlockPos::new(0, 65, 2);
        let data = if furnace {
            let mut data = empty();
            put(&mut data, target, 9);
            data.furnaces[3] = mornlea_storage::FurnaceSlot {
                generation: 9,
                active: true,
                block_index: mornlea_domain::chunk_block_index(target),
                input: ItemStack {
                    item: 6,
                    count: 2,
                    durability: 0,
                },
                fuel: ItemStack {
                    item: 5,
                    count: 1,
                    durability: 0,
                },
                ..Default::default()
            };
            data
        } else {
            with_chest()
        };
        let mut ctx = loaded_context(&mut state, session, data, ITEM_PICKAXE);
        let resolved = resolve_mine(
            actor,
            &primary_control(std::f32::consts::PI, 0.0),
            &ctx.read(),
        )
        .unwrap()
        .unwrap();
        ctx.transaction().try_mine(resolved).unwrap();
        let kind = if furnace {
            ContainerKind::Furnace
        } else {
            ContainerKind::Chest
        };
        assert!(
            ctx.read()
                .container_at(Dimension::OVERWORLD, target, kind)
                .is_none()
        );
        let snapshot = ctx.snapshot_state(world());
        let data = &snapshot.chunks[0].3;
        assert_eq!(snapshot.chunks[0].2, 9);
        if furnace {
            assert!(!data.furnaces[3].active);
            assert_eq!(data.furnaces[3].generation, 9);
        } else {
            assert!(!data.chests[5].active);
            assert_eq!(data.chests[5].generation, 7);
        }
        assert_eq!(
            ctx.read().drops(overworld_key(target)).len(),
            if furnace { 3 } else { 2 }
        );
        mornlea_storage::encode_chunk(&mornlea_storage::ChunkSave {
            key: mornlea_storage::ChunkKey {
                dimension: 0,
                x: 0,
                z: 0,
            },
            revision: 9,
            chunk: data.clone(),
        })
        .unwrap();
        assert_eq!(
            ctx.read().inventory(actor).unwrap().slots[0].durability,
            PICKAXE_DURABILITY - 1
        );
    }
}

#[test]
fn mining_rechecks_loaded_container_capture_and_drop_capacity_without_spending_tool() {
    use super::world_outputs::{with_chest, world};
    for full in [false, true] {
        let mut state = authority();
        let session = state
            .admit(admitted(73, "stale-mine"), TransportKind::Memory)
            .unwrap();
        let actor = ActorKey::Player(session);
        let mut ctx = loaded_context(&mut state, session, with_chest(), ITEM_PICKAXE);
        let target = BlockPos::new(0, 65, 2);
        let resolved = resolve_mine(
            actor,
            &primary_control(std::f32::consts::PI, 0.0),
            &ctx.read(),
        )
        .unwrap()
        .unwrap();
        if full {
            for slot in 0..32 {
                ctx.preload_drop(occupied_drop(slot));
            }
        } else {
            let before = ctx
                .read()
                .container_at(Dimension::OVERWORLD, target, ContainerKind::Chest)
                .unwrap();
            let mut after = before.clone();
            if let ContainerSlots::Chest(items) = &mut after.slots {
                items[0].count = 3;
            }
            ctx.stage(RuleEffect::Container { before, after }).unwrap();
        }
        let before = ctx.snapshot_state(world());
        assert!(ctx.transaction().try_mine(resolved).is_err());
        assert_eq!(ctx.snapshot_state(world()), before);
    }
}

#[test]
fn full_drop_slots_allow_exact_same_cell_mining_merge() {
    use super::world_outputs::{empty, put};
    let mut state = authority();
    let session = state
        .admit(admitted(74, "merge-mine"), TransportKind::Memory)
        .unwrap();
    let actor = ActorKey::Player(session);
    let target = BlockPos::new(0, 65, 2);
    let mut data = empty();
    put(&mut data, target, STONE);
    for (index, drop) in data.drops.iter_mut().enumerate() {
        *drop = mornlea_storage::DropSlot {
            generation: 1,
            active: true,
            block_index: mornlea_domain::chunk_block_index(target),
            stack: ItemStack {
                item: if index == 0 { ITEM_STONE } else { ITEM_COAL },
                count: if index == 0 { 63 } else { 64 },
                durability: 0,
            },
            ..Default::default()
        };
    }
    let mut ctx = loaded_context(&mut state, session, data, ITEM_PICKAXE);
    let resolved = resolve_mine(
        actor,
        &primary_control(std::f32::consts::PI, 0.0),
        &ctx.read(),
    )
    .unwrap()
    .unwrap();
    ctx.transaction().try_mine(resolved).unwrap();
    assert_eq!(ctx.read().drops(overworld_key(target)).len(), 32);
    assert_eq!(ctx.read().drops(overworld_key(target))[0].stack.count, 64);
}

#[test]
fn companion_mines_loaded_container_once_and_rebirth_advances_retained_generation() {
    use super::world_outputs::{with_chest, world};
    let mut state = authority();
    let mut ctx = harness_context(&mut state);
    let id = companion_id();
    let actor = ActorKey::Companion(id);
    let target = BlockPos::new(0, 65, 2);
    let mut data = with_chest();
    for slot in &mut data.chests[..5] {
        slot.generation = u32::MAX;
    }
    ctx.stage(RuleEffect::Environment(environment())).unwrap();
    ctx.stage(RuleEffect::Actor(companion_actor(id, [0.5, 64.0, 0.5])))
        .unwrap();
    ctx.preload_inventory(actor, InventoryRecord::empty());
    ctx.preload_ready_chunk(
        mornlea_server::core::world::ReadyChunk::try_new(overworld_key(target), 1, 8, data)
            .unwrap(),
    );
    let resolved = resolve_companion_mine(id, target, &ctx.read()).unwrap();
    ctx.transaction().try_mine(resolved).unwrap();
    assert!(
        ctx.read()
            .container_at(Dimension::OVERWORLD, target, ContainerKind::Chest)
            .is_none()
    );
    assert!(ctx.read().drops(overworld_key(target)).is_empty());
    let inventory = ctx.read().inventory(actor).unwrap();
    assert_eq!(
        inventory
            .slots
            .iter()
            .filter(|s| s.item == ITEM_CHEST)
            .map(|s| u32::from(s.count))
            .sum::<u32>(),
        0
    );
    assert_eq!(
        inventory
            .slots
            .iter()
            .filter(|s| s.item == ITEM_STONE)
            .map(|s| u32::from(s.count))
            .sum::<u32>(),
        4
    );
    assert!(resolve_companion_mine(id, target, &ctx.read()).is_err());
    let observed = ctx
        .read()
        .observation(Dimension::OVERWORLD, target)
        .unwrap();
    ctx.transaction()
        .try_system(
            SystemRule::Support,
            vec![BlockWrite {
                observed,
                replacement: CHEST,
            }],
        )
        .unwrap();
    let reborn = ctx
        .read()
        .container_at(Dimension::OVERWORLD, target, ContainerKind::Chest)
        .unwrap();
    assert_eq!(reborn.reference.slot(), 5);
    assert_eq!(reborn.reference.generation(), 8);
    assert_eq!(ctx.snapshot_state(world()).chunks[0].2, 9);
}

/// The shared interaction classifier passes fluids and open doors: a mine ray
/// through source water, flowing water or an open lower door selects the solid
/// cell behind, and the front cell keeps its form and revision.
#[test]
fn mine_ray_passes_fluids_and_open_doors_to_solid_behind() {
    const WATER_SOURCE: u16 = 27; // `core.WaterSourceID`
    const WATER_FLOWING: u16 = 34; // `core.WaterLevel7ID`
    const DOOR_LOWER_SOUTH_OPEN: u16 = 63; // `core.DoorLowerSouthOpen`
    for corridor in [WATER_SOURCE, WATER_FLOWING, DOOR_LOWER_SOUTH_OPEN] {
        fixture_case(
            AIR,
            corridor,
            STONE,
            ItemStack::default(),
            |context, session| {
                let actor = ActorKey::Player(session);
                let target = BlockPos::new(0, 65, 2);
                let front = BlockPos::new(0, 65, 1);
                let resolved = resolve_mine(
                    actor,
                    &primary_control(std::f32::consts::PI, 0.0),
                    &context.read(),
                )
                .expect("transparent corridor resolves")
                .expect("the solid behind is the target");
                let outcome = context.transaction().try_mine(resolved).expect("clear");
                assert_eq!(outcome.changed.len(), 1);
                assert_eq!(outcome.changed[0].pos, target);
                let view = context.read();
                assert_eq!(
                    view.observation(Dimension::OVERWORLD, target)
                        .unwrap()
                        .block,
                    AIR
                );
                let front_after = view.observation(Dimension::OVERWORLD, front).unwrap();
                assert_eq!(front_after.block, corridor);
                assert_eq!(front_after.revision, 1);
            },
        );
    }
}

/// A closed lower door stays solid to the mine ray: the resolver clears the
/// door pair and never touches the stone behind it.
#[test]
fn mine_ray_stops_at_closed_door() {
    const DOOR_LOWER_SOUTH_CLOSED: u16 = 62; // `core.DoorLowerSouthClosed`
    const DOOR_UPPER: u16 = 70; // `core.DoorUpper`
    const ITEM_DOOR: u16 = 43; // `core.ItemDoor`
    fixture_case(
        AIR,
        DOOR_LOWER_SOUTH_CLOSED,
        STONE,
        ItemStack::default(),
        |context, session| {
            let actor = ActorKey::Player(session);
            context.preload_block(observation(BlockPos::new(0, 66, 1), DOOR_UPPER));
            let resolved = resolve_mine(
                actor,
                &primary_control(std::f32::consts::PI, 0.0),
                &context.read(),
            )
            .expect("closed door resolves")
            .expect("the door is the target");
            let outcome = context.transaction().try_mine(resolved).expect("clear");
            assert_eq!(outcome.changed.len(), 2);
            let view = context.read();
            assert_eq!(
                view.observation(Dimension::OVERWORLD, BlockPos::new(0, 65, 2))
                    .unwrap()
                    .block,
                STONE,
                "the solid behind a closed door is untouched"
            );
            let drops = view.drops(overworld_key(BlockPos::new(0, 65, 1)));
            assert_eq!(drops.len(), 1);
            assert_eq!(drops[0].stack.item, ITEM_DOOR);
        },
    );
}

/// Ordinary placement replaces water reached through a transparent ray while
/// preserving the solid hit and publishing exactly one debit and delta.
#[test]
fn place_ray_passes_water_and_ordinary_block_replaces_it() {
    for water in [27, 34] {
        fixture_case(
            AIR,
            water,
            STONE,
            ItemStack {
                item: ITEM_DIRT,
                count: 3,
                durability: 0,
            },
            |context, session| {
                let actor = ActorKey::Player(session);
                let target = BlockPos::new(0, 65, 1);
                let hit = BlockPos::new(0, 65, 2);
                let before_inventory = *context.read().inventory(actor).unwrap();
                let before_hit = context.read().observation(Dimension::OVERWORLD, hit);
                let resolved = resolve_place(actor, &south_intent(), &context.read())
                    .expect("water replacement resolves");
                assert_eq!(*context.read().inventory(actor).unwrap(), before_inventory);
                assert_eq!(
                    context
                        .read()
                        .observation(Dimension::OVERWORLD, target)
                        .unwrap()
                        .block,
                    water
                );
                let outcome = context
                    .transaction()
                    .try_place(resolved)
                    .expect("replacement commits");
                assert_eq!(outcome.changed.len(), 1);
                assert_eq!(outcome.changed[0].pos, target);
                assert_eq!(outcome.changed[0].block, DIRT);
                assert_eq!(outcome.changed[0].generation, 1);
                assert_eq!(outcome.changed[0].revision, 2);
                assert!(outcome.inventory_changed);
                assert_eq!(outcome.drops_created, 0);
                let mut expected_inventory = before_inventory;
                expected_inventory.slots[0].count = 2;
                assert_eq!(
                    *context.read().inventory(actor).unwrap(),
                    expected_inventory
                );
                assert_eq!(
                    context.read().observation(Dimension::OVERWORLD, hit),
                    before_hit
                );
                assert_eq!(
                    context
                        .read()
                        .observation(Dimension::OVERWORLD, target)
                        .unwrap()
                        .block,
                    DIRT
                );
                assert!(context.events().is_empty());
            },
        );
    }
}

/// An open door is transparent to the ray but remains an occupied destination.
#[test]
fn place_ray_passes_open_door_but_destination_refuses() {
    fixture_case(
        AIR,
        63,
        STONE,
        ItemStack {
            item: ITEM_DIRT,
            count: 3,
            durability: 0,
        },
        |context, session| {
            let actor = ActorKey::Player(session);
            let cells = [
                BlockPos::new(0, 65, 0),
                BlockPos::new(0, 65, 1),
                BlockPos::new(0, 65, 2),
            ];
            let before = probe(
                context,
                &cells,
                &[actor],
                &[chest_reference()],
                &[overworld_key(cells[0])],
            );
            assert_eq!(
                resolve_place(actor, &south_intent(), &context.read()),
                Err(RuleReject::Wire(RejectReason::Occupied))
            );
            assert_eq!(
                probe(
                    context,
                    &cells,
                    &[actor],
                    &[chest_reference()],
                    &[overworld_key(cells[0])]
                ),
                before
            );
        },
    );
}

#[test]
fn placement_invalid_item_precedes_ray_and_destination_refusals() {
    for held in [
        ItemStack::default(),
        ItemStack {
            item: ITEM_COAL,
            count: 1,
            durability: 0,
        },
    ] {
        for scene in 0..5 {
            let mut state = authority();
            let session = state
                .admit(admitted(1, "Ada"), TransportKind::Memory)
                .unwrap();
            let actor = ActorKey::Player(session);
            let mut context = harness_context(&mut state);
            context
                .stage(RuleEffect::Environment(environment()))
                .unwrap();
            context
                .stage(RuleEffect::Actor(player_actor(
                    session,
                    [0.5, 64.0, 0.5],
                    std::f32::consts::PI,
                    0.0,
                )))
                .unwrap();
            context.preload_inventory(
                actor,
                hotbar_inventory(0, held.item, held.count, held.durability),
            );
            let cells: Vec<_> = (0..=6).map(|z| BlockPos::new(0, 65, z)).collect();
            // The unavailable destination is also a traversed ray cell. Other
            // scenes are observed air, an unavailable origin, a faceless hit,
            // and an occupied destination behind a transparent open door.
            for (z, pos) in cells.iter().enumerate() {
                if (scene == 1 && z == 1) || (scene == 2 && z == 0) {
                    continue;
                }
                let block = match (scene, z) {
                    (3, 0) | (4, 2) => STONE,
                    (4, 1) => 63,
                    _ => AIR,
                };
                context.preload_block(observation(*pos, block));
            }
            let before = probe(
                &context,
                &cells,
                &[actor],
                &[chest_reference()],
                &[overworld_key(cells[0])],
            );
            assert_eq!(
                resolve_place(actor, &south_intent(), &context.read()),
                Err(RuleReject::Wire(RejectReason::InvalidBlock)),
                "scene {scene}, held {held:?}"
            );
            assert_eq!(
                probe(
                    &context,
                    &cells,
                    &[actor],
                    &[chest_reference()],
                    &[overworld_key(cells[0])]
                ),
                before
            );
        }
    }
}

#[test]
fn placement_forms_refuse_water_with_source_reason_and_no_effects() {
    // Seeds, potatoes, carrots, saplings and torches require air. Doors and
    // beds retain their strict footprint occupancy refusal for water.
    for (item, reason) in [
        (34, RejectReason::InvalidBlock),
        (40, RejectReason::InvalidBlock),
        (41, RejectReason::InvalidBlock),
        (57, RejectReason::InvalidBlock),
        (44, RejectReason::InvalidBlock),
        (43, RejectReason::Occupied),
        (ITEM_BED, RejectReason::Occupied),
    ] {
        for water in [27, 34] {
            fixture_case(
                AIR,
                water,
                STONE,
                ItemStack {
                    item,
                    count: 3,
                    durability: 0,
                },
                |context, session| {
                    let actor = ActorKey::Player(session);
                    let cells = [
                        BlockPos::new(0, 65, 0),
                        BlockPos::new(0, 65, 1),
                        BlockPos::new(0, 65, 2),
                        BlockPos::new(0, 64, 1),
                        BlockPos::new(0, 66, 1),
                    ];
                    let before = probe(
                        context,
                        &cells,
                        &[actor],
                        &[chest_reference()],
                        &[overworld_key(cells[0])],
                    );
                    assert_eq!(
                        resolve_place(actor, &south_intent(), &context.read()),
                        Err(RuleReject::Wire(reason)),
                        "item {item}, water {water}"
                    );
                    assert_eq!(
                        probe(
                            context,
                            &cells,
                            &[actor],
                            &[chest_reference()],
                            &[overworld_key(cells[0])]
                        ),
                        before
                    );
                },
            );
        }
    }
}

/// A closed lower door is the place ray's hit: the face-adjacent eye-side cell
/// takes the new block while the door and the stone behind keep their state.
#[test]
fn place_ray_stops_at_closed_door() {
    const DOOR_LOWER_SOUTH_CLOSED: u16 = 62; // `core.DoorLowerSouthClosed`
    fixture_case(
        AIR,
        DOOR_LOWER_SOUTH_CLOSED,
        STONE,
        ItemStack {
            item: ITEM_DIRT,
            count: 3,
            durability: 0,
        },
        |context, session| {
            let actor = ActorKey::Player(session);
            let resolved =
                resolve_place(actor, &south_intent(), &context.read()).expect("door is the hit");
            let outcome = context
                .transaction()
                .try_place(resolved)
                .expect("eye-side placement commits");
            assert_eq!(outcome.changed.len(), 1);
            assert_eq!(outcome.changed[0].pos, BlockPos::new(0, 65, 0));
            assert_eq!(outcome.changed[0].block, DIRT);
            let view = context.read();
            assert_eq!(
                view.observation(Dimension::OVERWORLD, BlockPos::new(0, 65, 1))
                    .unwrap()
                    .block,
                DOOR_LOWER_SOUTH_CLOSED
            );
            assert_eq!(
                view.observation(Dimension::OVERWORLD, BlockPos::new(0, 65, 2))
                    .unwrap()
                    .block,
                STONE
            );
        },
    );
}

#[test]
fn compound_inventory_repeated_preimage_refuses_without_effects() {
    fixture_case(
        AIR,
        AIR,
        STONE,
        ItemStack {
            item: ITEM_DIRT,
            count: 2,
            durability: 0,
        },
        |ctx, session| {
            let actor = ActorKey::Player(session);
            let before_inventory = *ctx.read().inventory(actor).unwrap();
            let mut after = before_inventory;
            after.slots[0].count = 1;
            let patch = InventoryPatch::try_new(actor, before_inventory, after).unwrap();
            let before = ctx.snapshot_state(super::world_outputs::world());
            let cells = [
                BlockPos::new(0, 65, 0),
                BlockPos::new(0, 65, 1),
                BlockPos::new(0, 65, 2),
            ];
            let before_cells = probe(ctx, &cells, &[actor], &[], &[overworld_key(cells[0])]);
            let mut changed_actor = ctx.read().actor(actor).unwrap().clone();
            changed_actor.lifecycle = ActorLifecycle::Dead;
            assert_eq!(
                ctx.stage(RuleEffect::Compound(vec![
                    RuleEffect::Actor(changed_actor),
                    RuleEffect::Inventory(patch.clone()),
                    RuleEffect::Inventory(patch),
                ])),
                Err(RuleReject::StaleObservation)
            );
            assert_eq!(ctx.snapshot_state(super::world_outputs::world()), before);
            assert_eq!(
                probe(ctx, &cells, &[actor], &[], &[overworld_key(cells[0])]),
                before_cells
            );
        },
    );
}

#[test]
fn compound_inventory_chain_uses_listed_order() {
    for reversed in [false, true] {
        fixture_case(
            AIR,
            AIR,
            STONE,
            ItemStack {
                item: ITEM_DIRT,
                count: 2,
                durability: 0,
            },
            |ctx, session| {
                let actor = ActorKey::Player(session);
                let before = *ctx.read().inventory(actor).unwrap();
                let mut middle = before;
                middle.slots[0].count = 1;
                let mut after = middle;
                after.slots[0] = ItemStack::default();
                let mut effects = vec![
                    RuleEffect::Inventory(InventoryPatch::try_new(actor, before, middle).unwrap()),
                    RuleEffect::Inventory(InventoryPatch::try_new(actor, middle, after).unwrap()),
                ];
                if reversed {
                    effects.reverse();
                }
                let snapshot = ctx.snapshot_state(super::world_outputs::world());
                let result = ctx.stage(RuleEffect::Compound(effects));
                if reversed {
                    assert_eq!(result, Err(RuleReject::StaleObservation));
                    assert_eq!(ctx.snapshot_state(super::world_outputs::world()), snapshot);
                } else {
                    result.unwrap();
                    assert_eq!(ctx.read().inventory(actor), Some(&after));
                }
            },
        );
    }
}

#[test]
fn compound_inventory_distinct_actor_chains_do_not_interfere() {
    let mut state = authority();
    let first = state
        .admit(admitted(91, "First"), TransportKind::Memory)
        .unwrap();
    let second = state
        .admit(admitted(92, "Second"), TransportKind::Memory)
        .unwrap();
    let actors = [ActorKey::Player(first), ActorKey::Player(second)];
    let mut ctx = harness_context(&mut state);
    let before = hotbar_inventory(0, ITEM_DIRT, 2, 0);
    let mut middle = before;
    middle.slots[0].count = 1;
    let after = InventoryRecord::empty();
    for session in [first, second] {
        ctx.stage(RuleEffect::Actor(player_actor(
            session,
            [0.5, 64.0, 0.5],
            0.0,
            0.0,
        )))
        .unwrap();
        ctx.preload_inventory(ActorKey::Player(session), before);
    }
    ctx.stage(RuleEffect::Compound(vec![
        RuleEffect::Inventory(InventoryPatch::try_new(actors[0], before, middle).unwrap()),
        RuleEffect::Inventory(InventoryPatch::try_new(actors[1], before, middle).unwrap()),
        RuleEffect::Inventory(InventoryPatch::try_new(actors[0], middle, after).unwrap()),
        RuleEffect::Inventory(InventoryPatch::try_new(actors[1], middle, after).unwrap()),
    ]))
    .unwrap();
    for actor in actors {
        assert_eq!(ctx.read().inventory(actor), Some(&after));
    }
}

#[test]
fn single_inventory_effect_retains_compare_and_swap() {
    fixture_case(
        AIR,
        AIR,
        STONE,
        ItemStack {
            item: ITEM_DIRT,
            count: 2,
            durability: 0,
        },
        |ctx, session| {
            let actor = ActorKey::Player(session);
            let before = *ctx.read().inventory(actor).unwrap();
            let mut after = before;
            after.slots[0].count = 1;
            let patch = InventoryPatch::try_new(actor, before, after).unwrap();
            ctx.stage(RuleEffect::Inventory(patch.clone())).unwrap();
            let snapshot = ctx.snapshot_state(super::world_outputs::world());
            assert_eq!(
                ctx.stage(RuleEffect::Inventory(patch)),
                Err(RuleReject::StaleObservation)
            );
            assert_eq!(ctx.snapshot_state(super::world_outputs::world()), snapshot);
        },
    );
}

fn assert_support_place(
    context: &mut TickContext<'_>,
    session: SessionKey,
    intent: &PlacementIntent,
    target: BlockPos,
    form: u16,
    refusal: Option<RejectReason>,
    cells: &[BlockPos],
) {
    let actor = ActorKey::Player(session);
    let chunks = [overworld_key(target)];
    let before = probe(context, cells, &[actor], &[chest_reference()], &chunks);
    let resolved = resolve_place(actor, intent, &context.read());
    assert_eq!(
        probe(context, cells, &[actor], &[chest_reference()], &chunks),
        before
    );
    if let Some(reason) = refusal {
        assert_eq!(resolved, Err(RuleReject::Wire(reason)));
        return;
    }
    let outcome = context
        .transaction()
        .try_place(resolved.expect("support permits placement"))
        .expect("commit");
    assert_eq!(outcome.changed.len(), 1);
    assert_eq!(outcome.changed[0].pos, target);
    assert_eq!(outcome.changed[0].block, form);
    assert!(outcome.inventory_changed);
    let mut expected = before;
    expected.inventories[0].1.as_mut().unwrap().slots[0].count -= 1;
    let cell = expected
        .cells
        .iter_mut()
        .find(|(pos, _)| *pos == target)
        .unwrap()
        .1
        .as_mut()
        .unwrap();
    cell.block = form;
    cell.revision += 1;
    assert_eq!(
        probe(context, cells, &[actor], &[chest_reference()], &chunks),
        expected
    );
}

fn support_geometry(
    position: [f32; 3],
    look: LookAngles,
    item: u16,
    cells: &[(BlockPos, u16)],
    case: impl FnOnce(&mut TickContext<'_>, SessionKey, &PlacementIntent),
) {
    let mut state = authority();
    let session = state
        .admit(admitted(1, "Ada"), TransportKind::Memory)
        .unwrap();
    let actor = ActorKey::Player(session);
    let mut context = harness_context(&mut state);
    context
        .stage(RuleEffect::Environment(environment()))
        .unwrap();
    context
        .stage(RuleEffect::Actor(player_actor(
            session,
            position,
            look.yaw(),
            look.pitch(),
        )))
        .unwrap();
    context.preload_inventory(actor, hotbar_inventory(0, item, 3, 0));
    for (pos, block) in cells {
        context.preload_block(observation(*pos, *block));
    }
    let intent = PlacementIntent::try_new(look, 0).unwrap();
    case(&mut context, session, &intent);
}

#[test]
fn crop_side_face_requires_farmland_below_destination() {
    for (item, form) in [(34, 37), (40, 46), (41, 54)] {
        for support in [STONE, 35, 36] {
            fixture_case(
                AIR,
                AIR,
                STONE,
                ItemStack {
                    item,
                    count: 3,
                    durability: 0,
                },
                |ctx, session| {
                    let below = BlockPos::new(0, 64, 1);
                    ctx.preload_block(observation(below, support));
                    assert_support_place(
                        ctx,
                        session,
                        &south_intent(),
                        BlockPos::new(0, 65, 1),
                        form,
                        (support == STONE).then_some(RejectReason::InvalidBlock),
                        &[
                            BlockPos::new(0, 65, 0),
                            BlockPos::new(0, 65, 1),
                            BlockPos::new(0, 65, 2),
                            below,
                        ],
                    );
                },
            );
        }
    }
}

#[test]
fn sapling_side_face_requires_dirt_or_grass_below_destination() {
    for support in [STONE, 35, 36, DIRT, 4] {
        fixture_case(
            AIR,
            AIR,
            STONE,
            ItemStack {
                item: 57,
                count: 3,
                durability: 0,
            },
            |ctx, session| {
                let below = BlockPos::new(0, 64, 1);
                ctx.preload_block(observation(below, support));
                assert_support_place(
                    ctx,
                    session,
                    &south_intent(),
                    BlockPos::new(0, 65, 1),
                    89,
                    (!matches!(support, 3 | 4)).then_some(RejectReason::InvalidBlock),
                    &[
                        BlockPos::new(0, 65, 0),
                        BlockPos::new(0, 65, 1),
                        BlockPos::new(0, 65, 2),
                        below,
                    ],
                );
            },
        );
    }
}

#[test]
fn plant_missing_support_refuses_without_publication() {
    for item in [34, 40, 41, 57] {
        fixture_case(
            AIR,
            AIR,
            STONE,
            ItemStack {
                item,
                count: 3,
                durability: 0,
            },
            |ctx, session| {
                assert_support_place(
                    ctx,
                    session,
                    &south_intent(),
                    BlockPos::new(0, 65, 1),
                    0,
                    Some(RejectReason::ChunkNotReady),
                    &[
                        BlockPos::new(0, 65, 0),
                        BlockPos::new(0, 65, 1),
                        BlockPos::new(0, 65, 2),
                        BlockPos::new(0, 64, 1),
                    ],
                );
            },
        );
    }
}

#[test]
fn torch_standing_rejects_zero_collision_support_and_accepts_source_boxes() {
    let rejected = [
        37, 44, 46, 53, 54, 61, 84, 89, 71, 72, 73, 74, 75, 85, 86, 87, 88, 70,
    ];
    let accepted = [
        STONE, 19, 20, 35, 36, 76, 77, 78, 79, 80, 81, 82, 83, 62, 64, 66, 68,
    ];
    for (support, refusal) in rejected
        .into_iter()
        .map(|block| (block, Some(RejectReason::InvalidBlock)))
        .chain(accepted.into_iter().map(|block| (block, None)))
    {
        let cells = [
            (BlockPos::new(0, 67, 0), AIR),
            (BlockPos::new(0, 66, 0), AIR),
            (BlockPos::new(0, 65, 0), AIR),
            (BlockPos::new(0, 64, 0), support),
        ];
        support_geometry(
            [0.5, 66.0, 0.5],
            LookAngles::try_new(0.0, -std::f32::consts::FRAC_PI_2).unwrap(),
            44,
            &cells,
            |ctx, session, intent| {
                assert_support_place(
                    ctx,
                    session,
                    intent,
                    BlockPos::new(0, 65, 0),
                    71,
                    refusal,
                    &cells.map(|(pos, _)| pos),
                );
            },
        );
    }
}

#[test]
fn torch_wall_faces_place_on_the_source_hit_support() {
    for (yaw, step, form) in [
        (std::f32::consts::FRAC_PI_2, (-1, 0), 72),
        (-std::f32::consts::FRAC_PI_2, (1, 0), 73),
        (0.0, (0, -1), 74),
        (std::f32::consts::PI, (0, 1), 75),
    ] {
        let target = BlockPos::new(step.0, 65, step.1);
        let cells = [
            (BlockPos::new(0, 65, 0), AIR),
            (target, AIR),
            (BlockPos::new(step.0 * 2, 65, step.1 * 2), STONE),
        ];
        support_geometry(
            [0.5, 64.0, 0.5],
            LookAngles::try_new(yaw, 0.0).unwrap(),
            44,
            &cells,
            |ctx, session, intent| {
                assert_support_place(
                    ctx,
                    session,
                    intent,
                    target,
                    form,
                    None,
                    &cells.map(|(pos, _)| pos),
                );
            },
        );
    }
}

#[test]
fn torch_bottom_face_still_has_no_placeable_form() {
    let cells = [
        (BlockPos::new(0, 65, 0), AIR),
        (BlockPos::new(0, 66, 0), AIR),
        (BlockPos::new(0, 67, 0), STONE),
    ];
    support_geometry(
        [0.5, 64.0, 0.5],
        LookAngles::try_new(0.0, std::f32::consts::FRAC_PI_2).unwrap(),
        44,
        &cells,
        |ctx, session, intent| {
            assert_support_place(
                ctx,
                session,
                intent,
                BlockPos::new(0, 66, 0),
                0,
                Some(RejectReason::InvalidBlock),
                &cells.map(|(pos, _)| pos),
            );
        },
    );
}
