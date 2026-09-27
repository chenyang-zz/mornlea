//! Native search surface for deterministic bounded pathfinding.
//!
//! The frozen contract owns the `PathfindOp` trait and its request/result
//! types; this module re-exposes the pure grid reads and the zero-sized
//! provider. Caller scratch owns every bounded working buffer and resets it
//! for each call, including failures. The final result owns its waypoints
//! and complete revisions independently of scratch.

use crate::native::contracts::pathfind::{
    PathCell, PathError, PathGrid, PathResult, PathScratch, PathfindOp,
};

pub use crate::pathfind::{block_at, flat_index, is_passable, is_standing};

/// Runs the deterministic bounded search through caller scratch capacity.
///
/// A grid whose cell count exceeds scratch capacity fails with
/// `ScratchTooSmall` before any expansion.
/// Scratch keeps all working buffers across calls; only successful owned
/// result construction allocates after admission.
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
    /// Endpoint standing and scratch capacity are validated before the trivial
    /// start-equals-goal result or bounded expansion. Failures publish no path.
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
