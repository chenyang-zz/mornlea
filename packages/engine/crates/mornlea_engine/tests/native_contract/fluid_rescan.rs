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
/// order, then the four corners. The kernel's `skirt_column` owns the rule;
/// this mirror only stitches fixtures together.
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

    fn run(
        &self,
        range: RescanRange,
        start_section: u8,
        budget: u32,
        scratch: &mut RescanScratch,
        dst: &mut [[i32; 3]],
    ) -> Result<RescanSummary, KernelError> {
        NativeFluidRescan.rescan(
            &RescanRequest {
                view: self.view(),
                range,
                start_section,
                budget,
            },
            scratch,
            dst,
        )
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

fn one_cell_range() -> RescanRange {
    RescanRange {
        x0: 1,
        x1: 1,
        z0: 1,
        z1: 1,
    }
}

fn scratch(capacity: usize) -> RescanScratch {
    RescanScratch::try_with_capacity(capacity).expect("capacity must fit the native bound")
}

fn canary() -> [i32; 3] {
    [i32::MIN, i32::MAX, i32::MIN]
}

#[test]
fn rescan_budget_and_halo() {
    // The scanned column is box column (1, 1): interior to the box domain, so
    // the only reads outside the center chunk are the halo columns the seal
    // check needs. A one-cell range costs 16 per per-cell section (one cell
    // per y step).
    let mut halo = RescanBox::solid([0, 0]);
    halo.dense_section(5);
    // A single source at box (1, 1, y=87) unsealed through the -x halo column.
    halo.set_cell(5, 0, 7, 0, WATER_SOURCE);
    halo.set_skirt(0, 87, 1, AIR);

    // A uniform nonfluid section spends exactly one and emits nothing.
    let solid = RescanBox::solid([0, 0]);
    let mut dst = [canary(); 2];
    let summary = solid
        .run(one_cell_range(), 23, 10, &mut scratch(16), &mut dst)
        .expect("solid box must scan");
    assert_eq!(
        summary,
        RescanSummary {
            written: 0,
            spent: 1,
            done: true,
            next_section: 24,
        }
    );
    assert_eq!(dst, [canary(); 2]);

    // The unsealed edge source emits its exact world position: five uniform
    // sections, the dense section's 16 cells, then 18 uniform sections.
    let mut dst = [canary(); 2];
    let summary = halo
        .run(one_cell_range(), 0, 10_000, &mut scratch(384), &mut dst)
        .expect("halo box must scan");
    assert_eq!(
        summary,
        RescanSummary {
            written: 1,
            spent: 39,
            done: true,
            next_section: 24,
        }
    );
    assert_eq!(dst[0], [0, 23, 0]);
    assert_eq!(dst[1], canary());

    // A zero budget yields before the first section and leaves the
    // destination untouched.
    let mut zero_dst = [canary(); 2];
    let before = zero_dst;
    let summary = halo
        .run(one_cell_range(), 0, 0, &mut scratch(384), &mut zero_dst)
        .expect("zero budget is admitted");
    assert_eq!(
        summary,
        RescanSummary {
            written: 0,
            spent: 0,
            done: false,
            next_section: 0,
        }
    );
    assert_eq!(zero_dst, before);

    // A budget of one still completes the entered section (the documented
    // one-section overshoot) before yielding at the next boundary.
    let mut dense0 = RescanBox::solid([0, 0]);
    dense0.dense_section(0);
    let mut dst = [canary(); 2];
    let summary = dense0
        .run(one_cell_range(), 0, 1, &mut scratch(384), &mut dst)
        .expect("budget one is admitted");
    assert_eq!(
        summary,
        RescanSummary {
            written: 0,
            spent: 16,
            done: false,
            next_section: 1,
        }
    );
}

#[test]
fn position_order() {
    // Flowing water always emits, so the emission order is the pure scan
    // order: y16 outer, z middle, x inner.
    let mut box_ = RescanBox::solid([0, 0]);
    box_.dense_section(0);
    let flowing = WATER_SOURCE + 2;
    box_.set_cell(0, 1, 0, 2, flowing);
    box_.set_cell(0, 3, 0, 2, flowing);
    box_.set_cell(0, 1, 0, 4, flowing);
    box_.set_cell(0, 2, 1, 3, flowing);

    let range = RescanRange {
        x0: 2,
        x1: 4,
        z0: 3,
        z1: 5,
    };
    let mut dst = [[0; 3]; 8];
    let summary = box_
        .run(range, 0, 10_000, &mut scratch(9 * 16 * 24), &mut dst)
        .expect("ordering box must scan");
    assert_eq!(
        summary,
        RescanSummary {
            written: 4,
            spent: 9 * 16 + 23,
            done: true,
            next_section: 24,
        }
    );
    assert_eq!(
        dst[..4],
        [[1, -64, 2], [3, -64, 2], [1, -64, 4], [2, -63, 3]]
    );
    assert_eq!(dst[4], [0; 3]);
}

#[test]
fn next_section_for_start_23() {
    let box_ = RescanBox::solid([0, 0]);
    let mut dst = [[0; 3]; 1];

    // Completion at the last section reports the end of the section list.
    let summary = box_
        .run(full_range(), 23, 1, &mut scratch(256 * 16), &mut dst)
        .expect("last section must scan");
    assert_eq!(
        summary,
        RescanSummary {
            written: 0,
            spent: 1,
            done: true,
            next_section: 24,
        }
    );

    // A budget break at the last section reports the section that was not
    // entered.
    let summary = box_
        .run(full_range(), 23, 0, &mut scratch(256 * 16), &mut dst)
        .expect("zero budget is admitted");
    assert_eq!(
        summary,
        RescanSummary {
            written: 0,
            spent: 0,
            done: false,
            next_section: 23,
        }
    );
}

#[test]
fn range_bounds() {
    let box_ = RescanBox::solid([0, 0]);
    // A tiny scratch and a valid start make any ScratchTooSmall visible, so
    // these assertions also pin that range validity is checked first.
    let mut tiny = scratch(1);
    let mut dst = [canary(); 1];
    let cases = [
        (
            "x0 zero",
            RescanRange {
                x0: 0,
                x1: 4,
                z0: 1,
                z1: 4,
            },
        ),
        (
            "x1 past the halo",
            RescanRange {
                x0: 1,
                x1: 17,
                z0: 1,
                z1: 4,
            },
        ),
        (
            "z0 zero",
            RescanRange {
                x0: 1,
                x1: 4,
                z0: 0,
                z1: 4,
            },
        ),
        (
            "z1 past the halo",
            RescanRange {
                x0: 1,
                x1: 4,
                z0: 1,
                z1: 17,
            },
        ),
        (
            "x inverted",
            RescanRange {
                x0: 5,
                x1: 1,
                z0: 1,
                z1: 4,
            },
        ),
        (
            "z inverted",
            RescanRange {
                x0: 1,
                x1: 4,
                z0: 5,
                z1: 1,
            },
        ),
    ];
    for (name, range) in cases {
        let before = dst;
        assert_eq!(
            box_.run(range, 0, 10, &mut tiny, &mut dst),
            Err(KernelError::InvalidInput),
            "{name}"
        );
        assert_eq!(dst, before, "{name}");
    }

    // start_section is the second preflight gate and rejects the first index
    // past the last section.
    let before = dst;
    assert_eq!(
        box_.run(full_range(), 24, 10, &mut tiny, &mut dst),
        Err(KernelError::InvalidInput)
    );
    assert_eq!(dst, before);
}

#[test]
fn scratch_capacity() {
    let box_ = RescanBox::solid([0, 0]);
    // Worst case: every cell of the full range in every remaining section.
    const NEEDED: usize = 256 * SECTION_EDGE * SECTIONS;
    assert_eq!(NEEDED, 98_304);

    let mut dst = [canary(); 1];
    let before = dst;
    assert_eq!(
        box_.run(full_range(), 0, 10_000, &mut scratch(NEEDED - 1), &mut dst),
        Err(KernelError::ScratchTooSmall {
            needed: NEEDED,
            available: NEEDED - 1,
        })
    );
    assert_eq!(dst, before);

    let summary = box_
        .run(full_range(), 0, 10_000, &mut scratch(NEEDED), &mut dst)
        .expect("exact scratch capacity must be admitted");
    assert_eq!(
        summary,
        RescanSummary {
            written: 0,
            spent: 24,
            done: true,
            next_section: 24,
        }
    );
}

#[test]
fn capacity_and_canary() {
    let mut box_ = RescanBox::solid([0, 0]);
    box_.dense_section(0);
    let flowing = WATER_SOURCE + 2;
    box_.set_cell(0, 1, 0, 2, flowing);
    box_.set_cell(0, 3, 0, 2, flowing);
    box_.set_cell(0, 1, 0, 4, flowing);
    box_.set_cell(0, 2, 1, 3, flowing);
    let range = RescanRange {
        x0: 2,
        x1: 4,
        z0: 3,
        z1: 5,
    };

    // An exactly sized destination succeeds.
    let mut dst = [canary(); 4];
    let summary = box_
        .run(range, 0, 10_000, &mut scratch(9 * 16 * 24), &mut dst)
        .expect("exact destination must be admitted");
    assert_eq!(summary.written, 4);
    assert_eq!(dst[3], [2, -63, 3]);

    // One slot short fails after the scan and leaves every canary untouched.
    let mut short = [canary(); 3];
    let before = short;
    assert_eq!(
        box_.run(range, 0, 10_000, &mut scratch(9 * 16 * 24), &mut short),
        Err(KernelError::OutputTooSmall {
            needed: 4,
            available: 3,
        })
    );
    assert_eq!(short, before);

    // A surplus destination publishes only the used prefix.
    let mut padded = [canary(); 6];
    let summary = box_
        .run(range, 0, 10_000, &mut scratch(9 * 16 * 24), &mut padded)
        .expect("surplus destination must be admitted");
    assert_eq!(summary.written, 4);
    assert_eq!(padded[4], canary());
    assert_eq!(padded[5], canary());
}

#[test]
fn metadata_contradiction_uses_center() {
    // The metadata table's center entry claims the section is uniform stone
    // while the center section record is dense and carries one flowing cell.
    // The center section data must win: the cell is read and emitted.
    let mut box_ = RescanBox::solid([0, 0]);
    box_.dense_section(1);
    box_.set_cell(1, 0, 0, 0, WATER_SOURCE + 2);
    box_.set_meta(0, 0, 1, Some(STONE));

    let mut dst = [canary(); 2];
    let summary = box_
        .run(one_cell_range(), 0, 10_000, &mut scratch(384), &mut dst)
        .expect("contradicting box must scan");
    assert_eq!(
        summary,
        RescanSummary {
            written: 1,
            spent: 1 + 16 + 22,
            done: true,
            next_section: 24,
        }
    );
    // Section 1, y16 0 is world y -48: section 1 starts at index 16.
    assert_eq!(dst[0], [0, -48, 0]);
}

#[test]
fn reuse_after_failure() {
    let mut box_ = RescanBox::solid([0, 0]);
    box_.dense_section(0);
    box_.set_cell(0, 1, 0, 2, WATER_SOURCE + 2);
    let range = RescanRange {
        x0: 2,
        x1: 4,
        z0: 3,
        z1: 5,
    };
    let mut scratch = scratch(9 * 16 * 24);

    // A rejected request leaves the scratch reusable.
    assert_eq!(
        box_.run(
            RescanRange {
                x0: 0,
                x1: 4,
                z0: 3,
                z1: 5
            },
            0,
            10,
            &mut scratch,
            &mut [[0; 3]; 2],
        ),
        Err(KernelError::InvalidInput)
    );
    let mut dst = [canary(); 2];
    let summary = box_
        .run(range, 0, 10_000, &mut scratch, &mut dst)
        .expect("valid call after a rejection must succeed");
    assert_eq!(summary.written, 1);

    // A publication failure leaves the scratch reusable as well.
    assert_eq!(
        box_.run(range, 0, 10_000, &mut scratch, &mut [[0; 3]; 0],),
        Err(KernelError::OutputTooSmall {
            needed: 1,
            available: 0,
        })
    );
    let mut dst = [canary(); 2];
    let summary = box_
        .run(range, 0, 10_000, &mut scratch, &mut dst)
        .expect("valid call after a publication failure must succeed");
    assert_eq!(
        summary,
        RescanSummary {
            written: 1,
            spent: 9 * 16 + 23,
            done: true,
            next_section: 24,
        }
    );
    assert_eq!(dst[0], [1, -64, 2]);
}
