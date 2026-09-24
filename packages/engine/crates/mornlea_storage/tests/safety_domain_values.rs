//! Current save values use the domain constructors as their rule source.

use std::fs;
use std::path::PathBuf;

use mornlea_storage::{
    ItemStack, PlayerId, PlayerSave, StorageError, decode_player, encode_player,
};

fn bytes(version: u8, variant: u8, last: u8) -> [u8; 16] {
    let mut raw = [0u8; 16];
    raw[6] = version << 4;
    raw[8] = variant;
    raw[15] = last;
    raw
}

fn canonical() -> PlayerId {
    PlayerId::from_bytes([
        0x6f, 0xce, 0x82, 0x77, 0xa9, 0x33, 0x46, 0xcb, 0x9a, 0x1f, 0xda, 0x13, 0xb7, 0xee, 0x56,
        0x44,
    ])
}

#[test]
fn identity_checked_conversions_match_domain_uuid_v4() {
    let raw = canonical();
    let player = mornlea_storage::checked_player_id(raw).expect("player id");
    let companion = mornlea_storage::checked_companion_id(raw).expect("companion id");
    assert_eq!(player.bytes(), raw.to_bytes());
    assert_eq!(companion.bytes(), raw.to_bytes());

    for raw in [
        PlayerId::from_bytes([0u8; 16]),
        PlayerId::from_bytes(bytes(5, 0x80, 1)),
        PlayerId::from_bytes(bytes(4, 0x00, 1)),
    ] {
        assert!(matches!(
            mornlea_storage::checked_player_id(raw),
            Err(StorageError::Corrupt(_))
        ));
        assert!(matches!(
            mornlea_storage::checked_companion_id(raw),
            Err(StorageError::Corrupt(_))
        ));
        assert_eq!(
            raw.is_valid(),
            mornlea_domain::PlayerId::try_from_bytes(raw.to_bytes()).is_ok()
        );
    }
    assert!(raw_table_agrees());
}

#[test]
fn stack_ordinary_rules_follow_domain_and_armor_stays_raw() {
    let empty = ItemStack::default();
    let checked = mornlea_storage::checked_item_stack(empty).expect("empty stack");
    assert_eq!(checked.item(), 0);
    assert_eq!(checked.count(), 0);
    assert_eq!(checked.durability(), 0);

    assert!(
        mornlea_storage::checked_item_stack(ItemStack {
            item: 1,
            count: 64,
            durability: 0,
        })
        .is_ok()
    );
    assert!(matches!(
        mornlea_storage::checked_item_stack(ItemStack {
            item: 1,
            count: 65,
            durability: 0,
        }),
        Err(StorageError::Corrupt(_))
    ));
    assert!(matches!(
        mornlea_storage::checked_item_stack(ItemStack {
            item: 66,
            count: 1,
            durability: 0,
        }),
        Err(StorageError::Corrupt(_))
    ));
    for durability in [0u16, 132] {
        assert!(matches!(
            mornlea_storage::checked_item_stack(ItemStack {
                item: 10,
                count: 1,
                durability,
            }),
            Err(StorageError::Corrupt(_))
        ));
    }
    for durability in [1u16, 131] {
        assert!(
            mornlea_storage::checked_item_stack(ItemStack {
                item: 10,
                count: 1,
                durability,
            })
            .is_ok()
        );
    }
    assert!(matches!(
        mornlea_storage::checked_item_stack(ItemStack {
            item: 1,
            count: 1,
            durability: 1,
        }),
        Err(StorageError::Corrupt(_))
    ));

    for item in 0..=66u16 {
        for count in [0u8, 1, 64, 65] {
            for durability in [0u16, 1, 131, 132] {
                let raw = ItemStack {
                    item,
                    count,
                    durability,
                };
                assert_eq!(
                    raw.is_valid(),
                    mornlea_domain::ItemStack::try_new(item, count, durability).is_ok(),
                    "item {item} count {count} durability {durability}"
                );
            }
        }
    }

    let armor = ItemStack {
        item: 4242,
        count: 65,
        durability: 999,
    };
    assert!(matches!(
        mornlea_storage::checked_item_stack(armor),
        Err(StorageError::Corrupt(_))
    ));
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../../../packages/server/storage/player/testdata/player-v9.bin");
    let golden = fs::read(&path).unwrap_or_else(|err| panic!("read {}: {err}", path.display()));
    let player = PlayerId::from_bytes([
        0x00, 0x11, 0x22, 0x33, 0x44, 0x55, 0x46, 0x77, 0x88, 0x99, 0xaa, 0xbb, 0xcc, 0xdd, 0xee,
        0xff,
    ]);
    let decoded = decode_player(player, &golden).expect("v9 fixture");
    let mut save = PlayerSave {
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
    save.armor[0] = armor;
    save.armor[1] = ItemStack {
        item: u16::MAX,
        count: u8::MAX,
        durability: u16::MAX,
    };
    let encoded = encode_player(&save).expect("raw armor encodes");
    let round = decode_player(player, &encoded).expect("raw armor decodes");
    assert_eq!(round.armor[0], armor);
    assert_eq!(round.armor[1].item, u16::MAX);
    assert_eq!(round.armor[1].count, u8::MAX);
    assert_eq!(round.armor[1].durability, u16::MAX);
}

fn raw_table_agrees() -> bool {
    let samples = [
        canonical().to_bytes(),
        [0u8; 16],
        bytes(5, 0x80, 1),
        bytes(4, 0x00, 1),
        bytes(4, 0x80, 0),
        bytes(4, 0xc0, 9),
    ];
    samples.into_iter().all(|raw| {
        let storage = PlayerId::from_bytes(raw);
        storage.is_valid() == mornlea_domain::PlayerId::try_from_bytes(raw).is_ok()
    })
}
