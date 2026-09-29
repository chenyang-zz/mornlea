//! Authority replay for farming tools and buckets.

use mornlea_domain::{
    BlockPos, ChunkPos, Command, CommandEnvelope, CommandEnvelopeParts, Dimension, Event,
    EventRecipient, FiniteVec3, HotbarSlot, LookAngles, MotionState, MotionStateParts,
    SurvivalState, SurvivalStateParts, Weather,
};
use mornlea_protocol::{LoginStart, admit_login};
use mornlea_server::contracts::*;
use mornlea_server::rules::tools as provider;
use mornlea_server::state::{ActionKind, AuthorityState, TickContext};
use mornlea_storage::{ItemStack, PlayerLocation, PlayerSave};

const AIR: u16 = 0;
const STONE: u16 = 2;
const DIRT: u16 = 3;
const GRASS: u16 = 4;
const WATER_SOURCE: u16 = 27;
const WATER_FLOWING: u16 = 28;
const FARMLAND_DRY: u16 = 35;
const WHEAT_STAGE_3: u16 = 40;
const WHEAT_STAGE_7: u16 = 44;
const POTATO_STAGE_6: u16 = 52;
const CARROT_STAGE_0: u16 = 54;
const SAPLING: u16 = 89;
const STONE_HOE: u16 = 30;
const IRON_HOE: u16 = 31;
const BROKEN_STONE_HOE: u16 = 32;
const BROKEN_IRON_HOE: u16 = 33;
const BONE_MEAL: u16 = 39;
const EMPTY_BUCKET: u16 = 55;
const WATER_BUCKET: u16 = 56;

fn stack(item: u16, count: u8, durability: u16) -> ItemStack {
    ItemStack {
        item,
        count,
        durability,
    }
}

fn authority() -> AuthorityState {
    AuthorityState::try_new(
        ServerLimits::try_new(8, 4096, 512, 64, 64, 1_048_576).unwrap(),
        7,
    )
    .unwrap()
}

fn admitted() -> mornlea_protocol::AdmittedLogin {
    admitted_with_tag(1)
}

fn admitted_with_tag(tag: u8) -> mornlea_protocol::AdmittedLogin {
    let mut id = [0u8; 16];
    id[0] = tag;
    id[6] = 0x40;
    id[8] = 0x80;
    let start = LoginStart::new(
        mornlea_domain::PlayerId::try_from_bytes(id).unwrap(),
        "Tester",
        8,
    )
    .unwrap();
    admit_login(LoginStart::decode_inbound(&start.encode().unwrap()).unwrap()).unwrap()
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

fn actor(session: SessionKey) -> ActorRecord {
    let position = [0.5, 64.0, 0.5];
    let mut id = [0u8; 16];
    id[0] = 1;
    id[6] = 0x40;
    id[8] = 0x80;
    ActorRecord::try_new(
        ActorKey::Player(session),
        ActorLifecycle::Active,
        Dimension::OVERWORLD,
        MotionState::new(MotionStateParts {
            position: FiniteVec3::try_new(position).unwrap(),
            velocity: FiniteVec3::try_new([0.0; 3]).unwrap(),
            on_ground: true,
        }),
        LookAngles::try_new(std::f32::consts::PI, 0.0).unwrap(),
        SurvivalState::try_new(SurvivalStateParts {
            health: 20,
            oxygen: 300,
            hunger: 20,
            saturation_zero: false,
            armor_points: 0,
        })
        .unwrap(),
        ActorBody::Player(PlayerSave {
            player_id: mornlea_storage::PlayerId::from_bytes(id),
            revision: 1,
            display_name: "Tester".to_owned(),
            current: PlayerLocation {
                dimension: 0,
                position,
            },
            yaw: std::f32::consts::PI,
            pitch: 0.0,
            safe: None,
            inventory: mornlea_storage::Inventory::default(),
            health: 20,
            hunger: 20,
            saturation_milli: 5_000,
            exhaustion_milli: 0,
            respawn_present: false,
            respawn_position: [0.0; 3],
            respawn_dimension: 0,
            armor: [ItemStack::default(); 4],
        }),
    )
    .unwrap()
}

fn observed(pos: BlockPos, block: u16) -> BlockObservation {
    BlockObservation::try_new(
        ChunkKey {
            dimension: Dimension::OVERWORLD,
            pos: ChunkPos::new(pos.x() >> 4, pos.z() >> 4),
        },
        1,
        1,
        pos,
        block,
    )
    .unwrap()
}

fn scene(
    context: &mut TickContext<'_>,
    session: SessionKey,
    held: ItemStack,
    cells: &[(BlockPos, u16)],
) -> ActorKey {
    let player = ActorKey::Player(session);
    context
        .stage(RuleEffect::Environment(environment()))
        .unwrap();
    context.stage(RuleEffect::Actor(actor(session))).unwrap();
    let mut inventory = InventoryRecord::empty();
    inventory.slots[0] = held;
    context.preload_inventory(player, inventory);
    for (pos, block) in cells {
        context.preload_block(observed(*pos, *block));
    }
    player
}

fn cell(context: &TickContext<'_>, pos: BlockPos) -> BlockObservation {
    context
        .read()
        .observation(Dimension::OVERWORLD, pos)
        .unwrap()
}

fn held(context: &TickContext<'_>, player: ActorKey) -> ItemStack {
    context.read().inventory(player).unwrap().slots[0]
}

fn command(session: SessionKey, sequence: u64, action: Command) -> CommandEnvelope {
    CommandEnvelope::try_new(CommandEnvelopeParts {
        tick: 0,
        session: session.get(),
        sequence,
        arrival_index: 0,
        command: action,
    })
    .unwrap()
}

fn call(command: &CommandEnvelope) -> RuleCall<'_> {
    RuleCall {
        phase: RulePhase::Interaction,
        actor: None,
        command: Some(command),
        internal: None,
    }
}

fn look() -> LookAngles {
    LookAngles::try_new(std::f32::consts::PI, 0.0).unwrap()
}

fn ray_cells(hit: u16, adjacent: u16) -> [(BlockPos, u16); 3] {
    [
        (BlockPos::new(0, 65, 0), AIR),
        (BlockPos::new(0, 65, 1), adjacent),
        (BlockPos::new(0, 65, 2), hit),
    ]
}

#[derive(Clone, Debug, PartialEq)]
struct Probe {
    cells: Vec<BlockObservation>,
    inventory: InventoryRecord,
    events: Vec<mornlea_domain::RoutedEvent>,
}

fn probe(context: &TickContext<'_>, player: ActorKey, cells: &[BlockPos]) -> Probe {
    Probe {
        cells: cells.iter().map(|pos| cell(context, *pos)).collect(),
        inventory: *context.read().inventory(player).unwrap(),
        events: context.events().to_vec(),
    }
}

#[test]
fn hoe_last_point() {
    let target = BlockPos::new(0, 65, 2);
    let above = BlockPos::new(0, 66, 2);
    for (ground, hoe, broken) in [
        (DIRT, STONE_HOE, BROKEN_STONE_HOE),
        (GRASS, IRON_HOE, BROKEN_IRON_HOE),
    ] {
        let mut state = authority();
        let session = state.admit(admitted(), TransportKind::Memory).unwrap();
        let mut context = TickContext::harness(&mut state, TickBudget::full());
        let mut cells = ray_cells(ground, AIR).to_vec();
        cells.push((above, AIR));
        let player = scene(&mut context, session, stack(hoe, 1, 1), &cells);
        let intent = command(session, 1, Command::TillSoil(look()));
        assert_eq!(
            provider::run(&mut context, call(&intent)).unwrap().applied,
            1
        );
        assert_eq!(cell(&context, target).block, FARMLAND_DRY);
        assert_eq!(cell(&context, target).revision, 2);
        assert_eq!(held(&context, player), stack(broken, 1, 0));
        assert!(!context.mining_suppressed(player));
        assert_eq!(context.take_charges(), vec![(player, ActionKind::Till)]);
        assert!(context.events().is_empty());
    }
}

#[test]
fn bone_meal_one_stage() {
    let target = BlockPos::new(0, 65, 2);
    for (initial, next) in [
        (WHEAT_STAGE_3, WHEAT_STAGE_3 + 1),
        (POTATO_STAGE_6, POTATO_STAGE_6 + 1),
        (CARROT_STAGE_0, CARROT_STAGE_0 + 1),
    ] {
        let mut state = authority();
        let session = state.admit(admitted(), TransportKind::Memory).unwrap();
        let mut context = TickContext::harness(&mut state, TickBudget::full());
        let player = scene(
            &mut context,
            session,
            stack(BONE_MEAL, 2, 0),
            &ray_cells(initial, AIR),
        );
        let intent = command(session, 2, Command::BoneMeal(look()));
        assert_eq!(
            provider::run(&mut context, call(&intent)).unwrap().applied,
            1
        );
        assert_eq!(cell(&context, target).block, next);
        assert_eq!(held(&context, player), stack(BONE_MEAL, 1, 0));
        assert!(!context.mining_suppressed(player));
        assert!(context.events().is_empty());
    }
    for initial in [WHEAT_STAGE_7, SAPLING] {
        let mut state = authority();
        let session = state.admit(admitted(), TransportKind::Memory).unwrap();
        let mut context = TickContext::harness(&mut state, TickBudget::full());
        let player = scene(
            &mut context,
            session,
            stack(BONE_MEAL, 2, 0),
            &ray_cells(initial, AIR),
        );
        let before = probe(&context, player, &[target]);
        let intent = command(session, 3, Command::BoneMeal(look()));
        assert!(provider::run(&mut context, call(&intent)).is_err());
        assert_eq!(probe(&context, player, &[target]), before);
    }
}

#[test]
fn bucket_source_and_flowing() {
    let target = BlockPos::new(0, 65, 2);
    let adjacent = BlockPos::new(0, 65, 1);
    let mut state = authority();
    let session = state.admit(admitted(), TransportKind::Memory).unwrap();
    let mut context = TickContext::harness(&mut state, TickBudget::full());
    let player = scene(
        &mut context,
        session,
        stack(EMPTY_BUCKET, 1, 0),
        &ray_cells(WATER_SOURCE, WATER_FLOWING),
    );
    let collect = command(session, 4, Command::CollectWater(look()));
    assert_eq!(
        provider::run(&mut context, call(&collect)).unwrap().applied,
        1
    );
    assert_eq!(cell(&context, target).block, AIR);
    assert_eq!(cell(&context, adjacent).block, WATER_FLOWING);
    assert_eq!(held(&context, player), stack(WATER_BUCKET, 1, 0));
    assert!(context.mining_suppressed(player));
    assert_eq!(context.events().len(), 1);
    assert_eq!(
        context.events()[0].recipient(),
        EventRecipient::Session(session.get())
    );
    assert!(
        matches!(context.events()[0].event(), Event::PlaceBlockSucceeded(success) if success.sequence() == 4)
    );
    drop(context);
    let next_tick = TickContext::harness(&mut state, TickBudget::full());
    assert!(!next_tick.mining_suppressed(player));

    let mut state = authority();
    let session = state.admit(admitted(), TransportKind::Memory).unwrap();
    let mut context = TickContext::harness(&mut state, TickBudget::full());
    let player = scene(
        &mut context,
        session,
        stack(WATER_BUCKET, 1, 0),
        &ray_cells(STONE, WATER_FLOWING),
    );
    let place = command(session, 5, Command::PlaceWater(look()));
    assert_eq!(
        provider::run(&mut context, call(&place)).unwrap().applied,
        1
    );
    assert_eq!(cell(&context, adjacent).block, WATER_SOURCE);
    assert_eq!(held(&context, player), stack(EMPTY_BUCKET, 1, 0));
    assert!(context.mining_suppressed(player));
    assert_eq!(context.events().len(), 1);
    assert!(
        matches!(context.events()[0].event(), Event::PlaceBlockSucceeded(success) if success.sequence() == 5)
    );

    let mut state = authority();
    let session = state.admit(admitted(), TransportKind::Memory).unwrap();
    let mut context = TickContext::harness(&mut state, TickBudget::full());
    let player = scene(
        &mut context,
        session,
        stack(WATER_BUCKET, 1, 0),
        &ray_cells(STONE, AIR),
    );
    let place = command(session, 6, Command::PlaceWater(look()));
    assert_eq!(
        provider::run(&mut context, call(&place)).unwrap().applied,
        1
    );
    assert_eq!(cell(&context, adjacent).block, WATER_SOURCE);
    assert_eq!(held(&context, player), stack(EMPTY_BUCKET, 1, 0));
    assert!(context.mining_suppressed(player));
    assert_eq!(context.events().len(), 1);
    assert!(
        matches!(context.events()[0].event(), Event::PlaceBlockSucceeded(success) if success.sequence() == 6)
    );
}

#[test]
fn bucket_failure_conservation() {
    let hit = BlockPos::new(0, 65, 2);
    let adjacent = BlockPos::new(0, 65, 1);
    for (action, held_item, hit_block, adjacent_block) in [
        (Command::CollectWater(look()), EMPTY_BUCKET, STONE, AIR),
        (
            Command::CollectWater(look()),
            WATER_BUCKET,
            WATER_SOURCE,
            AIR,
        ),
        (
            Command::PlaceWater(look()),
            WATER_BUCKET,
            STONE,
            WATER_SOURCE,
        ),
        (Command::PlaceWater(look()), EMPTY_BUCKET, STONE, AIR),
    ] {
        let mut state = authority();
        let session = state.admit(admitted(), TransportKind::Memory).unwrap();
        let mut context = TickContext::harness(&mut state, TickBudget::full());
        let player = scene(
            &mut context,
            session,
            stack(held_item, 1, 0),
            &ray_cells(hit_block, adjacent_block),
        );
        let before = probe(&context, player, &[hit, adjacent]);
        let intent = command(session, 6, action);
        assert!(
            provider::run(&mut context, call(&intent)).is_err(),
            "refused action: {action:?}"
        );
        assert_eq!(probe(&context, player, &[hit, adjacent]), before);
        assert!(!context.mining_suppressed(player));
    }
    let mut state = authority();
    let session = state.admit(admitted(), TransportKind::Memory).unwrap();
    let mut context = TickContext::harness(&mut state, TickBudget::full());
    let player = scene(
        &mut context,
        session,
        stack(WATER_BUCKET, 1, 0),
        &[(BlockPos::new(0, 65, 0), AIR), (hit, STONE)],
    );
    let before = probe(&context, player, &[hit]);
    let intent = command(session, 7, Command::PlaceWater(look()));
    assert!(provider::run(&mut context, call(&intent)).is_err());
    assert_eq!(probe(&context, player, &[hit]), before);
    assert!(!context.mining_suppressed(player));

    let mut state = authority();
    let session = state.admit(admitted(), TransportKind::Memory).unwrap();
    let mut context = TickContext::harness(&mut state, TickBudget::full());
    let player = scene(
        &mut context,
        session,
        stack(EMPTY_BUCKET, 1, 0),
        &ray_cells(WATER_SOURCE, AIR),
    );
    context.charge(WorkKind::Effects, 4096).unwrap();
    let before = probe(&context, player, &[hit, adjacent]);
    let intent = command(session, 8, Command::CollectWater(look()));
    assert!(provider::run(&mut context, call(&intent)).is_err());
    assert_eq!(probe(&context, player, &[hit, adjacent]), before);
}

#[test]
fn bucket_full_suppression_capacity_preserves_state() {
    let hit = BlockPos::new(0, 65, 2);
    let adjacent = BlockPos::new(0, 65, 1);
    let mut state = authority();
    let previous: Vec<_> = (1..=8)
        .map(|tag| {
            state
                .admit(admitted_with_tag(tag), TransportKind::Memory)
                .unwrap()
        })
        .collect();
    state
        .close_session(previous[0], CloseReason::PeerGone)
        .unwrap();
    let session = state
        .admit(admitted_with_tag(9), TransportKind::Memory)
        .unwrap();
    let mut context = TickContext::harness(&mut state, TickBudget::full());
    let player = scene(
        &mut context,
        session,
        stack(EMPTY_BUCKET, 1, 0),
        &ray_cells(WATER_SOURCE, AIR),
    );
    for old in previous {
        context.suppress_mining(ActorKey::Player(old)).unwrap();
    }
    let before = probe(&context, player, &[hit, adjacent]);
    let intent = command(session, 11, Command::CollectWater(look()));
    assert!(provider::run(&mut context, call(&intent)).is_err());
    assert_eq!(probe(&context, player, &[hit, adjacent]), before);
    assert!(!context.mining_suppressed(player));
}

#[test]
fn readiness_and_selected_tool_refusal() {
    let hit = BlockPos::new(0, 65, 2);
    let above = BlockPos::new(0, 66, 2);
    for (above_block, held_item) in [
        (Some(STONE), STONE_HOE),
        (None, STONE_HOE),
        (Some(AIR), BROKEN_STONE_HOE),
    ] {
        let mut state = authority();
        let session = state.admit(admitted(), TransportKind::Memory).unwrap();
        let mut context = TickContext::harness(&mut state, TickBudget::full());
        let mut cells = ray_cells(DIRT, AIR).to_vec();
        if let Some(block) = above_block {
            cells.push((above, block));
        }
        let durability = if held_item == BROKEN_STONE_HOE { 0 } else { 1 };
        let player = scene(
            &mut context,
            session,
            stack(held_item, 1, durability),
            &cells,
        );
        let before = probe(&context, player, &[hit]);
        let intent = command(session, 8, Command::TillSoil(look()));
        assert!(provider::run(&mut context, call(&intent)).is_err());
        assert_eq!(probe(&context, player, &[hit]), before);
        assert!(context.take_charges().is_empty());
    }

    let mut state = authority();
    let session = state.admit(admitted(), TransportKind::Memory).unwrap();
    let mut context = TickContext::harness(&mut state, TickBudget::full());
    let player = scene(
        &mut context,
        session,
        stack(BONE_MEAL, 2, 0),
        &ray_cells(WHEAT_STAGE_3, AIR),
    );
    let inventory = *context.read().inventory(player).unwrap();
    context.preload_inventory(player, inventory.with_selected(HotbarSlot::new(1).unwrap()));
    let before = probe(&context, player, &[hit]);
    let intent = command(session, 9, Command::BoneMeal(look()));
    assert!(provider::run(&mut context, call(&intent)).is_err());
    assert_eq!(probe(&context, player, &[hit]), before);
}

#[test]
fn hoe_full_charge_capacity_preserves_state() {
    let target = BlockPos::new(0, 65, 2);
    let above = BlockPos::new(0, 66, 2);
    let mut state = authority();
    let session = state.admit(admitted(), TransportKind::Memory).unwrap();
    let mut context = TickContext::harness(&mut state, TickBudget::full());
    let mut cells = ray_cells(DIRT, AIR).to_vec();
    cells.push((above, AIR));
    let player = scene(&mut context, session, stack(STONE_HOE, 1, 1), &cells);
    for _ in 0..4096 {
        context.note_charge(player, ActionKind::Mining).unwrap();
    }
    let before = probe(&context, player, &[target]);
    let intent = command(session, 10, Command::TillSoil(look()));
    assert!(provider::run(&mut context, call(&intent)).is_err());
    assert_eq!(probe(&context, player, &[target]), before);
    assert_eq!(context.take_charges().len(), 4096);
}

#[test]
fn foreign_actor_call_refuses_without_effect() {
    let source = BlockPos::new(0, 65, 2);
    let mut state = authority();
    let session = state.admit(admitted(), TransportKind::Memory).unwrap();
    let foreign = state
        .admit(admitted_with_tag(2), TransportKind::Memory)
        .unwrap();
    let mut context = TickContext::harness(&mut state, TickBudget::full());
    let player = scene(
        &mut context,
        session,
        stack(EMPTY_BUCKET, 1, 0),
        &ray_cells(WATER_SOURCE, AIR),
    );
    context.note_charge(player, ActionKind::Mining).unwrap();
    let before = probe(&context, player, &[source]);
    let before_world = context.read().world();
    let intent = command(session, 12, Command::CollectWater(look()));
    let mut forged = call(&intent);
    forged.actor = Some(ActorKey::Player(foreign));
    assert!(matches!(
        provider::run(&mut context, forged),
        Err(ServerError::InvalidInput { .. })
    ));
    assert_eq!(probe(&context, player, &[source]), before);
    assert_eq!(context.read().world(), before_world);
    assert_eq!(context.take_charges(), vec![(player, ActionKind::Mining)]);
    assert!(!context.mining_suppressed(player));
}
