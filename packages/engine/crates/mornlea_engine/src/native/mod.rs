//! Native numerical engine entry points and shared contracts.

pub mod contracts;

#[allow(dead_code)]
pub mod collision;
pub mod fluid_eval;
pub mod fluid_rescan;
pub mod lod;
#[allow(dead_code)]
pub(crate) mod mesh;
#[allow(dead_code)]
pub(crate) mod pathfind;
#[allow(dead_code)]
pub mod physics;
#[allow(dead_code)]
pub mod raycast;
pub mod tree;
pub mod world_probe;
#[allow(dead_code)]
pub mod worldgen;
