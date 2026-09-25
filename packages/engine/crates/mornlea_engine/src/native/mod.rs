//! Native numerical engine entry points and shared contracts.

pub mod contracts;

#[allow(dead_code)]
pub mod collision;
#[allow(dead_code)]
pub(crate) mod fluid_eval;
#[allow(dead_code)]
pub(crate) mod fluid_rescan;
#[allow(dead_code)]
pub(crate) mod lod;
#[allow(dead_code)]
pub(crate) mod mesh;
#[allow(dead_code)]
pub(crate) mod pathfind;
#[allow(dead_code)]
pub mod physics;
#[allow(dead_code)]
pub mod raycast;
#[allow(dead_code)]
pub(crate) mod tree;
pub mod world_probe;
#[allow(dead_code)]
pub mod worldgen;
