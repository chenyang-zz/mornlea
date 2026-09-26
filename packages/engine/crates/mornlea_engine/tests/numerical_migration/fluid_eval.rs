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

unsafe extern "C" {
    fn mornlea_fluid_eval_batch(
        abi_version: u32,
        input: *const u8,
        input_len: usize,
        output: *mut u8,
        output_capacity: usize,
        output_len: *mut usize,
    ) -> u32;
    fn mornlea_engine_abi_version() -> u32;
}

/// Wire widths of the v1 eval request and its output records.
const EVAL_HEADER_BYTES: usize = 8;
const EVAL_ITEM_INPUT_BYTES: usize = 14;
const EVAL_LAYOUT_VERSION: u32 = 1;

/// ABI status codes mirrored from the engine exports.
const EVAL_STATUS_OK: u32 = 0;
const EVAL_STATUS_INVALID_ARGUMENT: u32 = 2;
const EVAL_STATUS_INPUT: u32 = 3;

const ABI_CANARY: u8 = 0xA5;

/// Encodes matrix items into the raw v1 ABI request: layout u32 LE plus
/// count u32 LE, then 14 bytes per item of 7 u16 LE cells in slot order.
fn eval_abi_input(items: &[[u16; 7]]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(EVAL_HEADER_BYTES + items.len() * EVAL_ITEM_INPUT_BYTES);
    bytes.extend_from_slice(&EVAL_LAYOUT_VERSION.to_le_bytes());
    bytes.extend_from_slice(&(items.len() as u32).to_le_bytes());
    for item in items {
        for cell in item {
            bytes.extend_from_slice(&cell.to_le_bytes());
        }
    }
    bytes
}

/// FNV-1a 64-bit digest over raw ABI output bytes, matching the Go oracle's
/// `hash/fnv` New64a over the same stream.
fn raw_digest(bytes: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for entry in bytes {
        hash ^= u64::from(*entry);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

/// Calls the real exported `mornlea_fluid_eval_batch` symbol with a
/// canary-filled destination, returning the status, the metadata word, and
/// the destination bytes. The caller keeps no aliasing: input, output and
/// metadata are disjoint by construction.
fn call_eval_abi(input: &[u8], output_len: usize) -> (u32, usize, Vec<u8>) {
    let version = unsafe { mornlea_engine_abi_version() };
    let mut output = vec![ABI_CANARY; output_len];
    let mut written = usize::MAX;
    let status = unsafe {
        mornlea_fluid_eval_batch(
            version,
            input.as_ptr(),
            input.len(),
            output.as_mut_ptr(),
            output.len(),
            &mut written,
        )
    };
    (status, written, output)
}

#[test]
fn fluid_eval_abi_4097() {
    // The 6-item rule matrix pins the full 72-byte output stream through the
    // real exported symbol: byte-identical to the typed provider's encoded
    // records, with the frozen stream digest shared with the Go oracle.
    let matrix = rule_matrix();
    let matrix_writes = evaluate(&matrix);
    let mut expected = Vec::with_capacity(matrix.len() * ITEM_OUTPUT_BYTES);
    for writes in &matrix_writes {
        expected.extend_from_slice(&encode_item(writes));
    }
    assert_eq!(expected.len(), 72);
    assert_eq!(record_digest(&matrix_writes), 0x84ac_a3f0_db8d_0ae2);
    assert_eq!(raw_digest(&expected), 0x84ac_a3f0_db8d_0ae2);

    let input = eval_abi_input(&matrix);
    let (status, written, output) = call_eval_abi(&input, expected.len());
    assert_eq!(status, EVAL_STATUS_OK);
    assert_eq!(written, expected.len());
    assert_eq!(output, expected);

    // The legacy ABI admits a 4097-record batch although one native call
    // rejects 4097: the adapter must chunk (at most 4096 per native call)
    // and publish the full stream only after every chunk succeeds. The
    // assertion cost stays bounded with a digest plus spot records.
    let big: Vec<[u16; 7]> = (0..MAX_BATCH_ITEMS + 1)
        .map(|index| matrix[index % matrix.len()])
        .collect();
    // The expectation stages through the bounded native core in chunks, the
    // same shape the adapter must take: one native call rejects 4097.
    let mut big_writes = vec![FluidWrites::default(); big.len()];
    let mut staged = 0;
    for chunk in big.chunks(MAX_BATCH_ITEMS) {
        let dst = &mut big_writes[staged..staged + chunk.len()];
        let written = NativeFluidEval
            .evaluate(chunk, dst)
            .expect("chunked batch must be admitted");
        assert_eq!(written, chunk.len());
        staged += chunk.len();
    }
    assert_eq!(staged, big.len());
    let mut big_expected = Vec::with_capacity(big.len() * ITEM_OUTPUT_BYTES);
    for writes in &big_writes {
        big_expected.extend_from_slice(&encode_item(writes));
    }
    assert_eq!(
        big_expected.len(),
        (MAX_BATCH_ITEMS + 1) * ITEM_OUTPUT_BYTES
    );
    let big_digest = raw_digest(&big_expected);
    let big_input = eval_abi_input(&big);
    let (status, written, big_output) = call_eval_abi(&big_input, big_expected.len());
    assert_eq!(status, EVAL_STATUS_OK);
    assert_eq!(written, big_expected.len());
    assert_eq!(raw_digest(&big_output), big_digest);
    assert_eq!(big_output.len(), big_expected.len());
    // Spot records prove the stream is ordered end to end, not just
    // digest-equal: first, middle, and last 12-byte records.
    for spot in [0, big.len() / 2, big.len() - 1] {
        let base = spot * ITEM_OUTPUT_BYTES;
        assert_eq!(
            &big_output[base..base + ITEM_OUTPUT_BYTES],
            &big_expected[base..base + ITEM_OUTPUT_BYTES],
            "spot record {spot}"
        );
    }
    assert_eq!(big_output, big_expected);

    // A one-byte-short destination is an invalid argument (status 2, never
    // the two-phase overflow probe): output and metadata stay canaried.
    let short_before = vec![ABI_CANARY; expected.len() - 1];
    let (status, written, short) = call_eval_abi(&input, expected.len() - 1);
    assert_eq!(status, EVAL_STATUS_INVALID_ARGUMENT);
    assert_eq!(written, 0);
    assert_eq!(short, short_before);

    // A count/length mismatch is malformed input (status 3) with the same
    // atomicity: output and metadata stay canaried.
    let mut malformed = input.clone();
    malformed.pop();
    let malformed_before = vec![ABI_CANARY; expected.len()];
    let (status, written, untouched) = call_eval_abi(&malformed, expected.len());
    assert_eq!(status, EVAL_STATUS_INPUT);
    assert_eq!(written, 0);
    assert_eq!(untouched, malformed_before);
}
