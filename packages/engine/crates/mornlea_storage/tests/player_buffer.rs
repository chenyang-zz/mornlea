use std::fs;
use std::path::PathBuf;

use mornlea_storage::{
    Inventory, ItemStack, PLAYER_ENVELOPE_LENGTH, PLAYER_MAX_PAYLOAD, PlayerId, PlayerLocation,
    PlayerSave, StorageError, decode_player, encode_player, encode_player_into, player_encoded_len,
};

fn read_go_fixture(relative: &str) -> Vec<u8> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../../../packages/")
        .join(relative);
    fs::read(&path).unwrap_or_else(|err| panic!("read {}: {err}", path.display()))
}

fn player_id() -> PlayerId {
    PlayerId::from_bytes([
        0x00, 0x11, 0x22, 0x33, 0x44, 0x55, 0x46, 0x77, 0x88, 0x99, 0xaa, 0xbb, 0xcc, 0xdd, 0xee,
        0xff,
    ])
}

fn valid_save_with_armor() -> PlayerSave {
    let mut save = PlayerSave {
        player_id: player_id(),
        revision: 42,
        display_name: "Tester".to_owned(),
        current: PlayerLocation {
            dimension: 0,
            position: [1.0, 64.0, 2.0],
        },
        yaw: 0.5,
        pitch: 0.25,
        safe: None,
        inventory: Inventory::default(),
        health: 20,
        hunger: 20,
        saturation_milli: 5_000,
        exhaustion_milli: 0,
        respawn_present: false,
        respawn_position: [0.0; 3],
        respawn_dimension: 0,
        armor: [
            ItemStack {
                item: 4242,
                count: 65,
                durability: 999,
            },
            ItemStack::default(),
            ItemStack::default(),
            ItemStack::default(),
        ],
    };
    save.inventory.hotbar.slots[0] = ItemStack {
        item: 1,
        count: 1,
        durability: 0,
    };
    save
}

#[test]
fn golden_player_v9_fixture_matches_bounded_writer() {
    let golden = read_go_fixture("server/storage/player/testdata/player-v9.bin");
    let decoded = decode_player(player_id(), &golden).expect("decode fixture");
    let save = PlayerSave {
        player_id: decoded.player_id,
        revision: decoded.revision,
        display_name: decoded.display_name,
        current: decoded.current,
        yaw: decoded.yaw,
        pitch: decoded.pitch,
        safe: decoded.safe,
        inventory: decoded.inventory,
        health: decoded.health,
        hunger: decoded.hunger,
        saturation_milli: decoded.saturation_milli,
        exhaustion_milli: decoded.exhaustion_milli,
        respawn_present: decoded.respawn_present,
        respawn_position: decoded.respawn_position,
        respawn_dimension: decoded.respawn_dimension,
        armor: decoded.armor,
    };
    assert_eq!(player_encoded_len(&save).expect("length"), golden.len());
    let mut buf = vec![0u8; golden.len()];
    let written = encode_player_into(&save, &mut buf).expect("encode_into");
    assert_eq!(written, golden.len());
    assert_eq!(&buf, golden.as_slice());
    assert_eq!(encode_player(&save).expect("encode"), golden);
}

#[test]
fn fresh_save_armor_tail_and_input_unchanged() {
    let save = valid_save_with_armor();
    let before = save.clone();
    let n = player_encoded_len(&save).expect("length");
    let mut buf = vec![0u8; n];
    encode_player_into(&save, &mut buf).expect("encode_into");
    let encoded = encode_player(&save).expect("encode");
    assert_eq!(&buf[..n], encoded.as_slice());
    assert_eq!(save, before);
    let payload = &buf[PLAYER_ENVELOPE_LENGTH..];
    let tail = &payload[payload.len() - 20..];
    assert_eq!(tail[0..5], [0x92, 0x10, 65, 0xE7, 0x03]);
    assert_eq!(tail[5..10], [0, 0, 0, 0, 0]);
    assert_eq!(tail[10..15], [0, 0, 0, 0, 0]);
    assert_eq!(tail[15..20], [0, 0, 0, 0, 0]);
}

#[test]
fn buffer_canaries_respect_capacity() {
    let save = valid_save_with_armor();
    let n = player_encoded_len(&save).expect("length");
    let expected = encode_player(&save).expect("encode");

    let mut short = vec![0xA5; n - 1];
    assert_eq!(
        encode_player_into(&save, &mut short),
        Err(StorageError::OutputTooSmall {
            needed: n,
            available: n - 1,
        })
    );
    assert!(short.iter().all(|&b| b == 0xA5));

    let mut exact = vec![0xA5; n];
    assert_eq!(encode_player_into(&save, &mut exact).expect("exact"), n);
    assert_eq!(&exact, expected.as_slice());

    let mut larger = vec![0xA5; n + 7];
    assert_eq!(encode_player_into(&save, &mut larger).expect("larger"), n);
    assert_eq!(&larger[..n], expected.as_slice());
    assert!(larger[n..].iter().all(|&b| b == 0xA5));
}

#[test]
fn corrupt_inputs_leave_canary_buffer_untouched() {
    let base = valid_save_with_armor();
    let n = player_encoded_len(&base).expect("base length");

    let mut invalid_id = base.clone();
    invalid_id.player_id = PlayerId::from_bytes([0; 16]);
    let mut buf = vec![0xA5; n - 1];
    assert!(matches!(
        encode_player_into(&invalid_id, &mut buf),
        Err(StorageError::Corrupt { .. })
    ));
    assert!(buf.iter().all(|&b| b == 0xA5));

    let mut zero_revision = base.clone();
    zero_revision.revision = 0;
    let mut buf = vec![0xA5; n - 1];
    assert!(matches!(
        encode_player_into(&zero_revision, &mut buf),
        Err(StorageError::Corrupt { .. })
    ));
    assert!(buf.iter().all(|&b| b == 0xA5));

    let mut bad_health = base.clone();
    bad_health.health = 21;
    let mut buf = vec![0xA5; n - 1];
    assert!(matches!(
        encode_player_into(&bad_health, &mut buf),
        Err(StorageError::Corrupt { .. })
    ));
    assert!(buf.iter().all(|&b| b == 0xA5));

    let mut bad_saturation = base.clone();
    bad_saturation.hunger = 20;
    bad_saturation.saturation_milli = 20_001;
    let mut buf = vec![0xA5; n - 1];
    assert!(matches!(
        encode_player_into(&bad_saturation, &mut buf),
        Err(StorageError::Corrupt { .. })
    ));
    assert!(buf.iter().all(|&b| b == 0xA5));

    let mut bad_pitch = base.clone();
    bad_pitch.pitch = std::f32::consts::FRAC_PI_2 + 0.01;
    let mut buf = vec![0xA5; n - 1];
    assert!(matches!(
        encode_player_into(&bad_pitch, &mut buf),
        Err(StorageError::Corrupt { .. })
    ));
    assert!(buf.iter().all(|&b| b == 0xA5));
}

#[test]
fn absent_respawn_zeroes_dirty_tail_fields() {
    let mut clean = valid_save_with_armor();
    clean.respawn_present = false;
    clean.respawn_position = [0.0; 3];
    clean.respawn_dimension = 0;

    let mut dirty = clean.clone();
    dirty.respawn_position = [9.0, 8.0, 7.0];
    dirty.respawn_dimension = 1;

    let a = encode_player(&clean).expect("clean");
    let b = encode_player(&dirty).expect("dirty");
    assert_eq!(a, b);
}

#[test]
fn exhaustion_milli_max_encodes() {
    let mut save = valid_save_with_armor();
    save.exhaustion_milli = 65535;
    assert!(encode_player(&save).is_ok());
    assert!(encode_player_into(&save, &mut vec![0; player_encoded_len(&save).unwrap()]).is_ok());
}

#[test]
fn invalid_inventory_slot_rejects_without_mutating_save() {
    let mut save = valid_save_with_armor();
    save.inventory.backpack[0] = ItemStack {
        item: 66,
        count: 1,
        durability: 0,
    };
    let before = save.clone();
    assert!(matches!(
        encode_player(&save),
        Err(StorageError::Corrupt { .. })
    ));
    assert_eq!(save, before);
}

#[test]
fn maximal_constructed_save_stays_below_max_payload() {
    let name = "\u{10000}".repeat(32);
    let save = PlayerSave {
        player_id: player_id(),
        revision: 1,
        display_name: name,
        current: PlayerLocation {
            dimension: 0,
            position: [0.0, 64.0, 0.0],
        },
        yaw: 0.0,
        pitch: 0.0,
        safe: Some(PlayerLocation {
            dimension: 0,
            position: [1.0, 65.0, 1.0],
        }),
        inventory: Inventory::default(),
        health: 20,
        hunger: 20,
        saturation_milli: 5_000,
        exhaustion_milli: 0,
        respawn_present: false,
        respawn_position: [0.0; 3],
        respawn_dimension: 0,
        armor: [ItemStack::default(); 4],
    };
    let len = player_encoded_len(&save).expect("length");
    assert!(len < PLAYER_ENVELOPE_LENGTH + PLAYER_MAX_PAYLOAD);
}
