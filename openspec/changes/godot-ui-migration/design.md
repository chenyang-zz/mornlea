## Planning status and prerequisites

This revision changes planning artifacts only. Runtime behavior, version identities, current producers, tracked images, and default startup do not change. The current Go server and Go pilot client-core remain transition implementations; final ownership follows the [target architecture](../../../docs/architecture-target.md). The [archived pilot design](../archive/2026-09-20-pilot-godot-client-migration/design.md) is historical evidence, not authority for new runtime ownership.

Implementation requires the relevant accepted F1, F2, and F3 exit evidence in their ledgers, including exact source SHA, fixture identities, command output, test discovery, and rollback decision. An existing proposal, checked planning status, text search, or optional-entry audit is not completion evidence. After a prerequisite is archived, resolve its ledger through its archive location and retain the accepted SHA. No task is complete merely because a test filter selected zero tests.

## Ownership and design decisions

Rust client-core is the UI data owner. It publishes typed views and validates intent tokens, command order, and confirmed outcomes. Embedded Python maps those views to Godot Control trees, focus state, and bounded event routing. Inventory/container state is never reconstructed in Python from wire bytes. Capture fixtures use the same typed views as runtime presentation.

Godot Control is selected, so there is no open WebView-versus-Control decision. Retain React/WebView only to characterize existing behavior offline and to run the previous release. Reject extending WebView because it expands a non-target product owner; reject production GDScript because embedded Python is the final feature language.

### Container UI execution slice

The independently reviewable container slice is specified in [the container-controls worker plan](plans/container-controls.md). It covers personal inventory/crafting, workbench, chest, and furnace; it does not claim completion of menus, character presentation, HUD, or the whole P10 change. The current `features/containers/feature.tres` reservation becomes the owner of these panels after its own parity and catalog gates. The existing `features/ui/` HUD remains a separate feature. This follows the existing host's scene-path feature lifecycle and lets the two panel groups develop in separate files after their shared Rust view/intent contract is accepted.

The Rust client-core publishes `ContainerViewV1` in one coherent frame and owns token issuance, selected source, recipe selection, command mapping, confirmed mirrors, and rejection. The Godot bridge projects it as `container_ui` and accepts `submit_container_intent_v1(Dictionary) -> int`; Python renders fixed Control slots and forwards semantic area/index intents without deriving counts, container identities, or network commands. The plan freezes field names, limits, operation rules, failure precedence, producer/consumer types, icon asset paths, and test cases. The F3 bridge is a prerequisite producer; if its accepted public API differs from this projection, reconcile F3 and P10 artifacts before dispatch rather than building a second Python adapter or guessing a mapping.

Inventory/workbench and chest/furnace are parallel after the Rust contract and shared slot Control are verified. A separate asset-generator node can run alongside either panel group because it owns only generated icon production. The controller serially integrates the scenes, catalog, lifecycle and full transcript evidence after those nodes. No worker edits another worker's files or the original protocol-completion checkout.

This slice adds no save or network wire version. It preserves the server-authoritative stack rules and bounded client queue; an accepted intent is not a confirmed move. The current pilot catalog retains the disabled comparison feature. A separate container candidate catalog marks the new feature required and fails activation when its Rust producer, embedded Python runtime, deterministic icon catalog, or semantic replay is unavailable; this catalog does not switch the default app root. Visual evidence and any canonical producer handoff remain governed by P12 and task 3.2.

## Risks and migration

UI appearance can look correct while stale-token behavior is wrong: intent/view transcript parity precedes pixel review. Font/layout differences need explicit fixture review; no threshold increase is permitted. Enumerate the supported UI surfaces and actions in the fixture manifest before coding, including failure/reset/focus/resize states. Enable surfaces only after complete behavior coverage and disable them by catalog on failure.

### Visual evidence dependency

The visual task consumes the capture/report contract delivered by phase 2 of [production tooling](../godot-production-tooling/tasks.md), after F3. It does not wait for all tooling producer handoffs: tooling establishes the contract first, features provide parity evidence next, and each case is handed off afterward. This avoids a P8/P9/P10/P12 dependency cycle.

Evidence has three ordered layers: semantic replay, untracked candidate capture, and explicitly approved canonical handoff. Each case names its semantic class, current and candidate producer, source SHA, scenario/input identity, asset/runtime/platform identity, capture boundary, and limits. Same-producer pixel regression retains its existing thresholds; cross-producer parity requires semantic evidence and human review of declared differences. Missing, stale, empty, failed, or non-comparable required cases block handoff. A dummy headless renderer cannot provide GPU pixel evidence; GPU capture uses a qualified non-foreground, no-focus path or requires explicit manual acceptance. No renderer-specific tracked class is created.

## Validation and rollback discipline

Named new test targets and scripts in `tasks.md` are prospective interfaces, not claims that they exist today. Their first implementation task creates them, records nonzero discovery, and runs a failing behavioral case before implementation. Reuse passing evidence only for the same tested SHA. Performance values are informational; invalid identity, incomplete coverage, real overflow, data loss, and I/O failures are hard errors. Automated validation must not launch or focus a foreground game window.

Commit each independently verified implementation node with its scoped evidence before starting the next node. Update affected directory `AGENTS.md` when ownership changes; otherwise record inherited guidance. Closeout includes formatting, Rust checks, six-module vet, all six Go modules under `make test-race`, and strict OpenSpec validation. Review durable architecture findings at the end of each round; record `Architecture skill: no change` when no new verified rule qualifies.
