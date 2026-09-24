//! Chunk logical length preflight and encode equivalence tests.

mod support;

use mornlea_storage::{
    CHUNK_ENVELOPE_LENGTH, CHUNK_MAX_DECODED_CHUNK, Chunk, ChunkCodec, ChunkKey, ChunkSave,
    ContainerSnapshot, DropSlot, ItemStack, StorageError, StorageKind, chunk_logical_len,
    chunk_logical_wire_len_for_test, decode_chunk, encode_chunk, encode_chunk_at_schema,
    encode_chunk_logical,
};
use support::chunk_safety::{activate_chest, activate_furnace, empty_save, set_section_block};

const SECTION_CELLS: usize = 4096;

fn packed_words(bits: u8, slots: &[(usize, u64)]) -> Vec<u64> {
    let per_word = 64 / bits as usize;
    let mut words = vec![0u64; SECTION_CELLS.div_ceil(per_word)];
    for &(index, slot) in slots {
        words[index / per_word] |= slot << ((index % per_word) * bits as usize);
    }
    words
}

fn all_air_save() -> mornlea_storage::ChunkSave {
    empty_save()
}

fn worst_indexed_palette_save() -> mornlea_storage::ChunkSave {
    let mut save = empty_save();
    let palette8: Vec<u16> = (0..90).collect();
    let packed8 = packed_words(8, &[(0, 89), (100, 18), (SECTION_CELLS - 1, 0)]);
    save.chunk.sections[0] = ContainerSnapshot {
        kind: StorageKind::Indexed,
        bits: 8,
        single: 0,
        palette: palette8,
        packed: packed8,
    };
    save
}

fn assert_len_matches_encode(save: &mornlea_storage::ChunkSave, schema: u32) {
    let len = chunk_logical_len(save, schema)
        .unwrap_or_else(|err| panic!("schema {schema} preflight: {err}"));
    let bytes = encode_chunk_logical(save.key, save.revision, &save.chunk, schema)
        .unwrap_or_else(|err| panic!("schema {schema} encode_logical: {err}"));
    assert_eq!(len, bytes.len(), "schema {schema}");
}

fn assert_preflight_corrupt(save: &mornlea_storage::ChunkSave, schema: u32) {
    match chunk_logical_len(save, schema) {
        Err(StorageError::Corrupt(_)) => {}
        other => panic!("schema {schema}: expected corrupt preflight, got {other:?}"),
    }
}

#[test]
fn logical_air_chunk_length_matches_encode_for_all_schemas() {
    let save = all_air_save();
    for schema in 1u32..=9 {
        assert_len_matches_encode(&save, schema);
    }
}

#[test]
fn logical_worst_indexed_palette_length_matches_encode_for_all_schemas() {
    let save = worst_indexed_palette_save();
    for schema in 1u32..=9 {
        assert_len_matches_encode(&save, schema);
    }
}

#[test]
fn logical_rejects_unsupported_schema_and_revision_zero() {
    let save = all_air_save();
    assert_preflight_corrupt(&save, 0);
    assert_preflight_corrupt(&save, 10);
    let zero = mornlea_storage::ChunkSave {
        revision: 0,
        ..save.clone()
    };
    for schema in 1..=9 {
        assert_preflight_corrupt(&zero, schema);
    }
}

#[test]
fn logical_rejects_invalid_dimension() {
    let mut save = all_air_save();
    save.key.dimension = 2;
    for schema in 1..=9 {
        assert_preflight_corrupt(&save, schema);
    }
}

#[test]
fn logical_rejects_wrong_section_count() {
    for len in [23usize, 25] {
        let mut save = all_air_save();
        save.chunk.sections.truncate(len.min(24));
        while save.chunk.sections.len() < len {
            save.chunk.sections.push(ContainerSnapshot {
                kind: StorageKind::Single,
                bits: 0,
                single: 0,
                palette: Vec::new(),
                packed: Vec::new(),
            });
        }
        for schema in 1..=9 {
            assert_preflight_corrupt(&save, schema);
        }
    }
}

#[test]
fn logical_rejects_invalid_palette() {
    let mut save = all_air_save();
    save.chunk.sections[0] = ContainerSnapshot {
        kind: StorageKind::Indexed,
        bits: 4,
        single: 0,
        palette: vec![0],
        packed: vec![1; 256],
    };
    for schema in 1..=9 {
        assert_preflight_corrupt(&save, schema);
    }
}

#[test]
fn logical_rejects_active_furnace_on_air() {
    let mut save = all_air_save();
    activate_furnace(&mut save, 0, 0);
    for schema in 1..=9 {
        assert_preflight_corrupt(&save, schema);
    }
}

#[test]
fn logical_rejects_omitted_nondefault_state() {
    let mut drop = all_air_save();
    drop.chunk.drops[0] = DropSlot {
        generation: 1,
        active: true,
        stack: ItemStack {
            item: 1,
            count: 1,
            durability: 0,
        },
        ..DropSlot::default()
    };
    let mut furnace = all_air_save();
    set_section_block(&mut furnace, 0, 9);
    activate_furnace(&mut furnace, 0, 0);
    let mut chest = all_air_save();
    set_section_block(&mut chest, 0, 11);
    activate_chest(&mut chest, 0, 0);
    for (save, schemas) in [(&drop, 1..=1), (&furnace, 1..=3), (&chest, 1..=5)] {
        for schema in schemas {
            assert_preflight_corrupt(save, schema);
        }
    }
}

#[test]
fn logical_rejects_v4_mutable_inactive_tool_drop() {
    let mut save = all_air_save();
    save.chunk.drops[0] = DropSlot {
        generation: 1,
        stack: ItemStack {
            item: 10,
            count: 1,
            durability: 0,
        },
        ..DropSlot::default()
    };
    encode_chunk_at_schema(&save, 9).expect("current schema accepts inactive tool");
    for schema in 2..=4 {
        assert_preflight_corrupt(&save, schema);
    }
}

#[test]
fn logical_legacy_multi_item_tool_drop_encodes_but_preflight_fails() {
    let mut save = all_air_save();
    save.chunk.drops[0] = DropSlot {
        generation: 1,
        active: true,
        stack: ItemStack {
            item: 10,
            count: 2,
            durability: 0,
        },
        ..DropSlot::default()
    };
    for schema in 2..=4 {
        encode_chunk_logical(save.key, save.revision, &save.chunk, schema)
            .expect("raw logical reserializes");
        assert_preflight_corrupt(&save, schema);
        assert!(matches!(
            encode_chunk_at_schema(&save, schema),
            Err(StorageError::Corrupt(_))
        ));
    }
}

#[test]
fn logical_worst_legal_payload_stays_below_decoded_cap() {
    let save = worst_indexed_palette_save();
    for schema in 1..=9 {
        let len = chunk_logical_len(&save, schema).expect("worst legal representable");
        assert!(len < CHUNK_MAX_DECODED_CHUNK);
    }
}

#[test]
fn logical_preflight_rejects_wire_length_above_decoded_cap() {
    let huge_palette = vec![0u16; (CHUNK_MAX_DECODED_CHUNK / 2).max(1)];
    let chunk = Chunk {
        sections: vec![
            ContainerSnapshot {
                kind: StorageKind::Single,
                bits: 0,
                single: 0,
                palette: huge_palette,
                packed: Vec::new(),
            };
            24
        ],
        drops: vec![DropSlot::default(); 32],
        furnaces: vec![Default::default(); 32],
        chests: vec![Default::default(); 16],
    };
    match chunk_logical_wire_len_for_test(&chunk, 1) {
        Err(StorageError::Corrupt(detail)) => {
            assert!(detail.contains("exceeds limit") || detail.contains("overflow"));
        }
        other => panic!("expected corrupt cap or overflow, got {other:?}"),
    }
}

#[test]
fn logical_air_lengths_follow_versioned_slot_layout() {
    let save = all_air_save();
    let section_bytes = 16 * 24;
    let expected = [
        (1u32, 32 + section_bytes),
        (2, 32 + section_bytes + 32 * 17),
        (3, 32 + section_bytes + 32 * 17),
        (4, 32 + section_bytes + 32 * 17 + 32 * 21),
        (5, 32 + section_bytes + 32 * 19 + 32 * 21),
        (6, 32 + section_bytes + 32 * 19 + 32 * 21 + 16 * 144),
        (7, 32 + section_bytes + 32 * 19 + 32 * 21 + 16 * 144),
        (8, 32 + section_bytes + 32 * 19 + 32 * 21 + 16 * 144),
        (9, 32 + section_bytes + 32 * 19 + 32 * 21 + 16 * 144),
    ];
    for (schema, want) in expected {
        let len = chunk_logical_len(&save, schema).expect("air chunk");
        assert_eq!(len, want, "schema {schema}");
        assert_len_matches_encode(&save, schema);
    }
}

fn assert_frame_header(frame: &[u8], declared_decoded_length: &[u8]) {
    assert_eq!(&frame[..4], &[0x28, 0xB5, 0x2F, 0xFD], "zstd frame magic");
    assert_eq!(frame[4], 0x64, "frame header descriptor");
    let declared = u32::from_le_bytes(declared_decoded_length.try_into().expect("four bytes"));
    let content = u32::from(u16::from_le_bytes([frame[5], frame[6]])) + 256;
    assert_eq!(content, declared, "frame content size");
}

fn encode_reference(save: &ChunkSave) -> Vec<u8> {
    encode_chunk(save).expect("free encode")
}

#[test]
fn context_valid_v9_fresh_and_reused_matches_free_api() {
    let save = empty_save();
    let expected = encode_reference(&save);
    let decoded_free = decode_chunk(save.key, save.revision, &expected).expect("free decode");

    let mut fresh = ChunkCodec::try_new().expect("context");
    let mut buf = vec![0u8; expected.len() + 16];
    let written = fresh.encode_into(&save, &mut buf).expect("encode fresh");
    assert_eq!(written, expected.len());
    assert_eq!(&buf[..written], expected.as_slice());
    assert_frame_header(&buf[CHUNK_ENVELOPE_LENGTH..], &buf[36..40]);

    let decoded_fresh = fresh
        .decode(save.key, save.revision, &buf[..written])
        .expect("decode fresh");
    assert_eq!(decoded_fresh, decoded_free);

    let written_reuse = fresh
        .encode_into(&save, &mut buf[..written + 8])
        .expect("encode reuse");
    assert_eq!(written_reuse, expected.len());
    assert_eq!(&buf[..written_reuse], expected.as_slice());
    let decoded_reuse = fresh
        .decode(save.key, save.revision, &buf[..written_reuse])
        .expect("decode reuse");
    assert_eq!(decoded_reuse, decoded_free);
}

#[test]
fn context_buffer_canaries_respect_capacity() {
    let save = empty_save();
    let expected = encode_reference(&save);
    let n = expected.len();
    let mut codec = ChunkCodec::try_new().expect("context");

    let mut short = vec![0xA5; n - 1];
    assert_eq!(
        codec.encode_into(&save, &mut short),
        Err(StorageError::OutputTooSmall {
            needed: n,
            available: n - 1,
        })
    );
    assert!(short.iter().all(|&b| b == 0xA5));

    let mut exact = vec![0xA5; n];
    assert_eq!(codec.encode_into(&save, &mut exact).expect("exact"), n);
    assert_eq!(&exact[..n], expected.as_slice());

    let mut larger = vec![0xA5; n + 7];
    assert_eq!(codec.encode_into(&save, &mut larger).expect("larger"), n);
    assert_eq!(&larger[..n], expected.as_slice());
    assert!(larger[n..].iter().all(|&b| b == 0xA5));
}

#[test]
fn context_invalid_aggregate_and_short_buffer_reports_corruption_first() {
    let mut invalid = empty_save();
    activate_furnace(&mut invalid, 0, 0);
    let n = encode_reference(&empty_save()).len();
    let mut codec = ChunkCodec::try_new().expect("context");
    let mut buf = vec![0xA5; n - 1];
    assert!(matches!(
        codec.encode_into(&invalid, &mut buf),
        Err(StorageError::Corrupt(_))
    ));
    assert!(buf.iter().all(|&b| b == 0xA5));
}

#[test]
fn context_decode_rejects_malformed_envelopes_without_value() {
    let save = empty_save();
    let valid = encode_reference(&save);
    let mut codec = ChunkCodec::try_new().expect("context");

    let wrong_key = ChunkKey {
        dimension: 1,
        ..save.key
    };
    assert!(codec.decode(wrong_key, save.revision, &valid).is_err());

    assert!(codec.decode(save.key, save.revision + 1, &valid).is_err());

    let mut future = valid.clone();
    future[8..12].copy_from_slice(&10u32.to_le_bytes());
    assert!(matches!(
        codec.decode(save.key, save.revision, &future),
        Err(StorageError::FutureVersion(_))
    ));

    let mut huge_compressed = valid.clone();
    huge_compressed[40..44].copy_from_slice(&(((1 << 20) + 1) as u32).to_le_bytes());
    assert!(
        codec
            .decode(save.key, save.revision, &huge_compressed)
            .is_err()
    );

    let mut huge_decoded = valid.clone();
    huge_decoded[36..40].copy_from_slice(&((CHUNK_MAX_DECODED_CHUNK + 1) as u32).to_le_bytes());
    assert!(
        codec
            .decode(save.key, save.revision, &huge_decoded)
            .is_err()
    );

    let truncated = valid[..valid.len().saturating_sub(3)].to_vec();
    assert!(codec.decode(save.key, save.revision, &truncated).is_err());

    let mut bad_checksum = valid.clone();
    *bad_checksum.last_mut().expect("frame tail") ^= 0xff;
    assert!(
        codec
            .decode(save.key, save.revision, &bad_checksum)
            .is_err()
    );

    let bomb_payload = vec![0u8; CHUNK_MAX_DECODED_CHUNK + 1];
    let bomb_frame = zstd::bulk::compress(&bomb_payload, 3).expect("test bomb frame");
    let mut bomb_envelope = valid.clone();
    bomb_envelope[36..40].copy_from_slice(&(bomb_payload.len() as u32).to_le_bytes());
    bomb_envelope[40..44].copy_from_slice(&(bomb_frame.len() as u32).to_le_bytes());
    bomb_envelope.truncate(CHUNK_ENVELOPE_LENGTH);
    bomb_envelope.extend_from_slice(&bomb_frame);
    assert!(
        codec
            .decode(save.key, save.revision, &bomb_envelope)
            .is_err()
    );
}

#[test]
fn context_recovers_after_failures_like_a_fresh_context() {
    let save = empty_save();
    let valid = encode_reference(&save);
    let mut codec = ChunkCodec::try_new().expect("context");

    let mut bad_checksum = valid.clone();
    *bad_checksum.last_mut().expect("tail") ^= 0xff;
    assert!(
        codec
            .decode(save.key, save.revision, &bad_checksum)
            .is_err()
    );

    let mut wrong_key = empty_save();
    wrong_key.key.dimension = 2;
    assert!(
        codec
            .encode_into(&wrong_key, &mut vec![0u8; valid.len()])
            .is_err()
    );

    let mut buf = vec![0u8; valid.len()];
    codec.encode_into(&save, &mut buf).expect("after failures");
    assert_eq!(&buf, valid.as_slice());

    let mut fresh = ChunkCodec::try_new().expect("fresh");
    let mut fresh_buf = vec![0u8; valid.len()];
    fresh
        .encode_into(&save, &mut fresh_buf)
        .expect("fresh encode");
    assert_eq!(buf, fresh_buf);
}

#[test]
fn context_two_independent_codecs_interleaved_share_no_state() {
    let save_a = empty_save();
    let mut save_b = empty_save();
    save_b.key.x = 4;
    save_b.revision = 2;
    save_b.chunk.sections[0].single = 3;

    let mut a = ChunkCodec::try_new().expect("a");
    let mut b = ChunkCodec::try_new().expect("b");
    let len_a = encode_reference(&save_a).len();
    let len_b = encode_reference(&save_b).len();
    let mut buf_a = vec![0u8; len_a];
    let mut buf_b = vec![0u8; len_b];

    a.encode_into(&save_a, &mut buf_a).expect("a encode");
    b.encode_into(&save_b, &mut buf_b).expect("b encode");
    let decoded_b = b
        .decode(save_b.key, save_b.revision, &buf_b)
        .expect("b decode");
    let decoded_a = a
        .decode(save_a.key, save_a.revision, &buf_a)
        .expect("a decode");

    assert_eq!(
        decoded_a,
        decode_chunk(save_a.key, save_a.revision, &buf_a).expect("free a")
    );
    assert_eq!(
        decoded_b,
        decode_chunk(save_b.key, save_b.revision, &buf_b).expect("free b")
    );
}
