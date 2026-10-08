# development-governance Specification

## Purpose

This capability keeps Mornlea's contributor and coding-agent workflow coherent, provider-aware, language-consistent, and mechanically verifiable without weakening correctness, safety, or validation gates.

## Requirements

### Requirement: Implementation orchestration is provider-aware

The repository SHALL support an OpenAI-native orchestration mode and a strict SDD mode. A controller verified as ChatGPT or Codex using an OpenAI model SHALL have standing project authorization to choose direct, delegated, or mixed execution without a separate per-task user request; it MUST NOT be required to run one fresh implementer and one fresh reviewer for every task. A non-OpenAI controller or a controller whose provider cannot be verified MUST use the strict `subagent-driven-development` task loop.

Both modes MUST preserve the approved OpenSpec scope, test-first implementation where behavior changes, file ownership, required validation, recorded rulings, and explicit user authorization boundaries. Orchestration freedom MUST NOT be interpreted as permission to skip a required gate or expand the task.

OpenAI-native mode MUST use an isolation-first execution policy and MUST run no more than three subagents concurrently. A bounded task SHOULD use a fresh isolated agent when it requires independent repository discovery, multi-file reasoning, specialized review, or a long work trace whose main-context retention cost exceeds its handoff and integration cost. Work SHOULD remain with the controller only when it is tiny, tightly coupled to the controller's current edit, or cheaper to complete than to specify and integrate. Parallel speed, independent file ownership, or an unused subagent slot alone MUST NOT justify delegation. Every delegate MUST receive a concise task brief and fresh or minimal context rather than a full conversation copy by default. The controller MUST NOT restart an already-running agent solely to change its model. An editing worker MUST have an isolated worktree or exclusive non-overlapping files.

#### Scenario: Verified OpenAI controller selects direct implementation

- **GIVEN** a ChatGPT or Codex controller has a verified OpenAI model identity and an approved implementation task
- **WHEN** the controller determines that direct implementation is the safest and least coupled execution shape
- **THEN** repository guidance SHALL permit the controller to implement without creating a per-task implementer/reviewer pair
- **AND** the same scope, tests, validation, ledger, and completion evidence SHALL remain required

#### Scenario: Verified OpenAI controller isolates a bounded context

- **GIVEN** a verified OpenAI controller identifies a bounded feature or review context whose main-context retention cost exceeds the handoff and integration cost, and the user has not prohibited subagents
- **WHEN** it selects the execution shape
- **THEN** the standing project policy SHALL permit delegation without an additional user confirmation or subagent-specific prompt
- **AND** the controller SHOULD prefer a fresh isolated agent with a concise task brief
- **AND** the controller SHALL define the isolated context boundary, ownership, integration point, and expected validation
- **AND** repository guidance MUST NOT force a fixed number or sequence of subagents beyond higher-priority runtime constraints

#### Scenario: OpenAI controller reaches the concurrency ceiling

- **GIVEN** three subagents are already running under an OpenAI-native controller
- **WHEN** another independent task becomes available
- **THEN** the controller MUST execute it directly, queue it, or wait for a slot
- **AND** MUST NOT start a fourth concurrent subagent

#### Scenario: Small task does not justify delegation

- **GIVEN** a task is low-risk, tightly coupled to the controller's current edit, and can be completed efficiently without independent research or ownership isolation
- **WHEN** the controller selects an execution shape
- **THEN** it SHOULD complete the task in the main agent
- **AND** MUST NOT delegate solely because a subagent slot is available

#### Scenario: Parallel speed is the only proposed benefit

- **GIVEN** the main agent can retain the task context without meaningful pollution and delegation is proposed only to finish sooner
- **WHEN** the controller selects an execution shape
- **THEN** it MUST keep the work in the main agent
- **AND** MUST NOT spend additional subagent context merely to maximize parallelism

#### Scenario: Context isolation justifies a new subagent

- **GIVEN** a bounded subtask meets the context-isolation criteria
- **WHEN** the controller prepares the delegation
- **THEN** it MUST give the worker a concise task brief with the isolated context boundary, ownership, integration point, and expected validation
- **AND** MUST NOT copy the full controller conversation by default

#### Scenario: Delegated work proves insufficient

- **GIVEN** a delegated worker produces observable validation failure, unresolved contradiction, or material uncertainty
- **WHEN** the controller decides whether to retry
- **THEN** it MUST prefer targeted follow-up or verification over restarting unrelated completed work
- **AND** MUST NOT restart an already-running agent solely to change its model

#### Scenario: Provider is non-OpenAI or unknown

- **GIVEN** the controlling model is non-OpenAI or its provider cannot be verified
- **WHEN** it implements an OpenSpec change, multi-step repair, or refactor
- **THEN** it MUST use the strict `subagent-driven-development` implementation/review loop
- **AND** the implementer and reviewer responsibilities MUST remain independent as defined by that skill

### Requirement: Planning and machine-governance artifacts use English

Active OpenSpec artifacts, newly added or substantively revised canonical OpenSpec prose, new or substantively revised project plans, task briefs, ledgers, `AGENTS.md`, project-owned `SKILL.md`, machine-readable governance configuration, and repository gate diagnostics SHALL use English for normative content. Identifiers, external API names, wire magic, and commands SHALL retain their exact technical spelling.

Unchanged pre-policy canonical specifications SHALL be grandfathered through a checked-in per-file language inventory that rejects new debt, count increases, and same-count non-English replacements. Translating that existing corpus is not required by this change. Archived OpenSpec changes and historical plans MAY retain their original language as immutable evidence. A historical artifact that is only linked, moved, or archived MUST NOT be bulk-translated solely to satisfy the current-language rule.

#### Scenario: New OpenSpec change is created

- **WHEN** a contributor creates a new proposal, delta specification, design, task list, or ledger
- **THEN** its normative prose MUST be written in English
- **AND** OpenSpec validation and repository language checks MUST reject newly introduced non-English normative prose

#### Scenario: Historical evidence remains untouched

- **GIVEN** an archived change or historical plan predates the language policy
- **WHEN** current documentation links to or inventories that artifact without changing its meaning
- **THEN** the historical artifact MAY remain in its original language
- **AND** the language gate MUST NOT require a mass rewrite of that evidence

#### Scenario: Existing canonical specification remains unchanged

- **GIVEN** a canonical specification contains grandfathered non-English prose recorded in the language inventory
- **WHEN** a change does not modify that prose or add new non-English prose
- **THEN** the specification MAY remain unchanged
- **AND** the inventory MUST reject any increase or non-English replacement debt

### Requirement: Explanatory and architectural documentation has English canonical files and Chinese counterparts

Every new or substantively revised explanatory or architectural document SHALL use an English canonical `*.md` file and a synchronized Simplified Chinese `*.zh.md` counterpart. New documentation MUST NOT use `*.en.md` as the English filename. Each pair MUST expose matching document identity and revision metadata, valid reciprocal navigation, and equivalent current version, path, command, and behavioral claims.

An unchanged pre-policy explanatory or architectural document MAY remain in its existing language when `docs/documentation-manifest.json` classifies it as `legacy`. A legacy entry MUST NOT declare partial pair metadata, and its first substantive revision MUST migrate it atomically to the bilingual convention. Legacy classification does not elevate stale prose above code, tests, canonical specifications, or current paired architecture documentation.

Machine-oriented artifacts, copied third-party licenses, generated files, active OpenSpec artifacts, project plans, task briefs, ledgers, and historical evidence are not bilingual-document pairs unless explicitly classified as explanatory or architectural documentation.

#### Scenario: New architectural document is added

- **WHEN** a contributor adds `docs/example.md` as a current architectural document
- **THEN** `docs/example.md` MUST contain the English canonical version
- **AND** `docs/example.zh.md` MUST contain the synchronized Chinese version with matching identity and revision metadata

#### Scenario: One side changes without its counterpart

- **GIVEN** an English/Chinese documentation pair exists
- **WHEN** a contributor changes a current path, command, version, architecture claim, or document revision on only one side
- **THEN** the documentation synchronization gate MUST fail
- **AND** it MUST identify the mismatched pair or claim

#### Scenario: Legacy document is not being revised

- **GIVEN** a pre-policy explanatory document is explicitly classified as `legacy`
- **WHEN** an unrelated change does not modify its prose
- **THEN** the document MAY remain at its existing path and language without a generated counterpart
- **AND** pair validation SHALL apply only after the document is migrated

#### Scenario: Legacy document receives a substantive revision

- **GIVEN** a pre-policy explanatory document is classified as `legacy`
- **WHEN** a change revises its explanation, architecture, commands, paths, versions, or behavioral claims
- **THEN** that change MUST create the English `*.md` canonical file and synchronized Chinese `*.zh.md` counterpart atomically
- **AND** update the manifest classification to `bilingual`

#### Scenario: Existing English-suffixed path is migrated

- **GIVEN** the repository already exposes an English document through a `*.en.md` path
- **WHEN** that document adopts the new naming convention
- **THEN** the English content MUST move to the canonical `*.md` path and the Chinese content MUST move to `*.zh.md`
- **AND** existing internal links MUST be updated and the obsolete `*.en.md` path MUST be removed without leaving an ambiguous third document

### Requirement: First-party source-code comments use English

New comments and documentation comments in first-party Go, Rust, C, GDScript, JavaScript, and TypeScript source SHALL use English. The rule applies to new architecture code, new files, package/file comments, exported API documentation, implementation comments, test comments, and inline comments; it MUST NOT rewrite identifiers, user-visible localized strings, protocol payloads, fixtures, copied licenses, vendored sources, or generated code.

The repository SHALL grandfather the existing legacy comment corpus. Its checked-in per-file inventory MUST allow unchanged existing non-English comments to remain, while rejecting new or replaced non-English comment debt, count increases, and non-English replacements of grandfathered comment text. Translating existing comments is not required by this change. Optional cleanup MAY reduce the inventory, but a zero-debt cutover requires a separately approved future decision.

#### Scenario: Contributor adds a Chinese code comment

- **GIVEN** a first-party source file is new or modified
- **WHEN** the changed comment text contains Chinese prose
- **THEN** the focused language gate MUST fail with the file and line
- **AND** unrelated localized string literals MUST remain accepted

#### Scenario: Existing comment remains unchanged

- **GIVEN** an existing source file contains grandfathered non-English comments
- **WHEN** a change modifies executable code without changing those comment texts
- **THEN** the language gate SHALL accept the unchanged grandfathered debt
- **AND** it MUST still reject any new or replaced non-English comment

#### Scenario: New Godot architecture code is added

- **GIVEN** a contributor adds Go, Rust, C, or GDScript code for the Godot migration architecture
- **WHEN** the new source is reviewed or validated
- **THEN** every comment in that new code MUST use English
- **AND** the legacy baseline MUST NOT be extended to exempt it

#### Scenario: Optional legacy cleanup reduces debt

- **GIVEN** a contributor independently translates a grandfathered comment without changing behavior
- **WHEN** the explicit baseline-update path runs
- **THEN** the recorded violation count MAY decrease
- **AND** the update MUST be rejected if any file or repository total increases

### Requirement: Current governance claims remain consistent with repository facts

Current governance and explanatory documentation SHALL agree on model orchestration policy, enabled or disabled Hook state, repository layout, and all versioned protocol, schema, ABI, and benchmark identities. A gate MUST compare current normative documents with code-owned version sources and MUST reject stale or contradictory current claims.

#### Scenario: Current document contains a stale ABI claim

- **GIVEN** the code-owned engine or client ABI version differs from a current documentation claim
- **WHEN** documentation validation runs
- **THEN** validation MUST fail and identify the document, stale value, and authoritative source

#### Scenario: Current documents disagree about Hook state

- **GIVEN** one current governance document states that project Hooks are installed while the authoritative project configuration shows they are removed
- **WHEN** governance consistency validation runs
- **THEN** validation MUST fail
- **AND** historical documents that explicitly identify themselves as historical evidence MUST remain exempt

### Requirement: Verified architecture conventions are distilled into a project skill

The repository SHALL maintain synchronized project-owned architecture skills for ChatGPT/Codex and Claude entry points. At the end of each implementation round, the controller MUST review the completed work for architectural ownership, dependency, lifecycle, platform, validation, and documentation conventions that are verified, cross-task reusable, and likely to change future implementation decisions.

Only a convention supported by current code/tests/specifications and not already expressed more authoritatively MAY be promoted. Task-specific history, unverified hypotheses, volatile counts, and duplicate policy text MUST NOT be added. The skill MUST preserve the repository source-of-truth order and MUST NOT replace canonical code, tests, OpenSpec specifications, or architecture documents. If no convention qualifies, the controller SHALL record a no-change retrospective in the ledger.

#### Scenario: Completed round reveals a reusable architecture convention

- **GIVEN** an implementation round has completed and validation proves a new cross-task ownership or dependency invariant
- **WHEN** the controller performs the round retrospective
- **THEN** it SHALL update both project architecture skill copies with a concise decision rule and authoritative references
- **AND** both copies MUST remain synchronized and valid

#### Scenario: Completed round has no promotable convention

- **GIVEN** a completed round only confirms existing rules or produces task-specific details
- **WHEN** the controller performs the round retrospective
- **THEN** it MUST NOT add redundant or volatile text to the project architecture skill
- **AND** SHALL record that no skill update was required

#### Scenario: Proposed skill convention conflicts with current truth

- **GIVEN** a proposed skill update conflicts with code, tests, canonical specifications, or current architecture documentation
- **WHEN** the controller evaluates the update
- **THEN** the conflicting skill text MUST be rejected or corrected
- **AND** the higher-priority source MUST remain authoritative

### Requirement: Important directories have scoped agent guidance

The repository SHALL maintain a concise `AGENTS.md` at every important directory boundary. An important directory is the repository root, a top-level module or package, or a subtree root that owns an independent ownership, dependency, lifecycle, or validation boundary or coordinates multiple packages, entry points, or asset classes. The guide MUST state the directory purpose, directory map, ownership and dependency boundaries, entry points, lifecycle constraints, and focused validation. The exact filename SHALL be `AGENTS.md`; `CLAUDE.md` MAY remain only an established thin import and SHALL NOT become a second source of directory rules.

When an important directory is created, reorganized, or materially reassigned, the same change MUST create or update its `AGENTS.md`. A directory without an independent invariant MAY inherit its nearest parent guide and MUST NOT receive a file merely for symmetry. Child guides MUST be additive and MUST be read together with their ancestor guides.

#### Scenario: Important directory is created or reorganized

- **GIVEN** a change creates, splits, or materially reassigns an important directory
- **WHEN** the change is reviewed and validated
- **THEN** the directory MUST contain an `AGENTS.md` covering its purpose, map, boundaries, entry points, and focused validation
- **AND** the guide update MUST be included in the same change scope

#### Scenario: Directory has no independent invariant

- **GIVEN** a directory inherits all applicable ownership, dependency, lifecycle, and validation rules from its parent
- **WHEN** a contributor evaluates whether to add a local guide
- **THEN** the contributor MAY inherit the parent `AGENTS.md` without creating a new file
- **AND** the change record MUST state the inheritance rationale when the directory was considered as part of a reorganization

#### Scenario: Nested guide does not replace ancestor guidance

- **GIVEN** an important child directory has its own `AGENTS.md`
- **WHEN** an agent works in that child directory
- **THEN** it MUST read the ancestor guidance before applying the child rules
- **AND** the child guide MUST add only child-specific constraints rather than copy parent rules

### Requirement: Shared boundaries are accepted before parallel task dispatch

When two or more implementation tasks consume a new or materially changed shared boundary, the controller SHALL identify one owner and accept a compile-ready contract before dispatching those tasks concurrently. The contract MUST define observable inputs and outputs, ownership and lifetime, ordering and units, invalid-input and failure behavior, resource and concurrency bounds, compatibility rules, and executable examples. Consumers MUST be able to compile and test against the contract without a concrete peer implementation. The contract MUST NOT expose implementation internals merely to enable parallel work.

#### Scenario: Two tasks need one new boundary

- **GIVEN** two independently reviewable tasks need the same changed type or operation
- **WHEN** the controller marks them ready for parallel implementation
- **THEN** one accepted contract revision and its implementation identity MUST precede both tasks
- **AND** each task MUST identify that revision and the public behavior it consumes

#### Scenario: A contract compiles with a double

- **GIVEN** a contract is accepted before its production provider exists
- **WHEN** a consumer builds a deterministic test double using only the public contract
- **THEN** the consumer MUST compile and exercise success and failure behavior
- **AND** the double MUST NOT count as provider conformance or integrated acceptance

### Requirement: Parallel task readiness follows real dependencies and exclusive ownership

A controller SHALL record a task dependency graph and exact editable, read-only, and integration-owned files before dispatch. Tasks MAY run concurrently only when their prerequisites have accepted evidence and their editable files, mutable shared state, and versioned compatibility decisions do not conflict. An interface-first plan MUST NOT imply that every task can run concurrently; shared adapters, registries, migrations, and final integration MUST have explicit serial owners and gates. Agent delegation remains governed by the separate provider-aware policy.

#### Scenario: Disjoint providers share an accepted contract

- **GIVEN** two providers consume the same accepted contract revision, own disjoint edits, and have no other dependency
- **WHEN** the controller evaluates dispatch readiness
- **THEN** both MAY begin from that revision and validate their own deliverables independently

#### Scenario: Tasks edit a shared adapter or versioned contract

- **GIVEN** two proposed tasks would edit one adapter, registry, ABI, protocol, or save-version decision
- **WHEN** the controller draws the dependency graph
- **THEN** the overlapping work MUST receive one integration owner or an explicit serial order
- **AND** it MUST NOT be described as independently concurrent work

### Requirement: Contract conformance and integration are separately proven

Provider tasks SHALL validate against executable contract cases, including meaningful failure and boundary cases. Consumer tests MAY use doubles to develop before providers finish, but final acceptance MUST execute the real provider with its consumers and any applicable independent compatibility oracle on one recorded integration identity. A task MUST NOT be closed on type compilation, a name-only inventory entry, an unexecuted test, or a passing double alone.

#### Scenario: All isolated tests pass but the real boundary differs

- **GIVEN** a provider's focused tests and a consumer's double-based tests pass separately
- **WHEN** the integrated provider produces a different error, unit, or result order
- **THEN** the integration gate MUST fail and the dependent work MUST remain open until the contract or implementation is reconciled

### Requirement: Shared contract changes are controller-owned and propagated

An implementation worker MUST report a contract discrepancy with concrete evidence and MUST NOT silently change a shared contract or another task's assumptions. The controller SHALL rule on the discrepancy, revise the contract and every affected task brief, establish a new accepted contract identity, and require affected work to revalidate against it before integration. A change to a versioned wire, save, or ABI contract MUST continue to obey that contract's compatibility and migration rules.

#### Scenario: A worker finds an incompatible field meaning

- **GIVEN** a worker finds that a frozen field's unit conflicts with verified source behavior
- **WHEN** the discrepancy is reported
- **THEN** the controller MUST resolve the unit, update affected producers and consumers, and record the new contract identity
- **AND** work validated only against the previous identity MUST NOT be accepted as current evidence

### Requirement: The controller owns design before task dispatch

For multi-step implementation planning, the main Agent SHALL use Superpowers `brainstorming` to resolve architectural and functional decisions and `writing-plans` to produce concrete task briefs. The main Agent MUST own module boundaries, shared types and APIs, data flow, lifecycle, state transitions, compatibility, failure behavior, resource bounds, test oracles, dependencies and acceptance. Workers MUST implement the specified contract rather than invent these decisions. Bounded evidence gathering and independent review MAY be delegated, but design responsibility MUST remain with the main Agent.

#### Scenario: A worker task leaves an architectural choice open

- **GIVEN** a proposed task requires the implementer to choose shared type ownership, error policy, concurrency behavior or the source of expected results
- **WHEN** the controller checks dispatch readiness
- **THEN** it MUST resolve and record that choice before dispatch
- **AND** it MUST NOT disguise the missing decision as “implement the remaining families” or “handle edge cases”

#### Scenario: Implementation exposes an unplanned contract conflict

- **GIVEN** a worker encounters behavior inconsistent with its brief or verified source
- **WHEN** continuing would require a new architectural or compatibility decision
- **THEN** the worker MUST report the concrete discrepancy and avoid the dependent change
- **AND** the main Agent MUST revise the design and affected dependent tasks before work resumes

### Requirement: Worker briefs are independently executable

Every implementation task SHALL specify its exact file ownership, predecessor artifacts and interfaces, resulting public contract, bounded implementation steps, concrete failing tests and expected outcomes, validation commands, exclusions, integration and rollback responsibilities. A task MUST produce an independently reviewable result. The main Agent MUST verify requirement coverage, acyclic dependencies and matching producer/consumer types before marking a task ready. A milestone containing multiple independently reviewable changes MUST be decomposed into actual task nodes, not left as a worker assignment.

#### Scenario: A worker receives only its assigned task

- **GIVEN** a worker has its task brief and the explicitly linked common contracts
- **WHEN** it starts implementation
- **THEN** it MUST have the inputs, types, algorithms and acceptance examples needed without reconstructing the controller conversation or choosing missing product behavior

#### Scenario: A migration has a working legacy implementation

- **GIVEN** behavior is being moved between languages
- **WHEN** the main Agent designs the target architecture
- **THEN** it MUST separately specify preserved observable semantics and target-language ownership, data structures and resource limits
- **AND** line-by-line translation or self-round-trip tests alone MUST NOT satisfy architectural or compatibility acceptance

### Requirement: Superpowers planning respects the project source of truth

Superpowers design and plan outputs SHALL remain in the active OpenSpec change, with one task-status source and explicit references to supporting task briefs. Installed skills MUST be discovered and read; unavailable required skills MUST be reported instead of being silently simulated. Skill workflows MUST respect existing user authorization, project provider policy and higher-priority runtime constraints, and MUST NOT create a second approval flow, installation, external message or parallel plan store on their own.

#### Scenario: The user requests a design and plan revision

- **GIVEN** the user has authorized redesigning unfinished work and producing its detailed plan
- **WHEN** Superpowers is applied
- **THEN** the Agent SHALL complete the authorized planning artifacts and report their validation
- **AND** MUST NOT confuse planning completion with implementation acceptance or start runtime implementation merely because the plan exists

### Requirement: Gameplay rule changes are batched until the P14 authority switch

Before the P14 default switch to the Rust authoritative server, gameplay rule changes SHALL be grouped into a planned batch OpenSpec change rather than opened as individual changes. A blocking defect MAY be fixed in its own change. A defect is blocking only when it causes at least one of:

- a hang: the server crashes, a tick returns Internal, or the world stops so players must reconnect;
- item loss: items vanish or duplicate without cause, or saved state reads back inconsistently;
- lost progress: a save rolls back, or player effort is irrecoverably discarded in a commonly reached situation.

Feel issues, rule inconsistencies, and rare edge cases with a workaround are not blocking. The gameplay owner decides unclear cases, and the fix's pull request description records the reason. Every gameplay change, batched or blocking, SHALL change the Go authority and the Rust port together and SHALL include Go/Rust parity tests.

#### Scenario: Non-blocking rule inconsistency is found

- **WHEN** a contributor finds a gameplay rule inconsistency that is not a blocking defect before P14
- **THEN** it is recorded for the next gameplay batch change
- **AND** no standalone gameplay change is opened for it

#### Scenario: Blocking defect is found

- **WHEN** a defect causes a hang, item loss, or lost progress as defined above
- **THEN** it may be fixed in its own change without waiting for a batch
- **AND** the fix changes Go and Rust together with parity tests
- **AND** the pull request description records why the defect is blocking

#### Scenario: Classification is unclear

- **WHEN** contributors disagree whether a defect is blocking, or the reporter is unsure
- **THEN** the gameplay owner decides
- **AND** the decision reason is recorded in the fix's pull request description

#### Scenario: P14 switch has completed

- **WHEN** the Rust authoritative server is the default authority
- **THEN** this batching requirement no longer applies and is removed or replaced by a follow-up change
