## ADDED Requirements

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
