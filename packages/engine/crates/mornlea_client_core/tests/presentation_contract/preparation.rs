//! The bounded near-preparation owner and resource-arena contract cases.
//!
//! Every row drives the owner through the frozen `PreparationPort` surface
//! plus the provider-owned per-step work budget, the retained-resource lookup
//! and the forget path. The `Subject` selector keeps the two landed doubles
//! addressable beside the real owner so the permanent artifact row pins the
//! wrong behaviors this provider replaces: draining beyond the per-step mesh
//! budget, publishing a stale or duplicate generation, never retaining a
//! resource at all, and charging a shared far-params allocation once per
//! holder instead of once per allocation.

use std::sync::Arc;

use mornlea_client_core::contracts::{
    ClientError, ClientLimits, ClientWorkBudget, PreparationPort, SessionEpoch,
};
use mornlea_client_core::preparation::{
    InvalidationReport, PreparationJob, PreparationPayload, PreparationQueue, PreparationResult,
    PreparationTicket, PreparedGeometry, PreparedResourceKey, RejectedPreparation, SectionKey,
    TerrainKey, TilePos,
};
use mornlea_domain::{ChunkPos, Dimension};
use mornlea_engine::native::contracts::mesh::{MeshQuad, MeshRegistryEntry};
use mornlea_engine::native::contracts::world::WorldgenParams;
use mornlea_protocol::MIN_Y;

use super::preparation_port::{
    QueueDouble, far_payload, fixture_params, fixture_registry, near_payload,
};
use super::support::PreparationFaucet;

/// Which owner a table row drives: the registered queue double, the harness
/// faucet, or the real bounded preparation owner.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Subject {
    /// The registered queue double: correct admission bounds, but no work
    /// budget, no staleness rejection and no retained-resource arena.
    QueueDouble,
    /// The harness faucet: delayed completion with correct registry identity
    /// accounting, but a shared far-params allocation is charged once per
    /// holder rather than once per allocation.
    Faucet,
    /// The real bounded preparation owner and resource arena.
    Provider,
}

/// The general row selector and the shared-allocation row selector; the
/// committed table drives the real owner through both.
const DRIVER: Subject = Subject::Provider;
const ARC_DRIVER: Subject = Subject::Provider;

/// The uniform driver surface the rows exercise.
enum Backend {
    QueueDouble(QueueDouble),
    Faucet(PreparationFaucet),
    Provider(PreparationQueue),
}

fn backend_of(kind: Subject, limits: ClientLimits) -> Backend {
    match kind {
        Subject::QueueDouble => Backend::QueueDouble(QueueDouble::new(limits)),
        Subject::Faucet => Backend::Faucet(PreparationFaucet::new(limits)),
        Subject::Provider => Backend::Provider(PreparationQueue::new(limits)),
    }
}

impl Backend {
    fn submit(&mut self, job: PreparationJob) -> Result<PreparationTicket, RejectedPreparation> {
        match self {
            Backend::QueueDouble(port) => port.try_submit(job),
            Backend::Faucet(port) => port.try_submit(job),
            Backend::Provider(port) => port.try_submit(job),
        }
    }

    /// Completes pending work under the demanded budget rechecked against the
    /// configured mesh-work ceiling, returning how many units were consumed.
    fn work(&mut self, limits: &ClientLimits, budget: ClientWorkBudget) -> u16 {
        match self {
            // The queue double couples completion to polling and has no
            // budget surface at all, so its work emulation drains everything.
            Backend::QueueDouble(port) => {
                let mut consumed = 0u16;
                while port.poll_ready().is_some() {
                    consumed += 1;
                }
                consumed
            }
            Backend::Faucet(port) => {
                let drain = usize::from(budget.meshes()).min(limits.mesh_work());
                u16::try_from(port.advance(drain)).unwrap_or(u16::MAX)
            }
            Backend::Provider(port) => port.work(budget),
        }
    }

    fn poll(&mut self) -> Option<PreparationResult> {
        match self {
            Backend::QueueDouble(port) => port.poll_ready(),
            Backend::Faucet(port) => port.poll_ready(),
            Backend::Provider(port) => port.poll_ready(),
        }
    }

    fn invalidate(&mut self, epoch: SessionEpoch) -> InvalidationReport {
        match self {
            Backend::QueueDouble(port) => port.invalidate(epoch),
            Backend::Faucet(port) => port.invalidate(epoch),
            Backend::Provider(port) => port.invalidate(epoch),
        }
    }

    /// The retained-resource lookup; the doubles never retain a resource.
    fn lookup(&self, key: &PreparedResourceKey) -> Result<Arc<PreparedGeometry>, ClientError> {
        match self {
            Backend::QueueDouble(_) | Backend::Faucet(_) => Err(ClientError::InvalidInput),
            Backend::Provider(port) => port.prepared_resource(key),
        }
    }

    /// The retained-resource release; the doubles have no arena to release.
    fn forget(&mut self, key: &PreparedResourceKey) -> Result<u64, ClientError> {
        match self {
            Backend::QueueDouble(_) | Backend::Faucet(_) => Ok(0),
            Backend::Provider(port) => port.forget(key),
        }
    }

    fn charge(&self) -> usize {
        match self {
            Backend::QueueDouble(port) => port.charge(),
            Backend::Faucet(port) => port.charge(),
            Backend::Provider(port) => port.charge(),
        }
    }

    fn tickets_issued(&self) -> u64 {
        match self {
            Backend::QueueDouble(port) => port.tickets_issued(),
            Backend::Faucet(port) => port.tickets_issued(),
            Backend::Provider(port) => port.tickets_issued(),
        }
    }
}

// --- checked fixtures ---

fn frozen_limits() -> ClientLimits {
    ClientLimits::try_new().expect("frozen limits")
}

/// The frozen limits with the mesh-work, record-count and byte ceilings named
/// by the row; every other limit stays at its frozen value.
fn limits_with(
    mesh_work: usize,
    preparation_results: usize,
    preparation_bytes: usize,
) -> ClientLimits {
    ClientLimits::try_new_with(
        128,
        8192,
        8 << 20,
        4104,
        8 << 20,
        256,
        4096,
        mesh_work,
        preparation_results,
        preparation_bytes,
        4096,
        8 << 20,
    )
    .expect("configured limits")
}

fn epoch(value: u64) -> SessionEpoch {
    SessionEpoch::try_new(value).expect("epoch")
}

/// One near-section resource key at the named chunk, section and identity.
fn near_key_at(
    epoch_value: u64,
    chunk: (i32, i32),
    section: u8,
    generation: u64,
    content: u64,
    job_id: u64,
) -> PreparedResourceKey {
    PreparedResourceKey::try_new(
        epoch(epoch_value),
        Dimension::OVERWORLD,
        TerrainKey::Section(
            SectionKey::try_new(ChunkPos::new(chunk.0, chunk.1), section).expect("section"),
        ),
        generation,
        content,
        job_id,
    )
    .expect("checked resource key")
}

/// One far-tile resource key at the named tile and identity.
fn far_key_at(
    epoch_value: u64,
    tile: [i32; 2],
    generation: u64,
    content: u64,
    job_id: u64,
) -> PreparedResourceKey {
    PreparedResourceKey::try_new(
        epoch(epoch_value),
        Dimension::OVERWORLD,
        TerrainKey::LodTile(TilePos::new(tile[0], tile[1])),
        generation,
        content,
        job_id,
    )
    .expect("checked resource key")
}

/// One paired near job whose payload origin matches its section index.
fn near_job(
    key: &PreparedResourceKey,
    registry: &Arc<mornlea_engine::native::contracts::mesh::MeshRegistry>,
) -> PreparationJob {
    let section = match key.key() {
        TerrainKey::Section(section) => *section,
        TerrainKey::LodTile(_) => panic!("near job needs a section key"),
    };
    let origin = MIN_Y + i32::from(section.section()) * 16;
    PreparationJob::try_new(
        *key,
        PreparationPayload::Near(near_payload(registry, origin)),
    )
    .expect("paired near job")
}

/// One paired far job for the named tile.
fn far_job(key: &PreparedResourceKey, params: &Arc<WorldgenParams>) -> PreparationJob {
    let tile = match key.key() {
        TerrainKey::LodTile(tile) => [tile.x(), tile.z()],
        TerrainKey::Section(_) => panic!("far job needs a lod-tile key"),
    };
    PreparationJob::try_new(*key, PreparationPayload::Far(far_payload(params, tile)))
        .expect("paired far job")
}

/// The registry allocation charge the owner counts once per shared Arc.
fn registry_charge(registry: &Arc<mornlea_engine::native::contracts::mesh::MeshRegistry>) -> usize {
    std::mem::size_of::<MeshRegistryEntry>() + registry.visibility().len() * 8 + 4
}

/// The geometry allocation charge of one owned result: the discriminant byte
/// plus the packed quad table the retained Arc owns.
fn geometry_charge(result: &PreparationResult) -> usize {
    let geometry = result.outcome().as_ref().expect("geometry result");
    1 + std::mem::size_of::<MeshQuad>() * geometry.quads()
}

// --- table rows ---

/// The record-count bound admits every far work item up to the frozen 4096
/// and returns the next one whole with no ticket and no charge change.
#[test]
fn admission_count_returns_whole_job() {
    let params = fixture_params();
    let mut subject = backend_of(DRIVER, frozen_limits());
    for job_id in 1..=4096u64 {
        let job = far_job(&far_key_at(1, [7, 7], 1, 1, job_id), &params);
        subject
            .submit(job)
            .unwrap_or_else(|_| panic!("far job {job_id} admitted"));
    }
    let over = far_job(&far_key_at(1, [7, 7], 1, 1, 4097), &params);
    let payload_bytes = over.payload().owned_bytes();
    let charge_before = subject.charge();
    let rejection = subject
        .submit(over)
        .expect_err("the count plus one rejects");
    assert_eq!(rejection.error(), ClientError::Capacity);
    assert_eq!(
        rejection.job().key().job_id().get(),
        4097,
        "the whole job is returned"
    );
    assert_eq!(
        rejection.job().payload().owned_bytes(),
        payload_bytes,
        "payload identity retained"
    );
    assert_eq!(subject.tickets_issued(), 4096, "no ticket on rejection");
    assert_eq!(subject.charge(), charge_before, "the charge is unchanged");
}

/// The byte bound: the frozen 64 MiB cap plus one, a tight cap plus one and a
/// cap below one job all reject the complete job before any allocation, and
/// the charge sums queue-held payloads with worker-held geometry.
#[test]
fn byte_bounds_frozen_cap_and_plus_one() {
    let registry = fixture_registry();
    let job_charge = near_payload(&registry, MIN_Y).owned_bytes() + registry_charge(&registry);

    // The frozen cap: shared-registry near payloads fill it exactly, the next
    // one returns whole.
    let mut subject = backend_of(DRIVER, frozen_limits());
    for job_id in 1..=297u64 {
        subject
            .submit(near_job(
                &near_key_at(1, (0, 0), 0, 1, 1, job_id),
                &registry,
            ))
            .unwrap_or_else(|_| panic!("near job {job_id} fits the frozen cap"));
    }
    let rejection = subject
        .submit(near_job(&near_key_at(1, (0, 0), 0, 1, 1, 298), &registry))
        .expect_err("the frozen cap rejects the next job");
    assert_eq!(rejection.error(), ClientError::Capacity);
    assert_eq!(
        rejection.job().key().job_id().get(),
        298,
        "whole job returned"
    );
    assert_eq!(subject.tickets_issued(), 297);

    // A tight cap of two charges plus one: two admitted, the third returned
    // whole with the charge unchanged.
    let limits = limits_with(4096, 4096, job_charge * 2 + 1);
    let mut subject = backend_of(DRIVER, limits);
    for (job_id, section) in [(1u64, 0u8), (2, 1)] {
        subject
            .submit(near_job(
                &near_key_at(1, (0, 0), section, 1, 1, job_id),
                &registry,
            ))
            .expect("admitted under the tight cap");
    }
    let charge_before = subject.charge();
    let rejection = subject
        .submit(near_job(&near_key_at(1, (0, 0), 2, 1, 1, 3), &registry))
        .expect_err("the byte cap plus one rejects");
    assert_eq!(rejection.error(), ClientError::Capacity);
    assert_eq!(rejection.job().key().job_id().get(), 3);
    assert_eq!(subject.charge(), charge_before, "the charge is unchanged");

    // A cap below one job: the first submission rejects before any allocation.
    let limits = limits_with(4096, 4096, job_charge - 1);
    let mut subject = backend_of(DRIVER, limits);
    let rejection = subject
        .submit(near_job(&near_key_at(1, (0, 0), 0, 1, 1, 1), &registry))
        .expect_err("a cap below one job rejects");
    assert_eq!(rejection.error(), ClientError::Capacity);
    assert_eq!(
        rejection.job().key().job_id().get(),
        1,
        "whole job returned"
    );
    assert_eq!(subject.tickets_issued(), 0);
    assert_eq!(subject.charge(), 0);

    // Queue-held plus worker-held summed: a completed result keeps owning its
    // geometry charge while a queued job keeps owning its payload, so the
    // third job admits only after the first drain frees payload bytes.
    let limits = limits_with(4096, 4096, job_charge * 2 + 1);
    let mut subject = backend_of(DRIVER, limits);
    for (job_id, section) in [(1u64, 0u8), (2, 1)] {
        subject
            .submit(near_job(
                &near_key_at(1, (0, 0), section, 1, 1, job_id),
                &registry,
            ))
            .expect("admitted");
    }
    assert!(
        subject
            .submit(near_job(&near_key_at(1, (0, 0), 2, 1, 1, 3), &registry))
            .is_err(),
        "three payloads exceed the cap"
    );
    assert_eq!(
        subject.work(&limits, ClientWorkBudget::try_new(0, 1).expect("budget")),
        1
    );
    subject
        .submit(near_job(&near_key_at(1, (0, 0), 2, 1, 1, 3), &registry))
        .expect("the drained payload frees its byte share for the next job");
}

/// A shared allocation is charged once while the owner holds it; separately
/// allocated equal payloads are separate charges.
#[test]
fn shared_arc_once_versus_distinct_equal() {
    let params = fixture_params();
    let far_charge = far_payload(&params, [1, 1]).owned_bytes();
    let params_charge = std::mem::size_of::<WorldgenParams>();

    // One shared far-params Arc: a cap of two payloads plus the single
    // allocation admits both jobs.
    let limits = limits_with(4096, 4096, far_charge * 2 + params_charge);
    let mut subject = backend_of(ARC_DRIVER, limits);
    subject
        .submit(far_job(&far_key_at(1, [1, 1], 1, 1, 1), &params))
        .expect("first shared job admitted");
    subject
        .submit(far_job(&far_key_at(1, [2, 2], 1, 1, 2), &params))
        .expect("the shared Arc is charged once, not once per holder");

    // Separately allocated equal params: each allocation charges separately,
    // so the tighter cap admits one and returns the second whole.
    let other = fixture_params();
    let limits = limits_with(4096, 4096, far_charge * 2 + params_charge * 2);
    let mut subject = backend_of(ARC_DRIVER, limits);
    subject
        .submit(far_job(&far_key_at(1, [1, 1], 1, 1, 1), &params))
        .expect("first distinct job admitted");
    subject
        .submit(far_job(&far_key_at(1, [2, 2], 1, 1, 2), &other))
        .expect("the second distinct allocation still fits");
    let tight = limits_with(4096, 4096, far_charge * 2 + params_charge * 2 - 1);
    let mut subject = backend_of(ARC_DRIVER, tight);
    subject
        .submit(far_job(&far_key_at(1, [1, 1], 1, 1, 1), &params))
        .expect("first distinct job admitted");
    let rejection = subject
        .submit(far_job(&far_key_at(1, [2, 2], 1, 1, 2), &other))
        .expect_err("distinct equal allocations charge separately");
    assert_eq!(rejection.error(), ClientError::Capacity);
    assert_eq!(
        rejection.job().key().job_id().get(),
        2,
        "whole job returned"
    );

    // The near registry follows the same identity rule.
    let registry = fixture_registry();
    let payload = near_payload(&registry, MIN_Y).owned_bytes();
    let limits = limits_with(4096, 4096, payload * 2 + registry_charge(&registry));
    let mut subject = backend_of(DRIVER, limits);
    subject
        .submit(near_job(&near_key_at(1, (0, 0), 0, 1, 1, 1), &registry))
        .expect("first near job admitted");
    subject
        .submit(near_job(&near_key_at(1, (0, 0), 1, 1, 1, 2), &registry))
        .expect("the shared registry is charged once");
}

/// The work budget: a zero-mesh step retains the FIFO remainder, the drain
/// never exceeds the tighter of the demanded budget and the configured
/// mesh-work ceiling, and the 4097-mesh request itself rejects before any
/// dequeue.
#[test]
fn budget_zero_retains_and_4097_rejects() {
    let registry = fixture_registry();
    let limits = limits_with(2, 4096, 64 << 20);
    let mut subject = backend_of(DRIVER, limits);
    for section in 0..5u8 {
        subject
            .submit(near_job(
                &near_key_at(1, (0, 0), section, 1, 1, u64::from(section) + 1),
                &registry,
            ))
            .expect("admitted");
    }
    assert_eq!(
        subject.work(
            &limits,
            ClientWorkBudget::try_new(0, 0).expect("zero budget")
        ),
        0,
        "a zero-mesh step retains the FIFO remainder"
    );
    assert_eq!(
        subject.work(
            &limits,
            ClientWorkBudget::try_new(0, 4096).expect("large budget")
        ),
        2,
        "the configured mesh ceiling bounds the drain"
    );
    assert_eq!(
        subject.work(
            &limits,
            ClientWorkBudget::try_new(0, 1).expect("one budget")
        ),
        1,
        "the demanded budget bounds the drain"
    );
    assert_eq!(
        subject.work(
            &limits,
            ClientWorkBudget::try_new(0, 4096).expect("large budget")
        ),
        2,
        "the remaining FIFO work drains under the ceiling"
    );
    assert_eq!(
        subject.work(
            &limits,
            ClientWorkBudget::try_new(0, 4096).expect("large budget")
        ),
        0,
        "the queue drains exactly"
    );
    assert!(
        ClientWorkBudget::try_new(0, 4097).is_err(),
        "the 4097-mesh request rejects before any dequeue"
    );
}

/// Results drain in submission order, a delivered geometry stays retained for
/// lookup by its exact key, and an unknown key is a typed invalid input.
#[test]
fn fifo_drain_and_retained_lookup() {
    let registry = fixture_registry();
    let mut subject = backend_of(DRIVER, frozen_limits());
    let mut keys = Vec::new();
    for section in 0..=2u8 {
        let key = near_key_at(1, (0, 0), section, 1, 1, u64::from(section) + 1);
        subject.submit(near_job(&key, &registry)).expect("admitted");
        keys.push(key);
    }
    assert_eq!(
        subject.work(
            &frozen_limits(),
            ClientWorkBudget::try_new(0, 4096).expect("budget")
        ),
        3
    );
    for key in &keys {
        let result = subject.poll().expect("FIFO result");
        assert_eq!(result.key(), key, "submission order");
    }
    assert!(
        subject.poll().is_none(),
        "the completed queue drains exactly"
    );

    let retained = subject
        .lookup(&keys[0])
        .expect("a delivered geometry is retained for lookup");
    assert_eq!(
        retained.quads(),
        0,
        "the retained geometry is the result value"
    );
    let unknown = near_key_at(1, (5, 5), 0, 1, 1, 99);
    assert_eq!(
        subject.lookup(&unknown).unwrap_err(),
        ClientError::InvalidInput,
        "an unknown key is a typed invalid input"
    );
}

/// A newer submitted generation expires an older queued identity of the same
/// terrain key: only the newer result publishes, and the stale payload
/// releases exactly once without publishing.
#[test]
fn newer_generation_beats_stale() {
    let registry = fixture_registry();
    let mut subject = backend_of(DRIVER, frozen_limits());
    let newer = near_key_at(1, (0, 0), 0, 2, 1, 1);
    let stale = near_key_at(1, (0, 0), 0, 1, 1, 2);
    subject
        .submit(near_job(&newer, &registry))
        .expect("admitted");
    subject
        .submit(near_job(&stale, &registry))
        .expect("admitted");
    assert_eq!(
        subject.work(
            &frozen_limits(),
            ClientWorkBudget::try_new(0, 4096).expect("budget")
        ),
        2,
        "both units consume a budget slot"
    );
    let result = subject.poll().expect("the newer generation publishes");
    assert_eq!(result.key().generation(), 2);
    assert!(
        subject.poll().is_none(),
        "the stale generation never publishes"
    );
    assert_eq!(
        subject.lookup(&stale).unwrap_err(),
        ClientError::InvalidInput,
        "the stale identity is never retained"
    );
    subject
        .lookup(&newer)
        .expect("the newer identity is retained");
}

/// Invalidation releases an epoch's pending job, completed result and retained
/// geometry exactly once; a repeated invalidation or late forget releases
/// nothing more, and a job of a retired epoch is a typed stale rejection.
#[test]
fn stale_epoch_releases_once() {
    let registry = fixture_registry();
    let mut subject = backend_of(DRIVER, frozen_limits());
    for (job_id, section) in [(1u64, 0u8), (2, 1), (3, 2)] {
        subject
            .submit(near_job(
                &near_key_at(3, (0, 0), section, 1, 1, job_id),
                &registry,
            ))
            .expect("admitted");
    }
    let limits = frozen_limits();
    assert_eq!(
        subject.work(&limits, ClientWorkBudget::try_new(0, 2).expect("budget")),
        2
    );
    let delivered = subject.poll().expect("delivered and retained");
    let stale_result_charge = geometry_charge(&delivered);
    let pending_charge = near_payload(&registry, MIN_Y).owned_bytes() + registry_charge(&registry);

    let report = subject.invalidate(epoch(3));
    assert_eq!(report.jobs_cancelled(), 1, "the queued job cancels");
    assert_eq!(report.results_stale(), 1, "the completed result releases");
    assert_eq!(
        report.bytes_released(),
        (pending_charge + 2 * stale_result_charge) as u64,
        "queue-held payload plus worker-held and retained geometry release"
    );
    assert_eq!(subject.charge(), 0, "every ownership of the epoch released");
    assert!(subject.poll().is_none());
    assert_eq!(
        subject.lookup(delivered.key()).unwrap_err(),
        ClientError::StaleEpoch,
        "a retired epoch's resource lookup is a typed stale rejection"
    );
    let again = subject.invalidate(epoch(3));
    assert_eq!(again.jobs_cancelled(), 0, "no double release");
    assert_eq!(again.results_stale(), 0);
    assert_eq!(again.bytes_released(), 0);
    assert_eq!(subject.charge(), 0);

    let rejection = subject
        .submit(near_job(&near_key_at(3, (0, 0), 3, 1, 1, 4), &registry))
        .expect_err("a retired epoch never admits new work");
    assert_eq!(rejection.error(), ClientError::StaleEpoch);
    assert_eq!(
        rejection.job().key().job_id().get(),
        4,
        "whole job returned"
    );
    assert_eq!(
        subject.forget(delivered.key()),
        Ok(0),
        "a late forget after invalidation releases nothing"
    );
}

/// Forgetting a retained resource releases its geometry exactly once; a
/// repeated forget and a later reset release nothing more.
#[test]
fn late_forget_releases_retained_once() {
    let registry = fixture_registry();
    let limits = frozen_limits();
    let mut subject = backend_of(DRIVER, limits);
    let key = near_key_at(1, (0, 0), 0, 1, 1, 1);
    subject.submit(near_job(&key, &registry)).expect("admitted");
    assert_eq!(
        subject.work(&limits, ClientWorkBudget::try_new(0, 1).expect("budget")),
        1
    );
    let delivered = subject.poll().expect("delivered and retained");
    let retained_charge = geometry_charge(&delivered);
    let before = subject.charge();
    assert!(before > 0, "the arena owns the retained geometry");

    assert_eq!(
        subject.forget(&key),
        Ok(retained_charge as u64),
        "the forget path releases the retained geometry once"
    );
    assert_eq!(subject.charge(), 0);
    assert_eq!(
        subject.forget(&key),
        Ok(0),
        "a repeated forget is not a double release"
    );
    assert_eq!(
        subject.lookup(&key).unwrap_err(),
        ClientError::InvalidInput,
        "a forgotten resource is no longer retained"
    );
    let report = subject.invalidate(epoch(1));
    assert_eq!(
        report.bytes_released(),
        0,
        "reset after forget releases nothing more"
    );
}

/// An identity completing twice publishes once and releases exactly once.
#[test]
fn duplicate_completion_not_double_release() {
    let registry = fixture_registry();
    let limits = frozen_limits();
    let mut subject = backend_of(DRIVER, limits);
    let key = near_key_at(1, (0, 0), 0, 1, 1, 7);
    subject
        .submit(near_job(&key, &registry))
        .expect("first admission");
    subject
        .submit(near_job(&key, &registry))
        .expect("second admission");
    assert_eq!(
        subject.work(&limits, ClientWorkBudget::try_new(0, 4096).expect("budget")),
        2,
        "both units consume a budget slot"
    );
    let delivered = subject.poll().expect("the first completion delivers");
    assert_eq!(delivered.key(), &key);
    assert!(
        subject.poll().is_none(),
        "the duplicate completion never publishes"
    );
    let report = subject.invalidate(epoch(1));
    assert_eq!(
        report.bytes_released(),
        geometry_charge(&delivered) as u64,
        "exactly one retained geometry releases"
    );
    let again = subject.invalidate(epoch(1));
    assert_eq!(again.bytes_released(), 0, "no double release");
}

/// The permanent artifact: the landed doubles' wrong behaviors the real owner
/// replaces, kept executable so the red stays reproducible.
#[test]
fn wrong_double_preparation_rejected() {
    let registry = fixture_registry();

    // The registered queue double couples completion to polling with no
    // budget surface: everything drains no matter what a step would demand.
    let mut double = QueueDouble::new(frozen_limits());
    for section in 0..=2u8 {
        double
            .try_submit(near_job(
                &near_key_at(1, (0, 0), section, 1, 1, u64::from(section) + 1),
                &registry,
            ))
            .expect("admitted");
    }
    let mut drained = 0u16;
    while double.poll_ready().is_some() {
        drained += 1;
    }
    assert_eq!(drained, 3, "the double ignores any per-step mesh budget");

    // It publishes a stale generation submitted after a newer one.
    double
        .try_submit(near_job(&near_key_at(1, (0, 0), 0, 2, 1, 1), &registry))
        .expect("admitted");
    double
        .try_submit(near_job(&near_key_at(1, (0, 0), 0, 1, 1, 2), &registry))
        .expect("admitted");
    assert!(
        double.poll_ready().is_some(),
        "the newer generation publishes"
    );
    assert!(
        double.poll_ready().is_some(),
        "the double admits the stale generation"
    );

    // And it retains no resource at all: no lookup surface exists on it.

    // The harness faucet charges a shared far-params allocation once per
    // holder, so the second shared job overflows a cap the identity rule
    // admits.
    let params = fixture_params();
    let far_charge = far_payload(&params, [1, 1]).owned_bytes();
    let params_charge = std::mem::size_of::<WorldgenParams>();
    let mut faucet =
        PreparationFaucet::new(limits_with(4096, 4096, far_charge * 2 + params_charge));
    faucet
        .try_submit(far_job(&far_key_at(1, [1, 1], 1, 1, 1), &params))
        .expect("first shared job admitted");
    assert!(
        faucet
            .try_submit(far_job(&far_key_at(1, [2, 2], 1, 1, 2), &params))
            .is_err(),
        "the faucet double-counts the shared Arc"
    );
}
