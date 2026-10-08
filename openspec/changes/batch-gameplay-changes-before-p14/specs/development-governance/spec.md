## ADDED Requirements

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
