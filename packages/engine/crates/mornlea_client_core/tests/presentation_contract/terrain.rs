//! Terrain family publication contract cases.
//!
//! The named cases drive the terrain family integrator over the two real
//! preparation owners — the bounded queue arena and the far-tile selection —
//! plus the confirmed mirror: near and far records coexisting without the
//! far band ever covering the near disk, payload-derived material and light
//! summaries, section replacement through an ordered remove before the
//! reused full key, the stale-mesh reset across epochs and generations,
//! exact full-range negative tiles, and the family record and frame byte
//! caps preserving the prior output. The red run of this table drove the
//! deliberately wrong `Wrong` publisher — the only behavior that existed
//! while the provider file was the contract landing's doc-only stub — and
//! its recorded wrong behavior stays executable by flipping `DRIVER`.

use std::collections::BTreeMap;
use std::sync::Arc;

use mornlea_client_core::contracts::{
    ClientError, ClientLimits, ClientWorkBudget, ConfirmedRevision, FAMILY_TERRAIN, FamilyKey,
    FamilyOperation, PreparationPort, RecordHeader, SessionEpoch, SessionPhase,
};
use mornlea_client_core::input::InputProjectionState;
use mornlea_client_core::prediction::PlayerProjectionState;
use mornlea_client_core::preparation::lod;
use mornlea_client_core::preparation::lod::LodSelection;
use mornlea_client_core::preparation::{
    InvalidationReport, LightSummary, LodConfig, LodStep, OwnedLodRequest, OwnedMeshView,
    PreparationJob, PreparationPayload, PreparationQueue, PreparationResult, PreparationTicket,
    PreparedResourceKey, RejectedPreparation, SectionKey, TerrainKey, TerrainVisibility, TilePos,
};
use mornlea_client_core::presentation::family_terrain::TerrainPublisher;
use mornlea_client_core::presentation::frame::{
    FamilyFrame, FamilyRecords, PresentationFrame, TerrainMaterial, TerrainRecord,
};
use mornlea_client_core::presentation::{
    AudioProjectionState, DiagnosticProjectionState, ErrorClassCounters, LifecycleProjectionState,
    MovementIntent, Pose, ProducerIdentity, ProjectionView, QueueCounters,
};
use mornlea_client_core::session::{ConfirmedMirror, ConfirmedMirrorParts, WorldConfirmed};
use mornlea_domain::{ChunkPos, Dimension};
use mornlea_engine::native::contracts::mesh::{MeshModel, MeshRegistry, MeshRegistryEntry};
use mornlea_engine::native::contracts::world::WorldgenParams;
use mornlea_protocol::MIN_Y;

use crate::preparation_port::fixture_params;

/// Which publisher the named cases drive. `Wrong` is the deliberately
/// wrong stand-in the behavioral reds ran against: a truncating tile
/// center, no mirror or ring identity checks, no removals, no caps and the
/// neutral material and light everywhere.
enum Driver {
    Provider,
    Wrong,
}

/// The implementation the named cases run against.
const DRIVER: Driver = Driver::Provider;

// --- shared fixtures ---

fn frozen_limits() -> ClientLimits {
    ClientLimits::try_new().expect("frozen limits")
}

/// A legal tighter configuration: the frozen limits with only the family
/// record count and frame byte cap replaced.
fn limits_with(family_records: usize, frame_bytes: usize) -> ClientLimits {
    let base = frozen_limits();
    ClientLimits::try_new_with(
        base.queued_input_events(),
        base.inbound_observations(),
        base.inbound_bytes(),
        base.outbound_commands(),
        base.outbound_bytes(),
        base.prediction_journal(),
        base.message_work(),
        base.mesh_work(),
        base.preparation_results(),
        base.preparation_bytes(),
        family_records,
        frame_bytes,
    )
    .expect("tighter limits")
}

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

/// The per-job static precharge of the fixture step.
fn charge() -> u32 {
    lod::static_charge_bytes(LodStep::Four)
}

/// The shared near-mesh registry fixture: stone, water, a cutout plant and
/// an emissive torch, so the material and light derivations have real
/// registry facts to read.
fn rich_registry() -> Arc<MeshRegistry> {
    let entries = [
        MeshRegistryEntry {
            id: 1,
            opaque: true,
            emission: 0,
            material: [1; 6],
            fluid_height: 0,
            light_attenuation: 0,
            block_top_raw: 0,
            model: MeshModel::Default,
        },
        MeshRegistryEntry {
            id: 2,
            opaque: false,
            emission: 0,
            material: [2; 6],
            fluid_height: 8,
            light_attenuation: 2,
            block_top_raw: 0,
            model: MeshModel::Default,
        },
        MeshRegistryEntry {
            id: 3,
            opaque: false,
            emission: 0,
            material: [3; 6],
            fluid_height: 0,
            light_attenuation: 0,
            block_top_raw: 0,
            model: MeshModel::Default,
        },
        MeshRegistryEntry {
            id: 4,
            opaque: true,
            emission: 14,
            material: [4; 6],
            fluid_height: 0,
            light_attenuation: 0,
            block_top_raw: 0,
            model: MeshModel::Default,
        },
    ];
    MeshRegistry::try_new(&entries, &[0, 0, 0, 0], 0, 5)
        .expect("checked registry")
        .into()
}

/// The driver-neutral harness: the real queue, the real selection, the
/// real publisher and the staged mirror, beside the shared job counter.
struct Harness {
    publisher: TerrainPublisher,
    queue: PreparationQueue,
    selection: LodSelection,
    mirror: ConfirmedMirror,
    epoch: SessionEpoch,
    frame_revision: u64,
    frame_index: u64,
    limits: ClientLimits,
    params: Arc<WorldgenParams>,
    registry: Arc<MeshRegistry>,
    job_id: u64,
}

fn new_harness(
    epoch_value: u64,
    generation: u64,
    config: LodConfig,
    limits: ClientLimits,
) -> Harness {
    let epoch = SessionEpoch::try_new(epoch_value).expect("epoch");
    Harness {
        publisher: TerrainPublisher::try_new().expect("publisher"),
        queue: PreparationQueue::new(limits),
        selection: LodSelection::try_new(epoch, Dimension::OVERWORLD, config, generation)
            .expect("checked selection"),
        mirror: ConfirmedMirror::try_new(ConfirmedMirrorParts::pending(epoch))
            .expect("pending mirror"),
        epoch,
        frame_revision: 1,
        frame_index: 0,
        limits,
        params: fixture_params(),
        registry: rich_registry(),
        job_id: 0,
    }
}

/// Rebuilds the confirmed mirror with exactly the held chunk columns. This
/// is the committed effect of the snapshot, delta and forget events the
/// real mirror provider commits.
fn set_mirror(h: &mut Harness, chunks: &[(Dimension, i32, i32, u64)]) {
    let mut world = WorldConfirmed::try_new(BTreeMap::new()).expect("world");
    for (dimension, x, z, revision) in chunks {
        world = world.with_chunk(*dimension, ChunkPos::new(*x, *z), *revision);
    }
    h.mirror = ConfirmedMirror::try_new(ConfirmedMirrorParts {
        epoch: h.epoch,
        revision: ConfirmedRevision::new(h.frame_revision.max(1)),
        phase: SessionPhase::Admitted,
        world: Some(world),
        actors: None,
        inventory: None,
        world_ui: None,
    })
    .expect("checked mirror");
}

/// One owned near payload filled with a single registry id.
fn near_payload(h: &Harness, section: u8, fill: u16) -> OwnedMeshView {
    OwnedMeshView::try_new(
        Box::new([fill; 110592]),
        [false; 9],
        Box::new([[0i16; 256]; 9]),
        MIN_Y + i32::from(section) * 16,
        Arc::clone(&h.registry),
    )
    .expect("checked mesh view")
}

/// One complete paired near job for one chunk section at a content
/// revision, with a fresh family-issued job identity.
fn near_job(
    h: &mut Harness,
    chunk: ChunkPos,
    section: u8,
    revision: u64,
    fill: u16,
) -> PreparationJob {
    h.job_id += 1;
    let key = PreparedResourceKey::try_new(
        h.epoch,
        Dimension::OVERWORLD,
        TerrainKey::Section(SectionKey::try_new(chunk, section).expect("section")),
        1,
        revision,
        h.job_id,
    )
    .expect("checked near key");
    let payload = near_payload(h, section, fill);
    PreparationJob::try_new(key, PreparationPayload::Near(payload)).expect("paired near job")
}

/// One complete paired far job of an arbitrary generation, submitted
/// outside the selection to stage rogue far work.
fn far_job(h: &mut Harness, tile: TilePos, generation: u64) -> PreparationJob {
    h.job_id += 1;
    let key = PreparedResourceKey::try_new(
        h.epoch,
        Dimension::OVERWORLD,
        TerrainKey::LodTile(tile),
        generation,
        generation,
        h.job_id,
    )
    .expect("checked far key");
    let request =
        OwnedLodRequest::try_new(Arc::clone(&h.params), [tile.x(), tile.z()], LodStep::Four)
            .expect("checked far request");
    PreparationJob::try_new(key, PreparationPayload::Far(request)).expect("paired far job")
}

/// The driver-switched near admission: the real publisher records the
/// payload summary and delegates to the real queue; the wrong driver
/// submits directly and records nothing.
fn admit_near(
    h: &mut Harness,
    job: PreparationJob,
) -> Result<PreparationTicket, RejectedPreparation> {
    match DRIVER {
        Driver::Provider => h.publisher.admit_near(&mut h.queue, job),
        Driver::Wrong => h.queue.try_submit(job),
    }
}

/// Builds one near job and admits it through the driver-switched path.
fn admit_section(
    h: &mut Harness,
    chunk: ChunkPos,
    section: u8,
    revision: u64,
    fill: u16,
    label: &str,
) {
    let job = near_job(h, chunk, section, revision, fill);
    admit_near(h, job).expect(label);
}

/// The test-side recording drain port for the wrong driver: identical
/// delegation, with the drained identities kept for the wrong publisher.
struct DrainProbe<'a> {
    queue: &'a mut PreparationQueue,
    drained: Vec<(PreparedResourceKey, bool)>,
}

impl PreparationPort for DrainProbe<'_> {
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

/// The wrong publisher: a truncating tile center, no mirror or ring
/// identity checks, no removals, no caps, and the neutral material and
/// light on every upsert it emits.
fn wrong_publish(
    h: &mut Harness,
    center: ChunkPos,
    budget: ClientWorkBudget,
) -> Result<Vec<TerrainRecord>, ClientError> {
    let center_tile = TilePos::new(center.x() / 4, center.z() / 4);
    h.selection.remove_out_of_ring(center_tile);
    h.selection.queue_ring(center_tile);
    h.queue.work(budget);
    let drained = {
        let mut probe = DrainProbe {
            queue: &mut h.queue,
            drained: Vec::new(),
        };
        h.selection
            .dispatch_frame(&mut probe, center_tile, &h.params)
            .expect("wrong selection frame");
        probe.drained
    };
    let mut records = Vec::new();
    for (key, delivered) in drained {
        if !delivered {
            continue;
        }
        let visibility = match key.key() {
            TerrainKey::Section(_) => TerrainVisibility::Near,
            TerrainKey::LodTile(_) => TerrainVisibility::Far,
        };
        let header = RecordHeader::try_new(
            h.epoch,
            ConfirmedRevision::new(h.frame_revision),
            None,
            FamilyOperation::Upsert,
        )
        .expect("wrong header");
        records.push(
            TerrainRecord::try_new(
                header,
                key.dimension(),
                *key.key(),
                key.content_revision(),
                key.generation(),
                TerrainMaterial::Opaque,
                visibility,
                LightSummary::try_new(0, 0).expect("neutral light"),
                Some(key),
            )
            .expect("wrong record"),
        );
    }
    Ok(records)
}

/// The driver-switched publication of one terrain frame.
fn publish(
    h: &mut Harness,
    center: ChunkPos,
    budget: ClientWorkBudget,
) -> Result<Vec<TerrainRecord>, ClientError> {
    h.frame_index += 1;
    let input = InputProjectionState::try_new(1).expect("input state");
    let player = PlayerProjectionState::try_new(
        h.epoch,
        ConfirmedRevision::new(h.frame_revision),
        Pose::try_new([0.0; 3], 0.0, 0.0).expect("pose"),
        None,
        Vec::new(),
        None,
        None,
        MovementIntent::try_new(None, true).expect("movement"),
    )
    .expect("player state");
    let audio = AudioProjectionState::try_new().expect("audio state");
    let lifecycle = LifecycleProjectionState::try_new(1).expect("lifecycle state");
    let diagnostics = DiagnosticProjectionState::try_new(
        ProducerIdentity::try_new([0; 20], [0; 32]).expect("producer"),
        QueueCounters::try_new(0, 0, 0, 0, 0, 0, 0).expect("queue counters"),
        ErrorClassCounters::try_new(0, 0, 0, 0, 0, 0).expect("rejection counters"),
    )
    .expect("diagnostics state");
    let view = ProjectionView::try_new(
        &h.mirror,
        &[],
        &input,
        &player,
        &audio,
        &lifecycle,
        &diagnostics,
        h.epoch,
        ConfirmedRevision::new(h.frame_revision),
        h.frame_index,
        &h.limits,
    )
    .expect("projection view");
    match DRIVER {
        Driver::Provider => h.publisher.publish(
            &view,
            &mut h.queue,
            &mut h.selection,
            center,
            &h.params,
            budget,
        ),
        Driver::Wrong => wrong_publish(h, center, budget),
    }
}

/// The work budget that drains everything the fixtures stage.
fn full_budget() -> ClientWorkBudget {
    ClientWorkBudget::try_new(0, 4096).expect("budget")
}

fn cheb(tile: TilePos, center: TilePos) -> i64 {
    let dx = i64::from(tile.x()) - i64::from(center.x());
    let dz = i64::from(tile.z()) - i64::from(center.z());
    dx.abs().max(dz.abs())
}

/// The minimal terrain-only frame's measured size, computed through the
/// accepted validator's own accounting.
fn minimal_frame_size(h: &Harness, records: &[TerrainRecord]) -> usize {
    PresentationFrame::try_new(
        h.epoch,
        ConfirmedRevision::new(h.frame_revision),
        h.frame_index,
        vec![
            FamilyFrame::try_new(
                FamilyKey::try_new(FAMILY_TERRAIN).expect("family key"),
                FamilyRecords::Terrain(records.to_vec()),
            )
            .expect("family frame"),
        ],
    )
    .expect("candidate frame")
    .validated_size()
    .expect("measured size")
}

/// Every upsert's complete resource reference resolves in the real arena.
fn assert_resources_resolve(h: &Harness, records: &[TerrainRecord]) {
    for record in records {
        if let Some(key) = record.resource() {
            assert!(
                h.queue.prepared_resource(key).is_ok(),
                "the referenced resource {key:?} is retained by the real arena"
            );
        }
    }
}

// --- the six named cases ---

/// `terrain::near_far_coexist_without_overlap`: one frame publishes near
/// section and far tile records together, every far tile stays in the
/// closed band strictly outside the near disk, a rogue far job inside the
/// near disk never publishes, and every reference resolves.
#[test]
fn near_far_coexist_without_overlap() {
    // View distance 2: the near disk is the center tile, the closed far
    // band is Chebyshev distance 1..=2, so 24 tiles.
    let mut h = new_harness(1, 7, far_config(true, 2, 3, 24 * charge()), frozen_limits());
    set_mirror(&mut h, &[(Dimension::OVERWORLD, 0, 0, 5)]);
    let center = ChunkPos::new(0, 0);
    let center_tile = lod::tile_from_chunk(center);

    // Frame one selects the ring; nothing has completed yet.
    let records = publish(&mut h, center, full_budget()).expect("first frame publishes");
    assert!(records.is_empty(), "the selection frame dispatches first");

    // A rogue far job inside the near disk — submitted outside the ring —
    // and a legitimate near section complete alongside the ring work.
    let rogue = far_job(&mut h, center_tile, 7);
    let rogue_key = *rogue.key();
    h.queue.try_submit(rogue).expect("rogue admission");
    admit_section(&mut h, center, 0, 5, 1, "near admission");

    let records = publish(&mut h, center, full_budget()).expect("coexisting frame publishes");
    let near: Vec<&TerrainRecord> = records
        .iter()
        .filter(|record| matches!(record.key(), TerrainKey::Section(_)))
        .collect();
    let far: Vec<&TerrainRecord> = records
        .iter()
        .filter(|record| matches!(record.key(), TerrainKey::LodTile(_)))
        .collect();
    assert_eq!(
        records.len(),
        25,
        "one near section beside the 24 far tiles"
    );
    assert_eq!(near.len(), 1, "exactly one near record");
    assert_eq!(far.len(), 24, "the full closed band");

    // Matching visibility and identity on both classes.
    for record in near.iter() {
        assert_eq!(*record.visibility(), TerrainVisibility::Near);
        assert_eq!(record.dimension(), Dimension::OVERWORLD);
        assert_eq!(record.content_revision(), 5, "the mirror's chunk revision");
        assert_eq!(record.generation(), 1, "the build generation");
        assert_eq!(
            *record.key(),
            TerrainKey::Section(SectionKey::try_new(center, 0).expect("section"))
        );
    }
    for record in far.iter() {
        assert_eq!(*record.visibility(), TerrainVisibility::Far);
        assert_eq!(record.generation(), 7, "the seed/config generation");
        assert_eq!(record.content_revision(), 7, "procedural content revision");
        let TerrainKey::LodTile(tile) = *record.key() else {
            panic!("a far record names a tile");
        };
        let distance = cheb(tile, center_tile);
        assert!(
            (1..=2).contains(&distance),
            "tile {tile:?} stays in the closed band outside the near disk"
        );
        assert_ne!(
            tile, center_tile,
            "the near-disk center tile never publishes"
        );
    }
    // The rogue far job's tile is the near-disk center: it released and
    // never published.
    assert!(
        records
            .iter()
            .all(|record| *record.key() != *rogue_key.key()),
        "an untracked far result never publishes"
    );
    assert!(
        h.queue.prepared_resource(&rogue_key).is_err(),
        "the rogue geometry is not retained"
    );
    assert_resources_resolve(&h, &records);
}

/// `terrain::fluid_material_and_light_summaries`: the material class and
/// light channels come from the real payload through the real registry —
/// a fluid section publishes `Water`, a cutout section `Cutout`, the block
/// channel is the brightest referenced emission and the sky channel is
/// full exactly when the section contains air.
#[test]
fn fluid_material_and_light_summaries() {
    let mut h = new_harness(
        1,
        7,
        far_config(false, 2, 3, 24 * charge()),
        frozen_limits(),
    );
    set_mirror(&mut h, &[(Dimension::OVERWORLD, 0, 0, 5)]);
    let chunk = ChunkPos::new(0, 0);
    // Sections 0..=4 filled with stone, water, plant, torch and air.
    admit_section(&mut h, chunk, 0, 5, 1, "stone admission");
    admit_section(&mut h, chunk, 1, 5, 2, "water admission");
    admit_section(&mut h, chunk, 2, 5, 3, "plant admission");
    admit_section(&mut h, chunk, 3, 5, 4, "torch admission");
    admit_section(&mut h, chunk, 4, 5, 0, "air admission");

    let records = publish(&mut h, chunk, full_budget()).expect("summary frame publishes");
    assert_eq!(records.len(), 5);
    let summary = |section: u8| {
        records
            .iter()
            .find(|record| {
                *record.key()
                    == TerrainKey::Section(SectionKey::try_new(chunk, section).expect("section"))
            })
            .unwrap_or_else(|| panic!("section {section} published"))
    };
    let stone = summary(0);
    assert_eq!(*stone.material(), TerrainMaterial::Opaque);
    assert_eq!(stone.light().sky(), 0, "no air in the stone section");
    assert_eq!(stone.light().block(), 0);
    let water = summary(1);
    assert_eq!(*water.material(), TerrainMaterial::Water, "fluid material");
    assert_eq!(water.light().sky(), 0, "water columns attenuate sky light");
    let plant = summary(2);
    assert_eq!(
        *plant.material(),
        TerrainMaterial::Cutout,
        "cutout material"
    );
    let torch = summary(3);
    assert_eq!(*torch.material(), TerrainMaterial::Opaque);
    assert_eq!(
        torch.light().block(),
        14,
        "the brightest referenced emission"
    );
    let air = summary(4);
    assert_eq!(air.light().sky(), 15, "air passes sky light fully");
    assert_resources_resolve(&h, &records);
}

/// `terrain::section_replacement`: a newer confirmed chunk revision
/// replaces the published mesh through one ordered remove before the same
/// full key is reused at the new revision, and the arena retains only the
/// replacement.
#[test]
fn section_replacement() {
    let mut h = new_harness(
        1,
        7,
        far_config(false, 2, 3, 24 * charge()),
        frozen_limits(),
    );
    let chunk = ChunkPos::new(2, 3);
    set_mirror(&mut h, &[(Dimension::OVERWORLD, 2, 3, 5)]);
    admit_section(&mut h, chunk, 2, 5, 1, "first admission");

    let first = publish(&mut h, chunk, full_budget()).expect("first mesh publishes");
    assert_eq!(first.len(), 1);
    assert_eq!(first[0].content_revision(), 5);
    let first_key = *first[0].resource().expect("referenced resource");
    assert_eq!(h.publisher.published_resources(), &[first_key]);

    // The confirmed chunk revision advances; the replacement mesh lands.
    set_mirror(&mut h, &[(Dimension::OVERWORLD, 2, 3, 6)]);
    admit_section(&mut h, chunk, 2, 6, 1, "replacement admission");
    let second = publish(&mut h, chunk, full_budget()).expect("replacement frame publishes");
    assert_eq!(second.len(), 2, "one remove and one reuse");
    assert_eq!(
        second[0].header().operation(),
        FamilyOperation::Remove,
        "the stale full key leaves publication first"
    );
    assert_eq!(
        second[0].resource(),
        None,
        "a removal references no resource"
    );
    assert_eq!(
        *second[0].key(),
        *second[1].key(),
        "the same full key is reused"
    );
    assert_eq!(second[1].content_revision(), 6, "the replacement revision");
    let second_key = *second[1].resource().expect("replacement resource");
    assert_ne!(first_key, second_key, "distinct job identities");
    assert_eq!(
        h.publisher.published_resources(),
        &[second_key],
        "only the replacement stays referenced"
    );
    assert!(
        h.queue.prepared_resource(&second_key).is_ok(),
        "the replacement geometry is retained"
    );
    assert_eq!(
        h.queue.prepared_resource(&first_key).unwrap_err(),
        ClientError::InvalidInput,
        "the superseded geometry released"
    );
}

/// `terrain::stale_mesh_reset`: an invalidated epoch releases the retained
/// mesh — never republished and typed stale at the arena — and a far
/// result of an old generation submitted outside the ring releases
/// instead of publishing.
#[test]
fn stale_mesh_reset() {
    let mut h = new_harness(
        1,
        7,
        far_config(false, 2, 3, 24 * charge()),
        frozen_limits(),
    );
    let chunk = ChunkPos::new(1, 1);
    set_mirror(&mut h, &[(Dimension::OVERWORLD, 1, 1, 4)]);
    admit_section(&mut h, chunk, 0, 4, 1, "admission");
    let first = publish(&mut h, chunk, full_budget()).expect("first mesh publishes");
    let stale_key = *first[0].resource().expect("resource");

    // The epoch invalidates: the arena retires the retained mesh. The
    // delivered geometry joins the released bytes without its own count,
    // because it was already delivered.
    let report = h.queue.invalidate(h.epoch);
    assert_eq!(report.jobs_cancelled(), 0);
    assert_eq!(report.results_stale(), 0);
    assert!(report.bytes_released() > 0, "the retained mesh releases");
    assert_eq!(
        h.queue.prepared_resource(&stale_key).unwrap_err(),
        ClientError::StaleEpoch,
        "the retired arena lookup is typed stale"
    );

    // A fresh epoch and selection over the same queue: the old reference
    // resets without a record and nothing stale publishes.
    let epoch = SessionEpoch::try_new(2).expect("epoch");
    h.epoch = epoch;
    h.selection = LodSelection::try_new(
        epoch,
        Dimension::OVERWORLD,
        far_config(false, 2, 3, 24 * charge()),
        7,
    )
    .expect("fresh selection");
    set_mirror(&mut h, &[(Dimension::OVERWORLD, 1, 1, 4)]);
    let records = publish(&mut h, chunk, full_budget()).expect("reset frame publishes");
    assert!(records.is_empty(), "the stale mesh never republishes");
    assert!(
        h.publisher.published_resources().is_empty(),
        "the cross-epoch reference reset"
    );

    // A far result of an old generation — submitted outside the ring —
    // releases instead of publishing.
    let rogue = far_job(&mut h, TilePos::new(5, 5), 6);
    let rogue_key = *rogue.key();
    h.queue.try_submit(rogue).expect("rogue admission");
    let records = publish(&mut h, chunk, full_budget()).expect("stale far frame publishes");
    assert!(
        records
            .iter()
            .all(|record| *record.key() != *rogue_key.key()),
        "an old-generation far result never publishes"
    );
    assert!(
        h.queue.prepared_resource(&rogue_key).is_err(),
        "the stale far geometry released"
    );
}

/// `terrain::full_range_negative_tile`: negative centers select the exact
/// negative band — arithmetic-shift centers, both closed radius edges, the
/// near-disk center excluded — and the extreme corner never wraps.
#[test]
fn full_range_negative_tile() {
    // A negative chunk center: the band is exact on the negative side and
    // the near section of a negative chunk publishes its exact key.
    let mut h = new_harness(1, 7, far_config(true, 2, 2, 24 * charge()), frozen_limits());
    set_mirror(&mut h, &[(Dimension::OVERWORLD, -1, -1, 5)]);
    let center = ChunkPos::new(-1, -1);
    let center_tile = lod::tile_from_chunk(center);
    assert_eq!(center_tile, TilePos::new(-1, -1), "floored center");
    publish(&mut h, center, full_budget()).expect("selection frame");
    admit_section(&mut h, center, 0, 5, 1, "near admission");
    let records = publish(&mut h, center, full_budget()).expect("negative frame publishes");
    let mut tiles = records
        .iter()
        .filter_map(|record| match record.key() {
            TerrainKey::LodTile(tile) => Some(*tile),
            TerrainKey::Section(_) => None,
        })
        .collect::<Vec<_>>();
    tiles.sort_by_key(|tile| (tile.x(), tile.z()));
    let mut expected: Vec<TilePos> = (-2..=0)
        .flat_map(|x| (-2..=0).map(move |z| TilePos::new(x, z)))
        .filter(|tile| *tile != center_tile)
        .collect();
    expected.sort_by_key(|tile| (tile.x(), tile.z()));
    assert_eq!(tiles, expected, "the exact negative closed band");
    assert_eq!(records.len(), 9, "eight tiles beside one near section");
    let near = records
        .iter()
        .find(|record| matches!(record.key(), TerrainKey::Section(_)))
        .expect("the near record");
    assert_eq!(
        *near.key(),
        TerrainKey::Section(SectionKey::try_new(center, 0).expect("section")),
        "the negative near key is exact"
    );
    assert_eq!(*near.visibility(), TerrainVisibility::Near);
    assert_resources_resolve(&h, &records);

    // The extreme corner: the band spans the closed distance-one ring
    // around the floored corner tile — the tile plane keeps a wide margin
    // inside i32, so nothing wraps — and the near-disk corner stays empty.
    let corner = ChunkPos::new(i32::MIN, i32::MIN);
    let corner_tile = lod::tile_from_chunk(corner);
    let mut h = new_harness(2, 7, far_config(true, 2, 2, 24 * charge()), frozen_limits());
    publish(&mut h, corner, full_budget()).expect("corner selection frame");
    let records = publish(&mut h, corner, full_budget()).expect("corner frame publishes");
    assert_eq!(records.len(), 8, "the full corner band");
    for record in &records {
        let TerrainKey::LodTile(tile) = *record.key() else {
            panic!("far records only");
        };
        assert!(
            (tile.x() - corner_tile.x()).abs() <= 1 && (tile.z() - corner_tile.z()).abs() <= 1,
            "the closed distance-one band around the corner tile"
        );
        assert!(
            tile.x() < 0 && tile.z() < 0,
            "no coordinate wraps toward the positive half-plane"
        );
        assert_ne!(tile, corner_tile, "the near-disk corner never publishes");
    }
}

/// `terrain::record_and_byte_caps_preserve_prior_output`: the family admits
/// a 4096-record change vector at the frozen count and byte cap through the
/// real owners at ring scale, while a vector one record over a configured
/// cap — or one record over a configured frame byte cap — rejects with the
/// typed capacity error and preserves the prior output and references.
#[test]
fn record_and_byte_caps_preserve_prior_output() {
    let center = ChunkPos::new(0, 0);

    // Ring scale under the frozen limits: the 8320-tile band of a wide
    // view distance fills the arena to its exact 4096 held-record bound
    // and the next frame publishes all 4096 far upserts.
    let mut h = new_harness(
        1,
        7,
        far_config(true, 64, 3, 4096 * charge()),
        frozen_limits(),
    );
    let records = publish(&mut h, center, full_budget()).expect("selection frame");
    assert!(records.is_empty());
    assert_eq!(
        h.queue.pending_jobs(),
        4096,
        "the arena is at its exact held-record bound"
    );
    let records = publish(&mut h, center, full_budget()).expect("ring-scale frame publishes");
    assert_eq!(
        records.len(),
        4096,
        "exactly the frozen family record count"
    );
    let mut keys = records
        .iter()
        .map(|record| *record.resource().expect("far resource"))
        .collect::<Vec<_>>();
    keys.sort_by_key(|key| (*key.key(), key.job_id().get()));
    let distinct = {
        let mut distinct = keys.clone();
        distinct.sort_by_key(|key| *key.key());
        distinct.dedup_by(|left, right| left.key() == right.key());
        distinct.len()
    };
    assert_eq!(distinct, 4096, "every record is a distinct full key");
    for key in [keys[0], keys[2048], keys[4095]] {
        assert!(h.queue.prepared_resource(&key).is_ok(), "spot arena lookup");
    }
    assert!(
        minimal_frame_size(&h, &records) <= frozen_limits().frame_bytes(),
        "the 4096-record vector fits the frozen frame byte cap"
    );

    // The record count cap admits exactly N and rejects N plus one: with
    // the count configured at four, four far upserts publish and a later
    // eight-record removal vector rejects, preserving the prior output.
    let mut h = new_harness(
        1,
        7,
        far_config(true, 2, 3, 4 * charge()),
        limits_with(4, frozen_limits().frame_bytes()),
    );
    publish(&mut h, center, full_budget()).expect("selection frame");
    let four = publish(&mut h, center, full_budget()).expect("four records admit");
    assert_eq!(four.len(), 4, "exactly the configured count admits");
    publish(&mut h, center, full_budget()).expect("second drain frame");
    let references = h.publisher.published_resources().to_vec();
    let jump = ChunkPos::new(1000, 1000);
    let error = publish(&mut h, jump, full_budget()).expect_err("eight removals reject");
    assert_eq!(error, ClientError::Capacity);
    assert_eq!(
        h.publisher.published_resources(),
        references,
        "references preserved"
    );
    assert_eq!(four.len(), 4, "the prior output stays the caller's vector");

    // The frame byte cap admits a vector at exactly the measured size and
    // rejects one record more, preserving the prior output; one byte of
    // slack admits the same two-record vector again.
    let measure = {
        let mut h = new_harness(1, 7, far_config(true, 2, 3, 2 * charge()), frozen_limits());
        publish(&mut h, center, full_budget()).expect("selection frame");
        let two = publish(&mut h, center, full_budget()).expect("two records");
        minimal_frame_size(&h, &two)
    };
    for (cap, label) in [
        (measure, "at the exact byte cap"),
        (measure + 1, "one byte of slack"),
    ] {
        let mut h = new_harness(
            1,
            7,
            far_config(true, 2, 3, 2 * charge()),
            limits_with(frozen_limits().family_records(), cap),
        );
        set_mirror(&mut h, &[(Dimension::OVERWORLD, 0, 0, 5)]);
        publish(&mut h, center, full_budget()).expect("selection frame");
        let two = publish(&mut h, center, full_budget()).expect(label);
        assert_eq!(two.len(), 2, "{label} admits the two-record vector");
        publish(&mut h, center, full_budget()).expect("second dispatch frame");
        let references = h.publisher.published_resources().to_vec();
        admit_section(&mut h, center, 0, 5, 1, "near admission");
        let error =
            publish(&mut h, center, full_budget()).expect_err("three records exceed the byte cap");
        assert_eq!(error, ClientError::Capacity, "{label} rejects over cap");
        assert_eq!(
            h.publisher.published_resources(),
            references,
            "the prior references are preserved"
        );
        assert_eq!(two.len(), 2, "the prior output stays the caller's vector");
    }
}

/// `terrain::rejected_upserts_retry_and_dropped_release`: a frame rejected
/// by the record cap preserves the prior output and references, forgets
/// every drained geometry whose key was never published (the arena returns
/// to its prior state for those keys), keeps the rejected upserts
/// retry-owned — neither consumed nor forgotten — and publishes them on the
/// next frame once capacity frees.
#[test]
fn rejected_upserts_retry_and_dropped_release() {
    // An eight-tile band, a four-job frame allowance and a record cap of
    // four: the first two frames publish the first four tiles.
    let mut h = new_harness(
        1,
        7,
        far_config(true, 2, 2, 4 * charge()),
        limits_with(4, frozen_limits().frame_bytes()),
    );
    set_mirror(&mut h, &[(Dimension::OVERWORLD, 0, 0, 5)]);
    let center = ChunkPos::new(0, 0);
    let center_tile = lod::tile_from_chunk(center);
    publish(&mut h, center, full_budget()).expect("selection frame");
    let four = publish(&mut h, center, full_budget()).expect("four records admit");
    assert_eq!(four.len(), 4);
    let references = h.publisher.published_resources().to_vec();
    assert_eq!(references.len(), 4);

    // The accepted frame's tail dispatches the remaining four tiles; a
    // rogue far job inside the near disk and one water near section join
    // them in the queue, and the frame that drains all six is rejected:
    // five valid upserts over a cap of four.
    let near = near_job(&mut h, center, 0, 5, 2);
    let near_key = *near.key();
    admit_near(&mut h, near).expect("near admission");
    let rogue = far_job(&mut h, center_tile, 7);
    let rogue_key = *rogue.key();
    h.queue.try_submit(rogue).expect("rogue admission");

    let error = publish(&mut h, center, full_budget()).expect_err("five upserts reject");
    assert_eq!(error, ClientError::Capacity);
    assert_eq!(four.len(), 4, "the prior output stays the caller's vector");
    assert_eq!(
        h.publisher.published_resources(),
        references,
        "the prior references are preserved"
    );
    // The never-published rogue released at rejection; the rejected valid
    // upserts stay retry-owned with their geometries retained.
    assert!(
        h.queue.prepared_resource(&rogue_key).is_err(),
        "a never-published drained geometry is forgotten at rejection"
    );
    assert!(
        h.queue.prepared_resource(&near_key).is_ok(),
        "the rejected near mesh is retained"
    );
    assert_eq!(
        h.publisher.retry_owned(),
        5,
        "four far tiles and one near section"
    );
    for record in &four {
        let key = record.resource().expect("far resource");
        assert!(
            h.queue.prepared_resource(key).is_ok(),
            "the prior output still resolves"
        );
    }

    // Capacity frees and the next frame publishes the retry-owned upserts.
    h.limits = limits_with(6, frozen_limits().frame_bytes());
    let records = publish(&mut h, center, full_budget()).expect("the retry frame publishes");
    assert_eq!(
        records.len(),
        5,
        "the rejected upserts publish after capacity frees"
    );
    let near = records
        .iter()
        .find(|record| *record.key() == *near_key.key())
        .expect("the rejected near section publishes");
    assert_eq!(
        *near.material(),
        TerrainMaterial::Water,
        "the retry carries its own summary"
    );
    assert!(
        records
            .iter()
            .all(|record| *record.key() != *rogue_key.key())
    );
    assert_eq!(
        h.publisher.retry_owned(),
        0,
        "an accepted publication consumes the retries"
    );
    assert_eq!(
        h.publisher.published_resources().len(),
        9,
        "four kept plus five published"
    );
    assert_resources_resolve(&h, &records);

    // A stale retry entry never resurrects: the mirror advanced past the
    // rejected sections' revision after the rejection.
    let mut h = new_harness(
        1,
        7,
        far_config(false, 2, 3, 24 * charge()),
        limits_with(1, frozen_limits().frame_bytes()),
    );
    let chunk = ChunkPos::new(3, 3);
    set_mirror(&mut h, &[(Dimension::OVERWORLD, 3, 3, 5)]);
    let job = near_job(&mut h, chunk, 1, 5, 2);
    let stale_key = *job.key();
    admit_near(&mut h, job).expect("first admission");
    admit_section(&mut h, chunk, 2, 5, 1, "second admission");
    let error = publish(&mut h, chunk, full_budget()).expect_err("two upserts reject");
    assert_eq!(error, ClientError::Capacity);
    assert_eq!(h.publisher.retry_owned(), 2);
    assert!(
        h.queue.prepared_resource(&stale_key).is_ok(),
        "the rejected geometry stays retry-owned"
    );
    set_mirror(&mut h, &[(Dimension::OVERWORLD, 3, 3, 6)]);
    let records = publish(&mut h, chunk, full_budget()).expect("the stale retry frame publishes");
    assert!(
        records
            .iter()
            .all(|record| *record.key() != *stale_key.key()),
        "a retry the mirror moved past never publishes"
    );
    assert!(
        h.queue.prepared_resource(&stale_key).is_err(),
        "the stale retry geometry is forgotten"
    );
    assert_eq!(h.publisher.retry_owned(), 0);
    assert_eq!(
        h.publisher.summary_slots(),
        0,
        "a dead retry releases its admission-summary slot with its geometry"
    );
}

/// `terrain::summary_slots_stay_bounded`: admission-summary slots exist only
/// while their exact admission is live in publication, in flight, or
/// retry-owned — sections removed by the mirror, replaced by a newer
/// revision, reset across an epoch, dead as stale retries, or dropped from
/// a rebased drain stop holding slots, and a re-admitted section records
/// its summary again.
#[test]
fn summary_slots_stay_bounded() {
    let mut h = new_harness(
        1,
        7,
        far_config(false, 2, 3, 24 * charge()),
        frozen_limits(),
    );
    let a = ChunkPos::new(0, 0);
    let b = ChunkPos::new(1, 0);
    let c = ChunkPos::new(2, 0);
    set_mirror(
        &mut h,
        &[
            (Dimension::OVERWORLD, 0, 0, 5),
            (Dimension::OVERWORLD, 1, 0, 5),
            (Dimension::OVERWORLD, 2, 0, 5),
        ],
    );

    // Three admissions record three slots before anything publishes.
    assert_eq!(h.publisher.summary_slots(), 0);
    admit_section(&mut h, a, 0, 5, 1, "a admission");
    admit_section(&mut h, b, 0, 5, 1, "b admission");
    admit_section(&mut h, c, 0, 5, 1, "c admission");
    assert_eq!(
        h.publisher.summary_slots(),
        3,
        "one slot per live admission"
    );
    let records = publish(&mut h, a, full_budget()).expect("three upserts publish");
    assert_eq!(records.len(), 3);
    assert_eq!(
        h.publisher.summary_slots(),
        3,
        "published sections keep their slots"
    );

    // A chunk the mirror dropped releases its slot with its removal record.
    set_mirror(
        &mut h,
        &[
            (Dimension::OVERWORLD, 0, 0, 5),
            (Dimension::OVERWORLD, 2, 0, 5),
        ],
    );
    let records = publish(&mut h, a, full_budget()).expect("the forget frame publishes");
    assert_eq!(records.len(), 1, "one ordered removal");
    assert_eq!(records[0].header().operation(), FamilyOperation::Remove);
    assert_eq!(
        h.publisher.summary_slots(),
        2,
        "the removed section releases its slot"
    );

    // A revision the mirror advanced past releases its slot too, even
    // though no replacement was admitted yet.
    set_mirror(
        &mut h,
        &[
            (Dimension::OVERWORLD, 0, 0, 5),
            (Dimension::OVERWORLD, 2, 0, 6),
        ],
    );
    publish(&mut h, a, full_budget()).expect("the replacement frame publishes");
    assert_eq!(
        h.publisher.summary_slots(),
        1,
        "the superseded section releases its slot"
    );

    // A cross-epoch reset releases the last slot without a record, and a
    // re-admitted section records its summary again.
    let epoch = SessionEpoch::try_new(2).expect("epoch");
    h.epoch = epoch;
    h.selection = LodSelection::try_new(
        epoch,
        Dimension::OVERWORLD,
        far_config(false, 2, 3, 24 * charge()),
        7,
    )
    .expect("fresh selection");
    set_mirror(&mut h, &[(Dimension::OVERWORLD, 0, 0, 5)]);
    let records = publish(&mut h, a, full_budget()).expect("the reset frame publishes");
    assert!(records.is_empty());
    assert_eq!(
        h.publisher.summary_slots(),
        0,
        "the cross-epoch reset releases its slot"
    );
    admit_section(&mut h, a, 0, 5, 2, "re-admission");
    assert_eq!(
        h.publisher.summary_slots(),
        1,
        "a re-admitted section records again"
    );
    let records = publish(&mut h, a, full_budget()).expect("the re-admission publishes");
    assert_eq!(records.len(), 1);
    assert_eq!(
        *records[0].material(),
        TerrainMaterial::Water,
        "the re-recorded summary is real"
    );

    // An old-epoch in-flight admission whose result only drains after the
    // epoch rebased dies as a never-published drain drop: it releases both
    // its geometry and its summary slot, even though it never appeared in
    // the published set.
    let mut h = new_harness(
        1,
        7,
        far_config(false, 2, 3, 24 * charge()),
        frozen_limits(),
    );
    let chunk = ChunkPos::new(5, 5);
    set_mirror(&mut h, &[(Dimension::OVERWORLD, 5, 5, 3)]);
    let job = near_job(&mut h, chunk, 0, 3, 2);
    let old_key = *job.key();
    admit_near(&mut h, job).expect("old-epoch admission");
    assert_eq!(
        h.publisher.summary_slots(),
        1,
        "the in-flight admission holds its slot"
    );
    let epoch = SessionEpoch::try_new(2).expect("epoch");
    h.epoch = epoch;
    h.selection = LodSelection::try_new(
        epoch,
        Dimension::OVERWORLD,
        far_config(false, 2, 3, 24 * charge()),
        7,
    )
    .expect("fresh selection");
    set_mirror(&mut h, &[(Dimension::OVERWORLD, 5, 5, 3)]);
    let records = publish(&mut h, chunk, full_budget()).expect("the rebased frame publishes");
    assert!(
        records.iter().all(|record| *record.key() != *old_key.key()),
        "an old-epoch result never publishes"
    );
    assert!(
        h.queue.prepared_resource(&old_key).is_err(),
        "the old-epoch geometry released"
    );
    assert_eq!(
        h.publisher.summary_slots(),
        0,
        "the cross-epoch drain drop releases its admission-summary slot"
    );
}
