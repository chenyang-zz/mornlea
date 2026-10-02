//! Handwritten Rust FFI declarations for the Go c-shared client core.
//!
//! The producer package `packages/client/cmd/mornlea-godot-core` exports the
//! symbols declared in its `exports.go` from a shared library built by
//! `scripts/godot/build-core.sh`; this module is the Rust-side mirror of that
//! export surface. Constants, record layouts, sizes, and alignments stay in
//! [`crate::abi`] (pinned against `include/mornlea_client_core.h`); this
//! module owns the extern declarations themselves, the typed handle and
//! status wrappers the bridge passes across the boundary, and the diagnostic
//! status-text table.
//!
//! Production wiring: the GDExtension must not link the producer artifact at
//! build time, because the engine workspace builds from a clean checkout
//! before any producer library exists (`make rust`), while the producer is
//! materialized later by `scripts/godot/build-core.sh` beside this extension
//! in `addons/mornlea_bridge/bin/`. Production therefore resolves the twelve
//! exports dynamically: a one-shot loader locates this extension's own image
//! directory, opens the colocated producer library, verifies its ABI major,
//! and hands the bridge a typed function table. The declared extern block
//! stays the pinned declaration mirror that the parity tests validate in both
//! directions against `exports.go` and the header; the loader's resolved
//! signatures are pinned against those same declarations.
//!
//! Test wiring: bridge lifecycle tests never load the producer library. The
//! [`CoreCalls`] seam is the single call surface the bridge owns, so tests
//! script a table with producer-vocabulary statuses while production wires
//! the dynamic table through the same trait.
//!
//! Unsafe confinement: the `unsafe extern "C"` block below is the only unsafe
//! code this module needs, and confinement for the whole crate is enforced by
//! a source-scan test rather than a crate-level `#![forbid(unsafe_code)]`,
//! because the Godot `ExtensionLibrary` implementation in the crate root
//! legitimately requires `unsafe impl`. The scan pins that exact exception
//! and fails on any other `unsafe` token outside this file.

// The extern declarations and the diagnostic status text stay
// declaration-only pins (production resolves the exports at runtime), and the
// cargo-test configuration never loads the producer library, so parts of this
// module remain unreferenced in some compilation profiles. Allow
// `dead_code` module-wide so the pinned mirror does not fail
// `cargo clippy --all-targets -- -D warnings` in those profiles.
#![allow(dead_code)]

use core::ffi::{c_char, c_int, c_void};
use core::mem::{align_of, size_of};
use std::sync::OnceLock;

use crate::abi;

/// One-line diagnostic text for a defined status word, or `None` for any
/// undefined word so producer drift fails closed instead of aliasing a
/// defined meaning. The texts are the header's documenting comments for the
/// `MORNLEA_CLIENT_STATUS_*` defines, pinned verbatim by the tests below.
pub fn status_text(word: u32) -> Option<&'static str> {
    match word {
        abi::STATUS_OK => Some("The call succeeded and every committed output byte is valid."),
        abi::STATUS_INVALID_ARGUMENT => {
            Some("Null, misaligned, overlapping, or oversized pointer and length arguments.")
        }
        abi::STATUS_ABI_MISMATCH => {
            Some("Caller ABI major, magic, or record identity does not match the producer.")
        }
        abi::STATUS_INPUT_REJECTED => Some(
            "Readable buffer whose content violates the family domain; the whole batch is \
            rejected and no producer state is consumed.",
        ),
        abi::STATUS_INSUFFICIENT_CAPACITY => Some(
            "Two-phase capacity signal: output buffer too small; the required byte count is \
            reported and nothing is written.",
        ),
        abi::STATUS_INVALID_HANDLE => Some("Unknown or wrong-type handle."),
        abi::STATUS_INVALID_STATE => Some("Correct handle in the wrong lifecycle phase or epoch."),
        abi::STATUS_DISCONNECTED => Some("The session already reached its terminal disconnect."),
        abi::STATUS_INTERNAL => {
            Some("Producer-internal failure without a narrower stable classification.")
        }
        abi::STATUS_PANIC => {
            Some("A recovered panic converted at the ABI boundary; no output is written.")
        }
        _ => None,
    }
}

/// The status word every client-core export returns. The wire type is the
/// header's frozen `uint32_t` status code; the newtype keeps status words
/// from mixing with other `u32` values at the later bridge call sites.
#[repr(transparent)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Status(u32);

impl Status {
    /// Decode one raw status word. `None` for any word outside the defined
    /// contiguous range, mirroring the fail-closed classification the status
    /// matrix demands for undefined producer words.
    pub const fn from_word(word: u32) -> Option<Status> {
        if word < abi::STATUS_COUNT {
            Some(Status(word))
        } else {
            None
        }
    }

    /// The raw wire word of this status.
    pub const fn as_word(self) -> u32 {
        self.0
    }

    /// Whether this word is `STATUS_OK`, the only status that ever commits
    /// output bytes.
    pub const fn is_ok(self) -> bool {
        self.0 == abi::STATUS_OK
    }
}

/// An opaque session handle issued by `mornlea_client_core_create` and
/// consumed by every other export. The producer packs a table slot and a
/// generation into the 64-bit value; the zero value is never issued because
/// generations start at one, and a destroyed handle value is never reissued.
#[repr(transparent)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ClientHandle(u64);

impl ClientHandle {
    /// Wrap one raw handle word exactly as the producer issued it.
    pub const fn from_word(word: u64) -> ClientHandle {
        ClientHandle(word)
    }

    /// The raw wire word of this handle.
    pub const fn as_word(self) -> u64 {
        self.0
    }
}

// The newtype widths are contract facts, not implementation details: the
// producer's C surface passes the status word as uint32_t and the handle as
// uint64_t, so any width or alignment change here is an ABI break.
const _: () = assert!(size_of::<Status>() == 4 && align_of::<Status>() == 4);
const _: () = assert!(size_of::<ClientHandle>() == 8 && align_of::<ClientHandle>() == 8);

/// The symbol names of every producer export, in `exports.go` declaration
/// order. The parity tests below assert this list equals both the `//export`
/// directives of `exports.go` and the declarations of the extern block in
/// this file, in both directions, so no side can gain or reorder an export
/// silently.
pub const EXPORT_SYMBOLS: [&str; 13] = [
    "mornlea_client_core_create",
    "mornlea_client_core_destroy",
    "mornlea_client_core_connect_begin",
    "mornlea_client_core_connect_poll",
    "mornlea_client_core_disconnect",
    "mornlea_client_core_submit_input",
    "mornlea_client_core_step",
    "mornlea_client_core_world_pull",
    "mornlea_client_core_frame_pull",
    "mornlea_client_core_environment_pull",
    "mornlea_client_core_status_pull",
    "mornlea_client_core_status_identity",
    "mornlea_client_core_abi_version",
];

// All unsafe code of the client-core consumer lives in this block: calling
// any declaration is unsafe because the producer trusts the caller for the
// pointer disciplines documented per export below (the bridge module owns
// upholding them). No test references these items as values, so the test
// binary never needs the produced shared library at link time.
unsafe extern "C" {
    /// Mirrors the `mornlea_client_core_create` wrapper over `coreCreate`:
    /// `requested_families` addresses `family_count` little-endian 64-bit
    /// words, each packing a feature-family identifier in the low 32 bits and
    /// the requested family contract version in the high 32 bits; a positive
    /// count needs a non-null, 8-byte-aligned array, and `out_handle` must be
    /// non-null. Only full success writes the out handle.
    fn mornlea_client_core_create(
        abi_major: u32,
        abi_minor: u32,
        requested_families: *const u64,
        family_count: u32,
        out_handle: *mut ClientHandle,
    ) -> Status;

    /// Mirrors `mornlea_client_core_destroy` over `coreDestroy`: releasing a
    /// handle this producer issued is idempotent, so destroying an already
    /// destroyed handle reports success again and any never-issued or stale
    /// value reports an invalid-handle status. The invalid-state answer a
    /// destroyed handle earns on every other export never applies to the
    /// tombstone's own idempotent client.
    fn mornlea_client_core_destroy(handle: ClientHandle) -> Status;

    /// Mirrors `mornlea_client_core_connect_begin` over `coreConnectBegin`:
    /// `address` is `address_len` UTF-8 "host:port" bytes, non-empty, at most
    /// the header's connection-address bound, non-null, and 8-byte aligned;
    /// the buffer is copied and never retained after the call returns.
    fn mornlea_client_core_connect_begin(
        handle: ClientHandle,
        address: *const u8,
        address_len: u32,
    ) -> Status;

    /// Mirrors `mornlea_client_core_connect_poll` over `coreConnectPoll`:
    /// `out_phase` must be non-null and receives the connection phase word on
    /// success; a terminally disconnected session writes no output byte.
    fn mornlea_client_core_connect_poll(handle: ClientHandle, out_phase: *mut u32) -> Status;

    /// Mirrors `mornlea_client_core_disconnect` over `coreDisconnect`:
    /// idempotent teardown of one live session's connection.
    fn mornlea_client_core_disconnect(handle: ClientHandle) -> Status;

    /// Mirrors `mornlea_client_core_submit_input` over `coreSubmitInput`:
    /// `buffer` is one complete input-family batch of exactly `length` bytes
    /// (at least the input header size, at most the batch bound), non-null
    /// and 8-byte aligned for a positive length; the batch is copied and
    /// never retained after the call returns.
    fn mornlea_client_core_submit_input(
        handle: ClientHandle,
        buffer: *const u8,
        length: u32,
    ) -> Status;

    /// Mirrors `mornlea_client_core_step` over `coreStep`: `request` is
    /// exactly one step-request record of the fixed wire size pinned by
    /// [`abi::STEP_REQUEST_BYTES`], non-null and 8-byte aligned; the record
    /// is copied and never retained after the call returns.
    fn mornlea_client_core_step(handle: ClientHandle, request: *const u8, length: u32) -> Status;

    /// Mirrors `mornlea_client_core_world_pull` over `coreWorldPull`: the
    /// two-phase capacity protocol; `required_out` must be non-null because
    /// every call may report a size, and a positive `capacity` needs a
    /// non-null, 8-byte-aligned `out` buffer whose declared span does not
    /// contain the size word. The pull drains one retained world batch.
    fn mornlea_client_core_world_pull(
        handle: ClientHandle,
        out: *mut u8,
        capacity: u32,
        required_out: *mut u32,
    ) -> Status;

    /// Mirrors `mornlea_client_core_frame_pull` over `coreFramePull` with
    /// the same pointer discipline as the world pull; the pull is
    /// non-consuming and serves the retained per-step frame snapshot.
    fn mornlea_client_core_frame_pull(
        handle: ClientHandle,
        out: *mut u8,
        capacity: u32,
        required_out: *mut u32,
    ) -> Status;

    /// Same aligned, non-overlapping, two-phase discipline as frame pulls.
    /// This additive family never mutates or consumes the retained frame.
    fn mornlea_client_core_environment_pull(
        handle: ClientHandle,
        out: *mut u8,
        capacity: u32,
        required_out: *mut u32,
    ) -> Status;

    /// Mirrors `mornlea_client_core_status_pull` over `coreStatusPull` with
    /// the same pointer discipline as the world pull; the pull is
    /// non-consuming and serves bounded status and metrics records.
    fn mornlea_client_core_status_pull(
        handle: ClientHandle,
        out: *mut u8,
        capacity: u32,
        required_out: *mut u32,
    ) -> Status;

    /// Mirrors `mornlea_client_core_status_identity` over
    /// `coreStatusIdentity` with the same pointer discipline as the world
    /// pull; the pull is non-consuming and serves the producer identity
    /// record (identity header plus family descriptors).
    fn mornlea_client_core_status_identity(
        handle: ClientHandle,
        out: *mut u8,
        capacity: u32,
        required_out: *mut u32,
    ) -> Status;

    /// Mirrors `mornlea_client_core_abi_version` over `coreAbiVersion`: the
    /// packed producer identity with the ABI major in the high 32 bits and
    /// the minor in the low 32 bits; it takes no pointer arguments and
    /// cannot fail.
    fn mornlea_client_core_abi_version() -> u64;

    // Dynamic-loader entry points of the platform runtime library (libSystem
    // on macOS, libc on Linux resolve them without an explicit link
    // attribute). They serve only the production table below: the extension
    // resolves the colocated producer library at runtime instead of linking
    // it at build time, because the workspace builds from a clean checkout
    // before any producer library exists. A Windows distribution would need
    // its own loader entry points; the pilot distributes macOS only.
    fn dlopen(path: *const c_char, mode: c_int) -> *mut c_void;
    fn dlsym(handle: *mut c_void, symbol: *const c_char) -> *mut c_void;
    fn dladdr(address: *const c_void, info: *mut DlInfo) -> c_int;
}

/// The `dladdr` result record of the platform dynamic loader, mirroring the
/// system `<dlfcn.h>` layout; only `dli_fname` (the containing image's path)
/// is consumed.
#[repr(C)]
struct DlInfo {
    dli_fname: *const c_char,
    dli_fbase: *mut c_void,
    dli_sname: *const c_char,
    dli_saddr: *const c_void,
}

/// `RTLD_NOW | RTLD_LOCAL` for the producer load: resolve every producer
/// reference eagerly (a missing engine dependency fails the load instead of a
/// later call) and keep the producer's symbols out of the global namespace so
/// they cannot collide with Godot or Py4Godot symbols. Values are the darwin
/// `dlfcn.h` constants.
const RTLD_NOW: c_int = 0x2;
const RTLD_LOCAL: c_int = 0x4;

/// The file name of the producer library as materialized beside this
/// extension by `scripts/godot/build-core.sh`.
const PRODUCER_LIBRARY_NAME: &str = "libmornlea_client_core.dylib";

/// A function whose address anchors `dladdr` to this extension image so the
/// producer library can be resolved relative to the extension's own
/// directory (the colocated-distribution contract). Never called.
#[inline(never)]
fn extension_image_anchor() {}

/// One pull outcome of a single producer pull call, mapped from the raw
/// status word: the two-phase capacity signal and the completed write carry
/// their sizes, every other word (including out-of-range producer drift)
/// stays a raw word so the consumer-side classification owns the reaction.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PullOutcome {
    /// `STATUS_OK`; `written` is the byte count the producer committed to the
    /// caller's buffer.
    Complete { written: u32 },
    /// `STATUS_INSUFFICIENT_CAPACITY`; nothing was written and `required`
    /// names the exact byte count of the current record.
    Capacity { required: u32 },
    /// Any other (possibly undefined) status word.
    Status(u32),
}

/// The four pull families of the producer surface.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PullFamily {
    /// Confirmed Go-derived lighting tied to the retained frame identity.
    Environment,
    /// Consuming drain of the retained world batch.
    World,
    /// Non-consuming per-step frame snapshot.
    Frame,
    /// Non-consuming status and metrics record set.
    Status,
    /// Non-consuming producer identity record.
    Identity,
}

impl PullFamily {
    pub(crate) fn pull(
        self,
        calls: &dyn CoreCalls,
        handle: ClientHandle,
        buffer: &mut [u8],
    ) -> PullOutcome {
        match self {
            Self::Environment => calls.environment_pull(handle, buffer),
            Self::World => calls.world_pull(handle, buffer),
            Self::Frame => calls.frame_pull(handle, buffer),
            Self::Status => calls.status_pull(handle, buffer),
            Self::Identity => calls.identity_pull(handle, buffer),
        }
    }
}

/// The core-call seam: every bridge lifecycle operation goes through this
/// trait with typed, pointer-free arguments, so the bridge never sees a raw
/// pointer or a retained native buffer. Production wires the dynamically
/// resolved producer table; bridge lifecycle tests script the same surface
/// with producer-vocabulary statuses. Every method is panic-free on caller
/// data and returns raw status words (`u32`) so undefined producer words
/// surface unchanged instead of panicking a classification.
pub(crate) trait CoreCalls {
    /// Added environment projection; old scripted cores fail closed by default.
    fn environment_pull(&self, _handle: ClientHandle, _buffer: &mut [u8]) -> PullOutcome {
        PullOutcome::Status(abi::STATUS_INVALID_STATE)
    }
    /// The producer's packed ABI version (`major << 32 | minor`), or `None`
    /// when this table cannot reach a producer at all.
    fn abi_version(&self) -> Option<u64>;

    /// `mornlea_client_core_create` with its request words owned by the
    /// implementation. Each word packs a family identifier in the low 32
    /// bits and the requested family contract version in the high 32 bits.
    fn create_session(
        &self,
        abi_major: u32,
        abi_minor: u32,
        requested_families: &[u64],
    ) -> Result<ClientHandle, u32>;

    /// `mornlea_client_core_destroy`; idempotent for a handle the producer
    /// issued.
    fn destroy_session(&self, handle: ClientHandle) -> u32;

    /// `mornlea_client_core_connect_begin`; the address bytes are copied by
    /// the implementation and never retained.
    fn connect_begin(&self, handle: ClientHandle, address: &[u8]) -> u32;

    /// `mornlea_client_core_connect_poll`; the phase word on success, the
    /// status word otherwise.
    fn connect_poll(&self, handle: ClientHandle) -> Result<u32, u32>;

    /// `mornlea_client_core_disconnect`.
    fn disconnect_session(&self, handle: ClientHandle) -> u32;

    /// `mornlea_client_core_submit_input`; the batch bytes are copied by the
    /// implementation and never retained.
    fn submit_input(&self, handle: ClientHandle, batch: &[u8]) -> u32;

    /// `mornlea_client_core_step`; the frozen request record is copied by the
    /// implementation and never retained.
    fn step(&self, handle: ClientHandle, request: &[u8; abi::STEP_REQUEST_BYTES]) -> u32;

    /// `mornlea_client_core_world_pull`.
    fn world_pull(&self, handle: ClientHandle, buffer: &mut [u8]) -> PullOutcome;

    /// `mornlea_client_core_frame_pull`.
    fn frame_pull(&self, handle: ClientHandle, buffer: &mut [u8]) -> PullOutcome;

    /// `mornlea_client_core_status_pull`.
    fn status_pull(&self, handle: ClientHandle, buffer: &mut [u8]) -> PullOutcome;

    /// `mornlea_client_core_status_identity`.
    fn identity_pull(&self, handle: ClientHandle, buffer: &mut [u8]) -> PullOutcome;
}

/// Encode the frozen step-family request record (wire bytes read "MCS1"):
/// magic, layout version, little-endian elapsed nanoseconds, and the two
/// little-endian budgets. The producer owns every domain check; this encoder
/// only lays out the pinned record.
pub(crate) fn encode_step_request(
    elapsed_ns: u64,
    message_budget: u32,
    mesh_budget: u32,
) -> [u8; abi::STEP_REQUEST_BYTES] {
    let mut record = [0u8; abi::STEP_REQUEST_BYTES];
    record[0..4].copy_from_slice(&abi::MAGIC_STEP.to_le_bytes());
    record[4..8].copy_from_slice(&abi::STEP_VERSION.to_le_bytes());
    record[8..16].copy_from_slice(&elapsed_ns.to_le_bytes());
    record[16..20].copy_from_slice(&message_budget.to_le_bytes());
    record[20..24].copy_from_slice(&mesh_budget.to_le_bytes());
    record
}

/// An 8-byte-aligned byte buffer backed by `u64` words: every record buffer
/// handed to the producer must carry the ABI alignment, and `Vec<u8>` does
/// not guarantee it. The default value holds no allocation; callers either
/// fill it from bytes (input paths) or grow it to a declared span (the
/// reusable pull buffers of `crate::pull_buffers`).
#[derive(Default)]
pub(crate) struct AlignedBytes {
    words: Vec<u64>,
    length: usize,
}

impl AlignedBytes {
    fn from_bytes(bytes: &[u8]) -> Self {
        let mut words = vec![0u64; bytes.len().div_ceil(size_of::<u64>())];
        // Safety: `words` is a live `u64` allocation of at least `bytes.len()`
        // bytes, and only this module constructs the aligned view.
        let view = unsafe {
            core::slice::from_raw_parts_mut(words.as_mut_ptr().cast::<u8>(), bytes.len())
        };
        view.copy_from_slice(bytes);
        Self {
            words,
            length: bytes.len(),
        }
    }

    fn as_mut_ptr(&mut self) -> *mut u8 {
        self.words.as_mut_ptr().cast::<u8>()
    }

    fn len(&self) -> u32 {
        u32::try_from(self.length).unwrap_or(u32::MAX)
    }

    /// Copy out the first `length` bytes as an owned vector, leaving no view
    /// into the aligned backing alive.
    pub(crate) fn to_owned_prefix(&self, length: usize) -> Vec<u8> {
        let count = length.min(self.length);
        let mut result = vec![0u8; count];
        // Safety: read-only access inside the live `words` allocation.
        let view =
            unsafe { core::slice::from_raw_parts(self.words.as_ptr().cast::<u8>(), self.length) };
        result.copy_from_slice(&view[..count]);
        result
    }

    /// Set the exact-write span to `required` bytes, growing the backing to
    /// the requirement at word granularity when capacity is insufficient and
    /// never shrinking it (the largest span ever served defines the
    /// steady-state footprint). Returns whether the backing grew.
    pub(crate) fn reserve_span(&mut self, required: usize) -> bool {
        let needed_words = required.div_ceil(size_of::<u64>());
        if needed_words > self.words.len() {
            self.words.resize(needed_words, 0);
            self.length = required;
            true
        } else {
            self.length = required;
            false
        }
    }

    /// Run `call` with the writable byte view of the current span. The view
    /// exists only inside the call, so no aliasing outlives the producer's
    /// synchronous copy.
    pub(crate) fn with_span_view<R>(&mut self, call: impl FnOnce(&mut [u8]) -> R) -> R {
        debug_assert!(self.length <= self.words.len() * size_of::<u64>());
        // Safety: the view stays inside the live `words` allocation, the
        // span invariant is maintained by `reserve_span` and `from_bytes`,
        // and the confined view cannot outlive this call.
        let view = unsafe { core::slice::from_raw_parts_mut(self.as_mut_ptr(), self.length) };
        call(view)
    }

    /// Read one little-endian `u64` at byte `offset` of the current span
    /// without constructing a view; the header decoders read a handful of
    /// fixed offsets only.
    pub(crate) fn le_u64_at(&self, offset: usize) -> u64 {
        debug_assert!(offset + size_of::<u64>() <= self.length);
        let mut value = 0u64;
        for index in 0..size_of::<u64>() {
            let byte_index = offset + index;
            let word = self.words[byte_index / size_of::<u64>()];
            let shift = (byte_index % size_of::<u64>()) * 8;
            value |= u64::from((word >> shift) as u8) << (index * 8);
        }
        value
    }

    /// Test-only byte capacity of the backing (word granularity).
    #[cfg(test)]
    pub(crate) fn capacity_bytes(&self) -> usize {
        self.words.len() * size_of::<u64>()
    }
}

/// The twelve producer exports as resolved function pointers. Field types
/// are pinned against the extern declarations by a source test; the field
/// order follows [`EXPORT_SYMBOLS`].
#[derive(Clone, Copy)]
struct ProducerVTable {
    create: unsafe extern "C" fn(u32, u32, *const u64, u32, *mut ClientHandle) -> Status,
    destroy: unsafe extern "C" fn(ClientHandle) -> Status,
    connect_begin: unsafe extern "C" fn(ClientHandle, *const u8, u32) -> Status,
    connect_poll: unsafe extern "C" fn(ClientHandle, *mut u32) -> Status,
    disconnect: unsafe extern "C" fn(ClientHandle) -> Status,
    submit_input: unsafe extern "C" fn(ClientHandle, *const u8, u32) -> Status,
    step: unsafe extern "C" fn(ClientHandle, *const u8, u32) -> Status,
    world_pull: unsafe extern "C" fn(ClientHandle, *mut u8, u32, *mut u32) -> Status,
    frame_pull: unsafe extern "C" fn(ClientHandle, *mut u8, u32, *mut u32) -> Status,
    environment_pull: unsafe extern "C" fn(ClientHandle, *mut u8, u32, *mut u32) -> Status,
    status_pull: unsafe extern "C" fn(ClientHandle, *mut u8, u32, *mut u32) -> Status,
    status_identity: unsafe extern "C" fn(ClientHandle, *mut u8, u32, *mut u32) -> Status,
    abi_version: unsafe extern "C" fn() -> u64,
}

/// Why the producer table is unavailable. Kept `Copy` so the cached load
/// result can carry the reason without allocation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ProducerLoadFailure {
    /// This extension's own image directory could not be resolved.
    ImageUnknown,
    /// The producer library is not present beside this extension.
    LibraryAbsent,
    /// A producer export symbol is missing from the loaded library.
    SymbolMissing,
    /// The loaded producer carries a different ABI major.
    MajorMismatch { found: u32 },
    /// The cargo-test configuration never loads the producer library.
    TestBinary,
}

impl ProducerLoadFailure {
    pub(crate) fn reason(self) -> &'static str {
        match self {
            Self::ImageUnknown => "extension image directory is unknown",
            Self::LibraryAbsent => "producer library is absent beside the extension",
            Self::SymbolMissing => "producer export symbol is missing",
            Self::MajorMismatch { .. } => "producer ABI major does not match the pinned ABI",
            Self::TestBinary => "test builds never load the producer library",
        }
    }
}

/// The cached one-shot producer load.
#[derive(Clone, Copy)]
enum ProducerTable {
    Ready {
        vtable: ProducerVTable,
        packed_version: u64,
    },
    Failed(ProducerLoadFailure),
}

static PRODUCER_TABLE: OnceLock<ProducerTable> = OnceLock::new();

/// The producer identity for the Godot-visible availability report: the
/// packed version on success, or the load failure reason.
pub(crate) struct ProducerIdentity {
    pub(crate) packed_version: Option<u64>,
    pub(crate) failure: Option<ProducerLoadFailure>,
}

pub(crate) fn producer_identity() -> ProducerIdentity {
    match producer_table() {
        ProducerTable::Ready { packed_version, .. } => ProducerIdentity {
            packed_version: Some(packed_version),
            failure: None,
        },
        ProducerTable::Failed(failure) => ProducerIdentity {
            packed_version: None,
            failure: Some(failure),
        },
    }
}

fn producer_table() -> ProducerTable {
    *PRODUCER_TABLE.get_or_init(load_producer_table)
}

/// Resolve the colocated producer library once and verify its ABI major.
/// The opened image stays loaded for the process lifetime by design: Godot
/// extensions are never unloaded mid-run, `dlclose` would invalidate every
/// vtable pointer still held by live bridge sessions, and a hot-reloaded
/// extension image gets fresh statics and its own `dlopen` reference.
fn load_producer_table() -> ProducerTable {
    use std::os::unix::ffi::{OsStrExt, OsStringExt};
    let Some(directory) = extension_directory() else {
        return ProducerTable::Failed(ProducerLoadFailure::ImageUnknown);
    };
    let mut path_bytes = directory.into_os_string().into_vec();
    path_bytes.push(b'/');
    path_bytes.extend_from_slice(PRODUCER_LIBRARY_NAME.as_bytes());
    path_bytes.push(0);
    let library_path = std::ffi::OsString::from_vec(path_bytes);
    // Safety: `library_path` is NUL-terminated by construction and `dlopen`
    // only reads it.
    let handle = unsafe {
        dlopen(
            library_path.as_bytes().as_ptr().cast::<c_char>(),
            RTLD_NOW | RTLD_LOCAL,
        )
    };
    if handle.is_null() {
        return ProducerTable::Failed(ProducerLoadFailure::LibraryAbsent);
    }
    macro_rules! symbol {
        ($name:literal) => {{
            let bytes = concat!($name, "\0");
            // Safety: the name is a static NUL-terminated string and `handle`
            // is a live `dlopen` result. The transmute reinterprets the
            // resolved address as the function pointer type pinned against
            // the extern declarations.
            let address = unsafe { dlsym(handle, bytes.as_ptr().cast::<c_char>()) };
            if address.is_null() {
                return ProducerTable::Failed(ProducerLoadFailure::SymbolMissing);
            }
            unsafe { core::mem::transmute::<*mut c_void, _>(address) }
        }};
    }
    // The transmute target inside `symbol!` is inferred from the vtable
    // field the result initializes, and the signature pin ties every field
    // to its pinned declaration, so an explicit annotation would only
    // duplicate the type text.
    #[allow(clippy::missing_transmute_annotations)]
    let vtable = ProducerVTable {
        create: symbol!("mornlea_client_core_create"),
        destroy: symbol!("mornlea_client_core_destroy"),
        connect_begin: symbol!("mornlea_client_core_connect_begin"),
        connect_poll: symbol!("mornlea_client_core_connect_poll"),
        disconnect: symbol!("mornlea_client_core_disconnect"),
        submit_input: symbol!("mornlea_client_core_submit_input"),
        step: symbol!("mornlea_client_core_step"),
        world_pull: symbol!("mornlea_client_core_world_pull"),
        frame_pull: symbol!("mornlea_client_core_frame_pull"),
        environment_pull: symbol!("mornlea_client_core_environment_pull"),
        status_pull: symbol!("mornlea_client_core_status_pull"),
        status_identity: symbol!("mornlea_client_core_status_identity"),
        abi_version: symbol!("mornlea_client_core_abi_version"),
    };
    // Safety: the resolved symbol takes no arguments and returns a word.
    let packed_version = unsafe { (vtable.abi_version)() };
    let found_major = (packed_version >> 32) as u32;
    if found_major != abi::ABI_MAJOR {
        return ProducerTable::Failed(ProducerLoadFailure::MajorMismatch { found: found_major });
    }
    ProducerTable::Ready {
        vtable,
        packed_version,
    }
}

/// The directory of this extension's own image, resolved through `dladdr` on
/// a module-local anchor function.
fn extension_directory() -> Option<std::path::PathBuf> {
    let mut info = DlInfo {
        dli_fname: core::ptr::null(),
        dli_fbase: core::ptr::null_mut(),
        dli_sname: core::ptr::null(),
        dli_saddr: core::ptr::null(),
    };
    // Safety: `DlInfo` is the platform `Dl_info` layout and `dladdr` only
    // writes into it for a valid in-image address.
    let found = unsafe { dladdr(extension_image_anchor as *const c_void, &mut info) };
    if found == 0 || info.dli_fname.is_null() {
        return None;
    }
    // Safety: `dli_fname` is a NUL-terminated path owned by the loader for
    // the lifetime of the image.
    let file_name = unsafe { core::ffi::CStr::from_ptr(info.dli_fname) };
    use std::os::unix::ffi::OsStrExt;
    let path = std::path::PathBuf::from(std::ffi::OsStr::from_bytes(file_name.to_bytes()));
    path.parent().map(std::path::Path::to_path_buf)
}

/// The production [`CoreCalls`] table over the dynamically resolved producer
/// exports. Every method owns its aligned input buffers and copies pull
/// results out of the aligned scratch before returning.
struct ProducerCore {
    vtable: ProducerVTable,
}

/// The [`CoreCalls`] table used when no producer library could be loaded:
/// every call reports the internal status word (the obligation baseline for
/// a producer-side failure with no narrower classification), so a session
/// never retains partial state behind an absent producer.
struct UnavailableCore {
    failure: ProducerLoadFailure,
}

impl CoreCalls for ProducerCore {
    fn abi_version(&self) -> Option<u64> {
        // Safety: the resolved symbol takes no arguments and returns a word.
        Some(unsafe { (self.vtable.abi_version)() })
    }

    fn create_session(
        &self,
        abi_major: u32,
        abi_minor: u32,
        requested_families: &[u64],
    ) -> Result<ClientHandle, u32> {
        let count = u32::try_from(requested_families.len()).unwrap_or(u32::MAX);
        let requested = if requested_families.is_empty() {
            // The bridge always requests the full pilot family set, so this
            // branch is a defensive guard making the empty-slice case total.
            // An empty slice owns no allocation, and `as_ptr` would yield a
            // dangling aligned address (8) — the same Go stack-scan hazard as
            // the pull path (see `pull_via`): the producer's cgo boundary
            // stores the pointer in Go pointer-typed stack variables, and the
            // Go runtime aborts on addresses below its minimum legal pointer
            // (4096). The producer rejects a zero count before reading the
            // pointer, so null preserves the outcome.
            core::ptr::null()
        } else {
            requested_families.as_ptr()
        };
        let mut out_handle = ClientHandle::from_word(0);
        // Safety: `requested_families` is a live u64 slice for the duration
        // of the synchronous call, and `out_handle` is a live out-parameter.
        let status = unsafe {
            (self.vtable.create)(abi_major, abi_minor, requested, count, &mut out_handle)
        };
        if status.is_ok() {
            Ok(out_handle)
        } else {
            Err(status.as_word())
        }
    }

    fn destroy_session(&self, handle: ClientHandle) -> u32 {
        // Safety: the handle value is a plain word passed by copy.
        unsafe { (self.vtable.destroy)(handle).as_word() }
    }

    fn connect_begin(&self, handle: ClientHandle, address: &[u8]) -> u32 {
        if address.is_empty() {
            // The producer rejects a zero-length address on its length before
            // reading any pointer, but its cgo boundary still stores the
            // pointer in Go pointer-typed stack variables, and an empty
            // `AlignedBytes` owns no allocation, so `as_mut_ptr` would cross
            // the dangling aligned address (8) — the same Go stack-scan
            // hazard as the pull path (see `pull_via`), where the runtime
            // aborts on addresses below its minimum legal pointer (4096).
            // Null is the one placeholder the boundary always tolerates.
            return unsafe { (self.vtable.connect_begin)(handle, core::ptr::null(), 0).as_word() };
        }
        let mut buffer = AlignedBytes::from_bytes(address);
        // Safety: the aligned buffer is live for the synchronous call and the
        // producer copies the bytes without retaining the pointer.
        unsafe { (self.vtable.connect_begin)(handle, buffer.as_mut_ptr(), buffer.len()).as_word() }
    }

    fn connect_poll(&self, handle: ClientHandle) -> Result<u32, u32> {
        let mut phase = 0u32;
        // Safety: `phase` is a live out-parameter for the synchronous call.
        let status = unsafe { (self.vtable.connect_poll)(handle, &mut phase) };
        if status.is_ok() {
            Ok(phase)
        } else {
            Err(status.as_word())
        }
    }

    fn disconnect_session(&self, handle: ClientHandle) -> u32 {
        // Safety: the handle value is a plain word passed by copy.
        unsafe { (self.vtable.disconnect)(handle).as_word() }
    }

    fn submit_input(&self, handle: ClientHandle, batch: &[u8]) -> u32 {
        if batch.is_empty() {
            // The producer rejects a zero-length batch on its length before
            // reading any pointer, but its cgo boundary still stores the
            // pointer in Go pointer-typed stack variables, and an empty
            // `AlignedBytes` owns no allocation, so `as_mut_ptr` would cross
            // the dangling aligned address (8) — the same Go stack-scan
            // hazard as the pull path (see `pull_via`), where the runtime
            // aborts on addresses below its minimum legal pointer (4096).
            // Null is the one placeholder the boundary always tolerates.
            return unsafe { (self.vtable.submit_input)(handle, core::ptr::null(), 0).as_word() };
        }
        let mut buffer = AlignedBytes::from_bytes(batch);
        // Safety: the aligned buffer is live for the synchronous call and the
        // producer copies the batch without retaining the pointer.
        unsafe { (self.vtable.submit_input)(handle, buffer.as_mut_ptr(), buffer.len()).as_word() }
    }

    fn step(&self, handle: ClientHandle, request: &[u8; abi::STEP_REQUEST_BYTES]) -> u32 {
        let mut buffer = AlignedBytes::from_bytes(request);
        // Safety: the aligned buffer is live for the synchronous call and the
        // producer copies the record without retaining the pointer.
        unsafe { (self.vtable.step)(handle, buffer.as_mut_ptr(), buffer.len()).as_word() }
    }

    fn world_pull(&self, handle: ClientHandle, buffer: &mut [u8]) -> PullOutcome {
        self.pull_via(
            // Safety: the resolved export copies synchronously through the
            // pointers `pull_via` owns for the call's duration.
            |core, out, capacity, required| unsafe {
                (core.vtable.world_pull)(handle, out, capacity, required)
            },
            buffer,
        )
    }

    fn frame_pull(&self, handle: ClientHandle, buffer: &mut [u8]) -> PullOutcome {
        self.pull_via(
            // Safety: see `world_pull`.
            |core, out, capacity, required| unsafe {
                (core.vtable.frame_pull)(handle, out, capacity, required)
            },
            buffer,
        )
    }

    fn environment_pull(&self, handle: ClientHandle, buffer: &mut [u8]) -> PullOutcome {
        self.pull_via(
            // Safety: `pull_via` owns valid aligned buffers and the size word
            // for this synchronous call, including null for capacity queries.
            |core, out, capacity, required| unsafe {
                (core.vtable.environment_pull)(handle, out, capacity, required)
            },
            buffer,
        )
    }

    fn status_pull(&self, handle: ClientHandle, buffer: &mut [u8]) -> PullOutcome {
        self.pull_via(
            // Safety: see `world_pull`.
            |core, out, capacity, required| unsafe {
                (core.vtable.status_pull)(handle, out, capacity, required)
            },
            buffer,
        )
    }

    fn identity_pull(&self, handle: ClientHandle, buffer: &mut [u8]) -> PullOutcome {
        self.pull_via(
            // Safety: see `world_pull`.
            |core, out, capacity, required| unsafe {
                (core.vtable.status_identity)(handle, out, capacity, required)
            },
            buffer,
        )
    }
}

impl ProducerCore {
    /// One pull call with the size out-parameter owned here. A zero-capacity
    /// query passes a null `out` pointer: the producer's pointer discipline
    /// only requires `out` for a positive capacity, and null is the one
    /// placeholder its Go runtime always tolerates (see the empty-buffer
    /// branch below).
    fn pull_via(
        &self,
        call: impl FnOnce(&Self, *mut u8, u32, *mut u32) -> Status,
        buffer: &mut [u8],
    ) -> PullOutcome {
        let mut required = 0u32;
        let capacity = u32::try_from(buffer.len()).unwrap_or(u32::MAX);
        let out = if buffer.is_empty() {
            // A zero-capacity query reads no buffer byte, but the producer's
            // cgo boundary still stores `out` in Go pointer-typed stack
            // variables while the call runs, and the Go runtime's stack scan
            // aborts the whole process when a pointer-typed slot holds any
            // address below its minimum legal pointer (4096) — even one the
            // call never dereferences. A non-null placeholder such as
            // `NonNull::<u8>::dangling()` (address 1) is therefore a latent
            // crash; null is accepted by every pull export at capacity zero.
            core::ptr::null_mut()
        } else {
            buffer.as_mut_ptr()
        };
        let status = call(self, out, capacity, &mut required);
        if status.is_ok() {
            PullOutcome::Complete { written: capacity }
        } else if status.as_word() == abi::STATUS_INSUFFICIENT_CAPACITY {
            PullOutcome::Capacity { required }
        } else {
            PullOutcome::Status(status.as_word())
        }
    }
}

impl CoreCalls for UnavailableCore {
    fn abi_version(&self) -> Option<u64> {
        None
    }

    fn create_session(
        &self,
        _abi_major: u32,
        _abi_minor: u32,
        _requested_families: &[u64],
    ) -> Result<ClientHandle, u32> {
        Err(abi::STATUS_INTERNAL)
    }

    fn destroy_session(&self, _handle: ClientHandle) -> u32 {
        abi::STATUS_INTERNAL
    }

    fn connect_begin(&self, _handle: ClientHandle, _address: &[u8]) -> u32 {
        abi::STATUS_INTERNAL
    }

    fn connect_poll(&self, _handle: ClientHandle) -> Result<u32, u32> {
        Err(abi::STATUS_INTERNAL)
    }

    fn disconnect_session(&self, _handle: ClientHandle) -> u32 {
        abi::STATUS_INTERNAL
    }

    fn submit_input(&self, _handle: ClientHandle, _batch: &[u8]) -> u32 {
        abi::STATUS_INTERNAL
    }

    fn step(&self, _handle: ClientHandle, _request: &[u8; abi::STEP_REQUEST_BYTES]) -> u32 {
        abi::STATUS_INTERNAL
    }

    fn world_pull(&self, _handle: ClientHandle, _buffer: &mut [u8]) -> PullOutcome {
        PullOutcome::Status(abi::STATUS_INTERNAL)
    }

    fn frame_pull(&self, _handle: ClientHandle, _buffer: &mut [u8]) -> PullOutcome {
        PullOutcome::Status(abi::STATUS_INTERNAL)
    }

    fn status_pull(&self, _handle: ClientHandle, _buffer: &mut [u8]) -> PullOutcome {
        PullOutcome::Status(abi::STATUS_INTERNAL)
    }

    fn identity_pull(&self, _handle: ClientHandle, _buffer: &mut [u8]) -> PullOutcome {
        PullOutcome::Status(abi::STATUS_INTERNAL)
    }
}

/// The production core-call table for new bridge sessions. Outside tests it
/// wires the dynamically resolved producer exports (or the fail-closed
/// unavailable table when the library could not be loaded); in cargo tests it
/// stays the unavailable table because no test may load the producer
/// artifact.
#[cfg(not(test))]
pub(crate) fn production_core_calls() -> Box<dyn CoreCalls> {
    match producer_table() {
        ProducerTable::Ready { vtable, .. } => Box::new(ProducerCore { vtable }),
        ProducerTable::Failed(failure) => Box::new(UnavailableCore { failure }),
    }
}

#[cfg(test)]
pub(crate) fn production_core_calls() -> Box<dyn CoreCalls> {
    Box::new(UnavailableCore {
        failure: ProducerLoadFailure::TestBinary,
    })
}

/// The safe Rust core adapter: the rust-producer half of this module.
///
/// The extern mirror above talks to the Go c-shared pilot producer and stays
/// explicitly pilot-only behind the `session_*` bridge methods. Everything
/// below owns the other mode: the `rust-client-core` producer whose sessions
/// are real C1 endpoint values behind a private token arena. Rust mode never
/// touches the dynamic loader, the resolved vtable or [`CoreCalls`]; opening
/// a core here cannot load the Go library even incidentally.
///
/// The adapter is engine-independent: it consumes and produces
/// [`abi::boundary::BoundaryValue`] copies, so the exported Godot methods and
/// the engine-free adapter tests call the identical routines — there is no
/// test-specific second implementation to drift from.
pub mod rust_core {
    use std::panic::AssertUnwindSafe;
    use std::sync::Arc;
    use std::time::Duration;

    use mornlea_client_core::contracts::{
        ClientConfig, ClientCore, ClientEndpoint, ClientError, ClientIdentity, ClientLimits,
        ConnectorRegistry, Endpoint, InputReceipt, MonotonicClock, SessionEpoch, StdMonotonicClock,
    };
    use mornlea_client_core::input::InputBatch;
    use mornlea_client_core::session::login::LoginSession;

    use crate::abi::boundary::{self, BoundaryValue, CoreOpenSpec, CoreTokenValue};
    use crate::feature_negotiation::{
        RUST_PRODUCER_FAMILIES, RUST_PRODUCER_FAMILY_RECORD_LIMIT, RUST_PRODUCER_NAME,
    };

    /// One arena slot's live core: the checked C1 configuration owner beside
    /// the live endpoint provider. The endpoint is the C1 substitution port,
    /// so the production path stores the real `LoginSession` and
    /// deterministic wrappers can only wrap a real endpoint, never replace
    /// the adapter routines themselves.
    struct CoreSlot {
        core: ClientCore,
        endpoint: Box<dyn ClientEndpoint>,
        limits: ClientLimits,
        /// The last successfully rendered whole-frame copy. It exists only
        /// after a pull validated a complete frame, and a failed pull never
        /// touches it.
        visible: Option<BoundaryValue>,
    }

    /// The private core-token arena: nonzero slots, one generation per slot,
    /// and one release per issued token.
    ///
    /// A token addresses a slot only while its generation matches; a stale
    /// slot or generation fails the lookup with `InvalidState` before any
    /// core dereference, so a rejected caller observes zero core calls. The
    /// first close of a live token releases the slot exactly once and
    /// advances its generation; every later close of the same issued token
    /// succeeds without another release.
    #[derive(Default)]
    pub struct CoreArena {
        slots: Vec<Option<CoreSlot>>,
        generations: Vec<u64>,
        releases: usize,
    }

    impl CoreArena {
        pub fn new() -> Self {
            Self::default()
        }

        /// The number of arena-level releases that ran. One per issued token
        /// regardless of how many times that token is closed.
        pub fn releases(&self) -> usize {
            self.releases
        }

        /// The live slot index a token addresses, or `None` for an unknown
        /// slot or a stale generation. This is the one lookup gate every
        /// core-touching routine runs first.
        fn live_index(&self, token: CoreTokenValue) -> Option<usize> {
            let index = token.slot().checked_sub(1)? as usize;
            let generation = *self.generations.get(index)?;
            if generation != token.generation() {
                return None;
            }
            self.slots.get(index)?.is_some().then_some(index)
        }

        fn slot(&self, token: CoreTokenValue) -> Result<&CoreSlot, ClientError> {
            let index = self.live_index(token).ok_or(ClientError::InvalidState)?;
            self.slots
                .get(index)
                .and_then(Option::as_ref)
                .ok_or(ClientError::Internal)
        }

        fn slot_mut(&mut self, token: CoreTokenValue) -> Result<&mut CoreSlot, ClientError> {
            let index = self.live_index(token).ok_or(ClientError::InvalidState)?;
            self.slots
                .get_mut(index)
                .and_then(Option::as_mut)
                .ok_or(ClientError::Internal)
        }

        /// The production open path: the safe Rust core over the real C1
        /// login provider. The bridge passes its native clock and connector
        /// registry; the boundary value carries only checked numbers. The
        /// checked configuration owner validates first, and the endpoint's
        /// own configuration is derived from that owner's accessors so both
        /// halves always agree.
        pub fn open(
            &mut self,
            spec: &CoreOpenSpec,
            clock: Arc<dyn MonotonicClock>,
            connectors: Arc<ConnectorRegistry>,
        ) -> Result<CoreTokenValue, ClientError> {
            let config = checked_config(spec, clock, connectors)?;
            let core = ClientCore::new(config)?;
            let endpoint_config = ClientConfig::try_new(
                *core.config().limits(),
                core.config().hello_timeout(),
                core.config().login_timeout(),
                Arc::clone(core.config().clock()),
                Arc::clone(core.config().connectors()),
            )?;
            let endpoint = LoginSession::new(endpoint_config)?;
            self.install(core, Box::new(endpoint))
        }

        /// The deterministic-substitution open path over the same slot,
        /// generation and lifecycle code. It accepts a wrapper around a real
        /// C1 endpoint so call counts, injected frames and caught panics are
        /// observable without a second copy of any adapter routine.
        pub fn open_with_endpoint(
            &mut self,
            spec: &CoreOpenSpec,
            endpoint: Box<dyn ClientEndpoint>,
        ) -> Result<CoreTokenValue, ClientError> {
            let config = checked_config(
                spec,
                Arc::new(StdMonotonicClock),
                Arc::new(ConnectorRegistry::new()),
            )?;
            self.install(ClientCore::new(config)?, endpoint)
        }

        /// Installs one checked core under a fresh token. The C1 owner has
        /// already validated the configuration, so a refused open never
        /// reached here and no token exists.
        fn install(
            &mut self,
            core: ClientCore,
            endpoint: Box<dyn ClientEndpoint>,
        ) -> Result<CoreTokenValue, ClientError> {
            let limits = *core.config().limits();
            let slot = CoreSlot {
                core,
                endpoint,
                limits,
                visible: None,
            };
            let index = if let Some(free) = self.slots.iter().position(Option::is_none) {
                free
            } else {
                self.slots.push(None);
                self.generations.push(0);
                self.slots.len() - 1
            };
            let generation = self.generations[index] + 1;
            self.generations[index] = generation;
            self.slots[index] = Some(slot);
            CoreTokenValue::new(
                u32::try_from(index + 1).map_err(|_| ClientError::Capacity)?,
                generation,
            )
        }

        /// The last successfully rendered whole-frame copy of one slot.
        pub fn visible_frame(&self, token: CoreTokenValue) -> Result<&BoundaryValue, ClientError> {
            self.slot(token)?
                .visible
                .as_ref()
                .ok_or(ClientError::InvalidState)
        }

        /// Begins one connection: a pending admission epoch only.
        pub fn connect(
            &mut self,
            token: CoreTokenValue,
            endpoint: Endpoint,
            identity: ClientIdentity,
        ) -> Result<SessionEpoch, ClientError> {
            self.slot_mut(token)?.endpoint.connect(endpoint, identity)
        }

        /// Submits one whole checked input batch.
        pub fn submit(
            &mut self,
            token: CoreTokenValue,
            batch: InputBatch,
        ) -> Result<InputReceipt, ClientError> {
            self.slot_mut(token)?
                .endpoint
                .submit_input(batch.epoch(), batch)
        }

        /// Drives exactly one bounded step.
        pub fn step(
            &mut self,
            token: CoreTokenValue,
            epoch: SessionEpoch,
            work: mornlea_client_core::contracts::ClientWorkBudget,
        ) -> Result<mornlea_client_core::contracts::StepReport, ClientError> {
            self.slot_mut(token)?.endpoint.step(epoch, work)
        }

        /// Pulls, validates and renders the whole visible frame as one
        /// owned boundary copy.
        ///
        /// The snapshot is rendered against the slot's frozen limits before
        /// anything is stored: a frame that fails the whole-frame checks
        /// (mixed headers, duplicate or missing required family, capacity)
        /// returns the typed error and leaves the slot's prior visible copy
        /// untouched. The returned value owns every byte — strings, vectors
        /// and field lists — so it stays valid after the core's own frame is
        /// released.
        pub fn pull_frame(
            &mut self,
            token: CoreTokenValue,
            epoch: SessionEpoch,
        ) -> Result<BoundaryValue, ClientError> {
            let slot = self.slot_mut(token)?;
            let frame = slot.endpoint.snapshot(epoch)?;
            let rendered = boundary::render_frame(&frame, &slot.limits)?;
            slot.visible = Some(rendered.clone());
            Ok(rendered)
        }

        /// Resets the live session onto a fresh epoch.
        pub fn reset(
            &mut self,
            token: CoreTokenValue,
            epoch: SessionEpoch,
        ) -> Result<SessionEpoch, ClientError> {
            self.slot_mut(token)?.endpoint.reset(epoch)
        }

        /// Releases one token's core. The first close of a live token drops
        /// the slot, advances its generation and counts one release; the
        /// endpoint's own terminal path runs inside that drop-adjacent call,
        /// and its typed refusal for a session that never connected is not a
        /// facade failure because there is nothing left to terminate. Every
        /// later close of the same issued token succeeds without another
        /// release.
        pub fn close(&mut self, token: CoreTokenValue) -> Result<(), ClientError> {
            let Some(index) = self.live_index(token) else {
                return Ok(());
            };
            if let Some(mut slot) = self.slots[index].take() {
                let _ = slot.endpoint.close();
                self.generations[index] += 1;
                self.releases += 1;
            }
            Ok(())
        }

        /// Renders the frozen producer family table for one live core.
        pub fn family_table(&self, token: CoreTokenValue) -> Result<BoundaryValue, ClientError> {
            self.slot(token)?;
            Ok(BoundaryValue::fields([
                (
                    "producer",
                    BoundaryValue::Text(RUST_PRODUCER_NAME.to_string()),
                ),
                (
                    "descriptors",
                    BoundaryValue::List(
                        RUST_PRODUCER_FAMILIES
                            .iter()
                            .map(|descriptor| {
                                BoundaryValue::fields([
                                    (
                                        "logical_name",
                                        BoundaryValue::Text(descriptor.logical_name.to_string()),
                                    ),
                                    (
                                        "numeric_id",
                                        BoundaryValue::Int(i64::from(descriptor.numeric_id)),
                                    ),
                                    ("major", BoundaryValue::Int(i64::from(descriptor.major))),
                                    ("minor", BoundaryValue::Int(i64::from(descriptor.minor))),
                                    (
                                        "record_limit",
                                        boundary::u64_text(RUST_PRODUCER_FAMILY_RECORD_LIMIT),
                                    ),
                                    ("record_bytes", boundary::u64_text(descriptor.record_bytes)),
                                ])
                            })
                            .collect(),
                    ),
                ),
            ]))
        }
    }

    /// Builds the checked configuration one open path hands to the C1 owner.
    /// The deadline policy is the facade's frozen five- and ten-second
    /// defaults expressed in the boundary's millisecond fields; a zero
    /// deadline rejects before any core exists.
    fn checked_config(
        spec: &CoreOpenSpec,
        clock: Arc<dyn MonotonicClock>,
        connectors: Arc<ConnectorRegistry>,
    ) -> Result<ClientConfig, ClientError> {
        ClientConfig::try_new(
            spec.limits,
            Duration::from_millis(u64::from(spec.hello_ms)),
            Duration::from_millis(u64::from(spec.login_ms)),
            clock,
            connectors,
        )
    }

    /// The panic guard every facade call runs under: a caught panic maps to
    /// the `Internal` failure through the same closed envelope, and the
    /// caller's prior visible state is untouched because every routine above
    /// validates into temporaries before it stores anything.
    pub fn guarded<T>(call: impl FnOnce() -> Result<T, ClientError>) -> Result<T, ClientError> {
        std::panic::catch_unwind(AssertUnwindSafe(call)).unwrap_or(Err(ClientError::Internal))
    }

    impl CoreArena {
        // -------------------------------------------------------------
        // The eight facade routines the exported Godot methods call. Each
        // one decodes its arguments, runs the matching core operation and
        // renders the closed envelope. The decode happens before any arena
        // lookup, so a rejected decoding makes zero core calls.
        // -------------------------------------------------------------

        /// `open_core`: opens the safe Rust core and answers its token.
        pub fn open_core_routine(
            &mut self,
            config: &BoundaryValue,
            clock: Arc<dyn MonotonicClock>,
            connectors: Arc<ConnectorRegistry>,
        ) -> BoundaryValue {
            boundary::outcome(guarded(|| {
                let spec = boundary::decode_open_core(config)?;
                self.open(&spec, clock, connectors)
                    .map(CoreTokenValue::to_boundary)
            }))
        }

        /// `connect`: pending admission only.
        pub fn connect_routine(
            &mut self,
            token: &BoundaryValue,
            endpoint: &BoundaryValue,
            identity: &BoundaryValue,
        ) -> BoundaryValue {
            boundary::outcome(guarded(|| {
                let token = CoreTokenValue::from_boundary(token)?;
                let endpoint = boundary::decode_endpoint(endpoint)?;
                let identity = boundary::decode_identity(identity)?;
                self.connect(token, endpoint, identity)
                    .map(|epoch| boundary::u64_text(epoch.get()))
            }))
        }

        /// `submit_typed_input`: one whole checked batch, decoded before any
        /// core call.
        pub fn submit_routine(
            &mut self,
            token: &BoundaryValue,
            epoch: &BoundaryValue,
            batch: &BoundaryValue,
        ) -> BoundaryValue {
            boundary::outcome(guarded(|| {
                let batch = boundary::decode_input_batch(batch)?;
                let epoch = boundary::decode_epoch(epoch)?;
                if epoch != batch.epoch() {
                    return Err(ClientError::StaleEpoch);
                }
                let token = CoreTokenValue::from_boundary(token)?;
                self.submit(token, batch)
                    .map(|receipt| boundary::render_receipt(&receipt))
            }))
        }

        /// `step`: one bounded step and the checked report copy.
        pub fn step_routine(
            &mut self,
            token: &BoundaryValue,
            epoch: &BoundaryValue,
            work: &BoundaryValue,
        ) -> BoundaryValue {
            boundary::outcome(guarded(|| {
                let token = CoreTokenValue::from_boundary(token)?;
                let epoch = boundary::decode_epoch(epoch)?;
                let work = boundary::decode_work(work)?;
                self.step(token, epoch, work)
                    .map(|report| boundary::render_step_report(&report))
            }))
        }

        /// `pull_typed_frame`: the validated whole frame as one owned copy.
        pub fn pull_frame_routine(
            &mut self,
            token: &BoundaryValue,
            epoch: &BoundaryValue,
        ) -> BoundaryValue {
            boundary::outcome(guarded(|| {
                let token = CoreTokenValue::from_boundary(token)?;
                let epoch = boundary::decode_epoch(epoch)?;
                self.pull_frame(token, epoch)
            }))
        }

        /// `family_table`: the frozen producer descriptor table.
        pub fn family_table_routine(&mut self, token: &BoundaryValue) -> BoundaryValue {
            boundary::outcome(guarded(|| {
                let token = CoreTokenValue::from_boundary(token)?;
                self.family_table(token)
            }))
        }

        /// `reset`: the fresh epoch after invalidation.
        pub fn reset_routine(
            &mut self,
            token: &BoundaryValue,
            epoch: &BoundaryValue,
        ) -> BoundaryValue {
            boundary::outcome(guarded(|| {
                let token = CoreTokenValue::from_boundary(token)?;
                let epoch = boundary::decode_epoch(epoch)?;
                self.reset(token, epoch)
                    .map(|epoch| boundary::u64_text(epoch.get()))
            }))
        }

        /// `close`: releases the token; repeated close of the same issued
        /// token succeeds without another release.
        pub fn close_routine(&mut self, token: &BoundaryValue) -> BoundaryValue {
            boundary::outcome(guarded(|| {
                let token = CoreTokenValue::from_boundary(token)?;
                self.close(token).map(|()| BoundaryValue::Null)
            }))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        ClientHandle, CoreCalls, EXPORT_SYMBOLS, ProducerCore, ProducerVTable, PullOutcome, Status,
        status_text,
    };
    use crate::abi::{
        self, IDENTITY_HEADER_BYTES, STATUS_ABI_MISMATCH, STATUS_COUNT, STATUS_DISCONNECTED,
        STATUS_INPUT_REJECTED, STATUS_INSUFFICIENT_CAPACITY, STATUS_INTERNAL,
        STATUS_INVALID_ARGUMENT, STATUS_INVALID_HANDLE, STATUS_INVALID_STATE, STATUS_OK,
        STATUS_PANIC, STEP_REQUEST_BYTES,
    };
    use core::mem::{align_of, size_of};
    use core::sync::atomic::{AtomicU32, AtomicUsize, Ordering};
    use std::collections::BTreeMap;
    use std::fs;
    use std::path::{Path, PathBuf};

    const HEADER_TEXT: &str =
        include_str!("../../../../client/cmd/mornlea-godot-core/include/mornlea_client_core.h");
    const EXPORTS_GO: &str = include_str!("../../../../client/cmd/mornlea-godot-core/exports.go");
    const MODULE_SOURCE: &str = include_str!("client_core.rs");

    /// Collapse every whitespace run to one space so tests can compare
    /// source text and header comments without depending on wrapping.
    fn normalize_whitespace(text: &str) -> String {
        text.split_whitespace().collect::<Vec<_>>().join(" ")
    }

    /// Parse the `//export` symbol names from the producer's `exports.go` in
    /// declaration order. The wrappers are the shared library's sole entry
    /// points, so this list is the producer-side truth the extern block must
    /// mirror.
    fn exported_symbols() -> Vec<String> {
        let mut symbols = Vec::new();
        for raw in EXPORTS_GO.lines() {
            let Some(name) = raw.trim().strip_prefix("//export ") else {
                continue;
            };
            let name = name.trim();
            assert!(
                name.chars()
                    .all(|character| character.is_ascii_alphanumeric() || character == '_'),
                "malformed export name {name}"
            );
            assert!(!symbols.contains(&name.to_string()), "export {name} twice");
            symbols.push(name.to_string());
        }
        assert!(!symbols.is_empty(), "exports were not parsed");
        symbols
    }

    /// Extract the declarations of the module's extern block from its own
    /// source text, whitespace-normalized, in declaration order. Parsing the
    /// module itself ties the pinned name list to the actual declarations:
    /// a renamed, reordered, or re-typed extern item fails the pin instead of
    /// passing vacuously against the constant alone.
    fn extern_declarations() -> Vec<String> {
        let lines: Vec<&str> = MODULE_SOURCE.lines().collect();
        let openers: Vec<usize> = lines
            .iter()
            .enumerate()
            .filter(|(_, line)| line.trim() == "unsafe extern \"C\" {")
            .map(|(index, _)| index)
            .collect();
        assert_eq!(
            openers.len(),
            1,
            "the module must hold exactly one extern block"
        );
        let closer = lines[openers[0] + 1..]
            .iter()
            .position(|line| line.trim() == "}")
            .expect("extern block closer");
        let body: Vec<&str> = lines[openers[0] + 1..openers[0] + 1 + closer]
            .iter()
            .copied()
            .filter(|line| !line.trim().starts_with("//"))
            .collect();
        let joined = body.join("\n");
        let mut declarations = Vec::new();
        for segment in joined.split(';') {
            let normalized = normalize_whitespace(segment);
            if normalized.is_empty() {
                continue;
            }
            // Whitespace collapsing leaves spaces adjacent to the wrapping
            // punctuation rustfmt introduces (after the opening parenthesis,
            // before the closing one, and after a trailing argument comma);
            // drop those so the pin is formatting-independent.
            declarations.push(
                normalized
                    .replace(", )", ")")
                    .replace("( ", "(")
                    .replace(" )", ")"),
            );
        }
        declarations
    }

    /// The name of one parsed extern declaration.
    fn declaration_name(declaration: &str) -> &str {
        let without_prefix = declaration
            .strip_prefix("fn ")
            .expect("declaration starts with fn");
        &without_prefix[..without_prefix.find('(').expect("declaration argument list")]
    }

    /// Extract and normalize the block comment immediately preceding line
    /// `define_index` of the header. The status defines each carry their
    /// one-line meaning in an adjacent comment; the adjacency itself is
    /// part of the contract, so a define without its comment fails here.
    fn header_comment_text(lines: &[&str], define_index: usize) -> String {
        let previous = lines[define_index - 1].trim();
        assert!(
            previous.ends_with("*/"),
            "status define at line {} lacks an adjacent documenting comment",
            define_index + 1
        );
        let mut start = define_index - 1;
        while !lines[start].trim().starts_with("/*") {
            assert!(start > 0, "comment block has no opener");
            start -= 1;
        }
        let block = lines[start..define_index]
            .iter()
            .map(|line| line.trim())
            .collect::<Vec<_>>()
            .join("\n");
        let inner = block
            .strip_prefix("/*")
            .expect("comment opener")
            .strip_suffix("*/")
            .expect("comment closer");
        let text = inner
            .lines()
            .map(|line| line.trim().trim_start_matches('*').trim())
            .collect::<Vec<_>>()
            .join(" ");
        normalize_whitespace(&text)
    }

    /// Header defines that share the `MORNLEA_CLIENT_STATUS_` prefix but are
    /// family vocabulary rather than status words: the status-family contract
    /// version and the status-family header wire size. A new such define must
    /// be listed here consciously, or the status parity check fails.
    const NON_STATUS_PREFIX_DEFINES: [&str; 2] = [
        "MORNLEA_CLIENT_STATUS_VERSION",
        "MORNLEA_CLIENT_STATUS_HEADER_BYTES",
    ];

    /// Parse every status-word define of the header into its value and
    /// one-line comment text. Only the pinned status names, the count
    /// sentinel, and the known non-status prefix defines are accepted, so a
    /// newly added status word fails until this module learns its text; the
    /// sentinel is a count, never a word a producer returns, so it carries no
    /// diagnostic text.
    fn header_status_defines() -> BTreeMap<String, (u32, String)> {
        let lines: Vec<&str> = HEADER_TEXT.lines().collect();
        let mut statuses = BTreeMap::new();
        for (index, raw) in lines.iter().enumerate() {
            let trimmed = raw.trim();
            let Some(rest) = trimmed.strip_prefix("#define MORNLEA_CLIENT_STATUS_") else {
                continue;
            };
            let mut fields = rest.split_whitespace();
            let suffix = fields.next().expect("status define name");
            let name = format!("MORNLEA_CLIENT_STATUS_{suffix}");
            if suffix == "COUNT" || NON_STATUS_PREFIX_DEFINES.contains(&name.as_str()) {
                continue;
            }
            let value = fields.next().expect("status define value");
            let value = value.strip_suffix('u').unwrap_or(value);
            assert!(
                value.len() == 1 || !value.starts_with('0'),
                "ambiguous literal {value}: decimal defines must not carry a leading zero"
            );
            let value: u32 = value.parse().expect("decimal status define value");
            let text = header_comment_text(&lines, index);
            assert!(
                statuses.insert(name.clone(), (value, text)).is_none(),
                "status {name} defined twice"
            );
        }
        assert_eq!(
            statuses.len(),
            STATUS_COUNT as usize,
            "the header must define exactly the pinned status words plus the count sentinel"
        );
        statuses
    }

    /// Blank out line comments, block comments, and string-literal bodies so
    /// a line-by-line token scan sees only code. The scanner fails closed on
    /// constructs it cannot model (every raw-string opener, including the
    /// zero-hash and prefixed forms, plus char literals whose body would
    /// open a string state) so new syntax forces a conscious scanner
    /// extension instead of a silent miss.
    fn strip_comments_and_string_bodies(source: &str) -> String {
        let characters: Vec<char> = source.chars().collect();
        let mut output = String::with_capacity(source.len());
        let mut index = 0;
        let mut in_string = false;
        let mut in_line_comment = false;
        let mut in_block_comment = false;
        while index < characters.len() {
            let character = characters[index];
            index += 1;
            if in_line_comment {
                if character == '\n' {
                    in_line_comment = false;
                    output.push('\n');
                } else {
                    output.push(' ');
                }
            } else if in_block_comment {
                if character == '*' && characters.get(index) == Some(&'/') {
                    index += 1;
                    in_block_comment = false;
                    output.push_str("  ");
                } else if character == '\n' {
                    output.push('\n');
                } else {
                    output.push(' ');
                }
            } else if in_string {
                output.push(' ');
                if character == '\\' {
                    // Keep an escape and its escaped character out of the
                    // scan; neither can close the literal.
                    if index < characters.len() {
                        index += 1;
                        output.push(' ');
                    }
                } else if character == '"' {
                    in_string = false;
                }
            } else if character == 'r' {
                // Every raw-string opener (`r"`, `r#"`, `r##"`, and the
                // `br"`/`cr"` prefixed forms, whose prefix the loop already
                // consumed as code) defeats this scanner: a raw body may
                // contain backslashes the escape rule would swallow and the
                // hashes defeat the closer match, either way hiding live
                // tokens. Fail closed on each opener instead.
                let mut probe = index;
                while characters.get(probe) == Some(&'#') {
                    probe += 1;
                }
                assert!(
                    characters.get(probe) != Some(&'"'),
                    "raw string literals need scanner support"
                );
                output.push(character);
            } else {
                match character {
                    '/' => {
                        if characters.get(index) == Some(&'/') {
                            index += 1;
                            in_line_comment = true;
                            output.push_str("  ");
                        } else if characters.get(index) == Some(&'*') {
                            index += 1;
                            in_block_comment = true;
                            output.push_str("  ");
                        } else {
                            output.push(character);
                        }
                    }
                    '"' => {
                        in_string = true;
                        output.push(' ');
                    }
                    '\'' => {
                        assert!(
                            characters.get(index) != Some(&'"')
                                && characters.get(index) != Some(&'\\'),
                            "char literal with scanner-sensitive body needs scanner support"
                        );
                        output.push(character);
                    }
                    _ => output.push(character),
                }
            }
        }
        output
    }

    /// (header define name, status word, one-line text) for every defined
    /// status, pinned so the text table cannot drift from the header.
    const STATUS_TEXT_PINS: [(&str, u32, &str); 10] = [
        (
            "MORNLEA_CLIENT_STATUS_OK",
            STATUS_OK,
            "The call succeeded and every committed output byte is valid.",
        ),
        (
            "MORNLEA_CLIENT_STATUS_INVALID_ARGUMENT",
            STATUS_INVALID_ARGUMENT,
            "Null, misaligned, overlapping, or oversized pointer and length arguments.",
        ),
        (
            "MORNLEA_CLIENT_STATUS_ABI_MISMATCH",
            STATUS_ABI_MISMATCH,
            "Caller ABI major, magic, or record identity does not match the producer.",
        ),
        (
            "MORNLEA_CLIENT_STATUS_INPUT_REJECTED",
            STATUS_INPUT_REJECTED,
            "Readable buffer whose content violates the family domain; the whole batch is \
            rejected and no producer state is consumed.",
        ),
        (
            "MORNLEA_CLIENT_STATUS_INSUFFICIENT_CAPACITY",
            STATUS_INSUFFICIENT_CAPACITY,
            "Two-phase capacity signal: output buffer too small; the required byte count is \
            reported and nothing is written.",
        ),
        (
            "MORNLEA_CLIENT_STATUS_INVALID_HANDLE",
            STATUS_INVALID_HANDLE,
            "Unknown or wrong-type handle.",
        ),
        (
            "MORNLEA_CLIENT_STATUS_INVALID_STATE",
            STATUS_INVALID_STATE,
            "Correct handle in the wrong lifecycle phase or epoch.",
        ),
        (
            "MORNLEA_CLIENT_STATUS_DISCONNECTED",
            STATUS_DISCONNECTED,
            "The session already reached its terminal disconnect.",
        ),
        (
            "MORNLEA_CLIENT_STATUS_INTERNAL",
            STATUS_INTERNAL,
            "Producer-internal failure without a narrower stable classification.",
        ),
        (
            "MORNLEA_CLIENT_STATUS_PANIC",
            STATUS_PANIC,
            "A recovered panic converted at the ABI boundary; no output is written.",
        ),
    ];

    /// The exact extern declaration of every export, normalized; each entry
    /// mirrors the `//export` wrapper of the same name in `exports.go` (the
    /// producer file holds the signature truth; its line numbers appear in
    /// the module's per-export documentation). Any argument type, argument
    /// order, or return-type drift fails this pin.
    const PINNED_DECLARATIONS: [&str; 13] = [
        "fn mornlea_client_core_create(abi_major: u32, abi_minor: u32, requested_families: \
         *const u64, family_count: u32, out_handle: *mut ClientHandle) -> Status",
        "fn mornlea_client_core_destroy(handle: ClientHandle) -> Status",
        "fn mornlea_client_core_connect_begin(handle: ClientHandle, address: *const u8, \
         address_len: u32) -> Status",
        "fn mornlea_client_core_connect_poll(handle: ClientHandle, out_phase: *mut u32) -> Status",
        "fn mornlea_client_core_disconnect(handle: ClientHandle) -> Status",
        "fn mornlea_client_core_submit_input(handle: ClientHandle, buffer: *const u8, length: \
         u32) -> Status",
        "fn mornlea_client_core_step(handle: ClientHandle, request: *const u8, length: u32) \
         -> Status",
        "fn mornlea_client_core_world_pull(handle: ClientHandle, out: *mut u8, capacity: u32, \
         required_out: *mut u32) -> Status",
        "fn mornlea_client_core_frame_pull(handle: ClientHandle, out: *mut u8, capacity: u32, \
         required_out: *mut u32) -> Status",
        "fn mornlea_client_core_environment_pull(handle: ClientHandle, out: *mut u8, capacity: \
         u32, required_out: *mut u32) -> Status",
        "fn mornlea_client_core_status_pull(handle: ClientHandle, out: *mut u8, capacity: u32, \
         required_out: *mut u32) -> Status",
        "fn mornlea_client_core_status_identity(handle: ClientHandle, out: *mut u8, capacity: \
         u32, required_out: *mut u32) -> Status",
        "fn mornlea_client_core_abi_version() -> u64",
    ];

    /// The exact extern declarations of the dynamic-loader entry points the
    /// production table resolves the producer library with, normalized; they
    /// live in the same extern block as the export mirror.
    const PINNED_LOADER_DECLARATIONS: [&str; 3] = [
        "fn dlopen(path: *const c_char, mode: c_int) -> *mut c_void",
        "fn dlsym(handle: *mut c_void, symbol: *const c_char) -> *mut c_void",
        "fn dladdr(address: *const c_void, info: *mut DlInfo) -> c_int",
    ];

    /// The producer vtable field for each export symbol, in
    /// [`EXPORT_SYMBOLS`] order; the signature pin derives each field's
    /// expected type from the matching pinned declaration.
    const VTABLE_FIELD_SYMBOLS: [(&str, &str); 13] = [
        ("create", "mornlea_client_core_create"),
        ("destroy", "mornlea_client_core_destroy"),
        ("connect_begin", "mornlea_client_core_connect_begin"),
        ("connect_poll", "mornlea_client_core_connect_poll"),
        ("disconnect", "mornlea_client_core_disconnect"),
        ("submit_input", "mornlea_client_core_submit_input"),
        ("step", "mornlea_client_core_step"),
        ("world_pull", "mornlea_client_core_world_pull"),
        ("frame_pull", "mornlea_client_core_frame_pull"),
        ("environment_pull", "mornlea_client_core_environment_pull"),
        ("status_pull", "mornlea_client_core_status_pull"),
        ("status_identity", "mornlea_client_core_status_identity"),
        ("abi_version", "mornlea_client_core_abi_version"),
    ];

    #[test]
    fn client_core_export_symbols_match_the_go_producer() {
        let exported = exported_symbols();
        // Both directions and in order: a renamed, added, dropped, or
        // reordered producer export fails until this mirror is consciously
        // re-pinned with the same review as the header change.
        assert_eq!(EXPORT_SYMBOLS.len(), 13);
        assert_eq!(exported, EXPORT_SYMBOLS.to_vec());
    }

    #[test]
    fn client_core_extern_block_pins_every_declaration() {
        let declarations = extern_declarations();
        assert_eq!(
            declarations.len(),
            PINNED_DECLARATIONS.len() + PINNED_LOADER_DECLARATIONS.len(),
            "the extern block holds the export mirror plus the loader entry points"
        );
        for (index, (declaration, pinned)) in
            declarations.iter().zip(PINNED_DECLARATIONS).enumerate()
        {
            assert_eq!(
                declaration,
                &normalize_whitespace(pinned),
                "extern declaration {index}"
            );
        }
        // The export names equal the pinned symbol list, which the symbol
        // parity test separately proves equal to the producer's exports.
        let names: Vec<&str> = declarations[..PINNED_DECLARATIONS.len()]
            .iter()
            .map(|item| declaration_name(item))
            .collect();
        assert_eq!(names, EXPORT_SYMBOLS.to_vec());
        for (declaration, pinned) in declarations[PINNED_DECLARATIONS.len()..]
            .iter()
            .zip(PINNED_LOADER_DECLARATIONS)
        {
            assert_eq!(declaration, &normalize_whitespace(pinned));
        }
    }

    /// The vtable the production table resolves at runtime must carry exactly
    /// the declared signatures, because the extern block itself is never
    /// linked (the producer is opened beside the extension at runtime). The
    /// expected type text is derived from the pinned declarations, so a
    /// signature change on either side fails this pin.
    #[test]
    fn client_core_vtable_pins_every_resolved_signature() {
        let body = vtable_struct_body();
        let names = VTABLE_FIELD_SYMBOLS.map(|(field, _)| field);
        for (index, field) in names.iter().enumerate() {
            let opener = format!("{field}: ");
            let start = body
                .find(&opener)
                .unwrap_or_else(|| panic!("vtable field {field} is missing"))
                + opener.len();
            let end = names
                .get(index + 1)
                .and_then(|next| body.find(&format!("{next}: ")))
                .unwrap_or(body.len());
            let declared = normalize_whitespace(&body[start..end]);
            let declared = declared.trim_end_matches(',');
            let expected = vtable_type_for_signature(PINNED_DECLARATIONS[index]);
            assert_eq!(declared, expected, "vtable field {field}");
        }
        // The field-to-symbol mapping itself projects onto the pinned export
        // order, so the loader resolves every field from the pinned list.
        let symbols: Vec<&str> = VTABLE_FIELD_SYMBOLS
            .iter()
            .map(|(_, symbol)| *symbol)
            .collect();
        assert_eq!(symbols, EXPORT_SYMBOLS.to_vec());
    }

    /// The source text of the producer vtable struct, whose fields the
    /// signature pin reads in declaration order.
    fn vtable_struct_body() -> String {
        let lines: Vec<&str> = MODULE_SOURCE.lines().collect();
        let opener = lines
            .iter()
            .position(|line| line.trim() == "struct ProducerVTable {")
            .expect("vtable struct opener");
        let closer = lines[opener + 1..]
            .iter()
            .position(|line| line.trim() == "}")
            .expect("vtable struct closer");
        lines[opener + 1..opener + 1 + closer].join("\n")
    }

    /// Derive the vtable field type for one pinned declaration: the same
    /// signature with argument names dropped and the `unsafe extern "C"`
    /// calling convention prefixed.
    fn vtable_type_for_signature(declaration: &str) -> String {
        let signature = declaration
            .strip_prefix("fn ")
            .expect("declaration starts with fn");
        let arguments_open = signature.find('(').expect("argument list");
        let arguments_close = signature.rfind(')').expect("argument list closer");
        let argument_types: Vec<&str> = signature[arguments_open + 1..arguments_close]
            .split(", ")
            .filter(|argument| !argument.is_empty())
            .map(|argument| argument.split_once(": ").expect("typed argument").1)
            .collect();
        let return_type = signature[arguments_close + 1..]
            .trim()
            .trim_start_matches("-> ")
            .trim();
        format!(
            "unsafe extern \"C\" fn({}) -> {}",
            argument_types.join(", "),
            return_type
        )
    }

    #[test]
    fn client_core_status_text_pins_every_defined_status() {
        for (name, word, text) in STATUS_TEXT_PINS {
            assert_eq!(status_text(word), Some(text), "status {name}");
        }
        // Contiguity: every word below the count sentinel decodes, the
        // sentinel and every word beyond it fail closed with no text.
        for word in 0..STATUS_COUNT {
            assert!(
                status_text(word).is_some(),
                "defined status word {word} has no text"
            );
            assert!(Status::from_word(word).is_some(), "word {word}");
        }
        for word in [STATUS_COUNT, STATUS_COUNT + 1, u32::MAX] {
            assert_eq!(status_text(word), None, "undefined status word {word}");
            assert!(Status::from_word(word).is_none(), "word {word}");
        }
        let mut words: Vec<u32> = STATUS_TEXT_PINS.iter().map(|pin| pin.1).collect();
        words.sort();
        words.dedup();
        assert_eq!(words.len(), STATUS_TEXT_PINS.len());
    }

    #[test]
    fn client_core_status_text_matches_the_header_documentation() {
        let defines = header_status_defines();
        let pinned: BTreeMap<&str, (u32, &str)> = STATUS_TEXT_PINS
            .iter()
            .map(|(name, word, text)| (*name, (*word, *text)))
            .collect();
        // Both directions: a status the header gains without this table
        // fails, and a table entry the header lost fails, so the mapping
        // cannot drift on either side.
        for (name, (word, text)) in &defines {
            let Some((pinned_word, pinned_text)) = pinned.get(name.as_str()) else {
                panic!("header status {name} has no pinned text");
            };
            assert_eq!(word, pinned_word, "status {name} value");
            assert_eq!(
                status_text(*word),
                Some(normalize_whitespace(pinned_text).as_str()),
                "status {name} text"
            );
            assert_eq!(
                &normalize_whitespace(text),
                &normalize_whitespace(pinned_text)
            );
        }
        for name in pinned.keys() {
            assert!(
                defines.contains_key(*name),
                "pinned status {name} is not in the header"
            );
        }
    }

    #[test]
    fn client_core_type_widths_and_abi_layouts_stay_pinned() {
        assert_eq!((size_of::<Status>(), align_of::<Status>()), (4, 4));
        assert_eq!(
            (size_of::<ClientHandle>(), align_of::<ClientHandle>()),
            (8, 8)
        );
        // The byte-buffer exports operate on the wire layouts mirrored in
        // `abi`; reusing those structs (no new mirrors) keeps the extern
        // disciplines and the layout pins on one definition.
        assert_eq!(size_of::<abi::StepRequest>(), STEP_REQUEST_BYTES);
        assert_eq!(size_of::<abi::IdentityHeader>(), IDENTITY_HEADER_BYTES);
        assert_eq!(align_of::<abi::StepRequest>(), abi::ABI_ALIGNMENT);
        assert_eq!(align_of::<abi::IdentityHeader>() * 2, abi::ABI_ALIGNMENT);
    }

    #[test]
    fn client_core_status_and_handle_round_trip_their_words() {
        for (_, word, _) in STATUS_TEXT_PINS {
            let status = Status::from_word(word).expect("defined status word");
            assert_eq!(status.as_word(), word);
            assert_eq!(status.is_ok(), word == STATUS_OK);
        }
        assert!(Status::from_word(STATUS_COUNT).is_none());
        let handle = ClientHandle::from_word(u64::MAX);
        assert_eq!(handle.as_word(), u64::MAX);
        assert_eq!(ClientHandle::from_word(0).as_word(), 0);
    }

    /// Collect every `.rs` file under `directory`, recursing into
    /// subdirectories and failing closed on any read error so the unsafe
    /// confinement scan cannot silently skip a tree it could not open.
    fn rust_sources_under(directory: &Path, sink: &mut Vec<PathBuf>) {
        for entry in fs::read_dir(directory).expect("read the source directory") {
            let path = entry.expect("source directory entry").path();
            if path.is_dir() {
                rust_sources_under(&path, sink);
                continue;
            }
            if path.extension().is_some_and(|extension| extension == "rs") {
                sink.push(path);
            }
        }
    }

    /// All unsafe code of the crate lives in this module. The crate root's
    /// `unsafe impl ExtensionLibrary` is the single pre-existing exception
    /// (Godot's extension trait is unsafe by contract), pinned exactly; any
    /// other `unsafe` token anywhere under `src/` fails this scan, including
    /// files added later in nested subdirectories, because the whole tree is
    /// enumerated at test time. This module itself is the sanctioned home and
    /// is exempt from the token scan; doc comments and string bodies of the
    /// other files are stripped first so prose cannot hide or fake an
    /// occurrence.
    #[test]
    fn client_core_unsafe_is_confined_to_the_ffi_module() {
        let source_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut sources = Vec::new();
        rust_sources_under(&source_dir, &mut sources);
        sources.sort();
        assert!(
            sources.iter().any(|path| path
                .file_name()
                .is_some_and(|name| name == "client_core.rs")),
            "the FFI module source is missing from the scan"
        );
        for path in &sources {
            let name = path
                .file_name()
                .expect("source file name")
                .to_str()
                .expect("utf-8 source file name");
            if name == "client_core.rs" {
                // The FFI module is the one sanctioned home of unsafe code.
                continue;
            }
            let source = fs::read_to_string(path).expect("read the source file");
            let stripped = strip_comments_and_string_bodies(&source);
            for (index, line) in stripped.lines().enumerate() {
                if !line.split_whitespace().any(|token| token == "unsafe") {
                    continue;
                }
                let allowed = name == "lib.rs"
                    && line.trim() == "unsafe impl ExtensionLibrary for MornleaGodotExtension {";
                assert!(
                    allowed,
                    "unsafe code outside the client_core FFI module: {name} line {}: {}",
                    index + 1,
                    line.trim()
                );
            }
        }
    }

    /// Raw `out` pointer and `capacity` of the most recent pull-shaped probe
    /// call, raw input pointer and length of the most recent input-shaped
    /// probe call (connect begin / submit input), and raw request pointer and
    /// count of the most recent create probe — all recorded by the scripted
    /// vtable below so the pointer discipline of the production call paths
    /// can be pinned without loading the producer library.
    static PROBE_OUT: AtomicUsize = AtomicUsize::new(usize::MAX);
    static PROBE_CAPACITY: AtomicU32 = AtomicU32::new(u32::MAX);
    static PROBE_INPUT_PTR: AtomicUsize = AtomicUsize::new(usize::MAX);
    static PROBE_INPUT_LEN: AtomicU32 = AtomicU32::new(u32::MAX);
    static PROBE_CREATE_PTR: AtomicUsize = AtomicUsize::new(usize::MAX);
    static PROBE_CREATE_COUNT: AtomicU32 = AtomicU32::new(u32::MAX);

    /// The fixed requirement the pull probe reports, so the mapped outcome is
    /// deterministic.
    const PROBE_REQUIRED: u32 = 80;

    /// One pull-shaped probe with the resolved vtable signature: records the
    /// raw `out` and `capacity` it was called with and answers the two-phase
    /// capacity signal.
    unsafe extern "C" fn probe_pull(
        _handle: ClientHandle,
        out: *mut u8,
        capacity: u32,
        required: *mut u32,
    ) -> Status {
        PROBE_OUT.store(out.addr(), Ordering::SeqCst);
        PROBE_CAPACITY.store(capacity, Ordering::SeqCst);
        // Safety: the production pull path always passes a live `required`
        // out-parameter (`&mut required` in `pull_via`).
        unsafe { *required = PROBE_REQUIRED };
        Status::from_word(abi::STATUS_INSUFFICIENT_CAPACITY).expect("defined status word")
    }

    /// The internal status word, for the vtable stubs this test never drives.
    fn internal_status() -> Status {
        Status::from_word(abi::STATUS_INTERNAL).expect("defined status word")
    }

    unsafe extern "C" fn probe_abi_version() -> u64 {
        (u64::from(abi::ABI_MAJOR) << 32) | u64::from(abi::ABI_MINOR)
    }

    /// One create-shaped probe: records the raw family-request pointer and
    /// count it was called with. The pin only drives the empty-request path,
    /// so the outcome is the internal status word.
    unsafe extern "C" fn probe_create(
        _abi_major: u32,
        _abi_minor: u32,
        requested_families: *const u64,
        family_count: u32,
        _out_handle: *mut ClientHandle,
    ) -> Status {
        PROBE_CREATE_PTR.store(requested_families.addr(), Ordering::SeqCst);
        PROBE_CREATE_COUNT.store(family_count, Ordering::SeqCst);
        internal_status()
    }

    unsafe extern "C" fn probe_unused_handle(_handle: ClientHandle) -> Status {
        internal_status()
    }

    /// One input-shaped probe (the connect-begin and submit-input signature):
    /// records the raw input pointer and length it was called with.
    unsafe extern "C" fn probe_input(
        _handle: ClientHandle,
        buffer: *const u8,
        length: u32,
    ) -> Status {
        PROBE_INPUT_PTR.store(buffer.addr(), Ordering::SeqCst);
        PROBE_INPUT_LEN.store(length, Ordering::SeqCst);
        internal_status()
    }

    unsafe extern "C" fn probe_unused_phase(_handle: ClientHandle, _out_phase: *mut u32) -> Status {
        internal_status()
    }

    /// The step stub: the step record is a fixed-size slice, so it has no
    /// empty-input case to pin.
    unsafe extern "C" fn probe_unused_step(
        _handle: ClientHandle,
        _request: *const u8,
        _length: u32,
    ) -> Status {
        internal_status()
    }

    /// A resolved-signature vtable whose pull and input slots probe; the
    /// remaining slots are unreachable stubs because the pinned paths only
    /// pull and submit.
    fn probe_vtable() -> ProducerVTable {
        ProducerVTable {
            environment_pull: probe_pull,
            create: probe_create,
            destroy: probe_unused_handle,
            connect_begin: probe_input,
            connect_poll: probe_unused_phase,
            disconnect: probe_unused_handle,
            submit_input: probe_input,
            step: probe_unused_step,
            world_pull: probe_pull,
            frame_pull: probe_pull,
            status_pull: probe_pull,
            status_identity: probe_pull,
            abi_version: probe_abi_version,
        }
    }

    /// The zero-capacity pull query must pass a null `out` pointer, never a
    /// non-null placeholder such as `NonNull::<u8>::dangling()` (address 1).
    /// All four pull families share `pull_via`, so each is driven through the
    /// probe. The producer's cgo boundary stores `out` in Go pointer-typed
    /// stack variables while the call runs, and the Go runtime's stack scan
    /// aborts the whole process during garbage collection when a pointer-typed
    /// slot holds any address below the runtime's minimum legal pointer (4096)
    /// — even a buffer the call never dereferences. Null is the one
    /// placeholder the boundary always tolerates, and every pull export
    /// accepts it at capacity zero. The positive-capacity half pins that a
    /// real buffer address is still passed when content can be written.
    #[test]
    fn producer_pulls_pass_null_out_for_zero_capacity_and_buffer_for_positive() {
        let core = ProducerCore {
            vtable: probe_vtable(),
        };
        let handle = ClientHandle::from_word(8);
        type PullFn = fn(&ProducerCore, ClientHandle, &mut [u8]) -> PullOutcome;
        let pulls: [PullFn; 4] = [
            |core, handle, buffer| core.world_pull(handle, buffer),
            |core, handle, buffer| core.frame_pull(handle, buffer),
            |core, handle, buffer| core.status_pull(handle, buffer),
            |core, handle, buffer| core.identity_pull(handle, buffer),
        ];
        let mut empty: [u8; 0] = [];
        for pull in pulls {
            let outcome = pull(&core, handle, &mut empty);
            assert_eq!(PROBE_OUT.load(Ordering::SeqCst), 0);
            assert_eq!(PROBE_CAPACITY.load(Ordering::SeqCst), 0);
            assert_eq!(
                outcome,
                PullOutcome::Capacity {
                    required: PROBE_REQUIRED
                }
            );
        }

        let mut buffer = [0u8; 80];
        let buffer_address = buffer.as_mut_ptr().addr();
        for pull in pulls {
            let outcome = pull(&core, handle, &mut buffer);
            assert_eq!(PROBE_OUT.load(Ordering::SeqCst), buffer_address);
            assert_eq!(PROBE_CAPACITY.load(Ordering::SeqCst), 80);
            assert_eq!(
                outcome,
                PullOutcome::Capacity {
                    required: PROBE_REQUIRED
                }
            );
        }
    }

    /// The input paths share the pull path's Go stack-scan constraint: an
    /// empty connect address, input batch, or family-request slice owns no
    /// allocation, so its pointer accessor yields a dangling aligned address
    /// (8), which the producer's Go runtime rejects in pointer-typed stack
    /// slots below its minimum legal pointer (4096) — aborting the process
    /// during garbage collection even though the producer rejects a zero
    /// length before reading any pointer. Each empty input must therefore
    /// pass null; a non-empty batch still passes a real aligned pointer.
    #[test]
    fn producer_inputs_pass_null_for_empty_slices() {
        let core = ProducerCore {
            vtable: probe_vtable(),
        };
        let handle = ClientHandle::from_word(8);

        core.connect_begin(handle, &[]);
        assert_eq!(PROBE_INPUT_PTR.load(Ordering::SeqCst), 0);
        assert_eq!(PROBE_INPUT_LEN.load(Ordering::SeqCst), 0);

        core.submit_input(handle, &[]);
        assert_eq!(PROBE_INPUT_PTR.load(Ordering::SeqCst), 0);
        assert_eq!(PROBE_INPUT_LEN.load(Ordering::SeqCst), 0);

        let outcome = core.create_session(abi::ABI_MAJOR, abi::ABI_MINOR, &[]);
        assert_eq!(PROBE_CREATE_PTR.load(Ordering::SeqCst), 0);
        assert_eq!(PROBE_CREATE_COUNT.load(Ordering::SeqCst), 0);
        assert_eq!(outcome, Err(abi::STATUS_INTERNAL));

        let batch = [1u8, 2, 3, 4, 5, 6, 7, 8];
        core.submit_input(handle, &batch);
        let pointer = PROBE_INPUT_PTR.load(Ordering::SeqCst);
        assert_ne!(pointer, 0);
        assert_eq!(pointer % size_of::<u64>(), 0);
        assert_eq!(PROBE_INPUT_LEN.load(Ordering::SeqCst), 8);
    }
}
