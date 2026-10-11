//! Terrain geometry, LOD configuration and the bounded preparation values.
//!
//! These are the C2-owned supporting values the contract landing freezes: the
//! closed `TerrainKey` union with matching visibility, the checked light
//! summary, the prepared-resource key, the owned preparation job payloads
//! (near mesh view, far LOD request) with their static byte charge, and the
//! checked result, ticket and report shapes the shared `PreparationPort`
//! trades. The trait itself lives in `contracts` beside the other C1 ports;
//! `PreparationQueue` below is its real bounded owner and the retained
//! prepared-resource arena.

use std::collections::VecDeque;
use std::num::NonZeroU64;
use std::sync::Arc;

use mornlea_domain::{ChunkPos, Dimension};
use mornlea_engine::native::contracts::mesh::{MeshQuad, MeshRegistry, MeshView};
use mornlea_engine::native::contracts::world::{LodQuad, LodRequest, WorldgenParams};
use mornlea_protocol::MIN_Y;

use crate::contracts::{
    ClientError, ClientLimits, ClientWorkBudget, PreparationPort, SessionEpoch,
};

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

/// One queue-held admitted job: the ticket it was admitted under, the owned
/// payload, its payload-only byte charge and the identities of every shared
/// Arc allocation the payload still references.
struct PendingUnit {
    ticket: PreparationTicket,
    job: PreparationJob,
    charge: usize,
    references: Vec<usize>,
}

/// One worker-held completed result awaiting delivery and arena retention.
struct CompletedUnit {
    result: PreparationResult,
    charge: usize,
}

/// One arena-retained geometry: the delivered result's key beside its shared
/// immutable allocation, held until a forget, a superseding result or an
/// invalidation releases it.
struct RetainedUnit {
    key: PreparedResourceKey,
    geometry: Arc<PreparedGeometry>,
    charge: usize,
}

/// The freshest admitted identity of one terrain key: the epoch, generation
/// and content revision of the newest admitted job, plus whether that exact
/// identity has already been delivered.
struct HighWaterMark {
    dimension: Dimension,
    terrain: TerrainKey,
    epoch: u64,
    generation: u64,
    content: u64,
    delivered: bool,
}

impl HighWaterMark {
    fn identity(&self) -> (u64, u64, u64) {
        (self.epoch, self.generation, self.content)
    }
}

/// The measured registry allocation charge: the entry table, the visibility
/// words and the two scalar ids. This is the frozen convention the contract
/// landing's doubles charge and its registered case pins.
fn registry_allocation(registry: &MeshRegistry) -> usize {
    std::mem::size_of_val(registry.entries()) + registry.visibility().len() * 8 + 4
}

/// The geometry allocation charge of one owned result: the discriminant byte
/// plus the packed quad table the shared `Arc` owns.
fn geometry_allocation(geometry: &PreparedGeometry) -> usize {
    1 + std::mem::size_of::<MeshQuad>() * geometry.quads()
}

/// The bounded near-preparation owner: the real `PreparationPort` provider
/// and the retained prepared-resource arena.
///
/// Ownership lifecycle: an admitted job is queue-held in `pending` and owns
/// its payload allocation (the owned arrays plus every shared Arc allocation,
/// charged once by identity while any held unit references it). `work`
/// consumes FIFO units under the per-step mesh budget; the consumed payload
/// releases and a completed result becomes worker-held in `completed`, owning
/// only its geometry allocation. `poll_ready` delivers the freshest completed
/// result and the arena retains its geometry in `retained` until the
/// publication owner reports no remaining reference through `forget`, a
/// newer result of the same terrain key supersedes it, or an invalidation
/// releases it. The worker only ever consumes the immutable input
/// allocations; it never observes or mutates any mirror, input admission
/// owner or published frame.
///
/// Bounds: every owned record (queue-held, worker-held or retained) counts
/// against `preparation_results`, and every owned byte counts against
/// `preparation_bytes` with checked arithmetic, so each bound admits exactly
/// its capacity and rejects one more with the typed `Capacity` class before
/// any allocation, returning the complete job. Key, ticket and mark records
/// are fixed-size scalars outside the allocation charge, matching the frozen
/// payload-plus-shared-allocation convention.
///
/// Staleness: an invalidated epoch is retired monotonically and never
/// resurrects, so its jobs reject as `StaleEpoch` and its results release
/// exactly once. A newer admitted identity of the same terrain key is the
/// high-water mark: an older queued result releases silently at completion
/// and an equal identity cannot deliver twice, so a stale or duplicate
/// completion never publishes.
///
/// The completed geometry is the deterministic empty stage of the checked
/// `PreparedGeometry` constructor; wiring the native numerical facade builds
/// (near section meshing and far LOD tiles) into this owner is the terrain
/// integration seam and does not alter any ownership rule above.
pub struct PreparationQueue {
    limits: ClientLimits,
    pending: VecDeque<PendingUnit>,
    completed: VecDeque<CompletedUnit>,
    retained: VecDeque<RetainedUnit>,
    high_water: Vec<HighWaterMark>,
    shared: Vec<(usize, usize)>,
    charge: usize,
    next_ticket: u64,
    tickets_issued: u64,
    retired_through: Option<u64>,
}

impl PreparationQueue {
    pub fn new(limits: ClientLimits) -> Self {
        Self {
            limits,
            pending: VecDeque::new(),
            completed: VecDeque::new(),
            retained: VecDeque::new(),
            high_water: Vec::new(),
            shared: Vec::new(),
            charge: 0,
            next_ticket: 1,
            tickets_issued: 0,
            retired_through: None,
        }
    }

    /// The total owned byte charge: queue-held payloads plus worker-held and
    /// retained geometry, measured, never invented.
    pub fn charge(&self) -> usize {
        self.charge
    }

    pub fn tickets_issued(&self) -> u64 {
        self.tickets_issued
    }

    /// Queue-held admitted jobs not yet consumed by a work step.
    pub fn pending_jobs(&self) -> usize {
        self.pending.len()
    }

    /// Worker-held completed results not yet delivered.
    pub fn completed_results(&self) -> usize {
        self.completed.len()
    }

    /// Arena-retained geometries not yet forgotten or invalidated.
    pub fn retained_resources(&self) -> usize {
        self.retained.len()
    }

    /// Safe Rust-only lookup of one retained prepared geometry. A key of a
    /// retired epoch is the typed stale rejection; an unknown or already
    /// released key is the typed invalid input; no fabricated resource is
    /// ever returned.
    pub fn prepared_resource(
        &self,
        key: &PreparedResourceKey,
    ) -> Result<Arc<PreparedGeometry>, ClientError> {
        if self.is_retired(key.epoch()) {
            return Err(ClientError::StaleEpoch);
        }
        self.retained
            .iter()
            .find(|entry| entry.key == *key)
            .map(|entry| Arc::clone(&entry.geometry))
            .ok_or(ClientError::InvalidInput)
    }

    /// Releases the retained geometry of exactly this resource key and
    /// reports the bytes released. The publication owner calls this once no
    /// published frame references the resource any more; a late or repeated
    /// forget after a supersede, a forget or an invalidation releases
    /// nothing, so ownership is released exactly once.
    pub fn forget(&mut self, key: &PreparedResourceKey) -> Result<u64, ClientError> {
        if let Some(index) = self.retained.iter().position(|entry| entry.key == *key)
            && let Some(released) = self.retained.remove(index)
        {
            self.charge -= released.charge;
            return Ok(released.charge as u64);
        }
        Ok(0)
    }

    /// Consumes FIFO pending units under the shared per-step work budget,
    /// returning how many units were consumed (completed or released as
    /// stale). The owner consumes only the mesh half of the budget, rechecked
    /// against its configured mesh-work ceiling before any dequeue: a
    /// zero-mesh step retains the accepted FIFO remainder, and no step ever
    /// dequeues beyond the tighter of the two ceilings.
    pub fn work(&mut self, budget: ClientWorkBudget) -> u16 {
        let drain = usize::from(budget.meshes()).min(self.limits.mesh_work());
        let mut consumed = 0usize;
        while consumed < drain {
            let Some(unit) = self.pending.pop_front() else {
                break;
            };
            self.consume_pending(unit);
            consumed += 1;
        }
        u16::try_from(consumed).unwrap_or(u16::MAX)
    }

    /// The stable identity of one shared allocation, used to charge it once
    /// while the owner holds any reference to it.
    fn arc_identity<T: ?Sized>(arc: &Arc<T>) -> usize {
        Arc::as_ptr(arc) as *const () as usize
    }

    /// An epoch at or below the retired bound is dead; epochs are issued
    /// monotonically by the session owner, so the bound never resurrects one.
    fn is_retired(&self, epoch: SessionEpoch) -> bool {
        self.retired_through
            .is_some_and(|bound| epoch.get() <= bound)
    }

    /// True when the key's identity may still deliver: it is strictly fresher
    /// than every high-water mark of its terrain key, or it is the mark
    /// itself and that mark has not delivered yet.
    fn is_stale_delivery(&self, key: &PreparedResourceKey) -> bool {
        let identity = (key.epoch().get(), key.generation(), key.content_revision());
        self.high_water.iter().any(|mark| {
            mark.dimension == key.dimension()
                && mark.terrain == *key.key()
                && (identity < mark.identity() || (identity == mark.identity() && mark.delivered))
        })
    }

    /// Records the freshest admitted identity of one terrain key; an equal
    /// identity keeps its delivery state and an older one changes nothing, so
    /// only a strictly newer submission moves the mark.
    fn bump_high_water(&mut self, key: &PreparedResourceKey) {
        let identity = (key.epoch().get(), key.generation(), key.content_revision());
        if let Some(mark) = self
            .high_water
            .iter_mut()
            .find(|mark| mark.dimension == key.dimension() && mark.terrain == *key.key())
        {
            if identity > mark.identity() {
                mark.epoch = identity.0;
                mark.generation = identity.1;
                mark.content = identity.2;
                mark.delivered = false;
            }
            return;
        }
        self.high_water.push(HighWaterMark {
            dimension: key.dimension(),
            terrain: *key.key(),
            epoch: identity.0,
            generation: identity.1,
            content: identity.2,
            delivered: false,
        });
    }

    /// The payload-only charge of one job beside the identity and allocation
    /// size of the shared Arc its payload references. A shared allocation is
    /// charged once for the whole time any held unit references it, while
    /// separately allocated equal payloads charge separately.
    fn payload_charge(job: &PreparationJob) -> (usize, Vec<(usize, usize)>) {
        match job.payload() {
            PreparationPayload::Near(near) => {
                let identity = Self::arc_identity(near.registry());
                (
                    near.owned_bytes(),
                    vec![(identity, registry_allocation(near.registry()))],
                )
            }
            PreparationPayload::Far(far) => {
                let identity = Self::arc_identity(far.params());
                (
                    far.owned_bytes(),
                    vec![(identity, std::mem::size_of::<WorldgenParams>())],
                )
            }
        }
    }

    /// Releases one queue-held ownership: its payload bytes always, plus each
    /// referenced shared allocation once the last holder releases it, so a
    /// later allocation reusing an address always charges again. The unit
    /// itself is already out of the queue. Returns the measured bytes
    /// released.
    fn release_ownership(&mut self, charge: usize, references: &[usize]) -> u64 {
        let mut released = charge;
        self.charge -= charge;
        for identity in references {
            if !self
                .pending
                .iter()
                .any(|other| other.references.contains(identity))
                && let Some(index) = self.shared.iter().position(|(held, _)| held == identity)
            {
                let (_, bytes) = self.shared.remove(index);
                released += bytes;
                self.charge -= bytes;
            }
        }
        released as u64
    }

    /// The worker step for one consumed unit: the payload allocation ends
    /// here, a fresh identity publishes a checked result that owns its
    /// geometry, and a retired, stale or duplicate identity releases exactly
    /// once without publishing.
    fn consume_pending(&mut self, unit: PendingUnit) {
        let PendingUnit {
            ticket,
            job,
            charge,
            references,
        } = unit;
        // The worker consumes the owned input allocation here, and it never
        // observes or mutates any mirror, input admission owner or frame.
        let _ = self.release_ownership(charge, &references);
        let key = *job.key();
        if self.is_retired(key.epoch()) || self.is_stale_delivery(&key) {
            return;
        }
        let geometry = match job.payload() {
            PreparationPayload::Near(_) => PreparedGeometry::Near(Vec::new()),
            PreparationPayload::Far(_) => PreparedGeometry::Far(Vec::new()),
        };
        let (outcome, charge) = match PreparedGeometry::try_new(geometry).map(Arc::new) {
            Ok(geometry) => {
                let geometry_charge = geometry_allocation(&geometry);
                match self.charge.checked_add(geometry_charge) {
                    Some(total) if total <= self.limits.preparation_bytes() => {
                        (Ok(geometry), geometry_charge)
                    }
                    // A geometry the byte bound cannot own publishes as a
                    // typed capacity failure instead of an over-bound
                    // allocation.
                    _ => (Err(ClientError::Capacity), 0),
                }
            }
            Err(error) => (Err(error), 0),
        };
        let result = PreparationResult::try_new(ticket, key, outcome).expect("checked result");
        self.charge += charge;
        self.completed.push_back(CompletedUnit { result, charge });
    }
}

impl PreparationPort for PreparationQueue {
    fn try_submit(
        &mut self,
        job: PreparationJob,
    ) -> Result<PreparationTicket, RejectedPreparation> {
        if self.is_retired(job.key().epoch()) {
            // A retired epoch never admits new work; the complete job returns
            // with the typed stale class.
            return Err(
                RejectedPreparation::try_new(ClientError::StaleEpoch, job).expect("rejection")
            );
        }
        // Every owned record counts: queue-held, worker-held and retained.
        let held = self.pending.len() + self.completed.len() + self.retained.len();
        let Some(held_plus_one) = held.checked_add(1) else {
            return Err(
                RejectedPreparation::try_new(ClientError::Capacity, job).expect("rejection")
            );
        };
        if held_plus_one > self.limits.preparation_results() {
            return Err(
                RejectedPreparation::try_new(ClientError::Capacity, job).expect("rejection")
            );
        }
        let (payload_bytes, references) = Self::payload_charge(&job);
        // The byte bound uses checked arithmetic: an overflow is the typed
        // capacity rejection, never a wrap or a panic. A shared allocation
        // that is already charged adds nothing.
        let mut extra = payload_bytes;
        for (identity, bytes) in &references {
            if !self.shared.iter().any(|(held, _)| held == identity) {
                extra += *bytes;
            }
        }
        let Some(new_charge) = self.charge.checked_add(extra) else {
            return Err(
                RejectedPreparation::try_new(ClientError::Capacity, job).expect("rejection")
            );
        };
        if new_charge > self.limits.preparation_bytes() {
            return Err(
                RejectedPreparation::try_new(ClientError::Capacity, job).expect("rejection")
            );
        }
        // Admission is all-or-nothing: nothing above mutated the owner, and
        // every step of the commit below is infallible.
        let Some(next) = self.next_ticket.checked_add(1) else {
            return Err(
                RejectedPreparation::try_new(ClientError::Capacity, job).expect("rejection")
            );
        };
        let ticket = PreparationTicket::try_new(self.next_ticket).expect("monotonic ticket");
        self.next_ticket = next;
        self.tickets_issued = self.tickets_issued.saturating_add(1);
        self.charge = new_charge;
        for (identity, bytes) in &references {
            if !self.shared.iter().any(|(held, _)| held == identity) {
                self.shared.push((*identity, *bytes));
            }
        }
        self.bump_high_water(job.key());
        self.pending.push_back(PendingUnit {
            ticket,
            job,
            charge: payload_bytes,
            references: references
                .into_iter()
                .map(|(identity, _)| identity)
                .collect(),
        });
        Ok(ticket)
    }

    fn poll_ready(&mut self) -> Option<PreparationResult> {
        loop {
            let unit = self.completed.pop_front()?;
            let CompletedUnit { result, charge } = unit;
            let key = *result.key();
            if self.is_retired(key.epoch()) || self.is_stale_delivery(&key) {
                // A result that went stale while awaiting delivery releases
                // its geometry exactly once and never publishes.
                self.charge -= charge;
                continue;
            }
            let outcome = result.outcome().clone();
            if let Ok(geometry) = outcome {
                // Retain the delivered geometry: a superseded entry of the
                // same terrain key releases exactly once and the fresh
                // allocation takes its place at the same total charge.
                if let Some(index) = self.retained.iter().position(|entry| {
                    entry.key.dimension() == key.dimension() && *entry.key.key() == *key.key()
                }) && let Some(superseded) = self.retained.remove(index)
                {
                    self.charge -= superseded.charge;
                }
                self.retained.push_back(RetainedUnit {
                    key,
                    geometry,
                    charge,
                });
            }
            if let Some(mark) = self
                .high_water
                .iter_mut()
                .find(|mark| mark.dimension == key.dimension() && mark.terrain == *key.key())
            {
                mark.delivered = true;
            }
            return Some(result);
        }
    }

    fn invalidate(&mut self, epoch: SessionEpoch) -> InvalidationReport {
        // Invalidation is monotonic: every epoch at or below the named one is
        // dead, matching the session owner's monotonically issued epochs.
        self.retired_through = Some(
            self.retired_through
                .map_or(epoch.get(), |prev| prev.max(epoch.get())),
        );
        let mut jobs_cancelled = 0u32;
        let mut results_stale = 0u32;
        let mut bytes_released = 0u64;

        let remaining = self.pending.len();
        for _ in 0..remaining {
            let unit = self.pending.pop_front().expect("queued unit");
            if self.is_retired(unit.job.key().epoch()) {
                jobs_cancelled += 1;
                let PendingUnit {
                    charge, references, ..
                } = unit;
                bytes_released += self.release_ownership(charge, &references);
            } else {
                self.pending.push_back(unit);
            }
        }
        let remaining = self.completed.len();
        for _ in 0..remaining {
            let unit = self.completed.pop_front().expect("completed unit");
            if self.is_retired(unit.result.key().epoch()) {
                results_stale += 1;
                bytes_released += unit.charge as u64;
                self.charge -= unit.charge;
            } else {
                self.completed.push_back(unit);
            }
        }
        let remaining = self.retained.len();
        for _ in 0..remaining {
            let unit = self.retained.pop_front().expect("retained unit");
            if self.is_retired(unit.key.epoch()) {
                // Retained geometry joins the released byte total without its
                // own count: it was already delivered, so it is neither a
                // cancelled job nor a stale result.
                bytes_released += unit.charge as u64;
                self.charge -= unit.charge;
            } else {
                self.retained.push_back(unit);
            }
        }
        // High-water marks of retired epochs drop with their ownership, so a
        // fresh epoch's first identity for the same terrain key is fresh.
        let bound = self.retired_through.expect("just set");
        self.high_water.retain(|mark| mark.epoch > bound);
        InvalidationReport::try_new(jobs_cancelled, results_stale, bytes_released)
            .expect("checked report")
    }
}

#[cfg(test)]
mod tests {
    //! Internal checks for the checked-overflow branches of the byte
    //! accounting. The frozen limits cap `preparation_bytes` far below
    //! `usize::MAX`, so an arithmetic overflow of the running charge is
    //! unreachable through the public surface; these tests drive the counter
    //! directly to pin that both checked points still reject with the typed
    //! `Capacity` class and mutate nothing.

    use super::*;
    use mornlea_engine::native::contracts::world::Materials;

    /// One paired far job with the smallest admitted payload charge.
    fn far_job_for(job_id: u64) -> PreparationJob {
        let materials = Materials {
            air: 0,
            stone: 1,
            dirt: 2,
            grass: 3,
            bedrock: 4,
            snow: 5,
            sand: 6,
            clay: 7,
            gravel: 8,
            iron_ore: 9,
            coal_ore: 10,
            oak_log: 11,
            leaves: 12,
            water: 13,
            short_grass: 14,
        };
        let mut perm = [0u8; 512];
        for (index, entry) in perm.iter_mut().enumerate() {
            *entry = (index % 256) as u8;
        }
        let params = Arc::new(WorldgenParams::try_new(1, materials, perm).expect("params"));
        let key = PreparedResourceKey::try_new(
            SessionEpoch::try_new(1).expect("epoch"),
            Dimension::OVERWORLD,
            TerrainKey::LodTile(TilePos::new(3, 4)),
            1,
            1,
            job_id,
        )
        .expect("checked key");
        let request = OwnedLodRequest::try_new(params, [3, 4], LodStep::Four).expect("request");
        PreparationJob::try_new(key, PreparationPayload::Far(request)).expect("paired far job")
    }

    /// The admission-side overflow: `try_submit`'s `checked_add` returns the
    /// typed `Capacity` rejection with the complete job and leaves the
    /// counter, the ticket sequence and the shared-identity set untouched.
    #[test]
    fn admission_byte_overflow_is_typed_capacity() {
        let mut port = PreparationQueue::new(ClientLimits::try_new().expect("frozen limits"));
        // The impossible-through-the-public-surface state: a running charge
        // one byte away from the address space, set directly instead of
        // allocating it.
        port.charge = usize::MAX;
        let rejection = port
            .try_submit(far_job_for(1))
            .expect_err("the charge overflow rejects admission");
        assert_eq!(rejection.error(), ClientError::Capacity);
        assert_eq!(
            rejection.job().key().job_id().get(),
            1,
            "the whole job is returned"
        );
        assert_eq!(port.tickets_issued(), 0, "no ticket is issued");
        assert_eq!(port.pending_jobs(), 0, "nothing is queued");
        assert_eq!(port.charge(), usize::MAX, "the counter is unchanged");
        assert!(port.shared.is_empty(), "no shared identity is recorded");
    }

    /// The completion-side overflow: when the running charge cannot own even
    /// the one-byte empty geometry, `consume_pending`'s `checked_add` rejects
    /// inside the worker, publishes the typed `Capacity` failure as the
    /// result, retains nothing and changes the counter by nothing.
    #[test]
    fn completion_byte_overflow_is_typed_capacity() {
        let mut port = PreparationQueue::new(ClientLimits::try_new().expect("frozen limits"));
        // A queued unit with a zero payload charge, staged directly so the
        // release step subtracts nothing and the overflow state reaches the
        // completion-side check exactly.
        port.charge = usize::MAX;
        port.pending.push_back(PendingUnit {
            ticket: PreparationTicket::try_new(1).expect("ticket"),
            job: far_job_for(1),
            charge: 0,
            references: Vec::new(),
        });
        let budget = ClientWorkBudget::try_new(0, 1).expect("budget");
        assert_eq!(port.work(budget), 1, "the unit is consumed");
        let result = port.poll_ready().expect("the typed failure publishes");
        assert_eq!(
            result.outcome(),
            &Err(ClientError::Capacity),
            "the overflow is the typed capacity failure"
        );
        assert_eq!(result.key().job_id().get(), 1, "exact request identity");
        assert_eq!(
            port.charge(),
            usize::MAX,
            "the failed geometry charges nothing"
        );
        assert_eq!(port.retained_resources(), 0, "nothing is retained");
        assert!(port.poll_ready().is_none(), "the queue drains exactly");
    }
}
