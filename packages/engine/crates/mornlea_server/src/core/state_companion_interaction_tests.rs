//! End-to-end companion mine/place execution through the real authority
//! tick: the runner pass decides at the tick boundary, its actions are
//! submitted as the running generation, and the authority mining and
//! placement providers settle them. The scenarios mirror the Go
//! interaction tests in `packages/server/server/companion_interact_test.go`.

use super::super::companion_chat::CompanionChatBook;
use super::super::companion_runner::{CompanionPathOutcome, PathOutcome};
use super::super::world::PreparedChunk;
use super::*;
use mornlea_domain::{
    ChatIntent, ChunkPos, FiniteVec3, MotionStateParts, SurvivalState, SurvivalStateParts,
};
use mornlea_engine::native::contracts::PathfindOp;
use mornlea_engine::native::contracts::pathfind::{PathCell, PathScratch};
use mornlea_engine::native::pathfind::NativePathfind;
use mornlea_storage::{
    COMPANION_TASK_COMPLETED, COMPANION_TASK_FAIL_INVENTORY_FULL, COMPANION_TASK_FAIL_NONE,
    COMPANION_TASK_FAIL_WORLD_CHANGED, COMPANION_TASK_FAILED, COMPANION_TASK_RUNNING,
    COMPANION_TASK_TIMED_OUT, ContainerSnapshot, ItemStack, StorageKind,
};

const STONE: u16 = 2;
const DIRT: u16 = 3;
const AIR: u16 = 0;
const ITEM_STONE: u16 = 1;
const ITEM_DIRT: u16 = 2;

fn raw(tag: u8) -> [u8; 16] {
    let mut bytes = [0u8; 16];
    bytes[0] = tag;
    bytes[6] = 0x40;
    bytes[8] = 0x80;
    bytes
}

fn chunk_key(x: i32, z: i32) -> ChunkKey {
    ChunkKey {
        dimension: Dimension::OVERWORLD,
        pos: ChunkPos::new(x, z),
    }
}

/// A flat stone floor topping out at y=-1 with the listed blocks set in
/// the y=0..15 section, so every y=0 cell stands on stone.
fn floor_chunk(blocks: &[(i32, i32, i32, u16)]) -> Chunk {
    let empty = ContainerSnapshot {
        kind: StorageKind::Single,
        bits: 0,
        single: 0,
        palette: vec![],
        packed: vec![],
    };
    let mut chunk = Chunk {
        sections: vec![empty; 24],
        drops: vec![Default::default(); 32],
        furnaces: vec![Default::default(); 32],
        chests: vec![Default::default(); 16],
    };
    for section in &mut chunk.sections[..4] {
        section.single = STONE;
    }
    if !blocks.is_empty() {
        let mut packed = vec![0u64; 1024];
        for &(x, y, z, block) in blocks {
            let cell = (y as usize) * 256 + (z as usize) * 16 + x as usize;
            packed[cell / 4] |= u64::from(block) << ((cell % 4) * 15);
        }
        chunk.sections[4] = ContainerSnapshot {
            kind: StorageKind::Direct,
            bits: 15,
            single: 0,
            palette: vec![],
            packed,
        };
    }
    chunk
}

/// Live authority with the 3x3 chunks around the origin chunk Ready; the
/// origin chunk carries the listed blocks.
/// Chat configuration is sealed after the first tick, so the companions
/// are configured before any chunk load settles.
fn world(
    blocks: &[(i32, i32, i32, u16)],
    definitions: &[(CompanionId, CompanionName)],
) -> AuthorityState {
    let mut a = AuthorityState::try_new(
        ServerLimits::try_new(8, 4096, 512, 64, 64, 1_048_576).unwrap(),
        42,
    )
    .unwrap();
    if !definitions.is_empty() {
        a.configure_companion_chat(definitions).unwrap();
    }
    a.enable_live_chunks().unwrap();
    let keys: BTreeSet<ChunkKey> = (-1..=1)
        .flat_map(|x| (-1..=1).map(move |z| chunk_key(x, z)))
        .collect();
    a.replace_chunk_wants(keys.clone()).unwrap();
    for (index, key) in keys.into_iter().enumerate() {
        let reservation = a.reserve_chunk_load(key).unwrap();
        let request = ChunkRequestId::try_new(index as u64 + 1).unwrap();
        a.bind_chunk_load(reservation, request).unwrap();
        let body = if key.pos == ChunkPos::new(0, 0) {
            floor_chunk(blocks)
        } else {
            floor_chunk(&[])
        };
        let prepared = PreparedChunk::try_new(
            key,
            reservation.generation(),
            RecoveredChunk {
                chunk: body,
                revision: 9,
                persisted_revision: 9,
                needs_rewrite: false,
                recovered: false,
            },
        )
        .unwrap();
        a.offer_acquired(AcquiredChunkEvent::Load {
            key,
            generation: reservation.generation(),
            request,
            result: Ok(Some(prepared)),
        })
        .unwrap();
        // At most eight loads are in flight; settle each batch.
        if index % 8 == 7 {
            a.advance_tick(TickBudget::full()).unwrap();
        }
    }
    a.advance_tick(TickBudget::full()).unwrap();
    a
}

fn companion_actor(id: CompanionId, position: [f32; 3]) -> ActorRecord {
    ActorRecord::try_new(
        ActorKey::Companion(id),
        ActorLifecycle::Active,
        Dimension::OVERWORLD,
        MotionState::new(MotionStateParts {
            position: FiniteVec3::try_new(position).unwrap(),
            velocity: FiniteVec3::try_new([0.0; 3]).unwrap(),
            on_ground: true,
        }),
        LookAngles::try_new(0.0, 0.0).unwrap(),
        SurvivalState::try_new(SurvivalStateParts {
            health: 20,
            oxygen: 300,
            hunger: 20,
            saturation_zero: false,
            armor_points: 0,
        })
        .unwrap(),
        ActorBody::Companion(mornlea_storage::CompanionBody {
            id: mornlea_storage::PlayerId::from_bytes(id.bytes()),
            dimension: 0,
            position,
            yaw: 0.0,
            pitch: 0.0,
            inventory: Default::default(),
        }),
    )
    .unwrap()
}

/// Stages one companion actor with its runtime and inventory.
fn add_companion(
    a: &mut AuthorityState,
    id: CompanionId,
    position: [f32; 3],
    inventory: InventoryRecord,
) {
    let mut ctx = TickContext::for_tick(a, TickBudget::full());
    ctx.stage(RuleEffect::Actor(companion_actor(id, position)))
        .unwrap();
    ctx.preload_inventory(ActorKey::Companion(id), inventory);
    ctx.commit_carried();
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
        peak_y: 0.0,
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

fn amu() -> (CompanionId, CompanionName) {
    (
        CompanionId::try_from_bytes(raw(9)).unwrap(),
        CompanionName::try_from_canonical("阿木".to_owned()).unwrap(),
    )
}

fn nova() -> (CompanionId, CompanionName) {
    (
        CompanionId::try_from_bytes(raw(10)).unwrap(),
        CompanionName::try_from_canonical("Nova".to_owned()).unwrap(),
    )
}

fn logged(a: &mut AuthorityState) -> SessionKey {
    let player =
        PlayerId::try_from_bytes([1, 0, 0, 0, 0, 0, 64, 0, 128, 0, 0, 0, 0, 0, 0, 1]).unwrap();
    let start = mornlea_protocol::LoginStart::new(player, "Ada", 8).unwrap();
    let inbound = mornlea_protocol::LoginStart::decode_inbound(&start.encode().unwrap()).unwrap();
    a.admit(
        mornlea_protocol::admit_login(inbound).unwrap(),
        TransportKind::Memory,
    )
    .unwrap()
}

fn chat(text: &str) -> PlayIntent {
    PlayIntent::Chat(
        ChatIntent::try_new(CommandText::try_from_canonical(text.to_owned()).unwrap()).unwrap(),
    )
}

fn stack(item: u16, count: u8) -> ItemStack {
    ItemStack {
        item,
        count,
        durability: 0,
    }
}

fn inventory_with(slots: &[(usize, ItemStack)]) -> InventoryRecord {
    let mut inventory = InventoryRecord::empty();
    for &(slot, value) in slots {
        inventory.slots[slot] = value;
    }
    inventory
}

/// Every one of the 36 slots holds a full stone stack.
fn full_of_stone() -> InventoryRecord {
    let mut inventory = InventoryRecord::empty();
    inventory.slots = [stack(ITEM_STONE, 64); 36];
    inventory
}

/// A Ready world with the companions placed and configured for chat.
struct Harness {
    a: AuthorityState,
    session: SessionKey,
    pending: Vec<CompanionPathOutcome>,
    next_request: u32,
    facts: Vec<DecidedChatFact>,
}

impl Harness {
    fn new(
        blocks: &[(i32, i32, i32, u16)],
        companions: &[((CompanionId, CompanionName), [f32; 3], InventoryRecord)],
    ) -> Self {
        let definitions: Vec<_> = companions.iter().map(|(def, _, _)| def.clone()).collect();
        let mut a = world(blocks, &definitions);
        let session = logged(&mut a);
        for ((id, _), position, inventory) in companions {
            add_companion(&mut a, *id, *position, *inventory);
            a.residents
                .runtimes
                .insert(ActorKey::Companion(*id), companion_runtime(*id));
        }
        Self {
            a,
            session,
            pending: Vec::new(),
            next_request: 0,
            facts: Vec::new(),
        }
    }

    /// Issues a chat command and installs the plan as the running task.
    fn run_plan(&mut self, name: &str, id: CompanionId, steps: Vec<PlanStep>, minutes: u32) {
        self.a
            .submit(self.session, chat(&format!("@{name} work")))
            .unwrap();
        self.a.prepare_companion_chat().unwrap();
        let planning = self
            .a
            .take_companion_chat_planning(id)
            .unwrap()
            .expect("planning receipt");
        let plan = AgentPlan::try_new("执行".to_owned(), steps).unwrap();
        assert!(
            self.a
                .install_companion_chat_plan_with_timeout(id, planning.generation, plan, minutes)
                .unwrap()
        );
        self.collect_facts();
    }

    /// Decided facts are published by the next tick, so they are drained
    /// at every boundary before the tick runs.
    fn collect_facts(&mut self) {
        let facts = self.a.take_decided_chat_facts();
        self.facts.extend(facts);
    }

    /// Moves world time to the running task's deadline.
    fn reach_deadline(&mut self, id: CompanionId) {
        let deadline = self.stored(id).deadline_ticks;
        self.a.metadata.world_time_ticks = deadline;
        if let Some(environment) = self.a.residents.environment.as_mut() {
            environment.world_time = deadline;
        }
    }

    /// One runtime pass: deliver held path outcomes, run the runner pass,
    /// search the requested paths for the next pass, submit the actions as
    /// the running generation, then advance the authority tick.
    fn step(&mut self) {
        let delivered = std::mem::take(&mut self.pending);
        let output = self.a.advance_companion_runners(delivered).unwrap();
        self.collect_facts();
        for request in output.path_requests {
            let cells = request.grid.size().iter().map(|v| *v as usize).product();
            let mut scratch = PathScratch::try_with_capacity(cells).unwrap();
            let cell = |pos: BlockPos| PathCell {
                x: pos.x(),
                y: pos.y(),
                z: pos.z(),
            };
            let outcome = match NativePathfind.find(
                &request.grid,
                cell(request.start),
                cell(request.goal),
                &mut scratch,
            ) {
                Ok(result) => PathOutcome::Found {
                    waypoints: result
                        .waypoints()
                        .iter()
                        .map(|c| BlockPos::new(c.x, c.y, c.z))
                        .collect(),
                    revisions: result
                        .revisions()
                        .iter()
                        .map(|r| (ChunkPos::new(r.chunk[0], r.chunk[1]), r.revision))
                        .collect(),
                },
                Err(_) => PathOutcome::Failed,
            };
            self.pending.push(CompanionPathOutcome {
                companion: request.companion,
                generation: request.generation,
                outcome,
            });
        }
        for (id, action) in output.actions {
            let generation = self
                .a
                .companion_chat
                .current_task(id)
                .expect("an action belongs to a running task")
                .generation;
            self.next_request += 1;
            let mut request = raw(200);
            request[12..16].copy_from_slice(&self.next_request.to_be_bytes());
            let envelope = CompanionActionEnvelope::try_new(
                id,
                self.a.next_tick(),
                AgentRequestId::try_from_bytes(request).unwrap(),
                RunId::try_from_bytes(raw(4)).unwrap(),
                SnapshotId::try_from_bytes(raw(5)).unwrap(),
                generation,
                1,
                [0; 32],
                action,
            )
            .unwrap();
            self.a.submit_companion(envelope).unwrap();
        }
        self.a.advance_tick(TickBudget::full()).unwrap();
    }

    fn running(&self, id: CompanionId) -> bool {
        self.a.companion_chat_queue(id).unwrap().current.is_some()
    }

    /// Steps until the companion's task leaves the slot; panics past the
    /// bound so a stuck task is a failure, not a hang.
    fn run_to_end(&mut self, id: CompanionId, bound: usize) -> usize {
        for ticks in 0..bound {
            if !self.running(id) {
                return ticks;
            }
            self.step();
        }
        panic!("task still running after {bound} ticks");
    }

    fn block(&self, pos: BlockPos) -> Option<u16> {
        self.a
            .settled_read()
            .unwrap()
            .observation(Dimension::OVERWORLD, pos)
            .map(|observed| observed.block)
    }

    fn inventory(&self, id: CompanionId) -> InventoryRecord {
        *self
            .a
            .residents
            .inventories
            .get(&ActorKey::Companion(id))
            .expect("companion inventory")
    }

    fn stored(&self, id: CompanionId) -> StoredCompanionTask {
        match &self
            .a
            .residents
            .runtimes
            .get(&ActorKey::Companion(id))
            .expect("companion runtime")
            .aux
        {
            ActorAux::Companion { task, .. } => task.clone(),
            _ => unreachable!(),
        }
    }

    /// Task states announced for the companion since the last drain.
    fn task_facts(&self, id: CompanionId) -> Vec<TaskState> {
        self.facts
            .iter()
            .filter_map(|fact| match &fact.body {
                ChatBody::Task {
                    companion, state, ..
                } if companion.id() == id => Some(*state),
                _ => None,
            })
            .collect()
    }

    /// World drops across the Ready window.
    fn drops(&self) -> usize {
        let view = self.a.settled_read().unwrap();
        (-1..=1)
            .flat_map(|x| (-1..=1).map(move |z| chunk_key(x, z)))
            .map(|key| view.drops(key).len())
            .sum()
    }
}

/// Another actor's edit between ticks, staged as a system block write.
fn set_block(a: &mut AuthorityState, at: BlockPos, block: u16) {
    let mut ctx = TickContext::for_tick(a, TickBudget::full());
    let observed = ctx.read().observation(Dimension::OVERWORLD, at).unwrap();
    let tick = ctx.read().tick();
    ctx.stage(RuleEffect::Blocks(BlockTxn::system(
        SystemRule::Support,
        tick,
        vec![BlockWrite::try_new(observed, block).unwrap()],
    )))
    .unwrap();
    ctx.commit_carried();
}

fn pos(x: i32, y: i32, z: i32) -> BlockPos {
    BlockPos::new(x, y, z)
}

fn mine(x: i32, y: i32, z: i32) -> PlanStep {
    PlanStep::Mine { x, y, z }
}

fn place(x: i32, y: i32, z: i32) -> PlanStep {
    PlanStep::Place {
        x,
        y,
        z,
        block: PlanBlock::Dirt,
    }
}

fn total(inventory: &InventoryRecord, item: u16) -> u32 {
    inventory
        .slots
        .iter()
        .filter(|slot| slot.item == item)
        .map(|slot| u32::from(slot.count))
        .sum()
}

/// Mirrors `TestCompanionManagerMineCompletesWithEventsAndLoot`: the
/// broken block's single product merges into the companion inventory,
/// nothing is dropped in the world, and the task completes.
#[test]
fn mine_credits_the_product_into_the_companion_inventory() {
    let (id, name) = amu();
    let mut h = Harness::new(
        &[(10, 0, 8, DIRT)],
        &[(
            (id, name),
            [9.5, 0.0, 8.5],
            inventory_with(&[(4, stack(ITEM_DIRT, 5))]),
        )],
    );
    h.run_plan("阿木", id, vec![mine(10, 0, 8)], 5);
    h.run_to_end(id, 60);
    assert_eq!(h.block(pos(10, 0, 8)), Some(AIR));
    let inventory = h.inventory(id);
    assert_eq!(
        inventory.slots[4],
        stack(ITEM_DIRT, 6),
        "merged into the existing stack"
    );
    assert_eq!(total(&inventory, ITEM_DIRT), 6);
    assert_eq!(h.drops(), 0, "no world drop");
    assert_eq!(h.stored(id).state, COMPANION_TASK_COMPLETED);
    assert_eq!(h.stored(id).fail_reason, COMPANION_TASK_FAIL_NONE);
    assert_eq!(h.task_facts(id).last(), Some(&TaskState::Completed));
    assert!(!h.a.residents.mining.contains_key(&ActorKey::Companion(id)));
}

/// With every slot full but one matching stack short of its cap, the
/// product still merges: capacity is judged per stack, not per slot.
#[test]
fn mine_into_a_full_inventory_merges_into_a_matching_stack() {
    let (id, name) = amu();
    let mut inventory = full_of_stone();
    inventory.slots[20] = stack(ITEM_DIRT, 63);
    let mut h = Harness::new(
        &[(10, 0, 8, DIRT)],
        &[((id, name), [9.5, 0.0, 8.5], inventory)],
    );
    h.run_plan("阿木", id, vec![mine(10, 0, 8)], 5);
    h.run_to_end(id, 60);
    assert_eq!(h.block(pos(10, 0, 8)), Some(AIR));
    let after = h.inventory(id);
    assert_eq!(after.slots[20], stack(ITEM_DIRT, 64));
    assert_eq!(total(&after, ITEM_STONE), 35 * 64);
    assert_eq!(h.stored(id).state, COMPANION_TASK_COMPLETED);
}

/// Mirrors `TestCompanionManagerMineInventoryFullKeepsBlock`: with no
/// room for the product the block is kept, the inventory is untouched,
/// and only this task fails as inventory full.
#[test]
fn mine_without_capacity_keeps_the_block_and_fails_inventory_full() {
    let (id, name) = amu();
    let mut h = Harness::new(
        &[(10, 0, 8, DIRT)],
        &[((id, name), [9.5, 0.0, 8.5], full_of_stone())],
    );
    h.run_plan("阿木", id, vec![mine(10, 0, 8)], 5);
    h.run_to_end(id, 60);
    assert_eq!(h.block(pos(10, 0, 8)), Some(DIRT), "block kept");
    assert_eq!(h.inventory(id), full_of_stone(), "inventory untouched");
    assert_eq!(h.drops(), 0);
    let stored = h.stored(id);
    assert_eq!(stored.state, COMPANION_TASK_FAILED);
    assert_eq!(stored.fail_reason, COMPANION_TASK_FAIL_INVENTORY_FULL);
    assert_eq!(
        h.task_facts(id).last(),
        Some(&TaskState::Failed(TaskFailure::InventoryFull))
    );
    assert!(
        !h.a.residents.mining.contains_key(&ActorKey::Companion(id)),
        "the finish path clears the hold and progress"
    );
    assert!(
        h.a.residents
            .actors
            .iter()
            .any(|actor| actor.key == ActorKey::Companion(id))
    );
}

/// Mirrors `TestCompanionManagerPlaceAtomicConsumeAndDepletion`: one
/// successful placement debits exactly one item.
#[test]
fn place_success_deducts_exactly_one() {
    let (id, name) = amu();
    let mut h = Harness::new(
        &[],
        &[(
            (id, name),
            [9.5, 0.0, 8.5],
            inventory_with(&[(0, stack(ITEM_DIRT, 3))]),
        )],
    );
    h.run_plan("阿木", id, vec![place(10, 0, 8)], 5);
    h.run_to_end(id, 60);
    assert_eq!(h.block(pos(10, 0, 8)), Some(DIRT));
    assert_eq!(h.inventory(id).slots[0], stack(ITEM_DIRT, 2));
    assert_eq!(h.stored(id).state, COMPANION_TASK_COMPLETED);
    assert_eq!(h.task_facts(id).last(), Some(&TaskState::Completed));
}

/// Depletion keeps the first placement and fails the second step as
/// inventory full, with the emptied inventory left as it was.
#[test]
fn place_depletion_keeps_prior_placement_and_fails_inventory_full() {
    let (id, name) = amu();
    let mut h = Harness::new(
        &[],
        &[(
            (id, name),
            [9.5, 0.0, 8.5],
            inventory_with(&[(0, stack(ITEM_DIRT, 1))]),
        )],
    );
    h.run_plan("阿木", id, vec![place(10, 0, 8), place(10, 1, 8)], 5);
    h.run_to_end(id, 80);
    assert_eq!(h.block(pos(10, 0, 8)), Some(DIRT), "first placement kept");
    assert_eq!(h.block(pos(10, 1, 8)), Some(AIR));
    assert_eq!(h.inventory(id), InventoryRecord::empty());
    let stored = h.stored(id);
    assert_eq!(stored.state, COMPANION_TASK_FAILED);
    assert_eq!(stored.fail_reason, COMPANION_TASK_FAIL_INVENTORY_FULL);
    assert_eq!(
        h.task_facts(id),
        vec![
            TaskState::Started,
            TaskState::Progress,
            TaskState::Failed(TaskFailure::InventoryFull)
        ]
    );
}

/// Mirrors `TestCompanionManagerPlaceTargetOccupiedFailsWorldChanged`:
/// an occupied target is never overwritten and the inventory stays
/// byte-identical.
#[test]
fn place_into_an_occupied_target_fails_world_changed_with_inventory_identical() {
    let (id, name) = amu();
    let inventory = inventory_with(&[(0, stack(ITEM_DIRT, 2)), (7, stack(ITEM_STONE, 9))]);
    let mut h = Harness::new(
        &[(10, 0, 8, STONE)],
        &[((id, name), [9.5, 0.0, 8.5], inventory)],
    );
    h.run_plan("阿木", id, vec![place(10, 0, 8)], 5);
    h.run_to_end(id, 60);
    assert_eq!(h.block(pos(10, 0, 8)), Some(STONE));
    assert_eq!(h.inventory(id), inventory, "inventory byte-identical");
    let stored = h.stored(id);
    assert_eq!(stored.state, COMPANION_TASK_FAILED);
    assert_eq!(stored.fail_reason, COMPANION_TASK_FAIL_WORLD_CHANGED);
    assert_eq!(
        h.task_facts(id).last(),
        Some(&TaskState::Failed(TaskFailure::WorldChanged))
    );
}

/// Mirrors `TestCompanionManagerMineTargetReplacedFailsWorldChanged` for
/// a target already gone before mining: only that companion's task fails
/// as world changed; the other companion keeps mining to completion.
#[test]
fn world_changed_before_mine_fails_only_that_task() {
    let a_def = amu();
    let b_def = nova();
    let (a_id, b_id) = (a_def.0, b_def.0);
    let mut h = Harness::new(
        &[(5, 0, 4, DIRT)],
        &[
            (a_def, [9.5, 0.0, 8.5], InventoryRecord::empty()),
            (b_def, [4.5, 0.0, 4.5], InventoryRecord::empty()),
        ],
    );
    h.run_plan("阿木", a_id, vec![mine(10, 0, 8)], 5);
    h.run_plan("Nova", b_id, vec![mine(5, 0, 4)], 5);
    for _ in 0..60 {
        if !h.running(a_id) && !h.running(b_id) {
            break;
        }
        h.step();
    }
    let a_task = h.stored(a_id);
    assert_eq!(a_task.state, COMPANION_TASK_FAILED);
    assert_eq!(a_task.fail_reason, COMPANION_TASK_FAIL_WORLD_CHANGED);
    assert_eq!(h.inventory(a_id), InventoryRecord::empty());
    let b_task = h.stored(b_id);
    assert_eq!(b_task.state, COMPANION_TASK_COMPLETED);
    assert_eq!(h.block(pos(5, 0, 4)), Some(AIR));
    assert_eq!(h.inventory(b_id).slots[0], stack(ITEM_DIRT, 1));
    assert_eq!(
        h.task_facts(a_id).last(),
        Some(&TaskState::Failed(TaskFailure::WorldChanged))
    );
    assert_eq!(h.task_facts(b_id).last(), Some(&TaskState::Completed));
    for id in [a_id, b_id] {
        assert!(
            h.a.residents
                .actors
                .iter()
                .any(|actor| actor.key == ActorKey::Companion(id))
        );
    }
}

/// A target replaced mid-mine (progress regresses or the required ticks
/// change) fails world changed and the new block is never mined.
#[test]
fn target_replaced_mid_mine_fails_world_changed() {
    let (id, name) = amu();
    let mut h = Harness::new(
        &[(10, 0, 8, DIRT)],
        &[((id, name), [9.5, 0.0, 8.5], InventoryRecord::empty())],
    );
    h.run_plan("阿木", id, vec![mine(10, 0, 8)], 5);
    // Run until progress is observed on the target.
    for _ in 0..30 {
        h.step();
        if h.a
            .residents
            .mining
            .get(&ActorKey::Companion(id))
            .is_some_and(|progress| progress.elapsed >= 2)
        {
            break;
        }
    }
    // Another actor swaps the block for stone, a different block/tool key.
    set_block(&mut h.a, pos(10, 0, 8), STONE);
    h.run_to_end(id, 60);
    assert_eq!(
        h.block(pos(10, 0, 8)),
        Some(STONE),
        "the new block is not mined"
    );
    assert_eq!(h.inventory(id), InventoryRecord::empty());
    let stored = h.stored(id);
    assert_eq!(stored.state, COMPANION_TASK_FAILED);
    assert_eq!(stored.fail_reason, COMPANION_TASK_FAIL_WORLD_CHANGED);
}

/// Steps run in plan order: walk, mine, walk, place. The mined product
/// is the item the placement consumes, so nothing is duplicated or lost.
#[test]
fn go_to_mine_go_to_place_runs_in_order_to_completion() {
    let (id, name) = amu();
    let mut h = Harness::new(
        &[(7, 0, 8, DIRT)],
        &[((id, name), [9.5, 0.0, 8.5], InventoryRecord::empty())],
    );
    h.run_plan(
        "阿木",
        id,
        vec![
            PlanStep::GoTo { x: 6, y: 0, z: 10 },
            mine(7, 0, 8),
            PlanStep::GoTo { x: 6, y: 0, z: 12 },
            place(7, 0, 12),
        ],
        5,
    );
    h.run_to_end(id, 600);
    assert_eq!(h.stored(id).state, COMPANION_TASK_COMPLETED);
    assert_eq!(h.block(pos(7, 0, 8)), Some(AIR));
    assert_eq!(h.block(pos(7, 0, 12)), Some(DIRT));
    assert_eq!(
        h.inventory(id),
        InventoryRecord::empty(),
        "mined item placed"
    );
    assert_eq!(
        h.task_facts(id),
        vec![
            TaskState::Started,
            TaskState::Progress,
            TaskState::Progress,
            TaskState::Progress,
            TaskState::Completed
        ]
    );
}

/// Timing out mid-mine is still a timeout: the hold and progress are
/// cleared, the block and inventory are untouched.
#[test]
fn timeout_during_mine_is_timed_out() {
    let (id, name) = amu();
    let mut h = Harness::new(
        &[(10, 0, 8, DIRT)],
        &[((id, name), [9.5, 0.0, 8.5], InventoryRecord::empty())],
    );
    h.run_plan("阿木", id, vec![mine(10, 0, 8)], 5);
    for _ in 0..30 {
        h.step();
        if h.a.residents.mining.contains_key(&ActorKey::Companion(id)) {
            break;
        }
    }
    assert!(
        h.a.residents.mining.contains_key(&ActorKey::Companion(id)),
        "the deadline lands mid-mine"
    );
    assert!(h.running(id));
    h.reach_deadline(id);
    h.step();
    assert!(!h.running(id));
    let stored = h.stored(id);
    assert_eq!(stored.state, COMPANION_TASK_TIMED_OUT);
    assert_eq!(stored.fail_reason, COMPANION_TASK_FAIL_NONE);
    assert_eq!(h.block(pos(10, 0, 8)), Some(DIRT));
    assert_eq!(h.inventory(id), InventoryRecord::empty());
    assert!(!h.a.residents.mining.contains_key(&ActorKey::Companion(id)));
    assert_eq!(h.task_facts(id).last(), Some(&TaskState::TimedOut));
}

/// Timing out while waiting to place leaves the inventory identical.
#[test]
fn timeout_during_place_is_timed_out_with_inventory_identical() {
    let (id, name) = amu();
    let inventory = inventory_with(&[(0, stack(ITEM_DIRT, 2))]);
    let mut h = Harness::new(&[], &[((id, name), [9.5, 0.0, 8.5], inventory)]);
    h.run_plan("阿木", id, vec![place(10, 0, 8)], 5);
    // Approach completes, but the deadline lands before the submission.
    while !h.a.companion_runners[&id].interaction_ready() {
        h.step();
    }
    h.reach_deadline(id);
    h.step();
    assert_eq!(h.stored(id).state, COMPANION_TASK_TIMED_OUT);
    assert_eq!(h.block(pos(10, 0, 8)), Some(AIR));
    assert_eq!(h.inventory(id), inventory);
}

/// Saves the running task and rebuilds an authority from what persists:
/// the world blocks, the inventory, and the stored running task. The
/// runner, the mining progress, and the queued envelopes are process
/// memory and do not survive.
fn restore(
    h: &Harness,
    id: CompanionId,
    name: CompanionName,
    blocks: &[(i32, i32, i32, u16)],
) -> Harness {
    assert_eq!(h.stored(id).state, COMPANION_TASK_RUNNING);
    let inventory = h.inventory(id);
    let actor =
        h.a.residents
            .actors
            .iter()
            .find(|actor| actor.key == ActorKey::Companion(id))
            .unwrap();
    let position = actor.motion.position().get();
    let ActorBody::Companion(body) = &actor.body else {
        unreachable!()
    };
    let raw_id = mornlea_storage::PlayerId::from_bytes(id.bytes());
    let mut loaded = mornlea_storage::StoredCompanions {
        source_schema: 5,
        revision: 1,
        agent_namespace_id: raw_id,
        records: vec![body.clone()],
        lifecycles: vec![mornlea_storage::StoredCompanionLifecycle {
            id: raw_id,
            active: true,
            memory_epoch: 1,
            memory_revision: 0,
            memory_operation_id: Default::default(),
            summary: String::new(),
            tombstone_operation_id: Default::default(),
        }],
        queues: Vec::new(),
    };
    // The running task is captured through the real save path.
    loaded.queues =
        h.a.companion_chat
            .snapshot_persisted(&loaded, &h.a.residents.runtimes)
            .unwrap();
    let mut a = world(blocks, &[]);
    let session = logged(&mut a);
    let (book, tasks) = CompanionChatBook::from_persisted(&[(id, name)], &loaded).unwrap();
    a.companion_chat = book;
    add_companion(&mut a, id, position, inventory);
    let mut runtime = companion_runtime(id);
    if let ActorAux::Companion {
        generation, task, ..
    } = &mut runtime.aux
    {
        *generation = tasks[&id].0;
        *task = tasks[&id].1.clone();
    }
    a.residents
        .runtimes
        .insert(ActorKey::Companion(id), runtime);
    Harness {
        a,
        session,
        pending: Vec::new(),
        next_request: 0,
        facts: Vec::new(),
    }
}

/// Restoring mid-mine re-approaches and mines from zero progress: the
/// block yields exactly one product across the restore.
#[test]
fn restore_mid_mine_restarts_and_credits_exactly_once() {
    let (id, name) = amu();
    let mut h = Harness::new(
        &[(10, 0, 8, DIRT)],
        &[(
            (id, name.clone()),
            [9.5, 0.0, 8.5],
            inventory_with(&[(0, stack(ITEM_DIRT, 1))]),
        )],
    );
    h.run_plan("阿木", id, vec![mine(10, 0, 8)], 5);
    for _ in 0..30 {
        h.step();
        if h.a
            .residents
            .mining
            .get(&ActorKey::Companion(id))
            .is_some_and(|progress| progress.elapsed >= 2)
        {
            break;
        }
    }
    assert_eq!(
        h.block(pos(10, 0, 8)),
        Some(DIRT),
        "saved before settlement"
    );
    let mut restored = restore(&h, id, name, &[(10, 0, 8, DIRT)]);
    assert!(restored.a.residents.mining.is_empty());
    restored.step();
    assert!(
        !restored.a.companion_runners[&id].interaction_ready(),
        "a restored interaction starts from its approach"
    );
    restored.run_to_end(id, 60);
    assert_eq!(restored.stored(id).state, COMPANION_TASK_COMPLETED);
    assert_eq!(restored.block(pos(10, 0, 8)), Some(AIR));
    assert_eq!(restored.inventory(id).slots[0], stack(ITEM_DIRT, 2));
    assert_eq!(total(&restored.inventory(id), ITEM_DIRT), 2);
}

/// Restoring after the block settled but before the step advanced finds
/// the target gone with no progress evidence and fails world changed, as
/// Go does; the credited product is kept exactly once.
#[test]
fn restore_after_mine_settled_fails_world_changed_without_losing_the_product() {
    let (id, name) = amu();
    let mut h = Harness::new(
        &[(10, 0, 8, DIRT)],
        &[(
            (id, name.clone()),
            [9.5, 0.0, 8.5],
            InventoryRecord::empty(),
        )],
    );
    h.run_plan("阿木", id, vec![mine(10, 0, 8)], 5);
    for _ in 0..30 {
        h.step();
        if h.block(pos(10, 0, 8)) == Some(AIR) {
            break;
        }
    }
    assert_eq!(h.block(pos(10, 0, 8)), Some(AIR));
    assert!(
        h.running(id),
        "saved before the runner observed the settlement"
    );
    let mut restored = restore(&h, id, name, &[]);
    restored.run_to_end(id, 60);
    let stored = restored.stored(id);
    assert_eq!(stored.state, COMPANION_TASK_FAILED);
    assert_eq!(stored.fail_reason, COMPANION_TASK_FAIL_WORLD_CHANGED);
    assert_eq!(restored.inventory(id).slots[0], stack(ITEM_DIRT, 1));
    assert_eq!(total(&restored.inventory(id), ITEM_DIRT), 1);
}

/// Restoring a place step after its placement settled completes the step
/// from the observed block without placing or debiting again.
#[test]
fn restore_after_place_settled_completes_without_a_second_debit() {
    let (id, name) = amu();
    let mut h = Harness::new(
        &[],
        &[(
            (id, name.clone()),
            [9.5, 0.0, 8.5],
            inventory_with(&[(0, stack(ITEM_DIRT, 3))]),
        )],
    );
    h.run_plan("阿木", id, vec![place(10, 0, 8)], 5);
    for _ in 0..30 {
        h.step();
        if h.block(pos(10, 0, 8)) == Some(DIRT) {
            break;
        }
    }
    assert!(h.running(id));
    assert_eq!(h.inventory(id).slots[0], stack(ITEM_DIRT, 2));
    let mut restored = restore(&h, id, name, &[(10, 0, 8, DIRT)]);
    restored.run_to_end(id, 60);
    assert_eq!(restored.stored(id).state, COMPANION_TASK_COMPLETED);
    assert_eq!(restored.inventory(id).slots[0], stack(ITEM_DIRT, 2));
}
