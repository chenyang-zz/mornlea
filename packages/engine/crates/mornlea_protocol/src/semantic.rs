//! The exhaustive wire-to-semantic adapters.
//!
//! This module is the seam between the typed packet registry in
//! [`crate::registry`] and the checked semantic values `mornlea_domain` owns.
//! Three `TryFrom` pairs cross it:
//!
//! * [`TryFrom<ClientPacket> for PlayIntent`] turns one decoded client record
//!   into the intent the authority admits: a sequenced command, a chat intent
//!   or a keep alive reply. The two negotiation records have no play intent
//!   and are refused.
//! * [`TryFrom<ServerPacket> for Event`] turns one decoded server record into
//!   the semantic publication it names. The six control families are refused,
//!   because a hello, a rejection, a login answer, a keep alive and a
//!   disconnect are transport facts the domain publication set deliberately
//!   excludes.
//! * [`TryFrom<Event> for ServerPacket`] and [`TryFrom<PlayIntent> for
//!   ClientPacket`] are the outbound halves, so a runtime that holds checked
//!   values can publish them without naming a packet byte.
//!
//! Two invariants govern every arm.
//!
//! **No metadata is invented.** A conversion moves the fields the wire carries
//! and nothing else. It never constructs a session, a tick, an arrival index,
//! a routing recipient or an authoritative result: those belong to the
//! ordering layer (`CommandEnvelope`), the runtime owner (`RoutedEvent`) and
//! the authority itself. The wire sequence is the one piece of intake metadata
//! an adapter touches, and it lives in [`PlayIntent::Sequenced`] — the payload
//! families carry none, so the adapter is the only place it is detached and
//! reattached. `CommandEnvelope` is therefore never constructed here; that is
//! recorded in this module's documentation and pinned by the group suite.
//!
//! **Transport budgets are applied before any copy.** The wire record caps
//! (4 companion records, 7 remote-player records, 32 drops, 64 hostile and
//! passive records, 128 projectile records, 4096 block changes and forget
//! chunks) are checked at the top of each outbound batch arm, before a single
//! record is converted or a buffer is reserved. A domain-valid batch above a
//! cap is refused as one packet rather than truncated into a silently partial
//! publication, because a truncating adapter would publish a delta the
//! authority never sent. The inbound direction relies on each packet's own
//! gate, which every arm runs first, so a record that reached the adapter
//! already satisfies the family's cap and field rules and the checked domain
//! constructors behind it cannot fail.
//!
//! Raw wire exceptions are converted through the gates earlier nodes
//! published rather than by restating their rules: a container reference goes
//! through [`ContainerRef::to_domain_present`], a reject reason through
//! [`reject_reason_from_wire`] / [`reject_reason_to_wire`], and a chat event
//! through the bidirectional pair in [`crate::chat_event`].

use crate::block::{MAX_CHUNK_BLOCK_INDEX, SECTIONS_PER_CHUNK};
use crate::block_changes::{BlockChange, BlockChanges, MAX_BLOCK_CHANGES};
use crate::bone_meal::BoneMeal;
use crate::chat_command::ChatCommand;
use crate::chat_event::ChatEvent;
use crate::chest_state::{CHEST_SLOTS, ChestState};
use crate::chunk_snapshot::{ChunkSnapshot, SectionData};
use crate::close_container::CloseContainer;
use crate::collect_water::CollectWater;
use crate::combat_hit::CombatHit;
use crate::command_rejected::{CommandRejected, reject_reason_from_wire, reject_reason_to_wire};
use crate::companion_despawn::CompanionDespawn;
use crate::companion_spawn::CompanionSpawn;
use crate::companion_states::{CompanionState, CompanionStates, MAX_COMPANION_STATES};
use crate::container_closed::ContainerClosed;
use crate::container_ref::{CONTAINER_KIND_CHEST, CONTAINER_KIND_FURNACE, ContainerRef};
use crate::crafting_state::{
    CRAFTING_GRID_SIZE_PERSONAL, CRAFTING_GRID_SIZE_WORKBENCH, CraftingState,
};
use crate::drop_id::MAX_ITEM_DROP_BATCH;
use crate::drop_selected_item::DropSelectedItem;
use crate::equip_armor::EquipArmor;
use crate::error::ProtocolError;
use crate::forget_chunks::{ForgetChunks, MAX_FORGET_CHUNKS};
use crate::furnace_state::FurnaceState;
use crate::hostile_despawn::{HostileDespawn, MAX_HOSTILE_RECORDS};
use crate::hostile_spawn::{
    HOSTILE_KIND_BONE_THROWER, HOSTILE_KIND_NIGHTWALKER, HostileSpawn, HostileSpawnRecord,
};
use crate::hostile_state::{HostileState, HostileStateRecord};
use crate::inventory_state::InventoryState;
use crate::item_drop_removes::ItemDropRemoves;
use crate::item_drop_upserts::{ItemDrop, ItemDropUpserts};
use crate::keep_alive_reply::KeepAliveReply;
use crate::move_container_stack::MoveContainerStack;
use crate::move_crafting_stack::MoveCraftingStack;
use crate::move_inventory_stack::MoveInventoryStack;
use crate::move_stack_partial::{
    DropStack, MoveStackPartial, QuickMoveStack, STACK_VIEW_CONTAINER, STACK_VIEW_CRAFTING,
    STACK_VIEW_INVENTORY,
};
use crate::open_container::OpenContainer;
use crate::passive_despawn::{
    PASSIVE_DESPAWN_DIED, PASSIVE_DESPAWN_VANISHED, PassiveDespawn, PassiveDespawnRecord,
};
use crate::passive_spawn::PassiveSpawn;
use crate::passive_state::PassiveState;
use crate::place_block::PlaceBlock;
use crate::place_block_succeeded::PlaceBlockSucceeded;
use crate::place_water::PlaceWater;
use crate::player_input::PlayerInput;
use crate::player_state::{BlockPos, PlayerState};
use crate::projectile_despawn::{MAX_PROJECTILE_RECORDS, ProjectileDespawn};
use crate::projectile_spawn::{
    PROJECTILE_KIND_ARROW, PROJECTILE_KIND_SHARD, ProjectileSpawn, ProjectileSpawnRecord,
};
use crate::projectile_state::ProjectileState;
use crate::registry::{ClientPacket, ServerPacket};
use crate::remote_player_despawn::RemotePlayerDespawn;
use crate::remote_player_spawn::RemotePlayerSpawn;
use crate::remote_player_states::{
    MAX_REMOTE_PLAYER_STATES, RemotePlayerState, RemotePlayerStates,
};
use crate::request_chunk_resync::RequestChunkResync;
use crate::select_hotbar::SelectHotbar;
use crate::take_crafting_output::TakeCraftingOutput;
use crate::till_soil::TillSoil;
use mornlea_domain::{
    BlockChange as DomainBlockChange, BlockChanges as DomainBlockChanges, BlockChangesParts,
    BlockPos as DomainBlockPos, ChatBody, ChatEvent as DomainChatEvent, ChatEventParts, ChatIntent,
    ChestState as DomainChestState, ChestStateParts, ChunkPos,
    ChunkSnapshot as DomainChunkSnapshot, ChunkSnapshotParts, CombatHit as DomainCombatHit,
    CombatTarget, Command, CommandRejection, CommandText,
    CompanionDespawn as DomainCompanionDespawn, CompanionName,
    CompanionSpawn as DomainCompanionSpawn, CompanionSpawnParts,
    CompanionState as DomainCompanionState, CompanionStateParts,
    CompanionStates as DomainCompanionStates, CompanionStatesParts,
    ContainerClosed as DomainContainerClosed, ContainerKind, ContainerRef as DomainContainerRef,
    CraftingMove, CraftingSize, CraftingState as DomainCraftingState, CraftingStateParts,
    Dimension, DisplayName, DomainError, DropId, Event, FiniteVec3,
    ForgetChunks as DomainForgetChunks, ForgetChunksParts, FurnaceState as DomainFurnaceState,
    FurnaceStateParts, HeldActions, HostileDespawn as DomainHostileDespawn, HostileDespawnParts,
    HostileKind, HostileSpawn as DomainHostileSpawn, HostileSpawnParts,
    HostileSpawnRecord as DomainHostileSpawnRecord, HostileSpawnRecordParts,
    HostileState as DomainHostileState, HostileStateParts,
    HostileStateRecord as DomainHostileStateRecord, HostileStateRecordParts, HotbarSlot,
    InventoryMove, InventoryState as DomainInventoryState, InventoryStateParts,
    ItemDrop as DomainItemDrop, ItemDropParts, ItemDropRemoves as DomainItemDropRemoves,
    ItemDropRemovesParts, ItemDropUpserts as DomainItemDropUpserts, ItemDropUpsertsParts,
    ItemStack, LookAngles, MiningState, MiningStateParts, MotionState, MotionStateParts, Movement,
    PalettedSection, PartialMove, PassiveDespawn as DomainPassiveDespawn, PassiveDespawnParts,
    PassiveDespawnReason, PassiveSpawn as DomainPassiveSpawn, PassiveSpawnParts,
    PassiveState as DomainPassiveState, PassiveStateParts, PlacementIntent, PlacementSuccess,
    PlayerControl, PlayerControlParts, ProjectileDespawn as DomainProjectileDespawn,
    ProjectileDespawnParts, ProjectileKind, ProjectileSpawn as DomainProjectileSpawn,
    ProjectileSpawnParts, ProjectileState as DomainProjectileState, ProjectileStateParts,
    RejectReason, RemotePlayerDespawn as DomainRemotePlayerDespawn,
    RemotePlayerSpawn as DomainRemotePlayerSpawn, RemotePlayerSpawnParts,
    RemotePlayerState as DomainRemotePlayerState, RemotePlayerStateParts,
    RemotePlayerStates as DomainRemotePlayerStates, RemotePlayerStatesParts, ResyncIntent, Season,
    StackSource, StackView, SurvivalState, SurvivalStateParts, Weather, WorldState,
    WorldStateParts,
};

/// One client-to-server intent the authority can admit.
///
/// The three shapes are the complete client surface: the nineteen sequenced
/// play commands, the chat channel that carries no sequence, and the keep
/// alive reply that carries a token and no command. A sequence lives here and
/// nowhere else on the semantic side, because the domain payload families
/// deliberately carry none — the ordering layer owns that metadata, and this
/// type is the one place the wire value is detached from a packet and handed
/// on.
#[derive(Clone, Debug, PartialEq)]
pub enum PlayIntent {
    /// One sequenced play command: the wire sequence beside the checked domain
    /// payload it arrived with.
    Sequenced {
        /// The command sequence the packet carried, verbatim.
        sequence: u64,
        /// The checked domain command the packet named.
        command: Command,
    },
    /// One chat instruction, retained verbatim and carrying no sequence,
    /// because chat travels its own bounded channel.
    Chat(ChatIntent),
    /// The reply to a server keep alive, which is liveness rather than play.
    KeepAliveReply {
        /// The token the server probed with.
        token: u64,
    },
}

/// Applies one wire record cap before any record is converted or copied.
///
/// The cap is a transport budget rather than a semantic rule: the domain
/// admits a larger batch and an authority may publish it in several frames,
/// but one packet can never carry more than its own ceiling. Refusing here
/// instead of after the copy is what keeps an over-cap batch from becoming a
/// silently truncated publication. The error is
/// [`ProtocolError::InvalidRange`], the same variant each family's own gate
/// answers a count above its bound with.
fn check_record_cap(count: usize, cap: impl Into<u64>) -> Result<(), ProtocolError> {
    if count as u64 > cap.into() {
        return Err(ProtocolError::InvalidRange);
    }
    Ok(())
}

/// Maps one domain rejection onto the protocol error the same boundary
/// publishes.
///
/// The identity, enum, text and float boundaries keep their own variants so a
/// consumer can classify a refusal the same way it classifies a wire refusal;
/// every range-like domain rule collapses onto [`ProtocolError::InvalidRange`],
/// which is the variant the packet validators use for those conditions.
fn wire_error(error: DomainError) -> ProtocolError {
    match error {
        DomainError::InvalidIdentity
        | DomainError::InvalidDropSlot
        | DomainError::InvalidDropGeneration
        | DomainError::InvalidContainerGeneration => ProtocolError::InvalidIdentity,
        DomainError::InvalidItem
        | DomainError::InvalidDimension
        | DomainError::InvalidWeather
        | DomainError::InvalidSeason
        | DomainError::InvalidCombatTarget => ProtocolError::InvalidEnum,
        DomainError::InvalidText => ProtocolError::InvalidString,
        DomainError::NonFiniteValue | DomainError::NonFiniteRotation => ProtocolError::InvalidFloat,
        _ => ProtocolError::InvalidRange,
    }
}

/// Wraps one look-angle pair through the domain's finite-rotation rule.
fn look_angles(yaw: f32, pitch: f32) -> Result<LookAngles, ProtocolError> {
    LookAngles::try_new(yaw, pitch).map_err(wire_error)
}

/// Wraps one position or velocity through the domain's finiteness rule.
fn finite_vec3(components: [f32; 3]) -> Result<FiniteVec3, ProtocolError> {
    FiniteVec3::try_new(components).map_err(wire_error)
}

/// Wraps one wire stack through the domain item rule.
///
/// The mapping is the one the packet families already publish: an unregistered
/// item number is the enum boundary and every count, durability and
/// canonical-empty refusal is the range boundary.
fn checked_stack(item: u16, count: u8, durability: u16) -> Result<ItemStack, ProtocolError> {
    ItemStack::try_new(item, count, durability).map_err(wire_error)
}

/// Maps one wire container kind onto the closed domain kind.
fn container_kind(kind: u8) -> Result<ContainerKind, ProtocolError> {
    match kind {
        CONTAINER_KIND_FURNACE => Ok(ContainerKind::Furnace),
        CONTAINER_KIND_CHEST => Ok(ContainerKind::Chest),
        _ => Err(ProtocolError::InvalidEnum),
    }
}

/// Maps one wire view number, with its container reference, onto the domain
/// view.
///
/// The container view needs a checked real reference, so it goes through the
/// 1.3 neutral gate and a foreign dimension, an unknown kind, a zero
/// generation or an out-of-range slot is refused here rather than narrowed
/// into a value the authority would reject. The two absent views carry no
/// container at all; that they must also carry the exact zero reference is a
/// rule the packet's own gate already applied before this function runs.
fn stack_view(container: &ContainerRef, view: u8) -> Result<StackView, ProtocolError> {
    match view {
        STACK_VIEW_INVENTORY => Ok(StackView::Inventory),
        STACK_VIEW_CRAFTING => Ok(StackView::Crafting),
        STACK_VIEW_CONTAINER => Ok(StackView::Container(container.to_domain_present()?)),
        _ => Err(ProtocolError::InvalidEnum),
    }
}

/// Maps one domain view back onto its wire view number and reference.
///
/// The absent views publish the exact all-zero record, which is the one wire
/// form that names "no container"; a real reference publishes the overworld
/// dimension, because the domain value is inherently an overworld one and
/// carries no dimension that could disagree.
fn wire_stack_view(view: StackView) -> (u8, ContainerRef) {
    match view {
        StackView::Inventory => (STACK_VIEW_INVENTORY, ContainerRef::NONE),
        StackView::Crafting => (STACK_VIEW_CRAFTING, ContainerRef::NONE),
        StackView::Container(reference) => (STACK_VIEW_CONTAINER, wire_container(&reference)),
    }
}

/// Publishes one checked domain container reference as its 18-byte wire form.
fn wire_container(reference: &DomainContainerRef) -> ContainerRef {
    ContainerRef {
        dimension: 0,
        chunk_x: reference.chunk().x(),
        chunk_z: reference.chunk().z(),
        kind: match reference.kind() {
            ContainerKind::Furnace => CONTAINER_KIND_FURNACE,
            ContainerKind::Chest => CONTAINER_KIND_CHEST,
        },
        slot: reference.slot(),
        generation: reference.generation(),
    }
}

/// Maps one wire hostile kind onto the closed domain kind.
fn hostile_kind(kind: u8) -> Result<HostileKind, ProtocolError> {
    match kind {
        HOSTILE_KIND_NIGHTWALKER => Ok(HostileKind::Nightwalker),
        HOSTILE_KIND_BONE_THROWER => Ok(HostileKind::BoneThrower),
        _ => Err(ProtocolError::InvalidEnum),
    }
}

/// Returns the wire value one closed domain hostile kind publishes.
fn wire_hostile_kind(kind: HostileKind) -> u8 {
    match kind {
        HostileKind::Nightwalker => HOSTILE_KIND_NIGHTWALKER,
        HostileKind::BoneThrower => HOSTILE_KIND_BONE_THROWER,
    }
}

/// Maps one wire projectile kind onto the closed domain kind.
fn projectile_kind(kind: u8) -> Result<ProjectileKind, ProtocolError> {
    match kind {
        PROJECTILE_KIND_SHARD => Ok(ProjectileKind::Shard),
        PROJECTILE_KIND_ARROW => Ok(ProjectileKind::Arrow),
        _ => Err(ProtocolError::InvalidEnum),
    }
}

/// Returns the wire value one closed domain projectile kind publishes.
fn wire_projectile_kind(kind: ProjectileKind) -> u8 {
    match kind {
        ProjectileKind::Shard => PROJECTILE_KIND_SHARD,
        ProjectileKind::Arrow => PROJECTILE_KIND_ARROW,
    }
}

/// Maps one wire passive despawn reason onto the closed domain reason.
fn passive_despawn_reason(reason: u8) -> Result<PassiveDespawnReason, ProtocolError> {
    match reason {
        PASSIVE_DESPAWN_VANISHED => Ok(PassiveDespawnReason::Vanished),
        PASSIVE_DESPAWN_DIED => Ok(PassiveDespawnReason::Died),
        _ => Err(ProtocolError::InvalidEnum),
    }
}

/// Returns the wire value one closed domain despawn reason publishes.
fn wire_passive_despawn_reason(reason: PassiveDespawnReason) -> u8 {
    match reason {
        PassiveDespawnReason::Vanished => PASSIVE_DESPAWN_VANISHED,
        PassiveDespawnReason::Died => PASSIVE_DESPAWN_DIED,
    }
}

/// Maps one wire crafting grid side onto the closed domain size.
fn crafting_size(size: u8) -> Result<CraftingSize, ProtocolError> {
    match size {
        CRAFTING_GRID_SIZE_PERSONAL => Ok(CraftingSize::Personal),
        CRAFTING_GRID_SIZE_WORKBENCH => Ok(CraftingSize::Workbench),
        _ => Err(ProtocolError::InvalidEnum),
    }
}

/// Returns the wire value one closed domain crafting size publishes.
fn wire_crafting_size(size: CraftingSize) -> u8 {
    match size {
        CraftingSize::Personal => CRAFTING_GRID_SIZE_PERSONAL,
        CraftingSize::Workbench => CRAFTING_GRID_SIZE_WORKBENCH,
    }
}

/// Maps one wire weather kind onto the closed domain weather.
fn weather(kind: u8) -> Result<Weather, ProtocolError> {
    Weather::try_new(kind).map_err(wire_error)
}

/// Maps one wire season onto the closed domain season.
fn season(value: u8) -> Result<Season, ProtocolError> {
    Season::try_new(value).map_err(wire_error)
}

/// Maps one wire combat target kind onto the closed domain target.
fn combat_target(kind: u8) -> Result<CombatTarget, ProtocolError> {
    CombatTarget::try_new(kind).map_err(wire_error)
}

/// Converts the wire mining block into the checked domain union.
fn mining_state(
    active: bool,
    target: BlockPos,
    progress: u16,
    required: u16,
    harvestable: bool,
) -> Result<MiningState, ProtocolError> {
    MiningState::try_new(MiningStateParts {
        active,
        target: DomainBlockPos::new(target.x, target.y, target.z),
        progress,
        required,
        harvestable,
    })
    .map_err(wire_error)
}

/// Publishes one checked domain mining union back into its wire block.
fn wire_mining_state(state: MiningState) -> (bool, BlockPos, u16, u16, bool) {
    match state {
        MiningState::Idle => (false, BlockPos { x: 0, y: 0, z: 0 }, 0, 0, false),
        MiningState::Active(active) => {
            let target = active.target();
            (
                true,
                BlockPos {
                    x: target.x(),
                    y: target.y(),
                    z: target.z(),
                },
                active.progress(),
                active.required(),
                active.harvestable(),
            )
        }
    }
}

/// Moves one wire chat event into the checked domain chat event.
///
/// The conversion is the 3.11 pair, which owns the branch shapes and the raw
/// absent companion identity. This adapter only routes the packet variant to
/// it, so the chat rules stay in one place.
fn chat_event(event: ChatEvent) -> Result<DomainChatEvent, ProtocolError> {
    DomainChatEvent::try_from(event)
}

impl TryFrom<ClientPacket> for PlayIntent {
    type Error = ProtocolError;

    /// Converts one decoded client packet into the intent it names.
    ///
    /// Every arm runs the record's own gate first, so the packet's rules —
    /// including the reference regime of the view-addressed commands and the
    /// nonzero-sequence rule of the take-crafting-output command — are applied
    /// before any domain value is assembled, and the checked constructors
    /// behind this match cannot fail on a record that passed it.
    ///
    /// The two negotiation records are refused with
    /// [`ProtocolError::UnknownPacket`]: a hello and a login start name no play
    /// intent, and this is the same variant the registry answers an
    /// unregistered key with, so one condition has one answer.
    fn try_from(packet: ClientPacket) -> Result<Self, Self::Error> {
        match packet {
            ClientPacket::ClientHello(_) | ClientPacket::LoginStart(_) => {
                Err(ProtocolError::UnknownPacket)
            }
            ClientPacket::PlayerInput(input) => {
                input.validate()?;
                Ok(PlayIntent::Sequenced {
                    sequence: input.sequence,
                    command: Command::PlayerInput(PlayerControl::new(PlayerControlParts {
                        movement: Movement {
                            move_x: input.move_x,
                            move_z: input.move_z,
                            jump: input.jump,
                        },
                        look: look_angles(input.yaw, input.pitch)?,
                        actions: HeldActions {
                            primary: input.mining,
                            eating: input.eating,
                            sprinting: input.sprinting,
                            sneaking: input.sneaking,
                        },
                    })),
                })
            }
            ClientPacket::PlaceBlock(command) => {
                command.validate()?;
                Ok(PlayIntent::Sequenced {
                    sequence: command.sequence,
                    command: Command::PlaceBlock(
                        PlacementIntent::try_new(
                            look_angles(command.yaw, command.pitch)?,
                            command.slot,
                        )
                        .map_err(wire_error)?,
                    ),
                })
            }
            ClientPacket::RequestChunkResync(request) => {
                request.validate()?;
                Ok(PlayIntent::Sequenced {
                    sequence: request.sequence,
                    command: Command::Resync(
                        ResyncIntent::try_new(
                            request.dimension.get(),
                            ChunkPos::new(request.chunk_x, request.chunk_z),
                            request.have_revision,
                        )
                        .map_err(wire_error)?,
                    ),
                })
            }
            ClientPacket::KeepAliveReply(reply) => {
                reply.validate()?;
                Ok(PlayIntent::KeepAliveReply { token: reply.token })
            }
            ClientPacket::SelectHotbar(selection) => {
                selection.validate()?;
                Ok(PlayIntent::Sequenced {
                    sequence: selection.sequence,
                    command: Command::SelectHotbar(
                        HotbarSlot::new(selection.slot).map_err(wire_error)?,
                    ),
                })
            }
            ClientPacket::MoveInventoryStack(command) => {
                command.validate()?;
                Ok(PlayIntent::Sequenced {
                    sequence: command.sequence,
                    command: Command::MoveInventory(
                        InventoryMove::try_new(command.from, command.to).map_err(wire_error)?,
                    ),
                })
            }
            ClientPacket::MoveCraftingStack(command) => {
                command.validate()?;
                Ok(PlayIntent::Sequenced {
                    sequence: command.sequence,
                    command: Command::MoveCrafting(
                        CraftingMove::try_new(command.from, command.to).map_err(wire_error)?,
                    ),
                })
            }
            ClientPacket::OpenContainer(command) => {
                command.validate()?;
                Ok(PlayIntent::Sequenced {
                    sequence: command.sequence,
                    command: Command::OpenContainer(look_angles(command.yaw, command.pitch)?),
                })
            }
            ClientPacket::MoveContainerStack(command) => {
                command.validate()?;
                Ok(PlayIntent::Sequenced {
                    sequence: command.sequence,
                    command: Command::MoveContainer(
                        mornlea_domain::ContainerMove::try_new(
                            ChunkPos::new(command.container.chunk_x, command.container.chunk_z),
                            container_kind(command.container.kind)?,
                            command.container.slot,
                            command.container.generation,
                            command.from,
                            command.to,
                        )
                        .map_err(wire_error)?,
                    ),
                })
            }
            ClientPacket::CloseContainer(command) => {
                command.validate()?;
                Ok(PlayIntent::Sequenced {
                    sequence: command.sequence,
                    command: Command::CloseContainer,
                })
            }
            ClientPacket::DropSelectedItem(command) => {
                command.validate()?;
                Ok(PlayIntent::Sequenced {
                    sequence: command.sequence,
                    command: Command::DropSelectedItem,
                })
            }
            ClientPacket::ChatCommand(command) => {
                command.validate()?;
                Ok(PlayIntent::Chat(
                    ChatIntent::try_new(
                        CommandText::try_from_canonical(command.text)
                            .map_err(|_| ProtocolError::InvalidString)?,
                    )
                    .map_err(wire_error)?,
                ))
            }
            ClientPacket::TillSoil(command) => {
                command.validate()?;
                Ok(PlayIntent::Sequenced {
                    sequence: command.sequence,
                    command: Command::TillSoil(look_angles(command.yaw, command.pitch)?),
                })
            }
            ClientPacket::BoneMeal(command) => {
                command.validate()?;
                Ok(PlayIntent::Sequenced {
                    sequence: command.sequence,
                    command: Command::BoneMeal(look_angles(command.yaw, command.pitch)?),
                })
            }
            ClientPacket::TakeCraftingOutput(command) => {
                command.validate()?;
                Ok(PlayIntent::Sequenced {
                    sequence: command.sequence,
                    command: Command::TakeCraftingOutput,
                })
            }
            ClientPacket::CollectWater(command) => {
                command.validate()?;
                Ok(PlayIntent::Sequenced {
                    sequence: command.sequence,
                    command: Command::CollectWater(look_angles(command.yaw, command.pitch)?),
                })
            }
            ClientPacket::PlaceWater(command) => {
                command.validate()?;
                Ok(PlayIntent::Sequenced {
                    sequence: command.sequence,
                    command: Command::PlaceWater(look_angles(command.yaw, command.pitch)?),
                })
            }
            ClientPacket::EquipArmor(command) => {
                command.validate()?;
                Ok(PlayIntent::Sequenced {
                    sequence: command.sequence,
                    command: Command::EquipArmor,
                })
            }
            ClientPacket::MoveStackPartial(command) => {
                command.validate()?;
                Ok(PlayIntent::Sequenced {
                    sequence: command.sequence,
                    command: Command::MovePartial(
                        PartialMove::try_new(
                            stack_view(&command.container, command.view)?,
                            command.from,
                            command.to,
                            command.single,
                        )
                        .map_err(wire_error)?,
                    ),
                })
            }
            ClientPacket::QuickMoveStack(command) => {
                command.validate()?;
                Ok(PlayIntent::Sequenced {
                    sequence: command.sequence,
                    command: Command::QuickMove(
                        StackSource::try_new(
                            stack_view(&command.container, command.view)?,
                            command.from,
                        )
                        .map_err(wire_error)?,
                    ),
                })
            }
            ClientPacket::DropStack(command) => {
                command.validate()?;
                Ok(PlayIntent::Sequenced {
                    sequence: command.sequence,
                    command: Command::DropStack(
                        StackSource::try_new(
                            stack_view(&command.container, command.view)?,
                            command.slot,
                        )
                        .map_err(wire_error)?,
                    ),
                })
            }
        }
    }
}

impl TryFrom<PlayIntent> for ClientPacket {
    type Error = ProtocolError;

    /// Converts one intent back into the packet that publishes it.
    ///
    /// The sequence is reattached only where the wire carries one: the chat
    /// channel and the keep alive reply publish no sequence, and the nineteen
    /// sequenced commands publish the value the intent holds. Each concrete
    /// constructor is the family's own gate, so an intent that names a value
    /// the wire refuses — a zero `TakeCraftingOutput` sequence, an
    /// out-of-range slot, a malformed reference — is refused here rather than
    /// published. The adapter adds no sequence policy of its own; the nonzero
    /// rule for `TakeCraftingOutput` belongs to the packet and, independently,
    /// to the domain envelope.
    fn try_from(intent: PlayIntent) -> Result<Self, Self::Error> {
        match intent {
            PlayIntent::Sequenced { sequence, command } => match command {
                Command::PlayerInput(control) => Ok(ClientPacket::PlayerInput(PlayerInput::new(
                    sequence,
                    control.movement().move_x,
                    control.movement().move_z,
                    control.movement().jump,
                    control.look().yaw(),
                    control.look().pitch(),
                    control.actions().primary,
                    control.actions().eating,
                    control.actions().sprinting,
                    control.actions().sneaking,
                )?)),
                Command::PlaceBlock(intent) => Ok(ClientPacket::PlaceBlock(PlaceBlock::new(
                    sequence,
                    intent.look().yaw(),
                    intent.look().pitch(),
                    intent.slot().get(),
                )?)),
                Command::Resync(intent) => {
                    Ok(ClientPacket::RequestChunkResync(RequestChunkResync::new(
                        sequence,
                        intent.dimension(),
                        intent.chunk().x(),
                        intent.chunk().z(),
                        intent.have_revision(),
                    )))
                }
                Command::SelectHotbar(slot) => Ok(ClientPacket::SelectHotbar(SelectHotbar::new(
                    sequence,
                    slot.get(),
                )?)),
                Command::OpenContainer(look) => Ok(ClientPacket::OpenContainer(
                    OpenContainer::new(sequence, look.yaw(), look.pitch())?,
                )),
                Command::TillSoil(look) => Ok(ClientPacket::TillSoil(TillSoil::new(
                    sequence,
                    look.yaw(),
                    look.pitch(),
                )?)),
                Command::BoneMeal(look) => Ok(ClientPacket::BoneMeal(BoneMeal::new(
                    sequence,
                    look.yaw(),
                    look.pitch(),
                )?)),
                Command::CollectWater(look) => Ok(ClientPacket::CollectWater(CollectWater::new(
                    sequence,
                    look.yaw(),
                    look.pitch(),
                )?)),
                Command::PlaceWater(look) => Ok(ClientPacket::PlaceWater(PlaceWater::new(
                    sequence,
                    look.yaw(),
                    look.pitch(),
                )?)),
                Command::MoveInventory(move_command) => Ok(ClientPacket::MoveInventoryStack(
                    MoveInventoryStack::new(sequence, move_command.from(), move_command.to())?,
                )),
                Command::MoveCrafting(move_command) => Ok(ClientPacket::MoveCraftingStack(
                    MoveCraftingStack::new(sequence, move_command.from(), move_command.to())?,
                )),
                Command::MoveContainer(move_command) => {
                    let container = move_command.container();
                    Ok(ClientPacket::MoveContainerStack(MoveContainerStack::new(
                        sequence,
                        wire_container(&container),
                        move_command.from(),
                        move_command.to(),
                    )?))
                }
                Command::CloseContainer => {
                    Ok(ClientPacket::CloseContainer(CloseContainer::new(sequence)))
                }
                Command::DropSelectedItem => Ok(ClientPacket::DropSelectedItem(
                    DropSelectedItem::new(sequence),
                )),
                Command::TakeCraftingOutput => Ok(ClientPacket::TakeCraftingOutput(
                    TakeCraftingOutput::new(sequence)?,
                )),
                Command::EquipArmor => Ok(ClientPacket::EquipArmor(EquipArmor::new(sequence))),
                Command::MovePartial(partial) => {
                    let (view, container) = wire_stack_view(partial.view());
                    Ok(ClientPacket::MoveStackPartial(MoveStackPartial::new(
                        sequence,
                        container,
                        view,
                        partial.from(),
                        partial.to(),
                        partial.single(),
                    )?))
                }
                Command::QuickMove(source) => {
                    let (view, container) = wire_stack_view(source.view());
                    Ok(ClientPacket::QuickMoveStack(QuickMoveStack::new(
                        sequence,
                        container,
                        view,
                        source.slot(),
                    )?))
                }
                Command::DropStack(source) => {
                    let (view, container) = wire_stack_view(source.view());
                    Ok(ClientPacket::DropStack(DropStack::new(
                        sequence,
                        container,
                        view,
                        source.slot(),
                    )?))
                }
            },
            PlayIntent::Chat(intent) => Ok(ClientPacket::ChatCommand(ChatCommand::new(
                intent.text().as_str().to_owned(),
            )?)),
            PlayIntent::KeepAliveReply { token } => {
                Ok(ClientPacket::KeepAliveReply(KeepAliveReply::new(token)?))
            }
        }
    }
}

impl TryFrom<ServerPacket> for Event {
    type Error = ProtocolError;

    /// Converts one decoded server packet into the publication it names.
    ///
    /// Every arm runs the record's own gate first, which is what applies the
    /// wire record caps and the family field rules on the way in; the checked
    /// domain constructors behind this match then restate no rule and cannot
    /// fail on a record that passed. The six control families are refused with
    /// [`ProtocolError::UnknownPacket`], because a hello, a rejection, a login
    /// answer, a keep alive and a disconnect are transport facts the domain
    /// publication set deliberately excludes — there is no `Event` variant
    /// that could carry one without turning a transport fact into a semantic
    /// observation.
    fn try_from(packet: ServerPacket) -> Result<Self, Self::Error> {
        match packet {
            ServerPacket::ServerHello(_)
            | ServerPacket::HandshakeReject(_)
            | ServerPacket::LoginSuccess(_)
            | ServerPacket::LoginReject(_)
            | ServerPacket::KeepAlive(_)
            | ServerPacket::Disconnect(_) => Err(ProtocolError::UnknownPacket),
            ServerPacket::ChunkSnapshot(snapshot) => {
                snapshot.validate()?;
                // The gate already pinned the list to the full column in
                // order, so the conversion moves each section through the
                // compact 1.3 conversion and places it by position rather than
                // expanding 98,304 cells per section.
                let mut sections: Vec<PalettedSection> = Vec::with_capacity(SECTIONS_PER_CHUNK);
                for section in snapshot.sections {
                    sections.push(PalettedSection::try_from(section)?);
                }
                let sections: Box<[PalettedSection; SECTIONS_PER_CHUNK]> = sections
                    .into_boxed_slice()
                    .try_into()
                    .map_err(|_| ProtocolError::InvalidRange)?;
                Ok(Event::ChunkSnapshot(
                    DomainChunkSnapshot::try_new(ChunkSnapshotParts {
                        dimension: snapshot.dimension,
                        chunk: ChunkPos::new(snapshot.chunk_x, snapshot.chunk_z),
                        revision: snapshot.revision,
                        sections,
                    })
                    .map_err(wire_error)?,
                ))
            }
            ServerPacket::BlockChanges(batch) => {
                batch.validate()?;
                let mut changes: Vec<DomainBlockChange> = Vec::with_capacity(batch.changes.len());
                for change in batch.changes {
                    changes.push(
                        DomainBlockChange::try_new(
                            DomainBlockPos::new(change.x, change.y, change.z),
                            change.block,
                        )
                        .map_err(wire_error)?,
                    );
                }
                Ok(Event::BlockChanges(
                    DomainBlockChanges::try_new(BlockChangesParts {
                        dimension: batch.dimension,
                        chunk: ChunkPos::new(batch.chunk_x, batch.chunk_z),
                        base_revision: batch.base_revision,
                        new_revision: batch.new_revision,
                        changes: changes.into_boxed_slice(),
                    })
                    .map_err(wire_error)?,
                ))
            }
            ServerPacket::ForgetChunks(batch) => {
                batch.validate()?;
                let mut chunks: Vec<ChunkPos> = Vec::with_capacity(batch.chunks.len());
                for (chunk_x, chunk_z) in batch.chunks {
                    chunks.push(ChunkPos::new(chunk_x, chunk_z));
                }
                Ok(Event::ForgetChunks(
                    DomainForgetChunks::try_new(ForgetChunksParts {
                        dimension: batch.dimension,
                        chunks: chunks.into_boxed_slice(),
                    })
                    .map_err(wire_error)?,
                ))
            }
            ServerPacket::PlayerState(state) => {
                state.validate()?;
                Ok(Event::PlayerState(mornlea_domain::PlayerState::new(
                    mornlea_domain::PlayerStateParts {
                        server_tick: state.server_tick,
                        last_input_sequence: state.last_input_sequence,
                        dimension: state.dimension,
                        motion: MotionState::new(MotionStateParts {
                            position: finite_vec3(state.position)?,
                            velocity: finite_vec3(state.velocity)?,
                            on_ground: state.on_ground,
                        }),
                        look: look_angles(state.yaw, state.pitch)?,
                        ready: state.ready,
                        reset: state.reset,
                        mining: mining_state(
                            state.mining_active,
                            state.mining_target,
                            state.mining_progress_ticks,
                            state.mining_required_ticks,
                            state.mining_harvestable,
                        )?,
                        survival: SurvivalState::try_new(SurvivalStateParts {
                            health: state.health,
                            oxygen: state.oxygen,
                            hunger: state.hunger,
                            saturation_zero: state.saturation_zero,
                            armor_points: state.armor_points,
                        })
                        .map_err(wire_error)?,
                        world: WorldState::try_new(WorldStateParts {
                            day_phase_offset: state.day_phase_offset,
                            world_time_ticks: state.world_time_ticks,
                            weather: weather(state.weather_kind)?,
                            season: season(state.season)?,
                            season_progress: state.season_progress,
                            temperature: state.temperature,
                        })
                        .map_err(wire_error)?,
                    },
                )))
            }
            ServerPacket::CommandRejected(rejection) => {
                rejection.validate()?;
                Ok(Event::CommandRejected(CommandRejection::new(
                    rejection.sequence,
                    reject_reason_from_wire(rejection.reason)?,
                )))
            }
            ServerPacket::RemotePlayerSpawn(spawn) => {
                spawn.validate()?;
                Ok(Event::RemotePlayerSpawn(DomainRemotePlayerSpawn::new(
                    RemotePlayerSpawnParts {
                        player_id: spawn.player_id,
                        display_name: DisplayName::try_from_canonical(spawn.display_name)
                            .map_err(|_| ProtocolError::InvalidString)?,
                        server_tick: spawn.server_tick,
                        dimension: spawn.dimension,
                        position: finite_vec3(spawn.position)?,
                        look: look_angles(spawn.yaw, spawn.pitch)?,
                    },
                )))
            }
            ServerPacket::RemotePlayerDespawn(despawn) => {
                despawn.validate()?;
                Ok(Event::RemotePlayerDespawn(DomainRemotePlayerDespawn::new(
                    despawn.player,
                )))
            }
            ServerPacket::RemotePlayerStates(batch) => {
                batch.validate()?;
                let mut states: Vec<mornlea_domain::RemotePlayerState> =
                    Vec::with_capacity(batch.players.len());
                for player in batch.players {
                    states.push(mornlea_domain::RemotePlayerState::new(
                        RemotePlayerStateParts {
                            player_id: player.player_id,
                            dimension: player.dimension,
                            position: finite_vec3(player.position)?,
                            look: look_angles(player.yaw, player.pitch)?,
                            reset: player.reset,
                        },
                    ));
                }
                Ok(Event::RemotePlayerStates(
                    DomainRemotePlayerStates::try_new(RemotePlayerStatesParts {
                        server_tick: batch.server_tick,
                        states: states.into_boxed_slice(),
                    })
                    .map_err(wire_error)?,
                ))
            }
            ServerPacket::InventoryState(state) => {
                state.validate()?;
                Ok(Event::InventoryState(DomainInventoryState::new(
                    InventoryStateParts {
                        selected: HotbarSlot::new(state.selected).map_err(wire_error)?,
                        hotbar: state.hotbar,
                        backpack: state.backpack,
                    },
                )))
            }
            ServerPacket::ItemDropUpserts(batch) => {
                batch.validate()?;
                let mut drops: Vec<DomainItemDrop> = Vec::with_capacity(batch.drops.len());
                for drop in batch.drops {
                    drops.push(
                        DomainItemDrop::try_new(ItemDropParts {
                            id: drop.id,
                            block_index: drop.block_index,
                            stack: checked_stack(drop.item, drop.count, drop.durability)?,
                        })
                        .map_err(wire_error)?,
                    );
                }
                Ok(Event::ItemDropUpserts(
                    DomainItemDropUpserts::try_new(ItemDropUpsertsParts {
                        server_tick: batch.server_tick,
                        drops: drops.into_boxed_slice(),
                    })
                    .map_err(wire_error)?,
                ))
            }
            ServerPacket::ItemDropRemoves(batch) => {
                batch.validate()?;
                Ok(Event::ItemDropRemoves(
                    DomainItemDropRemoves::try_new(ItemDropRemovesParts {
                        server_tick: batch.server_tick,
                        ids: batch.ids.into_boxed_slice(),
                    })
                    .map_err(wire_error)?,
                ))
            }
            ServerPacket::FurnaceState(state) => {
                state.validate()?;
                Ok(Event::FurnaceState(
                    DomainFurnaceState::try_new(FurnaceStateParts {
                        container: state.furnace.to_domain_present()?,
                        input: state.input,
                        fuel: state.fuel,
                        output: state.output,
                        progress_ticks: state.progress_ticks,
                        burn_ticks: state.burn_ticks,
                    })
                    .map_err(wire_error)?,
                ))
            }
            ServerPacket::ContainerClosed(closed) => {
                closed.validate()?;
                Ok(Event::ContainerClosed(DomainContainerClosed::new(
                    closed.container.to_domain_present()?,
                )))
            }
            ServerPacket::ChestState(state) => {
                state.validate()?;
                Ok(Event::ChestState(
                    DomainChestState::try_new(ChestStateParts {
                        container: state.chest.to_domain_present()?,
                        items: state.items,
                    })
                    .map_err(wire_error)?,
                ))
            }
            ServerPacket::ChatEvent(event) => Ok(Event::Chat(chat_event(event)?)),
            ServerPacket::CompanionSpawn(spawn) => {
                spawn.validate()?;
                Ok(Event::CompanionSpawn(
                    DomainCompanionSpawn::try_new(CompanionSpawnParts {
                        id: spawn.companion_id,
                        name: CompanionName::try_from_canonical(spawn.name)
                            .map_err(|_| ProtocolError::InvalidString)?,
                        server_tick: spawn.tick,
                        dimension: spawn.dimension,
                        position: finite_vec3(spawn.position)?,
                        look: look_angles(spawn.yaw, spawn.pitch)?,
                    })
                    .map_err(wire_error)?,
                ))
            }
            ServerPacket::CompanionStates(batch) => {
                batch.validate()?;
                let mut states: Vec<DomainCompanionState> = Vec::with_capacity(batch.states.len());
                for record in batch.states {
                    states.push(
                        DomainCompanionState::try_new(CompanionStateParts {
                            id: record.companion_id,
                            dimension: record.dimension,
                            position: finite_vec3(record.position)?,
                            look: look_angles(record.yaw, record.pitch)?,
                            reset: record.reset,
                        })
                        .map_err(wire_error)?,
                    );
                }
                Ok(Event::CompanionStates(
                    DomainCompanionStates::try_new(CompanionStatesParts {
                        server_tick: batch.tick,
                        states: states.into_boxed_slice(),
                    })
                    .map_err(wire_error)?,
                ))
            }
            ServerPacket::CompanionDespawn(despawn) => {
                despawn.validate()?;
                Ok(Event::CompanionDespawn(DomainCompanionDespawn::new(
                    despawn.companion,
                )))
            }
            ServerPacket::PlaceBlockSucceeded(success) => {
                success.validate()?;
                Ok(Event::PlaceBlockSucceeded(PlacementSuccess::new(
                    success.sequence,
                )))
            }
            ServerPacket::CraftingState(state) => {
                state.validate()?;
                Ok(Event::CraftingState(
                    DomainCraftingState::try_new(CraftingStateParts {
                        size: crafting_size(state.size)?,
                        slots: state.slots,
                        output: state.output,
                    })
                    .map_err(wire_error)?,
                ))
            }
            ServerPacket::HostileSpawn(batch) => {
                batch.validate()?;
                let mut spawns: Vec<DomainHostileSpawnRecord> =
                    Vec::with_capacity(batch.spawns.len());
                for record in batch.spawns {
                    spawns.push(
                        DomainHostileSpawnRecord::try_new(HostileSpawnRecordParts {
                            id: record.id,
                            dimension: record.dimension,
                            position: finite_vec3(record.position)?,
                            yaw: record.yaw,
                            health: record.health,
                            kind: hostile_kind(record.kind)?,
                        })
                        .map_err(wire_error)?,
                    );
                }
                Ok(Event::HostileSpawn(
                    DomainHostileSpawn::try_new(HostileSpawnParts {
                        server_tick: batch.server_tick,
                        spawns: spawns.into_boxed_slice(),
                    })
                    .map_err(wire_error)?,
                ))
            }
            ServerPacket::HostileState(batch) => {
                batch.validate()?;
                let mut states: Vec<DomainHostileStateRecord> =
                    Vec::with_capacity(batch.states.len());
                for record in batch.states {
                    states.push(
                        DomainHostileStateRecord::try_new(HostileStateRecordParts {
                            id: record.id,
                            position: finite_vec3(record.position)?,
                            velocity: finite_vec3(record.velocity)?,
                            yaw: record.yaw,
                            health: record.health,
                            kind: hostile_kind(record.kind)?,
                        })
                        .map_err(wire_error)?,
                    );
                }
                Ok(Event::HostileState(
                    DomainHostileState::try_new(HostileStateParts {
                        server_tick: batch.server_tick,
                        states: states.into_boxed_slice(),
                    })
                    .map_err(wire_error)?,
                ))
            }
            ServerPacket::HostileDespawn(batch) => {
                batch.validate()?;
                Ok(Event::HostileDespawn(
                    DomainHostileDespawn::try_new(HostileDespawnParts {
                        server_tick: batch.server_tick,
                        ids: batch.ids.into_boxed_slice(),
                    })
                    .map_err(wire_error)?,
                ))
            }
            ServerPacket::CombatHit(hit) => {
                hit.validate()?;
                Ok(Event::CombatHit(
                    DomainCombatHit::try_new(
                        hit.server_tick,
                        hit.damage,
                        combat_target(hit.target_kind)?,
                    )
                    .map_err(wire_error)?,
                ))
            }
            ServerPacket::PassiveSpawn(batch) => {
                batch.validate()?;
                let mut spawns: Vec<mornlea_domain::PassiveSpawnRecord> =
                    Vec::with_capacity(batch.spawns.len());
                for record in batch.spawns {
                    spawns.push(
                        mornlea_domain::PassiveSpawnRecord::try_new(
                            mornlea_domain::PassiveSpawnRecordParts {
                                id: record.id,
                                dimension: record.dimension,
                                position: finite_vec3(record.position)?,
                                yaw: record.yaw,
                                health: record.health,
                            },
                        )
                        .map_err(wire_error)?,
                    );
                }
                Ok(Event::PassiveSpawn(
                    DomainPassiveSpawn::try_new(PassiveSpawnParts {
                        server_tick: batch.server_tick,
                        spawns: spawns.into_boxed_slice(),
                    })
                    .map_err(wire_error)?,
                ))
            }
            ServerPacket::PassiveState(batch) => {
                batch.validate()?;
                let mut states: Vec<mornlea_domain::PassiveStateRecord> =
                    Vec::with_capacity(batch.states.len());
                for record in batch.states {
                    states.push(
                        mornlea_domain::PassiveStateRecord::try_new(
                            mornlea_domain::PassiveStateRecordParts {
                                id: record.id,
                                position: finite_vec3(record.position)?,
                                velocity: finite_vec3(record.velocity)?,
                                yaw: record.yaw,
                                health: record.health,
                                grazing: record.grazing != 0,
                            },
                        )
                        .map_err(wire_error)?,
                    );
                }
                Ok(Event::PassiveState(
                    DomainPassiveState::try_new(PassiveStateParts {
                        server_tick: batch.server_tick,
                        states: states.into_boxed_slice(),
                    })
                    .map_err(wire_error)?,
                ))
            }
            ServerPacket::PassiveDespawn(batch) => {
                batch.validate()?;
                let mut despawns: Vec<mornlea_domain::PassiveDespawnRecord> =
                    Vec::with_capacity(batch.despawns.len());
                for record in batch.despawns {
                    despawns.push(mornlea_domain::PassiveDespawnRecord::new(
                        record.id,
                        passive_despawn_reason(record.reason)?,
                    ));
                }
                Ok(Event::PassiveDespawn(
                    DomainPassiveDespawn::try_new(PassiveDespawnParts {
                        server_tick: batch.server_tick,
                        despawns: despawns.into_boxed_slice(),
                    })
                    .map_err(wire_error)?,
                ))
            }
            ServerPacket::ProjectileSpawn(batch) => {
                batch.validate()?;
                let mut spawns: Vec<mornlea_domain::ProjectileSpawnRecord> =
                    Vec::with_capacity(batch.spawns.len());
                for record in batch.spawns {
                    spawns.push(mornlea_domain::ProjectileSpawnRecord::new(
                        mornlea_domain::ProjectileSpawnRecordParts {
                            id: record.id,
                            kind: projectile_kind(record.kind)?,
                            dimension: record.dimension,
                            position: finite_vec3(record.position)?,
                            velocity: finite_vec3(record.velocity)?,
                        },
                    ));
                }
                Ok(Event::ProjectileSpawn(
                    DomainProjectileSpawn::try_new(ProjectileSpawnParts {
                        server_tick: batch.server_tick,
                        spawns: spawns.into_boxed_slice(),
                    })
                    .map_err(wire_error)?,
                ))
            }
            ServerPacket::ProjectileState(batch) => {
                batch.validate()?;
                let mut states: Vec<mornlea_domain::ProjectileStateRecord> =
                    Vec::with_capacity(batch.states.len());
                for record in batch.states {
                    states.push(mornlea_domain::ProjectileStateRecord::new(
                        mornlea_domain::ProjectileStateRecordParts {
                            id: record.id,
                            position: finite_vec3(record.position)?,
                        },
                    ));
                }
                Ok(Event::ProjectileState(
                    DomainProjectileState::try_new(ProjectileStateParts {
                        server_tick: batch.server_tick,
                        states: states.into_boxed_slice(),
                    })
                    .map_err(wire_error)?,
                ))
            }
            ServerPacket::ProjectileDespawn(batch) => {
                batch.validate()?;
                Ok(Event::ProjectileDespawn(
                    DomainProjectileDespawn::try_new(ProjectileDespawnParts {
                        server_tick: batch.server_tick,
                        ids: batch.ids.into_boxed_slice(),
                    })
                    .map_err(wire_error)?,
                ))
            }
        }
    }
}

impl TryFrom<Event> for ServerPacket {
    type Error = ProtocolError;

    /// Converts one checked publication back into the packet that carries it.
    ///
    /// Every batch arm applies its wire record cap BEFORE any record is
    /// converted or a buffer reserved, so a domain-valid batch above a cap is
    /// refused as one packet rather than truncated: the authority published
    /// every record and a truncating adapter would publish a delta it never
    /// sent. The submitted record order is preserved exactly, because the
    /// authority publishes the canonical order and a replay has to observe the
    /// sequence the record carried. Each concrete constructor is the family's
    /// own gate, so a value the wire refuses is refused here as well.
    ///
    /// No arm invents a routing recipient: `RoutedEvent` owns the destination
    /// and stays outside the packet, exactly as the domain keeps it outside
    /// the publication.
    fn try_from(event: Event) -> Result<Self, Self::Error> {
        match event {
            Event::ChunkSnapshot(snapshot) => {
                let mut sections: Vec<SectionData> = Vec::with_capacity(SECTIONS_PER_CHUNK);
                for (index, section) in snapshot.sections().iter().enumerate() {
                    sections.push(SectionData::try_from((section.clone(), index as i32))?);
                }
                Ok(ServerPacket::ChunkSnapshot(ChunkSnapshot::new(
                    snapshot.dimension(),
                    snapshot.chunk().x(),
                    snapshot.chunk().z(),
                    snapshot.revision(),
                    sections,
                )?))
            }
            Event::BlockChanges(batch) => {
                check_record_cap(batch.changes().len(), MAX_BLOCK_CHANGES)?;
                let changes: Vec<BlockChange> = batch
                    .changes()
                    .iter()
                    .map(|change| BlockChange {
                        x: change.position().x(),
                        y: change.position().y(),
                        z: change.position().z(),
                        block: change.block(),
                    })
                    .collect();
                Ok(ServerPacket::BlockChanges(BlockChanges::new(
                    batch.dimension(),
                    batch.chunk().x(),
                    batch.chunk().z(),
                    batch.base_revision(),
                    batch.new_revision(),
                    changes,
                )?))
            }
            Event::ForgetChunks(batch) => {
                check_record_cap(batch.chunks().len(), MAX_FORGET_CHUNKS)?;
                let chunks: Vec<(i32, i32)> = batch
                    .chunks()
                    .iter()
                    .map(|chunk| (chunk.x(), chunk.z()))
                    .collect();
                Ok(ServerPacket::ForgetChunks(ForgetChunks::new(
                    batch.dimension(),
                    chunks,
                )?))
            }
            Event::PlayerState(state) => {
                let (mining_active, mining_target, progress, required, harvestable) =
                    wire_mining_state(state.mining());
                let record = PlayerState {
                    server_tick: state.server_tick(),
                    last_input_sequence: state.last_input_sequence(),
                    dimension: state.dimension(),
                    position: state.motion().position().get(),
                    velocity: state.motion().velocity().get(),
                    yaw: state.look().yaw(),
                    pitch: state.look().pitch(),
                    on_ground: state.motion().on_ground(),
                    ready: state.ready(),
                    reset: state.reset(),
                    mining_active,
                    mining_target,
                    mining_progress_ticks: progress,
                    mining_required_ticks: required,
                    mining_harvestable: harvestable,
                    health: state.survival().health(),
                    oxygen: state.survival().oxygen(),
                    hunger: state.survival().hunger(),
                    saturation_zero: state.survival().saturation_zero(),
                    day_phase_offset: state.world().day_phase_offset(),
                    world_time_ticks: state.world().world_time_ticks(),
                    weather_kind: state.world().weather().wire_id(),
                    season: state.world().season().wire_id(),
                    season_progress: state.world().season_progress(),
                    temperature: state.world().temperature(),
                    armor_points: state.survival().armor_points(),
                };
                record.validate()?;
                Ok(ServerPacket::PlayerState(record))
            }
            Event::CommandRejected(rejection) => {
                Ok(ServerPacket::CommandRejected(CommandRejected::new(
                    rejection.sequence(),
                    reject_reason_to_wire(rejection.reason())?,
                )?))
            }
            Event::RemotePlayerSpawn(spawn) => {
                Ok(ServerPacket::RemotePlayerSpawn(RemotePlayerSpawn::new(
                    spawn.player_id(),
                    spawn.display_name().as_str().to_owned(),
                    spawn.server_tick(),
                    spawn.dimension(),
                    spawn.position().get(),
                    spawn.look().yaw(),
                    spawn.look().pitch(),
                )?))
            }
            Event::RemotePlayerDespawn(despawn) => Ok(ServerPacket::RemotePlayerDespawn(
                RemotePlayerDespawn::new(despawn.player_id()),
            )),
            Event::RemotePlayerStates(batch) => {
                check_record_cap(batch.states().len(), MAX_REMOTE_PLAYER_STATES)?;
                let states: Vec<RemotePlayerState> = batch
                    .states()
                    .iter()
                    .map(|state| RemotePlayerState {
                        player_id: state.player_id(),
                        dimension: state.dimension(),
                        position: state.position().get(),
                        yaw: state.look().yaw(),
                        pitch: state.look().pitch(),
                        reset: state.reset(),
                    })
                    .collect();
                Ok(ServerPacket::RemotePlayerStates(RemotePlayerStates::new(
                    batch.server_tick(),
                    states,
                )?))
            }
            Event::InventoryState(state) => {
                let mut hotbar = [ItemStack::EMPTY; crate::inventory_state::HOTBAR_SLOTS];
                hotbar.copy_from_slice(state.hotbar());
                let mut backpack = [ItemStack::EMPTY; crate::inventory_state::BACKPACK_SLOTS];
                backpack.copy_from_slice(state.backpack());
                Ok(ServerPacket::InventoryState(InventoryState::new(
                    state.selected().get(),
                    hotbar,
                    backpack,
                )?))
            }
            Event::ItemDropUpserts(batch) => {
                check_record_cap(batch.drops().len(), MAX_ITEM_DROP_BATCH)?;
                let drops: Vec<ItemDrop> = batch
                    .drops()
                    .iter()
                    .map(|drop| ItemDrop {
                        id: drop.id(),
                        block_index: drop.block_index(),
                        item: drop.stack().item(),
                        count: drop.stack().count(),
                        durability: drop.stack().durability(),
                    })
                    .collect();
                Ok(ServerPacket::ItemDropUpserts(ItemDropUpserts::new(
                    batch.server_tick(),
                    drops,
                )?))
            }
            Event::ItemDropRemoves(batch) => {
                check_record_cap(batch.ids().len(), MAX_ITEM_DROP_BATCH)?;
                Ok(ServerPacket::ItemDropRemoves(ItemDropRemoves::new(
                    batch.server_tick(),
                    batch.ids().to_vec(),
                )?))
            }
            Event::FurnaceState(state) => Ok(ServerPacket::FurnaceState(FurnaceState::new(
                wire_container(&state.container()),
                state.input(),
                state.fuel(),
                state.output(),
                state.progress_ticks(),
                state.burn_ticks(),
            )?)),
            Event::ContainerClosed(closed) => Ok(ServerPacket::ContainerClosed(
                ContainerClosed::new(wire_container(&closed.container()))?,
            )),
            Event::ChestState(state) => {
                let mut items = [ItemStack::EMPTY; CHEST_SLOTS];
                items.copy_from_slice(state.items());
                Ok(ServerPacket::ChestState(ChestState::new(
                    wire_container(&state.container()),
                    items,
                )?))
            }
            Event::Chat(event) => Ok(ServerPacket::ChatEvent(ChatEvent::try_from(event)?)),
            Event::CompanionSpawn(spawn) => Ok(ServerPacket::CompanionSpawn(CompanionSpawn::new(
                spawn.id(),
                spawn.name().as_str().to_owned(),
                spawn.server_tick(),
                spawn.dimension(),
                spawn.position().get(),
                spawn.look().yaw(),
                spawn.look().pitch(),
            )?)),
            Event::CompanionStates(batch) => {
                check_record_cap(batch.states().len(), MAX_COMPANION_STATES)?;
                let states: Vec<CompanionState> = batch
                    .states()
                    .iter()
                    .map(|state| CompanionState {
                        companion_id: state.id(),
                        dimension: state.dimension(),
                        position: state.position().get(),
                        yaw: state.look().yaw(),
                        pitch: state.look().pitch(),
                        reset: state.reset(),
                    })
                    .collect();
                Ok(ServerPacket::CompanionStates(CompanionStates::new(
                    batch.server_tick(),
                    states,
                )?))
            }
            Event::CompanionDespawn(despawn) => Ok(ServerPacket::CompanionDespawn(
                CompanionDespawn::new(despawn.id()),
            )),
            Event::PlaceBlockSucceeded(success) => Ok(ServerPacket::PlaceBlockSucceeded(
                PlaceBlockSucceeded::new(success.sequence()),
            )),
            Event::CraftingState(state) => {
                let slots: [ItemStack; 9] = state
                    .slots()
                    .to_vec()
                    .try_into()
                    .expect("the fixed nine crafting grid slots");
                Ok(ServerPacket::CraftingState(CraftingState::new(
                    wire_crafting_size(state.size()),
                    slots,
                    state.output(),
                )?))
            }
            Event::HostileSpawn(batch) => {
                check_record_cap(
                    batch.spawns().len(),
                    crate::hostile_spawn::HOSTILE_SPAWN_MAX_RECORDS,
                )?;
                let spawns: Vec<HostileSpawnRecord> = batch
                    .spawns()
                    .iter()
                    .map(|record| HostileSpawnRecord {
                        id: record.id(),
                        dimension: record.dimension(),
                        position: record.position().get(),
                        yaw: record.yaw(),
                        health: record.health(),
                        kind: wire_hostile_kind(record.kind()),
                    })
                    .collect();
                Ok(ServerPacket::HostileSpawn(HostileSpawn::new(
                    batch.server_tick(),
                    spawns,
                )?))
            }
            Event::HostileState(batch) => {
                check_record_cap(
                    batch.states().len(),
                    crate::hostile_state::HOSTILE_STATE_MAX_RECORDS,
                )?;
                let states: Vec<HostileStateRecord> = batch
                    .states()
                    .iter()
                    .map(|record| HostileStateRecord {
                        id: record.id(),
                        position: record.position().get(),
                        velocity: record.velocity().get(),
                        yaw: record.yaw(),
                        health: record.health(),
                        kind: wire_hostile_kind(record.kind()),
                    })
                    .collect();
                Ok(ServerPacket::HostileState(HostileState::new(
                    batch.server_tick(),
                    states,
                )?))
            }
            Event::HostileDespawn(batch) => {
                check_record_cap(batch.ids().len(), MAX_HOSTILE_RECORDS)?;
                Ok(ServerPacket::HostileDespawn(HostileDespawn::new(
                    batch.server_tick(),
                    batch.ids().to_vec(),
                )?))
            }
            Event::CombatHit(hit) => Ok(ServerPacket::CombatHit(CombatHit::new(
                hit.server_tick(),
                hit.damage(),
                hit.target().wire_id(),
            )?)),
            Event::PassiveSpawn(batch) => {
                check_record_cap(
                    batch.spawns().len(),
                    crate::passive_spawn::MAX_PASSIVE_SPAWN_RECORDS,
                )?;
                let spawns: Vec<crate::passive_spawn::PassiveSpawnRecord> = batch
                    .spawns()
                    .iter()
                    .map(|record| crate::passive_spawn::PassiveSpawnRecord {
                        id: record.id(),
                        dimension: record.dimension(),
                        position: record.position().get(),
                        yaw: record.yaw(),
                        health: record.health(),
                    })
                    .collect();
                Ok(ServerPacket::PassiveSpawn(PassiveSpawn::new(
                    batch.server_tick(),
                    spawns,
                )?))
            }
            Event::PassiveState(batch) => {
                check_record_cap(
                    batch.states().len(),
                    crate::passive_state::MAX_PASSIVE_STATE_RECORDS,
                )?;
                let states: Vec<crate::passive_state::PassiveStateRecord> = batch
                    .states()
                    .iter()
                    .map(|record| crate::passive_state::PassiveStateRecord {
                        id: record.id(),
                        position: record.position().get(),
                        velocity: record.velocity().get(),
                        yaw: record.yaw(),
                        health: record.health(),
                        grazing: u8::from(record.grazing()),
                    })
                    .collect();
                Ok(ServerPacket::PassiveState(PassiveState::new(
                    batch.server_tick(),
                    states,
                )?))
            }
            Event::PassiveDespawn(batch) => {
                check_record_cap(
                    batch.despawns().len(),
                    crate::passive_despawn::MAX_PASSIVE_RECORDS,
                )?;
                let despawns: Vec<PassiveDespawnRecord> = batch
                    .despawns()
                    .iter()
                    .map(|record| PassiveDespawnRecord {
                        id: record.id(),
                        reason: wire_passive_despawn_reason(record.reason()),
                    })
                    .collect();
                Ok(ServerPacket::PassiveDespawn(PassiveDespawn::new(
                    batch.server_tick(),
                    despawns,
                )?))
            }
            Event::ProjectileSpawn(batch) => {
                check_record_cap(batch.spawns().len(), MAX_PROJECTILE_RECORDS)?;
                let spawns: Vec<ProjectileSpawnRecord> = batch
                    .spawns()
                    .iter()
                    .map(|record| ProjectileSpawnRecord {
                        id: record.id(),
                        kind: wire_projectile_kind(record.kind()),
                        dimension: record.dimension(),
                        position: record.position().get(),
                        velocity: record.velocity().get(),
                    })
                    .collect();
                Ok(ServerPacket::ProjectileSpawn(ProjectileSpawn::new(
                    batch.server_tick(),
                    spawns,
                )?))
            }
            Event::ProjectileState(batch) => {
                check_record_cap(batch.states().len(), MAX_PROJECTILE_RECORDS)?;
                let states: Vec<crate::projectile_state::ProjectileStateRecord> = batch
                    .states()
                    .iter()
                    .map(|record| crate::projectile_state::ProjectileStateRecord {
                        id: record.id(),
                        position: record.position().get(),
                    })
                    .collect();
                Ok(ServerPacket::ProjectileState(ProjectileState::new(
                    batch.server_tick(),
                    states,
                )?))
            }
            Event::ProjectileDespawn(batch) => {
                check_record_cap(batch.ids().len(), MAX_PROJECTILE_RECORDS)?;
                Ok(ServerPacket::ProjectileDespawn(ProjectileDespawn::new(
                    batch.server_tick(),
                    batch.ids().to_vec(),
                )?))
            }
        }
    }
}
