//! Actual CPU outputs qualify the independent held-publication bound.

use super::*;
use crate::core::contracts::{ChunkKey, Deadline, Resource, ServerLimits, WorkerLifecycle};
use crate::core::source_encoding::SourceSnapshotEncoding;
use crate::core::world::{ChunkSaveView, ReadyChunk};
use mornlea_domain::{
    ChunkPos, CommandRejection, Dimension, Event, EventRecipient, RejectReason, RoutedEvent,
};
use mornlea_protocol::{ProtocolCodec, read_frame_ref};
use mornlea_storage::{Chunk, ContainerSnapshot, StorageKind};
use std::time::{Duration, Instant};

fn limits(players: u8, count: usize) -> ServerLimits {
    ServerLimits::try_new(players, 4096, 512, 64, count, 1_048_576).unwrap()
}
fn publication() -> TickPublication {
    TickPublication {
        tick: 9,
        events: vec![],
        control: vec![],
        counters: Default::default(),
    }
}
fn ordinary() -> RoutedEvent {
    RoutedEvent::new(
        EventRecipient::Broadcast,
        Event::CommandRejected(CommandRejection::new(66, RejectReason::InvalidRay)),
    )
}
fn session(n: u64) -> SessionKey {
    SessionKey::from_raw(n).unwrap()
}
fn capture(x: i32) -> ChunkSaveView {
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
    .capture(None, None)
}
fn deadline() -> Deadline {
    Deadline::after(Instant::now(), Duration::from_secs(10)).unwrap()
}
fn cpu(
    inputs: &[(SessionKey, ChunkSaveView)],
    workers: usize,
) -> Vec<crate::core::chunk_encoding::EncodedChunkSnapshot> {
    let mut owner = SourceSnapshotEncoding::try_new(workers).unwrap();
    let mut expected = Vec::new();
    let mut outputs = Vec::new();
    for group in inputs.chunks(8) {
        for (s, c) in group {
            expected.push((*s, owner.request(*s, c.clone()).unwrap(), c.clone()));
        }
        owner.wait(deadline()).unwrap();
        outputs.extend(owner.collect_ready());
    }
    owner.stop_new().unwrap();
    owner.wait(deadline()).unwrap();
    outputs.extend(owner.collect_ready());
    owner.close(deadline()).unwrap();
    // All deliberately failing product assertions run after the actual owner joins.
    assert_eq!(outputs.len(), inputs.len());
    outputs
        .into_iter()
        .zip(expected)
        .map(|(r, (s, id, c))| {
            let (actual_s, actual_id, actual_c, encoded) = r.into_parts();
            assert_eq!((actual_s, actual_id, actual_c), (s, id, c.clone()));
            let e = encoded.unwrap();
            assert_eq!(e.capture(), &c);
            assert_eq!(e.section_payload_bytes(), 48);
            e
        })
        .collect()
}
fn pairs(n: usize) -> Vec<(SessionKey, ChunkSaveView)> {
    (0..n).map(|i| (session(1), capture(i as i32))).collect()
}
fn append(
    batch: &mut PreparedSourcePublication,
    input: &(SessionKey, ChunkSaveView),
    e: crate::core::chunk_encoding::EncodedChunkSnapshot,
) -> Result<(), ServerError> {
    batch.append_encoded_snapshot(input.0, &input.1, e)
}
fn cap(limit: usize) -> ServerError {
    ServerError::Capacity {
        resource: Resource::Snapshots,
        limit,
        observed: limit + 1,
    }
}

#[test]
fn source_pass_budget_configured_pair_bound() {
    let input = pairs(11);
    let mut encoded = cpu(&input, 1).into_iter();
    let mut batch = PreparedSourcePublication::for_source_tick(publication(), limits(2, 5));
    batch.append_event(ordinary());
    for x in &input[..10] {
        append(&mut batch, x, encoded.next().unwrap()).unwrap();
    }
    assert_eq!(batch.publication().events.len(), 11);
    let before = batch.publication().clone();
    assert_eq!(
        append(&mut batch, &input[10], encoded.next().unwrap()),
        Err(cap(10))
    );
    assert_eq!(batch.publication(), &before);
}
#[test]
fn source_pass_budget_maximum_whole_pass() {
    let mut input: Vec<_> = (1..=8)
        .flat_map(|s| (0..64).map(move |x| (session(s), capture(x))))
        .collect();
    input.push((session(1), capture(64)));
    let mut encoded = cpu(&input, 2).into_iter();
    let mut batch = PreparedSourcePublication::for_source_tick(publication(), limits(8, 64));
    for x in &input[..512] {
        append(&mut batch, x, encoded.next().unwrap()).unwrap();
    }
    assert_eq!(batch.publication().events.len(), 512);
    for s in 1..=8 {
        let events: Vec<_> = batch
            .publication()
            .events
            .iter()
            .filter(|r| r.recipient() == EventRecipient::Session(s))
            .collect();
        assert_eq!(events.len(), 64);
        for (x, event) in events.iter().enumerate() {
            let Event::ChunkSnapshot(body) = event.event() else {
                panic!("source snapshot")
            };
            assert_eq!(
                (
                    body.dimension(),
                    body.chunk(),
                    body.revision(),
                    body.sections().len()
                ),
                (Dimension::OVERWORLD, ChunkPos::new(x as i32, 0), 5, 24)
            );
        }
    }
    let before = batch.publication().clone();
    assert_eq!(
        append(&mut batch, &input[512], encoded.next().unwrap()),
        Err(cap(512))
    );
    assert_eq!(batch.publication(), &before);
}
#[test]
fn source_pass_budget_zero_preserves_token_precedence() {
    let a = capture(0);
    let b = capture(0);
    assert_ne!(a, b);
    let input = [(session(1), b), (session(1), a.clone())];
    // Equal session/key inputs are collected in separate real owner passes.
    let mut encoded = cpu(&input[..1], 1);
    encoded.extend(cpu(&input[1..], 1));
    let mut encoded = encoded.into_iter();
    let mut batch = PreparedSourcePublication::for_source_tick(publication(), limits(8, 0));
    batch.append_event(ordinary());
    let before = batch.publication().clone();
    assert_eq!(
        batch.append_encoded_snapshot(session(1), &a, encoded.next().unwrap()),
        Err(ServerError::InvalidInput {
            field: "chunk_encode_capture"
        })
    );
    assert_eq!(batch.publication(), &before);
    assert_eq!(
        batch.append_encoded_snapshot(session(1), &a, encoded.next().unwrap()),
        Err(cap(0))
    );
    assert_eq!(batch.publication(), &before);
}
#[test]
fn source_pass_budget_legacy_eight_is_unchanged() {
    let input = pairs(10);
    let mut encoded = cpu(&input, 1).into_iter();
    let mut batch = PreparedSourcePublication::new(publication());
    for x in &input[..8] {
        append(&mut batch, x, encoded.next().unwrap()).unwrap();
    }
    let before = batch.publication().clone();
    assert_eq!(
        append(&mut batch, &input[8], encoded.next().unwrap()),
        Err(cap(8))
    );
    assert_eq!(batch.publication(), &before);
    assert_eq!(
        batch.append_encoded_snapshot(input[9].0, &input[0].1, encoded.next().unwrap()),
        Err(ServerError::InvalidInput {
            field: "chunk_encode_capture"
        })
    );
    assert_eq!(batch.publication(), &before);
}
#[test]
fn source_pass_budget_moves_original_pairs_with_interleaved_events() {
    let input = pairs(8);
    let encoded = cpu(&input, 2);
    let expected: Vec<_> = encoded
        .iter()
        .map(|e| {
            (
                e.snapshot().sections().as_ptr(),
                e.frame().as_bytes().as_ptr(),
                e.frame().as_bytes().to_vec(),
            )
        })
        .collect();
    let mut batch = PreparedSourcePublication::for_source_tick(publication(), limits(2, 4));
    batch.append_event(ordinary());
    for (x, e) in input.iter().zip(encoded) {
        append(&mut batch, x, e).unwrap();
        batch.append_event(ordinary());
    }
    let (publication, frames) = batch.into_parts();
    assert_eq!(publication.events.len(), 17);
    assert_eq!(frames.len(), 8);
    for ((index, frame), (i, (semantic, bytes, full))) in
        frames.into_iter().zip(expected.into_iter().enumerate())
    {
        assert_eq!(index, [1, 3, 5, 7, 9, 11, 13, 15][i]);
        let Event::ChunkSnapshot(body) = publication.events[index].event() else {
            panic!("paired snapshot")
        };
        assert_eq!(body.sections().as_ptr(), semantic);
        assert_eq!(frame.as_bytes().as_ptr(), bytes);
        assert_eq!(frame.as_bytes(), full);
        let wire = read_frame_ref(frame.as_bytes()).unwrap();
        assert_eq!(wire.consumed, frame.byte_len());
        let decoded = ProtocolCodec::new()
            .unwrap()
            .decode_snapshot(wire.payload)
            .unwrap();
        assert_eq!(
            (
                decoded.chunk_x,
                decoded.chunk_z,
                decoded.revision,
                decoded.sections.len()
            ),
            (i as i32, 0, 5, 24)
        );
    }
}
