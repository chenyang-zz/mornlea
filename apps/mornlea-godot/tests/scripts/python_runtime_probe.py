"""Prove that Godot uses only the pinned project-local embedded interpreter."""

import importlib.util
import os
import platform
import sys

from py4godot.classes import gdclass
from py4godot.classes.Node import Node


@gdclass
class python_runtime_probe(Node):
    def _ready(self) -> None:
        failures: list[str] = []
        expected_version = (3, 14, 4)
        if sys.version_info[:3] != expected_version:
            failures.append(f"unexpected CPython version: {platform.python_version()}")
        expected_architecture = {"darwin": "arm64", "linux": "x86_64"}.get(sys.platform)
        if expected_architecture is None or platform.machine() != expected_architecture:
            failures.append(f"unexpected CPython architecture: {platform.machine()}")
        if sys.flags.isolated != 1:
            failures.append("embedded CPython is not using isolated configuration")
        if sys.flags.ignore_environment != 1:
            failures.append("embedded CPython still accepts environment configuration")
        if sys.flags.no_user_site != 1:
            failures.append("embedded CPython still accepts the user site directory")
        if not sys.flags.safe_path:
            failures.append("embedded CPython safe-path mode is disabled")
        forbidden_pythonpath = os.environ.get("PYTHONPATH", "")
        if not forbidden_pythonpath:
            failures.append("qualification environment was not installed")
        if forbidden_pythonpath in sys.path:
            failures.append("external PYTHONPATH leaked into the embedded interpreter")
        # Every import path must stay inside the embedded runtime distribution.
        runtime_root = os.path.realpath(sys.prefix)
        expected_runtime_root = os.environ.get("MORNLEA_EXPECTED_PYTHON_ROOT", "")
        if not expected_runtime_root or runtime_root != os.path.realpath(expected_runtime_root):
            failures.append(f"unexpected embedded runtime prefix: {runtime_root}")
        working_directory = os.path.realpath(os.getcwd())
        for entry in sys.path:
            resolved_entry = os.path.realpath(entry)
            if not os.path.isabs(entry):
                failures.append(f"relative import path is present: {entry!r}")
                continue
            if os.path.commonpath((runtime_root, resolved_entry)) != runtime_root:
                failures.append(f"import path escapes the embedded runtime: {entry}")
            if resolved_entry == working_directory:
                failures.append("working directory leaked into the embedded import path")
        if importlib.util.find_spec("mornlea_external_runtime_poison") is not None:
            failures.append("a module from external PYTHONPATH remained importable")
        if importlib.util.find_spec("pip") is not None:
            failures.append("the runtime package installer remained importable")
        # The C qualification helper tests real socket denial; presentation
        # Python only confirms that its inherited Linux policy remains active.
        if sys.platform == "linux":
            with open("/proc/self/status", encoding="utf-8") as status_file:
                process_status = dict(line.split(":", 1) for line in status_file if ":" in line)
            if process_status.get("NoNewPrivs", "").strip() != "1":
                failures.append("Linux network denial lost no-new-privileges protection")
            if process_status.get("Seccomp", "").strip() != "2":
                failures.append("Linux network denial lost its inherited seccomp filter")
        if failures:
            for failure in failures:
                print(f"Py4Godot runtime check failed: {failure}")
            self.get_tree().quit(1)
            return
        print("Py4Godot runtime check passed.")
        self.get_tree().quit(0)
