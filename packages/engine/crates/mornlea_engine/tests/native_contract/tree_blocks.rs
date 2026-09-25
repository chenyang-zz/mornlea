use mornlea_engine::native::contracts::KernelError;
use mornlea_engine::native::contracts::world::{TreeBlock, TreeBlocks, TreeOp, TreeRequest};
use mornlea_engine::native::tree::NativeTree;

/// Evaluates the runtime tree at the given root through the native provider.
fn tree_at(seed: i64, x: i32, y: i32, z: i32) -> Result<TreeBlocks, KernelError> {
    NativeTree.tree_blocks(&TreeRequest {
        seed,
        root: [x, y, z],
    })
}

/// Ordered offset sequence of a produced tree, for geometry comparisons.
fn offsets(blocks: &TreeBlocks) -> Vec<[i8; 3]> {
    blocks
        .records()
        .iter()
        .map(|record| record.offset)
        .collect()
}

#[test]
fn tree_order_and_edges() {
    // Roots at both world-height edges produce a complete tree; one step
    // outside the admitted range is rejected before any geometry is read.
    for y in [-64, 311] {
        let blocks = tree_at(0, 3, y, 5).expect("edge root must be admitted");
        assert!(!blocks.records().is_empty());
    }
    for y in [-65, 312] {
        assert_eq!(tree_at(0, 3, y, 5), Err(KernelError::InvalidInput));
    }
}

#[test]
fn root_x_z_edges() {
    // Geometry touches the root's horizontal +-2 neighborhood, so admission
    // requires both neighbors to be representable; one step closer to an
    // i32 edge would wrap the geometry coordinates.
    for edge in [i32::MIN + 2, i32::MAX - 2] {
        let by_x = tree_at(0, edge, 64, 5).expect("representable x neighborhood");
        assert!(!by_x.records().is_empty());
        let by_z = tree_at(0, 5, 64, edge).expect("representable z neighborhood");
        assert!(!by_z.records().is_empty());
    }
    for beyond in [i32::MIN + 1, i32::MAX - 1] {
        assert_eq!(tree_at(0, beyond, 64, 5), Err(KernelError::InvalidInput));
        assert_eq!(tree_at(0, 5, 64, beyond), Err(KernelError::InvalidInput));
    }
}

#[test]
fn ordered_records_match_dy_dz_dx() {
    let blocks = tree_at(0, 3, -64, 5).unwrap();
    let records = blocks.records();
    for pair in records.windows(2) {
        let first = pair[0].offset;
        let second = pair[1].offset;
        assert!(
            (first[1], first[2], first[0]) < (second[1], second[2], second[0]),
            "records must be strictly increasing in (dy, dz, dx)"
        );
    }
    for record in records {
        let [dx, dy, dz] = record.offset;
        assert!((-2..=2).contains(&dx), "dx outside the fringe: {dx}");
        assert!((-2..=2).contains(&dz), "dz outside the fringe: {dz}");
        assert!(
            (0..=8).contains(&dy),
            "dy outside trunk and crown span: {dy}"
        );
        assert!(
            matches!(record.block, 0 | 17 | 19),
            "unexpected block id {}",
            record.block
        );
    }
}

#[test]
fn seed_selects_geometry() {
    // Seeds derived at the same root cover trunk heights five, six and seven.
    let height_five = tree_at(0, 0, 0, 0).unwrap();
    let height_six = tree_at(1, 0, 0, 0).unwrap();
    let height_seven = tree_at(12, 0, 0, 0).unwrap();
    for (label, blocks, want_logs) in [
        ("height five", &height_five, 5usize),
        ("height six", &height_six, 6),
        ("height seven", &height_seven, 7),
    ] {
        let logs = blocks
            .records()
            .iter()
            .filter(|record| record.block == 17)
            .count();
        assert_eq!(logs, want_logs, "{label} trunk log count");
    }
    for (a, b) in [(&height_five, &height_six), (&height_six, &height_seven)] {
        assert_ne!(a.records().len(), b.records().len());
        assert_ne!(offsets(a), offsets(b));
    }

    // A fluffy crown pair at one trunk height differs only in the crown.
    let plain_crown = &height_six;
    let fluffy_crown = tree_at(3, 0, 0, 0).unwrap();
    let fluffy_logs = fluffy_crown
        .records()
        .iter()
        .filter(|record| record.block == 17)
        .count();
    assert_eq!(fluffy_logs, 6);
    assert_ne!(plain_crown.records().len(), fluffy_crown.records().len());
    assert_ne!(offsets(plain_crown), offsets(&fluffy_crown));
}

#[test]
fn record_count_bounded() {
    for seed in 0..32i64 {
        for (x, y, z) in [
            (0, 0, 0),
            (3, -64, 5),
            (-7, 311, -13),
            (i32::MIN + 2, 64, i32::MIN + 2),
            (i32::MAX - 2, 64, i32::MAX - 2),
        ] {
            let blocks = tree_at(seed, x, y, z).unwrap();
            let count = blocks.records().len();
            assert!(
                (1..=128).contains(&count),
                "seed {seed} at ({x}, {y}, {z}) produced {count} records"
            );
        }
    }
}

#[test]
fn from_parts_rejects_oversize_len() {
    let records = [TreeBlock {
        offset: [0; 3],
        block: 0,
    }; 128];
    assert_eq!(
        TreeBlocks::from_parts(records, 129),
        Err(KernelError::InvalidInput)
    );
}
