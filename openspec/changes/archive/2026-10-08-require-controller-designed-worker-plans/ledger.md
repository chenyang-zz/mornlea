# Change ledger

## 2026-09-20 — Controller-owned Superpowers planning rule

User scope: redesign unstarted rust-runtime-foundation tasks using Superpowers and persist the rule that the main Agent resolves architecture/functionality and complete subtask detail before workers implement. This round changes planning/governance only. Repository baseline is60c476645ee6dae1f6392336a7f3c593d2163ae3 on codex/align-runtime-migration-plans.

Completed1.1: root AGENTS.md and openspec/config.yaml now require installed Superpowers brainstorming/writing-plans for new/materially revised multi-step plans; complete controller decisions, exact worker packets and readiness review precede dispatch. Both project orchestration skills route to the identical worker-planning reference. Existing provider/concurrency authorization, user scope and one OpenSpec status source remain controlling. No plugin-cache edits or installation.

Completed1.2: development-process and openspec workflow English/Chinese pairs are synchronized at doc_revision2026-09-20.1, including both documentation-manifest entries. The first documentation audit found the stale manifest revisions; these were corrected and the audit rerun successfully.

Completed2.1: foundation has a common execution contract, three source evidence tables, six detailed subsystem plans and a117-node task index (one retained accepted registration;116 unchecked runtime/acceptance nodes). Main Agent chose all target APIs/algorithms and authored the plans. Read-only agents supplied factual tables only. The old vague/controller-held event and kernel design assignments are replaced with concrete packets. Full readiness review is in ../rust-runtime-foundation/planning-review.md.

Completed2.2 validation:

- `go test ./packages/audit -run 'TestProviderAwareOrchestration|TestDelegationBudget|TestProjectOrchestration|TestProjectOpenSpecApply|TestOrchestrationCarriesDirectoryGuidancePolicy' -count=1`: pass.
- `go test ./packages/audit -run 'TestDocumentationManifest|TestCompletedDocumentationPairsAreSynchronized|TestDocumentationPairValidation|TestLegacyDocumentation|TestDocumentationSemanticPairRejectsCommandPathAndVersionDrift|TestDocumentationLinks' -count=1`: pass after manifest revision synchronization.
- Official skill-creator quick_validate.py against both orchestration skills: pass using existing packages/agent/.venv/bin/python (PyYAML6.0.3). Initial system Python invocation lacked PyYAML; no installation or environment modification was needed.
- Both SKILL.md files and worker-planning references byte-identical.
- Foundation planning check:117 unique IDs,59 packet families exactly matching registry,10 existing kernel families,seven save families,135 valid document/anchor links,no missing dependency/no cycle; only2.1 checked.
- `git diff --check`: pass.
- `openspec validate --all --strict --no-interactive`:125 passed,0 failed.

No runtime production source, test source, fixture, Cargo manifest/lock, Go module or default startup was changed. No Rust build/runtime/race/vet pass is claimed; those gates remain in the actual foundation implementation plan. Architecture skill: no change; this rule is correctly owned by governance/orchestration, and proposed runtime details are not yet verified implementation facts.

Commit preservation: preexisting .codex/skills/pr-submit/SKILL.md and .claude/skills/pr-submit/SKILL.md deletions remain untouched. Foundation ledger.md and tasks.md contain overlapping user/prior-round changes (including preexisting105 ledger lines and earlier2.3/2.4 status edits), and proposal/design/spec carry the prior review revision. The planning redesign depends on those artifacts; no broad commit was made that could absorb or misattribute them. All changes remain reviewable in the working tree; no staging, cleanup or external publication was performed.
