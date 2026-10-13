What this change does: The integrated repair makes the existing Rust confirmed ingress retain source freshness across frame consumption and enforce live actor limits. It preserves immutable original packets and ordinals through a private suppression mask, and leaves C1/C2/G1 public APIs and versions unchanged.

Looks good for the assigned core scope. Both prior core blockers are resolved at `2cea2020091168ea5a6845517bc49ccbdbae0268`. No additional definite original-criterion failure was found. This is scoped core acceptance, not native, provider, full-stage, remote or merge acceptance.

## Identity and scope

- Independent reviewer: OpenAI Codex `/root/review_core`.
- Frozen integrated source: `2cea2020091168ea5a6845517bc49ccbdbae0268`.
- Reviewed repair commit: `807afcd2002e094731e2b02fe02d9e8d2deb1ec8` and its connected caller/test/fixture changes; parent also identified worker predecessor `b7caaf3fe`, host repair `c0c008684` and additive inventory integration.
- Comparison source: `24f57d73c1e1c2132aa5ad3055767fec2628ca35`; earlier review `/private/tmp/mornlea-f3-completion-core-review.md` remains historical for that SHA.
- Own detached worktree: `/private/tmp/mornlea-f3-completion-core-final-review`. Checked exact HEAD and clean status before/after review. No tracked product, test, planning or inventory edits.
- Read current packet18 `plans/18-completion-review-repairs.md`, scoped guide and actual source/fixture diff. Applied the refined queue-count preflight, PlayerState cheap no-op, remote aggregate-cap exception and strict-versus-lenient source rules, rather than inventing new policy.
- Re-reviewed MirrorProvider construction/commit/staged stores/freshness; all mask setters/readers/accounting; five actor watermark branches and six live actor cap branches; the actual LoginSession ingress/effects/UI/ack path; frame and publication consumption callers; CoreArena endpoint consumer seam; the six changed pure-projection fixtures; the twelve new ingress regressions and additive inventory links. The full436-test core run also exercises unchanged input/prediction/preparation/terrain/I/O/lifecycle families from the initial review.
- Native Godot/Python, real F2 provider integration and global gates remain controller/other-reviewer scope; this reviewer did not run those commands.

## Verified repair behavior

1. **Freshness belongs to the sole confirmed ingress owner.** MirrorProvider stages SourceFreshness beside mirror stores and only swaps it after complete actor-cap, observation-count and retained-byte admission. Actor watermarks are bounded by accepted live non-drop identities, survive observation consumption, and retire with despawn; a fresh MirrorProvider starts empty. Adopted mirrors have no invented tick and seed their first checked state. PlayerState at/below retained tick queues no observation, returns the current revision and cannot reach prediction, acknowledgment, survival or environment providers.

2. **Strict and lenient actor policies remain distinct.** Remote-player/companion states reject the whole packet if any member is stale/equal. Hostile/passive/projectile packets suppress stale rows individually, retain fresh rows and advance the revision once. Entirely suppressed packets leave the current revision/queue unchanged after the accepted count preflight. Stale PlayerState does its cheap no-op before that queue preflight. Drop updates and all removals retain their existing source-order behavior; no blanket actor tick filter was introduced.

3. **Masks preserve provenance and own charged storage.** Suppression indices are generated in ascending original packet order, normalized through boxed storage and private after commit. The original protocol packet is retained unchanged. Exactly the three lenient state readers query the mask before dimension lookup, while enumerate-derived ProjectionOrder ordinals remain original indices. Resolved entries are filtered by the same original indices. Both first/second stale-member positions pass for each kind. Queue accounting charges mask capacity times usize size; cap-minus-one refusal preserves prior owner state and allows a subsequently valid lower tick, proving refusal did not advance freshness.

4. **Live counts are bounded independently of queue consumption.** Hostile64, passive32, projectile128, companion4 and drop800 are enforced, each constrained by selected family_records; aggregate live identities across all six kinds use the selected family bound. Remote7 is not misused as a live cap. Exact fixed caps, cap+one, selected aggregate caps, seeded constructor caps, multiple new spawn members, removal then lower-tick reuse and epoch replacement pass. Capacity is whole-observation refusal with mirror/revision/queue unchanged. An already-full actor observation queue can refuse before store cloning, as packet18 specifies. Later staging is bounded and checked before publication.

5. **The six fixture changes preserve reader evidence.** `actors`, `actors_companion`, `actors_passive`, `world_environment`, `world_survival` and `world_ui` keep their prior ordering, source-tick, bounds and body assertions. They now build exact AcceptedObservation/view inputs directly when the property intentionally includes stale/equal ticks that real ingress no longer admits. Comments explicitly name pure reader evidence. Actual ingress behavior is independently asserted in MirrorProvider/public-endpoint regressions; the fixtures do not replace those tests or relabel reader output as live admission.

6. **Inventory references are additive and executable.** A recursive JSON comparison confirms only appended rust_test/rust_tests references, with12 unique new names. Every name appears as passed in the exact-source focused12-test log. Existing packet variants, source fields and old test references remain unchanged. This small comparison is not a claim that the controller's full inventory/runtime gate ran here.

## Actual checks

All builds used `CARGO_BUILD_JOBS=2` and the reviewer-owned warm target `/private/tmp/mornlea-f3-completion-core-cargo`.

| cwd | Actual command | Exit/result |
|---|---|---|
| main workspace | `git worktree add --detach /private/tmp/mornlea-f3-completion-core-final-review 2cea2020091168ea5a6845517bc49ccbdbae0268` | 0 |
| detached worktree | `CARGO_BUILD_JOBS=2 CARGO_TARGET_DIR=/private/tmp/mornlea-f3-completion-core-cargo rustup run 1.97.1 cargo test --manifest-path packages/engine/Cargo.toml -p mornlea_client_core --locked` | 0;436 passed: unit48, lifecycle10, prediction48, presentation232, session98;0 ignored; doc tests0 |
| detached worktree | same prefix and manifest with `--test session_replay --locked freshness_` | 0;12 passed,86 filtered; independent focused repair run |
| detached worktree | `git diff --check 24f57d73c1e1c2132aa5ad3055767fec2628ca35 HEAD -- packages/engine/crates/mornlea_client_core` | 0 |
| external copied probe | `CARGO_BUILD_JOBS=2 CARGO_TARGET_DIR=/private/tmp/mornlea-f3-completion-core-cargo rustup run 1.97.1 cargo test --manifest-path /private/tmp/mornlea-f3-completion-core-final-probe/Cargo.toml --offline --locked -- --nocapture` | Original unmodified assertions:101;3 passed, one legacy unconditional unwrap hits the now-correct Capacity. Updated hostile probe assertion only:0;all4 pass |
| detached worktree | Read-only recursive inventory JSON comparison and lookup of all12 appended test names in focused log | 0; only additive refs, all12 independently executed |
| detached worktree | `git status --short; git rev-parse HEAD` | 0; clean, exact2cea2020091168ea5a6845517bc49ccbdbae0268 |

The original hostile-cap probe assumed all65 commits succeeded, then checked retained count. Replaying it unchanged preserves its actual nonzero result; its failure is now at setup unwrap on typed Capacity, before the old over-cap assertion. In a copied external probe, only that unconditional unwrap was replaced with the approved expectation for65: Capacity, unchanged mirror and empty observation queue; the original retained-count assertion remains. No candidate source changed and no product failure was hidden. The other three original probes ran unchanged and pass. Final retained counts are hostile64/companion4/passive32/projectile128/drop800, with no stale tick11 actor or survival/environment upsert.

Logs and SHA256:

- Full suite `/private/tmp/mornlea-f3-completion-core-final-tests.log`: `fdd5e2722af13ab65a7cb65f5275114de134a70daac0b1fe6121bcac875e324c`.
- Unmodified old probes `/private/tmp/mornlea-f3-completion-core-final-original-probes.log`: `a1de1113af1fe7056bdca922c2cccb0fd59eb39616fe907d2bcb72cf3c431172`.
- Approved refusal assertions `/private/tmp/mornlea-f3-completion-core-final-probes.log`: `6a17af58901c2104781407f661f40cfac7362fff17f9d7a4cf7a523360dba764`.
- Final copied probe source `/private/tmp/mornlea-f3-completion-core-final-probe/tests/probe.rs`: `8fe351d23e88cefaf32e69c09f4d3cddaad16b202043b85bb99ab0975f253a82`.
- Focused log `/private/tmp/mornlea-f3-completion-core-final-focused.log`, independently run12 repair cases: `33ac25f365f9f4097696040979ba044b6a2f1a7022c7120f258f420e18d25ca2`.

Initial packet18 lookup used an incorrect filename and emitted a read diagnostic; corrected to the actual current filename before review. The first extra inventory comparison only recognized the plural rust_tests key and asserted on21 singular rust_test append locations; the shell then printed a hash and returned0. That assertion was a checker-shape error, not passing evidence. The corrected standalone checker recognized both actual keys and independently returned0. No source/test gate was claimed from the failed read or checker assertion.

## Seven original criteria, final scope verdict

| Original criterion | Verdict for assigned core portion |
|---|---|
| Client state has one Rust owner | Pass: freshness/count policy remains in MirrorProvider; pure projections and G1 consume the same owner |
| Frame publication is atomic and bounded | Pass for reviewed core: both prior stale/retained-limit failures resolved, unchanged-state refusal/ordinals/mask charge and existing publication/resource checks pass |
| Reset and bridge lifecycle release ownership | Core pass; native bridge/process release acceptance belongs to controller |
| Rust core parity is independent of rendering | Reviewed headless core parity pass:436 tests, all4 external probes and source-specific strict/lenient/drop policy; real F2 provider acceptance belongs elsewhere |
| Migration preserves typed host isolation | Core dependency/typed caller seam pass; actual host/native boundary qualification not asserted here |
| Cross-runtime boundaries publish only bounded atomic snapshots | C1/C2 core portion pass; unchanged public API, admitted freshness, input/frame atomicity and retained bounds checked; native conversion belongs elsewhere |
| A Rust-mode host tick distributes one complete semantic frame | Core complete ten-family endpoint portion pass; actual Python/native fanout belongs elsewhere |

No remaining Critical/Important core finding at this immutable candidate. The core portions may be combined with independent same-source host/native/provider/global evidence by the controller; they do not close missing portions on their own. A source change invalidates this candidate verdict and requires affected revalidation.

Not checked: native Godot/Python process matrix/native100, complete real F2 integration and global Rust/Go/audit/OpenSpec gates, other operating systems, remote source/evidence readability. No product files changed.
Risk: full F3 acceptance and merge readiness still depend on the controller's exact-source qualification evidence; this report approves only the core scope.
