use crate::collision::{Bounds, CollisionCells, CollisionInputData, resolve_move_and_step};
use crate::native::contracts::KernelError;
use crate::native::contracts::collision::{CollisionOp, CollisionRequest, CollisionResult};

pub struct NativeCollision;

impl CollisionOp for NativeCollision {
    fn resolve(&self, request: &CollisionRequest<'_>) -> Result<CollisionResult, KernelError> {
        resolve_collision(request)
    }
}

pub fn resolve_collision(request: &CollisionRequest<'_>) -> Result<CollisionResult, KernelError> {
    let pos = request.position;
    let disp = request.displacement;
    let step = request.step_height;

    if !pos[0].is_finite() || !pos[1].is_finite() || !pos[2].is_finite() {
        return Err(KernelError::InvalidInput);
    }
    if !disp[0].is_finite() || !disp[1].is_finite() || !disp[2].is_finite() {
        return Err(KernelError::InvalidInput);
    }
    if !step.is_finite() {
        return Err(KernelError::InvalidInput);
    }

    const HALF_WIDTH: f32 = 0.3;
    const PLAYER_HEIGHT: f32 = 1.8;
    const EPSILON: f32 = 1e-5;
    const GROUND_PROBE: f32 = 1e-4;

    let min_x = pos[0].min(pos[0] + disp[0]) - HALF_WIDTH - EPSILON;
    let max_x = pos[0].max(pos[0] + disp[0]) + HALF_WIDTH + EPSILON;
    let min_y = pos[1] + 0.0_f32.min(disp[1]).min(step) - GROUND_PROBE - EPSILON;
    let max_y = pos[1] + 0.0_f32.max(disp[1]).max(step) + PLAYER_HEIGHT + EPSILON;
    let min_z = pos[2].min(pos[2] + disp[2]) - HALF_WIDTH - EPSILON;
    let max_z = pos[2].max(pos[2] + disp[2]) + HALF_WIDTH + EPSILON;

    let check_bounds = |min: f32, max: f32, axis: usize| {
        if !min.is_finite() || !max.is_finite() {
            return Err(KernelError::DisplacementOutOfBounds);
        }
        let min_i = min.floor() as i64;
        let max_i = max.floor() as i64;
        let grid_min = request.grid.origin()[axis] as i64;
        let grid_max = grid_min + request.grid.dimensions()[axis] as i64 - 1;

        if min_i < grid_min
            || max_i > grid_max
            || min_i < i32::MIN as i64
            || max_i > i32::MAX as i64
        {
            Err(KernelError::DisplacementOutOfBounds)
        } else {
            Ok(())
        }
    };

    check_bounds(min_x, max_x, 0)?;
    check_bounds(min_y, max_y, 1)?;
    check_bounds(min_z, max_z, 2)?;

    let grid_wrapper = NativeGridWrapper { grid: request.grid };
    let input = CollisionInputData {
        cells: &grid_wrapper,
        position: pos,
        displacement: disp,
        began_grounded: request.began_grounded,
        step_height: step,
    };

    let result = resolve_move_and_step(&input);

    Ok(CollisionResult {
        position: result.position,
        clipped: result.clipped,
        on_ground: result.on_ground,
        used_step: result.used_step,
        hit_unknown: result.hit_unknown,
    })
}

struct NativeGridWrapper<'a> {
    grid: crate::native::contracts::collision::CollisionGrid<'a>,
}

impl<'a> CollisionCells for NativeGridWrapper<'a> {
    fn loaded(&self, position: [i32; 3]) -> bool {
        let idx = self.index(position);
        self.grid.cells()[idx].loaded()
    }
    fn count(&self, position: [i32; 3]) -> usize {
        let idx = self.index(position);
        self.grid.cells()[idx].used() as usize
    }
    fn bounds(&self, position: [i32; 3], index: usize) -> Bounds {
        let idx = self.index(position);
        let aabb = self.grid.cells()[idx].boxes()[index];
        Bounds {
            minimum: [
                aabb.minimum[0] + position[0] as f32,
                aabb.minimum[1] + position[1] as f32,
                aabb.minimum[2] + position[2] as f32,
            ],
            maximum: [
                aabb.maximum[0] + position[0] as f32,
                aabb.maximum[1] + position[1] as f32,
                aabb.maximum[2] + position[2] as f32,
            ],
        }
    }
}

impl<'a> NativeGridWrapper<'a> {
    fn index(&self, position: [i32; 3]) -> usize {
        let origin = self.grid.origin();
        let dims = self.grid.dimensions();
        let x = (position[0] - origin[0]) as usize;
        let y = (position[1] - origin[1]) as usize;
        let z = (position[2] - origin[2]) as usize;
        (y * dims[0] as usize + x) * dims[2] as usize + z
    }
}
