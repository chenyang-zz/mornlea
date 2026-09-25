//! Executable `save.passive` corpus routes for `mornlea_storage`.
//!
//! Node 3.4 seeds v1 decode and current v1 encode evidence. The shared
//! dispatcher in `storage_corpus.rs` delegates here once the controller
//! registers the reviewed routes against integrated manifest assets.

use super::value_digest::{Value, value_sha256};
use crate::runtime_corpus::{FrozenCase, InputFormat};
use mornlea_storage::{
    PassiveMob, PassiveMobsSave, StorageError, decode_passive_mobs, encode_passive_mobs_into,
    passive_mobs_encoded_len,
};
use serde_json::Value as JsonValue;
use std::collections::BTreeMap;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub struct PassiveRoute {
    pub family: &'static str,
    pub version: &'static str,
    pub operation: &'static str,
}

/// Routes the passive module executes once reviewed assets are integrated.
pub const PASSIVE_REGISTERED_ROUTES: &[PassiveRoute] = &[
    PassiveRoute {
        family: "save.passive",
        version: "1",
        operation: "decode",
    },
    PassiveRoute {
        family: "save.passive",
        version: "1",
        operation: "encode",
    },
];

fn route_is_registered(case: &FrozenCase) -> bool {
    PASSIVE_REGISTERED_ROUTES.iter().any(|route| {
        route.family == case.family
            && route.version == case.version
            && route.operation == case.operation
    })
}

#[derive(Debug)]
struct PassiveArguments {
    capacity: Option<u32>,
}

fn parse_passive_arguments(case: &FrozenCase) -> Result<PassiveArguments, String> {
    let obj = case
        .arguments
        .as_object()
        .ok_or_else(|| format!("case {} arguments must be an object", case.id))?;
    let capacity = match obj.get("capacity") {
        None => None,
        Some(value) => Some(read_u32_json(value)?),
    };
    Ok(PassiveArguments { capacity })
}

fn read_u32_json(value: &JsonValue) -> Result<u32, String> {
    value
        .as_u64()
        .and_then(|number| u32::try_from(number).ok())
        .ok_or_else(|| "capacity must be an unsigned integer".to_string())
}

fn passive_schema_version(input: &[u8]) -> Result<u32, String> {
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

fn passive_record_value(record: &PassiveMob) -> Value {
    let mut fields = BTreeMap::new();
    fields.insert("id".to_string(), Value::Unsigned(record.id));
    fields.insert(
        "dimension".to_string(),
        Value::Signed(record.dimension as i64),
    );
    fields.insert("position".to_string(), vec3_value(record.position));
    fields.insert("velocity".to_string(), vec3_value(record.velocity));
    fields.insert("on_ground".to_string(), Value::Bool(record.on_ground));
    fields.insert("yaw".to_string(), Value::F32(record.yaw));
    fields.insert("health".to_string(), Value::Unsigned(record.health as u64));
    Value::Object(fields)
}

pub fn passive_mobs_value(mobs: &mornlea_storage::PassiveMobs) -> Value {
    let records = mobs
        .records
        .iter()
        .map(passive_record_value)
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
    if case.normalized.get("kind").and_then(JsonValue::as_str) != Some("error") {
        return Err(format!("case {} expected kind error", case.id));
    }
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

pub fn execute_passive_cases(cases: &[FrozenCase]) {
    if cases.is_empty() {
        panic!("passive corpus selection executed zero cases");
    }
    for case in cases {
        assert_eq!(
            case.family, "save.passive",
            "case {} is not a save.passive row",
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
            "unregistered passive route {}/{}/{} for case {}",
            case.family,
            case.version,
            case.operation,
            case.id
        );
        execute_passive_case(case).unwrap_or_else(|err| {
            panic!("case {} failed: {err}", case.id);
        });
    }
}

pub fn execute_passive_case(case: &FrozenCase) -> Result<(), String> {
    let args = parse_passive_arguments(case)?;
    match case.operation.as_str() {
        "decode" => execute_passive_decode(case, &args),
        "encode" => execute_passive_encode(case, &args),
        other => Err(format!("unsupported passive operation {other}")),
    }
}

fn execute_passive_decode(case: &FrozenCase, _args: &PassiveArguments) -> Result<(), String> {
    let schema = passive_schema_version(&case.input)?;
    let want_version: u32 = case
        .version
        .parse()
        .map_err(|_| format!("case {} has invalid version", case.id))?;
    match decode_passive_mobs(&case.input) {
        Ok(mobs) => {
            assert_ok_outcome(case)?;
            if schema != want_version {
                return Err(format!(
                    "case {} accepted input schema {schema}, want {want_version}",
                    case.id
                ));
            }
            expected_value_digest(case, &passive_mobs_value(&mobs))
        }
        Err(err) => {
            let category = storage_error_category(&err)
                .ok_or_else(|| format!("unclassified rejection: {err}"))?;
            assert_error_category(case, category)
        }
    }
}

fn execute_passive_encode(case: &FrozenCase, args: &PassiveArguments) -> Result<(), String> {
    if case.id.ends_with("/encode/count-33") {
        let records = (1..=33)
            .map(passive_mob_minimal)
            .collect::<Vec<PassiveMob>>();
        return match encode_passive_mobs_into(
            &PassiveMobsSave {
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

    let mobs = decode_passive_mobs(&case.input).map_err(|err| err.to_string())?;
    let mut save = PassiveMobsSave {
        revision: mobs.revision,
        records: mobs.records.clone(),
    };
    if case.id.contains("/encode/unsorted-canonical") {
        save.records = fixture_passive_records_shuffled();
    }
    let required = passive_mobs_encoded_len(&save).map_err(|err| err.to_string())?;
    let buf_len = super::checked_output_capacity(case, args.capacity, required)?;
    let mut buf = vec![0u8; buf_len];
    match encode_passive_mobs_into(&save, &mut buf) {
        Err(StorageError::OutputTooSmall {
            needed: want_needed,
            available: want_available,
        }) => {
            assert_error_category(case, "output_too_small")?;
            assert_output_too_small_fields(case, want_needed, want_available)?;
            Ok(())
        }
        Err(err) => {
            let category = storage_error_category(&err)
                .ok_or_else(|| format!("unclassified encode rejection: {err}"))?;
            assert_error_category(case, category)?;
            Ok(())
        }
        Ok(written) => {
            assert_ok_outcome(case)?;
            let encoded = &buf[..written];
            let round = decode_passive_mobs(encoded).map_err(|err| err.to_string())?;
            if round.records != mobs.records {
                return Err(format!("case {} encode round-trip record drift", case.id));
            }
            expected_value_digest(case, &passive_mobs_value(&mobs))?;
            if let Some(length) = case.normalized.get("length").and_then(JsonValue::as_u64)
                && length as usize != encoded.len()
            {
                return Err(format!(
                    "case {} encoded length {}, want {}",
                    case.id,
                    encoded.len(),
                    length
                ));
            }
            let encoded_ref = case
                .encoded
                .as_ref()
                .ok_or_else(|| format!("case {} missing encoded asset", case.id))?;
            if encoded_ref.as_slice() != encoded {
                return Err(format!("case {} encoded asset mismatch", case.id));
            }
            Ok(())
        }
    }
}

fn passive_mob_minimal(id: u64) -> PassiveMob {
    PassiveMob {
        id,
        dimension: 0,
        position: [0.0, 64.0, 0.0],
        velocity: [0.0, 0.0, 0.0],
        on_ground: false,
        yaw: 0.0,
        health: 1,
    }
}

fn fixture_passive_records_shuffled() -> Vec<PassiveMob> {
    vec![
        PassiveMob {
            id: 0x8000_0000_0000_0002,
            dimension: 0,
            position: [-12.5, 70.25, 3.5],
            velocity: [-1.25, 0.0, 0.5],
            on_ground: true,
            yaw: 1.25,
            health: 17,
        },
        PassiveMob {
            id: 0x4000_0000_0000_0001,
            dimension: 0,
            position: [0.5, 64.0, -9.75],
            velocity: [0.0, -3.25, 0.0],
            on_ground: false,
            yaw: -2.5,
            health: 20,
        },
        PassiveMob {
            id: 1,
            dimension: 0,
            position: [8.5, 65.5, 9.75],
            velocity: [2.0, 0.0, -2.0],
            on_ground: true,
            yaw: 3.0,
            health: 1,
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use mornlea_storage::encode_passive_mobs;

    fn fixture_decode_case(id: &str, version: &str, input: Vec<u8>, digest: String) -> FrozenCase {
        FrozenCase {
            id: id.to_string(),
            family: "save.passive".to_string(),
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
    fn passive_corpus_rejects_zero_case_selection() {
        let err = std::panic::catch_unwind(|| execute_passive_cases(&[]));
        assert!(
            err.is_err(),
            "zero-case passive selection must fail before codec execution"
        );
    }

    #[test]
    fn passive_routes_recognize_registered_paths() {
        assert!(route_is_registered(&FrozenCase {
            id: "save.passive/1/decode/v1-fixture".to_string(),
            family: "save.passive".to_string(),
            version: "1".to_string(),
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
    fn passive_v1_fixture_decode_executes_locally() {
        let golden =
            include_bytes!("../../../../../server/storage/passive/testdata/passive-mobs-v1.bin");
        let mobs = decode_passive_mobs(golden).expect("decode v1 fixture");
        let digest = value_sha256(&passive_mobs_value(&mobs));
        let case = fixture_decode_case(
            "save.passive/1/decode/v1-fixture",
            "1",
            golden.to_vec(),
            digest,
        );
        execute_passive_case(&case).expect("local v1 fixture decode");
    }

    #[test]
    fn passive_position_digest_mutation_fails_comparison() {
        let golden =
            include_bytes!("../../../../../server/storage/passive/testdata/passive-mobs-v1.bin");
        let mobs = decode_passive_mobs(golden).expect("decode v1 fixture");
        let mut stale = mobs.clone();
        if let Some(record) = stale.records.first_mut() {
            record.position[0] += 0.25;
        }
        let case = fixture_decode_case(
            "save.passive/1/decode/v1-fixture",
            "1",
            golden.to_vec(),
            value_sha256(&passive_mobs_value(&stale)),
        );
        let err = execute_passive_case(&case).expect_err("stale position digest must fail");
        assert!(
            err.contains("value digest mismatch"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn passive_reserved_verdict_digest_mutation_fails_comparison() {
        let golden =
            include_bytes!("../../../../../server/storage/passive/testdata/passive-mobs-v1.bin");
        let mobs = decode_passive_mobs(golden).expect("decode v1 fixture");
        let mut stale = mobs.clone();
        if let Some(record) = stale.records.first_mut() {
            record.on_ground = !record.on_ground;
        }
        let case = fixture_decode_case(
            "save.passive/1/decode/v1-fixture",
            "1",
            golden.to_vec(),
            value_sha256(&passive_mobs_value(&stale)),
        );
        let err = execute_passive_case(&case).expect_err("stale on_ground digest must fail");
        assert!(
            err.contains("value digest mismatch"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn passive_unsorted_encode_matches_canonical_bytes() {
        let shuffled = PassiveMobsSave {
            revision: 3,
            records: fixture_passive_records_shuffled(),
        };
        let canonical = encode_passive_mobs(&PassiveMobsSave {
            revision: 3,
            records: {
                let mut sorted = fixture_passive_records_shuffled();
                sorted.sort_by_key(|record| record.id);
                sorted
            },
        })
        .expect("encode sorted");
        let from_shuffle = encode_passive_mobs(&shuffled).expect("encode shuffled");
        assert_eq!(canonical, from_shuffle);
    }
}
