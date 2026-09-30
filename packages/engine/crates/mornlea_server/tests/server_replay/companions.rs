//! Companion action execution replay.
//!
//! Every scene below mirrors a frozen Go oracle row, cited at each case:
//!
//! - Intent validation from `packages/server/sim/entity/companion_action.go`
//!   (`validCompanionAction`, `applyCompanionActions`): move components in
//!   [-1, 1] with finite yaw, mine/place target Y in `[-64, 320)`, place block
//!   non-air and registered; first valid action per companion ID wins in
//!   arrival order; invalid, unknown-ID, inactive and same-ID later envelopes
//!   are ignored with no effects. Unknown IDs and inactive companions never
//!   select, matching the deterministic-discard rows in
//!   `companion_action_test.go` (`TestCompanionActionInboxBoundedAndSessionless`).
//! - Motion from the same file (`advanceActiveCompanions`) through the shared
//!   player physics exit (`TestCompanionActionSharesPlayerPhysicsExit`):
//!   every active companion steps once per call, a selected `Move` steers with
//!   its components and jump plus the normalized action yaw, and any other
//!   selection or no selection steps neutral input retaining the current yaw.
//! - Placement from `packages/server/sim/entity/companion_placement.go`
//!   (`settleCompanionPlacements`, `completeCompanionPlacement`): ID byte-order
//!   settle through the shared transaction, first-commit-wins on competing
//!   claims, and silent refusal (never a staged failure entry) on stale,
//!   unobserved, consumed, missing-item or competing writes.
//! - Mining intents stay with the accepted mining consumer (`mining.rs`
//!   `run_companion`): hold persists until release, saturation is retained on
//!   a full inventory, and the container batch commits all-or-none. This
//!   provider stages nothing for mining.
//!
//! No case chooses a value the oracle does not pin. Refusals compare a
//! before/after probe of actors, cells, inventories, progress and events,
//! not a bare `is_err`.

use mornlea_domain::{
    BlockPos, ChunkPos, Command, CommandEnvelope, CommandEnvelopeParts, CompanionId, ContainerKind,
    ContainerRef, Dimension, FiniteVec3, HeldActions, LookAngles, MotionState, MotionStateParts,
    Movement, PlayerControl, PlayerControlParts, SurvivalState, SurvivalStateParts, Weather,
};
use mornlea_server::contracts::*;
use mornlea_server::core::companion_ingress::{CompanionIngress, CompanionTaskGate};
use mornlea_server::rules::companions as provider;
use mornlea_server::rules::mining as mine_provider;
use mornlea_server::state::{AuthorityState, TickContext};
use mornlea_storage::{CompanionBody, ItemStack};

// Stable block numbers, mirrored from the frozen const block in
// `packages/shared/core/block.go`.
const AIR: u16 = 0; // `core.AirID`
const STONE: u16 = 2; // `core.StoneID`
const DIRT: u16 = 3; // `core.DirtID`
const GRASS: u16 = 4; // `core.GrassID`
const CHEST: u16 = 11; // `core.ChestID`
// Stable item numbers, mirrored from the frozen const block in
// `packages/shared/core/item.go`.
const ITEM_STONE_PICKAXE: u16 = 10; // `core.ItemStonePickaxe`
const ITEM_DIRT: u16 = 2; // `core.ItemDirt`
const ITEM_STONE: u16 = 1; // `core.ItemStone`

fn authority() -> AuthorityState {
    AuthorityState::try_new(
        ServerLimits::try_new(8, 4096, 512, 64, 64, 1_048_576).expect("limits"),
        7,
    )
    .expect("authority")
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

fn companion_id(tag: u8) -> CompanionId {
    CompanionId::try_from_bytes(uuid(tag)).expect("companion id")
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

fn companion_body(id: CompanionId, position: [f32; 3]) -> CompanionBody {
    CompanionBody {
        id: mornlea_storage::PlayerId::from_bytes(id.bytes()),
        dimension: 0,
        position,
        yaw: 0.0,
        pitch: 0.0,
        inventory: mornlea_storage::Inventory::default(),
    }
}

fn companion_actor(
    id: CompanionId,
    position: [f32; 3],
    yaw: f32,
    lifecycle: ActorLifecycle,
) -> ActorRecord {
    ActorRecord::try_new(
        ActorKey::Companion(id),
        lifecycle,
        Dimension::OVERWORLD,
        MotionState::new(MotionStateParts {
            position: FiniteVec3::try_new(position).expect("position"),
            velocity: FiniteVec3::try_new([0.0, 0.0, 0.0]).expect("velocity"),
            on_ground: true,
        }),
        LookAngles::try_new(yaw, 0.0).expect("look"),
        SurvivalState::try_new(SurvivalStateParts {
            health: 20,
            oxygen: 300,
            hunger: 20,
            saturation_zero: false,
            armor_points: 0,
        })
        .expect("survival"),
        ActorBody::Companion(companion_body(id, position)),
    )
    .expect("companion actor")
}

fn envelope(id: CompanionId, tag: u8, action: CompanionAction) -> CompanionActionEnvelope {
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

/// A raw envelope literal bypassing the constructor, the path a malformed
/// payload takes when it reaches the provider view without ingress cover.
fn raw_envelope(id: CompanionId, tag: u8, action: CompanionAction) -> CompanionActionEnvelope {
    CompanionActionEnvelope {
        companion_id: id,
        source_tick: 0,
        request_id: AgentRequestId::try_from_bytes(uuid(tag)).expect("request id"),
        run_id: RunId::try_from_bytes(uuid(tag + 1)).expect("run id"),
        snapshot_id: SnapshotId::try_from_bytes(uuid(tag + 2)).expect("snapshot id"),
        generation: 1,
        attempt: 1,
        snapshot_digest: [0u8; 32],
        action,
    }
}

fn move_action(move_x: i8, move_z: i8, jump: bool, yaw: f32) -> CompanionAction {
    CompanionAction::Move {
        move_x,
        move_z,
        jump,
        yaw,
    }
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

/// A human command envelope proving the command lane is a distinct shape:
/// the companion batch phases never accept one.
fn human_envelope(session: SessionKey) -> CommandEnvelope {
    let control = PlayerControl::new(PlayerControlParts {
        movement: Movement {
            move_x: 0,
            move_z: 0,
            jump: false,
        },
        look: LookAngles::try_new(0.0, 0.0).expect("look"),
        actions: HeldActions {
            primary: false,
            eating: false,
            sprinting: false,
            sneaking: false,
        },
    });
    CommandEnvelope::try_new(CommandEnvelopeParts {
        tick: 0,
        session: session.get(),
        sequence: 1,
        arrival_index: 0,
        command: Command::PlayerInput(control),
    })
    .expect("envelope")
}

fn intent_call() -> RuleCall<'static> {
    RuleCall {
        phase: RulePhase::CompanionIntent,
        actor: None,
        command: None,
        internal: None,
    }
}

fn motion_call() -> RuleCall<'static> {
    RuleCall {
        phase: RulePhase::CompanionMotion,
        actor: None,
        command: None,
        internal: None,
    }
}

fn placement_call() -> RuleCall<'static> {
    RuleCall {
        phase: RulePhase::CompanionPlacement,
        actor: None,
        command: None,
        internal: None,
    }
}

/// One open room of staged air plus a grass floor at y 0, the
/// movement-flat-world shape from `movementFlatChunk` in
/// `packages/server/sim/entity/movement_test.go`. Two active companions
/// stand at y 1: `mover` under test input and `idler` with a nonzero yaw
/// that neutral input must retain.
fn motion_scene(
    context: &mut TickContext<'_>,
    mover: CompanionId,
    idler: CompanionId,
) -> (ActorKey, ActorKey) {
    context
        .stage(RuleEffect::Environment(environment()))
        .expect("environment");
    context
        .stage(RuleEffect::Actor(companion_actor(
            mover,
            [2.5, 1.0, 0.5],
            0.0,
            ActorLifecycle::Active,
        )))
        .expect("mover");
    context
        .stage(RuleEffect::Actor(companion_actor(
            idler,
            [4.5, 1.0, 2.5],
            0.7,
            ActorLifecycle::Active,
        )))
        .expect("idler");
    for x in -4..=6 {
        for y in -1..=10 {
            for z in -4..=4 {
                context.preload_block(observation(BlockPos::new(x, y, z), AIR));
            }
        }
    }
    for x in -4..=6 {
        for z in -4..=4 {
            context.preload_block(observation(BlockPos::new(x, 0, z), GRASS));
        }
    }
    (ActorKey::Companion(mover), ActorKey::Companion(idler))
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

fn tool(item: u16, durability: u16) -> ItemStack {
    ItemStack {
        item,
        count: 1,
        durability,
    }
}

/// Observable authority state an ignored intent must leave untouched: actor
/// records, probed cells with full observations, inventories, mining
/// progress and events.
#[derive(Clone, Debug, PartialEq)]
struct Probe {
    actors: Vec<(ActorKey, Option<ActorRecord>)>,
    cells: Vec<(BlockPos, Option<BlockObservation>)>,
    inventories: Vec<(ActorKey, Option<InventoryRecord>)>,
    progress: Vec<(ActorKey, Option<MiningProgress>)>,
    events: usize,
}

fn probe(context: &TickContext<'_>, actors: &[ActorKey], cells: &[BlockPos]) -> Probe {
    let view = context.read();
    Probe {
        actors: actors
            .iter()
            .map(|key| (*key, view.actor(*key).cloned()))
            .collect(),
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

/// Intent validation (`validCompanionAction` plus the active-ID loop in
/// `applyCompanionActions`): the first valid envelope per companion wins in
/// arrival order, and every ignored envelope counts rejected with zero
/// staged state.
#[test]
fn intent_first_valid_wins_with_counts_and_no_effects() {
    let mut state = authority();
    let mut context = harness_context(&mut state);
    let active_a = companion_id(1);
    let active_b = companion_id(2);
    let active_c = companion_id(3);
    let active_d = companion_id(4);
    let active_e = companion_id(5);
    let active_f = companion_id(6);
    let unknown = companion_id(7);
    let idle = companion_id(8);
    for (id, yaw) in [
        (active_a, 0.0),
        (active_b, 0.0),
        (active_c, 0.0),
        (active_d, 0.0),
        (active_e, 0.0),
        (active_f, 0.0),
    ] {
        context
            .stage(RuleEffect::Environment(environment()))
            .expect("environment");
        context
            .stage(RuleEffect::Actor(companion_actor(
                id,
                [2.5, 1.0, 0.5],
                yaw,
                ActorLifecycle::Active,
            )))
            .expect("actor");
    }
    context
        .stage(RuleEffect::Actor(companion_actor(
            idle,
            [2.5, 1.0, 0.5],
            0.0,
            ActorLifecycle::Pending,
        )))
        .expect("pending actor");
    let target = BlockPos::new(2, 1, 2);
    // Arrival order: an invalid envelope never blocks the later valid one
    // for the same ID, while anything after the first valid selection is a
    // duplicate even when itself invalid.
    for action in [
        raw_envelope(active_a, 10, move_action(5, 0, false, 0.0)),
        envelope(active_a, 13, move_action(1, 0, false, 0.0)),
        raw_envelope(active_a, 16, move_action(0, 0, false, f32::NAN)),
        envelope(
            active_b,
            19,
            CompanionAction::Place {
                target,
                block: DIRT,
            },
        ),
        envelope(unknown, 22, move_action(1, 0, false, 0.0)),
        envelope(idle, 25, move_action(1, 0, false, 0.0)),
        envelope(
            active_c,
            28,
            CompanionAction::Place {
                target: BlockPos::new(2, 500, 2),
                block: DIRT,
            },
        ),
        envelope(active_d, 31, CompanionAction::Place { target, block: AIR }),
        envelope(
            active_e,
            34,
            CompanionAction::MineHold {
                target: BlockPos::new(2, -100, 2),
            },
        ),
        envelope(active_f, 37, CompanionAction::MineRelease),
    ] {
        context.preload_companion_action(action);
    }
    let keys = [
        ActorKey::Companion(active_a),
        ActorKey::Companion(active_b),
        ActorKey::Companion(active_c),
        ActorKey::Companion(active_d),
        ActorKey::Companion(active_e),
        ActorKey::Companion(active_f),
        ActorKey::Companion(unknown),
        ActorKey::Companion(idle),
    ];
    let before = probe(&context, &keys, &[target]);
    let report = provider::run(&mut context, intent_call()).expect("intent report");
    assert_eq!(
        report,
        PhaseReport {
            examined: 10,
            applied: 3,
            carried: 0,
            rejected: 7,
        }
    );
    assert_eq!(probe(&context, &keys, &[target]), before);
}

/// Any call shape outside the three batch phases refuses with the frozen
/// field before staging anything.
#[test]
fn wrong_shapes_refuse_before_effects() {
    let mut state = authority();
    let session = state
        .admit(admitted(2, "Bea"), TransportKind::Memory)
        .expect("session");
    let mut context = harness_context(&mut state);
    let id = companion_id(1);
    context
        .stage(RuleEffect::Environment(environment()))
        .expect("environment");
    context
        .stage(RuleEffect::Actor(companion_actor(
            id,
            [2.5, 1.0, 0.5],
            0.0,
            ActorLifecycle::Active,
        )))
        .expect("actor");
    let key = ActorKey::Companion(id);
    let before = probe(&context, &[key], &[BlockPos::new(2, 1, 2)]);
    let wrong_phase = RuleCall {
        phase: RulePhase::PlayerCommand,
        actor: None,
        command: None,
        internal: None,
    };
    assert_eq!(
        provider::run(&mut context, wrong_phase).unwrap_err(),
        ServerError::InvalidInput {
            field: "companion_call"
        }
    );
    for phase in [
        RulePhase::CompanionIntent,
        RulePhase::CompanionMotion,
        RulePhase::CompanionPlacement,
    ] {
        let with_actor = RuleCall {
            phase,
            actor: Some(key),
            command: None,
            internal: None,
        };
        assert_eq!(
            provider::run(&mut context, with_actor).unwrap_err(),
            ServerError::InvalidInput {
                field: "companion_call"
            },
            "actor shape {phase:?}"
        );
    }
    let command = human_envelope(session);
    let with_command = RuleCall {
        phase: RulePhase::CompanionIntent,
        actor: None,
        command: Some(&command),
        internal: None,
    };
    assert_eq!(
        provider::run(&mut context, with_command).unwrap_err(),
        ServerError::InvalidInput {
            field: "companion_call"
        }
    );
    let interaction = AuthorityInteraction {
        session,
        look: LookAngles::try_new(0.0, 0.0).expect("look"),
        kind: InteractionKind::Door,
        sequence: 0,
    };
    let with_internal = RuleCall {
        phase: RulePhase::CompanionIntent,
        actor: None,
        command: None,
        internal: Some(&interaction),
    };
    assert_eq!(
        provider::run(&mut context, with_internal).unwrap_err(),
        ServerError::InvalidInput {
            field: "companion_call"
        }
    );
    assert_eq!(probe(&context, &[key], &[BlockPos::new(2, 1, 2)]), before);
}

/// Motion (`advanceActiveCompanions` through the shared player exit): a
/// selected `Move` steers through the kernel while a companion without a
/// `Move` steps neutral input retaining yaw. Only actor records are staged:
/// no input lane persists across ticks.
#[test]
fn motion_steps_move_and_retains_yaw_neutral() {
    let mut state = authority();
    let mut context = harness_context(&mut state);
    let mover = companion_id(1);
    let idler = companion_id(2);
    let (mover_key, idler_key) = motion_scene(&mut context, mover, idler);
    context.preload_companion_action(envelope(mover, 10, move_action(1, 0, false, 0.0)));
    let report = provider::run(&mut context, motion_call()).expect("motion report");
    assert_eq!(
        report,
        PhaseReport {
            examined: 2,
            applied: 2,
            carried: 0,
            rejected: 0,
        }
    );
    let moved = context.read().actor(mover_key).expect("mover").clone();
    assert!(
        moved.motion.position().get()[0] > 2.5,
        "strafe input moves +X like the player exit: {:?}",
        moved.motion.position().get()
    );
    assert_eq!(moved.motion.position().get()[2], 0.5);
    assert!(moved.motion.on_ground());
    assert_eq!(moved.look.yaw(), 0.0);
    let idle = context.read().actor(idler_key).expect("idler").clone();
    assert_eq!(idle.look.yaw(), 0.7, "neutral input retains yaw");
    assert!(idle.motion.on_ground());
    // The provider owns no input lane: nothing but the stepped actor
    // records exists after the call.
    assert_eq!(context.read().runtime(mover_key), None);
    assert_eq!(context.read().runtime(idler_key), None);
    assert_eq!(context.read().mining(mover_key), None);
    assert_eq!(context.events().len(), 0);
}

/// Mining selections never stage through this provider: a `MineHold`
/// selects at intent, steps neutrally at motion, settles nothing at
/// placement, and leaves mining progress to the accepted consumer.
#[test]
fn mining_intents_stage_nothing_here() {
    let mut state = authority();
    let mut context = harness_context(&mut state);
    let miner = companion_id(1);
    let (miner_key, _) = motion_scene(&mut context, miner, companion_id(2));
    let target = BlockPos::new(2, 1, 2);
    context.preload_companion_action(envelope(miner, 10, CompanionAction::MineHold { target }));
    let intent = provider::run(&mut context, intent_call()).expect("intent");
    assert_eq!(intent.applied, 1);
    assert_eq!(intent.rejected, 0);
    let before_yaw = context.read().actor(miner_key).expect("miner").look.yaw();
    provider::run(&mut context, motion_call()).expect("motion");
    assert_eq!(
        context.read().actor(miner_key).expect("miner").look.yaw(),
        before_yaw,
        "mining selection steps neutrally"
    );
    let placement = provider::run(&mut context, placement_call()).expect("placement");
    assert_eq!(
        placement,
        PhaseReport {
            examined: 0,
            applied: 0,
            carried: 0,
            rejected: 0,
        }
    );
    assert_eq!(context.read().mining(miner_key), None);
    assert_eq!(context.events().len(), 0);
}

/// Placement (`settleCompanionPlacements`): selected intents settle in
/// companion-ID byte order through the shared transaction. The second claim
/// on the same cell, the stale target, and the missing item refuse with
/// inventory and world unchanged and no failure entry staged.
#[test]
fn placement_settles_in_id_order_with_atomic_refusals() {
    let mut state = authority();
    let mut context = harness_context(&mut state);
    let first = companion_id(1);
    let second = companion_id(2);
    let stale = companion_id(3);
    let empty_handed = companion_id(4);
    context
        .stage(RuleEffect::Environment(environment()))
        .expect("environment");
    for id in [first, second, stale, empty_handed] {
        context
            .stage(RuleEffect::Actor(companion_actor(
                id,
                [2.5, 1.0, 0.5],
                0.0,
                ActorLifecycle::Active,
            )))
            .expect("actor");
    }
    let shared = BlockPos::new(2, 1, 2);
    let taken = BlockPos::new(3, 1, 2);
    context.preload_block(observation(shared, AIR));
    context.preload_block(observation(taken, STONE));
    let mut first_inventory = InventoryRecord::empty();
    first_inventory.slots[0] = ItemStack {
        item: ITEM_DIRT,
        count: 3,
        durability: 0,
    };
    context.preload_inventory(ActorKey::Companion(first), first_inventory);
    let mut second_inventory = InventoryRecord::empty();
    second_inventory.slots[0] = ItemStack {
        item: ITEM_DIRT,
        count: 1,
        durability: 0,
    };
    context.preload_inventory(ActorKey::Companion(second), second_inventory);
    let mut stale_inventory = InventoryRecord::empty();
    stale_inventory.slots[0] = ItemStack {
        item: ITEM_DIRT,
        count: 1,
        durability: 0,
    };
    context.preload_inventory(ActorKey::Companion(stale), stale_inventory);
    context.preload_inventory(ActorKey::Companion(empty_handed), InventoryRecord::empty());
    // Reverse arrival order proves the settle order comes from the ID bytes,
    // not the queue order.
    for (id, tag, target) in [
        (empty_handed, 10u8, shared),
        (stale, 13u8, taken),
        (second, 16u8, shared),
        (first, 19u8, shared),
    ] {
        context.preload_companion_action(envelope(
            id,
            tag,
            CompanionAction::Place {
                target,
                block: DIRT,
            },
        ));
    }
    let intent = provider::run(&mut context, intent_call()).expect("intent");
    assert_eq!(intent.applied, 4);
    let report = provider::run(&mut context, placement_call()).expect("placement");
    assert_eq!(
        report,
        PhaseReport {
            examined: 4,
            applied: 1,
            carried: 0,
            rejected: 3,
        }
    );
    assert_eq!(
        context
            .read()
            .observation(Dimension::OVERWORLD, shared)
            .expect("cell")
            .block,
        DIRT,
        "lowest ID bytes commit first"
    );
    assert_eq!(
        context
            .read()
            .inventory(ActorKey::Companion(first))
            .expect("inventory")
            .slots[0],
        ItemStack {
            item: ITEM_DIRT,
            count: 2,
            durability: 0,
        },
        "winner debits the first matching stack"
    );
    assert_eq!(
        context
            .read()
            .inventory(ActorKey::Companion(second))
            .expect("inventory")
            .slots[0],
        ItemStack {
            item: ITEM_DIRT,
            count: 1,
            durability: 0,
        },
        "losing claim keeps its item"
    );
    assert_eq!(
        context
            .read()
            .observation(Dimension::OVERWORLD, taken)
            .expect("cell")
            .block,
        STONE,
        "stale target keeps its block"
    );
    assert_eq!(context.events().len(), 0);
}

/// The combined boundary regression: a wrong-digest late candidate is
/// refused at ingress and never reaches the view, so the provider run is a
/// no-op; a held mine target keeps neutral motion with saturated progress
/// retained through the view until release; and a full container target
/// keeps its block with saturated progress instead of a partial credit.
#[test]
fn neutral_hold_release_and_container_atomic() {
    // Provenance stays ingress-owned: the wrong digest refuses before any
    // reservation, so the view never carries the candidate.
    let id = companion_id(1);
    let gate = CompanionTaskGate::try_new(
        id,
        AgentRequestId::try_from_bytes(uuid(0xA1)).expect("request id"),
        RunId::try_from_bytes(uuid(0xB1)).expect("run id"),
        SnapshotId::try_from_bytes(uuid(0xC1)).expect("snapshot id"),
        [0xAB; 32],
        5,
        7,
    )
    .expect("gate");
    let mut ingress = CompanionIngress::try_new().expect("ingress");
    let mut late = CompanionActionEnvelope::try_new(
        id,
        10,
        gate.request_id(),
        gate.run_id(),
        gate.snapshot_id(),
        gate.generation(),
        gate.attempt(),
        gate.snapshot_digest(),
        CompanionAction::MineRelease,
    )
    .expect("gate-bound candidate");
    late.snapshot_digest = [0xCD; 32];
    assert_eq!(
        ingress.admit(&gate, 100, late).unwrap_err(),
        ServerError::InvalidInput {
            field: "snapshot_digest"
        }
    );
    assert_eq!(ingress.pending(), 0);
    let mut state = authority();
    let mut context = harness_context(&mut state);
    let report = provider::run(&mut context, intent_call()).expect("silent intent");
    assert_eq!(
        report,
        PhaseReport {
            examined: 0,
            applied: 0,
            carried: 0,
            rejected: 0,
        },
        "refused candidates never reach the view"
    );

    // Hold persists until release: progress accumulates across provider
    // phases that stage nothing for mining, and release clears.
    let mut state = authority();
    let mut context = harness_context(&mut state);
    let miner = companion_id(2);
    let (miner_key, _) = motion_scene(&mut context, miner, companion_id(3));
    let target = BlockPos::new(2, 1, 2);
    context.preload_block(observation(target, STONE));
    let mut inventory = InventoryRecord::empty();
    inventory.slots[0] = tool(ITEM_STONE_PICKAXE, 100);
    context.preload_inventory(miner_key, inventory);
    context.preload_companion_action(envelope(miner, 43, CompanionAction::MineHold { target }));
    let mine_call = RuleCall {
        phase: RulePhase::MiningStep,
        actor: Some(miner_key),
        command: None,
        internal: None,
    };
    mine_provider::run(&mut context, mine_call).expect("first hold tick");
    assert_eq!(
        context.read().mining(miner_key).expect("progress").elapsed,
        1
    );
    provider::run(&mut context, intent_call()).expect("intent");
    provider::run(&mut context, motion_call()).expect("motion");
    provider::run(&mut context, placement_call()).expect("placement");
    assert_eq!(
        context.read().mining(miner_key).expect("progress").elapsed,
        1,
        "the action provider retains progress through the view"
    );
    mine_provider::run(&mut context, mine_call).expect("second hold tick");
    assert_eq!(
        context.read().mining(miner_key).expect("progress").elapsed,
        2,
        "the hold persists without a new envelope"
    );
    context.preload_companion_action(envelope(miner, 46, CompanionAction::MineRelease));
    mine_provider::run(&mut context, mine_call).expect("release tick");
    assert_eq!(context.read().mining(miner_key), None);

    // Full-container preservation: a chest whose body plus contents cannot
    // credit keeps its block, its contents, the tool wear and the saturated
    // progress instead of settling partially.
    let mut state = authority();
    let mut context = harness_context(&mut state);
    let hauler = companion_id(4);
    let (hauler_key, _) = motion_scene(&mut context, hauler, companion_id(5));
    let chest = BlockPos::new(2, 1, 2);
    context.preload_block(observation(chest, CHEST));
    let mut full = InventoryRecord::empty();
    full.slots[0] = tool(ITEM_STONE_PICKAXE, 100);
    for slot in full.slots.iter_mut().skip(1) {
        *slot = ItemStack {
            item: ITEM_DIRT,
            count: 64,
            durability: 0,
        };
    }
    context.preload_inventory(hauler_key, full);
    context.preload_container(chest_record(
        chest,
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
    context.preload_companion_action(envelope(
        hauler,
        49,
        CompanionAction::MineHold { target: chest },
    ));
    let haul_call = RuleCall {
        phase: RulePhase::MiningStep,
        actor: Some(hauler_key),
        command: None,
        internal: None,
    };
    for _ in 1..15u32 {
        mine_provider::run(&mut context, haul_call).expect("progress");
    }
    assert!(
        mine_provider::run(&mut context, haul_call).is_err(),
        "full container output refuses"
    );
    let saturated = context
        .read()
        .mining(hauler_key)
        .expect("saturated progress")
        .clone();
    assert_eq!(saturated.elapsed, saturated.required);
    assert_eq!(
        context
            .read()
            .observation(Dimension::OVERWORLD, chest)
            .expect("cell")
            .block,
        CHEST,
        "refused container mining keeps its block"
    );
    assert_eq!(
        context
            .read()
            .inventory(hauler_key)
            .expect("inventory")
            .slots[0],
        tool(ITEM_STONE_PICKAXE, 100),
        "refused container mining never wears the tool"
    );
    provider::run(&mut context, intent_call()).expect("intent");
    provider::run(&mut context, motion_call()).expect("motion");
    provider::run(&mut context, placement_call()).expect("placement");
    assert_eq!(
        context.read().mining(hauler_key).expect("progress"),
        &saturated,
        "saturated progress survives the action phases"
    );
    assert_eq!(
        context
            .read()
            .observation(Dimension::OVERWORLD, chest)
            .expect("cell")
            .block,
        CHEST
    );
}
