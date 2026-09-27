## Purpose

Provide Rust-owned client sessions, mirrors, prediction and immutable typed presentation while preserving Godot feature isolation and legacy replay parity.

## ADDED Requirements

### Requirement: Client state has one Rust owner

The client core SHALL own login, packet processing, mirrors, reversible prediction, reconciliation and publication budgets. The presentation host MUST consume typed semantic values and submit typed intent without decoding packets, reading saves or owning a second mirror.

#### Scenario: Confirmed correction

- **GIVEN** predicted movement disagrees with a confirmed server observation
- **WHEN** the Rust core applies the correction and replays pending input
- **THEN** its mirror and presentation output MUST match the recorded compatibility contract
- **AND** Python MUST receive a semantic result without implementing reconciliation

#### Scenario: Host submits invalid intent

- **GIVEN** malformed or out-of-range semantic intent reaches the bridge
- **WHEN** the core validates the batch
- **THEN** it MUST reject the invalid batch without partial input publication or direct world mutation

### Requirement: Frame publication is atomic and bounded

A presentation frame SHALL represent one coherent session epoch and confirmed revision. Capacity, ordering and compatibility validation MUST happen before publication. Unsupported required families MUST fail explicitly; stale data MUST NOT become a newer frame.

#### Scenario: Mixed or stale revision

- **GIVEN** records from a prior epoch or different frame revision
- **WHEN** a frame is assembled or applied
- **THEN** it MUST reject the inconsistent publication and retain the last valid state

#### Scenario: Extreme accepted drop coordinates

- **GIVEN** a protocol-valid DropId with chunk x equal to i32::MAX and block index zero
- **WHEN** C2 projects its location
- **THEN** it MUST preserve the identity and publish exact finite f64 x34359738352
- **AND** it MUST NOT narrow through an overflowing i32 world coordinate or reject the valid observation
- **AND** an index98304 MUST fail frame validation without changing the previous frame

#### Scenario: Capacity exhaustion

- **GIVEN** a bounded input or output batch exceeds capacity
- **WHEN** publication is attempted
- **THEN** the core MUST report overflow and preserve the declared atomicity and retry/drop rules without silent truncation

### Requirement: Reset and bridge lifecycle release ownership

Disconnect, reconnect, reset and teardown SHALL invalidate prior session publications, clear presentation ownership and release native resources in a repeatable order. Panics or invalid transitions MUST become stable boundary failures.

#### Scenario: Repeated teardown with queued work

- **GIVEN** a session has pending network, mesh and presentation work
- **WHEN** reset and teardown are repeatedly exercised
- **THEN** no stale frame, resource leak, use-after-free or duplicate event MUST survive into the next session

### Requirement: Rust core parity is independent of rendering

Acceptance SHALL include offline Go transcript comparison and Rust-server integration for both local and remote semantics, without requiring a graphical renderer. A successful pilot screenshot MUST NOT substitute for protocol, prediction, failure-path or lifecycle evidence.

#### Scenario: Renderer unavailable

- **GIVEN** Godot graphics and audio devices are unavailable
- **WHEN** core contract and replay tests run
- **THEN** session, correction, ordering and bounded publication tests MUST still execute

### Requirement: Migration preserves typed host isolation

The qualified Godot Python host SHALL retain typed intent/view interfaces while the underlying core changes owner. Production GDScript, raw ABI access from Python and new Go client-core behavior MUST NOT be required. Visual producer handoff and default-client switch remain independently gated.

#### Scenario: Unsupported required family

- **GIVEN** a selected feature requires an unavailable or incompatible semantic family
- **WHEN** the host negotiates feature activation
- **THEN** activation MUST fail with a stable reason rather than fabricate state or fall back to a Go feature implementation
