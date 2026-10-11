# Independent endpoint review

**Verdict:** SCOPED_PASS; no endpoint source blockers found.
**Production source:** `e4fd8b770ff501ed8db832fa715a10c73cf27fb4`.
**Reviewer:** independent Codex agent `/root/endpoint_closeout_review`.
**Working directory:** `/Users/chen/work/mornlea/.worktrees/rust-authoritative-server`.

Reviewed Go `AgentServiceSettings.Validate`, its config/client/server-bootstrap callers, Rust `AgentEndpoint` and its config/wire consumers, shared parity fixtures/counts, accepted/refused real-Go wire tables, and the runtime companion lease-failure control. The original-authority Go guard prevents older `url.Parse` normalization from hiding prefixed or nested brackets; Rust requires the last opening bracket at authority index zero. Canonical/mapped loopback, scheme case, bracket-containing paths, query/fragment routing, and leading-zero ports retain their established controls. No protocol, save or ABI change is introduced.

The targeted diff from endpoint commit `dc619dd7c` to the production source is empty for every reviewed endpoint producer, consumer, test and parity fixture. The complete diff contains only CI runner/audit and planning/ledger changes. Reviewed endpoint files also have no uncommitted differences from the production source. The committed corpus has 112 bad, 34 good and 6 known-difference fixtures; all 34 good expectations exist. The moved prefix fixture retains its original bytes.

## Independently executed checks

| Command | Exit / observation |
| --- | --- |
| `git rev-parse HEAD` | 0; production source confirmed before and after review. |
| `git diff --stat dc619dd7c e4fd8b770ff501ed8db832fa715a10c73cf27fb4` | 0; four CI/planning/ledger paths only. Targeted endpoint diffs between these commits and against HEAD were also empty. |
| `go version` | 0; `go1.26.0 darwin/arm64`. |
| `go test ./packages/shared/companion -run '^TestAgentServiceSettingsBracketedHost$' -race -count=1 -v` | 0; all seven subtests pass: four accepted controls and three malformed-host refusals. |
| `git ls-tree -r --name-only e4fd8b770ff501ed8db832fa715a10c73cf27fb4 packages/shared/config/testdata/parity` with read-only Python count/expectation assertions | 0; 112 bad / 34 good / 34 expected / 6 known differences. |
| `git diff --check` | 0. |

The closing delta-spec GIVEN clarification restricts the scenario to a configured companion. Read-only inspection confirms both Go `applyAI` and Rust `validate_ai` still return before endpoint validation when companions are missing, null or empty. This matches the existing inactive-AI behavior and does not change the plan173 acceptance criteria. Closing REPORT/tasks keep functional integration, paired-fix synchronization and hosted acceptance separate.

## Limits

This is a local source-scoped review. Rust/Cargo, Go 1.26.9, actual Python Agent integration and full stage gates were not independently rerun; their controller evidence remains separate. No completed tests were repeated when writing this record. The candidate is unpublished and has no Issue anchor, so this verdict is not formal external acceptance, gameplay acceptance or merge/cutover authorization. No product files were edited.
