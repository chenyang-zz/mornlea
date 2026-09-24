//! Executable `save.player` corpus routes for `mornlea_storage`.
//!
//! Node 2.2a seeds current-schema decode and encode evidence. The shared
//! dispatcher in `storage_corpus.rs` delegates here once the controller
//! registers the reviewed routes against integrated manifest assets.

use super::value_digest::{Value, value_sha256};
use mornlea_storage::{
    Inventory, ItemStack, PlayerId, PlayerLocation, PlayerSave, StoredPlayer, StorageError,
    decode_player, encode_player_into, player_encoded_len,
};
use crate::runtime_corpus::{FrozenCase, InputFormat};
use serde_json::Value as JsonValue;
use std::collections::BTreeMap;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub struct PlayerRoute {
    pub family: &'static str,
    pub version: &'static str,
    pub operation: &'static str,
}

/// Routes the player module executes once reviewed assets are integrated.
pub const PLAYER_REGISTERED_ROUTES: &[PlayerRoute] = &[
    PlayerRoute {
        family: "save.player",
        version: "9",
        operation: "decode",
    },
    PlayerRoute {
        family: "save.player",
        version: "9",
        operation: "encode",
    },
    PlayerRoute {
        family: "save.player",
        version: "1",
        operation: "decode",
    },
    PlayerRoute {
        family: "save.player",
        version: "2",
        operation: "decode",
    },
    PlayerRoute {
        family: "save.player",
        version: "3",
        operation: "decode",
    },
    PlayerRoute {
        family: "save.player",
        version: "4",
        operation: "decode",
    },
    PlayerRoute {
        family: "save.player",
        version: "5",
        operation: "decode",
    },
    PlayerRoute {
        family: "save.player",
        version: "6",
        operation: "decode",
    },
    PlayerRoute {
        family: "save.player",
        version: "7",
        operation: "decode",
    },
    PlayerRoute {
        family: "save.player",
        version: "8",
        operation: "decode",
    },
];

pub fn player_legacy_early_case(id: &str) -> bool {
    id.starts_with("save.player/1/decode/")
        || id.starts_with("save.player/2/decode/")
        || id.starts_with("save.player/3/decode/")
        || id.starts_with("save.player/4/decode/")
        || id == "save.player/9/encode/v4-fixture-reencode"
}

pub fn player_legacy_late_case(id: &str) -> bool {
    id.starts_with("save.player/5/decode/")
        || id.starts_with("save.player/6/decode/")
        || id.starts_with("save.player/7/decode/")
        || id.starts_with("save.player/8/decode/")
        || id == "save.player/9/encode/v8-fixture-reencode"
}

fn route_is_registered(case: &FrozenCase) -> bool {
    PLAYER_REGISTERED_ROUTES.iter().any(|route| {
        route.family == case.family
            && route.version == case.version
            && route.operation == case.operation
    })
}

#[derive(Debug)]
struct PlayerArguments {
    player_id: PlayerId,
    capacity: Option<u32>,
}

fn parse_player_arguments(case: &FrozenCase) -> Result<PlayerArguments, String> {
    let obj = case
        .arguments
        .as_object()
        .ok_or_else(|| format!("case {} arguments must be an object", case.id))?;
    let id_text = obj
        .get("requested_player_id")
        .and_then(JsonValue::as_str)
        .ok_or_else(|| format!("case {} missing requested_player_id", case.id))?;
    if id_text.len() != 32
        || !id_text
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(format!(
            "case {} requested_player_id must be 32 lowercase hex digits",
            case.id
        ));
    }
    let mut id_bytes = [0u8; 16];
    for (index, chunk) in id_text.as_bytes().chunks(2).enumerate() {
        if index >= 16 {
            break;
        }
        let text = std::str::from_utf8(chunk).map_err(|_| "invalid hex")?;
        id_bytes[index] = u8::from_str_radix(text, 16).map_err(|_| "invalid hex")?;
    }
    let capacity = match obj.get("capacity") {
        None => None,
        Some(value) => Some(read_u32_json(value)?),
    };
    Ok(PlayerArguments {
        player_id: PlayerId::from_bytes(id_bytes),
        capacity,
    })
}

fn read_u32_json(value: &JsonValue) -> Result<u32, String> {
    value
        .as_u64()
        .and_then(|number| u32::try_from(number).ok())
        .ok_or_else(|| "capacity must be an unsigned integer".to_string())
}

fn player_schema_version(input: &[u8]) -> Result<u32, String> {
    if input.len() < 12 {
        return Err("input shorter than schema header".to_string());
    }
    Ok(u32::from_le_bytes(input[8..12].try_into().expect("slice length")))
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

fn location_value(location: &PlayerLocation) -> Value {
    let mut fields = BTreeMap::new();
    fields.insert("dimension".to_string(), Value::Signed(location.dimension as i64));
    fields.insert(
        "position".to_string(),
        Value::Array(
            location
                .position
                .iter()
                .map(|component| Value::F32(*component))
                .collect(),
        ),
    );
    Value::Object(fields)
}

fn inventory_value(inventory: &Inventory) -> Value {
    let hotbar_slots = inventory
        .hotbar
        .slots
        .iter()
        .map(item_stack_value)
        .collect::<Vec<_>>();
    let mut hotbar = BTreeMap::new();
    hotbar.insert(
        "selected".to_string(),
        Value::Unsigned(inventory.hotbar.selected as u64),
    );
    hotbar.insert("slots".to_string(), Value::Array(hotbar_slots));
    let backpack = inventory
        .backpack
        .iter()
        .map(item_stack_value)
        .collect::<Vec<_>>();
    let mut fields = BTreeMap::new();
    fields.insert("backpack".to_string(), Value::Array(backpack));
    fields.insert("hotbar".to_string(), Value::Object(hotbar));
    Value::Object(fields)
}

fn stored_player_value(stored: &StoredPlayer) -> Value {
    let safe = match &stored.safe {
        None => Value::Null,
        Some(location) => location_value(location),
    };
    let armor = stored.armor.iter().map(item_stack_value).collect::<Vec<_>>();
    let mut fields = BTreeMap::new();
    fields.insert("armor".to_string(), Value::Array(armor));
    fields.insert("current".to_string(), location_value(&stored.current));
    fields.insert("display_name".to_string(), Value::Utf8(stored.display_name.clone()));
    fields.insert(
        "exhaustion_milli".to_string(),
        Value::Unsigned(stored.exhaustion_milli as u64),
    );
    fields.insert("health".to_string(), Value::Unsigned(stored.health as u64));
    fields.insert("hunger".to_string(), Value::Unsigned(stored.hunger as u64));
    fields.insert("inventory".to_string(), inventory_value(&stored.inventory));
    fields.insert("needs_rewrite".to_string(), Value::Bool(stored.needs_rewrite));
    fields.insert("pitch".to_string(), Value::F32(stored.pitch));
    fields.insert(
        "player_id".to_string(),
        Value::Bytes(stored.player_id.to_bytes().to_vec()),
    );
    fields.insert(
        "respawn_dimension".to_string(),
        Value::Signed(stored.respawn_dimension as i64),
    );
    fields.insert(
        "respawn_position".to_string(),
        Value::Array(
            stored
                .respawn_position
                .iter()
                .map(|component| Value::F32(*component))
                .collect(),
        ),
    );
    fields.insert(
        "respawn_present".to_string(),
        Value::Bool(stored.respawn_present),
    );
    fields.insert("revision".to_string(), Value::Unsigned(stored.revision));
    fields.insert("safe".to_string(), safe);
    fields.insert(
        "saturation_milli".to_string(),
        Value::Unsigned(stored.saturation_milli as u64),
    );
    fields.insert("yaw".to_string(), Value::F32(stored.yaw));
    Value::Object(fields)
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

pub fn execute_player_cases(cases: &[FrozenCase]) {
    if cases.is_empty() {
        panic!("player corpus selection executed zero cases");
    }
    for case in cases {
        assert_eq!(
            case.family, "save.player",
            "case {} is not a save.player row",
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
            "unregistered player route {}/{}/{} for case {}",
            case.family, case.version, case.operation, case.id
        );
        execute_player_case(case).unwrap_or_else(|err| {
            panic!("case {} failed: {err}", case.id);
        });
    }
}

pub fn execute_player_case(case: &FrozenCase) -> Result<(), String> {
    let args = parse_player_arguments(case)?;
    match case.operation.as_str() {
        "decode" => execute_player_decode(case, &args),
        "encode" => execute_player_encode(case, &args),
        other => Err(format!("unsupported player operation {other}")),
    }
}

fn execute_player_decode(case: &FrozenCase, args: &PlayerArguments) -> Result<(), String> {
    let schema = player_schema_version(&case.input)?;
    let want_version: u32 = case
        .version
        .parse()
        .map_err(|_| format!("case {} has invalid version", case.id))?;
    if schema != want_version && schema != 0 && schema <= want_version {
        return Err(format!(
            "case {} input schema {schema}, want {want_version}",
            case.id
        ));
    }
    match decode_player(args.player_id, &case.input) {
        Ok(stored) => {
            assert_ok_outcome(case)?;
            if schema != want_version {
                return Err(format!(
                    "case {} accepted input schema {schema}, want {want_version}",
                    case.id
                ));
            }
            expected_value_digest(case, &stored_player_value(&stored))
        }
        Err(err) => {
            let category = storage_error_category(&err)
                .ok_or_else(|| format!("unclassified rejection: {err}"))?;
            assert_error_category(case, category)
        }
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

fn execute_player_encode(case: &FrozenCase, args: &PlayerArguments) -> Result<(), String> {
    let stored = decode_player(args.player_id, &case.input).map_err(|err| err.to_string())?;
    let digest_value = stored_player_value(&stored);
    let save = stored_to_save(&stored);
    let needed = player_encoded_len(&save).map_err(|err| err.to_string())?;
    let buf_len = args.capacity.map(|cap| cap as usize).unwrap_or(needed);
    let mut encoded = vec![0u8; buf_len];
    let written = match encode_player_into(&save, &mut encoded) {
        Err(StorageError::OutputTooSmall {
            needed: want_needed,
            available: want_available,
        }) => {
            assert_error_category(case, "output_too_small")?;
            assert_output_too_small_fields(case, want_needed, want_available)?;
            return Ok(());
        }
        Err(err) => return Err(err.to_string()),
        Ok(len) => len,
    };
    encoded.truncate(written);

    if case.normalized.get("kind").and_then(JsonValue::as_str) == Some("ok") {
        expected_value_digest(case, &digest_value)?;
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
        let round = decode_player(save.player_id, &encoded).map_err(|err| err.to_string())?;
        if round.needs_rewrite {
            return Err(format!("case {} encoded output needs rewrite", case.id));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime_corpus::{load_cases_for_consumer, CorpusConsumer};
    use std::fs;
    use std::path::PathBuf;

    fn read_go_fixture(relative: &str) -> Vec<u8> {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../../../packages/")
            .join(relative);
        fs::read(&path).unwrap_or_else(|err| panic!("read {}: {err}", path.display()))
    }

    fn fixture_player_id() -> PlayerId {
        PlayerId::from_bytes([
            0x00, 0x11, 0x22, 0x33, 0x44, 0x55, 0x46, 0x77, 0x88, 0x99, 0xaa, 0xbb, 0xcc, 0xdd,
            0xee, 0xff,
        ])
    }

    fn player_arguments_json() -> serde_json::Value {
        serde_json::json!({
            "requested_player_id": "00112233445546778899aabbccddeeff"
        })
    }

    fn player_truncated_wire(wire: &[u8], drop_tail: usize) -> Vec<u8> {
        if drop_tail == 0 || drop_tail >= wire.len() {
            return wire.to_vec();
        }
        wire[..wire.len() - drop_tail].to_vec()
    }

    fn player_corrupt_crc_wire(wire: &[u8]) -> Vec<u8> {
        let mut out = wire.to_vec();
        if out.len() > 40 {
            out[40] ^= 0xff;
        }
        out
    }

    fn player_wire_with_schema(wire: &[u8], schema: u32) -> Vec<u8> {
        let mut out = wire.to_vec();
        if out.len() >= 12 {
            out[8..12].copy_from_slice(&schema.to_le_bytes());
        }
        out
    }

    fn legacy_decode_ok_case(id: &str, version: &str, input: Vec<u8>) -> FrozenCase {
        let player_id = fixture_player_id();
        let stored = decode_player(player_id, &input).unwrap_or_else(|err| {
            panic!("case {id} decode fixture: {err}");
        });
        FrozenCase {
            id: id.to_string(),
            family: "save.player".to_string(),
            version: version.to_string(),
            consumer: CorpusConsumer::Storage,
            packet_key: None,
            operation: "decode".to_string(),
            arguments: player_arguments_json(),
            input_format: InputFormat::Binary,
            input: input.clone(),
            input_json: None,
            normalized: serde_json::json!({
                "kind": "ok",
                "category": "save",
                "value_sha256": value_sha256(&stored_player_value(&stored))
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
            family: "save.player".to_string(),
            version: version.to_string(),
            consumer: CorpusConsumer::Storage,
            packet_key: None,
            operation: "decode".to_string(),
            arguments: player_arguments_json(),
            input_format: InputFormat::Binary,
            input: input,
            input_json: None,
            normalized: serde_json::json!({
                "kind": "error",
                "category": category
            }),
            encoded: None,
            category: category.to_string(),
        }
    }

    fn legacy_encode_v8_reencode_case(v8_fixture: Vec<u8>) -> FrozenCase {
        let player_id = fixture_player_id();
        let stored = decode_player(player_id, &v8_fixture).expect("decode v8 fixture");
        assert!(
            stored.needs_rewrite,
            "historical v8 input must decode with needs_rewrite true"
        );
        let digest_value = stored_player_value(&stored);
        let save = stored_to_save(&stored);
        let needed = player_encoded_len(&save).expect("encoded len");
        let mut encoded = vec![0u8; needed];
        let written = encode_player_into(&save, &mut encoded).expect("encode v8 migration");
        encoded.truncate(written);
        FrozenCase {
            id: "save.player/9/encode/v8-fixture-reencode".to_string(),
            family: "save.player".to_string(),
            version: "9".to_string(),
            consumer: CorpusConsumer::Storage,
            packet_key: None,
            operation: "encode".to_string(),
            arguments: player_arguments_json(),
            input_format: InputFormat::Binary,
            input: v8_fixture,
            input_json: None,
            normalized: serde_json::json!({
                "kind": "ok",
                "category": "save",
                "value_sha256": value_sha256(&digest_value),
                "length": encoded.len()
            }),
            encoded: Some(encoded),
            category: "save".to_string(),
        }
    }

    fn legacy_encode_v4_reencode_case(v4_fixture: Vec<u8>) -> FrozenCase {
        let player_id = fixture_player_id();
        let stored = decode_player(player_id, &v4_fixture).expect("decode v4 fixture");
        assert!(
            stored.needs_rewrite,
            "historical v4 input must decode with needs_rewrite true"
        );
        let digest_value = stored_player_value(&stored);
        let save = stored_to_save(&stored);
        let needed = player_encoded_len(&save).expect("encoded len");
        let mut encoded = vec![0u8; needed];
        let written = encode_player_into(&save, &mut encoded).expect("encode v4 migration");
        encoded.truncate(written);
        FrozenCase {
            id: "save.player/9/encode/v4-fixture-reencode".to_string(),
            family: "save.player".to_string(),
            version: "9".to_string(),
            consumer: CorpusConsumer::Storage,
            packet_key: None,
            operation: "encode".to_string(),
            arguments: player_arguments_json(),
            input_format: InputFormat::Binary,
            input: v4_fixture,
            input_json: None,
            normalized: serde_json::json!({
                "kind": "ok",
                "category": "save",
                "value_sha256": value_sha256(&digest_value),
                "length": encoded.len()
            }),
            encoded: Some(encoded),
            category: "save".to_string(),
        }
    }

    fn player_legacy_late_fixture_cases() -> Vec<FrozenCase> {
        let v5 = read_go_fixture("server/storage/player/testdata/player-v5.bin");
        let v6 = read_go_fixture("server/storage/player/testdata/player-v6.bin");
        let v7 = read_go_fixture("server/storage/player/testdata/player-v7.bin");
        let v8 = read_go_fixture("server/storage/player/testdata/player-v8.bin");
        vec![
            legacy_decode_ok_case("save.player/5/decode/v5-fixture", "5", v5.clone()),
            legacy_decode_error_case(
                "save.player/5/decode/corrupt-crc",
                "5",
                player_corrupt_crc_wire(&v5),
                "corrupt",
            ),
            legacy_decode_ok_case("save.player/6/decode/v6-fixture", "6", v6.clone()),
            legacy_decode_error_case(
                "save.player/6/decode/truncated-payload",
                "6",
                player_truncated_wire(&v6, 1),
                "corrupt",
            ),
            legacy_decode_ok_case("save.player/7/decode/v7-fixture", "7", v7.clone()),
            legacy_decode_error_case(
                "save.player/7/decode/invalid-version-zero",
                "7",
                player_wire_with_schema(&v7, 0),
                "corrupt",
            ),
            legacy_decode_ok_case("save.player/8/decode/v8-fixture", "8", v8.clone()),
            legacy_decode_error_case(
                "save.player/8/decode/invalid-version-future",
                "8",
                player_wire_with_schema(&v8, 10),
                "future_version",
            ),
            legacy_encode_v8_reencode_case(v8),
        ]
    }

    fn player_legacy_early_fixture_cases() -> Vec<FrozenCase> {
        let v1 = read_go_fixture("server/storage/player/testdata/player-v1.bin");
        let v2 = read_go_fixture("server/storage/player/testdata/player-v2.bin");
        let v3 = read_go_fixture("server/storage/player/testdata/player-v3.bin");
        let v4 = read_go_fixture("server/storage/player/testdata/player-v4.bin");
        vec![
            legacy_decode_ok_case("save.player/1/decode/v1-fixture", "1", v1.clone()),
            legacy_decode_error_case(
                "save.player/1/decode/truncated-payload",
                "1",
                player_truncated_wire(&v1, 1),
                "corrupt",
            ),
            legacy_decode_ok_case("save.player/2/decode/v2-fixture", "2", v2.clone()),
            legacy_decode_error_case(
                "save.player/2/decode/corrupt-crc",
                "2",
                player_corrupt_crc_wire(&v2),
                "corrupt",
            ),
            legacy_decode_ok_case("save.player/3/decode/v3-fixture", "3", v3.clone()),
            legacy_decode_error_case(
                "save.player/3/decode/invalid-version-zero",
                "3",
                player_wire_with_schema(&v3, 0),
                "corrupt",
            ),
            legacy_decode_ok_case("save.player/4/decode/v4-fixture", "4", v4.clone()),
            legacy_decode_error_case(
                "save.player/4/decode/invalid-version-future",
                "4",
                player_wire_with_schema(&v4, 10),
                "future_version",
            ),
            legacy_encode_v4_reencode_case(v4),
        ]
    }

    fn player_cases_from_manifest() -> Vec<FrozenCase> {
        load_cases_for_consumer(CorpusConsumer::Storage)
            .into_iter()
            .filter(|case| case.family == "save.player")
            .collect()
    }

    const LEGACY_EARLY_INTEGRATED_CASE_IDS: &[&str] = &[
        "save.player/1/decode/truncated-payload",
        "save.player/1/decode/v1-fixture",
        "save.player/2/decode/corrupt-crc",
        "save.player/2/decode/v2-fixture",
        "save.player/3/decode/invalid-version-zero",
        "save.player/3/decode/v3-fixture",
        "save.player/4/decode/invalid-version-future",
        "save.player/4/decode/v4-fixture",
        "save.player/9/encode/v4-fixture-reencode",
    ];

    #[test]
    fn player_legacy_early_executes_integrated_manifest_cases() {
        let cases = player_cases_from_manifest()
            .into_iter()
            .filter(|case| player_legacy_early_case(&case.id))
            .collect::<Vec<_>>();
        let found: BTreeMap<&str, &FrozenCase> = cases
            .iter()
            .map(|case| (case.id.as_str(), case))
            .collect();
        for id in LEGACY_EARLY_INTEGRATED_CASE_IDS {
            if !found.contains_key(id) {
                panic!("integrated manifest missing legacy early case {id}");
            }
        }
        if cases.len() != LEGACY_EARLY_INTEGRATED_CASE_IDS.len() {
            panic!(
                "integrated manifest has {} legacy early cases, want {}",
                cases.len(),
                LEGACY_EARLY_INTEGRATED_CASE_IDS.len()
            );
        }
        execute_player_cases(&cases);
    }

    #[test]
    fn player_legacy_early_executes_fixture_cases() {
        let cases = player_legacy_early_fixture_cases();
        execute_player_cases(&cases);
    }

    #[test]
    fn player_legacy_late_case_ids_recognized() {
        assert!(player_legacy_late_case("save.player/5/decode/v5-fixture"));
        assert!(player_legacy_late_case("save.player/9/encode/v8-fixture-reencode"));
        assert!(!player_legacy_late_case("save.player/4/decode/v4-fixture"));
    }

    #[test]
    fn player_legacy_late_executes_fixture_cases() {
        let cases = player_legacy_late_fixture_cases();
        execute_player_cases(&cases);
    }

    #[test]
    fn player_legacy_late_v5_health_pinned_from_decode() {
        let v5 = read_go_fixture("server/storage/player/testdata/player-v5.bin");
        let player_id = fixture_player_id();
        let stored = decode_player(player_id, &v5).expect("decode v5 fixture");
        assert!(stored.needs_rewrite, "historical v5 decode must set needs_rewrite");
        assert_ne!(stored.health, 0, "v5 health must come from fixture decode");
        let case = legacy_decode_ok_case("save.player/5/decode/v5-fixture", "5", v5);
        let digest = value_sha256(&stored_player_value(&stored));
        assert_eq!(
            case.normalized.get("value_sha256").and_then(|v| v.as_str()),
            Some(digest.as_str()),
            "v5 digest must pin decoded health"
        );
    }

    #[test]
    fn player_legacy_late_v7_hunger_saturation_exhaustion_pinned_from_decode() {
        let v7 = read_go_fixture("server/storage/player/testdata/player-v7.bin");
        let player_id = fixture_player_id();
        let stored = decode_player(player_id, &v7).expect("decode v7 fixture");
        assert!(stored.needs_rewrite);
        assert_eq!(stored.hunger, 12);
        assert_eq!(stored.saturation_milli, 2500);
        assert_eq!(stored.exhaustion_milli, 1750);
        assert!(!stored.respawn_present);
        assert_eq!(stored.respawn_position, [0.0, 0.0, 0.0]);
        assert_eq!(stored.respawn_dimension, 0);
    }

    #[test]
    fn player_legacy_late_v8_respawn_present_pinned_from_decode() {
        let v8 = read_go_fixture("server/storage/player/testdata/player-v8.bin");
        let player_id = fixture_player_id();
        let stored = decode_player(player_id, &v8).expect("decode v8 fixture");
        assert!(stored.needs_rewrite);
        assert!(stored.respawn_present);
    }

    #[test]
    fn player_legacy_late_health_digest_mutation_fails_comparison() {
        let v5 = read_go_fixture("server/storage/player/testdata/player-v5.bin");
        let mut case = legacy_decode_ok_case("save.player/5/decode/v5-fixture", "5", v5);
        let player_id = fixture_player_id();
        let mut stored = decode_player(player_id, &case.input).expect("decode v5 fixture");
        stored.health += 1;
        case.normalized = serde_json::json!({
            "kind": "ok",
            "category": "save",
            "value_sha256": value_sha256(&stored_player_value(&stored))
        });
        let err = execute_player_case(&case).expect_err("stale health digest must fail");
        assert!(
            err.contains("value digest mismatch"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn player_legacy_late_needs_rewrite_digest_mutation_fails_comparison() {
        let v8 = read_go_fixture("server/storage/player/testdata/player-v8.bin");
        let mut case = legacy_decode_ok_case("save.player/8/decode/v8-fixture", "8", v8);
        let player_id = fixture_player_id();
        let mut stored = decode_player(player_id, &case.input).expect("decode v8 fixture");
        stored.needs_rewrite = false;
        case.normalized = serde_json::json!({
            "kind": "ok",
            "category": "save",
            "value_sha256": value_sha256(&stored_player_value(&stored))
        });
        let err = execute_player_case(&case).expect_err("stale needs_rewrite digest must fail");
        assert!(
            err.contains("value digest mismatch"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn player_legacy_early_armor_digest_mutation_fails_comparison() {
        let v3 = read_go_fixture("server/storage/player/testdata/player-v3.bin");
        let mut case = legacy_decode_ok_case("save.player/3/decode/v3-fixture", "3", v3);
        let player_id = fixture_player_id();
        let mut stored = decode_player(player_id, &case.input).expect("decode v3 fixture");
        stored.armor[0].item += 1;
        case.normalized = serde_json::json!({
            "kind": "ok",
            "category": "save",
            "value_sha256": value_sha256(&stored_player_value(&stored))
        });
        let err = execute_player_case(&case).expect_err("stale armor digest must fail");
        assert!(
            err.contains("value digest mismatch"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn player_legacy_early_needs_rewrite_digest_mutation_fails_comparison() {
        let v4 = read_go_fixture("server/storage/player/testdata/player-v4.bin");
        let mut case = legacy_decode_ok_case("save.player/4/decode/v4-fixture", "4", v4);
        let player_id = fixture_player_id();
        let mut stored = decode_player(player_id, &case.input).expect("decode v4 fixture");
        stored.needs_rewrite = false;
        case.normalized = serde_json::json!({
            "kind": "ok",
            "category": "save",
            "value_sha256": value_sha256(&stored_player_value(&stored))
        });
        let err = execute_player_case(&case).expect_err("stale needs_rewrite digest must fail");
        assert!(
            err.contains("value digest mismatch"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn player_current_executes_integrated_cases() {
        let cases = player_cases_from_manifest();
        if cases.is_empty() {
            panic!("no integrated save.player cases");
        }
        execute_player_cases(&cases);
    }

    #[test]
    fn player_current_armor_digest_mutation_fails_comparison() {
        let encoded = encode_player_fixture(&player_raw_armor_save());
        let player_id = PlayerId::from_bytes([
            0x00, 0x11, 0x22, 0x33, 0x44, 0x55, 0x46, 0x77, 0x88, 0x99, 0xaa, 0xbb, 0xcc, 0xdd,
            0xee, 0xff,
        ]);
        let stored = decode_player(player_id, &encoded).expect("decode armor fixture");
        let mut stale = stored;
        stale.armor[0].item += 1;
        let case = FrozenCase {
            id: "save.player/9/decode/raw-armor-triple".to_string(),
            family: "save.player".to_string(),
            version: "9".to_string(),
            consumer: CorpusConsumer::Storage,
            packet_key: None,
            operation: "decode".to_string(),
            arguments: serde_json::json!({
                "requested_player_id": "00112233445546778899aabbccddeeff"
            }),
            input_format: InputFormat::Binary,
            input: encoded,
            input_json: None,
            normalized: serde_json::json!({
                "kind": "ok",
                "category": "save",
                "value_sha256": value_sha256(&stored_player_value(&stale))
            }),
            encoded: None,
            category: "save".to_string(),
        };
        let err = execute_player_case(&case).expect_err("stale armor digest must fail");
        assert!(
            err.contains("value digest mismatch"),
            "unexpected error: {err}"
        );
    }

    fn player_raw_armor_save() -> PlayerSave {
        let mut save = fixture_player_save(7);
        save.armor[0] = ItemStack {
            item: 4242,
            count: 65,
            durability: 999,
        };
        save
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
            player_id: PlayerId::from_bytes([
                0x00, 0x11, 0x22, 0x33, 0x44, 0x55, 0x46, 0x77, 0x88, 0x99, 0xaa, 0xbb, 0xcc,
                0xdd, 0xee, 0xff,
            ]),
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
            respawn_present: true,
            respawn_position: [7.0, 65.0, -9.0],
            respawn_dimension: 0,
            armor: [
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
            ],
        }
    }

    fn encode_player_fixture(save: &PlayerSave) -> Vec<u8> {
        let needed = player_encoded_len(save).expect("encoded len");
        let mut buf = vec![0u8; needed];
        let written = encode_player_into(save, &mut buf).expect("encode");
        buf.truncate(written);
        buf
    }
}
