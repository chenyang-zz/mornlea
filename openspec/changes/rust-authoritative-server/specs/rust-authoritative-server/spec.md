## Purpose

Move authoritative server behavior to Rust while preserving validated outcomes, compatibility, bounded execution and reversible single-authority deployment.

## ADDED Requirements

### Requirement: Rust authority reproduces supported outcomes

The Rust server SHALL be the sole writer for its world's ticks, players, entities, inventory and persisted state. Every supported rule in the foundation inventory MUST have equivalent replay outcomes before server acceptance; missing rules MUST NOT silently fall back to Go or Python.

#### Scenario: Deterministic authority parity

- **GIVEN** a recorded Go world, seed and ordered input schedule covering supported rules
- **WHEN** an independent offline Rust server replay runs
- **THEN** authoritative state, events and save observations MUST agree at the declared checkpoints
- **AND** the comparison MUST NOT operate two online writers for one world

#### Scenario: Missing rule coverage

- **GIVEN** a supported rule lacks a Rust implementation or parity result
- **WHEN** server acceptance runs
- **THEN** it MUST report that rule as incomplete and block server cutover

### Requirement: Local and remote inputs use one validation path

Memory and TCP sessions SHALL use identical login, packet, validation, sequencing and authoritative tick semantics. Human actions MUST enter the session-bound validated command path. Agent-derived companion actions MUST enter a separately typed, sessionless candidate ingress and join the same authoritative validation and world-mutation pipeline; they MUST NOT forge a human session or sequence. The independent Agent service MUST remain isolated behind its versioned service contract.

#### Scenario: Invalid intent across transports

- **GIVEN** equivalent sessions send the same stale or invalid action over Memory and TCP
- **WHEN** the server validates their actions
- **THEN** both MUST observe the same existing-protocol outcome and neither MUST mutate world state
- **AND** stale or duplicate applied sequences MUST be silently discarded without a CommandRejected packet; an internal capacity classification MUST NOT create a new wire reason

#### Scenario: Agent timeout and untrusted candidate

- **GIVEN** the Agent times out or returns a candidate invalid at the current tick
- **WHEN** the server processes its result
- **THEN** tick progress MUST remain bounded and the candidate MUST cause no unauthorized world effect

### Requirement: Persistence failures preserve recoverability

The Rust server SHALL preserve supported save migration, crash consistency and explicit I/O error behavior. A failed read or write MUST NOT silently reset a world, discard data, downgrade a schema or publish a successful durable result.

#### Scenario: Interrupted save

- **GIVEN** a save is interrupted by injected I/O failure or process termination
- **WHEN** the server restarts against the test copy
- **THEN** recovery MUST match the supported persistence contract and expose unrecoverable errors explicitly
- **AND** source and rollback fixtures MUST remain unchanged

### Requirement: Runtime work and shutdown remain bounded

Tick and network work SHALL obey explicit queue, capacity and time/work budgets. Blocking storage, AI and network operations MUST remain outside hot-path ownership. Overflow and cancellation MUST have stable outcomes, and shutdown MUST release session and persistence resources.

#### Scenario: Saturation during shutdown

- **GIVEN** queues are full while storage or Agent requests are pending
- **WHEN** shutdown or cancellation occurs
- **THEN** each attempt MUST return within its caller deadline with its retained phase and outstanding ownership
- **AND** an unsuccessful attempt MUST preserve frozen authority and exclusive world ownership for retry, without dropping acknowledged durable work
- **AND** successful completion MUST release owned resources without replaying the final tick or completed sync

### Requirement: Authority selection is reversible

An opt-in Rust deployment SHALL select exactly one server authority. Rollback MUST stop that authority before restoring the previous compatible runtime and data; default-client replacement remains a later release decision.

#### Scenario: Competing owner or incompatible rollback

- **GIVEN** a world is already owned by another server or its data is incompatible with the proposed rollback runtime
- **WHEN** activation or rollback is requested
- **THEN** it MUST fail explicitly rather than dual-write or silently downgrade the save

### Requirement: Acceptance distinguishes actual providers and integrations

A contract or consumer-double pass SHALL NOT accept a provider or full integration. Every supported command, rule branch and lifecycle SHALL map to source-bound positive, failure and boundary results from real Rust code. Rust Agent acceptance MUST execute the actual independent Python HTTP service and MCP consumer against the new Rust producer on a rebuilt source identity.

#### Scenario: Double-only Agent result

- **GIVEN** existing Go/Python integration or a Rust fake-service test passes
- **WHEN** F2 Agent integration is evaluated
- **THEN** the Rust-to-real-Python/MCP gate MUST remain incomplete until that actual path executes nonempty assertions

#### Scenario: Sequence ordering and delayed budget

- **GIVEN** one session queues sequences9,9,8 before a tick and has no applied sequence
- **WHEN** the complete eligible batch is reduced
- **THEN** sequence8 MUST precede the first9 and the second9 MUST have no state or rejection event
- **AND** a zero or partial work budget MUST preserve the original receipt identity and earliest tick for carried records

#### Scenario: Per-key interrupted persistence

- **GIVEN** a save commits one region/key and fails another
- **WHEN** the owner processes completion
- **THEN** only validated committed revisions MUST be acknowledged and uncommitted snapshots MUST remain retryable
- **AND** a corrupt active region payload eligible for fallback MUST load the older payload with the supported promoted logical revision and rewrite flag without writing during load

#### Scenario: Hard authoritative tick failure

- **GIVEN** a trusted reducer phase or delivery fails, or a provider unwinds after an accepted partial mutation
- **WHEN** the executing tick returns
- **THEN** the endpoint MUST return a retained failure without advancing its successful tick counter or delivering that failed publication
- **AND** moved environmental schedules MUST return to their owner without claiming whole-tick mutation rollback
- **AND** new resident captures, save selection and metadata targets MUST remain fenced, while already-selected immutable work retains ownership
- **AND** the same failed authority MUST NOT replay the reduction or report a successful final tick/flush

#### Scenario: Actual unpublished final reduction

- **GIVEN** a healthy authority stops admission with accepted eligible commands
- **WHEN** its actual final reducer runs
- **THEN** it MUST execute the same phase engine with full declared budgets, commit successful accepted mutations and settle the environment once
- **AND** it MUST append no publication frames and advance the final endpoint exactly once across retries
