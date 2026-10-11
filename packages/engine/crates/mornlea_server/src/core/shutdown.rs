//! Retryable shutdown: one final unpublished tick and ordered closure.
//!
//! This module owns the resumable phase machine behind the endpoint's
//! shutdown boundary. A first attempt stops admission, drains every accepted
//! command into exactly one final unpublished tick, freezes the authority and
//! the Agent lease, quiesces the workers, finalizes Agent memory, flushes the
//! save families in the fixed order players, companions, hostiles, passives,
//! world chunks and metadata, syncs the store, releases the Agent lease, and
//! only then closes the Agent, the MCP service, the store and the workers
//! before the authority flips to `Closed`.
//!
//! Every phase boundary is a durability or ownership boundary. A completed
//! phase is recorded on the authority and never replayed, so a retry resumes
//! at the failed phase: a successful final tick is never executed twice, a
//! successful store sync is never repeated, a failed release retries with the
//! same lease the freeze retained instead of closing the Agent or the MCP
//! service behind it, and a failed close never repeats the releases and
//! closes that already succeeded. The final tick performs no publication:
//! its reducer consumes accepted work through the state ports and no outbox
//! frame is appended on the shutdown path.

use std::time::{Duration, Instant};

use super::contracts::{
    ActorPersistence, AgentHandle, Clock, Deadline, FinalReducer, McpLifecycle, MemoryFinalizer,
    Operation, SaveKey, ServerError, ServerPhase, ShutdownFailure, ShutdownPhase, ShutdownReport,
    StoreHandle, WorkerLifecycle,
};
use super::state::AuthorityState;

/// Mutable lifecycle ports for one shutdown attempt.
///
/// The caller owns every double or real provider behind these fields. The
/// MCP lifecycle close owns the snapshot registry and listener teardown
/// together with the HTTP service, so the machine needs no separate snapshot
/// port: registry teardown is not a substitute for the MCP service close and
/// is not closed ahead of it.
pub struct ShutdownPorts<'a> {
    pub reducer: &'a mut dyn FinalReducer,
    pub workers: &'a mut dyn WorkerLifecycle,
    pub actors: &'a mut dyn ActorPersistence,
    pub memory: &'a mut dyn MemoryFinalizer,
    pub store: &'a mut dyn StoreHandle,
    pub agent: &'a mut dyn AgentHandle,
    pub mcp: &'a mut dyn McpLifecycle,
    pub clock: &'a dyn Clock,
}

/// Runs the resumable shutdown phase machine against one authority.
///
/// The machine reads the retained progress first: phases already listed as
/// completed are never replayed, so a retry resumes at the failed phase. A
/// `Closed` authority answers with the retained final report without
/// touching any port, which makes a completed shutdown idempotent. On any
/// phase error the failed phase stays `next` in the recorded report, the
/// error is classified for retryability, and a still-running authority is
/// flipped to `Closing` so no new work is admitted while the shutdown is
/// retried.
pub fn shutdown(
    state: &mut AuthorityState,
    ports: &mut ShutdownPorts<'_>,
    deadline: Deadline,
) -> Result<ShutdownReport, ShutdownFailure> {
    if state.phase() == ServerPhase::Closed {
        return Ok(state.shutdown_progress());
    }
    let mut report = state.shutdown_progress();
    for phase in ShutdownPhase::successors() {
        if *phase == ShutdownPhase::Closed {
            break;
        }
        if report.completed.contains(phase) {
            continue;
        }
        if deadline.expired(ports.clock.monotonic()) {
            if *phase == ShutdownPhase::FinalizeMemory {
                report.outstanding = ports.memory.pending().outstanding;
            }
            let error = ServerError::Timeout {
                operation: Operation::Shutdown,
            };
            return Err(fail(state, report, *phase, error));
        }
        if let Err(error) = run_phase(state, ports, *phase, &mut report, deadline) {
            return Err(fail(state, report, *phase, error));
        }
        report.completed.push(*phase);
        report.next = phase.next();
        state.record_progress(report.clone());
    }
    state.mark_closed();
    Ok(state.shutdown_progress())
}

/// Executes one phase's boundary work.
///
/// The `report` accumulates the durability counters a failed or resumed
/// attempt must preserve; it is written back to the authority by the caller
/// after the phase succeeds.
fn run_phase(
    state: &mut AuthorityState,
    ports: &mut ShutdownPorts<'_>,
    phase: ShutdownPhase,
    report: &mut ShutdownReport,
    deadline: Deadline,
) -> Result<(), ServerError> {
    match phase {
        ShutdownPhase::StopAdmission => {
            // The first call flips Running to Closing and stops admission; a
            // resume that reaches this phase with Closing already set
            // completes it without further effect.
            let _ = state.begin_close();
            Ok(())
        }
        ShutdownPhase::FinalTick => {
            // Exactly one final tick across every attempt: the state port is
            // single-shot once the reducer succeeds, and this phase is never
            // re-entered after it is recorded. The tick is unpublished; the
            // reducer works through the state ports and appends nothing to
            // any outbox.
            let tick = state.run_final(ports.reducer)?;
            report.final_tick = Some(tick);
            Ok(())
        }
        ShutdownPhase::Freeze => {
            // Freezing the authority derives the flush identity, and freezing
            // the Agent handle retains the lease this attempt releases only
            // after the store sync; a failure before release keeps it.
            let _ = state.freeze();
            let _ = ports.agent.freeze(ports.clock);
            Ok(())
        }
        ShutdownPhase::WaitWorkers => {
            // Quiesce the worker plane as one boundary: refuse new work,
            // cancel run and hostile work, then wait for the workers. All
            // three are idempotent, so a retry of a failed wait is safe.
            ports.workers.stop_new()?;
            ports.workers.cancel()?;
            ports.workers.wait(deadline)
        }
        ShutdownPhase::FinalizeMemory => {
            finalize_memory(state, ports.memory, ports.clock, deadline, report)
        }
        ShutdownPhase::FlushPlayers => flush_lane(state, ports, deadline, report, Lane::Players),
        ShutdownPhase::FlushCompanions => {
            flush_family(ports, SaveKey::Companions, deadline, report)
        }
        ShutdownPhase::FlushHostiles => flush_family(ports, SaveKey::Hostiles, deadline, report),
        ShutdownPhase::FlushPassives => flush_family(ports, SaveKey::Passives, deadline, report),
        ShutdownPhase::FlushWorld => flush_lane(state, ports, deadline, report, Lane::World),
        ShutdownPhase::FlushMetadata => flush_family(ports, SaveKey::Metadata, deadline, report),
        ShutdownPhase::StoreSync => {
            ports.store.sync(deadline)?;
            // A successful sync is the durability barrier for every flushed
            // family, so the outstanding flush work clears here.
            report.outstanding = 0;
            Ok(())
        }
        ShutdownPhase::ReleaseAgent => {
            // The handle keeps returning the lease retained at freeze time,
            // so a release retry addresses the same lease identity instead
            // of minting a new one. Without an active lease the phase is a
            // no-op success. A failed release retains the lease, the Agent,
            // the MCP service and the registry: none of the later closes
            // have run at this boundary.
            if let Some(lease) = ports.agent.freeze(ports.clock) {
                ports.agent.release(&lease, deadline)?;
            }
            Ok(())
        }
        ShutdownPhase::AgentClose => ports.agent.close(deadline),
        ShutdownPhase::McpClose => ports.mcp.close(deadline),
        ShutdownPhase::StoreClose => ports.store.close(deadline),
        ShutdownPhase::CloseWorkers => ports.workers.close(deadline),
        ShutdownPhase::Closed => Ok(()),
    }
}

/// Both public shutdown entry points share the same retained-memory barrier.
/// The caller records the report and resumes at this phase after a failure.
pub(crate) fn finalize_memory(
    authority: &mut AuthorityState,
    memory: &mut dyn MemoryFinalizer,
    clock: &dyn Clock,
    deadline: Deadline,
    report: &mut ShutdownReport,
) -> Result<(), ServerError> {
    // Retained semantic work and cleanup joins remain visible on every
    // failure. A fresh phase attempt never replays earlier shutdown phases.
    report.outstanding = memory.pending().outstanding;
    let remaining = deadline
        .instant()
        .saturating_duration_since(clock.monotonic());
    let wall_deadline = Instant::now() + remaining.min(Duration::from_secs(30));
    if let Err(error) = memory.begin_attempt(deadline) {
        report.outstanding = memory.pending().outstanding;
        return Err(error);
    }
    loop {
        if deadline.expired(clock.monotonic()) || Instant::now() >= wall_deadline {
            report.outstanding = memory.pending().outstanding;
            return Err(ServerError::Timeout {
                operation: Operation::Shutdown,
            });
        }
        let progress = match memory.drain_authority(authority, deadline) {
            Ok(memory) => memory,
            Err(error) => {
                report.outstanding = memory.pending().outstanding;
                return Err(error);
            }
        };
        report.outstanding = progress.outstanding.max(memory.pending().outstanding);
        if deadline.expired(clock.monotonic()) || Instant::now() >= wall_deadline {
            report.outstanding = memory.pending().outstanding;
            return Err(ServerError::Timeout {
                operation: Operation::Shutdown,
            });
        }
        if report.outstanding == 0 {
            return Ok(());
        }
        // The wall bound also applies when an injected clock is fixed.
        // Small waits let Agent workers finish without an unbounded spin.
        std::thread::sleep(
            Duration::from_millis(1).min(wall_deadline.saturating_duration_since(Instant::now())),
        );
    }
}

/// The per-key flush lanes of the frozen authority.
enum Lane {
    Players,
    World,
}

/// Flushes one lane's frozen save keys.
///
/// The frozen authority is an idempotent read of the dirty and in-flight
/// key set, so deriving it here observes the final tick's effects even on a
/// resumed attempt, and a lane with no keys performs no flush call.
fn flush_lane(
    state: &mut AuthorityState,
    ports: &mut ShutdownPorts<'_>,
    deadline: Deadline,
    report: &mut ShutdownReport,
    lane: Lane,
) -> Result<(), ServerError> {
    let frozen = state.freeze();
    let keys = frozen
        .save_keys
        .iter()
        .filter(|key| match lane {
            Lane::Players => matches!(key, SaveKey::Player(_)),
            Lane::World => matches!(key, SaveKey::Chunk(_)),
        })
        .cloned()
        .collect::<Vec<_>>();
    for key in keys {
        let flushed = ports.actors.flush(key, deadline)?;
        report.durable = report.durable.saturating_add(flushed.durable);
    }
    Ok(())
}

/// Flushes one aggregate family.
///
/// The family's report carries the failed and outstanding counters the
/// failure report preserves verbatim, so an interrupted flush leaves the
/// retained report naming the work that has not completed.
fn flush_family(
    ports: &mut ShutdownPorts<'_>,
    family: SaveKey,
    deadline: Deadline,
    report: &mut ShutdownReport,
) -> Result<(), ServerError> {
    let flushed = ports.actors.flush(family, deadline)?;
    report.durable = report.durable.saturating_add(flushed.durable);
    report.failed = report.failed.saturating_add(flushed.failed);
    report.outstanding = flushed.outstanding;
    Ok(())
}

/// Records a failed phase and preserves its retry boundary.
///
/// The failed phase stays `next` and the error decides retryability, so the
/// recorded report is the exact resume point for the next attempt.
fn fail(
    state: &mut AuthorityState,
    mut report: ShutdownReport,
    phase: ShutdownPhase,
    error: ServerError,
) -> ShutdownFailure {
    report.next = phase;
    report.failed = report.failed.saturating_add(1);
    report.retryable = state.tick_failure().is_none() && is_transient(&error);
    if state.phase() == ServerPhase::Running {
        let _ = state.begin_close();
    }
    state.record_progress(report.clone());
    ShutdownFailure { error, report }
}

/// Transient failures can clear on a later attempt: timeouts, I/O errors,
/// cancellation, disconnection, and typed Agent conditions. Field, state,
/// and invariant violations are permanent; retrying them cannot succeed.
pub(crate) fn is_transient(error: &ServerError) -> bool {
    matches!(
        error,
        ServerError::Timeout { .. }
            | ServerError::Io { .. }
            | ServerError::Cancelled
            | ServerError::Disconnected
            | ServerError::Agent { .. }
    )
}
