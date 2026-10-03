# Shared nonplayer Snow speed contract

Node3.7l3ns0; predecessor source f9afd5b93a88cda9b5017c96177191bebb77beba and accepted closure8d3c774b16bbcb3a1f49037e48f514511bdd5172. ROOT owns decisions/status/integration. Fresh isolated author and exact-SHA read-only reviewer use GPT-6.1-sol/medium at the frozen planning SHA. This independently reviewable deliverable accepts a callable common scalar boundary before three actual companion/hostile/passive native consumers; it accepts none of those integrations. No protocol/save/FFI/version or player-motion change.

## Source and chosen representation

Go shared/physics/snow_slowdown.go:51 gates grounded/nonzero horizontal intent, reads one raw floorXYZ foot cell and scales copied WalkSpeed by float32(0.7) for Snow87/88 only, before sweep/native. Actual Go companion_action.go:130, hostile.go:287 and passive.go:289 all use this path. Exactly three Rust nonplayer NativePhysics exits omit it; player_motion already conforms. Independent census /workspace/scratch/f2-nonplayer-snow-slowdown-source-facts.md and canonical snow-cover are authorities. Footprint travel/settlement is separate.

Reuse accepted core::actor_placement::PlacementWorld and AuthorityReadView adapter (current raw in-tick writes before Ready base, source outside-height AIR), private actor_placement::snow_cell checked floorXYZ without GroundProbe/support, mornlea_domain::{Dimension,BlockPos}, core::contracts::{ChunkKey,ServerError}, mornlea_engine::native::contracts::physics::PhysicsTuning. Tuning has fifteen f32 fields; fourteen other than walk_speed remain bit-exact. No new reader trait, actor field, allocation or retained owner. Public additive internal Rust export permits the contract-only landing without dead-code exemptions; it is not a transport/native ABI.

Exact new core/actor_snow.rs API and production algorithm:

```rust
pub fn apply_snow_slowdown(
    world: &impl PlacementWorld,
    dimension: Dimension,
    position: [f32; 3],
    on_ground: bool,
    move_intent: [i8; 2],
    tuning: PhysicsTuning,
) -> Result<PhysicsTuning, ServerError> {
    if !on_ground || move_intent == [0, 0] {
        return Ok(tuning);
    }
    let cell = snow_cell(position)
        .map_err(|_| ServerError::InvalidInput { field: "actor" })?;
    if !(-64..320).contains(&cell.y()) {
        return Ok(tuning);
    }
    let mut result = tuning;
    if matches!(world.block_at(dimension, cell), Some(87 | 88)) {
        result.walk_speed *= 0.7_f32;
    }
    Ok(result)
}
```

Quiet guards precede invalid geometry/read. Qualified XYZ checks precede the height guard and any reader call. Existing geometry refusal maps to nonplayer InvalidInput actor. Out-of-height unchanged before the reader is equivalent to actual source Dimension.BlockAt AIR, even if a double fabricates Snow outsideheight. Insideheight exactly one raw read; None/AIR/nonSnow/85/86 unchanged. No readiness/height scan, fluid/jump/sprint/sneak exclusion, velocity/input-range validation, nativecall, state mutation or caller-policy change. Hard bound three checked floors/one block read, no heap/retained memory; no full-loop50ms/allocation acceptance claim.

## Three exact editable paths

S=packages/engine/crates/mornlea_server. New S/src/core/actor_snow.rs (function, six private tests/own helpers); S/src/core/mod.rs (only insert pub mod actor_snow; after actor_projection); S/AGENTS.md (one narrow paragraph after actor_placement and before pending_restore covering copied tuning/raw-foot ownership/bounds/separate integrations). All other old bytes/helpers/tests/algorithms unchanged. Workers do not edit OpenSpec/status/Go/native/Cargo/lockfiles/contracts/seals/audit exemptions. ROOT owns export integration and the later shared-contract identity.

## Six fully specified tests

Own private tests::CountingWorld implements PlacementWorld with response:Cell<Option<u16>>, trace:RefCell<Vec<(Dimension,BlockPos)>>. block_at appends supplied dimension/cell then returns response; ready_revision and ready_column_height panic. Test-only trace allocations do not imply production allocation. Own tuning() uses explicit fields in PhysicsTuning declaration order: fixed_delta_seconds0.05/step_height0.6/walk_speed4.3/ground_acceleration25/ground_deceleration30/air_acceleration5/jump_speed8/gravity24/terminal_fall_speed56/fluid_gravity4/fluid_sink_speed3/fluid_ascend_speed4/fluid_horizontal_drag2/sprint_speed_multiplier1.3/sneak_speed_multiplier0.3. Own bits() returns all fifteen to_bits values in that order; compare entire returned tuning to input bits with index2 replaced by the stated expected bits, and assert original input remains exact. Expected constants are independent Go source-f32 arithmetic, not production-helper-derived results.

Append exactly six core::actor_snow::tests names:

| Name | Concrete input and expected result |
|---|---|
| slowdown_raw_tiers_preserve_tuning | OW[2.5,64,3.5], grounded[1,0]; response tableNone/0/1/3/35/85/86/87/88/65535. Only87/88 return walk4040a3d7 (3.009999990463257), other rows4089999a (4.3). Repeat87/88 with basewalk17.25 bits418a0000, expected41413333 (12.074999809265137). Every call one exact trace(OW,(2,64,3)), all other fields/input exact. |
| slowdown_quiet_without_geometry_or_reads | Some87; ordinary[2.5,64,3.5] plus each axis independentlyNaN/+Infinity/-Infinity/f32::MAX and[2147483648,64,3.5]. Each position with !grounded intents[0,0]/[1,0]/[0,-1]/[1,1], and grounded[0,0]. Exact base returned, empty trace. Single baselineGREEN control. |
| slowdown_checked_geometry_refuses_without_reads | Some87, grounded[1,0], otherwise[2.5,64,3.5]; each axis independentlyNaN/+Infinity/-Infinity/f32::MAX/-f32::MAX/2147483648/-2147483904. InvalidInput actor, zero trace, input exact. Also invalidX combined with outsideheightY/invalidZ, and invalidY with validX/Z. No clamping/wrapping. |
| slowdown_foot_cell_dimension_and_height | Some87, grounded[0,-1]. Inheight rowsOW[2.5,64,3.5]->(2,64,3), Depths[-1.25,64.75,-16.125]->(-2,64,-17), OW[-2147483648,64,2147483520]->(-2147483648,64,2147483520), OW[2.5,-64,3.5]->(2,-64,3), Depths[2.5,319.99,3.5]->(2,319,3): one supplieddimension/cell trace,4040a3d7. First row neverreadsY63 proves noGroundProbe. Otherwisevalid[2.5,Y,3.5] withY-64.125/320/2147483520/-2147483648: base/zero trace despiteSome87. InvalidZ atY320 refuses beforeheight guard. |
| slowdown_current_unknown_observations | One world OW[2.5,64,3.5], grounded[1,-1], response sequenceNone/87/86/88/0 ->4089999a/4040a3d7/4089999a/4040a3d7/4089999a. Every call originalbase; exactlyone new equal trace entry, remaining fields/input exact. Double proves fresh scalar reread, not actual Ready/write integration. |
| slowdown_independent_consumer_values | Three typed caller examples labelledcompanion/commonwalker-hurler/passive: OW/Depths/OW; [2.5,64,3.5]/[-1.25,64.75,-16.125]/[8.5,1,8.5]; grounded intents[1,0]/[0,-1]/[-1,1]. Some88 produces4040a3d7 and exactlyone suppliedcell read; thenSome0 with originalbase produces4089999a for each. Sourceinput exact. These are common callable examples, not actual provider/native consumers accepted. |

RED adds callable apply_snow_slowdown returning Ok(tuning), export and all six cases, with no production algorithm/callers yet. All six compile/run; expected five intended assertionRED(1/3/4/5/6), one quietGREEN(2). Missingimports/compile/setup/provenance failures are notRED. Preserve inert source tar/diff/hash/rawlogs/discovery+execution identities/nonsecret exact commandmetadata beforeGREEN. Contract/recipe/count discrepancy returnsROOT, no silentpolicy/testchange.

## Validation and closure

1. ROOT verifies prior acceptance/tasks/ledger/untracked/mtimes. Independent read-only readiness must establish source/API/error/arithmetic/ownership/recipes READY before frozen planningSHA/author dispatch. Shared-contract SHA is this node's accepted resulting source, used verbatim by all later consumer packets; no deferred contract choice.
2. Fresh isolated author, exactthreepaths, inactive privateCargo cache reuse; immutable evidence outsideCargo. Source /workspace/.mornlea-env/env.sh in nonlogin shell; Rust1.97.1 --locked/jobs4/GOMAXPROCS4/defaultthreads/deadlines. Before Rusttests copy/hash BOTH native .so into privateCargo release AND worktree packages/engine/target/release: engine0423917bb6209cb565e9bfbc471d4a2a9fb1e505ef1cf51fa48b77b3836794ad/clientbb20e1d1e1ff0c2712636928f6e46321251ae127ca32392d3bb8505d50b66aa1. Never overwrite historical stable artifacts. Every actualrun uses /workspace/scratch/f2-child-supervisor.py followed directly by commandargv, without --; ownedchildrenonly.
3. Exact sixcase discovery: cargo test --manifest-path packages/engine/Cargo.toml -p mornlea_server --locked --lib core::actor_snow::tests -- --list; correspondingactualrun throughsupervisor gives5assertionRED/1GREEN then6GREEN. Existing --lib core::actor_placement::tests discovery/run28 and --test server_replay player_motion:: discovery/run20 remain unchanged. ROOT current source discovery verifies these identities beforedispatch; earlier guessed24 and zero-case lib motion discovery are preserved setup diagnostics, not executed tests.
4. Fresh own release build/stable outsideCargo hash beforeprocessfixtures; BOTH MORNLEA_RUST_SERVER_BIN and MORNLEA_SERVER_BIN select it. Immutable previouspackage /tmp/mornlea-f2-previous-recovery-v2-2cmjutz8/previous-runtime.json and previous-server (manifest9fec11cf/oraclee1a98fea/sealedF1d042982d), BOTH AgentPythonselectors=/workspace/mornlea/packages/agent/.venv/bin/python. Author/exact-SHA reviewer freshfull --lib/--test server_contract/--test server_replay/--test local_remote_parity/--test agent_process/--doc and focused persistence live_acquisition/live_chunk_saves/chunk_driver/chunk_encoding/chunk_retirement/actor_projection/source_player_restore. Expected1226distinctactual+3docs =368lib+275contracts+452replay+40parity+85focusedpersist+6Agent; topic/newrepeatsnotadded; discrepancyreturnsROOT.
5. AlltargetClippy-Dwarnings/Cargo fmt --all --check/diff, actualGo1.26audit go test ./... -count=1 from packages/audit, OPENSPEC_TELEMETRY=0 exact /workspace/.mornlea-env/cache/npm/_npx/833275e9fcb28e00/node_modules/.bin/openspec validate --all --strict --no-interactive128. IsolatedGo approved GOFLAGS=-mod=readonly -buildvcs=false/recordedownGOWORK, no exemption/sourceedits. Exactlythreepaths; two oldfiles reconstructexactly after removing assignedexport/newparagraph. Existing251seals1332occurrences225wholefiles unchanged; repositorywide NativePhysics/PhysicsRequest/rawSnowreader/derivedconsumer/commentID census confirms3futureactualnonplayer exits/no hidden caller. Englishcomments withouttaskIDs.
6. Scoped clean author commit feat(server): add shared nonplayer snow speed contract; no OpenSpec/status/push/subagents. Explicit report/rawlogs/argv/cwd/UTCstart/end/exit/sourceSHA/diff/native/previous/current/selectors/SHA256manifest outsideCargo. Fresh exact-SHA independentreview repeats source/spec/quality/TDD/oldbytes/seals/census/actualgates; ROOT integrates onlyPASS.
7. ROOT full1414actual+3docs =368lib/1binary/6Agent/40parity/272persist/275contracts/452replay, all49activationexplicitcurrentqualifiedprevious; otherworkspace/alltargetClippy/fmt/supportedLinuxquality/Goaudit/OpenSpec/diff. UnchangedF1/Go-race reuse only within preservedsource/provenancescope. Darwin/macOSfullstage remainsopen, noLinuxstop. ROOT closesonly3.7l3ns0, appendledger/design, scopedoriginalbranchcommit/push/freshremoteSHA, thenactualconsumerpacketsagainstacceptedcontractSHA. No forcepush/merge/deploy/F3/client/render/assets.

Rollback removes newmodule/function/six owncases/assignedexport/prose/OpenSpecnode; afteractualconsumeracceptance coordinatedcallerrollback isrequired. Passivefootprint/lifecycle/subscription/publication/save/cache/bootstrap/retirement/configuredtuning/humanmelee/fullgameplay/everyoutcome remainopen. Architecture skill:nochange pending accepted reusable boundary. Plan claims no implementation/test execution/fullF2acceptance.

## Exact command recipe

Author checkout=/workspace/scratch/f2-nonplayer-snow-contract-author, target=/workspace/scratch/f2-snow-author-cargo (inactive cache, old immutable evidence has no mutable Cargo entries). Reviewer checkout=/workspace/scratch/f2-nonplayer-snow-contract-review, target=/workspace/scratch/f2-trample-review-cargo. ROOT=/workspace/mornlea, target=/workspace/mornlea/packages/engine/target. ROOT creates the isolated worktrees only after accepted planning/reviewed source. Worktree role is selected once, not changed during a run. Every command's actual cwd/argv/env/source/diff/start/end/exit/rawhash goes into new role-specific /workspace/scratch/f2-nonplayer-snow-contract-{author,review,root}-evidence. Private targets are not evidence. Preserve default cargo test thread settings/timeouts, no --test-threads or deadline override. From the selected checkout in nonlogin Bash:

```bash
source /workspace/.mornlea-env/env.sh
# ROOT assigns exactly the role's target above before any build/test.
export CARGO_TARGET_DIR=/workspace/scratch/f2-snow-author-cargo
export CARGO_BUILD_JOBS=4 GOMAXPROCS=4
export GOWORK="$PWD/go.work"
export GOFLAGS='-mod=readonly -buildvcs=false'
export MORNLEA_AGENT_PYTHON=/workspace/mornlea/packages/agent/.venv/bin/python
export MORNLEA_COMPANION_AGENT_PYTHON=$MORNLEA_AGENT_PYTHON
export MORNLEA_PREVIOUS_PACKAGE=/tmp/mornlea-f2-previous-recovery-v2-2cmjutz8/previous-runtime.json
export MORNLEA_PREVIOUS_SERVER_BIN=/tmp/mornlea-f2-previous-recovery-v2-2cmjutz8/previous-server
mkdir -p "$CARGO_TARGET_DIR/release" packages/engine/target/release
cp /workspace/mornlea/packages/engine/target/release/libmornlea_engine.so "$CARGO_TARGET_DIR/release/"
cp /workspace/mornlea/packages/engine/target/release/libmornlea_client.so "$CARGO_TARGET_DIR/release/"
cp "$CARGO_TARGET_DIR/release/libmornlea_engine.so" packages/engine/target/release/
cp "$CARGO_TARGET_DIR/release/libmornlea_client.so" packages/engine/target/release/
sha256sum "$CARGO_TARGET_DIR/release/"libmornlea_{engine,client}.so packages/engine/target/release/libmornlea_{engine,client}.so
run_actual() { python /workspace/scratch/f2-child-supervisor.py "$@"; }
cargo test --manifest-path packages/engine/Cargo.toml -p mornlea_server --locked --lib core::actor_snow::tests -- --list
run_actual cargo test --manifest-path packages/engine/Cargo.toml -p mornlea_server --locked --lib core::actor_snow::tests
# The preceding command first observes inert5RED/1GREEN, later6GREEN.
# Freeze inert archive/raw evidence before implementing the prescribed function.
cargo test --manifest-path packages/engine/Cargo.toml -p mornlea_server --locked --lib core::actor_placement::tests -- --list
run_actual cargo test --manifest-path packages/engine/Cargo.toml -p mornlea_server --locked --lib core::actor_placement::tests
cargo test --manifest-path packages/engine/Cargo.toml -p mornlea_server --locked --test server_replay player_motion:: -- --list
run_actual cargo test --manifest-path packages/engine/Cargo.toml -p mornlea_server --locked --test server_replay player_motion::
cargo build --manifest-path packages/engine/Cargo.toml -p mornlea_server --bin mornlea-server --release --locked
```

Copy/hash the resulting binary to own role evidence/stable-release/mornlea-server (outsideCargo), then set BOTH binary selectors to that exact absolute file before the commands below. Record its hash, qualified native and previous manifest/executable/verifier identities. Command recipe above's author target line becomes the exact assigned reviewer/ROOT target for those roles; ROOT avoids the same-file native copies and rehashes existing qualified deployed libraries instead.

```bash
export MORNLEA_RUST_SERVER_BIN=/workspace/scratch/f2-nonplayer-snow-contract-author-evidence/stable-release/mornlea-server
export MORNLEA_SERVER_BIN=$MORNLEA_RUST_SERVER_BIN
for suite in lib doc; do
  cargo test --manifest-path packages/engine/Cargo.toml -p mornlea_server --locked --"$suite" -- --list
  run_actual cargo test --manifest-path packages/engine/Cargo.toml -p mornlea_server --locked --"$suite"
done
for suite in server_contract server_replay local_remote_parity agent_process; do
  cargo test --manifest-path packages/engine/Cargo.toml -p mornlea_server --locked --test "$suite" -- --list
  run_actual cargo test --manifest-path packages/engine/Cargo.toml -p mornlea_server --locked --test "$suite"
done
for topic in live_acquisition live_chunk_saves chunk_driver chunk_encoding chunk_retirement actor_projection source_player_restore; do
  cargo test --manifest-path packages/engine/Cargo.toml -p mornlea_server --locked --test persistence_failure "$topic" -- --list
  run_actual cargo test --manifest-path packages/engine/Cargo.toml -p mornlea_server --locked --test persistence_failure "$topic"
done
cargo clippy --manifest-path packages/engine/Cargo.toml -p mornlea_server --all-targets --locked -- -D warnings
cargo fmt --manifest-path packages/engine/Cargo.toml --all -- --check
git diff --check
(cd packages/audit && run_actual go test ./... -count=1)
export OPENSPEC_TELEMETRY=0
/workspace/.mornlea-env/cache/npm/_npx/833275e9fcb28e00/node_modules/.bin/openspec validate --all --strict --no-interactive
```

Reviewer uses own stable-review absolute path; ROOT uses own stable-root absolute path, neverauthor/reviewer binaries. ROOT additionally discovers/runs full unfiltered cargo test --manifest-path packages/engine/Cargo.toml -p mornlea_server --locked (run_actual for execution), including272persist/all49activation; run_actual cargo test --manifest-path packages/engine/Cargo.toml --workspace --exclude mornlea_server --locked; cargo clippy --manifest-path packages/engine/Cargo.toml --workspace --all-targets --locked -- -D warnings; run_actual bash scripts/ci/run-linux-quality.sh. Root original-branch push/ls-remote follows acceptance. Every role separately records test identities and unique totals; discovery does not count as execution. Failed RED commands are captured with actual nonzero code rather than aborting evidence collection; source publication/ACK/native integration is not inferred from scalar cases.
