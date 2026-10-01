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

Packet67 gives the actual authority one shared prepared/legacy FIFO and a separately bounded current-session index; borrowed endpoints and Memory transfer owners without body copies. Packet68 adds a prepared TCP lane to the existing shared send queue, preserving its partial head, identity checks,512-frame capacity,64-write/1MiB work budget and control-only acknowledgment ledger. These deliveries do not make a runtime publisher or reclaim retained authority history.

Packets69/70 specify a separate CPU encoding lane rather than on-tick encoding or disk-owner serialization. A compile-ready ChunkEncodePort contract precedes its actual provider and future source publisher. Opaque EncodedChunkSnapshot retains exact ChunkSaveView identity, source section-only payload charge and one PreparedFrame; revision equality alone cannot correlate captures. The provider owns at most8 queued/started/held requests across1..2 independent codec threads. Started cancellation remains charged until real completion; retryable explicit close joins all owners before success. Factory encoding is off tick and no eligibility policy crosses that CPU port. Actual source publication must independently qualify Ready/wanted/current captures and advance mirrors only on Queued; existing save captures also admit Unloading. Per-lane bounds do not establish a global runtime allocation bound or full executable acceptance.

Packet71 corrects only the actual TCP delivery fixture: each receive iteration drives scheduler, nonblocking input, core poll and output. Direct endpoint readiness may precede core LoginSuccess queuing; Prepared remains distinct from transport-acknowledged Active. A deterministic actual background-store case pins this ordering. Production transport and the encoding contract remain unchanged; serializing a test suite or increasing deadlines is not qualification.

Packet72 groups sparse observations into exact ChunkKey-owned BTreeMaps without resetting per-cell CAS history. Current cell counters and durable chunk revisions can differ after multiple writes and later commits, so clearing sparse observations at commit is rejected. Resident/context/read-view collection operations preserve current logical ordering and values; a crate-private whole-tree detach performs no per-cell work. Later physical reclamation owns detached-body destruction, acquisition identity and source schedule/lease policy; grouping alone does not retire a chunk or establish global bounds.

Packet73 fixes duplicated ownership accounting for a single admitted OpenContainer envelope across distinct container/workbench roles. One tick-local envelope with bounded per-phase ordinal references preserves every current settlement occurrence and its order, including repeated same-phase delivery, but charges the4,096 physical command ceiling once. Invalid controls retain both pre-deferred Motion and subsequent unowned Interaction roles; each phase independently admits at most4,096 references. Full ordering scalars distinguish fixture arrivals and conflicting same-identity payloads refuse explicitly. Nonconsuming phase reads and current watermark/error policy remain unchanged; this removes the genuine2,049-open internal overflow without claiming source target timing, a hard-error policy or full runtime acceptance.

### Detached chunk disposal ownership

Packet74 lands a noncopying opaque retired-body owner and an eight-charge retirement port before the actual CPU disposer or live AuthorityState erasure consumes it. One worker owns destructor work independently of disk/encoding. A refused submission returns the entire Ready/fixed-slot/sparse-tree owner for whole restoration; successful transfer removes authoritative residency only after source clean/unwanted/no-flight checks. Completion reports are scalar key/generation values, and held reports remain charged until collected. Explicit deadline close retains disposal/join obligations and cannot cancel/replay accepted work. Generation sources never recycle. Source future-due environmental entries and scalar view/anchor references are not blindly purged, and external immutable captures may still share underlying pages. Contract-double acceptance is separate from actual thread destruction and actual disk-unload-reload evidence; no global memory or executable claim follows from this lane.

Packet78 supersedes packet27’s infallible reducer/error-continuation and injected-only final adapter decisions. A trusted dispatch/delivery error or caught provider unwind ends tick advancement and becomes a retained authority failure. The same actual reducer serves normal and unpublished final reduction; partial accepted in-place writes are retained without a whole-tick rollback claim. Moved schedules return even on context-stage unwind; no new captures, selection, metadata target or managed transitions may expose partial state to persistence. Already-selected immutable work remains owned and can finish. Actual failure permanently blocks FinalTick/flush in both shutdown machines, while ordinary typed command refusals and unrelated transient shutdown retries keep their existing semantics. Cold-abort quiescence/release belongs to later production composition. Exact six-path algorithms, original source-scanned order refresh and real versus injected evidence are in78; acceptance is independent of full runtime.

Packet79 qualifies pure settled actor projection before production actor-target retention. Player pose/inventory live outside the original ActorBody; source-compatible save fields must come from their current owning overlays, with checked hunger narrowing and source respawn conversion. Hostile/passive health and attack/hurt fields remain their body provider's canonical values; neutral runtime fields must not erase them. Companion bodies use current pose/fixed inventory while memory/task/lifecycle aggregates remain separately owned. A real login/native tick/background disk/reopen recipe qualifies player field flow, not authoritative actor target ACK, bootstrap, pending restoration or runtime flush. Exact field/refusal/source ownership and bounded five-path scope are in79.

Packet80 qualifies one shared borrowed source geometry utility before player/companion lifecycle consumers. Whole-footprint Ready gating precedes restore collision; Current may be airborne, Safe requires complete footprint-cell support, and any ground contact remains a separate fact. Reduced block tops, source outheight-air placement reads, water spawn downgrade and stable nearest-column order are explicit. Checked spans/anchor enumeration avoid source overflowing loops without a new valid-coordinate clamp. Pure single-column work/allocation bounds do not authorize an unqualified whole-radius tick scan. The exact four-path API/provider/examples/tests are in80; pending scan cadence, state ownership, death/reset, subscriptions and executable composition remain separate.

Packet81 qualifies one bounded pending scan owner over accepted80 geometry. It retains unassessed Current before Safe, player rejected wanted keys versus companion clearing, source same-call ready-column cadence, strict tier improvement and deliberate stale fallback across readiness waits. Exhaustion observes at most81 sorted Ready revisions and performs no geometry until a relevant change. Player reset preserves captured anchor/radius and dimensionless spawn positions while switching to the current actor dimension. Exact structural work bounds and actual maximum-radius certified-air/current-write plus complete-water count probes make whole-owner costs explicit; measurements do not claim full-runtime50ms performance. Actor mutation/reset/death/safe/persistability/session-subscription/runtime consumers remain separate serial nodes.

The existing exact Ready non-air height cache certifies that every higher source row is loaded air, including transparent blocks and current accepted writes. Packet80 uses this optional certificate to skip only those source no-op rows, with a complete source row fallback for readers lacking a certificate. A verified empty column needs no384row scan. Literal no-certificate source read-order fixtures and certified bit-exact output/current-write equivalence are separate tests; the fast path changes neither chosen pose/tier nor activation cadence. Sparse observations and revision equality alone cannot supply the certificate.

Packet84 reuses AuthorityReadView for an O(1) healthy committed borrow. It excludes pending ingress and previous context transients, keeps the existing next-executable tick convention, and reads absolute time from an explicit World mirror, committed environment or startup metadata in that order. First hard failure fences new borrows and Closed refuses them. Actual login/native motion/actor-save projection/disk reopen consumes the borrow without resident clones; Agent, source publication, lifecycle selection and target ownership remain separate.


Packet83 integrates initial source player restoration through actual background login handoff and the post-Acquire/pre-player row. An explicit source mode preserves existing disabled replay fixtures, rejects direct allocation bypass, and owns at most8 whole pending scans with captured source anchor/radius. Missing storage has no Current candidate; actual Ready acquisition decides Current before supported Safe and fallback. Pending command gating precedes raw ACK and every provider/defer role so input/inventory cannot execute later in the activation tick; source close/resync/whole-container-defer exceptions preserve their existing downstream validation. The exclusive book returns after context drop on success/error/unwind, completed scans retain restart allocation, and local readiness/reset uses the existing final projector. Active unstick/safe/death, source subscription production, persistence eligibility/dirty/cache, companion lifecycle, source visibility and executable composition remain serial followups; manually driven actual chunk wants are explicitly limited evidence.

Packet86 lands a private O(1) in-place reset mapping before the active recovery and unified death consumers. A checked completed Player scan exposes only its captured anchor; the mapper changes fixed actor/runtime fields without cloning a durable body or an arbitrarily capacious public PathState. Generic reset preserves health/hunger and live bed/workbench; death fills hunger and health before invoking the same mapping. Actual scan restart, mining/sleep/action cleanup, crafting/drop settlement, source unstick and safe publication remain separately owned integration nodes. Local consumer doubles qualify the contract only.

Packet87 separates managed physical collision reads from mutation/CAS observations. Enabled acquisition returns AIR outside[-64,320) before horizontal readiness, as sealed Dimension.BlockAt does; in-height and disabled sparse reads keep the existing observation path. The private borrowed method changes no factory field or failure/clock lifetime. Executed grid/geometry input doubles land before all four native actor-grid consumers; actual native below-world motion and source recovery remain separately qualified.

Packet88 consumes that shared read in exactly the four native actor-grid sampling loops, retaining native unknown blocking, shapes, prism bounds and tuning. Actual background login/Ready player fall and top-plane jump, real acquired-world companion/passive/hostile native motion, missing-column blocking and disabled sparse controls qualify producer consumption. Passive/hostile below-MinY removal retains its existing lifecycle. The top-plane oracle starts feet318/head319.8 because native clipping ignores already-overlapped cells; positive travel beyond318.3 is causal against the old unknownY320 ceiling. Actor setup seams do not accept bootstrap or full runtime, and source active recovery remains a later owner.

Packet90 integrates the accepted89 common context reset into actual active recovery after regen/eating/bow and reset short-circuit, before oxygen/native motion. Geometry checks original pose followed by sixteen original-Y sixteenths, stopping on first unavailable or free pose. A lift changes position only and rebases the existing pre-step motion to avoid charging recovery displacement. An unsuccessful search or Y below -80 maps Pending without death refill and restarts the same captured scan in the current actor dimension; scan advancement waits until the next tick. Checked finite geometry retains existing typed span refusal rather than copying source overflow. Actual native fall/reacquisition, embedded, blocked and unknown-footprint recipes qualify this serial consumer with manual wants. Early legacy death, unified death settlement, Safe/Snow, action costs, inventory dirty/publication, source subscriptions, actor saves/bootstrap and executable composition remain separate unfinished ownership.


Packet91 prepares one zero-health source player death through indexed ownership before actual phase routing. Fixed inventory/crafting repack is lossless or a hard invariant; cumulative ring-drop previews clear only accepted slots. The source-only Ready cardinality guard uses the existing36,676managed ceiling before enumeration, including protection against public fixture injection. Live runtime bed state owns proof of unavailable versus invalid foot/head and a fixed optional candidate; both bed halves consume accepted PlacementWorld::block_at, whose outside-height AIR precedes horizontal readiness independently of acquisition mode. Same-DIM out-of-height beds clear, other-DIM beds remain untouched and checked-neighbor overflow retains an unverifiable bed. Durable body/path/independent sleep/view owners remain retained. Inventory/drop acceptance precedes fixed full-health/hunger reset, own receipt removal and the accepted common Pending mapping. A Copy restart payload and explicit source-session mode query land with prepared consumer examples before early/late death routing, same-scan restart and all five actual damage producers. This boundary accepts no source publication/Snow/save/runtime behavior.

Packet92 consumes accepted91 prepared death through one serial source-book consumer after projectile/hostile death and before passive work. Source-only zero-health survival defers before clone/environment/runtime and legacy late collection excludes the same actual sessions. The same sorted bounded book entry restarts in the current actor dimension with zero/one live-bed candidate and advances on the following tick. Five actual damage inputs execute background disk, Memory ACK and Ready/native fixtures; native landing has a separate subthreshold fullTick control. Hard crafting failure retains accepted damage and returns moved owners through the existing failure fence. Exact nine-path ownership/tests/bounds and unchanged legacy semantics are in92; Snow/Safe/publication/actor-save/full executable composition remain separate.
