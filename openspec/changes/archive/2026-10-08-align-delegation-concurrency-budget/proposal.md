## Why

The canonical `development-governance` specification caps OpenAI-native orchestration at two concurrent subagents, while root `AGENTS.md`, `openspec/config.yaml`, `docs/development-process.md`, `docs/openspec.md`, both `mornlea-implementation-orchestration` skill copies, and the `TestDelegationBudget*` audit tests all state three. The source-of-truth order ranks tests above canonical specifications, so the specification is the stale copy. No test reads the specification, which is why it drifted.

## What Changes

- Modify the `Implementation orchestration is provider-aware` requirement so OpenAI-native mode runs no more than three subagents concurrently, and update the concurrency-ceiling scenario to three running agents and a refused fourth.
- Add an audit test that fails when the canonical specification loses the three-subagent ceiling or reintroduces the two-subagent wording.

## Capabilities

### New Capabilities

None.

### Modified Capabilities

- `development-governance`: The canonical subagent concurrency ceiling becomes three, matching every other governance source.

## Impact

- Affected files: `openspec/specs/development-governance/spec.md` (through archive) and `packages/audit/governance_policy_test.go`.
- User outcome: contributors and agents read one consistent concurrency ceiling regardless of which governance source they open.
- Compatibility, saves, protocol, ABIs, runtime concurrency, and performance: not applicable; this changes agent-orchestration governance text and an audit test only.
- Non-goals: changing the ceiling value used by `AGENTS.md`, configuration, skills, or existing tests; changing any other orchestration rule.
- Rollback: revert the archive commit and the audit test together.
