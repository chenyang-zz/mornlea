# Independent F3 provider and capability acceptance review

What this change does: The Rust client owns session, mirror, prediction, preparation and ten semantic families. The actual-provider qualification connects that same core through the Godot arena to the Rust server's common Memory/TCP adapters; the latest repair physically retires the client socket before resetting the same player.

Reviewer: Codex `/root/review_provider`, read-only independent reviewer.
Exact candidate: `24f57d73c1e1c2132aa5ad3055767fec2628ca35`.
Detached clean checkout: `/private/tmp/mornlea-f3-r6-provider-qualification-review`.
Isolated Cargo target: `/private/tmp/mornlea-f3-completion-provider-cargo`.
Toolchain: explicitly pinned `rustup run 1.97.1`; Cargo uses `--locked`. Subsequent builds use `CARGO_BUILD_JOBS=2`.

**Verdict: accepted within the assigned real-provider/inventory source scope. No reproduced source blocker or missing provider-family case was found.** This supports the library portion of original 3.4 and inventory portion of 4.1. Native marshalling/release, native100, full stage gates and the complete seven-criterion verdict remain controller/sibling-owned and are not accepted by this report alone.

## Inspected scope and real ownership

Read root/scoped Rust guidance, OpenSpec configuration, F3 proposal, both deltas, design, tasks, qualification packet17 and the seven original criteria retained in the repair handoff. Read the entire `tests/rust_server_integration.rs` (1122 lines), entire shared `tests/support/rust_server.rs` (1500 lines), disposable TCP example, both retirement commits and their connected socket paths. Traced production `CoreArena`, G1 rendering/validation, session source effects and publication, all six actor projectors/assembler, inventory/container/crafting/furnace, environment/survival/chat/task/prompt, player, audio and diagnostic providers, and native preparation. Source-equivalent prior reports were used only after the connected source inspection; fresh results below do not depend on their claimed passing counts.

Ordinary observations originate `AuthorityState::advance_tick` through actual `LoginDriver::bind`, real codec/outbox ownership and actual Memory/TCP transport. `EndpointTap` observes successful operations while forwarding their results. `ObservedCore` delegates to the actual `ClientCore`; `CoreArena::open_with_endpoint` exercises the same slot/token/pull/render/release ownership. The separately named injected-frame test replaces a snapshot solely for malformed/capacity refusal. It is not an ordinary server observation producer.

Explicit substitutions are `ImmediateLoad` for stored-player persistence, staged disposable chunk/container/actor facts between ticks, and deterministic external planner completion. The remote player is independently admitted through the real Memory login path. No source packet is synthesized to impersonate a server observation, and no production server executable, disk-load or live planner claim follows from this fixture.

Memory sends complete controls to the real `MemoryTransport`, retains undelivered suffixes, transfers received controls once and acknowledges delivered/staged controls only. Its test-only `F2Memory` wrapper translates the real captured connection's terminal phase into existing `Closed(Disconnected)` and fences the captured ticket generation. It never invents Disconnect bytes. Retired feed/close evidence cannot target the replacement generation.

TCP records newly committed activations during the exact connection's pump/poll/flush operation. The bare-probe/two-client case proves ownership is not inferred from session enumeration. The same authority owner is moved into and out of the TCP fixture for pumping, not copied into a second authority.

## Actual family and lifecycle coverage

The Memory and TCP all-family cases require ten nonempty families across the admitted transcript, rather than counting empty descriptors. They require all six actor kinds, complete inventory/hotbar/chest/crafting/furnace and environment/survival/chat/task/derived prompt records, meaningful input/lifecycle/diagnostics/audio, and resolvable native prepared terrain. The six-actor removal cases exercise actual departure publications and ForgetChunks with its absent source tick. The placement case compares equal actual Memory/TCP TickPublications and checks authoritative block content/revisions and prepared replacement terrain.

Source-value assertions compare complete typed inventory/container/crafting/furnace and chat/task values, player pose/mining, environment/survival, and actor source IDs/selected pose/angle/tick/optional fields against consumed actual outboxes. Both correction/audio cases require an actual snow block at the predicted foot, sequenced authoritative ingress, correction of sequence1 followed by pending replay, no repeated predicted stride cue, and confirmed provenance. Confirmation assertions deduplicate publications by `(epoch, frame_index)`, preserve exact sequence/kind/headers/source ticks and attest actual consumed PlayerState or PlaceBlockSucceeded. Placement confirmation has no invented source tick; chat cannot acquire an authority sequence acknowledgment.

Whole-batch capacity and invalid mixed input preserve sequence/visible state. Explicit malformed, duplicate-family, capacity+one and stale pulls retain G1's last valid owned copy. Repeated arena close releases once. Terrain reset checks that retained prepared keys become stale and retire with the epoch.

The latest physical retirement fix is `05e1d1025`, with actual-provider reset coverage in `24f57d73c`. In `src/session/io.rs:596`, close holds connector ownership, obtains socket then state locks, performs `shutdown(Both)` on the same `Arc<TcpStream>` descriptor before clearing queues/removing the ticket, and retains ownership on a non-NotConnected shutdown failure. `bind_socket` at647 refuses late binding after close or connector cancellation. Worker reads/writes remain outside these locks; no worker join or reconnect delay was added.

`Rig::reset` explicitly invokes server-side close only for Memory's declared peer pairing. TCP reset relies on client EOF. `actual_tcp_repeated_same_player_reset_requires_client_socket_retirement` at integration line648 passes32 actual reset/readmission cycles with the same player, fresh authoritative sessions, stale old epochs, terrain retirement, first sequence1 and normal terminal refusal. It does not retry a LoginRejected, change player IDs or close the TCP server peer as a reset shortcut.

## Fresh commands and execution counts

All commands ran in the detached checkout above. Cargo prefix is `CARGO_BUILD_JOBS=2 CARGO_TARGET_DIR=/private/tmp/mornlea-f3-completion-provider-cargo rustup run 1.97.1 cargo test --manifest-path packages/engine/Cargo.toml`. The first discovery compile started before the requested two-job cap; subsequent commands apply it.

| Command suffix/check | Exit | Actual result | Evidence |
| --- | --- | --- | --- |
| `-p mornlea_godot --features runtime-qualification --test rust_server_integration --test rust_producer --locked` | 0 | G1 producer42 passed; integration40 passed, no ignored/filtered cases | `/private/tmp/mornlea-f3-completion-provider-tests.log` |
| Integration target `-- --list` | 0 | 40 discovered | `/private/tmp/mornlea-f3-completion-provider-list.log` |
| Core four named targets `-- --list` | 0 | lifecycle10, prediction48, presentation232, session86 discovered | `/private/tmp/mornlea-f3-completion-provider-core-list.log` |
| G1/real-provider targets with qualification feature `-- --list` | 0 | 42 and40 discovered | `/private/tmp/mornlea-f3-completion-provider-godot-list.log` |
| `-p mornlea_protocol --test runtime_contract --locked -- --list` | 0 | 134 discovered | `/private/tmp/mornlea-f3-completion-provider-protocol-list.log` |
| `python3 /private/tmp/mornlea-f3-completion-provider-inventory.py` | 0 | 67 rows/139 exact Cargo-discovered references, historical preservation and corpus checks pass | `/private/tmp/mornlea-f3-completion-provider-inventory.log` |
| `-p mornlea_server --test server_contract --locked agent_host::invalid_terminal_releases_only_its_current_gate -- --exact` | 0 | 1 selected passed;312 filtered | `/private/tmp/mornlea-f3-completion-provider-stale-agent.log` |
| `-p mornlea_server --test server_contract --locked runtime_companion::bootstrap_save_failure_stops_startup_before_agent_and_authority -- --exact` | 0 | 1 selected passed;312 filtered | `/private/tmp/mornlea-f3-completion-provider-failed-save.log` |
| `git diff --check 908a099282a1ad35736d0ce7ba17abcb089299a8 HEAD` | 0 | clean | command output in review session |

Integration40 comprises16 actual-provider scenarios, one explicitly injected malformed/capacity-frame scenario in an actual admitted rig, and23 embedded ABI/negotiation tests. It is not40 distinct live-server scenarios. The long32-cycle TCP regression passed; complete integration execution took200.55s. Discovery counts are not asserted as execution of those other targets; their full runtime acceptance is supplied by the separate core/full-stage owners.

A mistaken preliminary command used feature `qualification`; Cargo correctly returned101 because the package feature is named `runtime-qualification`. It executed zero tests. The corrected commands above passed and the wrong invocation is not acceptance evidence.

The fresh build's `mornlea_client_core` build output records `MORNLEA_CORE_SOURCE_SHA=24f57d73c1e1c2132aa5ad3055767fec2628ca35`; the isolated target did not carry a predecessor diagnostic identity.

## Existing failed-save and stale-Agent proof

These are fresh exact-source supplementary failure-injection checks required by packet17; they are separate from ordinary authority-provider output. They use canonical `TMPDIR=/private/tmp` and both Agent executable overrides pointing at `/Users/chen/work/mornlea/packages/agent/.venv/bin/python`.

`runtime_companion::bootstrap_save_failure_stops_startup_before_agent_and_authority` at source line598 runs actual `DiskStore::with_io`, actual configured startup and actual payload writing. Its explicit TempSync/Before fault returns PermissionDenied. The test requires typed `Save(Io { SyncPayload, PermissionDenied })`, no companion persistence enabled, no companion actor and no constructed Agent identity. It proves refusal before Agent/authority startup, not an F3 live-session disk save or complete crash/restart qualification.

`agent_host::invalid_terminal_releases_only_its_current_gate` at source line398 runs actual `PlanHost` and `SnapshotRegistry` with a scripted AgentHandle. A foreign response releases only its own gate; a later injected generation8 response to generation7 installs as `InstallReject::StaleIdentities`, leaving the other companion in flight. It proves actual host identity fencing under explicit stale-response injection, not live Agent HTTP/model execution.

Source hashes at this candidate:

- `tests/server_contract/runtime_companion.rs`: `7a923d533f19b2e2a1e562daca9bce44d781810c54e5e4ee4e8f5323feb64833`
- `tests/server_contract/agent_host.rs`: `6fb82da9c4034ac7a714ad8d4ba498f77fe5e6cabdce5ed50d7c0baddc7af0ab`
- `src/runtime/companion.rs`: `89f3b4b5dd7e2cff150ef90e524720cbc9e4cdda4829c987466b6ee0392516d1`
- `src/agent/host.rs`: `2377675e6a3d8aebb00e878d012f45f28548ba1122313a9b2c086721b33ba75b`

The complete server/protocol/storage trees and capability inventory are byte-identical to round-five source `908a099282a1ad35736d0ce7ba17abcb089299a8`; that carry comparison exited0. The current failure cases were nevertheless freshly run.

## Inventory and original acceptance reconciliation

The inventory has10 families,20 actions,36 packet variants,28 Go oracle rows,41 limit rows and67 unique flattened rows. All groups/rows have their declared Rust test references. All139 unique `crate::registered_target::exact_test_name` links match actual Cargo discovery in their correct target; none is missing or points only to a same-named function in a different target. Existing source fixtures, IDs, versions, measurements and selectors remain intact.

The inventory seal is preserved against round five and historical `f58a86d39`; stripping only current Rust reference fields and appended notes leaves identical history. The seal's corpus hash is checked against its exact `c8c049465b99df662c2916aa60bbe17a90f1013a` baseline. Relative to that sealed corpus, current data differs only at12 already-reviewed source-provenance SHA256 leaves; each matches its current actual source file. All1434 cases/expectations, identities, version values and source_revision remain identical. Historical values are not overwritten or represented as freshly measured limits.

Original seven criteria retain their scope:

| Original criterion | Evidence contributed here / remaining owner |
| --- | --- |
| Client state has one Rust owner | Actual AuthorityState → transport → ClientCore → CoreArena traced; ordinary observations and source correlations executed |
| Frame publication is atomic and bounded | Actual mixed/capacity input plus injected bad-frame preservation and G1 producer42 pass |
| Reset and bridge lifecycle release ownership | Real Memory/TCP reset and same-player32/socket retirement pass; native release proof belongs to native reviewer/controller |
| Rust core parity is independent of rendering | Both real providers and exact placement publication parity execute without a renderer; offline Go and complete native-process evidence belong to other assigned owners |
| Migration preserves typed host isolation | G1 source/production dependency boundary reviewed; Python/native isolation reviewed separately |
| Cross-runtime boundaries publish only bounded atomic snapshots | G1 owned value/validation and previous-copy preservation execute; actual native conversion remains separately owned |
| A Rust-mode host tick distributes one complete semantic frame | Complete ten-family arena publication executes; Python host tick/fanout and native proof reviewed separately |

No original criterion is removed or narrowed. Original3.4's real F2 library/C1/C2/G1 provider requirement has passing actual cases here; full native integration cannot be inferred. Original4.1's inventory/reference/history reconciliation passes here; a blanket zero-gap execution verdict additionally requires the independent core/native and full-stage evidence. `tasks.md` is correctly still pending at this source until those final controller gates are reconciled.

## Findings and limits

No Must fix or Should fix source finding was reproduced in the assigned provider/inventory scope. No product code, source test, fixture, dependency, default or version was edited; no commit, remote write, Godot process or foreground window was launched. The reused exact-SHA worktree stayed clean and HEAD was reread at the end. Evidence manifest is `/private/tmp/mornlea-f3-completion-provider-evidence.json`, including exact source, command logs and checker/report checksums. Architecture skill: no change.

Not checked here: native100/derivative/native marshalling and full global gates; the controller owns them. Risk: this scoped passing review alone cannot authorize a full F3 acceptance claim or merge.
