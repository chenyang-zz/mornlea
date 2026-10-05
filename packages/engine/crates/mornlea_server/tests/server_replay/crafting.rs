//! Replay cases for the workbench crafting provider.
//!
//! Every expected value mirrors a frozen Go oracle row, cited at each case:
//! the 25 fixed recipes from `packages/shared/core/recipe.go` (also sealed in
//! `testdata/runtime-migration/server/capability-inventory.json` under
//! `recipes`, source `recipe.go#recipePattern`), the grid move and take rows
//! from `packages/server/sim/entity/crafting.go` and the command rows in
//! `packages/server/sim/entity/tick.go`, the repack invariant
//! `canRepackCrafting` with the runtime rehearsal rows in
//! `packages/server/sim/runtime/crafting_test.go`, the bench open arm in
//! `packages/server/sim/entity/container.go` (`openContainer`), and the
//! unified crafting view bounds (grid `0..8`, backpack `9..44`). No case
//! chooses a value the oracle does not pin.

use std::f32::consts::FRAC_PI_2;

use super::*;
use mornlea_domain::{
    BlockPos, ChunkPos, Command, CommandEnvelope, CommandEnvelopeParts, ContainerKind,
    ContainerRef, CraftingMove, CraftingSize, Dimension, FiniteVec3, LookAngles, MotionState,
    MotionStateParts, PlayerId, SurvivalState, SurvivalStateParts, chunk_block_index,
};
use mornlea_protocol::{LoginStart, admit_login};
use mornlea_server::contracts::{
    ActorAux, ActorBody, ActorLifecycle, ActorRecord, ActorRuntime, BlockObservation, ChunkKey,
    EnvironmentState, InventoryRecord, RuleEffect, RulePhase, RuleTunables, SessionKey,
    TransportKind, ViewLease,
};
use mornlea_server::core::world::ReadyChunk;
use mornlea_server::rules::containers;
use mornlea_server::rules::crafting as provider;
use mornlea_storage::{
    ChestSlot, Chunk, ContainerSnapshot, ItemStack, PlayerLocation, PlayerSave, StorageKind,
    decode_player, encode_player,
};

// Stable block numbers, mirrored from the frozen const block in
// `packages/shared/core/block.go`.
const AIR: u16 = 0; // `core.AirID`
const GRASS_BLOCK: u16 = 2; // `core.GrassID`
const CHEST_BLOCK: u16 = 11; // `core.ChestID`
const WORKBENCH_BLOCK: u16 = 45; // `core.WorkbenchID`

// Stable item numbers, mirrored from the frozen const block in
// `packages/shared/core/item.go`.
const ITEM_STONE: u16 = 1; // `core.ItemStone`
const ITEM_DIRT: u16 = 2; // `core.ItemDirt`
const ITEM_STONE_BRICK: u16 = 4; // `core.ItemStoneBrick`
const ITEM_COAL: u16 = 5; // `core.ItemCoal`
const ITEM_IRON_INGOT: u16 = 7; // `core.ItemIronIngot`
const ITEM_FURNACE: u16 = 8; // `core.ItemFurnace`
const ITEM_IRON_BLOCK: u16 = 9; // `core.ItemIronBlock`
const ITEM_STONE_PICKAXE: u16 = 10; // `core.ItemStonePickaxe`
const ITEM_IRON_PICKAXE: u16 = 11; // `core.ItemIronPickaxe`
const ITEM_CHEST: u16 = 14; // `core.ItemChest`
const ITEM_LIGHT_BLOCK: u16 = 15; // `core.ItemLightBlock`
const ITEM_GRAVEL: u16 = 19; // `core.ItemGravel`
const ITEM_OAK_LOG: u16 = 20; // `core.ItemOakLog`
const ITEM_OAK_PLANKS: u16 = 21; // `core.ItemOakPlanks`
const ITEM_GLASS: u16 = 23; // `core.ItemGlass`
const ITEM_STONE_HOE: u16 = 30; // `core.ItemStoneHoe`
const ITEM_IRON_HOE: u16 = 31; // `core.ItemIronHoe`
const ITEM_WHEAT: u16 = 35; // `core.ItemWheat`
const ITEM_BREAD: u16 = 36; // `core.ItemBread`
const ITEM_STICK: u16 = 37; // `core.ItemStick`
const ITEM_WORKBENCH: u16 = 38; // `core.ItemWorkbench`
const ITEM_DOOR: u16 = 43; // `core.ItemDoor`
const ITEM_TORCH: u16 = 44; // `core.ItemTorch`
const ITEM_BED: u16 = 46; // `core.ItemBed`
const ITEM_WOODEN_SWORD: u16 = 47; // `core.ItemWoodenSword`
const ITEM_STONE_SWORD: u16 = 48; // `core.ItemStoneSword`
const ITEM_IRON_SWORD: u16 = 49; // `core.ItemIronSword`
const ITEM_EMPTY_BUCKET: u16 = 55; // `core.ItemEmptyBucket`
const ITEM_IRON_HELMET: u16 = 58; // `core.ItemIronHelmet`
const ITEM_IRON_CHESTPLATE: u16 = 59; // `core.ItemIronChestplate`
const ITEM_IRON_LEGGINGS: u16 = 60; // `core.ItemIronLeggings`
const ITEM_IRON_BOOTS: u16 = 61; // `core.ItemIronBoots`
const ITEM_ARROW: u16 = 63; // `core.ItemArrow`

/// One frozen fixed-shape recipe row. The cells use the Go stride-3 layout:
/// the trimmed shape occupies the top-left `width x height` block and the
/// rest stays zero. Every value is pinned twice, by `recipePattern` in
/// `packages/shared/core/recipe.go` and by the sealed
/// `testdata/runtime-migration/server/capability-inventory.json` `recipes`
/// rows (source sha256 `d3774d08ab65f15d5992fa8464d950b62220486a7a7c12722a443886799fdbe1`);
/// the `name` field is the Go const name.
struct FrozenRecipe {
    name: &'static str,
    width: u8,
    height: u8,
    cells: [u16; 9],
    output_item: u16,
    output_count: u8,
    output_durability: u16,
    mirror: bool,
}

const FROZEN_RECIPES: [FrozenRecipe; 25] = [
    FrozenRecipe {
        name: "RecipeStoneBricks",
        width: 2,
        height: 2,
        cells: [
            ITEM_STONE, ITEM_STONE, 0, ITEM_STONE, ITEM_STONE, 0, 0, 0, 0,
        ],
        output_item: ITEM_STONE_BRICK,
        output_count: 4,
        output_durability: 0,
        mirror: true,
    },
    FrozenRecipe {
        name: "RecipeFurnace",
        width: 3,
        height: 3,
        cells: [
            ITEM_STONE, ITEM_STONE, ITEM_STONE, ITEM_STONE, 0, ITEM_STONE, ITEM_STONE, ITEM_STONE,
            ITEM_STONE,
        ],
        output_item: ITEM_FURNACE,
        output_count: 1,
        output_durability: 0,
        mirror: true,
    },
    FrozenRecipe {
        name: "RecipeIronBlock",
        width: 3,
        height: 3,
        cells: [ITEM_IRON_INGOT; 9],
        output_item: ITEM_IRON_BLOCK,
        output_count: 1,
        output_durability: 0,
        mirror: true,
    },
    FrozenRecipe {
        name: "RecipeStonePickaxe",
        width: 3,
        height: 3,
        cells: [
            ITEM_STONE, ITEM_STONE, ITEM_STONE, 0, ITEM_STICK, 0, 0, ITEM_STICK, 0,
        ],
        output_item: ITEM_STONE_PICKAXE,
        output_count: 1,
        output_durability: 131,
        mirror: false,
    },
    FrozenRecipe {
        name: "RecipeIronPickaxe",
        width: 3,
        height: 3,
        cells: [
            ITEM_IRON_INGOT,
            ITEM_IRON_INGOT,
            ITEM_IRON_INGOT,
            0,
            ITEM_STICK,
            0,
            0,
            ITEM_STICK,
            0,
        ],
        output_item: ITEM_IRON_PICKAXE,
        output_count: 1,
        output_durability: 250,
        mirror: false,
    },
    FrozenRecipe {
        name: "RecipeChest",
        width: 3,
        height: 3,
        cells: [
            ITEM_OAK_PLANKS,
            ITEM_OAK_PLANKS,
            ITEM_OAK_PLANKS,
            ITEM_OAK_PLANKS,
            0,
            ITEM_OAK_PLANKS,
            ITEM_OAK_PLANKS,
            ITEM_OAK_PLANKS,
            ITEM_OAK_PLANKS,
        ],
        output_item: ITEM_CHEST,
        output_count: 1,
        output_durability: 0,
        mirror: true,
    },
    FrozenRecipe {
        name: "RecipeOakPlanks",
        width: 1,
        height: 1,
        cells: [ITEM_OAK_LOG, 0, 0, 0, 0, 0, 0, 0, 0],
        output_item: ITEM_OAK_PLANKS,
        output_count: 4,
        output_durability: 0,
        mirror: true,
    },
    FrozenRecipe {
        name: "RecipeLightBlock",
        width: 2,
        height: 2,
        cells: [
            ITEM_GLASS, ITEM_GLASS, 0, ITEM_GLASS, ITEM_GLASS, 0, 0, 0, 0,
        ],
        output_item: ITEM_LIGHT_BLOCK,
        output_count: 4,
        output_durability: 0,
        mirror: true,
    },
    FrozenRecipe {
        name: "RecipeStoneHoe",
        width: 2,
        height: 2,
        cells: [
            ITEM_STONE, ITEM_STICK, 0, ITEM_STONE, ITEM_STICK, 0, 0, 0, 0,
        ],
        output_item: ITEM_STONE_HOE,
        output_count: 1,
        output_durability: 131,
        mirror: false,
    },
    FrozenRecipe {
        name: "RecipeIronHoe",
        width: 2,
        height: 2,
        cells: [
            ITEM_IRON_INGOT,
            ITEM_STICK,
            0,
            ITEM_IRON_INGOT,
            ITEM_STICK,
            0,
            0,
            0,
            0,
        ],
        output_item: ITEM_IRON_HOE,
        output_count: 1,
        output_durability: 250,
        mirror: false,
    },
    FrozenRecipe {
        name: "RecipeBread",
        width: 3,
        height: 1,
        cells: [ITEM_WHEAT, ITEM_WHEAT, ITEM_WHEAT, 0, 0, 0, 0, 0, 0],
        output_item: ITEM_BREAD,
        output_count: 1,
        output_durability: 0,
        mirror: true,
    },
    FrozenRecipe {
        name: "RecipeStick",
        width: 1,
        height: 2,
        cells: [ITEM_OAK_PLANKS, 0, 0, ITEM_OAK_PLANKS, 0, 0, 0, 0, 0],
        output_item: ITEM_STICK,
        output_count: 4,
        output_durability: 0,
        mirror: true,
    },
    FrozenRecipe {
        name: "RecipeWorkbench",
        width: 2,
        height: 2,
        cells: [
            ITEM_OAK_PLANKS,
            ITEM_OAK_PLANKS,
            0,
            ITEM_OAK_PLANKS,
            ITEM_OAK_PLANKS,
            0,
            0,
            0,
            0,
        ],
        output_item: ITEM_WORKBENCH,
        output_count: 1,
        output_durability: 0,
        mirror: true,
    },
    FrozenRecipe {
        name: "RecipeDoor",
        width: 2,
        height: 3,
        cells: [
            ITEM_OAK_PLANKS,
            ITEM_OAK_PLANKS,
            0,
            ITEM_OAK_PLANKS,
            ITEM_OAK_PLANKS,
            0,
            ITEM_OAK_PLANKS,
            ITEM_OAK_PLANKS,
            0,
        ],
        output_item: ITEM_DOOR,
        output_count: 3,
        output_durability: 0,
        mirror: true,
    },
    FrozenRecipe {
        name: "RecipeTorch",
        width: 1,
        height: 2,
        cells: [ITEM_COAL, 0, 0, ITEM_STICK, 0, 0, 0, 0, 0],
        output_item: ITEM_TORCH,
        output_count: 4,
        output_durability: 0,
        mirror: true,
    },
    FrozenRecipe {
        name: "RecipeBed",
        width: 3,
        height: 3,
        cells: [
            ITEM_WHEAT,
            ITEM_WHEAT,
            ITEM_WHEAT,
            0,
            0,
            0,
            ITEM_OAK_PLANKS,
            ITEM_OAK_PLANKS,
            ITEM_OAK_PLANKS,
        ],
        output_item: ITEM_BED,
        output_count: 1,
        output_durability: 0,
        mirror: true,
    },
    FrozenRecipe {
        name: "RecipeWoodenSword",
        width: 1,
        height: 3,
        cells: [
            ITEM_OAK_PLANKS,
            0,
            0,
            ITEM_OAK_PLANKS,
            0,
            0,
            ITEM_STICK,
            0,
            0,
        ],
        output_item: ITEM_WOODEN_SWORD,
        output_count: 1,
        output_durability: 59,
        mirror: true,
    },
    FrozenRecipe {
        name: "RecipeStoneSword",
        width: 1,
        height: 3,
        cells: [ITEM_STONE, 0, 0, ITEM_STONE, 0, 0, ITEM_STICK, 0, 0],
        output_item: ITEM_STONE_SWORD,
        output_count: 1,
        output_durability: 131,
        mirror: true,
    },
    FrozenRecipe {
        name: "RecipeIronSword",
        width: 1,
        height: 3,
        cells: [
            ITEM_IRON_INGOT,
            0,
            0,
            ITEM_IRON_INGOT,
            0,
            0,
            ITEM_STICK,
            0,
            0,
        ],
        output_item: ITEM_IRON_SWORD,
        output_count: 1,
        output_durability: 250,
        mirror: true,
    },
    FrozenRecipe {
        name: "RecipeBucket",
        width: 3,
        height: 2,
        cells: [
            ITEM_IRON_INGOT,
            0,
            ITEM_IRON_INGOT,
            0,
            ITEM_IRON_INGOT,
            0,
            0,
            0,
            0,
        ],
        output_item: ITEM_EMPTY_BUCKET,
        output_count: 1,
        output_durability: 0,
        mirror: true,
    },
    FrozenRecipe {
        name: "RecipeIronHelmet",
        width: 3,
        height: 2,
        cells: [
            ITEM_IRON_INGOT,
            ITEM_IRON_INGOT,
            ITEM_IRON_INGOT,
            ITEM_IRON_INGOT,
            0,
            ITEM_IRON_INGOT,
            0,
            0,
            0,
        ],
        output_item: ITEM_IRON_HELMET,
        output_count: 1,
        output_durability: 165,
        mirror: true,
    },
    FrozenRecipe {
        name: "RecipeIronChestplate",
        width: 3,
        height: 3,
        cells: [
            ITEM_IRON_INGOT,
            0,
            ITEM_IRON_INGOT,
            ITEM_IRON_INGOT,
            ITEM_IRON_INGOT,
            ITEM_IRON_INGOT,
            ITEM_IRON_INGOT,
            ITEM_IRON_INGOT,
            ITEM_IRON_INGOT,
        ],
        output_item: ITEM_IRON_CHESTPLATE,
        output_count: 1,
        output_durability: 240,
        mirror: true,
    },
    FrozenRecipe {
        name: "RecipeIronLeggings",
        width: 3,
        height: 3,
        cells: [
            ITEM_IRON_INGOT,
            ITEM_IRON_INGOT,
            ITEM_IRON_INGOT,
            ITEM_IRON_INGOT,
            0,
            ITEM_IRON_INGOT,
            ITEM_IRON_INGOT,
            0,
            ITEM_IRON_INGOT,
        ],
        output_item: ITEM_IRON_LEGGINGS,
        output_count: 1,
        output_durability: 225,
        mirror: true,
    },
    FrozenRecipe {
        name: "RecipeIronBoots",
        width: 3,
        height: 2,
        cells: [
            ITEM_IRON_INGOT,
            0,
            ITEM_IRON_INGOT,
            ITEM_IRON_INGOT,
            0,
            ITEM_IRON_INGOT,
            0,
            0,
            0,
        ],
        output_item: ITEM_IRON_BOOTS,
        output_count: 1,
        output_durability: 195,
        mirror: true,
    },
    FrozenRecipe {
        name: "RecipeArrow",
        width: 1,
        height: 2,
        cells: [ITEM_GRAVEL, 0, 0, ITEM_STICK, 0, 0, 0, 0, 0],
        output_item: ITEM_ARROW,
        output_count: 2,
        output_durability: 0,
        mirror: true,
    },
];

fn player_session(tag: u8, name: &str) -> SessionKey {
    let mut bytes = [0u8; 16];
    bytes[0] = tag.max(1);
    bytes[6] = 0x40;
    bytes[8] = 0x80;
    let id = PlayerId::try_from_bytes(bytes).expect("player id");
    let start = LoginStart::new(id, name, 8).expect("login start");
    let inbound = LoginStart::decode_inbound(&start.encode().expect("encoded")).expect("inbound");
    let login = admit_login(inbound).expect("admitted");
    let mut mint = AuthorityState::try_new(limits(), 0).expect("authority");
    mint.admit(login, TransportKind::Memory).expect("session")
}

fn stack(item: u16, count: u8) -> ItemStack {
    ItemStack {
        item,
        count,
        durability: 0,
    }
}

fn durable(item: u16, count: u8, durability: u16) -> ItemStack {
    ItemStack {
        item,
        count,
        durability,
    }
}

fn uuid(tag: u8) -> [u8; 16] {
    let mut bytes = [0u8; 16];
    bytes[0] = tag.max(1);
    bytes[6] = 0x40;
    bytes[8] = 0x80;
    bytes
}

fn environment() -> EnvironmentState {
    EnvironmentState {
        seed: 7,
        next_tick: 0,
        world_time: 0,
        day_phase_offset: 0,
        season_offset: 0,
        weather: mornlea_domain::Weather::Clear,
        weather_remaining: 0,
        difficulty: 0,
        tunables: RuleTunables::source_defaults(),
    }
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

fn player_actor(
    session: SessionKey,
    position: [f32; 3],
    yaw: f32,
    pitch: f32,
    lifecycle: ActorLifecycle,
) -> ActorRecord {
    ActorRecord::try_new(
        ActorKey::Player(session),
        lifecycle,
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

fn overworld_key(pos: BlockPos) -> ChunkKey {
    ChunkKey {
        dimension: Dimension::OVERWORLD,
        pos: ChunkPos::new(pos.x() >> 4, pos.z() >> 4),
    }
}

fn observation(pos: BlockPos, block: u16) -> BlockObservation {
    BlockObservation::try_new(overworld_key(pos), 1, 1, pos, block).expect("block observation")
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

fn admit(
    context: &mut TickContext<'_>,
    envelope: &CommandEnvelope,
) -> Result<PhaseReport, ServerError> {
    provider::run(
        context,
        RuleCall {
            phase: RulePhase::PlayerCommand,
            actor: None,
            command: Some(envelope),
            internal: None,
        },
    )
}

fn drain(context: &mut TickContext<'_>) -> Result<PhaseReport, ServerError> {
    provider::run(
        context,
        RuleCall {
            phase: RulePhase::WorkbenchLifecycle,
            actor: None,
            command: None,
            internal: None,
        },
    )
}

/// Stages the environment, the actor and the given inventory, and returns the
/// actor key.
fn scene(
    context: &mut TickContext<'_>,
    session: SessionKey,
    inventory: InventoryRecord,
    lifecycle: ActorLifecycle,
) -> ActorKey {
    let actor = ActorKey::Player(session);
    context
        .stage(RuleEffect::Environment(environment()))
        .expect("environment");
    context
        .stage(RuleEffect::Actor(player_actor(
            session,
            [0.5, 64.0, 0.5],
            0.0,
            0.0,
            lifecycle,
        )))
        .expect("actor");
    context.preload_inventory(actor, inventory);
    actor
}

fn inventory_of(context: &TickContext<'_>, actor: ActorKey) -> InventoryRecord {
    context
        .read()
        .inventory(actor)
        .copied()
        .expect("inventory staged")
}

/// The trimmed `width x height` shape placed at the top-left of a `size`
/// sized grid (stride `size`: personal 2 or bench 3), `count` items per
/// covered cell. This is the layout `MatchCraftingGrid` reads for a placement
/// that already sits in the grid corner.
fn placed_grid(width: u8, height: u8, cells: &[u16; 9], size: u8, count: u8) -> [ItemStack; 9] {
    let mut grid = [ItemStack::default(); 9];
    for y in 0..height {
        for x in 0..width {
            let item = cells[usize::from(y) * 3 + usize::from(x)];
            if item != 0 {
                grid[usize::from(y) * usize::from(size) + usize::from(x)] = stack(item, count);
            }
        }
    }
    grid
}

/// The horizontal mirror of one trimmed shape, kept in the stride-3 layout
/// (`matchesPattern` mirror column reversal).
fn mirrored(recipe: &FrozenRecipe) -> ([u16; 9], u8, u8) {
    let mut cells = [0u16; 9];
    for y in 0..recipe.height {
        for x in 0..recipe.width {
            cells[usize::from(y) * 3 + usize::from(x)] =
                recipe.cells[usize::from(y) * 3 + usize::from(recipe.width - 1 - x)];
        }
    }
    (cells, recipe.width, recipe.height)
}

/// The 90-degree clockwise rotation of one trimmed shape: the rotated shape
/// has swapped dimensions and `rotated[y][x] = original[height - 1 - x][y]`.
fn rotated(recipe: &FrozenRecipe) -> ([u16; 9], u8, u8) {
    let mut cells = [0u16; 9];
    for y in 0..recipe.width {
        for x in 0..recipe.height {
            cells[usize::from(y) * 3 + usize::from(x)] =
                recipe.cells[usize::from(recipe.height - 1 - x) * 3 + usize::from(y)];
        }
    }
    (cells, recipe.height, recipe.width)
}

/// A personal (2x2) grid holds a `width x height` shape exactly when both
/// dimensions fit the four cells (`CraftingGridSizePersonal`, crafting.go);
/// every wider shape needs the opened bench.
fn fits_personal(width: u8, height: u8) -> bool {
    width <= 2 && height <= 2
}

fn grid_size_for(width: u8, height: u8) -> (u8, CraftingSize) {
    if fits_personal(width, height) {
        (2, CraftingSize::Personal)
    } else {
        (3, CraftingSize::Workbench)
    }
}

/// Runs one `TakeCraftingOutput` settlement on a fresh scene holding exactly
/// the given pack and grid, and hands the provider result and the settled
/// inventory record to the checker.
fn with_take(
    slots: &[(usize, ItemStack)],
    crafting: [ItemStack; 9],
    size: CraftingSize,
    check: impl FnOnce(Result<PhaseReport, ServerError>, &InventoryRecord),
) {
    let session = player_session(21, "take");
    let mut state = AuthorityState::try_new(limits(), 7).expect("authority");
    let mut context = TickContext::harness(&mut state, TickBudget::full());
    let mut inventory = InventoryRecord::empty();
    inventory.crafting_size = size;
    inventory.crafting = crafting;
    for (index, held) in slots {
        inventory.slots[*index] = *held;
    }
    let actor = scene(&mut context, session, inventory, ActorLifecycle::Active);
    let result = admit(
        &mut context,
        &envelope(session, 1, Command::TakeCraftingOutput),
    );
    check(result, &inventory_of(&context, actor));
}

#[test]
fn all_25_recipes_and_mirror() {
    // Each frozen recipe produces its exact output, count and durability
    // through a real take settlement (`applyTakeCraftingOutput` in
    // `packages/server/sim/entity/crafting.go` over `MatchCraftingGrid`):
    // the consumed grid empties, the credited pack stack equals the sealed
    // recipe output triple, and a repeat take refuses because the current
    // grid no longer matches (`CommandTakeCraftingOutput` row in tick.go).
    for recipe in &FROZEN_RECIPES {
        let (stride, size) = grid_size_for(recipe.width, recipe.height);
        let grid = placed_grid(recipe.width, recipe.height, &recipe.cells, stride, 1);
        let expected = durable(
            recipe.output_item,
            recipe.output_count,
            recipe.output_durability,
        );
        with_take(&[], grid, size, |result, after| {
            let report =
                result.unwrap_or_else(|error| panic!("{} take refused: {error:?}", recipe.name));
            assert_eq!(report.examined, 1);
            assert_eq!(report.applied, 1);
            assert_eq!(
                after.slots[0], expected,
                "{} exact output in the first empty pack slot",
                recipe.name
            );
            assert!(
                after.crafting.iter().all(|held| held.count == 0),
                "{} grid consumed once",
                recipe.name
            );
        });
        // The mirrored placement matches exactly when the recipe flag is set
        // (design.md D3 mirrored through `matchesPattern`): every
        // `mirror: true` shape in the frozen table is horizontally symmetric,
        // so the mirrored grid must take the identical output; a
        // `mirror: false` shape only matches when the mirrored grid equals
        // the original one (the stone and iron hoe rows).
        let (cells, width, height) = mirrored(recipe);
        let identical = cells == recipe.cells;
        let (stride, size) = grid_size_for(width, height);
        let mirrored_grid = placed_grid(width, height, &cells, stride, 1);
        let name = recipe.name;
        let mirror_ok = identical || recipe.mirror;
        with_take(&[], mirrored_grid, size, |result, after| {
            if mirror_ok {
                let _ = result
                    .unwrap_or_else(|error| panic!("{name} mirrored variant refused: {error:?}"));
                assert_eq!(
                    after.slots[0], expected,
                    "{name} mirrored variant exact output"
                );
            } else {
                assert!(result.is_err(), "{name} mirrored variant must not match");
                assert_eq!(
                    after.crafting, mirrored_grid,
                    "refusal leaves the grid unchanged"
                );
            }
        });

        // Rotation is never in the matching semantics (`MatchCraftingGrid`
        // doc: vertical flips and rotations never match): a rotated placement
        // only takes when the shape is rotationally symmetric, i.e. the
        // rotated trimmed grid equals the original one.
        let (cells, width, height) = rotated(recipe);
        let symmetric = width == recipe.width && height == recipe.height && cells == recipe.cells;
        let (stride, size) = grid_size_for(width, height);
        let grid = placed_grid(width, height, &cells, stride, 1);
        let name = recipe.name;
        with_take(&[], grid, size, |result, _after| {
            if symmetric {
                let _ = result.unwrap_or_else(|error| {
                    panic!("{name} rotated symmetric shape refused: {error:?}")
                });
            } else {
                assert!(result.is_err(), "{name} rotated shape must not match");
            }
        });
    }

    // Repeat clicks re-settle only what the current grid state allows
    // (`TestCraftingTakeOutputConsumesMatchOnceIntoInventory`): a two-deep
    // stone bricks grid takes twice, each take consuming every covered cell
    // exactly once and crediting the exact output again (merged to eight in
    // the pack), and the third take refuses because the consumed grid no
    // longer matches.
    let session = player_session(22, "repeat");
    let mut state = AuthorityState::try_new(limits(), 7).expect("authority");
    let mut context = TickContext::harness(&mut state, TickBudget::full());
    let mut inventory = InventoryRecord::empty();
    inventory.crafting = placed_grid(2, 2, &FROZEN_RECIPES[0].cells, 2, 2);
    let actor = scene(&mut context, session, inventory, ActorLifecycle::Active);
    assert!(
        admit(
            &mut context,
            &envelope(session, 1, Command::TakeCraftingOutput)
        )
        .is_ok()
    );
    let once = inventory_of(&context, actor);
    assert_eq!(
        once.slots[0],
        stack(ITEM_STONE_BRICK, 4),
        "first take credits once"
    );
    assert_eq!(
        once.crafting,
        placed_grid(2, 2, &FROZEN_RECIPES[0].cells, 2, 1),
        "one layer consumed"
    );
    assert!(
        admit(
            &mut context,
            &envelope(session, 2, Command::TakeCraftingOutput)
        )
        .is_ok()
    );
    let twice = inventory_of(&context, actor);
    assert_eq!(
        twice.slots[0],
        stack(ITEM_STONE_BRICK, 8),
        "second take merges again"
    );
    assert!(
        twice.crafting.iter().all(|held| held.count == 0),
        "second layer consumed"
    );
    assert!(
        admit(
            &mut context,
            &envelope(session, 3, Command::TakeCraftingOutput)
        )
        .is_err()
    );
    assert_eq!(
        inventory_of(&context, actor),
        twice,
        "third take refuses unchanged"
    );

    // Placement inside the bench grid does not change the match: the same
    // stone bricks shape sitting in the bottom-right bench corner trims to
    // the same bounding box and takes the same output (`trimPattern`, the
    // "position does not affect the result" row of `MatchCraftingGrid`).
    let mut corner = [ItemStack::default(); 9];
    corner[4] = stack(ITEM_STONE, 1);
    corner[5] = stack(ITEM_STONE, 1);
    corner[7] = stack(ITEM_STONE, 1);
    corner[8] = stack(ITEM_STONE, 1);
    with_take(&[], corner, CraftingSize::Workbench, |result, after| {
        let _ = result.expect("corner placement trims to the same shape");
        assert_eq!(after.slots[0], stack(ITEM_STONE_BRICK, 4));
    });

    // An extra item enlarges the trimmed bounding box, so the widened shape
    // no longer equals the recipe and the take refuses (`MatchCraftingGrid`
    // doc: extra items widen the box and miss).
    let mut widened = [ItemStack::default(); 9];
    widened[0] = stack(ITEM_STONE, 1);
    widened[1] = stack(ITEM_STONE, 1);
    widened[2] = stack(ITEM_STONE, 1);
    widened[3] = stack(ITEM_STONE, 1);
    with_take(&[], widened, CraftingSize::Workbench, |result, after| {
        assert!(result.is_err(), "widened bounding box must not match");
        assert_eq!(after.crafting[0], stack(ITEM_STONE, 1));
    });

    // Nonzero-durability items are never ingredients: a grid whose covered
    // cell holds a tool takes nothing and the tool stays untouched
    // (`TestCraftingDurabilityItemsNeverConsume` in
    // `packages/server/sim/runtime/crafting_test.go`).
    let pickaxe = durable(ITEM_STONE_PICKAXE, 1, 131);
    let mut grid = placed_grid(2, 2, &FROZEN_RECIPES[0].cells, 2, 1);
    grid[2] = pickaxe;
    with_take(&[], grid, CraftingSize::Personal, |result, after| {
        assert!(result.is_err(), "durability item must not settle");
        assert_eq!(after.crafting[2], pickaxe, "tool untouched");
    });
}

#[test]
fn capacity_close_repack() {
    // Full inventory refuses consumption. The rehearsal construction is the
    // "one slot short" row of `TestCraftingTakeOutputCapacityRehearsal`
    // (`packages/server/sim/runtime/crafting_test.go`): one oak log moved to
    // the grid, every other slot full. Taking 4 planks would fill the only
    // empty slot, but then the consumed grid could no longer repack, so the
    // whole view refuses unchanged.
    let session = player_session(31, "capacity");
    let mut state = AuthorityState::try_new(limits(), 7).expect("authority");
    let mut context = TickContext::harness(&mut state, TickBudget::full());
    let mut inventory = InventoryRecord::empty();
    inventory.slots[0] = stack(ITEM_OAK_LOG, 2);
    for index in 1..36 {
        inventory.slots[index] = stack(ITEM_STONE, 64);
    }
    let actor = scene(&mut context, session, inventory, ActorLifecycle::Active);
    assert!(
        admit(
            &mut context,
            &envelope(
                session,
                1,
                Command::MoveCrafting(CraftingMove::try_new(9, 0).expect("log to grid"))
            )
        )
        .is_ok()
    );
    let after_move = inventory_of(&context, actor);
    assert_eq!(after_move.crafting[0], stack(ITEM_OAK_LOG, 2));
    assert_eq!(after_move.slots[0], ItemStack::default());
    assert!(
        admit(
            &mut context,
            &envelope(session, 2, Command::TakeCraftingOutput)
        )
        .is_err(),
        "one slot short of output plus consumed grid must refuse"
    );
    let refused = inventory_of(&context, actor);
    assert_eq!(
        refused, after_move,
        "refused take leaves the view unchanged"
    );

    // The exactly-fitting sibling row of the same Go test: one backpack slot
    // free, so the output lands in the vacated source slot and the consumed
    // log repacks into the free backpack slot.
    let session = player_session(32, "marginal");
    let mut state = AuthorityState::try_new(limits(), 7).expect("authority");
    let mut context = TickContext::harness(&mut state, TickBudget::full());
    let mut inventory = InventoryRecord::empty();
    inventory.slots[0] = stack(ITEM_OAK_LOG, 2);
    for index in 1..36 {
        if index == 35 {
            continue;
        }
        inventory.slots[index] = stack(ITEM_STONE, 64);
    }
    let actor = scene(&mut context, session, inventory, ActorLifecycle::Active);
    assert!(
        admit(
            &mut context,
            &envelope(
                session,
                1,
                Command::MoveCrafting(CraftingMove::try_new(9, 0).expect("log to grid"))
            )
        )
        .is_ok()
    );
    let report = admit(
        &mut context,
        &envelope(session, 2, Command::TakeCraftingOutput),
    )
    .expect("exactly fitting take settles");
    assert_eq!(report.examined, 1);
    assert_eq!(report.applied, 1);
    let after = inventory_of(&context, actor);
    assert_eq!(
        after.slots[0],
        stack(ITEM_OAK_PLANKS, 4),
        "output lands first"
    );
    assert_eq!(
        after.crafting[0],
        stack(ITEM_OAK_LOG, 1),
        "the covered cell decrements once and keeps the remainder"
    );
    assert!(
        after.crafting.iter().skip(1).all(|held| held.count == 0),
        "no other grid cell holds anything"
    );

    // Bench open through the authoritative ray widens the grid
    // (`openContainer` workbench arm in
    // `packages/server/sim/entity/container.go` and
    // `TestWorkbenchOpenSetsSizeThreeWithoutContainerRef`): the open never
    // names a container reference and the drain reports the settled open.
    let session = player_session(33, "bench");
    let mut state = AuthorityState::try_new(limits(), 7).expect("authority");
    let mut context = TickContext::harness(&mut state, TickBudget::full());
    let mut inventory = InventoryRecord::empty();
    inventory.slots[0] = stack(ITEM_OAK_PLANKS, 4);
    for index in 1..6 {
        inventory.slots[index] = stack(ITEM_DIRT, 2);
    }
    let actor = scene(&mut context, session, inventory, ActorLifecycle::Active);
    context.preload_ready_chunk(ready_chunk(
        overworld_key(BlockPos::new(0, 63, 0)),
        &[
            (BlockPos::new(0, 65, 0), AIR),
            (BlockPos::new(0, 64, 0), AIR),
            (BlockPos::new(0, 63, 0), WORKBENCH_BLOCK),
        ],
    ));
    let open_look = LookAngles::try_new(0.0, -FRAC_PI_2).expect("look down");
    assert!(
        admit(
            &mut context,
            &envelope(session, 1, Command::OpenContainer(open_look))
        )
        .is_ok()
    );
    let report = drain(&mut context).expect("drain settles the deferred open");
    assert_eq!(
        report.examined, 2,
        "one open plus one bench session examined"
    );
    assert_eq!(report.applied, 1);
    assert_eq!(report.carried, 0);
    assert_eq!(report.rejected, 0);
    assert_eq!(
        inventory_of(&context, actor).crafting_size,
        CraftingSize::Workbench,
        "the settled open widens the grid"
    );

    // Fill the extended cells through the bench-sized view
    // (`fillExtendedSlots` in `TestCraftingCloseCommandRepacksExtendedSlots`):
    // planks to cell 0 and five two-stacks to cells 4..8.
    assert!(
        admit(
            &mut context,
            &envelope(
                session,
                2,
                Command::MoveCrafting(CraftingMove::try_new(9, 0).expect("planks to cell 0"))
            )
        )
        .is_ok()
    );
    for offset in 0..5usize {
        assert!(
            admit(
                &mut context,
                &envelope(
                    session,
                    3 + offset as u64,
                    Command::MoveCrafting(
                        CraftingMove::try_new(10 + offset as u8, 4 + offset as u8)
                            .expect("pack to extended cell"),
                    ),
                ),
            )
            .is_ok()
        );
    }
    let filled = inventory_of(&context, actor);
    assert_eq!(filled.crafting[0], stack(ITEM_OAK_PLANKS, 4));
    for cell in 4..9 {
        assert_eq!(
            filled.crafting[cell],
            stack(ITEM_DIRT, 2),
            "cell {cell} filled"
        );
    }

    // The lifecycle close repacks the extended cells back into the pack and
    // returns the grid to the personal size, keeping the personal cells
    // (`closeWorkbench` repacking `personalGridExtent..CraftingGridSlots`
    // in crafting.go). The anchor revalidation rows stay with the
    // publication path; the lifecycle arm this provider owns is the Active
    // check of `advanceWorkbenchLifecycle`.
    context
        .stage(RuleEffect::Actor(player_actor(
            session,
            [0.5, 64.0, 0.5],
            0.0,
            0.0,
            ActorLifecycle::Dead,
        )))
        .expect("lapsed actor");
    let report = drain(&mut context).expect("drain closes the lapsed bench");
    assert_eq!(
        report.examined, 2,
        "the replayed open plus the lapsed bench session"
    );
    assert_eq!(report.applied, 1, "the close repacks");
    assert_eq!(report.carried, 0);
    // The replayed open re-settles against the current state: the actor
    // record has lapsed, so the open refuses like any non-active open while
    // the close still repacks.
    assert_eq!(
        report.rejected, 1,
        "the replayed open refuses for the lapsed actor"
    );
    let closed = inventory_of(&context, actor);
    assert_eq!(closed.crafting_size, CraftingSize::Personal);
    assert_eq!(closed.crafting[0], stack(ITEM_OAK_PLANKS, 4), "cell 0 kept");
    for cell in 4..9 {
        assert_eq!(
            closed.crafting[cell],
            ItemStack::default(),
            "cell {cell} reclaimed"
        );
    }
    let mut total = 0u32;
    for held in closed.slots {
        if held.item == ITEM_DIRT {
            total += u32::from(held.count);
        }
    }
    assert_eq!(total, 10, "all ten dirt returned to the pack");

    // After the close the view is invalid: the personal grid refuses every
    // extended cell as source or target
    // (`craftingMoveCommandReasons` size gate; Go row
    // `TestCraftingMoveRejectsPersonalExtendedSlots`).
    let before = inventory_of(&context, actor);
    assert!(
        admit(
            &mut context,
            &envelope(
                session,
                20,
                Command::MoveCrafting(CraftingMove::try_new(10, 5).expect("pack to slot 5")),
            ),
        )
        .is_err(),
        "extended cell target refuses after the close"
    );
    assert_eq!(inventory_of(&context, actor), before);

    // A close whose extended cells cannot repack refuses with the whole view
    // unchanged (`TestCraftingCloseCommandRejectedWhenRepackImpossible`):
    // the state is hook-constructed because no legal command path reaches
    // it, mirroring the Go `SetPlayerCraftingGridForTest` setup.
    let session = player_session(34, "stuck");
    let mut state = AuthorityState::try_new(limits(), 7).expect("authority");
    let mut context = TickContext::harness(&mut state, TickBudget::full());
    let mut inventory = InventoryRecord::empty();
    inventory.crafting_size = CraftingSize::Workbench;
    for index in 0..36 {
        inventory.slots[index] = stack(ITEM_STONE, 64);
    }
    for cell in 4..9 {
        inventory.crafting[cell] = stack(ITEM_OAK_LOG, 64);
    }
    let actor = scene(&mut context, session, inventory, ActorLifecycle::Dead);
    let stuck = inventory_of(&context, actor);
    let report = drain(&mut context).expect("drain reports the refused close");
    assert_eq!(report.examined, 1);
    assert_eq!(report.applied, 0);
    assert_eq!(report.carried, 0);
    assert_eq!(report.rejected, 1, "the un-repackable close refuses");
    assert_eq!(
        inventory_of(&context, actor),
        stuck,
        "refused close leaves the view unchanged"
    );
}

#[test]
fn view_grid0_inventory9_boundary() {
    // The unified view maps grid slots 0..8 one-to-one and pack slot k to
    // view slot 9+k (`craftingViewSlot` / `setCraftingViewSlot` in
    // crafting.go). The whole move into the empty grid cell is Go row
    // `TestCraftingMoveInventoryIntoEmptyGridSlot`, and the reverse move
    // pins the same mapping in the other direction.
    let session = player_session(41, "boundary");
    let mut state = AuthorityState::try_new(limits(), 7).expect("authority");
    let mut context = TickContext::harness(&mut state, TickBudget::full());
    let mut inventory = InventoryRecord::empty();
    inventory.slots[0] = stack(ITEM_STONE, 3);
    inventory.slots[1] = stack(ITEM_DIRT, 1);
    let actor = scene(&mut context, session, inventory, ActorLifecycle::Active);
    assert!(
        admit(
            &mut context,
            &envelope(
                session,
                1,
                Command::MoveCrafting(CraftingMove::try_new(9, 0).expect("pack 0 to grid 0"))
            )
        )
        .is_ok()
    );
    let moved = inventory_of(&context, actor);
    assert_eq!(
        moved.crafting[0],
        stack(ITEM_STONE, 3),
        "view slot 9 is pack slot 0"
    );
    assert_eq!(
        moved.crafting[1],
        ItemStack::default(),
        "grid slot 1 untouched"
    );
    assert_eq!(
        moved.slots[0],
        ItemStack::default(),
        "source pack slot cleared"
    );
    assert_eq!(moved.slots[1], stack(ITEM_DIRT, 1), "pack slot 1 untouched");
    assert!(
        admit(
            &mut context,
            &envelope(
                session,
                2,
                Command::MoveCrafting(CraftingMove::try_new(0, 9).expect("grid 0 to pack 0"))
            )
        )
        .is_ok()
    );
    let restored = inventory_of(&context, actor);
    assert_eq!(
        restored.slots[0],
        stack(ITEM_STONE, 3),
        "view slot 9 maps back exactly"
    );
    assert_eq!(restored.crafting[0], ItemStack::default());

    // The bench view reaches grid slot 8 and pack slot 44 exactly
    // (`craftingViewSlots = core.InventorySlots + core.CraftingGridSlots`).
    let session = player_session(42, "bench-edge");
    let mut state = AuthorityState::try_new(limits(), 7).expect("authority");
    let mut context = TickContext::harness(&mut state, TickBudget::full());
    let mut inventory = InventoryRecord::empty();
    inventory.crafting_size = CraftingSize::Workbench;
    inventory.slots[35] = stack(ITEM_OAK_PLANKS, 2);
    let actor = scene(&mut context, session, inventory, ActorLifecycle::Active);
    assert!(
        admit(
            &mut context,
            &envelope(
                session,
                1,
                Command::MoveCrafting(CraftingMove::try_new(44, 8).expect("pack 35 to grid 8"))
            )
        )
        .is_ok()
    );
    let moved = inventory_of(&context, actor);
    assert_eq!(
        moved.crafting[8],
        stack(ITEM_OAK_PLANKS, 2),
        "view slot 44 is pack slot 35"
    );
    assert_eq!(moved.slots[35], ItemStack::default());
    assert!(
        admit(
            &mut context,
            &envelope(
                session,
                2,
                Command::MoveCrafting(CraftingMove::try_new(8, 44).expect("grid 8 to pack 35"))
            )
        )
        .is_ok()
    );
    assert_eq!(
        inventory_of(&context, actor).slots[35],
        stack(ITEM_OAK_PLANKS, 2)
    );

    // The static view bounds stay with the domain value rules
    // (`CraftingMove::try_new`): an end at or above the 45-slot view, a move
    // with both ends in the pack region, and a same-slot move are refused
    // before any authority read (`TestCraftingMoveRejectsOutOfRangeSlots`,
    // `TestCraftingMoveRejectsBothEndsInInventory`, and the same-slot row of
    // `TestCraftingMoveRejectsEmptySourceAndSameSlot`).
    assert_eq!(
        CraftingMove::try_new(45, 0),
        Err(mornlea_domain::DomainError::InvalidSlot)
    );
    assert_eq!(
        CraftingMove::try_new(0, 45),
        Err(mornlea_domain::DomainError::InvalidSlot)
    );
    assert_eq!(
        CraftingMove::try_new(9, 10),
        Err(mornlea_domain::DomainError::CraftingMoveInsideInventory)
    );
    assert_eq!(
        CraftingMove::try_new(44, 9),
        Err(mornlea_domain::DomainError::CraftingMoveInsideInventory)
    );
    assert_eq!(
        CraftingMove::try_new(3, 3),
        Err(mornlea_domain::DomainError::SourceEqualsTarget)
    );

    // The personal grid refuses its extended cells as source and target
    // while the bench is closed (`craftingMoveCommandReasons` size gate, Go
    // row `TestCraftingMoveRejectsPersonalExtendedSlots`): ten moves refuse,
    // two directions for each of the five extended cells.
    let session = player_session(43, "personal");
    let mut state = AuthorityState::try_new(limits(), 7).expect("authority");
    let mut context = TickContext::harness(&mut state, TickBudget::full());
    let mut inventory = InventoryRecord::empty();
    inventory.slots[0] = stack(ITEM_STONE, 3);
    let actor = scene(&mut context, session, inventory, ActorLifecycle::Active);
    let before = inventory_of(&context, actor);
    for slot in 4u8..9 {
        assert!(
            admit(
                &mut context,
                &envelope(
                    session,
                    u64::from(slot),
                    Command::MoveCrafting(
                        CraftingMove::try_new(9, slot).expect("pack to extended cell"),
                    ),
                ),
            )
            .is_err(),
            "extended cell {slot} target refuses"
        );
        assert!(
            admit(
                &mut context,
                &envelope(
                    session,
                    u64::from(slot) + 10,
                    Command::MoveCrafting(
                        CraftingMove::try_new(slot, 0).expect("extended cell to grid"),
                    ),
                ),
            )
            .is_err(),
            "extended cell {slot} source refuses"
        );
    }
    assert_eq!(
        inventory_of(&context, actor),
        before,
        "refused moves leave the view unchanged"
    );

    // An empty view source refuses with zero change (the "empty source" row
    // of `TestCraftingMoveRejectsEmptySourceAndSameSlot`), and an unlike
    // target refuses without swapping (`TestCraftingMoveRejectsDifferentItemTarget`).
    let session = player_session(44, "refusals");
    let mut state = AuthorityState::try_new(limits(), 7).expect("authority");
    let mut context = TickContext::harness(&mut state, TickBudget::full());
    let mut inventory = InventoryRecord::empty();
    inventory.crafting[0] = stack(ITEM_STONE, 2);
    inventory.slots[1] = stack(ITEM_OAK_PLANKS, 4);
    let actor = scene(&mut context, session, inventory, ActorLifecycle::Active);
    assert!(
        admit(
            &mut context,
            &envelope(
                session,
                1,
                Command::MoveCrafting(CraftingMove::try_new(11, 0).expect("empty source"))
            )
        )
        .is_err()
    );
    assert_eq!(
        inventory_of(&context, actor).crafting[0],
        stack(ITEM_STONE, 2)
    );
    assert!(
        admit(
            &mut context,
            &envelope(
                session,
                2,
                Command::MoveCrafting(CraftingMove::try_new(10, 0).expect("unlike target"))
            )
        )
        .is_err()
    );
    let unchanged = inventory_of(&context, actor);
    assert_eq!(
        unchanged.crafting[0],
        stack(ITEM_STONE, 2),
        "no swap into the grid"
    );
    assert_eq!(
        unchanged.slots[1],
        stack(ITEM_OAK_PLANKS, 4),
        "no swap out of the pack"
    );

    // Same-item targets merge up to the stack cap with the remainder kept at
    // the source (`TestCraftingMoveMergesSameItemUpToStackLimit`), grid-to-
    // grid moves merge and migrate the same way
    // (`TestCraftingMoveBetweenGridSlots`), and an already-full same-item
    // target refuses (`core.Inventory.MoveStack` row).
    let session = player_session(45, "merge");
    let mut state = AuthorityState::try_new(limits(), 7).expect("authority");
    let mut context = TickContext::harness(&mut state, TickBudget::full());
    let mut inventory = InventoryRecord::empty();
    inventory.crafting[0] = stack(ITEM_OAK_PLANKS, 60);
    inventory.crafting[1] = stack(ITEM_OAK_PLANKS, 30);
    inventory.slots[1] = stack(ITEM_OAK_PLANKS, 10);
    let actor = scene(&mut context, session, inventory, ActorLifecycle::Active);
    assert!(
        admit(
            &mut context,
            &envelope(
                session,
                1,
                Command::MoveCrafting(CraftingMove::try_new(10, 0).expect("merge from pack"))
            )
        )
        .is_ok()
    );
    let merged = inventory_of(&context, actor);
    assert_eq!(
        merged.crafting[0],
        stack(ITEM_OAK_PLANKS, 64),
        "grid cell merges to the cap"
    );
    assert_eq!(
        merged.slots[1],
        stack(ITEM_OAK_PLANKS, 6),
        "remainder stays at the source"
    );
    assert!(
        admit(
            &mut context,
            &envelope(
                session,
                2,
                Command::MoveCrafting(CraftingMove::try_new(0, 1).expect("grid to grid merge"))
            )
        )
        .is_ok()
    );
    let regrouped = inventory_of(&context, actor);
    assert_eq!(regrouped.crafting[1], stack(ITEM_OAK_PLANKS, 64));
    assert_eq!(
        regrouped.crafting[0],
        stack(ITEM_OAK_PLANKS, 30),
        "remainder stays at the source"
    );
    assert!(
        admit(
            &mut context,
            &envelope(
                session,
                3,
                Command::MoveCrafting(CraftingMove::try_new(0, 1).expect("full target"))
            )
        )
        .is_err(),
        "a full same-item target refuses"
    );
    let full = inventory_of(&context, actor);
    assert_eq!(full.crafting[0], stack(ITEM_OAK_PLANKS, 30));
    assert_eq!(full.crafting[1], stack(ITEM_OAK_PLANKS, 64));

    // An empty grid derives no output, so the take refuses and publishes
    // nothing (`TestCraftingTakeOutputRejectedWithoutMatch`).
    let session = player_session(46, "nomatch");
    let mut state = AuthorityState::try_new(limits(), 7).expect("authority");
    let mut context = TickContext::harness(&mut state, TickBudget::full());
    let inventory = InventoryRecord::empty();
    let actor = scene(&mut context, session, inventory, ActorLifecycle::Active);
    assert!(
        admit(
            &mut context,
            &envelope(session, 1, Command::TakeCraftingOutput)
        )
        .is_err(),
        "empty grid takes nothing"
    );
    assert_eq!(inventory_of(&context, actor), InventoryRecord::empty());

    // Shapes the bench open ray does not own refuse without widening the
    // grid (`TestWorkbenchOpenRejectsNonWorkbenchTarget`): a grass-floor hit
    // is not a bench, so the deferred open settles as a refusal and the
    // grid stays personal.
    let session = player_session(47, "no-bench");
    let mut state = AuthorityState::try_new(limits(), 7).expect("authority");
    let mut context = TickContext::harness(&mut state, TickBudget::full());
    let inventory = InventoryRecord::empty();
    let actor = scene(&mut context, session, inventory, ActorLifecycle::Active);
    context.preload_block(observation(BlockPos::new(0, 65, 0), AIR));
    context.preload_block(observation(BlockPos::new(0, 64, 0), AIR));
    context.preload_block(observation(BlockPos::new(0, 63, 0), 2)); // `core.GrassID`
    let open_look = LookAngles::try_new(0.0, -FRAC_PI_2).expect("look down");
    assert!(
        admit(
            &mut context,
            &envelope(session, 1, Command::OpenContainer(open_look))
        )
        .is_ok()
    );
    let report = drain(&mut context).expect("drain reports the refused open");
    assert_eq!(report.examined, 1);
    assert_eq!(report.applied, 0);
    assert_eq!(report.carried, 0);
    assert_eq!(report.rejected, 1);
    assert_eq!(
        inventory_of(&context, actor).crafting_size,
        CraftingSize::Personal
    );
}

/// A bench open shares the interaction classifier: the downward ray passes
/// water and open doors to the bench below, while a closed door is the hit
/// and refuses with the grid still personal.
#[test]
fn bench_open_passes_transparent_cells() {
    const WATER_SOURCE: u16 = 27; // `core.WaterSourceID`
    const WATER_FLOWING: u16 = 34; // `core.WaterLevel7ID`
    const DOOR_LOWER_SOUTH_OPEN: u16 = 63; // `core.DoorLowerSouthOpen`
    const DOOR_LOWER_SOUTH_CLOSED: u16 = 62; // `core.DoorLowerSouthClosed`
    for corridor in [WATER_SOURCE, WATER_FLOWING, DOOR_LOWER_SOUTH_OPEN] {
        let session = player_session(40, "bench-ray");
        let mut state = AuthorityState::try_new(limits(), 7).expect("authority");
        let mut context = TickContext::harness(&mut state, TickBudget::full());
        let actor = scene(
            &mut context,
            session,
            InventoryRecord::empty(),
            ActorLifecycle::Active,
        );
        context.preload_ready_chunk(ready_chunk(
            overworld_key(BlockPos::new(0, 63, 0)),
            &[
                (BlockPos::new(0, 65, 0), AIR),
                (BlockPos::new(0, 64, 0), corridor),
                (BlockPos::new(0, 63, 0), WORKBENCH_BLOCK),
            ],
        ));
        let open_look = LookAngles::try_new(0.0, -FRAC_PI_2).expect("look down");
        assert!(
            admit(
                &mut context,
                &envelope(session, 1, Command::OpenContainer(open_look))
            )
            .is_ok()
        );
        let report = drain(&mut context).expect("drain settles the deferred open");
        assert_eq!(report.applied, 1, "transparent corridor {corridor}");
        assert_eq!(
            inventory_of(&context, actor).crafting_size,
            CraftingSize::Workbench,
            "the bench below corridor {corridor} widens the grid"
        );
    }

    let session = player_session(41, "bench-door");
    let mut state = AuthorityState::try_new(limits(), 7).expect("authority");
    let mut context = TickContext::harness(&mut state, TickBudget::full());
    let actor = scene(
        &mut context,
        session,
        InventoryRecord::empty(),
        ActorLifecycle::Active,
    );
    context.preload_block(observation(BlockPos::new(0, 65, 0), AIR));
    context.preload_block(observation(
        BlockPos::new(0, 64, 0),
        DOOR_LOWER_SOUTH_CLOSED,
    ));
    context.preload_block(observation(BlockPos::new(0, 63, 0), WORKBENCH_BLOCK));
    let open_look = LookAngles::try_new(0.0, -FRAC_PI_2).expect("look down");
    assert!(
        admit(
            &mut context,
            &envelope(session, 1, Command::OpenContainer(open_look))
        )
        .is_ok()
    );
    let report = drain(&mut context).expect("drain reports the refused open");
    assert_eq!(report.applied, 0);
    assert_eq!(report.rejected, 1);
    assert_eq!(
        inventory_of(&context, actor).crafting_size,
        CraftingSize::Personal,
        "a closed door above the bench is the hit, not the bench"
    );
}

/// Builds one Ready chunk base holding exactly the given cells, everything
/// else air. The lifecycle revalidates the anchor against Ready state only,
/// so sparse `preload_block` cells open a bench but never keep it open.
fn ready_chunk(key: ChunkKey, cells: &[(BlockPos, u16)]) -> ReadyChunk {
    let mut chunk = Chunk {
        sections: vec![
            ContainerSnapshot {
                kind: StorageKind::Single,
                bits: 0,
                single: 0,
                palette: vec![],
                packed: vec![]
            };
            24
        ],
        drops: vec![Default::default(); 32],
        furnaces: vec![Default::default(); 32],
        chests: vec![Default::default(); 16],
    };
    for (pos, block) in cells {
        let index = chunk_block_index(*pos) as usize;
        let section = &mut chunk.sections[index / 4096];
        if section.kind != StorageKind::Direct {
            *section = ContainerSnapshot {
                kind: StorageKind::Direct,
                bits: 15,
                single: 0,
                palette: vec![],
                packed: vec![0; 1024],
            };
        }
        let cell = index % 4096;
        section.packed[cell / 4] |= u64::from(*block) << ((cell % 4) * 15);
    }
    ReadyChunk::try_new(key, 1, 1, chunk).expect("ready chunk")
}

/// The downward bench-open intent the envelope carries: look only, never a
/// target block.
fn open_down(session: SessionKey, sequence: u64) -> CommandEnvelope {
    envelope(
        session,
        sequence,
        Command::OpenContainer(LookAngles::try_new(0.0, -FRAC_PI_2).expect("look down")),
    )
}

/// Mints two distinct session identities on one authority. Each
/// `player_session` call admits on its own throwaway authority, so two calls
/// would collide on the same process-local key instead.
fn session_pair(first: u8, second: u8) -> (SessionKey, SessionKey) {
    use mornlea_server::contracts::ServerLimits;
    let mint_limits = ServerLimits::try_new(8, 1, 1, 1, 1, 1).expect("mint limits");
    let mut mint = AuthorityState::try_new(mint_limits, 0).expect("authority");
    let mut keys = [None, None];
    for (index, tag) in [first, second].into_iter().enumerate() {
        let mut bytes = [0u8; 16];
        bytes[0] = tag.max(1);
        bytes[6] = 0x40;
        bytes[8] = 0x80;
        let id = PlayerId::try_from_bytes(bytes).expect("player id");
        let start = LoginStart::new(id, "Tester", 8).expect("login start");
        let inbound =
            LoginStart::decode_inbound(&start.encode().expect("encoded")).expect("inbound");
        let login = admit_login(inbound).expect("admitted");
        keys[index] = Some(mint.admit(login, TransportKind::Memory).expect("session"));
    }
    (keys[0].expect("first"), keys[1].expect("second"))
}

/// Reads the staged workbench anchor for the actor, if any.
fn anchor_of(context: &TickContext<'_>, actor: ActorKey) -> Option<BlockPos> {
    match context.read().runtime(actor).map(|runtime| &runtime.aux) {
        Some(ActorAux::Player { workbench, .. }) => *workbench,
        _ => None,
    }
}

/// Stages one viewer lease for the session, standing in for an established
/// container view the bench open must end.
fn stage_lease(context: &mut TickContext<'_>, session: SessionKey) {
    let reference = ContainerRef::try_new(ChunkPos::new(0, -1), ContainerKind::Chest, 0, 1)
        .expect("lease reference");
    context
        .stage(RuleEffect::Viewer {
            session,
            view: Some(ViewLease::new(session, reference)),
        })
        .expect("lease");
}

/// Moves one whole stack through the crafting view, the legal command path
/// for filling grid cells.
fn move_view(context: &mut TickContext<'_>, session: SessionKey, sequence: u64, from: u8, to: u8) {
    assert!(
        admit(
            context,
            &envelope(
                session,
                sequence,
                Command::MoveCrafting(CraftingMove::try_new(from, to).expect("view move")),
            )
        )
        .is_ok(),
        "view move {from} to {to} settles"
    );
}

/// Opens the bench below the scene player through a Ready chunk, fills the
/// personal cell with planks and the extended cells with dirt pairs, and
/// leaves the bench open. Mirrors the stocked setup of
/// `TestCraftingWalkAwayClosesWorkbenchAndRepacks` in
/// `packages/server/sim/runtime/crafting_test.go`.
fn stocked_bench(
    context: &mut TickContext<'_>,
    session: SessionKey,
    actor: ActorKey,
) -> InventoryRecord {
    context.preload_ready_chunk(ready_chunk(
        overworld_key(BlockPos::new(0, 63, 0)),
        &[
            (BlockPos::new(0, 65, 0), AIR),
            (BlockPos::new(0, 64, 0), AIR),
            (BlockPos::new(0, 63, 0), WORKBENCH_BLOCK),
        ],
    ));
    assert!(admit(context, &open_down(session, 1)).is_ok());
    let report = drain(context).expect("drain settles the deferred open");
    assert_eq!(report.applied, 1, "the bench open widens the grid");
    assert_eq!(
        inventory_of(context, actor).crafting_size,
        CraftingSize::Workbench,
    );
    move_view(context, session, 2, 9, 0);
    for offset in 0..5u8 {
        move_view(
            context,
            session,
            3 + u64::from(offset),
            10 + offset,
            4 + offset,
        );
    }
    inventory_of(context, actor)
}

#[test]
fn bench_open_stages_anchor_size_and_clears_lease() {
    // The settled open is one atomic staging: the grid widens, the anchor
    // names the hit block, and any container lease ends, mirroring the
    // `openContainer` workbench arm in
    // `packages/server/sim/entity/container.go`.
    let session = player_session(60, "anchor-open");
    let mut state = AuthorityState::try_new(limits(), 7).expect("authority");
    let mut context = TickContext::harness(&mut state, TickBudget::full());
    let mut inventory = InventoryRecord::empty();
    inventory.slots[0] = stack(ITEM_OAK_PLANKS, 4);
    let actor = scene(&mut context, session, inventory, ActorLifecycle::Active);
    context.preload_ready_chunk(ready_chunk(
        overworld_key(BlockPos::new(0, 63, 0)),
        &[
            (BlockPos::new(0, 65, 0), AIR),
            (BlockPos::new(0, 64, 0), AIR),
            (BlockPos::new(0, 63, 0), WORKBENCH_BLOCK),
        ],
    ));
    stage_lease(&mut context, session);
    assert!(admit(&mut context, &open_down(session, 1)).is_ok());
    let report = drain(&mut context).expect("drain settles the deferred open");
    assert_eq!(
        report.examined, 2,
        "one open plus one bench session examined"
    );
    assert_eq!(report.applied, 1);
    assert_eq!(report.carried, 0);
    assert_eq!(report.rejected, 0);
    assert_eq!(
        inventory_of(&context, actor).crafting_size,
        CraftingSize::Workbench,
        "the settled open widens the grid"
    );
    assert_eq!(
        anchor_of(&context, actor),
        Some(BlockPos::new(0, 63, 0)),
        "the anchor names the hit block"
    );
    assert_eq!(
        context.read().viewer(session),
        None,
        "the bench open ends the container lease"
    );
}

#[test]
fn bench_reopen_reanchors_with_grid_intact() {
    // Re-opening while bench-sized is never an idempotent skip: the anchor
    // moves to the new hit, the lease clears again, and the grid and size
    // stay untouched.
    let session = player_session(61, "re-anchor");
    let mut state = AuthorityState::try_new(limits(), 7).expect("authority");
    let mut context = TickContext::harness(&mut state, TickBudget::full());
    let mut inventory = InventoryRecord::empty();
    inventory.slots[0] = stack(ITEM_OAK_PLANKS, 4);
    inventory.slots[1] = stack(ITEM_DIRT, 2);
    let actor = scene(&mut context, session, inventory, ActorLifecycle::Active);
    context.preload_ready_chunk(ready_chunk(
        overworld_key(BlockPos::new(0, 63, 0)),
        &[
            (BlockPos::new(0, 65, 0), AIR),
            (BlockPos::new(0, 64, 0), AIR),
            (BlockPos::new(0, 63, 0), WORKBENCH_BLOCK),
            (BlockPos::new(10, 65, 0), AIR),
            (BlockPos::new(10, 64, 0), AIR),
            (BlockPos::new(10, 63, 0), WORKBENCH_BLOCK),
        ],
    ));
    assert!(admit(&mut context, &open_down(session, 1)).is_ok());
    drain(&mut context).expect("first open anchors the lower bench");
    assert_eq!(anchor_of(&context, actor), Some(BlockPos::new(0, 63, 0)),);
    move_view(&mut context, session, 2, 9, 0);
    move_view(&mut context, session, 3, 10, 4);
    stage_lease(&mut context, session);
    context
        .stage(RuleEffect::Actor(player_actor(
            session,
            [10.5, 64.0, 0.5],
            0.0,
            0.0,
            ActorLifecycle::Active,
        )))
        .expect("moved actor");
    assert!(admit(&mut context, &open_down(session, 4)).is_ok());
    let report = drain(&mut context).expect("drain re-settles both opens");
    assert_eq!(report.examined, 3, "two opens plus one bench session");
    assert_eq!(report.applied, 1, "only the first replayed open stages");
    assert_eq!(report.carried, 1, "the second open finds nothing to change");
    assert_eq!(report.rejected, 0);
    let after = inventory_of(&context, actor);
    assert_eq!(after.crafting_size, CraftingSize::Workbench);
    assert_eq!(
        after.crafting[0],
        stack(ITEM_OAK_PLANKS, 4),
        "re-anchor keeps the personal cell"
    );
    assert_eq!(
        after.crafting[4],
        stack(ITEM_DIRT, 2),
        "re-anchor keeps the extended cell"
    );
    assert_eq!(
        anchor_of(&context, actor),
        Some(BlockPos::new(10, 63, 0)),
        "re-open moves the anchor to the new hit"
    );
    assert_eq!(
        context.read().viewer(session),
        None,
        "re-open ends the container lease again"
    );
}

#[test]
fn bench_walk_away_closes_and_repacks() {
    // The walk-away row of `TestCraftingWalkAwayClosesWorkbenchAndRepacks`:
    // eyes beyond `interaction_reach` from the anchor center close the bench
    // and reclaim the extended cells, keeping the personal cell.
    let session = player_session(62, "walk-away");
    let mut state = AuthorityState::try_new(limits(), 7).expect("authority");
    let mut context = TickContext::harness(&mut state, TickBudget::full());
    let mut inventory = InventoryRecord::empty();
    inventory.slots[0] = stack(ITEM_OAK_PLANKS, 4);
    for index in 1..6 {
        inventory.slots[index] = stack(ITEM_DIRT, 2);
    }
    let actor = scene(&mut context, session, inventory, ActorLifecycle::Active);
    stocked_bench(&mut context, session, actor);
    context
        .stage(RuleEffect::Actor(player_actor(
            session,
            [10.5, 64.0, 10.5],
            0.0,
            0.0,
            ActorLifecycle::Active,
        )))
        .expect("walked actor");
    let report = drain(&mut context).expect("drain revalidates the anchor");
    assert_eq!(
        report.examined, 2,
        "the replayed open plus the bench session"
    );
    assert_eq!(report.applied, 1, "the walk-away closes the bench");
    assert_eq!(report.carried, 0);
    assert_eq!(report.rejected, 1, "the replayed open refuses from afar");
    let closed = inventory_of(&context, actor);
    assert_eq!(closed.crafting_size, CraftingSize::Personal);
    assert_eq!(
        closed.crafting[0],
        stack(ITEM_OAK_PLANKS, 4),
        "the personal cell stays"
    );
    for cell in 4..9 {
        assert_eq!(
            closed.crafting[cell],
            ItemStack::default(),
            "extended cell {cell} reclaims"
        );
    }
    let mut total = 0u32;
    for held in closed.slots {
        if held.item == ITEM_DIRT {
            total += u32::from(held.count);
        }
    }
    assert_eq!(total, 10, "all ten dirt return to the pack");
}

#[test]
fn bench_mined_same_tick_closes() {
    // The mined row of `TestCraftingWorkbenchMinedClosesSameTick`: the anchor
    // block turning to air in the same tick closes the bench on the next
    // lifecycle pass and reclaims the grid, mirroring the Go sequencing of
    // mining settlement before the lifecycle.
    let session = player_session(63, "mined");
    let mut state = AuthorityState::try_new(limits(), 7).expect("authority");
    let mut context = TickContext::harness(&mut state, TickBudget::full());
    let mut inventory = InventoryRecord::empty();
    inventory.slots[0] = stack(ITEM_OAK_PLANKS, 4);
    for index in 1..6 {
        inventory.slots[index] = stack(ITEM_DIRT, 2);
    }
    let actor = scene(&mut context, session, inventory, ActorLifecycle::Active);
    stocked_bench(&mut context, session, actor);
    context.preload_ready_chunk(ready_chunk(
        overworld_key(BlockPos::new(0, 63, 0)),
        &[
            (BlockPos::new(0, 65, 0), AIR),
            (BlockPos::new(0, 64, 0), AIR),
            (BlockPos::new(0, 63, 0), AIR),
        ],
    ));
    let report = drain(&mut context).expect("drain revalidates the anchor");
    assert_eq!(
        report.examined, 2,
        "the replayed open plus the bench session"
    );
    assert_eq!(report.applied, 1, "the mined bench closes");
    assert_eq!(report.carried, 0);
    assert_eq!(
        report.rejected, 1,
        "the replayed open misses the mined bench"
    );
    let closed = inventory_of(&context, actor);
    assert_eq!(closed.crafting_size, CraftingSize::Personal);
    assert_eq!(
        closed.crafting[0],
        stack(ITEM_OAK_PLANKS, 4),
        "the personal cell stays"
    );
    for cell in 4..9 {
        assert_eq!(
            closed.crafting[cell],
            ItemStack::default(),
            "extended cell {cell} reclaims same tick"
        );
    }
    let mut total = 0u32;
    for held in closed.slots {
        if held.item == ITEM_DIRT {
            total += u32::from(held.count);
        }
    }
    assert_eq!(total, 10, "all ten dirt return to the pack");
}

#[test]
fn bench_wrong_block_closes() {
    // A replaced anchor block that is no workbench closes the bench exactly
    // like a mined one: the block gate, not the replacement kind, decides.
    let session = player_session(64, "replaced");
    let mut state = AuthorityState::try_new(limits(), 7).expect("authority");
    let mut context = TickContext::harness(&mut state, TickBudget::full());
    let mut inventory = InventoryRecord::empty();
    inventory.slots[0] = stack(ITEM_OAK_PLANKS, 4);
    for index in 1..6 {
        inventory.slots[index] = stack(ITEM_DIRT, 2);
    }
    let actor = scene(&mut context, session, inventory, ActorLifecycle::Active);
    stocked_bench(&mut context, session, actor);
    context.preload_ready_chunk(ready_chunk(
        overworld_key(BlockPos::new(0, 63, 0)),
        &[
            (BlockPos::new(0, 65, 0), AIR),
            (BlockPos::new(0, 64, 0), AIR),
            (BlockPos::new(0, 63, 0), GRASS_BLOCK),
        ],
    ));
    let report = drain(&mut context).expect("drain revalidates the anchor");
    assert_eq!(report.applied, 1, "the replaced bench closes");
    assert_eq!(
        report.rejected, 1,
        "the replayed open refuses the new block"
    );
    let closed = inventory_of(&context, actor);
    assert_eq!(closed.crafting_size, CraftingSize::Personal);
    assert_eq!(closed.crafting[0], stack(ITEM_OAK_PLANKS, 4));
    for cell in 4..9 {
        assert_eq!(
            closed.crafting[cell],
            ItemStack::default(),
            "extended cell {cell} reclaims"
        );
    }
}

#[test]
fn bench_not_ready_chunk_closes() {
    // Sparse cells settle the open ray but never establish a Ready chunk, so
    // the lifecycle closes the bench in the same drain: a missing
    // observation closes rather than vetoing the check.
    let session = player_session(65, "sparse");
    let mut state = AuthorityState::try_new(limits(), 7).expect("authority");
    let mut context = TickContext::harness(&mut state, TickBudget::full());
    let actor = scene(
        &mut context,
        session,
        InventoryRecord::empty(),
        ActorLifecycle::Active,
    );
    context.preload_block(observation(BlockPos::new(0, 65, 0), AIR));
    context.preload_block(observation(BlockPos::new(0, 64, 0), AIR));
    context.preload_block(observation(BlockPos::new(0, 63, 0), WORKBENCH_BLOCK));
    assert!(admit(&mut context, &open_down(session, 1)).is_ok());
    let report = drain(&mut context).expect("drain settles the open and the bench");
    assert_eq!(
        report.examined, 2,
        "one open plus one bench session examined"
    );
    assert_eq!(
        report.applied, 2,
        "the open settles, then the anchor check closes"
    );
    assert_eq!(report.carried, 0);
    assert_eq!(report.rejected, 0);
    assert_eq!(
        inventory_of(&context, actor).crafting_size,
        CraftingSize::Personal,
        "a not-Ready anchor never holds the bench open"
    );
}

#[test]
fn bench_dimension_move_closes() {
    // The anchor carries no dimension, so the lookup runs in the actor's
    // current dimension: a moved actor closes unless the same coordinates
    // hold a workbench there too.
    let session = player_session(66, "dimension");
    let mut state = AuthorityState::try_new(limits(), 7).expect("authority");
    let mut context = TickContext::harness(&mut state, TickBudget::full());
    let mut inventory = InventoryRecord::empty();
    inventory.slots[0] = stack(ITEM_OAK_PLANKS, 4);
    for index in 1..6 {
        inventory.slots[index] = stack(ITEM_DIRT, 2);
    }
    let actor = scene(&mut context, session, inventory, ActorLifecycle::Active);
    stocked_bench(&mut context, session, actor);
    context
        .stage(RuleEffect::Actor(
            ActorRecord::try_new(
                ActorKey::Player(session),
                ActorLifecycle::Active,
                Dimension::DEPTHS,
                MotionState::new(MotionStateParts {
                    position: FiniteVec3::try_new([0.5, 64.0, 0.5]).expect("position"),
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
                ActorBody::Player(player_body([0.5, 64.0, 0.5])),
            )
            .expect("moved actor"),
        ))
        .expect("dimension move");
    let report = drain(&mut context).expect("drain revalidates the anchor");
    assert_eq!(report.applied, 1, "the dimension move closes the bench");
    assert_eq!(
        report.rejected, 1,
        "the replayed open refuses off-overworld"
    );
    let closed = inventory_of(&context, actor);
    assert_eq!(closed.crafting_size, CraftingSize::Personal);
    assert_eq!(closed.crafting[0], stack(ITEM_OAK_PLANKS, 4));
    for cell in 4..9 {
        assert_eq!(
            closed.crafting[cell],
            ItemStack::default(),
            "extended cell {cell} reclaims"
        );
    }
}

#[test]
fn bench_repack_impossible_keeps_anchor_and_sibling_closes() {
    // The no-loss ruling for a close that cannot repack: the stuck bench
    // keeps its grid, size and anchor with `rejected` counted and nothing
    // staged, while the repackable sibling in the same pass still closes.
    let (stuck_session, sibling_session) = session_pair(67, 68);
    let mut state = AuthorityState::try_new(limits(), 7).expect("authority");
    let mut context = TickContext::harness(&mut state, TickBudget::full());
    let mut stuck_inventory = InventoryRecord::empty();
    stuck_inventory.crafting_size = CraftingSize::Workbench;
    for index in 0..36 {
        stuck_inventory.slots[index] = stack(ITEM_STONE, 64);
    }
    for cell in 4..9 {
        stuck_inventory.crafting[cell] = stack(ITEM_OAK_LOG, 64);
    }
    let stuck = scene(
        &mut context,
        stuck_session,
        stuck_inventory,
        ActorLifecycle::Active,
    );
    context.preload_block(observation(BlockPos::new(0, 63, 0), WORKBENCH_BLOCK));
    context
        .stage(RuleEffect::Runtime(ActorRuntime {
            key: stuck,
            controls: None,
            has_view: false,
            reset: false,
            attack_cooldown: 0,
            hurt_cooldown: 0,
            burn_cooldown: 0,
            oxygen: 300,
            peak_y: 64.0,
            exhaustion_milli: 0,
            saturation_milli: 5_000,
            since_damage_ticks: 0,
            drown_ticks: 0,
            starvation_ticks: 0,
            eating: None,
            bow: None,
            path: None,
            aux: ActorAux::Player {
                respawn: None,
                workbench: Some(BlockPos::new(0, 63, 0)),
            },
        }))
        .expect("stuck runtime");
    let mut sibling_inventory = InventoryRecord::empty();
    sibling_inventory.crafting_size = CraftingSize::Workbench;
    sibling_inventory.crafting[4] = stack(ITEM_DIRT, 2);
    let sibling = scene(
        &mut context,
        sibling_session,
        sibling_inventory,
        ActorLifecycle::Dead,
    );
    let report = drain(&mut context).expect("drain closes what it can");
    assert_eq!(report.examined, 2, "both bench sessions examined");
    assert_eq!(report.applied, 1, "the repackable sibling closes");
    assert_eq!(report.carried, 0);
    assert_eq!(
        report.rejected, 1,
        "the stuck bench stays open without loss"
    );
    let kept = inventory_of(&context, stuck);
    assert_eq!(kept.crafting_size, CraftingSize::Workbench);
    for cell in 4..9 {
        assert_eq!(
            kept.crafting[cell],
            stack(ITEM_OAK_LOG, 64),
            "stuck cell {cell} keeps its stack"
        );
    }
    assert_eq!(
        anchor_of(&context, stuck),
        Some(BlockPos::new(0, 63, 0)),
        "the refused close keeps the anchor"
    );
    let closed = inventory_of(&context, sibling);
    assert_eq!(closed.crafting_size, CraftingSize::Personal);
    assert_eq!(
        closed.crafting[4],
        ItemStack::default(),
        "the sibling reclaims its extended cell"
    );
}

#[test]
fn stale_anchor_crafting_works_until_close() {
    // Moves and takes gate on the size-derived slot extent only: with the
    // anchor block already gone, settlements still succeed until the
    // lifecycle pass closes the bench.
    let session = player_session(69, "stale");
    let mut state = AuthorityState::try_new(limits(), 7).expect("authority");
    let mut context = TickContext::harness(&mut state, TickBudget::full());
    let mut inventory = InventoryRecord::empty();
    inventory.slots[0] = stack(ITEM_OAK_LOG, 1);
    inventory.slots[1] = stack(ITEM_DIRT, 2);
    inventory.slots[2] = stack(ITEM_DIRT, 2);
    let actor = scene(&mut context, session, inventory, ActorLifecycle::Active);
    context.preload_ready_chunk(ready_chunk(
        overworld_key(BlockPos::new(0, 63, 0)),
        &[
            (BlockPos::new(0, 65, 0), AIR),
            (BlockPos::new(0, 64, 0), AIR),
            (BlockPos::new(0, 63, 0), WORKBENCH_BLOCK),
        ],
    ));
    assert!(admit(&mut context, &open_down(session, 1)).is_ok());
    drain(&mut context).expect("open anchors the bench");
    move_view(&mut context, session, 2, 9, 0);
    context.preload_ready_chunk(ready_chunk(
        overworld_key(BlockPos::new(0, 63, 0)),
        &[
            (BlockPos::new(0, 65, 0), AIR),
            (BlockPos::new(0, 64, 0), AIR),
            (BlockPos::new(0, 63, 0), AIR),
        ],
    ));
    assert!(
        admit(
            &mut context,
            &envelope(session, 3, Command::TakeCraftingOutput)
        )
        .is_ok(),
        "a take settles with a stale anchor"
    );
    assert_eq!(
        inventory_of(&context, actor).slots[0],
        stack(ITEM_OAK_PLANKS, 4),
        "the stale take credits the log recipe output"
    );
    move_view(&mut context, session, 4, 10, 4);
    assert_eq!(
        inventory_of(&context, actor).crafting[4],
        stack(ITEM_DIRT, 2),
        "a grid move lands with a stale anchor"
    );
    let report = drain(&mut context).expect("drain closes the stale bench");
    assert_eq!(report.applied, 1, "the stale bench closes on revalidation");
    assert_eq!(
        inventory_of(&context, actor).crafting_size,
        CraftingSize::Personal,
    );
}

#[test]
fn chest_open_preserves_bench() {
    // Preservation by absence: the container open stages no crafting state,
    // so the bench size and anchor survive a chest open. The chest scene
    // mirrors the container replay `viewer_scene` geometry with a Ready
    // chunk carrying the fixed chest slot.
    let session = player_session(70, "preserve");
    let mut state = AuthorityState::try_new(limits(), 7).expect("authority");
    let mut context = TickContext::harness(&mut state, TickBudget::full());
    let mut inventory = InventoryRecord::empty();
    inventory.slots[0] = stack(ITEM_OAK_PLANKS, 4);
    let actor = scene(&mut context, session, inventory, ActorLifecycle::Active);
    context.preload_ready_chunk(ready_chunk(
        overworld_key(BlockPos::new(0, 63, 0)),
        &[
            (BlockPos::new(0, 65, 0), AIR),
            (BlockPos::new(0, 64, 0), AIR),
            (BlockPos::new(0, 63, 0), WORKBENCH_BLOCK),
        ],
    ));
    let chest_pos = BlockPos::new(0, 65, -1);
    let mut chest = Chunk {
        sections: vec![
            ContainerSnapshot {
                kind: StorageKind::Single,
                bits: 0,
                single: 0,
                palette: vec![],
                packed: vec![]
            };
            24
        ],
        drops: vec![Default::default(); 32],
        furnaces: vec![Default::default(); 32],
        chests: vec![Default::default(); 16],
    };
    let chest_index = chunk_block_index(chest_pos) as usize;
    chest.sections[chest_index / 4096] = ContainerSnapshot {
        kind: StorageKind::Direct,
        bits: 15,
        single: 0,
        palette: vec![],
        packed: vec![0; 1024],
    };
    chest.sections[chest_index / 4096].packed[(chest_index % 4096) / 4] |= u64::from(CHEST_BLOCK);
    chest.chests[0] = ChestSlot {
        active: true,
        generation: 1,
        block_index: chest_index as u32,
        items: [ItemStack::default(); 27],
    };
    context.preload_ready_chunk(
        ReadyChunk::try_new(
            ChunkKey {
                dimension: Dimension::OVERWORLD,
                pos: ChunkPos::new(0, -1),
            },
            1,
            1,
            chest,
        )
        .expect("ready chest chunk"),
    );
    assert!(admit(&mut context, &open_down(session, 1)).is_ok());
    drain(&mut context).expect("open anchors the bench");
    assert_eq!(anchor_of(&context, actor), Some(BlockPos::new(0, 63, 0)),);
    let chest_look = LookAngles::try_new(0.0, 0.0).expect("look");
    let chest_open = envelope(session, 2, Command::OpenContainer(chest_look));
    assert!(
        provider::run(
            &mut context,
            RuleCall {
                phase: RulePhase::PlayerCommand,
                actor: None,
                command: Some(&chest_open),
                internal: None,
            },
        )
        .is_ok(),
        "the crafting provider defers the chest open"
    );
    assert!(
        containers::run(
            &mut context,
            RuleCall {
                phase: RulePhase::PlayerCommand,
                actor: None,
                command: Some(&chest_open),
                internal: None,
            },
        )
        .is_ok(),
        "the container provider defers the chest open"
    );
    let crafting_report = drain(&mut context).expect("crafting refuses the chest open");
    assert_eq!(
        crafting_report.rejected, 1,
        "the chest hit is not the bench arm's"
    );
    let container_report = containers::run(
        &mut context,
        RuleCall {
            phase: RulePhase::ContainerMove,
            actor: None,
            command: None,
            internal: None,
        },
    )
    .expect("container settles the chest open");
    assert_eq!(
        container_report.applied, 1,
        "the chest open stages its lease"
    );
    let lease = ContainerRef::try_new(ChunkPos::new(0, -1), ContainerKind::Chest, 0, 1)
        .expect("chest reference");
    assert_eq!(
        context.read().viewer(session),
        Some(ViewLease::new(session, lease)),
        "the chest lease stages"
    );
    assert_eq!(
        inventory_of(&context, actor).crafting_size,
        CraftingSize::Workbench,
        "the chest open keeps the bench size"
    );
    assert_eq!(
        anchor_of(&context, actor),
        Some(BlockPos::new(0, 63, 0)),
        "the chest open keeps the anchor"
    );
}

#[test]
fn bench_anchor_is_save_blind() {
    // The anchor is a runtime overlay only: the player save codec never reads
    // `ActorRuntime`, so a set anchor cannot survive a save roundtrip while
    // the grid-carrying inventory passes through unchanged.
    let session = player_session(71, "save-blind");
    let mut state = AuthorityState::try_new(limits(), 7).expect("authority");
    let mut context = TickContext::harness(&mut state, TickBudget::full());
    let mut inventory = InventoryRecord::empty();
    inventory.slots[0] = stack(ITEM_OAK_PLANKS, 4);
    inventory.slots[1] = stack(ITEM_DIRT, 2);
    let actor = scene(&mut context, session, inventory, ActorLifecycle::Active);
    context.preload_ready_chunk(ready_chunk(
        overworld_key(BlockPos::new(0, 63, 0)),
        &[
            (BlockPos::new(0, 65, 0), AIR),
            (BlockPos::new(0, 64, 0), AIR),
            (BlockPos::new(0, 63, 0), WORKBENCH_BLOCK),
        ],
    ));
    assert!(admit(&mut context, &open_down(session, 1)).is_ok());
    drain(&mut context).expect("open anchors the bench");
    move_view(&mut context, session, 2, 10, 4);
    let bench = BlockPos::new(0, 63, 0);
    assert_eq!(anchor_of(&context, actor), Some(bench), "live anchor set");
    let record = context.read().actor(actor).expect("actor").clone();
    let ActorBody::Player(save) = record.body.clone() else {
        panic!("player keeps a player body");
    };
    let bytes = encode_player(&save).expect("save encodes");
    let loaded = decode_player(save.player_id, &bytes).expect("save decodes");
    assert_eq!(
        loaded.inventory, save.inventory,
        "the roundtrip keeps the grid-carrying inventory"
    );
    assert_eq!(loaded.health, save.health);
    assert_eq!(loaded.respawn_present, save.respawn_present);
    // The save carries no anchor lane by construction (`PlayerSave` has no
    // such field and no code writes live runtime back into a save), so the
    // live anchor lives in the overlay only.
    assert_eq!(anchor_of(&context, actor), Some(bench));
}

// These prepared rule cases execute real staging; they do not establish startup ownership.
fn settle_crafting_stack_case(
    before: InventoryRecord,
    command: Command,
    expected: Option<InventoryRecord>,
) {
    let session = player_session(91, "stack-case");
    let mut state = AuthorityState::try_new(limits(), 7).unwrap();
    let mut context = TickContext::harness(&mut state, TickBudget::full());
    let actor = scene(&mut context, session, before, ActorLifecycle::Active);
    let result = admit(&mut context, &envelope(session, 1, command));
    match expected {
        Some(after) => {
            assert!(
                result.is_ok(),
                "supported crafting command refused: {result:?}"
            );
            assert_eq!(inventory_of(&context, actor), after);
        }
        None => {
            assert!(result.is_err(), "invalid settlement must refuse");
            assert_eq!(
                inventory_of(&context, actor),
                before,
                "refusal must preserve the whole record"
            );
        }
    }
}

fn crafting_partial(from: u8, to: u8, single: bool) -> Command {
    Command::MovePartial(
        mornlea_domain::PartialMove::try_new(mornlea_domain::StackView::Crafting, from, to, single)
            .unwrap(),
    )
}

fn crafting_quick(from: u8) -> Command {
    Command::QuickMove(
        mornlea_domain::StackSource::try_new(mornlea_domain::StackView::Crafting, from).unwrap(),
    )
}

#[test]
fn crafting_stack_partial_odd_half_and_single() {
    // Go stackSplitAmount rounds an odd half upward and derives each amount from the current source.
    for (single, moved, left) in [(false, 3, 2), (true, 1, 4)] {
        let mut before = InventoryRecord::empty();
        before.slots[0] = stack(ITEM_DIRT, 5);
        let mut after = before;
        after.slots[0] = stack(ITEM_DIRT, left);
        after.crafting[0] = stack(ITEM_DIRT, moved);
        settle_crafting_stack_case(before, crafting_partial(9, 0, single), Some(after));
    }
}

#[test]
fn crafting_stack_partial_capacity_and_atomic_refusals() {
    let mut before = InventoryRecord::empty();
    before.slots[0] = stack(ITEM_DIRT, 5);
    before.crafting[0] = stack(ITEM_DIRT, 63);
    let mut after = before;
    after.slots[0] = stack(ITEM_DIRT, 4);
    after.crafting[0] = stack(ITEM_DIRT, 64);
    settle_crafting_stack_case(before, crafting_partial(9, 0, false), Some(after));
    for target in [stack(ITEM_DIRT, 64), stack(ITEM_STONE, 1)] {
        before.crafting[0] = target;
        settle_crafting_stack_case(before, crafting_partial(9, 0, false), None);
    }
    before.crafting = [ItemStack::default(); 9];
    for (from, to) in [(9, 10), (9, 4), (4, 9)] {
        settle_crafting_stack_case(before, crafting_partial(from, to, false), None);
    }
    settle_crafting_stack_case(
        InventoryRecord::empty(),
        crafting_partial(9, 0, false),
        None,
    );
}

#[test]
fn crafting_stack_quick_first_fit_retains_remainder() {
    let mut before = InventoryRecord::empty();
    before.slots[0] = stack(ITEM_DIRT, 5);
    before.crafting[0] = stack(ITEM_STONE, 1);
    before.crafting[1] = stack(ITEM_DIRT, 63);
    let mut after = before;
    after.slots[0] = stack(ITEM_DIRT, 4);
    after.crafting[1] = stack(ITEM_DIRT, 64);
    settle_crafting_stack_case(before, crafting_quick(9), Some(after));
}

#[test]
fn crafting_stack_quick_grid_to_pack_uses_pickup_order() {
    let mut before = InventoryRecord::empty();
    before.crafting[0] = stack(ITEM_DIRT, 5);
    before.slots[0] = stack(ITEM_STONE, 1);
    before.slots[9] = stack(ITEM_DIRT, 60);
    let mut after = before;
    after.crafting[0] = ItemStack::default();
    after.slots[1] = stack(ITEM_DIRT, 5);
    settle_crafting_stack_case(before, crafting_quick(0), Some(after));
}

#[test]
fn crafting_stack_quick_bench_extent_and_durability() {
    let mut before = InventoryRecord::empty();
    before.crafting_size = CraftingSize::Workbench;
    before.crafting[..8].fill(stack(ITEM_STONE, 64));
    before.slots[0] = durable(ITEM_STONE_PICKAXE, 1, 17);
    let mut after = before;
    after.slots[0] = ItemStack::default();
    after.crafting[8] = durable(ITEM_STONE_PICKAXE, 1, 17);
    settle_crafting_stack_case(before, crafting_quick(9), Some(after));
    let mut reverse = after;
    reverse.slots[0] = durable(ITEM_STONE_PICKAXE, 1, 17);
    reverse.crafting[8] = ItemStack::default();
    settle_crafting_stack_case(after, crafting_quick(8), Some(reverse));
}

#[test]
fn crafting_stack_quick_empty_inactive_and_full_refusals() {
    let mut before = InventoryRecord::empty();
    before.slots[0] = stack(ITEM_DIRT, 5);
    before.crafting[..4].fill(stack(ITEM_STONE, 64));
    settle_crafting_stack_case(before, crafting_quick(9), None);
    settle_crafting_stack_case(before, crafting_quick(4), None);
    settle_crafting_stack_case(InventoryRecord::empty(), crafting_quick(0), None);
    before.slots.fill(stack(ITEM_STONE, 64));
    before.crafting[0] = stack(ITEM_DIRT, 5);
    settle_crafting_stack_case(before, crafting_quick(0), None);
}

#[test]
fn crafting_stack_repack_failure_preserves_complete_record() {
    // An intentionally invalid prepared preimage still cannot be partially settled.
    let mut before = InventoryRecord::empty();
    before.slots.fill(stack(ITEM_STONE, 64));
    before.slots[0] = stack(ITEM_DIRT, 5);
    before.crafting[0] = stack(ITEM_COAL, 1);
    settle_crafting_stack_case(before, crafting_partial(9, 1, true), None);
    settle_crafting_stack_case(before, crafting_quick(9), None);
    before.crafting[1] = stack(ITEM_DIRT, 5);
    before.slots[0] = stack(ITEM_DIRT, 63);
    settle_crafting_stack_case(before, crafting_quick(1), None);
}

#[test]
fn crafting_stack_partial_repeated_settlement_reads_current_source() {
    let session = player_session(92, "current-source");
    let mut state = AuthorityState::try_new(limits(), 7).unwrap();
    let mut context = TickContext::harness(&mut state, TickBudget::full());
    let mut before = InventoryRecord::empty();
    before.slots[0] = stack(ITEM_DIRT, 5);
    let actor = scene(&mut context, session, before, ActorLifecycle::Active);
    for sequence in [1, 2] {
        admit(
            &mut context,
            &envelope(session, sequence, crafting_partial(9, 0, false)),
        )
        .unwrap();
    }
    let mut after = before;
    after.slots[0] = stack(ITEM_DIRT, 1);
    after.crafting[0] = stack(ITEM_DIRT, 4);
    assert_eq!(inventory_of(&context, actor), after);
}
