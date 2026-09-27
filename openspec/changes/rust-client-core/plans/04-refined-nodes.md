# Refined node decisions

Unsplit nodes retain the exact files, APIs, algorithms, red/green cases, commands and rollback in the existing worker packet. The dependency register adds readiness requirements and does not grant edits to shared files.

These replace parents `3.1` and `3.3`. Read [supporting values](05-supporting-values.md) and [predecessors](03-parallel-readiness.md). Controller owns all module/test registrations.

## Node 3.1a: Descriptor table

Editable: `packages/engine/crates/mornlea_godot/src/feature_negotiation.rs`, `packages/engine/crates/mornlea_godot/tests/family_registry.rs`, `apps/mornlea-godot/catalog/capability_registry.tres`, `scripts/godot/capability_registry_check.py`. Read-only: accepted C2 records/limits, pilot 1..8 table, feature implementations. Exclude loader, bridge methods, host and enabling catalog.

Assign the ten Rust-producer IDs 1..10 in session,input,terrain,actors,player-view,inventory-ui,world-ui,audio-cues,lifecycle,diagnostics order, major 1/minor 0. Producer identity `rust-client-core` distinguishes this table from pilot numeric IDs. Resolve symbolic requirement before instantiation; reject duplicate/unknown key, duplicate numeric ID within producer, wrong major or unsupported required minor. Descriptor limits come from accepted C2, never pilot entity capacity seven. Tests `family_registry::all_ten_keys`, `family_registry::producer_scoped_ids`, `family_registry::missing_and_version_fail_closed`; run Rust Godot tests, `make godot-capability-check`, focused audits. Commit table/catalog together; rollback restores them without changing the pilot table.

## Node 3.1b: Safe core adapter

Editable: `packages/engine/crates/mornlea_godot/src/{abi,client_core,bridge,lib}.rs`, `packages/engine/crates/mornlea_godot/tests/rust_producer.rs`; braces expand literally. Use safe accepted C1/C2 and 3.1a table. Rust mode never loads Go core/old renderer; pilot rollback is explicit. Exact arguments/results and owned dictionary conversion are frozen in [06-godot-facade.md](06-godot-facade.md). New methods `open_core`, `connect`, `submit_typed_input`, `step`, `pull_typed_frame`, `family_table`, `reset`, `close` replace host usage serially in 3.2; old `session_*` methods remain pilot-only. No silent contract alias.

Copy complete validated frames to owned Godot values, retain prior visible state on error, map caught panic to Internal. Tests `rust_producer::rust_mode_does_not_load_go`, `rust_producer::whole_frame_failure_atomic`, `rust_producer::method_table_and_owned_values` execute actual core/adapter. Run Rust Godot tests, capability check and `go test ./packages/audit -run 'Architecture|Godot' -count=1`. Commit only named adapter files; host/catalog enabling is excluded. Rollback selects the complete prior adapter.

## Node 3.3a: Core lifecycle

Editable: `packages/engine/crates/mornlea_client_core/src/session/lifecycle.rs`, `packages/engine/crates/mornlea_client_core/tests/lifecycle_contract/lifecycle.rs`; predeclare at 1.2. Godot/Python/devices are read-only. Invalidate input, prediction, preparation and handles before next epoch; close is idempotent. Tests `lifecycle::old_work_cannot_cross_reset`, `lifecycle::pending_login_reset`, `lifecycle::reconnect_after_terminal`, `lifecycle::one_hundred_cycles` use deterministic clock/transport and assert zero retained work. Run `... -p mornlea_client_core --test lifecycle_contract --locked` and `go test ./packages/client/runtime -run 'Predict|Lifecycle|Step' -count=1`. Commit the pure-core provider; no real Godot process acceptance is claimed.

## Node 3.3b: Bridge release order

Editable: `packages/engine/crates/mornlea_godot/src/lifecycle.rs`, `packages/engine/crates/mornlea_godot/tests/lifecycle_contract.rs`, `apps/mornlea-godot/tests/scripts/rust_bridge_lifecycle_check.py`. Host wiring from 3.2 is read-only. Release Python feature consumers before core/native resources, ignore old-generation callbacks, contain invalid transition/panic. Tests `lifecycle_contract::consumers_before_core`, `lifecycle_contract::late_callback_ignored`, `lifecycle_contract::native_panic_is_internal` assert exact release trace and zero retained handles. Run Rust Godot tests, Python check and rebuilt real bridge harness. P11 device and P13 child ownership remain separate; rollback includes only bridge lifecycle and tests.

## Node 3.3c: Actual rebuilt process cycles

Editable: `scripts/godot/smoke.sh`, `apps/mornlea-godot/tests/scripts/rust_session_smoke_check.py`, `ledger.md`. Add prospective `--core rust --exercise-session` flags; rebuild/bind artifacts, reject stale/Go producer and startup-only pass. Exercise live create/login/reset/reconnect/destroy with queued input/mesh across 100 isolated headless process cycles; record source, binary, Python runtime hashes, nonzero cases and release traces. Run `scripts/godot/smoke.sh --iterations 100 --isolated-python --core rust --exercise-session` and the full Rust lifecycle target. These flags are unavailable before this node. No foreground window. Commit harness/evidence; real F2 family integration remains 3.4.

## Oracle corrections

Use `go test ./packages/client/runtime -run 'Predict|Step|Messages|Mirror|Mesh|Remote|Lifecycle' -count=1`; `Prediction` misses named correction tests. Use actual client mirror tests for companion/drop/hostile/passive/projectile, rather than claiming remote-player `presentation -run Entity` covers all kinds. Audio `-run Cue` tests PCM synthesis, not provenance; provenance uses client cue-selection/step transcripts and F3 real tests. Discover the intended oracle before acceptance.

## Node 2.3b: Far-tile preparation

Editable `packages/engine/crates/mornlea_client_core/src/preparation/lod.rs`, `packages/engine/crates/mornlea_client_core/tests/presentation_contract/lod.rs`; predeclared/exported in 1.2. Near mesh/preparation queue owners, session and family assembler are read-only. Consume exact LodConfig/TerrainKey/PreparedResourceKey and accepted numerical facade. Implement the ring, distance/x/z order and static precharge algorithm in 05-supporting-values; submit owned jobs/results through accepted bounded preparation port, never call Python/GPU. Cases `lod::no_near_overlap`, `lod::negative_tile_and_radius_edges`, `lod::disabled_has_no_jobs`, `lod::budget_plus_one_retained`, `lod::old_seed_or_epoch_is_stale` first fail then pass real native preparation. Run `rustup run 1.97.1 cargo test --manifest-path packages/engine/Cargo.toml -p mornlea_client_core --test presentation_contract --locked lod`, `go test ./packages/client/lod -count=1`, and `go test ./packages/client/cmd/mornlea/app -run Lod -count=1`, requiring nonzero cases. 2.5 serially assembles actual near/far records before P8. Commit this provider only; rollback removes its owned jobs/resources and leaves near preparation intact.
