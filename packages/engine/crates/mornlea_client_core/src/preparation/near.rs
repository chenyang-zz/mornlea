//! Bounded near admission from the one immutable confirmed world owner.

use std::collections::BTreeMap;
use std::ops::Bound::{Excluded, Included};
use std::sync::Arc;

use mornlea_domain::{ChunkPos, Dimension};
use mornlea_engine::native::contracts::MeshRegistry;

use super::{
    PreparationJob, PreparationPayload, PreparationQueue, PreparedResourceKey, SectionKey,
    TerrainKey,
};
use crate::contracts::{ClientError, ClientLimits, ClientWorkBudget, SessionEpoch};
use crate::presentation::family_terrain::TerrainPublisher;
use crate::session::WorldConfirmed;

const PAYLOAD_BYTES: usize = 110592 * 2 + 9 + 9 * 256 * 2 + 4 + 8;

/// Admission metadata follows queue ownership; only the confirmed world supplies blocks.
pub(crate) struct NearScheduler {
    epoch: SessionEpoch,
    limits: ClientLimits,
    admitted: BTreeMap<SectionKey, (PreparedResourceKey, [Option<u64>; 9])>,
    cursor: Option<(i32, i32, u8)>,
    next_job_id: u64,
}

impl NearScheduler {
    pub(crate) fn new(epoch: SessionEpoch, limits: ClientLimits) -> Self {
        Self {
            epoch,
            limits,
            admitted: BTreeMap::new(),
            cursor: None,
            next_job_id: 1,
        }
    }

    pub(crate) fn dispatch(
        &mut self,
        world: &WorldConfirmed,
        dimension: Dimension,
        registry: &Arc<MeshRegistry>,
        publisher: &mut TerrainPublisher,
        queue: &mut PreparationQueue,
        budget: ClientWorkBudget,
    ) -> Result<usize, ClientError> {
        let allowance = usize::from(budget.meshes());
        if allowance == 0 {
            return Ok(0);
        }
        self.admitted.retain(|section, (key, _)| {
            key.dimension() == dimension
                && queue.owns_key(key)
                && world.mesh_stamps(dimension, section.chunk()).is_ok()
        });
        let mut admitted = 0;
        let mut first = None;
        for _ in 0..24 * allowance {
            let Some(section) = self.candidate(world, dimension)? else {
                break;
            };
            if first == Some(section) {
                break;
            }
            first.get_or_insert(section);
            let chunk = section.chunk();
            // Metadata-only columns skip as a unit without fabricating loaded air.
            let present = world.chunks().contains_key(&(
                dimension.get(),
                i64::from(chunk.x()),
                i64::from(chunk.z()),
            ));
            let stamps = match world.mesh_stamps(dimension, chunk) {
                Ok(stamps) => stamps,
                Err(ClientError::InvalidInput)
                    if present && !has_center(world, dimension, chunk)? =>
                {
                    self.cursor = Some((chunk.x(), chunk.z(), 24));
                    continue;
                }
                Err(error) => return Err(error),
            };
            let previous = self.admitted.get(&section);
            if previous.is_some_and(|(_, held)| *held == stamps) {
                self.advance(section);
                continue;
            }
            if PAYLOAD_BYTES > self.limits.preparation_bytes() {
                return Err(ClientError::Capacity);
            }
            if previous.is_none() && self.admitted.len() >= self.limits.preparation_results()
                || self
                    .limits
                    .preparation_bytes()
                    .saturating_sub(queue.charge())
                    < PAYLOAD_BYTES
            {
                break;
            }
            let next = self
                .next_job_id
                .checked_add(1)
                .ok_or(ClientError::Capacity)?;
            let key = PreparedResourceKey::try_new(
                self.epoch,
                dimension,
                TerrainKey::Section(section),
                self.next_job_id,
                stamps[4].ok_or(ClientError::InvalidInput)?,
                self.next_job_id,
            )?;
            let payload = world.mesh_view(dimension, section, registry)?;
            let job = PreparationJob::try_new(key, PreparationPayload::Near(payload))?;
            match publisher.admit_near(queue, job) {
                Ok(_) => {
                    self.admitted.insert(section, (key, stamps));
                    self.next_job_id = next;
                    admitted += 1;
                    self.advance(section);
                    if admitted == allowance {
                        break;
                    }
                }
                Err(rejected) if rejected.error() == ClientError::Capacity => break,
                Err(rejected) => return Err(rejected.error()),
            }
        }
        Ok(admitted)
    }

    fn advance(&mut self, section: SectionKey) {
        self.cursor = Some((
            section.chunk().x(),
            section.chunk().z(),
            section.section() + 1,
        ));
    }

    fn candidate(
        &self,
        world: &WorldConfirmed,
        dimension: Dimension,
    ) -> Result<Option<SectionKey>, ClientError> {
        let upper = Included((dimension.get(), i64::MAX, i64::MAX));
        let lower = self.cursor.map_or(
            Included((dimension.get(), i64::MIN, i64::MIN)),
            |(x, z, si)| {
                let key = (dimension.get(), i64::from(x), i64::from(z));
                if si == 24 {
                    Excluded(key)
                } else {
                    Included(key)
                }
            },
        );
        let candidate = world.chunks().range((lower, upper)).next().or_else(|| {
            world
                .chunks()
                .range((Included((dimension.get(), i64::MIN, i64::MIN)), upper))
                .next()
        });
        let Some((&(_, x, z), _)) = candidate else {
            return Ok(None);
        };
        let x = i32::try_from(x).map_err(|_| ClientError::InvalidInput)?;
        let z = i32::try_from(z).map_err(|_| ClientError::InvalidInput)?;
        let si = self
            .cursor
            .filter(|(cx, cz, si)| *cx == x && *cz == z && *si < 24)
            .map_or(0, |(_, _, si)| si);
        SectionKey::try_new(ChunkPos::new(x, z), si).map(Some)
    }

    #[cfg(test)]
    fn metadata_count(&self) -> usize {
        self.admitted.len()
    }
}

fn has_center(
    world: &WorldConfirmed,
    dimension: Dimension,
    chunk: ChunkPos,
) -> Result<bool, ClientError> {
    let x = i32::try_from(i64::from(chunk.x()) * 16).map_err(|_| ClientError::InvalidInput)?;
    let z = i32::try_from(i64::from(chunk.z()) * 16).map_err(|_| ClientError::InvalidInput)?;
    Ok(world
        .block_at(
            dimension,
            mornlea_domain::BlockPos::new(x, mornlea_protocol::MIN_Y, z),
        )
        .is_some())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contracts::PreparationPort;
    use crate::preparation::PreparedGeometry;
    use crate::runtime_data::mesh_registry;
    use mornlea_domain::{
        BlockChange, BlockChanges, BlockChangesParts, BlockPos, ChunkSnapshot, ChunkSnapshotParts,
        PalettedSection,
    };

    fn limits(records: usize, bytes: usize) -> ClientLimits {
        let b = ClientLimits::try_new().unwrap();
        ClientLimits::try_new_with(
            b.queued_input_events(),
            b.inbound_observations(),
            b.inbound_bytes(),
            b.outbound_commands(),
            b.outbound_bytes(),
            b.prediction_journal(),
            b.message_work(),
            b.mesh_work(),
            records,
            bytes,
            b.family_records(),
            b.frame_bytes(),
        )
        .unwrap()
    }

    fn column(world: &mut WorldConfirmed, dimension: Dimension, chunk: ChunkPos, revision: u64) {
        world
            .apply_snapshot(
                &ChunkSnapshot::try_new(ChunkSnapshotParts {
                    dimension,
                    chunk,
                    revision,
                    sections: Box::new(std::array::from_fn(|_| {
                        PalettedSection::single(0).unwrap()
                    })),
                })
                .unwrap(),
                &ClientLimits::try_new().unwrap(),
            )
            .unwrap();
    }

    fn write(world: &mut WorldConfirmed, chunk: ChunkPos, base: u64, y: i32, id: u16) {
        let changes = BlockChanges::try_new(BlockChangesParts {
            dimension: Dimension::OVERWORLD,
            chunk,
            base_revision: base,
            new_revision: base + 1,
            changes: vec![
                BlockChange::try_new(
                    BlockPos::new(chunk.x() * 16 + 3, y, chunk.z() * 16 + 11),
                    id,
                )
                .unwrap(),
            ]
            .into_boxed_slice(),
        })
        .unwrap();
        world
            .apply_changes(&changes, &ClientLimits::try_new().unwrap())
            .unwrap();
    }

    fn owners(limits: ClientLimits) -> (NearScheduler, PreparationQueue, TerrainPublisher) {
        (
            NearScheduler::new(SessionEpoch::try_new(1).unwrap(), limits),
            PreparationQueue::new(limits),
            TerrainPublisher::try_new().unwrap(),
        )
    }

    fn stage(
        world: &WorldConfirmed,
        epoch: SessionEpoch,
        limits: ClientLimits,
        publisher: &mut TerrainPublisher,
        queue: &mut PreparationQueue,
        selection: &mut crate::preparation::lod::LodSelection,
        budget: ClientWorkBudget,
        far_jobs: u16,
    ) -> Result<crate::presentation::family_terrain::TerrainPublication, ClientError> {
        use crate::contracts::{ConfirmedRevision, SessionPhase};
        use crate::input::InputProjectionState;
        use crate::prediction::PlayerProjectionState;
        use crate::presentation::{
            AudioProjectionState, DiagnosticProjectionState, ErrorClassCounters,
            LifecycleProjectionState, MovementIntent, Pose, ProducerIdentity, ProjectionView,
            QueueCounters,
        };
        use crate::session::{ConfirmedMirror, ConfirmedMirrorParts};
        let revision = ConfirmedRevision::new(1);
        let mirror = ConfirmedMirror::try_new(ConfirmedMirrorParts {
            epoch,
            revision,
            phase: SessionPhase::Admitted,
            world: Some(world.clone()),
            actors: None,
            inventory: None,
            world_ui: None,
        })
        .unwrap();
        let input = InputProjectionState::try_new(1).unwrap();
        let player = PlayerProjectionState::try_new(
            epoch,
            revision,
            Pose::try_new([0.0; 3], 0.0, 0.0).unwrap(),
            None,
            Vec::new(),
            None,
            None,
            MovementIntent::try_new(None, true).unwrap(),
        )
        .unwrap();
        let audio = AudioProjectionState::try_new().unwrap();
        let lifecycle = LifecycleProjectionState::try_new(1).unwrap();
        let diagnostics = DiagnosticProjectionState::try_new(
            ProducerIdentity::try_new([0; 20], [0; 32]).unwrap(),
            QueueCounters::try_new(0, 0, 0, 0, 0, 0, 0).unwrap(),
            ErrorClassCounters::try_new(0, 0, 0, 0, 0, 0).unwrap(),
        )
        .unwrap();
        let view = ProjectionView::try_new(
            &mirror,
            &[],
            &input,
            &player,
            &audio,
            &lifecycle,
            &diagnostics,
            epoch,
            revision,
            1,
            &limits,
        )
        .unwrap();
        let params = crate::runtime_data::worldgen_params(42, Dimension::OVERWORLD, true).unwrap();
        publisher.prepare_with_dispatch_limit(
            &view,
            queue,
            selection,
            ChunkPos::new(0, 0),
            &params,
            budget,
            far_jobs,
        )
    }

    #[test]
    fn real_near_publication_replacement_survives_failed_frame_and_retires_on_retry() {
        use crate::contracts::{ConfirmedRevision, FAMILY_TERRAIN, FamilyKey};
        use crate::preparation::{LodConfig, LodStep};
        use crate::presentation::frame::{FamilyFrame, FamilyRecords, PresentationFrame};
        let limits = ClientLimits::try_new().unwrap();
        let registry = mesh_registry().unwrap();
        let chunk = ChunkPos::new(-2, -6);
        let mut world = WorldConfirmed::default();
        for dx in -1..=1 {
            for dz in -1..=1 {
                column(
                    &mut world,
                    Dimension::OVERWORLD,
                    ChunkPos::new(chunk.x() + dx, chunk.z() + dz),
                    1,
                );
            }
        }
        write(&mut world, chunk, 1, -56, 2);
        let (mut scheduler, mut queue, mut publisher) = owners(limits);
        let one = ClientWorkBudget::try_new(0, 1).unwrap();
        let mut selection = crate::preparation::lod::LodSelection::try_new(
            scheduler.epoch,
            Dimension::OVERWORLD,
            LodConfig::try_new(
                true,
                2,
                3,
                LodStep::Four,
                24 * super::super::lod::static_charge_bytes(LodStep::Four),
            )
            .unwrap(),
            1,
        )
        .unwrap();
        scheduler.cursor = Some((chunk.x(), chunk.z(), 0));
        assert_eq!(
            scheduler
                .dispatch(
                    &world,
                    Dimension::OVERWORLD,
                    &registry,
                    &mut publisher,
                    &mut queue,
                    one
                )
                .unwrap(),
            1
        );
        let first = stage(
            &world,
            scheduler.epoch,
            limits,
            &mut publisher,
            &mut queue,
            &mut selection,
            one,
            0,
        )
        .unwrap();
        assert_eq!(first.processed_meshes(), 1);
        assert_eq!(queue.tickets_issued(), 1);
        assert_eq!(selection.in_flight_len(), 0);
        let old = *first.records()[0].resource().unwrap();
        assert!(
            matches!(queue.prepared_resource(&old).unwrap().as_ref(),PreparedGeometry::Near(quads) if quads.len()==6)
        );
        publisher.commit(&mut queue, first);
        column(
            &mut world,
            Dimension::OVERWORLD,
            ChunkPos::new(chunk.x() + 1, chunk.z()),
            2,
        );
        scheduler.cursor = Some((chunk.x(), chunk.z(), 0));
        assert_eq!(
            scheduler
                .dispatch(
                    &world,
                    Dimension::OVERWORLD,
                    &registry,
                    &mut publisher,
                    &mut queue,
                    one
                )
                .unwrap(),
            1
        );
        let replacement = stage(
            &world,
            scheduler.epoch,
            limits,
            &mut publisher,
            &mut queue,
            &mut selection,
            one,
            0,
        )
        .unwrap();
        assert_eq!(replacement.processed_meshes(), 1);
        assert_eq!(replacement.records().len(), 2);
        let new = *replacement.records()[1].resource().unwrap();
        assert_eq!(new.content_revision(), old.content_revision());
        assert!(new.generation() > old.generation());
        let wrong_epoch = PresentationFrame::try_new(
            SessionEpoch::try_new(2).unwrap(),
            ConfirmedRevision::new(1),
            1,
            vec![
                FamilyFrame::try_new(
                    FamilyKey::try_new(FAMILY_TERRAIN).unwrap(),
                    FamilyRecords::Terrain(replacement.records().to_vec()),
                )
                .unwrap(),
            ],
        );
        assert_eq!(
            wrong_epoch.unwrap().validate(&limits),
            Err(ClientError::InvalidInput)
        );
        drop(replacement);
        assert_eq!(publisher.published_resources(), &[old]);
        assert!(queue.prepared_resource(&old).is_ok());
        let retry = stage(
            &world,
            scheduler.epoch,
            limits,
            &mut publisher,
            &mut queue,
            &mut selection,
            ClientWorkBudget::try_new(0, 0).unwrap(),
            0,
        )
        .unwrap();
        assert_eq!(retry.processed_meshes(), 0);
        assert_eq!(retry.records().len(), 2);
        publisher.commit(&mut queue, retry);
        assert_eq!(publisher.published_resources(), &[new]);
        assert!(queue.prepared_resource(&old).is_err());
        assert_eq!(queue.retained_resources(), 1);
    }

    #[test]
    fn remaining_far_allowance_bounds_admissions_and_reports_only_actual_work() {
        use crate::preparation::{LodConfig, LodStep};
        let limits = ClientLimits::try_new().unwrap();
        let world = WorldConfirmed::default();
        let (scheduler, mut queue, mut publisher) = owners(limits);
        let one = ClientWorkBudget::try_new(0, 1).unwrap();
        let mut selection = crate::preparation::lod::LodSelection::try_new(
            scheduler.epoch,
            Dimension::OVERWORLD,
            LodConfig::try_new(
                true,
                2,
                3,
                LodStep::Four,
                24 * super::super::lod::static_charge_bytes(LodStep::Four),
            )
            .unwrap(),
            1,
        )
        .unwrap();
        let empty = stage(
            &world,
            scheduler.epoch,
            limits,
            &mut publisher,
            &mut queue,
            &mut selection,
            one,
            1,
        )
        .unwrap();
        assert_eq!(empty.processed_meshes(), 0);
        assert_eq!(queue.tickets_issued(), 1);
        assert_eq!(queue.pending_jobs(), 1);
        publisher.commit(&mut queue, empty);
        let built = stage(
            &world,
            scheduler.epoch,
            limits,
            &mut publisher,
            &mut queue,
            &mut selection,
            one,
            0,
        )
        .unwrap();
        assert_eq!(built.processed_meshes(), 1);
        assert_eq!(queue.tickets_issued(), 1);
        assert_eq!(queue.pending_jobs(), 0);
        assert_eq!(built.records().len(), 1);
        publisher.commit(&mut queue, built);
    }

    #[test]
    fn real_near_admission_builds_six_stone_quads_and_unchanged_rotation_has_no_new_ticket() {
        let limits = ClientLimits::try_new().unwrap();
        let registry = mesh_registry().unwrap();
        let chunk = ChunkPos::new(-2, -6);
        let mut world = WorldConfirmed::default();
        column(&mut world, Dimension::OVERWORLD, chunk, 1);
        for dx in -1..=1 {
            for dz in -1..=1 {
                if dx != 0 || dz != 0 {
                    column(
                        &mut world,
                        Dimension::OVERWORLD,
                        ChunkPos::new(chunk.x() + dx, chunk.z() + dz),
                        1,
                    );
                }
            }
        }
        write(&mut world, chunk, 1, -56, 2);
        let (mut scheduler, mut queue, mut publisher) = owners(limits);
        scheduler.cursor = Some((chunk.x(), chunk.z(), 0));
        let budget = ClientWorkBudget::try_new(0, 216).unwrap();
        assert_eq!(
            scheduler
                .dispatch(
                    &world,
                    Dimension::OVERWORLD,
                    &registry,
                    &mut publisher,
                    &mut queue,
                    budget
                )
                .unwrap(),
            216
        );
        assert_eq!(queue.work(budget), 216);
        let mut stone = None;
        while let Some(result) = queue.poll_ready() {
            if *result.key().key() == TerrainKey::Section(SectionKey::try_new(chunk, 0).unwrap()) {
                let geometry = result.outcome().as_ref().unwrap();
                assert!(
                    matches!(geometry.as_ref(),PreparedGeometry::Near(quads) if quads.len()==6)
                );
                stone = Some(*result.key());
            }
        }
        assert!(stone.is_some());
        assert_eq!(scheduler.metadata_count(), 216);
        let tickets = queue.tickets_issued();
        assert_eq!(
            scheduler
                .dispatch(
                    &world,
                    Dimension::OVERWORLD,
                    &registry,
                    &mut publisher,
                    &mut queue,
                    ClientWorkBudget::try_new(0, 1).unwrap()
                )
                .unwrap(),
            0
        );
        assert_eq!(queue.tickets_issued(), tickets);
        assert!(queue.owns_key(&stone.unwrap()));
    }

    #[test]
    fn changed_neighbor_admits_new_generation_without_changing_center_revision() {
        let limits = ClientLimits::try_new().unwrap();
        let registry = mesh_registry().unwrap();
        let chunk = ChunkPos::new(-2, -6);
        let neighbor = ChunkPos::new(-1, -6);
        let mut world = WorldConfirmed::default();
        column(&mut world, Dimension::OVERWORLD, chunk, 1);
        column(&mut world, Dimension::OVERWORLD, neighbor, 1);
        let (mut scheduler, mut queue, mut publisher) = owners(limits);
        scheduler.cursor = Some((chunk.x(), chunk.z(), 0));
        let budget = ClientWorkBudget::try_new(0, 1).unwrap();
        scheduler
            .dispatch(
                &world,
                Dimension::OVERWORLD,
                &registry,
                &mut publisher,
                &mut queue,
                budget,
            )
            .unwrap();
        queue.work(budget);
        let first = queue.poll_ready().unwrap();
        column(&mut world, Dimension::OVERWORLD, neighbor, 2);
        scheduler.cursor = Some((chunk.x(), chunk.z(), 0));
        scheduler
            .dispatch(
                &world,
                Dimension::OVERWORLD,
                &registry,
                &mut publisher,
                &mut queue,
                budget,
            )
            .unwrap();
        queue.work(budget);
        let second = queue.poll_ready().unwrap();
        assert_eq!(
            second.key().content_revision(),
            first.key().content_revision()
        );
        assert!(second.key().generation() > first.key().generation());
        assert_ne!(second.key().job_id(), first.key().job_id());
        assert!(queue.prepared_resource(first.key()).is_ok());
    }

    #[test]
    fn zero_budget_metadata_and_byte_caps_preserve_retry_source() {
        let registry = mesh_registry().unwrap();
        let chunk = ChunkPos::new(0, 0);
        let mut world = WorldConfirmed::default();
        column(&mut world, Dimension::OVERWORLD, chunk, 1);
        let (mut scheduler, mut queue, mut publisher) = owners(limits(1, 64 << 20));
        let one = ClientWorkBudget::try_new(0, 1).unwrap();
        assert_eq!(
            scheduler
                .dispatch(
                    &world,
                    Dimension::OVERWORLD,
                    &registry,
                    &mut publisher,
                    &mut queue,
                    ClientWorkBudget::try_new(0, 0).unwrap()
                )
                .unwrap(),
            0
        );
        assert_eq!(scheduler.cursor, None);
        assert_eq!(queue.tickets_issued(), 0);
        assert_eq!(
            scheduler
                .dispatch(
                    &world,
                    Dimension::OVERWORLD,
                    &registry,
                    &mut publisher,
                    &mut queue,
                    one
                )
                .unwrap(),
            1
        );
        let cursor = scheduler.cursor;
        let next = scheduler.next_job_id;
        assert_eq!(
            scheduler
                .dispatch(
                    &world,
                    Dimension::OVERWORLD,
                    &registry,
                    &mut publisher,
                    &mut queue,
                    one
                )
                .unwrap(),
            0
        );
        assert_eq!(scheduler.cursor, cursor);
        assert_eq!(scheduler.next_job_id, next);
        assert_eq!(scheduler.metadata_count(), 1);
        queue.work(one);
        let first = queue.poll_ready().unwrap();
        queue.forget(first.key()).unwrap();
        assert_eq!(
            scheduler
                .dispatch(
                    &world,
                    Dimension::OVERWORLD,
                    &registry,
                    &mut publisher,
                    &mut queue,
                    one
                )
                .unwrap(),
            1
        );
        let (mut tiny, mut tiny_queue, mut tiny_publisher) = owners(limits(1, 1));
        assert_eq!(
            tiny.dispatch(
                &world,
                Dimension::OVERWORLD,
                &registry,
                &mut tiny_publisher,
                &mut tiny_queue,
                one
            ),
            Err(ClientError::Capacity)
        );
        assert_eq!(tiny_queue.tickets_issued(), 0);
        assert_eq!(tiny.metadata_count(), 0);
        assert_eq!(tiny.next_job_id, 1);
    }

    #[test]
    fn ordinary_metadata_skips_but_coordinate_and_job_id_overflow_reject_before_admission() {
        let limits = ClientLimits::try_new().unwrap();
        let registry = mesh_registry().unwrap();
        let mut world =
            WorldConfirmed::default().with_chunk(Dimension::OVERWORLD, ChunkPos::new(0, 0), 1);
        let chunk = ChunkPos::new(1, 0);
        column(&mut world, Dimension::OVERWORLD, chunk, 1);
        let (mut scheduler, mut queue, mut publisher) = owners(limits);
        let one = ClientWorkBudget::try_new(0, 1).unwrap();
        assert_eq!(
            scheduler
                .dispatch(
                    &world,
                    Dimension::OVERWORLD,
                    &registry,
                    &mut publisher,
                    &mut queue,
                    one
                )
                .unwrap(),
            1
        );
        queue.work(one);
        assert_eq!(
            *queue.poll_ready().unwrap().key().key(),
            TerrainKey::Section(SectionKey::try_new(chunk, 0).unwrap())
        );
        for extreme in [ChunkPos::new(i32::MAX, 0), ChunkPos::new(0, i32::MIN)] {
            let world = WorldConfirmed::default().with_chunk(Dimension::OVERWORLD, extreme, 1);
            let (mut scheduler, mut queue, mut publisher) = owners(limits);
            assert_eq!(
                scheduler.dispatch(
                    &world,
                    Dimension::OVERWORLD,
                    &registry,
                    &mut publisher,
                    &mut queue,
                    one
                ),
                Err(ClientError::InvalidInput)
            );
            assert_eq!(queue.tickets_issued(), 0);
            assert_eq!(queue.charge(), 0);
            assert_eq!(scheduler.metadata_count(), 0);
        }
        let (mut scheduler, mut queue, mut publisher) = owners(limits);
        scheduler.next_job_id = u64::MAX;
        assert_eq!(
            scheduler.dispatch(
                &world,
                Dimension::OVERWORLD,
                &registry,
                &mut publisher,
                &mut queue,
                one
            ),
            Err(ClientError::Capacity)
        );
        assert_eq!(queue.tickets_issued(), 0);
        assert_eq!(queue.charge(), 0);
        assert_eq!(scheduler.next_job_id, u64::MAX);
    }

    #[test]
    fn dimension_and_forget_prune_metadata_without_releasing_published_resources() {
        let limits = ClientLimits::try_new().unwrap();
        let registry = mesh_registry().unwrap();
        let chunk = ChunkPos::new(0, 0);
        let mut world = WorldConfirmed::default();
        column(&mut world, Dimension::OVERWORLD, chunk, 1);
        column(&mut world, Dimension::DEPTHS, chunk, 1);
        let (mut scheduler, mut queue, mut publisher) = owners(limits);
        let one = ClientWorkBudget::try_new(0, 1).unwrap();
        scheduler
            .dispatch(
                &world,
                Dimension::OVERWORLD,
                &registry,
                &mut publisher,
                &mut queue,
                one,
            )
            .unwrap();
        let mut selection = crate::preparation::lod::LodSelection::try_new(
            scheduler.epoch,
            Dimension::OVERWORLD,
            crate::preparation::LodConfig::try_new(
                false,
                2,
                3,
                crate::preparation::LodStep::Four,
                1,
            )
            .unwrap(),
            1,
        )
        .unwrap();
        let first = stage(
            &world,
            scheduler.epoch,
            limits,
            &mut publisher,
            &mut queue,
            &mut selection,
            one,
            0,
        )
        .unwrap();
        let old = *first.records()[0].resource().unwrap();
        publisher.commit(&mut queue, first);
        assert_eq!(publisher.published_resources(), &[old]);
        scheduler
            .dispatch(
                &world,
                Dimension::DEPTHS,
                &registry,
                &mut publisher,
                &mut queue,
                one,
            )
            .unwrap();
        assert_eq!(scheduler.metadata_count(), 1);
        assert!(queue.prepared_resource(&old).is_ok());
        assert_eq!(publisher.published_resources(), &[old]);
        world.remove_chunk(Dimension::DEPTHS, chunk);
        assert_eq!(
            scheduler
                .dispatch(
                    &world,
                    Dimension::DEPTHS,
                    &registry,
                    &mut publisher,
                    &mut queue,
                    one
                )
                .unwrap(),
            0
        );
        assert_eq!(scheduler.metadata_count(), 0);
        assert!(queue.prepared_resource(&old).is_ok());
        assert_eq!(publisher.published_resources(), &[old]);
        column(&mut world, Dimension::DEPTHS, chunk, 1);
        assert_eq!(
            scheduler
                .dispatch(
                    &world,
                    Dimension::DEPTHS,
                    &registry,
                    &mut publisher,
                    &mut queue,
                    one
                )
                .unwrap(),
            1
        );
    }
}
