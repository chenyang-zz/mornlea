# Worker planning contract

Use this reference while authoring or reviewing a multi-step implementation plan, after reading the installed Superpowers `brainstorming` and `writing-plans` skills. Root `AGENTS.md` controls scope, authorization and execution policy.

## Main Agent decisions

Resolve these before producing a dispatchable task:

1. Observable outcome, non-goals, supported versions and compatibility source.
2. Responsibility and dependency boundaries; one owner for every shared type, registry and invariant.
3. Exact public inputs/outputs, field types, units, ordering, mutability and conversion boundaries.
4. Lifecycle/state transitions, resource ownership, concurrency, cancellation and failure publication.
5. Algorithms, integer/float semantics, count/byte/work/allocation limits and overflow behavior.
6. Independent oracle, concrete positive/negative/boundary cases and deterministic expected results.
7. Dependency DAG, serial integration points, exclusive editable files and rollback unit.

For each new or materially changed boundary consumed by multiple tasks, choose one stable contract owner and plan its compile-ready landing before independent work. The landing defines validated public values or operations, executes consumer success and typed failure with a deterministic double, and records an accepted commit SHA. It does not invent a production fallback or claim provider parity. Use `docs/interface-first-parallel-development.md` for the boundary-selection table, minimum contract packet and three evidence gates. If no boundary qualifies, record that choice without adding a gratuitous interface.

Read-only discovery and independent reviews can supply evidence. The main Agent must choose and justify the design, settle conflicts and author the final plan itself. A worker can choose local variable names and equivalent private code organization within its file ownership; it cannot redefine any decision above.

For migrations, explicitly separate preserved external behavior from target-language representation. A legacy implementation supplies compatibility evidence, not an instruction to copy its ownership, unbounded work, allocation patterns or abstraction layers.

## Required task packet

Each node must contain or explicitly link all of the following. Common contracts may be shared, but the task must identify exactly which section it consumes.

- **Identity and readiness:** stable node ID, baseline, direct predecessors and one independently reviewable deliverable. A dependency wait is not unfinished design.
- **Shared contract identity:** accepted contract SHA and exact consumed signatures for any parallel task at a shared boundary; otherwise state the existing stable owner or why no separate landing applies.
- **Files:** exact create/modify/test paths and read-only authority; single owner for exports/shared tests. Include scoped-guide updates when boundaries change.
- **Interfaces:** fully spelled input/output signatures and field maps; specify whether named types are existing or which predecessor produces them.
- **Behavior:** ordered algorithm or state-transition steps, invariants, error precedence, publication and resource limits. State actual numeric bounds and units.
- **Test first:** concrete test code or a fully specified table consumed by named test helpers; include the helper's defining node, inputs, expected outcomes and what fails on the baseline. Do not label a test as behavioral if it only fails to import an absent type.
- **Implementation:** code for the non-obvious logic and precise transformation steps; no undefined helpers, ellipses standing for the algorithm, or instructions to invent edge cases. Full unrelated production files need not be pasted into a plan.
- **Validation:** exact discovery and run commands, nonzero expected case set, focused regression scope, performance counters when relevant, expected red/green distinction.
- **Closure:** exclusions, dependent integration check, controller review evidence, scoped commit message and rollback behavior. Workers report; the controller updates the task status and ledger after verification.

Classify evidence by gate: contract compilation and double execution; real-provider conformance; and real producer-consumer integration on a recorded SHA. A name-only inventory entry, compile-only call or passing double cannot close either latter gate. Assign one editor or a serial order to adapters, registries, generated artifacts and versioned migrations, even when providers have disjoint files.

Steps are small actions: write a specific regression, run and observe it fail for the intended reason, implement the prescribed change, run the focused checks, submit evidence and commit the verified node. A task groups the steps needed for one independently testable behavior. Do not make scaffolding-only microtasks that cannot prove a behavior; do not make an entire subsystem one task.

## Readiness review

The controller must answer yes to each question:

- Can a worker implement from this packet plus its named contracts, without the controller transcript?
- Can every requirement and review finding be traced to a node and an acceptance case?
- Do producer/consumer signatures and unit/ownership/error conventions agree?
- Are dependencies acyclic, with exactly one status source and a clear serial integration order?
- If multiple tasks share a changed boundary, has its compile-ready contract landed on an accepted SHA, and do all consumers use that same identity?
- Are concurrently ready tasks disjoint in editable files, mutable state and version decisions, with shared adapters and registries explicitly serialized?
- Do provider cases execute the real implementation, and does final integration execute real producers and consumers rather than only doubles?
- Are all compatibility, invalid-input, cancellation, resource and publication decisions settled?
- Does the negative case exercise the real boundary rather than mirror implementation details?
- Can a reviewer reject this node without invalidating an unrelated completed node?
- Does the plan preserve unrelated changes and leave external actions within existing authorization?

Reject a packet that says only “port the family”, “add appropriate validation”, “handle edge cases”, “same as the previous task”, or “the controller will design the interface later”. Replace it with the exact decisions and examples before dispatch. A discovery result that changes the contract returns to the main Agent and updates every affected task before implementation continues.

## Source and authorization handling

Keep decisions and implementation briefs within `openspec/changes/<change>/`; `tasks.md` is the single checkbox/status source. Supporting briefs name node IDs but do not maintain competing completion checkboxes. Do not edit installed plugin caches or create a second active plan in `docs/superpowers/`. Discover skill locations through available skill/plugin metadata or local installed `SKILL.md` resources.

`tasks.md` remains the sole OpenSpec plan identity and status source; linked packets describe how to execute a node but never become a packet-keyed status store. Append per-node implementation and status evidence to the change ledger before starting the next node. Batched retroactive acceptance is a recorded deviation, not the normal workflow, and it must identify the affected nodes and recovered evidence.

When a worker finds a frozen-contract conflict, require verified source and a failing case. The controller rules on the behavior, revises the contract and every affected task packet, lands a new accepted SHA, and reruns affected evidence before integration. Unaffected nodes may continue only when they do not consume the changed surface. Do not let a worker silently alter a shared declaration to make a local test pass.

A failed required closeout gate keeps the node open. Deadline pressure, prior review or sync effort, claims that a failure is inherited, and general archive approval do not change that result. A different gate policy requires explicit acceptance-contract revision before archive, with the active artifacts reconciled before the node is accepted.

For an absolute requirement such as “every producer” or “no writer,” require repository-wide producer enumeration during planning and repeat the enumeration during review. Revise the packet and file ownership before dispatch when any producer falls outside the proposed boundary; do not treat a verbal scope interpretation or package-local green test as repository-wide evidence.

Comment-only changes are not exempt from derived-consumer inventory. Repeat the derived-consumer enumeration during review, and reject the node when the final editable-file set no longer matches the assigned refresh authority or algorithm, artifact ownership, or downstream consumer gate. Focused tests do not prove derived consumers are current.

Respect user-supplied scope, chosen execution method and existing authorization. A request to revise design and plan authorizes those reversible artifacts, not runtime deployment. Skill defaults must not create repeated approvals, external notifications or model/provider changes. If a required skill truly cannot be found, state which one is missing and which dependent work cannot be truthfully performed.
