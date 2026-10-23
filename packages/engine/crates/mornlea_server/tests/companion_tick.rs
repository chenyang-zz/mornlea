//! Whole-tick companion task wiring, driven only through the public
//! single-tick function `ServerTick::step`.
//!
//! Every case runs a real authority: real chunks and physics, the real
//! task runner and path search, the real `advance_tick` publication and
//! the player outbox. Only the Agent is a fake that answers plan requests
//! with a fixed plan. No case writes world time, moves the companion
//! body, or builds a companion action envelope; the runner's actions
//! reach the tick only through the core fence the step calls.
//!
//! Go references are cited per case (`packages/server/server/...`).

use std::collections::BTreeMap;
use std::time::Instant;

use mornlea_domain::{
    BlockPos, ChatBody, ChatIntent, ChunkPos, Command, CommandText, CompanionId, CompanionName,
    Dimension, Event, EventRecipient, FiniteVec3, HeldActions, LookAngles, MotionState,
    MotionStateParts, Movement, PlayerControl, PlayerControlParts, PlayerId, SurvivalState,
    SurvivalStateParts, TaskState, Weather,
};
use mornlea_protocol::{
    AdmittedLogin, LoginStart, PlayIntent, ProtocolCodec, ServerPacket, State, admit_login,
    read_frame_ref,
};
use mornlea_server::contracts::{
    ActorAux, ActorBody, ActorKey, ActorLifecycle, ActorRecord, ActorRuntime, AgentHandle,
    AgentPlan, AgentPoll, AgentRequest, AgentRequestId, AgentResponse, ChunkKey, ClientInstanceId,
    Clock, CompanionAction, Deadline, EnvironmentState, FrozenLease, InventoryRecord, LeaseId,
    NamespaceId, PlanRequest, PlanResponse, PlanStep, PlanningSnapshot, RuleEffect, RuleTunables,
    ServerError, ServerLimits, ServerPhase, SessionKey, SnapshotId, SnapshotPort,
    SnapshotRegistration, TickBudget, TransportKind,
};
use mornlea_server::core::companion_chat::CompanionChatPhase;
use mornlea_server::core::companion_runner::{
    FOLLOW_DISTANCE_BLOCKS, FOLLOW_REPLAN_DRIFT_BLOCKS, TICKS_PER_MINUTE, TaskTimeoutMinutes,
};
use mornlea_server::core::world::ReadyChunk;
use mornlea_server::runtime::companion_planning::CompanionPlanningPorts;
use mornlea_server::runtime::tick_loop::{ServerTick, TickReport, TickStartup};
use mornlea_server::state::{AuthorityState, TickContext};
use mornlea_storage::{
    COMPANION_TASK_COMPLETED, COMPANION_TASK_RUNNING, COMPANION_TASK_TIMED_OUT, ChestSlot, Chunk,
    CompanionBody, ContainerSnapshot, FurnaceSlot, Inventory, ItemStack, PlayerId as SavePlayerId,
    PlayerLocation, PlayerSave, StorageKind, StoredCompanionTask, StoredPlayer,
};

const GRASS: u16 = 4;
const ITEM_DIRT: u16 = 2;
const ITEM_STONE: u16 = 1;
/// Standing cell height above the one-block grass floor at y=64.
const STAND_Y: i32 = 65;
/// Every fixture companion starts here.
const COMPANION_START: [f32; 3] = [8.5, 65.0, 8.5];

// ---------------------------------------------------------------------
// Fakes: the Agent and its snapshot registry, nothing else.
// ---------------------------------------------------------------------

struct TestClock(Instant);

impl Clock for TestClock {
    fn monotonic(&self) -> Instant {
        self.0
    }
    fn unix_ms(&self) -> i64 {
        1_800_000_000_000
    }
}

#[derive(Default)]
struct FakeAgent {
    submitted: Vec<PlanRequest>,
    terminals: BTreeMap<AgentRequestId, AgentPoll>,
}

impl FakeAgent {
    /// Answers the newest plan request with the plan.
    fn answer(&mut self, plan: AgentPlan) {
        let request = self.submitted.last().expect("a plan request").clone();
        assert!(
            !self.terminals.contains_key(&request.leased.base.request_id),
            "the newest request is still unanswered"
        );
        self.terminals.insert(
            request.leased.base.request_id,
            AgentPoll::Completed(AgentResponse::Plan(PlanResponse {
                leased: request.leased.clone(),
                run_id: request.run_id,
                companion_id: request.companion_id,
                generation: request.generation,
                snapshot_id: request.snapshot_id,
                snapshot_digest: request.snapshot_digest,
                plan,
            })),
        );
    }
}

impl AgentHandle for FakeAgent {
    fn submit(&mut self, request: AgentRequest) -> Result<AgentRequestId, ServerError> {
        match request {
            AgentRequest::Plan(plan) => {
                let id = plan.leased.base.request_id;
                self.submitted.push(plan);
                Ok(id)
            }
            _ => Err(ServerError::InvalidInput {
                field: "fake_agent_request",
            }),
        }
    }
    fn poll(&mut self, id: AgentRequestId) -> AgentPoll {
        self.terminals
            .get(&id)
            .cloned()
            .unwrap_or(AgentPoll::Pending)
    }
    fn cancel(&mut self, _id: AgentRequestId, _deadline: Deadline) -> Result<(), ServerError> {
        Ok(())
    }
    fn freeze(&mut self, _clock: &dyn Clock) -> Option<FrozenLease> {
        None
    }
    fn release(&mut self, _lease: &FrozenLease, _deadline: Deadline) -> Result<(), ServerError> {
        Ok(())
    }
    fn close(&mut self, _deadline: Deadline) -> Result<(), ServerError> {
        Ok(())
    }
}

#[derive(Default)]
struct FakeSnapshots;

impl SnapshotPort for FakeSnapshots {
    fn register(
        &mut self,
        _namespace: NamespaceId,
        _companion: CompanionId,
        _generation: u64,
        _snapshot: PlanningSnapshot,
        _deadline: Deadline,
    ) -> Result<SnapshotRegistration, ServerError> {
        SnapshotRegistration::try_new(
            SnapshotId::try_from_bytes(uuid(90)).unwrap(),
            [3u8; 32],
            vec![0u8; 32],
            "http://127.0.0.1:1/mcp".to_owned(),
        )
    }
    fn complete(&mut self, _id: SnapshotId) -> Result<(), ServerError> {
        Ok(())
    }
    fn cancel(&mut self, _id: SnapshotId) -> Result<(), ServerError> {
        Ok(())
    }
    fn close(&mut self) -> Result<(), ServerError> {
        Ok(())
    }
}

// ---------------------------------------------------------------------
// World fixture.
// ---------------------------------------------------------------------

fn uuid(tag: u8) -> [u8; 16] {
    let mut bytes = [0u8; 16];
    bytes[0] = tag.max(1);
    bytes[6] = 0x40;
    bytes[8] = 0x80;
    bytes
}

fn amu() -> CompanionId {
    CompanionId::try_from_bytes(uuid(9)).unwrap()
}

fn amu_name() -> CompanionName {
    CompanionName::try_from_canonical("阿木".to_owned()).unwrap()
}

fn player_id(tag: u8) -> PlayerId {
    PlayerId::try_from_bytes(uuid(tag)).unwrap()
}

fn admitted(tag: u8, name: &str) -> AdmittedLogin {
    let start = LoginStart::new(player_id(tag), name, 8).unwrap();
    admit_login(LoginStart::decode_inbound(&start.encode().unwrap()).unwrap()).unwrap()
}

fn stored_player(tag: u8, name: &str, position: [f32; 3]) -> StoredPlayer {
    StoredPlayer {
        player_id: SavePlayerId::from_bytes(uuid(tag)),
        revision: 1,
        display_name: name.to_owned(),
        current: PlayerLocation {
            dimension: 0,
            position,
        },
        yaw: 0.0,
        pitch: 0.0,
        safe: None,
        inventory: Inventory::default(),
        health: 20,
        hunger: 20,
        saturation_milli: 5_000,
        exhaustion_milli: 0,
        respawn_present: false,
        respawn_position: [0.0; 3],
        respawn_dimension: 0,
        armor: [ItemStack::default(); 4],
        needs_rewrite: false,
    }
}

fn survival() -> SurvivalState {
    SurvivalState::try_new(SurvivalStateParts {
        health: 20,
        oxygen: 300,
        hunger: 20,
        saturation_zero: false,
        armor_points: 0,
    })
    .unwrap()
}

fn motion(position: [f32; 3]) -> MotionState {
    MotionState::new(MotionStateParts {
        position: FiniteVec3::try_new(position).unwrap(),
        velocity: FiniteVec3::try_new([0.0; 3]).unwrap(),
        on_ground: true,
    })
}

fn companion_actor(id: CompanionId, position: [f32; 3]) -> ActorRecord {
    ActorRecord::try_new(
        ActorKey::Companion(id),
        ActorLifecycle::Active,
        Dimension::OVERWORLD,
        motion(position),
        LookAngles::try_new(0.0, 0.0).unwrap(),
        survival(),
        ActorBody::Companion(CompanionBody {
            id: SavePlayerId::from_bytes(id.bytes()),
            dimension: 0,
            position,
            yaw: 0.0,
            pitch: 0.0,
            inventory: Inventory::default(),
        }),
    )
    .unwrap()
}

fn companion_runtime(id: CompanionId) -> ActorRuntime {
    ActorRuntime {
        key: ActorKey::Companion(id),
        controls: None,
        has_view: false,
        reset: false,
        attack_cooldown: 0,
        hurt_cooldown: 0,
        burn_cooldown: 0,
        oxygen: 300,
        peak_y: 65.0,
        exhaustion_milli: 0,
        saturation_milli: 0,
        since_damage_ticks: 0,
        drown_ticks: 0,
        starvation_ticks: 0,
        eating: None,
        bow: None,
        path: None,
        aux: ActorAux::Companion {
            generation: 0,
            attempt: 1,
            task: StoredCompanionTask::default(),
            mining_target: None,
        },
    }
}

/// The companion's starting inventory: a few stacks, so "unchanged" is a
/// real comparison.
fn companion_inventory() -> InventoryRecord {
    let mut record = InventoryRecord::empty();
    record.slots[0] = ItemStack {
        item: ITEM_DIRT,
        count: 12,
        durability: 0,
    };
    record.slots[3] = ItemStack {
        item: ITEM_STONE,
        count: 5,
        durability: 0,
    };
    record
}

fn air_chunk() -> Chunk {
    Chunk {
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
        furnaces: vec![FurnaceSlot::default(); 32],
        chests: vec![ChestSlot::default(); 16],
    }
}

/// A walkable chunk: one grass layer at y=64 across the whole column.
fn ground_chunk() -> Chunk {
    let mut chunk = air_chunk();
    for x in 0..16 {
        for z in 0..16 {
            let index = mornlea_domain::chunk_block_index(BlockPos::new(x, 64, z)) as usize;
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
            section.packed[(index % 4096) / 4] |= u64::from(GRASS) << ((index % 4) * 15);
        }
    }
    chunk
}

fn stage(state: &mut AuthorityState, stage: impl FnOnce(&mut TickContext<'_>)) {
    let mut context = TickContext::restage(state, TickBudget::full());
    stage(&mut context);
    let residents = context.resident_snapshot();
    drop(context);
    state.commit_residents(residents);
}

fn environment() -> EnvironmentState {
    EnvironmentState {
        seed: 7,
        next_tick: 0,
        world_time: 1_000,
        day_phase_offset: 0,
        season_offset: 0,
        weather: Weather::Clear,
        weather_remaining: 0,
        difficulty: 0,
        tunables: RuleTunables::source_defaults(),
    }
}

fn login(state: &mut AuthorityState, tag: u8, name: &str, position: [f32; 3]) -> SessionKey {
    let session = state
        .prepare(admitted(tag, name), TransportKind::Memory)
        .unwrap();
    state
        .install(session, Some(stored_player(tag, name, position)))
        .unwrap();
    state.activate(session).unwrap();
    session
}

/// One companion task world: a flat ground window, the configured
/// companion with its runtime and inventory, an issuing player, and the
/// tick assembly with its startup-frozen timeout.
struct World {
    state: AuthorityState,
    tick: ServerTick,
    agent: FakeAgent,
    snapshots: FakeSnapshots,
    clock: TestClock,
    mint: u64,
    issuer: SessionKey,
    sequences: BTreeMap<SessionKey, u64>,
    reports: Vec<TickReport>,
    /// Frames each connected client has read off its outbox, as the
    /// transport drains outboxes every tick.
    received: BTreeMap<SessionKey, Vec<Vec<u8>>>,
}

impl World {
    fn new(timeout_minutes: u32) -> Self {
        Self::with_startup(TickStartup::new(
            TaskTimeoutMinutes::try_new(timeout_minutes).unwrap(),
            TickBudget::full(),
        ))
    }

    fn with_startup(startup: TickStartup) -> Self {
        let mut state = AuthorityState::try_new(
            ServerLimits::try_new(8, 4096, 512, 64, 64, 1_048_576)
                .unwrap()
                .with_view_radius(2),
            7,
        )
        .unwrap();
        stage(&mut state, |context| {
            context
                .stage(RuleEffect::Environment(environment()))
                .unwrap();
            for x in -1..=3 {
                for z in -1..=1 {
                    let key = ChunkKey {
                        dimension: Dimension::OVERWORLD,
                        pos: ChunkPos::new(x, z),
                    };
                    context.preload_ready_chunk(
                        ReadyChunk::try_new(key, 1, 1, ground_chunk()).unwrap(),
                    );
                }
            }
        });
        state
            .configure_companion_chat(&[(amu(), amu_name())])
            .unwrap();
        let issuer = login(&mut state, 1, "Ada", [0.5, 65.0, 0.5]);
        stage(&mut state, |context| {
            context
                .stage(RuleEffect::Actor(companion_actor(amu(), COMPANION_START)))
                .unwrap();
            context
                .stage(RuleEffect::Runtime(companion_runtime(amu())))
                .unwrap();
            context.preload_inventory(ActorKey::Companion(amu()), companion_inventory());
        });
        Self {
            state,
            tick: ServerTick::new(startup, vec![amu()]),
            agent: FakeAgent::default(),
            snapshots: FakeSnapshots,
            clock: TestClock(Instant::now()),
            mint: 100,
            issuer,
            sequences: BTreeMap::new(),
            reports: Vec::new(),
            received: BTreeMap::from([(issuer, Vec::new())]),
        }
    }

    /// Logs one more player in; its client drains its outbox every tick.
    fn login(&mut self, tag: u8, name: &str, position: [f32; 3]) -> SessionKey {
        let session = login(&mut self.state, tag, name, position);
        self.received.insert(session, Vec::new());
        session
    }

    /// One production tick through the public single-tick function.
    fn try_step(&mut self) -> Result<&TickReport, ServerError> {
        let mint_counter = &mut self.mint;
        let mut mint = || {
            *mint_counter += 1;
            let mut bytes = uuid(0x40);
            bytes[9..16].copy_from_slice(&mint_counter.to_be_bytes()[1..]);
            Ok(bytes)
        };
        let ports = CompanionPlanningPorts {
            agent: &mut self.agent,
            snapshots: &mut self.snapshots,
            clock: &self.clock,
            client: ClientInstanceId::try_from_bytes(uuid(2)).unwrap(),
            namespace: NamespaceId::try_from_bytes(uuid(3)).unwrap(),
            lease: Some((LeaseId::try_from_bytes(uuid(30)).unwrap(), 9)),
            mint_id: &mut mint,
        };
        let report = self.tick.step(&mut self.state, ports)?;
        self.reports.push(report);
        for (session, frames) in &mut self.received {
            frames.extend(self.state.take_outbox(*session, 512, 8 << 20).unwrap());
        }
        Ok(self.reports.last().unwrap())
    }

    fn step(&mut self) -> &TickReport {
        self.try_step().expect("a healthy tick")
    }

    fn submit_chat(&mut self, session: SessionKey, text: &str) {
        self.state
            .submit(
                session,
                PlayIntent::Chat(
                    ChatIntent::try_new(CommandText::try_from_canonical(text.to_owned()).unwrap())
                        .unwrap(),
                ),
            )
            .unwrap();
    }

    /// Holds a player's movement input, the way a client sends it.
    fn walk(&mut self, session: SessionKey, move_x: i8, move_z: i8) {
        let sequence = self.sequences.entry(session).or_insert(0);
        *sequence += 1;
        let command = Command::PlayerInput(PlayerControl::new(PlayerControlParts {
            movement: Movement {
                move_x,
                move_z,
                jump: false,
            },
            look: LookAngles::try_new(0.0, 0.0).unwrap(),
            actions: HeldActions {
                primary: false,
                eating: false,
                sprinting: false,
                sneaking: false,
            },
        }));
        self.state
            .submit(
                session,
                PlayIntent::Sequenced {
                    sequence: *sequence,
                    command,
                },
            )
            .unwrap();
    }

    /// Issues a command and installs the Agent's plan for it: the chat
    /// is admitted and dispatched on one tick, the answer installs on
    /// the next. Returns the install tick's report.
    fn install(&mut self, plan: AgentPlan) -> &TickReport {
        let issuer = self.issuer;
        self.submit_chat(issuer, "@阿木 走到那边");
        let dispatched = self.step();
        assert_eq!(dispatched.planning.dispatched, vec![amu()]);
        self.agent.answer(plan);
        let installed = self.step();
        assert_eq!(installed.planning.installed, vec![amu()]);
        self.reports.last().unwrap()
    }

    fn body(&self) -> ActorRecord {
        self.state
            .settled_read()
            .unwrap()
            .actor(ActorKey::Companion(amu()))
            .cloned()
            .expect("companion body")
    }

    fn position(&self) -> [f32; 3] {
        self.body().motion.position().get()
    }

    fn player_position(&self, session: SessionKey) -> [f32; 3] {
        self.state
            .settled_read()
            .unwrap()
            .actor(ActorKey::Player(session))
            .expect("player body")
            .motion
            .position()
            .get()
    }

    fn inventory(&self) -> InventoryRecord {
        *self
            .state
            .settled_read()
            .unwrap()
            .inventory(ActorKey::Companion(amu()))
            .expect("companion inventory")
    }

    fn stored(&self) -> StoredCompanionTask {
        match &self
            .state
            .residents()
            .runtimes
            .get(&ActorKey::Companion(amu()))
            .expect("companion runtime")
            .aux
        {
            ActorAux::Companion { task, .. } => task.clone(),
            _ => panic!("companion runtime aux"),
        }
    }

    fn phase(&self) -> Option<CompanionChatPhase> {
        self.state
            .companion_chat_queue(amu())
            .unwrap()
            .current
            .map(|current| current.phase)
    }

    fn world_time(&self) -> u64 {
        self.state.settled_read().unwrap().world_time()
    }

    /// Drains and decodes one session's outbox.
    fn outbox(&mut self, session: SessionKey) -> Vec<ServerPacket> {
        std::mem::take(self.received.get_mut(&session).expect("a connected client"))
            .iter()
            .map(|frame| {
                let parsed = read_frame_ref(frame).unwrap();
                ProtocolCodec::new()
                    .unwrap()
                    .decode_server(State::Play, parsed.packet_id, parsed.payload)
                    .unwrap()
            })
            .collect()
    }
}

/// The broadcast companion task states one tick published, in order.
fn task_states(report: &TickReport) -> Vec<TaskState> {
    report
        .publication
        .events
        .iter()
        .filter(|event| event.recipient() == EventRecipient::Broadcast)
        .filter_map(|event| match event.event() {
            Event::Chat(chat) => match chat.body() {
                ChatBody::Task {
                    companion, state, ..
                } if companion.id() == amu() => Some(*state),
                _ => None,
            },
            _ => None,
        })
        .collect()
}

/// The runner Move actions one tick submitted for the companion.
fn moves(report: &TickReport) -> usize {
    report
        .planning
        .actions
        .iter()
        .filter(|(id, action)| *id == amu() && matches!(action, CompanionAction::Move { .. }))
        .count()
}

fn go_to(x: i32, z: i32) -> PlanStep {
    PlanStep::GoTo { x, y: STAND_Y, z }
}

fn plan(steps: Vec<PlanStep>) -> AgentPlan {
    AgentPlan::try_new("走".to_owned(), steps).unwrap()
}

fn horizontal(a: [f32; 3], b: [f32; 3]) -> f32 {
    ((a[0] - b[0]).powi(2) + (a[2] - b[2]).powi(2)).sqrt()
}

fn standing_cell(position: [f32; 3]) -> BlockPos {
    BlockPos::new(
        position[0].floor() as i32,
        position[1].floor() as i32,
        position[2].floor() as i32,
    )
}

// ---------------------------------------------------------------------
// Whole-tick cases
// ---------------------------------------------------------------------

/// Go `companion_manager_test.go:386` `TestCompanionManagerTaskLifecycleEvents`:
/// a two-step go_to runs from install to its terminal state with the
/// task events Started, Progress, Completed in that order, inside the
/// Go 400-tick bound, and no task event follows the terminal one.
#[test]
fn install_runs_to_terminal_with_ordered_lifecycle_events() {
    let mut world = World::new(10);
    let start = world.position();
    let base_x = start[0].floor() as i32;
    let base_z = start[2].floor() as i32;
    world.install(plan(vec![
        go_to(base_x + 3, base_z),
        go_to(base_x + 6, base_z),
    ]));
    let mut states = task_states(world.reports.last().unwrap());
    for _ in 0..400 {
        if world.phase().is_none() {
            break;
        }
        let report = world.step();
        states.extend(task_states(report));
    }
    assert_eq!(
        states,
        vec![
            TaskState::Started,
            TaskState::Progress,
            TaskState::Completed
        ]
    );
    assert!(world.phase().is_none(), "the slot is free after completion");
    let stored = world.stored();
    assert_eq!(stored.state, COMPANION_TASK_COMPLETED);
    let offset = world.position()[0] - start[0];
    assert!(
        (5.0..=7.0).contains(&offset),
        "about six blocks travelled, got {offset}"
    );
    for _ in 0..3 {
        assert!(task_states(world.step()).is_empty(), "quiet after terminal");
    }
}

/// Go `companion_manager.go:829-860` `advancePathMovement`: a go_to
/// reaches its target cell through physics. The runner submits at most
/// one Move per tick, the body moves only because the authority tick
/// steps it (the position changes on many distinct ticks), and the step
/// completes on the tick the body stands on the goal cell.
#[test]
fn go_to_reaches_the_target_cell_through_physics() {
    let mut world = World::new(10);
    let goal = BlockPos::new(14, STAND_Y, 8);
    world.install(plan(vec![go_to(goal.x(), goal.z())]));
    let mut previous = world.position();
    let mut changed_ticks = 0;
    let mut completed_at = None;
    for _ in 0..400 {
        let report = world.step();
        assert!(moves(report) <= 1, "at most one Move per tick");
        let done = task_states(report).contains(&TaskState::Completed);
        let now = world.position();
        if now != previous {
            changed_ticks += 1;
        }
        previous = now;
        if done {
            completed_at = Some(now);
            break;
        }
    }
    let at = completed_at.expect("the go_to completes");
    assert_eq!(
        standing_cell(at),
        goal,
        "completion stands on the goal cell"
    );
    assert!(
        changed_ticks >= 3,
        "the body moved on {changed_ticks} ticks, not by a jump"
    );
}

/// Go `companion_follow_test.go:122`
/// `TestCompanionManagerFollowDistanceBoundaryAndResume` and `:262`
/// `TestCompanionManagerFollowExemptFromDeadline`: a follow keeps up with
/// a player who really walks away. The companion closes to within the
/// follow distance, holds there, re-paths while the target drifts by
/// more than the re-path threshold, and settles within the follow
/// distance again. The task stays Running with the zero deadline the
/// whole time. Distances use the ported Go constants, never literals.
#[test]
fn follow_keeps_up_with_a_walking_player() {
    let mut world = World::new(1);
    // Go places the target ten blocks out along +X, outside the follow
    // distance.
    let target_start = [COMPANION_START[0] + 10.0, 65.0, COMPANION_START[2]];
    let target = world.login(2, "Ben", target_start);
    world.step();
    let start = world.position();
    world.install(plan(vec![PlanStep::Follow {
        player_id: player_id(2),
    }]));
    assert_eq!(world.stored().deadline_ticks, 0, "follow is exempt");

    let settled = follow_settle(&mut world, target, 800);
    assert!(settled[0] > start[0], "moved toward the target");

    // The target walks back past the companion, Go's eight blocks beyond
    // it, with held movement input over many ticks.
    let goal_x = settled[0] - 8.0;
    let mut searches_while_walking = 0;
    let mut walking_ticks = 0;
    world.walk(target, -1, 0);
    while world.player_position(target)[0] > goal_x {
        let report = world.step();
        searches_while_walking += report.path_searches;
        walking_ticks += 1;
        assert!(walking_ticks < 400, "the target reaches its goal");
        assert_eq!(world.phase(), Some(CompanionChatPhase::Running));
    }
    world.walk(target, 0, 0);
    assert!(
        searches_while_walking >= 2,
        "the follow re-paths as the target drifts past {FOLLOW_REPLAN_DRIFT_BLOCKS} blocks"
    );
    let resumed = follow_settle(&mut world, target, 800);
    assert!(resumed[0] < settled[0], "followed the target back");
    assert_eq!(world.stored().deadline_ticks, 0);
    assert_eq!(world.stored().state, COMPANION_TASK_RUNNING);

    let issuer = world.issuer;
    world.submit_chat(issuer, "@阿木 停止");
    let report = world.step();
    assert_eq!(task_states(report), vec![TaskState::Stopped]);
    assert!(world.phase().is_none());
}

/// Go `companion_follow_test.go` `waitFollowSettle`: steps until the
/// companion is within the follow distance of the target and holds still
/// there for twenty ticks. Returns the settled position.
fn follow_settle(world: &mut World, target: SessionKey, bound: usize) -> [f32; 3] {
    for _ in 0..bound {
        let idle = moves(world.step()) == 0;
        assert_eq!(world.phase(), Some(CompanionChatPhase::Running));
        assert_eq!(world.stored().deadline_ticks, 0);
        let near =
            horizontal(world.position(), world.player_position(target)) <= FOLLOW_DISTANCE_BLOCKS;
        if idle && near {
            let before = world.position();
            for _ in 0..20 {
                let report = world.step();
                assert_eq!(moves(report), 0, "holds inside the follow distance");
            }
            let after = world.position();
            assert!(
                horizontal(before, after) <= 0.1,
                "held still: {before:?} -> {after:?}"
            );
            let distance = horizontal(after, world.player_position(target));
            assert!(
                distance <= FOLLOW_DISTANCE_BLOCKS,
                "settled {distance} blocks away, beyond {FOLLOW_DISTANCE_BLOCKS}"
            );
            return after;
        }
    }
    panic!("the follow did not settle in {bound} ticks");
}

/// Go `companion_publication.go:92-150`: the observing player's
/// outbox first gets one CompanionSpawn for the companion, then
/// CompanionStates whose position is the authoritative body position of
/// that tick, bit for bit, and changes from frame to frame while the
/// companion walks.
#[test]
fn player_outbox_sees_spawn_then_authoritative_states() {
    let mut world = World::new(10);
    let issuer = world.issuer;
    let mut sequence: Vec<&'static str> = Vec::new();
    let mut states = Vec::new();
    let mut collect = |world: &mut World, sequence: &mut Vec<&'static str>| {
        let body = world.position();
        let tick = world.reports.last().unwrap().publication.tick;
        for packet in world.outbox(issuer) {
            match packet {
                ServerPacket::CompanionSpawn(spawn) if spawn.companion_id == amu() => {
                    sequence.push("spawn");
                }
                ServerPacket::CompanionStates(batch) => {
                    for state in batch.states {
                        if state.companion_id == amu() {
                            assert_eq!(batch.tick, tick);
                            assert_eq!(
                                state.position.map(f32::to_bits),
                                body.map(f32::to_bits),
                                "the published position is the authoritative body"
                            );
                            sequence.push("states");
                            states.push(state.position);
                        }
                    }
                }
                _ => {}
            }
        }
    };
    world.install(plan(vec![go_to(14, 8)]));
    collect(&mut world, &mut sequence);
    for _ in 0..200 {
        let report = world.step();
        let done = task_states(report).contains(&TaskState::Completed);
        collect(&mut world, &mut sequence);
        if done {
            break;
        }
    }
    assert_eq!(sequence.first(), Some(&"spawn"));
    assert_eq!(
        sequence.iter().filter(|kind| **kind == "spawn").count(),
        1,
        "one spawn, then states"
    );
    assert!(sequence[1..].iter().all(|kind| *kind == "states"));
    let changes = states.windows(2).filter(|pair| pair[0] != pair[1]).count();
    assert!(changes >= 3, "published position changed {changes} times");
}

/// Go `companion_manager.go:1155` `dispatchPathRequests` and
/// `:707-756` `applyPathOutcomes`: a path search issued on the install
/// tick delivers no earlier than the next tick. The install tick issues
/// one search and no Move and the body stays put; the first Move comes
/// on the next tick, which that same tick's physics then applies.
#[test]
fn path_result_takes_effect_on_the_next_tick() {
    let mut world = World::new(10);
    let before = world.position();
    let installed = world.install(plan(vec![go_to(14, 8)]));
    assert_eq!(installed.path_searches, 1, "the install tick searches");
    assert_eq!(moves(installed), 0, "no Move on the install tick");
    assert_eq!(world.position(), before, "no movement on the install tick");
    let next = world.step();
    assert_eq!(moves(next), 1, "the first Move comes the next tick");
    assert_eq!(next.submission.admitted, vec![amu()]);
    assert_ne!(world.position(), before, "the next tick's physics moves it");
}

/// Go `companion_manager_test.go:720-765`
/// `TestCompanionManagerTaskTimesOutAtWorldTimeDeadline`: with a one
/// minute timeout a long go_to (Go's twenty fifteen-block legs, laid back
/// and forth here so the trip stays inside the loaded window) times out
/// on the world clock alone, after the real 1200 ticks. One tick before
/// the deadline the task still runs; the deadline tick publishes exactly
/// one TimedOut; afterwards the companion stops (Go's tolerance), stays
/// active and published, and its inventory is unchanged.
#[test]
fn timeout_fires_by_tick_and_keeps_body_and_inventory() {
    let mut world = World::new(1);
    let inventory = world.inventory();
    let start = world.position();
    let base_x = start[0].floor() as i32;
    let base_z = start[2].floor() as i32;
    let legs = (1..=20)
        .map(|leg| go_to(if leg % 2 == 1 { base_x + 15 } else { base_x }, base_z))
        .collect();
    world.install(plan(legs));
    let stored = world.stored();
    assert_eq!(stored.state, COMPANION_TASK_RUNNING);
    let deadline = stored.deadline_ticks;
    assert_eq!(deadline, stored.start_tick + TICKS_PER_MINUTE);
    let mut timed_out = 0;
    let mut ticks = 0;
    loop {
        // The runner pass inside the step reads the settled clock as it is
        // before the step; the clock only moves by one per real tick.
        let clock = world.world_time();
        let states = task_states(world.step());
        ticks += 1;
        assert_eq!(world.world_time(), clock + 1, "no world-time shortcut");
        assert!(
            !states.contains(&TaskState::Completed),
            "the trip outlasts the timeout"
        );
        timed_out += states
            .iter()
            .filter(|state| **state == TaskState::TimedOut)
            .count();
        if clock + 1 == deadline {
            assert_eq!(timed_out, 0, "one tick before the deadline");
            assert_eq!(world.phase(), Some(CompanionChatPhase::Running));
        }
        if clock == deadline {
            assert_eq!(timed_out, 1, "exactly one TimedOut on the deadline tick");
            break;
        }
        assert!(ticks <= 4_000, "Go's event bound");
    }
    assert!(ticks >= 1_199, "about 1200 real ticks, got {ticks}");
    assert_eq!(world.stored().state, COMPANION_TASK_TIMED_OUT);
    assert!(world.phase().is_none());

    let stop = world.position();
    let issuer = world.issuer;
    world.outbox(issuer);
    for _ in 0..5 {
        let report = world.step();
        assert_eq!(moves(report), 0, "no movement after the timeout");
        assert!(task_states(report).is_empty());
        assert_eq!(world.body().lifecycle, ActorLifecycle::Active);
        assert!(
            world.outbox(issuer).iter().any(|packet| matches!(
                packet,
                ServerPacket::CompanionStates(batch)
                    if batch.states.iter().any(|state| state.companion_id == amu())
            )),
            "the body is still published"
        );
    }
    let settled = world.position();
    let dx = settled[0] - stop[0];
    let dz = settled[2] - stop[2];
    assert!(
        dx * dx + dz * dz <= 0.01,
        "stopped: {stop:?} -> {settled:?}"
    );
    assert_eq!(world.inventory(), inventory, "inventory unchanged");
}

/// A closing or failed authority error comes back from the step
/// unchanged (the error `advance_tick` returns) and no task is failed.
#[test]
fn authority_errors_return_unchanged_without_failing_tasks() {
    // Closing.
    let mut world = World::new(10);
    world.install(plan(vec![go_to(14, 8)]));
    assert!(world.state.begin_close());
    let error = world.try_step().unwrap_err();
    assert_eq!(
        error,
        ServerError::InvalidState {
            phase: ServerPhase::Closing
        }
    );
    assert_eq!(world.state.advance_tick(TickBudget::full()), Err(error));
    assert_eq!(world.stored().state, COMPANION_TASK_RUNNING);
    assert_eq!(world.phase(), Some(CompanionChatPhase::Running));

    // Failed: a player body staged without its inventory fails the real
    // authority tick, which retains that failure.
    let mut world = World::new(10);
    world.install(plan(vec![go_to(14, 8)]));
    let ghost = world
        .state
        .prepare(admitted(3, "Cy"), TransportKind::Memory)
        .unwrap();
    world.state.install(ghost, None).unwrap();
    world.state.activate(ghost).unwrap();
    stage(&mut world.state, |context| {
        let position = [2.5, 65.0, 2.5];
        let stored = stored_player(3, "Cy", position);
        let save = PlayerSave {
            player_id: stored.player_id,
            revision: stored.revision,
            display_name: stored.display_name,
            current: stored.current,
            yaw: stored.yaw,
            pitch: stored.pitch,
            safe: stored.safe,
            inventory: stored.inventory,
            health: stored.health,
            hunger: stored.hunger,
            saturation_milli: stored.saturation_milli,
            exhaustion_milli: stored.exhaustion_milli,
            respawn_present: stored.respawn_present,
            respawn_position: stored.respawn_position,
            respawn_dimension: stored.respawn_dimension,
            armor: stored.armor,
        };
        let actor = ActorRecord::try_new(
            ActorKey::Player(ghost),
            ActorLifecycle::Active,
            Dimension::OVERWORLD,
            motion(position),
            LookAngles::try_new(0.0, 0.0).unwrap(),
            survival(),
            ActorBody::Player(save),
        )
        .unwrap();
        context.stage(RuleEffect::Actor(actor)).unwrap();
    });
    let failed = world.try_step().unwrap_err();
    assert!(matches!(failed, ServerError::Internal { .. }), "{failed:?}");
    assert_eq!(world.try_step().unwrap_err(), failed, "retained unchanged");
    assert_eq!(world.state.advance_tick(TickBudget::full()), Err(failed));
    assert_eq!(world.stored().state, COMPANION_TASK_RUNNING);
    assert_eq!(world.phase(), Some(CompanionChatPhase::Running));
    assert!(
        world.reports.iter().all(|report| !task_states(report)
            .iter()
            .any(|state| matches!(state, TaskState::Failed(_)))),
        "no task failed"
    );
}

/// Go `companion_manager.go:277` (the timeout is copied at
/// construction and never changes) and `config.go:696-704` (1..=60): the
/// frozen timeout cannot change after startup. Rebuilding the startup
/// value afterwards does not reach the running assembly, which stamps
/// its own value; out-of-range values never construct.
#[test]
fn frozen_timeout_cannot_change_after_startup() {
    assert!(TaskTimeoutMinutes::try_new(0).is_err());
    assert!(TaskTimeoutMinutes::try_new(61).is_err());
    let mut startup = TickStartup::new(TaskTimeoutMinutes::try_new(3).unwrap(), TickBudget::full());
    let mut world = World::with_startup(startup);
    startup = TickStartup::new(TaskTimeoutMinutes::try_new(60).unwrap(), TickBudget::full());
    assert_eq!(startup.task_timeout().get(), 60);
    assert_eq!(world.tick.task_timeout().get(), 3);
    world.install(plan(vec![go_to(14, 8)]));
    let stored = world.stored();
    assert_eq!(
        stored.deadline_ticks,
        stored.start_tick + 3 * TICKS_PER_MINUTE
    );
}
