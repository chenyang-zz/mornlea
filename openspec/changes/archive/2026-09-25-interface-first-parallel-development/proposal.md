## Why

Mornlea already requires exact worker contracts, but it does not state when a shared interface must land before parallel tasks start or what evidence freezes that interface. A file-ownership split alone can leave workers guessing types, changing common files concurrently, or treating test doubles as proof of integrated behavior.

## What Changes

- Require an interface-first dependency cut when several independently reviewable tasks consume the same new or materially changed boundary.
- Define a compile-ready contract landing, task readiness and exclusive ownership rules, provider and consumer conformance evidence, integration gates, and a controller-owned contract-change path.
- Publish a reusable bilingual guide and align root guidance, OpenSpec planning rules, and the synchronized project orchestration skill with it.

## Capabilities

### New Capabilities

None.

### Modified Capabilities

- `development-governance`: Make interface-first planning and evidence a decidable prerequisite for parallel implementation where tasks share a changing contract.

## Impact

- Affected documents and configuration: `AGENTS.md`, `openspec/config.yaml`, the two `mornlea-implementation-orchestration` skill copies and references, `docs/development-process*`, a new bilingual interface-first guide, `docs/README*`, and `docs/documentation-manifest.json`.
- User outcome: future task plans identify which work can start from one accepted contract SHA and which work must wait for serial integration; workers can implement against stable types without reading peer internals.
- Compatibility, saves, protocol, ABIs, and runtime behavior: unchanged. Thread and agent concurrency limits remain unchanged. Planning adds a readiness check, with no runtime or performance cost.
- Non-goals: requiring an interface layer for every small task, making dependent work concurrent, changing provider or subagent policy, implementing the planned Rust numerical interface, or accepting a change merely because its doubles compile.
- Rollback: revert this governance unit and its mirrored documentation; existing implementation and OpenSpec task evidence remain intact.
