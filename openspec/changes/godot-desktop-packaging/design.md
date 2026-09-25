## Planning status and prerequisites

This revision changes planning artifacts only. Runtime behavior, version identities, current producers, tracked images, and default startup do not change. The current Go server and Go pilot client-core remain transition implementations; final ownership follows the [target architecture](../../../docs/architecture-target.md). The [archived pilot design](../archive/2026-09-20-pilot-godot-client-migration/design.md) is historical evidence, not authority for new runtime ownership.

Implementation requires the relevant accepted F1, F2, and F3 exit evidence in their ledgers, including exact source SHA, fixture identities, command output, test discovery, and rollback decision. An existing proposal, checked planning status, text search, or optional-entry audit is not completion evidence. After a prerequisite is archived, resolve its ledger through its archive location and retain the accepted SHA. No task is complete merely because a test filter selected zero tests.

Interface baseline: the [target runtime interface map](../../../docs/runtime-interface-architecture.md) assigns S2/C1 session parity, `session@1`/`lifecycle@1` and T1 desktop/asset manifests to this boundary. The launcher consumes accepted server/client contracts and an exact export manifest; it cannot substitute a new local simulation path or activate a target-only semantic family.

## Ownership and selected local transport

Use a supervised Rust server process with loopback TCP for the first local distribution. This reuses F2's server and F3's existing remote path and makes child termination/world ownership explicit. Rust owns process supervision, startup status, bounded cancellation, and the client session. Python presents progress and submits semantic launch/cancel intent. Local Memory remains a shared F2/F3 conformance requirement, but P13 does not need a second in-process runtime to expose local play.

Reject a new Go Memory implementation because it expands transitional ownership. Defer an in-process Rust transport because it adds process/lifecycle complexity without changing user-visible semantics; it would require separate accepted evidence before adoption. The child server is the sole world writer. Lost supervision must not orphan a writable server or start a competing writer.

## Distribution and qualification

The supported target family is desktop macOS/Windows/Linux. Qualify and record each actual target architecture/runtime combination separately. Export only selected Python sources, exact embedded interpreter/extension, native runtime libraries, assets, fonts, licenses, and checksums. Resolve exported dependencies from the package filesystem layout, never current directory/system paths. Exclude development tests, capture tools, comparison CLI, cache/provenance build machinery, non-target libraries, and the standalone Agent environment.

Add `scripts/godot/desktop-release-check.sh --target <macos|windows|linux> --run-dir <explicit-directory>` as the prospective release-closure and lifecycle gate; it fails if run on an unqualified target or required evidence is absent. Existing macOS `make godot-build` is preparation, not proof of Windows/Linux qualification. Each gate records actual source and runtime identities and exercises runtime-missing, corrupt-resource, local-child-exit, and repeated teardown failures.

## Risks, migration, and rollback

Local process exit and save flush can race shutdown: test forced failure with temporary worlds and prove recovered durable state before release. Cross-target loaders may differ: never copy a platform pass to another target. Keep the existing default and legacy release intact while new packages are opt-in. Retain compatibility-checked save backups and restore the entire previous release with exclusive world ownership on rollback; no silent save downgrade or partial binary replacement.

### Visual evidence dependency

The visual task consumes the capture/report contract delivered by phase 2 of [production tooling](../godot-production-tooling/tasks.md), after F3. It does not wait for all tooling producer handoffs: tooling establishes the contract first, features provide parity evidence next, and each case is handed off afterward. This avoids a P8/P9/P10/P12 dependency cycle.

Evidence has three ordered layers: semantic replay, untracked candidate capture, and explicitly approved canonical handoff. Each case names its semantic class, current and candidate producer, source SHA, scenario/input identity, asset/runtime/platform identity, capture boundary, and limits. Same-producer pixel regression retains its existing thresholds; cross-producer parity requires semantic evidence and human review of declared differences. Missing, stale, empty, failed, or non-comparable required cases block handoff. A dummy headless renderer cannot provide GPU pixel evidence; GPU capture uses a qualified non-foreground, no-focus path or requires explicit manual acceptance. No renderer-specific tracked class is created.

## Validation and rollback discipline

Named new test targets and scripts in `tasks.md` are prospective interfaces, not claims that they exist today. Their first implementation task creates them, records nonzero discovery, and runs a failing behavioral case before implementation. Reuse passing evidence only for the same tested SHA. Performance values are informational; invalid identity, incomplete coverage, real overflow, data loss, and I/O failures are hard errors. Automated validation must not launch or focus a foreground game window.

Commit each independently verified implementation node with its scoped evidence before starting the next node. Update affected directory `AGENTS.md` when ownership changes; otherwise record inherited guidance. Closeout includes formatting, Rust checks, six-module vet, all six Go modules under `make test-race`, and strict OpenSpec validation. Review durable architecture findings at the end of each round; record `Architecture skill: no change` when no new verified rule qualifies.
