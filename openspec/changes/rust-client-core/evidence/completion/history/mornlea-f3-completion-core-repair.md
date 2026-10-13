Core repair implemented and locally committed at `b7caaf3feaf0b1e7b76d2c41859d07bd924d742f`. All required focused core gates pass. This is a developer source repair, not full F3/native/provider acceptance.

- Worker: `/root/repair_core`, isolated branch `fix/f3-core-completion`.
- Worktree: `/private/tmp/mornlea-f3-core-repair`; clean after commit.
- Accepted plan/base: `1f7ef2a9c4001897c277fada98754440867f3e5f`; core review baseline `24f57d73c1e1c2132aa5ad3055767fec2628ca35`.
- Subject: `fix(client-core): bound live mirrors and reject stale source facts`.
- Editable ownership: original seven packet files plus the controller-authorized six presentation fixture files below. No host, spec/status/ledger, registry, exports, dependencies, Cargo configuration or version changed. Main and feature worktrees were not mutated.
- Complete source patch: `/private/tmp/mornlea-f3-completion-core-repair.patch` (same commit diff). Source diff: 13 files, 1016 insertions, 39 deletions.

Behavior

- MirrorProvider owns latest PlayerState tick and normalized bounded `(ActorId,u64)` live non-drop history. Source state clones/stages and swaps only after the complete commit succeeds. Spawn seeds it, removal retires it, construction/reset clears it; adopted live mirror identities seed their first checked state because the unchanged public confirmed stores contain no source tick.
- Stale/equal PlayerState returns current revision with no queued observation. Remote-player/companion stale/equal identity rejects the entire state batch as InvalidInput. Hostile/passive/projectile stale/equal rows suppress individually; wholly suppressed admitted states return current revision. Drops and removals retain existing source semantics.
- Original checked mixed packets and source record ordinals remain intact. Only three pure state readers skip explicit private suppressed indices before resolving dimension. Public AcceptedObservation constructor defaults to all-current. The mask is normalized, immutable after mirror admission and charged by actual backing capacity to retained queue bytes.
- Shared retained count check guards staged commits AND `from_parts`: hostile64, passive32, projectile128, companion4, drop800; selected family limit constrains each and the checked aggregate of all six kinds. Remote7 is not a live cap. Capacity rejects atomically without eviction or watermark mutation.
- Controller queue-preflight ruling implemented: stale-player no-op precedes queue-count demand; non-player queue fullness is checked before store cloning. A wholly stale actor packet at an already-full queue may return Capacity unchanged. No additional freshness preflight helper was added.

Tests

12 new registered session replay tests cover backward/equal player facts after consumption, live actor history across consumption/removal/reuse/reset, strict whole mixed remote/companion rejection with stale first and stale last, every fixed live cap/cap+one, selected aggregate limits and remote8, refusal watermark rollback, mixed actor packet preservation/source ordinals both ways, actual suppression backing bytes and byte-refusal rollback, multi-member spawn Capacity atomicity, seeded fixed/selected cap enforcement, and actual public Memory endpoint stale actor/player publication and input acknowledgment.

Behavioral RED was observed before production implementation: nine selected tests executed, eight assertion failures and one preexisting rollback pass. All eight failures name missing freshness/capacity behavior; no compilation/setup failure is claimed as RED. Initial test import diagnostics were corrected before that behavioral run.

First full suite revealed six presentation fixtures incorrectly feeding stale/equal same-ID facts through real MirrorProvider for pure projection evidence. Controller extended ownership and froze the repair policy. Only source-input construction/view setup changed; all original body, order, source-tick and bounds assertions remain. These are now explicit immutable AcceptedObservation reader inputs (ResolvedActor supplied for dimensionless state). Equal/backward ticks were not changed to increasing ticks. The ingress regressions remain load-bearing. Exact historical failures:

- actors::interleaved_equal_and_absent_ticks_keep_private_source_order
- actors_companion::backwards_tick_keeps_observation_order_not_tick_order
- actors_passive::late_graze_keeps_observation_order_not_tick_order
- world_environment::optional_source_tick_preserved_verbatim
- world_survival::bounds_admit_maxima_and_zeros_and_reject_plus_one
- world_ui::equal_or_absent_ticks_retain_actual_source_order

Actual commands (all cwd `/private/tmp/mornlea-f3-core-repair`)

| Command | Exit/result | Log |
|---|---|---|
| `CARGO_BUILD_JOBS=2 CARGO_TARGET_DIR=/private/tmp/mornlea-f3-completion-core-cargo rustup run 1.97.1 cargo test --manifest-path packages/engine/Cargo.toml -p mornlea_client_core --test session_replay freshness_ --locked` (baseline behavioral RED) | 101; 1 passed, 8 failed, 86 filtered | `/private/tmp/mornlea-core-repair-red.log` |
| Same selected command after first implementation | 0; 9 passed, 86 filtered | `/private/tmp/mornlea-core-repair-focused.log` |
| `CARGO_BUILD_JOBS=2 CARGO_TARGET_DIR=/private/tmp/mornlea-f3-completion-core-cargo rustup run 1.97.1 cargo test --manifest-path packages/engine/Cargo.toml -p mornlea_client_core --locked` (initial full run) | 101; unit48, lifecycle10, prediction48 pass; presentation226 pass/6 fail; session target not reached | `/private/tmp/mornlea-core-repair-green-first.log` |
| `CARGO_BUILD_JOBS=2 CARGO_TARGET_DIR=/private/tmp/mornlea-f3-completion-core-cargo rustup run 1.97.1 cargo test --manifest-path packages/engine/Cargo.toml -p mornlea_client_core --test session_replay --locked` (expanded regressions) | 0; 97 passed | `/private/tmp/mornlea-core-repair-session.log` |
| `CARGO_BUILD_JOBS=2 CARGO_TARGET_DIR=/private/tmp/mornlea-f3-completion-core-cargo rustup run 1.97.1 cargo test --manifest-path packages/engine/Cargo.toml -p mornlea_client_core --test presentation_contract --locked` (fixture repair) | 0; 232 passed | `/private/tmp/mornlea-core-repair-projection.log` |
| `CARGO_BUILD_JOBS=2 CARGO_TARGET_DIR=/private/tmp/mornlea-f3-completion-core-cargo rustup run 1.97.1 cargo test --manifest-path packages/engine/Cargo.toml -p mornlea_client_core --locked` (final source, after queue ruling) | 0; **436 passed**: unit48, lifecycle10, prediction48, presentation232, session98; doc0 | `/private/tmp/mornlea-core-repair-green-final.log` |
| `CARGO_BUILD_JOBS=2 CARGO_TARGET_DIR=/private/tmp/mornlea-f3-completion-core-cargo rustup run 1.97.1 cargo clippy --manifest-path packages/engine/Cargo.toml -p mornlea_client_core --all-targets --locked -- -D warnings` | 0 | `/private/tmp/mornlea-core-repair-clippy-final.log` |
| `CARGO_BUILD_JOBS=2 CARGO_TARGET_DIR=/private/tmp/mornlea-f3-completion-core-cargo rustup run 1.97.1 cargo fmt --manifest-path packages/engine/Cargo.toml --all --check` | 0 | `/private/tmp/mornlea-core-repair-fmt-final.log` |
| `git diff --check` | 0 | `/private/tmp/mornlea-core-repair-diff-check.log` |

Cargo used the unoccupied compatible reviewer external target and compilation jobs2. Final successful suite was rerun after the controller requested restoring count preflight before clones. No failure is relabeled as success.

Changed files

- `packages/engine/crates/mornlea_client_core/src/presentation/actors/hostile.rs`
- `packages/engine/crates/mornlea_client_core/src/presentation/actors/passive.rs`
- `packages/engine/crates/mornlea_client_core/src/presentation/actors/projectile.rs`
- `packages/engine/crates/mornlea_client_core/src/presentation/mod.rs`
- `packages/engine/crates/mornlea_client_core/src/session/mirror.rs`
- `packages/engine/crates/mornlea_client_core/tests/presentation_contract/actors.rs`
- `packages/engine/crates/mornlea_client_core/tests/presentation_contract/actors_companion.rs`
- `packages/engine/crates/mornlea_client_core/tests/presentation_contract/actors_passive.rs`
- `packages/engine/crates/mornlea_client_core/tests/presentation_contract/world_environment.rs`
- `packages/engine/crates/mornlea_client_core/tests/presentation_contract/world_survival.rs`
- `packages/engine/crates/mornlea_client_core/tests/presentation_contract/world_ui.rs`
- `packages/engine/crates/mornlea_client_core/tests/session_replay/live.rs`
- `packages/engine/crates/mornlea_client_core/tests/session_replay/mirror.rs`

Evidence SHA256 (local paths accessible to controller; this manifest binds them to the above committed source and historical attempts)

- `/private/tmp/mornlea-core-repair-red.log`: `fb2d945c6fcad20e9eb8576828ae5140380796e0bd956e8eb3c2532b5cd343a5`
- `/private/tmp/mornlea-core-repair-focused.log`: `2d1a9725ae5fad9871bbef413334431fa052d3401136f53ee129479d07297150`
- `/private/tmp/mornlea-core-repair-green-first.log`: `369b67aea3b5ebff3e54e56bbc128bfbcc4bc58a67b24d38a92f94650b1edb09`
- `/private/tmp/mornlea-core-repair-session.log`: `25d07df02e088e80b78bf88f78ddd2adee2e20aaf30e60b4c3a52f5fbc0e7734`
- `/private/tmp/mornlea-core-repair-projection.log`: `ccdcc94ec4f8babf169af5d00ae8ff9c957337cc32e39e329f031a631063c861`
- `/private/tmp/mornlea-core-repair-green-final.log`: `a13aa01bb8650955eea9fe16d8084c906e3070291a5df41e610e56a593dedc8e`
- `/private/tmp/mornlea-core-repair-clippy-final.log`: `e8d3fabd3f18da3a147914b37b6dcbcfdb7a5383b6405030b2d492dbfd849443`
- `/private/tmp/mornlea-core-repair-fmt-final.log`: `e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855`
- `/private/tmp/mornlea-core-repair-diff-check.log`: `e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855`
- `/private/tmp/mornlea-core-repair-commit.log`: `bc4fb25a973edf44fde9df30bea402bec9ea36ffa20c88fa104c78836f87d614`
- `/private/tmp/mornlea-f3-completion-core-repair.patch`: `d794596c1682071ff42f5782daeacce3376663b532f5ce2e23857abd1a58868b`

Handoff and limits

Controller owns cherry-pick/integration, status/ledger, generated registry/native refresh, source-SHA handoff round and rollback. Independent review must rerun the external original probes at the integrated immutable SHA; the original review probe source and red logs were not changed. Rollback this scoped source commit and dependent qualification only.

Architecture skill: no change. The repair applies the existing single confirmed owner/immutable projection/bounded admission conventions; the family tick policies and numeric ceilings belong to the current contract, not new cross-task architecture guidance.

Not run: native Godot/Python, native100, full Memory/TCP/F2 integration qualification, full machine Rust/Go/audit/OpenSpec gates, other OSes, GitHub publication or final dev merge. No focused Go command was run, so `make rust` was not skipped as a Go prerequisite.
Risk: these green core gates establish the scoped repair only; full F3/native/provider acceptance and immutable integrated review remain controller-owned and pending.
