## Why

Rust shared contracts alone do not move authoritative ticks, validation or persistence out of Go. Migrate the server behind the same externally observable contracts, proving offline parity before selecting one Rust authority.

## Status and prerequisites

Planning only: F2 in [`docs/architecture-target.md`](../../../docs/architecture-target.md). The accepted [F1 baseline](../archive/2026-09-22-rust-runtime-foundation-baseline/proposal.md) is not complete F1 acceptance. F2 remains blocked on archived [domain-event](../archive/2026-09-22-rust-domain-event-completion/proposal.md), [protocol](../archive/2026-09-24-rust-protocol-completion/proposal.md), [region format](../archive/2026-09-23-rust-region-format-completion/proposal.md), and [storage safety](../archive/2026-09-24-rust-storage-safety-repairs/proposal.md) successors, the active [storage codec closure](../rust-storage-codec-closure/proposal.md), remaining numerical/pathfinding nodes in archived [runtime foundation](../archive/2026-09-21-rust-runtime-foundation/tasks.md), and final zero-gap F1 acceptance. Controller-owned Superpowers packets live in [execution-contract.md](execution-contract.md) and [plans/](plans/). Completion requires the implementation SHA, executed non-empty tests, server inventory coverage, failure-path results and rollback evidence in `ledger.md`; file existence, an archived baseline or OpenSpec status is insufficient.

## What Changes

- Implement the authoritative Rust server over F1 contracts and existing numerical kernels, including world, player/entity/inventory rules, persistence, budgets and session lifecycle.
- Keep Memory and TCP on one login, codec, validation and simulation path; revalidate independent Agent candidates at tick boundaries.
- Compare against recorded Go runs offline, exercise crash/I/O/cancellation paths, and qualify an opt-in Rust server without changing the default product entry.
- Provide a single-authority selection and rollback procedure with save compatibility; block completion for any missing supported behavior.

## Capabilities

### New Capabilities

- `rust-authoritative-server`: A Rust server that preserves authoritative outcomes, persistence, and local/remote semantics.

### Modified Capabilities

None. Existing wire, save and gameplay requirements remain the compatibility oracle; any discovered need to change observable behavior requires a scoped delta before implementation.

## Impact

- Affected areas: New `packages/engine/crates/mornlea_server/`, F1 contract crates, server fixtures and offline oracle tools. Existing Go server packages supply characterization evidence, not new product rules. Standalone `packages/agent/` remains a separate service.
- Compatibility: preserve current protocol/save versions and semantics. Inventory every version from verified code at implementation start. No version bump or silent conversion is authorized by this plan.
- Concurrency/performance: bounded queues, batches and tick work; no blocking I/O on hot paths. Report performance measurements; overflow, data loss, identity gaps and I/O failures remain hard failures.
- User outcome: the target Rust owner becomes independently testable with equivalent behavior; default startup remains the current Go application plus Rust renderer until P14.
- Non-goals: new gameplay, a default-client switch, production GDScript, Python authority, or simultaneous Go/Rust online writers.
- Rollback: retain the previous runtime and its compatible data; select one authority and never rely on a shadow writer or implicit save downgrade.

## Deferred and abandoned

Production Godot features and visual handoffs remain in P8–P12; distribution and default retirement remain P13–P14. No runtime implementation is claimed by this planning synchronization.
