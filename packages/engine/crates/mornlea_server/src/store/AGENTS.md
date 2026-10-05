# Authoritative persistence workers

This directory owns off-tick save queues, durable acknowledgments and the real
filesystem provider boundary. Save bytes and schema validation remain in
`mornlea_storage`; the tick never performs file I/O.

## Admission query for the automatic source acquisition caller

The background mailbox exposes one nonblocking load-capacity query:
`source_chunk_slots` refuses an inline owner before any drive, keeps the
original frozen/closed phase refusals, and otherwise forwards the free
chunk-load ledger slots (eight minus queued, started and unconsumed
results). The automatic source acquisition caller issues it after tick and
budget validation but before driving any provider, and treats zero slots as
a retain-queue answer rather than an error. Save occupancy is not load
capacity; the scheduler only forwards the query.

## Map and ownership

- `mailbox.rs` and `scheduler.rs` retain owned snapshots through queue, worker,
  completion and retry. A later error never erases earlier durable keys.
- Private `background.rs` transfers one backend, world lease and codec to one
  OS thread through a bounded immutable save handoff. Tick admission and polls
  use only nonblocking channel operations; off-tick lifecycle waits retain one
  result across timeout, and successful close joins before marking closed.
  Inline construction remains available for deterministic non-Send doubles.
  Immutable chunk views reserve the existing compression maximum using only
  checked capture identity. The backend owner normalizes its cloned request
  before materialization, encoding and disk writes; completion returns the
  original view without expanding it. Partial errors and retries retain that
  same capture. Direct `DiskStore` writes normalize before semantic duplicate
  validation and echo original targets. Standalone files reject chunk families.
- `io.rs` owns per-request monotonic cancellation, codec/I/O error mapping,
  complete-write loops and the native fallible-close adapters. Its only unsafe
  blocks consume a uniquely owned File descriptor/handle once. Do not extend
  that exception to callers or add environment-based fault bypasses.
- `region_io.rs` owns one region's bank/payload commits and compaction;
  `atomic_file.rs` owns standalone same-directory replacement.
- `disk.rs` joins the standalone/region providers with bounded LRU ownership,
  whole-request validation and exact partial acknowledgments. An evicted file
  close consumes its descriptor even on failure: remove that cache entry while
  retaining the world lease, so retries can reopen it or finish shutdown.
- `lease.rs` acquires the native world.lock before metadata or save access;
  only successful complete close releases it. `recovery.rs` streams named
  backups with canonical source identity, exact Go exclusion patterns and
  file/directory durability. Symlinks are rejected before exclusions and other
  nonregular entries afterward. Matching backup retries still sync the parent.

## Failure and lifecycle

Decoded loads preserve player rewrite flags, companion source schemas and
chunk recovery facts in LoadedValue. NotFound alone is absence; corruption,
future versions and arbitrary I/O errors cannot become a blank world.
Cancellation checks precede publication. Once bank write or rename starts,
finish the durability sequence without cancellation. A sync/close error is not
an acknowledgment. Uncertain bank failures require a refreshed bank view before
allocation; never reuse the old cached allocation map after a possible commit.
Injected hooks surround actual filesystem operations and are available only
through explicit constructors used by tests; native constructors use real I/O.

## Validation

Run `rustup run 1.97.1 cargo test --manifest-path packages/engine/Cargo.toml
-p mornlea_server --test persistence_failure --locked` from the repository root,
then crate clippy/fmt. Real provider acceptance additionally requires temporary
filesystem and subprocess crash cases from the active OpenSpec packet. A
mailbox or contract double does not accept on-disk persistence.

`region_io.rs` owns one real region descriptor and its selected dual-bank view.
A failed bank publication or reopen invalidates cached extents before reuse;
only a successful durability barrier permits revision acknowledgment. The
provider reports per-key commits separately from later errors. Compaction and
same-owner retries retain the exclusive owner and use fallible file/directory
close. Cross-region aggregation and the world lease belong to the disk backend.

`atomic_file.rs` owns the five standalone save families under an existing world
root. It preserves codec migration facts on load, serializes revision checks
and real replacement, and acknowledges only after temp/file/parent durability
and fallible close. Metadata's sequence is process-local: an uncertain published
candidate still prevents a stale overwrite, but cannot be acknowledged before
its retry barrier. The world owner creates and leases the root; this provider
establishes players and makes that directory entry durable before returning.

Private `loads.rs` retains at most sixteen player and eight chunk requests,
including unconsumed results and started cancellations. Player and chunk load
ports use the existing background command channel and sole backend thread;
validation, decoding and PreparedChunk construction stay on that owner. Inline
non-Send doubles execute loads only in `drive_workers`. Only Load/NotFound is
absence. First close freezes new load admission; retained loads must be consumed
or cancelled and drained before backend close. Save occupancy stays independent.

`AutosaveScheduler` delegates all player and chunk load operations directly to
its private mailbox. Runtime transport and acquisition borrow that existing
owner between tick polls; they receive no mutable store accessor or alternate
backend. A cancelled started load must still drain its real reply before the
runtime closes and joins the scheduler's sole owner.
