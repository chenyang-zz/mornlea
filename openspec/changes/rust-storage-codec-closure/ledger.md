# Planning ledger — Rust storage codec closure

## Scope and baseline

- Planning baseline: `d929eb9560bb310b249f443e48c404aee1f6ff6e` on `dev`, initially clean. The accepted safety evidence is `ba0165896b78daaa2a50c03364dc4b912382d117`; the integration baseline after specification sync and archive is `eb04eacf7f338bc11aa3b0fc606cea5267dc0c12`. Begin implementation from that archive baseline or a descendant, and use it as the `<accepted-safety-sha>` for later source and fixture diff gates. The completed safety change is [archived](../archive/2026-09-24-rust-storage-safety-repairs/ledger.md).
- The completed region-bank format implementation is read-only here. All seven `save.*` families currently have zero cases in `testdata/runtime-migration/contracts.json`; existing Rust fixture tests do not make the frozen corpus complete. F2 remains blocked on the later integrated F1, numerical/pathfinding and zero-gap gates.
- No protocol, save, ABI, benchmark or region-format version changes are planned. No production Go codec, committed source fixture or live world write is authorized by this change.

## Design and review rulings

- Keep existing raw storage DTOs and add exact caller-buffer writers; `OutputTooSmall` already exists from the region successor. Noncompressed writers validate and size before touching a destination. A reusable chunk context owns compressor, decompressor and scratch for one caller; complete logical and two-way decode comparison replaces compressed-byte equality.
- Metadata's Go pure codec is package-private, so its producer is a package-local `_test.go`; the same is true of historical chunk builders. These producers use a guarded standalone external exporter and a uniform `selection.json`; they cannot call the runtime-oracle test helper across packages. Other pure codec producers stay in runtime-oracle `_test.go` files, whose test-only imports are allowed by audit.
- The runtime-oracle production inventory remains stdlib-only. Node 1.1 owns the exact `CaseSpec.arguments` structure and test-only `storagedef` audit allowance. Codec-dependent version and capacity checks belong to each Go producer and Rust dispatcher, not the structural inventory validator.
- The shared Rust corpus loader has a closed consumer enum. Node 1.2 adds `mornlea_storage` before any storage manifest case, so unrelated domain/protocol/engine tests continue to load. Every storage input, parameter, operation, error class and full-value digest follows [the frozen corpus contract](plans/00-corpus-contract.md).
- Candidate assets remain external until controller review and exclusive copy. A zero-case Rust filter before integration is expected to fail; the nonempty green run occurs after integration. Per-family commits keep the existing equal source-revision fields and update exact `Family.Sources` hashes. Node 5.1 captures the last family commit SHA and refreshes both revision fields once, following the archived protocol corpus pattern.
- Impossible test requests were removed: constructed player payload and valid chunk logical data cannot exceed their large decode caps under fixed shape; those caps are exercised through malformed declared lengths. A fully legal companion v5 aggregate reaches exactly its 393,904-byte maximum. Boolean byte 2 cases are binary decode mutations, not impossible constructed `bool` values. Hostile v1 has no rewrite flag.
- A direct expected-JSON adapter is rejected through same-route valid-input swaps for every family, using the real dispatcher and unchanged expectation. This checks behavior without a brittle source scanner. Stage gates compare production Go source and original fixture bytes against the accepted safety baseline while allowing reviewed new test producers and corpus assets.

## Worker dispatch and ownership

The controller used Superpowers brainstorming and writing-plans with the project implementation-orchestration and architecture skills. Read-only agents independently checked code, source provenance, corpus feasibility and task readiness; the controller settled their findings. [Tasks](tasks.md) are the sole checkbox source; broad player, chunk, companion and region matrices were split into bounded dispatch slices in [the worker packets](plans/05-dispatch-slices.md). Each slice has a Go source candidate, Rust route, exact file ownership, prereq, expected red/green outcome, integration gate, commit and rollback. Shared `inventory.go`, `discover.go`, Rust loader/dispatcher, manifest, assets and task ledger are controller-serialized. No parallel Worker edits the same family producer. A contract conflict returns to the controller with input bytes and both observed outcomes; it does not trigger a worker-invented policy. Architecture skill: no change; this planning-only round has no new verified cross-task rule.

**Requirement coverage:** supported historical versions and migration evidence → 1.3–1.4b, 2.2a–2.2d, 2.4, 3.2/3.4, 4.3a–4.3c and 4.6a–4.6b; atomic noncompressed writers → 2.1/2.3/3.1/3.3/4.4/4.5; raw metadata → 2.3/2.4; bounded chunk and two-way decode → 4.1–4.3c; companion no invented v5 bootstrap → 4.6a/b; region selection → 1.3–1.4b; source-bound zero-gap closure → 1.1/1.2/5.1/5.2. Producer and consumer routes use the exact family/version/operation matrix in the corpus contract; shared integration proceeds serially, so the dependency graph is acyclic.

## Planning verification

- `openspec validate rust-storage-codec-closure --strict --no-interactive`: valid.
- `openspec validate --all --strict --no-interactive`: 128 passed, 0 failed, including both new storage changes.
- Task-link/anchor and whitespace scan: 27/27 linked nodes resolve uniquely; no trailing whitespace, tab or placeholder. Local Markdown link scan: 11 files, 0 broken links.
- Current frozen corpus inspection: each of seven `save.*` families has 0 declared and 0 actual cases; the plan does not claim any implementation coverage.
- `git diff --check`: exit 0 before staging; staged diff is checked again before the planning commit.

This codec change claims no implementation or runtime gate. At implementation closure, node 5.2 records actual discovered/executed case counts, all gate outputs, source/result SHAs, review and rollback evidence.

## 2026-09-24 — SDD execution: node 1.1 closed

- Orchestration: `superpowers-implementer` + `superpowers-reviewer` via Cloud bridge (`.cursor/rules/cloud-project-subagents.mdc`); reviewer model `grok-4.7-xhigh`, implementer `composer-2.5`.
- Commits: `b4991d0b` initial selection infra; `0b4504a5` test-only merge/read; `526c0c1c` candidate symlink walk aligned with `exportGeneratedAssets`.
- Review: first pass found production `cmd/runtime-oracle` build break (merge in prod calling test-only `validProducerIDs`); fixed by moving read/merge to `storage_selection_test.go`. Re-review approved after symlink walk fix.
- Gates: `go build ./packages/tools/cmd/runtime-oracle`; `go test ./packages/tools/cmd/runtime-oracle -run '^TestStorageSelection' -count=1`; `go test ./packages/audit -run '^TestRuntimeOracleInternalDependencies$' -count=1`.
- Node 1.2 dispatched in background (`superpowers-implementer`).
- Architecture skill: no change.

## 2026-09-24 — SDD execution: node 1.2 closed

- Commits: `be5cc843` consumer + digest; `a6817f0a` empty-selection fail-closed; `8a1d9a43` ledger validation record.
- Review fix: full `cargo test -p mornlea_storage --test storage_corpus` exits non-zero (`storage_corpus_rejects_empty_selection`); filtered `value_digest::` 2/2 pass. Matches protocol empty-selection polarity until node 1.3 registers routes.
- Gates: Go `TestStorageValueV1CrossLanguageGoldenTree`; Rust downstream `mornlea_domain`/`mornlea_protocol`/`mornlea_engine` tests per implementer report.
- Architecture skill: no change.

## 2026-09-24 — SDD execution: node 1.3 closed

- Commits: `4f4e24aa` region seed producer; `45924f37` controller integration of `save.region/1/decode` and `save.region/1/encode`; `2d2eb04b` removed the tracked-manifest writer and recorded the consumer in the runtime-oracle guide; `54da4405` retargeted the three baseline pins.
- Split check: `4f4e24aa` adds the producer, `region.rs`, the runner helper, and a `storage_corpus.rs` module declaration. Registry, Discover, assets, and dispatcher delegation landed in `45924f37`.
- Re-review of `45924f37..54da4405` approved. No Critical or Important findings remain.
- Minor findings held for the final review: helper file outside the named set; duplicate export test; encode arm returns before a non-ok category compare; superblock encode has no corpus row; duplicated route tables; comments that name the node; stale empty-selection test name; malformed-hash test accepts any error; export-unset test counts only the repository root; save-case pin matches ID before family; protocol totals comment omits the save count.
- Gates: `go test ./packages/tools/cmd/runtime-oracle -race -count=1` pass after the pin update; `go test ./packages/audit -count=1` pass; `cargo test -p mornlea_storage --test storage_corpus --locked` 8/8 pass; downstream `mornlea_domain` / `mornlea_protocol` / `mornlea_engine` tests pass.
- `save.region` now carries the four seed cases. The other six `save.*` families remain at zero cases.
- Architecture skill: no change.

## 2026-09-24 — SDD execution: node 1.4a closed

- Commits: `58a2669e` order producer and Rust `select_region_bank` execution; `f88b9d31` controller integration of `save.region/1/order` and the six reviewed assets.
- Split check: `58a2669e` touches only `storage_region_test.go` and `storage_corpus/region.rs`. Registry, dispatcher, manifest, assets, and baseline pins landed in `f88b9d31`.
- Review of `5a5791b4..f88b9d31` approved. No Critical or Important findings.
- Minor findings held for the final review: route-table comments still describe the seed; `region_corpus_executes_integrated_seed_cases` now also executes order rows.
- `save.region` now carries the four seed cases plus six order cases. The other six `save.*` families remain at zero cases. `source_revision` stays `b6043f004176055a2e39a98508b662691c3e4ef7`.
- Gates: `go test ./packages/tools/cmd/runtime-oracle -race -count=1` pass; Rust `region_order_` executes the six integrated cases.
- Architecture skill: no change.

## 2026-09-24 — SDD execution: node 1.4b closed

- Commits: `4b384054` region corruption producer and Rust decode execution; `57553b0d` controller integration of the sixteen reviewed decode assets.
- Split check: `4b384054` touches only `storage_region_test.go` and `storage_corpus/region.rs`. Manifest, assets, baseline pins, and the `future_version` category landed in `57553b0d`. No new route. Production codecs unchanged.
- Review of `69de5fb1..57553b0d` approved. No Critical or Important findings.
- Minor findings held for the final review: `protocol_frame_test.go` comment still says two sentinels; `TestStorageRegionCorruptOrderBankSwapFailsStaleDigest` discards `mustRepoRoot`; the corruption filter is broader than the sixteen labels.
- `save.region` now carries 26 cases (4 seed + 6 order + 16 corruption). The other six `save.*` families remain at zero cases. `source_revision` stays `b6043f004176055a2e39a98508b662691c3e4ef7`.
- Gates: `go test ./packages/tools/cmd/runtime-oracle -race -count=1` pass; Rust `region_corrupt_` executes the sixteen integrated cases; controller confirmation after review: `runtime_contract region_`, full `storage_corpus`, `mornlea_domain`/`mornlea_protocol`/`mornlea_engine` tests (134 passed), and `go test ./packages/audit -count=1` pass.
- Architecture skill: no change.

## 2026-09-24 — SDD execution: node 2.1 closed

- Commit: `99465548` adds `player_encoded_len`, `encode_player_into`, and `SliceWriter`. `encode_player` allocates that length and calls the caller-buffer writer.
- Review of `2571b26a..99465548` approved. No Critical or Important findings. The reported clippy failure is `clippy::collapsible_if` in `tests/storage_corpus/region.rs`, which this commit does not touch.
- Minor findings held for the final review: the payload-cap assertion only excludes `PLAYER_MAX_PAYLOAD`; the `AGENTS.md` boundary note names `player_encoded_len` as the writer; `encoded_len` and `encode_into` repeat the preflight; the absent-respawn branch dropped the residue comment.
- Gates reported by the implementer: `player_buffer` 8/8, `runtime_contract player_` 11/11. No save-family case count change. `source_revision` stays `b6043f004176055a2e39a98508b662691c3e4ef7`.
- Architecture skill: no change.

## 2026-09-24 — SDD execution: node 2.2a closed

- Commits: `e585e87e` current player producer and Rust module; `8d010bf5` dirty absent-respawn input, encode snapshot, and `OutputTooSmall` field check; `05075af2` integration of seven `save.player` cases; `5c907fd9` unregistered-route specimen moved to `save.player/9/order`.
- Split check: `e585e87e` and `8d010bf5` touch only `storage_player_test.go` and `storage_corpus/player.rs`. Registry, dispatcher, manifest, and assets landed in `05075af2`. `5c907fd9` touches only `storage_corpus.rs`.
- Reviews of the producer, the fix, the integration, and the route fix approved. The integration Important finding is closed by `5c907fd9`. No remaining Critical or Important findings.
- Minor findings held for the final review: export-unset compares the repository root entry count; encode re-decode checks only `needs_rewrite`; the encode success arm returns when `kind` is `error`; `05075af2` carries a `Co-authored-by` trailer; `contracts.json` has no trailing newline; the storage-vocabulary comment still says two sentinels after `output_too_small` was added; the route-table comment still describes only region.
- `save.player` has 7 cases on `/9/decode` and `/9/encode`. `save.region` stays at 26. The other five `save.*` families remain at zero. `source_revision` stays `b6043f004176055a2e39a98508b662691c3e4ef7`.
- Gates reported for the integration commit: `go test ./packages/tools/cmd/runtime-oracle -count=1` pass; Rust `storage_corpus` 13/13 including nonempty `player_`; `runtime_contract player_` 11/11. The route fix reran `storage_corpus` 13/13.
- Architecture skill: no change.

## 2026-09-24 — SDD execution: node 2.2b closed

- Commits: `533c575e` v1–v4 producer and Rust executor; `e00ebab2` pinned export no longer deletes the candidate; `46116e5f` integration of nine early player cases and decode routes 1–4.
- Split check: `533c575e` and `e00ebab2` touch only the player producer and `storage_corpus/player.rs`. Registry, dispatcher routes, manifest, and assets landed in `46116e5f`.
- Reviews of the producer, the export fix, and the integration approved. No remaining Critical or Important findings.
- Minor findings held for the final review: unused v9 decode route on the early selection; dead `playerManifest`; Go negative cases do not pin `corrupt` / `future_version`; `Co-authored-by` trailers on `533c575e`, `e00ebab2`, and `46116e5f`. The 2.2a note that `contracts.json` lacked a trailing newline is closed: `46116e5f` ends the file with a newline.
- `save.player` now has 16 cases. `save.region` stays at 26. The other five `save.*` families remain at zero. `source_revision` stays `b6043f004176055a2e39a98508b662691c3e4ef7`.
- Gates reported for the integration commit: `go test ./packages/tools/cmd/runtime-oracle -count=1` pass; Rust `storage_corpus player_` 6/6 including nonempty `player_legacy_early_`; `runtime_contract player_` 11/11.
- Architecture skill: no change.

## 2026-09-24 — SDD execution: node 2.2c closed

- Commits: `ea757ad3` v5–v8 producer and Rust executor; `80150f48` v5 health pinned at 13; `39574c0c` integration of nine late player cases and decode routes 5–8.
- Split check: `ea757ad3` and `80150f48` touch only the player producer and `storage_corpus/player.rs`. Registry, dispatcher routes, manifest, and assets landed in `39574c0c`.
- Reviews of the producer, the health pin, and the integration approved. The health-pin review's commit-trailer finding is the same hook-injected `Co-authored-by` already held; `80150f48` was pushed, so the message was not rewritten.
- Minor findings held for the final review: v8 pin omits the fixture respawn position and dimension; Rust v6 success path does not assert `needs_rewrite`; the v4 and v8 re-encode helpers are duplicated; the late export test does not list the nine ids.
- `save.player` now has 25 cases. `save.region` stays at 26. The other five `save.*` families remain at zero. `source_revision` stays `b6043f004176055a2e39a98508b662691c3e4ef7`.
- Gates reported for the integration commit: `go test ./packages/tools/cmd/runtime-oracle -count=1` pass; Rust `storage_corpus player_` 14/14 including nonempty `player_legacy_late_`; `runtime_contract player_` 11/11.
- Architecture skill: no change.

## 2026-09-24 — SDD execution: node 2.2d closed

- Commits: `4a81b8a6` adversarial player producer and Rust executor; `e78f3e0c` integration of twelve `save.player/9/decode` cases. No new route.
- Split check: `4a81b8a6` touches only `storage_player_test.go` and `storage_corpus/player.rs`. Manifest, assets, and the nonempty gate landed in `e78f3e0c`.
- Reviews of the producer and the integration approved. No Critical or Important findings.
- Minor findings held for the final review: schema and declared-length wires keep a stale CRC; `player_adversarial_case` uses `starts_with` for one id.
- `save.player` now has 37 cases. `save.region` stays at 26. The other five `save.*` families remain at zero. `source_revision` stays `b6043f004176055a2e39a98508b662691c3e4ef7`.
- Gates reported for the integration commit: `go test ./packages/tools/cmd/runtime-oracle -count=1` pass; Rust `storage_corpus player_` 19/19 including nonempty `player_adversarial_`; `runtime_contract player_` 11/11.
- Architecture skill: no change.

## 2026-09-24 — SDD execution: node 2.3 closed

- Commits: `c6a6104a` raw weather preservation and the 78-byte caller-buffer writer; `bba927e0` renames the contract test so it no longer claims weather rejection.
- Review of `a0b97020..c6a6104a` found one Important naming defect, closed by `bba927e0`. The rename re-review approved. No remaining Critical or Important findings.
- Minor finding held for the final review: the metadata boundary note says every invalid input is corruption, while a future schema returns `FutureVersion`.
- No save-family case count change. Schema stays v6. `source_revision` stays `b6043f004176055a2e39a98508b662691c3e4ef7`.
- Gates reported by the implementer: `metadata_buffer` 10/10; `runtime_contract metadata_` 6/6 after the rename; `go test ./packages/server/storage -run '^TestMetadata'` pass.
- Architecture skill: no change.

## Implementation evidence

### Node 1.1 — source-bound save selections

- Added `CaseSpec.arguments`, save-case structural checks in `validateCaseSpecConsumer`, `validateStorageArguments`, `StorageSelection`, `readStorageSelection`, and `mergeStorageSelections` in runtime-oracle (production stdlib-only).
- Added `storage_manifest_test.go` with `^TestStorageSelection` coverage for argument validation, candidate read/merge rejections, merge preservation, export-unset no-op, and test-only `storagedef` import.
- Extended `validProducerIDs` for storage producers; audit whitelists `packages/server/storage/storagedef` for oracle `_test.go` only.
- Validation:
  - `go test ./packages/tools/cmd/runtime-oracle -run '^TestStorageSelection' -count=1` — pass
  - `go test ./packages/tools/cmd/runtime-oracle -count=1` — pass
  - `go test ./packages/audit -run '^TestRuntimeOracleInternalDependencies$' -count=1` — pass

### Node 1.2 — Rust storage corpus consumer

- Added `CorpusConsumer::Storage` / `mornlea_storage` to `packages/engine/tests/runtime_corpus.rs` and the closed consumer map in `packages/engine/tests/AGENTS.md`.
- Added `packages/engine/crates/mornlea_storage/tests/storage_corpus.rs` with an empty route table, uncaught zero-case rejection via `load_cases_for_consumer` + `execute_storage_selection`, unregistered-route rejection, and manifest loader acceptance for `mornlea_storage`.
- Added `StorageValueV1` encoder and cross-language golden tree digest in `tests/storage_corpus/value_digest.rs`; Go oracle `TestStorageValueV1CrossLanguageGoldenTree` in `storage_value_digest_test.go` pins `sha256:9afe6b3bc14a7b3daa74b6be41bc34d2357ac15a874c92a20a8f84c3bf66206b`.
- Review fix (`a6817f0a`): removed green tests that treated empty route table / caught empty-selection panic as success; `storage_corpus_rejects_empty_selection` now fails the suite when the manifest has no storage cases (matches protocol empty-selection polarity). Task 1.2 rechecked pending controller acceptance after fix.
- Validation (post-fix):
  - `rustup run 1.97.1 cargo test --manifest-path packages/engine/Cargo.toml -p mornlea_storage --test storage_corpus --locked value_digest::` — 2/2 pass
  - `rustup run 1.97.1 cargo test --manifest-path packages/engine/Cargo.toml -p mornlea_storage --test storage_corpus --locked` — exit 101 (5 pass, 1 fail: `storage_corpus_rejects_empty_selection` panics `storage corpus selection executed zero cases`)
  - `rustup run 1.97.1 cargo test --manifest-path packages/engine/Cargo.toml -p mornlea_domain -p mornlea_protocol -p mornlea_engine --tests --locked` — pass (prior node 1.2 gate)
  - `go test ./packages/tools/cmd/runtime-oracle -run TestStorageValueV1CrossLanguageGoldenTree -count=1` — pass (prior node 1.2 gate)

## Accepted safety handoff

- The safety change closed 8/8 tasks after independent whole-change review and macOS acceptance: 104/104 storage tests, `make rust-check`, `make dev-check`, `make test-race`, independent audit, and 128/128 strict OpenSpec items before archive. Neither production Go storage code nor existing migration fixtures changed. Four safety requirements and 11 scenarios were synced into `openspec/specs/rust-runtime-foundation/spec.md`; strict validation after archive passed 127/127 items.
- The review exposed a distinction that codec workers must preserve: direct historical logical chunk encoding admits exactly reserializable raw pre-v5 multi-item tool drops, while current-value preflight and envelope encoding reject values that an older target schema would omit or alter. Nodes 4.1 and 4.2 now specify one complete current aggregate/schema validation per encode and a private already-validated appender. Node 4.4 starts from a borrowed-field safety validator that rejects oversized lifecycle summaries before copying, but still needs an exact allocation-free length preflight.
- This handoff changes prerequisite evidence and the linked aggregate packets; it does not complete any codec task or populate the seven zero-case `save.*` families. Architecture skill: no change.
