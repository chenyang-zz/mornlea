use mornlea_engine::native::contracts::world::{TreeBlock, TreeOp, TreeRequest};
use mornlea_engine::native::tree::NativeTree;

/// Converts frozen `(dx, dy, dz, block)` observation tuples into contract records.
fn frozen(tuples: &[(i8, i8, i8, u16)]) -> Vec<TreeBlock> {
    tuples
        .iter()
        .map(|&(dx, dy, dz, block)| TreeBlock {
            offset: [dx, dy, dz],
            block,
        })
        .collect()
}

/// Records produced by the typed provider for one request.
fn records_at(seed: i64, x: i32, y: i32, z: i32) -> Vec<TreeBlock> {
    NativeTree
        .tree_blocks(&TreeRequest {
            seed,
            root: [x, y, z],
        })
        .unwrap()
        .records()
        .to_vec()
}

#[test]
fn trunk_height_five_ordered_records() {
    // Pinned from the Go-oracle observation run (`TestKernelTreeBlocks`, seed 0
    // at root (0, 0, 0): trunk height five, ordinary crown). The typed provider
    // and the Go/ABI side must produce identical ordered tuples for the same
    // request.
    let want: [(i8, i8, i8, u16); 58] = [
        (0, 0, 0, 17),
        (0, 1, 0, 17),
        (-1, 2, -2, 19),
        (0, 2, -2, 19),
        (1, 2, -2, 19),
        (-2, 2, -1, 19),
        (-1, 2, -1, 19),
        (0, 2, -1, 19),
        (1, 2, -1, 19),
        (2, 2, -1, 19),
        (-2, 2, 0, 19),
        (-1, 2, 0, 19),
        (0, 2, 0, 17),
        (1, 2, 0, 19),
        (2, 2, 0, 19),
        (-2, 2, 1, 19),
        (-1, 2, 1, 19),
        (0, 2, 1, 19),
        (1, 2, 1, 19),
        (2, 2, 1, 19),
        (-1, 2, 2, 19),
        (0, 2, 2, 19),
        (1, 2, 2, 19),
        (-1, 3, -2, 19),
        (0, 3, -2, 19),
        (1, 3, -2, 19),
        (-2, 3, -1, 19),
        (-1, 3, -1, 19),
        (0, 3, -1, 19),
        (1, 3, -1, 19),
        (2, 3, -1, 19),
        (-2, 3, 0, 19),
        (-1, 3, 0, 19),
        (0, 3, 0, 17),
        (1, 3, 0, 19),
        (2, 3, 0, 19),
        (-2, 3, 1, 19),
        (-1, 3, 1, 19),
        (0, 3, 1, 19),
        (1, 3, 1, 19),
        (2, 3, 1, 19),
        (-1, 3, 2, 19),
        (0, 3, 2, 19),
        (1, 3, 2, 19),
        (-1, 4, -1, 19),
        (0, 4, -1, 19),
        (1, 4, -1, 19),
        (-1, 4, 0, 19),
        (0, 4, 0, 17),
        (1, 4, 0, 19),
        (-1, 4, 1, 19),
        (0, 4, 1, 19),
        (1, 4, 1, 19),
        (0, 5, -1, 19),
        (-1, 5, 0, 19),
        (0, 5, 0, 19),
        (1, 5, 0, 19),
        (0, 5, 1, 19),
    ];
    assert_eq!(records_at(0, 0, 0, 0), frozen(&want));
}

#[test]
fn trunk_height_six_ordered_records() {
    // Pinned from the Go-oracle observation run (`TestKernelTreeBlocks`, seed 1
    // at root (0, 0, 0): trunk height six, ordinary crown). The typed provider
    // and the Go/ABI side must produce identical ordered tuples for the same
    // request.
    let want: [(i8, i8, i8, u16); 59] = [
        (0, 0, 0, 17),
        (0, 1, 0, 17),
        (0, 2, 0, 17),
        (-1, 3, -2, 19),
        (0, 3, -2, 19),
        (1, 3, -2, 19),
        (-2, 3, -1, 19),
        (-1, 3, -1, 19),
        (0, 3, -1, 19),
        (1, 3, -1, 19),
        (2, 3, -1, 19),
        (-2, 3, 0, 19),
        (-1, 3, 0, 19),
        (0, 3, 0, 17),
        (1, 3, 0, 19),
        (2, 3, 0, 19),
        (-2, 3, 1, 19),
        (-1, 3, 1, 19),
        (0, 3, 1, 19),
        (1, 3, 1, 19),
        (2, 3, 1, 19),
        (-1, 3, 2, 19),
        (0, 3, 2, 19),
        (1, 3, 2, 19),
        (-1, 4, -2, 19),
        (0, 4, -2, 19),
        (1, 4, -2, 19),
        (-2, 4, -1, 19),
        (-1, 4, -1, 19),
        (0, 4, -1, 19),
        (1, 4, -1, 19),
        (2, 4, -1, 19),
        (-2, 4, 0, 19),
        (-1, 4, 0, 19),
        (0, 4, 0, 17),
        (1, 4, 0, 19),
        (2, 4, 0, 19),
        (-2, 4, 1, 19),
        (-1, 4, 1, 19),
        (0, 4, 1, 19),
        (1, 4, 1, 19),
        (2, 4, 1, 19),
        (-1, 4, 2, 19),
        (0, 4, 2, 19),
        (1, 4, 2, 19),
        (-1, 5, -1, 19),
        (0, 5, -1, 19),
        (1, 5, -1, 19),
        (-1, 5, 0, 19),
        (0, 5, 0, 17),
        (1, 5, 0, 19),
        (-1, 5, 1, 19),
        (0, 5, 1, 19),
        (1, 5, 1, 19),
        (0, 6, -1, 19),
        (-1, 6, 0, 19),
        (0, 6, 0, 19),
        (1, 6, 0, 19),
        (0, 6, 1, 19),
    ];
    assert_eq!(records_at(1, 0, 0, 0), frozen(&want));
}

#[test]
fn trunk_height_seven_ordered_records() {
    // Pinned from the Go-oracle observation run (`TestKernelTreeBlocks`, seed 12
    // at root (0, 0, 0): trunk height seven, ordinary crown). The typed
    // provider and the Go/ABI side must produce identical ordered tuples for the
    // same request.
    let want: [(i8, i8, i8, u16); 60] = [
        (0, 0, 0, 17),
        (0, 1, 0, 17),
        (0, 2, 0, 17),
        (0, 3, 0, 17),
        (-1, 4, -2, 19),
        (0, 4, -2, 19),
        (1, 4, -2, 19),
        (-2, 4, -1, 19),
        (-1, 4, -1, 19),
        (0, 4, -1, 19),
        (1, 4, -1, 19),
        (2, 4, -1, 19),
        (-2, 4, 0, 19),
        (-1, 4, 0, 19),
        (0, 4, 0, 17),
        (1, 4, 0, 19),
        (2, 4, 0, 19),
        (-2, 4, 1, 19),
        (-1, 4, 1, 19),
        (0, 4, 1, 19),
        (1, 4, 1, 19),
        (2, 4, 1, 19),
        (-1, 4, 2, 19),
        (0, 4, 2, 19),
        (1, 4, 2, 19),
        (-1, 5, -2, 19),
        (0, 5, -2, 19),
        (1, 5, -2, 19),
        (-2, 5, -1, 19),
        (-1, 5, -1, 19),
        (0, 5, -1, 19),
        (1, 5, -1, 19),
        (2, 5, -1, 19),
        (-2, 5, 0, 19),
        (-1, 5, 0, 19),
        (0, 5, 0, 17),
        (1, 5, 0, 19),
        (2, 5, 0, 19),
        (-2, 5, 1, 19),
        (-1, 5, 1, 19),
        (0, 5, 1, 19),
        (1, 5, 1, 19),
        (2, 5, 1, 19),
        (-1, 5, 2, 19),
        (0, 5, 2, 19),
        (1, 5, 2, 19),
        (-1, 6, -1, 19),
        (0, 6, -1, 19),
        (1, 6, -1, 19),
        (-1, 6, 0, 19),
        (0, 6, 0, 17),
        (1, 6, 0, 19),
        (-1, 6, 1, 19),
        (0, 6, 1, 19),
        (1, 6, 1, 19),
        (0, 7, -1, 19),
        (-1, 7, 0, 19),
        (0, 7, 0, 19),
        (1, 7, 0, 19),
        (0, 7, 1, 19),
    ];
    assert_eq!(records_at(12, 0, 0, 0), frozen(&want));
}

#[test]
fn fluffy_crown_ordered_records() {
    // Pinned from the Go-oracle observation run (`TestKernelTreeBlocks`, seed 3
    // at root (0, 0, 0): trunk height six with a fluffy crown). The typed
    // provider and the Go/ABI side must produce identical ordered tuples for the
    // same request.
    let want: [(i8, i8, i8, u16); 64] = [
        (0, 0, 0, 17),
        (0, 1, 0, 17),
        (0, 2, 0, 17),
        (-1, 3, -2, 19),
        (0, 3, -2, 19),
        (1, 3, -2, 19),
        (-2, 3, -1, 19),
        (-1, 3, -1, 19),
        (0, 3, -1, 19),
        (1, 3, -1, 19),
        (2, 3, -1, 19),
        (-2, 3, 0, 19),
        (-1, 3, 0, 19),
        (0, 3, 0, 17),
        (1, 3, 0, 19),
        (2, 3, 0, 19),
        (-2, 3, 1, 19),
        (-1, 3, 1, 19),
        (0, 3, 1, 19),
        (1, 3, 1, 19),
        (2, 3, 1, 19),
        (-1, 3, 2, 19),
        (0, 3, 2, 19),
        (1, 3, 2, 19),
        (-1, 4, -2, 19),
        (0, 4, -2, 19),
        (1, 4, -2, 19),
        (-2, 4, -1, 19),
        (-1, 4, -1, 19),
        (0, 4, -1, 19),
        (1, 4, -1, 19),
        (2, 4, -1, 19),
        (-2, 4, 0, 19),
        (-1, 4, 0, 19),
        (0, 4, 0, 17),
        (1, 4, 0, 19),
        (2, 4, 0, 19),
        (-2, 4, 1, 19),
        (-1, 4, 1, 19),
        (0, 4, 1, 19),
        (1, 4, 1, 19),
        (2, 4, 1, 19),
        (-1, 4, 2, 19),
        (0, 4, 2, 19),
        (1, 4, 2, 19),
        (-1, 5, -1, 19),
        (0, 5, -1, 19),
        (1, 5, -1, 19),
        (-1, 5, 0, 19),
        (0, 5, 0, 17),
        (1, 5, 0, 19),
        (-1, 5, 1, 19),
        (0, 5, 1, 19),
        (1, 5, 1, 19),
        (0, 6, -1, 19),
        (-1, 6, 0, 19),
        (0, 6, 0, 19),
        (1, 6, 0, 19),
        (0, 6, 1, 19),
        (0, 7, -1, 19),
        (-1, 7, 0, 19),
        (0, 7, 0, 19),
        (1, 7, 0, 19),
        (0, 7, 1, 19),
    ];
    assert_eq!(records_at(3, 0, 0, 0), frozen(&want));
}

#[test]
fn ordered_records_are_deterministic() {
    let first = records_at(12, 0, 0, 0);
    let second = records_at(12, 0, 0, 0);
    assert_eq!(first, second);
}

#[test]
fn different_seed_changes_ordered_records() {
    assert_ne!(records_at(0, 0, 0, 0), records_at(1, 0, 0, 0));
    assert_ne!(records_at(1, 0, 0, 0), records_at(3, 0, 0, 0));
}
