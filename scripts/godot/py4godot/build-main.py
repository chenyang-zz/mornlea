#!/usr/bin/env python3
"""Build the native Py4Godot dependency without replacing release Python wrappers."""

import argparse
import concurrent.futures
import hashlib
import json
import os
import platform
import re
import subprocess
from pathlib import Path

GENERATORS = (
    "generate_class_ids.py",
    "generate_method_ids.py",
    "generate_type_checking.py",
    "generate_call_static_methods.py",
    "generate_wrappers.py",
    "generate_constants.py",
    "generate_constant_core_classes.py",
    "generate_classes_cpp.py",
    "generate_classes_hpp.py",
    "generate_common_functions.py",
    "generate_common_functions_pyi.py",
    "generate_enums.py",
    "generate_enums_cpp.py",
    "generate_pxd_bridge.py",
    "generate_pxd_utility.py",
    "generate_utility_hpp.py",
    "generate_utils_cpp.py",
    "generate_native_structs.py",
    "generate_cast_helpers.py",
)
CYTHON_INPUTS = (
    "core/variant4/type_helpers.pyx",
    "core/variant4/cast_helpers.pyx",
    "wrappers/wrappers.pyx",
    "wrappers/type_checking.pyx",
    "pluginscript_api/utils/annotations.pyx",
    "pluginscript_api/utils/forward_print.pyx",
    "script_extension/script_extension_helpers.pyx",
    "utils/instance_utils.pyx",
    "script_instance/PyScriptInstance.pyx",
)


def check_host():
    if (platform.system(), platform.machine()) != ("Darwin", "arm64"):
        raise ValueError("native main build requires Darwin-arm64")
    compiler = subprocess.check_output(["clang++", "--version"], text=True)
    sdk = subprocess.check_output(
        ["xcrun", "--sdk", "macosx", "--show-sdk-version"], text=True
    ).strip()
    if (
        not re.search(r"^Apple clang version 21\.0\.0(?:\s|$)", compiler)
        or sdk != "26.5"
    ):
        raise ValueError(
            "native main build requires Apple clang 21.0.0 and macOS SDK 26.5"
        )


def run(command, cwd, log):
    environment = dict(os.environ, PYTHONHASHSEED="0")
    environment.pop("PYTHONPATH", None)
    environment.pop("PYTHONHOME", None)
    with log.open("w") as stream:
        stream.write(json.dumps([str(value) for value in command]) + "\n")
        stream.flush()
        subprocess.run(
            command,
            cwd=cwd,
            env=environment,
            stdout=stream,
            stderr=subprocess.STDOUT,
            check=True,
        )


def main_sources(source_root, python):
    output = subprocess.check_output(
        [str(python), str(source_root / "meson_scripts/glob_tools_cpp.py")],
        cwd=source_root,
        text=True,
    )
    sources = []
    for name in output.splitlines():
        relative = Path(name)
        if (
            relative.is_absolute()
            or ".." in relative.parts
            or relative.suffix != ".cpp"
        ):
            raise ValueError("native source list escapes its verified source root")
        source = source_root / relative
        if not source.is_file():
            raise ValueError("native source list names a missing source")
        sources.append(source)
    if not sources or len(sources) != len(set(sources)):
        raise ValueError("native source list is empty or duplicated")
    return sorted(sources)


def generate(source_root, python_root, build_root):
    python = python_root / "bin/python3.14"
    for name in ("classes", "enums", "cppclasses"):
        (source_root / "py4godot" / name).mkdir(exist_ok=True)
    # Source generation skips installed packages; its upstream interactive exit calls retain their status.
    for index, name in enumerate(GENERATORS):
        run(
            [
                str(python),
                "-S",
                "-c",
                "import builtins, runpy, sys; builtins.exit=sys.exit; runpy.run_path(sys.argv[1], run_name='__main__')",
                name,
            ],
            source_root / "generation_files",
            build_root / f"generate-{index:02d}.log",
        )
    template = source_root / "py4godot/godot_bindings/pythonpaths_template.h"
    template.with_name("pythonpaths.h").write_text(
        template.read_text()
        .replace("{python_ver}", "cpython-3.14.4")
        .replace("{python_ver_short}", "python3.14")
    )
    identity = build_root / "cython-identity.json"
    driver = (
        "import hashlib, json, sys; from importlib.metadata import distribution; "
        "from pathlib import Path; d=distribution('Cython'); "
        "assert d.version == '3.3.0', d.version; "
        "Path(sys.argv[1]).write_text(json.dumps({'version': d.version, "
        "'metadata_sha256': hashlib.sha256(d.read_text('METADATA').encode()).hexdigest(), "
        "'record_sha256': hashlib.sha256(d.read_text('RECORD').encode()).hexdigest()}, sort_keys=True)+'\\n'); "
        "sys.argv=['cython']+sys.argv[2:]; "
        "from Cython.Compiler.Main import main; main(command_line=True)"
    )
    command = [
        "uv",
        "run",
        "--offline",
        "--no-project",
        "--no-config",
        "--isolated",
        "--no-python-downloads",
        "--with",
        "Cython==3.3.0",
        "--python",
        str(python),
        "python",
        "-I",
        "-c",
        driver,
        str(identity),
        "--cplus",
        "-3",
        "-I",
        str(source_root),
    ]
    command.extend(str(source_root / "py4godot" / name) for name in CYTHON_INPUTS)
    run(command, source_root, build_root / "cython.log")
    for name in CYTHON_INPUTS:
        header = (source_root / "py4godot" / name).with_name(Path(name).stem + "_api.h")
        if not header.is_file():
            raise ValueError(f"Cython omitted required API header: {name}")


def compile_main(source_root, python_root, output, build_root, sources, jobs):
    includes = [
        source_root,
        source_root / "py4godot/godot_bindings",
        source_root / "py4godot/gdextension-api",
        source_root / "py4godot/core/variant4",
        source_root / "py4godot/script_instance",
        python_root / "include/python3.14",
    ]
    # These are the upstream native release flags; the loader has a separate warning policy.
    flags = ["-std=c++17", "-O3", "-DNDEBUG", "-w", "-Wno-address-of-temporary"]
    flags.extend("-I" + str(path) for path in includes)
    objects = [build_root / f"unit-{index:04d}.o" for index in range(len(sources))]

    def compile_unit(index):
        run(
            ["clang++", *flags, "-c", str(sources[index]), "-o", str(objects[index])],
            source_root,
            build_root / f"compile-{index:04d}.log",
        )

    with concurrent.futures.ThreadPoolExecutor(max_workers=jobs) as pool:
        for _ in pool.map(compile_unit, range(len(sources))):
            pass
    # A failed compile never reaches linking or replaces an earlier output.
    pending = build_root / "main.pending.dylib"
    run(
        [
            "clang++",
            "-dynamiclib",
            *flags,
            "-Wl,-install_name,@rpath/main.dylib",
            "-Wl,-rpath,@loader_path/../lib",
            *(str(path) for path in objects),
            "-L" + str(python_root / "lib"),
            "-lpython3.14",
            "-o",
            str(pending),
        ],
        source_root,
        build_root / "link.log",
    )
    pending.replace(output)


def build(source_root, python_root, output, jobs):
    check_host()
    if not 1 <= jobs <= 4:
        raise ValueError("native main build jobs must be between 1 and 4")
    if any(not path.is_absolute() for path in (source_root, python_root, output)):
        raise ValueError("native main build paths must be absolute")
    if not (source_root / "generation_files/generate_classes_cpp.py").is_file():
        raise ValueError("verified Py4Godot source root is missing")
    for name in (
        "bin/python3.14",
        "include/python3.14/Python.h",
        "lib/libpython3.14.dylib",
    ):
        if not (python_root / name).is_file():
            raise ValueError("staged CPython 3.14 runtime is incomplete")
    runtime = subprocess.check_output(
        [
            str(python_root / "bin/python3.14"),
            "-I",
            "-c",
            "import platform; print(platform.python_version(), platform.machine())",
        ],
        text=True,
    ).strip()
    if runtime != "3.14.4 arm64":
        raise ValueError("native main build requires staged CPython 3.14.4 arm64")
    output.parent.mkdir(parents=True, exist_ok=True)
    build_root = output.with_name(output.name + ".build")
    build_root.mkdir(exist_ok=True)
    generate(source_root, python_root, build_root)
    sources = main_sources(source_root, python_root / "bin/python3.14")
    if len(sources) != 1122:
        raise ValueError(
            f"native main source count mismatch: got {len(sources)}, want 1122"
        )
    (build_root / "sources.json").write_text(
        json.dumps([str(path.relative_to(source_root)) for path in sources], indent=2)
        + "\n"
    )
    compile_main(source_root, python_root, output, build_root, sources, jobs)
    print(
        f"Py4Godot native main built: units={len(sources)} sha256={hashlib.sha256(output.read_bytes()).hexdigest()} logs={build_root}"
    )


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--source-root", type=Path, required=True)
    parser.add_argument("--python-root", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--jobs", type=int, default=4)
    args = parser.parse_args()
    build(args.source_root, args.python_root, args.output, args.jobs)


if __name__ == "__main__":
    main()
