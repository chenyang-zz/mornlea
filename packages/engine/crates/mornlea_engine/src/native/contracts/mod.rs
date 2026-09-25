//! Typed numerical contracts for Mornlea native engine operations.

pub mod collision;
pub mod fluid;
pub mod mesh;
pub mod pathfind;
pub mod physics;
pub mod raycast;
pub mod world;

pub use collision::*;
pub use fluid::*;
pub use mesh::*;
pub use pathfind::*;
pub use physics::*;
pub use raycast::*;
pub use world::*;

/// Errors returned by native numerical kernel operations.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KernelError {
    InvalidInput,
    DisplacementOutOfBounds,
    OutputTooSmall { needed: usize, available: usize },
    ScratchTooSmall { needed: usize, available: usize },
    InvalidRegistry,
    EmissionOutOfRange,
    QueueOverflow,
    MissingHalo,
    OutputInvariant,
    Allocation,
}
