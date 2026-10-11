//! Far-LOD tile selection contract cases.
//!
//! The named cases drive the bounded far-tile selection: arithmetic-shift
//! tile centers, the closed Chebyshev ring outside the near mesh disk, the
//! deterministic dispatch order, the per-job static precharge against the
//! frame allowance, out-of-ring removal and the exactly-once release of
//! old-epoch or old-generation results, all through the accepted
//! deterministic port double. The red run of this table drove the
//! deliberately wrong `Wrong` driver — the only behavior that existed
//! while the provider file was the contract landing's doc-only stub — and
//! its recorded wrong behavior stays executable by flipping `DRIVER`.

use std::sync::Arc;

use mornlea_client_core::contracts::{ClientLimits, PreparationPort, SessionEpoch};
use mornlea_client_core::preparation::lod;
use mornlea_client_core::preparation::lod::LodSelection;
use mornlea_client_core::preparation::{
    InvalidationReport, LodConfig, LodStep, OwnedLodRequest, PreparationJob, PreparationPayload,
    PreparationResult, PreparationTicket, PreparedResourceKey, RejectedPreparation, TerrainKey,
    TilePos,
};
use mornlea_domain::{ChunkPos, Dimension};
use mornlea_engine::native::contracts::world::WorldgenParams;

use crate::preparation_port::fixture_params;
use crate::support::PreparationFaucet;

/// Which implementation the named cases drive. `Wrong` is the deliberately
/// wrong stand-in set the behavioral reds ran against: a truncating tile
/// center, a ring that overlaps the near mesh disk, a one-byte dispatch
/// charge, a queue that ignores the disabled flag and a poller that admits
/// stale generations.
enum Driver {
    Provider,
    Wrong,
}

/// The implementation the named cases run against.
const DRIVER: Driver = Driver::Provider;

// --- driver-switched stand-ins ---

fn center_of(chunk: ChunkPos) -> TilePos {
    match DRIVER {
        Driver::Provider => lod::tile_from_chunk(chunk),
        // Wrong: rounding division truncates negative chunks toward zero
        // instead of flooring along the tile grid.
        Driver::Wrong => TilePos::new(chunk.x() / 4, chunk.z() / 4),
    }
}

fn ring_of(config: &LodConfig) -> (u32, u32) {
    match DRIVER {
        Driver::Provider => (
            lod::near_tile_radius(config.view_distance()),
            lod::far_tile_radius(config.view_distance(), config.far_multiplier()),
        ),
        // Wrong: the inner radius drops the near-disk exclusion and the
        // outer radius truncates instead of rounding up.
        Driver::Wrong => {
            let view = u32::from(config.view_distance());
            let far = u32::from(config.far_multiplier());
            (view / 4, view * far / 4)
        }
    }
}

fn charge_of(step: LodStep) -> u32 {
    match DRIVER {
        Driver::Provider => lod::static_charge_bytes(step),
        // Wrong: one byte per job, so any leftover frame budget admits
        // more jobs than full static charges allow.
        Driver::Wrong => 1,
    }
}

/// The wrong selection state: it queues in scan order, ignores the
/// disabled flag, bills one byte per job and accepts every drained result
/// regardless of epoch or seed/config generation.
struct WrongState {
    epoch: SessionEpoch,
    generation: u64,
    config: LodConfig,
    pending: Vec<TilePos>,
    in_flight: Vec<TilePos>,
    retained: Vec<TilePos>,
    next_job_id: u64,
}

// --- shared fixtures ---

fn far_config(
    enabled: bool,
    view_distance: u8,
    far_multiplier: u8,
    bytes_per_frame: u32,
) -> LodConfig {
    LodConfig::try_new(
        enabled,
        view_distance,
        far_multiplier,
        LodStep::Four,
        bytes_per_frame,
    )
    .expect("checked lod config")
}

fn cheb(tile: TilePos, center: TilePos) -> i64 {
    let dx = i64::from(tile.x()) - i64::from(center.x());
    let dz = i64::from(tile.z()) - i64::from(center.z());
    dx.abs().max(dz.abs())
}

/// What one driven selection frame observably did.
struct FrameOutcome {
    dispatched: u32,
    accepted: u32,
    stale_released: u32,
    submitted: Vec<TilePos>,
}

/// Read-only observation wrapper around the accepted deterministic port
/// double: it records every submitted job identity and every drained
/// result key beside the double's own bounded accounting, so the named
/// cases assert exact tile and key identities without a second port
/// implementation.
struct RecordingPort {
    inner: PreparationFaucet,
    submitted_tiles: Vec<TilePos>,
    submitted_keys: Vec<PreparedResourceKey>,
    drained: Vec<PreparedResourceKey>,
}

impl RecordingPort {
    fn new(limits: ClientLimits) -> Self {
        Self {
            inner: PreparationFaucet::new(limits),
            submitted_tiles: Vec::new(),
            submitted_keys: Vec::new(),
            drained: Vec::new(),
        }
    }

    fn advance(&mut self, count: usize) -> usize {
        self.inner.advance(count)
    }
}

impl PreparationPort for RecordingPort {
    fn try_submit(
        &mut self,
        job: PreparationJob,
    ) -> Result<PreparationTicket, RejectedPreparation> {
        let key = *job.key();
        let tile = match job.payload() {
            PreparationPayload::Far(far) => TilePos::new(far.tile()[0], far.tile()[1]),
            _ => TilePos::new(0, 0),
        };
        let outcome = self.inner.try_submit(job);
        if outcome.is_ok() {
            self.submitted_tiles.push(tile);
            self.submitted_keys.push(key);
        }
        outcome
    }

    fn poll_ready(&mut self) -> Option<PreparationResult> {
        let result = self.inner.poll_ready()?;
        self.drained.push(*result.key());
        Some(result)
    }

    fn invalidate(&mut self, epoch: SessionEpoch) -> InvalidationReport {
        self.inner.invalidate(epoch)
    }
}

/// The driven selection: the real provider, or the wrong stand-in.
enum Inner {
    Provider { selection: LodSelection },
    Wrong(WrongState),
}

/// The driver-neutral selection harness the named cases script.
struct Harness {
    port: RecordingPort,
    params: Arc<WorldgenParams>,
    config: LodConfig,
    inner: Inner,
}

fn new_harness(epoch_value: u64, generation: u64, config: LodConfig) -> Harness {
    let epoch = SessionEpoch::try_new(epoch_value).expect("epoch");
    let inner = match DRIVER {
        Driver::Provider => Inner::Provider {
            selection: LodSelection::try_new(epoch, Dimension::OVERWORLD, config, generation)
                .expect("checked selection"),
        },
        Driver::Wrong => Inner::Wrong(WrongState {
            epoch,
            generation,
            config,
            pending: Vec::new(),
            in_flight: Vec::new(),
            retained: Vec::new(),
            next_job_id: 1,
        }),
    };
    Harness {
        port: RecordingPort::new(ClientLimits::try_new().expect("frozen limits")),
        params: fixture_params(),
        config,
        inner,
    }
}

/// Submits one far job through the recording port for the wrong driver.
fn submit_one(
    port: &mut RecordingPort,
    params: &Arc<WorldgenParams>,
    epoch: SessionEpoch,
    generation: u64,
    step: LodStep,
    next_job_id: &mut u64,
    tile: TilePos,
) -> bool {
    let key = PreparedResourceKey::try_new(
        epoch,
        Dimension::OVERWORLD,
        TerrainKey::LodTile(tile),
        generation,
        generation,
        *next_job_id,
    )
    .expect("checked far key");
    let request = OwnedLodRequest::try_new(Arc::clone(params), [tile.x(), tile.z()], step)
        .expect("checked far request");
    let job =
        PreparationJob::try_new(key, PreparationPayload::Far(request)).expect("paired far job");
    if port.try_submit(job).is_ok() {
        *next_job_id += 1;
        true
    } else {
        false
    }
}

fn queue_ring(harness: &mut Harness, center: TilePos) -> usize {
    match &mut harness.inner {
        Inner::Provider { selection } => selection.queue_ring(center),
        Inner::Wrong(state) => {
            // The wrong stand-in scans the real band bounds; its ring
            // wrongness lives in the `ring_of` stand-in the geometry cases
            // assert directly.
            let inner = u32::from(state.config.view_distance()) / 4 + 1;
            let chunks =
                u32::from(state.config.view_distance()) * u32::from(state.config.far_multiplier());
            let outer = chunks.div_ceil(4);
            let mut queued = 0usize;
            for dz in -(outer as i64)..=(outer as i64) {
                for dx in -(outer as i64)..=(outer as i64) {
                    if dx.abs().max(dz.abs()) < i64::from(inner) {
                        continue;
                    }
                    let tile = TilePos::new(center.x() + dx as i32, center.z() + dz as i32);
                    if state.pending.contains(&tile)
                        || state.in_flight.contains(&tile)
                        || state.retained.contains(&tile)
                    {
                        continue;
                    }
                    state.pending.push(tile);
                    queued += 1;
                }
            }
            queued
        }
    }
}

fn frame(harness: &mut Harness, center: TilePos) -> FrameOutcome {
    let mark = harness.port.submitted_tiles.len();
    match &mut harness.inner {
        Inner::Provider { selection } => {
            let report = selection
                .dispatch_frame(&mut harness.port, center, &harness.params)
                .expect("checked selection frame");
            FrameOutcome {
                dispatched: report.dispatched(),
                accepted: report.accepted_results(),
                stale_released: report.stale_released(),
                submitted: harness.port.submitted_tiles[mark..].to_vec(),
            }
        }
        Inner::Wrong(state) => {
            let mut accepted = 0u32;
            while let Some(result) = harness.port.poll_ready() {
                let key = *result.key();
                // Wrong: every drained result is admitted; neither an old
                // epoch nor an old seed/config generation is released.
                let TerrainKey::LodTile(tile) = *key.key() else {
                    continue;
                };
                accepted += 1;
                state.in_flight.retain(|wanted| *wanted != tile);
                state.retained.push(tile);
            }
            let mut dispatched = 0u32;
            let mut spent = 0u32;
            let allowance = state.config.bytes_per_frame();
            while !state.pending.is_empty() {
                // Wrong: a one-byte charge, so budget plus one keeps
                // admitting jobs, in queued scan order.
                if spent + charge_of(LodStep::Four) > allowance {
                    break;
                }
                let tile = state.pending[0];
                let step = state.config.step();
                let epoch = state.epoch;
                let generation = state.generation;
                if submit_one(
                    &mut harness.port,
                    &harness.params,
                    epoch,
                    generation,
                    step,
                    &mut state.next_job_id,
                    tile,
                ) {
                    state.pending.remove(0);
                    state.in_flight.push(tile);
                    dispatched += 1;
                    spent += charge_of(LodStep::Four);
                } else {
                    break;
                }
            }
            FrameOutcome {
                dispatched,
                accepted,
                stale_released: 0,
                submitted: harness.port.submitted_tiles[mark..].to_vec(),
            }
        }
    }
}

fn advance(harness: &mut Harness, count: usize) -> usize {
    harness.port.advance(count)
}

fn regenerate(harness: &mut Harness, generation: u64) {
    match &mut harness.inner {
        Inner::Provider { selection } => selection.regenerate(generation),
        // Wrong: only the recorded generation moves; nothing is re-queued
        // and nothing already tracked is invalidated.
        Inner::Wrong(state) => state.generation = generation,
    }
}

fn rebase_epoch(harness: &mut Harness, epoch_value: u64) {
    let epoch = SessionEpoch::try_new(epoch_value).expect("epoch");
    match &mut harness.inner {
        // A reset builds a fresh epoch-scoped selection over the same port.
        Inner::Provider { selection } => {
            let generation = selection.generation();
            let config = harness.config;
            *selection = LodSelection::try_new(epoch, Dimension::OVERWORLD, config, generation)
                .expect("checked epoch selection");
        }
        // Wrong: only the recorded epoch moves; old-epoch results stay
        // admissible.
        Inner::Wrong(state) => state.epoch = epoch,
    }
}

fn drop_outside(harness: &mut Harness, center: TilePos) -> usize {
    match &mut harness.inner {
        Inner::Provider { selection } => selection.remove_out_of_ring(center),
        Inner::Wrong(state) => {
            let (inner, outer) = ring_of(&state.config);
            let out_of_band = |tile: &TilePos| {
                let distance = cheb(*tile, center);
                distance < i64::from(inner) || distance > i64::from(outer)
            };
            let before = state.pending.len() + state.in_flight.len() + state.retained.len();
            state.pending.retain(|tile| !out_of_band(tile));
            state.in_flight.retain(|tile| !out_of_band(tile));
            state.retained.retain(|tile| !out_of_band(tile));
            before - (state.pending.len() + state.in_flight.len() + state.retained.len())
        }
    }
}

fn tracks(harness: &Harness, tile: TilePos) -> bool {
    match &harness.inner {
        Inner::Provider { selection } => selection.tracks(tile),
        Inner::Wrong(state) => {
            state.pending.contains(&tile)
                || state.in_flight.contains(&tile)
                || state.retained.contains(&tile)
        }
    }
}

fn pending(harness: &Harness) -> usize {
    match &harness.inner {
        Inner::Provider { selection } => selection.pending_len(),
        Inner::Wrong(state) => state.pending.len(),
    }
}

fn in_flight(harness: &Harness) -> usize {
    match &harness.inner {
        Inner::Provider { selection } => selection.in_flight_len(),
        Inner::Wrong(state) => state.in_flight.len(),
    }
}

fn retained(harness: &Harness) -> usize {
    match &harness.inner {
        Inner::Provider { selection } => selection.retained_len(),
        Inner::Wrong(state) => state.retained.len(),
    }
}

// --- the five named cases ---

/// `lod::no_near_overlap`: the far ring starts strictly outside the near
/// mesh disk, never publishes a near-section identity, and carries the
/// seed/config generation as both generation and content revision.
#[test]
fn no_near_overlap() {
    // View distance 8, multiplier 3: inner radius floor(8/4)+1 = 3, outer
    // radius ceil(8*3/4) = 6, band size 13*13 - 5*5 = 144 tiles.
    let config = far_config(true, 8, 3, 144 * charge_of(LodStep::Four));
    let mut harness = new_harness(1, 7, config);
    let center = TilePos::new(0, 0);
    assert_eq!(queue_ring(&mut harness, center), 144, "exact band size");
    let outcome = frame(&mut harness, center);
    assert_eq!(
        outcome.dispatched, 144,
        "a covering allowance dispatches the whole band"
    );
    assert_eq!(pending(&harness), 0);

    let (inner, outer) = ring_of(&harness.config);
    assert_eq!(inner, 3, "inner radius floor(view_distance/4)+1");
    assert_eq!(
        outer, 6,
        "outer radius ceil(view_distance*far_multiplier/4)"
    );
    let mut distances = harness
        .port
        .submitted_tiles
        .iter()
        .map(|tile| cheb(*tile, center))
        .collect::<Vec<_>>();
    distances.sort_unstable();
    assert_eq!(*distances.first().expect("band"), 3, "lower edge admitted");
    assert_eq!(*distances.last().expect("band"), 6, "upper edge admitted");

    // The shell's nearest covered block starts at inner * 64 blocks, at or
    // beyond the near mesh disk radius of view_distance * 16 blocks.
    assert!(
        distances.first().expect("band") * 64 >= i64::from(8) * 16,
        "the far band never covers the near mesh disk"
    );

    let near_key = TerrainKey::Section(
        mornlea_client_core::preparation::SectionKey::try_new(ChunkPos::new(0, 0), 0)
            .expect("section"),
    );
    for key in &harness.port.submitted_keys {
        assert!(
            !matches!(key.key(), TerrainKey::Section(_)),
            "a far tile is never published under a section identity"
        );
        assert_ne!(*key.key(), near_key);
        assert_eq!(key.generation(), 7, "seed/config generation");
        assert_eq!(key.content_revision(), 7, "procedural content revision");
    }
    let mut job_ids = harness
        .port
        .submitted_keys
        .iter()
        .map(|key| key.job_id().get())
        .collect::<Vec<_>>();
    job_ids.sort_unstable();
    let expected = (1u64..=144).collect::<Vec<_>>();
    assert_eq!(job_ids, expected, "nonzero job identities never repeat");
}

/// `lod::negative_tile_and_radius_edges`: arithmetic-shift tile centers are
/// exact for negative chunks, the closed band includes both radius edges,
/// and moving the center removes exactly the out-of-band tiles — including
/// the tile that crossed into the near mesh disk.
#[test]
fn negative_tile_and_radius_edges() {
    // Arithmetic-shift centers floor negative chunk coordinates onto the
    // tile grid; rounding division would lift them toward zero.
    assert_eq!(
        center_of(ChunkPos::new(-1, -1)),
        TilePos::new(-1, -1),
        "chunk -1 floors into tile -1"
    );
    assert_eq!(center_of(ChunkPos::new(-4, 3)), TilePos::new(-1, 0));
    assert_eq!(center_of(ChunkPos::new(-5, -8)), TilePos::new(-2, -2));
    assert_eq!(center_of(ChunkPos::new(7, 7)), TilePos::new(1, 1));
    assert_eq!(center_of(ChunkPos::new(3, 3)), TilePos::new(0, 0));
    assert_eq!(
        center_of(ChunkPos::new(i32::MIN, i32::MIN)),
        TilePos::new(i32::MIN >> 2, i32::MIN >> 2),
        "the extreme chunk column is exact"
    );
    for x in -17..=17 {
        let chunk = ChunkPos::new(x, -x);
        let tile = center_of(chunk);
        assert_eq!(tile.x(), x.div_euclid(4), "floor semantics on x");
        assert_eq!(tile.z(), (-x).div_euclid(4), "floor semantics on z");
    }

    // Closed radius edges over checked configurations, including the
    // minimum legal geometry where the band collapses to a single radius.
    let edge_table = [
        ((2u8, 2u8), (1u32, 1u32)),
        ((2, 3), (1, 2)),
        ((8, 3), (3, 6)),
        ((9, 3), (3, 7)),
        ((5, 3), (2, 4)),
        ((63, 2), (16, 32)),
        ((64, 8), (17, 128)),
    ];
    for ((view, far), want) in edge_table {
        let config = far_config(true, view, far, 1 << 20);
        assert_eq!(ring_of(&config), want, "view {view} multiplier {far}");
    }

    // A negative center queues the exact negative band: the inner-edge and
    // outer-edge tiles are present, the near-disk center tile is not.
    let config = far_config(true, 2, 3, 24 * charge_of(LodStep::Four));
    let mut harness = new_harness(1, 7, config);
    let center = center_of(ChunkPos::new(-1, -1));
    assert_eq!(queue_ring(&mut harness, center), 24);
    let outcome = frame(&mut harness, center);
    assert_eq!(outcome.dispatched, 24);
    assert!(
        outcome.submitted.contains(&TilePos::new(-2, -2)),
        "inner edge"
    );
    assert!(
        outcome.submitted.contains(&TilePos::new(-3, -3)),
        "outer edge"
    );
    assert!(
        !outcome.submitted.contains(&TilePos::new(-1, -1)),
        "the near-disk center tile is never queued"
    );
    for tile in &harness.port.submitted_tiles {
        let distance = cheb(*tile, center);
        assert!(
            (1..=2).contains(&distance),
            "tile {tile:?} stays inside the closed band"
        );
    }

    // Moving the center three tiles north drops the rows that left the
    // band, the new near-disk center tile, and nothing else.
    let removed = drop_outside(&mut harness, TilePos::new(-1, -3));
    assert_eq!(removed, 11, "only the out-of-band rows drop");
    assert!(
        !tracks(&harness, TilePos::new(-1, -3)),
        "the near-entering tile is released"
    );
    assert!(tracks(&harness, TilePos::new(-3, -3)), "outer edge kept");
    assert_eq!(in_flight(&harness), 13, "the surviving band is retained");
    assert_eq!(
        queue_ring(&mut harness, TilePos::new(-1, -3)),
        11,
        "the newly entered rows requeue"
    );
}

/// `lod::disabled_has_no_jobs`: an out-of-range configuration is a typed
/// rejection — never a silent clamp — and a disabled configuration selects
/// no jobs at all.
#[test]
fn disabled_has_no_jobs() {
    use mornlea_client_core::contracts::ClientError;

    // Checked ranges: view distance 2..=64, far multiplier 2..=8, steps
    // {2,4,8}; every out-of-range value is a typed rejection.
    assert_eq!(
        LodConfig::try_new(true, 1, 3, LodStep::Four, 1024).unwrap_err(),
        ClientError::InvalidInput
    );
    assert_eq!(
        LodConfig::try_new(true, 65, 3, LodStep::Four, 1024).unwrap_err(),
        ClientError::InvalidInput
    );
    assert_eq!(
        LodConfig::try_new(true, 8, 1, LodStep::Four, 1024).unwrap_err(),
        ClientError::InvalidInput
    );
    assert_eq!(
        LodConfig::try_new(true, 8, 9, LodStep::Four, 1024).unwrap_err(),
        ClientError::InvalidInput
    );
    assert_eq!(LodStep::try_new(0).unwrap_err(), ClientError::InvalidInput);
    assert_eq!(LodStep::try_new(3).unwrap_err(), ClientError::InvalidInput);
    assert_eq!(LodStep::try_new(16).unwrap_err(), ClientError::InvalidInput);
    for view in [2u8, 64] {
        for far in [2u8, 8] {
            assert!(LodConfig::try_new(true, view, far, LodStep::Four, 1024).is_ok());
        }
    }
    for step in [2u8, 4, 8] {
        assert!(LodStep::try_new(step).is_ok());
    }

    // A disabled configuration selects nothing: no queued ring, no
    // submitted jobs, no tracked tiles.
    let disabled = far_config(false, 2, 3, 24 * charge_of(LodStep::Four));
    let mut harness = new_harness(1, 7, disabled);
    assert_eq!(queue_ring(&mut harness, TilePos::new(0, 0)), 0);
    let outcome = frame(&mut harness, TilePos::new(0, 0));
    assert_eq!(outcome.dispatched, 0, "disabled selects no jobs");
    assert!(outcome.submitted.is_empty(), "nothing reaches the port");
    assert_eq!(pending(&harness), 0);

    // The same geometry enabled selects the whole band, so the empty
    // result above is the disabled branch and not an empty ring.
    let enabled = far_config(true, 2, 3, 24 * charge_of(LodStep::Four));
    let mut harness = new_harness(1, 7, enabled);
    assert_eq!(queue_ring(&mut harness, TilePos::new(0, 0)), 24);
    let outcome = frame(&mut harness, TilePos::new(0, 0));
    assert_eq!(outcome.dispatched, 24);
}

/// `lod::budget_plus_one_retained`: each dispatched job is precharged its
/// full static maximum before anything dispatches, so an allowance of N
/// charges plus one byte still retains job N+1 — never a partial dispatch.
#[test]
fn budget_plus_one_retained() {
    // The static maximum is the worst-case quad count 3N^2+2N over the
    // N = 64/step window grid times the 20-byte quad width; the step-2
    // worst case is exactly the native far scratch stage capacity.
    assert_eq!(charge_of(LodStep::Two), 62720);
    assert_eq!(charge_of(LodStep::Four), 16000);
    assert_eq!(charge_of(LodStep::Eight), 4160);
    assert_eq!(
        charge_of(LodStep::Two),
        u32::try_from(mornlea_client_core::preparation::PreparedGeometry::MAX_LOD_QUADS * 20)
            .expect("bound")
    );

    // Two full charges plus one byte: exactly two jobs dispatch, in the
    // deterministic distance-then-x-then-z order, and the remainder is
    // retained pending.
    let config = far_config(true, 2, 3, 2 * charge_of(LodStep::Four) + 1);
    let mut harness = new_harness(1, 7, config);
    let center = TilePos::new(0, 0);
    assert_eq!(queue_ring(&mut harness, center), 24);
    let outcome = frame(&mut harness, center);
    assert_eq!(
        outcome.dispatched, 2,
        "budget plus one retains the third job"
    );
    assert_eq!(
        outcome.submitted,
        vec![TilePos::new(-1, -1), TilePos::new(-1, 0)],
        "distance, then x, then z"
    );
    assert_eq!(pending(&harness), 22, "the remainder is kept");
    assert_eq!(in_flight(&harness), 2);

    // The next frame completes the in-flight pair and dispatches the next
    // two in the same total order.
    assert_eq!(advance(&mut harness, 2), 2);
    let outcome = frame(&mut harness, center);
    assert_eq!(outcome.accepted, 2);
    assert_eq!(outcome.stale_released, 0);
    assert_eq!(outcome.dispatched, 2);
    assert_eq!(
        outcome.submitted,
        vec![TilePos::new(-1, 1), TilePos::new(0, -1)],
        "the total order continues across frames"
    );

    // One charge plus one byte admits exactly one job; an allowance below
    // one full charge admits none — a job is never partially dispatched.
    let config = far_config(true, 2, 3, charge_of(LodStep::Four) + 1);
    let mut harness = new_harness(1, 7, config);
    assert_eq!(queue_ring(&mut harness, center), 24);
    let outcome = frame(&mut harness, center);
    assert_eq!(outcome.dispatched, 1, "a single full charge fits");
    assert_eq!(pending(&harness), 23);
    let config = far_config(true, 2, 3, charge_of(LodStep::Four) - 1);
    let mut harness = new_harness(1, 7, config);
    assert_eq!(queue_ring(&mut harness, center), 24);
    let outcome = frame(&mut harness, center);
    assert_eq!(outcome.dispatched, 0, "no partial charge ever dispatches");
    assert_eq!(pending(&harness), 24);
}

/// `lod::old_seed_or_epoch_is_stale`: results of an old seed/config
/// generation or an old epoch are released exactly once and never
/// retained; only the current generation's results are kept.
#[test]
fn old_seed_or_epoch_is_stale() {
    // An old seed/config generation: the whole band dispatches under
    // generation 7, the seed moves to 8, and every old result releases
    // exactly once while the band requeues and redispatches.
    let config = far_config(true, 2, 3, 24 * charge_of(LodStep::Four));
    let mut harness = new_harness(1, 7, config);
    let center = TilePos::new(0, 0);
    assert_eq!(queue_ring(&mut harness, center), 24);
    let outcome = frame(&mut harness, center);
    assert_eq!(outcome.dispatched, 24);
    assert_eq!(advance(&mut harness, 24), 24);
    regenerate(&mut harness, 8);
    let outcome = frame(&mut harness, center);
    assert_eq!(outcome.stale_released, 24, "the old generation releases");
    assert_eq!(outcome.accepted, 0, "a stale result is never retained");
    assert_eq!(retained(&harness), 0);
    assert_eq!(pending(&harness), 0, "the whole band redispatched");
    assert_eq!(in_flight(&harness), 24, "the new generation owns the band");

    // The new generation's own results are retained, and the old ones
    // never release a second time.
    assert_eq!(advance(&mut harness, 24), 24);
    let outcome = frame(&mut harness, center);
    assert_eq!(outcome.accepted, 24, "current results are retained");
    assert_eq!(outcome.stale_released, 0);
    assert_eq!(retained(&harness), 24);
    assert_eq!(
        harness.port.drained[0].generation(),
        7,
        "the drained stale identity is preserved"
    );
    let outcome = frame(&mut harness, center);
    assert_eq!(outcome.stale_released, 0, "no result releases twice");
    assert_eq!(outcome.accepted, 0);

    // An old epoch: results submitted under epoch 1 are stale for the
    // epoch-2 selection that shares the port, and release exactly once.
    let mut harness = new_harness(1, 8, far_config(true, 2, 3, 24 * charge_of(LodStep::Four)));
    assert_eq!(queue_ring(&mut harness, center), 24);
    assert_eq!(frame(&mut harness, center).dispatched, 24);
    assert_eq!(advance(&mut harness, 24), 24);
    rebase_epoch(&mut harness, 2);
    queue_ring(&mut harness, center);
    let outcome = frame(&mut harness, center);
    assert_eq!(outcome.stale_released, 24, "the old epoch releases");
    assert_eq!(outcome.accepted, 0);
    assert_eq!(outcome.dispatched, 24, "the new epoch selects its own jobs");
}
