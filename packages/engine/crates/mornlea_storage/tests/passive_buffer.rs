use mornlea_storage::{
    PassiveMob, PassiveMobsSave, StorageError, decode_passive_mobs, encode_passive_mobs,
    encode_passive_mobs_into, passive_mobs_encoded_len,
};

fn base_mob(id: u64) -> PassiveMob {
    PassiveMob {
        id,
        dimension: 0,
        position: [0.0, 0.0, 0.0],
        velocity: [0.0, 0.0, 0.0],
        on_ground: false,
        yaw: 0.0,
        health: 1,
    }
}

fn valid_save(records: Vec<PassiveMob>) -> PassiveMobsSave {
    PassiveMobsSave {
        revision: 1,
        records,
    }
}

#[test]
fn empty_save_revision_one_accepted_revision_zero_rejected() {
    let save = PassiveMobsSave {
        revision: 1,
        records: vec![],
    };
    let n = passive_mobs_encoded_len(&save).expect("length");
    assert_eq!(n, 32);
    let encoded = encode_passive_mobs(&save).expect("encode");
    assert_eq!(encoded.len(), 32);
    decode_passive_mobs(&encoded).expect("decode empty");

    let zero = PassiveMobsSave {
        revision: 0,
        records: vec![],
    };
    assert!(matches!(
        passive_mobs_encoded_len(&zero),
        Err(StorageError::Corrupt { .. })
    ));
    assert!(matches!(
        encode_passive_mobs(&zero),
        Err(StorageError::Corrupt { .. })
    ));
}

#[test]
fn record_count_thirty_two_accepted_thirty_three_rejected() {
    let records: Vec<_> = (1..=32).map(base_mob).collect();
    let save = valid_save(records);
    assert_eq!(passive_mobs_encoded_len(&save).expect("32 length"), 2336);
    encode_passive_mobs(&save).expect("32 encode");

    let too_many: Vec<_> = (1..=33).map(base_mob).collect();
    let save33 = valid_save(too_many);
    assert!(matches!(
        passive_mobs_encoded_len(&save33),
        Err(StorageError::Corrupt { .. })
    ));
    assert!(matches!(
        encode_passive_mobs(&save33),
        Err(StorageError::Corrupt { .. })
    ));
}

#[test]
fn unsorted_ids_encode_canonical_without_mutating_input() {
    let save = valid_save(vec![base_mob(2), base_mob(1)]);
    let before = save.clone();
    let encoded = encode_passive_mobs(&save).expect("encode");
    assert_eq!(save, before);
    let decoded = decode_passive_mobs(&encoded).expect("decode");
    assert_eq!(decoded.records[0].id, 1);
    assert_eq!(decoded.records[1].id, 2);
}

#[test]
fn duplicate_id_rejected() {
    let save = valid_save(vec![base_mob(1), base_mob(1)]);
    assert!(matches!(
        encode_passive_mobs(&save),
        Err(StorageError::Corrupt { .. })
    ));
}

#[test]
fn dimension_one_rejected() {
    let mut bad = base_mob(1);
    bad.dimension = 1;
    assert!(matches!(
        encode_passive_mobs(&valid_save(vec![bad])),
        Err(StorageError::Corrupt { .. })
    ));
}

#[test]
fn position_y_boundaries() {
    let mut low = base_mob(1);
    low.position[1] = -64.0;
    encode_passive_mobs(&valid_save(vec![low])).expect("y -64");

    let mut high = base_mob(2);
    high.position[1] = 319.0;
    encode_passive_mobs(&valid_save(vec![high])).expect("y 319");

    let mut too_high = base_mob(3);
    too_high.position[1] = 320.0;
    assert!(encode_passive_mobs(&valid_save(vec![too_high])).is_err());
}

#[test]
fn health_boundaries() {
    let mut one = base_mob(1);
    one.health = 1;
    encode_passive_mobs(&valid_save(vec![one])).expect("health 1");

    let mut twenty = base_mob(2);
    twenty.health = 20;
    encode_passive_mobs(&valid_save(vec![twenty])).expect("health 20");

    let mut zero = base_mob(3);
    zero.health = 0;
    assert!(encode_passive_mobs(&valid_save(vec![zero])).is_err());

    let mut high = base_mob(4);
    high.health = 21;
    assert!(encode_passive_mobs(&valid_save(vec![high])).is_err());
}

#[test]
fn one_record_is_seventy_two_bytes_with_zero_reserved_tail() {
    let encoded = encode_passive_mobs(&valid_save(vec![base_mob(1)])).expect("encode");
    assert_eq!(encoded.len(), 32 + 72);
    let record = &encoded[32..32 + 72];
    assert_eq!(record.len(), 72);
    assert!(record[42..72].iter().all(|&b| b == 0));
}

#[test]
fn buffer_canaries_at_exact_encoded_length() {
    let save = valid_save(vec![base_mob(1)]);
    let n = passive_mobs_encoded_len(&save).expect("length");
    assert_eq!(n, 32 + 72);
    let expected = encode_passive_mobs(&save).expect("encode");

    let mut short = vec![0xA5; n - 1];
    assert_eq!(
        encode_passive_mobs_into(&save, &mut short),
        Err(StorageError::OutputTooSmall {
            needed: n,
            available: n - 1,
        })
    );
    assert!(short.iter().all(|&b| b == 0xA5));

    let mut exact = vec![0xA5; n];
    assert_eq!(
        encode_passive_mobs_into(&save, &mut exact).expect("exact"),
        n
    );
    assert_eq!(&exact, expected.as_slice());

    let mut larger = vec![0xA5; n + 7];
    assert_eq!(
        encode_passive_mobs_into(&save, &mut larger).expect("larger"),
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
    let n = passive_mobs_encoded_len(&valid_save(vec![base_mob(99)])).expect("valid length");
    let mut buf = vec![0xA5; n - 1];
    assert!(matches!(
        encode_passive_mobs_into(&save, &mut buf),
        Err(StorageError::Corrupt { .. })
    ));
    assert!(buf.iter().all(|&b| b == 0xA5));
}
