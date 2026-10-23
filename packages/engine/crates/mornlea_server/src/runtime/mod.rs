//! Frozen runtime ownership boundary.
//!
//! `runtime` is the only crate module allowed to load process configuration.
//! Downstream modules (`core`, `rules`, `store`, `transport`, `agent`) receive
//! plain values after freeze and must not import this module. `companion`
//! starts configured companions and their Agent services; `companion_runner`
//! holds pending path searches between tick boundaries for the core task
//! runner; `companion_planning` assembles the companion plan loop over
//! the Agent host and the task runner; `tick_loop` composes one production
//! tick over the plan loop and the authority. Shutdown ports and tick
//! pacing land in later PRs.

pub mod companion;
pub mod companion_planning;
pub mod companion_runner;
pub mod config;
pub mod tick_loop;

pub use config::{
    AiConfig, ConfigError, ConfigPaths, ConfigWarning, LogLevel, LoggingConfig, RuntimeConfig,
};
