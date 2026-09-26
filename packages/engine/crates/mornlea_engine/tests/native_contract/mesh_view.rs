//! Typed mesh view contract: registry validation and the typed light entry.
//!
//! Only the public native surface is named here: the validated registry
//! constructor, the borrowed section view and the typed light build. Level and
//! queue internals stay crate-private contract state and are asserted by the
//! `light` topic unit tests instead.

use mornlea_engine::native::contracts::{
    KernelError, MeshModel, MeshRegistry, MeshRegistryEntry, MeshScratch, MeshView,
};
use mornlea_engine::native::mesh::{build_light, try_new_registry};

const AIR: u16 = 0;
const STONE: u16 = 1;
const LAMP: u16 = 2;
/// Block ids absent from every fixture registry are valid world data; the
/// typed lane must accept them rather than report a registry error.
const UNKNOWN: u16 = 60_000;
const BLOCKS: usize = 27 * 4096;

/// One registry entry in the frozen typed shape; `STONE` is the only opaque id
/// and every fixture entry starts at the neutral defaults.
fn entry(id: u16) -> MeshRegistryEntry {
    MeshRegistryEntry {
        id,
        opaque: id == STONE,
        emission: 0,
        material: [0; 6],
        fluid_height: 0,
        light_attenuation: 0,
        block_top_raw: 0,
        model: MeshModel::Default,
    }
}

/// The exact visibility word count rule: `R * ceil(R / 64)` all-zero words.
fn visibility(count: usize) -> Vec<u64> {
    vec![0_u64; count * count.div_ceil(64)]
}

/// Typed neighborhood cell index for a coordinate in `-16..=31`.
fn cell(x: i32, y: i32, z: i32) -> usize {
    let shifted = |value: i32| value + 16;
    let section = (shifted(x) >> 4) * 3 + (shifted(y) >> 4);
    let section = (section * 3 + (shifted(z) >> 4)) as usize;
    let offset = ((shifted(y) & 15) << 8) | ((shifted(z) & 15) << 4) | (shifted(x) & 15);
    section * 4096 + offset as usize
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
            blocks: Box::new([0; BLOCKS]),
            heights_present: Box::new([false; 9]),
            heights: Box::new([[0; 256]; 9]),
        }
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

#[test]
fn registry_accepts_96_entries_and_rejects_out_of_range_counts() {
    let entries: Vec<MeshRegistryEntry> = (0..96).map(entry).collect();
    let words = visibility(96);
    assert_eq!(words.len(), 192, "96 entries need R*ceil(R/64) words");
    assert!(try_new_registry(&entries, &words, AIR, STONE).is_ok());
    assert!(try_new_registry(&entries[..95], &visibility(95), AIR, STONE).is_ok());

    let over: Vec<MeshRegistryEntry> = (0..97).map(entry).collect();
    assert_eq!(
        try_new_registry(&over, &visibility(97), AIR, STONE).unwrap_err(),
        KernelError::InvalidRegistry,
    );

    assert_eq!(
        try_new_registry(&[], &[], AIR, STONE).unwrap_err(),
        KernelError::InvalidRegistry,
    );

    // A single entry cannot carry the two distinct sentinels the neighbor
    // lookups fall back to, so even a structurally shaped one-entry table is
    // refused by the semantic half of the constructor.
    assert_eq!(
        try_new_registry(&[entry(AIR)], &visibility(1), AIR, STONE).unwrap_err(),
        KernelError::InvalidRegistry,
    );

    // Exactly one visibility word per 64 ids, no slack in either direction.
    assert_eq!(
        try_new_registry(&entries, &words[..191], AIR, STONE).unwrap_err(),
        KernelError::InvalidRegistry,
    );
    let mut extra = words.clone();
    extra.push(0);
    assert_eq!(
        try_new_registry(&entries, &extra, AIR, STONE).unwrap_err(),
        KernelError::InvalidRegistry,
    );
}

#[test]
fn registry_rejects_unsorted_duplicate_and_sentinel_conflicts() {
    assert_eq!(
        try_new_registry(&[entry(STONE), entry(AIR)], &visibility(2), AIR, STONE).unwrap_err(),
        KernelError::InvalidRegistry,
    );
    assert_eq!(
        try_new_registry(&[entry(AIR), entry(AIR)], &visibility(2), AIR, STONE).unwrap_err(),
        KernelError::InvalidRegistry,
    );
    assert_eq!(
        try_new_registry(&[entry(AIR), entry(STONE)], &visibility(2), AIR, AIR).unwrap_err(),
        KernelError::InvalidRegistry,
    );
    // Barrier sentinel absent.
    assert_eq!(
        try_new_registry(&[entry(AIR), entry(LAMP)], &visibility(2), AIR, STONE).unwrap_err(),
        KernelError::InvalidRegistry,
    );
    // Air sentinel absent.
    assert_eq!(
        try_new_registry(&[entry(STONE), entry(LAMP)], &visibility(2), AIR, STONE).unwrap_err(),
        KernelError::InvalidRegistry,
    );
}

#[test]
fn registry_entry_ranges_match_the_byte_lane() {
    let ok = |second: MeshRegistryEntry| {
        try_new_registry(&[entry(AIR), second], &visibility(2), AIR, STONE)
    };

    let mut fluid = entry(STONE);
    fluid.fluid_height = 1;
    assert!(
        ok(fluid).is_ok(),
        "fluid height 1 is a legal isolated level"
    );

    let mut over_fluid = entry(STONE);
    over_fluid.fluid_height = 15;
    assert_eq!(
        ok(over_fluid).unwrap_err(),
        KernelError::InvalidRegistry,
        "fluid height 15 is reserved for the mesher's full-cell case",
    );

    let mut short = entry(STONE);
    short.block_top_raw = 14;
    assert!(ok(short).is_ok());
    let mut over_top = entry(STONE);
    over_top.block_top_raw = 15;
    assert_eq!(ok(over_top).unwrap_err(), KernelError::InvalidRegistry);

    let mut mutex = entry(STONE);
    mutex.fluid_height = 1;
    mutex.block_top_raw = 1;
    assert_eq!(
        ok(mutex).unwrap_err(),
        KernelError::InvalidRegistry,
        "fluid and short-block geometry must not share one entry",
    );

    let mut bed = entry(STONE);
    bed.model = MeshModel::Bed;
    assert!(ok(bed).is_ok(), "model tag 6 is the bed form");
    // The rejected tag 7 cannot even be named here: `MeshModel` is the closed
    // typed set, which is a stronger guarantee than the byte lane's range
    // check (covered by the raw producer and the input unit tests).

    let mut bright = entry(STONE);
    bright.emission = 15;
    assert!(ok(bright).is_ok());
    let mut over_bright = entry(STONE);
    over_bright.emission = 16;
    assert_eq!(
        ok(over_bright).unwrap_err(),
        KernelError::EmissionOutOfRange
    );

    let mut attenuating = entry(STONE);
    attenuating.light_attenuation = 1;
    assert!(ok(attenuating).is_ok());
    let mut over_attenuating = entry(STONE);
    over_attenuating.light_attenuation = 2;
    assert_eq!(
        ok(over_attenuating).unwrap_err(),
        KernelError::InvalidRegistry,
        "the build_sky bucket proof only holds for attenuation 0 or 1",
    );
}

#[test]
fn typed_light_validates_semantics_even_for_an_all_air_section() {
    // The frozen structural constructor does not check light attenuation; the
    // typed native entry must still refuse the view, all-air or not.
    let mut bad = entry(STONE);
    bad.light_attenuation = 2;
    let registry = MeshRegistry::try_new(&[entry(AIR), bad], &visibility(2), AIR, STONE).unwrap();
    let section = Section::air();
    let view = section.view(&registry);
    let mut scratch = MeshScratch::try_new().unwrap();

    assert_eq!(
        build_light(&view, &mut scratch).unwrap_err(),
        KernelError::InvalidRegistry,
    );
}

#[test]
fn typed_light_accepts_valid_sections_and_unknown_block_ids() {
    let mut lamp = entry(LAMP);
    lamp.emission = 15;
    let registry = try_new_registry(
        &[entry(AIR), entry(STONE), lamp],
        &visibility(3),
        AIR,
        STONE,
    )
    .unwrap();

    let mut bright = Section::air();
    bright.blocks[cell(8, 8, 8)] = LAMP;
    bright.blocks[cell(9, 8, 8)] = UNKNOWN;
    let bright = bright.view(&registry);

    let mut scratch = MeshScratch::try_new().unwrap();
    assert_eq!(build_light(&bright, &mut scratch), Ok(()));

    // Reusing the scratch for a dark section must be accepted too; the typed
    // entry resets the complete level volume and queue before each build.
    let dark = Section::air();
    let dark = dark.view(&registry);
    assert_eq!(build_light(&dark, &mut scratch), Ok(()));
}
