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

/// The rust-producer boundary layer: the engine-independent checked value
/// model, the canonical text and hex codecs, the private core-token value,
/// the closed failure envelope, and the decode/render routines that turn
/// host-owned boundary values into real C1/C2 checked values and back.
///
/// Nothing in this section names a Godot type. The bridge marshals native
/// dictionaries into [`BoundaryValue`] copies and calls the routines here,
/// so the engine-free adapter tests exercise exactly the code the exported
/// Godot methods run; a test-specific second implementation is structurally
/// impossible because both sides share these declarations.
///
/// Boundary rules frozen by the facade contract:
/// - every `u64` is canonical decimal text (no sign, no leading zero, `"0"`
///   for zero, at most twenty digits, at most `u64::MAX`);
/// - UUID identities are exactly 32 lowercase hex digits and digests are
///   fixed-length lowercase hex;
/// - typed leaves never coerce: a boolean field rejects an integer, a float
///   must be finite, and every struct field set is exact (no extra, no
///   missing);
/// - family/type tags are closed and case-sensitive.
pub mod boundary {
    use mornlea_client_core::contracts::CloseReason;
    use mornlea_client_core::contracts::InputReceipt;
    use mornlea_client_core::contracts::StepReport;
    use mornlea_client_core::contracts::{
        ClientError, ClientIdentity, ClientLimits, ClientWorkBudget, ConfirmedRevision, Endpoint,
        SessionEpoch,
    };
    use mornlea_client_core::input::{
        ClientIntent, ClientIntentKind, ContainerToken, CraftingViewToken, InputAction, InputBatch,
    };
    use mornlea_client_core::presentation::frame::{FamilyFrame, FamilyRecords, PresentationFrame};
    use mornlea_client_core::presentation::{
        CueProvenance, InputReceiptState, InventoryUiView, WorldUiView,
    };
    use mornlea_domain::{
        ChatBody, ChatIntent, CommandText, ContainerKind, ContainerMove, ContainerRef,
        CraftingMove, CraftingSize, HeldActions, HotbarSlot, InventoryMove, ItemStack, LookAngles,
        MiningState, PartialMove, PlacementIntent, PlayerControl, PlayerControlParts, PlayerId,
        ResyncIntent, StackSource, StackView, TaskState,
    };
    use mornlea_protocol::LoginStart;

    // ------------------------------------------------------------------
    // The engine-neutral value model
    // ------------------------------------------------------------------

    /// One host-owned boundary leaf, struct, vector or tagged union copied
    /// out of the native marshalling before any decoding runs.
    ///
    /// The closed leaf set is exactly what the facade contract admits:
    /// booleans, checked integers (every signed or small unsigned field),
    /// finite floats, text, vectors, struct field lists and tagged unions.
    /// There is no null-int coercion and no absent-field default: `Null` is
    /// the option marker only.
    #[derive(Clone, Debug, PartialEq)]
    pub enum BoundaryValue {
        Null,
        Bool(bool),
        Int(i64),
        Float(f64),
        Text(String),
        List(Vec<BoundaryValue>),
        Fields(Vec<(String, BoundaryValue)>),
    }

    impl BoundaryValue {
        /// Builds a struct value from field pairs.
        pub fn fields<const N: usize>(pairs: [(&str, BoundaryValue); N]) -> Self {
            Self::Fields(
                pairs
                    .into_iter()
                    .map(|(name, value)| (name.to_string(), value))
                    .collect(),
            )
        }

        /// Builds a tagged union value: exactly `tag` and `value`.
        pub fn tagged(tag: &str, value: BoundaryValue) -> Self {
            Self::fields([("tag", Self::Text(tag.to_string())), ("value", value)])
        }

        /// The named field of a struct value.
        pub fn field(&self, name: &str) -> Option<&BoundaryValue> {
            match self {
                Self::Fields(pairs) => pairs
                    .iter()
                    .find(|(key, _)| key == name)
                    .map(|(_, value)| value),
                _ => None,
            }
        }

        /// The tag and payload of a tagged union value.
        pub fn tag(&self) -> Option<(&str, &BoundaryValue)> {
            let tag = self.field("tag")?;
            let value = self.field("value")?;
            let tag = match tag {
                Self::Text(text) => text.as_str(),
                _ => return None,
            };
            Some((tag, value))
        }

        /// Rejects a struct value whose field set is not exactly `names`.
        pub fn exact_fields(&self, names: &[&str]) -> Result<(), ClientError> {
            let Self::Fields(pairs) = self else {
                return Err(ClientError::InvalidInput);
            };
            if pairs.len() != names.len() {
                return Err(ClientError::InvalidInput);
            }
            for name in names {
                if !pairs.iter().any(|(key, _)| key == name) {
                    return Err(ClientError::InvalidInput);
                }
            }
            Ok(())
        }

        pub fn as_bool(&self) -> Result<bool, ClientError> {
            match self {
                Self::Bool(value) => Ok(*value),
                _ => Err(ClientError::InvalidInput),
            }
        }

        /// A checked integer leaf inside `min..=max`; booleans and floats
        /// never coerce to integers.
        pub fn as_int_in(&self, min: i64, max: i64) -> Result<i64, ClientError> {
            match self {
                Self::Int(value) if min <= *value && *value <= max => Ok(*value),
                _ => Err(ClientError::InvalidInput),
            }
        }

        /// A finite float leaf; NaN and infinities reject the whole batch.
        pub fn as_finite_float(&self) -> Result<f64, ClientError> {
            match self {
                Self::Float(value) if value.is_finite() => Ok(*value),
                _ => Err(ClientError::InvalidInput),
            }
        }

        pub fn as_text(&self) -> Result<&str, ClientError> {
            match self {
                Self::Text(value) => Ok(value.as_str()),
                _ => Err(ClientError::InvalidInput),
            }
        }

        pub fn as_list(&self) -> Result<&[BoundaryValue], ClientError> {
            match self {
                Self::List(items) => Ok(items.as_slice()),
                _ => Err(ClientError::InvalidInput),
            }
        }
    }

    /// A required struct field.
    fn required<'a>(
        value: &'a BoundaryValue,
        name: &str,
    ) -> Result<&'a BoundaryValue, ClientError> {
        value.field(name).ok_or(ClientError::InvalidInput)
    }

    /// An optional field: present-and-null and absent are both `None` only
    /// when the caller says null is legal; the facade carries every optional
    /// field explicitly, so a missing key is a shape error.
    fn optional<'a>(
        value: &'a BoundaryValue,
        name: &str,
    ) -> Result<&'a BoundaryValue, ClientError> {
        value.field(name).ok_or(ClientError::InvalidInput)
    }

    // ------------------------------------------------------------------
    // Canonical text and hex codecs
    // ------------------------------------------------------------------

    /// Decodes one canonical decimal `u64` text: nonempty, at most twenty
    /// ASCII digits, no sign, and no leading zero unless the value is zero.
    /// Everything else — including `u64::MAX + 1` — rejects the whole batch.
    pub fn u64_from_text(value: &BoundaryValue) -> Result<u64, ClientError> {
        let text = value.as_text()?;
        if text.is_empty() || text.len() > 20 || !text.bytes().all(|b| b.is_ascii_digit()) {
            return Err(ClientError::InvalidInput);
        }
        if text.len() > 1 && text.starts_with('0') {
            return Err(ClientError::InvalidInput);
        }
        text.parse::<u64>().map_err(|_| ClientError::InvalidInput)
    }

    /// Renders one `u64` as canonical decimal text; the rendering is
    /// canonical by construction.
    pub fn u64_text(value: u64) -> BoundaryValue {
        BoundaryValue::Text(value.to_string())
    }

    fn u8_from(value: &BoundaryValue) -> Result<u8, ClientError> {
        u8::try_from(value.as_int_in(0, i64::from(u8::MAX))?).map_err(|_| ClientError::InvalidInput)
    }

    fn u16_from(value: &BoundaryValue) -> Result<u16, ClientError> {
        u16::try_from(value.as_int_in(0, i64::from(u16::MAX))?)
            .map_err(|_| ClientError::InvalidInput)
    }

    fn u32_from(value: &BoundaryValue) -> Result<u32, ClientError> {
        u32::try_from(value.as_int_in(0, i64::from(u32::MAX))?)
            .map_err(|_| ClientError::InvalidInput)
    }

    fn i32_from(value: &BoundaryValue) -> Result<i32, ClientError> {
        i32::try_from(value.as_int_in(i64::from(i32::MIN), i64::from(i32::MAX))?)
            .map_err(|_| ClientError::InvalidInput)
    }

    fn i8_from(value: &BoundaryValue) -> Result<i8, ClientError> {
        i8::try_from(value.as_int_in(i64::from(i8::MIN), i64::from(i8::MAX))?)
            .map_err(|_| ClientError::InvalidInput)
    }

    /// Decodes one lowercase hex string of exactly `2 * N` digits into `N`
    /// bytes. Uppercase digits, dashes, odd lengths and wrong lengths all
    /// reject: the facade admits exactly one spelling.
    fn hex_bytes<const N: usize>(value: &BoundaryValue) -> Result<[u8; N], ClientError> {
        let text = value.as_text()?;
        if text.len() != 2 * N {
            return Err(ClientError::InvalidInput);
        }
        let mut bytes = [0u8; N];
        for (index, pair) in text.as_bytes().chunks(2).enumerate() {
            let high = hex_digit(pair[0])?;
            let low = hex_digit(pair[1])?;
            bytes[index] = (high << 4) | low;
        }
        Ok(bytes)
    }

    fn hex_digit(byte: u8) -> Result<u8, ClientError> {
        match byte {
            b'0'..=b'9' => Ok(byte - b'0'),
            b'a'..=b'f' => Ok(byte - b'a' + 10),
            _ => Err(ClientError::InvalidInput),
        }
    }

    /// Renders bytes as fixed-length lowercase hex.
    pub fn hex_text(bytes: &[u8]) -> BoundaryValue {
        const DIGITS: &[u8; 16] = b"0123456789abcdef";
        let mut text = String::with_capacity(bytes.len() * 2);
        for byte in bytes {
            text.push(char::from(DIGITS[usize::from(byte >> 4)]));
            text.push(char::from(DIGITS[usize::from(byte & 0x0F)]));
        }
        BoundaryValue::Text(text)
    }

    /// A 32-digit lowercase-hex UUID identity.
    pub fn uuid_from_hex(value: &BoundaryValue) -> Result<[u8; 16], ClientError> {
        hex_bytes::<16>(value)
    }

    // ------------------------------------------------------------------
    // The core token and the closed failure envelope
    // ------------------------------------------------------------------

    /// The private core token: a nonzero arena slot plus its generation.
    /// Tokens are values, never pointers, and a stale generation fails the
    /// lookup before any core dereference.
    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    pub struct CoreTokenValue {
        slot: u32,
        generation: u64,
    }

    impl CoreTokenValue {
        pub fn new(slot: u32, generation: u64) -> Result<Self, ClientError> {
            if slot == 0 {
                return Err(ClientError::InvalidInput);
            }
            Ok(Self { slot, generation })
        }

        pub fn slot(self) -> u32 {
            self.slot
        }

        pub fn generation(self) -> u64 {
            self.generation
        }

        pub fn to_boundary(self) -> BoundaryValue {
            BoundaryValue::fields([
                ("slot", BoundaryValue::Int(i64::from(self.slot))),
                ("generation", u64_text(self.generation)),
            ])
        }

        pub fn from_boundary(value: &BoundaryValue) -> Result<Self, ClientError> {
            value.exact_fields(&["slot", "generation"])?;
            let slot = u32_from(required(value, "slot")?)?;
            let generation = u64_from_text(required(value, "generation")?)?;
            Self::new(slot, generation)
        }
    }

    /// The closed failure class names, one per `ClientError` variant, in the
    /// contract's declared vocabulary.
    pub fn client_error_class(error: &ClientError) -> &'static str {
        match error {
            ClientError::InvalidInput => "InvalidInput",
            ClientError::IncompatibleVersion => "IncompatibleVersion",
            ClientError::InvalidState => "InvalidState",
            ClientError::StaleEpoch => "StaleEpoch",
            ClientError::Capacity => "Capacity",
            ClientError::Timeout => "Timeout",
            ClientError::Disconnected => "Disconnected",
            ClientError::Io => "Io",
            ClientError::Internal => "Internal",
        }
    }

    /// The `ErrorValue` of one failure: the closed class name beside null
    /// detail slots. Capacity detail comes from the owning core; this
    /// boundary never fabricates a resource, limit or observation value.
    pub fn error_value(error: &ClientError) -> BoundaryValue {
        BoundaryValue::fields([
            (
                "class",
                BoundaryValue::Text(client_error_class(error).to_string()),
            ),
            ("resource", BoundaryValue::Null),
            ("limit", BoundaryValue::Null),
            ("observed", BoundaryValue::Null),
        ])
    }

    /// The closed method envelope: success carries `value` and a null
    /// error; failure carries a null value and the `ErrorValue`.
    pub fn outcome(result: Result<BoundaryValue, ClientError>) -> BoundaryValue {
        match result {
            Ok(value) => BoundaryValue::fields([
                ("ok", BoundaryValue::Bool(true)),
                ("value", value),
                ("error", BoundaryValue::Null),
            ]),
            Err(error) => BoundaryValue::fields([
                ("ok", BoundaryValue::Bool(false)),
                ("value", BoundaryValue::Null),
                ("error", error_value(&error)),
            ]),
        }
    }

    // ------------------------------------------------------------------
    // Method-argument decoding
    // ------------------------------------------------------------------

    /// The twelve limit fields of one checked `ClientLimits` value, in the
    /// frozen constructor order.
    const LIMIT_FIELDS: [&str; 12] = [
        "queued_input_events",
        "inbound_observations",
        "inbound_bytes",
        "outbound_commands",
        "outbound_bytes",
        "prediction_journal",
        "message_work",
        "mesh_work",
        "preparation_results",
        "preparation_bytes",
        "family_records",
        "frame_bytes",
    ];

    /// The checked core configuration one `open_core` call carries: the
    /// accepted limits, the hello/login deadline policy in milliseconds and
    /// the native connector capability the bridge resolves in its own
    /// registry. The clock and connector registry never cross the boundary;
    /// they are native injected ports.
    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    pub struct CoreOpenSpec {
        pub limits: ClientLimits,
        pub hello_ms: u32,
        pub login_ms: u32,
        pub connector_capability: u64,
    }

    /// Decodes the `open_core` configuration. Every limit is canonical
    /// `u64` text and the whole set must pass the accepted frozen ceilings
    /// (a zero or over-limit field rejects before anything opens).
    pub fn decode_open_core(value: &BoundaryValue) -> Result<CoreOpenSpec, ClientError> {
        value.exact_fields(&["limits", "hello_ms", "login_ms", "connector_capability"])?;
        let limits_value = required(value, "limits")?;
        limits_value.exact_fields(&LIMIT_FIELDS)?;
        let mut fields = [0u64; 12];
        for (index, name) in LIMIT_FIELDS.iter().enumerate() {
            fields[index] = u64_from_text(required(limits_value, name)?)?;
        }
        let limits = ClientLimits::try_new_with(
            fields[0] as usize,
            fields[1] as usize,
            fields[2] as usize,
            fields[3] as usize,
            fields[4] as usize,
            fields[5] as usize,
            fields[6] as usize,
            fields[7] as usize,
            fields[8] as usize,
            fields[9] as usize,
            fields[10] as usize,
            fields[11] as usize,
        )?;
        let hello_ms = u32_from(required(value, "hello_ms")?)?;
        let login_ms = u32_from(required(value, "login_ms")?)?;
        if hello_ms == 0 || login_ms == 0 {
            return Err(ClientError::InvalidInput);
        }
        let connector_capability = u64_from_text(required(value, "connector_capability")?)?;
        if connector_capability == 0 {
            return Err(ClientError::InvalidInput);
        }
        Ok(CoreOpenSpec {
            limits,
            hello_ms,
            login_ms,
            connector_capability,
        })
    }

    /// Decodes the checked endpoint union. A TCP host is a validated numeric
    /// IP literal, never a DNS name: the boundary performs no lookup.
    pub fn decode_endpoint(value: &BoundaryValue) -> Result<Endpoint, ClientError> {
        let (tag, payload) = value.tag().ok_or(ClientError::InvalidInput)?;
        match tag {
            "Memory" => {
                payload.exact_fields(&["connector_id"])?;
                let raw = u64_from_text(required(payload, "connector_id")?)?;
                let connector_id =
                    std::num::NonZeroU64::new(raw).ok_or(ClientError::InvalidInput)?;
                Ok(Endpoint::Memory { connector_id })
            }
            "Tcp" => {
                payload.exact_fields(&["host", "port"])?;
                let host = required(payload, "host")?.as_text()?.to_string();
                let port = u16_from(required(payload, "port")?)?;
                let address: std::net::SocketAddr = format!("{host}:{port}")
                    .parse()
                    .map_err(|_| ClientError::InvalidInput)?;
                Ok(Endpoint::Tcp(address))
            }
            _ => Err(ClientError::InvalidInput),
        }
    }

    /// Decodes the checked login identity: the exact protocol `LoginStart`
    /// parts — UUID identity, canonical display name, view distance — with
    /// the v45 identity rules enforced by the protocol constructor itself.
    pub fn decode_identity(value: &BoundaryValue) -> Result<ClientIdentity, ClientError> {
        value.exact_fields(&["login"])?;
        let login = required(value, "login")?;
        login.exact_fields(&["player_id", "display_name", "view_distance"])?;
        let player_id = PlayerId::try_from_bytes(uuid_from_hex(required(login, "player_id")?)?)
            .map_err(|_| ClientError::InvalidInput)?;
        let display_name = required(login, "display_name")?.as_text()?.to_string();
        let view_distance = u8_from(required(login, "view_distance")?)?;
        let start = LoginStart::new(player_id, display_name, view_distance)
            .map_err(|_| ClientError::InvalidInput)?;
        ClientIdentity::try_new(start)
    }

    /// Decodes the bounded step work request.
    pub fn decode_work(value: &BoundaryValue) -> Result<ClientWorkBudget, ClientError> {
        value.exact_fields(&["messages", "meshes"])?;
        let messages = u16_from(required(value, "messages")?)?;
        let meshes = u16_from(required(value, "meshes")?)?;
        ClientWorkBudget::try_new(messages, meshes)
    }

    /// Decodes the epoch text of an epoch-addressed call.
    pub fn decode_epoch(value: &BoundaryValue) -> Result<SessionEpoch, ClientError> {
        SessionEpoch::try_new(u64_from_text(value)?)
    }

    // ------------------------------------------------------------------
    // The twenty-action input batch
    // ------------------------------------------------------------------

    /// Decodes one look-angles pair.
    fn decode_look(value: &BoundaryValue) -> Result<LookAngles, ClientError> {
        value.exact_fields(&["yaw", "pitch"])?;
        let yaw = required(value, "yaw")?.as_finite_float()?;
        let pitch = required(value, "pitch")?.as_finite_float()?;
        let (yaw, pitch) = (yaw as f32, pitch as f32);
        if !yaw.is_finite() || !pitch.is_finite() {
            return Err(ClientError::InvalidInput);
        }
        LookAngles::try_new(yaw, pitch).map_err(|_| ClientError::InvalidInput)
    }

    /// Decodes one chunk column pair.
    fn decode_chunk(value: &BoundaryValue) -> Result<mornlea_domain::ChunkPos, ClientError> {
        value.exact_fields(&["x", "z"])?;
        let x = i32_from(required(value, "x")?)?;
        let z = i32_from(required(value, "z")?)?;
        Ok(mornlea_domain::ChunkPos::new(x, z))
    }

    /// Decodes one container reference parts value.
    fn decode_container_ref(value: &BoundaryValue) -> Result<ContainerRef, ClientError> {
        value.exact_fields(&["chunk", "kind", "slot", "generation"])?;
        let chunk = decode_chunk(required(value, "chunk")?)?;
        let kind = match required(value, "kind")?.as_text()? {
            "Furnace" => ContainerKind::Furnace,
            "Chest" => ContainerKind::Chest,
            _ => return Err(ClientError::InvalidInput),
        };
        let slot = u8_from(required(value, "slot")?)?;
        let generation = u32_from(required(value, "generation")?)?;
        ContainerRef::try_new(chunk, kind, slot, generation).map_err(|_| ClientError::InvalidInput)
    }

    /// Decodes the container-reference tag the stack views reuse.
    fn decode_stack_view(value: &BoundaryValue) -> Result<StackView, ClientError> {
        let (tag, payload) = value.tag().ok_or(ClientError::InvalidInput)?;
        match tag {
            "Inventory" => Ok(StackView::Inventory),
            "Crafting" => Ok(StackView::Crafting),
            "Container" => Ok(StackView::Container(decode_container_ref(payload)?)),
            _ => Err(ClientError::InvalidInput),
        }
    }

    /// Decodes the nullary-action rule: the payload must be exactly null.
    fn nullary(value: &BoundaryValue) -> Result<(), ClientError> {
        match value {
            BoundaryValue::Null => Ok(()),
            _ => Err(ClientError::InvalidInput),
        }
    }

    /// Decodes one intent payload by its closed tag. The twenty tags are the
    /// exact `ClientIntent` variant names; the payload parts are the F1
    /// public parts of each record, and a nullary action carries a null
    /// payload. No wire bytes are serialized here.
    fn decode_intent_payload(
        tag: &str,
        payload: &BoundaryValue,
    ) -> Result<ClientIntent, ClientError> {
        let intent = match tag {
            "PlayerInput" => {
                payload.exact_fields(&["movement", "look", "actions"])?;
                let movement_value = required(payload, "movement")?;
                movement_value.exact_fields(&["move_x", "move_z", "jump"])?;
                let movement = mornlea_domain::Movement {
                    move_x: i8_from(required(movement_value, "move_x")?)?,
                    move_z: i8_from(required(movement_value, "move_z")?)?,
                    jump: required(movement_value, "jump")?.as_bool()?,
                };
                let look = decode_look(required(payload, "look")?)?;
                let actions_value = required(payload, "actions")?;
                actions_value.exact_fields(&["primary", "eating", "sprinting", "sneaking"])?;
                let actions = HeldActions {
                    primary: required(actions_value, "primary")?.as_bool()?,
                    eating: required(actions_value, "eating")?.as_bool()?,
                    sprinting: required(actions_value, "sprinting")?.as_bool()?,
                    sneaking: required(actions_value, "sneaking")?.as_bool()?,
                };
                ClientIntent::PlayerInput(PlayerControl::new(PlayerControlParts {
                    movement,
                    look,
                    actions,
                }))
            }
            "PlaceBlock" => {
                payload.exact_fields(&["look", "slot"])?;
                let look = decode_look(required(payload, "look")?)?;
                let slot = u8_from(required(payload, "slot")?)?;
                ClientIntent::PlaceBlock(PlacementIntent::try_new(look, slot).map_err(into_error)?)
            }
            "Resync" => {
                payload.exact_fields(&["dimension", "chunk", "have_revision"])?;
                let dimension = u8_from(required(payload, "dimension")?)?;
                let chunk = decode_chunk(required(payload, "chunk")?)?;
                let have_revision = u64_from_text(required(payload, "have_revision")?)?;
                ClientIntent::Resync(
                    ResyncIntent::try_new(dimension, chunk, have_revision).map_err(into_error)?,
                )
            }
            "SelectHotbar" => {
                payload.exact_fields(&["slot"])?;
                let slot = u8_from(required(payload, "slot")?)?;
                ClientIntent::SelectHotbar(HotbarSlot::new(slot).map_err(into_error)?)
            }
            "OpenContainer" | "TillSoil" | "BoneMeal" | "CollectWater" | "PlaceWater" => {
                payload.exact_fields(&["look"])?;
                let look = decode_look(required(payload, "look")?)?;
                match tag {
                    "OpenContainer" => ClientIntent::OpenContainer(look),
                    "TillSoil" => ClientIntent::TillSoil(look),
                    "BoneMeal" => ClientIntent::BoneMeal(look),
                    "CollectWater" => ClientIntent::CollectWater(look),
                    _ => ClientIntent::PlaceWater(look),
                }
            }
            "MoveInventory" => {
                payload.exact_fields(&["from", "to"])?;
                let from = u8_from(required(payload, "from")?)?;
                let to = u8_from(required(payload, "to")?)?;
                ClientIntent::MoveInventory(InventoryMove::try_new(from, to).map_err(into_error)?)
            }
            "MoveCrafting" => {
                payload.exact_fields(&["from", "to"])?;
                let from = u8_from(required(payload, "from")?)?;
                let to = u8_from(required(payload, "to")?)?;
                ClientIntent::MoveCrafting(CraftingMove::try_new(from, to).map_err(into_error)?)
            }
            "MoveContainer" => {
                payload.exact_fields(&["container", "from", "to"])?;
                let container = decode_container_ref(required(payload, "container")?)?;
                let from = u8_from(required(payload, "from")?)?;
                let to = u8_from(required(payload, "to")?)?;
                ClientIntent::MoveContainer(
                    ContainerMove::try_new(
                        container.chunk(),
                        container.kind(),
                        container.slot(),
                        container.generation(),
                        from,
                        to,
                    )
                    .map_err(into_error)?,
                )
            }
            "CloseContainer" => {
                nullary(payload)?;
                ClientIntent::CloseContainer
            }
            "DropSelectedItem" => {
                nullary(payload)?;
                ClientIntent::DropSelectedItem
            }
            "TakeCraftingOutput" => {
                nullary(payload)?;
                ClientIntent::TakeCraftingOutput
            }
            "EquipArmor" => {
                nullary(payload)?;
                ClientIntent::EquipArmor
            }
            "MovePartial" => {
                payload.exact_fields(&["view", "from", "to", "single"])?;
                let view = decode_stack_view(required(payload, "view")?)?;
                let from = u8_from(required(payload, "from")?)?;
                let to = u8_from(required(payload, "to")?)?;
                let single = required(payload, "single")?.as_bool()?;
                ClientIntent::MovePartial(
                    PartialMove::try_new(view, from, to, single).map_err(into_error)?,
                )
            }
            "QuickMove" => {
                payload.exact_fields(&["view", "slot"])?;
                let view = decode_stack_view(required(payload, "view")?)?;
                let slot = u8_from(required(payload, "slot")?)?;
                ClientIntent::QuickMove(StackSource::try_new(view, slot).map_err(into_error)?)
            }
            "DropStack" => {
                payload.exact_fields(&["view", "slot"])?;
                let view = decode_stack_view(required(payload, "view")?)?;
                let slot = u8_from(required(payload, "slot")?)?;
                ClientIntent::DropStack(StackSource::try_new(view, slot).map_err(into_error)?)
            }
            "Chat" => {
                payload.exact_fields(&["text"])?;
                let text = required(payload, "text")?.as_text()?.to_string();
                let command = CommandText::try_from_canonical(text).map_err(into_error)?;
                ClientIntent::Chat(ChatIntent::new(command))
            }
            _ => return Err(ClientError::InvalidInput),
        };
        Ok(intent)
    }

    /// Domain rejections collapse to the boundary's invalid-input class.
    fn into_error(_: mornlea_domain::DomainError) -> ClientError {
        ClientError::InvalidInput
    }

    /// Decodes one container view token.
    fn decode_container_token(value: &BoundaryValue) -> Result<ContainerToken, ClientError> {
        value.exact_fields(&["epoch", "reference", "confirmed_revision"])?;
        let epoch = decode_epoch(required(value, "epoch")?)?;
        let reference = decode_container_ref(required(value, "reference")?)?;
        let revision = u64_from_text(required(value, "confirmed_revision")?)?;
        ContainerToken::try_new(epoch, reference, ConfirmedRevision::new(revision))
    }

    /// Decodes one crafting view token. It deliberately carries no container
    /// reference and no generation.
    fn decode_crafting_token(value: &BoundaryValue) -> Result<CraftingViewToken, ClientError> {
        value.exact_fields(&["epoch", "confirmed_revision", "size"])?;
        let epoch = decode_epoch(required(value, "epoch")?)?;
        let revision = u64_from_text(required(value, "confirmed_revision")?)?;
        let size = match required(value, "size")?.as_text()? {
            "Personal" => CraftingSize::Personal,
            "Workbench" => CraftingSize::Workbench,
            _ => return Err(ClientError::InvalidInput),
        };
        CraftingViewToken::try_new(epoch, ConfirmedRevision::new(revision), size)
    }

    /// Decodes one batch action: the tagged intent plus the optional tokens.
    /// Every action carries both token keys; an absent key is a shape error
    /// and a present token is a checked value.
    fn decode_action(value: &BoundaryValue) -> Result<InputAction, ClientError> {
        value.exact_fields(&["intent", "container", "crafting"])?;
        let intent_value = required(value, "intent")?;
        let (tag, payload) = intent_value.tag().ok_or(ClientError::InvalidInput)?;
        let intent = decode_intent_payload(tag, payload)?;
        let container = match optional(value, "container")? {
            BoundaryValue::Null => None,
            token => Some(decode_container_token(token)?),
        };
        let crafting = match optional(value, "crafting")? {
            BoundaryValue::Null => None,
            token => Some(decode_crafting_token(token)?),
        };
        Ok(InputAction {
            intent,
            container,
            crafting,
        })
    }

    /// Decodes the whole input batch: the epoch text, the action vector and
    /// every action's checked payload and tokens. The 128-action ceiling is
    /// the C1 constructor's own; a 129th action rejects the complete batch
    /// with the typed capacity error before any core call runs.
    pub fn decode_input_batch(value: &BoundaryValue) -> Result<InputBatch, ClientError> {
        value.exact_fields(&["epoch", "actions"])?;
        let epoch = decode_epoch(required(value, "epoch")?)?;
        let actions_value = required(value, "actions")?;
        let items = actions_value.as_list()?;
        let mut actions = Vec::with_capacity(items.len());
        for item in items {
            actions.push(decode_action(item)?);
        }
        InputBatch::try_new(epoch, actions)
    }

    // ------------------------------------------------------------------
    // Result rendering
    // ------------------------------------------------------------------

    /// Renders the input receipt's closed union.
    pub fn render_receipt(receipt: &InputReceipt) -> BoundaryValue {
        match receipt {
            InputReceipt::Noop => BoundaryValue::tagged("Noop", BoundaryValue::Null),
            InputReceipt::Queued {
                epoch,
                first_sequence,
                sequenced_count,
                chat_count,
            } => BoundaryValue::tagged(
                "Queued",
                BoundaryValue::fields([
                    ("epoch", u64_text(epoch.get())),
                    (
                        "first_sequence",
                        first_sequence.map_or(BoundaryValue::Null, u64_text),
                    ),
                    (
                        "sequenced_count",
                        BoundaryValue::Int(i64::from(*sequenced_count)),
                    ),
                    ("chat_count", BoundaryValue::Int(i64::from(*chat_count))),
                ]),
            ),
        }
    }

    /// Renders the terminal close reason's closed union.
    fn render_close_reason(reason: &CloseReason) -> BoundaryValue {
        match reason {
            CloseReason::LocalClose => BoundaryValue::tagged("LocalClose", BoundaryValue::Null),
            CloseReason::Timeout => BoundaryValue::tagged("Timeout", BoundaryValue::Null),
            CloseReason::Capacity => BoundaryValue::tagged("Capacity", BoundaryValue::Null),
            CloseReason::Internal => BoundaryValue::tagged("Internal", BoundaryValue::Null),
            CloseReason::RemoteDisconnect(text) => BoundaryValue::tagged(
                "RemoteDisconnect",
                BoundaryValue::Text(text.as_str().to_string()),
            ),
            CloseReason::LoginRejected(text) => BoundaryValue::tagged(
                "LoginRejected",
                BoundaryValue::Text(text.as_str().to_string()),
            ),
        }
    }

    /// Renders the checked step report copy.
    pub fn render_step_report(report: &StepReport) -> BoundaryValue {
        BoundaryValue::fields([
            ("epoch", u64_text(report.epoch().get())),
            (
                "confirmed_revision",
                u64_text(report.confirmed_revision().get()),
            ),
            ("frame_index", u64_text(report.frame_index())),
            (
                "processed_messages",
                BoundaryValue::Int(i64::from(report.processed_messages())),
            ),
            (
                "processed_meshes",
                BoundaryValue::Int(i64::from(report.processed_meshes())),
            ),
            (
                "pending_input",
                BoundaryValue::Int(report.pending_input() as i64),
            ),
            (
                "pending_inbound",
                BoundaryValue::Int(report.pending_inbound() as i64),
            ),
            (
                "pending_preparation",
                BoundaryValue::Int(report.pending_preparation() as i64),
            ),
            (
                "terminal",
                report
                    .terminal()
                    .map_or(BoundaryValue::Null, render_close_reason),
            ),
        ])
    }

    /// Renders the shared record header every family record carries.
    fn render_header(header: &mornlea_client_core::contracts::RecordHeader) -> BoundaryValue {
        BoundaryValue::fields([
            ("epoch", u64_text(header.epoch().get())),
            ("revision", u64_text(header.revision().get())),
            (
                "source_tick",
                header.source_tick().map_or(BoundaryValue::Null, u64_text),
            ),
            (
                "operation",
                BoundaryValue::Text(
                    match header.operation() {
                        mornlea_client_core::contracts::FamilyOperation::Upsert => "Upsert",
                        mornlea_client_core::contracts::FamilyOperation::Remove => "Remove",
                    }
                    .to_string(),
                ),
            ),
        ])
    }

    fn opt_u64(value: Option<u64>) -> BoundaryValue {
        value.map_or(BoundaryValue::Null, u64_text)
    }

    fn opt_f64(value: Option<f64>) -> BoundaryValue {
        value.map_or(BoundaryValue::Null, BoundaryValue::Float)
    }

    fn vec3(values: [f64; 3]) -> BoundaryValue {
        BoundaryValue::List(values.into_iter().map(BoundaryValue::Float).collect())
    }

    fn opt_vec3(values: Option<[f64; 3]>) -> BoundaryValue {
        values.map_or(BoundaryValue::Null, vec3)
    }

    /// Renders the block-position triple.
    fn block_pos(position: mornlea_domain::BlockPos) -> BoundaryValue {
        BoundaryValue::fields([
            ("x", BoundaryValue::Int(i64::from(position.x()))),
            ("y", BoundaryValue::Int(i64::from(position.y()))),
            ("z", BoundaryValue::Int(i64::from(position.z()))),
        ])
    }

    /// Renders one item stack by its public parts.
    fn stack(value: &ItemStack) -> BoundaryValue {
        BoundaryValue::fields([
            ("item", BoundaryValue::Int(i64::from(value.item()))),
            ("count", BoundaryValue::Int(i64::from(value.count()))),
            (
                "durability",
                BoundaryValue::Int(i64::from(value.durability())),
            ),
        ])
    }

    fn stacks(values: &[ItemStack]) -> BoundaryValue {
        BoundaryValue::List(values.iter().map(stack).collect())
    }

    /// Renders the container reference parts.
    fn container_ref(reference: ContainerRef) -> BoundaryValue {
        BoundaryValue::fields([
            (
                "chunk",
                BoundaryValue::fields([
                    ("x", BoundaryValue::Int(i64::from(reference.chunk().x()))),
                    ("z", BoundaryValue::Int(i64::from(reference.chunk().z()))),
                ]),
            ),
            (
                "kind",
                BoundaryValue::Text(
                    match reference.kind() {
                        ContainerKind::Furnace => "Furnace",
                        ContainerKind::Chest => "Chest",
                    }
                    .to_string(),
                ),
            ),
            ("slot", BoundaryValue::Int(i64::from(reference.slot()))),
            (
                "generation",
                BoundaryValue::Int(i64::from(reference.generation())),
            ),
        ])
    }

    /// Renders the terrain key union: a near section or a far tile.
    fn terrain_key(key: &mornlea_client_core::preparation::TerrainKey) -> BoundaryValue {
        match key {
            mornlea_client_core::preparation::TerrainKey::Section(section) => {
                BoundaryValue::tagged(
                    "Section",
                    BoundaryValue::fields([
                        (
                            "chunk",
                            BoundaryValue::fields([
                                ("x", BoundaryValue::Int(i64::from(section.chunk().x()))),
                                ("z", BoundaryValue::Int(i64::from(section.chunk().z()))),
                            ]),
                        ),
                        ("section", BoundaryValue::Int(i64::from(section.section()))),
                    ]),
                )
            }
            mornlea_client_core::preparation::TerrainKey::LodTile(tile) => BoundaryValue::tagged(
                "LodTile",
                BoundaryValue::fields([
                    ("x", BoundaryValue::Int(i64::from(tile.x()))),
                    ("z", BoundaryValue::Int(i64::from(tile.z()))),
                ]),
            ),
        }
    }

    /// Renders the player-control payload (the F1 public parts).
    fn player_control(control: &PlayerControl) -> BoundaryValue {
        let movement = control.movement();
        let look = control.look();
        let actions = control.actions();
        BoundaryValue::fields([
            (
                "movement",
                BoundaryValue::fields([
                    ("move_x", BoundaryValue::Int(i64::from(movement.move_x))),
                    ("move_z", BoundaryValue::Int(i64::from(movement.move_z))),
                    ("jump", BoundaryValue::Bool(movement.jump)),
                ]),
            ),
            (
                "look",
                BoundaryValue::fields([
                    ("yaw", BoundaryValue::Float(f64::from(look.yaw()))),
                    ("pitch", BoundaryValue::Float(f64::from(look.pitch()))),
                ]),
            ),
            (
                "actions",
                BoundaryValue::fields([
                    ("primary", BoundaryValue::Bool(actions.primary)),
                    ("eating", BoundaryValue::Bool(actions.eating)),
                    ("sprinting", BoundaryValue::Bool(actions.sprinting)),
                    ("sneaking", BoundaryValue::Bool(actions.sneaking)),
                ]),
            ),
        ])
    }

    /// Renders the checked pose.
    fn pose(pose: &mornlea_client_core::presentation::Pose) -> BoundaryValue {
        BoundaryValue::fields([
            ("position", vec3(pose.position())),
            ("yaw", BoundaryValue::Float(pose.yaw())),
            ("pitch", BoundaryValue::Float(pose.pitch())),
        ])
    }

    /// Renders the inventory view union.
    fn inventory_view(view: &InventoryUiView) -> BoundaryValue {
        match view {
            InventoryUiView::Inventory(state) => BoundaryValue::tagged(
                "Inventory",
                BoundaryValue::fields([
                    (
                        "selected",
                        BoundaryValue::Int(i64::from(state.selected().get())),
                    ),
                    ("hotbar", stacks(state.hotbar())),
                    ("backpack", stacks(state.backpack())),
                ]),
            ),
            InventoryUiView::Crafting(state) => BoundaryValue::tagged(
                "Crafting",
                BoundaryValue::fields([
                    (
                        "size",
                        BoundaryValue::Text(
                            match state.size() {
                                CraftingSize::Personal => "Personal",
                                CraftingSize::Workbench => "Workbench",
                            }
                            .to_string(),
                        ),
                    ),
                    ("slots", stacks(state.slots())),
                    ("output", stack(&state.output())),
                ]),
            ),
            InventoryUiView::Furnace(state) => BoundaryValue::tagged(
                "Furnace",
                BoundaryValue::fields([
                    ("container", container_ref(state.container())),
                    ("input", stack(&state.input())),
                    ("fuel", stack(&state.fuel())),
                    ("output", stack(&state.output())),
                    (
                        "progress_ticks",
                        BoundaryValue::Int(i64::from(state.progress_ticks())),
                    ),
                    (
                        "burn_ticks",
                        BoundaryValue::Int(i64::from(state.burn_ticks())),
                    ),
                ]),
            ),
            InventoryUiView::Chest(state) => BoundaryValue::tagged(
                "Chest",
                BoundaryValue::fields([
                    ("container", container_ref(state.container())),
                    ("items", stacks(state.items())),
                ]),
            ),
            InventoryUiView::Closed(reference) => {
                BoundaryValue::tagged("Closed", container_ref(*reference))
            }
            InventoryUiView::Rejected(rejection) => BoundaryValue::tagged(
                "Rejected",
                BoundaryValue::fields([
                    ("sequence", u64_text(rejection.sequence())),
                    (
                        "reason",
                        BoundaryValue::Text(format!("{:?}", rejection.reason())),
                    ),
                ]),
            ),
        }
    }

    /// Renders the world view union.
    fn world_view(view: &WorldUiView) -> BoundaryValue {
        match view {
            WorldUiView::Environment(state) => BoundaryValue::tagged(
                "Environment",
                BoundaryValue::fields([
                    (
                        "day_phase_offset",
                        BoundaryValue::Int(i64::from(state.day_phase_offset())),
                    ),
                    ("world_time_ticks", u64_text(state.world_time_ticks())),
                    (
                        "weather",
                        BoundaryValue::Text(format!("{:?}", state.weather())),
                    ),
                    (
                        "season",
                        BoundaryValue::Text(format!("{:?}", state.season())),
                    ),
                    (
                        "season_progress",
                        BoundaryValue::Int(i64::from(state.season_progress())),
                    ),
                    (
                        "temperature",
                        BoundaryValue::Int(i64::from(state.temperature())),
                    ),
                ]),
            ),
            WorldUiView::Survival(state) => BoundaryValue::tagged(
                "Survival",
                BoundaryValue::fields([
                    ("health", BoundaryValue::Int(i64::from(state.health()))),
                    ("oxygen", BoundaryValue::Text(state.oxygen().to_string())),
                    ("hunger", BoundaryValue::Text(state.hunger().to_string())),
                    (
                        "saturation_zero",
                        BoundaryValue::Bool(state.saturation_zero()),
                    ),
                    (
                        "armor_points",
                        BoundaryValue::Text(state.armor_points().to_string()),
                    ),
                ]),
            ),
            WorldUiView::Chat(event) => BoundaryValue::tagged(
                "Chat",
                BoundaryValue::fields([
                    ("event_id", u64_text(event.event_id())),
                    ("player_id", hex_text(&event.player_id().bytes())),
                    (
                        "player_name",
                        BoundaryValue::Text(event.player_name().as_str().to_string()),
                    ),
                    ("body", chat_body(event.body())),
                ]),
            ),
            WorldUiView::Task(task) => BoundaryValue::tagged(
                "Task",
                BoundaryValue::fields([
                    (
                        "observation",
                        BoundaryValue::fields([
                            ("epoch", u64_text(task.observation().epoch().get())),
                            (
                                "confirmed_revision",
                                u64_text(task.observation().confirmed_revision().get()),
                            ),
                            (
                                "ordinal",
                                BoundaryValue::Int(i64::from(task.observation().ordinal())),
                            ),
                        ]),
                    ),
                    (
                        "companion",
                        BoundaryValue::fields([
                            ("id", hex_text(&task.companion().id().bytes())),
                            (
                                "name",
                                BoundaryValue::Text(task.companion().name().as_str().to_string()),
                            ),
                        ]),
                    ),
                    (
                        "command",
                        BoundaryValue::Text(task.command().as_str().to_string()),
                    ),
                    ("state", task_state(task.state())),
                ]),
            ),
            WorldUiView::Prompt(prompt) => BoundaryValue::tagged(
                "Prompt",
                prompt.as_ref().map_or(BoundaryValue::Null, |view| {
                    BoundaryValue::fields([
                        ("target", block_pos(view.target())),
                        (
                            "label",
                            BoundaryValue::Text(view.label().as_str().to_string()),
                        ),
                    ])
                }),
            ),
        }
    }

    /// Renders the chat body union by its closed variants.
    fn chat_body(body: &ChatBody) -> BoundaryValue {
        fn speaker(
            companion: &mornlea_domain::CompanionSpeaker,
            command: &CommandText,
        ) -> [(&'static str, BoundaryValue); 2] {
            [
                ("companion_id", hex_text(&companion.id().bytes())),
                ("command", BoundaryValue::Text(command.as_str().to_string())),
            ]
        }
        match body {
            ChatBody::Accepted { companion, command } => BoundaryValue::tagged(
                "Accepted",
                BoundaryValue::fields(speaker(companion, command)),
            ),
            ChatBody::InvalidFormat => BoundaryValue::tagged("InvalidFormat", BoundaryValue::Null),
            ChatBody::UnknownCompanion { name } => BoundaryValue::tagged(
                "UnknownCompanion",
                BoundaryValue::Text(name.as_str().to_string()),
            ),
            ChatBody::QueueFull { companion, command } => BoundaryValue::tagged(
                "QueueFull",
                BoundaryValue::fields(speaker(companion, command)),
            ),
            ChatBody::NotFollowing { companion, command } => BoundaryValue::tagged(
                "NotFollowing",
                BoundaryValue::fields(speaker(companion, command)),
            ),
            ChatBody::Task {
                companion,
                command,
                state,
            } => {
                let mut fields: Vec<(String, BoundaryValue)> = speaker(companion, command)
                    .into_iter()
                    .map(|(name, value)| (name.to_string(), value))
                    .collect();
                fields.push(("state".to_string(), task_state(state)));
                BoundaryValue::tagged("Task", BoundaryValue::Fields(fields))
            }
            ChatBody::Speech { companion, text } => BoundaryValue::tagged(
                "Speech",
                BoundaryValue::fields([
                    ("companion_id", hex_text(&companion.id().bytes())),
                    ("speech", BoundaryValue::Text(text.as_str().to_string())),
                ]),
            ),
        }
    }

    /// Renders the task state union.
    fn task_state(state: &TaskState) -> BoundaryValue {
        match state {
            TaskState::Failed(reason) => {
                BoundaryValue::tagged("Failed", BoundaryValue::Text(format!("{reason:?}")))
            }
            TaskState::Started => BoundaryValue::tagged("Started", BoundaryValue::Null),
            TaskState::Progress => BoundaryValue::tagged("Progress", BoundaryValue::Null),
            TaskState::Completed => BoundaryValue::tagged("Completed", BoundaryValue::Null),
            TaskState::TimedOut => BoundaryValue::tagged("TimedOut", BoundaryValue::Null),
            TaskState::Stopped => BoundaryValue::tagged("Stopped", BoundaryValue::Null),
        }
    }

    /// Renders the mining union: `Idle` carries nothing and `Active` carries
    /// the checked swing fields.
    fn mining(state: &MiningState) -> BoundaryValue {
        match state {
            MiningState::Idle => BoundaryValue::tagged("Idle", BoundaryValue::Null),
            MiningState::Active(active) => BoundaryValue::tagged(
                "Active",
                BoundaryValue::fields([
                    ("target", block_pos(active.target())),
                    ("progress", BoundaryValue::Int(i64::from(active.progress()))),
                    ("required", BoundaryValue::Int(i64::from(active.required()))),
                    ("harvestable", BoundaryValue::Bool(active.harvestable())),
                ]),
            ),
        }
    }

    /// Renders the actor identity union.
    fn actor_id(id: &mornlea_client_core::presentation::frame::ActorId) -> BoundaryValue {
        use mornlea_client_core::presentation::frame::ActorId;
        match id {
            ActorId::RemotePlayer(player) => {
                BoundaryValue::tagged("RemotePlayer", hex_text(&player.bytes()))
            }
            ActorId::Drop(drop) => BoundaryValue::tagged(
                "Drop",
                BoundaryValue::fields([
                    ("dimension", BoundaryValue::Int(i64::from(drop.dimension()))),
                    ("slot", BoundaryValue::Int(i64::from(drop.slot()))),
                    ("generation", u64_text(u64::from(drop.generation()))),
                    (
                        "chunk",
                        BoundaryValue::fields([
                            ("x", BoundaryValue::Int(i64::from(drop.chunk().x()))),
                            ("z", BoundaryValue::Int(i64::from(drop.chunk().z()))),
                        ]),
                    ),
                ]),
            ),
            ActorId::Hostile(id) => BoundaryValue::tagged("Hostile", u64_text(id.get())),
            ActorId::Passive(id) => BoundaryValue::tagged("Passive", u64_text(id.get())),
            ActorId::Projectile(id) => BoundaryValue::tagged("Projectile", u64_text(id.get())),
            ActorId::Companion(id) => BoundaryValue::tagged("Companion", hex_text(&id.bytes())),
        }
    }

    /// Renders the actor dimension union.
    fn actor_dimension(
        dimension: &mornlea_client_core::presentation::frame::ActorDimension,
    ) -> BoundaryValue {
        match dimension {
            mornlea_client_core::presentation::frame::ActorDimension::Known(known) => {
                BoundaryValue::tagged("Known", BoundaryValue::Int(i64::from(known.get())))
            }
            mornlea_client_core::presentation::frame::ActorDimension::DropRaw(raw) => {
                BoundaryValue::tagged("DropRaw", BoundaryValue::Int(i64::from(*raw)))
            }
        }
    }

    /// Renders the actor detail union.
    fn actor_detail(
        detail: &mornlea_client_core::presentation::frame::ActorDetail,
    ) -> BoundaryValue {
        use mornlea_client_core::presentation::frame::ActorDetail;
        match detail {
            ActorDetail::RemotePlayer {
                display_name,
                reset,
            } => BoundaryValue::tagged(
                "RemotePlayer",
                BoundaryValue::fields([
                    (
                        "display_name",
                        display_name.as_ref().map_or(BoundaryValue::Null, |text| {
                            BoundaryValue::Text(text.as_str().to_string())
                        }),
                    ),
                    (
                        "reset",
                        reset.map_or(BoundaryValue::Null, BoundaryValue::Bool),
                    ),
                ]),
            ),
            ActorDetail::Drop {
                block_index,
                stack: drop_stack,
            } => BoundaryValue::tagged(
                "Drop",
                BoundaryValue::fields([
                    ("block_index", u64_text(u64::from(*block_index))),
                    ("stack", stack(drop_stack)),
                ]),
            ),
            ActorDetail::Hostile { archetype, health } => BoundaryValue::tagged(
                "Hostile",
                BoundaryValue::fields([
                    ("archetype", BoundaryValue::Int(i64::from(*archetype))),
                    ("health", BoundaryValue::Int(i64::from(*health))),
                ]),
            ),
            ActorDetail::Passive {
                health,
                grazing,
                despawn_reason,
            } => BoundaryValue::tagged(
                "Passive",
                BoundaryValue::fields([
                    (
                        "health",
                        health.map_or(BoundaryValue::Null, |v| BoundaryValue::Int(i64::from(v))),
                    ),
                    (
                        "grazing",
                        grazing.map_or(BoundaryValue::Null, |v| BoundaryValue::Int(i64::from(v))),
                    ),
                    (
                        "despawn_reason",
                        despawn_reason.map_or(BoundaryValue::Null, |reason| {
                            BoundaryValue::Text(format!("{reason:?}"))
                        }),
                    ),
                ]),
            ),
            ActorDetail::Projectile { archetype } => BoundaryValue::tagged(
                "Projectile",
                BoundaryValue::fields([(
                    "archetype",
                    archetype.map_or(BoundaryValue::Null, |v| BoundaryValue::Int(i64::from(v))),
                )]),
            ),
            ActorDetail::Companion { name, reset } => BoundaryValue::tagged(
                "Companion",
                BoundaryValue::fields([
                    (
                        "name",
                        name.as_ref().map_or(BoundaryValue::Null, |text| {
                            BoundaryValue::Text(text.as_str().to_string())
                        }),
                    ),
                    (
                        "reset",
                        reset.map_or(BoundaryValue::Null, BoundaryValue::Bool),
                    ),
                ]),
            ),
        }
    }

    /// Renders the cue provenance union.
    fn cue_provenance(provenance: &CueProvenance) -> BoundaryValue {
        match provenance {
            CueProvenance::Confirmed {
                observation,
                authoritative_event_id,
            } => BoundaryValue::tagged(
                "Confirmed",
                BoundaryValue::fields([
                    ("epoch", u64_text(observation.epoch().get())),
                    (
                        "confirmed_revision",
                        u64_text(observation.confirmed_revision().get()),
                    ),
                    (
                        "ordinal",
                        BoundaryValue::Int(i64::from(observation.ordinal())),
                    ),
                    ("authoritative_event_id", opt_u64(*authoritative_event_id)),
                ]),
            ),
            CueProvenance::Predicted { input_sequence } => BoundaryValue::tagged(
                "Predicted",
                BoundaryValue::fields([("input_sequence", u64_text(*input_sequence))]),
            ),
            CueProvenance::Local {
                local_event_sequence,
            } => BoundaryValue::tagged(
                "Local",
                BoundaryValue::fields([("local_event_sequence", u64_text(*local_event_sequence))]),
            ),
        }
    }

    /// Renders the intent kind's closed name.
    fn intent_kind(kind: &ClientIntentKind) -> BoundaryValue {
        BoundaryValue::Text(
            match kind {
                ClientIntentKind::PlayerInput => "PlayerInput",
                ClientIntentKind::PlaceBlock => "PlaceBlock",
                ClientIntentKind::Resync => "Resync",
                ClientIntentKind::SelectHotbar => "SelectHotbar",
                ClientIntentKind::OpenContainer => "OpenContainer",
                ClientIntentKind::TillSoil => "TillSoil",
                ClientIntentKind::BoneMeal => "BoneMeal",
                ClientIntentKind::CollectWater => "CollectWater",
                ClientIntentKind::PlaceWater => "PlaceWater",
                ClientIntentKind::MoveInventory => "MoveInventory",
                ClientIntentKind::MoveCrafting => "MoveCrafting",
                ClientIntentKind::MoveContainer => "MoveContainer",
                ClientIntentKind::CloseContainer => "CloseContainer",
                ClientIntentKind::DropSelectedItem => "DropSelectedItem",
                ClientIntentKind::TakeCraftingOutput => "TakeCraftingOutput",
                ClientIntentKind::EquipArmor => "EquipArmor",
                ClientIntentKind::MovePartial => "MovePartial",
                ClientIntentKind::QuickMove => "QuickMove",
                ClientIntentKind::DropStack => "DropStack",
                ClientIntentKind::Chat => "Chat",
            }
            .to_string(),
        )
    }

    /// Renders one closed family record vector.
    fn render_records(records: &FamilyRecords) -> BoundaryValue {
        match records {
            FamilyRecords::Session(records) => BoundaryValue::List(
                records
                    .iter()
                    .map(|record| {
                        BoundaryValue::fields([
                            ("header", render_header(record.header())),
                            (
                                "phase",
                                BoundaryValue::Text(format!("{:?}", record.phase())),
                            ),
                            (
                                "player_id",
                                record.player_id().map_or(BoundaryValue::Null, |id| {
                                    hex_text(&id.bytes())
                                }),
                            ),
                            (
                                "terminal",
                                record
                                    .terminal()
                                    .map_or(BoundaryValue::Null, render_close_reason),
                            ),
                        ])
                    })
                    .collect(),
            ),
            FamilyRecords::Input(records) => BoundaryValue::List(
                records
                    .iter()
                    .map(|record| {
                        BoundaryValue::fields([
                            ("header", render_header(record.header())),
                            ("local_sequence", opt_u64(record.local_sequence())),
                            ("intent_kind", intent_kind(&record.intent_kind())),
                            (
                                "receipt",
                                match record.receipt() {
                                    InputReceiptState::Queued => {
                                        BoundaryValue::tagged("Queued", BoundaryValue::Null)
                                    }
                                    InputReceiptState::Rejected { class } => BoundaryValue::tagged(
                                        "Rejected",
                                        BoundaryValue::Text(
                                            client_error_class(class).to_string(),
                                        ),
                                    ),
                                    InputReceiptState::Confirmed { server_tick } => {
                                        BoundaryValue::tagged(
                                            "Confirmed",
                                            BoundaryValue::fields([(
                                                "server_tick",
                                                opt_u64(*server_tick),
                                            )]),
                                        )
                                    }
                                },
                            ),
                        ])
                    })
                    .collect(),
            ),
            FamilyRecords::Terrain(records) => BoundaryValue::List(
                records
                    .iter()
                    .map(|record| {
                        BoundaryValue::fields([
                            ("header", render_header(record.header())),
                            (
                                "dimension",
                                BoundaryValue::Int(i64::from(record.dimension().get())),
                            ),
                            ("key", terrain_key(record.key())),
                            (
                                "content_revision",
                                u64_text(record.content_revision()),
                            ),
                            ("generation", u64_text(record.generation())),
                            (
                                "material",
                                BoundaryValue::Text(format!("{:?}", record.material())),
                            ),
                            (
                                "visibility",
                                BoundaryValue::Text(format!("{:?}", record.visibility())),
                            ),
                            (
                                "light",
                                BoundaryValue::fields([
                                    (
                                        "sky",
                                        BoundaryValue::Int(i64::from(record.light().sky())),
                                    ),
                                    (
                                        "block",
                                        BoundaryValue::Int(i64::from(record.light().block())),
                                    ),
                                ]),
                            ),
                            (
                                "resource",
                                record.resource().map_or(BoundaryValue::Null, |key| {
                                    BoundaryValue::fields([
                                        ("epoch", u64_text(key.epoch().get())),
                                        (
                                            "dimension",
                                            BoundaryValue::Int(i64::from(key.dimension().get())),
                                        ),
                                        ("key", terrain_key(key.key())),
                                        ("generation", u64_text(key.generation())),
                                        (
                                            "content_revision",
                                            u64_text(key.content_revision()),
                                        ),
                                        ("job_id", u64_text(key.job_id().get())),
                                    ])
                                }),
                            ),
                        ])
                    })
                    .collect(),
            ),
            FamilyRecords::Actors(records) => BoundaryValue::List(
                records
                    .iter()
                    .map(|record| {
                        BoundaryValue::fields([
                            ("header", render_header(record.header())),
                            (
                                "kind",
                                BoundaryValue::Text(format!("{:?}", record.kind())),
                            ),
                            ("id", actor_id(record.id())),
                            ("dimension", actor_dimension(record.dimension())),
                            ("position", opt_vec3(record.position())),
                            ("yaw", opt_f64(record.yaw())),
                            ("pitch", opt_f64(record.pitch())),
                            ("velocity", opt_vec3(record.velocity())),
                            (
                                "detail",
                                record
                                    .detail()
                                    .map_or(BoundaryValue::Null, actor_detail),
                            ),
                        ])
                    })
                    .collect(),
            ),
            FamilyRecords::PlayerView(records) => BoundaryValue::List(
                records
                    .iter()
                    .map(|record| {
                        BoundaryValue::fields([
                            ("header", render_header(record.header())),
                            ("confirmed_pose", pose(record.confirmed_pose())),
                            (
                                "predicted_pose",
                                record.predicted_pose().map_or(BoundaryValue::Null, pose),
                            ),
                            (
                                "look_ray",
                                record.look_ray().map_or(BoundaryValue::Null, |ray| {
                                    BoundaryValue::fields([
                                        ("origin", vec3(ray.origin())),
                                        (
                                            "look",
                                            BoundaryValue::fields([
                                                (
                                                    "yaw",
                                                    BoundaryValue::Float(f64::from(
                                                        ray.look().yaw(),
                                                    )),
                                                ),
                                                (
                                                    "pitch",
                                                    BoundaryValue::Float(f64::from(
                                                        ray.look().pitch(),
                                                    )),
                                                ),
                                            ]),
                                        ),
                                        (
                                            "reach",
                                            BoundaryValue::Float(f64::from(ray.reach())),
                                        ),
                                    ])
                                }),
                            ),
                            (
                                "movement",
                                BoundaryValue::fields([
                                    (
                                        "control",
                                        record
                                            .movement()
                                            .control()
                                            .map_or(BoundaryValue::Null, player_control),
                                    ),
                                    (
                                        "on_ground",
                                        BoundaryValue::Bool(record.movement().on_ground()),
                                    ),
                                ]),
                            ),
                            (
                                "correction",
                                record.correction().map_or(BoundaryValue::Null, |value| {
                                    BoundaryValue::fields([
                                        (
                                            "last_input_sequence",
                                            u64_text(value.last_input_sequence()),
                                        ),
                                        (
                                            "reason",
                                            BoundaryValue::Text(format!(
                                                "{:?}",
                                                value.reason()
                                            )),
                                        ),
                                    ])
                                }),
                            ),
                            ("mining", mining(record.mining())),
                        ])
                    })
                    .collect(),
            ),
            FamilyRecords::InventoryUi(records) => BoundaryValue::List(
                records
                    .iter()
                    .map(|record| {
                        BoundaryValue::fields([
                            ("header", render_header(record.header())),
                            ("view", inventory_view(record.view())),
                            (
                                "token",
                                record.token().map_or(BoundaryValue::Null, |token| {
                                    BoundaryValue::fields([
                                        ("epoch", u64_text(token.epoch().get())),
                                        (
                                            "reference",
                                            container_ref(token.reference()),
                                        ),
                                        (
                                            "confirmed_revision",
                                            u64_text(token.confirmed_revision().get()),
                                        ),
                                    ])
                                }),
                            ),
                            (
                                "outcome",
                                record.outcome().map_or(BoundaryValue::Null, |outcome| {
                                    match outcome {
                                        mornlea_client_core::presentation::UiOutcome::Rejected {
                                            sequence,
                                            reason,
                                        } => BoundaryValue::tagged(
                                            "Rejected",
                                            BoundaryValue::fields([
                                                ("sequence", u64_text(*sequence)),
                                                (
                                                    "reason",
                                                    BoundaryValue::Text(format!("{reason:?}")),
                                                ),
                                            ]),
                                        ),
                                        mornlea_client_core::presentation::UiOutcome::PlacementAccepted {
                                            sequence,
                                        } => BoundaryValue::tagged(
                                            "PlacementAccepted",
                                            BoundaryValue::fields([(
                                                "sequence",
                                                u64_text(*sequence),
                                            )]),
                                        ),
                                    }
                                }),
                            ),
                        ])
                    })
                    .collect(),
            ),
            FamilyRecords::WorldUi(records) => BoundaryValue::List(
                records
                    .iter()
                    .map(|record| {
                        BoundaryValue::fields([
                            ("header", render_header(record.header())),
                            ("view", world_view(record.view())),
                        ])
                    })
                    .collect(),
            ),
            FamilyRecords::AudioCues(records) => BoundaryValue::List(
                records
                    .iter()
                    .map(|record| {
                        BoundaryValue::fields([
                            ("header", render_header(record.header())),
                            (
                                "cue_id",
                                BoundaryValue::Int(i64::from(record.cue_id().get())),
                            ),
                            ("provenance", cue_provenance(record.provenance())),
                            (
                                "category",
                                BoundaryValue::Text(format!("{:?}", record.category())),
                            ),
                            ("position", opt_vec3(record.position())),
                            (
                                "gain",
                                BoundaryValue::Float(f64::from(record.gain().get())),
                            ),
                            (
                                "pitch",
                                BoundaryValue::Float(f64::from(record.pitch().get())),
                            ),
                        ])
                    })
                    .collect(),
            ),
            FamilyRecords::Lifecycle(records) => BoundaryValue::List(
                records
                    .iter()
                    .map(|record| {
                        BoundaryValue::fields([
                            ("header", render_header(record.header())),
                            (
                                "transition",
                                BoundaryValue::Text(format!("{:?}", record.transition())),
                            ),
                            ("generation", u64_text(record.generation())),
                            (
                                "resource_order",
                                BoundaryValue::List(
                                    record
                                        .resource_order()
                                        .iter()
                                        .map(|key| match key {
                                            mornlea_client_core::presentation::ResourceKey::InputJournal => BoundaryValue::tagged("InputJournal", BoundaryValue::Null),
                                            mornlea_client_core::presentation::ResourceKey::PreparationQueue => BoundaryValue::tagged("PreparationQueue", BoundaryValue::Null),
                                            mornlea_client_core::presentation::ResourceKey::PresentationFrames => BoundaryValue::tagged("PresentationFrames", BoundaryValue::Null),
                                            mornlea_client_core::presentation::ResourceKey::BridgeHandles => BoundaryValue::tagged("BridgeHandles", BoundaryValue::Null),
                                            mornlea_client_core::presentation::ResourceKey::Feature(id) => BoundaryValue::tagged("Feature", u64_text(*id)),
                                        })
                                        .collect(),
                                ),
                            ),
                        ])
                    })
                    .collect(),
            ),
            FamilyRecords::Diagnostics(records) => BoundaryValue::List(
                records
                    .iter()
                    .map(|record| {
                        BoundaryValue::fields([
                            ("header", render_header(record.header())),
                            (
                                "producer",
                                BoundaryValue::fields([
                                    (
                                        "source_sha",
                                        hex_text(&record.producer().source_sha()),
                                    ),
                                    (
                                        "contract_sha",
                                        hex_text(&record.producer().contract_sha()),
                                    ),
                                ]),
                            ),
                            ("frame_index", u64_text(record.frame_index())),
                            // The counter owners publish no public field
                            // accessors yet, so the exact checked values are
                            // rendered as their owned debug text; a later
                            // accessor export replaces these two leaves
                            // without touching the record's other fields.
                            (
                                "queue_high_water",
                                BoundaryValue::Text(format!(
                                    "{:?}",
                                    record.queue_high_water()
                                )),
                            ),
                            (
                                "rejected",
                                BoundaryValue::Text(format!("{:?}", record.rejected())),
                            ),
                        ])
                    })
                    .collect(),
            ),
        }
    }

    /// Renders one whole presentation frame as the owned facade value.
    ///
    /// The rendering validates first: the complete frame must pass the
    /// accepted C2 validator (mixed epoch or revision headers, duplicate
    /// families, over-cap families and over-cap frames reject) and must
    /// carry the session family every C1 publication carries. A frame that
    /// fails never reaches the caller and never replaces a prior visible
    /// copy, which is the facade's whole-frame atomicity rule.
    pub fn render_frame(
        frame: &PresentationFrame,
        limits: &ClientLimits,
    ) -> Result<BoundaryValue, ClientError> {
        frame.validate(limits)?;
        let has_session = frame
            .families()
            .iter()
            .any(|family| family.key().logical_name == "session");
        if !has_session {
            return Err(ClientError::InvalidInput);
        }
        Ok(BoundaryValue::fields([
            (
                "layout_major",
                BoundaryValue::Int(i64::from(frame.layout_major())),
            ),
            (
                "layout_minor",
                BoundaryValue::Int(i64::from(frame.layout_minor())),
            ),
            ("session_epoch", u64_text(frame.session_epoch().get())),
            (
                "confirmed_revision",
                u64_text(frame.confirmed_revision().get()),
            ),
            ("frame_index", u64_text(frame.frame_index())),
            (
                "families",
                BoundaryValue::List(
                    frame
                        .families()
                        .iter()
                        .map(|family: &FamilyFrame| {
                            BoundaryValue::fields([
                                (
                                    "key",
                                    BoundaryValue::fields([
                                        (
                                            "logical_name",
                                            BoundaryValue::Text(
                                                family.key().logical_name.to_string(),
                                            ),
                                        ),
                                        (
                                            "major",
                                            BoundaryValue::Int(i64::from(family.key().major)),
                                        ),
                                        (
                                            "minor",
                                            BoundaryValue::Int(i64::from(family.key().minor)),
                                        ),
                                    ]),
                                ),
                                ("records", render_records(family.records())),
                            ])
                        })
                        .collect(),
                ),
            ),
        ]))
    }
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
