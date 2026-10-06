//! Workbench crafting: the bench open, whole/partial/quick crafting-view moves, recipe matching
//! and the atomic output take.
//!
//! This provider owns the crafting view and its lifecycle. The view is
//! grid `0..8` over backpack `9..44` and is counted separately from every
//! `ContainerRef`: a crafting view is never a container, holds no viewer
//! lease and never stages a container delta. Ordinary inventory moves
//! (`MoveInventory` and pack-internal partial moves) stay with the inventory
//! provider, container views stay with the container provider, and this
//! module refuses every shape it does not own without effect.
//!
//! Mirrored Go rows, each cited at its site:
//!
//! - `packages/shared/core/recipe.go` (`recipePattern`, `MatchCraftingGrid`):
//!   the 25 fixed trimmed shapes with their exact output triples, the
//!   outer-empty trim that keeps interior holes, the horizontal mirror retry
//!   only where the recipe flag allows it, and the never-matching vertical
//!   flip and rotation. The same values are sealed in
//!   `testdata/runtime-migration/server/capability-inventory.json` under
//!   `recipes` (source `recipe.go#recipePattern`).
//! - `packages/shared/core/inventory.go` (`Inventory.AddStack`,
//!   `ConsumeRecipe`): the four-phase stable insertion order the output
//!   credit and every repack rehearsal share, and the stricter consume pass
//!   that refuses residue and durability materials on top of the match.
//! - `packages/server/sim/entity/crafting.go` (`applyMoveCraftingStack`,
//!   `applyTakeCraftingOutput`, `canRepackCrafting`,
//!   `tryAddPreservingCrafting`, `repackCraftingSlots`, `closeWorkbench`,
//!   `craftingMoveCommandReasons`, `craftingViewSlot`): the whole-stack view
//!   move with its unlike-target refusal, the take that matches, consumes
//!   and credits in one atomic settlement, the repack invariant every
//!   settlement rehearses, and the close that reclaims the extended cells
//!   before the grid returns to the personal size.
//! - `packages/server/sim/entity/tick.go` (`CommandMoveCraftingStack`,
//!   `CommandTakeCraftingOutput`): the player-command settlement rows whose
//!   refusal reason this provider collapses into its single error shape.
//! - `packages/server/sim/entity/container.go` (`openContainer`): the bench
//!   arm of the authoritative open ray. The bench is an ordinary block, not
//!   a container: a settled open widens that player's grid, anchors the hit
//!   block on the runtime lane and ends any container lease in one atomic
//!   staging, and the lifecycle pass revalidates the anchor every tick.
//!
//! Staging is a whole-record inventory patch, so output insertion, every
//! input debit and the grid side of a settlement are one atomic staging: a
//! refused rehearsal stages nothing and the whole view is unchanged.
use mornlea_domain::{
    BlockPos, ChunkPos, Command, CommandEnvelope, CraftingSize, Dimension, LookAngles, StackView,
};
use mornlea_engine::native::contracts::raycast::{Ray, RayCursor, RaycastOp};
use mornlea_engine::native::raycast::NativeRaycast;
use mornlea_storage::ItemStack;

use crate::contracts::{
    ActorAux, ActorBody, ActorKey, ActorLifecycle, ActorRuntime, BlockObservation, ChunkKey,
    InventoryPatch, InventoryRecord, PhaseReport, RuleCall, RuleEffect, RulePhase, ServerError,
    SessionKey,
};
use crate::core::interaction::{look_direction, normalized_direction, target_block};
use crate::state::{AuthorityReadView, TickContext};

/// Crafting grid cells (`core.CraftingGridSlots`,
/// `packages/shared/core/recipe.go`).
const CRAFTING_GRID_SLOTS: usize = 9;

/// Personal grid extent: the 2x2 cells a personal grid owns
/// (`CraftingGridSizePersonal`, `packages/server/sim/entity/crafting.go`).
const PERSONAL_GRID_EXTENT: usize = 4;

/// First pack slot of the unified crafting view: grid `0..8`, backpack
/// `9..44` (`craftingViewSlots`, `packages/server/sim/entity/crafting.go`).
const CRAFT_VIEW_PACK_FIRST: usize = CRAFTING_GRID_SLOTS;

/// Unified player inventory length: hotbar `0..8` plus backpack `9..35`
/// (`core.InventorySlots`, `packages/shared/core/inventory.go`).
const INVENTORY_SLOTS: usize = 36;

/// The absent item number (`core.ItemNone`).
const ITEM_NONE: u16 = 0;

/// Workbench block (`core.WorkbenchID`).
const WORKBENCH_BLOCK: u16 = 45;

// Stable item numbers, mirrored from the frozen const block in
// `packages/shared/core/item.go`.
const ITEM_STONE: u16 = 1;
const ITEM_STONE_BRICK: u16 = 4;
const ITEM_COAL: u16 = 5;
const ITEM_IRON_INGOT: u16 = 7;
const ITEM_FURNACE: u16 = 8;
const ITEM_IRON_BLOCK: u16 = 9;
const ITEM_STONE_PICKAXE: u16 = 10;
const ITEM_IRON_PICKAXE: u16 = 11;
const ITEM_CHEST: u16 = 14;
const ITEM_LIGHT_BLOCK: u16 = 15;
const ITEM_GRAVEL: u16 = 19;
const ITEM_OAK_LOG: u16 = 20;
const ITEM_OAK_PLANKS: u16 = 21;
const ITEM_GLASS: u16 = 23;
const ITEM_STONE_HOE: u16 = 30;
const ITEM_IRON_HOE: u16 = 31;
const ITEM_WHEAT: u16 = 35;
const ITEM_BREAD: u16 = 36;
const ITEM_STICK: u16 = 37;
const ITEM_WORKBENCH: u16 = 38;
const ITEM_DOOR: u16 = 43;
const ITEM_TORCH: u16 = 44;
const ITEM_BED: u16 = 46;
const ITEM_WOODEN_SWORD: u16 = 47;
const ITEM_STONE_SWORD: u16 = 48;
const ITEM_IRON_SWORD: u16 = 49;
const ITEM_EMPTY_BUCKET: u16 = 55;
const ITEM_IRON_HELMET: u16 = 58;
const ITEM_IRON_CHESTPLATE: u16 = 59;
const ITEM_IRON_LEGGINGS: u16 = 60;
const ITEM_IRON_BOOTS: u16 = 61;
const ITEM_ARROW: u16 = 63;

/// One trimmed fixed-shape recipe (`core.RecipePattern`,
/// `packages/shared/core/recipe.go`). Cells stay in the stride-3 layout: the
/// shape occupies the top-left `width x height` block and the rest is
/// `ITEM_NONE`, so cell 4 is always the center cell. `mirror` declares the
/// horizontal-mirror retry; a vertical flip or rotation never matches.
struct RecipePattern {
    width: usize,
    height: usize,
    cells: [u16; CRAFTING_GRID_SLOTS],
    output: ItemStack,
    mirror: bool,
}

/// The fixed recipe registry, ascending by the stable recipe numbers 1..=25
/// (`recipePattern`, the append-only table). Every value is pinned by
/// `packages/shared/core/recipe.go` and by the sealed
/// `testdata/runtime-migration/server/capability-inventory.json` `recipes`
/// rows; the trailing name on each row is the Go const.
const RECIPES: [RecipePattern; 25] = [
    // RecipeStoneBricks
    RecipePattern {
        width: 2,
        height: 2,
        cells: [
            ITEM_STONE, ITEM_STONE, ITEM_NONE, ITEM_STONE, ITEM_STONE, ITEM_NONE, ITEM_NONE,
            ITEM_NONE, ITEM_NONE,
        ],
        output: ItemStack {
            item: ITEM_STONE_BRICK,
            count: 4,
            durability: 0,
        },
        mirror: true,
    },
    // RecipeFurnace
    RecipePattern {
        width: 3,
        height: 3,
        cells: [
            ITEM_STONE, ITEM_STONE, ITEM_STONE, ITEM_STONE, ITEM_NONE, ITEM_STONE, ITEM_STONE,
            ITEM_STONE, ITEM_STONE,
        ],
        output: ItemStack {
            item: ITEM_FURNACE,
            count: 1,
            durability: 0,
        },
        mirror: true,
    },
    // RecipeIronBlock
    RecipePattern {
        width: 3,
        height: 3,
        cells: [ITEM_IRON_INGOT; CRAFTING_GRID_SLOTS],
        output: ItemStack {
            item: ITEM_IRON_BLOCK,
            count: 1,
            durability: 0,
        },
        mirror: true,
    },
    // RecipeStonePickaxe
    RecipePattern {
        width: 3,
        height: 3,
        cells: [
            ITEM_STONE, ITEM_STONE, ITEM_STONE, ITEM_NONE, ITEM_STICK, ITEM_NONE, ITEM_NONE,
            ITEM_STICK, ITEM_NONE,
        ],
        output: ItemStack {
            item: ITEM_STONE_PICKAXE,
            count: 1,
            durability: 131,
        },
        mirror: false,
    },
    // RecipeIronPickaxe
    RecipePattern {
        width: 3,
        height: 3,
        cells: [
            ITEM_IRON_INGOT,
            ITEM_IRON_INGOT,
            ITEM_IRON_INGOT,
            ITEM_NONE,
            ITEM_STICK,
            ITEM_NONE,
            ITEM_NONE,
            ITEM_STICK,
            ITEM_NONE,
        ],
        output: ItemStack {
            item: ITEM_IRON_PICKAXE,
            count: 1,
            durability: 250,
        },
        mirror: false,
    },
    // RecipeChest
    RecipePattern {
        width: 3,
        height: 3,
        cells: [
            ITEM_OAK_PLANKS,
            ITEM_OAK_PLANKS,
            ITEM_OAK_PLANKS,
            ITEM_OAK_PLANKS,
            ITEM_NONE,
            ITEM_OAK_PLANKS,
            ITEM_OAK_PLANKS,
            ITEM_OAK_PLANKS,
            ITEM_OAK_PLANKS,
        ],
        output: ItemStack {
            item: ITEM_CHEST,
            count: 1,
            durability: 0,
        },
        mirror: true,
    },
    // RecipeOakPlanks
    RecipePattern {
        width: 1,
        height: 1,
        cells: [
            ITEM_OAK_LOG,
            ITEM_NONE,
            ITEM_NONE,
            ITEM_NONE,
            ITEM_NONE,
            ITEM_NONE,
            ITEM_NONE,
            ITEM_NONE,
            ITEM_NONE,
        ],
        output: ItemStack {
            item: ITEM_OAK_PLANKS,
            count: 4,
            durability: 0,
        },
        mirror: true,
    },
    // RecipeLightBlock
    RecipePattern {
        width: 2,
        height: 2,
        cells: [
            ITEM_GLASS, ITEM_GLASS, ITEM_NONE, ITEM_GLASS, ITEM_GLASS, ITEM_NONE, ITEM_NONE,
            ITEM_NONE, ITEM_NONE,
        ],
        output: ItemStack {
            item: ITEM_LIGHT_BLOCK,
            count: 4,
            durability: 0,
        },
        mirror: true,
    },
    // RecipeStoneHoe
    RecipePattern {
        width: 2,
        height: 2,
        cells: [
            ITEM_STONE, ITEM_STICK, ITEM_NONE, ITEM_STONE, ITEM_STICK, ITEM_NONE, ITEM_NONE,
            ITEM_NONE, ITEM_NONE,
        ],
        output: ItemStack {
            item: ITEM_STONE_HOE,
            count: 1,
            durability: 131,
        },
        mirror: false,
    },
    // RecipeIronHoe
    RecipePattern {
        width: 2,
        height: 2,
        cells: [
            ITEM_IRON_INGOT,
            ITEM_STICK,
            ITEM_NONE,
            ITEM_IRON_INGOT,
            ITEM_STICK,
            ITEM_NONE,
            ITEM_NONE,
            ITEM_NONE,
            ITEM_NONE,
        ],
        output: ItemStack {
            item: ITEM_IRON_HOE,
            count: 1,
            durability: 250,
        },
        mirror: false,
    },
    // RecipeBread
    RecipePattern {
        width: 3,
        height: 1,
        cells: [
            ITEM_WHEAT, ITEM_WHEAT, ITEM_WHEAT, ITEM_NONE, ITEM_NONE, ITEM_NONE, ITEM_NONE,
            ITEM_NONE, ITEM_NONE,
        ],
        output: ItemStack {
            item: ITEM_BREAD,
            count: 1,
            durability: 0,
        },
        mirror: true,
    },
    // RecipeStick
    RecipePattern {
        width: 1,
        height: 2,
        cells: [
            ITEM_OAK_PLANKS,
            ITEM_NONE,
            ITEM_NONE,
            ITEM_OAK_PLANKS,
            ITEM_NONE,
            ITEM_NONE,
            ITEM_NONE,
            ITEM_NONE,
            ITEM_NONE,
        ],
        output: ItemStack {
            item: ITEM_STICK,
            count: 4,
            durability: 0,
        },
        mirror: true,
    },
    // RecipeWorkbench
    RecipePattern {
        width: 2,
        height: 2,
        cells: [
            ITEM_OAK_PLANKS,
            ITEM_OAK_PLANKS,
            ITEM_NONE,
            ITEM_OAK_PLANKS,
            ITEM_OAK_PLANKS,
            ITEM_NONE,
            ITEM_NONE,
            ITEM_NONE,
            ITEM_NONE,
        ],
        output: ItemStack {
            item: ITEM_WORKBENCH,
            count: 1,
            durability: 0,
        },
        mirror: true,
    },
    // RecipeDoor
    RecipePattern {
        width: 2,
        height: 3,
        cells: [
            ITEM_OAK_PLANKS,
            ITEM_OAK_PLANKS,
            ITEM_NONE,
            ITEM_OAK_PLANKS,
            ITEM_OAK_PLANKS,
            ITEM_NONE,
            ITEM_OAK_PLANKS,
            ITEM_OAK_PLANKS,
            ITEM_NONE,
        ],
        output: ItemStack {
            item: ITEM_DOOR,
            count: 3,
            durability: 0,
        },
        mirror: true,
    },
    // RecipeTorch
    RecipePattern {
        width: 1,
        height: 2,
        cells: [
            ITEM_COAL, ITEM_NONE, ITEM_NONE, ITEM_STICK, ITEM_NONE, ITEM_NONE, ITEM_NONE,
            ITEM_NONE, ITEM_NONE,
        ],
        output: ItemStack {
            item: ITEM_TORCH,
            count: 4,
            durability: 0,
        },
        mirror: true,
    },
    // RecipeBed
    RecipePattern {
        width: 3,
        height: 3,
        cells: [
            ITEM_WHEAT,
            ITEM_WHEAT,
            ITEM_WHEAT,
            ITEM_NONE,
            ITEM_NONE,
            ITEM_NONE,
            ITEM_OAK_PLANKS,
            ITEM_OAK_PLANKS,
            ITEM_OAK_PLANKS,
        ],
        output: ItemStack {
            item: ITEM_BED,
            count: 1,
            durability: 0,
        },
        mirror: true,
    },
    // RecipeWoodenSword
    RecipePattern {
        width: 1,
        height: 3,
        cells: [
            ITEM_OAK_PLANKS,
            ITEM_NONE,
            ITEM_NONE,
            ITEM_OAK_PLANKS,
            ITEM_NONE,
            ITEM_NONE,
            ITEM_STICK,
            ITEM_NONE,
            ITEM_NONE,
        ],
        output: ItemStack {
            item: ITEM_WOODEN_SWORD,
            count: 1,
            durability: 59,
        },
        mirror: true,
    },
    // RecipeStoneSword
    RecipePattern {
        width: 1,
        height: 3,
        cells: [
            ITEM_STONE, ITEM_NONE, ITEM_NONE, ITEM_STONE, ITEM_NONE, ITEM_NONE, ITEM_STICK,
            ITEM_NONE, ITEM_NONE,
        ],
        output: ItemStack {
            item: ITEM_STONE_SWORD,
            count: 1,
            durability: 131,
        },
        mirror: true,
    },
    // RecipeIronSword
    RecipePattern {
        width: 1,
        height: 3,
        cells: [
            ITEM_IRON_INGOT,
            ITEM_NONE,
            ITEM_NONE,
            ITEM_IRON_INGOT,
            ITEM_NONE,
            ITEM_NONE,
            ITEM_STICK,
            ITEM_NONE,
            ITEM_NONE,
        ],
        output: ItemStack {
            item: ITEM_IRON_SWORD,
            count: 1,
            durability: 250,
        },
        mirror: true,
    },
    // RecipeBucket
    RecipePattern {
        width: 3,
        height: 2,
        cells: [
            ITEM_IRON_INGOT,
            ITEM_NONE,
            ITEM_IRON_INGOT,
            ITEM_NONE,
            ITEM_IRON_INGOT,
            ITEM_NONE,
            ITEM_NONE,
            ITEM_NONE,
            ITEM_NONE,
        ],
        output: ItemStack {
            item: ITEM_EMPTY_BUCKET,
            count: 1,
            durability: 0,
        },
        mirror: true,
    },
    // RecipeIronHelmet
    RecipePattern {
        width: 3,
        height: 2,
        cells: [
            ITEM_IRON_INGOT,
            ITEM_IRON_INGOT,
            ITEM_IRON_INGOT,
            ITEM_IRON_INGOT,
            ITEM_NONE,
            ITEM_IRON_INGOT,
            ITEM_NONE,
            ITEM_NONE,
            ITEM_NONE,
        ],
        output: ItemStack {
            item: ITEM_IRON_HELMET,
            count: 1,
            durability: 165,
        },
        mirror: true,
    },
    // RecipeIronChestplate
    RecipePattern {
        width: 3,
        height: 3,
        cells: [
            ITEM_IRON_INGOT,
            ITEM_NONE,
            ITEM_IRON_INGOT,
            ITEM_IRON_INGOT,
            ITEM_IRON_INGOT,
            ITEM_IRON_INGOT,
            ITEM_IRON_INGOT,
            ITEM_IRON_INGOT,
            ITEM_IRON_INGOT,
        ],
        output: ItemStack {
            item: ITEM_IRON_CHESTPLATE,
            count: 1,
            durability: 240,
        },
        mirror: true,
    },
    // RecipeIronLeggings
    RecipePattern {
        width: 3,
        height: 3,
        cells: [
            ITEM_IRON_INGOT,
            ITEM_IRON_INGOT,
            ITEM_IRON_INGOT,
            ITEM_IRON_INGOT,
            ITEM_NONE,
            ITEM_IRON_INGOT,
            ITEM_IRON_INGOT,
            ITEM_NONE,
            ITEM_IRON_INGOT,
        ],
        output: ItemStack {
            item: ITEM_IRON_LEGGINGS,
            count: 1,
            durability: 225,
        },
        mirror: true,
    },
    // RecipeIronBoots
    RecipePattern {
        width: 3,
        height: 2,
        cells: [
            ITEM_IRON_INGOT,
            ITEM_NONE,
            ITEM_IRON_INGOT,
            ITEM_IRON_INGOT,
            ITEM_NONE,
            ITEM_IRON_INGOT,
            ITEM_NONE,
            ITEM_NONE,
            ITEM_NONE,
        ],
        output: ItemStack {
            item: ITEM_IRON_BOOTS,
            count: 1,
            durability: 195,
        },
        mirror: true,
    },
    // RecipeArrow
    RecipePattern {
        width: 1,
        height: 2,
        cells: [
            ITEM_GRAVEL,
            ITEM_NONE,
            ITEM_NONE,
            ITEM_STICK,
            ITEM_NONE,
            ITEM_NONE,
            ITEM_NONE,
            ITEM_NONE,
            ITEM_NONE,
        ],
        output: ItemStack {
            item: ITEM_ARROW,
            count: 2,
            durability: 0,
        },
        mirror: true,
    },
];

/// Effective grid edge length for one crafting size: a personal grid is 2x2
/// and a bench 3x3 (`CraftingGridSizePersonal` / `CraftingGridSizeWorkbench`
/// in `packages/server/sim/entity/crafting.go`).
pub(crate) fn grid_extent(size: CraftingSize) -> u8 {
    match size {
        CraftingSize::Personal => 2,
        CraftingSize::Workbench => 3,
    }
}

/// Collapses count-zero residue into empty cells and refuses residue beyond
/// the effective size, the shared normalization of `MatchCraftingGrid` and
/// `ConsumeRecipe`: a personal grid keeps its extended cells empty, so a
/// shrunken grid cannot keep matching over stale content.
fn normalized_cells(
    size: u8,
    slots: &[ItemStack; CRAFTING_GRID_SLOTS],
) -> Option<[u16; CRAFTING_GRID_SLOTS]> {
    let mut cells = [ITEM_NONE; CRAFTING_GRID_SLOTS];
    for index in 0..CRAFTING_GRID_SLOTS {
        if slots[index].count > 0 {
            cells[index] = slots[index].item;
        }
        if index >= usize::from(size) * usize::from(size) && cells[index] != ITEM_NONE {
            return None;
        }
    }
    Some(cells)
}

/// The non-empty bounding box of one normalized grid at stride `size`; an
/// all-empty grid has no match (`trimPattern` in
/// `packages/shared/core/recipe.go`).
fn trimmed_box(
    size: u8,
    cells: &[u16; CRAFTING_GRID_SLOTS],
) -> Option<(usize, usize, usize, usize)> {
    let mut min_x = usize::MAX;
    let mut min_y = usize::MAX;
    let mut max_x = 0;
    let mut max_y = 0;
    let mut found = false;
    for y in 0..usize::from(size) {
        for x in 0..usize::from(size) {
            if cells[y * usize::from(size) + x] == ITEM_NONE {
                continue;
            }
            found = true;
            min_x = min_x.min(x);
            max_x = max_x.max(x);
            min_y = min_y.min(y);
            max_y = max_y.max(y);
        }
    }
    if !found {
        return None;
    }
    Some((min_x, min_y, max_x - min_x + 1, max_y - min_y + 1))
}

/// Compares one already-trimmed grid against a pattern at the bounding-box
/// origin, reversing the pattern columns for the horizontal mirror attempt
/// (`matchesPattern` in `packages/shared/core/recipe.go`).
fn covers(
    size: u8,
    cells: &[u16; CRAFTING_GRID_SLOTS],
    pattern: &RecipePattern,
    origin_x: usize,
    origin_y: usize,
    mirror: bool,
) -> bool {
    for y in 0..pattern.height {
        for x in 0..pattern.width {
            let pattern_x = if mirror { pattern.width - 1 - x } else { x };
            let material = pattern.cells[y * 3 + pattern_x];
            if material != cells[(origin_y + y) * usize::from(size) + origin_x + x] {
                return false;
            }
        }
    }
    true
}

/// Matches one grid against the fixed registry and returns the hit with its
/// complete output (`MatchCraftingGrid` in
/// `packages/shared/core/recipe.go`): size 2 or 3 only, residue outside the
/// effective cells refuses, the trim keeps interior holes, recipes compare
/// in ascending order, and the mirror retries once only where the flag
/// allows it. Rotations and vertical flips are never in the semantics.
/// This is the frozen recipe matcher shared by the take provider and the
/// crafting publication.
pub(crate) fn match_grid(
    size: u8,
    slots: &[ItemStack; CRAFTING_GRID_SLOTS],
) -> Option<(usize, ItemStack)> {
    if size != 2 && size != 3 {
        return None;
    }
    let cells = normalized_cells(size, slots)?;
    let (origin_x, origin_y, width, height) = trimmed_box(size, &cells)?;
    for (index, pattern) in RECIPES.iter().enumerate() {
        if pattern.width != width || pattern.height != height {
            continue;
        }
        if covers(size, &cells, pattern, origin_x, origin_y, false)
            || (pattern.mirror && covers(size, &cells, pattern, origin_x, origin_y, true))
        {
            return Some((index, pattern.output));
        }
    }
    None
}

/// Decrements every pattern-covered cell of one candidate copy by exactly
/// one, the single-alignment form of `consumeAligned` in
/// `packages/shared/core/inventory.go`. The consume pass is stricter than
/// the match: a pattern hole must hold a true zero stack, a covered cell
/// must hold the material with a positive count, and a durability item is
/// never an ingredient (`ConsumeRecipe`: nonzero-durability items never
/// settle as shape material). Any failed cell abandons the whole attempt.
fn consume_aligned(
    candidate: &mut [ItemStack; CRAFTING_GRID_SLOTS],
    size: u8,
    pattern: &RecipePattern,
    origin_x: usize,
    origin_y: usize,
    mirror: bool,
) -> bool {
    for y in 0..pattern.height {
        for x in 0..pattern.width {
            let pattern_x = if mirror { pattern.width - 1 - x } else { x };
            let material = pattern.cells[y * 3 + pattern_x];
            let index = (origin_y + y) * usize::from(size) + origin_x + x;
            let mut stack = candidate[index];
            if material == ITEM_NONE {
                if stack.item != ITEM_NONE || stack.count != 0 {
                    return false;
                }
                continue;
            }
            if stack.item != material
                || stack.count == 0
                || mornlea_domain::durability_max(stack.item).is_some()
            {
                return false;
            }
            stack.count -= 1;
            candidate[index] = if stack.count == 0 {
                ItemStack::default()
            } else {
                stack
            };
        }
    }
    true
}

/// Consumes one matched shape on a copy of the grid and returns the consumed
/// grid (`ConsumeRecipe` in `packages/shared/core/inventory.go`): the same
/// normalization and alignment as the match, one decrement per covered cell,
/// and the mirror retry in the same order the match ran. Failure returns
/// nothing and leaves the caller's grid untouched.
fn consume_grid(
    size: u8,
    slots: &[ItemStack; CRAFTING_GRID_SLOTS],
    pattern: &RecipePattern,
) -> Option<[ItemStack; CRAFTING_GRID_SLOTS]> {
    if size != 2 && size != 3 {
        return None;
    }
    let cells = normalized_cells(size, slots)?;
    let (origin_x, origin_y, width, height) = trimmed_box(size, &cells)?;
    if pattern.width != width || pattern.height != height {
        return None;
    }
    let attempts = usize::from(pattern.mirror) + 1;
    for attempt in 0..attempts {
        let mut candidate = *slots;
        if consume_aligned(
            &mut candidate,
            size,
            pattern,
            origin_x,
            origin_y,
            attempt == 1,
        ) {
            return Some(candidate);
        }
    }
    None
}

/// Reads one unified view slot: grid `0..8` direct, pack `9..44` offset by
/// the grid length (`craftingViewSlot` in
/// `packages/server/sim/entity/crafting.go`).
pub(crate) fn view_slot(record: &InventoryRecord, slot: usize) -> ItemStack {
    if slot < CRAFTING_GRID_SLOTS {
        record.crafting[slot]
    } else {
        record.slots[slot - CRAFT_VIEW_PACK_FIRST]
    }
}

/// Writes one unified view slot (`setCraftingViewSlot` in
/// `packages/server/sim/entity/crafting.go`); the caller keeps the grid cell
/// inside the effective size.
pub(crate) fn set_view_slot(record: &mut InventoryRecord, slot: usize, stack: ItemStack) {
    if slot < CRAFTING_GRID_SLOTS {
        record.crafting[slot] = stack;
    } else {
        record.slots[slot - CRAFT_VIEW_PACK_FIRST] = stack;
    }
}

/// Credits one source stack into the pack, the exact `Inventory.AddStack`
/// row (`packages/shared/core/inventory.go`): hotbar merge, hotbar empty,
/// backpack merge, backpack empty, each in ascending slot order. An
/// empty-slot landing inherits the source durability, and the unabsorbed
/// remainder keeps the source form.
pub(crate) fn add_stack(
    slots: &[ItemStack; INVENTORY_SLOTS],
    source: ItemStack,
) -> ([ItemStack; INVENTORY_SLOTS], ItemStack) {
    let Some(limit) = mornlea_domain::item_stack_limit(source.item) else {
        return (*slots, source);
    };
    if source.item == ITEM_NONE || source.count == 0 {
        return (*slots, source);
    }
    let mut next = *slots;
    let mut remaining = source.count;
    for (merge, first, last) in [(true, 0, 8), (false, 0, 8), (true, 9, 35), (false, 9, 35)] {
        for index in first..=last {
            if remaining == 0 {
                return (next, ItemStack::default());
            }
            let held = next[index];
            if merge {
                if held.item != source.item || held.count >= limit {
                    continue;
                }
            } else if held.item != ITEM_NONE {
                continue;
            }
            let base = if merge { held.count } else { 0 };
            let moved = (limit - base).min(remaining);
            next[index] = ItemStack {
                item: source.item,
                count: base + moved,
                durability: if merge {
                    held.durability
                } else {
                    source.durability
                },
            };
            remaining -= moved;
        }
    }
    (
        next,
        ItemStack {
            item: source.item,
            count: remaining,
            durability: source.durability,
        },
    )
}

/// Bench repack invariant, the exact `canRepackCrafting` row
/// (`packages/server/sim/entity/crafting.go`): every nonempty grid cell must
/// still credit fully into the pack through the pickup order.
pub(crate) fn can_repack(slots: &[ItemStack; 36], grid: &[ItemStack; CRAFTING_GRID_SLOTS]) -> bool {
    let mut staged = *slots;
    for stack in grid.iter() {
        if stack.item == ITEM_NONE {
            continue;
        }
        let (next, leftover) = add_stack(&staged, *stack);
        if leftover.count != 0 {
            return false;
        }
        staged = next;
    }
    true
}

/// Whole-stack move between two unified view slots, the exact
/// `applyMoveCraftingStack` row (`packages/server/sim/entity/crafting.go`):
/// an empty target inherits the source item and durability truncated to the
/// amount, a same-item target merges up to the stack cap with the remainder
/// kept at the source, and an unlike target refuses because a grid move
/// never swaps. The repack rehearsal runs over the moved copy, so a move
/// that would strand grid content refuses whole.
fn move_view_stack(record: &mut InventoryRecord, from: usize, to: usize, amount: u8) -> bool {
    let source = view_slot(record, from);
    if source.item == ITEM_NONE || amount == 0 {
        return false;
    }
    let target = view_slot(record, to);
    let next_source;
    let next_target;
    if target.item == ITEM_NONE {
        let mut landed = source;
        landed.count = amount.min(source.count);
        next_target = landed;
        next_source = if source.count > landed.count {
            ItemStack {
                item: source.item,
                count: source.count - landed.count,
                durability: source.durability,
            }
        } else {
            ItemStack::default()
        };
    } else if target.item == source.item {
        let Some(limit) = mornlea_domain::item_stack_limit(source.item) else {
            return false;
        };
        if target.count >= limit {
            return false;
        }
        let moved = amount.min(source.count).min(limit - target.count);
        let mut merged = target;
        merged.count += moved;
        next_target = merged;
        next_source = if source.count > moved {
            ItemStack {
                item: source.item,
                count: source.count - moved,
                durability: source.durability,
            }
        } else {
            ItemStack::default()
        };
    } else {
        // Unlike targets never swap: a grid move only merges or migrates.
        return false;
    }
    set_view_slot(record, from, next_source);
    set_view_slot(record, to, next_target);
    if !record
        .slots
        .iter()
        .chain(record.crafting.iter())
        .all(ItemStack::is_valid)
    {
        return false;
    }
    can_repack(&record.slots, &record.crafting)
}

/// Stages the whole-record inventory patch when a settlement changed the
/// view. The patch is the single atomic staging arm: every slot and grid
/// cell of the settlement moves together or not at all.
fn stage_patch(
    ctx: &mut TickContext<'_>,
    actor: ActorKey,
    before: InventoryRecord,
    after: InventoryRecord,
) -> Result<(), ServerError> {
    if after == before {
        return Ok(());
    }
    let patch = InventoryPatch::try_new(actor, before, after)
        .map_err(|_| ServerError::InvalidInput { field: "inventory" })?;
    ctx.stage(RuleEffect::Inventory(patch))
        .map_err(|_| ServerError::InvalidInput { field: "inventory" })
}

/// The applied report of one settled player command.
fn applied_report() -> PhaseReport {
    PhaseReport {
        examined: 1,
        applied: 1,
        carried: 0,
        rejected: 0,
    }
}

/// Computes a quick move on a private copy using Go's first-fitting grid
/// cell or four-phase pickup order. Repack refusal discards the whole copy;
/// it never searches a later target after a fitting cell fails rehearsal.
fn quick_move_view(record: &mut InventoryRecord, from: usize) -> bool {
    let source = view_slot(record, from);
    if source.item == ITEM_NONE {
        return false;
    }
    if from < CRAFTING_GRID_SLOTS {
        let (slots, leftover) = add_stack(&record.slots, source);
        if leftover.count == source.count {
            return false;
        }
        record.slots = slots;
        set_view_slot(record, from, leftover);
    } else {
        let Some(limit) = mornlea_domain::item_stack_limit(source.item) else {
            return false;
        };
        let extent = usize::from(grid_extent(record.crafting_size));
        let Some(to) = (0..extent * extent).find(|&slot| {
            let target = record.crafting[slot];
            target.item == ITEM_NONE || (target.item == source.item && target.count < limit)
        }) else {
            return false;
        };
        let mut target = record.crafting[to];
        if target.item == ITEM_NONE {
            target = source;
            target.count = 0;
        }
        let moved = source.count.min(limit - target.count);
        target.count += moved;
        let leftover = if moved == source.count {
            ItemStack::default()
        } else {
            ItemStack {
                item: source.item,
                count: source.count - moved,
                durability: source.durability,
            }
        };
        set_view_slot(record, from, leftover);
        record.crafting[to] = target;
    }
    record.slots.iter().all(ItemStack::is_valid) && can_repack(&record.slots, &record.crafting)
}

/// Settles one admission or runs the lifecycle pass, exactly one of which
/// the call shape names. Admission (`RulePhase::PlayerCommand` with a
/// command) settles whole, partial and quick crafting-view moves and the output take inline,
/// mirroring the Go command rows that touch only player state, and defers
/// the bench opens into the lifecycle phase. The lifecycle pass
/// (`RulePhase::WorkbenchLifecycle` with no command) settles the deferred
/// bench opens against the authoritative ray and closes lapsed benches.
/// Every other shape refuses without effect.
pub fn run(ctx: &mut TickContext<'_>, call: RuleCall<'_>) -> Result<PhaseReport, ServerError> {
    match call.phase {
        RulePhase::PlayerCommand => admit(ctx, &call),
        RulePhase::WorkbenchLifecycle => advance(ctx, &call),
        _ => Err(ServerError::InvalidInput { field: "phase" }),
    }
}

/// One player-command settlement. The envelope's session is the sole
/// identity source; a refused settlement stages nothing, so the observable
/// state hash is untouched.
fn admit(ctx: &mut TickContext<'_>, call: &RuleCall<'_>) -> Result<PhaseReport, ServerError> {
    if call.actor.is_some() || call.internal.is_some() {
        return Err(ServerError::InvalidInput { field: "command" });
    }
    let envelope = call
        .command
        .ok_or(ServerError::InvalidInput { field: "command" })?;
    let session = SessionKey::from_raw(envelope.session())
        .ok_or(ServerError::InvalidInput { field: "session" })?;
    let actor = ActorKey::Player(session);
    match envelope.command() {
        Command::MovePartial(movement) if movement.view() == StackView::Crafting => {
            let from = usize::from(movement.from());
            let to = usize::from(movement.to());
            let before = *ctx
                .read()
                .inventory(actor)
                .ok_or(ServerError::InvalidInput { field: "session" })?;
            let mut after = before;
            let extent = usize::from(grid_extent(after.crafting_size));
            if (from >= CRAFTING_GRID_SLOTS && to >= CRAFTING_GRID_SLOTS)
                || [from, to]
                    .into_iter()
                    .any(|slot| slot < CRAFTING_GRID_SLOTS && slot >= extent * extent)
            {
                return Err(ServerError::InvalidInput { field: "crafting" });
            }
            // The client chooses half or single; the current authority stack
            // supplies the count, including odd halves rounded upward.
            let count = view_slot(&after, from).count;
            let amount = if movement.single() {
                1
            } else {
                count.div_ceil(2)
            };
            if !move_view_stack(&mut after, from, to, amount) {
                return Err(ServerError::InvalidInput { field: "crafting" });
            }
            stage_patch(ctx, actor, before, after)?;
            ctx.record_crafting_command_publication_dirty(session);
            Ok(applied_report())
        }
        Command::QuickMove(source) if source.view() == StackView::Crafting => {
            let from = usize::from(source.slot());
            let before = *ctx
                .read()
                .inventory(actor)
                .ok_or(ServerError::InvalidInput { field: "session" })?;
            let mut after = before;
            let extent = usize::from(grid_extent(after.crafting_size));
            if (from < CRAFTING_GRID_SLOTS && from >= extent * extent)
                || !quick_move_view(&mut after, from)
            {
                return Err(ServerError::InvalidInput { field: "crafting" });
            }
            stage_patch(ctx, actor, before, after)?;
            ctx.record_crafting_command_publication_dirty(session);
            Ok(applied_report())
        }
        Command::MoveCrafting(movement) => {
            let from = usize::from(movement.from());
            let to = usize::from(movement.to());
            let before = *ctx
                .read()
                .inventory(actor)
                .ok_or(ServerError::InvalidInput { field: "session" })?;
            let mut after = before;
            // The size gate of `craftingMoveCommandReasons`: a grid-side end
            // beyond the effective cells refuses, so the personal grid
            // rejects its extended slots in both directions. The static view
            // bounds, the same-slot rule and the pack-internal rule are
            // already the domain value rules `CraftingMove` enforces.
            let edge = usize::from(grid_extent(after.crafting_size));
            for end in [from, to] {
                if end < CRAFTING_GRID_SLOTS && end >= edge * edge {
                    return Err(ServerError::InvalidInput { field: "crafting" });
                }
            }
            // The whole-stack amount is the source's current count, the
            // `CommandMoveCraftingStack` row in
            // `packages/server/sim/entity/tick.go`.
            let amount = view_slot(&after, from).count;
            if !move_view_stack(&mut after, from, to, amount) {
                return Err(ServerError::InvalidInput { field: "crafting" });
            }
            stage_patch(ctx, actor, before, after)?;
            // An accepted move settles into the owner record and the private
            // grid, so it marks both owner-only publication lanes.
            ctx.record_crafting_command_publication_dirty(session);
            Ok(applied_report())
        }
        Command::TakeCraftingOutput => {
            let before = *ctx
                .read()
                .inventory(actor)
                .ok_or(ServerError::InvalidInput { field: "session" })?;
            let mut after = before;
            let size = grid_extent(after.crafting_size);
            let Some((index, output)) = match_grid(size, &after.crafting) else {
                return Err(ServerError::InvalidInput { field: "crafting" });
            };
            let pattern = &RECIPES[index];
            // Matching and consuming differ in strictness, so a successful
            // match does not imply a successful consume; the refusal is
            // stable and leaves every cell unchanged.
            let Some(consumed) = consume_grid(size, &after.crafting, pattern) else {
                return Err(ServerError::InvalidInput { field: "crafting" });
            };
            // `tryAddPreservingCrafting`: the full output must enter the
            // pack and the pack after that must still absorb the consumed
            // grid. Either half failing refuses the whole take, which is the
            // full-result-slot refusal of a full pack.
            let (next_slots, leftover) = add_stack(&after.slots, output);
            if leftover.count != 0 {
                return Err(ServerError::InvalidInput { field: "crafting" });
            }
            if !can_repack(&next_slots, &consumed) {
                return Err(ServerError::InvalidInput { field: "crafting" });
            }
            after.slots = next_slots;
            after.crafting = consumed;
            stage_patch(ctx, actor, before, after)?;
            // An accepted take, like the move above, marks both owner-only
            // publication lanes.
            ctx.record_crafting_command_publication_dirty(session);
            Ok(applied_report())
        }
        // The bench open rides the same admitted-open surface as the
        // container opens: the envelope carries only the look, so the
        // authoritative ray decides at settlement time whether the hit is a
        // bench (this phase) or a furnace or chest (the container
        // provider's phase, whose drain refuses bench hits without effect).
        Command::OpenContainer(_) => {
            ctx.defer(*envelope, RulePhase::WorkbenchLifecycle)?;
            Ok(applied_report())
        }
        // Every other command kind belongs to its own provider.
        _ => Err(ServerError::InvalidInput { field: "command" }),
    }
}

/// The lifecycle pass: the deferred bench opens settle in admission order
/// against the current state, then every bench-size session the actor
/// records name is checked and lapsed ones close.
fn advance(ctx: &mut TickContext<'_>, call: &RuleCall<'_>) -> Result<PhaseReport, ServerError> {
    if call.actor.is_some() || call.command.is_some() || call.internal.is_some() {
        return Err(ServerError::InvalidInput { field: "phase" });
    }
    let mut examined = 0;
    let mut applied = 0;
    let mut carried = 0;
    let mut rejected = 0;
    for envelope in ctx.deferred(RulePhase::WorkbenchLifecycle) {
        examined += 1;
        match envelope.command() {
            Command::OpenContainer(look) => match settle_bench_open(ctx, &envelope, look) {
                OpenOutcome::Applied => applied += 1,
                OpenOutcome::Carried => carried += 1,
                OpenOutcome::Refused => rejected += 1,
            },
            _ => rejected += 1,
        }
    }
    // Every bench-size session revalidates here, the full
    // `advanceWorkbenchLifecycle` row (`packages/server/sim/entity/crafting.go`),
    // in ascending actor key order like the Go session sort. Lapsed records
    // close exactly as before; Active actors run the staged anchor against
    // the chunk, block and reach gates. A bench with no staged anchor keeps
    // the lapsed-only behavior: there is nothing to invalidate. A close the
    // grid cannot repack stages nothing and stops the caller with a hard
    // invariant failure, preserving contents without settling later actors.
    let mut bench: Vec<ActorKey> = ctx
        .read()
        .actors()
        .iter()
        .filter(|record| {
            matches!(record.key, ActorKey::Player(_))
                && ctx
                    .read()
                    .inventory(record.key)
                    .is_some_and(|inventory| inventory.crafting_size == CraftingSize::Workbench)
        })
        .map(|record| record.key)
        .collect();
    bench.sort();
    for actor in bench {
        examined += 1;
        let basis = ctx.read().actor(actor).cloned();
        let Some(record) = basis.filter(|record| record.lifecycle == ActorLifecycle::Active) else {
            close_bench(ctx, actor)?;
            applied += 1;
            continue;
        };
        let anchored = match ctx.read().runtime(actor) {
            Some(runtime) => match &runtime.aux {
                ActorAux::Player { workbench, .. } => *workbench,
                _ => None,
            },
            None => None,
        };
        let Some(anchor) = anchored else {
            continue;
        };
        let Some(environment) = ctx.read().environment().cloned() else {
            // Without tunables the reach check cannot run, so the bench
            // fails closed exactly like a failed block lookup.
            close_bench(ctx, actor)?;
            applied += 1;
            continue;
        };
        let position = record.motion.position().get();
        let eye = [
            position[0],
            position[1] + environment.tunables.eye_height(),
            position[2],
        ];
        if anchor_valid(
            &ctx.read(),
            record.dimension,
            eye,
            environment.tunables.interaction_reach(),
            anchor,
        ) {
            continue;
        }
        close_bench(ctx, actor)?;
        applied += 1;
    }
    Ok(PhaseReport {
        examined,
        applied,
        carried,
        rejected,
    })
}

/// The outcome of one settled bench open.
pub(crate) enum OpenOutcome {
    /// The grid, anchor or lease staging changed the view.
    Applied,
    /// The grid already holds the bench size over the same anchor with no
    /// lease to end, so the re-open stages nothing.
    Carried,
    /// The ray hit nothing the bench arm owns.
    Refused,
}

/// The live command owner and raw lifecycle compatibility share this atomic arm.
/// Settles one bench open through the authoritative ray, the
/// `openContainer` workbench arm (`packages/server/sim/entity/container.go`):
/// the same eye position, reach and loaded-cell walk the container opens
/// use, but only a workbench hit settles. The hit stages one atomic
/// compound: the widened grid, the anchor naming the hit block on the
/// runtime lane, and the cleared container lease. A furnace, chest, other
/// block, miss or uncertifiable cell refuses without effect and stays the
/// container provider's or nobody's hit. Re-opening an already bench-sized
/// grid re-anchors to the new hit and clears the lease again without
/// touching the grid or size.
pub(crate) fn settle_bench_open(
    ctx: &mut TickContext<'_>,
    envelope: &CommandEnvelope,
    look: LookAngles,
) -> OpenOutcome {
    let Some(session) = SessionKey::from_raw(envelope.session()) else {
        return OpenOutcome::Refused;
    };
    let actor = ActorKey::Player(session);
    let record = match ctx.read().actor(actor) {
        Some(record) if record.lifecycle == ActorLifecycle::Active => record.clone(),
        _ => return OpenOutcome::Refused,
    };
    if record.dimension != Dimension::OVERWORLD {
        return OpenOutcome::Refused;
    }
    let before = match ctx.read().inventory(actor) {
        Some(inventory) => *inventory,
        None => return OpenOutcome::Refused,
    };
    let hit = {
        let view = ctx.read();
        let Some(environment) = view.environment() else {
            return OpenOutcome::Refused;
        };
        let position = record.motion.position().get();
        let eye = [
            position[0],
            position[1] + environment.tunables.eye_height(),
            position[2],
        ];
        let direction = look_direction(look.yaw(), look.pitch());
        match cast_ray(
            &view,
            record.dimension,
            eye,
            direction,
            environment.tunables.interaction_reach(),
        ) {
            Ok(Some(hit)) => hit,
            _ => return OpenOutcome::Refused,
        }
    };
    if hit.observed.block != WORKBENCH_BLOCK {
        return OpenOutcome::Refused;
    }
    // Classify the target first; current held sneaking refuses without
    // ending a prior view or recording a successful grid-open intent.
    if ctx
        .read()
        .runtime(actor)
        .and_then(|runtime| runtime.controls)
        .is_some_and(|controls| controls.actions().sneaking)
    {
        return OpenOutcome::Refused;
    }
    let mut after = before;
    after.crafting_size = CraftingSize::Workbench;
    let mut runtime = match anchor_base(&ctx.read(), actor) {
        Some(runtime) => runtime,
        None => return OpenOutcome::Refused,
    };
    match &mut runtime.aux {
        ActorAux::Player { workbench, .. } => *workbench = Some(hit.observed.pos),
        _ => return OpenOutcome::Refused,
    }
    let mut effects = Vec::with_capacity(3);
    if after != before {
        let patch = match InventoryPatch::try_new(actor, before, after) {
            Ok(patch) => patch,
            Err(_) => return OpenOutcome::Refused,
        };
        effects.push(RuleEffect::Inventory(patch));
    }
    let anchor_settled = match ctx.read().runtime(actor) {
        Some(staged) => match &staged.aux {
            ActorAux::Player { workbench, .. } => *workbench == Some(hit.observed.pos),
            _ => false,
        },
        None => false,
    };
    if !anchor_settled {
        effects.push(RuleEffect::Runtime(runtime));
    }
    if ctx.read().viewer(session).is_some() {
        effects.push(RuleEffect::Viewer {
            session,
            view: None,
        });
    }
    if effects.is_empty() {
        // A successful reopen restates the grid even when its anchor is unchanged.
        ctx.record_crafting_publication_dirty(session);
        return OpenOutcome::Carried;
    }
    match ctx.stage(RuleEffect::Compound(effects)) {
        Ok(()) => {
            ctx.record_crafting_publication_dirty(session);
            OpenOutcome::Applied
        }
        Err(_) => OpenOutcome::Refused,
    }
}

/// Neutral runtime base for the anchor lane: the staged record wins, and a
/// missing record synthesizes neutral transients from the actor itself, the
/// same fallback the sleep entry and the survival merge use instead of
/// refusing a normal sequencing.
fn anchor_base(view: &AuthorityReadView<'_>, actor: ActorKey) -> Option<ActorRuntime> {
    if let Some(staged) = view.runtime(actor) {
        return Some(staged.clone());
    }
    let record = view.actor(actor)?;
    let ActorBody::Player(save) = &record.body else {
        return None;
    };
    Some(ActorRuntime {
        key: record.key,
        controls: None,
        has_view: false,
        reset: false,
        attack_cooldown: 0,
        hurt_cooldown: 0,
        burn_cooldown: 0,
        oxygen: record.survival.oxygen(),
        peak_y: record.motion.position().get()[1],
        exhaustion_milli: u32::from(save.exhaustion_milli),
        saturation_milli: u32::from(save.saturation_milli),
        since_damage_ticks: 0,
        drown_ticks: 0,
        starvation_ticks: 0,
        eating: None,
        bow: None,
        path: None,
        aux: ActorAux::Player {
            respawn: None,
            workbench: None,
        },
    })
}

/// Reports whether the staged anchor still holds, the `workbenchAnchorValid`
/// row (`packages/server/sim/entity/crafting.go`): the anchor chunk is
/// Ready, the anchor cell still holds a workbench, and the eyes stay within
/// `interaction_reach` of the block center. The lookup runs in the actor's
/// current dimension, so a dimension move closes unless the same coordinates
/// hold a workbench there too. A missing observation closes rather than
/// vetoing the check, matching the interaction missing-cell philosophy.
fn anchor_valid(
    view: &AuthorityReadView<'_>,
    dimension: Dimension,
    eye: [f32; 3],
    reach: f32,
    anchor: BlockPos,
) -> bool {
    let key = ChunkKey {
        dimension,
        pos: ChunkPos::new(anchor.x() >> 4, anchor.z() >> 4),
    };
    if !view.ready_chunk(key) {
        return false;
    }
    let Some(observed) = view.observation(dimension, anchor) else {
        return false;
    };
    if observed.block != WORKBENCH_BLOCK {
        return false;
    }
    let center = [
        anchor.x() as f32 + 0.5,
        anchor.y() as f32 + 0.5,
        anchor.z() as f32 + 0.5,
    ];
    let squared =
        (center[0] - eye[0]).powi(2) + (center[1] - eye[1]).powi(2) + (center[2] - eye[2]).powi(2);
    squared <= reach * reach
}

/// Closes one bench-size grid: the extended cells `4..8` reclaim into the
/// pack through the pickup order and the grid returns to the personal size,
/// the exact `closeWorkbench` row over `repackCraftingSlots`
/// (`packages/server/sim/entity/crafting.go`). A cell that cannot fully
/// credit preserves the whole view and propagates a hard invariant failure.
/// The failed-tick owner fences publication and capture instead of allowing
/// the automatic lifecycle to continue with a stranded grid.
fn close_bench(ctx: &mut TickContext<'_>, actor: ActorKey) -> Result<(), ServerError> {
    let failure = ServerError::Internal {
        invariant: "automatic workbench repack",
    };
    let before = ctx.read().inventory(actor).copied().ok_or(failure)?;
    if before.crafting_size != CraftingSize::Workbench {
        return Err(failure);
    }
    let after = closed_inventory(before).ok_or(failure)?;
    stage_patch(ctx, actor, before, after).map_err(|_| failure)?;
    // Source close marks both owner records even when the pack is unchanged.
    if let ActorKey::Player(session) = actor {
        ctx.record_crafting_command_publication_dirty(session);
    }
    Ok(())
}

/// Previews only the extended bench cells before returning to a personal grid.
/// A failed credit leaves the caller's record unchanged.
pub(crate) fn closed_inventory(before: InventoryRecord) -> Option<InventoryRecord> {
    if before.crafting_size != CraftingSize::Workbench {
        return Some(before);
    }
    let mut slots = before.slots;
    for cell in PERSONAL_GRID_EXTENT..CRAFTING_GRID_SLOTS {
        let held = before.crafting[cell];
        if held.item == ITEM_NONE {
            continue;
        }
        let (next, leftover) = add_stack(&slots, held);
        if leftover.count != 0 {
            return None;
        }
        slots = next;
    }
    let mut after = before;
    after.slots = slots;
    for cell in PERSONAL_GRID_EXTENT..CRAFTING_GRID_SLOTS {
        after.crafting[cell] = ItemStack::default();
    }
    after.crafting_size = CraftingSize::Personal;
    Some(after)
}

/// Recovers all nine grid cells into the pack and returns the grid to the
/// personal size (`repackCraftingAll` in
/// `packages/server/sim/entity/crafting.go`). The rehearsal runs first
/// through the shared repack invariant, then the credit loop replays the
/// same cell order through the pickup order; any leftover refuses the whole
/// recovery with no partial credit, so the caller keeps every cell. The
/// death settlement consumes this helper before its per-slot drop walk. Not
/// yet staged by any provider; the allow marks the landing until the death
/// node stages its first compound.
#[allow(dead_code)]
pub(crate) fn repack_all(before: InventoryRecord) -> Option<InventoryRecord> {
    if !can_repack(&before.slots, &before.crafting) {
        return None;
    }
    let mut slots = before.slots;
    for cell in 0..CRAFTING_GRID_SLOTS {
        let held = before.crafting[cell];
        if held.item == ITEM_NONE {
            continue;
        }
        let (next, leftover) = add_stack(&slots, held);
        if leftover.count != 0 {
            return None;
        }
        slots = next;
    }
    let mut after = before;
    after.slots = slots;
    after.crafting = [ItemStack::default(); CRAFTING_GRID_SLOTS];
    after.crafting_size = CraftingSize::Personal;
    Some(after)
}

struct RayHit {
    observed: BlockObservation,
}

/// Walks the authoritative ray through the certified observations up to the
/// reach and reports the first target cell; an unobserved cell refuses the
/// whole walk. Cells the shared `target_block` classifier passes over continue
/// the walk. The shared open-surface row the container provider mirrors from
/// the Go loaded-chunk walk.
fn cast_ray(
    view: &AuthorityReadView<'_>,
    dimension: Dimension,
    origin: [f32; 3],
    direction: [f32; 3],
    reach: f32,
) -> Result<Option<RayHit>, ServerError> {
    const REFUSAL: ServerError = ServerError::InvalidInput { field: "crafting" };
    let normalized = normalized_direction(direction).ok_or(REFUSAL)?;
    let mut cursor = RayCursor::try_new(Ray {
        origin,
        direction: normalized,
        maximum: reach,
    })
    .map_err(|_| REFUSAL)?;
    loop {
        let batch = NativeRaycast.next_batch(&mut cursor).map_err(|_| REFUSAL)?;
        for record in batch.records() {
            let cell =
                mornlea_domain::BlockPos::new(record.cell[0], record.cell[1], record.cell[2]);
            match view.observation(dimension, cell) {
                None => return Err(REFUSAL),
                Some(observed) if !target_block(view, dimension, cell, observed.block) => {}
                Some(observed) => return Ok(Some(RayHit { observed })),
            }
        }
        if batch.is_done() {
            return Ok(None);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mornlea_domain::CraftingSize;

    fn stack(item: u16, count: u8) -> ItemStack {
        ItemStack {
            item,
            count,
            durability: 0,
        }
    }

    /// Full-grid recovery credits every cell in slot order and returns the
    /// grid to the personal size, so death settlement can drop the credited
    /// pack through the ordinary per-slot discipline.
    #[test]
    fn repack_all_credits_nine_cells_and_restores_personal() {
        let mut before = InventoryRecord::empty();
        before.crafting_size = CraftingSize::Workbench;
        before.slots[0] = stack(35, 60);
        before.crafting[0] = stack(35, 10);
        before.crafting[4] = stack(37, 5);
        before.crafting[8] = stack(35, 1);
        let after = repack_all(before).expect("roomy pack must absorb the grid");
        assert_eq!(after.crafting_size, CraftingSize::Personal);
        assert_eq!(after.crafting, [ItemStack::default(); CRAFTING_GRID_SLOTS]);
        assert_eq!(after.slots[0], stack(35, 64));
        assert_eq!(after.slots[1], stack(35, 7));
        assert_eq!(after.slots[2], stack(37, 5));
    }

    /// A pack with no room anywhere refuses the whole recovery instead of
    /// crediting part of the grid: the caller keeps every cell and retries
    /// on a later tick.
    #[test]
    fn repack_all_refuses_whole_when_pack_is_full() {
        let mut before = InventoryRecord::empty();
        before.crafting_size = CraftingSize::Workbench;
        before.slots = [stack(1, 64); INVENTORY_SLOTS];
        before.crafting[3] = stack(1, 1);
        assert!(repack_all(before).is_none());
    }

    /// An empty grid always repacks: only the size returns to personal while
    /// the pack passes through untouched.
    #[test]
    fn repack_all_empty_grid_only_resets_size() {
        let mut before = InventoryRecord::empty();
        before.crafting_size = CraftingSize::Workbench;
        before.slots[9] = stack(35, 3);
        let after = repack_all(before).expect("empty grid always repacks");
        assert_eq!(after.crafting_size, CraftingSize::Personal);
        assert_eq!(after.slots[9], stack(35, 3));
    }
}
