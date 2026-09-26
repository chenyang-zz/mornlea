//! Typed mesh geometry contract: staged publication and destination atomicity.
//!
//! Only the public native surface is named here: the validated registry
//! constructor, the borrowed section view, the reusable scratch and the
//! `MeshOp` geometry entry. The packed bit layout is asserted independently
//! against the documented ABI word, and the exact ordered fields are compared
//! with the Go oracle in the `numerical_migration` mesh topic.

use mornlea_engine::native::contracts::{
    KernelError, MeshModel, MeshOp, MeshQuad, MeshRegistry, MeshRegistryEntry, MeshScratch,
    MeshView,
};
use mornlea_engine::native::mesh::{NativeMesh, try_new_registry};

const AIR: u16 = 0;
const BARRIER: u16 = 1;
const STONE: u16 = 2;
const WHEAT: u16 = 20;
const STANDING_TORCH: u16 = 71;
const WALL_TORCH_POS_X: u16 = 72;
const BED: u16 = 76;

/// First layer of the plant material interval: the mesher identifies plants by
/// the material range, and a standing torch carrying this layer is the valid
/// model-plus-plant combination.
const WHEAT_MATERIAL: u16 = 31;
/// Torch material outside the plant interval, so a torch alone stays on the
/// model dispatcher path.
const TORCH_MATERIAL: u16 = 90;
/// Bed materials: the top face carries the per-form bed layer, the four sides
/// the shared frame layer.
const BED_TOP_MATERIAL: u16 = 700;
const BED_SIDE_MATERIAL: u16 = 91;
/// Wall-torch top-edge raw heights of the support and the far side.
const TORCH_WALL_TOP_NEAR_RAW: u8 = 8;
const TORCH_WALL_TOP_FAR_RAW: u8 = 13;
/// Bed top-height raw value, the shared 9/16 slab contract.
const BED_TOP_RAW: u8 = 8;

const BLOCKS: usize = 27 * 4096;
/// Fixed staging capacity of the caller-owned scratch: the conservative
/// geometry bound. A dense accepted registry stays inside it, and a
/// destination shorter than the actual count is reported, never truncated.
const STAGE_QUADS: usize = 40960;

/// One registry entry in the frozen typed shape with the neutral defaults the
/// geometry scenes start from.
fn entry(id: u16, opaque: bool, material: [u16; 6], model: MeshModel) -> MeshRegistryEntry {
    MeshRegistryEntry {
        id,
        opaque,
        emission: 0,
        material,
        fluid_height: 0,
        light_attenuation: 0,
        block_top_raw: 0,
        model,
    }
}

/// Visibility words of a typed table: one bit per ordered (row, column) id
/// pair, addressed by entry index exactly like the byte lane's bitmap.
fn visibility(count: usize, visible: &[(usize, usize)]) -> Vec<u64> {
    let words_per_row = count.div_ceil(64);
    let mut words = vec![0_u64; count * words_per_row];
    for &(row, column) in visible {
        words[row * words_per_row + column / 64] |= 1_u64 << (column % 64);
    }
    words
}

/// Builds a fully validated registry from entries and visible id pairs.
fn registry(entries: &[MeshRegistryEntry], visible: &[(usize, usize)]) -> MeshRegistry {
    try_new_registry(entries, &visibility(entries.len(), visible), AIR, BARRIER).unwrap()
}

/// Owned section fixture backing one borrowed [`MeshView`].
struct Section {
    blocks: Box<[u16; BLOCKS]>,
    heights_present: Box<[bool; 9]>,
    heights: Box<[[i16; 256]; 9]>,
}

impl Section {
    fn air() -> Self {
        Self {
            blocks: Box::new([AIR; BLOCKS]),
            heights_present: Box::new([false; 9]),
            heights: Box::new([[0; 256]; 9]),
        }
    }

    fn set(&mut self, x: i32, y: i32, z: i32, id: u16) {
        self.blocks[cell(x, y, z)] = id;
    }

    fn fill(&mut self, id: u16) {
        self.blocks.fill(id);
    }

    fn view<'a>(&'a self, registry: &'a MeshRegistry) -> MeshView<'a> {
        MeshView {
            blocks: &self.blocks,
            heights_present: &self.heights_present,
            heights: &self.heights,
            section_origin_y: 0,
            registry,
        }
    }
}

/// Typed neighborhood cell index for a coordinate in `-16..=31`.
fn cell(x: i32, y: i32, z: i32) -> usize {
    let shifted = |value: i32| value + 16;
    let section = (shifted(x) >> 4) * 3 + (shifted(y) >> 4);
    let section = (section * 3 + (shifted(z) >> 4)) as usize;
    let offset = ((shifted(y) & 15) << 8) | ((shifted(z) & 15) << 4) | (shifted(x) & 15);
    section * 4096 + offset as usize
}

/// Registry with an isolated opaque stone whose six faces are visible toward
/// air and whose per-face materials follow the face order.
fn faces_registry() -> MeshRegistry {
    registry(
        &[
            entry(AIR, false, [0; 6], MeshModel::Default),
            entry(BARRIER, true, [1; 6], MeshModel::Default),
            entry(STONE, true, [10, 11, 12, 13, 14, 15], MeshModel::Default),
        ],
        &[(2, 0)],
    )
}

/// Emits one isolated six-face stone at a cell far from every failing fixture,
/// so a partial publication of those fixtures always differs from the canary.
fn canary_quads() -> [MeshQuad; 6] {
    let registry = faces_registry();
    let mut section = Section::air();
    section.set(3, 4, 5, STONE);
    let view = section.view(&registry);
    let mut scratch = MeshScratch::try_new().unwrap();
    let mut dst = [MeshQuad::default(); 6];
    let count = NativeMesh.mesh(&view, &mut scratch, &mut dst).unwrap();
    assert_eq!(count, 6, "the canary fixture must publish its six faces");
    dst
}

/// Compact ordered quad fields: position, face, back flag, material and
/// corner heights.
type QuadFields = (u8, u8, u8, u8, bool, u16, [u8; 4]);

/// Compact ordered tuple of one staged quad.
fn fields(quad: &MeshQuad) -> QuadFields {
    (
        quad.x(),
        quad.y(),
        quad.z(),
        quad.face(),
        quad.back(),
        quad.material(),
        quad.corners(),
    )
}

#[test]
fn six_isolated_axial_faces_publish_in_face_order() {
    let registry = faces_registry();
    let mut section = Section::air();
    section.set(8, 8, 8, STONE);
    let view = section.view(&registry);
    let mut scratch = MeshScratch::try_new().unwrap();
    let mut dst = [MeshQuad::default(); 6];

    let count = NativeMesh.mesh(&view, &mut scratch, &mut dst).unwrap();
    assert_eq!(
        count, 6,
        "one isolated block must publish exactly six faces"
    );

    for (index, quad) in dst.iter().enumerate() {
        assert_eq!(
            fields(quad),
            (8, 8, 8, index as u8, false, 10 + index as u16, [0; 4]),
            "axial quad {index} must follow the face order with its own material",
        );
        assert_eq!((quad.w(), quad.h()), (1, 1), "axial quad spans the cell");
        assert_eq!(quad.ao(), 0xff, "an isolated block has no occluders");
        assert_eq!(quad.light(), 0, "the dark fixture carries no light");
    }

    // Bit equivalence with the documented 8-byte ABI layout, computed here
    // field by field instead of trusting the packed representation.
    let want = 8_u64 | 8_u64 << 4 | 8_u64 << 8 | 3_u64 << 20 | 13_u64 << 23 | 0xff_u64 << 39;
    assert_eq!(
        dst[3].packed(),
        want,
        "packed PosY word must match the layout"
    );
}

#[test]
fn plant_torch_and_bed_quads_keep_the_documented_order() {
    let registry = registry(
        &[
            entry(AIR, false, [0; 6], MeshModel::Default),
            entry(BARRIER, true, [1; 6], MeshModel::Default),
            entry(WHEAT, false, [WHEAT_MATERIAL; 6], MeshModel::Default),
            entry(
                STANDING_TORCH,
                false,
                [TORCH_MATERIAL; 6],
                MeshModel::StandingTorch,
            ),
            entry(
                WALL_TORCH_POS_X,
                false,
                [TORCH_MATERIAL; 6],
                MeshModel::WallTorchPosX,
            ),
            entry(
                BED,
                false,
                [
                    BED_SIDE_MATERIAL,
                    BED_SIDE_MATERIAL,
                    BED_SIDE_MATERIAL,
                    BED_TOP_MATERIAL,
                    BED_SIDE_MATERIAL,
                    BED_SIDE_MATERIAL,
                ],
                MeshModel::Bed,
            ),
        ],
        &[],
    );
    let mut section = Section::air();
    section.set(1, 8, 8, WHEAT);
    section.set(2, 8, 8, STANDING_TORCH);
    section.set(3, 8, 8, WALL_TORCH_POS_X);
    section.set(4, 8, 8, BED);
    let view = section.view(&registry);
    let mut scratch = MeshScratch::try_new().unwrap();
    let mut dst = [MeshQuad::default(); 16];

    let count = NativeMesh.mesh(&view, &mut scratch, &mut dst).unwrap();
    assert_eq!(count, 16, "4 plant + 4 torch + 3 wall torch + 5 bed quads");

    let near = TORCH_WALL_TOP_NEAR_RAW;
    let far = TORCH_WALL_TOP_FAR_RAW;
    let cross = |x: u8, material: u16| {
        [
            (x, 8, 8, 6, false, material, [0; 4]),
            (x, 8, 8, 6, true, material, [0; 4]),
            (x, 8, 8, 7, false, material, [0; 4]),
            (x, 8, 8, 7, true, material, [0; 4]),
        ]
    };
    let bed = BED_TOP_RAW;
    let mut want: Vec<QuadFields> = Vec::new();
    want.extend(cross(1, WHEAT_MATERIAL));
    want.extend(cross(2, TORCH_MATERIAL));
    want.extend([
        (3, 8, 8, 4, false, TORCH_MATERIAL, [0, 0, far, near]),
        (3, 8, 8, 5, false, TORCH_MATERIAL, [0, 0, far, near]),
        (3, 8, 8, 0, false, TORCH_MATERIAL, [0; 4]),
        (4, 8, 8, 0, false, BED_SIDE_MATERIAL, [0, bed, bed, 0]),
        (4, 8, 8, 1, false, BED_SIDE_MATERIAL, [0, bed, bed, 0]),
        (4, 8, 8, 4, false, BED_SIDE_MATERIAL, [0, 0, bed, bed]),
        (4, 8, 8, 5, false, BED_SIDE_MATERIAL, [0, 0, bed, bed]),
        (4, 8, 8, 3, false, BED_TOP_MATERIAL, [bed; 4]),
    ]);

    for (index, quad) in dst[..count].iter().enumerate() {
        assert_eq!(fields(quad), want[index], "quad {index} order/routing");
        assert_eq!(quad.ao(), 0xff, "grid-local geometry has no occluders");
        assert_eq!(quad.light(), 0, "the dark fixture carries no light");
    }
}

#[test]
fn standing_torch_with_plant_material_exceeds_six_quads_per_cell() {
    let registry = registry(
        &[
            entry(AIR, false, [0; 6], MeshModel::Default),
            entry(BARRIER, true, [1; 6], MeshModel::Default),
            entry(
                STANDING_TORCH,
                false,
                [WHEAT_MATERIAL; 6],
                MeshModel::StandingTorch,
            ),
        ],
        &[],
    );
    let mut section = Section::air();
    section.set(8, 8, 8, STANDING_TORCH);
    let view = section.view(&registry);
    let mut scratch = MeshScratch::try_new().unwrap();
    let mut dst = [MeshQuad::default(); 8];

    let count = NativeMesh.mesh(&view, &mut scratch, &mut dst).unwrap();
    assert_eq!(
        count, 8,
        "the plant pass and the model dispatcher each publish four quads"
    );
    for (index, quad) in dst.iter().enumerate() {
        assert_eq!((quad.x(), quad.y(), quad.z()), (8, 8, 8), "quad {index}");
        assert!(
            quad.face() == 6 || quad.face() == 7,
            "quad {index} must be a cross quad, got face {}",
            quad.face()
        );
        assert_eq!(quad.material(), WHEAT_MATERIAL, "quad {index}");
        assert_eq!((quad.w(), quad.h()), (1, 1), "quad {index}");
        assert_eq!(quad.corners(), [0; 4], "quad {index}");
    }
    // Plant pairs and model pairs alternate in the same documented pattern.
    let pattern: Vec<(u8, bool)> = dst.iter().map(|quad| (quad.face(), quad.back())).collect();
    assert_eq!(
        pattern,
        [
            (6, false),
            (6, true),
            (7, false),
            (7, true),
            (6, false),
            (6, true),
            (7, false),
            (7, true)
        ]
    );
}

#[test]
fn dense_model_plant_section_fills_the_stage_without_truncation() {
    let registry = registry(
        &[
            entry(AIR, false, [0; 6], MeshModel::Default),
            entry(BARRIER, true, [1; 6], MeshModel::Default),
            entry(
                STANDING_TORCH,
                false,
                [WHEAT_MATERIAL; 6],
                MeshModel::StandingTorch,
            ),
        ],
        &[],
    );
    let mut section = Section::air();
    section.fill(STANDING_TORCH);
    let view = section.view(&registry);
    let mut scratch = MeshScratch::try_new().unwrap();
    let mut dst = vec![MeshQuad::default(); STAGE_QUADS];

    let count = NativeMesh.mesh(&view, &mut scratch, &mut dst).unwrap();
    assert_eq!(
        count,
        8 * 4096,
        "the dense accepted combination exceeds the production-consistent bound"
    );
    assert!(
        count < STAGE_QUADS,
        "the conservative stage bound must cover the densest accepted registry"
    );
    let first = dst[0];
    assert_eq!(
        (first.x(), first.y(), first.z(), first.face(), first.back()),
        (0, 0, 0, 6, false),
        "the dense stream starts at the (0,0,0) plant pair"
    );
    let last = dst[count - 1];
    assert_eq!(
        (last.x(), last.y(), last.z(), last.face(), last.back()),
        (15, 15, 15, 7, true),
        "the dense stream ends with the (15,15,15) model pair"
    );
    assert_eq!(
        dst[count],
        MeshQuad::default(),
        "slots past the published prefix must stay untouched"
    );
}

#[test]
fn a_late_packing_violation_leaves_the_destination_untouched() {
    // The first axial quad (NegX) uses the legal material 10; the second
    // (PosX) uses a plant layer on an axial face, which the packed encoding
    // forbids. The staged lane must reject the call as an invariant violation
    // instead of panicking or publishing the valid prefix.
    let registry = registry(
        &[
            entry(AIR, false, [0; 6], MeshModel::Default),
            entry(BARRIER, true, [1; 6], MeshModel::Default),
            entry(
                STONE,
                true,
                [10, WHEAT_MATERIAL, 12, 13, 14, 15],
                MeshModel::Default,
            ),
        ],
        &[(2, 0)],
    );
    let mut section = Section::air();
    section.set(8, 8, 8, STONE);
    let view = section.view(&registry);
    let mut scratch = MeshScratch::try_new().unwrap();
    let canary = canary_quads();
    let mut dst = [MeshQuad::default(); 6];
    dst.copy_from_slice(&canary);

    assert_eq!(
        NativeMesh.mesh(&view, &mut scratch, &mut dst),
        Err(KernelError::OutputInvariant)
    );
    assert_eq!(
        dst, canary,
        "no destination slot may change on a late error"
    );
}

#[test]
fn a_short_destination_reports_the_exact_needed_count() {
    let registry = faces_registry();
    let mut section = Section::air();
    section.set(8, 8, 8, STONE);
    let view = section.view(&registry);
    let mut scratch = MeshScratch::try_new().unwrap();
    let canary = canary_quads();
    let mut short = [MeshQuad::default(); 5];
    short.copy_from_slice(&canary[..5]);

    assert_eq!(
        NativeMesh.mesh(&view, &mut scratch, &mut short),
        Err(KernelError::OutputTooSmall {
            needed: 6,
            available: 5,
        })
    );
    assert_eq!(
        short.as_slice(),
        &canary[..5],
        "a short destination must stay untouched"
    );

    // The exact capacity of the same call succeeds and publishes the stream.
    let mut exact = [MeshQuad::default(); 6];
    assert_eq!(NativeMesh.mesh(&view, &mut scratch, &mut exact), Ok(6));
    assert!(
        exact
            .iter()
            .all(|quad| (quad.x(), quad.y(), quad.z()) == (8, 8, 8)),
        "the exact-capacity retry must publish the six faces of the fixture"
    );
}

#[test]
fn registry_validation_precedes_geometry_publication() {
    // Structurally valid but semantically over-attenuating: the typed entry
    // must refuse the view before any geometry reaches the destination.
    let mut bad = entry(STONE, true, [10; 6], MeshModel::Default);
    bad.light_attenuation = 2;
    let registry = MeshRegistry::try_new(
        &[
            entry(AIR, false, [0; 6], MeshModel::Default),
            entry(BARRIER, true, [1; 6], MeshModel::Default),
            bad,
        ],
        &visibility(3, &[]),
        AIR,
        BARRIER,
    )
    .unwrap();
    let mut section = Section::air();
    section.set(8, 8, 8, STONE);
    let view = section.view(&registry);
    let mut scratch = MeshScratch::try_new().unwrap();
    let canary = canary_quads();
    let mut dst = [MeshQuad::default(); 6];
    dst.copy_from_slice(&canary);

    assert_eq!(
        NativeMesh.mesh(&view, &mut scratch, &mut dst),
        Err(KernelError::InvalidRegistry)
    );
    assert_eq!(dst, canary, "a rejected registry must not publish geometry");
}
