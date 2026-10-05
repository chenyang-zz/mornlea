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

## Agent boundary

[`src/agent/AGENTS.md`](src/agent/AGENTS.md) owns loopback HTTP, lease, host,
memory, snapshot and MCP lifecycle guidance. The actual `McpService` implements
`McpLifecycle`; successful explicit deadline-aware close proves every admitted
connection and accept join retired. Registry acquisition honors the same
caller deadline; timeout retains snapshots or held tool ownership for same-service
retry. Full Agent-to-authority runtime composition remains separate.

## Persistence boundary

`src/store/AGENTS.md` owns disk worker I/O, decoded loads, durable acknowledgment
and the narrowly confined native-close adapters. Region and standalone providers
consume its cancellation and error contracts before joining the real backend.

## Checked contracts (`src/core/contracts.rs`, `src/core/state.rs`)

- `src/core/actor_placement.rs` owns bounded borrowed restoration, support,
  Safe checkpoint and single-column spawn geometry. It reuses the motion
  collision mapping with source float arithmetic, strict contact, whole-footprint Ready gating and
  distinct complete/any support. Only its read-view adapter normalizes source
  out-of-height air. Complete current Ready non-air heights may skip proved air
  rows; readers without that certificate retain literal source row order.
  Checked spans and enumeration cap reads and allocation. Private source doubles
  and actual compact Ready/current-write fixtures qualify geometry alone;
  lifecycle, subscriptions, full-radius scan cadence and runtime stay caller-owned.
  Private Safe geometry performs at most four horizontal Ready checks before
  at most twelve body and four complete-support cells, with no retained owner
  or heap allocation. It preserves source float collapse, outside-height AIR
  and supported fluid eligibility independently of restore height limits.
  Private trample geometry copies at most four source-f32 support-layer cells
  with checked endpoints and no world reads, body-height or restore predicates.
  Private Snow geometry checks only the source foot-cell floors in axis order,
  without support, height-clamp, probe subtraction or world reads.

- `src/core/actor_snow.rs` owns copied nonplayer speed tuning over the borrowed
  raw foot cell. Quiet guards precede checked XYZ floors; source outside-height
  AIR precedes the single raw read. Thick Snow changes only copied walk speed,
  preserving other tuning bits and caller-owned actor state. Work is at most
  three checked floors and one read, with no allocation or retained owner.
  Private scalar doubles qualify this shared contract; actual companion,
  hostile and passive native integrations remain separately owned.

- `src/rules/passives.rs` privately owns the retained passive Snow producer:
  a 32-slot Option tracker table plus a 32-cell copied pending batch and a
  fixed tick-local owned registry that excludes exactly those residents from
  the legacy single-tick collector. Capture follows valid native motion, the
  home rollback and staging, before death settlement; the late settle drains
  the frozen prefix through the existing crop Snow consumer in bounded
  slices. Admission, newborn and death resets clear only tracker slots, so
  queued coordinates survive; the public batch entry keeps its book-less
  None behavior unchanged.

- `src/core/pending_restore.rs` retains bounded candidate, wanted-key, nearest
  column, fallback and exhausted-revision progress over borrowed placement reads.
  Current waits before Safe; ready columns keep source same-call cadence, and
  downgraded sites survive readiness gaps without revalidation. Stable exhausted
  scans read only sorted Ready revisions. Player restart preserves captured
  anchor/radius and dimensionless spawn retention in the current dimension.
  Private consumer doubles and actual maximum-radius Ready/current-write probes
  qualify this owner; actor lifecycle, reset effects, subscriptions and complete
  runtime workload remain separately owned.

- `src/core/source_player_reset.rs` owns the private checked in-place mapping
  for prepared Active player pairs and completed Player scans. It changes only
  fixed actor/runtime fields, preserves health/hunger and live bed/workbench,
  and leaves durable body and public path allocations untouched in constant work
  with zero allocation. `TickContext::begin_source_player_reset` checks retained
  health, phase, actual source session and indexed pair ownership before mapping,
  then removes only that player's mining/sleep/suppression participation. Earned
  receipts, raw input ACK, durable beds, viewers and the separately borrowed book
  stay untouched. Context point operations are logarithmic; live production
  admission is at most eight, but resident maps may retain history or stale private
  hook keys. Prepared recovery/death doubles, refusal snapshots and allocation/drop
  tests qualify this common context boundary. Active source recovery consumes
  it before oxygen/native motion and restarts the same captured scan in the
  current dimension. Positional lifts change only position and rebase the existing
  pre-step owner. Actual source death fills survival and hunger through this
  same mapping after all five damage producers; the serial book consumer restarts
  the retained scan for the next tick. Late successful-action settlement,
  subscriptions, save eligibility and executable runtime acceptance remain
  separately owned.

- `src/core/source_player_death.rs` prepares bounded fixed inventory and live-bed
  values for one indexed zero-health Active source player. The context guards the
  retained Ready map at the managed ceiling before cumulative ring-drop rehearsal;
  crafting repack refuses as a hard invariant. Both bed halves use the shared
  placement read, including source outside-height AIR independently of acquisition.
  Inventory/drop staging precedes survival refill, own receipt removal and common
  Pending mapping. Durable body/path allocations and independent bed/view owners
  remain retained. The Copy payload leaves completed scan restart with its caller.
  Eight private prepared cases qualify this boundary and abandonment semantics.
  The serial source book consumer excludes early and late legacy death, settles
  after starvation, drowning, native fall, melee and projectile damage, then
  restarts each same captured scan for next-tick advancement. Three private
  consumer cases and seven actual disk/login/native death recipes qualify this
  routing, including live beds and the retained hard-failure fence. Executable
  acceptance remains separately owned.

- `src/core/source_player_restore.rs` owns explicit source-mode background-login
  registration and at most eight live initial player scans. Missing saves retain
  only metadata fallback; loaded Current precedes supported Safe. Pending actors
  have an explicit runtime and source-rounded checked respawn coordinates. The
  central reducer gates ordinary commands before raw input ACK and deferred roles,
  then advances restoration after Acquire and before player survival. Whole-book
  move and recovery preserve scan allocations on success, trusted failure and
  unwind; retirement prunes only that live book. Local readiness/reset follows
  the existing final projector. Private authority cases and four real disk,
  background mailbox, Memory handshake/ACK and ChunkDriver Acquire recipes in
  `tests/persistence_failure/source_player_restore.rs` qualify initial restore
  with manual wants. Four additional actual disk/login/Acquire/native recipes
  qualify active recovery: native fall and current-dimension captured-anchor
  reacquisition, first free positional lift, blocked search and unknown footprint.
  Recovery preserves nondeath state and retains the same scan until next-tick
  advancement. Actual source death settles serially after all five damage
  producers and before passive advancement, preserving the captured anchor and
  current dimension for next-tick restoration. The same entry owns its restarted
  scan; waiting ticks never repeat inventory drops or survival refill. One keyed
  ever-spawned lookup qualifies Safe immediately after each player's native
  motion and fall settlement, before later players and late death. The indexed
  context updates only the existing heap-free Safe field, preserving body/path
  allocations and the same scan; prior accepted checkpoints survive later
  context abandonment. Actual healthy/lethal landing and top-floor native
  recipes qualify this owner. A keyed ever-spawned qualification captures actual
  airborne-to-grounded edges after native/fall settlement and before Safe. One
  private fixed 32-cell batch retains copied coordinates through death/reset and
  abandoned context loans, without cloning actor bodies or scan allocations.
  Successful late settlement drains only its length through existing crop transactions;
  missing environment retains the prefix. This bounds only candidate retention.
  One inline travel/cell tracker per entry and an eight-cell copied Snow batch
  preserve source stride and dimensionless memory across ticks. Capture follows
  trample before Safe/death; accepted resets clear only the tracker. Capacity
  preflight preserves both tracker and accepted prefix on refusal; settlement
  drains length only after fresh ordered Snow transactions succeed.
  Sorted ever-spawned registrations settle late action costs after Interaction
  and Mining through scalar-only indexed context borrows. Each accepted player
  consumes only its own bounded receipts; later refusal preserves that scalar
  prefix and foreign receipt order until context Drop.
  Subscriptions, publication, automatic actor save/cache
  eligibility and executable runtime acceptance remain separately owned.

- `src/core/actor_projection.rs` borrows one settled actor and optional fixed
  inventory/runtime overlays to produce checked existing storage values. Player
  current pose/survival, inventory/armor and hunger/respawn come from their
  current owners; safe and identity stay in the body. Companion projection
  mirrors the private body codec checks without fabricating lifecycle aggregates.
  Hostile/passive body health and hostile combat/target fields stay canonical;
  only hostile burn/distance use runtime. Every refusal is `actor_save`, and no
  input changes. It owns no lifecycle selection, revision allocation, target
  retention, ACK, bootstrap or runtime flush. Private field tests and the actual
  Memory login/native tick/background disk recipe in the persistence actor
  projection topic qualify this helper, not live actor save acceptance.

- `src/core/step.rs` shares one fallible phase engine between `reduce_tick`
  and `AuthoritativeFinalReducer`. The final adapter uses full budgets and
  commits successful resident/viewer work without delivery; `run_final` owns
  once-on-success consumption and endpoint advancement. Hard phase/delivery
  errors and trusted Rust unwinds retain the first error and fence the authority
  in Closing. Exclusive context drop returns partial resident/dirty ownership
  before the moved schedules and source player book return; it provides no whole-tick rollback.
  Failed authorities refuse new captures, selection and metadata targets while
  preserving already-selected immutable ownership and completion/return paths.
  Both shutdown entries permanently stop at failed FinalTick, including a
  retained I/O error; ordinary injected lifecycle failures keep their existing
  transient classification. Cold abort/quiescence and real executable runtime
  composition remain separate acceptance work. `core::state::failed_tick_tests`,
  `core::step::routing_tests`, and the actual-final case in
  `tests/server_contract/tick.rs` pin these boundaries.

- `src/core/deferred_commands.rs` retains at most 4096 immutable command
  envelopes per tick under their full tick/session/sequence/arrival identity.
  Each provider phase has a separate ordered list of at most 4096 ordinal
  references, preserving repeated same-phase delivery and shared roles without
  extra payload ownership. Conflicting same-key payloads refuse before mutation;
  reads are nonconsuming and all ownership ends with `TickContext`.

- `AuthorityState::settled_read` borrows the existing read view from healthy
  committed owners in constant construction work, without resident copies or
  pending ingress. The first retained tick failure fences new reads before
  Closed validation; healthy Running and Closing remain readable. Its tick is
  the next executable endpoint, while absolute time selects an explicit World
  mirror, committed environment, then startup metadata. The immutable loan
  excludes exclusive tick execution. Enumeration retains its existing costs
  and eligibility semantics. Private identity/clock/health tests and the actual
  Memory login/native motion/save/reopen projection case qualify this boundary;
  Agent, source publication and actor-target ownership remain separate.

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

- `src/core/chunk_encoding.rs` owns the checked off-tick network factory and
  narrow `ChunkEncodePort`. `EncodedChunkSnapshot` retains the exact supplied
  `ChunkSaveView` token, its section-only payload charge and one canonical
  `PreparedFrame`; equal numeric revisions cannot correlate captures. The port
  fixes eight total queued/started/held requests across one or two CPU owners,
  started-cancellation retention and retryable explicit join obligations.
  Factory and executing contract doubles do not qualify actual threads or a
  source publisher. Ready/wanted/current-capture eligibility stays with the
  later publication consumer; supplied Unloading captures are not filtered.

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
  per-cell CAS counters remain tick-local. Current Ready block edits also own
  a persistent fixed-page tree. `core::world::chunk_view_tests` pins path-copy
  work, page coverage and bounded current-root occupancy. Undo and save
  captures share immutable roots.
  `ChunkSaveView` equality is capture identity, and `capture_chunk_snapshot`
  copies only fixed slots and shares the base/root; it performs no scheduling.
  Disk `materialize` expansion runs off tick on the store owner and retains
  Direct15 sections covered by pages and fixed physical slots. Off-tick
  `ChunkSaveView::network_snapshot` returns checked domain sections with exact
  unchanged loaded storage; genuinely changed sections compact in first-appearance
  YZX order. Its byte charge counts section payloads only (air is 48 bytes),
  separately from the persistence estimate (air is 4096 bytes). Captures add only
  a fixed section-change union and validity bit, sharing the same immutable
  base/pages without frequency owners or a decoded body. Invalid private edits
  and zero capture identities refuse before expansion. `network_view_tests`
  pins these bounds, capture independence, logical codec roundtrips and actual
  unchanged Go `BuildChunkSnapshot` parity. Counter-only slots may
  produce distinct captures under the same revision, so revision-only caching
  is forbidden. Drop slot mutations share that one
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
  clone/finalize behavior. Defensive compounds retain initial preimages only
  for touched map keys, actor ordinals and appends, dirty membership and changed
  scalar lanes. Ready copies share compact bases; only projectile arms copy the
  capped vector. Direct parts, aggregate writes, captured containers and
  read-basis cells each have an independent 4096 ceiling checked before
  rehearsal or capture. The journal excludes touched actor search and record
  vector clone costs and does not bound all tick work.
  `owned_resident_tests` and `ready_commit_tests` enforce these invariants.

- `src/core/block_observations.rs` owns sparse observations grouped by exact
  chunk key. Resident/context/read views preserve full-key order, block values
  and per-cell CAS counters across durable commits; chunk revisions do not
  replace that history. Production loans move the collection, while explicit
  off-tick observations may clone it. Whole-chunk detachment transfers one
  existing tree without visiting cells; later retirement policy owns destruction.
  Touched-cell compound restoration prunes empty owners, and detached fixture
  replacement drops only its exact owner off tick. This representation does not
  perform physical unload or establish a global allocation bound.

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
  with the ordering/admission layer, mirroring Go's layering. Accepted
  settlements that staged a changed patch — plus an accepted equal
  `EquipArmor` swap — also mark the tick-local owner-only dirty publication
  lane, so the projection emits one final owner `InventoryState`; refusals
  and the idempotent re-select mark nothing.
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
  record. The tick-start freeze preserves the committed checked
  `EnvironmentState` tunables; only the metadata fallback seeds
  `RuleTunables::source_defaults()`, and every provider consumes that same
  frozen record — no CLI configuration loading exists.
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
  (regression-guarded).
  `TransportSessionPort` limits borrowed transport authority to submit/close;
  full endpoints retain those calls through explicit blanket delegation.
  Tick and shutdown ownership remain with the complete runtime.
  `src/transport/live.rs` supplies the actual borrowed endpoint over one
  `AuthorityState` and its existing `PlayerLoadPort`. `LoginDriver` retains
  at most sixteen uncommitted aliases, never save bodies or committed history.
  Ready installs once; only the common core's acknowledged success activates.
  A refused cancellation retires Prepared capacity while retaining its alias
  for bounded same-owner retry. `cancel_pending` freezes admission, visits the
  original aliases in ticket order, and preserves independent successes.
  Runtime shutdown and store close remain with their complete owners.
  Play-frame wire envelopes and connection reaping
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
  against the pre-step snapshot and direct-fixture action receipts) and fall
  damage, death-once with bed-preserving respawn,
  and damage-event emission that the sleep node consumes for wake. The
  serial reducer must construct contexts from pre-motion authority so the
  pre-step snapshot holds. Source players consume bounded late Till and human
  Mining receipts after their two source regions through indexed scalar borrows,
  reusing the same arithmetic without replaying survival timers or motion.
  Sorted live registrations preserve accepted scalar prefixes on later refusal;
  unconsumed receipts keep order until context Drop destroys their transient lane.
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
  one tier per captured stride sample before random sampling. Crop removal and its actual
  output batch commit together after the separate ground write. Environment
  must exist before collecting or draining landing events. Actual source-player
  capture owns a private fixed coordinate batch before Safe and late death;
  the late consumer reuses these unchanged transactions with fresh reads. Legacy
  collection excludes actual source sessions and preserves disabled fixture behavior.
  Actual source players retain their Snow tracker and fixed eight-cell batch in
  the moved book; checked capture precedes Safe/death and late fresh settlement
  needs no environment. Resets clear only the tracker and preserve queued cells.
  Legacy Snow excludes actual source sessions. The reducer recreates the generic
  schedule each tick; passive Snow lifetime, order and rounding remain open.
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
  Successful human completion preflights the bounded charge lane before its
  transaction and earns one Mining receipt after progress clears; companion,
  incomplete and refused work earns none.
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
  a Unix host. Compatible rollback runs the actual sealed Go all-family
  read-only verifier against current quiescent saves, so valid Rust storage
  mutations need not equal activation-time bytes. Restore verifies the installed
  backup through the same oracle before Go starts. Package identity is checked
  before stopping Rust; an already live previous owner is only probed. The
  control-only executable and storage-consumer proof do not qualify gameplay
  restart or repair the outstanding lock-span handoff boundary. A crash between
  the previous start and its manifest record refuses as `writer_live`; clear the orphan owner
  and re-run rollback.

The activation suite requires explicit `MORNLEA_PREVIOUS_PACKAGE` pointing to the
accepted sealed `previous-runtime.json` and `MORNLEA_PREVIOUS_SERVER_BIN` equal
to that record's `previous_executable`. Missing or empty fixtures fail, with no
build, PATH discovery or skip. `CARGO_BIN_EXE_mornlea-server` selects the actual
freshly compiled Rust binary; `MORNLEA_RUST_SERVER_BIN` is the explicit fallback
and is required by the script self-test. Set an isolated `CARGO_TARGET_DIR` for
focused work, and use an owned child supervisor on Linux when executing the
activation suite or self-test. `activation_verifier.rs` covers real current-save,
unloaded-family, restore/resume, immutable-package and unsafe-output consumers.

## Managed live chunk acquisition

`core/acquisition.rs` owns the opt-in lifecycle book, opaque reservations and
whole prepared-result transfers. Enabling requires an empty resident world and
legacy completion queue; disabled replay and sparse fixtures retain their
existing behavior. Load and generation aliases occupy separate lanes of eight
attempts, so equal numeric provider IDs coexist. The book retains at most 36660
wanted keys, 36676 managed records and sixteen staged results, with one checked
global generation scalar and no consumed-request or per-key history.

The actual Acquire rule consumes prepared owners after companion intent and
before player physics. Ready bases, fixed drop/container owners and source
durability facts are prepared off tick and moved into resident ownership.
Central reads and block/container/drop preflight require the managed Ready
phase. Forgotten successes retain their body and durability facts as Unloading,
unavailable to gameplay; rewant restores the same identity. Capture permits
Ready and Unloading for a later durable consumer. This lane performs no save,
provider cancellation, request driving or reclamation. Those serial consumers
must preserve this book's ownership and typed load errors. Private bounds and
tick clone/materialization counters live with the owning modules; actual disk
and native-generation consumer cases are registered in the persistence failure
live-acquisition topic.

`AuthorityReadView::live_collision_block` is the crate-private managed collision
read. Enabled acquisition returns AIR outside [-64, 320) before horizontal
readiness, without creating a mutation observation or trace entry. Disabled
sparse fixtures keep observation lookup at every coordinate. In-height reads
reuse current writes, Ready gating and the existing bounded trace refusal policy.
The added height branch allocates nothing; delegated observation retains its
existing lookup and trace costs. The private `live_collision_read_tests` group
covers boundaries, dimensions, retained Unloading ownership, actual current
transactions, read-view agreement and trace overflow. Its grid and geometry
consumer doubles qualify the input contract only. The player, companion, passive
and hostile actor-grid producers consume this read for actual native motion.
Sixteen height cases in the persistence source-player-restore topic qualify real
player login/acquisition, checked non-player provider admission, missing-column
blocking and disabled sparse controls. Player recovery, actor bootstrap and
physical terminal retirement remain separate integration work. Validate this
group, the full library and the inherited server, persistence, Agent, doctest,
Clippy and formatting gates before integration.

`core/chunk_driver.rs` borrows the sole scheduler/load owner and independent
native generation owner. It reserves authority before starting each port and
retains accepted starts before binding, with eight current records per source
and at most sixteen polls per pass in source/key order. A current key fences
both source maps before another start. A duplicate or impossible
binding quarantines the original request without cancellation or polling an
ambiguous alias. Failed offers retain the whole original event; successful
transfers keep authority's staged lane charged until Acquire. Typed source
failures become authority Failed facts at that phase, never automatic generation.

The driver selects no subscription, priority, retry or backpressure policy.
`stop_new` leaves bound polling available for a final Closing batch. Its borrower
still drives the existing scheduler owner and owns stop/cancel/wait/close;
dropping the driver proves no cancellation, quiescence or native join. The
persistence failure chunk-driver topic executes actual background DiskStore and
GenerationPool start/poll/tick recipes without manual successful offers. Held
load tests gate entry to the actual DiskStore read, not a native read syscall,
and release gates and explicitly close/join owners before reporting assertions.


Managed live saves use the acquisition book's ordered dirty index and eight
exact immutable flights. Unloading targets precede Ready autosaves; the first
candidate retains the source budget exception. Current dirty estimates and
held capture estimates remain separately charged when a tick commits newer
state. Selection, stats and qualified acknowledgments neither scan clean bodies
nor materialize them. Whole completion validation precedes durability updates;
only an exact held preimage advances current persisted facts, and an older ACK
leaves newer dirty/rewrite facts intact. Fresh refusal releases that exact flight
and recaptures the current target. Failed completion retains the original flight
for the scheduler's existing retry/backoff owner. Clean Unloading bodies enter
a key-local retirement index. Explicit
`AuthorityState::retire_unwanted_chunks` admits at most eight whole Ready,
fixed-slot and sparse-tree owners to the detached disposal port after latest
durability, unwanted, no-request and no-flight checks. Book erasure follows
successful admission; refusal restores the exact owner and preserves accepted
prefix progress. Admission proves residency transfer, while scalar collection
and explicit worker close prove disposal and join. Rewant before erasure retains
generation; actual reload after erasure allocates a fresh global generation.
Future schedule dues, viewer/workbench references and external immutable captures
remain unchanged. This consumer does not establish a global allocation bound or
wire the executable caller. Manual `remember_dirty` injection is
checked and restricted to disabled acquisition fixtures; enabling refuses any
existing fixture dirty or in-flight ownership.

## Prepared publication contract

`core::publication::PreparedFrame` owns one checked canonical protocol key and
immutable encoded bytes in an Arc. Its existing-encoder factory runs off tick;
clones and prepared transfers share the allocation without retaining a codec,
decoded body or authority borrow. Debug exposes metadata only. The crate-private
legacy-byte observation is off tick and transfers a unique allocation or copies
a shared allocation; actual prepared publication and transport must not call it.

`PreparedPublicationPort` declares explicit Queued/Closed receipts and whole-frame
FIFO budgets. Only Queued permits a caller to advance its source mirror revision;
it acknowledges queue admission alone. The first whole frame may exceed the byte
budget, while a zero frame budget takes none. AuthorityState owns one VecDeque
of PreparedFrame per session, shared by legacy and prepared publication, with
current Prepared/Active membership bounded to
eight keys independently of retained retired history. Prepared admission validates
Play identity before lookup, admits only Active/open queues and synchronously
retires a saturated receiver while retaining its queued prefix. LiveEndpoint
forwards the narrow port directly, and Memory drain_prepared_session moves owners
without legacy byte conversion or control-frame acknowledgment. Actual peer
acknowledgment remains the shared connection core's distinct lifecycle step. The
`server_contract::prepared_publication` executing double proves consumer semantics
and owned transfer; it does not accept the actual authority outbox or transport.
Factory tests separately execute the real protocol codec and family decoders.

Prepared TCP delivery uses `TcpTransport::forward_prepared_outbox` on the same
concrete borrowed `LiveEndpoint`. Committed connection identity and the existing
512 shared send slots are checked before taking authority owners. The returned
count records whole-owner transfer, never peer receipt or mirror advancement.
The Prepared lane borrows canonical bytes and bypasses core acknowledgments;
partial writes retain the exact owner under the existing 64-attempt/1 MiB drain
budgets. Completed core acknowledgments settle before a later fatal write closes
the socket and returns its original error. No codec or compatibility byte copy
runs in this transfer. The local/remote prepared-delivery topic executes the real
background DiskStore, login driver and socket. Private queue tests label their
injected writers and immediate load fixture separately from that actual-store
evidence. These bounds are per authority FIFO and socket, not a bound on retained
historical sessions or acceptance of a source publisher/background encoder.

`core/encoding_worker.rs` owns the bounded off-tick chunk framing lane: at most
eight queued, started and held-complete captures across one or two independent
protocol codecs on OS threads. Replies preserve exact capture identity; started
cancellation suppresses delivery but remains charged until real collection.
Caller start/poll/drive only transfer bounded owners. Explicit close cancels,
drains, disconnects and joins; a timeout retains handles for same-pool retry,
and an owner teardown panic remains a sticky join failure. Drop disconnects
without claiming quiescence. Private causal gates and fault wrappers execute
the actual codec and are separate from ordinary encoding evidence; the
persistence chunk-encoding topic executes actual disk acquisition, capture,
CPU framing, authority FIFO and Memory owner transfer. This lane decides no
Ready/wanted relevance or mirror advancement and accepts no source publisher
or executable runtime.

`core/chunk_retirement.rs` owns the opaque noncopying detached chunk contract.
One `RetiredChunk` moves the exact Ready, fixed drop/container and sparse tree
owners into a private box; metadata identity uses the managed key/generation.
Every refusal returns that whole owner for exact restoration. The public port
charges at most eight queued, started and held completions until FIFO scalar
collection. Close requires disposal, collection, disconnection and explicit CPU
join; timeout retains charges and handles for retry, and teardown panic remains
sticky. Its private executing double proves ownership and lifecycle obligations,
not actual threads, physical unload or authority eligibility. The production
managed consumer moves and restores these exact private body
owners through the authority retirement operation.

`core/retirement_worker.rs` implements the detached-owner port with exactly one
CPU thread and a worker-side whole-body FIFO. Caller ownership contains only
eight scalar identity/completion records; queued, started and held reports stay
charged until actual destruction and FIFO collection. Refusal returns the whole
incoming body. Deadline-aware close drains, disconnects and explicitly joins;
timeout retains charges or the join handle for same-owner retry, and worker
failure stays sticky without releasing lost obligations. Drop only disconnects,
so queued bodies never run destructors on the caller. Private causal gates and
panic probes execute actual destruction and remain separate from ordinary
factory evidence. Authority unload eligibility belongs to the managed consumer;
the persistence
failure retirement topic executes actual disk acquisition, qualified durable
ACK, CPU disposal collection and fresh-generation reload. Executable caller
coordination remains separate.
