What this change does: It merges the Rust semantic client-core implementation and typed Godot/Python adapter code into current dev. It preserves the existing production startup and treats the deliverable as code-only integration, with full F3 qualification still pending.

Review identity and scope

- Dev: bee580f76eab34618ce40a0fb942f5955b199d10.
- Feature: 11f8affdefcf886a2ddbda028516294ca73abc17.
- Initial resolved merge tree: 4de4d56500f99f86c6e7ef3890ac464a630c4496. This is a tree identity, not a committed source SHA.
- Reviewer: independent Codex merge_review agent. Read-only Git-object inspection; no source/index/branch changes, Godot process or game window. Report writes are confined to this file outside the checkout.
- Read root/scoped guidance, OpenSpec proposal/delta specifications/design/tasks, execution-entry evidence, accepted source bindings, final-review fixes and deferred dispositions. Inspected shared Cargo/audit/ABI/adapter/export/negotiation/host/checker changes, connected session/input/I/O/publication/lifecycle code and representative tests.

Important findings

1. Removed chat constructor prevents the merged native build (packages/engine/crates/mornlea_godot/src/abi.rs:1036).

   The incoming typed Chat decoder calls ChatIntent::new, but current dev replaces that constructor with ChatIntent::try_new to prevent source-valid persisted noncanonical commands from bypassing live chat admission. The fresh controller make rust run failed E0599 at this call, and immutable source inspection independently confirms the incompatible signatures. New client-core/Godot test fixtures also use the removed constructor and would prevent their test targets compiling.

   Reproduction: make rust on the original resolved tree; then cargo test --manifest-path packages/engine/Cargo.toml -p mornlea_client_core -p mornlea_godot --locked exposes remaining fixture callers. Smallest repair: use ChatIntent::try_new(command).map_err(into_error)? in the production decoder and try_new(...).unwrap() for known-canonical fixture construction. Keep dev's domain gate unchanged. Controller is making this repair; a new immutable tree and final committed SHA require recheck.

Critical findings: none found.

Preservation evidence

- Git-object diffs show no merge changes to dev's Rust server/protocol/storage/domain/numerical-engine or existing Go runtime trees. The latest authoritative/server/storage/FFI work is retained.
- The merge retains the feature client-core tree, capability inventory, capability catalog and checker bytes. Inventory raw SHA256: cf8899146becaa9c4feb7aa3a7880388755228c7dfb425aa2566d83bd1c26ed0. Existing prerequisite source/corpus pins were not rewritten as new acceptance.
- Cargo combines dev's server serde dependency with the feature client-core workspace member and Godot path dependencies; no external dependency/toolchain upgrade. The audit combines dev's legacy config allowance with the new workspace-member assertion.
- Shared Godot code retains dev's Linux loader definitions and section-storage-Y constants/coordinate tests, alongside the distinct Rust producer facade. Rust-mode open does not use the pilot Go loader. No production default or fallback switch appears in the diff.
- All 41 task checkbox rows are identical to the feature source. The proposal/tasks text now identifies code completion separately from qualification. The ledger preserves feature implementation history and dev's F1 archive retarget.
- git diff --check bee580f76eab34618ce40a0fb942f5955b199d10 4de4d56500f99f86c6e7ef3890ac464a630c4496 passed, exit 0.

Behaviors declined as outside this approved closeout

- Prompt record projection still lacks the position-to-block-ID contract; its task remains open.
- Native Godot Dictionary marshalling, embedded Python release qualification and 100 rebuilt real Godot/Python process cycles were not executed.
- Production connector registration, real F2 Memory/TCP session and all-family C1/C2/G1 integration are not accepted by these engine-free providers/routines. The current LoginSession/production-open staging limitations remain disclosed and are not new merge regressions.
- The documented connector.close failure ordering in LoginSession::terminate, the existing bounded audio-vector allocation in publication commit, and other already-dispositioned final-review limits remain with their named qualification owners. This merge review does not silently promote them to accepted runtime behavior.
- Renderer/resource realization, visual qualification, P8-P14/default switching and full F3 closeout remain excluded.

Verdict: fix finding 1 and recheck the repaired immutable identity before local code-only merge completion. No full F3, product-runtime or externally published acceptance is given. Aggregate validation is being executed by the controller; this report does not claim those unfinished results as independent passing evidence.

Not checked: Godot/Python processes, real server/client/bridge integration and full F3 acceptance. Known qualification limits above remain; final source identity recheck is pending.

Repaired-tree re-review, 2026-10-11

- Re-reviewed immutable tree: d93cf3a0b37f2def4fec6a5215e55f206fbb0947, with the same dev and feature inputs above. Preliminary findings remain preserved as historical evidence for the initial tree.
- Finding 1 is ADDRESSED. The production decoder at packages/engine/crates/mornlea_godot/src/abi.rs:1036 now uses ChatIntent::try_new(command).map_err(into_error)?. It preserves canonical CommandText construction and maps the domain refusal to the existing InvalidInput boundary before admission. All twelve new fixture calls use checked construction of canonical input. Independent Git-object scanning found no ChatIntent::new call in either affected crate's complete Rust source/test tree.
- The remaining production Python changes are type annotations and formatting. An independent AST comparison of original and repaired host code, with annotations removed, is exactly equal. Fixture __dict__ population is equivalent to the former dynamic attribute assignment; the JSON return cast affects typing only. No lint suppression or runtime-value conversion was added.
- Rechecked preservation: dev's server/protocol/storage/domain/engine and existing Go runtime trees remain identical; all 41 feature task checkbox rows and the client inventory/pins remain identical. git diff --check dev..repaired-tree passed, exit 0.
- Controller evidence recorded in this tree's ledger reports: actual full affected Rust tests 532 passing, release make rust passing, locked make godot-python-check passing (including mypy 54 source files), engine-free host 23 passing, source-only project/capability checks passing, strict OpenSpec 127 passing. These are controller-executed tests, not independently rerun tests by this reviewer. The serial dev-check/full stage validation and committed identity recheck were still pending when this appendix was written.
- Environment clarification: missing embedded Python was an earlier batch environment limitation. The locked runtime is available in this integration environment, and its actual static/unit gate now passed. This result still does not qualify a real Godot or embedded-Python process.

Repaired-tree Critical/Important findings: none remaining within the scoped merge review.

Verdict: the repaired source is suitable for the authorized local code-only merge once the controller's aggregate gates pass and the final committed source identity is verified. This verdict is scoped source integration review, not full F3 or external acceptance.

Declined behaviors remain: prompt record projection; native Godot marshalling and real embedded-Python release; 100 real Godot/Python cycles; production connector registration and real F2 Memory/TCP/all-family client/bridge integration; the documented deferred close-ordering and bounded publication-allocation qualification; rendering/visual/product/default switching; full F3 closeout.

Not checked: real Godot/Python/server/client processes and full F3 acceptance. Runtime qualification limits remain open; committed source identity recheck is pending.

Final committed source identity recheck, 2026-10-11

- Source commit: 470692d13f3cfe704a3c95d57ba714fe1853fd3d.
- Independently verified tree: d93cf3a0b37f2def4fec6a5215e55f206fbb0947, exactly the repaired tree already reviewed. The tree-to-commit diff is empty.
- Independently verified ordered merge parents: bee580f76eab34618ce40a0fb942f5955b199d10 (dev), then 11f8affdefcf886a2ddbda028516294ca73abc17 (feature).
- Commit subject: feat(client-core): merge rust client core into dev.
- Independently rechecked dev authority/domain/protocol/storage/numerical-engine/Go runtime preservation, all 41 feature checkbox rows, and unchanged feature inventory/catalog/checker source. git diff --check dev..source passed, exit 0.
- Controller reports all fresh aggregate gates completed with exit 0: make dev-check (all six Go vet/short and 85 Rust summaries totaling 4522 passed, zero failed/ignored), make test-race (all six Go modules), go test ./packages/audit -count=1, make rust, locked make godot-python-check (54 typed source files), engine-free host (23 cases), source-only project/capability gates and strict OpenSpec (127 passed, zero failed). These remain controller-executed evidence; the independent reviewer did not repeat suites.
- The controller reports the incoming remote heads were still unchanged immediately before commit. This reviewer verified local immutable objects and did not independently query remote visibility or publish an external record.

Final scoped verdict: PASS for local source integration of the approved code-only batch at 470692d13f3cfe704a3c95d57ba714fe1853fd3d. The sole observed merge defect was repaired and independently checked. No remaining Critical/Important findings within this integration scope. This is not full F3 qualification, external/GitHub acceptance, runtime/product acceptance or permission to change defaults.

Declined behaviors remain exactly those recorded in the repaired-tree appendix: prompt record projection; native Godot marshalling/release and real Python process qualification; 100 real process cycles; real F2 Memory/TCP/all-family integration and production connector registration; previously recorded qualification limits; rendering/visual/product/default switching; full F3 closeout. A subsequent evidence-only commit must name this source and pass a diff/identity check before its head is used as an evidence identity.

Not checked: real Godot/Python/server/client processes, external source visibility or full F3 acceptance. Those qualification limits remain open; reviewed committed source identity is now verified.
