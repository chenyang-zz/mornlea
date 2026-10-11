//! Bounded CPU generation ownership separate from the authoritative tick.

use std::collections::VecDeque;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::mpsc::{self, Receiver, SyncSender, TryRecvError, TrySendError};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use super::contracts::{
    ChunkKey, ChunkRequestId, Deadline, GenerationPoll, GenerationPort, Operation, RecoveredChunk,
    Resource, ServerError, ServerPhase, WorkerLifecycle,
};
use super::generation::ChunkGenerator;
use super::world::PreparedChunk;

const MAX_REQUESTS: usize = 8;

// This private CPU dependency permits causal unit tests; public construction
// always uses the accepted native generator with exclusively owned scratch.
trait GenerateOperation: Send {
    fn generate(&mut self, key: ChunkKey, generation: u64) -> Result<PreparedChunk, ServerError>;
}

struct Native(ChunkGenerator);
impl GenerateOperation for Native {
    fn generate(&mut self, key: ChunkKey, generation: u64) -> Result<PreparedChunk, ServerError> {
        let chunk = self.0.generate(key)?;
        PreparedChunk::try_new(
            key,
            generation,
            RecoveredChunk {
                chunk,
                revision: 1,
                persisted_revision: 0,
                needs_rewrite: false,
                recovered: false,
            },
        )
    }
}

struct Command {
    key: ChunkKey,
    generation: u64,
    reply: SyncSender<Result<PreparedChunk, ServerError>>,
}

struct Worker {
    sender: Option<SyncSender<Command>>,
    join: Option<JoinHandle<()>>,
    request: Option<ChunkRequestId>,
}

// Large completed records remain whole in the same capped ownership lane.
#[allow(clippy::large_enum_variant)]
enum RequestState {
    Queued,
    Started {
        worker: usize,
        reply: Receiver<Result<PreparedChunk, ServerError>>,
        cancelled: bool,
    },
    Complete(Result<PreparedChunk, ServerError>),
}
struct Request {
    id: ChunkRequestId,
    key: ChunkKey,
    generation: u64,
    state: RequestState,
}

/// Owns at most eight generation requests and one job per native CPU thread.
/// Admission and completion collection only move bounded ownership records;
/// explicit successful close is the boundary that proves every thread joined.
pub struct GenerationPool {
    workers: Vec<Worker>,
    requests: VecDeque<Request>,
    last_request: u64,
    stopping: bool,
    closed: bool,
    join_failed: bool,
}

impl GenerationPool {
    /// Builds independent native scratch off the tick before spawning owners.
    pub fn try_new(seed: i64, fluid_enabled: bool, workers: usize) -> Result<Self, ServerError> {
        if !(1..=2).contains(&workers) {
            return Err(ServerError::InvalidInput {
                field: "generation_workers",
            });
        }
        let operations = (0..workers)
            .map(|_| {
                ChunkGenerator::try_new(seed, fluid_enabled)
                    .map(|generator| Box::new(Native(generator)) as Box<dyn GenerateOperation>)
            })
            .collect::<Result<Vec<_>, _>>()?;
        Self::from_operations(operations)
    }

    fn from_operations(operations: Vec<Box<dyn GenerateOperation>>) -> Result<Self, ServerError> {
        let mut pool = Self {
            workers: Vec::with_capacity(operations.len()),
            requests: VecDeque::with_capacity(MAX_REQUESTS),
            last_request: 0,
            stopping: false,
            closed: false,
            join_failed: false,
        };
        for mut operation in operations {
            let (sender, commands) = mpsc::sync_channel::<Command>(1);
            let spawned = thread::Builder::new()
                .name("chunk-generation".into())
                .spawn(move || {
                    while let Ok(command) = commands.recv() {
                        // A CPU failure belongs to this request and must not kill
                        // the reusable owner or invent a successfully prepared base.
                        let result = catch_unwind(AssertUnwindSafe(|| {
                            let prepared = operation.generate(command.key, command.generation)?;
                            if prepared.key() != command.key
                                || prepared.generation() != command.generation
                            {
                                return Err(ServerError::InvalidInput {
                                    field: "generation result",
                                });
                            }
                            Ok(prepared)
                        }))
                        .unwrap_or(Err(ServerError::Internal {
                            invariant: "generation owner panic",
                        }));
                        let _ = command.reply.send(result);
                    }
                });
            match spawned {
                Ok(join) => pool.workers.push(Worker {
                    sender: Some(sender),
                    join: Some(join),
                    request: None,
                }),
                Err(error) => {
                    // Construction has admitted nothing: disconnect and join
                    // all prior idle owners before returning a spawn refusal.
                    for worker in &mut pool.workers {
                        worker.sender.take();
                    }
                    for worker in &mut pool.workers {
                        if let Some(join) = worker.join.take() {
                            let _ = join.join();
                        }
                    }
                    return Err(ServerError::Io {
                        operation: Operation::Load,
                        kind: error.kind(),
                    });
                }
            }
        }
        Ok(pool)
    }

    /// Collects available replies, fills free owners in FIFO order, and collects
    /// once more. No generation or preparation runs on this caller.
    pub fn drive(&mut self) -> usize {
        let collected = self.collect();
        self.dispatch();
        collected + self.collect()
    }
    pub fn queued_jobs(&self) -> usize {
        self.requests
            .iter()
            .filter(|r| matches!(r.state, RequestState::Queued))
            .count()
    }
    pub fn worker_jobs(&self) -> usize {
        self.requests
            .iter()
            .filter(|r| matches!(r.state, RequestState::Started { .. }))
            .count()
    }
    pub fn held_completions(&self) -> usize {
        self.requests
            .iter()
            .filter(|r| matches!(r.state, RequestState::Complete(_)))
            .count()
    }
    pub fn owned_jobs(&self) -> usize {
        self.requests.len()
    }

    fn collect(&mut self) -> usize {
        let mut collected = 0;
        let mut index = 0;
        while index < self.requests.len() {
            let completion = match &self.requests[index].state {
                RequestState::Started {
                    worker,
                    reply,
                    cancelled,
                } => match reply.try_recv() {
                    Ok(result) => Some((*worker, *cancelled, result)),
                    Err(TryRecvError::Disconnected) => Some((
                        *worker,
                        *cancelled,
                        Err(ServerError::Internal {
                            invariant: "generation owner disconnected",
                        }),
                    )),
                    Err(TryRecvError::Empty) => None,
                },
                _ => None,
            };
            if let Some((worker, cancelled, result)) = completion {
                self.workers[worker].request = None;
                collected += 1;
                if cancelled {
                    self.requests.remove(index);
                    continue;
                }
                self.requests[index].state = RequestState::Complete(result);
            }
            index += 1;
        }
        collected
    }

    fn dispatch(&mut self) {
        for (worker_index, worker) in self.workers.iter_mut().enumerate() {
            if worker.request.is_some() {
                continue;
            }
            let Some(request) = self
                .requests
                .iter_mut()
                .find(|r| matches!(r.state, RequestState::Queued))
            else {
                break;
            };
            let (reply, receiver) = mpsc::sync_channel(1);
            let command = Command {
                key: request.key,
                generation: request.generation,
                reply,
            };
            let sent = worker
                .sender
                .as_ref()
                .map(|sender| sender.try_send(command));
            match sent {
                Some(Ok(())) => {
                    worker.request = Some(request.id);
                    request.state = RequestState::Started {
                        worker: worker_index,
                        reply: receiver,
                        cancelled: false,
                    };
                }
                Some(Err(TrySendError::Full(_))) => break,
                Some(Err(TrySendError::Disconnected(_))) | None => {
                    request.state = RequestState::Complete(Err(ServerError::Internal {
                        invariant: "generation owner disconnected",
                    }));
                }
            }
        }
    }

    fn wait_for_work(
        &mut self,
        deadline: Deadline,
        operation: Operation,
    ) -> Result<(), ServerError> {
        loop {
            self.drive();
            if self.queued_jobs() == 0 && self.worker_jobs() == 0 {
                return Ok(());
            }
            pause(deadline, operation)?;
        }
    }
}

impl GenerationPort for GenerationPool {
    fn start_generation(
        &mut self,
        key: ChunkKey,
        generation: u64,
    ) -> Result<ChunkRequestId, ServerError> {
        if self.stopping {
            return Err(ServerError::InvalidState {
                phase: if self.closed {
                    ServerPhase::Closed
                } else {
                    ServerPhase::Closing
                },
            });
        }
        if generation == 0 {
            return Err(ServerError::InvalidInput {
                field: "chunk_generation",
            });
        }
        if self.requests.len() == MAX_REQUESTS {
            return Err(ServerError::Capacity {
                resource: Resource::ChunkResults,
                limit: MAX_REQUESTS,
                observed: MAX_REQUESTS + 1,
            });
        }
        let next = self
            .last_request
            .checked_add(1)
            .ok_or(ServerError::Internal {
                invariant: "generation request space",
            })?;
        let id = ChunkRequestId::try_new(next)?;
        self.requests.push_back(Request {
            id,
            key,
            generation,
            state: RequestState::Queued,
        });
        self.last_request = next;
        self.dispatch();
        Ok(id)
    }

    fn poll_generation(&mut self, request: ChunkRequestId) -> GenerationPoll {
        self.drive();
        let Some(index) = self
            .requests
            .iter()
            .position(|r| r.id == request && matches!(r.state, RequestState::Complete(_)))
        else {
            return GenerationPoll::Pending;
        };
        match self.requests.remove(index).map(|r| r.state) {
            Some(RequestState::Complete(Ok(prepared))) => GenerationPoll::Ready(prepared),
            Some(RequestState::Complete(Err(error))) => GenerationPoll::Failed(error),
            _ => unreachable!("selected a completed request"),
        }
    }

    fn cancel_generation(&mut self, request: ChunkRequestId) -> Result<(), ServerError> {
        let Some(index) = self.requests.iter().position(|r| r.id == request) else {
            return Ok(());
        };
        if let RequestState::Started { cancelled, .. } = &mut self.requests[index].state {
            *cancelled = true;
        } else {
            self.requests.remove(index);
        }
        Ok(())
    }
}

impl WorkerLifecycle for GenerationPool {
    fn stop_new(&mut self) -> Result<(), ServerError> {
        self.stopping = true;
        Ok(())
    }
    fn cancel(&mut self) -> Result<(), ServerError> {
        let mut index = 0;
        while index < self.requests.len() {
            if let RequestState::Started { cancelled, .. } = &mut self.requests[index].state {
                *cancelled = true;
                index += 1;
            } else {
                self.requests.remove(index);
            }
        }
        Ok(())
    }
    fn wait(&mut self, deadline: Deadline) -> Result<(), ServerError> {
        self.wait_for_work(deadline, Operation::Shutdown)
    }
    fn close(&mut self, deadline: Deadline) -> Result<(), ServerError> {
        if self.closed {
            return Ok(());
        }
        self.stop_new()?;
        self.cancel()?;
        self.wait_for_work(deadline, Operation::Close)?;
        for worker in &mut self.workers {
            worker.sender.take();
        }
        // Finished checks precede every consuming join. A deadline refusal
        // retains all remaining handles for retry on this same pool.
        for worker in &mut self.workers {
            if let Some(join) = worker.join.as_ref() {
                while !join.is_finished() {
                    pause(deadline, Operation::Close)?;
                }
            }
            if let Some(join) = worker.join.take()
                && join.join().is_err()
            {
                self.join_failed = true;
            }
        }
        if self.join_failed {
            return Err(ServerError::Internal {
                invariant: "generation owner join",
            });
        }
        self.closed = true;
        Ok(())
    }
}

impl Drop for GenerationPool {
    fn drop(&mut self) {
        // Drop disconnects owners but makes no quiescence or join claim.
        for worker in &mut self.workers {
            worker.sender.take();
        }
    }
}

fn pause(deadline: Deadline, operation: Operation) -> Result<(), ServerError> {
    let remaining = deadline.instant().saturating_duration_since(Instant::now());
    if remaining.is_zero() {
        return Err(ServerError::Timeout { operation });
    }
    thread::sleep(remaining.min(Duration::from_millis(1)));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use mornlea_domain::{ChunkPos, Dimension};
    use std::sync::{Arc, Condvar, Mutex};

    fn key(x: i32) -> ChunkKey {
        ChunkKey {
            dimension: Dimension::OVERWORLD,
            pos: ChunkPos::new(x, 0),
        }
    }
    fn deadline() -> Deadline {
        Deadline::at(Instant::now() + Duration::from_secs(5))
    }
    fn expired() -> Deadline {
        Deadline::at(Instant::now())
    }
    type Entry = (thread::ThreadId, ChunkKey, u64);
    type Gate = Arc<(Mutex<bool>, Condvar)>;
    fn release(gate: &Gate) {
        *gate.0.lock().unwrap() = true;
        gate.1.notify_all();
    }
    fn held_ok<T, E: std::fmt::Debug>(result: Result<T, E>, gate: &Gate) -> T {
        if result.is_err() {
            release(gate);
        }
        result.expect("held operation returned successfully")
    }
    struct ReleaseOnDrop(Gate);
    impl Drop for ReleaseOnDrop {
        fn drop(&mut self) {
            release(&self.0);
        }
    }
    struct HeldActualCpuOperation {
        native: Native,
        gate: Gate,
        entered: mpsc::Sender<Entry>,
    }
    impl GenerateOperation for HeldActualCpuOperation {
        fn generate(
            &mut self,
            key: ChunkKey,
            generation: u64,
        ) -> Result<PreparedChunk, ServerError> {
            self.entered
                .send((thread::current().id(), key, generation))
                .unwrap();
            let held = self.gate.0.lock().unwrap();
            drop(self.gate.1.wait_while(held, |open| !*open).unwrap());
            self.native.generate(key, generation)
        }
    }
    fn gated(workers: usize) -> (GenerationPool, Gate, mpsc::Receiver<Entry>) {
        let gate = Arc::new((Mutex::new(false), Condvar::new()));
        let (tx, rx) = mpsc::channel();
        let operations = (0..workers)
            .map(|_| {
                Box::new(HeldActualCpuOperation {
                    native: Native(ChunkGenerator::try_new(42, false).unwrap()),
                    gate: gate.clone(),
                    entered: tx.clone(),
                }) as Box<dyn GenerateOperation>
            })
            .collect();
        (
            GenerationPool::from_operations(operations).unwrap(),
            gate,
            rx,
        )
    }
    fn entered(rx: &mpsc::Receiver<Entry>, gate: &Gate) -> Entry {
        let result = rx.recv_timeout(Duration::from_secs(5));
        if result.is_err() {
            release(gate);
        }
        result.expect("actual CPU operation entered")
    }
    fn ready(pool: &mut GenerationPool, request: ChunkRequestId) -> PreparedChunk {
        pool.wait(deadline()).expect("actual completion");
        match pool.poll_generation(request) {
            GenerationPoll::Ready(value) => value,
            _ => panic!("expected actual prepared result"),
        }
    }

    #[test]
    fn held_actual_generation_returns_to_submitter_before_release() {
        let (pool, gate, entries) = gated(1);
        let (returned_tx, returned_rx) = mpsc::channel();
        let submitter_gate = gate.clone();
        let submitter = thread::spawn(move || {
            let mut pool = pool;
            let request = held_ok(pool.start_generation(key(0), 1), &submitter_gate);
            let driven = pool.drive();
            let pending = matches!(pool.poll_generation(request), GenerationPoll::Pending);
            returned_tx
                .send((pool, request, driven, pending, thread::current().id()))
                .ok();
        });
        let operation_thread = entered(&entries, &gate).0;
        let returned = returned_rx.recv_timeout(Duration::from_millis(50));
        release(&gate);
        submitter.join().expect("submitter joined");
        assert!(
            returned.is_ok(),
            "missing CPU handoff: synchronous actual generation holds submitter until gate release"
        );
        let (mut pool, request, driven, pending, submitter_thread) = returned.unwrap();
        assert_eq!(driven, 0);
        assert!(pending);
        assert_ne!(operation_thread, submitter_thread);
        assert_eq!(ready(&mut pool, request).key(), key(0));
        pool.close(deadline()).unwrap();
    }

    #[test]
    fn capacity_charges_queued_started_and_completed_independent_requests() {
        let (mut pool, gate, entries) = gated(1);
        let _cleanup = ReleaseOnDrop(gate.clone());
        let first = held_ok(pool.start_generation(key(0), 1), &gate);
        entered(&entries, &gate);
        let mut ids = vec![first];
        for _ in 1..8 {
            ids.push(held_ok(pool.start_generation(key(0), 1), &gate));
        }
        let initial = (
            pool.queued_jobs(),
            pool.worker_jobs(),
            pool.held_completions(),
            pool.owned_jobs(),
        );
        let ninth = pool.start_generation(key(0), 1);
        held_ok(pool.cancel_generation(ids[1]), &gate);
        let replacement = held_ok(pool.start_generation(key(0), 1), &gate);
        ids[1] = replacement;
        release(&gate);
        pool.wait(deadline()).unwrap();
        let completed = (
            pool.queued_jobs(),
            pool.worker_jobs(),
            pool.held_completions(),
            pool.owned_jobs(),
        );
        assert_eq!(initial, (7, 1, 0, 8));
        assert_eq!(
            ninth,
            Err(ServerError::Capacity {
                resource: Resource::ChunkResults,
                limit: 8,
                observed: 9
            })
        );
        assert_eq!(completed, (0, 0, 8, 8));
        assert_eq!(entries.try_iter().count(), 7);
        let again = pool.start_generation(key(0), 1);
        assert_eq!(again, ninth);
        for request in ids {
            assert_eq!(ready(&mut pool, request).generation(), 1);
        }
        assert_eq!(pool.owned_jobs(), 0);
        assert!(matches!(
            pool.poll_generation(first),
            GenerationPoll::Pending
        ));
        pool.close(deadline()).unwrap();
    }

    #[test]
    fn cancellation_suppresses_started_and_drops_completed_without_tombstones() {
        let (mut pool, gate, entries) = gated(1);
        let _cleanup = ReleaseOnDrop(gate.clone());
        let started = held_ok(pool.start_generation(key(0), 1), &gate);
        entered(&entries, &gate);
        let queued = held_ok(pool.start_generation(key(1), 2), &gate);
        held_ok(pool.cancel_generation(queued), &gate);
        held_ok(pool.cancel_generation(started), &gate);
        held_ok(pool.cancel_generation(started), &gate);
        let retained = (pool.worker_jobs(), pool.owned_jobs());
        let pending = matches!(pool.poll_generation(started), GenerationPoll::Pending);
        release(&gate);
        pool.wait(deadline()).unwrap();
        assert_eq!(retained, (1, 1));
        assert!(pending);
        assert!(entries.try_iter().next().is_none());
        assert_eq!(pool.owned_jobs(), 0);
        assert!(matches!(
            pool.poll_generation(started),
            GenerationPoll::Pending
        ));
        let complete = held_ok(pool.start_generation(key(2), 3), &gate);
        pool.wait(deadline()).unwrap();
        assert_eq!(pool.held_completions(), 1);
        held_ok(pool.cancel_generation(complete), &gate);
        held_ok(pool.cancel_generation(complete), &gate);
        held_ok(
            pool.cancel_generation(ChunkRequestId::try_new(987).unwrap()),
            &gate,
        );
        assert_eq!(pool.owned_jobs(), 0);
        pool.close(deadline()).unwrap();
    }

    #[test]
    fn expired_wait_and_close_retain_original_owner_until_real_reply_and_join() {
        let (mut pool, gate, entries) = gated(1);
        let _cleanup = ReleaseOnDrop(gate.clone());
        let request = held_ok(pool.start_generation(key(0), 1), &gate);
        let original = entered(&entries, &gate);
        let start = Instant::now();
        let waited = pool.wait(expired());
        let closed = pool.close(expired());
        let elapsed = start.elapsed();
        let ownership = (
            pool.worker_jobs(),
            pool.owned_jobs(),
            pool.workers[0].join.is_some(),
            pool.workers[0].sender.is_some(),
        );
        let refusal = pool.start_generation(key(0), 2);
        release(&gate);
        pool.close(deadline()).unwrap();
        assert_eq!(
            waited,
            Err(ServerError::Timeout {
                operation: Operation::Shutdown
            })
        );
        assert_eq!(
            closed,
            Err(ServerError::Timeout {
                operation: Operation::Close
            })
        );
        assert!(elapsed < Duration::from_millis(50));
        assert_eq!(ownership, (1, 1, true, true));
        assert_eq!(
            refusal,
            Err(ServerError::InvalidState {
                phase: ServerPhase::Closing
            })
        );
        assert_eq!(original.1, key(0));
        assert!(entries.try_iter().next().is_none());
        assert!(
            pool.workers
                .iter()
                .all(|w| w.join.is_none() && w.sender.is_none())
        );
        assert_eq!(pool.owned_jobs(), 0);
        assert!(matches!(
            pool.poll_generation(request),
            GenerationPoll::Pending
        ));
        assert_eq!(
            pool.start_generation(key(0), 1),
            Err(ServerError::InvalidState {
                phase: ServerPhase::Closed
            })
        );
        pool.close(expired()).unwrap();
    }

    #[test]
    fn two_workers_bound_fanout_and_reuse_original_owners() {
        let (mut pool, gate, entries) = gated(2);
        let _cleanup = ReleaseOnDrop(gate.clone());
        let a = held_ok(pool.start_generation(key(1), 1), &gate);
        let b = held_ok(pool.start_generation(key(2), 2), &gate);
        let owner_a = entered(&entries, &gate).0;
        let owner_b = entered(&entries, &gate).0;
        let c = held_ok(pool.start_generation(key(3), 3), &gate);
        let occupancy = (pool.worker_jobs(), pool.queued_jobs());
        let extra = entries.try_recv().is_ok();
        release(&gate);
        pool.wait(deadline()).unwrap();
        let owner_c = entries.recv_timeout(Duration::from_secs(5)).unwrap().0;
        assert_ne!(owner_a, owner_b);
        assert_eq!(occupancy, (2, 1));
        assert!(!extra);
        assert!([owner_a, owner_b].contains(&owner_c));
        for (id, expected) in [(a, key(1)), (b, key(2)), (c, key(3))] {
            assert_eq!(ready(&mut pool, id).key(), expected);
        }
        pool.close(deadline()).unwrap();
    }

    #[test]
    fn invalid_inputs_and_request_overflow_preserve_occupancy() {
        for count in [0, 3, usize::MAX] {
            assert!(matches!(
                GenerationPool::try_new(42, false, count),
                Err(ServerError::InvalidInput {
                    field: "generation_workers"
                })
            ));
        }
        let mut pool = GenerationPool::try_new(42, false, 1).unwrap();
        assert_eq!(
            pool.start_generation(key(0), 0),
            Err(ServerError::InvalidInput {
                field: "chunk_generation"
            })
        );
        assert_eq!(pool.owned_jobs(), 0);
        assert_eq!(pool.last_request, 0);
        pool.last_request = u64::MAX;
        assert_eq!(
            pool.start_generation(key(0), 1),
            Err(ServerError::Internal {
                invariant: "generation request space"
            })
        );
        assert_eq!(pool.owned_jobs(), 0);
        pool.stop_new().unwrap();
        pool.stop_new().unwrap();
        assert_eq!(
            pool.start_generation(key(0), 1),
            Err(ServerError::InvalidState {
                phase: ServerPhase::Closing
            })
        );
        pool.close(deadline()).unwrap();
    }

    struct ScriptedActual {
        native: Native,
        attempt: usize,
    }
    impl GenerateOperation for ScriptedActual {
        fn generate(&mut self, k: ChunkKey, generation: u64) -> Result<PreparedChunk, ServerError> {
            self.attempt += 1;
            match self.attempt {
                1 => panic!("injected CPU panic"),
                2 => Err(ServerError::Internal {
                    invariant: "injected generation error",
                }),
                3 => self.native.generate(k, generation + 1),
                4 => self.native.generate(key(99), generation),
                _ => self.native.generate(k, generation),
            }
        }
    }
    #[test]
    fn panic_error_and_wrong_identity_fail_only_original_job_then_native_succeeds() {
        let mut pool = GenerationPool::from_operations(vec![Box::new(ScriptedActual {
            native: Native(ChunkGenerator::try_new(42, false).unwrap()),
            attempt: 0,
        })])
        .unwrap();
        for expected in [
            ServerError::Internal {
                invariant: "generation owner panic",
            },
            ServerError::Internal {
                invariant: "injected generation error",
            },
            ServerError::InvalidInput {
                field: "generation result",
            },
            ServerError::InvalidInput {
                field: "generation result",
            },
        ] {
            let request = pool.start_generation(key(0), 7).unwrap();
            pool.wait(deadline()).unwrap();
            assert!(
                matches!(pool.poll_generation(request), GenerationPoll::Failed(error) if error == expected)
            );
            assert_eq!(pool.owned_jobs(), 0);
        }
        let actual = pool.start_generation(key(0), 8).unwrap();
        assert_eq!(ready(&mut pool, actual).generation(), 8);
        pool.close(deadline()).unwrap();
    }

    #[test]
    fn full_command_channel_retains_fifo_queue_and_wait_refuses_false_quiescence() {
        let mut pool = GenerationPool::try_new(42, false, 1).unwrap();
        let (sender, commands) = mpsc::sync_channel(1);
        let (reply, _reply_rx) = mpsc::sync_channel(1);
        sender
            .try_send(Command {
                key: key(9),
                generation: 9,
                reply,
            })
            .unwrap();
        pool.workers[0].sender = Some(sender);
        let first = pool.start_generation(key(1), 1).unwrap();
        let second = pool.start_generation(key(2), 2).unwrap();
        assert_eq!(
            (pool.queued_jobs(), pool.worker_jobs(), pool.owned_jobs()),
            (2, 0, 2)
        );
        assert_eq!(pool.drive(), 0);
        assert_eq!(
            pool.wait(expired()),
            Err(ServerError::Timeout {
                operation: Operation::Shutdown
            })
        );
        assert_eq!(pool.requests[0].id, first);
        assert_eq!(pool.requests[1].id, second);
        commands.recv().unwrap();
        pool.drive();
        let dispatched = commands.recv().unwrap();
        assert_eq!((dispatched.key, dispatched.generation), (key(1), 1));
        drop(dispatched.reply);
        pool.drive();
        let dispatched = commands.recv().unwrap();
        assert_eq!((dispatched.key, dispatched.generation), (key(2), 2));
        drop(dispatched.reply);
        pool.drive();
        for request in [first, second] {
            assert!(matches!(
                pool.poll_generation(request),
                GenerationPoll::Failed(ServerError::Internal {
                    invariant: "generation owner disconnected"
                })
            ));
        }
        assert_eq!(pool.owned_jobs(), 0);
        pool.close(deadline()).unwrap();
    }

    #[test]
    fn disconnected_command_channel_keeps_original_request_failure() {
        let mut pool = GenerationPool::try_new(42, false, 1).unwrap();
        let (sender, commands) = mpsc::sync_channel(1);
        drop(commands);
        pool.workers[0].sender = Some(sender);
        let request = pool.start_generation(key(4), 12).unwrap();
        assert_eq!(
            (
                pool.requests[0].id,
                pool.requests[0].key,
                pool.requests[0].generation
            ),
            (request, key(4), 12)
        );
        assert!(matches!(
            pool.poll_generation(request),
            GenerationPoll::Failed(ServerError::Internal {
                invariant: "generation owner disconnected"
            })
        ));
        assert!(matches!(
            pool.poll_generation(request),
            GenerationPoll::Pending
        ));
        pool.close(deadline()).unwrap();
    }

    struct HeldDrop {
        native: Native,
        gate: Gate,
        entered: mpsc::Sender<()>,
    }
    impl GenerateOperation for HeldDrop {
        fn generate(
            &mut self,
            key: ChunkKey,
            generation: u64,
        ) -> Result<PreparedChunk, ServerError> {
            self.native.generate(key, generation)
        }
    }
    impl Drop for HeldDrop {
        fn drop(&mut self) {
            self.entered.send(()).ok();
            let held = self.gate.0.lock().unwrap();
            drop(self.gate.1.wait_while(held, |open| !*open).unwrap());
        }
    }
    #[test]
    fn close_join_timeout_retains_disconnected_handle_and_retries_same_owner() {
        let gate = Arc::new((Mutex::new(false), Condvar::new()));
        let (entered_tx, entered_rx) = mpsc::channel();
        let mut pool = GenerationPool::from_operations(vec![Box::new(HeldDrop {
            native: Native(ChunkGenerator::try_new(42, false).unwrap()),
            gate: gate.clone(),
            entered: entered_tx,
        })])
        .unwrap();
        let _cleanup = ReleaseOnDrop(gate.clone());
        let closed = pool.close(Deadline::at(Instant::now() + Duration::from_millis(10)));
        let entered = entered_rx.recv_timeout(Duration::from_secs(5));
        let retained = (
            pool.workers[0].sender.is_none(),
            pool.workers[0].join.is_some(),
            pool.closed,
        );
        release(&gate);
        pool.close(deadline()).unwrap();
        assert!(entered.is_ok());
        assert_eq!(
            closed,
            Err(ServerError::Timeout {
                operation: Operation::Close
            })
        );
        assert_eq!(retained, (true, true, false));
        assert!(pool.workers[0].join.is_none());
        assert!(pool.closed);
    }

    struct PanicDrop(Native);
    impl GenerateOperation for PanicDrop {
        fn generate(
            &mut self,
            key: ChunkKey,
            generation: u64,
        ) -> Result<PreparedChunk, ServerError> {
            self.0.generate(key, generation)
        }
    }
    impl Drop for PanicDrop {
        fn drop(&mut self) {
            panic!("injected owner teardown panic");
        }
    }
    #[test]
    fn join_panic_retains_failed_boundary_and_never_marks_closed() {
        let mut pool = GenerationPool::from_operations(vec![Box::new(PanicDrop(Native(
            ChunkGenerator::try_new(42, false).unwrap(),
        )))])
        .unwrap();
        for _ in 0..2 {
            assert_eq!(
                pool.close(deadline()),
                Err(ServerError::Internal {
                    invariant: "generation owner join"
                })
            );
            assert!(!pool.closed);
            assert!(pool.join_failed);
            assert_eq!(
                pool.start_generation(key(0), 1),
                Err(ServerError::InvalidState {
                    phase: ServerPhase::Closing
                })
            );
        }
    }

    struct StepActual {
        native: Native,
        permits: mpsc::Receiver<()>,
        entered: mpsc::Sender<Entry>,
    }
    impl GenerateOperation for StepActual {
        fn generate(
            &mut self,
            key: ChunkKey,
            generation: u64,
        ) -> Result<PreparedChunk, ServerError> {
            self.entered
                .send((thread::current().id(), key, generation))
                .unwrap();
            // Disconnecting the test permit releases all remaining CPU work.
            self.permits.recv().ok();
            self.native.generate(key, generation)
        }
    }
    fn step_ok<T, E: std::fmt::Debug>(
        result: Result<T, E>,
        permit: &mut Option<mpsc::Sender<()>>,
    ) -> T {
        if result.is_err() {
            permit.take();
        }
        result.expect("stepped operation returned successfully")
    }
    #[test]
    fn mixed_completed_started_queued_occupancy_and_actual_fifo_are_bounded() {
        let (permit_tx, permit_rx) = mpsc::channel();
        let (entered_tx, entered_rx) = mpsc::channel();
        let mut pool = GenerationPool::from_operations(vec![Box::new(StepActual {
            native: Native(ChunkGenerator::try_new(42, false).unwrap()),
            permits: permit_rx,
            entered: entered_tx,
        })])
        .unwrap();
        let mut permit_tx = Some(permit_tx);
        let ids = (1..=8)
            .map(|generation| {
                step_ok(
                    pool.start_generation(key(generation as i32), generation),
                    &mut permit_tx,
                )
            })
            .collect::<Vec<_>>();
        let first = entered_rx.recv_timeout(Duration::from_secs(5));
        let sent = permit_tx.as_ref().unwrap().send(());
        step_ok(sent, &mut permit_tx);
        let until = Instant::now() + Duration::from_secs(5);
        while pool.held_completions() == 0 && Instant::now() < until {
            pool.drive();
            thread::sleep(Duration::from_millis(1));
        }
        let second = entered_rx.recv_timeout(Duration::from_secs(5));
        let occupancy = (
            pool.held_completions(),
            pool.worker_jobs(),
            pool.queued_jobs(),
            pool.owned_jobs(),
        );
        let refused = pool.start_generation(key(9), 9);
        drop(permit_tx);
        pool.wait(deadline()).unwrap();
        assert_eq!(occupancy, (1, 1, 6, 8));
        assert_eq!(
            refused,
            Err(ServerError::Capacity {
                resource: Resource::ChunkResults,
                limit: 8,
                observed: 9
            })
        );
        let mut observed = vec![first.unwrap().2, second.unwrap().2];
        observed.extend(entered_rx.try_iter().map(|entry| entry.2));
        assert_eq!(observed, (1..=8).collect::<Vec<_>>());
        for (index, id) in ids.into_iter().enumerate() {
            assert_eq!(ready(&mut pool, id).generation(), index as u64 + 1);
        }
        pool.close(deadline()).unwrap();
    }
}
