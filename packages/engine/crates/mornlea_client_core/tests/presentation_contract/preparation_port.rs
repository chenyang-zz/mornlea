//! The shared bounded preparation port's success/failure double and its
//! registered contract case.
//!
//! The double is the deterministic port owner: it charges the payload's own
//! allocations plus each shared Arc allocation once while the owner holds it,
//! admits all or returns the complete job, drains FIFO and releases stale
//! work exactly once.

use std::collections::VecDeque;
use std::sync::Arc;

use mornlea_client_core::contracts::{ClientError, ClientLimits, PreparationPort, SessionEpoch};
use mornlea_client_core::preparation::{
    InvalidationReport, OwnedLodRequest, OwnedMeshView, PreparationJob, PreparationPayload,
    PreparationResult, PreparationTicket, PreparedGeometry, PreparedResourceKey,
    RejectedPreparation, SectionKey, TerrainKey,
};
use mornlea_domain::{ChunkPos, Dimension};
use mornlea_engine::native::contracts::mesh::{MeshModel, MeshRegistry, MeshRegistryEntry};
use mornlea_engine::native::contracts::world::{Materials, WorldgenParams};
use mornlea_protocol::MIN_Y;

/// One checked far-LOD worldgen parameter fixture.
pub fn fixture_params() -> Arc<WorldgenParams> {
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
    WorldgenParams::try_new(1, materials, perm)
        .expect("checked worldgen params")
        .into()
}

/// One checked near-mesh registry fixture.
pub fn fixture_registry() -> Arc<MeshRegistry> {
    let entries = [MeshRegistryEntry {
        id: 1,
        opaque: true,
        emission: 0,
        material: [1; 6],
        fluid_height: 0,
        light_attenuation: 0,
        block_top_raw: 0,
        model: MeshModel::Default,
    }];
    MeshRegistry::try_new(&entries, &[0], 0, 1)
        .expect("checked registry")
        .into()
}

/// One checked near-mesh payload with the exact owned arrays.
pub fn near_payload(registry: &Arc<MeshRegistry>, origin_y: i32) -> OwnedMeshView {
    OwnedMeshView::try_new(
        Box::new([0u16; 110592]),
        [false; 9],
        Box::new([[0i16; 256]; 9]),
        origin_y,
        Arc::clone(registry),
    )
    .expect("checked mesh view")
}

/// One checked far-LOD payload.
pub fn far_payload(params: &Arc<WorldgenParams>, tile: [i32; 2]) -> OwnedLodRequest {
    OwnedLodRequest::try_new(
        Arc::clone(params),
        tile,
        mornlea_client_core::preparation::LodStep::Four,
    )
    .expect("checked lod request")
}

/// One checked near-section job key.
pub fn near_key(epoch: SessionEpoch, job_id: u64) -> PreparedResourceKey {
    PreparedResourceKey::try_new(
        epoch,
        Dimension::OVERWORLD,
        TerrainKey::Section(SectionKey::try_new(ChunkPos::new(0, 0), 0).expect("section")),
        1,
        1,
        job_id,
    )
    .expect("checked resource key")
}

/// The deterministic bounded queue double implementing the frozen port.
pub struct QueueDouble {
    limits: ClientLimits,
    queue: VecDeque<(PreparationTicket, PreparationJob, usize)>,
    results: VecDeque<(PreparationResult, usize)>,
    next_ticket: u64,
    tickets_issued: u64,
    charge: usize,
    seen_registries: Vec<usize>,
    seen_params: Vec<usize>,
}

impl QueueDouble {
    pub fn new(limits: ClientLimits) -> Self {
        Self {
            limits,
            queue: VecDeque::new(),
            results: VecDeque::new(),
            next_ticket: 1,
            tickets_issued: 0,
            charge: 0,
            seen_registries: Vec::new(),
            seen_params: Vec::new(),
        }
    }

    pub fn tickets_issued(&self) -> u64 {
        self.tickets_issued
    }

    pub fn charge(&self) -> usize {
        self.charge
    }

    fn registry_charge(&mut self, registry: &Arc<MeshRegistry>) -> usize {
        let identity = Arc::as_ptr(registry) as *const () as usize;
        if self.seen_registries.contains(&identity) {
            return 0;
        }
        self.seen_registries.push(identity);
        registry.entries().len() * std::mem::size_of::<MeshRegistryEntry>()
            + registry.visibility().len() * 8
            + 4
    }

    fn params_charge(&mut self, params: &Arc<WorldgenParams>) -> usize {
        let identity = Arc::as_ptr(params) as *const () as usize;
        if self.seen_params.contains(&identity) {
            return 0;
        }
        self.seen_params.push(identity);
        std::mem::size_of::<WorldgenParams>()
    }

    fn job_charge(&mut self, job: &PreparationJob) -> usize {
        match job.payload() {
            PreparationPayload::Near(near) => {
                near.owned_bytes() + self.registry_charge(near.registry())
            }
            PreparationPayload::Far(far) => far.owned_bytes() + self.params_charge(far.params()),
        }
    }
}

impl PreparationPort for QueueDouble {
    fn try_submit(
        &mut self,
        job: PreparationJob,
    ) -> Result<PreparationTicket, RejectedPreparation> {
        let held = self.queue.len() + self.results.len();
        if held + 1 > self.limits.preparation_results() {
            return Err(
                RejectedPreparation::try_new(ClientError::Capacity, job).expect("rejection")
            );
        }
        let extra = self.job_charge(&job);
        let new_charge = self.charge.saturating_add(extra);
        if new_charge > self.limits.preparation_bytes() {
            return Err(
                RejectedPreparation::try_new(ClientError::Capacity, job).expect("rejection")
            );
        }
        let ticket = PreparationTicket::try_new(self.next_ticket).expect("nonzero ticket");
        self.next_ticket += 1;
        self.tickets_issued += 1;
        self.charge = new_charge;
        self.queue.push_back((ticket, job, extra));
        Ok(ticket)
    }

    fn poll_ready(&mut self) -> Option<PreparationResult> {
        if let Some((result, _charge)) = self.results.pop_front() {
            return Some(result);
        }
        let (ticket, job, charge) = self.queue.pop_front()?;
        let key = *job.key();
        let geometry = match job.payload() {
            PreparationPayload::Near(_) => PreparedGeometry::Near(Vec::new()),
            PreparationPayload::Far(_) => PreparedGeometry::Far(Vec::new()),
        };
        let outcome = PreparedGeometry::try_new(geometry).map(Arc::new);
        let result = PreparationResult::try_new(ticket, key, outcome).expect("checked result");
        self.results.push_back((result, charge));
        self.results.pop_front().map(|(result, _)| result)
    }

    fn invalidate(&mut self, epoch: SessionEpoch) -> InvalidationReport {
        let mut jobs_cancelled = 0u32;
        let mut results_stale = 0u32;
        let mut bytes_released = 0u64;
        let remaining = self.queue.len();
        for _ in 0..remaining {
            let (ticket, job, charge) = self.queue.pop_front().expect("queued job");
            if job.key().epoch() == epoch {
                jobs_cancelled += 1;
                bytes_released += charge as u64;
            } else {
                self.queue.push_back((ticket, job, charge));
            }
        }
        let remaining = self.results.len();
        for _ in 0..remaining {
            let (result, charge) = self.results.pop_front().expect("held result");
            if result.key().epoch() == epoch {
                results_stale += 1;
                bytes_released += charge as u64;
            } else {
                self.results.push_back((result, charge));
            }
        }
        self.charge -= usize::try_from(bytes_released)
            .unwrap_or(self.charge)
            .min(self.charge);
        InvalidationReport::try_new(jobs_cancelled, results_stale, bytes_released)
            .expect("checked report")
    }
}

/// `preparation_port::rejected_job_retained`: a byte or count plus one
/// returns the whole job with no ticket increment, FIFO order is retained,
/// and stale invalidation releases exactly once.
#[test]
fn rejected_job_retained() {
    let epoch = SessionEpoch::try_new(1).expect("epoch");
    let registry = fixture_registry();
    let payload_bytes = near_payload(&registry, MIN_Y).owned_bytes();
    let registry_charge =
        std::mem::size_of::<MeshRegistryEntry>() + registry.visibility().len() * 8 + 4;
    let job_charge = payload_bytes + registry_charge;

    // Count plus one: two results are retained, the third is returned whole
    // with no ticket increment.
    let count_limits = ClientLimits::try_new_with(
        128,
        8192,
        8 << 20,
        4104,
        8 << 20,
        256,
        4096,
        4096,
        2,
        64 << 20,
        4096,
        8 << 20,
    )
    .expect("configured limits");
    let mut port = QueueDouble::new(count_limits);
    port.try_submit(
        PreparationJob::try_new(
            near_key(epoch, 1),
            PreparationPayload::Near(near_payload(&registry, MIN_Y)),
        )
        .expect("paired"),
    )
    .expect("first job admitted");
    port.try_submit(
        PreparationJob::try_new(
            near_key(epoch, 2),
            PreparationPayload::Near(near_payload(&registry, MIN_Y)),
        )
        .expect("paired"),
    )
    .expect("second job admitted");
    let rejected_job = PreparationJob::try_new(
        near_key(epoch, 3),
        PreparationPayload::Near(near_payload(&registry, MIN_Y)),
    )
    .expect("paired");
    let error = port
        .try_submit(rejected_job)
        .expect_err("count plus one rejects");
    assert_eq!(error.error(), ClientError::Capacity);
    assert_eq!(
        error.job().key().job_id().get(),
        3,
        "the whole job is returned"
    );
    assert_eq!(
        error.job().payload().owned_bytes(),
        payload_bytes,
        "payload identity retained"
    );
    assert_eq!(port.tickets_issued(), 2, "no ticket increment on rejection");

    // Byte plus one: a cap of two charges plus one admits two jobs and
    // returns the third whole.
    let byte_limits = ClientLimits::try_new_with(
        128,
        8192,
        8 << 20,
        4104,
        8 << 20,
        256,
        4096,
        4096,
        4096,
        job_charge * 2 + 1,
        4096,
        8 << 20,
    )
    .expect("configured limits");
    let mut port = QueueDouble::new(byte_limits);
    port.try_submit(
        PreparationJob::try_new(
            near_key(epoch, 1),
            PreparationPayload::Near(near_payload(&registry, MIN_Y)),
        )
        .expect("paired"),
    )
    .expect("first job admitted");
    port.try_submit(
        PreparationJob::try_new(
            near_key(epoch, 2),
            PreparationPayload::Near(near_payload(&registry, MIN_Y)),
        )
        .expect("paired"),
    )
    .expect("second job admitted");
    let charge_before = port.charge();
    let error = port
        .try_submit(
            PreparationJob::try_new(
                near_key(epoch, 3),
                PreparationPayload::Near(near_payload(&registry, MIN_Y)),
            )
            .expect("paired"),
        )
        .expect_err("byte plus one rejects");
    assert_eq!(error.error(), ClientError::Capacity);
    assert_eq!(error.job().key().job_id().get(), 3);
    assert_eq!(port.charge(), charge_before, "the charge is unchanged");

    // The frozen 64 MiB cap: 297 separately allocated mesh payloads fit, the
    // 298th returns whole.
    let mut port = QueueDouble::new(ClientLimits::try_new().expect("frozen limits"));
    for job_id in 1..=297u64 {
        port.try_submit(
            PreparationJob::try_new(
                near_key(epoch, job_id),
                PreparationPayload::Near(near_payload(&registry, MIN_Y)),
            )
            .expect("paired"),
        )
        .unwrap_or_else(|_| panic!("job {job_id} fits under the frozen cap"));
    }
    let error = port
        .try_submit(
            PreparationJob::try_new(
                near_key(epoch, 298),
                PreparationPayload::Near(near_payload(&registry, MIN_Y)),
            )
            .expect("paired"),
        )
        .expect_err("the frozen cap rejects the 298th job");
    assert_eq!(error.job().key().job_id().get(), 298, "whole job returned");
    assert_eq!(port.tickets_issued(), 297);

    // FIFO: results drain in submission order with their exact identities.
    let mut port = QueueDouble::new(ClientLimits::try_new().expect("frozen limits"));
    for job_id in 1..=3u64 {
        port.try_submit(
            PreparationJob::try_new(
                near_key(epoch, job_id),
                PreparationPayload::Near(near_payload(&registry, MIN_Y)),
            )
            .expect("paired"),
        )
        .expect("admitted");
    }
    for expected in 1..=3u64 {
        let result = port.poll_ready().expect("FIFO result");
        assert_eq!(result.key().job_id().get(), expected, "FIFO order");
    }
    assert!(port.poll_ready().is_none(), "the queue drains exactly");

    // Stale invalidation releases exactly once.
    let mut port = QueueDouble::new(ClientLimits::try_new().expect("frozen limits"));
    port.try_submit(
        PreparationJob::try_new(
            near_key(epoch, 1),
            PreparationPayload::Near(near_payload(&registry, MIN_Y)),
        )
        .expect("paired"),
    )
    .expect("admitted");
    let charge = port.charge();
    let report = port.invalidate(epoch);
    assert_eq!(report.jobs_cancelled(), 1);
    assert!(
        report.bytes_released() > 0,
        "the released charge is measured"
    );
    assert_eq!(port.charge(), 0);
    let _ = charge;
    assert!(port.poll_ready().is_none(), "stale work never completes");
    let report = port.invalidate(epoch);
    assert_eq!(report.jobs_cancelled(), 0, "no double release");
    assert_eq!(report.results_stale(), 0);
    assert_eq!(report.bytes_released(), 0);
}

/// The checked constructor enforces the key/payload pairing and the native
/// admitted ranges before any queue sees the job.
#[test]
fn near_far_pairing_enforced() {
    let epoch = SessionEpoch::try_new(1).expect("epoch");
    let registry = fixture_registry();
    let params = fixture_params();

    // A near payload requires a section key with the matching origin.
    let far_key = PreparedResourceKey::try_new(
        epoch,
        Dimension::OVERWORLD,
        TerrainKey::LodTile(mornlea_client_core::preparation::TilePos::new(3, 4)),
        1,
        1,
        1,
    )
    .expect("far key");
    assert!(
        PreparationJob::try_new(
            far_key,
            PreparationPayload::Near(near_payload(&registry, MIN_Y))
        )
        .is_err()
    );

    // A far payload requires a lod-tile key with the matching tile.
    let section_key = near_key(epoch, 1);
    assert!(
        PreparationJob::try_new(
            section_key,
            PreparationPayload::Far(far_payload(&params, [3, 4]))
        )
        .is_err()
    );

    // A near payload whose section origin disagrees with its section index
    // rejects.
    let wrong_origin = PreparedResourceKey::try_new(
        epoch,
        Dimension::OVERWORLD,
        TerrainKey::Section(SectionKey::try_new(ChunkPos::new(0, 0), 1).expect("section")),
        1,
        1,
        1,
    )
    .expect("key");
    assert!(
        PreparationJob::try_new(
            wrong_origin,
            PreparationPayload::Near(near_payload(&registry, MIN_Y))
        )
        .is_err()
    );

    // A matched far pair admits.
    let matched = PreparedResourceKey::try_new(
        epoch,
        Dimension::OVERWORLD,
        TerrainKey::LodTile(mornlea_client_core::preparation::TilePos::new(3, 4)),
        1,
        1,
        2,
    )
    .expect("far key");
    assert!(
        PreparationJob::try_new(
            matched,
            PreparationPayload::Far(far_payload(&params, [3, 4]))
        )
        .is_ok()
    );
}
