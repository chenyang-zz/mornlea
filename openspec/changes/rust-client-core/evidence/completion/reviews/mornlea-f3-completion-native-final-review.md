# Integrated source native/host scoped review

Reviewed immutable source: `2cea2020091168ea5a6845517bc49ccbdbae0268`.
Reviewer: Codex `/root/review_native`; independent read-only review.
Checkout: `/private/tmp/mornlea-f3-completion-native-final`, detached exact SHA, clean before and after tests.
Rust build output: `/private/tmp/mornlea-f3-completion-native-target`; `CARGO_BUILD_JOBS=2`. No production edits, shared native output mutation, runtime materialization/install or Godot process launch by reviewer.

## Scoped code verdict

No remaining reproduced defect in native/host/qualification source scope. The original silent-failure finding is resolved and independently accepted in the integrated host. This is code-scope acceptance at the exact source above; runtime/native100/derivative evidence remains to be inspected before claiming complete native qualification.

The independently accepted host source `c0c008684057971e9311a2eb30f675c1f3312aad` is byte-identical here. SHA256:

- `apps/mornlea-godot/app/host/feature_host.py`: `20ea27fdc7c9bad6c85bb304d5e7c651cf6d805ae87c633c3467ca1a9c532a73`
- `apps/mornlea-godot/tests/unit/test_rust_feature_host.py`: `093eee2897f84ec7bcaceb5591af37edb70069ea1c47e3c74503a8d5b31e7deb`

Entire tracked `apps/mornlea-godot`, `scripts/godot` and `packages/engine/crates/mornlea_godot` trees, plus core `src/contracts.rs`, `src/presentation/frame.rs` and `build.rs`, are unchanged from that accepted host source. This preserves native Dictionary/Array/scalar/NIL conversion, closed input/schema validators, exactly eight facade methods, one-step/one-pull same-frame fanout, consumer-before-native release, refused-close retry ownership, guard/probe isolation, session scenes/fixture server, sandbox runner, Py4Godot patch/builder/pinned identity and export exclusions.

Reviewed new core changes at the G1 integration seam: `AcceptedObservation` gains only private `suppressed_actor_records`, initialized empty by its unchanged public constructor; query/setter/byte-accounting remain crate-private. Three actor state providers skip those private suppressed ordinals before resolution while retaining original packet enumeration for published record provenance. Mirror-owned freshness/capacity changes do not change C1 signatures, `FamilyRecords`, G1 render fields, scalar encodings or host schemas. The private mask is frozen at commit and charged to actual retained observation backing; native rendering continues from validated complete semantic frames, never from raw/private mask storage. Full mirror correctness and provider parity are separately reviewed by core/provider siblings.

The capability inventory JSON has unchanged keys, list row counts/order, schema/seal/identity/family/row values, and every existing test reference. The only semantic edits append Rust regression references to existing `rust_tests` and `rust_test` arrays. This was independently checked recursively, not inferred from the diff size. The core contract digest legitimately changes because its accepted digest input includes `presentation/mod.rs`; expected current digest is `53c88b42016db0da0d1459799c6b301a4e7232a7c4bbe4a8c85afdf4d6f415b1`. Native diagnostics and process reports must attest that digest and current source, not reuse predecessor binaries.

## Fresh integrated-source checks

Working directory: `/private/tmp/mornlea-f3-completion-native-final`.

| Command | Actual result | Evidence |
| --- | --- | --- |
| `/Users/chen/work/mornlea/apps/mornlea-godot/.venv/bin/python -B -m unittest discover -s apps/mornlea-godot/tests/unit -v` |exit0;48 tests passed, no skips | `/private/tmp/mornlea-f3-completion-native-final-host.log` |
| `CARGO_BUILD_JOBS=2 CARGO_TARGET_DIR=/private/tmp/mornlea-f3-completion-native-target rustup run 1.97.1 cargo test --manifest-path packages/engine/Cargo.toml -p mornlea_godot --locked --test rust_producer --test lifecycle_contract --test family_registry` |exit0;42+27+17=86 tests passed, no ignored/filtered tests | `/private/tmp/mornlea-f3-completion-native-final-adapter.log` |
| Exact Git tree comparison, SHA256 host/test comparison, recursive inventory JSON comparison and independent contract digest calculation |exit0;unchanged scopes and append-only references verified | `/private/tmp/mornlea-f3-completion-native-final-identity.json` |
| `git status --short` / `git rev-parse HEAD` after tests |exit0;clean exact `2cea2020091168ea5a6845517bc49ccbdbae0268` | tool trace and detached checkout |

Two initial disposable recursive comparison attempts exited1 because the reviewer check assumed only one of the inventory's existing key spellings (`rust_tests` versus `rust_test`). The corrected check explicitly permits both append-only arrays and rejects every other change; it passed. These were review-helper assertions, not source or production test failures.

## Carried byte-identical evidence

`/private/tmp/mornlea-f3-completion-host-rereview.md` binds the accepted host source and both matching digests. Its independent all-nine-G1-class step/pull failure probe, three repeated refusals each, bounded status/no-trace-growth assertions, malformed envelope refusal, preservation/recovery/reset assertions, Ruff and actual-stub mypy checks are carried after independent byte-identity verification. The complete48 host suite was also freshly run here.

`/private/tmp/mornlea-f3-completion-native-review.md` contains broader connected native/host/qualification code review at predecessor `24f57d73c1e1c2132aa5ad3055767fec2628ca35`, with its failure finding superseded by the repaired exact host evidence. Its runner23 (including real Darwin exact-port sandbox probes), builder7, shell syntax and native library153 checks apply to unchanged helper/adapter bytes; they are explicitly carried, not newly executed at this integrated source. Fresh86 adapter executions exercise the current core integration.

The legacy `lifecycle.rs::ownership::CoreOwnership` helper is used only by `lifecycle_contract` tests; its27 results must not be misattributed to actual Python host refused-close/release wiring. The actual host48 and adapter42 tests plus native matrix are the applicable proofs.

## Pending native evidence inspection

Controller exclusively owns actual native bridge/scalar/release qualification, rebuilt100 separate sessions, derivative editor/export matrix and full stage gates. On receipt of current-source artifacts, reviewer will verify source/contract/native/Python/Godot/server identities, exactly100 successful separate process entries with meaningful families/session/reset/reconnect/preparation/release evidence, ordered markers, static-runtime freeze versus recorded bytecode caches, owned-only TCP sandbox results, residue/crash/error failures and retained historical failed attempts. No duplicate100 run is requested.

Not checked yet: the controller's new-source native reports and complete runtime qualification; core/provider/go-corpus and aggregate gates are separately owned. Risk: code-scope acceptance and byte-identical helper evidence do not alone close full native or F3 acceptance.

## Fresh native boundary evidence inspected

Controller-run supervisor `/private/tmp/mornlea-f3-completion-native-bridge.py` completed exit0, recorded at `/private/tmp/mornlea-f3-completion-final-native-bridge.log`. Its owned project/report is `/private/tmp/mornlea-f3-completion-native-bridge.wp37d6z2/`. Reviewer read supervisor and actual import/boundary logs, confirmed the accepted exact scalar/NIL/stale-token/duplicate-close/panic/release assertions in the byte-identical scene script, and independently rehashed the retained native artifact and complete final Python identity.

Report binds source `2cea2020091168ea5a6845517bc49ccbdbae0268`, expected contract `53c88b42016db0da0d1459799c6b301a4e7232a7c4bbe4a8c85afdf4d6f415b1`, native SHA256 `4f235884d23100862bce473f8656e9eb39c3de7e14f4412482b4b4a1b250b7a1`, Godot SHA256 `c7cccbf8fb143e34e02fd6521e09be2c2b974f0d5db080b19071c9c570718ccf`, and pinned Darwin `4.7-alpha21-mornlea.3` loader/main identities. Static Python digest is unchanged (`a75451c4ca07ad78505ac9cf0221edc0d8b890f148181020ba172d16c56380e5`);1214 generated cache paths are explicitly retained. Independent final-tree rehash matches the report.

`boundary.log` contains exactly one expected-native-panic marker, consumer release before native core release, exactly one success marker and ordered Rust deinitialization. `import.log` and `boundary.log` contain no runner-defined error/leak marker. Their SHA256 are respectively `f411554456c19ee26d8e0757b2c7bcdfb86329a4fc44e0cf8869a8822bd977cf` and `b61326308e1976b5c6c71d2c16926bcb3d9513109f7a30f2bf54c09e47f5d2d2`. The sole known connect-shadow warning concerns the pinned godot-rust0.5.5 dynamic method convention; it is not a failure and no suppression/bypass was applied.

The supervisor copies an owned project, verifies identical native bytes before launch, poisons ambient Python, supplies empty PATH and owned HOME, applies deny-all-network, uses the reviewed45-second child deadline/owned process cleanup, and rechecks native/static Python identities after actual import and boundary runs. This qualifies actual native scalar/NIL marshalling and bridge release/panic behavior at the current source; it is not substituted for native100 or complete derivative qualification, which were still ongoing at inspection time.

Historical native100 failure preservation was independently checked: `/private/tmp/mornlea-rust-session-smoke.g2vuqdzp/report.json` still binds failed source `908a099282a1ad35736d0ce7ba17abcb089299a8`, requested100, exactly11 completed iterations, `ok:false`, child exit1 and no cleanup error. Iteration12 log/result, server log and aggregate report all remain present. This historical failure is not counted as current passing qualification.

## Fresh100 separate native sessions inspected

Current-source aggregate `/private/tmp/mornlea-rust-session-smoke.6mldydr8/report.json` is `ok:true`, requested100, exactly100 retained iteration reports and100 logs, source `2cea2020091168ea5a6845517bc49ccbdbae0268`, contract `53c88b42016db0da0d1459799c6b301a4e7232a7c4bbe4a8c85afdf4d6f415b1`, explicit qualification feature. Reviewer independently applied the byte-identical strict report and lifecycle/error/leak validators to every per-process result/log and matched each to the aggregate entries. All100 pass; no startup-only report or missing meaningful family/reset/reconnect/input/preparation/retention/release assertion is accepted. The reviewed runner launches one owned process per iteration with a unique token and checks completion/residue; no duplicate100 execution was launched by this reviewer.

All five sandbox results and retained probe logs pass: only owned127.0.0.1:53895 connects; other loopback port, external address, IPv6 and otherIPv4 at the same port refuse with permission errors. Native hash `4f235884d23100862bce473f8656e9eb39c3de7e14f4412482b4b4a1b250b7a1` and Godot hash `c7cccbf8fb143e34e02fd6521e09be2c2b974f0d5db080b19071c9c570718ccf` match the retained independently rehashed boundary artifact. Fixture hash is `49a33ec227fc367031b92c4a98cb8b4fabebb1128115f0d7ab9cb4f3f723a30a`.

Actual fixture result records200 admitted sessions,82272 source packets,11424 source ticks,201 retired connections (includes the allowed sandbox probe), zero retained sockets, no errors, owned stop requested and no deadline expiry. Server log has no runner-defined error/leak marker; aggregate has no error or cleanup_error. Python static digest remains `a75451c4ca07ad78505ac9cf0221edc0d8b890f148181020ba172d16c56380e5`; immutable loader/executable/library hashes match before/after. Initial1210 and final1218 generated caches are recorded separately.

Independent inspection command exited0; summary `/private/tmp/mornlea-f3-completion-native100-inspection.json`. Aggregate report SHA256: `415cc7058157d1af20365820c268a41361fabe7a475bf6212d21b2ef3280bfc3`. Scoped native session100 qualification is accepted at this source. Full native verdict still awaits the controller's complete derivative editor/headless/coexistence/exported-program matrix.

## Final scoped native qualification verdict — accepted

At immutable source `2cea2020091168ea5a6845517bc49ccbdbae0268`, original3.3b (actual native Godot/Python scalar/NIL marshalling, release ordering and panic containment) and3.3c (rebuilt artifact-bound100 real separate process session cycles) pass independent source/evidence review. The native portion of3.4 (real F2 TCP session admission, all ten meaningful families, typed input, preparation, reset/reconnect, stale epoch/token refusal, retained frame and consumer-first release through C1/C2/G1) also passes. Real Memory/TCP all-family provider semantic parity is separately owned by the provider reviewer; full-stage4.1/4.2 and overall F3 acceptance are controller-owned. No remaining reproduced native/host/qualification blocker was found. This final verdict supersedes only the earlier pending-evidence statements in this append-only report.

Current-source complete Darwin matrix is independently inspected: `/private/tmp/mornlea-f3-completion-final-native-results.json` binds initial and final source to the exact SHA above and records both native commands exit0. Matrix command `scripts/godot/python-runtime-check.sh --qualify --offline` completed in337.347s; `scripts/godot/smoke.sh --iterations 100 --isolated-python --core rust --exercise-session` completed in753.6s. Working directory is `/Users/chen/work/mornlea/.worktrees/rust-client-core`. Reviewer reread current HEAD/status after completion: exact source and clean worktree, no source change.

Matrix log `/private/tmp/mornlea-f3-completion-final-python-matrix.log` records verified canonical `.3` runtime materialization and exactly one final success summary with effective100 isolated embedded Python cycles, Darwin-arm64 CPython3.14.4 and exported macOS app. The reviewed unchanged script reaches that summary only after bundled runtime/version/no-installer checks, isolated poisoned-path/network-denied editor load,100 successful headless probes, coexistence, locked ordinary debug/release bridge builds, pinned export templates, export and actual exported-program Python/coexistence assertions. A failed child, missing marker, extension/script error or external runtime marker exits before success; no exemption or fault bypass appears. Qualification-only scenes remain excluded from the ordinary export artifact. This is an executed native/export matrix, not static source-only substitution.

Reviewer independently rehashed installed native main/loader against tracked manifests: main `15558cb16f7dc65ad59a9bcdf101c142a6f4ace52d9fac0b819fa6e61388eaf6`; loader `bb2e4f7fef9fb7bf10150e80892d91077ec34cd951d722bff220798604b8ef4f`. Matrix materialization records full canonical artifact `8da24f2f79d033a88493410930ced3d47161e1963f68b278da0ca07339c3c465`. Generated caches after execution are not confused with that canonical pre-import payload. `.3` Darwin acceptance does not claim Linux NIL/runtime qualification, default switching, production rollout or external publication.

Independent matrix inspection exited0 and is summarized in `/private/tmp/mornlea-f3-completion-matrix-inspection.json`. Matrix log SHA256: `f4384cfc65307c78b05eb7ebe07bd2fbea499f50bdde7f91932a5c7a5c333686`; native command ledger SHA256: `3cc03f64c010e6a8f823935b8949f1d2a65135d03cdaef3bdf23b64bc9c59188`. Successful individual editor/export outputs are captured and asserted by the script rather than preserved separately; the retained outer command log, actual exit and source-bound command ledger establish the matrix result. Native100 retains every process result/log separately as inspected above.

Not checked by this scoped reviewer: final full-stage Go/Rust/audit/OpenSpec aggregates, complete Memory provider/oracle semantics, merge or remote readability. Risk: native acceptance is limited to the qualified Darwin-arm64 derivative and the exact source; changed source or derivative requires renewed identity/review.
