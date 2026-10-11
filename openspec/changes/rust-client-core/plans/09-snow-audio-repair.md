# Snow source and audio ownership repair

This is an execution brief linked from packet08; tasks.md remains the status authority.

## Ownership and accepted prerequisites

Controller design and integration ownership remain unchanged. A fresh isolated worker owns core `src/prediction.rs`, optional private `src/prediction/snow.rs`, `src/presentation/family_audio.rs`, audio fields/reservation in `src/presentation/mod.rs`, audio staging/commit in `src/presentation/assembly.rs`, registered prediction/audio/frame tests and their narrow fixture registrations. Other source, guides, planning, status, native/runtime/server/Godot files are read-only. Prerequisite correction contract is source `0bb8104bcdb916a1a337aead3ce8855d72fa1b72`, integrated `5fe1f5901`; whole-publication gate is source `71e83c14a90cf127e00c177eae8078476bde511a`, integrated `db5508d64`.

## Compile-ready snow facts before consumer

Use Go `packages/client/cmd/mornlea/app/app_snow.go` as the behavioral oracle: f32 horizontal displacement, first sample silent, position baseline updated on every sample, displacement above 8 silent without changing accumulated travel, airborne silent without accumulating or clearing travel, threshold 0.8 resets travel before checking loaded snow IDs85..88. No movement journal entry alone implies snow.

`PlayerProjectionState` gains private `predicted_snow_cues: Vec<u64>`, a read-only `predicted_snow_cues() -> &[u64]`, and checked `with_predicted_snow_cues(Vec<u64>) -> Result<Self, ClientError>`. A cue must name a non-acknowledged entry of this state's journal, be nonzero and unique, and the vector may not exceed MAX_PREDICTION_JOURNAL. Existing constructors default to empty cues. This is internal semantic input, not a wire/frame schema change.

`PredictionReplay` owns a private stride accumulator, bounded pending cue sequences, and once-per-frame sample flag. Add `observe_snow_step(foot_block: Option<u16>) -> Result<(), ClientError>`: use the owner's final predicted/confirmed f32 position and ground bit; call at most once between begin_frame calls; absent authority is silent. Emit only the latest nonzero sequence still pending in this owner, at most one cue per frame. Check selected queued_input_events and prediction_journal caps before committing accumulator/cue changes. `consume_snow_cues(count) -> Result<(), ClientError>` validates a FIFO prefix before draining. Projection carries the retained source sequences; ordinary correction/rejection removes facts whose pending source disappeared, never samples sound during replay. First ready/reset/not-ready authority and epoch reset clear the accumulator/facts. begin_frame resets the sample flag, not unpublished cues.

Land this callable source contract and concrete source tests as the first commit; report SHA and nonempty red/green results, then wait for controller acceptance before changing family_audio. Tests exercise real PredictionReplay and actual native physics environment: first/stationary/dry/unloaded/airborne/teleport silent; accumulated grounded snow emits once; repeated sample in one frame silent; correction replay creates no new fact; reset clears. Retain existing correction transactional tests. A compile-only test is not source/provider acceptance.

## Audio consumer and bounded staged history

After accepted source SHA, project_audio reads only predicted_snow_cues, preserving current confirmed/local attribution and cancellation semantics. Update stale journal-implies-sound fixtures to supply concrete accepted source facts; test plain journal without facts stays silent and actual accepted snow fact sounds exactly once.

Extend internal AudioDedupDelta with private `retirements: Vec<AudioDedupKey>` and checked `with_retirements(Vec<AudioDedupKey>) -> Result<Self, ClientError>` plus read-only getter. Only Predicted keys may retire; other classes reject. project_audio proposes retirement of held predicted dedup/cancellation keys whose sequence is no longer in the current non-acknowledged journal, and any newly cancelled key with no live source. All confirmed/local IDs remain entire-epoch history. No eviction window is invented.

prepare_publication constructs a complete replacement AudioProjectionState in the reservation: retire obsolete predicted keys, remove cancelled committed keys, insert nonduplicate keys, retain pending cancellation only while it is not retired. Normalize actual vectors before charging. The combined history/cancellation count is capped by selected family_records, and owned Vec capacities times size_of::<AudioDedupKey>() must fit selected frame_bytes with checked arithmetic. Reject Capacity before mutating any owner. Staging permits exactly N and rejects N+1, including tighter caps and cancellations. commit_publication swaps the prebuilt audio state by move; no Vec push/extend or allocation in commit. The full gate, attribution checks, input/lifecycle/event cursors and last visible Arc remain unchanged on refusal.

Tests: history N plus a new unique confirmed event rejects even though the new frame itself fits, with every owner unchanged; duplicate does not grow history; cancelled obsolete predicted metadata retires while arbitrary confirmed event IDs remain held; exact byte cap/plus-one rejects atomically; successful reservation includes complete audio replacement and commit performs only swaps/drains. Worker commits consumer/history as separate coherent nodes and records evidence in /private/tmp; controller alone integrates/statuses/rolls back.

Commands: locked workspace core prediction_replay/presentation_contract filters, then full core suite and Clippy all-targets -Dwarnings. No dependency, lock/toolchain, version, native process, server or remote write. Controller composes observe/consume calls into the actual endpoint only after these source/provider SHAs are accepted.

Controller refinement: publication rejects cancellation keys other than Predicted with InvalidInput. Confirmed and Local dedup IDs live for the entire epoch; only no-longer-live Predicted source metadata can retire. The staged reservation owns normalized complete next audio state before any publication owner changes.
