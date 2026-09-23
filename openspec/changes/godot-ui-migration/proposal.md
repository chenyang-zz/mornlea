## Why

The selected final UI route is Godot Control with embedded Python. The pilot HUD does not cover menus, inventory/container interactions, tokens, focus, and complete UI fixtures.

## What Changes

- Inventory and preserve existing UI intent/view semantics using Rust contracts and offline legacy characterization.
- Implement Godot Control UI in embedded Python; keep WebView solely as the previous producer or release rollback.
- Produce per-fixture semantic and visual acceptance before handoff.

## Scope and prerequisites

This is a planned, independently reversible production slice. Non-goals are new gameplay rules, expansion of Go real-time ownership, production GDScript, mobile/Web/console support, and unreviewed baseline updates. [F1](../archive/2026-09-21-rust-runtime-foundation/proposal.md), [F2](../rust-authoritative-server/proposal.md), and [F3](../rust-client-core/proposal.md) provide the Rust contracts, sole authoritative server, and typed client-core bridge it consumes; their accepted ledger evidence is required before dependent implementation. Python remains Godot's feature language. This planning revision authorizes no runtime cutover, tracked baseline update, or version bump.



## Capabilities

### New Capabilities

- `godot-ui-migration`: Provide production Godot Control UI over Rust semantic views, preserving tokens and confirmed interaction behavior.

### Modified Capabilities

None. The current pilot contract remains scoped to the pilot; this new capability does not rewrite it.

## Impact

- Affected: Rust client-core/bridge UI families, `apps/mornlea-godot/features/ui/`, UI fixture harnesses, and reviewed producer records.
- Compatibility: no protocol/save or legacy client ABI change is planned; versioned semantic views consume F3 contracts.
- Concurrency/performance: bounded view/event batches and UI callbacks; focus/resize state stays presentation-owned.
- Rollback: disable the new UI feature and select the prior release/producer; current canonical UI images stay unchanged until approved handoff.
