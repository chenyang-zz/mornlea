## Context

The accepted F1 baseline supplies only a reviewed subset of shared Rust contracts and offline evidence. The archived domain-event successor and later protocol, storage, numerical-API and pathfinding successors must close before final F1 acceptance. The Go server remains the current production owner. F2 creates an opt-in Rust replacement only after that acceptance; P14 separately controls the default product switch. The complete rule inventory must be reconciled before claiming parity.

## Goals / Non-Goals

Move existing server behavior to one Rust owner while preserving external outcomes. Do not add gameplay, client presentation, implicit save conversions, concurrent Go/Rust authorities or a Python simulation loop.

## Decisions

### One server core, two transports

Add `packages/engine/crates/mornlea_server/` depending on F1 domain/protocol/storage and `mornlea_engine`, without dependencies on Godot or the client core. Expose one bounded server-core entry path for in-memory and TCP adapters. Session state, tick order, deterministic commands and persistence observations remain core-owned; adapters never bypass login/validation. Keep immutable cross-thread publications, explicit capacity/overflow decisions and cancellation ownership.

Create the crate's `AGENTS.md` and update the workspace guide. A capability inventory under `testdata/runtime-migration/server/` maps every current authoritative feature and failure contract to a Rust test/replay; unsupported coverage blocks F2 completion. Frozen inventory rules and worker packets are in [plans/00-capability-inventory.md](plans/00-capability-inventory.md) and [execution-contract.md](execution-contract.md).

### Migrate bounded capability groups

Start with session/tick scheduling, then world/chunk/environment/fluid work, player movement/actions/inventory, actor/combat/drop/companion rules, and persistence coordination. Numerical work calls the one kernel implementation. Before each group, add failing replay and failure-path tests from its declared corpus. A group too large for one session must be broken down in this change's tasks with exact tests and ownership before coding; do not declare a partial server complete.

### Persistence and independent Agent

Storage workers own blocking I/O with bounded submission/acknowledgment and shutdown policies matching the current contract. Use test copies for migration and crash injection. Preserve the existing versioned loopback Agent HTTP/MCP boundary, cancellation and revalidation at the tick boundary. Do not embed, shell out to or import the independent Agent into the game runtime. The embedded Godot Python environment is unrelated.

### Authority selection and rollback

Parity compares separate offline runs. Opt-in integration owns its temporary world exclusively; concurrent writers must be refused before opening writable state. An activation record names runtime, contract versions and save backup. Rollback stops Rust, verifies compatible data or restores the explicitly retained snapshot, then starts the previous release. It never silently rewrites newer saves. This stage may ship an opt-in server but does not change the default client or remove Go.

### Rejected alternatives

A live shadow server cannot validate safely by writing the same world. A Go fallback leaves Rust authority incomplete. Local shortcuts create a privileged second simulation path. Python callbacks in the authoritative loop violate language ownership and bounded execution.

## Risks / Trade-offs

- Replay agreement may omit error semantics → require malformed packets, save errors, saturation, shutdown and Agent timeout cases.
- Persistence differs by platform → test supported file/error behavior and retain backups before activation.
- Performance numbers can hide incomplete work → fail overflow/data loss/I/O errors while keeping timing measurements informational.

## Migration Plan

Accept complete F1, including the domain-event, protocol, storage, numerical-API and pathfinding successors; freeze server coverage; implement independently verified core capabilities; run offline differential and transport/persistence failure tests; qualify opt-in activation and rollback. The archived baseline and any one successor are insufficient authorization to begin F2 implementation. F3 can consume an accepted protocol/session contract during development, but final integration acceptance needs the complete F2 result.

## Validation and completion evidence

Implementation follows failing contract/replay tests, minimum implementation, then refactoring. Test targets in `tasks.md` are prospective until their owning task registers them. Use the actual Rust workspace (`--manifest-path packages/engine/Cargo.toml`) and named integration targets; inspect `-- --list` output and reject empty discovery. Do not substitute text searches or unrelated optional-build audits for prerequisite acceptance.

Before any implementation task starts, record the final F1 acceptance source SHA, the accepted successor set, zero uncovered supported families and the complete acceptance command in `ledger.md`. Then record source SHA, corpus digest/coverage, command, discovered/executed tests, result, failure cases and rollback proof for F2 work. Rust stage completion requires its full declared inventory, not only the first successful slice. Commit each independently verified task; if an inventory item exceeds one session, refine it into explicit capability tasks before implementation rather than checking off a broad placeholder. Planning validation proves artifact structure only.

At implementation closeout run formatting, `make rust-check`, `make dev-check` (including all six Go-module vet commands), `make test-race`, `go test ./packages/audit -count=1`, and `openspec validate --all --strict --no-interactive`. Add the change-specific replay, failure-injection and platform gates. No graphical foreground window may be started by automated tests.
