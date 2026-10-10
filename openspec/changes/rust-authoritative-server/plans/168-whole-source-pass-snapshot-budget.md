# Whole source pass prepared snapshot budget implementation plan

**Goal:** Allow a whole source publication to retain its validated per-recipient snapshot count while preserving the existing eight-frame legacy builder.
**Architecture:** PreparedSourcePublication stores an immutable snapshot_limit. new retains8; for_source_tick derives max_players*snapshot_chunks from checked ServerLimits (at most8*64=512). Exact-token validation precedes this bound and append keeps original semantic/frame owners. Actual independent CPU batches remain at most8 across1/2 workers.
**Tech Stack:** Rust1.97.1 server/publication, validated ServerLimits, actual SourceSnapshotEncoding and protocol factories.
**Spec:** ../specs/rust-authoritative-server/spec.md and ../design.md. Node3.7l3spb; tasks.md owns status; root direct executing-plans/TDD under full-task authorization.

## Identity and scope

Baseline accepted167 cc869a295b1e0b966e35cda3ec2004b952b4a11f. Existing paired owner accepted1648370413e2200a1a6cae95a7d10e8b184ecd0743e and actual CPU owner accepted165f0c118025738739991d7fb4e363ae1c9038ef423. Existing checked ServerLimits bounds players1..8 and per-session snapshot count0..64; no separate limit type, schema/version, trait or fallback. Root owns design/writes/integration/rollback; existing isolated reviewers read-only. Exact source3: packages/engine/crates/mornlea_server/AGENTS.md; packages/engine/crates/mornlea_server/src/core/publication.rs (snapshot_limit field, constructors, capacity, private cfg-test child registration); new packages/engine/crates/mornlea_server/src/core/publication_source_budget.rs (five single-topic tests). Docs4: design.md, ledger.md, tasks.md, this packet. All state/reducer/acquisition/pool/book/codec/provider/Go/config/executable/skills/dev/archives read-only. No new important directory; inherit server guide and update ownership note. No clone/worktree/Loom/Claude/push/deploy/window. Preserve Go79/dev9/archive2/original evidence.

## Frozen callable contract

PreparedSourcePublication::for_source_tick(publication:TickPublication,limits:ServerLimits)->Self derives immutable snapshot_limit=usize::from(limits.max_players())*limits.snapshot_chunks(). Constructor cannot fail because checked limits structurally bound product to512 (usize safe); zero count yields0. Existing new(publication) still limits8. No arbitrary caller-provided usize, dynamic reserve or self-modifying limit.

append_encoded_snapshot preserves exact original capture-token check first. If current held paired count>=snapshot_limit, return Capacity Resource::Snapshots limit:snapshot_limit observed:current_count+1, without appending event/frame. Valid original token then moves encoded capture/semantic/section charge/frame through existing into_source_parts. Capacity counts held snapshot pairs, not ordinary events or CPU requests. Existing append_event/publication/into_publication/private into_parts behavior unchanged. Selected per-recipient count/bytes/first-oversize priority remains the future selector's responsibility; this constructor does not certify Wanted, freshness, per-session selection, Queue admission or automatic timing. New whole-pass source caller must name this accepted SHA before using it.

Rejected alternatives: globally replace legacy8 with512 would weaken existing contract; unbounded growth breaks ownership bounds; borrowing live mutable limits can change capacity mid-pass; treating CPU cap8 as full-pass limit truncates legal8recipient*64source work; encoding on append repeats normalization/framing on authority path.

## Five test-first cases and helpers

Private cfg-test publication child uses literal full AIR ReadyChunk owners at Overworld(x,0), generation7/revision5,24Single0 sections and fixed32D/32F/16C slots. Inputs are prepared complete immutable captures, not disk/native generation or automatic source eligibility. Actual SourceSnapshotEncoding1/2 supplies outputs from actual codec threads. Helper admits batches at most8, waits/collects whole results without replacing owner, stop_new/wait/collect/close SAME owner before literal product assertions; every result must retain original session/request/capture. CPU cap is independent of accumulated builder count. Complete frame decoded with real ProtocolCodec where relevant. No fake provider, test-only production constructor or borrowed authority.

1. source_pass_budget_configured_pair_bound: checked limits players2/snapshotcount5 =>capacity10; append ten original CPU outputs in exact input order, then eleventh returns literal Resource::Snapshots limit10 observed11 with semantic publication unchanged. Existing new=>8 minimal forwarding fails ninth append causally; ordinary prefix events do not count as snapshot pairs.
2. source_pass_budget_maximum_whole_pass: checked8/64 =>capacity512. Actual two-worker owner processes64groups of8 for sessions1..8 and keys0..63, no duplicate session/key, joined before builder assertions. Append all512 original outputs; event counts/per-recipient64 and metadata literal revision5/24AIR hold; next matched output refuses limit512 observed513 without mutation. This qualifies prepared whole-pass capacity and real CPU/pair ownership only, not actual512 FIFO/source selection. Minimal8 forwarding fails ninth causally.
3. source_pass_budget_zero_preserves_token_precedence: checked8/0 =>capacity0. Equal key/gen/rev but fresh different token gives InvalidInput chunk_encode_capture before capacity; matching original gives literal limit0 observed1. Neither mutates ordinary event prefix. Minimal8 forwarding wrongly accepts matching output.
4. source_pass_budget_legacy_eight_is_unchanged: existing new accepts8 and refuses9 with original literal bound; mismatched token still wins when full and leaves publication unchanged. Actual CPU results/close, no constructor fallback or selector. Forwarding control.
5. source_pass_budget_moves_original_pairs_with_interleaved_events: checked2/4 =>capacity8. Append ordinary prefix/interstitial events and8 actual encoded values, record original semantic section pointers/frame pointers/full bytes before consumption. into_parts moves events and frame indices matching the literal sequence (snapshot indices1,3,5,7,9,11,13,15); frame/semantic allocations and full wire snapshot keys/revision/24AIR remain original. Constructor limit alone cannot certify source queue mirrors. Legacy8 forwarding passes this move control.

Write five tests/child registration first; separate declaration-only missing constructor evidence. Minimal for_source_tick forwards to existing new without product limit; expected three causal failures/two controls. Freeze test bytes/minimal source after qualified baseline. Final snapshot_limit constructor/guard makes same five pass. All helper/admission/fixture errors retained/excluded and reconciled before qualification; no assertion weakening after product.

## Validation and closure

Rust1.97.1 locked/offline --lib source_pass_budget_5; --lib prepared_source_8; --lib source_encoding_8; --lib core::chunk_encoding::tests6; --lib source_continuation_7; native env -u CARGO_TARGET_DIR make rust before actual --test persistence_failure chunk_encoding2, actual acquisition10 and full; --test server_replay publication_192; full make rust-check expected3989/76/70nonempty6empty/replay677/0failedignoredwarnings/fmtClippy. Go snapshot7/businessMemoryTCP1 race/count1; audits4/strict128; Go79/dev9/archive2/frozen3/index protected. Root rg derived-consumer scan of publication.rs and cap annotations; existing state legacy bound assertion stays unchanged. Source/docs/committed immutable independent review of source3/docs4 exactsevenpaths. Scoped commit feat(server): bound prepared snapshots for whole source passes. Root owns inverse rollback. Architecture retrospective after verified round; no installed plugin edits or fabricated AOCI.

This is compile-ready whole-pass bound with actual CPU/builder evidence, not selection, transport admission, async tick completion or runtime integration. Accepted167 private continuation still completes synchronously. Actual automatic selector/CPU temporal producer/512 FIFO integration/configured startup/shutdown/trusted observer/all outcomes and broad3.8/4.1/4.2 OPEN; all-taskACTIVE.

## Root self-review

Checked ServerLimits producer bounds match new constructor consumer units; no new shared type or parallel writer. Snapshot capacity and provider capacity are distinct finite owners, and repeated CPU batches do not uncharge or replace original result tokens. Legacy8 remains unchanged and separately executed. Three causal/two controls cover zero/product-limit and original pointer/index moves. Derived consumer notes/legacy literal tests are read-only; root owns cfg child and Cargo/native/audit scans. No future async task is marked ready without its own exact producer checkpoint/caller/errors/resource and source-parity oracle. Planning scope is one independently rejectable ownership contract.

## Verified source and final gates

Root qualified declaration4 raw4fa1716e78cb1bf6af91c2b62211de0b89803c87884a45b688472fb57e622947 and original causal RED3/control2 raw26939ef17ad9fd666cad9d566bc79b7faef76399ab0c0ae636b42844428a66f8. Original missing-trait declaration and full Clippy useless_vec failure are preserved and excluded from final acceptance. After exact two-element Vec-to-array correction, minimal forwarding RED still3causal/2controls rawfba469184779ff0c06a797966596996f5a206f2850a74177050f5907ef8b6fbf; complete revised test SHA59dec2ad0f0452f06348eab2bbb16171120d4a48d56c0ffb954254f756e215ef then stays unchanged through all final gates. Independent source P3 corrected only struct comment; revised source3 RAW5aa0a75195fe3e635a6f6b63bd964707af6d7ba502bdafa2b4c441e314b59276 independently SCOPED_PASS. Old original source-bound passes remain historical, not revised final receipts.

Final twelve actual commands bind this source3 over accepted167; raw receipts and command verification are in runtime/whole-source-pass-snapshot-budget-20261007.

| Gate | Passed | Raw log SHA256 |
| --- | ---: | --- |
| budget-final-revised | 5 | fd789e503cbc49a4125c193a70fe9aef8cd31d8fb1e682c98b46ee4633619b1d |
| prepared-source-controls-revised | 8 | 442710522c968a5d154c81d1668207550bbf69f9be34206abdc9f4fbfac1ecd0 |
| source-owner-controls-revised | 8 | aa392a5d7b9d5fec290503fbc3fe8b1bf38e832c63ad7683255db89fae3bd135 |
| factory-controls-revised | 6 | 6c7909f7dc70d35e839105300484c4ba5e3d38f0d83fb7798d22868ba634ce42 |
| continuation-controls-revised | 7 | 7020bf9d006bcf3f354cf5cb19df19ea1fefccb80373e02d3daec11dbc36eac0 |
| native-build-revised | 0 | 7452c2c23caf08e870a0a6654d001c7b46d3eb294a9bb3e2c9f4c9e261c22ba3 |
| actual-capture-provider-controls-revised | 2 | 087a18d4ff59b899f18f54d078535554bec65c490fe69235084148cabac9f525 |
| actual-acquisition-controls-revised | 10 | 73352a25f94306c198993bcc90cccf077700d774669a6411c9978b432bb4d164 |
| publication-controls-revised | 192 | 2c6fc35832a75f91813e88e5e5989dd1bf2bbe49e874b6433a8091e66b8c9173 |
| full-rust-revised | 3989 | 3cae80394941c592c52bbdfbfee8256e34a720239817763164de23b49aaa2d04 |
| source-snapshot-controls-revised | 7 | 12a4d30c4461b392a7cdaea5e0df1120e1b53f4f70e43533a307585903bf859c |
| source-memory-tcp-control-revised | 1 | 20a07a02d452ea321a7474780a1a90923c098a7ff8ec1ecb3af082f977276a6f |

FullRust3989 covers76 summaries/70nonempty6empty/replay677/zero failedignoredwarnings and fmt/Clippy. Native build precedes real providers/fullRust/Go; Go7+1 are actual source race/count1 controls. Go79 current/parent plus dev9/archive2/frozen3/index protected. Root architecture retrospective: Architecture skill: no change, existing bounded immutable owner/lifecycle rules cover this scoped constructor; no future selector/runtime fact promoted. AOCI tools unavailable/canonical complete index absent; no fabricated cognition or maintenance. Separate closing audits/strict, independent docs and immutable committed source review precede durable acceptance; broad tasks stay ACTIVE/OPEN.

Closing audits4 exit0/raw2459f62baa41a3166c267114f959c55cadf44b6eb39c77012b6337b9a3e53340 and strictOpenSpec128 exit0/rawfa8f335ac9d9186c728097c895a4f7cc08103eee9eb44e4b9e681650edb6af67 bind unchanged revised source3. Fourteen total final gates; independent docs/immutable committed review and durable evidence are separate local acceptance steps.
