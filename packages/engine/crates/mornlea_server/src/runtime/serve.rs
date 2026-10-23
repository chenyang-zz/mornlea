//! Gameplay host assembly: DiskStore, companions, ServerTick, TCP, and pacing.
//!
//! The binary only parses flags and calls [`run`]. Tick pacing lives here via
//! [`super::host_tick`]. Configuration-file consumption beyond load/defaults
//! and fluid rescan stay in later PRs. Observer / dialogue-memory lanes are
//! not wired here yet (Owen open).

use std::collections::BTreeSet;
use std::fmt;
use std::io;
use std::net::{SocketAddr, TcpListener};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use mornlea_domain::{ChunkPos, CompanionId, Dimension};
use mornlea_storage::{HostileMobs, Metadata, PassiveMobs, METADATA_CURRENT_VERSION};

use crate::contracts::{
    AgentHandle, AgentPoll, AgentRequest, AgentRequestId, ChunkKey, ClientInstanceId, Clock,
    ConnectionId, ConnectionProgress, Deadline, NamespaceId, SaveBudget, ServerError, ServerLimits,
    SnapshotPort, StoreHandle, StoreLimits, WorkerLifecycle,
};
use crate::core::acquisition::LiveChunkPhase;
use crate::core::chunk_driver::ChunkDriver;
use crate::core::companion_runner::TaskTimeoutMinutes;
use crate::core::generation_worker::GenerationPool;
use crate::core::step::AuthoritativeFinalReducer;
use crate::runtime::companion::{
    env_credential, start_companions, system_identity, CompanionRuntime, CompanionStartupPorts,
    SystemClock,
};
use crate::runtime::companion_planning::CompanionPlanningPorts;
use crate::runtime::host_tick::{HostClock, HostTickScheduler, WallClock};
use crate::runtime::shutdown::{
    outcome_from_step, run_shutdown, RuntimeShutdownPorts, TickOutcome,
};
use crate::runtime::tick_loop::{ServerTick, TickStartup};
use crate::runtime::RuntimeConfig;
use crate::state::AuthorityState;
use crate::store::disk::{DiskOptions, DiskStore};
use crate::store::lease::WorldLease;
use crate::store::mailbox::StoreMailbox;
use crate::store::scheduler::{AutosaveScheduler, SchedulerConfig};
use crate::store::shutdown::SharedScheduler;
use crate::transport::live::LoginDriver;
use crate::transport::tcp::TcpTransport;

/// Flags the binary parsed for gameplay serve.
pub struct ServeOptions {
    pub world: PathBuf,
    pub listen: SocketAddr,
    pub max_ticks: u64,
    pub config: Option<PathBuf>,
}

/// Typed failure from gameplay host assembly or the paced loop.
#[derive(Debug)]
pub enum ServeError {
    Config(crate::runtime::ConfigError),
    Disk(ServerError),
    Authority(ServerError),
    Companions(crate::runtime::companion::CompanionStartError),
    Bind(io::Error),
    Store(ServerError),
    Shutdown(crate::contracts::ShutdownFailure),
    Message(&'static str),
}

impl fmt::Display for ServeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Config(error) => write!(f, "config: {error}"),
            Self::Disk(error) => write!(f, "disk: {error:?}"),
            Self::Authority(error) => write!(f, "authority: {error:?}"),
            Self::Companions(error) => write!(f, "companions: {error}"),
            Self::Bind(error) => write!(f, "bind: {error}"),
            Self::Store(error) => write!(f, "store: {error:?}"),
            Self::Shutdown(error) => write!(f, "shutdown: {error:?}"),
            Self::Message(message) => write!(f, "{message}"),
        }
    }
}

/// Runs `on_tick` once per scheduled host tick until `stop` returns true.
///
/// Sleeps a short slice when idle so a wall-clock host does not busy-spin.
/// Tests inject [`super::host_tick::ManualClock`] and pass `idle_sleep: None`,
/// advancing the clock themselves between iterations via [`HostTickScheduler::clock_mut`].
pub fn run_paced_loop<C: HostClock>(
    mut scheduler: HostTickScheduler<C>,
    mut stop: impl FnMut(&mut HostTickScheduler<C>) -> bool,
    mut on_tick: impl FnMut(),
    idle_sleep: Option<Duration>,
) {
    while !stop(&mut scheduler) {
        let due = scheduler.poll();
        if due == 0 {
            if let Some(pause) = idle_sleep {
                std::thread::sleep(pause);
            }
            continue;
        }
        for _ in 0..due {
            on_tick();
        }
    }
}

/// Production wall-clock loop at 20 TPS with a 1 ms idle pause.
pub fn run_wall_clock_loop(mut stop: impl FnMut() -> bool, on_tick: impl FnMut()) {
    run_paced_loop(
        HostTickScheduler::new(WallClock),
        |_| stop(),
        on_tick,
        Some(Duration::from_millis(1)),
    );
}

fn uuid_v4(tag: u8) -> [u8; 16] {
    let mut bytes = [0u8; 16];
    bytes[0] = tag;
    bytes[6] = 0x40;
    bytes[8] = 0x80;
    bytes
}

fn default_metadata() -> Metadata {
    Metadata {
        format_version: METADATA_CURRENT_VERSION,
        seed: 42,
        spawn_dimension: 0,
        spawn_anchor: mornlea_storage::MetadataChunkPos { x: 0, z: 0 },
        world_time_ticks: 0,
        day_phase_offset: 0,
        weather_kind: 0,
        weather_ticks_remaining: 0,
        depths_spawn_anchor: mornlea_storage::MetadataChunkPos { x: 0, z: 0 },
        depths_seed_salt: 0,
        difficulty: 0,
    }
}

fn disk_options() -> DiskOptions {
    DiskOptions {
        region_handle_cap: 8,
        create: default_metadata(),
    }
}

fn server_limits() -> Result<ServerLimits, ServeError> {
    ServerLimits::try_new(8, 4096, 512, 64, 64, 1_048_576).map_err(ServeError::Authority)
}

fn store_limits() -> Result<StoreLimits, ServeError> {
    StoreLimits::try_new(2, 16, 3, 3, 3, 1, 8, 4_194_304).map_err(ServeError::Store)
}

/// Opens DiskStore, enables persistence, optionally starts companions, binds
/// TCP, and runs [`ServerTick::step`] under the 20 TPS wall-clock host until
/// `max_ticks` or `stop` is set. Returns the number of ticks executed.
pub fn run(options: ServeOptions, stop: Arc<AtomicBool>) -> Result<u64, ServeError> {
    let _ = std::fs::create_dir_all(&options.world);
    let _lease = if options.world.join("world.meta").is_file() {
        Some(WorldLease::acquire(&options.world).map_err(ServeError::Disk)?)
    } else {
        None
    };

    let config = match &options.config {
        Some(path) => RuntimeConfig::load(path).map_err(ServeError::Config)?,
        None => RuntimeConfig::defaults(),
    };

    let listener = TcpListener::bind(options.listen).map_err(ServeError::Bind)?;
    let mut transport = TcpTransport::from_listener(listener).map_err(ServeError::Bind)?;
    let mut login = LoginDriver::new();

    let mut disk = DiskStore::open(&options.world, disk_options()).map_err(ServeError::Disk)?;
    let mut authority =
        AuthorityState::try_new_with_metadata(server_limits()?, disk.metadata().clone())
            .map_err(ServeError::Authority)?;
    authority
        .enable_source_player_restoration(1)
        .map_err(ServeError::Authority)?;
    authority
        .enable_live_chunks()
        .map_err(ServeError::Authority)?;
    authority
        .enable_actor_saves()
        .map_err(ServeError::Authority)?;
    authority
        .enable_player_persistence()
        .map_err(ServeError::Authority)?;
    authority
        .enable_mob_persistence(
            HostileMobs {
                revision: 0,
                records: vec![],
            },
            PassiveMobs {
                revision: 0,
                records: vec![],
            },
        )
        .map_err(ServeError::Authority)?;

    let clock = SystemClock;
    let clock_arc: Arc<dyn Clock + Send + Sync> = Arc::new(SystemClock);
    let credential = |name: &str| env_credential(name);
    let mut identity = || system_identity();
    let mut companions = start_companions(
        config.ai(),
        &mut disk,
        &mut authority,
        CompanionStartupPorts {
            clock: Arc::clone(&clock_arc),
            credential: &credential,
            identity: &mut identity,
        },
    )
    .map_err(ServeError::Companions)?;

    let companion_ids: Vec<CompanionId> = companions
        .as_ref()
        .map(|runtime| runtime.definitions().iter().map(|(id, _)| *id).collect())
        .unwrap_or_default();

    let mailbox =
        StoreMailbox::try_new_background(store_limits()?, disk).map_err(ServeError::Store)?;
    let mut scheduler = AutosaveScheduler::try_new(SchedulerConfig::default(), mailbox)
        .map_err(ServeError::Store)?;

    let seed = authority.world_seed();
    warm_spawn_chunk(&mut authority, &mut scheduler, seed)?;

    let startup = match companions.as_ref() {
        Some(runtime) => TickStartup::for_companions_full(runtime),
        None => TickStartup::with_full_budget(TaskTimeoutMinutes::DEFAULT),
    };
    let mut tick = ServerTick::new(startup, companion_ids);

    let ticks = Arc::new(AtomicU64::new(0));
    let tick_cap = options.max_ticks;
    println!(
        "mornlea-server serve ready world={} listen={}",
        options.world.display(),
        options.listen
    );
    let _ = std::io::Write::flush(&mut std::io::stdout());

    let stop_flag = Arc::clone(&stop);
    let ticks_for_stop = Arc::clone(&ticks);
    let mut idle_agent = IdleAgent;
    let mut idle_snapshots = IdleSnapshots;
    let mut mint_counter = 0u64;
    let mut mint = || {
        mint_counter = mint_counter.saturating_add(1);
        let mut bytes = uuid_v4(0x40);
        bytes[9..16].copy_from_slice(&mint_counter.to_be_bytes()[1..]);
        Ok(bytes)
    };
    let mut connections: Vec<ConnectionId> = Vec::new();

    run_paced_loop(
        HostTickScheduler::new(WallClock),
        |_| stop_flag.load(Ordering::SeqCst) || ticks_for_stop.load(Ordering::SeqCst) >= tick_cap,
        || {
            if let Ok(Some(id)) =
                transport.accept_one(&mut login.bind(&mut authority, &mut scheduler), &clock)
            {
                connections.push(id);
            }

            // Advance logins and drain control frames before the tick so a
            // peer that just connected can enter Play on this period.
            let mut live = Vec::with_capacity(connections.len());
            for id in connections.drain(..) {
                let progress = {
                    let mut endpoint = login.bind(&mut authority, &mut scheduler);
                    let _ = transport.pump_in(id, &mut endpoint, &clock);
                    transport.poll(id, &mut endpoint, &clock)
                };
                if matches!(progress, ConnectionProgress::Closed { .. }) {
                    continue;
                }
                let _ = transport.flush_out(id, &mut login.bind(&mut authority, &mut scheduler));
                live.push(id);
            }
            connections = live;

            let step_result = match companions.as_mut() {
                Some(runtime) => {
                    let client = runtime.agent().client();
                    let namespace = runtime.agent().namespace();
                    let lease = runtime.agent().current_lease();
                    let (agent, snapshots) = runtime.agent_mut().planning_handles();
                    let ports = CompanionPlanningPorts {
                        agent,
                        snapshots,
                        clock: &clock,
                        client,
                        namespace,
                        lease,
                        mint_id: &mut mint,
                    };
                    tick.step(&mut authority, ports)
                }
                None => {
                    let ports = CompanionPlanningPorts {
                        agent: &mut idle_agent,
                        snapshots: &mut idle_snapshots,
                        clock: &clock,
                        client: ClientInstanceId::try_from_bytes(uuid_v4(0x11)).unwrap(),
                        namespace: NamespaceId::try_from_bytes(uuid_v4(0x22)).unwrap(),
                        lease: None,
                        mint_id: &mut mint,
                    };
                    tick.step(&mut authority, ports)
                }
            };

            match outcome_from_step(step_result) {
                TickOutcome::Continued => {
                    let next = authority.next_tick().saturating_sub(1);
                    let _ = scheduler.poll_tick(next, SaveBudget::default(), &mut authority);
                    scheduler.drive_workers();
                    // Forward session outboxes onto committed TCP peers, then flush.
                    let mut live = Vec::with_capacity(connections.len());
                    for id in connections.drain(..) {
                        let mut endpoint = login.bind(&mut authority, &mut scheduler);
                        if let Some(session) = transport.active_session(id) {
                            let _ = transport.forward_prepared_outbox(
                                id,
                                session,
                                &mut endpoint,
                                64,
                                1 << 20,
                            );
                        }
                        let (_, progress) = transport.flush_out(id, &mut endpoint);
                        if !matches!(progress, ConnectionProgress::Closed { .. }) {
                            live.push(id);
                        }
                    }
                    connections = live;
                    ticks.fetch_add(1, Ordering::SeqCst);
                }
                TickOutcome::BeginShutdown(trigger) => {
                    eprintln!("FAIL tick {:?}", trigger.error());
                    stop_flag.store(true, Ordering::SeqCst);
                }
            }
        },
        Some(Duration::from_millis(1)),
    );

    let executed = ticks.load(Ordering::SeqCst);
    // Drop live peers before shutdown so sessions do not linger on the wire.
    for id in connections.drain(..) {
        transport.close(
            id,
            crate::contracts::CloseReason::Shutdown,
            &mut login.bind(&mut authority, &mut scheduler),
        );
    }
    finish_shutdown(&mut authority, &mut companions, scheduler, &clock)?;
    let _ = login;
    let _ = transport;
    Ok(executed)
}

fn warm_spawn_chunk(
    authority: &mut AuthorityState,
    scheduler: &mut AutosaveScheduler<DiskStore>,
    seed: i64,
) -> Result<(), ServeError> {
    let spawn = ChunkKey {
        dimension: Dimension::OVERWORLD,
        pos: ChunkPos::new(0, 0),
    };
    let mut driver = ChunkDriver::new();
    let mut pool = GenerationPool::try_new(seed, false, 1).map_err(ServeError::Authority)?;
    let deadline =
        Deadline::after(Instant::now(), Duration::from_secs(30)).map_err(ServeError::Authority)?;
    let budget = TickStartup::with_full_budget(TaskTimeoutMinutes::DEFAULT).budget();

    authority
        .replace_chunk_wants(BTreeSet::from([spawn]))
        .map_err(ServeError::Authority)?;
    driver
        .start_load(authority, scheduler, spawn, deadline)
        .map_err(ServeError::Authority)?;
    drain_chunk_driver(&mut driver, authority, scheduler, &mut pool, deadline)?;
    authority
        .advance_tick(budget)
        .map_err(ServeError::Authority)?;

    driver
        .start_generation(authority, &mut pool, spawn)
        .map_err(ServeError::Authority)?;
    drain_chunk_driver(&mut driver, authority, scheduler, &mut pool, deadline)?;
    authority
        .advance_tick(budget)
        .map_err(ServeError::Authority)?;

    let phase = authority
        .live_chunk_facts(spawn)
        .map(|f| f.phase)
        .ok_or(ServeError::Message("spawn chunk load"))?;
    if phase != LiveChunkPhase::Ready {
        let _ = WorkerLifecycle::close(&mut pool, deadline);
        return Err(ServeError::Message("spawn chunk load"));
    }
    let _ = WorkerLifecycle::close(&mut pool, deadline);
    Ok(())
}

fn drain_chunk_driver(
    driver: &mut ChunkDriver,
    authority: &mut AuthorityState,
    scheduler: &mut AutosaveScheduler<DiskStore>,
    pool: &mut GenerationPool,
    deadline: Deadline,
) -> Result<(), ServeError> {
    let until = Instant::now() + Duration::from_secs(30);
    loop {
        if Instant::now() >= until || deadline.expired(Instant::now()) {
            return Err(ServeError::Message("spawn chunk timeout"));
        }
        scheduler.drive_workers();
        let report = driver.poll(authority, scheduler, pool);
        if report.retained == 0 {
            if let Some(error) = report.first_error {
                return Err(ServeError::Authority(error));
            }
            return Ok(());
        }
        thread::yield_now();
    }
}

fn finish_shutdown(
    authority: &mut AuthorityState,
    companions: &mut Option<CompanionRuntime>,
    scheduler: AutosaveScheduler<DiskStore>,
    clock: &dyn Clock,
) -> Result<(), ServeError> {
    let shared = SharedScheduler::new(scheduler);
    let mut reducer = AuthoritativeFinalReducer;
    let mut workers = IdleWorkers;
    let mut actors = shared.actors(clock);
    let mut memory = IdleMemory;
    let mut store = shared.store();
    let deadline =
        Deadline::after(Instant::now(), Duration::from_secs(30)).map_err(ServeError::Authority)?;

    if let Some(runtime) = companions.as_mut() {
        let (agent, mcp) = runtime.agent_mut().shutdown_handles();
        let mut ports = RuntimeShutdownPorts {
            reducer: &mut reducer,
            workers: &mut workers,
            actors: &mut actors,
            memory: &mut memory,
            store: &mut store,
            agent,
            mcp,
            clock,
        };
        run_shutdown(authority, &mut ports, deadline, None).map_err(ServeError::Shutdown)?;
        runtime.agent_mut().mark_closed();
    } else {
        let mut agent = IdleAgent;
        let mut mcp = IdleMcp;
        let mut ports = RuntimeShutdownPorts {
            reducer: &mut reducer,
            workers: &mut workers,
            actors: &mut actors,
            memory: &mut memory,
            store: &mut store,
            agent: &mut agent,
            mcp: &mut mcp,
            clock,
        };
        run_shutdown(authority, &mut ports, deadline, None).map_err(ServeError::Shutdown)?;
    }
    Ok(())
}

struct IdleAgent;
impl AgentHandle for IdleAgent {
    fn submit(&mut self, _: AgentRequest) -> Result<AgentRequestId, ServerError> {
        Err(ServerError::Internal {
            invariant: "idle agent",
        })
    }
    fn poll(&mut self, _: AgentRequestId) -> AgentPoll {
        AgentPoll::Failed(ServerError::Internal {
            invariant: "idle agent",
        })
    }
    fn cancel(&mut self, _: AgentRequestId, _: Deadline) -> Result<(), ServerError> {
        Ok(())
    }
    fn freeze(&mut self, _: &dyn Clock) -> Option<crate::contracts::FrozenLease> {
        None
    }
    fn release(
        &mut self,
        _: &crate::contracts::FrozenLease,
        _: Deadline,
    ) -> Result<(), ServerError> {
        Ok(())
    }
    fn close(&mut self, _: Deadline) -> Result<(), ServerError> {
        Ok(())
    }
}

struct IdleSnapshots;
impl SnapshotPort for IdleSnapshots {
    fn register(
        &mut self,
        _: NamespaceId,
        _: CompanionId,
        _: u64,
        _: crate::contracts::PlanningSnapshot,
        _: Deadline,
    ) -> Result<crate::contracts::SnapshotRegistration, ServerError> {
        Err(ServerError::Internal {
            invariant: "idle snapshots",
        })
    }
    fn complete(&mut self, _: crate::contracts::SnapshotId) -> Result<(), ServerError> {
        Ok(())
    }
    fn cancel(&mut self, _: crate::contracts::SnapshotId) -> Result<(), ServerError> {
        Ok(())
    }
    fn close(&mut self) -> Result<(), ServerError> {
        Ok(())
    }
}

struct IdleMcp;
impl crate::contracts::McpLifecycle for IdleMcp {
    fn close(&mut self, _: Deadline) -> Result<(), ServerError> {
        Ok(())
    }
}

struct IdleMemory;
impl crate::contracts::MemoryFinalizer for IdleMemory {
    fn pending(&self) -> crate::contracts::MemoryFinalizationReport {
        crate::contracts::MemoryFinalizationReport {
            completed: 0,
            outstanding: 0,
        }
    }
    fn begin_attempt(&mut self, _: Deadline) -> Result<(), ServerError> {
        Ok(())
    }
    fn drain(
        &mut self,
        _: Deadline,
    ) -> Result<crate::contracts::MemoryFinalizationReport, ServerError> {
        Ok(self.pending())
    }
}

struct IdleWorkers;
impl WorkerLifecycle for IdleWorkers {
    fn stop_new(&mut self) -> Result<(), ServerError> {
        Ok(())
    }
    fn cancel(&mut self) -> Result<(), ServerError> {
        Ok(())
    }
    fn wait(&mut self, _: Deadline) -> Result<(), ServerError> {
        Ok(())
    }
    fn close(&mut self, _: Deadline) -> Result<(), ServerError> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::host_tick::{ManualClock, TICK_PERIOD};
    use std::time::Instant;

    fn empty_authority() -> AuthorityState {
        AuthorityState::try_new(
            ServerLimits::try_new(8, 4096, 512, 64, 64, 1_048_576).unwrap(),
            7,
        )
        .unwrap()
    }

    #[test]
    fn paced_loop_advances_world_time_only_on_executed_ticks() {
        let mut authority = empty_authority();
        let mut sched = HostTickScheduler::new(ManualClock::new(Instant::now()));
        let before = authority.next_tick();
        sched.clock_mut().advance(TICK_PERIOD);
        assert_eq!(sched.poll(), 1);
        authority.advance_tick_full().unwrap();
        assert_eq!(authority.next_tick(), before + 1);
        sched.clock_mut().advance(TICK_PERIOD * 5);
        let due = sched.poll();
        assert_eq!(due, 2);
        for _ in 0..due {
            authority.advance_tick_full().unwrap();
        }
        assert_eq!(
            authority.next_tick(),
            before + 3,
            "five late periods execute two ticks; dropped periods skip world time"
        );
    }

    #[test]
    fn run_paced_loop_executes_two_on_time_ticks() {
        use std::cell::Cell;
        let ticks = Cell::new(0u8);
        let periods = Cell::new(0u8);
        run_paced_loop(
            HostTickScheduler::new(ManualClock::new(Instant::now())),
            |sched| {
                if ticks.get() >= 2 {
                    return true;
                }
                periods.set(periods.get() + 1);
                sched.clock_mut().advance(TICK_PERIOD);
                false
            },
            || ticks.set(ticks.get() + 1),
            None,
        );
        assert_eq!(ticks.get(), 2);
        assert!(periods.get() >= 2);
    }

    #[test]
    fn wall_period_constant_is_fifty_ms() {
        assert_eq!(TICK_PERIOD, Duration::from_millis(50));
    }

    #[test]
    fn production_serve_source_does_not_call_advance_tick_full() {
        let serve = std::fs::read_to_string(
            std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src/runtime/serve.rs"),
        )
        .unwrap();
        let production: String = serve
            .lines()
            .take_while(|line| !line.contains("#[cfg(test)]"))
            .filter(|line| !line.trim_start().starts_with("//"))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            !production.contains("advance_tick_full"),
            "production serve must drive ServerTick::step, not advance_tick_full"
        );
        // warm_spawn may call TickBudget::full via advance_tick — that must
        // stay on TickStartup for constructors; advance_tick itself is fine.
        assert!(
            production.contains("tick.step(") || production.contains("ServerTick"),
            "production serve must call ServerTick::step"
        );
    }
}
