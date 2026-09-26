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
    let want: Vec<[i32; 2]> = grid.revisions().iter().map(|entry| entry.chunk).collect();
    assert_eq!(
        result.revisions(),
        want.as_slice(),
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
