//! Per-tick publication projection cases.
//!
//! These tests pin the tick-end projection that constructs the visibility and
//! record event families beside the existing private player observation: the
//! frozen family order inside one tick, the exact payloads built through the
//! same checked domain constructors the projection uses, the resync provider
//! leg, the chat addressing outcomes, and the refusal boundaries (chunks that
//! are not Ready, resyncs outside interest, refused commands, and the chat
//! queue ceiling).

use mornlea_domain::{
    self, BlockChange, BlockPos, ChatBody, ChatEvent, ChatEventParts, ChatIntent, ChunkPos,
    Command, CommandText, CompanionDespawn, CompanionId, CompanionName, CompanionSpawn,
    CompanionSpawnParts, CompanionSpeaker, CompanionStates, ContainerKind, ContainerRef,
    CraftingSize, CraftingState, CraftingStateParts, Dimension, DisplayName, DropId, Event,
    EventRecipient, FiniteVec3, ForgetChunks, HeldActions, HostileId, HostileKind, HostileSpawn,
    HostileSpawnParts, HostileSpawnRecord, HostileSpawnRecordParts, HotbarSlot, InventoryState,
    InventoryStateParts, ItemDrop, ItemDropParts, ItemDropUpserts, ItemStack, LookAngles,
    MotionState, MotionStateParts, Movement, PartialMove, PassiveDespawn, PassiveDespawnParts,
    PassiveDespawnReason, PassiveDespawnRecord, PassiveId, PassiveSpawn, PassiveSpawnParts,
    PassiveSpawnRecord, PassiveSpawnRecordParts, PassiveState, PassiveStateParts,
    PassiveStateRecord, PassiveStateRecordParts, PlayerControl, PlayerControlParts, PlayerId,
    ProjectileDespawn, ProjectileDespawnParts, ProjectileId, ProjectileKind, ProjectileSpawn,
    ProjectileSpawnParts, ProjectileSpawnRecord, ProjectileSpawnRecordParts, ProjectileState,
    ProjectileStateParts, ProjectileStateRecord, ProjectileStateRecordParts, RemotePlayerSpawn,
    RemotePlayerSpawnParts, RemotePlayerState, RemotePlayerStateParts, RemotePlayerStates,
    ResyncIntent, StackSource, StackView, SurvivalState, SurvivalStateParts, TaskFailure,
    TaskState, Weather,
};
use mornlea_protocol::{AdmittedLogin, LoginStart, PlayIntent, admit_login};
use mornlea_server::contracts::{
    ActorAux, ActorBody, ActorKey, ActorLifecycle, ActorRecord, ActorRuntime, AgentPlan,
    AgentRequestId, ChunkKey, CloseReason, CompanionAction, CompanionActionEnvelope, DropRecord,
    EnvironmentState, PlanStep, ProjectileRecord, Resource, RuleEffect, RuleTunables, RunId,
    ServerError, ServerLimits, SessionKey, SnapshotId, TickBudget, TransportKind,
};
use mornlea_server::core::companion_chat::CompanionChatPhase;
use mornlea_server::core::world::ReadyChunk;
use mornlea_server::state::{AuthorityState, TickContext};
use mornlea_storage::{
    COMPANION_TASK_FAIL_INVALID_PLAN, COMPANION_TASK_FAIL_INVENTORY_FULL,
    COMPANION_TASK_FAIL_PATH_UNREACHABLE, COMPANION_TASK_FAIL_PLANNER_UNAVAILABLE,
    COMPANION_TASK_FAIL_WORLD_CHANGED, COMPANION_TASK_FAILED, COMPANION_TASK_RUNNING,
    COMPANION_TASK_STOPPED, ChestSlot, Chunk, CompanionBody, ContainerSnapshot, FurnaceSlot,
    HostileMob, Inventory, ItemStack as StorageStack, PassiveMob, PlayerId as SavePlayerId,
    PlayerLocation, StorageKind, StoredCompanionTask, StoredPlayer,
};

const GRASS: u16 = 4;
const DIRT_BLOCK: u16 = 3;
const ITEM_DIRT: u16 = 2;
const ITEM_STONE: u16 = 1;
const ITEM_STONE_HOE: u16 = 30;
const ITEM_STICK: u16 = 37;
const ITEM_RAW_IRON: u16 = 6;
const ITEM_COAL: u16 = 5;

fn authority() -> AuthorityState {
    AuthorityState::try_new(
        ServerLimits::try_new(8, 4096, 512, 64, 64, 1_048_576)
            .unwrap()
            .with_view_radius(2),
        7,
    )
    .unwrap()
}

fn authority_with_view_radius(cap: usize) -> AuthorityState {
    AuthorityState::try_new(
        ServerLimits::try_new(8, 4096, 512, 64, 64, 1_048_576)
            .unwrap()
            .with_view_radius(cap),
        7,
    )
    .unwrap()
}

fn uuid(tag: u8) -> [u8; 16] {
    let mut bytes = [0u8; 16];
    bytes[0] = tag.max(1);
    bytes[6] = 0x40;
    bytes[8] = 0x80;
    bytes
}

fn admitted(tag: u8, name: &str) -> AdmittedLogin {
    admitted_with_view(tag, name, 8)
}

fn admitted_with_view(tag: u8, name: &str, view: u8) -> AdmittedLogin {
    let id = PlayerId::try_from_bytes(uuid(tag)).unwrap();
    let start = LoginStart::new(id, name, view).unwrap();
    let inbound = LoginStart::decode_inbound(&start.encode().unwrap()).unwrap();
    admit_login(inbound).unwrap()
}

fn stored_with(
    tag: u8,
    name: &str,
    position: [f32; 3],
    yaw: f32,
    pitch: f32,
    customize: impl FnOnce(&mut StoredPlayer),
) -> StoredPlayer {
    let mut player = StoredPlayer {
        player_id: SavePlayerId::from_bytes(uuid(tag)),
        revision: 1,
        display_name: name.to_owned(),
        current: PlayerLocation {
            dimension: 0,
            position,
        },
        yaw,
        pitch,
        safe: None,
        inventory: Inventory::default(),
        health: 20,
        hunger: 20,
        saturation_milli: 5_000,
        exhaustion_milli: 0,
        respawn_present: false,
        respawn_position: [0.0; 3],
        respawn_dimension: 0,
        armor: [StorageStack::default(); 4],
        needs_rewrite: false,
    };
    customize(&mut player);
    player
}

fn login(
    state: &mut AuthorityState,
    tag: u8,
    name: &str,
    position: [f32; 3],
    yaw: f32,
    pitch: f32,
) -> SessionKey {
    login_with(state, tag, name, position, yaw, pitch, |_| {})
}

fn login_with(
    state: &mut AuthorityState,
    tag: u8,
    name: &str,
    position: [f32; 3],
    yaw: f32,
    pitch: f32,
    customize: impl FnOnce(&mut StoredPlayer),
) -> SessionKey {
    let login = admitted(tag, name);
    let stored = stored_with(tag, name, position, yaw, pitch, customize);
    let session = state.prepare(login, TransportKind::Memory).unwrap();
    state.install(session, Some(stored)).unwrap();
    state.activate(session).unwrap();
    session
}

fn login_declared(
    state: &mut AuthorityState,
    tag: u8,
    name: &str,
    position: [f32; 3],
    yaw: f32,
    pitch: f32,
    view: u8,
) -> SessionKey {
    let login = admitted_with_view(tag, name, view);
    let stored = stored_with(tag, name, position, yaw, pitch, |_| {});
    let session = state.prepare(login, TransportKind::Memory).unwrap();
    state.install(session, Some(stored)).unwrap();
    state.activate(session).unwrap();
    session
}

fn snapshot_positions(events: &[Event]) -> Vec<ChunkPos> {
    events
        .iter()
        .filter_map(|event| match event {
            Event::ChunkSnapshot(snapshot) => Some(snapshot.chunk()),
            _ => None,
        })
        .collect()
}

fn forget_positions(events: &[Event]) -> Vec<ChunkPos> {
    let mut positions = Vec::new();
    for event in events {
        if let Event::ForgetChunks(batch) = event {
            positions.extend(batch.chunks().iter().copied());
        }
    }
    positions
}

fn preload_keys(state: &mut AuthorityState, keys: &[(i32, i32)]) {
    stage(state, |context| {
        for (x, z) in keys {
            context.preload_ready_chunk(
                ReadyChunk::try_new(chunk_key(*x, *z), 1, 1, ground_chunk()).unwrap(),
            );
        }
    });
}

fn chunk_key(x: i32, z: i32) -> ChunkKey {
    ChunkKey {
        dimension: Dimension::OVERWORLD,
        pos: ChunkPos::new(x, z),
    }
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

/// Writes one block cell into a compact chunk, converting the touched section
/// to direct storage the way the container fixtures do.
fn set_cell(chunk: &mut Chunk, pos: BlockPos, block: u16) {
    let index = mornlea_domain::chunk_block_index(pos) as usize;
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

/// A walkable chunk: one grass layer at y 64 across the whole column.
fn ground_chunk() -> Chunk {
    let mut chunk = air_chunk();
    for x in 0..16 {
        for z in 0..16 {
            set_cell(&mut chunk, BlockPos::new(x, 64, z), GRASS);
        }
    }
    chunk
}

/// Installs one active chest slot at the given cell and returns its reference.
fn chest_in_chunk(chunk: &mut Chunk, pos: BlockPos, items: [StorageStack; 27]) -> ContainerRef {
    set_cell(chunk, pos, 11);
    let index = mornlea_domain::chunk_block_index(pos) as u32;
    chunk.chests[0] = ChestSlot {
        generation: 1,
        active: true,
        block_index: index,
        items,
    };
    ContainerRef::try_new(
        ChunkPos::new(pos.x() >> 4, pos.z() >> 4),
        ContainerKind::Chest,
        0,
        1,
    )
    .unwrap()
}

/// Installs one active furnace slot at the given cell and returns its reference.
#[allow(clippy::too_many_arguments)]
fn furnace_in_chunk(
    chunk: &mut Chunk,
    pos: BlockPos,
    input: StorageStack,
    fuel: StorageStack,
    output: StorageStack,
    burn_ticks: u16,
    progress_ticks: u8,
) -> ContainerRef {
    set_cell(chunk, pos, 9);
    let index = mornlea_domain::chunk_block_index(pos) as u32;
    chunk.furnaces[1] = FurnaceSlot {
        generation: 1,
        active: true,
        block_index: index,
        input,
        fuel,
        output,
        burn_ticks,
        progress_ticks,
    };
    ContainerRef::try_new(
        ChunkPos::new(pos.x() >> 4, pos.z() >> 4),
        ContainerKind::Furnace,
        1,
        1,
    )
    .unwrap()
}

/// Stages fixtures between production ticks on a context seeded from the
/// committed residents, so every existing lane survives the round trip.
fn stage(state: &mut AuthorityState, stage: impl FnOnce(&mut TickContext<'_>)) {
    let mut context = TickContext::restage(state, TickBudget::full());
    stage(&mut context);
    let residents = context.resident_snapshot();
    drop(context);
    state.commit_residents(residents);
}

fn environment(world_time: u64) -> EnvironmentState {
    EnvironmentState {
        seed: 7,
        next_tick: 0,
        world_time,
        day_phase_offset: 0,
        season_offset: 0,
        weather: Weather::Clear,
        weather_remaining: 0,
        difficulty: 0,
        tunables: RuleTunables::source_defaults(),
    }
}

fn companion_id(tag: u8) -> CompanionId {
    CompanionId::try_from_bytes(uuid(tag)).unwrap()
}

/// The deterministic published companion name: `companion-` plus the
/// lowercase hex of the identity's first eight bytes.
fn derived_name(id: CompanionId) -> String {
    let mut text = String::from("companion-");
    for byte in &id.bytes()[..8] {
        text.push_str(&format!("{byte:02x}"));
    }
    text
}

/// The configured chat companion: real identity tag 9 with the name `U+963F U+6728`.
fn amu_id() -> CompanionId {
    companion_id(9)
}

/// The configured chat name `U+963F U+6728`.
fn amu_name() -> CompanionName {
    CompanionName::try_from_canonical("阿木".to_owned()).unwrap()
}

/// Registers the configured `U+963F U+6728` companion before the first tick.
fn configure_amu(state: &mut AuthorityState) {
    state
        .configure_companion_chat(&[(amu_id(), amu_name())])
        .unwrap();
}

/// A second configured companion for independent-bound probes.
fn second_pair() -> (CompanionId, CompanionName) {
    (
        companion_id(10),
        CompanionName::try_from_canonical("阿火".to_owned()).unwrap(),
    )
}

/// A neutral companion runtime with provider-owned attempt 7 and no task.
fn companion_chat_runtime(id: CompanionId) -> ActorRuntime {
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
            attempt: 7,
            task: StoredCompanionTask::default(),
            mining_target: None,
        },
    }
}

/// Stages one active companion actor with a neutral companion runtime.
fn stage_companion_with_runtime(state: &mut AuthorityState, id: CompanionId, position: [f32; 3]) {
    stage(state, |context| {
        context
            .stage(RuleEffect::Actor(companion_actor(id, position, 0.0)))
            .unwrap();
        context
            .stage(RuleEffect::Runtime(companion_chat_runtime(id)))
            .unwrap();
    });
}

/// A checked terminal-follow plan for the issuer's player identity.
fn follow_plan(player: PlayerId) -> AgentPlan {
    AgentPlan::try_new(
        "跟随我".to_owned(),
        vec![PlanStep::Follow { player_id: player }],
    )
    .unwrap()
}

/// A checked finite plan with no terminal follow.
fn finite_plan() -> AgentPlan {
    AgentPlan::try_new(
        "前进".to_owned(),
        vec![PlanStep::GoTo { x: 1, y: 65, z: 1 }],
    )
    .unwrap()
}

/// One companion action envelope naming an explicit task generation.
fn chat_envelope(
    id: CompanionId,
    generation: u64,
    tag: u8,
    action: CompanionAction,
) -> CompanionActionEnvelope {
    CompanionActionEnvelope::try_new(
        id,
        0,
        AgentRequestId::try_from_bytes(uuid(tag)).unwrap(),
        RunId::try_from_bytes(uuid(tag + 1)).unwrap(),
        SnapshotId::try_from_bytes(uuid(tag + 2)).unwrap(),
        generation,
        1,
        [0u8; 32],
        action,
    )
    .unwrap()
}

/// Every broadcast chat event in one publication, in publication order.
fn broadcast_chats(
    publication: &mornlea_server::contracts::TickPublication,
) -> Vec<mornlea_domain::ChatEvent> {
    publication
        .events
        .iter()
        .filter(|event| event.recipient() == EventRecipient::Broadcast)
        .filter_map(|event| match event.event() {
            Event::Chat(chat) => Some(chat.clone()),
            _ => None,
        })
        .collect()
}

/// Every chat event in one publication regardless of recipient, in order.
fn all_chats(
    publication: &mornlea_server::contracts::TickPublication,
) -> Vec<mornlea_domain::ChatEvent> {
    publication
        .events
        .iter()
        .filter_map(|event| match event.event() {
            Event::Chat(chat) => Some(chat.clone()),
            _ => None,
        })
        .collect()
}

/// Every chat event addressed to one session, in publication order.
///
/// Returns owned clones (test-only, bounded by the 256 intake ceiling) so
/// callers can borrow the temporary session view without escaping it.
fn session_chats(events: &[Event]) -> Vec<ChatEvent> {
    events
        .iter()
        .filter_map(|event| match event {
            Event::Chat(event) => Some(event.clone()),
            _ => None,
        })
        .collect()
}

/// Asserts one sender-only rejection: the exact body at one monotonic id,
/// session-only delivery, chat-quiet observers, and no broadcast fact.
fn assert_sender_only_rejection(
    publication: &mornlea_server::contracts::TickPublication,
    sender: SessionKey,
    listener: SessionKey,
    id: u64,
    body: &ChatBody,
) {
    let sender_chats = session_chats(&events_for(publication, sender));
    assert_eq!(sender_chats.len(), 1);
    assert_eq!(
        sender_chats[0],
        ChatEvent::try_new(ChatEventParts {
            event_id: id,
            player_id: PlayerId::try_from_bytes(uuid(1)).unwrap(),
            player_name: DisplayName::try_from_canonical("Ada".to_owned()).unwrap(),
            body: body.clone(),
        })
        .unwrap()
    );
    assert!(session_chats(&events_for(publication, listener)).is_empty());
    assert!(broadcast_chats(publication).is_empty());
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

fn look(yaw: f32, pitch: f32) -> LookAngles {
    LookAngles::try_new(yaw, pitch).unwrap()
}

fn companion_actor(id: CompanionId, position: [f32; 3], yaw: f32) -> ActorRecord {
    ActorRecord::try_new(
        ActorKey::Companion(id),
        ActorLifecycle::Active,
        Dimension::OVERWORLD,
        MotionState::new(MotionStateParts {
            position: FiniteVec3::try_new(position).unwrap(),
            velocity: FiniteVec3::try_new([0.0; 3]).unwrap(),
            on_ground: true,
        }),
        look(yaw, 0.0),
        survival(),
        ActorBody::Companion(CompanionBody {
            id: SavePlayerId::from_bytes(id.bytes()),
            dimension: 0,
            position,
            yaw,
            pitch: 0.0,
            inventory: Inventory::default(),
        }),
    )
    .unwrap()
}

fn hostile_actor(id: u64, position: [f32; 3], kind: u8) -> ActorRecord {
    ActorRecord::try_new(
        ActorKey::Hostile(HostileId::try_new(id).unwrap()),
        ActorLifecycle::Active,
        Dimension::OVERWORLD,
        MotionState::new(MotionStateParts {
            position: FiniteVec3::try_new(position).unwrap(),
            velocity: FiniteVec3::try_new([0.0; 3]).unwrap(),
            on_ground: true,
        }),
        look(0.0, 0.0),
        survival(),
        ActorBody::Hostile(HostileMob {
            id,
            dimension: 0,
            position,
            velocity: [0.0; 3],
            on_ground: true,
            yaw: 0.0,
            health: 20,
            attack_cooldown: 0,
            hurt_cooldown: 0,
            burn_cooldown: 20,
            has_target: false,
            player_id: SavePlayerId::from_bytes([0; 16]),
            next_repath_ticks: 0,
            distant_ticks: 0,
            kind,
        }),
    )
    .unwrap()
}

fn passive_actor(id: u64, position: [f32; 3]) -> ActorRecord {
    ActorRecord::try_new(
        ActorKey::Passive(PassiveId::try_new(id).unwrap()),
        ActorLifecycle::Active,
        Dimension::OVERWORLD,
        MotionState::new(MotionStateParts {
            position: FiniteVec3::try_new(position).unwrap(),
            velocity: FiniteVec3::try_new([0.0; 3]).unwrap(),
            on_ground: true,
        }),
        look(0.0, 0.0),
        survival(),
        ActorBody::Passive(PassiveMob {
            id,
            dimension: 0,
            position,
            velocity: [0.0; 3],
            on_ground: true,
            yaw: 0.0,
            health: 20,
        }),
    )
    .unwrap()
}

/// The neutral passive runtime the combat settlement row requires: an
/// inactive graze lane at home beside the spawn column.
fn passive_runtime(id: PassiveId) -> ActorRuntime {
    ActorRuntime {
        key: ActorKey::Passive(id),
        controls: None,
        has_view: false,
        reset: false,
        attack_cooldown: 0,
        hurt_cooldown: 0,
        burn_cooldown: 0,
        oxygen: 0,
        peak_y: 65.0,
        exhaustion_milli: 0,
        saturation_milli: 0,
        since_damage_ticks: 0,
        drown_ticks: 0,
        starvation_ticks: 0,
        eating: None,
        bow: None,
        path: None,
        aux: ActorAux::Passive {
            home: BlockPos::new(8, 65, 8),
            flee_ticks: 0,
            flee_from: None,
            graze_ticks: 0,
            graze_at: None,
            fresh: false,
        },
    }
}

fn drop_record(slot: u8, age: u32, position: [f32; 3]) -> DropRecord {
    DropRecord {
        id: DropId::try_new(0, ChunkPos::new(0, 0), slot, 1).unwrap(),
        position: FiniteVec3::try_new(position).unwrap(),
        stack: StorageStack {
            item: ITEM_COAL,
            count: 2,
            durability: 0,
        },
        pickup_delay: 5,
        age,
    }
}

fn stack(item: u16, count: u8) -> ItemStack {
    ItemStack::try_new(item, count, 0).unwrap()
}

fn storage(item: u16, count: u8) -> StorageStack {
    StorageStack {
        item,
        count,
        durability: 0,
    }
}

fn storage_array<const N: usize>(filled: &[(usize, u16, u8)]) -> [StorageStack; N] {
    let mut slots = [StorageStack::default(); N];
    for (index, item, count) in filled {
        slots[*index] = storage(*item, *count);
    }
    slots
}

fn item_array<const N: usize>(filled: &[(usize, u16, u8)]) -> [ItemStack; N] {
    let mut slots = [ItemStack::EMPTY; N];
    for (index, item, count) in filled {
        slots[*index] = stack(*item, *count);
    }
    slots
}

/// One tick's events addressed to one session, in publication order.
fn events_for(
    publication: &mornlea_server::contracts::TickPublication,
    session: SessionKey,
) -> Vec<Event> {
    publication
        .events
        .iter()
        .filter(|event| event.recipient() == EventRecipient::Session(session.get()))
        .map(|event| event.event().clone())
        .collect()
}

/// The first event of one variant addressed to one session.
fn find_event(events: &[Event], predicate: impl Fn(&Event) -> bool) -> Option<&Event> {
    events.iter().find(|event| predicate(event))
}

fn submit(state: &mut AuthorityState, session: SessionKey, sequence: u64, command: Command) {
    state
        .submit(session, PlayIntent::Sequenced { sequence, command })
        .unwrap();
}

fn submit_chat(state: &mut AuthorityState, session: SessionKey, text: &str) {
    state
        .submit(
            session,
            PlayIntent::Chat(ChatIntent::new(
                CommandText::try_from_canonical(text.to_owned()).unwrap(),
            )),
        )
        .unwrap();
}

/// Seeds the shared world fixture: the day environment, the standing ground
/// chunk under the spawn column, and no other residents.
fn seed_world(state: &mut AuthorityState) {
    stage(state, |context| {
        context
            .stage(RuleEffect::Environment(environment(1_000)))
            .unwrap();
        context.preload_ready_chunk(
            ReadyChunk::try_new(chunk_key(0, 0), 1, 1, ground_chunk()).unwrap(),
        );
    });
}

/// projection::idle_authority_publishes_nothing — no active sessions, one
/// advance_tick, publication.events is empty.
#[test]
fn projection_idle_authority_publishes_nothing() {
    let mut state = authority();
    let publication = state.advance_tick(TickBudget::full()).unwrap();
    assert!(publication.events.is_empty());
    // A prepared-only session is not an admitted observer either.
    let _ = state
        .prepare(admitted(1, "Ada"), TransportKind::Memory)
        .unwrap();
    let publication = state.advance_tick(TickBudget::full()).unwrap();
    assert!(publication.events.is_empty());
    // A retired session keeps the projection silent.
    let session = state
        .prepare(admitted(2, "Bo"), TransportKind::Memory)
        .unwrap();
    state.retire(session, CloseReason::PeerGone).unwrap();
    let publication = state.advance_tick(TickBudget::full()).unwrap();
    assert!(publication.events.is_empty());
}

/// projection::remote_lifecycle_spawn_states_despawn_order — two logged-in
/// players staged in the same area exchange spawn, state and despawn events in
/// the frozen family order, and a distant peer never spawns.
#[test]
fn projection_remote_lifecycle_spawn_states_despawn_order() {
    let mut state = authority();
    seed_world(&mut state);
    let first = login(&mut state, 1, "Ada", [0.5, 65.0, 0.5], 0.0, 0.0);
    let second = login(&mut state, 2, "Ben", [4.5, 65.0, 4.5], 0.75, -0.25);
    // Tick A: each observer sees the other spawn, no state batch yet.
    let tick_a = state.advance_tick(TickBudget::full()).unwrap();
    let first_events = events_for(&tick_a, first);
    let expected_spawn = RemotePlayerSpawn::new(RemotePlayerSpawnParts {
        player_id: PlayerId::try_from_bytes(uuid(2)).unwrap(),
        display_name: DisplayName::try_from_canonical("Ben".to_owned()).unwrap(),
        server_tick: 0,
        dimension: Dimension::OVERWORLD,
        position: FiniteVec3::try_new([4.5, 65.0, 4.5]).unwrap(),
        look: look(0.75, -0.25),
    });
    assert_eq!(
        first_events
            .iter()
            .filter(|event| matches!(event, Event::RemotePlayerSpawn(_)))
            .count(),
        1
    );
    assert!(
        first_events.iter().any(
            |event| matches!(event, Event::RemotePlayerSpawn(spawn) if *spawn == expected_spawn)
        )
    );
    assert!(
        !first_events
            .iter()
            .any(|event| matches!(event, Event::RemotePlayerStates(_)))
    );
    // The snapshot precedes the spawn inside tick A.
    let snapshot = first_events
        .iter()
        .position(|event| matches!(event, Event::ChunkSnapshot(_)))
        .unwrap();
    let spawn = first_events
        .iter()
        .position(|event| matches!(event, Event::RemotePlayerSpawn(_)))
        .unwrap();
    assert!(snapshot < spawn);
    // Tick B: no motion, so both persist with one exact state batch.
    let tick_b = state.advance_tick(TickBudget::full()).unwrap();
    let first_events = events_for(&tick_b, first);
    let expected_state = RemotePlayerStates::try_new(mornlea_domain::RemotePlayerStatesParts {
        server_tick: 1,
        states: vec![RemotePlayerState::new(RemotePlayerStateParts {
            player_id: PlayerId::try_from_bytes(uuid(2)).unwrap(),
            dimension: Dimension::OVERWORLD,
            position: FiniteVec3::try_new([4.5, 65.0, 4.5]).unwrap(),
            look: look(0.75, -0.25),
            reset: false,
        })]
        .into_boxed_slice(),
    })
    .unwrap();
    assert_eq!(
        find_event(&first_events, |event| matches!(
            event,
            Event::RemotePlayerStates(_)
        )),
        Some(&Event::RemotePlayerStates(expected_state)),
        "one exact persisting state batch and no second spawn"
    );
    // Move the second player far away and stage one newly ready chunk: the
    // same tick must order despawn before snapshot.
    stage(&mut state, |context| {
        let mut actor = context
            .read()
            .actor(ActorKey::Player(second))
            .cloned()
            .unwrap();
        actor.motion = MotionState::new(MotionStateParts {
            position: FiniteVec3::try_new([500.5, 65.0, 4.5]).unwrap(),
            velocity: FiniteVec3::try_new([0.0; 3]).unwrap(),
            on_ground: true,
        });
        context.stage(RuleEffect::Actor(actor)).unwrap();
        context.preload_ready_chunk(
            ReadyChunk::try_new(chunk_key(1, 0), 1, 1, ground_chunk()).unwrap(),
        );
    });
    let tick_c = state.advance_tick(TickBudget::full()).unwrap();
    let first_events = events_for(&tick_c, first);
    let despawn = first_events
        .iter()
        .position(|event| {
            matches!(
                event,
                Event::RemotePlayerDespawn(despawn) if despawn.player_id()
                    == PlayerId::try_from_bytes(uuid(2)).unwrap()
            )
        })
        .unwrap();
    let snapshot = first_events
        .iter()
        .position(|event| matches!(event, Event::ChunkSnapshot(_)))
        .unwrap();
    assert!(despawn < snapshot, "despawn precedes the later snapshot");
    assert!(
        !first_events
            .iter()
            .any(|event| matches!(event, Event::RemotePlayerStates(_)))
    );
    // A distant third player never becomes visible to the first observer.
    let _far = login(&mut state, 3, "Cleo", [200.5, 65.0, 0.5], 0.0, 0.0);
    for _ in 0..2 {
        let publication = state.advance_tick(TickBudget::full()).unwrap();
        assert!(
            !events_for(&publication, first).iter().any(|event| matches!(
                event,
                Event::RemotePlayerSpawn(spawn) if spawn.player_id()
                    == PlayerId::try_from_bytes(uuid(3)).unwrap()
            ))
        );
    }
    // The projection families all precede the private player observation.
    let publication = state.advance_tick(TickBudget::full()).unwrap();
    for event in publication.events.iter() {
        if matches!(event.event(), Event::PlayerState(_)) {
            continue;
        }
        if let EventRecipient::Session(raw) = event.recipient()
            && raw == first.get()
        {
            let index = publication
                .events
                .iter()
                .position(|other| {
                    other.recipient() == EventRecipient::Session(first.get())
                        && matches!(other.event(), Event::PlayerState(_))
                })
                .unwrap();
            let family = publication.events.iter().position(|e| e == event).unwrap();
            assert!(family < index);
        }
    }
}

/// projection::companion_lifecycle_spawn_states_despawn — one companion near
/// the player publishes its derived-name spawn, a static state batch, and a
/// despawn when the observer's interest exits.
#[test]
fn projection_companion_lifecycle_spawn_states_despawn() {
    let mut state = authority();
    seed_world(&mut state);
    let session = login(&mut state, 1, "Ada", [0.5, 65.0, 0.5], 0.0, 0.0);
    let id = companion_id(9);
    let name = derived_name(id);
    stage(&mut state, |context| {
        context
            .stage(RuleEffect::Actor(companion_actor(
                id,
                [8.5, 65.0, 8.5],
                1.25,
            )))
            .unwrap();
    });
    let tick_a = state.advance_tick(TickBudget::full()).unwrap();
    let events = events_for(&tick_a, session);
    let expected = CompanionSpawn::try_new(CompanionSpawnParts {
        id,
        name: mornlea_domain::CompanionName::try_from_canonical(name.clone()).unwrap(),
        server_tick: 0,
        dimension: Dimension::OVERWORLD,
        position: FiniteVec3::try_new([8.5, 65.0, 8.5]).unwrap(),
        look: look(1.25, 0.0),
    })
    .unwrap();
    assert_eq!(
        find_event(&events, |event| matches!(event, Event::CompanionSpawn(_))),
        Some(&Event::CompanionSpawn(expected)),
        "the spawn carries the exact derived name {name}"
    );
    assert!(
        !events
            .iter()
            .any(|event| matches!(event, Event::CompanionStates(_)))
    );
    let tick_b = state.advance_tick(TickBudget::full()).unwrap();
    let events = events_for(&tick_b, session);
    let expected_states = CompanionStates::try_new(mornlea_domain::CompanionStatesParts {
        server_tick: 1,
        states: vec![
            mornlea_domain::CompanionState::try_new(mornlea_domain::CompanionStateParts {
                id,
                dimension: Dimension::OVERWORLD,
                position: FiniteVec3::try_new([8.5, 65.0, 8.5]).unwrap(),
                look: look(1.25, 0.0),
                reset: false,
            })
            .unwrap(),
        ]
        .into_boxed_slice(),
    })
    .unwrap();
    assert_eq!(
        find_event(&events, |event| matches!(event, Event::CompanionStates(_))),
        Some(&Event::CompanionStates(expected_states))
    );
    // Move the observer's interest away: the companion leaves visibility.
    stage(&mut state, |context| {
        let mut actor = context
            .read()
            .actor(ActorKey::Player(session))
            .cloned()
            .unwrap();
        actor.motion = MotionState::new(MotionStateParts {
            position: FiniteVec3::try_new([400.5, 65.0, 0.5]).unwrap(),
            velocity: FiniteVec3::try_new([0.0; 3]).unwrap(),
            on_ground: true,
        });
        context.stage(RuleEffect::Actor(actor)).unwrap();
    });
    let tick_c = state.advance_tick(TickBudget::full()).unwrap();
    let events = events_for(&tick_c, session);
    assert_eq!(
        find_event(&events, |event| matches!(event, Event::CompanionDespawn(_))),
        Some(&Event::CompanionDespawn(CompanionDespawn::new(id)))
    );
    assert!(
        !events
            .iter()
            .any(|event| matches!(event, Event::CompanionStates(_)))
    );
}

/// projection::chunk_snapshot_then_block_changes_then_forget — the first Ready
/// contact publishes the exact column, later writes publish contiguous deltas,
/// a late subscriber gets one snapshot instead of a delta, and leaving
/// interest forgets the exact sorted column list.
#[test]
fn projection_chunk_snapshot_then_block_changes_then_forget() {
    let mut state = authority();
    seed_world(&mut state);
    let first = login_with(
        &mut state,
        1,
        "Ada",
        [0.5, 65.0, 0.5],
        0.0,
        0.0,
        |player: &mut StoredPlayer| {
            player.inventory.hotbar.slots[0] = storage(ITEM_DIRT, 3);
        },
    );
    let tick_a = state.advance_tick(TickBudget::full()).unwrap();
    let events = events_for(&tick_a, first);
    let snapshot = events
        .iter()
        .find_map(|event| match event {
            Event::ChunkSnapshot(snapshot) => Some(snapshot.clone()),
            _ => None,
        })
        .unwrap();
    assert_eq!(snapshot.dimension(), Dimension::OVERWORLD);
    assert_eq!(snapshot.chunk(), ChunkPos::new(0, 0));
    assert_eq!(snapshot.revision(), 1);
    for (x, z) in [(0, 0), (7, 3), (15, 15)] {
        let index = mornlea_domain::chunk_block_index(BlockPos::new(x, 64, z)) as usize;
        assert_eq!(snapshot.sections()[8].block_at(index % 4096), Some(GRASS));
    }
    assert_eq!(snapshot.sections()[0].block_at(0), Some(0));

    // One placed block becomes one exact contiguous delta.
    submit(
        &mut state,
        first,
        1,
        Command::PlaceBlock(
            mornlea_domain::PlacementIntent::try_new(
                look(std::f32::consts::PI, -std::f32::consts::FRAC_PI_4),
                0,
            )
            .unwrap(),
        ),
    );
    let tick_b = state.advance_tick(TickBudget::full()).unwrap();
    let events = events_for(&tick_b, first);
    let expected = mornlea_domain::BlockChanges::try_new(mornlea_domain::BlockChangesParts {
        dimension: Dimension::OVERWORLD,
        chunk: ChunkPos::new(0, 0),
        base_revision: 1,
        new_revision: 2,
        changes: vec![BlockChange::try_new(BlockPos::new(0, 65, 2), DIRT_BLOCK).unwrap()]
            .into_boxed_slice(),
    })
    .unwrap();
    assert_eq!(
        find_event(&events, |event| matches!(event, Event::BlockChanges(_))),
        Some(&Event::BlockChanges(expected))
    );

    // A late subscriber first sees the chunk after two revision steps: the
    // first contact is a snapshot, and the next write is a contiguous delta.
    submit(
        &mut state,
        first,
        2,
        Command::PlaceBlock(
            mornlea_domain::PlacementIntent::try_new(
                look(std::f32::consts::PI, -std::f32::consts::FRAC_PI_4),
                0,
            )
            .unwrap(),
        ),
    );
    let second = login(&mut state, 2, "Ben", [1.5, 65.0, 1.5], 0.0, 0.0);
    let tick_c = state.advance_tick(TickBudget::full()).unwrap();
    let second_events = events_for(&tick_c, second);
    let snapshot = second_events
        .iter()
        .find_map(|event| match event {
            Event::ChunkSnapshot(snapshot) => Some(snapshot.clone()),
            _ => None,
        })
        .unwrap();
    assert_eq!(snapshot.chunk(), ChunkPos::new(0, 0));
    assert_eq!(
        snapshot.revision(),
        3,
        "the late subscriber sees a snapshot"
    );
    assert!(
        !second_events
            .iter()
            .any(|event| matches!(event, Event::BlockChanges(_)))
    );
    let first_events = events_for(&tick_c, first);
    let expected = mornlea_domain::BlockChanges::try_new(mornlea_domain::BlockChangesParts {
        dimension: Dimension::OVERWORLD,
        chunk: ChunkPos::new(0, 0),
        base_revision: 2,
        new_revision: 3,
        changes: vec![BlockChange::try_new(BlockPos::new(0, 65, 1), DIRT_BLOCK).unwrap()]
            .into_boxed_slice(),
    })
    .unwrap();
    assert_eq!(
        find_event(&first_events, |event| matches!(
            event,
            Event::BlockChanges(_)
        )),
        Some(&Event::BlockChanges(expected))
    );

    // Both observers see one more contiguous delta after the late snapshot.
    submit(
        &mut state,
        first,
        3,
        Command::PlaceBlock(
            mornlea_domain::PlacementIntent::try_new(
                look(std::f32::consts::PI, -std::f32::consts::FRAC_PI_4),
                0,
            )
            .unwrap(),
        ),
    );
    let tick_d = state.advance_tick(TickBudget::full()).unwrap();
    for session in [first, second] {
        let events = events_for(&tick_d, session);
        let delta = events
            .iter()
            .find_map(|event| match event {
                Event::BlockChanges(batch) => Some(batch.clone()),
                _ => None,
            })
            .unwrap();
        assert_eq!(delta.base_revision(), 3);
        assert_eq!(delta.new_revision(), 4);
        assert_eq!(
            delta.changes(),
            &[BlockChange::try_new(BlockPos::new(0, 66, 1), DIRT_BLOCK).unwrap()]
        );
    }

    // Leaving interest forgets the exact sorted column list.
    stage(&mut state, |context| {
        let mut actor = context
            .read()
            .actor(ActorKey::Player(first))
            .cloned()
            .unwrap();
        actor.motion = MotionState::new(MotionStateParts {
            position: FiniteVec3::try_new([500.5, 65.0, 0.5]).unwrap(),
            velocity: FiniteVec3::try_new([0.0; 3]).unwrap(),
            on_ground: true,
        });
        context.stage(RuleEffect::Actor(actor)).unwrap();
    });
    let tick_e = state.advance_tick(TickBudget::full()).unwrap();
    let events = events_for(&tick_e, first);
    let mut forgotten = BTreeColumnOrder::default();
    for x in -2..=2 {
        for z in -2..=2 {
            forgotten.push(ChunkPos::new(x, z));
        }
    }
    let expected = ForgetChunks::try_new(mornlea_domain::ForgetChunksParts {
        dimension: Dimension::OVERWORLD,
        chunks: forgotten.0.clone().into_boxed_slice(),
    })
    .unwrap();
    assert_eq!(
        find_event(&events, |event| matches!(event, Event::ForgetChunks(_))),
        Some(&Event::ForgetChunks(expected))
    );
}

/// Collects chunk columns in the x-then-z order the forget batch carries.
#[derive(Default)]
struct BTreeColumnOrder(Vec<ChunkPos>);

impl BTreeColumnOrder {
    fn push(&mut self, pos: ChunkPos) {
        self.0.push(pos);
        self.0.sort();
    }
}

/// projection::resync_replies_full_snapshot_and_ignores_have_revision — each
/// provider resync re-sends the full current snapshot before ordinary
/// first-sends, HaveRevision is ignored, and out-of-interest or unready
/// requests produce no event while still consuming their sequence.
#[test]
fn projection_resync_replies_full_snapshot_and_ignores_have_revision() {
    let mut state = authority();
    seed_world(&mut state);
    let session = login(&mut state, 1, "Ada", [0.5, 65.0, 0.5], 0.0, 0.0);
    let _ = state.advance_tick(TickBudget::full()).unwrap();
    // One newly ready chunk beside the resync target, staged after the first
    // contact so the same tick carries one resync and one first send.
    stage(&mut state, |context| {
        context.preload_ready_chunk(
            ReadyChunk::try_new(chunk_key(1, 0), 1, 1, ground_chunk()).unwrap(),
        );
    });
    submit(
        &mut state,
        session,
        1,
        Command::Resync(ResyncIntent::try_new(0, ChunkPos::new(0, 0), 999).unwrap()),
    );
    submit(
        &mut state,
        session,
        2,
        Command::Resync(ResyncIntent::try_new(0, ChunkPos::new(9, 9), 1).unwrap()),
    );
    submit(
        &mut state,
        session,
        3,
        Command::Resync(ResyncIntent::try_new(0, ChunkPos::new(2, 0), 1).unwrap()),
    );
    let tick_b = state.advance_tick(TickBudget::full()).unwrap();
    assert_eq!(
        tick_b.counters.commands, 3,
        "every resync consumes its slot"
    );
    assert_eq!(state.session(session).unwrap().last_applied_sequence, 3);
    let events = events_for(&tick_b, session);
    let snapshots: Vec<_> = events
        .iter()
        .filter_map(|event| match event {
            Event::ChunkSnapshot(snapshot) => Some(snapshot.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(snapshots.len(), 2, "only the wanted ready columns answer");
    assert_eq!(snapshots[0].chunk(), ChunkPos::new(0, 0));
    assert_eq!(snapshots[0].revision(), 1);
    assert_eq!(snapshots[1].chunk(), ChunkPos::new(1, 0));
    // A second resync with a different HaveRevision replies the same payload.
    submit(
        &mut state,
        session,
        4,
        Command::Resync(ResyncIntent::try_new(0, ChunkPos::new(0, 0), 1).unwrap()),
    );
    let tick_c = state.advance_tick(TickBudget::full()).unwrap();
    let events = events_for(&tick_c, session);
    let snapshots: Vec<_> = events
        .iter()
        .filter_map(|event| match event {
            Event::ChunkSnapshot(snapshot) => Some(snapshot.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(snapshots.len(), 1);
    assert_eq!(snapshots[0].chunk(), ChunkPos::new(0, 0));
    assert_eq!(snapshots[0].revision(), 1);
    assert_eq!(snapshots[0].sections()[8], {
        let again = events_for(&tick_b, session);
        let first = again
            .iter()
            .filter_map(|event| match event {
                Event::ChunkSnapshot(snapshot) if snapshot.chunk() == ChunkPos::new(0, 0) => {
                    Some(snapshot.clone())
                }
                _ => None,
            })
            .next()
            .unwrap();
        first.sections()[8].clone()
    });
}

/// projection::hostile_lifecycle_events — one staged hostile becomes visible
/// with an exact spawn record, persists with one exact state batch, and its
/// death leaves only the despawn.
#[test]
fn projection_hostile_lifecycle_events() {
    let mut state = authority();
    seed_world(&mut state);
    let session = login(&mut state, 1, "Ada", [0.5, 65.0, 0.5], 0.0, 0.0);
    let id = HostileId::try_new(31).unwrap();
    stage(&mut state, |context| {
        context
            .stage(RuleEffect::Actor(hostile_actor(31, [8.5, 65.0, 8.5], 0)))
            .unwrap();
    });
    let tick_a = state.advance_tick(TickBudget::full()).unwrap();
    let events = events_for(&tick_a, session);
    let expected = HostileSpawn::try_new(HostileSpawnParts {
        server_tick: 0,
        spawns: vec![
            HostileSpawnRecord::try_new(HostileSpawnRecordParts {
                id,
                dimension: Dimension::OVERWORLD,
                position: FiniteVec3::try_new([8.5, 65.0, 8.5]).unwrap(),
                yaw: 0.0,
                health: 20,
                kind: HostileKind::Nightwalker,
            })
            .unwrap(),
        ]
        .into_boxed_slice(),
    })
    .unwrap();
    assert_eq!(
        find_event(&events, |event| matches!(event, Event::HostileSpawn(_))),
        Some(&Event::HostileSpawn(expected))
    );
    assert!(
        !events
            .iter()
            .any(|event| matches!(event, Event::HostileState(_)))
    );
    // Tick B: the persisting body batch reflects the settled actor record.
    let tick_b = state.advance_tick(TickBudget::full()).unwrap();
    let events = events_for(&tick_b, session);
    let body = state
        .residents()
        .actors
        .iter()
        .find(|actor| actor.key == ActorKey::Hostile(id))
        .cloned()
        .unwrap();
    let expected = mornlea_domain::HostileState::try_new(mornlea_domain::HostileStateParts {
        server_tick: 1,
        states: vec![
            mornlea_domain::HostileStateRecord::try_new(mornlea_domain::HostileStateRecordParts {
                id,
                position: body.motion.position(),
                velocity: body.motion.velocity(),
                yaw: body.look.yaw(),
                health: body.survival.health(),
                kind: HostileKind::Nightwalker,
            })
            .unwrap(),
        ]
        .into_boxed_slice(),
    })
    .unwrap();
    assert_eq!(
        find_event(&events, |event| matches!(event, Event::HostileState(_))),
        Some(&Event::HostileState(expected))
    );
    // Death: the actor leaves the visible set, so only the despawn remains.
    stage(&mut state, |context| {
        let mut actor = context
            .read()
            .actor(ActorKey::Hostile(id))
            .cloned()
            .unwrap();
        actor.lifecycle = ActorLifecycle::Dead;
        context.stage(RuleEffect::Actor(actor)).unwrap();
    });
    let tick_c = state.advance_tick(TickBudget::full()).unwrap();
    let events = events_for(&tick_c, session);
    assert_eq!(
        find_event(&events, |event| matches!(event, Event::HostileDespawn(_))),
        Some(&Event::HostileDespawn(
            mornlea_domain::HostileDespawn::try_new(mornlea_domain::HostileDespawnParts {
                server_tick: 2,
                ids: vec![id].into_boxed_slice(),
            })
            .unwrap()
        ))
    );
    assert!(
        !events
            .iter()
            .any(|event| matches!(event, Event::HostileState(_)))
    );
}

/// projection::passive_lifecycle_events — the same three shapes for a staged
/// cow, with the died despawn reason for the death-settlement removal.
#[test]
fn projection_passive_lifecycle_events() {
    let mut state = authority();
    seed_world(&mut state);
    let session = login(&mut state, 1, "Ada", [0.5, 65.0, 0.5], 0.0, 0.0);
    let id = PassiveId::try_new(7).unwrap();
    stage(&mut state, |context| {
        context
            .stage(RuleEffect::Actor(passive_actor(7, [8.5, 65.0, 8.5])))
            .unwrap();
        context
            .stage(RuleEffect::Runtime(passive_runtime(id)))
            .unwrap();
    });
    let tick_a = state.advance_tick(TickBudget::full()).unwrap();
    let events = events_for(&tick_a, session);
    let body = state
        .residents()
        .actors
        .iter()
        .find(|actor| actor.key == ActorKey::Passive(id))
        .cloned()
        .unwrap();
    let expected = PassiveSpawn::try_new(PassiveSpawnParts {
        server_tick: 0,
        spawns: vec![
            PassiveSpawnRecord::try_new(PassiveSpawnRecordParts {
                id,
                dimension: Dimension::OVERWORLD,
                position: body.motion.position(),
                yaw: body.look.yaw(),
                health: body.survival.health(),
            })
            .unwrap(),
        ]
        .into_boxed_slice(),
    })
    .unwrap();
    assert_eq!(
        find_event(&events, |event| matches!(event, Event::PassiveSpawn(_))),
        Some(&Event::PassiveSpawn(expected))
    );
    let tick_b = state.advance_tick(TickBudget::full()).unwrap();
    let events = events_for(&tick_b, session);
    let body = state
        .residents()
        .actors
        .iter()
        .find(|actor| actor.key == ActorKey::Passive(id))
        .cloned()
        .unwrap();
    let expected = PassiveState::try_new(PassiveStateParts {
        server_tick: 1,
        states: vec![
            PassiveStateRecord::try_new(PassiveStateRecordParts {
                id,
                position: body.motion.position(),
                velocity: body.motion.velocity(),
                yaw: body.look.yaw(),
                health: body.survival.health(),
                grazing: false,
            })
            .unwrap(),
        ]
        .into_boxed_slice(),
    })
    .unwrap();
    assert_eq!(
        find_event(&events, |event| matches!(event, Event::PassiveState(_))),
        Some(&Event::PassiveState(expected))
    );
    stage(&mut state, |context| {
        let mut actor = context
            .read()
            .actor(ActorKey::Passive(id))
            .cloned()
            .unwrap();
        actor.lifecycle = ActorLifecycle::Dead;
        context.stage(RuleEffect::Actor(actor)).unwrap();
    });
    let tick_c = state.advance_tick(TickBudget::full()).unwrap();
    let events = events_for(&tick_c, session);
    let expected = PassiveDespawn::try_new(PassiveDespawnParts {
        server_tick: 2,
        despawns: vec![PassiveDespawnRecord::new(id, PassiveDespawnReason::Died)]
            .into_boxed_slice(),
    })
    .unwrap();
    assert_eq!(
        find_event(&events, |event| matches!(event, Event::PassiveDespawn(_))),
        Some(&Event::PassiveDespawn(expected))
    );
}

/// projection::passive_death_beats_simultaneous_view_exit — a killed cow and
/// a living cow both leave the observer's interest in the same tick: the
/// death publishes Died, the live exit publishes Vanished, both records ride
/// one ascending batch, a session that never observed them sees no despawn,
/// and the next tick repeats nothing.
#[test]
fn projection_passive_death_beats_simultaneous_view_exit() {
    let mut state = authority();
    seed_world(&mut state);
    let observer = login(&mut state, 1, "Ada", [0.5, 65.0, 0.5], 0.0, 0.0);
    let stranger = login(&mut state, 2, "Ben", [200.5, 65.0, 0.5], 0.0, 0.0);
    let dead = PassiveId::try_new(7).unwrap();
    let live = PassiveId::try_new(11).unwrap();
    stage(&mut state, |context| {
        context
            .stage(RuleEffect::Actor(passive_actor(7, [8.5, 65.0, 8.5])))
            .unwrap();
        context
            .stage(RuleEffect::Runtime(passive_runtime(dead)))
            .unwrap();
        context
            .stage(RuleEffect::Actor(passive_actor(11, [9.5, 65.0, 9.5])))
            .unwrap();
        context
            .stage(RuleEffect::Runtime(passive_runtime(live)))
            .unwrap();
    });
    let tick_a = state.advance_tick(TickBudget::full()).unwrap();
    assert_eq!(
        events_for(&tick_a, observer)
            .iter()
            .filter(|event| matches!(event, Event::PassiveSpawn(_)))
            .count(),
        1,
        "both cows ride one spawn batch on the visibility tick"
    );
    assert!(
        !events_for(&tick_a, stranger)
            .iter()
            .any(|event| matches!(event, Event::PassiveDespawn(_))),
        "the distant session observes neither cow"
    );
    // Between ticks the observer walks out of interest while one cow dies:
    // death wins the simultaneous exit for the killed cow only.
    stage(&mut state, |context| {
        let mut gone = context
            .read()
            .actor(ActorKey::Passive(dead))
            .cloned()
            .unwrap();
        gone.lifecycle = ActorLifecycle::Dead;
        context.stage(RuleEffect::Actor(gone)).unwrap();
        let mut walker = context
            .read()
            .actor(ActorKey::Player(observer))
            .cloned()
            .unwrap();
        walker.motion = MotionState::new(MotionStateParts {
            position: FiniteVec3::try_new([500.5, 65.0, 4.5]).unwrap(),
            velocity: FiniteVec3::try_new([0.0; 3]).unwrap(),
            on_ground: true,
        });
        context.stage(RuleEffect::Actor(walker)).unwrap();
    });
    let tick_b = state.advance_tick(TickBudget::full()).unwrap();
    let events = events_for(&tick_b, observer);
    let expected = PassiveDespawn::try_new(PassiveDespawnParts {
        server_tick: 1,
        despawns: vec![
            PassiveDespawnRecord::new(dead, PassiveDespawnReason::Died),
            PassiveDespawnRecord::new(live, PassiveDespawnReason::Vanished),
        ]
        .into_boxed_slice(),
    })
    .unwrap();
    assert_eq!(
        find_event(&events, |event| matches!(event, Event::PassiveDespawn(_))),
        Some(&Event::PassiveDespawn(expected)),
        "death beats the simultaneous view exit; the live exit stays vanished"
    );
    assert!(
        !events_for(&tick_b, stranger)
            .iter()
            .any(|event| matches!(event, Event::PassiveDespawn(_))),
        "only sessions that previously observed them see the despawn"
    );
    // The follow-up tick repeats neither record.
    let tick_c = state.advance_tick(TickBudget::full()).unwrap();
    assert!(
        !events_for(&tick_c, observer)
            .iter()
            .any(|event| matches!(event, Event::PassiveDespawn(_))),
        "the departure publishes exactly once"
    );
}

/// projection::passive_quiet_fallout_reports_vanished — a cow staged just
/// above the world floor with a downward velocity falls through it under the
/// real movement kernel: the producer marks the quiet removal, so the despawn
/// publishes Vanished rather than Died, exactly once.
#[test]
fn projection_passive_quiet_fallout_reports_vanished() {
    let mut state = authority();
    // Managed acquisition for the standing column: tick 0 installs it Ready
    // so the real movement kernel falls through open air instead of
    // colliding with an unmanaged sparse cell at the floor.
    state.enable_live_chunks().unwrap();
    stage(&mut state, |context| {
        context
            .stage(RuleEffect::Environment(environment(1_000)))
            .unwrap();
    });
    let key = chunk_key(0, 0);
    state
        .replace_chunk_wants(std::collections::BTreeSet::from([key]))
        .unwrap();
    let reservation = state.reserve_chunk_load(key).unwrap();
    let request = mornlea_server::contracts::ChunkRequestId::try_new(1).unwrap();
    state.bind_chunk_load(reservation, request).unwrap();
    let prepared = mornlea_server::core::world::PreparedChunk::try_new(
        key,
        reservation.generation(),
        mornlea_server::contracts::RecoveredChunk {
            chunk: ground_chunk(),
            revision: 1,
            persisted_revision: 1,
            needs_rewrite: false,
            recovered: false,
        },
    )
    .unwrap();
    state
        .offer_acquired(
            mornlea_server::core::acquisition::AcquiredChunkEvent::Load {
                key,
                generation: reservation.generation(),
                request,
                result: Ok(Some(prepared)),
            },
        )
        .unwrap();
    let _ = state.advance_tick(TickBudget::full()).unwrap();
    let session = login(&mut state, 1, "Ada", [0.5, 65.0, 0.5], 0.0, 0.0);
    let id = PassiveId::try_new(13).unwrap();
    stage(&mut state, |context| {
        let falling = ActorRecord::try_new(
            ActorKey::Passive(id),
            ActorLifecycle::Active,
            Dimension::OVERWORLD,
            MotionState::new(MotionStateParts {
                position: FiniteVec3::try_new([8.5, -60.0, 8.5]).unwrap(),
                velocity: FiniteVec3::try_new([0.0, -10.0, 0.0]).unwrap(),
                on_ground: false,
            }),
            look(0.0, 0.0),
            survival(),
            ActorBody::Passive(PassiveMob {
                id: 13,
                dimension: 0,
                position: [8.5, -60.0, 8.5],
                velocity: [0.0, -10.0, 0.0],
                on_ground: false,
                yaw: 0.0,
                health: 20,
            }),
        )
        .unwrap();
        context.stage(RuleEffect::Actor(falling)).unwrap();
        context
            .stage(RuleEffect::Runtime(passive_runtime(id)))
            .unwrap();
    });
    let tick_a = state.advance_tick(TickBudget::full()).unwrap();
    assert!(
        events_for(&tick_a, session).iter().any(|event| matches!(
            event,
            Event::PassiveSpawn(batch) if batch.spawns().iter().any(|record| record.id() == id)
        )),
        "the falling cow is visible for one tick before it crosses the floor"
    );
    // The kernel crosses the floor within a bounded fall; the first despawn
    // naming this cow carries vanished, never died.
    let mut vanished = false;
    for _ in 0..40 {
        let publication = state.advance_tick(TickBudget::full()).unwrap();
        let events = events_for(&publication, session);
        if let Some(Event::PassiveDespawn(batch)) = find_event(&events, |event| {
            matches!(
                event,
                Event::PassiveDespawn(batch)
                    if batch.despawns().iter().any(|record| record.id() == id)
            )
        }) {
            assert_eq!(
                batch.despawns(),
                [PassiveDespawnRecord::new(
                    id,
                    PassiveDespawnReason::Vanished
                )],
                "the real fall-out removal publishes vanished"
            );
            vanished = true;
            break;
        }
    }
    assert!(
        vanished,
        "the fall crosses the floor within the bounded ticks"
    );
    let follow_up = state.advance_tick(TickBudget::full()).unwrap();
    assert!(
        !events_for(&follow_up, session).iter().any(|event| matches!(
            event,
            Event::PassiveDespawn(batch)
                if batch.despawns().iter().any(|record| record.id() == id)
        )),
        "the quiet removal publishes exactly once"
    );
}

/// projection::projectile_lifecycle_events — one staged projectile publishes
/// its exact spawn, its moved flight body, and its age-expiry removal.
#[test]
fn projection_projectile_lifecycle_events() {
    let mut state = authority();
    seed_world(&mut state);
    let session = login(&mut state, 1, "Ada", [0.5, 65.0, 0.5], 0.0, 0.0);
    let id = ProjectileId::try_new(5).unwrap();
    let stage_projectile = |state: &mut AuthorityState, age: u32, y: f32| {
        stage(state, |context| {
            context
                .stage(RuleEffect::Projectile {
                    before: None,
                    after: Some(ProjectileRecord {
                        id,
                        owner: ActorKey::Player(session),
                        dimension: Dimension::OVERWORLD,
                        position: FiniteVec3::try_new([4.5, y, 4.5]).unwrap(),
                        velocity: FiniteVec3::try_new([0.0, 0.0, 0.0]).unwrap(),
                        kind: ProjectileKind::Shard,
                        damage: 3,
                        age,
                    }),
                })
                .unwrap();
        });
    };
    stage_projectile(&mut state, 97, 100.0);
    let tick_a = state.advance_tick(TickBudget::full()).unwrap();
    let events = events_for(&tick_a, session);
    let flight = state
        .residents()
        .projectiles
        .iter()
        .find(|record| record.id == id)
        .cloned()
        .unwrap();
    let expected = ProjectileSpawn::try_new(ProjectileSpawnParts {
        server_tick: 0,
        spawns: vec![ProjectileSpawnRecord::new(ProjectileSpawnRecordParts {
            id,
            kind: ProjectileKind::Shard,
            dimension: Dimension::OVERWORLD,
            position: flight.position,
            velocity: flight.velocity,
        })]
        .into_boxed_slice(),
    })
    .unwrap();
    assert_eq!(
        find_event(&events, |event| matches!(event, Event::ProjectileSpawn(_))),
        Some(&Event::ProjectileSpawn(expected))
    );
    // Tick B: the flight moved the body; the state batch is exact.
    let tick_b = state.advance_tick(TickBudget::full()).unwrap();
    let events = events_for(&tick_b, session);
    let record = state
        .residents()
        .projectiles
        .iter()
        .find(|record| record.id == id)
        .cloned()
        .unwrap();
    let expected = ProjectileState::try_new(ProjectileStateParts {
        server_tick: 1,
        states: vec![ProjectileStateRecord::new(ProjectileStateRecordParts {
            id,
            position: record.position,
        })]
        .into_boxed_slice(),
    })
    .unwrap();
    assert_eq!(
        find_event(&events, |event| matches!(event, Event::ProjectileState(_))),
        Some(&Event::ProjectileState(expected))
    );
    // Tick D: the age ceiling removes the projectile from the visible set.
    let _ = state.advance_tick(TickBudget::full()).unwrap();
    let tick_d = state.advance_tick(TickBudget::full()).unwrap();
    let events = events_for(&tick_d, session);
    assert_eq!(
        find_event(&events, |event| matches!(
            event,
            Event::ProjectileDespawn(_)
        )),
        Some(&Event::ProjectileDespawn(
            ProjectileDespawn::try_new(ProjectileDespawnParts {
                server_tick: 3,
                ids: vec![id].into_boxed_slice(),
            })
            .unwrap()
        ))
    );
    assert!(
        !events
            .iter()
            .any(|event| matches!(event, Event::ProjectileState(_)))
    );
}

/// projection::item_drop_upserts_and_removes — one staged drop publishes its
/// exact upsert, and the age ceiling removes it with one exact batch.
#[test]
fn projection_item_drop_upserts_and_removes() {
    let mut state = authority();
    seed_world(&mut state);
    let session = login(&mut state, 1, "Ada", [0.5, 65.0, 0.5], 0.0, 0.0);
    // Staged out of identity order: the published batch still ascends.
    stage(&mut state, |context| {
        context.preload_drop(drop_record(3, 5_997, [0.5, 65.5, 2.5]));
        context.preload_drop(drop_record(2, 5_997, [1.5, 65.5, 2.5]));
    });
    let tick_a = state.advance_tick(TickBudget::full()).unwrap();
    let events = events_for(&tick_a, session);
    let expected = ItemDropUpserts::try_new(mornlea_domain::ItemDropUpsertsParts {
        server_tick: 0,
        drops: vec![
            ItemDrop::try_new(ItemDropParts {
                id: DropId::try_new(0, ChunkPos::new(0, 0), 2, 1).unwrap(),
                block_index: mornlea_domain::chunk_block_index(BlockPos::new(1, 65, 2)),
                stack: stack(ITEM_COAL, 2),
            })
            .unwrap(),
            ItemDrop::try_new(ItemDropParts {
                id: DropId::try_new(0, ChunkPos::new(0, 0), 3, 1).unwrap(),
                block_index: mornlea_domain::chunk_block_index(BlockPos::new(0, 65, 2)),
                stack: stack(ITEM_COAL, 2),
            })
            .unwrap(),
        ]
        .into_boxed_slice(),
    })
    .unwrap();
    assert_eq!(
        find_event(&events, |event| matches!(event, Event::ItemDropUpserts(_))),
        Some(&Event::ItemDropUpserts(expected))
    );
    // Two more aged ticks reach the 6000-tick lifetime and remove both drops.
    let _ = state.advance_tick(TickBudget::full()).unwrap();
    let tick_c = state.advance_tick(TickBudget::full()).unwrap();
    let events = events_for(&tick_c, session);
    assert_eq!(
        find_event(&events, |event| matches!(event, Event::ItemDropRemoves(_))),
        Some(&Event::ItemDropRemoves(
            mornlea_domain::ItemDropRemoves::try_new(mornlea_domain::ItemDropRemovesParts {
                server_tick: 2,
                ids: vec![
                    DropId::try_new(0, ChunkPos::new(0, 0), 2, 1).unwrap(),
                    DropId::try_new(0, ChunkPos::new(0, 0), 3, 1).unwrap(),
                ]
                .into_boxed_slice(),
            })
            .unwrap()
        ))
    );
    assert!(
        !events
            .iter()
            .any(|event| matches!(event, Event::ItemDropUpserts(_)))
    );
}

/// projection::chat_contract_config_atomic_bounds_and_idempotence — the
/// immutable address book holds at most four pairs, duplicate ids or names
/// refuse atomically, identical repeats are idempotent across ticks, later
/// differing configurations (including the length-matching duplicate
/// `[A, A]`) refuse, and an explicit empty configuration seals the same way.
#[test]
fn projection_chat_contract_config_atomic_bounds_and_idempotence() {
    let pair = |tag: u8, name: &str| {
        (
            companion_id(tag),
            CompanionName::try_from_canonical(name.to_owned()).unwrap(),
        )
    };
    let (a_id, a_name) = pair(9, "阿木");
    let (b_id, b_name) = pair(10, "阿火");
    let (c_id, c_name) = pair(11, "阿土");
    let (d_id, d_name) = pair(12, "阿水");
    let (e_id, e_name) = pair(13, "阿金");
    let config_err = Err(ServerError::InvalidInput {
        field: "companion_chat_config",
    });

    // Five pairs exceed the bound of four.
    let mut state = authority();
    assert_eq!(
        state.configure_companion_chat(&[
            (a_id, a_name.clone()),
            (b_id, b_name.clone()),
            (c_id, c_name.clone()),
            (d_id, d_name.clone()),
            (e_id, e_name.clone()),
        ]),
        config_err
    );
    // The refusal is atomic: a smaller configuration still installs after it.
    state
        .configure_companion_chat(&[(a_id, a_name.clone())])
        .unwrap();

    // Duplicate ids refuse, and duplicate names refuse.
    let mut state = authority();
    assert_eq!(
        state.configure_companion_chat(&[(a_id, a_name.clone()), (a_id, b_name.clone())]),
        config_err
    );
    assert_eq!(
        state.configure_companion_chat(&[(a_id, a_name.clone()), (b_id, a_name.clone())]),
        config_err
    );

    // Identical repeats are idempotent, before and after the first tick.
    let mut state = authority();
    seed_world(&mut state);
    let _ = login(&mut state, 1, "Ada", [0.5, 65.0, 0.5], 0.0, 0.0);
    state
        .configure_companion_chat(&[(a_id, a_name.clone()), (b_id, b_name.clone())])
        .unwrap();
    state
        .configure_companion_chat(&[(a_id, a_name.clone()), (b_id, b_name.clone())])
        .unwrap();
    let _ = state.advance_tick(TickBudget::full()).unwrap();
    state
        .configure_companion_chat(&[(a_id, a_name.clone()), (b_id, b_name.clone())])
        .unwrap();

    // Existing [A, B] then the length-matching duplicate [A, A]: refused,
    // never reported identical.
    assert_eq!(
        state.configure_companion_chat(&[(a_id, a_name.clone()), (a_id, a_name.clone())]),
        config_err
    );
    // Any later differing configuration refuses once sealed.
    assert_eq!(
        state.configure_companion_chat(&[(a_id, a_name.clone())]),
        config_err
    );

    // An explicit empty configuration seals the same way.
    let mut state = authority();
    state.configure_companion_chat(&[]).unwrap();
    assert_eq!(
        state.configure_companion_chat(&[(a_id, a_name.clone())]),
        config_err
    );
}

/// projection::chat_contract_inactive_admission_and_unknown — an unconfigured
/// active companion's derived name is unknown with no queue view, while a
/// configured companion admits while inactive as a real capacity receipt.
#[test]
fn projection_chat_contract_inactive_admission_and_unknown() {
    // Unconfigured: the live derived name addresses nobody.
    let mut state = authority();
    seed_world(&mut state);
    let sender = login(&mut state, 1, "Ada", [0.5, 65.0, 0.5], 0.0, 0.0);
    let listener = login(&mut state, 2, "Ben", [4.5, 65.0, 4.5], 0.0, 0.0);
    let id = companion_id(9);
    stage(&mut state, |context| {
        context
            .stage(RuleEffect::Actor(companion_actor(
                id,
                [8.5, 65.0, 8.5],
                0.0,
            )))
            .unwrap();
    });
    assert!(state.companion_chat_queue(id).is_none());
    submit_chat(&mut state, sender, &format!("@{} dig", derived_name(id)));
    let tick = state.advance_tick(TickBudget::full()).unwrap();
    let sender_chats = session_chats(&events_for(&tick, sender));
    let unknown = ChatEvent::try_new(ChatEventParts {
        event_id: 1,
        player_id: PlayerId::try_from_bytes(uuid(1)).unwrap(),
        player_name: DisplayName::try_from_canonical("Ada".to_owned()).unwrap(),
        body: ChatBody::UnknownCompanion {
            name: CompanionName::try_from_canonical(derived_name(id)).unwrap(),
        },
    })
    .unwrap();
    assert_eq!(sender_chats, vec![unknown]);
    assert!(session_chats(&events_for(&tick, listener)).is_empty());
    assert!(state.companion_chat_queue(id).is_none());

    // Configured while inactive: the pending entry is a real reservation.
    let mut state = authority();
    seed_world(&mut state);
    let sender = login(&mut state, 1, "Ada", [0.5, 65.0, 0.5], 0.0, 0.0);
    configure_amu(&mut state);
    submit_chat(&mut state, sender, "@阿木 collect wood");
    let tick = state.advance_tick(TickBudget::full()).unwrap();
    let chats = broadcast_chats(&tick);
    assert_eq!(chats.len(), 1);
    assert_eq!(chats[0].event_id(), 1);
    assert_eq!(
        chats[0].body(),
        &ChatBody::Accepted {
            companion: CompanionSpeaker::new(amu_id(), amu_name()),
            command: CommandText::try_from_canonical("collect wood".to_owned()).unwrap(),
        }
    );
    let view = state.companion_chat_queue(amu_id()).unwrap();
    let current = view.current.as_ref().expect("admitted current");
    assert_eq!(current.generation, 1);
    assert_eq!(current.command.as_str(), "collect wood");
    assert_eq!(current.phase, CompanionChatPhase::Queued);
    assert!(view.pending.is_empty());
}

/// projection::chat_contract_fifo_capacity_two_slots — one current task plus
/// sixteen pending commands are actually queued before the eighteenth
/// instruction rejects queue-full with the snapshot preserved; two
/// companions are independently bounded; a sixteen-command batch promotes its
/// head leaving fifteen pending; and an exact stop bypasses the full FIFO as
/// not-following instead of queue-full.
#[test]
fn projection_chat_contract_fifo_capacity_two_slots() {
    let mut state = authority();
    seed_world(&mut state);
    let sender = login(&mut state, 1, "Ada", [0.5, 65.0, 0.5], 0.0, 0.0);
    let listener = login(&mut state, 2, "Ben", [4.5, 65.0, 4.5], 0.0, 0.0);
    let (second_id, second_name) = second_pair();
    state
        .configure_companion_chat(&[(amu_id(), amu_name()), (second_id, second_name.clone())])
        .unwrap();

    // The first instruction becomes the current task; the next sixteen fill
    // the pending FIFO, and every admission broadcasts exactly once.
    submit_chat(&mut state, sender, "@阿木 dig 0");
    let _ = state.advance_tick(TickBudget::full()).unwrap();
    for n in 1..17 {
        submit_chat(&mut state, sender, &format!("@阿木 dig {}", n));
    }
    let tick_a = state.advance_tick(TickBudget::full()).unwrap();
    let chats = broadcast_chats(&tick_a);
    assert_eq!(chats.len(), 16);
    for pair in chats.windows(2) {
        assert!(pair[0].event_id() < pair[1].event_id());
    }
    let view = state.companion_chat_queue(amu_id()).unwrap();
    assert_eq!(view.current.as_ref().unwrap().generation, 1);
    assert_eq!(view.current.as_ref().unwrap().command.as_str(), "dig 0");
    assert_eq!(view.pending.len(), 16);
    assert_eq!(view.pending[0].0.as_str(), "dig 1");
    assert_eq!(view.pending[15].0.as_str(), "dig 16");

    // The eighteenth instruction rejects queue-full with no queue effect,
    // while the second companion admits independently in the same tick.
    submit_chat(&mut state, sender, "@阿木 dig overflow");
    submit_chat(&mut state, sender, "@阿火 hello");
    let tick_b = state.advance_tick(TickBudget::full()).unwrap();
    let chats = broadcast_chats(&tick_b);
    assert_eq!(chats.len(), 1);
    assert_eq!(
        chats[0].body(),
        &ChatBody::Accepted {
            companion: CompanionSpeaker::new(second_id, second_name.clone()),
            command: CommandText::try_from_canonical("hello".to_owned()).unwrap(),
        }
    );
    let sender_chats = session_chats(&events_for(&tick_b, sender));
    assert_eq!(sender_chats.len(), 1);
    assert_eq!(
        sender_chats[0].body(),
        &ChatBody::QueueFull {
            companion: CompanionSpeaker::new(amu_id(), amu_name()),
            command: CommandText::try_from_canonical("dig overflow".to_owned()).unwrap(),
        }
    );
    assert!(session_chats(&events_for(&tick_b, listener)).is_empty());
    let view = state.companion_chat_queue(amu_id()).unwrap();
    assert_eq!(view.current.as_ref().unwrap().generation, 1);
    assert_eq!(view.current.as_ref().unwrap().command.as_str(), "dig 0");
    assert_eq!(view.pending.len(), 16);
    assert_eq!(view.pending[15].0.as_str(), "dig 16");
    let other = state.companion_chat_queue(second_id).unwrap();
    assert_eq!(other.current.as_ref().unwrap().generation, 1);
    assert_eq!(other.current.as_ref().unwrap().command.as_str(), "hello");
    assert!(other.pending.is_empty());

    // A fresh sixteen-command batch promotes its head, leaving fifteen
    // pending; the next tick admits the sixteenth pending entry.
    let mut state = authority();
    seed_world(&mut state);
    let sender = login(&mut state, 1, "Ada", [0.5, 65.0, 0.5], 0.0, 0.0);
    configure_amu(&mut state);
    for n in 0..16 {
        submit_chat(&mut state, sender, &format!("@阿木 dig {}", n));
    }
    let _ = state.advance_tick(TickBudget::full()).unwrap();
    let view = state.companion_chat_queue(amu_id()).unwrap();
    assert_eq!(view.current.as_ref().unwrap().command.as_str(), "dig 0");
    assert_eq!(view.pending.len(), 15);
    submit_chat(&mut state, sender, "@阿木 dig 16");
    let _ = state.advance_tick(TickBudget::full()).unwrap();
    let view = state.companion_chat_queue(amu_id()).unwrap();
    assert_eq!(view.current.as_ref().unwrap().generation, 1);
    assert_eq!(view.current.as_ref().unwrap().command.as_str(), "dig 0");
    assert_eq!(view.pending.len(), 16);
    assert_eq!(view.pending[15].0.as_str(), "dig 16");

    // The exact stop bypasses the full FIFO: the queued task answers
    // not-following instead of queue-full, preserving every queue fact.
    submit_chat(&mut state, sender, "@阿木 停止");
    let tick = state.advance_tick(TickBudget::full()).unwrap();
    let sender_chats = session_chats(&events_for(&tick, sender));
    assert_eq!(sender_chats.len(), 1);
    assert_eq!(
        sender_chats[0].body(),
        &ChatBody::NotFollowing {
            companion: CompanionSpeaker::new(amu_id(), amu_name()),
            command: CommandText::try_from_canonical("停止".to_owned()).unwrap(),
        }
    );
    let view = state.companion_chat_queue(amu_id()).unwrap();
    assert_eq!(view.current.as_ref().unwrap().command.as_str(), "dig 0");
    assert_eq!(view.pending.len(), 16);
}

/// projection::chat_contract_intake_limit_and_stop_phrases — 256 staged
/// entries are admitted to staging while the 257th refuses with the
/// transport capacity error before mutation and no fake overflow fact; the
/// ordinary phrases `U+505C U+6B62 U+79FB U+52A8` and `stop` admit while the trimmed exact
/// `U+505C U+6B62` takes the stop path and is never queued.
#[test]
fn projection_chat_contract_intake_limit_and_stop_phrases() {
    let mut state = authority();
    seed_world(&mut state);
    let sender = login(&mut state, 1, "Ada", [0.5, 65.0, 0.5], 0.0, 0.0);
    configure_amu(&mut state);
    let chat = |text: &str| {
        PlayIntent::Chat(ChatIntent::new(
            CommandText::try_from_canonical(text.to_owned()).unwrap(),
        ))
    };
    for _ in 0..256 {
        state.submit(sender, chat("@阿木 dig")).unwrap();
    }
    assert_eq!(
        state.submit(sender, chat("@阿木 dig")),
        Err(ServerError::Capacity {
            resource: Resource::Commands,
            limit: 256,
            observed: 257,
        })
    );
    let tick = state.advance_tick(TickBudget::full()).unwrap();
    let all = all_chats(&tick);
    // Sixteen admissions plus two hundred forty queue-full rejections: every
    // staged entry yields exactly one real fact, nothing silent or fake.
    assert_eq!(all.len(), 256);
    assert_eq!(
        all.iter()
            .filter(|event| matches!(event.body(), ChatBody::Accepted { .. }))
            .count(),
        16
    );
    assert_eq!(
        all.iter()
            .filter(|event| matches!(event.body(), ChatBody::QueueFull { .. }))
            .count(),
        240
    );
    let view = state.companion_chat_queue(amu_id()).unwrap();
    assert!(view.current.is_some());
    assert_eq!(view.pending.len(), 15);

    // Ordinary phrases admit; the trimmed exact stop does not queue.
    let mut state = authority();
    seed_world(&mut state);
    let sender = login(&mut state, 1, "Ada", [0.5, 65.0, 0.5], 0.0, 0.0);
    configure_amu(&mut state);
    submit_chat(&mut state, sender, "@阿木 停止移动");
    submit_chat(&mut state, sender, "@阿木 stop");
    submit_chat(&mut state, sender, "@阿木  停止");
    let tick = state.advance_tick(TickBudget::full()).unwrap();
    let chats = broadcast_chats(&tick);
    assert_eq!(chats.len(), 2);
    assert_eq!(
        chats[0].body(),
        &ChatBody::Accepted {
            companion: CompanionSpeaker::new(amu_id(), amu_name()),
            command: CommandText::try_from_canonical("停止移动".to_owned()).unwrap(),
        }
    );
    assert_eq!(
        chats[1].body(),
        &ChatBody::Accepted {
            companion: CompanionSpeaker::new(amu_id(), amu_name()),
            command: CommandText::try_from_canonical("stop".to_owned()).unwrap(),
        }
    );
    let sender_chats = session_chats(&events_for(&tick, sender));
    assert_eq!(sender_chats.len(), 1);
    assert!(matches!(
        sender_chats[0].body(),
        ChatBody::NotFollowing { .. }
    ));
    let view = state.companion_chat_queue(amu_id()).unwrap();
    assert_eq!(view.current.as_ref().unwrap().command.as_str(), "停止移动");
    assert_eq!(view.pending.len(), 1);
    assert_eq!(view.pending[0].0.as_str(), "stop");
}

/// projection::chat_contract_stop_rejections_and_take_once — idle, queued,
/// planning, and running-finite tasks all answer a stop with a sender-only
/// not-following rejection that preserves every task and FIFO fact; the
/// planning receipt is returned exactly once with monotonic observer-quiet
/// event ids.
#[test]
fn projection_chat_contract_stop_rejections_and_take_once() {
    let mut state = authority();
    seed_world(&mut state);
    let sender = login(&mut state, 1, "Ada", [0.5, 65.0, 0.5], 0.0, 0.0);
    let listener = login(&mut state, 2, "Ben", [4.5, 65.0, 4.5], 0.0, 0.0);
    configure_amu(&mut state);
    let stop_body = ChatBody::NotFollowing {
        companion: CompanionSpeaker::new(amu_id(), amu_name()),
        command: CommandText::try_from_canonical("停止".to_owned()).unwrap(),
    };
    // The companion actor exists throughout; only the task phase varies.
    stage_companion_with_runtime(&mut state, amu_id(), [8.5, 65.0, 8.5]);

    // Idle: no current task at all.
    submit_chat(&mut state, sender, "@阿木 停止");
    let tick = state.advance_tick(TickBudget::full()).unwrap();
    assert_sender_only_rejection(&tick, sender, listener, 1, &stop_body);
    let view = state.companion_chat_queue(amu_id()).unwrap();
    assert!(view.current.is_none());
    assert!(view.pending.is_empty());

    // Queued: admission without any planning take.
    submit_chat(&mut state, sender, "@阿木 collect stone");
    let _ = state.advance_tick(TickBudget::full()).unwrap();
    submit_chat(&mut state, sender, "@阿木 停止");
    let tick = state.advance_tick(TickBudget::full()).unwrap();
    assert_sender_only_rejection(&tick, sender, listener, 3, &stop_body);
    let view = state.companion_chat_queue(amu_id()).unwrap();
    assert_eq!(view.current.as_ref().unwrap().generation, 1);
    assert_eq!(
        view.current.as_ref().unwrap().phase,
        CompanionChatPhase::Queued
    );

    // Planning: the receipt is returned exactly once, then the stop still
    // rejects with the planning task preserved.
    let first = state
        .take_companion_chat_planning(amu_id())
        .unwrap()
        .expect("planning receipt");
    assert_eq!(first.generation, 1);
    assert_eq!(first.phase, CompanionChatPhase::Planning);
    assert_eq!(first.command.as_str(), "collect stone");
    assert!(
        state
            .take_companion_chat_planning(amu_id())
            .unwrap()
            .is_none()
    );
    submit_chat(&mut state, sender, "@阿木 停止");
    let tick = state.advance_tick(TickBudget::full()).unwrap();
    assert_sender_only_rejection(&tick, sender, listener, 4, &stop_body);
    let view = state.companion_chat_queue(amu_id()).unwrap();
    assert_eq!(
        view.current.as_ref().unwrap().phase,
        CompanionChatPhase::Planning
    );

    // Running with a finite plan: still not following.
    assert!(
        state
            .install_companion_chat_plan(amu_id(), 1, finite_plan())
            .unwrap()
    );
    let tick = state.advance_tick(TickBudget::full()).unwrap();
    assert!(tick
        .events
        .iter()
        .any(|event| matches!(event.event(), Event::Chat(event) if matches!(event.body(), ChatBody::Task { state: TaskState::Started, .. }))));
    submit_chat(&mut state, sender, "@阿木 停止");
    submit_chat(&mut state, sender, "@阿木 停止");
    let tick = state.advance_tick(TickBudget::full()).unwrap();
    let sender_chats = session_chats(&events_for(&tick, sender));
    assert_eq!(sender_chats.len(), 2);
    for chat in &sender_chats {
        assert_eq!(chat.body(), &stop_body);
    }
    let ids: Vec<u64> = sender_chats.iter().map(|event| event.event_id()).collect();
    assert!(ids[0] < ids[1]);
    assert!(session_chats(&events_for(&tick, listener)).is_empty());
    let view = state.companion_chat_queue(amu_id()).unwrap();
    let current = view.current.as_ref().unwrap();
    assert_eq!(current.phase, CompanionChatPhase::Running);
    assert_eq!(current.generation, 1);
    assert!(view.pending.is_empty());
}

/// projection::chat_contract_install_stop_runtime_fence — the real install
/// seam produces a running terminal-follow task the runtime agrees with; a
/// different stopper stops it with the original issuer and original command
/// broadcast while the full pending FIFO is preserved and the head promotes
/// in the same tick; controls, path, mining, and already queued actions are
/// cleared; stale generations refuse and other companions stay unaffected.
#[test]
fn projection_chat_contract_install_stop_runtime_fence() {
    let mut state = authority();
    seed_world(&mut state);
    let sender = login(&mut state, 1, "Ada", [0.5, 65.0, 0.5], 0.0, 0.0);
    let stopper = login(&mut state, 2, "Ben", [4.5, 65.0, 4.5], 0.0, 0.0);
    configure_amu(&mut state);
    stage_companion_with_runtime(&mut state, amu_id(), [8.5, 65.0, 8.5]);
    submit_chat(&mut state, sender, "@阿木 dig 0");
    let _ = state.advance_tick(TickBudget::full()).unwrap();

    let planned = state
        .take_companion_chat_planning(amu_id())
        .unwrap()
        .expect("planning receipt");
    assert_eq!(planned.issuer.player_name.as_str(), "Ada");
    let player = PlayerId::try_from_bytes(uuid(1)).unwrap();
    assert!(
        state
            .install_companion_chat_plan(amu_id(), 1, follow_plan(player))
            .unwrap()
    );
    // The authoritative runtime agrees: generation and stored task facts are
    // set while the provider-owned attempt is untouched.
    let runtime = state
        .residents()
        .runtimes
        .get(&ActorKey::Companion(amu_id()))
        .expect("companion runtime")
        .clone();
    match &runtime.aux {
        ActorAux::Companion {
            generation,
            attempt,
            task,
            mining_target,
        } => {
            assert_eq!(*generation, 1);
            assert_eq!(*attempt, 7);
            assert_eq!(task.command.as_str(), "dig 0");
            assert_eq!(task.state, COMPANION_TASK_RUNNING);
            assert_eq!(task.plan_steps.len(), 1);
            assert!(mining_target.is_none());
        }
        _ => panic!("companion aux"),
    }

    // Fill the pending FIFO, queue one live action at the running
    // generation, then stop from a different session in the same window.
    for n in 1..17 {
        submit_chat(&mut state, sender, &format!("@阿木 dig {}", n));
    }
    state
        .submit_companion(chat_envelope(
            amu_id(),
            1,
            40,
            CompanionAction::Move {
                move_x: 1,
                move_z: 0,
                jump: false,
                yaw: 0.0,
            },
        ))
        .unwrap();
    submit_chat(&mut state, stopper, "@阿木 停止");
    let tick = state.advance_tick(TickBudget::full()).unwrap();

    // One started fact, sixteen admissions, one stopped fact: the stop
    // carries the original issuer Ada and the original command.
    let chats = broadcast_chats(&tick);
    assert_eq!(chats.len(), 18);
    let stopped = chats
        .iter()
        .find(|event| {
            matches!(
                event.body(),
                ChatBody::Task {
                    state: TaskState::Stopped,
                    ..
                }
            )
        })
        .expect("stopped broadcast");
    assert_eq!(
        stopped.player_id(),
        PlayerId::try_from_bytes(uuid(1)).unwrap()
    );
    assert_eq!(stopped.player_name().as_str(), "Ada");
    assert_eq!(
        stopped.body(),
        &ChatBody::Task {
            companion: CompanionSpeaker::new(amu_id(), amu_name()),
            command: CommandText::try_from_canonical("dig 0".to_owned()).unwrap(),
            state: TaskState::Stopped,
        }
    );
    let stopped_route = tick
        .events
        .iter()
        .find(|event| {
            matches!(event.event(), Event::Chat(event) if matches!(event.body(), ChatBody::Task { state: TaskState::Stopped, .. }))
        })
        .expect("stopped route");
    assert_eq!(stopped_route.recipient(), EventRecipient::Broadcast);
    assert!(chats.iter().any(|event| matches!(
        event.body(),
        ChatBody::Task {
            state: TaskState::Started,
            ..
        }
    )));

    // The full pending FIFO is preserved and its head is already queued with
    // a fresh generation in the same tick.
    let view = state.companion_chat_queue(amu_id()).unwrap();
    let current = view.current.as_ref().expect("promoted head");
    assert_eq!(current.generation, 2);
    assert_eq!(current.command.as_str(), "dig 1");
    assert_eq!(current.phase, CompanionChatPhase::Queued);
    assert_eq!(view.pending.len(), 15);
    assert_eq!(view.pending[0].0.as_str(), "dig 2");

    // Stop clears held controls, path, mining, the running task state, and
    // the already queued action: the companion never moves.
    let runtime = state
        .residents()
        .runtimes
        .get(&ActorKey::Companion(amu_id()))
        .expect("companion runtime")
        .clone();
    assert!(runtime.controls.is_none());
    assert!(runtime.path.is_none());
    match &runtime.aux {
        ActorAux::Companion {
            task,
            mining_target,
            ..
        } => {
            assert_eq!(task.state, COMPANION_TASK_STOPPED);
            assert!(mining_target.is_none());
        }
        _ => panic!("companion aux"),
    }
    assert!(
        !state
            .residents()
            .mining
            .contains_key(&ActorKey::Companion(amu_id()))
    );
    let residents = state.residents();
    let actor = residents
        .actors
        .iter()
        .find(|actor| actor.key == ActorKey::Companion(amu_id()))
        .expect("companion actor");
    assert_eq!(actor.motion.position().get()[0], 8.5);
    assert_eq!(actor.motion.position().get()[2], 8.5);

    // The delayed stale generation refuses; an unconfigured companion's
    // trusted action is unaffected.
    assert_eq!(
        state.submit_companion(chat_envelope(amu_id(), 1, 50, CompanionAction::MineRelease,)),
        Err(ServerError::InvalidInput {
            field: "companion_generation",
        })
    );
    state
        .submit_companion(chat_envelope(
            companion_id(50),
            1,
            60,
            CompanionAction::MineRelease,
        ))
        .unwrap();

    // Without the stop, the same live action executes: the control companion
    // moves, proving the stillness above comes from the purge.
    let mut control = authority();
    seed_world(&mut control);
    let sender = login(&mut control, 1, "Ada", [0.5, 65.0, 0.5], 0.0, 0.0);
    configure_amu(&mut control);
    stage_companion_with_runtime(&mut control, amu_id(), [8.5, 65.0, 8.5]);
    submit_chat(&mut control, sender, "@阿木 dig 0");
    let _ = control.advance_tick(TickBudget::full()).unwrap();
    control
        .take_companion_chat_planning(amu_id())
        .unwrap()
        .expect("planning receipt");
    control
        .install_companion_chat_plan(amu_id(), 1, follow_plan(player))
        .unwrap();
    control
        .submit_companion(chat_envelope(
            amu_id(),
            1,
            40,
            CompanionAction::Move {
                move_x: 1,
                move_z: 0,
                jump: false,
                yaw: 0.0,
            },
        ))
        .unwrap();
    let _ = control.advance_tick(TickBudget::full()).unwrap();
    let residents = control.residents();
    let actor = residents
        .actors
        .iter()
        .find(|actor| actor.key == ActorKey::Companion(amu_id()))
        .expect("companion actor");
    let position = actor.motion.position().get();
    assert!(
        (position[0] - 8.5).abs() > 1e-6 || (position[2] - 8.5).abs() > 1e-6,
        "the live action moves the companion without a stop"
    );
}

/// projection::chat_contract_terminal_finish_and_quota — terminal finish
/// needs the matching generation and phase (completed/timed-out only
/// running, failed from planning or running, never queued), refuses
/// non-terminal states before mutation, advances generations, honors the
/// four-fact lifecycle quota with refusal before mutation, and still
/// broadcasts for a retired original issuer.
#[test]
fn projection_chat_contract_terminal_finish_and_quota() {
    // Queued tasks never complete or fail here.
    let mut state = authority();
    seed_world(&mut state);
    let sender = login(&mut state, 1, "Ada", [0.5, 65.0, 0.5], 0.0, 0.0);
    configure_amu(&mut state);
    stage_companion_with_runtime(&mut state, amu_id(), [8.5, 65.0, 8.5]);
    submit_chat(&mut state, sender, "@阿木 dig 0");
    let _ = state.advance_tick(TickBudget::full()).unwrap();
    assert!(
        !state
            .finish_companion_chat_task(amu_id(), 1, TaskState::Completed)
            .unwrap()
    );
    assert!(
        !state
            .finish_companion_chat_task(
                amu_id(),
                1,
                TaskState::Failed(TaskFailure::PlannerUnavailable),
            )
            .unwrap()
    );
    let view = state.companion_chat_queue(amu_id()).unwrap();
    assert_eq!(
        view.current.as_ref().unwrap().phase,
        CompanionChatPhase::Queued
    );

    // Non-terminal states refuse before any mutation.
    state
        .take_companion_chat_planning(amu_id())
        .unwrap()
        .expect("planning receipt");
    let player = PlayerId::try_from_bytes(uuid(1)).unwrap();
    assert!(
        state
            .install_companion_chat_plan(amu_id(), 1, follow_plan(player))
            .unwrap()
    );
    for refused in [TaskState::Started, TaskState::Progress, TaskState::Stopped] {
        assert_eq!(
            state.finish_companion_chat_task(amu_id(), 1, refused),
            Err(ServerError::InvalidInput {
                field: "companion_chat_state",
            })
        );
    }
    let view = state.companion_chat_queue(amu_id()).unwrap();
    assert_eq!(
        view.current.as_ref().unwrap().phase,
        CompanionChatPhase::Running
    );

    // A running completion broadcasts the terminal fact and the next command
    // advances the generation.
    assert!(
        state
            .finish_companion_chat_task(amu_id(), 1, TaskState::Completed)
            .unwrap()
    );
    submit_chat(&mut state, sender, "@阿木 second");
    let tick = state.advance_tick(TickBudget::full()).unwrap();
    let terminal = broadcast_chats(&tick)
        .into_iter()
        .find(|event| {
            matches!(
                event.body(),
                ChatBody::Task {
                    state: TaskState::Completed,
                    ..
                }
            )
        })
        .expect("completed broadcast");
    assert_eq!(
        terminal.body(),
        &ChatBody::Task {
            companion: CompanionSpeaker::new(amu_id(), amu_name()),
            command: CommandText::try_from_canonical("dig 0".to_owned()).unwrap(),
            state: TaskState::Completed,
        }
    );
    let view = state.companion_chat_queue(amu_id()).unwrap();
    let current = view.current.as_ref().expect("next generation");
    assert_eq!(current.generation, 2);
    assert_eq!(current.command.as_str(), "second");

    // A timed-out finish needs the running second generation.
    state
        .take_companion_chat_planning(amu_id())
        .unwrap()
        .expect("planning receipt");
    assert!(
        state
            .install_companion_chat_plan(amu_id(), 2, follow_plan(player))
            .unwrap()
    );
    assert!(
        state
            .finish_companion_chat_task(amu_id(), 2, TaskState::TimedOut)
            .unwrap()
    );
    let tick = state.advance_tick(TickBudget::full()).unwrap();
    assert!(broadcast_chats(&tick).iter().any(|event| matches!(
        event.body(),
        ChatBody::Task {
            state: TaskState::TimedOut,
            ..
        }
    )));

    // Failed finishes a planning task; timed-out finishes a running one;
    // wrong generations and unconfigured ids stay false without mutation.
    let mut state = authority();
    seed_world(&mut state);
    let sender = login(&mut state, 1, "Ada", [0.5, 65.0, 0.5], 0.0, 0.0);
    configure_amu(&mut state);
    stage_companion_with_runtime(&mut state, amu_id(), [8.5, 65.0, 8.5]);
    submit_chat(&mut state, sender, "@阿木 dig 0");
    let _ = state.advance_tick(TickBudget::full()).unwrap();
    state
        .take_companion_chat_planning(amu_id())
        .unwrap()
        .expect("planning receipt");
    assert!(
        state
            .finish_companion_chat_task(
                amu_id(),
                1,
                TaskState::Failed(TaskFailure::PathUnreachable),
            )
            .unwrap()
    );
    assert!(
        state
            .companion_chat_queue(amu_id())
            .unwrap()
            .current
            .is_none()
    );
    assert!(
        !state
            .finish_companion_chat_task(amu_id(), 9, TaskState::Completed)
            .unwrap()
    );
    assert!(
        !state
            .finish_companion_chat_task(companion_id(50), 1, TaskState::Completed)
            .unwrap()
    );

    // Four lifecycle facts fit between drains; the fifth refuses before any
    // mutation while the running task is preserved.
    let mut state = authority();
    seed_world(&mut state);
    let sender = login(&mut state, 1, "Ada", [0.5, 65.0, 0.5], 0.0, 0.0);
    let pairs = [
        (amu_id(), amu_name()),
        second_pair(),
        (
            companion_id(11),
            CompanionName::try_from_canonical("阿土".to_owned()).unwrap(),
        ),
        (
            companion_id(12),
            CompanionName::try_from_canonical("阿水".to_owned()).unwrap(),
        ),
    ];
    state.configure_companion_chat(&pairs).unwrap();
    for (id, position) in [
        (amu_id(), [8.5, 65.0, 8.5]),
        (companion_id(10), [9.5, 65.0, 9.5]),
        (companion_id(11), [10.5, 65.0, 10.5]),
        (companion_id(12), [11.5, 65.0, 11.5]),
    ] {
        stage_companion_with_runtime(&mut state, id, position);
    }
    submit_chat(&mut state, sender, "@阿木 one");
    submit_chat(&mut state, sender, "@阿火 two");
    submit_chat(&mut state, sender, "@阿土 three");
    submit_chat(&mut state, sender, "@阿水 four");
    let _ = state.advance_tick(TickBudget::full()).unwrap();
    for id in [
        amu_id(),
        companion_id(10),
        companion_id(11),
        companion_id(12),
    ] {
        state
            .take_companion_chat_planning(id)
            .unwrap()
            .expect("planning receipt");
        assert!(
            state
                .install_companion_chat_plan(id, 1, follow_plan(player))
                .unwrap()
        );
    }
    assert_eq!(
        state.finish_companion_chat_task(amu_id(), 1, TaskState::Completed),
        Err(ServerError::Capacity {
            resource: Resource::Commands,
            limit: 4,
            observed: 5,
        })
    );
    let view = state.companion_chat_queue(amu_id()).unwrap();
    assert_eq!(
        view.current.as_ref().unwrap().phase,
        CompanionChatPhase::Running
    );

    // A retired original issuer still finishes with its original broadcast.
    let mut state = authority();
    seed_world(&mut state);
    let sender = login(&mut state, 1, "Ada", [0.5, 65.0, 0.5], 0.0, 0.0);
    let _ = login(&mut state, 2, "Ben", [4.5, 65.0, 4.5], 0.0, 0.0);
    configure_amu(&mut state);
    stage_companion_with_runtime(&mut state, amu_id(), [8.5, 65.0, 8.5]);
    submit_chat(&mut state, sender, "@阿木 dig 0");
    let _ = state.advance_tick(TickBudget::full()).unwrap();
    state
        .take_companion_chat_planning(amu_id())
        .unwrap()
        .expect("planning receipt");
    state
        .install_companion_chat_plan(amu_id(), 1, follow_plan(player))
        .unwrap();
    state.retire(sender, CloseReason::PeerGone).unwrap();
    assert!(
        state
            .finish_companion_chat_task(amu_id(), 1, TaskState::TimedOut)
            .unwrap()
    );
    let tick = state.advance_tick(TickBudget::full()).unwrap();
    let terminal = broadcast_chats(&tick)
        .into_iter()
        .find(|event| {
            matches!(
                event.body(),
                ChatBody::Task {
                    state: TaskState::TimedOut,
                    ..
                }
            )
        })
        .expect("timed-out broadcast");
    assert_eq!(
        terminal.player_id(),
        PlayerId::try_from_bytes(uuid(1)).unwrap()
    );
    assert_eq!(terminal.player_name().as_str(), "Ada");
}

/// projection::chat_contract_stored_failure_reasons — each closed failure
/// maps to its stored task state and reason on the authoritative runtime
/// while the original command and issuer broadcast; a companion that goes
/// quiet after the planning take still terminates without an active actor.
#[test]
fn projection_chat_contract_stored_failure_reasons() {
    let cases = [
        (
            TaskFailure::PlannerUnavailable,
            COMPANION_TASK_FAIL_PLANNER_UNAVAILABLE,
        ),
        (TaskFailure::InvalidPlan, COMPANION_TASK_FAIL_INVALID_PLAN),
        (
            TaskFailure::PathUnreachable,
            COMPANION_TASK_FAIL_PATH_UNREACHABLE,
        ),
        (TaskFailure::WorldChanged, COMPANION_TASK_FAIL_WORLD_CHANGED),
        (
            TaskFailure::InventoryFull,
            COMPANION_TASK_FAIL_INVENTORY_FULL,
        ),
    ];
    for (failure, reason) in cases {
        let mut state = authority();
        seed_world(&mut state);
        let sender = login(&mut state, 1, "Ada", [0.5, 65.0, 0.5], 0.0, 0.0);
        configure_amu(&mut state);
        stage_companion_with_runtime(&mut state, amu_id(), [8.5, 65.0, 8.5]);
        submit_chat(&mut state, sender, "@阿木 dig 0");
        let _ = state.advance_tick(TickBudget::full()).unwrap();
        state
            .take_companion_chat_planning(amu_id())
            .unwrap()
            .expect("planning receipt");
        assert!(
            state
                .finish_companion_chat_task(amu_id(), 1, TaskState::Failed(failure))
                .unwrap()
        );
        let residents = state.residents();
        let runtime = residents
            .runtimes
            .get(&ActorKey::Companion(amu_id()))
            .expect("companion runtime");
        match &runtime.aux {
            ActorAux::Companion { task, .. } => {
                assert_eq!(task.state, COMPANION_TASK_FAILED, "failure {failure:?}");
                assert_eq!(task.fail_reason, reason, "failure {failure:?}");
            }
            _ => panic!("companion aux"),
        }
        let tick = state.advance_tick(TickBudget::full()).unwrap();
        let failed = broadcast_chats(&tick)
            .into_iter()
            .find(|event| {
                matches!(
                    event.body(),
                    ChatBody::Task {
                        state: TaskState::Failed(_),
                        ..
                    }
                )
            })
            .expect("failed broadcast");
        assert_eq!(
            failed.body(),
            &ChatBody::Task {
                companion: CompanionSpeaker::new(amu_id(), amu_name()),
                command: CommandText::try_from_canonical("dig 0".to_owned()).unwrap(),
                state: TaskState::Failed(failure),
            }
        );
        assert_eq!(
            failed.player_id(),
            PlayerId::try_from_bytes(uuid(1)).unwrap()
        );
        assert_eq!(failed.player_name().as_str(), "Ada");
    }

    // A companion that goes quiet after the planning take still terminates:
    // the original fact emits and the present runtime is cleaned.
    let mut state = authority();
    seed_world(&mut state);
    let sender = login(&mut state, 1, "Ada", [0.5, 65.0, 0.5], 0.0, 0.0);
    configure_amu(&mut state);
    stage_companion_with_runtime(&mut state, amu_id(), [8.5, 65.0, 8.5]);
    submit_chat(&mut state, sender, "@阿木 dig 0");
    let _ = state.advance_tick(TickBudget::full()).unwrap();
    state
        .take_companion_chat_planning(amu_id())
        .unwrap()
        .expect("planning receipt");
    let quiet = {
        let residents = state.residents();
        let mut actor = residents
            .actors
            .iter()
            .find(|actor| actor.key == ActorKey::Companion(amu_id()))
            .expect("companion actor")
            .clone();
        actor.lifecycle = ActorLifecycle::Pending;
        actor
    };
    stage(&mut state, |context| {
        context.stage(RuleEffect::Actor(quiet)).unwrap();
    });
    assert!(
        state
            .finish_companion_chat_task(
                amu_id(),
                1,
                TaskState::Failed(TaskFailure::PlannerUnavailable),
            )
            .unwrap()
    );
    assert!(
        state
            .companion_chat_queue(amu_id())
            .unwrap()
            .current
            .is_none()
    );
    let tick = state.advance_tick(TickBudget::full()).unwrap();
    let failed = broadcast_chats(&tick)
        .into_iter()
        .find(|event| {
            matches!(
                event.body(),
                ChatBody::Task {
                    state: TaskState::Failed(_),
                    ..
                }
            )
        })
        .expect("failed broadcast without an active actor");
    assert_eq!(
        failed.body(),
        &ChatBody::Task {
            companion: CompanionSpeaker::new(amu_id(), amu_name()),
            command: CommandText::try_from_canonical("dig 0".to_owned()).unwrap(),
            state: TaskState::Failed(TaskFailure::PlannerUnavailable),
        }
    );
    assert_eq!(failed.player_name().as_str(), "Ada");
}

/// projection::chat_contract_issuer_capture_and_ray — the issuer pose, look,
/// and look hit are captured before same-tick movement and retained by the
/// planning receipt across movement and retirement; the native Ready ray is
/// fixed-3x3 around the foot independent of wanted, skips unloaded and
/// out-of-square cells by continuing, targets the context-free upper door
/// even above an open lower, and skips air, fluids, and open doors.
#[test]
fn projection_chat_contract_issuer_capture_and_ray() {
    // Capture precedes same-tick movement; the receipt never rereads. A
    // chat-free priming tick commits the player actor first, since the login
    // install only materializes the actor during reduction.
    let mut state = authority();
    seed_world(&mut state);
    let sender = login(&mut state, 1, "Ada", [0.5, 65.0, 0.5], 0.0, 0.0);
    configure_amu(&mut state);
    let _ = state.advance_tick(TickBudget::full()).unwrap();
    submit_chat(&mut state, sender, "@阿木 collect wood");
    submit(
        &mut state,
        sender,
        1,
        Command::PlayerInput(PlayerControl::new(PlayerControlParts {
            movement: Movement {
                move_x: 1,
                move_z: 0,
                jump: false,
            },
            look: LookAngles::try_new(0.0, 0.0).unwrap(),
            actions: HeldActions {
                primary: false,
                eating: false,
                sprinting: false,
                sneaking: false,
            },
        })),
    );
    let _ = state.advance_tick(TickBudget::full()).unwrap();
    let issuer = state
        .companion_chat_queue(amu_id())
        .unwrap()
        .current
        .as_ref()
        .expect("current")
        .issuer
        .clone();
    assert_eq!(issuer.session, sender);
    assert_eq!(issuer.player_id, PlayerId::try_from_bytes(uuid(1)).unwrap());
    assert_eq!(issuer.player_name.as_str(), "Ada");
    assert_eq!(issuer.position.get(), [0.5, 65.0, 0.5]);
    assert_eq!((issuer.look.yaw(), issuer.look.pitch()), (0.0, 0.0));
    // The same tick moved the player through held-control ingress while the
    // captured facts stayed at the pre-move pose.
    let residents = state.residents();
    let actor = residents
        .actors
        .iter()
        .find(|actor| actor.key == ActorKey::Player(sender))
        .expect("player actor");
    let stepped = actor.motion.position().get();
    assert!(
        (stepped[0] - 0.5).abs() > 1e-6 || (stepped[2] - 0.5).abs() > 1e-6,
        "held control moves the player in the capture tick"
    );
    // The planning take needs the live companion actor; capture already ran.
    stage(&mut state, |context| {
        context
            .stage(RuleEffect::Actor(companion_actor(
                amu_id(),
                [8.5, 65.0, 8.5],
                0.0,
            )))
            .unwrap();
    });
    let planned = state
        .take_companion_chat_planning(amu_id())
        .unwrap()
        .expect("planning receipt");
    assert_eq!(planned.issuer.position.get(), [0.5, 65.0, 0.5]);
    assert_eq!(planned.source_tick, 1);
    // A committed overwrite on the next tick plus the issuer's retirement
    // still cannot rewrite the owned receipt.
    let moved = {
        let residents = state.residents();
        let mut actor = residents
            .actors
            .iter()
            .find(|actor| actor.key == ActorKey::Player(sender))
            .expect("player actor")
            .clone();
        actor.motion = MotionState::new(MotionStateParts {
            position: FiniteVec3::try_new([10.5, 65.0, 10.5]).unwrap(),
            velocity: FiniteVec3::try_new([0.0; 3]).unwrap(),
            on_ground: true,
        });
        actor
    };
    stage(&mut state, |context| {
        context.stage(RuleEffect::Actor(moved)).unwrap();
    });
    let _ = state.advance_tick(TickBudget::full()).unwrap();
    state.retire(sender, CloseReason::PeerGone).unwrap();
    assert_eq!(planned.issuer.position.get(), [0.5, 65.0, 0.5]);
    assert_eq!(planned.issuer.player_name.as_str(), "Ada");

    // With no committed player actor the capture falls back to the source
    // pose with a zero look and no hit.
    let mut state = authority();
    seed_world(&mut state);
    let sender = login(&mut state, 1, "Ada", [0.5, 65.0, 0.5], 0.0, 0.0);
    configure_amu(&mut state);
    submit_chat(&mut state, sender, "@阿木 collect wood");
    let _ = state.advance_tick(TickBudget::full()).unwrap();
    let issuer = state
        .companion_chat_queue(amu_id())
        .unwrap()
        .current
        .as_ref()
        .expect("current")
        .issuer
        .clone();
    assert_eq!(issuer.position.get(), [0.0, 1.0, 0.0]);
    assert_eq!((issuer.look.yaw(), issuer.look.pitch()), (0.0, 0.0));
    assert_eq!(issuer.look_hit, None);

    // A solid cell in the look path hits, with no wanted dependence: the
    // publication radius is zero yet the Ready cell still targets.
    let mut state = authority_with_view_radius(0);
    seed_world(&mut state);
    let sender = login(&mut state, 1, "Ada", [0.5, 65.0, 0.5], 0.0, 0.0);
    configure_amu(&mut state);
    let mut solid = ground_chunk();
    set_cell(&mut solid, BlockPos::new(0, 66, -1), GRASS);
    stage(&mut state, |context| {
        context.preload_ready_chunk(ReadyChunk::try_new(chunk_key(0, -1), 1, 1, solid).unwrap());
    });
    // Prime the player actor before capture; the radius stays zero.
    let _ = state.advance_tick(TickBudget::full()).unwrap();
    submit_chat(&mut state, sender, "@阿木 collect wood");
    let _ = state.advance_tick(TickBudget::full()).unwrap();
    let hit = state
        .companion_chat_queue(amu_id())
        .unwrap()
        .current
        .as_ref()
        .expect("current")
        .issuer
        .look_hit;
    assert_eq!(hit, Some(BlockPos::new(0, 66, -1)));

    // The same geometry without the Ready chunk yields no hit.
    let mut state = authority();
    seed_world(&mut state);
    let sender = login(&mut state, 1, "Ada", [0.5, 65.0, 0.5], 0.0, 0.0);
    configure_amu(&mut state);
    let _ = state.advance_tick(TickBudget::full()).unwrap();
    submit_chat(&mut state, sender, "@阿木 collect wood");
    let _ = state.advance_tick(TickBudget::full()).unwrap();
    let hit = state
        .companion_chat_queue(amu_id())
        .unwrap()
        .current
        .as_ref()
        .expect("current")
        .issuer
        .look_hit;
    assert_eq!(hit, None);

    // A diagonal ray continues past unready corner cells to the Ready
    // diagonal target inside the fixed 3x3.
    let mut state = authority();
    seed_world(&mut state);
    let sender = login(&mut state, 1, "Ada", [15.5, 65.0, 15.5], -2.3561945, 0.0);
    configure_amu(&mut state);
    let mut diagonal = ground_chunk();
    set_cell(&mut diagonal, BlockPos::new(17, 66, 17), GRASS);
    stage(&mut state, |context| {
        context.preload_ready_chunk(ReadyChunk::try_new(chunk_key(1, 1), 1, 1, diagonal).unwrap());
    });
    let _ = state.advance_tick(TickBudget::full()).unwrap();
    submit_chat(&mut state, sender, "@阿木 collect wood");
    let _ = state.advance_tick(TickBudget::full()).unwrap();
    let hit = state
        .companion_chat_queue(amu_id())
        .unwrap()
        .current
        .as_ref()
        .expect("current")
        .issuer
        .look_hit;
    assert_eq!(hit, Some(BlockPos::new(17, 66, 17)));

    // Outside the fixed 3x3 there is no hit even when the reach covers
    // the Ready target: the square bound decides, not the reach.
    let mut state = authority();
    seed_world(&mut state);
    let sender = login(&mut state, 1, "Ada", [0.5, 65.0, 0.5], -1.5707964, 0.0);
    configure_amu(&mut state);
    let mut far = ground_chunk();
    set_cell(&mut far, BlockPos::new(33, 66, 0), GRASS);
    stage(&mut state, |context| {
        context.preload_ready_chunk(ReadyChunk::try_new(chunk_key(2, 0), 1, 1, far).unwrap());
    });
    let _ = state.advance_tick(TickBudget::full()).unwrap();
    // Source-default tunables with only the reach extended to forty blocks:
    // staged after priming so the capture tick reads it before the freeze
    // restores defaults.
    let base = RuleTunables::source_defaults();
    let far_reach = RuleTunables::try_new(
        base.physics(),
        base.regen_delay_ticks(),
        base.regen_interval_ticks(),
        base.drown_interval_ticks(),
        base.starvation_interval_ticks(),
        base.regen_hunger_threshold(),
        base.exhaustion_threshold_milli(),
        base.eating_ticks(),
        base.furnace_burn_ticks(),
        base.furnace_smelt_ticks(),
        base.fluid_delay(),
        base.random_attempts(),
        base.crop_growth_percent(),
        40.0,
        base.eye_height(),
        base.drop_pickup_delay_ticks(),
        base.player_drop_pickup_delay_ticks(),
        base.drop_lifetime_ticks(),
        base.drop_pickup_range(),
    )
    .unwrap();
    stage(&mut state, |context| {
        let mut env = environment(1_000);
        env.tunables = far_reach;
        context.stage(RuleEffect::Environment(env)).unwrap();
    });
    submit_chat(&mut state, sender, "@阿木 collect wood");
    let _ = state.advance_tick(TickBudget::full()).unwrap();
    let hit = state
        .companion_chat_queue(amu_id())
        .unwrap()
        .current
        .as_ref()
        .expect("current")
        .issuer
        .look_hit;
    assert_eq!(hit, None);

    // The upper door targets even above an open lower; open doors, fluids,
    // and plain air do not.
    for (target, lower, expected) in [
        (70u16, 63u16, Some(BlockPos::new(0, 66, -1))),
        (63u16, 0u16, None),
        (30u16, 0u16, None),
    ] {
        let mut state = authority();
        seed_world(&mut state);
        let sender = login(&mut state, 1, "Ada", [0.5, 65.0, 0.5], 0.0, 0.0);
        configure_amu(&mut state);
        let mut cells = ground_chunk();
        set_cell(&mut cells, BlockPos::new(0, 66, -1), target);
        if lower != 0 {
            set_cell(&mut cells, BlockPos::new(0, 65, -1), lower);
        }
        stage(&mut state, |context| {
            context
                .preload_ready_chunk(ReadyChunk::try_new(chunk_key(0, -1), 1, 1, cells).unwrap());
        });
        let _ = state.advance_tick(TickBudget::full()).unwrap();
        submit_chat(&mut state, sender, "@阿木 collect wood");
        let _ = state.advance_tick(TickBudget::full()).unwrap();
        let hit = state
            .companion_chat_queue(amu_id())
            .unwrap()
            .current
            .as_ref()
            .expect("current")
            .issuer
            .look_hit;
        assert_eq!(hit, expected, "target cell {}", target);
    }
}

/// projection::chat_accepted_broadcast_and_rejects_sender_only — malformed
/// and unknown addressing reject to the sender alone, a valid command
/// broadcasts with strictly increasing event ids, the task FIFO holds one
/// current task plus sixteen pending commands before the next instruction
/// rejects queue-full, and chat never enqueues a sequenced command.
#[test]
fn projection_chat_accepted_broadcast_and_rejects_sender_only() {
    let mut state = authority();
    seed_world(&mut state);
    let sender = login(&mut state, 1, "Ada", [0.5, 65.0, 0.5], 0.0, 0.0);
    let listener = login(&mut state, 2, "Ben", [4.5, 65.0, 4.5], 0.0, 0.0);
    let id = companion_id(9);
    let name = mornlea_domain::CompanionName::try_from_canonical(derived_name(id)).unwrap();
    // Fixture migration: the derived companion name is explicitly registered
    // before the first tick. There is no production auto-registration.
    state
        .configure_companion_chat(&[(id, name.clone())])
        .unwrap();
    stage(&mut state, |context| {
        context
            .stage(RuleEffect::Actor(companion_actor(
                id,
                [8.5, 65.0, 8.5],
                0.0,
            )))
            .unwrap();
        context.preload_drop(drop_record(3, 5_997, [0.5, 65.5, 2.5]));
    });
    submit_chat(&mut state, sender, "hello");
    submit_chat(&mut state, sender, "@nobody dig");
    submit_chat(
        &mut state,
        sender,
        &format!("@{} mine stone", name.as_str()),
    );
    let tick_a = state.advance_tick(TickBudget::full()).unwrap();
    assert_eq!(tick_a.counters.commands, 0, "chat enqueues no commands");
    let sender_events = events_for(&tick_a, sender);
    let malformed = ChatEvent::try_new(ChatEventParts {
        event_id: 1,
        player_id: PlayerId::try_from_bytes(uuid(1)).unwrap(),
        player_name: DisplayName::try_from_canonical("Ada".to_owned()).unwrap(),
        body: ChatBody::InvalidFormat,
    })
    .unwrap();
    assert_eq!(
        find_event(&sender_events, |event| matches!(
            event,
            Event::Chat(chat_event) if chat_event.body() == &ChatBody::InvalidFormat
        )),
        Some(&Event::Chat(malformed)),
        "the malformed chat rejects to the sender only"
    );
    let unknown = ChatEvent::try_new(ChatEventParts {
        event_id: 2,
        player_id: PlayerId::try_from_bytes(uuid(1)).unwrap(),
        player_name: DisplayName::try_from_canonical("Ada".to_owned()).unwrap(),
        body: ChatBody::UnknownCompanion {
            name: mornlea_domain::CompanionName::try_from_canonical("nobody".to_owned()).unwrap(),
        },
    })
    .unwrap();
    assert_eq!(
        find_event(&sender_events, |event| matches!(
            event,
            Event::Chat(chat_event) if matches!(chat_event.body(), ChatBody::UnknownCompanion { .. })
        )),
        Some(&Event::Chat(unknown))
    );
    let accepted = ChatEvent::try_new(ChatEventParts {
        event_id: 3,
        player_id: PlayerId::try_from_bytes(uuid(1)).unwrap(),
        player_name: DisplayName::try_from_canonical("Ada".to_owned()).unwrap(),
        body: ChatBody::Accepted {
            companion: CompanionSpeaker::new(id, name.clone()),
            command: CommandText::try_from_canonical("mine stone".to_owned()).unwrap(),
        },
    })
    .unwrap();
    assert_eq!(
        tick_a
            .events
            .iter()
            .filter(|event| matches!(event.event(), Event::Chat(_)))
            .count(),
        3
    );
    let broadcast = tick_a
        .events
        .iter()
        .find(|event| matches!(event.event(), Event::Chat(event) if *event == accepted))
        .unwrap();
    assert_eq!(broadcast.recipient(), EventRecipient::Broadcast);
    let listener_events = events_for(&tick_a, listener);
    assert!(
        !listener_events
            .iter()
            .any(|event| matches!(event, Event::Chat(_)))
    );
    // The chat family follows the mob and drop families and precedes the
    // private player observation.
    let chat_index = tick_a
        .events
        .iter()
        .position(|event| matches!(event.event(), Event::Chat(_)))
        .unwrap();
    let drop_index = tick_a
        .events
        .iter()
        .position(|event| {
            matches!(
                event.event(),
                Event::ItemDropUpserts(_) | Event::ItemDropRemoves(_)
            )
        })
        .unwrap();
    let player_index = tick_a
        .events
        .iter()
        .position(|event| matches!(event.event(), Event::PlayerState(_)))
        .unwrap();
    assert!(drop_index < chat_index && chat_index < player_index);
    // The task FIFO ceiling: one current task plus sixteen pending commands
    // are accepted, and the eighteenth instruction rejects queue-full to the
    // sender only while the ids keep increasing.
    for _ in 0..17 {
        submit_chat(&mut state, sender, &format!("@{} dig", name.as_str()));
    }
    let tick_b = state.advance_tick(TickBudget::full()).unwrap();
    assert_eq!(tick_b.counters.commands, 0);
    let chats: Vec<&mornlea_domain::ChatEvent> = tick_b
        .events
        .iter()
        .filter_map(|event| match event.event() {
            Event::Chat(event) => Some(event),
            _ => None,
        })
        .collect();
    assert_eq!(chats.len(), 17);
    for pair in chats.windows(2) {
        assert!(pair[0].event_id() < pair[1].event_id());
    }
    let full = chats.last().unwrap();
    assert_eq!(
        full.body(),
        &ChatBody::QueueFull {
            companion: CompanionSpeaker::new(id, name.clone()),
            command: CommandText::try_from_canonical("dig".to_owned()).unwrap(),
        }
    );
    assert_eq!(
        tick_b
            .events
            .iter()
            .find(|event| matches!(event.event(), Event::Chat(chat) if chat.event_id() == full.event_id()))
            .unwrap()
            .recipient(),
        EventRecipient::Session(sender.get())
    );
    assert_eq!(chats[0].event_id(), 4, "ids continue across ticks");
}

/// projection::record_states_inventory_chest_container_crafting_furnace —
/// real commands publish the exact owner-only record states, a close
/// publishes the container-closed notice, and a refused command publishes
/// nothing.
#[test]
fn projection_record_states_inventory_chest_container_crafting_furnace() {
    let mut state = authority();
    // The world: standing ground, one chest in front, one furnace behind.
    stage(&mut state, |context| {
        context
            .stage(RuleEffect::Environment(environment(1_000)))
            .unwrap();
        context.preload_ready_chunk(
            ReadyChunk::try_new(chunk_key(0, 0), 1, 1, ground_chunk()).unwrap(),
        );
        let mut front = ground_chunk();
        let chest_ref = chest_in_chunk(
            &mut front,
            BlockPos::new(0, 66, -1),
            [StorageStack::default(); 27],
        );
        context.preload_ready_chunk(ReadyChunk::try_new(chunk_key(0, -1), 1, 1, front).unwrap());
        let _chest_ref = chest_ref;
        let mut home = ground_chunk();
        let furnace_ref = furnace_in_chunk(
            &mut home,
            BlockPos::new(0, 66, 1),
            storage(ITEM_RAW_IRON, 2),
            storage(ITEM_COAL, 2),
            StorageStack::default(),
            0,
            0,
        );
        context.preload_ready_chunk(ReadyChunk::try_new(chunk_key(0, 0), 1, 1, home).unwrap());
        let _furnace_ref = furnace_ref;
    });
    let owner = login_with(
        &mut state,
        1,
        "Ada",
        [0.5, 65.0, 0.5],
        0.0,
        0.0,
        |player: &mut StoredPlayer| {
            player.inventory.backpack[0] = storage(ITEM_DIRT, 5);
        },
    );
    let other = login(&mut state, 2, "Ben", [4.5, 65.0, 4.5], 0.0, 0.0);
    let _ = state.advance_tick(TickBudget::full()).unwrap();

    // A hotbar selection publishes the exact owner-only inventory state.
    submit(
        &mut state,
        owner,
        1,
        Command::SelectHotbar(HotbarSlot::new(2).unwrap()),
    );
    // A refused drop (no lease held) leaves the other session unchanged.
    submit(
        &mut state,
        other,
        1,
        Command::DropStack(
            StackSource::try_new(
                StackView::Container(
                    ContainerRef::try_new(ChunkPos::new(0, -1), ContainerKind::Chest, 0, 1)
                        .unwrap(),
                ),
                36,
            )
            .unwrap(),
        ),
    );
    let tick_one = state.advance_tick(TickBudget::full()).unwrap();
    let expected = InventoryState::new(InventoryStateParts {
        selected: HotbarSlot::new(2).unwrap(),
        hotbar: item_array(&[]),
        backpack: item_array(&[(0, ITEM_DIRT, 5)]),
    });
    assert_eq!(
        find_event(&events_for(&tick_one, owner), |event| matches!(
            event,
            Event::InventoryState(_)
        )),
        Some(&Event::InventoryState(expected)),
        "the selection publishes the exact inventory to the owner"
    );
    assert!(
        !events_for(&tick_one, other)
            .iter()
            .any(|event| matches!(event, Event::InventoryState(_))),
        "the refused command and the foreign session publish nothing"
    );

    // Opening the chest publishes its exact (empty) contents.
    submit(&mut state, owner, 2, Command::OpenContainer(look(0.0, 0.0)));
    let tick_two = state.advance_tick(TickBudget::full()).unwrap();
    let chest_ref =
        ContainerRef::try_new(ChunkPos::new(0, -1), ContainerKind::Chest, 0, 1).unwrap();
    let expected = mornlea_domain::ChestState::try_new(mornlea_domain::ChestStateParts {
        container: chest_ref,
        items: item_array(&[]),
    })
    .unwrap();
    assert_eq!(
        find_event(&events_for(&tick_two, owner), |event| matches!(
            event,
            Event::ChestState(_)
        )),
        Some(&Event::ChestState(expected))
    );

    // One partial move publishes the moved chest contents.
    submit(
        &mut state,
        owner,
        3,
        Command::MovePartial(
            PartialMove::try_new(StackView::Container(chest_ref), 9, 36, true).unwrap(),
        ),
    );
    let tick_three = state.advance_tick(TickBudget::full()).unwrap();
    let expected = mornlea_domain::ChestState::try_new(mornlea_domain::ChestStateParts {
        container: chest_ref,
        items: item_array(&[(0, ITEM_DIRT, 1)]),
    })
    .unwrap();
    assert_eq!(
        find_event(&events_for(&tick_three, owner), |event| matches!(
            event,
            Event::ChestState(_)
        )),
        Some(&Event::ChestState(expected))
    );

    // Closing publishes the exact released reference, and a crafting grid
    // staged in the same window publishes after the close notice.
    submit(&mut state, owner, 4, Command::CloseContainer);
    stage(&mut state, |context| {
        let actor = ActorKey::Player(owner);
        let mut record = context.read().inventory(actor).cloned().unwrap();
        record.crafting = storage_array(&[
            (0, ITEM_STONE, 1),
            (1, ITEM_STICK, 1),
            (2, ITEM_STONE, 1),
            (3, ITEM_STICK, 1),
        ]);
        context.preload_inventory(actor, record);
    });
    let tick_four = state.advance_tick(TickBudget::full()).unwrap();
    let tick_four_events = events_for(&tick_four, owner);
    assert_eq!(
        find_event(&tick_four_events, |event| matches!(
            event,
            Event::ContainerClosed(_)
        )),
        Some(&Event::ContainerClosed(
            mornlea_domain::ContainerClosed::new(chest_ref)
        ))
    );
    let closed_at = tick_four_events
        .iter()
        .position(|event| matches!(event, Event::ContainerClosed(_)))
        .unwrap();
    let crafting_at = tick_four_events
        .iter()
        .position(|event| matches!(event, Event::CraftingState(_)))
        .unwrap();
    assert!(
        closed_at < crafting_at,
        "the close notice precedes the crafting family"
    );

    // The staged personal grid is the two-by-two stone-hoe pattern
    // (`RecipeStoneHoe` in the frozen registry): stone and stick in both
    // rows, whose exact matched output is one durable stone hoe.
    let expected = CraftingState::try_new(CraftingStateParts {
        size: CraftingSize::Personal,
        slots: item_array(&[
            (0, ITEM_STONE, 1),
            (1, ITEM_STICK, 1),
            (2, ITEM_STONE, 1),
            (3, ITEM_STICK, 1),
        ]),
        output: ItemStack::try_new(ITEM_STONE_HOE, 1, 131).unwrap(),
    })
    .unwrap();
    assert_eq!(
        find_event(&tick_four_events, |event| matches!(
            event,
            Event::CraftingState(_)
        )),
        Some(&Event::CraftingState(expected))
    );
    // The atomic output take empties the grid in the same tick the credited
    // hoe changes the pack.
    submit(&mut state, owner, 5, Command::TakeCraftingOutput);
    let tick_five = state.advance_tick(TickBudget::full()).unwrap();
    let expected = CraftingState::try_new(CraftingStateParts {
        size: CraftingSize::Personal,
        slots: item_array(&[]),
        output: ItemStack::EMPTY,
    })
    .unwrap();
    assert_eq!(
        find_event(&events_for(&tick_five, owner), |event| matches!(
            event,
            Event::CraftingState(_)
        )),
        Some(&Event::CraftingState(expected))
    );

    // Opening the furnace publishes its burning body exactly.
    submit(
        &mut state,
        owner,
        6,
        Command::OpenContainer(look(std::f32::consts::PI, 0.0)),
    );
    let tick_six = state.advance_tick(TickBudget::full()).unwrap();
    let furnace_ref =
        ContainerRef::try_new(ChunkPos::new(0, 0), ContainerKind::Furnace, 1, 1).unwrap();
    let expected = mornlea_domain::FurnaceState::try_new(mornlea_domain::FurnaceStateParts {
        container: furnace_ref,
        input: stack(ITEM_RAW_IRON, 2),
        fuel: stack(ITEM_COAL, 1),
        output: ItemStack::EMPTY,
        // Ignition happened on tick 0; six later ticks advanced the timers.
        progress_ticks: 7,
        burn_ticks: 1_600 - 7,
    })
    .unwrap();
    assert_eq!(
        find_event(&events_for(&tick_six, owner), |event| matches!(
            event,
            Event::FurnaceState(_)
        )),
        Some(&Event::FurnaceState(expected))
    );

    // One more grid change in the same window as the burning furnace pins the
    // last leg of the record order: the crafting family precedes the furnace
    // family. The lone stone matches no recipe, so the output slot is empty.
    stage(&mut state, |context| {
        let actor = ActorKey::Player(owner);
        let mut record = context.read().inventory(actor).cloned().unwrap();
        record.crafting = storage_array(&[(0, ITEM_STONE, 1)]);
        context.preload_inventory(actor, record);
    });
    let tick_seven = state.advance_tick(TickBudget::full()).unwrap();
    let tick_seven_events = events_for(&tick_seven, owner);
    let crafting_at = tick_seven_events
        .iter()
        .position(|event| matches!(event, Event::CraftingState(_)))
        .unwrap();
    let furnace_at = tick_seven_events
        .iter()
        .position(|event| matches!(event, Event::FurnaceState(_)))
        .unwrap();
    assert!(
        crafting_at < furnace_at,
        "the crafting family precedes the furnace family"
    );
    let expected = mornlea_domain::FurnaceState::try_new(mornlea_domain::FurnaceStateParts {
        container: furnace_ref,
        input: stack(ITEM_RAW_IRON, 2),
        fuel: stack(ITEM_COAL, 1),
        output: ItemStack::EMPTY,
        progress_ticks: 8,
        burn_ticks: 1_600 - 8,
    })
    .unwrap();
    assert_eq!(
        find_event(&tick_seven_events, |event| matches!(
            event,
            Event::FurnaceState(_)
        )),
        Some(&Event::FurnaceState(expected))
    );
}

/// projection::batch_splitting_respects_wire_caps — five visible companions
/// publish five spawns, then consecutive state batches of four and one.
#[test]
fn projection_batch_splitting_respects_wire_caps() {
    let mut state = authority();
    seed_world(&mut state);
    let session = login(&mut state, 1, "Ada", [0.5, 65.0, 0.5], 0.0, 0.0);
    stage(&mut state, |context| {
        for tag in 1..=5u8 {
            context
                .stage(RuleEffect::Actor(companion_actor(
                    companion_id(tag),
                    [8.5, 65.0, 8.5],
                    0.0,
                )))
                .unwrap();
        }
    });
    let tick_a = state.advance_tick(TickBudget::full()).unwrap();
    let events = events_for(&tick_a, session);
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(event, Event::CompanionSpawn(_)))
            .count(),
        5,
        "the spawn tick publishes one spawn per companion"
    );
    assert!(
        !events
            .iter()
            .any(|event| matches!(event, Event::CompanionStates(_)))
    );
    let tick_b = state.advance_tick(TickBudget::full()).unwrap();
    let events = events_for(&tick_b, session);
    let batches: Vec<&mornlea_domain::CompanionStates> = events
        .iter()
        .filter_map(|event| match event {
            Event::CompanionStates(batch) => Some(batch),
            _ => None,
        })
        .collect();
    assert_eq!(batches.len(), 2, "five records split four and one");
    assert_eq!(batches[0].states().len(), 4);
    assert_eq!(batches[1].states().len(), 1);
    let mut ids: Vec<CompanionId> = batches[0]
        .states()
        .iter()
        .chain(batches[1].states())
        .map(|record| record.id())
        .collect();
    let mut expected: Vec<CompanionId> = (1..=5u8).map(companion_id).collect();
    expected.sort();
    ids.sort();
    assert_eq!(ids, expected);
    for batch in batches {
        assert_eq!(batch.server_tick(), 1);
    }
}

/// projection::despawn_families_emit_once — every departed entity publishes
/// its despawn or removal exactly once: the tick after the departure tick
/// publishes none of that family again for the departed identities.
#[test]
fn projection_despawn_families_emit_once() {
    let mut state = authority();
    seed_world(&mut state);
    let observer = login(&mut state, 1, "Ada", [0.5, 65.0, 0.5], 0.0, 0.0);
    let remote = login(&mut state, 2, "Ben", [4.5, 65.0, 4.5], 0.0, 0.0);
    let companion = companion_id(9);
    let hostile = HostileId::try_new(31).unwrap();
    let passive = PassiveId::try_new(7).unwrap();
    let projectile = ProjectileId::try_new(5).unwrap();
    let drop_id = DropId::try_new(0, ChunkPos::new(0, 0), 3, 1).unwrap();
    stage(&mut state, |context| {
        context
            .stage(RuleEffect::Actor(companion_actor(
                companion,
                [8.5, 65.0, 8.5],
                0.0,
            )))
            .unwrap();
        context
            .stage(RuleEffect::Actor(hostile_actor(31, [9.5, 65.0, 9.5], 0)))
            .unwrap();
        context
            .stage(RuleEffect::Actor(passive_actor(7, [7.5, 65.0, 7.5])))
            .unwrap();
        context
            .stage(RuleEffect::Runtime(passive_runtime(passive)))
            .unwrap();
        context
            .stage(RuleEffect::Projectile {
                before: None,
                after: Some(ProjectileRecord {
                    id: projectile,
                    owner: ActorKey::Player(observer),
                    dimension: Dimension::OVERWORLD,
                    position: FiniteVec3::try_new([4.5, 100.0, 4.5]).unwrap(),
                    velocity: FiniteVec3::try_new([0.0, 0.0, 0.0]).unwrap(),
                    kind: ProjectileKind::Shard,
                    damage: 3,
                    // The age ceiling removes the projectile on tick B.
                    age: 99,
                }),
            })
            .unwrap();
        // The lifetime ceiling removes the drop on tick B.
        context.preload_drop(drop_record(3, 5_998, [0.5, 65.5, 2.5]));
    });
    // Tick A: every family becomes visible.
    let tick_a = state.advance_tick(TickBudget::full()).unwrap();
    let spawned = events_for(&tick_a, observer);
    for present in [
        matches!(spawned.as_slice(), event if event.iter().any(|event| matches!(event, Event::RemotePlayerSpawn(_)))),
        matches!(spawned.as_slice(), event if event.iter().any(|event| matches!(event, Event::CompanionSpawn(_)))),
        matches!(spawned.as_slice(), event if event.iter().any(|event| matches!(event, Event::HostileSpawn(_)))),
        matches!(spawned.as_slice(), event if event.iter().any(|event| matches!(event, Event::PassiveSpawn(_)))),
        matches!(spawned.as_slice(), event if event.iter().any(|event| matches!(event, Event::ProjectileSpawn(_)))),
        matches!(spawned.as_slice(), event if event.iter().any(|event| matches!(event, Event::ItemDropUpserts(_)))),
    ] {
        assert!(present, "every family spawns on its visibility tick");
    }
    // The projectile families precede the passive families in the frozen
    // order, on the spawn tick and on the departure tick alike.
    let position_of =
        |events: &[Event], family: &dyn Fn(&Event) -> bool| events.iter().position(family).unwrap();
    let is_projectile = |event: &Event| {
        matches!(
            event,
            Event::ProjectileSpawn(_) | Event::ProjectileDespawn(_)
        )
    };
    let is_passive =
        |event: &Event| matches!(event, Event::PassiveSpawn(_) | Event::PassiveDespawn(_));
    assert!(position_of(&spawned, &is_projectile) < position_of(&spawned, &is_passive));
    // Between A and B every family departs: the remote leaves interest, the
    // mobs die, the observer walks away from the still-living companion, and
    // the two transients reach their age ceilings during tick B.
    stage(&mut state, |context| {
        let mut moved = context
            .read()
            .actor(ActorKey::Player(remote))
            .cloned()
            .unwrap();
        moved.motion = MotionState::new(MotionStateParts {
            position: FiniteVec3::try_new([640.5, 65.0, 0.5]).unwrap(),
            velocity: FiniteVec3::try_new([0.0; 3]).unwrap(),
            on_ground: true,
        });
        context.stage(RuleEffect::Actor(moved)).unwrap();
        for departed in [ActorKey::Hostile(hostile), ActorKey::Passive(passive)] {
            let mut actor = context.read().actor(departed).cloned().unwrap();
            actor.lifecycle = ActorLifecycle::Dead;
            context.stage(RuleEffect::Actor(actor)).unwrap();
        }
        let mut observer_actor = context
            .read()
            .actor(ActorKey::Player(observer))
            .cloned()
            .unwrap();
        observer_actor.motion = MotionState::new(MotionStateParts {
            position: FiniteVec3::try_new([400.5, 65.0, 400.5]).unwrap(),
            velocity: FiniteVec3::try_new([0.0; 3]).unwrap(),
            on_ground: true,
        });
        context.stage(RuleEffect::Actor(observer_actor)).unwrap();
    });
    let tick_b = state.advance_tick(TickBudget::full()).unwrap();
    let departed = events_for(&tick_b, observer);
    let despawns_of =
        |family: &dyn Fn(&Event) -> bool| departed.iter().filter(|event| family(event)).count();
    let remote_despawn = |event: &Event| {
        matches!(
            event,
            Event::RemotePlayerDespawn(despawn) if despawn.player_id()
                == PlayerId::try_from_bytes(uuid(2)).unwrap()
        )
    };
    let companion_despawn = |event: &Event| {
        matches!(
            event,
            Event::CompanionDespawn(despawn) if despawn.id() == companion
        )
    };
    let hostile_despawn = |event: &Event| {
        matches!(
            event,
            Event::HostileDespawn(batch) if batch.ids() == [hostile]
        )
    };
    let passive_despawn = |event: &Event| {
        matches!(
            event,
            Event::PassiveDespawn(batch)
                if batch.despawns() == [PassiveDespawnRecord::new(passive, PassiveDespawnReason::Died)]
        )
    };
    let projectile_despawn = |event: &Event| {
        matches!(
            event,
            Event::ProjectileDespawn(batch) if batch.ids() == [projectile]
        )
    };
    let drop_remove =
        |event: &Event| matches!(event, Event::ItemDropRemoves(batch) if batch.ids() == [drop_id]);
    for (family, count) in [
        ("remote", despawns_of(&remote_despawn)),
        ("companion", despawns_of(&companion_despawn)),
        ("hostile", despawns_of(&hostile_despawn)),
        ("passive", despawns_of(&passive_despawn)),
        ("projectile", despawns_of(&projectile_despawn)),
        ("drop", despawns_of(&drop_remove)),
    ] {
        assert_eq!(count, 1, "the {family} family departs exactly once");
    }
    assert!(
        position_of(&departed, &is_projectile) < position_of(&departed, &is_passive),
        "the projectile families precede the passive families on departure"
    );
    // Tick C: none of the departed families publishes again.
    let tick_c = state.advance_tick(TickBudget::full()).unwrap();
    let follow_up = events_for(&tick_c, observer);
    let repeated = follow_up.iter().any(|event| {
        remote_despawn(event)
            || companion_despawn(event)
            || hostile_despawn(event)
            || passive_despawn(event)
            || projectile_despawn(event)
            || drop_remove(event)
    });
    assert!(
        !repeated,
        "the follow-up tick republishes a departed family: {follow_up:?}"
    );

    // Re-entry: the observer walks back, so the still-living companion
    // becomes visible again and publishes exactly one fresh spawn; every
    // permanently departed family stays silent, and the next tick resumes
    // the persisting companion states without a second spawn.
    stage(&mut state, |context| {
        let mut observer_actor = context
            .read()
            .actor(ActorKey::Player(observer))
            .cloned()
            .unwrap();
        observer_actor.motion = MotionState::new(MotionStateParts {
            position: FiniteVec3::try_new([0.5, 65.0, 0.5]).unwrap(),
            velocity: FiniteVec3::try_new([0.0; 3]).unwrap(),
            on_ground: true,
        });
        context.stage(RuleEffect::Actor(observer_actor)).unwrap();
    });
    let tick_d = state.advance_tick(TickBudget::full()).unwrap();
    let reentered = events_for(&tick_d, observer);
    let respawned = reentered
        .iter()
        .filter(|event| matches!(event, Event::CompanionSpawn(spawn) if spawn.id() == companion))
        .count();
    assert_eq!(respawned, 1, "re-entry publishes exactly one fresh spawn");
    let reopened = reentered.iter().any(|event| {
        remote_despawn(event)
            || hostile_despawn(event)
            || passive_despawn(event)
            || projectile_despawn(event)
            || drop_remove(event)
    });
    assert!(
        !reopened,
        "permanently departed families stay silent on re-entry: {reentered:?}"
    );
    let tick_e = state.advance_tick(TickBudget::full()).unwrap();
    let settled = events_for(&tick_e, observer);
    assert!(
        !settled
            .iter()
            .any(|event| matches!(event, Event::CompanionSpawn(_)))
    );
    assert!(
        settled.iter().any(
            |event| matches!(event, Event::CompanionStates(batch) if batch.states().len() == 1)
        )
    );
}

/// projection::first_send_snapshots_ascend_across_ticks — the first-send
/// pass is one sorted union: a smaller newly-wanted column publishes before
/// an older unsent column that is still pending from a previous tick.
#[test]
fn projection_first_send_snapshots_ascend_across_ticks() {
    let mut state = authority();
    // One Ready ground column east of the spawn area; the player's own
    // column stays unready, so nothing else can publish on the first tick.
    stage(&mut state, |context| {
        context
            .stage(RuleEffect::Environment(environment(1_000)))
            .unwrap();
        context.preload_ready_chunk(
            ReadyChunk::try_new(chunk_key(3, 0), 1, 1, ground_chunk()).unwrap(),
        );
    });
    let session = login(&mut state, 1, "Ada", [40.5, 65.0, 0.5], 0.0, 0.0);
    let tick_a = state.advance_tick(TickBudget::full()).unwrap();
    let sent: Vec<ChunkPos> = events_for(&tick_a, session)
        .iter()
        .filter_map(|event| match event {
            Event::ChunkSnapshot(snapshot) => Some(snapshot.chunk()),
            _ => None,
        })
        .collect();
    assert_eq!(sent, [ChunkPos::new(3, 0)]);
    // The player walks one column west, so the interest both keeps an older
    // unsent column ((2, -2), wanted since tick A but only now ready) and
    // gains a smaller new one ((-1, 0)), beside the new standing column.
    stage(&mut state, |context| {
        let mut actor = context
            .read()
            .actor(ActorKey::Player(session))
            .cloned()
            .unwrap();
        actor.motion = MotionState::new(MotionStateParts {
            position: FiniteVec3::try_new([24.5, 65.0, 0.5]).unwrap(),
            velocity: FiniteVec3::try_new([0.0; 3]).unwrap(),
            on_ground: true,
        });
        context.stage(RuleEffect::Actor(actor)).unwrap();
        for key in [chunk_key(-1, 0), chunk_key(1, 0), chunk_key(2, -2)] {
            context.preload_ready_chunk(ReadyChunk::try_new(key, 1, 1, ground_chunk()).unwrap());
        }
    });
    let tick_b = state.advance_tick(TickBudget::full()).unwrap();
    let sent: Vec<ChunkPos> = events_for(&tick_b, session)
        .iter()
        .filter_map(|event| match event {
            Event::ChunkSnapshot(snapshot) => Some(snapshot.chunk()),
            _ => None,
        })
        .collect();
    assert_eq!(
        sent,
        [
            ChunkPos::new(-1, 0),
            ChunkPos::new(1, 0),
            ChunkPos::new(2, -2)
        ],
        "retained and newly wanted first sends ascend together"
    );
}

fn authority_default() -> AuthorityState {
    AuthorityState::try_new(
        ServerLimits::try_new(8, 4096, 512, 64, 64, 1_048_576).unwrap(),
        7,
    )
    .unwrap()
}

/// projection_wanted_default_bound_and_builder_values — the true default
/// server view bound is 33, the builder overrides it, and the protocol
/// constructors refuse declared distances 1 and 65.
#[test]
fn projection_wanted_default_bound_and_builder_values() {
    let defaults = ServerLimits::try_new(8, 4096, 512, 64, 64, 1_048_576).unwrap();
    assert_eq!(defaults.view_radius(), 33);
    assert_eq!(
        ServerLimits::try_new(8, 4096, 512, 64, 64, 1_048_576)
            .unwrap()
            .with_view_radius(0)
            .view_radius(),
        0
    );
    assert_eq!(
        ServerLimits::try_new(8, 4096, 512, 64, 64, 1_048_576)
            .unwrap()
            .with_view_radius(2)
            .view_radius(),
        2
    );
    assert_eq!(
        ServerLimits::try_new(8, 4096, 512, 64, 64, 1_048_576)
            .unwrap()
            .with_view_radius(33)
            .view_radius(),
        33
    );
    assert_eq!(
        ServerLimits::try_new(8, 4096, 512, 64, 64, 1_048_576)
            .unwrap()
            .with_view_radius(usize::MAX)
            .view_radius(),
        usize::MAX
    );
    let id = PlayerId::try_from_bytes(uuid(1)).unwrap();
    assert!(LoginStart::new(id, "Ada", 1).is_err());
    assert!(LoginStart::new(id, "Ada", 65).is_err());
}

/// projection_wanted_default_includes_chunk_three — true default 33 with a
/// declared 8 publishes Ready (3, 0), which the old radius-two derivation
/// never published.
#[test]
fn projection_wanted_default_includes_chunk_three() {
    let mut state = authority_default();
    assert_eq!(state.limits().view_radius(), 33);
    seed_world(&mut state);
    let session = login_declared(&mut state, 1, "Ada", [0.5, 65.0, 0.5], 0.0, 0.0, 8);
    preload_keys(&mut state, &[(3, 0)]);
    let tick = state.advance_tick(TickBudget::full()).unwrap();
    let sent = snapshot_positions(&events_for(&tick, session));
    assert!(sent.contains(&ChunkPos::new(0, 0)));
    assert!(
        sent.contains(&ChunkPos::new(3, 0)),
        "declared 8 under default 33 publishes (3, 0)"
    );
}

/// projection_wanted_declared_minimum_boundary — declared 2 (effective 3)
/// covers (3, -3) but not (4, 0).
#[test]
fn projection_wanted_declared_minimum_boundary() {
    let mut state = authority_with_view_radius(33);
    seed_world(&mut state);
    let session = login_declared(&mut state, 1, "Ada", [0.5, 65.0, 0.5], 0.0, 0.0, 2);
    preload_keys(&mut state, &[(3, -3), (4, 0)]);
    let tick = state.advance_tick(TickBudget::full()).unwrap();
    let sent = snapshot_positions(&events_for(&tick, session));
    assert!(sent.contains(&ChunkPos::new(3, -3)));
    assert!(
        !sent.contains(&ChunkPos::new(4, 0)),
        "declared 2 stops before (4, 0)"
    );
}

/// projection_wanted_declared_max_clamped_to_default — declared 64 under
/// default 33 (effective 33) includes (33, 0) but excludes (34, 0).
#[test]
fn projection_wanted_declared_max_clamped_to_default() {
    let mut state = authority_with_view_radius(33);
    seed_world(&mut state);
    let session = login_declared(&mut state, 1, "Ada", [0.5, 65.0, 0.5], 0.0, 0.0, 64);
    preload_keys(&mut state, &[(33, 0), (34, 0)]);
    let tick = state.advance_tick(TickBudget::full()).unwrap();
    let sent = snapshot_positions(&events_for(&tick, session));
    assert!(sent.contains(&ChunkPos::new(33, 0)));
    assert!(!sent.contains(&ChunkPos::new(34, 0)));
}

/// projection_wanted_unbounded_cap_supports_sixty_five — cap usize::MAX
/// with declared 64 (effective 65) includes (65, 0) but excludes (66, 0).
#[test]
fn projection_wanted_unbounded_cap_supports_sixty_five() {
    let mut state = authority_with_view_radius(usize::MAX);
    seed_world(&mut state);
    let session = login_declared(&mut state, 1, "Ada", [0.5, 65.0, 0.5], 0.0, 0.0, 64);
    preload_keys(&mut state, &[(65, 0), (66, 0)]);
    let tick = state.advance_tick(TickBudget::full()).unwrap();
    let sent = snapshot_positions(&events_for(&tick, session));
    assert!(sent.contains(&ChunkPos::new(65, 0)));
    assert!(!sent.contains(&ChunkPos::new(66, 0)));
}

/// projection_wanted_cap_two_clamps_wide_declaration — cap 2 clamps
/// declared 8 to radius 2: (2, 0) publishes, (3, 0) does not.
#[test]
fn projection_wanted_cap_two_clamps_wide_declaration() {
    let mut state = authority_with_view_radius(2);
    seed_world(&mut state);
    let session = login_declared(&mut state, 1, "Ada", [0.5, 65.0, 0.5], 0.0, 0.0, 8);
    preload_keys(&mut state, &[(2, 0), (3, 0)]);
    let tick = state.advance_tick(TickBudget::full()).unwrap();
    let sent = snapshot_positions(&events_for(&tick, session));
    assert!(sent.contains(&ChunkPos::new(2, 0)));
    assert!(!sent.contains(&ChunkPos::new(3, 0)));
}

/// projection_wanted_two_sessions_differ_and_resync_isolated — co-located
/// declared 2 and 8: Ready (4, 0) publishes only in the wide session, each
/// session's resync answers only its own wanted, and no mirror leaks across
/// sessions.
#[test]
fn projection_wanted_two_sessions_differ_and_resync_isolated() {
    let mut state = authority_with_view_radius(33);
    seed_world(&mut state);
    let narrow = login_declared(&mut state, 1, "Ada", [0.5, 65.0, 0.5], 0.0, 0.0, 2);
    let wide = login_declared(&mut state, 2, "Bea", [0.5, 65.0, 0.5], 0.0, 0.0, 8);
    preload_keys(&mut state, &[(4, 0)]);
    let tick_a = state.advance_tick(TickBudget::full()).unwrap();
    let narrow_sent = snapshot_positions(&events_for(&tick_a, narrow));
    let wide_sent = snapshot_positions(&events_for(&tick_a, wide));
    assert!(narrow_sent.contains(&ChunkPos::new(0, 0)));
    assert!(
        !narrow_sent.contains(&ChunkPos::new(4, 0)),
        "declared 2 never mirrors (4, 0)"
    );
    assert!(wide_sent.contains(&ChunkPos::new(0, 0)));
    assert!(
        wide_sent.contains(&ChunkPos::new(4, 0)),
        "declared 8 publishes (4, 0)"
    );
    submit(
        &mut state,
        narrow,
        1,
        Command::Resync(ResyncIntent::try_new(0, ChunkPos::new(4, 0), 1).unwrap()),
    );
    submit(
        &mut state,
        wide,
        1,
        Command::Resync(ResyncIntent::try_new(0, ChunkPos::new(4, 0), 1).unwrap()),
    );
    let tick_b = state.advance_tick(TickBudget::full()).unwrap();
    assert_eq!(state.session(narrow).unwrap().last_applied_sequence, 1);
    assert_eq!(state.session(wide).unwrap().last_applied_sequence, 1);
    let narrow_resync = snapshot_positions(&events_for(&tick_b, narrow));
    let wide_resync = snapshot_positions(&events_for(&tick_b, wide));
    assert!(
        !narrow_resync.contains(&ChunkPos::new(4, 0)),
        "the narrow resync stays silent outside its wanted"
    );
    assert_eq!(
        wide_resync,
        vec![ChunkPos::new(4, 0)],
        "the wide resync answers its own wanted alone"
    );
}

/// projection_wanted_late_join_gets_own_first_snapshot — a shared Ready
/// column gives the late subscriber its own first snapshot without a
/// duplicate for the existing subscriber.
#[test]
fn projection_wanted_late_join_gets_own_first_snapshot() {
    let mut state = authority_with_view_radius(33);
    seed_world(&mut state);
    let first = login_declared(&mut state, 1, "Ada", [0.5, 65.0, 0.5], 0.0, 0.0, 8);
    let tick_a = state.advance_tick(TickBudget::full()).unwrap();
    assert!(snapshot_positions(&events_for(&tick_a, first)).contains(&ChunkPos::new(0, 0)));
    let late = login_declared(&mut state, 2, "Bea", [0.5, 65.0, 0.5], 0.0, 0.0, 8);
    let tick_b = state.advance_tick(TickBudget::full()).unwrap();
    assert_eq!(
        snapshot_positions(&events_for(&tick_b, late)),
        vec![ChunkPos::new(0, 0)],
        "the late joiner snapshots the shared Ready column"
    );
    assert!(
        snapshot_positions(&events_for(&tick_b, first)).is_empty(),
        "the existing subscriber sees no duplicate"
    );
}

/// projection_wanted_unready_skips_then_snapshots_once — a wanted but
/// unready key publishes nothing, then snapshots exactly once when Ready.
#[test]
fn projection_wanted_unready_skips_then_snapshots_once() {
    let mut state = authority_with_view_radius(33);
    seed_world(&mut state);
    let session = login_declared(&mut state, 1, "Ada", [0.5, 65.0, 0.5], 0.0, 0.0, 8);
    let tick_a = state.advance_tick(TickBudget::full()).unwrap();
    assert!(!snapshot_positions(&events_for(&tick_a, session)).contains(&ChunkPos::new(3, 0)));
    preload_keys(&mut state, &[(3, 0)]);
    let tick_b = state.advance_tick(TickBudget::full()).unwrap();
    assert_eq!(
        snapshot_positions(&events_for(&tick_b, session)),
        vec![ChunkPos::new(3, 0)]
    );
    let tick_c = state.advance_tick(TickBudget::full()).unwrap();
    assert!(
        snapshot_positions(&events_for(&tick_c, session)).is_empty(),
        "the Ready column never resnapshots"
    );
}

/// projection_wanted_move_forgets_old_and_snapshots_new — declared 2 moves
/// from chunk 0 to chunk 1: the exited x -3 column forgets sorted, the new
/// x 4 boundary snapshots first, and retained (3, 0) never resnapshots.
#[test]
fn projection_wanted_move_forgets_old_and_snapshots_new() {
    let mut state = authority_with_view_radius(33);
    seed_world(&mut state);
    let session = login_declared(&mut state, 1, "Ada", [0.5, 65.0, 0.5], 0.0, 0.0, 2);
    let mut exited: Vec<(i32, i32)> = Vec::new();
    for z in -3..=3 {
        exited.push((-3, z));
    }
    let mut first_keys = exited.clone();
    first_keys.push((3, 0));
    preload_keys(
        &mut state,
        &first_keys.iter().map(|(x, z)| (*x, *z)).collect::<Vec<_>>(),
    );
    let tick_a = state.advance_tick(TickBudget::full()).unwrap();
    let sent_a = snapshot_positions(&events_for(&tick_a, session));
    assert!(sent_a.contains(&ChunkPos::new(-3, 0)));
    assert!(sent_a.contains(&ChunkPos::new(3, 0)));
    assert!(!sent_a.contains(&ChunkPos::new(4, 0)));
    stage(&mut state, |context| {
        let mut actor = context
            .read()
            .actor(ActorKey::Player(session))
            .cloned()
            .unwrap();
        actor.motion = MotionState::new(MotionStateParts {
            position: FiniteVec3::try_new([16.5, 65.0, 0.5]).unwrap(),
            velocity: FiniteVec3::try_new([0.0; 3]).unwrap(),
            on_ground: true,
        });
        context.stage(RuleEffect::Actor(actor)).unwrap();
        context.preload_ready_chunk(
            ReadyChunk::try_new(chunk_key(4, 0), 1, 1, ground_chunk()).unwrap(),
        );
    });
    let tick_b = state.advance_tick(TickBudget::full()).unwrap();
    let events_b = events_for(&tick_b, session);
    let mut expected: Vec<ChunkPos> = (-3..=3).map(|z| ChunkPos::new(-3, z)).collect();
    expected.sort();
    let forgets = forget_positions(&events_b);
    assert_eq!(forgets, expected, "the exited column forgets sorted");
    for event in &events_b {
        if let Event::ChunkSnapshot(snapshot) = event {
            assert_eq!(
                snapshot.dimension(),
                Dimension::OVERWORLD,
                "wanted stays in the actor's own dimension"
            );
        }
    }
    assert_eq!(
        snapshot_positions(&events_b),
        vec![ChunkPos::new(4, 0)],
        "only the new boundary first-sends"
    );
}

/// projection_wanted_negative_center — a negative foot chunk center keeps
/// the inclusive square semantics: (-4, -4) publishes, (3, 0) does not.
#[test]
fn projection_wanted_negative_center() {
    let mut state = authority_with_view_radius(33);
    stage(&mut state, |context| {
        context
            .stage(RuleEffect::Environment(environment(1_000)))
            .unwrap();
    });
    let session = login_declared(&mut state, 1, "Ada", [-0.5, 65.0, -0.5], 0.0, 0.0, 2);
    preload_keys(&mut state, &[(-4, -4), (3, 0)]);
    let tick = state.advance_tick(TickBudget::full()).unwrap();
    let sent = snapshot_positions(&events_for(&tick, session));
    assert!(sent.contains(&ChunkPos::new(-4, -4)));
    assert!(!sent.contains(&ChunkPos::new(3, 0)));
}

/// projection_wanted_resync_order_watermark_and_stale — a wanted Ready
/// resync answers before an ordinary first send even with HaveRevision above
/// the server revision; nonwanted and unready resyncs stay silent while
/// consuming their sequences; repeat sequences stay stale.
#[test]
fn projection_wanted_resync_order_watermark_and_stale() {
    let mut state = authority_with_view_radius(33);
    seed_world(&mut state);
    let session = login_declared(&mut state, 1, "Ada", [0.5, 65.0, 0.5], 0.0, 0.0, 8);
    let _ = state.advance_tick(TickBudget::full()).unwrap();
    preload_keys(&mut state, &[(1, 0), (4, 0), (10, 0)]);
    submit(
        &mut state,
        session,
        1,
        Command::Resync(ResyncIntent::try_new(0, ChunkPos::new(0, 0), 999).unwrap()),
    );
    submit(
        &mut state,
        session,
        2,
        Command::Resync(ResyncIntent::try_new(0, ChunkPos::new(10, 0), 1).unwrap()),
    );
    submit(
        &mut state,
        session,
        3,
        Command::Resync(ResyncIntent::try_new(0, ChunkPos::new(2, 0), 1).unwrap()),
    );
    submit(
        &mut state,
        session,
        4,
        Command::Resync(ResyncIntent::try_new(0, ChunkPos::new(4, 0), 1).unwrap()),
    );
    let tick_b = state.advance_tick(TickBudget::full()).unwrap();
    assert_eq!(tick_b.counters.commands, 4);
    assert_eq!(state.session(session).unwrap().last_applied_sequence, 4);
    let snapshots = snapshot_positions(&events_for(&tick_b, session));
    assert_eq!(
        snapshots,
        vec![
            ChunkPos::new(0, 0),
            ChunkPos::new(4, 0),
            ChunkPos::new(1, 0),
        ],
        "wanted Ready resyncs answer before the ordinary first send"
    );
    submit(
        &mut state,
        session,
        4,
        Command::Resync(ResyncIntent::try_new(0, ChunkPos::new(0, 0), 1).unwrap()),
    );
    let tick_c = state.advance_tick(TickBudget::full()).unwrap();
    assert_eq!(tick_c.counters.stale, 1);
    assert_eq!(state.session(session).unwrap().last_applied_sequence, 4);
    assert!(
        snapshot_positions(&events_for(&tick_c, session)).is_empty(),
        "the repeated sequence stays stale"
    );
    submit(
        &mut state,
        session,
        5,
        Command::Resync(ResyncIntent::try_new(0, ChunkPos::new(0, 0), 1).unwrap()),
    );
    let tick_d = state.advance_tick(TickBudget::full()).unwrap();
    assert_eq!(state.session(session).unwrap().last_applied_sequence, 5);
    assert_eq!(
        snapshot_positions(&events_for(&tick_d, session)),
        vec![ChunkPos::new(0, 0)]
    );
}
/// projection_wanted_cap_zero_is_center_only — cap 0 publishes only the
/// center column.
#[test]
fn projection_wanted_cap_zero_is_center_only() {
    let mut state = authority_with_view_radius(0);
    seed_world(&mut state);
    let session = login_declared(&mut state, 1, "Ada", [0.5, 65.0, 0.5], 0.0, 0.0, 8);
    preload_keys(&mut state, &[(1, 0)]);
    let tick = state.advance_tick(TickBudget::full()).unwrap();
    let sent = snapshot_positions(&events_for(&tick, session));
    assert!(sent.contains(&ChunkPos::new(0, 0)));
    assert!(!sent.contains(&ChunkPos::new(1, 0)));
}

/// The iron helmet wire item, the same frozen `core.ItemIronHelmet` number the
/// armor provider mirrors (`packages/shared/core/item.go`, helmet durability
/// ceiling 165).
const PROJECTION_IRON_HELMET: u16 = 58;

/// One session's inventory state publications for a tick, in order.
fn projection_inventory_states(events: &[Event]) -> Vec<Event> {
    events
        .iter()
        .filter(|event| matches!(event, Event::InventoryState(_)))
        .cloned()
        .collect()
}

/// projection_inventory_dirty_select_round_trip_publishes_once — two accepted
/// selects that settle back to the held slot in one tick leave no record diff,
/// yet the dirty lane still publishes exactly one complete owner inventory
/// state, never to the foreign session, and a quiet tick publishes none.
#[test]
fn projection_inventory_dirty_select_round_trip_publishes_once() {
    let mut state = authority();
    seed_world(&mut state);
    let owner = login(&mut state, 1, "Ada", [0.5, 65.0, 0.5], 0.0, 0.0);
    let other = login(&mut state, 2, "Ben", [4.5, 65.0, 4.5], 0.0, 0.0);
    let join = state.advance_tick(TickBudget::full()).unwrap();
    let baseline = find_event(&events_for(&join, owner), |event| {
        matches!(event, Event::InventoryState(_))
    })
    .cloned()
    .expect("the join publication carries the complete owner inventory snapshot");

    // Select away and back inside one tick: the final record equals the join
    // snapshot, so only the dirty lane can publish, and it publishes once.
    submit(
        &mut state,
        owner,
        1,
        Command::SelectHotbar(HotbarSlot::new(2).unwrap()),
    );
    submit(
        &mut state,
        owner,
        2,
        Command::SelectHotbar(HotbarSlot::new(0).unwrap()),
    );
    let dirty = state.advance_tick(TickBudget::full()).unwrap();
    assert_eq!(
        projection_inventory_states(&events_for(&dirty, owner)),
        vec![baseline],
        "the select round trip publishes exactly one complete owner inventory state"
    );
    assert!(
        !events_for(&dirty, other)
            .iter()
            .any(|event| matches!(event, Event::InventoryState(_))),
        "the foreign session never sees the owner inventory"
    );

    let quiet = state.advance_tick(TickBudget::full()).unwrap();
    assert!(
        projection_inventory_states(&events_for(&quiet, owner)).is_empty(),
        "a quiet tick publishes no inventory state"
    );
}

/// projection_inventory_dirty_select_carry_budget_publishes_once — a command
/// budget that carries the round trip settles it on the second tick and
/// republishes the unchanged complete owner state exactly once.
#[test]
fn projection_inventory_dirty_select_carry_budget_publishes_once() {
    let mut state = authority();
    seed_world(&mut state);
    let owner = login(&mut state, 1, "Ada", [0.5, 65.0, 0.5], 0.0, 0.0);
    let _ = state.advance_tick(TickBudget::full()).unwrap();
    submit(
        &mut state,
        owner,
        1,
        Command::SelectHotbar(HotbarSlot::new(2).unwrap()),
    );
    submit(
        &mut state,
        owner,
        2,
        Command::SelectHotbar(HotbarSlot::new(0).unwrap()),
    );
    submit(
        &mut state,
        owner,
        3,
        Command::SelectHotbar(HotbarSlot::new(2).unwrap()),
    );
    let carried_tick = state
        .advance_tick(TickBudget::try_new(1, 512, 65536, 65536, 65536).unwrap())
        .unwrap();
    assert_eq!(carried_tick.counters.commands, 1);
    assert_eq!(carried_tick.counters.carried, 2);
    let selected_two = InventoryState::new(InventoryStateParts {
        selected: HotbarSlot::new(2).unwrap(),
        hotbar: item_array(&[]),
        backpack: item_array(&[]),
    });
    assert_eq!(
        projection_inventory_states(&events_for(&carried_tick, owner)),
        vec![Event::InventoryState(selected_two.clone())],
        "the one executed select publishes exactly one complete owner state"
    );

    let settled_tick = state
        .advance_tick(TickBudget::try_new(2, 512, 65536, 65536, 65536).unwrap())
        .unwrap();
    assert_eq!(settled_tick.counters.commands, 2);
    assert_eq!(settled_tick.counters.carried, 0);
    assert_eq!(
        projection_inventory_states(&events_for(&settled_tick, owner)),
        vec![Event::InventoryState(selected_two)],
        "the carried round trip republishes the unchanged complete state once"
    );

    let quiet = state.advance_tick(TickBudget::full()).unwrap();
    assert!(
        projection_inventory_states(&events_for(&quiet, owner)).is_empty(),
        "a quiet tick publishes no inventory state"
    );
}

/// projection_inventory_dirty_equip_armor_publishes_once — an accepted equip of
/// the held iron helmet publishes exactly one complete owner inventory state
/// with the helmet gone from the hotbar and settled into the armor region.
#[test]
fn projection_inventory_dirty_equip_armor_publishes_once() {
    let mut state = authority();
    seed_world(&mut state);
    let owner = login_with(
        &mut state,
        1,
        "Ada",
        [0.5, 65.0, 0.5],
        0.0,
        0.0,
        |player: &mut StoredPlayer| {
            player.inventory.hotbar.slots[0] = StorageStack {
                item: PROJECTION_IRON_HELMET,
                count: 1,
                durability: 165,
            };
        },
    );
    let other = login(&mut state, 2, "Ben", [4.5, 65.0, 4.5], 0.0, 0.0);
    let join = state.advance_tick(TickBudget::full()).unwrap();
    assert!(
        find_event(&events_for(&join, owner), |event| matches!(
            event,
            Event::InventoryState(_)
        ))
        .is_some(),
        "the join publication carries the complete owner inventory snapshot"
    );

    submit(&mut state, owner, 1, Command::EquipArmor);
    let equip = state.advance_tick(TickBudget::full()).unwrap();
    let expected = InventoryState::new(InventoryStateParts {
        selected: HotbarSlot::new(0).unwrap(),
        hotbar: item_array(&[]),
        backpack: item_array(&[]),
    });
    assert_eq!(
        projection_inventory_states(&events_for(&equip, owner)),
        vec![Event::InventoryState(expected)],
        "the equip publishes exactly one complete owner inventory state"
    );
    assert!(
        !events_for(&equip, other)
            .iter()
            .any(|event| matches!(event, Event::InventoryState(_))),
        "the foreign session never sees the owner inventory"
    );
    // The helmet settled into the armor region with count and durability kept.
    stage(&mut state, |context| {
        let record = context
            .read()
            .inventory(ActorKey::Player(owner))
            .cloned()
            .unwrap();
        assert_eq!(
            record.armor[0],
            StorageStack {
                item: PROJECTION_IRON_HELMET,
                count: 1,
                durability: 165,
            }
        );
    });

    let quiet = state.advance_tick(TickBudget::full()).unwrap();
    assert!(
        projection_inventory_states(&events_for(&quiet, owner)).is_empty(),
        "a quiet tick publishes no inventory state"
    );
}

/// projection_inventory_dirty_noop_refused_stale_publish_nothing — the held
/// reselect, a refused empty whole move, and a stale replay never mark the
/// dirty lane, so the tick publishes no inventory state at all.
#[test]
fn projection_inventory_dirty_noop_refused_stale_publish_nothing() {
    let mut state = authority();
    seed_world(&mut state);
    let owner = login(&mut state, 1, "Ada", [0.5, 65.0, 0.5], 0.0, 0.0);
    let join = state.advance_tick(TickBudget::full()).unwrap();
    assert!(
        find_event(&events_for(&join, owner), |event| matches!(
            event,
            Event::InventoryState(_)
        ))
        .is_some(),
        "the join publication carries the complete owner inventory snapshot"
    );

    // Re-selecting the held slot is the idempotent no-op row.
    submit(
        &mut state,
        owner,
        1,
        Command::SelectHotbar(HotbarSlot::new(0).unwrap()),
    );
    // A whole move whose source slot is empty is refused without effect.
    submit(
        &mut state,
        owner,
        2,
        Command::MoveInventory(mornlea_domain::InventoryMove::try_new(5, 6).unwrap()),
    );
    // A stale replay of an already-admitted sequence is skipped upstream; its
    // observed result is not unwrapped, only that it cannot mark the lane.
    let _stale = state.submit(
        owner,
        PlayIntent::Sequenced {
            sequence: 1,
            command: Command::SelectHotbar(HotbarSlot::new(2).unwrap()),
        },
    );
    let tick = state.advance_tick(TickBudget::full()).unwrap();
    assert!(
        projection_inventory_states(&events_for(&tick, owner)).is_empty(),
        "the no-op, refused, and stale commands publish no inventory state"
    );
}
