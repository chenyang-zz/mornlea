//! Checked preparation and executing load-port consumers, independent of disk.

use std::time::{Duration, Instant};

use mornlea_domain::{ChunkPos, Dimension, PlayerId};
use mornlea_server::contracts::{
    ChunkKey, ChunkLoadPoll, ChunkLoadPort, ChunkRequestId, Deadline, LoadPoll, LoginTicket,
    Operation, PlayerLoadPort, RecoveredChunk, ServerError,
};
use mornlea_server::core::world::PreparedChunk;
use mornlea_storage::{Chunk, ContainerSnapshot, StorageKind};

fn key() -> ChunkKey {
    ChunkKey {
        dimension: Dimension::new(1).unwrap(),
        pos: ChunkPos::new(-1, 7),
    }
}

fn recovered() -> RecoveredChunk {
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
        revision: 9,
        persisted_revision: 4,
        needs_rewrite: true,
        recovered: true,
    }
}

#[test]
fn preparation_preserves_source_facts_and_moves_ready() {
    let prepared = PreparedChunk::try_new(key(), 3, recovered()).unwrap();
    assert_eq!(prepared.key(), key());
    assert_eq!(prepared.generation(), 3);
    assert_eq!(prepared.revision(), 9);
    assert_eq!(prepared.persisted_revision(), 4);
    assert!(prepared.needs_rewrite());
    assert!(prepared.recovered());
    let (_ready, persisted, rewrite, recovered) = prepared.into_parts();
    assert_eq!((persisted, rewrite, recovered), (4, true, true));
}

#[test]
fn preparation_rejects_identity_before_revision_and_storage() {
    let mut raw = recovered();
    raw.revision = 0;
    raw.chunk.sections.clear();
    assert!(matches!(
        PreparedChunk::try_new(key(), 0, raw),
        Err(ServerError::InvalidInput {
            field: "chunk_generation"
        })
    ));
}

#[test]
fn preparation_rejects_invalid_revision_facts() {
    for (revision, persisted) in [(0, 0), (4, 5)] {
        let mut raw = recovered();
        raw.revision = revision;
        raw.persisted_revision = persisted;
        assert!(matches!(
            PreparedChunk::try_new(key(), 1, raw),
            Err(ServerError::InvalidInput {
                field: "chunk_revision"
            })
        ));
    }
}

#[test]
fn preparation_checks_compact_storage_and_keeps_flags_independent() {
    let mut raw = recovered();
    raw.chunk.sections.clear();
    assert!(matches!(
        PreparedChunk::try_new(key(), 1, raw),
        Err(ServerError::InvalidInput {
            field: "ready_chunk"
        })
    ));
    let mut raw = recovered();
    raw.persisted_revision = raw.revision;
    raw.needs_rewrite = false;
    let prepared = PreparedChunk::try_new(key(), 1, raw).unwrap();
    assert!(!prepared.needs_rewrite());
    assert!(prepared.recovered());
}

struct LoadDouble {
    chunk: Option<ChunkLoadPoll>,
    player: Option<LoadPoll>,
    cancelled: usize,
}

impl PlayerLoadPort for LoadDouble {
    fn start(
        &mut self,
        _player: PlayerId,
        _deadline: Deadline,
    ) -> Result<LoginTicket, ServerError> {
        LoginTicket::try_from_raw(1)
    }
    fn poll(&mut self, _ticket: LoginTicket) -> LoadPoll {
        self.player.take().unwrap_or(LoadPoll::Pending)
    }
    fn cancel(&mut self, _ticket: LoginTicket) -> Result<(), ServerError> {
        self.player = None;
        self.cancelled += 1;
        Ok(())
    }
}

impl ChunkLoadPort for LoadDouble {
    fn start_chunk(
        &mut self,
        _key: ChunkKey,
        _generation: u64,
        _deadline: Deadline,
    ) -> Result<ChunkRequestId, ServerError> {
        ChunkRequestId::try_new(2)
    }
    fn poll_chunk(&mut self, _request: ChunkRequestId) -> ChunkLoadPoll {
        self.chunk.take().unwrap_or(ChunkLoadPoll::Pending)
    }
    fn cancel_chunk(&mut self, _request: ChunkRequestId) -> Result<(), ServerError> {
        self.chunk = None;
        self.cancelled += 1;
        Ok(())
    }
}

#[test]
fn consumer_calls_accept_prepared_missing_failure_and_cancel() {
    let deadline = Deadline::after(Instant::now(), Duration::from_secs(1)).unwrap();
    let player =
        PlayerId::try_from_bytes([1, 0, 0, 0, 0, 0, 0x40, 0, 0x80, 0, 0, 0, 0, 0, 0, 1]).unwrap();
    let mut double = LoadDouble {
        chunk: Some(ChunkLoadPoll::Loaded(Some(
            PreparedChunk::try_new(key(), 3, recovered()).unwrap(),
        ))),
        player: Some(LoadPoll::Loaded(None)),
        cancelled: 0,
    };
    let ticket = PlayerLoadPort::start(&mut double, player, deadline).unwrap();
    assert!(matches!(
        PlayerLoadPort::poll(&mut double, ticket),
        LoadPoll::Loaded(None)
    ));
    let request = ChunkLoadPort::start_chunk(&mut double, key(), 3, deadline).unwrap();
    let ChunkLoadPoll::Loaded(Some(prepared)) = ChunkLoadPort::poll_chunk(&mut double, request)
    else {
        panic!("prepared chunk")
    };
    assert_eq!(prepared.revision(), 9);
    double.chunk = Some(ChunkLoadPoll::Loaded(None));
    assert!(matches!(
        ChunkLoadPort::poll_chunk(&mut double, request),
        ChunkLoadPoll::Loaded(None)
    ));
    double.chunk = Some(ChunkLoadPoll::Failed(ServerError::Timeout {
        operation: Operation::Load,
    }));
    assert!(matches!(
        ChunkLoadPort::poll_chunk(&mut double, request),
        ChunkLoadPoll::Failed(ServerError::Timeout {
            operation: Operation::Load
        })
    ));
    PlayerLoadPort::cancel(&mut double, ticket).unwrap();
    ChunkLoadPort::cancel_chunk(&mut double, request).unwrap();
    assert_eq!(double.cancelled, 2);
    assert!(matches!(
        ChunkLoadPort::poll_chunk(&mut double, request),
        ChunkLoadPoll::Pending
    ));
}
