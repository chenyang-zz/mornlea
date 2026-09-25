//! Executable `save.chunk` legacy decode routes for `mornlea_storage`.
//!
//! Node 4.3a seeds v1..v4 migration evidence. The shared dispatcher in
//! `storage_corpus.rs` delegates here once the controller registers the
//! reviewed routes against integrated manifest assets.

use super::value_digest::{Value, value_sha256};
use crate::runtime_corpus::{FrozenCase, InputFormat};
use mornlea_storage::{
    ChestSlot, ChunkKey, ChunkSave, ContainerSnapshot, DecodedChunk, DropSlot, FurnaceSlot, ItemStack,
    StorageError, decode_chunk, decode_chunk_envelope, encode_chunk,
};
const CHUNK_CURRENT_SCHEMA: u32 = 9;
use serde_json::Value as JsonValue;
use std::collections::BTreeMap;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub struct ChunkRoute {
    pub family: &'static str,
    pub version: &'static str,
    pub operation: &'static str,
}

/// Routes the chunk module executes once reviewed assets are integrated.
pub const CHUNK_REGISTERED_ROUTES: &[ChunkRoute] = &[
    ChunkRoute {
        family: "save.chunk",
        version: "1",
        operation: "decode",
    },
    ChunkRoute {
        family: "save.chunk",
        version: "2",
        operation: "decode",
    },
    ChunkRoute {
        family: "save.chunk",
        version: "3",
        operation: "decode",
    },
    ChunkRoute {
        family: "save.chunk",
        version: "4",
        operation: "decode",
    },
    ChunkRoute {
        family: "save.chunk",
        version: "5",
        operation: "decode",
    },
    ChunkRoute {
        family: "save.chunk",
        version: "6",
        operation: "decode",
    },
    ChunkRoute {
        family: "save.chunk",
        version: "7",
        operation: "decode",
    },
    ChunkRoute {
        family: "save.chunk",
        version: "8",
        operation: "decode",
    },
    ChunkRoute {
        family: "save.chunk",
        version: "9",
        operation: "decode",
    },
    ChunkRoute {
        family: "save.chunk",
        version: "9",
        operation: "encode",
    },
];

pub fn chunk_legacy_early_case(id: &str) -> bool {
    id.starts_with("save.chunk/1/decode/")
        || id.starts_with("save.chunk/2/decode/")
        || id.starts_with("save.chunk/3/decode/")
        || id.starts_with("save.chunk/4/decode/")
}

pub fn chunk_legacy_late_case(id: &str) -> bool {
    id.starts_with("save.chunk/5/decode/")
        || id.starts_with("save.chunk/6/decode/")
        || id.starts_with("save.chunk/7/decode/")
        || id.starts_with("save.chunk/8/decode/")
        || id.starts_with("save.chunk/9/decode/v9-fixture")
        || id.starts_with("save.chunk/9/decode/truncated-payload")
}

pub fn chunk_current_case(id: &str) -> bool {
    id.starts_with("save.chunk/9/decode/v9-fluid-fixture")
        || id.starts_with("save.chunk/9/decode/v9-chest-registry")
        || id.starts_with("save.chunk/9/encode/")
}

fn route_is_registered(case: &FrozenCase) -> bool {
    CHUNK_REGISTERED_ROUTES.iter().any(|route| {
        route.family == case.family
            && route.version == case.version
            && route.operation == case.operation
    })
}

#[derive(Debug)]
struct ChunkArguments {
    key: ChunkKey,
    revision: u64,
}

fn parse_chunk_arguments(case: &FrozenCase) -> Result<ChunkArguments, String> {
    let obj = case
        .arguments
        .as_object()
        .ok_or_else(|| format!("case {} arguments must be an object", case.id))?;
    let dimension = read_i32_json(obj.get("dimension").ok_or_else(|| {
        format!("case {} missing dimension", case.id)
    })?)?;
    let x = read_i32_json(obj.get("x").ok_or_else(|| format!("case {} missing x", case.id))?)?;
    let z = read_i32_json(obj.get("z").ok_or_else(|| format!("case {} missing z", case.id))?)?;
    let revision_text = obj
        .get("revision")
        .and_then(JsonValue::as_str)
        .ok_or_else(|| format!("case {} missing revision", case.id))?;
    let revision = revision_text
        .parse::<u64>()
        .map_err(|_| format!("case {} revision must be an unsigned decimal string", case.id))?;
    if revision == 0 {
        return Err(format!("case {} revision must be nonzero", case.id));
    }
    Ok(ChunkArguments {
        key: ChunkKey {
            dimension,
            x,
            z,
        },
        revision,
    })
}

fn read_i32_json(value: &JsonValue) -> Result<i32, String> {
    value
        .as_i64()
        .and_then(|number| i32::try_from(number).ok())
        .ok_or_else(|| "signed 32-bit field out of range".to_string())
}

fn chunk_schema_version(input: &[u8]) -> Result<u32, String> {
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

fn item_stack_value(stack: &ItemStack) -> Value {
    let mut fields = BTreeMap::new();
    fields.insert("count".to_string(), Value::Unsigned(stack.count as u64));
    fields.insert(
        "durability".to_string(),
        Value::Unsigned(stack.durability as u64),
    );
    fields.insert("item".to_string(), Value::Unsigned(stack.item as u64));
    Value::Object(fields)
}

fn section_value(section: &ContainerSnapshot) -> Value {
    let palette = section
        .palette
        .iter()
        .map(|id| Value::Unsigned(*id as u64))
        .collect::<Vec<_>>();
    let packed = section
        .packed
        .iter()
        .map(|word| Value::Unsigned(*word))
        .collect::<Vec<_>>();
    let mut fields = BTreeMap::new();
    fields.insert("kind".to_string(), Value::Unsigned(section.kind as u64));
    fields.insert("bits".to_string(), Value::Unsigned(section.bits as u64));
    fields.insert("single".to_string(), Value::Unsigned(section.single as u64));
    fields.insert("palette".to_string(), Value::Array(palette));
    fields.insert("packed".to_string(), Value::Array(packed));
    Value::Object(fields)
}

fn drop_value(drop: &DropSlot) -> Value {
    let mut fields = BTreeMap::new();
    fields.insert("generation".to_string(), Value::Unsigned(drop.generation as u64));
    fields.insert("active".to_string(), Value::Bool(drop.active));
    fields.insert("stack".to_string(), item_stack_value(&drop.stack));
    fields.insert(
        "block_index".to_string(),
        Value::Unsigned(drop.block_index as u64),
    );
    fields.insert("age_ticks".to_string(), Value::Unsigned(drop.age_ticks as u64));
    fields.insert(
        "pickup_delay_ticks".to_string(),
        Value::Unsigned(drop.pickup_delay_ticks as u64),
    );
    Value::Object(fields)
}

fn furnace_value(slot: &FurnaceSlot) -> Value {
    let mut fields = BTreeMap::new();
    fields.insert("generation".to_string(), Value::Unsigned(slot.generation as u64));
    fields.insert("active".to_string(), Value::Bool(slot.active));
    fields.insert(
        "block_index".to_string(),
        Value::Unsigned(slot.block_index as u64),
    );
    fields.insert("input".to_string(), item_stack_value(&slot.input));
    fields.insert("fuel".to_string(), item_stack_value(&slot.fuel));
    fields.insert("output".to_string(), item_stack_value(&slot.output));
    fields.insert(
        "progress_ticks".to_string(),
        Value::Unsigned(slot.progress_ticks as u64),
    );
    fields.insert("burn_ticks".to_string(), Value::Unsigned(slot.burn_ticks as u64));
    Value::Object(fields)
}

fn chest_value(slot: &ChestSlot) -> Value {
    let items = slot
        .items
        .iter()
        .map(item_stack_value)
        .collect::<Vec<_>>();
    let mut fields = BTreeMap::new();
    fields.insert("generation".to_string(), Value::Unsigned(slot.generation as u64));
    fields.insert("active".to_string(), Value::Bool(slot.active));
    fields.insert(
        "block_index".to_string(),
        Value::Unsigned(slot.block_index as u64),
    );
    fields.insert("items".to_string(), Value::Array(items));
    Value::Object(fields)
}

fn chunk_key_value(key: &ChunkKey) -> Value {
    let mut fields = BTreeMap::new();
    fields.insert("dimension".to_string(), Value::Signed(key.dimension as i64));
    fields.insert("x".to_string(), Value::Signed(key.x as i64));
    fields.insert("z".to_string(), Value::Signed(key.z as i64));
    Value::Object(fields)
}

fn chunk_body_value(chunk: &mornlea_storage::Chunk) -> Value {
    let sections = chunk
        .sections
        .iter()
        .map(section_value)
        .collect::<Vec<_>>();
    let drops = chunk.drops.iter().map(drop_value).collect::<Vec<_>>();
    let furnaces = chunk.furnaces.iter().map(furnace_value).collect::<Vec<_>>();
    let chests = chunk.chests.iter().map(chest_value).collect::<Vec<_>>();
    let mut fields = BTreeMap::new();
    fields.insert("sections".to_string(), Value::Array(sections));
    fields.insert("drops".to_string(), Value::Array(drops));
    fields.insert("furnaces".to_string(), Value::Array(furnaces));
    fields.insert("chests".to_string(), Value::Array(chests));
    Value::Object(fields)
}

pub fn decoded_chunk_value(decoded: &DecodedChunk) -> Value {
    let mut fields = BTreeMap::new();
    fields.insert("key".to_string(), chunk_key_value(&decoded.key));
    fields.insert("revision".to_string(), Value::Unsigned(decoded.revision));
    fields.insert(
        "schema".to_string(),
        Value::Unsigned(CHUNK_CURRENT_SCHEMA as u64),
    );
    fields.insert("migrated".to_string(), Value::Bool(decoded.migrated));
    fields.insert("chunk".to_string(), chunk_body_value(&decoded.chunk));
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

#[allow(dead_code)]
pub fn execute_chunk_cases(cases: &[FrozenCase]) {
    if cases.is_empty() {
        panic!("chunk corpus selection executed zero cases");
    }
    for case in cases {
        assert_eq!(
            case.family, "save.chunk",
            "case {} is not a save.chunk row",
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
            "unregistered chunk route {}/{}/{} for case {}",
            case.family,
            case.version,
            case.operation,
            case.id
        );
        execute_chunk_case(case).unwrap_or_else(|err| {
            panic!("case {} failed: {err}", case.id);
        });
    }
}

pub fn execute_chunk_case(case: &FrozenCase) -> Result<(), String> {
    let args = parse_chunk_arguments(case)?;
    match case.operation.as_str() {
        "decode" => execute_chunk_decode(case, &args),
        "encode" => execute_chunk_encode(case, &args),
        other => Err(format!("unsupported chunk operation {other}")),
    }
}

fn assert_logical_length(case: &FrozenCase, length: usize) -> Result<(), String> {
    let want = case
        .normalized
        .get("logical_length")
        .and_then(JsonValue::as_u64)
        .ok_or_else(|| format!("case {} missing logical_length", case.id))?;
    if length as u64 != want {
        return Err(format!(
            "case {} logical_length {}, want {}",
            case.id,
            length,
            want
        ));
    }
    Ok(())
}

fn execute_chunk_encode(case: &FrozenCase, args: &ChunkArguments) -> Result<(), String> {
    let decoded = decode_chunk(args.key, args.revision, &case.input)
        .map_err(|err| err.to_string())?;
    let digest_value = decoded_chunk_value(&decoded);
    let save = ChunkSave {
        key: args.key,
        revision: args.revision,
        chunk: decoded.chunk.clone(),
    };
    let frame = encode_chunk(&save).map_err(|err| err.to_string())?;
    if case.normalized.get("kind").and_then(JsonValue::as_str) == Some("ok") {
        expected_value_digest(case, &digest_value)?;
        let envelope = decode_chunk_envelope(&frame).map_err(|err| err.to_string())?;
        assert_logical_length(case, envelope.bytes.len())?;
        let encoded_ref = case
            .encoded
            .as_ref()
            .ok_or_else(|| format!("case {} missing encoded asset", case.id))?;
        if encoded_ref.as_slice() != envelope.bytes.as_slice() {
            return Err(format!("case {} logical bytes mismatch", case.id));
        }
        let round = decode_chunk(save.key, save.revision, &frame).map_err(|err| err.to_string())?;
        if round.migrated {
            return Err(format!("case {} encoded output marked migrated", case.id));
        }
    }
    Ok(())
}

fn execute_chunk_decode(case: &FrozenCase, args: &ChunkArguments) -> Result<(), String> {
    let schema = chunk_schema_version(&case.input)?;
    let want_version: u32 = case
        .version
        .parse()
        .map_err(|_| format!("case {} has invalid version", case.id))?;
    match decode_chunk(args.key, args.revision, &case.input) {
        Ok(decoded) => {
            assert_ok_outcome(case)?;
            if schema != want_version {
                return Err(format!(
                    "case {} accepted input schema {schema}, want {want_version}",
                    case.id
                ));
            }
            expected_value_digest(case, &decoded_chunk_value(&decoded))
        }
        Err(err) => {
            let category = storage_error_category(&err)
                .ok_or_else(|| format!("unclassified rejection: {err}"))?;
            assert_error_category(case, category)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;

    fn fixture_chunk_key() -> ChunkKey {
        ChunkKey {
            dimension: 0,
            x: -3,
            z: 7,
        }
    }

    fn fixture_revision() -> u64 {
        19
    }

    fn chunk_arguments_json() -> serde_json::Value {
        serde_json::json!({
            "dimension": 0,
            "x": -3,
            "z": 7,
            "revision": "19"
        })
    }

    fn read_go_fixture(relative: &str) -> Vec<u8> {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../../../packages/")
            .join(relative);
        fs::read(&path).unwrap_or_else(|err| panic!("read {}: {err}", path.display()))
    }

    fn chunk_truncated_wire(wire: &[u8], drop_tail: usize) -> Vec<u8> {
        if drop_tail == 0 || drop_tail >= wire.len() {
            return wire.to_vec();
        }
        wire[..wire.len() - drop_tail].to_vec()
    }

    fn chunk_corrupt_compressed_wire(wire: &[u8]) -> Vec<u8> {
        let mut out = wire.to_vec();
        if out.len() > 44 {
            out[44] ^= 0xff;
        }
        out
    }

    fn chunk_wire_with_schema(wire: &[u8], schema: u32) -> Vec<u8> {
        let mut out = wire.to_vec();
        if out.len() >= 12 {
            out[8..12].copy_from_slice(&schema.to_le_bytes());
        }
        out
    }

    fn legacy_decode_ok_case(id: &str, version: &str, input: Vec<u8>) -> FrozenCase {
        let decoded = decode_chunk(fixture_chunk_key(), fixture_revision(), &input)
            .unwrap_or_else(|err| panic!("case {id} decode fixture: {err}"));
        FrozenCase {
            id: id.to_string(),
            family: "save.chunk".to_string(),
            version: version.to_string(),
            consumer: crate::runtime_corpus::CorpusConsumer::Storage,
            packet_key: None,
            operation: "decode".to_string(),
            arguments: chunk_arguments_json(),
            input_format: InputFormat::Binary,
            input: input.clone(),
            input_json: None,
            normalized: serde_json::json!({
                "kind": "ok",
                "category": "save",
                "value_sha256": value_sha256(&decoded_chunk_value(&decoded))
            }),
            encoded: None,
            category: "save".to_string(),
        }
    }

    fn legacy_decode_error_case(
        id: &str,
        version: &str,
        input: Vec<u8>,
        category: &str,
    ) -> FrozenCase {
        FrozenCase {
            id: id.to_string(),
            family: "save.chunk".to_string(),
            version: version.to_string(),
            consumer: crate::runtime_corpus::CorpusConsumer::Storage,
            packet_key: None,
            operation: "decode".to_string(),
            arguments: chunk_arguments_json(),
            input_format: InputFormat::Binary,
            input,
            input_json: None,
            normalized: serde_json::json!({
                "kind": "error",
                "category": category
            }),
            encoded: None,
            category: category.to_string(),
        }
    }

    fn chunk_legacy_early_fixture_cases() -> Vec<FrozenCase> {
        let v1 = read_go_fixture("server/storage/chunk/testdata/chunk-v1.bin");
        let v2 = read_go_fixture("server/storage/chunk/testdata/chunk-v2.bin");
        let v3 = read_go_fixture("server/storage/chunk/testdata/chunk-v3.bin");
        let v4 = read_go_fixture("server/storage/chunk/testdata/chunk-v4.bin");
        vec![
            legacy_decode_ok_case("save.chunk/1/decode/v1-fixture", "1", v1.clone()),
            legacy_decode_error_case(
                "save.chunk/1/decode/truncated-payload",
                "1",
                chunk_truncated_wire(&v1, 1),
                "corrupt",
            ),
            legacy_decode_ok_case("save.chunk/2/decode/v2-fixture", "2", v2.clone()),
            legacy_decode_error_case(
                "save.chunk/2/decode/corrupt-crc",
                "2",
                chunk_corrupt_compressed_wire(&v2),
                "corrupt",
            ),
            legacy_decode_ok_case("save.chunk/3/decode/v3-fixture", "3", v3.clone()),
            legacy_decode_error_case(
                "save.chunk/3/decode/invalid-version-zero",
                "3",
                chunk_wire_with_schema(&v3, 0),
                "corrupt",
            ),
            legacy_decode_ok_case("save.chunk/4/decode/v4-fixture", "4", v4.clone()),
            legacy_decode_error_case(
                "save.chunk/4/decode/invalid-version-future",
                "4",
                chunk_wire_with_schema(&v4, 10),
                "future_version",
            ),
        ]
    }

    #[test]
    fn chunk_legacy_early_routes_recognize_registered_paths() {
        assert!(route_is_registered(&FrozenCase {
            id: "save.chunk/2/decode/v2-fixture".to_string(),
            family: "save.chunk".to_string(),
            version: "2".to_string(),
            consumer: crate::runtime_corpus::CorpusConsumer::Storage,
            packet_key: None,
            operation: "decode".to_string(),
            arguments: chunk_arguments_json(),
            input_format: InputFormat::Binary,
            input: vec![],
            input_json: None,
            normalized: JsonValue::Object(serde_json::Map::new()),
            encoded: None,
            category: "save".to_string(),
        }));
    }

    #[test]
    fn chunk_legacy_early_fixture_cases_execute_locally() {
        let cases = chunk_legacy_early_fixture_cases();
        assert_eq!(cases.len(), 8);
        for case in &cases {
            execute_chunk_case(case).expect("local early chunk fixture");
        }
    }

    #[test]
    fn chunk_legacy_early_section_digest_mutation_fails_comparison() {
        let golden = read_go_fixture("server/storage/chunk/testdata/chunk-v1.bin");
        let decoded =
            decode_chunk(fixture_chunk_key(), fixture_revision(), &golden).expect("decode v1");
        let mut stale = decoded.clone();
        if let Some(section) = stale.chunk.sections.first_mut() {
            if !section.palette.is_empty() {
                section.palette[0] = section.palette[0].saturating_add(1);
            } else {
                section.single = section.single.saturating_add(1);
            }
        }
        let stale_digest = value_sha256(&decoded_chunk_value(&stale));
        let ok_digest = value_sha256(&decoded_chunk_value(&decoded));
        assert_ne!(stale_digest, ok_digest);
        let mut case = legacy_decode_ok_case("save.chunk/1/decode/v1-fixture", "1", golden);
        case.normalized = serde_json::json!({
            "kind": "ok",
            "category": "save",
            "value_sha256": stale_digest
        });
        let err = execute_chunk_case(&case).expect_err("stale section digest must fail");
        assert!(
            err.contains("value digest mismatch"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn chunk_legacy_early_drop_digest_mutation_fails_comparison() {
        let golden = read_go_fixture("server/storage/chunk/testdata/chunk-v2.bin");
        let decoded =
            decode_chunk(fixture_chunk_key(), fixture_revision(), &golden).expect("decode v2");
        let mut stale = decoded.clone();
        let slot = stale
            .chunk
            .drops
            .iter()
            .position(|drop| drop.active)
            .expect("v2 fixture must carry an active drop");
        stale.chunk.drops[slot].age_ticks += 1;
        let stale_digest = value_sha256(&decoded_chunk_value(&stale));
        let mut case = legacy_decode_ok_case("save.chunk/2/decode/v2-fixture", "2", golden);
        case.normalized = serde_json::json!({
            "kind": "ok",
            "category": "save",
            "value_sha256": stale_digest
        });
        let err = execute_chunk_case(&case).expect_err("stale drop digest must fail");
        assert!(
            err.contains("value digest mismatch"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn chunk_legacy_early_case_ids_match_go_producer() {
        for id in [
            "save.chunk/1/decode/v1-fixture",
            "save.chunk/1/decode/truncated-payload",
            "save.chunk/2/decode/v2-fixture",
            "save.chunk/2/decode/corrupt-crc",
            "save.chunk/3/decode/v3-fixture",
            "save.chunk/3/decode/invalid-version-zero",
            "save.chunk/4/decode/v4-fixture",
            "save.chunk/4/decode/invalid-version-future",
        ] {
            assert!(chunk_legacy_early_case(id), "missing early id {id}");
        }
    }

    fn chunk_legacy_late_fixture_cases() -> Vec<FrozenCase> {
        let v5 = read_go_fixture("server/storage/chunk/testdata/chunk-v5.bin");
        let v6 = read_go_fixture("server/storage/chunk/testdata/chunk-v6.bin");
        let v7 = read_go_fixture("server/storage/chunk/testdata/chunk-v7.bin");
        let v8 = read_go_fixture("server/storage/chunk/testdata/chunk-v8.bin");
        let v9 = read_go_fixture("server/storage/chunk/testdata/chunk-v9.bin");
        vec![
            legacy_decode_ok_case("save.chunk/5/decode/v5-fixture", "5", v5.clone()),
            legacy_decode_error_case(
                "save.chunk/5/decode/corrupt-crc",
                "5",
                chunk_corrupt_compressed_wire(&v5),
                "corrupt",
            ),
            legacy_decode_ok_case("save.chunk/6/decode/v6-fixture", "6", v6.clone()),
            legacy_decode_error_case(
                "save.chunk/6/decode/truncated-payload",
                "6",
                chunk_truncated_wire(&v6, 1),
                "corrupt",
            ),
            legacy_decode_ok_case("save.chunk/7/decode/v7-fixture", "7", v7.clone()),
            legacy_decode_error_case(
                "save.chunk/7/decode/invalid-version-zero",
                "7",
                chunk_wire_with_schema(&v7, 0),
                "corrupt",
            ),
            legacy_decode_ok_case("save.chunk/8/decode/v8-fixture", "8", v8.clone()),
            legacy_decode_error_case(
                "save.chunk/8/decode/invalid-version-future",
                "8",
                chunk_wire_with_schema(&v8, 10),
                "future_version",
            ),
            legacy_decode_ok_case("save.chunk/9/decode/v9-fixture", "9", v9.clone()),
            legacy_decode_error_case(
                "save.chunk/9/decode/truncated-payload",
                "9",
                chunk_truncated_wire(&v9, 1),
                "corrupt",
            ),
        ]
    }

    #[test]
    fn chunk_legacy_late_routes_recognize_registered_paths() {
        assert!(route_is_registered(&FrozenCase {
            id: "save.chunk/7/decode/v7-fixture".to_string(),
            family: "save.chunk".to_string(),
            version: "7".to_string(),
            consumer: crate::runtime_corpus::CorpusConsumer::Storage,
            packet_key: None,
            operation: "decode".to_string(),
            arguments: chunk_arguments_json(),
            input_format: InputFormat::Binary,
            input: vec![],
            input_json: None,
            normalized: JsonValue::Object(serde_json::Map::new()),
            encoded: None,
            category: "save".to_string(),
        }));
    }

    #[test]
    fn chunk_legacy_late_fixture_cases_execute_locally() {
        let cases = chunk_legacy_late_fixture_cases();
        assert_eq!(cases.len(), 10);
        for case in &cases {
            execute_chunk_case(case).expect("local late chunk fixture");
        }
    }

    #[test]
    fn chunk_legacy_late_case_ids_match_go_producer() {
        for id in [
            "save.chunk/5/decode/v5-fixture",
            "save.chunk/5/decode/corrupt-crc",
            "save.chunk/6/decode/v6-fixture",
            "save.chunk/6/decode/truncated-payload",
            "save.chunk/7/decode/v7-fixture",
            "save.chunk/7/decode/invalid-version-zero",
            "save.chunk/8/decode/v8-fixture",
            "save.chunk/8/decode/invalid-version-future",
            "save.chunk/9/decode/v9-fixture",
            "save.chunk/9/decode/truncated-payload",
        ] {
            assert!(chunk_legacy_late_case(id), "missing late id {id}");
        }
    }

    #[test]
    fn chunk_current_routes_recognize_registered_paths() {
        assert!(chunk_current_case("save.chunk/9/encode/v9-fixture-exact"));
        assert!(route_is_registered(&FrozenCase {
            id: "save.chunk/9/encode/v9-fixture-exact".to_string(),
            family: "save.chunk".to_string(),
            version: "9".to_string(),
            consumer: crate::runtime_corpus::CorpusConsumer::Storage,
            packet_key: None,
            operation: "encode".to_string(),
            arguments: chunk_arguments_json(),
            input_format: InputFormat::Binary,
            input: vec![],
            input_json: None,
            normalized: JsonValue::Object(serde_json::Map::new()),
            encoded: None,
            category: "save".to_string(),
        }));
    }
}
