//! Current chunk encoders must apply the same aggregate validator as decode.

mod support;

use mornlea_storage::{
    ContainerSnapshot, DropSlot, StorageError, StorageKind, decode_chunk, encode_chunk,
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
