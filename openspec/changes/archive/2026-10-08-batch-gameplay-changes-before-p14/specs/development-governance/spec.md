## ADDED Requirements

### Requirement: Gameplay rule changes are batched until the Go real-time authority is retired

Until the Go real-time authority is retired and gameplay no longer needs to stay in parity with Go (currently planned in P14, `godot-default-client-switch`), gameplay rule changes SHALL be grouped into a planned batch OpenSpec change rather than opened as individual changes. A blocking defect MAY be fixed in its own change. A defect is blocking only when it causes at least one of:

- a hang: the server crashes, a tick returns Internal, or the world stops so players must reconnect;
- item loss: items vanish or duplicate without cause, or saved state reads back inconsistently;
- lost progress: a save rolls back, or player effort is irrecoverably discarded in a commonly reached situation.

Feel issues, rule inconsistencies, and rare edge cases with a workaround are not blocking. The gameplay owner SHALL decide unclear cases, and the fix's pull request description MUST record the reason. Every gameplay change, batched or blocking, SHALL change the Go authority and the Rust port together and SHALL include Go/Rust parity tests.

#### Scenario: Non-blocking rule inconsistency is found

- **GIVEN** the Go real-time authority has not been retired
- **WHEN** a contributor finds a gameplay rule inconsistency that is not a blocking defect
- **THEN** it SHALL be recorded for the next gameplay batch change
- **AND** a standalone gameplay change MUST NOT be opened for it

#### Scenario: Blocking defect is found

- **GIVEN** the Go real-time authority has not been retired
- **WHEN** a defect causes a hang, item loss, or lost progress as defined above
- **THEN** it MAY be fixed in its own change without waiting for a batch
- **AND** the fix MUST change Go and Rust together with Go/Rust parity tests
- **AND** the pull request description MUST record why the defect is blocking

#### Scenario: Classification is unclear

- **GIVEN** a gameplay defect is reported before the Go real-time authority is retired
- **WHEN** contributors disagree whether the defect is blocking, or the reporter is unsure
- **THEN** the gameplay owner SHALL decide
- **AND** the fix's pull request description MUST record the decision reason

#### Scenario: Go real-time authority has been retired

- **GIVEN** this batching requirement is in force
- **WHEN** the Go real-time authority is retired and gameplay no longer needs to stay in parity with Go
- **THEN** this batching requirement SHALL no longer apply
- **AND** it MUST be removed or replaced by a follow-up change
