//! Whole-section `RenderingServer` RID table for the pilot Godot terrain path.
//!
//! Design decision 5 of the Godot client migration: after the mesh-prepare
//! worker expanded a section payload into owned per-surface arrays, the
//! extension manages terrain meshes and instances through the low-level
//! `RenderingServer`, and section identity is
//! `(dimension, x, y, z, revision)`. This module owns the table half of
//! that ruling: replacement and drop are atomic whole-section operations,
//! and no intermediate state is ever partially visible.
//!
//! Threading model: the table is data plus a [`RenderBackend`], and the
//! production backend is main-thread-only — every `RenderingServer` call
//! it makes is guarded by `godot::init::is_main_thread` exactly the way
//! the bridge guards its instance methods, answering off-thread calls
//! with the stable internal status word instead of panicking or touching
//! engine state. Cargo tests have no live Godot engine, so every backend
//! interaction sits behind the [`RenderBackend`] seam: the scripted test
//! backend allocates monotonic fake RIDs, records an event and free
//! history, panics on double frees and use-after-free, and injects
//! submission failures.
//!
//! Atomicity contract of [`TerrainRenderer::upsert_section`]: ALL surfaces
//! of the replacement mesh are built before any live state changes. The
//! new mesh is only attached to the scenario by the final
//! instance-registration call, which happens after every surface
//! submission succeeded — so a section never appears partially drawn. On
//! any failure mid-build, only the partially built NEW resources are
//! freed; the old section stays fully intact and visible, and the table
//! records no revision for the failed attempt (a retry of the same
//! revision is therefore not stale, mirroring the all-or-nothing
//! presentation batch whose rejection consumes no state).
//!
//! Revision policy, pinned against the Go presentation contract
//! (`presentation.WorldBatch.ValidateAfter`): revisions are strictly
//! monotonic per section and zero is not a valid revision. An upsert or
//! drop with a revision less than OR EQUAL to the stored revision is
//! refused with `abi::STATUS_INPUT_REJECTED` without any backend call —
//! an equal-revision re-upsert is a producer bug, not an idempotent
//! retry, because the producer's own validation rejects it the same way.
//! Dropping a section the table never held is a successful no-op: the Go
//! validation rule only compares revisions of sections that exist, so a
//! batch-valid drop of an absent section can legitimately reach the
//! renderer.
//!
//! Ownership boundary: the renderer does NOT create or free materials,
//! the scenario, or the atlas texture. The three per-surface-class
//! material RIDs (opaque, cutout, water) and the scenario RID are
//! construction inputs owned by the integration that wires the bridge to
//! the world feature; the table frees only the mesh and instance RIDs it
//! allocated itself. Instance transforms carry the section world origin
//! (section coordinates times sixteen blocks per edge, with storage Y
//! translated by the world's minimum height); vertex positions
//! stay section-local per the terrain shader contract, so the GPU adds
//! them.
//!
//! Accounting: each live section holds at most
//! [`SECTION_SURFACE_CLASSES`] surfaces inside exactly two table-owned
//! RIDs (one mesh plus one instance — surfaces are mesh-internal, not
//! separate RIDs), so the worst case is two table-owned RIDs per live
//! section on top of the three borrowed material RIDs and the borrowed
//! scenario. The live-section count itself is bounded by the caller's
//! upload budget and reclamation policy, which the later terrain tasks
//! own; [`TerrainRenderer::facts`] is the pilot report's decodable
//! accounting surface for those limits.

// This module's `#![allow(dead_code)]` era ended when the bridge terrain
// surface became its production consumer: every item below is either
// production code (the table, the backend, the constructors the bridge
// assembles through) or test-only through an explicit `#[cfg(test)]`.

use std::collections::BTreeMap;

use godot::classes::RenderingServer;
use godot::classes::mesh::ArrayType;
use godot::classes::rendering_server::{ArrayCustomFormat, ArrayFormat, PrimitiveType};
use godot::obj::{EngineBitfield, EngineEnum};
use godot::prelude::*;

use crate::abi;
use crate::mesh_worker::SectionId;
use crate::quad_decode::{SectionGeometry, SurfaceGeometry};

/// Blocks along one section edge. The mesher's frozen worst case is six
/// quads per block, so `abi::MAX_SECTION_MESH_QUADS` equals six times this
/// constant cubed; the pinning tests assert that relationship.
pub(crate) const SECTION_EDGE_BLOCKS: i32 = abi::SECTION_EDGE_BLOCKS as i32;

/// Surface classes one section can carry: opaque, cutout, and water, in
/// submission order. Surface submission order is stable (opaque, cutout,
/// water) because the surface index inside the Godot mesh — and the
/// failure-injection points of the tests — depend on it. The production
/// paths decide order through `surface_plan`; the named count is the
/// accounting bound the tests pin against.
#[cfg(test)]
pub(crate) const SECTION_SURFACE_CLASSES: usize = 3;

/// The three per-surface-class material RIDs the terrain shaders of the
/// pilot world feature define. Borrowed inputs: the renderer assigns them
/// to mesh surfaces but never creates or frees them.
#[derive(Clone, Copy, Debug)]
pub(crate) struct TerrainMaterials {
    pub(crate) opaque: Rid,
    pub(crate) cutout: Rid,
    pub(crate) water: Rid,
}

/// The revision-free identity of one world section: dimension, signed X/Z
/// coordinates, and a zero-based storage Y index, the world-family vocabulary of
/// `core.SectionKey`. The table keys its map on this coordinate; the
/// presentation revision that rides along in [`SectionId`] is tracked per
/// entry instead.
#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct SectionCoord {
    pub(crate) dimension: u32,
    pub(crate) x: i32,
    pub(crate) y: i32,
    pub(crate) z: i32,
}

impl SectionCoord {
    /// The map key of one operation record's identity.
    pub(crate) fn of(id: SectionId) -> Self {
        Self {
            dimension: id.dimension,
            x: id.x,
            y: id.y,
            z: id.z,
        }
    }
}

/// The render backend operations the RID table needs, in the crate's
/// core-call seam style: production calls the real `RenderingServer`
/// singleton (main-thread only), tests run a scripted table that
/// allocates fake RIDs and injects failures. Every method either fails
/// without leaving a resource the caller must free, or hands the caller
/// the allocated RID — the table's rollback frees exactly the RIDs it
/// received.
pub(crate) trait RenderBackend {
    /// Allocate one empty mesh RID.
    fn mesh_create(&self) -> Result<Rid, u32>;

    /// Append one surface built from the per-surface arrays, with the
    /// per-surface material RID assigned to the new surface index.
    /// Failing must not grow the mesh.
    fn mesh_add_surface(
        &self,
        mesh: Rid,
        surface: &SurfaceGeometry,
        material: Rid,
    ) -> Result<(), u32>;

    /// Create one instance, point it at `mesh`, register it into
    /// `scenario`, and place it at `transform`. The instance RID is the
    /// only step that makes a mesh visible, so callers submit every
    /// surface before calling this.
    fn instance_create(&self, mesh: Rid, scenario: Rid, transform: Transform3D)
    -> Result<Rid, u32>;

    /// Free one table-owned RID. Materials and the scenario are never
    /// passed here.
    fn free_rid(&self, rid: Rid);
}

/// The production backend over the real `RenderingServer` singleton.
///
/// Main-thread discipline: Godot servers are main-thread-only, so every
/// method answers off-thread calls with `abi::STATUS_INTERNAL` without
/// touching the engine — mirroring the bridge's `on_main_thread` guard.
/// Failure detection is postcondition-based because the low-level calls
/// report misuse through Godot's error log instead of return values: a
/// returned mesh or instance RID must be valid, and a surface submission
/// must grow the mesh's surface count.
struct ServerRenderBackend;

impl ServerRenderBackend {
    fn server() -> Gd<RenderingServer> {
        RenderingServer::singleton()
    }

    fn on_main_thread() -> bool {
        godot::init::is_main_thread()
    }
}

impl RenderBackend for ServerRenderBackend {
    fn mesh_create(&self) -> Result<Rid, u32> {
        if !Self::on_main_thread() {
            return Err(abi::STATUS_INTERNAL);
        }
        let mut server = Self::server();
        let mesh = server.mesh_create();
        if mesh.is_valid() {
            Ok(mesh)
        } else {
            Err(abi::STATUS_INTERNAL)
        }
    }

    fn mesh_add_surface(
        &self,
        mesh: Rid,
        surface: &SurfaceGeometry,
        material: Rid,
    ) -> Result<(), u32> {
        if !Self::on_main_thread() {
            return Err(abi::STATUS_INTERNAL);
        }
        // The table skips empty surface classes; reaching this with an
        // empty geometry would submit a degenerate Godot surface.
        if surface.vertices.is_empty() || surface.indices.is_empty() {
            return Err(abi::STATUS_INPUT_REJECTED);
        }
        let arrays = build_surface_arrays(surface);
        let mut server = Self::server();
        let before = server.mesh_get_surface_count(mesh);
        server
            .mesh_add_surface_from_arrays_ex(mesh, PrimitiveType::TRIANGLES, &arrays)
            .compress_format(custom0_format_flags())
            .done();
        let after = server.mesh_get_surface_count(mesh);
        // The low-level call returns nothing; a surface count that did not
        // grow by exactly one means Godot rejected the arrays.
        if after != before + 1 {
            return Err(abi::STATUS_INTERNAL);
        }
        server.mesh_surface_set_material(mesh, after - 1, material);
        Ok(())
    }

    fn instance_create(
        &self,
        mesh: Rid,
        scenario: Rid,
        transform: Transform3D,
    ) -> Result<Rid, u32> {
        if !Self::on_main_thread() {
            return Err(abi::STATUS_INTERNAL);
        }
        let mut server = Self::server();
        let instance = server.instance_create();
        if instance.is_invalid() {
            return Err(abi::STATUS_INTERNAL);
        }
        // The three registrations report misuse through Godot's own error
        // log and have no decodable failure; the valid instance RID is the
        // caller's to free either way.
        server.instance_set_base(instance, mesh);
        server.instance_set_scenario(instance, scenario);
        server.instance_set_transform(instance, transform);
        Ok(instance)
    }

    fn free_rid(&self, rid: Rid) {
        if !Self::on_main_thread() {
            // A wrong-thread free must never touch the engine; refuse it
            // loudly instead of racing the render thread. The scripted
            // backend treats any unaccounted free as a test failure, so a
            // table bug that reaches this branch is caught in tests.
            godot_error!("[mornlea-terrain] refusing off-main-thread RID free");
            return;
        }
        Self::server().free_rid(rid);
    }
}

/// The production backend constructor, in the `production_core_calls`
/// style. Never call it from cargo tests: without a live Godot engine the
/// first singleton access is not a decodable failure.
pub(crate) fn production_render_backend() -> Box<dyn RenderBackend> {
    Box::new(ServerRenderBackend)
}

/// Build the Godot mesh surface array layout the pilot terrain shaders
/// document: `VERTEX`/`NORMAL` (`PackedVector3Array`), `TEX_UV`
/// (`PackedVector2Array`), `CUSTOM0` as one RGBA float per vertex
/// (`shade`, `sky`, `block`, atlas layer), and `INDEX`
/// (`PackedInt32Array`). Unused slots stay `null`, which the low-level
/// surface call accepts.
fn build_surface_arrays(surface: &SurfaceGeometry) -> VarArray {
    let mut arrays = VarArray::new();
    for _ in 0..ArrayType::MAX.ord() {
        arrays.push(&Variant::nil());
    }
    let positions =
        PackedVector3Array::from_iter(surface.vertices.iter().map(|vertex| {
            Vector3::new(vertex.position[0], vertex.position[1], vertex.position[2])
        }));
    let normals = PackedVector3Array::from_iter(
        surface
            .vertices
            .iter()
            .map(|vertex| Vector3::new(vertex.normal[0], vertex.normal[1], vertex.normal[2])),
    );
    let uvs = PackedVector2Array::from_iter(
        surface
            .vertices
            .iter()
            .map(|vertex| Vector2::new(vertex.uv[0], vertex.uv[1])),
    );
    // CUSTOM0 RGBA float: x = shade, y = sky nibble, z = block nibble,
    // w = atlas layer — the exact contract of the terrain shader family.
    let custom0 = PackedFloat32Array::from_iter(surface.vertices.iter().flat_map(|vertex| {
        [
            vertex.shade,
            f32::from(vertex.sky),
            f32::from(vertex.block),
            f32::from(vertex.layer),
        ]
    }));
    let indices = PackedInt32Array::from_iter(
        surface
            .indices
            .iter()
            .map(|index| i32::try_from(*index).unwrap_or(i32::MAX)),
    );
    arrays.set(slot(ArrayType::VERTEX), &Variant::from(positions));
    arrays.set(slot(ArrayType::NORMAL), &Variant::from(normals));
    arrays.set(slot(ArrayType::TEX_UV), &Variant::from(uvs));
    arrays.set(slot(ArrayType::CUSTOM0), &Variant::from(custom0));
    arrays.set(slot(ArrayType::INDEX), &Variant::from(indices));
    arrays
}

/// The custom-channel encoding cannot be derived from the packed arrays,
/// so the RGBA float contract is passed explicitly through the surface
/// call's flag parameter: the `ArrayCustomFormat` value shifted into the
/// `CUSTOM0` slot.
fn custom0_format_flags() -> ArrayFormat {
    // Bitfield ordinals are already `u64`; the enum ordinal is not.
    EngineBitfield::from_ord(
        (ArrayCustomFormat::RGBA_FLOAT.ord() as u64) << ArrayFormat::CUSTOM0_SHIFT.ord(),
    )
}

fn slot(array_type: ArrayType) -> usize {
    array_type.ord() as usize
}

/// The instance transform of one section: identity basis with the section
/// world origin. Y is a storage index measured from the world's minimum
/// height. Section coordinates are exact integers and the edge is
/// sixteen blocks, so float math stays exact for every representable
/// section coordinate.
fn section_transform(coord: &SectionCoord) -> Transform3D {
    let origin = Vector3::new(
        (coord.x as f32) * (SECTION_EDGE_BLOCKS as f32),
        (coord.y as f32) * (SECTION_EDGE_BLOCKS as f32) - (abi::WORLD_Y_BIAS_BLOCKS as f32),
        (coord.z as f32) * (SECTION_EDGE_BLOCKS as f32),
    );
    Transform3D::new(Basis::IDENTITY, origin)
}

/// The live resources of one held section. `surfaces` counts the
/// non-empty surface classes submitted into the mesh; it never exceeds
/// [`SECTION_SURFACE_CLASSES`].
struct SectionEntry {
    revision: u64,
    mesh: Rid,
    instance: Rid,
    surfaces: usize,
}

/// The decodable accounting snapshot of the table, in the worker-facts
/// style: how many whole sections are live and how many mesh surfaces
/// they hold. The pilot report's RID and mesh counts derive from these
/// (two table-owned RIDs per section, surfaces live inside the mesh).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct RendererFacts {
    pub(crate) sections: usize,
    pub(crate) surfaces: usize,
}

/// The structural identity of one live section for the bridge's summary
/// surface: the coordinate, the recorded presentation revision, and how
/// many non-empty surface classes the section's mesh carries.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct SectionSummary {
    pub(crate) coord: SectionCoord,
    pub(crate) revision: u64,
    pub(crate) surfaces: usize,
}

/// The whole-section `RenderingServer` RID table; see the module
/// documentation for the threading, atomicity, revision, and ownership
/// contracts.
pub(crate) struct TerrainRenderer {
    backend: Box<dyn RenderBackend>,
    scenario: Rid,
    materials: TerrainMaterials,
    /// Keyed by section coordinate; the `BTreeMap` order makes reset and
    /// per-dimension enumeration deterministic.
    sections: BTreeMap<SectionCoord, SectionEntry>,
    /// Total live surfaces across all sections (at most
    /// [`SECTION_SURFACE_CLASSES`] per section).
    surfaces: usize,
}

impl TerrainRenderer {
    /// Assemble the table over one backend, the borrowed scenario RID,
    /// and the three borrowed per-surface-class material RIDs.
    pub(crate) fn new(
        backend: Box<dyn RenderBackend>,
        scenario: Rid,
        materials: TerrainMaterials,
    ) -> Self {
        Self {
            backend,
            scenario,
            materials,
            sections: BTreeMap::new(),
            surfaces: 0,
        }
    }

    /// Replace one whole section at `id.revision`.
    ///
    /// Rejection domain (no backend call, no state change): zero
    /// revision, a revision less than or equal to the stored one, or a
    /// geometry the table refuses — no non-empty surface class at all
    /// (removing a section is a drop, the presentation payload rule), a
    /// non-empty surface without indices, or a surface above the frozen
    /// per-section vertex bound.
    ///
    /// Success path: every surface of the new mesh is submitted, the
    /// instance registration attaches the finished mesh to the scenario,
    /// and only then is the old section freed — the old and new meshes
    /// coexist during the swap so a failure can never leave a hole.
    pub(crate) fn upsert_section(
        &mut self,
        id: SectionId,
        geometry: &SectionGeometry,
    ) -> Result<(), u32> {
        if id.revision == 0 {
            return Err(abi::STATUS_INPUT_REJECTED);
        }
        validate_geometry(geometry)?;
        let coord = SectionCoord::of(id);
        if let Some(entry) = self.sections.get(&coord)
            && id.revision <= entry.revision
        {
            return Err(abi::STATUS_INPUT_REJECTED);
        }
        let new_entry = self.build_section(&coord, id.revision, geometry)?;
        self.surfaces += new_entry.surfaces;
        if let Some(old) = self.sections.insert(coord, new_entry) {
            self.surfaces -= old.surfaces;
            self.free_section(&old);
        }
        Ok(())
    }

    /// Drop one whole section at `id.revision`, freeing its RIDs exactly
    /// once. A drop of a section the table does not hold is a successful
    /// no-op; a drop at a revision less than or equal to the stored one
    /// (and a zero revision) is refused without any backend call.
    pub(crate) fn drop_section(&mut self, id: SectionId) -> Result<(), u32> {
        if id.revision == 0 {
            return Err(abi::STATUS_INPUT_REJECTED);
        }
        let coord = SectionCoord::of(id);
        if let Some(entry) = self.sections.get(&coord)
            && id.revision <= entry.revision
        {
            return Err(abi::STATUS_INPUT_REJECTED);
        }
        if let Some(entry) = self.sections.remove(&coord) {
            self.surfaces -= entry.surfaces;
            self.free_section(&entry);
        }
        Ok(())
    }

    /// Session reset: free every held section in section-coordinate order
    /// (instance before mesh within each section), leaving an empty
    /// table that accepts fresh upserts.
    pub(crate) fn reset(&mut self) {
        for entry in std::mem::take(&mut self.sections).into_values() {
            self.free_section(&entry);
        }
        self.surfaces = 0;
    }

    /// The held section coordinates of one dimension, in coordinate
    /// order. Cheap by construction: a filtered walk of the ordered map.
    /// The budget stage consumes the iterator form in production; this
    /// snapshot form serves the tests that pin exact coordinate sets.
    #[cfg(test)]
    pub(crate) fn sections_in_dimension(&self, dimension: u32) -> Vec<SectionCoord> {
        self.sections
            .keys()
            .filter(|coord| coord.dimension == dimension)
            .copied()
            .collect()
    }

    /// The held section coordinates of one dimension, in coordinate order,
    /// without building a snapshot. The per-frame budget stage consumes
    /// this for out-of-view reclamation: a `BTreeMap::range` prefix over
    /// the dimension keeps the per-frame walk allocation-free and linear in
    /// held sections, so per-frame consumption is never quadratic and never
    /// copies the whole table into a throwaway `Vec`.
    pub(crate) fn sections_in_dimension_iter(
        &self,
        dimension: u32,
    ) -> impl Iterator<Item = SectionCoord> + '_ {
        let first = SectionCoord {
            dimension,
            x: i32::MIN,
            y: i32::MIN,
            z: i32::MIN,
        };
        let last = SectionCoord {
            dimension,
            x: i32::MAX,
            y: i32::MAX,
            z: i32::MAX,
        };
        self.sections.range(first..=last).map(|(coord, _)| *coord)
    }

    /// The structural identity of one live section for the bridge's summary
    /// surface: the coordinate, the recorded presentation revision, and how
    /// many non-empty surface classes the section's mesh carries.
    pub(crate) fn section_summaries(&self) -> Vec<SectionSummary> {
        self.sections
            .iter()
            .map(|(coord, entry)| SectionSummary {
                coord: *coord,
                revision: entry.revision,
                surfaces: entry.surfaces,
            })
            .collect()
    }

    /// Owner-side reclamation: free one held section by coordinate alone,
    /// with no revision arbitration, and report whether a section was
    /// freed. Producer operations go through [`TerrainRenderer::upsert_section`]
    /// and [`TerrainRenderer::drop_section`] with revision discipline;
    /// reclamation is the table owner reclaiming its own resources — the
    /// same ownership action [`TerrainRenderer::reset`] performs on every
    /// section at once — so no producer revision exists to compare and
    /// none is consulted. Removing an absent section is a no-op returning
    /// `false`.
    pub(crate) fn reclaim_section(&mut self, coord: SectionCoord) -> bool {
        if let Some(entry) = self.sections.remove(&coord) {
            self.surfaces -= entry.surfaces;
            self.free_section(&entry);
            true
        } else {
            false
        }
    }

    /// The pilot-report accounting snapshot.
    pub(crate) fn facts(&self) -> RendererFacts {
        RendererFacts {
            sections: self.sections.len(),
            surfaces: self.surfaces,
        }
    }

    /// Build one complete section entry without touching live state: the
    /// mesh is created and fully surfaced, the instance registration (the
    /// only step that makes anything visible) runs last, and on any
    /// failure the partially built new mesh is freed while the caller's
    /// old section stays untouched.
    fn build_section(
        &self,
        coord: &SectionCoord,
        revision: u64,
        geometry: &SectionGeometry,
    ) -> Result<SectionEntry, u32> {
        let mesh = self.backend.mesh_create()?;
        let built = match self.submit_surfaces(mesh, geometry) {
            Ok(surfaces) => {
                match self
                    .backend
                    .instance_create(mesh, self.scenario, section_transform(coord))
                {
                    Ok(instance) => Ok(SectionEntry {
                        revision,
                        mesh,
                        instance,
                        surfaces,
                    }),
                    Err(word) => Err(word),
                }
            }
            Err(word) => Err(word),
        };
        match built {
            Ok(entry) => Ok(entry),
            // Preservation of old resources on failure: only the new mesh
            // (with however many surfaces landed inside it) is freed, and
            // no revision is recorded for the failed attempt.
            Err(word) => {
                self.backend.free_rid(mesh);
                Err(word)
            }
        }
    }

    /// Submit every non-empty surface class in stable order and report
    /// how many surfaces the mesh carries.
    fn submit_surfaces(&self, mesh: Rid, geometry: &SectionGeometry) -> Result<usize, u32> {
        let mut submitted = 0;
        for (surface, material) in self.surface_plan(geometry) {
            self.backend.mesh_add_surface(mesh, surface, material)?;
            submitted += 1;
        }
        Ok(submitted)
    }

    /// The non-empty surface classes with their material RIDs, in the
    /// stable opaque/cutout/water order.
    fn surface_plan<'a>(
        &'a self,
        geometry: &'a crate::quad_decode::SectionGeometry,
    ) -> impl Iterator<Item = (&'a SurfaceGeometry, Rid)> {
        let classes = [
            (&geometry.opaque, self.materials.opaque),
            (&geometry.cutout, self.materials.cutout),
            (&geometry.water, self.materials.water),
        ];
        classes
            .into_iter()
            .filter(|(surface, _)| !surface.vertices.is_empty())
    }

    /// Free one section's RIDs exactly once, instance before mesh: the
    /// instance detaches from the scenario before the mesh it draws goes
    /// away, and the deterministic order is what the scripted free
    /// history asserts.
    fn free_section(&self, entry: &SectionEntry) {
        self.backend.free_rid(entry.instance);
        self.backend.free_rid(entry.mesh);
    }
}

impl Drop for TerrainRenderer {
    /// Drop is `reset`: an owner that forgets explicit teardown still
    /// frees every table-owned RID before the backend is dropped, so no
    /// mesh or instance outlives the table into engine shutdown.
    fn drop(&mut self) {
        self.reset();
    }
}

/// The geometry domain the table accepts, checked before any backend
/// call: at least one non-empty surface class (removing a section is a
/// drop), every non-empty surface indexed, and the frozen per-section
/// vertex bound respected. Decode owns the deep per-quad validation;
/// these are the paranoid bounds that keep submission inputs decodable.
fn validate_geometry(geometry: &SectionGeometry) -> Result<(), u32> {
    let classes = [&geometry.opaque, &geometry.cutout, &geometry.water];
    let mut any_surface = false;
    for surface in classes {
        if surface.vertices.is_empty() {
            continue;
        }
        any_surface = true;
        if surface.indices.is_empty() {
            return Err(abi::STATUS_INPUT_REJECTED);
        }
        if surface.vertices.len() > abi::MAX_SECTION_MESH_QUADS as usize * 4 {
            return Err(abi::STATUS_INPUT_REJECTED);
        }
    }
    if !any_surface {
        return Err(abi::STATUS_INPUT_REJECTED);
    }
    Ok(())
}

/// Test-shared scripted render backend, in the bridge's `ScriptedCore`
/// style. Shared by the RID-table tests below and the per-frame budget
/// stage tests: both need the same correctness oracle — a backend that
/// allocates monotonic fake RIDs, records an event and free history,
/// panics on double frees, frees of foreign RIDs, and use-after-free, and
/// injects submission failures. One implementation keeps the oracle
/// knowledge written once.
#[cfg(test)]
pub(crate) mod render_script {
    use super::{RenderBackend, TerrainMaterials, TerrainRenderer};
    use crate::abi;
    use crate::quad_decode::SurfaceGeometry;
    use godot::prelude::*;
    use std::cell::RefCell;
    use std::collections::BTreeSet;
    use std::rc::Rc;

    /// Borrowed caller-owned RIDs, far above the scripted allocator's
    /// range so an accidental table free of them cannot alias an
    /// allocated fake RID.
    pub(crate) const SCRIPT_SCENARIO: Rid = Rid::new(900_000);
    pub(crate) const SCRIPT_OPAQUE: Rid = Rid::new(900_001);
    pub(crate) const SCRIPT_CUTOUT: Rid = Rid::new(900_002);
    pub(crate) const SCRIPT_WATER: Rid = Rid::new(900_003);

    /// One recorded backend interaction; tests assert on the event
    /// history to prove submission order, material wiring, instance
    /// origins, and the free sequence.
    #[derive(Clone, Debug, PartialEq)]
    pub(crate) enum ScriptEvent {
        MeshCreated {
            mesh: Rid,
        },
        SurfaceAdded {
            mesh: Rid,
            vertices: usize,
            material: Rid,
        },
        InstanceCreated {
            mesh: Rid,
            scenario: Rid,
            origin: Transform3D,
        },
        Freed {
            rid: Rid,
        },
    }

    /// The mutable half of the scripted backend.
    struct ScriptState {
        next_rid: u64,
        allocated: Vec<u64>,
        live: BTreeSet<u64>,
        events: Vec<ScriptEvent>,
        fail_mesh_create: bool,
        /// Fail the mesh_add_surface call when this many surface calls
        /// remain; each successful call spends one.
        fail_surface_in: Option<usize>,
        fail_instance: bool,
    }

    impl ScriptState {
        /// Monotonic fake RID allocation; ids are never reused.
        fn allocate(&mut self) -> Rid {
            self.next_rid += 1;
            let rid = Rid::new(self.next_rid);
            self.live.insert(self.next_rid);
            self.allocated.push(self.next_rid);
            rid
        }
    }

    /// A scripted render backend shared with the test through an `Rc`, in
    /// the bridge's `ScriptedCore` style: the renderer owns its boxed
    /// copy while the test keeps scripting and inspecting the same
    /// state. The backend doubles as the correctness oracle — freeing an
    /// unknown or already-freed RID, or submitting to a freed mesh,
    /// panics with a named message, so every test using it proves the
    /// absence of double frees, leaks into the engine, and
    /// use-after-free.
    #[derive(Clone)]
    pub(crate) struct ScriptedBackend {
        state: Rc<RefCell<ScriptState>>,
    }

    impl ScriptedBackend {
        pub(crate) fn new() -> Self {
            Self {
                state: Rc::new(RefCell::new(ScriptState {
                    next_rid: 0,
                    allocated: Vec::new(),
                    live: BTreeSet::new(),
                    events: Vec::new(),
                    fail_mesh_create: false,
                    fail_surface_in: None,
                    fail_instance: false,
                })),
            }
        }

        /// Make the next mesh allocation fail once.
        pub(crate) fn fail_mesh_create(&self) {
            self.state.borrow_mut().fail_mesh_create = true;
        }

        /// Make the `remaining`-th surface submission from now fail
        /// (0 = the very next one); earlier calls succeed.
        pub(crate) fn fail_surface_in(&self, remaining: usize) {
            self.state.borrow_mut().fail_surface_in = Some(remaining);
        }

        /// Make the next instance registration fail once.
        pub(crate) fn fail_instance(&self) {
            self.state.borrow_mut().fail_instance = true;
        }

        /// Clear every injected failure.
        pub(crate) fn recover(&self) {
            let mut state = self.state.borrow_mut();
            state.fail_mesh_create = false;
            state.fail_surface_in = None;
            state.fail_instance = false;
        }

        pub(crate) fn events(&self) -> Vec<ScriptEvent> {
            self.state.borrow().events.clone()
        }

        pub(crate) fn is_alive(&self, rid: Rid) -> bool {
            self.state.borrow().live.contains(&rid.to_u64())
        }

        /// The freed RID ids in free order.
        pub(crate) fn free_history(&self) -> Vec<u64> {
            self.state
                .borrow()
                .events
                .iter()
                .filter_map(|event| match event {
                    ScriptEvent::Freed { rid } => Some(rid.to_u64()),
                    _ => None,
                })
                .collect()
        }

        /// Every id the backend ever allocated, in allocation order.
        pub(crate) fn allocated(&self) -> Vec<u64> {
            self.state.borrow().allocated.clone()
        }

        /// The ids still holding a live fake resource.
        pub(crate) fn live_ids(&self) -> Vec<u64> {
            self.state.borrow().live.iter().copied().collect()
        }

        pub(crate) fn renderer(&self) -> TerrainRenderer {
            TerrainRenderer::new(
                Box::new(self.clone()),
                SCRIPT_SCENARIO,
                TerrainMaterials {
                    opaque: SCRIPT_OPAQUE,
                    cutout: SCRIPT_CUTOUT,
                    water: SCRIPT_WATER,
                },
            )
        }

        /// One scripted renderer plus the shared script handle, the
        /// seam-test assembly shared by the table and budget suites.
        pub(crate) fn scripted_renderer() -> (TerrainRenderer, ScriptedBackend) {
            let backend = ScriptedBackend::new();
            let renderer = backend.renderer();
            (renderer, backend)
        }
    }

    impl RenderBackend for ScriptedBackend {
        fn mesh_create(&self) -> Result<Rid, u32> {
            let mut state = self.state.borrow_mut();
            if state.fail_mesh_create {
                state.fail_mesh_create = false;
                return Err(abi::STATUS_INTERNAL);
            }
            let mesh = state.allocate();
            state.events.push(ScriptEvent::MeshCreated { mesh });
            Ok(mesh)
        }

        fn mesh_add_surface(
            &self,
            mesh: Rid,
            surface: &SurfaceGeometry,
            material: Rid,
        ) -> Result<(), u32> {
            let mut state = self.state.borrow_mut();
            assert!(
                state.live.contains(&mesh.to_u64()),
                "surface submitted to a freed or unknown mesh {}",
                mesh.to_u64()
            );
            if let Some(remaining) = state.fail_surface_in {
                if remaining == 0 {
                    state.fail_surface_in = None;
                    return Err(abi::STATUS_INTERNAL);
                }
                state.fail_surface_in = Some(remaining - 1);
            }
            state.events.push(ScriptEvent::SurfaceAdded {
                mesh,
                vertices: surface.vertices.len(),
                material,
            });
            Ok(())
        }

        fn instance_create(
            &self,
            mesh: Rid,
            scenario: Rid,
            transform: Transform3D,
        ) -> Result<Rid, u32> {
            let mut state = self.state.borrow_mut();
            assert!(
                state.live.contains(&mesh.to_u64()),
                "instance registered on a freed or unknown mesh {}",
                mesh.to_u64()
            );
            if state.fail_instance {
                state.fail_instance = false;
                return Err(abi::STATUS_INTERNAL);
            }
            let instance = state.allocate();
            state.events.push(ScriptEvent::InstanceCreated {
                mesh,
                scenario,
                origin: transform,
            });
            Ok(instance)
        }

        fn free_rid(&self, rid: Rid) {
            let mut state = self.state.borrow_mut();
            let id = rid.to_u64();
            assert!(
                state.live.remove(&id),
                "double free or foreign RID {id}: the table may free only RIDs it allocated"
            );
            state.events.push(ScriptEvent::Freed { rid });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::render_script::{
        SCRIPT_CUTOUT, SCRIPT_OPAQUE, SCRIPT_SCENARIO, SCRIPT_WATER, ScriptEvent, ScriptedBackend,
    };
    use super::{
        RendererFacts, SECTION_EDGE_BLOCKS, SECTION_SURFACE_CLASSES, SectionCoord, TerrainRenderer,
    };
    use crate::abi;
    use crate::mesh_worker::SectionId;
    use crate::quad_decode::{ExpandedVertex, QUAD_INDICES, SectionGeometry, SurfaceGeometry};
    use godot::prelude::*;

    /// One scripted renderer plus its shared script handle: the
    /// seam-test assembly of this suite.
    fn scripted_renderer() -> (TerrainRenderer, ScriptedBackend) {
        ScriptedBackend::scripted_renderer()
    }

    fn vertex(layer: u16) -> ExpandedVertex {
        ExpandedVertex {
            position: [1.0, 2.0, 3.0],
            normal: [0.0, 1.0, 0.0],
            uv: [0.25, 0.75],
            layer,
            ao_factor: 0.8,
            sky: 12,
            block: 3,
            shade: 0.9,
        }
    }

    /// One surface of `quads` quads; zero quads is the empty class the
    /// table must skip.
    fn quad_surface(layer: u16, quads: usize) -> SurfaceGeometry {
        SurfaceGeometry {
            vertices: vec![vertex(layer); quads * 4],
            indices: (0..quads)
                .flat_map(|quad| QUAD_INDICES.map(|index| index + (quad as u32) * 4))
                .collect(),
        }
    }

    fn geometry(opaque: usize, cutout: usize, water: usize) -> SectionGeometry {
        SectionGeometry {
            opaque: quad_surface(11, opaque),
            cutout: quad_surface(12, cutout),
            water: quad_surface(28, water),
        }
    }

    fn section(dimension: u32, x: i32, revision: u64) -> SectionId {
        SectionId {
            dimension,
            x,
            y: 3,
            z: 7,
            revision,
        }
    }

    /// The instance origin the table must place one section at.
    fn expected_origin(id: SectionId) -> Transform3D {
        Transform3D::new(
            Basis::IDENTITY,
            Vector3::new(
                (id.x as f32) * (SECTION_EDGE_BLOCKS as f32),
                (id.y as f32) * (SECTION_EDGE_BLOCKS as f32) - 64.0,
                (id.z as f32) * (SECTION_EDGE_BLOCKS as f32),
            ),
        )
    }

    #[test]
    fn terrain_resources_maps_storage_section_y_to_world_height() {
        use super::section_transform;

        for (section_y, world_y) in [(0, -64.0), (4, 0.0), (23, 304.0)] {
            let coord = SectionCoord {
                dimension: 0,
                x: -2,
                y: section_y,
                z: 3,
            };
            assert_eq!(
                section_transform(&coord).origin,
                Vector3::new(-32.0, world_y, 48.0),
                "section Y is a zero-based storage index"
            );
        }
    }

    #[test]
    fn terrain_resources_pins_the_frozen_section_bounds() {
        // The mesher worst case is six quads per block of a 16x16x16
        // section, and one section carries at most the three surface
        // classes — the accounting bounds of this table derive from them.
        assert_eq!(SECTION_EDGE_BLOCKS, 16);
        assert_eq!(abi::MAX_SECTION_MESH_QUADS, 6 * 16 * 16 * 16);
        assert_eq!(SECTION_SURFACE_CLASSES, 3);
    }

    #[test]
    fn terrain_resources_upsert_builds_surfaces_and_instance() {
        let (mut renderer, script) = scripted_renderer();
        let id = section(0, -3, 5);
        assert_eq!(renderer.upsert_section(id, &geometry(2, 1, 1)), Ok(()));

        assert_eq!(
            renderer.facts(),
            RendererFacts {
                sections: 1,
                surfaces: 3
            }
        );
        // The first upsert allocates mesh then instance, submits the
        // non-empty classes in stable order with their material RIDs, and
        // frees nothing.
        assert_eq!(
            script.events(),
            vec![
                ScriptEvent::MeshCreated { mesh: Rid::new(1) },
                ScriptEvent::SurfaceAdded {
                    mesh: Rid::new(1),
                    vertices: 8,
                    material: SCRIPT_OPAQUE
                },
                ScriptEvent::SurfaceAdded {
                    mesh: Rid::new(1),
                    vertices: 4,
                    material: SCRIPT_CUTOUT
                },
                ScriptEvent::SurfaceAdded {
                    mesh: Rid::new(1),
                    vertices: 4,
                    material: SCRIPT_WATER
                },
                ScriptEvent::InstanceCreated {
                    mesh: Rid::new(1),
                    scenario: SCRIPT_SCENARIO,
                    origin: expected_origin(id)
                },
            ]
        );
        assert!(script.free_history().is_empty());
        drop(renderer);
        // Drop is reset: the instance frees before the mesh it draws.
        assert_eq!(script.free_history(), vec![2, 1]);
    }

    #[test]
    fn terrain_resources_upsert_skips_empty_surface_classes() {
        let (mut renderer, script) = scripted_renderer();
        assert_eq!(
            renderer.upsert_section(section(1, 2, 1), &geometry(0, 0, 3)),
            Ok(())
        );
        // Only the water class is non-empty: one surface, one material.
        assert_eq!(
            renderer.facts(),
            RendererFacts {
                sections: 1,
                surfaces: 1
            }
        );
        let surfaces: Vec<ScriptEvent> = script
            .events()
            .into_iter()
            .filter(|event| matches!(event, ScriptEvent::SurfaceAdded { .. }))
            .collect();
        assert_eq!(
            surfaces,
            vec![ScriptEvent::SurfaceAdded {
                mesh: Rid::new(1),
                vertices: 12,
                material: SCRIPT_WATER
            }]
        );
    }

    #[test]
    fn terrain_resources_replacement_swaps_the_whole_section() {
        let (mut renderer, script) = scripted_renderer();
        assert_eq!(
            renderer.upsert_section(section(0, 4, 5), &geometry(2, 0, 0)),
            Ok(())
        );
        let old_mesh = Rid::new(1);
        let old_instance = Rid::new(2);
        assert_eq!(
            renderer.upsert_section(section(0, 4, 6), &geometry(0, 1, 2)),
            Ok(())
        );

        // One live section whose surface accounting follows the new mesh.
        assert_eq!(
            renderer.facts(),
            RendererFacts {
                sections: 1,
                surfaces: 2
            }
        );
        // The old section is freed exactly once, instance before mesh,
        // only after the new section is fully live.
        assert_eq!(script.free_history(), vec![2, 1]);
        assert!(!script.is_alive(old_mesh));
        assert!(!script.is_alive(old_instance));
        assert!(script.is_alive(Rid::new(3)));
        assert!(script.is_alive(Rid::new(4)));
        // The instance registration is the last event of the new build;
        // the old section's frees follow it.
        match script.events().last() {
            Some(ScriptEvent::Freed { rid }) => assert_eq!(*rid, old_mesh),
            other => panic!("expected the old mesh free last, got {other:?}"),
        }
    }

    #[test]
    fn terrain_resources_refuses_stale_equal_and_zero_revisions() {
        let (mut renderer, script) = scripted_renderer();
        let id = section(0, 9, 7);
        assert_eq!(renderer.upsert_section(id, &geometry(1, 1, 1)), Ok(()));
        let events_before = script.events().len();

        // Zero, equal, and older upserts are refused with the input
        // rejection word and consume no backend call and no revision.
        for stale in [0, 7, 3] {
            assert_eq!(
                renderer.upsert_section(
                    SectionId {
                        revision: stale,
                        ..id
                    },
                    &geometry(1, 0, 0)
                ),
                Err(abi::STATUS_INPUT_REJECTED)
            );
        }
        // Stale and zero drops are refused the same way.
        for stale in [0, 7, 2] {
            assert_eq!(
                renderer.drop_section(SectionId {
                    revision: stale,
                    ..id
                }),
                Err(abi::STATUS_INPUT_REJECTED)
            );
        }
        assert_eq!(script.events().len(), events_before);
        assert!(script.free_history().is_empty());
        assert_eq!(
            renderer.facts(),
            RendererFacts {
                sections: 1,
                surfaces: 3
            }
        );

        // A strictly newer revision still replaces the section.
        assert_eq!(
            renderer.upsert_section(SectionId { revision: 8, ..id }, &geometry(1, 0, 0)),
            Ok(())
        );
        assert_eq!(script.free_history().len(), 2);
    }

    #[test]
    fn terrain_resources_failure_mid_surface_preserves_the_old_section() {
        for fail_at in 0..SECTION_SURFACE_CLASSES {
            let (mut renderer, script) = scripted_renderer();
            let first = section(0, 1, 10);
            assert_eq!(renderer.upsert_section(first, &geometry(2, 1, 1)), Ok(()));
            let old_mesh = Rid::new(1);
            let old_instance = Rid::new(2);
            let events_before = script.events().len();

            // Fail the fail_at-th surface submission of the replacement.
            script.fail_surface_in(fail_at);
            let second = section(0, 1, 11);
            assert_eq!(
                renderer.upsert_section(second, &geometry(2, 1, 1)),
                Err(abi::STATUS_INTERNAL),
                "surface {fail_at} must fail the whole upsert"
            );

            // The old section is exactly intact: still live, never freed,
            // still the recorded revision, and the only freed RID is the
            // partially built new mesh.
            assert!(script.is_alive(old_mesh));
            assert!(script.is_alive(old_instance));
            assert_eq!(script.free_history(), vec![3], "failure at {fail_at}");
            assert_eq!(
                renderer.facts(),
                RendererFacts {
                    sections: 1,
                    surfaces: 3
                }
            );
            assert_eq!(
                script.events().len(),
                events_before + 1 + fail_at + 1,
                "mesh creation, {fail_at} surfaces, and the rollback free; the failing surface records no event"
            );

            // After backend recovery the same revision retries cleanly:
            // the failed attempt recorded no revision, so it is not
            // stale, and the swap completes with the old section freed
            // exactly once.
            script.recover();
            assert_eq!(renderer.upsert_section(second, &geometry(1, 1, 1)), Ok(()));
            assert_eq!(script.free_history(), vec![3, 2, 1]);
            assert_eq!(
                renderer.facts(),
                RendererFacts {
                    sections: 1,
                    surfaces: 3
                }
            );
        }
    }

    #[test]
    fn terrain_resources_failure_at_allocation_or_instance_preserves_the_old_section() {
        // Mesh allocation failure: nothing is allocated, nothing freed,
        // the old section untouched.
        let (mut renderer, script) = scripted_renderer();
        assert_eq!(
            renderer.upsert_section(section(0, 2, 4), &geometry(1, 1, 1)),
            Ok(())
        );
        script.fail_mesh_create();
        assert_eq!(
            renderer.upsert_section(section(0, 2, 5), &geometry(1, 0, 0)),
            Err(abi::STATUS_INTERNAL)
        );
        assert!(script.free_history().is_empty());
        assert!(script.is_alive(Rid::new(1)) && script.is_alive(Rid::new(2)));
        assert_eq!(
            renderer.facts(),
            RendererFacts {
                sections: 1,
                surfaces: 3
            }
        );

        // Instance registration failure: the fully surfaced new mesh is
        // freed, the old section untouched.
        script.recover();
        script.fail_instance();
        assert_eq!(
            renderer.upsert_section(section(0, 2, 6), &geometry(1, 0, 0)),
            Err(abi::STATUS_INTERNAL)
        );
        assert_eq!(script.free_history(), vec![3]);
        assert!(script.is_alive(Rid::new(1)) && script.is_alive(Rid::new(2)));
        assert_eq!(
            renderer.facts(),
            RendererFacts {
                sections: 1,
                surfaces: 3
            }
        );

        // Recovery completes the replacement on the same revision.
        script.recover();
        assert_eq!(
            renderer.upsert_section(section(0, 2, 6), &geometry(1, 0, 0)),
            Ok(())
        );
        assert_eq!(script.free_history(), vec![3, 2, 1]);
    }

    #[test]
    fn terrain_resources_drop_frees_once_and_missing_drop_is_a_noop() {
        let (mut renderer, script) = scripted_renderer();
        assert_eq!(
            renderer.upsert_section(section(0, 3, 1), &geometry(1, 0, 1)),
            Ok(())
        );
        assert_eq!(renderer.drop_section(section(0, 3, 2)), Ok(()));

        // The section's RIDs are freed exactly once, instance first.
        assert_eq!(script.free_history(), vec![2, 1]);
        assert_eq!(
            renderer.facts(),
            RendererFacts {
                sections: 0,
                surfaces: 0
            }
        );

        // Dropping a section the table does not hold succeeds without
        // any backend call — the presentation contract allows a
        // batch-valid drop of an absent section.
        let events_before = script.events().len();
        assert_eq!(renderer.drop_section(section(0, 3, 9)), Ok(()));
        assert_eq!(renderer.drop_section(section(4, 8, 1)), Ok(()));
        assert_eq!(script.events().len(), events_before);
    }

    #[test]
    fn terrain_resources_dimensions_key_independently() {
        let (mut renderer, script) = scripted_renderer();
        let overworld = section(0, 6, 1);
        let depths = section(1, 6, 1);
        assert_eq!(
            renderer.upsert_section(overworld, &geometry(1, 0, 0)),
            Ok(())
        );
        assert_eq!(renderer.upsert_section(depths, &geometry(0, 0, 1)), Ok(()));

        // Same coordinates in another dimension are distinct sections.
        assert_eq!(
            renderer.facts(),
            RendererFacts {
                sections: 2,
                surfaces: 2
            }
        );
        assert_eq!(
            renderer.sections_in_dimension(0),
            vec![SectionCoord::of(overworld)]
        );
        assert_eq!(
            renderer.sections_in_dimension(1),
            vec![SectionCoord::of(depths)]
        );
        assert!(renderer.sections_in_dimension(7).is_empty());

        // Dropping one dimension's section leaves the other untouched.
        assert_eq!(renderer.drop_section(section(0, 6, 2)), Ok(()));
        assert_eq!(script.free_history(), vec![2, 1]);
        assert!(script.is_alive(Rid::new(3)));
        assert!(script.is_alive(Rid::new(4)));
        assert_eq!(
            renderer.facts(),
            RendererFacts {
                sections: 1,
                surfaces: 1
            }
        );
    }

    #[test]
    fn terrain_resources_reset_frees_every_section_in_coordinate_order() {
        let (mut renderer, script) = scripted_renderer();
        let first = section(0, 5, 1);
        let second = section(1, -1, 1);
        let third = section(0, -2, 1);
        for id in [first, second, third] {
            assert_eq!(renderer.upsert_section(id, &geometry(1, 0, 0)), Ok(()));
        }
        // Allocation order: first=1,2 second=3,4 third=5,6. The map order
        // is (dimension, x, y, z), so reset frees third, first, second —
        // instance before mesh within each section.
        renderer.reset();
        assert_eq!(script.free_history(), vec![6, 5, 2, 1, 4, 3]);
        assert!(script.live_ids().is_empty());
        assert_eq!(
            renderer.facts(),
            RendererFacts {
                sections: 0,
                surfaces: 0
            }
        );

        // The emptied table accepts fresh upserts at fresh revisions.
        assert_eq!(
            renderer.upsert_section(
                SectionId {
                    revision: 1,
                    ..first
                },
                &geometry(1, 0, 0)
            ),
            Ok(())
        );
        assert_eq!(
            renderer.facts(),
            RendererFacts {
                sections: 1,
                surfaces: 1
            }
        );
    }

    #[test]
    fn terrain_resources_rejects_degenerate_geometry_without_backend_calls() {
        let (mut renderer, script) = scripted_renderer();
        // No non-empty surface class at all: removing a section is a drop.
        assert_eq!(
            renderer.upsert_section(section(0, 1, 1), &geometry(0, 0, 0)),
            Err(abi::STATUS_INPUT_REJECTED)
        );
        // A surface without indices cannot be drawn.
        let mut unindexed = geometry(1, 0, 0);
        unindexed.opaque.indices.clear();
        assert_eq!(
            renderer.upsert_section(section(0, 1, 1), &unindexed),
            Err(abi::STATUS_INPUT_REJECTED)
        );
        // A surface above the frozen per-section vertex bound.
        let oversized = SectionGeometry {
            opaque: SurfaceGeometry {
                vertices: vec![vertex(11); abi::MAX_SECTION_MESH_QUADS as usize * 4 + 1],
                indices: Vec::new(),
            },
            cutout: SurfaceGeometry::default(),
            water: SurfaceGeometry::default(),
        };
        assert_eq!(
            renderer.upsert_section(section(0, 1, 1), &oversized),
            Err(abi::STATUS_INPUT_REJECTED)
        );
        assert!(script.events().is_empty());
        assert_eq!(
            renderer.facts(),
            RendererFacts {
                sections: 0,
                surfaces: 0
            }
        );
    }

    #[test]
    fn terrain_resources_rid_allocation_is_unique_and_monotonic() {
        let (mut renderer, script) = scripted_renderer();
        let ids: Vec<SectionId> = (0..3).map(|index| section(0, index, 1)).collect();
        for id in &ids {
            assert_eq!(renderer.upsert_section(*id, &geometry(1, 1, 1)), Ok(()));
        }
        assert_eq!(renderer.drop_section(section(0, 0, 2)), Ok(()));
        renderer.reset();
        assert_eq!(
            renderer.upsert_section(
                SectionId {
                    revision: 1,
                    ..ids[0]
                },
                &geometry(1, 0, 0)
            ),
            Ok(())
        );

        // Every allocated id is distinct and strictly increasing, so no
        // freed RID is ever recycled into a live resource.
        let allocated = script.allocated();
        // Three sections, then the fresh upsert after reset: eight
        // mesh-plus-instance allocations in total.
        assert_eq!(allocated.len(), 8);
        assert!(allocated.windows(2).all(|pair| pair[0] < pair[1]));
        // The fresh section is live right now, and teardown frees it.
        assert_eq!(script.live_ids(), vec![7, 8]);
        drop(renderer);
        assert!(script.live_ids().is_empty());
    }

    #[test]
    fn terrain_resources_drop_without_reset_still_frees_everything() {
        let (mut renderer, script) = scripted_renderer();
        assert_eq!(
            renderer.upsert_section(section(0, 0, 1), &geometry(1, 1, 1)),
            Ok(())
        );
        assert_eq!(
            renderer.upsert_section(section(1, 0, 1), &geometry(0, 1, 0)),
            Ok(())
        );
        drop(renderer);
        // Drop is reset: every table-owned RID was freed exactly once and
        // nothing leaks into the scripted engine; sections free in
        // coordinate order (overworld before depths), instance before
        // mesh within each.
        assert_eq!(script.free_history(), vec![2, 1, 4, 3]);
        assert!(script.live_ids().is_empty());
    }
}
