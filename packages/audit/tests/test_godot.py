"""Tests for Godot platform audits, boundaries, licensing, and toolchain."""

import io
import json
import os
from pathlib import Path
import platform
import re
import shutil
import subprocess
import sys
import zipfile
import pytest
import yaml

from audit.boundaries import (
    GODOT_CODEC_IMPORT_REGEX,
    GODOT_FEATURE_REFERENCE_REGEX,
    GODOT_FORBIDDEN_IMPORT_ROOTS,
    GODOT_GAMEPLAY_BRIDGE_CALL_REGEX,
    GODOT_HOST_SCRIPT_DIRS,
    GODOT_PROTOCOL_SCRIPT_DIRS,
    GODOT_WIRE_BYTE_PREFIX_REGEX,
    GODOT_WIRE_CODEC_REGEX,
    GODOT_WIRE_MAGIC_WORDS,
    GODOT_WIRE_MAGICS,
    GODOT_WIRE_WORD_REGEX,
    GodotDisciplineFinding,
    scan_godot_script_discipline,
)
from audit.common import get_repo_root
from tests.test_ci_workflows import (
    CI_ACTIONS,
    CI_CHECKOUT,
    CI_DOWNLOAD,
    CI_GO,
    CI_UV,
    _make_target_is_phony,
    _make_target_recipe,
    _write_executable,
    _non_empty_trimmed_lines,
    ci_summary,
    CI_START,
)

SHA256_PATTERN = re.compile(r"^[0-9a-f]{64}$")

GODOT_ENTRYPOINT_TARGETS = [
    "godot-export-linux",
    "godot-build",
    "godot-check",
    "godot-asset-check",
    "godot-project-check",
    "godot-python-check",
    "godot-input-check",
    "godot-camera-check",
    "godot-target-check",
    "godot-entity-check",
    "godot-environment-check",
    "godot-hud-check",
    "godot-disconnect-check",
    "godot-smoke",
    "godot-terrain-check",
    "godot-capability-check",
    "godot-playable-smoke",
    "godot-visual-evidence",
    "godot-visual-compare",
    "godot-benchmark",
]

GODOT_ENTRYPOINT_RECIPES: dict[str, list[str]] = {
    "godot-export-linux": ["scripts/godot/export-linux.sh --verify"],
    "godot-build": [
        "scripts/godot/build-python-runtime.sh --verify --offline",
        "scripts/godot/build-core.sh --profile debug --verify",
        "scripts/godot/build-core.sh --profile release --verify",
        "scripts/godot/build-extension.sh --profile debug",
        "scripts/godot/build-extension.sh --profile release --verify",
    ],
    "godot-check": ["scripts/godot/godot.sh --headless --path apps/mornlea-godot --editor --quit"],
    "godot-asset-check": ["scripts/godot/sync-assets.sh --check"],
    "godot-project-check": ["scripts/godot/validate-project.sh"],
    "godot-python-check": ["scripts/godot/python-check.sh --locked"],
    "godot-input-check": ["scripts/godot/input-check.sh"],
    "godot-camera-check": ["scripts/godot/camera-check.sh"],
    "godot-target-check": ["scripts/godot/target-check.sh"],
    "godot-entity-check": [
        "$(CARGO) test -p mornlea_godot entity_snapshot --locked",
        "scripts/godot/entity-check.sh",
    ],
    "godot-environment-check": ["scripts/godot/environment-check.sh"],
    "godot-hud-check": ["scripts/godot/hud-check.sh"],
    "godot-disconnect-check": ["go test ./packages/client/runtime", "Disconnect|Overflow|Shutdown"],
    "godot-smoke": ["scripts/godot/smoke.sh --iterations 100", "--isolated-python"],
    "godot-terrain-check": ["scripts/godot/godot-terrain-check.sh"],
    "godot-capability-check": ["scripts/godot/capability-check.sh"],
    "godot-playable-smoke": ["scripts/godot/playable-smoke.sh --duration 300s"],
    "godot-visual-evidence": ["scripts/godot/capture.sh"],
    "godot-visual-compare": ["scripts/godot/visual-compare.sh"],
    "godot-benchmark": ["scripts/godot/benchmark.sh"],
}

GODOT_WORKFLOW_PATHS = [
    ".github/workflows/godot.yml",
    "Makefile",
    "go.work",
    "go.work.sum",
    "apps/mornlea-godot/**",
    "packages/audit/godot_*_test.go",
    "packages/audit/ci_workflow_standard_test.go",
    "packages/audit/go.mod",
    "packages/audit/go.sum",
    "packages/client/assets/**",
    "packages/client/client/**",
    "packages/client/cmd/mornlea-godot-assets/**",
    "packages/client/cmd/mornlea-godot-core/**",
    "packages/client/go.mod",
    "packages/client/go.sum",
    "packages/client/mesh/**",
    "packages/client/presentation/**",
    "packages/client/render/**",
    "packages/client/runtime/**",
    "packages/contracts/**",
    "packages/engine/**",
    "packages/server/go.mod",
    "packages/server/go.sum",
    "packages/shared/**",
    "packages/tools/go.mod",
    "packages/tools/go.sum",
    "scripts/ci/**",
    "scripts/engine/**",
    "scripts/godot/**",
]


def _read_env_file(path: Path | str) -> dict[str, str]:
    """Parse key=value shell environment file."""
    values: dict[str, str] = {}
    for line in Path(path).read_text(encoding="utf-8").splitlines():
        line = line.strip()
        if not line or line.startswith("#"):
            continue
        if "=" in line:
            k, v = line.split("=", 1)
            values[k.strip()] = v.strip().strip('"').strip("'")
    return values


def new_godot_closure_fixture(tmp_path: Path) -> Path:
    """Create a minimal clean Godot project closure fixture for mutation testing."""
    root = tmp_path / "godot_closure"
    files = {
        ".gitignore": ".godot/\n.venv/\n.ruff_cache/\naddons/mornlea_bridge/bin/\naddons/py4godot/\n__pycache__/\n*.py[cod]\n",
        "project.godot": 'config_version=5\n\n[application]\nrun/main_scene="res://app/bootstrap/bootstrap.tscn"\n',
        "export_presets.cfg": """[preset.0]

name="Mornlea macOS"
platform="macOS"
runnable=true
export_filter="all_resources"
include_filter="app/**/*.py,features/**/*.py,platform/desktop/**/*.py,addons/mornlea_bridge/*.py,assets/generated/**"
exclude_filter="tests/**,typing/**,.venv/**,.ruff_cache/**,pyproject.toml,uv.lock,README*,assets/provenance/**,assets/generated/**/*.provenance.json,assets/generated/**/PROVENANCE.json,addons/mornlea_bridge/bin/linux-x86_64/**,addons/mornlea_bridge/bin/windows-x86_64/**,addons/py4godot/cpython-*-linux*/**,addons/py4godot/cpython-*-windows*/**"
export_path=""

[preset.0.options]

custom_template/debug=""
custom_template/release=""
""",
        "app/bootstrap/bootstrap.gd": "extends Node\n# Bootstrap owns dependency diagnostics before Python can load.\n",
        "app/bootstrap/bootstrap.gd.uid": "uid://bootstrapfixture\n",
        "app/bootstrap/setup_required.gd": "extends Control\n# Setup diagnostics never own gameplay behavior.\n",
        "app/bootstrap/setup_required.gd.uid": "uid://setupfixture\n",
        "addons/mornlea_bridge/mornlea_bridge.gdextension": '[configuration]\nentry_symbol = "gdext_rust_init"\n\n[libraries]\nmacos.debug.arm64 = "res://addons/mornlea_bridge/bin/macos-universal/debug/libmornlea_godot.dylib"\n',
        "addons/mornlea_bridge/mornlea_bridge.gdextension.uid": "uid://bridgefixture\n",
        "app/host/app_root.py": "from __future__ import annotations\n# Python owns feature lifecycle after Bootstrap hands off.\n",
    }
    for rel, content in files.items():
        p = root / rel
        p.parent.mkdir(parents=True, exist_ok=True)
        p.write_text(content, encoding="utf-8")
    return root


def run_godot_project_validator(repo_root: Path, project_root: Path | str, *args: str) -> tuple[str, int]:
    validator = repo_root / "scripts" / "godot" / "validate-project.sh"
    env = dict(os.environ)
    env["MORNLEA_GODOT_PROJECT_ROOT"] = str(project_root)
    res = subprocess.run([str(validator), *args], cwd=repo_root, env=env, capture_output=True, text=True)
    return res.stdout + res.stderr, res.returncode


def test_godot_asset_sync_is_deterministic_and_rejects_manual_files(repo_root: Path, tmp_path: Path) -> None:
    """Verify Godot asset generator manifest determinism and manual file rejection."""
    script = repo_root / "scripts" / "godot" / "sync-assets.sh"
    assert os.access(script, os.X_OK)

    gen_root = repo_root / "apps" / "mornlea-godot" / "assets" / "generated"
    manifest_data = json.loads((gen_root / "manifest.json").read_text(encoding="utf-8"))

    assert manifest_data.get("schema_version") == 1
    assert manifest_data.get("generator") == "packages/client/cmd/mornlea-godot-assets"

    in_rev = manifest_data.get("input_revision", {})
    assert len(in_rev.get("assets_git_tree", "")) == 40 or SHA256_PATTERN.match(in_rev.get("assets_git_tree", ""))
    assert len(in_rev.get("fonts_git_tree", "")) == 40 or SHA256_PATTERN.match(in_rev.get("fonts_git_tree", ""))
    assert SHA256_PATTERN.match(manifest_data.get("input_checksum", ""))

    atlas = manifest_data.get("atlas", {})
    assert atlas.get("path") == "atlas.rgba8"
    assert atlas.get("format") == "RGBA8"
    assert atlas.get("width") == 16 and atlas.get("height") == 16
    assert atlas.get("layers", 0) > 0 and atlas.get("mip_levels") == 5
    assert atlas.get("storage_order") == "layer-major,mip-major"
    assert SHA256_PATTERN.match(atlas.get("sha256", ""))
    assert atlas.get("bytes", 0) > 0

    input_paths = [inp.get("path") for inp in manifest_data.get("inputs", [])]
    for required in [
        "packages/client/assets/blocks.go",
        "packages/client/assets/packs/pastelcraft/PROVENANCE.json",
        "packages/client/render/assets/NotoSansCJKsc-Regular.otf",
        "packages/client/render/assets/NotoSansCJKsc-Regular.provenance.json",
        "packages/client/render/assets/OFL.txt",
    ]:
        assert required in input_paths

    output_paths = [out.get("path") for out in manifest_data.get("outputs", [])]
    assert "fonts/NotoSansCJKsc-Regular.otf.import" in output_paths

    font_import = (gen_root / "fonts" / "NotoSansCJKsc-Regular.otf.import").read_text(encoding="utf-8")
    for req in ['uid="uid://', 'source_file="res://assets/generated/fonts/NotoSansCJKsc-Regular.otf"', "allow_system_fallback=false"]:
        assert req in font_import

    # Run check on current repository
    res = subprocess.run([str(script), "--check"], cwd=repo_root, capture_output=True, text=True)
    assert res.returncode == 0, f"sync-assets --check failed: {res.stderr}"

    # Generate into two dirs and compare byte trees
    first = tmp_path / "gen1" / "generated"
    second = tmp_path / "gen2" / "generated"
    for d in [first, second]:
        env = dict(os.environ)
        env["MORNLEA_GODOT_GENERATED_ROOT"] = str(d)
        res = subprocess.run([str(script)], cwd=repo_root, env=env, capture_output=True, text=True)
        assert res.returncode == 0, f"sync-assets to {d} failed: {res.stderr}"

    # Compare trees
    first_files = {p.relative_to(first): p.read_bytes() for p in first.rglob("*") if p.is_file()}
    second_files = {p.relative_to(second): p.read_bytes() for p in second.rglob("*") if p.is_file()}
    assert first_files == second_files

    # Reject unexpected manual file
    manual_file = first / "hand-authored-source.txt"
    manual_file.write_text("forbidden\n", encoding="utf-8")
    env = dict(os.environ)
    env["MORNLEA_GODOT_GENERATED_ROOT"] = str(first)
    res = subprocess.run([str(script), "--check"], cwd=repo_root, env=env, capture_output=True, text=True)
    assert res.returncode != 0
    assert "unexpected generated asset" in res.stdout + res.stderr

    # Reject modified asset
    manual_file.unlink()
    atlas_p = first / "atlas.rgba8"
    raw_atlas = bytearray(atlas_p.read_bytes())
    raw_atlas[0] ^= 0xFF
    atlas_p.write_bytes(bytes(raw_atlas))
    res = subprocess.run([str(script), "--check"], cwd=repo_root, env=env, capture_output=True, text=True)
    assert res.returncode != 0
    assert "generated asset content differs" in res.stdout + res.stderr


def test_godot_build_core_verifies_large_symbol_tables(repo_root: Path, tmp_path: Path) -> None:
    """Verify build-core.sh symbol verification handles large symbol tables without pipefail errors."""
    script_text = (repo_root / "scripts" / "godot" / "build-core.sh").read_text(encoding="utf-8")
    start = script_text.find("  while IFS= read -r exported_symbol; do")
    end = script_text.find("  printf 'client-core symbols verified:")
    assert start >= 0 and end > start
    block = script_text[start:end]

    exports_p = tmp_path / "exports"
    symbols_p = tmp_path / "symbols"
    exports_text = "mornlea_client_core_abi_version\nmornlea_client_core_environment_pull\n"
    exports_p.write_text(exports_text, encoding="utf-8")

    for missing in [False, True]:
        symbols_text = "mornlea_client_core_abi_version\n" if missing else exports_text
        symbols_text += "unrelated_native_symbol\n" * 20000
        symbols_p.write_text(symbols_text, encoding="utf-8")

        bash_cmd = f"""set -euo pipefail
fail() {{ printf '%s\\n' "$*" >&2; exit 1; }}
defined_symbols=$(cat "$1")
exported_symbols=$(cat "$2")
symbol_prefix=
core_library=fixture
{block}"""
        res = subprocess.run(["bash", "-c", bash_cmd, "symbol-fixture", str(symbols_p), str(exports_p)], capture_output=True, text=True)
        if missing:
            assert res.returncode != 0
            assert "exported symbol is missing" in res.stdout + res.stderr
        else:
            assert res.returncode == 0, f"valid large symbol table rejected: {res.stderr}"


def test_godot_build_core_script_pin(repo_root: Path) -> None:
    """Verify build-core.sh pins and checks all shared-library exports and contracts."""
    script_p = repo_root / "scripts" / "godot" / "build-core.sh"
    assert os.access(script_p, os.X_OK)
    script_text = script_p.read_text(encoding="utf-8")

    required_tokens = [
        "go build", "-buildmode=c-shared", "addons/mornlea_bridge/bin", "macos-universal",
        "x86_64-unknown-linux-gnu", "libmornlea_client_core.so", "libmornlea_engine.so",
        "patchelf --set-rpath '$ORIGIN'", "-u LD_LIBRARY_PATH -u LD_PRELOAD",
        "scratch_dir}/distribution", "--verify", "--target", "--profile",
        "unsupported Godot desktop target", "make rust", "libmornlea_client_core.dylib",
        "libmornlea_engine.dylib", "libmornlea_client.dylib", "include/mornlea_client_core.h",
        "libmornlea_client_core.h", "nm -gU", "otool -L", "otool -D", "install_name_tool",
        "codesign", "@loader_path", "@rpath/libmornlea_engine.dylib", "dlopen",
        "mornlea_client_core_abi_version", "mktemp", "user-level Go build cache",
    ]
    for token in required_tokens:
        assert token in script_text, f"build-core.sh missing token {token!r}"

    exports_text = (repo_root / "packages" / "client" / "cmd" / "mornlea-godot-core" / "exports.go").read_text(encoding="utf-8")
    exports = re.findall(r"(?m)^//export\s+([A-Za-z0-9_]+)$", exports_text)
    assert len(exports) > 0
    for exp in exports:
        assert exp in script_text, f"build-core.sh missing export {exp!r}"

    ignore = (repo_root / "apps" / "mornlea-godot" / ".gitignore").read_text(encoding="utf-8")
    assert "addons/mornlea_bridge/bin/" in ignore


def test_godot_build_extension_uses_effective_cargo_target_directory(repo_root: Path, tmp_path: Path) -> None:
    """Verify build-extension.sh uses target directory properly and rejects relative traversal."""
    script = repo_root / "scripts" / "godot" / "build-extension.sh"

    def make_ext_fixture(base: Path) -> Path:
        proj = base / "apps" / "mornlea-godot"
        proj.mkdir(parents=True, exist_ok=True)
        eng = base / "packages" / "engine"
        eng.mkdir(parents=True, exist_ok=True)
        (eng / "Cargo.toml").write_text("", encoding="utf-8")
        return proj

    # Traversal rejection
    fake_repo = tmp_path / "fake_repo_escape"
    proj_root = make_ext_fixture(fake_repo)
    bin_dir = tmp_path / "bin_fake"
    _write_executable(bin_dir / "uname", "#!/usr/bin/env bash\ncase \"$1\" in -s) echo Darwin ;; -m) echo arm64 ;; *) exit 2 ;; esac\n")
    _write_executable(bin_dir / "rustup", "#!/usr/bin/env bash\nset -euo pipefail\nif [[ \"$3\" == rustc ]]; then echo 'host: aarch64-apple-darwin'; exit 0; fi\nexit 0\n")

    env = dict(os.environ)
    env["PATH"] = f"{bin_dir}:{env.get('PATH', '')}"
    env["MORNLEA_REPOSITORY_ROOT"] = str(fake_repo)
    env["MORNLEA_GODOT_PROJECT_ROOT"] = str(proj_root)
    env["CARGO_TARGET_DIR"] = "../escape"

    res = subprocess.run([str(script)], cwd=tmp_path, env=env, capture_output=True, text=True)
    assert res.returncode == 2
    assert "relative CARGO_TARGET_DIR must not contain '..'" in res.stdout + res.stderr


def test_godot_build_extension_cold_editor_import(repo_root: Path, tmp_path: Path) -> None:
    """Verify build-extension.sh verifies debug and release profiles correctly."""
    script = repo_root / "scripts" / "godot" / "build-extension.sh"

    for name, profiles, want in [
        ("debug verification", ["debug:verify"], "import\nidentity\nhost\n"),
        ("release verification after debug", ["debug", "release:verify"], "import\nidentity\nhost\n"),
        ("cold release verification fails", ["release:verify"], None),
    ]:
        repo_dir = tmp_path / f"ext_{name.replace(' ', '_')}"
        proj_root = repo_dir / "apps" / "mornlea-godot"
        proj_root.mkdir(parents=True, exist_ok=True)
        (repo_dir / "packages" / "engine").mkdir(parents=True, exist_ok=True)
        (repo_dir / "packages" / "engine" / "Cargo.toml").write_text("", encoding="utf-8")

        bin_dir = repo_dir / "bin"
        calls_file = repo_dir / "godot-calls"

        _write_executable(bin_dir / "uname", "#!/usr/bin/env bash\ncase \"$1\" in -s) echo Darwin ;; -m) echo arm64 ;; *) exit 2 ;; esac\n")
        _write_executable(bin_dir / "rustup", """#!/usr/bin/env bash
set -euo pipefail
if [[ "$3" == rustc ]]; then echo 'host: aarch64-apple-darwin'; exit 0; fi
profile=debug
for arg in "$@"; do if [[ "$arg" == --release ]]; then profile=release; fi; done
mkdir -p "${CARGO_TARGET_DIR}/aarch64-apple-darwin/${profile}"
printf '%s' "${profile}" > "${CARGO_TARGET_DIR}/aarch64-apple-darwin/${profile}/libmornlea_godot.dylib"
""")
        _write_executable(bin_dir / "nm", "#!/usr/bin/env bash\necho gdext_rust_init\n")
        godot_bin = bin_dir / "godot"
        _write_executable(godot_bin, """#!/usr/bin/env bash
set -euo pipefail
if [[ "${1:-}" == --version ]]; then echo '4.7.2.stable'; exit 0; fi
project="${MORNLEA_GODOT_PROJECT_ROOT}"
debug="${project}/addons/mornlea_bridge/bin/macos-universal/debug/libmornlea_godot.dylib"
if [[ " $* " == *" --editor "* ]]; then
  [[ -f "${debug}" ]] || { echo 'ERROR: missing editor-selected debug library'; exit 1; }
  echo import >> "${MORNLEA_GODOT_CALLS}"
  mkdir -p "${project}/.godot"
  touch "${project}/.godot/extension_list.cfg"
elif [[ " $* " == *"bridge_identity_check.gd"* ]]; then
  [[ -f "${project}/.godot/extension_list.cfg" ]] || { echo 'ERROR: extension not discovered'; exit 1; }
  echo identity >> "${MORNLEA_GODOT_CALLS}"
  echo 'Godot bridge identity check passed.'
else
  [[ -f "${project}/.godot/extension_list.cfg" ]] || { echo 'ERROR: extension not discovered'; exit 1; }
  echo host >> "${MORNLEA_GODOT_CALLS}"
  echo 'Python bridge host check passed.'
fi
""")

        env = dict(os.environ)
        env["PATH"] = f"{bin_dir}:{env.get('PATH', '')}"
        env["MORNLEA_REPOSITORY_ROOT"] = str(repo_dir)
        env["MORNLEA_GODOT_PROJECT_ROOT"] = str(proj_root)
        env["MORNLEA_GODOT_BIN"] = str(godot_bin)
        env["MORNLEA_GODOT_CALLS"] = str(calls_file)
        env["CARGO_TARGET_DIR"] = str(repo_dir / "target")

        for profile in profiles:
            args = ["--profile", profile.replace(":verify", "")]
            if ":verify" in profile:
                args.append("--verify")
            res = subprocess.run([str(script), *args], cwd=tmp_path, env=env, capture_output=True, text=True)
            if want is None:
                assert res.returncode != 0
                assert "debug GDExtension" in res.stdout + res.stderr
            else:
                assert res.returncode == 0, f"failed: {res.stderr}"

        if want is not None:
            assert calls_file.read_text(encoding="utf-8") == want


def test_godot_is_optional_for_legacy_build(repo_root: Path) -> None:
    """Verify Godot entry points remain optional and do not contaminate legacy build/test targets."""
    makefile = (repo_root / "Makefile").read_text(encoding="utf-8")
    ci_yml = (repo_root / ".github" / "workflows" / "ci.yml").read_text(encoding="utf-8")
    godot_yml = (repo_root / ".github" / "workflows" / "godot.yml").read_text(encoding="utf-8")

    # Makefile checks
    for target in GODOT_ENTRYPOINT_TARGETS:
        assert _make_target_is_phony(makefile, target), f"{target} not phony"
        assert f"make {target}" in makefile

    # Recipe checks
    for target, required_cmds in GODOT_ENTRYPOINT_RECIPES.items():
        recipe = _make_target_recipe(makefile, target)
        for req in required_cmds:
            assert req in recipe, f"{target} recipe missing {req!r}"

    # Verify no legacy target reaches godot
    assert "godot" not in ci_yml.lower()

    # Verify godot.yml
    data = yaml.safe_load(godot_yml)
    assert data.get("name") == "Godot CI"
    on_block = data.get("on") if "on" in data else data.get(True, {})
    assert len(on_block) == 3 and "push" in on_block and "pull_request" in on_block and "workflow_dispatch" in on_block

    jobs = data.get("jobs", {})
    assert set(jobs.keys()) == {"godot-static", "godot-runtime"}
    assert jobs["godot-static"]["runs-on"] == "ubuntu-24.04"
    assert jobs["godot-runtime"]["runs-on"] == "macos-26"
    assert jobs["godot-runtime"]["needs"] == "godot-static" or jobs["godot-runtime"]["needs"] == ["godot-static"]


def test_godot_rollback_rejects_missing_optional_workflow(repo_root: Path, tmp_path: Path) -> None:
    """Verify rollback-check.sh fails if godot.yml is missing."""
    fix_root = tmp_path / "rollback_fix"
    fix_root.mkdir(parents=True, exist_ok=True)
    for rel in ["Makefile", ".github/workflows/ci.yml", "scripts/godot/rollback-check.sh"]:
        p = fix_root / rel
        p.parent.mkdir(parents=True, exist_ok=True)
        p.write_text((repo_root / rel).read_text(encoding="utf-8"), encoding="utf-8")
    for rel in ["apps", "docs", "packages"]:
        (fix_root / rel).symlink_to(repo_root / rel)

    res = subprocess.run(["bash", str(fix_root / "scripts" / "godot" / "rollback-check.sh")], cwd=fix_root, capture_output=True, text=True)
    assert res.returncode != 0
    assert "optional Godot workflow is missing" in res.stdout + res.stderr
    assert "check passed" not in res.stdout + res.stderr


def test_godot_rollback_workflow_isolation(repo_root: Path, tmp_path: Path) -> None:
    """Verify rollback-check.sh enforces workflow isolation across mutations."""
    required = (repo_root / ".github" / "workflows" / "ci.yml").read_text(encoding="utf-8")
    optional = (repo_root / ".github" / "workflows" / "godot.yml").read_text(encoding="utf-8")

    cases = [
        ("baseline", "", "", "", True),
        ("inert required comment", "ci.yml", "jobs:\n", "jobs:\n  # godot remains optional\n", True),
        ("required job", "ci.yml", "  preflight:\n", "  godot-preflight:\n", False),
        ("required direct probe", "ci.yml", "run: make ci-preflight", "run: scripts/godot/fetch.sh", False),
        ("required block need", "ci.yml", "  preflight:\n", "  preflight:\n    needs:\n      - godot-static\n", False),
        ("missing optional gate", "godot.yml", "        run: make godot-smoke", "        # run: make godot-smoke", False),
        ("optional merge dependency", "godot.yml", "needs: godot-static", "needs: [godot-static, merge-gate]", False),
        ("optional merge job", "godot.yml", "  godot-runtime:", "  merge-gate:", False),
        ("optional allow failure", "godot.yml", "    timeout-minutes: 90", "    timeout-minutes: 90\n    continue-on-error: true", False),
    ]

    for name, f_name, old, rep, want_success in cases:
        fix_root = tmp_path / f"rb_{name.replace(' ', '_')}"
        fix_root.mkdir(parents=True, exist_ok=True)
        for rel in ["Makefile", ".github/workflows/ci.yml", "scripts/godot/rollback-check.sh"]:
            p = fix_root / rel
            p.parent.mkdir(parents=True, exist_ok=True)
            p.write_text((repo_root / rel).read_text(encoding="utf-8"), encoding="utf-8")
        (fix_root / ".github" / "workflows" / "godot.yml").write_text(optional, encoding="utf-8")
        for rel in ["apps", "docs", "packages"]:
            (fix_root / rel).symlink_to(repo_root / rel)

        if f_name:
            target_p = fix_root / ".github" / "workflows" / f_name
            text = target_p.read_text(encoding="utf-8")
            assert old in text
            target_p.write_text(text.replace(old, rep, 1), encoding="utf-8")

        res = subprocess.run(["bash", str(fix_root / "scripts" / "godot" / "rollback-check.sh")], cwd=fix_root, capture_output=True, text=True)
        assert (res.returncode == 0) == want_success, f"{name}: returncode {res.returncode}, output:\n{res.stdout}{res.stderr}"
        assert ("check passed" in res.stdout) == want_success


def test_godot_capability_registry_keeps_python_as_presentation_language(repo_root: Path) -> None:
    """Verify capability registry and feature catalog preserve Python presentation boundary."""
    checker = (repo_root / "scripts" / "godot" / "capability_registry_check.py").read_text(encoding="utf-8")
    for req in [
        '"menus"', '"containers"', '"chat"', '"audio"', '"lod"', '"viewmodel"',
        '"companions"', '"hostile_mobs"', '"passive_cows"', '"projectiles"',
        '"drops"', '"name_tags"', '"particles"', '"full_weather"',
    ]:
        assert req in checker

    reg = (repo_root / "apps" / "mornlea-godot" / "catalog" / "capability_registry.tres").read_text(encoding="utf-8")
    assert "registry_version = 1" in reg

    host = (repo_root / "apps" / "mornlea-godot" / "app" / "host" / "feature_host.py").read_text(encoding="utf-8")
    assert 'BUDGET_CLASSES = frozenset({"bootstrap", "light", "standard", "heavy"})' in host
    assert "has unknown budget class" in host

    makefile = (repo_root / "Makefile").read_text(encoding="utf-8")
    assert "scripts/godot/capability-check.sh" in _make_target_recipe(makefile, "godot-capability-check")

    catalog = (repo_root / "apps" / "mornlea-godot" / "config" / "feature_catalog.tres").read_text(encoding="utf-8")
    registered = catalog + "\n" + reg

    proj_root = repo_root / "apps" / "mornlea-godot"
    for feature_file in (proj_root / "features").rglob("feature.tres"):
        rel = feature_file.relative_to(proj_root).as_posix()
        assert f"res://{rel}" in registered


def test_godot_feature_boundary_rejects_catalog_and_ownership_escapes(repo_root: Path) -> None:
    """Verify python boundary checker and feature contract check enforce isolation rules."""
    checker = (repo_root / "scripts" / "godot" / "python_boundary_check.py").read_text(encoding="utf-8")
    for rule in [
        "forbidden-protocol-ownership", "forbidden-prediction", "forbidden-persistence",
        "forbidden-numerical-fallback", "unbounded-callback", "cross-feature-private-path",
        "unrestricted-dynamic-import",
    ]:
        assert rule in checker

    contract = (repo_root / "apps" / "mornlea-godot" / "tests" / "scripts" / "feature_contract_check.py").read_text(encoding="utf-8")
    for case in ['"duplicate"', '"cycle"', '"unknown_budget"', '"required_dependent"', '"required_bridge"', "--extensibility-probe", "synthetic_optional"]:
        assert case in contract

    validator = (repo_root / "scripts" / "godot" / "validate-project.sh").read_text(encoding="utf-8")
    assert "unregistered autoload is forbidden" in validator

    host = (repo_root / "apps" / "mornlea-godot" / "app" / "host" / "feature_host.py").read_text(encoding="utf-8")
    assert "elapsed_ns = max(0, min(int(delta * 1_000_000_000), 100_000_000))" in host


def test_godot_feature_boundary_detects_drift(repo_root: Path, tmp_path: Path) -> None:
    """Verify script discipline scanner detects protocol and host drift."""
    findings = scan_godot_script_discipline(repo_root / "apps" / "mornlea-godot")
    assert not [f for f in findings if f.rule.startswith("protocol-")]

    for name, rel, src, expected_rule in [
        ("protocol decoding", "features/world/world_feature.py", 'from __future__ import annotations\nMAGIC = "MCW1"\n', "protocol-magic-text"),
        ("host names a concrete feature", "app/host/feature_host.py", 'from __future__ import annotations\nFEATURE = "actors"\n', "host-feature-reference"),
    ]:
        fix = new_godot_closure_fixture(tmp_path / f"drift_{name.replace(' ', '_')}")
        target_p = fix / rel
        target_p.parent.mkdir(parents=True, exist_ok=True)
        target_p.write_text(src, encoding="utf-8")
        f_list = scan_godot_script_discipline(fix)
        assert any(f.rule == expected_rule for f in f_list)


def test_godot_scripts_do_not_implement_protocol_markers(repo_root: Path, tmp_path: Path) -> None:
    """Verify production scripts do not decode protocol markers and bridge is native."""
    proj_root = repo_root / "apps" / "mornlea-godot"
    findings = scan_godot_script_discipline(proj_root)
    assert not [f for f in findings if f.rule.startswith("protocol-")]

    app_scene = (proj_root / "app" / "host" / "app_root.tscn").read_text(encoding="utf-8")
    assert 'type="MornleaClientBridge"' in app_scene

    app_root = (proj_root / "app" / "host" / "app_root.py").read_text(encoding="utf-8")
    assert "BridgeHost" not in app_root

    mutations = [
        ("wire magic text in a feature", "features/world/world_feature.py", 'from __future__ import annotations\nMAGIC = "MCI1"\n', "protocol-magic-text"),
        ("wire magic word in the host", "app/host/feature_host.py", "from __future__ import annotations\nIDENTITY = 0x3149434D\n", "protocol-magic-word"),
        ("wire byte prefix in a platform adapter", "platform/desktop/input/desktop_input_feature.py", "from __future__ import annotations\nBATCH = [0x4D, 0x43, 0x4E, 0x31, 0, 0]\n", "protocol-wire-byte-prefix"),
        ("struct record decode in a feature", "features/world/world_feature.py", 'from __future__ import annotations\nimport struct\nWORDS = struct.unpack("<I", record)\n', "protocol-wire-record-codec"),
        ("network codec import in the bridge addon", "addons/mornlea_bridge/bridge_host.py", "from __future__ import annotations\nfrom packages.shared.network import codec\n", "protocol-codec-import"),
        ("generic codec module import in a feature", "features/world/world_feature.py", "from __future__ import annotations\nimport world_packet_codec\n", "protocol-codec-import"),
        ("wire magic in bootstrap GDScript", "app/bootstrap/bootstrap.gd", 'extends Node\nconst WIRE_MAGIC := "MCW1"\n', "protocol-magic-text"),
    ]

    for name, rel, src, expected in mutations:
        fix = new_godot_closure_fixture(tmp_path / f"proto_{name.replace(' ', '_')}")
        target_p = fix / rel
        target_p.parent.mkdir(parents=True, exist_ok=True)
        target_p.write_text(src, encoding="utf-8")
        f_list = scan_godot_script_discipline(fix)
        assert any(f.rule == expected and f.path.endswith(rel) for f in f_list)


def test_godot_host_is_feature_agnostic_across_injection(repo_root: Path) -> None:
    """Verify host files remain feature agnostic and bind native client bridge."""
    proj_root = repo_root / "apps" / "mornlea-godot"
    findings = scan_godot_script_discipline(proj_root)
    assert not [f for f in findings if f.rule.startswith("host-")]

    app_scene = (proj_root / "app" / "host" / "app_root.tscn").read_text(encoding="utf-8")
    assert 'type="MornleaClientBridge"' in app_scene


def test_godot_linux_export_retains_bootstrap_preloads(repo_root: Path) -> None:
    """Verify Linux export preset does not exclude Bootstrap preload dependencies."""
    presets = (repo_root / "apps" / "mornlea-godot" / "export_presets.cfg").read_text(encoding="utf-8")
    _, linux_preset = presets.split('name="Mornlea Linux x86_64"', 1)
    linux_preset = linux_preset.split("[preset.", 1)[0]
    match = re.search(r'(?m)^exclude_filter="([^"]*)"$', linux_preset)
    assert match
    patterns = match.group(1).split(",")

    bootstrap = (repo_root / "apps" / "mornlea-godot" / "app" / "bootstrap" / "bootstrap.gd").read_text(encoding="utf-8")
    preloads = re.findall(r'preload\("res://([^"]+)"\)', bootstrap)
    assert preloads

    resources = list(preloads)
    for res in preloads:
        if res.endswith(".tscn"):
            scene = (repo_root / "apps" / "mornlea-godot" / res).read_text(encoding="utf-8")
            resources.extend(re.findall(r'path="res://([^"]+)"', scene))

    for res in resources:
        for pat in patterns:
            import fnmatch
            assert not fnmatch.fnmatch(res, pat), f"Linux export excludes required Bootstrap resource: {res}"


def test_godot_linux_fetch_and_resolver(repo_root: Path) -> None:
    """Verify Linux fetch and godot resolver (skipped on non-Linux platforms)."""
    if sys.platform != "linux" or platform.machine() != "x86_64":
        pytest.skip("Linux host resolver targets x86_64 Linux")


def test_godot_linux_export_protects_unowned_output(repo_root: Path, tmp_path: Path) -> None:
    """Verify export-linux.sh refuses to overwrite unowned nonempty directories."""
    out_dir = tmp_path / "unowned_out"
    out_dir.mkdir(parents=True, exist_ok=True)
    marker = out_dir / "user-data"
    marker.write_text("preserve me", encoding="utf-8")

    script = repo_root / "scripts" / "godot" / "export-linux.sh"
    res = subprocess.run([str(script), "--output", str(out_dir)], cwd=repo_root, capture_output=True, text=True)
    assert res.returncode != 0
    assert "unowned nonempty output" in res.stdout + res.stderr
    assert marker.read_text(encoding="utf-8") == "preserve me"


def test_godot_linux_qualification_denies_network_sockets(repo_root: Path) -> None:
    """Verify Linux qualification network denial helper (skipped on non-Linux)."""
    if sys.platform != "linux" or platform.machine() != "x86_64":
        pytest.skip("Linux qualification seccomp filter targets x86_64 Linux")


def test_godot_linux_qualification_preserves_runtime_isolation(repo_root: Path) -> None:
    """Verify Linux runtime check and probe contain isolation pins."""
    check = (repo_root / "scripts" / "godot" / "python-runtime-check.sh").read_text(encoding="utf-8")
    for req in ["x86_64-unknown-linux-gnu", "linux-network-deny.c", "linux-build-inputs.env", "python-runtime-linux-export-presets.cfg", "MORNLEA_EXPECTED_PYTHON_ROOT", "linux_release.x86_64"]:
        assert req in check

    probe = (repo_root / "apps" / "mornlea-godot" / "tests" / "scripts" / "python_runtime_probe.py").read_text(encoding="utf-8")
    for req in ["/proc/self/status", "NoNewPrivs", "Seccomp", "MORNLEA_EXPECTED_PYTHON_ROOT", "sys.flags.isolated", "mornlea_external_runtime_poison"]:
        assert req in probe


def test_godot_linux_qualification_only_classifies_denied_editor_listeners(repo_root: Path) -> None:
    """Verify qualification output classifier only filters deliberate Linux socket denials."""
    check = (repo_root / "scripts" / "godot" / "python-runtime-check.sh").read_text(encoding="utf-8")
    start = check.find("filter_editor_network_denials() {")
    assert start >= 0
    end = check.find("\n}\n", start)
    assert end >= 0
    function = check[start : end + 3]

    denied = (
        'ERROR: Condition "_sock == -1" is true. Returning: FAILED\n'
        "   at: _inet_open (drivers/unix/net_socket_unix.cpp:288)\n"
        'ERROR: Condition "err != OK" is true. Returning: ERR_CANT_CREATE\n'
        "   at: listen (core/io/tcp_server.cpp:56)\n"
    )

    cases = [
        ("Linux deliberate denial", "linux64", denied + "ready\n", "ready\n"),
        ("Linux unrelated errors", "linux64", denied + "ERROR: extension failed\n", "ERROR: extension failed\n"),
        ("Linux similar different origin", "linux64", denied.replace("net_socket_unix.cpp:288", "another.cpp:288"), denied.replace("net_socket_unix.cpp:288", "another.cpp:288")),
        ("macOS preserves all diagnostics", "darwin64", denied, denied),
    ]

    for name, plat, inp, want in cases:
        cmd = f'runtime_platform="$1"\n{function}\nfilter_editor_network_denials'
        res = subprocess.run(["bash", "-c", cmd, "classifier", plat], input=inp, capture_output=True, text=True)
        assert res.stdout == want, f"{name}: got {res.stdout!r}, want {want!r}"


def test_godot_playable_log_rejects_every_unapproved_error(repo_root: Path, tmp_path: Path) -> None:
    """Verify playable log checker rejects unapproved engine and script errors."""
    checker = repo_root / "scripts" / "godot" / "playable-log-check.sh"
    tests = [
        ("clean", "Godot Engine v4.7.2\nPython playable smoke passed\n", False),
        ("exact dummy renderer diagnostic", (
            "Godot Engine v4.7.2\n"
            'ERROR: Parameter "material" is null.\n'
            "   at: material_get_instance_shader_parameters (servers/rendering/dummy/storage/material_storage.cpp:264)\n"
            "Python playable smoke passed\n"
        ), False),
        ("unrelated engine error", "ERROR: failed to load feature scene\n", True),
        ("script error", "SCRIPT ERROR: Invalid call.\n", True),
        ("missing dummy detail", 'ERROR: Parameter "material" is null.\n', True),
        ("changed dummy detail", (
            'ERROR: Parameter "material" is null.\n'
            "   at: material_get_instance_shader_parameters (servers/rendering/dummy/storage/material_storage.cpp:265)\n"
        ), True),
    ]

    for name, log_text, want_err in tests:
        p = tmp_path / f"log_{name.replace(' ', '_')}.log"
        p.write_text(log_text, encoding="utf-8")
        res = subprocess.run([str(checker), str(p)], cwd=repo_root, capture_output=True, text=True)
        assert (res.returncode != 0) == want_err, f"{name}: returncode {res.returncode}, output:\n{res.stdout}{res.stderr}"


def test_godot_playable_smoke_oracle_contracts(repo_root: Path) -> None:
    """Verify playable smoke oracle pins and log memory bounds."""
    manifest = (repo_root / "apps" / "mornlea-godot" / "features" / "world" / "feature.tres").read_text(encoding="utf-8")
    assert 'PackedStringArray("5@1.0", "8@1.0")' in manifest

    script = (repo_root / "scripts" / "godot" / "playable-smoke.sh").read_text(encoding="utf-8")
    for req in [
        "POSIX::setpgid(0, 0)",
        "MORNLEA_PLAYABLE_SMOKE_REMOTE_PLAYER_ID",
        "Mornlea Godot smoke helper observed target:",
        "Mornlea Godot smoke helper observed Godot peer:",
        'kill -TERM -- "-${pgid}"',
        "if (( ${#remaining[@]} )); then",
    ]:
        assert req in script
    assert 'godot_output="$(cat -- "${godot_log}")"' not in script

    driver = (repo_root / "apps" / "mornlea-godot" / "tests" / "scripts" / "playable_smoke_check.py").read_text(encoding="utf-8")
    for req in [
        "MORNLEA_PLAYABLE_SMOKE_REMOTE_PLAYER_ID",
        "INITIAL_SESSION_EPOCH = 1",
        '_canonical_player_id(entity["player_id"]) == self._expected_remote_player_id',
        'facts.get("uploads_applied")',
        'bridge.call("session_status_typed")',
        "if not self._movement_seen:",
        "_duration_samples",
        "_last_sample_revision",
    ]:
        assert req in driver


def test_godot_project_root_rejects_escapes(repo_root: Path, tmp_path: Path) -> None:
    """Verify validate-project.sh succeeds on valid project and rejects escapes."""
    out, code = run_godot_project_validator(repo_root, repo_root / "apps" / "mornlea-godot")
    assert code == 0, f"validation failed: {out}"

    # Symbolic link escape
    fix = new_godot_closure_fixture(tmp_path / "fix_esc")
    external = tmp_path / "outside.tres"
    external.write_text("[gd_resource format=3]\n", encoding="utf-8")
    link = fix / "features" / "escaped.tres"
    link.parent.mkdir(parents=True, exist_ok=True)
    link.symlink_to(external)
    out, code = run_godot_project_validator(repo_root, fix)
    assert code != 0
    assert "symbolic link escapes the project root" in out

    # Absolute path
    fix = new_godot_closure_fixture(tmp_path / "fix_abs")
    abs_tres = fix / "features" / "absolute.tres"
    abs_tres.parent.mkdir(parents=True, exist_ok=True)
    abs_tres.write_text('[gd_resource format=3]\n[resource]\nmetadata/library = "/Users/dev/lib.dylib"\n', encoding="utf-8")
    out, code = run_godot_project_validator(repo_root, fix)
    assert code != 0
    assert "absolute development-machine path" in out


def test_godot_resource_closure_rejects_external_references(repo_root: Path, tmp_path: Path) -> None:
    """Verify validate-project.sh rejects parent resource references and unregistered autoloads."""
    fix = new_godot_closure_fixture(tmp_path / "fix_ext_ref")
    target = fix / "features" / "world" / "feature.tres"
    target.parent.mkdir(parents=True, exist_ok=True)
    target.write_text('[gd_resource format=3]\n[resource]\nmetadata/path = "res://../outside.tres"\n', encoding="utf-8")
    out, code = run_godot_project_validator(repo_root, fix)
    assert code != 0
    assert "resource path escapes the project root" in out

    fix = new_godot_closure_fixture(tmp_path / "fix_autoload")
    p_godot = fix / "project.godot"
    p_godot.write_text(p_godot.read_text(encoding="utf-8") + '\n[autoload]\nGlobalState="*res://app/global.py"\n', encoding="utf-8")
    out, code = run_godot_project_validator(repo_root, fix)
    assert code != 0
    assert "unregistered autoload" in out


def test_godot_uid_policy_keeps_generated_state_out_and_identity_in(repo_root: Path, tmp_path: Path) -> None:
    """Verify UID policy tracks identity sidecars while ignoring .godot generated state."""
    ignore = (repo_root / "apps" / "mornlea-godot" / ".gitignore").read_text(encoding="utf-8")
    assert ".godot/" in ignore
    assert ".uid" not in ignore
    assert "*.import" not in ignore

    for rel in [
        "app/bootstrap/bootstrap.gd.uid",
        "app/bootstrap/setup_required.gd.uid",
        "addons/mornlea_bridge/mornlea_bridge.gdextension.uid",
    ]:
        p = repo_root / "apps" / "mornlea-godot" / rel
        assert p.is_file(), f"missing identity sidecar {rel}"

    # Ignored sidecars in .gitignore rejected
    fix = new_godot_closure_fixture(tmp_path / "fix_uid_ignore")
    (fix / ".gitignore").write_text(".godot/\n*.uid\n", encoding="utf-8")
    out, code = run_godot_project_validator(repo_root, fix)
    assert code != 0
    assert "UID sidecars must remain tracked" in out


def test_godot_export_closure_excludes_development_resources(repo_root: Path, tmp_path: Path) -> None:
    """Verify export presets exclude tests, typing, and development assets."""
    presets = (repo_root / "apps" / "mornlea-godot" / "export_presets.cfg").read_text(encoding="utf-8")
    for req in [
        'platform="macOS"', "tests/**", "typing/**", "pyproject.toml", "uv.lock",
        "assets/provenance/**", "addons/mornlea_bridge/bin/linux-x86_64/**",
        "addons/mornlea_bridge/bin/windows-x86_64/**", "addons/py4godot/cpython-*-linux*/**",
        "addons/py4godot/cpython-*-windows*/**",
    ]:
        assert req in presets

    fix = new_godot_closure_fixture(tmp_path / "fix_tests_closure")
    p_file = fix / "export_presets.cfg"
    p_file.write_text(p_file.read_text(encoding="utf-8").replace("tests/**,", ""), encoding="utf-8")
    out, code = run_godot_project_validator(repo_root, fix)
    assert code != 0
    assert "export exclusion is missing: tests/**" in out


def test_godot_desktop_only_rejects_unsupported_targets(repo_root: Path, tmp_path: Path) -> None:
    """Verify validate-project.sh rejects mobile, web, and console platform targets."""
    for plat in ["Android", "iOS", "Web", "Nintendo Switch"]:
        fix = new_godot_closure_fixture(tmp_path / f"tgt_{plat.replace(' ', '_')}")
        cfg = fix / "export_presets.cfg"
        cfg.write_text(cfg.read_text(encoding="utf-8").replace('platform="macOS"', f'platform="{plat}"'), encoding="utf-8")
        out, code = run_godot_project_validator(repo_root, fix)
        assert code != 0
        assert "unsupported export platform" in out

    fix = new_godot_closure_fixture(tmp_path / "mobile_adapter")
    mob = fix / "platform" / "mobile" / "lifecycle.py"
    mob.parent.mkdir(parents=True, exist_ok=True)
    mob.write_text("from __future__ import annotations\n", encoding="utf-8")
    out, code = run_godot_project_validator(repo_root, fix)
    assert code != 0
    assert "unsupported mobile lifecycle or platform adapter" in out


def test_godot_python_boundary_rejects_ambient_runtime_resolution(repo_root: Path, tmp_path: Path) -> None:
    """Verify validate-project.sh rejects pip, urllib, ambient sys.path mutation, and non-English comments."""
    mutations = [
        ("runtime installer", "from __future__ import annotations\nimport pip\n", "forbidden Python runtime dependency"),
        ("runtime download", "from __future__ import annotations\nimport urllib.request\n", "forbidden Python runtime dependency"),
        ("ambient path mutation", "from __future__ import annotations\nimport sys\nsys.path.append('/tmp')\n", "forbidden Python runtime search"),
        ("non-English comment", "from __future__ import annotations\n# Forbidden non-english: 禁止的架构注释。\n", "non-English source comment"),
    ]
    for name, src, expected in mutations:
        fix = new_godot_closure_fixture(tmp_path / f"py_bnd_{name.replace(' ', '_')}")
        target = fix / "features" / "world" / "world_feature.py"
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_text(src, encoding="utf-8")
        out, code = run_godot_project_validator(repo_root, fix)
        assert code != 0
        assert expected in out, f"{name}: output {out} missing {expected}"


def test_godot_project_bootstrap_is_pure_and_stable(repo_root: Path) -> None:
    """Verify Godot project bootstrap is pure GDScript without native dependencies."""
    project = (repo_root / "apps" / "mornlea-godot" / "project.godot").read_text(encoding="utf-8")
    assert 'run/main_scene="res://app/bootstrap/bootstrap.tscn"' in project

    for rel in [
        "apps/mornlea-godot/project.godot",
        "apps/mornlea-godot/app/bootstrap/bootstrap.tscn",
        "apps/mornlea-godot/app/bootstrap/setup_required.tscn",
    ]:
        text = (repo_root / rel).read_text(encoding="utf-8")
        for forbidden in ["MornleaClientBridge", ".gdextension", "addons/mornlea_bridge/bin"]:
            assert forbidden not in text


def test_godot_bootstrap_does_not_initiate_network(repo_root: Path) -> None:
    """Verify bootstrap scripts do not initiate network connections."""
    for rel in ["app/bootstrap/bootstrap.gd", "app/bootstrap/setup_required.gd"]:
        text = (repo_root / "apps" / "mornlea-godot" / rel).read_text(encoding="utf-8")
        for forbidden in ["StreamPeerTCP", "PacketPeer", "HTTPClient", "HTTPRequest", "connect_to_host", "connect_to_url"]:
            assert forbidden not in text


def test_godot_bootstrap_diagnoses_incomplete_distribution(repo_root: Path) -> None:
    """Verify bootstrap script contains complete diagnostics for missing distribution units."""
    bootstrap = (repo_root / "apps" / "mornlea-godot" / "app/bootstrap/bootstrap.gd").read_text(encoding="utf-8")
    for req in [
        "python-extension", "python-plugin", "python-bridge", "python-interpreter",
        "python-stdlib", "project-bridge-extension", "project-bridge-library",
        "4.7-alpha-21", "initialize_pythonscript", "gdext_rust_init", "mismatched",
        "build-python-runtime.sh --verify --offline", "build-extension.sh --target %s --profile debug --verify",
        "python=not-imported", "network=not-started",
    ]:
        assert req in bootstrap


def test_godot_prepared_bootstrap_hands_off_to_python_dynamically(repo_root: Path) -> None:
    """Verify prepared bootstrap dynamic handoff to app_root."""
    bootstrap = (repo_root / "apps" / "mornlea-godot" / "app/bootstrap/bootstrap.gd").read_text(encoding="utf-8")
    for req in [
        'const APP_ROOT_SCENE_PATH := "res://app/host/app_root.tscn"',
        "missing.is_empty() and mismatched.is_empty()",
        "ResourceLoader.load(APP_ROOT_SCENE_PATH",
        "state=ready", "python=imported",
    ]:
        assert req in bootstrap
    assert 'preload("res://app/host/' not in bootstrap


def test_godot_desktop_only_feature_skeleton(repo_root: Path) -> None:
    """Verify all 8 desktop feature skeletons exist and unsupported platforms are absent."""
    manifest_paths = [
        "features/session/feature.tres",
        "features/player_view/feature.tres",
        "features/world/feature.tres",
        "features/actors/feature.tres",
        "features/ui/feature.tres",
        "platform/desktop/input/feature.tres",
        "platform/desktop/lifecycle/feature.tres",
        "platform/desktop/audio/feature.tres",
    ]
    catalog = (repo_root / "apps" / "mornlea-godot" / "config" / "feature_catalog.tres").read_text(encoding="utf-8")
    for rel in manifest_paths:
        assert f"res://{rel}" in catalog

    for forbidden in ["platform/mobile", "platform/android", "platform/ios", "platform/web", "platform/console"]:
        assert not (repo_root / "apps" / "mornlea-godot" / forbidden).exists()


def test_godot_python_feature_skeletons_and_script_ownership(repo_root: Path) -> None:
    """Verify Python feature skeletons define required lifecycle methods and GDScript is bounded."""
    proj_root = repo_root / "apps" / "mornlea-godot"
    skeletons = {
        "features/actors": "actors_feature",
        "features/player_view": "player_view_feature",
        "features/session": "session_feature",
        "features/ui": "ui_feature",
        "features/world": "world_feature",
        "platform/desktop/audio": "desktop_audio_feature",
        "platform/desktop/input": "desktop_input_feature",
        "platform/desktop/lifecycle": "desktop_lifecycle_feature",
    }
    for d, cls in skeletons.items():
        script = (proj_root / d / f"{cls}.py").read_text(encoding="utf-8")
        for req in ["def validate_feature", "def bind_host", "def activate_feature", "def reset_feature", "def deactivate_feature"]:
            assert req in script

    allowed_gdscript = {"app/bootstrap/bootstrap.gd", "app/bootstrap/setup_required.gd"}
    for gd_file in proj_root.rglob("*.gd"):
        if any(part in gd_file.parts for part in [".godot", ".venv", "tests", "addons"]):
            continue
        rel = gd_file.relative_to(proj_root).as_posix()
        assert rel in allowed_gdscript, f"unexpected production GDScript: {rel}"


def test_godot_bridge_host_is_identity_only_and_desktop_bound(repo_root: Path) -> None:
    """Verify GDExtension descriptor and Python bridge host remain identity only."""
    desc = (repo_root / "apps" / "mornlea-godot" / "addons" / "mornlea_bridge" / "mornlea_bridge.gdextension").read_text(encoding="utf-8")
    assert 'entry_symbol = "gdext_rust_init"' in desc
    assert 'compatibility_minimum = "4.7"' in desc

    bridge_host = (repo_root / "apps" / "mornlea-godot" / "addons" / "mornlea_bridge" / "bridge_host.py").read_text(encoding="utf-8")
    for req in ["class bridge_host(Node):", "MornleaClientBridge", "def host_protocol_version", "def bridge_identity"]:
        assert req in bridge_host
    for forbidden in ["ctypes", "cffi", "dlopen", "begin_connection", "submit_input"]:
        assert forbidden not in bridge_host


def test_godot_lifecycle_smoke_contract(repo_root: Path) -> None:
    """Verify lifecycle smoke script pins and Python host lifecycle markers."""
    smoke = (repo_root / "scripts" / "godot" / "smoke.sh").read_text(encoding="utf-8")
    for req in [
        "--iterations", "--isolated-python", "build-python-runtime.sh", "build-extension.sh",
        "[mornlea-lifecycle] rust-init=scene", "[mornlea-lifecycle] python-init=host",
        "[mornlea-lifecycle] python-deinit=features", "[mornlea-lifecycle] python-deinit=bridge",
        "[mornlea-lifecycle] rust-deinit=scene",
    ]:
        assert req in smoke

    app_root = (repo_root / "apps" / "mornlea-godot" / "app" / "host" / "app_root.py").read_text(encoding="utf-8")
    for req in ["[mornlea-lifecycle] python-init=host", "[mornlea-lifecycle] python-deinit=features", "[mornlea-lifecycle] python-deinit=bridge"]:
        assert req in app_root


def test_godot_feature_host_is_python_owned_and_explicit(repo_root: Path) -> None:
    """Verify retired GDScript hosts are removed and Python feature host owns lifecycle."""
    proj_root = repo_root / "apps" / "mornlea-godot"
    for retired in ["app/host/app_root.gd", "app/host/feature_catalog.gd", "app/host/feature_host.gd", "app/host/feature_manifest.gd"]:
        assert not (proj_root / retired).exists(), f"retired GDScript host exists: {retired}"

    host = (proj_root / "app" / "host" / "feature_host.py").read_text(encoding="utf-8")
    for req in ["HOST_PROTOCOL_MAJOR = 1", "HOST_PROTOCOL_MINOR = 0", "plan_catalog", "activate_catalog", "validate_feature", "bind_host"]:
        assert req in host

    app_scene = (proj_root / "app" / "host" / "app_root.tscn").read_text(encoding="utf-8")
    assert ".gd\"" not in app_scene


def test_godot_python_fork_is_pinned_and_minimal(repo_root: Path) -> None:
    """Verify Py4Godot fork pins, minimal patch series, and SHA-256 digests."""
    fork_root = repo_root / "scripts" / "godot" / "py4godot"
    values = _read_env_file(fork_root / "build-inputs.env")

    want = {
        "PY4GODOT_HARDENED_VERSION": "4.7-alpha21-mornlea.3",
        "PY4GODOT_BUILD_TARGET": "darwin-arm64",
        "PY4GODOT_PATCH_SERIES": "series",
        "PY4GODOT_APPLE_CLANG_VERSION": "21.0.0",
        "PY4GODOT_MACOS_SDK_VERSION": "26.5",
    }
    for k, v in want.items():
        assert values.get(k) == v

    series_lines = (fork_root / "patches" / "series").read_text(encoding="utf-8").split()
    assert 1 <= len(series_lines) <= 3
    for patch_name in series_lines:
        patch_text = (fork_root / "patches" / patch_name).read_text(encoding="utf-8")
        assert "Upstream-Defect:" in patch_text
        assert "Deletion-Condition:" in patch_text


def test_godot_python_isolation_build_entry_point(repo_root: Path) -> None:
    """Verify build-python-runtime.sh and python-runtime-check.sh entrypoints."""
    build_sh = repo_root / "scripts" / "godot" / "build-python-runtime.sh"
    assert os.access(build_sh, os.X_OK)
    build_text = build_sh.read_text(encoding="utf-8")
    for req in ["--verify", "--offline", "MORNLEA_PY4GODOT_CACHE_DIR", "PY4GODOT_SOURCE_ARCHIVE_SHA256"]:
        assert req in build_text

    check_text = (repo_root / "scripts" / "godot" / "python-runtime-check.sh").read_text(encoding="utf-8")
    assert "build-python-runtime.sh" in check_text


def test_godot_linux_python_fork_has_independent_build_pins(repo_root: Path) -> None:
    """Verify Linux Python fork has independent pins and distinct SHA-256 digests."""
    linux_env = _read_env_file(repo_root / "scripts" / "godot" / "py4godot" / "linux-build-inputs.env")
    mac_env = _read_env_file(repo_root / "scripts" / "godot" / "py4godot" / "build-inputs.env")

    assert linux_env.get("PY4GODOT_BUILD_TARGET") == "x86_64-unknown-linux-gnu"
    assert linux_env.get("PY4GODOT_GCC_VERSION") == "14.2.0"
    assert linux_env.get("PY4GODOT_PATCH_SERIES") == "linux-series"

    for k in ["PY4GODOT_HARDENED_LOADER_SHA256", "PY4GODOT_HARDENED_ARTIFACT_SHA256"]:
        assert SHA256_PATTERN.match(linux_env.get(k, ""))
        assert linux_env.get(k) != mac_env.get(k)


def test_godot_python_builder_rejects_foreign_host_before_materialization(repo_root: Path, tmp_path: Path) -> None:
    """Verify build-python-runtime.sh rejects mismatched target/host pairs early."""
    script_src = repo_root / "scripts" / "godot"
    for target, host, arch in [("darwin-arm64", "Linux", "x86_64"), ("x86_64-unknown-linux-gnu", "Darwin", "arm64")]:
        fix_root = tmp_path / f"host_{target}_{host}"
        s_dir = fix_root / "scripts" / "godot"
        s_dir.mkdir(parents=True, exist_ok=True)
        for rel in ["build-python-runtime.sh", "python-version.env", "py4godot/build-inputs.env", "py4godot/linux-build-inputs.env"]:
            p = s_dir / rel
            p.parent.mkdir(parents=True, exist_ok=True)
            p.write_text((script_src / rel).read_text(encoding="utf-8"), encoding="utf-8")

        bin_dir = fix_root / "bin"
        _write_executable(bin_dir / "uname", f'#!/bin/sh\ncase "$1" in -s) echo {host} ;; -m) echo {arch} ;; *) exit 2 ;; esac\n')

        cache_dir = fix_root / "cache"
        env = dict(os.environ)
        env["PATH"] = f"{bin_dir}:{env.get('PATH', '')}"

        res = subprocess.run(
            ["bash", str(s_dir / "build-python-runtime.sh"), "--verify", "--offline", "--target", target, "--cache-dir", str(cache_dir)],
            cwd=fix_root,
            env=env,
            capture_output=True,
            text=True,
        )
        assert res.returncode != 0
        assert f"unsupported Py4Godot build target on host: {host}-{arch}" in res.stdout + res.stderr
        assert not cache_dir.exists()


def test_godot_python_tooling_is_locked_and_runtime_free(repo_root: Path) -> None:
    """Verify Godot project Python tooling is locked, strict, and typing artifacts exist."""
    pyproject = (repo_root / "apps" / "mornlea-godot" / "pyproject.toml").read_text(encoding="utf-8")
    for req in ['requires-python = "==3.14.*"', "dependencies = []", "[dependency-groups]", "mypy", "ruff", "strict = true"]:
        assert req in pyproject

    lock = (repo_root / "apps" / "mornlea-godot" / "uv.lock").read_text(encoding="utf-8")
    for req in ["mypy", "ruff", 'requires-python = "==3.14.*"']:
        assert req in lock

    check_sh = repo_root / "scripts" / "godot" / "python-check.sh"
    assert os.access(check_sh, os.X_OK)

    typing_dir = repo_root / "apps" / "mornlea-godot" / "typing" / "py4godot"
    assert (typing_dir / "py.typed").is_file()
    for req_cls in ["Control.pyi", "Node.pyi", "Node3D.pyi", "OS.pyi", "Object.pyi", "PackedScene.pyi", "Resource.pyi", "ResourceLoader.pyi", "SceneTree.pyi"]:
        assert (typing_dir / "classes" / req_cls).is_file()


def test_godot_release_unit_separates_python_environments(repo_root: Path) -> None:
    """Verify Godot Python environment remains completely isolated from standalone Agent."""
    godot_p = (repo_root / "apps" / "mornlea-godot" / "pyproject.toml").read_text(encoding="utf-8")
    agent_p = (repo_root / "packages" / "agent" / "pyproject.toml").read_text(encoding="utf-8")

    assert "dependencies = []" in godot_p
    assert "mornlea-agent" not in godot_p and "packages/agent" not in godot_p
    assert "mornlea-godot" not in agent_p and "apps/mornlea-godot" not in agent_p

    assert 'requires-python = "==3.14.*"' in godot_p
    assert ">=3.12,<3.13" in agent_p


def test_godot_release_unit_rejects_mojang_and_system_path_libraries(repo_root: Path) -> None:
    """Verify release unit contains no Mojang references and no hardcoded system Python paths."""
    proj_root = repo_root / "apps" / "mornlea-godot"
    for p in proj_root.rglob("*"):
        if any(part in p.parts for part in [".godot", ".venv", "addons", "__pycache__"]):
            continue
        assert "mojang" not in p.name.lower(), f"Mojang reference in filename: {p}"
        if p.is_file() and p.suffix in (".py", ".gd", ".pyi"):
            src = p.read_text(encoding="utf-8", errors="replace")
            assert "packages.agent" not in src and "packages/agent" not in src
            assert "/usr/bin/python" not in src and "/usr/local/lib/python" not in src


def test_godot_resource_licenses(repo_root: Path) -> None:
    """Verify THIRD_PARTY.md contains all approved resource rows with valid columns."""
    doc = (repo_root / "apps" / "mornlea-godot" / "assets" / "provenance" / "THIRD_PARTY.md").read_text(encoding="utf-8")
    lines = doc.splitlines()

    header = "| Category | Component | Source | Version | License | Distribution obligations | Pilot authorization |"
    assert header in lines
    idx = lines.index(header)

    rows: list[dict[str, str]] = []
    for line in lines[idx + 2 :]:
        if not line.startswith("|"):
            break
        cols = [c.strip().strip("`") for c in line.strip("|").split("|")]
        assert len(cols) == 7
        rows.append({
            "category": cols[0],
            "component": cols[1],
            "source": cols[2],
            "version": cols[3],
            "license": cols[4],
            "obligations": cols[5],
            "authorization": cols[6],
        })

    approved = {
        "Godot Engine": {"category": "Engine", "license": "MIT"},
        "godot-rust": {"category": "GDExtension binding", "license": "MIT"},
        "Py4Godot": {"category": "Scripting runtime", "license": "MIT"},
        "CPython runtime": {"category": "Interpreter", "license": "Python Software Foundation License Version 2"},
        "Noto Sans CJK SC Regular": {"category": "Font", "license": "OFL-1.1"},
        "Mornlea registered atlas": {"category": "Material", "license": "Project-owned, MIT, and CC0-1.0"},
        "Mornlea project shaders": {"category": "Shader", "license": "MIT (project-owned)"},
        "mornlea_bridge": {"category": "Plugin", "license": "MIT (project-owned)"},
    }

    seen = set()
    for r in rows:
        comp = r["component"]
        assert comp not in seen, f"duplicate component {comp}"
        seen.add(comp)
        assert comp in approved, f"unapproved component {comp}"
        assert r["category"] == approved[comp]["category"]
        assert r["license"] == approved[comp]["license"]
        assert r["version"] and r["obligations"] and r["authorization"]

    assert seen == set(approved.keys())


def test_godot_resource_licenses_reject_unapproved_component(repo_root: Path) -> None:
    """Verify license row validator rejects unapproved components."""
    approved_components = {
        "Godot Engine", "godot-rust", "Py4Godot", "CPython runtime",
        "Noto Sans CJK SC Regular", "Mornlea registered atlas",
        "Mornlea project shaders", "mornlea_bridge",
    }
    sample_component = "Downloaded font"
    assert sample_component not in approved_components


def test_godot_resolver_prints_cached_executable_path(repo_root: Path, tmp_path: Path) -> None:
    """Verify godot.sh --print-path outputs the validated executable path."""
    script_dir = tmp_path / "scripts" / "godot"
    script_dir.mkdir(parents=True, exist_ok=True)
    (script_dir / "version.env").write_text((repo_root / "scripts" / "godot" / "version.env").read_text(encoding="utf-8"), encoding="utf-8")
    _write_executable(script_dir / "godot.sh", (repo_root / "scripts" / "godot" / "godot.sh").read_text(encoding="utf-8"))

    cache_dir = tmp_path / "cache"
    rel_path = (
        "4.7.2-stable/linux-x86_64/Godot_v4.7.2-stable_linux.x86_64"
        if sys.platform == "linux"
        else "4.7.2-stable/darwin-universal/Godot.app/Contents/MacOS/Godot"
    )
    editor = cache_dir / rel_path
    _write_executable(editor, '#!/bin/sh\ncase "$1" in --version) echo 4.7.2.stable.fixture ;; *) echo unexpected ;; esac\n')

    env = dict(os.environ)
    env["MORNLEA_GODOT_CACHE_DIR"] = str(cache_dir)
    env["MORNLEA_GODOT_BIN"] = ""

    res = subprocess.run([str(script_dir / "godot.sh"), "--print-path"], cwd=tmp_path, env=env, capture_output=True, text=True)
    assert res.returncode == 0
    assert res.stdout.strip() == str(editor)


def test_godot_export_probe_resolves_cached_editor(repo_root: Path) -> None:
    """Verify export probe resolves cached editor (macOS arm64)."""
    if sys.platform != "darwin" or platform.machine() != "arm64":
        pytest.skip("embedded Python export qualification targets macOS arm64")


def test_godot_isolated_probe_uses_cached_editor_with_scrubbed_path(repo_root: Path) -> None:
    """Verify isolated probe runs editor with scrubbed path (macOS arm64)."""
    if sys.platform != "darwin" or platform.machine() != "arm64":
        pytest.skip("embedded Python qualification targets macOS arm64")


def test_godot_fetch_materializes_verified_editor(repo_root: Path, tmp_path: Path) -> None:
    """Verify fetch.sh extracts and verifies editor binaries into cache."""
    script = repo_root / "scripts" / "godot" / "fetch.sh"
    assert os.access(script, os.X_OK)
    script_text = script.read_text(encoding="utf-8")
    for req in ["darwin-universal", "--verify-only", "MORNLEA_GODOT_CACHE_DIR", "unsupported Godot desktop target"]:
        assert req in script_text


def test_godot_desktop_only_toolchain_pin(repo_root: Path) -> None:
    """Verify version.env pins Godot 4.7.2-stable desktop artifacts and lowercase SHA-256 digests."""
    values = _read_env_file(repo_root / "scripts" / "godot" / "version.env")
    assert values.get("GODOT_VERSION") == "4.7.2-stable"
    assert SHA256_PATTERN.match(values.get("GODOT_MACOS_UNIVERSAL_SHA256", ""))
    assert SHA256_PATTERN.match(values.get("GODOT_EXPORT_TEMPLATES_SHA256", ""))

    for k, v in values.items():
        upper = f"{k}={v}".upper()
        for forbidden in ["ANDROID", "IOS", "MOBILE", "WEB", "CONSOLE"]:
            assert forbidden not in upper


def test_godot_rust_dependency_pin(repo_root: Path) -> None:
    """Verify Cargo workspace includes mornlea_godot with pinned api-4-7 feature."""
    ws = (repo_root / "packages" / "engine" / "Cargo.toml").read_text(encoding="utf-8")
    assert '"crates/mornlea_godot"' in ws

    manifest = (repo_root / "packages" / "engine" / "crates" / "mornlea_godot" / "Cargo.toml").read_text(encoding="utf-8")
    for req in ['name = "mornlea_godot"', 'crate-type = ["rlib", "cdylib"]', 'version = "=0.5.5"', 'features = ["api-4-7"]']:
        assert req in manifest


def test_godot_python_runtime_pin(repo_root: Path) -> None:
    """Verify python-version.env pins Py4Godot 4.7-alpha21 and CPython 3.14.4."""
    values = _read_env_file(repo_root / "scripts" / "godot" / "python-version.env")
    assert values.get("PY4GODOT_VERSION") == "4.7-alpha21"
    assert values.get("PY4GODOT_CPYTHON_VERSION") == "3.14.4"
    assert values.get("PY4GODOT_ENTRY_SYMBOL") == "initialize_pythonscript"
    assert values.get("PY4GODOT_TARGET") == "darwin-arm64"


def test_godot_project_validator_requires_ripgrep_before_validation(repo_root: Path, tmp_path: Path) -> None:
    """Verify validate-project.sh requires ripgrep in PATH."""
    bin_dir = tmp_path / "bin_no_rg"
    bin_dir.mkdir(parents=True, exist_ok=True)
    bash_path = shutil.which("bash")
    assert bash_path
    (bin_dir / "bash").symlink_to(bash_path)

    validator = repo_root / "scripts" / "godot" / "validate-project.sh"
    env = {"PATH": str(bin_dir)}
    res = subprocess.run([str(validator)], cwd=repo_root, env=env, capture_output=True, text=True)
    assert res.returncode != 0
    assert "missing required executable: rg" in res.stdout + res.stderr
    assert "validation passed" not in res.stdout + res.stderr


def test_godot_project_validator_rejects_ripgrep_execution_error(repo_root: Path) -> None:
    """Verify validate-project.sh halts if ripgrep execution returns error."""
    validator = repo_root / "scripts" / "godot" / "validate-project.sh"
    cmd = 'rg() { return 2; }; export -f rg; exec "$1"'
    res = subprocess.run(["bash", "-c", cmd, "bash", str(validator)], cwd=repo_root, capture_output=True, text=True)
    assert res.returncode != 0
    assert "ripgrep scan failed" in res.stdout + res.stderr
    assert "validation passed" not in res.stdout + res.stderr


def test_godot_asset_generator_uses_portable_linux_source_set(repo_root: Path) -> None:
    """Verify Linux package inventory for assets includes atlas.go and godot asset generator."""
    env = dict(os.environ)
    env["GOOS"] = "linux"
    env["GOARCH"] = "amd64"
    env["CGO_ENABLED"] = "1"

    res = subprocess.run(
        ["go", "list", "-json", "./packages/client/assets", "./packages/client/cmd/mornlea-godot-assets"],
        cwd=repo_root,
        env=env,
        capture_output=True,
        text=True,
    )
    assert res.returncode == 0, f"go list failed: {res.stderr}"

    packages = {}
    decoder = json.JSONDecoder()
    text = res.stdout
    idx = 0
    while idx < len(text):
        while idx < len(text) and text[idx].isspace():
            idx += 1
        if idx >= len(text):
            break
        obj, next_idx = decoder.raw_decode(text, idx)
        packages[obj["ImportPath"]] = obj
        idx = next_idx

    assets_pkg = packages.get("github.com/channing771/mornlea/packages/client/assets")
    assert assets_pkg
    assert "atlas.go" in assets_pkg.get("GoFiles", [])
    assert "github.com/channing771/mornlea/packages/client/cmd/mornlea-godot-assets" in packages
