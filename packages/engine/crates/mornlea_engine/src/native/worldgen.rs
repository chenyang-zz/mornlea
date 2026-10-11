use crate::native::contracts::KernelError;
use crate::native::contracts::world::{Materials, WorldgenOp, WorldgenParams, WorldgenScratch};

/// Builds owned numerical inputs with the Go-compatible seeded permutation.
/// Material validation remains at the typed boundary; dimension policy belongs to callers.
pub fn seeded_params(seed: i64, materials: Materials) -> Result<WorldgenParams, KernelError> {
    WorldgenParams::try_new(seed, materials, super::go_random::permutation(seed))
}

pub struct NativeWorldgen;

impl WorldgenOp for NativeWorldgen {
    fn generate_chunk(
        &self,
        params: &WorldgenParams,
        chunk: [i32; 2],
        scratch: &mut WorldgenScratch,
        dst: &mut [u16],
    ) -> Result<usize, KernelError> {
        if dst.len() < 98304 {
            return Err(KernelError::OutputTooSmall {
                needed: 98304,
                available: dst.len(),
            });
        }
        let legacy_params = params.as_legacy();
        legacy_params.generate_chunk(chunk[0], chunk[1], &mut *scratch.stage);
        dst[..98304].copy_from_slice(&*scratch.stage);
        Ok(98304)
    }
}
