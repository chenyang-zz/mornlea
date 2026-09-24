use mornlea_storage::{
    HostileMob, HostileMobsSave, PlayerId, StorageError, decode_hostile_mobs, encode_hostile_mobs,
    encode_hostile_mobs_into, hostile_mobs_encoded_len,
};

fn base_mob(id: u64) -> HostileMob {
    HostileMob {
        id,
        dimension: 0,
        position: [0.0, 0.0, 0.0],
        velocity: [0.0, 0.0, 0.0],
        on_ground: false,
        yaw: 0.0,
        health: 1,
        attack_cooldown: 0,
        hurt_cooldown: 0,
        burn_cooldown: 0,
        has_target: false,
        player_id: PlayerId::from_bytes([0; 16]),
        next_repath_ticks: 0,
        distant_ticks: 0,
        kind: 0,
    }
}

fn valid_save(records: Vec<HostileMob>) -> HostileMobsSave {
    HostileMobsSave {
        revision: 1,
        records,
    }
}

#[test]
fn empty_save_revision_one_accepted_revision_zero_rejected() {
    let save = HostileMobsSave {
        revision: 1,
        records: vec![],
    };
    let n = hostile_mobs_encoded_len(&save).expect("length");
    assert_eq!(n, 32);
    let encoded = encode_hostile_mobs(&save).expect("encode");
    assert_eq!(encoded.len(), 32);
    decode_hostile_mobs(&encoded).expect("decode empty");

    let zero = HostileMobsSave {
        revision: 0,
        records: vec![],
    };
    assert!(matches!(
        hostile_mobs_encoded_len(&zero),
        Err(StorageError::Corrupt { .. })
    ));
    assert!(matches!(
        encode_hostile_mobs(&zero),
        Err(StorageError::Corrupt { .. })
    ));
}

#[test]
fn record_count_sixty_four_accepted_sixty_five_rejected() {
    let records: Vec<_> = (1..=64).map(base_mob).collect();
    let save = valid_save(records);
    hostile_mobs_encoded_len(&save).expect("64 length");
    encode_hostile_mobs(&save).expect("64 encode");

    let too_many: Vec<_> = (1..=65).map(base_mob).collect();
    let save65 = valid_save(too_many);
    assert!(matches!(
        hostile_mobs_encoded_len(&save65),
        Err(StorageError::Corrupt { .. })
    ));
    assert!(matches!(
        encode_hostile_mobs(&save65),
        Err(StorageError::Corrupt { .. })
    ));
}

#[test]
fn unsorted_ids_encode_canonical_without_mutating_input() {
    let save = valid_save(vec![base_mob(2), base_mob(1)]);
    let before = save.clone();
    let encoded = encode_hostile_mobs(&save).expect("encode");
    assert_eq!(save, before);
    let decoded = decode_hostile_mobs(&encoded).expect("decode");
    assert_eq!(decoded.records[0].id, 1);
    assert_eq!(decoded.records[1].id, 2);
}

#[test]
fn duplicate_id_rejected() {
    let save = valid_save(vec![base_mob(1), base_mob(1)]);
    assert!(matches!(
        encode_hostile_mobs(&save),
        Err(StorageError::Corrupt { .. })
    ));
}

#[test]
fn position_y_boundaries() {
    let mut low = base_mob(1);
    low.position[1] = -64.0;
    encode_hostile_mobs(&valid_save(vec![low])).expect("y -64");

    let mut high = base_mob(2);
    high.position[1] = 319.0;
    encode_hostile_mobs(&valid_save(vec![high])).expect("y 319");

    let mut too_high = base_mob(3);
    too_high.position[1] = 320.0;
    assert!(encode_hostile_mobs(&valid_save(vec![too_high])).is_err());
}

#[test]
fn health_zero_rejected_twenty_one_rejected() {
    let mut zero = base_mob(1);
    zero.health = 0;
    assert!(encode_hostile_mobs(&valid_save(vec![zero])).is_err());

    let mut high = base_mob(2);
    high.health = 21;
    assert!(encode_hostile_mobs(&valid_save(vec![high])).is_err());

    let mut ok = base_mob(3);
    ok.health = 20;
    encode_hostile_mobs(&valid_save(vec![ok])).expect("health 20");
}

#[test]
fn cooldown_twenty_accepted_twenty_one_rejected() {
    let mut ok = base_mob(1);
    ok.attack_cooldown = 20;
    ok.hurt_cooldown = 20;
    ok.burn_cooldown = 20;
    encode_hostile_mobs(&valid_save(vec![ok])).expect("cooldown 20");

    let mut bad = base_mob(2);
    bad.attack_cooldown = 21;
    assert!(encode_hostile_mobs(&valid_save(vec![bad])).is_err());
}

#[test]
fn distant_ticks_six_hundred_accepted_six_hundred_one_rejected() {
    let mut ok = base_mob(1);
    ok.distant_ticks = 600;
    encode_hostile_mobs(&valid_save(vec![ok])).expect("distant 600");

    let mut bad = base_mob(2);
    bad.distant_ticks = 601;
    assert!(encode_hostile_mobs(&valid_save(vec![bad])).is_err());
}

#[test]
fn target_absent_nonzero_raw_rejected_target_present_invalid_uuid_rejected() {
    let mut absent_garbage = base_mob(1);
    absent_garbage.has_target = false;
    absent_garbage.player_id = PlayerId::from_bytes([0xFF; 16]);
    assert!(matches!(
        encode_hostile_mobs(&valid_save(vec![absent_garbage])),
        Err(StorageError::Corrupt { .. })
    ));

    let mut bad_target = base_mob(2);
    bad_target.has_target = true;
    bad_target.player_id = PlayerId::from_bytes([0xFF; 16]);
    assert!(encode_hostile_mobs(&valid_save(vec![bad_target])).is_err());
}

#[test]
fn kind_zero_and_one_accepted_two_rejected() {
    let mut k0 = base_mob(1);
    k0.kind = 0;
    encode_hostile_mobs(&valid_save(vec![k0])).expect("kind 0");

    let mut k1 = base_mob(2);
    k1.kind = 1;
    k1.has_target = true;
    k1.player_id = PlayerId::from_bytes([
        0x6f, 0xce, 0x82, 0x77, 0xa9, 0x33, 0x46, 0xcb, 0x9a, 0x1f, 0xda, 0x13, 0xb7, 0xee, 0x56,
        0x44,
    ]);
    encode_hostile_mobs(&valid_save(vec![k1])).expect("kind 1");

    let mut k2 = base_mob(3);
    k2.kind = 2;
    assert!(encode_hostile_mobs(&valid_save(vec![k2])).is_err());
}

#[test]
fn next_repath_ticks_max_preserved() {
    let mut mob = base_mob(1);
    mob.next_repath_ticks = u64::MAX;
    let encoded = encode_hostile_mobs(&valid_save(vec![mob])).expect("encode");
    let decoded = decode_hostile_mobs(&encoded).expect("decode");
    assert_eq!(decoded.records[0].next_repath_ticks, u64::MAX);
}

#[test]
fn buffer_canaries_at_exact_encoded_length() {
    let save = valid_save(vec![base_mob(1)]);
    let n = hostile_mobs_encoded_len(&save).expect("length");
    assert_eq!(n, 32 + 73);
    let expected = encode_hostile_mobs(&save).expect("encode");

    let mut short = vec![0xA5; n - 1];
    assert_eq!(
        encode_hostile_mobs_into(&save, &mut short),
        Err(StorageError::OutputTooSmall {
            needed: n,
            available: n - 1,
        })
    );
    assert!(short.iter().all(|&b| b == 0xA5));

    let mut exact = vec![0xA5; n];
    assert_eq!(
        encode_hostile_mobs_into(&save, &mut exact).expect("exact"),
        n
    );
    assert_eq!(&exact, expected.as_slice());

    let mut larger = vec![0xA5; n + 7];
    assert_eq!(
        encode_hostile_mobs_into(&save, &mut larger).expect("larger"),
        n
    );
    assert_eq!(&larger[..n], expected.as_slice());
    assert!(larger[n..].iter().all(|&b| b == 0xA5));
}

#[test]
fn invalid_record_with_short_buffer_returns_corruption_and_preserves_canaries() {
    let mut bad = base_mob(1);
    bad.health = 0;
    let save = valid_save(vec![bad]);
    let n = hostile_mobs_encoded_len(&valid_save(vec![base_mob(99)])).expect("valid length");
    let mut buf = vec![0xA5; n - 1];
    assert!(matches!(
        encode_hostile_mobs_into(&save, &mut buf),
        Err(StorageError::Corrupt { .. })
    ));
    assert!(buf.iter().all(|&b| b == 0xA5));
}
