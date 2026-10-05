//! Fixed per-frame terrain upload/drop budgets, out-of-view reclamation,
//! and session reset for the pilot Godot terrain path.
//!
//! Design decision 5 of the Godot client migration rules the split: "a
//! Rust worker first prepares owned CPU arrays; the main thread submits
//! them under a fixed per-frame upload budget." This module is that
//! budget. One [`TerrainBudgetStage`] owns the whole terrain pipeline —
//! the mesh-prepare worker, the whole-section RID table, and the session
//! epoch — and one `frame` call per Godot frame performs bounded work in
//! a fixed order:
//!
//! 1. Reclamation first: held sections of the camera dimension beyond
//!    the view radius are freed, at most [`DROP_BUDGET_SECTIONS`] per
//!    frame. Drops run strictly before uploads so removals are never
//!    starved by upload pressure, and capacity and memory are freed
//!    before new work adds to them.
//! 2. Upload drain second: at most [`UPLOAD_BUDGET_SECTIONS`] prepared
//!    results leave the worker's completed queue per frame. FIFO is the
//!    worker's contract, so the remainder stays queued in submit order
//!    and the next frame continues where this one stopped. Bounding the
//!    drain matters because a completed result's retained value is
//!    expanded geometry — roughly twenty-five times the packed input,
//!    worst case near a hundred mebibytes at the queue bound — so the
//!    per-frame drain plus the worker's own backpressure keeps that
//!    retention transient instead of resident.
//!
//! Budget unit and values: the whole section is the atomic unit of every
//! stage of this pipeline — one worker result, one renderer upsert, one
//! renderer drop, one reclamation decision — so both budgets are plain,
//! decidable section counts. [`UPLOAD_BUDGET_SECTIONS`] is 64 because 64
//! section-operations per interactive frame is the proven steady-state
//! mesh budget of the client this pilot replaces (the Go application's
//! `SteadyFrameMeshWorkMax`). One maximal section submission — one mesh,
//! at most three surfaces, one instance — is the worst-case unit, and
//! real sections sit far below the frozen per-section quad ceiling.
//! [`DROP_BUDGET_SECTIONS`] is also 64: a drop is two RID frees, orders
//! of magnitude cheaper than an upsert, so an equal count keeps a
//! frame's worst case at 128 whole-section operations while reclamation
//! keeps exact pace with uploads.
//!
//! View policy, pinned to the authoritative streaming truth: the pilot
//! baseline manifest (`testdata/godot-pilot/baseline-manifest.json`)
//! records `view_distance: 32`, and the perfcheck gate validates that
//! field against the same 2..64 domain as the login protocol's
//! view-distance bounds — it is the login-declared view distance, in
//! chunks. The server derives the session subscription radius as
//! declared+1 chunks and keeps a chunk when the absolute delta of both
//! horizontal axes stays at most that radius: a horizontal Chebyshev
//! band with an inclusive boundary. Chunk X/Z equals section X/Z (a
//! chunk is one sixteen-block section column), so the radius translates
//! one-to-one into section units, and the vertical axis carries no
//! distance because the server streams whole chunk columns. Reclamation
//! mirrors exactly that set: a held section of the camera dimension is
//! reclaimed when its horizontal Chebyshev distance in section units
//! exceeds [`VIEW_RADIUS_SECTIONS`] (declared+1); a section exactly at
//! the radius is kept, matching the authoritative inclusive comparison;
//! Y never participates. The old client's own scheduling truth is the
//! same metric with no hysteresis (the LOD scheduler's exact Chebyshev
//! bands), so this pilot has none either.
//!
//! Drain-side staleness filter: a drained result is uploaded only when
//! it belongs to the current session epoch, its section lies inside the
//! camera's view band, and its decode succeeded. Everything else —
//! retired-epoch results, sections beyond the radius, decode failures —
//! is discarded at drain without any renderer call, so stale work never
//! spends upload budget on geometry the same frame's reclamation would
//! remove. Reclamation itself walks only the camera dimension: sections
//! held in other dimensions are session-reset territory (world re-entry
//! drives reset), and the drain filter prevents new cross-dimension
//! buildup because uploads happen only for the camera's dimension.
//!
//! Session reset cancels the worker's queued jobs, drains and discards
//! every already-published completed result (the worker's epoch filter
//! runs at publication, so a cancel alone does not retract results it
//! already published), resets the RID table, and moves the session to a
//! strictly greater epoch. Epochs are never reused after a cancel: every
//! reset allocates the next epoch before any later submit can run, so
//! the forbidden cancel-then-resubmit-under-one-epoch interleaving
//! cannot occur by construction.
//!
//! Metrics: [`TerrainBudgetStage::facts`] reports cumulative
//! session-lifetime accounting — packed input bytes at submit, expanded
//! output bytes and worker-side prepare durations at drain (measured by
//! the worker around its own decode, attributing prepare cost where it
//! happens instead of estimating it on the main thread), and renderer
//! wall time (reclamation frees plus section upserts) measured inside
//! `frame`. Timings are recorded observations only: no queue, budget, or
//! ordering decision in this module reads a clock. Live counts come
//! from the RID table at the end of each frame; peak counts are
//! session-lifetime high-water marks and deliberately survive `reset`,
//! because a peak that reset clears would defeat the purpose of a
//! high-water mark. RID counts are the two table-owned RIDs per live
//! section (mesh plus instance); the three borrowed material RIDs and
//! the borrowed scenario are constant construction inputs and are not
//! counted.
//!
//! Node prohibition: the terrain path is section-granular by
//! construction — every renderer operation in this pipeline is a
//! whole-section upsert, drop, or reclaim over exactly one mesh and one
//! instance RID, never a per-block scene-tree node — and the granularity
//! test pins that decidable: a full-block-budget section (the mesher
//! worst case of six quads per block across all 4096 blocks of a
//! section) costs exactly the same two table-owned RIDs as a one-quad
//! section.

// This module's `#![allow(dead_code)]` era ended when the bridge terrain
// surface became its production consumer: the bridge session owns one
// stage, submits pulled batches, drives one frame per Godot frame, and
// reports the facts, so every stage surface below is production code.

use std::time::{Duration, Instant};

use crate::mesh_worker::{MeshWorker, SectionId, Submit, Take};
use crate::quad_decode::{ExpandedVertex, QUAD_BYTES, SectionGeometry, SurfaceGeometry};
use crate::terrain_resources::{SectionCoord, TerrainRenderer};

/// The login-declared view distance of the pilot baseline, in chunks.
/// The baseline manifest records `view_distance: 32`, perfcheck
/// validates it against the login protocol's 2..64 domain. The pinned
/// Go core currently declares the login MINIMUM (2) with view-distance
/// configuration deferred, so this constant is the baseline identity,
/// not today's session value — see the radius note below. Only the
/// radius below and the pinning tests read it.
#[cfg(test)]
pub(crate) const VIEW_DISTANCE_CHUNKS: i64 = 32;

/// The view radius reclamation and the drain filter use, in section
/// units: baseline view distance plus one, mirroring the server's
/// subscription-radius derivation (declared+1). The exactness claim is
/// conditional: the keep-band matches the stream radius only while the
/// session declares exactly 32 — today's minimum-declaring pilot makes
/// the band strictly wider (reclamation idles; safe direction), while a
/// future declaration above 32 would stream past the band and churn,
/// so the bridge integration must derive or assert the declared value
/// before reclamation is load-bearing. Chunk and section X/Z are the
/// same coordinate, so no unit conversion applies.
pub(crate) const VIEW_RADIUS_SECTIONS: i64 = 32 + 1;

/// Maximum whole-section upserts one frame may submit. See the module
/// documentation for the derivation against the replaced client's
/// steady-state mesh budget.
pub(crate) const UPLOAD_BUDGET_SECTIONS: usize = 64;

/// Maximum whole-section drops one frame may apply through reclamation.
/// See the module documentation for why the drop budget equals the
/// upload budget in section count while costing far less per unit.
pub(crate) const DROP_BUDGET_SECTIONS: usize = 64;

/// Table-owned RIDs one live section costs: one mesh plus one instance.
/// Surfaces live inside the mesh, and the materials and scenario are
/// borrowed construction inputs, so this is the complete per-section RID
/// cost of the terrain path.
pub(crate) const RIDS_PER_SECTION: usize = 2;

/// The first session epoch. Zero stays invalid for the same reason the
/// RID table rejects zero revisions: an uninitialized counter must never
/// alias a legitimate value.
pub(crate) const FIRST_EPOCH: u64 = 1;

/// The camera's position in section coordinates: the dimension plus the
/// horizontal section X/Z the camera column centers on. Y is absent by
/// design — the authoritative streaming truth subscribes whole chunk
/// columns, so the view band has no vertical extent to consult.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct CameraSection {
    pub(crate) dimension: u32,
    pub(crate) x: i32,
    pub(crate) z: i32,
}

/// What one `frame` call did, in the same section units the budgets use.
/// Plain data, no Godot types: the bridge forwards it later, tests
/// assert on it now.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct FrameReport {
    /// Sections freed by out-of-view reclamation this frame.
    pub(crate) reclaimed_sections: usize,
    /// Reclamation found more candidates than the drop budget allowed;
    /// the next frame continues the sweep in coordinate order.
    pub(crate) reclamation_pending: bool,
    /// Prepared results that left the worker's completed queue this
    /// frame (submitted, discarded, or failed alike).
    pub(crate) drained_results: usize,
    /// Whole-section upserts the renderer accepted this frame.
    pub(crate) uploads_applied: usize,
    /// Upserts the renderer refused this frame; the failed result is
    /// dropped, the old section stays intact, and the stage advances no
    /// revision bookkeeping, so a producer resend of the same revision
    /// remains legal.
    pub(crate) uploads_failed: usize,
    /// Drained results discarded without a renderer call: retired epoch,
    /// out of view, or decode failure.
    pub(crate) uploads_discarded: usize,
}

/// The pilot-report accounting snapshot: cumulative byte and timing
/// totals, live counts, and session-lifetime peaks. Cumulative counters
/// and peaks survive [`TerrainBudgetStage::reset`]; live counts track
/// the current table state. No Godot types — typed consumption is the
/// bridge task's concern.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct TerrainBudgetFacts {
    /// Session epoch currently submitted under; strictly monotonic
    /// across resets. No `Default` derive: epoch zero is invalid
    /// (`FIRST_EPOCH` is one).
    pub(crate) epoch: u64,
    /// Frames driven.
    pub(crate) frames: u64,
    /// Session resets performed.
    pub(crate) resets: u64,
    /// Packed payload bytes accepted for prepare (submit-side,
    /// `QUAD_BYTES` per quad; counted on accept, not on reject, because
    /// a rejected payload returns to the caller uncounted and unspent).
    pub(crate) packed_input_bytes: u64,
    /// Expanded geometry bytes drained from the worker: vertex bytes
    /// plus index bytes of every successfully decoded result, counted
    /// at drain wherever the drain happens (frame or reset), including
    /// results later discarded by the view or epoch filter — the worker
    /// really expanded them, and the report must not hide that cost.
    /// Results the worker itself discards at publication (retired or
    /// superseded epochs) are never drained and stay uncounted.
    pub(crate) expanded_output_bytes: u64,
    /// Worker-side decode wall time summed over drained results;
    /// recording only, never consulted by any decision.
    pub(crate) prepare_duration: Duration,
    /// Wall time of the whole in-`frame` render phase — reclamation
    /// (including the table-range scan and candidate collection) plus
    /// section upserts; recording only, never consulted by any decision.
    pub(crate) upload_duration: Duration,
    /// Whole-section upserts the renderer accepted.
    pub(crate) uploads_applied: u64,
    /// Upserts the renderer refused.
    pub(crate) uploads_failed: u64,
    /// Drained results discarded without a renderer call.
    pub(crate) uploads_discarded: u64,
    /// Sections freed by out-of-view reclamation.
    pub(crate) reclaimed_sections: u64,
    /// Producer-driven drops applied through the stage.
    pub(crate) producer_drops_applied: u64,
    /// Live whole sections in the RID table right now.
    pub(crate) live_sections: usize,
    /// Live mesh surfaces across all held sections.
    pub(crate) live_surfaces: usize,
    /// Table-owned RIDs right now: `RIDS_PER_SECTION` per live section.
    pub(crate) live_rids: usize,
    /// Session-lifetime peaks; survive reset.
    pub(crate) peak_sections: usize,
    pub(crate) peak_surfaces: usize,
    pub(crate) peak_rids: usize,
}

/// The per-frame terrain pipeline stage: mesh-prepare worker plus
/// whole-section RID table under fixed budgets, one session epoch, and
/// the pilot report's accounting. See the module documentation for the
/// ordering, view, reset, and metric contracts.
pub(crate) struct TerrainBudgetStage {
    worker: MeshWorker,
    renderer: TerrainRenderer,
    /// The session epoch every submit currently carries. Strictly
    /// monotonic across resets; never zero; never reused after the
    /// reset that cancelled it.
    epoch: u64,
    // Cumulative counters; see `TerrainBudgetFacts` for meaning. Peaks
    // and totals are session-lifetime and survive reset by design.
    frames: u64,
    resets: u64,
    packed_input_bytes: u64,
    expanded_output_bytes: u64,
    prepare_duration: Duration,
    upload_duration: Duration,
    uploads_applied: u64,
    uploads_failed: u64,
    uploads_discarded: u64,
    reclaimed_sections: u64,
    producer_drops_applied: u64,
    peak_sections: usize,
    peak_surfaces: usize,
    peak_rids: usize,
}

impl TerrainBudgetStage {
    /// Assemble the stage over one renderer (whose backend may be the
    /// production `RenderingServer` backend or a test script) and a
    /// freshly started production decode worker. The only failure is
    /// worker-thread spawn failure, surfaced as the worker's stable
    /// internal word.
    pub(crate) fn new(renderer: TerrainRenderer) -> Result<Self, u32> {
        let worker = MeshWorker::new()?;
        Ok(Self::from_parts(worker, renderer))
    }

    /// Test seam: assemble the stage over an injected worker (for
    /// example a gated or slowed decode worker) and renderer, so
    /// determinism-sensitive tests control in-flight timing without
    /// sleeps. Production always goes through [`TerrainBudgetStage::new`].
    #[cfg(test)]
    pub(crate) fn with_worker(worker: MeshWorker, renderer: TerrainRenderer) -> Self {
        Self::from_parts(worker, renderer)
    }

    fn from_parts(worker: MeshWorker, renderer: TerrainRenderer) -> Self {
        Self {
            worker,
            renderer,
            epoch: FIRST_EPOCH,
            frames: 0,
            resets: 0,
            packed_input_bytes: 0,
            expanded_output_bytes: 0,
            prepare_duration: Duration::ZERO,
            upload_duration: Duration::ZERO,
            uploads_applied: 0,
            uploads_failed: 0,
            uploads_discarded: 0,
            reclaimed_sections: 0,
            producer_drops_applied: 0,
            peak_sections: 0,
            peak_surfaces: 0,
            peak_rids: 0,
        }
    }

    /// Submit one section upsert for preparation under the current
    /// session epoch. A pass-through of the worker's try-style contract:
    /// `Full` hands the payload back for a later retry (retry policy is
    /// the caller's), and packed bytes are counted only on `Queued`,
    /// because a rejected payload returns unspent. No view filter runs
    /// here — the producer streams what the server streams; staleness is
    /// filtered at drain.
    pub(crate) fn submit(&mut self, id: SectionId, packed: Vec<u64>) -> Submit {
        let packed_bytes = (packed.len() * QUAD_BYTES) as u64;
        match self.worker.try_submit(self.epoch, id, packed) {
            Submit::Queued => {
                self.packed_input_bytes += packed_bytes;
                Submit::Queued
            }
            rejected => rejected,
        }
    }

    /// Apply one producer-driven section drop through the RID table's
    /// revision-disciplined drop. The world-batch drop path of the
    /// presentation contract lands here; reclamation is a separate,
    /// owner-side mechanism that needs no revision.
    pub(crate) fn drop_section(&mut self, id: SectionId) -> Result<(), u32> {
        let applied = self.renderer.drop_section(id);
        if applied.is_ok() {
            self.producer_drops_applied += 1;
        }
        applied
    }

    /// Drive one frame: reclaim out-of-view sections under the drop
    /// budget, then drain prepared results under the upload budget.
    /// Both budgets are fixed; leftover work stays queued (FIFO for
    /// uploads, coordinate order for reclamation) and the next frame
    /// continues. Never blocks on the worker, never allocates
    /// proportionally to held sections (reclamation collects at most one
    /// candidate beyond the drop budget).
    pub(crate) fn frame(&mut self, camera: CameraSection) -> FrameReport {
        self.frames += 1;
        let mut report = FrameReport::default();

        // Phase one, reclamation, strictly before uploads: removals are
        // never starved by upload pressure and freed capacity precedes
        // new use. The candidate collection is bounded by the budget
        // plus one — one lookahead entry to report whether more
        // candidates remain — never by the held-section count.
        let reclamation_started = Instant::now();
        let candidates: Vec<SectionCoord> = self
            .renderer
            .sections_in_dimension_iter(camera.dimension)
            .filter(|coord| !section_in_view(camera, *coord))
            .take(DROP_BUDGET_SECTIONS + 1)
            .collect();
        let mut drop_budget = DROP_BUDGET_SECTIONS;
        for coord in candidates {
            if drop_budget == 0 {
                report.reclamation_pending = true;
                break;
            }
            if self.renderer.reclaim_section(coord) {
                drop_budget -= 1;
                report.reclaimed_sections += 1;
                self.reclaimed_sections += 1;
            }
        }
        self.upload_duration += reclamation_started.elapsed();

        // Phase two, the bounded upload drain. Drained is not equal to
        // submitted: the epoch, view, and decode filters below discard
        // stale results without a renderer call, and every discard still
        // spends drain budget — taking stale work off the queue is real
        // work and keeps queue retention transient.
        let upload_started = Instant::now();
        let mut drain_budget = UPLOAD_BUDGET_SECTIONS;
        while drain_budget > 0 {
            let prepared = match self.worker.try_take() {
                Take::Output(prepared) => prepared,
                // Empty: nothing ready this frame; Closed: the worker is
                // torn down and no result will ever follow. Either way
                // the drain stops without waiting.
                Take::Empty | Take::Closed => break,
            };
            drain_budget -= 1;
            report.drained_results += 1;
            let crate::mesh_worker::PreparedSection {
                epoch,
                id,
                input_quads: _,
                prepare_duration,
                outcome,
            } = prepared;
            self.prepare_duration += prepare_duration;
            if let Ok(geometry) = &outcome {
                self.expanded_output_bytes += expanded_section_bytes(geometry) as u64;
            }
            match outcome {
                Ok(geometry) => {
                    if epoch != self.epoch || !section_in_view(camera, SectionCoord::of(id)) {
                        self.uploads_discarded += 1;
                        report.uploads_discarded += 1;
                        continue;
                    }
                    match self.renderer.upsert_section(id, &geometry) {
                        Ok(()) => {
                            self.uploads_applied += 1;
                            report.uploads_applied += 1;
                        }
                        Err(_) => {
                            // The renderer preserved the old section and
                            // recorded no revision for the failed
                            // attempt; the stage advances no bookkeeping
                            // either, so a producer resend of this exact
                            // revision stays legal. The expanded payload
                            // is dropped here — retaining it for retry
                            // would rebuild the unbounded expanded-memory
                            // retention this budget exists to prevent.
                            self.uploads_failed += 1;
                            report.uploads_failed += 1;
                        }
                    }
                }
                // A decode failure produced no geometry; the producer's
                // own batch validation is the authority on retrying it.
                Err(_) => {
                    self.uploads_discarded += 1;
                    report.uploads_discarded += 1;
                }
            }
        }
        self.upload_duration += upload_started.elapsed();

        // Live counts and peaks once, at frame end: producer drops and
        // both frame phases have all settled by this point.
        let renderer_facts = self.renderer.facts();
        self.peak_sections = self.peak_sections.max(renderer_facts.sections);
        self.peak_surfaces = self.peak_surfaces.max(renderer_facts.surfaces);
        self.peak_rids = self
            .peak_rids
            .max(renderer_facts.sections * RIDS_PER_SECTION);
        report
    }

    /// Session reset: cancel every queued worker job of the current
    /// epoch, drain and discard every already-published result (the
    /// worker's epoch filter runs at publication, so already-published
    /// results need this explicit drain), free every held section, and
    /// move to a strictly greater epoch. A closed worker is not an error
    /// here: `close` already discarded its queues, the drain observes
    /// `Closed` immediately, and the renderer reset still runs.
    pub(crate) fn reset(&mut self) {
        let retired = self.epoch;
        // Strictly monotonic epochs, never reused after the cancel
        // below: exhaustion of the u64 epoch space is unreachable by
        // construction (a reset every nanosecond would need centuries),
        // so the overflow panic mirrors the Go runtime's terminal
        // treatment of its epoch overflow rather than silently reusing
        // a cancelled epoch.
        self.epoch = self
            .epoch
            .checked_add(1)
            .expect("terrain epoch space exhausted");
        // Retire the old epoch: queued jobs are discarded now and any
        // in-flight job's result is discarded at publication. This
        // ignores the closed-worker error on purpose (see above).
        let _ = self.worker.cancel_pending(retired);
        while let Take::Output(prepared) = self.worker.try_take() {
            self.prepare_duration += prepared.prepare_duration;
            if let Ok(geometry) = &prepared.outcome {
                self.expanded_output_bytes += expanded_section_bytes(geometry) as u64;
            }
            self.uploads_discarded += 1;
        }
        self.renderer.reset();
        self.resets += 1;
    }

    /// The live section inventory in coordinate order, forwarded to the
    /// bridge's structural summary surface.
    pub(crate) fn section_summaries(&self) -> Vec<crate::terrain_resources::SectionSummary> {
        self.renderer.section_summaries()
    }

    /// The pilot-report accounting snapshot; see [`TerrainBudgetFacts`].
    pub(crate) fn facts(&self) -> TerrainBudgetFacts {
        let renderer_facts = self.renderer.facts();
        TerrainBudgetFacts {
            epoch: self.epoch,
            frames: self.frames,
            resets: self.resets,
            packed_input_bytes: self.packed_input_bytes,
            expanded_output_bytes: self.expanded_output_bytes,
            prepare_duration: self.prepare_duration,
            upload_duration: self.upload_duration,
            uploads_applied: self.uploads_applied,
            uploads_failed: self.uploads_failed,
            uploads_discarded: self.uploads_discarded,
            reclaimed_sections: self.reclaimed_sections,
            producer_drops_applied: self.producer_drops_applied,
            live_sections: renderer_facts.sections,
            live_surfaces: renderer_facts.surfaces,
            live_rids: renderer_facts.sections * RIDS_PER_SECTION,
            peak_sections: self.peak_sections,
            peak_surfaces: self.peak_surfaces,
            peak_rids: self.peak_rids,
        }
    }

    /// Test-only observable of the worker's queue state, so tests poll
    /// for structural facts (a fully drained or exactly-full queue)
    /// instead of timing.
    #[cfg(test)]
    pub(crate) fn worker_facts(&self) -> crate::mesh_worker::WorkerFacts {
        self.worker.facts()
    }
}

/// Horizontal Chebyshev distance between the camera column and one
/// section, in section units, computed in i64 so extreme section
/// coordinates cannot overflow the per-axis delta. The metric matches
/// the authoritative streaming comparison (per-axis absolute delta,
/// horizontal axes only) and the old client's LOD scheduling truth.
fn chebyshev_sections(camera: CameraSection, coord: SectionCoord) -> i64 {
    let dx = (i64::from(coord.x) - i64::from(camera.x)).abs();
    let dz = (i64::from(coord.z) - i64::from(camera.z)).abs();
    dx.max(dz)
}

/// Whether one section is inside the camera's view band: same dimension
/// and horizontal Chebyshev distance at most the view radius. The
/// boundary is inclusive — the authoritative streaming comparison keeps
/// a chunk exactly at the radius. Y never participates.
fn section_in_view(camera: CameraSection, coord: SectionCoord) -> bool {
    coord.dimension == camera.dimension && chebyshev_sections(camera, coord) <= VIEW_RADIUS_SECTIONS
}

/// Expanded byte cost of one prepared section: vertex bytes plus index
/// bytes across the three surface classes. Error results carry no
/// geometry and contribute nothing.
fn expanded_section_bytes(geometry: &SectionGeometry) -> usize {
    fn surface_bytes(surface: &SurfaceGeometry) -> usize {
        surface.vertices.len() * std::mem::size_of::<ExpandedVertex>()
            + surface.indices.len() * std::mem::size_of::<u32>()
    }
    surface_bytes(&geometry.opaque)
        + surface_bytes(&geometry.cutout)
        + surface_bytes(&geometry.water)
}

#[cfg(test)]
mod tests {
    use super::{
        CameraSection, DROP_BUDGET_SECTIONS, FIRST_EPOCH, FrameReport, RIDS_PER_SECTION,
        TerrainBudgetStage, UPLOAD_BUDGET_SECTIONS, VIEW_DISTANCE_CHUNKS, VIEW_RADIUS_SECTIONS,
    };
    use crate::abi;
    use crate::mesh_worker::{MeshWorker, SectionId, Submit};
    use crate::quad_decode::{
        self, CUTOUT_MATERIAL_LEAVES, PLANT_MATERIAL_SHORT_GRASS, TestQuad, WATER_MATERIAL,
    };
    use crate::terrain_resources::SECTION_SURFACE_CLASSES;
    use crate::terrain_resources::render_script::{
        SCRIPT_CUTOUT, SCRIPT_OPAQUE, SCRIPT_SCENARIO, SCRIPT_WATER, ScriptEvent, ScriptedBackend,
    };
    use godot::prelude::*;
    use std::thread;
    use std::time::{Duration, Instant};

    /// Generous but finite bound for every poll in this suite; a correct
    /// stage settles in milliseconds, so hitting a deadline is a bug
    /// report, not a race.
    const TEST_TIMEOUT: Duration = Duration::from_secs(20);
    const POLL_STEP: Duration = Duration::from_millis(1);

    fn stone_quad() -> u64 {
        TestQuad::default().pack()
    }

    fn leaves_quad() -> u64 {
        TestQuad {
            material: CUTOUT_MATERIAL_LEAVES,
            w: 2,
            h: 1,
            ..TestQuad::default()
        }
        .pack()
    }

    fn water_quad() -> u64 {
        TestQuad {
            material: WATER_MATERIAL,
            corners: [8, 9, 10, 11],
            ..TestQuad::default()
        }
        .pack()
    }

    fn grass_quad() -> u64 {
        TestQuad {
            face: 6,
            material: PLANT_MATERIAL_SHORT_GRASS,
            ..TestQuad::default()
        }
        .pack()
    }

    /// One overworld section identity at the given coordinates.
    fn section_at(x: i32, y: i32, z: i32, revision: u64) -> SectionId {
        SectionId {
            dimension: 0,
            x,
            y,
            z,
            revision,
        }
    }

    fn camera_at(x: i32, z: i32) -> CameraSection {
        CameraSection { dimension: 0, x, z }
    }

    fn staged() -> (TerrainBudgetStage, ScriptedBackend) {
        let (renderer, script) = ScriptedBackend::scripted_renderer();
        let stage = TerrainBudgetStage::new(renderer).expect("stage starts");
        (stage, script)
    }

    /// Poll a structural predicate until it holds, with a finite
    /// deadline and a named failure message — never a sleep-based race.
    fn poll_until(probe: impl Fn() -> bool, what: &str) {
        let deadline = Instant::now() + TEST_TIMEOUT;
        while !probe() {
            assert!(
                Instant::now() < deadline,
                "terrain_budget timed out waiting for {what}"
            );
            thread::sleep(POLL_STEP);
        }
    }

    /// Submit `packed` sections and wait until the worker published
    /// every result, so the next `frame` observes a deterministically
    /// full completed queue.
    fn submit_and_prepare(stage: &mut TerrainBudgetStage, sections: Vec<(SectionId, Vec<u64>)>) {
        let expected = sections.len();
        for (id, packed) in sections {
            assert!(
                matches!(stage.submit(id, packed), Submit::Queued),
                "section {} must queue",
                id.x
            );
        }
        poll_until(
            || stage.worker_facts().completed_jobs == expected,
            "the worker to publish every submitted section",
        );
    }

    /// Drive frames until no prepared result remains queued, and return
    /// the per-frame reports. Each frame drains at most the upload
    /// budget, so this converges in bounded frames for bounded test
    /// batches.
    fn drain_frames(stage: &mut TerrainBudgetStage, camera: CameraSection) -> Vec<FrameReport> {
        let mut reports = Vec::new();
        loop {
            let report = stage.frame(camera);
            let finished = report.drained_results < UPLOAD_BUDGET_SECTIONS;
            reports.push(report);
            if finished {
                return reports;
            }
        }
    }

    #[test]
    fn terrain_budget_pins_budgets_view_radius_and_rid_constants() {
        // Upload budget equals the replaced client's steady-state
        // per-frame mesh budget (Go `SteadyFrameMeshWorkMax` = 64).
        assert_eq!(UPLOAD_BUDGET_SECTIONS, 64);
        // The drop budget equals the upload budget in section count
        // while each drop costs only two RID frees.
        assert_eq!(DROP_BUDGET_SECTIONS, UPLOAD_BUDGET_SECTIONS);
        // The manifest's login-declared view distance (chunks) plus one
        // is the authoritative subscription radius; chunk X/Z equals
        // section X/Z so the radius is the same number in sections.
        assert_eq!(VIEW_DISTANCE_CHUNKS, 32);
        assert_eq!(VIEW_RADIUS_SECTIONS, 33);
        // Exactly two table-owned RIDs per section: mesh plus instance.
        assert_eq!(RIDS_PER_SECTION, 2);
        assert_eq!(FIRST_EPOCH, 1);
    }

    #[test]
    fn terrain_budget_frame_applies_at_most_the_upload_budget_and_continues() {
        let (mut stage, _script) = staged();
        let camera = camera_at(0, 0);
        // More ready sections than the budget, all inside the view band:
        // a ten-by-ten cluster, every section within Chebyshev distance
        // ten of the camera.
        let sections: Vec<(SectionId, Vec<u64>)> = (0..100)
            .map(|index| {
                (
                    section_at(index % 10, -1, index / 10, 1),
                    vec![stone_quad()],
                )
            })
            .collect();
        submit_and_prepare(&mut stage, sections);

        // Frame one submits exactly the budget; the remainder stays
        // queued in FIFO order for the next frame.
        let first = stage.frame(camera);
        assert_eq!(first.uploads_applied, UPLOAD_BUDGET_SECTIONS);
        assert_eq!(first.drained_results, UPLOAD_BUDGET_SECTIONS);
        assert_eq!(first.uploads_failed, 0);
        assert_eq!(first.uploads_discarded, 0);
        assert_eq!(
            stage.worker_facts().completed_jobs,
            100 - UPLOAD_BUDGET_SECTIONS
        );
        let facts = stage.facts();
        assert_eq!(facts.live_sections, UPLOAD_BUDGET_SECTIONS);
        assert_eq!(facts.live_rids, UPLOAD_BUDGET_SECTIONS * RIDS_PER_SECTION);
        assert_eq!(facts.peak_sections, UPLOAD_BUDGET_SECTIONS);

        // Frame two continues and finishes the backlog; frame three
        // finds an empty queue.
        let second = stage.frame(camera);
        assert_eq!(second.uploads_applied, 100 - UPLOAD_BUDGET_SECTIONS);
        assert_eq!(stage.worker_facts().completed_jobs, 0);
        let third = stage.frame(camera);
        assert_eq!(third.drained_results, 0);
        assert_eq!(third.uploads_applied, 0);
        let facts = stage.facts();
        assert_eq!(facts.live_sections, 100);
        assert_eq!(facts.uploads_applied, 100);
        assert_eq!(facts.peak_sections, 100);
    }

    #[test]
    fn terrain_budget_applies_drops_before_uploads_within_a_frame() {
        let (mut stage, script) = staged();
        // Three sections live around camera A.
        let camera_a = camera_at(0, 0);
        let old: Vec<(SectionId, Vec<u64>)> = (0..3)
            .map(|index| (section_at(index, -1, 0, 1), vec![stone_quad()]))
            .collect();
        submit_and_prepare(&mut stage, old);
        drain_frames(&mut stage, camera_a);
        assert_eq!(stage.facts().live_sections, 3);

        // Move far enough that every held section is out of view while
        // three fresh sections are ready: one frame must both reclaim
        // and upload, and reclamation must run first.
        let camera_b = camera_at(VIEW_RADIUS_SECTIONS as i32 + 10, 0);
        let fresh: Vec<(SectionId, Vec<u64>)> = (0..3)
            .map(|index| (section_at(camera_b.x + index, -1, 0, 1), vec![stone_quad()]))
            .collect();
        submit_and_prepare(&mut stage, fresh);
        let events_before = script.events().len();
        let report = stage.frame(camera_b);

        assert_eq!(
            report.reclaimed_sections, 3,
            "the old sections leave the view band"
        );
        assert_eq!(
            report.uploads_applied, 3,
            "the fresh sections upload in the same frame"
        );
        assert_eq!(stage.facts().live_sections, 3);

        // The free events of the reclaimed sections strictly precede the
        // first new mesh allocation of the same frame: drop-priority-first.
        let tail = &script.events()[events_before..];
        let first_free = tail
            .iter()
            .position(|event| matches!(event, ScriptEvent::Freed { .. }))
            .expect("the frame frees the reclaimed sections");
        let last_free = tail
            .iter()
            .rposition(|event| matches!(event, ScriptEvent::Freed { .. }))
            .expect("the frame frees the reclaimed sections");
        let first_new_mesh = tail
            .iter()
            .position(|event| matches!(event, ScriptEvent::MeshCreated { .. }))
            .expect("the frame uploads the fresh sections");
        assert!(
            last_free < first_new_mesh,
            "every drop precedes every upload (first free at {first_free}, last free at {last_free}, first mesh at {first_new_mesh})"
        );
        assert_eq!(script.free_history().len(), 3 * RIDS_PER_SECTION);
    }

    #[test]
    fn terrain_budget_reclaims_out_of_view_sections_under_the_drop_budget() {
        let (mut stage, script) = staged();
        // Camera A sees the whole batch: 120 bulk sections at distance
        // exactly on the boundary plus the two sections camera B keeps.
        let camera_a = camera_at(60, 0);
        let camera_b = camera_at(0, 0);
        let mut sections: Vec<(SectionId, Vec<u64>)> = Vec::new();
        // Bulk: x in 34..=93, z in 0..=1 — chebyshev at most 33 from
        // camera A (uploadable) and at least 34 from camera B (out of
        // view after the move). 120 > DROP_BUDGET, so reclamation needs
        // more than one frame.
        for x in 34..=93i32 {
            for z in 0..=1i32 {
                sections.push((section_at(x, -1, z, 1), vec![stone_quad()]));
            }
        }
        // Kept under camera B: one section exactly at the view radius
        // (the inclusive boundary) and one differing only in Y, which
        // the horizontal metric must ignore.
        sections.push((section_at(33, -1, 0, 1), vec![stone_quad()]));
        sections.push((section_at(30, 500, 0, 1), vec![stone_quad()]));
        submit_and_prepare(&mut stage, sections);
        drain_frames(&mut stage, camera_a);
        assert_eq!(stage.facts().live_sections, 122);

        // Frame one under camera B reclaims exactly the drop budget and
        // reports the pending remainder; frame two finishes the sweep.
        let first = stage.frame(camera_b);
        assert_eq!(first.reclaimed_sections, DROP_BUDGET_SECTIONS);
        assert!(first.reclamation_pending);
        assert_eq!(first.uploads_applied, 0);
        assert_eq!(stage.facts().live_sections, 122 - DROP_BUDGET_SECTIONS);

        let second = stage.frame(camera_b);
        assert_eq!(second.reclaimed_sections, 120 - DROP_BUDGET_SECTIONS);
        assert!(!second.reclamation_pending);
        let facts = stage.facts();
        // Exactly the boundary section and the far-Y section remain: the
        // inclusive radius keeps distance 33, and Y never participates.
        assert_eq!(facts.live_sections, 2);
        assert_eq!(facts.live_rids, 2 * RIDS_PER_SECTION);
        assert_eq!(facts.reclaimed_sections, 120);
        assert_eq!(facts.peak_sections, 122);
        assert_eq!(script.free_history().len(), 120 * RIDS_PER_SECTION);
        // Uploads follow the worker's FIFO, so the two survivors were
        // allocated last: exactly the boundary section's and the far-Y
        // section's mesh-plus-instance RIDs stay live.
        assert_eq!(script.live_ids(), vec![241, 242, 243, 244]);
    }

    #[test]
    fn terrain_budget_discards_drained_results_outside_the_view_or_undecodable() {
        let (mut stage, script) = staged();
        let camera = camera_at(0, 0);
        // Two sections inside the band, one beyond the radius, and one
        // whose payload decodes to an error word.
        let mut sections: Vec<(SectionId, Vec<u64>)> = vec![
            (section_at(0, -1, 0, 1), vec![stone_quad()]),
            (section_at(5, -1, 5, 1), vec![water_quad()]),
            (
                section_at(VIEW_RADIUS_SECTIONS as i32 + 1, -1, 0, 1),
                vec![stone_quad()],
            ),
        ];
        let invalid = vec![stone_quad(), stone_quad() | 1 << 63];
        sections.push((section_at(1, -1, 0, 1), invalid));
        submit_and_prepare(&mut stage, sections);

        let report = stage.frame(camera);
        assert_eq!(report.uploads_applied, 2);
        assert_eq!(report.uploads_discarded, 2);
        assert_eq!(report.uploads_failed, 0);
        // Only the two in-view sections reached the renderer.
        let mesh_creations = script
            .events()
            .iter()
            .filter(|event| matches!(event, ScriptEvent::MeshCreated { .. }))
            .count();
        assert_eq!(mesh_creations, 2);
        let facts = stage.facts();
        assert_eq!(facts.live_sections, 2);
        assert_eq!(facts.uploads_discarded, 2);
        // The error result contributed no expanded bytes; the two decoded
        // ones did (pinned by the metrics test).
        assert!(facts.expanded_output_bytes > 0);
    }

    #[test]
    fn terrain_budget_renderer_failure_preserves_state_and_allows_same_revision_retry() {
        let (mut stage, script) = staged();
        let camera = camera_at(0, 0);
        let id = section_at(0, -1, 0, 1);
        submit_and_prepare(&mut stage, vec![(id, vec![stone_quad(), leaves_quad()])]);
        drain_frames(&mut stage, camera);
        assert_eq!(stage.facts().live_sections, 1);

        // The renderer refuses the replacement: the old section stays
        // intact, the failure is counted, and no revision bookkeeping
        // advances — proven by the same-revision retry below.
        script.fail_mesh_create();
        let replacement = section_at(0, -1, 0, 2);
        submit_and_prepare(&mut stage, vec![(replacement, vec![water_quad()])]);
        let report = stage.frame(camera);
        assert_eq!(report.uploads_failed, 1);
        assert_eq!(report.uploads_applied, 0);
        let facts = stage.facts();
        assert_eq!(facts.live_sections, 1);
        assert_eq!(facts.uploads_failed, 1);
        assert_eq!(facts.peak_sections, 1);

        // After backend recovery the producer resubmits the exact same
        // revision and the upsert succeeds — legal only because neither
        // the renderer nor the stage recorded the failed attempt.
        script.recover();
        submit_and_prepare(&mut stage, vec![(replacement, vec![water_quad()])]);
        let report = stage.frame(camera);
        assert_eq!(report.uploads_applied, 1);
        assert_eq!(stage.facts().live_sections, 1);
        // The replaced old section was freed exactly once.
        assert_eq!(script.free_history().len(), RIDS_PER_SECTION);
    }

    #[test]
    fn terrain_budget_reset_cancels_drains_clears_and_restarts_on_a_fresh_epoch() {
        let (mut stage, script) = staged();
        let camera = camera_at(0, 0);
        // Two sections fully prepared and live, plus one more batch
        // already submitted behind them.
        let first_batch: Vec<(SectionId, Vec<u64>)> = (0..2)
            .map(|index| (section_at(index, -1, 0, 1), vec![stone_quad()]))
            .collect();
        submit_and_prepare(&mut stage, first_batch);
        drain_frames(&mut stage, camera);
        assert_eq!(stage.facts().live_sections, 2);
        let peaks_before = {
            let facts = stage.facts();
            (facts.peak_sections, facts.peak_rids)
        };
        let second_batch: Vec<(SectionId, Vec<u64>)> = (2..5)
            .map(|index| (section_at(index, -1, 0, 1), vec![stone_quad()]))
            .collect();
        let second_batch_len = second_batch.len() as u64;
        submit_and_prepare(&mut stage, second_batch);
        let discards_before = stage.facts().uploads_discarded;
        let epoch_before = stage.facts().epoch;

        stage.reset();

        // The completed results of the retired epoch were drained and
        // discarded, the queued jobs were cancelled, and every held
        // section was freed.
        poll_until(
            || {
                let facts = stage.worker_facts();
                facts.completed_jobs == 0 && facts.pending_jobs == 0
            },
            "the reset worker to settle with empty queues",
        );
        let facts = stage.facts();
        assert_eq!(facts.live_sections, 0);
        assert_eq!(facts.live_rids, 0);
        assert_eq!(facts.uploads_discarded, discards_before + second_batch_len);
        assert_eq!(facts.resets, 1);
        assert_eq!(
            facts.epoch,
            epoch_before + 1,
            "epochs are strictly monotonic"
        );
        assert!(script.live_ids().is_empty());
        // Peaks survive reset by design: a high-water mark the reset
        // cleared would hide exactly the spike reset exists for.
        assert_eq!((facts.peak_sections, facts.peak_rids), peaks_before);

        // Subsequent frames work on the fresh epoch: a section the old
        // epoch already used re-uploads at the same revision because the
        // reset table holds nothing, and the frame reports it applied.
        let fresh: Vec<(SectionId, Vec<u64>)> = vec![
            (section_at(0, -1, 0, 1), vec![stone_quad()]),
            (section_at(1, -1, 0, 1), vec![leaves_quad()]),
        ];
        submit_and_prepare(&mut stage, fresh);
        let report = stage.frame(camera);
        assert_eq!(report.uploads_applied, 2);
        assert_eq!(report.uploads_discarded, 0);
        let facts = stage.facts();
        assert_eq!(facts.live_sections, 2);
        assert_eq!(facts.epoch, epoch_before + 1);
    }

    #[test]
    fn terrain_budget_metrics_record_bytes_durations_and_peaks_honestly() {
        // A slowed decode worker makes prepare duration decidable: every
        // decode provably takes at least the sleep, so the recorded sum
        // must reach the number of sections times the sleep.
        const DECODE_FLOOR: Duration = Duration::from_millis(5);
        let (renderer, script) = ScriptedBackend::scripted_renderer();
        let worker = MeshWorker::with_decode(Box::new(move |packed: &[u64]| {
            thread::sleep(DECODE_FLOOR);
            quad_decode::decode_pilot_section(packed)
        }))
        .expect("slowed worker starts");
        let mut stage = TerrainBudgetStage::with_worker(worker, renderer);
        let camera = camera_at(0, 0);

        // One pure-opaque section and one cutout+water section.
        submit_and_prepare(
            &mut stage,
            vec![
                (section_at(0, -1, 0, 1), vec![stone_quad()]),
                (section_at(1, -1, 0, 1), vec![leaves_quad(), water_quad()]),
            ],
        );
        let report = stage.frame(camera);
        assert_eq!(report.uploads_applied, 2);

        let facts = stage.facts();
        // Packed input is counted at submit: three quads, eight bytes
        // each.
        assert_eq!(facts.packed_input_bytes, 3 * quad_decode::QUAD_BYTES as u64);
        // Expanded output is counted at drain: each quad expands to four
        // vertices and six indices in its surface class.
        let vertex_bytes = std::mem::size_of::<crate::quad_decode::ExpandedVertex>();
        let one_quad = 4 * vertex_bytes + 6 * std::mem::size_of::<u32>();
        assert_eq!(
            facts.expanded_output_bytes as usize,
            3 * one_quad,
            "three expanded quads across three surface classes"
        );
        // Prepare duration is the worker-side decode wall time: two
        // decodes, each at least the injected floor.
        assert!(facts.prepare_duration >= 2 * DECODE_FLOOR);
        // Upload duration is recorded (reclamation plus upserts), and
        // the recording is additive — it never decreases.
        assert!(facts.upload_duration >= Duration::ZERO);
        let recorded_upload = facts.upload_duration;
        // Peaks track the live maximum, and peak RIDs stay pinned at the
        // per-section RID cost.
        assert_eq!(facts.live_sections, 2);
        assert_eq!(facts.peak_sections, 2);
        assert_eq!(facts.peak_rids, 2 * RIDS_PER_SECTION);
        assert!(facts.peak_rids >= facts.live_rids);
        assert!(facts.peak_sections >= facts.live_sections);
        assert!(facts.peak_surfaces >= facts.live_surfaces);

        // A further frame adds no uploads and never lowers the recorded
        // totals.
        stage.frame(camera);
        let later = stage.facts();
        assert_eq!(later.uploads_applied, 2);
        assert!(later.upload_duration >= recorded_upload);
        assert_eq!(later.prepare_duration, facts.prepare_duration);
        assert_eq!(script.live_ids().len(), 2 * RIDS_PER_SECTION);
    }

    #[test]
    fn terrain_budget_terrain_path_is_section_granular_never_block_granular() {
        let (mut stage, script) = staged();
        let camera = camera_at(0, 0);
        // A one-quad section and the maximal section: 24576 quads is the
        // mesher's worst case of six quads per block across all 4096
        // blocks of one section — the full block budget.
        let maximal = abi::MAX_SECTION_MESH_QUADS as usize;
        let sections: Vec<(SectionId, Vec<u64>)> = vec![
            (section_at(0, -1, 0, 1), vec![stone_quad()]),
            (section_at(1, -1, 0, 1), vec![stone_quad(); maximal]),
        ];
        submit_and_prepare(&mut stage, sections);
        drain_frames(&mut stage, camera);

        // Both sections cost exactly the same table-owned RID count: one
        // mesh plus one instance each. A one-Node-per-block design would
        // hold at least 4096 nodes for the full section; the whole-
        // section path holds two RIDs regardless of block count, and the
        // per-section surface count stays at the class bound.
        let facts = stage.facts();
        assert_eq!(facts.live_sections, 2);
        assert_eq!(facts.live_rids, 2 * RIDS_PER_SECTION);
        assert_eq!(facts.peak_rids, 2 * RIDS_PER_SECTION);
        assert!(facts.live_surfaces <= 2 * SECTION_SURFACE_CLASSES);
        let allocated = script.allocated();
        assert_eq!(
            allocated.len(),
            2 * RIDS_PER_SECTION,
            "RID cost per section is constant, independent of quad or block count"
        );
        // The maximal section really carried its blocks: 24576 quads
        // expanded, four vertices each, in one surface.
        let events = script.events();
        let maximal_surfaces = events
            .iter()
            .filter(|event| match event {
                ScriptEvent::SurfaceAdded { vertices, .. } => *vertices == maximal * 4,
                _ => false,
            })
            .count();
        assert_eq!(
            maximal_surfaces, 1,
            "the maximal section submitted one surface"
        );
    }

    #[test]
    fn terrain_budget_smoke_pipeline_scripted_batch_visible_state() {
        let (mut stage, script) = staged();
        let camera = camera_at(0, 0);
        // A synthetic mixed batch: opaque, cutout, water, and plant
        // quads across three sections.
        let sections: Vec<(SectionId, Vec<u64>)> = vec![
            (section_at(0, -1, 0, 1), vec![stone_quad(), stone_quad()]),
            (section_at(1, -1, 0, 1), vec![leaves_quad(), water_quad()]),
            (section_at(0, -1, 1, 1), vec![grass_quad()]),
        ];
        submit_and_prepare(&mut stage, sections);
        let reports = drain_frames(&mut stage, camera);
        assert_eq!(
            reports
                .iter()
                .map(|report| report.uploads_applied)
                .sum::<usize>(),
            3
        );

        // Visible state: one mesh plus one instance per section, each
        // instance at its section world origin, every non-empty surface
        // class wired to its material RID.
        let facts = stage.facts();
        assert_eq!(facts.live_sections, 3);
        assert_eq!(facts.live_rids, 3 * RIDS_PER_SECTION);
        let events = script.events();
        let instances: Vec<&ScriptEvent> = events
            .iter()
            .filter(|event| matches!(event, ScriptEvent::InstanceCreated { .. }))
            .collect();
        assert_eq!(instances.len(), 3);
        let expected_origin = |x: i32, y: i32, z: i32| {
            Transform3D::new(
                Basis::IDENTITY,
                Vector3::new(
                    (x as f32) * 16.0,
                    (y as f32) * 16.0 - 64.0,
                    (z as f32) * 16.0,
                ),
            )
        };
        let expected_origins = [
            expected_origin(0, -1, 0),
            expected_origin(1, -1, 0),
            expected_origin(0, -1, 1),
        ];
        for event in instances {
            let ScriptEvent::InstanceCreated {
                scenario, origin, ..
            } = event
            else {
                panic!("filtered for instances, got {event:?}")
            };
            assert_eq!(*scenario, SCRIPT_SCENARIO);
            assert!(
                expected_origins.contains(origin),
                "instance placed at a section world origin, got {origin:?}"
            );
        }
        let materials: Vec<Rid> = events
            .iter()
            .filter_map(|event| match event {
                ScriptEvent::SurfaceAdded { material, .. } => Some(*material),
                _ => None,
            })
            .collect();
        assert!(materials.contains(&SCRIPT_OPAQUE));
        assert!(materials.contains(&SCRIPT_CUTOUT));
        assert!(materials.contains(&SCRIPT_WATER));

        // The producer drop path runs through the stage with revision
        // discipline: a stale drop is refused without any renderer call,
        // a newer drop frees the section exactly once.
        let events_before = script.events().len();
        assert_eq!(
            stage.drop_section(section_at(0, -1, 0, 1)),
            Err(abi::STATUS_INPUT_REJECTED),
            "a zero revision is refused"
        );
        assert_eq!(script.events().len(), events_before);
        assert_eq!(stage.drop_section(section_at(0, -1, 0, 2)), Ok(()));
        let facts = stage.facts();
        assert_eq!(facts.live_sections, 2);
        assert_eq!(facts.producer_drops_applied, 1);
        assert_eq!(script.free_history().len(), RIDS_PER_SECTION);

        // Teardown frees everything the stage still holds.
        drop(stage);
        assert!(script.live_ids().is_empty());
    }
}
