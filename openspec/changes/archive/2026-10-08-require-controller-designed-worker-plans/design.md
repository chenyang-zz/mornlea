## Context

The project already uses OpenSpec and provider-aware orchestration. Superpowers is installed locally; its planning defaults need project routing so the main Agent owns design and workers receive concrete instructions. The current foundation review exposes the cost of vague family-wide assignments.

## Goals / Non-Goals

Make detailed controller-authored planning the default for multi-step work. Preserve project execution choice, user authorization and the single OpenSpec source. Do not impose a model vendor on workers, a fixed reviewer topology, repeated approvals, installed plugin cache paths or a new implementation framework.

## Decisions

1. Root `AGENTS.md` states the mandatory rule; `openspec/config.yaml` routes design/tasks/apply to it. The synchronized project orchestration skill carries a concise entry point and a detailed `references/worker-planning.md` checklist.
2. Use installed Superpowers `brainstorming` and `writing-plans`, discovering their actual skill resources rather than hardcoding a developer's cache version. Save design decisions in the existing `design.md` and task status in `tasks.md`; supporting briefs live alongside that change. The user request authorizes this planning revision; no runtime implementation is implied.
3. Every worker packet contains exact files, prerequisites, interface, algorithm/state transition, bounds, failure cases, concrete test body, commands, commit/integration and rollback. A plan step is one action; a task is one testable behavior. A dependency wait is distinct from an unresolved design decision.
4. The main Agent may ask isolated readers for facts and reviewers for criticism. It must itself choose the architecture, resolve trade-offs, author the task contracts and review their integration. Missing decisions are filled before dispatch, not assigned back to the worker.
5. Migrations preserve behavior with executable independent oracles while the controller chooses native ownership and algorithms. No requirement to replicate legacy data structures, accidental allocation or wrapper layers.
6. Keep English/Chinese explanatory documents synchronized. Leave unrelated historical documents and plugin caches untouched. This change adds a governance requirement; the existing concurrency/provider requirement is not modified.

## Risks / Trade-offs

- Long plans become hard to navigate: one task index plus linked subsystem briefs, shared contracts referenced explicitly rather than copied inconsistently.
- A verbose plan still delegates decisions: readiness checks inspect unresolved choices and concrete examples, not word count.
- Plugin availability differs between machines: discover actual resources and report absence; no silent fallback that claims Superpowers was used.

## Validation and rollback

Run focused governance/orchestration/documentation audits, compare mirrored skill files, validate skill frontmatter/references, and strictly validate OpenSpec. Exercise the readiness checklist on an ambiguous migration brief and a concrete replacement; ensure the former is rejected. Runtime test/race/vet gates belong to actual runtime implementation nodes, not this documentation-only change. Reversing this policy has no save/runtime effect and must preserve completed task evidence.
