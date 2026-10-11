//! Public preparation/capture fixtures and executing contract doubles.
//! These cases qualify the actual factory, not disk acquisition or CPU threads.

use std::collections::VecDeque;

use mornlea_domain::{ChunkPos, Dimension, Event};
use mornlea_protocol::{Direction, ProtocolCodec, ServerPacket, State, read_frame_ref};
use mornlea_server::contracts::*;
use mornlea_server::core::chunk_encoding::{
    ChunkEncodePoll, ChunkEncodePort, ChunkEncodeRequestId, EncodedChunkSnapshot,
    MAX_CHUNK_ENCODE_REQUESTS,
};
use mornlea_server::core::publication::{EnqueueOutcome, PreparedFrame};
use mornlea_server::core::world::{ChunkSaveView, PreparedChunk};
use mornlea_server::state::{AuthorityState, TickContext};
use mornlea_storage::{Chunk, ContainerSnapshot, StorageKind};

fn key() -> ChunkKey {
    ChunkKey {
        dimension: Dimension::OVERWORLD,
        pos: ChunkPos::new(-2, -3),
    }
}

// Explicit off-tick preparation fixture copied from the accepted capture topic.
// Moving a Ready base here establishes no real disk acquisition or publisher.
fn authority() -> AuthorityState {
    let mut state = AuthorityState::try_new(
        ServerLimits::try_new(8, 4096, 512, 64, 64, 1_048_576).unwrap(),
        13,
    )
    .unwrap();
    let prepared = PreparedChunk::try_new(
        key(),
        7,
        RecoveredChunk {
            chunk: Chunk {
                sections: vec![
                    ContainerSnapshot {
                        kind: StorageKind::Single,
                        bits: 0,
                        single: 0,
                        palette: vec![],
                        packed: vec![],
                    };
                    24
                ],
                drops: vec![Default::default(); 32],
                furnaces: vec![Default::default(); 32],
                chests: vec![Default::default(); 16],
            },
            revision: 5,
            persisted_revision: 5,
            needs_rewrite: false,
            recovered: false,
        },
    )
    .unwrap();
    let mut ctx = TickContext::harness(&mut state, TickBudget::full());
    ctx.preload_ready_chunk(prepared.into_parts().0);
    let residents = ctx.resident_snapshot();
    drop(ctx);
    state.commit_residents(residents);
    state
}

fn capture(state: &AuthorityState) -> ChunkSaveView {
    let snapshot = state
        .capture_chunk_snapshot(key(), SaveUrgency::Autosave)
        .unwrap();
    let SaveValue::ChunkView(view) = snapshot.value else {
        panic!("lazy capture")
    };
    view
}

fn actual_encode(view: ChunkSaveView) -> EncodedChunkSnapshot {
    EncodedChunkSnapshot::encode(view, &mut ProtocolCodec::new().unwrap()).unwrap()
}

fn decode(frame: &PreparedFrame) -> ServerPacket {
    let wire = read_frame_ref(frame.as_bytes()).unwrap();
    assert_eq!(wire.consumed, frame.byte_len());
    assert_eq!(wire.packet_id, frame.packet_key().id);
    ServerPacket::ChunkSnapshot(
        ProtocolCodec::new()
            .unwrap()
            .decode_snapshot(wire.payload)
            .unwrap(),
    )
}

#[test]
fn checked_request_identity_is_nonzero_and_nominal() {
    assert_eq!(
        ChunkEncodeRequestId::try_new(0),
        Err(ServerError::InvalidInput {
            field: "chunk_encode_request",
        })
    );
    let one = ChunkEncodeRequestId::try_new(1).unwrap();
    let last = ChunkEncodeRequestId::try_new(u64::MAX).unwrap();
    assert_eq!((one.get(), last.get()), (1, u64::MAX));
    assert!(one < last);
    assert_ne!(
        std::any::TypeId::of::<ChunkEncodeRequestId>(),
        std::any::TypeId::of::<ChunkRequestId>()
    );
}

#[test]
fn actual_factory_retains_exact_capture_and_roundtrips_air() {
    let state = authority();
    let view = capture(&state);
    let expected =
        ServerPacket::try_from(Event::ChunkSnapshot(view.network_snapshot().unwrap().0)).unwrap();
    let output = actual_encode(view.clone());
    assert_eq!(output.capture(), &view);
    assert_eq!(
        (
            output.capture().key(),
            output.capture().generation(),
            output.capture().revision()
        ),
        (key(), 7, 5)
    );
    assert_eq!(output.section_payload_bytes(), 48);
    assert_eq!(output.frame().packet_key(), expected.key());
    assert_eq!(
        output.frame().packet_key().direction,
        Direction::ServerToClient
    );
    assert_eq!(output.frame().packet_key().state, State::Play);
    assert_eq!(decode(output.frame()), expected);
    let ServerPacket::ChunkSnapshot(packet) = expected else {
        unreachable!()
    };
    assert!(packet.logical_size() > output.section_payload_bytes());
    assert_ne!(output.frame().byte_len(), output.section_payload_bytes());
}

fn correlate(
    output: EncodedChunkSnapshot,
    expected: &ChunkSaveView,
) -> Result<PreparedFrame, ServerError> {
    if output.capture() != expected {
        return Err(ServerError::Internal {
            invariant: "chunk encode correlation",
        });
    }
    Ok(output.into_frame())
}

#[test]
fn equal_numeric_captures_are_distinct_and_frame_survives_owner_drop() {
    let (first, second, output, packet) = {
        let state = authority();
        let first = capture(&state);
        let second = capture(&state);
        let packet =
            ServerPacket::try_from(Event::ChunkSnapshot(first.network_snapshot().unwrap().0))
                .unwrap();
        let output = actual_encode(first.clone());
        (first, second, output, packet)
    };
    assert_eq!(
        (first.key(), first.generation(), first.revision()),
        (second.key(), second.generation(), second.revision())
    );
    assert_ne!(first, second);
    assert_eq!(output.capture(), &first);
    assert_ne!(output.capture(), &second);
    let retained = output.frame().clone();
    let pointer = retained.as_bytes().as_ptr();
    let bytes = retained.as_bytes().to_vec();
    let moved = correlate(output, &first).unwrap();
    drop(first);
    drop(second);
    assert_eq!(moved.as_bytes().as_ptr(), pointer);
    assert_eq!(moved.packet_key(), retained.packet_key());
    assert_eq!(moved.as_bytes(), bytes);
    assert_eq!(decode(&moved), packet);
}

// This bounded executing double deliberately has no OS owners or join claims.
// Only its explicit off-tick harness constructs actual factory results.
struct EncodingDouble {
    records: VecDeque<Record>,
    last: u64,
    stopping: bool,
    closed: bool,
}
struct Record {
    id: ChunkEncodeRequestId,
    capture: ChunkSaveView,
    outcome: ChunkEncodePoll,
}
impl EncodingDouble {
    fn new() -> Self {
        Self {
            records: VecDeque::with_capacity(MAX_CHUNK_ENCODE_REQUESTS),
            last: 0,
            stopping: false,
            closed: false,
        }
    }
    fn execute_off_tick(&mut self, id: ChunkEncodeRequestId, substitute: Option<ChunkSaveView>) {
        let record = self.records.iter_mut().find(|r| r.id == id).unwrap();
        let input = substitute.unwrap_or_else(|| record.capture.clone());
        record.outcome =
            match EncodedChunkSnapshot::encode(input, &mut ProtocolCodec::new().unwrap()) {
                Ok(output) => ChunkEncodePoll::Ready(output),
                Err(error) => ChunkEncodePoll::Failed(error),
            };
    }
    fn fail_explicitly(&mut self, id: ChunkEncodeRequestId, error: ServerError) {
        self.records
            .iter_mut()
            .find(|r| r.id == id)
            .unwrap()
            .outcome = ChunkEncodePoll::Failed(error);
    }
}
impl ChunkEncodePort for EncodingDouble {
    fn start_encode(
        &mut self,
        capture: ChunkSaveView,
    ) -> Result<ChunkEncodeRequestId, ServerError> {
        if self.closed {
            return Err(ServerError::InvalidState {
                phase: ServerPhase::Closed,
            });
        }
        if self.stopping {
            return Err(ServerError::InvalidState {
                phase: ServerPhase::Closing,
            });
        }
        if capture.generation() == 0 || capture.revision() == 0 {
            return Err(ServerError::InvalidInput {
                field: "chunk_network_snapshot",
            });
        }
        if self.records.len() == MAX_CHUNK_ENCODE_REQUESTS {
            return Err(ServerError::Capacity {
                resource: Resource::ChunkEncodes,
                limit: 8,
                observed: 9,
            });
        }
        let raw = self.last.checked_add(1).ok_or(ServerError::Internal {
            invariant: "chunk encode request space",
        })?;
        let id = ChunkEncodeRequestId::try_new(raw)?;
        self.records.push_back(Record {
            id,
            capture,
            outcome: ChunkEncodePoll::Pending,
        });
        self.last = raw;
        Ok(id)
    }
    fn poll_encode(&mut self, request: ChunkEncodeRequestId) -> ChunkEncodePoll {
        let Some(index) = self
            .records
            .iter()
            .position(|r| r.id == request && !matches!(r.outcome, ChunkEncodePoll::Pending))
        else {
            return ChunkEncodePoll::Pending;
        };
        self.records.remove(index).unwrap().outcome
    }
    fn cancel_encode(&mut self, request: ChunkEncodeRequestId) -> Result<(), ServerError> {
        if let Some(index) = self.records.iter().position(|r| r.id == request) {
            self.records.remove(index);
        }
        Ok(())
    }
}
struct Consumer {
    request: ChunkEncodeRequestId,
    capture: ChunkSaveView,
}
impl Consumer {
    fn start(port: &mut impl ChunkEncodePort, capture: ChunkSaveView) -> Self {
        let request = port.start_encode(capture.clone()).unwrap();
        Self { request, capture }
    }
    fn poll(&self, port: &mut impl ChunkEncodePort) -> Result<Option<PreparedFrame>, ServerError> {
        match port.poll_encode(self.request) {
            ChunkEncodePoll::Pending => Ok(None),
            ChunkEncodePoll::Ready(output) => correlate(output, &self.capture).map(Some),
            ChunkEncodePoll::Failed(error) => Err(error),
        }
    }
}

#[test]
fn executing_double_consumer_correlates_pending_ready_failed_and_cancelled() {
    let state = authority();
    let mut port = EncodingDouble::new();
    let consumer = Consumer::start(&mut port, capture(&state));
    assert!(consumer.poll(&mut port).unwrap().is_none());
    port.execute_off_tick(consumer.request, None);
    assert_eq!(
        decode(&consumer.poll(&mut port).unwrap().unwrap()),
        ServerPacket::try_from(Event::ChunkSnapshot(
            consumer.capture.network_snapshot().unwrap().0
        ))
        .unwrap()
    );
    assert!(consumer.poll(&mut port).unwrap().is_none());
    let forged = Consumer::start(&mut port, capture(&state));
    let sibling = capture(&state);
    assert_eq!(sibling.revision(), forged.capture.revision());
    assert_ne!(sibling, forged.capture);
    port.execute_off_tick(forged.request, Some(sibling));
    assert!(matches!(
        forged.poll(&mut port),
        Err(ServerError::Internal {
            invariant: "chunk encode correlation"
        })
    ));
    let failed = Consumer::start(&mut port, capture(&state));
    port.fail_explicitly(
        failed.request,
        ServerError::InvalidInput { field: "packet" },
    );
    assert!(matches!(
        failed.poll(&mut port),
        Err(ServerError::InvalidInput { field: "packet" })
    ));
    let cancelled = Consumer::start(&mut port, capture(&state));
    port.cancel_encode(cancelled.request).unwrap();
    port.cancel_encode(cancelled.request).unwrap();
    assert!(cancelled.poll(&mut port).unwrap().is_none());
    let unknown = ChunkEncodeRequestId::try_new(u64::MAX).unwrap();
    port.cancel_encode(unknown).unwrap();
    assert!(matches!(
        port.poll_encode(unknown),
        ChunkEncodePoll::Pending
    ));
    let held = Consumer::start(&mut port, capture(&state));
    port.execute_off_tick(held.request, None);
    port.cancel_encode(held.request).unwrap();
    assert!(held.poll(&mut port).unwrap().is_none());
}

#[test]
fn executing_double_bounds_held_results_and_uses_monotonic_ids() {
    let state = authority();
    let mut port = EncodingDouble::new();
    let ids: Vec<_> = (0..8)
        .map(|_| port.start_encode(capture(&state)).unwrap())
        .collect();
    port.execute_off_tick(ids[0], None);
    assert_eq!(
        port.start_encode(capture(&state)),
        Err(ServerError::Capacity {
            resource: Resource::ChunkEncodes,
            limit: 8,
            observed: 9
        })
    );
    assert_eq!(port.last, 8);
    port.cancel_encode(ids[1]).unwrap();
    let later = port.start_encode(capture(&state)).unwrap();
    assert_eq!(later.get(), 9);
    assert!(matches!(port.poll_encode(ids[1]), ChunkEncodePoll::Pending));
    assert!(matches!(
        port.poll_encode(ids[0]),
        ChunkEncodePoll::Ready(_)
    ));
    assert!(port.start_encode(capture(&state)).is_ok());
    port.stopping = true;
    assert_eq!(
        port.start_encode(capture(&state)),
        Err(ServerError::InvalidState {
            phase: ServerPhase::Closing
        })
    );
    port.closed = true;
    assert_eq!(
        port.start_encode(capture(&state)),
        Err(ServerError::InvalidState {
            phase: ServerPhase::Closed
        })
    );
    let mut exhausted = EncodingDouble::new();
    exhausted.last = u64::MAX;
    assert_eq!(
        exhausted.start_encode(capture(&state)),
        Err(ServerError::Internal {
            invariant: "chunk encode request space"
        })
    );
    assert!(exhausted.records.is_empty());
}

// A publication receipt double records a whole owner and a configured receipt.
// The actual source publisher remains a separate integration responsibility.
struct PublicationReceiptDouble {
    outcome: EnqueueOutcome,
    received: Option<PreparedFrame>,
}
impl PublicationReceiptDouble {
    fn enqueue(&mut self, frame: PreparedFrame) -> EnqueueOutcome {
        if self.outcome == EnqueueOutcome::Queued {
            self.received = Some(frame);
        }
        self.outcome
    }
}
#[test]
fn publication_receipt_double_advances_mirror_only_after_queued() {
    let state = authority();
    let output = actual_encode(capture(&state));
    let revision = output.capture().revision();
    let frame = output.into_frame();
    let pointer = frame.as_bytes().as_ptr();
    let mut mirror = 0;
    for (outcome, expected_mirror) in [
        (EnqueueOutcome::Closed, 0),
        (EnqueueOutcome::Queued, revision),
        (EnqueueOutcome::Closed, revision),
    ] {
        let mut port = PublicationReceiptDouble {
            outcome,
            received: None,
        };
        if port.enqueue(frame.clone()) == EnqueueOutcome::Queued {
            mirror = revision;
        }
        if outcome == EnqueueOutcome::Queued {
            assert_eq!(port.received.unwrap().as_bytes().as_ptr(), pointer);
            assert_eq!(mirror, revision);
        } else {
            assert!(port.received.is_none());
        }
        assert_eq!(mirror, expected_mirror);
    }
}
