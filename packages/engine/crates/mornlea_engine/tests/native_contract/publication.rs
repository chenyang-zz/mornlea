//! Typed destination atomicity on `OutputTooSmall`: a one-slot-short
//! destination reports the exact needed count and leaves every destination
//! slot untouched.

use mornlea_engine::native::contracts::KernelError;
use mornlea_engine::native::contracts::fluid::{
    FluidRescanOp, FluidWrites, RescanRange, RescanRequest, RescanScratch, RescanSection,
    RescanView,
};
use mornlea_engine::native::contracts::mesh::{MeshModel, MeshOp, MeshQuad};
use mornlea_engine::native::contracts::world::{
    LodRequest, LodScratch, LodStep, Materials, WorldgenParams,
};
use mornlea_engine::native::contracts::{FluidEvalOp, LodOp};
use mornlea_engine::native::fluid_eval::NativeFluidEval;
use mornlea_engine::native::fluid_rescan::NativeFluidRescan;
use mornlea_engine::native::lod::NativeLod;
use mornlea_engine::native::mesh::{NativeMesh, try_new_registry};

const STAGE_QUADS: usize = 3136;

fn worldgen_params() -> WorldgenParams {
    let mut perm = [0u8; 512];
    for (index, entry) in perm.iter_mut().enumerate() {
        *entry = (index & 255) as u8;
    }
    WorldgenParams::try_new(
        42,
        Materials {
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
        },
        perm,
    )
    .unwrap()
}

type MeshViewParts = (Box<[u16; 110592]>, Box<[bool; 9]>, Box<[[i16; 256]; 9]>);

#[allow(clippy::type_complexity)]
fn mesh_view_parts() -> MeshViewParts {
    let mut blocks = Box::new([0_u16; 110592]);
    // One isolated opaque stone in the center section: index 13 * 4096 + 8*256
    // + 8*16 + 8 places it at local (8, 8, 8) of the middle section.
    blocks[13 * 4096 + 8 * 256 + 8 * 16 + 8] = 2;
    (blocks, Box::new([false; 9]), Box::new([[0; 256]; 9]))
}

#[test]
fn mesh_short_destination_reports_needed_and_keeps_canary() {
    use mornlea_engine::native::contracts::mesh::{MeshRegistryEntry, MeshScratch, MeshView};

    let registry = try_new_registry(
        &[
            MeshRegistryEntry {
                id: 0,
                opaque: false,
                emission: 0,
                material: [0; 6],
                fluid_height: 0,
                light_attenuation: 0,
                block_top_raw: 0,
                model: MeshModel::Default,
            },
            MeshRegistryEntry {
                id: 1,
                opaque: true,
                material: [1; 6],
                emission: 0,
                fluid_height: 0,
                light_attenuation: 0,
                block_top_raw: 0,
                model: MeshModel::Default,
            },
            MeshRegistryEntry {
                id: 2,
                opaque: true,
                material: [10, 11, 12, 13, 14, 15],
                emission: 0,
                fluid_height: 0,
                light_attenuation: 0,
                block_top_raw: 0,
                model: MeshModel::Default,
            },
        ],
        &[0, 0, 1],
        0,
        1,
    )
    .unwrap();
    let (blocks, heights_present, heights) = mesh_view_parts();
    let view = MeshView {
        blocks: &blocks,
        heights_present: &heights_present,
        heights: &heights,
        section_origin_y: 0,
        registry: &registry,
    };
    let mut scratch = MeshScratch::try_new().unwrap();

    let mut probe = vec![MeshQuad::default(); STAGE_QUADS.max(40960)];
    let needed = NativeMesh.mesh(&view, &mut scratch, &mut probe).unwrap();
    assert!(needed > 0);

    // Fill the short destination with the known-good prefix plus one
    // trailing probe quad: a partial publish would overwrite the canary
    // tail, and the equality check below would fail.
    let mut short = vec![MeshQuad::default(); needed - 1];
    short.clone_from_slice(&probe[..needed - 1]);
    let canary = short.clone();
    assert_eq!(
        NativeMesh.mesh(&view, &mut scratch, &mut short),
        Err(KernelError::OutputTooSmall {
            needed,
            available: needed - 1,
        })
    );
    assert_eq!(short, canary);
}

#[test]
fn lod_short_destination_reports_needed_and_keeps_canary() {
    use mornlea_engine::native::contracts::world::LodQuad;

    let params = worldgen_params();
    let mut scratch = LodScratch::try_new().unwrap();
    let request = LodRequest {
        params: &params,
        tile: [-3, 2],
        step: LodStep::Four,
    };
    let mut probe = vec![LodQuad::default(); STAGE_QUADS];
    let needed = NativeLod.build(&request, &mut scratch, &mut probe).unwrap();
    assert!(needed > 0);

    // Fill the short destination with the known-good prefix: a partial
    // publish would overwrite the canary tail.
    let mut short = vec![LodQuad::default(); needed - 1];
    short.clone_from_slice(&probe[..needed - 1]);
    let canary = short.clone();
    assert_eq!(
        NativeLod.build(&request, &mut scratch, &mut short),
        Err(KernelError::OutputTooSmall {
            needed,
            available: needed - 1,
        })
    );
    assert_eq!(short, canary);
}

#[test]
fn fluid_eval_short_destination_reports_needed_and_keeps_canary() {
    let op = NativeFluidEval;
    // Two source cells with open cells below publish exactly two writes.
    let items = [[27, 2, 0, 2, 2, 2, 2], [27, 2, 0, 2, 2, 2, 2]];
    let mut probe = [FluidWrites::default(); 2];
    let needed = op.evaluate(&items, &mut probe).unwrap();
    assert_eq!(needed, 2);

    // A one-slot destination pre-filled with the first known-good write:
    // a partial publish would overwrite the surviving slot.
    let mut short = [FluidWrites::default(); 1];
    short[0] = probe[0];
    let canary = short;
    assert_eq!(
        op.evaluate(&items, &mut short),
        Err(KernelError::OutputTooSmall {
            needed: 2,
            available: 1,
        })
    );
    assert_eq!(short, canary);
}

#[test]
fn fluid_rescan_short_destination_reports_needed_and_keeps_canary() {
    let sections = std::array::from_fn(|_| RescanSection::Uniform(2));
    let skirt = Box::new([[2_u16; 384]; 68]);
    let metadata = Box::new([Some(2_u16); 216]);
    // Section 0 emits one flowing cell at local (1, 0, 2) of the full range.
    let mut dense = Box::new([2_u16; 4096]);
    dense[1 + 2 * 16] = 29;
    let sections = {
        let mut fixed = sections;
        fixed[0] = RescanSection::Dense(&dense);
        fixed
    };
    let view = RescanView::try_new(sections, &skirt, &metadata, [0, 0]).unwrap();
    let request = RescanRequest {
        view,
        range: RescanRange {
            x0: 2,
            x1: 4,
            z0: 3,
            z1: 5,
        },
        start_section: 0,
        budget: 10_000,
    };
    let mut scratch = RescanScratch::try_with_capacity(9 * 16 * 24).unwrap();
    let mut probe = [[i32::MIN, i32::MAX, i32::MIN]; 8];
    let summary = NativeFluidRescan
        .rescan(&request, &mut scratch, &mut probe)
        .unwrap();
    assert!(summary.written > 0);
    let needed = summary.written;

    let canary = vec![[i32::MIN, i32::MAX, i32::MIN]; needed - 1];
    let mut short = canary.clone();
    assert_eq!(
        NativeFluidRescan.rescan(&request, &mut scratch, &mut short),
        Err(KernelError::OutputTooSmall {
            needed,
            available: needed - 1,
        })
    );
    assert_eq!(short, canary);
}
