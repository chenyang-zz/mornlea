## Context

Complete F1 is sealed at source `d042982d33bb1694d768b75b01c297bd02534a08` and corpus `sha256:8b5813ecce2fe866ab1c4786a35925e8aadd9aab476196abb189d3c72d4f28f3`:112 points,1434 cases,zero gaps. Its D0/P0/S0/K0 APIs are implemented. Go remains the production authority. F2 supplies a separately qualified opt-in Rust server; P14 controls default switch.

The [target interface map](../../../docs/runtime-interface-architecture.md) assigns S1–S4. The [exact declaration packet](plans/02-core-seams.md) and [algorithms/test tables](plans/04-refined-nodes.md) complete the design. Node1.2 must land compile-ready S1/S3/S4 declarations, checked values and executing consumer doubles on an accepted SHA before providers;3.2 similarly lands S2 before either transport adapter. Planned signatures have no runtime acceptance.

## Goals / Non-Goals

Move existing server behavior to one Rust owner while preserving external outcomes. Do not add gameplay, client presentation, implicit save conversions, concurrent Go/Rust authorities or a Python simulation loop.

## Decisions

### One server core, two transports

Add `packages/engine/crates/mornlea_server/` depending on F1 domain/protocol/storage and `mornlea_engine`, without dependencies on Godot or the client core. Expose one bounded server-core entry path for in-memory and TCP adapters. Session state, tick order, deterministic commands and persistence observations remain core-owned; adapters never bypass login/validation. Keep immutable cross-thread publications, explicit capacity/overflow decisions and cancellation ownership.

Create the crate's `AGENTS.md` and update the workspace guide. A capability inventory under `testdata/runtime-migration/server/` maps every current authoritative feature and failure contract to a Rust test/replay; unsupported coverage blocks F2 completion.

### Migrate bounded capability groups

Start with session/tick scheduling, then world/chunk/environment/fluid work, player movement/actions/inventory, actor/combat/drop/companion rules, and persistence coordination. Numerical work calls the one kernel implementation. Before each group, add failing replay and failure-path tests from its declared corpus. A group too large for one session must be broken down in this change's tasks with exact tests and ownership before coding; do not declare a partial server complete.

### Persistence and independent Agent

Storage workers own blocking I/O with bounded submission/acknowledgment and shutdown policies matching the current contract. Use test copies for migration and crash injection. The [S1 compile-ready seams](plans/02-core-seams.md) define one atomic world/inventory transaction, a separate sessionless companion ingress, measured budgets, save scheduling and retryable final-tick shutdown. Preserve the existing versioned loopback Agent HTTP/MCP boundary, cancellation and revalidation at the tick boundary. Do not embed, shell out to or import the independent Agent into the game runtime. The embedded Godot Python environment is unrelated.

### Authority selection and rollback

Parity compares separate offline runs. Opt-in integration owns its temporary world exclusively; concurrent writers must be refused before opening writable state. An activation record names runtime, contract versions and save backup. Rollback stops Rust, verifies compatible data or restores the explicitly retained snapshot, then starts the previous release. It never silently rewrites newer saves. This stage may ship an opt-in server but does not change the default client or remove Go.

### Rejected alternatives

A live shadow server cannot validate safely by writing the same world. A Go fallback leaves Rust authority incomplete. Local shortcuts create a privileged second simulation path. Python callbacks in the authoritative loop violate language ownership and bounded execution.

## Risks / Trade-offs

- Replay agreement may omit error semantics → require malformed packets, save errors, saturation, shutdown and Agent timeout cases.
- Persistence differs by platform → test supported file/error behavior and retain backups before activation.
- Performance numbers can hide incomplete work → fail overflow/data loss/I/O errors while keeping timing measurements informational.

## Migration Plan

Verify the accepted complete F1 seal; freeze source-bound server coverage and measured bounds; implement independently verified core capabilities; run offline differential and transport/persistence failure tests; qualify opt-in activation and rollback. The archived baseline and any one successor are insufficient authorization to begin F2 implementation. F3 can consume an accepted protocol/session contract during development, but final integration acceptance needs the complete F2 result.

## Validation and completion evidence

Implementation follows failing contract/replay tests, minimum implementation, then refactoring. Test targets in `tasks.md` are prospective until their owning task registers them. Use the actual Rust workspace (`--manifest-path packages/engine/Cargo.toml`) and named integration targets; inspect `-- --list` output and reject empty discovery. Do not substitute text searches or unrelated optional-build audits for prerequisite acceptance.

Before any implementation task starts, record the final F1 acceptance source SHA, the accepted successor set, zero uncovered supported families and the complete acceptance command in `ledger.md`. Then record source SHA, corpus digest/coverage, command, discovered/executed tests, result, failure cases and rollback proof for F2 work. Rust stage completion requires its full declared inventory, not only the first successful slice. Commit each independently verified task; if an inventory item exceeds one session, refine it into explicit capability tasks before implementation rather than checking off a broad placeholder. Planning validation proves artifact structure only.

At implementation closeout run formatting, `make rust-check`, `make dev-check` (including all six Go-module vet commands), `make test-race`, `go test ./packages/audit -count=1`, and `openspec validate --all --strict --no-interactive`. Add the change-specific replay, failure-injection and platform gates. No graphical foreground window may be started by automated tests.

## Post-numerical-closure planning decisions

The [node dependency and ownership register](plans/03-parallel-readiness.md) and [refined node decisions](plans/04-refined-nodes.md) are normative execution inputs alongside the original packets. Preserve prior scope and separate source-contract, real-provider and real-integration acceptance. Complete F1 acceptance is recorded in the independently owned [foundation successor](../rust-runtime-foundation-acceptance/proposal.md).

The controller compared wholesale migration resequencing, immediate parallel dispatch from prospective signatures, and bounded refinement of existing nodes. Bounded refinement preserves reviewed scope and existing node identities while making dependencies, shared edits and source-information limits decidable. Immediate dispatch remains blocked by missing accepted contract/provider SHAs. Broad shared files stay serial; only disjoint providers with accepted predecessors can overlap.

## Review correction decisions

The controller used installed Superpowers brainstorming and writing-plans and chose explicit typed state/effect ports over mutable state exposure or provider-designed types. Intake preserves sorted-batch sequence semantics; resource refusal is internal/silent close where v45 has no matching reason. Receipts name earliest eligibility; carry preserves provenance. Atomic footprint/output transactions include tool wear, container removal and actor-specific products. Rule packets include every accepted command plus random world branches; shared algorithms/expected fixture values are controller-authored.

Persistence uses existing MCGR/MCGB bank commits and standalone atomic replacement, OS world.lock and per-key partial acknowledgments. A journal/global checkpoint was rejected because it would change the compatibility oracle. Go selection/estimate budgets remain distinct from proposed Rust hard retained-ownership caps. Completion backlog is measured explicitly before bounds acceptance.

Agent HTTP remains schema v1 with no source_tick/attempt fields; private u64 attempt/source_tick distinguish callbacks from game world time. HTTP/lease, frozen snapshot/MCP, task/dialogue/memory and real-process gate have separate nodes. Test-only process owners launch the real Python gateway with deterministic model fixtures; production never launches or embeds Python. Shutdown exposes once-only phases and a failure report, retaining frozen Agent/world ownership on retry. Release is not given a fabricated idempotent receipt.

Opt-in activation and rollback execute stop/wait/verify-or-restore/start against selected real binaries and disposable worlds. Dry-run remains inspection only. Parallel packet/file/DAG updates preserve accepted F3 S2 alias and all F2 checkboxes remain pending.


## Projectile contract completion

Continuation discovery found that the declared projectile before/after effect
only appended records, and fixture initialization discarded projectiles and actor
runtime. Node 2.7b0 completes the accepted declaration before provider work:
immutable reads, exact-record compare-and-replace/removal, bounded ordered
compound preflight, and fixture continuity. A provider-private projectile store
was rejected because it would create a second owner and bypass the tick overlay.
The refined packet defines error precedence and tests; 2.7b consumes its accepted
commit, while 3.1 retains real reducer ownership.
