//! Deterministic bounded search migration: the typed search must preserve
//! the Go oracle expansion, tie, cost and budget choices exactly.
//!
//! Every waypoint sequence below is the Go `FindPath` observation for the
//! same terrain (see the matching scene in the runtime oracle search test),
//! never a handcrafted golden. A real path mismatch reports the first
//! differing waypoint so an ordering drift is attributable to one decision.
//! Fixture origins stay near zero, far from the signed extremes, to avoid the
//! ruled integer-origin divergence edge.

use mornlea_engine::native::contracts::{
    PathBlockTable, PathCell, PathError, PathGrid, PathRevision, PathScratch,
};
use mornlea_engine::native::pathfind::{NativePathfind, find_path};
use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;

thread_local! {
    // Counting is scoped to one test thread so parallel family tests are excluded.
    static SEARCH_ALLOCATIONS: Cell<Option<usize>> = const { Cell::new(None) };
    static SEARCH_FAIL_AFTER: Cell<Option<usize>> = const { Cell::new(None) };
}

struct SearchCountingAllocator;

fn record_search_allocation() {
    let _ = SEARCH_ALLOCATIONS.try_with(|count| {
        if let Some(value) = count.get() {
            count.set(Some(value + 1));
        }
    });
}

fn fail_search_allocation() -> bool {
    SEARCH_FAIL_AFTER
        .try_with(|remaining| match remaining.get() {
            Some(0) => {
                remaining.set(None);
                true
            }
            Some(count) => {
                remaining.set(Some(count - 1));
                false
            }
            None => false,
        })
        .unwrap_or(false)
}

unsafe impl GlobalAlloc for SearchCountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        record_search_allocation();
        if fail_search_allocation() {
            return std::ptr::null_mut();
        }
        unsafe { System.alloc(layout) }
    }

    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        unsafe { System.dealloc(pointer, layout) }
    }

    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        record_search_allocation();
        if fail_search_allocation() {
            return std::ptr::null_mut();
        }
        unsafe { System.realloc(pointer, layout, size) }
    }
}

#[global_allocator]
static SEARCH_ALLOCATOR: SearchCountingAllocator = SearchCountingAllocator;

/// Passable air id shared with the Go oracle table.
const AIR: u16 = 0;
/// Solid stone id shared with the Go oracle table.
const STONE: u16 = 2;

/// Expansion budget shared with the Go oracle.
const BUDGET: usize = 4096;

fn cell(x: i32, y: i32, z: i32) -> PathCell {
    PathCell { x, y, z }
}

fn cells(points: &[(i32, i32, i32)]) -> Vec<PathCell> {
    points.iter().map(|&(x, y, z)| cell(x, y, z)).collect()
}

fn air_table() -> PathBlockTable {
    PathBlockTable::from_passable_ids(&[AIR]).expect("air-only table must build")
}

/// Builds a grid from a block writer over an origin box, mirroring the Go
/// `NewPathGrid` fetch loop cell by cell in Y-fast order.
fn build(
    origin: PathCell,
    size: [u32; 3],
    write: &dyn Fn(i32, i32, i32) -> u16,
    revisions: Vec<PathRevision>,
) -> PathGrid {
    let total = size[0] as usize * size[1] as usize * size[2] as usize;
    let mut blocks = Vec::with_capacity(total);
    for x in 0..size[0] as i32 {
        for z in 0..size[2] as i32 {
            for y in 0..size[1] as i32 {
                blocks.push(write(origin.x + x, origin.y + y, origin.z + z));
            }
        }
    }
    PathGrid::try_new(
        origin,
        size,
        blocks.into_boxed_slice(),
        air_table(),
        revisions,
    )
    .expect("search fixture grid must build")
}

/// Flat floor fixture: solid floor at `floor_y`, air elsewhere.
fn flat_floor(origin: PathCell, size: [u32; 3], floor_y: i32) -> PathGrid {
    build(
        origin,
        size,
        &|_, y, _| if y == floor_y { STONE } else { AIR },
        Vec::new(),
    )
}

fn scratch_for(grid: &PathGrid) -> PathScratch {
    let [sx, sy, sz] = grid.size();
    let cells = sx as usize * sy as usize * sz as usize;
    PathScratch::try_with_capacity(cells).expect("scratch must fit the fixture grid")
}

fn search(grid: &PathGrid, start: PathCell, goal: PathCell) -> Result<Vec<PathCell>, PathError> {
    let mut scratch = scratch_for(grid);
    find_path(grid, start, goal, &mut scratch).map(|result| result.waypoints().to_vec())
}

/// Compares two ordered waypoint sequences and reports the first differing
/// waypoint, so a real mismatch names the decision that drifted.
fn assert_waypoints(got: &[PathCell], want: &[PathCell], scene: &str) {
    assert_eq!(
        got.len(),
        want.len(),
        "{scene}: waypoint count = {}, want {} (got {got:?})",
        got.len(),
        want.len(),
    );
    for (index, (g, w)) in got.iter().zip(want.iter()).enumerate() {
        assert_eq!(
            g, w,
            "{scene}: first differing waypoint at index {index}: got {g:?}, want {w:?} (got {got:?})"
        );
    }
}

fn assert_revisions_match_grid(
    result: &mornlea_engine::native::contracts::PathResult,
    grid: &PathGrid,
    scene: &str,
) {
    let want = grid.revisions();
    assert_eq!(
        result.revisions(),
        want,
        "{scene}: normalized revision identity must flow into the result"
    );
}

/// Straight corridor: gap jumps (cost 2 over two cells) tie with two flats,
/// and the fixed expansion order settles on the even cells.
fn corridor() {
    let grid = flat_floor(cell(0, 63, 0), [5, 3, 1], 63);
    let got = search(&grid, cell(0, 64, 0), cell(4, 64, 0)).expect("corridor must succeed");
    // Go observation for the same floor strip.
    assert_waypoints(
        &got,
        &cells(&[(0, 64, 0), (2, 64, 0), (4, 64, 0)]),
        "corridor",
    );
}

/// One-block step up: the jump transition climbs with cost 2.
fn jump_up() {
    let grid = build(
        cell(0, 63, 0),
        [2, 4, 1],
        &|x, y, _| {
            if (x == 0 && y == 63) || (x == 1 && y == 64) {
                STONE
            } else {
                AIR
            }
        },
        Vec::new(),
    );
    let got = search(&grid, cell(0, 64, 0), cell(1, 65, 0)).expect("jump must succeed");
    assert_waypoints(&got, &cells(&[(0, 64, 0), (1, 65, 0)]), "jump_up");
}

/// Blocked jump headroom: stone at the clearance cell above the start refuses
/// the only climb, and no detour exists in the one-column box.
fn jump_head_blocked() {
    let grid = build(
        cell(0, 63, 0),
        [2, 4, 1],
        &|x, y, _| {
            if (x == 0 && y == 63) || (x == 1 && y == 64) || (x == 0 && y == 66) {
                STONE
            } else {
                AIR
            }
        },
        Vec::new(),
    );
    assert_eq!(
        search(&grid, cell(0, 64, 0), cell(1, 65, 0)),
        Err(PathError::Unreachable),
        "blocked jump clearance must be unreachable, never a new category"
    );
}

/// One-cell drop: the fall transition descends with cost 1.
fn fall_one() {
    let grid = build(
        cell(0, 63, 0),
        [2, 4, 1],
        &|x, y, _| {
            if (x == 0 && y == 64) || (x == 1 && y == 63) {
                STONE
            } else {
                AIR
            }
        },
        Vec::new(),
    );
    let got = search(&grid, cell(0, 65, 0), cell(1, 64, 0)).expect("fall must succeed");
    assert_waypoints(&got, &cells(&[(0, 65, 0), (1, 64, 0)]), "fall_one");
}

/// Two-cell drop exceeds the model: only a one-cell fall is admitted.
fn fall_two_blocked() {
    let grid = build(
        cell(0, 63, 0),
        [2, 4, 1],
        &|x, y, _| {
            if (x == 0 && y == 65) || (x == 1 && y == 63) {
                STONE
            } else {
                AIR
            }
        },
        Vec::new(),
    );
    assert_eq!(
        search(&grid, cell(0, 66, 0), cell(1, 64, 0)),
        Err(PathError::Unreachable),
        "two-cell drop must be unreachable"
    );
}

/// One-cell gap: the gap transition crosses in a single step of cost 2 with
/// no support required under the middle cell.
fn gap_two() {
    let grid = build(
        cell(0, 63, 0),
        [3, 3, 1],
        &|x, y, _| {
            if y == 63 && (x == 0 || x == 2) {
                STONE
            } else {
                AIR
            }
        },
        Vec::new(),
    );
    let got = search(&grid, cell(0, 64, 0), cell(2, 64, 0)).expect("gap must succeed");
    assert_waypoints(&got, &cells(&[(0, 64, 0), (2, 64, 0)]), "gap_two");
}

/// Blocked gap headroom: stone in the middle head cell refuses the crossing.
fn gap_head_blocked() {
    let grid = build(
        cell(0, 63, 0),
        [3, 3, 1],
        &|x, y, _| {
            if (y == 63 && (x == 0 || x == 2)) || (x == 1 && y == 65) {
                STONE
            } else {
                AIR
            }
        },
        Vec::new(),
    );
    assert_eq!(
        search(&grid, cell(0, 64, 0), cell(2, 64, 0)),
        Err(PathError::Unreachable),
        "gap with blocked middle head must be unreachable"
    );
}

/// Two-cell gap exceeds the model: the crossing admits exactly one cell.
fn gap_two_cells_blocked() {
    let grid = build(
        cell(0, 63, 0),
        [4, 3, 1],
        &|x, y, _| {
            if y == 63 && (x == 0 || x == 3) {
                STONE
            } else {
                AIR
            }
        },
        Vec::new(),
    );
    assert_eq!(
        search(&grid, cell(0, 64, 0), cell(3, 64, 0)),
        Err(PathError::Unreachable),
        "two-cell gap must be unreachable"
    );
}

/// Flat and gap both eligible in one direction: the fixed order emits flat
/// first, and the tied costs keep the first-insertion waypoint choice.
fn flat_and_gap_both_eligible() {
    let grid = flat_floor(cell(0, 63, 0), [4, 3, 1], 63);
    let got = search(&grid, cell(0, 64, 0), cell(2, 64, 0)).expect("flat+gap must succeed");
    // Go emits flat `(1, 64, 0)` before gap `(2, 64, 0)` from the start, yet
    // the goal pop still selects the direct gap step; pin the observation.
    assert_waypoints(&got, &cells(&[(0, 64, 0), (2, 64, 0)]), "flat_and_gap");
}

/// Symmetric diamond: two equal shortest routes tie, and the first-insertion
/// order (negative X before positive Z) freezes the X-first path.
fn diamond_first_insertion_tie() {
    let grid = flat_floor(cell(0, 63, 0), [2, 3, 2], 63);
    let got = search(&grid, cell(0, 64, 0), cell(1, 64, 1)).expect("diamond must succeed");
    assert_waypoints(
        &got,
        &cells(&[(0, 64, 0), (1, 64, 0), (1, 64, 1)]),
        "diamond_tie",
    );
    let mirror = search(&grid, cell(1, 64, 0), cell(0, 64, 1)).expect("mirror must succeed");
    assert_waypoints(
        &mirror,
        &cells(&[(1, 64, 0), (0, 64, 0), (0, 64, 1)]),
        "diamond_mirror",
    );
}

/// Strict decrease-key: cell `(0, 64, 2)` is first discovered with a higher
/// cost and later improved, which the retained ordinal must survive. The
/// waypoint sequence is the Go observation for this terrain.
fn decrease_key() {
    let grid = build(
        cell(0, 63, 0),
        [8, 4, 4],
        &|x, y, z| {
            // Solid walking floor plus the step block east of the start and
            // the head block south of the start; the detour they force makes
            // the oracle improve one queued cell through a strict decrease.
            let floor = y == 63;
            let step = x == 1 && z == 0 && y == 64;
            let head = x == 0 && z == 1 && y == 65;
            if floor || step || head { STONE } else { AIR }
        },
        Vec::new(),
    );
    let got = search(&grid, cell(0, 64, 0), cell(7, 64, 3)).expect("detour must succeed");
    assert_waypoints(
        &got,
        &cells(&[
            (0, 64, 0),
            (1, 65, 0),
            (1, 64, 1),
            (3, 64, 1),
            (5, 64, 1),
            (7, 64, 1),
            (7, 64, 3),
        ]),
        "decrease_key",
    );
}

/// Equal-cost parent tie: the terraced climb rediscovers cells at equal cost,
/// and the original parent with the lower ordinal must be retained. The
/// waypoint sequence is the Go observation for this terrain.
fn equal_cost_parent_tie() {
    let grid = build(
        cell(0, 60, 0),
        [7, 6, 5],
        &|x, y, z| {
            if z == 2 {
                if x < 2 {
                    if y == 60 { STONE } else { AIR }
                } else if y == 61 {
                    STONE
                } else {
                    AIR
                }
            } else if y == 60 {
                STONE
            } else {
                AIR
            }
        },
        Vec::new(),
    );
    let got = search(&grid, cell(0, 61, 2), cell(6, 62, 2)).expect("terrace must succeed");
    assert_waypoints(
        &got,
        &cells(&[(0, 61, 2), (1, 61, 2), (2, 62, 2), (4, 62, 2), (6, 62, 2)]),
        "equal_cost_parent",
    );
}

/// Start equals goal on standing ground: a single waypoint after standing
/// validation, carrying the normalized revision identity.
fn start_equals_goal() {
    let grid = build(
        cell(100, 63, -50),
        [3, 3, 1],
        &|_, y, _| if y == 63 { STONE } else { AIR },
        vec![
            PathRevision {
                chunk: [3, 1],
                revision: 9,
            },
            PathRevision {
                chunk: [1, 2],
                revision: 4,
            },
        ],
    );
    let start = cell(100, 64, -50);
    let mut scratch = scratch_for(&grid);
    let result = find_path(&grid, start, start, &mut scratch).expect("start==goal must succeed");
    assert_waypoints(result.waypoints(), &[start], "start_equals_goal");
    assert_revisions_match_grid(&result, &grid, "start_equals_goal");
}

/// Non-standing endpoints fail with the unreachable category, matching the
/// oracle: no new failure variant exists for bad endpoints.
fn nonstanding_endpoints_unreachable() {
    let grid = build(
        cell(0, 63, 0),
        [3, 3, 1],
        &|x, y, _| if y == 63 && x != 0 { STONE } else { AIR },
        Vec::new(),
    );
    assert_eq!(
        search(&grid, cell(0, 64, 0), cell(2, 64, 0)),
        Err(PathError::Unreachable),
        "non-standing start must be unreachable"
    );
    assert_eq!(
        search(&grid, cell(2, 64, 0), cell(0, 64, 0)),
        Err(PathError::Unreachable),
        "non-standing goal must be unreachable"
    );
}

/// Narrow one-cell corridor forcing a linear pop order: the budget check runs
/// before the 4097th pop, so a goal at the end of a 4096-long corridor
/// succeeds while the same shape one cell longer exhausts the budget. These
/// lengths are the Go-observed boundary pair for this corridor shape.
fn budget_boundary() {
    let corridor = |length: i32| {
        build(
            cell(0, 63, -1),
            [length as u32, 3, 3],
            &|_, y, z| if y == 63 && z == 0 { STONE } else { AIR },
            Vec::new(),
        )
    };
    let inside = corridor(BUDGET as i32);
    let ok = search(&inside, cell(0, 64, 0), cell(BUDGET as i32 - 1, 64, 0));
    assert!(ok.is_ok(), "4096-long corridor goal must succeed: {ok:?}");
    let outside = corridor(BUDGET as i32 + 1);
    assert_eq!(
        search(&outside, cell(0, 64, 0), cell(BUDGET as i32, 64, 0)),
        Err(PathError::BudgetExceeded),
        "4097-long corridor goal must exhaust the budget"
    );
}

/// One scratch serves success, failure, success: the second success matches a
/// fresh-scratch run and the retained first result is unchanged.
fn scratch_reuse_success_failure_success() {
    let grid = flat_floor(cell(0, 63, 0), [5, 3, 1], 63);
    let start = cell(0, 64, 0);
    let goal = cell(4, 64, 0);
    let mut scratch = scratch_for(&grid);
    let first = find_path(&grid, start, goal, &mut scratch).expect("first must succeed");
    let first_kept = first.waypoints().to_vec();
    assert_waypoints(
        &first_kept,
        &cells(&[(0, 64, 0), (2, 64, 0), (4, 64, 0)]),
        "reuse_first",
    );
    assert_eq!(
        find_path(&grid, cell(0, 64, 0), cell(0, 63, 0), &mut scratch),
        Err(PathError::Unreachable),
        "middle failure must be unreachable"
    );
    let third = find_path(&grid, start, goal, &mut scratch).expect("reuse must succeed again");
    assert_waypoints(
        third.waypoints(),
        &cells(&[(0, 64, 0), (2, 64, 0), (4, 64, 0)]),
        "reuse_third",
    );
    assert_waypoints(
        &first_kept,
        &cells(&[(0, 64, 0), (2, 64, 0), (4, 64, 0)]),
        "retained_first",
    );
    let mut fresh = scratch_for(&grid);
    let fresh_result = find_path(&grid, start, goal, &mut fresh).expect("fresh must succeed");
    assert_waypoints(
        third.waypoints(),
        fresh_result.waypoints(),
        "reuse_matches_fresh",
    );
}

/// The provider object serves the same search through the frozen trait.
fn provider_matches_free_search() {
    let grid = flat_floor(cell(0, 63, 0), [5, 3, 1], 63);
    let start = cell(0, 64, 0);
    let goal = cell(4, 64, 0);
    let provider = NativePathfind;
    let mut scratch = PathScratch::try_with_capacity(15).expect("scratch must build");
    let result = {
        use mornlea_engine::native::contracts::PathfindOp;
        provider
            .find(&grid, start, goal, &mut scratch)
            .expect("provider must succeed")
    };
    assert_waypoints(
        result.waypoints(),
        &cells(&[(0, 64, 0), (2, 64, 0), (4, 64, 0)]),
        "provider_search",
    );
    assert_revisions_match_grid(&result, &grid, "provider_search");
}

#[test]
fn path_search_transition_matrix() {
    corridor();
    jump_up();
    jump_head_blocked();
    fall_one();
    fall_two_blocked();
    gap_two();
    gap_head_blocked();
    gap_two_cells_blocked();
    flat_and_gap_both_eligible();
    diamond_first_insertion_tie();
    decrease_key();
    equal_cost_parent_tie();
    start_equals_goal();
    nonstanding_endpoints_unreachable();
    budget_boundary();
    scratch_reuse_success_failure_success();
    provider_matches_free_search();
}

fn extreme_horizontal_path(axis: usize, maximum: bool) {
    let mut origin = cell(0, 0, 0);
    let base = if maximum { i32::MAX - 1 } else { i32::MIN };
    let (size, start, goal) = if axis == 0 {
        origin.x = base;
        ([2, 3, 1], cell(base, 1, 0), cell(base + 1, 1, 0))
    } else {
        origin.z = base;
        ([1, 3, 2], cell(0, 1, base), cell(0, 1, base + 1))
    };
    let grid = build(
        origin,
        size,
        &|_, y, _| if y == 0 { STONE } else { AIR },
        Vec::new(),
    );
    let mut scratch = scratch_for(&grid);
    let result =
        find_path(&grid, start, goal, &mut scratch).expect("boundary goal must be reachable");
    assert_eq!(result.waypoints(), &[start, goal]);
    let reverse =
        find_path(&grid, goal, start, &mut scratch).expect("reverse boundary path must exist");
    assert_eq!(reverse.waypoints(), &[goal, start]);
}

#[test]
fn path_search_x_minimum() {
    extreme_horizontal_path(0, false);
}

#[test]
fn path_search_x_maximum() {
    extreme_horizontal_path(0, true);
}

#[test]
fn path_search_z_minimum() {
    extreme_horizontal_path(2, false);
}

#[test]
fn path_search_z_maximum() {
    extreme_horizontal_path(2, true);
}

#[test]
fn path_search_warmed_unreachable_does_not_allocate() {
    let grid = build(
        cell(0, 0, 0),
        [3, 3, 1],
        &|x, y, _| if y == 0 || x == 1 { STONE } else { AIR },
        Vec::new(),
    );
    let start = cell(0, 1, 0);
    let goal = cell(2, 1, 0);
    let mut scratch = scratch_for(&grid);
    assert_eq!(
        find_path(&grid, start, goal, &mut scratch),
        Err(PathError::Unreachable)
    );
    SEARCH_ALLOCATIONS.with(|count| count.set(Some(0)));
    let result = find_path(&grid, start, goal, &mut scratch);
    let allocations = SEARCH_ALLOCATIONS.with(|count| count.replace(None).unwrap());
    assert_eq!(result, Err(PathError::Unreachable));
    assert_eq!(
        allocations, 0,
        "warmed failed searches must reuse their buffers"
    );
}

#[test]
fn path_search_result_retains_full_revisions_across_reuse() {
    let revisions = vec![
        PathRevision {
            chunk: [2, 3],
            revision: u64::MAX,
        },
        PathRevision {
            chunk: [-1, 4],
            revision: 23,
        },
        PathRevision {
            chunk: [2, 3],
            revision: u64::MAX,
        },
    ];
    let grid = build(
        cell(0, 0, 0),
        [3, 3, 1],
        &|_, y, _| if y == 0 { STONE } else { AIR },
        revisions,
    );
    let expected = [
        PathRevision {
            chunk: [-1, 4],
            revision: 23,
        },
        PathRevision {
            chunk: [2, 3],
            revision: u64::MAX,
        },
    ];
    let mut scratch = scratch_for(&grid);
    let first = find_path(&grid, cell(0, 1, 0), cell(2, 1, 0), &mut scratch).unwrap();
    assert_eq!(first.revisions(), &expected);
    assert_eq!(
        find_path(&grid, cell(0, 0, 0), cell(2, 1, 0), &mut scratch),
        Err(PathError::Unreachable)
    );
    let trivial = find_path(&grid, cell(1, 1, 0), cell(1, 1, 0), &mut scratch).unwrap();
    assert_eq!(trivial.revisions(), &expected);
    assert_eq!(trivial.waypoints(), &[cell(1, 1, 0)]);
    assert_eq!(first.revisions(), &expected);
    assert_eq!(first.waypoints(), &[cell(0, 1, 0), cell(2, 1, 0)]);
}

fn with_failed_allocation<T>(index: usize, operation: impl FnOnce() -> T) -> T {
    SEARCH_FAIL_AFTER.with(|remaining| remaining.set(Some(index)));
    let result = operation();
    SEARCH_FAIL_AFTER.with(|remaining| remaining.set(None));
    result
}

#[test]
fn path_search_allocation_failures_are_typed() {
    let table = with_failed_allocation(0, air_table_result);
    assert_eq!(table, Err(PathError::Allocation));
    SEARCH_ALLOCATIONS.with(|count| count.set(Some(0)));
    let scratch = PathScratch::try_with_capacity(9).unwrap();
    let scratch_allocations = SEARCH_ALLOCATIONS.with(|count| count.replace(None).unwrap());
    drop(scratch);
    assert!(scratch_allocations > 0);
    for index in 0..scratch_allocations {
        let result = with_failed_allocation(index, || PathScratch::try_with_capacity(9));
        assert!(matches!(result, Err(PathError::Allocation)));
    }

    let grid = build(
        cell(0, 0, 0),
        [3, 3, 1],
        &|_, y, _| if y == 0 { STONE } else { AIR },
        vec![PathRevision {
            chunk: [0, 0],
            revision: u64::MAX,
        }],
    );
    let mut scratch = scratch_for(&grid);
    let start = cell(0, 1, 0);
    for goal in [start, cell(2, 1, 0)] {
        SEARCH_ALLOCATIONS.with(|count| count.set(Some(0)));
        let first = find_path(&grid, start, goal, &mut scratch).unwrap();
        let result_allocations = SEARCH_ALLOCATIONS.with(|count| count.replace(None).unwrap());
        assert_eq!(
            result_allocations, 2,
            "only the exact path and revisions allocate"
        );
        for index in 0..result_allocations {
            let rejected =
                with_failed_allocation(index, || find_path(&grid, start, goal, &mut scratch));
            assert_eq!(rejected, Err(PathError::Allocation));
            let retry = find_path(&grid, start, goal, &mut scratch).unwrap();
            assert_eq!(retry, first);
        }
    }
}

fn air_table_result() -> Result<PathBlockTable, PathError> {
    PathBlockTable::from_passable_ids(&[AIR])
}

#[test]
fn path_search_scratch_capacity_failure_can_reuse_smaller_grid() {
    let larger = flat_floor(cell(0, 0, 0), [3, 3, 1], 0);
    let smaller = flat_floor(cell(0, 0, 0), [2, 3, 1], 0);
    let mut scratch = PathScratch::try_with_capacity(6).unwrap();
    assert_eq!(
        find_path(&larger, cell(0, 1, 0), cell(2, 1, 0), &mut scratch),
        Err(PathError::ScratchTooSmall)
    );
    let result = find_path(&smaller, cell(0, 1, 0), cell(1, 1, 0), &mut scratch).unwrap();
    assert_eq!(result.waypoints(), &[cell(0, 1, 0), cell(1, 1, 0)]);
}
