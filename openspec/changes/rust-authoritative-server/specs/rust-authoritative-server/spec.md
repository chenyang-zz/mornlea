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
- **THEN** both MUST receive equivalent rejection and neither MUST mutate world state

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
- **THEN** it MUST terminate within the declared bound, preserve failure reporting and release owned resources without dropping acknowledged durable work

### Requirement: Authority selection is reversible

An opt-in Rust deployment SHALL select exactly one server authority. Rollback MUST stop that authority before restoring the previous compatible runtime and data; default-client replacement remains a later release decision.

#### Scenario: Competing owner or incompatible rollback

- **GIVEN** a world is already owned by another server or its data is incompatible with the proposed rollback runtime
- **WHEN** activation or rollback is requested
- **THEN** it MUST fail explicitly rather than dual-write or silently downgrade the save
