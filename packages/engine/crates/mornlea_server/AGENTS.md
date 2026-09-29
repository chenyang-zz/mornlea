# Authoritative server

`packages/engine/crates/mornlea_server` owns the Rust authoritative server:
session admission, tick staging, save ownership, and the agent host boundary.
OpenSpec behavior for this crate lives in
`openspec/changes/rust-authoritative-server/`. The crate is a windowless rlib.
Production dependencies are the F1 crates `mornlea_domain`,
`mornlea_protocol`, and `mornlea_storage`, plus `mornlea_engine` solely for
`PhysicsTuning`. It does not own GPU rendering, Godot presentation, or the
Python Agent process. Workspace membership is `packages/engine/Cargo.toml`.
No dependency-direction test guards this crate yet; review the manifest
against this file.

## Checked contracts (`src/core/contracts.rs`, `src/core/state.rs`)

- `ServerLimits`, `TickBudget`, and `StoreLimits` reject an over-ceiling
  constructor before they reserve command storage. `AuthorityState` keeps
  world, sessions, queues, tick, and publication private to `src/core/state.rs`.
- `TickContext::stage` validates every component of a compound effect before
  it applies any component, counting projectile inserts already accepted in
  that compound. `contract_double::compound_rejects_partial` and
  `contract_double::compound_projectile_insert_is_atomic` require a
  second-component failure to leave every earlier component absent.
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
  the common transport integration node — do not extend the scaffold.
- `src/core/mutation.rs` owns the authority-resolved world transaction: the
  four `resolve_*` functions build complete private `BlockTxn`s — footprint
  from the frozen Go block tables (door/bed two cells, cross-chunk), selected
  item/tool wear with the Go exemption predicate, actor-specific output
  preflight (human staged drops under per-chunk occupancy, companion
  inventory credit) and captured container slots — and commit only through
  `MutationTxn::{try_place, try_mine, try_system}`. No client-supplied target
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
- `src/rules/fluids.rs` owns boundary rescans before updates over the F1
  `NativeFluidEval` kernel: snapshot the 7-neighborhood before writing,
  strongest-merge with sorted writes, one `try_system` commit per batch,
  requeue at now+5 under the frozen due order, 512 per sorted dimension,
  and the section-aware 65536/4096/4095 rescan ceilings. The provider
  carries work in a caller-owned `FluidSchedule`; the serial reducer owns
  tick wiring.
- `src/rules/mining.rs` owns continuous mining progression on the
  `MiningStep` phase: key-unchanged increments to saturation, key change
  restarts at 1, completions through the 1.6 transaction exactly once, human
  failure clearing vs companion-full retaining saturated progress, the
  human-only snow clear without drop capacity, and bow-draw suppression.
  Grass saturates but its seed roll belongs to the random-rules node;
  companion settlement wear belongs to a 1.6 follow-up.
- `src/rules/containers.rs` owns container views on `ContainerMove` plus
  open/close admission: generation/dimension/range-validated leases with
  output 38 never a destination, furnace input-then-fuel priority, repack
  preview before commit, and close invalidating only after successful
  repack. Container commits stage `RuleEffect::Container` and view leases
  stage `RuleEffect::Viewer` through context overlays (both rollback-safe);
  the overlay-to-authority commit leg belongs to the serial reducer.
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
- Topic modules under `src/rules/`, `src/transport/`, `src/store/` and
  `src/agent/` that are not named above stay registered and empty of
  behavior. Later nodes own them. This crate does not implement the tick
  reducer, a transport adapter, a real disk backend or the Agent host.

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
