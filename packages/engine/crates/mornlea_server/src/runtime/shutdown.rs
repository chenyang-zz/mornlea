//! Runtime shutdown assembly: session teardown, tick-error handoff, and
//! Agent/MCP ports for [`crate::core::shutdown::shutdown`].
//!
//! Core owns the phase machine. This module is the production input side:
//!
//! 1. An ordinary [`crate::runtime::tick_loop::ServerTick::step`] error is
//!    handed to shutdown unchanged — never rewritten as world-changed, and
//!    never handled by failing companions one by one.
//! 2. Before the final tick, live sessions are retired so their queued
//!    commands are discarded on the input side (Go `host_shutdown.go:78`
//!    detach; Go `session_ingress.go:379-381` drops detached input). Core
//!    then sees a queue without those commands and needs no shutdown
//!    special case.
//! 3. Agent lease release and Agent/MCP close go through separate
//!    [`crate::contracts::AgentHandle`] / [`crate::contracts::McpLifecycle`]
//!    borrows from [`crate::runtime::companion::AgentServices`], matching
//!    Go `host_shutdown.go:108-153` (release first; on failure keep lease,
//!    Agent and MCP for retry; on success close in core phase order).

use crate::contracts::{
    ActorPersistence, AgentHandle, Clock, CloseReason, Deadline, FinalReducer, McpLifecycle,
    MemoryFinalizer, ServerError, ShutdownFailure, ShutdownReport, StoreHandle, WorkerLifecycle,
};
use crate::core::shutdown::{ShutdownPorts, shutdown as core_shutdown};
use crate::runtime::companion::AgentServices;
use crate::state::AuthorityState;

/// Why shutdown was entered from the ordinary tick path.
///
/// The wrapped [`ServerError`] is the exact value [`crate::runtime::tick_loop::ServerTick::step`]
/// (or the public single-tick fn) returned. Callers must not rewrite it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TickShutdownTrigger {
    error: ServerError,
}

impl TickShutdownTrigger {
    /// Builds a trigger from a tick error. The error is stored unchanged.
    pub fn from_tick_error(error: ServerError) -> Self {
        Self { error }
    }

    /// The unchanged tick error that forced shutdown.
    pub fn error(&self) -> ServerError {
        self.error
    }
}

/// Classifies one ordinary tick outcome for the host loop.
///
/// A successful report continues the loop. An error becomes a
/// [`TickShutdownTrigger`] so the host enters shutdown with that same error;
/// it is never mapped to a companion world-changed failure.
#[derive(Debug)]
pub enum TickOutcome {
    /// The tick completed.
    Continued,
    /// The tick error must enter shutdown unchanged.
    BeginShutdown(TickShutdownTrigger),
}

/// Maps a [`Result`] from `ServerTick::step` onto [`TickOutcome`].
///
/// The error arm preserves the value bit-for-bit. A mutation that swallowed
/// the error as "world changed" or dropped companions would fail the pin
/// tests in this module.
pub fn outcome_from_step<T>(result: Result<T, ServerError>) -> TickOutcome {
    match result {
        Ok(_) => TickOutcome::Continued,
        Err(error) => TickOutcome::BeginShutdown(TickShutdownTrigger::from_tick_error(error)),
    }
}

/// Retires every non-retired session on the authority.
///
/// Each retire drops that session's queued commands and staged chats (see
/// [`AuthorityState::retire`]), so the final tick never observes them. The
/// transport/runtime caller runs this before [`run_shutdown`]. No core
/// shutdown special case is required.
pub fn teardown_sessions(state: &mut AuthorityState) -> Result<usize, ServerError> {
    let keys = state.sessions_for_shutdown();
    let mut retired = 0;
    for key in keys {
        match state.retire(key, CloseReason::Shutdown) {
            Ok(()) => retired += 1,
            Err(ServerError::StaleSession { .. }) => {}
            Err(error) => return Err(error),
        }
    }
    Ok(retired)
}

/// Ports the runtime hands to [`crate::core::shutdown::shutdown`].
///
/// `agent` and `mcp` must be the split handles from
/// [`AgentServices::shutdown_handles`]; a combined close is not accepted.
pub struct RuntimeShutdownPorts<'a> {
    pub reducer: &'a mut dyn FinalReducer,
    pub workers: &'a mut dyn WorkerLifecycle,
    pub actors: &'a mut dyn ActorPersistence,
    pub memory: &'a mut dyn MemoryFinalizer,
    pub store: &'a mut dyn StoreHandle,
    pub agent: &'a mut dyn AgentHandle,
    pub mcp: &'a mut dyn McpLifecycle,
    pub clock: &'a dyn Clock,
}

/// Tears down sessions, then runs the core shutdown machine.
///
/// Order matches Go `Host.Shutdown`: detach sessions (and discard their
/// queued commands) before the world shutdown's final step. The optional
/// `tick_trigger` is recorded only so callers can assert the tick error was
/// preserved; core shutdown still reads the authority's retained failure /
/// phase for its own refusals.
pub fn run_shutdown(
    state: &mut AuthorityState,
    ports: &mut RuntimeShutdownPorts<'_>,
    deadline: Deadline,
    _tick_trigger: Option<&TickShutdownTrigger>,
) -> Result<ShutdownReport, ShutdownFailure> {
    if let Err(error) = teardown_sessions(state) {
        return Err(ShutdownFailure {
            error,
            report: state.shutdown_progress(),
        });
    }
    let mut core_ports = ShutdownPorts {
        reducer: ports.reducer,
        workers: ports.workers,
        actors: ports.actors,
        memory: ports.memory,
        store: ports.store,
        agent: ports.agent,
        mcp: ports.mcp,
        clock: ports.clock,
    };
    core_shutdown(state, &mut core_ports, deadline)
}

/// Convenience: split [`AgentServices`] into the two core shutdown ports.
pub fn agent_shutdown_ports(
    services: &mut AgentServices,
) -> (&mut dyn AgentHandle, &mut dyn McpLifecycle) {
    services.shutdown_handles()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contracts::{ServerLimits, ServerPhase, TickBudget, TransportKind};
    use crate::core::companion_runner::TaskTimeoutMinutes;
    use crate::runtime::companion_planning::tests::Rig;
    use crate::runtime::tick_loop::{ServerTick, TickStartup};
    use mornlea_domain::{Command, CompanionId, PlayerId};
    use mornlea_protocol::{LoginStart, PlayIntent, admit_login};

    fn limits() -> ServerLimits {
        ServerLimits::try_new(8, 4096, 512, 64, 64, 1_048_576).unwrap()
    }

    fn admitted(tag: u8, name: &str) -> mornlea_protocol::AdmittedLogin {
        let mut bytes = [0u8; 16];
        bytes[0] = tag;
        bytes[6] = 0x40;
        bytes[8] = 0x80;
        let id = PlayerId::try_from_bytes(bytes).unwrap();
        admit_login(
            LoginStart::decode_inbound(
                &LoginStart::new(id, name, 8).unwrap().encode().unwrap(),
            )
            .unwrap(),
        )
        .unwrap()
    }

    #[test]
    fn tick_error_enters_shutdown_trigger_unchanged() {
        let closing = ServerError::InvalidState {
            phase: ServerPhase::Closing,
        };
        match outcome_from_step::<()>(Err(closing)) {
            TickOutcome::BeginShutdown(trigger) => {
                assert_eq!(
                    trigger.error(),
                    ServerError::InvalidState {
                        phase: ServerPhase::Closing
                    }
                );
            }
            TickOutcome::Continued => panic!("error must begin shutdown"),
        }
        let failed = ServerError::Internal {
            invariant: "tick fixture",
        };
        match outcome_from_step::<()>(Err(failed)) {
            TickOutcome::BeginShutdown(trigger) => assert_eq!(trigger.error(), failed),
            TickOutcome::Continued => panic!("error must begin shutdown"),
        }
        assert!(matches!(
            outcome_from_step(Ok(())),
            TickOutcome::Continued
        ));
    }

    /// A swallow that rewrote the tick error as a benign continue (the
    /// "world-changed / drop companions" class of mistake) must go RED
    /// against the real helper.
    #[test]
    fn swallow_mutation_would_lose_the_tick_error() {
        let error = ServerError::InvalidState {
            phase: ServerPhase::Closing,
        };
        let outcome = outcome_from_step::<()>(Err(error));
        assert!(
            matches!(outcome, TickOutcome::BeginShutdown(_)),
            "production must begin shutdown"
        );
        // Mutated swallow shape (forbidden):
        let swallowed = match Err::<(), _>(error) {
            Ok(()) => TickOutcome::Continued,
            Err(_) => TickOutcome::Continued,
        };
        assert!(matches!(swallowed, TickOutcome::Continued));
        assert!(
            !matches!(outcome_from_step::<()>(Err(error)), TickOutcome::Continued),
            "tick error must not be swallowed as Continued"
        );
    }

    #[test]
    fn server_tick_closing_error_is_the_shutdown_trigger() {
        let mut state = AuthorityState::try_new(limits(), 42).unwrap();
        assert!(state.begin_close());
        let mut tick = ServerTick::new(
            TickStartup::new(TaskTimeoutMinutes::try_new(5).unwrap(), TickBudget::full()),
            Vec::<CompanionId>::new(),
        );
        let mut rig = Rig::new();
        let err = rig
            .with_ports(|ports| tick.step(&mut state, ports))
            .unwrap_err();
        let TickOutcome::BeginShutdown(trigger) = outcome_from_step::<()>(Err(err)) else {
            panic!("must begin shutdown");
        };
        assert_eq!(
            trigger.error(),
            ServerError::InvalidState {
                phase: ServerPhase::Closing
            }
        );
    }

    #[test]
    fn teardown_discards_queued_commands_without_ack() {
        let mut state = AuthorityState::try_new(limits(), 42).unwrap();
        let session = state
            .prepare(admitted(1, "Ada"), TransportKind::Memory)
            .unwrap();
        // Activate so accept admits sequenced commands.
        state.install(session, None).unwrap();
        state.activate(session).unwrap();
        let look = mornlea_domain::LookAngles::try_new(0.0, -1.4).unwrap();
        let command = Command::PlaceBlock(
            mornlea_domain::PlacementIntent::try_new(look, 0).unwrap(),
        );
        state
            .accept(
                session,
                PlayIntent::Sequenced {
                    sequence: 1,
                    command,
                },
            )
            .unwrap();
        assert_eq!(state.queued_command_len(), 1);
        assert_eq!(teardown_sessions(&mut state).unwrap(), 1);
        assert_eq!(state.queued_command_len(), 0);
        assert!(state.sessions_for_shutdown().is_empty());
        assert_eq!(
            state.session(session).map(|f| f.phase),
            Some(crate::contracts::SessionPhase::Retired)
        );
    }

    #[test]
    fn agent_services_has_no_combined_close_until() {
        // AgentServices must not offer a combined close_until that closed
        // MCP before the lease (the old Go-mismatched order). McpService
        // and SnapshotRegistry keep their own close_until.
        let text = include_str!("companion.rs");
        let mut in_impl = false;
        for (index, line) in text.lines().enumerate() {
            let trimmed = line.trim();
            if trimmed.starts_with("impl AgentServices") {
                in_impl = true;
                continue;
            }
            if in_impl && trimmed.starts_with("impl ") {
                in_impl = false;
            }
            if in_impl && trimmed.starts_with("pub fn close_until") {
                panic!(
                    "AgentServices must not expose close_until (line {}): {trimmed}",
                    index + 1
                );
            }
        }
    }

    #[test]
    fn drive_shutdown_is_gated_behind_test_support() {
        let text = std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/core/state.rs"),
        )
        .unwrap();
        let mut saw_gate = false;
        let mut pending_cfg = false;
        for line in text.lines() {
            let trimmed = line.trim();
            if trimmed == "#[cfg(any(test, feature = \"test-support\"))]" {
                pending_cfg = true;
                continue;
            }
            if pending_cfg && trimmed.starts_with("pub fn drive_shutdown") {
                saw_gate = true;
                break;
            }
            if !trimmed.is_empty() && !trimmed.starts_with("//") && !trimmed.starts_with("#[") {
                pending_cfg = false;
            }
        }
        assert!(
            saw_gate,
            "drive_shutdown must be gated with cfg(any(test, feature = \"test-support\"))"
        );
        assert!(
            text.contains("pub struct ShutdownIo"),
            "ShutdownIo must remain for tests"
        );
        // Every ShutdownIo definition must also sit behind the same cfg.
        let mut cfg_before_io = false;
        let mut io_gated = false;
        for line in text.lines() {
            let trimmed = line.trim();
            if trimmed == "#[cfg(any(test, feature = \"test-support\"))]" {
                cfg_before_io = true;
                continue;
            }
            if trimmed.starts_with("pub struct ShutdownIo") {
                io_gated = cfg_before_io;
                break;
            }
            if !trimmed.is_empty() && !trimmed.starts_with("//") && !trimmed.starts_with("#[") {
                cfg_before_io = false;
            }
        }
        assert!(io_gated, "ShutdownIo must be cfg-gated");
    }
}
