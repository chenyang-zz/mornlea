//! Constants-only mirror of the Mornlea client-core ABI.
//!
//! The single source of truth is the C header at
//! `packages/client/cmd/mornlea-godot-core/include/mornlea_client_core.h`,
//! which the Go c-shared client core and this GDExtension consumer pin
//! independently. This module declares no Godot API usage, no unsafe code, and
//! no FFI loading; the binding surface that calls the produced library lives
//! with the bridge implementation. Unit tests here parse the header text and
//! assert that every Rust constant, struct size, member offset, and alignment
//! equals the header, in both directions, so the two sides cannot drift apart
//! silently.

// This mirror is deliberately ahead of its non-test consumers: the Rust FFI
// and bridge surfaces that read these constants and structs land with the
// later client-core tasks, and until then only the pinning tests below
// reference most items. Allow `dead_code` module-wide so the constants-only
// mirror does not fail `cargo clippy --all-targets -- -D warnings` before
// those consumers exist; remove this allowance once production code consumes
// the module directly.
#![allow(dead_code)]

/// Client-core ABI major version. A new major is required whenever an existing
/// layout or semantic changes; compatible additions raise the minor only.
pub const ABI_MAJOR: u32 = 1;

/// Client-core ABI minor version. Compatible additions (new families, new
/// records, skippable fields) raise this value together with the affected
/// family version.
pub const ABI_MINOR: u32 = 1;

/// Success. No other status writes any output byte, except reporting the
/// required size for the two-phase capacity signal.
pub const STATUS_OK: u32 = 0;

/// Null, misaligned, overlapping, or oversized pointer and length arguments.
pub const STATUS_INVALID_ARGUMENT: u32 = 1;

/// Caller ABI major, magic, or record identity does not match the producer.
pub const STATUS_ABI_MISMATCH: u32 = 2;

/// Syntactically readable buffer whose content violates the family domain;
/// the whole batch is rejected and no state is consumed.
pub const STATUS_INPUT_REJECTED: u32 = 3;

/// Two-phase capacity signal: the output buffer is too small, the required
/// byte count is reported, and nothing is written.
pub const STATUS_INSUFFICIENT_CAPACITY: u32 = 4;

/// Unknown or wrong-type handle.
pub const STATUS_INVALID_HANDLE: u32 = 5;

/// Correct handle in the wrong lifecycle phase or epoch.
pub const STATUS_INVALID_STATE: u32 = 6;

/// The session already reached its terminal disconnect.
pub const STATUS_DISCONNECTED: u32 = 7;

/// Producer-internal failure without a narrower stable classification.
pub const STATUS_INTERNAL: u32 = 8;

/// A recovered panic converted at the ABI boundary; no output is written.
pub const STATUS_PANIC: u32 = 9;

/// Number of defined status codes; new codes append, none is repurposed.
pub const STATUS_COUNT: u32 = 10;

/// Low 16 bits of every magic tag: the ASCII bytes "MC" (client core).
pub const MAGIC_TAG_PREFIX: u32 = 0x434D;

/// High byte of every magic tag: the layout-era digit, ASCII "1" for majors
/// of generation 1. It moves only with a new ABI major.
pub const MAGIC_GENERATION: u32 = 0x31;

/// Magic tag of the identity/lifecycle family; wire bytes read "MCI1".
pub const MAGIC_IDENTITY: u32 = 0x3149434D;

/// Magic tag of the connection family; wire bytes read "MCC1".
pub const MAGIC_CONNECTION: u32 = 0x3143434D;

/// Magic tag of the input family; wire bytes read "MCN1".
pub const MAGIC_INPUT: u32 = 0x314E434D;

/// Magic tag of the step family; wire bytes read "MCS1".
pub const MAGIC_STEP: u32 = 0x3153434D;

/// Magic tag of the world family; wire bytes read "MCW1".
pub const MAGIC_WORLD: u32 = 0x3157434D;

/// Magic tag of the frame family; wire bytes read "MCF1".
pub const MAGIC_FRAME: u32 = 0x3146434D;

/// Magic tag of the status/metrics family; wire bytes read "MCM1".
pub const MAGIC_STATUS: u32 = 0x314D434D;

/// Magic tag of the additive environment projection family; wire bytes read
/// "MCE1" and its payload is tied to a retained frame revision/epoch.
pub const MAGIC_ENVIRONMENT: u32 = 0x3145434D;

/// Byte alignment of every record buffer and size granularity of every fixed
/// header, so concatenated header-plus-payload sequences stay aligned.
pub const ABI_ALIGNMENT: usize = 8;

/// Stable identifier of the identity/lifecycle family.
pub const FAMILY_IDENTITY: u32 = 1;

/// Stable identifier of the connection family.
pub const FAMILY_CONNECTION: u32 = 2;

/// Stable identifier of the input family.
pub const FAMILY_INPUT: u32 = 3;

/// Stable identifier of the step family.
pub const FAMILY_STEP: u32 = 4;

/// Stable identifier of the world family.
pub const FAMILY_WORLD: u32 = 5;

/// Stable identifier of the frame family.
pub const FAMILY_FRAME: u32 = 6;

/// Stable identifier of the status/metrics family.
pub const FAMILY_STATUS: u32 = 7;

/// Number of feature families defined by the pilot contract.
pub const FAMILY_COUNT: u32 = 8;
/// Stable identifier of the additive environment projection family.
pub const FAMILY_ENVIRONMENT: u32 = 8;
/// Contract version of the environment projection family.
pub const ENVIRONMENT_VERSION: u32 = 1;
/// Fixed byte size of one environment projection record.
pub const ENVIRONMENT_BYTES: usize = 48;

/// Contract version of the identity/lifecycle family.
pub const IDENTITY_VERSION: u32 = 1;

/// Contract version of the connection family.
pub const CONNECTION_VERSION: u32 = 1;

/// Contract version of the input family.
pub const INPUT_VERSION: u32 = 1;

/// Contract version of the step family.
pub const STEP_VERSION: u32 = 1;

/// Contract version of the world family.
pub const WORLD_VERSION: u32 = 1;

/// Existing world records use zero-based storage indices for section Y.
pub const SECTION_EDGE_BLOCKS: u32 = 16;
/// Subtract this bias after converting a storage index into blocks.
pub const WORLD_Y_BIAS_BLOCKS: u32 = 64;

/// Contract version of the frame family.
pub const FRAME_VERSION: u32 = 1;

/// Contract version of the status/metrics family.
pub const STATUS_VERSION: u32 = 1;

/// Maximum device events in one input batch; a larger batch is rejected whole.
pub const MAX_INPUT_EVENTS: u32 = 128;

/// Maximum UTF-8 bytes of one "host:port" connection address.
pub const MAX_CONNECTION_ADDRESS_BYTES: u32 = 256;

/// Maximum receiver polls in one step, mirroring the runtime step budget.
pub const MAX_STEP_MESSAGE_BUDGET: u32 = 4096;

/// Maximum ready sections drained in one step, mirroring the world batch
/// operation limit.
pub const MAX_STEP_MESH_BUDGET: u32 = 4096;

/// Maximum upsert plus drop operations in one world batch.
pub const MAX_WORLD_BATCH_OPERATIONS: u32 = 4096;

/// Maximum packed quads in one section mesh payload.
pub const MAX_SECTION_MESH_QUADS: u32 = 24576;

/// Maximum packed quads in one whole world batch.
pub const MAX_WORLD_BATCH_QUADS: u32 = 524288;

/// Maximum entity records in one frame snapshot.
pub const MAX_ENTITY_RECORDS: u32 = 7;

/// Maximum UTF-8 bytes of the frame target-name payload.
pub const MAX_TARGET_NAME_BYTES: u32 = 64;

/// Maximum records in one status/metrics pull.
pub const MAX_STATUS_RECORDS: u32 = 64;

/// Version of the frame snapshot aggregate layout carried inside frame
/// records; equal to the frame family version for the pilot generation.
pub const FRAME_SNAPSHOT_VERSION: u32 = 1;

/// Wire size of `MornleaClientIdentityHeader`.
pub const IDENTITY_HEADER_BYTES: usize = 24;

/// Wire size of `MornleaClientFamilyDescriptor`.
pub const FAMILY_DESCRIPTOR_BYTES: usize = 24;

/// Wire size of `MornleaClientConnectionHeader`.
pub const CONNECTION_HEADER_BYTES: usize = 16;

/// Wire size of `MornleaClientInputHeader`.
pub const INPUT_HEADER_BYTES: usize = 16;

/// Wire size of `MornleaClientStepRequest`.
pub const STEP_REQUEST_BYTES: usize = 24;

/// Wire size of `MornleaClientWorldHeader`.
pub const WORLD_HEADER_BYTES: usize = 32;

/// Wire size of `MornleaClientFrameHeader`.
pub const FRAME_HEADER_BYTES: usize = 40;

/// Wire size of `MornleaClientStatusHeader`.
pub const STATUS_HEADER_BYTES: usize = 16;

/// Fixed header of the identity/lifecycle family: the producer reports its ABI
/// identity followed by `family_count` descriptor records.
///
/// Wire layout (little-endian): magic u32, layout u32, abi_major u32,
/// abi_minor u32, family_count u32, reserved u32.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct IdentityHeader {
    pub magic: u32,
    pub layout: u32,
    pub abi_major: u32,
    pub abi_minor: u32,
    pub family_count: u32,
    pub reserved: u32,
}

/// One feature-family descriptor record: stable family identifier, its
/// contract version, the bounded record limit (0 when unbounded or not
/// applicable), the fixed per-record wire size (0 for variable-length
/// records), and reserved zero padding.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct FamilyDescriptor {
    pub family: u32,
    pub version: u32,
    pub record_limit: u32,
    pub record_bytes: u32,
    pub reserved: [u32; 2],
}

/// Fixed header of the connection family: a begin-connect request whose
/// variable payload is `address_len` UTF-8 "host:port" bytes.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct ConnectionHeader {
    pub magic: u32,
    pub layout: u32,
    pub address_len: u32,
    pub reserved: u32,
}

/// Fixed header of the input family: one per-frame event batch whose record
/// count is bounded by [`MAX_INPUT_EVENTS`].
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct InputHeader {
    pub magic: u32,
    pub layout: u32,
    pub event_count: u32,
    pub reserved: u32,
}

/// Fixed request record of the step family: one bounded step driven only by
/// the explicit elapsed nanoseconds and the two budgets. No implicit wall
/// clock and no network or GPU wait participates in a step.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct StepRequest {
    pub magic: u32,
    pub layout: u32,
    pub elapsed_ns: u64,
    pub message_budget: u32,
    pub mesh_budget: u32,
}

/// Fixed header of the world family: one all-or-nothing section batch with
/// monotonic epoch and atlas revision, bounded operation and packed-quad
/// counts, published through the two-phase capacity protocol.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct WorldHeader {
    pub magic: u32,
    pub layout: u32,
    pub operation_count: u32,
    pub quad_count: u32,
    pub epoch: u64,
    pub atlas_revision: u64,
}

/// Fixed header of the frame family: the per-step semantic snapshot identity,
/// entity record count, and target-name length, followed by the family
/// payload records.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct FrameHeader {
    pub magic: u32,
    pub layout: u32,
    pub frame_version: u32,
    pub entity_count: u32,
    pub name_len: u32,
    pub reserved: u32,
    pub revision: u64,
    pub epoch: u64,
}

/// Fixed header of the status/metrics family: a bounded pull of stable error,
/// counter, and timing-sample records.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct StatusHeader {
    pub magic: u32,
    pub layout: u32,
    pub record_count: u32,
    pub reserved: u32,
}

#[cfg(test)]
mod tests {
    use super::{
        ABI_ALIGNMENT, ABI_MAJOR, ABI_MINOR, CONNECTION_HEADER_BYTES, CONNECTION_VERSION,
        ConnectionHeader, ENVIRONMENT_BYTES, ENVIRONMENT_VERSION, FAMILY_CONNECTION, FAMILY_COUNT,
        FAMILY_DESCRIPTOR_BYTES, FAMILY_ENVIRONMENT, FAMILY_FRAME, FAMILY_IDENTITY, FAMILY_INPUT,
        FAMILY_STATUS, FAMILY_STEP, FAMILY_WORLD, FRAME_HEADER_BYTES, FRAME_SNAPSHOT_VERSION,
        FRAME_VERSION, FamilyDescriptor, FrameHeader, IDENTITY_HEADER_BYTES, IDENTITY_VERSION,
        INPUT_HEADER_BYTES, INPUT_VERSION, IdentityHeader, InputHeader, MAGIC_CONNECTION,
        MAGIC_ENVIRONMENT, MAGIC_FRAME, MAGIC_GENERATION, MAGIC_IDENTITY, MAGIC_INPUT,
        MAGIC_STATUS, MAGIC_STEP, MAGIC_TAG_PREFIX, MAGIC_WORLD, MAX_CONNECTION_ADDRESS_BYTES,
        MAX_ENTITY_RECORDS, MAX_INPUT_EVENTS, MAX_SECTION_MESH_QUADS, MAX_STATUS_RECORDS,
        MAX_STEP_MESH_BUDGET, MAX_STEP_MESSAGE_BUDGET, MAX_TARGET_NAME_BYTES,
        MAX_WORLD_BATCH_OPERATIONS, MAX_WORLD_BATCH_QUADS, STATUS_ABI_MISMATCH, STATUS_COUNT,
        STATUS_DISCONNECTED, STATUS_HEADER_BYTES, STATUS_INPUT_REJECTED,
        STATUS_INSUFFICIENT_CAPACITY, STATUS_INTERNAL, STATUS_INVALID_ARGUMENT,
        STATUS_INVALID_HANDLE, STATUS_INVALID_STATE, STATUS_OK, STATUS_PANIC, STATUS_VERSION,
        STEP_REQUEST_BYTES, STEP_VERSION, StatusHeader, StepRequest, WORLD_HEADER_BYTES,
        WORLD_VERSION, WorldHeader,
    };
    use core::mem::{align_of, offset_of, size_of};
    use std::collections::BTreeMap;

    const HEADER_TEXT: &str =
        include_str!("../../../../client/cmd/mornlea-godot-core/include/mornlea_client_core.h");

    /// Parse every numeric `#define MORNLEA_CLIENT_*` from the header text.
    /// Header defines stay single-line literals (decimal or 0x hex, optional
    /// `u` suffix) so both the Go and Rust parsers read them without a C
    /// preprocessor. A decimal literal with a redundant leading zero is
    /// rejected as ambiguous: the Go parser reads base 0, where a leading
    /// zero means octal, so such a literal could pass both set-equality
    /// checks while C/Go and Rust disagree on its value.
    fn header_defines() -> BTreeMap<String, u32> {
        let mut defines = BTreeMap::new();
        for raw in HEADER_TEXT.lines() {
            let line = raw.trim();
            let Some(rest) = line.strip_prefix("#define ") else {
                continue;
            };
            if !rest.starts_with("MORNLEA_CLIENT_") {
                continue;
            }
            let mut fields = rest.split_whitespace();
            let name = fields.next().expect("define name").to_string();
            if name == "MORNLEA_CLIENT_CORE_H" {
                // The include guard carries no value; skip it.
                continue;
            }
            let value = fields.next().expect("define value");
            let value = value.strip_suffix('u').unwrap_or(value);
            let parsed = if let Some(hex) = value.strip_prefix("0x") {
                u32::from_str_radix(hex, 16).expect("hex define value")
            } else {
                assert!(
                    value.len() == 1 || !value.starts_with('0'),
                    "ambiguous literal {value}: decimal defines must not carry a leading zero"
                );
                value.parse::<u32>().expect("decimal define value")
            };
            assert!(
                defines.insert(name.clone(), parsed).is_none(),
                "header defines {name} twice"
            );
        }
        assert!(!defines.is_empty(), "header defines were not parsed");
        defines
    }

    /// The complete name-to-value map of constants this module must mirror.
    fn expected_defines() -> BTreeMap<&'static str, u32> {
        let expected: &[(&str, u32)] = &[
            ("MORNLEA_CLIENT_ABI_MAJOR", ABI_MAJOR),
            ("MORNLEA_CLIENT_ABI_MINOR", ABI_MINOR),
            ("MORNLEA_CLIENT_STATUS_OK", STATUS_OK),
            (
                "MORNLEA_CLIENT_STATUS_INVALID_ARGUMENT",
                STATUS_INVALID_ARGUMENT,
            ),
            ("MORNLEA_CLIENT_STATUS_ABI_MISMATCH", STATUS_ABI_MISMATCH),
            (
                "MORNLEA_CLIENT_STATUS_INPUT_REJECTED",
                STATUS_INPUT_REJECTED,
            ),
            (
                "MORNLEA_CLIENT_STATUS_INSUFFICIENT_CAPACITY",
                STATUS_INSUFFICIENT_CAPACITY,
            ),
            (
                "MORNLEA_CLIENT_STATUS_INVALID_HANDLE",
                STATUS_INVALID_HANDLE,
            ),
            ("MORNLEA_CLIENT_STATUS_INVALID_STATE", STATUS_INVALID_STATE),
            ("MORNLEA_CLIENT_STATUS_DISCONNECTED", STATUS_DISCONNECTED),
            ("MORNLEA_CLIENT_STATUS_INTERNAL", STATUS_INTERNAL),
            ("MORNLEA_CLIENT_STATUS_PANIC", STATUS_PANIC),
            ("MORNLEA_CLIENT_STATUS_COUNT", STATUS_COUNT),
            ("MORNLEA_CLIENT_MAGIC_TAG_PREFIX", MAGIC_TAG_PREFIX),
            ("MORNLEA_CLIENT_MAGIC_GENERATION", MAGIC_GENERATION),
            ("MORNLEA_CLIENT_MAGIC_IDENTITY", MAGIC_IDENTITY),
            ("MORNLEA_CLIENT_MAGIC_CONNECTION", MAGIC_CONNECTION),
            ("MORNLEA_CLIENT_MAGIC_INPUT", MAGIC_INPUT),
            ("MORNLEA_CLIENT_MAGIC_STEP", MAGIC_STEP),
            ("MORNLEA_CLIENT_MAGIC_WORLD", MAGIC_WORLD),
            ("MORNLEA_CLIENT_MAGIC_FRAME", MAGIC_FRAME),
            ("MORNLEA_CLIENT_MAGIC_STATUS", MAGIC_STATUS),
            ("MORNLEA_CLIENT_MAGIC_ENVIRONMENT", MAGIC_ENVIRONMENT),
            ("MORNLEA_CLIENT_ABI_ALIGNMENT", ABI_ALIGNMENT as u32),
            ("MORNLEA_CLIENT_FAMILY_IDENTITY", FAMILY_IDENTITY),
            ("MORNLEA_CLIENT_FAMILY_CONNECTION", FAMILY_CONNECTION),
            ("MORNLEA_CLIENT_FAMILY_INPUT", FAMILY_INPUT),
            ("MORNLEA_CLIENT_FAMILY_STEP", FAMILY_STEP),
            ("MORNLEA_CLIENT_FAMILY_WORLD", FAMILY_WORLD),
            ("MORNLEA_CLIENT_FAMILY_FRAME", FAMILY_FRAME),
            ("MORNLEA_CLIENT_FAMILY_STATUS", FAMILY_STATUS),
            ("MORNLEA_CLIENT_FAMILY_ENVIRONMENT", FAMILY_ENVIRONMENT),
            ("MORNLEA_CLIENT_FAMILY_COUNT", FAMILY_COUNT),
            ("MORNLEA_CLIENT_IDENTITY_VERSION", IDENTITY_VERSION),
            ("MORNLEA_CLIENT_CONNECTION_VERSION", CONNECTION_VERSION),
            ("MORNLEA_CLIENT_INPUT_VERSION", INPUT_VERSION),
            ("MORNLEA_CLIENT_STEP_VERSION", STEP_VERSION),
            ("MORNLEA_CLIENT_WORLD_VERSION", WORLD_VERSION),
            (
                "MORNLEA_CLIENT_SECTION_EDGE_BLOCKS",
                super::SECTION_EDGE_BLOCKS,
            ),
            (
                "MORNLEA_CLIENT_WORLD_Y_BIAS_BLOCKS",
                super::WORLD_Y_BIAS_BLOCKS,
            ),
            ("MORNLEA_CLIENT_FRAME_VERSION", FRAME_VERSION),
            ("MORNLEA_CLIENT_STATUS_VERSION", STATUS_VERSION),
            ("MORNLEA_CLIENT_ENVIRONMENT_VERSION", ENVIRONMENT_VERSION),
            ("MORNLEA_CLIENT_ENVIRONMENT_BYTES", ENVIRONMENT_BYTES as u32),
            ("MORNLEA_CLIENT_MAX_INPUT_EVENTS", MAX_INPUT_EVENTS),
            (
                "MORNLEA_CLIENT_MAX_CONNECTION_ADDRESS_BYTES",
                MAX_CONNECTION_ADDRESS_BYTES,
            ),
            (
                "MORNLEA_CLIENT_MAX_STEP_MESSAGE_BUDGET",
                MAX_STEP_MESSAGE_BUDGET,
            ),
            ("MORNLEA_CLIENT_MAX_STEP_MESH_BUDGET", MAX_STEP_MESH_BUDGET),
            (
                "MORNLEA_CLIENT_MAX_WORLD_BATCH_OPERATIONS",
                MAX_WORLD_BATCH_OPERATIONS,
            ),
            (
                "MORNLEA_CLIENT_MAX_SECTION_MESH_QUADS",
                MAX_SECTION_MESH_QUADS,
            ),
            (
                "MORNLEA_CLIENT_MAX_WORLD_BATCH_QUADS",
                MAX_WORLD_BATCH_QUADS,
            ),
            ("MORNLEA_CLIENT_MAX_ENTITY_RECORDS", MAX_ENTITY_RECORDS),
            (
                "MORNLEA_CLIENT_MAX_TARGET_NAME_BYTES",
                MAX_TARGET_NAME_BYTES,
            ),
            ("MORNLEA_CLIENT_MAX_STATUS_RECORDS", MAX_STATUS_RECORDS),
            (
                "MORNLEA_CLIENT_FRAME_SNAPSHOT_VERSION",
                FRAME_SNAPSHOT_VERSION,
            ),
            (
                "MORNLEA_CLIENT_IDENTITY_HEADER_BYTES",
                IDENTITY_HEADER_BYTES as u32,
            ),
            (
                "MORNLEA_CLIENT_FAMILY_DESCRIPTOR_BYTES",
                FAMILY_DESCRIPTOR_BYTES as u32,
            ),
            (
                "MORNLEA_CLIENT_CONNECTION_HEADER_BYTES",
                CONNECTION_HEADER_BYTES as u32,
            ),
            (
                "MORNLEA_CLIENT_INPUT_HEADER_BYTES",
                INPUT_HEADER_BYTES as u32,
            ),
            (
                "MORNLEA_CLIENT_STEP_REQUEST_BYTES",
                STEP_REQUEST_BYTES as u32,
            ),
            (
                "MORNLEA_CLIENT_WORLD_HEADER_BYTES",
                WORLD_HEADER_BYTES as u32,
            ),
            (
                "MORNLEA_CLIENT_FRAME_HEADER_BYTES",
                FRAME_HEADER_BYTES as u32,
            ),
            (
                "MORNLEA_CLIENT_STATUS_HEADER_BYTES",
                STATUS_HEADER_BYTES as u32,
            ),
        ];
        expected.iter().copied().collect()
    }

    #[test]
    fn abi_header_defines_match_rust_constants() {
        let expected = expected_defines();
        let parsed = header_defines();
        for (name, want) in &expected {
            let Some(got) = parsed.get(*name) else {
                panic!("header is missing define {name}");
            };
            assert_eq!(got, want, "define {name}");
        }
        for name in parsed.keys() {
            assert!(
                expected.contains_key(name.as_str()),
                "header define {name} has no pinned Rust constant"
            );
        }
    }

    #[test]
    fn abi_identity_pins_major_minor_status_order_and_families() {
        assert_eq!((ABI_MAJOR, ABI_MINOR), (1, 1));
        let statuses = [
            STATUS_OK,
            STATUS_INVALID_ARGUMENT,
            STATUS_ABI_MISMATCH,
            STATUS_INPUT_REJECTED,
            STATUS_INSUFFICIENT_CAPACITY,
            STATUS_INVALID_HANDLE,
            STATUS_INVALID_STATE,
            STATUS_DISCONNECTED,
            STATUS_INTERNAL,
            STATUS_PANIC,
        ];
        for (index, status) in statuses.iter().enumerate() {
            assert_eq!(*status, index as u32, "status {index}");
        }
        assert_eq!(STATUS_COUNT, statuses.len() as u32);

        let families = [
            FAMILY_IDENTITY,
            FAMILY_CONNECTION,
            FAMILY_INPUT,
            FAMILY_STEP,
            FAMILY_WORLD,
            FAMILY_FRAME,
            FAMILY_STATUS,
            FAMILY_ENVIRONMENT,
        ];
        for (index, family) in families.iter().enumerate() {
            assert_eq!(*family, index as u32 + 1, "family {index}");
        }
        assert_eq!(FAMILY_COUNT, families.len() as u32);
        assert_eq!(ABI_ALIGNMENT, 8);

        let versions = [
            IDENTITY_VERSION,
            CONNECTION_VERSION,
            INPUT_VERSION,
            STEP_VERSION,
            WORLD_VERSION,
            FRAME_VERSION,
            STATUS_VERSION,
            ENVIRONMENT_VERSION,
        ];
        assert!(versions.iter().all(|version| *version == 1));
    }

    #[test]
    fn abi_magic_tags_compose_from_prefix_family_and_generation() {
        let tags = [
            (MAGIC_IDENTITY, b'I'),
            (MAGIC_CONNECTION, b'C'),
            (MAGIC_INPUT, b'N'),
            (MAGIC_STEP, b'S'),
            (MAGIC_WORLD, b'W'),
            (MAGIC_FRAME, b'F'),
            (MAGIC_STATUS, b'M'),
        ];
        let mut seen = std::collections::BTreeSet::new();
        for (magic, family_char) in tags {
            let want = MAGIC_TAG_PREFIX | u32::from(family_char) << 16 | MAGIC_GENERATION << 24;
            assert_eq!(magic, want, "magic for family char {}", family_char);
            assert!(seen.insert(magic), "magic {magic:#x} is reused");
        }
        // The generation digit is tied to the ABI major: generation "1"
        // covers every major-1 layout era and moves only with a new major.
        assert_eq!(MAGIC_GENERATION, u32::from(b'0') + ABI_MAJOR);
    }

    #[test]
    fn abi_limits_pin_the_frozen_contract_values() {
        // Limits mirrored from frozen Go constants keep their exact values
        // here as a second, language-independent pin: world batch operations
        // and the step mesh budget come from the presentation world batch
        // bound, the step message budget from the runtime step drain bound,
        // section quads from the mesher worst case, batch quads from the
        // 4 MiB packed payload bound, entity records from the remote-player
        // batch capacity, and the snapshot version from the frame aggregate.
        assert_eq!(MAX_WORLD_BATCH_OPERATIONS, 4096);
        assert_eq!(MAX_STEP_MESH_BUDGET, 4096);
        assert_eq!(MAX_STEP_MESSAGE_BUDGET, 4096);
        assert_eq!(MAX_SECTION_MESH_QUADS, 6 * 4096);
        assert_eq!(MAX_WORLD_BATCH_QUADS, 4 * 1024 * 1024 / 8);
        assert_eq!(MAX_ENTITY_RECORDS, 7);
        assert_eq!(FRAME_SNAPSHOT_VERSION, 1);
        // Pilot-only bounds without a frozen Go counterpart stay explicit so
        // any later change is a reviewed contract decision.
        assert_eq!(MAX_INPUT_EVENTS, 128);
        assert_eq!(MAX_CONNECTION_ADDRESS_BYTES, 256);
        assert_eq!(MAX_TARGET_NAME_BYTES, 64);
        assert_eq!(MAX_STATUS_RECORDS, 64);
    }

    #[test]
    fn abi_layout_mirrors_match_declared_sizes_offsets_and_alignment() {
        let sizes: [(&str, usize, usize); 8] = [
            (
                "identity header",
                size_of::<IdentityHeader>(),
                IDENTITY_HEADER_BYTES,
            ),
            (
                "family descriptor",
                size_of::<FamilyDescriptor>(),
                FAMILY_DESCRIPTOR_BYTES,
            ),
            (
                "connection header",
                size_of::<ConnectionHeader>(),
                CONNECTION_HEADER_BYTES,
            ),
            ("input header", size_of::<InputHeader>(), INPUT_HEADER_BYTES),
            ("step request", size_of::<StepRequest>(), STEP_REQUEST_BYTES),
            ("world header", size_of::<WorldHeader>(), WORLD_HEADER_BYTES),
            ("frame header", size_of::<FrameHeader>(), FRAME_HEADER_BYTES),
            (
                "status header",
                size_of::<StatusHeader>(),
                STATUS_HEADER_BYTES,
            ),
        ];
        for (name, size, declared) in sizes {
            assert_eq!(size, declared, "{name} size");
            assert_eq!(
                size % ABI_ALIGNMENT,
                0,
                "{name} size is not a multiple of the ABI alignment"
            );
        }

        assert_eq!(align_of::<IdentityHeader>(), 4);
        assert_eq!(align_of::<FamilyDescriptor>(), 4);
        assert_eq!(align_of::<ConnectionHeader>(), 4);
        assert_eq!(align_of::<InputHeader>(), 4);
        assert_eq!(align_of::<StepRequest>(), 8);
        assert_eq!(align_of::<WorldHeader>(), 8);
        assert_eq!(align_of::<FrameHeader>(), 8);
        assert_eq!(align_of::<StatusHeader>(), 4);

        let offsets: [(&str, usize); 14] = [
            ("identity abi_major", offset_of!(IdentityHeader, abi_major)),
            (
                "identity family_count",
                offset_of!(IdentityHeader, family_count),
            ),
            (
                "descriptor record_limit",
                offset_of!(FamilyDescriptor, record_limit),
            ),
            (
                "connection address_len",
                offset_of!(ConnectionHeader, address_len),
            ),
            ("input event_count", offset_of!(InputHeader, event_count)),
            ("step elapsed_ns", offset_of!(StepRequest, elapsed_ns)),
            (
                "step message_budget",
                offset_of!(StepRequest, message_budget),
            ),
            ("world epoch", offset_of!(WorldHeader, epoch)),
            (
                "world atlas_revision",
                offset_of!(WorldHeader, atlas_revision),
            ),
            (
                "frame frame_version",
                offset_of!(FrameHeader, frame_version),
            ),
            ("frame name_len", offset_of!(FrameHeader, name_len)),
            ("frame revision", offset_of!(FrameHeader, revision)),
            ("frame epoch", offset_of!(FrameHeader, epoch)),
            (
                "status record_count",
                offset_of!(StatusHeader, record_count),
            ),
        ];
        let expected_offsets: [usize; 14] = [8, 16, 8, 8, 8, 8, 16, 16, 24, 8, 16, 24, 32, 8];
        for ((name, offset), want) in offsets.iter().zip(expected_offsets) {
            assert_eq!(*offset, want, "{name} offset");
        }

        // 64-bit members appear only at 8-byte offsets so a naturally aligned
        // buffer keeps them aligned in any header-plus-payload sequence.
        for offset in [
            offset_of!(StepRequest, elapsed_ns),
            offset_of!(WorldHeader, epoch),
            offset_of!(WorldHeader, atlas_revision),
            offset_of!(FrameHeader, revision),
            offset_of!(FrameHeader, epoch),
        ] {
            assert_eq!(offset % ABI_ALIGNMENT, 0, "u64 offset {offset}");
        }
    }
}
