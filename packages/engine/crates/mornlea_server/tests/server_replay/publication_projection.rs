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
    Command, CommandText, CompanionDespawn, CompanionId, CompanionSpawn, CompanionSpawnParts,
    CompanionSpeaker, CompanionStates, ContainerKind, ContainerRef, CraftingSize, CraftingState,
    CraftingStateParts, Dimension, DisplayName, DropId, Event, EventRecipient, FiniteVec3,
    ForgetChunks, HostileId, HostileKind, HostileSpawn, HostileSpawnParts, HostileSpawnRecord,
    HostileSpawnRecordParts, HotbarSlot, InventoryState, InventoryStateParts, ItemDrop,
    ItemDropParts, ItemDropUpserts, ItemStack, LookAngles, MotionState, MotionStateParts,
    PartialMove, PassiveDespawn, PassiveDespawnParts, PassiveDespawnReason, PassiveDespawnRecord,
    PassiveId, PassiveSpawn, PassiveSpawnParts, PassiveSpawnRecord, PassiveSpawnRecordParts,
    PassiveState, PassiveStateParts, PassiveStateRecord, PassiveStateRecordParts, PlayerId,
    ProjectileDespawn, ProjectileDespawnParts, ProjectileId, ProjectileKind, ProjectileSpawn,
    ProjectileSpawnParts, ProjectileSpawnRecord, ProjectileSpawnRecordParts, ProjectileState,
    ProjectileStateParts, ProjectileStateRecord, ProjectileStateRecordParts, RemotePlayerSpawn,
    RemotePlayerSpawnParts, RemotePlayerState, RemotePlayerStateParts, RemotePlayerStates,
    ResyncIntent, StackSource, StackView, SurvivalState, SurvivalStateParts, Weather,
};
use mornlea_protocol::{AdmittedLogin, LoginStart, PlayIntent, admit_login};
use mornlea_server::contracts::{
    ActorAux, ActorBody, ActorKey, ActorLifecycle, ActorRecord, ActorRuntime, ChunkKey,
    CloseReason, DropRecord, EnvironmentState, ProjectileRecord, RuleEffect, RuleTunables,
    ServerLimits, SessionKey, TickBudget, TransportKind,
};
use mornlea_server::core::world::ReadyChunk;
use mornlea_server::state::{AuthorityState, TickContext};
use mornlea_storage::{
    ChestSlot, Chunk, CompanionBody, ContainerSnapshot, FurnaceSlot, HostileMob, Inventory,
    ItemStack as StorageStack, PassiveMob, PlayerId as SavePlayerId, PlayerLocation, StorageKind,
    StoredPlayer,
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
        ServerLimits::try_new(8, 4096, 512, 64, 64, 1_048_576).unwrap(),
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
    let id = PlayerId::try_from_bytes(uuid(tag)).unwrap();
    let start = LoginStart::new(id, name, 8).unwrap();
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
/// cow, with the vanished despawn reason for the removal.
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
        despawns: vec![PassiveDespawnRecord::new(
            id,
            PassiveDespawnReason::Vanished,
        )]
        .into_boxed_slice(),
    })
    .unwrap();
    assert_eq!(
        find_event(&events, |event| matches!(event, Event::PassiveDespawn(_))),
        Some(&Event::PassiveDespawn(expected))
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

/// projection::chat_accepted_broadcast_and_rejects_sender_only — malformed
/// and unknown addressing reject to the sender alone, a valid command
/// broadcasts with strictly increasing event ids, the 257th chat in one tick
/// rejects queue-full, and chat never enqueues a sequenced command.
#[test]
fn projection_chat_accepted_broadcast_and_rejects_sender_only() {
    let mut state = authority();
    seed_world(&mut state);
    let sender = login(&mut state, 1, "Ada", [0.5, 65.0, 0.5], 0.0, 0.0);
    let listener = login(&mut state, 2, "Ben", [4.5, 65.0, 4.5], 0.0, 0.0);
    let id = companion_id(9);
    let name = mornlea_domain::CompanionName::try_from_canonical(derived_name(id)).unwrap();
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
    // The queue ceiling: the 257th chat in one tick rejects queue-full to the
    // sender only, and the ids keep increasing.
    for _ in 0..257 {
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
    assert_eq!(chats.len(), 257);
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
                if batch.despawns() == [PassiveDespawnRecord::new(passive, PassiveDespawnReason::Vanished)]
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
