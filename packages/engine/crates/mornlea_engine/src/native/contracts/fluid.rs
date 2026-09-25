use super::KernelError;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum NeighborSlot {
    SelfCell = 0,
    Above = 1,
    Below = 2,
    PosX = 3,
    NegX = 4,
    PosZ = 5,
    NegZ = 6,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FluidChange {
    pub slot: NeighborSlot,
    pub block: u16,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FluidWrites {
    pub(crate) changes: [FluidChange; 4],
    pub(crate) len: usize,
}

impl Default for FluidWrites {
    fn default() -> Self {
        Self {
            changes: [FluidChange {
                slot: NeighborSlot::SelfCell,
                block: 0,
            }; 4],
            len: 0,
        }
    }
}

impl FluidWrites {
    pub fn changes(&self) -> &[FluidChange] {
        &self.changes[..self.len]
    }
}

pub trait FluidEvalOp {
    fn evaluate(&self, items: &[[u16; 7]], dst: &mut [FluidWrites]) -> Result<usize, KernelError>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RescanSection<'a> {
    Uniform(u16),
    Dense(&'a [u16; 4096]),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RescanView<'a> {
    pub(crate) sections: [RescanSection<'a>; 24],
    pub(crate) skirt: &'a [[u16; 384]; 68],
    pub(crate) metadata: &'a [Option<u16>; 216],
    pub(crate) center: [i32; 2],
}

impl<'a> RescanView<'a> {
    pub fn try_new(
        sections: [RescanSection<'a>; 24],
        skirt: &'a [[u16; 384]; 68],
        metadata: &'a [Option<u16>; 216],
        center: [i32; 2],
    ) -> Result<Self, KernelError> {
        for &c in &center {
            let base = (c as i64)
                .checked_mul(16)
                .ok_or(KernelError::InvalidInput)?;
            if base.checked_sub(1).is_none() || base.checked_add(16).is_none() {
                return Err(KernelError::InvalidInput);
            }
            if base - 1 < i32::MIN as i64 || base + 16 > i32::MAX as i64 {
                return Err(KernelError::InvalidInput);
            }
        }
        Ok(Self {
            sections,
            skirt,
            metadata,
            center,
        })
    }

    pub fn sections(&self) -> &[RescanSection<'a>; 24] {
        &self.sections
    }

    pub fn center(&self) -> [i32; 2] {
        self.center
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RescanRange {
    pub x0: u8,
    pub x1: u8,
    pub z0: u8,
    pub z1: u8,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RescanRequest<'a> {
    pub view: RescanView<'a>,
    pub range: RescanRange,
    pub start_section: u8,
    pub budget: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RescanSummary {
    pub written: usize,
    pub spent: u32,
    pub done: bool,
    pub next_section: u8,
}

pub struct RescanScratch {
    #[allow(dead_code)]
    pub(crate) positions: Vec<[i32; 3]>,
    #[allow(dead_code)]
    pub(crate) capacity: usize,
}

impl RescanScratch {
    pub fn try_with_capacity(max_positions: usize) -> Result<Self, KernelError> {
        if max_positions > 124416 {
            return Err(KernelError::InvalidInput);
        }
        let mut positions = Vec::new();
        positions
            .try_reserve_exact(max_positions)
            .map_err(|_| KernelError::Allocation)?;
        Ok(Self {
            positions,
            capacity: max_positions,
        })
    }
}

pub trait FluidRescanOp {
    fn rescan(
        &self,
        request: &RescanRequest<'_>,
        scratch: &mut RescanScratch,
        dst: &mut [[i32; 3]],
    ) -> Result<RescanSummary, KernelError>;
}
