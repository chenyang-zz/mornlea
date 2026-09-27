# Final F1 acceptance implementation plan

> For agentic workers: use the project-selected Superpowers execution method, test-first and scoped commits. Only `tasks.md` owns checkbox/status state.

**Goal:** Supply complete, honest prerequisite evidence before F2/F3 start.

**Architecture:** Retain the actual external Go authority producer and existing Rust contract owner. Add an observed rejection and a strict set-complete gate, then serialize corpus publication and full consumer acceptance.

**Tech Stack:** Go 1.26, Rust 1.97.1, existing runtime-oracle/frozen corpus.

**Spec:** [design](../design.md), [behavioral specification](../specs/rust-runtime-foundation-acceptance/spec.md).

## Global constraints

- Preserve protocol v45, player/chunk v9, metadata v6, companions v5, hostiles v2, passives v1, engine ABI v11, renderer ABI v19, scenario v23 and every unrelated frozen case.
- Production runtime, CLI flag set and archived changes are read-only; normal tests never rewrite tracked evidence. No coverage exemption or false Rust-authority label.
- Baseline `974458f0`; numerical implementation `9b843bbc`. Accepted successor SHA is recorded only after actual gates.
- Existing stable `ReconcileComplete` and `order_commands` need no separate contract landing. The shared manifest has one serial editor. The small prerequisite chain is serial; parallel future workers remain blocked.

## Review focus

1. Generic rejection test exits zero while full reconciliation fails: node 1.1 asserts no error and exact covered set.
2. Rejection expected values fabricated from input: 1.2 executes Engine.Step and hashes before/after state.
3. Rust sorter falsely claiming world effects: 1.3 preserves the external owner and executes pure Rust tests separately.
4. Source refresh loses unrelated cases: 1.3 compares baseline bytes/hashes and case-set union.
5. A source-bound consumer is not executed: 2.1 records nonzero consumer counts and actual source/corpus identities.

## Node 1.1: Mandatory acceptance red

Editable: `packages/tools/cmd/runtime-oracle/complete_acceptance_test.go`; read-only `inventory.go`, discovery/helpers and frozen corpus. Add `TestRuntimeFoundationCompleteAcceptance`: call existing discoverLive/LoadInventory/ReconcileComplete, fail on any error/uncovered point, and compare Covered to the sorted set of every discovered supported family/version. Assert the exact required `domain.input` source-path set includes `packages/server/sim/runtime/command_order_oracle_test.go`, `packages/engine/crates/mornlea_domain/src/input/order.rs`, `packages/engine/crates/mornlea_domain/tests/command_order.rs`, `packages/engine/crates/mornlea_domain/tests/corpus_domain.rs`, `packages/tools/cmd/runtime-oracle/inventory_test.go`, `packages/tools/cmd/runtime-oracle/protocol_coverage_test.go` and `packages/tools/cmd/runtime-oracle/protocol_frame_test.go`; enumerate and freeze that set in this test before publication. Current generic reconciliation only enforces a required source set for kernel families, so this assertion supplies domain enforcement rather than assuming it exists. Add `TestRuntimeFoundationCompleteAcceptanceMutations` against cloned fixtures: missing failure case, removed source binding, changed expected bytes, missing route and empty family each fails with its named reason.

Run `go test ./packages/tools/cmd/runtime-oracle -run '^TestRuntimeFoundationCompleteAcceptance' -count=1 -v`. Current strict case must fail for domain.input/45; mutation tests verify fail-closed behavior. Record this red; the node is accepted only after 1.3 supplies the real green. A deliberately red test may stay unstaged while 1.2 proceeds; its exact files and red log are recorded to prevent an incomplete commit. This is one integrated test-first unit: 1.1–1.3 are committed together only after focused green, because 1.1 alone cannot pass. Rollback reverts this test and its input evidence together.

## Node 1.2: Observed stale-sequence producer

Editable: `packages/server/sim/runtime/command_order_oracle_test.go`; export drafts only under a fresh explicit `RUNTIME_ORACLE_EXPORT_DIR`. Read-only engine/production commands, existing success fixture and Rust domain sources. Add `TestCommandOrderOracleStaleSequenceNoEffect` and one case named `stale-sequence-no-effect`: tick7, one live session, sequence0, arrival0, select_hotbar slot7. Capture inventory and chunk revision before real Engine.Step; initial selection and last sequence are0. Assert discarded1/admitted0, selection0, unchanged inventory/revision, last sequence0, placement successes0. Error normalization is `kind:error/category:stale-sequence`, representing actual discard; do not invent a wire CommandRejection packet.

Refactor test-only producer dispatch to execute both positive and stale cases while preserving positive bytes. `commandOrderRejectedOutcomeFor` reads actual observations and rejects a non-rejected run rather than returning synthetic error. The existing singleton classifier assumes admission and is not reused for this case: compare observed before/after selection, inventory/chunk hashes and the boundary probe; initialize the session entry even when its highest sequence is zero, then repeat zero and submit sequence one to observe the last-sequence boundary. Derive discard/admit from those observed effects and probes, not from the input schedule. Keep the old positive classifier/normalization byte-identical. Test malformed expectation mismatch behaviorally first. Export the two input/expected files create-exclusively using the existing protocol; unset export root writes nothing. Run `go test ./packages/server/sim/runtime -run '^TestCommandOrderOracle' -count=1 -v`, require both old success and new rejection. No production API or runtime behavior changes. This producer cannot import into runtime-oracle's production package.

## Node 1.3: Serial reviewed publication

Controller owns `testdata/runtime-migration/contracts.json`, the two new `cases/domain/command_order/stale-sequence-no-effect.{input,expected}.json`, `packages/tools/cmd/runtime-oracle/{inventory.go,inventory_test.go,protocol_coverage_test.go,protocol_frame_test.go,storage_coverage_test.go}`, `packages/engine/crates/mornlea_domain/tests/corpus_domain.rs`, applicable source-bound guards and guides. These are shared integration files, never another producer's edits. Do not add a CLI rewrite flag.

Merge only the new external `domain.input/45` order case into the full baseline. Preserve the existing success owner/bytes and every unrelated case; union and sort actual source hashes including Go producer, Rust ordering provider/test and affected exclusion tests. Refresh hashes for every existing family that pins an edited consumer file; derive this reverse set by manifest source path enumeration. Update exact partition assertions from baseline external count1 to2; pure Rust domain case count does not change. Adjust tests that expected complete reconciliation to stay incomplete so they accept zero gaps without removing their synthetic negative checks. Add exactly `stale-sequence` to the test-only closed executed-domain rejection vocabulary in `protocol_frame_test.go`, separate from login admission and structural decoding; update its exhaustive rejection guard to consult that set. Run the complete runtime-oracle test package so every tracked expected outcome is checked against the vocabulary. No exemptions.

Run the new mandatory acceptance tests, old inventory/protocol mutation tests, `go test ./packages/server/sim/runtime -run '^TestCommandOrderOracle' -count=1`, `rustup run 1.97.1 cargo test --manifest-path packages/engine/Cargo.toml -p mornlea_domain --test command_order --test corpus_domain --locked`, and `go test ./packages/audit -run 'RuntimeOracle|Corpus' -count=1`. Require nonzero tests, exact unchanged unrelated hashes and green complete set. Commit the coherent 1.1–1.3 unit; no next runtime feature starts before its evidence. Rollback restores corpus/consumer assertions together.

## Node 2.1: Complete integrated consumers

Production files read-only. Controller owns evidence logs/ledger. Execute full `go test ./packages/tools/cmd/runtime-oracle -race -count=1`, `go test ./packages/server/sim/runtime -run '^TestCommandOrderOracle' -count=1 -v`, and these existing nonempty Rust targets: `rustup run 1.97.1 cargo test --manifest-path packages/engine/Cargo.toml -p mornlea_domain --test corpus_domain --test command_order --locked`; the same command prefix with `-p mornlea_protocol --test protocol_corpus --locked`, `-p mornlea_storage --test storage_corpus --locked`, and `-p mornlea_engine --test native_contract --test numerical_migration --locked`. Record each complete expanded command, never a nonexistent aggregate replay target. Run `rustup run 1.97.1 cargo fmt --manifest-path packages/engine/Cargo.toml --all -- --check`, `make rust`, `make rust-check`, `make dev-check`, `make test-race`, `go test ./packages/audit -count=1`, `openspec validate --all --strict --no-interactive`. Full stage gates bind one integrated source/manifest identity. Record exact commands/results and nonzero cases by consumer; omitted/failed consumer keeps this node open. Rust ordering alone does not prove authority outcomes. No foreground client.

## Node 2.2: Prerequisite seal

Editable: this ledger and `acceptance.json`, `../rust-authoritative-server/ledger.md`, `../rust-client-core/ledger.md`, their prerequisite readiness registers, the dispatch index and paired `docs/runtime-interface-architecture{,.zh}.md` prerequisite facts. Record `{accepted_source_sha,corpus_sha256,discovered_supported_points,covered_points,uncovered:[],consumer_counts,commands,rollback}`. Assert covered/discovered equality and compare consumer case sets to the corpus. Cite numerical and other accepted successor ledgers without rewriting archives. Explicitly state `domain.input/order` remains an external Go authority reference; pure Rust contracts are accepted but F2 Rust rules and F3 real integration are not.

Run mandatory zero-gap test, strict OpenSpec validation and `git diff --check`; commit the seal separately after 2.1. F2 1.1a and F3 1.1 consume this accepted source/corpus identity. Rollback invalidates that seal and blocks dependent dispatch. Architecture skill promotion requires verified reusable rules; otherwise record no change.
