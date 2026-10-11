# One-time hostile death and combat outcome integration

Required execution skill: executing-plans. Status belongs only to tasks.md.
Goal: prove same-tick Combat-to-death integration for node 2.7c by driving
the accepted melee (2.7c1) and death (2.7c2) providers in sequence. Tests
only unless a proof fails, in which case stop and report. No reducer
acceptance. Oracle: plans/01-server-slices.md:53 (2.7c slice),
plans/04-refined-nodes.md:199 (combat/death row), :218 row 5 (phase order).

## Ownership

S = packages/engine/crates/mornlea_server. Worker edits only:
- S/tests/server_replay/hostile_outcomes.rs: same-tick sequencing proofs.
All other files read-only, including both providers, drops/survival/passives
consumers, phases, ports and guides. If any proof fails against the
providers, the worker stops that proof with evidence and reports to main
(possible provider defect for a main ruling); behavior fixes are never
snuck into this node. No new phases, effects, helpers, directories, or Go
oracle additions beyond the already-accepted entity Combat/HostileDeath
matches (worker lists which ones the proofs exercise).

## Proofs (all in-fixture, Combat then HostilePlayerDeaths same tick)

1. Lethal hit settles once: a killing intent settles death with loot and
   `Dead`; re-running the death phase settles nothing further (no second
   loot, no duplicate removal, counts prove it).
2. Simultaneous hits: hostile-plus-player mutual lethal intents settle both
   deaths in one death phase with each side's loot staged.
3. Old-ID late hit: a batch naming a removed actor, a stale tick, or a
   reordered identity refuses with zero effects on loot, health, or events.
4. Ordering: the death phase visibly follows the combat phase in-fixture
   (kills land only after combat staged them), and passive advancement is
   untouched by this node.
5. Hit-event order: attacker-only `CombatHit` events precede death staging
   with no death/despawn events fabricated by the providers (matches Go,
   which signals subscribers separately).

Explicitly out of scope with owner pointers: death-drop pickup by passives
or players (node 2.8b owns producer-to-pickup integration); save/restart
parity of dead records and drops (node 3.7 owns real save/restart proof);
subscriber-visible despawn publication (publication diffing owns it; 3.1
composes the order). The worker pins none of these behaviors here.

## Validation and review depth

RED: each proof fails before its sequencing exists (e.g., death phase
driven without prior combat stages nothing; assert the negative
explicitly). GREEN: all proofs pass. Run pinned focused
hostile_outcomes replay, full server, clippy all-targets -D warnings, fmt,
diff. Review depth is proportional: tests-only node with zero source
changes gets main verification plus full gates, recorded here, instead of a
separate reviewer cycle. Worker scoped English commit, main verifies,
updates guide/tasks/ledger and reruns integration gates.

## Review focus

One-time settlement under re-runs, mutual-lethal completeness, stale-batch
silence, and the documented exclusions above.
