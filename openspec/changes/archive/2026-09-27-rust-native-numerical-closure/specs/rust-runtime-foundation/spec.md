## ADDED Requirements

### Requirement: Numerical operations are callable through validated Rust values

The Rust numerical owner SHALL expose typed calls for collision, physics, ray traversal, world chunk generation, world probes, runtime trees, LOD, fluid evaluation, fluid rescan, mesh/light, and pathfinding. Native callers MUST NOT construct an engine ABI byte request to invoke these operations. Invalid values MUST return a typed failure before an algorithm can observe them.

#### Scenario: A native caller supplies a valid request

- **GIVEN** a supported numerical request represented as typed Rust values
- **WHEN** a Rust runtime calls the corresponding operation
- **THEN** it receives the same successful numerical observation as the established compatibility oracle without serializing an ABI request

#### Scenario: A native caller supplies a malformed request

- **GIVEN** a nonfinite scalar, invalid extent, unknown mode, or malformed registry appropriate to one operation
- **WHEN** the native operation is called
- **THEN** it returns the documented typed failure and publishes no result

### Requirement: Numerical work and publication are bounded

Each native operation SHALL enforce a declared count, byte, or work budget before unbounded allocation or execution. Caller-owned scratch MUST be exclusive for a call, reusable after success or failure, and MUST NOT leak a partial result into caller output. A short destination MUST report the required capacity in the operation's declared units.

#### Scenario: Capacity boundary

- **GIVEN** a valid request and a destination one element below the required length
- **WHEN** the operation runs
- **THEN** it reports the exact required element count and leaves the destination unchanged

#### Scenario: Scratch reuse after failure

- **GIVEN** a warmed scratch object used for a successful call and then for a rejected call
- **WHEN** it is reused for another valid request
- **THEN** the last result equals a fresh-scratch result and no prior observation is published

### Requirement: Native pathfinding preserves deterministic choices

Pathfinding SHALL read an immutable bounded block snapshot, use injected passability, and return an owned path with normalized revision identity. It MUST preserve the supported Go movement, cost, ordering, tie, and 4096-expansion semantics. Invalid grids, unreachable paths, and budget exhaustion MUST remain distinguishable.

#### Scenario: Two legal transitions have equal cost

- **GIVEN** a grid where movement choices tie under the supported expansion order
- **WHEN** Go and Rust evaluate the same start, goal, passability, and revisions offline
- **THEN** the exact waypoint and revision sequences agree

#### Scenario: Goal crosses the expansion limit

- **GIVEN** otherwise identical corridors whose goals require the 4096th and 4097th expansions
- **WHEN** each is searched
- **THEN** the former can succeed and the latter returns budget exhaustion without a partial path

### Requirement: The existing engine ABI remains compatible

Engine ABI v11 symbols, accepted layouts, valid-input values, status meanings, and capacity units SHALL remain compatible while ABI adapters and native Rust calls share the numerical algorithm. Rejected pointer aliases and late failures MUST NOT publish partial payloads.

#### Scenario: Valid ABI request

- **GIVEN** a current Go caller sends a valid ABI v11 numerical request
- **WHEN** the adapted Rust engine executes it
- **THEN** the status and output bytes match the frozen compatibility case

#### Scenario: Rejected aliased metadata

- **GIVEN** an ABI output metadata pointer aliases an input, scratch, or output buffer
- **WHEN** the engine rejects the request
- **THEN** it leaves every aliased byte unchanged and returns the existing failure status

### Requirement: Numerical acceptance uses executed source-bound evidence

Every one of the eleven numerical inventory families MUST have nonempty Go-produced and Rust-consumed cases covering success, invalid input, boundaries, and result ordering where applicable. The acceptance gate MUST reject a missing family, missing compiled consumer, changed scalar, changed fixture digest, or unexecuted case; an inventory name alone MUST NOT count as coverage.

#### Scenario: One family is not executed

- **GIVEN** a frozen inventory entry with no matching Rust operation execution
- **WHEN** numerical closure is checked
- **THEN** the gate reports that family as uncovered and fails

#### Scenario: An expected result is altered

- **GIVEN** a valid corpus case whose expected scalar or path waypoint is mutated without changing its input
- **WHEN** the independent Rust consumer executes it
- **THEN** comparison fails for that case even if all manifest files and family names exist
