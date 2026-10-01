//! Bounded far-terrain tile selection.
//!
//! This owner selects which far tiles the client prepares, in which order,
//! and under which byte allowance; real generation stays with the shared
//! bounded `PreparationPort` and the accepted native facade. Every
//! dispatched job is a `TerrainKey::LodTile` pairing — a far tile is never
//! published under a fake section identity — and every drained result whose
//! epoch or seed/config generation no longer matches is released exactly
//! once instead of being retained.
//!
//! The ring geometry mirrors the measured source scheduler: one tile spans
//! a fixed 4x4 chunk grid, the center tile is the chunk column shifted
//! along that grid, and the band is the closed Chebyshev ring between the
//! near-mesh exclusion radius and the far coverage radius. Because the
//! shell's windows take the column maximum, a tile inside the near disk
//! would poke through the precise surface at a nearer depth, so the band's
//! lower edge starts strictly outside it.

use std::sync::Arc;

use mornlea_domain::{ChunkPos, Dimension};
use mornlea_engine::native::contracts::world::WorldgenParams;

use super::{
    LodConfig, LodStep, OwnedLodRequest, PreparationJob, PreparationPayload, PreparedResourceKey,
    TerrainKey, TilePos,
};
use crate::contracts::{ClientError, PreparationPort, SessionEpoch};

/// Chunks per far-tile axis: one tile spans a fixed 4x4 chunk grid, so its
/// center conversion shifts by two bits.
const TILE_CHUNKS: u32 = 4;

/// Columns per tile axis; the fixed native far-tile shape.
const TILE_COLUMNS: u32 = 64;

/// The fixed 20-byte width of one far quad: x/z/y i32 + w/d u16 + face u8 +
/// material u16 + shade u8, the same width the accepted native shell
/// stream bills.
const QUAD_BYTES: u32 = 20;

/// The far-tile center of one chunk column: an arithmetic shift along the
/// tile grid, exact for negative chunk coordinates because it floors
/// toward negative infinity instead of truncating toward zero.
pub fn tile_from_chunk(chunk: ChunkPos) -> TilePos {
    TilePos::new(chunk.x() >> 2, chunk.z() >> 2)
}

/// The band's inner Chebyshev radius `floor(view_distance/4)+1`: the
/// near-mesh exclusion, so the shell's nearest covered block
/// `radius * 64` starts at or beyond the near coverage
/// `view_distance * 16` and the two disks never overlap.
pub fn near_tile_radius(view_distance: u8) -> u32 {
    u32::from(view_distance) / TILE_CHUNKS + 1
}

/// The band's outer Chebyshev radius `ceil(view_distance*far_multiplier/4)`:
/// rounding up keeps the fully fogged distance covered by uploaded tiles,
/// so a non-dividing configuration leaves no sky gap at the outer edge.
pub fn far_tile_radius(view_distance: u8, far_multiplier: u8) -> u32 {
    let chunks = u32::from(view_distance) * u32::from(far_multiplier);
    (chunks + TILE_CHUNKS - 1) / TILE_CHUNKS
}

/// The static maximum byte charge of one far-tile build at `step`: the
/// worst-case quad count `3N^2+2N` over the `N = 64/step` window grid —
/// one top quad per window plus both skirt directions per boundary — times
/// the fixed quad byte width. The bound depends only on the fixed input
/// shape, never on terrain content, and the step-2 worst case is exactly
/// the native far scratch stage capacity `PreparedGeometry::MAX_LOD_QUADS`.
pub fn static_charge_bytes(step: LodStep) -> u32 {
    let windows = TILE_COLUMNS / u32::from(step.value());
    (3 * windows * windows + 2 * windows) * QUAD_BYTES
}

/// The Chebyshev tile distance, computed in i64 so legal extreme tiles
/// cannot overflow the subtraction.
fn chebyshev_distance(tile: TilePos, center: TilePos) -> i64 {
    let dx = i64::from(tile.x()) - i64::from(center.x());
    let dz = i64::from(tile.z()) - i64::from(center.z());
    dx.abs().max(dz.abs())
}

/// What one selection frame observably did; every count is measured, never
/// invented.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct SelectionReport {
    dispatched: u32,
    accepted_results: u32,
    stale_released: u32,
    abandoned_released: u32,
    retained_pending: u32,
}

impl SelectionReport {
    /// Jobs the port admitted this frame.
    pub fn dispatched(&self) -> u32 {
        self.dispatched
    }

    /// Current-generation results retained this frame.
    pub fn accepted_results(&self) -> u32 {
        self.accepted_results
    }

    /// Old-epoch or old-generation results released this frame; each drains
    /// exactly once, so a repeated frame never counts them again.
    pub fn stale_released(&self) -> u32 {
        self.stale_released
    }

    /// Current-generation results for tiles the ring no longer tracks.
    pub fn abandoned_released(&self) -> u32 {
        self.abandoned_released
    }

    /// Tiles still pending after this frame.
    pub fn retained_pending(&self) -> u32 {
        self.retained_pending
    }
}

/// The far-tile selection owner of one epoch, dimension and checked
/// seed/config generation.
///
/// It tracks pending (queued, not dispatched), in-flight (dispatched and
/// admitted by the port) and retained (current-generation results) tiles,
/// and owns no generation itself: the port's workers build the geometry.
/// The state transitions are all-or-nothing per tile — a tile is queued,
/// dispatched, retained or released, never partially owned.
pub struct LodSelection {
    epoch: SessionEpoch,
    dimension: Dimension,
    config: LodConfig,
    generation: u64,
    next_job_id: u64,
    pending: Vec<TilePos>,
    in_flight: Vec<TilePos>,
    retained: Vec<TilePos>,
}

impl LodSelection {
    /// Checked construction. The configuration is re-validated at this
    /// boundary exactly as the contract landing validated it, so an
    /// out-of-range view distance or multiplier is a typed rejection here
    /// too — never a silent clamp — while a disabled configuration
    /// constructs fine and simply selects no jobs.
    pub fn try_new(
        epoch: SessionEpoch,
        dimension: Dimension,
        config: LodConfig,
        seed_generation: u64,
    ) -> Result<Self, ClientError> {
        let view = u32::from(config.view_distance());
        let far = u32::from(config.far_multiplier());
        let view_min = u32::from(LodConfig::VIEW_DISTANCE_MIN);
        let view_max = u32::from(LodConfig::VIEW_DISTANCE_MAX);
        let far_min = u32::from(LodConfig::FAR_MULTIPLIER_MIN);
        let far_max = u32::from(LodConfig::FAR_MULTIPLIER_MAX);
        if !(view_min..=view_max).contains(&view) || !(far_min..=far_max).contains(&far) {
            return Err(ClientError::InvalidInput);
        }
        Ok(Self {
            epoch,
            dimension,
            config,
            generation: seed_generation,
            next_job_id: 1,
            pending: Vec::new(),
            in_flight: Vec::new(),
            retained: Vec::new(),
        })
    }

    /// The closed Chebyshev ring bounds of the checked configuration.
    pub fn ring_bounds(&self) -> (u32, u32) {
        (
            near_tile_radius(self.config.view_distance()),
            far_tile_radius(self.config.view_distance(), self.config.far_multiplier()),
        )
    }

    /// The selection's epoch.
    pub fn epoch(&self) -> SessionEpoch {
        self.epoch
    }

    /// The selection's dimension.
    pub fn dimension(&self) -> Dimension {
        self.dimension
    }

    /// The checked configuration.
    pub fn config(&self) -> &LodConfig {
        &self.config
    }

    /// The current checked seed/config generation.
    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// Tiles queued but not yet dispatched.
    pub fn pending_len(&self) -> usize {
        self.pending.len()
    }

    /// Tiles dispatched and admitted by the port whose results have not
    /// arrived.
    pub fn in_flight_len(&self) -> usize {
        self.in_flight.len()
    }

    /// Tiles whose current-generation results are retained.
    pub fn retained_len(&self) -> usize {
        self.retained.len()
    }

    /// Whether any state tracks the tile.
    pub fn tracks(&self, tile: TilePos) -> bool {
        self.pending.contains(&tile)
            || self.in_flight.contains(&tile)
            || self.retained.contains(&tile)
    }

    /// Queues every tile of the closed ring around `center` that no state
    /// already tracks. Repeated calls merge — a tile already pending,
    /// in-flight or retained never queues twice — and the near disk inside
    /// the inner radius is never queued. A disabled configuration queues
    /// nothing. Returns the newly queued count.
    pub fn queue_ring(&mut self, center: TilePos) -> usize {
        if !self.config.enabled() {
            return 0;
        }
        let (inner, outer) = self.ring_bounds();
        let (inner, outer) = (i64::from(inner), i64::from(outer));
        let mut queued = 0usize;
        for dz in -outer..=outer {
            for dx in -outer..=outer {
                if dx.abs().max(dz.abs()) < inner {
                    continue;
                }
                let x = i64::from(center.x()) + dx;
                let z = i64::from(center.z()) + dz;
                // A legal ring cannot leave the checked tile plane; the
                // checked conversion simply skips an impossible coordinate
                // instead of wrapping it.
                let (Ok(x), Ok(z)) = (i32::try_from(x), i32::try_from(z)) else {
                    continue;
                };
                let tile = TilePos::new(x, z);
                if self.tracks(tile) {
                    continue;
                }
                self.pending.push(tile);
                queued += 1;
            }
        }
        queued
    }

    /// Removes every tracked tile outside the closed ring around `center`.
    /// Both edges release: a tile beyond the outer radius leaves the band,
    /// and a tile that crossed inside the inner radius yields to the near
    /// mesh, so the zero-overlap guarantee survives movement. A removed
    /// in-flight tile's late result is released by the next frame's poll.
    /// Returns the removed count.
    pub fn remove_out_of_ring(&mut self, center: TilePos) -> usize {
        let (inner, outer) = self.ring_bounds();
        let (inner, outer) = (i64::from(inner), i64::from(outer));
        let out_of_band = |tile: &TilePos| {
            let distance = chebyshev_distance(*tile, center);
            distance < inner || distance > outer
        };
        let before = self.pending.len() + self.in_flight.len() + self.retained.len();
        self.pending.retain(|tile| !out_of_band(tile));
        self.in_flight.retain(|tile| !out_of_band(tile));
        self.retained.retain(|tile| !out_of_band(tile));
        before - (self.pending.len() + self.in_flight.len() + self.retained.len())
    }

    /// Re-bases the selection onto a new checked seed/config generation.
    ///
    /// Every tile of the old generation must regenerate, so in-flight and
    /// retained tiles return to pending and their late results arrive
    /// stale; the job identity counter continues, so a stale result can
    /// never collide with a new job's key.
    pub fn regenerate(&mut self, seed_generation: u64) {
        self.generation = seed_generation;
        let superseded = std::mem::take(&mut self.in_flight);
        self.pending.extend(superseded);
        let superseded = std::mem::take(&mut self.retained);
        self.pending.extend(superseded);
    }

    /// One bounded selection frame through the shared port.
    ///
    /// First the ready results drain and classify: a result of another
    /// epoch or an old seed/config generation is stale and releases exactly
    /// once — the poll itself consumed it — a current-generation result of
    /// a tracked tile is retained, and a current-generation result of an
    /// untracked tile (its ring left it behind) is released. Then pending
    /// tiles dispatch in the deterministic total order, Chebyshev distance
    /// first, then x, then z: each next job is precharged its full static
    /// maximum before it submits, so an allowance of N charges plus one
    /// byte retains job N+1 whole and a job never dispatches on a partial
    /// charge. A job the port rejects returns complete, its tile keeps its
    /// pending slot and the frame stops; the accepted in-flight work and
    /// the un-dispatched remainder are both kept.
    pub fn dispatch_frame<P: PreparationPort>(
        &mut self,
        port: &mut P,
        center: TilePos,
        params: &Arc<WorldgenParams>,
    ) -> Result<SelectionReport, ClientError> {
        let mut report = SelectionReport::default();
        while let Some(result) = port.poll_ready() {
            let key = result.key();
            if key.epoch() != self.epoch || key.generation() != self.generation {
                report.stale_released += 1;
                continue;
            }
            let TerrainKey::LodTile(tile) = *key.key() else {
                // Only far jobs of this selection share the port; a foreign
                // key is treated as untracked and released.
                report.abandoned_released += 1;
                continue;
            };
            if let Some(index) = self.in_flight.iter().position(|wanted| *wanted == tile) {
                self.in_flight.swap_remove(index);
                self.retained.push(tile);
                report.accepted_results += 1;
            } else {
                report.abandoned_released += 1;
            }
        }

        if self.config.enabled() {
            let charge = static_charge_bytes(self.config.step());
            let allowance = self.config.bytes_per_frame();
            let mut spent = 0u32;
            self.pending.sort_by(|left, right| {
                chebyshev_distance(*left, center)
                    .cmp(&chebyshev_distance(*right, center))
                    .then_with(|| left.x().cmp(&right.x()))
                    .then_with(|| left.z().cmp(&right.z()))
            });
            while !self.pending.is_empty() {
                // The precharge gate: the remaining allowance must cover the
                // next job's full static maximum, or the job is retained.
                if allowance.saturating_sub(spent) < charge {
                    break;
                }
                let tile = self.pending[0];
                let key = PreparedResourceKey::try_new(
                    self.epoch,
                    self.dimension,
                    TerrainKey::LodTile(tile),
                    self.generation,
                    // Procedural far content has no authoritative chunk
                    // revision; the checked seed/config generation is its
                    // content revision.
                    self.generation,
                    self.next_job_id,
                )?;
                let request = OwnedLodRequest::try_new(
                    Arc::clone(params),
                    [tile.x(), tile.z()],
                    self.config.step(),
                )?;
                let job = PreparationJob::try_new(key, PreparationPayload::Far(request))?;
                match port.try_submit(job) {
                    Ok(_ticket) => {
                        spent = spent.saturating_add(charge);
                        self.next_job_id = self
                            .next_job_id
                            .checked_add(1)
                            .ok_or(ClientError::Capacity)?;
                        self.pending.remove(0);
                        self.in_flight.push(tile);
                        report.dispatched += 1;
                    }
                    Err(rejected) => {
                        // The port returned the complete job; nothing was
                        // charged and the tile keeps its pending slot.
                        let _ = rejected.into_job();
                        break;
                    }
                }
            }
        }
        report.retained_pending = u32::try_from(self.pending.len()).unwrap_or(u32::MAX);
        Ok(report)
    }
}
