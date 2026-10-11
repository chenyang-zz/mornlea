## Why

The final switch is a product-runtime cutover, not a launcher rename. It must retire Go real-time ownership only after the Rust/Godot/Python stack has survived two release cycles with proven rollback.

## What Changes

- Gate default startup on accepted F1–F3, P8–P13, two release cycles, and a recoverable previous release.
- Qualify native missing-runtime diagnostics, then retire the migration Bootstrap and pilot Go core from the product closure.
- Replace scoped optional-only enforcement with product enforcement and retire old runtime consumers in verified nodes.

## Scope and prerequisites

This is a planned, independently reversible production slice. Non-goals are new gameplay rules, expansion of Go real-time ownership, production GDScript, mobile/Web/console support, and unreviewed baseline updates. [F1](../archive/2026-10-08-rust-runtime-foundation-acceptance/proposal.md), [F2](../rust-authoritative-server/proposal.md), and [F3](../rust-client-core/proposal.md) provide the Rust contracts, sole authoritative server, and typed client-core bridge it consumes; their accepted ledger evidence is required before dependent implementation. Python remains Godot's feature language. This planning revision authorizes no runtime cutover, tracked baseline update, or version bump.

Non-goals: new gameplay, an unqualified Python runtime, save downgrade, mobile platforms, automatic baseline approval, or concurrent online Go/Rust authorities.

## Capabilities

### New Capabilities

- `godot-default-client-switch`: Switch the default product to Rust server/client-core and Godot/Python after two release cycles, with complete rollback and explicit transition retirement.

### Modified Capabilities

- `godot-client-pilot`: Scope existing pilot-only Go ownership, Bootstrap, and unchanged-default requirements to the transition, and define the accepted native-diagnostic Rust/Godot/Python cutover without weakening unrelated authority or isolation behavior.

## Impact

- Affected: default launch/build/release entry points, desktop native launcher, release manifests, Rust/Godot bridge linkage, pilot Go ABI consumers, scoped architecture audits, and current architecture/version documentation.
- **BREAKING**: after accepted cutover, the default product requires the qualified Godot/Python distribution instead of the old Go/Rust renderer stack. No protocol/save version change is assumed; compatibility evidence must prove it or require a separate explicit migration.
- ABI: renderer client ABI v19 and the pilot Go client-core ABI are distinct. Exclude both obsolete product dependencies only when their consumers are retired, while preserving the previous complete release.
- Concurrency/performance: one online authority; existing bounded queues and hard error gates remain. Numeric benchmarks are informational.
- Rollback: select the complete previous release and a compatible save backup, never partial binary restoration or online dual-write.
