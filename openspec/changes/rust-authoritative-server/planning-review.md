# F2 task-readiness review

## Result and scope

Reviewed on 2026-09-27 against source checkpoint `88417c2f10a566fc05e06b3a0e7ad9cdd01a0183` and the accepted F1 source/corpus identity. The task graph and ownership structure are clear, but the complete F2 plan does **not** yet qualify for the S1 contract landing or parallel provider dispatch. Several packets contradict the compatibility oracle, and others require implementation workers to design shared behavior that the controller must decide first.

The review covers all 42 pending nodes: nine core/contract nodes, 21 rule-provider nodes and 12 adapter/integration/closure nodes. Three fresh read-only GPT-6 Sol reviewers separately audited those areas; the controller checked normative consistency and the complete dependency register. This is a review artifact, not a replacement implementation contract or a second task-status source. [tasks.md](tasks.md) remains authoritative. No runtime implementation or acceptance is recorded here.

P1 means an important contract, behavior or acceptance gap that must be resolved before its affected implementation node. P2 means a narrower ambiguity or metadata mismatch that must be reconciled before its affected dispatch. Prospective Rust files not existing yet are not findings.

## Structural evidence

- Exactly 42 task IDs and 42 direct-predecessor rows; sets are identical and unique.
- All local predecessor references resolve, and the dependency graph is acyclic.
- All 35 relative links in the pre-review change artifacts resolve.
- Strict validation of `rust-authoritative-server` passed.
- File-per-node ownership, serial shared registration and separate contract/provider/integration gates are present.
- F1 is already sealed; the current source-bound inventory entry node can gather evidence. A future accepted S1 SHA remains necessary for its consumers.

These checks establish organization, not the semantic completeness of the task packets.

## Required corrections

### R01 — P1: Admission changes the frozen ordering behavior

Locations: [execution contract](plans/00-execution.md), line49; [session packet](plans/01-server-slices.md), line11.

`submit` requires a strictly increasing sequence at arrival and immediately commits that water mark. Current Go ingress does not prefilter sequence; `Engine.Step` sorts the tick batch and then checks the last applied sequence. The sealed positive authority fixture arrives as `9/select2`, `9/place`, `8/select1`, and executes 8 before the first 9. The proposed admission policy rejects 8 and changes the frozen result.

Evidence: [engine_step.go](../../../packages/server/sim/runtime/engine_step.go), lines68 and96; [session_ingress.go](../../../packages/server/server/session_ingress.go), line373; [command_order_oracle_test.go](../../../packages/server/sim/runtime/command_order_oracle_test.go), line114.

Required decision: distinguish queue admission from applied-sequence authority, freeze duplicate/out-of-order handling, and execute the sealed fixture through the real `submit` to `advance_tick` path.

### R02 — P1: Rejection semantics imply an unsupported wire response

Locations: [specification](specs/rust-authoritative-server/spec.md), line32; [common transport packet](plans/01-server-slices.md), line69.

The spec says stale actions receive equivalent rejection. The sealed Go stale-sequence case is a silent discard with no `CommandRejection`. The plan does not map each `ServerEndpoint::submit` error to wire response, disconnection or silent no-effect. The v45 rejection vocabulary has no stale-sequence or Capacity reason; an implementation cannot invent either while preserving v45.

Evidence: [authority producer](../../../packages/server/sim/runtime/command_order_oracle_test.go), lines319,702 and729; [command_rejected.rs](../../../packages/engine/crates/mornlea_protocol/src/command_rejected.rs), lines7–21.

Required decision: state the observable result for every rejection class, separate offline classification from wire publication, and freeze identical Memory/TCP mappings without changing the protocol.

### R03 — P1: Shared declarations and private-state access remain unresolved

Locations: [contract packet](plans/01-server-slices.md), line9; [core seams](plans/02-core-seams.md), lines15–19; [inventory provider](plans/01-server-slices.md), line41.

`AuthorityReadView` has no getter table; `WorkKind`, `RuleEffect` and `RuleReject` have no full variants/fields; phase entrypoints lack complete signatures. Core providers also lack accepted ports for session mutation, queue drain/cancel, chunk-result admission and outbox reads while `state.rs` is outside their editable files. The inventory packet still asks its worker to define shared item/stack ownership and debit/credit.

Concrete cross-provider values are missing: confirmed internal damage with target identity, death/mining drop batches, sleep-to-environment offsets and fluid/farmland carry state. F1 `CombatHit` contains target kind, not target ID, and cannot substitute for an internal damage command.

Required decision: freeze exact fields, signatures, ownership and error semantics for each producer/consumer row before 1.2 implements them. Include every shared-state access port and a matching consumer double; contract landing is not permission to defer architecture design.

### R04 — P1: Mining lifecycle and atomic effects are incomplete

Locations: [mutation packet](plans/01-server-slices.md), line17; [world mutation](plans/01-server-slices.md), line25; [companion execution](plans/01-server-slices.md), line63; [resolved transaction](plans/02-core-seams.md), line19.

The packet describes one target/world change with a matching debit or credit. Existing mining also needs progress, target/tool-switch reset, tool durability, two-block door/bed footprints and container contents. Human products become world drops, while companion products are credited to inventory. Container removal and output capacity must be preflighted and committed together; a single-target signature without declared footprint and output ports does not settle that behavior.

Evidence: [mining.go](../../../packages/server/sim/entity/mining.go), lines136,403,710,766 and799; [companion_mining_test.go](../../../packages/server/sim/entity/companion_mining_test.go), line176.

Required decision: assign progress-state ownership, freeze bounded multi-block/per-chunk observations and actor-specific output types, and define the world/container/tool/inventory/drop atomic preflight and commit. Add cross-chunk door/bed, full-drop capacity and full companion inventory cases.

### R05 — P1: Supported rules lack explicit implementation owners

Locations: [crop packet](plans/01-server-slices.md), line33; [survival packet](plans/01-server-slices.md), line39; [inventory and container packets](plans/01-server-slices.md), lines41–47.

The actual random-tick path includes sapling tree growth, dry farmland degradation, grass spread and snow/melt branches beyond the packet's crop/trample list. Eating requires inventory/hunger atomic ownership despite survival excluding inventory. Tilling, bone meal, bucket operations, armor equipment, quick move and panel drop have no explicit algorithm and acceptance mapping in the current nodes.

Evidence: [environment.go](../../../packages/server/sim/realm/environment.go), line1053; [eating.go](../../../packages/server/sim/entity/eating.go), line58.

Required decision: enumerate every supported command and rule branch into a concrete node, owned source/test files and a decidable case. Inventory discovery may expose missing rows; it cannot silently grant unspecified work to the nearest provider.

### R06 — P1: Algorithm and oracle descriptions are too abstract

Locations: [replay helpers](plans/02-core-seams.md), line21; [common provider procedure](plans/01-server-slices.md), line21; [farmland](plans/01-server-slices.md), line31; [furnaces](plans/01-server-slices.md), line47.

Labels such as wet neighbor, exact recipe, deterministic carry and restart parity do not provide a concrete input schedule, expected event/state value, error class or budget counter. Helper parameter types and the real single-phase provider invocation are not fully declared. Farmland omits the scan window, candidate ordering, reservation and reinsertion policy; furnace progression omits the fact that invalid input/full output also pauses remaining fuel.

Evidence: [environment.go](../../../packages/server/sim/realm/environment.go), lines16 and459, uses radius4 and a two-layer 162-read reservation; [furnace.go](../../../packages/server/sim/entity/furnace.go), line50, pins pause behavior. Other actor, projectile and sleep packets have the same need for explicit formulas/order and concrete boundary rows.

Required decision: give each node its non-obvious ordered algorithm, exact numeric boundaries and source-bound input/expected table. Declare helper types and ensure provider tests call real code independently of the contract double. Read-only legacy files supply evidence; they must not transfer design responsibility to workers.

### R07 — P1: Projectile saturation contradicts production behavior

Location: [projectile packet](plans/01-server-slices.md), line51.

The 128/129 case is described as a reported rejection. Current production spawning evicts the minimum projectile ID at capacity and successfully inserts the new projectile. The Go regression asserts that the new ID exists and the oldest one disappears.

Evidence: [projectile.go](../../../packages/server/sim/entity/projectile.go), line145; [projectile_set_test.go](../../../packages/server/sim/entity/projectile_set_test.go), line45.

Required decision: distinguish defensive collection insertion rejection from production spawning/eviction, and freeze the exact retained IDs and removal observations for the 129th spawn.

### R08 — P1: Container checks refer to fields absent from accepted input

Location: [container packet](plans/01-server-slices.md), line43. The [readiness review focus](plans/03-parallel-readiness.md), line26, already prohibits inventing wire container revisions.

The packet requires token/revision validation and token invalidation at close. Accepted `ContainerMove` carries `ContainerRef` plus from/to; the reference has chunk/kind/slot/generation. Go checks server-owned viewer relationships, generation and distance. No input token/revision exists. If the terms mean server-internal state, their owner and matching policy are unspecified.

Evidence: [inventory.rs](../../../packages/engine/crates/mornlea_domain/src/input/inventory.rs), line121; [container.go](../../../packages/server/sim/entity/container.go), lines25 and169.

Required decision: freeze checks using available validated facts, define any private session/view state explicitly, and cover delayed move after close without expanding the wire contract.

### R09 — P1: Real storage commit and recovery are not specified

Locations: [store packets](plans/01-server-slices.md), lines75–79; [save declarations](plans/02-core-seams.md), line29.

The mailbox schedules encoded work, but the plan does not assign an exact filesystem backend/commit algorithm. Recovery refers to a last complete checkpoint and corrupt/torn journal without defining either. Existing region persistence has a payload-sync, inactive-bank write and bank-sync boundary; the recovery oracle can return old payload with a newer logical revision and `NeedsRewrite`, without writing during load. F1 Rust storage supplies codecs and bank selection, not a complete I/O owner.

Evidence: [region.go](../../../packages/server/storage/chunk/region.go), lines414–435; [region_recovery_test.go](../../../packages/server/storage/chunk/region_recovery_test.go), lines21–43.

Required decision: freeze the disk backend owner/files, codec-to-file mapping, durability ticket boundary, bank selection and exact crash points. Do not leave a worker to invent a journal or global checkpoint policy. Specify compatible recovery/rollback observations and validation before mutable access.

### R10 — P1: Shutdown ports and failure reporting are incomplete

Locations: [shutdown packet](plans/04-refined-nodes.md), line31; [store interface](plans/00-execution.md), line47; [deadline/errors](plans/02-core-seams.md), line11; [shutdown phases](plans/02-core-seams.md), line31.

The packet needs final-reducer, worker-cancel, sync, close, clock and lease-release ports. S3 lists only submit/poll/poll_tick/flush and does not decide whether flush includes sync/close. Timeout is described but absent from the closed `ServerError` list. The error return does not explain how phase/outstanding/retry information survives. These gaps prevent a worker from implementing precise once-only phase retries.

Evidence: [shutdown.go](../../../packages/server/server/shutdown.go), line175, keeps distinct sync/close progress.

Required decision: freeze the phase enum, callable lifecycle ports, clock injection and complete failure report. State what the next call does after failure in each phase; completed sync must not repeat merely because close failed.

### R11 — P1: Agent/MCP production responsibilities and lifecycle lack owners

Locations: [Agent packet](plans/01-server-slices.md), line81; [S4 interface](plans/00-execution.md), line47; [shutdown](plans/02-core-seams.md), line31.

A real plan request requires lease/client/run/snapshot/digest/deadline/MCP endpoint/capability identity. Existing host behavior registers a frozen snapshot and owns MCP, Acquire/Heartbeat/Cancel/Release and memory/dialogue/reconcile flows. No F2 node/files define those owners; S4 submit/poll has no cancel/close lifecycle. Release failure after persistence must retain the same frozen lease, MCP server and snapshot registry for retry, which is not in the current shutdown packet.

Evidence: [agent_client.go](../../../packages/shared/companion/agent_client.go), lines100–114; [companion_agent.go](../../../packages/server/server/companion_agent.go), lines616 and642; [host_shutdown.go](../../../packages/server/server/host_shutdown.go), lines105–150. Existing host shutdown tests pin retry with the same frozen lease and registry/MCP retention.

Required decision: assign these responsibilities and files, freeze request/snapshot/lease types and source-tick/digest lifetimes, and include cancel/release/retry ports in the shared landing. Keep the independent Python service boundary and bounded tick ownership.

### R12 — P1: Agent integration does not execute the new Rust consumer with the real service

Locations: [Agent validation](plans/01-server-slices.md), line81; [final integration](plans/01-server-slices.md), line83.

`make companion-agent-integration` tests Python and existing Go consumers; it does not execute the prospective Rust S4. Final F2 integration explicitly uses a fake Agent service. These tests can verify Rust transport/provider behavior and compatibility doubles, but cannot establish the declared real Rust producer-consumer integration with Python/MCP.

Required decision: add an explicit nonempty real-process Rust S4 to Python Agent/MCP gate, with rebuilt source identity, owned disposable lifecycle, schema/lease/snapshot cases, timeout and shutdown evidence. Keep double, real provider and real integration results distinct.

### R13 — P2: Capacity measurements omit a retained ownership lane

Location: [capacity measurement packet](plans/04-refined-nodes.md), line15.

The report separately measures queued, worker-held and retry jobs but does not explicitly include completed-but-unconsumed jobs. Go `saveCompletions` retains `Job.Snapshots` until the tick drains or moves it to retry. This ownership transfer can leave the 4 MiB proof incomplete if completion backlog is not sampled and included in ticket deduplication.

Evidence: [world.go](../../../packages/server/server/persistence/world.go), lines263 and272.

Required decision: include a completion lane, sample enqueue/consume boundaries, and freeze the ownership sum/deduplication rule with a schedule that combines queued, held, completed and retry states.

### R14 — P2: Cross-tick receipts and arrival identity are ambiguous

Locations: [submission](plans/00-execution.md), line49; [mailbox](plans/01-server-slices.md), line13.

The receipt assigns the next tick, while a zero/small command budget carries accepted records to later ticks. The plan does not decide whether receipt.tick is earliest or guaranteed execution time, whether carry preserves envelope.tick, or the arrival-index scope/reset rule. Existing F1 ordering allows mixed ticks, so this is missing F2 policy rather than an existing API prohibition.

Required decision: freeze receipt/envelope/actual execution tick semantics, index assignment and overflow/reset, then assert exact order and attribution across zero-budget and multi-tick carry cases.

### R15 — P2: Prerequisite status is inconsistent across normative artifacts

Locations: [proposal](proposal.md), line7; [design](design.md), lines3–5 and55; [readiness register](plans/03-parallel-readiness.md), line15.

The readiness register and ledger correctly bind the accepted F1 seal. The proposal still declares F2 blocked on remaining final F1 evidence, and design refers to planned K0. This leaves two conflicting entry-state descriptions after F1 acceptance.

Required decision: synchronize proposal/design with the accepted source/corpus seal, retaining the distinction between accepted foundation and unimplemented F2 providers. Preserve historical ledger entries.

### R16 — P1: S2 common connection and bounded handoff are not declared

Locations: [common/Memory/TCP packets](plans/01-server-slices.md), lines69–73.

The two adapters are told to forward through common admission without a complete connection API, bounded adapter-to-core handoff, outbox drain/close interface, buffer ownership or post-login heartbeat policy. F1 frame decoding borrows the input and returns Truncated for an incomplete buffer, while the packet requires both truncated rejection and successful fragmented TCP. It does not distinguish incomplete data awaiting continuation from terminal EOF.

Required decision: freeze S2 signatures, handshake/play/close transitions, owned buffer transfer, numeric send/receive limits, fragment/EOF decisions and existing heartbeat timing, then accept this compiling common contract before either adapter consumes it.

### R17 — P1: Dirty snapshot and completion acknowledgment have no complete S3 seam

Locations: [save declarations](plans/02-core-seams.md), line29; [scheduler packet](plans/01-server-slices.md), line77.

Private authority/save state produces dirty, urgent, unload and metadata work, but S3 submit consumes an already encoded request and poll_tick receives only tick/budget. No exact seam gives the scheduler snapshots, returns failed ownership or applies committed revisions to authority. Current Go retains per-key committed/uncommitted results and rejects impossible acknowledgment revisions. The named Schedule/Retry/Backpressure/Flush filter does not execute several SaveCompletion/SaveError regressions.

Evidence: [world.go](../../../packages/server/server/persistence/world.go), line284; `TestSaveErrorAcknowledgesOnlyCommittedAndRetainsUncommitted` and existing SaveCompletion cases.

Required decision: freeze bounded snapshot selection, ownership transfer, per-ticket/per-key completion and acknowledgment interfaces; state partial, stale and future revision outcomes and select their real oracles.

### R18 — P2: Rollback has no complete callable workflow

Location: [activation packet](plans/01-server-slices.md), line85.

The binary/script flags expose activation and dry-run, but no complete rollback operation, previous-runtime identity input or executable stop/wait/restore/restart sequence. The self-test can establish a selected real binary/hash without demonstrating quiescence and rollback of a running authority.

Required decision: freeze the rollback entry and each failure result. Execute actual disposable activation, stop/quiescence, compatible verification or named-backup restore and previous-runtime acceptance, including interrupted steps. Do not substitute dry-run evidence for the real workflow.

## Controller ruling and correction order

The current task topology can be retained. Before S1 landing, reconcile compatibility decisions, complete source-bound capability coverage, freeze exact shared declarations and concrete oracles, and update every affected producer/consumer packet together. Persistence/Agent lifecycle and real integration also require their own complete decisions and bounded task/file ownership. Measure all retained ownership lanes before accepting limits. Re-run structural, type/coverage and semantic review after correction.

The source-bound inventory entry may gather current facts, but no missing design is delegated to an implementation worker. A passing strict OpenSpec validator does not accept these semantic gaps. No F2 checkbox is closed by this review. Architecture skill: no change; existing verified project rules already require these decisions.
