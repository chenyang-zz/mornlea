//! Versioned save records and supported migration codecs.
//!
//! This crate may depend on `mornlea_domain` and must not depend on protocol
//! codecs, the numerical kernel, a graphical host, or an online authority.
//! Standalone entity families stay first-class save rows.

#![deny(unsafe_code)]

mod bytes;
mod chunk;
mod companion;
mod crc32c;
mod error;
mod hostile;
mod identity;
mod items;
mod passive;
mod player;
mod region;
mod world_metadata;

#[doc(hidden)]
pub use chunk::chunk_logical_wire_len_for_test;
pub use chunk::{
    COMPRESSION_ZSTD as CHUNK_COMPRESSION_ZSTD, CURRENT_SCHEMA as CHUNK_CURRENT_SCHEMA, ChestSlot,
    Chunk, ChunkCodec, ChunkSave, ContainerSnapshot, DecodedChunk, DropSlot,
    ENVELOPE_LENGTH as CHUNK_ENVELOPE_LENGTH, ENVELOPE_VERSION as CHUNK_ENVELOPE_VERSION,
    FurnaceSlot, LogicalPayload, MAX_DECODED_CHUNK as CHUNK_MAX_DECODED_CHUNK,
    OLDEST_SCHEMA as CHUNK_OLDEST_SCHEMA, StorageKind, checked_section, chunk_logical_len,
    decode as decode_chunk, decode_envelope as decode_chunk_envelope,
    decode_logical as decode_chunk_logical, encode as encode_chunk,
    encode_at_schema as encode_chunk_at_schema, encode_logical as encode_chunk_logical,
};
pub use companion::{
    CURRENT_SCHEMA as COMPANION_CURRENT_SCHEMA, CompanionBody, CompanionSave,
    ENVELOPE_VERSION as COMPANION_ENVELOPE_VERSION, MAX_FIFO_ENTRIES as COMPANION_MAX_FIFO_ENTRIES,
    MAX_FILE_LENGTH as COMPANION_MAX_FILE_LENGTH, MAX_PLAN_STEPS as COMPANION_MAX_PLAN_STEPS,
    MAX_STORED as COMPANION_MAX_STORED, MAX_SUMMARY_BYTES as COMPANION_MAX_SUMMARY_BYTES,
    MAX_TASK_COMMAND_BYTES as COMPANION_MAX_TASK_COMMAND_BYTES, PlanStep,
    SCHEMA_V1 as COMPANION_SCHEMA_V1, SCHEMA_V2 as COMPANION_SCHEMA_V2,
    SCHEMA_V3 as COMPANION_SCHEMA_V3, SCHEMA_V4 as COMPANION_SCHEMA_V4, StoredCompanionLifecycle,
    StoredCompanionQueue, StoredCompanionTask, StoredCompanions, decode as decode_companions,
    encode as encode_companions,
};
pub use companion::{
    PLAN_STEP_FOLLOW as COMPANION_PLAN_STEP_FOLLOW, PLAN_STEP_GO_TO as COMPANION_PLAN_STEP_GO_TO,
    PLAN_STEP_MINE as COMPANION_PLAN_STEP_MINE, PLAN_STEP_PLACE as COMPANION_PLAN_STEP_PLACE,
    TASK_COMPLETED as COMPANION_TASK_COMPLETED,
    TASK_FAIL_INVALID_PLAN as COMPANION_TASK_FAIL_INVALID_PLAN,
    TASK_FAIL_INVENTORY_FULL as COMPANION_TASK_FAIL_INVENTORY_FULL,
    TASK_FAIL_NONE as COMPANION_TASK_FAIL_NONE,
    TASK_FAIL_PATH_UNREACHABLE as COMPANION_TASK_FAIL_PATH_UNREACHABLE,
    TASK_FAIL_PLANNER_UNAVAILABLE as COMPANION_TASK_FAIL_PLANNER_UNAVAILABLE,
    TASK_FAIL_WORLD_CHANGED as COMPANION_TASK_FAIL_WORLD_CHANGED,
    TASK_FAILED as COMPANION_TASK_FAILED, TASK_PLANNING as COMPANION_TASK_PLANNING,
    TASK_QUEUED as COMPANION_TASK_QUEUED, TASK_RUNNING as COMPANION_TASK_RUNNING,
    TASK_STOPPED as COMPANION_TASK_STOPPED, TASK_TIMED_OUT as COMPANION_TASK_TIMED_OUT,
    TASK_VALIDATING as COMPANION_TASK_VALIDATING,
};
pub use crc32c::{crc32c, crc32c_join};
pub use error::{StorageError, StorageResult};
pub use hostile::{
    CURRENT_SCHEMA as HOSTILE_CURRENT_SCHEMA, ENVELOPE_VERSION as HOSTILE_ENVELOPE_VERSION,
    HostileMob, HostileMobs, HostileMobsSave, MAX_FILE_LENGTH as HOSTILE_MAX_FILE_LENGTH,
    MAX_HOSTILE_MOBS, SCHEMA_V1 as HOSTILE_SCHEMA_V1, decode as decode_hostile_mobs,
    encode as encode_hostile_mobs, encode_hostile_mobs_into, hostile_mobs_encoded_len,
};
pub use identity::{PlayerId, checked_companion_id, checked_player_id};
pub use items::{
    BACKPACK_SLOTS, HOTBAR_SLOTS, ITEM_ID_MAX, Inventory, ItemStack, MAX_STACK_COUNT,
    checked_item_stack, item_max_durability, item_stack_limit,
};
pub use passive::{
    CURRENT_SCHEMA as PASSIVE_CURRENT_SCHEMA, ENVELOPE_VERSION as PASSIVE_ENVELOPE_VERSION,
    MAX_FILE_LENGTH as PASSIVE_MAX_FILE_LENGTH, MAX_PASSIVE_MOBS, PassiveMob, PassiveMobs,
    PassiveMobsSave, decode as decode_passive_mobs, encode as encode_passive_mobs,
    encode_passive_mobs_into, passive_mobs_encoded_len,
};
pub use player::{
    CURRENT_SCHEMA as PLAYER_CURRENT_SCHEMA, ENVELOPE_LENGTH as PLAYER_ENVELOPE_LENGTH,
    MAX_PAYLOAD as PLAYER_MAX_PAYLOAD, OLDEST_SCHEMA as PLAYER_OLDEST_SCHEMA, PlayerLocation,
    PlayerSave, StoredPlayer, decode as decode_player, encode as encode_player,
    encode_into as encode_player_into, encoded_len as player_encoded_len,
    schema_readable as player_schema_readable,
};
pub use region::{
    BANK_A_START_SECTOR, BANK_B_START_SECTOR, BANK_SIZE, Bank as RegionBank,
    CURRENT_VERSION as REGION_CURRENT_VERSION, ChunkKey, DATA_START_SECTOR, Entry as RegionEntry,
    MAX_COMPRESSED_CHUNK, REGION_SLOTS, RegionKey, SECTOR_SIZE, decode_region_bank,
    decode_superblock, encode_region_bank, encode_region_bank_into, encode_superblock,
    encode_superblock_into, region_for, select_region_bank,
};
pub use world_metadata::{
    CURRENT_VERSION as METADATA_CURRENT_VERSION, ChunkPos as MetadataChunkPos, Metadata,
    V1 as METADATA_V1, V2 as METADATA_V2, V3 as METADATA_V3, V4 as METADATA_V4, V5 as METADATA_V5,
    decode as decode_world_metadata, encode as encode_world_metadata,
    encode_into as encode_world_metadata_into, encoded_len as world_metadata_encoded_len,
    valid_difficulty, valid_weather,
};

/// Workspace crate identity consumed by the foundation registration tests.
pub const CRATE_NAME: &str = "mornlea_storage";

/// Domain crate identity that save records share.
pub const DOMAIN_CRATE_NAME: &str = mornlea_domain::CRATE_NAME;
