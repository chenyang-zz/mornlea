//! Frozen runtime ownership boundary.
//!
//! `runtime` is the only crate module allowed to load process configuration.
//! Downstream modules (`core`, `rules`, `store`, `transport`, `agent`) receive
//! plain values after freeze and must not import this module. `companion`
//! starts configured companions and their Agent services; `companion_runner`
//! holds pending path searches between tick boundaries for the core task
//! runner; `companion_planning` assembles the companion plan loop over
//! the Agent host and the task runner. Shutdown ports and the serve tick
//! loop land in later PRs.

pub mod companion;
pub mod companion_planning;
pub mod companion_runner;
pub mod config;

pub use config::{
    AiConfig, ConfigError, ConfigPaths, ConfigWarning, LogLevel, LoggingConfig, RuntimeConfig,
};
