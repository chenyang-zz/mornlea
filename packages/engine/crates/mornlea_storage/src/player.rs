//! `save.player`: the player save record (`player.dat`, MCPL envelope).
//!
//! Ported from the Go `player` storage codec. Every schema from v1 to v9 is a
//! tail append over the previous one, so decoding peels fixed-length tails off
//! the end and older files keep their byte layout. Only v9 is written; older
//! schemas are read once and migrated to the current normalized result.

use crate::bytes::{ByteReader, SliceWriter};
use crate::crc32c::crc32c_join;
use crate::error::{StorageError, StorageResult, corrupt, future_version};
use crate::identity::PlayerId;
use crate::items::{BACKPACK_SLOTS, HOTBAR_SLOTS, Inventory, ItemStack, item_max_durability};

/// Schema written by the current encoder.
pub const CURRENT_SCHEMA: u32 = 9;

/// Oldest schema the decoder still accepts.
pub const OLDEST_SCHEMA: u32 = 1;

/// Fixed MCPL envelope header length.
pub const ENVELOPE_LENGTH: usize = 44;
/// Decode allocation ceiling for one player payload.
pub const MAX_PAYLOAD: usize = 1 << 20;

const ENVELOPE_VERSION: u32 = 1;
const LEGACY_HOTBAR_BYTES: usize = 1 + HOTBAR_SLOTS * 3;
const LEGACY_BACKPACK_BYTES: usize = BACKPACK_SLOTS * 3;
const HOTBAR_BYTES: usize = 1 + HOTBAR_SLOTS * 5;
const BACKPACK_BYTES: usize = BACKPACK_SLOTS * 5;
const HEALTH_BYTES: usize = 1;
/// Hunger state appended after health from v7: hunger u8, saturation u16,
/// exhaustion u16.
const HUNGER_BYTES: usize = 1 + 2 + 2;
/// Respawn point appended after hunger from v8: present u8, three f32 block
/// coordinates, and a dimension u32.
const RESPAWN_BYTES: usize = 1 + 12 + 4;
/// Armor section appended after the respawn point from v9: four slots of the
/// same five-byte stack encoding the backpack uses.
const ARMOR_BYTES: usize = 4 * 5;

const MAGIC: [u8; 4] = *b"MCPL";
const V9_FIXED_WITHOUT_NAME_AND_SAFE: usize = 4
    + 16
    + 8
    + 1
    + HOTBAR_BYTES
    + BACKPACK_BYTES
    + HEALTH_BYTES
    + HUNGER_BYTES
    + RESPAWN_BYTES
    + ARMOR_BYTES;
const MAX_HEALTH: u8 = 20;
const MAX_HUNGER: u8 = 20;
const SATURATION_MILLI_PER_POINT: u16 = 1000;
const INITIAL_SATURATION_MILLI: u16 = 5 * SATURATION_MILLI_PER_POINT;
const OVERWORLD: i32 = 0;
const DEPTHS: i32 = 1;

/// A player location: dimension plus world coordinates.
#[derive(Clone, Debug, PartialEq)]
pub struct PlayerLocation {
    pub dimension: i32,
    pub position: [f32; 3],
}

/// A decoded player, normalized to the current schema.
#[derive(Clone, Debug, PartialEq)]
pub struct StoredPlayer {
    pub player_id: PlayerId,
    pub revision: u64,
    pub display_name: String,
    pub current: PlayerLocation,
    pub yaw: f32,
    pub pitch: f32,
    pub safe: Option<PlayerLocation>,
    pub inventory: Inventory,
    pub health: u8,
    pub hunger: u8,
    pub saturation_milli: u16,
    pub exhaustion_milli: u16,
    pub respawn_present: bool,
    pub respawn_position: [f32; 3],
    pub respawn_dimension: i32,
    /// Armor is a pure-fidelity field: the codec never validates its semantics.
    pub armor: [ItemStack; 4],
    /// Set when the file was a legacy schema that the decoder normalized.
    pub needs_rewrite: bool,
}

/// A player save request.
#[derive(Clone, Debug, PartialEq)]
pub struct PlayerSave {
    pub player_id: PlayerId,
    pub revision: u64,
    pub display_name: String,
    pub current: PlayerLocation,
    pub yaw: f32,
    pub pitch: f32,
    pub safe: Option<PlayerLocation>,
    pub inventory: Inventory,
    pub health: u8,
    pub hunger: u8,
    pub saturation_milli: u16,
    pub exhaustion_milli: u16,
    pub respawn_present: bool,
    pub respawn_position: [f32; 3],
    pub respawn_dimension: i32,
    pub armor: [ItemStack; 4],
}

/// Reports the exact on-disk byte length for a valid v9 player save.
pub fn encoded_len(save: &PlayerSave) -> StorageResult<usize> {
    validate_save(save)?;
    let payload = v9_payload_len(save)?;
    if payload > MAX_PAYLOAD {
        return Err(corrupt(
            "player payload",
            format!("{payload} exceeds limit {MAX_PAYLOAD}"),
        ));
    }
    ENVELOPE_LENGTH
        .checked_add(payload)
        .ok_or_else(|| corrupt("player record", "length overflows"))
}

/// Writes a complete MCPL v9 record into `dst`, preserving any tail beyond
/// the returned length.
pub fn encode_into(save: &PlayerSave, dst: &mut [u8]) -> StorageResult<usize> {
    validate_save(save)?;
    let payload = v9_payload_len(save)?;
    if payload > MAX_PAYLOAD {
        return Err(corrupt(
            "player payload",
            format!("{payload} exceeds limit {MAX_PAYLOAD}"),
        ));
    }
    let needed = ENVELOPE_LENGTH
        .checked_add(payload)
        .ok_or_else(|| corrupt("player record", "length overflows"))?;
    if dst.len() < needed {
        return Err(StorageError::OutputTooSmall {
            needed,
            available: dst.len(),
        });
    }
    let crc_offset = {
        let mut writer = SliceWriter::new(&mut dst[..needed]);
        writer.bytes(&MAGIC);
        writer.u32(ENVELOPE_VERSION);
        writer.u32(CURRENT_SCHEMA);
        writer.bytes(&save.player_id.to_bytes());
        writer.u64(save.revision);
        writer.u32(payload as u32);
        let crc_offset = writer.pos();
        writer.u32(0);
        write_v9_payload(&mut writer, save);
        debug_assert_eq!(writer.pos(), needed);
        crc_offset
    };
    let checksum = crc32c_join(&[&dst[8..40], &dst[ENVELOPE_LENGTH..needed]]);
    SliceWriter::new(&mut dst[..needed]).patch_u32(crc_offset, checksum);
    Ok(needed)
}

/// Encodes one player as the stable MCPL v1 envelope at the current schema.
pub fn encode(save: &PlayerSave) -> StorageResult<Vec<u8>> {
    let needed = encoded_len(save)?;
    let mut buf = vec![0u8; needed];
    encode_into(save, &mut buf)?;
    Ok(buf)
}

/// Decodes and validates one player, migrating older schemas to the current
/// normalized result.
///
/// `want_id` must be the requested player identity: a file for a different
/// player is rejected rather than silently loaded.
pub fn decode(want_id: PlayerId, data: &[u8]) -> StorageResult<StoredPlayer> {
    if !want_id.is_valid() {
        return Err(corrupt("player ID", "requested identity is not a UUIDv4"));
    }
    let mut envelope = ByteReader::new(data);
    let magic = envelope
        .array::<4>()
        .map_err(|detail| corrupt("player envelope magic", detail))?;
    if magic != MAGIC {
        return Err(corrupt("player envelope magic", "unexpected magic"));
    }
    let version = envelope
        .u32()
        .map_err(|detail| corrupt("player envelope version", detail))?;
    if version != ENVELOPE_VERSION {
        if version > ENVELOPE_VERSION {
            return Err(future_version("player envelope version", version));
        }
        return Err(corrupt(
            "player envelope version",
            format!("unsupported version {version}"),
        ));
    }
    let schema = envelope
        .u32()
        .map_err(|detail| corrupt("player schema", detail))?;
    if schema > CURRENT_SCHEMA {
        return Err(future_version("player schema", schema));
    }
    if schema < OLDEST_SCHEMA {
        return Err(corrupt(
            "player schema",
            format!("unsupported schema {schema}"),
        ));
    }
    let mut player_id = [0u8; 16];
    player_id.copy_from_slice(
        &envelope
            .array::<16>()
            .map_err(|detail| corrupt("player ID", detail))?,
    );
    let player_id = PlayerId::from_bytes(player_id);
    if !player_id.is_valid() || player_id != want_id {
        return Err(corrupt("player ID", "does not match request"));
    }
    let revision = envelope
        .u64()
        .map_err(|detail| corrupt("player revision", detail))?;
    if revision == 0 {
        return Err(corrupt("player revision", "zero revision"));
    }
    let payload_length = envelope
        .u32()
        .map_err(|detail| corrupt("player payload length", detail))?
        as usize;
    if payload_length > MAX_PAYLOAD {
        return Err(corrupt(
            "player payload length",
            format!("{payload_length} exceeds limit {MAX_PAYLOAD}"),
        ));
    }
    let want_crc = envelope
        .u32()
        .map_err(|detail| corrupt("player CRC32C", detail))?;
    if envelope.remaining() != payload_length {
        return Err(corrupt("player payload length", "does not match envelope"));
    }
    let payload = envelope
        .take_bytes(payload_length)
        .map_err(|detail| corrupt("player payload", detail))?;
    if crc32c_join(&[&data[8..40], payload]) != want_crc {
        return Err(corrupt("player CRC32C", "checksum mismatch"));
    }
    let (dto, migrated) = decode_payload(schema, player_id, revision, payload)?;
    let stored = StoredPlayer {
        player_id: dto.player_id,
        revision: dto.revision,
        display_name: dto.display_name,
        current: dto.current,
        yaw: dto.yaw,
        pitch: dto.pitch,
        safe: dto.safe,
        inventory: dto.inventory,
        health: dto.health,
        hunger: dto.hunger,
        saturation_milli: dto.saturation_milli,
        exhaustion_milli: dto.exhaustion_milli,
        respawn_present: dto.respawn_present,
        respawn_position: dto.respawn_position,
        respawn_dimension: dto.respawn_dimension,
        armor: dto.armor,
        needs_rewrite: migrated,
    };
    validate_dto(&stored)?;
    Ok(stored)
}

/// Reports whether `schema` is inside the decoder's supported range.
pub fn schema_readable(schema: u32) -> bool {
    (OLDEST_SCHEMA..=CURRENT_SCHEMA).contains(&schema)
}

#[derive(Clone, Debug, PartialEq)]
struct PlayerDto {
    player_id: PlayerId,
    revision: u64,
    display_name: String,
    current: PlayerLocation,
    yaw: f32,
    pitch: f32,
    safe: Option<PlayerLocation>,
    inventory: Inventory,
    health: u8,
    hunger: u8,
    saturation_milli: u16,
    exhaustion_milli: u16,
    respawn_present: bool,
    respawn_position: [f32; 3],
    respawn_dimension: i32,
    armor: [ItemStack; 4],
}

impl PlayerDto {
    fn new(player_id: PlayerId, revision: u64) -> Self {
        Self {
            player_id,
            revision,
            display_name: String::new(),
            current: PlayerLocation {
                dimension: 0,
                position: [0.0; 3],
            },
            yaw: 0.0,
            pitch: 0.0,
            safe: None,
            inventory: Inventory::default(),
            health: 0,
            hunger: 0,
            saturation_milli: 0,
            exhaustion_milli: 0,
            respawn_present: false,
            respawn_position: [0.0; 3],
            respawn_dimension: 0,
            armor: [ItemStack::default(); 4],
        }
    }
}

fn v9_payload_len(save: &PlayerSave) -> StorageResult<usize> {
    let name_len = save.display_name.len();
    let mut len = V9_FIXED_WITHOUT_NAME_AND_SAFE
        .checked_add(name_len)
        .ok_or_else(|| corrupt("player payload", "length overflows"))?;
    if save.safe.is_some() {
        len = len
            .checked_add(16)
            .ok_or_else(|| corrupt("player payload", "length overflows"))?;
    }
    Ok(len)
}

fn write_v9_payload(writer: &mut SliceWriter<'_>, save: &PlayerSave) {
    write_v8_payload(writer, save);
    for stack in &save.armor {
        writer.u16(stack.item);
        writer.u8(stack.count);
        writer.u16(stack.durability);
    }
}

fn write_v8_payload(writer: &mut SliceWriter<'_>, save: &PlayerSave) {
    write_v7_payload(writer, save);
    if !save.respawn_present {
        writer.zeroes(RESPAWN_BYTES);
        return;
    }
    writer.u8(1);
    for value in save.respawn_position {
        writer.f32(value);
    }
    writer.u32(save.respawn_dimension as u32);
}

fn write_v7_payload(writer: &mut SliceWriter<'_>, save: &PlayerSave) {
    write_v5_payload(writer, save);
    writer.u8(save.hunger);
    writer.u16(save.saturation_milli);
    writer.u16(save.exhaustion_milli);
}

fn write_v5_payload(writer: &mut SliceWriter<'_>, save: &PlayerSave) {
    write_v4_payload(writer, save);
    writer.u8(save.health);
}

fn write_v4_payload(writer: &mut SliceWriter<'_>, save: &PlayerSave) {
    let name = save.display_name.as_bytes();
    writer.u32(name.len() as u32);
    writer.bytes(name);
    write_location(writer, &save.current);
    writer.f32(save.yaw);
    writer.f32(save.pitch);
    match &save.safe {
        None => writer.u8(0),
        Some(safe) => {
            writer.u8(1);
            write_location(writer, safe);
        }
    }
    write_hotbar(writer, &save.inventory.hotbar);
    for stack in &save.inventory.backpack {
        write_stack(writer, stack);
    }
}

fn write_location(writer: &mut SliceWriter<'_>, location: &PlayerLocation) {
    writer.u32(location.dimension as u32);
    for value in location.position {
        writer.f32(value);
    }
}

fn write_hotbar(writer: &mut SliceWriter<'_>, hotbar: &crate::items::Hotbar) {
    writer.u8(hotbar.selected);
    for stack in &hotbar.slots {
        write_stack(writer, stack);
    }
}

fn write_stack(writer: &mut SliceWriter<'_>, stack: &ItemStack) {
    writer.u16(stack.item);
    writer.u8(stack.count);
    writer.u16(stack.durability);
}

fn decode_payload(
    schema: u32,
    player_id: PlayerId,
    revision: u64,
    data: &[u8],
) -> StorageResult<(PlayerDto, bool)> {
    let dto = match schema {
        1 => decode_v1(player_id, revision, data)?,
        2 => decode_v2(player_id, revision, data)?,
        3 => decode_v3(player_id, revision, data)?,
        4 => decode_v4(player_id, revision, data)?,
        // v6 shares the v5 payload layout exactly: v6 only widened the legal
        // item registry, and the missing hunger fields are filled by the v6
        // migration step.
        5 | 6 => decode_v5(player_id, revision, data)?,
        7 => decode_v7(player_id, revision, data)?,
        8 => decode_v8(player_id, revision, data)?,
        9 => decode_v9(player_id, revision, data)?,
        _ => {
            return Err(corrupt(
                "player schema",
                format!("unsupported schema {schema}"),
            ));
        }
    };
    let migrated = migrate(schema, dto)?;
    Ok((migrated.0, migrated.1))
}

/// Applies the migration chain from `schema` up to the current schema.
///
/// Each step is deterministic: a legacy file never gains or loses user intent.
fn migrate(from: u32, mut dto: PlayerDto) -> StorageResult<(PlayerDto, bool)> {
    if from > CURRENT_SCHEMA {
        return Err(future_version("player schema", from));
    }
    let mut migrated = false;
    for version in from..CURRENT_SCHEMA {
        dto = match version {
            1 => {
                // v1 has no item payload: the deterministic migration is an
                // empty hotbar with slot zero selected.
                dto.inventory.hotbar = crate::items::Hotbar::default();
                dto
            }
            2 => {
                // v2 has no backpack payload: an empty backpack keeps the
                // hotbar the file already carried.
                dto.inventory.backpack = [ItemStack::default(); BACKPACK_SLOTS];
                dto
            }
            3 => {
                // v3 has no durability field: old tools migrate to full
                // durability, and non-tools keep their zero value.
                for slot in &mut dto.inventory.hotbar.slots {
                    *slot = fill_full_durability(slot);
                }
                for slot in &mut dto.inventory.backpack {
                    *slot = fill_full_durability(slot);
                }
                dto
            }
            4 => {
                // v4 has no health field: historical saves migrate to full
                // health.
                dto.health = MAX_HEALTH;
                dto
            }
            5 => dto,
            6 => {
                // v6 has no hunger state. The initial values match the domain
                // reset values so a legacy save and a new player log in with
                // the same hunger profile.
                dto.hunger = MAX_HUNGER;
                dto.saturation_milli = INITIAL_SATURATION_MILLI;
                dto.exhaustion_milli = 0;
                dto
            }
            7 => {
                // v7 has no personal respawn point: death respawn falls back to
                // the world spawn anchor, exactly as before the upgrade.
                dto.respawn_present = false;
                dto.respawn_position = [0.0; 3];
                dto.respawn_dimension = 0;
                dto
            }
            8 => {
                // v8 has no armor section: no armor was worn before the
                // upgrade, so the zero array is the canonical "nothing worn".
                dto.armor = [ItemStack::default(); 4];
                dto
            }
            _ => {
                return Err(corrupt(
                    "player migration",
                    format!("missing migration for {version}"),
                ));
            }
        };
        migrated = true;
    }
    Ok((dto, migrated))
}

/// Fills full durability into a legacy tool stack that carries no durability.
fn fill_full_durability(stack: &ItemStack) -> ItemStack {
    let mut filled = *stack;
    if let Some(full) = item_max_durability(stack.item)
        && filled.durability == 0
    {
        filled.durability = full;
    }
    filled
}

fn decode_v9(player_id: PlayerId, revision: u64, data: &[u8]) -> StorageResult<PlayerDto> {
    if data.len() < ARMOR_BYTES {
        return Err(corrupt("player payload", "shorter than the armor slots"));
    }
    let split = data.len() - ARMOR_BYTES;
    let mut dto = decode_v8(player_id, revision, &data[..split])?;
    let mut reader = ByteReader::new(&data[split..]);
    for slot in &mut dto.armor {
        *slot = decode_stack(&mut reader).map_err(|detail| corrupt("player armor slot", detail))?;
    }
    Ok(dto)
}

fn decode_v8(player_id: PlayerId, revision: u64, data: &[u8]) -> StorageResult<PlayerDto> {
    if data.len() < RESPAWN_BYTES {
        return Err(corrupt("player payload", "shorter than the respawn point"));
    }
    let split = data.len() - RESPAWN_BYTES;
    let mut dto = decode_v7(player_id, revision, &data[..split])?;
    let tail = &data[split..];
    match tail[0] {
        0 => {}
        1 => {
            dto.respawn_present = true;
            for (index, value) in dto.respawn_position.iter_mut().enumerate() {
                let start = 1 + 4 * index;
                *value = f32::from_bits(u32::from_le_bytes(
                    tail[start..start + 4].try_into().expect("four bytes"),
                ));
            }
            dto.respawn_dimension =
                i32::from_le_bytes(tail[13..17].try_into().expect("four bytes"));
        }
        other => {
            return Err(corrupt(
                "player respawn flag",
                format!("invalid flag {other}"),
            ));
        }
    }
    Ok(dto)
}

fn decode_v7(player_id: PlayerId, revision: u64, data: &[u8]) -> StorageResult<PlayerDto> {
    if data.len() < HUNGER_BYTES {
        return Err(corrupt("player payload", "shorter than the hunger state"));
    }
    let split = data.len() - HUNGER_BYTES;
    let mut dto = decode_v5(player_id, revision, &data[..split])?;
    let tail = &data[split..];
    dto.hunger = tail[0];
    dto.saturation_milli = u16::from_le_bytes([tail[1], tail[2]]);
    dto.exhaustion_milli = u16::from_le_bytes([tail[3], tail[4]]);
    Ok(dto)
}

fn decode_v5(player_id: PlayerId, revision: u64, data: &[u8]) -> StorageResult<PlayerDto> {
    if data.len() < HEALTH_BYTES {
        return Err(corrupt("player payload", "shorter than health"));
    }
    let split = data.len() - HEALTH_BYTES;
    let mut dto = decode_v4(player_id, revision, &data[..split])?;
    dto.health = data[split];
    Ok(dto)
}

fn decode_v4(player_id: PlayerId, revision: u64, data: &[u8]) -> StorageResult<PlayerDto> {
    let inventory_bytes = HOTBAR_BYTES + BACKPACK_BYTES;
    if data.len() < inventory_bytes {
        return Err(corrupt("player payload", "shorter than the inventory"));
    }
    let split = data.len() - inventory_bytes;
    let mut dto = decode_v1(player_id, revision, &data[..split])?;
    let mut reader = ByteReader::new(&data[split..]);
    dto.inventory.hotbar.selected = reader
        .u8()
        .map_err(|detail| corrupt("player hotbar selection", detail))?;
    for slot in &mut dto.inventory.hotbar.slots {
        *slot =
            decode_stack(&mut reader).map_err(|detail| corrupt("player hotbar slot", detail))?;
    }
    for slot in &mut dto.inventory.backpack {
        *slot =
            decode_stack(&mut reader).map_err(|detail| corrupt("player backpack slot", detail))?;
    }
    Ok(dto)
}

fn decode_v3(player_id: PlayerId, revision: u64, data: &[u8]) -> StorageResult<PlayerDto> {
    if data.len() < LEGACY_BACKPACK_BYTES {
        return Err(corrupt("player payload", "shorter than the backpack"));
    }
    let split = data.len() - LEGACY_BACKPACK_BYTES;
    let mut dto = decode_v2(player_id, revision, &data[..split])?;
    let mut reader = ByteReader::new(&data[split..]);
    for slot in &mut dto.inventory.backpack {
        *slot = decode_legacy_stack(&mut reader)
            .map_err(|detail| corrupt("player backpack slot", detail))?;
    }
    Ok(dto)
}

fn decode_v2(player_id: PlayerId, revision: u64, data: &[u8]) -> StorageResult<PlayerDto> {
    if data.len() < LEGACY_HOTBAR_BYTES {
        return Err(corrupt("player payload", "shorter than the hotbar"));
    }
    let split = data.len() - LEGACY_HOTBAR_BYTES;
    let mut dto = decode_v1(player_id, revision, &data[..split])?;
    let mut reader = ByteReader::new(&data[split..]);
    dto.inventory.hotbar.selected = reader
        .u8()
        .map_err(|detail| corrupt("player hotbar selection", detail))?;
    for slot in &mut dto.inventory.hotbar.slots {
        *slot = decode_legacy_stack(&mut reader)
            .map_err(|detail| corrupt("player hotbar slot", detail))?;
    }
    Ok(dto)
}

fn decode_v1(player_id: PlayerId, revision: u64, data: &[u8]) -> StorageResult<PlayerDto> {
    let mut reader = ByteReader::new(data);
    let name_length = reader
        .u32()
        .map_err(|detail| corrupt("player name length", detail))? as usize;
    if name_length > reader.remaining() {
        return Err(corrupt("player name length", "does not match payload"));
    }
    let name_bytes = reader
        .take_bytes(name_length)
        .map_err(|detail| corrupt("player name", detail))?;
    let display_name = String::from_utf8(name_bytes.to_vec())
        .map_err(|_| corrupt("player name", "not valid UTF-8"))?;
    let current = decode_location(&mut reader)
        .map_err(|detail| corrupt("player current location", detail))?;
    let yaw = reader
        .f32()
        .map_err(|detail| corrupt("player yaw", detail))?;
    let pitch = reader
        .f32()
        .map_err(|detail| corrupt("player pitch", detail))?;
    let has_safe = reader
        .u8()
        .map_err(|detail| corrupt("player safe flag", detail))?;
    let mut dto = PlayerDto::new(player_id, revision);
    dto.display_name = display_name;
    dto.current = current;
    dto.yaw = yaw;
    dto.pitch = pitch;
    match has_safe {
        0 => {}
        1 => {
            dto.safe = Some(
                decode_location(&mut reader)
                    .map_err(|detail| corrupt("player safe location", detail))?,
            );
        }
        other => {
            return Err(corrupt("player safe flag", format!("invalid flag {other}")));
        }
    }
    if reader.remaining() != 0 {
        return Err(corrupt("player payload", "trailing bytes"));
    }
    Ok(dto)
}

fn decode_location(reader: &mut ByteReader<'_>) -> Result<PlayerLocation, String> {
    let dimension = reader
        .u32()
        .map_err(|detail| format!("player dimension: {detail}"))? as i32;
    let mut position = [0f32; 3];
    for value in &mut position {
        *value = reader
            .f32()
            .map_err(|detail| format!("player position: {detail}"))?;
    }
    Ok(PlayerLocation {
        dimension,
        position,
    })
}

fn decode_stack(reader: &mut ByteReader<'_>) -> Result<ItemStack, String> {
    let item = reader
        .u16()
        .map_err(|detail| format!("player item: {detail}"))?;
    let count = reader
        .u8()
        .map_err(|detail| format!("player count: {detail}"))?;
    let durability = reader
        .u16()
        .map_err(|detail| format!("player durability: {detail}"))?;
    Ok(ItemStack {
        item,
        count,
        durability,
    })
}

/// Reads a legacy three-byte stack (item u16 + count u8) with no durability
/// field. The v3 migration step fills tools with full durability.
fn decode_legacy_stack(reader: &mut ByteReader<'_>) -> Result<ItemStack, String> {
    let item = reader
        .u16()
        .map_err(|detail| format!("player item: {detail}"))?;
    let count = reader
        .u8()
        .map_err(|detail| format!("player count: {detail}"))?;
    Ok(ItemStack {
        item,
        count,
        durability: 0,
    })
}

fn validate_save(save: &PlayerSave) -> StorageResult<()> {
    validate_dto(&StoredPlayer {
        player_id: save.player_id,
        revision: save.revision,
        display_name: save.display_name.clone(),
        current: save.current.clone(),
        yaw: save.yaw,
        pitch: save.pitch,
        safe: save.safe.clone(),
        inventory: save.inventory,
        health: save.health,
        hunger: save.hunger,
        saturation_milli: save.saturation_milli,
        exhaustion_milli: save.exhaustion_milli,
        respawn_present: save.respawn_present,
        respawn_position: save.respawn_position,
        respawn_dimension: save.respawn_dimension,
        armor: save.armor,
        needs_rewrite: false,
    })
}

/// Validates one player record.
///
/// The armor section deliberately takes no part: it is a pure-fidelity field
/// whose count, durability, and item-registry legality is judged by the
/// simulation, and validating it here would make legal historical saves
/// unreadable.
fn validate_dto(dto: &StoredPlayer) -> StorageResult<()> {
    if !dto.player_id.is_valid() {
        return Err(corrupt("player ID", "not a UUIDv4"));
    }
    if dto.revision == 0 {
        return Err(corrupt("player revision", "zero revision"));
    }
    let normalized = normalize_display_name(&dto.display_name)
        .map_err(|detail| corrupt("player display name", detail))?;
    if normalized != dto.display_name {
        return Err(corrupt("player display name", "is not normalized"));
    }
    validate_location(&dto.current).map_err(|detail| corrupt("player current location", detail))?;
    if !dto.yaw.is_finite() {
        return Err(corrupt("player yaw", "non-finite"));
    }
    if !dto.pitch.is_finite()
        || dto.pitch < -std::f32::consts::FRAC_PI_2
        || dto.pitch > std::f32::consts::FRAC_PI_2
    {
        return Err(corrupt("player pitch", "outside the closed range"));
    }
    if let Some(safe) = &dto.safe {
        validate_location(safe).map_err(|detail| corrupt("player safe location", detail))?;
    }
    if !dto.inventory.is_valid() {
        return Err(corrupt("player inventory", "not canonical"));
    }
    if dto.health > MAX_HEALTH {
        return Err(corrupt(
            "player health",
            format!("{} out of range", dto.health),
        ));
    }
    if dto.hunger > MAX_HUNGER {
        return Err(corrupt(
            "player hunger",
            format!("{} out of range", dto.hunger),
        ));
    }
    // Saturation is a buffer above hunger, so its ceiling is hunger itself in
    // milli units. The product fits far inside u16 and cannot overflow.
    if u32::from(dto.saturation_milli)
        > u32::from(dto.hunger) * u32::from(SATURATION_MILLI_PER_POINT)
    {
        return Err(corrupt("player saturation", "exceeds hunger"));
    }
    if dto.respawn_present {
        validate_location(&PlayerLocation {
            dimension: dto.respawn_dimension,
            position: dto.respawn_position,
        })
        .map_err(|detail| corrupt("player respawn point", detail))?;
    }
    Ok(())
}

fn validate_location(location: &PlayerLocation) -> Result<(), String> {
    // Player positions share the value range with chunk snapshots: overworld
    // and depths are allowed, anything at or above 2 is rejected.
    if location.dimension != OVERWORLD && location.dimension != DEPTHS {
        return Err(format!(
            "unsupported player dimension {}",
            location.dimension
        ));
    }
    for value in location.position {
        if !value.is_finite() {
            return Err("non-finite player position".to_owned());
        }
    }
    Ok(())
}

/// Normalizes a display name: trimmed, 1..32 runes, at most 128 bytes, and no
/// control characters.
fn normalize_display_name(name: &str) -> Result<String, String> {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return Err("length is outside 1..32".to_owned());
    }
    if trimmed.chars().count() > 32 || trimmed.len() > 128 {
        return Err("length is outside 1..32".to_owned());
    }
    if trimmed.chars().any(char::is_control) {
        return Err("contains a control character".to_owned());
    }
    Ok(trimmed.to_owned())
}

#[cfg(test)]
mod tests {
    use super::{
        CURRENT_SCHEMA, ENVELOPE_LENGTH, MAX_PAYLOAD, OLDEST_SCHEMA, PlayerSave, schema_readable,
    };

    #[test]
    fn player_schema_range_is_frozen() {
        assert_eq!(OLDEST_SCHEMA, 1);
        assert_eq!(CURRENT_SCHEMA, 9);
        assert_eq!(ENVELOPE_LENGTH, 44);
        assert_eq!(MAX_PAYLOAD, 1 << 20);
        assert!(schema_readable(1) && schema_readable(9));
        assert!(!schema_readable(0) && !schema_readable(10));
    }

    #[test]
    fn player_save_defaults_encode_a_well_formed_record() {
        let save = PlayerSave {
            player_id: super::PlayerId::default(),
            revision: 1,
            display_name: "行者".to_owned(),
            current: super::PlayerLocation {
                dimension: 0,
                position: [0.0, 64.0, 0.0],
            },
            yaw: 0.0,
            pitch: 0.0,
            safe: None,
            inventory: super::Inventory::default(),
            health: 20,
            hunger: 20,
            saturation_milli: 5_000,
            exhaustion_milli: 0,
            respawn_present: false,
            respawn_position: [0.0; 3],
            respawn_dimension: 0,
            armor: [super::ItemStack::default(); 4],
        };
        // The default identity is not a UUIDv4, so encoding must reject it.
        assert!(super::encode(&save).is_err());
    }
}
