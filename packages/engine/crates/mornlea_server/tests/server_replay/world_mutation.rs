//! Placement geometry and door interaction replay.
//!
//! Every gate below mirrors a frozen Go oracle row, cited at each case:
//!
//! - Door and bed footprints, the closed-before-open lower order and the
//!   upper-cell single form from `packages/shared/core/block.go` with the
//!   placement rules in `packages/server/sim/entity/door.go` (`tryPlaceDoor`)
//!   and `packages/server/sim/entity/bed.go` (`tryPlaceBed`).
//! - The door toggle rule in `packages/server/sim/entity/door.go`
//!   (`handleInteractDoor`, `executeInteractDoor`): only the lower half flips,
//!   the upper half stays the single upper form, both halves must be present
//!   and well formed, an unready partner refuses, and a ray that finds no door
//!   is a silent no-op. The sneak gate in that file reads the held-controls
//!   bit owned by the movement provider; the frozen tick context exposes no
//!   held-controls surface, so this node stages no sneak branch and records
//!   the deferral in the provider module.
//! - Ray reach from the tunables (`engine.tunables.InteractionReach`,
//!   default 6.0) with the eye-height origin and the `LookDirection` formula
//!   in `packages/server/sim/entity/command.go`.
//! - First-wins contention through the shared atomic transaction: the loser
//!   re-resolves against the refreshed view and refuses, mirroring the
//!   transaction ordering the authority resolvers commit through.
//! - The duplicate-delivery boundary the inventory provider pins: replaying an
//!   identical envelope after the first settlement refuses with the record
//!   unchanged, because the world basis moved under it.
//!
//! No case chooses a value the oracle does not pin. Refusals compare a
//! before/after probe of cells, full inventory records and events, not a bare
//! `is_err`.

use mornlea_domain::{
    BlockPos, ChunkPos, Command, CommandEnvelope, CommandEnvelopeParts, CompanionId, Dimension,
    FiniteVec3, HeldActions, HotbarSlot, LookAngles, MotionState, MotionStateParts, Movement,
    PlacementIntent, PlayerControl, PlayerControlParts, RejectReason, SurvivalState,
    SurvivalStateParts, Weather,
};
use mornlea_protocol::{LoginStart, admit_login};
use mornlea_server::contracts::*;
use mornlea_server::core::mutation::{resolve_companion_place, resolve_mine, resolve_place};
use mornlea_server::rules::world_mutation as provider;
use mornlea_server::state::{AuthorityState, TickContext};
use mornlea_storage::{CompanionBody, ItemStack, PlayerLocation, PlayerSave};

// Stable block numbers, mirrored from the frozen const block in
// `packages/shared/core/block.go`.
const AIR: u16 = 0; // `core.AirID`
const STONE: u16 = 2; // `core.StoneID`
const DIRT: u16 = 3; // `core.DirtID`
const BEDROCK: u16 = 5; // `core.BedrockID`
const DOOR_LOWER_NORTH_CLOSED: u16 = 66;
const DOOR_LOWER_NORTH_OPEN: u16 = 67;
const DOOR_UPPER: u16 = 70; // `core.DoorUpper`
const BED_FOOT_EAST: u16 = 79; // `core.BedFootEastID`
const BED_HEAD_EAST: u16 = 83; // `core.BedHeadEastID`
// Stable item numbers, mirrored from the frozen const block in
// `packages/shared/core/item.go`.
const ITEM_DIRT: u16 = 2; // `core.ItemDirt`
const ITEM_DOOR: u16 = 43; // `core.ItemDoor`
const ITEM_BED: u16 = 46; // `core.ItemBed`

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

fn overworld_key(pos: BlockPos) -> ChunkKey {
    ChunkKey {
        dimension: Dimension::OVERWORLD,
        pos: ChunkPos::new(pos.x() >> 4, pos.z() >> 4),
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

fn hotbar_inventory(slot: usize, item: u16, count: u8) -> InventoryRecord {
    let mut record = InventoryRecord::empty();
    record.slots[slot] = ItemStack {
        item,
        count,
        durability: 0,
    };
    record
}

/// South-facing horizontal look in hotbar slot zero, the placement intent the
/// envelope carries: look plus slot only, never a target or revision.
fn south_intent() -> PlacementIntent {
    PlacementIntent::try_new(
        LookAngles::try_new(std::f32::consts::PI, 0.0).expect("look"),
        0,
    )
    .expect("placement intent")
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

fn place_call(envelope: &CommandEnvelope) -> RuleCall<'_> {
    RuleCall {
        phase: RulePhase::Interaction,
        actor: None,
        command: Some(envelope),
        internal: None,
    }
}

fn door_call(interaction: &AuthorityInteraction) -> RuleCall<'_> {
    RuleCall {
        phase: RulePhase::Interaction,
        actor: None,
        command: None,
        internal: Some(interaction),
    }
}

fn door_interaction(
    session: SessionKey,
    yaw: f32,
    pitch: f32,
    sequence: u64,
) -> AuthorityInteraction {
    AuthorityInteraction {
        session,
        look: LookAngles::try_new(yaw, pitch).expect("look"),
        kind: InteractionKind::Door,
        sequence,
    }
}

/// Observable authority state a refusal must leave untouched: probed cells
/// with their full observations, actor inventories and the event count.
#[derive(Clone, Debug, PartialEq)]
struct Probe {
    cells: Vec<(BlockPos, Option<BlockObservation>)>,
    inventories: Vec<(ActorKey, Option<InventoryRecord>)>,
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
        events: context.events().len(),
    }
}

/// One aimed south placement scene: an active player at `[0.5, 64.0, 0.5]`
/// looking south with `held` in hotbar slot zero, the eye cell and target
/// cell staged by the caller alongside the hit cell.
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
    let mut inventory = InventoryRecord::empty();
    inventory.slots[0] = held;
    context.preload_inventory(actor, inventory);
    for (pos, block) in cells {
        context.preload_block(observation(*pos, *block));
    }
    actor
}

/// The frozen row end to end: a valid door placement writes both halves
/// preserving direction, an internal door toggle flips the lower half while
/// the upper half keeps its single form, a malformed or unready partner cell
/// refuses, and ray reach refuses. Bed footprints follow the registered block
/// rules at two cells and may cross a chunk seam.
#[test]
fn door_pair_and_internal_toggle() {
    // Valid door placement through the provider: yaw PI faces north, so the
    // lower half is the north closed form with the single upper form above.
    let lower = BlockPos::new(0, 65, 1);
    let upper = BlockPos::new(0, 66, 1);
    let cells = [
        BlockPos::new(0, 65, 0),
        lower,
        upper,
        BlockPos::new(0, 64, 1),
        BlockPos::new(0, 65, 2),
    ];
    let mut state = authority();
    let session = state
        .admit(admitted(1, "Ada"), TransportKind::Memory)
        .expect("session");
    let mut context = harness_context(&mut state);
    let actor = south_scene(
        &mut context,
        session,
        ItemStack {
            item: ITEM_DOOR,
            count: 1,
            durability: 0,
        },
        &[
            (BlockPos::new(0, 65, 0), AIR),
            (lower, AIR),
            (upper, AIR),
            (BlockPos::new(0, 64, 1), STONE),
            (BlockPos::new(0, 65, 2), STONE),
        ],
    );
    let placed = envelope(session, 1, Command::PlaceBlock(south_intent()));
    let report = provider::run(&mut context, place_call(&placed)).expect("door placed");
    assert_eq!(
        report,
        PhaseReport {
            examined: 1,
            applied: 1,
            carried: 0,
            rejected: 0
        }
    );
    let lower_after = context
        .read()
        .observation(Dimension::OVERWORLD, lower)
        .expect("lower half");
    assert_eq!(lower_after.block, DOOR_LOWER_NORTH_CLOSED);
    assert_eq!(lower_after.revision, 2);
    let upper_after = context
        .read()
        .observation(Dimension::OVERWORLD, upper)
        .expect("upper half");
    assert_eq!(upper_after.block, DOOR_UPPER);
    assert_eq!(upper_after.revision, 2);
    assert_eq!(
        context.read().inventory(actor).expect("inventory").slots[0],
        ItemStack::default(),
        "one door item debited"
    );
    assert_eq!(context.events().len(), 0);

    // Bed footprint across a chunk seam through the provider: east-facing foot
    // at x=15 in chunk 0 with the head at x=16 in chunk 1, both revision 2.
    let foot = BlockPos::new(15, 65, 0);
    let head = BlockPos::new(16, 65, 0);
    let bed_look = LookAngles::try_new(-std::f32::consts::FRAC_PI_2, -1.4).expect("look");
    let mut state = authority();
    let session = state
        .admit(admitted(2, "Bea"), TransportKind::Memory)
        .expect("session");
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
    let bed_actor = ActorKey::Player(session);
    context.preload_inventory(bed_actor, hotbar_inventory(0, ITEM_BED, 2));
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
    let bed_intent = PlacementIntent::try_new(bed_look, 0).expect("bed intent");
    let bed_placed = envelope(session, 2, Command::PlaceBlock(bed_intent));
    let report = provider::run(&mut context, place_call(&bed_placed)).expect("bed placed");
    assert_eq!(report.applied, 1);
    assert_eq!(
        context
            .read()
            .observation(Dimension::OVERWORLD, foot)
            .expect("foot cell")
            .block,
        BED_FOOT_EAST
    );
    assert_eq!(
        context
            .read()
            .observation(Dimension::OVERWORLD, head)
            .expect("head cell")
            .block,
        BED_HEAD_EAST
    );
    assert_eq!(
        context
            .read()
            .observation(Dimension::OVERWORLD, foot)
            .expect("foot cell")
            .revision,
        2
    );
    assert_eq!(
        context
            .read()
            .observation(Dimension::OVERWORLD, head)
            .expect("head cell")
            .revision,
        2
    );
    assert_eq!(
        context
            .read()
            .inventory(bed_actor)
            .expect("inventory")
            .slots[0]
            .count,
        1
    );

    // Internal toggle aimed at the lower half flips closed to open while the
    // upper half keeps its single form at the same revision; the toggle debits
    // no inventory.
    let mut state = authority();
    let session = state
        .admit(admitted(3, "Cara"), TransportKind::Memory)
        .expect("session");
    let mut context = harness_context(&mut state);
    let actor = south_scene(
        &mut context,
        session,
        ItemStack::default(),
        &[
            (BlockPos::new(0, 65, 0), AIR),
            (lower, DOOR_LOWER_NORTH_CLOSED),
            (upper, DOOR_UPPER),
            (BlockPos::new(0, 65, 2), STONE),
        ],
    );
    let toggle = door_interaction(session, std::f32::consts::PI, 0.0, 3);
    let report = provider::run(&mut context, door_call(&toggle)).expect("toggled open");
    assert_eq!(report.applied, 1);
    let lower_after = context
        .read()
        .observation(Dimension::OVERWORLD, lower)
        .expect("lower half");
    assert_eq!(lower_after.block, DOOR_LOWER_NORTH_OPEN);
    assert_eq!(lower_after.revision, 2);
    let upper_after = context
        .read()
        .observation(Dimension::OVERWORLD, upper)
        .expect("upper half");
    assert_eq!(upper_after.block, DOOR_UPPER);
    assert_eq!(
        upper_after.revision, 1,
        "the upper half keeps its form and revision"
    );
    assert_eq!(
        context.read().inventory(actor).expect("inventory").slots[0],
        ItemStack::default()
    );
    assert_eq!(context.events().len(), 0);

    // Toggle back aimed at the upper half: the eye sits one cell higher so the
    // horizontal ray meets the upper cell first, and the lower half flips open
    // to closed with its direction preserved.
    let mut state = authority();
    let session = state
        .admit(admitted(4, "Dan"), TransportKind::Memory)
        .expect("session");
    let mut context = harness_context(&mut state);
    context
        .stage(RuleEffect::Environment(environment()))
        .expect("environment");
    context
        .stage(RuleEffect::Actor(player_actor(
            session,
            [0.5, 65.0, 0.5],
            std::f32::consts::PI,
            0.0,
        )))
        .expect("actor");
    let high_actor = ActorKey::Player(session);
    context.preload_inventory(high_actor, InventoryRecord::empty());
    context.preload_block(observation(BlockPos::new(0, 66, 0), AIR));
    context.preload_block(observation(upper, DOOR_UPPER));
    context.preload_block(observation(lower, DOOR_LOWER_NORTH_OPEN));
    let toggle = door_interaction(session, std::f32::consts::PI, 0.0, 4);
    provider::run(&mut context, door_call(&toggle)).expect("toggled closed");
    assert_eq!(
        context
            .read()
            .observation(Dimension::OVERWORLD, lower)
            .expect("lower half")
            .block,
        DOOR_LOWER_NORTH_CLOSED
    );
    assert_eq!(
        context
            .read()
            .observation(Dimension::OVERWORLD, upper)
            .expect("upper half")
            .block,
        DOOR_UPPER
    );

    // Malformed partner: the upper cell holds stone, so the toggle refuses
    // with both cells and the inventory unchanged.
    let mut state = authority();
    let session = state
        .admit(admitted(5, "Eve"), TransportKind::Memory)
        .expect("session");
    let mut context = harness_context(&mut state);
    let actor = south_scene(
        &mut context,
        session,
        ItemStack::default(),
        &[
            (BlockPos::new(0, 65, 0), AIR),
            (lower, DOOR_LOWER_NORTH_CLOSED),
            (upper, STONE),
            (BlockPos::new(0, 65, 2), STONE),
        ],
    );
    let before = probe(&context, &cells, &[actor]);
    let toggle = door_interaction(session, std::f32::consts::PI, 0.0, 5);
    assert!(provider::run(&mut context, door_call(&toggle)).is_err());
    assert_eq!(probe(&context, &cells, &[actor]), before);

    // Unready partner: the upper cell is unobserved, so the toggle refuses
    // with the lower cell and the inventory unchanged.
    let mut state = authority();
    let session = state
        .admit(admitted(6, "Fay"), TransportKind::Memory)
        .expect("session");
    let mut context = harness_context(&mut state);
    let actor = south_scene(
        &mut context,
        session,
        ItemStack::default(),
        &[
            (BlockPos::new(0, 65, 0), AIR),
            (lower, DOOR_LOWER_NORTH_CLOSED),
            (BlockPos::new(0, 65, 2), STONE),
        ],
    );
    let ready_cells = [BlockPos::new(0, 65, 0), lower, BlockPos::new(0, 65, 2)];
    let before = probe(&context, &ready_cells, &[actor]);
    let toggle = door_interaction(session, std::f32::consts::PI, 0.0, 6);
    assert!(provider::run(&mut context, door_call(&toggle)).is_err());
    assert_eq!(probe(&context, &ready_cells, &[actor]), before);

    // A ray that meets a non-door block is the silent no-op the oracle pins:
    // success with nothing applied and nothing changed.
    let mut state = authority();
    let session = state
        .admit(admitted(7, "Gia"), TransportKind::Memory)
        .expect("session");
    let mut context = harness_context(&mut state);
    let actor = south_scene(
        &mut context,
        session,
        ItemStack::default(),
        &[
            (BlockPos::new(0, 65, 0), AIR),
            (BlockPos::new(0, 65, 1), AIR),
            (BlockPos::new(0, 65, 2), STONE),
        ],
    );
    let plain = [
        BlockPos::new(0, 65, 0),
        BlockPos::new(0, 65, 1),
        BlockPos::new(0, 65, 2),
    ];
    let before = probe(&context, &plain, &[actor]);
    let toggle = door_interaction(session, std::f32::consts::PI, 0.0, 7);
    let report = provider::run(&mut context, door_call(&toggle)).expect("non-door no-op");
    assert_eq!(report.applied, 0);
    assert_eq!(probe(&context, &plain, &[actor]), before);

    // Ray reach: the door stands beyond the 6.0 interaction reach, so the
    // exhausted ray refuses with the staged cells and inventory unchanged.
    let mut state = authority();
    let session = state
        .admit(admitted(8, "Hal"), TransportKind::Memory)
        .expect("session");
    let mut context = harness_context(&mut state);
    let actor = south_scene(
        &mut context,
        session,
        ItemStack::default(),
        &[
            (BlockPos::new(0, 65, 0), AIR),
            (BlockPos::new(0, 65, 1), AIR),
            (BlockPos::new(0, 65, 2), AIR),
            (BlockPos::new(0, 65, 3), AIR),
            (BlockPos::new(0, 65, 4), AIR),
            (BlockPos::new(0, 65, 5), AIR),
            (BlockPos::new(0, 65, 6), AIR),
        ],
    );
    let reach_cells: Vec<BlockPos> = (0..=6).map(|z| BlockPos::new(0, 65, z)).collect();
    let before = probe(&context, &reach_cells, &[actor]);
    let toggle = door_interaction(session, std::f32::consts::PI, 0.0, 8);
    assert!(provider::run(&mut context, door_call(&toggle)).is_err());
    assert_eq!(probe(&context, &reach_cells, &[actor]), before);
}

/// Same-tick human place and companion proposal on one cell: the first commits
/// and the loser re-resolves against the refreshed view and refuses, leaving
/// the committed state untouched.
#[test]
fn human_companion_collision_first_wins() {
    let contested = BlockPos::new(0, 65, 1);
    let cells = [BlockPos::new(0, 65, 0), contested, BlockPos::new(0, 65, 2)];

    // Human first: the provider commits dirt, then the companion proposal
    // against the refreshed view refuses with both inventories unchanged.
    let companion = companion_id();
    let mut state = authority();
    let session = state
        .admit(admitted(1, "Ada"), TransportKind::Memory)
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
    context.preload_inventory(player, hotbar_inventory(0, ITEM_DIRT, 2));
    context.preload_inventory(companion_key, hotbar_inventory(3, ITEM_DIRT, 1));
    for (pos, block) in [
        (BlockPos::new(0, 65, 0), AIR),
        (contested, AIR),
        (BlockPos::new(0, 65, 2), STONE),
    ] {
        context.preload_block(observation(pos, block));
    }
    let placed = envelope(session, 1, Command::PlaceBlock(south_intent()));
    provider::run(&mut context, place_call(&placed)).expect("human committed");
    let before = probe(&context, &cells, &[player, companion_key]);
    let refused = resolve_companion_place(companion, contested, DIRT, &context.read());
    assert_eq!(refused, Err(RuleReject::Wire(RejectReason::Occupied)));
    assert_eq!(probe(&context, &cells, &[player, companion_key]), before);
    let committed = context
        .read()
        .observation(Dimension::OVERWORLD, contested)
        .expect("contested cell");
    assert_eq!(committed.block, DIRT);
    assert_eq!(committed.revision, 2);
    assert_eq!(
        context.read().inventory(player).expect("inventory").slots[0].count,
        1
    );
    assert_eq!(
        context
            .read()
            .inventory(companion_key)
            .expect("companion inventory")
            .slots[3]
            .count,
        1,
        "the loser keeps its item"
    );
    assert_eq!(context.events().len(), 0);

    // Companion first: its proposal resolves against the fresh view, then
    // the human commits through the provider, then the companion commits its
    // now-stale resolution and refuses. The winner stays untouched.
    let companion = companion_id();
    let mut state = authority();
    let session = state
        .admit(admitted(2, "Bea"), TransportKind::Memory)
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
    context.preload_inventory(player, hotbar_inventory(0, ITEM_DIRT, 2));
    context.preload_inventory(companion_key, hotbar_inventory(3, ITEM_DIRT, 1));
    for (pos, block) in [
        (BlockPos::new(0, 65, 0), AIR),
        (contested, AIR),
        (BlockPos::new(0, 65, 2), STONE),
    ] {
        context.preload_block(observation(pos, block));
    }
    let proposal = resolve_companion_place(companion, contested, DIRT, &context.read())
        .expect("companion place");
    let placed = envelope(session, 1, Command::PlaceBlock(south_intent()));
    provider::run(&mut context, place_call(&placed)).expect("human committed");
    let before = probe(&context, &cells, &[player, companion_key]);
    let refused = context.transaction().try_place(proposal);
    assert_eq!(refused, Err(RuleReject::StaleObservation));
    assert_eq!(probe(&context, &cells, &[player, companion_key]), before);
    let committed = context
        .read()
        .observation(Dimension::OVERWORLD, contested)
        .expect("contested cell");
    assert_eq!(committed.block, DIRT);
    assert_eq!(committed.revision, 2);
    assert_eq!(
        context.read().inventory(player).expect("inventory").slots[0].count,
        1,
        "the human winner debited once"
    );
    assert_eq!(
        context
            .read()
            .inventory(companion_key)
            .expect("companion inventory")
            .slots[3]
            .count,
        1,
        "the stale loser keeps its item"
    );
    assert_eq!(context.events().len(), 0);
}

/// Stale chunk generation/revision, an empty selected slot, a protected mining
/// target and a duplicate command each refuse with the world cells and the
/// full inventory records unchanged.
#[test]
fn stale_missing_protected_duplicate_refused() {
    let target = BlockPos::new(0, 65, 1);
    let cells = [BlockPos::new(0, 65, 0), target, BlockPos::new(0, 65, 2)];

    // Stale generation: the target cell reloads at generation 2 between
    // resolve and commit, so the commit refuses and stages nothing.
    let mut state = authority();
    let session = state
        .admit(admitted(1, "Ada"), TransportKind::Memory)
        .expect("session");
    let mut context = harness_context(&mut state);
    let actor = south_scene(
        &mut context,
        session,
        ItemStack {
            item: ITEM_DIRT,
            count: 2,
            durability: 0,
        },
        &[
            (BlockPos::new(0, 65, 0), AIR),
            (target, AIR),
            (BlockPos::new(0, 65, 2), STONE),
        ],
    );
    let resolved = resolve_place(actor, &south_intent(), &context.read()).expect("resolved place");
    context.preload_block(observation_at(target, AIR, 2, 1));
    let before = probe(&context, &cells, &[actor]);
    let refused = context.transaction().try_place(resolved);
    assert_eq!(refused, Err(RuleReject::StaleObservation));
    assert_eq!(probe(&context, &cells, &[actor]), before);

    // Stale revision: an unrelated write advances the target revision between
    // resolve and commit, so the commit refuses and stages nothing.
    let mut state = authority();
    let session = state
        .admit(admitted(2, "Bea"), TransportKind::Memory)
        .expect("session");
    let mut context = harness_context(&mut state);
    let actor = south_scene(
        &mut context,
        session,
        ItemStack {
            item: ITEM_DIRT,
            count: 2,
            durability: 0,
        },
        &[
            (BlockPos::new(0, 65, 0), AIR),
            (target, AIR),
            (BlockPos::new(0, 65, 2), STONE),
        ],
    );
    let resolved = resolve_place(actor, &south_intent(), &context.read()).expect("resolved place");
    let advance = BlockWrite::try_new(observation(target, AIR), AIR).expect("revision advance");
    context
        .transaction()
        .try_system(SystemRule::Support, vec![advance])
        .expect("unrelated write");
    let before = probe(&context, &cells, &[actor]);
    let refused = context.transaction().try_place(resolved);
    assert_eq!(refused, Err(RuleReject::StaleObservation));
    assert_eq!(probe(&context, &cells, &[actor]), before);

    // Empty selected slot: the hotbar oracle pins the empty-slot placement
    // reject, and the provider refuses with cells and inventory unchanged.
    let mut state = authority();
    let session = state
        .admit(admitted(3, "Cara"), TransportKind::Memory)
        .expect("session");
    let mut context = harness_context(&mut state);
    let actor = south_scene(
        &mut context,
        session,
        ItemStack::default(),
        &[
            (BlockPos::new(0, 65, 0), AIR),
            (target, AIR),
            (BlockPos::new(0, 65, 2), STONE),
        ],
    );
    let before = probe(&context, &cells, &[actor]);
    let placed = envelope(session, 3, Command::PlaceBlock(south_intent()));
    assert!(provider::run(&mut context, place_call(&placed)).is_err());
    assert_eq!(probe(&context, &cells, &[actor]), before);

    // Protected target: bedrock has no block-drop entry, so completed mining
    // through the shared geometry refuses with cells and inventory unchanged.
    // Mining progression itself belongs to the mining provider; this leg pins
    // the shared refusal the geometry inherits.
    let mut state = authority();
    let session = state
        .admit(admitted(4, "Dan"), TransportKind::Memory)
        .expect("session");
    let mut context = harness_context(&mut state);
    let actor = south_scene(
        &mut context,
        session,
        ItemStack::default(),
        &[
            (BlockPos::new(0, 65, 0), AIR),
            (target, AIR),
            (BlockPos::new(0, 65, 2), BEDROCK),
        ],
    );
    // The ray meets bedrock two cells out, so the hit cell carries bedrock.
    let before = probe(&context, &cells, &[actor]);
    let refused = resolve_mine(
        actor,
        &PlayerControl::new(PlayerControlParts {
            movement: Movement {
                move_x: 0,
                move_z: 0,
                jump: false,
            },
            look: LookAngles::try_new(std::f32::consts::PI, 0.0).expect("look"),
            actions: HeldActions {
                primary: true,
                eating: false,
                sprinting: false,
                sneaking: false,
            },
        }),
        &context.read(),
    );
    assert_eq!(refused, Err(RuleReject::Wire(RejectReason::ProtectedBlock)));
    assert_eq!(probe(&context, &cells, &[actor]), before);

    // Duplicate command, envelope level: with a single item in the slot the
    // first settlement consumes it and replaying the identical envelope
    // refuses against the emptied slot with cells and inventory unchanged.
    // A re-resolved ray would legitimately re-target in front of moved world
    // cells, so envelope redelivery is closed by the ordering layer sequence
    // gate with this source-empty refusal as the provider-visible remainder,
    // the same boundary the inventory provider pins.
    let mut state = authority();
    let session = state
        .admit(admitted(5, "Eve"), TransportKind::Memory)
        .expect("session");
    let mut context = harness_context(&mut state);
    let actor = south_scene(
        &mut context,
        session,
        ItemStack {
            item: ITEM_DIRT,
            count: 1,
            durability: 0,
        },
        &[
            (BlockPos::new(0, 65, 0), AIR),
            (target, AIR),
            (BlockPos::new(0, 65, 2), STONE),
        ],
    );
    let placed = envelope(session, 5, Command::PlaceBlock(south_intent()));
    provider::run(&mut context, place_call(&placed)).expect("first settles");
    let before = probe(&context, &cells, &[actor]);
    assert!(provider::run(&mut context, place_call(&placed)).is_err());
    assert_eq!(probe(&context, &cells, &[actor]), before);
    assert_eq!(
        context
            .read()
            .observation(Dimension::OVERWORLD, target)
            .expect("target cell")
            .block,
        DIRT
    );

    // Duplicate command, transaction level: committing the same resolution
    // twice refuses the second commit on the advanced revision with cells and
    // inventory unchanged. The provider delegates to exactly this path.
    let mut state = authority();
    let session = state
        .admit(admitted(6, "Fay"), TransportKind::Memory)
        .expect("session");
    let mut context = harness_context(&mut state);
    let actor = south_scene(
        &mut context,
        session,
        ItemStack {
            item: ITEM_DIRT,
            count: 2,
            durability: 0,
        },
        &[
            (BlockPos::new(0, 65, 0), AIR),
            (target, AIR),
            (BlockPos::new(0, 65, 2), STONE),
        ],
    );
    let resolved = resolve_place(actor, &south_intent(), &context.read()).expect("resolved place");
    context
        .transaction()
        .try_place(resolved.clone())
        .expect("first commits");
    let before = probe(&context, &cells, &[actor]);
    let refused = context.transaction().try_place(resolved);
    assert_eq!(refused, Err(RuleReject::StaleObservation));
    assert_eq!(probe(&context, &cells, &[actor]), before);

    // Foreign shapes refuse without effect: a wrong phase, a missing command,
    // a non-placement command and the bed interaction the sleep provider owns.
    let mut state = authority();
    let session = state
        .admit(admitted(7, "Gus"), TransportKind::Memory)
        .expect("session");
    let mut context = harness_context(&mut state);
    let actor = south_scene(
        &mut context,
        session,
        ItemStack {
            item: ITEM_DIRT,
            count: 2,
            durability: 0,
        },
        &[
            (BlockPos::new(0, 65, 0), AIR),
            (target, AIR),
            (BlockPos::new(0, 65, 2), STONE),
        ],
    );
    let before = probe(&context, &cells, &[actor]);
    let placed = envelope(session, 6, Command::PlaceBlock(south_intent()));
    assert!(
        provider::run(
            &mut context,
            RuleCall {
                phase: RulePhase::Publish,
                actor: None,
                command: Some(&placed),
                internal: None,
            }
        )
        .is_err()
    );
    assert!(
        provider::run(
            &mut context,
            RuleCall {
                phase: RulePhase::Interaction,
                actor: None,
                command: None,
                internal: None,
            }
        )
        .is_err()
    );
    let foreign = envelope(
        session,
        7,
        Command::SelectHotbar(HotbarSlot::new(4).expect("slot")),
    );
    assert!(provider::run(&mut context, place_call(&foreign)).is_err());
    let bed = AuthorityInteraction {
        session,
        look: LookAngles::try_new(std::f32::consts::PI, 0.0).expect("look"),
        kind: InteractionKind::Bed,
        sequence: 8,
    };
    assert!(provider::run(&mut context, door_call(&bed)).is_err());
    assert_eq!(probe(&context, &cells, &[actor]), before);
}

/// A committed placement is visible to the same-tick fluid/support read: the
/// staged overlay the later phases read returns the placed cell with one
/// revision advance.
#[test]
fn mutation_before_fluid_support_order() {
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
            item: ITEM_DIRT,
            count: 2,
            durability: 0,
        },
        &[
            (BlockPos::new(0, 65, 0), AIR),
            (target, AIR),
            (BlockPos::new(0, 65, 2), STONE),
        ],
    );
    let placed = envelope(session, 1, Command::PlaceBlock(south_intent()));
    provider::run(&mut context, place_call(&placed)).expect("placed");
    // The later-phase read surface is the context overlay observation: the
    // placed cell reads back with the committed block at revision 2 while an
    // untouched cell keeps revision 1.
    let read = context
        .read()
        .observation(Dimension::OVERWORLD, target)
        .expect("placed cell");
    assert_eq!(read.block, DIRT);
    assert_eq!(read.revision, 2);
    let hit = context
        .read()
        .observation(Dimension::OVERWORLD, BlockPos::new(0, 65, 2))
        .expect("hit cell");
    assert_eq!(hit.block, STONE);
    assert_eq!(hit.revision, 1);
    assert_eq!(
        context.read().inventory(actor).expect("inventory").slots[0].count,
        1
    );
    assert_eq!(context.events().len(), 0);
}
