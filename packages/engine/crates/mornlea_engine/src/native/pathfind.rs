//! Native grid surface for the immutable pathfinding snapshot.
//!
//! The frozen contract keeps owning its type API; this module only re-exposes
//! the pure grid reads for external consumers such as the migration test.
//! There is no search entry here — deterministic search arrives separately.

pub use crate::pathfind::{block_at, flat_index, is_passable, is_standing};
