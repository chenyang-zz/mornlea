use super::preparation_port::{far_payload, fixture_params};
use mornlea_client_core::preparation::{OwnedMeshView, PreparationQueue};
use mornlea_client_core::{
    ClientLimits, ClientWorkBudget, PreparationJob, PreparationPayload, PreparationPort,
    PreparedGeometry, PreparedResourceKey, SectionKey, SessionEpoch, TerrainKey, TilePos,
};
use mornlea_domain::{ChunkPos, Dimension};
use mornlea_engine::native::contracts::{
    LodOp, LodQuad, LodScratch, MeshModel, MeshOp, MeshQuad, MeshRegistryEntry, MeshScratch,
};
use mornlea_engine::native::lod::NativeLod;
use mornlea_engine::native::mesh::{NativeMesh, try_new_registry};
use std::sync::Arc;

#[test]
fn preparation_builds_and_retains_the_actual_near_mesh() {
    let entry = |id, opaque| MeshRegistryEntry {
        id,
        opaque,
        emission: 0,
        material: [1; 6],
        fluid_height: 0,
        light_attenuation: 0,
        block_top_raw: 0,
        model: MeshModel::Default,
    };
    let registry = Arc::new(
        try_new_registry(
            &[entry(0, false), entry(1, true), entry(2, true)],
            &[0, 0, 1],
            0,
            1,
        )
        .unwrap(),
    );
    let mut blocks = vec![0u16; 110592].into_boxed_slice();
    blocks[13 * 4096 + 8 * 256 + 8 * 16 + 8] = 2;
    let near = OwnedMeshView::try_new(
        blocks.try_into().unwrap(),
        [false; 9],
        vec![[0; 256]; 9].into_boxed_slice().try_into().unwrap(),
        -64,
        registry,
    )
    .unwrap();
    let mut scratch = MeshScratch::try_new().unwrap();
    let mut expected = vec![MeshQuad::default(); PreparedGeometry::MAX_SECTION_QUADS];
    let count = NativeMesh
        .mesh(&near.as_mesh_view(), &mut scratch, &mut expected)
        .unwrap();
    expected.truncate(count);
    assert!(count > 0);
    let epoch = SessionEpoch::try_new(1).unwrap();
    let key = PreparedResourceKey::try_new(
        epoch,
        Dimension::OVERWORLD,
        TerrainKey::Section(SectionKey::try_new(ChunkPos::new(0, 0), 0).unwrap()),
        1,
        1,
        1,
    )
    .unwrap();
    let mut queue = PreparationQueue::new(ClientLimits::try_new().unwrap());
    queue
        .try_submit(PreparationJob::try_new(key, PreparationPayload::Near(near)).unwrap())
        .unwrap();
    assert_eq!(queue.work(ClientWorkBudget::try_new(0, 1).unwrap()), 1);
    let result = queue.poll_ready().unwrap();
    assert_eq!(
        result.outcome().as_ref().unwrap().as_ref(),
        &PreparedGeometry::Near(expected)
    );
    assert!(Arc::ptr_eq(
        result.outcome().as_ref().unwrap(),
        &queue.prepared_resource(&key).unwrap()
    ));
    let charge = queue.charge();
    assert_eq!(queue.invalidate(epoch).bytes_released(), charge as u64);
    assert_eq!(queue.charge(), 0);
    assert!(matches!(
        queue.prepared_resource(&key),
        Err(mornlea_client_core::ClientError::StaleEpoch)
    ));
}

#[test]
fn preparation_builds_the_actual_far_lod_geometry() {
    let params = fixture_params();
    let far = far_payload(&params, [0, 0]);
    let mut scratch = LodScratch::try_new().unwrap();
    let mut expected = vec![LodQuad::default(); PreparedGeometry::MAX_LOD_QUADS];
    let count = NativeLod
        .build(&far.as_lod_request(), &mut scratch, &mut expected)
        .unwrap();
    expected.truncate(count);
    assert!(count > 0);
    let key = PreparedResourceKey::try_new(
        SessionEpoch::try_new(1).unwrap(),
        Dimension::OVERWORLD,
        TerrainKey::LodTile(TilePos::new(0, 0)),
        1,
        1,
        1,
    )
    .unwrap();
    let mut queue = PreparationQueue::new(ClientLimits::try_new().unwrap());
    queue
        .try_submit(PreparationJob::try_new(key, PreparationPayload::Far(far)).unwrap())
        .unwrap();
    queue.work(ClientWorkBudget::try_new(0, 1).unwrap());
    assert_eq!(
        queue
            .poll_ready()
            .unwrap()
            .outcome()
            .as_ref()
            .unwrap()
            .as_ref(),
        &PreparedGeometry::Far(expected)
    );
}
