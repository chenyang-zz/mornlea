# Source workbench sneak eligibility implementation plan

Goal: a recognized workbench hit while source held sneaking is true refuses before grid, anchor, lease or publication intent changes. Architecture: the existing shared atomic bench arm adds the source eligibility check after target classification; live immediate intake and raw deferred compatibility reuse it. Tech: staged ActorRuntime.controls/HeldActions and existing OpenOutcome. Spec: ../design.md and ../specs/rust-authoritative-server/spec.md. Root direct executing-plans/TDD; tasks.md alone owns status.

## Baseline and ownership

Accepted predecessor734b0bd4e7d51fd40eab3a4be83be2d885bf217c. Exact crate-relative editable files AGENTS.md, src/rules/crafting.rs, tests/server_replay/crafting.rs, tests/server_replay/publication_projection.rs (module registration), NEW tests/server_replay/publication_bench_eligibility.rs. Active design/ledger/tasks/new packet editable. Existing command_lifecycle::scene source-enabled fixture is read-only. Go, schema, manifests, sealed corpus, all other providers/tests/dev/archive bytes read-only. Root sole designer/writer/integrator/rollback; native source/docs readers only.

Go sim/entity/container.go91–116 recognizes furnace/chest/workbench, then sneakingHeld rejects InvalidInput, then Ready/slot validation. Rust containers already checks sneaking for active physical slots; its NoTarget reaches bench, whose arm currently ignores sneaking. Only two bench-arm callers: live step::admit_command and crafting::advance raw compatibility. This node accepts state eligibility alone, not missing wire refusal publication or physical-slot error precedence. No shared public interface change or independent contract landing needed.

## Algorithm, state and bounds

In existing pub(crate) settle_bench_open(ctx:&mut TickContext<'_>,envelope:&CommandEnvelope,look:LookAngles)->OpenOutcome, after hit.observed.block==WORKBENCH_BLOCK check current runtime controls actions.sneaking using is_some_and. If true return OpenOutcome::Refused before after/anchor/effects/record_crafting_publication_dirty. Missing controls remain neutral for existing isolated callers; source registered players have actual intake controls. Existing successful equal reopen/anchor relocation/lease ending semantics remain unchanged. Current command prefix owns held controls; at most one indexed runtime lookup, no scan/allocation/queue/I/O. Source NoTarget for other blocks remains unchanged because classification precedes guard.

Choose shared atomic arm over live-only check, late physics flag inference or forcing a container fallback reason: both real call paths need the same staged state predicate. Existing OpenOutcome still collapses wire reasons; explicit refusal-wire node will qualify that separately. No architecture/codec/version expansion.

## RED cases and steps

Before product, new native topic imports parent projection fixture and read-only command_lifecycle::scene. Actual stationary PlayerInput(primaryfalse/eatingfalse/sneakingtrue,lookPI) and OpenContainer(lookPI) in same tick:

1. Fresh personal grid stays exactly unchanged, anchorNone/viewerNone, no CraftingState/InventoryState for owner/foreign. Parent widens (RED). Next actual sneakingfalse input then open succeeds Workbench with source anchor(0,66,1) and grid publication (positive restart control).
2. Actual chest open already owns front reference; same-tick sneaking input plus bench open retains exact lease and complete inventory, anchorNone, no grid/owner inventory publication; parent clears lease/widens (RED). Full normal ChestState cadence remains accepted; do not incorrectly assert all events silent.
3. Actual bench open owns valid anchor and grid; sneaking input plus same-bench reopen preserves complete inventory and anchor, emits no new CraftingState intent; parent equal reopen restates grid (RED). PlayerState motion publication is not included in a silence assertion.
4. Existing crafting raw-call fixture opens real Ready bench, then stages held sneaking controls and a prepared container lease, admits another open. Raw compatibility drains both retained opens against current state: rejected2/applied0/carried0/examined3 including valid current bench; exact inventory/anchor/viewer unchanged. Parent clears lease (RED). This is raw-provider state proof with prepared held cause, not live command-prefix proof.

Run pinned offline locked cargo test --manifest-path packages/engine/Cargo.toml -p mornlea_server --test server_replay publication_projection::bench_eligibility --locked -- --nocapture and crafting::bench_sneaking before product, require four causal assertion REDs. Implement one guard and concise English ownership comment, update crate guide, cargo fmt1.97.1; all four+full replay and make rust-check. env -u CARGO_TARGET_DIR make rust precedes unchanged Go TestWorkbenchOpenSetsSizeThreeWithoutContainerRef/TestWorkbenchOpenRejectsNonWorkbenchTarget and existing container sneaking native control if discoverable, race/count1 (exact discovered entity TestSneakOpenContainerAuthoritativelyRejected/TestSneakControlSprintAcceleratesAndOpens plus the two named runtime controls). Four audits/strict128; freeze five source hashes, source/docs reviews and dev9/archive2/index protection before scoped fix(server): reject workbench opens while sneaking commit.

## Review, consumers and exclusions

Check target-classification precedence, current intake held flag rather than late physics state, no dirty intent on equal refused reopen, atomic lease/grid/anchor conservation, next eligible open positive, raw-call compatibility and complete container cadence. Cargo owns compilation; source/guide/plan audits consume edits, no generated/hashed derivative or new important directory. Root resolves observed full-gate overlaps before expanding exact scope.

Automatic workbench close invariant/hard error, physical slot+sneak reason precedence, all command refusal wire outcomes, actor snapshot/reset/reconnect, queued mirror/off-tick encoding, configured Agent/gameplay runtime/full supported outcome inventory and broad3.8/4.1/4.2 remain OPEN. Rollback is reviewed scoped inverse. Architecture skill:no change; canonical complete AOCI index/tools absent, no invented cognition. Full direct user authorization controls redundant skill gates; no Loom/Claude/new clone/worktree/push/deploy.

Fixture qualification: first-red fails to import TickPublication in the new descendant helper; qualify the existing contracts type. Preserve it as compile qualification, then run fresh genuine REDs before product. Crate-guide scope also reconciles its stale bench-anchor/intent description to already accepted live ownership, without changing the separate automatic-close failure policy.
