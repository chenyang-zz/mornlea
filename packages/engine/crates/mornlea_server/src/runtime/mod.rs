//! Frozen runtime ownership boundary.
//!
//! `runtime` is the only crate module allowed to load process configuration.
//! Downstream modules (`core`, `rules`, `store`, `transport`, `agent`) receive
//! plain values after freeze and must not import this module. Companion host
//! assembly, shutdown ports, and the serve tick loop land in later PRs.

pub mod config;

pub use config::{
    AiConfig, ConfigError, ConfigPaths, ConfigWarning, LogLevel, LoggingConfig, RuntimeConfig,
};
