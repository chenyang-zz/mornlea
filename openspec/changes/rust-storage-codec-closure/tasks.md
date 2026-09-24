# Rust storage codec closure — Superpowers implementation plan

> For worker agents: implement one linked node per assignment. This is the only checkbox/status source. Read the linked packet and its named source contracts; do not design a missing API or change a corpus identity. The controller integrates shared exports, case assets, manifest, status and ledger.

**Goal:** close executable Go/Rust parity and bounded output for all seven supported save families.

**Architecture:** one Rust format owner with caller-owned output, one Go producer per family, one source-bound frozen corpus under the [exact case contract](plans/00-corpus-contract.md), one Rust consumer dispatching real public codecs. Region format implementation is already complete.

**Tech stack:** Rust 1.97.1, Go 1.26, zstd 0.13 already pinned. **Spec:** [delta](specs/rust-runtime-foundation/spec.md); [design](design.md). **Prerequisite:** accepted `rust-storage-safety-repairs` ledger/result SHA.

**Global constraints:** read chunk/player 1..9, companion 1..5, hostile 1..2, passive/region 1, metadata 1..6; current writes 9/9/5/2/1/1/6. No protocol/save/ABI/version bump, live world, Go production edit, fixture rewrite or default switch. For each node: concrete failing case first, named nonempty discovery, green focused gate, controller-reviewed scoped commit. Corpus producers export only to fresh external create-exclusive directories; tracked assets and complete manifest are controller-owned.

**Review focus:** (1) a zero-case save family counted by name; (2) invalid value plus short destination returning capacity first or mutating canaries; (3) raw weather 255 rejected by Rust; (4) legacy companion decode being misrepresented as valid v5 re-encode; (5) chunk zstd block bytes compared instead of full logical cross-decode. Each is pinned in the owning packet.

## 1. Evidence foundation and region

- [x] 1.1 [Validate exact storage arguments and safe candidate selections](plans/01-evidence.md#node-1-1). Go `inventory.go`, `storage_manifest_test.go`, export ID table; run nonempty `^TestStorageSelection`.
- [x] 1.2 [Register the Rust storage corpus consumer and typed digest](plans/01-evidence.md#node-1-2). Shared `runtime_corpus.rs`, tests guide and storage corpus skeleton; run digest/parser tests plus existing derived consumers.
- [x] 1.3 [Execute the first region bank and superblock routes](plans/01-evidence.md#node-1-3). Go `storage_region_test.go`, Rust `storage_corpus/region.rs`; controller integrates reviewed candidates before the nonempty Rust region gate.
- [x] 1.4a [Execute committed-bank ordering and fallback](plans/05-dispatch-slices.md#node-1-4a). Add one real `order` route, integrate reviewed candidate, run nonempty region order gate.
- [x] 1.4b [Execute region geometry and corruption cases](plans/05-dispatch-slices.md#node-1-4b). Reseal semantic CRC mutations, integrate reviewed candidate, run nonempty region corruption gate.

## 2. Player and metadata

- [x] 2.1 [Add exact player caller-buffer writer](plans/02-records.md#node-2-1). Rust `src/{bytes,player,lib}.rs`, `tests/player_buffer.rs`; run `cargo test -p mornlea_storage --test player_buffer --locked` with the pinned workspace manifest.
- [x] 2.2a [Execute player v9 and current writer cases](plans/05-dispatch-slices.md#node-2-2a). Add real current decode/encode routes and nonempty Go/Rust gates.
- [x] 2.2b [Execute player v1–v4 migrations](plans/05-dispatch-slices.md#node-2-2b). Add four historical decode routes and one current re-encode case.
- [x] 2.2c [Execute player v5–v8 migrations](plans/05-dispatch-slices.md#node-2-2c). Add four historical decode routes and current re-encode case.
- [x] 2.2d [Close player malformed and boundary corpus](plans/05-dispatch-slices.md#node-2-2d). Add CRC, length, schema, ID, respawn and rewrite mutations.
- [x] 2.3 [Preserve raw metadata and add atomic writer](plans/02-records.md#node-2-3). Rust `src/{world_metadata,lib}.rs`, `tests/metadata_buffer.rs`; run Rust `--test metadata_buffer`.
- [x] 2.4 [Execute metadata v1..v6 and invalid corpus](plans/02-records.md#node-2-4). Go package-local `packages/server/storage/metadata_oracle_test.go`, Rust `storage_corpus/metadata.rs`; run Go `^TestMetadataOracle` and Rust `storage_corpus metadata_`.

## 3. Entity record families

- [x] 3.1 [Add hostile canonical bounded writer](plans/02-records.md#node-3-1). Rust `src/{hostile,lib}.rs`, `tests/hostile_buffer.rs`; run Rust `--test hostile_buffer`.
- [x] 3.2 [Execute hostile v1/v2 corpus](plans/02-records.md#node-3-2). Go `storage_hostile_test.go`, Rust `storage_corpus/hostile.rs`; run Go `^TestStorageHostile` and Rust `storage_corpus hostile_`.
- [ ] 3.3 [Add passive canonical bounded writer](plans/02-records.md#node-3-3). Rust `src/{passive,lib}.rs`, `tests/passive_buffer.rs`; run Rust `--test passive_buffer`.
- [ ] 3.4 [Execute passive v1 corpus](plans/02-records.md#node-3-4). Go `storage_passive_test.go`, Rust `storage_corpus/passive.rs`; run Go `^TestStoragePassive` and Rust `storage_corpus passive_`.

## 4. Chunk and companion aggregates

- [ ] 4.1 [Preflight exact chunk logical length](plans/03-aggregates.md#node-4-1). Rust `src/{chunk,lib}.rs`, `tests/chunk_codec.rs`; run Rust `--test chunk_codec logical_`.
- [ ] 4.2 [Add caller-owned chunk compression and bounded decode](plans/03-aggregates.md#node-4-2). Rust `src/{chunk,lib}.rs`, `tests/chunk_codec.rs`; run Rust `--test chunk_codec context_`.
- [ ] 4.3a [Execute chunk v1–v4 migrations](plans/05-dispatch-slices.md#node-4-3a). Package-local Go builder, four decode routes, nonempty Rust gate.
- [ ] 4.3b [Execute chunk v5–v9 and current output](plans/05-dispatch-slices.md#node-4-3b). Add late decode/current encode routes and logical-byte evidence.
- [ ] 4.3c [Close chunk cross-decode and rejection matrix](plans/05-dispatch-slices.md#node-4-3c). Go decode a Rust-produced frame, then run bounded corruption cases.
- [ ] 4.4 [Preflight companion length and canonical index plan](plans/03-aggregates.md#node-4-4). Rust `src/{companion,lib}.rs`, `tests/companion_buffer.rs`; run Rust `--test companion_buffer preflight_`.
- [ ] 4.5 [Add companion atomic caller-buffer writer](plans/03-aggregates.md#node-4-5). Rust `src/{companion,lib}.rs`, `tests/companion_buffer.rs`; run Rust `--test companion_buffer writer_`.
- [ ] 4.6a [Execute companion v1–v4 history](plans/05-dispatch-slices.md#node-4-6a). Prove legacy owner, task, FIFO and summary without v5 bootstrap.
- [ ] 4.6b [Execute companion v5 and adversarial closure](plans/05-dispatch-slices.md#node-4-6b). Prove exact current bytes, bounds, membership and corruption.

## 5. Whole-storage acceptance

- [ ] 5.1 [Prove seven-family zero-gap route and mutation closure](plans/04-closure.md#node-5-1). Go `storage_coverage_test.go`, Rust `storage_corpus.rs`, reviewed `contracts.json`; run `go test ./packages/tools/cmd/runtime-oracle -run '^TestStorageCorpus' -count=1` and Rust `--test storage_corpus`.
- [ ] 5.2 [Run complete stage gates and record rollback evidence](plans/04-closure.md#node-5-2). Run format, `make rust-check`, `make dev-check`, `make test-race`, audit and strict OpenSpec; bind result SHA/case counts/source revision in `ledger.md`.
