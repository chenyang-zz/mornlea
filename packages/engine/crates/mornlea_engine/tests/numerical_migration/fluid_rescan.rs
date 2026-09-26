use mornlea_engine::native::contracts::KernelError;
use mornlea_engine::native::contracts::fluid::{
    FluidRescanOp, RescanRange, RescanRequest, RescanScratch, RescanSection, RescanSummary,
    RescanView,
};
use mornlea_engine::native::fluid_rescan::NativeFluidRescan;

/// Stable block ids of the fluid rule table, matching `internal/core/block.go`.
const AIR: u16 = 0;
const STONE: u16 = 2;
const WATER_SOURCE: u16 = 27;

const SECTIONS: usize = 24;
const SECTION_EDGE: usize = 16;
const WORLD_HEIGHT: usize = 384;
const SKIRT_COLUMNS: usize = 68;
const METADATA_CELLS: usize = 216;

/// Test-local mirror of the kernel's frozen skirt column order: four sides in
/// (x=-1, z=0..15), (x=16, z=0..15), (z=-1, x=0..15), (z=16, x=0..15) main
/// order, then the four corners.
fn skirt_column(bx: i32, bz: i32) -> usize {
    match (bx, bz) {
        (0, 1..=16) => (bz - 1) as usize,
        (17, 1..=16) => SECTION_EDGE + (bz - 1) as usize,
        (1..=16, 0) => 2 * SECTION_EDGE + (bx - 1) as usize,
        (1..=16, 17) => 3 * SECTION_EDGE + (bx - 1) as usize,
        (0, 0) => 4 * SECTION_EDGE,
        (17, 0) => 4 * SECTION_EDGE + 1,
        (0, 17) => 4 * SECTION_EDGE + 2,
        (17, 17) => 4 * SECTION_EDGE + 3,
        _ => panic!("skirt column must lie in the box"),
    }
}

/// Test-local mirror of the metadata table's chunk order: center first, then
/// the eight neighbours in row-major (-1..=1) order.
fn meta_slot(dx: i32, dz: i32) -> usize {
    match (dx, dz) {
        (0, 0) => 0,
        (-1, -1) => 1,
        (0, -1) => 2,
        (1, -1) => 3,
        (-1, 0) => 4,
        (1, 0) => 5,
        (-1, 1) => 6,
        (0, 1) => 7,
        (1, 1) => 8,
        _ => panic!("metadata slot must lie in the 3x3 neighborhood"),
    }
}

/// Owned fixture data for one typed rescan view: uniform flags plus dense cell
/// storage per section, the full skirt halo and the 3x3 metadata table.
struct RescanBox {
    center: [i32; 2],
    uniform: [Option<u16>; SECTIONS],
    dense: Box<[[u16; SECTION_EDGE * SECTION_EDGE * SECTION_EDGE]; SECTIONS]>,
    skirt: Box<[[u16; WORLD_HEIGHT]; SKIRT_COLUMNS]>,
    metadata: Box<[Option<u16>; METADATA_CELLS]>,
}

impl RescanBox {
    /// All-stone box: 24 uniform stone sections, stone skirt, stone metadata.
    fn solid(center: [i32; 2]) -> Self {
        Self {
            center,
            uniform: [Some(STONE); SECTIONS],
            dense: Box::new([[STONE; SECTION_EDGE * SECTION_EDGE * SECTION_EDGE]; SECTIONS]),
            skirt: Box::new([[STONE; WORLD_HEIGHT]; SKIRT_COLUMNS]),
            metadata: Box::new([Some(STONE); METADATA_CELLS]),
        }
    }

    fn uniform_section(&mut self, section: usize, id: u16) {
        self.uniform[section] = Some(id);
    }

    fn dense_section(&mut self, section: usize) {
        self.uniform[section] = None;
        self.dense[section] = [STONE; SECTION_EDGE * SECTION_EDGE * SECTION_EDGE];
    }

    fn set_cell(&mut self, section: usize, lx: usize, y16: usize, lz: usize, id: u16) {
        self.dense[section][lx + lz * SECTION_EDGE + y16 * SECTION_EDGE * SECTION_EDGE] = id;
    }

    fn set_skirt(&mut self, bx: i32, y: usize, bz: i32, id: u16) {
        self.skirt[skirt_column(bx, bz)][y] = id;
    }

    fn set_meta(&mut self, dx: i32, dz: i32, section: usize, value: Option<u16>) {
        self.metadata[meta_slot(dx, dz) * SECTIONS + section] = value;
    }

    fn view(&self) -> RescanView<'_> {
        RescanView::try_new(
            std::array::from_fn(|section| match self.uniform[section] {
                Some(id) => RescanSection::Uniform(id),
                None => RescanSection::Dense(&self.dense[section]),
            }),
            &self.skirt,
            &self.metadata,
            self.center,
        )
        .expect("fixture centers must be representable")
    }

    /// Runs one request and returns the summary plus the published positions.
    fn run(
        &self,
        range: RescanRange,
        start_section: u8,
        budget: u32,
        scratch: &mut RescanScratch,
        dst: &mut [[i32; 3]],
    ) -> Result<(RescanSummary, Vec<[i32; 3]>), KernelError> {
        let summary = NativeFluidRescan.rescan(
            &RescanRequest {
                view: self.view(),
                range,
                start_section,
                budget,
            },
            scratch,
            dst,
        )?;
        Ok((summary, dst[..summary.written].to_vec()))
    }
}

fn full_range() -> RescanRange {
    RescanRange {
        x0: 1,
        x1: 16,
        z0: 1,
        z1: 16,
    }
}

fn scratch(capacity: usize) -> RescanScratch {
    RescanScratch::try_with_capacity(capacity).expect("capacity must fit the native bound")
}

/// Worst-case scratch for the full-range matrix: every cell of every remaining
/// section may be emitted once.
fn matrix_scratch() -> RescanScratch {
    scratch(256 * SECTION_EDGE * SECTIONS)
}

/// One observed scenario: name, summary and ordered world positions.
type Observation = (&'static str, RescanSummary, Vec<[i32; 3]>);

/// The shared observation matrix the Go runtime oracle records through the
/// release ABI in `TestFluidRescan`: a uniform nonfluid section, a sealed
/// source, an unsealed source at a halo edge, a dense mixed section, and the
/// budget-zero and budget-one boundaries. The expected world positions and
/// summaries are the frozen Go/ABI observations; the typed lane must publish
/// exactly these, plus the resume cursor the typed summary carries.
fn run_matrix() -> Vec<Observation> {
    let mut out = Vec::new();

    let uniform = RescanBox::solid([0, 0]);
    out.push((
        "uniform nonfluid",
        uniform
            .run(
                full_range(),
                0,
                100_000,
                &mut matrix_scratch(),
                &mut [[0; 3]; 1],
            )
            .expect("uniform box must scan"),
    ));

    // A world-bottom source is sealed by the barrier below and emits nothing.
    let mut sealed = RescanBox::solid([0, 0]);
    sealed.dense_section(0);
    sealed.set_cell(0, 2, 0, 2, WATER_SOURCE);
    out.push((
        "sealed source",
        sealed
            .run(
                full_range(),
                0,
                100_000,
                &mut matrix_scratch(),
                &mut [[0; 3]; 1],
            )
            .expect("sealed box must scan"),
    ));

    // Section 3 is a uniform source section whose section-level fixed point is
    // broken by one air neighbour, so it is scanned per cell. One skirt column
    // is air, so the sixteen edge sources on that column are unsealed and
    // emit, in y16 order, through the halo.
    let mut edge = RescanBox::solid([0, 0]);
    edge.uniform_section(3, WATER_SOURCE);
    edge.set_meta(0, 1, 3, Some(AIR));
    for y in 0..WORLD_HEIGHT {
        edge.set_skirt(0, y, 5, AIR);
    }
    out.push((
        "unsealed edge source",
        edge.run(
            full_range(),
            0,
            100_000,
            &mut matrix_scratch(),
            &mut [[0; 3]; 16],
        )
        .expect("edge box must scan"),
    ));

    // One dense mixed section at center (-3, 2): flowing water, one unsealed
    // source, and three sources sealed by neighbours, the skirt corner and
    // the section below.
    let mut mixed = RescanBox::solid([-3, 2]);
    mixed.dense_section(2);
    mixed.set_cell(2, 3, 1, 4, WATER_SOURCE + 2);
    mixed.set_cell(2, 5, 2, 6, WATER_SOURCE);
    mixed.set_cell(2, 5, 1, 6, AIR);
    mixed.set_cell(2, 8, 3, 9, WATER_SOURCE);
    mixed.set_cell(2, 0, 5, 0, WATER_SOURCE);
    mixed.set_cell(2, 15, 0, 7, WATER_SOURCE);
    out.push((
        "dense mixed",
        mixed
            .run(
                full_range(),
                0,
                100_000,
                &mut matrix_scratch(),
                &mut [[0; 3]; 2],
            )
            .expect("mixed box must scan"),
    ));

    let zero = RescanBox::solid([0, 0]);
    out.push((
        "budget zero",
        zero.run(full_range(), 0, 0, &mut matrix_scratch(), &mut [[0; 3]; 1])
            .expect("zero budget is admitted"),
    ));

    let one = RescanBox::solid([0, 0]);
    out.push((
        "budget one",
        one.run(full_range(), 0, 1, &mut matrix_scratch(), &mut [[0; 3]; 1])
            .expect("budget one is admitted"),
    ));

    out.into_iter()
        .map(|(name, (summary, positions))| (name, summary, positions))
        .collect()
}

/// Frozen Go/ABI observations for the shared matrix: summary fields and the
/// ordered world positions, in the same scenario order as `run_matrix`.
fn matrix_expectations() -> Vec<Observation> {
    let summary = |written, spent, done, next_section| RescanSummary {
        written,
        spent,
        done,
        next_section,
    };
    vec![
        ("uniform nonfluid", summary(0, 24, true, 24), vec![]),
        ("sealed source", summary(0, 4119, true, 24), vec![]),
        (
            "unsealed edge source",
            summary(16, 4119, true, 24),
            (0..16).map(|y16| [0, 48 + y16 - 64, 4]).collect(),
        ),
        (
            "dense mixed",
            summary(2, 4119, true, 24),
            vec![[-45, -31, 36], [-43, -30, 38]],
        ),
        ("budget zero", summary(0, 0, false, 0), vec![]),
        ("budget one", summary(0, 1, false, 1), vec![]),
    ]
}

#[test]
fn abi_writes_match_go_observations() {
    let matrix = run_matrix();
    let want = matrix_expectations();
    for ((name, summary, positions), (want_name, want_summary, want_positions)) in
        matrix.iter().zip(&want)
    {
        assert_eq!(name, want_name);
        assert_eq!(summary, want_summary, "{name} summary");
        assert_eq!(positions, want_positions, "{name} positions");
    }
}

#[test]
fn evaluation_is_deterministic() {
    let first = run_matrix();
    let second = run_matrix();
    assert_eq!(first, second, "repeated scans must be identical");
}

#[test]
fn native_range_matches_go_rejections() {
    // The legacy ABI layout admits box columns 0 and 17; the typed lane is the
    // strict interior domain and rejects both at the range gate.
    let box_ = RescanBox::solid([0, 0]);
    let cases = [
        RescanRange {
            x0: 0,
            x1: 4,
            z0: 1,
            z1: 4,
        },
        RescanRange {
            x0: 1,
            x1: 17,
            z0: 1,
            z1: 4,
        },
        RescanRange {
            x0: 1,
            x1: 4,
            z0: 0,
            z1: 4,
        },
        RescanRange {
            x0: 1,
            x1: 4,
            z0: 1,
            z1: 17,
        },
    ];
    for range in cases {
        let mut dst = [[0; 3]; 1];
        assert_eq!(
            box_.run(range, 0, 10, &mut scratch(1), &mut dst),
            Err(KernelError::InvalidInput),
            "range {range:?}"
        );
    }
}
