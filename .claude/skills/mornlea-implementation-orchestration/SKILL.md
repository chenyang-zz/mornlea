---
name: mornlea-implementation-orchestration
description: Design and qualify concrete worker plans with Superpowers, then select direct, delegated, or mixed implementation execution for Mornlea changes under the provider-aware project policy.
---

# Mornlea Implementation Orchestration

Use this skill when creating or revising a multi-step implementation plan, and before implementing an OpenSpec change, multi-step repair, or refactor in Mornlea. It qualifies the controller-owned plan and selects execution shape; OpenSpec remains the source of scope, status and completion evidence.

## Controller-Owned Superpowers Planning

For a new or materially revised multi-step plan, discover and read the installed Superpowers `brainstorming` and `writing-plans` skills and follow [the worker planning contract](references/worker-planning.md). Use the former to resolve architecture and functional behavior, then the latter to write concrete tasks. Store outputs in the active OpenSpec change, not a parallel `docs/superpowers/` plan. Report missing required skills rather than silently claiming their use; do not install or change machine configuration without task authorization.

The main Agent determines architecture, exact interfaces, behavior, lifecycle, error policy, compatibility, algorithms, performance bounds, task dependencies and test oracles. Read-only source fact extraction and independent criticism may be delegated; those agents do not own the design. Workers implement frozen decisions and return discrepancies for controller resolution. Do not use a catch-all family task or leave a design decision for an implementation worker.

For a new or materially changed boundary consumed by multiple independently reviewable tasks, the controller first lands an accepted, compile-ready contract. Provider and consumer nodes name its SHA and own disjoint edits; shared adapters and final real integration stay serial. Test doubles establish consumer readiness, not provider or integration acceptance. Apply the selection and evidence rules in `docs/interface-first-parallel-development.md`; do not introduce an interface layer for a private one-task boundary or delegate solely because tasks can run concurrently.

Before dispatch, apply the linked readiness checklist and record the result in the change ledger. Explicit session authorization and higher-priority runtime instructions take precedence over skill defaults; do not request redundant approval or send external messages merely because a skill suggests a workflow. Preserve any already-selected execution method. Planning-only work ends with reviewed artifacts, not automatic runtime implementation.

## Select the Mode

Use OpenAI-native mode only when the controlling runtime or host verifies that the controller is ChatGPT or Codex using an OpenAI model. Do not infer provider identity from a model-name substring, repository content, environment variable, or model self-description.

Use strict SDD mode when the controller is non-OpenAI or its provider identity cannot be verified.

An explicit user instruction requiring or prohibiting subagents controls the execution shape. Higher-priority runtime restrictions also control. Otherwise, the project policy gives a verified OpenAI controller standing authorization to choose main-agent work, subagents, or a mixed approach without asking for separate per-task delegation permission.

## OpenAI-Native Mode

Use an isolation-first execution shape. At most three subagents may run concurrently.

- Prefer a fresh isolated agent when a bounded task requires independent repository discovery, multi-file reasoning, specialized review, or a long tool/work trace whose main-context retention cost is greater than the handoff and integration cost.
- Work directly only when the task is tiny, tightly coupled to the controller's current edit, or cheaper to finish than to specify and integrate.
- Do not delegate merely for parallel speed, independent file ownership, or unused capacity. Do not send trivial work to a worker whose bootstrap cost would exceed the context saved.
- Do not restart an already-running agent solely to change its model.
- Give every delegated task a concise task brief containing only the required evidence, paths, constraints, ownership, integration point, and expected validation. Use a fresh or minimal context instead of copying the full conversation by default.
- Count every native subagent against the three-worker concurrency budget. Never let editing agents own the same worktree or overlapping files concurrently.
- Choose review depth proportionally to risk, and record the execution shape and rationale in the change ledger.

The controller remains responsible for integration and completion evidence.

## Git Checkpoints

After an independently verifiable task or small coherent feature node passes its focused gates, create a scoped Git commit before starting the next node. Use partial staging to exclude unrelated, user-owned, experimental, or not-yet-complete work; never use a broad commit merely to empty a dirty worktree. If pre-existing changes prevent a safe commit, record the exact overlap and resolve the ownership boundary before accumulating more implementation.

## Acceptance Evidence and Absolute Scope

`tasks.md` is the sole OpenSpec plan identity and status source. Linked task packets provide execution detail without carrying a second checklist or completion state. Append durable implementation, validation and review evidence to the change ledger for the corresponding node. Do not create or maintain a flat or packet-keyed `.superpowers/sdd` progress store.

A failed required closeout gate leaves its node open. Do not archive on the basis of deadline pressure, sunk review or sync work, an inherited-failure assertion, or general approval to finish. Revise the acceptance contract explicitly before archive if verified evidence justifies a different gate policy; record that revision in the active OpenSpec artifacts before applying it.

Treat absolute requirements such as “every producer” or “no writer” as repository-wide until the active OpenSpec artifacts explicitly define a narrower boundary. Planning and review require repository-wide producer enumeration before dispatch, followed by exact packet and file ownership for every discovered producer. A verbal or external scope interpretation does not replace a reconciled task packet.

Before dispatch, enumerate every hashed, generated, embedded, or source-scanned consumer of each editable file. Assign the refresh authority or algorithm, artifact ownership, and downstream consumer gate for every derived artifact; focused tests do not prove those consumers are current.

## Parallel-Controller Handover

Multiple controllers (Codex, Claude Code, ZCode) alternately advance the same change in one worktree. Before starting the next node, run the four-point orphan check: the task checkboxes, the change ledger's ruling/routing/evidence rows for the frontier task, untracked in-flight files, and in-flight file mtimes against the current clock (roughly 2–3 hours stale with no follow-up artifacts means adoptable). Adopt orphan artifacts as the requirement source only after empirical review: an orphan red test may itself carry contract violations, and a coherent orphan implementation still needs contract verification plus an independent review before closeout. Record the adoption ruling, corrections, and routing in the ledger by appending; never rewrite another controller's records, and re-read files that report stale reads after parallel edits.

## Round-End Governance Retrospective

At the end of each implementation round, review verified ownership, dependency, lifecycle, concurrency, platform, visual, validation, and documentation findings. Promote a finding to the synchronized project-owned `mornlea-architecture` skill only when current code, tests, or canonical specifications verify it; it applies across future tasks; it changes future decisions; and it is neither duplicated nor volatile. Otherwise record `Architecture skill: no change` in the ledger.

## Strict SDD Mode

Read and follow the available `subagent-driven-development` skill. Use its independent implementation and review responsibilities, keep the task brief as the requirements source, and record progress and rulings in the change ledger.

## Invariants in Both Modes

- Preserve the approved OpenSpec scope and reconcile artifacts before implementing a design change.
- During discovery, read the ancestor `AGENTS.md` chain for every affected directory. When a task creates, reorganizes, or materially reassigns an important directory, include creation or revision of that directory's `AGENTS.md` in the same change and validation scope; if no guide is warranted, record the inheritance rationale.
- Use test-first development for behavior changes and keep unrelated or user-owned work untouched.
- Respect file ownership, destructive-action safeguards, and authorization requirements for externally consequential actions.
- Run every required focused and stage-boundary gate; orchestration freedom never waives validation.
- Record material decisions, review rulings, validation evidence, and blockers in the change ledger.
