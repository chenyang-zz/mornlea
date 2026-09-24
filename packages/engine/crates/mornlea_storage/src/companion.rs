//! `save.companion`: the companion aggregate save (`companions.ai`).
//!
//! Ported from the Go `companion` storage codec. Schema v5 carries a 16-byte
//! agent namespace plus per-record lifecycle metadata; v1..v4 stay read-only
//! migration input. Records are written in strictly ascending ID order and the
//! CRC-32C covers the identity fields plus the payload, never the magic or the
//! checksum bytes.

use crate::bytes::{ByteReader, ByteWriter};
use crate::crc32c::crc32c_join;
use crate::error::{StorageResult, corrupt, future_version};
use crate::identity::PlayerId;
use crate::items::{Inventory, ItemStack};

/// Companion lifecycle operation identity. It shares the canonical UUIDv4 rule
/// with [`PlayerId`]; the alias keeps the save contract's own vocabulary.
pub type Identity = PlayerId;

/// Envelope version written by the current encoder.
pub const ENVELOPE_VERSION: u32 = 1;

/// Legacy schemas still accepted for read-only migration.
pub const SCHEMA_V1: u32 = 1;
pub const SCHEMA_V2: u32 = 2;
pub const SCHEMA_V3: u32 = 3;
pub const SCHEMA_V4: u32 = 4;

/// Schema written by the current encoder.
pub const CURRENT_SCHEMA: u32 = 5;

const HEADER_LENGTH: usize = 32;
const RECORD_LENGTH: usize = 221;
/// Physical byte ceiling of any reachable v5 file.
pub const MAX_FILE_LENGTH: usize = 393_904;

/// Records one aggregate save may hold, counted active plus inactive.
pub const MAX_STORED: usize = 64;
/// Records that may be active at the same time.
pub const MAX_ACTIVE: usize = 4;

/// UTF-8 byte ceiling of one task or FIFO command.
pub const MAX_TASK_COMMAND_BYTES: usize = 1024;
/// Defensive ceiling on persisted plan steps per task.
pub const MAX_PLAN_STEPS: usize = 5_000;
/// FIFO depth ceiling, matching the runtime task queue capacity.
pub const MAX_FIFO_ENTRIES: usize = 16;
/// UTF-8 byte ceiling of one persisted dialogue summary.
pub const MAX_SUMMARY_BYTES: usize = 2_048;

const TASK_COMMAND_PREFIX_LENGTH: usize = 2;
const SUMMARY_PREFIX_LENGTH: usize = 2;
/// Byte length of a `go_to`/`mine` step (kind + three int32 coordinates).
const PLAN_STEP_LENGTH: usize = 13;

// v5 flags pin active, task, and FIFO to bits 0/1/2. Legacy v2..v4 use a
// different bit assignment, so a v5 lifecycle bit must never be read as an
// old task bit.
const FLAG_ACTIVE: u8 = 1 << 0;
const FLAG_HAS_TASK: u8 = 1 << 1;
const FLAG_HAS_FIFO: u8 = 1 << 2;
const LEGACY_FLAG_HAS_TASK: u8 = 1 << 0;
const LEGACY_FLAG_HAS_FIFO: u8 = 1 << 1;
const LEGACY_FLAG_HAS_SUMMARY: u8 = 1 << 2;

const MAGIC: [u8; 4] = *b"MCAI";
const OVERWORLD: i32 = 0;
const MIN_Y: i32 = -64;
const MAX_Y: i32 = 320;

/// Task lifecycle state. Values are the seven-state domain enum.
pub const TASK_QUEUED: u8 = 1;
pub const TASK_PLANNING: u8 = 2;
pub const TASK_VALIDATING: u8 = 3;
pub const TASK_RUNNING: u8 = 4;
pub const TASK_COMPLETED: u8 = 5;
pub const TASK_FAILED: u8 = 6;
pub const TASK_TIMED_OUT: u8 = 7;
pub const TASK_STOPPED: u8 = 8;

/// Stable failure reasons. Zero means "not failed".
pub const TASK_FAIL_NONE: u8 = 0;
pub const TASK_FAIL_PLANNER_UNAVAILABLE: u8 = 1;
pub const TASK_FAIL_INVALID_PLAN: u8 = 2;
pub const TASK_FAIL_PATH_UNREACHABLE: u8 = 3;
pub const TASK_FAIL_WORLD_CHANGED: u8 = 4;
pub const TASK_FAIL_INVENTORY_FULL: u8 = 5;

/// Delivered plan-step kinds.
pub const PLAN_STEP_GO_TO: u8 = 1;
pub const PLAN_STEP_FOLLOW: u8 = 2;
pub const PLAN_STEP_MINE: u8 = 3;
pub const PLAN_STEP_PLACE: u8 = 4;

/// One persisted companion body. Runtime-derived facts are absent: the body is
/// the authoritative simulation snapshot the domain owns.
#[derive(Clone, Debug, PartialEq)]
pub struct CompanionBody {
    pub id: PlayerId,
    pub dimension: i32,
    pub position: [f32; 3],
    pub yaw: f32,
    pub pitch: f32,
    pub inventory: Inventory,
}

/// v5 lifecycle and memory metadata associated with one body record.
///
/// An active record keeps a full recovery mirror; an inactive one keeps only a
/// tombstone operation. The codec validates the two shapes as disjoint sets.
#[derive(Clone, Debug, PartialEq)]
pub struct StoredCompanionLifecycle {
    pub id: PlayerId,
    pub active: bool,
    pub memory_epoch: u64,
    pub memory_revision: u64,
    pub memory_operation_id: Identity,
    pub summary: String,
    pub tombstone_operation_id: Identity,
}

/// One persisted task payload.
///
/// The plan's model summary and generation are deliberately not persisted: a
/// plan summary is free model text rather than a task fact, and generation
/// only discards stale in-flight worker results.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct StoredCompanionTask {
    pub command: String,
    pub plan_steps: Vec<PlanStep>,
    pub step_index: i32,
    pub state: u8,
    pub start_tick: u64,
    pub deadline_ticks: u64,
    pub fail_reason: u8,
}

/// One persisted task-domain payload: the current task, the FIFO commands, and
/// a summary that only carries schema v4 legacy text.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct StoredCompanionQueue {
    pub id: PlayerId,
    pub has_current: bool,
    pub current: StoredCompanionTask,
    pub pending: Vec<String>,
    /// Only carried for schema v4 migration input; a v5 save rejects it.
    pub summary: String,
}

/// One atomic plan step. Fields are used per kind.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct PlanStep {
    pub kind: u8,
    pub x: i32,
    pub y: i32,
    pub z: i32,
    pub block: u16,
    pub player_id: PlayerId,
}

/// A decoded companion aggregate.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct StoredCompanions {
    /// Schema the bytes came from; zero means no aggregate file exists yet.
    pub source_schema: u32,
    pub revision: u64,
    pub agent_namespace_id: Identity,
    pub records: Vec<CompanionBody>,
    pub lifecycles: Vec<StoredCompanionLifecycle>,
    pub queues: Vec<StoredCompanionQueue>,
}

/// A v5 companion aggregate save request.
///
/// `lifecycles` must be the same ID set as `records`, and queues may only
/// reference active records. Input slices are read but never mutated.
#[derive(Clone, Debug, PartialEq)]
pub struct CompanionSave {
    pub revision: u64,
    pub agent_namespace_id: Identity,
    pub records: Vec<CompanionBody>,
    pub lifecycles: Vec<StoredCompanionLifecycle>,
    pub queues: Vec<StoredCompanionQueue>,
}

/// Encodes a companion aggregate into its canonical v5 on-disk form.
pub fn encode(save: &CompanionSave) -> StorageResult<Vec<u8>> {
    let (records, lifecycles, queues) = canonical_v5_parts(save)?;
    let mut payload_length = Identity::default().to_bytes().len();
    for body in &records {
        let lifecycle = lifecycles
            .iter()
            .find(|candidate| candidate.id == body.id)
            .expect("canonical lifecycles cover every record");
        payload_length += RECORD_LENGTH + 1 + 8;
        if !lifecycle.active {
            payload_length += Identity::default().to_bytes().len();
            continue;
        }
        payload_length += 8 + Identity::default().to_bytes().len();
        payload_length += SUMMARY_PREFIX_LENGTH + lifecycle.summary.len();
        if let Some(queue) = queues.iter().find(|candidate| candidate.id == body.id) {
            if queue.has_current {
                payload_length += task_encoded_length(&queue.current);
            }
            if !queue.pending.is_empty() {
                payload_length += fifo_encoded_length(&queue.pending);
            }
        }
    }

    let mut encoded = ByteWriter::new();
    encoded.bytes(&MAGIC);
    encoded.u32(ENVELOPE_VERSION);
    encoded.u32(CURRENT_SCHEMA);
    encoded.u64(save.revision);
    encoded.u32(records.len() as u32);
    encoded.u32(payload_length as u32);
    encoded.u32(0);
    encoded.bytes(&save.agent_namespace_id.to_bytes());
    for body in &records {
        let lifecycle = lifecycles
            .iter()
            .find(|candidate| candidate.id == body.id)
            .expect("canonical lifecycles cover every record");
        append_body(&mut encoded, body);
        let queue = queues.iter().find(|candidate| candidate.id == body.id);
        let mut flags = 0u8;
        if lifecycle.active {
            flags |= FLAG_ACTIVE;
        }
        if lifecycle.active && queue.is_some_and(|queue| queue.has_current) {
            flags |= FLAG_HAS_TASK;
        }
        if lifecycle.active && queue.is_some_and(|queue| !queue.pending.is_empty()) {
            flags |= FLAG_HAS_FIFO;
        }
        encoded.u8(flags);
        encoded.u64(lifecycle.memory_epoch);
        if !lifecycle.active {
            encoded.bytes(&lifecycle.tombstone_operation_id.to_bytes());
            continue;
        }
        encoded.u64(lifecycle.memory_revision);
        encoded.bytes(&lifecycle.memory_operation_id.to_bytes());
        encoded.u16(lifecycle.summary.len() as u16);
        encoded.bytes(lifecycle.summary.as_bytes());
        if flags & FLAG_HAS_TASK != 0 {
            append_task(
                &mut encoded,
                &queue.expect("task flag implies a queue").current,
            );
        }
        if flags & FLAG_HAS_FIFO != 0 {
            append_fifo(
                &mut encoded,
                &queue.expect("FIFO flag implies a queue").pending,
            );
        }
    }
    if encoded.len() != HEADER_LENGTH + payload_length || encoded.len() > MAX_FILE_LENGTH {
        return Err(corrupt(
            "companion file length",
            format!("{} exceeds limit {MAX_FILE_LENGTH}", encoded.len()),
        ));
    }
    let mut bytes = encoded.into_vec();
    let checksum = crc32c_join(&[&bytes[8..28], &bytes[HEADER_LENGTH..]]);
    bytes[28..32].copy_from_slice(&checksum.to_le_bytes());
    Ok(bytes)
}

/// Decodes a companion aggregate, migrating v1..v4 read-only.
///
/// The file-length gate runs before any parsing or allocation.
pub fn decode(data: &[u8]) -> StorageResult<StoredCompanions> {
    if data.len() > MAX_FILE_LENGTH {
        return Err(corrupt(
            "companion file length",
            format!("{} exceeds limit {MAX_FILE_LENGTH}", data.len()),
        ));
    }
    let mut reader = ByteReader::new(data);
    let magic = reader
        .array::<4>()
        .map_err(|detail| corrupt("companion envelope magic", detail))?;
    if magic != MAGIC {
        return Err(corrupt("companion envelope magic", "unexpected magic"));
    }
    let version = reader
        .u32()
        .map_err(|detail| corrupt("companion envelope version", detail))?;
    if version != ENVELOPE_VERSION {
        if version > ENVELOPE_VERSION {
            return Err(future_version("companion envelope version", version));
        }
        return Err(corrupt(
            "companion envelope version",
            format!("unsupported version {version}"),
        ));
    }
    let schema = reader
        .u32()
        .map_err(|detail| corrupt("companion schema", detail))?;
    if !schema_readable(schema) {
        // Rejection still classifies against the current schema: anything
        // newer is a clear future-version signal, while membership in the
        // literal read whitelist stays independent of the current value.
        if schema > CURRENT_SCHEMA {
            return Err(future_version("companion schema", schema));
        }
        return Err(corrupt(
            "companion schema",
            format!("unsupported schema {schema}"),
        ));
    }
    let revision = reader
        .u64()
        .map_err(|detail| corrupt("companion revision", detail))?;
    if revision == 0 {
        return Err(corrupt("companion revision", "zero revision"));
    }
    let count = reader
        .u32()
        .map_err(|detail| corrupt("companion count", detail))?;
    if count as usize > MAX_STORED {
        return Err(corrupt(
            "companion count",
            format!("{count} exceeds limit {MAX_STORED}"),
        ));
    }
    let payload_length = reader
        .u32()
        .map_err(|detail| corrupt("companion payload length", detail))?;
    // v1 is a fixed-stride layout; v2 onward varies per record and only bounds
    // the total payload.
    if schema == SCHEMA_V1 && payload_length as usize != count as usize * RECORD_LENGTH {
        return Err(corrupt("companion payload length", "does not match count"));
    }
    let want_crc = reader
        .u32()
        .map_err(|detail| corrupt("companion CRC32C", detail))?;
    if reader.remaining() != payload_length as usize {
        return Err(corrupt("companion payload length", "does not match file"));
    }
    if crc32c_join(&[&data[8..28], &data[HEADER_LENGTH..]]) != want_crc {
        return Err(corrupt("companion CRC32C", "checksum mismatch"));
    }
    if schema == CURRENT_SCHEMA {
        decode_v5_payload(&mut reader, count as usize, revision)
    } else {
        decode_legacy_payload(&mut reader, schema, count as usize, revision)
    }
}

/// Reports whether `schema` is inside the literal read whitelist v1..v5.
pub fn schema_readable(schema: u32) -> bool {
    matches!(
        schema,
        SCHEMA_V1 | SCHEMA_V2 | SCHEMA_V3 | SCHEMA_V4 | CURRENT_SCHEMA
    )
}

fn decode_legacy_payload(
    reader: &mut ByteReader<'_>,
    schema: u32,
    count: usize,
    revision: u64,
) -> StorageResult<StoredCompanions> {
    let mut records: Vec<CompanionBody> = Vec::with_capacity(count);
    let mut queues: Vec<StoredCompanionQueue> = Vec::new();
    for index in 0..count {
        let body = decode_body(reader)
            .map_err(|detail| corrupt("companion record", format!("{index}: {detail}")))?;
        if index > 0 && records[index - 1].id.to_bytes() >= body.id.to_bytes() {
            return Err(corrupt("companion records", "IDs are not strictly sorted"));
        }
        if schema == SCHEMA_V1 {
            records.push(body);
            continue;
        }
        let mut queue = decode_queue_sections(reader, schema)
            .map_err(|detail| corrupt("companion record", format!("{index}: {detail}")))?;
        // The queue section has no owner field. Bind it to this body before
        // deciding whether the section is empty, so a later consumer cannot
        // attach the work to the wrong record by queue order.
        queue.id = body.id;
        if queue.has_current || !queue.pending.is_empty() || !queue.summary.is_empty() {
            queues.push(queue);
        }
        records.push(body);
    }
    if reader.remaining() != 0 {
        return Err(corrupt(
            "companion payload",
            format!("{} trailing bytes", reader.remaining()),
        ));
    }
    Ok(StoredCompanions {
        source_schema: schema,
        revision,
        agent_namespace_id: Identity::default(),
        records,
        lifecycles: Vec::new(),
        queues,
    })
}

fn decode_v5_payload(
    reader: &mut ByteReader<'_>,
    count: usize,
    revision: u64,
) -> StorageResult<StoredCompanions> {
    let namespace_bytes = reader
        .array::<16>()
        .map_err(|detail| corrupt("companion agent namespace", detail))?;
    let namespace = Identity::from_bytes(namespace_bytes);
    if !namespace.is_valid() {
        return Err(corrupt(
            "companion agent namespace",
            "not a canonical UUIDv4",
        ));
    }

    let mut records: Vec<CompanionBody> = Vec::with_capacity(count);
    let mut lifecycles: Vec<StoredCompanionLifecycle> = Vec::with_capacity(count);
    let mut queues: Vec<StoredCompanionQueue> = Vec::new();
    let mut active_count = 0usize;
    for index in 0..count {
        let body = decode_body(reader)
            .map_err(|detail| corrupt("companion record", format!("{index}: {detail}")))?;
        if index > 0 && records[index - 1].id.to_bytes() >= body.id.to_bytes() {
            return Err(corrupt("companion records", "IDs are not strictly sorted"));
        }
        let flags = reader
            .u8()
            .map_err(|detail| corrupt("companion record flags", format!("{index}: {detail}")))?;
        if flags & !(FLAG_ACTIVE | FLAG_HAS_TASK | FLAG_HAS_FIFO) != 0 {
            return Err(corrupt(
                "companion record flags",
                format!("{flags:#x} reserved"),
            ));
        }
        let epoch = reader
            .u64()
            .map_err(|detail| corrupt("companion memory epoch", format!("{index}: {detail}")))?;
        let mut lifecycle = StoredCompanionLifecycle {
            id: body.id,
            active: flags & FLAG_ACTIVE != 0,
            memory_epoch: epoch,
            memory_revision: 0,
            memory_operation_id: Identity::default(),
            summary: String::new(),
            tombstone_operation_id: Identity::default(),
        };
        if lifecycle.active {
            active_count += 1;
            if active_count > MAX_ACTIVE {
                return Err(corrupt(
                    "companion active count",
                    format!("{active_count} exceeds limit {MAX_ACTIVE}"),
                ));
            }
        }
        if !lifecycle.active {
            if flags != 0 {
                return Err(corrupt(
                    "companion record flags",
                    format!("inactive record carries {flags:#x}"),
                ));
            }
            let identity = decode_identity(reader, "tombstone operation")
                .map_err(|detail| corrupt("companion record", format!("{index}: {detail}")))?;
            lifecycle.tombstone_operation_id = identity;
        } else {
            lifecycle.memory_revision = reader.u64().map_err(|detail| {
                corrupt("companion memory revision", format!("{index}: {detail}"))
            })?;
            let operation_bytes = reader.array::<16>().map_err(|detail| {
                corrupt("companion memory operation", format!("{index}: {detail}"))
            })?;
            lifecycle.memory_operation_id = Identity::from_bytes(operation_bytes);
            let summary_length = reader.u16().map_err(|detail| {
                corrupt(
                    "companion memory summary length",
                    format!("{index}: {detail}"),
                )
            })? as usize;
            if summary_length > MAX_SUMMARY_BYTES {
                return Err(corrupt(
                    "companion memory summary length",
                    format!("{summary_length} exceeds limit {MAX_SUMMARY_BYTES}"),
                ));
            }
            let summary_bytes = reader.take_bytes(summary_length).map_err(|detail| {
                corrupt("companion memory summary", format!("{index}: {detail}"))
            })?;
            lifecycle.summary = String::from_utf8(summary_bytes.to_vec())
                .map_err(|_| corrupt("companion memory summary", "not valid UTF-8"))?;
            let mut queue = StoredCompanionQueue::default();
            if flags & FLAG_HAS_TASK != 0 {
                queue.current = decode_task(reader, CURRENT_SCHEMA)
                    .map_err(|detail| corrupt("companion record", format!("{index}: {detail}")))?;
                queue.has_current = true;
            }
            if flags & FLAG_HAS_FIFO != 0 {
                queue.pending = decode_fifo(reader)
                    .map_err(|detail| corrupt("companion record", format!("{index}: {detail}")))?;
            }
            if queue.has_current || !queue.pending.is_empty() {
                queue.id = body.id;
                queues.push(queue);
            }
        }
        validate_v5_lifecycle(&lifecycle)
            .map_err(|detail| corrupt("companion record", format!("{index}: {detail}")))?;
        records.push(body);
        lifecycles.push(lifecycle);
    }
    if reader.remaining() != 0 {
        return Err(corrupt(
            "companion payload",
            format!("{} trailing bytes", reader.remaining()),
        ));
    }
    // The decoded aggregate must round-trip through the canonical encoder, so
    // a file that decodes can always be rewritten without silent repair.
    canonical_v5_parts(&CompanionSave {
        revision,
        agent_namespace_id: namespace,
        records: records.clone(),
        lifecycles: lifecycles.clone(),
        queues: queues.clone(),
    })?;
    Ok(StoredCompanions {
        source_schema: CURRENT_SCHEMA,
        revision,
        agent_namespace_id: namespace,
        records,
        lifecycles,
        queues,
    })
}

fn decode_identity(reader: &mut ByteReader<'_>, field: &str) -> Result<Identity, String> {
    let bytes = reader
        .array::<16>()
        .map_err(|detail| format!("companion {field}: {detail}"))?;
    let identity = Identity::from_bytes(bytes);
    if !identity.is_valid() {
        return Err(format!("invalid companion {field}"));
    }
    Ok(identity)
}

/// Decodes the trailing flags and optional task, FIFO, and summary sections of
/// a v2+ record. A zero flag byte yields an empty payload; reserved bits are
/// corruption. The summary bit is only legal from v4 onward.
fn decode_queue_sections(
    reader: &mut ByteReader<'_>,
    schema: u32,
) -> Result<StoredCompanionQueue, String> {
    let flags = reader
        .u8()
        .map_err(|detail| format!("companion record flags: {detail}"))?;
    let mut allowed = LEGACY_FLAG_HAS_TASK | LEGACY_FLAG_HAS_FIFO;
    if schema >= SCHEMA_V4 {
        allowed |= LEGACY_FLAG_HAS_SUMMARY;
    }
    if flags & !allowed != 0 {
        return Err(format!("companion record flags {flags:#x} reserved"));
    }
    let mut queue = StoredCompanionQueue::default();
    if flags & LEGACY_FLAG_HAS_TASK != 0 {
        queue.current = decode_task(reader, schema)?;
        queue.has_current = true;
    }
    if flags & LEGACY_FLAG_HAS_FIFO != 0 {
        queue.pending = decode_fifo(reader)?;
    }
    if flags & LEGACY_FLAG_HAS_SUMMARY != 0 {
        queue.summary = decode_summary(reader)?;
    }
    Ok(queue)
}

/// Decodes a summary section: a u16 length prefix plus the text bytes.
///
/// An over-long, zero-length, NUL-bearing, or non-UTF-8 summary is corruption:
/// the summary is model-produced free text and the persistence layer never
/// guesses, cleans, or truncates it.
fn decode_summary(reader: &mut ByteReader<'_>) -> Result<String, String> {
    let length = reader
        .u16()
        .map_err(|detail| format!("companion summary length: {detail}"))? as usize;
    if length > MAX_SUMMARY_BYTES {
        return Err(format!(
            "companion summary length {length} exceeds limit {MAX_SUMMARY_BYTES}"
        ));
    }
    if length == 0 {
        return Err("companion summary section without text".to_owned());
    }
    let text = reader
        .take_bytes(length)
        .map_err(|detail| format!("companion summary: {detail}"))?;
    if !is_utf8(text) {
        return Err("companion summary is not valid UTF-8".to_owned());
    }
    if text.contains(&0u8) {
        return Err("companion summary contains NUL".to_owned());
    }
    Ok(String::from_utf8(text.to_vec()).expect("summary bytes validated as UTF-8"))
}

fn is_utf8(bytes: &[u8]) -> bool {
    std::str::from_utf8(bytes).is_ok()
}

fn task_encoded_length(task: &StoredCompanionTask) -> usize {
    let mut length = TASK_COMMAND_PREFIX_LENGTH + task.command.len() + 2 + 4 + 1 + 1 + 8 + 8;
    for step in &task.plan_steps {
        length += plan_step_wire_length(step.kind);
    }
    length
}

/// Byte length of a step in the v3 variable-length layout: `go_to`/`mine` are
/// 13 bytes, `place` appends a u16 block, and `follow` carries a 16-byte
/// target player ID instead of coordinates. Unknown kinds cannot pass payload
/// validation; the defensive maximum keeps the length budget from understating
/// an unencodable step.
fn plan_step_wire_length(kind: u8) -> usize {
    match kind {
        PLAN_STEP_GO_TO | PLAN_STEP_MINE => PLAN_STEP_LENGTH,
        PLAN_STEP_PLACE => PLAN_STEP_LENGTH + 2,
        _ => 1 + 16,
    }
}

fn fifo_encoded_length(pending: &[String]) -> usize {
    let mut length = 2;
    for command in pending {
        length += TASK_COMMAND_PREFIX_LENGTH + command.len();
    }
    length
}

fn append_task(encoded: &mut ByteWriter, task: &StoredCompanionTask) {
    encoded.u16(task.command.len() as u16);
    encoded.bytes(task.command.as_bytes());
    encoded.u16(task.plan_steps.len() as u16);
    for step in &task.plan_steps {
        append_plan_step(encoded, step);
    }
    encoded.u32(task.step_index as u32);
    encoded.u8(task.state);
    encoded.u8(task.fail_reason);
    encoded.u64(task.start_tick);
    encoded.u64(task.deadline_ticks);
}

fn append_plan_step(encoded: &mut ByteWriter, step: &PlanStep) {
    encoded.u8(step.kind);
    if step.kind == PLAN_STEP_FOLLOW {
        encoded.bytes(&step.player_id.to_bytes());
        return;
    }
    encoded.u32(step.x as u32);
    encoded.u32(step.y as u32);
    encoded.u32(step.z as u32);
    if step.kind == PLAN_STEP_PLACE {
        encoded.u16(step.block);
    }
}

fn decode_task(reader: &mut ByteReader<'_>, schema: u32) -> Result<StoredCompanionTask, String> {
    let command_length = reader
        .u16()
        .map_err(|detail| format!("companion task command length: {detail}"))?
        as usize;
    let command_bytes = reader
        .take_bytes(command_length)
        .map_err(|detail| format!("companion task command: {detail}"))?;
    let command = String::from_utf8(command_bytes.to_vec())
        .map_err(|_| "companion task command: not valid UTF-8".to_owned())?;
    let step_count = reader
        .u16()
        .map_err(|detail| format!("companion task plan length: {detail}"))?
        as usize;
    if step_count > MAX_PLAN_STEPS {
        return Err(format!(
            "companion task plan steps {step_count} exceeds limit {MAX_PLAN_STEPS}"
        ));
    }
    let mut plan_steps = Vec::with_capacity(step_count);
    for index in 0..step_count {
        let step = if schema == SCHEMA_V2 {
            decode_plan_step_fixed(reader)
        } else {
            decode_plan_step_v3(reader)
        }
        .map_err(|detail| format!("companion task plan step {index}: {detail}"))?;
        plan_steps.push(step);
    }
    let step_index = reader
        .u32()
        .map_err(|detail| format!("companion task step index: {detail}"))?
        as i32;
    let state = reader
        .u8()
        .map_err(|detail| format!("companion task state: {detail}"))?;
    let fail_reason = reader
        .u8()
        .map_err(|detail| format!("companion task fail reason: {detail}"))?;
    let start_tick = reader
        .u64()
        .map_err(|detail| format!("companion task start tick: {detail}"))?;
    let deadline_ticks = reader
        .u64()
        .map_err(|detail| format!("companion task deadline: {detail}"))?;
    let task = StoredCompanionTask {
        command,
        plan_steps,
        step_index,
        state,
        start_tick,
        deadline_ticks,
        fail_reason,
    };
    validate_task(&task, schema)?;
    Ok(task)
}

/// Decodes a v2 fixed 13-byte step (kind + three int32 coordinates). The v2
/// encoder only ever wrote `go_to`; any other kind byte is rejected by payload
/// validation, so no kind dispatch happens here.
fn decode_plan_step_fixed(reader: &mut ByteReader<'_>) -> Result<PlanStep, String> {
    let kind = reader
        .u8()
        .map_err(|detail| format!("companion plan step kind: {detail}"))?;
    let x = decode_step_coord(reader, "X")?;
    let y = decode_step_coord(reader, "Y")?;
    let z = decode_step_coord(reader, "Z")?;
    Ok(PlanStep {
        kind,
        x,
        y,
        z,
        block: 0,
        player_id: PlayerId::default(),
    })
}

/// Decodes a v3 variable-length step: the kind comes first, then the
/// kind-specific payload. An illegal kind fails immediately rather than
/// guessing a stride, which would misread every later field.
fn decode_plan_step_v3(reader: &mut ByteReader<'_>) -> Result<PlanStep, String> {
    let kind = reader
        .u8()
        .map_err(|detail| format!("companion plan step kind: {detail}"))?;
    let mut step = PlanStep {
        kind,
        ..PlanStep::default()
    };
    match kind {
        PLAN_STEP_GO_TO | PLAN_STEP_MINE | PLAN_STEP_PLACE => {
            step.x = decode_step_coord(reader, "X")?;
            step.y = decode_step_coord(reader, "Y")?;
            step.z = decode_step_coord(reader, "Z")?;
            if kind == PLAN_STEP_PLACE {
                step.block = reader
                    .u16()
                    .map_err(|detail| format!("companion plan step block: {detail}"))?;
            }
        }
        PLAN_STEP_FOLLOW => {
            let player_id = reader
                .array::<16>()
                .map_err(|detail| format!("companion plan step player ID: {detail}"))?;
            step.player_id = PlayerId::from_bytes(player_id);
        }
        _ => return Err(format!("companion plan step kind {kind} is not delivered")),
    }
    Ok(step)
}

fn decode_step_coord(reader: &mut ByteReader<'_>, field: &str) -> Result<i32, String> {
    reader
        .u32()
        .map(|value| value as i32)
        .map_err(|detail| format!("companion plan step {field}: {detail}"))
}

fn append_fifo(encoded: &mut ByteWriter, pending: &[String]) {
    encoded.u16(pending.len() as u16);
    for command in pending {
        encoded.u16(command.len() as u16);
        encoded.bytes(command.as_bytes());
    }
}

fn decode_fifo(reader: &mut ByteReader<'_>) -> Result<Vec<String>, String> {
    let count = reader
        .u16()
        .map_err(|detail| format!("companion FIFO count: {detail}"))? as usize;
    if count > MAX_FIFO_ENTRIES {
        return Err(format!(
            "companion FIFO depth {count} exceeds limit {MAX_FIFO_ENTRIES}"
        ));
    }
    if count == 0 {
        return Err("companion FIFO section without entries".to_owned());
    }
    let mut pending = Vec::with_capacity(count);
    for index in 0..count {
        let length = reader
            .u16()
            .map_err(|detail| format!("companion FIFO entry length: {detail}"))?
            as usize;
        let entry = reader
            .take_bytes(length)
            .map_err(|detail| format!("companion FIFO entry: {detail}"))?;
        let command = String::from_utf8(entry.to_vec())
            .map_err(|_| format!("companion FIFO entry {index}: not valid UTF-8"))?;
        validate_plan_text(&command, MAX_TASK_COMMAND_BYTES, true)
            .map_err(|detail| format!("companion FIFO entry {index}: {detail}"))?;
        pending.push(command);
    }
    Ok(pending)
}

fn append_body(encoded: &mut ByteWriter, body: &CompanionBody) {
    encoded.bytes(&body.id.to_bytes());
    encoded.u32(body.dimension as u32);
    for value in body.position {
        encoded.f32(value);
    }
    encoded.f32(body.yaw);
    encoded.f32(body.pitch);
    encoded.u8(body.inventory.hotbar.selected);
    for stack in &body.inventory.hotbar.slots {
        append_stack(encoded, stack);
    }
    for stack in &body.inventory.backpack {
        append_stack(encoded, stack);
    }
}

fn append_stack(encoded: &mut ByteWriter, stack: &ItemStack) {
    encoded.u16(stack.item);
    encoded.u8(stack.count);
    encoded.u16(stack.durability);
}

fn decode_body(reader: &mut ByteReader<'_>) -> Result<CompanionBody, String> {
    let id_bytes = reader
        .array::<16>()
        .map_err(|detail| format!("companion ID: {detail}"))?;
    let id = PlayerId::from_bytes(id_bytes);
    let dimension = reader
        .u32()
        .map_err(|detail| format!("companion dimension: {detail}"))? as i32;
    let mut position = [0f32; 3];
    for value in &mut position {
        *value = reader
            .f32()
            .map_err(|detail| format!("companion position: {detail}"))?;
    }
    let yaw = reader
        .f32()
        .map_err(|detail| format!("companion yaw: {detail}"))?;
    let pitch = reader
        .f32()
        .map_err(|detail| format!("companion pitch: {detail}"))?;
    let mut inventory = Inventory::default();
    inventory.hotbar.selected = reader
        .u8()
        .map_err(|detail| format!("companion selected slot: {detail}"))?;
    for slot in &mut inventory.hotbar.slots {
        *slot =
            decode_stack(reader).map_err(|detail| format!("companion hotbar slot: {detail}"))?;
    }
    for slot in &mut inventory.backpack {
        *slot =
            decode_stack(reader).map_err(|detail| format!("companion backpack slot: {detail}"))?;
    }
    let body = CompanionBody {
        id,
        dimension,
        position,
        yaw,
        pitch,
        inventory,
    };
    validate_body(&body)?;
    Ok(body)
}

fn decode_stack(reader: &mut ByteReader<'_>) -> Result<ItemStack, String> {
    let item = reader
        .u16()
        .map_err(|detail| format!("companion item: {detail}"))?;
    let count = reader
        .u8()
        .map_err(|detail| format!("companion count: {detail}"))?;
    let durability = reader
        .u16()
        .map_err(|detail| format!("companion durability: {detail}"))?;
    Ok(ItemStack {
        item,
        count,
        durability,
    })
}

fn validate_body(body: &CompanionBody) -> Result<(), String> {
    if !body.id.is_valid() {
        return Err("invalid companion ID".to_owned());
    }
    if body.dimension != OVERWORLD {
        return Err(format!(
            "unsupported companion dimension {}",
            body.dimension
        ));
    }
    for value in body.position {
        if !value.is_finite() {
            return Err("non-finite companion position".to_owned());
        }
    }
    if !body.yaw.is_finite() {
        return Err("non-finite companion yaw".to_owned());
    }
    if !body.pitch.is_finite() || body.pitch < -core_half_pi() || body.pitch > core_half_pi() {
        return Err("invalid companion pitch".to_owned());
    }
    if !body.inventory.is_valid() {
        return Err("invalid companion inventory".to_owned());
    }
    Ok(())
}

fn core_half_pi() -> f32 {
    std::f32::consts::FRAC_PI_2
}

/// Validates one task payload: enum and pairing rules, command bounds, plan
/// structure, and the "plan only persists while running" field coupling.
///
/// The schema only decides which step kinds exist; every other invariant is
/// schema independent, so encoding and decoding share this gate.
fn validate_task(task: &StoredCompanionTask, schema: u32) -> Result<(), String> {
    if !(TASK_QUEUED..=TASK_STOPPED).contains(&task.state) {
        return Err(format!("companion task state {} outside enum", task.state));
    }
    if task.state == TASK_FAILED {
        if !(TASK_FAIL_PLANNER_UNAVAILABLE..=TASK_FAIL_INVENTORY_FULL).contains(&task.fail_reason) {
            return Err(format!(
                "companion task fail reason {} invalid",
                task.fail_reason
            ));
        }
    } else if task.fail_reason != TASK_FAIL_NONE {
        return Err(format!(
            "companion task fail reason {} without failed state",
            task.fail_reason
        ));
    }
    validate_plan_text(&task.command, MAX_TASK_COMMAND_BYTES, true)
        .map_err(|detail| format!("companion task command: {detail}"))?;
    if task.state == TASK_RUNNING {
        if task.plan_steps.is_empty() {
            return Err("running companion task has no plan steps".to_owned());
        }
        if task.step_index < 0 || task.step_index as usize >= task.plan_steps.len() {
            return Err(format!(
                "companion task step index {} outside plan",
                task.step_index
            ));
        }
    } else if !task.plan_steps.is_empty()
        || task.step_index != 0
        || task.start_tick != 0
        || task.deadline_ticks != 0
    {
        return Err("companion task keeps plan progress outside running state".to_owned());
    }
    if task.plan_steps.len() > MAX_PLAN_STEPS {
        return Err(format!(
            "companion task plan steps {} exceeds limit {MAX_PLAN_STEPS}",
            task.plan_steps.len()
        ));
    }
    let mut has_follow = false;
    for (index, step) in task.plan_steps.iter().enumerate() {
        validate_plan_step(step, index, task.plan_steps.len(), schema)?;
        if step.kind == PLAN_STEP_FOLLOW {
            has_follow = true;
        }
    }
    // A sustained follow never persists a deadline: a zero deadline is the
    // runtime timeout exemption, and a nonzero one would wrongly re-arm the
    // timer after a restart. v2 payloads carry no follow steps, so this gate
    // does not affect v2 migration.
    if has_follow && task.deadline_ticks != 0 {
        return Err(format!(
            "companion follow task keeps deadline {}",
            task.deadline_ticks
        ));
    }
    Ok(())
}

/// Validates one plan step's structure.
///
/// v2 only ever wrote `go_to`; any other kind is impossible v2 bytes and is
/// rejected as corruption. v3 validates the full delivered kind set: the
/// coordinate steps must keep Y inside the world, a follow target must be a
/// valid UUIDv4 and only the last step, and every unused field must be zero so
/// the variable-length encoding never silently drops a value.
fn validate_plan_step(
    step: &PlanStep,
    index: usize,
    total: usize,
    schema: u32,
) -> Result<(), String> {
    if schema == SCHEMA_V2 {
        if step.kind != PLAN_STEP_GO_TO {
            return Err(format!(
                "companion task plan step {index} kind {} is not go_to",
                step.kind
            ));
        }
        if step.y < MIN_Y || step.y >= MAX_Y {
            return Err(format!(
                "companion task plan step {index} Y={} outside world",
                step.y
            ));
        }
        return Ok(());
    }
    match step.kind {
        PLAN_STEP_GO_TO | PLAN_STEP_MINE => {
            if step.block != 0 || !step.player_id.is_zero() {
                return Err(format!(
                    "companion task plan step {index} keeps unused payload"
                ));
            }
        }
        PLAN_STEP_PLACE => {
            if !step.player_id.is_zero() {
                return Err(format!(
                    "companion task plan step {index} keeps unused player payload"
                ));
            }
        }
        PLAN_STEP_FOLLOW => {
            if step.x != 0 || step.y != 0 || step.z != 0 || step.block != 0 {
                return Err(format!(
                    "companion task plan step {index} keeps unused coordinate payload"
                ));
            }
            if !step.player_id.is_valid() {
                return Err(format!(
                    "companion task plan step {index} follow target invalid"
                ));
            }
            if index != total - 1 {
                return Err(format!(
                    "companion task plan step {index} follow is not last"
                ));
            }
        }
        _ => {
            return Err(format!(
                "companion task plan step {index} kind {} is not delivered",
                step.kind
            ));
        }
    }
    if step.kind != PLAN_STEP_FOLLOW && (step.y < MIN_Y || step.y >= MAX_Y) {
        return Err(format!(
            "companion task plan step {index} Y={} outside world",
            step.y
        ));
    }
    Ok(())
}

/// Validates a set of queue payloads: non-empty, unique IDs, each referencing
/// an existing record, and every task and FIFO entry bounded.
fn validate_queues(
    queues: &[StoredCompanionQueue],
    records: &[CompanionBody],
    schema: u32,
) -> Result<(), String> {
    let mut seen: Vec<PlayerId> = Vec::with_capacity(queues.len());
    for (index, queue) in queues.iter().enumerate() {
        validate_queue(queue, schema)
            .map_err(|detail| format!("companion queue {index}: {detail}"))?;
        if seen.contains(&queue.id) {
            return Err("duplicate companion queue ID".to_owned());
        }
        if !records.iter().any(|body| body.id == queue.id) {
            return Err("companion queue without body record".to_owned());
        }
        seen.push(queue.id);
    }
    Ok(())
}

/// Validates one queue payload.
///
/// When `has_current` is false the current task does not reach the disk, so it
/// must be entirely zero: a nonzero value cannot be expressed on disk and
/// accepting it would silently drop data.
fn validate_queue(queue: &StoredCompanionQueue, schema: u32) -> Result<(), String> {
    if !queue.has_current && queue.pending.is_empty() && queue.summary.is_empty() {
        return Err("empty companion queue".to_owned());
    }
    if !queue.id.is_valid() {
        return Err("invalid companion queue ID".to_owned());
    }
    if queue.has_current {
        validate_task(&queue.current, schema)?;
    } else if !task_is_zero(&queue.current) {
        return Err("companion queue keeps current task without HasCurrent".to_owned());
    }
    if queue.pending.len() > MAX_FIFO_ENTRIES {
        return Err(format!(
            "companion FIFO depth {} exceeds limit {MAX_FIFO_ENTRIES}",
            queue.pending.len()
        ));
    }
    for (index, command) in queue.pending.iter().enumerate() {
        validate_plan_text(command, MAX_TASK_COMMAND_BYTES, true)
            .map_err(|detail| format!("companion FIFO entry {index}: {detail}"))?;
    }
    validate_summary(&queue.summary)
}

/// Validates the persisted summary boundary: at most
/// [`MAX_SUMMARY_BYTES`] bytes, valid UTF-8, and no NUL. An empty string is
/// legal and means "no summary"; the save boundary imposes no non-empty rule.
fn validate_summary(summary: &str) -> Result<(), String> {
    if summary.len() > MAX_SUMMARY_BYTES {
        return Err(format!(
            "companion summary {} bytes exceeds limit {MAX_SUMMARY_BYTES}",
            summary.len()
        ));
    }
    if !is_utf8(summary.as_bytes()) {
        return Err("companion summary is not valid UTF-8".to_owned());
    }
    if summary.contains('\0') {
        return Err("companion summary contains NUL".to_owned());
    }
    Ok(())
}

/// Reports whether a task payload is entirely zero.
fn task_is_zero(task: &StoredCompanionTask) -> bool {
    task.command.is_empty()
        && task.plan_steps.is_empty()
        && task.step_index == 0
        && task.state == 0
        && task.start_tick == 0
        && task.deadline_ticks == 0
        && task.fail_reason == TASK_FAIL_NONE
}

/// Validates one v5 lifecycle record.
fn validate_v5_lifecycle(lifecycle: &StoredCompanionLifecycle) -> Result<(), String> {
    if !lifecycle.id.is_valid() {
        return Err("invalid companion lifecycle ID".to_owned());
    }
    if lifecycle.memory_epoch == 0 {
        return Err("zero companion memory epoch".to_owned());
    }
    if lifecycle.active {
        if !lifecycle.tombstone_operation_id.is_zero() {
            return Err("active companion carries tombstone".to_owned());
        }
        if lifecycle.memory_revision == 0 {
            if !lifecycle.memory_operation_id.is_zero() || !lifecycle.summary.is_empty() {
                return Err("invalid canonical-zero companion memory".to_owned());
            }
            return Ok(());
        }
        if !lifecycle.memory_operation_id.is_valid() {
            return Err("invalid companion memory operation".to_owned());
        }
        validate_summary(&lifecycle.summary)?;
        return Ok(());
    }
    if lifecycle.memory_revision != 0
        || !lifecycle.memory_operation_id.is_zero()
        || !lifecycle.summary.is_empty()
    {
        return Err("inactive companion carries memory mirror".to_owned());
    }
    if !lifecycle.tombstone_operation_id.is_valid() {
        return Err("invalid companion tombstone operation".to_owned());
    }
    Ok(())
}

/// Canonicalizes a save request into sorted, validated parts.
fn canonical_v5_parts(
    save: &CompanionSave,
) -> StorageResult<(
    Vec<CompanionBody>,
    Vec<StoredCompanionLifecycle>,
    Vec<StoredCompanionQueue>,
)> {
    if save.revision == 0 {
        return Err(corrupt("companion revision", "zero revision"));
    }
    if !save.agent_namespace_id.is_valid() {
        return Err(corrupt(
            "companion agent namespace",
            "not a canonical UUIDv4",
        ));
    }
    // Count gates run before any clone or per-record scan. A 65-body request
    // must report the count even when the first body is also invalid, and an
    // oversized queue list must not be copied in order to discover that.
    if save.records.len() > MAX_STORED {
        return Err(corrupt(
            "companion count",
            format!("{} exceeds limit {MAX_STORED}", save.records.len()),
        ));
    }
    if save.lifecycles.len() != save.records.len() {
        return Err(corrupt(
            "companion lifecycles",
            "set does not match records",
        ));
    }
    if save.queues.len() > MAX_ACTIVE {
        return Err(corrupt(
            "companion queues",
            format!("{} exceeds limit {MAX_ACTIVE}", save.queues.len()),
        ));
    }
    let mut records = save.records.clone();
    records.sort_by_key(|left| left.id.to_bytes());
    for (index, body) in records.iter().enumerate() {
        validate_body(body)
            .map_err(|detail| corrupt("companion record", format!("{index}: {detail}")))?;
        if index > 0 && records[index - 1].id == body.id {
            return Err(corrupt("companion records", "duplicate companion ID"));
        }
    }
    if save.lifecycles.len() != records.len() {
        return Err(corrupt(
            "companion lifecycles",
            "set does not match records",
        ));
    }
    let mut lifecycles = save.lifecycles.clone();
    lifecycles.sort_by_key(|left| left.id.to_bytes());
    let mut active: Vec<PlayerId> = Vec::new();
    for (index, lifecycle) in lifecycles.iter().enumerate() {
        if index > 0 && lifecycles[index - 1].id == lifecycle.id {
            return Err(corrupt("companion lifecycles", "duplicate ID"));
        }
        if lifecycle.id != records[index].id {
            return Err(corrupt(
                "companion lifecycles",
                "set does not match records",
            ));
        }
        validate_v5_lifecycle(lifecycle)
            .map_err(|detail| corrupt("companion lifecycle", format!("{index}: {detail}")))?;
        if lifecycle.active {
            active.push(lifecycle.id);
        }
    }
    if active.len() > MAX_ACTIVE {
        return Err(corrupt(
            "companion active count",
            format!("{} exceeds limit {MAX_ACTIVE}", active.len()),
        ));
    }
    validate_queues(&save.queues, &records, CURRENT_SCHEMA)
        .map_err(|detail| corrupt("companion queues", detail))?;
    let mut queues = save.queues.clone();
    queues.sort_by_key(|left| left.id.to_bytes());
    for queue in &queues {
        if !queue.summary.is_empty() {
            return Err(corrupt(
                "companion queues",
                "v5 queue carries legacy summary",
            ));
        }
        if !active.contains(&queue.id) {
            return Err(corrupt(
                "companion queues",
                "inactive companion carries task or FIFO",
            ));
        }
    }
    Ok((records, lifecycles, queues))
}

/// Validates bounded plan text: valid UTF-8, no control characters, within the
/// byte ceiling, and non-blank when required.
fn validate_plan_text(
    value: &str,
    max_bytes: usize,
    require_non_empty: bool,
) -> Result<(), String> {
    if !is_utf8(value.as_bytes()) {
        return Err("not valid UTF-8".to_owned());
    }
    if value.len() > max_bytes {
        return Err(format!("{} bytes exceeds limit {max_bytes}", value.len()));
    }
    if value.chars().any(char::is_control) {
        return Err("contains a control character".to_owned());
    }
    if require_non_empty && value.trim().is_empty() {
        return Err("is empty".to_owned());
    }
    Ok(())
}
