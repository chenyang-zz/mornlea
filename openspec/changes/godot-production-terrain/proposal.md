## Why

The remote pilot proves only a small near-ring loop. Production terrain needs bounded full-distance presentation after Rust owns world interpretation and client preparation.

## What Changes

- Consume Rust semantic terrain publications for near/far rings, LOD, water, cutout, fog, and lighting.
- Add bounded Godot resource pools, reset cancellation, and independent catalog disable.
- Produce reviewed world evidence before any per-case producer handoff.

## Scope and prerequisites

This is a planned, independently reversible production slice. Non-goals are new gameplay rules, expansion of Go real-time ownership, production GDScript, mobile/Web/console support, and unreviewed baseline updates. [F1](../archive/2026-10-08-rust-runtime-foundation-acceptance/proposal.md), [F2](../rust-authoritative-server/proposal.md), and [F3](../rust-client-core/proposal.md) provide the Rust contracts, sole authoritative server, and typed client-core bridge it consumes; their accepted ledger evidence is required before dependent implementation. Python remains Godot's feature language. This planning revision authorizes no runtime cutover, tracked baseline update, or version bump.



## Capabilities

### New Capabilities

- `godot-production-terrain`: Present production terrain from Rust semantic publications with bounded resources and reversible visual ownership.

### Modified Capabilities

None. The current pilot contract remains scoped to the pilot; this new capability does not rewrite it.

## Impact

- Affected: `packages/engine/crates/mornlea_client_core`, existing `mornlea_engine` and `mornlea_godot`, `apps/mornlea-godot/features/world/`, and terrain evidence/harness files.
- Compatibility: consume F1/F3 versions; no new network/save schema or legacy client ABI change is planned. An incompatible semantic-family extension requires explicit contract revision before implementation.
- Concurrency/performance: bounded immutable publications and upload queues; thresholds remain unchanged and measurements are informational.
- Rollback: disable this feature or restore its previous catalog entry and producer; existing default startup remains available.
