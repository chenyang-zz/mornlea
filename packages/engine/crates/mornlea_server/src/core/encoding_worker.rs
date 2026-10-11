//! Bounded off-tick chunk framing with independent codecs and retryable joins.

use std::collections::VecDeque;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::mpsc::{self, Receiver, SyncSender, TryRecvError, TrySendError};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use super::chunk_encoding::{
    ChunkEncodePoll, ChunkEncodePort, ChunkEncodeRequestId, EncodedChunkSnapshot,
    MAX_CHUNK_ENCODE_REQUESTS,
};
use super::contracts::{Deadline, Operation, Resource, ServerError, ServerPhase, WorkerLifecycle};
use super::world::ChunkSaveView;
use mornlea_protocol::ProtocolCodec;

// Public construction exclusively creates real codec owners; this private seam
// lets causal tests hold or fault an actual operation without a production bypass.
trait EncodeOperation: Send {
    fn encode(&mut self, capture: ChunkSaveView) -> Result<EncodedChunkSnapshot, ServerError>;
}
struct CodecOperation(ProtocolCodec);
impl EncodeOperation for CodecOperation {
    fn encode(&mut self, capture: ChunkSaveView) -> Result<EncodedChunkSnapshot, ServerError> {
        EncodedChunkSnapshot::encode(capture, &mut self.0)
    }
}
struct Command {
    capture: ChunkSaveView,
    reply: SyncSender<Result<EncodedChunkSnapshot, ServerError>>,
}

struct Worker {
    sender: Option<SyncSender<Command>>,
    join: Option<JoinHandle<()>>,
    request: Option<ChunkEncodeRequestId>,
}

// Large completed records remain whole in the same capped ownership lane.
#[allow(clippy::large_enum_variant)]
enum RequestState {
    Queued,
    Started {
        worker: usize,
        reply: Receiver<Result<EncodedChunkSnapshot, ServerError>>,
        cancelled: bool,
    },
    Complete(Result<EncodedChunkSnapshot, ServerError>),
}
struct Request {
    id: ChunkEncodeRequestId,
    capture: ChunkSaveView,
    state: RequestState,
}

/// Owns at most eight encoding requests and one job per codec CPU thread.
/// Admission and completion collection only move bounded ownership records;
/// explicit successful close is the boundary that proves every thread joined.
pub struct ChunkEncodingPool {
    workers: Vec<Worker>,
    requests: VecDeque<Request>,
    last_request: u64,
    stopping: bool,
    closed: bool,
    join_failed: bool,
}

impl ChunkEncodingPool {
    /// Builds independent codec scratch off the tick before spawning owners.
    pub fn try_new(workers: usize) -> Result<Self, ServerError> {
        if !(1..=2).contains(&workers) {
            return Err(ServerError::InvalidInput {
                field: "chunk_encode_workers",
            });
        }
        let operations = (0..workers)
            .map(|_| {
                ProtocolCodec::new()
                    .map(|codec| Box::new(CodecOperation(codec)) as Box<dyn EncodeOperation>)
                    .map_err(|_| ServerError::InvalidInput { field: "packet" })
            })
            .collect::<Result<Vec<_>, _>>()?;
        Self::from_operations(operations)
    }

    fn from_operations(operations: Vec<Box<dyn EncodeOperation>>) -> Result<Self, ServerError> {
        if !(1..=2).contains(&operations.len()) {
            return Err(ServerError::InvalidInput {
                field: "chunk_encode_workers",
            });
        }
        let mut pool = Self {
            workers: Vec::with_capacity(operations.len()),
            requests: VecDeque::with_capacity(MAX_CHUNK_ENCODE_REQUESTS),
            last_request: 0,
            stopping: false,
            closed: false,
            join_failed: false,
        };
        for mut operation in operations {
            let (sender, commands) = mpsc::sync_channel::<Command>(1);
            let spawned = thread::Builder::new()
                .name("chunk-encoding".into())
                .spawn(move || {
                    while let Ok(command) = commands.recv() {
                        // A CPU failure belongs to this request and must not kill
                        // the reusable owner or invent a successful encoded result.
                        let expected = command.capture.clone();
                        let result = catch_unwind(AssertUnwindSafe(|| {
                            let encoded = operation.encode(command.capture)?;
                            if encoded.capture() != &expected {
                                return Err(ServerError::Internal {
                                    invariant: "chunk encode result",
                                });
                            }
                            Ok(encoded)
                        }))
                        .unwrap_or(Err(ServerError::Internal {
                            invariant: "chunk encode owner panic",
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
                        operation: Operation::Encode,
                        kind: error.kind(),
                    });
                }
            }
        }
        Ok(pool)
    }

    /// Collects available replies, fills free owners in FIFO order, and collects
    /// once more. No extraction or encoding runs on this caller.
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
                            invariant: "chunk encode owner disconnected",
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
                capture: request.capture.clone(),
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
                        invariant: "chunk encode owner disconnected",
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

impl ChunkEncodePort for ChunkEncodingPool {
    fn start_encode(
        &mut self,
        capture: ChunkSaveView,
    ) -> Result<ChunkEncodeRequestId, ServerError> {
        if self.stopping || self.closed {
            return Err(ServerError::InvalidState {
                phase: if self.closed {
                    ServerPhase::Closed
                } else {
                    ServerPhase::Closing
                },
            });
        }
        if capture.generation() == 0 || capture.revision() == 0 {
            return Err(ServerError::InvalidInput {
                field: "chunk_network_snapshot",
            });
        }
        if self.requests.len() == MAX_CHUNK_ENCODE_REQUESTS {
            return Err(ServerError::Capacity {
                resource: Resource::ChunkEncodes,
                limit: MAX_CHUNK_ENCODE_REQUESTS,
                observed: MAX_CHUNK_ENCODE_REQUESTS + 1,
            });
        }
        let next = self
            .last_request
            .checked_add(1)
            .ok_or(ServerError::Internal {
                invariant: "chunk encode request space",
            })?;
        let id = ChunkEncodeRequestId::try_new(next)?;
        self.requests.push_back(Request {
            id,
            capture,
            state: RequestState::Queued,
        });
        self.last_request = next;
        self.dispatch();
        Ok(id)
    }

    fn poll_encode(&mut self, request: ChunkEncodeRequestId) -> ChunkEncodePoll {
        self.drive();
        let Some(index) = self
            .requests
            .iter()
            .position(|r| r.id == request && matches!(r.state, RequestState::Complete(_)))
        else {
            return ChunkEncodePoll::Pending;
        };
        match self.requests.remove(index).map(|r| r.state) {
            Some(RequestState::Complete(Ok(prepared))) => ChunkEncodePoll::Ready(prepared),
            Some(RequestState::Complete(Err(error))) => ChunkEncodePoll::Failed(error),
            _ => unreachable!("selected a completed request"),
        }
    }

    fn cancel_encode(&mut self, request: ChunkEncodeRequestId) -> Result<(), ServerError> {
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

impl WorkerLifecycle for ChunkEncodingPool {
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
                invariant: "chunk encode owner join",
            });
        }
        self.closed = true;
        Ok(())
    }
}

impl Drop for ChunkEncodingPool {
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
    use crate::core::contracts::ChunkKey;
    use crate::core::world::{
        ReadyChunk, materializations, ready_clones, reset_materializations, reset_ready_clones,
    };
    use mornlea_domain::{BlockPos, ChunkPos, Dimension};
    use mornlea_protocol::{Direction, ServerPacket, State, read_frame_ref};
    use mornlea_storage::{Chunk, ContainerSnapshot, StorageKind};
    use std::sync::{Arc, Condvar, Mutex};

    type Gate = Arc<(Mutex<bool>, Condvar)>;
    type Entry = (thread::ThreadId, ChunkSaveView);
    fn deadline() -> Deadline {
        Deadline::after(Instant::now(), Duration::from_secs(5)).unwrap()
    }
    fn expired() -> Deadline {
        Deadline::at(Instant::now())
    }
    fn source(x: i32) -> ReadyChunk {
        ReadyChunk::try_new(
            ChunkKey {
                dimension: Dimension::OVERWORLD,
                pos: ChunkPos::new(x, 0),
            },
            7,
            5,
            Chunk {
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
            },
        )
        .unwrap()
    }
    fn capture(x: i32) -> ChunkSaveView {
        source(x).capture(None, None)
    }
    fn codec() -> CodecOperation {
        CodecOperation(ProtocolCodec::new().unwrap())
    }
    fn open(gate: &Gate) {
        *gate.0.lock().unwrap() = true;
        gate.1.notify_all();
    }
    struct ReleaseOnDrop(Gate);
    impl Drop for ReleaseOnDrop {
        fn drop(&mut self) {
            open(&self.0);
        }
    }
    fn held(gate: &Gate) {
        let locked = gate.0.lock().unwrap();
        drop(gate.1.wait_while(locked, |opened| !*opened).unwrap());
    }
    fn ready(pool: &mut ChunkEncodingPool, id: ChunkEncodeRequestId) -> EncodedChunkSnapshot {
        let until = deadline();
        loop {
            match pool.poll_encode(id) {
                ChunkEncodePoll::Ready(v) => return v,
                ChunkEncodePoll::Failed(e) => panic!("actual encode failed: {e:?}"),
                ChunkEncodePoll::Pending => {
                    assert!(!until.expired(Instant::now()), "actual CPU reply deadline")
                }
            }
            thread::yield_now();
        }
    }
    fn decode(v: &EncodedChunkSnapshot) -> mornlea_protocol::ChunkSnapshot {
        let frame = v.frame();
        assert_eq!(
            (frame.packet_key().direction, frame.packet_key().state),
            (Direction::ServerToClient, State::Play)
        );
        assert_eq!(
            frame.packet_key(),
            ServerPacket::try_from(mornlea_domain::Event::ChunkSnapshot(
                v.capture().network_snapshot().unwrap().0
            ))
            .unwrap()
            .key()
        );
        let wire = read_frame_ref(frame.as_bytes()).unwrap();
        assert_eq!(wire.consumed, frame.byte_len());
        ProtocolCodec::new()
            .unwrap()
            .decode_snapshot(wire.payload)
            .unwrap()
    }
    // Each wrapper runs the real codec after a causal gate. CPU counters are
    // reset on that owner, independently of the caller's thread-local counters.
    struct HeldCodecOperation {
        actual: CodecOperation,
        gate: Gate,
        entries: SyncSender<Entry>,
        counts: SyncSender<(usize, usize)>,
        attempt: usize,
        hold_only: Option<usize>,
    }
    impl EncodeOperation for HeldCodecOperation {
        fn encode(&mut self, capture: ChunkSaveView) -> Result<EncodedChunkSnapshot, ServerError> {
            self.attempt += 1;
            self.entries
                .send((thread::current().id(), capture.clone()))
                .unwrap();
            if self.hold_only.is_none_or(|n| n == self.attempt) {
                held(&self.gate);
            }
            reset_ready_clones();
            reset_materializations();
            let encoded = self.actual.encode(capture);
            self.counts
                .send((ready_clones(), materializations()))
                .unwrap();
            encoded
        }
    }
    fn gated(
        workers: usize,
        only: Option<usize>,
    ) -> (
        ChunkEncodingPool,
        Gate,
        Receiver<Entry>,
        Receiver<(usize, usize)>,
    ) {
        let gate = Arc::new((Mutex::new(false), Condvar::new()));
        let (entries, rx) = mpsc::sync_channel(8);
        let (counts, cx) = mpsc::sync_channel(8);
        let operations = (0..workers)
            .map(|_| {
                Box::new(HeldCodecOperation {
                    actual: codec(),
                    gate: gate.clone(),
                    entries: entries.clone(),
                    counts: counts.clone(),
                    attempt: 0,
                    hold_only: only,
                }) as Box<dyn EncodeOperation>
            })
            .collect();
        (
            ChunkEncodingPool::from_operations(operations).unwrap(),
            gate,
            rx,
            cx,
        )
    }
    fn capacity() -> ServerError {
        ServerError::Capacity {
            resource: Resource::ChunkEncodes,
            limit: 8,
            observed: 9,
        }
    }

    #[test]
    fn public_construction_bounds_and_actual_canonical_owner_survive_source_drop() {
        for workers in [0, 3] {
            assert!(matches!(
                ChunkEncodingPool::try_new(workers),
                Err(ServerError::InvalidInput {
                    field: "chunk_encode_workers"
                })
            ));
        }
        for workers in [1, 2] {
            let mut pool = ChunkEncodingPool::try_new(workers).unwrap();
            let supplied = capture(-2);
            let id = pool.start_encode(supplied.clone()).unwrap();
            let encoded = ready(&mut pool, id);
            assert_eq!(encoded.capture(), &supplied);
            assert_eq!(encoded.section_payload_bytes(), 48);
            assert_eq!(decode(&encoded).revision, 5);
            pool.close(deadline()).unwrap();
            assert!(pool.workers.iter().all(|w| w.join.is_none()));
            assert_eq!(encoded.capture(), &supplied);
        }
    }

    #[test]
    fn held_actual_codec_returns_caller_without_body_clone_or_disk_materialization() {
        let (pool, gate, entries, counts) = gated(1, None);
        let _release = ReleaseOnDrop(gate.clone());
        let (returned, rx) = mpsc::sync_channel(1);
        let submitter = thread::spawn(move || {
            let mut pool = pool;
            let token = capture(0);
            reset_ready_clones();
            reset_materializations();
            let id = pool.start_encode(token.clone()).unwrap();
            let driven = pool.drive();
            let pending = matches!(pool.poll_encode(id), ChunkEncodePoll::Pending);
            returned
                .send((
                    pool,
                    id,
                    token,
                    driven,
                    pending,
                    ready_clones(),
                    materializations(),
                    thread::current().id(),
                ))
                .ok();
        });
        let returned = rx.recv_timeout(Duration::from_secs(5));
        open(&gate);
        submitter.join().unwrap();
        let (mut pool, id, token, driven, pending, clones, expanded, caller) =
            returned.expect("caller returns while actual codec held");
        let (cpu, observed) = entries.recv_timeout(Duration::from_secs(5)).unwrap();
        assert_ne!(cpu, caller);
        assert_eq!(observed, token);
        assert_eq!((driven, pending, clones, expanded), (0, true, 0, 0));
        assert_eq!(ready(&mut pool, id).capture(), &token);
        assert_eq!(counts.recv_timeout(Duration::from_secs(5)).unwrap(), (0, 0));
        pool.close(deadline()).unwrap();
    }

    #[test]
    fn one_and_two_actual_codecs_charge_eight_cancelled_started_and_held_results() {
        for workers in [1, 2] {
            let (mut pool, gate, entries, counts) = gated(workers, None);
            let _release = ReleaseOnDrop(gate.clone());
            let mut ids = (0..8)
                .map(|x| pool.start_encode(capture(x)).unwrap())
                .collect::<Vec<_>>();
            let owner_threads = (0..workers)
                .map(|_| entries.recv_timeout(Duration::from_secs(5)).unwrap().0)
                .collect::<Vec<_>>();
            if workers == 2 {
                assert_ne!(owner_threads[0], owner_threads[1]);
            }
            assert_eq!(
                (pool.worker_jobs(), pool.queued_jobs(), pool.owned_jobs()),
                (workers, 8 - workers, 8)
            );
            assert_eq!(pool.start_encode(capture(9)), Err(capacity()));
            pool.cancel_encode(ids[7]).unwrap();
            assert_eq!(pool.owned_jobs(), 7);
            ids[7] = pool.start_encode(capture(9)).unwrap();
            pool.cancel_encode(ids[0]).unwrap();
            assert_eq!(pool.owned_jobs(), 8);
            assert!(matches!(pool.poll_encode(ids[0]), ChunkEncodePoll::Pending));
            open(&gate);
            let until = deadline();
            let mut collected = 0;
            while pool.queued_jobs() != 0 || pool.worker_jobs() != 0 {
                collected += pool.drive();
                assert!(!until.expired(Instant::now()));
                thread::yield_now();
            }
            assert_eq!(collected, 8);
            assert_eq!(pool.owned_jobs(), 7);
            assert!(matches!(pool.poll_encode(ids[0]), ChunkEncodePoll::Pending));
            // Observe the completed cohort before admitting another real job.
            assert_eq!(counts.try_iter().count(), 8);
            let extra = pool.start_encode(capture(10)).unwrap();
            ids[0] = extra;
            pool.wait(deadline()).unwrap();
            assert_eq!(
                (
                    pool.queued_jobs(),
                    pool.worker_jobs(),
                    pool.held_completions(),
                    pool.owned_jobs()
                ),
                (0, 0, 8, 8)
            );
            assert_eq!(pool.start_encode(capture(11)), Err(capacity()));
            entries.try_iter().for_each(drop);
            counts.try_iter().for_each(drop);
            let output = ready(&mut pool, ids[1]);
            let cloned = output.frame().clone();
            let ptr = cloned.as_bytes().as_ptr();
            let moved = output.into_frame();
            assert_eq!(ptr, moved.as_bytes().as_ptr());
            assert_eq!(cloned.packet_key(), moved.packet_key());
            assert_eq!(cloned.as_bytes(), moved.as_bytes());
            assert_eq!(pool.owned_jobs(), 7);
            let next = pool.start_encode(capture(12)).unwrap();
            assert!(next > extra);
            assert!(matches!(pool.poll_encode(ids[1]), ChunkEncodePoll::Pending));
            assert!(matches!(
                pool.poll_encode(ChunkEncodeRequestId::try_new(999).unwrap()),
                ChunkEncodePoll::Pending
            ));
            pool.wait(deadline()).unwrap();
            pool.cancel_encode(ids[2]).unwrap();
            assert_eq!(pool.owned_jobs(), 7);
            pool.cancel().unwrap();
            assert_eq!(pool.owned_jobs(), 0);
            pool.close(deadline()).unwrap();
        }
    }

    #[test]
    fn actual_single_codec_fifo_skips_only_cancelled_queued_capture() {
        let (mut pool, gate, entries, _counts) = gated(1, Some(1));
        let _release = ReleaseOnDrop(gate.clone());
        let tokens = (0..8).map(capture).collect::<Vec<_>>();
        let ids = tokens
            .iter()
            .map(|v| pool.start_encode(v.clone()).unwrap())
            .collect::<Vec<_>>();
        let first = entries.recv_timeout(Duration::from_secs(5)).unwrap().1;
        pool.cancel_encode(ids[3]).unwrap();
        open(&gate);
        pool.wait(deadline()).unwrap();
        let observed = std::iter::once(first)
            .chain(entries.try_iter().map(|e| e.1))
            .collect::<Vec<_>>();
        let expected = tokens
            .iter()
            .enumerate()
            .filter(|(i, _)| *i != 3)
            .map(|(_, v)| v.clone())
            .collect::<Vec<_>>();
        assert_eq!(observed, expected);
        for (i, id) in ids.into_iter().enumerate() {
            if i != 3 {
                assert_eq!(ready(&mut pool, id).capture(), &tokens[i]);
            }
        }
        pool.close(deadline()).unwrap();
    }

    #[test]
    fn mixed_complete_started_queued_eight_is_charged_before_any_poll() {
        let (mut pool, gate, entries, _counts) = gated(1, Some(2));
        let _release = ReleaseOnDrop(gate.clone());
        let ids = (0..8)
            .map(|x| pool.start_encode(capture(x)).unwrap())
            .collect::<Vec<_>>();
        let until = deadline();
        while pool.held_completions() == 0 {
            pool.drive();
            assert!(!until.expired(Instant::now()));
            thread::yield_now();
        }
        entries.recv_timeout(Duration::from_secs(5)).unwrap();
        entries.recv_timeout(Duration::from_secs(5)).unwrap();
        assert_eq!(
            (
                pool.held_completions(),
                pool.worker_jobs(),
                pool.queued_jobs(),
                pool.owned_jobs()
            ),
            (1, 1, 6, 8)
        );
        assert_eq!(pool.start_encode(capture(9)), Err(capacity()));
        open(&gate);
        pool.wait(deadline()).unwrap();
        for id in ids {
            ready(&mut pool, id);
        }
        pool.close(deadline()).unwrap();
    }

    #[test]
    fn held_old_capture_and_changed_pages_preserve_exact_distinct_tokens() {
        let (mut pool, gate, entries, _counts) = gated(1, Some(1));
        let _release = ReleaseOnDrop(gate.clone());
        let mut source = source(0);
        let old = source.capture(None, None);
        let same_numbers = source.capture(None, None);
        assert_ne!(old, same_numbers);
        let old_id = pool.start_encode(old.clone()).unwrap();
        entries.recv_timeout(Duration::from_secs(5)).unwrap();
        source.set_block(BlockPos::new(0, -64, 0), 1);
        source.mark_blocks_dirty();
        source.finish_tick(false);
        let changed = source.capture(None, None);
        let current_id = pool.start_encode(changed.clone()).unwrap();
        let equal_id = pool.start_encode(same_numbers.clone()).unwrap();
        let repeated_id = pool.start_encode(old.clone()).unwrap();
        drop(source);
        open(&gate);
        let a = ready(&mut pool, old_id);
        let b = ready(&mut pool, current_id);
        let c = ready(&mut pool, equal_id);
        let d = ready(&mut pool, repeated_id);
        assert_eq!(a.capture(), &old);
        assert_eq!(b.capture(), &changed);
        assert_eq!(c.capture(), &same_numbers);
        assert_eq!(d.capture(), &old);
        assert_eq!(decode(&a).revision, 5);
        assert_eq!(decode(&b).revision, 6);
        assert_ne!(decode(&a).sections[0], decode(&b).sections[0]);
        assert_eq!(decode(&a), decode(&c));
        assert_eq!(decode(&a), decode(&d));
        pool.close(deadline()).unwrap();
    }

    #[test]
    fn expired_drain_close_preserves_started_owner_and_same_pool_retry_joins() {
        let (mut pool, gate, entries, _counts) = gated(1, None);
        let _release = ReleaseOnDrop(gate.clone());
        let token = capture(0);
        let id = pool.start_encode(token.clone()).unwrap();
        entries.recv_timeout(Duration::from_secs(5)).unwrap();
        assert_eq!(
            pool.wait(expired()),
            Err(ServerError::Timeout {
                operation: Operation::Shutdown
            })
        );
        assert_eq!(
            pool.close(expired()),
            Err(ServerError::Timeout {
                operation: Operation::Close
            })
        );
        assert_eq!((pool.worker_jobs(), pool.owned_jobs()), (1, 1));
        assert!(pool.workers[0].sender.is_some() && pool.workers[0].join.is_some());
        assert_eq!(
            pool.start_encode(token.clone()),
            Err(ServerError::InvalidState {
                phase: ServerPhase::Closing
            })
        );
        open(&gate);
        pool.close(deadline()).unwrap();
        assert_eq!(pool.owned_jobs(), 0);
        assert!(pool.workers[0].join.is_none());
        assert!(matches!(pool.poll_encode(id), ChunkEncodePoll::Pending));
        pool.close(expired()).unwrap();
        assert_eq!(
            pool.start_encode(token),
            Err(ServerError::InvalidState {
                phase: ServerPhase::Closed
            })
        );
    }

    #[test]
    fn stop_and_wait_preserve_actual_held_result_until_explicit_cancel() {
        let mut pool = ChunkEncodingPool::try_new(1).unwrap();
        let id = pool.start_encode(capture(0)).unwrap();
        pool.stop_new().unwrap();
        pool.wait(deadline()).unwrap();
        assert_eq!(
            (
                pool.worker_jobs(),
                pool.held_completions(),
                pool.owned_jobs()
            ),
            (0, 1, 1)
        );
        assert_eq!(ready(&mut pool, id).section_payload_bytes(), 48);
        pool.cancel_encode(ChunkEncodeRequestId::try_new(99).unwrap())
            .unwrap();
        pool.cancel().unwrap();
        pool.close(deadline()).unwrap();
    }

    // These wrappers explicitly inject defensive failures, then use the actual
    // codec. They establish fault containment, not ordinary codec failures.
    struct FaultThenActual {
        actual: CodecOperation,
        attempt: usize,
        wrong: ChunkSaveView,
    }
    impl EncodeOperation for FaultThenActual {
        fn encode(&mut self, v: ChunkSaveView) -> Result<EncodedChunkSnapshot, ServerError> {
            self.attempt += 1;
            match self.attempt {
                1 => panic!("injected encode panic"),
                2 => self.actual.encode(self.wrong.clone()),
                _ => self.actual.encode(v),
            }
        }
    }
    #[test]
    fn injected_panic_wrong_token_and_actual_factory_refusal_preserve_owner() {
        let mut pool = ChunkEncodingPool::from_operations(vec![Box::new(FaultThenActual {
            actual: codec(),
            attempt: 0,
            wrong: capture(0),
        })])
        .unwrap();
        for invariant in ["chunk encode owner panic", "chunk encode result"] {
            let id = pool.start_encode(capture(0)).unwrap();
            pool.wait(deadline()).unwrap();
            assert!(
                matches!(pool.poll_encode(id),ChunkEncodePoll::Failed(ServerError::Internal { invariant:actual }) if actual==invariant)
            );
        }
        let mut source = source(0);
        source.set_block(BlockPos::new(0, -64, 0), 32767);
        source.mark_blocks_dirty();
        source.finish_tick(false);
        let id = pool.start_encode(source.capture(None, None)).unwrap();
        pool.wait(deadline()).unwrap();
        assert!(matches!(
            pool.poll_encode(id),
            ChunkEncodePoll::Failed(ServerError::InvalidInput {
                field: "chunk_network_snapshot"
            })
        ));
        let id = pool.start_encode(capture(0)).unwrap();
        assert_eq!(ready(&mut pool, id).section_payload_bytes(), 48);
        pool.close(deadline()).unwrap();
    }

    struct HeldDrop {
        actual: CodecOperation,
        gate: Gate,
        entered: SyncSender<()>,
    }
    impl EncodeOperation for HeldDrop {
        fn encode(&mut self, v: ChunkSaveView) -> Result<EncodedChunkSnapshot, ServerError> {
            self.actual.encode(v)
        }
    }
    impl Drop for HeldDrop {
        fn drop(&mut self) {
            self.entered.send(()).ok();
            held(&self.gate);
        }
    }
    #[test]
    fn actual_result_then_injected_teardown_gate_retains_disconnected_join_for_retry() {
        let gate = Arc::new((Mutex::new(false), Condvar::new()));
        let (tx, rx) = mpsc::sync_channel(1);
        let mut pool = ChunkEncodingPool::from_operations(vec![Box::new(HeldDrop {
            actual: codec(),
            gate: gate.clone(),
            entered: tx,
        })])
        .unwrap();
        let _release = ReleaseOnDrop(gate.clone());
        let id = pool.start_encode(capture(0)).unwrap();
        pool.wait(deadline()).unwrap();
        assert_eq!(ready(&mut pool, id).section_payload_bytes(), 48);
        assert_eq!(
            pool.close(expired()),
            Err(ServerError::Timeout {
                operation: Operation::Close
            })
        );
        assert!(pool.workers[0].sender.is_none() && pool.workers[0].join.is_some());
        assert!(!pool.closed);
        rx.recv_timeout(Duration::from_secs(5)).unwrap();
        open(&gate);
        pool.close(deadline()).unwrap();
        assert!(pool.closed && pool.workers[0].join.is_none());
    }
    struct PanicDrop(CodecOperation);
    impl EncodeOperation for PanicDrop {
        fn encode(&mut self, v: ChunkSaveView) -> Result<EncodedChunkSnapshot, ServerError> {
            self.0.encode(v)
        }
    }
    impl Drop for PanicDrop {
        fn drop(&mut self) {
            panic!("injected teardown panic");
        }
    }
    #[test]
    fn actual_result_then_injected_teardown_panic_is_sticky_join_refusal() {
        let mut pool =
            ChunkEncodingPool::from_operations(vec![Box::new(PanicDrop(codec()))]).unwrap();
        let id = pool.start_encode(capture(0)).unwrap();
        assert_eq!(ready(&mut pool, id).section_payload_bytes(), 48);
        for _ in 0..2 {
            assert_eq!(
                pool.close(deadline()),
                Err(ServerError::Internal {
                    invariant: "chunk encode owner join"
                })
            );
            assert!(pool.join_failed && !pool.closed);
        }
    }

    #[test]
    fn explicit_disconnected_channels_fail_whole_original_requests_and_keep_cap() {
        let mut pool = ChunkEncodingPool::try_new(1).unwrap();
        let (sender, commands) = mpsc::sync_channel(1);
        drop(commands);
        pool.workers[0].sender = Some(sender);
        let tokens = (0..8).map(capture).collect::<Vec<_>>();
        let ids = tokens
            .iter()
            .map(|v| pool.start_encode(v.clone()).unwrap())
            .collect::<Vec<_>>();
        assert_eq!(pool.owned_jobs(), 8);
        assert_eq!(pool.start_encode(capture(9)), Err(capacity()));
        for (i, id) in ids.into_iter().enumerate() {
            assert_eq!(pool.requests[0].capture, tokens[i]);
            assert!(matches!(
                pool.poll_encode(id),
                ChunkEncodePoll::Failed(ServerError::Internal {
                    invariant: "chunk encode owner disconnected"
                })
            ));
        }
        pool.close(deadline()).unwrap();
    }
    #[test]
    fn explicit_full_channel_preserves_fifo_and_reply_disconnect_failure() {
        let mut pool = ChunkEncodingPool::try_new(1).unwrap();
        let (sender, commands) = mpsc::sync_channel(1);
        let (reply, _rx) = mpsc::sync_channel(1);
        sender
            .try_send(Command {
                capture: capture(9),
                reply,
            })
            .unwrap();
        pool.workers[0].sender = Some(sender);
        let a = capture(1);
        let b = capture(2);
        let ids = [
            pool.start_encode(a.clone()).unwrap(),
            pool.start_encode(b.clone()).unwrap(),
        ];
        assert_eq!((pool.queued_jobs(), pool.worker_jobs()), (2, 0));
        assert_eq!(
            pool.wait(expired()),
            Err(ServerError::Timeout {
                operation: Operation::Shutdown
            })
        );
        commands.recv().unwrap();
        for token in [a, b] {
            pool.drive();
            let command = commands.recv().unwrap();
            assert_eq!(command.capture, token);
            drop(command.reply);
            pool.drive();
        }
        for id in ids {
            assert!(matches!(
                pool.poll_encode(id),
                ChunkEncodePoll::Failed(ServerError::Internal {
                    invariant: "chunk encode owner disconnected"
                })
            ));
        }
        pool.close(deadline()).unwrap();
    }
    #[test]
    fn checked_request_exhaustion_never_admits_or_wraps() {
        let mut pool = ChunkEncodingPool::try_new(1).unwrap();
        pool.last_request = u64::MAX;
        assert_eq!(
            pool.start_encode(capture(0)),
            Err(ServerError::Internal {
                invariant: "chunk encode request space"
            })
        );
        assert_eq!(pool.owned_jobs(), 0);
        assert_eq!(pool.last_request, u64::MAX);
        pool.close(deadline()).unwrap();
    }
    #[test]
    fn captured_source_slots_remain_immutable_while_actual_network_sections_stay_equal() {
        use crate::core::drop_store::DropState;
        let (mut pool, gate, entries, counts) = gated(1, Some(1));
        let _release = ReleaseOnDrop(gate.clone());
        let mut source = source(0);
        let mut slots = DropState::new(source.key, source.drop_slots().try_into().unwrap());
        let old = source.capture(Some(&slots), None);
        let old_id = pool.start_encode(old.clone()).unwrap();
        entries.recv_timeout(Duration::from_secs(5)).unwrap();
        slots.slots[31].generation = 19;
        slots.slots[31].age_ticks = 123;
        slots.dirty = true;
        let changed = source.capture(Some(&slots), None);
        source.finish_tick(true);
        let new_id = pool.start_encode(changed.clone()).unwrap();
        drop(source);
        drop(slots);
        open(&gate);
        let a = ready(&mut pool, old_id);
        let b = ready(&mut pool, new_id);
        assert_eq!(a.capture(), &old);
        assert_eq!(b.capture(), &changed);
        assert_eq!((decode(&a).revision, decode(&b).revision), (5, 6));
        assert_eq!(decode(&a).sections, decode(&b).sections);
        assert_eq!(counts.recv_timeout(Duration::from_secs(5)).unwrap(), (0, 0));
        assert_eq!(counts.recv_timeout(Duration::from_secs(5)).unwrap(), (0, 0));
        // Explicit off-tick disk projection observes captured slots only after
        // the independently measured CPU encoding counters were reported.
        assert_eq!(old.materialize().chunk.drops[31].age_ticks, 0);
        assert_eq!(changed.materialize().chunk.drops[31].age_ticks, 123);
        pool.close(deadline()).unwrap();
    }
}
