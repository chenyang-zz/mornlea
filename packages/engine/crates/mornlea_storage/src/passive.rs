//! `save.passive`: the passive-mob aggregate save (`passive_mobs.bin`).
//!
//! Ported from the Go `passive` storage codec. The envelope carries a fixed
//! 32-byte header followed by fixed-stride 72-byte records, so any length
//! mismatch is corruption rather than a layout variant. Records are written in
//! strictly ascending ID order and the CRC-32C covers the identity fields plus
//! the payload, never the magic or the checksum bytes themselves.

use crate::bytes::{ByteReader, SliceWriter, is_zero};
use crate::crc32c::crc32c_join;
use crate::error::{StorageError, StorageResult, corrupt, future_version};

/// Maximum number of records one passive-mob save may hold.
pub const MAX_PASSIVE_MOBS: usize = 32;

/// Envelope version written by the current encoder.
pub const ENVELOPE_VERSION: u32 = 1;

/// Schema written by the current encoder; older schemas are read-only input.
pub const CURRENT_SCHEMA: u32 = 1;

const HEADER_LENGTH: usize = 32;
const RECORD_LENGTH: usize = 72;
const RESERVED_LENGTH: usize = 30;
/// Physical byte ceiling for the aggregate file.
pub const MAX_FILE_LENGTH: usize = HEADER_LENGTH + MAX_PASSIVE_MOBS * RECORD_LENGTH;

const MAGIC: [u8; 4] = *b"PMST";
const OVERWORLD: i32 = 0;
const MIN_Y: f32 = -64.0;
const MAX_Y: f32 = 320.0;
const MAX_HEALTH: u8 = 20;

/// One persisted passive mob. Runtime-derived facts (flee timer, birth chunk,
/// newborn flag) are deliberately absent: they are re-derived on restore.
#[derive(Clone, Debug, PartialEq)]
pub struct PassiveMob {
    pub id: u64,
    pub dimension: i32,
    pub position: [f32; 3],
    pub velocity: [f32; 3],
    pub on_ground: bool,
    pub yaw: f32,
    pub health: u8,
}

/// A decoded passive-mob aggregate.
#[derive(Clone, Debug, PartialEq)]
pub struct PassiveMobs {
    pub revision: u64,
    pub records: Vec<PassiveMob>,
}

/// A passive-mob aggregate save request. Record order is irrelevant; the
/// encoder always emits canonical ascending-ID order.
#[derive(Clone, Debug, PartialEq)]
pub struct PassiveMobsSave {
    pub revision: u64,
    pub records: Vec<PassiveMob>,
}

/// Reports the exact on-disk byte length for a valid passive-mob aggregate.
pub fn passive_mobs_encoded_len(save: &PassiveMobsSave) -> StorageResult<usize> {
    let sorted_indices = prepare_encode_indices(save)?;
    let count = sorted_indices.len();
    let payload_length = count
        .checked_mul(RECORD_LENGTH)
        .ok_or_else(|| corrupt("passive payload length", "overflow"))?;
    let total = HEADER_LENGTH
        .checked_add(payload_length)
        .ok_or_else(|| corrupt("passive file length", "overflow"))?;
    if total > MAX_FILE_LENGTH {
        return Err(corrupt(
            "passive file length",
            format!("{total} exceeds limit {MAX_FILE_LENGTH}"),
        ));
    }
    Ok(total)
}

/// Writes a complete passive-mob aggregate into `dst`, preserving any tail
/// beyond the returned length. Validation and capacity checks precede the
/// first write, so failure publishes no bytes.
pub fn encode_passive_mobs_into(save: &PassiveMobsSave, dst: &mut [u8]) -> StorageResult<usize> {
    let sorted_indices = prepare_encode_indices(save)?;
    let needed = passive_mobs_encoded_len(save)?;
    if dst.len() < needed {
        return Err(StorageError::OutputTooSmall {
            needed,
            available: dst.len(),
        });
    }
    let count = sorted_indices.len();
    let payload_length = count * RECORD_LENGTH;
    const CRC_OFFSET: usize = 28;
    {
        let mut writer = SliceWriter::new(&mut dst[..needed]);
        writer.bytes(&MAGIC);
        writer.u32(ENVELOPE_VERSION);
        writer.u32(CURRENT_SCHEMA);
        writer.u64(save.revision);
        writer.u32(count as u32);
        writer.u32(payload_length as u32);
        writer.u32(0);
        debug_assert_eq!(writer.pos(), HEADER_LENGTH);
    }
    {
        let mut writer = SliceWriter::new(&mut dst[HEADER_LENGTH..needed]);
        for index in &sorted_indices {
            append_record_slice(&mut writer, &save.records[*index]);
        }
        debug_assert_eq!(writer.pos(), payload_length);
    }
    let checksum = crc32c_join(&[&dst[8..CRC_OFFSET], &dst[HEADER_LENGTH..needed]]);
    SliceWriter::new(&mut dst[..needed]).patch_u32(CRC_OFFSET, checksum);
    Ok(needed)
}

/// Encodes a passive-mob aggregate into its canonical on-disk form.
///
/// The caller's `records` slice is never mutated: emission order is canonical
/// ascending ID only.
pub fn encode(save: &PassiveMobsSave) -> StorageResult<Vec<u8>> {
    let needed = passive_mobs_encoded_len(save)?;
    let mut bytes = vec![0u8; needed];
    encode_passive_mobs_into(save, &mut bytes)?;
    Ok(bytes)
}

/// Validates count and records, then returns indices sorted by passive ID.
fn prepare_encode_indices(save: &PassiveMobsSave) -> StorageResult<Vec<usize>> {
    if save.revision == 0 {
        return Err(corrupt("passive revision", "zero revision"));
    }
    if save.records.len() > MAX_PASSIVE_MOBS {
        return Err(corrupt(
            "passive count",
            format!("{} exceeds limit {MAX_PASSIVE_MOBS}", save.records.len()),
        ));
    }
    for (index, record) in save.records.iter().enumerate() {
        validate_record(record)
            .map_err(|detail| corrupt("passive record", format!("{index}: {detail}")))?;
    }
    let mut sorted_indices: Vec<usize> = Vec::new();
    sorted_indices
        .try_reserve_exact(save.records.len())
        .map_err(|_| corrupt("passive count", "index reservation failed"))?;
    sorted_indices.extend(0..save.records.len());
    sorted_indices.sort_by_key(|&index| save.records[index].id);
    for index in 1..sorted_indices.len() {
        let prev = save.records[sorted_indices[index - 1]].id;
        let current = save.records[sorted_indices[index]].id;
        if prev == current {
            return Err(corrupt("passive records", "duplicate passive ID"));
        }
    }
    Ok(sorted_indices)
}

/// Decodes a passive-mob aggregate. File length, count, and payload length are
/// checked before any record is parsed or allocated.
pub fn decode(data: &[u8]) -> StorageResult<PassiveMobs> {
    if data.len() > MAX_FILE_LENGTH {
        return Err(corrupt(
            "passive file length",
            format!("{} exceeds limit {MAX_FILE_LENGTH}", data.len()),
        ));
    }
    let mut reader = ByteReader::new(data);
    let magic = reader
        .array::<4>()
        .map_err(|detail| corrupt("passive envelope magic", detail))?;
    if magic != MAGIC {
        return Err(corrupt("passive envelope magic", "unexpected magic"));
    }
    let version = reader
        .u32()
        .map_err(|detail| corrupt("passive envelope version", detail))?;
    if version != ENVELOPE_VERSION {
        if version > ENVELOPE_VERSION {
            return Err(future_version("passive envelope version", version));
        }
        return Err(corrupt(
            "passive envelope version",
            format!("unsupported version {version}"),
        ));
    }
    let schema = reader
        .u32()
        .map_err(|detail| corrupt("passive schema", detail))?;
    if schema != CURRENT_SCHEMA {
        if schema > CURRENT_SCHEMA {
            return Err(future_version("passive schema", schema));
        }
        return Err(corrupt(
            "passive schema",
            format!("unsupported schema {schema}"),
        ));
    }
    let revision = reader
        .u64()
        .map_err(|detail| corrupt("passive revision", detail))?;
    if revision == 0 {
        return Err(corrupt("passive revision", "zero revision"));
    }
    let count = reader
        .u32()
        .map_err(|detail| corrupt("passive count", detail))?;
    if count as usize > MAX_PASSIVE_MOBS {
        return Err(corrupt(
            "passive count",
            format!("{count} exceeds limit {MAX_PASSIVE_MOBS}"),
        ));
    }
    let payload_length = reader
        .u32()
        .map_err(|detail| corrupt("passive payload length", detail))?;
    if payload_length as usize != count as usize * RECORD_LENGTH {
        return Err(corrupt("passive payload length", "does not match count"));
    }
    let want_crc = reader
        .u32()
        .map_err(|detail| corrupt("passive CRC32C", detail))?;
    if reader.remaining() != payload_length as usize {
        return Err(corrupt("passive payload length", "does not match file"));
    }
    let mut records: Vec<PassiveMob> = Vec::with_capacity(count as usize);
    for index in 0..count as usize {
        let record = decode_record(&mut reader)
            .map_err(|detail| corrupt("passive record", format!("{index}: {detail}")))?;
        if index > 0 && records[index - 1].id >= record.id {
            return Err(corrupt("passive records", "IDs are not strictly sorted"));
        }
        records.push(record);
    }
    if reader.remaining() != 0 {
        return Err(corrupt(
            "passive payload",
            format!("{} trailing bytes", reader.remaining()),
        ));
    }
    if crc32c_join(&[&data[8..28], &data[HEADER_LENGTH..]]) != want_crc {
        return Err(corrupt("passive CRC32C", "checksum mismatch"));
    }
    Ok(PassiveMobs { revision, records })
}

fn validate_record(record: &PassiveMob) -> Result<(), String> {
    if record.id == 0 {
        return Err("zero passive ID".to_owned());
    }
    if record.dimension != OVERWORLD {
        return Err(format!(
            "unsupported passive dimension {}",
            record.dimension
        ));
    }
    for value in record.position {
        if !value.is_finite() {
            return Err("non-finite passive position".to_owned());
        }
    }
    if record.position[1] < MIN_Y || record.position[1] >= MAX_Y {
        return Err(format!(
            "passive position Y {} outside world",
            record.position[1]
        ));
    }
    for value in record.velocity {
        if !value.is_finite() {
            return Err("non-finite passive velocity".to_owned());
        }
    }
    if !record.yaw.is_finite() {
        return Err("non-finite passive yaw".to_owned());
    }
    if record.health == 0 || record.health > MAX_HEALTH {
        return Err(format!(
            "passive health {} outside 1..{MAX_HEALTH}",
            record.health
        ));
    }
    Ok(())
}

fn append_record_slice(writer: &mut SliceWriter<'_>, record: &PassiveMob) {
    writer.u64(record.id);
    writer.u32(record.dimension as u32);
    for value in record.position {
        writer.f32(value);
    }
    for value in record.velocity {
        writer.f32(value);
    }
    writer.u8(u8::from(record.on_ground));
    writer.f32(record.yaw);
    writer.u8(record.health);
    writer.zeroes(RESERVED_LENGTH);
}

fn decode_record(reader: &mut ByteReader<'_>) -> Result<PassiveMob, String> {
    let id = reader
        .u64()
        .map_err(|detail| format!("passive ID: {detail}"))?;
    let dimension = reader
        .u32()
        .map_err(|detail| format!("passive dimension: {detail}"))? as i32;
    let mut position = [0f32; 3];
    for value in &mut position {
        *value = reader
            .f32()
            .map_err(|detail| format!("passive position: {detail}"))?;
    }
    let mut velocity = [0f32; 3];
    for value in &mut velocity {
        *value = reader
            .f32()
            .map_err(|detail| format!("passive velocity: {detail}"))?;
    }
    let on_ground = reader
        .u8()
        .map_err(|detail| format!("passive onGround: {detail}"))?;
    if on_ground > 1 {
        return Err(format!("passive onGround {on_ground} is not a bool"));
    }
    let yaw = reader
        .f32()
        .map_err(|detail| format!("passive yaw: {detail}"))?;
    let health = reader
        .u8()
        .map_err(|detail| format!("passive health: {detail}"))?;
    let reserved = reader
        .array::<RESERVED_LENGTH>()
        .map_err(|detail| format!("passive reserved: {detail}"))?;
    if !is_zero(&reserved) {
        return Err("passive reserved bytes are not zero".to_owned());
    }
    let record = PassiveMob {
        id,
        dimension,
        position,
        velocity,
        on_ground: on_ground == 1,
        yaw,
        health,
    };
    validate_record(&record)?;
    Ok(record)
}
