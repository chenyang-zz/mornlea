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

Late death eligibility retains the source Active lifecycle gate. Earlier recovery may convert a just-starved player to Pendinghealth0 with inventory retained; real scan activation later makes that player eligible for one death settlement before publication, followed by the next restore. An exact-library public-fixture probe executes this intersection and confirms the existing Go ordering. No early refill or health-specific recovery override is introduced; the blanket goal wording is qualified to players still Active at late selection.

Packet93 adds a private source Safe geometry predicate without restore height/health/dry/any-contact restrictions, and updates only the indexed heap-free Safe location after each player finishes native motion and fall settlement. A bounded registered-book lookup qualifies this existing-loop call, retaining reset/recovery skips and prior accepted writes before a later failure. Whole-footprint Ready precedes body/free/complete-support reads; existing source float collapse and outside-height AIR remain exact. Actual healthy/lethal native landing and top-row column activation qualify this checkpoint owner; source subscriptions, later footprint timing, publication/automatic actor saves/cache and executable composition remain separate.

The Safe integration updates the checkpoint after a successful positional lift that continues through native motion. The existing actual lift regression therefore compares its whole body against an expected pre-tick body whose only prescribed change is SafeSomeOverworld[8.5,65,8.5]. Other recovery/reset/refusal assertions and all shared helpers remain exact; this single test expectation follows source tryUnstick/native/Safe cadence.


## Actual source Safe checkpoint acceptance

Private Safe sampling is accepted after each actual source player's native motion and fall settlement, before later players and late death. The checked source-f32 footprint requires horizontal Ready cells before body reads, free space and complete support; it deliberately preserves fluid support, empty float-collapsed spans, outside-height AIR and zero-health eligibility independently of restore-height rules. The keyed ever-spawned book qualification and indexed context mapping write only the existing heap-free Safe value, retaining rich body/path allocations, the same restoration scan and accepted prefix mutations after a later refusal. Reset and failed-recovery continues bypass sampling; a successful positional lift continues through native motion and records its supported Safe pose.

The exact seven-file source532f7b90c3c5c669580e67440513ab5a67ea4c01 is independently reviewed and integrated as e9e79fce3acb64d76e0cf5b5469069950b080729. Author and independent review each execute1181distinctactual+3docs; ROOT executes1369distinctactual+3docs including all49activation cases. Only the named successful-lift expected-body assertion is revised under the recorded ruling; the shared whole-body helper and other30 persistence cases remain exact. Local geometry/mutation bounds do not accept full-loop allocation or50ms timing. Snow/trample source timing, subscriptions, dirty publication/action costs, automatic actor saves/cache/bootstrap and executable gameplay remain open, as do broad3.7/3.8/4.1/4.2. Architecture skill: no change.


## Actual source trample capture design

The next bounded integration node records each actual source player's landing geometry immediately after native motion/fall and before Safe, retaining copied dimension/position candidates through later death/reset until the original Trample write region. A private fixed32-cell batch travels with the existing exclusively moved SourcePlayerBook, adding no heap queue, actor-runtime field or public API. Existing checked placement endpoints qualify source-f32 horizontal spans and support Y without world reads, body-height or restore/support eligibility. Eight actual live players contribute at most four cells each in session/X/Z order. Capacity or malformed geometry refuses before a new append; accepted prefix candidates survive a later abandoned context through the existing moved-owner return and failed-authority fence.

The serial consumer reads fresh cells through the unchanged crop/drop/ground-first transaction owner and clears length only after a successful whole settlement; environment absence preserves pending coordinates. The public legacy collector excludes actual live source sessions so activation-only poses and lethal reset positions are not recollected; source-disabled fixture behavior remains exact. Retaining the full generic tracker map and moving world writes into motion were rejected. Snow/passive tracker lifetime, source rounding/order and all broader runtime/save/publication concerns remain separate open owners. Exact interfaces, thirteen TDD cases, source-only scope and1194author/reviewer versus1382ROOT actual gates are frozen in plans/94-source-player-trample-capture.md. No separate shared-contract landing applies to this single private serial deliverable. Architecture skill: no change.


## Actual source player Snow ownership

Packet95 retains one inline transient travel/cell tracker in each existing source player entry and one fixed eight-cell copied candidate batch in its exclusive moved book. Native/fall capture occurs after trample and before Safe, while original Snow writes stay late before generic Snow and random updates. Reset/death clears only that player tracker, preserving captured coordinates. Previewed tracker mutation commits only after a candidate capacity preflight; geometry uses checked floor of foot coordinates without a ground probe or gameplay-height clamp. Source f32 motion/squared distance/travel and f64 sqrt narrowing, dimensionless same-cell memory, one candidate per tick and no airborne reset remain exact. Existing legacy source-disabled fixtures retain their provider contract and exclude actual source players to prevent activation phantom/double sampling.

Retaining the generic schedule was rejected because it preserves unrelated unbounded tracker ownership and late timing; immediate writes would move the source region. This single private serial node needs no public/shared contract landing. Actual native18tick, lethal landing and activation recipes separate runtime behavior from prepared borrowing/capacity/reset and source-order evidence. Generic/passive Snow and later runtime joins remain independently open. User-directed continuation explicitly keeps advancing Linux implementation while Darwin/macOS-only stage evidence remains pending.

Source player Snow is accepted at integrated e6923f6d71325824a7f38e3217628e57c2b11f27, byte-identical to independently reviewed7c9552e8df73e0a17d6b33a67ac42080cb5ccb4d. Fourteen compiled assertion RED cases and actual activation/18-tick/lethal recipes precede final author/reviewer1208actual+3docs and ROOT1396actual+3docs, including all49activation cases against the current36538c6f release. This accepts only the player owner; passive lifetime/timing/slowdown, source subscriptions, publication/action costs, actor saves/cache/bootstrap, configured tick tunables and executable gameplay remain open. Architecture skill: no change.


## Late source player action cost ownership

Packet96 reuses the existing bounded4096 transient receipt lane, adding one successful human Mining producer and an indexed scalar-only actual-source consumer after Interaction and after Mining. It uses existing charge_milli/exhausted_state, preserves current clamp-to-u16 compatibility and direct melee100, and updates only current survival and durable/runtime hunger lanes before final PlayerState. Refusal preserves owner/receipt snapshots; later-player refusal retains earlier scalar acceptance while context Drop destroys transient receipts. No whole-owner clones, new public interface, save revision or rerun of PostPhysics is introduced. Actual default-threshold Memory/native Till/Mine recipes distinguish integration from prepared custom-threshold tables. Twelve concrete cases include ten expected assertion RED and two refused/incomplete baseline GREEN preservation controls. Custom configured tick tunables and all broad joins remain open; exact file/error/bounds/gates are in96. Architecture skill: no change.


The late-cost actual Till fixture uses pitch-0.65, corrected from-0.95 after real NativeRaycast demonstrated adjacent floor occlusion. It preserves the full floor63, dirt(8,63,6), separate mining pitch-0.5 and all source outcomes. Native setup failures are retained and excluded from RED; the corrected firstsolid native proof precedes the full inert rerun. No gameplay algorithm or old fixture is changed.

Late action costs are accepted at integrated f9afd5b93a88cda9b5017c96177191bebb77beba, byte-identical across the exact nine paths to independently reviewed bc1d068c31eda6a0ab195cbead494b8c32f4d3ee. Corrected compiled baseline10assertionRED/2GREEN controls, author/reviewer1220actual+3docs and ROOT1408actual+3docs pass. All49 activation cases select the fresh0eba2b79 executable and unchanged previous-v2 package. Only3.7l3ac closes; actual successful human melee endpoint evidence, configured tunables and the broader runtime/save/publication/every-outcome joins remain open. Architecture skill: no change.

## Shared nonplayer Snow scalar boundary

Packet97 reuses accepted PlacementWorld raw reads and checked source foot geometry to produce a copied PhysicsTuning with walk_speed multiplied by f32 0.7 only for grounded, moving actors over raw87/88. Quiet guards precede geometry; qualified geometry precedes source outside-height AIR and a single current raw read. Other fourteen fields and caller-owned pose/lifecycle/input/refusal remain untouched. At most three checked floors and one read/no retained allocation are local bounds. This common callable contract lands independently before the three missing companion/hostile/passive native consumers; six double cases accept scalar semantics only. Accepted player motion stays unchanged. Public additive internal Rust export is distinct from protocol/FFI/save ABI; actual acquired-world/native consumer recipes remain separately gated. No interface or implementation is accepted merely by this planning section.


## Source acquisition caller continuation

[Plan109](plans/109-source-acquisition.md) freezes one explicit library caller over the accepted source restore producers and borrowed request driver. Completion relevance is per settled event at Acquire; replacing the whole union there would prematurely unload unrelated Ready bodies. One dirty reconcile follows player/companion actor advancement before hostile/gameplay. Real background provider admission slots protect retained FIFO candidates from Capacity-induced Failed transitions. Pending spawn dirty retry is preserved separately from quiet saved-restore failure. The manual tick remains unchanged; assembled runtime, autosave/actor persistence, bootstrap and trusted-observer composition remain open.

The library caller is accepted at implementation `9fbfa118745f7f0043616a55322a4876a352d908`, whose server tree is identical to the dev-synchronized formal-review source `9811adb6fe50a40c7133d55b725b48e4dc71825d`. Fresh independent GPT-6.1-sol high review verifies actual provider paths, qualified RED/GREEN evidence, bounded ownership, manual controls and original test prefixes. The only Minor finding concerned missing positive private coverage of quiet generation enqueue alongside an unrelated Failed load. Direct Codex follow-up `d84c863871b94987fc50a1ae2f19c333aff0957c` adds one independently reviewed prepared-control case without changing production bytes or any prior test. The actual Disk/Native integration remains separate from that injected-completion control. The source-bound workspace run passes 3450 results including doctests with no failures or ignored tests; its pre-commit artifact identity is retained in the ledger rather than relabelled as a clean-SHA run.

This accepts no automatic save producer or provider shutdown composition: the backpressure case qualifies consumption of the existing externally driven latch, and the caller still borrows owners whose draining and close belong to its borrower. No protocol, save, ABI, dependency or default entry changes occur. The broader 3.8 and original4.1/4.2 remain open. Subsequent authoring is direct Codex work under the user's no-Loom ruling, using the existing canonical worktree. Architecture skill: no change; these ownership rules are already covered by the scoped guides and verified cross-task conventions.


## Full remaining-task execution ruling

The user requires completion of every remaining task using the canonical serial checkout and direct Codex authorship, without Loom, new serial worktrees, push or deployment. The approved F2 behavior remains the scope. The previous acquisition acceptance is retained; broad activation and zero-gap/full-stage gates remain open. Controller-owned packets qualify actual consumer nodes before implementation, and accepted nodes are committed serially. Native isolated agents collect source evidence and independently review; they do not author product code.

The first discovered missing command behavior is Crafting-view partial and quick movement. Packet110 routes the existing checked domain values to the existing crafting provider, stages one complete record after local repack rehearsal, and records existing owner-only inventory/crafting intent after successful staging. No public type or shared contract changes. Reusing inventory's region move would select the wrong target order; treating a crafting view as a container would invent a lease and mutation boundary. Both alternatives are rejected. Fixed bounded loops preserve the authoritative command-stage settlement.

Source census also identifies automatic player/aggregate save ownership and cache, loaded companion/mob bootstrap, background snapshot publication and mirror admission, command receipts and publication cadence/identity, trusted observer/Agent composition, configuration loading, executable tick/transport/shutdown composition, complete source outcome evidence and actual opt-in rollback as distinct remaining acceptance boundaries. A prepared provider or named fixture does not accept these integrations. Their exact interfaces and independently testable packets must be frozen by the controller before implementation; this discovery entry does not mark those nodes worker-ready or complete.


## Surviving drop wire-value projection

Packet111 retains complete copied checked ItemDrop values in the private session mirror instead of identity membership alone. It compares only wire-visible ID/cell/stack, publishes surviving-ID changes, and emits removes before upserts in existing sorted32-entry batches. Retaining whole DropRecord would republish every aging tick and retain unrelated lifecycle quantities; emitting an upsert for every visible record would also violate Go quiet behavior. Both alternatives are rejected. The actual queue-admission mirror transaction remains a separate owner; this private tick-end projection repair does not accept that integration or source-mode/executable composition.


## Mixed entity lifecycle event ordering

Packet112 moves each existing hostile/passive/projectile despawn batch before spawn and survivor-state construction, preserving the original membership preimage until the final private mirror assignment. This matches source capacity-release ordering without changing typed records, bounds, passive reason precedence or survivor selection. Mutating previous membership early would incorrectly turn arrivals into state candidates; a cross-family event sort would obscure each provider's stable batch order. Both alternatives are rejected. Prepared actor setup plus actual reducer/aging qualifies event construction only; source visibility, queue admission and automatic runtime composition remain open.


## Retained actor save target ownership

Packet113 freezes an ordered bounded ActorSaveLedger over existing SaveValue/OwnedSnapshot. It keeps at most16player and three singleton aggregate current values, one immutable selected target per key, exact codec estimates, separate durable revisions and caller-defined eligibility/pinning. Selection stamps persisted+1 only after atomic prefix preflight; an ACK compares complete immutable preimages and keeps later current/force dirty. Scheduler retry timing and disk ownership stay outside this owner. Revision normalization affects comparison only; current bodies never certify durability. A new additive callable owner lands with executing public consumer recipes before state/cache/bootstrap consumers cite its accepted SHA. No new trait is needed. Extending fixture dirty vectors into live state would mix unbounded unrelated owners, and capturing actors directly into the scheduler would lose latest/retry/cache ownership; both alternatives are rejected. Actual capture, missing confirmation, retirement/repack, aggregate merge, bootstrap and runtime remain separately gated.

Packet113 review correction: immutable target identity compares float bits, preserving codec-valid absent-respawn NaN and rejecting signed-zero forgery. Semantic latest-current comparison remains source IEEE; ACK recomputes dirty with a normalized target clone. Eligible revision-zero initial values require an explicit initial dirty obligation; clean absent baselines remain ineligible until source confirmation/change. These are not-yet-landed contract corrections; future consumers bind only the corrected accepted SHA.


### Reservation-aware scheduler progress

Packet114 keeps the mailbox whole-request admission contract, while the scheduler adapts an oversized multi-record cohort to an ordered noncopying prefix. Only Capacity permits halving fallback. Fresh tails return to their authority; retry tails retain original attempt/deadline and a unique child cohort, while accepted originals remain correlated with their exact ticket. Both due and final retry dispatch share the same bounded operation. Refusal diagnostics remain visible. Actual generated eight-chunk saves/reopen qualify real progress separately from scripted retry doubles. Actor-state/automatic capture/bootstrap/runtime remain pending.


Reservation adaptation also applies to fresh final flush; its admitted prefix counts as progress before stall detection. Mailbox codec/admission validation remains whole-request, so any non-Capacity submit refusal returns the complete selection. Additional decoded identities/conflicts remain actual DiskStore preflight for each admitted request. A selection divided across multiple requests is not a transaction across those requests, consistent with Go's separately dispatched region jobs. Refusal/partial durability is explicit. Prefix halving performs bounded O(n log n) owner moves, with no extra payload clones or I/O.


### Common authority actor persistence owner

Packet115 consumes accepted ledger4247ae4a5 and scheduler3c526dd26. The opaque authority owns an opt-in ledger through a private child implementation unit, exposing checked trusted-producer retention/observation and borrowed current facts. Nonempty actor/chunk selections alternate, while empty urgent passes do not consume preference. Per-family bounds remain19actors/eightchunks; enabled mixed completion preflights at most28unique targets before any durable mutation. Exact refused owners and admitted failure retries remain distinct; statistics/frozen keys include actors. Closing flush may select existing obligations but refuses new producer observations. Automatic gameplay capture/cache/bootstrap remain separate consumers.

Authority actor routing accepted: opt-in private ledger joins live SaveAuthority, with nineteen actor/eight chunk/one metadata bounded mixed preflight, both actor and chunk exact held-identity rejection, immutable failed-flight retry, actor/chunk group alternation and Closing flush. Actual generated chunk plus explicit actor producer fixtures persist and reopen through AutosaveScheduler/background DiskStore. This accepts common routing and real storage consumer only; automatic gameplay capture, cache/prepare/retirement, aggregate bootstrap/config and executable remain future source owners.

Source player persistence next design uses one bounded16 lifecycle cache table beside the accepted ledger, not a second body/revision owner. Cache lookup precedes physical load; loaded rewrite and quiet source Missing baseline remain distinct, name confirmation is staged, settled capture uses only8indexed current players, and disconnect rehearses all9crafting cells before force capture and physical player release. Opt-in LoginDriver logical ticket namespace preserves original provider correlation and disabled behavior. Private state-child orchestration is a single coherent feature; linkedpacket116 freezes exact APIs/error/bounds/producer seams and explicit actual-versus-fixture acceptance. Existing user all-task/direct-authoring ruling applies; no redundant skill approval or second plan store.

Player persistence is now an explicit healthy pre-tick library opt-in over the accepted actor ledger. Sixteen bounded lifecycle records include loads, while eight indexed current source players feed settled ordinary/final capture. The real borrowed login endpoint uses logical receipts and original provider aliases separately, checks cache before any disk admission and confirms staged names/missing eligibility only on successful login. Exit rehearses every crafting cell and captures before physical swap removal; typed refusals and sticky failed settlement preserve ownership. Actual background disk/load/save and Memory/TCP consumer evidence is distinct from prepared source lifecycle controls. This does not accept automatic companion/mob aggregates or the assembled executable.

Source mob persistence joins validated off-tick complete hostile/passive bootstrap, bounded Active admission and complete ordinary/final aggregate capture through the existing ledger. The private authority owns no second body/flight cache. Terminal residents are retained until passive Died/Vanished publication consumes their identity, then bounded swap removal repairs the sole enduring player index and releases only identity-local runtime/path ownership. Prepared all-family startup precedes any mutation; missing empty baselines remain quiet until actual roster observation. Both aggregate codecs/projections are validated before either save owner advances. Resource::Actors bounds the opted-in actor work110 and Active64/32; disabled fixtures remain unchanged. Packet117 qualifies actual DiskStore/native settlement and complete saved bodies, including identical older retry bytes and newer final durable revision. Prepared lethal passive, quiet pose/timer and shard inputs remain distinct from the real downstream providers. The assembled executable and complete paired source outcome inventory remain open.

The next companion prerequisite is a pure source-compatible configuration merge in storage, before any runtime, task or memory consumer lands. Missing/legacy records require canonical namespace/lifecycle migration; v5 keeps stored bodies, unchanged epochs/queues and separate memory revisions, while configuration transitions increment epoch and clear stale task/mirror ownership. Entropy remains an explicit fallible caller port with source canonical call order; both inputs stay borrowed and only a completely validated v5 result escapes. The existing codec's borrowed-field canonical helper validates bounded strings/tasks before copies. Packet118 freezes this compile-ready contract and real unchanged Go merge/encode outcome fixtures; startup durable write, authoritative captures and final memory CAS remain later serial integration nodes.

The companion merge error boundary separates StorageError from a generic caller identity error, preserving actual entropy I/O kind and inner cause without classifying it as corrupt save data. The merge places no trait bound on caller errors; callers using bare None explicitly choose an error type. This pre-consumer revision changes no merge ordering, bytes or schema.

Companion task persistence uses a complete prepared chat book plus fresh runtime task handoffs. Restored Running tasks retain exact source steps/progress/timing outside model-plan ownership, with only a private terminal-follow marker in chat; no synthetic model summary or Started event is invented. Missing issuer sessions are explicit Option None. Source-valid stored commands retain surrounding whitespace through a separate bounded domain constructor; live chat canonical admission remains strict. Packet119 freezes these prerequisites before actual authority/Agent consumers.

Saved command context is broader than live ChatIntent and immutable ChatEvent. The checked domain boundaries preserve the original canonical human/wire admitted set, and actual chat projection follows source ID allocation then invalid-fact omission rather than panicking or dropping later events. All existing intent consumers adapt to fallible construction; no protocol fixture or wire rule changes.


Complete companion persistence consumes accepted task-owner eaba096066d52c5bd159f5894ad1465d937f2d5b. One private authority child prepares every configured Pending body/inventory/runtime/scan and restored chat before the final ledger admission, then transfers all owners atomically. The latest aggregate remains only in the ledger; settled Active bodies replace records while inactive/Pending bodies and lifecycle/memory remain retained. Raw generation/phase/model-summary observation is compared separately from normalized saved queues, preserving source dirty behavior and normalized ACK cleanup. Packet120 freezes the exact boundaries and real-disk controls before implementation.

#### Authoritative companion persistence qualification

The complete durable aggregate now lands atomically with configured Pending bodies, neutral runtimes, inventory and the accepted restored chat owner. All three resident collections reserve the new owner count before any scan or transfer. Preflight rejects hidden companion body/runtime families as well as missing, duplicate or foreign identities. Settled ordinary and unpublished final captures preserve every inactive/Pending record and lifecycle/memory field, replace Active bodies and validated task queues, and retain latest/immutable targets exclusively in the actor ledger. A private bounded raw observation cache includes phase, generation and model summary; equal normalized wire content can still become dirty, while an exact ACK recomputes dirty from saved content.

Qualification is27new Rust controls (23persistence/4private), four actual background/synchronous DiskStore controls, three exact Go/Rust full encoded outcomes and actual unchanged Go summary characterization plus private Rust summary controls. Go12source controls pass with race/count1. Full Rust76suites3614results, four language/comment audits and strict128items pass on the ten frozen source hashes. Native independent source review is SCOPED_PASS; documentation acceptance is the remaining node-closing step. Evidence: task12/runtime/authoritative-companion-persistence-20261006/acceptance-summary.json. This qualifies the library caller/owner, not a real configured Agent or assembled executable. Memory CAS/final durable handoff and broad3.8/4.1/4.2 remain open.

Node closing: independent DOCS_PASS verified all final counts/log hashes and scope distinctions; frozen ten-path SCOPED_PASS remains exact. The scoped companion startup/capture node is accepted. Broad3.8/4.1/4.2 remain open.

### Companion lifecycle memory CAS plan

Root used installed brainstorming/writing-plans and the project orchestration contract to select a serial lifecycle adapter over accepted427769751, documented in plans/121-companion-lifecycle-memory-cas.md. Direct execution follows the explicit all-task/direct-authoring user ruling; no redundant approval, new worktree or alternate plan store. The source requires highest occupied aggregate revision availability before idempotence, strictly increasing (possibly jumping) memory revisions,2048byte/NUL-only summary validation, and expected epoch/revision CAS. Healthy Closing is callable for later finalization while sticky/Closed/frozen ownership refuses. Agent working mirror/reservation transfer and final drain integration are separately open; the complete latest aggregate remains actor-ledger-owned. Read-only census found no callable CompanionAggregateOwner/MemoryHost and no MemoryOwner production caller; do not assume those planned names exist. Main owns every interface/error/test/provider decision; the packet is concrete and self-reviewed, with no parallel product writer or shared-boundary drift.

### Companion lifecycle memory CAS verification

Root direct authoring implements borrowed complete lifecycle reads and active-memory replacement on the sole complete ledger. Six frozen source/test/guide paths bind identity SHA79e0cd4aa7eb7609a0ee1cdd57fd4db60c2cb9afa3d5fa00c0b13d2ebecf24f4 at accepted prerequisite427769751. Declaration-only missing-API RED, genuine occupied-MAX RED and frozen-idempotence RED precede the corresponding minimal fixes; all earlier failures remain evidence. Early receipts used a partial or previous identity path set and are not relabeled as final six-path qualification.

Thirteen public topic tests and three private prepared-state controls pass in the full gate. Highest occupied aggregate revision availability precedes proposal validation and idempotence, including held/failed MAX targets. Higher memory revision jumps and exact stale-expectation idempotence match source;2048UTF-8byte/empty/padding/non-NUL controls remain valid. Replacement preserves namespace, bodies, inactive/tombstone metadata, queues and raw task observations. Healthy Closing accepts replacement before final flush; Closed/frozen/sticky cases refuse.

Two actual background DiskStore cases prove immutable failed body/task target retry before latest memory final flush/full reopen, and real v5 zero-memory baseline/native placement/save followed by higher remote revision installation with inactive tombstone preservation. They use the accepted literal source fixture and narrowly exposed test-only helpers; no new production fault surface. Actual unchanged Go Companions/codec with a capturing store and actual Rust public authority outputs match all16accepted/refused/idempotent outcomes, complete normalized bytes and selected envelope targets, including held10then11. This Go proof is not physical filesystem evidence or a live Agent settlement. Four unchanged Go atomic/in-flight/occupied-MAX/overflow tests plus two occupied subtests pass with race/count1.

make rust-check PASS:76suites3630passed/0failed/0ignored,139.954seconds, loge9012406b5d9376715fd64d132e1842426daa357cf7a8aaae4a232bf7b31e472. Workspace fmt/Clippy and tests pass on the six frozen hashes. Four language/comment audits and strict128OpenSpec pass. Native SCOPED_PASS has no P1/P2 finding and binds all six hashes; final documentation review is pending. Dev HEAD/status/nine bytes, both archived dirty files and staged-empty canonical index are exact. Architecture skill: no change. AOCI callable tools and a usable complete canonical index remain absent; no formal cognition or maintenance receipt is claimed, and main dev cognition assets remain protected.

This accepts the scoped lifecycle/CAS provider only after documentation closure. Agent working-mirror/reservation settlement must pass CAS before readiness/effects/removal; existing poll/drain still only acknowledges remote settlement. Ordinary/final provider handoff, configured Agent/task runtime, assembled executable, source outcome closure and broad3.8/4.1/4.2 remain open. Evidence: task12/runtime/companion-lifecycle-memory-cas-20261006.

Node closing: native DOCS_PASS verifies final evidence and the corrected remaining-gap inventory; six frozen hashes retain SCOPED_PASS. Scoped lifecycle memory CAS is accepted. Agent settlement/final durable handoff and broad3.8/4.1/4.2 remain open.

### Authoritative memory settlement plan

Root uses project orchestration and installed brainstorming/writing-plans to select in-poll authoritative CAS gating over accepted f667c1f754c75c96d14535c8a83c4db7a88a8550. Packet122 fixes exact APIs, namespace/latest-lifecycle binding, complete fulfilled reservation output, source equal/higher reconcile policy, durable tombstone acknowledgment, bounded cleanup and an explicit authority-aware finalizer wrapper. Bare remote-provider controls retain their qualified scope; configured consumers must choose the authoritative adapter. Both shutdown callers pass the same mutable authority without aliasing it into a wrapper. No second aggregate/proposal cache or schema/dependency changes. Root owns design, implementation, tests and serial integration; native census/review remains read-only. User full-task/direct authorization overrides redundant skill approval/store/worktree defaults. Test doubles, actual Agent HTTP and actual disk evidence are separately bounded; combined-runtime acceptance is not assumed. Readiness checklist covers exact editable/read-only paths, prerequisites, algorithms, failures/successes, limits, consumer tests, rollback and exclusions before code. Architecture skill: no change.

### Authoritative memory settlement verification

Frozen parent f667c1f754c75c96d14535c8a83c4db7a88a8550 plus thirteen exact dirty source/test/guide digests accepts the authoritative provider and finalizer ports. Complete lifecycle CAS precedes commit mirror/readiness/fulfilled-reservation release; higher reconcile installs against the current durable base; equal/absent and inactive/tombstone paths prove the current owner. Malformed publicly mutable reservations refuse before arithmetic/copy. Enabled complete authority rejects the trait-default remote-only finalizer. Delete response epoch/tombstone mismatches remain Fenced before durable validation. Both shutdown callers pass the same authority to the explicit wrapper.

Actual qualified API RED, three CAS-order assertion REDs, Closed/absent and delete-mirror assertion REDs, wrong-response-companion RED, mutable-base overflow RED and two independent-review guard REDs precede corrections. Final seventeen new contract tests pass; the existing twenty-seven provider controls preserve their remote-only scope. A scripted Agent with actual background DiskStore proves immutable failed companion target retry followed by newer Closing memory capture, final flush and exact reopen. A separate actual Python Agent/AgentHttpWire/LeaseController process test proves absent reconcile, ordinary commit and fresh Closing authoritative finalization. It uses a prepared complete ledger and does not include physical disk or the entire shutdown machine. No combined configured runtime, dialogue issuer/effect or executable acceptance is inferred.

Final make rust-check passes fmt/Clippy and76suites3649results, zero failed/ignored,143.237s; raw log69eaff766561e472965d22e3d91972de6a9725a42409e162a4011eeeeb5cd062. All twelve unchanged Go memory/finalization controls pass race/count1. Corrected native make rust passes on final frozen source; four language/comment audits and strict128 pass. Original self-copy build failure, default1.98.0 unrelated Clippy failure and original borrow lint remain historical failed receipts; no gate is bypassed or relabeled. Final source review SCOPED_PASS binds all thirteen paths to identity698fc23bae6454ea600cb527e9700c2087f6c177c40f98b1be5e406242a7ce38. Evidence: task12/runtime/authoritative-memory-settlement-20261006. Documentation acceptance and closing audits precede the scoped local commit; configured bootstrap/Agent execution, publication outcomes, executable/trusted observer and broad3.8/4.1/4.2 remain open.

### Active owner publication intent plan

Root applies brainstorming/writing-plans for packet123 over acceptedd9d6b5be604d3fb7b5ee5ee0115bc4cebb7c2027. Compare the checked inventory wire mirror rather than the full storage/grid record; gate both owner branches on Active and mark existing bounded dirty intent only after successful workbench/eating/pickup settlement. Selected exact wire mirror over duplicated storage-field checks or a broad producer-bit rewrite. Source Pending/private-state, repeated/reanchored bench and equal final eating/pickup outcomes receive concrete failing tests, with prepared lifecycle versus actual provider scope distinguished. No queue receipt/encoder/container lifecycle redesign. Root owns exact9editable paths and serial integration/rollback; no worker implementation. Review coverage/types/acyclic ownership/bounds is complete before code. User full/direct authorization persists; Architecture skill: no change.

### Active owner publication intent verification

Source-bound five genuine assertion REDs become five GREEN; final six controls pass in all537replay cases. Source now retains the exact InventoryState wire mirror, gates owner inventory/grid diffs by Active and marks successful first/reopened/reanchored workbench grid intent without extra inventory. Actual Eating then DropStep with prepared pre-completion runtime consumes and picks up identical food, returning the same whole inventory while publishing exactly one final owner wire state. Prepared Pending transition and initial Pending controls preserve private PlayerState and defer record mirrors until Active; these do not claim an executed death or bootstrap. Foreign/quiet/refused and existing equal armor/crafting/container command regressions retain source semantics.

Fresh final fmt/Clippy76Rust suites3655PASS0failed/ignored137.356s, logd9ebce489aa55b01bbc3f4ac6faca8b213cfd1226efc78a5442c6729019392f0. Native build PASS and unchanged Go13top/18all owner/workbench/eating/pickup controls race/count1PASS; audits4/strict128PASS. First fixture compilation used wrong EatingProgress field/private drops observation; the later initial-Pending fixture confused StoredPlayer with PlayerSave. These qualifications remain failed receipts, not behavior RED. Final ninepath identitybc4fc738ebfc53c3030caba31a5380054368b8bbc43d07b5b8ac0dbc82d6ac50 receives independent SCOPED_PASS. Evidence: task12/runtime/active-owner-publication-intent-20261006. Queue receipt/encoder, container cadence/lifecycle/order, empty revision barriers, actor visibility/reset/reconnect, drop geometry and configured runtime/whole outcomes remain pending; broad3.8/4.1/4.2 OPEN. Documentation review and closing gates precede scoped commit.

### Source world publication boundaries plan

Root uses installed brainstorming/writing-plans for packet124 over accepted407f0d9e5714f74670a6a2f308b419cee5eb1480. Include existing dirty physical slot keys in source empty block revision barriers, and use fixed radius-two Ready drop interest independent of session snapshots. Chosen existing dirty index and bounded8player/200key/6400drop union avoid duplicate indices and a whole retained drop clone. Five exact editable paths, helper interface, checked geometry, control scopes, actual command tests, exclusions, review focus and rollback are resolved before product code. Root serial ownership; native review read-only. Queue/encoder/remaining publication/config/runtime and broad milestones remain open. Architecture skill: no change.

### Source world publication boundaries verification

Five genuine assertion REDs become five GREEN; sixth same-tick slot/block coalescing control and full543replayPASS. Actual selected-drop and chest-panel-drop commands emit complete empty barriers for each physical dirty chunk, followed by a contiguous block delta without gap snapshot; a late subscriber receives its full current snapshot instead. Counter-only quiet emits no barrier. Prepared radius-two/radius-three drop records prove fixed source interest for both narrow and wide session snapshot bounds; an unready sparse drop stays absent until an actual Ready physical fixture is installed. Those prepared controls do not claim an executed spawn or physical persistence provider.

Production drop copying now joins only current admitted speakers to Active player actors, deriving at most200Readykeys/6400fixed-slot copies, then observer radius2admits at most800records. Unavailable and Unloading owners are filtered by the actual read view; no queued snapshot is required for drops. This replaces the all-resident drop clone. Source dirty-key capture and one empty group preserve existing revisions/order.

Fresh make rust-check fmt/Clippy76suites3661PASS0failed/ignored150.832s, logac440bed29adadcd75dd8960b9cef3c93dbe8b8fdbabf0e9cd7a7d0a4daf33de; native build PASS26.289s, unchanged Go6top source codec/delta/drop controls race/count1PASS; audits4/strict128PASS. Final five-path identity4bbd8d42b32aa15c6b0c730c281cb4659658baaf7397016160d08f3a55c26316 gets independent SCOPED_PASS. Evidence: task12/runtime/source-world-publication-boundaries-20261006. Queue receipt/encoder, actor snapshot visibility/reset/reconnect, container lifecycle/cadence/order, config/runtime/whole-outcomes and broad3.8/4.1/4.2 remain open. Final docs acceptance and closing gates precede scoped commit.

### Source container publication lifecycle plan

Root applies installed brainstorming/writing-plans for packet125 over accepted a6a7bad5e4f1823ce9475ded8bdbae5815a4abc7. Exact seven editable paths, bounded fixed-slot position and final lease validation, ephemeral invalidation facts, full cadence and source record order are resolved before code. Source explicit-close test is corrected from actual Go evidence. Current valid wire remains overworld-only; no contract widening. Root sole writer/integrator/rollback; native review read-only. Controller self-review verifies requirement coverage, exact types, acyclic ownership, bounds and concrete RED cases. Immediate open/close command phase newly identified remains a separate explicit gap. Architecture skill: no change; broad3.8/4.1/4.2 OPEN.

The actual Memory/TCP parity test carries the same obsolete explicit-close expectation. Packet125 adds only its explicit-close assertion to the owned eighth source path; both adapters must emit neither container state nor close and preserve transcript equality. Source full gate failure is retained and corrected gate required.

### Source container publication lifecycle verification

Twelve new cases pass: eleven genuine assertion REDs and one baseline replacement control; final555replayPASS. Exact native opens/closes/workbench replacement/crafting/transfer prove source container publication, while pose/Pending/dimension/physical owner and no-prior-mirror are prepared inputs, not upstream producer qualification. Actual Memory/TCP parity test passes corrected explicit-close silence and identical complete tick transcript. Source invalidation validates Active/OV/Ready/exact generation and source float32 reach over at most eight current leases; valid full states repeat every tick. Container diff mirrors are removed; exact ephemeral invalidations emit once in source record order.

Fresh corrected fmt/Clippy76Rust suites3673PASS0failed/0ignored117.954s, logfa7216a1c0ed88d5c24f417ed395c6f284678359a013b5fffca0d881aab0e109. Nativebuild14.477sPASS on unchanged production source, unchanged Go10top race/count1PASS log4f33fa0b59f06a2f4000deeafbe94950b99a9880a63f629cb962f2a2321a4b00; audits4/strict128PASS. SourceSCOPED_PASS binds eight paths to66994ba447952aa5c797482890267c20cea6292b2df248269c04a84e4d1d4eb1. First qualification wrong Vec field and first full obsolete parity expectation failure are retained separately, never relabeled. Evidence: task12/runtime/source-container-publication-lifecycle-20261006. Existing parity scenario/test comments still say explicit-close notice (minor); record for next owned same-topic scope before4.1 acceptance. New source mining container/reset guard gap and immediate command phase remain explicitly OPEN with queue/encoder, visibility/reset/reconnect, configured runtime/Agent/outcomes and broad3.8/4.1/4.2. Architecture skill: no change. Root direct writer, native readers only. No usable complete AOCI index/tools; no fabricated receipt. Dev9/archive2/frozen8/index protection and docs closing audits precede scoped commit.

### Immediate container lifecycle command plan

Root applies installed brainstorming/writing-plans after accepted f93d42abac39314f82e7f792066bfb7b8c740d8d. Packet126 freezes seven editable source paths, exact early lifecycle routing through accepted checked settlement and private bench arm only on NoTarget, concrete actual hold/open/close and command-prefix REDs. Selected live producer early settlement over incompatible raw-provider rewrite or a duplicate retained queue. Raw deferred provider compatibility remains isolated; no live lifecycle producer uses its late bags. Source rejection/mining/bench-sneaking/failure-policy gaps remain explicit and separate. Controller readiness self-review covers exact types, bounds, causal assertions, acyclic ownership and all derived consumers. Root design/write/integrate/rollback; native read-only review. Architecture skill: no change; broad milestones OPEN.

### Source view marker prerequisite ruling

Independent source census /root/player_view_ready_census verifies Go runtime/entity_delegate.go48–101 copies registered subscription hasViewtrue, and entity/eating_test.go543 confirms no production false writer. Source Rust prepare inherits merged_runtime.has_viewfalse and activation never changes it. Earlier root nine-Ready readiness assumption is rejected. Packet126 expands exact scope to source_player_restore.rs registration and the state.rs contradicted private test before product; add genuine registration-without-Ready RED and retain actual eating threshold evidence. No geometry-derived policy or fabricated upstream proof. Frozen9sourcepaths, source marker and immediate commands form the actual registered action integration deliverable.

## Immediate lifecycle and source view marker acceptance evidence (2026-10-06)

Source registration-without-Ready yields one genuine marker RED. After marker correction, actual source-enabled PlayerInput and real31ticks yield seven command-timing REDs; reverse replacement is a positive control. All nine cases pass and full564replayPASS. The original legacy-marker threshold failures, invalid spawn-radius failure and marker self-witness failures are fixture qualification, never counted as timing REDs. Complete held-food inventory/survival and runtime progress are compared; saved bodies, initial inventories and nine Ready geometry keys are prepared inputs, not disk/acquisition acceptance. Registered Pending view marker is independent of geometry and remains separate from action eligibility.

Final frozen nine-path identity ebc58fd1638a755bd65d33c7c408751c045235d34619e9a067f6d778828b3259 receives revised independent SCOPED_PASS, no P1/P2. Live lifecycle settles in sorted intake; only container NoTarget reaches the disjoint bench arm; no live open/close reaches late bags. Raw checked deferred provider controls retain their compatibility contract. Old parity-close comments and intake bag doc are corrected. First full gate fails two obsolete deferred-capacity fixtures; validated deferred MoveContainer restores all six private fault checks without changing any sticky error/save/final-replay assertion. Original failure is retained.

Fresh corrected fmt/Clippy76Rust suites3682PASS0failed/0ignored135.321s, log0da2d25c4cfec618e87c995c1a00aa7f4c0f72f3ca6a3b79fb342036050aca7d. Native build24.885sPASS before Go; its later two-path delta consists only of intake comments and cfg(test) fault fixture, with production executable behavior unchanged. Unchanged Go8top race/count1PASS log7e8f3b3add345b320040982bf976e7aef702d0a465c1cdf01c1e560709524098; Go eating tests include prepared runtime flags and do not independently qualify a source command producer. Audits4/strict128PASS. Evidence: task12/runtime/immediate-container-lifecycle-commands-20261006. Source mining reset/view/viewer/held-bow guards, bench sneaking/automatic-close failure, wire refusals, queue/encoder, actor visibility/reset/reconnect, config/Agent/executable/outcomes and broad3.8/4.1/4.2 remain OPEN. Architecture skill: no change. Root sole writer; native reviewers read-only. No canonical usable complete AOCI index/tools; no fabricated receipt/maintenance. Dev9/archive2/frozen9/index protection and independent docs/closing audits precede scoped local commit.

## Source human mining suspension planning (2026-10-06)

Root applies installed brainstorming/writing-plans for packet127 at accepted df9354ff2556efeb8e49a4c17032a50ef6c65563. Read-only mining census names one producer/two direct test consumers and exact source guard matrix. Provider-owned constant indexed guard preserves source registered marker, early viewer and companion isolation. Six exact editable source paths, actual14tick open/close cases and prepared provider cause/restart controls are resolved before product. Root self-review covers requirement/test mapping, exact existing types, bounded work, acyclic dependencies and derived Cargo/audit consumers. Root sole writer; native source/docs reviews isolated and read-only. Broad milestones OPEN; Architecture skill: no change.

## Source human mining suspension acceptance evidence (2026-10-06)

At accepted parent df9354ff2556efeb8e49a4c17032a50ef6c65563 four real-provider regressions retain or restart progress under prepared reset/unavailable-view/intact-bow/broken-bow causes. Two actual registered source PlayerInput cases progress14real ticks then open a chest/furnace on completion tick: parent destroys workbench45 to0 despite current viewer. All six causal REDs become GREEN and full570replayPASS. RED tests precede cargo fmt only; no assertions or behavior changed after RED, and final formatted test identity is separately bound. Provider causes are prepared controls with actual progression; native registration/placement/input/progression/open/close/restart execute real owners over prepared saved bodies and Ready geometry. No disk/acquisition/reset-producer inference.

The sole MiningStep human provider now clears before ray resolution for source reset, unavailable subscription, current viewer or either selected bow item62/65. Existing quiet stage_clear retains controls, no charge/tool/world mutation, and next eligible tick starts1. Tick-local suppression, active bow-progress defense and companion/output paths remain unchanged. Only the contradicted successful-provider human_runtime fixture is corrected to source has_viewtrue. Independent SCOPED_PASS binds six paths to228b8c0889d72b90b434391eb30bd719bad8f114c61b730e30c73b2d272f23ee; no findings.

Fresh fmt/Clippy76Rust suites3688PASS0failed/0ignored145.483s, log0ac6825381449f3e08451e17acd39d2004f1e11cf5ff82e68fdd3c17c72b5baf. Native build25.519sPASS; actual unchanged Go3top race/count1PASS loge075791b8ae5dab11577b81475b596b372639922bca7619303dd7d8b09f432cd (prepared view/reset/bow causes distinguished from native inputs); audits4/strict128PASS. Evidence: task12/runtime/source-human-mining-suspension-20261006. Bench sneak/automatic-close failure, command refusal wire outcomes, actor visibility/reset/reconnect, queued mirror/off-tick encoding, configured Agent/gameplay runtime/full outcomes and broad3.8/4.1/4.2 OPEN. Root sole designer/writer/integrator/rollback; native review read-only; inherited guides apply, no generated derivatives. Architecture skill: no change. No canonical usable complete AOCI index/tools or fabricated receipt. Dev9/archive2/frozen6/index protection, independent docs and closing audits precede local scoped commit.

## Source workbench sneak eligibility planning (2026-10-06)

Root applies installed brainstorming/writing-plans for packet128 at accepted734b0bd4e7d51fd40eab3a4be83be2d885bf217c. Actual Go target-first/sneak guard and Rust shared bench-arm two-caller census justify a single indexed state predicate before atomic mutations/intent. Five exact source paths, three native prefix/conservation/intent REDs and raw compatibility RED, complete controls/bounds/types/consumer gates/exclusions are resolved before product. Root self-review meets task readiness; no separate shared landing required. Root sole writer/integrator, native source/docs readers isolated. Architecture skill:no change; broad milestones OPEN.

## Source workbench sneak eligibility acceptance evidence (2026-10-06)

At parent734b0bd4e7d51fd40eab3a4be83be2d885bf217c three native command-prefix cases prove wrongly widened Personal grid, wrongly ended chest lease and wrongly restated equal workbench grid. One raw compatibility case proves both retained opens incorrectly apply/carry against current held sneaking. Four causal REDs become four GREEN and574replayPASS. Initial missing TickPublication import is preserved compile qualification, not behavior RED; later formatting changes no assertion/behavior. Live source registration/placement/PlayerInput/OpenContainer execute actual owners over prepared saved body/Ready geometry. Raw controls and lease are prepared cause inputs. Releasing actual sneaking permits opening; refused equal reopen emits no grid intent; retained chest keeps its accepted full cadence. No wire refusal, physical-slot reason precedence or runtime proof is inferred.

Shared bench arm now checks current held sneaking only after workbench classification and before all grid/anchor/lease/intent mutation. Live immediate and raw deferred callers reuse it; no extra queue/interface. Crate guide reconciles old bench-anchor and intent wording to already accepted ownership. Five frozen paths identitya4c1029a00e38334ffb6e6521a9b30f37ea20fb5320e6248324e00b7b40054d0 receive independent SCOPED_PASS, no findings.

Fresh fmt/Clippy76Rust suites3692PASS0failed/0ignored143.484s, log6ed05dd4a1ea99c41b97172a31c2498916cc929a07d2d2c51eb36561e5d6e328. Native build25.530sPASS; unchanged Go4top race/count1PASS (actual common container sneaking negative/sprint positive and normal workbench controls, not a separate executed Go bench-sneak fixture); audits4/strict128PASS. Evidence: task12/runtime/workbench-sneak-eligibility-20261006. Automatic-close invariant/hard error, physical-slot reason precedence and every supported command/held-mining-completion refusal wire outcome remain OPEN, with actor visibility/reset/reconnect, queued mirror/off-tick encoding, configured Agent/gameplay runtime/full inventory and broad3.8/4.1/4.2. Next command census retained separately as facts, no acceptance or blanket mapping. Root sole designer/writer/integrator; native readers read-only. Architecture skill:no change; no usable canonical complete AOCI index/tools or fabricated receipt. Dev9/archive2/frozen5/index protection, independent docs and closing audits precede scoped local commit.

## Source workbench close compatibility ruling and planning (2026-10-06)

Root applies installed brainstorming/writing-plans for packet129 at accepted2a236a0029d0457ce88e520f94dda32f19f5fb88. Actual Go closeWorkbench sets both owner dirty flags even for empty-grid close, and automatic failure panics instead of continuing siblings. Historical packet22 chose no-loss/rejected continuation; preserve that record but correct current compatibility through accepted packet78 typed sticky hard-failure owner. Explicit close remains ordinary InvalidInput refusal; both success producers mark existing owner dirty lanes only after atomic stage. Six exact editable paths, seven causal REDs (one revised prior expectation +sixnew), bounded APIs, source/actual prepared-cause distinctions and all acceptance/rollback decisions are frozen before product. No new shared interface landing. Root sole writer/integrator; native source/docs readers isolated. Architecture skill:no change; broad milestones OPEN.

## Checked command outcome shared boundary planning (2026-10-06)

After accepted2a2b84922092db9484f3eb93c61461cb8f596798 root applies installed brainstorming/writing-plans/project worker-planning for packet130. Whole-dispatch facts identify multiple separately reviewable real consumers; first land checked CommandDisposition/CommandProvider and compact bounded owner/sequence-preserving phased refusal log. Explicit semantic Refused consumes, Unowned falls through, hard ServerError propagates. Five exact source paths and five interface/double/order/independent-capacity/zero-sequence acceptance cases are frozen before product. Declaration-only RED is labelled contract evidence, not runtime behavioral RED. Producer/dispatcher integration remains open and must name actual accepted contract SHA. Root owns all design/write/integrate/rollback; native readers isolated. Architecture skill:no change; broad milestones OPEN.
