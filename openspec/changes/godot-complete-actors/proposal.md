## Why

The pilot presents only one remote-entity class. Complete actors need explicit identity, lifecycle, and evidence contracts over Rust semantic entity families.

## What Changes

- Present companions, hostiles, passives, projectiles, viewmodel, and effects through independently disableable features.
- Preserve atomic entity batches, pooling, interpolation inputs, reset, and confirmed outcome semantics.
- Review static and motion evidence per actor family before canonical handoff.

## Scope and prerequisites

This is a planned, independently reversible production slice. Non-goals are new gameplay rules, expansion of Go real-time ownership, production GDScript, mobile/Web/console support, and unreviewed baseline updates. [F1](../archive/2026-10-08-rust-runtime-foundation-acceptance/proposal.md), [F2](../rust-authoritative-server/proposal.md), and [F3](../rust-client-core/proposal.md) provide the Rust contracts, sole authoritative server, and typed client-core bridge it consumes; their accepted ledger evidence is required before dependent implementation. Python remains Godot's feature language. This planning revision authorizes no runtime cutover, tracked baseline update, or version bump.



## Capabilities

### New Capabilities

- `godot-complete-actors`: Present complete actors and effects from Rust semantic families without creating gameplay authority.

### Modified Capabilities

None. The current pilot contract remains scoped to the pilot; this new capability does not rewrite it.

## Impact

- Affected: Rust client-core/bridge entity publications; `apps/mornlea-godot/features/actors/`, `features/effects/`, `features/viewmodel/`; actor harness and evidence fixtures.
- Protocol/save compatibility: consume F1/F2 contracts; no new actor rules, packet formats, save schemas, or legacy ABI changes are planned.
- Concurrency/performance: immutable bounded entity batches, bounded pools/callbacks, cancellation by epoch.
- Rollback: disable affected catalog families and restore previous case producers without changing authoritative state.
