//! Frozen planning snapshot registry and digest.
//!
//! The registry holds at most four frozen snapshots, mirroring the Go
//! `SnapshotRegistry` in `packages/shared/companion/snapshot_registry.go`.
//! Registration validates identity, deadline, and snapshot shape, encodes the
//! digest outside the owner lock, then rechecks deadline and capacity before
//! inserting. Expiry is `deadline + 5 s`; expiration is observed synchronously
//! at the next registry interaction, which keeps every outcome deterministic
//! under a fake clock. Completing, cancelling, expiring, or closing a record
//! fires its cancellation flag and invalidates the capability; every failed
//! authorization or materialization reports the same unavailable error so no
//! existence detail leaks.
//!
//! The digest is the canonical JSON of the frozen snapshot with sorted keys,
//! compact UTF-8, no trailing newline, and Go-compatible `float32` numeric
//! text, followed by lowercase SHA-256. `source_tick` and any attempt counter
//! stay private correlation and are never serialized.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use mornlea_domain::registered_block;

use crate::contracts::{
    Clock, Deadline, NamespaceId, PlanningSnapshot, Resource, ServerError, SnapshotId,
    SnapshotPort, SnapshotRegistration,
};

/// Maximum number of simultaneously live frozen snapshots.
pub const REGISTRY_CAPACITY: usize = 4;
/// Grace after the run deadline during which an in-flight bounded read may
/// still observe the snapshot.
pub const SNAPSHOT_EXPIRY_GRACE: Duration = Duration::from_secs(5);
/// Exclusive upper bound of the terrain digest JSON: reaching it fails.
pub const MAX_TERRAIN_DIGEST_BYTES: usize = 53 * 1024;
/// Inclusive upper bound of the full snapshot digest JSON: exceeding it fails.
pub const MAX_SNAPSHOT_DIGEST_BYTES: usize = 96 * 1024;

const TERRAIN_DIMENSIONS: [u8; 3] = [33, 17, 33];
const TERRAIN_READY_BYTES: usize = 137;
const TERRAIN_COLUMNS: usize = 1089;
const TERRAIN_BLOCKS: usize = 18_513;
const BLOCK_Y_MIN: i32 = -64;
const BLOCK_Y_MAX: i32 = 320;
const MAX_EXPOSED_BLOCKS: usize = 256;
const MAX_CHUNK_REVISIONS: usize = 9;
const MAX_ONLINE_PLAYERS: usize = 8;
const MAX_INSTRUCTION_BYTES: usize = 1024;
const MAX_STATUS_BYTES: usize = 96;

fn invalid(field: &'static str) -> ServerError {
    ServerError::InvalidInput { field }
}

fn unavailable() -> ServerError {
    ServerError::Agent {
        code: crate::contracts::AgentErrorCode::AgentUnavailable,
        status: crate::contracts::AgentErrorCode::AgentUnavailable.status(),
    }
}

// ---------------------------------------------------------------------------
// SHA-256 over the canonical bytes.
// ---------------------------------------------------------------------------

/// Minimal SHA-256 (FIPS 180-4) so the digest needs no extra dependency.
fn sha256(message: &[u8]) -> [u8; 32] {
    const K: [u32; 64] = [
        0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4,
        0xab1c5ed5, 0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe,
        0x9bdc06a7, 0xc19bf174, 0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f,
        0x4a7484aa, 0x5cb0a9dc, 0x76f988da, 0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7,
        0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967, 0x27b70a85, 0x2e1b2138, 0x4d2c6dfc,
        0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85, 0xa2bfe8a1, 0xa81a664b,
        0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070, 0x19a4c116,
        0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
        0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7,
        0xc67178f2,
    ];
    let mut state: [u32; 8] = [
        0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab,
        0x5be0cd19,
    ];
    let mut padded = message.to_vec();
    let bit_len = (message.len() as u64).wrapping_mul(8);
    padded.push(0x80);
    while padded.len() % 64 != 56 {
        padded.push(0);
    }
    padded.extend_from_slice(&bit_len.to_be_bytes());
    for chunk in padded.chunks_exact(64) {
        let mut schedule = [0u32; 64];
        for index in 0..16 {
            schedule[index] = u32::from_be_bytes([
                chunk[4 * index],
                chunk[4 * index + 1],
                chunk[4 * index + 2],
                chunk[4 * index + 3],
            ]);
        }
        for index in 16..64 {
            let lower = schedule[index - 15];
            let upper = schedule[index - 2];
            let small_zero = lower.rotate_right(7) ^ lower.rotate_right(18) ^ (lower >> 3);
            let small_one = upper.rotate_right(17) ^ upper.rotate_right(19) ^ (upper >> 10);
            schedule[index] = schedule[index - 16]
                .wrapping_add(small_zero)
                .wrapping_add(schedule[index - 7])
                .wrapping_add(small_one);
        }
        let mut work = state;
        for index in 0..64 {
            let big_one =
                work[4].rotate_right(6) ^ work[4].rotate_right(11) ^ work[4].rotate_right(25);
            let choice = (work[4] & work[5]) ^ ((!work[4]) & work[6]);
            let temp_one = work[7]
                .wrapping_add(big_one)
                .wrapping_add(choice)
                .wrapping_add(K[index])
                .wrapping_add(schedule[index]);
            let big_zero =
                work[0].rotate_right(2) ^ work[0].rotate_right(13) ^ work[0].rotate_right(22);
            let majority = (work[0] & work[1]) ^ (work[0] & work[2]) ^ (work[1] & work[2]);
            let temp_two = big_zero.wrapping_add(majority);
            work[7] = work[6];
            work[6] = work[5];
            work[5] = work[4];
            work[4] = work[3].wrapping_add(temp_one);
            work[3] = work[2];
            work[2] = work[1];
            work[1] = work[0];
            work[0] = temp_one.wrapping_add(temp_two);
        }
        for index in 0..8 {
            state[index] = state[index].wrapping_add(work[index]);
        }
    }
    let mut digest = [0u8; 32];
    for (index, word) in state.iter().enumerate() {
        digest[4 * index..4 * index + 4].copy_from_slice(&word.to_be_bytes());
    }
    digest
}

// ---------------------------------------------------------------------------
// Base64 planes and capability bearer.
// ---------------------------------------------------------------------------

const STD_ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
const URL_ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";

fn base64_encode(data: &[u8], alphabet: &[u8; 64], pad: bool) -> String {
    let mut text = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let word = match chunk.len() {
            3 => ((chunk[0] as u32) << 16) | ((chunk[1] as u32) << 8) | chunk[2] as u32,
            2 => ((chunk[0] as u32) << 16) | ((chunk[1] as u32) << 8),
            _ => (chunk[0] as u32) << 16,
        };
        let width = match chunk.len() {
            3 => 4,
            2 => 3,
            _ => 2,
        };
        for index in 0..width {
            text.push(alphabet[((word >> (18 - 6 * index)) & 63) as usize] as char);
        }
        if pad {
            for _ in width..4 {
                text.push('=');
            }
        }
    }
    text
}

fn base64_value(byte: u8) -> Option<u32> {
    match byte {
        b'A'..=b'Z' => Some((byte - b'A') as u32),
        b'a'..=b'z' => Some((byte - b'a' + 26) as u32),
        b'0'..=b'9' => Some((byte - b'0' + 52) as u32),
        b'-' | b'+' => Some(62),
        b'_' | b'/' => Some(63),
        _ => None,
    }
}

/// Strict unpadded base64url decode: the re-encode must reproduce the input,
/// so padded or non-canonical spellings never authenticate.
fn base64_url_decode(text: &str) -> Option<Vec<u8>> {
    if text.is_empty() || text.len() % 4 == 1 || !text.is_ascii() {
        return None;
    }
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len() / 4 * 3 + 3);
    for chunk in bytes.chunks(4) {
        let mut word = 0u32;
        for (index, byte) in chunk.iter().enumerate() {
            word |= base64_value(*byte)? << (18 - 6 * index);
        }
        // A short final quantum must have zero padding bits.
        let unused = match chunk.len() {
            4 => 0,
            3 => 2,
            2 => 4,
            _ => return None,
        };
        if word & ((1 << unused) - 1) != 0 {
            return None;
        }
        out.push((word >> 16) as u8);
        if chunk.len() > 2 {
            out.push((word >> 8) as u8);
        }
        if chunk.len() > 3 {
            out.push(word as u8);
        }
    }
    if base64_encode(&out, URL_ALPHABET, false) != text {
        return None;
    }
    Some(out)
}

/// Constant-time byte comparison for capability checks.
fn constant_time_eq(left: &[u8], right: &[u8]) -> bool {
    if left.len() != right.len() {
        return false;
    }
    let mut diff = 0u8;
    for (a, b) in left.iter().zip(right.iter()) {
        diff |= a ^ b;
    }
    diff == 0
}

// ---------------------------------------------------------------------------
// Go-compatible canonical JSON.
// ---------------------------------------------------------------------------

/// Go `encoding/json` string escaping with HTML escaping disabled: quotes,
/// backslashes, short control escapes, `\u00xx` for other C0 controls, and
/// always-escaped U+2028/U+2029. Everything else is raw UTF-8.
fn escape_into(out: &mut Vec<u8>, value: &str) {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    out.push(b'"');
    for ch in value.chars() {
        match ch {
            '"' => out.extend_from_slice(b"\\\""),
            '\\' => out.extend_from_slice(b"\\\\"),
            '\n' => out.extend_from_slice(b"\\n"),
            '\r' => out.extend_from_slice(b"\\r"),
            '\t' => out.extend_from_slice(b"\\t"),
            '\u{2028}' => out.extend_from_slice(b"\\u2028"),
            '\u{2029}' => out.extend_from_slice(b"\\u2029"),
            c if (c as u32) < 0x20 => {
                out.extend_from_slice(b"\\u00");
                out.push(HEX[(c as u8 >> 4) as usize]);
                out.push(HEX[(c as u8 & 0x0f) as usize]);
            }
            c => {
                let mut encoded = [0u8; 4];
                out.extend_from_slice(c.encode_utf8(&mut encoded).as_bytes());
            }
        }
    }
    out.push(b'"');
}

/// Go `encoding/json` spelling of one finite `float32`: the shortest
/// round-trip digits rendered plain between `1e-6` and `1e21` and scientific
/// with a signed unpadded exponent outside that range. Negative zero renders
/// as `-0`. Shared with the MCP tools so tool text and digests spell floats
/// identically.
pub(crate) fn go_float_text(value: f32) -> Result<String, ServerError> {
    if !value.is_finite() {
        return Err(invalid("snapshot_float"));
    }
    if value == 0.0 {
        return Ok(if value.is_sign_negative() {
            "-0".to_owned()
        } else {
            "0".to_owned()
        });
    }
    // Rust `Display` is the shortest decimal expansion without an exponent;
    // the transform below only repositions the point Go-style.
    let plain = format!("{value}");
    let (negative, digits) = match plain.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, plain.as_str()),
    };
    let (int_part, frac_part) = match digits.split_once('.') {
        Some((int_part, frac_part)) => (int_part, frac_part),
        None => (digits, ""),
    };
    let mut significant = format!("{int_part}{frac_part}");
    let leading = significant.len() - significant.trim_start_matches('0').len();
    significant = significant.trim_start_matches('0').to_owned();
    while significant.ends_with('0') {
        significant.pop();
    }
    debug_assert!(!significant.is_empty());
    // Value is `0.D x 10^point` with `D` the significant digits.
    let point = int_part.len() as i64 - leading as i64;
    let magnitude = (f64::from(value)).abs();
    let scientific = !(1e-6..1e21).contains(&magnitude);
    let mut rendered = String::new();
    if negative {
        rendered.push('-');
    }
    if scientific {
        let exponent = point - 1;
        rendered.push(significant.remove(0));
        if !significant.is_empty() {
            rendered.push('.');
            rendered.push_str(&significant);
        }
        rendered.push('e');
        rendered.push(if exponent < 0 { '-' } else { '+' });
        rendered.push_str(&exponent.abs().to_string());
    } else if point <= 0 {
        rendered.push_str("0.");
        for _ in point..0 {
            rendered.push('0');
        }
        rendered.push_str(&significant);
    } else {
        let point = point as usize;
        if point < significant.len() {
            rendered.push_str(&significant[..point]);
            rendered.push('.');
            rendered.push_str(&significant[point..]);
        } else {
            rendered.push_str(&significant);
            for _ in significant.len()..point {
                rendered.push('0');
            }
        }
    }
    Ok(rendered)
}

fn go_text_valid(value: &str, max_bytes: usize, require_non_empty: bool) -> bool {
    value.len() <= max_bytes
        && !value.chars().any(|ch| ch.is_control())
        && (!require_non_empty || !value.trim().is_empty())
}

fn uuid_text(bytes: [u8; 16]) -> String {
    let hex: String = bytes.iter().map(|byte| format!("{byte:02x}")).collect();
    format!(
        "{}-{}-{}-{}-{}",
        &hex[0..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..32]
    )
}

// ---------------------------------------------------------------------------
// Snapshot validation and digest.
// ---------------------------------------------------------------------------

fn valid_block_y(y: i32) -> bool {
    (BLOCK_Y_MIN..BLOCK_Y_MAX).contains(&y)
}

fn validate_snapshot(snapshot: &PlanningSnapshot) -> Result<(), ServerError> {
    if !go_text_valid(snapshot.instruction.as_str(), MAX_INSTRUCTION_BYTES, true) {
        return Err(invalid("snapshot_instruction"));
    }
    if mornlea_domain::PlayerId::try_from_bytes(snapshot.issuer.player_id.bytes()).is_err() {
        return Err(invalid("snapshot_issuer"));
    }
    let issuer_position = snapshot.issuer.position.get();
    if !issuer_position
        .iter()
        .all(|component| component.is_finite())
        || !snapshot.issuer.look.yaw().is_finite()
        || !snapshot.issuer.look.pitch().is_finite()
    {
        return Err(invalid("snapshot_issuer"));
    }
    if snapshot
        .issuer
        .look_hit
        .is_some_and(|hit| !valid_block_y(hit.y()))
    {
        return Err(invalid("snapshot_issuer"));
    }
    if mornlea_domain::CompanionId::try_from_bytes(snapshot.companion.companion_id.bytes()).is_err()
    {
        return Err(invalid("snapshot_companion"));
    }
    let companion_position = snapshot.companion.position.get();
    if !companion_position
        .iter()
        .all(|component| component.is_finite())
        || !snapshot.companion.look.yaw().is_finite()
        || !snapshot.companion.look.pitch().is_finite()
    {
        return Err(invalid("snapshot_companion"));
    }
    if !snapshot
        .companion
        .inventory
        .iter()
        .all(|slot| slot.is_valid())
    {
        return Err(invalid("snapshot_inventory"));
    }
    if !go_text_valid(
        snapshot.companion.task_status.as_str(),
        MAX_STATUS_BYTES,
        false,
    ) {
        return Err(invalid("snapshot_task_status"));
    }
    if snapshot.exposed_blocks.len() > MAX_EXPOSED_BLOCKS {
        return Err(invalid("snapshot_exposed"));
    }
    for (index, block) in snapshot.exposed_blocks.iter().enumerate() {
        if block.block_id == 0
            || !registered_block(block.block_id)
            || !valid_block_y(block.position.y())
            || (index > 0
                && (block.position.x(), block.position.y(), block.position.z())
                    <= (
                        snapshot.exposed_blocks[index - 1].position.x(),
                        snapshot.exposed_blocks[index - 1].position.y(),
                        snapshot.exposed_blocks[index - 1].position.z(),
                    ))
        {
            return Err(invalid("snapshot_exposed"));
        }
    }
    validate_terrain(snapshot)?;
    if snapshot.chunk_revisions.len() > MAX_CHUNK_REVISIONS {
        return Err(invalid("snapshot_chunks"));
    }
    for (index, revision) in snapshot.chunk_revisions.iter().enumerate() {
        if index > 0
            && (revision.pos.x(), revision.pos.z())
                <= (
                    snapshot.chunk_revisions[index - 1].pos.x(),
                    snapshot.chunk_revisions[index - 1].pos.z(),
                )
        {
            return Err(invalid("snapshot_chunks"));
        }
    }
    if snapshot.online_players.len() > MAX_ONLINE_PLAYERS {
        return Err(invalid("snapshot_players"));
    }
    for (index, player) in snapshot.online_players.iter().enumerate() {
        if mornlea_domain::PlayerId::try_from_bytes(player.player_id.bytes()).is_err() {
            return Err(invalid("snapshot_players"));
        }
        let position = player.position.get();
        if !position.iter().all(|component| component.is_finite())
            || !player.look.yaw().is_finite()
            || !player.look.pitch().is_finite()
            || player.look_hit.is_some_and(|hit| !valid_block_y(hit.y()))
            || (index > 0
                && snapshot.online_players[index - 1].player_id.bytes() >= player.player_id.bytes())
        {
            return Err(invalid("snapshot_players"));
        }
    }
    Ok(())
}

fn validate_terrain(snapshot: &PlanningSnapshot) -> Result<(), ServerError> {
    let terrain = &snapshot.terrain;
    if terrain.dimensions != TERRAIN_DIMENSIONS
        || terrain.ready_columns.len() != TERRAIN_READY_BYTES
        || terrain.heights.len() != TERRAIN_COLUMNS
        || terrain.blocks.len() != TERRAIN_BLOCKS
    {
        return Err(invalid("snapshot_terrain"));
    }
    if i64::from(terrain.origin.x()) + 33 - 1 > i64::from(i32::MAX)
        || i64::from(terrain.origin.y()) + 17 - 1 > i64::from(i32::MAX)
        || i64::from(terrain.origin.z()) + 33 - 1 > i64::from(i32::MAX)
        || terrain.ready_columns[TERRAIN_READY_BYTES - 1] & 0xfe != 0
    {
        return Err(invalid("snapshot_terrain"));
    }
    for column in 0..TERRAIN_COLUMNS {
        let ready = terrain.ready_columns[column / 8] & (1 << (column % 8)) != 0;
        let height = i32::from(terrain.heights[column]);
        if (!ready && height != BLOCK_Y_MIN - 1)
            || (ready && height != BLOCK_Y_MIN - 1 && !valid_block_y(height))
        {
            return Err(invalid("snapshot_terrain"));
        }
    }
    for (index, block) in terrain.blocks.iter().enumerate() {
        let dx = index / (17 * 33);
        let dy = (index % (17 * 33)) / 33;
        let dz = index % 33;
        let column = dx * 33 + dz;
        let y = terrain.origin.y() + dy as i32;
        let observable =
            terrain.ready_columns[column / 8] & (1 << (column % 8)) != 0 && valid_block_y(y);
        if (!observable && *block != 0) || (observable && !registered_block(*block)) {
            return Err(invalid("snapshot_terrain"));
        }
    }
    // The origin is the companion floor cell shifted by `[-16, -8, -16]`.
    let position = snapshot.companion.position.get();
    let floor = [
        (f64::from(position[0])).floor(),
        (f64::from(position[1])).floor(),
        (f64::from(position[2])).floor(),
    ];
    const MIN: f64 = i32::MIN as f64;
    const MAX: f64 = i32::MAX as f64;
    if floor[0] < MIN + 16.0
        || floor[0] > MAX - 16.0
        || floor[1] < MIN + 8.0
        || floor[1] > MAX - 8.0
        || floor[2] < MIN + 16.0
        || floor[2] > MAX - 16.0
        || terrain.origin.x() != floor[0] as i32 - 16
        || terrain.origin.y() != floor[1] as i32 - 8
        || terrain.origin.z() != floor[2] as i32 - 16
    {
        return Err(invalid("snapshot_terrain"));
    }
    Ok(())
}

struct DigestWriter {
    out: Vec<u8>,
}

impl DigestWriter {
    fn key(&mut self, key: &str) {
        escape_into(&mut self.out, key);
        self.out.push(b':');
    }

    fn text(&mut self, value: &str) {
        escape_into(&mut self.out, value);
    }

    fn int(&mut self, value: impl std::fmt::Display) {
        self.out.extend_from_slice(value.to_string().as_bytes());
    }

    fn float(&mut self, value: f32) -> Result<(), ServerError> {
        self.out.extend_from_slice(go_float_text(value)?.as_bytes());
        Ok(())
    }

    fn position(&mut self, x: i32, y: i32, z: i32) {
        self.out.push(b'{');
        self.key("x");
        self.int(x);
        self.out.push(b',');
        self.key("y");
        self.int(y);
        self.out.push(b',');
        self.key("z");
        self.int(z);
        self.out.push(b'}');
    }

    fn look_pair(
        &mut self,
        player_id: [u8; 16],
        position: [f32; 3],
        yaw: f32,
        pitch: f32,
        look_hit: Option<(i32, i32, i32)>,
    ) -> Result<(), ServerError> {
        self.out.push(b'{');
        self.key("has_look_hit");
        self.out.extend_from_slice(if look_hit.is_some() {
            b"true"
        } else {
            b"false"
        });
        self.out.push(b',');
        self.key("look_hit");
        match look_hit {
            Some((x, y, z)) => self.position(x, y, z),
            None => self.out.extend_from_slice(b"null"),
        }
        self.out.push(b',');
        self.key("pitch");
        self.float(pitch)?;
        self.out.push(b',');
        self.key("player_id");
        self.text(&uuid_text(player_id));
        self.out.push(b',');
        self.key("position");
        self.out.push(b'[');
        self.float(position[0])?;
        self.out.push(b',');
        self.float(position[1])?;
        self.out.push(b',');
        self.float(position[2])?;
        self.out.push(b']');
        self.out.push(b',');
        self.key("yaw");
        self.float(yaw)?;
        self.out.push(b'}');
        Ok(())
    }
}

/// Canonical terrain digest JSON: big-endian planes with padded standard
/// base64, strictly below [`MAX_TERRAIN_DIGEST_BYTES`].
pub fn canonical_terrain_digest(
    terrain: &crate::contracts::SnapshotTerrain,
) -> Result<Vec<u8>, ServerError> {
    // Length and dimension checks run here so the digest owns the terrain
    // shape even for callers that bypass snapshot validation.
    if terrain.dimensions != TERRAIN_DIMENSIONS
        || terrain.ready_columns.len() != TERRAIN_READY_BYTES
        || terrain.heights.len() != TERRAIN_COLUMNS
        || terrain.blocks.len() != TERRAIN_BLOCKS
    {
        return Err(invalid("snapshot_terrain"));
    }
    let mut heights = Vec::with_capacity(TERRAIN_COLUMNS * 2);
    for height in &terrain.heights {
        heights.extend_from_slice(&height.to_be_bytes());
    }
    let mut blocks = Vec::with_capacity(TERRAIN_BLOCKS * 2);
    for block in &terrain.blocks {
        blocks.extend_from_slice(&block.to_be_bytes());
    }
    let mut writer = DigestWriter { out: Vec::new() };
    writer.out.push(b'{');
    writer.key("blocks_be_u16_b64");
    writer.text(&base64_encode(&blocks, STD_ALPHABET, true));
    writer.out.push(b',');
    writer.key("dimensions");
    writer.out.extend_from_slice(b"[33,17,33]");
    writer.out.push(b',');
    writer.key("heights_be_i16_b64");
    writer.text(&base64_encode(&heights, STD_ALPHABET, true));
    writer.out.push(b',');
    writer.key("origin");
    writer.position(terrain.origin.x(), terrain.origin.y(), terrain.origin.z());
    writer.out.push(b',');
    writer.key("ready_columns_b64");
    writer.text(&base64_encode(&terrain.ready_columns, STD_ALPHABET, true));
    writer.out.push(b'}');
    if writer.out.len() >= MAX_TERRAIN_DIGEST_BYTES {
        return Err(invalid("snapshot_terrain"));
    }
    Ok(writer.out)
}

/// Canonical snapshot digest JSON with its lowercase SHA-256. The legacy
/// height list is not a field of the frozen snapshot, so only the dense
/// height plane enters the digest.
pub fn canonical_snapshot_digest(
    snapshot: &PlanningSnapshot,
) -> Result<(Vec<u8>, [u8; 32]), ServerError> {
    validate_snapshot(snapshot)?;
    let mut writer = DigestWriter { out: Vec::new() };
    writer.out.push(b'{');
    writer.key("chunk_revisions");
    writer.out.push(b'[');
    for (index, revision) in snapshot.chunk_revisions.iter().enumerate() {
        if index > 0 {
            writer.out.push(b',');
        }
        writer.out.push(b'{');
        writer.key("revision");
        writer.int(revision.revision);
        writer.out.push(b',');
        writer.key("x");
        writer.int(revision.pos.x());
        writer.out.push(b',');
        writer.key("z");
        writer.int(revision.pos.z());
        writer.out.push(b'}');
    }
    writer.out.push(b']');
    writer.out.push(b',');
    writer.key("companion");
    writer.out.push(b'{');
    writer.key("companion_id");
    writer.text(&uuid_text(snapshot.companion.companion_id.bytes()));
    writer.out.push(b',');
    writer.key("inventory");
    writer.out.push(b'[');
    let mut first_slot = true;
    for (slot, stack) in snapshot.companion.inventory.iter().enumerate() {
        if stack.item == 0 {
            continue;
        }
        if !first_slot {
            writer.out.push(b',');
        }
        first_slot = false;
        writer.out.push(b'{');
        writer.key("count");
        writer.int(stack.count);
        writer.out.push(b',');
        writer.key("durability");
        writer.int(stack.durability);
        writer.out.push(b',');
        writer.key("item_id");
        writer.int(stack.item);
        writer.out.push(b',');
        writer.key("slot");
        writer.int(slot);
        writer.out.push(b'}');
    }
    writer.out.push(b']');
    writer.out.push(b',');
    writer.key("pitch");
    writer.float(snapshot.companion.look.pitch())?;
    writer.out.push(b',');
    writer.key("position");
    let companion_position = snapshot.companion.position.get();
    writer.out.push(b'[');
    writer.float(companion_position[0])?;
    writer.out.push(b',');
    writer.float(companion_position[1])?;
    writer.out.push(b',');
    writer.float(companion_position[2])?;
    writer.out.push(b']');
    writer.out.push(b',');
    writer.key("task_status");
    writer.text(snapshot.companion.task_status.as_str());
    writer.out.push(b',');
    writer.key("yaw");
    writer.float(snapshot.companion.look.yaw())?;
    writer.out.push(b'}');
    writer.out.push(b',');
    writer.key("exposed_blocks");
    writer.out.push(b'[');
    for (index, block) in snapshot.exposed_blocks.iter().enumerate() {
        if index > 0 {
            writer.out.push(b',');
        }
        writer.out.push(b'{');
        writer.key("block_id");
        writer.int(block.block_id);
        writer.out.push(b',');
        writer.key("position");
        writer.position(block.position.x(), block.position.y(), block.position.z());
        writer.out.push(b'}');
    }
    writer.out.push(b']');
    writer.out.push(b',');
    writer.key("instruction");
    writer.text(snapshot.instruction.as_str());
    writer.out.push(b',');
    writer.key("issuer");
    {
        let issuer = &snapshot.issuer;
        let position = issuer.position.get();
        writer.look_pair(
            issuer.player_id.bytes(),
            position,
            issuer.look.yaw(),
            issuer.look.pitch(),
            issuer.look_hit.map(|hit| (hit.x(), hit.y(), hit.z())),
        )?;
    }
    writer.out.push(b',');
    writer.key("online_players");
    writer.out.push(b'[');
    for (index, player) in snapshot.online_players.iter().enumerate() {
        if index > 0 {
            writer.out.push(b',');
        }
        writer.look_pair(
            player.player_id.bytes(),
            player.position.get(),
            player.look.yaw(),
            player.look.pitch(),
            player.look_hit.map(|hit| (hit.x(), hit.y(), hit.z())),
        )?;
    }
    writer.out.push(b']');
    writer.out.push(b',');
    writer.key("terrain");
    let terrain = canonical_terrain_digest(&snapshot.terrain)?;
    writer.out.extend_from_slice(&terrain);
    writer.out.push(b',');
    writer.key("world_time_ticks");
    writer.int(snapshot.world_time_ticks);
    writer.out.push(b'}');
    if writer.out.len() > MAX_SNAPSHOT_DIGEST_BYTES {
        return Err(invalid("snapshot_digest"));
    }
    let digest = sha256(&writer.out);
    Ok((writer.out, digest))
}

// ---------------------------------------------------------------------------
// Registry.
// ---------------------------------------------------------------------------

/// Entropy source for snapshot UUIDs and the 32 capability bytes. Tests
/// inject a deterministic cycle; production uses operating-system entropy.
pub trait SnapshotEntropy: Send + Sync {
    fn fill(&self, out: &mut [u8; 32]) -> Result<(), ServerError>;
}

/// Operating-system entropy for unguessable snapshot capabilities. Failure
/// refuses registration rather than substituting predictable process state.
pub struct SystemEntropy;

impl SystemEntropy {
    pub fn new() -> Self {
        Self
    }
}

impl Default for SystemEntropy {
    fn default() -> Self {
        Self::new()
    }
}

impl SnapshotEntropy for SystemEntropy {
    fn fill(&self, out: &mut [u8; 32]) -> Result<(), ServerError> {
        getrandom::fill(out).map_err(|_| invalid("entropy"))
    }
}

struct Record {
    capability: [u8; 32],
    bearer: String,
    namespace: NamespaceId,
    companion: mornlea_domain::CompanionId,
    generation: u64,
    digest: [u8; 32],
    snapshot: PlanningSnapshot,
    deadline: Deadline,
    expires_at: Instant,
    cancelled: Arc<AtomicBool>,
}

struct Core {
    closed: bool,
    by_id: HashMap<[u8; 16], Record>,
    by_bearer: HashMap<String, [u8; 16]>,
}

struct Shared {
    core: Mutex<Core>,
    clock: Arc<dyn Clock + Send + Sync>,
    entropy: Arc<dyn SnapshotEntropy>,
    endpoint: String,
}

/// Frozen snapshot registry implementing the [`SnapshotPort`] contract.
#[derive(Clone)]
pub struct SnapshotRegistry {
    shared: Arc<Shared>,
}

impl SnapshotRegistry {
    pub fn try_new(
        clock: Arc<dyn Clock + Send + Sync>,
        entropy: Arc<dyn SnapshotEntropy>,
        mcp_endpoint: String,
    ) -> Result<Self, ServerError> {
        if mcp_endpoint.len() > 256
            || mcp_endpoint.is_empty()
            || mcp_endpoint.chars().any(|ch| ch.is_control())
        {
            return Err(invalid("mcp_endpoint"));
        }
        Ok(Self {
            shared: Arc::new(Shared {
                core: Mutex::new(Core {
                    closed: false,
                    by_id: HashMap::with_capacity(REGISTRY_CAPACITY),
                    by_bearer: HashMap::with_capacity(REGISTRY_CAPACITY),
                }),
                clock,
                entropy,
                endpoint: mcp_endpoint,
            }),
        })
    }

    pub fn endpoint(&self) -> &str {
        &self.shared.endpoint
    }

    fn reap_expired(&self, now: Instant) -> Vec<Arc<AtomicBool>> {
        let mut core = self.shared.core.lock().unwrap();
        let mut expired = Vec::new();
        core.by_id.retain(|_, record| {
            if now >= record.expires_at {
                expired.push(record.cancelled.clone());
                false
            } else {
                true
            }
        });
        if !expired.is_empty() {
            let live: Vec<[u8; 16]> = core.by_id.keys().copied().collect();
            core.by_bearer.retain(|_, id| live.contains(id));
        }
        drop(core);
        for flag in &expired {
            flag.store(true, Ordering::SeqCst);
        }
        expired
    }

    fn register_shared(
        &self,
        namespace: NamespaceId,
        companion: mornlea_domain::CompanionId,
        generation: u64,
        snapshot: PlanningSnapshot,
        deadline: Deadline,
    ) -> Result<SnapshotRegistration, ServerError> {
        if generation == 0 || snapshot.companion.companion_id != companion {
            return Err(invalid("snapshot_scope"));
        }
        let now = self.shared.clock.monotonic();
        if deadline.expired(now) {
            return Err(invalid("snapshot_deadline"));
        }
        let expires_at = deadline
            .instant()
            .checked_add(SNAPSHOT_EXPIRY_GRACE)
            .ok_or(invalid("snapshot_deadline"))?;
        self.reap_expired(now);
        {
            let core = self.shared.core.lock().unwrap();
            if core.closed {
                return Err(ServerError::InvalidState {
                    phase: crate::contracts::ServerPhase::Closed,
                });
            }
            if core.by_id.len() >= REGISTRY_CAPACITY {
                return Err(ServerError::Capacity {
                    resource: Resource::Snapshots,
                    limit: REGISTRY_CAPACITY,
                    observed: core.by_id.len(),
                });
            }
        }
        // Digest and identity work stays outside the owner lock; deadline and
        // capacity are rechecked before the record becomes visible.
        let (_, digest) = canonical_snapshot_digest(&snapshot)?;
        let frozen = snapshot;
        let mut uuid_raw = [0u8; 32];
        self.shared.entropy.fill(&mut uuid_raw)?;
        let mut uuid_bytes: [u8; 16] =
            uuid_raw[0..16].try_into().map_err(|_| invalid("entropy"))?;
        uuid_bytes[6] = uuid_bytes[6] & 0x0f | 0x40;
        uuid_bytes[8] = uuid_bytes[8] & 0x3f | 0x80;
        let id = SnapshotId::try_from_bytes(uuid_bytes).map_err(|_| invalid("entropy"))?;
        let mut capability_raw = [0u8; 32];
        self.shared.entropy.fill(&mut capability_raw)?;
        let bearer = base64_encode(&capability_raw, URL_ALPHABET, false);

        let now = self.shared.clock.monotonic();
        if deadline.expired(now) {
            return Err(invalid("snapshot_deadline"));
        }
        self.reap_expired(now);
        let mut core = self.shared.core.lock().unwrap();
        if core.closed {
            return Err(ServerError::InvalidState {
                phase: crate::contracts::ServerPhase::Closed,
            });
        }
        if core.by_id.len() >= REGISTRY_CAPACITY {
            return Err(ServerError::Capacity {
                resource: Resource::Snapshots,
                limit: REGISTRY_CAPACITY,
                observed: core.by_id.len(),
            });
        }
        if core.by_id.contains_key(&uuid_bytes) || core.by_bearer.contains_key(&bearer) {
            return Err(invalid("snapshot_identity"));
        }
        core.by_bearer.insert(bearer.clone(), uuid_bytes);
        core.by_id.insert(
            uuid_bytes,
            Record {
                capability: capability_raw,
                bearer,
                namespace,
                companion,
                generation,
                digest,
                snapshot: frozen,
                deadline,
                expires_at,
                cancelled: Arc::new(AtomicBool::new(false)),
            },
        );
        drop(core);
        SnapshotRegistration::try_new(
            id,
            digest,
            capability_raw.to_vec(),
            self.shared.endpoint.clone(),
        )
    }

    fn finish_shared(&self, id: SnapshotId) -> Result<(), ServerError> {
        let mut core = self.shared.core.lock().unwrap();
        let record = core
            .by_id
            .remove(&id.bytes())
            .ok_or(invalid("snapshot_id"))?;
        core.by_bearer.remove(&record.bearer);
        drop(core);
        record.cancelled.store(true, Ordering::SeqCst);
        Ok(())
    }

    /// Idempotent shared close for the MCP service, which holds the registry
    /// behind `Arc` and cannot take `&mut`.
    pub fn close_shared(&self) {
        let mut core = self.shared.core.lock().unwrap();
        if core.closed {
            return;
        }
        core.closed = true;
        let records: Vec<Record> = core.by_id.drain().map(|(_, record)| record).collect();
        core.by_bearer.clear();
        drop(core);
        for record in &records {
            record.cancelled.store(true, Ordering::SeqCst);
        }
    }

    /// Authorizes a bearer without copying the frozen snapshot. Every failure
    /// is the same unavailable error.
    pub fn authorize(&self, capability: &str) -> Result<SnapshotAuth, ServerError> {
        let presented = base64_url_decode(capability).ok_or_else(unavailable)?;
        if presented.len() != 32 {
            return Err(unavailable());
        }
        let now = self.shared.clock.monotonic();
        let mut core = self.shared.core.lock().unwrap();
        if core.closed {
            return Err(unavailable());
        }
        let id = core
            .by_bearer
            .get(capability)
            .copied()
            .ok_or_else(unavailable)?;
        let expired = match core.by_id.get(&id) {
            Some(record)
                if constant_time_eq(&record.capability, &presented) && now < record.expires_at =>
            {
                false
            }
            Some(_) => true,
            None => true,
        };
        if expired {
            let record = core.by_id.remove(&id);
            core.by_bearer.remove(capability);
            drop(core);
            if let Some(record) = record {
                record.cancelled.store(true, Ordering::SeqCst);
            }
            return Err(unavailable());
        }
        drop(core);
        Ok(SnapshotAuth {
            registry: Arc::downgrade(&self.shared),
            id,
        })
    }

    /// Materializes an authorization into an independent deep-copy lease.
    pub fn materialize(&self, auth: &SnapshotAuth) -> Result<SnapshotLease, ServerError> {
        let shared = auth.registry.upgrade().ok_or_else(unavailable)?;
        if !Arc::ptr_eq(&shared, &self.shared) {
            return Err(unavailable());
        }
        let now = self.shared.clock.monotonic();
        let fields = {
            let mut core = self.shared.core.lock().unwrap();
            if core.closed {
                return Err(unavailable());
            }
            let record = core.by_id.get(&auth.id).ok_or_else(unavailable)?;
            if now >= record.expires_at {
                let record = core.by_id.remove(&auth.id).ok_or_else(unavailable)?;
                core.by_bearer.remove(&record.bearer);
                let cancelled = record.cancelled.clone();
                drop(core);
                cancelled.store(true, Ordering::SeqCst);
                return Err(unavailable());
            }
            SnapshotLeaseFields {
                namespace: record.namespace,
                companion: record.companion,
                generation: record.generation,
                digest: record.digest,
                deadline: record.deadline,
                expires_at: record.expires_at,
                cancelled: record.cancelled.clone(),
            }
        };
        if fields.cancelled.load(Ordering::SeqCst) {
            return Err(unavailable());
        }
        // The stored snapshot is never mutated after insert, but presence and
        // expiry are revalidated under the lock so a concurrent finish or
        // expiry between the two passes cannot hand out a dead lease.
        let snapshot = {
            let mut core = self.shared.core.lock().unwrap();
            let record = core.by_id.get(&auth.id).ok_or_else(unavailable)?;
            if now >= record.expires_at {
                let record = core.by_id.remove(&auth.id).ok_or_else(unavailable)?;
                core.by_bearer.remove(&record.bearer);
                let cancelled = record.cancelled.clone();
                drop(core);
                cancelled.store(true, Ordering::SeqCst);
                return Err(unavailable());
            }
            record.snapshot.clone()
        };
        if fields.cancelled.load(Ordering::SeqCst) {
            return Err(unavailable());
        }
        Ok(SnapshotLease {
            id: SnapshotId::try_from_bytes(auth.id).map_err(|_| unavailable())?,
            namespace: fields.namespace,
            companion: fields.companion,
            generation: fields.generation,
            digest: fields.digest,
            snapshot,
            deadline: fields.deadline,
            expires_at: fields.expires_at,
            cancelled: fields.cancelled,
        })
    }

    /// Authorize-then-materialize for direct callers.
    pub fn lookup(&self, capability: &str) -> Result<SnapshotLease, ServerError> {
        let auth = self.authorize(capability)?;
        self.materialize(&auth)
    }
}

struct SnapshotLeaseFields {
    namespace: NamespaceId,
    companion: mornlea_domain::CompanionId,
    generation: u64,
    digest: [u8; 32],
    deadline: Deadline,
    expires_at: Instant,
    cancelled: Arc<AtomicBool>,
}

impl SnapshotPort for SnapshotRegistry {
    fn register(
        &mut self,
        namespace: NamespaceId,
        companion: mornlea_domain::CompanionId,
        generation: u64,
        snapshot: PlanningSnapshot,
        deadline: Deadline,
    ) -> Result<SnapshotRegistration, ServerError> {
        self.register_shared(namespace, companion, generation, snapshot, deadline)
    }

    fn complete(&mut self, id: SnapshotId) -> Result<(), ServerError> {
        self.finish_shared(id)
    }

    fn cancel(&mut self, id: SnapshotId) -> Result<(), ServerError> {
        self.finish_shared(id)
    }

    fn close(&mut self) -> Result<(), ServerError> {
        self.close_shared();
        Ok(())
    }
}

/// Opaque authorization from [`SnapshotRegistry::authorize`]: it names a live
/// record but carries no snapshot data.
#[derive(Clone)]
pub struct SnapshotAuth {
    registry: std::sync::Weak<Shared>,
    id: [u8; 16],
}

/// One MCP handler's deep-copy view of a frozen snapshot. `checkpoint`
/// reports the uniform unavailable error once the record completes, cancels,
/// expires, or the registry closes.
#[derive(Clone)]
pub struct SnapshotLease {
    id: SnapshotId,
    namespace: NamespaceId,
    companion: mornlea_domain::CompanionId,
    generation: u64,
    digest: [u8; 32],
    snapshot: PlanningSnapshot,
    deadline: Deadline,
    expires_at: Instant,
    cancelled: Arc<AtomicBool>,
}

impl SnapshotLease {
    pub fn checkpoint(&self) -> Result<(), ServerError> {
        if self.cancelled.load(Ordering::SeqCst) {
            return Err(unavailable());
        }
        Ok(())
    }

    pub fn snapshot_id(&self) -> SnapshotId {
        self.id
    }

    pub fn namespace_id(&self) -> NamespaceId {
        self.namespace
    }

    pub fn companion_id(&self) -> mornlea_domain::CompanionId {
        self.companion
    }

    pub fn generation(&self) -> u64 {
        self.generation
    }

    pub fn digest(&self) -> [u8; 32] {
        self.digest
    }

    pub fn digest_hex(&self) -> String {
        self.digest
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect()
    }

    pub fn snapshot(&self) -> &PlanningSnapshot {
        &self.snapshot
    }

    pub fn deadline(&self) -> Deadline {
        self.deadline
    }

    pub fn expires_at(&self) -> Instant {
        self.expires_at
    }
}
