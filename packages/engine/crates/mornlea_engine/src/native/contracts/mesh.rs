use super::KernelError;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum MeshModel {
    Default = 0,
    StandingTorch = 1,
    WallTorchPosX = 2,
    WallTorchNegX = 3,
    WallTorchPosZ = 4,
    WallTorchNegZ = 5,
    Bed = 6,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MeshRegistryEntry {
    pub id: u16,
    pub opaque: bool,
    pub emission: u8,
    pub material: [u16; 6],
    pub fluid_height: u8,
    pub light_attenuation: u8,
    pub block_top_raw: u8,
    pub model: MeshModel,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MeshRegistry {
    pub(crate) entries: Vec<MeshRegistryEntry>,
    pub(crate) visibility: Vec<u64>,
    pub(crate) air: u16,
    pub(crate) barrier: u16,
}

impl MeshRegistry {
    pub fn try_new(
        entries: &[MeshRegistryEntry],
        visibility: &[u64],
        air: u16,
        barrier: u16,
    ) -> Result<Self, KernelError> {
        let r = entries.len();
        if !(1..=96).contains(&r) {
            return Err(KernelError::InvalidRegistry);
        }
        for i in 0..(r - 1) {
            if entries[i].id >= entries[i + 1].id {
                return Err(KernelError::InvalidRegistry);
            }
        }
        #[allow(clippy::manual_div_ceil)]
        let words_needed = r * ((r + 63) / 64);
        if visibility.len() != words_needed {
            return Err(KernelError::InvalidRegistry);
        }
        for entry in entries {
            if entry.emission > 15 {
                return Err(KernelError::EmissionOutOfRange);
            }
            if entry.fluid_height > 14 {
                return Err(KernelError::InvalidRegistry);
            }
            if (entry.model as u8) > 6 {
                return Err(KernelError::InvalidRegistry);
            }
        }
        Ok(Self {
            entries: entries.to_vec(),
            visibility: visibility.to_vec(),
            air,
            barrier,
        })
    }

    pub fn entries(&self) -> &[MeshRegistryEntry] {
        &self.entries
    }

    pub fn visibility(&self) -> &[u64] {
        &self.visibility
    }

    pub fn air(&self) -> u16 {
        self.air
    }

    pub fn barrier(&self) -> u16 {
        self.barrier
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MeshView<'a> {
    pub blocks: &'a [u16; 110592],
    pub heights_present: &'a [bool; 9],
    pub heights: &'a [[i16; 256]; 9],
    pub section_origin_y: i32,
    pub registry: &'a MeshRegistry,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MeshQuad {
    pub(crate) x: u8,
    pub(crate) y: u8,
    pub(crate) z: u8,
    pub(crate) w: u8,
    pub(crate) h: u8,
    pub(crate) face: u8,
    pub(crate) material: u16,
    pub(crate) ao: u8,
    pub(crate) light: u8,
    pub(crate) corners: [u8; 4],
    pub(crate) back: bool,
}

impl Default for MeshQuad {
    fn default() -> Self {
        Self {
            x: 0,
            y: 0,
            z: 0,
            w: 1,
            h: 1,
            face: 0,
            material: 0,
            ao: 0,
            light: 0,
            corners: [0; 4],
            back: false,
        }
    }
}

impl MeshQuad {
    pub fn x(&self) -> u8 {
        self.x
    }
    pub fn y(&self) -> u8 {
        self.y
    }
    pub fn z(&self) -> u8 {
        self.z
    }
    pub fn w(&self) -> u8 {
        self.w
    }
    pub fn h(&self) -> u8 {
        self.h
    }
    pub fn face(&self) -> u8 {
        self.face
    }
    pub fn material(&self) -> u16 {
        self.material
    }
    pub fn ao(&self) -> u8 {
        self.ao
    }
    pub fn light(&self) -> u8 {
        self.light
    }
    pub fn corners(&self) -> [u8; 4] {
        self.corners
    }
    pub fn back(&self) -> bool {
        self.back
    }

    pub fn packed(&self) -> u64 {
        let face = match self.face {
            0 => crate::quad::Face::NegX,
            1 => crate::quad::Face::PosX,
            2 => crate::quad::Face::NegY,
            3 => crate::quad::Face::PosY,
            4 => crate::quad::Face::NegZ,
            5 => crate::quad::Face::PosZ,
            6 => crate::quad::Face::PlantDiagA,
            _ => crate::quad::Face::PlantDiagB,
        };
        crate::quad::Quad {
            x: self.x,
            y: self.y,
            z: self.z,
            w: self.w,
            h: self.h,
            face,
            material: self.material,
            ao: self.ao,
            light: self.light,
            corners: self.corners,
            back: self.back,
        }
        .pack()
    }
}

pub struct MeshScratch {
    #[allow(dead_code)]
    pub(crate) levels: Box<[u8; 110592]>,
    #[allow(dead_code)]
    pub(crate) queue: Box<[u32; 110592]>,
    #[allow(dead_code)]
    pub(crate) stage: Box<[MeshQuad; 40960]>,
}

impl MeshScratch {
    pub fn try_new() -> Result<Self, KernelError> {
        let levels = vec![0u8; 110592].into_boxed_slice();
        let levels = match levels.try_into() {
            Ok(b) => b,
            Err(_) => return Err(KernelError::Allocation),
        };
        let queue = vec![0u32; 110592].into_boxed_slice();
        let queue = match queue.try_into() {
            Ok(b) => b,
            Err(_) => return Err(KernelError::Allocation),
        };
        let stage = vec![MeshQuad::default(); 40960].into_boxed_slice();
        let stage = match stage.try_into() {
            Ok(b) => b,
            Err(_) => return Err(KernelError::Allocation),
        };
        Ok(Self {
            levels,
            queue,
            stage,
        })
    }
}

pub trait MeshOp {
    fn mesh(
        &self,
        view: &MeshView<'_>,
        scratch: &mut MeshScratch,
        dst: &mut [MeshQuad],
    ) -> Result<usize, KernelError>;
}
