use super::KernelError;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Aabb {
    pub minimum: [f32; 3],
    pub maximum: [f32; 3],
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CollisionCell {
    pub(crate) loaded: bool,
    pub(crate) boxes: [Aabb; 8],
    pub(crate) used: u8,
}

impl Default for CollisionCell {
    fn default() -> Self {
        Self {
            loaded: false,
            boxes: [Aabb {
                minimum: [0.0; 3],
                maximum: [0.0; 3],
            }; 8],
            used: 0,
        }
    }
}

impl CollisionCell {
    pub fn try_new(loaded: bool, boxes: [Aabb; 8], used: u8) -> Result<Self, KernelError> {
        if used > 8 {
            return Err(KernelError::InvalidInput);
        }
        for b in &boxes[..used as usize] {
            if !b.minimum[0].is_finite()
                || !b.minimum[1].is_finite()
                || !b.minimum[2].is_finite()
                || !b.maximum[0].is_finite()
                || !b.maximum[1].is_finite()
                || !b.maximum[2].is_finite()
            {
                return Err(KernelError::InvalidInput);
            }
        }
        Ok(Self { loaded, boxes, used })
    }

    pub fn loaded(&self) -> bool {
        self.loaded
    }

    pub fn used(&self) -> u8 {
        self.used
    }

    pub fn boxes(&self) -> &[Aabb] {
        &self.boxes[..self.used as usize]
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CollisionGrid<'a> {
    pub(crate) origin: [i32; 3],
    pub(crate) dimensions: [u32; 3],
    pub(crate) cells: &'a [CollisionCell],
}

impl<'a> CollisionGrid<'a> {
    pub fn try_new(
        origin: [i32; 3],
        dimensions: [u32; 3],
        cells: &'a [CollisionCell],
    ) -> Result<Self, KernelError> {
        if dimensions[0] == 0 || dimensions[1] == 0 || dimensions[2] == 0 {
            return Err(KernelError::InvalidInput);
        }
        let total_cells = dimensions[0]
            .checked_mul(dimensions[1])
            .and_then(|p| p.checked_mul(dimensions[2]))
            .ok_or(KernelError::InvalidInput)?;
        if total_cells > 4096 || cells.len() != total_cells as usize {
            return Err(KernelError::InvalidInput);
        }
        for axis in 0..3 {
            let orig = origin[axis] as i64;
            let dim_minus_1 = (dimensions[axis] - 1) as i64;
            let far = orig + dim_minus_1;
            if far < i32::MIN as i64 || far > i32::MAX as i64 {
                return Err(KernelError::InvalidInput);
            }
        }
        Ok(Self {
            origin,
            dimensions,
            cells,
        })
    }

    pub fn origin(&self) -> [i32; 3] {
        self.origin
    }

    pub fn dimensions(&self) -> [u32; 3] {
        self.dimensions
    }

    pub fn cells(&self) -> &'a [CollisionCell] {
        self.cells
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CollisionRequest<'a> {
    pub position: [f32; 3],
    pub displacement: [f32; 3],
    pub began_grounded: bool,
    pub step_height: f32,
    pub grid: CollisionGrid<'a>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CollisionResult {
    pub position: [f32; 3],
    pub clipped: [bool; 3],
    pub on_ground: bool,
    pub used_step: bool,
    pub hit_unknown: bool,
}

pub trait CollisionOp {
    fn resolve(&self, request: &CollisionRequest<'_>) -> Result<CollisionResult, KernelError>;
}
