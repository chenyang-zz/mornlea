# Rust authoritative server — Superpowers implementation plan

> **For worker agents:** implement one linked node per assignment. This is the only checkbox/status source. Read [execution-contract.md](execution-contract.md) and the linked packet; do not design a missing API or start implementation before task **1.1** records complete F1 acceptance. The controller integrates inventory assets, manifest fragments, status, and ledger.

**Goal:** move authoritative server behavior to Rust with offline Go parity, shared Memory/TCP validation, bounded persistence, and reversible opt-in activation.

**Architecture:** one `mornlea_server` core over F1 contracts and `mornlea_engine`, with transport and Agent adapters that never bypass admission. See [design.md](design.md) and [spec](specs/rust-authoritative-server/spec.md).

**Tech stack:** Rust 1.97.1, Go 1.26 oracles, protocol v45 and current save schemas unchanged.

**Global constraints:** F2 blocked until complete F1 acceptance (task 1.1). No default startup switch, no dual writers, no production Go rule changes, no Python authority. Each implementation node: concrete failing case first, nonempty `--list` discovery, focused green gate, controller-reviewed scoped commit. Corpus and inventory producers write only to fresh external directories; tracked assets are controller-owned.

**Review focus:** (1) starting implementation without F1 closeout; (2) Memory/TCP diverging validation; (3) missing `server.authority.*` inventory row; (4) blocking I/O on tick hot path; (5) Agent candidate applied without revalidation. Each is pinned in the owning packet.

## 1. Prerequisites and core lifecycle

- [ ] 1.1 [Verify complete F1 acceptance gate](plans/01-evidence.md#node-11-verify-complete-f1-acceptance). Controller-only; bind final F1 SHA or record explicit blocker.
- [ ] 1.2 [Register `mornlea_server` and four integration targets](plans/01-evidence.md#node-12-register-mornlea_server-crate-and-inventory-skeleton). Run `cargo metadata` and `cargo test -p mornlea_server --tests -- --list`.
- [ ] 1.3 [Bounded tick queue, shutdown, and cancellation](plans/01-evidence.md#node-13-bounded-tick-queue-shutdown-and-cancellation). Run `cargo test -p mornlea_server --test server_contract tick::`.

## 2. Authoritative capabilities

- [ ] 2.1a [Realm chunk ingress replay](plans/02-world-replay.md#node-21a-realm-chunk-ingress-and-mutation-commit). Run `server_replay realm::`.
- [ ] 2.1b [Environment and moisture replay](plans/02-world-replay.md#node-21b-environment-and-farmland-moisture). Run `server_replay realm::`.
- [ ] 2.1c [Fluid scheduling replay](plans/02-world-replay.md#node-21c-fluid-scheduling). Run `server_replay fluid::`.
- [ ] 2.2 [Player command and inventory parity](plans/03-player-replay.md#node-22-player-command-and-inventory-parity). Run `server_replay player::` and `server_contract command_reject::`.
- [ ] 2.3 [Hostile, passive, and companion replay](plans/04-actor-replay.md#node-23-hostile-passive-combat-and-companion-rules). Run `server_replay hostile::` and `companion::`.
- [ ] 2.4 [Bounded persistence and recovery](plans/05-persistence-agent.md#node-24-bounded-persistence-and-recovery). Run `persistence_failure`.
- [ ] 2.5 [Loopback Agent adapter](plans/05-persistence-agent.md#node-25-loopback-agent-adapter). Run `server_contract agent::` and `make companion-agent-integration`.

## 3. Transport and authority acceptance

- [ ] 3.1 [Memory and TCP shared ingress](plans/06-transport-authority.md#node-31-shared-ingress-for-memory-and-tcp). Run `local_remote_parity`.
- [ ] 3.2 [Exclusive activation and rollback](plans/06-transport-authority.md#node-32-exclusive-activation-and-rollback). Run `persistence_failure selection::` and `server_contract authority::`.

## 4. Closeout

- [ ] 4.1 [Zero-gap inventory and architecture review](plans/07-closeout.md#node-41-zero-gap-inventory-and-architecture-promotion). Run full named suites and `go test ./packages/audit -count=1`.
- [ ] 4.2 [Stage gates and rollback evidence](plans/07-closeout.md#node-42-stage-gates-and-rollback-evidence). Run `make rust-check`, `make dev-check`, `make test-race`, and strict OpenSpec validate.
