//! Foundation registration and dependency-direction contracts for `mornlea_storage`.
//! Save-family ports remain recorded in the change ledger until each family
//! has a failing behavioral case of its own.

use mornlea_storage::{
    BANK_A_START_SECTOR, BANK_B_START_SECTOR, BANK_SIZE, CHUNK_CURRENT_SCHEMA,
    CHUNK_ENVELOPE_LENGTH, CHUNK_MAX_DECODED_CHUNK, CHUNK_OLDEST_SCHEMA, COMPANION_CURRENT_SCHEMA,
    COMPANION_ENVELOPE_VERSION, COMPANION_MAX_FIFO_ENTRIES, COMPANION_MAX_FILE_LENGTH,
    COMPANION_MAX_STORED, COMPANION_PLAN_STEP_FOLLOW, COMPANION_PLAN_STEP_GO_TO,
    COMPANION_PLAN_STEP_MINE, COMPANION_PLAN_STEP_PLACE, COMPANION_SCHEMA_V1, COMPANION_SCHEMA_V2,
    COMPANION_SCHEMA_V3, COMPANION_SCHEMA_V4, COMPANION_TASK_FAIL_NONE, COMPANION_TASK_RUNNING,
    ChestSlot, Chunk, ChunkKey, ChunkSave, CompanionBody, CompanionSave, ContainerSnapshot,
    DATA_START_SECTOR, DecodedChunk, DropSlot, FurnaceSlot, HOSTILE_CURRENT_SCHEMA,
    HOSTILE_ENVELOPE_VERSION, HOSTILE_MAX_FILE_LENGTH, HOSTILE_SCHEMA_V1, HostileMob,
    HostileMobsSave, Inventory, ItemStack, MAX_COMPRESSED_CHUNK, MAX_HOSTILE_MOBS,
    MAX_PASSIVE_MOBS, METADATA_CURRENT_VERSION, METADATA_V1, METADATA_V2, METADATA_V3, METADATA_V4,
    METADATA_V5, Metadata, MetadataChunkPos, PASSIVE_CURRENT_SCHEMA, PASSIVE_ENVELOPE_VERSION,
    PASSIVE_MAX_FILE_LENGTH, PLAYER_CURRENT_SCHEMA, PLAYER_ENVELOPE_LENGTH, PLAYER_MAX_PAYLOAD,
    PassiveMob, PassiveMobsSave, PlanStep, PlayerId, PlayerLocation, PlayerSave, REGION_SLOTS,
    RegionBank, RegionEntry, RegionKey, SECTOR_SIZE, StorageError, StorageKind,
    StoredCompanionLifecycle, StoredCompanionQueue, StoredCompanionTask, StoredPlayer, crc32c,
    crc32c_join, decode_chunk, decode_chunk_envelope, decode_chunk_logical, decode_companions,
    decode_hostile_mobs, decode_passive_mobs, decode_player, decode_region_bank, decode_superblock,
    decode_world_metadata, encode_chunk, encode_chunk_at_schema, encode_chunk_logical,
    encode_companions, encode_hostile_mobs, encode_passive_mobs, encode_player, encode_region_bank,
    encode_region_bank_into, encode_superblock, encode_superblock_into, encode_world_metadata,
    item_max_durability, region_for, select_region_bank,
};
use std::fs;
use std::path::PathBuf;

const FORBIDDEN_PRODUCTION_DEPS: &[&str] = &[
    "mornlea_protocol",
    "mornlea_engine",
    "mornlea_client",
    "mornlea_godot",
];

#[test]
fn crate_identity_matches_workspace_name() {
    assert_eq!(mornlea_storage::CRATE_NAME, env!("CARGO_PKG_NAME"));
}

/// `mornlea_storage` may depend only on `mornlea_domain` plus the single
/// approved compression crate. The `save.chunk` envelope is a zstd frame, so
/// decoding the committed Go fixtures needs a zstd decoder, and re-encoding was
/// approved to attempt byte-exact parity with the Go `Encode` output (content
/// checksum included). `zstd` — the crate that binds the reference libzstd C
/// implementation — is therefore the one permitted non-domain production
/// dependency. Everything else stays forbidden: protocol codecs, the numerical
/// kernel, a graphical host, and the Godot bridge. The permitted set is pinned
/// exactly so the dependency surface cannot drift silently.
#[test]
fn production_manifest_depends_only_on_domain() {
    let keys = production_dependency_keys(&read_manifest(env!("CARGO_MANIFEST_DIR")));
    assert_eq!(keys, ["mornlea_domain", "zstd"]);
    for forbidden in FORBIDDEN_PRODUCTION_DEPS {
        assert!(
            !keys.iter().any(|key| key == forbidden),
            "mornlea_storage must not depend on {forbidden}"
        );
    }
}

#[test]
fn domain_does_not_depend_on_storage() {
    let domain_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../mornlea_domain");
    let keys = production_dependency_keys(&read_manifest(domain_dir.to_str().unwrap()));
    assert!(
        !keys.iter().any(|key| key == "mornlea_storage"),
        "mornlea_domain must not depend on mornlea_storage"
    );
}

#[test]
fn inventory_assigns_save_families_to_this_crate() {
    let json = read_inventory();
    let owner = format!("\"eventual_owner\": \"{}\"", env!("CARGO_PKG_NAME"));
    let count = json.matches(&owner).count();
    assert_eq!(
        count, 7,
        "save inventory rows drifted; update intended family ports before implementing them"
    );
    for family in [
        "save.chunk",
        "save.companion",
        "save.hostile",
        "save.passive",
        "save.player",
        "save.region",
        "save.world-metadata",
    ] {
        assert!(
            json.contains(&format!("\"id\": \"{family}\"")),
            "missing save family {family}"
        );
    }
}

#[test]
fn passive_current_schema_round_trip_preserves_bytes() {
    let save = PassiveMobsSave {
        revision: 41,
        records: vec![
            passive_mob(7, 1.5, -3.25, 12.0),
            passive_mob(3, -64.0, 0.0, 319.999),
        ],
    };
    let encoded = encode_passive_mobs(&save).expect("encode passive mobs");
    assert_eq!(encoded.len(), 32 + 2 * 72);
    let decoded = decode_passive_mobs(&encoded).expect("decode passive mobs");
    let mut expected = save.records.clone();
    expected.sort_by_key(|record| record.id);
    assert_eq!(decoded.revision, 41);
    assert_eq!(decoded.records, expected);
    let reencoded = encode_passive_mobs(&PassiveMobsSave {
        revision: decoded.revision,
        records: decoded.records.clone(),
    })
    .expect("re-encode passive mobs");
    assert_eq!(reencoded, encoded, "canonical form must be byte-stable");
}

#[test]
fn passive_encode_is_canonical_regardless_of_input_order() {
    let forward = PassiveMobsSave {
        revision: 5,
        records: vec![passive_mob(1, 0.0, 0.0, 0.0), passive_mob(2, 1.0, 1.0, 1.0)],
    };
    let mut reversed = forward.clone();
    reversed.records.reverse();
    assert_eq!(
        encode_passive_mobs(&forward).expect("encode forward"),
        encode_passive_mobs(&reversed).expect("encode reversed"),
    );
}

#[test]
fn passive_maximum_records_fit_the_file_ceiling() {
    let records: Vec<PassiveMob> = (1..=MAX_PASSIVE_MOBS as u64)
        .map(|id| passive_mob(id, 0.5, 64.0, 100.0))
        .collect();
    let save = PassiveMobsSave {
        revision: 9,
        records,
    };
    let encoded = encode_passive_mobs(&save).expect("encode maximum passive mobs");
    assert_eq!(encoded.len(), PASSIVE_MAX_FILE_LENGTH);
    let decoded = decode_passive_mobs(&encoded).expect("decode maximum passive mobs");
    assert_eq!(decoded.records.len(), MAX_PASSIVE_MOBS);
}

#[test]
fn passive_decode_rejects_future_versions() {
    let encoded = encode_passive_mobs(&PassiveMobsSave {
        revision: 1,
        records: vec![passive_mob(1, 0.0, 0.0, 0.0)],
    })
    .expect("encode passive mobs");

    let mut future_schema = encoded.clone();
    future_schema[8..12].copy_from_slice(&(PASSIVE_CURRENT_SCHEMA + 1).to_le_bytes());
    assert_eq!(
        decode_passive_mobs(&future_schema).unwrap_err(),
        StorageError::FutureVersion("passive schema: 2".to_owned()),
    );

    let mut future_envelope = encoded.clone();
    future_envelope[4..8].copy_from_slice(&(PASSIVE_ENVELOPE_VERSION + 1).to_le_bytes());
    assert_eq!(
        decode_passive_mobs(&future_envelope).unwrap_err(),
        StorageError::FutureVersion("passive envelope version: 2".to_owned()),
    );
}

#[test]
fn passive_decode_rejects_corrupt_and_partial_records() {
    let encoded = encode_passive_mobs(&PassiveMobsSave {
        revision: 3,
        records: vec![passive_mob(1, 0.0, 0.0, 0.0), passive_mob(2, 1.0, 1.0, 1.0)],
    })
    .expect("encode passive mobs");

    for length in [0, 4, 31, 32, 40, encoded.len() - 1] {
        assert!(
            decode_passive_mobs(&encoded[..length]).is_err(),
            "truncated passive file of {length} bytes must be rejected"
        );
    }

    let mut oversized_count = encoded.clone();
    oversized_count[20..24].copy_from_slice(&(MAX_PASSIVE_MOBS as u32 + 1).to_le_bytes());
    assert!(decode_passive_mobs(&oversized_count).is_err());

    let mut oversized_file = encoded.clone();
    oversized_file.extend_from_slice(&[0u8; 8]);
    assert!(decode_passive_mobs(&oversized_file).is_err());

    let mut nonzero_reserved = encoded.clone();
    nonzero_reserved[32 + 42] = 1;
    reseal_passive(&mut nonzero_reserved);
    assert!(decode_passive_mobs(&nonzero_reserved).is_err());

    let mut broken_crc = encoded.clone();
    broken_crc[60] ^= 0xff;
    assert!(decode_passive_mobs(&broken_crc).is_err());

    let mut unsorted = encoded.clone();
    unsorted[32..40].copy_from_slice(&9u64.to_le_bytes());
    reseal_passive(&mut unsorted);
    assert!(decode_passive_mobs(&unsorted).is_err());

    let mut bad_bool = encoded.clone();
    bad_bool[32 + 36] = 2;
    reseal_passive(&mut bad_bool);
    assert!(decode_passive_mobs(&bad_bool).is_err());

    let mut bad_dimension = encoded.clone();
    bad_dimension[32 + 8..32 + 12].copy_from_slice(&1u32.to_le_bytes());
    reseal_passive(&mut bad_dimension);
    assert!(decode_passive_mobs(&bad_dimension).is_err());

    let mut bad_health = encoded.clone();
    bad_health[32 + 41] = 21;
    reseal_passive(&mut bad_health);
    assert!(decode_passive_mobs(&bad_health).is_err());
}

/// Recomputes the passive envelope checksum in place so a semantic mutation
/// is rejected by the record validator rather than the CRC gate.
fn reseal_passive(bytes: &mut [u8]) {
    let checksum = crc32c_join(&[&bytes[8..28], &bytes[32..]]);
    bytes[28..32].copy_from_slice(&checksum.to_le_bytes());
}

#[test]
fn passive_encode_rejects_invalid_saves() {
    assert!(
        encode_passive_mobs(&PassiveMobsSave {
            revision: 0,
            records: vec![passive_mob(1, 0.0, 0.0, 0.0)],
        })
        .is_err()
    );

    let too_many: Vec<PassiveMob> = (1..=MAX_PASSIVE_MOBS as u64 + 1)
        .map(|id| passive_mob(id, 0.0, 0.0, 0.0))
        .collect();
    assert!(
        encode_passive_mobs(&PassiveMobsSave {
            revision: 1,
            records: too_many,
        })
        .is_err()
    );

    let duplicate = PassiveMobsSave {
        revision: 1,
        records: vec![passive_mob(1, 0.0, 0.0, 0.0), passive_mob(1, 1.0, 1.0, 1.0)],
    };
    assert!(encode_passive_mobs(&duplicate).is_err());

    let mut outside_world = passive_mob(1, 0.0, 320.0, 0.0);
    assert!(
        encode_passive_mobs(&PassiveMobsSave {
            revision: 1,
            records: vec![outside_world.clone()],
        })
        .is_err()
    );
    outside_world.position[1] = -64.001;
    assert!(
        encode_passive_mobs(&PassiveMobsSave {
            revision: 1,
            records: vec![outside_world],
        })
        .is_err()
    );

    let mut zero_id = passive_mob(0, 0.0, 0.0, 0.0);
    assert!(
        encode_passive_mobs(&PassiveMobsSave {
            revision: 1,
            records: vec![zero_id.clone()],
        })
        .is_err()
    );
    zero_id.health = 0;
    zero_id.id = 1;
    assert!(
        encode_passive_mobs(&PassiveMobsSave {
            revision: 1,
            records: vec![zero_id],
        })
        .is_err()
    );
}

#[test]
fn passive_committed_go_fixture_round_trips_byte_for_byte() {
    let golden = read_go_fixture("server/storage/passive/testdata/passive-mobs-v1.bin");
    assert_eq!(golden.len(), 32 + 3 * 72);
    assert_eq!(&golden[0..4], b"PMST");
    let before = golden.clone();
    let decoded = decode_passive_mobs(&golden).expect("decode committed passive fixture");
    assert_eq!(golden, before, "decoding must not rewrite the input bytes");
    assert_eq!(decoded.revision, 11);
    assert_eq!(decoded.records.len(), 3);
    assert_eq!(decoded.records[0].id, 1);
    assert_eq!(decoded.records[0].position, [8.5, 65.5, 9.75]);
    assert_eq!(decoded.records[0].health, 1);
    let reencoded = encode_passive_mobs(&PassiveMobsSave {
        revision: decoded.revision,
        records: decoded.records.clone(),
    })
    .expect("re-encode committed passive fixture");
    assert_eq!(
        reencoded, golden,
        "the Rust encoder must reproduce the committed Go bytes exactly"
    );
}

fn passive_mob(id: u64, x: f32, y: f32, z: f32) -> PassiveMob {
    PassiveMob {
        id,
        dimension: 0,
        position: [x, y, z],
        velocity: [0.0, 0.0, 0.0],
        on_ground: true,
        yaw: 1.25,
        health: 20,
    }
}

#[test]
fn hostile_current_schema_round_trip_preserves_bytes() {
    let save = HostileMobsSave {
        revision: 31,
        records: vec![
            hostile_bone_thrower(0x8000_0000_0000_0002),
            hostile_nightcrawler(0x4000_0000_0000_0001),
            hostile_nightcrawler(1),
        ],
    };
    let encoded = encode_hostile_mobs(&save).expect("encode hostile mobs");
    assert_eq!(encoded.len(), 32 + 3 * 73);
    let decoded = decode_hostile_mobs(&encoded).expect("decode hostile mobs");
    let mut expected = save.records.clone();
    expected.sort_by_key(|record| record.id);
    assert_eq!(decoded.revision, 31);
    assert_eq!(decoded.records, expected);
    let reencoded = encode_hostile_mobs(&HostileMobsSave {
        revision: decoded.revision,
        records: decoded.records.clone(),
    })
    .expect("re-encode hostile mobs");
    assert_eq!(reencoded, encoded, "canonical form must be byte-stable");
    assert_eq!(HOSTILE_MAX_FILE_LENGTH, 4704);
}

#[test]
fn hostile_encode_is_canonical_regardless_of_input_order() {
    let forward = HostileMobsSave {
        revision: 5,
        records: vec![hostile_nightcrawler(1), hostile_nightcrawler(2)],
    };
    let mut reversed = forward.clone();
    reversed.records.reverse();
    assert_eq!(
        encode_hostile_mobs(&forward).expect("encode forward"),
        encode_hostile_mobs(&reversed).expect("encode reversed"),
    );
}

#[test]
fn hostile_empty_collection_round_trips() {
    let encoded = encode_hostile_mobs(&HostileMobsSave {
        revision: 4,
        records: Vec::new(),
    })
    .expect("encode empty hostile mobs");
    assert_eq!(encoded.len(), 32);
    let decoded = decode_hostile_mobs(&encoded).expect("decode empty hostile mobs");
    assert_eq!(decoded.revision, 4);
    assert!(decoded.records.is_empty());
}

#[test]
fn hostile_maximum_records_fit_the_file_ceiling() {
    let records: Vec<HostileMob> = (1..=MAX_HOSTILE_MOBS as u64)
        .map(hostile_nightcrawler)
        .collect();
    let encoded = encode_hostile_mobs(&HostileMobsSave {
        revision: 23,
        records,
    })
    .expect("encode maximum hostile mobs");
    assert_eq!(encoded.len(), HOSTILE_MAX_FILE_LENGTH);
    let decoded = decode_hostile_mobs(&encoded).expect("decode maximum hostile mobs");
    assert_eq!(decoded.records.len(), MAX_HOSTILE_MOBS);

    let mut trailing = encoded.clone();
    trailing.push(0);
    assert!(decode_hostile_mobs(&trailing).is_err());

    let too_many: Vec<HostileMob> = (1..=MAX_HOSTILE_MOBS as u64 + 1)
        .map(hostile_nightcrawler)
        .collect();
    assert!(
        encode_hostile_mobs(&HostileMobsSave {
            revision: 1,
            records: too_many,
        })
        .is_err()
    );

    let oversized = vec![0x5au8; HOSTILE_MAX_FILE_LENGTH + 1];
    assert!(decode_hostile_mobs(&oversized).is_err());
}

#[test]
fn hostile_v1_fixture_migrates_to_kind_zero_and_rewrites_as_v2() {
    let golden = read_go_fixture("server/storage/hostile/testdata/hostile-mobs-v1.bin");
    assert_eq!(golden.len(), 32 + 3 * 72);
    assert_eq!(
        u32::from_le_bytes(golden[8..12].try_into().unwrap()),
        HOSTILE_SCHEMA_V1,
        "the frozen v1 golden must stay on schema 1"
    );
    let before = golden.clone();
    let decoded = decode_hostile_mobs(&golden).expect("decode committed hostile v1 fixture");
    assert_eq!(
        golden, before,
        "v1 migration must not rewrite the input bytes"
    );
    assert_eq!(decoded.revision, 19);
    let expected = [
        hostile_far(1),
        hostile_idle(0x4000_0000_0000_0001),
        hostile_bone_thrower(0x8000_0000_0000_0002),
    ];
    for (record, want) in decoded.records.iter().zip(expected.iter()) {
        let mut migrated = want.clone();
        migrated.kind = 0;
        assert_eq!(
            record, &migrated,
            "v1 migration must keep every field but kind"
        );
    }

    let rewritten = encode_hostile_mobs(&HostileMobsSave {
        revision: decoded.revision,
        records: decoded.records.clone(),
    })
    .expect("rewrite migrated hostile mobs");
    assert_eq!(
        u32::from_le_bytes(rewritten[8..12].try_into().unwrap()),
        HOSTILE_CURRENT_SCHEMA,
    );
    assert_eq!(
        &rewritten[0..8],
        &golden[0..8],
        "envelope magic and version"
    );
    assert_eq!(&rewritten[12..24], &golden[12..24], "revision and count");
    assert_eq!(
        u32::from_le_bytes(rewritten[24..28].try_into().unwrap()) as usize,
        (golden.len() - 32) + expected.len(),
    );
    for index in 0..expected.len() {
        let v1_record = &golden[32 + index * 72..32 + (index + 1) * 72];
        let v2_record = &rewritten[32 + index * 73..32 + (index + 1) * 73];
        assert_eq!(
            &v2_record[..72],
            v1_record,
            "v1 record {index} must be preserved byte-for-byte as the v2 prefix"
        );
        assert_eq!(v2_record[72], 0, "migrated kind must stay zero");
    }
}

#[test]
fn hostile_v2_fixture_round_trips_byte_for_byte() {
    let golden = read_go_fixture("server/storage/hostile/testdata/hostile-mobs-v2.bin");
    assert_eq!(golden.len(), 32 + 3 * 73);
    assert_eq!(
        u32::from_le_bytes(golden[8..12].try_into().unwrap()),
        HOSTILE_CURRENT_SCHEMA,
    );
    let decoded = decode_hostile_mobs(&golden).expect("decode committed hostile v2 fixture");
    let reencoded = encode_hostile_mobs(&HostileMobsSave {
        revision: decoded.revision,
        records: decoded.records.clone(),
    })
    .expect("re-encode committed hostile v2 fixture");
    assert_eq!(
        reencoded, golden,
        "the Rust encoder must reproduce the committed Go bytes exactly"
    );
}

#[test]
fn hostile_decode_rejects_future_versions() {
    let encoded = encode_hostile_mobs(&HostileMobsSave {
        revision: 1,
        records: vec![hostile_nightcrawler(1)],
    })
    .expect("encode hostile mobs");

    let mut future_schema = encoded.clone();
    future_schema[8..12].copy_from_slice(&(HOSTILE_CURRENT_SCHEMA + 1).to_le_bytes());
    assert_eq!(
        decode_hostile_mobs(&future_schema).unwrap_err(),
        StorageError::FutureVersion("hostile schema: 3".to_owned()),
    );

    let mut future_envelope = encoded.clone();
    future_envelope[4..8].copy_from_slice(&(HOSTILE_ENVELOPE_VERSION + 1).to_le_bytes());
    assert_eq!(
        decode_hostile_mobs(&future_envelope).unwrap_err(),
        StorageError::FutureVersion("hostile envelope version: 2".to_owned()),
    );
}

#[test]
fn hostile_decode_rejects_corrupt_and_partial_records() {
    let encoded = encode_hostile_mobs(&HostileMobsSave {
        revision: 3,
        records: vec![hostile_nightcrawler(1), hostile_bone_thrower(2)],
    })
    .expect("encode hostile mobs");

    for length in [0, 4, 31, 32, 40, 33, encoded.len() - 1] {
        assert!(
            decode_hostile_mobs(&encoded[..length]).is_err(),
            "truncated hostile file of {length} bytes must be rejected"
        );
    }

    let mut oversized_count = encoded.clone();
    oversized_count[20..24].copy_from_slice(&(MAX_HOSTILE_MOBS as u32 + 1).to_le_bytes());
    assert!(decode_hostile_mobs(&oversized_count).is_err());

    let mut bad_crc = encoded.clone();
    bad_crc[60] ^= 0xff;
    assert!(decode_hostile_mobs(&bad_crc).is_err());

    let mut unsorted = encoded.clone();
    unsorted[32..40].copy_from_slice(&9u64.to_le_bytes());
    reseal_hostile(&mut unsorted);
    assert!(decode_hostile_mobs(&unsorted).is_err());

    let mut bad_cooldown = encoded.clone();
    bad_cooldown[32 + 42] = 21;
    reseal_hostile(&mut bad_cooldown);
    assert!(decode_hostile_mobs(&bad_cooldown).is_err());

    let mut bad_distant = encoded.clone();
    bad_distant[32 + 70..32 + 72].copy_from_slice(&601u16.to_le_bytes());
    reseal_hostile(&mut bad_distant);
    assert!(decode_hostile_mobs(&bad_distant).is_err());

    let mut bad_kind = encoded.clone();
    bad_kind[32 + 72] = 2;
    reseal_hostile(&mut bad_kind);
    assert!(decode_hostile_mobs(&bad_kind).is_err());

    let mut bad_bool = encoded.clone();
    bad_bool[32 + 36] = 2;
    reseal_hostile(&mut bad_bool);
    assert!(decode_hostile_mobs(&bad_bool).is_err());

    let mut bad_target = encoded.clone();
    bad_target[32 + 73 + 45] = 0;
    reseal_hostile(&mut bad_target);
    assert!(decode_hostile_mobs(&bad_target).is_err());
}

#[test]
fn hostile_encode_rejects_invalid_saves() {
    assert!(
        encode_hostile_mobs(&HostileMobsSave {
            revision: 0,
            records: vec![hostile_nightcrawler(1)],
        })
        .is_err()
    );

    let duplicate = HostileMobsSave {
        revision: 1,
        records: vec![hostile_nightcrawler(1), hostile_nightcrawler(1)],
    };
    assert!(encode_hostile_mobs(&duplicate).is_err());

    let mut zero_id = hostile_nightcrawler(0);
    assert!(
        encode_hostile_mobs(&HostileMobsSave {
            revision: 1,
            records: vec![zero_id.clone()],
        })
        .is_err()
    );
    zero_id.id = 1;
    zero_id.health = 0;
    assert!(
        encode_hostile_mobs(&HostileMobsSave {
            revision: 1,
            records: vec![zero_id],
        })
        .is_err()
    );

    let mut stale_target = hostile_nightcrawler(1);
    stale_target.player_id =
        PlayerId::from_bytes([0x6f, 0xce, 0x82, 0x77, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
    assert!(
        encode_hostile_mobs(&HostileMobsSave {
            revision: 1,
            records: vec![stale_target],
        })
        .is_err()
    );
}

/// Recomputes the hostile envelope checksum in place so a semantic mutation is
/// rejected by the record validator rather than the CRC gate.
fn reseal_hostile(bytes: &mut [u8]) {
    let checksum = crc32c_join(&[&bytes[8..28], &bytes[32..]]);
    bytes[28..32].copy_from_slice(&checksum.to_le_bytes());
}

fn hostile_nightcrawler(id: u64) -> HostileMob {
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

/// The committed fixture's "idle" nightcrawler.
fn hostile_idle(id: u64) -> HostileMob {
    HostileMob {
        position: [0.5, 64.0, -9.75],
        velocity: [0.0, -3.25, 0.0],
        yaw: -2.5,
        health: 20,
        ..hostile_nightcrawler(id)
    }
}

/// The committed fixture's "far" nightcrawler.
fn hostile_far(id: u64) -> HostileMob {
    HostileMob {
        position: [8.5, 65.5, 9.75],
        velocity: [2.0, 0.0, -2.0],
        on_ground: true,
        yaw: 3.0,
        health: 1,
        burn_cooldown: 19,
        distant_ticks: 600,
        ..hostile_nightcrawler(id)
    }
}

fn hostile_bone_thrower(id: u64) -> HostileMob {
    HostileMob {
        id,
        dimension: 0,
        position: [-12.5, 70.25, 3.5],
        velocity: [-1.25, 0.0, 0.5],
        on_ground: true,
        yaw: 1.25,
        health: 17,
        attack_cooldown: 3,
        hurt_cooldown: 1,
        burn_cooldown: 5,
        has_target: true,
        player_id: PlayerId::from_bytes([
            0x6f, 0xce, 0x82, 0x77, 0xa9, 0x33, 0x46, 0xcb, 0x9a, 0x1f, 0xda, 0x13, 0xb7, 0xee,
            0x56, 0x44,
        ]),
        next_repath_ticks: 905,
        distant_ticks: 120,
        kind: 1,
    }
}

#[test]
fn region_bank_rejects_wrong_slot_count() {
    for count in [REGION_SLOTS - 1, REGION_SLOTS + 1] {
        let entries = vec![RegionEntry::default(); count];
        assert!(matches!(
            RegionBank::try_from_entries(1, entries),
            Err(StorageError::Corrupt(_))
        ));
    }
}

#[test]
fn region_bank_constructor_preserves_valid_shape() {
    let mut entries = vec![RegionEntry::default(); REGION_SLOTS];
    entries[0] = RegionEntry {
        offset_sector: DATA_START_SECTOR,
        sector_count: 1,
        payload_length: 0,
        revision: 1,
        payload_crc32c: 0,
    };
    let bank = RegionBank::try_from_entries(1, entries).expect("construct region bank");
    assert_eq!(bank.entries.len(), REGION_SLOTS);
    let key = region_bank_key();
    let encoded = encode_region_bank(key, &bank).expect("encode region bank");
    let decoded =
        decode_region_bank(key, &encoded, 16 * SECTOR_SIZE as i64).expect("decode region bank");
    assert_eq!(decoded, bank);
}

#[test]
fn region_bank_into_is_atomic() {
    let key = RegionKey {
        dimension: -3,
        x: -1,
        z: 2,
    };
    let mut bank = RegionBank::empty();
    bank.generation = 1;
    bank.entries[0] = RegionEntry {
        offset_sector: DATA_START_SECTOR,
        sector_count: 1,
        payload_length: 0,
        revision: 1,
        payload_crc32c: 0,
    };

    let mut short = vec![0xA5; BANK_SIZE - 1];
    assert_eq!(
        encode_region_bank_into(key, &bank, &mut short),
        Err(StorageError::OutputTooSmall {
            needed: BANK_SIZE,
            available: BANK_SIZE - 1,
        })
    );
    assert!(short.iter().all(|byte| *byte == 0xA5));

    let mut invalid = bank.clone();
    invalid.entries[0].offset_sector = DATA_START_SECTOR - 1;
    assert!(matches!(
        encode_region_bank_into(key, &invalid, &mut short),
        Err(StorageError::Corrupt(_))
    ));
    assert!(short.iter().all(|byte| *byte == 0xA5));

    let owned = encode_region_bank(key, &bank).expect("encode region bank");
    let mut exact = vec![0xA5; BANK_SIZE];
    assert_eq!(
        encode_region_bank_into(key, &bank, &mut exact),
        Ok(BANK_SIZE)
    );
    assert_eq!(exact, owned);

    let mut larger = vec![0xA5; BANK_SIZE + 7];
    assert_eq!(
        encode_region_bank_into(key, &bank, &mut larger),
        Ok(BANK_SIZE)
    );
    assert_eq!(&larger[..BANK_SIZE], &owned);
    assert!(larger[BANK_SIZE..].iter().all(|byte| *byte == 0xA5));
}

#[test]
fn region_superblock_into_preserves_tail() {
    let key = RegionKey {
        dimension: -3,
        x: -1,
        z: 2,
    };
    let mut short = vec![0xA5; SECTOR_SIZE as usize - 1];
    assert_eq!(
        encode_superblock_into(key, &mut short),
        Err(StorageError::OutputTooSmall {
            needed: SECTOR_SIZE as usize,
            available: SECTOR_SIZE as usize - 1,
        })
    );
    assert!(short.iter().all(|byte| *byte == 0xA5));

    let owned = encode_superblock(key);
    let mut exact = vec![0xA5; SECTOR_SIZE as usize];
    assert_eq!(
        encode_superblock_into(key, &mut exact),
        Ok(SECTOR_SIZE as usize)
    );
    assert_eq!(exact, owned);

    let mut larger = vec![0xA5; SECTOR_SIZE as usize + 7];
    assert_eq!(
        encode_superblock_into(key, &mut larger),
        Ok(SECTOR_SIZE as usize)
    );
    assert_eq!(&larger[..SECTOR_SIZE as usize], &owned);
    assert!(
        larger[SECTOR_SIZE as usize..]
            .iter()
            .all(|byte| *byte == 0xA5)
    );
}

#[test]
fn region_go_crc_reference() {
    let key = RegionKey {
        dimension: -3,
        x: -1,
        z: 2,
    };
    let superblock = encode_superblock(key);
    assert_eq!(u32_at(&superblock, 12), (-3i32) as u32);
    assert_eq!(u32_at(&superblock, 16), (-1i32) as u32);
    assert_eq!(u32_at(&superblock, 20), 2);
    assert_eq!(u32_at(&superblock, 4092), 0xef55_4c52);

    let mut bank = RegionBank::empty();
    bank.generation = 1;
    bank.entries[0] = RegionEntry {
        offset_sector: DATA_START_SECTOR,
        sector_count: 1,
        payload_length: 0,
        revision: 1,
        payload_crc32c: 0,
    };
    let encoded = encode_region_bank(key, &bank).expect("encode region bank");
    assert_eq!(u32_at(&encoded, 60), 0x24d2_71a5);
}

#[test]
fn region_superblock_exact_layout_and_round_trip() {
    let key = RegionKey {
        dimension: -7,
        x: -2,
        z: 3,
    };
    let encoded = encode_superblock(key);
    assert_eq!(&encoded[0..4], b"MCGR");
    let fields: [(usize, u32); 9] = [
        (4, 1),
        (8, 4096),
        (12, 0xffff_fff9),
        (16, 0xffff_fffe),
        (20, 3),
        (24, 1),
        (28, 8),
        (32, 7),
        (36, 15),
    ];
    for (offset, want) in fields {
        assert_eq!(
            u32_at(&encoded, offset),
            want,
            "superblock field at {offset}"
        );
    }
    assert!(encoded[40..4092].iter().all(|byte| *byte == 0));
    assert_eq!(u32_at(&encoded, 4092), crc32c(&encoded[..4092]));
    // Frozen layout constants the format contract depends on.
    assert_eq!(BANK_A_START_SECTOR, 1);
    assert_eq!(BANK_B_START_SECTOR, 8);
    assert_eq!(BANK_SIZE, 7 * SECTOR_SIZE as usize);
    assert_eq!(DATA_START_SECTOR, 15);
    assert_eq!(REGION_SLOTS, 1024);
    assert_eq!(MAX_COMPRESSED_CHUNK, 1 << 20);
    decode_superblock(key, &encoded).expect("decode superblock");
}

#[test]
fn region_superblock_rejects_corruption() {
    let key = RegionKey {
        dimension: 0,
        x: -2,
        z: 3,
    };
    let valid = encode_superblock(key);

    let mut trailing = valid.to_vec();
    trailing.push(0);
    let cases: Vec<(&str, Vec<u8>, RegionKey)> = vec![
        (
            "wrong magic",
            mutate_superblock(&valid, 0, 0xff, false),
            key,
        ),
        ("past version", put_super_u32(&valid, 4, 0), key),
        ("future version", put_super_u32(&valid, 4, 2), key),
        ("wrong sector size", put_super_u32(&valid, 8, 2048), key),
        (
            "wrong dimension",
            valid.to_vec(),
            RegionKey {
                dimension: 1,
                x: -2,
                z: 3,
            },
        ),
        (
            "wrong x",
            valid.to_vec(),
            RegionKey {
                dimension: 0,
                x: -1,
                z: 3,
            },
        ),
        (
            "wrong z",
            valid.to_vec(),
            RegionKey {
                dimension: 0,
                x: -2,
                z: 4,
            },
        ),
        ("wrong bank A sector", put_super_u32(&valid, 24, 2), key),
        ("wrong bank B sector", put_super_u32(&valid, 28, 9), key),
        ("wrong bank size", put_super_u32(&valid, 32, 6), key),
        ("wrong data start", put_super_u32(&valid, 36, 14), key),
        (
            "nonzero reserved byte",
            mutate_superblock(&valid, 40, 1, true),
            key,
        ),
        ("invalid CRC", mutate_superblock(&valid, 100, 1, false), key),
        ("short", valid[..4095].to_vec(), key),
        ("trailing", trailing, key),
    ];
    for (name, bytes, key) in cases {
        let err = decode_superblock(key, &bytes)
            .err()
            .unwrap_or_else(|| panic!("{name}: expected rejection"));
        assert_eq!(
            matches!(err, StorageError::FutureVersion(_)),
            name == "future version",
            "{name}: unexpected error class {err:?}"
        );
    }
}

#[test]
fn region_bank_round_trip_and_selection() {
    let key = region_bank_key();
    let mut want = RegionBank::empty();
    want.generation = 9;
    want.entries[31] = RegionEntry {
        offset_sector: 15,
        sector_count: 2,
        payload_length: 5000,
        revision: 7,
        payload_crc32c: 0x1234_5678,
    };
    let encoded = encode_region_bank(key, &want).expect("encode region bank");
    let got =
        decode_region_bank(key, &encoded, 17 * SECTOR_SIZE as i64).expect("decode region bank");
    assert_eq!(got, want);

    let mut older = RegionBank::empty();
    older.generation = 8;
    let (selected, index) = select_region_bank(Ok(older), Ok(got)).expect("select region bank");
    assert_eq!(index, 1);
    assert_eq!(selected.generation, 9);
}

#[test]
fn region_bank_exact_layout() {
    let key = RegionKey {
        dimension: -7,
        x: -2,
        z: 3,
    };
    let mut bank = RegionBank::empty();
    bank.generation = 9;
    bank.entries[31] = RegionEntry {
        offset_sector: 15,
        sector_count: 2,
        payload_length: 5000,
        revision: 7,
        payload_crc32c: 0x1234_5678,
    };
    let encoded = encode_region_bank(key, &bank).expect("encode region bank");
    assert_eq!(&encoded[0..4], b"MCGB");
    let fields: [(usize, u32); 9] = [
        (4, 1),
        (8, 4096),
        (12, 0xffff_fff9),
        (16, 0xffff_fffe),
        (20, 3),
        (32, 1024),
        (36, 24),
        (40, 7),
        (44, 15),
    ];
    for (offset, want) in fields {
        assert_eq!(u32_at(&encoded, offset), want, "bank field at {offset}");
    }
    assert_eq!(u64_at(&encoded, 24), 9);
    assert!(encoded[48..60].iter().all(|byte| *byte == 0));

    let entry = 64 + 31 * 24;
    assert_eq!(u32_at(&encoded, entry), 15);
    assert_eq!(u32_at(&encoded, entry + 4), 2);
    assert_eq!(u32_at(&encoded, entry + 8), 5000);
    assert_eq!(u64_at(&encoded, entry + 12), 7);
    assert_eq!(u32_at(&encoded, entry + 20), 0x1234_5678);
    assert!(encoded[64 + 1024 * 24..].iter().all(|byte| *byte == 0));

    let mut checksum_input = encoded.to_vec();
    checksum_input[60..64].copy_from_slice(&[0, 0, 0, 0]);
    assert_eq!(u32_at(&encoded, 60), crc32c(&checksum_input));
}

#[test]
fn region_bank_accepts_empty_generation_zero_and_max_generation() {
    let key = region_bank_key();
    for generation in [0u64, u64::MAX] {
        let mut bank = RegionBank::empty();
        bank.generation = generation;
        let encoded = encode_region_bank(key, &bank).expect("encode bank");
        let got = decode_region_bank(key, &encoded, DATA_START_SECTOR as i64 * SECTOR_SIZE as i64)
            .expect("decode bank");
        assert_eq!(got, bank, "generation {generation}");
    }
}

#[test]
fn region_bank_rejects_corruption() {
    let key = region_bank_key();
    let file_size = 17 * SECTOR_SIZE as i64;
    let mut base = RegionBank::empty();
    base.generation = 9;
    base.entries[0] = RegionEntry {
        offset_sector: 15,
        sector_count: 2,
        payload_length: 5000,
        revision: 7,
        payload_crc32c: 0x1234_5678,
    };
    let valid = encode_region_bank(key, &base).expect("encode region bank");

    let mut trailing = valid.to_vec();
    trailing.push(0);
    let cases: Vec<(&str, Vec<u8>, i64)> = vec![
        ("wrong magic", mutate_bank(&valid, 0, 1, false), file_size),
        ("past version", put_bank_u32(&valid, 4, 0), file_size),
        ("future version", put_bank_u32(&valid, 4, 2), file_size),
        (
            "wrong sector size",
            put_bank_u32(&valid, 8, 2048),
            file_size,
        ),
        (
            "wrong entry count",
            put_bank_u32(&valid, 32, 1023),
            file_size,
        ),
        ("wrong entry size", put_bank_u32(&valid, 36, 20), file_size),
        ("wrong bank sectors", put_bank_u32(&valid, 40, 6), file_size),
        ("wrong data start", put_bank_u32(&valid, 44, 14), file_size),
        (
            "nonzero header reserved byte",
            mutate_bank(&valid, 48, 1, true),
            file_size,
        ),
        ("invalid CRC", mutate_bank(&valid, 100, 1, false), file_size),
        (
            "generation zero with entry",
            put_bank_u64(&valid, 24, 0),
            file_size,
        ),
        (
            "offset inside headers",
            put_bank_entry_u32(&valid, 0, 0, 14),
            file_size,
        ),
        (
            "zero sector count",
            put_bank_entry_u32(&valid, 0, 4, 0),
            file_size,
        ),
        (
            "payload over one MiB",
            put_bank_entry_u32(&valid, 0, 8, (1 << 20) + 1),
            272 * SECTOR_SIZE as i64,
        ),
        (
            "payload exceeds extent",
            put_bank_entry_u32(&valid, 0, 4, 1),
            file_size,
        ),
        (
            "extent past EOF",
            put_bank_entry_u32(&valid, 0, 0, 16),
            file_size,
        ),
        (
            "uint32 extent overflow",
            put_bank_entry_u32(&valid, 0, 0, u32::MAX),
            i64::MAX,
        ),
        (
            "absent entry with nonzero tail",
            put_bank_entry_u32(&valid, 1, 0, 1),
            file_size,
        ),
        (
            "zero revision",
            put_bank_entry_u64(&valid, 0, 12, 0),
            file_size,
        ),
        (
            "nonzero trailing padding",
            mutate_bank(&valid, 64 + 1024 * 24, 1, true),
            file_size,
        ),
        ("short", valid[..valid.len() - 1].to_vec(), file_size),
        ("trailing", trailing, file_size),
    ];
    for (name, bytes, size) in cases {
        let err = decode_region_bank(key, &bytes, size)
            .err()
            .unwrap_or_else(|| panic!("{name}: expected rejection"));
        assert_eq!(
            matches!(err, StorageError::FutureVersion(_)),
            name == "future version",
            "{name}: unexpected error class {err:?}"
        );
    }

    // Overlapping extents need a second populated slot.
    let mut overlap = put_bank_entry_u32(&valid, 1, 0, 16);
    overlap = put_bank_entry_u32(&overlap, 1, 4, 1);
    overlap = put_bank_entry_u32(&overlap, 1, 8, 1);
    overlap = put_bank_entry_u64(&overlap, 1, 12, 8);
    assert!(decode_region_bank(key, &overlap, file_size).is_err());
}

#[test]
fn region_encode_bank_rejects_invalid_structure() {
    let key = region_bank_key();
    let empty = RegionBank::empty();
    let valid_entry = RegionEntry {
        offset_sector: 15,
        sector_count: 1,
        payload_length: 1,
        revision: 1,
        payload_crc32c: 0,
    };

    let mut generation_zero_with_entry = empty.clone();
    generation_zero_with_entry.entries[0] = valid_entry;
    assert!(encode_region_bank(key, &generation_zero_with_entry).is_err());

    let mut absent_with_tail = empty.clone();
    absent_with_tail.generation = 1;
    absent_with_tail.entries[0] = RegionEntry {
        sector_count: 1,
        ..RegionEntry::default()
    };
    assert!(encode_region_bank(key, &absent_with_tail).is_err());

    let mut zero_sector_count = empty.clone();
    zero_sector_count.generation = 1;
    zero_sector_count.entries[0] = RegionEntry {
        offset_sector: 15,
        sector_count: 0,
        payload_length: 1,
        revision: 1,
        payload_crc32c: 0,
    };
    assert!(encode_region_bank(key, &zero_sector_count).is_err());

    let mut oversized = empty.clone();
    oversized.generation = 1;
    oversized.entries[0] = RegionEntry {
        offset_sector: 15,
        sector_count: 257,
        payload_length: (1 << 20) + 1,
        revision: 1,
        payload_crc32c: 0,
    };
    assert!(encode_region_bank(key, &oversized).is_err());

    let mut zero_revision = empty.clone();
    zero_revision.generation = 1;
    zero_revision.entries[0] = RegionEntry {
        offset_sector: 15,
        sector_count: 1,
        payload_length: 1,
        revision: 0,
        payload_crc32c: 0,
    };
    assert!(encode_region_bank(key, &zero_revision).is_err());

    let mut overlapping = empty.clone();
    overlapping.generation = 1;
    overlapping.entries[0] = RegionEntry {
        offset_sector: 15,
        sector_count: 2,
        payload_length: 1,
        revision: 1,
        payload_crc32c: 0,
    };
    overlapping.entries[1] = RegionEntry {
        offset_sector: 16,
        sector_count: 1,
        payload_length: 1,
        revision: 2,
        payload_crc32c: 0,
    };
    assert!(encode_region_bank(key, &overlapping).is_err());

    let mut committed = empty.clone();
    committed.generation = 9;
    assert!(encode_region_bank(key, &committed).is_ok());
}

#[test]
fn region_select_bank_validity_and_ties() {
    let mut committed = RegionBank::empty();
    committed.generation = 9;
    let mut newer = RegionBank::empty();
    newer.generation = 10;
    let mut different = RegionBank::empty();
    different.generation = 9;
    different.entries[0] = RegionEntry {
        offset_sector: 15,
        sector_count: 1,
        payload_length: 1,
        revision: 1,
        payload_crc32c: 0,
    };
    let invalid = || Err(StorageError::Corrupt("region bank: injected".to_owned()));

    assert!(select_region_bank(invalid(), invalid()).is_err());
    let (bank, index) = select_region_bank(Ok(committed.clone()), invalid()).expect("only A valid");
    assert_eq!(index, 0);
    assert_eq!(bank, committed);
    let (bank, index) = select_region_bank(invalid(), Ok(committed.clone())).expect("only B valid");
    assert_eq!(index, 1);
    assert_eq!(bank, committed);
    let (bank, index) =
        select_region_bank(Ok(newer.clone()), Ok(committed.clone())).expect("newer A");
    assert_eq!(index, 0);
    assert_eq!(bank, newer);
    let (bank, index) =
        select_region_bank(Ok(committed.clone()), Ok(newer.clone())).expect("newer B");
    assert_eq!(index, 1);
    assert_eq!(bank, newer);
    let (bank, index) =
        select_region_bank(Ok(committed.clone()), Ok(committed.clone())).expect("identical tie");
    assert_eq!(index, 0);
    assert_eq!(bank, committed);
    assert!(select_region_bank(Ok(committed.clone()), Ok(different)).is_err());

    // A structurally valid generation-zero bank is an uncommitted standby: it
    // loses against a committed peer but is an error when the peer also fails.
    let standby = RegionBank::empty();
    assert!(select_region_bank(Ok(standby.clone()), invalid()).is_err());
    assert!(select_region_bank(Ok(standby.clone()), Ok(standby.clone())).is_err());
    let (bank, index) =
        select_region_bank(Ok(standby), Ok(committed.clone())).expect("standby A and committed B");
    assert_eq!(index, 1);
    assert_eq!(bank, committed);
}

#[test]
fn region_for_uses_floor_division() {
    let cases: [(i32, i32, usize); 9] = [
        (-33, -2, 31 * 32 + 31),
        (-32, -1, 0),
        (-31, -1, 32 + 1),
        (-1, -1, 31 * 32 + 31),
        (0, 0, 0),
        (1, 0, 32 + 1),
        (31, 0, 31 * 32 + 31),
        (32, 1, 0),
        (33, 1, 32 + 1),
    ];
    for (chunk, region, slot) in cases {
        let (key, got_slot) = region_for(ChunkKey {
            dimension: 0,
            x: chunk,
            z: chunk,
        });
        assert_eq!((key.x, key.z), (region, region), "chunk {chunk}");
        assert_eq!(got_slot, slot, "chunk {chunk} slot");
    }
}

#[test]
fn region_for_handles_min_int32() {
    let (key, slot) = region_for(ChunkKey {
        dimension: 0,
        x: i32::MIN,
        z: i32::MIN,
    });
    assert_eq!((key.x, key.z), (-67_108_864, -67_108_864));
    assert_eq!(slot, 0);
}

fn region_bank_key() -> RegionKey {
    RegionKey {
        dimension: 0,
        x: -2,
        z: 3,
    }
}

fn mutate_superblock(block: &[u8; 4096], offset: usize, mask: u8, reseal: bool) -> Vec<u8> {
    let mut mutated = block.to_vec();
    mutated[offset] ^= mask;
    if reseal {
        reseal_superblock(&mut mutated);
    }
    mutated
}

fn put_super_u32(block: &[u8; 4096], offset: usize, value: u32) -> Vec<u8> {
    let mut mutated = block.to_vec();
    mutated[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
    reseal_superblock(&mut mutated);
    mutated
}

fn reseal_superblock(block: &mut [u8]) {
    let checksum = crc32c(&block[..4092]);
    block[4092..4096].copy_from_slice(&checksum.to_le_bytes());
}

fn mutate_bank(bank: &[u8], offset: usize, mask: u8, reseal: bool) -> Vec<u8> {
    let mut mutated = bank.to_vec();
    mutated[offset] ^= mask;
    if reseal {
        reseal_bank(&mut mutated);
    }
    mutated
}

fn put_bank_u32(bank: &[u8], offset: usize, value: u32) -> Vec<u8> {
    let mut mutated = bank.to_vec();
    mutated[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
    reseal_bank(&mut mutated);
    mutated
}

fn put_bank_u64(bank: &[u8], offset: usize, value: u64) -> Vec<u8> {
    let mut mutated = bank.to_vec();
    mutated[offset..offset + 8].copy_from_slice(&value.to_le_bytes());
    reseal_bank(&mut mutated);
    mutated
}

fn put_bank_entry_u32(bank: &[u8], slot: usize, field: usize, value: u32) -> Vec<u8> {
    put_bank_u32(bank, 64 + slot * 24 + field, value)
}

fn put_bank_entry_u64(bank: &[u8], slot: usize, field: usize, value: u64) -> Vec<u8> {
    put_bank_u64(bank, 64 + slot * 24 + field, value)
}

fn reseal_bank(bank: &mut [u8]) {
    bank[60..64].copy_from_slice(&[0, 0, 0, 0]);
    let checksum = crc32c(bank);
    bank[60..64].copy_from_slice(&checksum.to_le_bytes());
}

fn u32_at(encoded: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes(encoded[offset..offset + 4].try_into().expect("four bytes"))
}

fn u64_at(encoded: &[u8], offset: usize) -> u64 {
    u64::from_le_bytes(encoded[offset..offset + 8].try_into().expect("eight bytes"))
}
#[test]
fn metadata_current_schema_round_trip_preserves_bytes() {
    let metadata = Metadata {
        format_version: METADATA_CURRENT_VERSION,
        seed: -42,
        spawn_dimension: -3,
        spawn_anchor: MetadataChunkPos { x: 7, z: -11 },
        world_time_ticks: 987_654,
        day_phase_offset: 6_781,
        weather_kind: 2,
        weather_ticks_remaining: 1_000,
        depths_spawn_anchor: MetadataChunkPos { x: -4, z: 9 },
        depths_seed_salt: 0x9E37_79B9_7F4A_7C15,
        difficulty: 2,
    };
    let one = encode_world_metadata(&metadata).expect("encode metadata");
    let two = encode_world_metadata(&metadata).expect("encode metadata again");
    assert_eq!(one, two, "same metadata must encode deterministically");
    assert_eq!(one.len(), 12 + 62 + 4);
    assert_eq!(&one[0..4], b"MCGM");
    assert_eq!(u32_at(&one, 4), METADATA_CURRENT_VERSION);
    assert_eq!(u32_at(&one, 8), 62);
    assert_eq!(u32_at(&one, one.len() - 4), crc32c(&one[..one.len() - 4]));
    assert_eq!(
        decode_world_metadata(&one).expect("decode metadata"),
        metadata
    );
}

#[test]
fn metadata_rejects_malformed_bytes() {
    let valid = encode_world_metadata(&Metadata {
        format_version: METADATA_CURRENT_VERSION,
        seed: 42,
        spawn_dimension: 0,
        spawn_anchor: MetadataChunkPos { x: 3, z: -2 },
        ..default_metadata()
    })
    .expect("encode metadata");

    let mut crc_corrupt = valid.clone();
    crc_corrupt[12] ^= 0xff;
    let mut future_version = valid.clone();
    future_version[4..8].copy_from_slice(&(METADATA_CURRENT_VERSION + 1).to_le_bytes());
    let mut past_version = valid.clone();
    past_version[4..8].copy_from_slice(&0u32.to_le_bytes());
    let mut wrong_payload_length = valid.clone();
    wrong_payload_length[8..12].copy_from_slice(&19u32.to_le_bytes());
    let mut trailing = valid.clone();
    trailing.push(0);

    let cases: Vec<(&str, Vec<u8>)> = vec![
        ("CRC corruption", crc_corrupt),
        ("future version", future_version),
        ("past version", past_version),
        ("short", valid[..valid.len() - 1].to_vec()),
        ("trailing", trailing),
        ("wrong payload length", wrong_payload_length),
    ];
    for (name, bytes) in cases {
        let err = decode_world_metadata(&bytes)
            .err()
            .unwrap_or_else(|| panic!("{name}: expected rejection"));
        assert_eq!(
            matches!(err, StorageError::FutureVersion(_)),
            name == "future version",
            "{name}: unexpected error class {err:?}"
        );
    }
}

#[test]
fn metadata_legacy_versions_migrate_to_the_same_normalized_result() {
    let spawn_anchor = MetadataChunkPos { x: 5, z: -6 };
    // Each legacy payload is a pure tail prefix of v6, so the shared fields
    // must survive and the missing tails must take their documented defaults.
    let mut v1 = Vec::new();
    append_u64(&mut v1, 123_456_789);
    append_u32(&mut v1, 0);
    append_u32(&mut v1, spawn_anchor.x as u32);
    append_u32(&mut v1, spawn_anchor.z as u32);

    let mut v2 = v1.clone();
    append_u64(&mut v2, 123_456);

    let mut v3 = v2.clone();
    append_u64(&mut v3, 678);

    let mut v4 = v3.clone();
    append_u8(&mut v4, 1);
    append_u32(&mut v4, 5_000);

    let mut v5 = v4.clone();
    append_u32(&mut v5, 2);
    append_u32(&mut v5, spawn_anchor.x as u32);
    append_u32(&mut v5, spawn_anchor.z as u32);
    append_u64(&mut v5, 0x9E37_79B9_7F4A_7C15);

    let expected_common = Metadata {
        format_version: METADATA_CURRENT_VERSION,
        seed: 123_456_789,
        spawn_dimension: 0,
        spawn_anchor,
        ..default_metadata()
    };

    let decoded_v1 = decode_world_metadata(&seal_metadata(METADATA_V1, v1)).expect("decode v1");
    let mut want_v1 = expected_common.clone();
    want_v1.depths_spawn_anchor = spawn_anchor;
    assert_eq!(decoded_v1, want_v1);

    let decoded_v2 = decode_world_metadata(&seal_metadata(METADATA_V2, v2)).expect("decode v2");
    let mut want_v2 = expected_common.clone();
    want_v2.world_time_ticks = 123_456;
    want_v2.depths_spawn_anchor = spawn_anchor;
    assert_eq!(decoded_v2, want_v2);

    let decoded_v3 = decode_world_metadata(&seal_metadata(METADATA_V3, v3)).expect("decode v3");
    let mut want_v3 = expected_common.clone();
    want_v3.world_time_ticks = 123_456;
    want_v3.day_phase_offset = 678;
    want_v3.depths_spawn_anchor = spawn_anchor;
    assert_eq!(decoded_v3, want_v3);

    let decoded_v4 = decode_world_metadata(&seal_metadata(METADATA_V4, v4)).expect("decode v4");
    let mut want_v4 = expected_common.clone();
    want_v4.world_time_ticks = 123_456;
    want_v4.day_phase_offset = 678;
    want_v4.weather_kind = 1;
    want_v4.weather_ticks_remaining = 5_000;
    want_v4.depths_spawn_anchor = spawn_anchor;
    assert_eq!(decoded_v4, want_v4);

    let decoded_v5 = decode_world_metadata(&seal_metadata(METADATA_V5, v5)).expect("decode v5");
    let mut want_v5 = expected_common.clone();
    want_v5.world_time_ticks = 123_456;
    want_v5.day_phase_offset = 678;
    want_v5.weather_kind = 1;
    want_v5.weather_ticks_remaining = 5_000;
    want_v5.depths_spawn_anchor = spawn_anchor;
    assert_eq!(decoded_v5, want_v5);
}

#[test]
fn metadata_v5_with_wrong_dimension_count_is_rejected() {
    let mut payload = Vec::new();
    append_u64(&mut payload, 1);
    append_u32(&mut payload, 0);
    append_u32(&mut payload, 0);
    append_u32(&mut payload, 0);
    append_u64(&mut payload, 0);
    append_u64(&mut payload, 0);
    append_u8(&mut payload, 1);
    append_u32(&mut payload, 0);
    append_u32(&mut payload, 3);
    append_u32(&mut payload, 0);
    append_u32(&mut payload, 0);
    append_u64(&mut payload, 0);
    let err = decode_world_metadata(&seal_metadata(METADATA_V5, payload))
        .expect_err("wrong dimension count must be rejected");
    assert!(matches!(err, StorageError::Corrupt(_)), "{err:?}");
}

#[test]
fn metadata_v6_rejects_invalid_difficulty_and_preserves_raw_weather() {
    for difficulty in [3u8, 255] {
        let mut payload = Vec::new();
        append_u64(&mut payload, 1);
        append_u32(&mut payload, 0);
        append_u32(&mut payload, 0);
        append_u32(&mut payload, 0);
        append_u64(&mut payload, 0);
        append_u64(&mut payload, 0);
        append_u8(&mut payload, 1);
        append_u32(&mut payload, 0);
        append_u32(&mut payload, 2);
        append_u32(&mut payload, 0);
        append_u32(&mut payload, 0);
        append_u64(&mut payload, 0);
        append_u8(&mut payload, difficulty);
        assert!(decode_world_metadata(&seal_metadata(METADATA_CURRENT_VERSION, payload)).is_err());
    }
    for weather in [3u8, 255] {
        let mut payload = Vec::new();
        append_u64(&mut payload, 1);
        append_u32(&mut payload, 0);
        append_u32(&mut payload, 0);
        append_u32(&mut payload, 0);
        append_u64(&mut payload, 0);
        append_u64(&mut payload, 0);
        append_u8(&mut payload, weather);
        append_u32(&mut payload, 0);
        append_u32(&mut payload, 2);
        append_u32(&mut payload, 0);
        append_u32(&mut payload, 0);
        append_u64(&mut payload, 0);
        append_u8(&mut payload, 0);
        let decoded = decode_world_metadata(&seal_metadata(METADATA_CURRENT_VERSION, payload))
            .expect("raw weather bytes must decode");
        assert_eq!(decoded.weather_kind, weather);
    }
}

#[test]
fn metadata_encode_rejects_non_current_version() {
    for version in [0u32, 1, 5, 7] {
        let mut metadata = default_metadata();
        metadata.format_version = version;
        assert!(encode_world_metadata(&metadata).is_err());
    }
    let mut invalid_difficulty = default_metadata();
    invalid_difficulty.difficulty = 3;
    assert!(encode_world_metadata(&invalid_difficulty).is_err());
}

fn default_metadata() -> Metadata {
    Metadata {
        format_version: METADATA_CURRENT_VERSION,
        seed: 0,
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

/// Wraps a legacy payload in the fixed metadata header and seals the CRC-32C.
fn seal_metadata(version: u32, mut payload: Vec<u8>) -> Vec<u8> {
    let mut bytes = Vec::new();
    bytes.extend_from_slice(b"MCGM");
    bytes.extend_from_slice(&version.to_le_bytes());
    bytes.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    bytes.append(&mut payload);
    let checksum = crc32c(&bytes);
    bytes.extend_from_slice(&checksum.to_le_bytes());
    bytes
}

fn append_u8(bytes: &mut Vec<u8>, value: u8) {
    bytes.push(value);
}

fn append_u32(bytes: &mut Vec<u8>, value: u32) {
    bytes.extend_from_slice(&value.to_le_bytes());
}

fn append_u64(bytes: &mut Vec<u8>, value: u64) {
    bytes.extend_from_slice(&value.to_le_bytes());
}
#[test]
fn companion_v5_golden_fixture_round_trips_byte_for_byte() {
    let golden = read_go_fixture("server/storage/companion/testdata/companions-v5.bin");
    assert_eq!(u32_at(&golden, 8), COMPANION_CURRENT_SCHEMA);
    assert_eq!(COMPANION_MAX_FILE_LENGTH, 393_904);

    let save = companion_golden_save();
    let encoded = encode_companions(&save).expect("encode golden companion save");
    assert_eq!(
        encoded, golden,
        "the Rust encoder must reproduce the committed Go v5 bytes exactly"
    );
    let decoded = decode_companions(&golden).expect("decode golden companion save");
    assert_eq!(decoded.source_schema, COMPANION_CURRENT_SCHEMA);
    assert_eq!(decoded.revision, 47);
    assert_eq!(decoded.agent_namespace_id, agent_identity(0x70));
    assert_eq!(decoded.records.len(), 3);
    assert_eq!(decoded.lifecycles.len(), 3);
    assert_eq!(decoded.queues.len(), 1);
    assert_eq!(decoded.lifecycles[0].memory_revision, 11);
    assert_eq!(decoded.lifecycles[0].summary, "阿木记得北边橡树旁的小路。");
    assert_eq!(decoded.lifecycles[1].memory_revision, 0);
    assert!(decoded.lifecycles[1].summary.is_empty());
    assert!(!decoded.lifecycles[2].active);
    assert_eq!(
        decoded.lifecycles[2].tombstone_operation_id,
        agent_identity(0x73)
    );
}

#[test]
fn companion_v1_fixture_migrates_to_v5_with_namespace_and_lifecycles() {
    let golden = read_go_fixture("server/storage/companion/testdata/companions-v1.bin");
    assert_eq!(golden.len(), 32 + 2 * 221);
    assert_eq!(u32_at(&golden, 8), COMPANION_SCHEMA_V1);
    let before = golden.clone();
    let decoded = decode_companions(&golden).expect("decode committed companion v1 fixture");
    assert_eq!(
        golden, before,
        "v1 migration must not rewrite the input bytes"
    );
    assert_eq!(decoded.source_schema, COMPANION_SCHEMA_V1);
    assert_eq!(decoded.revision, 19);
    assert_eq!(decoded.records.len(), 2);
    assert_eq!(decoded.records[0].id, companion_id(1));
    assert_eq!(decoded.records[1].id, companion_id(2));
    assert!(decoded.queues.is_empty());
    assert!(decoded.lifecycles.is_empty());

    let mut lifecycles: Vec<StoredCompanionLifecycle> = decoded
        .records
        .iter()
        .enumerate()
        .map(|(index, body)| StoredCompanionLifecycle {
            id: body.id,
            active: true,
            memory_epoch: 1,
            ..fixture_lifecycle(body.id, index < 4)
        })
        .collect();
    // The canonical-zero mirror keeps an empty string and a zero operation ID.
    for lifecycle in &mut lifecycles {
        lifecycle.memory_revision = 0;
        lifecycle.memory_operation_id = PlayerId::default();
        lifecycle.summary = String::new();
    }
    let reencoded = encode_companions(&CompanionSave {
        revision: 19,
        agent_namespace_id: agent_identity(0x70),
        records: decoded.records.clone(),
        lifecycles,
        queues: Vec::new(),
    })
    .expect("rewrite migrated companion aggregate");
    assert_eq!(reencoded.len(), 560);
    assert_eq!(u32_at(&reencoded, 8), COMPANION_CURRENT_SCHEMA);
    let migrated = decode_companions(&reencoded).expect("decode rewritten companion aggregate");
    assert_eq!(migrated.revision, 19);
    assert_eq!(migrated.records, decoded.records);
    assert!(migrated.queues.is_empty());
}

#[test]
fn companion_legacy_v2_v3_v4_fixtures_decode_without_repair() {
    // Each legacy fixture carries a different number of task/FIFO sections and
    // only v4 adds a summary section.
    for (name, schema, revision, queues) in [
        ("companions-v2.bin", COMPANION_SCHEMA_V2, 41, 1),
        ("companions-v3.bin", COMPANION_SCHEMA_V3, 43, 1),
        ("companions-v4.bin", COMPANION_SCHEMA_V4, 47, 2),
    ] {
        let golden = read_go_fixture(&format!("server/storage/companion/testdata/{name}"));
        assert_eq!(u32_at(&golden, 8), schema, "{name} schema");
        let before = golden.clone();
        let decoded = decode_companions(&golden).unwrap_or_else(|err| panic!("{name}: {err}"));
        assert_eq!(
            golden, before,
            "{name}: decode must not rewrite the input bytes"
        );
        assert_eq!(decoded.source_schema, schema, "{name} source schema");
        assert_eq!(decoded.revision, revision, "{name} revision");
        assert_eq!(decoded.records.len(), 2, "{name} record count");
        assert!(decoded.lifecycles.is_empty(), "{name} lifecycles");
        assert_eq!(decoded.queues.len(), queues, "{name} queue count");
    }
}

#[test]
fn companion_decode_rejects_future_and_unsupported_versions() {
    let encoded = encode_companions(&companion_golden_save()).expect("encode companion save");

    let mut future_schema = encoded.clone();
    future_schema[8..12].copy_from_slice(&(COMPANION_CURRENT_SCHEMA + 1).to_le_bytes());
    assert_eq!(
        decode_companions(&future_schema).unwrap_err(),
        StorageError::FutureVersion("companion schema: 6".to_owned()),
    );

    let mut future_envelope = encoded.clone();
    future_envelope[4..8].copy_from_slice(&(COMPANION_ENVELOPE_VERSION + 1).to_le_bytes());
    assert_eq!(
        decode_companions(&future_envelope).unwrap_err(),
        StorageError::FutureVersion("companion envelope version: 2".to_owned()),
    );

    let mut unsupported_schema = encoded.clone();
    unsupported_schema[8..12].copy_from_slice(&6u32.to_le_bytes());
    unsupported_schema[8..12].copy_from_slice(&7u32.to_le_bytes());
    assert!(matches!(
        decode_companions(&unsupported_schema).unwrap_err(),
        StorageError::FutureVersion(_)
    ));
}

#[test]
fn companion_decode_rejects_corrupt_and_partial_bytes() {
    let encoded = encode_companions(&companion_golden_save()).expect("encode companion save");

    for length in [0, 4, 31, 32, 33, 48, encoded.len() - 1] {
        assert!(
            decode_companions(&encoded[..length]).is_err(),
            "truncated companion file of {length} bytes must be rejected"
        );
    }

    let mut oversized = encoded.clone();
    oversized.extend_from_slice(&[0u8; 8]);
    assert!(decode_companions(&oversized).is_err());

    let mut broken_crc = encoded.clone();
    broken_crc[60] ^= 0xff;
    assert!(decode_companions(&broken_crc).is_err());

    // A count that the payload cannot back is rejected before any allocation.
    let mut oversized_count = encoded.clone();
    oversized_count[20..24].copy_from_slice(&(COMPANION_MAX_STORED as u32 + 1).to_le_bytes());
    assert!(decode_companions(&oversized_count).is_err());

    // Resealing after the mutation proves the rejection comes from the record
    // validator rather than the CRC gate.
    let mut unsorted = encoded.clone();
    unsorted[32 + 16..32 + 32].copy_from_slice(&companion_id(9).to_bytes());
    reseal_companion(&mut unsorted);
    assert!(decode_companions(&unsorted).is_err());
}

#[test]
fn companion_encode_rejects_invalid_couplings() {
    let bodies = companion_bodies();
    let mut duplicate = CompanionSave {
        revision: 1,
        agent_namespace_id: agent_identity(0x70),
        records: vec![bodies[1].clone(), bodies[1].clone()],
        lifecycles: Vec::new(),
        queues: Vec::new(),
    };
    duplicate.lifecycles = vec![
        fixture_lifecycle(bodies[1].id, true),
        fixture_lifecycle(bodies[1].id, true),
    ];
    assert!(encode_companions(&duplicate).is_err());

    // A lifecycle set that does not line up with the record set is rejected.
    let mut mismatched = CompanionSave {
        revision: 1,
        agent_namespace_id: agent_identity(0x70),
        records: vec![bodies[1].clone()],
        lifecycles: vec![fixture_lifecycle(bodies[1].id, true)],
        queues: Vec::new(),
    };
    mismatched
        .lifecycles
        .push(fixture_lifecycle(companion_id(3), false));
    assert!(encode_companions(&mismatched).is_err());

    // An inactive companion may not carry a task or FIFO payload.
    let mut inactive_with_queue = CompanionSave {
        revision: 1,
        agent_namespace_id: agent_identity(0x70),
        records: vec![bodies[1].clone()],
        lifecycles: vec![fixture_lifecycle(bodies[1].id, false)],
        queues: vec![StoredCompanionQueue {
            id: bodies[1].id,
            pending: vec!["跟我来".to_owned()],
            ..StoredCompanionQueue::default()
        }],
    };
    assert!(encode_companions(&inactive_with_queue).is_err());

    // A v5 queue may not carry a legacy summary.
    inactive_with_queue.lifecycles[0].active = true;
    inactive_with_queue.queues[0].summary = "legacy summary".to_owned();
    assert!(encode_companions(&inactive_with_queue).is_err());

    // Zero revision and an invalid namespace are rejected up front.
    let mut zero_revision = CompanionSave {
        revision: 0,
        agent_namespace_id: agent_identity(0x70),
        records: Vec::new(),
        lifecycles: Vec::new(),
        queues: Vec::new(),
    };
    assert!(encode_companions(&zero_revision).is_err());
    zero_revision.revision = 1;
    zero_revision.agent_namespace_id = PlayerId::default();
    assert!(encode_companions(&zero_revision).is_err());
}

/// Recomputes the companion envelope checksum in place so a semantic mutation
/// is rejected by the record validator rather than the CRC gate.
fn reseal_companion(bytes: &mut [u8]) {
    let checksum = crc32c_join(&[&bytes[8..28], &bytes[32..]]);
    bytes[28..32].copy_from_slice(&checksum.to_le_bytes());
}

fn agent_identity(last: u8) -> PlayerId {
    PlayerId::from_bytes([
        0x10, 0x11, 0x12, 0x13, 0x14, 0x15, 0x46, 0x17, 0x88, 0x19, 0x1a, 0x1b, 0x1c, 0x1d, 0x1e,
        last,
    ])
}

fn fixture_lifecycle(id: PlayerId, active: bool) -> StoredCompanionLifecycle {
    let mut lifecycle = if active {
        StoredCompanionLifecycle {
            id,
            active,
            memory_epoch: 1,
            memory_revision: 0,
            memory_operation_id: PlayerId::default(),
            summary: String::new(),
            tombstone_operation_id: PlayerId::default(),
        }
    } else {
        StoredCompanionLifecycle {
            id,
            active,
            memory_epoch: 1,
            memory_revision: 0,
            memory_operation_id: PlayerId::default(),
            summary: String::new(),
            tombstone_operation_id: agent_identity(id.to_bytes()[15] + 0x40),
        }
    };
    lifecycle.active = active;
    lifecycle
}

fn companion_id(last: u8) -> PlayerId {
    PlayerId::from_bytes([
        0x00, 0x11, 0x22, 0x33, 0x44, 0x55, 0x46, 0x77, 0x88, 0x99, 0xaa, 0xbb, 0xcc, 0xdd, 0xee,
        last,
    ])
}

fn companion_stack(item: u16, count: u8, durability: u16) -> ItemStack {
    ItemStack {
        item,
        count,
        durability,
    }
}

/// The committed fixture's "high" companion body.
fn companion_high() -> CompanionBody {
    let mut body = CompanionBody {
        id: companion_id(2),
        dimension: 0,
        position: [-12.5, 70.0, 3.25],
        yaw: 1.25,
        pitch: -0.5,
        inventory: Inventory::default(),
    };
    body.inventory.hotbar.selected = 4;
    body.inventory.hotbar.slots[0] = companion_stack(1, 64, 0);
    body.inventory.hotbar.slots[4] = companion_stack(10, 1, 131);
    body.inventory.backpack[0] = companion_stack(20, 7, 0);
    body
}

/// The committed fixture's "low" companion body.
fn companion_low() -> CompanionBody {
    let mut body = CompanionBody {
        id: companion_id(1),
        dimension: 0,
        position: [8.5, 65.0, -9.75],
        yaw: -2.5,
        pitch: 0.75,
        inventory: Inventory::default(),
    };
    body.inventory.hotbar.selected = 2;
    body.inventory.hotbar.slots[2] = companion_stack(23, 12, 0);
    body.inventory.backpack[7] = companion_stack(11, 1, 250);
    body.inventory.backpack[26] = companion_stack(2, 5, 0);
    body
}

fn companion_bodies() -> Vec<CompanionBody> {
    vec![companion_high(), companion_low()]
}

fn companion_v3_queues() -> Vec<StoredCompanionQueue> {
    let pending: Vec<String> = (1..=COMPANION_MAX_FIFO_ENTRIES)
        .map(|index| format!("v3排队第{index}条"))
        .collect();
    vec![StoredCompanionQueue {
        id: companion_id(1),
        has_current: true,
        current: StoredCompanionTask {
            command: "去橡树旁挖一格垫一块再跟着我".to_owned(),
            plan_steps: vec![
                PlanStep {
                    kind: COMPANION_PLAN_STEP_GO_TO,
                    x: -8,
                    y: 70,
                    z: 6,
                    ..PlanStep::default()
                },
                PlanStep {
                    kind: COMPANION_PLAN_STEP_MINE,
                    x: -7,
                    y: 69,
                    z: 6,
                    ..PlanStep::default()
                },
                PlanStep {
                    kind: COMPANION_PLAN_STEP_PLACE,
                    x: -6,
                    y: 69,
                    z: 6,
                    block: 18,
                    ..PlanStep::default()
                },
                PlanStep {
                    kind: COMPANION_PLAN_STEP_FOLLOW,
                    player_id: PlayerId::from_bytes([
                        0x00, 0x11, 0x22, 0x33, 0x44, 0x55, 0x46, 0x77, 0x88, 0x99, 0xaa, 0xbb,
                        0xcc, 0xdd, 0xee, 0x05,
                    ]),
                    ..PlanStep::default()
                },
            ],
            step_index: 2,
            state: COMPANION_TASK_RUNNING,
            start_tick: 2400,
            deadline_ticks: 0,
            fail_reason: COMPANION_TASK_FAIL_NONE,
        },
        pending,
        summary: String::new(),
    }]
}

/// The committed v5 golden save, with a nonzero mirror, a canonical-zero
/// mirror, an inactive tombstone, and one task-bearing queue.
fn companion_golden_save() -> CompanionSave {
    let bodies = companion_bodies();
    let mut inactive_body = bodies[0].clone();
    inactive_body.id = companion_id(3);
    inactive_body.position = [24.5, 68.0, -17.5];
    let records = vec![bodies[1].clone(), bodies[0].clone(), inactive_body];

    let mut nonzero = fixture_lifecycle(records[0].id, true);
    nonzero.memory_epoch = 7;
    nonzero.memory_revision = 11;
    nonzero.memory_operation_id = agent_identity(0x71);
    nonzero.summary = "阿木记得北边橡树旁的小路。".to_owned();
    let mut zero = fixture_lifecycle(records[1].id, true);
    zero.memory_epoch = 3;
    let mut inactive = fixture_lifecycle(records[2].id, false);
    inactive.memory_epoch = 9;
    inactive.tombstone_operation_id = agent_identity(0x73);

    CompanionSave {
        revision: 47,
        agent_namespace_id: agent_identity(0x70),
        records,
        lifecycles: vec![nonzero, zero, inactive],
        queues: companion_v3_queues(),
    }
}
#[test]
fn player_v9_fixture_round_trips_byte_for_byte() {
    let golden = read_go_fixture("server/storage/player/testdata/player-v9.bin");
    assert_eq!(u32_at(&golden, 8), PLAYER_CURRENT_SCHEMA);
    assert_eq!(golden.len(), PLAYER_ENVELOPE_LENGTH + 273);
    let before = golden.clone();
    let decoded = decode_player(player_id(), &golden).expect("decode committed player v9 fixture");
    assert_eq!(golden, before, "decoding must not rewrite the input bytes");
    assert_eq!(decoded.revision, 19);
    assert!(!decoded.needs_rewrite);
    let save = stored_to_save(&decoded);
    let reencoded = encode_player(&save).expect("re-encode committed player v9 fixture");
    assert_eq!(
        reencoded, golden,
        "the Rust encoder must reproduce the committed Go bytes exactly"
    );
    let round = decode_player(player_id(), &reencoded).expect("decode re-encoded player");
    assert_eq!(round, decoded);
}

#[test]
fn player_current_schema_round_trip_preserves_fields() {
    let mut save = fixture_player_save(7);
    save.respawn_present = true;
    save.respawn_position = [7.0, 65.0, -9.0];
    save.respawn_dimension = 0;
    save.armor = armor_fixture_stacks();
    let encoded = encode_player(&save).expect("encode player");
    assert_eq!(u32_at(&encoded, 8), PLAYER_CURRENT_SCHEMA);
    let got = decode_player(save.player_id, &encoded).expect("decode player");
    assert_eq!(got.display_name, "Chen");
    assert_eq!(got.current, save.current);
    assert_eq!(got.safe, save.safe);
    assert_eq!(got.yaw, 1.25);
    assert_eq!(got.pitch, -0.5);
    assert_eq!(got.inventory, save.inventory);
    assert_eq!(got.health, 13);
    assert_eq!(got.hunger, 12);
    assert_eq!(got.saturation_milli, 2500);
    assert_eq!(got.exhaustion_milli, 1750);
    assert!(got.respawn_present);
    assert_eq!(got.respawn_position, save.respawn_position);
    assert_eq!(got.respawn_dimension, 0);
    assert_eq!(got.armor, save.armor);
    assert!(!got.needs_rewrite);
    // The decoded safe location must not alias the save payload.
    assert!(save.safe.is_some());
}

#[test]
fn player_without_respawn_encodes_independently_of_residue() {
    let save = fixture_player_save(7);
    let encoded = encode_player(&save).expect("encode player");
    let got = decode_player(save.player_id, &encoded).expect("decode player");
    assert!(!got.respawn_present);
    assert_eq!(got.respawn_position, [0.0; 3]);
    assert_eq!(got.respawn_dimension, 0);

    let mut residue = save.clone();
    residue.respawn_position = [1.0, 2.0, 3.0];
    residue.respawn_dimension = 0;
    let residue_encoded = encode_player(&residue).expect("encode residue player");
    assert_eq!(
        encoded, residue_encoded,
        "present=0 must encode deterministically regardless of residue"
    );
}

#[test]
fn player_armor_section_is_the_fixed_twenty_byte_tail() {
    let mut save = fixture_player_save(9);
    save.respawn_present = true;
    save.respawn_position = [7.0, 65.0, -9.0];
    save.respawn_dimension = 0;
    save.armor = armor_fixture_stacks();
    let encoded = encode_player(&save).expect("encode armored player");

    let mut unequipped = save.clone();
    unequipped.armor = [ItemStack::default(); 4];
    let plain = encode_player(&unequipped).expect("encode unequipped player");
    assert_eq!(encoded.len(), plain.len());
    assert_eq!(
        &encoded[PLAYER_ENVELOPE_LENGTH..encoded.len() - 20],
        &plain[PLAYER_ENVELOPE_LENGTH..plain.len() - 20],
        "the armor section must sit at the fixed end of the payload"
    );
    assert_ne!(
        &encoded[encoded.len() - 20..],
        &plain[plain.len() - 20..],
        "the armor fixture must carry nonzero bytes for the tail assertion"
    );
}

#[test]
fn player_depths_locations_round_trip() {
    let mut save = fixture_player_save(7);
    save.current.dimension = 1;
    if let Some(safe) = &mut save.safe {
        safe.dimension = 1;
    }
    save.respawn_present = true;
    save.respawn_position = [7.0, 65.0, -9.0];
    save.respawn_dimension = 1;
    let encoded = encode_player(&save).expect("encode depths player");
    let got = decode_player(save.player_id, &encoded).expect("decode depths player");
    assert_eq!(got.current.dimension, 1);
    assert_eq!(got.safe.as_ref().map(|safe| safe.dimension), Some(1));
    assert_eq!(got.respawn_dimension, 1);
}

#[test]
fn player_legacy_fixtures_decode_without_repair() {
    for schema in 1..=8u32 {
        let golden = read_go_fixture(&format!(
            "server/storage/player/testdata/player-v{schema}.bin"
        ));
        assert_eq!(u32_at(&golden, 8), schema, "player-v{schema}.bin schema");
        let before = golden.clone();
        let decoded = decode_player(player_id(), &golden)
            .unwrap_or_else(|err| panic!("player-v{schema}.bin: {err}"));
        assert_eq!(
            golden, before,
            "player-v{schema}.bin: decode must not rewrite the input bytes"
        );
        assert_eq!(decoded.revision, 19, "player-v{schema}.bin revision");
        assert!(
            decoded.needs_rewrite,
            "player-v{schema}.bin: a legacy schema must be marked for rewrite"
        );
    }
}

#[test]
fn player_legacy_migrations_normalize_documented_defaults() {
    let v1 = read_go_fixture("server/storage/player/testdata/player-v1.bin");
    let decoded = decode_player(player_id(), &v1).expect("decode player v1");
    assert_eq!(decoded.inventory.hotbar.selected, 0);
    assert!(
        decoded
            .inventory
            .hotbar
            .slots
            .iter()
            .all(|slot| *slot == ItemStack::default()),
        "v1 has no item payload and must migrate to an empty hotbar"
    );

    // v6 shares the v5 payload layout and gains only the item registry, so the
    // v6 migration step fills the hunger defaults.
    let v6 = read_go_fixture("server/storage/player/testdata/player-v6.bin");
    let decoded = decode_player(player_id(), &v6).expect("decode player v6");
    // Health is persisted from v5, so the v6 fixture keeps its own value; only
    // the hunger state is a migration default.
    assert_eq!(decoded.hunger, 20);
    assert_eq!(decoded.saturation_milli, 5000);
    assert_eq!(decoded.exhaustion_milli, 0);

    // v8 still lacks the armor section, which migrates to four empty slots;
    // the respawn point it carries survives untouched.
    let v8 = read_go_fixture("server/storage/player/testdata/player-v8.bin");
    let decoded = decode_player(player_id(), &v8).expect("decode player v8");
    assert_eq!(decoded.armor, [ItemStack::default(); 4]);

    // v7 has no personal respawn point, which migrates to "no respawn".
    let v7 = read_go_fixture("server/storage/player/testdata/player-v7.bin");
    let decoded = decode_player(player_id(), &v7).expect("decode player v7");
    assert!(!decoded.respawn_present);
    assert_eq!(decoded.respawn_position, [0.0; 3]);
    assert_eq!(decoded.respawn_dimension, 0);
    assert_eq!(decoded.hunger, 12);
    assert_eq!(decoded.saturation_milli, 2500);
    assert_eq!(decoded.exhaustion_milli, 1750);

    // v3 stacks carry no durability, so tools migrate to full durability.
    let v3 = read_go_fixture("server/storage/player/testdata/player-v3.bin");
    let decoded = decode_player(player_id(), &v3).expect("decode player v3");
    assert!(
        decoded
            .inventory
            .hotbar
            .slots
            .iter()
            .chain(decoded.inventory.backpack.iter())
            .all(|slot| slot.item == 0
                || slot.durability != 0
                || item_max_durability(slot.item).is_none())
    );
}

#[test]
fn player_decode_rejects_future_versions() {
    let encoded = encode_player(&fixture_player_save(7)).expect("encode player");

    let mut future_schema = encoded.clone();
    future_schema[8..12].copy_from_slice(&(PLAYER_CURRENT_SCHEMA + 1).to_le_bytes());
    assert_eq!(
        decode_player(player_id(), &future_schema).unwrap_err(),
        StorageError::FutureVersion("player schema: 10".to_owned()),
    );

    let mut future_envelope = encoded.clone();
    future_envelope[4..8].copy_from_slice(&2u32.to_le_bytes());
    assert_eq!(
        decode_player(player_id(), &future_envelope).unwrap_err(),
        StorageError::FutureVersion("player envelope version: 2".to_owned()),
    );
}

#[test]
fn player_decode_rejects_corrupt_and_partial_bytes() {
    let encoded = encode_player(&fixture_player_save(7)).expect("encode player");

    for length in [0, 4, 43, 44, 45, encoded.len() - 1] {
        assert!(
            decode_player(player_id(), &encoded[..length]).is_err(),
            "truncated player file of {length} bytes must be rejected"
        );
    }

    let mut trailing = encoded.clone();
    trailing.push(0);
    assert!(decode_player(player_id(), &trailing).is_err());

    let mut broken_crc = encoded.clone();
    let last = broken_crc.len() - 1;
    broken_crc[last] ^= 0xff;
    assert!(decode_player(player_id(), &broken_crc).is_err());

    // A payload longer than the declared envelope length is rejected before
    // any allocation.
    let mut oversized_payload = encoded.clone();
    oversized_payload[36..40].copy_from_slice(&(PLAYER_MAX_PAYLOAD as u32 + 1).to_le_bytes());
    assert!(decode_player(player_id(), &oversized_payload).is_err());

    // A payload that is one byte shorter than the inventory requires is a
    // layout mismatch, not a record.
    let mut short_inventory = encoded.clone();
    let declared = u32::from_le_bytes(short_inventory[36..40].try_into().expect("four bytes"));
    short_inventory[36..40].copy_from_slice(&(declared - 1).to_le_bytes());
    assert!(decode_player(player_id(), &short_inventory).is_err());

    let mut bad_safe_flag = encoded.clone();
    let safe_flag_offset = PLAYER_ENVELOPE_LENGTH + payload_prefix_length(&encoded);
    bad_safe_flag[safe_flag_offset] = 2;
    reseal_player(&mut bad_safe_flag);
    assert!(decode_player(player_id(), &bad_safe_flag).is_err());
}

#[test]
fn player_decode_rejects_foreign_and_invalid_identities() {
    let encoded = encode_player(&fixture_player_save(7)).expect("encode player");
    let mut foreign = player_id().to_bytes();
    foreign[15] = 0x00;
    assert!(decode_player(PlayerId::from_bytes(foreign), &encoded).is_err());
    assert!(decode_player(PlayerId::default(), &encoded).is_err());
}

#[test]
fn player_encode_rejects_invalid_saves() {
    let mut zero_revision = fixture_player_save(0);
    assert!(encode_player(&zero_revision).is_err());
    zero_revision.revision = 7;
    zero_revision.display_name = "   ".to_owned();
    assert!(encode_player(&zero_revision).is_err());
    zero_revision.display_name = "Chen".to_owned();
    zero_revision.health = 21;
    assert!(encode_player(&zero_revision).is_err());
    zero_revision.health = 20;
    zero_revision.current.dimension = 2;
    assert!(encode_player(&zero_revision).is_err());
    zero_revision.current.dimension = 0;
    zero_revision.pitch = 2.0;
    assert!(encode_player(&zero_revision).is_err());
}

/// Identity every committed `chunk-v*.bin` fixture carries.
const CHUNK_FIXTURE_KEY: ChunkKey = ChunkKey {
    dimension: 0,
    x: -3,
    z: 7,
};
const CHUNK_FIXTURE_REVISION: u64 = 19;

fn chunk_fixture(schema: u32) -> Vec<u8> {
    read_go_fixture(&format!(
        "server/storage/chunk/testdata/chunk-v{schema}.bin"
    ))
}

fn chunk_fixture_chunk(schema: u32) -> DecodedChunk {
    decode_chunk(
        CHUNK_FIXTURE_KEY,
        CHUNK_FIXTURE_REVISION,
        &chunk_fixture(schema),
    )
    .unwrap_or_else(|err| panic!("decode chunk-v{schema}.bin: {err}"))
}

/// Packs `values` into the non-crossing word layout the paletted container
/// uses: `64 / bits` slots per word, the remaining bits unused.
fn pack_section_words(bits: u8, values: &[u32]) -> Vec<u64> {
    let per_word = 64 / bits as usize;
    let mut words = vec![0u64; 4096_usize.div_ceil(per_word)];
    for (index, value) in values.iter().enumerate() {
        let shift = (index % per_word) * bits as usize;
        words[index / per_word] |= u64::from(*value) << shift;
    }
    words
}

/// A chunk with one section per storage kind plus every container section
/// populated, so the encode/decode path covers drops, furnaces and chests.
fn synthetic_chunk() -> Chunk {
    let mut sections = Vec::with_capacity(24);
    sections.push(ContainerSnapshot {
        kind: StorageKind::Single,
        bits: 0,
        single: 2,
        palette: Vec::new(),
        packed: Vec::new(),
    });
    sections.push(ContainerSnapshot {
        kind: StorageKind::Indexed,
        bits: 4,
        single: 0,
        palette: (0..16u16).collect(),
        packed: pack_section_words(4, &[0, 1, 2, 3, 15, 0, 0, 0]),
    });
    // Section 2 is direct storage holding a furnace and a chest block so the
    // container slots decoded from it can point at real furnace/chest blocks.
    let mut direct_values = vec![0u32; 4096];
    direct_values[0] = 9;
    direct_values[1] = 11;
    sections.push(ContainerSnapshot {
        kind: StorageKind::Direct,
        bits: 15,
        single: 0,
        palette: Vec::new(),
        packed: pack_section_words(15, &direct_values),
    });
    for _ in 3..24 {
        sections.push(ContainerSnapshot {
            kind: StorageKind::Single,
            bits: 0,
            single: 0,
            palette: Vec::new(),
            packed: Vec::new(),
        });
    }

    let mut drops = vec![DropSlot::default(); 32];
    drops[0] = DropSlot {
        generation: 4,
        active: true,
        stack: ItemStack {
            item: 1,
            count: 12,
            durability: 0,
        },
        block_index: 2 * 4096 + 5,
        age_ticks: 71,
        pickup_delay_ticks: 3,
    };
    drops[1] = DropSlot {
        generation: 9,
        active: true,
        stack: ItemStack {
            item: 10,
            count: 1,
            durability: 131,
        },
        block_index: 2 * 4096 + 6,
        age_ticks: 4,
        pickup_delay_ticks: 0,
    };

    let mut furnaces = vec![FurnaceSlot::default(); 32];
    furnaces[0] = FurnaceSlot {
        generation: 5,
        active: true,
        block_index: 2 * 4096,
        input: ItemStack {
            item: 6,
            count: 3,
            durability: 0,
        },
        fuel: ItemStack {
            item: 5,
            count: 2,
            durability: 0,
        },
        output: ItemStack {
            item: 7,
            count: 1,
            durability: 0,
        },
        progress_ticks: 120,
        burn_ticks: 900,
    };
    furnaces[1] = FurnaceSlot {
        generation: 6,
        ..FurnaceSlot::default()
    };

    let mut chests = vec![ChestSlot::default(); 16];
    chests[0] = ChestSlot {
        generation: 7,
        active: true,
        block_index: 2 * 4096 + 1,
        items: std::array::from_fn(|index| ItemStack {
            item: if index == 0 { 1 } else { 0 },
            count: if index == 0 { 5 } else { 0 },
            durability: 0,
        }),
    };
    chests[1] = ChestSlot {
        generation: 8,
        ..ChestSlot::default()
    };

    Chunk {
        sections,
        drops,
        furnaces,
        chests,
    }
}

/// Decode exactness: every committed fixture must reproduce its exact logical
/// payload byte for byte. The comparison re-serializes the decoded chunk at the
/// schema the fixture itself declares, so a field the codec misreads, reorders
/// or drops fails here even though the zstd frame still decompresses.
#[test]
fn chunk_committed_fixtures_decode_to_the_exact_logical_payload() {
    for schema in CHUNK_OLDEST_SCHEMA..=CHUNK_CURRENT_SCHEMA {
        let payload = chunk_fixture(schema);
        let envelope = decode_chunk_envelope(&payload)
            .unwrap_or_else(|err| panic!("chunk-v{schema} envelope: {err}"));
        assert_eq!(envelope.schema, schema, "chunk-v{schema} declared schema");
        assert_eq!(envelope.key, CHUNK_FIXTURE_KEY);
        assert_eq!(envelope.revision, CHUNK_FIXTURE_REVISION);
        let logical = decode_chunk_logical(
            envelope.key,
            envelope.revision,
            envelope.schema,
            &envelope.bytes,
        )
        .unwrap_or_else(|err| panic!("chunk-v{schema} logical: {err}"));
        let reserialized =
            encode_chunk_logical(envelope.key, envelope.revision, &logical, envelope.schema)
                .unwrap_or_else(|err| panic!("chunk-v{schema} logical re-encode: {err}"));
        assert_eq!(
            reserialized, envelope.bytes,
            "chunk-v{schema} logical payload is not reproduced byte for byte"
        );
    }
}

/// Encode/decode losslessness plus the fixture round trip: encoding the chunk
/// a fixture decodes to, then decoding that envelope again, must give back the
/// same logical chunk.
#[test]
fn chunk_encode_decode_round_trip_is_lossless() {
    for schema in CHUNK_OLDEST_SCHEMA..=CHUNK_CURRENT_SCHEMA {
        let decoded = chunk_fixture_chunk(schema);
        let save = ChunkSave {
            key: decoded.key,
            revision: decoded.revision,
            chunk: decoded.chunk.clone(),
        };
        let encoded = encode_chunk(&save)
            .unwrap_or_else(|err| panic!("encode decoded chunk-v{schema}: {err}"));
        let again = decode_chunk(save.key, save.revision, &encoded)
            .unwrap_or_else(|err| panic!("decode re-encoded chunk-v{schema}: {err}"));
        assert_eq!(again.chunk, decoded.chunk, "chunk-v{schema} round trip");
        assert_eq!(again.key, decoded.key);
        assert_eq!(again.revision, decoded.revision);
        assert_eq!(again.schema, CHUNK_CURRENT_SCHEMA);
        assert!(!again.migrated, "a freshly encoded chunk is not migrated");
    }
}

/// A synthetic chunk exercises drops, furnaces and chests together with all
/// three paletted storage kinds.
#[test]
fn chunk_synthetic_chunk_round_trip_preserves_every_section() {
    let chunk = synthetic_chunk();
    let save = ChunkSave {
        key: CHUNK_FIXTURE_KEY,
        revision: 91,
        chunk: chunk.clone(),
    };
    let encoded = encode_chunk(&save).expect("encode synthetic chunk");
    let decoded = decode_chunk(save.key, save.revision, &encoded).expect("decode synthetic chunk");
    assert_eq!(decoded.chunk, chunk);
    assert_eq!(decoded.key, save.key);
    assert_eq!(decoded.revision, save.revision);
    assert_eq!(decoded.schema, CHUNK_CURRENT_SCHEMA);
    assert!(!decoded.migrated);
}

/// Migration convergence: one logical chunk encoded at every supported schema
/// must decode and migrate to the same normalized chunk.
#[test]
fn chunk_supported_versions_converge_on_one_normalized_result() {
    let normalized = chunk_fixture_chunk(CHUNK_CURRENT_SCHEMA).chunk;
    // Schemas below 2 carry no drops, below 4 no furnaces and below 6 no
    // chests, so the shared logical chunk leaves those sections empty.
    let bare = Chunk {
        drops: vec![DropSlot::default(); normalized.drops.len()],
        furnaces: vec![FurnaceSlot::default(); normalized.furnaces.len()],
        chests: vec![ChestSlot::default(); normalized.chests.len()],
        ..normalized.clone()
    };
    for schema in CHUNK_OLDEST_SCHEMA..=CHUNK_CURRENT_SCHEMA {
        let save = ChunkSave {
            key: CHUNK_FIXTURE_KEY,
            revision: CHUNK_FIXTURE_REVISION,
            chunk: bare.clone(),
        };
        let encoded = encode_chunk_at_schema(&save, schema)
            .unwrap_or_else(|err| panic!("encode chunk at schema {schema}: {err}"));
        let decoded = decode_chunk(save.key, save.revision, &encoded)
            .unwrap_or_else(|err| panic!("decode chunk at schema {schema}: {err}"));
        assert_eq!(decoded.chunk, bare, "schema {schema} did not converge");
        assert_eq!(decoded.schema, CHUNK_CURRENT_SCHEMA);
        assert_eq!(
            decoded.migrated,
            schema < CHUNK_CURRENT_SCHEMA,
            "schema {schema} migration flag"
        );
    }
}

/// Every committed fixture decodes, migrates to the current schema, and reports
/// whether it needs a rewrite.
#[test]
fn chunk_committed_fixtures_migrate_to_the_current_schema() {
    for schema in CHUNK_OLDEST_SCHEMA..=CHUNK_CURRENT_SCHEMA {
        let decoded = chunk_fixture_chunk(schema);
        assert_eq!(decoded.schema, CHUNK_CURRENT_SCHEMA);
        assert_eq!(decoded.migrated, schema < CHUNK_CURRENT_SCHEMA);
        assert_eq!(decoded.key, CHUNK_FIXTURE_KEY);
        assert_eq!(decoded.revision, CHUNK_FIXTURE_REVISION);
    }
}

#[test]
fn chunk_decode_rejects_future_and_unsupported_versions() {
    for schema in CHUNK_OLDEST_SCHEMA..=CHUNK_CURRENT_SCHEMA {
        let mut payload = chunk_fixture(schema);
        payload[8..12].copy_from_slice(&(CHUNK_CURRENT_SCHEMA + 1).to_le_bytes());
        let err = decode_chunk(CHUNK_FIXTURE_KEY, CHUNK_FIXTURE_REVISION, &payload)
            .expect_err("future chunk schema was accepted");
        assert!(
            matches!(err, StorageError::FutureVersion(_)),
            "future schema error = {err}"
        );

        payload[8..12].copy_from_slice(&0u32.to_le_bytes());
        let err = decode_chunk(CHUNK_FIXTURE_KEY, CHUNK_FIXTURE_REVISION, &payload)
            .expect_err("unsupported chunk schema was accepted");
        assert!(
            matches!(err, StorageError::Corrupt(_)),
            "unsupported schema error = {err}"
        );

        payload[8..12].copy_from_slice(&schema.to_le_bytes());
        payload[4..8].copy_from_slice(&2u32.to_le_bytes());
        let err = decode_chunk(CHUNK_FIXTURE_KEY, CHUNK_FIXTURE_REVISION, &payload)
            .expect_err("future envelope version was accepted");
        assert!(
            matches!(err, StorageError::FutureVersion(_)),
            "future envelope version error = {err}"
        );
        payload[4..8].copy_from_slice(&0u32.to_le_bytes());
        let err = decode_chunk(CHUNK_FIXTURE_KEY, CHUNK_FIXTURE_REVISION, &payload)
            .expect_err("unsupported envelope version was accepted");
        assert!(
            matches!(err, StorageError::Corrupt(_)),
            "unsupported envelope version error = {err}"
        );
    }
}

#[test]
fn chunk_decode_rejects_corrupt_and_partial_payloads() {
    let encoded = encode_chunk(&ChunkSave {
        key: CHUNK_FIXTURE_KEY,
        revision: CHUNK_FIXTURE_REVISION,
        chunk: chunk_fixture_chunk(CHUNK_CURRENT_SCHEMA).chunk,
    })
    .expect("encode chunk");
    let logical = decode_chunk_envelope(&encoded).expect("envelope").bytes;

    let mut cases: Vec<(&str, Vec<u8>)> = vec![
        ("empty payload", Vec::new()),
        (
            "short header",
            encoded[..CHUNK_ENVELOPE_LENGTH - 1].to_vec(),
        ),
        ("wrong magic", {
            let mut payload = encoded.clone();
            payload[0] = b'X';
            payload
        }),
        ("unknown compression ID", {
            let mut payload = encoded.clone();
            payload[32..36].copy_from_slice(&99u32.to_le_bytes());
            payload
        }),
        ("compressed length over limit", {
            let mut payload = encoded.clone();
            payload[40..44].copy_from_slice(&(MAX_COMPRESSED_CHUNK + 1).to_le_bytes());
            payload
        }),
        ("decoded length over limit", {
            let mut payload = encoded.clone();
            payload[36..40].copy_from_slice(&(CHUNK_MAX_DECODED_CHUNK as u32 + 1).to_le_bytes());
            payload
        }),
        (
            "truncated compressed bytes",
            encoded[..encoded.len() - 1].to_vec(),
        ),
        ("trailing envelope bytes", {
            let mut payload = encoded.clone();
            payload.push(0);
            payload
        }),
    ];

    for (name, payload) in cases.drain(..) {
        let err = decode_chunk(CHUNK_FIXTURE_KEY, CHUNK_FIXTURE_REVISION, &payload)
            .expect_err("{name}: payload was accepted");
        assert!(
            matches!(err, StorageError::Corrupt(_)),
            "{name}: error = {err:?}"
        );
    }

    // The requested identity is part of the envelope contract.
    let err = decode_chunk(
        ChunkKey {
            dimension: 0,
            x: -2,
            z: 7,
        },
        CHUNK_FIXTURE_REVISION,
        &encoded,
    )
    .expect_err("foreign chunk key was accepted");
    assert!(matches!(err, StorageError::Corrupt(_)), "error = {err}");
    let err = decode_chunk(CHUNK_FIXTURE_KEY, 20, &encoded)
        .expect_err("foreign chunk revision was accepted");
    assert!(matches!(err, StorageError::Corrupt(_)), "error = {err}");
    let err = decode_chunk(CHUNK_FIXTURE_KEY, 0, &encoded).expect_err("zero revision was accepted");
    assert!(matches!(err, StorageError::Corrupt(_)), "error = {err}");

    // Logical-layer corruption: the frame still decompresses, so only the
    // logical contract can reject it.
    let logical_cases: Vec<(&str, Vec<u8>)> = vec![
        ("wrong logical magic", {
            let mut bytes = logical.clone();
            bytes[0] = b'X';
            bytes
        }),
        ("section count mismatch", {
            let mut bytes = logical.clone();
            bytes[28..32].copy_from_slice(&23u32.to_le_bytes());
            bytes
        }),
        ("section order mismatch", {
            let mut bytes = logical.clone();
            bytes[32..36].copy_from_slice(&1u32.to_le_bytes());
            bytes
        }),
        ("trailing logical bytes", {
            let mut bytes = logical.clone();
            bytes.push(0);
            bytes
        }),
        (
            "truncated logical bytes",
            logical[..logical.len() - 8].to_vec(),
        ),
    ];
    for (name, bytes) in logical_cases {
        let err = decode_chunk_logical(
            CHUNK_FIXTURE_KEY,
            CHUNK_FIXTURE_REVISION,
            CHUNK_CURRENT_SCHEMA,
            &bytes,
        )
        .expect_err("{name}: logical payload was accepted");
        assert!(
            matches!(err, StorageError::Corrupt(_)),
            "{name}: error = {err:?}"
        );
    }
}

#[test]
fn chunk_encode_rejects_invalid_saves() {
    let valid = ChunkSave {
        key: CHUNK_FIXTURE_KEY,
        revision: 19,
        chunk: chunk_fixture_chunk(CHUNK_CURRENT_SCHEMA).chunk,
    };

    let mut zero_revision = valid.clone();
    zero_revision.revision = 0;
    assert!(encode_chunk(&zero_revision).is_err());

    let mut unsupported_dimension = valid.clone();
    unsupported_dimension.key.dimension = 2;
    assert!(encode_chunk(&unsupported_dimension).is_err());

    let mut wrong_section_count = valid.clone();
    wrong_section_count.chunk.sections.pop();
    assert!(encode_chunk(&wrong_section_count).is_err());

    let mut wrong_drop_count = valid.clone();
    wrong_drop_count.chunk.drops.pop();
    assert!(encode_chunk(&wrong_drop_count).is_err());

    let mut wrong_furnace_count = valid.clone();
    wrong_furnace_count.chunk.furnaces.pop();
    assert!(encode_chunk(&wrong_furnace_count).is_err());

    let mut wrong_chest_count = valid.clone();
    wrong_chest_count.chunk.chests.pop();
    assert!(encode_chunk(&wrong_chest_count).is_err());

    // An indexed palette longer than its bit width cannot be read back.
    let mut oversized_palette = valid.clone();
    oversized_palette.chunk.sections[1] = ContainerSnapshot {
        kind: StorageKind::Indexed,
        bits: 4,
        single: 0,
        palette: (0..17u16).collect(),
        packed: pack_section_words(4, &[0, 1]),
    };
    assert!(encode_chunk(&oversized_palette).is_err());

    // A furnace slot that breaks its own tick ceilings is not a valid save.
    let mut broken_furnace = valid.clone();
    broken_furnace.chunk.furnaces[0] = FurnaceSlot {
        generation: 5,
        active: true,
        block_index: 0,
        input: ItemStack::default(),
        fuel: ItemStack::default(),
        output: ItemStack::default(),
        progress_ticks: 200,
        burn_ticks: 0,
    };
    assert!(encode_chunk(&broken_furnace).is_err());
}

/// A slot that is itself valid but points at the wrong block, or shares its
/// block with another active slot, is rejected by the encoder. Decode uses the
/// same aggregate check, so those saves never become bytes.
#[test]
fn chunk_decode_rejects_container_slots_pointing_at_the_wrong_block() {
    let mut chunk = synthetic_chunk();
    chunk.furnaces[0].block_index = 2 * 4096 + 200;
    let save = ChunkSave {
        key: CHUNK_FIXTURE_KEY,
        revision: 33,
        chunk: chunk.clone(),
    };
    assert!(matches!(encode_chunk(&save), Err(StorageError::Corrupt(_))));

    chunk.furnaces[0].block_index = 2 * 4096;
    chunk.chests[0].block_index = 2 * 4096 + 200;
    let save = ChunkSave {
        key: CHUNK_FIXTURE_KEY,
        revision: 34,
        chunk: chunk.clone(),
    };
    assert!(matches!(encode_chunk(&save), Err(StorageError::Corrupt(_))));

    // Two active furnaces sharing one block index are rejected too.
    let mut shared = synthetic_chunk();
    shared.furnaces[1] = FurnaceSlot {
        generation: 6,
        active: true,
        block_index: 2 * 4096,
        ..shared.furnaces[1]
    };
    let save = ChunkSave {
        key: CHUNK_FIXTURE_KEY,
        revision: 35,
        chunk: shared,
    };
    assert!(matches!(encode_chunk(&save), Err(StorageError::Corrupt(_))));
}

/// The frame header and the trailing content checksum are the two parts of the
/// zstd frame both implementations were verified to write identically. They are
/// pinned here so a real regression in either is still caught, even though the
/// compressed block payload between them intentionally differs.
#[test]
fn chunk_frame_header_and_content_checksum_match_the_reference_frame() {
    for schema in CHUNK_OLDEST_SCHEMA..=CHUNK_CURRENT_SCHEMA {
        let payload = chunk_fixture(schema);
        let frame = &payload[CHUNK_ENVELOPE_LENGTH..];
        assert_frame_header(frame, &payload[36..40]);
        // The checksum occupies the last four bytes of the frame; corrupting it
        // must be rejected rather than silently ignored.
        let mut broken = payload.clone();
        let last = broken.len() - 1;
        broken[last] ^= 0xff;
        let err = decode_chunk(CHUNK_FIXTURE_KEY, CHUNK_FIXTURE_REVISION, &broken)
            .expect_err("corrupted content checksum was accepted");
        assert!(matches!(err, StorageError::Corrupt(_)), "error = {err}");
    }

    let encoded = encode_chunk(&ChunkSave {
        key: CHUNK_FIXTURE_KEY,
        revision: CHUNK_FIXTURE_REVISION,
        chunk: chunk_fixture_chunk(CHUNK_CURRENT_SCHEMA).chunk,
    })
    .expect("encode chunk");
    assert_frame_header(&encoded[CHUNK_ENVELOPE_LENGTH..], &encoded[36..40]);
    let mut broken = encoded.clone();
    let last = broken.len() - 1;
    broken[last] ^= 0xff;
    let err = decode_chunk(CHUNK_FIXTURE_KEY, CHUNK_FIXTURE_REVISION, &broken)
        .expect_err("corrupted content checksum was accepted");
    assert!(matches!(err, StorageError::Corrupt(_)), "error = {err}");
}

/// Asserts the reference zstd frame header: magic, a single-segment descriptor
/// with the content checksum and a two-byte content size, no dictionary ID, and
/// a content size equal to the envelope's declared decoded length.
fn assert_frame_header(frame: &[u8], declared_decoded_length: &[u8]) {
    assert_eq!(&frame[..4], &[0x28, 0xB5, 0x2F, 0xFD], "zstd frame magic");
    // Single segment (bit 5), content checksum (bit 2), two-byte content size
    // (bits 7-6 = 01), no dictionary ID (bits 1-0 = 00).
    assert_eq!(frame[4], 0x64, "frame header descriptor");
    let declared = u32::from_le_bytes(declared_decoded_length.try_into().expect("four bytes"));
    let content = u32::from(u16::from_le_bytes([frame[5], frame[6]])) + 256;
    assert_eq!(content, declared, "frame content size");
    assert!(
        frame.len() >= 8,
        "frame is shorter than header plus checksum"
    );
}

fn payload_prefix_length(encoded: &[u8]) -> usize {
    // Name length prefix plus the name bytes, ending at the yaw field.
    let name_length = u32::from_le_bytes(
        encoded[PLAYER_ENVELOPE_LENGTH..PLAYER_ENVELOPE_LENGTH + 4]
            .try_into()
            .expect("four bytes"),
    ) as usize;
    4 + name_length + 4 + 12 + 4 + 4
}

/// Recomputes the MCPL envelope checksum in place so a semantic mutation is
/// rejected by the record validator rather than the CRC gate.
fn reseal_player(bytes: &mut [u8]) {
    let payload_length = u32::from_le_bytes(bytes[36..40].try_into().expect("four bytes")) as usize;
    let checksum = crc32c_join(&[&bytes[8..40], &bytes[PLAYER_ENVELOPE_LENGTH..]]);
    let _ = payload_length;
    bytes[40..44].copy_from_slice(&checksum.to_le_bytes());
}

fn player_id() -> PlayerId {
    PlayerId::from_bytes([
        0x00, 0x11, 0x22, 0x33, 0x44, 0x55, 0x46, 0x77, 0x88, 0x99, 0xaa, 0xbb, 0xcc, 0xdd, 0xee,
        0xff,
    ])
}

fn stored_to_save(stored: &StoredPlayer) -> PlayerSave {
    PlayerSave {
        player_id: stored.player_id,
        revision: stored.revision,
        display_name: stored.display_name.clone(),
        current: stored.current.clone(),
        yaw: stored.yaw,
        pitch: stored.pitch,
        safe: stored.safe.clone(),
        inventory: stored.inventory,
        health: stored.health,
        hunger: stored.hunger,
        saturation_milli: stored.saturation_milli,
        exhaustion_milli: stored.exhaustion_milli,
        respawn_present: stored.respawn_present,
        respawn_position: stored.respawn_position,
        respawn_dimension: stored.respawn_dimension,
        armor: stored.armor,
    }
}

fn fixture_player_save(revision: u64) -> PlayerSave {
    let mut inventory = Inventory::default();
    inventory.hotbar.selected = 3;
    inventory.hotbar.slots[0] = ItemStack {
        item: 1,
        count: 64,
        durability: 0,
    };
    inventory.hotbar.slots[4] = ItemStack {
        item: 10,
        count: 1,
        durability: 131,
    };
    inventory.hotbar.slots[6] = ItemStack {
        item: 3,
        count: 1,
        durability: 0,
    };
    inventory.backpack[0] = ItemStack {
        item: 2,
        count: 12,
        durability: 0,
    };
    inventory.backpack[7] = ItemStack {
        item: 11,
        count: 1,
        durability: 250,
    };
    inventory.backpack[26] = ItemStack {
        item: 1,
        count: 5,
        durability: 0,
    };
    PlayerSave {
        player_id: player_id(),
        revision,
        display_name: "Chen".to_owned(),
        current: PlayerLocation {
            dimension: 0,
            position: [2.5, 70.0, -3.5],
        },
        yaw: 1.25,
        pitch: -0.5,
        safe: Some(PlayerLocation {
            dimension: 0,
            position: [1.5, 65.0, -2.5],
        }),
        inventory,
        health: 13,
        hunger: 12,
        saturation_milli: 2500,
        exhaustion_milli: 1750,
        respawn_present: false,
        respawn_position: [0.0; 3],
        respawn_dimension: 0,
        armor: [ItemStack::default(); 4],
    }
}

fn armor_fixture_stacks() -> [ItemStack; 4] {
    [
        ItemStack {
            item: 58,
            count: 1,
            durability: 165,
        },
        ItemStack {
            item: 59,
            count: 1,
            durability: 0,
        },
        ItemStack {
            item: 60,
            count: 0,
            durability: 165,
        },
        ItemStack::default(),
    ]
}
fn read_go_fixture(relative: &str) -> Vec<u8> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../../../packages/")
        .join(relative);
    fs::read(&path).unwrap_or_else(|err| panic!("read {}: {err}", path.display()))
}

fn read_manifest(dir: &str) -> String {
    fs::read_to_string(PathBuf::from(dir).join("Cargo.toml")).expect("crate manifest")
}

fn read_inventory() -> String {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../../../testdata/runtime-migration/contracts.json");
    fs::read_to_string(&path).unwrap_or_else(|err| panic!("read {}: {err}", path.display()))
}

fn production_dependency_keys(manifest: &str) -> Vec<String> {
    let Some(rest) = manifest.split("[dependencies]\n").nth(1) else {
        return Vec::new();
    };
    let section = rest.split("\n[").next().unwrap_or(rest);
    section
        .lines()
        .filter_map(|line| {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                return None;
            }
            line.split('=')
                .next()
                .map(str::trim)
                .filter(|key| !key.is_empty())
                .map(str::to_string)
        })
        .collect()
}
