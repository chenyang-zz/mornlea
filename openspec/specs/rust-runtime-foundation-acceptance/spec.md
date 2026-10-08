# rust-runtime-foundation-acceptance Specification

## Purpose

Provide complete, source-bound foundation acceptance that exposes every coverage gap and distinguishes pure Rust contracts from transitional authority observations before runtime migration starts.

## Requirements

### Requirement: Final foundation acceptance rejects every coverage gap

Final acceptance SHALL evaluate every supported family/version from the live registries and frozen corpus. It MUST fail for any uncovered point, missing source binding, non-executed required consumer or changed expected observation. A successful rejection test MUST NOT be reported as successful complete acceptance.

#### Scenario: The remaining input family is uncovered

- **GIVEN** a supported input family has success evidence but no executed rejection evidence
- **WHEN** final foundation acceptance evaluates the corpus
- **THEN** it MUST fail and identify that family/version
- **AND** dependent runtime implementation MUST remain blocked

#### Scenario: All foundation families are accepted

- **GIVEN** every discovered family/version has complete source-bound executed evidence
- **WHEN** the integrated acceptance runs on one identified source baseline
- **THEN** it MUST report zero uncovered points and exactly the discovered supported coverage set
- **AND** retain command, case count, source and corpus identity for downstream use

### Requirement: Authority and domain evidence retain their actual owner

Input-ordering evidence SHALL distinguish pure ordering and validation from authoritative admission and world effects. Existing external authority cases MUST retain their actual producer; a pure Rust ordering result MUST NOT claim world or inventory outcomes. Rejection evidence MUST derive from an executed failure or discarded action, rather than a fabricated expected result or coverage exemption.

#### Scenario: A stale input has no effect

- **GIVEN** a registered session's last admitted sequence is zero and it submits a sequence-zero hotbar change
- **WHEN** the real transitional authority processes the recorded tick
- **THEN** the input MUST be discarded, selection and inventory MUST remain unchanged, and no placement outcome MUST be produced
- **AND** the rejection evidence MUST identify the external authority producer

#### Scenario: Pure ordering is mislabeled as authority

- **GIVEN** a consumer only sorts validated envelopes
- **WHEN** its result is offered as evidence for world or inventory effects
- **THEN** acceptance MUST refuse that ownership claim

### Requirement: Prerequisite acceptance remains reversible and scoped

Foundation acceptance SHALL preserve protocol, save, ABI, production authority and corpus evidence outside its named additions. Downstream implementation MUST consume the sealed source/corpus identity, and a later relevant source change MUST invalidate or revalidate that evidence.

#### Scenario: An unrelated corpus case is lost

- **GIVEN** an input-evidence candidate removes or changes an unrelated case
- **WHEN** the candidate is reconciled
- **THEN** publication MUST fail and retain the prior corpus

#### Scenario: A downstream baseline differs

- **GIVEN** a downstream checkout differs in a source bound by the sealed foundation evidence
- **WHEN** a runtime worker is dispatched
- **THEN** the controller MUST require new applicable validation before accepting that prerequisite
