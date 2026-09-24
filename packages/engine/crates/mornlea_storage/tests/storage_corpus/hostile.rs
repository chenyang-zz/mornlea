//! Executable `save.hostile` corpus routes for `mornlea_storage`.
//!
//! Node 3.2 seeds v1/v2 decode and current v2 encode evidence. The shared
//! dispatcher in `storage_corpus.rs` delegates here once the controller
//! registers the reviewed routes against integrated manifest assets.

use super::value_digest::{Value, value_sha256};
use crate::runtime_corpus::{FrozenCase, InputFormat};
use mornlea_storage::{
    HostileMob, HostileMobsSave, PlayerId, StorageError, decode_hostile_mobs,
    encode_hostile_mobs_into, hostile_mobs_encoded_len,
};
use serde_json::Value as JsonValue;
use std::collections::BTreeMap;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub struct HostileRoute {
    pub family: &'static str,
    pub version: &'static str,
    pub operation: &'static str,
}

/// Routes the hostile module executes once reviewed assets are integrated.
pub const HOSTILE_REGISTERED_ROUTES: &[HostileRoute] = &[
    HostileRoute {
        family: "save.hostile",
        version: "1",
        operation: "decode",
    },
    HostileRoute {
        family: "save.hostile",
        version: "2",
        operation: "decode",
    },
    HostileRoute {
        family: "save.hostile",
        version: "2",
        operation: "encode",
    },
];

fn route_is_registered(case: &FrozenCase) -> bool {
    HOSTILE_REGISTERED_ROUTES.iter().any(|route| {
        route.family == case.family
            && route.version == case.version
            && route.operation == case.operation
    })
}

#[derive(Debug)]
struct HostileArguments {
    capacity: Option<u32>,
}

fn parse_hostile_arguments(case: &FrozenCase) -> Result<HostileArguments, String> {
    let obj = case
        .arguments
        .as_object()
        .ok_or_else(|| format!("case {} arguments must be an object", case.id))?;
    let capacity = match obj.get("capacity") {
        None => None,
        Some(value) => Some(read_u32_json(value)?),
    };
    Ok(HostileArguments { capacity })
}

fn read_u32_json(value: &JsonValue) -> Result<u32, String> {
    value
        .as_u64()
        .and_then(|number| u32::try_from(number).ok())
        .ok_or_else(|| "capacity must be an unsigned integer".to_string())
}

fn hostile_schema_version(input: &[u8]) -> Result<u32, String> {
    if input.len() < 12 {
        return Err("input shorter than schema header".to_string());
    }
    Ok(u32::from_le_bytes(
        input[8..12].try_into().expect("slice length"),
    ))
}

fn storage_error_category(err: &StorageError) -> Option<&'static str> {
    match err {
        StorageError::Corrupt(_) => Some("corrupt"),
        StorageError::FutureVersion(_) => Some("future_version"),
        StorageError::OutputTooSmall { .. } => Some("output_too_small"),
    }
}

fn vec3_value(values: [f32; 3]) -> Value {
    Value::Array(vec![
        Value::F32(values[0]),
        Value::F32(values[1]),
        Value::F32(values[2]),
    ])
}

fn hostile_record_value(record: &HostileMob) -> Value {
    let mut fields = BTreeMap::new();
    fields.insert("id".to_string(), Value::Unsigned(record.id));
    fields.insert("dimension".to_string(), Value::Signed(record.dimension as i64));
    fields.insert("position".to_string(), vec3_value(record.position));
    fields.insert("velocity".to_string(), vec3_value(record.velocity));
    fields.insert("on_ground".to_string(), Value::Bool(record.on_ground));
    fields.insert("yaw".to_string(), Value::F32(record.yaw));
    fields.insert("health".to_string(), Value::Unsigned(record.health as u64));
    fields.insert(
        "attack_cooldown".to_string(),
        Value::Unsigned(record.attack_cooldown as u64),
    );
    fields.insert(
        "hurt_cooldown".to_string(),
        Value::Unsigned(record.hurt_cooldown as u64),
    );
    fields.insert(
        "burn_cooldown".to_string(),
        Value::Unsigned(record.burn_cooldown as u64),
    );
    fields.insert("has_target".to_string(), Value::Bool(record.has_target));
    fields.insert(
        "player_id".to_string(),
        Value::Bytes(record.player_id.to_bytes().to_vec()),
    );
    fields.insert(
        "next_repath_ticks".to_string(),
        Value::Unsigned(record.next_repath_ticks),
    );
    fields.insert(
        "distant_ticks".to_string(),
        Value::Unsigned(record.distant_ticks as u64),
    );
    fields.insert("kind".to_string(), Value::Unsigned(record.kind as u64));
    Value::Object(fields)
}

pub fn hostile_mobs_value(mobs: &mornlea_storage::HostileMobs) -> Value {
    let records = mobs
        .records
        .iter()
        .map(hostile_record_value)
        .collect::<Vec<_>>();
    let mut fields = BTreeMap::new();
    fields.insert("revision".to_string(), Value::Unsigned(mobs.revision));
    fields.insert("records".to_string(), Value::Array(records));
    Value::Object(fields)
}

fn expected_value_digest(case: &FrozenCase, value: &Value) -> Result<(), String> {
    let expected = case
        .normalized
        .get("value_sha256")
        .and_then(JsonValue::as_str)
        .ok_or_else(|| format!("case {} missing value_sha256", case.id))?;
    let actual = value_sha256(value);
    if actual != expected {
        return Err(format!(
            "case {} value digest mismatch: got {actual}, want {expected}",
            case.id
        ));
    }
    Ok(())
}

fn assert_ok_outcome(case: &FrozenCase) -> Result<(), String> {
    match case.normalized.get("kind").and_then(JsonValue::as_str) {
        Some("ok") => Ok(()),
        _ => Err(format!("case {} expected kind ok", case.id)),
    }
}

fn assert_error_category(case: &FrozenCase, category: &str) -> Result<(), String> {
    match case.normalized.get("category").and_then(JsonValue::as_str) {
        Some(value) if value == category => Ok(()),
        other => Err(format!(
            "case {} category mismatch: got {:?}, want {category}",
            case.id, other
        )),
    }
}

fn assert_output_too_small_fields(
    case: &FrozenCase,
    needed: usize,
    available: usize,
) -> Result<(), String> {
    let want_needed = case
        .normalized
        .get("needed")
        .and_then(JsonValue::as_u64)
        .ok_or_else(|| format!("case {} missing needed", case.id))?;
    let want_available = case
        .normalized
        .get("available")
        .and_then(JsonValue::as_u64)
        .ok_or_else(|| format!("case {} missing available", case.id))?;
    if needed as u64 != want_needed || available as u64 != want_available {
        return Err(format!(
            "case {} output_too_small fields mismatch: got needed {needed} available {available}, want needed {want_needed} available {want_available}",
            case.id
        ));
    }
    Ok(())
}

pub fn execute_hostile_cases(cases: &[FrozenCase]) {
    if cases.is_empty() {
        panic!("hostile corpus selection executed zero cases");
    }
    for case in cases {
        assert_eq!(
            case.family, "save.hostile",
            "case {} is not a save.hostile row",
            case.id
        );
        assert_eq!(
            case.input_format,
            InputFormat::Binary,
            "case {} must use binary input",
            case.id
        );
        assert!(
            route_is_registered(case),
            "unregistered hostile route {}/{}/{} for case {}",
            case.family,
            case.version,
            case.operation,
            case.id
        );
        execute_hostile_case(case).unwrap_or_else(|err| {
            panic!("case {} failed: {err}", case.id);
        });
    }
}

pub fn execute_hostile_case(case: &FrozenCase) -> Result<(), String> {
    let args = parse_hostile_arguments(case)?;
    match case.operation.as_str() {
        "decode" => execute_hostile_decode(case, &args),
        "encode" => execute_hostile_encode(case, &args),
        other => Err(format!("unsupported hostile operation {other}")),
    }
}

fn execute_hostile_decode(case: &FrozenCase, _args: &HostileArguments) -> Result<(), String> {
    let schema = hostile_schema_version(&case.input)?;
    let want_version: u32 = case
        .version
        .parse()
        .map_err(|_| format!("case {} has invalid version", case.id))?;
    match decode_hostile_mobs(&case.input) {
        Ok(mobs) => {
            assert_ok_outcome(case)?;
            if schema != want_version {
                return Err(format!(
                    "case {} accepted input schema {schema}, want {want_version}",
                    case.id
                ));
            }
            expected_value_digest(case, &hostile_mobs_value(&mobs))
        }
        Err(err) => {
            let category = storage_error_category(&err)
                .ok_or_else(|| format!("unclassified rejection: {err}"))?;
            assert_error_category(case, category)
        }
    }
}

fn execute_hostile_encode(case: &FrozenCase, args: &HostileArguments) -> Result<(), String> {
    if case.id.ends_with("/encode/count-65") {
        let records = (1..=65)
            .map(hostile_nightcrawler)
            .collect::<Vec<HostileMob>>();
        return match encode_hostile_mobs_into(
            &HostileMobsSave {
                revision: 1,
                records,
            },
            &mut [],
        ) {
            Ok(_) => Err(format!("case {} expected encode rejection", case.id)),
            Err(err) => {
                let category = storage_error_category(&err)
                    .ok_or_else(|| format!("unclassified encode rejection: {err}"))?;
                assert_error_category(case, category)
            }
        };
    }

    let mobs = decode_hostile_mobs(&case.input).map_err(|err| err.to_string())?;
    let digest = value_sha256(&hostile_mobs_value(&mobs));
    let mut save = HostileMobsSave {
        revision: mobs.revision,
        records: mobs.records.clone(),
    };
    if case.id.contains("/encode/unsorted-canonical") {
        save.records = fixture_hostile_records_shuffled();
    }
    let required = hostile_mobs_encoded_len(&save).map_err(|err| err.to_string())?;
    let buf_len = args.capacity.map(|cap| cap as usize).unwrap_or(required);
    let mut buf = vec![0u8; buf_len];
    match encode_hostile_mobs_into(&save, &mut buf) {
        Err(StorageError::OutputTooSmall {
            needed: want_needed,
            available: want_available,
        }) => {
            assert_error_category(case, "output_too_small")?;
            assert_output_too_small_fields(case, want_needed, want_available)?;
            return Ok(());
        }
        Err(err) => {
            let category = storage_error_category(&err)
                .ok_or_else(|| format!("unclassified encode rejection: {err}"))?;
            assert_error_category(case, category)?;
            return Ok(());
        }
        Ok(written) => {
            assert_ok_outcome(case)?;
            let encoded = &buf[..written];
            let round = decode_hostile_mobs(encoded).map_err(|err| err.to_string())?;
            if round.records != mobs.records {
                return Err(format!("case {} encode round-trip record drift", case.id));
            }
            if let Some(expected_encoded) = &case.encoded {
                if expected_encoded.as_slice() != encoded {
                    return Err(format!("case {} encoded asset mismatch", case.id));
                }
            }
            let expected_digest = case
                .normalized
                .get("value_sha256")
                .and_then(JsonValue::as_str)
                .ok_or_else(|| format!("case {} missing value_sha256", case.id))?;
            if digest != expected_digest {
                return Err(format!(
                    "case {} value digest mismatch: got {digest}, want {expected_digest}",
                    case.id
                ));
            }
            Ok(())
        }
    }
}

fn hostile_nightcrawler(id: u64) -> HostileMob {
    HostileMob {
        id,
        dimension: 0,
        position: [0.0, 64.0, 0.0],
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

fn fixture_hostile_target_player_id() -> PlayerId {
    PlayerId::from_bytes([
        0x6f, 0xce, 0x82, 0x77, 0xa9, 0x33, 0x46, 0xcb, 0x9a, 0x1f, 0xda, 0x13, 0xb7, 0xee, 0x56,
        0x44,
    ])
}

fn fixture_hostile_records_shuffled() -> Vec<HostileMob> {
    vec![
        HostileMob {
            id: 0x8000_0000_0000_0002,
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
            player_id: fixture_hostile_target_player_id(),
            next_repath_ticks: 905,
            distant_ticks: 120,
            kind: 1,
        },
        HostileMob {
            id: 0x4000_0000_0000_0001,
            dimension: 0,
            position: [0.5, 64.0, -9.75],
            velocity: [0.0, -3.25, 0.0],
            on_ground: false,
            yaw: -2.5,
            health: 20,
            attack_cooldown: 0,
            hurt_cooldown: 0,
            burn_cooldown: 0,
            has_target: false,
            player_id: PlayerId::from_bytes([0; 16]),
            next_repath_ticks: 0,
            distant_ticks: 0,
            kind: 0,
        },
        HostileMob {
            id: 1,
            dimension: 0,
            position: [8.5, 65.5, 9.75],
            velocity: [2.0, 0.0, -2.0],
            on_ground: true,
            yaw: 3.0,
            health: 1,
            attack_cooldown: 0,
            hurt_cooldown: 0,
            burn_cooldown: 19,
            has_target: false,
            player_id: PlayerId::from_bytes([0; 16]),
            next_repath_ticks: 0,
            distant_ticks: 600,
            kind: 0,
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use mornlea_storage::encode_hostile_mobs;

    fn fixture_decode_case(id: &str, version: &str, input: Vec<u8>, digest: String) -> FrozenCase {
        FrozenCase {
            id: id.to_string(),
            family: "save.hostile".to_string(),
            version: version.to_string(),
            consumer: crate::runtime_corpus::CorpusConsumer::Storage,
            packet_key: None,
            operation: "decode".to_string(),
            arguments: JsonValue::Object(serde_json::Map::new()),
            input_format: InputFormat::Binary,
            input,
            input_json: None,
            normalized: serde_json::json!({
                "kind": "ok",
                "category": "save",
                "value_sha256": digest
            }),
            encoded: None,
            category: "save".to_string(),
        }
    }

    #[test]
    fn hostile_corpus_rejects_zero_case_selection() {
        let err = std::panic::catch_unwind(|| execute_hostile_cases(&[]));
        assert!(
            err.is_err(),
            "zero-case hostile selection must fail before codec execution"
        );
    }

    #[test]
    fn hostile_routes_recognize_registered_paths() {
        assert!(route_is_registered(&FrozenCase {
            id: "save.hostile/2/decode/v2-fixture".to_string(),
            family: "save.hostile".to_string(),
            version: "2".to_string(),
            consumer: crate::runtime_corpus::CorpusConsumer::Storage,
            packet_key: None,
            operation: "decode".to_string(),
            arguments: JsonValue::Object(serde_json::Map::new()),
            input_format: InputFormat::Binary,
            input: vec![],
            input_json: None,
            normalized: JsonValue::Object(serde_json::Map::new()),
            encoded: None,
            category: "save".to_string(),
        }));
    }

    #[test]
    fn hostile_v2_fixture_decode_executes_locally() {
        let golden = include_bytes!("../../../../../server/storage/hostile/testdata/hostile-mobs-v2.bin");
        let mobs = decode_hostile_mobs(golden).expect("decode v2 fixture");
        let digest = value_sha256(&hostile_mobs_value(&mobs));
        let case = fixture_decode_case(
            "save.hostile/2/decode/v2-fixture",
            "2",
            golden.to_vec(),
            digest,
        );
        execute_hostile_case(&case).expect("local v2 fixture decode");
    }

    #[test]
    fn hostile_has_target_digest_mutation_fails_comparison() {
        let golden = include_bytes!("../../../../../server/storage/hostile/testdata/hostile-mobs-v2.bin");
        let mobs = decode_hostile_mobs(golden).expect("decode v2 fixture");
        let mut stale = mobs.clone();
        if let Some(record) = stale.records.iter_mut().find(|record| record.has_target) {
            record.has_target = false;
        }
        let case = fixture_decode_case(
            "save.hostile/2/decode/v2-fixture",
            "2",
            golden.to_vec(),
            value_sha256(&hostile_mobs_value(&stale)),
        );
        let err = execute_hostile_case(&case).expect_err("stale has_target digest must fail");
        assert!(
            err.contains("value digest mismatch"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn hostile_kind_digest_mutation_fails_comparison() {
        let golden = include_bytes!("../../../../../server/storage/hostile/testdata/hostile-mobs-v2.bin");
        let mobs = decode_hostile_mobs(golden).expect("decode v2 fixture");
        let mut stale = mobs.clone();
        if let Some(record) = stale.records.iter_mut().find(|record| record.kind == 1) {
            record.kind = 0;
        }
        let case = fixture_decode_case(
            "save.hostile/2/decode/v2-fixture",
            "2",
            golden.to_vec(),
            value_sha256(&hostile_mobs_value(&stale)),
        );
        let err = execute_hostile_case(&case).expect_err("stale kind digest must fail");
        assert!(
            err.contains("value digest mismatch"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn hostile_unsorted_encode_matches_canonical_bytes() {
        let shuffled = HostileMobsSave {
            revision: 3,
            records: fixture_hostile_records_shuffled(),
        };
        let canonical = encode_hostile_mobs(&HostileMobsSave {
            revision: 3,
            records: {
                let mut sorted = fixture_hostile_records_shuffled();
                sorted.sort_by_key(|record| record.id);
                sorted
            },
        })
        .expect("encode sorted");
        let from_shuffle = encode_hostile_mobs(&shuffled).expect("encode shuffled");
        assert_eq!(canonical, from_shuffle);
    }
}
