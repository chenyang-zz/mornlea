# P14 two-cycle release evidence contract

This is the controller-owned v2 contract for P14 1.2 and the two future, real release cycles. It is a plan, not acceptance evidence. The accepted P13 release manifest, P12 v2 run schema and F1–F3/G1 contract SHAs are prerequisites. A mock/self-test proves only the validator, never a platform or cycle.

## One cycle report

`cycle.json` is emitted only after a clean committed build, full target/case tests and a complete previous-release restore. Its schema is:

```text
schema_version: 2
cycle_index: 1|2
release_id; source_sha; source_tree_sha256; build_run_id; test_run_id
release_set_sha256; manifest_sha256; closure_report_sha256
target_packages[]: {target_os, target_arch, package_sha256, manifest_sha256}
accepted_contract_shas: {F1,F2,F3,G1}; descriptor_table_sha256
protocol_version; save_schema_versions; bridge_abi; embedded_python_id
expected_target_set_sha256; expected_feature_case_set_sha256
platform_reports[]: {target_os, target_arch, release_id, source_sha,
  build_run_id, test_run_id, package_sha256, report_path, report_sha256,
  package_closure_sha256, runtime_probe_sha256, executed_case_count,
  hard_error_count, overflow_count, io_error_count}
feature_cases[]: {target_os, target_arch, target_package_sha256,
  feature_key, case_id, class, required, source_sha, release_id,
  build_run_id, test_run_id, p12_run_id, p12_run_sha256,
  semantic_report_sha256, case_artifact_sha256?, difference_sha256?,
  canonical_owner, canonical_owner_registry_sha256,
  owner_decision: approved_godot_handoff|approved_legacy_retention|semantic_only,
  approval_record_sha256?, semantic_status, comparison_status, hard_error_count}
hard_error_report: {path, sha256, total_errors, total_overflows,
  total_io_errors, data_loss_count}
previous_release: {release_sha256, manifest_sha256, release_set_sha256,
  target_packages[], backup_sha256, world_snapshot_sha256, save_schema_versions}
restore_reports[]: {target_os, target_arch, restore_run_id, report_path,
  report_sha256, tested_previous_package_sha256, tested_backup_sha256,
  tested_world_snapshot_sha256, result: pass|fail}
```

Every path is run-relative and resolves inside the explicit cycle directory; every sha256 is recomputed from the artifact. `source_sha` must equal a clean committed source tree and match all platform, feature and P12 strict run reports. `release_set_sha256` is the digest of canonically sorted `(target_os, target_arch, package_sha256, manifest_sha256)` rows. Each platform and feature report must name its target key and match that row's own package hash, plus this cycle's `release_id`, `build_run_id` and `test_run_id`; different OS packages are never required to share bytes. `expected_target_set_sha256` is the sorted digest of all supported P13 target OS/arch rows (macOS, Windows, Linux for the current release plan); `expected_feature_case_set_sha256` is the sorted digest of applicable `(target_os, target_arch, feature_key, case_id)` rows from the accepted P8–P11 and P12 required-case registry, including semantic-only desktop/release cases. The validator rejects a missing, duplicate or extra required target/case, zero executed tests, mock report, wrong actual host OS/arch, stale descriptor/source/package, missing required approval or any nonzero hard error, overflow, I/O error or data loss. A `cross_producer_review` candidate is not canonical acceptance. Every required visual case must have an explicit per-case owner decision: an approved P12 Godot handoff whose canonical baseline/owner matches this cycle, or an explicit human cutover approval to retain the named legacy comparison owner while Godot is selected as runtime. Either decision requires a case-bound `approval_record_sha256`; an unreviewed Godot candidate blocks release even if the legacy owner remains canonical. A semantic-only case uses `semantic_only` and has no visual approval. Each target's restore report proves that target's named prior package, backup and world snapshot were tested on its actual host after this cycle's writer stopped.

The `approved_godot_handoff` proof is the P12 per-case transfer approval bound to the canonical Godot baseline/owner registry; the current cycle must compare to that same approved owner and hash, so a later clean release run does not need to pretend its run ID was known at handoff time. The `approved_legacy_retention` proof is a separate per-case **per-cycle** human review record `{target_os, target_arch, case_id, cycle_index, source_sha, release_id, p12_run_id, p12_run_sha256, semantic_report_sha256, candidate_artifact_sha256, difference_sha256, legacy_owner, legacy_baseline_sha256, reviewer, decision}` with `decision: retain_legacy_for_cutover`. The validator hashes and matches every field to the current case, and requires a fresh decision for cycle 2's distinct candidate. `cross_producer_review` without this proof remains a candidate only; with the proof it permits release comparison against the named legacy baseline but never transfers canonical P12 ownership. The later whole-product cutover approval in P14 3.1 is separate and binds both sealed cycle digests.

## Independence between cycles

Cycle 1 builds target package set A and completes tests and per-target restore. Only afterward may cycle 2 build subsequent target package set B. The two reports must have different `release_id`, `build_run_id`, `test_run_id`, `release_set_sha256`, each matching target package hash, target report hashes, P12 run IDs and restore run IDs. Cycle 2 source SHA may be the same only if a separately committed release input makes target package set B verifiably different; a copied build artifact or repackaged identical bytes fails. Every cycle-2 platform and feature report must bind B's own target package/build/test IDs. Borrowing a passing Windows result, visual run, prior-release restore or hard-error report from cycle 1 fails even when its case contents match. Each cycle is sealed with a digest over `cycle.json` and referenced report hashes. No default switch or source retirement occurs during either cycle.

## Validator and failure fixtures

P14 1.2 registers a permissive fake that incorrectly accepts one cycle, reused platform reports and a wrong backup; these are behavioral reds. The strict validator greens a complete two-cycle fixture and rejects: missing P8 case; absent Windows actual-host proof; duplicate release/release-set/target-package/build/test ID; target report from another target package; P12 run from another cycle; stale canonical owner; unreviewed visual candidate or reused per-case cutover approval; nonzero data loss; dirty source; missing/corrupt restore artifact; and restore against the wrong world. P14 2.3c and 2.4c run the same validator on **real** full reports, plus `make dev-check`, `make test-race` and complete previous-release restore. The ledger records both sealed cycle digests and every actual command/result. Approval for P14 3.1 is a later explicit user decision on those concrete reports.
