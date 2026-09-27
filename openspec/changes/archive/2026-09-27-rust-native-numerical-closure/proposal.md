## Why

The accepted Rust domain, protocol, and storage slices still leave eleven numerical families uncovered: ten existing engine operations have no safe native Rust entry point, and companion pathfinding remains Go-only. F2 cannot own an authoritative Rust tick while it has to construct engine ABI byte requests or call Go for these operations. This change freezes their interfaces before independent implementations start.

## What Changes

- Publish validated, typed Rust interfaces for the ten existing `mornlea_engine` operations and deterministic pathfinding. A contract landing supplies the shared types and test doubles before implementation lanes begin.
- Implement the families behind those interfaces with bounded work, caller-owned reusable scratch where needed, atomic result publication, and executable Go/ABI comparison cases.
- Keep the existing numerical algorithms and engine ABI v11 behavior for valid inputs. Close the eleven source-bound corpus routes and report remaining F1 gaps explicitly; the separate complete F1 acceptance remains a later gate.

## Capabilities

### New Capabilities

None.

### Modified Capabilities

- `rust-runtime-foundation`: add safe native numerical operations, deterministic pathfinding, and executable numerical compatibility evidence.

## Impact

- Affected: `packages/engine/crates/mornlea_engine/`, test-only Go producers under `packages/tools/cmd/runtime-oracle/` and `packages/shared/nativeabi/`, `testdata/runtime-migration/`, and focused directory guides.
- Compatibility: protocol v45, player/chunk schema v9, metadata v6, companion v5, hostile v2, passive v1, engine ABI v11, and client ABI v19 remain unchanged. No live save or wire migration is involved.
- Concurrency/performance: immutable borrowed inputs, exclusive scratch and fixed per-call capacities; timing is informational, while overflow, partial publication, data loss, and I/O are hard failures. No hot-path blocking or unbounded allocation is introduced.
- User outcome: future Rust server/client-core work can compile and test against stable numerical contracts before every implementation lane finishes. Current Go authority and default startup remain unchanged.
- Non-goals: Rust server authority, Godot presentation, new gameplay rules, ABI redesign, concurrent online writers, or complete F1 acceptance by task-file status alone.
- Rollback: revert each reviewed numerical lane and its test-only corpus fragment independently; the old Go product path, existing ABI, fixtures, and saves remain available.
