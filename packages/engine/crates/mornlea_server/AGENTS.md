# Authoritative server

`packages/engine/crates/mornlea_server` owns the Rust authoritative server:
session admission, tick staging, save ownership, and the agent host boundary.
OpenSpec behavior for this crate lives in
`openspec/changes/rust-authoritative-server/`. The crate is a windowless rlib.
Production dependencies are the F1 crates `mornlea_domain`,
`mornlea_protocol`, and `mornlea_storage`, plus `mornlea_engine` for checked numerical kernels and
`PhysicsTuning`; pinned `serde_json` handles the existing backup identity JSON.
Native descriptor close uses target-scoped `libc` on Unix and
`windows-sys` on Windows; all other unsafe code remains denied. It does not own
GPU rendering, Godot presentation, or the
Python Agent process. Workspace membership is `packages/engine/Cargo.toml`.
No dependency-direction test guards this crate yet; review the manifest
against this file.

## Persistence boundary

`src/store/AGENTS.md` owns disk worker I/O, decoded loads, durable acknowledgment
and the narrowly confined native-close adapters. Region and standalone providers
consume its cancellation and error contracts before joining the real backend.

## Checked contracts (`src/core/contracts.rs`, `src/core/state.rs`)

- `AuthorityState::try_new_with_metadata` validates startup save metadata before
  any provider starts. `try_metadata_snapshot` captures only resident world
  time, day display offset and weather; startup seed, anchors, salt and
  difficulty remain fixed. Distinct targets advance a checked process-local
  sequence and snapshots own immutable bodies. The scheduler uses the fallible
  `SaveAuthority` capture for cadence and final flush, retaining pending work
  on refusal. Legacy `metadata_snapshot` reads only the last captured target.

- `src/core/generation.rs` owns off-tick seed-compatible terrain preparation.
  One `ChunkGenerator` retains independent Overworld/Depths checked parameters,
  native scratch and a fixed dense destination, then returns an owned compact
  chunk with first-appearance palettes and inactive fixed slots. Private
  `src/core/go_random.rs` preserves the Go RNG/Shuffle stream and BSD notice.
  Generation owns no authority, storage, request identity or publication;
  acquisition supplies revisions and validates Ready preparation separately.

- `src/core/generation_worker.rs` owns at most eight queued, started or held
  generation requests across one or two OS owners. Each retains independent
  native scratch and prepares Ready bases off the tick. `GenerationPort`
  transfers results whole; started cancellation retains the charge until the
  real reply is collected. Deadlines preserve work and join handles for retry;
  only explicit successful close proves every owner joined. No disk, authority
  or transport handle crosses this boundary; live acquisition remains separate.

- `src/core/world.rs` validates compact Ready chunk bases and derives non-air
  heights off the tick. Read views use exact chunk/cell indexing and observe
  sparse writes first; missing chunks never become air. Height changes are
  immediate, including transparent blocks. Replay snapshots preserve compact
  unchanged sections and advance each changed chunk's durable revision once;
  per-cell CAS counters remain tick-local. Drop slot mutations share that one
  durable increment; counter-only age/delay changes preserve the source dirty
  selection behavior. Actual chunk acquisition and commit
  ownership belongs to the serial reducer.

- `PreparedChunk` in `src/core/world.rs` carries a checked Ready base with
  independent persisted revision, rewrite and recovery facts. Preparation runs
  off the tick and installation moves that base. `ChunkLoadPort` and the existing
  `PlayerLoadPort` describe bounded asynchronous ownership; contract doubles
  qualify their values and calls, while the store provider and live consumers
  require separate integration acceptance.

- `src/core/drop_store.rs` owns the 32 persistent drop slots per chunk, including
  inactive generations, and their slot-ordered active view. Batches accept at
  most 36 inputs and rehearse merge/split/capacity on one fixed copy. Ordered
  compound effects retain only affected chunk copies and publish them after
  all other arms succeed. Drop patches compare full preimages, preserve identity
  and permit only same-item count reductions or removal plus counter changes.
  Fixture replay treats compact slots as the identity source; float centers are
  observations and never reconstruct an already loaded slot. Full-range stored
  chunk coordinates follow source int32 wrapping then float32 center rounding.
  Providers preflight with read().check_drop_batch and must revalidate at commit.
  Mining and system block outputs use exact integer source cells rather than
  round-tripping float centers. Other producer integration remains with its
  provider.

- `src/core/container_store.rs` owns fixed Ready furnace/chest slots keyed by
  dimension and chunk. Wire references remain Overworld-only; internal reads
  and `WorldContainer` effects carry dimension separately. Mining captures the
  actual active slot and exact preimage, clears it while retaining generation,
  and settles all outputs with the block write. Placement reserves the lowest
  reusable inactive slot and skips exhausted generations. Compound effects
  rehearse affected arrays before publishing; counters and contents share the
  block/drop chunk revision. Sparse fixture containers never override a Ready
  miss. Inventory/container transfers use one compound to conserve items when
  a durable write refuses.

- Production `TickContext::for_tick` exclusively moves resident maps out of
  authority for the serial reducer. `commit_carried` finalizes only keys with
  accepted block/drop/container changes and moves residents back; one key shares
  one durable revision. An abandoned loan restores current ownership and dirty
  identity without finalization or publication, including the original absent
  sleep record unless touched. This is recovery, not global tick rollback.
  Detached harnesses and explicit `resident_snapshot` observations keep their
  clone/finalize behavior. Defensive compound snapshots remain until a separate
  bounded journal replaces them; this change does not bound all tick work.
  `owned_resident_tests` and `ready_commit_tests` enforce these invariants.

- `ServerLimits`, `TickBudget`, and `StoreLimits` reject an over-ceiling
  constructor before they reserve command storage. `AuthorityState` keeps
  world, sessions, queues, tick, and publication private to `src/core/state.rs`.
- `TickContext::stage` validates every component of a compound effect before
  it applies any component, previewing ordered projectile insert/replace/remove operations in
  that compound. `contract_double::compound_rejects_partial` and
  `contract_double::compound_projectile_insert_is_atomic` require a
  second-component failure to leave every earlier component absent.
- Projectile effects compare exact preimages, preserve IDs on replacement and
  enforce the 128-record cap in compound order. `read().projectiles()` exposes
  an immutable overlay. Damage effects retain at most 4096 ordered intents;
  compound refusal preserves both lanes. Fixture initialization and snapshots
  preserve actor runtime and projectiles across replay steps. Damage settlement
  and real reducer persistence remain separate provider responsibilities.
- `AuthorityState::publish` encodes `TickPublication` events with
  `ServerPacket::try_from(Event)` and encodes every control packet through
  `ProtocolCodec::encode_server_into` before appending. A conversion or
  encode refusal returns `ServerError::InvalidInput` for field `packet` and
  appends none of that publication.
  `contract_double::event_publication_reaches_outbox` requires the encoded
  non-login event in the outbox.
- `SubmitSaveError` returns the refused `SaveRequest`. `SaveCompletion`
  echoes the submitted key and revision.
  `contract_double::completion_returns_ownership` pins both.
- `ServerEndpoint::shutdown` returns `ShutdownFailure` with the same report
  the authority retains. `contract_double::shutdown_failure_retains_report`
  retries the failed phase and does not replay a completed final tick.
- `Clock::monotonic` and `Clock::unix_ms` stay separate.
  `contract_double::clock_units_separate` rejects a wall-clock value as a
  monotonic deadline.
- `src/core/session.rs` owns the session provider over the state ports:
  `admit`/`submit`/`close_session`/`apply_sorted_batch` (admission capacity,
  control-plane split, retire-once, freeze-sort-apply with silent stale
  discard). `src/core/mailbox.rs` owns the bounded mailboxes:
  `budgeted_batch` (positional budgeted prefix, identity-preserving carry,
  watermark walk), `admit_chunk_result` (admitted / cancelled-discarded /
  duplicate-discarded classification via `chunk_discard_counts()`), and
  `cancel_chunk_request` (one-shot tombstone). `src/core/publication.rs`
  owns tick delivery: `publish_tick`, `drain_outbox`, `close_receiver`; a
  saturated 512-frame outbox silently retires only that receiver with
  `CloseReason::SlowReceiver` inside `AuthorityState::publish` — no
  Disconnect frame is appended and publishing never blocks.
  Outboxes own complete canonical protocol frames, including packet IDs.
  Memory and TCP transfer the same bytes. TCP checks committed connection
  ownership and remaining send slots before draining authority ownership;
  caller-supplied packet identities and post-drain framing are forbidden.
- `src/core/shutdown.rs` owns the retryable shutdown machine over the frozen
  lifecycle ports: `shutdown` drives the 18-phase resumable sequence — stop
  admission, one final unpublished tick, authority and Agent-lease freeze,
  worker quiesce, Agent memory finalization, ordered family flush, store sync
  as the durability barrier, Agent release with frozen-lease retention,
  same-lease retry and expiry skip, then the Agent/MCP/store/worker closes.
  Completed phases are never replayed and a failure keeps the failed phase as
  the retry boundary with the lease and resources retained.
  `AuthorityState::drive_shutdown` is the contract-landing scaffold behind the
  consumer double's endpoint; the real endpoint rewires to this provider at
  the common transport integration node. Both entries freeze the actual Agent
  before memory work, stop/cancel/wait workers, and share the pending-memory
  deadline barrier. Production persistence composition remains separate.
  `MemoryOwner` retains semantic operations across attempts, reconciles exact
  remote state before retrying a commit, and counts refused cleanup joins
  independently. Actual Python/SQLite shutdown tests lose a confirmed response
  and prove reconciliation precedes flush and release without a second commit.
  Absent-ID local retirement is idempotent; first worker panic and live timeout
  retain their diagnostic meanings.
- `src/core/mutation.rs` owns the authority-resolved world transaction: the
  four `resolve_*` functions build complete private `BlockTxn`s — footprint
  from the frozen Go block tables (door/bed two cells, cross-chunk), selected
  item/tool wear with the Go exemption predicate, actor-specific output
  preflight (human bounded drop merge/capacity rehearsal, companion
  inventory credit) and captured container slots — and commit only through
  `MutationTxn::{try_place, try_mine, try_system, try_system_with_drops}`. No client-supplied target
  or revision reaches commit; a refusal leaves world, inventory, containers,
  tool and events unchanged. The `resolve_*` stubs at the end of `state.rs`
  remain the consumer double's scaffold until the endpoint rewire; the
  per-`SystemRule` replacement tables belong to their rule providers.
- `src/rules/inventory.rs` owns the inventory authority provider: ordinary
  inventory/armor settlement for `SelectHotbar`/`MoveInventory`/`EquipArmor`/
  `MovePartial`/`QuickMove` over whole-record `InventoryPatch` staging —
  stack moves with cap/remainder/swap, partial halves, the four-phase
  quick-move credit, armor points 2/6/5/2 with wear only on actual reduction
  (fall damage bypasses). Item conservation is the invariant and a refused
  settlement stages nothing; sequence gating and actor-lifecycle checks stay
  with the ordering/admission layer, mirroring Go's layering.
- `src/rules/tools.rs` owns authority-ray hoe, bone meal and bucket commands.
  Soil/crop/water writes and selected-item debit use the same atomic placement
  transaction. Receipt and tick-local mining-suppression capacity are checked
  before mutation; only successful buckets publish PlacementSuccess. Command
  calls carry no separate actor key. Ready-session retirement filtering belongs
  to the serial reducer and is not provided by an Active actor record alone.
- `src/rules/projectiles.rs` owns bow draw, resolved projectile birth and
  ID-ordered flight. Each impact atomically removes the projectile and settles
  target health, armor, runtime interruption and knockback before the next
  projectile selects a target. Settled impacts never enter the pending damage
  lane. Spawn evicts the minimum ID only after validation; world observation
  is capped at 512 cells plus one numerical-only completion probe. The reducer
  supplies Ready subscription squares and owns hostile ranged spawn decisions,
  sleep victim routing, later death/reset and persistence.
- `src/rules/environment.rs` owns the end-of-tick environment provider: it
  advances world time exactly once per tick (saturating, never wrapping),
  keeps sleep as a `day_phase_offset` display change only, and rolls weather
  and season through the frozen Go dice (SplitMix64 salts, segment/duration
  intervals, restore-zero preserving kind and regenerating duration). The
  mirrored constants and KAT literals reproduce the Go engine exactly;
  staging goes through `RuleEffect::Environment` plus the world publication
  record.
- `src/core/companion_ingress.rs` owns the sessionless companion candidate
  admission: whole-payload validation (provenance, digest, generation,
  source tick, finite yaw, world-Y target bound) before a global four-slot
  inbox reservation — every refusal leaves index and counters untouched, and
  a human session is unavailable by type (sealed envelope). Duplicate
  identity is pending-inbox-scoped; the tick boundary re-enforces it. The
  `state.rs` `submit_companion` scaffold is per-companion and must be
  replaced, not composed, when the endpoint rewire lands.
- `src/rules/world_acquisition.rs` owns the chunk-acquisition gate: keyed
  wants (dimension+chunk in the map key, generation+request in the want
  record) with consumed-first ordering so a drained-then-repeated completion
  refuses `AlreadyConsumed`, stale generations and not-wanted or superseded
  requests refused state-preservingly, and the 64-result ready cap refusing
  whole. Accepted completions consume as rule-level records with
  `PhaseReport` counts — the frozen `RuleEffect` has no chunk arm and the
  authority keeps no chunk store, so payload landing and the
  apply-once-before-physics wiring belong to the serial reducer node.
- `src/transport/common.rs` owns the one shared transport admission path
  both adapters call: frame decode, hello validation, a 16-deep prelogin
  reservation (the seventeenth refuses `Capacity`), login admission through
  the frozen prepare/install/activate lifecycle with send-acknowledgment
  gating the single activation, and S2 coalesced ingress with its exact
  byte cap. `TransportAuthority` and `HandshakeLimits` are declared here
  with plan-exact signatures because the contract landing did not place
  them; the declaration file moves only if a consumer outside the transport
  subtree appears. A failed load never double-retires its prepared session
  (regression-guarded). Play-frame wire envelopes and connection reaping
  belong to the adapter nodes.
- `src/rules/world_mutation.rs` owns placement geometry on the
  `Interaction` phase: current-look ray plus selected slot settled only
  through the accepted 1.6 transaction, door/bed two-cell footprints with
  one revision per changed chunk, same-tick first-wins contention, and an
  internal door toggle that writes the lower cell only (Go-verified; the
  upper half stays). Reach refusal is pinned; sneak refusal waits for the
  movement node that stages held controls. Provider-level gameplay
  rejections collapse to a single error shape — the serial reducer owns the
  final contract for how they surface.
- `src/rules/player_motion.rs` owns player motion on the `PlayerMotion`
  phase plus `PlayerCommand` control intake: latest validated held controls
  win per session (invalid clears), one authoritative advance through the F1
  physics/collision kernels at dt 0.05 with tunables snapshotted, and the
  held input staged into `ActorRuntime.controls` through the declared
  `read.runtime` lane for the Interaction-phase sneak gate (pinned by the
  reducer node). Poses are bounds-checked against the int32 edge before the
  kernels ever see them.
- `src/rules/player_survival.rs` owns survival on the three actor phases:
  regen/starvation with the exact counter gates, pre-physics oxygen and
  drowning, post-physics exhaustion from motion charges (jump/swim/sprint
  against the pre-step snapshot, mining/till/melee through the action
  receipt seam) and fall damage, death-once with bed-preserving respawn,
  and damage-event emission that the sleep node consumes for wake. The
  serial reducer must construct contexts from pre-motion authority so the
  pre-step snapshot holds.
- `src/core/player_publication.rs` owns the live tick-end private player
  projection after final movement, actions, damage and climate settlement.
  It derives mining, armor, saturation and per-height temperature from final
  resident values. The reducer replaces provisional fixture poses, orders
  private observations before hits, and consumes reset after capturing it.
  Input acknowledgment is session-owned, recorded before semantic control
  validation and retained across idle ticks independently of command ordering.
- `src/rules/fluids.rs` owns boundary rescans before updates over the F1
  `NativeFluidEval` kernel: snapshot the 7-neighborhood before writing,
  strongest-merge with sorted writes, then one transaction per target,
  requeue at now+5 under the frozen due order, 512 per sorted dimension,
  and the section-aware 65536/4096/4095 rescan ceilings. The provider
  carries work in a caller-owned `FluidSchedule`; the serial reducer owns
  tick wiring. Plant replacement atomically publishes source-compatible
  outputs; a drop-capacity refusal leaves that plant and retries its
  neighborhood without discarding independent successful water writes.
- `src/rules/eating.rs` owns atomic eating on the `Eating` phase: hold 32
  with the start counting 1 and `(slot, item)` continuity, interrupt
  precedence structural over settlement, and the one-compound settlement
  (inventory, hunger, saturation with the frozen food table, progress
  reset). Progress is transient only; the eating-ticks constant mirrors the
  frozen tunables value (no getter exists on that field).
- `src/rules/furnaces.rs` owns tick-driven furnace advancement on the
  `FurnaceStep` phase: sorted-unique interest advancing each furnace at
  most once, pause-without-waste on invalid input or full output, ignition
  at burn 0 (coal, 1600, same-tick −1/+1) and the 200-tick completion,
  smelt rows through the single domain table, and exact restart from
  stored values with no wall clock. Interest reaches the provider as an
  explicit slice through the same batch-entry/body split as chunk
  acquisition; the serial reducer owns the interest set and viewer
  publication.
- `src/rules/crafting.rs` owns workbench crafting on the lifecycle phase
  plus `MoveCrafting`/`TakeCraftingOutput` admission: the exact 25-recipe
  frozen table (sealed in the capability inventory), trim-preserving-holes
  matching with mirror only per flag, nonzero-durability never an
  ingredient, and one whole-record patch per settlement — take refuses
  before staging on full output or failed repack rehearsal, close repacks
  or refuses. The command-close grid-size drop belongs to the container
  provider's `CloseContainer`; the bench-anchor recheck arm waits for a
  contract surface that can express the anchor.
- `src/rules/farmland.rs` owns bounded moisture checks on the `Farmland`
  phase: one check per candidate, the 162-read neighborhood reservation
  before any scan, events staged before rescan work, x-fastest/z/y cursor
  order, and the shared 65536/65536 budgets with original-due retention on
  deferral. The cadence is event-driven (enqueue due = current tick) — no
  periodic re-check exists in the Go source; the dry-revert roll belongs to
  the random-rules node.
- `src/rules/passives.rs` owns the passive lifecycle on the
  `PassiveStepDeaths` phase: global 32 with one daytime candidate per tick
  anchored by session-order rotation, local 6-in-48 on grass, the exact
  priority chain (swim/shore/flee/graze/tempt/idle/wander), temptation at
  8-inclusive stopping at 2.5, and grazing as a 20-tick event settling
  grass-to-dirt through the transaction with the frozen sampler salts.
  Death stages the terminal record with no loot (drops are a later node);
  flee arming belongs to the combat nodes.
- `src/rules/crops.rs` owns actor footprints on the `Trample` and
  `SnowFootprint` phases: landing-edge detection with the 0.6-width strict
  2x2 AABB, farmland/crop settlement through single-write transactions with
  the frozen capacity preflight (full capacity keeps the whole cell
  silently), and the one exceptional second-write fault mirroring Go's
  ordered two-commit stage — never a two-write transaction. Snow reduces
  one tier per landing before random sampling. Crop removal and its actual
  output batch commit together after the separate ground write. Environment
  must exist before collecting or draining landing events.
- `src/rules/sleep.rs` owns sleep settlement on its batch phase: authority
  bed-ray entry with the Go refusal order, the exact seasonal morning
  transition (`EffectiveMorningOffset` with `DayArcTicks`/`YearPhaseAt`
  mirrored branch-for-branch; the absolute clock never moves), the wake
  matrix (move/jump/actual damage wake; look and sprint alone never),
  disconnect eligibility, and bed-record retention (clearing proven-missing
  rows is the respawn path's). Sleep state is reducer-carried — the
  furnace-precedent split; the reducer prunes respawned/disconnected
  sessions on the non-transition path.
- `src/rules/hostile_actors.rs` owns the hostile lifecycle on the motion
  and burn-distant phases: global 64 with one night-window candidate per
  tick anchored by session-order rotation (never uuid order — the target
  tie alone uses uuid bytes), the frozen hash/radius/axis chain with the
  low-byte<13 admission gate and the separate ≤7 block-light BFS, local
  walker/hurler caps, and 600-tick distant removal staged as a terminal
  record (set removal is the serial reducer's; the read view's linear
  block scan is a reducer-node throughput concern).
- Tick-local successful bucket receipts suppress mining only in the current
  context (at most eight player keys). Tools check suppression and exhaustion
  receipt capacity before their transaction, then record success with the same
  exclusive context. No suppression flag enters saved actor state.
- `src/rules/mining.rs` owns continuous mining progression on the
  `MiningStep` phase: key-unchanged increments to saturation, key change
  restarts at 1, completions through the 1.6 transaction exactly once, human
  failure clearing vs companion-full retaining saturated progress, the
  human-only snow clear without drop capacity, and bow-draw suppression.
  Harvest outputs use the shared deterministic samplers at completion; an
  empty output list imposes no drop-capacity gate. Human door/bed footprints
  clear atomically, while companions retain the source single-cell door
  asymmetry. Companion output credit precedes selected-slot wear, including
  newly credited durable tools. Workbench is excluded from crop predicates.
- `src/rules/containers.rs` owns exact Ready container opens, close previews,
  transfers and whole-stack panel drops through `settle_command`. Open binds
  the actual fixed slot/generation immediately; transfers require that live
  lease. The context seeds one complete viewer set and never falls back to a
  closed committed lease. Close reclaims only extended workbench cells before
  clearing the view. Output slot 38 is a legal drop source, never a transfer
  destination. Inventory, container and drop writes settle in one compound;
  every successful explicit container patch durably touches its chunk even
  when its slots remain equal. Reach invalidation belongs to publication after
  transfers. The serial reducer owns net viewer commit, retirement pruning and
  real phase scheduling; workbench anchor mutual exclusion remains separate.
- `src/store/scheduler.rs` owns save scheduling over the accepted mailbox:
  completions before due retries before urgent before cadence autosave (latched
  until dirty and in-flight clear), saturating-tick backoff 20…1200 with
  oldest-first cohorts, the unsaved-byte ceiling with 90%-exit hysteresis,
  unload retention, and a deadline flush that never half-applies. Selection
  and retry compose the frozen `SaveAuthority`/`StoreHandle` seams only.
- `src/transport/memory.rs` owns the in-process adapter: owned frames move
  through the shared connection core with codec-only envelope framing, the
  S1 outbox drains FIFO without direct world mutation, success commits only
  on acknowledgment, and the 513th frame retires only the slow receiver. The
  adapter struct holds no authority handle, so the frame path is the only
  mutation path.
- `src/transport/tcp.rs` owns the loopback TCP adapter: real sockets with
  ephemeral ports, fragmented-frame reassembly with partial-arrival proof,
  virtual 5 s/10 s expiries by clock advance only, peer-reset recovery with
  continued session numbering, and slow-receiver isolation end to end. The
  same transcript produces identical session IDs and event order on both
  adapters (proven by the joint parity run, not by reading the sibling).
- `src/store/mailbox.rs` owns the durable store mailbox over the frozen
  `StoreHandle` surface: admission through the frozen `try_admit` lane/job
  caps and the 4 MiB reservation ceiling (a chunk reserves the compressed
  maximum plus envelope before encoding and shrinks to the actual length
  after), one ownership ticket counted once across queue, workers and
  completion, whole-request return on every refusal, per-key durable
  revisions only after the backend commit, and stale-ticket completions
  reported without clearing newer in-flight work. `poll_tick` performs no
  I/O; the real disk backend is a later node and this module's tests drive
  an explicitly named backend double.
- `src/agent/http.rs` and `src/agent/lease.rs` own the Agent HTTP wire and
  the lease machine: loopback-literal endpoints with closed-schema
  validation and the exact body/response/header byte budgets, the frozen
  Absent→Active→Frozen→Closed lease graph with checked control revisions
  (a late outcome never revives a fence; heartbeats preserve it), and the
  Go-parity MCP gate requiring an explicit nonzero port. Request-id
  minting is test-seeded SplitMix64 — production crypto-grade parity is a
  host-node question, and the frozen `AgentRequest` union carries no
  Live/Ready probes yet.
- Topic modules under `src/rules/`, `src/transport/`, `src/store/` and
  `src/agent/` that are not named above stay registered and empty of
  behavior. Later nodes own them. The actual tick reducer and Agent host remain
  unaccepted; real transport and disk providers require their later endpoint
  integration before full server acceptance.

## Consumer double (`tests/server_contract/contract_double.rs`)

The executing double is not server acceptance. It proves the declared ports
accept and reject owned values:

- `contract_double::valid_receipt` queues a sequenced intent without applying
  its sequence, accepts chat and keepalive as control, and returns
  `StaleSession` after retire.
- `contract_double::all_ports_type_flow` drives the declared port methods on
  one authority.
- `contract_double::load_prepare_install_send_activate` keeps a prepared
  session from accepting play until activation, and reuses a retired player
  slot.
- `inventory::capability_inventory_rows_are_unique_and_complete` checks the
  frozen capability inventory rows. It does not execute them.
- `tests/server_replay.rs`, `tests/persistence_failure.rs`,
  `tests/local_remote_parity.rs`, and `tests/agent_process.rs` register their
  topic files with `#[path]` and stay empty of cases. Integration-test crate
  roots resolve modules beside the entry file, so a same-named subdirectory
  is not found without `#[path]`.

## Focused verification

From the repository root, with Rust 1.97.1:

```bash
rustup run 1.97.1 cargo metadata --manifest-path packages/engine/Cargo.toml --no-deps --format-version 1
rustup run 1.97.1 cargo test --manifest-path packages/engine/Cargo.toml -p mornlea_server --test server_contract --locked -- --list
rustup run 1.97.1 cargo test --manifest-path packages/engine/Cargo.toml -p mornlea_server --test server_contract --locked contract_double
```

`src/rules/random_blocks.rs` owns deterministic section sampling and ordered
crop, dry farmland, tree, grass and snow updates. Its active set is sorted and
deduplicated, includes Ready chunks only, and validates complete coordinate
spans before mutation. Each sample sees current writes and heights; NativeTree
preflights the whole bounded footprint before one transaction. Start-of-tick
climate remains fixed while sequential block observations advance. The serial
reducer owns interest selection and placement of this phase in the tick.

`src/rules/drops.rs` owns bounded lifetime and pickup over a caller-provided
Ready interest set: sort/deduplicate at most200 input keys, validate at most8
Active players before mutation, increment wrapping age and decrement delay,
expire before pickup, then credit in session order within the source f32 radius.
Whole inventory/drop compare-and-replace settlement preserves crafting repack
capacity. Counter-only changes retain source non-dirty semantics. The reducer
owns active interest, producer ordering and publication; this provider does not
accept those integrations by itself. Checked Q and inventory/crafting panel
commands read the current authoritative source and atomically debit it with a
foot-position output. The crate-visible foot preparation helper checks Active
lifecycle, bounded coordinates, Ready ownership and exact merge capacity before
any source change; container settlement reuses it. Command providers retain typed
refusals for the later reducer, and never trust client quantities or positions.

`src/rules/harvest.rs` owns allocation-free source harvest dice. Crop yield
streams include completion tick; short-grass seeds and extra leaf saplings omit
tick entirely so a retry cannot reroll them. The scalar dimension is pure hash
input, not a world-admission surface. Mining and environmental consumers own
which output stacks are permitted and must settle them with their block writes.

`src/rules/supports.rs` owns four ordered pass-entry snapshots of actual
successful block changes: short grass, saplings, torches, then beds. Preloads
and no-op writes never enter the private changed ledger; compound rollback
restores it. Each removal sees current world writes, and item-producing
removals atomically stage their exact outputs. Unavailable support observations
follow each source predicate; paired bed removal refuses an unavailable
counterpart. The reducer invokes this provider after ordinary world writes.

`HostileMeleeBatch` is a bounded immutable tick-local input contract. It retains
exact hostile IDs and process-local target sessions, validates unique attackers
before allocation and orders its owned copy by hostile ID. Saved chase UUIDs
never authorize attacks. The producer owns pre-physics selection and earliest
valid action handling; combat owns current identity, cooldown and post-motion
range validation. Contract examples do not accept those production providers.

`src/core/interaction.rs` owns world-aware interaction target classification.
Air and fluids pass through; open lower doors pass through, and an upper door
uses its same-dimension lower form with missing/nonlower fallback closed. Target
classification grants no collision, support or harvest permission. Callers
retain their own original-cell readiness/error policy, including projectile
flight's distinct handling. Interaction consumers must reuse this classifier.

## Opt-in activation and rollback

The opt-in runtime qualifies one disposable world copy for the Rust server
without changing default startup. `src/bin/mornlea-server.rs` proves one
claim: this exact process exclusively owns the named world and answers the
local control plane. `scripts/rust-server-opt-in.sh` drives `activate` and
`rollback` around it, `testdata/runtime-migration/server/activation.json`
pins the manifest shape, and `tests/persistence_failure/activation.rs`
executes the real workflows against the rebuilt Rust binary and the real
previous Go binary. Default `Makefile`, Go command, and product entry stay
untouched; the script self-test and the activation suite assert that.

- The binary accepts `--world`, `--listen`, `--activation-manifest`,
  `--control-socket`, and `--dry-run`. It binds the loopback game port as a
  reservation, matches the manifest nonce and world path, acquires the OS
  world lock, loads the stored metadata through the real standalone reader,
  then serves `status` and `shutdown{deadline_ms}` (deadline at most 30000)
  with `{nonce,phase,final_tick,world_closed}` on the local socket. It runs
  no ticks, admits no sessions, and rejects every other op, so no world
  action endpoint exists. Orderly shutdown closes the store, reports
  `Quiescent`, records the manifest, and exits so the lock releases exactly
  once; a close failure retains `StopRequested` with `shutdown_failed`. The
  binary never hashes executables; the script validates binary hashes before
  start and on resume, while the binary binds the nonce, world, and lock.
- The script confines the world, backup, and run directory inside the run
  directory under a system temporary tree, refuses leaf symlink aliases,
  validates both binary hashes and the named backup identity before start,
  and configures the previous binary explicitly, never through `PATH`. The
  versioned manifest carries runtime, source, executable, previous binary,
  protocol and save identities, world and backup paths and tree hashes,
  control socket, pid, start nonce, lease identity, and phase.
- Phases run `Prepared`, `RustRunning`, `StopRequested`, `Quiescent`,
  `DataVerified`, `PreviousRunning`. Rollback requests real Rust shutdown,
  waits for process exit, proves lock reacquisition, then either preserves
  current bytes under the compatible policy or reinstalls the verified
  backup through the resumable stage, retire, install renames under the
  restore policy; a shutdown timeout or error retains `StopRequested` and
  refuses the previous start. Crashes resume idempotently from the manifest
  by checking pid liveness never alone but control nonce, hashes, and lock
  state together. Failure codes are `invalid_manifest`, `identity_mismatch`,
  `writer_live`, `shutdown_failed`, `incompatible_save`, `backup_mismatch`,
  `restore_failed`, and `previous_start_failed`, each retaining the last
  proven phase with no dual writer. Dry-run reports the selected Rust
  binary and hash but creates nothing and qualifies no cutover.
- Tree digests agree byte for byte between the script and the suite: regular
  files sorted by `/`-joined relative path, each contributing path bytes,
  one zero byte, the little-endian content length, then the content. World
  trees skip `world.lock`; backup and staged trees additionally skip the
  backup identity record.
- Known opt-in limitations live here until revisited before any default
  consideration: mob disk seeding, chunk streaming, despawn projection, and
  the production gameplay transport (the binary reserves the game port and
  serves only the control plane with `final_tick` zero). The script requires
  a Unix host. Wiring the offline previous-runtime verifier binary into the
  compatible policy is separate controller-owned work; until it lands, that
  policy checks manifest identities and hashes, preserves current bytes, and
  proves compatibility through the previous binary's own load plus verified
  login, seed continuity, and sole lock. A crash between the previous start
  and its manifest record refuses as `writer_live`; clear the orphan owner
  and re-run rollback.
