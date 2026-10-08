## MODIFIED Requirements

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
