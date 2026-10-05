# Source acquisition caller plan note

The authoritative implementation interface, algorithms, and acceptance follow the [English plan](109-source-acquisition.md); tasks.md is the sole status source.

This unit continues from the accepted companion-restore producer: an explicit library caller borrows the original background scheduler and the Native GenerationPool, uses the local restore book before the real Acquire to decide whether a completion event is still wanted, and performs exactly one dirty reconcile after AdvanceActors and before hostile/gameplay. Manual advance_tick/replace/start/offer semantics are preserved. Insufficient capacity retains unstarted FIFO requests; a real typed failure must not be disguised as missing. The Pending spawn-scan dirty retry and the ordinary saved-restore failure without every-tick retry are accepted separately.

Actual Disk saved→Ready and Disk missing→NeedsGeneration→Native→Ready are the primary acceptance; doubles or declaration compilation do not accept the integration. The limits remain wanted/candidates 36660, resident 36676, load8, CPU8, staged16, with 16 admission attempts per pass. The Loom controller remains pinned to b9e4cceb576e0f1f1187d88ae35996e65b727afc; Claude writes the Rust/tests, and Codex only designs, documents, formats, and accepts.

Do not redo companion; do not change Go/binary/controlplane; do not wait for new timeout features. The original 4.1/4.2, automatic save/cache/bootstrap, the full executable runtime, and trusted-observer composition remain open; this plan claims no completion.

An ordinary Missing completion only independently appends a Generate and does not mark subscriptions dirty. Whether or not this tick's dirty reconcile runs, still-wanted NeedsGeneration keys are appended after the conditional new Load batch; unrelated Failed player keys must not be reselected because of it. An independent review of the original Go source verifies this condition, and the correction is frozen before the production call.
