//! Current immutable source captures stay separate from save scheduling.

use super::{AuthorityState, ChunkKey, ServerError, ServerPhase};
use crate::core::acquisition::LiveChunkPhase;
use crate::core::world::{ChunkSaveView, ReadyChunk};

impl AuthorityState {
    /// Borrows current Ready scalar identity for bounded source relevance checks.
    /// Ordinary source consumers query after successful carried settlement.
    pub fn source_snapshot_identity(
        &self,
        key: ChunkKey,
    ) -> Result<Option<(u64, u64)>, ServerError> {
        let Some(chunk) = self.source_snapshot_ready(key)? else {
            return Ok(None);
        };
        Ok(Some((
            chunk.generation,
            self.source_snapshot_revision(key, chunk)?,
        )))
    }

    /// Shares immutable pages and fixed physical slots for off-tick encoding.
    /// Each capture is a distinct token; current scalar equality is no receipt.
    pub fn capture_source_snapshot(
        &self,
        key: ChunkKey,
    ) -> Result<Option<ChunkSaveView>, ServerError> {
        let Some(chunk) = self.source_snapshot_ready(key)? else {
            return Ok(None);
        };
        // Refuse overflow before the legacy capture's checked-write assumption.
        self.source_snapshot_revision(key, chunk)?;
        Ok(Some(chunk.capture(
            self.residents.drops.get(&key),
            self.residents.container_chunks.get(&key),
        )))
    }

    fn source_snapshot_ready(&self, key: ChunkKey) -> Result<Option<&ReadyChunk>, ServerError> {
        if let Some(error) = self.tick_failure {
            return Err(error);
        }
        if self.phase != ServerPhase::Running {
            return Err(ServerError::InvalidState { phase: self.phase });
        }
        let chunk = self.residents.ready.get(&key);
        if self.acquisition.enabled() {
            let Some(facts) = self
                .acquisition
                .facts(key)
                .filter(|f| f.phase == LiveChunkPhase::Ready)
            else {
                return Ok(None);
            };
            // Managed readiness belongs to both the scalar book and exact body.
            if !chunk.is_some_and(|r| {
                r.key == key && r.generation == facts.generation && r.revision == facts.revision
            }) {
                return Err(ServerError::Internal {
                    invariant: "source snapshot ready identity",
                });
            }
        }
        let Some(chunk) = chunk else {
            return Ok(None);
        };
        if chunk.key != key {
            return Err(ServerError::Internal {
                invariant: "source snapshot ready identity",
            });
        }
        if chunk.generation == 0 || chunk.revision == 0 {
            return Err(ServerError::InvalidInput {
                field: "chunk_network_snapshot",
            });
        }
        Ok(Some(chunk))
    }

    fn source_snapshot_revision(
        &self,
        key: ChunkKey,
        chunk: &ReadyChunk,
    ) -> Result<u64, ServerError> {
        chunk.checked_pending_revision(
            self.residents
                .drops
                .get(&key)
                .is_some_and(|state| state.dirty)
                || self
                    .residents
                    .container_chunks
                    .get(&key)
                    .is_some_and(|state| state.dirty),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::super::*;
    use crate::core::source_encoding::{SourceEncodeResult, SourceSnapshotEncoding};
    use crate::core::world::{
        materializations, ready_clones, reset_materializations, reset_ready_clones,
    };
    use mornlea_protocol::{ProtocolCodec, read_frame_ref};
    use mornlea_storage::{Chunk, ContainerSnapshot, StorageKind};
    use std::time::{Duration, Instant};

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
    fn ready(x: i32, generation: u64, revision: u64) -> ReadyChunk {
        ReadyChunk::try_new(
            key(x),
            generation,
            revision,
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
    fn session(n: u64) -> SessionKey {
        SessionKey::from_raw(n).unwrap()
    }
    fn deadline() -> Deadline {
        Deadline::after(Instant::now(), Duration::from_secs(10)).unwrap()
    }
    fn finish(owner: &mut SourceSnapshotEncoding) -> Vec<SourceEncodeResult> {
        owner.stop_new().unwrap();
        owner.wait(deadline()).unwrap();
        let results = owner.collect_ready();
        owner.close(deadline()).unwrap();
        results
    }
    fn decoded(result: &SourceEncodeResult) -> mornlea_protocol::ChunkSnapshot {
        let encoded = result.result().as_ref().unwrap();
        let wire = read_frame_ref(encoded.frame().as_bytes()).unwrap();
        assert_eq!(wire.consumed, encoded.frame().byte_len());
        ProtocolCodec::new()
            .unwrap()
            .decode_snapshot(wire.payload)
            .unwrap()
    }

    #[test]
    fn source_snapshot_ready_identity_and_fresh_tokens() {
        let mut state = authority();
        state.residents.ready.insert(key(0), ready(0, 7, 5));
        let sparse = key(1);
        let pos = BlockPos::new(16, -64, 0);
        state.residents.blocks.insert(
            (sparse, pos),
            BlockObservation::try_new(sparse, 9, 3, pos, 1).unwrap(),
        );
        reset_ready_clones();
        reset_materializations();
        assert_eq!(state.source_snapshot_identity(key(0)), Ok(Some((7, 5))));
        let a = state.capture_source_snapshot(key(0)).unwrap().unwrap();
        let b = state.capture_source_snapshot(key(0)).unwrap().unwrap();
        assert_ne!(a, b);
        assert_eq!((a.key(), a.generation(), a.revision()), (key(0), 7, 5));
        for missing in [
            sparse,
            key(2),
            ChunkKey {
                dimension: Dimension::DEPTHS,
                pos: ChunkPos::new(0, 0),
            },
        ] {
            assert_eq!(state.source_snapshot_identity(missing), Ok(None));
            assert_eq!(state.capture_source_snapshot(missing), Ok(None));
        }
        assert_eq!((ready_clones(), materializations()), (0, 0));
        assert_eq!(state.next_tick(), 0);
        assert_eq!(state.residents.ready.len(), 1);
        assert_eq!(state.residents.blocks.len(), 1);
    }

    #[test]
    fn source_snapshot_actual_cpu_cancellation_uses_current_authority() {
        for replace in [false, true] {
            let mut state = authority();
            state.residents.ready.insert(key(0), ready(0, 7, 5));
            state.residents.ready.insert(key(1), ready(1, 7, 5));
            let old = state.capture_source_snapshot(key(0)).unwrap().unwrap();
            let peer = state.capture_source_snapshot(key(1)).unwrap().unwrap();
            let mut owner = SourceSnapshotEncoding::try_new(1).unwrap();
            let old_id = owner.request(session(1), old.clone()).unwrap();
            let peer_id = owner.request(session(2), peer.clone()).unwrap();
            if replace {
                state.residents.ready.insert(key(0), ready(0, 8, 5));
            } else {
                let r = state.residents.ready.get_mut(&key(0)).unwrap();
                r.set_block(BlockPos::new(0, -64, 0), 1);
                r.mark_blocks_dirty();
                r.finish_tick(false);
            }
            let current = state.source_snapshot_identity(key(0));
            let cancelled = owner
                .retain_current(|_, k, generation, revision| {
                    state.source_snapshot_identity(k) == Ok(Some((generation, revision)))
                })
                .unwrap();
            let results = finish(&mut owner);
            assert_eq!(current, Ok(Some(if replace { (8, 5) } else { (7, 6) })));
            assert_eq!(cancelled, 1);
            assert_eq!(results.len(), 1);
            assert_ne!(results[0].request(), old_id);
            assert_eq!(
                (results[0].session(), results[0].request()),
                (session(2), peer_id)
            );
            assert_eq!(results[0].capture(), &peer);
            assert_eq!(results[0].result().as_ref().unwrap().capture(), &peer);
            let wire = decoded(&results[0]);
            assert_eq!((wire.chunk_x, wire.chunk_z, wire.revision), (1, 0, 5));
            assert_eq!(wire.sections.len(), 24);
            assert!(wire.sections.iter().all(|s| s.single == 0 && s.bits == 0));
            assert_eq!(
                results[0]
                    .result()
                    .as_ref()
                    .unwrap()
                    .section_payload_bytes(),
                48
            );
            let (old_body, charge) = old.network_snapshot().unwrap();
            assert_eq!(charge, 48);
            assert!(old_body.sections().iter().all(|s| s.as_single() == Some(0)));
        }
    }

    #[test]
    fn source_snapshot_fixed_slot_capture_retains_equal_revision_values() {
        let mut state = authority();
        let r = ready(0, 7, 5);
        let slots = r.drop_slots().try_into().unwrap();
        state.residents.ready.insert(key(0), r);
        state
            .residents
            .drops
            .insert(key(0), DropState::new(key(0), slots));
        let old = state.capture_source_snapshot(key(0)).unwrap().unwrap();
        state.residents.drops.get_mut(&key(0)).unwrap().slots[31].age_ticks = 13;
        let current = state.capture_source_snapshot(key(0)).unwrap().unwrap();
        let identity = state.source_snapshot_identity(key(0));
        let mut owner = SourceSnapshotEncoding::try_new(2).unwrap();
        let old_id = owner.request(session(1), old.clone()).unwrap();
        let current_id = owner.request(session(2), current.clone()).unwrap();
        let results = finish(&mut owner);
        assert_ne!(old, current);
        assert_eq!(identity, Ok(Some((7, 5))));
        assert_eq!(old.materialize().chunk.drops[31].age_ticks, 0);
        assert_eq!(current.materialize().chunk.drops[31].age_ticks, 13);
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].request(), old_id);
        assert_eq!(results[1].request(), current_id);
        assert_eq!(results[0].capture(), &old);
        assert_eq!(results[1].capture(), &current);
        assert_eq!(decoded(&results[0]), decoded(&results[1]));
        for result in results {
            assert_eq!(
                result.result().as_ref().unwrap().section_payload_bytes(),
                48
            );
        }
    }

    #[test]
    fn source_snapshot_sticky_failure_and_phase_precede_capture() {
        for phase in [ServerPhase::Closing, ServerPhase::Closed] {
            let mut state = authority();
            state.residents.ready.insert(key(0), ready(0, 7, 5));
            state.phase = phase;
            reset_ready_clones();
            reset_materializations();
            assert_eq!(
                state.source_snapshot_identity(key(0)),
                Err(ServerError::InvalidState { phase })
            );
            assert_eq!(
                state.capture_source_snapshot(key(0)),
                Err(ServerError::InvalidState { phase })
            );
            assert_eq!((ready_clones(), materializations()), (0, 0));
            assert_eq!(state.residents.ready.len(), 1);
            let failure = ServerError::Internal {
                invariant: "source snapshot prepared tick failure",
            };
            state.fail_tick(failure);
            for k in [key(0), key(9)] {
                assert_eq!(state.source_snapshot_identity(k), Err(failure));
                assert_eq!(state.capture_source_snapshot(k), Err(failure));
            }
        }
        let mut state = authority();
        state.residents.ready.insert(key(0), ready(0, 0, 5));
        assert_eq!(
            state.source_snapshot_identity(key(0)),
            Err(ServerError::InvalidInput {
                field: "chunk_network_snapshot"
            })
        );
        assert_eq!(
            state.capture_source_snapshot(key(0)),
            Err(ServerError::InvalidInput {
                field: "chunk_network_snapshot"
            })
        );
        state.residents.ready.insert(key(0), ready(1, 7, 5));
        let wrong = ServerError::Internal {
            invariant: "source snapshot ready identity",
        };
        assert_eq!(state.source_snapshot_identity(key(0)), Err(wrong));
        assert_eq!(state.capture_source_snapshot(key(0)), Err(wrong));
    }

    #[test]
    fn source_snapshot_managed_unready_resident_never_becomes_ready() {
        let mut state = authority();
        state.enable_live_chunks().unwrap();
        state.residents.ready.insert(key(0), ready(0, 7, 5));
        assert_eq!(state.source_snapshot_identity(key(0)), Ok(None));
        assert_eq!(state.capture_source_snapshot(key(0)), Ok(None));
        state.replace_chunk_wants(BTreeSet::from([key(0)])).unwrap();
        let reservation = state.reserve_chunk_load(key(0)).unwrap();
        state
            .bind_chunk_load(reservation, ChunkRequestId::try_new(1).unwrap())
            .unwrap();
        assert_eq!(
            state.live_chunk_facts(key(0)).unwrap().phase,
            LiveChunkPhase::Loading
        );
        assert_eq!(state.source_snapshot_identity(key(0)), Ok(None));
        assert_eq!(state.capture_source_snapshot(key(0)), Ok(None));
        assert_eq!(state.residents.ready.len(), 1);
    }

    #[test]
    fn source_snapshot_invalid_network_input_reaches_actual_failed_owner() {
        let mut state = authority();
        let mut r = ready(0, 7, 5);
        r.set_block(BlockPos::new(0, -64, 0), 32767);
        r.mark_blocks_dirty();
        r.finish_tick(false);
        state.residents.ready.insert(key(0), r);
        let token = state
            .capture_source_snapshot(key(0))
            .unwrap()
            .expect("network capture cannot depend on disk estimate");
        let mut owner = SourceSnapshotEncoding::try_new(1).unwrap();
        let id = owner.request(session(1), token.clone()).unwrap();
        let results = finish(&mut owner);
        assert_eq!(results.len(), 1);
        assert_eq!(
            (results[0].session(), results[0].request()),
            (session(1), id)
        );
        assert_eq!(results[0].capture(), &token);
        assert_eq!(token.revision(), 6);
        assert_eq!(
            results[0].result().as_ref().unwrap_err(),
            &ServerError::InvalidInput {
                field: "chunk_network_snapshot"
            }
        );
    }

    #[test]
    fn source_snapshot_exhausted_pending_revision_is_typed() {
        let mut state = authority();
        state.residents.ready.insert(key(0), ready(0, 7, u64::MAX));
        assert_eq!(
            state.source_snapshot_identity(key(0)),
            Ok(Some((7, u64::MAX)))
        );
        assert_eq!(
            state
                .capture_source_snapshot(key(0))
                .unwrap()
                .unwrap()
                .revision(),
            u64::MAX
        );
        for slots_dirty in [false, true] {
            let mut state = authority();
            let mut r = ready(0, 7, u64::MAX);
            if slots_dirty {
                let mut drops = DropState::new(key(0), r.drop_slots().try_into().unwrap());
                drops.dirty = true;
                state.residents.drops.insert(key(0), drops);
            } else {
                r.set_block(BlockPos::new(0, -64, 0), 1);
                r.mark_blocks_dirty();
            }
            state.residents.ready.insert(key(0), r);
            let error = ServerError::Internal {
                invariant: "source snapshot revision",
            };
            assert_eq!(state.source_snapshot_identity(key(0)), Err(error));
            assert_eq!(state.capture_source_snapshot(key(0)), Err(error));
        }
    }
}
