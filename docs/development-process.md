---
doc_id: development-process
doc_revision: 2026-09-25.1
language: en
counterpart: development-process.zh.md
---
# Mornlea development process

This is the single current process document. `docs/feature-backlog.md`, role cards in `docs/agents/`, GitHub Discussion #71, and task briefs reference it. Code, tests, and `openspec/specs/` are authoritative.

## Workflow policy

OpenAI-native orchestration is isolation-first. A verified OpenAI ChatGPT/Codex controller has standing authorization to choose direct, delegated, or mixed execution. Prefer a fresh agent for bounded repository discovery, multi-file reasoning, specialized review, or a long trace whose main-context retention cost exceeds its handoff cost; keep only tiny, tightly coupled, or cheaper-to-finish work in the controller. Parallel speed and unused capacity are not sufficient by themselves, and no more than three subagents may run concurrently. Give each worker a concise task brief and a fresh or minimal context. A non-OpenAI or unknown-provider controller must use strict `subagent-driven-development`, including independent implementation and review. Every mode preserves scope, ownership, test-first work, validation, and authorization boundaries.

At the end of each implementation round, promote only stable cross-task architectural conventions into `mornlea-architecture`; otherwise record `Architecture skill: no change`.

## Controller-owned design and worker plans

For every new or materially revised multi-step implementation plan, the main Agent uses the installed Superpowers `brainstorming` and `writing-plans` skills. It settles architectural and functional behavior before dispatch: module ownership, exact APIs and field types, data flow, lifecycle/state transitions, compatibility, failure policy, algorithms and resource limits. Evidence gathering and review may be delegated; design decisions and integration remain with the main Agent. A migration preserves observable behavior while the main Agent designs the target-language ownership and data structures.

Each worker receives exact editable/read-only files, predecessor interfaces, concrete implementation steps and code/algorithm examples, failing tests with expected results, validation commands, exclusions and rollback/integration ownership. The main Agent checks requirement coverage, matching types and an acyclic dependency graph. Broad milestones must become actual independently testable nodes; a missing design decision is not a ready task. A worker returns a contract discrepancy to the main Agent rather than inventing a policy.

Keep decisions in the active OpenSpec `design.md`, status in `tasks.md`, and detailed linked briefs inside that change. Follow the readiness checklist in the project `mornlea-implementation-orchestration` skill. Discover the installed Superpowers resources instead of pinning machine-specific cache paths. Existing user authorization and higher-priority runtime rules control; planning skills do not create another approval flow, external communication or automatic runtime implementation.

For a new or materially changed boundary shared by independent tasks, use the [interface-first parallel development guide](interface-first-parallel-development.md) to land one accepted contract before dispatch and to separate contract, provider and real integration evidence.

## Stages

### Roles and claim discipline

The controller coordinates and rules on work; it may implement directly under the provider policy, but must not bypass required review. A planner maintains planning material and does not claim tasks or modify feature code. A reviewer independently checks the task’s changed behavior. Claim only one `ready` backlog row, change it to `claimed`, record `<agent> @ <branch>` and the exclusive file set, and do not transfer a claim without controller ruling. `queued` and `design candidate` rows are not claimable.

### 0. Claim
Read `docs/feature-backlog.md` and `openspec/config.yaml`; claim only a `ready` row. Record the owner and exclusive file set, claim one row at a time, and preserve unrelated dirty worktree changes.

### 1. Clarify
Classify the work as `spike`, `bounded`, or `architectural`; inspect source, tests, history, and the task source; clarify purpose, boundaries, success criteria, and constraints one question at a time; and present a short design for explicit approval before implementation. The confirmation channel is device-first (`confirm.sh ask` → Feishu reply → `feishu-listener.js`/`AGENT_RESUME`); if unavailable or timed out, use the structured GitHub Discussion fallback and stop at the confirmation point. If requirements change, update OpenSpec artifacts first.

### 2. Isolate and specify
Substantial work uses an isolated worktree/branch. Complex features, new modules, cross-package refactors, save/protocol, concurrency, or performance-contract changes require `proposal.md`, delta specs, `design.md`, `tasks.md`, and `ledger.md`, validated with:

```bash
openspec validate --all --strict --no-interactive
```

Spelling, formatting, and disposable experiments may be direct with proportionate validation.

### 3. Implement
Follow `tasks.md` and red → green → refactor. Keep tests with code, one topic per test file, one shared-helper center per package, and synchronize cross-language constants in one task. For delegation, provide a concise brief containing only the task, necessary evidence and paths, baseline SHA, relevant change artifacts, constraints, ownership, integration point, and exact validation; do not copy the whole controller transcript. An OpenAI ChatGPT/Codex controller decides whether a separate reviewer adds enough context isolation or risk reduction; it is not required to use a one-round implementation/one-round review pattern. Non-OpenAI or unknown-provider controllers retain the strict fresh-implementer and independent-review pattern. Record progress, review when performed, evidence, and `Ruling: <decision> — <reason> — <mistake addressed>` in `ledger.md`; copy unresolved items into proposal.md’s “Deferred and abandoned” section. Reuse evidence by baseline SHA only when unchanged; focused review checks changed behavior, while full race remains a gate.

### 4. Branch review and gates

Use [Continuous integration](continuous-integration.md) for the current required job graph, platform-specific commands, artifact verification, and optional Godot workflow. Run the applicable local gates before publishing a candidate:

```bash
make rust
make test-race
go vet ./packages/contracts/... ./packages/shared/... ./packages/server/... ./packages/client/... ./packages/tools/... ./packages/audit/...
test -z "$(gofmt -l .)"
openspec validate --all --strict --no-interactive
```

Add applicable benchmark, fuzz/golden, visual, and platform gates. Benchmarks are informational; overflow, data loss, report identity, and I/O errors are hard failures. Removed automatic Hooks remain removed; maintain only `scripts/agent-hooks/guard.mjs` and its tests.

For required CI failures, repair the cause and rerun the failed job and its dependent gate explicitly. Do not hide a failure with automatic validation retries.

### 5. Closeout
Confirm `go test -list` sets when splitting files; sync delta specs and archive each change; update scoped guidance and progress only with verified facts. Behavior changes use PR/CI (`gh pr create`, `gh pr checks --watch`, repair and repeat until green, then `gh pr merge --merge`); pure sync/archive documentation may merge directly after local gates. Preserve historical evidence and unresolved items.

## Parallelism and conflicts
Protocol, save-schema, engine/client ABI, and benchmark-scenario upgrades are mutually exclusive. Versioned core gameplay is serial. Parallel work is allowed only when file ownership and version impact do not overlap. Freeze scope after claiming and reconcile OpenSpec artifacts before changing it.

The interface-first guide defines the additional dependency and ownership checks for parallel task readiness; it does not alter agent delegation policy.

## Quick reference

| Stage | Action | Key output |
|---|---|---|
| 0 | Claim | owner and exclusive file set |
| 1 | Clarify | approved design decision |
| 2 | Specify | validated OpenSpec change |
| 3 | Implement | tested change and ledger |
| 4 | Gate | review and validation evidence |
| 5 | Close | synchronized and archived change |
