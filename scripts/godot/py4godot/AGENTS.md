# Embedded Python Runtime Build

## Purpose and map

This directory owns the minimal ordered patches and reproducible native build
inputs for the Py4Godot presentation runtime. `build-inputs.env` pins the Darwin
derivative; `linux-build-inputs.env` retains the independently accepted Linux
pilot derivative and its separate patch series. `patches/` changes upstream
isolation, lifetime and Variant correctness only. `build-main.py` generates
required native sources/API headers and builds the owning Darwin main library.

## Ownership and lifecycle

The parent `build-python-runtime.sh` verifies upstream archives and patches,
stages the release wrappers, verifies native/payload digests and installs the
complete generated addon. The helper never installs. It requires exact staged
CPython, compiler/SDK and offline Cython inputs, uses at most four compile workers,
retains external build logs, and replaces its owned output only after linking.
Build commands disable bytecode writes so runtime artifact identity is independent
of whether the native cache was already populated. Runtime imports may create
bytecode caches; qualification records these while checking all static bytes.

The Darwin NIL repair is not qualified for the older Linux main library. A new
Linux derivative needs its own rebuilt main checksum and runtime matrix. Neither
derivative exposes gameplay APIs or expands presentation ownership.

## Validation

Run `python3 -m unittest discover -s scripts/godot/py4godot -p build_main_test.py`,
`bash -n scripts/godot/build-python-runtime.sh`, and the locked Python checks.
Artifact acceptance additionally requires verified materialization, actual native
Dictionary/Array scalar marshalling and the existing isolated runtime/export
matrix. A changed pinned derivative cannot reuse predecessor runtime acceptance.
