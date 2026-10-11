"""Retain and release actual owned native frames as a presentation consumer."""

from __future__ import annotations

import json
from typing import Any

from py4godot.classes import gdclass
from py4godot.classes.core import Array, Dictionary
from py4godot.classes.Node import Node


def _plain(value: Any) -> Any:
    if isinstance(value, Dictionary):
        keys = value.keys()
        return {str(keys.get(i)): _plain(value.get(keys.get(i))) for i in range(keys.size())}
    if isinstance(value, Array):
        return [_plain(value.get(i)) for i in range(value.size())]
    return value


@gdclass
class rust_runtime_probe(Node):
    def _ready(self) -> None:
        self._frame: Any = None
        self._count = 0

    def validate_feature(self, _feature_id: str) -> str:
        return ""

    def bind_host(self, services_path: str) -> str:
        bridge = self.get_node(services_path)
        return "" if bridge is not None and bridge.has_method("open_core") else "missing core"

    def activate_feature(self, _epoch: int) -> str:
        return ""

    def reset_feature(self, _epoch: int) -> None:
        self._frame = None

    def apply_typed_frame(self, frame: object) -> None:
        if not isinstance(frame, Dictionary):
            raise ValueError("the consumer did not receive an owned native Dictionary")
        self._frame = frame
        self._count += 1

    def frame_json(self) -> str:
        return json.dumps(_plain(self._frame), sort_keys=True, separators=(",", ":"))

    def applied_count(self) -> int:
        return self._count

    def deactivate_feature(self) -> None:
        self._frame = None
        print("[mornlea-rust-session] python-release=consumer")
