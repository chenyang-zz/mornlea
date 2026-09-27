## Planning status and prerequisites

This revision changes planning artifacts only. Runtime behavior, version identities, current producers, tracked images, and default startup do not change. The current Go server and Go pilot client-core remain transition implementations; final ownership follows the [target architecture](../../../docs/architecture-target.md). The [archived pilot design](../archive/2026-09-20-pilot-godot-client-migration/design.md) is historical evidence, not authority for new runtime ownership.

Implementation requires the relevant accepted F1, F2, and F3 exit evidence in their ledgers, including exact source SHA, fixture identities, command output, test discovery, and rollback decision. An existing proposal, checked planning status, text search, or optional-entry audit is not completion evidence. After a prerequisite is archived, resolve its ledger through its archive location and retain the accepted SHA. No task is complete merely because a test filter selected zero tests.

Interface baseline: the [target runtime interface map](../../../docs/runtime-interface-architecture.md) assigns C2/G1 and `actors@1`/`player-view@1` to this boundary. Before independent actor producer and scene work, accept exact family schemas and bridge mapping with tested identity reuse, despawn, epoch reset and capacity failure; the logical names do not enable the pilot ABI.

## Ownership and design decisions

Rust server/domain owns entity rules and outcomes. Rust client-core owns identities, epochs, accepted ordering, confirmed events, and semantic interpolation inputs. Python owns scenes, animation, pooled instances, and bounded effect application. Any hot native resource transformation stays in `mornlea_godot`; no actor feature parses packets or simulates projectile hits.

Split independently replaceable actor, effect, and viewmodel capabilities through the catalog rather than expanding `app/`. Register complete dependency sets before enabling a family. Preserve the stable root and migration-only Bootstrap. Reject a single all-actor state machine in Python because it recreates client-core ownership; reject enabling unsupported kinds with placeholder commands because presentation must not fabricate authority.

## Risks and migration

Pooled identities can accidentally survive reuse: include repeated despawn/reuse and reset callback tests. Animation timing may differ across producers: compare event ordering and bounded timing windows from fixed replay before human motion review. Each family stays disabled until its tests pass. A failed family rolls back independently; current Go code is only the frozen offline oracle.

### Visual evidence dependency

The visual task consumes the capture/report contract delivered by phase 2 of [production tooling](../godot-production-tooling/tasks.md), after F3. It does not wait for all tooling producer handoffs: tooling establishes the contract first, features provide parity evidence next, and each case is handed off afterward. This avoids a P8/P9/P10/P12 dependency cycle.

Evidence has three ordered layers: semantic replay, untracked candidate capture, and explicitly approved canonical handoff. Each case names its semantic class, current and candidate producer, source SHA, scenario/input identity, asset/runtime/platform identity, capture boundary, and limits. Same-producer pixel regression retains its existing thresholds; cross-producer parity requires semantic evidence and human review of declared differences. Missing, stale, empty, failed, or non-comparable required cases block handoff. A dummy headless renderer cannot provide GPU pixel evidence; GPU capture uses a qualified non-foreground, no-focus path or requires explicit manual acceptance. No renderer-specific tracked class is created.

## Validation and rollback discipline

Named new test targets and scripts in `tasks.md` are prospective interfaces, not claims that they exist today. Their first implementation task creates them, records nonzero discovery, and runs a failing behavioral case before implementation. Reuse passing evidence only for the same tested SHA. Performance values are informational; invalid identity, incomplete coverage, real overflow, data loss, and I/O failures are hard errors. Automated validation must not launch or focus a foreground game window.

Commit each independently verified implementation node with its scoped evidence before starting the next node. Update affected directory `AGENTS.md` when ownership changes; otherwise record inherited guidance. Closeout includes formatting, Rust checks, six-module vet, all six Go modules under `make test-race`, and strict OpenSpec validation. Review durable architecture findings at the end of each round; record `Architecture skill: no change` when no new verified rule qualifies.

## Post-numerical-closure planning decisions

The [node dependency and ownership register](plans/03-parallel-readiness.md) and [refined node decisions](plans/04-refined-nodes.md) are normative execution inputs alongside the original packets. Preserve prior scope and separate source-contract, real-provider and real-integration acceptance. Numerical completion does not supply complete F1 acceptance; use the independently owned [foundation successor](../rust-runtime-foundation-acceptance/proposal.md).

The controller compared wholesale migration resequencing, immediate parallel dispatch from prospective signatures, and bounded refinement of existing nodes. Bounded refinement preserves reviewed scope and existing node identities while making dependencies, shared edits and source-information limits decidable. Immediate dispatch remains blocked by missing accepted contract/provider SHAs. Broad shared files stay serial; only disjoint providers with accepted predecessors can overlap.

The actor source matrix in [refined decisions](plans/04-refined-nodes.md) governs supported presentation. Attack/lure/projectile-hit/task correlations absent from the accepted source cannot be synthesized as confirmed outcomes. Effects consume actual observation/cue provenance; tags require a source name.
