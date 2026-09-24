# Mornlea Project Guide

## Scope

Agent guidance accumulates along the ancestor chain. The nearest `AGENTS.md` adds rules for its subtree, but scoped guidance must not weaken the global safety, correctness, ownership, or validation requirements in this file.

## Cursor Superpowers subagents

When using the Superpowers plugin for development in Cursor, dispatch its implementer role through this project's Cursor subagent `superpowers-implementer` (`.cursor/agents/superpowers-implementer.md`) and its code review role through `superpowers-reviewer` (`.cursor/agents/superpowers-reviewer.md`). This includes the implementer and reviewer in `subagent-driven-development`. A generic subagent, plugin default agent, or the controller itself does not fulfill either role. If the corresponding project subagent is unavailable, stop that role's dispatch and report the missing configuration.

## Project and contracts

Mornlea's current production implementation is an independent voxel game written primarily in Go 1.26. The root `go.work` coordinates six modules: `packages/contracts`, `packages/shared`, `packages/server`, `packages/client`, `packages/tools`, and `packages/audit`; each module path begins with `github.com/channing771/mornlea/packages/<unit>`, and the repository root is not a Go module. The repository contains the custom client, authoritative server, world storage, physics, the Rust `mornlea_engine` numerical engine, and the Rust `mornlea_client` wgpu renderer. The intended final runtime is documented in [`docs/architecture-target.md`](docs/architecture-target.md): Rust owns the real-time server and client core, while Godot uses embedded Python for presentation. It is not compatible with the official Minecraft protocol, saves, or copyrighted assets.

The current baseline is protocol v45, player schema v9, chunk schema v9, world metadata v6, standalone `companions.ai` schema v5, standalone `hostile_mobs` schema v2, standalone `passive_mobs` schema v1, engine ABI v11, client ABI v19, and benchmark scenario v23.

## Source-of-truth order

When sources conflict, verify the current state in this order: code and tests -> `openspec/specs/` -> `docs/architecture.md` -> `docs/notes/progress.md` -> `docs/superpowers/`. For a new architecture decision, read [`docs/architecture-target.md`](docs/architecture-target.md) as the target and use `docs/architecture.md` only to identify migration seams. Historical documents provide context but do not override verified current behavior or the approved target direction.

## Repository areas and scoped guidance

- Cross-language JSON contracts and `go:embed` exports used by Go and Python: `packages/contracts/`.
- Shared domain packages used by the server and client: `packages/shared/`; more specific guides exist in subdirectories such as `packages/shared/network/AGENTS.md`.
- Simulation, fluids, storage, server runtime, and `cmd/mornlea-server`: `packages/server/`.
- Client session, render CPU half, mesh, LOD, audio, assets, and graphical client commands: `packages/client/`; dependency directions are enforced by `packages/audit`.
- Development tools such as perfcheck, the agent board, gfxspike, and composite grass side generation: `packages/tools/`.
- Cross-module architecture gates that do not import audited units: `packages/audit/AGENTS.md`.
- Rust engines, clients, and C ABIs: `packages/engine/AGENTS.md`.
- Documentation structure, long-lived explanations, and test-organization documents: `docs/AGENTS.md`.
- Scripts, releases, and automation: `scripts/AGENTS.md`.
- Python companion Agent service: `packages/agent/AGENTS.md`.
- OpenSpec project context and artifact rules: `openspec/config.yaml`.

## Directory-scoped guidance

- Important directories are the repository root, top-level modules or packages, and subtree roots that own an independent ownership, dependency, lifecycle, or validation boundary, or coordinate multiple packages, entry points, or asset classes.
- Each important directory MUST have a concise `AGENTS.md` beside the work it governs. It records the directory purpose, directory map, ownership and dependency boundaries, entry points, lifecycle constraints, and focused validation. Use the exact uppercase filename; do not introduce a parallel lowercase `agent.md` convention.
- Create or update the important directory's `AGENTS.md` in the same change when the directory is created, reorganized, or materially reassigned. If it has no independent invariant, inherit the parent guide and do not add a file merely for symmetry.
- `CLAUDE.md` remains a thin import at established repository or subtree roots; it is not a second source of directory rules.
- Use `docs/agents-md-style.md` for the detailed directory-guide structure and self-review checklist.

## Before starting work

1. Read `openspec/config.yaml`.
2. If the task belongs to an OpenSpec change, read its `proposal.md`, delta specifications, `design.md`, and `tasks.md` in that order.
3. Check `git status` and preserve pre-existing and unrelated user changes.
4. On a clean checkout, run `make rust` before a focused Go command for work that involves Rust.

## Global architecture boundaries

- The server is the sole authority for world and player state; clients hold only mirrors, predictions, and presentation state.
- Local Memory and remote TCP modes must reuse the same login, simulation, and validation path.
- No Go package may import WebGPU bindings; the Rust client exclusively owns GPU rendering.
- Go may call Rust only through the repository's established ABI bridges; do not add production fallbacks or side channels.
- The current Go server communicates with the independent Python Agent service only through loopback Agent HTTP and MCP contracts. It must not shell out to, embed, or use FFI with that service. The final Godot product has a separate qualified embedded Python runtime for presentation features; it must not share the Agent's imports, process state, or dependencies. Neither Python runtime may submit unvalidated world actions.
- New real-time architecture work follows [`docs/architecture-target.md`](docs/architecture-target.md): Rust is the final owner of authoritative server logic, protocol/storage contracts, client session/mirror/prediction, and numerical kernels; Godot's embedded Python is the final presentation language. Existing Go runtime and Python pilot code are transition exceptions and must not be expanded without an explicit removal condition in OpenSpec.
- A message and its slices are immutable after a successful cross-goroutine send.
- Authoritative tick, render, and network hot paths must not perform unbounded work or blocking CPU, disk, or network operations.
- Do not add Mojang copyrighted textures or other unauthorized binary art assets.

## Engineering discipline

- Keep changes minimal and focused, reuse existing abstractions, and do not opportunistically refactor unrelated code.
- For new or changed behavior, write a failing test first, implement the minimum fix, and then refactor.
- All new first-party source comments, GoDoc, Rust doc comments, and test comments use English, including every comment added with the Godot architecture. Existing non-English comments are grandfathered and need not be translated as unrelated work; any new or substantively rewritten comment must follow the English rule. Preserve the exact spelling of identifiers, wire magic, external APIs, and technical terms.
- When a comment names a Go identifier, wrap it in backticks and explain intent, boundaries, or trade-offs rather than restating the code.
- New architecture and boundary code must include concise English comments or doc comments at ownership, lifecycle, compatibility, and non-obvious failure decisions. A new architectural unit with no explanatory comments is incomplete; do not add comments that merely narrate syntax.
- Source comments, GoDoc, and Rust doc comments must not contain task identifiers matching `[A-F]-[0-9]{2}`. Refer to the feature or contract by name instead. Task identifiers are allowed only in planning artifacts such as `docs/feature-backlog.md`, `docs/notes/`, examples about this rule under `docs/agents/`, OpenSpec artifacts, and `scripts/`.
- Preserve all pre-existing and unrelated user changes; never revert, overwrite, or clean them without authorization.
- Every Git commit message is one English line in the form `<type>(<scope>): <subject>`. `type` is `feat`, `fix`, `docs`, `refactor`, `perf`, `test`, or `chore`; `scope` is optional; the imperative subject starts lowercase and has no trailing period. Do not add a body, footer, or `Co-Authored-By` line except for messages generated automatically by merge tooling.
- Pull request titles use the same one-line English format. Pull request bodies use `.github/PULL_REQUEST_TEMPLATE.md` in English: concise bullets under `## Summary` for changes and contract/version impact, and one actual command or gate result per line under `## Validation`. Do not add generated-by signatures.
- Do not use destructive Git operations, force-push, skip Hooks, or bypass failing gates with exemption variables unless the user explicitly authorizes that action.

## Development workflow and orchestration

Complex features, new modules, cross-package refactors, save changes, protocol changes, and performance-contract changes require OpenSpec. Reconcile the change artifacts before continuing whenever implementation and planning diverge. Small spelling fixes, formatting-only changes, and disposable experiments may be handled directly but still require proportionate validation.

For every new or materially revised multi-step implementation plan, the main Agent MUST use the installed Superpowers `brainstorming` and `writing-plans` skills. The main Agent owns the complete architectural and functional decomposition: module boundaries, shared types and exact APIs, data flow and lifecycle, state transitions, compatibility and error policy, resource bounds, algorithms, dependencies, concrete failing tests, and acceptance. Workers implement these decisions; they must not be asked to design missing behavior, choose shared ownership, or mechanically translate legacy architecture. Evidence gathering and review may be delegated, but the main Agent retains design and integration responsibility.

Before dispatch, every task MUST have an independently testable deliverable, exact editable/read-only files, prerequisite interfaces, implementation steps with concrete code or algorithm examples, expected failure/success results, validation commands, exclusions, and integration/rollback ownership. The main Agent checks requirement coverage, matching producer/consumer types and acyclic dependencies. Split broad family-wide milestones into actual bounded task nodes; unresolved design choices are not worker-ready. If a worker discovers a contract conflict, it reports evidence to the main Agent for a design/task update instead of inventing a policy. Use the checklist in the project `mornlea-implementation-orchestration` skill.

Superpowers plans stay in the active OpenSpec change: `design.md` owns decisions, `tasks.md` owns status, and linked task briefs hold execution detail. Discover and read the installed skill resources; do not hardcode machine-specific plugin cache paths or silently claim unavailable skills were used. Explicit user authorization and higher-priority runtime instructions control; planning skills do not create redundant approval flows, external messages, model configuration changes, or a second plan store. Planning completion is not implementation acceptance.

After an independently verifiable OpenSpec task or small coherent feature node passes its focused gates, create a scoped Git commit before starting the next node. Use partial staging to exclude unrelated, user-owned, experimental, or incomplete work; never make a broad commit merely to empty a dirty worktree. If pre-existing changes prevent a safe commit, record the exact overlap before accumulating more implementation.

A verified ChatGPT or Codex controller using an OpenAI model has standing project authorization to choose direct, delegated, or mixed execution without a separate per-task user request. This standing authorization means that the absence of an explicit subagent request is not a main-agent-only restriction.

OpenAI-native orchestration is isolation-first. At most three subagents may run concurrently. Prefer a fresh isolated agent for a bounded task that needs independent repository discovery, multi-file reasoning, specialized review, or a long work trace that would otherwise increase main-context retention. Work directly only when the task is tiny, tightly coupled to the controller's current edit, or cheaper to finish than to specify and integrate. Do not delegate merely for parallel speed, independent file ownership, or unused capacity. Give every delegate a concise task brief with only the evidence, paths, constraints, ownership, integration point, and acceptance criteria it needs; do not fork the full conversation by default. Do not restart an already-running agent solely to change its model.

Do not dispatch a subagent using GPT-6 Astra. Select another available model for every new delegation, including implementation, review, and evidence gathering.

The controller records material orchestration decisions, isolation reasons, and rulings in the change ledger. At the end of each implementation round, review verified architectural discoveries for promotion into the synchronized project-owned `mornlea-architecture` skill. Promote only stable cross-task decision rules backed by current code, tests, or canonical specifications; otherwise record `Architecture skill: no change` rather than adding task history or volatile facts.

An explicit user prohibition or higher-priority runtime restriction always controls. A non-OpenAI or unknown-provider controller must use strict `subagent-driven-development`, including independent implementation and review responsibilities as defined by that skill. In every mode, preserve approved OpenSpec scope, test-first development, ownership boundaries, required validation, completion evidence, destructive-action safety, and authorization for externally consequential actions. Orchestration discretion is not authority to skip a gate or expand the task.

See `docs/development-process.md`, `docs/openspec.md`, and `docs/test-organization.md` for detailed workflow guidance.

## Validation

Choose validation in increasing order of risk. Editing loops and task closure normally stop at the lowest proportionate T0/T1 level. Run `dev-check`, `test-race-short` (T2), and full gates (T3) only at stage boundaries such as before push or commit, or when reproducing CI failures. Validation already recorded in a change ledger may be reused for the same baseline SHA. Focused commands and test tiers are documented in `docs/notes/test-quickstart.md`:

```bash
make rust
make companion-agent-check
make companion-agent-integration
go test ./path/to/affected/package -race -count=1
go test ./packages/audit -count=1
make dev-check
make test-race-changed
make test-race
openspec validate --all --strict --no-interactive
```

Benchmark and `perfcheck` measurements are informational and do not change exit status. Report incompleteness, real overflow, data loss, and I/O errors remain hard failures. Automated tests must not launch or focus a foreground game window unless the user explicitly requests manual acceptance testing.

## Hooks

The `scripts/agent-hooks/guard.mjs` gates that were formerly registered in `.codex/hooks.json` and `.claude/settings.json` are no longer installed; both Hook configurations were removed. The implementation and `node --test scripts/agent-hooks/guard.test.mjs` remain in the repository and CI. Agents must still follow every gate above and must not treat the removed automatic Hooks as relaxed policy.
