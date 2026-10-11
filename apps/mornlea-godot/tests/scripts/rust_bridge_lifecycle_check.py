"""Check actual native marshalling and release in an embedded Python process."""

from __future__ import annotations

import json
from typing import Any

from py4godot.classes import gdclass
from py4godot.classes.core import Array, Dictionary
from py4godot.classes.Node import Node
from py4godot.utils.smart_cast import register_cast_function  # type: ignore[import-not-found]

register_cast_function("MornleaClientBridge", lambda bridge: bridge)
register_cast_function("MornleaBoundaryProbe", lambda bridge: bridge)

CONFIG: dict[str, Any] = {
    "limits": {
        "queued_input_events": "128",
        "inbound_observations": "8192",
        "inbound_bytes": "8388608",
        "outbound_commands": "4104",
        "outbound_bytes": "8388608",
        "prediction_journal": "256",
        "message_work": "4096",
        "mesh_work": "4096",
        "preparation_results": "4096",
        "preparation_bytes": "67108864",
        "family_records": "4096",
        "frame_bytes": "8388608",
    },
    "hello_ms": 5000,
    "login_ms": 10000,
    "connector_capability": "1",
}


def _native(value: Any) -> Any:
    if isinstance(value, dict):
        result = Dictionary.new0()
        for key, item in value.items():
            result.set(key, _native(item))
        return result
    if isinstance(value, list):
        array = Array.new0()
        for item in value:
            array.append(_native(item))
        return array
    return value


def _plain(value: Any) -> Any:
    if isinstance(value, Dictionary):
        keys = value.keys()
        return {str(keys.get(i)): _plain(value.get(keys.get(i))) for i in range(keys.size())}
    if isinstance(value, Array):
        return [_plain(value.get(i)) for i in range(value.size())]
    return value


def _success(value: Any) -> Any:
    result = _plain(value)
    assert isinstance(result, dict) and set(result) == {"ok", "value", "error"}, result
    assert result["ok"] is True and result["error"] is None, result
    return result["value"]


@gdclass
class rust_bridge_lifecycle_check(Node):
    def _ready(self) -> None:
        try:
            self._exercise_native()
        except Exception as failure:
            print(f"Rust native lifecycle check failed: {failure!r}")
            self.get_tree().quit(1)
            return
        print("Rust native lifecycle check passed.")
        self.get_tree().quit(0)

    def _exercise_native(self) -> None:
        bridge = self.get_node("ClientBridge")
        probe = self.get_node("BoundaryProbe")
        token = _success(bridge.call("open_core", _native(CONFIG)))
        native_token = _native(token)
        assert isinstance(token["generation"], str) and token["generation"] == "1"
        table = _success(bridge.call("family_table", native_token))
        assert table["producer"] == "rust-client-core" and len(table["descriptors"]) == 10
        assert {entry["numeric_id"] for entry in table["descriptors"]} == set(range(1, 11))
        malformed = {**token, "generation": "01"}
        refused = _plain(bridge.call("family_table", _native(malformed)))
        assert refused["ok"] is False and refused["error"]["class"] == "InvalidInput"
        panic = _plain(probe.call("panic_result"))
        assert panic["ok"] is False and panic["error"]["class"] == "Internal", panic
        assert _success(bridge.call("close", native_token)) is None
        assert _success(bridge.call("close", native_token)) is None
        assert probe.call("releases") == 1
        replacement = _success(bridge.call("open_core", _native(CONFIG)))
        assert replacement["slot"] == token["slot"]
        assert int(replacement["generation"]) > int(token["generation"])
        stale = _plain(bridge.call("family_table", native_token))
        assert stale["ok"] is False and stale["error"]["class"] == "InvalidState"
        assert table["producer"] == "rust-client-core"
        assert _success(bridge.call("close", _native(replacement))) is None

        host = self.get_node("FeatureHost")
        endpoint = {"tag": "Tcp", "value": {"host": "127.0.0.1", "port": 1}}
        identity = {
            "login": {
                "player_id": "01000000000040008000000000000000",
                "display_name": "NativeProbe",
                "view_distance": 8,
            }
        }
        raw = host.call(
            "activate_rust_catalog",
            "res://tests/fixtures/catalogs/rust_runtime_probe.tres",
            "../ClientBridge",
            1,
            json.dumps(CONFIG),
            json.dumps(endpoint),
            json.dumps(identity),
        )
        assert isinstance(raw, str), raw
        result = json.loads(raw)
        assert result["ok"], result
        assert host.call("active_count") == 1
        host.call("deactivate_features")
        assert host.call("active_count") == 0
        assert probe.call("releases") == 3
        host.call("deactivate_features")
        assert probe.call("releases") == 3
        print("[mornlea-rust-session] native-release=core")
