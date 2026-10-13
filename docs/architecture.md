---
doc_id: architecture
doc_revision: 2026-09-16.1
language: en
counterpart: architecture.zh.md
---
# Mornlea current architecture

> This document records the current implementation. For all new architecture decisions, read [`architecture-target.md`](architecture-target.md) first. The Godot/Python pilot described below is a migration-era implementation and is not the final language or runtime topology.

## Migration-era Godot pilot (not the target architecture)

The migration project root is `apps/mornlea-godot/`, organized around coarse features and a `platform/desktop` target for macOS, Windows, and Linux only. The current pilot uses an embedded Python host, a Go client-core, and a Rust Godot adapter. Those are transition seams: Python remains the intended final Godot feature language, while the Go client-core and its c-shared boundary are replaced by Rust client-core. Android, iOS, Web, and consoles are out of scope.

## 1. System overview

Mornlea consists of Go applications, an independent Python companion Agent service, and two Rust `cdylib` libraries. Go owns application assembly, the world and authoritative simulation, protocol and transport, storage, client mirrors, and CPU-side render layout and upload orchestration. Python runs only Planner, Dialogue, and compact memory. Rust `mornlea_engine` provides the windowless numerical kernel, while Rust `mornlea_client` provides Darwin windows, events, and the GPU backend. Go and Rust cooperate only through established C ABI bridges; Go and Python cooperate only through loopback HTTP/MCP contracts. There is no interchangeable second authoritative implementation.

## 2. Server authority and client mirrors

The server is the sole authority for world, player, companion, inventory, and gameplay state. Clients submit input intent, keep validated authoritative mirrors, and may make reversible predictions for local responsiveness; a prediction can never confirm a server outcome.

Authoritative state changes are settled serially through fixed phases of the server tick and then published through the protocol. Client rendering and UI consume only mirrors and local presentation state; they neither read nor modify authoritative simulation directly.

## 3. Shared Memory/TCP path

Ordinary local play uses the Memory transport and remote play uses TCP. Both reuse the same packet/codec contract, login state machine, session assembly, and authoritative simulation. Memory changes only the delivery medium; it does not provide a same-process privileged path around login, input validation, or server adjudication.

`packages/shared/network` owns session and transport orchestration: shared stream interfaces, the Play endpoint façade, the login state machine, and the Memory transport. It re-exports aliases for the protocol-message subpackage `packages/shared/network/protocol` (the packet/message/registry/snapshot protocol layer) and codec subpackage `packages/shared/network/codec` (packet-to-wire codecs and frame encapsulation), preserving the existing `network.X` surface. `packages/shared/network/tcp` owns TCP listeners, dialing, and stream implementations, and has transport-only responsibility. The Host and simulation layers consume the same validated commands, so local and remote modes share behavioral semantics.

## 4. Independent companion Agent service

The companion path has the following fixed dependency and write-authority boundary:

```text
Go Host / Agent HTTP client ──loopback HTTP v1──> Python FastAPI / LangGraph
Python MCP SDK             ──loopback MCP v1───> Go frozen SnapshotRegistry
Go tick ──strict decode + current-world revalidation──> Task Runner ──> world writes
Python SQLite <──compact MemoryState CAS──> Go companions.ai v5 recovery mirror
```

The Go server alone owns companion bodies, inventories, tasks/FIFO, generation, snapshots, final plan validation, and every world write. Python returns candidate Plans, Dialogue proposals, and memory results. The six MCP tools only read the 33×17×33 frozen terrain projection or perform pure candidate validation; they cannot submit `CompanionAction`. Go does not shell out to, use FFI with, or embed Python, and it does not start or supervise the Agent process.

When Agent, MCP, or provider access is unavailable, the authoritative world, existing Running tasks, and FIFO continue. New planning fails with stable `PlannerUnavailable`, and Dialogue is skipped. No failure introduces a direct-model fallback or makes a recovery mirror a normal Dialogue prompt source.

The Python service uses Python 3.12, FastAPI, LangChain/LangGraph, one Uvicorn worker, and one SQLite writer. Planner and Dialogue create a transient graph for each request and retain no checkpoint. SQLite stores only namespace lease/fencing, per-companion epochs, summary/revision/operation, and tombstone/CAS metadata; it does not store plans, tasks, FIFO, snapshots, prompts, messages, personas, lines, or proposals. Python is the runtime summary authority; the Go `companions.ai` v5 mirror is used only to reconcile after database loss or process restart.

Go and Python each cap global Planner/Dialogue runs at four, permit one in-flight request per companion with no waiting queue, and use default/hard limits of 3/5 model calls, 4/8 autonomous tools, and 30/60 seconds total duration. The snapshot registry retains at most four immutable snapshots for the caller deadline plus a five-second TTL. Cancellation immediately rejects new lookups; handlers that already acquired a view observe cancellation at bounded-loop, encoding, and response-commit checkpoints, then discard the complete result. The authoritative tick only publishes or receives bounded immutable values and performs no Agent HTTP, MCP, model, SQLite, or JSON-encoding I/O.

Terminal Dialogue first returns an uncommitted proposal. After revalidation at the tick boundary, Go creates an accepted reservation; a subsequent generation change does not revoke it. Go updates the v5 mirror and broadcasts one line only after the matching operation/epoch CAS commit succeeds. Every active-to-inactive or inactive-to-active transition advances the memory epoch; active canonical-zero, active nonzero mirror, and inactive tombstone each have strict replay identities, while a higher epoch fences every older result. Safe shutdown stops new chat, cancels and waits for workers, freezes queues/actors, saves final v5 state, flushes the world, releases the namespace, and then closes MCP and world storage. A save or flush failure leaves the component retryable.

## 5. Go package responsibilities and archcheck dependency boundaries

- `packages/client/cmd/mornlea` and `packages/server/cmd/mornlea-server` own application entry points and resource-lifecycle assembly.
- `packages/shared` is a shared-domain Go module (a separate `go.mod` and a `go.work` member). It owns domain packages used on both server and client sides: `core` (public domain types and native raycasts), `world` (chunk and world data models), `physics` (player movement and collision), `pathfind` (bounded pathfinding over immutable snapshots), `companion` (companion identity and domain types), `network` (plus protocol/codec/tcp, login state machine and transports), `worldgen` (seed setup and chunk write-back), `tuning` (Tunables snapshots), `profile`, `config`, `logging`, and `nativeabi` (the sole Go bridge to the engine ABI).
- `packages/server/sim` contains guidance only. Four subpackages carry authoritative simulation: `contract` (cross-boundary DTOs), `realm` (world dimensions and single-tick transactions), `entity` (players, companions, nightwalkers, and gameplay settlement), and `runtime` (Engine and Step orchestration). Tunables snapshots live in `packages/shared/tuning`. See `packages/server/sim/AGENTS.md` and `packages/audit` for dependency direction and single-commit discipline.
- `packages/server/storage` owns encoding, migration, recovery, and disk lifecycle for world, player, companion, and nightwalker data. Subpackages including `packages/server/storage/chunk`, `packages/server/storage/player`, `packages/server/storage/companion`, `packages/server/storage/hostile`, and `packages/server/storage/region` refine the implementation while the top level retains the public consumption surface.
- `packages/server/server` assembles Host, Server, login, sessions, the authoritative tick, publication, and shutdown. It delegates persistence lifecycle to `packages/server/server/persistence`; it does not own save queues, retry state, or workers, and retains only compatibility re-exports for `PersistenceStatus` and `ErrPlayerPersistenceBackpressure`.
- `packages/server/server/persistence` independently owns load, observation, asynchronous save, retry, flush/close, and worker lifecycle for world chunks and metadata, players, companions, and hostiles. Production code depends only on `packages/shared/companion`, `packages/shared/core`, `packages/shared/physics`, `packages/server/sim/runtime`, and `packages/server/storage`; it must not import `packages/server/server` back or access private Host/Server state. `packages/audit` is authoritative for dependency direction.
- `packages/shared/pathfind` owns bounded pathfinding over immutable snapshots and depends only on `packages/shared/core`. `packages/shared/companion` and `packages/server/server` consume it, but pathfinding owns neither gameplay nor world access.
- `packages/client` is the client-domain Go module (a separate `go.mod` and a `go.work` member). `client` owns client mirrors, input prediction, message receipt, the client ABI bridge, and CPU-side render orchestration. `render` (plus `hud`), `mesh`, `assets`, `lod`, `audio`, `packages/shared/worldgen`, and `packages/server/fluid` provide domain-data descriptions, CPU encoding, and Rust-call orchestration; they own neither a GPU backend nor a second production numerical implementation. `packages/client/cmd/mornlea` is the graphical client entry point; `app` and `benchmark` assemble a local authoritative Host in process and are the only locations in this module permitted to import `packages/server`.
- `packages/audit` source guards enforce two exempt edges between server and client modules: server production files cannot import client (the client mirror for Memory/TCP integration occurs only in `_test.go`), and client-domain library files cannot import server (server assembly belongs only to `packages/client/cmd/mornlea`).

The `allowed` table in `packages/audit/audit/dependencies.py` is authoritative for permitted direct internal dependencies. `packages/audit` also uses `TestSimAuthorityStateOwnershipStaysExplicit` to scan runtime package variables and holders, binding the sole mutation/commit to the actual `StepWithTunables` call path, and `TestAuthorityTickTunablesStayExplicit` to keep authoritative-tick parameter capture and forwarding explicit. Other gates protect the absence of Go WebGPU dependencies, the headless-server closure, companion Agent-service publication boundaries, Make/CI gates, and long-lived version baselines. This document deliberately does not copy a dependency allowlist that evolves with packages.

## 6. `mornlea_engine` / engine ABI v11

`mornlea_engine` is the sole production implementation for mesh/light, collision, raycasts, physics-tick integration, world generation, LOD shells, and fluid-rule evaluation/rescan traversal. Go still owns orchestration of fluid queues, budgets, cursors, and washout settlement: `packages/server/fluid` and `packages/server/sim/realm` invoke fluid kernels through `packages/shared/nativeabi`. The crate remains windowless, owns no authoritative world state, performs no file or network I/O, and carries no business rules such as damage, inventory, permissions, or tick orchestration.

The current engine C ABI is v11. Only `packages/shared/nativeabi` may touch it from Go; domain packages construct semantic inputs and decode results. Header, Rust FFI, Go bridge, ABI version, and cross-language consistency checks evolve as one unit. Neither side may retain the other side's pointers after a call returns.

## 7. `mornlea_client` / client ABI v19

`mornlea_client` owns Darwin windows and event collection, the in-process WKWebView menu layer, GPU resources, shaders, render passes, window surfaces, and off-screen rendering. The embedded Vite + TypeScript + React frontend presents window-oriented UI (main menu, settings, pause, and F3) through WKWebView; Rust serves assets from embedded bytes through the `mornlea://` scheme handler. It also presents the persistent HUD, inventory/crafting, character page, workbench, chest, furnace, and tooltips, so production frames no longer prepare or submit panel GPU instances. `-connect` is the interactive online entry point and mounts the same frontend on its first downstream state; only capture and benchmark remain windowless and without WebView. Go imports no WebGPU binding and uses window and renderer domain interfaces only through the client ABI bridge in `packages/client/client`.

The current client C ABI is v19 and evolves independently from the engine ABI. Header, Rust FFI, the `packages/client/client` bridge, version checks, and cross-language checks must move together. A failure or insufficient capacity must not publish partial output.

The renderer has a compact derived `RenderWorld` cache updated atomically by MRW1; the Go Mirror remains the source of truth for client logic. This cache entry point is currently driven only by Rust/Go tests and is not yet connected to `packages/client/cmd/mornlea/app`. Go still owns production mesh scheduling, connectivity/visibility, per-section upload, and draw inputs; moving those duties belongs to a future change.

## 8. Graphical-client and headless-dedicated-server release units

The Darwin graphical client `mornlea` depends on `mornlea_engine` and `mornlea_client` from the same build and must not mix ABI components across builds. The headless dedicated server `mornlea-server` does not depend on `mornlea_client`, windows, or the GPU stack, but its authoritative physics and spatial queries still depend on `mornlea_engine` from the same build.

The Linux dedicated-server release unit consists of `mornlea-server` and adjacent `libmornlea_engine.so`, loaded through `$ORIGIN`. They must be distributed as a single release unit that cannot be mixed across versions.

## 9. Concurrency, data, and hot-path constraints

- A message and its slices become immutable after a successful cross-goroutine send; later mutation requires a copy.
- Authoritative tick, rendering, and network hot paths do bounded work only. They do not block on disk, network, model calls, or other heavy CPU work.
- Heavy work leaves the hot path through bounded queues, immutable snapshots, or workers, and its results rejoin at boundaries with explicit ownership.
- Persistence concurrency boundary: `packages/server/server/persistence` isolates disk I/O for four owners with bounded channels and fixed workers. `World` uses `Options.SaveWorkers` workers (`saveJobs`/`saveCompletions` capacity is `SaveWorkers*2`); `Players` has two workers (`playerSaveJobCapacity=16` and `playerSaveDoneCapacity=2`); `Companions` and `Hostiles` have one worker each (capacity one each). The authoritative tick performs only bounded, non-blocking `World.Observe`/`Drain`, `Players.Observe`/`Poll`, `Companions.Observe`/`Poll`, and `Hostiles.Observe`/`Poll` scheduling; it never waits for persistence. `SaveObserver` is called only on the `World` worker's `SaveBatch` timing path, not the tick path. `World.Flush` and `World.ShutdownContextError` use `Options.EngineLocker` (root `Server.stepMu`, or a private `sync.Mutex` fallback when the subpackage is constructed independently) for a short engine/state transition before `World.mu`; they release both immediately before waiting on channel/context. `Drain`/`Status` retain the established contract that the caller holds the tick lock.
- Protocol, save, and FFI entry points validate type, length, count, capacity, and version before allocating, traversing, or writing output.
- Overflow, data loss, incomplete report identity, and I/O errors fail explicitly; they are never silently truncated or swallowed.

## 10. Behavior specifications, implementation progress, and historical design entry points

Code, tests, and [`openspec/specs/`](../openspec/specs/) define current observable behavior. Changes being implemented are in [`openspec/changes/`](../openspec/changes/); see [`docs/openspec.md`](openspec.md) for the workflow.

[`docs/notes/progress.md`](notes/progress.md) records implementation chronology. `docs/superpowers/` and [`openspec/changes/archive/`](../openspec/changes/archive/) retain historical designs and change evidence; they do not override current architecture or the canonical behavior specifications. [`docs/README.md`](README.md) is the complete documentation entry point.

## 11. Repository map

```text
.
├── packages/
│   ├── agent/
│   │   └── companion/      Independent Python 3.12 FastAPI/LangGraph/SQLite service
│   ├── contracts/          Independent minimal Go module (a go.work workspace member)
│   │   └── companion-agent/ Shared HTTP v1 and MCP v1 manifest/schema/goldens
│   ├── shared/             Shared-domain Go module (a go.work workspace member): domain packages used by server and client
│   │   ├── core/           Public domain types and native raycast batch driver
│   │   ├── nativeabi/      Sole Go bridge to the engine C ABI
│   │   ├── logging/        Modular logging
│   │   ├── physics/        Player movement and collision
│   │   ├── pathfind/       Bounded pathfinding over immutable snapshots
│   │   ├── world/          Chunk and world data models
│   │   ├── worldgen/       Worldgen seed-to-permutation setup, Rust calls, and chunk write-back
│   │   ├── companion/      Independent companion identities, static definitions, and body types
│   │   ├── network/        Binary protocol, login state machine, and Memory/TCP transport (protocol/codec/tcp subpackages)
│   │   ├── tuning/         Tunables snapshots and validation (lifted from sim/tuning)
│   │   ├── profile/        Stable local player identity and profile
│   │   └── config/         Shared JSON configuration loading and validation
│   ├── client/             Client-domain Go module (a go.work workspace member)
│   │   ├── client/         Input, camera, prediction, window/client ABI, and client mirror
│   │   ├── render/         CPU half of rendering: layout, encoding, and upload scheduling (hud subpackage)
│   │   ├── mesh/           Chunk-mesh production API (implemented in the Rust cdylib)
│   │   ├── lod/            Far-ring tile scheduling and CPU encoding
│   │   ├── audio/          Darwin-local procedural cues
│   │   ├── assets/         Block definitions and procedural textures (packs embeds the default texture pack)
│   │   └── cmd/
│   │       └── mornlea/    Game client and embedded-server assembly (app/capture/benchmark/devcapture subpackages)
│   ├── server/             Server-domain Go module (a go.work workspace member)
│   │   ├── cmd/
│   │   │   └── mornlea-server/ Headless TCP dedicated server
│   │   ├── sim/            Authoritative-simulation guidance directory (production lives in contract/realm/entity/runtime)
│   │   │   ├── contract/   Cross-boundary DTOs
│   │   │   ├── realm/      World dimensions, persistence, and environment transactions
│   │   │   ├── entity/     Players, companions, nightwalkers, and gameplay settlement
│   │   │   └── runtime/    Engine, subscriptions, and Step orchestration
│   │   ├── fluid/          Bounded authoritative fluid-update queue and native wrapper for fluid kernels
│   │   ├── storage/        World, region-file, and player-state persistence
│   │   └── server/         Server Host, Server, login, sessions, authoritative tick, publication, and shutdown orchestration
│   │       └── persistence/ Four persistence owners: loading, observation, asynchronous saving, retry, flush, and workers
│   ├── tools/              Development-tools module (a go.work workspace member)
│   │   ├── perfcheck/      Performance-report comparison tool
│   │   ├── agent-board/    AI worker execution-status board (web is a React frontend)
│   │   ├── gfxspike/       Rust-renderer terrain rendering verification program
│   │   └── composite_grass_side/ Grass-block side texture compositor
│   ├── audit/              Standalone Python audit package: architecture and governance gate tests (dependency direction, unit boundaries, identity, and baseline versions)
│   └── engine/
│       └── crates/
│           ├── mornlea_engine/ Pinned Rust 1.97.1 cdylib: mesh/light/collision/raycast/physics/worldgen/lod/fluid
│           └── mornlea_client/ Darwin windows, event loop, WebView menu layer (frontend React app), and all GPU rendering
├── scripts/agent-hooks/     Retired Hook policy implementation and CI tests
└── docs/                    Design, implementation plans, performance records, and implementation progress
```
