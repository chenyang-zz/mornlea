# Entity rule provider review

Baseline at start: 5ab842a6784d50425001ba28be4c6fe3f24a04da. Baseline at evidence completion: ec9b83f4a130d590afc91e2c574166d91ca3ef15.
Repository: /Users/chen/work/mornlea-f2-server. Read-only review; no repository file edits, staging or commits. Concurrent transport commits and eating edits were preserved. All reproductions are disposable files under /tmp/mornlea-f2-entity-probes and depend on the actual repository Rust crates by path. Rust compiler is the repository-pinned 1.97.1. No foreground app/window was launched.

## Scope and evidence limits

Read every line of the eight owned production files: rules/hostile_actors.rs, hostile_actions.rs, hostile_outcomes.rs, passives.rs, companions.rs, drops.rs, harvest.rs, mining.rs (8,187 lines including the hostile_outcomes internal test tail). Reviewed ancestor/scoped guides, openspec/config.yaml, active specification and planning context, and packets21/23/24/14 governing action, companion, death and mining decisions. Current Go oracle reading focused on the cited algorithms in sim/entity/hostile.go, hostile_action.go, passive.go, passive_graze.go, passive_spawn.go, companion_action.go, mining.go, death.go, drop.go, server/hostile_manager.go, shared/physics/submersion.go and updates/sampler.go. This is a local provider review, not renewed executable, wire projection, persistence assembly, Agent or deployment acceptance.

Matching replay modules were inspected selectively for helpers, assertions and corresponding branch coverage; the entire test suite was executed, but this does not mean every assertion was independently source-qualified. The disposable crate copies matching test helper modules solely to construct real AuthorityState/TickContext inputs and calls real production providers; production algorithms are never copied into a fake provider. The few Go scalar reference expressions in new assertions are cited below. Earlier raw rustc linking attempts failed due to toolchain/dependency identity mismatch; these are compile failures, not RED evidence. Cargo path-dependency builds resolved that and produced the behavior evidence below.

## Behavior RED command and artifacts

From packages/engine:

```
cargo test --manifest-path /tmp/mornlea-f2-entity-probes/Cargo.toml review_ -- --nocapture
```

Final result: exit101; **11 tests compiled and executed, all11 behavior failures**. Log: /tmp/mornlea-f2-entity-probes/red.log. Ten probes establish nine implementation defects; the remaining probe establishes an explicit packet/oracle conflict. These are not compile-only failures. Temp harness main.rs imports contracts and state and the five copied helper modules. No repository tests were altered.

## Confirmed implementation defects

### E1 — P1: daylight burn never advances authoritative hostile health

Locations: src/rules/hostile_actors.rs:532 decrements only entry.body.health; :379 writes the original self.survival into the staged ActorRecord. src/rules/hostile_outcomes.rs:901 selects deaths using actor.survival.health(). Combat freeze/settlement also consumes survival rather than only save-body health.

Oracle: Go sim/entity/hostile.go:365-369 decrements the one authoritative entry.health and same-tick settleHostileDeaths consumes it. No approved policy admits diverging health lanes.

Executed REDs:
- hostile_actors::review_burn_updates_authoritative_health: body starts20 and burn_cooldown1, air sky observed through y319. One burn call makes body19 but survival remains20; assertion expected survival19.
- hostile_actors::review_lethal_burn_reaches_death_settlement: same scene381 exposed calls reaches body0, then actual HostilePlayerDeaths. Actor remains Active, expected Dead. No Ready loot chunk is needed to establish removal because the accepted death contract permits all-full loot omission.

Minimum acceptance: nonlethal burn updates both authoritative and durable health; lethal burn followed by actual death provider produces one death and source loot/removal semantics, with no subsequent combat health restoration. Existing fresh_skip_neutral_move_and_burn only checks body.health (:900), so it passes while authoritative health is wrong.

### E2 — P1: independent player slots claim the same unreduced drop capacity

Locations: hostile_outcomes.rs:1040-1069 plans every inventory/armor slot against the same immutable view; :1228 checks only that original view and appends a future DropBatch. Final compound at:1133 refuses if accumulated batches exceed the physical32-slot chunk capacity.

Oracle: Go death.go:166-209 immediately commits each accepted slot batch before rehearsing the next slot. Approved packet24 requires per-slot independence and ring fallback, preserving unplaceable items through respawn.

Executed RED: hostile_outcomes::review_player_death_rehearses_cumulative_capacity. Ready death chunk has31 nonmergeable existing slots; adjacent Ready chunk is empty; dead player has wheat and dirt in two slots. Each planned output independently sees the same one free death-chunk slot, the final compound rejects, report.applied=0 (expected1), actor does not respawn. This repeats on retry although there is ample neighbor capacity.

Minimum acceptance: cumulative per-slot rehearsal includes previous successful outputs, follows ring order, and respawns once; two stacks use last local slot then neighbor. If no neighbor exists, the first fitting slot drops and the other stays in inventory while respawn still succeeds. Armor durability and existing all-or-none per-slot split rules stay intact.

### E3 — P2: companion mining ignores first-valid action selection

Locations: mining.rs:358-379 explicitly uses latest hold/release proposal; companions.rs:192-213 selects first valid action per ID in arrival order. Thus motion/placement/intent and mining can consume different actions from the same queue.

Oracle: Go companion_action.go:73-82 selects one first-valid action across all action kinds; :98-105 updates miningHeld only for that chosen action. Approved packet23 freezes first-valid-per-ID and names mining as end-to-end hold owner.

Executed RED: mining::review_duplicate_release_does_not_override_first_hold. Actual view has MineHold(DIRT) then same-ID MineRelease. Actual mining progress is absent, expected progress1 from the first valid hold. The hold scene has observed ray cells and a real active companion/inventory.

Minimum acceptance: valid Hold then Release retains hold; Release then Hold remains released; first valid Move/Place followed by Hold cannot simultaneously mine; invalid action preceding valid Hold does not consume selection. All consumers must observe the same selection semantics.

### E4 — P2: passive wander uses a20-bit denominator for a24-bit hash

Locations: passives.rs:821 divides by1,048,576 although it masks0xFFFFFF. Matching replay helper tests/server_replay/passives.rs:401 repeats the same mistake.

Oracle: Go passive.go:378 uses 2π/0x1000000, i.e.16,777,216. Go passive_wander_test.go:25 independently computes that expression. This changes heading at ordinary coordinates, not just float roundoff.

Executed RED: passives::review_wander_uses_full_24bit_quantum. Seed0,tick0,id41, cow starts at the exact Go heading1.4673759. Actual provider changes yaw to1.2673758, although target heading already matches. Room, runtime and cow are real provider inputs.

Minimum acceptance: use the source24-bit angular quantum, independently grounded expected yaw, and source bounded-turn behavior across at least two segments. Correct the test mirror too; do not derive expected values from the faulty Rust expression.

### E5 — P2: hostile fluid AABB omits upper intersecting cells

Locations: hostile_actors.rs:1979-1980 fluid_upper uses floor(max)-1, while submersion_body :1960-1962 scans through that bound. Companions and passives correctly use checked ceil(max)-1.

Oracle: Go shared/physics/submersion.go:89-94 uses checked ceil(max)-1. Body maxY2.8 intersects y2 even though feet are y1.

Executed RED: hostile_actors::review_hostile_torso_fluid_matches_body_fluid_step. Identical airborne hostile[2.5,1,2.5] and observed air room, water only at torso cell[2,2,2] versus water at torso+feet. Both bodies intersect water and should apply same fluid gravity. Actual torso-only vy=-1.6; torso+feet vy=-0.32000002.

Minimum acceptance: ceil upper bounds preserve exact touching-boundary exclusion and detect upper/side partial intersections; real physics step matches submerged controls. Checked arithmetic must also cover extreme endpoints.

### E6 — P2: dead nearest player masks the next live hostile target

Locations: hostile_actors.rs:1225 active_players filters lifecycle only; hostile_actions.rs:229-241 first selects nearest then discards it for health0 without selecting the next candidate. Motion also consumes nearest_target directly.

Oracle: Go server/hostile_manager.go:754-761 filters zero-health players before nearestTarget. Approved packet21 calls for live targets.

Executed RED: hostile_actions::review_dead_nearest_does_not_mask_alive_target. Active dead player x1.0, Active live player x1.5, walker x0.5. Both are within1.8. Actual melee batch is empty, expected one entry targeting living session.

Minimum acceptance: liveness filtering before distance/tie comparison; dead nearest cannot suppress melee, shot or chase of a farther live same-dimension player. Preserve sorted-session spawn anchor semantics separately: Go sortedActiveSessions intentionally only filters lifecycle, so globally changing the helper used for spawn/distant would broaden behavior unless controller freezes that change.

### E7 — P2: clamped path goal is compared to raw target, forcing every-tick replan

Locations: hostile_actors.rs:880-885 compares path.target with raw player block; refresh_path :1123 stores the clamped/standable goal instead. Out-of-window goals or standable-Y adjustments therefore invalidate a still-valid path immediately.

Oracle: Go hostile_manager dispatch/outcomes retain player identity and use the20-tick cadence after success. It does not compare its clamped goal against the raw moving target every tick.

Executed RED: hostile_actors::review_far_target_preserves_repath_cadence. Flat observed33×9×33 window centered100, stationary player x130.5. First actual movement stores next_repath_tick20. Carry actual actor/runtime records into a context at tick1, same Ready observations/target, run actual movement again: next_repath_tick becomes21, expected20. Initial generation/cursor-only same-tick probes passed and were not sufficient evidence; final cross-tick deadline assertion fails.

Minimum acceptance: a stationary target beyond±16 or with adjusted standableY does not reset the successful deadline before due; target identity changes, Ready revision changes and path exhaustion still invalidate according to source rules. Preserve durable world-time deadline semantics separately; current ctx.tick versus Go WorldTime is another clock-qualification concern below.

### E8 — P2: passive birth neighborhood is measured in blocks instead of chunks

Locations: passives.rs:1585-1589 computes floor(position)-home and tests distance>1; :273 and :389 store home as birth BlockPos. Rollback at:711 then stops the cow within roughly two blocks of its birth location.

Oracle: Go passive.go:584 converts position to ChunkPos and compares to birth ChunkPos; permitted neighborhood is3×3 chunks. Existing restoration/source stores home=blockPosOf(position).Chunk().

Executed RED: passives::review_home_neighborhood_is_chunk_radius. Birth home block[0,1,0]; cow atx1.99, yaw−π/2, real observed flat room, freshfalse, real provider run. Its next positive-X step enters block2 but remains in birth chunk0. Actual pose rolls back, expected x>1.99.

Minimum acceptance: walking across block2 and any legal position within birth chunk±onechunk succeeds subject to physics; crossing into a chunk with Chebyshev distance2 rolls back; negative coordinates use floor/shift semantics and large deltas are computed without overflow. Keep the existing private home representation only if both comparisons correctly project to chunks.

### E9 — P1: accepted finite hostile coordinate can panic the authoritative motion provider

Location: hostile_actors.rs:1338 computes center.x−16 unchecked (z same at:1340; additional arithmetic needs review). ActorRecord accepts the record and hostile storage validates finite X/Z without shrinking the signed cell domain (mornlea_storage/src/hostile.rs:287-290).

Executed RED: hostile_actors::review_extreme_valid_actor_cannot_panic_path_window. Real active hostile at[i32::MIN as f32,40,0.5], living target[0.5,40,0.5], valid finite records, day environment and peaceful difficulty prevent spawn. Actual debug-profile HostileMotion panics at1338, “attempt to subtract with overflow”, before any world window is needed. Release-profile unchecked arithmetic may wrap instead; release behavior was not executed, so this is not a verified release-profile panic claim.

Minimum acceptance: boundary accepted records cause deterministic error/defer/validated source result, never panic or silently map to another cell. Controller must freeze overflow policy; this review does not choose wrapping, checked refusal or world-bound restrictions. Private helper floor_to_i32 :2017-2023 also maps out-of-range finite positions to0 despite its refusal comment; this is source-inspected and not separately executed in these probes.

## Explicit packet/oracle conflict, not worker miss

### C1 — hostile shot spread clock

Rust hostile_actions.rs:387 passes environment.world_time into shard_velocity. Packet21 explicitly freezes “seed, world_time (not tick)”. Current Go sim/entity/hostile_action.go:134 instead passes engine.tick.Load(), and sampler.HostileShotSpread hashes that argument. The canonical supported-outcome parity specification requires checkpoint agreement.

Executed discriminator: hostile_actions::review_shot_spread_uses_executing_tick. Worldtime1000, executingtick0, id11, seed7, clear eye corridor. Actual velocity=[21.953844,−0.92104274,−1.0864613]; Go tick-key expression=[21.960176,−0.5814505,−1.1885362]. This proves source drift, but implementation currently obeys the explicitly frozen packet. Main must reconcile the source-versus-approved-plan conflict before dispatching a repair; no local policy was invented.

## Source-only concerns / unqualified branches

- Companion MineHold lifetime: mining.rs:384-385 uses the prior MiningProgress.target as the only carried hold. Invalid/unready ray clears that record at:405; successful completion also clears it. No separate miningHeld/miningTarget lane exists in this provider's visible carried state. Go companion_action.go:98-105 and mining.go:350-389 retain miningHeld/target separately while invalid/unready input clears only progress. Consequently a Hold that first sees an unready ray, or succeeds and later sees a replacement block, appears to lose the source's until-Release intent. Source-only inference here: no additional actual cross-tick hold/recovery probe was executed. Shared-state repair requires controller design, not a private worker guess.
- Hostile path time/identity: advance_movement :849 uses ctx.read().tick for next_repath_ticks; Go manager :252/:293/:378 uses WorldTime for durable deadlines. Also stale comparison does not test target PlayerID, so a newly selected player in the same target block may preserve old durable target facts. Source-only inference; no separate identity or restored-clock RED executed.
- Checked-coordinate breadth: hostile spawn/light/prism/path and passive spawn/dry probes use additional unchecked signed arithmetic, and passive outside_home originally subtracts before widening. The executed panic qualifies one path-window branch, not every boundary. Native physics sweep arithmetic and huge finite velocity refusal were read but not exhaustively differential-tested.
- Source bounds versus provider qualification: spawn64/32 caps, hostile melee64, combat snapshot104/intents72, fluid grids4096 cells and item drop32 slots were inspected. Existing focused tests establish selected edges, not all legal combination ceilings. The Go hostile manager's per-tick2 snapshot/inflight2 worker policy is not accepted by merely executing the local synchronous Rust path helper; global worker/executable assembly remains outside this review.
- Reviewed without new confirmed local defect: companion first-valid motion/placement, normalized neutral yaw, inventory debit and common mutation refusal; human mining ray/progression/suppression and completion delegation; drop counter aging/wrapping, key dedup/order, partial pickup and crafting recovery preflight; harvest salts/hash order, signed-coordinate zero extension and tick-stable grass/leaf rolls; hostile spawn seasonal/light/kind/local-cap gates; passive priority/graze/tempt/fixed-loot flow; combat freeze/tie/reach/armor/knockback/death gate. These remain read/source plus existing-test evidence, not full Go differential qualification.
- Structural mining/container/door/bed/harvest output implementation lives in shared mutation.rs, not an owned file. This review read delegation edges but does not reaccept the entire shared resolver implementation. Real projection, terminal resident cleanup, durable save selection/restoration and Agent integration findings recorded elsewhere are excluded.

## Existing focused gates

All commands ran from packages/engine with CARGO_TARGET_DIR=target/cargo and pinned toolchain:

```
cargo test -p mornlea_server --test server_replay --locked <module>::
```

- hostile_actors: test result: ok. 10 passed; 0 failed; 0 ignored; 0 measured; 318 filtered out; finished in 0.21s; log /tmp/mornlea-f2-entity-hostile_actors-gate.log
- hostile_actions: test result: ok. 19 passed; 0 failed; 0 ignored; 0 measured; 309 filtered out; finished in 0.00s; log /tmp/mornlea-f2-entity-hostile_actions-gate.log
- hostile_outcomes: test result: ok. 59 passed; 0 failed; 0 ignored; 0 measured; 269 filtered out; finished in 0.02s; log /tmp/mornlea-f2-entity-hostile_outcomes-gate.log
- passives: test result: ok. 5 passed; 0 failed; 0 ignored; 0 measured; 323 filtered out; finished in 0.01s; log /tmp/mornlea-f2-entity-passives-gate.log
- companions: test result: ok. 6 passed; 0 failed; 0 ignored; 0 measured; 322 filtered out; finished in 0.01s; log /tmp/mornlea-f2-entity-companions-gate.log
- drops: test result: ok. 26 passed; 0 failed; 0 ignored; 0 measured; 302 filtered out; finished in 0.05s; log /tmp/mornlea-f2-entity-drops-gate.log
- mining: test result: ok. 10 passed; 0 failed; 0 ignored; 0 measured; 318 filtered out; finished in 0.00s; log /tmp/mornlea-f2-entity-mining-gate.log

Total scoped existing tests:10+19+59+5+6+26+10=135, all pass. Full existing server_replay had327 passing and1 failure because explicit MORNLEA_AGENT_PYTHON fixture variable was absent (full_corpus::real_agent_candidate_admitted_and_stale_refused), exit101; log /tmp/mornlea-f2-entity-existing-replay.log. The environment-dependent Agent case is not counted as an entity defect or passed acceptance. No fresh Go executable differential run or make rust was performed during this read-only review. No use of previous /tmp Go binaries as trusted live oracle evidence.

## Production file identities at completion

- /Users/chen/work/mornlea-f2-server/packages/engine/crates/mornlea_server/src/rules/hostile_actors.rs: SHA256 92e9ef49b8b4a19e21dc07be48015b6f999712584baee69f89602bbe0189fb60
- /Users/chen/work/mornlea-f2-server/packages/engine/crates/mornlea_server/src/rules/hostile_actions.rs: SHA256 e8a620fee26a7fc26e9275c7249f01075f10c550287b00269a06c5b07de696f1
- /Users/chen/work/mornlea-f2-server/packages/engine/crates/mornlea_server/src/rules/hostile_outcomes.rs: SHA256 3eb63d120c52b809be39f4b70134d73d3c50c3ebfc186849d96478271b20bb2d
- /Users/chen/work/mornlea-f2-server/packages/engine/crates/mornlea_server/src/rules/passives.rs: SHA256 be91a35d3066114cc17de8d66b70700bef5b00316bfdf4c4125dd457ec8d1c1a
- /Users/chen/work/mornlea-f2-server/packages/engine/crates/mornlea_server/src/rules/companions.rs: SHA256 6c1cd6896acf0c1ed630aa973f89d640001e2b36fb63e41b607faf68cde4cc07
- /Users/chen/work/mornlea-f2-server/packages/engine/crates/mornlea_server/src/rules/drops.rs: SHA256 12b268a96d09529ac5baa9d693c62dce4eff7f162db30a17e0ef64bdc4a9bcab
- /Users/chen/work/mornlea-f2-server/packages/engine/crates/mornlea_server/src/rules/harvest.rs: SHA256 b799d44b098f1db4c66aae30a410da8259f38bd313576cf33a2fdb50fa90b48d
- /Users/chen/work/mornlea-f2-server/packages/engine/crates/mornlea_server/src/rules/mining.rs: SHA256 cbda1986a98fbc1a28d3960e0b0110a9ad967639d89433778927fe77a03c129b
