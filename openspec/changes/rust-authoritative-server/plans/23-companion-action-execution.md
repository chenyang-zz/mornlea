# Companion action revalidation and shared-pipeline execution

Required execution skill: executing-plans. Status belongs only to tasks.md.
Goal: implement source-compatible companion action execution for node 2.9b
across the three accepted phases, reusing the frozen ingress, the shared 1.6
mutation transaction, and the accepted mining consumer. No dialogue, no
reducer wiring, no new shared surface. Oracle:
packages/server/sim/entity/companion_action.go, companion_placement.go,
mining.go, runtime/engine_step.go, tick.go, companion_manager.go;
slices plans/01-server-slices.md:63; refined node plans/04-refined-nodes.md:204,
215-223; order plans/03-parallel-readiness.md:72.

## Ownership and accepted prerequisites

S = packages/engine/crates/mornlea_server. Worker edits only:
- S/src/rules/companions.rs: the three phase handlers plus private
  selection/physics helpers.
- S/tests/server_replay/companions.rs: action execution regressions.
All other files read-only, including S/src/rules/mining.rs (accepted
consumer, verify-only), S/src/core/mutation.rs (accepted resolvers),
S/src/core/contracts.rs, S/src/core/state.rs, S/src/core/companion_ingress.rs
and guides. Main owns guides, artifacts, integration and rollback. Accepted
baselines: ingress 2.9a, transaction 1.6, placement 2.1b, mining 2.1c,
inventory 2.6a. No new phases, effects, dependencies, wire/save changes, Go
edits, fixture fallbacks, death/reset, text handling or world scans. No new
directory. Single-task private boundary: no separate contract landing; the
frozen ports below are the contract.

## Frozen interfaces

`pub fn run(ctx: &mut TickContext<'_>, call: RuleCall<'_>)
-> Result<PhaseReport, ServerError>` dispatches exactly:
- `RulePhase::CompanionIntent` with actor/command/internal all `None`:
  validate every envelope on `view.companion_actions()` (arrival order) and
  select the first valid action per companion ID. Invalid payloads, unknown
  or inactive companions, and same-ID later duplicates are ignored with zero
  effects and zero staged state; they count `rejected`, never error.
- `RulePhase::CompanionMotion` with all-`None`: step each companion with a
  selected Move through the accepted motion kernel path; companions without
  a Move keep neutral input retaining yaw.
- `RulePhase::CompanionPlacement` with all-`None`: settle selected Place
  intents in companion-ID byte order through the shared transaction.
Any other shape is `InvalidInput` field `"companion_call"`, before effects.
`PhaseReport`: examined is envelopes seen (Intent) or companions stepped /
proposals settled; applied is selections committed; rejected is ignored or
refused intents; carried 0. Selection is recomputed from the view on every
call; no cross-tick intent state lives in this provider.

## Source algorithm and decisions

Intent validation mirrors Go payload defense (`companion_action.go:12-47`):
Move components within [-1,1] and finite with finite yaw; target Y within
`[-64, 320)`; Place block non-air and registered. Liveness comes from the
read view: only `Active` `ActorKey::Companion` records are selectable.
Provenance (generation/attempt/digest/run/snapshot) stays ingress-owned at
admit time: no provider-visible gate accessor exists, so the provider never
re-reads gates. The combined regression proves the boundary instead: a
wrong-digest late candidate is refused at ingress, never reaches the view,
and the provider run is a no-op with `rejected` counting only what it sees.
First-valid-per-ID in arrival order falls out of a single ordered pass;
placement output is additionally sorted by companion-ID bytes, matching Go's
byte-sorted settle.

Motion mirrors the accepted player kernel path (`NativePhysics`, same body
constants and fixed-step discipline): Go companions share the player physics
exit (`TestCompanionActionSharesPlayerPhysicsExit`) and name no
companion-specific extents. Move maps to forward-gear yaw
`normalizeYaw(atan2(-x, -z))` with jump passthrough; anything else stages
neutral input retaining the actor's current yaw, following the player-motion
neutral convention. Inputs never persist across ticks.

Placement reuses `resolve_companion_place` plus `MutationTxn::try_place`:
proposed block on the placeable registry, observed-air target, debit from the
first matching stack of the companion's inventory. Every refusal (stale,
unobserved, consumed, missing item, competing write) stages nothing and
counts `rejected`. Competing human/companion claims resolve first-commit-wins
through ordinary revision advance; placement-before-interaction order is a 3.1
reducer constraint, not provider code. Never stage companion failure entries
anywhere: Go records no `result.Rejected` for companion failures.

Mining intents are owned end-to-end by the accepted `mining.rs`
`run_companion` (hold persists until release, ray must hit exactly the
target, full inventory keeps saturated progress, container batch all-or-none):
this provider stages nothing for mining and duplicates none of its registry
logic. Worker verifies release-clears read-only; any defect there escalates
as a 2.1c follow-up, never a silent expansion. Agent text cannot enter this
provider by type: envelopes carry no text lane.

## RED/GREEN steps and exact acceptance

1. Write replay tests against the missing provider before its body (plus the
   named `companions::neutral_hold_release_and_container_atomic` covering
   wrong-digest silence, full-container block preservation, and saturated
   progress retention through the view). Verify named RED output; zero
   filtered tests is not evidence.
2. Intent: first-valid-per-ID wins in arrival order; invalid payloads,
   unknown/inactive IDs and later duplicates ignored with counts and no
   effects. Motion: Move steers through the kernel; no-Move retains yaw with
   neutral input; inputs do not persist. Placement: ID-ordered settle,
   stale/missing/competing cases refuse with inventory and world unchanged.
3. Run pinned focused companions replay, mining replay consumer regression,
   ingress contract regression, full server, clippy all-targets -D warnings,
   fmt, diff; `make rust` before focused Go entity
   `CompanionAction|CompanionMining` (including container and placement
   files), runtime `Companion`, and lifecycle orchestration oracles (list
   actual matches). Independent review covers selection order, neutral-yaw
   retention, first-commit-wins, saturated-progress preservation, and the
   no-text invariant. Worker scoped English commit, main integrates, updates
   guide/tasks/ledger and reruns integration gates. Refusal/conflict returns
   evidence to main; no invented policy. Main owns rollback and revalidation
   of affected providers.

## Review focus

Arrival-order selection with byte-ordered placement output, neutral input
that retains rather than zeroes yaw, lossless full-inventory saturation, and
the ingress-owned provenance boundary are explicitly covered above. Queue
ordering and drain mechanics stay with the reducer/endpoint; phase order
follows the accepted `RulePhase` sequence into node 3.1.
