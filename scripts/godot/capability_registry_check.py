#!/usr/bin/env python3
"""Validate the disabled coarse-capability registry without loading Godot."""

from __future__ import annotations

import re
import sys
from pathlib import Path


EXPECTED = {
    "menus": ("3@1.0", "6@1.0", "7@1.0"),
    "containers": ("3@1.0", "5@1.0", "6@1.0", "7@1.0"),
    "chat": ("2@1.0", "3@1.0", "6@1.0", "7@1.0"),
    "audio": ("6@1.0", "8@1.0"),
    "lod": ("5@1.0",),
    "viewmodel": ("3@1.0", "6@1.0"),
    "companions": ("6@1.0",),
    "hostile_mobs": ("6@1.0",),
    "passive_cows": ("6@1.0",),
    "projectiles": ("6@1.0",),
    "drops": ("6@1.0",),
    "name_tags": ("6@1.0",),
    "particles": ("6@1.0", "8@1.0"),
    "full_weather": ("8@1.0",),
}
EXPECTED_BUDGETS = {
    "audio": "light",
    "name_tags": "light",
    "lod": "heavy",
    "full_weather": "heavy",
}
ALLOWED_FAMILIES = frozenset(range(1, 9))
# The frozen rust-client-core descriptor table, mirroring the Rust
# `RUST_PRODUCER_FAMILIES` table in
# packages/engine/crates/mornlea_godot/src/feature_negotiation.rs: ten logical
# families in contract order with numeric IDs 1..10, every family at major
# 1 / minor 0. The per-family record limit mirrors the accepted client-core
# `ClientLimits::MAX_FAMILY_RECORDS` (4096); records are variable-length
# semantic values, so the fixed per-record size is 0. Registry tokens encode
# each descriptor as name@major.minor:numeric_id:record_limit:record_bytes.
RUST_PRODUCER = "rust-client-core"
RUST_PRODUCER_FAMILIES = (
    ("session", 1),
    ("input", 2),
    ("terrain", 3),
    ("actors", 4),
    ("player-view", 5),
    ("inventory-ui", 6),
    ("world-ui", 7),
    ("audio-cues", 8),
    ("lifecycle", 9),
    ("diagnostics", 10),
)
RUST_PRODUCER_FAMILY_RECORD_LIMIT = 4096
RUST_PRODUCER_FAMILY_RECORD_BYTES = 0
RUST_PRODUCER_TOKEN = re.compile(r"([a-z-]+)@([0-9]+)\.([0-9]+):([0-9]+):([0-9]+):([0-9]+)")
METADATA = re.compile(r"^metadata/([a-z_]+)\s*=\s*(.*)$", re.MULTILINE)
QUOTED = re.compile(r'"([^"\\]*(?:\\.[^"\\]*)*)"')


def metadata(text: str) -> dict[str, str]:
    return {name: value.strip() for name, value in METADATA.findall(text)}


def strings(value: str) -> tuple[str, ...]:
    if not value.startswith("PackedStringArray(") or not value.endswith(")"):
        raise ValueError(f"expected PackedStringArray, got {value}")
    return tuple(match.group(1) for match in QUOTED.finditer(value))


def quoted(value: str) -> str:
    matches = QUOTED.findall(value)
    if len(matches) != 1:
        raise ValueError(f"expected one quoted string, got {value}")
    return matches[0]


def scalar(fields: dict[str, str], name: str, expected: str) -> None:
    if fields.get(name) != expected:
        raise ValueError(f"{name} = {fields.get(name)!r}, want {expected!r}")


def validate_manifest(root: Path, name: str, path: Path, expected_families: tuple[str, ...]) -> None:
    if not path.is_file():
        raise ValueError(f"{name}: missing manifest {path}")
    feature_dir = path.parent
    files = sorted(item.name for item in feature_dir.iterdir() if item.is_file())
    if files != ["feature.tres"]:
        raise ValueError(f"{name}: capability directory contains implementation files {files}")
    fields = metadata(path.read_text(encoding="utf-8"))
    scalar(fields, "feature_id", f'"{name}"')
    scalar(fields, "host_protocol_major", "1")
    scalar(fields, "host_protocol_minor", "0")
    scalar(fields, "contract_version", '"1.0"')
    scalar(fields, "enabled", "false")
    scalar(fields, "implementation_status", '"reserved"')
    scalar(fields, "entry_scene_path", '""')
    scalar(fields, "required", "false")
    scalar(fields, "budget_class", f'"{EXPECTED_BUDGETS.get(name, "standard")}"')
    scalar(fields, "reset_policy", '"reset"')
    if strings(fields.get("dependencies", "")):
        raise ValueError(f"{name}: reserved capability has feature dependencies")
    actual_families = strings(fields.get("required_bridge_families", ""))
    if actual_families != expected_families:
        raise ValueError(f"{name}: family dependencies {actual_families}, want {expected_families}")
    for family in actual_families:
        match = re.fullmatch(r"([0-9]+)@1\.0", family)
        if match is None or int(match.group(1)) not in ALLOWED_FAMILIES:
            raise ValueError(f"{name}: family dependency is not a known numeric v1 family: {family}")


def validate_rust_producer(fields: dict[str, str]) -> None:
    """Validate the frozen rust-client-core producer table and its symbolic
    resolution: exactly ten descriptors in contract order, unique numeric
    IDs, and the registered family contract versions and record limits."""
    if quoted(fields.get("rust_producer", "")) != RUST_PRODUCER:
        raise ValueError(f"rust producer {fields.get('rust_producer')!r}, want {RUST_PRODUCER!r}")
    tokens = strings(fields.get("rust_producer_families", ""))
    if len(tokens) != len(RUST_PRODUCER_FAMILIES):
        raise ValueError(
            f"rust producer table has {len(tokens)} descriptors, want {len(RUST_PRODUCER_FAMILIES)}"
        )
    resolved: dict[str, int] = {}
    for index, token in enumerate(tokens):
        match = RUST_PRODUCER_TOKEN.fullmatch(token)
        if match is None:
            raise ValueError(f"rust producer descriptor {index} is not a family token: {token}")
        name, major, minor, numeric_id, record_limit, record_bytes = match.groups()
        expected_name, expected_id = RUST_PRODUCER_FAMILIES[index]
        if name != expected_name or int(numeric_id) != expected_id:
            raise ValueError(
                f"rust producer descriptor {index} is {name}:{numeric_id}, "
                f"want {expected_name}:{expected_id}"
            )
        if (int(major), int(minor)) != (1, 0):
            raise ValueError(f"{name}: family contract version {major}.{minor}, want 1.0")
        if int(record_limit) != RUST_PRODUCER_FAMILY_RECORD_LIMIT:
            raise ValueError(
                f"{name}: record limit {record_limit}, want {RUST_PRODUCER_FAMILY_RECORD_LIMIT}"
            )
        if int(record_bytes) != RUST_PRODUCER_FAMILY_RECORD_BYTES:
            raise ValueError(
                f"{name}: record bytes {record_bytes}, want {RUST_PRODUCER_FAMILY_RECORD_BYTES}"
            )
        resolved[name] = int(numeric_id)
    if len(set(resolved.values())) != len(resolved):
        raise ValueError("rust producer table contains duplicate numeric IDs")
    # Features resolve symbolic logical names to descriptors before any
    # feature is instantiated; these two names must resolve through the table.
    for name in ("audio-cues", "lifecycle"):
        if name not in resolved:
            raise ValueError(f"rust producer table does not resolve symbolic family {name}")


def validate(root: Path) -> None:
    registry_path = root / "catalog/capability_registry.tres"
    fields = metadata(registry_path.read_text(encoding="utf-8"))
    scalar(fields, "registry_version", "1")
    manifest_paths = strings(fields.get("manifest_paths", ""))
    expected_paths = tuple(f"res://features/{name}/feature.tres" for name in EXPECTED)
    if manifest_paths != expected_paths:
        raise ValueError(f"registry manifest paths {manifest_paths}, want {expected_paths}")
    if len(set(manifest_paths)) != len(manifest_paths):
        raise ValueError("registry contains duplicate manifest paths")
    for name, resource_path in zip(EXPECTED, manifest_paths):
        if not resource_path.startswith("res://features/"):
            raise ValueError(f"{name}: manifest is outside the feature root")
        relative = resource_path.removeprefix("res://")
        validate_manifest(root, name, root / relative, EXPECTED[name])
    validate_rust_producer(fields)


def main() -> int:
    root = (
        Path(__file__).resolve().parents[2] / "apps/mornlea-godot"
        if len(sys.argv) == 1
        else Path(sys.argv[1]).resolve()
    )
    try:
        validate(root)
    except (OSError, ValueError) as error:
        print(f"Godot capability registry check failed: {error}", file=sys.stderr)
        return 1
    print(
        f"Godot capability registry check passed "
        f"({len(EXPECTED)} disabled coarse capabilities, "
        f"{len(RUST_PRODUCER_FAMILIES)} rust producer families)."
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
