//! Chunk logical length preflight and encode equivalence tests.

mod support;

use mornlea_storage::{
    CHUNK_MAX_DECODED_CHUNK, Chunk, ContainerSnapshot, DropSlot, ItemStack, StorageError,
    StorageKind, chunk_logical_len, chunk_logical_wire_len_for_test, encode_chunk_at_schema,
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
