# Drop producer-to-pickup integration proofs

Required execution skill: executing-plans. Status belongs only to tasks.md.
Goal: close node 2.8b at provider level by proving staged producer drops
flow through aging into player pickup. Tests only unless a proof fails, in
which case stop and report. No reducer acceptance. Oracle:
plans/01-server-slices.md:57 (2.8b slice), plans/04-refined-nodes.md:201
(2.8b row), plans/09-drop-lifecycle.md (provider contract).

## Provenance ruling (controller)

A read-only scout mapped all 24 Go producers and all consumers to accepted
Rust counterparts; mob pickup is confirmed absent-by-design on both sides
(Go `pickUpDrop` iterates sessions only), so "death before passive pickup"
is phase order, not missing behavior. The sole remainder is production tick
wiring (no live caller of `drops::run`/`advance`, `core/step.rs` stubbed),
which belongs to node 3.1 per the slice execution order, not here. This node
proves provider-level integration: death-staged loot becomes eligible and
pickable through the accepted `advance` path in-fixture.

## Ownership

S = packages/engine/crates/mornlea_server. Worker edits only:
- S/tests/server_replay/drops.rs: producer-to-pickup sequencing proofs.
All other files read-only, including all providers, ports and guides. If any
proof fails against the providers, stop that proof with evidence and report
to main (possible provider defect for a main ruling); behavior fixes are
never snuck into this node. No new helpers, phases, effects, directories,
or Go oracle additions beyond already-accepted entity Drop/Pickup matches.

## Proofs (all in-fixture, death staging then DropStep advance)

1. Death loot visibility: hostile/player/passive death staging lands
   `Drops` effects that `advance` observes (ages, does not duplicate or
   drop); delay counts down per advance with no pickup while nonzero.
2. Pickup after delay: death/Q loot (delay 40) is untouchable at 39 and
   picked up at 40 with atomic inventory credit and drop removal/split;
   partial-stack remainder conservation holds.
3. Expiry precedence: age 5999 advances to 6000-expired before any pickup
   in the same step; expired records stage removal only.
4. Full-output preservation: panel/full-inventory pickup attempts leave the
   world output unchanged with conservation asserted.
5. Mining delay pin: assert which staged outputs carry delay 10 vs 40 from
   the providers as read-only evidence (worker cites the staging sites;
   invents no new delay).

Explicitly out of scope with owner pointers: production tick wiring and
active-interest derivation (node 3.1); save/restart of drop slots (node
3.7); subscriber-visible drop upserts (publication/3.1). The worker pins
none of these here.

## Validation and review depth

RED: each proof fails before its sequencing exists (unstaged loot is
invisible to advance; delay-nonzero blocks pickup — assert the negatives
explicitly). GREEN: all proofs pass. Run pinned focused drops replay, full
server, clippy all-targets -D warnings, fmt, diff. Review depth is
proportional (tests-only, zero source): main verification plus full gates,
no separate reviewer cycle. Worker scoped English commit, main verifies,
updates guide/tasks/ledger and reruns integration gates.

## Review focus

Delay-gated eligibility, expiry-before-pickup precedence, partial remainder
conservation, full-output preservation, and the documented exclusions above.
