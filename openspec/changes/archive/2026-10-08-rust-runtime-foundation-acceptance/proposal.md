## Why

The numerical closure is accepted and archived, but complete F1 reconciliation still rejects `domain.input/45`. Its single real Go authority case is successful; the existing rejection test deliberately permits this remaining non-kernel gap. F2 needs an honest, independently sealed prerequisite instead of treating that test's exit code as complete acceptance.

## What Changes

- Add source-bound rejection evidence from the real Go authority without relabeling its world/inventory output as a pure Rust domain result.
- Add a mandatory complete-acceptance test that rejects every uncovered family/version and verifies the entire discovered coverage set.
- Execute the existing Rust domain ordering, protocol, storage and numerical consumers on one integrated baseline and seal the final F1 evidence for F2/F3.

## Capabilities

### New Capabilities

- `rust-runtime-foundation-acceptance`: Fail-closed final foundation acceptance with explicit Rust-contract versus transitional authority evidence.

### Modified Capabilities

None. Existing domain, protocol, storage, kernel and authority behavior remains unchanged.

## Impact

- Affected files: offline Go producer/tests, Rust ordering tests and corpus source bindings, frozen runtime-migration evidence, and prerequisite ledgers. Production APIs and algorithms are read-only.
- Compatibility: protocol v45, player/chunk v9, metadata v6, companions v5, hostiles v2, passives v1, engine ABI v11, renderer client ABI v19 and scenario v23 remain unchanged.
- Concurrency/performance: offline tests only; no hot-path, queue, allocation or online authority change. No timing threshold is introduced.
- Outcome: controllers can determine whether F1 is fully accepted before dispatching server/core implementation.
- Non-goals: F2 rules, Rust authority parity, a new CLI flag, coverage exemptions, archive rewrites, save changes, visual updates or default cutover.
- Rollback: revert this successor's tests/fixture/source bindings as one unit; retain accepted numerical implementation and all unrelated corpus evidence.
