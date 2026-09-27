# P12 strict evidence, case registry and reviewed handoff

This is the controller-owned schema for 1.2 and every P12/P8–P14 producer. The schema is prospective until the P12 CLI and tests land; a plan or fixture-only pass is not a captured run. `tasks.md` owns status. The case registry is `testdata/runtime-migration/tooling/feature-case-registry.json`; the canonical owner registry remains `testdata/visual-golden/producer-registry.json`. Both are content hashed in every accepted run.

## Feature-to-case closure

The registry has one record per `(feature_key, case_id)` with `{class, required, scenario_id, source_case_id, current_owner, capture_kind, minimum_samples}`. Feature keys are exactly `terrain`, `actors`, `ui`, `desktop`, `release` and `tooling-fixture`. Classes are `ui`, `world`, `motion` or `semantic`. `terrain` requires world cases; `actors` requires world and motion cases; `ui` requires UI cases. `desktop` and `release` require semantic cases and may have optional UI/world cases if the 1.1 inventory proves them. `tooling-fixture` is an isolated self-test key and never counts toward product coverage. A semantic case has no image comparison and uses `comparison_status: not_applicable`; its semantic and lifecycle reports must pass. UI/world required cases demand a nonempty qualified GPU capture; motion requires timed samples and explicit review. Each accepted current-owner case from `testdata/visual-golden/{ui,world,motion}` is represented exactly once under a production feature, or 1.1 records a specific exclusion with source and audit evidence. `--feature all --validate-current` checks the whole registry/current-owner set; a `tooling-fixture` run cannot accept 2.4.

## Strict `run.json` version 2

The CLI accepts `--feature <registered-key> --run-dir <explicit-path> --strict`; no implicit latest run, tracked output, network or symlink escape. A strict handoff/release run requires a clean committed `source_sha` and matching content hash; a dirty development candidate may use nonstrict mode with an explicit patch digest but cannot be approved or reused. Run identity is:

```text
schema_version = 2
run_id; feature; source_sha; source_tree_sha256; worktree_state
producer_id; bridge_id; runtime_id; package_sha256?; asset_manifest_sha256
accepted_contract_shas = {F1, F2, F3, G1}; descriptor_table_sha256
case_registry_sha256; canonical_owner_registry_sha256; expected_case_set_sha256
platform = {os, arch, runtime_version}; scenario_id; input_sha256
budgets = {messages, meshes, frame_bytes, rejected_count, overflow_count, io_error_count}
cases[] = {case_id, class, required, scenario_id, input_sha256,
  semantic_status, semantic_report_path, semantic_report_sha256,
  semantic_report_byte_count, capture: {kind, completion_tick?, width?,
    height?, no_focus_proof_sha256?, timing_window?},
  artifact_path?, sha256?, byte_count?, sample_count,
  baseline_owner?, baseline_sha256?, difference_path?, difference_sha256?,
  comparison_status, hard_error_count}
```

All IDs/hashes are nonempty and checked against the actual file or accepted ledger. `package_sha256` is required for `release`. Case IDs are unique and exactly cover required entries for the selected feature; `expected_case_set_sha256` is the canonical sorted registry subset digest. Every case has its own capture metadata and **separate** semantic report and optional GPU/motion artifact; a mixed world/motion run cannot reuse one capture descriptor for both cases. All semantic, capture and difference paths are run-relative, normalized, inside the explicit run directory and never symlinks to tracked paths. Width/height and `no_focus_proof_sha256` are required for qualified GPU UI/world captures; motion requires positive sample count and a bounded `timing_window` with explicit first/last sample and clock identity; semantic-only cases use `capture.kind: none`, a nonempty semantic report and no GPU image. The report validator reads both evidence files for visual cases and the semantic file for semantic-only cases, verifies every hash/byte count and rejects missing, empty, stale, truncated or wrong-producer artifacts. Any required `semantic_status != pass`, `hard_error_count > 0`, `overflow_count > 0`, `io_error_count > 0`, `comparison_status == failed|not_comparable`, or missing review-required difference artifact makes strict acceptance fail. `cross_producer_review` is a candidate state, never a canonical pass. Numeric benchmark variance is informational and does not change process exit status.

The capture script writes to a temporary directory under the explicit run root, `fsync`s/validates completed metadata and artifacts, then atomically renames to its final run ID. Cancellation, capture error or report failure leaves no final run. A no-focus GPU adapter must report actual renderer/viewport identity; a dummy headless renderer can supply semantic/lifecycle evidence only. The semantic replay adapter consumes a recorded F2/F3/G1 transcript; it cannot fake success by reading a previously generated PNG.

## Reviewed per-case transfer

The handoff manifest is versioned and contains one record per touched case/file:

```text
case_id; candidate_run_id; candidate_source_sha; candidate_run_sha256
candidate_artifact_path; candidate_artifact_sha256; candidate_case_input_sha256
case_registry_sha256; canonical_owner_registry_sha256
previous_owner; previous_artifact_sha256; next_owner
difference_path; difference_sha256; semantic_report_sha256
reviewer; approval_record_path; approval_record_sha256; approval_case_id
rollback_manifest_sha256; allowlisted_tracked_paths[]
```

The reviewer approves the exact candidate/run, difference artifact, previous/current hashes and destination owner for that case. Free-text `expected_diff` alone is never approval. The handoff tool verifies a clean committed candidate source, approval record hash/case binding, complete allowlist, staged image/registry hashes and previous owner state before any tracked write. It stages the full image+registry transaction outside the tracked tree, publishes only the allowlisted set, and restores the complete previous set on partial I/O or post-handoff failure. A different candidate, missing difference artifact, dirty source or reused approval fails closed. P12 3.2 remains the serial controller action that obtains explicit per-case human approval; no planning artifact grants it.

## Contract and integration tests

The 1.2 double has valid semantic, valid UI/world GPU and valid motion cases, including a mixed world/motion run with distinct per-case capture and semantic files, plus negatives for absent required case, dirty strict handoff, wrong registry digest and empty artifact. Phase-2 2.4 runs both isolated tooling fixtures and `--feature all --validate-current` against the complete current-owner registry; it records case discovery/execution counts for every class. Handoff 3.1 tests exact approval binding, world capture that incidentally writes motion, symlink/path escape, partial I/O, repeated invocation and full rollback in temporary baseline trees. Release/cutover consumers bind this schema's run and case hashes; they may not borrow a report from another build/test cycle.
