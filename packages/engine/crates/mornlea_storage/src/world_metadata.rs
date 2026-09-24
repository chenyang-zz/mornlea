//! `save.world-metadata`: the world metadata record (`world.meta`).
//!
//! Ported from the Go `storage` metadata codec. Every version is a pure tail
//! append over v1, so a legacy file keeps its byte layout and simply reads
//! missing fields as their documented migration defaults. Decoding normalizes
//! to the current version; encoding only ever writes the current version.

use crate::bytes::SliceWriter;
use crate::crc32c::crc32c;
use crate::error::{StorageError, StorageResult, corrupt, future_version};

/// Schema written by the current encoder.
pub const CURRENT_VERSION: u32 = 6;

/// Legacy schemas still accepted for read-only migration.
pub const V1: u32 = 1;
pub const V2: u32 = 2;
pub const V3: u32 = 3;
pub const V4: u32 = 4;
pub const V5: u32 = 5;

const HEADER_LENGTH: usize = 12;
const CHECKSUM_LENGTH: usize = 4;
const PAYLOAD_LENGTH: usize = 62;
const V5_PAYLOAD_LENGTH: usize = 61;
const V4_PAYLOAD_LENGTH: usize = 41;
const V3_PAYLOAD_LENGTH: usize = 36;
const V2_PAYLOAD_LENGTH: usize = 28;
const V1_PAYLOAD_LENGTH: usize = 20;
/// Fixed dimension-table size persisted from v5 onward: overworld plus depths.
const DIMENSION_COUNT: u32 = 2;
/// Seed salt used when a legacy file carries no dimension table.
const DEPTHS_SEED_SALT: u64 = 0x9E37_79B9_7F4A_7C15;
/// Exact on-disk byte length for a valid current-schema metadata record.
const RECORD_LENGTH: usize = HEADER_LENGTH + PAYLOAD_LENGTH + CHECKSUM_LENGTH;

const MAGIC: [u8; 4] = *b"MCGM";
const WEATHER_CLEAR: u8 = 0;
const WEATHER_RAIN: u8 = 1;
const WEATHER_THUNDER: u8 = 2;
const DIFFICULTY_NORMAL: u8 = 0;
const DIFFICULTY_PEACEFUL: u8 = 1;
const DIFFICULTY_HARD: u8 = 2;

/// A chunk coordinate on a per-dimension grid.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ChunkPos {
    pub x: i32,
    pub z: i32,
}

/// A world metadata record.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Metadata {
    /// Always the schema the record normalized to after decoding.
    pub format_version: u32,
    pub seed: i64,
    pub spawn_dimension: i32,
    pub spawn_anchor: ChunkPos,
    /// Absolute authoritative world time, persisted from v2.
    pub world_time_ticks: u64,
    /// Display phase offset, persisted from v3.
    pub day_phase_offset: u64,
    /// Weather kind byte persisted from v4; the codec preserves any raw value.
    pub weather_kind: u8,
    /// Remaining ticks of the current weather segment, persisted from v4.
    pub weather_ticks_remaining: u32,
    /// Second-dimension spawn anchor, persisted from v5.
    pub depths_spawn_anchor: ChunkPos,
    /// Second-dimension terrain seed salt, persisted from v5.
    pub depths_seed_salt: u64,
    /// `0` normal, `1` peaceful, `2` hard; persisted from v6.
    pub difficulty: u8,
}

fn validate_encode(metadata: &Metadata) -> StorageResult<()> {
    if metadata.format_version > CURRENT_VERSION {
        return Err(future_version("metadata version", metadata.format_version));
    }
    if metadata.format_version != CURRENT_VERSION {
        return Err(corrupt(
            "metadata version",
            format!("unsupported version {}", metadata.format_version),
        ));
    }
    if !valid_difficulty(metadata.difficulty) {
        return Err(corrupt(
            "metadata difficulty",
            format!("{}", metadata.difficulty),
        ));
    }
    Ok(())
}

/// Reports the exact on-disk byte length for a valid current-schema metadata record.
pub fn encoded_len(metadata: &Metadata) -> StorageResult<usize> {
    validate_encode(metadata)?;
    Ok(RECORD_LENGTH)
}

/// Writes a complete current-schema metadata record into `dst`, preserving any
/// tail beyond the returned length.
pub fn encode_into(metadata: &Metadata, dst: &mut [u8]) -> StorageResult<usize> {
    validate_encode(metadata)?;
    if dst.len() < RECORD_LENGTH {
        return Err(StorageError::OutputTooSmall {
            needed: RECORD_LENGTH,
            available: dst.len(),
        });
    }
    let checksum_offset = {
        let mut writer = SliceWriter::new(&mut dst[..RECORD_LENGTH]);
        writer.bytes(&MAGIC);
        writer.u32(metadata.format_version);
        writer.u32(PAYLOAD_LENGTH as u32);
        writer.u64(metadata.seed as u64);
        writer.u32(metadata.spawn_dimension as u32);
        writer.u32(metadata.spawn_anchor.x as u32);
        writer.u32(metadata.spawn_anchor.z as u32);
        writer.u64(metadata.world_time_ticks);
        writer.u64(metadata.day_phase_offset);
        writer.u8(metadata.weather_kind);
        writer.u32(metadata.weather_ticks_remaining);
        writer.u32(DIMENSION_COUNT);
        writer.u32(metadata.depths_spawn_anchor.x as u32);
        writer.u32(metadata.depths_spawn_anchor.z as u32);
        writer.u64(metadata.depths_seed_salt);
        writer.u8(metadata.difficulty);
        debug_assert_eq!(writer.pos(), RECORD_LENGTH - CHECKSUM_LENGTH);
        writer.pos()
    };
    let checksum = crc32c(&dst[..checksum_offset]);
    dst[checksum_offset..checksum_offset + CHECKSUM_LENGTH]
        .copy_from_slice(&checksum.to_le_bytes());
    Ok(RECORD_LENGTH)
}

/// Encodes `metadata` as the current-schema world metadata record.
///
/// Only the current version may be written: re-encoding a legacy record at its
/// old version would freeze the migration gap onto disk.
pub fn encode(metadata: &Metadata) -> StorageResult<Vec<u8>> {
    let needed = encoded_len(metadata)?;
    let mut bytes = vec![0u8; needed];
    encode_into(metadata, &mut bytes)?;
    Ok(bytes)
}

/// Decodes a world metadata record and normalizes it to the current version.
///
/// File length, payload length, and checksum are all verified before any field
/// is read, so a truncated file never yields a half-parsed record.
pub fn decode(encoded: &[u8]) -> StorageResult<Metadata> {
    if encoded.len() < HEADER_LENGTH {
        return Err(corrupt("metadata header", "short"));
    }
    if encoded[0..4] != MAGIC {
        return Err(corrupt("metadata magic", "unexpected magic"));
    }
    let version = u32_at(encoded, 4);
    if version > CURRENT_VERSION {
        return Err(future_version("metadata version", version));
    }
    let want_payload_length = match version {
        CURRENT_VERSION => PAYLOAD_LENGTH,
        V5 => V5_PAYLOAD_LENGTH,
        V4 => V4_PAYLOAD_LENGTH,
        V3 => V3_PAYLOAD_LENGTH,
        V2 => V2_PAYLOAD_LENGTH,
        V1 => V1_PAYLOAD_LENGTH,
        _ => {
            return Err(corrupt(
                "metadata version",
                format!("unsupported version {version}"),
            ));
        }
    };
    let payload_length = u32_at(encoded, 8);
    if payload_length as usize != want_payload_length {
        return Err(corrupt(
            "metadata payload length",
            format!("{payload_length}, want {want_payload_length}"),
        ));
    }
    let want_length = HEADER_LENGTH + payload_length as usize + CHECKSUM_LENGTH;
    if encoded.len() != want_length {
        return Err(corrupt(
            "metadata length",
            format!("{}, want {want_length}", encoded.len()),
        ));
    }
    let checksum_offset = want_length - CHECKSUM_LENGTH;
    if crc32c(&encoded[..checksum_offset]) != u32_at(encoded, checksum_offset) {
        return Err(corrupt("metadata CRC32C", "checksum mismatch"));
    }

    let payload = &encoded[HEADER_LENGTH..checksum_offset];
    let mut metadata = Metadata {
        format_version: CURRENT_VERSION,
        seed: i64::from_le_bytes(payload[0..8].try_into().expect("eight bytes")),
        spawn_dimension: u32_at(payload, 8) as i32,
        spawn_anchor: ChunkPos {
            x: u32_at(payload, 12) as i32,
            z: u32_at(payload, 16) as i32,
        },
        world_time_ticks: 0,
        day_phase_offset: 0,
        weather_kind: WEATHER_CLEAR,
        weather_ticks_remaining: 0,
        depths_spawn_anchor: ChunkPos { x: 0, z: 0 },
        depths_seed_salt: DEPTHS_SEED_SALT,
        difficulty: DIFFICULTY_NORMAL,
    };
    // World time is persisted from v2, the day phase offset from v3, weather
    // from v4, the dimension table from v5, and difficulty from v6. Missing
    // tails keep their documented migration defaults: zero time, clear weather
    // with zero remaining ticks, the overworld anchor and the fixed seed salt
    // for depths, and normal difficulty.
    if version >= V2 {
        metadata.world_time_ticks = u64_at(payload, 20);
    }
    if version >= V3 {
        metadata.day_phase_offset = u64_at(payload, 28);
    }
    if version >= V4 {
        metadata.weather_kind = payload[36];
        metadata.weather_ticks_remaining = u32_at(payload, 37);
    }
    if version >= V5 {
        let dimension_count = u32_at(payload, 41);
        if dimension_count != DIMENSION_COUNT {
            return Err(corrupt(
                "metadata dimension count",
                format!("{dimension_count}"),
            ));
        }
        metadata.depths_spawn_anchor = ChunkPos {
            x: u32_at(payload, 45) as i32,
            z: u32_at(payload, 49) as i32,
        };
        metadata.depths_seed_salt = u64_at(payload, 53);
    } else {
        metadata.depths_spawn_anchor = metadata.spawn_anchor;
    }
    if version == CURRENT_VERSION {
        metadata.difficulty = payload[61];
        if !valid_difficulty(metadata.difficulty) {
            return Err(corrupt(
                "metadata difficulty",
                format!("{}", metadata.difficulty),
            ));
        }
    }
    Ok(metadata)
}

/// Reports whether `kind` is one of the three persisted weather states.
pub const fn valid_weather(kind: u8) -> bool {
    matches!(kind, WEATHER_CLEAR | WEATHER_RAIN | WEATHER_THUNDER)
}

/// Reports whether `difficulty` is one of the three persisted difficulties.
pub const fn valid_difficulty(difficulty: u8) -> bool {
    matches!(
        difficulty,
        DIFFICULTY_NORMAL | DIFFICULTY_PEACEFUL | DIFFICULTY_HARD
    )
}

fn u32_at(encoded: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes(encoded[offset..offset + 4].try_into().expect("four bytes"))
}

fn u64_at(encoded: &[u8], offset: usize) -> u64 {
    u64::from_le_bytes(encoded[offset..offset + 8].try_into().expect("eight bytes"))
}
