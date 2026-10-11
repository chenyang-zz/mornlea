# Read-only player-provider algorithm review

Repository: /Users/chen/work/mornlea-f2-server. Inspected providers at fcd9c26e622bfe0b45f7dd6e3337b7634c4723b4; final repeated probes ran at f01b81c7b67bfa4e22fc0c366cb4cc18a26c35df. `git diff fcd9c26e HEAD --` the four owned providers and Go entity source is empty. No repository files were edited. Concurrent controller/worker changes were preserved. This supplements /tmp/mornlea-f2-acceptance-review.md and does not re-count its executable, transport, projection, persistence assembly or activation findings.

I read all production code in src/rules/player_motion.rs, player_survival.rs, eating.rs and projectiles.rs, their Rust replay modules, the source-cited Go methods and relevant physics helpers. Below paths prefixed `Rust/` resolve to packages/engine/crates/mornlea_server/; `Go/` paths are repository-relative. Go expected outcomes below are derived from current source and relevant Go tests. New probes execute real Rust providers through TickContext, not a process or a mock provider. I did not run a newly instrumented Go executable for these cases.

## Executed evidence

Disposable test crate: /tmp/mornlea-f2-player-review-probe. Its dependencies point at the real checkout crates. Copied existing replay modules preserve their original tests; appended `review_*` probes are outside the repository. Four providers' existing tests all PASS: eating4, motion6, projectiles24, survival8 =42. Twelve new provider probes all FAIL: motion4, survival6, projectiles2. These are isolated provider regressions and source-exact parity discrepancies; they do not prove server-process acceptance.

Commands (cwd repository, Rust1.97.1):

```
CARGO_TARGET_DIR=/Users/chen/work/mornlea-f2-server/packages/engine/target/cargo rustup run 1.97.1 cargo test --manifest-path /tmp/mornlea-f2-player-review-probe/Cargo.toml --offline --tests -- --skip review_
CARGO_TARGET_DIR=/Users/chen/work/mornlea-f2-server/packages/engine/target/cargo rustup run 1.97.1 cargo test --manifest-path /tmp/mornlea-f2-player-review-probe/Cargo.toml --offline --test motion review_ -- --nocapture
CARGO_TARGET_DIR=/Users/chen/work/mornlea-f2-server/packages/engine/target/cargo rustup run 1.97.1 cargo test --manifest-path /tmp/mornlea-f2-player-review-probe/Cargo.toml --offline --test survival review_ -- --nocapture
CARGO_TARGET_DIR=/Users/chen/work/mornlea-f2-server/packages/engine/target/cargo rustup run 1.97.1 cargo test --manifest-path /tmp/mornlea-f2-player-review-probe/Cargo.toml --offline --test projectiles review_ -- --nocapture
```

Logs: /tmp/mornlea-f2-player-provider-existing-green.log (42 PASS), /tmp/mornlea-f2-player-provider-red.log (motion4 FAIL; the --tests command stops after that executable), /tmp/mornlea-f2-player-provider-survival-red.log (survival6 FAIL), /tmp/mornlea-f2-player-provider-projectile-red.log (projectiles2 FAIL). Probe compilation initially needed the exact CommandEnvelopeParts tick/arrival_index fields; this was corrected in the disposable test before behavioral RED. A preliminary snow assertion against speed5 was under-specified; the final RED uses actual source-default4.3 and snow factor0.7.

## New actionable provider-local findings

### 1. P1: minimum int32 coordinate can panic or cause an effectively unbounded fluid scan

Rust/src/rules/player_survival.rs1048–1049 and player_motion.rs479–480 compute `checked_ceil(maximum)? - 1` without checking subtraction. At x=-2147483648f32, adding/subtracting half-width0.3 rounds back to x; checked_ceil legitimately returns i32::MIN. Debug panics on subtraction; release without overflow checks wraps to i32::MAX and scans a range of4,294,967,296 x cells unless it encounters fluid first. The workspace release profile specifies unwind but no overflow-check override. Go shared/physics/submersion.go109 has the same wrapping arithmetic, so this is a boundedness/correctness defect rather than an approved parity divergence to emulate. Rust provider docs promise checked coordinate refusal, not panic/unbounded work.

RED `review_minimum_coordinate_refuses_without_panic`: stage an Active actor at [-2147483648,10,.5], runtime peak10, no fluids, call PostPhysics under catch_unwind. Actual panic at survival.rs1049. The current save schema's validate_location only checks finite components (mornlea_storage/src/player.rs828–844), and FiniteVec3 also allows this value. This does NOT prove the present login/restore path delivers such a save to an Active actor; it establishes a legal admitted provider input and a missing safety boundary. Controller owns precise refusal/clamp policy.

### 2. P2: sprint gating is bypassed, loses held intent, and uses post-charge hunger

Rust survival.rs726–727 stages hunger/sneak suppression, but motion.rs259–278 reselects the raw latest deferred packet and passes its sprint bit to NativePhysics at322. No hunger condition exists in that motion leg. With a fresh forward sprint packet at hunger5 and initial z velocity-5, real provider sequence Intake→PrePhysicsOxygen→Motion produces z=-5.59 (4.3×1.3) although the oxygen phase's runtime sprint bit was false. Go player.go592–604 copies held input locally and clears sprint before physics. RED `review_fresh_hungry_sprint_packet_is_gated_before_physics`.

Also survival.rs726 and826–827 replace stored held controls with suppressed sprint=false. Go suppresses a local copy of player.input, preserving intent. Thus hunger5→eat to≥6 without a new packet cannot resume held sprint in Rust. RED `review_sprint_intent_survives_hunger_gate` shows the intent bit is irreversibly cleared by Oxygen; this is a state-level reproduction, not a full eat-then-motion process test.

Third, survival.rs802–812 charges jump first, then rechecks hunger against the mutated work before sprint. Go player.go642–656 charges jump then sprint using the same pre-physics input, with no post-charge gate. RED `review_jump_charge_cannot_cancel_same_step_sprint_charge`: pre-step grounded, post-step airborne, forward+jump+sprint, hunger6, saturation0, exhaustion3970, threshold4000. Jump50 drops hunger to5, remainder20. Rust skips sprint80 and persists20; Go persists100. This is a saved exhaustion difference. Existing sprint replay tests only inspect PostPhysics suppression with uncomplicated counters and miss both producer/consumer and crossing-threshold cases.

### 3. P2: wet landings settle fall damage before canceling the peak

Rust survival.rs816–825 applies damage before its body-fluid peak reset. Go player.go660–667 resets peak to landingY when the landed body is in fluid BEFORE applyFallDamage. This is specifically covered by Go runtime/player_fluid_fall_test.go23 `TestFallIntoFluidCancelsFallDamage`, including a dry-start/wet-grounded-end guard.

RED `review_wet_landing_preserves_health`: actor grounded at [.5,10,.5], peak14, health20, water at(0,10,0), PostPhysics. Rust health19 vs expected20, then resets peak10 too late. Existing Rust fall_curve_3_4 tests dry simple heights; oxygen_fall_respawn_and_correction exercises oxygen/fatal fall but not wet landing cancellation. This requires a provider algorithm fix, not merely wiring a missing phase.

### 4. P2: survival damage does not interrupt bow draw

Rust survival.rs345–365 explicitly leaves bow untouched; work omits the bow lane and write_back at521–531 preserves runtime.bow. Death settlement603–612 also retains it. Go player.go702–714 clears both eating and bow on every positive damage. Projectiles already correctly clear both at projectiles.rs277–279, demonstrating an inconsistent damage contract between existing providers.

RED `review_starvation_interrupts_bow`: hunger0, health20, starvation79, drawn bow20; RegenStarvation deals1 but leaves BowProgress20. Expected None with ammunition/durability unchanged. Drowning/fall use the same broken helper. Relevant Go test: bow_test.go234 interruption matrix, damage subcase near307; death clearing is bow_test.go372. Existing Rust survival harness initializes bow=None throughout its source rows, so its damage tests cannot detect this omission.

### 5. P2: rejected input can become bow release and fire an arrow

Rust motion.rs187–194 defers invalid input but does not clear bow. Motion clears held control at259–264 and preserves other runtime lanes at368–371. BowDraw then treats no primary bit as release at projectiles.rs687–695. Go tick.go103–112 explicitly clears bow on invalid input precisely to avoid a rejected input becoming release; Go bow_test.go687 covers this.

RED `review_invalid_player_input_cannot_fire_existing_bow`: stage Active bow holder, runtime controls.primary=true, bow progress20, arrow inventory. Intake invalid move_x2/primaryfalse returns InvalidInput; Motion processes the tombstone; BowDraw then spawns an arrow/debits ammunition. Expected no projectile and zero progress. This is a direct real-provider chain; an eventual correct phase schedule by itself does not restore the missing invalid-input interruption. Depending on assembly order the accidental release can be delayed to the next tick; no real process timing claim was made.

### 6. P2: reset-marked Active actors still run motion

Rust motion.rs256–335 initializes/clones runtime but never checks reset; Go player.go576–578 skips physics for reset actors after pre-motion survival/eating/bow. RED `review_reset_actor_does_not_advance`: Active grounded actor velocity[1,0,0], runtime.reset=true, Motion changes velocity to[0,0,0]. Expected exact motion preservation. Existing motion_preserves_sibling_runtime sets reset=true and asserts only runtime identity, so it actively misses this actor-state change.

Related unqualified branches remain: Go player.go579–588 guards below-MinY-16, runs tryUnstick, and requests reset; these providers do not implement those branches. I did not execute new specific below-world/unstick probes. This is local missing lifecycle behavior and should be tracked separately from prior generic process assembly incompleteness.

### 7. P2: sneak-edge intent clamp is absent

Rust motion.rs266–331 forwards sneak and movement to NativePhysics; no edge support sampling exists. Its module comments53–56 defer this behavior; survival comments696–697/879 incorrectly imply accepted native kernels own it. Go player.go605–610 calls shared/physics/sneak_edge.go16–43 BEFORE sweep/kernel. Native engine step.rs301–311 only changes speed, and collision does not invent this input clamp.

RED `review_sneak_edge_suppresses_intent`: loaded air world with one grass support(0,0,0), grounded position[.8,1,.5], velocity0, yaw0, move_x1+sneak and no jump. Both forward footprints are over loaded unsupported x1, so Go clamps movement to0. Rust moves x=.8→.86450005. Relevant Go tests: shared/physics/sneak_edge_test.go53/65/77 (cliff, zero intent, unloaded fallback). Existing replay tests have no sneak cliff.

### 8. P2: thick-snow slowdown is omitted from the mirrored physics entry

Go shared/physics/step.go48 first applies applySnowLayerSlowdown; snow_slowdown.go56–76 samples grounded foot cell when movement is nonzero, scaling this step's WalkSpeed by0.7 for snow tier3/4. Rust motion.rs291–305 uses original tuning and never samples snow for this purpose. Zero-collision snow mapping is correct but insufficient. NativePhysics only consumes tuning and cannot know raw snow identity from zero-box cells.

RED `review_deep_snow_scales_walk_speed`: source-default walk4.3, grounded [.5,1,.5], velocity[4.3,0,0], move_x1, foot block87 (SnowLayer3). Rust keeps velocity4.3; expected3.01 (4.3×0.7). Relevant Go tests: shared/physics/snow_slowdown_test.go62/89/113/136. No thick-snow motion case exists in Rust replay. This finding concerns slowdown, not the separate already-deferred snow-footprint write collection.

## Exact parity discrepancies requiring controller rulings

### 9. Fall subtraction promotes to f64 before the source f32 rounding

Rust survival.rs817 uses `f64(peak)-f64(land)`. Go player.go691 uses `float64(peakY - Position.Y())`, where subtraction rounds as f32 first. RED `review_fall_preserves_go_f32_subtraction`: peak4.1f32, grounded landing.1f32. Go f32 difference rounds4, damage1; Rust f64 difference≈3.999999903 floors3, damage0. Rust health20 vs expected19. Both positions are valid provider inputs. I did not prove landingY.1 is reachable on the current full-block/bed/farmland collision geometry; therefore this is an exact replay/parity discrepancy, not a demonstrated ordinary terrain gameplay bug. Larger/malformed finite peak differences also need checked damage integer arithmetic: finite-only peak validation at765 does not ensure `(as i32)-3` cannot overflow, but I did not append a separate extreme-peak probe.

### 10. Long-held bow changes source u16 overflow behavior

Rust projectiles.rs669 uses saturating_add; Go bow.go168 uses uint16++. After65535 held ticks (~54.6 minutes at20Hz), the source wraps progress0, whereas Rust remains65535 and can fire a full arrow on release. RED `review_bow_progress_matches_go_u16_wrap` seeds max progress and advances one hold: Rust65535 vs Go0. Existing tests cover5/6/19/20, no wrap boundary.

The product spec describes continued full draw after20 ticks, while source implementation wraps. This is a source/spec tension requiring an explicit controller ruling and tests, not worker discretion to adopt source overflow or silently call saturation equivalent. No fix or shared-interface design was invented.

## Tunable and saved-state coverage gaps

RuleTunables stores regen_delay/regen_interval/drown_interval/starvation_interval/regen_hunger_threshold/eating_ticks (core/contracts.rs1499–1569) and accepts non-default values, but exposes no getters for these six lanes. Survival hardcodes100/40/20/80/18; eating hardcodes32 at66/182. Their comments explicitly qualify defaults, yet Go advances all these values from the per-tick snapshot. Go eating_test.go605 explicitly tests configured eating duration. This is an acknowledged unimplemented configuration surface, not evidence of full supported tunable parity. I did not change interfaces or add configurable probes, because controller owns that contract.

Survival writes changed health/hunger/saturation/exhaustion into PlayerSave body (490–504), and exhaustion charge counter difference in finding2 reaches that body. Eating atomically stages InventoryPatch, Actor and Runtime; bow atomically stages inventory+progress debit and intentionally leaves the debit committed on defensive spawn failure, matching Go's documented order. Motion and bow publication/save reconciliation outside the provider was excluded because the prior review already identified assembly defects. No actual disk save/restart or real process test was run in this review.

## Checked branches and what the current tests establish

- Motion: intake shape/active key checks; movement -1..1 and finite/bounded look; stale/duplicate and latest input; invalid tombstones; held continuation; yaw normalization; body/eye fluid observations; sweep ground/air acceleration/deceleration, jump/fluid ascent/gravity terminal branches; fused vector length; int32 floor/ceil and prism4096 cap; y/x/z cells; unloaded-as-blocking; air/fluids/plants/torches/snow, bed9/16, farmland15/16, all eight lower door shapes plus upper; NativePhysics staging and sibling lane retention. Shape, simple wall/fluid/fall, finite-out-of-range and sequence cases are covered by existing six tests; missing behaviors above were not covered. I did not re-prove every numeric vector by executing Go.
- Survival: difficulty validation; actor/body and zero-health entry; saved/default runtime merging; regen full-health no-op/counter before hunger gate/modulo and peaceful restore; starvation hunger reset/peaceful/normal floor/hard death; exhaustion wide threshold loop, partial saturation then hunger, zero-threshold fallback, typed receipt draining; oxygen dry refill/positive drain/zero interval; pre-step jump/swim/sprint predicates; swim float64 displacement conversion with cap; fall and peak updates; health/save body sync; pose/damage event construction and zero-tick restriction. Eight tests cover source-default simple states and configured exhaustion threshold. Missing cross-lane interruption, wet landing, post-charge gate, bounds and precision cases are now RED.
- Eating: all seven food IDs and source recovery table; unavailable runtime/basis; release/reset/open viewer/unready view/nonfood/full hunger interruption; slot+item+nonzero progress identity; first tick1;32 settlement; empty consume defensive clear; quantity normalization; wide hunger/saturation clamp against updated hunger; compound inventory/actor/runtime. Four existing tests PASS. No new standalone eating arithmetic defect was found. This is default-duration provider evidence only; no process/order/tunable-duration acceptance.
- Projectiles: scope≤8/radius arithmetic; sorted ID flight and latest target health; age100 entry retirement; gravity before integration; nonfinite next removal; unavailable world ray treated no hit; ray512 cap and bounded lookahead; first block/entity with block exact tie; target kind/ID tie; arrows exclude owner, shards players only, active/dimension/health gates; shared player AABB segment entry; height/subscription retirement; player armor reduction/durability/reset eating/bow, hostile damage, passive flee60/from previous projectile/graze clear; horizontal knockback; arrow owner raw-damage confirmation vs shard silence; missing target lanes and invalid confirmation before compound removal; cap128 oldest-ID eviction, duplicate refusal and64-rehash deterministic ID, source seed/tick/dimension/kind/owner/IEEE position hash; bow interruption/re-entry/ammo scan tiers/debit/order/zero durability and trig rounding.24 tests PASS; no additional ordinary flight mismatch found. Long-hold wrap and invalid-input/bow coupling are new RED. Full mixed-projectile randomized Go parity and real-process projectile publication/death integration were not established.

Refusal evidence: existing tests compare actor/cells/events or richer hit-state snapshots for invalid shape/scopes/target lanes/capacity. Projectile provider intentionally commits earlier projectiles before a later projectile's ray-cap error (documented), so I did not label that as whole-batch atomicity. Actor/runtime stage calls in motion/survival are separate, but current runtime staging has no independently failing shape arm; I did not claim a speculative partial stage failure. Main remains responsible for fallible allocation/resource behavior and whole-tick error policy.

Conclusion: the42 green source-bound cases do not qualify these families as equivalent. The concrete provider defects above are independently reproducible even without the known missing executable integration; acceptance stays pending. Controller must resolve the exact source/spec tensions and coverage gaps explicitly. Architecture skill: no change; these are implementation findings, not stable new cross-task rules.
