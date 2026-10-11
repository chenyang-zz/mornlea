//! Bounded request correlation over borrowed store and native CPU owners.
//!
//! Authority reservations precede provider starts; staged aliases remain charged
//! until Acquire. This driver never selects source goals, cancels ambiguous
//! requests, prepares bodies or waits for workers. Its borrower owns explicit
//! provider driving and shutdown, including every native join before release.
use std::collections::BTreeMap;

use super::{
    acquisition::AcquiredChunkEvent,
    contracts::{
        ChunkKey, ChunkLoadPoll, ChunkLoadPort, ChunkRequestId, Deadline, GenerationPoll,
        GenerationPort, Resource, ServerError, ServerPhase,
    },
    state::AuthorityState,
};

const SOURCE_LIMIT: usize = 8;

/// One nonblocking pass, including independent offers settled before a refusal.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ChunkPollReport {
    pub polled: usize,
    pub offered: usize,
    pub pending: usize,
    pub retained: usize,
    pub first_error: Option<ServerError>,
}

enum Phase {
    Bound,
    // An ambiguous successful start has a request; an impossible abort refusal
    // has no request and retains the original port error for ownership diagnosis.
    Quarantined { start_error: Option<ServerError> },
}
struct Record {
    key: ChunkKey,
    generation: u64,
    request: Option<ChunkRequestId>,
    phase: Phase,
    // The event owns its original prepared payload. Refusals move it back here
    // without copying compact blocks or allocating another result envelope.
    event: Option<AcquiredChunkEvent>,
}

/// At most eight current records per source, with no request or key history.
/// Successful offers transfer ownership to authority without releasing its lane.
/// Dropping this value establishes neither cancellation nor provider quiescence.
#[derive(Default)]
pub struct ChunkDriver {
    loads: BTreeMap<ChunkKey, Record>,
    generations: BTreeMap<ChunkKey, Record>,
    stopped: bool,
    fault: Option<ServerError>,
}
impl ChunkDriver {
    pub fn new() -> Self {
        Self::default()
    }
    /// Counts all load ownership, including held bodies and unbound quarantine.
    pub fn pending_loads(&self) -> usize {
        self.loads.len()
    }
    /// Counts all CPU ownership, including held bodies and unbound quarantine.
    pub fn pending_generations(&self) -> usize {
        self.generations.len()
    }
    pub fn held_completions(&self) -> usize {
        self.loads
            .values()
            .chain(self.generations.values())
            .filter(|r| r.event.is_some())
            .count()
    }
    /// The first permanent correlation fault; transient refusals are report-local.
    pub fn last_error(&self) -> Option<ServerError> {
        self.fault
    }
    /// Stops new starts only. Bound work may still offer its final Closing batch.
    pub fn stop_new(&mut self) {
        self.stopped = true;
    }
    fn check_start(
        &self,
        records: &BTreeMap<ChunkKey, Record>,
        key: ChunkKey,
    ) -> Result<(), ServerError> {
        if self.stopped {
            return Err(ServerError::InvalidState {
                phase: ServerPhase::Closing,
            });
        }
        if let Some(error) = self.fault {
            return Err(error);
        }
        if records.len() == SOURCE_LIMIT {
            return Err(ServerError::Capacity {
                resource: Resource::ChunkRequests,
                limit: SOURCE_LIMIT,
                observed: SOURCE_LIMIT + 1,
            });
        }
        if self.loads.contains_key(&key) || self.generations.contains_key(&key) {
            return Err(ServerError::InvalidInput {
                field: "chunk_driver_request",
            });
        }
        Ok(())
    }
    /// Reserves before starting the borrowed load port, retaining every accepted
    /// start before binding. No callback intervenes in this serial sequence.
    pub fn start_load(
        &mut self,
        authority: &mut AuthorityState,
        loads: &mut dyn ChunkLoadPort,
        key: ChunkKey,
        deadline: Deadline,
    ) -> Result<ChunkRequestId, ServerError> {
        self.check_start(&self.loads, key)?;
        let reservation = authority.reserve_chunk_load(key)?;
        let generation = reservation.generation();
        let request = match loads.start_chunk(key, generation, deadline) {
            Ok(request) => request,
            Err(error) => {
                if let Err(refusal) = authority.abort_chunk_load(reservation, error) {
                    self.loads.insert(
                        key,
                        Record {
                            key,
                            generation,
                            request: None,
                            phase: Phase::Quarantined {
                                start_error: Some(error),
                            },
                            event: None,
                        },
                    );
                    self.fault.get_or_insert(refusal);
                    return Err(refusal);
                }
                return Err(error);
            }
        };
        self.loads.insert(
            key,
            Record {
                key,
                generation,
                request: Some(request),
                phase: Phase::Quarantined { start_error: None },
                event: None,
            },
        );
        if let Err(error) = authority.bind_chunk_load(reservation, request) {
            // A duplicate alias may name another live request. Never cancel,
            // retry binding or poll this ambiguous start as a second target.
            self.fault.get_or_insert(error);
            return Err(error);
        }
        self.loads.get_mut(&key).expect("retained start").phase = Phase::Bound;
        Ok(request)
    }
    /// Starts the independent CPU owner only after authority permits generation.
    pub fn start_generation(
        &mut self,
        authority: &mut AuthorityState,
        generations: &mut dyn GenerationPort,
        key: ChunkKey,
    ) -> Result<ChunkRequestId, ServerError> {
        self.check_start(&self.generations, key)?;
        let reservation = authority.reserve_chunk_generation(key)?;
        let generation = reservation.generation();
        let request = match generations.start_generation(key, generation) {
            Ok(request) => request,
            Err(error) => {
                if let Err(refusal) = authority.abort_chunk_generation(reservation, error) {
                    self.generations.insert(
                        key,
                        Record {
                            key,
                            generation,
                            request: None,
                            phase: Phase::Quarantined {
                                start_error: Some(error),
                            },
                            event: None,
                        },
                    );
                    self.fault.get_or_insert(refusal);
                    return Err(refusal);
                }
                return Err(error);
            }
        };
        self.generations.insert(
            key,
            Record {
                key,
                generation,
                request: Some(request),
                phase: Phase::Quarantined { start_error: None },
                event: None,
            },
        );
        if let Err(error) = authority.bind_chunk_generation(reservation, request) {
            self.fault.get_or_insert(error);
            return Err(error);
        }
        self.generations
            .get_mut(&key)
            .expect("retained start")
            .phase = Phase::Bound;
        Ok(request)
    }
    /// Visits at most sixteen records in Load/Generated, then ChunkKey order.
    /// Each bound request polls once and each held event offers once per pass.
    pub fn poll(
        &mut self,
        authority: &mut AuthorityState,
        loads: &mut dyn ChunkLoadPort,
        generations: &mut dyn GenerationPort,
    ) -> ChunkPollReport {
        let mut report = ChunkPollReport::default();
        self.loads.retain(|_, record| {
            visit(
                record,
                authority,
                &mut report,
                &mut self.fault,
                |key, generation, request| match loads.poll_chunk(request) {
                    ChunkLoadPoll::Pending => None,
                    ChunkLoadPoll::Loaded(result) => Some(AcquiredChunkEvent::Load {
                        key,
                        generation,
                        request,
                        result: Ok(result),
                    }),
                    ChunkLoadPoll::Failed(error) => Some(AcquiredChunkEvent::Load {
                        key,
                        generation,
                        request,
                        result: Err(error),
                    }),
                },
            )
        });
        self.generations.retain(|_, record| {
            visit(
                record,
                authority,
                &mut report,
                &mut self.fault,
                |key, generation, request| match generations.poll_generation(request) {
                    GenerationPoll::Pending => None,
                    GenerationPoll::Ready(result) => Some(AcquiredChunkEvent::Generated {
                        key,
                        generation,
                        request,
                        result: Ok(result),
                    }),
                    GenerationPoll::Failed(error) => Some(AcquiredChunkEvent::Generated {
                        key,
                        generation,
                        request,
                        result: Err(error),
                    }),
                },
            )
        });
        report.retained = self.loads.len() + self.generations.len();
        report
    }
}
fn visit(
    record: &mut Record,
    authority: &mut AuthorityState,
    report: &mut ChunkPollReport,
    fault: &mut Option<ServerError>,
    poll: impl FnOnce(ChunkKey, u64, ChunkRequestId) -> Option<AcquiredChunkEvent>,
) -> bool {
    if let Phase::Quarantined { start_error } = record.phase {
        // Retain the bounded original refusal without retrying an unbound start.
        let _ = start_error;
        return true;
    }
    if record.event.is_none() {
        report.polled += 1;
        record.event = poll(
            record.key,
            record.generation,
            record.request.expect("bound request"),
        );
        if record.event.is_none() {
            report.pending += 1;
            return true;
        }
    }
    let event = record.event.take().expect("held completion");
    match authority.offer_acquired(event) {
        Ok(()) => {
            report.offered += 1;
            false
        }
        Err(rejected) => {
            report.first_error.get_or_insert(rejected.error);
            if !matches!(
                rejected.error,
                ServerError::Capacity {
                    resource: Resource::ChunkResults,
                    ..
                } | ServerError::InvalidState { .. }
            ) {
                fault.get_or_insert(rejected.error);
            }
            record.event = Some(rejected.event);
            true
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::{contracts::*, generation_worker::GenerationPool, state::TickContext, world};
    use crate::store::{
        disk::{DiskOptions, DiskStore},
        mailbox::StoreMailbox,
        scheduler::{AutosaveScheduler, SchedulerConfig},
    };
    use mornlea_domain::{BlockPos, ChunkPos, Dimension};
    use mornlea_storage::{Chunk, ChunkSave, ContainerSnapshot, StorageKind};
    use std::{
        fs,
        path::PathBuf,
        thread,
        time::{Duration, Instant, SystemTime, UNIX_EPOCH},
    };
    fn key(x: i32) -> ChunkKey {
        ChunkKey {
            dimension: Dimension::OVERWORLD,
            pos: ChunkPos::new(x, 0),
        }
    }
    fn authority() -> AuthorityState {
        AuthorityState::try_new(
            ServerLimits::try_new(8, 4096, 512, 64, 64, 1_048_576).unwrap(),
            42,
        )
        .unwrap()
    }
    fn deadline() -> Deadline {
        Deadline::after(Instant::now(), Duration::from_secs(10)).unwrap()
    }
    struct Root(PathBuf);
    impl Drop for Root {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    struct Owners {
        store: AutosaveScheduler<DiskStore>,
        pool: GenerationPool,
        closed: bool,
    }
    impl Owners {
        fn close(&mut self) -> (Result<(), ServerError>, Result<(), ServerError>) {
            let disk = self.store.close(deadline());
            let cpu = self.pool.close(deadline());
            self.closed = disk.is_ok() && cpu.is_ok();
            (disk, cpu)
        }
    }
    impl Drop for Owners {
        fn drop(&mut self) {
            if !self.closed {
                let _ = self.close();
            }
        }
    }
    #[test]
    fn actual_disk_start_poll_offer_and_acquire_preserve_body_without_cloning_or_materializing() {
        let root = Root(std::env::temp_dir().join(format!(
                "mornlea-driver-counters-{}-{}",
                std::process::id(),
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            )));
        let mut state = authority();
        let SaveValue::Metadata(metadata) = state.metadata_snapshot().value else {
            panic!("metadata")
        };
        let mut setup = authority();
        let mut ctx = TickContext::harness(&mut setup, TickBudget::full());
        let mut saved = Chunk {
            sections: vec![
                ContainerSnapshot {
                    kind: StorageKind::Single,
                    bits: 0,
                    single: 0,
                    palette: vec![],
                    packed: vec![]
                };
                24
            ],
            drops: vec![Default::default(); 32],
            furnaces: vec![Default::default(); 32],
            chests: vec![Default::default(); 16],
        };
        saved.drops[31].generation = 19;
        saved.furnaces[31].generation = 20;
        saved.chests[15].generation = 21;
        ctx.preload_ready_chunk(world::ReadyChunk::try_new(key(0), 1, 8, saved).unwrap());
        for (pos, block) in [(BlockPos::new(1, 1, 1), 11), (BlockPos::new(2, 80, 2), 9)] {
            let observed = ctx.read().observation(Dimension::OVERWORLD, pos).unwrap();
            ctx.transaction()
                .try_system(
                    SystemRule::Support,
                    vec![BlockWrite::try_new(observed, block).unwrap()],
                )
                .unwrap();
        }
        let expected = ctx.resident_snapshot().ready_snapshot().remove(0).3;
        drop(ctx);
        let store = AutosaveScheduler::try_new(
            SchedulerConfig::default(),
            StoreMailbox::try_new_background(
                StoreLimits::try_new(2, 16, 3, 3, 3, 1, 8, 4_194_304).unwrap(),
                DiskStore::open(
                    &root.0,
                    DiskOptions {
                        create: metadata,
                        region_handle_cap: 1,
                    },
                )
                .unwrap(),
            )
            .unwrap(),
        )
        .unwrap();
        let mut owners = Owners {
            store,
            pool: GenerationPool::try_new(42, false, 1).unwrap(),
            closed: false,
        };
        let ticket = owners
            .store
            .submit(SaveRequest {
                snapshots: vec![
                    OwnedSnapshot::try_new(
                        SaveKey::Chunk(key(0)),
                        9,
                        1,
                        SaveUrgency::Autosave,
                        SaveValue::Chunk(ChunkSave {
                            key: mornlea_storage::ChunkKey {
                                dimension: 0,
                                x: 0,
                                z: 0,
                            },
                            revision: 9,
                            chunk: expected.clone(),
                        }),
                    )
                    .unwrap(),
                ],
            })
            .unwrap();
        let until = deadline();
        let completion = loop {
            owners.store.drive_workers();
            if let SavePoll::Completed(v) = StoreHandle::poll(&mut owners.store, ticket) {
                break v;
            }
            assert!(!until.expired(Instant::now()));
            thread::yield_now();
        };
        state.enable_live_chunks().unwrap();
        state.replace_chunk_wants([key(0)].into()).unwrap();
        world::reset_ready_clones();
        world::reset_materializations();
        let mut driver = ChunkDriver::new();
        driver
            .start_load(&mut state, &mut owners.store, key(0), deadline())
            .unwrap();
        let after_start = (world::ready_clones(), world::materializations());
        let until = deadline();
        let offered = loop {
            owners.store.drive_workers();
            let report = driver.poll(&mut state, &mut owners.store, &mut owners.pool);
            if report.offered == 1 {
                break report;
            }
            assert!(!until.expired(Instant::now()));
            thread::yield_now();
        };
        let after_offer = (world::ready_clones(), world::materializations());
        let tick = state.advance_tick(TickBudget::full());
        let after_tick = (world::ready_clones(), world::materializations());
        let (disk_closed, cpu_closed) = owners.close();
        disk_closed.unwrap();
        cpu_closed.unwrap();
        tick.unwrap();
        assert!(completion.error.is_none());
        assert_eq!(offered.retained, 0);
        assert_eq!(
            (after_start, after_offer, after_tick),
            ((0, 0), (0, 0), (0, 0))
        );
        let mut ctx = TickContext::for_tick(&mut state, TickBudget::full());
        let read = ctx.read();
        assert_eq!(
            read.block(Dimension::OVERWORLD, BlockPos::new(1, 1, 1)),
            Some(11)
        );
        assert_eq!(
            read.block(Dimension::OVERWORLD, BlockPos::new(2, 80, 2)),
            Some(9)
        );
        assert_eq!(read.highest_non_air(Dimension::OVERWORLD, 1, 1), Some(1));
        assert_eq!(read.highest_non_air(Dimension::OVERWORLD, 2, 2), Some(80));
        assert_eq!(read.container_refs(key(0)).len(), 2);
        ctx.commit_carried();
        drop(ctx);
        let SaveValue::ChunkView(view) = state
            .capture_chunk_snapshot(key(0), SaveUrgency::Unload)
            .unwrap()
            .value
        else {
            panic!("capture")
        };
        assert_eq!(view, view.clone());
        assert_eq!(world::ready_clones(), 0);
        assert_eq!(world::materializations(), 0);
        assert_eq!(view.materialize().chunk, expected);
    }

    struct FaultLoad {
        body: Option<world::PreparedChunk>,
        polls: usize,
    }
    impl ChunkLoadPort for FaultLoad {
        fn start_chunk(
            &mut self,
            _: ChunkKey,
            _: u64,
            _: Deadline,
        ) -> Result<ChunkRequestId, ServerError> {
            ChunkRequestId::try_new(1)
        }
        fn poll_chunk(&mut self, _: ChunkRequestId) -> ChunkLoadPoll {
            self.polls += 1;
            ChunkLoadPoll::Loaded(self.body.take())
        }
        fn cancel_chunk(&mut self, _: ChunkRequestId) -> Result<(), ServerError> {
            panic!("ambiguous ownership cannot cancel")
        }
    }
    struct NeverGeneration;
    impl GenerationPort for NeverGeneration {
        fn start_generation(&mut self, _: ChunkKey, _: u64) -> Result<ChunkRequestId, ServerError> {
            panic!("no implicit generation")
        }
        fn poll_generation(&mut self, _: ChunkRequestId) -> GenerationPoll {
            panic!("no generation request")
        }
        fn cancel_generation(&mut self, _: ChunkRequestId) -> Result<(), ServerError> {
            panic!("no implicit cancellation")
        }
    }
    #[test]
    fn refused_identity_retains_original_body_and_request_without_cloning() {
        let mut state = authority();
        state.enable_live_chunks().unwrap();
        state.replace_chunk_wants([key(0)].into()).unwrap();
        let mut original = Chunk {
            sections: vec![
                ContainerSnapshot {
                    kind: StorageKind::Single,
                    bits: 0,
                    single: 2,
                    palette: vec![],
                    packed: vec![]
                };
                24
            ],
            drops: vec![Default::default(); 32],
            furnaces: vec![Default::default(); 32],
            chests: vec![Default::default(); 16],
        };
        original.drops[31].generation = 19;
        original.furnaces[31].generation = 20;
        original.chests[15].generation = 21;
        let body = world::PreparedChunk::try_new(
            key(99),
            17,
            RecoveredChunk {
                chunk: original.clone(),
                revision: 9,
                persisted_revision: 7,
                needs_rewrite: true,
                recovered: true,
            },
        )
        .unwrap();
        let mut loads = FaultLoad {
            body: Some(body),
            polls: 0,
        };
        let mut driver = ChunkDriver::new();
        world::reset_ready_clones();
        world::reset_materializations();
        let request = driver
            .start_load(&mut state, &mut loads, key(0), deadline())
            .unwrap();
        let first = driver.poll(&mut state, &mut loads, &mut NeverGeneration);
        let second = driver.poll(&mut state, &mut loads, &mut NeverGeneration);
        assert_eq!((first.polled, second.polled, loads.polls), (1, 0, 1));
        assert_eq!((world::ready_clones(), world::materializations()), (0, 0));
        let record = driver.loads.remove(&key(0)).unwrap();
        assert_eq!(
            (record.key, record.generation, record.request),
            (key(0), 1, Some(request))
        );
        let Some(AcquiredChunkEvent::Load {
            key: outer,
            generation,
            request: id,
            result: Ok(Some(held)),
        }) = record.event
        else {
            panic!("original held owner")
        };
        assert_eq!((outer, generation, id), (key(0), 1, request));
        assert_eq!(
            (
                held.key(),
                held.generation(),
                held.revision(),
                held.persisted_revision(),
                held.needs_rewrite(),
                held.recovered()
            ),
            (key(99), 17, 9, 7, true, true)
        );
        assert_eq!(
            held.into_parts().0.capture(None, None).materialize().chunk,
            original
        );
    }
}
