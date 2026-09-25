//! `save.hostile`: the hostile-mob aggregate save (`hostile_mobs.bin`).
//!
//! Ported from the Go `hostile` storage codec. The decoder accepts schema v1
//! and v2; only v2 is written. v1 records lack the trailing `kind` byte, so a
//! v1 file is read in as a nightcrawler and stays byte-identical until the
//! next normal save rewrites it as v2.

use crate::bytes::{ByteReader, SliceWriter};
use crate::crc32c::crc32c_join;
use crate::error::{StorageError, StorageResult, corrupt, future_version};
use crate::identity::PlayerId;

/// Maximum number of records one hostile-mob save may hold.
pub const MAX_HOSTILE_MOBS: usize = 64;

/// Envelope version written by the current encoder.
pub const ENVELOPE_VERSION: u32 = 1;

/// Legacy schema still accepted for read-only migration.
pub const SCHEMA_V1: u32 = 1;

/// Schema written by the current encoder.
pub const CURRENT_SCHEMA: u32 = 2;

const HEADER_LENGTH: usize = 32;
const RECORD_LENGTH_V1: usize = 72;
const RECORD_LENGTH_V2: usize = 73;
const RECORD_LENGTH: usize = RECORD_LENGTH_V2;
/// Physical byte ceiling for the aggregate file.
pub const MAX_FILE_LENGTH: usize = HEADER_LENGTH + MAX_HOSTILE_MOBS * RECORD_LENGTH;

const MAGIC: [u8; 4] = *b"MHST";
const OVERWORLD: i32 = 0;
const MIN_Y: f32 = -64.0;
const MAX_Y: f32 = 320.0;
const MAX_HEALTH: u8 = 20;
const COOLDOWN_PERIOD_TICKS: u8 = 20;
const MAX_DISTANT_TICKS: u16 = 600;

/// One persisted hostile mob. Path and planning generation are deliberately
/// absent: they are runtime-derived and re-planned on the first tick.
#[derive(Clone, Debug, PartialEq)]
pub struct HostileMob {
    pub id: u64,
    pub dimension: i32,
    pub position: [f32; 3],
    pub velocity: [f32; 3],
    pub on_ground: bool,
    pub yaw: f32,
    pub health: u8,
    pub attack_cooldown: u8,
    pub hurt_cooldown: u8,
    pub burn_cooldown: u8,
    pub has_target: bool,
    pub player_id: PlayerId,
    pub next_repath_ticks: u64,
    pub distant_ticks: u16,
    /// `0` is a nightcrawler, `1` a bone thrower; v1 records always migrate to
    /// `0`.
    pub kind: u8,
}

/// A decoded hostile-mob aggregate.
#[derive(Clone, Debug, PartialEq)]
pub struct HostileMobs {
    pub revision: u64,
    pub records: Vec<HostileMob>,
}

/// A hostile-mob aggregate save request. Record order is irrelevant; the
/// encoder always emits canonical ascending-ID order.
#[derive(Clone, Debug, PartialEq)]
pub struct HostileMobsSave {
    pub revision: u64,
    pub records: Vec<HostileMob>,
}

/// Reports the exact on-disk byte length for a valid hostile-mob aggregate.
pub fn hostile_mobs_encoded_len(save: &HostileMobsSave) -> StorageResult<usize> {
    let sorted_indices = prepare_encode_indices(save)?;
    let count = sorted_indices.len();
    let payload_length = count
        .checked_mul(RECORD_LENGTH)
        .ok_or_else(|| corrupt("hostile payload length", "overflow"))?;
    let total = HEADER_LENGTH
        .checked_add(payload_length)
        .ok_or_else(|| corrupt("hostile file length", "overflow"))?;
    if total > MAX_FILE_LENGTH {
        return Err(corrupt(
            "hostile file length",
            format!("{total} exceeds limit {MAX_FILE_LENGTH}"),
        ));
    }
    Ok(total)
}

/// Writes a complete hostile-mob aggregate into `dst`, preserving any tail
/// beyond the returned length. Validation and capacity checks precede the
/// first write, so failure publishes no bytes.
pub fn encode_hostile_mobs_into(save: &HostileMobsSave, dst: &mut [u8]) -> StorageResult<usize> {
    let sorted_indices = prepare_encode_indices(save)?;
    let needed = hostile_mobs_encoded_len(save)?;
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
    dst[CRC_OFFSET..CRC_OFFSET + 4].copy_from_slice(&checksum.to_le_bytes());
    Ok(needed)
}

/// Encodes a hostile-mob aggregate into its canonical v2 on-disk form.
///
/// The caller's `records` slice is never mutated: emission order is canonical
/// ascending ID only.
pub fn encode(save: &HostileMobsSave) -> StorageResult<Vec<u8>> {
    let needed = hostile_mobs_encoded_len(save)?;
    let mut bytes = vec![0u8; needed];
    encode_hostile_mobs_into(save, &mut bytes)?;
    Ok(bytes)
}

/// Validates count and records, then returns indices sorted by hostile ID.
fn prepare_encode_indices(save: &HostileMobsSave) -> StorageResult<Vec<usize>> {
    if save.revision == 0 {
        return Err(corrupt("hostile revision", "zero revision"));
    }
    if save.records.len() > MAX_HOSTILE_MOBS {
        return Err(corrupt(
            "hostile count",
            format!("{} exceeds limit {MAX_HOSTILE_MOBS}", save.records.len()),
        ));
    }
    for (index, record) in save.records.iter().enumerate() {
        validate_record_for_encode(record)
            .map_err(|detail| corrupt("hostile record", format!("{index}: {detail}")))?;
    }
    let mut sorted_indices: Vec<usize> = Vec::new();
    sorted_indices
        .try_reserve_exact(save.records.len())
        .map_err(|_| corrupt("hostile count", "index reservation failed"))?;
    sorted_indices.extend(0..save.records.len());
    sorted_indices.sort_by_key(|&index| save.records[index].id);
    for index in 1..sorted_indices.len() {
        let prev = save.records[sorted_indices[index - 1]].id;
        let current = save.records[sorted_indices[index]].id;
        if prev == current {
            return Err(corrupt("hostile records", "duplicate hostile ID"));
        }
    }
    Ok(sorted_indices)
}

/// Decodes a hostile-mob aggregate. File length, count, and payload length are
/// checked before any record is parsed or allocated.
pub fn decode(data: &[u8]) -> StorageResult<HostileMobs> {
    if data.len() > MAX_FILE_LENGTH {
        return Err(corrupt(
            "hostile file length",
            format!("{} exceeds limit {MAX_FILE_LENGTH}", data.len()),
        ));
    }
    let mut reader = ByteReader::new(data);
    let magic = reader
        .array::<4>()
        .map_err(|detail| corrupt("hostile envelope magic", detail))?;
    if magic != MAGIC {
        return Err(corrupt("hostile envelope magic", "unexpected magic"));
    }
    let version = reader
        .u32()
        .map_err(|detail| corrupt("hostile envelope version", detail))?;
    if version != ENVELOPE_VERSION {
        if version > ENVELOPE_VERSION {
            return Err(future_version("hostile envelope version", version));
        }
        return Err(corrupt(
            "hostile envelope version",
            format!("unsupported version {version}"),
        ));
    }
    let schema = reader
        .u32()
        .map_err(|detail| corrupt("hostile schema", detail))?;
    let record_length = match schema {
        SCHEMA_V1 => RECORD_LENGTH_V1,
        CURRENT_SCHEMA => RECORD_LENGTH_V2,
        _ => {
            if schema > CURRENT_SCHEMA {
                return Err(future_version("hostile schema", schema));
            }
            return Err(corrupt(
                "hostile schema",
                format!("unsupported schema {schema}"),
            ));
        }
    };
    let revision = reader
        .u64()
        .map_err(|detail| corrupt("hostile revision", detail))?;
    if revision == 0 {
        return Err(corrupt("hostile revision", "zero revision"));
    }
    let count = reader
        .u32()
        .map_err(|detail| corrupt("hostile count", detail))?;
    if count as usize > MAX_HOSTILE_MOBS {
        return Err(corrupt(
            "hostile count",
            format!("{count} exceeds limit {MAX_HOSTILE_MOBS}"),
        ));
    }
    let payload_length = reader
        .u32()
        .map_err(|detail| corrupt("hostile payload length", detail))?;
    if payload_length as usize != count as usize * record_length {
        return Err(corrupt("hostile payload length", "does not match count"));
    }
    let want_crc = reader
        .u32()
        .map_err(|detail| corrupt("hostile CRC32C", detail))?;
    if reader.remaining() != payload_length as usize {
        return Err(corrupt("hostile payload length", "does not match file"));
    }
    let mut records: Vec<HostileMob> = Vec::with_capacity(count as usize);
    for index in 0..count as usize {
        let record = decode_record(&mut reader, schema)
            .map_err(|detail| corrupt("hostile record", format!("{index}: {detail}")))?;
        if index > 0 && records[index - 1].id >= record.id {
            return Err(corrupt("hostile records", "IDs are not strictly sorted"));
        }
        records.push(record);
    }
    if reader.remaining() != 0 {
        return Err(corrupt(
            "hostile payload",
            format!("{} trailing bytes", reader.remaining()),
        ));
    }
    if crc32c_join(&[&data[8..28], &data[HEADER_LENGTH..]]) != want_crc {
        return Err(corrupt("hostile CRC32C", "checksum mismatch"));
    }
    Ok(HostileMobs { revision, records })
}

fn validate_record_for_encode(record: &HostileMob) -> Result<(), String> {
    validate_record_fields(record)
}

fn validate_record(record: &HostileMob) -> Result<(), String> {
    validate_record_fields(record)
}

fn validate_record_fields(record: &HostileMob) -> Result<(), String> {
    if record.id == 0 {
        return Err("zero hostile ID".to_owned());
    }
    if record.dimension != OVERWORLD {
        return Err(format!(
            "unsupported hostile dimension {}",
            record.dimension
        ));
    }
    for value in record.position {
        if !value.is_finite() {
            return Err("non-finite hostile position".to_owned());
        }
    }
    if record.position[1] < MIN_Y || record.position[1] >= MAX_Y {
        return Err(format!(
            "hostile position Y {} outside world",
            record.position[1]
        ));
    }
    for value in record.velocity {
        if !value.is_finite() {
            return Err("non-finite hostile velocity".to_owned());
        }
    }
    if !record.yaw.is_finite() {
        return Err("non-finite hostile yaw".to_owned());
    }
    if record.health == 0 || record.health > MAX_HEALTH {
        return Err(format!(
            "hostile health {} outside 1..{MAX_HEALTH}",
            record.health
        ));
    }
    for cooldown in [
        record.attack_cooldown,
        record.hurt_cooldown,
        record.burn_cooldown,
    ] {
        if cooldown > COOLDOWN_PERIOD_TICKS {
            return Err(format!(
                "hostile cooldown {cooldown} exceeds period {COOLDOWN_PERIOD_TICKS}"
            ));
        }
    }
    if record.distant_ticks > MAX_DISTANT_TICKS {
        return Err(format!(
            "hostile distant ticks {} exceeds limit {MAX_DISTANT_TICKS}",
            record.distant_ticks
        ));
    }
    if record.kind > 1 {
        return Err(format!(
            "hostile kind {} outside domain {{0,1}}",
            record.kind
        ));
    }
    if !record.has_target {
        if !record.player_id.is_zero() {
            return Err("hostile without target keeps player ID".to_owned());
        }
        return Ok(());
    }
    if !record.player_id.is_valid() {
        return Err("hostile target is not a valid UUIDv4".to_owned());
    }
    Ok(())
}

fn append_record_slice(writer: &mut SliceWriter<'_>, record: &HostileMob) {
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
    writer.u8(record.attack_cooldown);
    writer.u8(record.hurt_cooldown);
    writer.u8(record.burn_cooldown);
    writer.u8(u8::from(record.has_target));
    writer.bytes(&record.player_id.to_bytes());
    writer.u64(record.next_repath_ticks);
    writer.u16(record.distant_ticks);
    writer.u8(record.kind);
}

fn decode_record(reader: &mut ByteReader<'_>, schema: u32) -> Result<HostileMob, String> {
    let id = reader
        .u64()
        .map_err(|detail| format!("hostile ID: {detail}"))?;
    let dimension = reader
        .u32()
        .map_err(|detail| format!("hostile dimension: {detail}"))? as i32;
    let mut position = [0f32; 3];
    for value in &mut position {
        *value = reader
            .f32()
            .map_err(|detail| format!("hostile position: {detail}"))?;
    }
    let mut velocity = [0f32; 3];
    for value in &mut velocity {
        *value = reader
            .f32()
            .map_err(|detail| format!("hostile velocity: {detail}"))?;
    }
    let on_ground = reader
        .u8()
        .map_err(|detail| format!("hostile onGround: {detail}"))?;
    if on_ground > 1 {
        return Err(format!("hostile onGround {on_ground} is not a bool"));
    }
    let yaw = reader
        .f32()
        .map_err(|detail| format!("hostile yaw: {detail}"))?;
    let health = reader
        .u8()
        .map_err(|detail| format!("hostile health: {detail}"))?;
    let attack_cooldown = reader
        .u8()
        .map_err(|detail| format!("hostile attack cooldown: {detail}"))?;
    let hurt_cooldown = reader
        .u8()
        .map_err(|detail| format!("hostile hurt cooldown: {detail}"))?;
    let burn_cooldown = reader
        .u8()
        .map_err(|detail| format!("hostile burn cooldown: {detail}"))?;
    let has_target = reader
        .u8()
        .map_err(|detail| format!("hostile hasTarget: {detail}"))?;
    if has_target > 1 {
        return Err(format!("hostile hasTarget {has_target} is not a bool"));
    }
    let player_id = PlayerId::from_bytes(
        reader
            .array::<16>()
            .map_err(|detail| format!("hostile player ID: {detail}"))?,
    );
    let next_repath_ticks = reader
        .u64()
        .map_err(|detail| format!("hostile next repath: {detail}"))?;
    let distant_ticks = reader
        .u16()
        .map_err(|detail| format!("hostile distant ticks: {detail}"))?;
    // Only v2 records carry the trailing kind byte; a v1 record ends here and
    // the zero value is exactly the "nightcrawler" migration semantics.
    let kind = if schema == CURRENT_SCHEMA {
        reader
            .u8()
            .map_err(|detail| format!("hostile kind: {detail}"))?
    } else {
        0
    };
    let record = HostileMob {
        id,
        dimension,
        position,
        velocity,
        on_ground: on_ground == 1,
        yaw,
        health,
        attack_cooldown,
        hurt_cooldown,
        burn_cooldown,
        has_target: has_target == 1,
        player_id,
        next_repath_ticks,
        distant_ticks,
        kind,
    };
    validate_record(&record)?;
    Ok(record)
}
