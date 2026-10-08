## 1. Canonical ceiling alignment

- [x] 1.1 Add `TestCanonicalGovernanceSpecDelegationBudget` and its drift-guard fixture test to `packages/audit/governance_policy_test.go`; confirm it fails against the two-subagent canonical text with `go test ./packages/audit -run 'CanonicalGovernanceSpecDelegationBudget' -count=1`.
- [x] 1.2 Write the `development-governance` MODIFIED delta that changes the ceiling to three and the scenario to three running agents and a refused fourth; validate with `openspec validate align-delegation-concurrency-budget --strict`.
- [x] 1.3 Archive the change into `openspec/specs/development-governance/spec.md`, then confirm `go test ./packages/audit -run 'Delegation|ProviderAwareOrchestration' -count=1` passes and `openspec validate --all --strict --no-interactive` reports zero failures.
