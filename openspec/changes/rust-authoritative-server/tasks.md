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
- [x] 3.7 Prove real local/remote, save/restart and Agent integration against the full inventory ([packet](plans/01-server-slices.md)) — accepted 2026-10-04: original limited integration at `37118f582` accepted via external independent Codex supplemental review; fresh final audit PASS; functional evidence inherited from `137e531` because runtime and non-comment test bytes are unchanged; native Loom reviewer remains timed_out/NO VERDICT — no native Loom PASS and no whole F2 acceptance. Prior review record: three Mac rounds 2026-10-03 by ZCode; precise row bindings 71 bound / 7 open implementation gaps; sealed full persistence 272/272 on `1b0367c5a`; remote re-verification blocked; see ledger.
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
- [ ] 3.8 Qualify explicit opt-in activation and rollback without changing default startup ([packet](plans/04-refined-nodes.md)).
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

- [ ] 3.7l3ca Join automatic source player/companion wants, priority and actual chunk driver at the original phase seams ([plan](plans/109-source-acquisition.md)) — tests checkpoint b4602d03add639d6b7272bbfbc04d9db8882b927 compiles/runs9behaviorRED+manual1PASS and phaseguards2RED; old b9 producer capture failure remains preserved; accepted frozen4a produced compile-ready draft4ae030ba with private4PASS/1FAIL; actual repair added source inputs/state wrappers but fails compiler E0432 and retains C1 mismatches. Automatic reducer/driver join is still absent; fresh supported integration remains required. Real Disk/native GREEN, exact-SHA gates and independent review remain required. Original4.1/4.2 and runtime/bootstrap/save/cache stay open.

## 4. Closeout

- [ ] 4.1 Reconcile zero-gap inventory, real-provider/integration evidence and guides. Verified remaining source gaps: automatic acquisition subscription union, dirty-publication outcome reconciliation, automatic actor save/cache lifecycle, nonempty bootstrap, assembled gameplay executable, every supported Go-to-Rust outcome inventory, and configuration-file loading/runtime ownership. Declared session-radius wanted/resync publication and passive-death reason publication are already present; do not reopen them.
- [ ] 4.2 Run complete Rust, Go, audit and OpenSpec stage gates on the recorded SHA.
