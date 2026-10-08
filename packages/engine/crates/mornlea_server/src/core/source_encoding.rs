//! Bounded source routing over separately owned actual snapshot codec threads.

use super::chunk_encoding::{
    ChunkEncodePoll, ChunkEncodePort, ChunkEncodeRequestId, EncodedChunkSnapshot,
    MAX_CHUNK_ENCODE_REQUESTS,
};
use super::contracts::{ChunkKey, Deadline, ServerError, ServerPhase, SessionKey, WorkerLifecycle};
use super::encoding_worker::ChunkEncodingPool;
use super::world::ChunkSaveView;

struct SourceEncodeRequest {
    session: SessionKey,
    request: ChunkEncodeRequestId,
    capture: ChunkSaveView,
}

/// Owns source routing separately from authority and codec CPU lifecycle.
pub struct SourceSnapshotEncoding {
    pool: ChunkEncodingPool,
    requests: Vec<SourceEncodeRequest>,
    stopping: bool,
    closed: bool,
}

/// Transfers one original routing identity and its whole actual result.
pub struct SourceEncodeResult {
    session: SessionKey,
    request: ChunkEncodeRequestId,
    capture: ChunkSaveView,
    result: Result<EncodedChunkSnapshot, ServerError>,
}
impl SourceEncodeResult {
    pub fn session(&self) -> SessionKey {
        self.session
    }
    pub fn request(&self) -> ChunkEncodeRequestId {
        self.request
    }
    pub fn capture(&self) -> &ChunkSaveView {
        &self.capture
    }
    pub fn result(&self) -> &Result<EncodedChunkSnapshot, ServerError> {
        &self.result
    }
    pub fn into_parts(
        self,
    ) -> (
        SessionKey,
        ChunkEncodeRequestId,
        ChunkSaveView,
        Result<EncodedChunkSnapshot, ServerError>,
    ) {
        (self.session, self.request, self.capture, self.result)
    }
}
impl SourceSnapshotEncoding {
    /// Builds one or two actual codec CPU owners off the authoritative tick.
    pub fn try_new(workers: usize) -> Result<Self, ServerError> {
        Ok(Self {
            pool: ChunkEncodingPool::try_new(workers)?,
            requests: Vec::with_capacity(MAX_CHUNK_ENCODE_REQUESTS),
            stopping: false,
            closed: false,
        })
    }
    pub fn request(
        &mut self,
        session: SessionKey,
        capture: ChunkSaveView,
    ) -> Result<ChunkEncodeRequestId, ServerError> {
        // Relevance never replaces a capture: duplicate keys require explicit
        // cancellation, and refused admissions consume no provider identity.
        if self.closed || self.stopping {
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
        if self.contains(session, capture.key()) {
            return Err(ServerError::InvalidInput {
                field: "source_chunk_encode",
            });
        }
        let expected = capture.clone();
        let request = self.pool.start_encode(capture)?;
        self.requests.push(SourceEncodeRequest {
            session,
            request,
            capture: expected,
        });
        Ok(request)
    }
    pub fn contains(&self, session: SessionKey, key: ChunkKey) -> bool {
        self.requests
            .iter()
            .any(|r| r.session == session && r.capture.key() == key)
    }
    pub fn retained_requests(&self) -> usize {
        self.requests.len()
    }
    /// Includes cancelled started work until the actual pool collects its reply.
    pub fn charged_requests(&self) -> usize {
        self.pool.owned_jobs()
    }
    /// Cancels obsolete routing using bounded, nonblocking caller scalar facts.
    /// The caller owns live session, Wanted and world-version validation.
    pub fn retain_current(
        &mut self,
        mut is_current: impl FnMut(SessionKey, ChunkKey, u64, u64) -> bool,
    ) -> Result<usize, ServerError> {
        let mut cancelled = 0;
        let mut index = 0;
        while index < self.requests.len() {
            let record = &self.requests[index];
            if is_current(
                record.session,
                record.capture.key(),
                record.capture.generation(),
                record.capture.revision(),
            ) {
                index += 1;
                continue;
            }
            // Do not drive first: a cancelled started request keeps its real
            // provider charge until its actual reply is collected.
            self.pool.cancel_encode(record.request)?;
            self.requests.remove(index);
            cancelled += 1;
        }
        Ok(cancelled)
    }
    /// Moves actual completed results in retained admission order, skipping Pending.
    /// Also collects cancelled replies when the routing book is empty.
    pub fn collect_ready(&mut self) -> Vec<SourceEncodeResult> {
        self.pool.drive();
        let mut results = Vec::new();
        let mut index = 0;
        while index < self.requests.len() {
            let result = match self.pool.poll_encode(self.requests[index].request) {
                ChunkEncodePoll::Pending => {
                    index += 1;
                    continue;
                }
                ChunkEncodePoll::Ready(result) => {
                    if result.capture() == &self.requests[index].capture {
                        Ok(result)
                    } else {
                        Err(ServerError::Internal {
                            invariant: "source chunk encode result",
                        })
                    }
                }
                ChunkEncodePoll::Failed(error) => Err(error),
            };
            let record = self.requests.remove(index);
            results.push(SourceEncodeResult {
                session: record.session,
                request: record.request,
                capture: record.capture,
                result,
            });
        }
        results
    }
}
impl WorkerLifecycle for SourceSnapshotEncoding {
    fn stop_new(&mut self) -> Result<(), ServerError> {
        self.pool.stop_new()?;
        self.stopping = true;
        Ok(())
    }
    fn cancel(&mut self) -> Result<(), ServerError> {
        self.pool.cancel()?;
        self.requests.clear();
        Ok(())
    }
    fn wait(&mut self, deadline: Deadline) -> Result<(), ServerError> {
        self.pool.wait(deadline)
    }
    fn close(&mut self, deadline: Deadline) -> Result<(), ServerError> {
        self.stop_new()?;
        // Pool close cancels outstanding results. On refusal, retain this same
        // shutdown owner for close retry, without promising result recovery.
        self.pool.close(deadline)?;
        self.requests.clear();
        self.closed = true;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::chunk_encoding::ChunkEncodeRequestId;
    use crate::core::contracts::ServerError;
    use crate::core::contracts::{
        ChunkKey, Deadline, Operation, Resource, ServerPhase, SessionKey, TickPublication,
        WorkerLifecycle,
    };
    use crate::core::publication::PreparedSourcePublication;
    use crate::core::world::{ChunkSaveView, ReadyChunk};
    use mornlea_domain::{BlockPos, ChunkPos, Dimension, Event};
    use mornlea_protocol::{ProtocolCodec, read_frame_ref};
    use mornlea_storage::{Chunk, ContainerSnapshot, StorageKind};
    use std::time::{Duration, Instant};

    fn session(n: u64) -> SessionKey {
        SessionKey::from_raw(n).unwrap()
    }
    fn key(x: i32) -> ChunkKey {
        ChunkKey {
            dimension: Dimension::OVERWORLD,
            pos: ChunkPos::new(x, 0),
        }
    }
    fn source(x: i32, indexed: bool) -> ReadyChunk {
        let mut sections = vec![
            ContainerSnapshot {
                kind: StorageKind::Single,
                bits: 0,
                single: 0,
                palette: vec![],
                packed: vec![]
            };
            24
        ];
        if indexed {
            sections[0] = ContainerSnapshot {
                kind: StorageKind::Indexed,
                bits: 4,
                single: 0,
                palette: vec![2, 0, 1],
                packed: vec![0; 256],
            };
        }
        ReadyChunk::try_new(
            key(x),
            7,
            5,
            Chunk {
                sections,
                drops: vec![Default::default(); 32],
                furnaces: vec![Default::default(); 32],
                chests: vec![Default::default(); 16],
            },
        )
        .unwrap()
    }
    fn capture(x: i32) -> ChunkSaveView {
        source(x, false).capture(None, None)
    }
    fn deadline() -> Deadline {
        Deadline::after(Instant::now(), Duration::from_secs(10)).unwrap()
    }
    fn complete(owner: &mut SourceSnapshotEncoding) -> Vec<SourceEncodeResult> {
        owner.wait(deadline()).unwrap();
        owner.collect_ready()
    }
    fn close(owner: &mut SourceSnapshotEncoding) {
        owner.stop_new().unwrap();
        owner.wait(deadline()).unwrap();
        owner.close(deadline()).unwrap();
    }
    fn capacity() -> ServerError {
        ServerError::Capacity {
            resource: Resource::ChunkEncodes,
            limit: 8,
            observed: 9,
        }
    }
    fn assert_results(
        results: &[SourceEncodeResult],
        expected: &[(SessionKey, ChunkEncodeRequestId, ChunkSaveView)],
    ) {
        assert_eq!(results.len(), expected.len());
        for (result, (session, id, capture)) in results.iter().zip(expected) {
            assert_eq!(result.session(), *session);
            assert_eq!(result.request(), *id);
            assert_eq!(result.capture(), capture);
            let encoded = result.result().as_ref().unwrap();
            assert_eq!(encoded.capture(), capture);
            assert_eq!(encoded.section_payload_bytes(), 48);
            let wire = read_frame_ref(encoded.frame().as_bytes()).unwrap();
            assert_eq!(wire.consumed, encoded.frame().byte_len());
            let decoded = ProtocolCodec::new()
                .unwrap()
                .decode_snapshot(wire.payload)
                .unwrap();
            assert_eq!(
                (decoded.chunk_x, decoded.chunk_z),
                (capture.key().pos.x(), capture.key().pos.z())
            );
            assert_eq!(decoded.revision, 5);
            assert_eq!(decoded.sections.len(), 24);
            assert!(
                decoded
                    .sections
                    .iter()
                    .all(|s| s.bits == 0 && s.single == 0)
            );
        }
    }

    #[test]
    fn source_encoding_actual_result_retains_owner_for_builder() {
        for workers in [0, 3] {
            assert!(matches!(
                SourceSnapshotEncoding::try_new(workers),
                Err(ServerError::InvalidInput {
                    field: "chunk_encode_workers"
                })
            ));
        }
        let mut owner = SourceSnapshotEncoding::try_new(2).unwrap();
        let ready = source(-2, true);
        let original = ready.capture(None, None);
        let id = owner.request(session(1), original.clone()).unwrap();
        drop(ready);
        let mut results = complete(&mut owner);
        close(&mut owner);
        assert_eq!(results.len(), 1);
        let result = results.remove(0);
        assert_eq!((result.session(), result.request()), (session(1), id));
        assert_eq!(result.capture(), &original);
        let (_, _, token, encoded) = result.into_parts();
        let encoded = encoded.unwrap();
        assert_eq!(encoded.capture(), &token);
        assert_eq!(encoded.section_payload_bytes(), 2100);
        let semantic_ptr = encoded.snapshot().sections().as_ptr();
        let (bits, palette, words) = encoded.snapshot().sections()[0].as_indexed().unwrap();
        assert_eq!(bits, 4);
        assert_eq!(palette, [2, 0, 1]);
        assert_eq!(words, [0; 256]);
        let palette_ptr = palette.as_ptr();
        let words_ptr = words.as_ptr();
        let frame_ptr = encoded.frame().as_bytes().as_ptr();
        let frame_bytes = encoded.frame().as_bytes().to_vec();
        let wire = read_frame_ref(encoded.frame().as_bytes()).unwrap();
        assert_eq!(wire.consumed, encoded.frame().byte_len());
        let decoded = ProtocolCodec::new()
            .unwrap()
            .decode_snapshot(wire.payload)
            .unwrap();
        assert_eq!((decoded.chunk_x, decoded.chunk_z), (-2, 0));
        assert_eq!(decoded.revision, 5);
        assert_eq!(decoded.sections.len(), 24);
        assert_eq!(decoded.sections[0].palette, [2, 0, 1]);
        assert_eq!(decoded.sections[0].packed, [0; 256]);
        assert!(
            decoded.sections[1..]
                .iter()
                .all(|s| s.bits == 0 && s.single == 0)
        );
        let mut batch = PreparedSourcePublication::new(TickPublication {
            tick: 37,
            events: vec![],
            control: vec![],
            counters: Default::default(),
        });
        batch
            .append_encoded_snapshot(session(1), &original, encoded)
            .unwrap();
        let (publication, frames, refusals) = batch.into_parts();
        assert!(refusals.is_empty());
        let Event::ChunkSnapshot(snapshot) = publication.events[0].event() else {
            panic!("snapshot owner");
        };
        assert_eq!(snapshot.sections().as_ptr(), semantic_ptr);
        let (_, palette, words) = snapshot.sections()[0].as_indexed().unwrap();
        assert_eq!(palette.as_ptr(), palette_ptr);
        assert_eq!(words.as_ptr(), words_ptr);
        assert_eq!(frames[0].0, 0);
        assert_eq!(frames[0].1.as_bytes().as_ptr(), frame_ptr);
        assert_eq!(frames[0].1.as_bytes(), frame_bytes);
    }

    #[test]
    fn source_encoding_duplicate_and_phase_precedence() {
        let mut owner = SourceSnapshotEncoding::try_new(1).unwrap();
        let token = capture(0);
        let id = owner.request(session(1), token.clone()).unwrap();
        let duplicate = owner.request(session(1), capture(0));
        let peer_token = capture(0);
        let peer_id = owner.request(session(2), peer_token.clone()).unwrap();
        let contained = owner.contains(session(1), key(0));
        owner.stop_new().unwrap();
        let stopping_duplicate = owner.request(session(1), token.clone());
        let stopping_new = owner.request(session(1), capture(1));
        let results = complete(&mut owner);
        close(&mut owner);
        owner.close(Deadline::at(Instant::now())).unwrap();
        let closed = owner.request(session(1), token.clone());
        assert_eq!(
            duplicate,
            Err(ServerError::InvalidInput {
                field: "source_chunk_encode"
            })
        );
        assert!(contained);
        assert_eq!(
            stopping_duplicate,
            Err(ServerError::InvalidState {
                phase: ServerPhase::Closing
            })
        );
        assert_eq!(
            stopping_new,
            Err(ServerError::InvalidState {
                phase: ServerPhase::Closing
            })
        );
        assert_eq!(
            closed,
            Err(ServerError::InvalidState {
                phase: ServerPhase::Closed
            })
        );
        assert_eq!(peer_id.get(), id.get() + 1);
        assert_results(
            &results,
            &[(session(1), id, token), (session(2), peer_id, peer_token)],
        );
        assert_eq!(
            (owner.retained_requests(), owner.charged_requests()),
            (0, 0)
        );
    }

    #[test]
    fn source_encoding_eight_queue_cancel_releases_actual_capacity() {
        let mut owner = SourceSnapshotEncoding::try_new(1).unwrap();
        let tokens = (0..8).map(capture).collect::<Vec<_>>();
        let ids = tokens
            .iter()
            .map(|v| owner.request(session(1), v.clone()).unwrap())
            .collect::<Vec<_>>();
        let full = owner.request(session(1), capture(8));
        let cancelled = owner.retain_current(|_, k, _, _| k != key(7)).unwrap();
        let counts = (owner.retained_requests(), owner.charged_requests());
        let ninth_token = capture(8);
        let ninth = owner.request(session(1), ninth_token.clone());
        let results = complete(&mut owner);
        close(&mut owner);
        assert_eq!(full, Err(capacity()));
        assert_eq!(cancelled, 1);
        assert_eq!(counts, (7, 7));
        let ninth = ninth.unwrap();
        assert!(ninth > ids[7]);
        let mut expected = (0..7)
            .map(|i| (session(1), ids[i], tokens[i].clone()))
            .collect::<Vec<_>>();
        expected.push((session(1), ninth, ninth_token));
        assert_results(&results, &expected);
    }

    #[test]
    fn source_encoding_cancelled_started_is_charged_until_collection() {
        let mut owner = SourceSnapshotEncoding::try_new(1).unwrap();
        let tokens = (0..8).map(capture).collect::<Vec<_>>();
        let ids = tokens
            .iter()
            .map(|v| owner.request(session(1), v.clone()).unwrap())
            .collect::<Vec<_>>();
        let cancelled = owner.retain_current(|_, k, _, _| k != key(0)).unwrap();
        let counts = (owner.retained_requests(), owner.charged_requests());
        let full = owner.request(session(1), capture(8));
        let results = complete(&mut owner);
        let released = owner.charged_requests();
        let extra_token = capture(8);
        let extra = owner.request(session(1), extra_token.clone()).unwrap();
        let extras = complete(&mut owner);
        close(&mut owner);
        assert_eq!(cancelled, 1);
        assert_eq!(counts, (7, 8));
        assert_eq!(full, Err(capacity()));
        assert_eq!(released, 0);
        let expected = (1..8)
            .map(|i| (session(1), ids[i], tokens[i].clone()))
            .collect::<Vec<_>>();
        assert_results(&results, &expected);
        assert!(extra > ids[7]);
        assert_results(&extras, &[(session(1), extra, extra_token)]);
    }

    #[test]
    fn source_encoding_obsolete_relevance_cancels_only_owner() {
        for (present, generation, revision, wanted) in [
            (false, 7, 5, true),
            (true, 8, 5, true),
            (true, 7, 6, true),
            (true, 7, 5, false),
        ] {
            let mut owner = SourceSnapshotEncoding::try_new(1).unwrap();
            let a = capture(0);
            let b = capture(1);
            let a_id = owner.request(session(1), a.clone()).unwrap();
            let b_id = owner.request(session(2), b.clone()).unwrap();
            let mut arguments = vec![];
            let cancelled = owner
                .retain_current(|s, k, g, r| {
                    arguments.push((s, k, g, r));
                    s != session(1) || (present && generation == g && revision == r && wanted)
                })
                .unwrap();
            let results = complete(&mut owner);
            close(&mut owner);
            assert_eq!(cancelled, 1);
            assert_eq!(
                arguments,
                [(session(1), key(0), 7, 5), (session(2), key(1), 7, 5)]
            );
            assert_results(&results, &[(session(2), b_id, b)]);
            assert!(results.iter().all(|r| r.request() != a_id));
        }
    }

    #[test]
    fn source_encoding_held_completion_cancel_is_immediate() {
        let mut owner = SourceSnapshotEncoding::try_new(1).unwrap();
        let tokens = (0..4).map(capture).collect::<Vec<_>>();
        let ids = tokens
            .iter()
            .map(|v| owner.request(session(1), v.clone()).unwrap())
            .collect::<Vec<_>>();
        owner.wait(deadline()).unwrap();
        let before = (owner.retained_requests(), owner.charged_requests());
        let cancelled = owner.retain_current(|_, k, _, _| k != key(1)).unwrap();
        let after = (owner.retained_requests(), owner.charged_requests());
        let results = owner.collect_ready();
        let empty = owner
            .retain_current(|_, _, _, _| panic!("empty routing book"))
            .unwrap();
        owner.request(session(2), capture(8)).unwrap();
        owner.cancel().unwrap();
        let cancelled_all = (owner.retained_requests(), owner.charged_requests());
        owner.wait(deadline()).unwrap();
        let quiet = owner.collect_ready();
        owner.cancel().unwrap();
        close(&mut owner);
        assert_eq!(cancelled_all, (0, 1));
        assert!(quiet.is_empty());
        assert_eq!(before, (4, 4));
        assert_eq!(cancelled, 1);
        assert_eq!(after, (3, 3));
        assert_eq!(empty, 0);
        let expected = [0, 2, 3].map(|i| (session(1), ids[i], tokens[i].clone()));
        assert_results(&results, &expected);
    }

    #[test]
    fn source_encoding_actual_factory_failed_owner_and_healthy_peer() {
        let mut owner = SourceSnapshotEncoding::try_new(1).unwrap();
        let mut invalid = source(0, false);
        invalid.set_block(BlockPos::new(0, -64, 0), 32767);
        invalid.mark_blocks_dirty();
        invalid.finish_tick(false);
        let bad = invalid.capture(None, None);
        let good = capture(1);
        let bad_id = owner.request(session(1), bad.clone()).unwrap();
        let good_id = owner.request(session(2), good.clone()).unwrap();
        let results = complete(&mut owner);
        close(&mut owner);
        assert_eq!(results.len(), 2);
        assert_eq!(
            (results[0].session(), results[0].request()),
            (session(1), bad_id)
        );
        assert_eq!(results[0].capture(), &bad);
        assert_eq!(
            results[0].result().as_ref().unwrap_err(),
            &ServerError::InvalidInput {
                field: "chunk_network_snapshot"
            }
        );
        assert_results(&results[1..], &[(session(2), good_id, good)]);
    }

    #[test]
    fn source_encoding_expired_wait_retains_cohort_for_same_owner_retry() {
        let mut owner = SourceSnapshotEncoding::try_new(1).unwrap();
        let tokens = (0..8).map(capture).collect::<Vec<_>>();
        let ids = tokens
            .iter()
            .map(|v| owner.request(session(1), v.clone()).unwrap())
            .collect::<Vec<_>>();
        owner.stop_new().unwrap();
        let expired = owner.wait(Deadline::at(Instant::now()));
        let retained = owner.retained_requests();
        let admission = owner.request(session(1), capture(8));
        let results = complete(&mut owner);
        close(&mut owner);
        assert_eq!(
            expired,
            Err(ServerError::Timeout {
                operation: Operation::Shutdown
            })
        );
        assert_eq!(retained, 8);
        assert_eq!(
            admission,
            Err(ServerError::InvalidState {
                phase: ServerPhase::Closing
            })
        );
        let expected = (0..8)
            .map(|i| (session(1), ids[i], tokens[i].clone()))
            .collect::<Vec<_>>();
        assert_results(&results, &expected);
    }
}
