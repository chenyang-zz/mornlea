//! Native search surface for deterministic bounded pathfinding.
//!
//! The frozen contract owns the `PathfindOp` trait and its request/result
//! types; this module re-exposes the pure grid reads, the scratch-capacity
//! search entry that serves the frozen signature, and the zero-sized
//! provider. Working state lives in function-local bounded buffers sized by
//! the scratch capacity: the scratch carries only a cell count on the frozen
//! surface, so each call hosts the indexed heap in a fresh local space and
//! drops it at return, which resets trivially on every call including
//! failures. The owned result never borrows from scratch.

use crate::native::contracts::pathfind::{
    PathCell, PathError, PathGrid, PathResult, PathScratch, PathfindOp,
};

pub use crate::pathfind::{block_at, flat_index, is_passable, is_standing};

/// Runs the deterministic bounded search through caller scratch capacity.
///
/// The scratch capacity is the only frozen working-state handle: a grid whose
/// cell count exceeds it fails with `ScratchTooSmall` before any expansion.
/// The search space is a function-local buffer sized by the grid cell count,
/// so reuse needs no explicit reset and only the final owned path allocates.
pub fn find_path(
    grid: &PathGrid,
    start: PathCell,
    goal: PathCell,
    scratch: &mut PathScratch,
) -> Result<PathResult, PathError> {
    crate::pathfind::find_path(grid, start, goal, scratch)
}

/// Zero-sized native provider for deterministic bounded pathfinding.
///
/// Ownership: the provider is stateless and holds no search state. Each call
/// serves the frozen trait signature directly through the scratch-capacity
/// entry above.
pub struct NativePathfind;

impl PathfindOp for NativePathfind {
    /// Searches the immutable grid snapshot for an owned waypoint path.
    ///
    /// Standing validation runs before the trivial start-equals-goal check,
    /// then scratch capacity, then the bounded expansion: endpoint or budget
    /// failures publish no partial path and leave the owned result untouched.
    fn find(
        &self,
        grid: &PathGrid,
        start: PathCell,
        goal: PathCell,
        scratch: &mut PathScratch,
    ) -> Result<PathResult, PathError> {
        find_path(grid, start, goal, scratch)
    }
}
