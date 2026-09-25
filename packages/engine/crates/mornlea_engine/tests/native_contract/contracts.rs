use mornlea_engine::native::contracts::*;

struct DoubleCollision;
impl CollisionOp for DoubleCollision {
    fn resolve(&self, _request: &CollisionRequest<'_>) -> Result<CollisionResult, KernelError> {
        Ok(CollisionResult {
            position: [0.0; 3],
            clipped: [false; 3],
            on_ground: true,
            used_step: false,
            hit_unknown: false,
        })
    }
}

struct DoublePhysics;
impl PhysicsOp for DoublePhysics {
    fn step(&self, _request: &PhysicsRequest<'_>) -> Result<PhysicsResult, KernelError> {
        Ok(PhysicsResult {
            state: PhysicsState {
                position: [0.0; 3],
                velocity: [0.0; 3],
                on_ground: true,
            },
            clipped: [false; 3],
            used_step: false,
            hit_unknown: false,
        })
    }
}

struct DoubleRaycast;
impl RaycastOp for DoubleRaycast {
    fn next_batch(&self, _cursor: &mut RayCursor) -> Result<RayBatch, KernelError> {
        RayBatch::from_parts([RayRecord {
            cell: [0; 3],
            face: RayFace::Origin,
            distance: 0.0,
        }; 64], 0, true)
    }
}

struct DoubleWorldgen;
impl WorldgenOp for DoubleWorldgen {
    fn generate_chunk(
        &self,
        _params: &WorldgenParams,
        _chunk: [i32; 2],
        _scratch: &mut WorldgenScratch,
        _dst: &mut [u16],
    ) -> Result<usize, KernelError> {
        Ok(98304)
    }
}

struct DoubleProbe;
impl ProbeOp for DoubleProbe {
    fn probe(
        &self,
        _params: &WorldgenParams,
        _queries: &[ProbeQuery],
        _dst: &mut [ProbeValue],
    ) -> Result<usize, KernelError> {
        Ok(0)
    }
}

struct DoubleTree;
impl TreeOp for DoubleTree {
    fn tree_blocks(&self, _request: &TreeRequest) -> Result<TreeBlocks, KernelError> {
        TreeBlocks::from_parts([TreeBlock {
            offset: [0; 3],
            block: 0,
        }; 128], 0)
    }
}

struct DoubleLod;
impl LodOp for DoubleLod {
    fn build(
        &self,
        _request: &LodRequest<'_>,
        _scratch: &mut LodScratch,
        _dst: &mut [LodQuad],
    ) -> Result<usize, KernelError> {
        Ok(0)
    }
}

struct DoubleFluidEval;
impl FluidEvalOp for DoubleFluidEval {
    fn evaluate(
        &self,
        items: &[[u16; 7]],
        dst: &mut [FluidWrites],
    ) -> Result<usize, KernelError> {
        if items.is_empty() {
            return Err(KernelError::InvalidInput);
        }
        if dst.len() < items.len() {
            return Err(KernelError::OutputTooSmall {
                needed: items.len(),
                available: dst.len(),
            });
        }
        Ok(items.len())
    }
}

struct DoubleFluidRescan;
impl FluidRescanOp for DoubleFluidRescan {
    fn rescan(
        &self,
        _request: &RescanRequest<'_>,
        _scratch: &mut RescanScratch,
        _dst: &mut [[i32; 3]],
    ) -> Result<RescanSummary, KernelError> {
        Ok(RescanSummary {
            written: 0,
            spent: 0,
            done: true,
            next_section: 24,
        })
    }
}

struct DoubleMesh;
impl MeshOp for DoubleMesh {
    fn mesh(
        &self,
        _view: &MeshView<'_>,
        _scratch: &mut MeshScratch,
        _dst: &mut [MeshQuad],
    ) -> Result<usize, KernelError> {
        Ok(0)
    }
}

struct DoublePathfind;
impl PathfindOp for DoublePathfind {
    fn find(
        &self,
        _grid: &PathGrid,
        _start: PathCell,
        _goal: PathCell,
        _scratch: &mut PathScratch,
    ) -> Result<PathResult, PathError> {
        Ok(PathResult::new(vec![].into_boxed_slice(), vec![].into_boxed_slice()))
    }
}

fn generic_eval_caller(
    op: &dyn FluidEvalOp,
    items: &[[u16; 7]],
    dst: &mut [FluidWrites],
) -> Result<usize, KernelError> {
    op.evaluate(items, dst)
}

#[test]
fn contract_doubles_compile_and_dispatch() {
    let cells = vec![CollisionCell::default(); 4096];
    let grid = CollisionGrid::try_new([0, 0, 0], [16, 16, 16], &cells).unwrap();

    let _ = DoubleCollision.resolve(&CollisionRequest {
        position: [0.0; 3],
        displacement: [0.0; 3],
        began_grounded: true,
        step_height: 0.0,
        grid,
    });
    let _ = DoublePhysics.step(&PhysicsRequest {
        controls: PhysicsControls {
            move_x: 0,
            move_z: 0,
            jump: false,
            yaw_sin: 0.0,
            yaw_cos: 1.0,
            body_in_fluid: false,
            sprinting: false,
            sneaking: false,
        },
        tuning: PhysicsTuning {
            fixed_delta_seconds: 0.05,
            step_height: 0.0,
            walk_speed: 0.0,
            ground_acceleration: 0.0,
            ground_deceleration: 0.0,
            air_acceleration: 0.0,
            jump_speed: 0.0,
            gravity: 0.0,
            terminal_fall_speed: 0.0,
            fluid_gravity: 0.0,
            fluid_sink_speed: 0.0,
            fluid_ascend_speed: 0.0,
            fluid_horizontal_drag: 0.0,
            sprint_speed_multiplier: 1.0,
            sneak_speed_multiplier: 1.0,
        },
        state: PhysicsState {
            position: [0.0; 3],
            velocity: [0.0; 3],
            on_ground: true,
        },
        sweep: SweepBounds { minimum: [0.0; 3], maximum: [1.0; 3] },
        grid,
    });
    let _ = DoubleRaycast.next_batch(&mut RayCursor::try_new(Ray {
        origin: [0.0; 3],
        direction: [0.0, 1.0, 0.0],
        maximum: 1.0,
    }).unwrap());
    let perm = [0u8; 512];
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
    let params = WorldgenParams::try_new(0, materials, perm).unwrap();
    let mut w_scratch = WorldgenScratch::try_new().unwrap();
    let _ = DoubleWorldgen.generate_chunk(&params, [0, 0], &mut w_scratch, &mut [0; 98304]);
    let _ = DoubleProbe.probe(&params, &[], &mut []);
    let _ = DoubleTree.tree_blocks(&TreeRequest { seed: 0, root: [0; 3] });
    let mut l_scratch = LodScratch::try_new().unwrap();
    let _ = DoubleLod.build(&LodRequest {
        params: &params,
        tile: [0, 0],
        step: LodStep::Two,
    }, &mut l_scratch, &mut []);
    let mut r_scratch = RescanScratch::try_with_capacity(100).unwrap();
    let skirt = Box::new([[0u16; 384]; 68]);
    let skirt_ref = Box::leak(skirt);
    let metadata = Box::new([None; 216]);
    let metadata_ref = Box::leak(metadata);
    let _ = DoubleFluidRescan.rescan(&RescanRequest {
        view: RescanView::try_new([RescanSection::Uniform(0); 24], skirt_ref, metadata_ref, [0, 0]).unwrap(),
        range: RescanRange { x0: 0, x1: 0, z0: 0, z1: 0 },
        start_section: 0,
        budget: 0,
    }, &mut r_scratch, &mut []);
    let mut m_scratch = MeshScratch::try_new().unwrap();
    let blocks = Box::new([0u16; 110592]);
    let blocks_ref = Box::leak(blocks) as &[u16; 110592];
    let heights_present = Box::new([false; 9]);
    let heights_present_ref = Box::leak(heights_present) as &[bool; 9];
    let heights = Box::new([[0i16; 256]; 9]);
    let heights_ref = Box::leak(heights) as &[[i16; 256]; 9];
    let mesh_reg = MeshRegistry::try_new(&[MeshRegistryEntry {
        id: 0,
        opaque: true,
        emission: 0,
        material: [0; 6],
        fluid_height: 0,
        light_attenuation: 0,
        block_top_raw: 0,
        model: MeshModel::Default,
    }], &[0], 0, 0).unwrap();
    let view = MeshView {
        blocks: blocks_ref,
        heights_present: heights_present_ref,
        heights: heights_ref,
        section_origin_y: 0,
        registry: &mesh_reg,
    };
    let _ = DoubleMesh.mesh(&view, &mut m_scratch, &mut []);
    let mut p_scratch = PathScratch::try_with_capacity(100).unwrap();
    let _ = DoublePathfind.find(&PathGrid::try_new(PathCell { x: 0, y: 0, z: 0 }, [1, 1, 1], vec![0].into_boxed_slice(), PathBlockTable::from_passable_ids(&[]).unwrap(), vec![]).unwrap(), PathCell { x: 0, y: 0, z: 0 }, PathCell { x: 0, y: 0, z: 0 }, &mut p_scratch);

    let double = DoubleFluidEval;
    let mut dst = [FluidWrites::default(); 2];
    let ok_res = generic_eval_caller(&double, &[[1, 0, 0, 0, 0, 0, 0]], &mut dst);
    assert_eq!(ok_res, Ok(1));

    let err_res = generic_eval_caller(&double, &[], &mut dst);
    assert_eq!(err_res, Err(KernelError::InvalidInput));
}

#[test]
fn shared_collision_constructors() {
    let cells = vec![CollisionCell::default(); 4096];
    let grid_ok = CollisionGrid::try_new([0, 0, 0], [16, 16, 16], &cells);
    assert!(grid_ok.is_ok());

    // Zero dimension rejected
    let grid_zero = CollisionGrid::try_new([0, 0, 0], [0, 16, 16], &cells);
    assert_eq!(grid_zero, Err(KernelError::InvalidInput));

    // 4097 cells rejected
    let cells_4097 = vec![CollisionCell::default(); 4097];
    let grid_too_many = CollisionGrid::try_new([0, 0, 0], [17, 16, 16], &cells_4097);
    assert_eq!(grid_too_many, Err(KernelError::InvalidInput));

    // Dimension count mismatch rejected
    let grid_mismatch = CollisionGrid::try_new([0, 0, 0], [16, 16, 16], &cells[..4095]);
    assert_eq!(grid_mismatch, Err(KernelError::InvalidInput));

    // Origin endpoint overflow rejected
    let grid_overflow = CollisionGrid::try_new([i32::MAX, 0, 0], [2, 1, 1], &cells[..2]);
    assert_eq!(grid_overflow, Err(KernelError::InvalidInput));

    // Used box NaN rejected
    let mut bad_boxes = [Aabb {
        minimum: [0.0; 3],
        maximum: [1.0; 3],
    }; 8];
    bad_boxes[0].minimum[0] = f32::NAN;
    let cell_used_nan = CollisionCell::try_new(true, bad_boxes, 1);
    assert_eq!(cell_used_nan, Err(KernelError::InvalidInput));

    // Unused slot NaN accepted
    let mut unused_nan_boxes = [Aabb {
        minimum: [0.0; 3],
        maximum: [1.0; 3],
    }; 8];
    unused_nan_boxes[1].minimum[0] = f32::NAN;
    let cell_unused_nan = CollisionCell::try_new(true, unused_nan_boxes, 1);
    assert!(cell_unused_nan.is_ok());
}

#[test]
fn shared_world_params_constructor() {
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
    let perm = [0u8; 512];
    let params = WorldgenParams::try_new(42, materials, perm);
    assert!(params.is_ok());
    let params = params.unwrap();
    assert_eq!(params.seed(), 42);
    assert_eq!(params.materials().air, 0);
    assert_eq!(params.perm(), &perm);

    // water == air duplicate accepted
    let mut water_is_air_materials = materials;
    water_is_air_materials.water = water_is_air_materials.air;
    let params_water_air = WorldgenParams::try_new(42, water_is_air_materials, perm);
    assert!(params_water_air.is_ok());

    // another duplicate rejected (e.g. stone == dirt)
    let mut bad_materials = materials;
    bad_materials.stone = bad_materials.dirt;
    let params_bad = WorldgenParams::try_new(42, bad_materials, perm);
    assert_eq!(params_bad, Err(KernelError::InvalidInput));
}

