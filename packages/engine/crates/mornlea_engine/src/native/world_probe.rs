use crate::native::contracts::KernelError;
use crate::native::contracts::world::{ProbeOp, ProbeQuery, ProbeValue, WorldgenParams};

/// Maximum number of point queries accepted in one call; the bound is a
/// contract-level batch limit, not a scratch capacity.
const MAX_PROBES: usize = 64;

/// Zero-sized native provider for bounded world point probes.
///
/// Ownership: the provider is stateless and holds no scratch; every call
/// converts the validated world parameters once and evaluates against that
/// local value.
pub struct NativeWorldProbe;

impl ProbeOp for NativeWorldProbe {
    /// Evaluates a bounded batch of world point queries.
    ///
    /// Validation order is part of the contract: the query count must be in
    /// `1..=MAX_PROBES` and the destination must hold at least one slot per
    /// query before any evaluation happens, so a rejected call never touches
    /// `dst`. Results are staged in a fixed local buffer and published only
    /// after every query evaluated, keeping the used prefix all-or-nothing.
    fn probe(
        &self,
        params: &WorldgenParams,
        queries: &[ProbeQuery],
        dst: &mut [ProbeValue],
    ) -> Result<usize, KernelError> {
        if queries.is_empty() || queries.len() > MAX_PROBES {
            return Err(KernelError::InvalidInput);
        }
        let count = queries.len();
        if dst.len() < count {
            return Err(KernelError::OutputTooSmall {
                needed: count,
                available: dst.len(),
            });
        }
        // One legacy parameter conversion per call; the samplers are
        // read-only, so evaluation cannot mutate the caller's world.
        let legacy = params.as_legacy();
        let mut stage = [ProbeValue::default(); MAX_PROBES];
        for (slot, query) in stage[..count].iter_mut().zip(queries) {
            *slot = match *query {
                ProbeQuery::Height { x, z } => ProbeValue::Height(legacy.height_at(x, z)),
                ProbeQuery::Terrain {
                    position: [x, y, z],
                } => ProbeValue::Block(legacy.terrain_block_at(x, y, z)),
                ProbeQuery::Base {
                    position: [x, y, z],
                } => ProbeValue::Block(legacy.base_block_at(x, y, z)),
            };
        }
        dst[..count].copy_from_slice(&stage[..count]);
        Ok(count)
    }
}
