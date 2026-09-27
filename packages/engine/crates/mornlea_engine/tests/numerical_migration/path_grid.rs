//! Immutable path grid checks: shape validation, revision identity and
//! snapshot reads through the native grid surface.
//!
//! The fixtures mirror the Go oracle scenes in
//! `packages/tools/cmd/runtime-oracle/pathfind_test.go`: the same coordinate
//! terrain must read back through the same Y-fast order on both sides, so a
//! mismatch here fails before any search work builds on these reads.

use mornlea_engine::native::contracts::{
    PathBlockTable, PathCell, PathError, PathGrid, PathRevision,
};
use mornlea_engine::native::pathfind::{block_at, flat_index, is_passable, is_standing};

/// Passable air id shared with the block table used by the Go oracle.
const AIR: u16 = 0;
/// Solid stone id shared with the block table used by the Go oracle.
const STONE: u16 = 2;
/// Unknown block id: absent from every passability table, always blocked.
const UNKNOWN: u16 = 65535;

/// Cell cap shared with the frozen grid contract.
const CELLS_CAP: usize = 131072;

fn cell(x: i32, y: i32, z: i32) -> PathCell {
    PathCell { x, y, z }
}

fn revision(chunk_x: i32, chunk_z: i32, revision: u64) -> PathRevision {
    PathRevision {
        chunk: [chunk_x, chunk_z],
        revision,
    }
}

fn air_table() -> PathBlockTable {
    PathBlockTable::from_passable_ids(&[AIR]).expect("air-only table must build")
}

/// Builds a grid whose flat blocks carry their own Y-fast marker values, so
/// reads can prove the indexing order coordinate by coordinate.
fn marker_grid(origin: PathCell, size: [u32; 3]) -> PathGrid {
    let cells = size[0] as usize * size[1] as usize * size[2] as usize;
    let blocks: Box<[u16]> = (0..cells)
        .map(|index| index as u16)
        .collect::<Vec<_>>()
        .into_boxed_slice();
    PathGrid::try_new(origin, size, blocks, air_table(), Vec::new())
        .expect("marker grid must build")
}

/// Reference Y-fast address used to pin the expected marker values.
fn y_fast(size: [u32; 3], x: u32, y: u32, z: u32) -> usize {
    ((x as usize * size[2] as usize) + z as usize) * size[1] as usize + y as usize
}

fn build(
    origin: PathCell,
    size: [u32; 3],
    blocks: Vec<u16>,
    revisions: Vec<PathRevision>,
) -> Result<PathGrid, PathError> {
    PathGrid::try_new(
        origin,
        size,
        blocks.into_boxed_slice(),
        air_table(),
        revisions,
    )
}

/// Unit cube success plus every shape rejection: zero dimensions, cell cap
/// overflow, wrong block length and signed far-corner overflow.
fn unit_cube_and_shape_rejections() {
    let unit = build(cell(0, 0, 0), [1, 1, 1], vec![AIR], vec![revision(0, 0, 7)])
        .expect("unit cube must build");
    assert_eq!(unit.origin(), cell(0, 0, 0));
    assert_eq!(unit.size(), [1, 1, 1]);
    assert_eq!(unit.revisions(), &[revision(0, 0, 7)]);

    // Any zero dimension is rejected before any block is consulted.
    for size in [[0, 1, 1], [1, 0, 1], [1, 1, 0]] {
        assert_eq!(
            build(cell(0, 0, 0), size, vec![AIR], Vec::new()),
            Err(PathError::InvalidGrid),
            "zero dimension {size:?} must be rejected"
        );
    }

    // One cell over the cap is rejected even with a matching block box.
    assert_eq!(
        build(
            cell(0, 0, 0),
            [CELLS_CAP as u32 + 1, 1, 1],
            vec![AIR; CELLS_CAP + 1],
            Vec::new()
        ),
        Err(PathError::InvalidGrid),
        "cell count over the cap must be rejected"
    );

    // A short or long box never yields a partial grid.
    assert_eq!(
        build(cell(0, 0, 0), [2, 1, 1], vec![AIR, AIR, AIR], Vec::new()),
        Err(PathError::InvalidGrid),
        "long block box must be rejected"
    );
    assert_eq!(
        build(cell(0, 0, 0), [2, 1, 1], vec![AIR], Vec::new()),
        Err(PathError::InvalidGrid),
        "short block box must be rejected"
    );

    // A far corner past the signed maximum is rejected on every axis.
    assert_eq!(
        build(
            cell(i32::MAX, i32::MAX, i32::MAX),
            [2, 2, 2],
            vec![AIR; 8],
            Vec::new()
        ),
        Err(PathError::InvalidGrid),
        "far corner past the maximum must be rejected"
    );
    assert_eq!(
        build(cell(i32::MAX, 0, 0), [2, 1, 1], vec![AIR, AIR], Vec::new()),
        Err(PathError::InvalidGrid),
        "far corner past the maximum on one axis must be rejected"
    );

    // The minimum origin still builds: the boundary is one-sided because the
    // far corner only grows toward the maximum, never below the minimum.
    let low = build(
        cell(i32::MIN, i32::MIN, i32::MIN),
        [2, 2, 2],
        vec![AIR; 8],
        Vec::new(),
    )
    .expect("minimum origin must build");
    assert_eq!(block_at(&low, i32::MIN, i32::MIN, i32::MIN), Some(AIR));
}

/// The cell cap boundary: exactly at the cap builds on one axis and spread
/// across three axes.
fn cell_cap_boundary() {
    build(
        cell(0, 0, 0),
        [CELLS_CAP as u32, 1, 1],
        vec![AIR; CELLS_CAP],
        Vec::new(),
    )
    .expect("exactly at the cap must build");
    build(
        cell(0, 0, 0),
        [64, 32, 64],
        vec![AIR; CELLS_CAP],
        Vec::new(),
    )
    .expect("exactly at the cap across three axes must build");
}

/// Revision identity: cap, pre-dedup cap, sorting, collapse and conflicts.
fn revision_identity() {
    // Nine unique revisions are accepted and reported sorted by chunk.
    let nine: Vec<PathRevision> = (0..9).map(|i| revision(8 - i, i % 3, i as u64)).collect();
    let grid = build(cell(0, 0, 0), [1, 1, 1], vec![AIR], nine.clone())
        .expect("nine revisions must build");
    let mut sorted = nine.clone();
    sorted.sort_by_key(|entry| entry.chunk);
    assert_eq!(grid.revisions(), sorted.as_slice());

    // Ten unique revisions are rejected.
    let ten: Vec<PathRevision> = (0..10).map(|i| revision(i, 0, i as u64)).collect();
    assert_eq!(
        build(cell(0, 0, 0), [1, 1, 1], vec![AIR], ten),
        Err(PathError::InvalidRevision),
        "ten revisions must be rejected"
    );

    // Ten entries collapsing to nine are still rejected: the cap applies
    // before dedup.
    let mut ten_collapsing = nine.clone();
    ten_collapsing.push(nine[0]);
    assert_eq!(
        build(cell(0, 0, 0), [1, 1, 1], vec![AIR], ten_collapsing),
        Err(PathError::InvalidRevision),
        "ten entries before dedup must be rejected"
    );

    // Equal duplicates collapse and sort.
    let grid = build(
        cell(0, 0, 0),
        [1, 1, 1],
        vec![AIR],
        vec![revision(1, 0, 2), revision(0, 0, 1), revision(1, 0, 2)],
    )
    .expect("equal duplicates must collapse");
    assert_eq!(grid.revisions(), &[revision(0, 0, 1), revision(1, 0, 2)]);

    // Conflicting duplicates reject.
    assert_eq!(
        build(
            cell(0, 0, 0),
            [1, 1, 1],
            vec![AIR],
            vec![revision(0, 0, 1), revision(0, 0, 2)]
        ),
        Err(PathError::InvalidRevision),
        "conflicting duplicates must be rejected"
    );
}

/// Y-fast order proved through marker reads at coordinates where an X-fast
/// layout would disagree, plus overflow and out-of-range refusal.
fn y_fast_order() {
    let origin = cell(10, 20, 30);
    let size = [2, 2, 3];
    let grid = marker_grid(origin, size);
    for x in 0..2_u32 {
        for y in 0..2_u32 {
            for z in 0..3_u32 {
                let want = y_fast(size, x, y, z) as u16;
                assert_eq!(
                    block_at(&grid, 10 + x as i32, 20 + y as i32, 30 + z as i32),
                    Some(want),
                    "marker at local ({x}, {y}, {z}) must follow Y-fast order"
                );
                assert_eq!(flat_index(size, x, y, z), Some(y_fast(size, x, y, z)));
            }
        }
    }

    // Spotlight coordinates: an X-fast layout
    // `((y * size_z) + z) * size_x + x` would produce 1 and 6 here.
    assert_eq!(block_at(&grid, 11, 20, 30), Some(6));
    assert_eq!(block_at(&grid, 10, 21, 30), Some(1));

    // Outside the box reads nothing and never panics.
    assert_eq!(block_at(&grid, 9, 20, 30), None);
    assert_eq!(block_at(&grid, 12, 21, 32), None);
    assert_eq!(block_at(&grid, 11, 22, 30), None);
    assert_eq!(flat_index(size, 2, 0, 0), None);
    assert_eq!(flat_index(size, 0, 2, 0), None);
    assert_eq!(flat_index(size, 0, 0, 3), None);

    // Astronomic sizes refuse instead of wrapping.
    assert_eq!(
        flat_index([u32::MAX; 3], u32::MAX - 1, u32::MAX - 1, u32::MAX - 1),
        None,
        "overflowing address arithmetic must refuse"
    );
}

/// Unknown ids are stored and readable but never passable; standing on an
/// unknown support still counts as solid ground, exactly like the oracle.
fn unknown_ids_stay_blocked() {
    // Flat order is Y-fast: per x column, floor, walking layer, headroom.
    let walking: Box<[u16]> =
        vec![STONE, AIR, AIR, STONE, UNKNOWN, AIR, STONE, AIR, AIR].into_boxed_slice();
    let grid = PathGrid::try_new(cell(0, 63, 0), [3, 3, 1], walking, air_table(), Vec::new())
        .expect("unknown-id grid must build");

    // The unknown id is present in the snapshot ...
    assert_eq!(block_at(&grid, 1, 64, 0), Some(UNKNOWN));
    // ... but never passable and never standing room.
    assert!(!is_passable(&grid, 1, 64, 0));
    assert!(is_passable(&grid, 0, 64, 0));
    assert!(!is_standing(&grid, cell(1, 64, 0)));
    assert!(is_standing(&grid, cell(0, 64, 0)));

    // An unknown support still counts as solid ground: the rule blocks
    // occupying unknown cells, not standing on them.
    let supported: Box<[u16]> =
        vec![STONE, AIR, AIR, UNKNOWN, AIR, AIR, STONE, AIR, AIR].into_boxed_slice();
    let grid = PathGrid::try_new(
        cell(0, 63, 0),
        [3, 3, 1],
        supported,
        air_table(),
        Vec::new(),
    )
    .expect("unknown-support grid must build");
    assert!(is_standing(&grid, cell(1, 64, 0)));
}

/// Ownership transfer: retained clones can be mutated freely after the move
/// without reaching the grid snapshot.
fn ownership_snapshot() {
    let origin = cell(-5, 0, 7);
    let size = [2, 1, 2];
    let blocks = vec![AIR, STONE, AIR, STONE];
    let blocks_clone = blocks.clone();
    let passable = vec![AIR];
    let mut passable_clone = passable.clone();
    let revisions = vec![revision(0, 0, 1)];
    let mut revisions_clone = revisions.clone();
    let grid = PathGrid::try_new(
        origin,
        size,
        blocks.into_boxed_slice(),
        PathBlockTable::from_passable_ids(&passable).expect("table must build"),
        revisions,
    )
    .expect("owned grid must build");

    // Mutate only the retained clones; the moved originals are gone and the
    // grid must read exactly as constructed.
    let mut blocks_clone = blocks_clone;
    blocks_clone.fill(UNKNOWN);
    passable_clone.push(STONE);
    revisions_clone[0] = revision(9, 9, 9);
    revisions_clone.push(revision(8, 8, 8));

    for x in 0..2_i32 {
        for z in 0..2_i32 {
            let want = if z == 0 { AIR } else { STONE };
            assert_eq!(block_at(&grid, -5 + x, 0, 7 + z), Some(want));
            assert_eq!(is_passable(&grid, -5 + x, 0, 7 + z), z == 0);
        }
    }
    assert_eq!(grid.revisions(), &[revision(0, 0, 1)]);
}

/// Two identical constructions read identical on every cell and revision.
fn identical_constructions_agree() {
    let make = || {
        PathGrid::try_new(
            cell(3, -2, 5),
            [3, 2, 2],
            (0..12)
                .map(|index| (index % 3) as u16)
                .collect::<Vec<_>>()
                .into_boxed_slice(),
            PathBlockTable::from_passable_ids(&[0, 1]).expect("table must build"),
            vec![revision(2, 1, 4), revision(0, 3, 2)],
        )
        .expect("repeated construction must build")
    };
    let first = make();
    let second = make();
    assert_eq!(first.origin(), second.origin());
    assert_eq!(first.size(), second.size());
    assert_eq!(first.revisions(), second.revisions());
    for x in 3..6 {
        for y in -2..0 {
            for z in 5..7 {
                assert_eq!(
                    block_at(&first, x, y, z),
                    block_at(&second, x, y, z),
                    "block reads must agree at ({x}, {y}, {z})"
                );
                assert_eq!(
                    is_passable(&first, x, y, z),
                    is_passable(&second, x, y, z),
                    "passability must agree at ({x}, {y}, {z})"
                );
                assert_eq!(
                    is_standing(&first, cell(x, y, z)),
                    is_standing(&second, cell(x, y, z)),
                    "standing must agree at ({x}, {y}, {z})"
                );
            }
        }
    }
}

#[test]
fn path_grid_shape_revision_and_snapshot() {
    unit_cube_and_shape_rejections();
    cell_cap_boundary();
    revision_identity();
    y_fast_order();
    unknown_ids_stay_blocked();
    ownership_snapshot();
    identical_constructions_agree();
}

#[test]
fn path_grid_rejects_product_overflow() {
    assert_eq!(
        build(cell(0, 0, 0), [u32::MAX; 3], Vec::new(), Vec::new()),
        Err(PathError::InvalidGrid)
    );
}

#[test]
fn path_grid_rejects_product_wrapping_to_zero() {
    assert_eq!(
        build(
            cell(i32::MIN, i32::MIN, i32::MIN),
            [1 << 22; 3],
            Vec::new(),
            Vec::new()
        ),
        Err(PathError::InvalidGrid)
    );
}
