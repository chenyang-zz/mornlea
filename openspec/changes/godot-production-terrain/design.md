## Planning status and prerequisites

This revision changes planning artifacts only. Runtime behavior, version identities, current producers, tracked images, and default startup do not change. The current Go server and Go pilot client-core remain transition implementations; final ownership follows the [target architecture](../../../docs/architecture-target.md). The [archived pilot design](../archive/2026-09-20-pilot-godot-client-migration/design.md) is historical evidence, not authority for new runtime ownership.

Implementation requires the relevant accepted F1, F2, and F3 exit evidence in their ledgers, including exact source SHA, fixture identities, command output, test discovery, and rollback decision. An existing proposal, checked planning status, text search, or optional-entry audit is not completion evidence. After a prerequisite is archived, resolve its ledger through its archive location and retain the accepted SHA. No task is complete merely because a test filter selected zero tests.

Interface baseline: the [target runtime interface map](../../../docs/runtime-interface-architecture.md) assigns C2/G1 and `terrain@1` to this boundary. Before independently implementing mesh producer and Godot consumer, accept the exact terrain family schema and bridge registry mapping with validated stale/removal/capacity examples; its logical name alone is not an enabled ABI family.

## Ownership and design decisions

`mornlea_engine` remains the single numerical implementation. `mornlea_client_core` owns chunk interpretation, revisions, visibility, mesh scheduling, and bounded semantic publication; `mornlea_godot` validates the bridge and owns any necessary native bulk resource conversion. Embedded Python in `features/world/` applies typed resources within declared budgets. GPU resource release remains on the Godot owner thread. No per-cell FFI or raw buffers reach Python.

Reuse the stable project root and catalog. The pure-GDScript Bootstrap stays a migration diagnostic until the later native-launcher cutover; this feature does not promise it as the permanent main scene. Reject a second Go/Python mesher because it duplicates numerical ownership. Reject a monolithic host terrain implementation because it prevents independent disable and teardown tests.

Define budget values and supported view-distance cases in the test fixture manifest before enabling the feature. Preserve full LOD transitions, visibility, material classes, fog/light inputs, and reset semantics. Pools have explicit capacities; a stale completion is discarded by identity, while real overflow fails visibly. Python never invents a new revision.

## Risks and migration

Resource leaks and delayed completions can survive a reset: repeated activate/reset/deactivate tests must account for all owned resources. Different rasterization can change pixels: record differences without loosening tolerances. Roll out behind a disabled catalog entry, validate semantic parity, then enable only the approved feature profile. Disable or restore the prior producer on failure; saves are untouched.

### Visual evidence dependency

The visual task consumes the capture/report contract delivered by phase 2 of [production tooling](../godot-production-tooling/tasks.md), after F3. It does not wait for all tooling producer handoffs: tooling establishes the contract first, features provide parity evidence next, and each case is handed off afterward. This avoids a P8/P9/P10/P12 dependency cycle.

Evidence has three ordered layers: semantic replay, untracked candidate capture, and explicitly approved canonical handoff. Each case names its semantic class, current and candidate producer, source SHA, scenario/input identity, asset/runtime/platform identity, capture boundary, and limits. Same-producer pixel regression retains its existing thresholds; cross-producer parity requires semantic evidence and human review of declared differences. Missing, stale, empty, failed, or non-comparable required cases block handoff. A dummy headless renderer cannot provide GPU pixel evidence; GPU capture uses a qualified non-foreground, no-focus path or requires explicit manual acceptance. No renderer-specific tracked class is created.

## Validation and rollback discipline

Named new test targets and scripts in `tasks.md` are prospective interfaces, not claims that they exist today. Their first implementation task creates them, records nonzero discovery, and runs a failing behavioral case before implementation. Reuse passing evidence only for the same tested SHA. Performance values are informational; invalid identity, incomplete coverage, real overflow, data loss, and I/O failures are hard errors. Automated validation must not launch or focus a foreground game window.

Commit each independently verified implementation node with its scoped evidence before starting the next node. Update affected directory `AGENTS.md` when ownership changes; otherwise record inherited guidance. Closeout includes formatting, Rust checks, six-module vet, all six Go modules under `make test-race`, and strict OpenSpec validation. Review durable architecture findings at the end of each round; record `Architecture skill: no change` when no new verified rule qualifies.

## Post-numerical-closure planning decisions

The [node dependency and ownership register](plans/03-parallel-readiness.md) and [refined node decisions](plans/04-refined-nodes.md) are normative execution inputs alongside the original packets. Preserve prior scope and separate source-contract, real-provider and real-integration acceptance. Numerical completion does not supply complete F1 acceptance; use the independently owned [foundation successor](../rust-runtime-foundation-acceptance/proposal.md).

The controller compared wholesale migration resequencing, immediate parallel dispatch from prospective signatures, and bounded refinement of existing nodes. Bounded refinement preserves reviewed scope and existing node identities while making dependencies, shared edits and source-information limits decidable. Immediate dispatch remains blocked by missing accepted contract/provider SHAs. Broad shared files stay serial; only disjoint providers with accepted predecessors can overlap.

Section upload (64), mesh completion drain (32) and separately measured live resources are different bounds. The controller owns world-root integration, test dispatch and stable lod manifest relocation.
