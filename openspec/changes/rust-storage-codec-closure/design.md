## Context

This change begins only after `rust-storage-safety-repairs` closes. The Rust storage crate already contains current-schema round trips and historical fixture decoders, and the archived region successor already implemented fixed banks and caller-buffer encoders. Those tests do not supply a complete executable Go/Rust save corpus: all seven `save.*` rows have zero cases in the current manifest. The Go server remains the only live authority.

## Goals / Non-Goals

**Goals:** prove every supported save version and important failure class through independent Go outcomes; provide bounded atomic Rust output APIs; retain logical rather than compressed-byte parity for chunk zstd.

**Non-Goals:** file-store implementation, background I/O, crash recovery, a schema/version bump, online Rust authority or F1 numerical acceptance.

## Decisions

### Format/codec boundary and exact interfaces

Reuse the raw format DTOs and checked current-value conversions from the safety change. All noncompressed writers keep their existing `encode_*(&Save)->StorageResult<Vec<u8>>` convenience function and add:

| Family token | Exact new crate-root functions | Maximum |
| --- | --- | --- |
| player | `player_encoded_len(&PlayerSave)->StorageResult<usize>`; `encode_player_into(&PlayerSave,&mut [u8])->StorageResult<usize>` | payload 1 MiB |
| world_metadata | `world_metadata_encoded_len(&Metadata)->StorageResult<usize>`; `encode_world_metadata_into(&Metadata,&mut [u8])->StorageResult<usize>` | fixed v6 78 bytes |
| hostile_mobs | `hostile_mobs_encoded_len(&HostileMobsSave)->StorageResult<usize>`; `encode_hostile_mobs_into(&HostileMobsSave,&mut [u8])->StorageResult<usize>` | 64 records |
| passive_mobs | `passive_mobs_encoded_len(&PassiveMobsSave)->StorageResult<usize>`; `encode_passive_mobs_into(&PassiveMobsSave,&mut [u8])->StorageResult<usize>` | 32 records, 2,336 bytes |
| companions | `companions_encoded_len(&CompanionSave)->StorageResult<usize>`; `encode_companions_into(&CompanionSave,&mut [u8])->StorageResult<usize>` | 64 bodies, 393,904 bytes |

`StorageError::OutputTooSmall { needed, available }` already exists. A shared private slice writer in `src/bytes.rs` may write only after complete validation and checked length calculation; every remaining write is then infallible within the declared length. Each wrapper allocates exactly its checked length and calls its `_into` function. For canonical entity order, validate all records first, then sort at most 64 indices with fallible reservation; do not clone whole records or large strings to sort. Do not use a temporary output allocation merely to simulate atomicity.

The chunk family is different: `ChunkCodec::try_new()->StorageResult<Self>` owns one compressor, one decompressor and reusable logical/compressed scratch. `ChunkCodec::encode_into(&mut self,&ChunkSave,&mut [u8])->StorageResult<usize>` and `decode(&mut self,ChunkKey,u64,&[u8])->StorageResult<DecodedChunk>` are sequential caller-owned operations; `chunk_logical_len(&ChunkSave,u32)->StorageResult<usize>` is an exact preflight for diagnostic logical encoding. Free `encode_chunk/decode_chunk` construct a context as allocating conveniences. Validate aggregate and 2 MiB logical length before compression; enforce the 1 MiB compressed frame, then destination capacity, then copy the complete 44-byte envelope plus frame in one publication. Failed decode resets scratch visibility and returns no partial chunk. A context is neither global nor shared across concurrent callers.

### Go evidence and frozen consumer

The production Go codec is the source of expected values. Every family has an owned test-only producer; chunk has two because its historical envelope builder is package-private. Metadata uses `packages/server/storage/metadata_oracle_test.go`, and chunk history uses `packages/server/storage/chunk/chunk_oracle_test.go`; other family cases use `packages/tools/cmd/runtime-oracle/storage_<family>_test.go`, including current chunk cases. Each producer executes real Go decode/encode or private builder operations and exports reviewed input/expected/encoded assets create-exclusively under `RUNTIME_ORACLE_EXPORT_DIR`, never into the repository. Package-local producers implement a guarded standalone exporter and hand over the same `selection.json` plus asset layout; they do not import the runtime-oracle test binary. Case IDs have the form `save.<family>/<version>/<operation>/<label>`; one zero checkpoint applies to stateless codec cases. `CaseSpec.arguments` binds requested player ID, chunk key/revision and region key/file size/selection operands to raw binary input. The [frozen corpus contract](plans/00-corpus-contract.md) specifies all keys, operations, typed result digests, error classes, source identity and integration order. Binary corruption tests reseal CRC when the intended oracle is a semantic field. Unknown/future version, truncated header/body, trailing bytes, over-limit declared lengths, wrong identities, checksum and canonical-order cases are separate.

Add `mornlea_storage` to `BaselineConsumerRegistry` only with real `ConsumerRoute { FamilyID, Version, Operation }` registrations. The first region node creates one real executed route; later family nodes append only their executed routes. A test-only `StorageSelection { Cases []CaseSpec; Sources []SourceSpec; Routes []ConsumerRoute }` and `mergeStorageSelections(root, base, ...)->(Inventory,error)` clone the manifest, reject duplicate/conflicting/unrouted cases and missing/stale source hashes, preserve unrelated cases, sort case/source lists and reconcile after assets are integrated. Preserve the existing equal manifest/Go source-revision fields during per-family commits; exact `Family.Sources` hashes bind each edited producer. At closure, capture the last committed family result SHA, update both revision fields once, then rerun derived consumers and commit. The controller alone reviews external candidates, copies assets and complete manifest, then runs the full derived consumer set. Add `CorpusConsumer::Storage` to the shared Rust loader before the first storage case, because the loader otherwise rejects the entire manifest in domain/protocol/engine tests. Rust `tests/storage_corpus.rs` dispatches every registered save route to the real public codec and compares a complete typed value digest plus output bytes/logical bytes; it rejects an unknown route or a zero selected case set. No expected value is copied from Rust output. A worker may compile/list before integration; the controller integrates the candidate before requiring the nonzero Rust corpus gate.

### Per-family compatibility

Player reads v1..v9 and writes v9; raw armor and absent-respawn residue remain byte faithful. Metadata reads v1..v6, writes 78-byte v6 and preserves every weather u8; runtime weather clamp and day-phase modulo are excluded. Hostile reads v1/v2, writes 73-byte v2 records with v1 kind=0 migration; passive reads/writes v1 72-byte records with 30 zero reserved bytes. Companion reads v1..v5, writes v5 only from explicit valid v5 input; legacy decoding does not bootstrap namespace/lifecycle. Chunk reads v1..v9, writes v9 by default and compares logical payload and both cross-decodes rather than zstd compressed bytes. Region v1 encoding is already implemented; this change supplies executable cross-language selection and corruption evidence only.

### Dependency, ownership and rollback

Run region/evidence foundation first, then the five noncompressed families, then reusable chunk and companion work, then source-bound whole-storage closure. Shared `src/lib.rs`, `src/bytes.rs`, `inventory.go`, `contracts.json` and the Rust corpus dispatcher are serialized controller integration points; no concurrent workers edit them. Family production source and producer test files are exclusive per node. A failed slice is reverted with its producer/module delta, routes, assets and matching family source hash while previous slices stay valid. Format codecs never open a live world. Scoped guides are updated only when their ownership or validation map changes.

## Risks / Trade-offs

- Historical decoders may admit raw fields invalid as gameplay values -> normalize only according to the Go migration, then apply current-value rules at the documented boundary; do not invent repairs.
- A compressed-byte comparison would fail on two valid zstd encoders -> compare complete logical bytes and both decode directions, while keeping size/CRC/frame-header assertions.
- A manifest with names but no execution could falsely close F1 -> require nonempty discovery, route dispatch and scalar/digest mutation failures for every family.
- An output writer may mutate a canary before finding a late validation failure -> compute/validate length and sorted order completely before touching the destination.
- Two agents could overwrite one manifest/export -> producer candidates remain external and controller integrates one family at a time.

## Migration Plan

Implement the node packets in `tasks.md` with red-green evidence and scoped commits. Do not touch production Go codecs or committed source fixtures. Run format, Rust workspace, six-module Go vet/race, audit, strict OpenSpec and corpus identity gates serially on one result SHA. Only an accepted codec closure may feed the later F1 contract gate; F2 remains blocked until all F1 successors and zero-gap acceptance pass. Rollback selects the previous Rust codec revision, not a live save downgrade.
