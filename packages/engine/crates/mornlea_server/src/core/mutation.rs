//! Authority-resolved atomic world transactions.
//!
//! The resolvers turn human intent and companion proposals into complete,
//! fully prefetched transactions over the authority read view; the
//! `MutationTxn` entry points on `TickContext` then validate and commit them
//! atomically. Every target cell, generation, revision, footprint and
//! inventory debit is derived from authority state — neither a client intent
//! nor a companion proposal ever supplies a target or a revision. Later rule
//! providers consume exactly these entry points; they never construct
//! resolved values themselves.
//!
//! Block and item numbering, footprints, typed reject reasons, tool
//! durability and output capacities are mirrored from the frozen Go tables,
//! each cited at its table below. Where this module deliberately narrows a Go
//! behavior, the boundary comment names the provider that owns the rest.

use mornlea_domain::{
    BlockPos, ChunkPos, CompanionId, ContainerKind, Dimension, FiniteVec3, PlacementIntent,
    PlayerControl, RejectReason, registered_block,
};
use mornlea_engine::native::contracts::raycast::{Ray, RayCursor, RayFace, RaycastOp};
use mornlea_engine::native::raycast::NativeRaycast;
use mornlea_storage::ItemStack;

use super::contracts::{
    ActorKey, ActorLifecycle, BlockObservation, BlockTxn, BlockWrite, CapturedContainer, ChunkKey,
    ContainerRecord, ContainerSlots, DropBatch, DropSource, EnvironmentState, InventoryPatch,
    InventoryRecord, MutationProducer, ResolvedMining, ResolvedPlacement, RuleReject,
};
use super::state::AuthorityReadView;
use crate::rules::harvest;

/// Hotbar length inside the unified 36-slot inventory (`core.HotbarSlots`).
const HOTBAR_SLOTS: usize = 9;

/// Unified inventory length (`core.InventorySlots`, hotbar plus backpack,
/// `packages/shared/core/inventory.go`).
const INVENTORY_SLOTS: usize = HOTBAR_SLOTS + 27;

// Stable block numbers, mirrored from the frozen const block in
// `packages/shared/core/block.go`. Only the numbers this module reads are
// named; predicates below own the ranges.
const AIR: u16 = 0; // `core.AirID`
const GLASS: u16 = 20; // `core.GlassID`
const LEAVES: u16 = 19; // `core.LeavesID`
const FURNACE_BLOCK: u16 = 9; // `core.FurnaceID`
const CHEST_BLOCK: u16 = 11; // `core.ChestID`
const DOOR_LOWER_FIRST: u16 = 62; // `core.DoorLowerSouthClosed`
const DOOR_UPPER: u16 = 70; // `core.DoorUpper`
const TORCH_STANDING: u16 = 71; // `core.TorchStandingID`
const TORCH_WALL_POS_X: u16 = 72; // `core.TorchWallPosXID`
const TORCH_WALL_NEG_X: u16 = 73; // `core.TorchWallNegXID`
const TORCH_WALL_POS_Z: u16 = 74; // `core.TorchWallPosZID`
const TORCH_WALL_NEG_Z: u16 = 75; // `core.TorchWallNegZID`
const BED_FOOT_SOUTH: u16 = 76; // `core.BedFootSouthID`
const BED_HEAD_SPAN: u16 = 7; // last bed form minus `BED_FOOT_SOUTH`

// Stable item numbers, mirrored from the frozen const block in
// `packages/shared/core/item.go`.
const ITEM_NONE: u16 = 0; // `core.ItemNone`
const ITEM_TORCH: u16 = 44; // `core.ItemTorch`

/// Reports whether a block number is one of the eight fluid forms
/// (`core.IsFluid`, `packages/shared/core/fluid.go`).
pub(crate) fn is_fluid(block: u16) -> bool {
    (27..=34).contains(&block)
}

/// Reports whether a block number is dry or wet farmland (`core.IsFarmland`,
/// `packages/shared/core/farming.go`).
pub(crate) fn is_farmland(block: u16) -> bool {
    (35..=36).contains(&block)
}

/// Reports whether a block number is any crop stage — wheat, potato or
/// carrot (`core.IsCrop`, `packages/shared/core/farming.go`).
pub(crate) fn is_crop(block: u16) -> bool {
    (37..=44).contains(&block) || (46..=53).contains(&block) || (54..=61).contains(&block)
}

/// Reports whether a block number is the wild short grass
/// (`core.IsWildGrass`, `packages/shared/core/farming.go`).
pub(crate) fn is_wild_grass(block: u16) -> bool {
    block == 84 // `core.ShortGrassID`
}

/// Reports whether a block number is the sapling (`core.IsSapling`).
fn is_sapling(block: u16) -> bool {
    block == 89 // `core.SaplingID`
}

/// Reports whether a block number uses plant semantics (`core.IsPlant`).
fn is_plant(block: u16) -> bool {
    is_crop(block) || is_wild_grass(block) || is_sapling(block)
}

/// Reports whether a block number is a door half (`core.IsDoor`).
fn is_door(block: u16) -> bool {
    (DOOR_LOWER_FIRST..=DOOR_UPPER).contains(&block)
}

/// Reports whether a block number is a bed form (`core.IsBed`,
/// `packages/shared/core/bed.go`).
fn is_bed(block: u16) -> bool {
    (BED_FOOT_SOUTH..=BED_FOOT_SOUTH + BED_HEAD_SPAN).contains(&block)
}

/// Reports whether a block number is a torch form (`core.IsTorch`,
/// `packages/shared/core/block_properties.go`).
pub(crate) fn is_torch(block: u16) -> bool {
    (TORCH_STANDING..=TORCH_WALL_NEG_Z).contains(&block)
}

/// Reports whether a block number is a snow layer (`core.IsSnowLayer`).
pub(crate) fn is_snow_layer(block: u16) -> bool {
    (85..=88).contains(&block)
}

/// Stone pick (`core.ItemStonePickaxe`).
const ITEM_STONE_PICKAXE: u16 = 10;

/// Iron pick (`core.ItemIronPickaxe`).
const ITEM_IRON_PICKAXE: u16 = 11;

/// Spent stone pick (`core.ItemBrokenStonePickaxe`).
const ITEM_BROKEN_STONE_PICKAXE: u16 = 12;

/// Spent iron pick (`core.ItemBrokenIronPickaxe`).
const ITEM_BROKEN_IRON_PICKAXE: u16 = 13;

/// Soil-group and related blocks at 5 ticks (`miningRule`,
/// `packages/server/sim/entity/mining.go`): dirt, grass, sand, gravel,
/// leaves, glass, wool, clay and snow block.
const FIVE_TICK_BLOCKS: [u16; 9] = [3, 4, 15, 16, 19, 20, 22, 24, 25];

/// Wooden blocks at 15 ticks: log, planks and workbench.
const WOOD_BLOCKS: [u16; 3] = [17, 18, 45];

/// Stone-tier blocks whose tool row is 30/15/8: stone, cobblestone, smooth
/// stone, brick, roof tile and mossy cobblestone.
const STONE_TIER: [u16; 6] = [2, 13, 14, 21, 23, 26];

/// Stonebrick-tier blocks whose tool row is 30-failed/15/8: stonebrick,
/// furnace, chest, light block, coal ore and iron ore.
const STONEBRICK_TIER: [u16; 6] = [6, 9, 11, 12, 7, 8];

/// Iron block (`core.IronBlockID`): stone pick 20 failed, iron pick 10.
const IRON_BLOCK: u16 = 10;

/// Required ticks and harvestability of one block under one held item, the
/// exact `miningRule` table (`packages/server/sim/entity/mining.go`). The zero
/// sentinel means unmineable and always pairs with `false`; the native ray
/// never decides mineability, so this table is the only gate.
pub(crate) fn mining_rule(block: u16, held: u16) -> (u16, bool) {
    if is_door(block) || is_bed(block) {
        return (15, true);
    }
    if is_crop(block) || is_wild_grass(block) || is_sapling(block) {
        return (1, true);
    }
    if is_snow_layer(block) {
        return (1, false);
    }
    if is_farmland(block) || FIVE_TICK_BLOCKS.contains(&block) {
        return (5, true);
    }
    if WOOD_BLOCKS.contains(&block) {
        return (15, true);
    }
    if STONE_TIER.contains(&block) {
        return match held {
            ITEM_NONE | ITEM_BROKEN_STONE_PICKAXE | ITEM_BROKEN_IRON_PICKAXE => (30, true),
            ITEM_STONE_PICKAXE => (15, true),
            ITEM_IRON_PICKAXE => (8, true),
            _ => (30, false),
        };
    }
    if STONEBRICK_TIER.contains(&block) {
        return match held {
            ITEM_STONE_PICKAXE => (15, true),
            ITEM_IRON_PICKAXE => (8, true),
            _ => (30, false),
        };
    }
    if block == IRON_BLOCK {
        return match held {
            ITEM_STONE_PICKAXE => (20, false),
            ITEM_IRON_PICKAXE => (10, true),
            _ => (40, false),
        };
    }
    (0, false)
}

/// Item-to-block placement mapping, the exact `core.ItemPlacement` table
/// (`packages/shared/core/item.go`): the block one placeable item writes.
/// The torch is absent on purpose — its form depends on the hit face and is
/// resolved by [`placeable_block_at_face`].
fn item_placement(item: u16) -> Option<u16> {
    match item {
        1 => Some(2),   // ItemStone -> StoneID
        2 => Some(3),   // ItemDirt -> DirtID
        3 => Some(4),   // ItemGrass -> GrassID
        4 => Some(6),   // ItemStoneBrick -> StoneBrickID
        8 => Some(9),   // ItemFurnace -> FurnaceID
        9 => Some(10),  // ItemIronBlock -> IronBlockID
        14 => Some(11), // ItemChest -> ChestID
        15 => Some(12), // ItemLightBlock -> LightBlockID
        16 => Some(13), // ItemCobblestone -> CobblestoneID
        17 => Some(14), // ItemSmoothStone -> SmoothStoneID
        18 => Some(15), // ItemSand -> SandID
        19 => Some(16), // ItemGravel -> GravelID
        20 => Some(17), // ItemOakLog -> OakLogID
        21 => Some(18), // ItemOakPlanks -> OakPlanksID
        22 => Some(19), // ItemLeaves -> LeavesID
        23 => Some(20), // ItemGlass -> GlassID
        24 => Some(21), // ItemBrick -> BrickID
        25 => Some(22), // ItemWhiteWool -> WhiteWoolID
        26 => Some(23), // ItemRoofTile -> RoofTileID
        27 => Some(24), // ItemClay -> ClayID
        28 => Some(25), // ItemSnowBlock -> SnowBlockID
        29 => Some(26), // ItemMossyCobblestone -> MossyCobblestoneID
        34 => Some(37), // ItemWheatSeeds -> WheatStage0ID
        38 => Some(45), // ItemWorkbench -> WorkbenchID
        40 => Some(46), // ItemPotato -> PotatoStage0ID
        41 => Some(54), // ItemCarrot -> CarrotStage0ID
        43 => Some(62), // ItemDoor -> DoorLowerSouthClosed
        46 => Some(76), // ItemBed -> BedFootSouthID
        57 => Some(89), // ItemSapling -> SaplingID
        _ => None,
    }
}

/// Block-to-item drop mapping, the exact `core.BlockDrop` table
/// (`packages/shared/core/item.go`). The mining-rule table owns mineability;
/// short grass and snow use special no-body output paths, while protected
/// blocks have no progression (`packages/server/sim/entity/mining.go`).
fn block_drop(block: u16) -> Option<u16> {
    match block {
        2 => Some(1),        // StoneID -> ItemStone
        3 => Some(2),        // DirtID -> ItemDirt
        4 => Some(3),        // GrassID -> ItemGrass
        6 => Some(4),        // StoneBrickID -> ItemStoneBrick
        7 => Some(5),        // CoalOreID -> ItemCoal
        8 => Some(6),        // IronOreID -> ItemRawIron
        9 => Some(8),        // FurnaceID -> ItemFurnace
        10 => Some(9),       // IronBlockID -> ItemIronBlock
        11 => Some(14),      // ChestID -> ItemChest
        12 => Some(15),      // LightBlockID -> ItemLightBlock
        13 => Some(16),      // CobblestoneID -> ItemCobblestone
        14 => Some(17),      // SmoothStoneID -> ItemSmoothStone
        15 => Some(18),      // SandID -> ItemSand
        16 => Some(19),      // GravelID -> ItemGravel
        17 => Some(20),      // OakLogID -> ItemOakLog
        18 => Some(21),      // OakPlanksID -> ItemOakPlanks
        19 => Some(22),      // LeavesID -> ItemLeaves
        20 => Some(23),      // GlassID -> ItemGlass
        21 => Some(24),      // BrickID -> ItemBrick
        22 => Some(25),      // WhiteWoolID -> ItemWhiteWool
        23 => Some(26),      // RoofTileID -> ItemRoofTile
        24 => Some(27),      // ClayID -> ItemClay
        25 => Some(28),      // SnowBlockID -> ItemSnowBlock
        26 => Some(29),      // MossyCobblestoneID -> ItemMossyCobblestone
        35 | 36 => Some(2),  // FarmlandDryID/FarmlandWetID -> ItemDirt
        37..=43 => Some(34), // WheatStage0ID..WheatStage6ID -> ItemWheatSeeds
        44 => Some(35),      // WheatStage7ID -> ItemWheat
        45 => Some(38),      // WorkbenchID -> ItemWorkbench
        46..=53 => Some(40), // PotatoStage0ID..PotatoStage7ID -> ItemPotato
        54..=61 => Some(41), // CarrotStage0ID..CarrotStage7ID -> ItemCarrot
        62..=70 => Some(43), // door forms -> ItemDoor
        71..=75 => Some(44), // torch forms -> ItemTorch
        76..=83 => Some(46), // bed forms -> ItemBed
        89 => Some(57),      // SaplingID -> ItemSapling
        _ => None,
    }
}

/// The spent form of one durable item, the exact `core.ItemBrokenForm` table
/// (`packages/shared/core/item.go`); armor has no broken form because its
/// durability is expressed in place.
fn item_broken_form(item: u16) -> Option<u16> {
    match item {
        10 => Some(12), // ItemStonePickaxe -> ItemBrokenStonePickaxe
        11 => Some(13), // ItemIronPickaxe -> ItemBrokenIronPickaxe
        30 => Some(32), // ItemStoneHoe -> ItemBrokenStoneHoe
        31 => Some(33), // ItemIronHoe -> ItemBrokenIronHoe
        47 => Some(50), // ItemWoodenSword -> ItemBrokenWoodenSword
        48 => Some(51), // ItemStoneSword -> ItemBrokenStoneSword
        49 => Some(52), // ItemIronSword -> ItemBrokenIronSword
        62 => Some(65), // ItemBow -> ItemBrokenBow
        _ => None,
    }
}

/// Item-times-hit-face placement form, the exact
/// `core.PlaceableBlockAtFace` window (`packages/shared/core/block_properties.go`):
/// the torch is the only face-dependent item — top face stands, each horizontal
/// face takes the same-named wall form, the bottom face has no form. Every
/// other placeable item keeps its face-independent [`item_placement`] block.
fn placeable_block_at_face(item: u16, face: RayFace) -> Option<u16> {
    if item == ITEM_TORCH {
        return match face {
            RayFace::PosY => Some(TORCH_STANDING),
            RayFace::PosX => Some(TORCH_WALL_POS_X),
            RayFace::NegX => Some(TORCH_WALL_NEG_X),
            RayFace::PosZ => Some(TORCH_WALL_POS_Z),
            RayFace::NegZ => Some(TORCH_WALL_NEG_Z),
            _ => None,
        };
    }
    item_placement(item)
}

/// Horizontal facing from yaw, the exact `yawToDoorDir` sector rule
/// (`packages/server/sim/entity/door.go`): south 0, west 1, north 2, east 3
/// over the yaw normalized to `[-pi, pi)`.
fn yaw_to_facing(yaw: f32) -> u8 {
    let two_pi = 2.0 * std::f64::consts::PI;
    let mut normalized = f64::from(yaw) + std::f64::consts::PI;
    normalized %= two_pi;
    if normalized < 0.0 {
        normalized += two_pi;
    }
    let yaw_norm = (normalized - std::f64::consts::PI) as f32;
    let quarter = std::f32::consts::FRAC_PI_4;
    let three_quarters = 3.0 * quarter;
    if (-quarter..quarter).contains(&yaw_norm) {
        0
    } else if (quarter..three_quarters).contains(&yaw_norm) {
        1
    } else if yaw_norm >= three_quarters || yaw_norm < -three_quarters {
        2
    } else {
        3
    }
}

/// Closed door-lower form of one facing (`doorLowerID`,
/// `packages/server/sim/entity/door.go`; the frozen order in
/// `packages/shared/core/block.go` runs south/west/north/east, closed before
/// open).
fn door_lower_closed(facing: u8) -> u16 {
    DOOR_LOWER_FIRST + u16::from(facing) * 2
}

/// Bed foot form of one facing (`core.BedFootID` over `bedFootIDs`,
/// `packages/shared/core/bed.go`).
fn bed_foot_id(facing: u8) -> u16 {
    BED_FOOT_SOUTH + u16::from(facing)
}

/// Bed head form of one facing (`core.BedHeadID`): the head block is the
/// same-facing foot form shifted by four (`bedHeadIDs`, `core/bed.go`).
fn bed_head_id(facing: u8) -> u16 {
    BED_FOOT_SOUTH + u16::from(facing) + 4
}

/// Head-cell offset from the foot cell of one facing (`bedHeadOffsets`,
/// `packages/shared/core/bed.go`): south `+Z`, west `-X`, north `-Z`,
/// east `+X`.
fn bed_head_offset(facing: u8) -> (i32, i32) {
    match facing {
        0 => (0, 1),
        1 => (-1, 0),
        2 => (0, -1),
        _ => (1, 0),
    }
}

/// Solid-support predicate for door and bed placement (`isSolidSupport`,
/// `packages/server/sim/entity/door.go`): farmland, or any registered,
/// non-air, non-glass, non-leaves, non-fluid, non-plant, non-door block.
fn solid_support(block: u16) -> bool {
    is_farmland(block)
        || (registered_block(block)
            && block != AIR
            && block != GLASS
            && block != LEAVES
            && !is_fluid(block)
            && !is_plant(block)
            && !is_door(block))
}

/// Chunk column of a world position (`core.BlockPos.Chunk`,
/// `packages/shared/core/pos.go`; the domain keeps its copy private to the
/// section codec). The shift is arithmetic so negative coordinates floor
/// toward negative infinity.
fn chunk_of(pos: BlockPos) -> ChunkPos {
    ChunkPos::new(pos.x() >> 4, pos.z() >> 4)
}

/// Chunk key of a cell in one dimension.
fn chunk_key(dimension: Dimension, pos: BlockPos) -> ChunkKey {
    ChunkKey {
        dimension,
        pos: chunk_of(pos),
    }
}

/// World center of one block cell, the anchor a mined batch drops at.
fn block_center(pos: BlockPos) -> FiniteVec3 {
    FiniteVec3::try_new([
        pos.x() as f32 + 0.5,
        pos.y() as f32 + 0.5,
        pos.z() as f32 + 0.5,
    ])
    .expect("block centers are finite")
}

/// Unit look direction of a rotation, the exact `LookDirection` formula
/// (`packages/server/sim/entity/command.go`): yaw zero faces north (`-Z`),
/// positive pitch looks up.
fn look_direction(yaw: f32, pitch: f32) -> [f32; 3] {
    let cos_pitch = pitch.cos();
    [-yaw.sin() * cos_pitch, pitch.sin(), -yaw.cos() * cos_pitch]
}

/// One classified ray hit: the full observation of the first non-air cell
/// and the face the ray entered it through.
struct RayHit {
    observed: BlockObservation,
    face: RayFace,
}

/// Walks the F1 ray kernel (`NativeRaycast`) batch by batch and classifies
/// every traversed cell against the view: an unobserved cell reports
/// `unobserved` (the authority cannot certify geometry it has not observed),
/// observed air continues the walk, and any other block is the hit. The
/// origin record is classified like any other cell, so a ray starting inside
/// a solid cell hits it with the `Origin` face.
fn cast_interaction_ray(
    view: &AuthorityReadView<'_>,
    dimension: Dimension,
    origin: [f32; 3],
    direction: [f32; 3],
    reach: f32,
    unobserved: RuleReject,
) -> Result<Option<RayHit>, RuleReject> {
    let length =
        (direction[0] * direction[0] + direction[1] * direction[1] + direction[2] * direction[2])
            .sqrt();
    if !length.is_finite() || length < 1e-6 {
        return Err(RuleReject::Wire(RejectReason::InvalidRay));
    }
    let normalized = [
        direction[0] / length,
        direction[1] / length,
        direction[2] / length,
    ];
    let mut cursor = RayCursor::try_new(Ray {
        origin,
        direction: normalized,
        maximum: reach,
    })
    .map_err(|_| RuleReject::Wire(RejectReason::InvalidRay))?;
    loop {
        let batch = NativeRaycast
            .next_batch(&mut cursor)
            .map_err(|_| RuleReject::Wire(RejectReason::InvalidRay))?;
        for record in batch.records() {
            let cell = BlockPos::new(record.cell[0], record.cell[1], record.cell[2]);
            match view.observation(dimension, cell) {
                None => return Err(unobserved),
                Some(observed) if observed.block == AIR => {}
                Some(observed) => {
                    return Ok(Some(RayHit {
                        observed,
                        face: record.face,
                    }));
                }
            }
        }
        if batch.is_done() {
            return Ok(None);
        }
    }
}

/// The neighbor cell across one entry face, the `adjacentBlock` translation
/// (`packages/server/sim/entity/placement.go`).
fn adjacent(cell: BlockPos, face: RayFace) -> BlockPos {
    let (dx, dy, dz) = match face {
        RayFace::NegX => (-1, 0, 0),
        RayFace::PosX => (1, 0, 0),
        RayFace::NegY => (0, -1, 0),
        RayFace::PosY => (0, 1, 0),
        RayFace::NegZ => (0, 0, -1),
        RayFace::PosZ => (0, 0, 1),
        RayFace::Origin => (0, 0, 0),
    };
    BlockPos::new(cell.x() + dx, cell.y() + dy, cell.z() + dz)
}

/// The observation basis every resolver must be able to certify: an active
/// actor record with its current pose and dimension, plus the environment
/// tunables. A missing actor or environment is `StaleObservation` rather
/// than a wire reject because it is the authority's own basis that is
/// unavailable, not the client payload that is invalid.
struct ActorBasis {
    dimension: Dimension,
    eye: [f32; 3],
    reach: f32,
    drop_pickup_delay: u8,
}

fn actor_basis(view: &AuthorityReadView<'_>, actor: ActorKey) -> Result<ActorBasis, RuleReject> {
    let record = view.actor(actor).ok_or(RuleReject::StaleObservation)?;
    if record.lifecycle != ActorLifecycle::Active {
        return Err(RuleReject::StaleObservation);
    }
    let environment: &EnvironmentState = view.environment().ok_or(RuleReject::StaleObservation)?;
    let tunables = environment.tunables;
    let position = record.motion.position().get();
    Ok(ActorBasis {
        dimension: record.dimension,
        eye: [
            position[0],
            position[1] + tunables.eye_height(),
            position[2],
        ],
        reach: tunables.interaction_reach(),
        drop_pickup_delay: tunables.drop_pickup_delay_ticks(),
    })
}

/// One observed cell that must be air; unobserved reports `unobserved` and a
/// non-air cell reports `Occupied`.
fn require_air(
    view: &AuthorityReadView<'_>,
    dimension: Dimension,
    pos: BlockPos,
    unobserved: RuleReject,
) -> Result<BlockObservation, RuleReject> {
    match view.observation(dimension, pos) {
        None => Err(unobserved),
        Some(observed) if observed.block == AIR => Ok(observed),
        Some(_) => Err(RuleReject::Wire(RejectReason::Occupied)),
    }
}

/// One observed cell below a footprint cell that must be solid support;
/// unobserved reports `ChunkNotReady` and a non-supporting block reports
/// `InvalidBlock` (the door and bed placement rules,
/// `packages/server/sim/entity/door.go` and `bed.go`).
fn require_support(
    view: &AuthorityReadView<'_>,
    dimension: Dimension,
    pos: BlockPos,
) -> Result<(), RuleReject> {
    match view.observation(dimension, pos) {
        None => Err(RuleReject::Wire(RejectReason::ChunkNotReady)),
        Some(below) if solid_support(below.block) => Ok(()),
        Some(_) => Err(RuleReject::Wire(RejectReason::InvalidBlock)),
    }
}

/// Consumes one item from the named hotbar slot (`core.Hotbar.Consume`,
/// `packages/shared/core/item.go`): the slot must hold a positive count and
/// the emptied slot becomes the canonical empty stack.
fn consume_hotbar_one(inventory: &InventoryRecord, slot: usize) -> Option<InventoryRecord> {
    let stack = inventory.slots[slot];
    if stack.item == ITEM_NONE || stack.count == 0 {
        return None;
    }
    let mut after = *inventory;
    after.slots[slot] = if stack.count == 1 {
        ItemStack::default()
    } else {
        ItemStack {
            item: stack.item,
            count: stack.count - 1,
            durability: stack.durability,
        }
    };
    Some(after)
}

/// Consumes one item from the first matching stack anywhere in the unified
/// 36-slot inventory (`consumeFirstInventoryItem`,
/// `packages/server/sim/entity/companion_placement.go`).
fn consume_first_item(inventory: &InventoryRecord, item: u16) -> Option<InventoryRecord> {
    for slot in 0..INVENTORY_SLOTS {
        let stack = inventory.slots[slot];
        if stack.item != item || stack.count == 0 {
            continue;
        }
        let mut after = *inventory;
        after.slots[slot] = if stack.count == 1 {
            ItemStack::default()
        } else {
            ItemStack {
                item,
                count: stack.count - 1,
                durability: stack.durability,
            }
        };
        return Some(after);
    }
    None
}

/// Credits one stack into the unified inventory, the exact
/// `core.Inventory.AddStack` phase order (`packages/shared/core/inventory.go`):
/// hotbar merge, hotbar empty, backpack merge, backpack empty, each phase in
/// ascending slot order with per-item stack-limit splitting. Returns the
/// next slots and any remainder that did not fit.
fn credit_stack(
    mut slots: [ItemStack; INVENTORY_SLOTS],
    source: ItemStack,
) -> ([ItemStack; INVENTORY_SLOTS], ItemStack) {
    if source.item == ITEM_NONE || !source.is_valid() {
        return (slots, source);
    }
    let Some(limit) = mornlea_domain::item_stack_limit(source.item) else {
        return (slots, source);
    };
    let mut remaining = source.count;
    for (first, last, merge) in [
        (0usize, HOTBAR_SLOTS, true),
        (0, HOTBAR_SLOTS, false),
        (HOTBAR_SLOTS, INVENTORY_SLOTS, true),
        (HOTBAR_SLOTS, INVENTORY_SLOTS, false),
    ] {
        for current in slots.iter_mut().take(last).skip(first) {
            if remaining == 0 {
                break;
            }
            if merge {
                if current.item != source.item || current.count >= limit {
                    continue;
                }
            } else if current.item != ITEM_NONE {
                continue;
            }
            let base = if merge { current.count } else { 0 };
            let moved = (limit - base).min(remaining);
            *current = ItemStack {
                item: source.item,
                count: base + moved,
                durability: source.durability,
            };
            remaining -= moved;
        }
        if remaining == 0 {
            return (slots, ItemStack::default());
        }
    }
    (
        slots,
        ItemStack {
            item: source.item,
            count: remaining,
            durability: source.durability,
        },
    )
}

/// Credits a whole output batch into a copy of one inventory; any stack that
/// does not fully fit refuses the whole batch with nothing applied, the
/// all-or-nothing staging of `CompanionMineContainerStaging`
/// (`packages/server/sim/entity/mining.go`).
fn credit_batch(inventory: &InventoryRecord, stacks: &[ItemStack]) -> Option<InventoryRecord> {
    let mut slots = inventory.slots;
    for stack in stacks {
        let (next, leftover) = credit_stack(slots, *stack);
        if leftover.item != ITEM_NONE || leftover.count != 0 {
            return None;
        }
        slots = next;
    }
    let mut after = *inventory;
    after.slots = slots;
    Some(after)
}

/// Wears the selected hotbar tool by one on a completed mine, the
/// single Go predicate `consumeMiningToolDurability` ->
/// `consumeToolDurability` -> `consumeToolDurabilityAt`
/// (`packages/server/sim/entity/mining.go`). The exemptions come first and
/// live here, not in a downstream provider, because a provider cannot remove
/// wear from an already-resolved transaction: a crop mined with an intact
/// hoe (`hoeHarvestDurabilityExempt`, `core.IsCrop` plus `core.TillingTool`,
/// `packages/shared/core/farming.go`), wild grass or a sapling of any held
/// item, and an intact sword on any block (`IsIntactSword`,
/// `packages/shared/core/item.go`). After the exemptions, only a single-copy
/// durable item wears: durability above one decrements, and durability one
/// becomes the registered broken form.
fn wear_selected_tool(inventory: &InventoryRecord, mined_block: u16) -> Option<InventoryRecord> {
    let index = usize::from(inventory.selected.get());
    let stack = inventory.slots[index];
    let exempt = (is_crop(mined_block) && tilling_tool(stack.item))
        || is_wild_grass(mined_block)
        || is_sapling(mined_block)
        || is_intact_sword(stack.item);
    if exempt || stack.count != 1 || mornlea_domain::durability_max(stack.item).is_none() {
        return None;
    }
    let mut after = *inventory;
    if stack.durability > 1 {
        after.slots[index] = ItemStack {
            item: stack.item,
            count: 1,
            durability: stack.durability - 1,
        };
        return Some(after);
    }
    let broken = item_broken_form(stack.item)?;
    after.slots[index] = ItemStack {
        item: broken,
        count: 1,
        durability: 0,
    };
    Some(after)
}

/// Reports whether an item is one of the two intact hoes (`core.TillingTool`,
/// `packages/shared/core/farming.go`); the broken forms are excluded there.
fn tilling_tool(item: u16) -> bool {
    matches!(item, 30 | 31)
}

/// Reports whether an item is an intact sword (`core.IsIntactSword`,
/// `packages/shared/core/item.go`).
fn is_intact_sword(item: u16) -> bool {
    matches!(item, 47..=49)
}

/// Captures the full slot record of a mined container block. A mined
/// container with no staged record is unready — the same `ChunkNotReady`
/// refusal the Go completion path publishes for a missing container record
/// (`packages/server/sim/entity/mining.go`).
fn capture_container(
    view: &AuthorityReadView<'_>,
    dimension: Dimension,
    pos: BlockPos,
    block: u16,
    unobserved: RuleReject,
) -> Result<ContainerRecord, RuleReject> {
    let kind = match block {
        CHEST_BLOCK => ContainerKind::Chest,
        FURNACE_BLOCK => ContainerKind::Furnace,
        _ => return Err(unobserved),
    };
    view.container_at(dimension, pos, kind).ok_or(unobserved)
}

/// The non-empty contents of one container record in fixed slot order — a
/// chest in 27-slot order, a furnace in input/fuel/output order — skipping
/// empty stacks exactly as the Go output paths do.
fn container_stacks(record: &ContainerRecord) -> Vec<ItemStack> {
    let slots = match &record.slots {
        ContainerSlots::Chest(slots) => slots.as_slice(),
        ContainerSlots::Furnace { slots, .. } => slots.as_slice(),
    };
    slots
        .iter()
        .copied()
        .filter(|stack| stack.item != ITEM_NONE && stack.count > 0)
        .collect()
}

fn output_stack(item: u16, count: u8) -> ItemStack {
    ItemStack {
        item,
        count,
        durability: 0,
    }
}

/// Prefetches both structural cells before output rehearsal. Coordinates are
/// checked before arithmetic and against the source world height, so an
/// unavailable partner can never turn a paired clear into a single clear.
fn mining_writes(
    view: &AuthorityReadView<'_>,
    dimension: Dimension,
    struck: BlockObservation,
    companion: bool,
) -> Result<(Vec<BlockWrite>, BlockPos), RuleReject> {
    let target = struck.pos;
    let mut positions = Vec::with_capacity(2);
    let mut anchor = target;
    if is_bed(struck.block) {
        let facing = ((struck.block - BED_FOOT_SOUTH) % 4) as u8;
        let (dx, dz) = bed_head_offset(facing);
        let head = struck.block >= BED_FOOT_SOUTH + 4;
        let other = BlockPos::new(
            target
                .x()
                .checked_add(if head { -dx } else { dx })
                .ok_or(RuleReject::StaleObservation)?,
            target.y(),
            target
                .z()
                .checked_add(if head { -dz } else { dz })
                .ok_or(RuleReject::StaleObservation)?,
        );
        positions.push(if head { other } else { target });
        positions.push(if head { target } else { other });
    } else if is_door(struck.block) && !companion {
        let upper = struck.block == DOOR_UPPER;
        let other_y = target
            .y()
            .checked_add(if upper { -1 } else { 1 })
            .ok_or(RuleReject::StaleObservation)?;
        let other = BlockPos::new(target.x(), other_y, target.z());
        anchor = if upper { other } else { target };
        positions.push(anchor);
        positions.push(if upper { target } else { other });
    } else {
        positions.push(target);
    }
    let mut writes = Vec::with_capacity(positions.len());
    for pos in positions {
        if !(-64..320).contains(&pos.y()) {
            return Err(RuleReject::Wire(RejectReason::ChunkNotReady));
        }
        let observed = if pos == target {
            struck
        } else {
            view.observation(dimension, pos)
                .ok_or(RuleReject::Wire(RejectReason::ChunkNotReady))?
        };
        writes.push(BlockWrite::try_new(observed, AIR)?);
    }
    Ok((writes, anchor))
}

/// Resolves one human placement intent into a complete transaction.
///
/// Preflight order: the actor's pose and reach, the ray with every traversed
/// cell's observation, the placement target, the held item's placement form
/// (which fixes the footprint family), every footprint cell with its support,
/// then the item debit. The resolved transaction carries each write's exact
/// observed basis; committing it through `MutationTxn::try_place` revalidates
/// that basis against the context and applies it atomically.
pub fn resolve_place(
    actor: ActorKey,
    intent: &PlacementIntent,
    view: &AuthorityReadView<'_>,
) -> Result<ResolvedPlacement, RuleReject> {
    let basis = actor_basis(view, actor)?;
    let look = intent.look();
    let direction = look_direction(look.yaw(), look.pitch());
    let hit = cast_interaction_ray(
        view,
        basis.dimension,
        basis.eye,
        direction,
        basis.reach,
        RuleReject::Wire(RejectReason::ChunkNotReady),
    )?
    .ok_or(RuleReject::Wire(RejectReason::NoTarget))?;
    if hit.face == RayFace::Origin {
        // The ray starts inside a solid cell, so there is no entry face to
        // place against; Go rejects the faceless hit as `Occupied`.
        return Err(RuleReject::Wire(RejectReason::Occupied));
    }
    let target = adjacent(hit.observed.pos, hit.face);
    let target_observed = require_air(
        view,
        basis.dimension,
        target,
        RuleReject::Wire(RejectReason::ChunkNotReady),
    )?;

    // The held item selects the placement form and with it the footprint
    // family: single-cell items write the target; door and bed items write
    // their two-cell footprint per the frozen tables (door: lower cell plus
    // the cell above, `tryPlaceDoor`, `packages/server/sim/entity/door.go`;
    // bed: foot cell plus the head cell along the facing, `tryPlaceBed` and
    // `packages/shared/core/bed.go`). An empty or non-placeable slot refuses
    // with the reason the Go hotbar oracle pins (`RejectInvalidBlock`,
    // `packages/server/sim/runtime/hotbar_test.go`). Every footprint cell is
    // prefetched before any write is built.
    let inventory = *view.inventory(actor).ok_or(RuleReject::StaleObservation)?;
    let slot = usize::from(intent.slot().get());
    let held = inventory.slots[slot];
    let form = placeable_block_at_face(held.item, hit.face)
        .ok_or(RuleReject::Wire(RejectReason::InvalidBlock))?;
    let facing = yaw_to_facing(look.yaw());
    let mut writes = Vec::with_capacity(2);
    if is_door(form) {
        let upper = BlockPos::new(target.x(), target.y() + 1, target.z());
        let upper_observed = require_air(
            view,
            basis.dimension,
            upper,
            RuleReject::Wire(RejectReason::ChunkNotReady),
        )?;
        require_support(
            view,
            basis.dimension,
            BlockPos::new(target.x(), target.y() - 1, target.z()),
        )?;
        writes.push(BlockWrite::try_new(
            target_observed,
            door_lower_closed(facing),
        )?);
        writes.push(BlockWrite::try_new(upper_observed, DOOR_UPPER)?);
    } else if is_bed(form) {
        let (dx, dz) = bed_head_offset(facing);
        let head = BlockPos::new(target.x() + dx, target.y(), target.z() + dz);
        let head_observed = require_air(
            view,
            basis.dimension,
            head,
            RuleReject::Wire(RejectReason::ChunkNotReady),
        )?;
        require_support(
            view,
            basis.dimension,
            BlockPos::new(target.x(), target.y() - 1, target.z()),
        )?;
        require_support(
            view,
            basis.dimension,
            BlockPos::new(head.x(), head.y() - 1, head.z()),
        )?;
        writes.push(BlockWrite::try_new(target_observed, bed_foot_id(facing))?);
        writes.push(BlockWrite::try_new(head_observed, bed_head_id(facing))?);
    } else {
        writes.push(BlockWrite::try_new(target_observed, form)?);
    }
    let after =
        consume_hotbar_one(&inventory, slot).ok_or(RuleReject::Wire(RejectReason::InvalidBlock))?;

    let txn = BlockTxn {
        producer: MutationProducer::Actor(actor),
        tick: view.tick(),
        writes,
        inventory: Some(InventoryPatch::try_new(actor, inventory, after)?),
        containers: Vec::new(),
        drops: None,
        mining: None,
    };
    Ok(ResolvedPlacement { txn })
}

/// Resolves one held-primary human mining control into the completed-mine
/// settlement transaction: the block's drop stack, the mined container's
/// captured contents, the human output capacity preflight and the selected
/// tool's wear. Released primary and a ray over observed air resolve to
/// `Ok(None)` — nothing to settle. Snow layers take the clear-only pre-branch
/// below and settle with no drops and no capacity gate.
pub fn resolve_mine(
    actor: ActorKey,
    control: &PlayerControl,
    view: &AuthorityReadView<'_>,
) -> Result<Option<ResolvedMining>, RuleReject> {
    if !control.actions().primary {
        return Ok(None);
    }
    let basis = actor_basis(view, actor)?;
    let look = control.look();
    let direction = look_direction(look.yaw(), look.pitch());
    let Some(hit) = cast_interaction_ray(
        view,
        basis.dimension,
        basis.eye,
        direction,
        basis.reach,
        RuleReject::Wire(RejectReason::ChunkNotReady),
    )?
    else {
        return Ok(None);
    };
    let observed = hit.observed;
    let inventory = *view.inventory(actor).ok_or(RuleReject::StaleObservation)?;
    let held = inventory.slots[usize::from(inventory.selected.get())];
    let (required, harvestable) = mining_rule(observed.block, held.item);
    if required == 0 {
        return Err(RuleReject::Wire(RejectReason::ProtectedBlock));
    }
    let (writes, anchor) = mining_writes(view, basis.dimension, observed, false)?;
    let mut stacks = Vec::new();
    let mut containers = Vec::new();
    if observed.block == CHEST_BLOCK || observed.block == FURNACE_BLOCK {
        let record = capture_container(
            view,
            basis.dimension,
            observed.pos,
            observed.block,
            RuleReject::Wire(RejectReason::ChunkNotReady),
        )?;
        if harvestable {
            stacks.push(output_stack(
                block_drop(observed.block).expect("registered container"),
                1,
            ));
        }
        stacks.extend(container_stacks(&record));
        containers.push(CapturedContainer {
            key: chunk_key(basis.dimension, observed.pos),
            record,
        });
    } else if is_door(observed.block) {
        if harvestable {
            stacks.push(output_stack(43, 1));
        }
    } else if is_bed(observed.block) {
        if harvestable {
            stacks.push(output_stack(46, 1));
        }
    } else if is_wild_grass(observed.block) {
        let seed = view.environment().ok_or(RuleReject::StaleObservation)?.seed;
        if harvest::short_grass(seed, u32::from(basis.dimension.get()), observed.pos) {
            stacks.push(output_stack(34, 1));
        }
    } else if is_snow_layer(observed.block) {
        // Snow has no item representation, but a durable selected tool still wears.
    } else if harvestable {
        let item =
            block_drop(observed.block).ok_or(RuleReject::Wire(RejectReason::ProtectedBlock))?;
        let seed = view.environment().ok_or(RuleReject::StaleObservation)?.seed;
        let dimension = u32::from(basis.dimension.get());
        match observed.block {
            44 => {
                let (wheat, seeds) = harvest::wheat(seed, view.tick(), dimension, observed.pos);
                stacks.push(output_stack(item, wheat));
                stacks.push(output_stack(34, seeds));
            }
            53 => {
                stacks.push(output_stack(
                    item,
                    harvest::potato(seed, view.tick(), dimension, observed.pos),
                ));
                if harvest::poison_potato(seed, view.tick(), dimension, observed.pos) {
                    stacks.push(output_stack(42, 1));
                }
            }
            61 => stacks.push(output_stack(
                item,
                harvest::carrot(seed, view.tick(), dimension, observed.pos),
            )),
            19 => {
                stacks.push(output_stack(item, 1));
                if harvest::leaf_sapling(seed, dimension, observed.pos) {
                    stacks.push(output_stack(57, 1));
                }
            }
            _ => stacks.push(output_stack(item, 1)),
        }
    }
    let patch = wear_selected_tool(&inventory, observed.block).map(|after| {
        InventoryPatch::try_new(actor, inventory, after).expect("wear keeps the actor key")
    });
    // An empty result never reserves a drop slot. For nonempty output the
    // checked batch and captured world cells revalidate together at commit.
    let drops = if stacks.is_empty() {
        None
    } else {
        let batch = DropBatch::try_new(
            DropSource::Mining {
                actor,
                target: anchor,
                tick: view.tick(),
            },
            basis.dimension,
            block_center(anchor),
            stacks,
            basis.drop_pickup_delay,
        )?;
        view.check_drop_batch(&batch)?;
        Some(batch)
    };
    let txn = BlockTxn {
        producer: MutationProducer::Actor(actor),
        tick: view.tick(),
        writes,
        inventory: patch,
        containers,
        drops,
        mining: None,
    };
    Ok(Some(ResolvedMining { txn }))
}

/// Resolves one companion placement proposal against current authority: the
/// proposed block must be on the companion placeable registry, the target
/// must be observed air, and the debit comes from the first matching stack
/// in the companion's inventory (`completeCompanionPlacement` and
/// `companionPlaceableBlock`, `packages/server/sim/entity/companion_placement.go`).
/// A stale, unobserved or already-consumed target refuses — never a silent
/// success — and competing claims resolve first-commit-wins through the
/// ordinary revision advance when committed.
pub fn resolve_companion_place(
    actor: CompanionId,
    target: BlockPos,
    block: u16,
    view: &AuthorityReadView<'_>,
) -> Result<ResolvedPlacement, RuleReject> {
    let key = ActorKey::Companion(actor);
    let basis = actor_basis(view, key)?;
    let item =
        companion_placeable_block(block).ok_or(RuleReject::Wire(RejectReason::InvalidBlock))?;
    let target_observed = require_air(view, basis.dimension, target, RuleReject::StaleObservation)?;
    let inventory = *view.inventory(key).ok_or(RuleReject::StaleObservation)?;
    let after = consume_first_item(&inventory, item).ok_or(RuleReject::StaleObservation)?;

    let txn = BlockTxn {
        producer: MutationProducer::Actor(key),
        tick: view.tick(),
        writes: vec![BlockWrite::try_new(target_observed, block)?],
        inventory: Some(InventoryPatch::try_new(key, inventory, after)?),
        containers: Vec::new(),
        drops: None,
        mining: None,
    };
    Ok(ResolvedPlacement { txn })
}

/// Resolves one companion mining proposal against current authority: the ray
/// from the companion's eye to the target center must hit exactly the
/// proposed cell (`advanceCompanionMining`,
/// `packages/server/sim/entity/mining.go`), the block must be on the
/// companion mineable registry (`companionMineableBlock`), and the output
/// batch credits into the companion's inventory instead of staging world
/// drops, refusing whole when no slot can take it.
pub fn resolve_companion_mine(
    actor: CompanionId,
    target: BlockPos,
    view: &AuthorityReadView<'_>,
) -> Result<ResolvedMining, RuleReject> {
    let key = ActorKey::Companion(actor);
    let basis = actor_basis(view, key)?;
    let center = block_center(target).get();
    let direction = [
        center[0] - basis.eye[0],
        center[1] - basis.eye[1],
        center[2] - basis.eye[2],
    ];
    let hit = cast_interaction_ray(
        view,
        basis.dimension,
        basis.eye,
        direction,
        basis.reach,
        RuleReject::StaleObservation,
    )?
    .ok_or(RuleReject::StaleObservation)?;
    if hit.observed.pos != target {
        // Blocked, out of reach or superseded: the proposal no longer
        // matches the authority's geometry.
        return Err(RuleReject::StaleObservation);
    }
    if !companion_mineable_block(hit.observed.block) {
        return Err(RuleReject::StaleObservation);
    }
    let observed = hit.observed;
    let inventory = *view.inventory(key).ok_or(RuleReject::StaleObservation)?;
    let held = inventory.slots[usize::from(inventory.selected.get())];
    let (required, harvestable) = mining_rule(observed.block, held.item);
    if required == 0 {
        return Err(RuleReject::Wire(RejectReason::ProtectedBlock));
    }
    let (writes, _) = mining_writes(view, basis.dimension, observed, true)?;
    let mut stacks = Vec::new();
    let mut containers = Vec::new();
    if harvestable {
        stacks.push(output_stack(
            block_drop(observed.block).ok_or(RuleReject::Wire(RejectReason::ProtectedBlock))?,
            1,
        ));
    }
    if observed.block == CHEST_BLOCK || observed.block == FURNACE_BLOCK {
        let record = capture_container(
            view,
            basis.dimension,
            observed.pos,
            observed.block,
            RuleReject::StaleObservation,
        )?;
        stacks.extend(container_stacks(&record));
        containers.push(CapturedContainer {
            key: chunk_key(basis.dimension, observed.pos),
            record,
        });
    }
    // Source AddStack settles the output first, then the selected slot wears
    // in that credited copy. An empty selected slot may thus receive a tool
    // from a container and wear it during this same accepted mine.
    let credited =
        credit_batch(&inventory, &stacks).ok_or(RuleReject::Wire(RejectReason::HotbarFull))?;
    let after = wear_selected_tool(&credited, observed.block).unwrap_or(credited);
    let patch = if after == inventory {
        None
    } else {
        Some(InventoryPatch::try_new(key, inventory, after)?)
    };
    let txn = BlockTxn {
        producer: MutationProducer::Actor(key),
        tick: view.tick(),
        writes,
        inventory: patch,
        containers,
        drops: None,
        mining: None,
    };
    Ok(ResolvedMining { txn })
}

/// The companion placeable registry (`companionPlaceableBlock`,
/// `packages/server/sim/entity/companion_placement.go`): crops, farmland and
/// fluids are explicitly refused; every other block must reverse-map
/// through `BlockDrop` to an item that `ItemPlacement` maps back to the same
/// block. Returns the item a placement consumes.
fn companion_placeable_block(block: u16) -> Option<u16> {
    if is_crop(block) || is_farmland(block) || is_fluid(block) {
        return None;
    }
    let item = block_drop(block)?;
    if item_placement(item) == Some(block) {
        Some(item)
    } else {
        None
    }
}

/// The companion mineable registry (`companionMineableBlock`,
/// `packages/server/sim/entity/mining.go`): crops, farmland, torches, wild
/// grass, snow layers and fluids are explicitly refused; every other block
/// needs a single registered drop.
fn companion_mineable_block(block: u16) -> bool {
    if is_crop(block)
        || is_farmland(block)
        || is_torch(block)
        || is_wild_grass(block)
        || is_snow_layer(block)
        || is_fluid(block)
    {
        return false;
    }
    block_drop(block).is_some()
}
