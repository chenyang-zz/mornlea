# Ledger: align-delegation-concurrency-budget

Baseline: `dev` at `43c8e1d97c54b747cd26ceaa470ba19b79012566`, after archiving `require-controller-designed-worker-plans` (that change also edits `development-governance`, so it is archived first to avoid two pending deltas on the same specification).

Ruling: root `AGENTS.md:89`, `openspec/config.yaml`, both orchestration skills, and `TestDelegationBudgetGuardDetectsDrift` state three concurrent subagents; only the canonical specification stated two. Tests rank above canonical specifications, so the specification is corrected rather than the tests and guidance.

Red: `go test ./packages/audit -run 'CanonicalGovernanceSpecDelegationBudget' -count=1` failed with four violations (two missing three-subagent fragments, two stale two-subagent fragments); the drift-guard fixture test passed.

Green and archive evidence are recorded in the pull request validation section for the same source SHA.

Architecture skill: no change; this is a governance-text correction with no new architectural rule.
