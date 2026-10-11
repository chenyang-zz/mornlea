## Planning status and prerequisites

This revision changes planning artifacts only. Runtime behavior, version identities, current producers, tracked images, and default startup do not change. The current Go server and Go pilot client-core remain transition implementations; final ownership follows the [target architecture](../../../docs/architecture-target.md). The [archived pilot design](../archive/2026-09-20-pilot-godot-client-migration/design.md) is historical evidence, not authority for new runtime ownership.

Implementation requires the relevant accepted F1, F2, and F3 exit evidence in their ledgers, including exact source SHA, fixture identities, command output, test discovery, and rollback decision. An existing proposal, checked planning status, text search, or optional-entry audit is not completion evidence. After a prerequisite is archived, resolve its ledger through its archive location and retain the accepted SHA. No task is complete merely because a test filter selected zero tests.

Interface baseline: the [target runtime interface map](../../../docs/runtime-interface-architecture.md) assigns T1 replay/diagnostics and `diagnostics@1` to this boundary. Tooling consumes accepted runtime identities and producer schemas, while the [strict evidence, case registry and approval schema](plans/02-evidence-schema.md) is the controller-owned contract before independent adapters or visual handoffs; no tooling report may imply a missing family is implemented.

## Dependency phases and ownership

Phase 1 consumes accepted F1 replay/schema contracts and F3 client-core; any authoritative server replay also requires F2 acceptance. Phase 2 publishes the capture/report contract independently of P8–P11 handoffs. Features then supply their own evidence. Phase 3 transfers only reviewed per-case producer ownership. This ordering avoids requiring completed features before tooling exists or requiring all tooling handoffs before features can capture.

Use a tooling-only `mornlea-migration-evidence` binary under `packages/engine/crates/mornlea_client_core/src/bin/`, with tooling modules and dependencies separated from the runtime library. The library must not depend on the CLI/comparison modules; exports explicitly exclude them. Reuse F1/F3 Rust replay and semantic types rather than inventing `mornlea_tools` or `mornlea_kernel` crates. `scripts/godot/migration-evidence.sh` is a thin orchestrator around the Rust validator and Godot capture adapter.

The planned public command is `scripts/godot/migration-evidence.sh --feature <registered-feature> --run-dir <explicit-directory> --strict`. It is unavailable until phase 2 implements it. It never updates tracked files, selects the latest run, or infers missing identities. `testdata/visual-golden/producer-registry.json` records per-case canonical/candidate ownership and required coverage. Candidate outputs live under `build/visual/migration/`; the registry is metadata, not a fourth image category.

## Capture and report decisions

Register UI/window fixtures under `ui/`, stable world frames under `world/`, and cross-tick human-review GIFs under `motion/`. Bind scenario/input/asset/runtime/platform identities, source SHA, dimensions, work budgets, capture completion boundary, valid sample counts, semantic replay result, producer identity, and difference artifacts. Do not reinterpret a window-only fixture as headless world evidence. A qualified no-focus GPU adapter supplies pixel evidence; dummy headless runs supply only non-pixel lifecycle/semantic evidence. Automated tools never launch a foreground game window.

The Rust strict validator rejects required missing, empty, failed, stale, and non-comparable cases. Performance numeric differences are informational; identity, coverage, overflow, data loss, and I/O errors remain hard failures. Compare same-producer regression pixels at the existing thresholds. Cross-producer evidence records semantic agreement and reviewed expected differences without pretending every backend shares identical rasterization.

## Canonical comparison during partial migration

Phase 2 also introduces `scripts/godot/visual-regression.sh --run-dir <explicit-directory> --strict`, a comparison-only dispatcher over the producer registry. It runs each required PNG only through its current canonical owner and accounts separately for motion review freshness; missing/duplicate ownership and unsupported scoped capture fail closed. Register tested per-case adapters, including a scoped legacy UI adapter if its existing CLI cannot select fixtures. This dispatcher is an optional new entry until P14; do not redirect legacy Make targets in violation of the current optional-entry contract.

After a partial handoff, the old full `make visual-check` or frontend command must not compare legacy pixels against Godot-owned images. Remaining legacy cases use the dispatcher's scoped legacy adapter. A full old-producer check is valid only in the retained previous release with that release's baseline set. Transfer and rollback preserve complete producer/registry/baseline identities together.

## Approved update transaction

A separate `scripts/godot/migration-handoff.sh --manifest <reviewed-manifest> --run-dir <explicit-directory>` consumes a manifest binding each approved case to exact candidate/run/artifact/difference hashes, current and next owner, reviewer, explicit approval record hash, and rollback set. It is unavailable until phase 3. Comparison cannot invoke it implicitly. Stage images and registry changes outside the tracked tree, enumerate every generated file including motion side effects of existing world-update commands, verify the approved allowlist, then publish the complete set or restore the previous set on failure. Unapproved output is rejected or excluded before publication. Run the ownership-aware canonical dispatcher afterward; keep the old producer runnable for rollback.

Reject automatic acceptance of cross-renderer differences because it bypasses semantic review. Reject widening pixel tolerance because it weakens regression protection. Reject placing capture/CLI dependencies in the client-core library because product release and hot paths must not depend on comparison tools.

## Risks and rollback

The current updater may touch more files than the requested world case. Transaction tests must cover unrelated motion files, registry failures, interrupted writes, and reruns. Keep old capture/benchmark/import tooling until the corresponding new tool is proven on identical fixtures; never delete all old tools merely because the registry exists. Required CI promotion stays a separate reviewed decision and must update, rather than evade, optional-only audits.

## Validation and rollback discipline

Named new test targets and scripts in `tasks.md` are prospective interfaces, not claims that they exist today. Their first implementation task creates them, records nonzero discovery, and runs a failing behavioral case before implementation. Reuse passing evidence only for the same tested SHA. Performance values are informational; invalid identity, incomplete coverage, real overflow, data loss, and I/O failures are hard errors. Automated validation must not launch or focus a foreground game window.

Commit each independently verified implementation node with its scoped evidence before starting the next node. Update affected directory `AGENTS.md` when ownership changes; otherwise record inherited guidance. Closeout includes formatting, Rust checks, six-module vet, all six Go modules under `make test-race`, and strict OpenSpec validation. Review durable architecture findings at the end of each round; record `Architecture skill: no change` when no new verified rule qualifies.

## Post-numerical-closure planning decisions

The [node dependency and ownership register](plans/03-parallel-readiness.md) and [refined node decisions](plans/04-refined-nodes.md) are normative execution inputs alongside the original packets. Preserve prior scope and separate source-contract, real-provider and real-integration acceptance. Numerical completion does not supply complete F1 acceptance; use the independently owned [foundation successor](../archive/2026-10-08-rust-runtime-foundation-acceptance/proposal.md).

The controller compared wholesale migration resequencing, immediate parallel dispatch from prospective signatures, and bounded refinement of existing nodes. Bounded refinement preserves reviewed scope and existing node identities while making dependencies, shared edits and source-information limits decidable. Immediate dispatch remains blocked by missing accepted contract/provider SHAs. Broad shared files stay serial; only disjoint providers with accepted predecessors can overlap.

Still capture and motion timing have disjoint providers and tests. The serial dispatcher owns explicitly named legacy UI/world subset adapters; current-owner acceptance does not wait on later feature candidates.
