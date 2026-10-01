//! Terrain geometry, LOD configuration and the bounded preparation values.
//!
//! These are the C2-owned supporting values the contract landing freezes: the
//! closed `TerrainKey` union with matching visibility, the checked light
//! summary, the prepared-resource key, the owned preparation job payloads
//! (near mesh view, far LOD request) with their static byte charge, and the
//! checked result, ticket and report shapes the shared `PreparationPort`
//! trades. The trait itself lives in `contracts` beside the other C1 ports;
//! the real bounded queue is a later provider's owner.

use std::num::NonZeroU64;
use std::sync::Arc;

use mornlea_domain::{ChunkPos, Dimension};
use mornlea_engine::native::contracts::mesh::{MeshQuad, MeshRegistry, MeshView};
use mornlea_engine::native::contracts::world::{LodQuad, LodRequest, WorldgenParams};
use mornlea_protocol::MIN_Y;

use crate::contracts::{ClientError, SessionEpoch};

pub mod lod;

/// The discrete accepted far-LOD step set {2, 4, 8}; the source default is 4.
#[derive(Clone, Copy, Debug, Eq, PartialEq, PartialOrd, Ord)]
pub enum LodStep {
    Two,
    Four,
    Eight,
}

impl LodStep {
    pub fn value(self) -> u8 {
        match self {
            LodStep::Two => 2,
            LodStep::Four => 4,
            LodStep::Eight => 8,
        }
    }

    pub fn try_new(value: u8) -> Result<Self, ClientError> {
        match value {
            2 => Ok(LodStep::Two),
            4 => Ok(LodStep::Four),
            8 => Ok(LodStep::Eight),
            _ => Err(ClientError::InvalidInput),
        }
    }

    /// The accepted native step the owned request maps to during a call.
    pub(crate) fn to_native(self) -> mornlea_engine::native::contracts::world::LodStep {
        match self {
            LodStep::Two => mornlea_engine::native::contracts::world::LodStep::Two,
            LodStep::Four => mornlea_engine::native::contracts::world::LodStep::Four,
            LodStep::Eight => mornlea_engine::native::contracts::world::LodStep::Eight,
        }
    }
}

/// The frozen far-LOD configuration. Source ranges: view distance 2..=64,
/// far multiplier 2..=8 with source default 3, discrete steps {2,4,8} with
/// source default 4, and the measured existing per-frame byte allowance.
/// Loading normalization stays the source policy, not a runtime invention.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LodConfig {
    enabled: bool,
    view_distance: u8,
    far_multiplier: u8,
    step: LodStep,
    bytes_per_frame: u32,
}

impl LodConfig {
    pub const VIEW_DISTANCE_MIN: u8 = 2;
    pub const VIEW_DISTANCE_MAX: u8 = 64;
    pub const FAR_MULTIPLIER_MIN: u8 = 2;
    pub const FAR_MULTIPLIER_MAX: u8 = 8;
    pub const DEFAULT_FAR_MULTIPLIER: u8 = 3;

    pub fn try_new(
        enabled: bool,
        view_distance: u8,
        far_multiplier: u8,
        step: LodStep,
        bytes_per_frame: u32,
    ) -> Result<Self, ClientError> {
        if !(Self::VIEW_DISTANCE_MIN..=Self::VIEW_DISTANCE_MAX).contains(&view_distance) {
            return Err(ClientError::InvalidInput);
        }
        if !(Self::FAR_MULTIPLIER_MIN..=Self::FAR_MULTIPLIER_MAX).contains(&far_multiplier) {
            return Err(ClientError::InvalidInput);
        }
        Ok(Self {
            enabled,
            view_distance,
            far_multiplier,
            step,
            bytes_per_frame,
        })
    }

    pub fn enabled(&self) -> bool {
        self.enabled
    }

    pub fn view_distance(&self) -> u8 {
        self.view_distance
    }

    pub fn far_multiplier(&self) -> u8 {
        self.far_multiplier
    }

    pub fn step(&self) -> LodStep {
        self.step
    }

    pub fn bytes_per_frame(&self) -> u32 {
        self.bytes_per_frame
    }
}

/// A near-terrain section key: one chunk column and the section index inside
/// it. The section is below the fixed 24-section world height.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct SectionKey {
    chunk: ChunkPos,
    section: u8,
}

impl SectionKey {
    pub const SECTIONS_PER_CHUNK: u8 = 24;

    pub fn try_new(chunk: ChunkPos, section: u8) -> Result<Self, ClientError> {
        if section >= Self::SECTIONS_PER_CHUNK {
            return Err(ClientError::InvalidInput);
        }
        Ok(Self { chunk, section })
    }

    pub fn chunk(&self) -> ChunkPos {
        self.chunk
    }

    pub fn section(&self) -> u8 {
        self.section
    }
}

/// A far-LOD tile position: checked i32 coordinates in 64-block tile units,
/// not chunk units. Negative tiles are legal world geometry.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct TilePos {
    x: i32,
    z: i32,
}

impl TilePos {
    pub fn new(x: i32, z: i32) -> Self {
        Self { x, z }
    }

    pub fn x(&self) -> i32 {
        self.x
    }

    pub fn z(&self) -> i32 {
        self.z
    }
}

/// The closed terrain key union. A far tile is never published under a fake
/// section identity, and every ordered removal names the full key.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum TerrainKey {
    Section(SectionKey),
    LodTile(TilePos),
}

/// The visibility class of one terrain key; it must match the key tag.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TerrainVisibility {
    Near,
    Far,
}

/// The checked sky and block light summary; both channels are at most 15.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LightSummary {
    sky: u8,
    block: u8,
}

impl LightSummary {
    pub fn try_new(sky: u8, block: u8) -> Result<Self, ClientError> {
        if sky > 15 || block > 15 {
            return Err(ClientError::InvalidInput);
        }
        Ok(Self { sky, block })
    }

    pub fn sky(&self) -> u8 {
        self.sky
    }

    pub fn block(&self) -> u8 {
        self.block
    }
}

/// The opaque prepared-resource key: the epoch, dimension, terrain key,
/// generation, content revision and nonzero job id of one retained geometry.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PreparedResourceKey {
    epoch: SessionEpoch,
    dimension: Dimension,
    key: TerrainKey,
    generation: u64,
    content_revision: u64,
    job_id: NonZeroU64,
}

impl PreparedResourceKey {
    pub fn try_new(
        epoch: SessionEpoch,
        dimension: Dimension,
        key: TerrainKey,
        generation: u64,
        content_revision: u64,
        job_id: u64,
    ) -> Result<Self, ClientError> {
        let job_id = NonZeroU64::try_from(job_id).map_err(|_| ClientError::InvalidInput)?;
        Ok(Self {
            epoch,
            dimension,
            key,
            generation,
            content_revision,
            job_id,
        })
    }

    pub fn epoch(&self) -> SessionEpoch {
        self.epoch
    }

    pub fn dimension(&self) -> Dimension {
        self.dimension
    }

    pub fn key(&self) -> &TerrainKey {
        &self.key
    }

    pub fn generation(&self) -> u64 {
        self.generation
    }

    pub fn content_revision(&self) -> u64 {
        self.content_revision
    }

    pub fn job_id(&self) -> NonZeroU64 {
        self.job_id
    }
}

/// The owned near-mesh input of one preparation job.
///
/// It owns the immutable block and height allocations the borrowed native
/// `MeshView` aliases only while a worker holds the borrow; the registry is
/// shared ownership and is charged once while the owner holds it.
#[derive(Clone, Debug)]
pub struct OwnedMeshView {
    blocks: Box<[u16; 110592]>,
    heights_present: [bool; 9],
    heights: Box<[[i16; 256]; 9]>,
    section_origin_y: i32,
    registry: Arc<MeshRegistry>,
}

impl OwnedMeshView {
    pub fn try_new(
        blocks: Box<[u16; 110592]>,
        heights_present: [bool; 9],
        heights: Box<[[i16; 256]; 9]>,
        section_origin_y: i32,
        registry: Arc<MeshRegistry>,
    ) -> Result<Self, ClientError> {
        // The native admitted ranges are the fixed array shapes and the
        // registry's own checked construction; the origin is validated
        // against the addressed section when the job is paired.
        Ok(Self {
            blocks,
            heights_present,
            heights,
            section_origin_y,
            registry,
        })
    }

    pub fn blocks(&self) -> &[u16; 110592] {
        &self.blocks
    }

    pub fn heights_present(&self) -> &[bool; 9] {
        &self.heights_present
    }

    pub fn heights(&self) -> &[[i16; 256]; 9] {
        &self.heights
    }

    pub fn section_origin_y(&self) -> i32 {
        self.section_origin_y
    }

    pub fn registry(&self) -> &Arc<MeshRegistry> {
        &self.registry
    }

    /// The borrowed native view, valid only while this owned value is
    /// borrowed and unmoved.
    pub fn as_mesh_view(&self) -> MeshView<'_> {
        MeshView {
            blocks: &self.blocks,
            heights_present: &self.heights_present,
            heights: &self.heights,
            section_origin_y: self.section_origin_y,
            registry: &self.registry,
        }
    }

    /// The job's own allocation charge: the block and height arrays, the
    /// present flags, the origin scalar and the shared Arc slot. The shared
    /// registry allocation itself is charged once by the owner tracking Arc
    /// identity, not once per holder.
    pub fn owned_bytes(&self) -> usize {
        self.blocks.len() * 2 + self.heights_present.len() + self.heights.len() * 256 * 2 + 4 + 8
    }
}

/// The owned far-LOD request of one preparation job. It maps to the accepted
/// borrowed native `LodRequest` only during the worker's call.
#[derive(Clone, Debug)]
pub struct OwnedLodRequest {
    params: Arc<WorldgenParams>,
    tile: [i32; 2],
    step: LodStep,
}

impl OwnedLodRequest {
    pub fn try_new(
        params: Arc<WorldgenParams>,
        tile: [i32; 2],
        step: LodStep,
    ) -> Result<Self, ClientError> {
        Ok(Self { params, tile, step })
    }

    pub fn params(&self) -> &Arc<WorldgenParams> {
        &self.params
    }

    pub fn tile(&self) -> [i32; 2] {
        self.tile
    }

    pub fn step(&self) -> LodStep {
        self.step
    }

    /// The borrowed native request, valid only while this owned value is
    /// borrowed and unmoved.
    pub fn as_lod_request(&self) -> LodRequest<'_> {
        LodRequest {
            params: &self.params,
            tile: self.tile,
            step: self.step.to_native(),
        }
    }

    pub fn owned_bytes(&self) -> usize {
        8 + 8 + 1
    }
}

/// The owned job payload: a near section mesh or a far LOD request.
#[derive(Clone, Debug)]
pub enum PreparationPayload {
    Near(OwnedMeshView),
    Far(OwnedLodRequest),
}

impl PreparationPayload {
    /// The payload's own allocation charge, excluding shared Arc allocations
    /// the owner tracks by identity.
    pub fn owned_bytes(&self) -> usize {
        match self {
            PreparationPayload::Near(near) => near.owned_bytes(),
            PreparationPayload::Far(far) => far.owned_bytes(),
        }
    }
}

/// One bounded preparation job: the prepared-resource key it will satisfy
/// and the owned payload. The checked constructor enforces the key/payload
/// pairing and the native admitted ranges before any queue sees the job.
#[derive(Clone, Debug)]
pub struct PreparationJob {
    key: PreparedResourceKey,
    payload: PreparationPayload,
}

impl PreparationJob {
    pub fn try_new(
        key: PreparedResourceKey,
        payload: PreparationPayload,
    ) -> Result<Self, ClientError> {
        match (&key.key, &payload) {
            (TerrainKey::Section(section), PreparationPayload::Near(near)) => {
                let expected_origin = MIN_Y + i32::from(section.section()) * 16;
                if near.section_origin_y() != expected_origin {
                    return Err(ClientError::InvalidInput);
                }
            }
            (TerrainKey::LodTile(tile), PreparationPayload::Far(far)) => {
                if far.tile() != [tile.x(), tile.z()] {
                    return Err(ClientError::InvalidInput);
                }
            }
            _ => return Err(ClientError::InvalidInput),
        }
        Ok(Self { key, payload })
    }

    pub fn key(&self) -> &PreparedResourceKey {
        &self.key
    }

    pub fn payload(&self) -> &PreparationPayload {
        &self.payload
    }

    pub fn into_parts(self) -> (PreparedResourceKey, PreparationPayload) {
        (self.key, self.payload)
    }
}

/// The published preparation output. Native bounds are enforced before a
/// value is published: one near section mesh carries at most the measured
/// per-section quad ceiling and one far tile at most the native scratch
/// stage capacity.
#[derive(Clone, Debug, PartialEq)]
pub enum PreparedGeometry {
    Near(Vec<MeshQuad>),
    Far(Vec<LodQuad>),
}

impl PreparedGeometry {
    /// The measured per-section packed-quad ceiling.
    pub const MAX_SECTION_QUADS: usize = 24576;
    /// The native far scratch stage capacity per tile build.
    pub const MAX_LOD_QUADS: usize = 3136;

    pub fn try_new(value: PreparedGeometry) -> Result<Self, ClientError> {
        let len = match &value {
            PreparedGeometry::Near(quads) => quads.len(),
            PreparedGeometry::Far(quads) => quads.len(),
        };
        let max = match &value {
            PreparedGeometry::Near(_) => Self::MAX_SECTION_QUADS,
            PreparedGeometry::Far(_) => Self::MAX_LOD_QUADS,
        };
        if len > max {
            return Err(ClientError::Capacity);
        }
        Ok(value)
    }

    pub fn quads(&self) -> usize {
        match self {
            PreparedGeometry::Near(quads) => quads.len(),
            PreparedGeometry::Far(quads) => quads.len(),
        }
    }
}

/// A privately issued admission ticket tied to the key's epoch and job
/// generation. Tickets are never pointers and never reused.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct PreparationTicket {
    value: NonZeroU64,
}

impl PreparationTicket {
    pub fn try_new(value: u64) -> Result<Self, ClientError> {
        let value = NonZeroU64::try_from(value).map_err(|_| ClientError::InvalidInput)?;
        Ok(Self { value })
    }

    pub fn get(&self) -> u64 {
        self.value.get()
    }
}

/// One drained preparation result: it retains its exact request identity on
/// success and on error, and a stale result is released exactly once.
#[derive(Clone, Debug)]
pub struct PreparationResult {
    ticket: PreparationTicket,
    key: PreparedResourceKey,
    outcome: Result<Arc<PreparedGeometry>, ClientError>,
}

impl PreparationResult {
    pub fn try_new(
        ticket: PreparationTicket,
        key: PreparedResourceKey,
        outcome: Result<Arc<PreparedGeometry>, ClientError>,
    ) -> Result<Self, ClientError> {
        Ok(Self {
            ticket,
            key,
            outcome,
        })
    }

    pub fn ticket(&self) -> PreparationTicket {
        self.ticket
    }

    pub fn key(&self) -> &PreparedResourceKey {
        &self.key
    }

    pub fn outcome(&self) -> &Result<Arc<PreparedGeometry>, ClientError> {
        &self.outcome
    }

    pub fn into_outcome(self) -> Result<Arc<PreparedGeometry>, ClientError> {
        self.outcome
    }
}

/// A failed admission: the typed class plus the complete owned job, so the
/// caller never loses the payload it tried to submit.
#[derive(Clone, Debug)]
pub struct RejectedPreparation {
    error: ClientError,
    job: PreparationJob,
}

impl RejectedPreparation {
    pub fn try_new(error: ClientError, job: PreparationJob) -> Result<Self, ClientError> {
        Ok(Self { error, job })
    }

    pub fn error(&self) -> ClientError {
        self.error
    }

    pub fn job(&self) -> &PreparationJob {
        &self.job
    }

    pub fn into_job(self) -> PreparationJob {
        self.job
    }
}

/// What one epoch invalidation released. Counts are measured, never invented.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct InvalidationReport {
    jobs_cancelled: u32,
    results_stale: u32,
    bytes_released: u64,
}

impl InvalidationReport {
    pub fn try_new(
        jobs_cancelled: u32,
        results_stale: u32,
        bytes_released: u64,
    ) -> Result<Self, ClientError> {
        Ok(Self {
            jobs_cancelled,
            results_stale,
            bytes_released,
        })
    }

    pub fn jobs_cancelled(&self) -> u32 {
        self.jobs_cancelled
    }

    pub fn results_stale(&self) -> u32 {
        self.results_stale
    }

    pub fn bytes_released(&self) -> u64 {
        self.bytes_released
    }
}
