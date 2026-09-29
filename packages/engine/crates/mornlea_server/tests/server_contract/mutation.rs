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
    expected.slots[0] = ItemStack {
        item: ITEM_CHEST,
        count: 1,
        durability: 0,
    };
    for slot in 1..=CHEST_SLOTS {
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
        1
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
