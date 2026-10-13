# Independent native, host and qualification review

What this change does: It moves the Godot typed host onto the Rust client owner, repairs native container/NIL conversion and release ordering, and adds a real TCP session qualification runner. Default production selection remains the pilot; native fault probes are compiled only with `runtime-qualification`.

Reviewed source: `24f57d73c1e1c2132aa5ad3055767fec2628ca35`.
Reviewer: Codex delegated `/root/review_native`, read-only product review.
Checkout: `/private/tmp/mornlea-f3-r6-native-review`, detached exact SHA; clean before and after tests. Cargo output: `/private/tmp/mornlea-f3-completion-native-target`; no shared build output was changed. No Godot process, shared runtime rebuild/materialization/install, or foreground window was launched by this reviewer. The sandbox unit test starts only owned Python socket probes.

## Must fix

1. **Failed frame publication is silent** (`apps/mornlea-godot/app/host/feature_host.py:1900`–`1912`). The host returns zero on a refused step/pull or invalid frame, preserving the old frame but publishing no stable failure through a log, trace, status or consumer callback. The original approved delta scenario at `openspec/changes/rust-client-core/specs/godot-client-pilot/spec.md:88`–`93` requires reporting that failure. A broken or incompatible producer can therefore leave the display frozen with no host indication.

   Exact-SHA reproduction uses the repository's existing native-container test shells to import the actual host, activates one consumer, publishes one valid frame, then refuses `step` and separately `pull_typed_frame` with `Internal`. Both calls preserve the prior frame but emit empty stdout/stderr, leave `trace_json()` unchanged, and dispatch no failure callback. `/private/tmp/mornlea-f3-completion-native-failure-probe.log` records both cases; command exited 0 because its assertions verify this observed defect. The smallest fix is a bounded stable host failure result/status (including typed facade class or a stable validation refusal), cleared on a successful publication and tested without changing preservation or fanout.

Verdict for this immutable source: changes requested for original criterion **A Rust-mode host tick distributes one complete semantic frame**, specifically failure reporting. This finding is independent of whether the controller's native100 succeeds.

## Coverage and verified behavior

Read root/scoped guides, `openspec/config.yaml`, F3 proposal, both delta specifications, design/tasks, runtime packet17, and original seven criteria in the repair handoff. Reviewed the connected host conversion/activation/negotiation/frame validators/tick/reset/deactivation; facade ABI scalar/tag decoding and frame rendering; Rust `CoreArena`, `BridgeSession`, native conversion and panic boundary; qualification feature registration and probe; scene lifecycle/session consumers; TCP fixture example/readiness/shutdown; smoke shell and external runner; Py4Godot main builder, ordered NIL patch and platform identities; runtime materializer/qualification flow; normal and qualification export exclusions. Review was not limited to the latest diff.

- Exact scalar leaves remain exact: native BOOL/INT/FLOAT/STRING/NIL conversion; finite checks; unsigned64 uses canonical decimal strings. Dictionary and Array None probes cover both native construction routes. The NIL patch invokes `variant_new_nil` against the correct native storage instead of the INT constructor.
- Native input bounds precede recursive copying and action admission; host representation bounds apply across arguments/keys and reject unsupported, nonfinite, cyclic/deep/oversized trees. Complete host payload validation and frozen descriptor checks precede replacing visible state or fanout.
- One successful host tick calls one step and one frame pull, then fans out the same native owned frame in dependency order. Failed admission preserves previous host/consumer frame ownership; no Go fallback is selected.
- Consumers deactivate in reverse dependency order and drop frame references before native close. Production host keeps a refused-close token/bridge in retiring state, suppresses further ticking/reactivation, and permits explicit teardown retry. `CoreArena` retires a slot only after successful endpoint close; duplicate close is idempotent. Bridge exit uses guarded close-all.
- Native methods wrap argument conversion, owner dispatch and output rendering in the native panic guard. Test fault class is behind the explicit qualification feature and absent from normal registration. Scalar/probe scenes are excluded from normal application export; qualification export presets exclude probe-dependent scenes when ordinary bridge artifacts are used.
- Rust smoke rebuilds and records source, contract, native bridge/server/Godot and Python identities. Per-process diagnostics must match current source/contract; exact report fields/count types, ordered unique lifecycle markers, error/leak/crash detection, bounded child timeout/reaping, stale report refusal and residue checks fail closed. All iteration logs are retained. Static Python bytes are rechecked separately from generated bytecode caches. Actual Darwin sandbox probes permit only owned numeric127.0.0.1 TCP port and refuse another loopback port, external address, ::1, and127.0.0.2.
- Builder pins compiler/SDK/staged Python/offline Cython, limits jobs to four, sorts/validates1122 source units, retains build logs and replaces output only after successful link. Materialization independently verifies archive/patch/main/loader/full payload hashes. Linux retains its separate older derivative; this review does not infer Darwin NIL acceptance on Linux.

## Actual validation

All commands below used exact-SHA checkout unless stated. Each completed with exit0 and nonempty discovery where applicable.

| Command | Actual result | Evidence |
| --- | --- | --- |
| `/Users/chen/work/mornlea/apps/mornlea-godot/.venv/bin/python -B -m unittest discover -s apps/mornlea-godot/tests/unit -v` |45 passed | `/private/tmp/mornlea-f3-completion-native-host.log` |
| `CARGO_TARGET_DIR=/private/tmp/mornlea-f3-completion-native-target rustup run 1.97.1 cargo test --manifest-path packages/engine/Cargo.toml -p mornlea_godot --locked --test rust_producer --test lifecycle_contract --test family_registry` |42+27+17 =86 passed | `/private/tmp/mornlea-f3-completion-native-rust.log` |
| `CARGO_BUILD_JOBS=2 CARGO_TARGET_DIR=/private/tmp/mornlea-f3-completion-native-target rustup run 1.97.1 cargo test --manifest-path packages/engine/Cargo.toml -p mornlea_godot --locked --lib` |153 passed | `/private/tmp/mornlea-f3-completion-native-lib.log` |
| `/Users/chen/work/mornlea/apps/mornlea-godot/.venv/bin/python -B -m unittest discover -s scripts/godot -p '*_test.py' -v` |23 passed, includes actual exact-port sandbox probes | `/private/tmp/mornlea-f3-completion-native-runner.log` |
| `/Users/chen/work/mornlea/apps/mornlea-godot/.venv/bin/python -B -m unittest discover -s scripts/godot/py4godot -p build_main_test.py -v` |7 passed | `/private/tmp/mornlea-f3-completion-native-builder.log` |
| `bash -n scripts/godot/smoke.sh scripts/godot/build-python-runtime.sh scripts/godot/build-extension.sh scripts/godot/python-runtime-check.sh` |exit0 | `/private/tmp/mornlea-f3-completion-native-shell.log` |
| `/Users/chen/work/mornlea/apps/mornlea-godot/.venv/bin/python -B /private/tmp/mornlea-f3-completion-native-failure-probe.py` |2 independently reproduced silent failures; assertions exit0 | `/private/tmp/mornlea-f3-completion-native-failure-probe.log` |

The first Cargo run began before the controller requested a two-job cap; it completed. Subsequent Cargo uses `CARGO_BUILD_JOBS=2`. No suite was filtered to zero or silently skipped.

## Original criterion attribution and limitations

| Original criterion | Scoped conclusion |
| --- | --- |
| Client state has one Rust owner | Adapter/host source supports pass; full live provider parity belongs to separate reviewer. |
| Frame publication is atomic and bounded | Scoped host/adapter validation tests pass; no complete-provider acceptance claimed here. |
| Reset and bridge lifecycle release ownership | Production host/CoreArena code and focused tests pass; actual native release/process acceptance is controller-owned and was not executed here. |
| Rust core parity is independent of rendering | Engine-free239 adapter/library tests execute; full offline Go corpus/actual Memory/TCP semantics are separate review. |
| Migration preserves typed host isolation | Source and host tests pass; no raw Python ABI/gameplay owner or implicit fallback introduced. |
| Cross-runtime boundaries publish only bounded atomic snapshots | Source and engine-free tests pass; actual native scalar/marshalling matrix is controller-owned. |
| A Rust-mode host tick distributes one complete semantic frame | One-step/pull/fanout and preservation pass, but stable failure reporting fails as finding1. |

`lifecycle.rs::ownership::CoreOwnership` is used only by `lifecycle_contract` tests, not by the production bridge/host. Its comments describe future bridge wiring, and its close takes/drops a token even when close refuses (`lifecycle.rs:328`–`338`), unlike the actual repaired host/CoreArena. Those27 tests qualify that helper and shared arena behavior; they cannot be cited as proof of production host refused-close/release wiring. The45 host tests,42 producer tests and actual native matrix provide the applicable evidence. This is an evidence-attribution limitation, not a second reproduced runtime defect.

Not checked: Actual Godot native lifecycle/scalar matrix, rebuilt100 processes, editor/export derivative matrix, full F2 all-family Memory/TCP integration, Go corpus/aggregate gates and remote readability; controller/other reviewers own these. No merge/full-F3 acceptance is issued by this scoped review. Risk: old frames remain visible without any reported host failure at the reviewed SHA.
