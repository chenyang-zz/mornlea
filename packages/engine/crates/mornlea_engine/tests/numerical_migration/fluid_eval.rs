use mornlea_engine::native::contracts::KernelError;
use mornlea_engine::native::contracts::fluid::{
    FluidChange, FluidEvalOp, FluidWrites, NeighborSlot,
};
use mornlea_engine::native::fluid_eval::NativeFluidEval;

/// Stable block ids of the fluid rule table, matching `internal/core/block.go`.
const AIR: u16 = 0;
const STONE: u16 = 2;
const WATER_SOURCE: u16 = 27;

/// Largest batch the native lane accepts. The legacy ABI admits one more item;
/// that is adapter-layer compatibility and not a native bound.
const MAX_BATCH_ITEMS: usize = 4096;

/// Wire layout of one output item: four 3-byte entries of slot u8 + block id
/// u16 LE. A slot byte of 0xFF marks an unused entry with zero block bytes.
const ITEM_OUTPUT_BYTES: usize = 12;
const SLOT_NO_WRITE: u8 = 0xFF;

/// Cells in slot order: self, above, below, +x, -x, +z, -z.
fn cells(
    self_id: u16,
    above: u16,
    below: u16,
    pos_x: u16,
    neg_x: u16,
    pos_z: u16,
    neg_z: u16,
) -> [u16; 7] {
    [self_id, above, below, pos_x, neg_x, pos_z, neg_z]
}

fn change(slot: NeighborSlot, block: u16) -> FluidChange {
    FluidChange { slot, block }
}

/// The shared rule-matrix observation batch, in the same order as the Go
/// oracle's records: vertical priority, horizontal spreading, non-source
/// decay, flowing survival, the infinite-source upgrade and the unknown-id
/// skip.
fn rule_matrix() -> [[u16; 7]; 6] {
    [
        cells(WATER_SOURCE, STONE, AIR, STONE, STONE, STONE, STONE),
        cells(WATER_SOURCE, STONE, STONE, AIR, AIR, AIR, AIR),
        cells(WATER_SOURCE + 2, STONE, AIR, STONE, STONE, STONE, STONE),
        cells(WATER_SOURCE + 3, STONE, STONE, WATER_SOURCE, AIR, AIR, AIR),
        cells(AIR, STONE, STONE, WATER_SOURCE, WATER_SOURCE, STONE, STONE),
        cells(65535, STONE, STONE, STONE, STONE, STONE, STONE),
    ]
}

/// Expected ordered change lists per matrix item, pinned from the Go-oracle
/// observation run.
fn rule_changes() -> [Vec<FluidChange>; 6] {
    let source = WATER_SOURCE;
    [
        vec![change(NeighborSlot::Below, source + 1)],
        vec![
            change(NeighborSlot::PosX, source + 1),
            change(NeighborSlot::NegX, source + 1),
            change(NeighborSlot::PosZ, source + 1),
            change(NeighborSlot::NegZ, source + 1),
        ],
        vec![change(NeighborSlot::SelfCell, AIR)],
        vec![
            change(NeighborSlot::NegX, source + 4),
            change(NeighborSlot::PosZ, source + 4),
            change(NeighborSlot::NegZ, source + 4),
        ],
        vec![change(NeighborSlot::SelfCell, source)],
        vec![],
    ]
}

fn evaluate(items: &[[u16; 7]]) -> Vec<FluidWrites> {
    let mut dst = vec![FluidWrites::default(); items.len()];
    let written = NativeFluidEval
        .evaluate(items, &mut dst)
        .expect("matrix batch must be admitted");
    assert_eq!(written, items.len());
    dst
}

/// Encodes one item's typed writes into the 12-byte ABI record so parity is
/// checked on the exact wire bytes the Go oracle digests. Used entries pack
/// from index 0; the remainder carries the no-write sentinel.
fn encode_item(writes: &FluidWrites) -> [u8; ITEM_OUTPUT_BYTES] {
    let mut record = [
        SLOT_NO_WRITE,
        0,
        0,
        SLOT_NO_WRITE,
        0,
        0,
        SLOT_NO_WRITE,
        0,
        0,
        SLOT_NO_WRITE,
        0,
        0,
    ];
    for (index, entry) in writes.changes().iter().enumerate() {
        let base = index * 3;
        record[base] = entry.slot as u8;
        record[base + 1..base + 3].copy_from_slice(&entry.block.to_le_bytes());
    }
    record
}

/// FNV-1a 64-bit digest over the encoded record stream, matching the Go
/// oracle's `hash/fnv` New64a over the same bytes.
fn record_digest(writes: &[FluidWrites]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for entry in writes.iter().flat_map(encode_item) {
        hash ^= u64::from(entry);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

#[test]
fn abi_writes_match_go_observations() {
    let writes = evaluate(&rule_matrix());
    let expected = rule_changes();
    for (index, item) in writes.iter().enumerate() {
        assert_eq!(item.changes(), expected[index], "matrix item {index}");
    }

    // Pinned release-ABI digest shared with `TestFluidEval` in the Go runtime
    // oracle, computed over the exact 72-byte output stream.
    assert_eq!(record_digest(&writes), 0x84ac_a3f0_db8d_0ae2);
}

#[test]
fn evaluation_is_deterministic() {
    let items = rule_matrix();
    let first = evaluate(&items);
    let second = evaluate(&items);
    assert_eq!(first, second, "repeated evaluation must be identical");
    assert_eq!(record_digest(&first), record_digest(&second));
}

#[test]
fn count_boundary_is_native_bounded() {
    let full = vec![rule_matrix()[0]; MAX_BATCH_ITEMS];
    let mut dst = vec![FluidWrites::default(); MAX_BATCH_ITEMS];
    assert_eq!(
        NativeFluidEval.evaluate(&full, &mut dst),
        Ok(MAX_BATCH_ITEMS)
    );

    let over = vec![rule_matrix()[0]; MAX_BATCH_ITEMS + 1];
    let mut untouched = vec![FluidWrites::default(); MAX_BATCH_ITEMS + 1];
    let before = untouched.clone();
    assert_eq!(
        NativeFluidEval.evaluate(&over, &mut untouched),
        Err(KernelError::InvalidInput)
    );
    assert_eq!(untouched, before);
}
