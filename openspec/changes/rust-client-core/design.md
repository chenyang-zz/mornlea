## Context

The existing Rust Godot extension dynamically loads a pilot Go client-core. Python already consumes typed semantic views. F1 and the accepted F2 protocol/session contract are prerequisites; this stage replaces client runtime ownership without turning pilot code into the final architecture.

## Goals / Non-Goals

Preserve typed presentation behavior while moving sessions, mirrors, prediction and frame production to Rust. Do not expand Go core behavior, migrate complete terrain/UI/actors/audio features, transfer tracked visual producers, delete the legacy renderer, or switch defaults.

## Decisions

### Rust core and adapter direction

Add `packages/engine/crates/mornlea_client_core/` with dependencies on F1 protocol/domain and `mornlea_engine`; it must not depend on the Godot adapter, server implementation, Python or GPU host. Existing `mornlea_godot` depends on the Rust core through a safe Rust-facing API and remains sole owner of Godot conversions, feature negotiation and boundary failure translation. Client and server may share contracts, never mutable authority.

Create the new crate guide and update `packages/engine/AGENTS.md`. Keep the old Go dynamic-library path as an explicitly selected pilot/rollback adapter while comparing offline; never automatically fall back after a Rust core failure. The old Go core ABI and `mornlea_client` ABI v19 are separate surfaces and remain until their independent retirement evidence exists.

### Semantic continuity

Move the meaning of the current Go `FrameSnapshot` and typed input/view families into Rust records; do not freeze Go internal layouts as the final contract. One session epoch and one coherent frame revision cover every sampled view. The frame revision also advances when an accepted Rust-owned local UI selection, cancellation, or panel transition changes a published view during a silent server session; confirmed gameplay mirrors remain unchanged until server publication. Rust owns network decoding, mirrors, correction, mesh scheduling and capacity/revision policy. Godot Python applies typed views in bounded batches and owns presentation resources. Unsupported family negotiation fails closed.

Establish an inventory of frame/input/world/entity/UI/cue families under `testdata/runtime-migration/client/`, mapping each current source to Rust tests and required P8–P11 consumers. Publish versioned family contracts; leave new visual features disabled until their own changes pass. No production GDScript or Python raw ABI/wire access is introduced.

### Thread and lifecycle ownership

Keep bounded queues and immutable publications between session, preparation and main-thread application. Declare reset, reconnect, overflow, drop/upsert and shutdown semantics before implementation. The adapter releases Python feature references before destroying Rust resources; panic handling never unwinds into the host. Test repeated create/reset/destroy with queued work without a graphics device, then qualify the real headless Godot/Python lifecycle separately. The existing `make godot-smoke` validates startup/teardown markers but does not establish a Rust-backed session. Extend the lifecycle harness with explicit Rust producer/revision selection, live create/reset/reconnect/destroy and queued work; fail when only engine/Python startup passes or a stale/Go producer is selected. Rebuild the tested native artifacts and bind their identities to the report.

### Evidence and rejected alternatives

Use offline Go transcripts for parity and F2 integration for local/remote behavior. Screenshots do not prove prediction, ordering or cancellation. Recreating Python wire decoders or preserving a Go runtime fallback would retain the wrong owner. Calling Rust per cell instead of semantic batches violates the bounded bridge direction.

## Risks / Trade-offs

- Typed APIs can match shapes but differ in ordering → compare semantic observations across epochs and correction paths.
- Changing the native link can break packaging → record producer/bridge identity and defer exported release closure to P13; qualify source-tree loading and failure behavior now.
- Client-core migration may expose visual differences → retain candidates under existing pilot rules and diagnose semantics; any tracked update waits for a separate feature handoff.

## Migration Plan

Accept F1 and F2 session contracts; inventory families; implement sessions and state, prediction, preparation/publication and lifecycle in separate tested nodes; adapt the Rust Godot bridge; run replay and real Rust-server integration. Publish F3 completion and family versions to P8–P12. Rollback selects the previous pilot/legacy release as a whole without extending it with new features. F3 does not retire the Bootstrap; P14 must first provide qualified native diagnostics.

## Validation and completion evidence

Implementation follows failing contract/replay tests, minimum implementation, then refactoring. Test targets in `tasks.md` are prospective until their owning task registers them. Use the actual Rust workspace (`--manifest-path packages/engine/Cargo.toml`) and named integration targets; inspect `-- --list` output and reject empty discovery. Do not substitute text searches or unrelated optional-build audits for prerequisite acceptance.

Record source SHA, corpus digest/coverage, command, discovered/executed tests, result, failure cases and rollback proof in `ledger.md`. Rust stage completion requires its full declared inventory, not only the first successful slice. Commit each independently verified task; if an inventory item exceeds one session, refine it into explicit capability tasks before implementation rather than checking off a broad placeholder. Planning validation proves artifact structure only.

At implementation closeout run formatting, `make rust-check`, `make dev-check` (including all six Go-module vet commands), `make test-race`, `go test ./packages/audit -count=1`, and `openspec validate --all --strict --no-interactive`. Add the change-specific replay, failure-injection and platform gates. No graphical foreground window may be started by automated tests.
