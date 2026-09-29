# Authoritative persistence workers

This directory owns off-tick save queues, durable acknowledgments and the real
filesystem provider boundary. Save bytes and schema validation remain in
`mornlea_storage`; the tick never performs file I/O.

## Map and ownership

- `mailbox.rs` and `scheduler.rs` retain owned snapshots through queue, worker,
  completion and retry. A later error never erases earlier durable keys.
- `io.rs` owns per-request monotonic cancellation, codec/I/O error mapping,
  complete-write loops and the native fallible-close adapters. Its only unsafe
  blocks consume a uniquely owned File descriptor/handle once. Do not extend
  that exception to callers or add environment-based fault bypasses.
- `region_io.rs` owns one region's bank/payload commits and compaction;
  `atomic_file.rs` owns standalone same-directory replacement.
- `disk.rs`, `lease.rs` and `recovery.rs` are the serial join for exclusive world
  ownership, decoded load/recovery and backup. Planned modules are not runtime
  acceptance until their OpenSpec nodes pass.

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
