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

unsafe extern "C" {
    fn mornlea_tree_blocks(
        abi_version: u32,
        input: *const u8,
        input_len: usize,
        output: *mut u8,
        output_len: usize,
    ) -> u32;
    fn mornlea_engine_abi_version() -> u32;
}

const ABI_CANARY: u8 = 0xA5;

/// Builds the raw 28-byte `MTB1` tree request: magic + layout 1 + seed LE +
/// x/y/z LE.
fn tree_abi_input(seed: i64, x: i32, y: i32, z: i32) -> Vec<u8> {
    let mut bytes = vec![0u8; 28];
    bytes[0..4].copy_from_slice(b"MTB1");
    bytes[4..8].copy_from_slice(&1u32.to_le_bytes());
    bytes[8..16].copy_from_slice(&seed.to_le_bytes());
    bytes[16..20].copy_from_slice(&x.to_le_bytes());
    bytes[20..24].copy_from_slice(&y.to_le_bytes());
    bytes[24..28].copy_from_slice(&z.to_le_bytes());
    bytes
}

/// Encodes typed provider records as ABI output bytes: count u32 LE plus one
/// 8-byte record per entry (dx/dy/dz, reserved 0, block LE, reserved 0).
fn tree_encoded(records: &[TreeBlock]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(4 + records.len() * 8);
    bytes.extend_from_slice(&(records.len() as u32).to_le_bytes());
    for record in records {
        bytes.push(record.offset[0] as u8);
        bytes.push(record.offset[1] as u8);
        bytes.push(record.offset[2] as u8);
        bytes.push(0);
        bytes.extend_from_slice(&record.block.to_le_bytes());
        bytes.extend_from_slice(&[0u8; 2]);
    }
    bytes
}

/// Calls the real exported `mornlea_tree_blocks` symbol with a
/// canary-filled destination, returning the status and the destination bytes.
fn call_tree_abi(input: &[u8], output_len: usize) -> (u32, Vec<u8>) {
    let version = unsafe { mornlea_engine_abi_version() };
    let mut output = vec![ABI_CANARY; output_len];
    let status = unsafe {
        mornlea_tree_blocks(
            version,
            input.as_ptr(),
            input.len(),
            output.as_mut_ptr(),
            output.len(),
        )
    };
    (status, output)
}

#[test]
fn tree_blocks_abi_order() {
    // Seed 0 at (3,-64,5): the full output bytes (count plus every ordered
    // 8-byte record) must equal the typed provider's encoded bytes, pinning
    // ordered-record parity through the adapter.
    let input = tree_abi_input(0, 3, -64, 5);
    let expected = tree_encoded(&records_at(0, 3, -64, 5));
    let count = u32::from_le_bytes(expected[0..4].try_into().unwrap()) as usize;
    let needed = 4usize.checked_add(count.checked_mul(8).unwrap()).unwrap();
    assert_eq!(needed, expected.len(), "needed matches the encoded bytes");
    let (status, output) = call_tree_abi(&input, needed);
    assert_eq!(status, 0, "seed 0 at (3,-64,5) status");
    assert_eq!(output, expected, "seed 0 at (3,-64,5) full output bytes");

    // Seed 1 at the negative root (-7,311,-13): the same full-bytes equality
    // pins the upper height edge through the adapter.
    let high_input = tree_abi_input(1, -7, 311, -13);
    let high_expected = tree_encoded(&records_at(1, -7, 311, -13));
    let (status, high_output) = call_tree_abi(&high_input, high_expected.len());
    assert_eq!(status, 0, "seed 1 at (-7,311,-13) status");
    assert_eq!(
        high_output, high_expected,
        "seed 1 at (-7,311,-13) full output bytes"
    );

    // One byte short: status 7 with the full canary untouched.
    let short_before = vec![ABI_CANARY; needed - 1];
    let (status, short) = call_tree_abi(&input, needed - 1);
    assert_eq!(status, 7, "short output status");
    assert_eq!(short, short_before, "short output keeps the canary");

    // Larger destination: status 0 with the suffix beyond `needed` untouched,
    // pinning the at-least-capacity ruling.
    let (status, large) = call_tree_abi(&input, needed + 16);
    assert_eq!(status, 0, "larger destination status");
    assert_eq!(
        large[..needed],
        expected[..],
        "larger destination prefix bytes"
    );
    assert_eq!(
        large[needed..],
        vec![ABI_CANARY; 16],
        "larger destination suffix untouched"
    );
}
