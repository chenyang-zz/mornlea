//! `save.chunk`: the chunk save record (`CHNK` zstd envelope).
//!
//! Ported from the Go `chunk` storage codec. One envelope is a fixed 44-byte
//! header followed by a zstd frame carrying an `MCGC` logical payload: the
//! chunk identity, one paletted-container snapshot per section, and the fixed
//! drop, furnace and chest slot arrays.
//!
//! # Encoder output is intentionally not byte-identical to the Go encoder
//!
//! This was verified and ruled on; it is not an unresolved defect and must not
//! be "fixed". The Go envelope is built by `github.com/klauspost/compress/zstd`,
//! a pure-Go zstd implementation whose compressed block payload differs from the
//! reference libzstd bound here at every compression level, even though the frame
//! header and the trailing content checksum are byte-identical and the total
//! frame length can match. A standalone Go program re-encodes all nine committed
//! fixtures exactly, so the divergence is exclusively a Rust-versus-Go encoder
//! difference, and no crate available here binds klauspost.
//!
//! Cross-implementation compatibility is therefore defined at the logical/decode
//! level: zstd frames are self-describing, so the Go decoder reads what this
//! encoder writes and this decoder reads what the Go encoder wrote. Acceptance
//! for this family is semantic round-trip — exact decode of every committed
//! fixture, lossless encode/decode, migration convergence, and rejection without
//! implicit repair — never byte-identical re-encode. Do not add an assertion that
//! a frame produced here equals a committed fixture's compressed bytes.
//!
//! # Layers
//!
//! The module keeps the Go decomposition: [`encode`]/[`decode`] are the whole
//! envelope, [`decode_envelope`] is the header plus zstd frame on its own,
//! [`encode_logical`]/[`decode_logical`] are the `MCGC` payload, and `migrate`
//! normalizes an older schema. The intermediate layers are public so contract
//! tests can prove decode exactness byte for byte and migration convergence
//! without reaching into private state, following the precedent set by
//! [`crate::crc32c`].

use crate::bytes::{ByteReader, ByteWriter, SliceWriter};
use crate::error::{StorageError, StorageResult, corrupt, future_version};
use crate::items::{ITEM_COAL, ITEM_NONE, ItemStack, item_max_durability};
use crate::region::{ChunkKey, MAX_COMPRESSED_CHUNK};

/// Schema written by the current encoder.
pub const CURRENT_SCHEMA: u32 = 9;

/// Oldest schema the decoder still accepts.
pub const OLDEST_SCHEMA: u32 = 1;

/// `CHNK` envelope version. A larger value is a future version; a smaller one is
/// unsupported rather than repairable.
pub const ENVELOPE_VERSION: u32 = 1;

/// Fixed `CHNK` envelope header length.
pub const ENVELOPE_LENGTH: usize = 44;

/// Compression ID of the frame after the header. Any other value is corrupt.
pub const COMPRESSION_ZSTD: u32 = 1;

/// Decoded ceiling for one chunk payload, shared by the encoder and the decoder.
pub const MAX_DECODED_CHUNK: usize = 2 << 20;

const ENVELOPE_MAGIC: [u8; 4] = *b"CHNK";
const LOGICAL_MAGIC: [u8; 4] = *b"MCGC";

const SECTIONS_PER_CHUNK: usize = 24;
const BLOCKS_PER_SECTION: usize = 4096;
const DROPS_PER_CHUNK: usize = 32;
const FURNACES_PER_CHUNK: usize = 32;
const CHESTS_PER_CHUNK: usize = 16;
const CHEST_SLOTS: usize = 27;

/// Palette bound checked before any palette allocation.
const MAX_PALETTE_ENTRIES: usize = 1 << 8;
/// Packed-word bound checked before any packed allocation.
const MAX_PACKED_WORDS: usize = BLOCKS_PER_SECTION / 4;
/// Bits per block in direct storage.
const DIRECT_BITS: u8 = 15;

const FURNACE_SMELT_TICKS: u8 = 200;
const FURNACE_BURN_TICKS: u16 = 1600;

/// Dimension allowlist: only the overworld and `Depths` may be persisted.
const OVERWORLD: i32 = 0;
const DEPTHS: i32 = 1;

const FURNACE_BLOCK: u16 = 9;
const CHEST_BLOCK: u16 = 11;

/// Compression level mirroring the Go encoder's default (`SpeedDefault`).
const COMPRESSION_LEVEL: i32 = 3;

/// Storage form of one paletted section, matching `world.StorageKind`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StorageKind {
    /// One block ID for the whole section.
    Single = 0,
    /// Palette indices packed at 4 or 8 bits per block.
    Indexed = 1,
    /// Global block IDs packed at 15 bits per block.
    Direct = 2,
}

impl StorageKind {
    fn from_u8(value: u8) -> Option<Self> {
        match value {
            0 => Some(Self::Single),
            1 => Some(Self::Indexed),
            2 => Some(Self::Direct),
            _ => None,
        }
    }
}

/// One section's independent, verifiable block snapshot.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ContainerSnapshot {
    pub kind: StorageKind,
    pub bits: u8,
    pub single: u16,
    pub palette: Vec<u16>,
    pub packed: Vec<u64>,
}

/// One fixed drop slot inside a chunk.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct DropSlot {
    pub generation: u32,
    pub active: bool,
    pub stack: ItemStack,
    pub block_index: u32,
    pub age_ticks: u32,
    pub pickup_delay_ticks: u8,
}

/// One fixed furnace slot inside a chunk.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct FurnaceSlot {
    pub generation: u32,
    pub active: bool,
    pub block_index: u32,
    pub input: ItemStack,
    pub fuel: ItemStack,
    pub output: ItemStack,
    pub progress_ticks: u8,
    pub burn_ticks: u16,
}

impl FurnaceSlot {
    /// Reports whether the slot satisfies the fixed furnace constraints. An
    /// inactive slot keeps only its generation; an active one must point at a
    /// registered generation, stay inside both tick ceilings, and hold a
    /// canonical input, fuel and output.
    pub fn is_valid(&self) -> bool {
        if !self.active {
            return *self
                == Self {
                    generation: self.generation,
                    ..Self::default()
                };
        }
        if self.generation == 0 {
            return false;
        }
        if self.progress_ticks >= FURNACE_SMELT_TICKS || self.burn_ticks > FURNACE_BURN_TICKS {
            return false;
        }
        valid_furnace_input(self.input)
            && self.fuel.is_valid()
            && (self.fuel.item == ITEM_NONE || self.fuel.item == ITEM_COAL)
            && valid_furnace_output(self.output)
    }
}

/// One fixed chest slot inside a chunk.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ChestSlot {
    pub generation: u32,
    pub active: bool,
    pub block_index: u32,
    pub items: [ItemStack; CHEST_SLOTS],
}

impl ChestSlot {
    /// Reports whether the slot satisfies the fixed chest constraints. An
    /// inactive slot keeps only its generation; an active one must have a
    /// registered generation and twenty-seven canonical item slots.
    pub fn is_valid(&self) -> bool {
        if !self.active {
            return *self
                == Self {
                    generation: self.generation,
                    ..Self::default()
                };
        }
        if self.generation == 0 {
            return false;
        }
        self.items.iter().all(ItemStack::is_valid)
    }
}

/// One chunk's persisted content: the section snapshots plus the fixed slot
/// arrays.
///
/// The chunk position is deliberately not stored here. The Go `world.Chunk`
/// carries its own `Pos`, so the Go encoder rejects a save whose chunk position
/// disagrees with the requested key; in this value model the key on
/// [`ChunkSave`] is the single source of truth, so that disagreement cannot be
/// constructed in the first place.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Chunk {
    pub sections: Vec<ContainerSnapshot>,
    pub drops: Vec<DropSlot>,
    pub furnaces: Vec<FurnaceSlot>,
    pub chests: Vec<ChestSlot>,
}

/// One chunk save request.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ChunkSave {
    pub key: ChunkKey,
    pub revision: u64,
    pub chunk: Chunk,
}

/// One caller-owned chunk compression and decompression context.
///
/// The struct owns bounded logical and compressed scratch plus one zstd context
/// pair reused across calls. It is not `Sync`: one context belongs to one caller,
/// and the `&mut self` borrow serializes access without an internal lock.
pub struct ChunkCodec {
    compressor: zstd::bulk::Compressor<'static>,
    decompressor: zstd::bulk::Decompressor<'static>,
    logical_scratch: Vec<u8>,
    compressed_scratch: Vec<u8>,
}

impl ChunkCodec {
    /// Creates the zstd context pair and scratch buffers this codec reuses.
    pub fn try_new() -> StorageResult<Self> {
        let mut compressor = zstd::bulk::Compressor::new(COMPRESSION_LEVEL)
            .map_err(|err| corrupt("create zstd compressor", err))?;
        compressor
            .set_parameter(zstd::zstd_safe::CParameter::ChecksumFlag(true))
            .map_err(|err| corrupt("enable zstd checksum", err))?;
        compressor
            .set_parameter(zstd::zstd_safe::CParameter::ContentSizeFlag(true))
            .map_err(|err| corrupt("enable zstd content size", err))?;

        let mut decompressor = zstd::bulk::Decompressor::new()
            .map_err(|err| corrupt("create zstd decompressor", err))?;
        let window_log = MAX_DECODED_CHUNK.trailing_zeros();
        decompressor
            .set_parameter(zstd::zstd_safe::DParameter::WindowLogMax(window_log))
            .map_err(|err| corrupt("configure zstd window", err))?;

        Ok(Self {
            compressor,
            decompressor,
            logical_scratch: Vec::new(),
            compressed_scratch: Vec::new(),
        })
    }

    /// Encodes one current-schema chunk into `dst` and returns the bytes written.
    ///
    /// [`chunk_logical_len`] is the sole current-value validator; the logical
    /// payload is appended through the private already-validated writer without
    /// calling [`validate_save`] or [`encode_logical`] again.
    pub fn encode_into(&mut self, save: &ChunkSave, dst: &mut [u8]) -> StorageResult<usize> {
        self.logical_scratch.clear();
        self.compressed_scratch.clear();
        let schema = CURRENT_SCHEMA;
        let logical_len = match chunk_logical_len(save, schema) {
            Ok(len) => len,
            Err(err) => {
                self.clear_visible_scratch();
                return Err(err);
            }
        };
        fill_logical_scratch(
            &mut self.logical_scratch,
            save.key,
            save.revision,
            &save.chunk,
            schema,
        );
        debug_assert_eq!(self.logical_scratch.len(), logical_len);

        if let Err(err) = self.compress_logical() {
            self.clear_visible_scratch();
            return Err(err);
        }

        let frame_len = self.compressed_scratch.len();
        if frame_len > MAX_COMPRESSED_CHUNK as usize {
            self.clear_visible_scratch();
            return Err(corrupt(
                "compressed chunk",
                format!("{frame_len} exceeds limit {MAX_COMPRESSED_CHUNK}"),
            ));
        }
        let total = ENVELOPE_LENGTH
            .checked_add(frame_len)
            .ok_or_else(|| corrupt("chunk envelope length", "overflow"))?;
        if dst.len() < total {
            self.clear_visible_scratch();
            return Err(StorageError::OutputTooSmall {
                needed: total,
                available: dst.len(),
            });
        }

        write_envelope(
            &mut dst[..total],
            schema,
            save.key,
            save.revision,
            logical_len,
            &self.compressed_scratch,
        )?;
        Ok(total)
    }

    /// Verifies one envelope and returns an owned chunk normalized to the current schema.
    pub fn decode(
        &mut self,
        key: ChunkKey,
        revision: u64,
        payload: &[u8],
    ) -> StorageResult<DecodedChunk> {
        self.logical_scratch.clear();
        self.compressed_scratch.clear();
        if revision == 0 {
            self.clear_visible_scratch();
            return Err(corrupt("chunk revision", "zero requested revision"));
        }
        let envelope = match self.decode_envelope(payload) {
            Ok(envelope) => envelope,
            Err(err) => {
                self.clear_visible_scratch();
                return Err(err);
            }
        };
        if envelope.key != key || envelope.revision != revision {
            self.clear_visible_scratch();
            return Err(corrupt(
                "chunk envelope",
                "key or revision does not match request",
            ));
        }
        let chunk = match decode_logical(
            envelope.key,
            envelope.revision,
            envelope.schema,
            &self.logical_scratch,
        ) {
            Ok(chunk) => chunk,
            Err(err) => {
                self.clear_visible_scratch();
                return Err(err);
            }
        };
        let (chunk, migrated) = match migrate(envelope.schema, chunk) {
            Ok(pair) => pair,
            Err(err) => {
                self.clear_visible_scratch();
                return Err(err);
            }
        };
        if let Err(err) = validate_chunk(&chunk) {
            self.clear_visible_scratch();
            return Err(err);
        }
        Ok(DecodedChunk {
            key: envelope.key,
            revision: envelope.revision,
            schema: CURRENT_SCHEMA,
            chunk,
            migrated,
        })
    }

    fn decode_envelope(&mut self, payload: &[u8]) -> StorageResult<LogicalPayload> {
        let mut envelope = ByteReader::new(payload);
        let magic = envelope
            .array::<4>()
            .map_err(|detail| corrupt("envelope magic", detail))?;
        if magic != ENVELOPE_MAGIC {
            return Err(corrupt("envelope magic", "unexpected magic"));
        }
        let version = envelope
            .u32()
            .map_err(|detail| corrupt("envelope version", detail))?;
        if version > ENVELOPE_VERSION {
            return Err(future_version("envelope version", version));
        }
        if version != ENVELOPE_VERSION {
            return Err(corrupt(
                "envelope version",
                format!("unsupported envelope version {version}"),
            ));
        }
        let schema = envelope
            .u32()
            .map_err(|detail| corrupt("envelope schema", detail))?;
        if schema > CURRENT_SCHEMA {
            return Err(future_version("chunk schema", schema));
        }
        if schema < OLDEST_SCHEMA {
            return Err(corrupt(
                "chunk schema",
                format!("unsupported chunk schema {schema}"),
            ));
        }
        let key = decode_key(&mut envelope).map_err(|detail| corrupt("envelope key", detail))?;
        let revision = envelope
            .u64()
            .map_err(|detail| corrupt("envelope revision", detail))?;
        if revision == 0 {
            return Err(corrupt("envelope revision", "zero revision"));
        }
        let compression = envelope
            .u32()
            .map_err(|detail| corrupt("compression ID", detail))?;
        if compression != COMPRESSION_ZSTD {
            return Err(corrupt(
                "compression ID",
                format!("unknown compression ID {compression}"),
            ));
        }
        let decoded_length = envelope
            .u32()
            .map_err(|detail| corrupt("decoded length", detail))?
            as usize;
        if decoded_length > MAX_DECODED_CHUNK {
            return Err(corrupt(
                "decoded length",
                format!("{decoded_length} exceeds limit {MAX_DECODED_CHUNK}"),
            ));
        }
        let compressed_length = envelope
            .u32()
            .map_err(|detail| corrupt("compressed length", detail))?
            as usize;
        if compressed_length > MAX_COMPRESSED_CHUNK as usize {
            return Err(corrupt(
                "compressed length",
                format!("{compressed_length} exceeds limit {MAX_COMPRESSED_CHUNK}"),
            ));
        }
        if envelope.remaining() != compressed_length {
            return Err(corrupt(
                "compressed length",
                "does not match the envelope remainder",
            ));
        }
        let compressed = envelope
            .take_bytes(compressed_length)
            .map_err(|detail| corrupt("compressed bytes", detail))?;

        self.logical_scratch =
            decompress_with(compressed, decoded_length, Some(&mut self.decompressor))?;
        if self.logical_scratch.len() != decoded_length {
            return Err(corrupt(
                "decoded length",
                "does not match the decompressed payload",
            ));
        }
        Ok(LogicalPayload {
            key,
            revision,
            schema,
            bytes: Vec::new(),
        })
    }

    fn compress_logical(&mut self) -> StorageResult<()> {
        self.compressed_scratch.clear();
        let logical_len = self.logical_scratch.len();
        let bound = zstd::zstd_safe::compress_bound(logical_len);
        if bound > MAX_COMPRESSED_CHUNK as usize {
            return Err(corrupt(
                "compressed chunk",
                format!("compress bound {bound} exceeds limit {MAX_COMPRESSED_CHUNK}"),
            ));
        }
        self.compressor
            .context_mut()
            .set_pledged_src_size(Some(logical_len as u64))
            .map_err(|err| corrupt("pledge zstd source size", err))?;
        self.compressed_scratch
            .try_reserve(bound)
            .map_err(|_| corrupt("compressed scratch", "allocation failed"))?;
        let written = self
            .compressor
            .compress_to_buffer(&self.logical_scratch, &mut self.compressed_scratch)
            .map_err(|err| corrupt("compress chunk", err))?;
        self.compressed_scratch.truncate(written);
        Ok(())
    }

    fn clear_visible_scratch(&mut self) {
        self.logical_scratch.clear();
        self.compressed_scratch.clear();
    }
}

/// A decoded chunk normalized to the current schema.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DecodedChunk {
    pub key: ChunkKey,
    pub revision: u64,
    /// Always [`CURRENT_SCHEMA`]: older schemas are migrated on decode.
    pub schema: u32,
    pub chunk: Chunk,
    /// Set when the record was read at an older schema and needs a rewrite.
    pub migrated: bool,
}

/// One verified `CHNK` envelope: the identity it declares plus the decompressed
/// `MCGC` logical payload, still at the schema the envelope carried.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LogicalPayload {
    pub key: ChunkKey,
    pub revision: u64,
    pub schema: u32,
    pub bytes: Vec<u8>,
}

/// Encodes one chunk as the bounded, versioned `CHNK` envelope at the current
/// schema.
pub fn encode(save: &ChunkSave) -> StorageResult<Vec<u8>> {
    encode_at_schema(save, CURRENT_SCHEMA)
}

/// Encodes one chunk as a `CHNK` envelope at `schema`.
///
/// Production callers use [`encode`], which always writes the current schema.
/// The older schemas are accepted so a contract test can encode one logical
/// chunk at every supported version and prove that decoding and migrating each
/// converges on the same normalized result. Bounds are checked before any
/// allocation or compression, and a payload that cannot be read back is rejected
/// rather than truncated.
pub fn encode_at_schema(save: &ChunkSave, schema: u32) -> StorageResult<Vec<u8>> {
    if !(OLDEST_SCHEMA..=CURRENT_SCHEMA).contains(&schema) {
        return Err(corrupt(
            "chunk schema",
            format!("unsupported schema {schema}"),
        ));
    }
    let logical_len = chunk_logical_len(save, schema)?;
    let logical = append_logical(save.key, save.revision, &save.chunk, schema);
    debug_assert_eq!(logical.len(), logical_len);
    let compressed = compress(&logical)?;
    if compressed.len() > MAX_COMPRESSED_CHUNK as usize {
        return Err(corrupt(
            "compressed chunk",
            format!("{} exceeds limit {MAX_COMPRESSED_CHUNK}", compressed.len()),
        ));
    }

    let mut payload = ByteWriter::new();
    payload.bytes(&ENVELOPE_MAGIC);
    payload.u32(ENVELOPE_VERSION);
    payload.u32(schema);
    payload.u32(save.key.dimension as u32);
    payload.u32(save.key.x as u32);
    payload.u32(save.key.z as u32);
    payload.u64(save.revision);
    payload.u32(COMPRESSION_ZSTD);
    payload.u32(logical.len() as u32);
    payload.u32(compressed.len() as u32);
    payload.bytes(&compressed);
    Ok(payload.into_vec())
}

/// Verifies one `CHNK` envelope and reconstructs an independent chunk.
///
/// `key` and `revision` must be the requested identity: a record for a different
/// chunk is rejected rather than silently loaded.
pub fn decode(key: ChunkKey, revision: u64, payload: &[u8]) -> StorageResult<DecodedChunk> {
    if revision == 0 {
        return Err(corrupt("chunk revision", "zero requested revision"));
    }
    let envelope = decode_envelope(payload)?;
    if envelope.key != key || envelope.revision != revision {
        return Err(corrupt(
            "chunk envelope",
            "key or revision does not match request",
        ));
    }
    let chunk = decode_logical(
        envelope.key,
        envelope.revision,
        envelope.schema,
        &envelope.bytes,
    )?;
    let (chunk, migrated) = migrate(envelope.schema, chunk)?;
    validate_chunk(&chunk)?;
    Ok(DecodedChunk {
        key: envelope.key,
        revision: envelope.revision,
        schema: CURRENT_SCHEMA,
        chunk,
        migrated,
    })
}

/// Verifies one `CHNK` envelope and returns the decompressed `MCGC` logical
/// payload with the identity and schema it declares.
///
/// This is the envelope layer standing on its own. Contract tests use it to
/// obtain the exact logical bytes a committed Go fixture carries, so decode
/// exactness can be proven byte for byte against [`encode_logical`]. Every
/// length and version bound is checked before any allocation or decompression.
pub fn decode_envelope(payload: &[u8]) -> StorageResult<LogicalPayload> {
    let mut envelope = ByteReader::new(payload);
    let magic = envelope
        .array::<4>()
        .map_err(|detail| corrupt("envelope magic", detail))?;
    if magic != ENVELOPE_MAGIC {
        return Err(corrupt("envelope magic", "unexpected magic"));
    }
    let version = envelope
        .u32()
        .map_err(|detail| corrupt("envelope version", detail))?;
    if version > ENVELOPE_VERSION {
        return Err(future_version("envelope version", version));
    }
    if version != ENVELOPE_VERSION {
        return Err(corrupt(
            "envelope version",
            format!("unsupported envelope version {version}"),
        ));
    }
    let schema = envelope
        .u32()
        .map_err(|detail| corrupt("envelope schema", detail))?;
    if schema > CURRENT_SCHEMA {
        return Err(future_version("chunk schema", schema));
    }
    if schema < OLDEST_SCHEMA {
        return Err(corrupt(
            "chunk schema",
            format!("unsupported chunk schema {schema}"),
        ));
    }
    let key = decode_key(&mut envelope).map_err(|detail| corrupt("envelope key", detail))?;
    let revision = envelope
        .u64()
        .map_err(|detail| corrupt("envelope revision", detail))?;
    if revision == 0 {
        return Err(corrupt("envelope revision", "zero revision"));
    }
    let compression = envelope
        .u32()
        .map_err(|detail| corrupt("compression ID", detail))?;
    if compression != COMPRESSION_ZSTD {
        return Err(corrupt(
            "compression ID",
            format!("unknown compression ID {compression}"),
        ));
    }
    let decoded_length = envelope
        .u32()
        .map_err(|detail| corrupt("decoded length", detail))? as usize;
    if decoded_length > MAX_DECODED_CHUNK {
        return Err(corrupt(
            "decoded length",
            format!("{decoded_length} exceeds limit {MAX_DECODED_CHUNK}"),
        ));
    }
    let compressed_length = envelope
        .u32()
        .map_err(|detail| corrupt("compressed length", detail))?
        as usize;
    if compressed_length > MAX_COMPRESSED_CHUNK as usize {
        return Err(corrupt(
            "compressed length",
            format!("{compressed_length} exceeds limit {MAX_COMPRESSED_CHUNK}"),
        ));
    }
    if envelope.remaining() != compressed_length {
        return Err(corrupt(
            "compressed length",
            "does not match the envelope remainder",
        ));
    }
    let compressed = envelope
        .take_bytes(compressed_length)
        .map_err(|detail| corrupt("compressed bytes", detail))?;

    let bytes = decompress_with(compressed, decoded_length, None)?;
    if bytes.len() != decoded_length {
        return Err(corrupt(
            "decoded length",
            "does not match the decompressed payload",
        ));
    }
    Ok(LogicalPayload {
        key,
        revision,
        schema,
        bytes,
    })
}

/// Serializes one chunk as an `MCGC` logical payload at `schema`.
///
/// [`encode_at_schema`] writes the current schema; the older schemas exist so a
/// contract test can re-serialize a decoded legacy chunk and compare it byte for
/// byte with the logical bytes its fixture carried.
/// Reports the exact `MCGC` logical payload length for a current [`ChunkSave`]
/// at `schema`, after one aggregate and representability check.
///
/// Direct [`encode_logical`] keeps separate raw admission for historical tool
/// drops; this preflight is for current-value envelope encoding only.
pub fn chunk_logical_len(save: &ChunkSave, schema: u32) -> StorageResult<usize> {
    if !(OLDEST_SCHEMA..=CURRENT_SCHEMA).contains(&schema) {
        return Err(corrupt(
            "chunk schema",
            format!("unsupported schema {schema}"),
        ));
    }
    validate_save(save, schema)?;
    logical_payload_len(&save.chunk, schema)
}

/// Wire length of one logical payload after validation; no dense block expansion.
fn logical_payload_len(chunk: &Chunk, schema: u32) -> StorageResult<usize> {
    let mut total = LOGICAL_HEADER_LEN;
    for (index, section) in chunk.sections.iter().enumerate() {
        total = total
            .checked_add(section_wire_len(section)?)
            .ok_or_else(|| corrupt("logical payload length", "overflow"))?;
        if total > MAX_DECODED_CHUNK {
            return Err(corrupt(
                "decoded chunk",
                format!("{total} exceeds limit {MAX_DECODED_CHUNK} at section {index}"),
            ));
        }
    }
    if schema >= 2 {
        let drop_bytes = if schema >= 5 {
            DROP_SLOT_WITH_DURABILITY_LEN
        } else {
            DROP_SLOT_LEGACY_LEN
        };
        total = total
            .checked_add(
                DROPS_PER_CHUNK
                    .checked_mul(drop_bytes)
                    .ok_or_else(|| corrupt("logical payload length", "overflow"))?,
            )
            .ok_or_else(|| corrupt("logical payload length", "overflow"))?;
        if total > MAX_DECODED_CHUNK {
            return Err(corrupt(
                "decoded chunk",
                format!("{total} exceeds limit {MAX_DECODED_CHUNK} after drops"),
            ));
        }
    }
    if schema >= 4 {
        total = total
            .checked_add(
                FURNACES_PER_CHUNK
                    .checked_mul(FURNACE_SLOT_LEN)
                    .ok_or_else(|| corrupt("logical payload length", "overflow"))?,
            )
            .ok_or_else(|| corrupt("logical payload length", "overflow"))?;
        if total > MAX_DECODED_CHUNK {
            return Err(corrupt(
                "decoded chunk",
                format!("{total} exceeds limit {MAX_DECODED_CHUNK} after furnaces"),
            ));
        }
    }
    if schema >= 6 {
        total = total
            .checked_add(
                CHESTS_PER_CHUNK
                    .checked_mul(CHEST_SLOT_LEN)
                    .ok_or_else(|| corrupt("logical payload length", "overflow"))?,
            )
            .ok_or_else(|| corrupt("logical payload length", "overflow"))?;
        if total > MAX_DECODED_CHUNK {
            return Err(corrupt(
                "decoded chunk",
                format!("{total} exceeds limit {MAX_DECODED_CHUNK} after chests"),
            ));
        }
    }
    Ok(total)
}

/// `MCGC` header: magic, schema, key, revision, and section count.
const LOGICAL_HEADER_LEN: usize = 32;
const DROP_SLOT_LEGACY_LEN: usize = 17;
const DROP_SLOT_WITH_DURABILITY_LEN: usize = 19;
const FURNACE_SLOT_LEN: usize = 21;
const CHEST_SLOT_LEN: usize = 144;

/// Fixed-prefix section record plus palette and packed payload sizes.
fn section_wire_len(section: &ContainerSnapshot) -> StorageResult<usize> {
    let palette_bytes = section
        .palette
        .len()
        .checked_mul(2)
        .ok_or_else(|| corrupt("section wire length", "overflow"))?;
    let packed_bytes = section
        .packed
        .len()
        .checked_mul(8)
        .ok_or_else(|| corrupt("section wire length", "overflow"))?;
    let base = 16usize;
    base.checked_add(palette_bytes)
        .and_then(|n| n.checked_add(packed_bytes))
        .ok_or_else(|| corrupt("section wire length", "overflow"))
}

/// Hidden wire sum without validation; contract tests use this to prove the
/// decoded cap and checked arithmetic on impossible shapes.
#[doc(hidden)]
pub fn chunk_logical_wire_len_for_test(chunk: &Chunk, schema: u32) -> StorageResult<usize> {
    logical_payload_len(chunk, schema)
}

pub fn encode_logical(
    key: ChunkKey,
    revision: u64,
    chunk: &Chunk,
    schema: u32,
) -> StorageResult<Vec<u8>> {
    if !(OLDEST_SCHEMA..=CURRENT_SCHEMA).contains(&schema) {
        return Err(corrupt(
            "chunk schema",
            format!("unsupported schema {schema}"),
        ));
    }
    if key.dimension != OVERWORLD && key.dimension != DEPTHS {
        return Err(corrupt(
            "chunk dimension",
            format!("unsupported chunk dimension {}", key.dimension),
        ));
    }
    if revision == 0 {
        return Err(corrupt("chunk revision", "zero revision"));
    }
    // A logical payload may be raw historical data, including pre-v5 tool
    // drops. Admission uses its declared layout before appending bytes.
    validate_chunk_at_schema(chunk, schema, schema)?;
    Ok(append_logical(key, revision, chunk, schema))
}

/// Appends only after the caller has admitted both values and schema fidelity.
fn append_logical(key: ChunkKey, revision: u64, chunk: &Chunk, schema: u32) -> Vec<u8> {
    let mut logical = ByteWriter::new();
    write_logical_payload(&mut logical, key, revision, chunk, schema);
    logical.into_vec()
}

fn fill_logical_scratch(
    scratch: &mut Vec<u8>,
    key: ChunkKey,
    revision: u64,
    chunk: &Chunk,
    schema: u32,
) {
    scratch.clear();
    let mut logical = ByteWriter::new();
    write_logical_payload(&mut logical, key, revision, chunk, schema);
    *scratch = logical.into_vec();
}

fn write_logical_payload(
    logical: &mut ByteWriter,
    key: ChunkKey,
    revision: u64,
    chunk: &Chunk,
    schema: u32,
) {
    logical.bytes(&LOGICAL_MAGIC);
    logical.u32(schema);
    logical.u32(key.dimension as u32);
    logical.u32(key.x as u32);
    logical.u32(key.z as u32);
    logical.u64(revision);
    logical.u32(SECTIONS_PER_CHUNK as u32);
    for (index, section) in chunk.sections.iter().enumerate() {
        append_section(logical, index as u32, section);
    }
    if schema >= 2 {
        for drop in &chunk.drops {
            append_drop_slot(logical, drop, schema >= 5);
        }
    }
    if schema >= 4 {
        for furnace in &chunk.furnaces {
            append_furnace_slot(logical, furnace);
        }
    }
    if schema >= 6 {
        for chest in &chunk.chests {
            append_chest_slot(logical, chest);
        }
    }
}

fn write_envelope(
    dst: &mut [u8],
    schema: u32,
    key: ChunkKey,
    revision: u64,
    logical_len: usize,
    compressed: &[u8],
) -> StorageResult<()> {
    let mut writer = SliceWriter::new(dst);
    writer.bytes(&ENVELOPE_MAGIC);
    writer.u32(ENVELOPE_VERSION);
    writer.u32(schema);
    writer.u32(key.dimension as u32);
    writer.u32(key.x as u32);
    writer.u32(key.z as u32);
    writer.u64(revision);
    writer.u32(COMPRESSION_ZSTD);
    writer.u32(logical_len as u32);
    writer.u32(compressed.len() as u32);
    writer.bytes(compressed);
    debug_assert_eq!(writer.pos(), ENVELOPE_LENGTH + compressed.len());
    Ok(())
}

/// Parses one `MCGC` logical payload at `schema` without migrating it.
///
/// The chunk identity inside the payload must match the requested one, and the
/// declared schema must match `schema`; a mismatch is corruption rather than a
/// reason to guess.
pub fn decode_logical(
    key: ChunkKey,
    revision: u64,
    schema: u32,
    data: &[u8],
) -> StorageResult<Chunk> {
    if !(OLDEST_SCHEMA..=CURRENT_SCHEMA).contains(&schema) {
        return Err(corrupt(
            "chunk schema",
            format!("unsupported chunk schema {schema}"),
        ));
    }
    let mut logical = ByteReader::new(data);
    let magic = logical
        .array::<4>()
        .map_err(|detail| corrupt("logical magic", detail))?;
    if magic != LOGICAL_MAGIC {
        return Err(corrupt("logical magic", "unexpected magic"));
    }
    let declared = logical
        .u32()
        .map_err(|detail| corrupt("logical schema", detail))?;
    if declared != schema {
        return Err(corrupt(
            "logical schema",
            "does not match the envelope schema",
        ));
    }
    let payload_key = decode_key(&mut logical).map_err(|detail| corrupt("logical key", detail))?;
    let payload_revision = logical
        .u64()
        .map_err(|detail| corrupt("logical revision", detail))?;
    if payload_key != key || payload_revision != revision {
        return Err(corrupt(
            "logical key",
            "key or revision does not match the envelope",
        ));
    }
    let count = logical
        .u32()
        .map_err(|detail| corrupt("section count", detail))? as usize;
    if count != SECTIONS_PER_CHUNK {
        return Err(corrupt(
            "section count",
            format!("{count}, want {SECTIONS_PER_CHUNK}"),
        ));
    }

    let mut sections = Vec::with_capacity(SECTIONS_PER_CHUNK);
    for index in 0..SECTIONS_PER_CHUNK {
        let section_index = logical
            .u32()
            .map_err(|detail| corrupt("section index", detail))?
            as usize;
        if section_index != index {
            return Err(corrupt(
                "section index",
                format!("{section_index} at position {index}"),
            ));
        }
        let section = decode_container_snapshot(&mut logical)
            .map_err(|detail| corrupt("section", format!("{index}: {detail}")))?;
        sections.push(section);
    }
    let mut drops = vec![DropSlot::default(); DROPS_PER_CHUNK];
    if schema >= 2 {
        for (slot, drop) in drops.iter_mut().enumerate() {
            *drop = if schema >= 5 {
                decode_drop_slot(&mut logical)
            } else {
                decode_legacy_drop_slot(&mut logical)
            }
            .map_err(|detail| corrupt("drop slot", format!("{slot}: {detail}")))?;
        }
    }
    let mut furnaces = vec![FurnaceSlot::default(); FURNACES_PER_CHUNK];
    if schema >= 4 {
        for (slot, furnace) in furnaces.iter_mut().enumerate() {
            *furnace = decode_furnace_slot(&mut logical)
                .map_err(|detail| corrupt("furnace slot", format!("{slot}: {detail}")))?;
        }
    }
    let mut chests = vec![ChestSlot::default(); CHESTS_PER_CHUNK];
    if schema >= 6 {
        for (slot, chest) in chests.iter_mut().enumerate() {
            *chest = decode_chest_slot(&mut logical)
                .map_err(|detail| corrupt("chest slot", format!("{slot}: {detail}")))?;
        }
    }
    if logical.remaining() != 0 {
        return Err(corrupt("logical payload", "trailing bytes"));
    }
    Ok(Chunk {
        sections,
        drops,
        furnaces,
        chests,
    })
}

/// Normalizes a chunk read at `schema` to the current schema.
///
/// Every step from `schema` up to [`CURRENT_SCHEMA`] runs in order, so a chunk
/// read at the oldest schema and one read one step before the current schema
/// converge on the same normalized value. The chunk is taken by value, so a
/// rejected migration cannot leave a half-migrated record behind.
fn migrate(schema: u32, chunk: Chunk) -> StorageResult<(Chunk, bool)> {
    if schema > CURRENT_SCHEMA {
        return Err(future_version("chunk schema", schema));
    }
    let mut migrated = false;
    let mut current = chunk;
    for version in schema..CURRENT_SCHEMA {
        current = match version {
            // v1 carries no drop payload; the deterministic migration is an
            // empty drop set.
            1 => Chunk {
                drops: vec![DropSlot::default(); DROPS_PER_CHUNK],
                ..current
            },
            // v2 and v3 share the v3 layout; the step only lets older readers
            // reject records with newer blocks.
            2 => current,
            // v3 carries no furnace payload; the deterministic migration is an
            // empty furnace set.
            3 => Chunk {
                furnaces: vec![FurnaceSlot::default(); FURNACES_PER_CHUNK],
                ..current
            },
            // v4 drops carry no durability and may hold legacy multi-item tool
            // stacks.
            4 => migrate_v4_drops(current)?,
            // v5 carries no chest payload; the deterministic migration is an
            // empty chest set.
            5 => Chunk {
                chests: vec![ChestSlot::default(); CHESTS_PER_CHUNK],
                ..current
            },
            // v6, v7 and v8 share their successors' layouts and only add block
            // semantics, so their steps are identities. v9 likewise keeps the
            // v8 payload layout and only appends fluid block numbers, so no
            // block data is rewritten and no water is injected into old chunks.
            6..=8 => current,
            other => {
                return Err(corrupt(
                    "chunk migration",
                    format!("missing migration from schema {other}"),
                ));
            }
        };
        migrated = true;
    }
    Ok((current, migrated))
}

/// Applies the v4 drop migration: missing durability is filled with the tool's
/// full durability, and legacy multi-item tool stacks are split into one slot
/// per item.
fn migrate_v4_drops(chunk: Chunk) -> StorageResult<Chunk> {
    let mut drops = chunk.drops;
    for drop in &mut drops {
        drop.stack = fill_full_durability(drop.stack);
    }
    split_legacy_tool_drop_stacks(&mut drops)?;
    Ok(Chunk { drops, ..chunk })
}

/// Fills a missing durability on a tool stack; non-tools keep the zero value.
fn fill_full_durability(mut stack: ItemStack) -> ItemStack {
    if let Some(full) = item_max_durability(stack.item)
        && stack.durability == 0
    {
        stack.durability = full;
    }
    stack
}

/// Splits a legacy multi-item tool drop into one slot per item, reusing the
/// lowest inactive slot that has not been exhausted.
///
/// The split is rejected when no reusable slot remains: the record is corrupt
/// rather than repairable by dropping items.
fn split_legacy_tool_drop_stacks(drops: &mut [DropSlot]) -> StorageResult<()> {
    let sources = drops.to_vec();
    for (source_slot, source) in sources.iter().enumerate() {
        if !source.active || source.stack.count <= 1 {
            continue;
        }
        if item_max_durability(source.stack.item).is_none() {
            continue;
        }
        drops[source_slot].stack.count = 1;
        for _ in 1..source.stack.count {
            let Some(target_slot) = drops
                .iter()
                .position(|candidate| !candidate.active && candidate.generation != u32::MAX)
            else {
                return Err(corrupt(
                    "legacy tool drop",
                    format!("slot {source_slot} has insufficient reusable drop slots"),
                ));
            };
            drops[target_slot] = DropSlot {
                generation: drops[target_slot].generation + 1,
                stack: ItemStack {
                    count: 1,
                    ..source.stack
                },
                ..*source
            };
        }
    }
    Ok(())
}

/// Rejects a save that could never be read back, before any allocation.
///
/// Key and revision admission stay here. Section, drop, furnace, chest, and
/// active-container checks are the same `validate_chunk` path decode uses, so
/// an encoder cannot publish a chunk the decoder would reject.
fn validate_save(save: &ChunkSave, output_schema: u32) -> StorageResult<()> {
    if save.revision == 0 {
        return Err(corrupt("chunk revision", "zero revision"));
    }
    if save.key.dimension != OVERWORLD && save.key.dimension != DEPTHS {
        return Err(corrupt(
            "chunk dimension",
            format!("unsupported chunk dimension {}", save.key.dimension),
        ));
    }
    validate_chunk_at_schema(&save.chunk, CURRENT_SCHEMA, output_schema)
}

/// Rejects a chunk whose fixed slot arrays have the wrong length.
fn validate_chunk_shape(chunk: &Chunk) -> StorageResult<()> {
    if chunk.sections.len() != SECTIONS_PER_CHUNK {
        return Err(corrupt(
            "chunk sections",
            format!("{}, want {SECTIONS_PER_CHUNK}", chunk.sections.len()),
        ));
    }
    if chunk.drops.len() != DROPS_PER_CHUNK {
        return Err(corrupt(
            "chunk drops",
            format!("{}, want {DROPS_PER_CHUNK}", chunk.drops.len()),
        ));
    }
    if chunk.furnaces.len() != FURNACES_PER_CHUNK {
        return Err(corrupt(
            "chunk furnaces",
            format!("{}, want {FURNACES_PER_CHUNK}", chunk.furnaces.len()),
        ));
    }
    if chunk.chests.len() != CHESTS_PER_CHUNK {
        return Err(corrupt(
            "chunk chests",
            format!("{}, want {CHESTS_PER_CHUNK}", chunk.chests.len()),
        ));
    }
    Ok(())
}

/// Rejects a chunk that decodes but could not have been a live chunk: every
/// section snapshot must be a loadable container, and every active furnace or
/// chest slot must point at a matching block inside the chunk.
///
/// The derived height map the Go side rebuilds here is not persisted, so it is
/// not modeled.
fn validate_chunk(chunk: &Chunk) -> StorageResult<()> {
    validate_chunk_at_schema(chunk, CURRENT_SCHEMA, CURRENT_SCHEMA)
}

/// Validates one raw aggregate and refuses fields its output layout would lose.
/// A whole-envelope writer admits current values; a direct logical writer
/// admits historical values using their own drop rule.
fn validate_chunk_at_schema(
    chunk: &Chunk,
    value_schema: u32,
    output_schema: u32,
) -> StorageResult<()> {
    validate_chunk_shape(chunk)?;
    // One domain section per snapshot. Active containers read `block_at`
    // instead of scanning the 98,304 cells into a dense buffer.
    let mut sections = Vec::with_capacity(chunk.sections.len());
    for (index, snapshot) in chunk.sections.iter().enumerate() {
        sections.push(checked_section(snapshot).map_err(|err| match err {
            StorageError::Corrupt(detail) => {
                let detail = detail.strip_prefix("section: ").unwrap_or(detail.as_str());
                corrupt("section", format!("{index}: {detail}"))
            }
            other => other,
        })?);
    }
    for (slot, drop) in chunk.drops.iter().enumerate() {
        (if value_schema < 5 {
            validate_legacy_drop_slot(drop)
        } else {
            validate_drop_slot(drop)
        })
        .map_err(|detail| corrupt("drop slot", format!("{slot}: {detail}")))?;
        if output_schema < 2 && *drop != DropSlot::default() {
            return Err(corrupt(
                "drop slot",
                format!("{slot}: schema omits nondefault drop"),
            ));
        }
        if output_schema < 5 && drop.stack.durability != 0 {
            return Err(corrupt(
                "drop slot",
                format!("{slot}: schema omits durability"),
            ));
        }
        // Pre-v5 migration fills zero durability even in inactive tool drops.
        // Current-value envelope encoding must not publish bytes that decode
        // into a different inactive slot; raw logical reserialization is exempt.
        if value_schema == CURRENT_SCHEMA
            && (2..5).contains(&output_schema)
            && !drop.active
            && drop.stack.durability == 0
            && item_max_durability(drop.stack.item).is_some()
        {
            return Err(corrupt(
                "drop slot",
                format!("{slot}: legacy migration would change inactive tool durability"),
            ));
        }
    }
    let mut seen_furnaces: Vec<u32> = Vec::new();
    for (slot, furnace) in chunk.furnaces.iter().enumerate() {
        if !furnace.is_valid() {
            return Err(corrupt(
                "furnace slot",
                format!("{slot} is not a valid fixed slot"),
            ));
        }
        if output_schema < 4 && *furnace != FurnaceSlot::default() {
            return Err(corrupt(
                "furnace slot",
                format!("{slot}: schema omits nondefault furnace"),
            ));
        }
        if !furnace.active {
            continue;
        }
        let (section, local) = split_block_index(furnace.block_index).ok_or_else(|| {
            corrupt(
                "furnace slot",
                format!("{slot} block index is outside the chunk"),
            )
        })?;
        if seen_furnaces.contains(&furnace.block_index) {
            return Err(corrupt(
                "furnace slot",
                format!("{slot} shares block index {}", furnace.block_index),
            ));
        }
        seen_furnaces.push(furnace.block_index);
        if sections[section].block_at(local) != Some(FURNACE_BLOCK) {
            return Err(corrupt(
                "furnace slot",
                format!("{slot} does not point at a furnace block"),
            ));
        }
    }
    let mut seen_chests: Vec<u32> = Vec::new();
    for (slot, chest) in chunk.chests.iter().enumerate() {
        if !chest.is_valid() {
            return Err(corrupt(
                "chest slot",
                format!("{slot} is not a valid fixed slot"),
            ));
        }
        if output_schema < 6 && *chest != ChestSlot::default() {
            return Err(corrupt(
                "chest slot",
                format!("{slot}: schema omits nondefault chest"),
            ));
        }
        if !chest.active {
            continue;
        }
        let (section, local) = split_block_index(chest.block_index).ok_or_else(|| {
            corrupt(
                "chest slot",
                format!("{slot} block index is outside the chunk"),
            )
        })?;
        if seen_chests.contains(&chest.block_index) {
            return Err(corrupt(
                "chest slot",
                format!("{slot} shares block index {}", chest.block_index),
            ));
        }
        seen_chests.push(chest.block_index);
        if sections[section].block_at(local) != Some(CHEST_BLOCK) {
            return Err(corrupt(
                "chest slot",
                format!("{slot} does not point at a chest block"),
            ));
        }
    }
    Ok(())
}

/// Reports whether the input slot is empty or holds a registered smelting input.
fn valid_furnace_input(stack: ItemStack) -> bool {
    if !stack.is_valid() {
        return false;
    }
    if stack.item == ITEM_NONE {
        return true;
    }
    mornlea_domain::smelting_output(stack.item).is_some()
}

/// Reports whether the output slot is empty or holds a smelting product.
fn valid_furnace_output(stack: ItemStack) -> bool {
    if !stack.is_valid() {
        return false;
    }
    stack.item == ITEM_NONE || mornlea_domain::is_smelting_product(stack.item)
}

/// Splits a compact chunk block index into a section index and a linear index
/// inside that section, or reports that it lies outside the chunk.
fn split_block_index(index: u32) -> Option<(usize, usize)> {
    let index = index as usize;
    if index >= SECTIONS_PER_CHUNK * BLOCKS_PER_SECTION {
        return None;
    }
    Some((index / BLOCKS_PER_SECTION, index % BLOCKS_PER_SECTION))
}

/// Reads the raw slot at a section-linear index from the non-crossing word
/// layout: `64 / bits` slots per word, the remaining bits unused.
///
/// Production association reads `PalettedSection::block_at`. This helper stays
/// for the layout unit test.
#[cfg(test)]
fn read_packed(packed: &[u64], bits: u8, index: usize) -> Option<u32> {
    // Only the three storage widths reach here, but the shift and mask below
    // would be undefined for a zero or out-of-range width, so guard anyway.
    if bits == 0 || bits > 63 {
        return None;
    }
    let per_word = 64 / bits as usize;
    let word = *packed.get(index / per_word)?;
    let shift = ((index % per_word) * bits as usize) as u32;
    let mask = (1u64 << bits) - 1;
    Some(((word >> shift) & mask) as u32)
}

/// Number of words needed for `BLOCKS_PER_SECTION` slots of `bits` bits each.
fn words_for(bits: u8) -> usize {
    let per_word = 64 / bits as usize;
    BLOCKS_PER_SECTION.div_ceil(per_word)
}

/// Converts one raw section into a domain section.
///
/// Mode residue is checked once here. Palette order, packed bits, registered
/// blocks, and palette indexes are decided by the domain constructors, which
/// read the compact arrays directly and do not expand the section.
pub fn checked_section(raw: &ContainerSnapshot) -> StorageResult<mornlea_domain::PalettedSection> {
    reject_section_residue(raw)?;
    let converted = match raw.kind {
        StorageKind::Single => mornlea_domain::PalettedSection::single(raw.single),
        StorageKind::Indexed => mornlea_domain::PalettedSection::indexed(
            raw.bits,
            raw.palette.clone().into_boxed_slice(),
            raw.packed.clone().into_boxed_slice(),
        ),
        StorageKind::Direct => {
            mornlea_domain::PalettedSection::direct(raw.packed.clone().into_boxed_slice())
        }
    };
    converted.map_err(|_| corrupt("section", "domain section rejected the snapshot"))
}

/// Rejects mode residue before a domain constructor sees the snapshot.
///
/// A single section must carry no packed payload, an indexed section must
/// carry no single value and the exact palette/word shape, and a direct
/// section must carry no single value and no palette. Registration and
/// per-cell scans stay in the domain constructor.
fn reject_section_residue(snapshot: &ContainerSnapshot) -> StorageResult<()> {
    match snapshot.kind {
        StorageKind::Single => {
            if snapshot.bits != 0 || !snapshot.palette.is_empty() || !snapshot.packed.is_empty() {
                return Err(corrupt("section", "single storage has compressed payload"));
            }
        }
        StorageKind::Indexed => {
            if snapshot.bits != 4 && snapshot.bits != 8 {
                return Err(corrupt(
                    "section",
                    format!("indexed bits {} is invalid", snapshot.bits),
                ));
            }
            if snapshot.single != 0 {
                return Err(corrupt("section", "indexed storage has a single value"));
            }
            if snapshot.palette.is_empty() || snapshot.palette.len() > 1usize << snapshot.bits {
                return Err(corrupt(
                    "section",
                    format!(
                        "palette length {} is invalid for {} bits",
                        snapshot.palette.len(),
                        snapshot.bits
                    ),
                ));
            }
            if snapshot.packed.len() != words_for(snapshot.bits) {
                return Err(corrupt(
                    "section",
                    format!(
                        "packed length {}, want {}",
                        snapshot.packed.len(),
                        words_for(snapshot.bits)
                    ),
                ));
            }
        }
        StorageKind::Direct => {
            if snapshot.bits != DIRECT_BITS {
                return Err(corrupt(
                    "section",
                    format!("direct bits {} is invalid", snapshot.bits),
                ));
            }
            if snapshot.single != 0 || !snapshot.palette.is_empty() {
                return Err(corrupt(
                    "section",
                    "direct storage has a palette or single value",
                ));
            }
            if snapshot.packed.len() != words_for(DIRECT_BITS) {
                return Err(corrupt(
                    "section",
                    format!(
                        "packed length {}, want {}",
                        snapshot.packed.len(),
                        words_for(DIRECT_BITS)
                    ),
                ));
            }
        }
    }
    Ok(())
}

/// Decodes one section snapshot, checking every bound before the matching
/// allocation.
fn decode_container_snapshot(reader: &mut ByteReader<'_>) -> Result<ContainerSnapshot, String> {
    let kind = reader.u8()?;
    let bits = reader.u8()?;
    let single = reader.u16()?;
    let palette_length = reader.u32()? as usize;
    if palette_length > MAX_PALETTE_ENTRIES {
        return Err(format!(
            "palette length {palette_length} exceeds bound {MAX_PALETTE_ENTRIES}"
        ));
    }
    // Four bytes for the packed length must survive the palette read.
    if reader.remaining() < palette_length * 2 + 4 {
        return Err("unexpected end of record".to_owned());
    }
    let mut palette = Vec::with_capacity(palette_length);
    for _ in 0..palette_length {
        palette.push(reader.u16()?);
    }
    let packed_length = reader.u32()? as usize;
    if packed_length > MAX_PACKED_WORDS {
        return Err(format!(
            "packed length {packed_length} exceeds bound {MAX_PACKED_WORDS}"
        ));
    }
    if reader.remaining() < packed_length * 8 {
        return Err("unexpected end of record".to_owned());
    }
    let mut packed = Vec::with_capacity(packed_length);
    for _ in 0..packed_length {
        packed.push(reader.u64()?);
    }
    let kind =
        StorageKind::from_u8(kind).ok_or_else(|| format!("unknown snapshot storage {kind}"))?;
    Ok(ContainerSnapshot {
        kind,
        bits,
        single,
        palette,
        packed,
    })
}

/// Decodes one chunk key: a dimension inside the allowlist plus coordinates.
fn decode_key(reader: &mut ByteReader<'_>) -> Result<ChunkKey, String> {
    let dimension = reader.u32()? as i32;
    // Only the overworld and `Depths` may be persisted; anything else is
    // corruption rather than a value to guess a dimension for.
    if dimension > DEPTHS {
        return Err(format!("unsupported dimension {dimension}"));
    }
    let x = reader.u32()? as i32;
    let z = reader.u32()? as i32;
    Ok(ChunkKey { dimension, x, z })
}

fn append_section(dst: &mut ByteWriter, index: u32, snapshot: &ContainerSnapshot) {
    dst.u32(index);
    dst.u8(snapshot.kind as u8);
    dst.u8(snapshot.bits);
    dst.u16(snapshot.single);
    dst.u32(snapshot.palette.len() as u32);
    for id in &snapshot.palette {
        dst.u16(*id);
    }
    dst.u32(snapshot.packed.len() as u32);
    for word in &snapshot.packed {
        dst.u64(*word);
    }
}

/// Appends one drop slot. `with_durability` selects the schema-5 layout; older
/// schemas omit the durability field.
fn append_drop_slot(dst: &mut ByteWriter, drop: &DropSlot, with_durability: bool) {
    dst.u32(drop.generation);
    dst.u8(u8::from(drop.active));
    dst.u16(drop.stack.item);
    dst.u8(drop.stack.count);
    if with_durability {
        dst.u16(drop.stack.durability);
    }
    dst.u32(drop.block_index);
    dst.u32(drop.age_ticks);
    dst.u8(drop.pickup_delay_ticks);
}

fn append_furnace_slot(dst: &mut ByteWriter, furnace: &FurnaceSlot) {
    dst.u32(furnace.generation);
    dst.u8(u8::from(furnace.active));
    dst.u32(furnace.block_index);
    for stack in [furnace.input, furnace.fuel, furnace.output] {
        dst.u16(stack.item);
        dst.u8(stack.count);
    }
    dst.u8(furnace.progress_ticks);
    dst.u16(furnace.burn_ticks);
}

fn append_chest_slot(dst: &mut ByteWriter, chest: &ChestSlot) {
    dst.u32(chest.generation);
    dst.u8(u8::from(chest.active));
    dst.u32(chest.block_index);
    for stack in &chest.items {
        dst.u16(stack.item);
        dst.u8(stack.count);
        dst.u16(stack.durability);
    }
}

/// Decodes one drop slot at the current layout and validates it.
fn decode_drop_slot(reader: &mut ByteReader<'_>) -> Result<DropSlot, String> {
    let mut drop = DropSlot {
        generation: reader.u32()?,
        ..DropSlot::default()
    };
    let active = reader.u8()?;
    if active > 1 {
        return Err(format!("invalid drop active flag {active}"));
    }
    drop.active = active == 1;
    drop.stack.item = reader.u16()?;
    drop.stack.count = reader.u8()?;
    drop.stack.durability = reader.u16()?;
    drop.block_index = reader.u32()?;
    drop.age_ticks = reader.u32()?;
    drop.pickup_delay_ticks = reader.u8()?;
    validate_drop_slot(&drop)?;
    Ok(drop)
}

/// Decodes one drop slot at the pre-schema-5 layout, which has no durability and
/// may hold a legacy multi-item tool stack.
fn decode_legacy_drop_slot(reader: &mut ByteReader<'_>) -> Result<DropSlot, String> {
    let mut drop = DropSlot {
        generation: reader.u32()?,
        ..DropSlot::default()
    };
    let active = reader.u8()?;
    if active > 1 {
        return Err(format!("invalid drop active flag {active}"));
    }
    drop.active = active == 1;
    drop.stack.item = reader.u16()?;
    drop.stack.count = reader.u8()?;
    drop.block_index = reader.u32()?;
    drop.age_ticks = reader.u32()?;
    drop.pickup_delay_ticks = reader.u8()?;
    validate_legacy_drop_slot(&drop)?;
    Ok(drop)
}

/// Applies the pre-v5 drop rule before durability and tool-count migration.
fn validate_legacy_drop_slot(drop: &DropSlot) -> Result<(), String> {
    if !drop.active {
        return Ok(());
    }
    if drop.generation == 0 {
        return Err("active drop slot has zero generation".to_owned());
    }
    if drop.block_index >= (SECTIONS_PER_CHUNK * BLOCKS_PER_SECTION) as u32 {
        return Err(format!(
            "drop block index {} is outside the chunk",
            drop.block_index
        ));
    }
    if item_max_durability(drop.stack.item).is_some() {
        // Legacy tool stacks are bounded by the general stack limit; the split
        // into single-item slots is the v4 migration's job.
        if drop.stack.count < 1 || drop.stack.count > crate::items::MAX_STACK_COUNT {
            return Err("legacy tool drop stack is invalid".to_owned());
        }
        return Ok(());
    }
    if !drop.stack.is_valid() {
        return Err("drop stack is invalid".to_owned());
    }
    Ok(())
}

/// Checks an active drop slot's fixed field bounds; an inactive slot keeps only
/// its generation.
fn validate_drop_slot(drop: &DropSlot) -> Result<(), String> {
    if !drop.active {
        return Ok(());
    }
    if drop.generation == 0 {
        return Err("active drop slot has zero generation".to_owned());
    }
    if !drop.stack.is_valid() {
        return Err("drop stack is invalid".to_owned());
    }
    if drop.block_index >= (SECTIONS_PER_CHUNK * BLOCKS_PER_SECTION) as u32 {
        return Err(format!(
            "drop block index {} is outside the chunk",
            drop.block_index
        ));
    }
    Ok(())
}

/// Decodes one furnace slot and validates it.
fn decode_furnace_slot(reader: &mut ByteReader<'_>) -> Result<FurnaceSlot, String> {
    let mut furnace = FurnaceSlot {
        generation: reader.u32()?,
        ..FurnaceSlot::default()
    };
    let active = reader.u8()?;
    if active > 1 {
        return Err(format!("furnace active flag {active} is not 0 or 1"));
    }
    furnace.active = active == 1;
    furnace.block_index = reader.u32()?;
    for stack in [&mut furnace.input, &mut furnace.fuel, &mut furnace.output] {
        stack.item = reader.u16()?;
        stack.count = reader.u8()?;
    }
    furnace.progress_ticks = reader.u8()?;
    furnace.burn_ticks = reader.u16()?;
    if !furnace.is_valid() {
        return Err("furnace slot is not a valid fixed slot".to_owned());
    }
    Ok(furnace)
}

/// Decodes one chest slot and validates it.
fn decode_chest_slot(reader: &mut ByteReader<'_>) -> Result<ChestSlot, String> {
    let mut chest = ChestSlot {
        generation: reader.u32()?,
        ..ChestSlot::default()
    };
    let active = reader.u8()?;
    if active > 1 {
        return Err(format!("chest active flag {active} is not 0 or 1"));
    }
    chest.active = active == 1;
    chest.block_index = reader.u32()?;
    for stack in &mut chest.items {
        stack.item = reader.u16()?;
        stack.count = reader.u8()?;
        stack.durability = reader.u16()?;
    }
    if !chest.is_valid() {
        return Err("chest slot is not a valid fixed slot".to_owned());
    }
    Ok(chest)
}

/// Compresses one logical payload into a single-frame zstd stream.
///
/// The settings mirror the Go encoder: one worker and the content checksum
/// enabled, with the exact logical length pledged so the frame carries its
/// content size. The compressed block payload legitimately differs from the Go
/// encoder's; see the module documentation.
fn compress(logical: &[u8]) -> StorageResult<Vec<u8>> {
    let mut encoder = zstd::stream::write::Encoder::new(Vec::new(), COMPRESSION_LEVEL)
        .map_err(|err| corrupt("create zstd encoder", err))?;
    encoder
        .set_pledged_src_size(Some(logical.len() as u64))
        .map_err(|err| corrupt("pledge zstd source size", err))?;
    encoder
        .include_checksum(true)
        .map_err(|err| corrupt("enable zstd checksum", err))?;
    encoder
        .include_contentsize(true)
        .map_err(|err| corrupt("enable zstd content size", err))?;
    std::io::Write::write_all(&mut encoder, logical)
        .map_err(|err| corrupt("compress chunk", err))?;
    encoder
        .finish()
        .map_err(|err| corrupt("finish zstd frame", err))
}

/// Decompresses one zstd frame into a destination of exactly `decoded_length`
/// bytes, matching the Go `DecodeAll` call. The frame's own content checksum is
/// verified, so a truncated or altered frame is rejected here.
fn decompress_with(
    compressed: &[u8],
    decoded_length: usize,
    decompressor: Option<&mut zstd::bulk::Decompressor<'_>>,
) -> StorageResult<Vec<u8>> {
    match decompressor {
        Some(context) => context
            .decompress(compressed, decoded_length)
            .map_err(|err| corrupt("decompress chunk", err)),
        None => zstd::bulk::decompress(compressed, decoded_length)
            .map_err(|err| corrupt("decompress chunk", err)),
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::StorageError;
    use crate::items::{ITEM_STONE, ITEM_STONE_PICKAXE};

    fn empty_slot_arrays() -> (Vec<DropSlot>, Vec<FurnaceSlot>, Vec<ChestSlot>) {
        (
            vec![DropSlot::default(); DROPS_PER_CHUNK],
            vec![FurnaceSlot::default(); FURNACES_PER_CHUNK],
            vec![ChestSlot::default(); CHESTS_PER_CHUNK],
        )
    }

    fn air_chunk() -> Chunk {
        let (drops, furnaces, chests) = empty_slot_arrays();
        Chunk {
            sections: vec![
                ContainerSnapshot {
                    kind: StorageKind::Single,
                    bits: 0,
                    single: 0,
                    palette: Vec::new(),
                    packed: Vec::new(),
                };
                SECTIONS_PER_CHUNK
            ],
            drops,
            furnaces,
            chests,
        }
    }

    #[test]
    fn envelope_layout_is_the_frozen_forty_four_bytes() {
        let save = ChunkSave {
            key: ChunkKey {
                dimension: OVERWORLD,
                x: -3,
                z: 7,
            },
            revision: 19,
            chunk: air_chunk(),
        };
        let encoded = encode(&save).expect("encode air chunk");
        assert_eq!(&encoded[..4], b"CHNK");
        assert_eq!(
            u32::from_le_bytes(encoded[4..8].try_into().unwrap()),
            ENVELOPE_VERSION
        );
        assert_eq!(
            u32::from_le_bytes(encoded[8..12].try_into().unwrap()),
            CURRENT_SCHEMA
        );
        assert_eq!(u32::from_le_bytes(encoded[12..16].try_into().unwrap()), 0);
        assert_eq!(
            u32::from_le_bytes(encoded[16..20].try_into().unwrap()),
            (-3i32) as u32
        );
        assert_eq!(u32::from_le_bytes(encoded[20..24].try_into().unwrap()), 7);
        assert_eq!(u64::from_le_bytes(encoded[24..32].try_into().unwrap()), 19);
        assert_eq!(
            u32::from_le_bytes(encoded[32..36].try_into().unwrap()),
            COMPRESSION_ZSTD
        );
        // The current schema always writes every container section: 24 fixed
        // sixteen-byte single-storage records, then the fixed drop, furnace and
        // chest slot arrays.
        let logical_length = 32
            + SECTIONS_PER_CHUNK * 16
            + DROPS_PER_CHUNK * 19
            + FURNACES_PER_CHUNK * 21
            + CHESTS_PER_CHUNK * (9 + CHEST_SLOTS * 5);
        assert_eq!(
            u32::from_le_bytes(encoded[36..40].try_into().unwrap()) as usize,
            logical_length
        );
        assert_eq!(
            u32::from_le_bytes(encoded[40..44].try_into().unwrap()) as usize,
            encoded.len() - ENVELOPE_LENGTH
        );
    }

    #[test]
    fn air_chunk_round_trips() {
        let save = ChunkSave {
            key: ChunkKey {
                dimension: DEPTHS,
                x: 12,
                z: -40,
            },
            revision: 3,
            chunk: air_chunk(),
        };
        let encoded = encode(&save).expect("encode");
        let decoded = decode(save.key, save.revision, &encoded).expect("decode");
        assert_eq!(decoded.chunk, save.chunk);
        assert_eq!(decoded.schema, CURRENT_SCHEMA);
        assert!(!decoded.migrated);
    }

    #[test]
    fn packed_word_layout_never_crosses_a_word_boundary() {
        assert_eq!(words_for(4), 256);
        assert_eq!(words_for(8), 512);
        assert_eq!(words_for(DIRECT_BITS), 1024);
        let packed = vec![u64::MAX; words_for(4)];
        assert_eq!(read_packed(&packed, 4, 0), Some(0x0f));
        assert_eq!(read_packed(&packed, 4, 15), Some(0x0f));
        assert_eq!(read_packed(&packed, 4, 16), Some(0x0f));
        assert_eq!(read_packed(&[], 4, 0), None);
        assert_eq!(read_packed(&packed, 0, 0), None);
    }

    #[test]
    fn split_block_index_covers_the_whole_chunk() {
        assert_eq!(split_block_index(0), Some((0, 0)));
        assert_eq!(split_block_index(4095), Some((0, 4095)));
        assert_eq!(split_block_index(4096), Some((1, 0)));
        assert_eq!(
            split_block_index((SECTIONS_PER_CHUNK * BLOCKS_PER_SECTION - 1) as u32),
            Some((SECTIONS_PER_CHUNK - 1, BLOCKS_PER_SECTION - 1))
        );
        assert_eq!(
            split_block_index((SECTIONS_PER_CHUNK * BLOCKS_PER_SECTION) as u32),
            None
        );
    }

    #[test]
    fn inactive_slots_keep_only_their_generation() {
        let furnace = FurnaceSlot {
            generation: 4,
            ..FurnaceSlot::default()
        };
        assert!(furnace.is_valid());
        assert!(
            !FurnaceSlot {
                block_index: 9,
                ..furnace
            }
            .is_valid()
        );
        let chest = ChestSlot {
            generation: 4,
            ..ChestSlot::default()
        };
        assert!(chest.is_valid());
        assert!(
            !ChestSlot {
                block_index: 9,
                ..chest
            }
            .is_valid()
        );
    }

    #[test]
    fn furnace_fuel_and_output_whitelists_are_enforced() {
        let base = FurnaceSlot {
            generation: 1,
            active: true,
            block_index: 0,
            input: ItemStack {
                item: ITEM_NONE,
                count: 0,
                durability: 0,
            },
            fuel: ItemStack {
                item: ITEM_COAL,
                count: 1,
                durability: 0,
            },
            output: ItemStack::default(),
            progress_ticks: 0,
            burn_ticks: 0,
        };
        assert!(base.is_valid());
        assert!(
            !FurnaceSlot {
                fuel: ItemStack {
                    item: ITEM_STONE_PICKAXE,
                    count: 1,
                    durability: 131,
                },
                ..base
            }
            .is_valid()
        );
        assert!(
            !FurnaceSlot {
                output: ItemStack {
                    item: ITEM_STONE,
                    count: 1,
                    durability: 0,
                },
                ..base
            }
            .is_valid()
        );
        assert!(
            !FurnaceSlot {
                input: ItemStack {
                    item: ITEM_STONE,
                    count: 1,
                    durability: 0,
                },
                ..base
            }
            .is_valid()
        );
    }

    #[test]
    fn dimension_allowlist_rejects_beyond_depths() {
        let mut save = ChunkSave {
            key: ChunkKey {
                dimension: DEPTHS,
                x: 0,
                z: 0,
            },
            revision: 1,
            chunk: air_chunk(),
        };
        assert!(encode(&save).is_ok());
        save.key.dimension = 2;
        assert!(matches!(encode(&save), Err(StorageError::Corrupt(_))));
    }

    #[test]
    fn legacy_tool_split_rejects_insufficient_capacity() {
        let mut drops = vec![DropSlot::default(); DROPS_PER_CHUNK];
        drops[0] = DropSlot {
            generation: 1,
            active: true,
            stack: ItemStack {
                item: ITEM_STONE_PICKAXE,
                count: 2,
                durability: 131,
            },
            block_index: 0,
            age_ticks: 0,
            pickup_delay_ticks: 0,
        };
        for slot in drops.iter_mut().skip(1) {
            slot.generation = u32::MAX;
        }
        assert!(split_legacy_tool_drop_stacks(&mut drops).is_err());
    }

    /// An active slot that points at air is not a readable aggregate, so the
    /// encoder rejects it before any bytes are published.
    #[test]
    fn active_container_slots_must_point_at_their_block() {
        let mut chunk = air_chunk();
        chunk.furnaces[0] = FurnaceSlot {
            generation: 2,
            active: true,
            block_index: 0,
            input: ItemStack::default(),
            fuel: ItemStack::default(),
            output: ItemStack::default(),
            progress_ticks: 0,
            burn_ticks: 0,
        };
        let save = ChunkSave {
            key: ChunkKey {
                dimension: OVERWORLD,
                x: 0,
                z: 0,
            },
            revision: 1,
            chunk: chunk.clone(),
        };
        assert!(matches!(encode(&save), Err(StorageError::Corrupt(_))));

        let mut chunk = air_chunk();
        chunk.chests[0] = ChestSlot {
            generation: 2,
            active: true,
            block_index: 0,
            items: [ItemStack::default(); CHEST_SLOTS],
        };
        let save = ChunkSave {
            key: ChunkKey {
                dimension: OVERWORLD,
                x: 0,
                z: 0,
            },
            revision: 1,
            chunk,
        };
        assert!(matches!(encode(&save), Err(StorageError::Corrupt(_))));
    }
}
