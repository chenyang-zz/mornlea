//! Typed native mesh/light provider surface.
//!
//! This module owns the validated registry constructor, the typed light build
//! and the staged geometry provider for the mesh/light lane. The borrowed
//! section view and the reusable scratch are the frozen contract types
//! ([`MeshView`] and [`MeshScratch`]); geometry is emitted by the shared
//! `greedy` core into the scratch's fixed quad stage and published only after
//! the complete stream and its exact count are known.
//!
//! Validation order is fixed: a typed registry is fully validated before any
//! light or geometry work, and the light entry re-validates the view's registry
//! so an externally constructed view can never reach the solver with a table
//! the byte lane would have rejected.

use crate::native::contracts::KernelError;
use crate::native::contracts::mesh::{
    MeshOp, MeshQuad, MeshRegistry, MeshRegistryEntry, MeshScratch, MeshView,
};

/// Builds a fully validated typed registry snapshot.
///
/// `MeshRegistry::try_new` owns the frozen structural contract (entry count,
/// strict id order and visibility word count). The shared semantic pass in
/// `crate::input` then closes the remaining ranges — light attenuation, short
/// block top height, fluid/short-block exclusivity and the sentinel rules — so
/// callers that hand the registry to the light lane are guaranteed the same
/// acceptance the byte parser grants.
pub fn try_new_registry(
    entries: &[MeshRegistryEntry],
    visibility: &[u64],
    air: u16,
    barrier: u16,
) -> Result<MeshRegistry, KernelError> {
    let registry = MeshRegistry::try_new(entries, visibility, air, barrier)?;
    crate::input::validate_typed_registry(&registry)?;
    Ok(registry)
}

/// Builds sky and block light for one typed section view into `scratch`.
///
/// The registry embedded in the view is validated first, including for an
/// all-air section, so a semantically invalid table fails before the solver
/// reads any block. On success the scratch owns the complete level volume and
/// an empty queue; reusing the same scratch for another view resets both before
/// that build. A rejected call may leave partial levels in the scratch but
/// publishes no light result. Present height columns require a representable
/// origin-relative sample window; absent columns never use that arithmetic.
pub fn build_light(view: &MeshView<'_>, scratch: &mut MeshScratch) -> Result<(), KernelError> {
    crate::input::validate_typed_registry(view.registry)?;
    // Native callers receive a typed rejection before the shared solver adds
    // neighborhood offsets. The transitional ABI retains its own admission.
    if view.heights_present.contains(&true)
        && (view.section_origin_y.checked_sub(16).is_none()
            || view.section_origin_y.checked_add(31).is_none())
    {
        return Err(KernelError::InvalidInput);
    }
    crate::light::build_light_view(view, scratch)
}

/// Zero-sized native provider for one section's mesh geometry.
///
/// Ownership: the provider is stateless. Light levels, the queue and the fixed
/// staging array all live in the caller-owned `MeshScratch`, so a warm caller
/// reuses a single allocation across sections. The provider borrows the view
/// for the call only and never keeps a reference to the destination.
pub struct NativeMesh;

impl MeshOp for NativeMesh {
    /// Builds light and stages the complete quad stream for one validated
    /// section view, then publishes it into `dst` once.
    ///
    /// The registry is validated before any light or geometry work. The shared
    /// geometry core writes checked records into the scratch's fixed 40960-quad
    /// stage: a record whose fields violate the packing domains, or a stream
    /// that would exceed the conservative stage bound, fails with
    /// `OutputInvariant` and leaves `dst` untouched. Only after the exact count
    /// is known is it compared with `dst.len()`: a shorter destination returns
    /// `OutputTooSmall` with the exact count and still publishes nothing, and a
    /// success copies the used prefix once.
    ///
    /// The scratch's level volume and queue hold the light result this call
    /// consumed; the staged prefix is only meaningful until the next `mesh`
    /// call on the same scratch.
    fn mesh(
        &self,
        view: &MeshView<'_>,
        scratch: &mut MeshScratch,
        dst: &mut [MeshQuad],
    ) -> Result<usize, KernelError> {
        build_light(view, scratch)?;
        // The center is section 13 in the owned 3x3x3 neighborhood. Preserve
        // the ABI's empty-center observation even when a valid custom table
        // marks air faces visible; light has already reset reusable scratch.
        if view.blocks[13 * 4096..14 * 4096]
            .iter()
            .all(|&block| block == view.registry.air())
        {
            return Ok(0);
        }
        let access = crate::greedy::TypedMeshAccess::new(view);
        let MeshScratch {
            levels,
            queue,
            stage,
        } = scratch;
        let light = crate::light::LightScratch::new(&mut levels[..], &mut queue[..]);
        let mut staged = crate::greedy::MeshStage::new(&mut stage[..]);
        crate::greedy::mesh_geometry(&access, &light, &mut staged)?;
        let quads = staged.output();
        if dst.len() < quads.len() {
            return Err(KernelError::OutputTooSmall {
                needed: quads.len(),
                available: dst.len(),
            });
        }
        dst[..quads.len()].copy_from_slice(quads);
        Ok(quads.len())
    }
}
