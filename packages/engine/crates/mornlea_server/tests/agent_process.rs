//! Rust to actual Python Agent integration: the tests own the helper child.
//!
//! Production server code never launches Python; only these tests spawn the
//! read-only helper beside the real gateway, planner, dialogue store, lease
//! state, and model SDK.

#[path = "agent_process/integration.rs"]
mod integration;
#[path = "agent_process/process.rs"]
mod process;
