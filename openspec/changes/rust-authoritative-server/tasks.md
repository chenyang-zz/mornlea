# F2 Rust authoritative server implementation

The checkboxes below are the sole current node-status source. [Execution contract and dependency graph](plans/00-execution.md), [compile-ready S1 seams](plans/02-core-seams.md) and [exact worker packets](plans/01-server-slices.md) are part of this plan. These new crate/tests are prospective until their owning nodes land; `-- --list` alone is never provider acceptance. Record the accepted contract SHA, fixture identity, behavioral red/green and focused commit for each node in `ledger.md`. A worker edits only packet-owned files. F1 complete acceptance is a hard prerequisite; the current Go server remains the production authority until the separate product cutover.

[Controller algorithms and concrete acceptance tables](plans/04-refined-nodes.md) cover every command and rule branch. The node graph includes separate real filesystem and Agent lifecycle/integration owners.

See the [node-by-node dependency and ownership gate](plans/03-parallel-readiness.md) before dispatch. It refines the linked packets without duplicating status.

See the [cross-change dispatch map](../godot-default-client-switch/plans/00-cross-change-dispatch.md) for parallel lanes and serial gates.

## 1. Contract and lifecycle

- [x] 1.1a Verify the sealed complete F1 prerequisite and enumerate server capabilities.
- [x] 1.1b Record command, chunk-result and persistence high-water from every supported server replay.
- [x] 1.1c Freeze S1 bounds and source-bound capability-to-test mappings before the contract landing.
- [x] 1.2 Land compiling S1 contract, bounded types and executing consumer double; freeze its SHA.
- [x] 1.3 Implement session admission, sequenced intake and control-plane separation.
- [x] 1.4 Implement bounded tick/chunk mailboxes and cancellation.
- [x] 1.5a Implement owned tick publication and bounded slow-receiver outboxes.
- [x] 1.5b Implement one final unpublished tick and retryable shutdown phases.
- [x] 1.6 Implement authority-resolved atomic placement/mining transaction and failure invariants.

## 2. Independent authoritative rule providers

- [x] 2.1a Implement chunk acquisition and stale-generation rejection.
- [x] 2.1b Implement world placement/internal interaction geometry through the atomic transaction.
- [x] 2.1c0 Land shared source interaction target classification ([packet](plans/18-interaction-target-contract.md)).
- [x] 2.1c0a Land exact source look and ray normalization helpers ([packet](plans/20-interaction-ray-consumers.md)).
- [x] 2.1c1 Migrate remaining authority interaction rays and preserve operation-specific water targeting ([packet](plans/20-interaction-ray-consumers.md)).
- [x] 2.1c Implement continuous mining progress, tool reset and atomic completion.
- [x] 2.2 Implement time, season, weather and environment transition replay.
- [x] 2.3 Implement bounded fluid rescan and update scheduling.
- [x] 2.4a Implement bounded farmland moisture.
- [x] 2.4b Implement actor trample and snow footprints.
- [x] 2.4d0 Complete indexed Ready world reads, height cache and compact replay snapshots.
- [x] 2.4d Implement deterministic crop, dry farmland, tree, grass and snow random rules.
- [x] 2.4c Implement ordered block-support sweeps ([exact packet](plans/11-support-sweeps.md)).
- [x] 2.5a Implement authoritative player control and movement.
- [x] 2.5b Implement survival transitions and correction observations.
- [x] 2.5c Implement atomic eating inventory and hunger settlement.
- [x] 2.6a Implement inventory authority and item conservation.
- [x] 2.6b Implement containers and generation/view validation.
- [x] 2.6c Implement atomic workbench crafting.
- [x] 2.6c1 Complete workbench anchor lifecycle and source container-view replacement ([packet](plans/22-workbench-anchor-lifecycle.md)).
- [x] 2.6d Implement tick-driven furnaces.
- [x] 2.6e0 Add bounded tick-local mining suppression and action receipt preflight.
- [x] 2.6e Implement farming tools, bone meal and atomic buckets.
- [x] 2.7a Implement hostile lifecycle and motion provider.
- [x] 2.7a1 Complete frozen pre-physics hostile targeting, melee and ranged action production ([packet](plans/21-hostile-action-production.md)).
- [x] 2.7b0 Complete projectile read, compare-and-replace staging and replay initialization contracts.
- [x] 2.7b Implement projectiles and hit validation.
- [x] 2.7c0 Land frozen hostile melee input contract ([packet](plans/17-combat-input-contract.md)).
- [x] 2.7c1 Implement bounded melee snapshot and settlement ([packet](plans/19-melee-settlement.md)).
- [x] 2.7c2 Complete player, hostile and passive death outputs and reset ([packet](plans/24-death-outputs-reset.md)).
- [x] 2.7c Implement one-time hostile death and combat outcomes ([packet](plans/25-combat-outcome-integration.md)).
- [x] 2.8a Implement passive lifecycle.
- [x] 2.8b0 Complete bounded drop staging, identity and snapshot contracts.
- [x] 2.8b1 Implement bounded drop lifetime and atomic pickup.
- [x] 2.8b2 Complete fixed container ownership and atomic world outputs.
- [x] 2.8b3a Land shared deterministic harvest samplers ([contract packet](plans/12-harvest-contract.md)).
- [x] 2.8b3b Complete flooded-plant and trample drop outputs ([packet](plans/13-environment-outputs.md)).
- [x] 2.8b3c Complete human mining harvest and no-drop branches ([packet](plans/14-mining-outputs.md)).
- [x] 2.8b4a Implement selected-item and inventory/crafting panel drops ([packet](plans/15-player-drop-commands.md)).
- [x] 2.8b4b Complete container view binding and panel drop settlement ([packet](plans/16-container-drop-views.md)).
- [x] 2.8b4 Implement selected-item and panel drop commands.
- [x] 2.8b Complete drop commands and all producer integration ([packet](plans/26-drop-producer-integration.md)).
- [x] 2.8c Implement sleeping and time transition.
- [x] 2.9a Implement sessionless companion candidate provenance and admission.
- [x] 2.9b Revalidate and execute companion actions through the shared mutation pipeline ([packet](plans/23-companion-action-execution.md)).

## 3. Adapters and serial integration

- [x] 3.1 Integrate the one authoritative tick reducer after all rule providers, including the refined packet's retired-session filtering, explicit damage-victim routing and indexed observation gates ([packet](plans/27-authoritative-tick-reducer.md)).
- [x] 3.2 Land the common protocol/login/validation transport path.
- [x] 3.3a Implement the Memory adapter over common admission.
- [x] 3.3b Implement the TCP adapter over common admission.
- [x] 3.4a Implement bounded durable store mailbox.
- [x] 3.4b Implement autosave, retry, backpressure and flush scheduling.
- [x] 3.4c0 Complete decoded-load, partial-commit and cancellable persistence I/O contracts.
- [x] 3.4c Implement real region commit, crash boundaries and compaction.
- [x] 3.4d Implement standalone atomic file persistence.
- [x] 3.5 Implement exclusive world lease, recovery and named-backup rollback.
- [x] 3.6a Implement Agent HTTP wire and lease provider.
- [x] 3.6b Implement frozen snapshot registry and MCP provider ([packet](plans/04-refined-nodes.md)).
- [x] 3.6c Implement Agent task, dialogue and memory ownership ([packet](plans/04-refined-nodes.md)).
- [x] 3.6d Execute Rust integration with the actual Python Agent and MCP ([packet](plans/04-refined-nodes.md)).
- [x] 3.7a Implement tick hydration for live adapter flows ([packet](plans/28-tick-hydration.md)).
- [x] 3.7 Prove real local/remote, save/restart and Agent integration against the full inventory ([packet](plans/01-server-slices.md)) — fresh evidence 2026-10-08 for the seven previously open rows is in [evidence/3.7-rerun](evidence/3.7-rerun/REPORT.md); chen removed the native Loom review gate on 2026-10-08, see the ledger entry "Task 3.7 rerun evidence (2026-10-08)". Earlier record — accepted 2026-10-04: original limited integration at `37118f582` accepted via external independent Codex supplemental review; fresh final audit PASS; functional evidence inherited from `137e531` because runtime and non-comment test bytes are unchanged; native Loom reviewer remains timed_out/NO VERDICT — no native Loom PASS and no whole F2 acceptance. Prior review record: three Mac rounds 2026-10-03 by ZCode; precise row bindings 71 bound / 7 open implementation gaps; sealed full persistence 272/272 on `1b0367c5a`; remote re-verification blocked; see ledger.
- [x] 3.7p0 Retain canonical packet identity through owned Memory/TCP publication ([packet](plans/44-owned-wire-publication.md)).
- [x] 3.7s0 Run durable store work on a bounded background owner ([packet](plans/46-background-store-owner.md)).
- [x] 3.7p1 Publish final authoritative player state each tick ([packet](plans/47-final-player-publication.md)).
- [x] 3.7p2v Extract source-compatible off-tick chunk network snapshots ([packet](plans/64-source-compatible-network-views.md)).
- [x] 3.7p2c Land checked prepared frames and explicit publication receipts ([packet](plans/66-prepared-publication-contract.md)).
- [x] 3.7p2q Implement the actual prepared FIFO and borrowed Memory delivery ([packet](plans/67-prepared-outbox-owner.md)).
- [x] 3.7p2t Transfer immutable prepared frames through the actual TCP queue ([packet](plans/68-prepared-tcp-delivery.md)).
- [x] 3.7p2ec Land checked captured-chunk encoding ownership and request contracts ([packet](plans/69-chunk-encoding-contract.md)).
- [x] 3.7p2e Encode immutable chunk captures on bounded background CPU owners ([packet](plans/70-background-chunk-encoding.md)).
- [x] 3.7g0 Generate production seed-compatible compact chunks ([packet](plans/48-production-worldgen.md)).
- [x] 3.7g1 Prepare generated chunks on bounded background owners ([packet](plans/51-background-generation-owner.md)).
- [x] 3.7l0c Land checked prepared chunk and load-port contracts ([packet](plans/49-background-load-ports.md)).
- [x] 3.7l0 Load players and prepared chunks on the sole store owner ([packet](plans/49-background-load-ports.md)).
- [x] 3.7l1 Install actual prepared chunks at the authoritative acquisition phase ([packet](plans/60-live-prepared-acquisition.md)).
- [x] 3.7l2 Drive live chunk requests through the actual borrowed owners ([packet](plans/61-live-chunk-request-driver.md)).
- [x] 3.7l3g Qualify shared source restoration, support and spawn-column geometry ([packet](plans/80-source-actor-placement.md)).
- [x] 3.7l3s Retain bounded pending restore scan, wanted keys and exhausted revisions ([packet](plans/81-pending-restore-scan.md)).
- [x] 3.7l3p Integrate initial pending player registration and actual Ready restoration ([packet](plans/83-source-player-initial-restore.md)).
- [x] 3.7l3rc Land the common checked source player reset mapping ([packet](plans/86-source-player-reset-contract.md)).
- [x] 3.7l3mc Land managed collision-block read semantics ([packet](plans/87-live-collision-read-contract.md)).
- [x] 3.7l3mp Integrate managed collision reads in all four actual native actor-grid producers ([packet](plans/88-live-collision-actor-adapters.md)).
- [x] 3.7l3cx Land checked in-place source player context reset ownership ([packet](plans/89-source-player-context-reset.md)).
- [x] 3.7l3r Recover active source players and restart captured scans before native motion ([packet](plans/90-active-player-recovery.md)).
- [x] 3.7l3dc Land bounded prepared source player death settlement and live-bed restart payload ([packet](plans/91-source-player-death-context.md)).
- [x] 3.7l3d Integrate actual source player death after all damage and restart retained scans ([packet](plans/92-source-player-death-consumer.md)).
- [x] 3.7l3sf Record actual post-native supported player Safe checkpoints in place ([packet](plans/93-source-player-safe-checkpoint.md)).
- [x] 3.7l3tr Capture actual source player trample candidates before death and settle fixed coordinates ([packet](plans/94-source-player-trample-capture.md)).
- [x] 3.7l3sn Retain actual source player Snow travel, capture before death and settle original cells ([packet](plans/95-source-player-snow-capture.md)).
- [x] 3.7l3ac Settle successful source player Till and Mining exhaustion in their same-tick regions ([packet](plans/96-source-player-action-costs.md)).
- [x] 3.7l3ns0 Qualify the common raw-foot nonplayer thick-Snow speed contract before actual native consumers ([packet](plans/97-nonplayer-snow-scalar-contract.md)) — accepted 2026-10-04: fresh library 374/374 including the 6 prescribed Snow contracts; independent Codex source review PASS; evidence source `255a3e205`; actual native consumers were subsequently accepted in their separate companion `cd3a74b` and hostile/passive `d4aaae` stages; this node remains the scalar-contract boundary.
- [x] 3.7l3ps Retain actual passive Snow travel/cell ownership and settle captured cells in the existing late Snow region ([plan](plans/103-passive-snow-capture.md)) — accepted 2026-10-05 at `3be1988a586950cbdfb51520a836a05ee856787b`: real Claude implementation; actual Native 2 intended RED -> 5/5 GREEN, private boundaries 8/8, complete Rust gate 3402 passed/0 failed/0 ignored, release and Go SnowFootprint PASS, independent GPT-6.1-sol scoped review PASS. Original 4.1/4.2 remain open.
- [x] 3.7l3ct Preserve committed checked tuning through actual tick freeze ([plan](plans/104-committed-tick-tunables.md)) — accepted 2026-10-05 runtime SHA `d8b394add28aa2ba0f2272d0680536c950ebeb24`: real local Claude CLI implementation, tests-only `34f8596d3` actual 1 control PASS/2 intended RED -> 3/3 GREEN; full Rust 3405 passed/0 failed/0 ignored, release, three relevant Go tests and OpenSpec128 PASS; independent GPT-6.1-sol scoped PASS. This accepts committed-record consumption only; configuration-file loading and executable assembly remain open.
- [x] 3.7l3ic Preserve accepted inventory-command publication intent through final owner projection ([plan](plans/105-inventory-command-publication.md)) — accepted 2026-10-05 at runtime `e09b6301ee417b00f37703c5cbd8420c3a35f2ec`: real local Claude implementation; corrected actual round-trip/carry/equal-armor 3 intended RED plus control -> 4/4 GREEN; full Rust75 suites3409/0 failed/0 ignored, replay506, release, Go29 top-level tests and OpenSpec128 PASS; independent GPT-6.1-sol scoped PASS. Only five existing inventory::run command families are accepted; other dirty writers, automatic acquisition and original4.1/4.2 remain open.
- [x] 3.7l3cf Preserve accepted crafting-command InventoryState/CraftingState publication intent ([plan](plans/106-crafting-command-publication.md)) — accepted 2026-10-05 at runtime `9582938ed876a9f084147cd9c9265fb0a5cd3342`: real local Claude; corrected actual2 intended publication RED/control PASS ->3/3 GREEN, full Rust75 suites3412/0 failed/0 ignored/replay509, release, actual Go crafting23 and OpenSpec128 PASS; independent GPT-6.1-sol scoped PASS. Only MoveCrafting/TakeCraftingOutput intent accepted; other dirty writers and original4.1/4.2 remain open.
- [x] 3.7l3co Preserve accepted container-command owner inventory publication intent ([plan](plans/107-container-command-publication.md)) — accepted 2026-10-05 at runtime `1065295128ee3891077949b4e45e200279cf247a`: actual local Claude; two genuine panel-drop RED/control PASS ->3/3 GREEN, full Rust75 suites3415/0 failed/0 ignored/replay512, release, actual Go7 and OpenSpec128 PASS; independent GPT-6.1-sol scoped PASS. Only accepted transfer/drop inventory intent; container cadence/lifecycle and original4.1/4.2 remain open.
- [x] 3.7l3cr Register and advance actual pending source companions before player restoration ([plan](plans/108-source-companion-restore.md)) — accepted 2026-10-05 at runtime `60f89185273e2df58e259818e843f956653d259a`: actual local Claude; qualified behavior RED1 control PASS/5 producer failures ->6 actual Ready/Native GREEN, full Rust75 suites3422/zero failed/ignored/replay513, release, nonempty actual GoCompanion42 top-level and OpenSpec128 PASS; independent GPT-6.1-sol medium SCOPED_PASS. Only checked pending producer/activation/wants/action isolation; acquisition/storage bootstrap/save/cache/runtime and original4.1/4.2 remain open.
- [x] 3.7m0 Capture live metadata targets and preserve restart continuity ([packet](plans/50-live-metadata-target.md)).
- [x] 3.7r0 Move resident ownership through live ticks and finalize changed chunks only ([packet](plans/52-owned-resident-tick.md)).
- [x] 3.7r1 Bound defensive compound rollback to touched resident keys ([packet](plans/53-bounded-compound-undo.md)).
- [x] 3.7r2 Preserve chunk-local sparse observation ownership and CAS history ([packet](plans/72-chunk-owned-observations.md)).
- [x] 3.7r3c Land noncopying bounded chunk-retirement ownership and port contracts ([packet](plans/74-chunk-retirement-contract.md)).
- [x] 3.7r3e Dispose whole retired chunk owners on a bounded CPU thread ([packet](plans/76-background-chunk-retirement.md)).
- [x] 3.7r3 Reclaim clean unwanted managed chunks and prove real durable reload ([packet](plans/77-live-chunk-retirement.md)).
- [x] 3.7r4 Expose healthy committed authority reads without cloning residents ([packet](plans/84-settled-authority-read.md)).
- [x] 3.7s1 Hand immutable chunk save views to the actual store owner ([packet](plans/55-immutable-chunk-save-view.md)).
- [x] 3.7s2e Cache source-compatible payload estimates for live chunk captures ([packet](plans/62-live-chunk-payload-estimate.md)).
- [x] 3.7s2 Select current managed dirty targets and apply qualified durability ACKs ([packet](plans/63-live-chunk-durability.md)).
- [x] 3.7s3p Project settled actor fields into source-compatible durable records ([packet](plans/79-settled-actor-save-projection.md)).
- [x] 3.7n0c Land the narrow transport session contract ([packet](plans/56-live-transport-login.md)).
- [x] 3.7n0 Bind actual Memory/TCP login to the background store ([packet](plans/56-live-transport-login.md)).
- [x] 3.7a1 Retain bounded MCP connections through actual deadline-aware shutdown ([packet](plans/58-owned-mcp-shutdown.md)).
- [x] 3.7l3ir Publish source Active PlayerInput admission refusals with original acknowledgment and defensive cleanup ([packet](plans/134-source-active-input-refusals.md)). Direct Codex: four causal REDs/four new GREEN plus strengthened4096input saturation/clock case; corrected76Rust suites3722PASS0failed/ignored/replay595, source Go movement3/action2/overlay2/outbox2 race/count1PASS, revised independent SCOPED_PASS. No-tests receipts excluded; other outcomes/queue/runtime and broad3.8/4.1/4.2 OPEN.

- [x] 3.7l3iv Publish source inventory and equip command admission outcomes while retaining hard trusted staging failures ([packet](plans/135-source-inventory-command-outcomes.md)). Direct Codex: fivecausalRED/onecontrol→sixnativeGREEN and2private declaration qualifications/actualtrusted invariantGREEN;76Rust suites3730PASS0failed/ignored/replay601, Go runtime4/4 entity11/11 oracle4/0 race/count1PASS, independent SCOPED_PASS. Rawactor-free/lateSelect/dirtyintent retained; otheroutcomes/queue/runtime and broad3.8/4.1/4.2 OPEN.

- [x] 3.7l3qor Advance owner inventory/crafting mirrors only after actual queue admission ([packet](plans/145-source-owner-record-admission.md)). Other mirrors/background encoding/runtime and broad3.8/4.1/4.2 OPEN.

- [x] 3.8 Qualify explicit opt-in activation and rollback without changing default startup ([packet](plans/04-refined-nodes.md)); evidence in [`evidence/3.8-activation-rollback/REPORT.md`](evidence/3.8-activation-rollback/REPORT.md) ([PR #22](https://github.com/chenyang-zz/mornlea/pull/22)).
- [x] 3.8r0 Retain the exclusive world lease throughout actual backup restoration ([packet](plans/65-restore-world-lease.md)).
- [x] 3.8v0 Qualify the actual previous Go read-only save verifier ([packet](plans/54-previous-runtime-verifier.md)).
- [x] 3.8v1 Rebuild and bind the sealed previous runtime package ([packet](plans/57-prepare-previous-package.md)).
- [x] 3.8v2 Invoke the actual sealed verifier before starting the previous runtime ([packet](plans/59-previous-verifier-consumer.md)).

## Review repairs

The [review repair packet](plans/29-review-repairs.md) is the execution authority for bounded corrections discovered during whole-branch review. Provider tests do not close the reopened production integration gate.

- [x] 3.9l1 Make the MCP serve-failure fixture portable across Unix socket layouts.
- [x] 3.9l2 Bound loopback delivery waits by real deadlines while preserving protocol clocks.
- [x] 3.9l3 Recognize proven Linux zombie termination without weakening writer-lock guards ([packet](plans/33-linux-process-termination.md)).
- [x] 3.9l4 Drive actual TCP receive progress through asynchronous login readiness ([packet](plans/71-prepared-tcp-receive-progress.md)).
- [x] 3.9l5 Mint distinct admitted identities in actual two-container-viewer fixtures ([packet](plans/75-distinct-container-viewer-fixture.md)).
- [x] 3.9l6 Recognize EOF or peer reset as control connection retirement without accepting a timeout ([packet](plans/82-control-retirement-fixture.md)).
- [x] 3.9l7 Count retained semantic and cancellation-join memory obligations after timeout ([packet](plans/85-memory-timeout-obligation-fixture.md)).
- [x] 3.9l8 Drive actual live TCP input during delivery and held-login admission ([packet](plans/98-live-tcp-fixture-progress.md)).
- [x] 3.9l9 Await actual nonblocking loopback acceptance in the prepared-owner fixture ([packet](plans/99-prepared-tcp-accept-fixture.md)).
- [x] 3.9l10 Preserve actual noncanonical paths and valid bank handoff in verifier fixtures ([packet](plans/100-storage-verifier-fixtures.md)) — accepted 2026-10-04: fresh sealed producer and full persistence 272/272 PASS; independent Codex source review PASS; evidence source `c8ed2ff52`, current Go fixture/oracle `360609e4`.

- [x] 3.9a Preserve player runtime lanes through movement.
- [x] 3.9b Carry environment and sleep state across live ticks.
- [x] 3.9c1 Reclaim terminal common transport ownership with bounded diagnostics.
- [x] 3.9c2 Reap terminal TCP sockets and bound send work.
- [x] 3.9c3 Share bounded deferred command ownership across provider phase roles ([packet](plans/73-shared-deferred-command-ownership.md)).
- [x] 3.9e0 Fence hard tick failures and execute one actual unpublished final reduction ([packet](plans/78-failed-tick-and-final-reducer.md)).
- [x] 3.9e Repair retained region parent durability barriers.
- [x] 3.9f Use OS entropy for snapshot bearer capabilities.
- [x] 3.9g Reconcile Rust backup compatibility identities and English comment audit.
- [x] 3.9h Bound activation control requests.
- [x] 3.9i Correlate save completion ownership and prevent duplicate retries.
- [x] 3.9p1 Repair survival damage, fall and boundary semantics.
- [x] 3.9p2 Stage validated player controls before action providers and interrupt rejected actions.
- [x] 3.9p3 Preserve held sprint intent, gate effective physics and skip reset movement.
- [x] 3.9p4 Restore sneak-edge support probes and thick-snow speed scaling.
- [x] 3.9t0 Expose the accepted per-tick survival, eating and furnace tuning snapshot.
- [x] 3.9t1e Consume configured eating duration at the settlement boundary.
- [x] 3.9t1s Consume configured survival timing from one tick snapshot.
- [x] 3.9t1f Consume configured furnace timing from one batch snapshot.
- [x] 3.9s1 Keep sleep refusals atomic and check bed partner coordinates.
- [x] 3.9s2 Prune disconnected sleep anchors before admitting replacement sessions.
- [x] 3.9w1 Enforce source sneak-door gates after target classification.
- [x] 3.9q1 Synchronize daylight burn health before hostile death settlement.
- [x] 3.9q2 Rehearse cumulative player-death drop capacity across slots.
- [x] 3.9q3 Share first-valid companion mining action and retain held intent.
- [x] 3.9q4 Correct passive wander angular quantization.
- [x] 3.9q5 Detect upper hostile body intersections with fluid.
- [x] 3.9q6 Filter dead players before hostile target selection.
- [x] 3.9q7 Preserve hostile target identity and successful repath cadence.
- [x] 3.9q7a Preserve due-first hostile UUID selection and calendar repath cadence ([packet](plans/32-hostile-path-cadence.md)).
- [x] 3.9q7b Revalidate covered Ready chunk revisions across committed live ticks ([state prerequisite](plans/35-ready-revision-commit.md); [provider packet](plans/36-hostile-ready-revisions.md)).
- [x] 3.9q8 Compare passive birth neighborhoods in chunks.
- [x] 3.9q9 Refuse unrepresentable hostile geometry without partial effects.
- [x] 3.9q10 Reconcile and apply executing-tick hostile shot spread.
- [x] 3.9m1 Match placement refusal precedence and ordinary fluid replacement.
- [x] 3.9m2 Validate crop, sapling and torch support before placement.
- [x] 3.9m3 Refuse player-overlapping placement target shapes.
- [x] 3.9m4 Revalidate resolved geometry and active actor bases at commit.
- [x] 3.9m5 Validate compound inventory preimages in cumulative order.
- [x] 3.9d1 Enforce Agent HTTP absolute deadlines and close cancellation.
- [x] 3.9d2a Accept a clonable request cancellation contract with real HTTP evidence.
- [x] 3.9d2 Bound and reclaim Agent business request ownership.
- [x] 3.9d3 Retain and reap Agent control and business workers through bounded close.
- [x] 3.9d4 Admit frozen finalization while fencing new or late plans.
- [x] 3.9d5 Transfer and retire host/memory terminal request ownership once.
- [x] 3.9d6 Drive retryable memory finalization through bounded shutdown ([provider](plans/38-memory-finalization-progress.md), [real integration](plans/40-real-memory-shutdown.md), [panic consumers](plans/42-panic-retirement-consumers.md), [entry quiescence](plans/43-shutdown-entry-quiescence.md)).

- [x] 3.7l3ca Join automatic source player/companion wants, priority and actual chunk driver at the original phase seams ([plan](plans/109-source-acquisition.md)) — accepted 2026-10-05: implementation `9fbfa118745f7f0043616a55322a4876a352d908`, formally reviewed after dev synchronization at `9811adb6fe50a40c7133d55b725b48e4dc71825d`; coverage-only follow-up `d84c863871b94987fc50a1ae2f19c333aff0957c`. Qualified original 9 behavior RED/manual control PASS becomes 10/10 actual GREEN; original test prefixes and the three cleanup-only drain calls are verified. Fresh GPT-6.1-sol high runtime SCOPED_PASS and test-delta PASS; the sole Minor coverage finding is resolved by an additional quiet-generation/FIFO/Failed-preservation case. Full Rust fmt/Clippy and 75 suites/3450 passing results including doctests, zero failed/ignored; actual Go runtime 33 top-level and entity 112 top-level cases PASS. Release build, six-module vet/short and strict OpenSpec 128 results are source-bound in the ledger. Only explicit library acquisition is accepted; executable composition, bootstrap, automatic actor saves/cache, trusted observer and original4.1/4.2 remain open. Historical controller failures and usage states are preserved.

- [x] 3.7l3cq Settle Crafting-view partial and quick moves with atomic source-compatible state and final owner publication ([packet](plans/110-crafting-partial-quick.md)) — direct Codex implementation; independent native source review SCOPED_PASS; genuine7behavior RED plus2refusal controls, initial9GREEN and final525 replay PASS; Rust fmt/Clippy75suites3460results PASS. Only this command node is accepted; broad3.8/4.1/4.2 remain open.

- [x] 3.7l3dp Preserve full surviving drop wire values and source remove-before-upsert event order ([packet](plans/111-drop-wire-mirror.md)). Accepted direct Codex node:2genuine behavior RED plusquiet control,3GREEN, full528replay and75Rust suites3463results PASS; independent native SCOPED_PASS. Event construction only; queue admission/mirror and broad3.8/4.1/4.2 remain open.

- [x] 3.7l3eo Preserve despawn-before-spawn-before-state order for mixed hostile/passive/projectile transitions ([packet](plans/112-mixed-entity-publication-order.md)). Direct Codex node:3qualified ordering REDs then3GREEN,531replay and75Rust suites3466results PASS; native independent SCOPED_PASS. Event construction only; queue admission/mirror and broad3.8/4.1/4.2 remain open.

- [x] 3.7l3as Land the bounded retained actor save ledger and exact latest/flight/revision contract ([packet](plans/113-retained-actor-save-ledger.md)). Direct Codex callable-contract/provider landing: six genuine review-correction REDs then20GREEN, inline mailbox consumer with explicit backend double, Rust75suites3486results PASS and native independent SCOPED_PASS. Automatic capture/cache/bootstrap/physical disk/runtime and broad3.8/4.1/4.2 remain open.

- [x] 3.7l3sp Preserve ordered progress for fresh and retry save cohorts exceeding available mailbox reservations ([packet](plans/114-save-admission-progress.md)). Direct Codex:3initial assertion REDs plus actual fresh-flush review RED, final5GREEN including two real8chunk save/reopen paths; corrected Rust75suites3491results PASS, native SCOPED_PASS. Actor-state/cache/bootstrap/runtime and broad3.8/4.1/4.2 remain open.

- [x] 3.7l3ao Integrate bounded actor ledger selection, mixed completion preflight, statistics and flush ownership into the live authority ([packet](plans/115-authority-actor-save-routing.md)). Direct Codex: missing-API declaration RED, three genuine mixed-chunk-forgery review REDs, final13GREEN including real background autosave/Closing flush and five-family reopen; Rust75suites3504results PASS, native corrected SCOPED_PASS. Automatic producers/cache/bootstrap/runtime and broad3.8/4.1/4.2 remain open.

- [x] 3.7l3pc Integrate source player cache-before-load, name/missing confirmation, indexed settled capture and lossless retirement with the actual shared login driver ([packet](plans/116-source-player-persistence.md)). Direct Codex: declaration-only RED plus genuine stale-retirement RED, corrected16topic/2private GREEN; real DiskStore/Memory actions/autosave, TCP cache reconnect, offline final flush and all-field reopen; full75Rust suites3522results PASS, independent corrected SCOPED_PASS. Prepared Pending control does not claim an executed death producer; executable/aggregate/bootstrap and broad3.8/4.1/4.2 remain open.

- [x] 3.7l3mr Restore complete hostile/passive aggregates, bound terminal residency and capture settled/final live rosters through actual persistence ([packet](plans/117-source-mob-persistence.md)). Direct Codex:4genuine behavior REDs,18topic/3private GREEN; actual disk/native death and quiet removal, Died/Vanished/loot/shard ownership, exact failed-save retry and later final flush/reopen; Rust75suites3543results PASS, independent SCOPED_PASS. Passive lethal input and shard ownership are prepared controls with real downstream providers; executable and broad3.8/4.1/4.2 remain open.


- [x] 3.7l3cm Land source-compatible pure companion missing/legacy/v5 configuration and lifecycle merge before authoritative consumers ([packet](plans/118-companion-configuration-merge.md)). Direct Codex: declaration-only REDs,24GREEN including19actual unchanged Go merge/encode outcomes and original non-Clone entropy I/O cause; full Rust76suites3567results PASS, independent revised SCOPED_PASS. Startup/capture/CAS consumers and broad3.8/4.1/4.2 remain open.

- [x] 3.7l3tp Qualify complete chat-owned companion task restoration and settled queue normalization before authoritative consumers ([packet](plans/119-companion-task-persistence.md)). Direct Codex: declaration-only REDs plus genuine typed-chat/publication REDs,20new focused GREEN; full Rust76suites3587results and all4actual Go restore controls PASS after verified permission restoration; independent SCOPED_PASS/DOCS_PASS. This accepts task owner and prepared publication only; automatic bootstrap/capture/path/Agent/executable and broad3.8/4.1/4.2 remain open.

- [x] 3.7l3cp Join complete durable companion startup and settled body/task capture to the authority with source raw-task dirty semantics ([packet](plans/120-authoritative-companion-persistence.md)). Direct Codex: qualified API RED, seven initial capture assertion REDs plus capacity/hidden-family correction REDs;23topic/4private GREEN, four actual disk caller/placement/save/retry/final/reopen controls and three exact full Go/Rust raw-observation outcomes; Rust76suites3614PASS, Go12-race/count1PASS, independent SCOPED_PASS/DOCS_PASS. Model summary is actual Go plus private Rust control; memory CAS/final handoff/Agent/config/executable and broad3.8/4.1/4.2 remain open.

- [x] 3.7l3mw Land source-compatible lifecycle memory reads/CAS over the sole companion ledger, including occupied-revision refusal and healthy Closing capture ([packet](plans/121-companion-lifecycle-memory-cas.md)).

- [x] 3.7l3ma Gate Agent memory settlement and final drain through authoritative lifecycle CAS before mirror/readiness/reservation acceptance ([packet](plans/122-authoritative-memory-settlement.md)).

- [x] 3.7l3oi Preserve Active owner wire mirrors and successful workbench/eating/pickup publication intent ([packet](plans/123-active-owner-publication-intent.md)).

- [x] 3.7l3wb Preserve empty physical-slot revision barriers and bounded radius-two Ready drop publication ([packet](plans/124-source-world-publication-boundaries.md)).

- [x] 3.7l3cl Preserve source final container invalidation, complete cadence and owner record order ([packet](plans/125-source-container-publication-lifecycle.md)).

- [x] 3.7l3lc Settle actual source lifecycle commands in envelope order before actor advancement ([packet](plans/126-immediate-container-lifecycle-commands.md)). Direct Codex: genuine registration RED plus seven qualified actual hold/command-timing REDs, nine GREEN and564replay; corrected76Rust suites3682PASS0failed/ignored, Go8-race/count1PASS, revised independent SCOPED_PASS. Prepared geometry is not the source view marker; raw provider compatibility remains. Broad3.8/4.1/4.2 OPEN.

- [x] 3.7l3ms Enforce source human mining suspension and native open/close interruption ([packet](plans/127-source-human-mining-suspension.md)). Direct Codex: four prepared provider causes and two actual14tick mining/open/close cases yield six causal REDs then six GREEN/570replay; Rust76suites3688PASS0failed/ignored, Go3-race/count1PASS, independent SCOPED_PASS. Whole lifecycle producers/runtime and broad3.8/4.1/4.2 OPEN.

- [x] 3.7l3bs Enforce source workbench sneak eligibility without changing grid/anchor/lease or publication intent ([packet](plans/128-workbench-sneak-eligibility.md)). Direct Codex: three native prefix/conservation/intent plus one raw compatibility causal REDs become four GREEN/574replay; Rust76suites3692PASS0failed/ignored, Go4-race/count1PASS, independent SCOPED_PASS. Wire/auto-close/whole outcomes/runtime and broad3.8/4.1/4.2 OPEN.

- [x] 3.7l3bc Preserve complete source workbench close success intent and automatic hard-failure fencing ([packet](plans/129-source-workbench-close-outcomes.md)). Direct Codex: seven causal REDs (sixnew plus revised sibling),580replay and six sticky fault controls GREEN;76Rust suites3698PASS0failed/ignored, unchanged Go7-race/count1PASS, independent SCOPED_PASS. Prepared fault causes are separate from actual open/mining success; wire/queue/runtime and broad3.8/4.1/4.2 OPEN.

- [x] 3.7l3ocr Land checked shared command disposition and bounded ordered refusal contract with executing consumer doubles ([packet](plans/130-checked-command-outcome-contract.md)). Direct Codex: qualified declaration-only RED, five executing contract GREEN; corrected76Rust suites3703PASS0failed/ignored, independent SCOPED_PASS. Actual providers/Pending cleanup/source phase/tick/queue integration and broad3.8/4.1/4.2 OPEN.

- [x] 3.7l3pr Join source Pending command refusals and defensive action cleanup to the actual ordered tick ([packet](plans/131-source-pending-command-refusals.md)). Direct Codex: two causal REDs plus two interface qualifications, four new GREEN/six unchanged fault controls;76Rust suites3707PASS0failed/ignored, Go source lifecycle-race/count1PASS, independent SCOPED_PASS. Prepared acquisition/fault causes do not accept upstream producers or transport parity; other family/phase/queue/runtime and broad3.8/4.1/4.2 OPEN.

- [x] 3.7l3hp Settle actual hotbar selection after actor advancement in the ordered interaction prefix ([packet](plans/132-source-hotbar-selection-phase.md)). Direct Codex: three causal REDs/reverse control then four GREEN;76Rust suites3711PASS0failed/ignored/replay584, actual unchanged Go5 runtime and disposable overlay3 source phase cases race/count1PASS, independent SCOPED_PASS. Raw compatibility retained; wire/other phases/queue/runtime and broad3.8/4.1/4.2 OPEN.

- [x] 3.7l3pl Apply source placement look during admission without losing later input or raw motion compatibility ([packet](plans/133-source-placement-admission-look.md)). Direct Codex: five original causal REDs plus independent near-pi review RED and one reverse control, seven GREEN; corrected76Rust suites3718PASS0failed/ignored/replay591, Go4 overlay/3 movement/5 placement race/count1PASS, corrected independent SCOPED_PASS. Raw compatibility retained; late placement/other outcomes/queue/runtime and broad3.8/4.1/4.2 OPEN.

- [x] 3.7l3cfr Publish source ordinary crafting admission dispositions with atomic owner intent and trusted failure separation ([packet](plans/136-source-crafting-command-outcomes.md)). Direct Codex: six native causalRED/onecontrol then7GREEN, two actual trustedfaultRED/GREEN;76Rust suites3739PASS0failed/ignored/replay608, unchangedGo15top/14sub and externaloracle4top/0sub race/count1PASS, independent SCOPED_PASS. Prepared causes do not accept upstream/runtime; othercommand/held/queue/config/runtime and broad3.8/4.1/4.2 OPEN.

- [x] 3.7l3lor Publish exact immediate Open/Close refusal outcomes and physical sneak precedence with hard trusted staging ([packet](plans/137-source-lifecycle-command-outcomes.md)). Broad3.8/4.1/4.2 OPEN.

- [x] 3.7l3ctr Publish exact late whole/partial/quick container transfer outcomes and hard trusted staging ([packet](plans/138-source-container-transfer-outcomes.md)). Container drop/other outcomes and broad3.8/4.1/4.2 OPEN.

- [x] 3.7l3cdr Publish exact late container drop refusals and hard trusted preparation/staging ([packet](plans/139-source-container-drop-outcomes.md)). Native8/private1 GREEN; six causal REDs, one recovery-precedence qualification, one control;76Rust3767/replay632, Go4 oracle and actual controls race/count1 PASS, independent source SCOPED_PASS. Full-tick recovery8 and direct phase3 separately qualified; docs/immutable closing evidence in packet. Other outcomes and broad3.8/4.1/4.2 OPEN.

- [x] 3.7l3idr Publish selected and inline panel drop intake/settlement outcomes ([packet](plans/140-source-inline-drop-outcomes.md)). Native8/private1 GREEN; eight initial causal REDs plus review-caused panel lifecycle RED;76Rust3776/replay640, Go5 source oracle and unchanged controls race/count1 PASS, independent corrected source SCOPED_PASS. Selected late8 versus panel current empty9/foot3 afteractualreset; rawtypedcompat preserved; docs/immutable closing evidence in packet. Other outcomes and broad3.8/4.1/4.2 OPEN.

- [x] 3.7l3pfr Land checked placement preflight provenance and source footprint/support readiness ([packet](plans/141-source-placement-preflight-contract.md)). Raw4/private2 GREEN; three causal REDs/onecontrol plus existing torch/water fixture reconciliation;76Rust3782/replay644, Go4 actual source oracle and12top/18sub controls race/count1 PASS, independent finalFOURsource SCOPED_PASS. Sparse support/owned pose consumer qualifications retained; live placement wire/success and broad3.8/4.1/4.2 OPEN.

- [x] 3.7l3pwr Publish exact live placement outcomes and post-commit owner confirmation ([packet](plans/142-source-live-placement-outcomes.md)). Native9/private1 GREEN; eight original causal failures plus corrected physical capacity witness, all history retained;76Rust3792/replay653, Go5 source oracle and7top/2sub controls race/count1 PASS, independent finalFIVEsource SCOPED_PASS. Prepared consumer versus real runtime qualifications and docs/immutable closing evidence retained. Other command/runtime outcomes and broad3.8/4.1/4.2 OPEN.

- [x] 3.7l3twr Publish source tool intake and late command outcomes with trusted failure separation and accepted inventory intent ([packet](plans/143-source-tool-command-outcomes.md)). Held and remaining runtime outcomes/broad3.8/4.1/4.2 OPEN.

- [x] 3.7l3hmr Publish source held mining completion refusals and canonical held ray with trusted failure provenance ([packet](plans/144-source-held-mining-outcomes.md)). Only this provider node; broad3.8/4.1/4.2 and configured/runtime/queue/actor work OPEN.

- [x] 3.7l3arr Preserve captured remote and companion reset with unchanged source consumption cadence ([packet](plans/146-source-actor-reset-capture.md)). Only this private projection node; other actor/queue/runtime work and broad3.8/4.1/4.2 OPEN.

- [x] 3.7l3rri Distinguish same-UUID remote session replacement and preserve despawn/spawn/survivor ordering ([packet](plans/147-source-remote-reconnect-identity.md)). Other actor/queue/runtime work and broad3.8/4.1/4.2 OPEN.

- [x] 3.7l3qsa Advance full snapshot mirrors only after actual queue admission ([packet](plans/148-source-snapshot-admission.md)). Delta/actor/encoding/runtime and broad3.8/4.1/4.2 OPEN.

- [x] 3.7l3qda Advance contiguous delta mirrors only after matching-base frame queue admission ([packet](plans/149-source-delta-admission.md)). Wholebatch overflow/actor/encoding/runtime and broad3.8/4.1/4.2 OPEN.

- [x] 3.7l3qdv Reject invalid eligible whole delta publications at the source recipient queue boundary ([packet](plans/150-source-delta-validation.md)). Actor timing/encoding/runtime and broad3.8/4.1/4.2 OPEN.

- [x] 3.7l3qrp Advance remote UUID/incarnation membership only after frame queue admission ([packet](plans/151-source-remote-queue-admission.md)). Foot visibility/other actor mirrors/cross-peer timing/runtime and broad3.8/4.1/4.2 OPEN.

- [x] 3.7l3qcp Advance companion membership only after exact frame queue admission ([packet](plans/152-source-companion-queue-admission.md)). Foot gating/other actor mirrors/runtime and broad3.8/4.1/4.2 OPEN.

- [x] 3.7l3qhs Advance complete hostile packet memberships only after queue admission ([packet](plans/153-source-hostile-queue-admission.md)). Foot cuts/other mirrors/cross-peer timing/runtime and broad3.8/4.1/4.2 OPEN.
- [x] 3.7l3qps Implement complete passive spawn/despawn queue membership with unchanged Died/Vanished source reasons; follow [private plan](plans/154-source-passive-queue-admission.md), RED9/GREEN9, provider and scoped closing gates.
- [x] 3.7l3qpj Implement complete projectile queue membership under exported128 wire cap; follow [private plan](plans/155-source-projectile-queue-admission.md), RED9/GREEN9, native/source controls and scoped closing gates.

- [x] 3.7l3qdp Implement complete drop wire-value/removal queue admission under exported32 frame cap; follow [private plan](plans/156-source-drop-queue-admission.md), RED11/GREEN11 and real-provider/scoped closing evidence.

- [x] 3.7l3dfi Preserve physical integer drop block indices in source publication without changing numerical centers; follow [private plan](plans/157-source-drop-physical-index.md), qualified RED3/GREEN3, eleven final focused gates/full3909/native-before-six-Go controls and independent scoped review. Prepared far consumer fidelity only; actor/snapshot/runtime and broad3.8/4.1/4.2 remain OPEN.

## 4. Closeout


- [ ] 4.1 Reconcile zero-gap supported Go-to-Rust outcome inventory, real-provider/integration evidence and guides. Remaining composition gaps include trusted-observer and cross-peer retirement/visibility behavior, configured companion bootstrap/Agent execution and combined durable shutdown handoff, assembled gameplay executable adopting accepted acquisition/encoding and player/mob persistence, configuration-file loading/runtime ownership, and complete source fluid rescan/update configuration parity. Accepted library queue-admission/mirror, current capture, actual CPU and retained encoded source caller boundaries remain accepted; executable adoption and every supported outcome mapping still require evidence. Declared session-radius wanted/resync publication and passive-death reason publication are already present; do not reopen them.
- [ ] 4.2 Run complete Rust, Go, audit and OpenSpec stage gates on the recorded SHA.
- [ ] 4.3 When the production host tick loop lands, include tests that use the manually advanced host-layer tick clock and cover four properties ([decision](design.md#host-tick-loop-overload-policy-decision)): (1) the average rate does not exceed 20 ticks per second; (2) at most one missed tick is carried over and run immediately, further missed ticks are dropped, and dropped ticks are not caught up; (3) world time advances only on executed ticks; (4) one-shot actions submitted while ticks are dropped carry over to the next executed tick and all run in per-session submission order, with none lost, while movement frames may still be coalesced to the latest frame per tick. This is an F2 archive gate: if F2 is archived before a production loop exists, this line must be explicitly moved to P14 (`godot-default-client-switch`), not deleted.
- [x] 4.4 Derive season and season progress from one Rust source: `rules/environment.rs` `season_at` and `season_progress_at`, called by `rules/sleep.rs` and `core/player_publication.rs` ([PR #20](https://github.com/chenyang-zz/mornlea/pull/20)); `season_derivation_matches_go_pins` pins 17 values to the Go formula.
- [ ] 4.5 Before archiving F2 or merging it into dev, verify that every paired Go/Rust fix landed during the interim exists on both `dev` and the PR #5 branch (`cursor/rust-authoritative-server-98e6`). This is an F2 archive gate.
- [x] 4.6 Derive year index and year phase from one Rust source: `rules/environment.rs` `year_index` and `year_phase_at`, called by sleep, hostiles, passives, player publication and random blocks; `year_phase_derivation_matches_go_pins` pins Go `core.yearIndex` / `YearPhaseAt` values. This is an F2 archive gate ([PR #21](https://github.com/chenyang-zz/mornlea/pull/21)).
- [ ] 4.7 Derive day arc and effective day phase from one Rust source: move `day_arc_ticks` and `effective_day_phase*` (currently duplicated in `rules/sleep.rs`, `rules/hostile_actors.rs` and `rules/passives.rs`, and computed inline for temperature in `core/player_publication.rs` and for climate in `rules/random_blocks.rs`) into `rules/environment.rs`, pinned to Go values first. Migration lands in its own PR after 3.8 and 4.1. This is an F2 archive gate.
- [ ] 4.9 Port non-Overworld companions. Go places configured companions at the world metadata spawn dimension, but Rust core accepts only Overworld companion bodies, so a world whose spawn dimension is not the Overworld currently keeps its stored companion records and save bytes unchanged, spawns none, and reports a `NonOverworldSpawn` warning (`runtime/companion.rs`, [PR #25](https://github.com/chenyang-zz/mornlea/pull/25)). Port non-Overworld companion bodies through restore, motion, publication and persistence, pinned to Go values first, then spawn configured companions at the metadata spawn dimension. This is an F2 archive gate.

- [x] 3.7l3ssb Bound per-session source snapshot selection by count/section bytes and resync/distance/key order ([private plan](plans/158-source-snapshot-selection.md)); causal RED8+replayRED1, final8/1/6/13/3/192/capture10/full3917/native-before-Go7+1 and independent source SCOPED_PASS. Prepared consumer and actual Go representation qualifications; separate actor/async/runtime and broad3.8/4.1/4.2 OPEN.

- [x] 3.7l3afv Gate Active observer actor foot visibility on per-session snapshot history and preserve early departure/late arrival order ([private plan](plans/159-source-actor-foot-visibility.md)); qualified revised RED ten causal failures/two controls then GREEN12; source3 SCOPED_PASS, eleven focused gates/full3929/76/replay677/native-before-Go26+1. Pending observer, async/config/runtime and broad3.8/4.1/4.2 OPEN.

- [x] 3.7l3psv Capture per-session publication subscriptions at the source mid-tick phase and publish registered Pending terrain ([private plan](plans/160-source-publication-subscriptions.md)); actual consumer RED/GREEN, private bounds and independent scoped closing gates. Pending entity families and broad3.8/4.1/4.2 remain OPEN.

- [x] 3.7l3peo Publish four entity families for registered Pending observers from their captured per-session foot snapshots ([private plan](plans/161-source-pending-observer-entities.md)); actual tick/FIFO, Closed/prefix and unchanged Active guard controls. Broader runtime and3.8/4.1/4.2 remain OPEN.

- [x] 3.7l3esa Admit exactly correlated off-tick encoded snapshots into the authority FIFO with Queued-only snapshot mirrors ([private plan](plans/162-encoded-snapshot-admission.md)); actual CPU/full Memory ownership and context/error/Closed/saturation controls. Source async selection/runtime and broad3.8/4.1/4.2 remain OPEN.

- [x] 3.7l3ecp Retain and move complete semantic snapshot/frame publication owners from actual CPU results ([private plan](plans/163-encoded-snapshot-publication-parts.md)); declaration/causal RED, actual factory/pool and independent scoped gates before consumer integration.

- [x] 3.7l3esp Publish paired CPU snapshot frames through whole-publication preflight and move semantic output through the reducer ([private plan](plans/164-prepared-source-publication.md)); actual CPU/Memory ownership and atomic/Closed/prefix controls before source selector integration.

- [x] 3.7l3seo Own bounded actual CPU source snapshot requests, exact whole results and cancellation/shutdown bookkeeping ([private plan](plans/165-source-snapshot-encoding-owner.md)); source selector/current-version truth and broad3.8/4.1/4.2 remain OPEN.

- [x] 3.7l3snc Capture current Ready source identity and immutable network targets without persistence estimates or caller normalization ([private plan](plans/166-source-snapshot-capture.md)); seven prepared-cause/actual CPU tests and two actual disk/native-generation consumers, checked revision exhaustion, source SCOPED_PASS and fullRust3977; separate closing docs/immutable evidence in packet. Async timing/runtime and broad3.8/4.1/4.2 remain OPEN.

- [x] 3.7l3stc Retain exclusive source reduction ownership across post-commit suspension and complete the original tick once ([packet](plans/167-exclusive-source-tick-continuation.md)); current synchronous consumer, seven trusted fault/drop cases, derived row guards, actual acquisition/driver controls and fullRust3984/sourceSCOPEDPASS; separate closing docs/immutable evidence in packet. Async selector/runtime and broad3.8/4.1/4.2 OPEN.

- [x] 3.7l3spb Bound whole-source-pass prepared snapshot pairs from validated player and per-recipient counts while retaining legacy8 ([packet](plans/168-whole-source-pass-snapshot-budget.md)); five actual CPU/builder cases, qualified RED3/control2, revised source3 SCOPED_PASS and twelve final gates/fullRust3989; separate closing docs/immutable evidence in packet. Automatic selection/512FIFO/runtime/broad3.8/4.1/4.2 OPEN.

- [x] 3.7l3srf Preserve successful source snapshot prefixes when malformed Ready data closes only its recipient ([packet](plans/169-source-snapshot-ordered-refusal.md)); thirteen frozen prepared/actual CPU/reducer cases, qualified RED10/control3, revised source8 SCOPED_PASS and seventeen source-bound final gates/fullRust4002; docs/immutable evidence in packet. Async selection/runtime/broad3.8/4.1/4.2 OPEN.

- [x] 3.7l3set Integrate automatic source acquisition with one retained real tick, bounded actual CPU snapshot selection and original prepared FIFO delivery ([packet](plans/170-source-encoded-tick-integration.md)); frozen private11/revised actual4, qualified RED5/control6+RED4, complete decoded Disk/native body oracles, revised source13 SCOPED_PASS and fullRust4019/76/replay679. Closing docs/immutable evidence is recorded separately. Executable, sealed previous build-target isolation and broad3.8/4.1/4.2 OPEN.

- [x] 3.8btiso Isolate sealed previous-package native build artifacts from caller Cargo targets and verify actual package/previous-consumer continuity ([packet](plans/171-sealed-previous-build-target-isolation.md)); build ownership only, broad3.8/4.1/4.2 OPEN.

- [x] 3.7fgb Share the section-aware fluid rescan target across the mixed-dimension FIFO while retaining per-dimension statistics and exact carry ([packet](plans/172-shared-fluid-rescan-work-budget.md)); accounting only, automatic whole-chunk/config/runtime and broad3.8/4.1/4.2 OPEN.
