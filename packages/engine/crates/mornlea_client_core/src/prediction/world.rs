//! Bounded numerical views borrow the confirmed owner anew at each actual position.

use super::*;
use crate::runtime_data;
use mornlea_engine::native::contracts::CollisionCell;

#[derive(Clone, Copy)]
pub(super) enum BorrowedEnvironment<'a> {
    Grid(StepEnvironment<'a>),
    World(&'a WorldConfirmed, Dimension),
}

impl BorrowedEnvironment<'_> {
    pub(super) fn step(
        self,
        state: PhysicsState,
        control: &PlayerControl,
    ) -> Result<(PhysicsState, [bool; 3], bool, bool), ClientError> {
        match self {
            Self::Grid(environment) => kernel_step(state, control, &environment),
            Self::World(world, dimension) => {
                let fluid = body_in_fluid(world, dimension, state.position)?;
                let controls = physics_controls(control, fluid);
                let sweep = sweep_bounds(state, &controls);
                let lower = [
                    f64::from(state.position[0]) + f64::from(sweep.minimum[0]) - 0.3,
                    f64::from(state.position[1]) + f64::from(sweep.minimum[1]),
                    f64::from(state.position[2]) + f64::from(sweep.minimum[2]) - 0.3,
                ];
                let upper = [
                    f64::from(state.position[0]) + f64::from(sweep.maximum[0]) + 0.3,
                    f64::from(state.position[1])
                        + f64::from(sweep.maximum[1])
                        + 1.8
                        + f64::from(F1_DEFAULT_TUNING.step_height),
                    f64::from(state.position[2]) + f64::from(sweep.maximum[2]) + 0.3,
                ];
                let mut minimum = [0; 3];
                let mut maximum = [0; 3];
                for axis in 0..3 {
                    minimum[axis] = checked_integer(lower[axis].floor() - 1.0)?;
                    maximum[axis] = checked_integer(upper[axis].floor() + 1.0)?;
                }
                let owned = WorldGrid::new(world, dimension, minimum, maximum)?;
                kernel_step(
                    state,
                    control,
                    &StepEnvironment::new(owned.borrow()?, fluid),
                )
            }
        }
    }

    pub(super) fn target(
        self,
        origin: [f32; 3],
        look: LookAngles,
        revision: ConfirmedRevision,
    ) -> Result<Option<RayTarget>, ClientError> {
        match self {
            Self::Grid(environment) => ray_target(origin, look, revision, environment.grid()),
            Self::World(world, dimension) => {
                let mut minimum = [0; 3];
                let mut maximum = [0; 3];
                for axis in 0..3 {
                    let center = f64::from(origin[axis]).floor();
                    minimum[axis] = checked_integer(center - 7.0)?;
                    maximum[axis] = checked_integer(center + 7.0)?;
                }
                let owned = WorldGrid::new(world, dimension, minimum, maximum)?;
                ray_target(origin, look, revision, &owned.borrow()?)
            }
        }
    }
}

fn checked_integer(value: f64) -> Result<i32, ClientError> {
    if !value.is_finite() || value < f64::from(i32::MIN) || value > f64::from(i32::MAX) {
        return Err(ClientError::InvalidInput);
    }
    Ok(value as i32)
}

struct WorldGrid {
    origin: [i32; 3],
    dimensions: [u32; 3],
    cells: Vec<CollisionCell>,
}

impl WorldGrid {
    fn new(
        world: &WorldConfirmed,
        dimension: Dimension,
        minimum: [i32; 3],
        maximum: [i32; 3],
    ) -> Result<Self, ClientError> {
        let mut dimensions = [0; 3];
        let mut count = 1u64;
        for axis in 0..3 {
            let length = i64::from(maximum[axis]) - i64::from(minimum[axis]) + 1;
            dimensions[axis] = u32::try_from(length).map_err(|_| ClientError::Capacity)?;
            count = count
                .checked_mul(u64::from(dimensions[axis]))
                .ok_or(ClientError::Capacity)?;
        }
        if count == 0 || count > 4096 {
            return Err(ClientError::Capacity);
        }
        let mut cells = Vec::with_capacity(count as usize);
        // Native numerical views use YXZ; compact chunk storage uses YZX.
        for y in minimum[1]..=maximum[1] {
            for x in minimum[0]..=maximum[0] {
                for z in minimum[2]..=maximum[2] {
                    cells.push(runtime_data::collision_cell(
                        world.block_at(dimension, BlockPos::new(x, y, z)),
                    )?);
                }
            }
        }
        Ok(Self {
            origin: minimum,
            dimensions,
            cells,
        })
    }

    fn borrow(&self) -> Result<CollisionGrid<'_>, ClientError> {
        CollisionGrid::try_new(self.origin, self.dimensions, &self.cells)
            .map_err(|_| ClientError::InvalidInput)
    }
}

fn body_in_fluid(
    world: &WorldConfirmed,
    dimension: Dimension,
    position: [f32; 3],
) -> Result<bool, ClientError> {
    // Match Go's float32 body bounds, excluding cells touched with zero volume.
    let lower = [position[0] - 0.3, position[1], position[2] - 0.3];
    let upper = [position[0] + 0.3, position[1] + 1.8, position[2] + 0.3];
    let mut minimum = [0; 3];
    let mut maximum = [0; 3];
    for axis in 0..3 {
        minimum[axis] = checked_integer(f64::from(lower[axis]).floor())?;
        let ceiling = checked_integer(f64::from(upper[axis]).ceil())?;
        maximum[axis] = (i64::from(ceiling) - 1).max(i64::from(minimum[axis])) as i32;
    }
    for y in minimum[1]..=maximum[1] {
        for x in minimum[0]..=maximum[0] {
            for z in minimum[2]..=maximum[2] {
                if world
                    .block_at(dimension, BlockPos::new(x, y, z))
                    .is_some_and(runtime_data::is_water)
                {
                    return Ok(true);
                }
            }
        }
    }
    Ok(false)
}
