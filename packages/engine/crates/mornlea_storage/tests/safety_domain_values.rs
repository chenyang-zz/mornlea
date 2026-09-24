//! Current save values use the domain constructors as their rule source.

use std::fs;
use std::path::PathBuf;

use mornlea_storage::{
    ContainerSnapshot, ItemStack, PlayerId, PlayerSave, StorageError, StorageKind, checked_section,
    decode_player, encode_player,
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

const SECTION_CELLS: usize = 4096;

fn packed_words(bits: u8, slots: &[(usize, u64)]) -> Vec<u64> {
    let per_word = 64 / bits as usize;
    let mut words = vec![0u64; SECTION_CELLS.div_ceil(per_word)];
    for &(index, slot) in slots {
        words[index / per_word] |= slot << ((index % per_word) * bits as usize);
    }
    words
}

fn single_section(block: u16) -> ContainerSnapshot {
    ContainerSnapshot {
        kind: StorageKind::Single,
        bits: 0,
        single: block,
        palette: Vec::new(),
        packed: Vec::new(),
    }
}

fn indexed_section(bits: u8, palette: Vec<u16>, packed: Vec<u64>) -> ContainerSnapshot {
    ContainerSnapshot {
        kind: StorageKind::Indexed,
        bits,
        single: 0,
        palette,
        packed,
    }
}

fn direct_section(packed: Vec<u64>) -> ContainerSnapshot {
    ContainerSnapshot {
        kind: StorageKind::Direct,
        bits: 15,
        single: 0,
        palette: Vec::new(),
        packed,
    }
}

fn assert_section_corrupt(raw: &ContainerSnapshot) {
    assert!(matches!(
        checked_section(raw),
        Err(StorageError::Corrupt(_))
    ));
}

#[test]
fn section_checked_conversions_preserve_compact_views_and_reject_residue() {
    for block in [0u16, 89] {
        let raw = single_section(block);
        let section = checked_section(&raw).expect("registered single section");
        assert_eq!(section.as_single(), Some(block));
        assert_eq!(section.as_indexed(), None);
        assert_eq!(section.as_direct(), None);
        assert_eq!(section.block_at(0), Some(block));
        assert_eq!(section.block_at(100), Some(block));
        assert_eq!(section.block_at(SECTION_CELLS - 1), Some(block));
    }
    assert_section_corrupt(&single_section(90));

    let mut nonzero_bits = single_section(0);
    nonzero_bits.bits = 4;
    assert_section_corrupt(&nonzero_bits);
    let mut leftover_palette = single_section(0);
    leftover_palette.palette = vec![0];
    assert_section_corrupt(&leftover_palette);
    let mut leftover_packed = single_section(0);
    leftover_packed.packed = vec![0];
    assert_section_corrupt(&leftover_packed);

    let palette4 = vec![18u16, 0];
    let packed4 = packed_words(4, &[(0, 0), (1, 1), (SECTION_CELLS - 1, 0)]);
    let raw4 = indexed_section(4, palette4.clone(), packed4.clone());
    let section4 = checked_section(&raw4).expect("4-bit section");
    let (bits, palette, words) = section4.as_indexed().expect("indexed view");
    assert_eq!(bits, 4);
    assert_eq!(palette, palette4.as_slice());
    assert_eq!(words, packed4.as_slice());
    assert_eq!(section4.as_single(), None);
    assert_eq!(section4.block_at(0), Some(18));
    assert_eq!(section4.block_at(1), Some(0));
    assert_eq!(section4.block_at(SECTION_CELLS - 1), Some(18));

    let palette8: Vec<u16> = (0..90).collect();
    let packed8 = packed_words(8, &[(0, 89), (100, 18), (SECTION_CELLS - 1, 0)]);
    let raw8 = indexed_section(8, palette8.clone(), packed8.clone());
    let section8 = checked_section(&raw8).expect("8-bit section");
    let (bits, palette, words) = section8.as_indexed().expect("indexed view");
    assert_eq!(bits, 8);
    assert_eq!(palette, palette8.as_slice());
    assert_eq!(words, packed8.as_slice());
    assert_eq!(words.len(), 512);
    assert_eq!(section8.block_at(0), Some(89));
    assert_eq!(section8.block_at(100), Some(18));
    assert_eq!(section8.block_at(SECTION_CELLS - 1), Some(0));

    let packed_direct = packed_words(15, &[(0, 0), (100, 9), (SECTION_CELLS - 1, 89)]);
    let raw_direct = direct_section(packed_direct.clone());
    let section_direct = checked_section(&raw_direct).expect("direct section");
    assert_eq!(section_direct.as_direct(), Some(packed_direct.as_slice()));
    assert_eq!(section_direct.as_single(), None);
    assert_eq!(section_direct.as_indexed(), None);
    assert_eq!(section_direct.block_at(0), Some(0));
    assert_eq!(section_direct.block_at(100), Some(9));
    assert_eq!(section_direct.block_at(SECTION_CELLS - 1), Some(89));

    assert_section_corrupt(&indexed_section(4, Vec::new(), packed_words(4, &[])));
    assert_section_corrupt(&indexed_section(4, vec![0, 0], packed_words(4, &[])));
    assert_section_corrupt(&indexed_section(4, (0..17).collect(), packed_words(4, &[])));
    assert_section_corrupt(&indexed_section(4, vec![0, 1], vec![0u64; 255]));
    assert_section_corrupt(&indexed_section(4, vec![0, 1], vec![0u64; 257]));
    assert_section_corrupt(&indexed_section(4, vec![0, 1], packed_words(4, &[(5, 2)])));
    assert_section_corrupt(&indexed_section(4, vec![0, 90], packed_words(4, &[])));
    let mut nonzero_single = indexed_section(4, vec![0], packed_words(4, &[]));
    nonzero_single.single = 1;
    assert_section_corrupt(&nonzero_single);

    let mut high_bits = packed_words(15, &[(0, 89)]);
    high_bits[7] |= 1 << 60;
    assert_section_corrupt(&direct_section(high_bits));
    assert_section_corrupt(&direct_section(packed_words(15, &[(0, 90)])));
    assert_section_corrupt(&direct_section(vec![0u64; 1023]));
    let mut direct_single = direct_section(packed_words(15, &[]));
    direct_single.single = 1;
    assert_section_corrupt(&direct_single);
    let mut direct_palette = direct_section(packed_words(15, &[]));
    direct_palette.palette = vec![0];
    assert_section_corrupt(&direct_palette);
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
