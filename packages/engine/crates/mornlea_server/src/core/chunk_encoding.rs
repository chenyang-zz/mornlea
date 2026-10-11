//! Checked off-tick chunk framing and bounded CPU request ownership.
//! Publication eligibility and authority state remain with their consumers.

use std::num::NonZeroU64;

use mornlea_domain::Event;
use mornlea_protocol::{ProtocolCodec, ServerPacket};

use super::contracts::ServerError;
use super::publication::PreparedFrame;
use super::world::ChunkSaveView;

/// Maximum queued, started and held-complete requests across all CPU owners.
pub const MAX_CHUNK_ENCODE_REQUESTS: usize = 8;

/// Process-local encoding identity, independent of disk and generation IDs.
///
/// ```compile_fail
/// use mornlea_server::contracts::ChunkRequestId;
/// use mornlea_server::core::chunk_encoding::ChunkEncodeRequestId;
/// let disk: ChunkRequestId = ChunkEncodeRequestId::try_new(1).unwrap();
/// ```
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct ChunkEncodeRequestId(NonZeroU64);

impl ChunkEncodeRequestId {
    /// Rejects zero without creating an ambiguous request identity.
    pub fn try_new(raw: u64) -> Result<Self, ServerError> {
        NonZeroU64::new(raw)
            .map(Self)
            .ok_or(ServerError::InvalidInput {
                field: "chunk_encode_request",
            })
    }

    pub fn get(self) -> u64 {
        self.0.get()
    }
}

/// One exact capture, its checked semantic snapshot, section charge and frame.
///
/// Equality of key, generation and revision cannot replace capture identity.
/// This owner is deliberately not Clone; borrowed frames may be Arc-cloned.
#[derive(Debug)]
pub struct EncodedChunkSnapshot {
    capture: ChunkSaveView,
    snapshot: mornlea_domain::ChunkSnapshot,
    section_payload_bytes: usize,
    frame: PreparedFrame,
}

impl EncodedChunkSnapshot {
    /// Extracts, converts and frames a supplied immutable capture OFF TICK.
    ///
    /// No disk materialization or authority borrow occurs. Network extraction
    /// errors propagate unchanged; packet conversion and codec refusal use the
    /// existing packet input error. The section charge excludes the envelope,
    /// compression and persistence estimate, and is 48 bytes for all-air data.
    pub fn encode(capture: ChunkSaveView, codec: &mut ProtocolCodec) -> Result<Self, ServerError> {
        let (snapshot, section_payload_bytes) = capture.network_snapshot()?;
        // Existing packet conversion consumes its value. Retain the semantic
        // owner here so later publication moves it without another expansion.
        let packet = ServerPacket::try_from(Event::ChunkSnapshot(snapshot.clone()))
            .map_err(|_| ServerError::InvalidInput { field: "packet" })?;
        let frame = PreparedFrame::encode(codec, &packet)?;
        Ok(Self {
            capture,
            snapshot,
            section_payload_bytes,
            frame,
        })
    }

    /// Borrows the exact supplied capture token for consumer correlation.
    pub fn capture(&self) -> &ChunkSaveView {
        &self.capture
    }

    /// Borrows the cached checked value without extracting capture data again.
    pub fn snapshot(&self) -> &mornlea_domain::ChunkSnapshot {
        &self.snapshot
    }

    /// Moves the exact capture, semantic allocations, section charge and frame.
    /// No expansion, cloning or codec work occurs on the consuming caller.
    pub fn into_source_parts(
        self,
    ) -> (
        ChunkSaveView,
        mornlea_domain::ChunkSnapshot,
        usize,
        PreparedFrame,
    ) {
        (
            self.capture,
            self.snapshot,
            self.section_payload_bytes,
            self.frame,
        )
    }

    pub fn section_payload_bytes(&self) -> usize {
        self.section_payload_bytes
    }

    pub fn frame(&self) -> &PreparedFrame {
        &self.frame
    }

    /// Moves the original frame, dropping unused semantic and capture owners.
    pub fn into_frame(self) -> PreparedFrame {
        self.frame
    }
}

/// Transfers one whole result; a Pending observation transfers no ownership.
pub enum ChunkEncodePoll {
    Pending,
    Ready(EncodedChunkSnapshot),
    Failed(ServerError),
}

/// Narrow caller operations for a separately owned bounded CPU lane.
///
/// Providers retain at most eight queued, started or held-complete requests
/// across one or two OS owners, with one started job per owner. Caller start,
/// poll and cancel move only bounded owners: no expansion, packing, framing,
/// codec allocation, disk/authority/socket lock or wait runs on the caller.
///
/// Admission precedence is stopping/closed (InvalidState Closing/Closed), then
/// zero capture generation or revision (InvalidInput chunk_network_snapshot),
/// then capacity (ChunkEncodes, limit 8, observed 9), then checked monotonic ID
/// exhaustion (Internal chunk encode request space). IDs are consumed only on
/// successful admission and never recycled. Other invalid capture facts fail
/// on the actual CPU factory and transfer as Failed, never a successful frame.
///
/// Successful admission retains the supplied capture until whole Ready/Failed
/// transfer or safe cancellation collection. Unknown, consumed or cancelled
/// polls are Pending; consumers correlate their own records before polling and
/// compare exact capture tokens before accepting a frame. Equal numeric
/// key/generation/revision identities are not interchangeable.
///
/// Unknown cancellation is idempotent Ok. Queued or held-complete cancellation
/// drops its record immediately. Started cancellation suppresses delivery but
/// retains its charge and capture/frame owners until the real reply is collected.
///
/// Concrete providers also implement the existing WorkerLifecycle: stop_new
/// fences admission; cancel removes idle/completed and marks started records;
/// wait(deadline) drains real queued/started work and may retain held results.
/// Successful close cancels, drains, disconnects and joins every actual owner.
/// Timeout retains handles and ownership for same-provider retry. Drop only
/// disconnects, making no join or quiescence claim. CPU code is not force
/// cancelled and completion is never invented. Contract doubles alone cannot
/// establish actual thread, started-cancellation charge or join behavior.
pub trait ChunkEncodePort {
    fn start_encode(&mut self, capture: ChunkSaveView)
    -> Result<ChunkEncodeRequestId, ServerError>;
    fn poll_encode(&mut self, request: ChunkEncodeRequestId) -> ChunkEncodePoll;
    fn cancel_encode(&mut self, request: ChunkEncodeRequestId) -> Result<(), ServerError>;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::contracts::ChunkKey;
    use crate::core::world::{ReadyChunk, materializations, reset_materializations};
    use mornlea_domain::{BlockPos, ChunkPos, Dimension};
    use mornlea_protocol::read_frame_ref;
    use mornlea_storage::{Chunk, ContainerSnapshot, StorageKind};

    fn ready(indexed: bool) -> ReadyChunk {
        let mut sections = vec![
            ContainerSnapshot {
                kind: StorageKind::Single,
                bits: 0,
                single: 0,
                palette: vec![],
                packed: vec![],
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
            ChunkKey {
                dimension: Dimension::OVERWORLD,
                pos: ChunkPos::new(-2, -3),
            },
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

    fn packet(output: &EncodedChunkSnapshot) -> mornlea_protocol::ChunkSnapshot {
        let wire = read_frame_ref(output.frame().as_bytes()).unwrap();
        ProtocolCodec::new()
            .unwrap()
            .decode_snapshot(wire.payload)
            .unwrap()
    }

    #[test]
    fn actual_factory_preserves_loaded_storage_then_packs_changed_first_appearance() {
        let mut source = ready(true);
        let old = source.capture(None, None);
        let mut codec = ProtocolCodec::new().unwrap();
        reset_materializations();
        let output = EncodedChunkSnapshot::encode(old.clone(), &mut codec).unwrap();
        assert_eq!(output.capture(), &old);
        let original = packet(&output);
        assert_eq!(original.sections[0].palette, [2, 0, 1]);
        assert_eq!(original.sections[0].bits, 4);
        assert_eq!(original.sections[0].packed, vec![0; 256]);
        assert_eq!(output.section_payload_bytes(), 2100);
        source.set_block(BlockPos::new(-32, -64, -48), 1);
        source.mark_blocks_dirty();
        source.finish_tick(false);
        let changed = source.capture(None, None);
        let current = EncodedChunkSnapshot::encode(changed.clone(), &mut codec).unwrap();
        assert_eq!(current.capture(), &changed);
        assert_ne!(current.capture(), output.capture());
        let changed_packet = packet(&current);
        assert_eq!(changed_packet.revision, 6);
        assert_eq!(changed_packet.sections[0].palette, [1, 2]);
        assert_eq!(changed_packet.sections[0].packed[0], 0x1111_1111_1111_1110);
        assert!(
            changed_packet.sections[0].packed[1..]
                .iter()
                .all(|&word| word == 0x1111_1111_1111_1111)
        );
        assert_eq!(packet(&output), original);
        assert_eq!(
            packet(&EncodedChunkSnapshot::encode(old, &mut codec).unwrap()),
            original
        );
        assert_eq!(materializations(), 0);
    }

    #[test]
    fn actual_factory_invalid_private_edit_refuses_without_disk_expansion() {
        let mut source = ready(false);
        let old = source.capture(None, None);
        source.set_block(BlockPos::new(-32, -64, -48), 32767);
        source.mark_blocks_dirty();
        source.finish_tick(false);
        let invalid = source.capture(None, None);
        let mut codec = ProtocolCodec::new().unwrap();
        reset_materializations();
        assert!(matches!(
            EncodedChunkSnapshot::encode(invalid, &mut codec),
            Err(ServerError::InvalidInput {
                field: "chunk_network_snapshot"
            })
        ));
        assert_eq!(materializations(), 0);
        let retained = EncodedChunkSnapshot::encode(old.clone(), &mut codec).unwrap();
        assert_eq!(retained.capture(), &old);
        assert_eq!(retained.section_payload_bytes(), 48);
        assert!(packet(&retained).sections.iter().all(|s| s.single == 0));
        assert_eq!(materializations(), 0);
    }

    fn assert_source_parts(
        output: EncodedChunkSnapshot,
        expected: &ChunkSaveView,
        revision: u64,
        charge: usize,
        indexed: Option<(&[u16], u64, u64)>,
    ) {
        let snapshot = output.snapshot();
        assert_eq!(snapshot.dimension(), Dimension::OVERWORLD);
        assert_eq!(snapshot.chunk(), ChunkPos::new(-2, -3));
        assert_eq!(snapshot.revision(), revision);
        assert_eq!(snapshot.sections().len(), 24);
        let sections_ptr = snapshot.sections().as_ptr();
        let indexed_ptrs = indexed.map(|(palette, first, rest)| {
            let (bits, actual_palette, words) = snapshot.sections()[0].as_indexed().unwrap();
            assert_eq!(bits, 4);
            assert_eq!(actual_palette, palette);
            assert_eq!(words.len(), 256);
            assert_eq!(words[0], first);
            assert!(words[1..].iter().all(|&word| word == rest));
            (actual_palette.as_ptr(), words.as_ptr())
        });
        let air_start = usize::from(indexed.is_some());
        assert!(
            snapshot.sections()[air_start..]
                .iter()
                .all(|s| s.as_single() == Some(0))
        );
        let frame_ptr = output.frame().as_bytes().as_ptr();
        let frame_bytes = output.frame().as_bytes().to_vec();
        let (capture, snapshot, section_charge, frame) = output.into_source_parts();
        assert_eq!(&capture, expected);
        assert_eq!(section_charge, charge);
        assert_eq!(snapshot.dimension(), Dimension::OVERWORLD);
        assert_eq!(snapshot.chunk(), ChunkPos::new(-2, -3));
        assert_eq!(snapshot.revision(), revision);
        assert_eq!(snapshot.sections().len(), 24);
        assert_eq!(snapshot.sections().as_ptr(), sections_ptr);
        assert_eq!(frame.as_bytes().as_ptr(), frame_ptr);
        assert_eq!(frame.as_bytes(), frame_bytes);
        let wire = read_frame_ref(frame.as_bytes()).unwrap();
        let decoded = ProtocolCodec::new()
            .unwrap()
            .decode_snapshot(wire.payload)
            .unwrap();
        assert_eq!(decoded.dimension, Dimension::OVERWORLD);
        assert_eq!((decoded.chunk_x, decoded.chunk_z), (-2, -3));
        assert_eq!(decoded.revision, revision);
        assert_eq!(decoded.sections.len(), 24);
        if let Some((palette, first, rest)) = indexed {
            let (bits, actual_palette, words) = snapshot.sections()[0].as_indexed().unwrap();
            let (palette_ptr, words_ptr) = indexed_ptrs.unwrap();
            assert_eq!(actual_palette.as_ptr(), palette_ptr);
            assert_eq!(words.as_ptr(), words_ptr);
            assert_eq!(bits, 4);
            assert_eq!(actual_palette, palette);
            assert_eq!(words.len(), 256);
            assert_eq!(words[0], first);
            assert!(words[1..].iter().all(|&word| word == rest));
            assert_eq!(decoded.sections[0].bits, 4);
            assert_eq!(decoded.sections[0].palette, palette);
            assert_eq!(decoded.sections[0].packed.len(), 256);
            assert_eq!(decoded.sections[0].packed[0], first);
            assert!(
                decoded.sections[0].packed[1..]
                    .iter()
                    .all(|&word| word == rest)
            );
        }
        assert!(
            snapshot.sections()[air_start..]
                .iter()
                .all(|s| s.as_single() == Some(0))
        );
        assert!(
            decoded.sections[air_start..]
                .iter()
                .all(|s| s.single == 0 && s.bits == 0)
        );
    }

    #[test]
    fn source_parts_air_moves_complete_owned_column() {
        let source = ready(false);
        let capture = source.capture(None, None);
        let output =
            EncodedChunkSnapshot::encode(capture.clone(), &mut ProtocolCodec::new().unwrap())
                .unwrap();
        assert_source_parts(output, &capture, 5, 48, None);
    }

    #[test]
    fn source_parts_loaded_indexed_moves_palette_and_words() {
        let source = ready(true);
        let capture = source.capture(None, None);
        let output =
            EncodedChunkSnapshot::encode(capture.clone(), &mut ProtocolCodec::new().unwrap())
                .unwrap();
        assert_source_parts(output, &capture, 5, 2100, Some((&[2, 0, 1], 0, 0)));
    }

    #[test]
    fn source_parts_changed_capture_remains_frozen() {
        let mut source = ready(true);
        source.set_block(BlockPos::new(-32, -64, -48), 1);
        source.mark_blocks_dirty();
        source.finish_tick(false);
        let capture = source.capture(None, None);
        let output =
            EncodedChunkSnapshot::encode(capture.clone(), &mut ProtocolCodec::new().unwrap())
                .unwrap();
        source.set_block(BlockPos::new(-31, -64, -48), 0);
        source.mark_blocks_dirty();
        source.finish_tick(false);
        drop(source);
        assert_source_parts(
            output,
            &capture,
            6,
            2098,
            Some((&[1, 2], 0x1111_1111_1111_1110, 0x1111_1111_1111_1111)),
        );
    }

    #[test]
    fn source_parts_actual_cpu_result_survives_joined_close() {
        use crate::core::contracts::{Deadline, WorkerLifecycle};
        use crate::core::encoding_worker::ChunkEncodingPool;
        use std::time::{Duration, Instant};

        let source = ready(true);
        let capture = source.capture(None, None);
        let mut pool = ChunkEncodingPool::try_new(1).unwrap();
        let request = pool.start_encode(capture.clone()).unwrap();
        let until = Instant::now() + Duration::from_secs(10);
        let output = loop {
            match pool.poll_encode(request) {
                ChunkEncodePoll::Ready(output) => break output,
                ChunkEncodePoll::Failed(error) => panic!("actual encode failed: {error:?}"),
                ChunkEncodePoll::Pending => {
                    assert!(Instant::now() < until, "actual encode did not finish");
                    std::thread::yield_now();
                }
            }
        };
        pool.stop_new().unwrap();
        pool.wait(Deadline::after(Instant::now(), Duration::from_secs(10)).unwrap())
            .unwrap();
        pool.close(Deadline::after(Instant::now(), Duration::from_secs(10)).unwrap())
            .unwrap();
        drop(pool);
        drop(source);
        assert_source_parts(output, &capture, 5, 2100, Some((&[2, 0, 1], 0, 0)));
    }
}
