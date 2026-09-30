# Production tick state: residents persist, logins seed actors

Required execution skill: executing-plans. Status belongs only to tasks.md.
Goal: give live ticks memory for node 3.7a: AuthorityState resident maps
with tick-start seeding and tick-end commit-back, plus login→actor initial
staging from saves with Go-sourced defaults. No save-format, transport, or
provider-behavior changes. This plan SUPERSEDES the original hydration
premise (per-tick re-derivation was never the architecture; the stopped
first attempt and its evidence are recorded in the ledger, not implemented).

## Provenance ruling (controller)

Two read-only scout rounds proved production ticks run empty overlays with
nothing to re-derive from: session bodies write-only, chunk results never
drained, no actor seeding, no despawn projection. The missing piece is
persistence across ticks plus initial seeding, not per-tick recomputation.
A prior worker correctly stopped the old premise rather than inventing
defaults; every default below is Go-cited, every structural choice follows
the accepted viewers/schedules/sleep commit pattern.

## Ownership

S = packages/engine/crates/mornlea_server. Worker edits only:
- S/src/core/state.rs: resident maps on `AuthorityState` mirroring the
  overlay (actors, runtimes, inventories, mining, projectiles, environment,
  blocks, ready, drops, containers, container_chunks) with init, tick-start
  seeding into the overlay, and tick-end commit-back (full clone-out /
  full replace, the `commit_viewers` pattern — no merge logic).
- S/src/core/step.rs: seed-then-dispatch-then-commit ordering in the
  reducer; login scan (below) before row 1.
- S/src/core/login_seed.rs (new, small): save→actor mapping ONLY, pure
  functions over save types (no state access).
- S/tests/server_replay/tick_state.rs (new replay topic) plus index
  registration following existing topic patterns.
All other files read-only, including saves/transport/providers/ports and
guides. Main owns guides, artifacts, integration and rollback. No new
phases, effects, wire/save changes, Go edits, or world scans. Single
bounded node; projection and streaming stay separate follow-ups.

## Frozen rules (no invented defaults)

Commit-back mirrors overlay maps 1:1 each tick; seeding clones them back
each tick start. Costs are bounded by existing caps (resident/actor caps,
chunk-result caps); a bounds test pins full-resident carry-over.

Login staging (Active sessions with a save body and no player actor):
`MotionState` position from save current, velocity zero, `on_ground`
false; look from restore yaw/pitch; health restore-or-20; hunger 20,
saturation 5000, exhaustion/starvation 0, oxygen 300 (restore hunger
triple overrides when present); crafting empty size Personal;
cooldowns/mining/eating/bow zeroed; dimension from save current;
lifecycle Active — every value cites Go `RegisterPlayer`
(`player.go:167-252`), `hunger.go:167-174`, `crafting.go:14-36`.
`InventoryRecord.slots[0..9]` = hotbar, `[9..36]` = backpack per the
`crafting.rs:72` unified-length rule; selected from save hotbar selection;
armor copied slot-wise. Mobs are NOT seeded from disk here (no in-memory
mob-save maps exist; disk restore is 3.8 startup scope); committed mob
actors persist uniformly once present. Chunk acquisition/drain stays as-is
(fixture/test-driven; production streaming is a 3.8 follow-up); Ready maps
persist like everything else. Dirty-snapshot marking for saves belongs to
3.7 proper, not here.

Out of scope with pointers: despawn/drop projection (follow-up blocks
3.8); save cadence (scheduler); agent install feed (endpoint);
TransportAuthority production impl (3.8); runtime chunk streaming (3.8).

## RED/GREEN and acceptance

RED: two consecutive live-shaped ticks lose everything (actors gone,
Ready gone, drops gone, environment reset); login session never becomes an
actor. GREEN: identical setup carries all maps tick-to-tick
(field-by-field asserts), login seeds exact Go-default actors, committed
mob actors persist, empty-tick stays empty. Run pinned focused tick_state
replay, full server, clippy all-targets -D warnings, fmt, diff.
Independent review covers mapping exactness against the Go table (every
non-obvious value cited), commit-back completeness (no map silently
dropped), and the documented exclusions. Worker scoped English commits
allowed (maps+commit, then seeding), main integrates, updates
guide/tasks/ledger and reruns integration gates. Refusal/conflict returns
evidence to main; no invented policy. Main owns rollback and revalidation.
