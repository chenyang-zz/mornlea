# Independent scoped host repair rereview

What this change does: The host keeps one bounded latest Rust publication failure string, exposed through `last_rust_failure()`. Refused or malformed publications continue to preserve the prior frame and consumer resources; successful publication, Rust activation or reset clears the status.

Reviewed source: `c0c008684057971e9311a2eb30f675c1f3312aad`.
Reviewer: Codex delegated `/root/review_native`, independent read-only product review.
Checkout: `/private/tmp/mornlea-f3-completion-host-rereview`, newly created detached exact-SHA worktree, clean before/after validation.
Scope: packet18 node6.2, actual `apps/mornlea-godot/app/host/feature_host.py`, its connected existing activation/reset/step/pull/frame-copy/release behavior, and regression harness `tests/unit/test_rust_feature_host.py`. Read the complete39-line production repair and69-line test addition against the original approved failure-reporting scenario. The final integrated core/source is not yet reviewed or accepted by this report.

## Verdict

The original silent-publication-failure finding is resolved in this scoped host source. No remaining reproduced defect was found in the requested host repair. This verdict accepts node6.2's host failure-reporting behavior at the SHA above; it does not accept final F3/native/runtime integration.

The new accessor reports exact closed G1 classes for refused step/pull, a stable InvalidInput reason for malformed successful bodies or native fanout-copy refusal, and UnknownError for unknown class/missing-shape failure envelopes. It retains only one short fixed-vocabulary string. Failed calls do not replace the owned frame or fan out partially; repeated failures do not add logs or trace history. Valid publication clears status before ordered fanout, refused reset retains it, and successful reset/activation clears it.

## Independent behavior evidence

The durable probe `/private/tmp/mornlea-f3-completion-host-status-probe.py` imports the actual production host through the repository's existing native-container shells. It independently tests every existing G1 class (`InvalidInput`, `IncompatibleVersion`, `InvalidState`, `StaleEpoch`, `Capacity`, `Timeout`, `Disconnected`, `Io`, `Internal`) for both step and pull refusal, three repeated calls per class. Every status is exact, the previously published frame remains identical, consumer apply count remains unchanged, and `trace_json()` does not grow. It also tests unknown large arbitrary error text, missing-field failure envelope and absent result for each method, valid recovery and fanout resumption, refused-reset retention and successful-reset clearing. All assertions pass. The three committed regressions additionally cover malformed success bodies, frame-copy refusal and successful new activation.

Historical evidence erratum: The original review/probe case text described the existing scripted bridge's default refusal as Internal. The harness default is actually InvalidState. Original report/probe bytes remain preserved; this corrects only the label, not the independently observed silent-refusal/preservation behavior. The new probe explicitly exercises both InvalidState and Internal (and all seven other closed classes).

## Actual commands

Working directory for every command: `/private/tmp/mornlea-f3-completion-host-rereview`.

| Command | Exit/result | Evidence |
| --- | --- | --- |
| `/Users/chen/work/mornlea/apps/mornlea-godot/.venv/bin/python -B -m unittest discover -s apps/mornlea-godot/tests/unit -v` |0;48 tests passed | `/private/tmp/mornlea-f3-completion-host-rereview-unit.log` |
| `/Users/chen/work/mornlea/apps/mornlea-godot/.venv/bin/python -B /private/tmp/mornlea-f3-completion-host-status-probe.py` |0;18 exact method/class cases with3 repeats each,6 malformed-result cases, preservation/recovery/reset assertions | `/private/tmp/mornlea-f3-completion-host-status-probe.log` |
| `/Users/chen/work/mornlea/apps/mornlea-godot/.venv/bin/ruff format --check apps/mornlea-godot/app/host/feature_host.py apps/mornlea-godot/tests/unit/test_rust_feature_host.py` |0;2 files already formatted | `/private/tmp/mornlea-f3-completion-host-rereview-format.log` |
| `/Users/chen/work/mornlea/apps/mornlea-godot/.venv/bin/ruff check apps/mornlea-godot/app/host/feature_host.py apps/mornlea-godot/tests/unit/test_rust_feature_host.py` |0;all checks passed | `/private/tmp/mornlea-f3-completion-host-rereview-lint.log` |
| `MYPYPATH=apps/mornlea-godot/typing /Users/chen/work/mornlea/apps/mornlea-godot/.venv/bin/mypy --cache-dir=/private/tmp/mornlea-f3-completion-host-mypy --config-file apps/mornlea-godot/pyproject.toml apps/mornlea-godot/app/host/feature_host.py apps/mornlea-godot/tests/unit/test_rust_feature_host.py` |0;no issues in2 files, actual repository stubs | `/private/tmp/mornlea-f3-completion-host-rereview-mypy.log` |
| `git status --short` and `git rev-parse HEAD` |0;clean exact reviewed SHA | independently checked after all runs |

## Scope identity for later integration verification

SHA256 at scoped repair source:

- `apps/mornlea-godot/app/host/feature_host.py`: `20ea27fdc7c9bad6c85bb304d5e7c651cf6d805ae87c633c3467ca1a9c532a73`
- `apps/mornlea-godot/tests/unit/test_rust_feature_host.py`: `093eee2897f84ec7bcaceb5591af37edb70069ea1c47e3c74503a8d5b31e7deb`

Later integrated-source review must independently confirm these bytes and the associated stub/contract dependencies before carrying this scoped acceptance. Core changes remain pending; a scoped repair SHA is not final source or evidence of rebuilt native100. No Godot/shared runtime launch/install/materialization/rebuild, production edit, remote write or merge occurred in this rereview.

Not checked: final integrated core/source, native marshalling/process100, derivative editor/export matrix, full F2/all-family acceptance and aggregate stage gates; controller and other reviewers own them. Risk: this report alone cannot close full F3 qualification or authorize merge.
