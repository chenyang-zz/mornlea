# Noncompressed record packets

Rust source root: `packages/engine/crates/mornlea_storage`. Go runtime-oracle root: `packages/tools/cmd/runtime-oracle`. Every `_into` writer implements this ordered template: validate the complete input using the existing decoder-compatible rules; compute exact length with checked arithmetic; reject format maximum; reject `dst.len() < needed` as `StorageError::OutputTooSmall { needed, available:dst.len() }`; only then write through a bounded slice cursor whose remaining writes cannot fail; return `needed` and leave the suffix unchanged. Invalid input wins over short capacity. A valid convenience `encode` allocates exactly `needed` and calls `_into`. Test N-1, N, N+7 destinations filled with 0xA5, invalid+short precedence, and an unchanged source object. The first writer node owns private `src/bytes.rs` slice-writer support; later nodes consume that exact API. An error after the first destination write is a design defect, not a reason to accept partial output.

For every corpus node, the Go producer runs actual exported pure codecs, except metadata's package-local tests. The [corpus contract](00-corpus-contract.md) fixes raw binary input, exact `arguments`, the complete typed `value_sha256`, error classes and candidate handoff. The Rust module executes the corresponding crate-root API and proves at least one changed scalar makes the complete digest differ. The producer exports candidates outside the repo; the controller reviews asset/source hashes, integrates one family, then runs the **nonempty** Rust corpus, Go inventory/trace and Rust domain/protocol/engine consumers before committing. A pre-integration `-- --list` is a compile check only.

<a id="node-2-1"></a>
## Node 2.1 — player bounded writer

**Prerequisites:** 1.4b. **Editable:** `src/bytes.rs`, `src/player.rs`, `src/lib.rs`, new `tests/player_buffer.rs`. **Read-only:** Go `packages/server/storage/player/player_codec.go`, current v9 binary and old Rust `tests/runtime_contract.rs`. **Produce:** `player_encoded_len(&PlayerSave)->StorageResult<usize>`, `encode_player_into(&PlayerSave,&mut[u8])->StorageResult<usize>`. Existing `encode_player` and `decode_player` keep signatures.

**Cases:** valid v9 fixture exact bytes and a fresh save with 20-byte raw armor; N-1/N/N+7 canaries; invalid player UUID, revision 0, health 21, saturation one above hunger*1000, pitch beyond inclusive ±pi/2, absent respawn with dirty location tail normalized to zeros; exhaustion 65,535 accepted. A validated constructed v9 DTO cannot reach the 1 MiB payload cap; node 2.2 tests an oversized declared payload length in a binary envelope. A respawn flag outside 0/1 is a mutated binary decode case in node 2.2 because the constructed DTO field is Boolean. Invalid+short must report corruption and preserve every canary. An unchanged input with an invalid ordinary inventory item 66 rejects; raw armor (4242,65,999) succeeds. Baseline red: no caller-buffer/length interface, and a substantive canary/precedence test must become green.

**Implementation:** derive v9 payload length from fixed fields plus display-name byte length with `checked_add`; validate all fields first; use `SliceWriter<'a> { dst:&'a mut [u8], pos:usize }` with private infallible `u8/u16/u32/u64/bytes` methods after preflight. Compute CRC32C over the same header/payload spans as current `encode`, patch only bytes inside N, then return N. The wrapper allocates N, calls into, and cannot silently fall back to the old Vec encoder. Do not validate armor as an ordinary domain stack or apply runtime health correction.

**Validation:** `rustup run 1.97.1 cargo test --manifest-path packages/engine/Cargo.toml -p mornlea_storage --test player_buffer --locked -- --list` then without list; `... --test runtime_contract --locked player_`; storage clippy with `-D warnings`. **Commit:** `feat(storage): add atomic player writer`. **Rollback:** revert this API node and its consumers together; no save rewrite.

<a id="node-2-2"></a>
## Node 2.2 — player version corpus

This is the complete case matrix; the actual worker nodes and scoped commits are [2.2a–2.2d](05-dispatch-slices.md#node-2-2a). Do not assign this parent matrix as one job.

**Prerequisite:** 2.1. **Editable:** Go `storage_player_test.go`, Rust `tests/storage_corpus/player.rs`; controller updates `inventory.go`, dispatcher registration and manifest/assets serially. **Read-only:** Go `packages/server/storage/player/testdata/player-v1.bin` through v9 and pure codec/migration source.

**Case matrix:** decode each v1..v9 fixture and compare every ID, revision, name, location, safe position, yaw/pitch, inventory slot, health, hunger, saturation, exhaustion, respawn, raw armor, `needs_rewrite` and current normalized schema. Current v9 encode bytes are exact. For each old version, compare Rust decoded value with Go migration result, then current re-encode with Go's current encode of that result, never with the old bytes. Negatives: wrong requested UUID; revision zero; unsupported schema 0/future 10; truncated header/body; trailing byte; bad CRC; resealed invalid health/pitch/respawn flag; short destination N-1. Include absent respawn dirty 16-byte location tail accepted on decode then written as zeros. Mutation of one armor byte or rewrite flag must fail comparison.

**Producer/consumer:** Go calls `player.Decode(wantID, bytes)` and `player.Encode(save)` on copied fixtures; it writes decimal-string u64 revisions. Rust calls `decode_player`, `encode_player_into`; no fixture update flag. Register decode routes for 1..9 and current encode route 9 with nonzero cases and one zero checkpoint per case.

**Family validation after 2.2d:** `go test ./packages/tools/cmd/runtime-oracle -run '^TestStoragePlayer' -count=1`; `rustup run 1.97.1 cargo test --manifest-path packages/engine/Cargo.toml -p mornlea_storage --test storage_corpus --locked player_ -- --list` and without list; `... --test runtime_contract --locked player_`. Commits and rollback ownership are per dispatch slice.

<a id="node-2-3"></a>
## Node 2.3 — raw metadata and atomic writer

**Prerequisite:** 2.2d. **Editable:** `src/world_metadata.rs`, `src/lib.rs`, new `tests/metadata_buffer.rs`. **Read-only:** Go `packages/server/storage/metadata.go`, existing Rust `runtime_contract.rs` metadata tests. **Produce:** `world_metadata_encoded_len(&Metadata)->StorageResult<usize>` and `encode_world_metadata_into(&Metadata,&mut[u8])->StorageResult<usize>`; current encode/decode signatures remain.

**Cases:** current `weather_kind=7` and 255, `spawn_dimension=-3`, `day_phase_offset=u64::MAX`, signed seed boundary, full remaining-ticks u32, difficulty 0..2 accepted and byte-preserved; difficulty 3 rejected. N=78 with N-1/N/N+7 canaries; invalid difficulty+short returns corruption first. Version 0/7, wrong two-dimension count, truncated/trailing/header/CRC errors remain rejected. Baseline red: Rust weather 7/255 encode and decode reject while Go codec preserves them; existing test that expects weather rejection must be revised by this node, not hidden.

**Implementation:** remove weather 0..2 codec admission in both encode and decode, retaining difficulty validation. Do not change `valid_weather` as a runtime helper unless its name/callers are reconciled; it may still report runtime-known kinds but the codec must not call it. Exact current output is 78 bytes. Use the shared slice writer; do not modulo phase or clamp weather here.

**Validation:** `rustup run 1.97.1 cargo test --manifest-path packages/engine/Cargo.toml -p mornlea_storage --test metadata_buffer --locked` red/green and list; `... --test runtime_contract --locked metadata_`; `go test ./packages/server/storage -run '^TestMetadata' -count=1`. **Commit:** `fix(storage): preserve raw metadata values`. **Rollback:** revert this node; schema stays v6.

<a id="node-2-4"></a>
## Node 2.4 — metadata version corpus

**Prerequisite:** 2.3. **Editable:** new Go `packages/server/storage/metadata_oracle_test.go` in package `storage`, new Rust `tests/storage_corpus/metadata.rs`; controller registers routes/assets. **Read-only:** Go private `encodeMetadata/decodeMetadata` and legacy builders in `metadata_*_test.go`.

**Cases:** exact v1..v6 total lengths 36/44/52/57/77/78; v1 time 0, v1/v2 phase 0, v1..v3 weather clear/remaining 0, v1..v4 Depths anchor equal spawn and salt `0x9E3779B97F4A7C15`, v1..v5 normal difficulty 0. Preserve arbitrary raw spawn dimension, weather 7/255, phase max and signed seed in v6. Two-dimension count !=2, difficulty 3, wrong CRC/header/version, truncated/tail bytes reject. Each historical decode compares all fields; current encode is exact bytes. Mutating one anchor/salt/weather byte in normalized expectation must fail.

**Producer:** package-local test calls the private pure codec and existing v1..v5 builders, never `DiskStore` or `MemoryStore`; it implements the guarded standalone `storage/metadata` exporter and writes `selection.json` plus assets at the exact relative paths in [the handoff contract](00-corpus-contract.md). It cannot import the runtime-oracle `_test.go` helper. With the export variable unset it writes nothing; with a repo-contained, symlinked, duplicate or pre-existing child it fails before opening an asset. The controller adds this producer file to `Discover` for metadata, verifies its source hash and imports the external candidate through `readStorageSelection`; no production export is added. Rust calls `decode_world_metadata` and `encode_world_metadata_into`. Register decode routes 1..6, encode route 6.

**Validation:** `go test ./packages/server/storage -run '^TestMetadataOracle$' -count=1`; Rust `--test storage_corpus --locked metadata_ -- --list` and execution; `... --test runtime_contract --locked metadata_`. **Commit:** `test(storage): execute metadata version corpus`. **Rollback:** family routes/assets/tests only.

<a id="node-3-1"></a>
## Node 3.1 — hostile bounded writer

**Prerequisite:** 2.4 for serial shared byte writer/exports. **Editable:** `src/hostile.rs`, `src/lib.rs`, new `tests/hostile_buffer.rs`. **Read-only:** Go `packages/server/storage/hostile/hostile_codec.go`. **Produce:** `hostile_mobs_encoded_len(&HostileMobsSave)->StorageResult<usize>`, `encode_hostile_mobs_into(&HostileMobsSave,&mut[u8])->StorageResult<usize>`.

**Cases:** empty revision 1 accepted, revision 0 rejected; 64/65 records; unsorted IDs 2,1 encoded as 1,2 without changing input; duplicate ID; Y -64/319 accepted, 320 rejected; health 0/21; cooldown 20/21; distant 600/601; target absent with nonzero raw bytes, target present with invalid UUID; kind 0/1 accepted, 2 rejected; `next_repath_ticks=u64::MAX` preserved; N-1/N/N+7 canaries and invalid+short precedence. v2 record length 73. Baseline red: missing writer API; canonical/negative cases give substantive oracle.

**Implementation:** validate count <=64 and every record before allocating sorted `Vec<usize>`; `try_reserve_exact` at most 64, sort indices by ID, detect duplicates, compute `32 + 73*count` with checked arithmetic, then write canonical bytes and CRC through shared slice writer. Remove full record-vector cloning. Keep absent target zero encoding and no path/planning fields.

**Validation:** Rust `--test hostile_buffer --locked` list and execution; `--test runtime_contract --locked hostile_`; `go test ./packages/server/storage/hostile -count=1`. **Commit:** `feat(storage): add atomic hostile writer`. **Rollback:** this writer node only.

<a id="node-3-2"></a>
## Node 3.2 — hostile v1/v2 corpus

**Prerequisite:** 3.1. **Editable:** Go `storage_hostile_test.go`, Rust `tests/storage_corpus/hostile.rs`; controller handles route/assets. **Read-only:** Go v1/v2 binaries and codec.

**Cases:** v1 72-byte records decode with kind 0; v2 73-byte records keep kind and current exact bytes. The hostile decoder has no source-version or rewrite field, so compare only its actual normalized fields and a separate current v2 encode result. Include count 0/64/65; unsorted input canonical output; duplicate/out-of-order stored ID; Y, health, cooldown, distant, target relationship, kind, reserved/header/truncated/trailing/CRC. Reseal CRC for target/kind semantic negatives. Compare every field, including `next_repath_ticks`, and mutate one target-presence or kind observation to prove comparator sensitivity. Register decode routes 1/2 and encode route 2.

**Validation:** `go test ./packages/tools/cmd/runtime-oracle -run '^TestStorageHostile' -count=1`; Rust `--test storage_corpus --locked hostile_ -- --list` and execution; `--test runtime_contract --locked hostile_`. **Commit:** `test(storage): execute hostile save corpus`. **Rollback:** family corpus only.

<a id="node-3-3"></a>
## Node 3.3 — passive bounded writer

**Prerequisite:** 3.2 for serial shared exports. **Editable:** `src/passive.rs`, `src/lib.rs`, new `tests/passive_buffer.rs`. **Read-only:** Go `packages/server/storage/passive/passive_codec.go`. **Produce:** `passive_mobs_encoded_len(&PassiveMobsSave)->StorageResult<usize>`, `encode_passive_mobs_into(&PassiveMobsSave,&mut[u8])->StorageResult<usize>`.

**Cases:** empty save revision 1 accepted, revision 0 rejected; 32 records accepted, 33 rejected before cloning/sorting; IDs 2,1 canonicalize without mutation, duplicate rejects; dimension 1, Y 320, health 0/21 reject. Boolean byte 2 is a resealed binary decode case in node 3.4 because the constructed DTO field is Boolean. Assert exact 72-byte records with 30 zero reserved bytes, maximum file 2,336, N-1/N/N+7 canary and invalid+short precedence. No grazing, fleeing or birth runtime fields added.

**Implementation:** validate before sorted index reservation, use at most 32 indices, exact `32 + 72*count` checked length and shared slice writer. Preserve current v1 bytes. **Validation:** Rust `--test passive_buffer --locked` list/execution, `--test runtime_contract --locked passive_`, `go test ./packages/server/storage/passive -count=1`. **Commit:** `feat(storage): add atomic passive writer`. **Rollback:** writer node only.

<a id="node-3-4"></a>
## Node 3.4 — passive v1 corpus

**Prerequisite:** 3.3. **Editable:** Go `storage_passive_test.go`, Rust `tests/storage_corpus/passive.rs`; controller handles routes/assets. **Read-only:** Go passive-v1 binary, codec and tests.

**Cases:** fixture exact decode/current re-encode; empty revision 1, 32/33 count, canonical ID order/duplicate, dimension/Y/health rejection, a Boolean byte of 2 rejected after CRC reseal, every one of 30 reserved-tail bytes changed with CRC resealed, wrong version/header/CRC, truncated and trailing bytes. Mutate one record coordinate or reserved-field verdict in expected JSON to prove comparator execution. Register v1 decode and encode routes; case count must be nonzero.

**Validation:** `go test ./packages/tools/cmd/runtime-oracle -run '^TestStoragePassive' -count=1`; Rust `--test storage_corpus --locked passive_ -- --list` and execution; `--test runtime_contract --locked passive_`. **Commit:** `test(storage): execute passive save corpus`. **Rollback:** this family corpus only.
