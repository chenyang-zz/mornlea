# Mornlea

<p align="center">
  <img src="https://img.shields.io/badge/Go-1.26-00ADD8" alt="Go 1.26">
  <img src="https://img.shields.io/badge/Rust-1.97.1-f74c00" alt="Rust 1.97.1">
  <img src="https://img.shields.io/badge/client-macOS-9cf" alt="macOS client">
  <a href="docs/notes/compatibility.md#current-version-matrix"><img src="https://img.shields.io/badge/versions-compatibility%20matrix-blue" alt="version matrix"></a>
  <img src="https://img.shields.io/badge/license-MIT-green" alt="MIT">
  <a href="https://github.com/chenyang-zz/mornlea/actions/workflows/ci.yml"><img src="https://github.com/chenyang-zz/mornlea/actions/workflows/ci.yml/badge.svg" alt="CI"></a>
</p>

English · [简体中文](README.zh.md)

Mornlea is an independent voxel survival game. It has its own client, authoritative server, world format, and binary protocol. It does not speak the Minecraft protocol, and it does not load Minecraft saves or Mojang assets.

Local play and LAN play use the same login and simulation path. The server decides world and player outcomes. The client keeps mirrors, prediction, and presentation.

Current contract versions (protocol, save schemas, engine and client ABI, benchmark scenario) are listed in one place: the [version matrix](docs/notes/compatibility.md#current-version-matrix).

## Features

**World.** Procedural terrain, oak trees and saplings, short grass, ores, flowing water, and snow cover. The server advances a fixed day length, seasons, temperature, and weather (clear, rain, thunder). Sky light and block light are derived on the client from the authoritative block mirror. Worlds, players, hostile mobs, and passive mobs persist.

**Survival.** Break and place blocks, tools with durability, personal 2×2 crafting and a 3×3 workbench, furnaces, and chests. Farm wheat, potatoes, and carrots. Hunger, food, oxygen, health, fall damage, and iron armor are server-authoritative. Combat includes swords, bows, and two hostile kinds (nightwalkers and bone throwers), plus passive cattle. Sneak with Shift, sprint by double-tapping `W`, sleep in beds, and carry water buckets. A new player starts with an empty inventory. World difficulty is `peaceful`, `normal`, or `hard`.

**Multiplayer.** Ordinary single-player starts an in-process server. `mornlea-server` is a headless TCP server for a trusted LAN: no authentication or encryption, at most 8 players, and no in-game server browser. Connect with `--connect`.

**Companions.** The Go server can run up to four named companions and their `go_to`, `follow`, `mine`, and `place` tasks. An optional Python service plans and speaks; it cannot write the world by itself.

**Client.** On macOS the default client is `mornlea`: a Rust wgpu renderer, an in-process WebView menu (main menu, settings, loading, pause), and a WebView HUD. `F5` cycles first person, third person behind, and third person in front. `apps/mornlea-godot` is an optional desktop pilot and is not what `make run` starts.

Some older boundary notes in [docs/notes/limitations.md](docs/notes/limitations.md) predate weather, armor, sneaking, bows, and cattle. When that note and the code disagree, the code and [openspec/specs](openspec/specs/) win.

## Screenshots

Headless visual baselines, 640×360. They are the tracked world scenes, not a live recording.

| Noon terrain | Oak grove |
| --- | --- |
| ![Noon terrain](testdata/visual-golden/world/terrain-noon.png) | ![Oak grove](testdata/visual-golden/world/oak-grove.png) |
| Rain | Snow cover |
| ![Rain at noon](testdata/visual-golden/world/rain-noon.png) | ![Snow cover](testdata/visual-golden/world/snow-cover.png) |

## Requirements

- macOS for the graphical client. The client entry point is built with Darwin constraints and is exercised mainly on Apple Silicon.
- Go 1.26.
- Rust 1.97.1 from rustup. The pin is `packages/engine/rust-toolchain.toml`.
- A C toolchain and CGO. On macOS that is Xcode Command Line Tools:

```bash
xcode-select --install
```

- Make.
- Python 3.12 and `uv` only if you enable the companion agent.
- Godot 4.7.2 only if you work on the optional pilot. See [apps/mornlea-godot/README.md](apps/mornlea-godot/README.md).

Linux can build the headless server with `make build-linux-server`. That target does not produce the graphical client.

## Quick start

```bash
git clone https://github.com/chenyang-zz/mornlea.git
cd mornlea
make run
```

The first launch generates terrain inside the view distance and is slower than later launches. The default save is `worlds/default`.

```bash
make run ARGS="--world worlds/demo"
```

`make run` builds the pinned Rust libraries, then starts the client. Do not mix a binary with a `mornlea_engine` library from another build.

## Usage

Local play opens a main menu, then an in-process world. Remote play skips that local world:

```bash
make rust
go run ./packages/server/cmd/mornlea-server --listen :25565 --world worlds/lan --seed 42 --max-players 8
go run ./packages/client/cmd/mornlea --connect 127.0.0.1:25565 --name PlayerA
```

`--seed` applies only when the world directory is created. `--max-players` accepts `1..8`. Do not expose this TCP port to the public internet. More server flags, including `--difficulty`, are in [docs/notes/lan-server.md](docs/notes/lan-server.md).

`make build` links `bin/mornlea` and `bin/mornlea-server` and copies `bin/libmornlea_engine.dylib`. It then copies attribution files from `packages/client/assets/packs/pixel_perfection`, which is no longer in the tree. That copy fails after the binaries are written. The embedded default textures are the Pastelcraft subset in `packages/client/assets/packs/pastelcraft`.

`make build-linux-server` writes the Linux amd64 headless server and an adjacent `bin/libmornlea_engine.so`. Ship those two files together.

### Controls

| Input | Action |
| --- | --- |
| `W` `A` `S` `D` | Move |
| Space | Jump; hold to swim up |
| Double-tap `W` | Sprint while moving forward |
| Left Shift | Sneak |
| Mouse | Look |
| Hold left button | Mine, melee, or draw a bow |
| Hold right button | Use: place, open, till, eat, doors, beds, workbench, equip armor |
| `1`–`9` | Hotbar slot |
| `E` | Inventory, or close the open container |
| `Q` | Drop one item from the selected hotbar slot |
| Enter | Chat, including `@name command` for a companion |
| `F5` | Cycle the camera |
| Esc | Close the open panel, or pause |
| `F3` | Debug panel, only with `--dev` |

Inventory clicks can move a whole stack, split a stack, or quick-move it. The server applies the move. Numbers, recipes, and companion commands are in [docs/notes/gameplay.md](docs/notes/gameplay.md).

## Project structure

The repository root is not a Go module. `go.work` lists six modules. Go import paths still use the prefix `github.com/channing771/mornlea/packages/<unit>` even though the GitHub repository is `chenyang-zz/mornlea`.

```text
packages/
  contracts/   JSON contracts shared by Go and Python
  shared/      domain types, physics, network, world, engine ABI bridge
  server/      authoritative simulation, fluids, storage, mornlea-server
  client/      mirrors, prediction, CPU-side render prep, mornlea
  tools/       perfcheck, agent board, and other dev tools
  audit/       architecture gates
  engine/      Rust workspace: mornlea_engine, mornlea_client, and migration crates
  agent/       optional Python companion agent
apps/
  mornlea-godot/   optional Godot desktop pilot
```

Today, Go owns the live server, protocol session used by the default client, and storage. `mornlea_engine` is the numerical kernel (mesh, light, collision, raycast, physics, worldgen, fluids). `mornlea_client` owns the Darwin window, input, WebView shell, and GPU rendering. Go does not call WebGPU. The Rust domain, protocol, and storage crates, and the Godot app, are migration work. The intended end state is described in [docs/architecture-target.md](docs/architecture-target.md); the running system is described in [docs/architecture.md](docs/architecture.md).

## Documentation

| Topic | Document |
| --- | --- |
| Gameplay | [docs/notes/gameplay.md](docs/notes/gameplay.md) |
| Configuration | [docs/notes/configuration.md](docs/notes/configuration.md) |
| LAN server | [docs/notes/lan-server.md](docs/notes/lan-server.md) |
| Texture packs | [docs/texture-packs.md](docs/texture-packs.md) |
| Limits | [docs/notes/limitations.md](docs/notes/limitations.md) |
| Saves and protocol upgrades | [docs/notes/compatibility.md](docs/notes/compatibility.md) |
| Visual baselines | [docs/notes/visual-verification.md](docs/notes/visual-verification.md) |
| Current architecture | [docs/architecture.md](docs/architecture.md) |
| Target architecture | [docs/architecture-target.md](docs/architecture-target.md) |
| What has shipped | [docs/notes/progress.md](docs/notes/progress.md) |
| Doc index | [docs/README.md](docs/README.md) |

Several player notes are written in Chinese.

## Common commands

| Command | What it does |
| --- | --- |
| `make help` | List Makefile targets |
| `make run` | Build Rust libraries and start the macOS client |
| `make build` | Link both binaries and copy the engine dylib; the trailing notice copy still fails |
| `make build-linux-server` | Linux amd64 headless server plus `libmornlea_engine.so` |
| `make test` | Go tests for all six modules |
| `make test-race` | The same tests with the race detector |
| `make dev-check` | Short Go checks plus Rust fmt, clippy, and tests |
| `make rust` | Build the pinned Rust cdylibs |
| `make rust-check` | Rust fmt, clippy, and workspace tests |
| `make visual-check` | Compare headless frames with the visual baselines |
| `make companion-agent-check` | Python format, lint, types, and unit tests |
| `make companion-agent-integration` | Go/Python process contract, no external network |

## Contributing

Read [AGENTS.md](AGENTS.md) before changing code. Small fixes can go straight to a pull request. Protocol, save, performance-contract, and cross-package work goes through OpenSpec first: [docs/openspec.md](docs/openspec.md) and [docs/development-process.md](docs/development-process.md).

Do not add Mojang textures or other unauthorized art.

## License

The project is [MIT](LICENSE).

The embedded default block textures are a renamed subset of [Pastelcraft](https://modrinth.com/resourcepack/pastelcraft) by XradicalD (Square Dreams), also MIT. Attribution is in [packages/client/assets/packs/pastelcraft/ATTRIBUTION.md](packages/client/assets/packs/pastelcraft/ATTRIBUTION.md). Unmapped layers fall back to procedural textures.
