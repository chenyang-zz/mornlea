//! The exhaustive wire-to-semantic adapters: `PlayIntent` on the client side
//! and the `Event` bridge on the server side.
//!
//! `src/semantic.rs` is the seam where a decoded packet becomes a checked
//! semantic value and a checked semantic value becomes a packet. This suite
//! pins three properties of that seam.
//!
//! First, totality: every one of the 23 registered client packets maps to
//! exactly one `PlayIntent` shape — 19 sequenced commands, one chat intent,
//! one keep alive reply and the two negotiation refusals — and every one of
//! the 36 registered server packets maps either to one of the 30 closed
//! `Event` variants or to a refusal for the six control families. The tables
//! below enumerate the arms and assert the executed counts, so a packet added
//! to the registry without an adapter arm fails here instead of silently
//! falling through.
//!
//! Second, no metadata leakage: a conversion carries the wire sequence into
//! `PlayIntent::Sequenced` and back, and nothing else. No conversion invents a
//! session, a tick, an arrival index, a routing recipient or an authoritative
//! result, and `mornlea_domain::CommandEnvelope` is never constructed — the
//! envelope is the ordering layer's owner of that metadata, so the adapter's
//! source is pinned to not name it.
//!
//! Third, the transport budgets: the wire record caps (4 companion records,
//! 7 remote-player records, 32 drops, 64 hostile and passive records, 128
//! projectile records, 4096 world-delta and forget records) are applied by the
//! outbound conversion BEFORE any record is copied, so a domain-valid batch
//! above a cap is refused as one packet instead of being truncated into a
//! silently partial publication. The inbound direction relies on the packet's
//! own gate, which the adapters run first.

use mornlea_domain::{
    BlockChange as DomainBlockChange, BlockChanges as DomainBlockChanges, BlockChangesParts,
    BlockPos, ChatBody, ChatEvent as DomainChatEvent, ChatEventParts,
    ChestState as DomainChestState, ChestStateParts, ChunkPos,
    ChunkSnapshot as DomainChunkSnapshot, ChunkSnapshotParts, CombatHit as DomainCombatHit,
    CombatTarget, Command, CommandRejection, CompanionDespawn as DomainCompanionDespawn,
    CompanionId, CompanionName, CompanionSpawn as DomainCompanionSpawn, CompanionSpawnParts,
    CompanionSpeaker, CompanionState as DomainCompanionState, CompanionStateParts,
    CompanionStates as DomainCompanionStates, CompanionStatesParts,
    ContainerClosed as DomainContainerClosed, ContainerKind, ContainerRef as DomainContainerRef,
    CraftingMove, CraftingSize, CraftingState as DomainCraftingState, CraftingStateParts,
    Dimension, DisplayName, DomainError, DropId, Event, FiniteVec3,
    ForgetChunks as DomainForgetChunks, ForgetChunksParts, FurnaceState as DomainFurnaceState,
    FurnaceStateParts, HeldActions, HostileDespawn as DomainHostileDespawn, HostileDespawnParts,
    HostileId, HostileKind, HostileSpawn as DomainHostileSpawn, HostileSpawnParts,
    HostileSpawnRecord as DomainHostileSpawnRecord, HostileSpawnRecordParts,
    HostileState as DomainHostileState, HostileStateParts,
    HostileStateRecord as DomainHostileStateRecord, HostileStateRecordParts, HotbarSlot,
    InventoryMove, InventoryState as DomainInventoryState, InventoryStateParts,
    ItemDrop as DomainItemDrop, ItemDropParts, ItemDropRemoves as DomainItemDropRemoves,
    ItemDropRemovesParts, ItemDropUpserts as DomainItemDropUpserts, ItemDropUpsertsParts,
    ItemStack, LookAngles, MAX_SEMANTIC_BATCH_RECORDS, MiningState, MiningStateParts, MotionState,
    MotionStateParts, Movement, PalettedSection, PartialMove,
    PassiveDespawn as DomainPassiveDespawn, PassiveDespawnParts, PassiveDespawnReason,
    PassiveDespawnRecord as DomainPassiveDespawnRecord, PassiveId,
    PassiveSpawn as DomainPassiveSpawn, PassiveSpawnParts,
    PassiveSpawnRecord as DomainPassiveSpawnRecord, PassiveSpawnRecordParts,
    PassiveState as DomainPassiveState, PassiveStateParts,
    PassiveStateRecord as DomainPassiveStateRecord, PassiveStateRecordParts, PlacementIntent,
    PlacementSuccess, PlayerControl, PlayerControlParts, PlayerId,
    PlayerState as DomainPlayerState, PlayerStateParts,
    ProjectileDespawn as DomainProjectileDespawn, ProjectileDespawnParts, ProjectileId,
    ProjectileKind, ProjectileSpawn as DomainProjectileSpawn, ProjectileSpawnParts,
    ProjectileSpawnRecord as DomainProjectileSpawnRecord, ProjectileSpawnRecordParts,
    ProjectileState as DomainProjectileState, ProjectileStateParts,
    ProjectileStateRecord as DomainProjectileStateRecord, ProjectileStateRecordParts, RejectReason,
    RemotePlayerDespawn as DomainRemotePlayerDespawn, RemotePlayerSpawn as DomainRemotePlayerSpawn,
    RemotePlayerSpawnParts, RemotePlayerState as DomainRemotePlayerState, RemotePlayerStateParts,
    RemotePlayerStates as DomainRemotePlayerStates, RemotePlayerStatesParts, ResyncIntent, Season,
    StackSource, StackView, SurvivalState, SurvivalStateParts, TaskState, Weather, WorldState,
    WorldStateParts,
};
use mornlea_protocol::{
    CHAT_EVENT_ACCEPTED, CHAT_EVENT_REJECTED, CHAT_REJECT_INVALID_FORMAT, CHAT_REJECT_NONE,
    CONTAINER_KIND_FURNACE, ChatEvent, ClientHello, ClientPacket, ContainerRef,
    DISCONNECT_SLOW_CLIENT, Direction, Disconnect, FurnaceState, HANDSHAKE_VERSION_MISMATCH,
    HandshakeReject, InboundHello, InboundLoginStart, LOGIN_SERVER_FULL, LoginReject, LoginStart,
    LoginSuccess, MoveContainerStack, MoveStackPartial, PacketKey, PlayIntent, ProtocolCodec,
    ProtocolError, STACK_VIEW_CONTAINER, SectionData, ServerHello, ServerPacket, State,
    decode_client, encode_client_into,
};

/// Lowest block Y inside the world, so a full-section batch of block changes
/// can name real cells without leaving the vertical span.
const MIN_Y: i32 = -64;

/// Normalized `PlayerInput` observation: `(sequence, move_x, move_z, jump,
/// yaw_bits, pitch_bits, mining, eating, sprinting, sneaking)`.
type NormalizedPlayerInput = (u64, i8, i8, bool, u32, u32, bool, bool, bool, bool);

/// Normalized `PlayerState` observation: `(dimension, server_tick, yaw_bits,
/// pitch_bits, on_ground, ready, reset)`.
type NormalizedPlayerState = (u8, u64, u32, u32, bool, bool, bool);

/// Normalized hostile spawn record: `(id, dimension, health, yaw_bits)`.
type NormalizedHostileSpawnRecord = (u64, u8, u8, u32);

/// Normalized companion state batch: `(tick, published identity order)`.
type NormalizedCompanionStates = (u64, Vec<[u8; 16]>);

/// Normalized chat event: `(event_id, kind, reject_reason)`.
type NormalizedChatEvent = (u64, u8, u8);

/// Normalized command rejection: `(sequence, reason wire id)`.
type NormalizedCommandRejected = (u64, u8);

/// Asserts that a normalized expectation is sensitive to exactly the field a
/// mutation changes.
///
/// The unmutated clone must agree with the actual observation and the mutated
/// clone must not: a mutation the comparison cannot see would mean the
/// normalized form dropped the field, which is the drift this pin forbids.
fn assert_mutation_is_visible<T: Clone + PartialEq + std::fmt::Debug>(
    actual: &T,
    mutate: impl FnOnce(&mut T),
) {
    let mut expected = actual.clone();
    assert_eq!(
        &expected, actual,
        "the unmutated expectation must agree with the actual observation"
    );
    mutate(&mut expected);
    assert_ne!(
        &expected, actual,
        "a mutated expectation must not agree with the actual observation"
    );
}

fn player_id_with_seed(seed: u8) -> PlayerId {
    let mut bytes = [0u8; 16];
    for (index, byte) in bytes.iter_mut().enumerate() {
        *byte = index as u8 + seed;
    }
    bytes[6] = 0x4a;
    bytes[8] = 0x81;
    PlayerId::try_from_bytes(bytes).expect("a UUIDv4 player identity")
}

fn player_id() -> PlayerId {
    player_id_with_seed(1)
}

fn companion_id(seed: u8) -> CompanionId {
    let mut bytes = [0u8; 16];
    for (index, byte) in bytes.iter_mut().enumerate() {
        *byte = index as u8 + seed;
    }
    bytes[6] = 0x4a;
    bytes[8] = 0x81;
    CompanionId::try_from_bytes(bytes).expect("a UUIDv4 companion identity")
}

fn display_name() -> DisplayName {
    DisplayName::try_from_canonical("Player".to_owned()).expect("a display name")
}

fn companion_name() -> CompanionName {
    CompanionName::try_from_canonical("Rook".to_owned()).expect("a companion name")
}

fn command_text(text: &str) -> mornlea_domain::CommandText {
    mornlea_domain::CommandText::try_from_canonical(text.to_owned()).expect("a command text")
}

fn chat_intent() -> mornlea_domain::ChatIntent {
    mornlea_domain::ChatIntent::try_new(command_text("@Rook mine stone")).unwrap()
}

fn look() -> LookAngles {
    LookAngles::try_new(-0.0, 1.5).expect("finite look angles")
}

fn position() -> FiniteVec3 {
    FiniteVec3::try_new([1.0, -2.0, 3.0]).expect("a finite position")
}

fn velocity() -> FiniteVec3 {
    FiniteVec3::try_new([-0.5, 0.25, 4.0]).expect("a finite velocity")
}

fn wire_container_ref() -> ContainerRef {
    ContainerRef {
        dimension: 0,
        chunk_x: 3,
        chunk_z: -4,
        kind: CONTAINER_KIND_FURNACE,
        slot: 2,
        generation: 7,
    }
}

fn domain_container_ref() -> DomainContainerRef {
    DomainContainerRef::try_new(ChunkPos::new(3, -4), ContainerKind::Furnace, 2, 7)
        .expect("a valid furnace reference")
}

fn sequenced(command: Command) -> PlayIntent {
    PlayIntent::Sequenced {
        sequence: 7,
        command,
    }
}

fn player_input_command() -> Command {
    Command::PlayerInput(PlayerControl::new(PlayerControlParts {
        movement: Movement {
            move_x: 1,
            move_z: -1,
            jump: true,
        },
        look: look(),
        actions: HeldActions {
            primary: true,
            eating: false,
            sprinting: true,
            sneaking: false,
        },
    }))
}

/// The nineteen sequenced domain commands, one per registered client command
/// that carries a sequence.
fn sequenced_commands() -> Vec<Command> {
    vec![
        player_input_command(),
        Command::PlaceBlock(PlacementIntent::try_new(look(), 3).expect("a placement intent")),
        Command::Resync(
            ResyncIntent::try_new(0, ChunkPos::new(-2, 5), 9).expect("a resync intent"),
        ),
        Command::SelectHotbar(HotbarSlot::new(8).expect("a hotbar slot")),
        Command::MoveInventory(InventoryMove::try_new(0, 35).expect("an inventory move")),
        Command::MoveCrafting(CraftingMove::try_new(0, 9).expect("a crafting move")),
        Command::OpenContainer(look()),
        Command::MoveContainer(
            mornlea_domain::ContainerMove::try_new(
                ChunkPos::new(3, -4),
                ContainerKind::Furnace,
                2,
                7,
                0,
                36,
            )
            .expect("a container move"),
        ),
        Command::CloseContainer,
        Command::DropSelectedItem,
        Command::TakeCraftingOutput,
        Command::TillSoil(look()),
        Command::BoneMeal(look()),
        Command::CollectWater(look()),
        Command::PlaceWater(look()),
        Command::EquipArmor,
        Command::MovePartial(
            PartialMove::try_new(StackView::Inventory, 0, 5, true).expect("a partial move"),
        ),
        Command::QuickMove(
            StackSource::try_new(StackView::Crafting, 2).expect("a quick-move source"),
        ),
        Command::DropStack(
            StackSource::try_new(StackView::Container(domain_container_ref()), 4)
                .expect("a drop-stack source"),
        ),
    ]
}

/// The name of one domain command, pinned by an exhaustive match so a new
/// command variant fails to compile here instead of silently passing.
fn command_name(command: &Command) -> &'static str {
    match command {
        Command::PlayerInput(_) => "PlayerInput",
        Command::PlaceBlock(_) => "PlaceBlock",
        Command::Resync(_) => "Resync",
        Command::SelectHotbar(_) => "SelectHotbar",
        Command::OpenContainer(_) => "OpenContainer",
        Command::TillSoil(_) => "TillSoil",
        Command::BoneMeal(_) => "BoneMeal",
        Command::CollectWater(_) => "CollectWater",
        Command::PlaceWater(_) => "PlaceWater",
        Command::MoveInventory(_) => "MoveInventory",
        Command::MoveCrafting(_) => "MoveCrafting",
        Command::MoveContainer(_) => "MoveContainer",
        Command::CloseContainer => "CloseContainer",
        Command::DropSelectedItem => "DropSelectedItem",
        Command::TakeCraftingOutput => "TakeCraftingOutput",
        Command::EquipArmor => "EquipArmor",
        Command::MovePartial(_) => "MovePartial",
        Command::QuickMove(_) => "QuickMove",
        Command::DropStack(_) => "DropStack",
    }
}

/// The label one converted intent publishes: the command name, `Chat` or
/// `KeepAliveReply`.
fn intent_label(intent: &PlayIntent) -> &'static str {
    match intent {
        PlayIntent::Sequenced { command, .. } => command_name(command),
        PlayIntent::Chat(_) => "Chat",
        PlayIntent::KeepAliveReply { .. } => "KeepAliveReply",
    }
}

/// The label one converted event publishes, pinned by an exhaustive match over
/// the closed 30-variant set.
fn event_label(event: &Event) -> &'static str {
    match event {
        Event::ChunkSnapshot(_) => "ChunkSnapshot",
        Event::BlockChanges(_) => "BlockChanges",
        Event::ForgetChunks(_) => "ForgetChunks",
        Event::PlayerState(_) => "PlayerState",
        Event::CommandRejected(_) => "CommandRejected",
        Event::RemotePlayerSpawn(_) => "RemotePlayerSpawn",
        Event::RemotePlayerDespawn(_) => "RemotePlayerDespawn",
        Event::RemotePlayerStates(_) => "RemotePlayerStates",
        Event::InventoryState(_) => "InventoryState",
        Event::ItemDropUpserts(_) => "ItemDropUpserts",
        Event::ItemDropRemoves(_) => "ItemDropRemoves",
        Event::FurnaceState(_) => "FurnaceState",
        Event::ContainerClosed(_) => "ContainerClosed",
        Event::ChestState(_) => "ChestState",
        Event::Chat(_) => "Chat",
        Event::CompanionSpawn(_) => "CompanionSpawn",
        Event::CompanionStates(_) => "CompanionStates",
        Event::CompanionDespawn(_) => "CompanionDespawn",
        Event::PlaceBlockSucceeded(_) => "PlaceBlockSucceeded",
        Event::CraftingState(_) => "CraftingState",
        Event::HostileSpawn(_) => "HostileSpawn",
        Event::HostileState(_) => "HostileState",
        Event::HostileDespawn(_) => "HostileDespawn",
        Event::CombatHit(_) => "CombatHit",
        Event::PassiveSpawn(_) => "PassiveSpawn",
        Event::PassiveState(_) => "PassiveState",
        Event::PassiveDespawn(_) => "PassiveDespawn",
        Event::ProjectileSpawn(_) => "ProjectileSpawn",
        Event::ProjectileState(_) => "ProjectileState",
        Event::ProjectileDespawn(_) => "ProjectileDespawn",
    }
}

/// The registry key one client command name publishes under, written out per
/// command so the table pins the family mapping independently of the packet
/// the adapter produced.
fn client_key(command: &str) -> u32 {
    match command {
        "PlayerInput" => 0,
        "PlaceBlock" => 2,
        "Resync" => 3,
        "KeepAliveReply" => 4,
        "SelectHotbar" => 5,
        "MoveInventory" => 6,
        "MoveCrafting" => 7,
        "OpenContainer" => 8,
        "MoveContainer" => 9,
        "CloseContainer" => 10,
        "DropSelectedItem" => 11,
        "Chat" => 12,
        "TillSoil" => 13,
        "BoneMeal" => 14,
        "TakeCraftingOutput" => 15,
        "CollectWater" => 16,
        "PlaceWater" => 17,
        "EquipArmor" => 18,
        "MovePartial" => 19,
        "QuickMove" => 20,
        "DropStack" => 21,
        other => panic!("unexpected client command name {other}"),
    }
}

/// The registry key one server event name publishes under, written out per
/// publication for the same reason.
fn server_key(event: &str) -> u32 {
    match event {
        "ChunkSnapshot" => 0,
        "BlockChanges" => 1,
        "ForgetChunks" => 2,
        "PlayerState" => 3,
        "CommandRejected" => 4,
        "RemotePlayerSpawn" => 7,
        "RemotePlayerDespawn" => 8,
        "RemotePlayerStates" => 9,
        "InventoryState" => 10,
        "ItemDropUpserts" => 11,
        "ItemDropRemoves" => 12,
        "FurnaceState" => 13,
        "ContainerClosed" => 14,
        "ChestState" => 15,
        "Chat" => 16,
        "CompanionSpawn" => 17,
        "CompanionStates" => 18,
        "CompanionDespawn" => 19,
        "PlaceBlockSucceeded" => 20,
        "CraftingState" => 21,
        "HostileSpawn" => 22,
        "HostileState" => 23,
        "HostileDespawn" => 24,
        "CombatHit" => 25,
        "PassiveSpawn" => 26,
        "PassiveState" => 27,
        "PassiveDespawn" => 28,
        "ProjectileSpawn" => 29,
        "ProjectileState" => 30,
        "ProjectileDespawn" => 31,
        other => panic!("unexpected server event name {other}"),
    }
}

fn client_play_key(id: u32) -> PacketKey {
    PacketKey {
        direction: Direction::ClientToServer,
        state: State::Play,
        id,
    }
}

fn server_play_key(id: u32) -> PacketKey {
    PacketKey {
        direction: Direction::ServerToClient,
        state: State::Play,
        id,
    }
}

/// Every registered client packet with the key it publishes under and the
/// intent label the adapter must answer with. `"Refused"` marks the two
/// negotiation packets, which have no play intent.
fn client_table() -> Vec<(ClientPacket, PacketKey, &'static str)> {
    let mut table = Vec::new();
    let hello = ClientHello::new(45).expect("a current client hello");
    let payload = hello.encode().expect("encode current hello");
    table.push((
        ClientPacket::ClientHello(ClientHello::decode_inbound(&payload).expect("an inbound hello")),
        PacketKey {
            direction: Direction::ClientToServer,
            state: State::Handshake,
            id: 0,
        },
        "Refused",
    ));
    let login = LoginStart::new(player_id(), "Player", 8).expect("a login start");
    let payload = login.encode().expect("encode current login");
    table.push((
        ClientPacket::LoginStart(
            LoginStart::decode_inbound(&payload).expect("an inbound login start"),
        ),
        PacketKey {
            direction: Direction::ClientToServer,
            state: State::Login,
            id: 0,
        },
        "Refused",
    ));
    for command in sequenced_commands() {
        let expected = command_name(&command);
        let packet = ClientPacket::try_from(sequenced(command)).expect("a client packet");
        table.push((packet, client_play_key(client_key(expected)), expected));
    }
    let packet = ClientPacket::try_from(PlayIntent::Chat(chat_intent())).expect("a chat command");
    table.push((packet, client_play_key(12), "Chat"));
    let packet = ClientPacket::try_from(PlayIntent::KeepAliveReply { token: 4242 })
        .expect("a keep alive reply");
    table.push((packet, client_play_key(4), "KeepAliveReply"));
    table
}

fn domain_sections() -> Box<[PalettedSection; 24]> {
    let sections: Vec<PalettedSection> = (0..24)
        .map(|_| PalettedSection::single(0).expect("a single air section"))
        .collect();
    sections
        .into_boxed_slice()
        .try_into()
        .expect("the fixed 24-section column")
}

fn domain_inventory() -> DomainInventoryState {
    DomainInventoryState::new(InventoryStateParts {
        selected: HotbarSlot::new(2).expect("a hotbar slot"),
        hotbar: [ItemStack::EMPTY; 9],
        backpack: [ItemStack::EMPTY; 27],
    })
}

fn domain_crafting() -> DomainCraftingState {
    DomainCraftingState::try_new(CraftingStateParts {
        size: CraftingSize::Workbench,
        slots: [ItemStack::EMPTY; 9],
        output: ItemStack::EMPTY,
    })
    .expect("a crafting state")
}

fn domain_furnace() -> DomainFurnaceState {
    DomainFurnaceState::try_new(FurnaceStateParts {
        container: domain_container_ref(),
        input: ItemStack::EMPTY,
        fuel: ItemStack::EMPTY,
        output: ItemStack::EMPTY,
        progress_ticks: 12,
        burn_ticks: 900,
    })
    .expect("a furnace state")
}

fn domain_chest() -> DomainChestState {
    DomainChestState::try_new(ChestStateParts {
        container: DomainContainerRef::try_new(ChunkPos::new(3, -4), ContainerKind::Chest, 1, 9)
            .expect("a chest reference"),
        items: [ItemStack::EMPTY; 27],
    })
    .expect("a chest state")
}

fn domain_player_state() -> DomainPlayerState {
    DomainPlayerState::new(PlayerStateParts {
        server_tick: 11,
        last_input_sequence: 4,
        dimension: Dimension::DEPTHS,
        motion: MotionState::new(MotionStateParts {
            position: position(),
            velocity: velocity(),
            on_ground: true,
        }),
        look: look(),
        ready: true,
        reset: false,
        mining: MiningState::try_new(MiningStateParts {
            active: false,
            target: BlockPos::ORIGIN,
            progress: 0,
            required: 0,
            harvestable: false,
        })
        .expect("an idle mining block"),
        survival: SurvivalState::try_new(SurvivalStateParts {
            health: 18,
            oxygen: 240,
            hunger: 15,
            saturation_zero: false,
            armor_points: 6,
        })
        .expect("a survival state"),
        world: WorldState::try_new(WorldStateParts {
            day_phase_offset: 6000,
            world_time_ticks: 12345,
            weather: Weather::Rain,
            season: Season::Autumn,
            season_progress: 40,
            temperature: -3,
        })
        .expect("a world state"),
    })
}

fn domain_block_changes(count: usize) -> DomainBlockChanges {
    let changes: Vec<DomainBlockChange> = (0..count)
        .map(|index| {
            let local_y = index / 256;
            let local_z = (index % 256) / 16;
            let local_x = index % 16;
            DomainBlockChange::try_new(
                BlockPos::new(local_x as i32, MIN_Y + local_y as i32, local_z as i32),
                0,
            )
            .expect("a registered block change")
        })
        .collect();
    DomainBlockChanges::try_new(BlockChangesParts {
        dimension: Dimension::OVERWORLD,
        chunk: ChunkPos::new(0, 0),
        base_revision: 4,
        new_revision: 5,
        changes: changes.into_boxed_slice(),
    })
    .expect("a domain-valid block-change batch")
}

fn domain_forget_chunks(count: usize) -> DomainForgetChunks {
    let chunks: Vec<ChunkPos> = (0..count)
        .map(|index| ChunkPos::new(index as i32, -(index as i32)))
        .collect();
    DomainForgetChunks::try_new(ForgetChunksParts {
        dimension: Dimension::OVERWORLD,
        chunks: chunks.into_boxed_slice(),
    })
    .expect("a domain-valid forget batch")
}

fn domain_remote_player_states(count: usize) -> DomainRemotePlayerStates {
    let states: Vec<DomainRemotePlayerState> = (0..count)
        .map(|index| {
            DomainRemotePlayerState::new(RemotePlayerStateParts {
                player_id: player_id_with_seed(index as u8 + 1),
                dimension: Dimension::OVERWORLD,
                position: position(),
                look: look(),
                reset: index % 2 == 0,
            })
        })
        .collect();
    DomainRemotePlayerStates::try_new(RemotePlayerStatesParts {
        server_tick: 21,
        states: states.into_boxed_slice(),
    })
    .expect("a domain-valid remote-player batch")
}

fn domain_companion_states(count: usize) -> DomainCompanionStates {
    let states: Vec<DomainCompanionState> = (0..count)
        .map(|index| {
            DomainCompanionState::try_new(CompanionStateParts {
                id: companion_id(index as u8 + 1),
                dimension: Dimension::OVERWORLD,
                position: position(),
                look: look(),
                reset: index % 2 == 0,
            })
            .expect("a companion state record")
        })
        .collect();
    DomainCompanionStates::try_new(CompanionStatesParts {
        server_tick: 31,
        states: states.into_boxed_slice(),
    })
    .expect("a domain-valid companion batch")
}

fn domain_hostile_spawn(count: usize) -> DomainHostileSpawn {
    let spawns: Vec<DomainHostileSpawnRecord> = (0..count)
        .map(|index| {
            DomainHostileSpawnRecord::try_new(HostileSpawnRecordParts {
                id: HostileId::try_new(index as u64 + 1).expect("a hostile identity"),
                dimension: Dimension::OVERWORLD,
                position: position(),
                yaw: 0.5,
                health: 12,
                kind: HostileKind::Nightwalker,
            })
            .expect("a hostile spawn record")
        })
        .collect();
    DomainHostileSpawn::try_new(HostileSpawnParts {
        server_tick: 41,
        spawns: spawns.into_boxed_slice(),
    })
    .expect("a domain-valid hostile spawn batch")
}

fn domain_hostile_state(count: usize) -> DomainHostileState {
    let states: Vec<DomainHostileStateRecord> = (0..count)
        .map(|index| {
            DomainHostileStateRecord::try_new(HostileStateRecordParts {
                id: HostileId::try_new(index as u64 + 1).expect("a hostile identity"),
                position: position(),
                velocity: velocity(),
                yaw: -0.25,
                health: 9,
                kind: HostileKind::BoneThrower,
            })
            .expect("a hostile state record")
        })
        .collect();
    DomainHostileState::try_new(HostileStateParts {
        server_tick: 42,
        states: states.into_boxed_slice(),
    })
    .expect("a domain-valid hostile state batch")
}

fn domain_hostile_despawn(count: usize) -> DomainHostileDespawn {
    let ids: Vec<HostileId> = (0..count)
        .map(|index| HostileId::try_new(index as u64 + 1).expect("a hostile identity"))
        .collect();
    DomainHostileDespawn::try_new(HostileDespawnParts {
        server_tick: 43,
        ids: ids.into_boxed_slice(),
    })
    .expect("a domain-valid hostile despawn batch")
}

fn domain_passive_spawn(count: usize) -> DomainPassiveSpawn {
    let spawns: Vec<DomainPassiveSpawnRecord> = (0..count)
        .map(|index| {
            DomainPassiveSpawnRecord::try_new(PassiveSpawnRecordParts {
                id: PassiveId::try_new(index as u64 + 1).expect("a passive identity"),
                dimension: Dimension::OVERWORLD,
                position: position(),
                yaw: 1.5,
                health: 14,
            })
            .expect("a passive spawn record")
        })
        .collect();
    DomainPassiveSpawn::try_new(PassiveSpawnParts {
        server_tick: 51,
        spawns: spawns.into_boxed_slice(),
    })
    .expect("a domain-valid passive spawn batch")
}

fn domain_passive_state(count: usize) -> DomainPassiveState {
    let states: Vec<DomainPassiveStateRecord> = (0..count)
        .map(|index| {
            DomainPassiveStateRecord::try_new(PassiveStateRecordParts {
                id: PassiveId::try_new(index as u64 + 1).expect("a passive identity"),
                position: position(),
                velocity: velocity(),
                yaw: -1.5,
                health: 7,
                grazing: index % 2 == 0,
            })
            .expect("a passive state record")
        })
        .collect();
    DomainPassiveState::try_new(PassiveStateParts {
        server_tick: 52,
        states: states.into_boxed_slice(),
    })
    .expect("a domain-valid passive state batch")
}

fn domain_passive_despawn(count: usize) -> DomainPassiveDespawn {
    let despawns: Vec<DomainPassiveDespawnRecord> = (0..count)
        .map(|index| {
            DomainPassiveDespawnRecord::new(
                PassiveId::try_new(index as u64 + 1).expect("a passive identity"),
                PassiveDespawnReason::Died,
            )
        })
        .collect();
    DomainPassiveDespawn::try_new(PassiveDespawnParts {
        server_tick: 53,
        despawns: despawns.into_boxed_slice(),
    })
    .expect("a domain-valid passive despawn batch")
}

fn domain_projectile_spawn(count: usize) -> DomainProjectileSpawn {
    let spawns: Vec<DomainProjectileSpawnRecord> = (0..count)
        .map(|index| {
            DomainProjectileSpawnRecord::new(ProjectileSpawnRecordParts {
                id: ProjectileId::try_new(index as u64 + 1).expect("a projectile identity"),
                kind: ProjectileKind::Arrow,
                dimension: Dimension::OVERWORLD,
                position: position(),
                velocity: velocity(),
            })
        })
        .collect();
    DomainProjectileSpawn::try_new(ProjectileSpawnParts {
        server_tick: 61,
        spawns: spawns.into_boxed_slice(),
    })
    .expect("a domain-valid projectile spawn batch")
}

fn domain_projectile_state(count: usize) -> DomainProjectileState {
    let states: Vec<DomainProjectileStateRecord> = (0..count)
        .map(|index| {
            DomainProjectileStateRecord::new(ProjectileStateRecordParts {
                id: ProjectileId::try_new(index as u64 + 1).expect("a projectile identity"),
                position: position(),
            })
        })
        .collect();
    DomainProjectileState::try_new(ProjectileStateParts {
        server_tick: 62,
        states: states.into_boxed_slice(),
    })
    .expect("a domain-valid projectile state batch")
}

fn domain_projectile_despawn(count: usize) -> DomainProjectileDespawn {
    let ids: Vec<ProjectileId> = (0..count)
        .map(|index| ProjectileId::try_new(index as u64 + 1).expect("a projectile identity"))
        .collect();
    DomainProjectileDespawn::try_new(ProjectileDespawnParts {
        server_tick: 63,
        ids: ids.into_boxed_slice(),
    })
    .expect("a domain-valid projectile despawn batch")
}

fn domain_item_drop_upserts(count: usize) -> DomainItemDropUpserts {
    let drops: Vec<DomainItemDrop> = (0..count)
        .map(|index| {
            DomainItemDrop::try_new(ItemDropParts {
                id: DropId::try_new(0, ChunkPos::new(0, 0), 0, index as u32 + 1)
                    .expect("a drop identity"),
                block_index: index as u32,
                stack: ItemStack::EMPTY,
            })
            .expect("an item drop")
        })
        .collect();
    DomainItemDropUpserts::try_new(ItemDropUpsertsParts {
        server_tick: 71,
        drops: drops.into_boxed_slice(),
    })
    .expect("a domain-valid drop batch")
}

fn domain_item_drop_removes(count: usize) -> DomainItemDropRemoves {
    let ids: Vec<DropId> = (0..count)
        .map(|index| {
            DropId::try_new(0, ChunkPos::new(0, 0), 0, index as u32 + 1).expect("a drop identity")
        })
        .collect();
    DomainItemDropRemoves::try_new(ItemDropRemovesParts {
        server_tick: 72,
        ids: ids.into_boxed_slice(),
    })
    .expect("a domain-valid drop-remove batch")
}

fn domain_chat_event() -> DomainChatEvent {
    DomainChatEvent::try_new(ChatEventParts {
        event_id: 5,
        player_id: player_id(),
        player_name: display_name(),
        body: ChatBody::Accepted {
            companion: CompanionSpeaker::new(companion_id(1), companion_name()),
            command: command_text("mine stone"),
        },
    })
    .expect("a chat event")
}

/// All thirty closed domain events, one per registered server publication.
fn domain_events() -> Vec<Event> {
    vec![
        Event::ChunkSnapshot(
            DomainChunkSnapshot::try_new(ChunkSnapshotParts {
                dimension: Dimension::OVERWORLD,
                chunk: ChunkPos::new(1, -1),
                revision: 3,
                sections: domain_sections(),
            })
            .expect("a chunk snapshot"),
        ),
        Event::BlockChanges(domain_block_changes(2)),
        Event::ForgetChunks(domain_forget_chunks(2)),
        Event::PlayerState(domain_player_state()),
        Event::CommandRejected(CommandRejection::new(6, RejectReason::InvalidRay)),
        Event::RemotePlayerSpawn(DomainRemotePlayerSpawn::new(RemotePlayerSpawnParts {
            player_id: player_id(),
            display_name: display_name(),
            server_tick: 12,
            dimension: Dimension::OVERWORLD,
            position: position(),
            look: look(),
        })),
        Event::RemotePlayerDespawn(DomainRemotePlayerDespawn::new(player_id())),
        Event::RemotePlayerStates(domain_remote_player_states(2)),
        Event::InventoryState(domain_inventory()),
        Event::ItemDropUpserts(domain_item_drop_upserts(2)),
        Event::ItemDropRemoves(domain_item_drop_removes(2)),
        Event::FurnaceState(domain_furnace()),
        Event::ContainerClosed(DomainContainerClosed::new(domain_container_ref())),
        Event::ChestState(domain_chest()),
        Event::Chat(domain_chat_event()),
        Event::CompanionSpawn(
            DomainCompanionSpawn::try_new(CompanionSpawnParts {
                id: companion_id(1),
                name: companion_name(),
                server_tick: 13,
                dimension: Dimension::OVERWORLD,
                position: position(),
                look: look(),
            })
            .expect("a companion spawn"),
        ),
        Event::CompanionStates(domain_companion_states(2)),
        Event::CompanionDespawn(DomainCompanionDespawn::new(companion_id(1))),
        Event::PlaceBlockSucceeded(PlacementSuccess::new(8)),
        Event::CraftingState(domain_crafting()),
        Event::HostileSpawn(domain_hostile_spawn(2)),
        Event::HostileState(domain_hostile_state(2)),
        Event::HostileDespawn(domain_hostile_despawn(2)),
        Event::CombatHit(
            DomainCombatHit::try_new(14, 5, CombatTarget::Hostile).expect("a combat hit"),
        ),
        Event::PassiveSpawn(domain_passive_spawn(2)),
        Event::PassiveState(domain_passive_state(2)),
        Event::PassiveDespawn(domain_passive_despawn(2)),
        Event::ProjectileSpawn(domain_projectile_spawn(2)),
        Event::ProjectileState(domain_projectile_state(2)),
        Event::ProjectileDespawn(domain_projectile_despawn(2)),
    ]
}

/// Every registered server packet with the key it publishes under and the
/// event label the adapter must answer with. `"Refused"` marks the six
/// control families, which are not semantic publications.
fn server_table() -> Vec<(ServerPacket, PacketKey, &'static str)> {
    let mut table = vec![
        (
            ServerPacket::ServerHello(ServerHello::new(45).expect("a server hello")),
            PacketKey {
                direction: Direction::ServerToClient,
                state: State::Handshake,
                id: 0,
            },
            "Refused",
        ),
        (
            ServerPacket::HandshakeReject(
                HandshakeReject::new(45, HANDSHAKE_VERSION_MISMATCH, "version")
                    .expect("a handshake reject"),
            ),
            PacketKey {
                direction: Direction::ServerToClient,
                state: State::Handshake,
                id: 1,
            },
            "Refused",
        ),
        (
            ServerPacket::LoginSuccess(LoginSuccess::new(player_id(), 7)),
            PacketKey {
                direction: Direction::ServerToClient,
                state: State::Login,
                id: 0,
            },
            "Refused",
        ),
        (
            ServerPacket::LoginReject(
                LoginReject::new(LOGIN_SERVER_FULL, "full").expect("a login reject"),
            ),
            PacketKey {
                direction: Direction::ServerToClient,
                state: State::Login,
                id: 1,
            },
            "Refused",
        ),
        (
            ServerPacket::KeepAlive(mornlea_protocol::KeepAlive::new(9).expect("a keep alive")),
            server_play_key(5),
            "Refused",
        ),
        (
            ServerPacket::Disconnect(
                Disconnect::new(DISCONNECT_SLOW_CLIENT, "slow").expect("a disconnect"),
            ),
            server_play_key(6),
            "Refused",
        ),
    ];
    for event in domain_events() {
        let expected = event_label(&event);
        let packet = ServerPacket::try_from(event).expect("a server packet");
        table.push((packet, server_play_key(server_key(expected)), expected));
    }
    table
}

fn normalize_player_input(intent: &PlayIntent) -> NormalizedPlayerInput {
    let PlayIntent::Sequenced { sequence, command } = intent else {
        panic!("a sequenced intent");
    };
    let Command::PlayerInput(control) = command else {
        panic!("a player input command");
    };
    (
        *sequence,
        control.movement().move_x,
        control.movement().move_z,
        control.movement().jump,
        control.look().yaw().to_bits(),
        control.look().pitch().to_bits(),
        control.actions().primary,
        control.actions().eating,
        control.actions().sprinting,
        control.actions().sneaking,
    )
}

fn normalize_player_state(event: &Event) -> NormalizedPlayerState {
    let Event::PlayerState(state) = event else {
        panic!("a player state event");
    };
    (
        state.dimension().get(),
        state.server_tick(),
        state.look().yaw().to_bits(),
        state.look().pitch().to_bits(),
        state.motion().on_ground(),
        state.ready(),
        state.reset(),
    )
}

fn normalize_hostile_spawn(event: &Event) -> Vec<NormalizedHostileSpawnRecord> {
    let Event::HostileSpawn(batch) = event else {
        panic!("a hostile spawn event");
    };
    batch
        .spawns()
        .iter()
        .map(|record| {
            (
                record.id().get(),
                record.dimension().get(),
                record.health(),
                record.yaw().to_bits(),
            )
        })
        .collect()
}

fn normalize_companion_states(event: &Event) -> NormalizedCompanionStates {
    let Event::CompanionStates(batch) = event else {
        panic!("a companion states event");
    };
    (
        batch.server_tick(),
        batch
            .states()
            .iter()
            .map(|record| record.id().bytes())
            .collect(),
    )
}

fn normalize_chat_event(event: &Event) -> NormalizedChatEvent {
    let Event::Chat(event) = event else {
        panic!("a chat event");
    };
    let (kind, reason) = match event.body() {
        ChatBody::Accepted { .. } => (CHAT_EVENT_ACCEPTED, CHAT_REJECT_NONE),
        ChatBody::InvalidFormat => (CHAT_EVENT_REJECTED, CHAT_REJECT_INVALID_FORMAT),
        ChatBody::UnknownCompanion { .. } => (CHAT_EVENT_REJECTED, 2),
        ChatBody::QueueFull { .. } => (CHAT_EVENT_REJECTED, 4),
        ChatBody::NotFollowing { .. } => (CHAT_EVENT_REJECTED, 5),
        ChatBody::Task { state, .. } => match state {
            TaskState::Started => (3, CHAT_REJECT_NONE),
            TaskState::Progress => (4, CHAT_REJECT_NONE),
            TaskState::Completed => (5, CHAT_REJECT_NONE),
            TaskState::TimedOut => (7, CHAT_REJECT_NONE),
            TaskState::Stopped => (8, CHAT_REJECT_NONE),
            TaskState::Failed(_) => (6, 16),
        },
        ChatBody::Speech { .. } => (9, CHAT_REJECT_NONE),
    };
    (event.event_id(), kind, reason)
}

fn normalize_command_rejected(event: &Event) -> NormalizedCommandRejected {
    let Event::CommandRejected(rejection) = event else {
        panic!("a command rejected event");
    };
    (rejection.sequence(), rejection.reason().wire_id())
}

#[test]
fn semantic_every_client_packet_maps_to_exactly_one_intent() {
    let table = client_table();
    let mut sequenced = 0usize;
    let mut chat = 0usize;
    let mut keep_alive = 0usize;
    let mut refused = 0usize;
    for (packet, expected_key, expected) in &table {
        assert_eq!(
            &packet.key(),
            expected_key,
            "the packet publishes the key the table names"
        );
        match PlayIntent::try_from(packet.clone()) {
            Ok(intent) => {
                assert_eq!(intent_label(&intent), *expected);
                match intent {
                    PlayIntent::Sequenced { .. } => sequenced += 1,
                    PlayIntent::Chat(_) => chat += 1,
                    PlayIntent::KeepAliveReply { .. } => keep_alive += 1,
                }
            }
            Err(error) => {
                assert_eq!(*expected, "Refused");
                assert_eq!(error, ProtocolError::UnknownPacket);
                refused += 1;
            }
        }
    }
    assert_eq!(
        (sequenced, chat, keep_alive, refused),
        (19, 1, 1, 2),
        "19 sequenced commands, one chat intent, one keep alive reply, two refusals"
    );
    assert_eq!(table.len(), 23, "every registered client key is enumerated");
}

#[test]
fn semantic_every_server_packet_maps_to_exactly_one_event() {
    let table = server_table();
    let mut events = 0usize;
    let mut refused = 0usize;
    for (packet, expected_key, expected) in &table {
        assert_eq!(
            &packet.key(),
            expected_key,
            "the packet publishes the key the table names"
        );
        match Event::try_from(packet.clone()) {
            Ok(event) => {
                assert_eq!(event_label(&event), *expected);
                events += 1;
            }
            Err(error) => {
                assert_eq!(*expected, "Refused");
                assert_eq!(error, ProtocolError::UnknownPacket);
                refused += 1;
            }
        }
    }
    assert_eq!(
        (events, refused),
        (30, 6),
        "30 publications and six control refusals"
    );
    assert_eq!(table.len(), 36, "every registered server key is enumerated");
}

#[test]
fn semantic_every_client_intent_round_trips_through_the_wire() {
    for (packet, _, expected) in client_table() {
        if expected == "Refused" {
            continue;
        }
        let intent = PlayIntent::try_from(packet.clone()).expect("a converted intent");
        let rebuilt = ClientPacket::try_from(intent.clone()).expect("a rebuilt packet");
        let mut first = vec![0u8; 4096];
        let mut second = vec![0u8; 4096];
        let written_first = encode_client_into(&packet, &mut first).expect("the original encoding");
        let written_second =
            encode_client_into(&rebuilt, &mut second).expect("the rebuilt encoding");
        assert_eq!(
            &first[..written_first],
            &second[..written_second],
            "the rebuilt packet publishes the same bytes"
        );
        let key = rebuilt.key();
        let decoded =
            decode_client(key.state, key.id, &second[..written_second]).expect("the bytes decode");
        assert_eq!(
            PlayIntent::try_from(decoded).expect("the decoded packet converts"),
            intent,
            "the intent survives a full wire round trip"
        );
    }
}

#[test]
fn semantic_every_event_round_trips_through_the_wire() {
    let mut codec = ProtocolCodec::new().expect("a codec context pair");
    let mut buffer = vec![0u8; 1 << 20];
    for (packet, _, expected) in server_table() {
        if expected == "Refused" {
            continue;
        }
        let event = Event::try_from(packet.clone()).expect("a converted event");
        let rebuilt = ServerPacket::try_from(event.clone()).expect("a rebuilt packet");
        let written = codec
            .encode_server_into(&rebuilt, &mut buffer)
            .expect("the rebuilt encoding");
        let key = rebuilt.key();
        let decoded = codec
            .decode_server(key.state, key.id, &buffer[..written])
            .expect("the rebuilt bytes decode");
        assert_eq!(
            Event::try_from(decoded).expect("the decoded packet converts"),
            event,
            "the event survives a full wire round trip"
        );
    }
}

#[test]
fn semantic_a_zero_sequence_is_carried_verbatim_for_the_commands_that_admit_it() {
    let intent = PlayIntent::Sequenced {
        sequence: 0,
        command: player_input_command(),
    };
    let packet = ClientPacket::try_from(intent.clone()).expect("a zero-sequence player input");
    let ClientPacket::PlayerInput(input) = &packet else {
        panic!("a player input packet");
    };
    assert_eq!(input.sequence, 0, "the sequence is carried, not rewritten");
    let rebuilt = PlayIntent::try_from(packet).expect("the packet converts back");
    assert_eq!(rebuilt, intent);
}

#[test]
fn semantic_the_take_crafting_output_sequence_rule_stays_with_the_packet_and_the_envelope() {
    let intent = PlayIntent::Sequenced {
        sequence: 0,
        command: Command::TakeCraftingOutput,
    };
    assert_eq!(
        ClientPacket::try_from(intent).unwrap_err(),
        ProtocolError::InvalidRange,
        "the packet gate refuses a zero acknowledgement sequence"
    );
    let parts = mornlea_domain::CommandEnvelopeParts {
        tick: 1,
        session: 2,
        sequence: 0,
        arrival_index: 0,
        command: Command::TakeCraftingOutput,
    };
    assert!(
        mornlea_domain::CommandEnvelope::try_new(parts).is_err(),
        "the domain envelope refuses the same zero sequence"
    );
    let admitted = mornlea_domain::CommandEnvelopeParts {
        sequence: 1,
        ..parts
    };
    assert!(
        mornlea_domain::CommandEnvelope::try_new(admitted).is_ok(),
        "a nonzero sequence is admitted by the envelope"
    );
}

#[test]
fn semantic_a_foreign_container_dimension_fails_instead_of_aliasing_the_overworld() {
    for dimension in [256i32, -1, 1] {
        let reference = ContainerRef {
            dimension,
            ..wire_container_ref()
        };
        let move_command = MoveContainerStack {
            sequence: 3,
            container: reference,
            from: 0,
            to: 36,
        };
        assert_eq!(
            PlayIntent::try_from(ClientPacket::MoveContainerStack(move_command)).unwrap_err(),
            ProtocolError::InvalidRange,
            "dimension {dimension} is not the overworld and must not alias into it"
        );
        let partial = MoveStackPartial {
            sequence: 3,
            container: reference,
            view: STACK_VIEW_CONTAINER,
            from: 0,
            to: 4,
            single: false,
        };
        assert_eq!(
            PlayIntent::try_from(ClientPacket::MoveStackPartial(partial)).unwrap_err(),
            ProtocolError::InvalidRange,
            "the container view refuses dimension {dimension} as well"
        );
        let furnace = FurnaceState {
            furnace: reference,
            input: ItemStack::EMPTY,
            fuel: ItemStack::EMPTY,
            output: ItemStack::EMPTY,
            progress_ticks: 1,
            burn_ticks: 2,
        };
        assert_eq!(
            Event::try_from(ServerPacket::FurnaceState(furnace)).unwrap_err(),
            ProtocolError::InvalidRange,
            "a server publication refuses dimension {dimension} as well"
        );
    }
    let unknown_kind = ContainerRef {
        kind: 2,
        ..wire_container_ref()
    };
    let move_command = MoveContainerStack {
        sequence: 3,
        container: unknown_kind,
        from: 0,
        to: 36,
    };
    assert_eq!(
        PlayIntent::try_from(ClientPacket::MoveContainerStack(move_command)).unwrap_err(),
        ProtocolError::InvalidEnum,
        "an unknown kind is an enum boundary, not a range one"
    );
}

#[test]
fn semantic_the_absent_companion_identity_maps_to_variant_shaped_absence() {
    let invalid_format = ChatEvent {
        event_id: 3,
        player_id: player_id(),
        player_name: "Player".to_owned(),
        companion_id: [0u8; 16],
        companion_name: String::new(),
        kind: CHAT_EVENT_REJECTED,
        reject_reason: CHAT_REJECT_INVALID_FORMAT,
        command: String::new(),
        speech: String::new(),
    };
    let event = Event::try_from(ServerPacket::ChatEvent(invalid_format)).expect("a chat event");
    let Event::Chat(chat) = &event else {
        panic!("a chat event");
    };
    assert_eq!(
        chat.body(),
        &ChatBody::InvalidFormat,
        "the malformed branch carries no companion at all"
    );

    let accepted = ChatEvent {
        event_id: 4,
        player_id: player_id(),
        player_name: "Player".to_owned(),
        companion_id: companion_id(1).bytes(),
        companion_name: "Rook".to_owned(),
        kind: CHAT_EVENT_ACCEPTED,
        reject_reason: CHAT_REJECT_NONE,
        command: "mine stone".to_owned(),
        speech: String::new(),
    };
    let event = Event::try_from(ServerPacket::ChatEvent(accepted)).expect("a chat event");
    let Event::Chat(chat) = &event else {
        panic!("a chat event");
    };
    let ChatBody::Accepted { companion, .. } = chat.body() else {
        panic!("an accepted branch");
    };
    assert_eq!(
        companion.id(),
        companion_id(1),
        "a companion-bearing branch publishes the checked identity"
    );
}

#[test]
fn semantic_a_mutated_sequence_fails_the_normalized_comparison() {
    let intent = sequenced(player_input_command());
    let actual = normalize_player_input(&intent);
    assert_mutation_is_visible(&actual, |expected| expected.0 = actual.0 + 1);
}

#[test]
fn semantic_a_mutated_world_dimension_fails_the_normalized_comparison() {
    let event = Event::PlayerState(domain_player_state());
    let actual = normalize_player_state(&event);
    assert_mutation_is_visible(&actual, |expected| expected.0 = actual.0 ^ 1);
}

#[test]
fn semantic_a_mutated_actor_health_fails_the_normalized_comparison() {
    let event = Event::HostileSpawn(domain_hostile_spawn(2));
    let actual = normalize_hostile_spawn(&event);
    assert_mutation_is_visible(&actual, |expected| expected[0].2 = actual[0].2 + 1);
}

#[test]
fn semantic_a_mutated_record_order_fails_the_normalized_comparison() {
    let event = Event::CompanionStates(domain_companion_states(2));
    let actual = normalize_companion_states(&event);
    assert_mutation_is_visible(&actual, |expected| expected.1.reverse());
}

#[test]
fn semantic_a_mutated_chat_kind_fails_the_normalized_comparison() {
    let event = Event::Chat(domain_chat_event());
    let actual = normalize_chat_event(&event);
    assert_mutation_is_visible(&actual, |expected| expected.1 = actual.1 + 1);
}

#[test]
fn semantic_a_mutated_chat_reject_reason_fails_the_normalized_comparison() {
    let event = Event::Chat(
        DomainChatEvent::try_new(ChatEventParts {
            event_id: 6,
            player_id: player_id(),
            player_name: display_name(),
            body: ChatBody::InvalidFormat,
        })
        .expect("a chat event"),
    );
    let actual = normalize_chat_event(&event);
    assert_mutation_is_visible(&actual, |expected| expected.2 = actual.2 + 1);
}

#[test]
fn semantic_a_mutated_reject_reason_fails_the_normalized_comparison() {
    let event = Event::CommandRejected(CommandRejection::new(6, RejectReason::InvalidRay));
    let actual = normalize_command_rejected(&event);
    assert_mutation_is_visible(&actual, |expected| expected.1 = actual.1 + 1);
}

#[test]
fn semantic_no_conversion_constructs_a_command_envelope() {
    // The pin is by absence: the adapter may explain in prose why it does not
    // build the ordering envelope, but it may never import, name or construct
    // one, because intake metadata is the ordering layer's to assign.
    let source = include_str!("../src/semantic.rs");
    for needle in [
        "CommandEnvelope::",
        "CommandEnvelope {",
        "CommandEnvelopeParts",
        "CommandEnvelope,",
        "CommandEnvelope>",
    ] {
        assert!(
            !source.contains(needle),
            "the adapter must never import, name or construct the ordering envelope"
        );
    }
}

#[test]
fn semantic_a_five_record_companion_batch_rejects_one_wire_conversion() {
    let batch = domain_companion_states(5);
    assert_eq!(
        ServerPacket::try_from(Event::CompanionStates(batch)).unwrap_err(),
        ProtocolError::InvalidRange,
        "the four-record companion cap fires before any record is copied"
    );
    let admitted = domain_companion_states(4);
    assert!(
        ServerPacket::try_from(Event::CompanionStates(admitted)).is_ok(),
        "a four-record batch is one wire packet"
    );
}

#[test]
fn semantic_every_wire_batch_cap_fires_before_the_copy() {
    let cases: Vec<(&str, usize, usize, fn(usize) -> Event)> = vec![
        ("companion states", 4, 5, |count| {
            Event::CompanionStates(domain_companion_states(count))
        }),
        ("remote player states", 7, 8, |count| {
            Event::RemotePlayerStates(domain_remote_player_states(count))
        }),
        ("item drop upserts", 32, 33, |count| {
            Event::ItemDropUpserts(domain_item_drop_upserts(count))
        }),
        ("item drop removes", 32, 33, |count| {
            Event::ItemDropRemoves(domain_item_drop_removes(count))
        }),
        ("hostile spawn", 64, 65, |count| {
            Event::HostileSpawn(domain_hostile_spawn(count))
        }),
        ("hostile state", 64, 65, |count| {
            Event::HostileState(domain_hostile_state(count))
        }),
        ("hostile despawn", 64, 65, |count| {
            Event::HostileDespawn(domain_hostile_despawn(count))
        }),
        ("passive spawn", 64, 65, |count| {
            Event::PassiveSpawn(domain_passive_spawn(count))
        }),
        ("passive state", 64, 65, |count| {
            Event::PassiveState(domain_passive_state(count))
        }),
        ("passive despawn", 64, 65, |count| {
            Event::PassiveDespawn(domain_passive_despawn(count))
        }),
        ("projectile spawn", 128, 129, |count| {
            Event::ProjectileSpawn(domain_projectile_spawn(count))
        }),
        ("projectile state", 128, 129, |count| {
            Event::ProjectileState(domain_projectile_state(count))
        }),
        ("projectile despawn", 128, 129, |count| {
            Event::ProjectileDespawn(domain_projectile_despawn(count))
        }),
        ("block changes", 4096, 4097, |count| {
            Event::BlockChanges(domain_block_changes(count))
        }),
        ("forget chunks", 4096, 4097, |count| {
            Event::ForgetChunks(domain_forget_chunks(count))
        }),
    ];
    for (name, admitted, refused, build) in cases {
        assert!(
            ServerPacket::try_from(build(admitted)).is_ok(),
            "{name}: {admitted} records are one wire packet"
        );
        if refused <= MAX_SEMANTIC_BATCH_RECORDS {
            assert_eq!(
                ServerPacket::try_from(build(refused)).unwrap_err(),
                ProtocolError::InvalidRange,
                "{name}: {refused} records exceed the wire cap and are refused, never truncated"
            );
        }
    }
}

#[test]
fn semantic_a_domain_batch_above_the_semantic_work_cap_never_reaches_the_adapter() {
    let oversized: Vec<DomainBlockChange> = (0..4097)
        .map(|_| DomainBlockChange::try_new(BlockPos::new(0, MIN_Y, 0), 0).expect("a block change"))
        .collect();
    assert_eq!(
        DomainBlockChanges::try_new(BlockChangesParts {
            dimension: Dimension::OVERWORLD,
            chunk: ChunkPos::new(0, 0),
            base_revision: 1,
            new_revision: 2,
            changes: oversized.into_boxed_slice(),
        })
        .unwrap_err(),
        DomainError::BatchTooLarge,
        "the domain work cap refuses the batch before the wire cap is consulted"
    );
}
