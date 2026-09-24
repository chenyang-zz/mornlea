# Chunk and companion aggregate packets

Rust root is `packages/engine/crates/mornlea_storage`; Go oracle root is `packages/tools/cmd/runtime-oracle`. Corpus nodes consume [the exact evidence contract](00-corpus-contract.md); all nodes consume accepted safety validators. Do not weaken their error order. No codec node opens a file or starts a runtime worker. Each writer owns its scratch for one mutable call; no global cache, lock or shared mutable buffer. The controller serializes `src/lib.rs`, Rust corpus module registration, Go registry and manifest/assets.

<a id="node-4-1"></a>
## Node 4.1 — chunk logical length

**Prerequisites:** 3.4 and safety node 2.2. **Editable:** `src/chunk.rs`, `src/lib.rs`, new `tests/chunk_codec.rs` logical tests. **Read-only:** Go `packages/server/storage/chunk/chunk_codec_logical.go`, v1..v9 binaries, Rust `runtime_contract.rs`. **Produce:** `chunk_logical_len(&ChunkSave, schema:u32)->StorageResult<usize>`.

**Cases:** supported schemas 1..9 return the exact byte length actually emitted by `encode_chunk_logical` for a valid all-air chunk and a worst legal indexed palette; schema 0/10, revision 0, invalid dimension, 23/25 sections, invalid palette and active furnace on air fail before any logical output. A fully valid fixed-shape chunk cannot reach 2 MiB, so node 4.3 tests the >2 MiB declared logical length at envelope decode. Keep checked arithmetic and the cap in preflight even though the current legal shape stays below it. Version-dependent omission is explicit: v1 no drops/furnaces/chests, v2..3 drops only, v4..5 drops+furnaces, v6..9 all three; v2..4 drops lack durability until v5. Compare each length against actual Go private logical builder output where supported, never assume the current layout for old versions. Baseline red: the preflight API is absent; the length/equivalence assertions are the substantive oracle.

**Algorithm:** call the complete safety validator first, compute header + each section's fixed/variable payload + version-selected fixed slot arrays with `checked_add/checked_mul`, reject >2 MiB. Do not compress, allocate a dense block array or silently truncate. Wire `encode_logical` to this preflight so a later writer can reserve exactly the length. **Validation:** Rust `--test chunk_codec --locked logical_ -- --list` and execution, `--test runtime_contract --locked chunk_`, storage clippy. **Commit:** `feat(storage): preflight chunk logical length`. **Rollback:** node only.

<a id="node-4-2"></a>
## Node 4.2 — reusable chunk context

**Prerequisite:** 4.1. **Editable:** `src/chunk.rs`, `src/lib.rs`, `tests/chunk_codec.rs` context tests. **Read-only:** Go chunk codec and region size constants. **Produce:** `ChunkCodec::try_new()->StorageResult<Self>`; `encode_into(&mut self,&ChunkSave,&mut[u8])->StorageResult<usize>`; `decode(&mut self,ChunkKey,u64,&[u8])->StorageResult<DecodedChunk>`. Free `encode_chunk/decode_chunk` remain allocating wrappers.

**Construction and algorithm:** own `zstd::bulk::Compressor<'static>`, `zstd::bulk::Decompressor<'static>`, `Vec<u8>` logical and compressed scratch in the struct. Construct contexts fallibly; configure compression level equal to current `COMPRESSION_LEVEL`, one worker, checksum and content-size flags. For each encode, clear scratch lengths, run safety validator and `chunk_logical_len`, fill exact logical bytes, pledge source size, compress into bounded scratch, reject frame >1 MiB, compute total `44+frame_len` with checked add, then check destination and copy complete envelope+frame; N-1 canary stays unchanged. For decode, validate envelope/version/key/revision/declared size and compressed bound, decompress into a destination capped by declared <=2 MiB, require exact result length and checksum, parse/migrate/validate current aggregate, return owned result. On any error clear visible scratch lengths so a later valid call cannot use stale bytes. Never retain input/destination references in the result.

**Cases:** valid v9 through fresh and reused context, identical logical fields and documented frame-header/checksum to existing free API; N-1/N/N+7 output canaries; invalid aggregate plus short capacity reports corruption first; wrong key/revision, future schema, compressed length >1 MiB, decoded length >2 MiB, truncated frame, zstd checksum flip and bomb fail without value; then valid request through the same context equals a fresh context. Two independent contexts interleaved produce equivalent values; no global state. Rust/Go compressed block bytes are not compared. Baseline red: no context or caller-output API.

**Validation:** Rust `--test chunk_codec --locked context_ -- --list` and execution; `--test runtime_contract --locked chunk_`; `make rust`. **Commit:** `feat(storage): add bounded reusable chunk codec`. **Rollback:** revert this API node; older allocating free functions remain the compatibility surface.

<a id="node-4-3"></a>
## Node 4.3 — chunk corpus and cross-decode

This is the complete case matrix; actual worker nodes and scoped commits are [4.3a–4.3c](05-dispatch-slices.md#node-4-3a). Do not assign this parent matrix as one job.

**Prerequisite:** 4.2. **Editable:** Go `storage_chunk_test.go`, Go package-local `packages/server/storage/chunk/chunk_oracle_test.go`, Rust `tests/storage_corpus/chunk.rs`; controller owns route/assets. **Read-only:** Go `chunk-v1.bin` through v9 and Go production codec.

**Case matrix:** all nine fixtures decode to Go's complete normalized key/revision, every section palette/packed word, 32 drop slots, 32 furnace slots, 16 chest slots and migration/rewrite result; v1 no drops, v3 no furnace, v5 no chest, v4 legacy tool durability/stack split, and no synthetic water insertion. A full v9 valid current value round trips in both decoders. Rust-produced v9 frame is decoded by Go and Go-produced frame by Rust; compare complete logical bytes and current normalized fields, not compressed block bytes. Invalid cases include schema 0/10, wrong key/revision, truncated envelope/frame, trailing bytes, checksum mutation, claimed size >2 MiB or frame >1 MiB, palette error, active container on wrong block, and insufficient free drop slots for legacy multi-stack split. Reseal envelope/CRC where the intended semantic path requires it. Mutate one packed word, slot generation or migration flag in expected JSON and prove the comparator fails.

**Producer:** `chunk.Encode/Decode` for current cases; package-local `testEnvelopeForSchema` and private logical helpers only in `chunk_oracle_test.go` for historical builders, no new production export. That file implements the guarded standalone `chunk/migration` exporter from the corpus contract; with export unset it writes nothing, and it refuses repo-contained/symlinked roots, duplicate paths and pre-existing children before file creation. The controller adds both Go producer files to `Discover` for `save.chunk` and imports their `selection.json` through `readStorageSelection`. Rust uses `ChunkCodec` plus public free decoders. Register decode routes 1..9 and encode route 9; `encoded` for the encode route holds Go's complete logical bytes, never the Go zstd frame. For the Rust-to-Go direction, a Rust context test emits one deterministic v9 frame to `RUST_STORAGE_FRAME_EXPORT_DIR` outside the repo; `TestChunkMigrationOracle` reads its absolute `RUST_STORAGE_FRAME_INPUT`, verifies Go `chunk.Decode` and exports an ordinary source-bound decode case. The controller checks this handoff before integrating it. Go-to-Rust is the ordinary Go-produced frame decode case. No Rust-produced observation is used as an expected result.

**Family validation after 4.3c:** `go test ./packages/server/storage/chunk -run '^TestChunkMigrationOracle' -count=1` with and without reviewed Rust-frame input; `go test ./packages/tools/cmd/runtime-oracle -run '^TestStorageChunk' -count=1`; controller integrates both external candidates, then Rust `--test storage_corpus --locked chunk_ -- --list` and execution plus `--test runtime_contract --locked chunk_`. A pre-integration zero-case Rust run is not green. Commits and rollback ownership are per dispatch slice.

<a id="node-4-4"></a>
## Node 4.4 — companion exact preflight

**Prerequisites:** 4.3c and safety node 1.2. **Editable:** `src/companion.rs`, `src/lib.rs`, new `tests/companion_buffer.rs` preflight tests. **Read-only:** Go `packages/server/storage/companion/companion_v5.go` and v5 codec. **Produce:** `companions_encoded_len(&CompanionSave)->StorageResult<usize>`.

**Cases:** exact length of v5 current fixture and generated 0, 1, 4 active/64 total-body saves; 65 bodies, five active, missing/duplicate lifecycle, orphan/inactive queue, 17 FIFO, 5,001 plan steps, 1,025-byte command and 2,049-byte summary all reject before a record/string clone. Reproduce Go's maximum legal v5 shape and accept exactly 393,904 bytes. No otherwise legal input exceeds that cap; test checked arithmetic with invalid constituent counts and lengths, preserving their error precedence. Accepted unsorted input returns the same length as canonical sorted Go encode and remains unchanged. Error precedence: revision, namespace, body count, lifecycle membership, per-record/task validity, encoded length. Baseline red: no exact preflight API; existing `encode` clones three vectors first.

**Algorithm:** use borrowed body/lifecycle/queue slices; validate full membership and task/text bounds; sort only at most 64/64/4 indices with fallible reservation; sum fixed and variable field byte lengths via checked addition; return total header+payload <=393,904. Do not build a temporary encoded Vec to measure length or clone task strings. The index-order plan may be private and consumed by 4.5, but `companions_encoded_len` is the public result.

**Validation:** Rust `--test companion_buffer --locked preflight_ -- --list` and execution, `--test runtime_contract --locked companion_`, storage clippy. **Commit:** `feat(storage): preflight companion save length`. **Rollback:** this preflight node only.

<a id="node-4-5"></a>
## Node 4.5 — companion caller-buffer writer

**Prerequisite:** 4.4. **Editable:** `src/companion.rs`, `src/lib.rs`, `tests/companion_buffer.rs` writer tests. **Produce:** `encode_companions_into(&CompanionSave,&mut[u8])->StorageResult<usize>`; existing `encode_companions` delegates through length + exact allocation.

**Cases:** current v5 fixture byte equality, unsorted records/lifecycles/queues canonicalized without changing input, valid empty queue omitted, nonempty legacy summary in v5 rejected; N-1/N/N+7 canary and invalid+short precedence. Pin current task with every go_to/mine/place/follow step discriminant, step index boundary, state/failure reason, FIFO order and memory/tombstone alternatives. A final call after a rejected oversized request still emits the same bytes. Baseline red: missing caller-buffer API and full clone strategy.

**Algorithm:** call complete 4.4 preflight and its bounded sorted-index plan before touching `dst`; use shared slice writer to emit header/body/lifecycle/task/FIFO in canonical order, CRC over the existing spans, return exact length. Do not clone records, task plans or strings; do not generate namespace or lifecycle. Validate before capacity, then one infallible write phase.

**Validation:** Rust `--test companion_buffer --locked writer_ -- --list` and execution, full `--test companion_buffer`, `--test runtime_contract --locked companion_`; `go test ./packages/server/storage/companion -count=1`. **Commit:** `feat(storage): add atomic companion writer`. **Rollback:** this writer node and its wrapper change, retaining preflight.

<a id="node-4-6"></a>
## Node 4.6 — companion version corpus

This is the complete case matrix; actual worker nodes and scoped commits are [4.6a–4.6b](05-dispatch-slices.md#node-4-6a). Do not assign this parent matrix as one job.

**Prerequisite:** 4.5. **Editable:** Go `storage_companion_test.go`, Rust `tests/storage_corpus/companion.rs`; controller owns routes/assets. **Read-only:** Go v1..v5 binaries and pure codec.

**Cases:** decode every v1..v5 fixture; compare source schema, revision, namespace, body identity/pose/inventory, lifecycle IDs/active/memory/tombstone alternatives, task command/step/state/index/start/deadline/reason, FIFO order and legacy v4 summary. v1 has no queue; v2/v3/v4 retained queues carry owner IDs from safety node 1.1. V5 current encode uses an explicit Go-valid `CompanionSave`; legacy decoded values without namespace/lifecycles are not passed to v5 encoder or called a successful migration. Include 64/65, four/five active, duplicate/missing lifecycle, orphan/inactive queue, too-long command/plan/FIFO/summary, future/unsupported schema, truncated/trailing bytes, CRC corruption, malformed UUID. Mutate one owner ID or ordered FIFO element in expected JSON to prove comparator sensitivity.

**Producer:** Go `companion.Decode` on existing bytes and `companion.Encode` only on explicit valid v5 saves. Rust `decode_companions` and `encode_companions_into` mirror those operations. Register decode routes 1..5, encode route 5, with nonzero actual cases and one zero checkpoint each.

**Family validation after 4.6b:** `go test ./packages/tools/cmd/runtime-oracle -run '^TestStorageCompanion' -count=1`; Rust `--test storage_corpus --locked companion_ -- --list` and execution; `--test runtime_contract --locked companion_`. Commits and rollback ownership are per dispatch slice.
