## Purpose

Give future Rust-core/Godot-Python implementation plans one explicit, version-aware interface ownership map without presenting unimplemented APIs as current runtime behavior.

## ADDED Requirements

### Requirement: Target-oriented tasks resolve a single boundary owner

A target-runtime task that crosses a server, client-core, Godot, Agent, storage or tooling boundary SHALL cite the target interface catalog row and identify its producer, consumer, dependency direction and implementation state. A new shared boundary MUST acquire a reviewed owner and compile-ready landing before independently developed consumers start.

#### Scenario: Two features need one new client publication

- **GIVEN** terrain and actor tasks need a new presentation frame field
- **WHEN** their work is scheduled
- **THEN** the client-core presentation owner MUST land and accept the shared declaration and validated examples first
- **AND** the feature tasks MUST use that accepted identity rather than edit the common frame independently

#### Scenario: A private helper is proposed

- **GIVEN** a helper is used only inside one feature task
- **WHEN** its plan is reviewed
- **THEN** the helper MAY remain private without a new shared contract landing

### Requirement: Authority and publication metadata have one source

The target map SHALL assign server session/tick/arrival metadata and world mutation to server ingress and tick ownership, and client epoch/revision/frame assembly to client core. Protocol conversion and presentation MUST NOT fabricate authoritative metadata or create a second state owner.

#### Scenario: Transport converts a sequenced intent

- **GIVEN** a decoded v45 play intent with a client sequence
- **WHEN** it enters the server
- **THEN** server ingress MUST assign live session, tick and arrival metadata before domain command ordering
- **AND** Memory and TCP MUST use the same admission and validation path

#### Scenario: A stale presentation batch arrives

- **GIVEN** a Godot batch carries an earlier client epoch or incompatible confirmed revision
- **WHEN** the bridge validates it
- **THEN** it MUST reject the whole batch and preserve the last valid publication

### Requirement: Planned families are versioned and disabled until integrated

The target map SHALL distinguish logical family names from numeric ABI descriptors. A required family MUST have a versioned schema, limits and actual producer/consumer mapping before activation; missing or incompatible requirements MUST fail explicitly. Existing wire, save and pilot ABI versions MUST remain unchanged by publishing the map.

#### Scenario: Symbolic audio requirement meets a numeric-only pilot table

- **GIVEN** a feature requires `audio-cues@1` but the bridge provides no numeric descriptor mapping for it
- **WHEN** activation is negotiated
- **THEN** the feature MUST remain disabled with a stable incompatibility reason
- **AND** no fake audio cue or Go fallback MAY satisfy the requirement

#### Scenario: Future additive family

- **GIVEN** a new feature needs a semantic family absent from the catalog
- **WHEN** its change is proposed
- **THEN** its owner, schema, compatibility effect, bounds, error cases, contract landing and integration tests MUST be added before parallel provider/consumer work

### Requirement: Planning evidence is not runtime acceptance

The catalog SHALL label existing and target-only surfaces. A planned signature, mock, screenshot or compiling declaration MUST NOT be reported as a real provider or end-to-end integration result.

#### Scenario: F2 server crate has not landed

- **GIVEN** the map describes `ServerCore::advance_tick` but no accepted server implementation SHA exists
- **WHEN** F2 readiness is reported
- **THEN** it MUST remain a target contract and F2 MUST retain its existing prerequisites and real replay/failure gates
