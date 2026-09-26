use crate::lod::{LodFace as ShellFace, sample_field_into, visit_lod_shell};
use crate::native::contracts::KernelError;
use crate::native::contracts::world::{LodFace, LodOp, LodQuad, LodRequest, LodScratch, LodStep};

/// Zero-sized native provider for bounded LOD shell generation.
///
/// Ownership: the provider is stateless and keeps no storage of its own.
/// Window samples and staged quads live in the caller-owned `LodScratch`, so a
/// warm caller reuses one allocation across every step size and tile; the only
/// per-call allocation is the claimed mask inside the shared walk. Quads are
/// staged first and only the used prefix is published into `dst` after the
/// exact required count is known, so a short destination never receives a
/// partial shell and surplus slots stay untouched.
pub struct NativeLod;

/// Maps the shell face onto the contract face at the typed boundary; the two
/// enums share discriminants but stay independent contracts.
fn map_face(face: ShellFace) -> LodFace {
    match face {
        ShellFace::Top => LodFace::Top,
        ShellFace::NegX => LodFace::NegX,
        ShellFace::PosX => LodFace::PosX,
        ShellFace::NegZ => LodFace::NegZ,
        ShellFace::PosZ => LodFace::PosZ,
    }
}

impl LodOp for NativeLod {
    /// Builds the deterministic shell of one tile into the caller-owned
    /// destination.
    ///
    /// Admission mirrors the legacy ABI gate before any sampling: per axis the
    /// tile origin times 64, the far edge `base + 64` and the boundary-ring
    /// reach `base - 8` must all be representable. The three checks run in
    /// independent chains per axis because each guards its own overflow, and
    /// chaining them through one shared base would wrongly admit tiles whose
    /// boundary ring wraps. A rejected request returns `InvalidInput` and
    /// leaves the destination untouched.
    ///
    /// Parameters are converted once per call. Sampling fills the caller's
    /// sample buffer and the shared walk stages quads into the caller's stage.
    /// The stage holds exactly the static worst-case quad count
    /// `3 * N * N + 2 * N` at `N = 32`, so the walk can never index past its
    /// last slot and no second bounds check is needed. Publication happens
    /// once: when `dst` is shorter than the required count the call fails with
    /// `OutputTooSmall` carrying that count without touching `dst`; on success
    /// only `dst[..count]` is written.
    fn build(
        &self,
        request: &LodRequest<'_>,
        scratch: &mut LodScratch,
        dst: &mut [LodQuad],
    ) -> Result<usize, KernelError> {
        for tile in request.tile {
            let base = tile.checked_mul(64).ok_or(KernelError::InvalidInput)?;
            base.checked_add(64).ok_or(KernelError::InvalidInput)?;
            base.checked_sub(8).ok_or(KernelError::InvalidInput)?;
        }
        let legacy = request.params.as_legacy();
        let step = match request.step {
            LodStep::Two => 2,
            LodStep::Four => 4,
            LodStep::Eight => 8,
        };
        let LodScratch { samples, stage } = scratch;
        let field = sample_field_into(&legacy, request.tile[0], request.tile[1], step, samples);
        let mut len = 0usize;
        visit_lod_shell(&field, legacy.materials.air, |quad| {
            stage[len] = LodQuad {
                x: quad.x,
                z: quad.z,
                y: quad.y,
                w: quad.w,
                d: quad.d,
                face: map_face(quad.face),
                material: quad.material,
                shade: quad.shade,
            };
            len += 1;
            true
        });
        if dst.len() < len {
            return Err(KernelError::OutputTooSmall {
                needed: len,
                available: dst.len(),
            });
        }
        dst[..len].copy_from_slice(&stage[..len]);
        Ok(len)
    }
}
