//! Pure immutable reads over the frozen pathfinding grid.
//!
//! The grid owns its block snapshot: construction copies every block once and
//! validates shape and revision identity up front, so these readers never meet
//! a half-built grid. They borrow the grid, copy out small values and keep no
//! state, which is what lets later search work treat any grid as an immutable
//! snapshot — a repeated read of the same coordinates always agrees with the
//! first, and no read can ever observe a torn or resized block box.
//!
//! Indexing order matches the oracle exactly. The snapshot is filled outer X,
//! middle Z, inner Y, so the address of a local cell is
//! `((x * size_z) + z) * size_y + y`: Y runs fastest. An X-fast or Z-fast
//! layout would place different markers at the same coordinates, and the
//! migration test pins coordinates where they disagree.
//!
//! Constructor validation always precedes any block read: a rejected shape
//! never yields a grid, so readers can rely on the origin, the sizes and the
//! block length agreeing without re-checking them.

use crate::native::contracts::pathfind::{PathCell, PathError, PathGrid, PathResult, PathScratch};

/// Address of a local cell in Y-fast order, or `None` when a local coordinate
/// lies outside the sizes or the address arithmetic would overflow.
///
/// The bounds check comes first so oversized locals refuse without touching
/// arithmetic; every multiply and add after that is checked, so astronomic
/// sizes refuse instead of wrapping to a wrong slot.
pub fn flat_index(size: [u32; 3], x: u32, y: u32, z: u32) -> Option<usize> {
    if x >= size[0] || y >= size[1] || z >= size[2] {
        return None;
    }
    let column = (x as usize)
        .checked_mul(size[2] as usize)?
        .checked_add(z as usize)?;
    column
        .checked_mul(size[1] as usize)?
        .checked_add(y as usize)
}

/// Block id stored at world coordinates, or `None` outside the grid.
///
/// Local coordinates come from widened subtraction so an origin near the
/// signed extremes cannot wrap; anything outside the box refuses instead of
/// reading out of bounds, and the lookup itself never panics.
pub fn block_at(grid: &PathGrid, x: i32, y: i32, z: i32) -> Option<u16> {
    let origin = grid.origin();
    let size = grid.size();
    let local_x = i64::from(x) - i64::from(origin.x);
    let local_y = i64::from(y) - i64::from(origin.y);
    let local_z = i64::from(z) - i64::from(origin.z);
    if local_x < 0
        || local_y < 0
        || local_z < 0
        || local_x >= i64::from(size[0])
        || local_y >= i64::from(size[1])
        || local_z >= i64::from(size[2])
    {
        return None;
    }
    let index = flat_index(size, local_x as u32, local_y as u32, local_z as u32)?;
    grid.blocks.get(index).copied()
}

/// Whether the cell at world coordinates can be occupied: in the grid and
/// admitted by the passability table.
///
/// Unknown block ids stay blocked by table default — the table answers false
/// for every id it was not built with, so a snapshot can carry ids the table
/// never heard of without opening a hole.
pub fn is_passable(grid: &PathGrid, x: i32, y: i32, z: i32) -> bool {
    match block_at(grid, x, y, z) {
        Some(id) => grid.passability.is_passable(id),
        None => false,
    }
}

/// Whether an actor can stand with feet at `cell`, following the oracle rule
/// exactly: feet and head passable, and the support cell below in the grid and
/// nonpassable.
///
/// Support below the grid edge does not count — standing needs a seen solid
/// cell, never an off-snapshot guess. Widened vertical steps keep cells at the
/// signed extremes refusing instead of wrapping.
pub fn is_standing(grid: &PathGrid, cell: PathCell) -> bool {
    if !is_passable(grid, cell.x, cell.y, cell.z) {
        return false;
    }
    let Some(head_y) = cell.y.checked_add(1) else {
        return false;
    };
    if !is_passable(grid, cell.x, head_y, cell.z) {
        return false;
    }
    let Some(support_y) = cell.y.checked_sub(1) else {
        return false;
    };
    match block_at(grid, cell.x, support_y, cell.z) {
        Some(id) => !grid.passability.is_passable(id),
        None => false,
    }
}

/// Expansion budget shared with the oracle: at most this many pops per call.
pub const MAX_EXPANSIONS: usize = 4096;

/// Cost of each transition kind, matching the oracle movement prices.
const COST_FLAT: u32 = 1;
const COST_JUMP: u32 = 2;
const COST_FALL: u32 = 1;
const COST_GAP: u32 = 2;

/// Search state per cell: unseen, queued in the heap, or closed after a pop.
#[derive(Clone, Copy, PartialEq, Eq)]
enum CellState {
    Unseen,
    Open,
    Closed,
}

/// Working state for one search, allocated fresh per call.
///
/// `find_path` builds one `SearchSpace` sized by the grid cell count after
/// the scratch-capacity check admits the grid, so the vectors never exceed
/// the admitted bound. The vectors are indexed by flat cell address; the
/// generation reset holds trivially because the state is fresh. The `heap`
/// holds cell addresses ordered by `(f, insertion-ordinal)`; `position` maps
/// an address back to its heap slot for decrease-key updates that retain the
/// original ordinal.
struct SearchSpace {
    cost: Vec<u32>,
    parent: Vec<usize>,
    state: Vec<CellState>,
    generation: Vec<u32>,
    position: Vec<usize>,
    ordinal: Vec<u64>,
    heap: Vec<usize>,
    key: Vec<u64>,
    current: u32,
    insertions: u64,
}

impl SearchSpace {
    /// Builds zeroed working state for `cells` addresses.
    fn new(cells: usize) -> Self {
        Self {
            cost: vec![u32::MAX; cells],
            parent: vec![usize::MAX; cells],
            state: vec![CellState::Unseen; cells],
            generation: vec![0; cells],
            position: vec![usize::MAX; cells],
            ordinal: vec![u64::MAX; cells],
            heap: Vec::new(),
            key: vec![u64::MAX; cells],
            current: 0,
            insertions: 0,
        }
    }

    /// Resets every call, including after a failure: the generation bump
    /// retires all prior marks, the heap is emptied and the insertion counter
    /// restarts so ordinals always reflect this call's emission order.
    fn reset(&mut self) {
        self.current = self.current.wrapping_add(1);
        if self.current == 0 {
            self.generation.fill(0);
            self.state.fill(CellState::Unseen);
            self.current = 1;
        }
        self.heap.clear();
        self.insertions = 0;
    }

    /// Marks an address live for this generation, clearing stale marks left
    /// by an older generation on first touch.
    fn touch(&mut self, index: usize) {
        if self.generation[index] != self.current {
            self.generation[index] = self.current;
            self.state[index] = CellState::Unseen;
            self.cost[index] = u32::MAX;
            self.parent[index] = usize::MAX;
            self.position[index] = usize::MAX;
            self.ordinal[index] = u64::MAX;
            self.key[index] = u64::MAX;
        }
    }
}

/// Address of a world cell inside the grid, or `None` outside the box.
///
/// Local coordinates come from widened subtraction so an origin near the
/// signed extremes cannot wrap; the shared `flat_index` then bounds-checks
/// and guards the address arithmetic.
fn cell_index(grid: &PathGrid, cell: PathCell) -> Option<usize> {
    let origin = grid.origin();
    let size = grid.size();
    let local_x = i64::from(cell.x) - i64::from(origin.x);
    let local_y = i64::from(cell.y) - i64::from(origin.y);
    let local_z = i64::from(cell.z) - i64::from(origin.z);
    if local_x < 0 || local_y < 0 || local_z < 0 {
        return None;
    }
    if local_x >= i64::from(size[0])
        || local_y >= i64::from(size[1])
        || local_z >= i64::from(size[2])
    {
        return None;
    }
    flat_index(size, local_x as u32, local_y as u32, local_z as u32)
}

/// World coordinates of a flat address, by inverting the Y-fast layout.
///
/// The address is known live for this call, so the division chain stays in
/// range and every component lands back inside the grid box.
fn index_cell(grid: &PathGrid, index: usize) -> PathCell {
    let origin = grid.origin();
    let size = grid.size();
    let stride_y = size[1] as usize;
    let stride_z = size[2] as usize;
    let column = index / stride_y;
    let y = (index % stride_y) as i32;
    let x = (column / stride_z) as i32;
    let z = (column % stride_z) as i32;
    PathCell {
        x: origin.x + x,
        y: origin.y + y,
        z: origin.z + z,
    }
}

/// Horizontal Manhattan distance to the goal, ignoring height.
///
/// The differences are widened before the absolute value so cells at opposite
/// signed extremes cannot overflow; the sum is checked, matching the oracle
/// heuristic exactly while refusing astronomic spans instead of wrapping.
fn heuristic(cell: PathCell, goal: PathCell) -> Option<u64> {
    let dx = (i64::from(cell.x) - i64::from(goal.x)).unsigned_abs();
    let dz = (i64::from(cell.z) - i64::from(goal.z)).unsigned_abs();
    dx.checked_add(dz)
}

/// Key of a queued address: `g + heuristic`, computed in widened arithmetic
/// so a large cost plus a large distance refuses instead of wrapping.
fn key_of(space: &SearchSpace, grid: &PathGrid, goal: PathCell, index: usize) -> Option<u64> {
    let distance = heuristic(index_cell(grid, index), goal)?;
    u64::from(space.cost[index]).checked_add(distance)
}

/// Orders two heap entries by `(f, insertion-ordinal)`: lower key first,
/// earlier emission winning every tie, exactly like the oracle pop order.
fn heap_less(space: &SearchSpace, a: usize, b: usize) -> bool {
    if space.key[a] != space.key[b] {
        return space.key[a] < space.key[b];
    }
    space.ordinal[a] < space.ordinal[b]
}

/// Moves the entry at `slot` toward the root while the heap order is
/// violated; used for both insertion and decrease-key updates.
fn sift_up(space: &mut SearchSpace, mut slot: usize) {
    while slot > 0 {
        let parent = (slot - 1) / 2;
        if heap_less(space, space.heap[slot], space.heap[parent]) {
            space.heap.swap(slot, parent);
            space.position[space.heap[slot]] = slot;
            space.position[space.heap[parent]] = parent;
            slot = parent;
        } else {
            break;
        }
    }
}

/// Removes and returns the minimum heap entry, or `None` when empty.
fn heap_pop(space: &mut SearchSpace) -> Option<usize> {
    let top = *space.heap.first()?;
    let last = space.heap.pop().expect("nonempty heap has a last entry");
    if !space.heap.is_empty() {
        space.heap[0] = last;
        space.position[last] = 0;
        let mut slot = 0;
        loop {
            let left = slot * 2 + 1;
            let right = slot * 2 + 2;
            let mut smallest = slot;
            if left < space.heap.len() && heap_less(space, space.heap[left], space.heap[smallest]) {
                smallest = left;
            }
            if right < space.heap.len() && heap_less(space, space.heap[right], space.heap[smallest])
            {
                smallest = right;
            }
            if smallest == slot {
                break;
            }
            space.heap.swap(slot, smallest);
            space.position[space.heap[slot]] = slot;
            space.position[space.heap[smallest]] = smallest;
            slot = smallest;
        }
    }
    space.position[top] = usize::MAX;
    Some(top)
}

/// Queues a fresh address with its cost, parent and next insertion ordinal.
fn heap_insert(
    space: &mut SearchSpace,
    grid: &PathGrid,
    goal: PathCell,
    index: usize,
    cost: u32,
    parent: usize,
) -> Result<(), PathError> {
    space.touch(index);
    space.cost[index] = cost;
    space.parent[index] = parent;
    space.state[index] = CellState::Open;
    space.ordinal[index] = space.insertions;
    space.insertions = space
        .insertions
        .checked_add(1)
        .ok_or(PathError::Allocation)?;
    space.key[index] = key_of(space, grid, goal, index).ok_or(PathError::Allocation)?;
    space.heap.push(index);
    let slot = space.heap.len() - 1;
    space.position[index] = slot;
    sift_up(space, slot);
    Ok(())
}

/// Lowers the cost and parent of a queued address, retaining its original
/// insertion ordinal so earlier emission keeps winning later ties.
fn heap_decrease(
    space: &mut SearchSpace,
    grid: &PathGrid,
    goal: PathCell,
    index: usize,
    cost: u32,
    parent: usize,
) -> Result<(), PathError> {
    space.cost[index] = cost;
    space.parent[index] = parent;
    space.key[index] = key_of(space, grid, goal, index).ok_or(PathError::Allocation)?;
    let slot = space.position[index];
    sift_up(space, slot);
    Ok(())
}

/// Offers one transition to the search: unseen addresses are inserted, open
/// addresses with a strictly better candidate are decreased, and every other
/// outcome retains the original parent and ordinal.
///
/// Closed addresses never reopen, and equal-cost candidates keep the earlier
/// parent, which is what freezes the oracle tie choices.
fn offer(
    space: &mut SearchSpace,
    grid: &PathGrid,
    goal: PathCell,
    target: PathCell,
    candidate: u32,
    parent: usize,
) -> Result<(), PathError> {
    let Some(index) = cell_index(grid, target) else {
        return Ok(());
    };
    space.touch(index);
    match space.state[index] {
        CellState::Closed => {}
        CellState::Unseen => {
            heap_insert(space, grid, goal, index, candidate, parent)?;
        }
        CellState::Open => {
            if candidate < space.cost[index] {
                heap_decrease(space, grid, goal, index, candidate, parent)?;
            }
        }
    }
    Ok(())
}

/// Emits the four independent transitions of one direction in oracle order:
/// flat, jump-up, fall-one, gap-two. Each check stands alone — a flat step
/// never suppresses the gap step of the same direction — and each carries
/// its own cost.
///
/// Jump clearance reads the headroom cell above the current feet; fall reads
/// the lowered neighbor; gap reads the intermediate feet and head. All three
/// refuse through the grid readers when their coordinates leave the box.
/// Step context for one pop: the cell, its heap address, goal and cost.
///
/// Grouping keeps the per-direction emission under the argument-count lint
/// without changing the oracle order or costs.
struct StepContext {
    cell: PathCell,
    goal: PathCell,
    parent: usize,
    node_cost: u32,
}

fn expand_direction(
    space: &mut SearchSpace,
    grid: &PathGrid,
    step: &StepContext,
    dx: i32,
    dz: i32,
) -> Result<(), PathError> {
    let cell = step.cell;
    let goal = step.goal;
    let parent = step.parent;
    let node_cost = step.node_cost;
    let flat = PathCell {
        x: cell.x + dx,
        y: cell.y,
        z: cell.z + dz,
    };
    if is_standing(grid, flat) {
        let candidate = node_cost
            .checked_add(COST_FLAT)
            .ok_or(PathError::Allocation)?;
        offer(space, grid, goal, flat, candidate, parent)?;
    }
    if let Some(clear_y) = cell.y.checked_add(2) {
        let jump = PathCell {
            x: cell.x + dx,
            y: cell.y + 1,
            z: cell.z + dz,
        };
        if is_standing(grid, jump) && is_passable(grid, cell.x, clear_y, cell.z) {
            let candidate = node_cost
                .checked_add(COST_JUMP)
                .ok_or(PathError::Allocation)?;
            offer(space, grid, goal, jump, candidate, parent)?;
        }
    }
    if let Some(low_y) = cell.y.checked_sub(1) {
        let drop = PathCell {
            x: cell.x + dx,
            y: low_y,
            z: cell.z + dz,
        };
        if is_standing(grid, drop) {
            let candidate = node_cost
                .checked_add(COST_FALL)
                .ok_or(PathError::Allocation)?;
            offer(space, grid, goal, drop, candidate, parent)?;
        }
    }
    let gap = PathCell {
        x: cell.x + 2 * dx,
        y: cell.y,
        z: cell.z + 2 * dz,
    };
    if is_standing(grid, gap)
        && is_passable(grid, cell.x + dx, cell.y, cell.z + dz)
        && is_passable(grid, cell.x + dx, cell.y + 1, cell.z + dz)
    {
        let candidate = node_cost
            .checked_add(COST_GAP)
            .ok_or(PathError::Allocation)?;
        offer(space, grid, goal, gap, candidate, parent)?;
    }
    Ok(())
}

/// Reconstructs the parent chain from the goal back to the start into an
/// owned waypoint box, ordered start to goal.
///
/// The chain length is bounded by the grid cell count: a longer chain means a
/// corrupted parent link, and the call fails instead of looping.
fn reconstruct(
    space: &mut SearchSpace,
    grid: &PathGrid,
    cells: usize,
    start: usize,
    goal: usize,
) -> Result<Box<[PathCell]>, PathError> {
    let mut chain = Vec::new();
    let mut cursor = goal;
    loop {
        if chain.len() > cells {
            return Err(PathError::Allocation);
        }
        space.touch(cursor);
        chain.push(index_cell(grid, cursor));
        if cursor == start {
            break;
        }
        cursor = space.parent[cursor];
        if cursor == usize::MAX {
            return Err(PathError::Allocation);
        }
    }
    chain.reverse();
    Ok(chain.into_boxed_slice())
}

/// Runs the deterministic bounded search over an immutable grid snapshot.
///
/// Standing validation runs before the trivial start-equals-goal check, and
/// scratch capacity before any expansion: a grid whose cell count exceeds the
/// scratch capacity fails with `ScratchTooSmall`. The scratch resets on every
/// call, including failures, and the owned result never borrows from it.
pub fn find_path(
    grid: &PathGrid,
    start: PathCell,
    goal: PathCell,
    scratch: &mut PathScratch,
) -> Result<PathResult, PathError> {
    if !is_standing(grid, start) || !is_standing(grid, goal) {
        return Err(PathError::Unreachable);
    }
    let [size_x, size_y, size_z] = grid.size();
    let cells = (size_x as usize)
        .checked_mul(size_y as usize)
        .and_then(|count| count.checked_mul(size_z as usize))
        .ok_or(PathError::InvalidGrid)?;
    if cells > scratch_cells(scratch) {
        return Err(PathError::ScratchTooSmall);
    }
    let mut space = SearchSpace::new(cells);
    space.reset();
    let Some(start_index) = cell_index(grid, start) else {
        return Err(PathError::Unreachable);
    };
    let Some(goal_index) = cell_index(grid, goal) else {
        return Err(PathError::Unreachable);
    };
    if start == goal {
        let revisions: Box<[_]> = grid
            .revisions()
            .iter()
            .copied()
            .collect::<Vec<_>>()
            .into_boxed_slice();
        return Ok(PathResult::new(Box::new([start]), revisions));
    }
    // The start heuristic must be computable before insertion admits the call.
    heuristic(start, goal).ok_or(PathError::Allocation)?;
    heap_insert(&mut space, grid, goal, start_index, 0, usize::MAX)?;
    let mut expansions = 0_usize;
    loop {
        let Some(current) = heap_pop(&mut space) else {
            return Err(PathError::Unreachable);
        };
        if expansions == MAX_EXPANSIONS {
            return Err(PathError::BudgetExceeded);
        }
        space.touch(current);
        space.state[current] = CellState::Closed;
        expansions += 1;
        if current == goal_index {
            let waypoints = reconstruct(&mut space, grid, cells, start_index, goal_index)?;
            let revisions: Box<[_]> = grid
                .revisions()
                .iter()
                .copied()
                .collect::<Vec<_>>()
                .into_boxed_slice();
            return Ok(PathResult::new(waypoints, revisions));
        }
        let node_cost = space.cost[current];
        let cell = index_cell(grid, current);
        // Direction order is part of the frozen contract.
        let step = StepContext {
            cell,
            goal,
            parent: current,
            node_cost,
        };
        for (dx, dz) in [(-1, 0), (1, 0), (0, -1), (0, 1)] {
            expand_direction(&mut space, grid, &step, dx, dz)?;
        }
    }
}

/// Reads the frozen scratch capacity.
///
/// `PathScratch.cells` is crate-visible, so the search reads the bound
/// directly without touching the frozen surface.
fn scratch_cells(scratch: &PathScratch) -> usize {
    scratch.cells
}
