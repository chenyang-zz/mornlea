//! Frozen runtime ownership boundary.
//!
//! `runtime` is the only crate module allowed to load process configuration.
//! Downstream modules (`core`, `rules`, `store`, `transport`, `agent`) receive
//! plain values after freeze and must not import this module. `companion`
//! starts configured companions and their Agent services; `companion_runner`
//! holds pending path searches between tick boundaries for the core task
//! runner; `companion_planning` assembles the companion plan loop over
//! the Agent host and the task runner; `tick_loop` composes one production
//! tick over the plan loop and the authority; `shutdown` tears sessions
//! down and hands tick errors into the core shutdown machine. `host_tick` paces the production loop at 20 TPS; `trusted_observer` is the host Attach/center/close surface.

pub mod companion;
pub mod host_tick;
pub mod serve;
pub mod companion_planning;
pub mod companion_runner;
pub mod config;
pub mod shutdown;
pub mod tick_loop;
pub mod trusted_observer;

pub use config::{
    AiConfig, ConfigError, ConfigPaths, ConfigWarning, LogLevel, LoggingConfig, RuntimeConfig,
};

pub use trusted_observer::{ObserverEndpoint, TrustedObserverHost};
