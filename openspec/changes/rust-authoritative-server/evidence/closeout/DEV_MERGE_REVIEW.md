# Final local dev merge paired-source review

**Verdict: SCOPED_PASS.** This independent review binds the prior preservation finding to actual merge commit `aab9b0f0c998924124567bdbe2d22003e1a55b78`. No missing paired implementation, corpus-source drift or loss of the dev replay correction was found.

Review date: 2026-10-11, Asia/Shanghai. Reviewer: isolated OpenAI Codex agent `/root/closeout_inventory`. Scope: immutable commit identities and source preservation only. Prior evidence: `/tmp/mornlea-dev-merge-paired-review.md`. Product, planning, test-running, integration and remote-publication decisions remain controller-owned.

## Independently observed immutable identities

| Fact | Observed value |
| --- | --- |
| Merge commit | `aab9b0f0c998924124567bdbe2d22003e1a55b78` |
| Merge tree | `ef48de84206f2ad70724f623f4f5fbdf7d495401` |
| First parent (previous dev) | `84cd626d6a6f233608bbc63245f1aa09c1f951ee` |
| Second parent (reviewed candidate) | `e9bf930e0a3868ed57a30837b6833bb9b8c38c8c` |
| `refs/heads/dev` | `aab9b0f0c998924124567bdbe2d22003e1a55b78` |
| `refs/heads/cursor/rust-authoritative-server-98e6` | `aab9b0f0c998924124567bdbe2d22003e1a55b78` |
| Feature worktree `git status --porcelain` | empty |

`git show -s --format=%T%n%P` confirms the exact tree and ordered parents. `git merge-base --is-ancestor` returned 0 for both parents, paired endpoint repair `dc619dd7c48312f287ffbc62335b604e488dcaac`, and dev replay repair `a2403f981c6aaf789dcb7c7a8765f5eb8f361458`. Both local refs were independently reread immediately before this final report was written; they still name the reviewed merge.

The actual merge tree is byte-for-byte the same Git tree identity reviewed before commit. Final checks below independently read blobs from the immutable merge commit, rather than relying only on that identity or the mutable working directory.

## Paired implementation and evidence retention

Each of the following thirteen files was byte-compared between merge and candidate commits. All match exactly:

- `packages/shared/companion/agent_config.go`
- `packages/shared/companion/agent_config_test.go`
- `packages/engine/crates/mornlea_server/src/agent/endpoint.rs`
- `packages/engine/crates/mornlea_server/src/runtime/config/client.rs`
- `packages/engine/crates/mornlea_server/src/runtime/config/parity_tests.rs`
- `packages/engine/crates/mornlea_server/tests/server_contract/agent_endpoint.rs`
- `packages/engine/crates/mornlea_server/tests/server_contract/runtime_companion.rs`
- `packages/shared/config/parity_fixtures_test.go`
- `packages/shared/config/testdata/parity/bad/ai_endpoint_prefix_before_bracket.json`
- `packages/server/server/companion_planning_parity_test.go`
- `packages/engine/crates/mornlea_server/src/core/companion_planning.rs`
- `packages/engine/crates/mornlea_server/src/core/companion_planning_tests.rs`
- `packages/engine/crates/mornlea_server/src/agent/snapshot_planning_tests.rs`

The malformed-prefix fixture's old `good/ai_endpoint_prefix_before_bracket.json` and obsolete `expected/ai_endpoint_prefix_before_bracket.json` are absent from the immutable merge tree. Its fixture-class census remains **112 bad / 34 good / 6 known differences**, as pinned by both Go and Rust consumers.

The three companion planning fixtures are also byte-identical to candidate commit blobs and retain the previous reviewed SHA-256 identities:

| Fixture under `packages/server/server/testdata/companion_planning/` | SHA-256 |
| --- | --- |
| `planning_world_bottom_sparse.json` | `9a89d87c9d77d50d7730addb1e7310c5c54d2b0095b747c2e73617849c8bd658` |
| `queued_negative_world_top.json` | `f12d9461e56c0e3d3728c2175632ff55c01ed90e12f7c9023c635f6462606a83` |
| `running_surface_capped.json` | `c36deb6ba5dc42b23ccef9118964772ddd15eeb4e2fd8096f7ba1fc4ae3cdb5d` |

The dev-side `packages/server/server/player_integration_test.go` is byte-identical to first-parent dev, preserving the real-ticker replay correction. Its SHA-256 remains `a2aee8b1f5307eba9d5f86c8be69258a0e26e6118f176f56e6f24a5208d66544`. The merge did not restore the candidate's older test copy.

The exhaustive product-diff census in the prior report remains applicable to these same parent identities: after their common ancestor, the only incoming Go production change is the paired Agent bracketed-host validator; dev has no Go production change. The only tracked path modified on both sides is `docs/documentation-manifest.json`. This final review introduces no documentation policy or additional implementation scope.

## Fresh source-bound corpus checks from the merge commit

A read-only assert-based Python check used `git cat-file --batch` to obtain all reviewed blobs at the exact merge SHA, recomputed SHA-256, and exited 0. The checks are source-preservation assertions, not gameplay tests.

- Foundation manifest: **167 distinct named source files**, **86 families**, **1434 cases**, **zero source digest mismatches**.
- Server inventory: **78 rows**, **17 distinct declared Go byte ranges**, **zero range digest mismatches**. The checker honored `#bytes[start:end]` semantics and did not substitute whole-file or symbol-name hashes.
- The three serialized corpus files preserve their previously reviewed identities:

| File | SHA-256 |
| --- | --- |
| `testdata/runtime-migration/contracts.json` | `6d49a5526a90e04328f62147aaf142b45b6cbc324baec241b47c7fd6d0dcfc4c` |
| `testdata/runtime-migration/server/capability-inventory.json` | `468e0dc679aaf5a7c26ec1cb413fd1185b81371ce2daddadf843f2a96dd66dc1` |
| `testdata/runtime-migration/server/capacity-measurements.json` | `856041f764639de963deeb00c00beb3f2de2af09e98e9466fde31083b5596fd4` |

These serialized-file hashes are distinct from the accepted canonical corpus digest `sha256:8b5813ecce2fe866ab1c4786a35925e8aadd9aab476196abb189d3c72d4f28f3`. The accepted foundation source seal remains `d042982d33bb1694d768b75b01c297bd02534a08`. No corpus identity was regenerated for this review.

## Task 4.5 scope and limits

The local synchronization condition is now source-bound and verified: both local dev and the feature branch include both parents, the paired endpoint implementation and its evidence, and dev's replay fix. The controller may use this result as the **local paired-fix preservation evidence for 4.5**. This report does not alter `tasks.md`, certify any uninspected future source, or make an archive decision.

This is not full F2 acceptance: gameplay executable composition, companion plan-loop/task-runner/non-Overworld work, fluid configuration adoption and other open obligations keep their existing status. Full `dev-check` and race gates are running under the controller; this reviewer did not rerun, interrupt or claim them. No new runtime behavior test was executed.

No remote head was queried or pushed, and the exact merge has not been declared externally readable or hosted-CI accepted. The review is local and the report is accessible to the shared-workspace controller only. Root AOCI removal is explicitly user-owned and this review made no AOCI call or repository write.

Only `/tmp/mornlea-dev-merge-final-paired-review.md` was written. No repository file, commit, branch, index, worktree or remote state was modified by this final identity-binding task.
