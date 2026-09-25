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

## 2026-09-24 — SDD execution: node 2.4 closed

- Commits: `5e0abae4` metadata producer and Rust module; `25c7ef45` export-guard fix; `91894f49` integration of fifteen `save.world-metadata` cases and decode routes 1..6 plus encode 6.
- Split check: `5e0abae4` and `25c7ef45` touch only `metadata_oracle_test.go` and `storage_corpus/metadata.rs`. Registry, dispatcher, Discover, manifest, and assets landed in `91894f49`.
- Reviews of the producer, the export fix, and the integration approved. The four Important export-guard findings are closed by `25c7ef45`. No remaining Critical or Important findings.
- Minor findings held for the final review: the weather mutation is compared with the v4 digest; the dimension-count comment states both offsets 49 and 53; encode returns on the category string without comparing `needed`, `available`, or `length`.
- `save.world-metadata` has 15 cases. `save.region` stays at 26. `save.player` stays at 37. The other four `save.*` families remain at zero. Save total is 78. `source_revision` stays `b6043f004176055a2e39a98508b662691c3e4ef7`. Schema stays v6.
- Gates reported for the integration commit: `go test ./packages/server/storage -run '^TestMetadataOracle$'` pass; `go test ./packages/tools/cmd/runtime-oracle -count=1` pass; Rust `storage_corpus` 37/37 including nonempty `metadata_`; `runtime_contract metadata_` 6/6.
- Architecture skill: no change.

## 2026-09-24 — SDD execution: node 3.1 closed

- Commits: `98595344` hostile caller-buffer writer; `ec8b8845` rejects a nonzero absent-target id instead of writing zeros.
- Ruling: an absent target is encoded only when its player id is already zero. Encode and decode share that rule. A uniform nonzero id is corrupt, matching Go `validateHostileRecord`. Health 0 stays rejected.
- Review of `6704c75a..98595344` found that repair. The fix re-review of `ec8b8845` approved. No remaining Critical or Important findings.
- Minor findings held for the final review: the CRC patch uses `copy_from_slice` instead of `SliceWriter::patch_u32`; length validation runs more than once per encode.
- No save-family case count change. `source_revision` stays `b6043f004176055a2e39a98508b662691c3e4ef7`.
- Gates reported for the fix: `hostile_buffer` 13/13; `runtime_contract hostile_` 9/9; `go test ./packages/server/storage/hostile -count=1` pass.
- Architecture skill: no change.

## 2026-09-24 — SDD execution: node 3.2 closed

- Commits: `220f7271` hostile v1/v2 producer and Rust executor; `cb017d34` cooldown 20 and short-capacity `needed`/`available`; `37f51f3f` integration of fifty `save.hostile` cases and decode routes 1–2 plus encode route 2.
- Split check: `220f7271` and `cb017d34` touch only `storage_hostile_test.go` and `storage_corpus/hostile.rs`. Registry, dispatcher, Discover, manifest, and assets landed in `37f51f3f`.
- Reviews of the producer, the cooldown pin, and the integration approved. The two Important findings are closed by `cb017d34`. No remaining Critical or Important findings.
- Minor findings held for the final review: the local v2 fixture test executes a digest it just built; a successful encode compares the encoded asset without reading `length`.
- `save.hostile` has 50 cases. `save.region` stays at 26. `save.player` stays at 37. `save.world-metadata` stays at 15. The other three `save.*` families remain at zero. Save total is 128. `source_revision` stays `b6043f004176055a2e39a98508b662691c3e4ef7`.
- Gates reported for the integration commit: `go test ./packages/tools/cmd/runtime-oracle -count=1` pass; Rust `storage_corpus` 44/44 including nonempty `hostile_`; `runtime_contract hostile_` 9/9.
- Architecture skill: no change. The short-capacity field check and the absent-target rule are already in the corpus contract and the hostile writer note.

## 2026-09-24 — SDD execution: node 3.3 closed

- Commits: `4827ffbf` passive caller-buffer writer; `7497d3cc` deletes unused `ByteWriter::zeroes`.
- Review of `817a07b6..4827ffbf` required that deletion. The fix re-review of `7497d3cc` approved. No remaining Critical or Important findings.
- Held for the final review, outside this node: eight pre-existing `storage_corpus` clippy lints (`redundant-field-names`, `collapsible-if`, `needless-return`) in the player, region, metadata, and hostile executors.
- No save-family case count change. Schema stays v1. `source_revision` stays `b6043f004176055a2e39a98508b662691c3e4ef7`.
- Gates: `rustup run 1.97.1 cargo test --manifest-path packages/engine/Cargo.toml -p mornlea_storage --test passive_buffer --locked` 10/10; `cargo clippy -p mornlea_storage --lib --locked -- -D warnings` pass. `runtime_contract passive_` 7/7 and `go test ./packages/server/storage/passive -count=1` were reported by the implementer.
- Architecture skill: no change. The caller-buffer rule is already recorded for the other noncompressed writers.

## 2026-09-24 — SDD execution: node 3.4 closed

- Commits: `307e1f64` passive v1 producer and Rust executor; `1621544d` rejects a shared case id or asset path and checks encode `length`; `363e8261` skips a reviewed pinned export; `03922a38` integration of seventy `save.passive` cases and decode/encode route 1.
- Split check: `307e1f64`, `1621544d`, and `363e8261` touch only `storage_passive_test.go` and `storage_corpus/passive.rs`. Registry, dispatcher, Discover, manifest, and assets landed in `03922a38`.
- Reviews of the producer, both fixes, and the integration approved. The colliding export, the missing encode `length` check, and the fatal pinned-child test are closed. No remaining Critical or Important findings.
- Minor finding held for the final review: `validatePassiveExportCandidates` has no subtest that shares an id or asset path and asserts no child is created.
- `save.passive` has 70 cases. `save.region` stays at 26. `save.player` stays at 37. `save.world-metadata` stays at 15. `save.hostile` stays at 50. Save total is 198. `source_revision` stays `b6043f004176055a2e39a98508b662691c3e4ef7`. Schema stays v1.
- Gates reported for the integration commit: `go test ./packages/tools/cmd/runtime-oracle -count=1` pass; Rust `storage_corpus` 51/51 including nonempty `passive_`; `runtime_contract passive_` 7/7.
- Architecture skill: no change. The external-candidate handoff and the noncompressed writer rule are already recorded.

## 2026-09-24 — SDD execution: node 4.1 closed

- Commit: `cf15c529` adds `chunk_logical_len` and routes `encode_at_schema` through that preflight plus an already-validated appender.
- Review of `7bffedbc..cf15c529` approved. No Critical or Important findings. The Go private logical builder only writes the current schema, and those widths match the preflight. Node 4.3 owns the historical logical-byte oracle.
- Minor findings held for the final review: `chunk_codec` warns that `LAST_BLOCK_INDEX` is unused; `chunk_logical_len` carries the serialization doc comment that belongs on `encode_logical`.
- No save-family case count change. `source_revision` stays `b6043f004176055a2e39a98508b662691c3e4ef7`.
- Gates reported by the implementer: `chunk_codec logical_` 13/13; `runtime_contract chunk_` 10/10; `cargo clippy -p mornlea_storage --lib -- -D warnings` pass.
- Architecture skill: no change. The current-value preflight versus raw historical admission split is already in the design.

## 2026-09-24 — SDD execution: node 4.2 closed

- Commits: `5aaa54cd` reusable `ChunkCodec`; `bf3415d6` shares the envelope parser; `0cd29a78` returns the helper's decompressed bytes.
- Review of `220b0942..5aaa54cd` required one parser. The first fix still returned an empty `LogicalPayload.bytes`. The second fix re-review of `0cd29a78` approved. No remaining Critical or Important findings. One caller-thread bulk context satisfies the one-worker rule because `NbWorkers(1)` is unsupported without `zstdmt`.
- Minor findings held for the final review: `AGENTS.md` dropped the `chunk_logical_len` sentence when the context clause was added; recovery does not decode a valid frame after every named failure; scratch reuse still allocates a fresh logical buffer.
- No save-family case count change. `source_revision` stays `b6043f004176055a2e39a98508b662691c3e4ef7`.
- Gates reported for the return-shape fix: `chunk_codec context_` 6/6; `logical_` 13/13; `runtime_contract chunk_` 10/10. `make rust` passed on `5aaa54cd`.
- Architecture skill: no change. The reusable context and the single envelope parser are local to this codec.

## 2026-09-25 — SDD execution: node 4.3a closed

- Commits: `ff43a5a3` early chunk producer and Rust executor; `82e45dfc` temp export root; `a6bd9815` v4 tool-split pin in the test only; `416a0f75` integration of eight `save.chunk` decode cases and routes 1–4.
- The first pinned selection hashed `ff43a5a3` and was not imported. The replacement at `/tmp/runtime-oracle-chunk-4.3a-fix` was pretty-printed after export. The approved candidate is the compact 5640-byte tree at `/tmp/runtime-oracle-chunk-4.3a-fix2/chunk/migration/`, byte-identical to a fresh exporter run of `a6bd9815`.
- Reviews of the producer, the tool pin, the compact handoff, and the integration approved. No remaining Critical or Important findings.
- Minor findings held for the final review: v3 does not assert empty chests; the Go digest mutation shares the `*world.Chunk` pointer; the v2 invalid id is named `corrupt-crc` for a compressed-frame byte; `chunkPinnedExportDir` is unused; the exporter refusals have no subtest; the tool pin does not check `Active` or `Generation`.
- `save.chunk` has 8 cases. `save.region` stays at 26. `save.player` stays at 37. `save.world-metadata` stays at 15. `save.hostile` stays at 50. `save.passive` stays at 70. Save total is 206. `source_revision` stays `b6043f004176055a2e39a98508b662691c3e4ef7`.
- Gates reported for the integration commit: `go test ./packages/tools/cmd/runtime-oracle -count=1` pass; Rust `storage_corpus` 57/57 including nonempty `chunk_legacy_early_`; `runtime_contract chunk_` 10/10.
- Architecture skill: no change. The external compact-export handoff is already the corpus-contract rule.

## 2026-09-25 — SDD execution: node 4.3b closed

- Commits: `e6f5de7d` late and current chunk producers; `42bf54fb` v9 registry pin; `16ec27d2` integration of thirteen `save.chunk` cases, decode routes 5–9, and encode route 9.
- Approved candidates: compact `/tmp/runtime-oracle-chunk-4.3b-late-fix/chunk/migration/` (10 decode cases) and `/tmp/runtime-oracle-chunk-4.3b-current-fix/runtime-oracle/storage-chunk/` (2 decode cases and 1 logical encode). The non-fix trees were not imported.
- Reviews of the producer, the pin, and the integration approved. No remaining Critical or Important findings.
- Minor findings held for the final review: v5 drop-slot equality is structural zeros; `chunk_current_routes_recognize_registered_paths` does not call `execute_chunk_encode`; `storage_manifest_test.go` aligns one new `true` at a different column.
- `save.chunk` has 21 cases. `save.region` stays at 26. `save.player` stays at 37. `save.world-metadata` stays at 15. `save.hostile` stays at 50. `save.passive` stays at 70. Save total is 219. `source_revision` stays `b6043f004176055a2e39a98508b662691c3e4ef7`.
- Gates reported for the integration commit: `go test ./packages/tools/cmd/runtime-oracle -count=1` pass; Rust `storage_corpus` 63/63 including nonempty `chunk_legacy_late_` and `chunk_current_`; `runtime_contract chunk_` 10/10. The reviewer did not re-run those commands.
- Architecture skill: no change. The external compact-export handoff is already the corpus-contract rule.

## 2026-09-25 — SDD execution: node 4.3c closed

- Commits: `91de8daa` adversarial and cross producers; `00c408d7` cross-export, repo exclusion, and recovery decode; `84be0f19` integration of fourteen `save.chunk` decode cases.
- Approved candidates: compact `/tmp/runtime-oracle-chunk-4.3c-adversarial-fix/chunk/migration/` (13 cases) and `/tmp/runtime-oracle-chunk-4.3c-cross-fix/chunk/migration/` (one Go decode of the 180-byte Rust v9 frame). The non-fix trees were not imported.
- Reviews of the producer, the export fix, and the integration approved. No remaining Critical or Important findings.
- Minor findings held for the final review: the external cross selection lists decode routes 9 through 5; `LAST_BLOCK_INDEX` still warns in `chunk_codec`; the frame-export ancestor walk is not executed when the export variable is unset.
- `save.chunk` has 35 cases. `save.region` stays at 26. `save.player` stays at 37. `save.world-metadata` stays at 15. `save.hostile` stays at 50. `save.passive` stays at 70. Save total is 233. `source_revision` stays `b6043f004176055a2e39a98508b662691c3e4ef7`. Decode routes stay 1–9 and encode route 9.
- Gates reported for the integration commit: `go test ./packages/tools/cmd/runtime-oracle -count=1` pass; Rust `storage_corpus` 69/69 including nonempty `chunk_adversarial_` and `chunk_cross_`; `runtime_contract chunk_` 10/10. The integration reviewer compared manifest objects and asset bytes and did not re-run those commands.
- Architecture skill: no change. The chunk gate partition and the Rust-to-Go frame handoff are already in the corpus contract.

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
