## Why

Feature migration needs one reproducible evidence contract before individual producers can be reviewed. Pilot capture scripts and empty mappings are insufficient to authorize canonical ownership.

## What Changes

- Establish per-case producer registry, identity-complete results, explicit run selection, and strict required-case acceptance.
- Separate semantic replay, candidate capture, and approved atomic handoff; preserve UI/world/motion classes and existing pixel thresholds.
- Migrate capture, benchmark, devcapture, import, and selected CI tooling in bounded nodes after evidence exists.

## Scope and prerequisites

This is a planned, independently reversible production slice. Non-goals are new gameplay rules, expansion of Go real-time ownership, production GDScript, mobile/Web/console support, and unreviewed baseline updates. [F1](../archive/2026-10-08-rust-runtime-foundation-acceptance/proposal.md), [F2](../rust-authoritative-server/proposal.md), and [F3](../rust-client-core/proposal.md) provide the Rust contracts, sole authoritative server, and typed client-core bridge it consumes; their accepted ledger evidence is required before dependent implementation. Python remains Godot's feature language. This planning revision authorizes no runtime cutover, tracked baseline update, or version bump.



## Capabilities

### New Capabilities

- `godot-production-tooling`: Provide reproducible migration evidence and reviewed per-case visual producer handoff without expanding runtime authority.

### Modified Capabilities

None. The current pilot contract remains scoped to the pilot; this new capability does not rewrite it.

## Impact

- Affected: tooling-only Rust CLI under `mornlea_client_core`, `scripts/godot/`, visual registry, report fixtures, scoped audit tests, and approved CI producer jobs.
- Protocol/save compatibility: consume F1/F2/F3 replay contracts; no protocol/save change. Report schema is versioned separately and invalid/missing identities fail closed.
- Concurrency/performance: bounded capture, sampling, and report operations outside real-time hot paths; performance measurements remain informational.
- Rollback: restore previous registry and reviewed image set atomically, and retain old tool entry points until their replacements pass. Required CI/default startup changes belong to P14 unless explicitly approved for a case.
