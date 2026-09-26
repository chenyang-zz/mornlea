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

use crate::native::contracts::pathfind::{PathCell, PathGrid};

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
