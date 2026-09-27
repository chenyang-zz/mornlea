## Planning status and prerequisites

This revision changes planning artifacts only. Runtime behavior, version identities, current producers, tracked images, and default startup do not change. The current Go server and Go pilot client-core remain transition implementations; final ownership follows the [target architecture](../../../docs/architecture-target.md). The [archived pilot design](../archive/2026-09-20-pilot-godot-client-migration/design.md) is historical evidence, not authority for new runtime ownership.

Implementation requires the relevant accepted F1, F2, and F3 exit evidence in their ledgers, including exact source SHA, fixture identities, command output, test discovery, and rollback decision. An existing proposal, checked planning status, text search, or optional-entry audit is not completion evidence. After a prerequisite is archived, resolve its ledger through its archive location and retain the accepted SHA. No task is complete merely because a test filter selected zero tests.

Interface baseline: the [target runtime interface map](../../../docs/runtime-interface-architecture.md) supplies the D0–T1 ownership and version register that the release manifest must enumerate. P14 consumes actual accepted implementation identities and real integration for every required family; its [v2 two-cycle evidence contract](plans/02-cycle-schema.md) binds each target, case and restore report to a clean build/run identity; the target catalog alone cannot satisfy either release cycle or rollback gate.

## Complete dependency gate

Consume the accepted F1/F2/F3 ledgers plus [terrain](../godot-production-terrain/tasks.md), [actors](../godot-complete-actors/tasks.md), [UI](../godot-ui-migration/tasks.md), [desktop devices](../godot-desktop-audio/tasks.md), [tooling](../godot-production-tooling/tasks.md), and [packaging](../godot-desktop-packaging/tasks.md). Record the concrete release feature/platform manifest; no required item can be hidden by removing it from a report. Two distinct release cycles must each bind source SHA, release/package identities, protocol/save versions, complete semantic/visual coverage, performance hard-error checks, local/remote behavior, and successful restore of the previous release. Re-running one build twice is not two cycles.

## Runtime and ABI retirement

Rust server owns the world; Rust client-core owns session/mirror/prediction; existing `mornlea_godot` owns the typed bridge; embedded Python owns presentation. Inventory transitive dependencies and executable launch paths to prove the selected release has no Go real-time dependency. Preserve Go only for identified offline tools and previous-release artifacts. The `mornlea_client` renderer ABI v19 is not the pilot Go client-core header ABI; inventory both surfaces and consumers independently before removing either. Do not mass-delete Go directories while tools or a previous release still require them.

## Launcher and scoped contract transition

Implement native diagnostics before changing the main scene or removing Bootstrap. They must preserve unprepared source-project openability and identify missing/corrupt native/Python artifacts, target, and preparation action before importing Python or starting a session. A native launcher is the final product path; GDScript never receives gameplay features.

The accompanying full `godot-client-pilot` delta scopes the pilot's current Go ownership and optional entry to pre-cutover operation and states the accepted product behavior. Update `packages/audit/godot_entrypoints_test.go`, project/bootstrap closure audits, Makefile entry points, and CI requirements through red-first replacement tests. Optional-only guards remain active until their replacements are ready. Reject exemption flags or blanket test deletion: source/resource isolation, desktop scope, authority, Python boundaries, and unrelated visual ownership must remain enforced.

## Release sequence and risks

First qualify the native launcher and all product closure tests while default startup stays unchanged. Then accumulate two release-cycle records. Only afterward apply the approved default switch. Finally remove unused pilot/old-runtime consumers one bounded node at a time with dependency and rollback evidence. Keep the old release artifact runnable throughout the cutover and retirement window.

A current save may be incompatible with the old release: rollback selects an explicitly compatibility-checked backup, never an implicit schema downgrade. A platform missing runtime or producer evidence blocks its required release acceptance. A failed new launch exits cleanly rather than mixing runtimes. Source deletion is postponed until the accepted retirement inventory proves no required consumer remains.

## Validation and rollback discipline

Named new test targets and scripts in `tasks.md` are prospective interfaces, not claims that they exist today. Their first implementation task creates them, records nonzero discovery, and runs a failing behavioral case before implementation. Reuse passing evidence only for the same tested SHA. Performance values are informational; invalid identity, incomplete coverage, real overflow, data loss, and I/O failures are hard errors. Automated validation must not launch or focus a foreground game window.

Commit each independently verified implementation node with its scoped evidence before starting the next node. Update affected directory `AGENTS.md` when ownership changes; otherwise record inherited guidance. Closeout includes formatting, Rust checks, six-module vet, all six Go modules under `make test-race`, and strict OpenSpec validation. Review durable architecture findings at the end of each round; record `Architecture skill: no change` when no new verified rule qualifies.
