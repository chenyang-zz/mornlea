//! Executable `save.region` corpus routes for `mornlea_storage`.
//!
//! Node 1.3 seeds superblock and bank decode/encode evidence. The shared
//! dispatcher in `storage_corpus.rs` delegates here once the controller
//! registers the reviewed routes against integrated manifest assets.

use super::value_digest::{Value, value_sha256};
use mornlea_storage::{
    decode_region_bank, decode_superblock, encode_region_bank_into, encode_superblock_into,
    select_region_bank, BANK_SIZE, RegionBank, RegionEntry, RegionKey, StorageError,
};
use crate::runtime_corpus::{FrozenCase, InputFormat};
use serde_json::Value as JsonValue;
use std::collections::BTreeMap;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub struct RegionRoute {
    pub family: &'static str,
    pub version: &'static str,
    pub operation: &'static str,
}

/// Routes the region module executes once reviewed assets are integrated.
pub const REGION_REGISTERED_ROUTES: &[RegionRoute] = &[
    RegionRoute {
        family: "save.region",
        version: "1",
        operation: "decode",
    },
    RegionRoute {
        family: "save.region",
        version: "1",
        operation: "encode",
    },
    RegionRoute {
        family: "save.region",
        version: "1",
        operation: "order",
    },
];

fn route_is_registered(case: &FrozenCase) -> bool {
    REGION_REGISTERED_ROUTES.iter().any(|route| {
        route.family == case.family
            && route.version == case.version
            && route.operation == case.operation
    })
}

#[derive(Debug)]
struct RegionArguments {
    file_size: i64,
    component: String,
    capacity: Option<u32>,
}

fn parse_region_arguments(case: &FrozenCase) -> Result<(RegionArguments, RegionKey), String> {
    let obj = case
        .arguments
        .as_object()
        .ok_or_else(|| format!("case {} arguments must be an object", case.id))?;
    let dimension = read_i32_field(obj, "dimension")?;
    let x = read_i32_field(obj, "x")?;
    let z = read_i32_field(obj, "z")?;
    let file_size = read_decimal_i64_field(obj, "file_size")?;
    let component = read_string_field(obj, "component")?;
    let capacity = match obj.get("capacity") {
        None => None,
        Some(value) => Some(read_u32_json(value)?),
    };
    Ok((
        RegionArguments {
            file_size,
            component,
            capacity,
        },
        RegionKey {
            dimension,
            x,
            z,
        },
    ))
}

fn read_i32_field(obj: &serde_json::Map<String, JsonValue>, key: &str) -> Result<i32, String> {
    let value = obj.get(key).ok_or_else(|| format!("missing {key}"))?;
    value
        .as_i64()
        .and_then(|number| i32::try_from(number).ok())
        .ok_or_else(|| format!("{key} must be a signed 32-bit integer"))
}

fn read_decimal_i64_field(
    obj: &serde_json::Map<String, JsonValue>,
    key: &str,
) -> Result<i64, String> {
    let value = obj.get(key).ok_or_else(|| format!("missing {key}"))?;
    let text = value
        .as_str()
        .ok_or_else(|| format!("{key} must be a decimal string"))?;
    text.parse::<i64>()
        .map_err(|_| format!("{key} must be a signed decimal string"))
}

fn read_string_field(
    obj: &serde_json::Map<String, JsonValue>,
    key: &str,
) -> Result<String, String> {
    obj.get(key)
        .and_then(JsonValue::as_str)
        .map(str::to_string)
        .ok_or_else(|| format!("{key} must be a string"))
}

fn read_u32_json(value: &JsonValue) -> Result<u32, String> {
    value
        .as_u64()
        .and_then(|number| u32::try_from(number).ok())
        .ok_or_else(|| "capacity must be an unsigned integer".to_string())
}

fn region_schema_version(input: &[u8]) -> Result<u32, String> {
    if input.len() < 8 {
        return Err("input shorter than schema header".to_string());
    }
    Ok(u32::from_le_bytes(input[4..8].try_into().expect("slice length")))
}

fn storage_error_category(err: &StorageError) -> Option<&'static str> {
    match err {
        StorageError::Corrupt(_) => Some("corrupt"),
        StorageError::FutureVersion(_) => Some("future_version"),
        StorageError::OutputTooSmall { .. } => Some("output_too_small"),
    }
}

fn entry_value(entry: &RegionEntry) -> Value {
    let mut fields = BTreeMap::new();
    fields.insert(
        "offset_sector".to_string(),
        Value::Unsigned(entry.offset_sector as u64),
    );
    fields.insert(
        "payload_crc32c".to_string(),
        Value::Unsigned(entry.payload_crc32c as u64),
    );
    fields.insert(
        "payload_length".to_string(),
        Value::Unsigned(entry.payload_length as u64),
    );
    fields.insert(
        "revision".to_string(),
        Value::Unsigned(entry.revision),
    );
    fields.insert(
        "sector_count".to_string(),
        Value::Unsigned(entry.sector_count as u64),
    );
    Value::Object(fields)
}

fn bank_value(bank: &RegionBank) -> Value {
    let entries = bank.entries.iter().map(entry_value).collect::<Vec<_>>();
    let mut fields = BTreeMap::new();
    fields.insert("entries".to_string(), Value::Array(entries));
    fields.insert(
        "generation".to_string(),
        Value::Unsigned(bank.generation),
    );
    Value::Object(fields)
}

fn superblock_value() -> Value {
    Value::Object(BTreeMap::new())
}

fn order_value(selected_index: usize, bank: &RegionBank) -> Value {
    let mut fields = BTreeMap::new();
    fields.insert("bank".to_string(), bank_value(bank));
    fields.insert(
        "selected_index".to_string(),
        Value::Unsigned(selected_index as u64),
    );
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

pub fn execute_region_cases(cases: &[FrozenCase]) {
    if cases.is_empty() {
        panic!("region corpus selection executed zero cases");
    }
    for case in cases {
        assert_eq!(
            case.family, "save.region",
            "case {} is not a save.region row",
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
            "unregistered region route {}/{}/{} for case {}",
            case.family, case.version, case.operation, case.id
        );
        execute_region_case(case).unwrap_or_else(|err| {
            panic!("case {} failed: {err}", case.id);
        });
    }
}

pub fn execute_region_case(case: &FrozenCase) -> Result<(), String> {
    let (args, key) = parse_region_arguments(case)?;
    match case.operation.as_str() {
        "order" => execute_region_order(case, &args, key),
        "decode" | "encode" => {
            let schema = region_schema_version(&case.input)?;
            let want_version: u32 = case
                .version
                .parse()
                .map_err(|_| format!("case {} has invalid version", case.id))?;
            if schema != want_version {
                return Err(format!(
                    "case {} input schema {schema}, want {want_version}",
                    case.id
                ));
            }
            match case.operation.as_str() {
                "decode" => execute_region_decode(case, &args, key),
                "encode" => execute_region_encode(case, &args, key),
                other => Err(format!("unsupported region operation {other}")),
            }
        }
        other => Err(format!("unsupported region operation {other}")),
    }
}

fn execute_region_order(
    case: &FrozenCase,
    args: &RegionArguments,
    key: RegionKey,
) -> Result<(), String> {
    if args.component != "banks" {
        return Err(format!(
            "case {} order requires component \"banks\"",
            case.id
        ));
    }
    if case.input.len() != 2 * BANK_SIZE {
        return Err(format!(
            "case {} order input length {}, want {}",
            case.id,
            case.input.len(),
            2 * BANK_SIZE
        ));
    }
    let (bank_a_bytes, bank_b_bytes) = case.input.split_at(BANK_SIZE);
    let bank_a = decode_region_bank(key, bank_a_bytes, args.file_size);
    let bank_b = decode_region_bank(key, bank_b_bytes, args.file_size);
    match select_region_bank(bank_a, bank_b) {
        Ok((bank, index)) => {
            assert_ok_outcome(case)?;
            expected_value_digest(case, &order_value(index, &bank))
        }
        Err(err) => {
            let category = storage_error_category(&err)
                .ok_or_else(|| format!("unclassified rejection: {err}"))?;
            assert_error_category(case, category)
        }
    }
}

fn execute_region_decode(
    case: &FrozenCase,
    args: &RegionArguments,
    key: RegionKey,
) -> Result<(), String> {
    match args.component.as_str() {
        "superblock" => match decode_superblock(key, &case.input) {
            Ok(()) => {
                assert_ok_outcome(case)?;
                expected_value_digest(case, &superblock_value())
            }
            Err(err) => {
                let category = storage_error_category(&err)
                    .ok_or_else(|| format!("unclassified rejection: {err}"))?;
                assert_error_category(case, category)
            }
        },
        "bank" => match decode_region_bank(key, &case.input, args.file_size) {
            Ok(bank) => {
                assert_ok_outcome(case)?;
                expected_value_digest(case, &bank_value(&bank))
            }
            Err(err) => {
                let category = storage_error_category(&err)
                    .ok_or_else(|| format!("unclassified rejection: {err}"))?;
                assert_error_category(case, category)
            }
        },
        other => Err(format!("decode component {other} is unsupported")),
    }
}

fn execute_region_encode(
    case: &FrozenCase,
    args: &RegionArguments,
    key: RegionKey,
) -> Result<(), String> {
    let (value, encoded) = match args.component.as_str() {
        "superblock" => {
            decode_superblock(key, &case.input).map_err(|err| err.to_string())?;
            let digest_value = superblock_value();
            let mut out = vec![0u8; mornlea_storage::SECTOR_SIZE as usize];
            let len = encode_superblock_into(key, &mut out).map_err(|err| err.to_string())?;
            (digest_value, out[..len].to_vec())
        }
        "bank" => {
            let bank = decode_region_bank(key, &case.input, args.file_size)
                .map_err(|err| err.to_string())?;
            let digest_value = bank_value(&bank);
            let mut out = vec![0u8; mornlea_storage::BANK_SIZE];
            let len =
                encode_region_bank_into(key, &bank, &mut out).map_err(|err| err.to_string())?;
            (digest_value, out[..len].to_vec())
        }
        other => return Err(format!("encode component {other} is unsupported")),
    };

    if let Some(capacity) = args.capacity {
        if (capacity as usize) < encoded.len() {
            assert_error_category(case, "output_too_small")?;
            return Ok(());
        }
    }

    if case.normalized.get("kind").and_then(JsonValue::as_str) == Some("ok") {
        expected_value_digest(case, &value)?;
        if let Some(length) = case.normalized.get("length").and_then(JsonValue::as_u64) {
            if length as usize != encoded.len() {
                return Err(format!(
                    "case {} encoded length {}, want {}",
                    case.id,
                    encoded.len(),
                    length
                ));
            }
        }
        let encoded_ref = case
            .encoded
            .as_ref()
            .ok_or_else(|| format!("case {} missing encoded asset", case.id))?;
        if encoded_ref.as_slice() != encoded.as_slice() {
            return Err(format!("case {} encoded bytes mismatch", case.id));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime_corpus::{load_cases_for_consumer, CorpusConsumer};

    fn region_cases_from_manifest() -> Vec<FrozenCase> {
        load_cases_for_consumer(CorpusConsumer::Storage)
            .into_iter()
            .filter(|case| case.family == "save.region")
            .collect()
    }

    #[test]
    fn region_corpus_executes_integrated_seed_cases() {
        let cases = region_cases_from_manifest();
        execute_region_cases(&cases);
    }

    #[test]
    fn region_corpus_rejects_unregistered_route_before_dispatch() {
        let case = FrozenCase {
            id: "save.region/1/decode/unregistered".to_string(),
            family: "save.region".to_string(),
            version: "1".to_string(),
            consumer: CorpusConsumer::Storage,
            packet_key: None,
            operation: "inspect".to_string(),
            arguments: serde_json::json!({
                "dimension": -3,
                "x": -1,
                "z": 2,
                "file_size": "65536",
                "component": "bank"
            }),
            input_format: InputFormat::Binary,
            input: vec![0x00; BANK_SIZE],
            input_json: None,
            normalized: serde_json::json!({"kind":"ok","category":"save","value_sha256":"sha256:00"}),
            encoded: None,
            category: "save".to_string(),
        };
        let err = std::panic::catch_unwind(|| execute_region_cases(std::slice::from_ref(&case)));
        assert!(err.is_err(), "unknown region routes must fail before dispatch");
    }

    #[test]
    fn region_order_executes_integrated_order_cases() {
        let cases = region_cases_from_manifest()
            .into_iter()
            .filter(|case| case.operation == "order")
            .collect::<Vec<_>>();
        if cases.is_empty() {
            panic!("no integrated save.region order cases");
        }
        execute_region_cases(&cases);
    }
}
