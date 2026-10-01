## Why

The Godot pilot still receives its session, mirror, prediction and presentation data through a Go client-core ABI. Replace that runtime owner with Rust before production Godot features expand the pilot seam.

## Status and prerequisites

Planning only: F3 in [`docs/architecture-target.md`](../../../docs/architecture-target.md). Implementation entry must re-bind completed [F1](../rust-runtime-foundation-acceptance/proposal.md) and the jointly accepted [F2](../rust-authoritative-server/proposal.md) common/Memory/TCP S2 contract on an actual accepted source; historical pending prose is not current prerequisite evidence. F3 final integration acceptance requires the F2 Rust server parity evidence. Completion requires the implementation SHA, executed non-empty tests, corpus coverage, failure-path results and rollback evidence in `ledger.md`; file existence or OpenSpec status is insufficient.

## What Changes

- Implement Rust login/session, mirrors, prediction/reconciliation, bounded publication and immutable semantic frame assembly using F1 contracts and the F2 server contract.
- Adapt the existing Rust Godot extension to the Rust core while preserving typed Python intent and feature views.
- Prove transcript, correction, stale/reset, capacity and lifecycle parity against offline Go evidence and the Rust server.
- Publish versioned presentation families required by P8–P11 without implementing those features or transferring visual baseline ownership.
- Specify the scoped presentation exception for extreme admitted drop chunk coordinates: preserve identity and compute exact finite f64 geometry instead of inheriting Go's narrowed i32 shift wrap. The delta scenario pins that boundary; protocol/save/gameplay validity does not change.

## Capabilities

### New Capabilities

- `rust-client-core`: A Rust-owned client state machine with bounded typed semantic presentation and input.

### Modified Capabilities

- `godot-client-pilot`: make Go core ownership a pre-F3 transition condition, move session/protocol/mirror/prediction and atomic typed publication to the accepted Rust core, and preserve the diagnostic Bootstrap until the later product cutover.

Existing wire, save and gameplay requirements remain the compatibility oracle; any additional observable behavior change requires a scoped delta before implementation.

## Impact

- Affected areas: New `packages/engine/crates/mornlea_client_core/`, existing `mornlea_godot`, typed bridge tests and pilot replay fixtures. The legacy Go core and client ABI v19 are distinct rollback surfaces; neither is deleted by this change. No new Go real-time behavior.
- Compatibility: preserve current protocol/save versions and semantics. Inventory every version from verified code at implementation start. No version bump or silent conversion is authorized by this plan.
- Concurrency/performance: bounded queues, batches and tick work; no blocking I/O on hot paths. Report performance measurements; overflow, data loss, identity gaps and I/O failures remain hard failures.
- User outcome: the target Rust owner becomes independently testable with equivalent behavior; default startup remains the current Go application plus Rust renderer until P14.
- Non-goals: new gameplay, a default-client switch, production GDScript, Python authority, or simultaneous Go/Rust online writers.
- Rollback: retain the previous runtime and its compatible data; select one authority and never rely on a shadow writer or implicit save downgrade.

## Deferred and abandoned

Production Godot features and visual handoffs remain in P8–P12; distribution and default retirement remain P13–P14. No runtime implementation is claimed by this planning synchronization.


## Current code-only implementation batch

The user selected the complete semantic client core, safe Rust producer adapter code, and Python host ownership migration, including all ten families and twenty typed actions. The approved design and [code-only execution packet](plans/07-code-only-implementation-handoff.md) preserve the existing architecture and node identities. Automated core/replay/adapter-routine/host/static checks must not launch Godot in any mode, including headless or an indirect runner. Scenes, models, renderer/resource realization, visual qualification, default switching, and the running F2 server implementation remain outside this batch.

This interim code deliverable is distinct from full F3 acceptance. Actual native Godot marshalling/release qualification, 100 rebuilt real process cycles, real F2/C1/C2/G1 local/remote integration, and full stage closure remain required at their separately authorized gates. A code-only result or documentation push cannot close those gates. Final planning artifacts are English in this active OpenSpec change; the GLM5.3 handoff is portable and does not install, sign in to, submit to, or launch a coding app.
