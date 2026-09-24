//! Current chunk encoders must apply the same aggregate validator as decode.

mod support;

use mornlea_storage::{
    ContainerSnapshot, DropSlot, ItemStack, StorageError, StorageKind, decode_chunk,
    decode_chunk_logical, encode_chunk, encode_chunk_at_schema, encode_chunk_logical,
};
use support::chunk_safety::{
    LAST_BLOCK_INDEX, activate_chest, activate_furnace, empty_save, set_section_block,
};

fn assert_corrupt(save: &mornlea_storage::ChunkSave) {
    match encode_chunk(save) {
        Err(StorageError::Corrupt(_)) => {}
        other => panic!("expected corrupt, got {other:?}"),
    }
}

fn assert_round_trip(save: &mornlea_storage::ChunkSave) {
    let encoded = encode_chunk(save).expect("valid chunk encodes");
    let decoded = decode_chunk(save.key, save.revision, &encoded).expect("valid chunk decodes");
    assert_eq!(decoded.key, save.key);
    assert_eq!(decoded.revision, save.revision);
    assert_eq!(decoded.chunk, save.chunk);
}

#[test]
fn current_encoder_rejects_unreadable_aggregates_and_keeps_valid_ones() {
    let mut furnace_on_air = empty_save();
    activate_furnace(&mut furnace_on_air, 0, 0);
    assert_corrupt(&furnace_on_air);

    let mut chest_on_air = empty_save();
    activate_chest(&mut chest_on_air, 0, 0);
    assert_corrupt(&chest_on_air);

    let mut furnace_on_chest = empty_save();
    set_section_block(&mut furnace_on_chest, 0, 11);
    activate_furnace(&mut furnace_on_chest, 0, 0);
    assert_corrupt(&furnace_on_chest);

    let mut chest_on_furnace = empty_save();
    set_section_block(&mut chest_on_furnace, 0, 9);
    activate_chest(&mut chest_on_furnace, 0, 0);
    assert_corrupt(&chest_on_furnace);

    let mut duplicate_furnaces = empty_save();
    set_section_block(&mut duplicate_furnaces, 0, 9);
    activate_furnace(&mut duplicate_furnaces, 0, 0);
    activate_furnace(&mut duplicate_furnaces, 1, 0);
    assert_corrupt(&duplicate_furnaces);

    let mut duplicate_chests = empty_save();
    set_section_block(&mut duplicate_chests, 0, 11);
    activate_chest(&mut duplicate_chests, 0, 0);
    activate_chest(&mut duplicate_chests, 1, 0);
    assert_corrupt(&duplicate_chests);

    let mut outside = empty_save();
    set_section_block(&mut outside, 0, 9);
    activate_furnace(&mut outside, 0, LAST_BLOCK_INDEX + 1);
    assert_corrupt(&outside);

    let mut inactive_shape = empty_save();
    inactive_shape.chunk.furnaces[0].block_index = 1;
    assert_corrupt(&inactive_shape);

    let mut bad_drop = empty_save();
    bad_drop.chunk.drops[31] = DropSlot {
        generation: 0,
        active: true,
        ..DropSlot::default()
    };
    assert_corrupt(&bad_drop);

    for len in [23usize, 25] {
        let mut save = empty_save();
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
        assert_corrupt(&save);
    }
    for len in [31usize, 33] {
        let mut save = empty_save();
        save.chunk.drops.truncate(len.min(32));
        save.chunk.drops.resize(len, DropSlot::default());
        assert_corrupt(&save);
        let mut save = empty_save();
        save.chunk.furnaces.truncate(len.min(32));
        save.chunk.furnaces.resize(len, Default::default());
        assert_corrupt(&save);
    }
    for len in [15usize, 17] {
        let mut save = empty_save();
        save.chunk.chests.truncate(len.min(16));
        save.chunk.chests.resize(len, Default::default());
        assert_corrupt(&save);
    }

    let mut overrun = empty_save();
    overrun.chunk.sections[0] = ContainerSnapshot {
        kind: StorageKind::Indexed,
        bits: 4,
        single: 0,
        palette: vec![0],
        packed: vec![1; 256],
    };
    assert_corrupt(&overrun);

    let mut last = empty_save();
    set_section_block(&mut last, LAST_BLOCK_INDEX, 9);
    activate_furnace(&mut last, 0, LAST_BLOCK_INDEX);
    assert_round_trip(&last);

    let mut mixed = empty_save();
    set_section_block(&mut mixed, 0, 9);
    set_section_block(&mut mixed, 4096, 11);
    activate_furnace(&mut mixed, 0, 0);
    activate_chest(&mut mixed, 0, 4096);
    assert_round_trip(&mixed);
}

#[test]
fn logical_encoders_reject_the_same_invalid_aggregates() {
    let mut furnace_on_air = empty_save();
    activate_furnace(&mut furnace_on_air, 0, 0);
    let mut duplicate_chests = empty_save();
    set_section_block(&mut duplicate_chests, 0, 11);
    activate_chest(&mut duplicate_chests, 0, 0);
    activate_chest(&mut duplicate_chests, 1, 0);
    for save in [&furnace_on_air, &duplicate_chests] {
        let before = save.chunk.clone();
        for schema in 1u32..=9 {
            assert_corrupt_logical(save.key, save.revision, &save.chunk, schema);
            assert!(matches!(
                encode_chunk_at_schema(save, schema),
                Err(StorageError::Corrupt(_))
            ));
        }
        assert_eq!(save.chunk, before);
    }

    let valid = empty_save();
    assert!(encode_chunk_logical(valid.key, 0, &valid.chunk, 9).is_err());
    let mut bad_dimension = valid.clone();
    bad_dimension.key.dimension = 2;
    assert!(encode_chunk_logical(bad_dimension.key, 1, &bad_dimension.chunk, 9).is_err());
    assert!(encode_chunk_logical(valid.key, 1, &valid.chunk, 0).is_err());
    assert!(encode_chunk_logical(valid.key, 1, &valid.chunk, 10).is_err());

    let mut last = empty_save();
    set_section_block(&mut last, LAST_BLOCK_INDEX, 9);
    activate_furnace(&mut last, 0, LAST_BLOCK_INDEX);
    let before = last.chunk.clone();
    let encoded =
        encode_chunk_logical(last.key, last.revision, &last.chunk, 9).expect("last index");
    assert_eq!(last.chunk, before);
    let decoded = decode_chunk_logical(last.key, last.revision, 9, &encoded).expect("decode last");
    assert_eq!(decoded, last.chunk);

    for schema in 1u32..=9 {
        let encoded = encode_chunk_logical(valid.key, valid.revision, &valid.chunk, schema)
            .unwrap_or_else(|err| panic!("schema {schema}: {err}"));
        let decoded = decode_chunk_logical(valid.key, valid.revision, schema, &encoded)
            .unwrap_or_else(|err| panic!("decode schema {schema}: {err}"));
        assert_eq!(decoded, valid.chunk, "schema {schema}");
    }
}

fn assert_corrupt_logical(
    key: mornlea_storage::ChunkKey,
    revision: u64,
    chunk: &mornlea_storage::Chunk,
    schema: u32,
) {
    match encode_chunk_logical(key, revision, chunk, schema) {
        Err(StorageError::Corrupt(_)) => {}
        other => panic!("schema {schema}: expected corrupt, got {other:?}"),
    }
}

#[test]
fn logical_legacy_tool_drop_reserializes_without_current_stack_admission() {
    let mut save = empty_save();
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
        let bytes = encode_chunk_logical(save.key, save.revision, &save.chunk, schema)
            .unwrap_or_else(|err| panic!("schema {schema}: {err}"));
        let decoded = decode_chunk_logical(save.key, save.revision, schema, &bytes)
            .unwrap_or_else(|err| panic!("decode schema {schema}: {err}"));
        assert_eq!(decoded, save.chunk);
        assert!(matches!(
            encode_chunk_at_schema(&save, schema),
            Err(StorageError::Corrupt(_))
        ));
    }
}

#[test]
fn logical_historical_schemas_reject_omitted_nondefault_state() {
    let mut drop = empty_save();
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
    let mut furnace = empty_save();
    set_section_block(&mut furnace, 0, 9);
    activate_furnace(&mut furnace, 0, 0);
    let mut chest = empty_save();
    set_section_block(&mut chest, 0, 11);
    activate_chest(&mut chest, 0, 0);
    let mut durability = empty_save();
    durability.chunk.drops[0] = DropSlot {
        stack: ItemStack {
            item: 10,
            count: 1,
            durability: 131,
        },
        ..drop.chunk.drops[0]
    };
    let mut inactive_drop = empty_save();
    inactive_drop.chunk.drops[0].generation = 1;
    let mut inactive_furnace = empty_save();
    inactive_furnace.chunk.furnaces[0].generation = 1;
    let mut inactive_chest = empty_save();
    inactive_chest.chunk.chests[0].generation = 1;
    for (save, schemas) in [
        (&drop, 1..=1),
        (&furnace, 1..=3),
        (&chest, 1..=5),
        (&durability, 2..=4),
        (&inactive_drop, 1..=1),
        (&inactive_furnace, 1..=3),
        (&inactive_chest, 1..=5),
    ] {
        for schema in schemas {
            assert_corrupt_logical(save.key, save.revision, &save.chunk, schema);
            assert!(matches!(
                encode_chunk_at_schema(save, schema),
                Err(StorageError::Corrupt(_))
            ));
        }
    }
}

#[test]
fn logical_omitted_drop_precedes_later_container_failure() {
    let mut save = empty_save();
    save.chunk.drops[0].generation = 1;
    activate_furnace(&mut save, 0, 0);
    for result in [
        encode_chunk_logical(save.key, save.revision, &save.chunk, 1),
        encode_chunk_at_schema(&save, 1),
    ] {
        match result {
            Err(StorageError::Corrupt(detail)) => assert!(detail.starts_with("drop slot:")),
            other => panic!("expected drop corruption first, got {other:?}"),
        }
    }
}

#[test]
fn logical_historical_envelope_rejects_inactive_tool_durability_migration() {
    let mut save = empty_save();
    save.chunk.drops[0] = DropSlot {
        generation: 1,
        stack: ItemStack {
            item: 10,
            count: 1,
            durability: 0,
        },
        ..DropSlot::default()
    };
    encode_chunk_at_schema(&save, 9).expect("current inactive drop is valid");
    for schema in 2..=4 {
        let raw = encode_chunk_logical(save.key, save.revision, &save.chunk, schema)
            .expect("raw historical logical payload reserializes");
        assert_eq!(
            decode_chunk_logical(save.key, save.revision, schema, &raw).unwrap(),
            save.chunk,
        );
        assert!(matches!(
            encode_chunk_at_schema(&save, schema),
            Err(StorageError::Corrupt(_))
        ));
    }
}
