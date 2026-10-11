//! The registered presentation contract target: the geometry and preparation
//! port cases plus the family provider test modules reserved by the contract
//! landing.

#![allow(dead_code)]

#[path = "support/mod.rs"]
mod support;

#[path = "presentation_contract/actors.rs"]
mod actors;
#[path = "presentation_contract/actors_companion.rs"]
mod actors_companion;
#[path = "presentation_contract/actors_drop.rs"]
mod actors_drop;
#[path = "presentation_contract/actors_hostile.rs"]
mod actors_hostile;
#[path = "presentation_contract/actors_passive.rs"]
mod actors_passive;
#[path = "presentation_contract/actors_projectile.rs"]
mod actors_projectile;
#[path = "presentation_contract/actors_remote.rs"]
mod actors_remote;
#[path = "presentation_contract/audio.rs"]
mod audio;
#[path = "presentation_contract/diagnostics.rs"]
mod diagnostics;
#[path = "presentation_contract/frame.rs"]
mod frame;
#[path = "presentation_contract/geometry.rs"]
mod geometry;
#[path = "presentation_contract/inventory_container.rs"]
mod inventory_container;
#[path = "presentation_contract/inventory_core.rs"]
mod inventory_core;
#[path = "presentation_contract/inventory_crafting.rs"]
mod inventory_crafting;
#[path = "presentation_contract/inventory_furnace.rs"]
mod inventory_furnace;
#[path = "presentation_contract/inventory_ui.rs"]
mod inventory_ui;
#[path = "presentation_contract/lod.rs"]
mod lod;
#[path = "presentation_contract/player_view.rs"]
mod player_view;
#[path = "presentation_contract/preparation.rs"]
mod preparation;
#[path = "presentation_contract/preparation_port.rs"]
mod preparation_port;
#[path = "presentation_contract/terrain.rs"]
mod terrain;
#[path = "presentation_contract/world_chat.rs"]
mod world_chat;
#[path = "presentation_contract/world_environment.rs"]
mod world_environment;
#[path = "presentation_contract/world_prompt.rs"]
mod world_prompt;
#[path = "presentation_contract/world_survival.rs"]
mod world_survival;
#[path = "presentation_contract/world_task.rs"]
mod world_task;
#[path = "presentation_contract/world_ui.rs"]
mod world_ui;
