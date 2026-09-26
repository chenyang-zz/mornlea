# Numerical corpus and closeout file packets

**Goal:** execute, source-bind and reconcile all ten ABI v11 numerical routes and the independent Go/Rust pathfinding route. **Architecture:** a controller-owned corpus loader calls actual exported Rust ABI symbols for legacy families and the typed Rust pathfinder for `kernel.pathfind`; Go produces observations, with package-local raw-status helpers where its public bridge panics. **Tech stack:** runtime oracle, Rust integration tests, strict OpenSpec gates. **Spec:** [delta](../specs/rust-runtime-foundation/spec.md), [design](../design.md), [case and error contract](../worker-briefs.md#node-5-14). `tasks.md` alone records status. A list-only test or zero-case inventory is not acceptance.

<a id="node-3-12"></a>
## Node 3.12 — register and execute all eleven routes

**Prerequisite:** reviewed 2.1–2.13 providers, 3.1 repair and 3.2–3.11 adapters. **Exact editable files and responsibilities:**

| File | Change |
| --- | --- |
| `packages/tools/cmd/runtime-oracle/kernel_test.go` | Check all eleven case IDs/categories/source hashes; merge reviewed family valid drafts with raw-status drafts by unique case ID; assert each route has executed success, error and boundary. |
| `packages/tools/cmd/runtime-oracle/inventory.go` and `inventory_test.go` | Add the closed eleven `{FamilyID,Version,Operation:"kernel"}` entries in `BaselineConsumerRegistry()` and its exact-map test; reject an absent or extra route. |
| `packages/tools/cmd/runtime-oracle/protocol_frame_test.go` | Extend the frozen outcome vocabulary only: add a `corpusKernelCategories` set with the nine ABI status names plus the six pathfind categories and accept it in the whole-tree check; existing structural/admission/storage sets untouched. |
| `packages/tools/cmd/runtime-oracle/protocol_coverage_test.go` | Accept the `kernel.` slice with its own counted total plus reconcile, mirroring the existing slices; existing four totals unchanged. |
| `packages/tools/cmd/runtime-oracle/AGENTS.md` | Explain test-only producer ownership, external export and focused commands. |
| `packages/shared/nativeabi/kernel_oracle_test.go` | Add `TestKernelABIRawOracle` in the existing package, calling package-private status helpers; assert status/0xa5 canaries locally and export malformed cases only with `RUNTIME_ORACLE_EXPORT_DIR`, without new production cgo/API. |
| `packages/shared/nativeabi/AGENTS.md` | Describe package-local raw oracle boundary and no tracked asset write. |
| `packages/engine/crates/mornlea_engine/Cargo.toml` | Add only test dependencies needed by the engine corpus: `serde = "=1.0.229"`, `serde_json = "=1.0.151"`, `sha2 = "0.10"`. |
| `packages/engine/Cargo.lock` | Edit only if dependency resolution under `--locked` requires it; record the reason. |
| `packages/engine/crates/mornlea_engine/tests/native_contract.rs` and `tests/numerical_migration.rs` | Register all family topic files once and assert nonempty discovery; this controller-owned edit reconciles any registration missed at 1.2. |
| `packages/engine/tests/runtime_corpus.rs` | Add `CorpusConsumer::Engine` string `mornlea_engine`, case/argument validation and binary/JSON execution dispatch; unknown route, argument, category or unexecuted case is failure. |
| `packages/engine/tests/AGENTS.md` | Document the engine consumer, fixture provenance and focused gate. |
| `testdata/runtime-migration/contracts.json` and new kernel case assets | Add actual reviewed cases, expected JSON and all source/input SHA-256; never write a placeholder or zero-case family. |

**Read-only:** all production Go bridges, accepted Rust providers/adapters, canonical specs and other consumers except the two shared corpus gates this node extends per the rows above. Source-bound family producer fragments created by 2.x nodes are inputs outside the repo, not automatically accepted assets.

**Case encoding:** ten `kernel.mornlea_*` v11 cases use `input_format:"binary"` containing only an existing raw symbol request. `arguments` explicitly include integer `abi_version`, output capacity in ABI bytes (mesh in `u64` slots), scratch capacity where applicable, and one of `normal|short|null|metadata-alias-input|metadata-alias-output|metadata-alias-scratch`. Initialize aligned output/scratch/metadata arenas to `0xa5`; run the exported Rust symbol; normalize status 0..9 into `ok`, `abi-version`, `invalid-argument`, `input`, `scratch`, `registry`, `emission`, `output-overflow`, `queue-overflow`, `panic`. Include status, applicable count/length/done/cursor digest, a SHA-256 of the **entire** caller-visible output arena and a success-only used-prefix digest. Collision/physics/raycast include ordered `f32` bit hex. Invalid arguments/capacity above harness bounds fail loading rather than silently skip. JSON input is capped at 256 KiB, binary at 4 MiB, manifest at 4 MiB and all cases at 8192.

`kernel.pathfind/1` uses `input_format:"json"`, `operation_kind:"grid"|"search"`, `origin`, `size`, Y-fast `blocks_rle:[[id,count],...]`, `passable_ids`, sorted revision inputs with decimal-string `u64`, and start/goal for search. Check positive RLE counts and exact expanded product `<=131072` before allocation. Normalize exact sorted revisions and ordered waypoints on success, otherwise one of `invalid-grid|invalid-revision|scratch-too-small|unreachable|budget-exceeded|allocation`. Go calls its real `NewPathBlockTable`, `NewPathGrid` and `FindPath`; Rust calls its independently implemented typed API. A copied loaded input must not mutate the owned Rust grid.

**Minimum executed matrix:**

| Route | Success | Failure | Boundary |
| --- | --- | --- | --- |
| `kernel.mornlea_collision_resolve/11` | `floor-wall`: 16 bytes and bits | `invalid-used-nan`: input and canary | `short-output-15`: status 7 and canary |
| `kernel.mornlea_physics_step/11` | `floor-landing`: 32 bytes and bits | `axis-two`: input | `ulp-sweep`: one accepted/two rejected |
| `kernel.mornlea_raycast_batch/11` | `second-batch`: ordered records/cursor | `tampered-cursor`: frozen category | `record-65`: two calls and done |
| `kernel.mornlea_worldgen_chunk/11` | `seed-zero-chunk-zero`: 196608-byte digest | `duplicate-material`: input | `signed-extreme`: release digest |
| `kernel.mornlea_worldgen_probe/11` | `height-terrain-base`: three records | `mode-three`: input | `query-64-65`: both observations |
| `kernel.mornlea_tree_blocks/11` | `oak-order`: count/order | `root-y-minus-65`: input | `short-by-one`: status 7/canary |
| `kernel.mornlea_lod_shell/11` | `step-two`: ordered quads | `invalid-step`: input | `exact-needed-retry`: status 7/metadata then success |
| `kernel.mornlea_fluid_eval_batch/11` | `vertical-priority`: slot order | `malformed-length`: input | `count-4097`: ABI success/digest |
| `kernel.mornlea_fluid_rescan/11` | `interior-source`: summary/positions | `outer-missing-halo`: status 9/canary | `budget-one`: continuation and retry |
| `kernel.mornlea_mesh_section/11` | `six-faces`: ordered packed quads | `registry-97`: registry | `model-plant-late-overflow`: status 7/canary |
| `kernel.pathfind/1` | `corridor`: waypoints/revisions | `blocked-support`: unreachable | `goal-pop-4096-4097`: success/error |

**Test-first and import steps:**

1. In `kernel_test.go` and `inventory_test.go`, assert exactly eleven nonempty kernel routes, a compiled Rust `mornlea_engine` consumer and at least one actual executed `ok`, `error` and boundary for each; current manifest `cases:null` must make this red. In the Rust loader, reject a dispatch function that merely discovers names or never invokes the operation.
2. Implement guarded raw producer in `nativeabi/kernel_oracle_test.go`. With the export variable unset, it only asserts current ABI statuses/canaries; with a create-exclusive external directory, it emits request/expected assets and selection metadata. Reject repo-contained, symlinked, duplicate or pre-existing output children before opening assets. Merge its drafts with family valid-case drafts by exact ID/source SHA, rejecting duplicates or mismatches.
3. Add the closed registry, Engine loader, test dependencies and guides; dispatch each binary family to its actual exported symbol and JSON pathfind to the typed search. Refresh every affected family `sources[].sha256`, including shared `worldgen.rs`, `ffi.rs`, Go producer/test and pathfind source, and any derived consumer hashes. Do not weaken provenance or add a case without its source.
4. Import reviewed assets into `testdata/runtime-migration/`. Run actual per-family loops with counters **after** the call and comparison, then assert eleven nonempty success/error/boundary counts. Mutate one loaded expected scalar/float bit or path waypoint and prove `assert_normalized` fails; restore the asset and verify its SHA. Run working reconciliation and complete-coverage rejection/acceptance; report source SHA, case IDs, discovered/executed counts and allocation/scratch observations.

**Validate:** `go test ./packages/tools/cmd/runtime-oracle -run 'TestKernelOracle|TestPathfindOracle|TestContractInventory' -count=1`; `go test ./packages/shared/nativeabi -run '^TestKernelABIRawOracle$' -count=1`; `rustup run 1.97.1 cargo test --manifest-path packages/engine/Cargo.toml -p mornlea_engine --test native_contract --locked -- --list` and execution; the same for `--test numerical_migration`; `go test ./packages/shared/nativeabi -count=1`. **Commit:** `test(engine): close native numerical compatibility coverage`. **Rollback:** case assets, manifest, registry, loader, test dependencies and guide changes together if a route remains empty; accepted providers/adapters remain.

<a id="node-4-1"></a>
## Node 4.1 — reconcile review evidence

**Prerequisite:** 3.12 accepted. **Editable:** `openspec/changes/rust-native-numerical-closure/ledger.md`; update a scoped `AGENTS.md` or active plan/spec only if a verified discrepancy requires it, then repeat the affected focused gate. **Read-only review set:** delta spec, all provider/adapter source and comments, corpus assets/manifest, source/derived-consumer inventory, `packages/engine/AGENTS.md`, `packages/engine/tests/AGENTS.md`, and canonical architecture target.

Make an explicit table mapping each delta scenario to executed case ID, actual result SHA, Go producer source SHA, Rust consumer, error/capacity category, ABI v11 verdict and rollback unit. Independently compare every edited source and derived hash with `contracts.json`; check English ownership/lifecycle/failure comments and directory-guide scope. Any missing case, stale hash, unexplained status or non-English new comment returns to its owning node instead of being marked accepted. Review stable cross-task findings for `mornlea-architecture`; if no current code/test-backed reusable decision exists, record `Architecture skill: no change`. Validate `git diff --check`, `go test ./packages/audit -count=1`, and `openspec validate rust-native-numerical-closure --type change --strict --no-interactive`. **Commit:** `docs(openspec): reconcile numerical closure evidence`. **Rollback:** this review/ledger change only.

<a id="node-4-2"></a>
## Node 4.2 — integrated numerical gate

**Prerequisite:** 4.1 accepted and one integrated result SHA. **Editable:** `openspec/changes/rust-native-numerical-closure/ledger.md` for gate evidence and `tasks.md` status only after success. **Read-only:** all numerical production source, ABI and corpus files while the gate runs; if a defect appears, reopen the owning node, fix/test/commit there and restart this gate on the new single SHA.

Record baseline SHA and corpus manifest SHA, then run on that **same** source state: `rustup run 1.97.1 cargo fmt --manifest-path packages/engine/Cargo.toml --all --check`; `make rust-check`; `make dev-check` (six-module Go vet); `make test-race` (six Go modules); `go test ./packages/audit -count=1`; `openspec validate --all --strict --no-interactive`. Record each exact command, exit result, SHA and eleven executed case counts. A failed required gate leaves 4.2 open; timings are informational, real overflow/data-loss/I/O failures are not. Closure supplies evidence to the separate complete F1 zero-gap acceptance; it does not start F2 or switch the default runtime. **Commit:** `docs(openspec): record numerical closure gates`. **Rollback:** only invalid ledger/status evidence, not unrelated accepted provider commits.
