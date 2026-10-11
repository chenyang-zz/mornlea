# Companion lifecycle memory CAS implementation plan

Goal: expose source-compatible lifecycle reads and atomic active-memory replacement on the sole complete companion ledger, preserving body/task state and every older immutable target. Architecture: extend the accepted private authoritative companion owner; the actor ledger validates revision availability before mutation. Tech: existing Rust domain/storage/server contracts, actual background DiskStore and off-tree unchanged Go characterization. Spec: ../design.md and ../specs/rust-authoritative-server/spec.md. Root uses executing-plans directly; tasks.md alone owns status.

## Accepted prerequisite and execution

Accepted authority companion startup/capture SHA4277697511c57337e0f312d564a561f1a32006c9 supplies the sole complete aggregate and raw-task cache. Root owns design/product/tests/integration/rollback; native agents remain read-only census/review. The explicit user instruction to finish all tasks directly controls execution and avoids redundant stage approvals; brainstorming/writing-plans still resolve and document the concrete contract before implementation. No clone/tree/Loom/Claude/push/deploy, source Go and protected dev/archive assets unchanged. No protocol/schema/ABI/dependency changes, blocking I/O or new public fault hook.

Alternatives: a second memory aggregate cache would drift from body/task capture and old targets; using generic observe would miss source CAS/idempotence/revision rules and its public Running-only gate cannot serve finalization. Selected: one serial lifecycle operation over latest ledger content, healthy Closing supported before final flush. Remote MemoryOwner remains its working provider mirror, not a second durable aggregate.

Editable paths relative to packages/engine/crates/mornlea_server: src/core/state_companion_persistence.rs for lifecycle ports; src/core/actor_save.rs for the private revision-availability query; AGENTS.md; NEW tests/persistence_failure/source_companion_memory.rs; existing source_companion_persistence.rs only to include that child and expose its private actual-disk fixture helpers to descendant tests; NEW src/core/state_companion_memory_tests.rs plus private test registration if Closed/frozen controls require access. Active design/tasks/ledger/this packet editable. All Agent provider implementation, shutdown machines, Go, sealed fixtures, other rules/manifests read-only. Existing private leaves inherit the crate guide. Derived consumer gates are workspace fmt/Clippy/tests and source-comment/language audits; no pinned Go source digest changes.

## Exact interfaces and policies

AuthorityState public methods:

```rust
pub fn companion_memory_lifecycles(&self) -> Option<&[StoredCompanionLifecycle]>;
pub fn companion_memory_lifecycle(&self, id: CompanionId) -> Option<&StoredCompanionLifecycle>;
pub fn replace_companion_memory(
    &mut self, id: CompanionId, epoch: u64, expected_revision: u64,
    next_revision: u64, operation: OperationId, summary: String,
) -> Result<(), ServerError>;
```

StoredCompanionLifecycle is the accepted mornlea_storage type; OperationId is the existing checked crate contracts UUIDv4 type, not a new domain identity. Read-only borrows expose sorted complete latest metadata, including inactive bodies, with no allocation; disabled ownership or missing ID returns None. Immutable borrows cannot escape an authority mutation. Reads do not claim remote readiness and may still inspect retained closed metadata.

Replacement requires managed live acquisition, healthy Running or Closing, enabled whole companion owner and a retained open ledger. Existing require_live_chunks(false) preserves sticky failure/Closed refusal. Healthy Closing is allowed for the later finalization caller, but this node does not implement that caller. The actor ledger checks the highest occupied aggregate revision before validating proposal or checking idempotence:

```rust
pub(crate) fn check_replacement_revision(&self, key: &SaveKey) -> Result<(), ServerError>;
// open/existing; max(persisted, flight.revision if held).checked_add(1)
```

No ticket/revision is allocated. Failed admitted writes keep their immutable flight and therefore occupy its revision. Overflow returns existing Internal{invariant:"actor save revision space"} and preserves all content, dirty/force/flight facts. In particular a MAX target held over MAX-1 acknowledged revision refuses even an otherwise idempotent memory replacement, matching Go's validation order. This helper changes neither ordinary body observation nor selection.

Proposal requires epoch!=0, next_revision!=0 and next_revision>expected_revision. Source reconciliation may jump several memory revisions: do not restrict this aggregate CAS to +1. Operation is already checked UUIDv4. Summary is UTF-8 by String, at most2048bytes inclusive and contains no NUL; empty, edge spaces, tabs/newlines and other non-NUL controls remain source-valid. Invalid payload field companion_memory; active/epoch/expected conflict field companion_memory_cas; missing ID field companion_memory_lifecycle. Error messages are internal typed diagnostics, not new wire outcomes.

Look up the bounded latest lifecycle. Exact already-installed active epoch/next revision/operation/summary returns Ok even when expected_revision is stale, but only after availability and proposal validation. It neither dirties a clean value nor clears existing dirty/flight ownership. Otherwise require active, same epoch and memory_revision==expected_revision. Prepare an owned complete latest candidate, replace just that lifecycle's revision/operation/summary and zero its tombstone. Preserve namespace, epoch, active flag, every body/inactive lifecycle/task/FIFO and current raw-task cache. Full existing codec validates before ledger.observe(value,true,false); no effect or resident mutation, no dialogue event, no mirror/provider reservation mutation. Successful observe dirties changed complete content and leaves an older target immutable. Subsequent settled body/task capture reads this new latest lifecycle and cannot overwrite it with startup data.

## Review focus and concrete controls

Most consequential edge classes: aggregate MAX/occupied MAX before idempotence; source revision jumps and stale-expectation idempotence;2048UTF-8bytes/empty/non-NUL controls; concurrent older failed target while memory changes; healthy Closing final capture versus sticky/Closed/frozen ownership. Every class has a named test below. No caller decides missing shared behavior.

1. Write missing-API declaration RED tests in the new descendant topic, reusing the existing literal fixture. Validate reads over1active/64retained sorted metadata, None for disabled/missing. Atomically replace epoch7/memory2→3/operation200/summary"new memory"; assert bodies, queues, inactive metadata and namespace exact, aggregate selection revision10. Source memory gap2→20 must succeed. Exact already-installed metadata with stale expected0 must remain clean. Refuse wrong epoch/expected/inactive/missing/zero epoch/zero next/not-increasing/NUL/2049byte summaries with identical complete aggregate, resident state and save stats.
2. Observe qualified missing-method failure, then implement minimum read/replace. Concrete summary table: empty, " padded ", newline/tab,1024two-byte characters (2048bytes) succeed;1025two-byte and embeddedNUL refuse. Upstream OperationId zero/version/variant controls remain in the existing identity contract and cannot be passed to replacement.
3. Add genuine assertion RED for occupied MAX before replacement if the availability seam is initially absent. Cover acknowledged MAX, held MAX over MAX-1 and a failed target returned for retry, including exact installed idempotence. Pin no mutation/dispatch on overflow. Implement the private highest-occupied query before cloning variable input.
4. Public owner controls: older body/task target remains exact when lifecycle CAS changes latest, ACK leaves latest dirty and next selection advances envelope by one; capture preserves changed memory after ordinary and unpublished final execution; healthy Closing replacement allowed, disabled/Closed/sticky/frozen refused in private prepared-state controls. Tests name prepared state mutations and do not claim an assembled shutdown.
5. Actual background DiskStore controls reuse the accepted native-placement recipe, with helper visibility limited to the test parent subtree. Save baseline10, admit/fail actual companion TempSync target11, replace memory on latest, run actual unpublished final capture, flush identical old11 before latest12, explicitly close and reopen complete metadata/body/task fields. A source-valid active zero-memory baseline may be seeded through real disk and replaced from0 with an arbitrary compatible higher memory revision; retain inactive tombstone metadata exactly.
6. Off-tree unchanged Go program calls exported actual Companions.ReplaceActiveMemory/MemoryLifecycles and actual codec with a capturing store; pair full accepted/refused/idempotent payload outcomes and revision sequence against Rust public authority output. This is source-owner characterization, not a real Go filesystem claim. Run unchanged Go atomic/in-flight/occupied-MAX/overflow focused tests with -race/count1. Root owns all oracle code.

Run focused Rust --test persistence_failure source_companion_persistence::memory and private memory tests offline/locked, then final make rust-check on frozen source hashes. Run four language/comment audits, strict OpenSpec128, independent source/doc acceptance and dev/archive protection; scoped English commit before next product node. Failed setup/assertion/gate attempts stay evidence. No foreground game window.

## Handoff and exclusions

Later ordinary/final Agent consumers use these exact ports and accepted SHA after this node commits. They must retain proposal ownership through durable CAS and must not treat the current MemoryOwner::drain counts as a durable acknowledgment. That adapter/finalization implementation is a separate serial node: no provider reservation/drain change is hidden here. Configuration, complete Agent runtime, executable assembly, all remaining outcomes and broad3.8/4.1/4.2 remain open. Rollback is a reviewed inverse of only this node; existing accepted body/task ownership stays intact. Root self-review pins every argument type, bounds/validation order, consumer prerequisite, source mismatch and exclusion. Architecture skill: no change unless a verified stable cross-task rule warrants promotion.
