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


## Ordered projectile impact settlement

Projectile flight settles each hit in the current actor/inventory/runtime
overlay before scanning the next projectile. Deferring raw damage until the
later combat/death phase was rejected because the next projectile must skip a
target killed by the preceding projectile. Each hit stages removal and all
target changes atomically, reuses the accepted armor helper, and emits only
the existing owner confirmation. It does not also queue a damage intent.
Death/reset/drop ownership remains later in the fixed tick order. The exact
algorithm and red/green cases are in the refined projectile packet.

The serial reducer must pass actual damaged player identities to sleeping
settlement; a wire CombatHit recipient can be an attacker and is not a victim
identity. It must also discard commands from sessions retired after admission
but before execution. Both remain integration gates until their real consumers
are implemented and tested. No wire/save schema changes follow this decision.


## Persistence provider continuation contracts

Decoded loads preserve F1 migration facts in LoadedValue rather than converting
to SaveValue. Region commit reports committed keys and an error independently,
matching per-key S3 acknowledgment. A per-request cancellation token is checked
before durable publication, with no cancellation after publication begins. A
shared narrow I/O hook exposes actual failure/crash boundaries and partial/zero
writes to integration tests; ordinary constructors always use native I/O.
Native fallible close is confined to two platform adapters. The exact landing,
provider signatures and retry semantics are in
[the persistence continuation packet](plans/05-persistence-io-contract.md).

## Indexed Ready world observations

Rule and snapshot consumers share checked compact Ready bases and exact keyed
overlay reads. Missing cells are not inferred as air; sky uses the source's
highest non-air column including transparent blocks. Base validation and height
construction happen off the tick, while bounded writes update heights before
the next provider reads them. Replay snapshots preserve base content and
increment each changed chunk once, separately from per-cell staging CAS.
[The Ready world packet](plans/06-world-read-and-random.md) owns the accepted
contract and random-rule refinement. Actual reducer integration remains open.

The [exclusive backend packet](plans/07-disk-backend.md) fixes world-lock order,
metadata sequence initialization, bounded region ownership, partial durable
acknowledgments, and source-compatible named backups for the real disk join.

The [drop ownership contract](plans/08-drop-contract.md) fixes 36 input stacks,
32 persistent slots, ordered atomic staging and tick revision semantics before
any drop-producing or pickup provider is accepted. Counter-only aging retains
the verified Go dirty-selection behavior; this migration adds no save guarantee.

The [drop lifecycle packet](plans/09-drop-lifecycle.md) consumes accepted slots
independently of loot producers. The reducer retains the radius2 active-interest
union and publication ownership; command and producer acceptance stays separate.

The [world output contract](plans/10-world-output-contract.md) reconciles fixed
container slots and complete mining/system transactions. Internal chunk dimension
remains separate from the Overworld-only v45 container wire reference. Exact
integer source cells own block-generated loot; float centers are observations.

The [container view/drop packet](plans/16-container-drop-views.md) replaces
history-based first-move binding with the actual Ready ray-hit reference and
seeds one complete tick viewer set, so close cannot fall back to a stale lease.
A container write is an explicit durable touch even when an inventory-region
transfer leaves its slots equal, matching the Go revision barrier. Reach
invalidates views during publication after transfers; reducer scheduling and
workbench anchor ownership remain explicit follow-ups.

The [combat input contract](plans/17-combat-input-contract.md) freezes typed
hostile melee choices before independent action/settlement implementations.
Source audit requires pre-player-physics action facts and post-motion combat
validation, hurler-specific ranged strategy, and all-Ready death spill order.
Earlier provider tests do not accept these missing integrations. Workbench
anchor lifetime and container mutual exclusion also remain an explicit node.


The [melee provider packet](plans/19-melee-settlement.md) freezes bounded combat
actors and intents before cooldown mutation, preserves live identity checks and
mutual lethal hits, and returns actual damaged player sessions separately from
attacker-directed hit receipts. Death and cross-phase runtime merging remain
explicit integration work. The existing exhaustion calculation becomes one
pure configured-threshold helper shared by survival and melee.


## Whole-branch review correction

The activation executable at the review baseline only holds a world lock and a control socket; it neither ticks nor serves gameplay. This does not qualify the Rust authority described by the proposal and delta specification. The prior ledger ruling that deferred production transport, chunk loading, mob recovery and publication until default cutover is withdrawn. Leaving default startup on Go is a deployment constraint, not an exception to F2 behavior or acceptance. Node 3.7 is reopened: identical Rust transcripts, source fingerprints and directly constructed save records do not establish full real-runtime parity or durable mutation. Nodes 3.8 and 4 remain pending until those real paths and the previous-runtime rollback verifier execute.

Bounded review fixes first preserve existing ownership: movement clones the current actor runtime and changes only held controls; the tick-start environment uses committed live state after initial metadata hydration; sleep anchors and eligibility persist with resident state. Terminal transport records release payload ownership while retaining only bounded diagnostic replay. Agent I/O uses one absolute deadline across all reads. Exact tests and file ownership are in the review repair packet. These fixes change no wire/save version or gameplay rule.


### Source-exact shot clock and transient retirement

Verified Go hostile shard production uses executing authority tick, independently of calendar world time. The review correction in plan29 supersedes packet21's earlier world_time spread input; salt, float order and version remain unchanged. Session retirement prunes only transient sleep participation/anchors immediately, while durable respawn and latest-save/history ownership remain separate. Placement plant and torch support predicates follow current collision and farming source facts; body overlap and commit read bases retain separately qualified serial owners.


### Source order for death and placement geometry

Player death output rehearses candidate chunks in ring order, then inventory and armor slots within each candidate, retaining successful fixed-slot scratch state for later attempts before one compound publication. Placement player overlap uses the current Go target/default-form gate with a separate full-cell torch gate; it does not add new bed-head or door-upper collision rules. Exact private shapes, float order and refusal precedence are frozen in plan29.


### Frozen lease business classes

Freeze removes the current planner lease and fences late planner outcomes while retaining only the unexpired finalization/release identity. Commit/reconcile/delete/run cleanup may use that identity until successful release or expiry; plan/dialogue cannot. Admission and late-result checks share one private eligibility policy. Terminal transfer and memory finalization retry remain separate acceptance nodes.

### Production resident and durable capture ownership

Packets52/53 replace resident clone-out/full replacement and whole-world defensive rollback with exclusive moved ownership, changed-key commit and initial touched-key undo. Explicit replay/save snapshots remain off-tick. Drop returns retained ownership without inventing successful publication or global rollback. Private compound aggregates cap writes/captures/read bases independently at the existing4096 effect ceiling, preventing cross-component multiplication.

Packet55 uses persistent fixed128-cell pages routed by ten page-index bits for immutable chunk save captures. A write copies one fixed route/page; selection shares one capture Arc; materialization and compression run on the existing store owner. This avoids whole compact/body cloning, unbounded delta layers and a second rebase/compaction state machine on authority. Capture identity is O(1); actual codec equality after off-tick normalization retains existing equal-revision write conflict checks. Fixed current slots accompany block roots so slot age and non-dirty counter semantics remain source-exact. Actual acquisition/durable selection consumes these accepted prerequisites serially; provider acceptance does not close the production executable gate.
### Sealed previous package and current-world verification

The opt-in consumer requires the accepted sealed previous-runtime package, binds its exact source/oracle/server/verifier/native identities into the activation record, and revalidates them before stopping a current writer. Compatible rollback is based on the actual previous Go all-family read-only verifier against current quiescent data, rather than activation-time byte equality. Restore also verifies the installed named backup before the previous writer starts. Packet59 freezes exact shape/path/hash/timeouts/report/phase/error behavior. This independent consumer prerequisite does not close gameplay restart or the outstanding lease-span review requirement.
### Managed live chunk acquisition

Packet60 freezes a distinct enabled managed mode on an initially empty authority world, preserving legacy sparse replay fixtures. A bounded book owns only current wanted/resident/pending facts, scalar generations and16 prepared completion transfers; actual off-tick loaded/generated data installs after companion actions and before physics. Central reads and staging treat all non-Ready phases as unavailable while immutable Unloading data remains retained for the later durable/reclamation consumer. Source pending-key discovery establishes conservative36660 wanted/36676 managed ceilings; source overflowing anchor enumeration is not copied. Full request driving, goal/safe placement and durable unload remain serial real-provider successors.

### Restore ownership across directory installation

Packet65 replaces the momentary FREE probe and retire-before-install gap with one retained native flock, a staged hardlink to the original lock inode and native atomic directory exchange. The canonical world remains present and shares the same lease through copy, installation, verification and checkpoint publication. Recorded original directory identity distinguishes crash roles even when body hashes are equal; the old directory then retires under the same guard. Unsupported native operations, foreign roots and historical split lock lineages refuse with retained diagnostics. The sealed Go verifier and subsequent writer acquire their own unchanged locks after restore completes; this does not claim descriptor handoff or full runtime qualification.

### Qualified prepared publication

Packet66 lands an opaque immutable canonical-frame owner and explicit Queued/Closed result before the actual source snapshot publisher or background encoder consumes a new delivery surface. The off-tick factory preserves the real protocol key/codec bytes; clone and transfer share one Arc. A new narrow port leaves legacy publication signatures unchanged and makes successful enqueue the sole mirror-version advancement condition. Contract doubles/factory tests establish types and caller semantics; actual outbox/transport/encoder lifecycle and global retirement are subsequent real-provider nodes, not inferred from legacy publish returning Ok.
