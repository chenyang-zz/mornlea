//! Sequential per-recipient selection over actual CPU results and retained view books.

use super::*;
use crate::core::chunk_encoding::{ChunkEncodeRequestId, EncodedChunkSnapshot};
use crate::core::publication::PreparedSourcePublication;
use crate::core::source_encoding::SourceSnapshotEncoding;
use crate::core::world::ChunkSaveView;

struct EncodedRecipient {
    session: SessionKey,
    candidates: Vec<ChunkKey>,
    cursor: usize,
    bytes: usize,
    pending: Option<(ChunkEncodeRequestId, ChunkSaveView)>,
    outputs: Vec<(ChunkSaveView, EncodedChunkSnapshot)>,
    stopped: bool,
    refused: bool,
}

/// One original projection, whose recipient cursors never replay committed simulation.
pub(crate) struct EncodedSourceProjectionWork {
    projection: SourceProjectionWork,
    rows: Vec<EncodedRecipient>,
}

impl EncodedSourceProjectionWork {
    pub(crate) fn prepare(
        state: &mut AuthorityState,
        tick: u64,
        outcome: &TickOutcome,
    ) -> Result<Self, ServerError> {
        let projection = SourceProjectionWork::prepare(state, tick, outcome)?;
        let rows = projection
            .observers
            .iter()
            .zip(&projection.view_list)
            .map(|(observer, view)| EncodedRecipient {
                session: observer.session,
                candidates: snapshot_candidates(observer, view),
                cursor: 0,
                bytes: 0,
                pending: None,
                outputs: Vec::new(),
                stopped: projection.refused_sessions.contains(&observer.session)
                    || state.limits().snapshot_chunks() == 0,
                refused: false,
            })
            .collect();
        Ok(Self { projection, rows })
    }

    pub(crate) fn poll(
        &mut self,
        state: &AuthorityState,
        encoding: &mut SourceSnapshotEncoding,
    ) -> Result<bool, ServerError> {
        let limits = state.limits();
        // The dedicated owner contains at most one request per checked recipient.
        for result in encoding.collect_ready() {
            let (session, request, capture, result) = result.into_parts();
            let row = self
                .rows
                .iter_mut()
                .find(|row| row.session == session)
                .ok_or(ServerError::Internal {
                    invariant: "source snapshot recipient correlation",
                })?;
            let (expected_request, expected_capture) =
                row.pending.take().ok_or(ServerError::Internal {
                    invariant: "source snapshot pending correlation",
                })?;
            if request != expected_request
                || capture != expected_capture
                || state.source_snapshot_identity(capture.key())?
                    != Some((capture.generation(), capture.revision()))
            {
                return Err(ServerError::Internal {
                    invariant: "source snapshot current correlation",
                });
            }
            match result {
                Ok(encoded) => {
                    let charge = encoded.section_payload_bytes();
                    if encoded.capture() != &capture {
                        return Err(ServerError::Internal {
                            invariant: "source snapshot encoded capture",
                        });
                    }
                    if !row.outputs.is_empty() && row.bytes + charge > limits.snapshot_bytes() {
                        row.stopped = true;
                    } else {
                        row.bytes += charge;
                        row.outputs.push((capture, encoded));
                        if row.outputs.len() >= limits.snapshot_chunks() {
                            row.stopped = true;
                        }
                    }
                }
                Err(ServerError::InvalidInput {
                    field: "chunk_network_snapshot",
                }) => {
                    row.refused = true;
                    row.stopped = true;
                }
                Err(error) => return Err(error),
            }
        }
        let mut attempts = 0;
        for (index, row) in self.rows.iter_mut().enumerate() {
            while !row.stopped && row.pending.is_none() && attempts < 16 {
                let Some(&key) = row.candidates.get(row.cursor) else {
                    row.stopped = true;
                    break;
                };
                row.cursor += 1;
                attempts += 1;
                let Some(capture) = state.capture_source_snapshot(key)? else {
                    if let Some(entry) = self.projection.view_list[index].chunks.get_mut(&key) {
                        entry.resync_queued = false;
                    }
                    continue;
                };
                let request = encoding.request(row.session, capture.clone())?;
                row.pending = Some((request, capture));
            }
            // Exhaustion itself costs no candidate admission and may finish an empty row.
            if row.pending.is_none() && row.cursor == row.candidates.len() {
                row.stopped = true;
            }
        }
        Ok(self
            .rows
            .iter()
            .all(|row| row.stopped && row.pending.is_none())
            && encoding.charged_requests() == 0)
    }

    pub(crate) fn restore(self, state: &mut AuthorityState) {
        self.projection.restore(state);
    }

    pub(crate) fn finish(
        self,
        state: &mut AuthorityState,
        outcome: &TickOutcome,
    ) -> EncodedSourceProjection {
        let Self {
            mut projection,
            rows,
        } = self;
        let selected = rows
            .iter()
            .flat_map(|row| {
                row.outputs
                    .iter()
                    .map(move |(capture, _)| (row.session, capture.key()))
            })
            .collect();
        let early = std::mem::take(&mut projection.early);
        let refused_sessions = projection.refused_sessions.clone();
        let mut late = Vec::new();
        projection.finish_tail(state, outcome, &selected, &mut late);
        EncodedSourceProjection {
            early,
            rows,
            late,
            refused_sessions,
        }
    }
}

pub(crate) struct EncodedSourceProjection {
    early: Vec<RoutedEvent>,
    rows: Vec<EncodedRecipient>,
    late: Vec<RoutedEvent>,
    refused_sessions: Vec<SessionKey>,
}
impl EncodedSourceProjection {
    /// Append-only pairing preserves original CPU frames and ordered recipient cuts.
    pub(crate) fn append_to(
        self,
        batch: &mut PreparedSourcePublication,
    ) -> Result<(usize, Vec<SessionKey>), ServerError> {
        for event in self.early {
            batch.append_event(event);
        }
        let before_snapshots = batch.publication().events.len();
        for row in self.rows {
            for (capture, encoded) in row.outputs {
                batch.append_encoded_snapshot(row.session, &capture, encoded)?;
            }
            if row.refused {
                batch.append_source_refusal(row.session)?;
            }
        }
        for event in self.late {
            batch.append_event(event);
        }
        Ok((before_snapshots, self.refused_sessions))
    }
}
