//! The terrain family publication owner: the integration seam between the
//! two accepted real preparation owners and the confirmed mirror.
//!
//! This integrator connects the bounded `PreparationQueue` resource arena
//! and the `LodSelection` far-tile ring with the mirror's confirmed chunk
//! revisions, and publishes the checked `terrain@1` change vector: near
//! `Section` upserts for freshly prepared meshes whose identity still
//! matches the mirror's held chunk revision, far `LodTile` upserts for
//! current-generation shell tiles the ring tracks, and ordered `Remove`
//! records for full keys whose chunk the mirror no longer holds at the same
//! content revision, whose tile left the far ring, or whose generation the
//! ring left behind. A removal always precedes reuse of the same full key,
//! every record carries its full terrain key and — for an upsert — its
//! complete prepared-resource reference only, and no GPU handle or Python
//! geometry exists anywhere in the vector.
//!
//! Integration seam: [`LodSelection::dispatch_frame`] drains the shared
//! port itself, and a result it classifies as stale, untracked or foreign
//! is dropped after the real arena already retained it. The private port
//! probe below is the lossless answer to that drop channel: it is a
//! transparent adapter over the real queue that records every drained
//! result identity, so the family learns the exact key of each dropped
//! geometry and releases it with `forget` instead of leaking arena
//! ownership. Near jobs are admitted through [`TerrainPublisher::admit_near`]
//! so the same owner can summarize each payload's material class and light
//! channels from the real shared registry at admission — the registry
//! fields the mesh oracles classify (fluid height, cutout transparency,
//! block emission) — because the drained result no longer carries its
//! input payload. The native numerical builds inside the queue and the
//! near-ring section selection remain later seams; this family consumes
//! both owners exactly as they landed.
//!
//! Bounds and failure policy: the assembled vector is checked against the
//! frozen family-record count and against the measured frame byte bound
//! (through the accepted frame validator's own accounting over a minimal
//! terrain-only frame) before the reference set commits. A rejected
//! publication preserves the previous output and the previous
//! resource-reference set in full, per the retry-owned model: an upsert the
//! frame rejected stays eligible for the next publication — its geometry
//! is neither consumed nor forgotten — while every drained geometry whose
//! key was never published releases through `forget` on every exit path,
//! before any cap decision, so the arena never retains a geometry the
//! prior output does not reference. The preparation owners themselves
//! still advance their own state on a failed frame — their port semantics
//! are non-transactional by contract — but nothing they retain is released
//! or published by a rejected vector. The admission-summary store is
//! bounded alongside: a slot survives only while its exact admission is
//! live in publication, still in flight, or retry-owned through the retry
//! list's own copy, so removed sections and reset epochs stop holding
//! slots and a re-admitted section records its summary again.

use std::sync::Arc;

use mornlea_domain::{ChunkPos, Dimension};
use mornlea_engine::native::contracts::world::WorldgenParams;

use crate::contracts::{
    ClientError, ClientWorkBudget, FAMILY_TERRAIN, FamilyKey, FamilyOperation, PreparationPort,
    RecordHeader, SessionEpoch,
};
use crate::preparation::lod::LodSelection;
use crate::preparation::{
    InvalidationReport, LightSummary, OwnedMeshView, PreparationJob, PreparationPayload,
    PreparationQueue, PreparationResult, PreparationTicket, PreparedResourceKey,
    RejectedPreparation, TerrainKey, TerrainVisibility, lod,
};
use crate::presentation::ProjectionView;
use crate::presentation::frame::{
    FamilyFrame, FamilyRecords, PresentationFrame, TerrainMaterial, TerrainRecord,
};

/// The far-shell light summary: the procedural shell renders a sky-lit
/// opaque surface, so the sky channel is full and the block channel has no
/// source. Material and light for a near record come from its recorded
/// admission summary instead.
fn far_shell_light() -> LightSummary {
    LightSummary::try_new(15, 0).expect("checked shell summary")
}

/// The empty summary a removal carries: a drop names its full key and no
/// content, so the record's material and light fields are the neutral
/// values the checked constructor admits.
fn removal_light() -> LightSummary {
    LightSummary::try_new(0, 0).expect("checked removal summary")
}

/// The far-shell admission summary: the same neutral class a fresh far
/// upsert carries, kept beside the retry entry so a re-attempt needs no
/// other state.
fn far_shell_summary() -> SectionSummary {
    SectionSummary {
        material: TerrainMaterial::Opaque,
        light: far_shell_light(),
    }
}

/// The material class and light channels of one near payload, recorded once
/// at admission from the real owned mesh view and its shared registry.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct SectionSummary {
    material: TerrainMaterial,
    light: LightSummary,
}

/// Summarizes one owned near payload: a referenced fluid entry publishes
/// `Water`, a referenced cutout entry `Cutout`, and otherwise the section is
/// `Opaque`; the block channel is the brightest referenced emission and the
/// sky channel is full exactly when the section contains air. These are the
/// registry facts the accepted mesh oracles classify; per-quad light and
/// sky occlusion above the section belong to the later native build seam
/// and are deliberately not invented here.
fn summarize_payload(payload: &OwnedMeshView) -> SectionSummary {
    let registry = payload.registry();
    let entries = registry.entries();
    let air = registry.air();
    let mut material = TerrainMaterial::Opaque;
    let mut block = 0u8;
    let mut sky = false;
    for &value in payload.blocks().iter() {
        if value == air {
            sky = true;
            continue;
        }
        // The registry keeps entries sorted by id, so the resolution is a
        // binary search; a value no entry resolves contributes no class,
        // exactly as the native mesher treats an unknown block.
        let Ok(index) = entries.binary_search_by(|entry| entry.id.cmp(&value)) else {
            continue;
        };
        let entry = &entries[index];
        if entry.fluid_height > 0 {
            material = TerrainMaterial::Water;
        } else if !entry.opaque && material != TerrainMaterial::Water {
            material = TerrainMaterial::Cutout;
        }
        block = block.max(entry.emission);
    }
    // The registry constructor bounds every emission at fifteen, so both
    // channels are always inside the checked light range.
    SectionSummary {
        material,
        light: LightSummary::try_new(if sky { 15 } else { 0 }, block)
            .expect("registry emissions are within the light range"),
    }
}

/// The terrain family publication owner.
///
/// It owns exactly three pieces of family state: the complete resource keys
/// the last accepted output referenced (its release obligations toward the
/// arena), the admission-time material/light summaries of the near jobs it
/// admitted (one slot per terrain key; a newer identity of the same section
/// replaces it, and slots leaving publication or reset across epochs are
/// pruned), and the retry list of upserts a rejected frame did not publish
/// (each entry carries its own summary, so a re-attempt depends on nothing
/// else). Every world, ring and arena fact is borrowed from the real owners
/// at publication time.
pub struct TerrainPublisher {
    published: Vec<PreparedResourceKey>,
    summaries: Vec<(PreparedResourceKey, SectionSummary)>,
    retry: Vec<(PreparedResourceKey, SectionSummary)>,
}

impl TerrainPublisher {
    pub fn try_new() -> Result<Self, ClientError> {
        Ok(Self {
            published: Vec::new(),
            summaries: Vec::new(),
            retry: Vec::new(),
        })
    }

    /// The complete resource keys the last accepted output referenced, in
    /// publication order. A rejected publication leaves this unchanged.
    pub fn published_resources(&self) -> &[PreparedResourceKey] {
        &self.published
    }

    /// The number of live admission-summary slots. Bounded by the sections
    /// currently published, still in flight, or just dropped this frame.
    pub fn summary_slots(&self) -> usize {
        self.summaries.len()
    }

    /// The number of upserts a rejected frame left retry-owned. Zero after
    /// every accepted publication.
    pub fn retry_owned(&self) -> usize {
        self.retry.len()
    }

    /// Admits one near preparation job through the real queue while
    /// recording the payload's material/light summary under its exact
    /// resource key. The queue stays the owner of storage, work and
    /// retention; a rejected admission returns the complete job unchanged
    /// and the recorded summary is simply replaced by the next admission of
    /// the same section. Far jobs belong to the selection's own dispatch
    /// and carry no summary.
    pub fn admit_near(
        &mut self,
        queue: &mut PreparationQueue,
        job: PreparationJob,
    ) -> Result<PreparationTicket, RejectedPreparation> {
        let key = *job.key();
        if let PreparationPayload::Near(payload) = job.payload() {
            let summary = summarize_payload(payload);
            self.summaries.retain(|(held, _)| held.key() != key.key());
            self.summaries.push((key, summary));
        }
        queue.try_submit(job)
    }

    /// Publishes one terrain family change vector through the real owners.
    ///
    /// The step order is fixed: far-ring maintenance around the center
    /// tile, removal derivation against the mirror and the ring, the real
    /// work step under the demanded budget, then the real selection frame
    /// drained through the port probe with every drained identity captured
    /// before any error propagates. Results completed by this frame's work
    /// step are therefore drained by the next frame's dispatch, which is
    /// the owners' own pipelined discipline. Retry-owned upserts from a
    /// previously rejected frame re-attempt first, so rejected work never
    /// strands behind fresher work.
    pub fn publish(
        &mut self,
        view: &ProjectionView<'_>,
        queue: &mut PreparationQueue,
        selection: &mut LodSelection,
        center: ChunkPos,
        params: &Arc<WorldgenParams>,
        budget: ClientWorkBudget,
    ) -> Result<Vec<TerrainRecord>, ClientError> {
        if selection.epoch() != view.frame_epoch() {
            // A selection of another epoch never feeds this frame; the
            // frame's terrain family must be rebased by its controller.
            return Err(ClientError::StaleEpoch);
        }
        let center_tile = lod::tile_from_chunk(center);
        // Ring maintenance first: release what left the band, then queue
        // what entered it, so membership is exact before anything drains.
        selection.remove_out_of_ring(center_tile);
        selection.queue_ring(center_tile);

        // Removal derivation and release staging. A published key leaves
        // publication through one ordered Remove record when its chunk left
        // the mirror or fell behind the confirmed chunk revision, or when
        // the ring no longer tracks its tile at the current generation. A
        // key of another epoch is the cross-epoch reset: released without
        // a record, because the old frame was dropped wholesale. These
        // releases retire references the prior output still holds, so they
        // apply only on the successful path.
        let mut removes: Vec<TerrainRecord> = Vec::new();
        let mut kept: Vec<PreparedResourceKey> = Vec::new();
        let mut retire: Vec<PreparedResourceKey> = Vec::new();
        let mut leaving: Vec<PreparedResourceKey> = Vec::new();
        for key in &self.published {
            if key.epoch() != view.frame_epoch() {
                retire.push(*key);
                leaving.push(*key);
                continue;
            }
            let current = match key.key() {
                TerrainKey::Section(section) => {
                    // The committed effect of a forget or a newer snapshot:
                    // the mirror holds the chunk at a different revision.
                    held_revision(view, key.dimension(), section.chunk())
                        == Some(key.content_revision())
                }
                TerrainKey::LodTile(tile) => {
                    selection.tracks(*tile) && key.generation() == selection.generation()
                }
            };
            if current {
                kept.push(*key);
            } else {
                removes.push(removal_record(view, *key)?);
                retire.push(*key);
                leaving.push(*key);
            }
        }
        // Summary bounding: an admission slot survives only while its exact
        // admission is live in publication, still in flight toward a drain,
        // or retry-owned through the retry list's own copy — a key that
        // just left publication or crossed an epoch boundary stops holding
        // a slot, and a re-admitted section records its summary again.
        if !leaving.is_empty() {
            self.summaries.retain(|(held, _)| !leaving.contains(held));
        }

        // The real work step, then the real selection frame over the same
        // real queue through the recording probe. The drained identities
        // are captured before any dispatch error propagates, so every exit
        // path below handles them.
        queue.work(budget);
        let (drained, dispatch_error) = {
            let mut probe = PortProbe {
                queue,
                drained: Vec::new(),
            };
            let outcome = selection.dispatch_frame(&mut probe, center_tile, params);
            let drained = probe.drained;
            (drained, outcome.err())
        };

        // Classification shared by every exit path from here on: upserts
        // this frame attempts (retry-owned entries first, then the fresh
        // drain) and the never-published keys that must release.
        let mut near_upserts: Vec<TerrainRecord> = Vec::new();
        let mut far_upserts: Vec<TerrainRecord> = Vec::new();
        let mut attempted: Vec<(PreparedResourceKey, SectionSummary)> = Vec::new();
        let mut dropped: Vec<PreparedResourceKey> = Vec::new();

        // Retry-owned re-attempts: an upsert a previously rejected frame
        // did not publish retries first. It survives only while its
        // identity is still current and the real arena still retains its
        // geometry; anything else releases as never-published.
        let retrying = std::mem::take(&mut self.retry);
        for (key, summary) in retrying {
            if !self.retry_current(view, selection, queue, &key) {
                dropped.push(key);
                continue;
            }
            let record = upsert_record(view, key, summary.material, summary.light)?;
            push_upsert(&mut near_upserts, &mut far_upserts, record);
            attempted.push((key, summary));
        }

        // The fresh drain. A failed build retained nothing; a result of
        // another epoch, a far result of another generation or an untracked
        // tile, a near result that no longer matches the confirmed chunk
        // revision or was never admitted through this publisher, and a
        // result the arena no longer retains never publish — each releases
        // as never-published. The freshest delivery of one full key wins
        // the upsert slot.
        for (key, delivered) in drained {
            if !delivered {
                continue;
            }
            if key.epoch() != view.frame_epoch() {
                dropped.push(key);
                continue;
            }
            let summary = match key.key() {
                TerrainKey::Section(section) => {
                    if held_revision(view, key.dimension(), section.chunk())
                        != Some(key.content_revision())
                    {
                        // A stale mesh resets: it never publishes and its
                        // arena slot returns through the release below.
                        dropped.push(key);
                        self.forget_summary(&key);
                        continue;
                    }
                    match self.summaries.iter().find(|(held, _)| held == &key) {
                        Some((_, summary)) => *summary,
                        None => {
                            // Only a job this publisher admitted carries the
                            // attribution a record needs.
                            dropped.push(key);
                            continue;
                        }
                    }
                }
                TerrainKey::LodTile(tile) => {
                    if key.generation() != selection.generation() || !selection.tracks(*tile) {
                        dropped.push(key);
                        continue;
                    }
                    far_shell_summary()
                }
            };
            // A record publishes only while the real arena still retains
            // its geometry: a later delivery of the same terrain key
            // supersedes the retained entry out from under an earlier
            // candidate of the same drain.
            if queue.prepared_resource(&key).is_err() {
                dropped.push(key);
                self.forget_summary(&key);
                continue;
            }
            let record = upsert_record(view, key, summary.material, summary.light)?;
            push_upsert(&mut near_upserts, &mut far_upserts, record);
            attempted.push((key, summary));
        }

        // Never-published releases apply on every exit path from here,
        // before any cap decision: the arena retains nothing the prior
        // output does not reference, exactly as the retry-owned model
        // requires — the dropped keys were never referenced.
        for key in &dropped {
            let _ = queue.forget(key);
        }

        if let Some(error) = dispatch_error {
            // The frame failed inside the real selection dispatch after it
            // drained: the attempted upserts stay retry-owned, the dropped
            // geometries released above, and the prior output and its
            // references are untouched.
            self.retry = attempted;
            return Err(error);
        }

        // Assemble and check the complete vector before the reference set
        // commits: the frozen per-family record count, then the measured
        // frame byte bound through the accepted frame validator's own
        // accounting over a minimal terrain-only frame, so a family that
        // could never fit a legal frame rejects here instead of at
        // publication. A rejected vector leaves its upserts retry-owned.
        let mut records = removes;
        records.append(&mut near_upserts);
        records.append(&mut far_upserts);
        if records.len() > view.limits().family_records() {
            self.retry = attempted;
            return Err(ClientError::Capacity);
        }
        let candidate = PresentationFrame::try_new(
            view.frame_epoch(),
            view.frame_revision(),
            view.frame_index(),
            vec![FamilyFrame::try_new(
                FamilyKey::try_new(FAMILY_TERRAIN)?,
                FamilyRecords::Terrain(records.clone()),
            )?],
        )?;
        if candidate.validated_size()? > view.limits().frame_bytes() {
            self.retry = attempted;
            return Err(ClientError::Capacity);
        }

        // The single commit: apply the staged retirements exactly once (a
        // late or repeated forget after a supersede or an invalidation
        // releases nothing), then swap the reference set to the survivors
        // plus this frame's upserts. Every attempted upsert published or
        // was superseded inside the same vector by a fresher identity of
        // its terrain key, whose delivery already released the older
        // geometry, so the retry list empties.
        for key in &retire {
            let _ = queue.forget(key);
        }
        kept.extend(
            records
                .iter()
                .filter_map(|record| record.resource().copied()),
        );
        self.published = kept;
        self.retry = Vec::new();
        Ok(records)
    }

    /// Whether one retry-owned identity may still publish: its epoch is the
    /// frame's, the real arena still retains its geometry, and its section
    /// or tile is still current against the mirror and the ring.
    fn retry_current(
        &self,
        view: &ProjectionView<'_>,
        selection: &LodSelection,
        queue: &PreparationQueue,
        key: &PreparedResourceKey,
    ) -> bool {
        if key.epoch() != view.frame_epoch() {
            return false;
        }
        if queue.prepared_resource(key).is_err() {
            return false;
        }
        match key.key() {
            TerrainKey::Section(section) => {
                held_revision(view, key.dimension(), section.chunk())
                    == Some(key.content_revision())
            }
            TerrainKey::LodTile(tile) => {
                selection.tracks(*tile) && key.generation() == selection.generation()
            }
        }
    }

    /// Drops one exact admission-summary slot whose candidate just left the
    /// publishable set.
    fn forget_summary(&mut self, key: &PreparedResourceKey) {
        self.summaries.retain(|(held, _)| held != key);
    }
}

/// The confirmed content revision of one chunk column, if the mirror holds
/// it. The lookup key is the mirror's own typed triple.
fn held_revision(view: &ProjectionView<'_>, dimension: Dimension, chunk: ChunkPos) -> Option<u64> {
    view.mirror()
        .world()
        .chunks()
        .get(&(dimension.get(), i64::from(chunk.x()), i64::from(chunk.z())))
        .copied()
}

/// The visibility class of one full terrain key; it always matches the key
/// tag, which the checked record constructor enforces again.
fn visibility_of(key: &TerrainKey) -> TerrainVisibility {
    match key {
        TerrainKey::Section(_) => TerrainVisibility::Near,
        TerrainKey::LodTile(_) => TerrainVisibility::Far,
    }
}

/// One ordered removal record: the full key names what leaves publication;
/// the material and light fields carry the neutral summary because a drop
/// has no content.
fn removal_record(
    view: &ProjectionView<'_>,
    key: PreparedResourceKey,
) -> Result<TerrainRecord, ClientError> {
    let header = RecordHeader::try_new(
        view.frame_epoch(),
        view.frame_revision(),
        None,
        FamilyOperation::Remove,
    )?;
    TerrainRecord::try_new(
        header,
        key.dimension(),
        *key.key(),
        key.content_revision(),
        key.generation(),
        TerrainMaterial::Opaque,
        visibility_of(key.key()),
        removal_light(),
        None,
    )
}

/// One upsert record: the complete prepared-resource reference beside the
/// full key and the identity the resource was built from.
fn upsert_record(
    view: &ProjectionView<'_>,
    key: PreparedResourceKey,
    material: TerrainMaterial,
    light: LightSummary,
) -> Result<TerrainRecord, ClientError> {
    let header = RecordHeader::try_new(
        view.frame_epoch(),
        view.frame_revision(),
        None,
        FamilyOperation::Upsert,
    )?;
    TerrainRecord::try_new(
        header,
        key.dimension(),
        *key.key(),
        key.content_revision(),
        key.generation(),
        material,
        visibility_of(key.key()),
        light,
        Some(key),
    )
}

/// Routes one upsert into its class vector: the freshest delivery of one
/// full key wins the slot, so one frame publishes at most one current
/// record per full key.
fn push_upsert(near: &mut Vec<TerrainRecord>, far: &mut Vec<TerrainRecord>, record: TerrainRecord) {
    let target = match record.key() {
        TerrainKey::Section(_) => near,
        TerrainKey::LodTile(_) => far,
    };
    if let Some(slot) = target.iter_mut().find(|held| held.key() == record.key()) {
        *slot = record;
    } else {
        target.push(record);
    }
}

/// The transparent port probe: a `PreparationPort` adapter over the real
/// bounded queue that records the exact identity of every drained result
/// and whether the real arena delivered a geometry for it.
///
/// `LodSelection::dispatch_frame` drops every result it classifies as
/// stale, untracked or foreign — including any near-section result that
/// shares the port — after the real `poll_ready` already retained its
/// geometry. The probe is the only place those identities survive, which is
/// what lets the publication owner release the dropped geometries instead
/// of leaking them; delegation itself is byte-for-byte the real owner.
struct PortProbe<'a> {
    queue: &'a mut PreparationQueue,
    drained: Vec<(PreparedResourceKey, bool)>,
}

impl PreparationPort for PortProbe<'_> {
    fn try_submit(
        &mut self,
        job: PreparationJob,
    ) -> Result<PreparationTicket, RejectedPreparation> {
        self.queue.try_submit(job)
    }

    fn poll_ready(&mut self) -> Option<PreparationResult> {
        let result = self.queue.poll_ready()?;
        let key = *result.key();
        let delivered = result.outcome().is_ok();
        self.drained.push((key, delivered));
        Some(result)
    }

    fn invalidate(&mut self, epoch: SessionEpoch) -> InvalidationReport {
        self.queue.invalidate(epoch)
    }
}
