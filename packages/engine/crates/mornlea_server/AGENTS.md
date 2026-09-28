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
- Topic modules under `src/rules/`, `src/transport/`, `src/store/`,
  `src/agent/`, and the remaining non-contract `src/core/` files are
  registered and empty of behavior. Later nodes own them. This crate does
  not implement a world rule, reducer, transport adapter, or disk backend.

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
