# Persistence provider contract completion

This packet refines the existing region and standalone provider packets. It
implements the approved S0/S3 architecture; it does not change save formats.
The controller owns decisions and the serial contract landing. Tasks.md remains
the only status source. Baseline: `4cedf529`.

## Contract node 3.4c0

Files: `S/src/core/contracts.rs`, new `S/src/store/io.rs`, `S/src/store/mod.rs`,
`S/tests/persistence_failure/io.rs`, its test entry, the two existing mailbox
and scheduler test doubles, `S/Cargo.toml`, engine `Cargo.lock`, S/AGENTS.md and new S/src/store/AGENTS.md.
All other providers are read-only. No source hashes in the migration corpus name
these server files; no fixture asset is changed. The controller integrates and
rolls back this node before either dependent provider starts.

Add `LoadedValue::{Chunk(RecoveredChunk),Player(StoredPlayer),
Companions(StoredCompanions),Hostiles(HostileMobs),Passives(PassiveMobs),
Metadata(Metadata)}`. It preserves F1 decoded migration flags and source schema;
never convert a decoded legacy player/companion directly to a save and discard
those facts. `DiskBackend::load(&mut self, SaveKey) -> Result<LoadedValue,
ServerError>` replaces its save-value return. Only the mailbox/scheduler doubles
currently implement that unused load port; update their return types. Provider
load never rewrites or invents missing-file defaults.

Add `StorageFailure::{Corrupt,FutureVersion,OutputTooSmall}` (Copy/Eq) and
`ServerError::Storage { family: &'static str, kind: StorageFailure }`. Shared
`io::storage_error(family, StorageError)` maps the three variants without parsing
human text or inventing a numeric future version. `io::io_error(operation,
io::Error)` preserves `ErrorKind`; missing slot/file is exactly
`Io { operation: Load, kind: NotFound }`, never corruption or an empty world.

Add `DiskWriteOutcome { committed: Vec<(SaveKey,u64)>, error:
Option<ServerError> }` with no all-or-nothing Result wrapper. Existing committed
revisions remain representable beside a later region/compaction error. The
single-region provider cannot claim cross-region atomicity.

In `store/io.rs`, add cloneable `IoCancellation` over `Arc<AtomicBool>` with
`new()/default`, `cancel()`, `check()->Result<(),ServerError>`. Cancellation is
monotonic and shared by clones. Providers check at operation entry and after
payload/temp sync immediately before bank publication/rename. Once publication
starts, finish its durability sequence and do not turn a committed result into
cancellation. A fresh save request receives a fresh token; this is not an
owner-wide permanent cancelled flag.

`IoPhase::{Before,After}` and public hidden `DiskIo: Send` provide
`boundary(&mut self, IoFaultPoint, IoPhase)->io::Result<()>` (default success),
`write(&mut self,&mut File,&[u8])->io::Result<usize>` (default actual Write), and
`close(&mut self,File)->io::Result<()>` (default fallible native close).
`NativeDiskIo` uses these defaults. These are fault-observation hooks around
provider-owned real filesystem operations, not a replacement filesystem.
Before error skips the operation; after error reports failure after the actual
operation happened. Subprocess crash hooks may terminate at these same edges.
Hidden injected constructors compile in normal builds for integration tests;
there are no environment switches or production fallback paths.

Shared `write_all(io:&mut dyn DiskIo,file:&mut File,bytes:&[u8])` retries
Interrupted, consumes each partial prefix, returns WriteZero on zero progress,
and propagates every other error. Shared `close_file(File)` consumes exactly
one owned descriptor/handle and reports the native close result. Its tiny Unix
and Windows implementations are the only new scoped unsafe allowance; the
crate's deny remains elsewhere. Pin already-locked libc=0.2.189 on Unix and
windows-sys=0.61.2 with Win32_Foundation on Windows. No dependency version is
upgraded. Sync before close remains the provider's responsibility.

Tests `io::cancel_clone`, `io::error_classes`, `io::partial_interrupted_write`,
`io::zero_write_refuses`,
`io::decoded_migration_and_partial_commit` use actual temporary files plus a
short-write injector and consumer double. Compiling initial no-op cancellation
and write helper must fail assertions; implement minimum checked behavior and
run all five cases, full server crate, fmt/clippy, strict OpenSpec and diff.
The decoded test carries legacy player needs_rewrite and companion source_schema
through LoadedValue unchanged and keeps committed A7 alongside a WriteBank error.
Commit `fix(server): complete persistence provider io contracts`.

## Region provider refinement

Consume the accepted contract SHA recorded after 3.4c0. Existing exclusive files
and Go algorithms remain in plan04. Public API:
`RegionIo::open(&Path, storage::RegionKey)->Result<Self,ServerError>`;
`with_io(&Path,storage::RegionKey,Box<dyn DiskIo>)->Result<Self,ServerError>`;
`load(&mut self,ChunkKey)->Result<RecoveredChunk,ServerError>`;
`save(&mut self,&[storage::ChunkSave],&IoCancellation)->DiskWriteOutcome`;
`compact(&mut self,&IoCancellation)->Result<(),ServerError>`;
`sync(&mut self)->Result<(),ServerError>`; `close(&mut self)->Result<(),ServerError>`.
Region creation uses TempWrite/TempSync/Rename/DirectorySync boundaries; save
uses PayloadWrite/PayloadSync/BankWrite/BankSync. At each boundary invoke Before,
perform actual operation, then After. Newly written keys are acknowledged only
after BankSync and its After hook both succeed. A reported error after that
physical sync may leave a complete new bank readable but returns no new ack.
Compaction failure after a previously acknowledged commit retains those keys.
Known higher durable revisions may be returned with an error for another key.

After an uncertain bank write/sync failure, never continue allocation from the
old cached bank. Invalidate the cached bank view and reread/validate both banks
before the next load/save. Load stays byte-preserving. Before a subsequent save
acknowledges a revision discovered through uncertain IO, perform a successful
file sync; failure preserves uncertainty and returns no newly claimed ack.
A failed compaction/close may require reopening the canonical path before retry;
never use a consumed descriptor. Keep the exclusive region owner in place.

`partial_regions` instantiates two RegionIo owners and explicitly aggregates the
first success and second failure, proving provider reports only. Actual
DiskBackend aggregation remains the later disk integration node. Include an
uncertain bank-failure/retry test so the retry cannot overwrite the newer bank's
referenced payload. Root lease and backup are excluded from this node.

## Standalone provider refinement

Consume the same accepted contract. API:
`AtomicFiles::open(root:&Path)->Result<Self,ServerError>`;
`with_io(root:&Path,Box<dyn DiskIo>)->Result<Self,ServerError>`;
`load(&mut self,SaveKey)->Result<LoadedValue,ServerError>`;
`save(&mut self,&OwnedSnapshot,&IoCancellation)->Result<u64,ServerError>`;
`sync(&mut self)->Result<(),ServerError>`; `close(&mut self)->Result<(),ServerError>`.
The root is exclusively owned by the caller; open establishes the players
subdirectory and propagates errors. Reject Chunk keys before any filesystem
mutation. Each provider instance serializes operations through &mut self.
Existing path mappings, codecs, revision policy and failure table in plan04
remain exact. Decode to LoadedValue directly, retaining player needs_rewrite
and companion source_schema; no speculative migration save on load.

Invoke Before/After around TempWrite, TempSync, Rename and DirectorySync. The
injected write and close methods also execute; a short/zero write or close error
must not be hidden. Pre-rename errors clean the temp and preserve canonical;
post-rename errors leave the complete new canonical but return failure, never
an acknowledged revision. Future/corrupt/other read failures cannot create or
replace a canonical file. Metadata's process-local sequence is acknowledged
only after the complete replacement succeeds. Already durable identical
revision returns its current revision without unnecessary replacement; a prior
post-rename failure must retry the durability barrier before claiming success.

Controller clarifications: standalone lower/equal-conflicting revision maps to
`InvalidInput { field: "revision" }`; corrupt/future loads retain their Storage
class. Region storage keys whose dimension cannot map to the domain Dimension
are refused before save with `InvalidInput { field: "dimension" }`; open may
still validate storage-supported keys, but cannot fabricate a domain SaveKey.

Standalone review completion: root already exists and belongs to the world
owner. Open establishes players and always syncs/fallibly closes root before
returning, including retry when a prior failed open already made players.
Metadata retains a process-local latest physically published candidate
(sequence, canonical bytes, acknowledged flag), installed immediately after
rename and before its After hook. Lower sequence/equal conflicting content
refuses; equal identical retry finishes durability before ack. Pre-rename
failure retains the preceding candidate; post-rename failure retains the new
candidate without acknowledgment. No sequence enters metadata format6.
