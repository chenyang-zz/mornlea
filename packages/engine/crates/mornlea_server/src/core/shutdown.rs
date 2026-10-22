//! Retryable shutdown: one final unpublished tick and ordered closure.
//!
//! This module owns the resumable phase machine behind the endpoint's
//! shutdown boundary. A first attempt stops admission, flushes the players
//! settled by the last ordinary tick, drains every accepted command into
//! exactly one final unpublished tick, freezes the authority and the Agent
//! lease, quiesces the workers, finalizes Agent memory, flushes the remaining
//! save families in the fixed order companions, hostiles, passives, world
//! chunks and metadata, syncs the store, releases the Agent lease, and
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
        ShutdownPhase::FlushPlayers => {
            // The first phase after admission stops: a failed authority
            // refuses here with its retained error, before any flush I/O or
            // final tick, so the attempt stays permanently failed.
            if let Some(error) = state.tick_failure() {
                return Err(error);
            }
            flush_lane(state, ports, phase, deadline, report)
        }
        ShutdownPhase::FlushWorld => flush_lane(state, ports, phase, deadline, report),
        ShutdownPhase::FlushCompanions
        | ShutdownPhase::FlushHostiles
        | ShutdownPhase::FlushPassives
        | ShutdownPhase::FlushMetadata => flush_family(state, ports, phase, deadline, report),
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

/// The six save families in the fixed order the shutdown flush visits
/// them, mirroring Go: players (`host_shutdown.go:83`) before the server
/// shutdown, then companions, hostiles and passives (`shutdown.go:152`,
/// `:161`, `:169`), world chunks and metadata (`shutdown.go:175`, which is
/// `persistence/world.go:196` then `:199`). Each family is a barrier: the
/// next one starts only after the store reports the previous one durable,
/// and a failed family stops the attempt at that family.
pub const FLUSH_ORDER: [ShutdownPhase; 6] = [
    ShutdownPhase::FlushPlayers,
    ShutdownPhase::FlushCompanions,
    ShutdownPhase::FlushHostiles,
    ShutdownPhase::FlushPassives,
    ShutdownPhase::FlushWorld,
    ShutdownPhase::FlushMetadata,
];

/// Selects the save keys one flush phase hands to the store, in order.
///
/// Pure over the frozen pending key set. The players and world lanes take
/// only their own pending keys in the frozen order; the aggregate families
/// and metadata are visited exactly once whether or not they are pending,
/// like the unconditional Go `Flush` calls. Any other phase selects nothing.
pub fn family_keys(phase: ShutdownPhase, pending: &[SaveKey]) -> Vec<SaveKey> {
    match phase {
        ShutdownPhase::FlushPlayers | ShutdownPhase::FlushWorld => pending
            .iter()
            .filter(|key| flush_phase(key) == phase)
            .cloned()
            .collect(),
        ShutdownPhase::FlushCompanions => vec![SaveKey::Companions],
        ShutdownPhase::FlushHostiles => vec![SaveKey::Hostiles],
        ShutdownPhase::FlushPassives => vec![SaveKey::Passives],
        ShutdownPhase::FlushMetadata => vec![SaveKey::Metadata],
        _ => Vec::new(),
    }
}

/// The flush phase that owns one save key. Pure; a store uses it to keep
/// every write it issues during a phase inside that phase's family.
pub fn flush_phase(key: &SaveKey) -> ShutdownPhase {
    match key {
        SaveKey::Player(_) => ShutdownPhase::FlushPlayers,
        SaveKey::Companions => ShutdownPhase::FlushCompanions,
        SaveKey::Hostiles => ShutdownPhase::FlushHostiles,
        SaveKey::Passives => ShutdownPhase::FlushPassives,
        SaveKey::Chunk(_) => ShutdownPhase::FlushWorld,
        SaveKey::Metadata => ShutdownPhase::FlushMetadata,
    }
}

/// Flushes one lane's frozen save keys.
///
/// The frozen authority is an idempotent read of the dirty and in-flight
/// key set, so deriving it here observes every settled tick before the
/// phase (the last ordinary tick for players, the final tick for the world)
/// even on a resumed attempt, and a lane with no keys performs no flush call.
fn flush_lane(
    state: &mut AuthorityState,
    ports: &mut ShutdownPorts<'_>,
    phase: ShutdownPhase,
    deadline: Deadline,
    report: &mut ShutdownReport,
) -> Result<(), ServerError> {
    let frozen = state.freeze();
    for key in family_keys(phase, &frozen.save_keys) {
        let flushed = ports.actors.flush_authority(state, key, deadline)?;
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
    state: &mut AuthorityState,
    ports: &mut ShutdownPorts<'_>,
    phase: ShutdownPhase,
    deadline: Deadline,
    report: &mut ShutdownReport,
) -> Result<(), ServerError> {
    for family in family_keys(phase, &[]) {
        let flushed = ports.actors.flush_authority(state, family, deadline)?;
        report.durable = report.durable.saturating_add(flushed.durable);
        report.failed = report.failed.saturating_add(flushed.failed);
        report.outstanding = flushed.outstanding;
    }
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

#[cfg(test)]
mod flush_order_tests {
    use super::{FLUSH_ORDER, family_keys, flush_phase};
    use crate::core::contracts::{ChunkKey, SaveKey, ShutdownPhase};
    use mornlea_domain::{ChunkPos, Dimension, PlayerId};

    /// Reads one Go production source relative to the repository root.
    fn go_source(relative: &str) -> String {
        let path = format!("{}/../../../{relative}", env!("CARGO_MANIFEST_DIR"));
        std::fs::read_to_string(&path).unwrap_or_else(|error| panic!("{path}: {error}"))
    }

    /// One-based line of the only occurrence of `needle` in `source`.
    fn line_of(source: &str, needle: &str) -> usize {
        let lines: Vec<usize> = source
            .lines()
            .enumerate()
            .filter(|(_, line)| line.contains(needle))
            .map(|(index, _)| index + 1)
            .collect();
        assert_eq!(lines.len(), 1, "{needle:?} must appear exactly once");
        lines[0]
    }

    fn position(phase: ShutdownPhase) -> usize {
        ShutdownPhase::successors()
            .iter()
            .position(|candidate| *candidate == phase)
            .unwrap()
    }

    #[test]
    fn flush_order_is_the_fixed_family_subsequence() {
        let flushes: Vec<ShutdownPhase> = ShutdownPhase::successors()
            .iter()
            .copied()
            .filter(|phase| FLUSH_ORDER.contains(phase))
            .collect();
        assert_eq!(
            FLUSH_ORDER,
            [
                ShutdownPhase::FlushPlayers,
                ShutdownPhase::FlushCompanions,
                ShutdownPhase::FlushHostiles,
                ShutdownPhase::FlushPassives,
                ShutdownPhase::FlushWorld,
                ShutdownPhase::FlushMetadata,
            ]
        );
        assert_eq!(flushes, FLUSH_ORDER);
        // Players flush right after admission stops and before the final
        // tick, like Go's host flush ahead of the server shutdown.
        assert_eq!(
            position(ShutdownPhase::StopAdmission) + 1,
            position(ShutdownPhase::FlushPlayers)
        );
        assert_eq!(
            position(ShutdownPhase::FlushPlayers) + 1,
            position(ShutdownPhase::FinalTick)
        );
        // The world-side families sit contiguously between memory
        // finalization and the store sync, so no other boundary runs between
        // two of them.
        let first = position(ShutdownPhase::FlushCompanions);
        for (offset, phase) in FLUSH_ORDER[1..].iter().enumerate() {
            assert_eq!(position(*phase), first + offset);
        }
        assert_eq!(position(ShutdownPhase::FinalizeMemory) + 1, first);
        assert_eq!(
            position(ShutdownPhase::StoreSync),
            first + FLUSH_ORDER.len() - 1
        );
    }

    #[test]
    fn flush_order_matches_go_shutdown_source() {
        // Go host shutdown flushes players, then runs the server shutdown,
        // whose barriers run companions, hostiles, passives, world (chunks
        // then metadata), store sync, the companion runtime close hook, and
        // the store close, each returning before the next starts.
        let host = go_source("server/server/host_shutdown.go");
        let server = go_source("server/server/shutdown.go");
        let world = go_source("server/server/persistence/world.go");
        // Sessions detach (unregistering each player from the engine) and
        // players flush before the server shutdown runs its final step, so
        // the saved player is the body the last ordinary step settled.
        assert_eq!(
            line_of(
                &host,
                "h.world.DetachSession(active.Session, active.Generation, network.ErrClosed)"
            ),
            78
        );
        assert_eq!(line_of(&host, "h.players.Flush(ctx)"), 83);
        assert_eq!(line_of(&host, "worldErr := h.world.Shutdown(ctx)"), 93);
        assert_eq!(
            line_of(&server, "server.engine.StepWithTunables(tickTunables)"),
            75
        );
        assert!(position(ShutdownPhase::StopAdmission) < position(ShutdownPhase::FlushPlayers));
        assert!(position(ShutdownPhase::FlushPlayers) < position(ShutdownPhase::FinalTick));
        let pinned = [
            (
                ShutdownPhase::FlushCompanions,
                "server.companions.Flush(ctx)",
                152,
            ),
            (
                ShutdownPhase::FlushHostiles,
                "server.hostiles.Flush(ctx)",
                161,
            ),
            (
                ShutdownPhase::FlushPassives,
                "server.passives.Flush(ctx)",
                169,
            ),
            (ShutdownPhase::FlushWorld, "server.world.Flush(ctx)", 175),
            (ShutdownPhase::StoreSync, "server.store.Sync(ctx)", 179),
            (
                ShutdownPhase::ReleaseAgent,
                "server.beforeStoreClose(ctx)",
                188,
            ),
            (ShutdownPhase::StoreClose, "server.store.Close()", 193),
        ];
        let mut previous_line = 75;
        let mut previous_phase = position(ShutdownPhase::FinalTick);
        for (phase, needle, line) in pinned {
            assert_eq!(line_of(&server, needle), line, "{needle}");
            assert!(line > previous_line, "Go order at {needle}");
            assert!(position(phase) > previous_phase, "Rust order at {phase:?}");
            previous_line = line;
            previous_phase = position(phase);
        }
        // Inside the Go world barrier chunks precede metadata.
        let chunks = line_of(&world, "world.flushFrozen(ctx)");
        let metadata = line_of(&world, "return world.flushMetadata(ctx)");
        assert_eq!((chunks, metadata), (196, 199));
        assert!(position(ShutdownPhase::FlushWorld) < position(ShutdownPhase::FlushMetadata));
        assert!(position(ShutdownPhase::FlushMetadata) < position(ShutdownPhase::StoreSync));
    }

    fn player(tag: u8) -> SaveKey {
        let mut bytes = [0u8; 16];
        bytes[0] = tag;
        bytes[6] = 0x40;
        bytes[8] = 0x80;
        SaveKey::Player(PlayerId::try_from_bytes(bytes).unwrap())
    }

    fn chunk(x: i32) -> SaveKey {
        SaveKey::Chunk(ChunkKey {
            dimension: Dimension::OVERWORLD,
            pos: ChunkPos::new(x, 0),
        })
    }

    #[test]
    fn family_keys_selects_each_family_only() {
        let pending = vec![
            chunk(2),
            player(1),
            SaveKey::Hostiles,
            chunk(1),
            player(2),
            SaveKey::Companions,
        ];
        let before = pending.clone();
        assert_eq!(
            family_keys(ShutdownPhase::FlushPlayers, &pending),
            vec![player(1), player(2)]
        );
        assert_eq!(
            family_keys(ShutdownPhase::FlushWorld, &pending),
            vec![chunk(2), chunk(1)]
        );
        // Aggregate families and metadata are visited once whether or not
        // they are pending, like the unconditional Go Flush calls.
        for (phase, key) in [
            (ShutdownPhase::FlushCompanions, SaveKey::Companions),
            (ShutdownPhase::FlushHostiles, SaveKey::Hostiles),
            (ShutdownPhase::FlushPassives, SaveKey::Passives),
            (ShutdownPhase::FlushMetadata, SaveKey::Metadata),
        ] {
            assert_eq!(family_keys(phase, &pending), vec![key.clone()]);
            assert_eq!(family_keys(phase, &[]), vec![key]);
        }
        for phase in ShutdownPhase::successors() {
            if !FLUSH_ORDER.contains(phase) {
                assert!(family_keys(*phase, &pending).is_empty(), "{phase:?}");
            }
        }
        for key in &pending {
            assert!(family_keys(flush_phase(key), &pending).contains(key));
        }
        assert_eq!(
            flush_phase(&SaveKey::Metadata),
            ShutdownPhase::FlushMetadata
        );
        assert_eq!(
            flush_phase(&SaveKey::Passives),
            ShutdownPhase::FlushPassives
        );
        // Pure: the same input gives the same answer and is left untouched.
        assert_eq!(
            family_keys(ShutdownPhase::FlushWorld, &pending),
            family_keys(ShutdownPhase::FlushWorld, &pending)
        );
        assert_eq!(pending, before);
    }
}
