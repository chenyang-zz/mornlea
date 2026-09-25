use super::KernelError;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Materials {
    pub air: u16,
    pub stone: u16,
    pub dirt: u16,
    pub grass: u16,
    pub bedrock: u16,
    pub snow: u16,
    pub sand: u16,
    pub clay: u16,
    pub gravel: u16,
    pub iron_ore: u16,
    pub coal_ore: u16,
    pub oak_log: u16,
    pub leaves: u16,
    pub water: u16,
    pub short_grass: u16,
}

impl Materials {
    pub fn as_slice(&self) -> [u16; 15] {
        [
            self.air,
            self.stone,
            self.dirt,
            self.grass,
            self.bedrock,
            self.snow,
            self.sand,
            self.clay,
            self.gravel,
            self.iron_ore,
            self.coal_ore,
            self.oak_log,
            self.leaves,
            self.water,
            self.short_grass,
        ]
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorldgenParams {
    pub(crate) seed: i64,
    pub(crate) materials: Materials,
    pub(crate) perm: [u8; 512],
}

impl WorldgenParams {
    pub fn try_new(seed: i64, materials: Materials, perm: [u8; 512]) -> Result<Self, KernelError> {
        let arr = materials.as_slice();
        for i in 0..15 {
            for j in (i + 1)..15 {
                if arr[i] == arr[j] {
                    let is_water_air = (i == 0 && j == 13) || (i == 13 && j == 0);
                    if !is_water_air {
                        return Err(KernelError::InvalidInput);
                    }
                }
            }
        }
        Ok(Self {
            seed,
            materials,
            perm,
        })
    }

    pub fn seed(&self) -> i64 {
        self.seed
    }

    pub fn materials(&self) -> Materials {
        self.materials
    }

    pub fn perm(&self) -> &[u8; 512] {
        &self.perm
    }

    #[allow(dead_code)]
    pub(crate) fn as_legacy(&self) -> crate::worldgen::WorldgenParams {
        crate::worldgen::WorldgenParams {
            seed: self.seed,
            materials: crate::worldgen::Materials {
                air: self.materials.air,
                stone: self.materials.stone,
                dirt: self.materials.dirt,
                grass: self.materials.grass,
                bedrock: self.materials.bedrock,
                snow: self.materials.snow,
                sand: self.materials.sand,
                clay: self.materials.clay,
                gravel: self.materials.gravel,
                iron_ore: self.materials.iron_ore,
                coal_ore: self.materials.coal_ore,
                oak_log: self.materials.oak_log,
                leaves: self.materials.leaves,
                water: self.materials.water,
                short_grass: self.materials.short_grass,
            },
            perm: self.perm,
        }
    }
}

pub struct WorldgenScratch {
    #[allow(dead_code)]
    pub(crate) stage: Box<[u16; 98304]>,
}

impl WorldgenScratch {
    pub fn try_new() -> Result<Self, KernelError> {
        let stage = vec![0u16; 98304].into_boxed_slice();
        let stage = match stage.try_into() {
            Ok(b) => b,
            Err(_) => return Err(KernelError::Allocation),
        };
        Ok(Self { stage })
    }
}

pub trait WorldgenOp {
    fn generate_chunk(
        &self,
        params: &WorldgenParams,
        chunk: [i32; 2],
        scratch: &mut WorldgenScratch,
        dst: &mut [u16],
    ) -> Result<usize, KernelError>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProbeQuery {
    Height { x: i32, z: i32 },
    Terrain { position: [i32; 3] },
    Base { position: [i32; 3] },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProbeValue {
    Height(i32),
    Block(u16),
}

impl Default for ProbeValue {
    fn default() -> Self {
        ProbeValue::Block(0)
    }
}

pub trait ProbeOp {
    fn probe(
        &self,
        params: &WorldgenParams,
        queries: &[ProbeQuery],
        dst: &mut [ProbeValue],
    ) -> Result<usize, KernelError>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TreeRequest {
    pub seed: i64,
    pub root: [i32; 3],
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TreeBlock {
    pub offset: [i8; 3],
    pub block: u16,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TreeBlocks {
    pub(crate) records: [TreeBlock; 128],
    pub(crate) len: usize,
}

impl TreeBlocks {
    pub fn from_parts(records: [TreeBlock; 128], len: usize) -> Result<Self, KernelError> {
        if len > 128 {
            return Err(KernelError::InvalidInput);
        }
        Ok(Self { records, len })
    }

    pub fn records(&self) -> &[TreeBlock] {
        &self.records[..self.len]
    }
}

pub trait TreeOp {
    fn tree_blocks(&self, request: &TreeRequest) -> Result<TreeBlocks, KernelError>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LodStep {
    Two,
    Four,
    Eight,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum LodFace {
    Top = 0,
    NegX = 1,
    PosX = 2,
    NegZ = 3,
    PosZ = 4,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LodQuad {
    pub(crate) x: i32,
    pub(crate) z: i32,
    pub(crate) y: i32,
    pub(crate) w: u16,
    pub(crate) d: u16,
    pub(crate) face: LodFace,
    pub(crate) material: u16,
    pub(crate) shade: u8,
}

impl Default for LodQuad {
    fn default() -> Self {
        Self {
            x: 0,
            z: 0,
            y: 0,
            w: 1,
            d: 1,
            face: LodFace::Top,
            material: 0,
            shade: 0,
        }
    }
}

impl LodQuad {
    pub fn x(&self) -> i32 {
        self.x
    }
    pub fn z(&self) -> i32 {
        self.z
    }
    pub fn y(&self) -> i32 {
        self.y
    }
    pub fn w(&self) -> u16 {
        self.w
    }
    pub fn d(&self) -> u16 {
        self.d
    }
    pub fn face(&self) -> LodFace {
        self.face
    }
    pub fn material(&self) -> u16 {
        self.material
    }
    pub fn shade(&self) -> u8 {
        self.shade
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LodRequest<'a> {
    pub params: &'a WorldgenParams,
    pub tile: [i32; 2],
    pub step: LodStep,
}

pub struct LodScratch {
    #[allow(dead_code)]
    pub(crate) samples: Box<[[i32; 2]; 1156]>,
    #[allow(dead_code)]
    pub(crate) stage: Box<[LodQuad; 3136]>,
}

impl LodScratch {
    pub fn try_new() -> Result<Self, KernelError> {
        let samples = vec![[0i32; 2]; 1156].into_boxed_slice();
        let samples = match samples.try_into() {
            Ok(b) => b,
            Err(_) => return Err(KernelError::Allocation),
        };
        let stage = vec![LodQuad::default(); 3136].into_boxed_slice();
        let stage = match stage.try_into() {
            Ok(b) => b,
            Err(_) => return Err(KernelError::Allocation),
        };
        Ok(Self { samples, stage })
    }
}

pub trait LodOp {
    fn build(
        &self,
        request: &LodRequest<'_>,
        scratch: &mut LodScratch,
        dst: &mut [LodQuad],
    ) -> Result<usize, KernelError>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_as_legacy() {
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
        let perm = [42; 512];
        let params = WorldgenParams::try_new(12345, materials, perm).unwrap();

        let legacy = params.as_legacy();
        assert_eq!(legacy.seed, 12345);
        assert_eq!(legacy.materials.air, 0);
        assert_eq!(legacy.materials.stone, 1);
        assert_eq!(legacy.materials.dirt, 2);
        assert_eq!(legacy.materials.grass, 3);
        assert_eq!(legacy.materials.bedrock, 4);
        assert_eq!(legacy.materials.snow, 5);
        assert_eq!(legacy.materials.sand, 6);
        assert_eq!(legacy.materials.clay, 7);
        assert_eq!(legacy.materials.gravel, 8);
        assert_eq!(legacy.materials.iron_ore, 9);
        assert_eq!(legacy.materials.coal_ore, 10);
        assert_eq!(legacy.materials.oak_log, 11);
        assert_eq!(legacy.materials.leaves, 12);
        assert_eq!(legacy.materials.water, 13);
        assert_eq!(legacy.materials.short_grass, 14);
        assert_eq!(legacy.perm, perm);
    }
}
