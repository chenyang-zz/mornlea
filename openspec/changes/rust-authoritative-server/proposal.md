## Why

Rust shared contracts alone do not move authoritative ticks, validation or persistence out of Go. Migrate the server behind the same externally observable contracts, proving offline parity before selecting one Rust authority.

## Status and prerequisites

F2 implementation is in progress; whole-branch review has reopened integrated acceptance. Complete F1 is accepted at source `d042982d33bb1694d768b75b01c297bd02534a08` and corpus `sha256:8b5813ecce2fe866ab1c4786a35925e8aadd9aab476196abb189d3c72d4f28f3`, covering112 supported points/1434 executed cases/zero gaps; see the [foundation seal](../rust-runtime-foundation-acceptance/acceptance.json). D0/P0/S0/K0 and the numerical implementation are implemented prerequisites. The initial runnable task verifies that seal and inventories source-bound server coverage. Parallel providers remain blocked until the new S1 compile-ready contract and consumer doubles are accepted on an implementation SHA. Planning validation cannot close an implementation checkbox.

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

Production Godot features and visual handoffs remain in P8–P12; distribution and default retirement remain P13–P14. Rule providers and contract tests are implemented, but complete executable runtime acceptance remains pending.
