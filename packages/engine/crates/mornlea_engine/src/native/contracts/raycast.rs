use super::KernelError;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Ray {
    pub origin: [f32; 3],
    pub direction: [f32; 3],
    pub maximum: f32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum RayFace {
    Origin = 255,
    NegX = 0,
    PosX = 1,
    NegY = 2,
    PosY = 3,
    NegZ = 4,
    PosZ = 5,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RayRecord {
    pub cell: [i32; 3],
    pub face: RayFace,
    pub distance: f32,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RayBatch {
    pub(crate) records: [RayRecord; 64],
    pub(crate) len: usize,
    pub(crate) done: bool,
}

impl RayBatch {
    pub fn from_parts(
        records: [RayRecord; 64],
        len: usize,
        done: bool,
    ) -> Result<Self, KernelError> {
        if len > 64 {
            return Err(KernelError::InvalidInput);
        }
        Ok(Self { records, len, done })
    }

    pub fn records(&self) -> &[RayRecord] {
        &self.records[..self.len]
    }

    pub fn is_done(&self) -> bool {
        self.done
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct RayCursor {
    pub(crate) ray: Ray,
    // Complete private DDA state keeps each continuation bounded regardless of
    // traversal history; safe callers cannot fabricate cursor history.
    pub(crate) continuation: Option<crate::raycast::RaycastCursor>,
}

impl RayCursor {
    pub fn try_new(ray: Ray) -> Result<Self, KernelError> {
        if !ray.origin[0].is_finite()
            || !ray.origin[1].is_finite()
            || !ray.origin[2].is_finite()
            || !ray.direction[0].is_finite()
            || !ray.direction[1].is_finite()
            || !ray.direction[2].is_finite()
            || !ray.maximum.is_finite()
            || ray.maximum <= 0.0
            || (ray.direction[0] == 0.0 && ray.direction[1] == 0.0 && ray.direction[2] == 0.0)
        {
            return Err(KernelError::InvalidInput);
        }
        Ok(Self {
            ray,
            continuation: None,
        })
    }

    pub fn ray(&self) -> Ray {
        self.ray
    }

    pub fn is_done(&self) -> bool {
        self.continuation
            .as_ref()
            .is_some_and(|cursor| cursor.state == 2)
    }
}

pub trait RaycastOp {
    fn next_batch(&self, cursor: &mut RayCursor) -> Result<RayBatch, KernelError>;
}
