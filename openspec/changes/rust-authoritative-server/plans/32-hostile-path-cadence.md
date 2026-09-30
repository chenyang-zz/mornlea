# Hostile target identity and repath cadence

This packet refines node3.9q7 into a bounded cadence correction and a later Ready-revision acceptance. Controller owns decisions, integration/status and exact rollback. Baseline46b2eaca includes accepted checked geometry7fb4fbe6. Editable for the cadence node only src/rules/hostile_actors.rs and tests/server_replay/hostile_actors.rs beneath packages/engine/crates/mornlea_server. All core contracts/state, actions, storage and other replay files stay read-only. One isolated Sol implementer consumes existing interfaces; no new shared contract or schema change.

## Verified source and decisions

Read packages/server/server/hostile_manager.go dispatchSnapshots, advanceRunners and applyPathOutcome. The source checks persisted NextRepathTicks against WorldTime BEFORE selecting a new nearest UUID. Its comment promising immediate target change is broader than its actual due-first implementation. Follow actual implementation. A resolved/clamped PathState.target is a waypoint goal, never player identity. HostileMob.has_target/player_id own identity and next_repath_ticks owns durable cadence. Executing tick remains the separate shot-spread clock and is untouched.

1. Read now=view.world_time(). Dispatch only when now>=body.next_repath_ticks, regardless of path absence. A restored target with a future deadline and no transient path waits rather than treating absence as stale.
2. On due dispatch, call existing nearest_target (live Active same-dimension, stable UUID tie). Compare chosen UUID to durable has_target/player_id. Capture existing path.generation before clearing a changed target's path. A first/new identity increments generation once using checked_add; same identity retains it. No comparison of resolved path.target with raw player cell. On a failed refresh the absent transient path has no independently promised global generation history; do not add an unused persistent counter.
3. On due target loss clear only this hostile's target pair/path and set now+1. On due chosen target within walker attack range or hurler hold/retreat bands retain target facts and revisit next tick without a new path. Otherwise refresh using the current raw goal, preserving checked geometry and NativePathfind; success sets body/path deadline now+20, existing failure/defer sets now+1.
4. For movement range/retreat between dispatches, resolve the already-owned UUID from live Active same-dimension players. A newly nearer/different player cannot change target before the deadline. Same player's live position may change hold/retreat decisions without replanning. Existing path movement may continue when the owned target disappears before dispatch is due, as source advanceRunners does; do not clear identity early.
5. Exhausted path after consume_arrived must clear path/cursor and set durable now+1. Remove the unconditional final path-deadline copy which overwrites that retry. Successful refresh already writes both deadline fields. Preserve atomic batch publication, fluid/physics and sibling runtime behavior.

## Test-first acceptance

Use actual provider and existing ResidentTickState/runtime carry, not repeated context as cross-tick proof. Record old-source failures before implementation.

- Stationary target beyond the33-cell window and same UUID whose standing Y resolves to another floor retain exact path generation/resolved goal/success deadline across at least three real carries before due. Move its raw cell before due: no reset; at due recompute current goal.
- WorldTime1000 with executing tick0 produces success deadline1020, not20. Restore has_target/UUID with future deadline4096 and pathNone:4095 remains neutral/no path,4096 refreshes and sets4116. Correct existing legacy expected20 to1020 using verified source evidence.
- New nearer UUID and target disappearance before deadline preserve stored identity; due dispatch selects/clears respectively. Stable same UUID refresh does not bump generation; changed UUID with generation7 produces8 before any path clear.
- Arrived final waypoint while success deadline is still future clears path and keeps now+1 after physics. Failed/uncovered refresh retries now+1. Keep nearest live/dead/tie, hurler bands, ordinary geometry and all rollback cases green.

## Revision prerequisite and exclusions

Node3.9q7 remains open after cadence acceptance. The revision subnode requires an exact authoritative Ready-chunk revision query plus a correct live commit/materialization boundary. Current per-cell CAS revisions are not chunk revisions; max/arbitrary cell values cannot be used. Current Ready snapshot increments base once for dirty content but resident carry retains original base, so merely comparing base+dirty fails after a second later tick write. Controller must first qualify that boundary and then test changed/missing/nonReady covered chunks before accepting revision reuse. No unqualified fallback to sparse fixture observations or broad state edits are authorized in the cadence task. Production integration acceptance remains open.

## Gates and handoff

Source /workspace/.mornlea-env/env.sh, pinned Rust1.97.1 and own CARGO_TARGET_DIR. Run server_replay hostile_actors, hostile_actions and hostile_outcomes; require nonzero discovery, exact behavioral RED/GREEN. Two owned rustfmt files, server all-target clippy --locked -- -D warnings, diff check and source-comment policy. Enumerate derived consumers; no Go hashed source edits or corpus changes. Scoped fix(server): preserve hostile target and calendar repath cadence commit, exact full diff and evidence. Controller independently reviews and runs focused gates before marking cadence subnode; rollback exact two files only.
