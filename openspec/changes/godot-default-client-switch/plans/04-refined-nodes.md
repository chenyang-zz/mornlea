# Refined node decisions

Unsplit nodes retain the exact files, APIs, algorithms, red/green cases, commands and rollback in the existing worker packet. The dependency register adds readiness requirements and does not grant edits to shared files.

## Source retirement is an inventory-gated serial action

P14 1.1 enumerates actual direct/transitive product consumers into `testdata/runtime-migration/release/cutover-inventory.json` with exact `{consumer_path,edge_kind,producer_identity,product_or_offline,retirement_node,rollback_artifact}` rows. Nodes 3.2/3.3/3.4 may edit only those accepted rows' named consumer files and their registered audit fixtures; no glob deletion or inferred directory-wide ownership. 3.3 renderer ABI v19 and 3.4 Go runtime edges stay distinct. Re-enumeration finding an additional consumer requires controller reconciliation and focused revalidation before any removal.

Before approval, validate two independently built package sets, actual-host reports, fresh per-case evidence and complete prior-release/world restores against exact sealed cycle digests. 3.1 remains blocked until explicit approval names those digests. Planning refinement supplies no cutover, tracked-image or destructive retirement authorization. Preserve prior release and offline tools throughout retirement; each accepted source-removal node has its own scoped commit and rollback.

## Node 2.2b: Candidate native diagnostic integration

Controller exclusively edits `packages/engine/crates/mornlea_client_core/src/bin/mornlea-desktop-launcher.rs`, `packages/engine/crates/mornlea_client_core/tests/product_cutover/launcher.rs` and test registration. Consume accepted 2.1 native probe, 2.2 closure checker and P13 launcher. Run probe and closure before any Python load/feature import or server start; missing Python must be diagnosed by Rust while Python never runs. Tests `launcher::probe_before_python`, `launcher::missing_interpreter_without_import`, `launcher::corrupt_bridge_without_child`, `launcher::prepared_source_and_export` assert exact call order and zero child/import on failure. Run `rustup run 1.97.1 cargo test --manifest-path packages/engine/Cargo.toml -p mornlea_client_core --test product_cutover --locked launcher` and the cutover script self-test. Default Makefile/project entry remains read-only. Commit candidate launcher integration; both later cycle builds bind this accepted SHA and execute packaged diagnostics on each actual target. Rollback restores the previous opt-in launcher only.
