//! Authoritative server crate.
//!
//! The crate owns session admission, tick staging, save ownership, and the
//! agent host boundary. GPU rendering, Godot presentation, and the Python
//! Agent process stay outside it. Imports are the foundation crates plus the
//! numerical `PhysicsTuning` value stored on a rule snapshot.

#![deny(unsafe_code)]

pub mod agent;
pub mod core;
pub mod rules;
pub mod runtime;
pub mod store;
pub mod transport;

pub use core::contracts;
pub use core::state;
