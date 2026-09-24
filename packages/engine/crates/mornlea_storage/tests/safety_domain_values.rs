//! Current save values use the domain constructors as their rule source.

use mornlea_storage::{PlayerId, StorageError};

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
