//! Executable `save.companion` legacy decode routes for `mornlea_storage`.
//!
//! Node 4.6a seeds v1..v4 migration evidence. The shared dispatcher in
//! `storage_corpus.rs` delegates here once the controller registers the
//! reviewed routes against integrated manifest assets.

use super::value_digest::{Value, value_sha256};
use crate::runtime_corpus::{FrozenCase, InputFormat};
use mornlea_storage::{
    CompanionBody, CompanionSave, Inventory, ItemStack, PlanStep, StorageError,
    StoredCompanionLifecycle, StoredCompanionQueue, StoredCompanionTask, StoredCompanions,
    companions_encoded_len, decode_companions, encode_companions_into,
};
use serde_json::Value as JsonValue;
use std::collections::BTreeMap;
use std::path::PathBuf;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub struct CompanionRoute {
    pub family: &'static str,
    pub version: &'static str,
    pub operation: &'static str,
}

/// Routes the companion module executes once reviewed assets are integrated.
pub const COMPANION_REGISTERED_ROUTES: &[CompanionRoute] = &[
    CompanionRoute {
        family: "save.companion",
        version: "1",
        operation: "decode",
    },
    CompanionRoute {
        family: "save.companion",
        version: "2",
        operation: "decode",
    },
    CompanionRoute {
        family: "save.companion",
        version: "3",
        operation: "decode",
    },
    CompanionRoute {
        family: "save.companion",
        version: "4",
        operation: "decode",
    },
    CompanionRoute {
        family: "save.companion",
        version: "5",
        operation: "decode",
    },
    CompanionRoute {
        family: "save.companion",
        version: "5",
        operation: "encode",
    },
];

/// Current-schema v5 cases exported in node 4.6b (integrated separately).
pub fn companion_current_case(id: &str) -> bool {
    id == "save.companion/5/decode/v5-fixture"
        || id == "save.companion/5/decode/v5-roundtrip-alt"
        || id == "save.companion/5/decode/max-legal-size"
        || id == "save.companion/5/encode/v5-canonical"
        || id == "save.companion/5/encode/capacity-minus-one"
}

/// Adversarial v5 decode cases exported in node 4.6b (integrated separately).
pub fn companion_adversarial_case(id: &str) -> bool {
    id.starts_with("save.companion/5/decode/body-count-")
        || id == "save.companion/5/decode/active-count-five"
        || id == "save.companion/5/decode/duplicate-lifecycle"
        || id == "save.companion/5/decode/missing-lifecycle"
        || id == "save.companion/5/decode/orphan-queue"
        || id == "save.companion/5/decode/inactive-queue"
        || id == "save.companion/5/decode/command-over-limit"
        || id == "save.companion/5/decode/plan-steps-over-limit"
        || id == "save.companion/5/decode/fifo-over-limit"
        || id == "save.companion/5/decode/summary-over-limit"
        || id == "save.companion/5/decode/invalid-version-zero"
        || id == "save.companion/5/decode/invalid-version-future"
        || id == "save.companion/5/decode/truncated-header"
        || id == "save.companion/5/decode/trailing-byte"
        || id == "save.companion/5/decode/corrupt-crc"
        || id == "save.companion/5/decode/malformed-uuid"
}

pub fn companion_legacy_case(id: &str) -> bool {
    id.starts_with("save.companion/1/decode/")
        || id.starts_with("save.companion/2/decode/")
        || id.starts_with("save.companion/3/decode/")
        || id.starts_with("save.companion/4/decode/")
}

fn route_is_registered(case: &FrozenCase) -> bool {
    COMPANION_REGISTERED_ROUTES.iter().any(|route| {
        route.family == case.family
            && route.version == case.version
            && route.operation == case.operation
    })
}

fn companion_schema_version(input: &[u8]) -> Result<u32, String> {
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

fn companion_body_value(body: &CompanionBody) -> Value {
    let mut fields = BTreeMap::new();
    fields.insert("dimension".to_string(), Value::Signed(body.dimension as i64));
    fields.insert("id".to_string(), Value::Bytes(body.id.to_bytes().to_vec()));
    fields.insert("inventory".to_string(), inventory_value(&body.inventory));
    fields.insert("pitch".to_string(), Value::F32(body.pitch));
    fields.insert("position".to_string(), vec3_value(body.position));
    fields.insert("yaw".to_string(), Value::F32(body.yaw));
    Value::Object(fields)
}

fn plan_step_value(step: &PlanStep) -> Value {
    let mut fields = BTreeMap::new();
    fields.insert("block".to_string(), Value::Unsigned(step.block as u64));
    fields.insert("kind".to_string(), Value::Unsigned(step.kind as u64));
    fields.insert(
        "player_id".to_string(),
        Value::Bytes(step.player_id.to_bytes().to_vec()),
    );
    fields.insert("x".to_string(), Value::Signed(step.x as i64));
    fields.insert("y".to_string(), Value::Signed(step.y as i64));
    fields.insert("z".to_string(), Value::Signed(step.z as i64));
    Value::Object(fields)
}

fn companion_task_value(task: &StoredCompanionTask) -> Value {
    let steps: Vec<Value> = task.plan_steps.iter().map(plan_step_value).collect();
    let mut fields = BTreeMap::new();
    fields.insert("command".to_string(), Value::Utf8(task.command.clone()));
    fields.insert(
        "deadline_ticks".to_string(),
        Value::Unsigned(task.deadline_ticks),
    );
    fields.insert(
        "fail_reason".to_string(),
        Value::Unsigned(task.fail_reason as u64),
    );
    fields.insert("plan_steps".to_string(), Value::Array(steps));
    fields.insert("start_tick".to_string(), Value::Unsigned(task.start_tick));
    fields.insert("state".to_string(), Value::Unsigned(task.state as u64));
    fields.insert(
        "step_index".to_string(),
        Value::Signed(task.step_index as i64),
    );
    Value::Object(fields)
}

fn companion_queue_value(queue: &StoredCompanionQueue) -> Value {
    let pending: Vec<Value> = queue
        .pending
        .iter()
        .map(|entry| Value::Utf8(entry.clone()))
        .collect();
    let current = if queue.has_current {
        companion_task_value(&queue.current)
    } else {
        Value::Null
    };
    let mut fields = BTreeMap::new();
    fields.insert("current".to_string(), current);
    fields.insert("has_current".to_string(), Value::Bool(queue.has_current));
    fields.insert("id".to_string(), Value::Bytes(queue.id.to_bytes().to_vec()));
    fields.insert("pending".to_string(), Value::Array(pending));
    fields.insert("summary".to_string(), Value::Utf8(queue.summary.clone()));
    Value::Object(fields)
}

fn companion_lifecycle_value(lifecycle: &StoredCompanionLifecycle) -> Value {
    let mut fields = BTreeMap::new();
    fields.insert("active".to_string(), Value::Bool(lifecycle.active));
    fields.insert("id".to_string(), Value::Bytes(lifecycle.id.to_bytes().to_vec()));
    fields.insert(
        "memory_epoch".to_string(),
        Value::Unsigned(lifecycle.memory_epoch),
    );
    fields.insert(
        "memory_operation_id".to_string(),
        Value::Bytes(lifecycle.memory_operation_id.to_bytes().to_vec()),
    );
    fields.insert(
        "memory_revision".to_string(),
        Value::Unsigned(lifecycle.memory_revision),
    );
    fields.insert(
        "summary".to_string(),
        Value::Utf8(lifecycle.summary.clone()),
    );
    fields.insert(
        "tombstone_operation_id".to_string(),
        Value::Bytes(lifecycle.tombstone_operation_id.to_bytes().to_vec()),
    );
    Value::Object(fields)
}

pub fn companions_value(stored: &StoredCompanions) -> Value {
    let records: Vec<Value> = stored.records.iter().map(companion_body_value).collect();
    let lifecycles: Vec<Value> = stored
        .lifecycles
        .iter()
        .map(companion_lifecycle_value)
        .collect();
    let queues: Vec<Value> = stored.queues.iter().map(companion_queue_value).collect();
    let mut fields = BTreeMap::new();
    fields.insert(
        "agent_namespace_id".to_string(),
        Value::Bytes(stored.agent_namespace_id.to_bytes().to_vec()),
    );
    fields.insert("lifecycles".to_string(), Value::Array(lifecycles));
    fields.insert("queues".to_string(), Value::Array(queues));
    fields.insert("records".to_string(), Value::Array(records));
    fields.insert("revision".to_string(), Value::Unsigned(stored.revision));
    fields.insert(
        "source_schema".to_string(),
        Value::Unsigned(stored.source_schema as u64),
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

#[allow(dead_code)]
pub fn execute_companion_cases(cases: &[FrozenCase]) {
    if cases.is_empty() {
        panic!("companion corpus selection executed zero cases");
    }
    for case in cases {
        assert_eq!(
            case.family, "save.companion",
            "case {} is not a save.companion row",
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
            "unregistered companion route {}/{}/{} for case {}",
            case.family,
            case.version,
            case.operation,
            case.id
        );
        execute_companion_case(case).unwrap_or_else(|err| {
            panic!("case {} failed: {err}", case.id);
        });
    }
}

#[derive(Debug)]
struct CompanionArguments {
    capacity: Option<u32>,
}

fn parse_companion_arguments(case: &FrozenCase) -> Result<CompanionArguments, String> {
    let obj = case
        .arguments
        .as_object()
        .ok_or_else(|| format!("case {} arguments must be an object", case.id))?;
    let capacity = match obj.get("capacity") {
        None => None,
        Some(value) => Some(
            value
                .as_u64()
                .and_then(|number| u32::try_from(number).ok())
                .ok_or_else(|| format!("case {} capacity must be u32", case.id))?,
        ),
    };
    Ok(CompanionArguments { capacity })
}

fn stored_to_save(stored: &StoredCompanions) -> CompanionSave {
    CompanionSave {
        revision: stored.revision,
        agent_namespace_id: stored.agent_namespace_id,
        records: stored.records.clone(),
        lifecycles: stored.lifecycles.clone(),
        queues: stored.queues.clone(),
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

pub fn execute_companion_case(case: &FrozenCase) -> Result<(), String> {
    let args = parse_companion_arguments(case)?;
    match case.operation.as_str() {
        "decode" => execute_companion_decode(case),
        "encode" => execute_companion_encode(case, &args),
        other => Err(format!("unsupported companion operation {other}")),
    }
}

fn execute_companion_decode(case: &FrozenCase) -> Result<(), String> {
    let schema = companion_schema_version(&case.input)?;
    let want_version: u32 = case
        .version
        .parse()
        .map_err(|_| format!("case {} has invalid version", case.id))?;
    match decode_companions(&case.input) {
        Ok(stored) => {
            assert_ok_outcome(case)?;
            if schema != want_version {
                return Err(format!(
                    "case {} accepted input schema {schema}, want {want_version}",
                    case.id
                ));
            }
            expected_value_digest(case, &companions_value(&stored))
        }
        Err(err) => {
            let category = storage_error_category(&err)
                .ok_or_else(|| format!("unclassified rejection: {err}"))?;
            assert_error_category(case, category)
        }
    }
}

fn execute_companion_encode(case: &FrozenCase, args: &CompanionArguments) -> Result<(), String> {
    let stored = decode_companions(&case.input).map_err(|err| err.to_string())?;
    let digest_value = companions_value(&stored);
    let save = stored_to_save(&stored);
    let needed = companions_encoded_len(&save).map_err(|err| err.to_string())?;
    let buf_len = args.capacity.map(|cap| cap as usize).unwrap_or(needed);
    let mut encoded = vec![0u8; buf_len];
    let written = match encode_companions_into(&save, &mut encoded) {
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
        if let Some(expected) = &case.encoded {
            if expected.as_slice() != encoded.as_slice() {
                return Err(format!("case {} encoded bytes mismatch", case.id));
            }
        }
        let round = decode_companions(&encoded).map_err(|err| err.to_string())?;
        if round.source_schema != 5 {
            return Err(format!(
                "case {} encoded output schema {}",
                case.id,
                round.source_schema
            ));
        }
    }
    Ok(())
}

fn read_go_fixture(name: &str) -> Vec<u8> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(format!(
        "../../../../packages/server/storage/companion/testdata/{name}"
    ));
    std::fs::read(&path).unwrap_or_else(|err| panic!("read {}: {err}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use mornlea_storage::crc32c_join;

    fn companion_arguments_json() -> JsonValue {
        JsonValue::Object(serde_json::Map::new())
    }

    fn companion_truncated_wire(wire: &[u8], drop_tail: usize) -> Vec<u8> {
        if drop_tail == 0 || drop_tail >= wire.len() {
            return wire.to_vec();
        }
        wire[..wire.len() - drop_tail].to_vec()
    }

    fn companion_corrupt_crc_wire(wire: &[u8]) -> Vec<u8> {
        let mut out = wire.to_vec();
        if out.len() >= 32 {
            out[28] ^= 0xff;
        }
        out
    }

    fn companion_wire_with_schema(wire: &[u8], schema: u32) -> Vec<u8> {
        let mut out = wire.to_vec();
        if out.len() >= 12 {
            out[8..12].copy_from_slice(&schema.to_le_bytes());
        }
        out
    }

    fn legacy_decode_ok_case(id: &str, version: &str, input: Vec<u8>) -> FrozenCase {
        let stored =
            decode_companions(&input).unwrap_or_else(|err| panic!("case {id} decode fixture: {err}"));
        FrozenCase {
            id: id.to_string(),
            family: "save.companion".to_string(),
            version: version.to_string(),
            consumer: crate::runtime_corpus::CorpusConsumer::Storage,
            packet_key: None,
            operation: "decode".to_string(),
            arguments: companion_arguments_json(),
            input_format: InputFormat::Binary,
            input: input.clone(),
            input_json: None,
            normalized: serde_json::json!({
                "kind": "ok",
                "category": "save",
                "value_sha256": value_sha256(&companions_value(&stored))
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
            family: "save.companion".to_string(),
            version: version.to_string(),
            consumer: crate::runtime_corpus::CorpusConsumer::Storage,
            packet_key: None,
            operation: "decode".to_string(),
            arguments: companion_arguments_json(),
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

    fn companion_v3_two_distinct_queues_wire() -> Vec<u8> {
        let v3 = read_go_fixture("companions-v3.bin");
        let v4 = read_go_fixture("companions-v4.bin");
        let dec_v3 = decode_companions(&v3).expect("decode v3 fixture");
        let dec_v4 = decode_companions(&v4).expect("decode v4 fixture");
        encode_companion_legacy_wire(3, dec_v3.revision, &dec_v3.records, &[
            dec_v3.queues[0].clone(),
            dec_v4.queues[1].clone(),
        ])
        .expect("encode v3 dual-queue wire")
    }

    const LEGACY_FLAG_HAS_TASK: u8 = 1 << 0;
    const LEGACY_FLAG_HAS_FIFO: u8 = 1 << 1;
    const LEGACY_FLAG_HAS_SUMMARY: u8 = 1 << 2;

    fn append_plan_step(out: &mut Vec<u8>, step: &PlanStep) {
        out.push(step.kind);
        if step.kind == mornlea_storage::COMPANION_PLAN_STEP_FOLLOW {
            out.extend_from_slice(&step.player_id.to_bytes());
        } else {
            out.extend_from_slice(&step.x.to_le_bytes());
            out.extend_from_slice(&step.y.to_le_bytes());
            out.extend_from_slice(&step.z.to_le_bytes());
            if step.kind == mornlea_storage::COMPANION_PLAN_STEP_PLACE {
                out.extend_from_slice(&step.block.to_le_bytes());
            }
        }
    }

    fn append_task(out: &mut Vec<u8>, task: &StoredCompanionTask) {
        out.extend_from_slice(&(task.command.len() as u16).to_le_bytes());
        out.extend_from_slice(task.command.as_bytes());
        out.extend_from_slice(&(task.plan_steps.len() as u16).to_le_bytes());
        for step in &task.plan_steps {
            append_plan_step(out, step);
        }
        out.extend_from_slice(&(task.step_index as u32).to_le_bytes());
        out.push(task.state);
        out.push(task.fail_reason);
        out.extend_from_slice(&task.start_tick.to_le_bytes());
        out.extend_from_slice(&task.deadline_ticks.to_le_bytes());
    }

    fn append_fifo(out: &mut Vec<u8>, pending: &[String]) {
        out.extend_from_slice(&(pending.len() as u16).to_le_bytes());
        for entry in pending {
            out.extend_from_slice(&(entry.len() as u16).to_le_bytes());
            out.extend_from_slice(entry.as_bytes());
        }
    }

    fn append_body(out: &mut Vec<u8>, body: &CompanionBody) {
        out.extend_from_slice(&body.id.to_bytes());
        out.extend_from_slice(&(body.dimension as u32).to_le_bytes());
        for value in body.position {
            out.extend_from_slice(&value.to_le_bytes());
        }
        out.extend_from_slice(&body.yaw.to_le_bytes());
        out.extend_from_slice(&body.pitch.to_le_bytes());
        out.push(body.inventory.hotbar.selected);
        for stack in &body.inventory.hotbar.slots {
            out.extend_from_slice(&(stack.item as u16).to_le_bytes());
            out.push(stack.count);
            out.extend_from_slice(&stack.durability.to_le_bytes());
        }
        for stack in &body.inventory.backpack {
            out.extend_from_slice(&(stack.item as u16).to_le_bytes());
            out.push(stack.count);
            out.extend_from_slice(&stack.durability.to_le_bytes());
        }
    }

    fn append_legacy_queue(out: &mut Vec<u8>, schema: u32, queue: &StoredCompanionQueue) {
        let mut flags = 0u8;
        if queue.has_current {
            flags |= LEGACY_FLAG_HAS_TASK;
        }
        if !queue.pending.is_empty() {
            flags |= LEGACY_FLAG_HAS_FIFO;
        }
        if schema >= 4 && !queue.summary.is_empty() {
            flags |= LEGACY_FLAG_HAS_SUMMARY;
        }
        out.push(flags);
        if flags & LEGACY_FLAG_HAS_TASK != 0 {
            append_task(out, &queue.current);
        }
        if flags & LEGACY_FLAG_HAS_FIFO != 0 {
            append_fifo(out, &queue.pending);
        }
        if flags & LEGACY_FLAG_HAS_SUMMARY != 0 {
            out.extend_from_slice(&(queue.summary.len() as u16).to_le_bytes());
            out.extend_from_slice(queue.summary.as_bytes());
        }
    }

    fn encode_companion_legacy_wire(
        schema: u32,
        revision: u64,
        records: &[CompanionBody],
        per_record: &[StoredCompanionQueue],
    ) -> Result<Vec<u8>, String> {
        let mut payload = Vec::new();
        for (index, body) in records.iter().enumerate() {
            append_body(&mut payload, body);
            if schema == 1 {
                continue;
            }
            let queue = per_record.get(index).cloned().unwrap_or_default();
            if queue.has_current || !queue.pending.is_empty() || !queue.summary.is_empty() {
                append_legacy_queue(&mut payload, schema, &queue);
            } else {
                payload.push(0);
            }
        }
        let mut header = Vec::new();
        header.extend_from_slice(b"MCAI");
        header.extend_from_slice(&1u32.to_le_bytes());
        header.extend_from_slice(&schema.to_le_bytes());
        header.extend_from_slice(&revision.to_le_bytes());
        header.extend_from_slice(&(records.len() as u32).to_le_bytes());
        header.extend_from_slice(&(payload.len() as u32).to_le_bytes());
        header.extend_from_slice(&0u32.to_le_bytes());
        header.extend_from_slice(&payload);
        let checksum = crc32c_join(&[&header[8..28], &header[32..]]);
        header[28..32].copy_from_slice(&checksum.to_le_bytes());
        Ok(header)
    }

    fn companion_legacy_fixture_cases() -> Vec<FrozenCase> {
        let v1 = read_go_fixture("companions-v1.bin");
        let v2 = read_go_fixture("companions-v2.bin");
        let v3 = read_go_fixture("companions-v3.bin");
        let v4 = read_go_fixture("companions-v4.bin");
        vec![
            legacy_decode_ok_case("save.companion/1/decode/v1-fixture", "1", v1.clone()),
            legacy_decode_error_case(
                "save.companion/1/decode/truncated-payload",
                "1",
                companion_truncated_wire(&v1, 1),
                "corrupt",
            ),
            legacy_decode_ok_case("save.companion/2/decode/v2-fixture", "2", v2.clone()),
            legacy_decode_error_case(
                "save.companion/2/decode/corrupt-crc",
                "2",
                companion_corrupt_crc_wire(&v2),
                "corrupt",
            ),
            legacy_decode_ok_case("save.companion/3/decode/v3-fixture", "3", v3.clone()),
            legacy_decode_error_case(
                "save.companion/3/decode/invalid-version-zero",
                "3",
                companion_wire_with_schema(&v3, 0),
                "corrupt",
            ),
            legacy_decode_ok_case("save.companion/4/decode/v4-fixture", "4", v4.clone()),
            legacy_decode_error_case(
                "save.companion/4/decode/invalid-version-future",
                "4",
                companion_wire_with_schema(&v4, 10),
                "future_version",
            ),
        ]
    }

    #[test]
    fn companion_legacy_routes_recognize_registered_paths() {
        assert!(route_is_registered(&FrozenCase {
            id: "save.companion/2/decode/v2-fixture".to_string(),
            family: "save.companion".to_string(),
            version: "2".to_string(),
            consumer: crate::runtime_corpus::CorpusConsumer::Storage,
            packet_key: None,
            operation: "decode".to_string(),
            arguments: companion_arguments_json(),
            input_format: InputFormat::Binary,
            input: vec![],
            input_json: None,
            normalized: JsonValue::Object(serde_json::Map::new()),
            encoded: None,
            category: "save".to_string(),
        }));
    }

    #[test]
    fn companion_legacy_fixture_cases_execute_locally() {
        let cases = companion_legacy_fixture_cases();
        assert_eq!(cases.len(), 8);
        for case in &cases {
            execute_companion_case(case).expect("local legacy companion fixture");
        }
    }

    #[test]
    fn companion_legacy_queue_digest_mutation_fails_comparison() {
        let golden = read_go_fixture("companions-v4.bin");
        let mut stale = decode_companions(&golden).expect("decode v4");
        if let Some(queue) = stale.queues.first_mut() {
            if let Some(entry) = queue.pending.first_mut() {
                entry.push('x');
            }
        }
        let stale_digest = value_sha256(&companions_value(&stale));
        let mut case = legacy_decode_ok_case("save.companion/4/decode/v4-fixture", "4", golden);
        case.normalized = serde_json::json!({
            "kind": "ok",
            "category": "save",
            "value_sha256": stale_digest
        });
        let err = execute_companion_case(&case).expect_err("stale queue digest must fail");
        assert!(
            err.contains("value digest mismatch"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn companion_legacy_input_swap_digest_mutation_fails_comparison() {
        let v3 = read_go_fixture("companions-v3.bin");
        let dual = companion_v3_two_distinct_queues_wire();
        let fixture = decode_companions(&v3).expect("decode v3");
        let dual_decoded = decode_companions(&dual).expect("decode dual");
        let fixture_digest = value_sha256(&companions_value(&fixture));
        let dual_digest = value_sha256(&companions_value(&dual_decoded));
        assert_ne!(fixture_digest, dual_digest);
        let mut case = legacy_decode_ok_case("save.companion/3/decode/v3-fixture", "3", v3);
        case.normalized = serde_json::json!({
            "kind": "ok",
            "category": "save",
            "value_sha256": fixture_digest
        });
        case.input = dual;
        let err = execute_companion_case(&case).expect_err("swapped input must fail digest");
        assert!(
            err.contains("value digest mismatch"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn companion_legacy_case_ids_match_go_producer() {
        for id in [
            "save.companion/1/decode/v1-fixture",
            "save.companion/1/decode/truncated-payload",
            "save.companion/2/decode/v2-fixture",
            "save.companion/2/decode/corrupt-crc",
            "save.companion/3/decode/v3-fixture",
            "save.companion/3/decode/invalid-version-zero",
            "save.companion/4/decode/v4-fixture",
            "save.companion/4/decode/invalid-version-future",
        ] {
            assert!(companion_legacy_case(id), "missing legacy id {id}");
        }
    }

    #[test]
    fn companion_current_case_ids_recognized() {
        assert!(companion_current_case("save.companion/5/decode/v5-fixture"));
        assert!(!companion_current_case("save.companion/4/decode/v4-fixture"));
    }

    #[test]
    fn companion_adversarial_case_ids_recognized() {
        assert!(companion_adversarial_case("save.companion/5/decode/corrupt-crc"));
        assert!(!companion_adversarial_case("save.companion/5/decode/v5-fixture"));
    }

    #[test]
    fn companion_current_executes_fixture_cases() {
        let golden = read_go_fixture("companions-v5.bin");
        let case = legacy_decode_ok_case("save.companion/5/decode/v5-fixture", "5", golden);
        execute_companion_case(&case).expect("v5 golden fixture");
    }

    #[test]
    fn companion_adversarial_executes_fixture_cases() {
        let golden = read_go_fixture("companions-v5.bin");
        let case = legacy_decode_error_case(
            "save.companion/5/decode/corrupt-crc",
            "5",
            companion_corrupt_crc_wire(&golden),
            "corrupt",
        );
        execute_companion_case(&case).expect("corrupt crc adversarial fixture");
    }

    #[test]
    fn companion_current_queue_owner_digest_mutation_fails_comparison() {
        let golden = read_go_fixture("companions-v5.bin");
        let mut stale = decode_companions(&golden).expect("decode v5");
        if let Some(queue) = stale.queues.first_mut() {
            queue.id = mornlea_storage::PlayerId::from_bytes([0; 16]);
        }
        let mut case = legacy_decode_ok_case("save.companion/5/decode/v5-fixture", "5", golden);
        case.normalized = serde_json::json!({
            "kind": "ok",
            "category": "save",
            "value_sha256": value_sha256(&companions_value(&stale))
        });
        let err = execute_companion_case(&case).expect_err("stale queue owner digest must fail");
        assert!(err.contains("value digest mismatch"), "unexpected error: {err}");
    }

    #[test]
    fn companion_current_fifo_digest_mutation_fails_comparison() {
        let golden = read_go_fixture("companions-v5.bin");
        let mut stale = decode_companions(&golden).expect("decode v5");
        if let Some(queue) = stale.queues.first_mut() {
            if let Some(entry) = queue.pending.first_mut() {
                entry.push('x');
            }
        }
        let mut case = legacy_decode_ok_case("save.companion/5/decode/v5-fixture", "5", golden);
        case.normalized = serde_json::json!({
            "kind": "ok",
            "category": "save",
            "value_sha256": value_sha256(&companions_value(&stale))
        });
        let err = execute_companion_case(&case).expect_err("stale fifo digest must fail");
        assert!(err.contains("value digest mismatch"), "unexpected error: {err}");
    }
}
