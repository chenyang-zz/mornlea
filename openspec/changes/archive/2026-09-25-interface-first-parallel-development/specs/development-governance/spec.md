## ADDED Requirements

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
