use mornlea_storage::{
    METADATA_CURRENT_VERSION, Metadata, MetadataChunkPos, StorageError, decode_world_metadata,
    encode_world_metadata, encode_world_metadata_into, world_metadata_encoded_len,
};

fn base_metadata() -> Metadata {
    Metadata {
        format_version: METADATA_CURRENT_VERSION,
        seed: 1,
        spawn_dimension: 0,
        spawn_anchor: MetadataChunkPos { x: 0, z: 0 },
        world_time_ticks: 0,
        day_phase_offset: 0,
        weather_kind: 0,
        weather_ticks_remaining: 0,
        depths_spawn_anchor: MetadataChunkPos { x: 0, z: 0 },
        depths_seed_salt: 0x9E37_79B9_7F4A_7C15,
        difficulty: 0,
    }
}

fn round_trip_preserves_bytes(metadata: &Metadata) {
    let encoded = encode_world_metadata(metadata).expect("encode");
    let decoded = decode_world_metadata(&encoded).expect("decode");
    assert_eq!(decoded, *metadata);
    let mut buf = vec![0u8; encoded.len()];
    let written = encode_world_metadata_into(metadata, &mut buf).expect("encode_into");
    assert_eq!(written, encoded.len());
    assert_eq!(buf, encoded);
}

#[test]
fn weather_kind_seven_and_255_are_byte_preserved() {
    for weather in [7u8, 255] {
        let mut metadata = base_metadata();
        metadata.weather_kind = weather;
        round_trip_preserves_bytes(&metadata);
        let encoded = encode_world_metadata(&metadata).expect("encode");
        assert_eq!(encoded[48], weather);
    }
}

#[test]
fn spawn_dimension_negative_three_is_preserved() {
    let mut metadata = base_metadata();
    metadata.spawn_dimension = -3;
    round_trip_preserves_bytes(&metadata);
}

#[test]
fn day_phase_offset_max_is_preserved() {
    let mut metadata = base_metadata();
    metadata.day_phase_offset = u64::MAX;
    round_trip_preserves_bytes(&metadata);
}

#[test]
fn signed_seed_boundary_is_preserved() {
    let mut metadata = base_metadata();
    metadata.seed = i64::MIN;
    round_trip_preserves_bytes(&metadata);
}

#[test]
fn weather_ticks_remaining_max_is_preserved() {
    let mut metadata = base_metadata();
    metadata.weather_ticks_remaining = u32::MAX;
    round_trip_preserves_bytes(&metadata);
}

#[test]
fn difficulty_zero_one_two_accepted_and_preserved() {
    for difficulty in [0u8, 1, 2] {
        let mut metadata = base_metadata();
        metadata.difficulty = difficulty;
        round_trip_preserves_bytes(&metadata);
        let encoded = encode_world_metadata(&metadata).expect("encode");
        assert_eq!(encoded[73], difficulty);
    }
}

#[test]
fn difficulty_three_is_rejected_on_encode() {
    let mut metadata = base_metadata();
    metadata.difficulty = 3;
    assert!(matches!(
        encode_world_metadata(&metadata),
        Err(StorageError::Corrupt { .. })
    ));
    assert!(matches!(
        world_metadata_encoded_len(&metadata),
        Err(StorageError::Corrupt { .. })
    ));
}

#[test]
fn buffer_canaries_at_exact_record_length() {
    let metadata = base_metadata();
    let n = world_metadata_encoded_len(&metadata).expect("length");
    assert_eq!(n, 78);
    let expected = encode_world_metadata(&metadata).expect("encode");

    let mut short = vec![0xA5; n - 1];
    assert_eq!(
        encode_world_metadata_into(&metadata, &mut short),
        Err(StorageError::OutputTooSmall {
            needed: n,
            available: n - 1,
        })
    );
    assert!(short.iter().all(|&b| b == 0xA5));

    let mut exact = vec![0xA5; n];
    assert_eq!(
        encode_world_metadata_into(&metadata, &mut exact).expect("exact"),
        n
    );
    assert_eq!(&exact, expected.as_slice());

    let mut larger = vec![0xA5; n + 7];
    assert_eq!(
        encode_world_metadata_into(&metadata, &mut larger).expect("larger"),
        n
    );
    assert_eq!(&larger[..n], expected.as_slice());
    assert!(larger[n..].iter().all(|&b| b == 0xA5));
}

#[test]
fn difficulty_three_with_short_buffer_returns_corruption_and_preserves_canaries() {
    let mut metadata = base_metadata();
    metadata.difficulty = 3;
    let n = world_metadata_encoded_len(&base_metadata()).expect("valid length");
    let mut buf = vec![0xA5; n - 1];
    assert!(matches!(
        encode_world_metadata_into(&metadata, &mut buf),
        Err(StorageError::Corrupt { .. })
    ));
    assert!(buf.iter().all(|&b| b == 0xA5));
}

#[test]
fn legacy_and_malformed_records_stay_rejected() {
    use mornlea_storage::crc32c;

    let valid = encode_world_metadata(&base_metadata()).expect("encode");

    let mut version_zero = valid.clone();
    version_zero[4..8].copy_from_slice(&0u32.to_le_bytes());
    assert!(decode_world_metadata(&version_zero).is_err());

    let mut version_seven = valid.clone();
    version_seven[4..8].copy_from_slice(&7u32.to_le_bytes());
    assert!(decode_world_metadata(&version_seven).is_err());

    let mut wrong_dimension_count = valid.clone();
    wrong_dimension_count[53..57].copy_from_slice(&3u32.to_le_bytes());
    let checksum_offset = valid.len() - 4;
    let checksum = crc32c(&wrong_dimension_count[..checksum_offset]);
    wrong_dimension_count[checksum_offset..].copy_from_slice(&checksum.to_le_bytes());
    assert!(decode_world_metadata(&wrong_dimension_count).is_err());

    assert!(decode_world_metadata(&valid[..valid.len() - 1]).is_err());
    let mut trailing = valid.clone();
    trailing.push(0);
    assert!(decode_world_metadata(&trailing).is_err());

    let mut bad_magic = valid.clone();
    bad_magic[0] ^= 0xff;
    assert!(decode_world_metadata(&bad_magic).is_err());

    let mut bad_crc = valid.clone();
    bad_crc[12] ^= 0xff;
    assert!(decode_world_metadata(&bad_crc).is_err());

    for version in [0u32, 1, 5, 7] {
        let mut metadata = base_metadata();
        metadata.format_version = version;
        assert!(encode_world_metadata(&metadata).is_err());
    }
}
