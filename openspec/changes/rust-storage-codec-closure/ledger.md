# Planning ledger — Rust storage codec closure

## Scope and baseline

- Planning baseline: `d929eb9560bb310b249f443e48c404aee1f6ff6e` on `dev`, initially clean. This change is downstream of the accepted `rust-storage-safety-repairs` result SHA, which does not yet exist; implementation must capture it before node 1.1.
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

No implementation or runtime gate is claimed. At implementation closure, node 5.2 records actual discovered/executed case counts, all gate outputs, source/result SHAs, review and rollback evidence.
