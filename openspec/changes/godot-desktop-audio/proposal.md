## Why

The pilot intentionally excludes desktop audio and complete device handling. Production adapters need confirmed cue identity, focus behavior, and no-device guarantees over Rust semantic events.

## What Changes

- Add bounded cue playback and desktop input adaptation using embedded Python and Godot device APIs.
- Preserve confirmed-event de-duplication, no-device execution, and repeated teardown.
- Qualify only implemented desktop targets and retain independent adapter disable.

## Scope and prerequisites

This is a planned, independently reversible production slice. Non-goals are new gameplay rules, expansion of Go real-time ownership, production GDScript, mobile/Web/console support, and unreviewed baseline updates. [F1](../archive/2026-10-08-rust-runtime-foundation-acceptance/proposal.md), [F2](../rust-authoritative-server/proposal.md), and [F3](../rust-client-core/proposal.md) provide the Rust contracts, sole authoritative server, and typed client-core bridge it consumes; their accepted ledger evidence is required before dependent implementation. Python remains Godot's feature language. This planning revision authorizes no runtime cutover, tracked baseline update, or version bump.



## Capabilities

### New Capabilities

- `godot-desktop-audio`: Provide bounded desktop audio and device adapters with confirmed cue identity and device-free automated execution.

### Modified Capabilities

None. The current pilot contract remains scoped to the pilot; this new capability does not rewrite it.

## Impact

- Affected: Rust client-core/bridge event families, `apps/mornlea-godot/platform/desktop/`, audio resources, input/audio harnesses.
- Compatibility: no protocol/save schema or legacy ABI change is planned; current cue identities and configuration semantics are characterized offline.
- Concurrency/performance: bounded queues; no main-thread device retry or unbounded callback loop.
- Rollback: disable audio/controller adapters; silent playback preserves confirmed gameplay and other presentation.
