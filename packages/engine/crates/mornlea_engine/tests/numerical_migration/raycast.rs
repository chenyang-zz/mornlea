use std::mem::size_of;

use mornlea_engine::native::contracts::raycast::{Ray, RayCursor, RaycastOp};
use mornlea_engine::native::raycast::NativeRaycast;

unsafe extern "C" {
    fn mornlea_raycast_batch(
        abi_version: u32,
        input: *const u8,
        input_len: usize,
        cursor: *mut u8,
        cursor_len: usize,
        output: *mut u8,
        output_len: usize,
        output_count: *mut usize,
        done: *mut u8,
    ) -> u32;
    fn mornlea_engine_abi_version() -> u32;
}

const ABI_INPUT_BYTES: usize = 40;
const ABI_CURSOR_BYTES: usize = 64;
const ABI_RECORD_BYTES: usize = 20;
const ABI_RECORD_CAPACITY: usize = 64;
const ABI_OUTPUT_BYTES: usize = 1280;
const CANARY_BYTE: u8 = 0xA5;

#[test]
fn test_numerical_migration() {
    let ray = Ray {
        origin: [0.5, 0.5, 0.5],
        direction: [1.0, 0.0, 0.0],
        maximum: 70.0,
    };
    let mut cursor = RayCursor::try_new(ray).unwrap();
    let provider = NativeRaycast;

    let batch1 = provider.next_batch(&mut cursor).unwrap();
    let batch2 = provider.next_batch(&mut cursor).unwrap();

    assert_eq!(batch1.records().len(), 64);
    assert_eq!(batch2.records().len(), 7);

    // Exact parity check for specific records
    let r0 = batch1.records()[0];
    assert_eq!(r0.cell, [0, 0, 0]);
    assert_eq!(r0.distance.to_bits(), 0.0_f32.to_bits());

    let r1 = batch1.records()[1];
    assert_eq!(r1.cell, [1, 0, 0]);
    assert_eq!(r1.distance.to_bits(), 0.5_f32.to_bits());

    let r63 = batch1.records()[63];
    assert_eq!(r63.cell, [63, 0, 0]);
    assert_eq!(r63.distance.to_bits(), 62.5_f32.to_bits());

    let r64 = batch2.records()[0];
    assert_eq!(r64.cell, [64, 0, 0]);
    assert_eq!(r64.distance.to_bits(), 63.5_f32.to_bits());

    let r70 = batch2.records()[6];
    assert_eq!(r70.cell, [70, 0, 0]);
    assert_eq!(r70.distance.to_bits(), 69.5_f32.to_bits());

    assert!(batch2.is_done());
}

/// Minimal raw `MGR1` encoder: mirrors the `MGR1` field layout of the
/// `raycast_input_is_valid` admission structurally (origin at 8/12/16,
/// direction at 20/24/28, maximum at 32, zeroed tail at 36..40).
fn raw_raycast_input(origin: [f32; 3], direction: [f32; 3], maximum: f32) -> [u8; ABI_INPUT_BYTES] {
    let mut input = [0_u8; ABI_INPUT_BYTES];
    input[0..4].copy_from_slice(b"MGR1");
    input[4..8].copy_from_slice(&1_u32.to_le_bytes());
    for (index, value) in origin.into_iter().enumerate() {
        input[8 + index * 4..12 + index * 4].copy_from_slice(&value.to_bits().to_le_bytes());
    }
    for (index, value) in direction.into_iter().enumerate() {
        input[20 + index * 4..24 + index * 4].copy_from_slice(&value.to_bits().to_le_bytes());
    }
    input[32..36].copy_from_slice(&maximum.to_bits().to_le_bytes());
    input
}

/// Minimal zeroed `MRC1` cursor encoder: mirrors the fresh-cursor layout of
/// the weak-history admission structurally (magic, version, zeroed words).
fn raw_raycast_cursor() -> [u8; ABI_CURSOR_BYTES] {
    let mut cursor = [0_u8; ABI_CURSOR_BYTES];
    cursor[0..4].copy_from_slice(b"MRC1");
    cursor[4..8].copy_from_slice(&1_u32.to_le_bytes());
    cursor
}

/// Calls the real exported `mornlea_raycast_batch` symbol with canary-padded
/// arenas and full-word metadata sentinels, returning the status.
#[allow(clippy::too_many_arguments)]
fn call_raycast_abi(
    input: &[u8],
    cursor_arena: &mut [u8],
    cursor_offset: usize,
    output_arena: &mut [u8],
    output_offset: usize,
    count: &mut usize,
    done: &mut u8,
) -> u32 {
    let version = unsafe { mornlea_engine_abi_version() };
    unsafe {
        mornlea_raycast_batch(
            version,
            input.as_ptr(),
            input.len(),
            cursor_arena.as_mut_ptr().add(cursor_offset),
            ABI_CURSOR_BYTES,
            output_arena.as_mut_ptr().add(output_offset),
            ABI_OUTPUT_BYTES,
            count,
            done,
        )
    }
}

/// Asserts that the guard bytes around both arenas still hold `CANARY_BYTE`.
fn assert_raycast_guards_intact(cursor_arena: &[u8], output_arena: &[u8]) {
    assert_eq!(cursor_arena[0], CANARY_BYTE, "cursor leading guard");
    assert_eq!(
        cursor_arena[ABI_CURSOR_BYTES + 1],
        CANARY_BYTE,
        "cursor trailing guard"
    );
    assert_eq!(output_arena[0], CANARY_BYTE, "output leading guard");
    assert_eq!(
        output_arena[ABI_OUTPUT_BYTES + 1],
        CANARY_BYTE,
        "output trailing guard"
    );
}

#[test]
fn raycast_abi_cursor_sequence() {
    // The 65-record ray from the accepted 2.3 migration vector: origin
    // [0.5, 0.5, 0.5] along +X with a 70.0 maximum yields 64 records on the
    // first call and the remaining 7 on the second call with `done` set.
    let input = raw_raycast_input([0.5, 0.5, 0.5], [1.0, 0.0, 0.0], 70.0);

    let mut cursor_arena = [CANARY_BYTE; ABI_CURSOR_BYTES + 2];
    cursor_arena[1..1 + ABI_CURSOR_BYTES].copy_from_slice(&raw_raycast_cursor());
    let mut output_arena = [CANARY_BYTE; ABI_OUTPUT_BYTES + 2];
    let mut count = 0xa5a5_a5a5_usize;
    let mut done = 0xa5_u8;

    let status = call_raycast_abi(
        &input,
        &mut cursor_arena,
        1,
        &mut output_arena,
        1,
        &mut count,
        &mut done,
    );
    assert_eq!(status, 0, "first batch status");
    assert_eq!(count, 64, "first batch count");
    assert_eq!(done, 0, "first batch done flag");
    let first_cursor: [u8; ABI_CURSOR_BYTES] =
        cursor_arena[1..1 + ABI_CURSOR_BYTES].try_into().unwrap();
    let first_bytes: [u8; ABI_OUTPUT_BYTES] =
        output_arena[1..1 + ABI_OUTPUT_BYTES].try_into().unwrap();
    assert_raycast_guards_intact(&cursor_arena, &output_arena);
    // First batch: cells [0,0,0] .. [63,0,0]; record 0 is the origin cell
    // with face `0xff` and distance 0.0, later records enter on the -X face
    // (`0`) with distances 0.5, 1.5, ... 62.5.
    for index in 0..ABI_RECORD_CAPACITY {
        let offset = index * ABI_RECORD_BYTES;
        let record = &first_bytes[offset..offset + ABI_RECORD_BYTES];
        let cell = [
            i32::from_le_bytes(record[0..4].try_into().unwrap()),
            i32::from_le_bytes(record[4..8].try_into().unwrap()),
            i32::from_le_bytes(record[8..12].try_into().unwrap()),
        ];
        assert_eq!(cell, [index as i32, 0, 0], "first batch cell {index}");
        let face = record[12];
        let distance = f32::from_bits(u32::from_le_bytes(record[16..20].try_into().unwrap()));
        if index == 0 {
            assert_eq!(face, 0xff, "origin record face");
            assert_eq!(
                distance.to_bits(),
                0.0_f32.to_bits(),
                "origin record distance"
            );
        } else {
            assert_eq!(face, 0, "entry face of record {index}");
            assert_eq!(
                distance.to_bits(),
                ((index as f32) - 0.5).to_bits(),
                "distance of record {index}"
            );
        }
        assert_eq!(
            &record[13..16],
            &[0, 0, 0],
            "face padding of record {index}"
        );
    }
    // Exact cursor bytes after the first call: 64 records (the origin cell
    // plus 63 +X steps) leave the entry cursor at cell [63,0,0] with steps
    // [1,0,0], deltas [1.0, inf, inf], and per-axis maxima [63.5, inf, inf].
    let mut expected_first_cursor = raw_raycast_cursor();
    expected_first_cursor[8] = 1;
    expected_first_cursor[12..16].copy_from_slice(&63_i32.to_le_bytes());
    expected_first_cursor[16..20].copy_from_slice(&0_i32.to_le_bytes());
    expected_first_cursor[20..24].copy_from_slice(&0_i32.to_le_bytes());
    expected_first_cursor[24..28].copy_from_slice(&1_i32.to_le_bytes());
    expected_first_cursor[28..32].copy_from_slice(&0_i32.to_le_bytes());
    expected_first_cursor[32..36].copy_from_slice(&0_i32.to_le_bytes());
    expected_first_cursor[36..40].copy_from_slice(&1.0_f32.to_bits().to_le_bytes());
    expected_first_cursor[40..44].copy_from_slice(&f32::INFINITY.to_bits().to_le_bytes());
    expected_first_cursor[44..48].copy_from_slice(&f32::INFINITY.to_bits().to_le_bytes());
    expected_first_cursor[48..52].copy_from_slice(&63.5_f32.to_bits().to_le_bytes());
    expected_first_cursor[52..56].copy_from_slice(&f32::INFINITY.to_bits().to_le_bytes());
    expected_first_cursor[56..60].copy_from_slice(&f32::INFINITY.to_bits().to_le_bytes());
    assert_eq!(
        first_cursor, expected_first_cursor,
        "first batch full cursor bytes"
    );

    // Second call on the returned cursor yields records [64,0,0] ..
    // [70,0,0] with distances 63.5 .. 69.5 and the `done` flag set.
    let second_input = input;
    let mut second_count = 0xa5a5_a5a5_usize;
    let mut second_done = 0xa5_u8;
    let status = call_raycast_abi(
        &second_input,
        &mut cursor_arena,
        1,
        &mut output_arena,
        1,
        &mut second_count,
        &mut second_done,
    );
    assert_eq!(status, 0, "second batch status");
    assert_eq!(second_count, 7, "second batch count");
    assert_eq!(second_done, 1, "second batch done flag");
    let second_cursor: [u8; ABI_CURSOR_BYTES] =
        cursor_arena[1..1 + ABI_CURSOR_BYTES].try_into().unwrap();
    let second_bytes: [u8; ABI_OUTPUT_BYTES] =
        output_arena[1..1 + ABI_OUTPUT_BYTES].try_into().unwrap();
    assert_raycast_guards_intact(&cursor_arena, &output_arena);
    for index in 0..7 {
        let offset = index * ABI_RECORD_BYTES;
        let record = &second_bytes[offset..offset + ABI_RECORD_BYTES];
        let cell = [
            i32::from_le_bytes(record[0..4].try_into().unwrap()),
            i32::from_le_bytes(record[4..8].try_into().unwrap()),
            i32::from_le_bytes(record[8..12].try_into().unwrap()),
        ];
        assert_eq!(
            cell,
            [(64 + index) as i32, 0, 0],
            "second batch cell {index}"
        );
        assert_eq!(record[12], 0, "second batch face {index}");
        assert_eq!(&record[13..16], &[0, 0, 0], "second batch padding {index}");
        assert_eq!(
            f32::from_bits(u32::from_le_bytes(record[16..20].try_into().unwrap())).to_bits(),
            (63.5 + index as f32).to_bits(),
            "second batch distance {index}"
        );
    }
    // Trailing output past the 7 published records is the zeroed tail of
    // the staged local buffer: the first call's records must not leak
    // through, and the publish copies the full local output.
    assert_eq!(
        &second_bytes[7 * ABI_RECORD_BYTES..],
        &vec![0_u8; ABI_OUTPUT_BYTES - 7 * ABI_RECORD_BYTES][..],
        "second batch trailing output"
    );
    // Exact cursor bytes after the second call: 7 records step the entry
    // cursor to cell [70,0,0], where the next +X distance (70.5) exceeds the
    // 70.0 maximum, so the terminal state 2 cursor keeps cell [70,0,0] with
    // per-axis maxima [70.5, inf, inf].
    let mut expected_second_cursor = raw_raycast_cursor();
    expected_second_cursor[8] = 2;
    expected_second_cursor[12..16].copy_from_slice(&70_i32.to_le_bytes());
    expected_second_cursor[16..20].copy_from_slice(&0_i32.to_le_bytes());
    expected_second_cursor[20..24].copy_from_slice(&0_i32.to_le_bytes());
    expected_second_cursor[24..28].copy_from_slice(&1_i32.to_le_bytes());
    expected_second_cursor[28..32].copy_from_slice(&0_i32.to_le_bytes());
    expected_second_cursor[32..36].copy_from_slice(&0_i32.to_le_bytes());
    expected_second_cursor[36..40].copy_from_slice(&1.0_f32.to_bits().to_le_bytes());
    expected_second_cursor[40..44].copy_from_slice(&f32::INFINITY.to_bits().to_le_bytes());
    expected_second_cursor[44..48].copy_from_slice(&f32::INFINITY.to_bits().to_le_bytes());
    expected_second_cursor[48..52].copy_from_slice(&70.5_f32.to_bits().to_le_bytes());
    expected_second_cursor[52..56].copy_from_slice(&f32::INFINITY.to_bits().to_le_bytes());
    expected_second_cursor[56..60].copy_from_slice(&f32::INFINITY.to_bits().to_le_bytes());
    assert_eq!(
        second_cursor, expected_second_cursor,
        "second batch full cursor bytes"
    );

    // Repeat-done: a third call on the done cursor returns empty done. The
    // cursor arena is preserved; the output arena receives the staged local
    // publish (a zeroed empty batch), like every successful call.
    let done_cursor_before: [u8; ABI_CURSOR_BYTES] =
        cursor_arena[1..1 + ABI_CURSOR_BYTES].try_into().unwrap();
    let mut repeat_count = 0xa5a5_a5a5_usize;
    let mut repeat_done = 0xa5_u8;
    let status = call_raycast_abi(
        &input,
        &mut cursor_arena,
        1,
        &mut output_arena,
        1,
        &mut repeat_count,
        &mut repeat_done,
    );
    assert_eq!(status, 0, "repeat-done status");
    assert_eq!(repeat_count, 0, "repeat-done count");
    assert_eq!(repeat_done, 1, "repeat-done flag");
    assert_eq!(
        &cursor_arena[1..1 + ABI_CURSOR_BYTES],
        &done_cursor_before,
        "repeat-done cursor preserved"
    );
    assert_eq!(
        &output_arena[1..1 + ABI_OUTPUT_BYTES],
        &vec![0_u8; ABI_OUTPUT_BYTES][..],
        "repeat-done output is the staged empty batch"
    );
    assert_raycast_guards_intact(&cursor_arena, &output_arena);

    // Tampered cursor: bad magic keeps status 3 with output/cursor arenas
    // and metadata untouched.
    let mut bad_cursor_arena = [CANARY_BYTE; ABI_CURSOR_BYTES + 2];
    bad_cursor_arena[1..1 + ABI_CURSOR_BYTES].copy_from_slice(&raw_raycast_cursor());
    bad_cursor_arena[1] = b'X';
    let mut bad_output_arena = [CANARY_BYTE; ABI_OUTPUT_BYTES + 2];
    let bad_cursor_before = bad_cursor_arena;
    let bad_output_before = bad_output_arena;
    let mut bad_count = 0xa5a5_a5a5_usize;
    let mut bad_done = 0xa5_u8;
    let status = call_raycast_abi(
        &input,
        &mut bad_cursor_arena,
        1,
        &mut bad_output_arena,
        1,
        &mut bad_count,
        &mut bad_done,
    );
    assert_eq!(status, 3, "tampered cursor status");
    assert_eq!(bad_count, 0, "tampered cursor count cleared");
    assert_eq!(bad_done, 0, "tampered cursor done cleared");
    // Only the leading metadata clear may run before admission fails; the
    // caller arenas keep every byte.
    assert_eq!(bad_cursor_arena, bad_cursor_before, "tampered cursor arena");
    assert_eq!(bad_output_arena, bad_output_before, "tampered output arena");

    // Output/cursor overlap: the aliased arenas keep the existing status 2.
    // Metadata words are cleared before the overlap check (the shared
    // clear-then-validate ordering), while every arena byte is preserved.
    let mut overlap_arena = [CANARY_BYTE; ABI_CURSOR_BYTES + ABI_OUTPUT_BYTES + 2];
    overlap_arena[1..1 + ABI_CURSOR_BYTES].copy_from_slice(&raw_raycast_cursor());
    let overlap_before = overlap_arena;
    let mut overlap_count = usize::from_ne_bytes([0xa5; size_of::<usize>()]);
    let mut overlap_done = 0xa5_u8;
    let version = unsafe { mornlea_engine_abi_version() };
    let status = unsafe {
        mornlea_raycast_batch(
            version,
            input.as_ptr(),
            input.len(),
            overlap_arena.as_mut_ptr().add(1),
            ABI_CURSOR_BYTES,
            overlap_arena.as_mut_ptr().add(2),
            ABI_OUTPUT_BYTES,
            &mut overlap_count,
            &mut overlap_done,
        )
    };
    assert_eq!(status, 2, "overlap status");
    assert_eq!(overlap_arena, overlap_before, "overlap arenas preserved");
    assert_eq!(overlap_count, 0, "overlap count cleared");
    assert_eq!(overlap_done, 0, "overlap done cleared");
}
