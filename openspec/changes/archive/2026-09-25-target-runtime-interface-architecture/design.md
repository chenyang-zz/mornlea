## Context

The [target architecture](../../../docs/architecture-target.md) already fixes language ownership. Current source and tests fix the existing Rust domain, protocol and storage codecs; `rust-authoritative-server` and `rust-client-core` remain planning-only. The [interface-first guide](../../../docs/interface-first-parallel-development.md) defines the required compile-ready landing and three evidence gates. This change supplies the missing cross-project target map, not another implementation plan store.

## Goals and decisions

1. **One owner per boundary.** The [catalog](../../../docs/runtime-interface-architecture.md) assigns D0/P0/S0/K0/S1–S4/C1–C2/G1/V1/T1. The allowed dependency graph is domain/protocol/storage/kernel → server or client core → Rust Godot bridge → Python host. Agent service and presentation Python are isolated. Go is an offline oracle and current migration runtime.
2. **Stable spine, versioned families.** Domain commands/events and v45 packet registry remain closed, typed and existing. New presentation capabilities use individually versioned logical families. The only shared control envelope is epoch/revision plus explicit count/byte and identity metadata; no generic dynamic message bus is introduced. Logical family names do not assign new numeric ABI IDs before the exclusive F3 registry landing.
3. **Authority, state and lifecycle.** Server ingress alone assigns session/tick/arrival metadata; `advance_tick` alone owns world mutation. Client core alone owns session epochs, mirrors, prediction, confirmed revisions and atomic frame assembly. Godot owns scene/resource/device lifetime and validates whole batches. Neither Python environment can commit world actions.
4. **Explicit failure policy.** Each boundary maps semantic failure classes to its own typed error; it must validate before mutation, report capacity without truncation, and preserve durable or irreversible outcomes. Existing `DomainError`, `ProtocolError`, ABI statuses, wire and save versions are not replaced. Existing code-pinned caps are cited; unimplemented server capacity numbers are not guessed.
5. **Landing sequence and evidence.** Complete F1 precedes F2 S1. S1 precedes transport/persistence/Agent providers and F2 integration. The accepted F2 session contract is a prerequisite of C1; C2 precedes G1 and per-family P8–P11 work. Each shared landing uses the existing interface-first contract packet, validated fixtures and consumer double before parallel consumers. Real provider and integrated evidence remain separate.

The Rust operation lists in the catalog are interface *shapes*. For example, `ServerCore::submit` means queued input, while `advance_tick` returns authoritative publication; `ClientCore::snapshot` returns one immutable epoch/revision frame. They do not claim an existing crate or make unaccepted F1 types callable. Exact new declarations and queue values are frozen only by the owning stage's accepted contract SHA, with the target map updated on conflict.

## Data and compatibility flow

Host input → typed Godot bridge → client core semantic batch → existing Rust protocol v45 → server session intake → ordered domain command → one authoritative tick → domain events and persistence request → v45 server publication → client mirror/correction → immutable typed presentation frame → Godot feature family. The independent Agent service provides bounded candidates to server intake and is revalidated at a tick boundary. Memory and TCP change only transport, not login, validation or simulation.

No wire, save or existing ABI layouts change in this unit. Future changes require their own versioned delta and compatibility corpus. A new family may be introduced additively only after it has an owner, schema, limits, real producer/consumer and registry identity; major changes to existing family meaning need a major bump. F2/F3 plans and prerequisite gates remain in force.

## Rejected alternatives

- One universal plugin interface or untyped event bus would hide closed domain/protocol coverage and permit silent unknown payloads.
- Freezing new numeric ABI IDs and queue sizes in a document before a producer and measured tests exist would create a false compatibility promise.
- Letting Godot/Python decode packets, own mirrors, or validate Agent world actions would create duplicate authority and lifecycle paths.
- Treating the existing Go pilot bridge as the target Rust producer would report unsupported families as implemented.

## Risks and checks

- **Catalog drift:** root guidance requires future target plans to cite a row; a new feature starts with a versioned catalog/OpenSpec update and contract landing.
- **Planning mistaken for implementation:** target status, per-row existing/target state and explicit gap list remain visible in both languages.
- **Cross-language mismatch:** F3 registry owns one logical-name-to-numeric descriptor mapping; required symbolic audio/lifecycle families fail activation until that mapping exists.
- **Unbounded work:** each stage landing must freeze its own numeric queue/work caps with focused overflow and shutdown tests. Current caps are cited directly from source.

Validation for this change is documentation/OpenSpec validation and link/manifest audit. Runtime acceptance belongs to the named F1–P14 changes. The architecture skill should remain unchanged because this guide is a target project map, not a newly verified current-code rule.
