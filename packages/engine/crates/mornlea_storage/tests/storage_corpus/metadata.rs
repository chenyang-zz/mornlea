//! Executable `save.world-metadata` corpus routes for `mornlea_storage`.
//!
//! Node 2.4 seeds v1..v6 decode and current encode evidence. The shared
//! dispatcher in `storage_corpus.rs` delegates here once the controller
//! registers the reviewed routes against integrated manifest assets.

use super::value_digest::{Value, value_sha256};
use mornlea_storage::{
    Metadata, MetadataChunkPos, StorageError, decode_world_metadata, encode_world_metadata_into,
    world_metadata_encoded_len,
};
use crate::runtime_corpus::{FrozenCase, InputFormat};
use serde_json::Value as JsonValue;
use std::collections::BTreeMap;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub struct MetadataRoute {
    pub family: &'static str,
    pub version: &'static str,
    pub operation: &'static str,
}

/// Routes the metadata module executes once reviewed assets are integrated.
pub const METADATA_REGISTERED_ROUTES: &[MetadataRoute] = &[
    MetadataRoute {
        family: "save.world-metadata",
        version: "1",
        operation: "decode",
    },
    MetadataRoute {
        family: "save.world-metadata",
        version: "2",
        operation: "decode",
    },
    MetadataRoute {
        family: "save.world-metadata",
        version: "3",
        operation: "decode",
    },
    MetadataRoute {
        family: "save.world-metadata",
        version: "4",
        operation: "decode",
    },
    MetadataRoute {
        family: "save.world-metadata",
        version: "5",
        operation: "decode",
    },
    MetadataRoute {
        family: "save.world-metadata",
        version: "6",
        operation: "decode",
    },
    MetadataRoute {
        family: "save.world-metadata",
        version: "6",
        operation: "encode",
    },
];

fn route_is_registered(case: &FrozenCase) -> bool {
    METADATA_REGISTERED_ROUTES.iter().any(|route| {
        route.family == case.family
            && route.version == case.version
            && route.operation == case.operation
    })
}

#[derive(Debug)]
struct MetadataArguments {
    capacity: Option<u32>,
}

fn parse_metadata_arguments(case: &FrozenCase) -> Result<MetadataArguments, String> {
    let obj = case
        .arguments
        .as_object()
        .ok_or_else(|| format!("case {} arguments must be an object", case.id))?;
    let capacity = match obj.get("capacity") {
        None => None,
        Some(value) => Some(read_u32_json(value)?),
    };
    Ok(MetadataArguments { capacity })
}

fn read_u32_json(value: &JsonValue) -> Result<u32, String> {
    value
        .as_u64()
        .and_then(|number| u32::try_from(number).ok())
        .ok_or_else(|| "capacity must be an unsigned integer".to_string())
}

fn metadata_schema_version(input: &[u8]) -> Result<u32, String> {
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

fn chunk_pos_value(pos: MetadataChunkPos) -> Value {
    let mut fields = BTreeMap::new();
    fields.insert("x".to_string(), Value::Signed(pos.x as i64));
    fields.insert("z".to_string(), Value::Signed(pos.z as i64));
    Value::Object(fields)
}

pub fn metadata_value(metadata: &Metadata) -> Value {
    let mut fields = BTreeMap::new();
    fields.insert(
        "format_version".to_string(),
        Value::Unsigned(metadata.format_version as u64),
    );
    fields.insert("seed".to_string(), Value::Signed(metadata.seed));
    fields.insert(
        "spawn_dimension".to_string(),
        Value::Signed(metadata.spawn_dimension as i64),
    );
    fields.insert(
        "spawn_anchor".to_string(),
        chunk_pos_value(metadata.spawn_anchor),
    );
    fields.insert(
        "world_time_ticks".to_string(),
        Value::Unsigned(metadata.world_time_ticks),
    );
    fields.insert(
        "day_phase_offset".to_string(),
        Value::Unsigned(metadata.day_phase_offset),
    );
    fields.insert(
        "weather_kind".to_string(),
        Value::Unsigned(metadata.weather_kind as u64),
    );
    fields.insert(
        "weather_ticks_remaining".to_string(),
        Value::Unsigned(metadata.weather_ticks_remaining as u64),
    );
    fields.insert(
        "depths_spawn_anchor".to_string(),
        chunk_pos_value(metadata.depths_spawn_anchor),
    );
    fields.insert(
        "depths_seed_salt".to_string(),
        Value::Unsigned(metadata.depths_seed_salt),
    );
    fields.insert(
        "difficulty".to_string(),
        Value::Unsigned(metadata.difficulty as u64),
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

pub fn execute_metadata_cases(cases: &[FrozenCase]) {
    if cases.is_empty() {
        panic!("metadata corpus selection executed zero cases");
    }
    for case in cases {
        assert_eq!(
            case.family, "save.world-metadata",
            "case {} is not a save.world-metadata row",
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
            "unregistered metadata route {}/{}/{} for case {}",
            case.family,
            case.version,
            case.operation,
            case.id
        );
        execute_metadata_case(case).unwrap_or_else(|err| {
            panic!("case {} failed: {err}", case.id);
        });
    }
}

pub fn execute_metadata_case(case: &FrozenCase) -> Result<(), String> {
    let args = parse_metadata_arguments(case)?;
    match case.operation.as_str() {
        "decode" => execute_metadata_decode(case, &args),
        "encode" => execute_metadata_encode(case, &args),
        other => Err(format!("unsupported metadata operation {other}")),
    }
}

fn execute_metadata_decode(case: &FrozenCase, _args: &MetadataArguments) -> Result<(), String> {
    let schema = metadata_schema_version(&case.input)?;
    let want_version: u32 = case
        .version
        .parse()
        .map_err(|_| format!("case {} has invalid version", case.id))?;
    match decode_world_metadata(&case.input) {
        Ok(metadata) => {
            assert_ok_outcome(case)?;
            if schema != want_version {
                return Err(format!(
                    "case {} accepted input schema {schema}, want {want_version}",
                    case.id
                ));
            }
            expected_value_digest(case, &metadata_value(&metadata))
        }
        Err(err) => {
            let category = storage_error_category(&err)
                .ok_or_else(|| format!("unclassified rejection: {err}"))?;
            assert_error_category(case, category)
        }
    }
}

fn execute_metadata_encode(case: &FrozenCase, args: &MetadataArguments) -> Result<(), String> {
    let metadata = decode_world_metadata(&case.input).map_err(|err| err.to_string())?;
    let digest = value_sha256(&metadata_value(&metadata));
    let required = world_metadata_encoded_len(&metadata).map_err(|err| err.to_string())?;
    let mut buf = vec![0u8; required];
    if let Some(capacity) = args.capacity {
        if capacity as usize < required {
            assert_error_category(case, "output_too_small")?;
            return Ok(());
        }
        if capacity as usize > buf.len() {
            buf.resize(capacity as usize, 0);
        }
    }
    match encode_world_metadata_into(&metadata, &mut buf) {
        Ok(written) => {
            assert_ok_outcome(case)?;
            let encoded = &buf[..written];
            let round = decode_world_metadata(encoded).map_err(|err| err.to_string())?;
            if round != metadata {
                return Err(format!("case {} encode round-trip drift", case.id));
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
        Err(err) => {
            let category = storage_error_category(&err)
                .ok_or_else(|| format!("unclassified encode rejection: {err}"))?;
            assert_error_category(case, category)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mornlea_storage::{METADATA_CURRENT_VERSION, encode_world_metadata};

    fn spawn_anchor() -> MetadataChunkPos {
        MetadataChunkPos { x: 5, z: -6 }
    }

    fn boundary_metadata() -> Metadata {
        Metadata {
            format_version: METADATA_CURRENT_VERSION,
            seed: -42,
            spawn_dimension: -3,
            spawn_anchor: MetadataChunkPos { x: 7, z: -11 },
            world_time_ticks: 0x0102_0304_0506_0708,
            day_phase_offset: u64::MAX,
            weather_kind: 7,
            weather_ticks_remaining: 5000,
            depths_spawn_anchor: MetadataChunkPos { x: -5, z: 9 },
            depths_seed_salt: 0x0123_4567_89AB_CDEF,
            difficulty: 0,
        }
    }

    fn fixture_decode_case(id: &str, version: &str, input: Vec<u8>, digest: String) -> FrozenCase {
        FrozenCase {
            id: id.to_string(),
            family: "save.world-metadata".to_string(),
            version: version.to_string(),
            consumer: crate::runtime_corpus::CorpusConsumer::Storage,
            packet_key: None,
            operation: "decode".to_string(),
            arguments: JsonValue::Object(BTreeMap::new()),
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
    fn metadata_routes_recognize_registered_paths() {
        assert!(route_is_registered(&FrozenCase {
            id: "save.world-metadata/6/encode/v6-boundary".to_string(),
            family: "save.world-metadata".to_string(),
            version: "6".to_string(),
            consumer: crate::runtime_corpus::CorpusConsumer::Storage,
            packet_key: None,
            operation: "encode".to_string(),
            arguments: JsonValue::Object(BTreeMap::new()),
            input_format: InputFormat::Binary,
            input: vec![],
            input_json: None,
            normalized: JsonValue::Object(BTreeMap::new()),
            encoded: None,
            category: "save".to_string(),
        }));
    }

    #[test]
    fn metadata_decode_fixture_executes_local_v6_weather_255() {
        let mut metadata = boundary_metadata();
        metadata.weather_kind = 255;
        let input = encode_world_metadata(&metadata).expect("encode v6 weather 255");
        let digest = value_sha256(&metadata_value(&metadata));
        let case = fixture_decode_case(
            "save.world-metadata/6/decode/v6-weather-255",
            "6",
            input,
            digest,
        );
        execute_metadata_case(&case).expect("local v6 weather 255 decode");
    }

    #[test]
    fn metadata_anchor_digest_mutation_fails_comparison() {
        let metadata = boundary_metadata();
        let input = encode_world_metadata(&metadata).expect("encode boundary metadata");
        let mut stale = metadata.clone();
        stale.depths_spawn_anchor.x += 1;
        let case = fixture_decode_case(
            "save.world-metadata/6/decode/v6-boundary",
            "6",
            input,
            value_sha256(&metadata_value(&stale)),
        );
        let err = execute_metadata_case(&case).expect_err("stale anchor digest must fail");
        assert!(
            err.contains("value digest mismatch"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn metadata_salt_digest_mutation_fails_comparison() {
        let metadata = boundary_metadata();
        let input = encode_world_metadata(&metadata).expect("encode boundary metadata");
        let mut stale = metadata.clone();
        stale.depths_seed_salt += 1;
        let case = fixture_decode_case(
            "save.world-metadata/6/decode/v6-boundary",
            "6",
            input,
            value_sha256(&metadata_value(&stale)),
        );
        let err = execute_metadata_case(&case).expect_err("stale salt digest must fail");
        assert!(
            err.contains("value digest mismatch"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn metadata_weather_digest_mutation_fails_comparison() {
        let metadata = boundary_metadata();
        let input = encode_world_metadata(&metadata).expect("encode boundary metadata");
        let mut stale = metadata;
        stale.weather_kind = 9;
        let case = fixture_decode_case(
            "save.world-metadata/6/decode/v6-boundary",
            "6",
            input,
            value_sha256(&metadata_value(&stale)),
        );
        let err = execute_metadata_case(&case).expect_err("stale weather digest must fail");
        assert!(
            err.contains("value digest mismatch"),
            "unexpected error: {err}"
        );
    }
}
