---
doc_id: godot-client-project
language: en
counterpart: README.zh.md
revision: 2026-09-30.1
---

# Mornlea Godot Client

This directory is the stable Godot project root for the desktop client pilot and any later approved full migration. Open this directory itself in Godot Project Manager; do not open the repository root. The project remains here through later migration stages so scene UIDs, `res://` paths, tooling, and release identities do not need a second relocation.

## Open the project

The project requires Godot 4.7.2 Standard. Verify the pinned official artifacts without downloading them:

```bash
scripts/godot/fetch.sh --verify-only
```

Open and import the project without a foreground window:

```bash
# Linux x86_64
scripts/godot/godot.sh --headless --path apps/mornlea-godot --editor --quit-after 120

# macOS
scripts/godot/godot.sh --headless --path apps/mornlea-godot --editor --quit
```

Godot 4.7.2's Linux EditorHelp documentation callback can outlive immediate shutdown after a cold import. The bounded 120-frame import allows that callback to settle before the editor exits.

You may set `MORNLEA_GODOT_BIN` to an absolute path for an exact Godot 4.7.2 executable. The wrapper also accepts the official `/Applications/Godot.app` installation when its version matches the project pin.

The permanent main scene is pure GDScript and built-in Godot nodes. Python and native artifacts are optional while opening the editor. Bootstrap reports missing Python extension, embedded interpreter, standard-library, and project-bridge files; incompatible Godot or extension descriptors; the detected desktop target; and the exact preparation command. This diagnostic path neither imports Python features nor connects to a server.

Prepare the current macOS Apple Silicon runtime and bridge with:

```bash
scripts/godot/build-python-runtime.sh --verify --offline
scripts/godot/build-extension.sh --target aarch64-apple-darwin --profile debug --verify
```

Exercise both clean-state diagnostic paths without modifying the project checkout:

```bash
scripts/godot/openable-smoke.sh --without-native
scripts/godot/openable-smoke.sh --without-python
```

## Linux x86_64 distribution

Linux builds use the same production feature catalog as the macOS project. The qualified target is `x86_64-unknown-linux-gnu`; the Godot export preset is `Mornlea Linux x86_64`. This packages the existing desktop pilot features and does not establish full game feature parity.

Prepare Go 1.26, Rust 1.97.1, the pinned GNU C++ toolchain from `scripts/godot/py4godot/linux-build-inputs.env`, `patchelf`, `binutils`, `ripgrep`, `unzip`, and `shasum`. Then prepare the host-native Rust libraries, official Godot editor/templates, and embedded Python runtime, and export and verify the production main scene headlessly:

```bash
make rust
scripts/godot/fetch.sh --target linux-x86_64
scripts/godot/build-python-runtime.sh --verify --target x86_64-unknown-linux-gnu
make godot-build
make godot-export-linux
```

The default output is the ignored `build/godot-linux/` directory. Distribute that entire directory: `mornlea.x86_64`, `mornlea.pck`, the colocated Rust/Go shared libraries, and `addons/py4godot/cpython-3.14.4-linux64/` are one unit. CPython loads its standard library from this filesystem tree, so copying only the executable and PCK is insufficient. Production Python scripts also remain at their mapped filesystem paths because Py4Godot executes their classes through its file loader. The current ELF artifacts require glibc 2.34 or newer and a system `libstdc++` providing `GLIBCXX_3.4.32` (GCC 13.2 or newer). Launch the exported executable directly; no system Python, `PYTHONPATH`, `LD_LIBRARY_PATH`, runtime installer, or network download is needed.

The Python loader is compiled with the compiler identified in `scripts/godot/py4godot/linux-build-inputs.env`; its loader, patch series, source archive, and complete runtime checksums are independently pinned. A different compiler or altered runtime is rejected before export. The editor and template archives are SHA-256 checked against `scripts/godot/version.env`; the Linux editor digest was obtained after checking the official release SHA-512 manifest.

`make godot-build` builds debug and release native artifacts from the verified external cache offline. `make godot-export-linux` requires those prepared artifacts, validates source/resource closure and generated assets, restores the clean embedded runtime from its verified offline cache, verifies its checksums, then activates and closes the production catalog from the release bundle. Verification requires Bootstrap readiness, the six active features (session, actors, desktop input, player view, UI, and world), the two explicitly disabled audio/lifecycle skeletons, and clean Python teardown. Run export while no other Godot process uses the project runtime, since it rematerializes the ignored Python add-on. All automated checks run headlessly. To export already prepared artifacts into a different absolute directory:

```bash
scripts/godot/export-linux.sh --verify --output /absolute/path/mornlea-linux
```

The exporter stages and verifies the replacement before publishing it. It rejects nonempty output directories without its ownership marker and keeps a previously owned bundle usable if staging or validation fails. `distribution.env` records tool/runtime pins and source revision, including whether local changes were present.

## Asset synchronization

`packages/client/assets` remains authoritative for the registered atlas, and `packages/client/render/assets` remains authoritative for the registered Noto Sans CJK font. Materialize their Godot-local derivatives with:

```bash
scripts/godot/sync-assets.sh
scripts/godot/sync-assets.sh --check
```

The generated directory contains the layer-major, mip-major RGBA8 atlas, the registered font and its OFL/provenance files, retained material license records, and a deterministic manifest. The manifest records the source Git trees, every input checksum, one aggregate input checksum, atlas layout, and every output checksum. Do not edit or add files under `assets/generated/`; the checker rejects missing, changed, symbolic-link, and hand-authored files.

## Python development checks

Production Python dependencies are empty. The embedded CPython runtime contains only the qualified Py4Godot unit and does not install development tools. Ruff and mypy are development-only dependencies resolved exactly by `uv.lock`; `uv` may populate the local ignored `.venv/` from that lock, but it does not modify or install into the embedded runtime. Local Py4Godot stubs under `typing/` provide the checked interface without importing generated add-on code.

Run formatting, lint, strict typing, boundary mutation tests, and source-policy checks with:

```bash
scripts/godot/python-check.sh --locked
```

The boundary check rejects companion Agent imports, direct native ABI or dynamic-library access, Python-side networking, runtime installers, process execution, unrestricted dynamic imports, and non-English source comments.

## Python feature host contract

After Bootstrap completes the dependency handoff, `app/host/app_root.py` and `app/host/feature_host.py` own catalog planning and feature lifecycle. `config/feature_catalog.tres` is the explicit allowlist; it names coarse `feature.tres` manifests instead of scanning directories. Manifests declare host protocol `1.0`, one project-local entry scene, stable dependencies, versioned bridge-family requirements, required or optional failure semantics, a budget class, and a reset policy.

The lifecycle is deterministic: validate the catalog, instantiate in sorted dependency order, validate the Python feature, inject one typed Godot bridge service, activate for an epoch, reset, and deactivate in reverse order. An incompatible or failed required feature stops assembly and releases already active features. An optional feature is disabled with an observable result, and dependents cannot silently activate through it. Feature scripts do not import sibling project modules or discover implementations dynamically; Godot resource paths provide bounded composition while the isolated interpreter keeps project directories out of `sys.path`.

Run the embedded-Python contract, additive-extension, and native-bridge integration checks with:

```bash
scripts/godot/feature-contract-check.sh
scripts/godot/feature-contract-check.sh --extensibility-probe
scripts/godot/feature-contract-check.sh --bridge-integration
```

## Transcript terrain check

The world feature's native terrain pipeline (packed-quad decode, mesh-prepare worker, RenderingServer RID table, and per-frame budgets) is proven end to end by a headless check that replays a deterministic protocol v44 scenario into a real pilot session. A transcript helper (`packages/client/cmd/mornlea-godot-transcripts`) serves the scenario under `testdata/godot-pilot/transcripts/terrain/` on loopback: round one publishes an initial chunk snapshot, a block delta, a chunk forget, and a disconnect; round two simulates world re-entry. The check scene activates the production catalog, connects through the session feature, and asserts the bridge's structural terrain summary after every stage, including that no stale section survives the forget and the disconnect/reset. Run it with:

```bash
make godot-terrain-check
```

## Dedicated-server terrain smoke

The same terrain chain is also proven against the real authoritative server, not a transcript. The smoke gate (`scripts/godot/terrain-smoke.sh`) builds `mornlea-server` into a per-run temporary directory, starts it on a deterministic loopback port with its world, config, and logs rooted in temporary paths so nothing touches repository saves, waits for its startup line, then runs the headless smoke scene. The scene activates the production catalog, logs the pilot into the real server, waits for the same loaded criterion as the transcript check's initial snapshot (confirmed session phase Play plus at least one live terrain section in the structural summary), runs a fixed 300-frame budget while failing loudly on any terrain error word after the loaded criterion, and proves the clean close leaves no stale section. The gate asserts the success marker, requires a clean SIGTERM shutdown of the server, and reaps every child through an exit trap with a survivors check. Run it with:

```bash
scripts/godot/terrain-smoke.sh
```

## Architecture boundary

Godot owns desktop windowing, keyboard/mouse collection, presentation, pilot UI, and Godot resource lifecycles. The Go client runtime will continue to own protocol v44, mirrors, prediction, semantic frame state, and bounded network processing. Numerical mesh, lighting, collision, raycast, and physics remain in engine ABI v11. The authoritative Go server remains the only owner of world and player truth.

Future functionality extends the same root through coarse `features/`, `platform/desktop/`, `config/`, and the single `addons/mornlea_bridge/` boundary. Bootstrap must remain feature-agnostic. Mobile, Web, console, touch, sensors, and mobile lifecycle support are outside this project.

Generated editor cache and native bridge binaries are ignored. Script and shader `.uid` sidecars are intentionally tracked once Godot generates them.
