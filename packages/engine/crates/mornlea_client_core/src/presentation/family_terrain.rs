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
//! terrain-only frame) before anything commits. A rejected publication
//! preserves the previous output and the previous resource-reference set
//! in full; only the successful path applies the staged arena releases and
//! swaps the reference set. The preparation owners themselves still advance
//! their own state on a failed frame — their port semantics are
//! non-transactional by contract — but nothing they retain is released or
//! published by a rejected vector.

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
/// It owns exactly two pieces of family state: the complete resource keys
/// the last accepted output referenced (its release obligations toward the
/// arena) and the admission-time material/light summaries of the near jobs
/// it admitted (one slot per terrain key; a newer identity of the same
/// section replaces it). Every world, ring and arena fact is borrowed from
/// the real owners at publication time.
pub struct TerrainPublisher {
    published: Vec<PreparedResourceKey>,
    summaries: Vec<(PreparedResourceKey, SectionSummary)>,
}

impl TerrainPublisher {
    pub fn try_new() -> Result<Self, ClientError> {
        Ok(Self {
            published: Vec::new(),
            summaries: Vec::new(),
        })
    }

    /// The complete resource keys the last accepted output referenced, in
    /// publication order. A rejected publication leaves this unchanged.
    pub fn published_resources(&self) -> &[PreparedResourceKey] {
        &self.published
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
    /// drained through the port probe. Results completed by this frame's
    /// work step are therefore drained by the next frame's dispatch, which
    /// is the owners' own pipelined discipline; the probe keeps every
    /// drained identity, including the ones the selection drops, so no
    /// retained geometry is ever lost to the drop channel.
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
        // a record, because the old frame was dropped wholesale.
        let mut removes: Vec<TerrainRecord> = Vec::new();
        let mut kept: Vec<PreparedResourceKey> = Vec::new();
        let mut release: Vec<PreparedResourceKey> = Vec::new();
        for key in &self.published {
            if key.epoch() != view.frame_epoch() {
                release.push(*key);
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
                release.push(*key);
            }
        }

        // The real work step, then the real selection frame over the same
        // real queue through the recording probe.
        queue.work(budget);
        let drained = {
            let mut probe = PortProbe {
                queue,
                drained: Vec::new(),
            };
            selection.dispatch_frame(&mut probe, center_tile, params)?;
            probe.drained
        };

        // Classification of everything the frame drained. A failed build
        // retained nothing; a result of another epoch, a far result of
        // another generation or an untracked tile, and a near result that
        // no longer matches the confirmed chunk revision never publish and
        // release their retained geometry exactly once. The freshest
        // delivery of one full key wins the upsert slot.
        let mut near_upserts: Vec<TerrainRecord> = Vec::new();
        let mut far_upserts: Vec<TerrainRecord> = Vec::new();
        for (key, delivered) in drained {
            if !delivered {
                continue;
            }
            if key.epoch() != view.frame_epoch() {
                release.push(key);
                continue;
            }
            match key.key() {
                TerrainKey::Section(section) => {
                    if held_revision(view, key.dimension(), section.chunk())
                        != Some(key.content_revision())
                    {
                        // A stale mesh resets: it never publishes and its
                        // arena slot returns through the staged release.
                        release.push(key);
                        continue;
                    }
                    let Some((_, summary)) = self.summaries.iter().find(|(held, _)| held == &key)
                    else {
                        // Only a job this publisher admitted carries the
                        // attribution a record needs; anything else is not
                        // this family's work and releases.
                        release.push(key);
                        continue;
                    };
                    replace_or_push(
                        &mut near_upserts,
                        upsert_record(view, key, summary.material, summary.light)?,
                    );
                }
                TerrainKey::LodTile(tile) => {
                    if key.generation() != selection.generation() || !selection.tracks(*tile) {
                        release.push(key);
                        continue;
                    }
                    replace_or_push(
                        &mut far_upserts,
                        upsert_record(view, key, TerrainMaterial::Opaque, far_shell_light())?,
                    );
                }
            }
        }

        // Assemble and check the complete vector before anything commits:
        // the frozen per-family record count, then the measured frame byte
        // bound through the accepted frame validator's own accounting over
        // a minimal terrain-only frame, so a family that could never fit a
        // legal frame rejects here instead of at publication.
        let mut records = removes;
        records.append(&mut near_upserts);
        records.append(&mut far_upserts);
        if records.len() > view.limits().family_records() {
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
            return Err(ClientError::Capacity);
        }

        // The single commit: apply the staged arena releases exactly once
        // (a late or repeated forget after a supersede or an invalidation
        // releases nothing), then swap the reference set to the survivors
        // plus this frame's upserts.
        for key in &release {
            let _ = queue.forget(key);
        }
        kept.extend(
            records
                .iter()
                .filter_map(|record| record.resource().copied()),
        );
        self.published = kept;
        Ok(records)
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

/// The freshest delivery of one full key wins its upsert slot: a later
/// result of the same terrain key replaces the earlier record in place, so
/// one frame publishes at most one current record per full key.
fn replace_or_push(records: &mut Vec<TerrainRecord>, record: TerrainRecord) {
    if let Some(slot) = records.iter_mut().find(|held| held.key() == record.key()) {
        *slot = record;
    } else {
        records.push(record);
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
