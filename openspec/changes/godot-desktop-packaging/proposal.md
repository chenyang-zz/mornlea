## Why

The final desktop product needs local play and complete release closures beyond the remote macOS pilot. Packaging must prove Rust authority and embedded runtime isolation on each supported desktop target.

## What Changes

- Select supervised Rust server launch over loopback TCP for initial local play, sharing the remote login and validation path.
- Package and qualify macOS, Windows, and Linux closures independently, including embedded Python and licensed assets.
- Verify local/remote saves, startup failures, teardown, and previous-release restore before P14.

## Scope and prerequisites

This is a planned, independently reversible production slice. Non-goals are new gameplay rules, expansion of Go real-time ownership, production GDScript, mobile/Web/console support, and unreviewed baseline updates. [F1](../archive/2026-10-08-rust-runtime-foundation-acceptance/proposal.md), [F2](../rust-authoritative-server/proposal.md), and [F3](../rust-client-core/proposal.md) provide the Rust contracts, sole authoritative server, and typed client-core bridge it consumes; their accepted ledger evidence is required before dependent implementation. Python remains Godot's feature language. This planning revision authorizes no runtime cutover, tracked baseline update, or version bump.



## Capabilities

### New Capabilities

- `godot-desktop-packaging`: Deliver desktop release closures and local play through the same authoritative Rust runtime used remotely.

### Modified Capabilities

None. The current pilot contract remains scoped to the pilot; this new capability does not rewrite it.

## Impact

- Affected: Rust server/client-core launch lifecycle, `apps/mornlea-godot/platform/desktop/`, desktop export presets, `scripts/godot/` release tooling, and release audit fixtures.
- Compatibility: use F1/F2 protocol/save migration contracts; no new schema version is implied. Any unsupported save downgrade blocks rollback until a compatible backup is selected.
- Concurrency/performance: one supervised child/server writer, bounded launch/cancel/status operations outside the main frame path.
- Rollback: stop the new runtime, restore previous release and compatible backup, or return to the previous remote-only profile; default switch is out of scope.
