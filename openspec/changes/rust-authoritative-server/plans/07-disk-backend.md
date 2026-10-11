# Exclusive disk backend and named backup

This refines node 3.5 against accepted persistence contracts `4eaae498`, region
provider `2acadd8b` and standalone provider `091a0b73`. It changes no schema.
The controller owns integration/rollback. Tasks.md is the only status source.

## Ownership and interfaces

The isolated implementer owns `S/src/store/{lease,disk,recovery}.rs`,
`S/tests/persistence_failure/recovery.rs`, and the small `RegionIo::should_compact`
addition in `S/src/store/region_io.rs`. S is
`packages/engine/crates/mornlea_server`. It also owns adding exact existing-lock
`serde_json = "=1.0.151"` to S/Cargo.toml and updating only that crate's dependency
list in engine/Cargo.lock. All core contracts, other providers, guides and plans
are read-only. No hashed corpus input or generated asset changes. No new guide:
these units inherit store/AGENTS.md; the controller updates its ownership text.

Public concrete interfaces, all returning ServerError on failure:

```rust
WorldLease::acquire(root: &Path) -> Result<WorldLease, ServerError>;
WorldLease::release(&mut self) -> Result<(), ServerError>;
pub struct DiskOptions { pub create: Metadata, pub region_handle_cap: usize }
DiskStore::open(root: &Path, options: DiskOptions) -> Result<Self, ServerError>;
DiskStore::with_io(root: &Path, options: DiskOptions,
    factory: Box<dyn Fn() -> Box<dyn DiskIo> + Send>) -> Result<Self, ServerError>;
DiskStore::metadata(&self) -> &Metadata;
DiskStore::initial_metadata_sequence(&self) -> u64; // always 2
DiskStore::open_region_count(&self) -> usize;
DiskStore::backup(&mut self, destination: &Path,
    cancel: &IoCancellation) -> Result<(), ServerError>;
RegionIo::should_compact(&mut self, min_waste: u64, ratio: f64)
    -> Result<bool, ServerError>;
```

DiskStore implements the accepted DiskBackend exactly. The factory is an
explicit test seam making separate DiskIo instances sharing test-owned failure
state; the native constructor uses NativeDiskIo. It creates no I/O thread here:
the mailbox/reducer worker integration owns scheduling. Backup is a synchronous
off-tick operation requiring exclusive `&mut DiskStore`, so reads, saves and
compaction cannot interleave on the same owner.

## Algorithms and failure policy

1. Create the root if absent, then acquire root/world.lock using native
   `File::try_lock` before AtomicFiles::open or metadata reads. Reject a symlink
   lock. A contending lock maps to Io/Load/WouldBlock. Match Go's real flock on
   Unix and verify interoperability in both directions with a child Go process.
   Do not delete the lock file. release is idempotent; a failed unlock retains
   its File. Successful release drops the unlocked File. Other I/O errors retain
   their ErrorKind. The lease is held as long as a live backend has incomplete
   close; Rust Drop is ordinary final resource cleanup, not a durability claim.
2. AtomicFiles::load(Metadata) succeeds with the decoded value; only NotFound
   uses options.create and saves it atomically with sequence 1. Other failures
   abort without replacement. Initial caller metadata sequence starts at 2 for
   both existing/new roots; it is process-local and never enters format v6.
   A loaded root's seed is authoritative regardless of options.create.
3. Region path is dimensions/<dimension>/regions/r.<x>.<z>.region. On a load
   miss test canonical file existence before RegionIo::open, which otherwise
   creates it. Reject nonrepresentable dimensions. Creating the hierarchy for
   saves synchronizes each newly created directory's parent before proceeding;
   retries sync existing ancestors too, so failed barriers cannot be skipped.
   Use fallible DiskIo close for file/directory operations. No Windows no-op
   durability fallback: unqualified platform errors must remain visible.
4. Keep at most region_handle_cap descriptors (0 means 256), keyed by storage
   RegionKey, with monotonically ordered use marks; renormalize on overflow.
   Evict least recent (key tie) after sync and fallible close. Refuse the new
   open on failure, preserving the lease and bounded count. Since this owner is
   serial there is no in-flight reference exception or mutex protocol to invent.
5. Before any write, check all snapshot key/value identities and embedded
   revisions: chunk coordinate/dimension, player id, standalone aggregate
   revisions, metadata sequence nonzero. Run codec validation without emitting
   bytes to disk. A malformed batch returns every original owned snapshot,
   identical ticket/submitted order, no commits and an error. Select highest
   revision per key; only conflicting equal highest values reject the batch.
   Lower superseded values cannot veto it. Process selected keys in variant declaration order
   (Chunk, Player, Companions, Hostiles, Passives, Metadata), sorting chunk
   identity by (dimension,x,z) and player identity by its 16 bytes. SaveKey does
   not implement Ord: use a private comparator, without changing contracts.
   Group chunks by region with sorted region and chunk keys. Stop after the first I/O
   failure; retain every earlier durable (key, revision). Region outcomes may
   contain both commits and an error. Do not rewrite returned snapshot ownership.
6. After a successful region save, compact only when wasted data sectors are
   >=8 MiB AND >=25% of data bytes. should_compact ensures a current bank view,
   obtains actual length, sums active entries; absent/nonpositive/impossible
   live>data returns false, actual stat errors return Err. A compaction failure
   reports error but never removes the preceding save's durable acknowledgments.
7. sync synchronizes cached regions and AtomicFiles. close first freezes further
   operations, then synchronizes once, closes regions in key order, closes
   AtomicFiles and finally releases the lease. Record completed phases so a
   retry never syncs an already closed region. A close error retains the lease;
   a consumed region descriptor can be removed only after its retryable close
   boundary is accounted for. No failed close is reported as success on the
   original call. Successful close/release is idempotent.
8. Backup ports Go storage/backup.go, including source-owned extra files.
   Resolve absolute lexical source and destination for identity and canonical
   source/parent for rejecting self/child and symlink aliases. Existing target
   must be a real directory with regular <=4096-byte identity
   `.mcgo-world-backup-v1.json` matching {source,seed,migration_version:1};
   matching reuse still syncs/closes the destination parent. Reject all other
   existing targets unchanged. New copy uses a unique sibling .<name>.tmp-*,
   sorted recursive traversal and a <=64 KiB copy buffer. Lstat each entry and
   reject symlinks before exclusions. Skip world.lock, identity, and exact Go
   hidden temporary patterns (a second dot is required, e.g. .a.tmp-*).
   Reject remaining nonregular files after exclusions, matching Go even for a
   temp-named FIFO. Preserve Unix permission bits.
   Sync and fallibly close each complete file, write identity with serde_json,
   sync directories bottom-up, recheck target absence and cancellation, rename,
   then sync/close parent without checking cancellation after publication.
   Use DiskIo boundary hooks around real copy/write/sync/rename operations.
   An error before rename never publishes a target; an error after rename leaves
   a complete reusable identity-matching target and reports failure. Remove only
   the temporary directory created by this attempt. Backup is not restore or a
   cross-family checkpoint; restore remains node 3.8.

## Red-first acceptance

In recovery.rs first write failing tests covering: second-process contention;
Go lock prevents Rust and Rust prevents Go; absence creates exactly metadata;
corrupt/future metadata remains byte-identical; existing seed wins; chunk read
miss creates no region; all six families round-trip through DiskBackend; bad
identity/revision refuses whole batch; highest duplicate arbitration; a later
family/region injected fault keeps earlier commits and all snapshot ownership;
cache cap 1 evicts/reopens; failed close retains lock until retry; recovered
standby chunk retains revision/needs_rewrite without load-time write; backup
copies extra files and codecs while excluding temporary files/lock; wrong
identity/self/child/symlink/FIFO refusal; matching backup retry after parent
sync failure; cancellation and partial-write faults never publish partial files.
Use actual files and injected real-operation hooks, not backend doubles. Tests
must not assume Windows proof from Unix results. Include a compaction policy
boundary test; reuse accepted region crash tests rather than duplicate them.

Validation: focused recovery tests, full server tests, clippy -D warnings and
fmt check using Rust 1.97.1/--locked; make rust before Go tests on the clean
isolated baseline; Go storage oracle tests (world files/disk/backup) and actual
native cross-runtime subprocess lock proof. Controller separately integrates,
reviews and runs strict OpenSpec. Report commands, red/green evidence, exact
commit and any conflict; do not waive or invent policy for an uncovered case.
