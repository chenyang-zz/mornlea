//! Typed native mesh/light provider surface.
//!
//! This module owns the validated registry constructor and the typed light
//! build for the mesh/light lane. The borrowed section view and the reusable
//! scratch are the frozen contract types ([`MeshView`] and [`MeshScratch`]);
//! geometry publication is not part of this module yet, so the typed surface
//! ends at the light result held in the scratch.
//!
//! Validation order is fixed: a typed registry is fully validated before any
//! light work, and the light entry re-validates the view's registry so an
//! externally constructed view can never reach the solver with a table the
//! byte lane would have rejected.

use crate::native::contracts::KernelError;
use crate::native::contracts::mesh::{MeshRegistry, MeshRegistryEntry, MeshScratch, MeshView};

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
/// publishes no light result.
pub fn build_light(view: &MeshView<'_>, scratch: &mut MeshScratch) -> Result<(), KernelError> {
    crate::input::validate_typed_registry(view.registry)?;
    crate::light::build_light_view(view, scratch)
}
