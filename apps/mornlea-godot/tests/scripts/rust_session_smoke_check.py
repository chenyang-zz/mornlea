"""Exercise an actual native Rust session and Python fanout against the F2 fixture."""

from __future__ import annotations

import json
import time
from pathlib import Path
from typing import Any

from py4godot.classes import gdclass
from py4godot.classes.core import Array, Dictionary
from py4godot.classes.Node import Node
from py4godot.classes.OS import OS
from py4godot.utils.smart_cast import register_cast_function  # type: ignore[import-not-found]

register_cast_function("MornleaClientBridge", lambda bridge: bridge)
register_cast_function("MornleaBoundaryProbe", lambda bridge: bridge)

LIMITS = {
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
}
FAMILIES = (
    "session",
    "input",
    "terrain",
    "actors",
    "player-view",
    "inventory-ui",
    "world-ui",
    "audio-cues",
    "lifecycle",
    "diagnostics",
)


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
class rust_session_smoke_check(Node):
    def _ready(self) -> None:
        print("[mornlea-lifecycle] python-init=host")
        self._finished = False
        self._phase = "first"
        self._deadline = time.monotonic() + 35
        self._seen: set[str] = set()
        self._admitted = False
        self._submitted = False
        self._prepared = False
        self._retained: Any = None
        self._retained_json = ""
        self._output: Path | None = None
        try:
            args = OS.instance().get_cmdline_user_args()
            options = {}
            for index in range(args.size()):
                text = args.get(index)
                if text.startswith("--mornlea-rust-"):
                    key, value = text.split("=", 1)
                    assert key not in options, key
                    options[key] = value
            address = options["--mornlea-rust-server"]
            host, port_text = address.rsplit(":", 1)
            assert host == "127.0.0.1" and port_text.isdecimal(), address
            port = int(port_text)
            assert 1 <= port <= 65535, port
            self._source = options["--mornlea-rust-source"]
            self._contract = options["--mornlea-rust-contract"]
            self._output = Path(options["--mornlea-rust-report"])
            self._bridge = self.get_node("ClientBridge")
            self._host = self.get_node("FeatureHost")
            self._probe = self.get_node("BoundaryProbe")
            config = {
                "limits": LIMITS,
                "hello_ms": 5000,
                "login_ms": 10000,
                "connector_capability": "1",
            }
            endpoint = {"tag": "Tcp", "value": {"host": host, "port": port}}
            identity = {
                "login": {
                    "player_id": "01000000000040008000000000000000",
                    "display_name": "NativeSession",
                    "view_distance": 2,
                }
            }
            raw = self._host.call(
                "activate_rust_catalog",
                "res://tests/fixtures/catalogs/rust_runtime_probe.tres",
                "../ClientBridge",
                1,
                json.dumps(config),
                json.dumps(endpoint),
                json.dumps(identity),
            )
            assert isinstance(raw, str), raw
            activated = json.loads(raw)
            assert activated["ok"] and self._host.call("active_count") == 1, activated
            self._consumer = self._host.get_node("RustRuntimeProbe")
            self._token = _success(self._probe.call("current_token"))
            table = _success(self._bridge.call("family_table", _native(self._token)))
            assert table["producer"] == "rust-client-core", table
            descriptors = table["descriptors"]
            assert [(entry["logical_name"], entry["numeric_id"]) for entry in descriptors] == [
                (name, index + 1) for index, name in enumerate(FAMILIES)
            ], descriptors
            self._epoch = self._host.call("rust_session_epoch")
            assert isinstance(self._epoch, str) and int(self._epoch) > 0, self._epoch
            # Connect publishes Open before the host's first semantic tick replaces it.
            initial = _success(
                self._bridge.call("pull_typed_frame", _native(self._token), self._epoch)
            )
            assert isinstance(initial, dict), initial
            self._observe(initial)
        except Exception as failure:
            self._fail(failure)

    def _process(self, _delta: float) -> None:
        if self._finished:
            return
        try:
            assert time.monotonic() < self._deadline, (
                self._phase,
                sorted(self._seen),
                self._admitted,
                self._prepared,
            )
            raw = self._consumer.call("frame_json")
            assert isinstance(raw, str), raw
            frame = json.loads(raw)
            if frame is None:
                return
            self._observe(frame)
            if not self._admitted:
                return
            if self._phase == "first":
                if not self._submitted:
                    self._submit()
                    self._submitted = True
                    return
                if self._prepared and self._seen == set(FAMILIES):
                    self._reset()
            elif self._phase == "reset" and self._prepared:
                self._finish()
        except Exception as failure:
            self._fail(failure)

    def _exit_tree(self) -> None:
        if self._finished:
            print("[mornlea-lifecycle] python-deinit=features")
            print("[mornlea-lifecycle] python-deinit=bridge")

    def _observe(self, frame: dict[str, Any]) -> None:
        assert frame["session_epoch"] == self._epoch, frame["session_epoch"]
        assert [family["key"]["logical_name"] for family in frame["families"]] == list(FAMILIES)
        for family in frame["families"]:
            name = family["key"]["logical_name"]
            records = family["records"]
            if records:
                self._seen.add(name)
            for record in records:
                assert record["header"]["epoch"] == self._epoch, record
                if name == "session":
                    assert record["terminal"] is None, record
                    self._admitted |= record["phase"] == "Admitted"
                elif name == "terrain":
                    self._prepared |= record["resource"] is not None
                elif name == "diagnostics":
                    assert record["producer"] == {
                        "source_sha": self._source,
                        "contract_sha": self._contract,
                    }, record["producer"]

    def _submit(self) -> None:
        actions = [
            {
                "intent": {
                    "tag": "PlayerInput",
                    "value": {
                        "movement": {"move_x": 1, "move_z": 0, "jump": False},
                        "look": {"yaw": 0.0, "pitch": -0.5},
                        "actions": {
                            "primary": False,
                            "eating": False,
                            "sprinting": False,
                            "sneaking": False,
                        },
                    },
                },
                "container": None,
                "crafting": None,
            },
            {
                "intent": {"tag": "SelectHotbar", "value": {"slot": 2}},
                "container": None,
                "crafting": None,
            },
            {
                "intent": {"tag": "Chat", "value": {"text": "native lifecycle"}},
                "container": None,
                "crafting": None,
            },
        ]
        receipt = _success(
            self._bridge.call(
                "submit_typed_input",
                _native(self._token),
                self._epoch,
                _native({"epoch": self._epoch, "actions": actions}),
            )
        )
        assert receipt["tag"] == "Queued" and receipt["value"]["sequenced_count"] == 2, receipt

    def _reset(self) -> None:
        # A retained native copy must outlive subsequent producer publications and close.
        self._retained = self._bridge.call("pull_typed_frame", _native(self._token), self._epoch)
        self._retained_json = json.dumps(_success(self._retained), sort_keys=True)
        old = self._epoch
        assert isinstance(old, str), old
        self._host.call("reset_features", 2)
        self._epoch = self._host.call("rust_session_epoch")
        assert isinstance(self._epoch, str) and int(self._epoch) > int(old), self._epoch
        stale = _plain(
            self._bridge.call(
                "step", _native(self._token), old, _native({"messages": 64, "meshes": 32})
            )
        )
        assert stale["ok"] is False and stale["error"]["class"] == "StaleEpoch", stale
        assert json.dumps(_success(self._retained), sort_keys=True) == self._retained_json
        self._phase = "reset"
        self._admitted = False
        self._prepared = False

    def _finish(self) -> None:
        self._host.call("deactivate_features")
        assert self._host.call("active_count") == 0
        assert self._consumer.call("frame_json") == "null"
        assert self._probe.call("releases") == 1
        assert _success(self._bridge.call("close", _native(self._token))) is None
        assert self._probe.call("releases") == 1
        stale = _plain(self._bridge.call("family_table", _native(self._token)))
        assert stale["ok"] is False and stale["error"]["class"] == "InvalidState", stale
        assert json.dumps(_success(self._retained), sort_keys=True) == self._retained_json
        print("[mornlea-rust-session] native-release=core")
        result = {
            "ok": True,
            "source_sha": self._source,
            "contract_sha": self._contract,
            "families": list(range(1, 11)),
            "resets": 1,
            "reconnects": 1,
            "inputs": 1,
            "preparation": True,
            "release_order": True,
            "retained_frame": True,
        }
        assert self._output is not None
        self._output.write_text(json.dumps(result, sort_keys=True) + "\n", encoding="utf-8")
        self._finished = True
        print("[mornlea-rust-session] session=complete")
        self.get_tree().quit(0)

    def _fail(self, failure: Exception) -> None:
        self._finished = True
        print(f"Rust session smoke failed: {failure!r}")
        if self._output is not None:
            self._output.write_text(
                json.dumps({"ok": False, "error": repr(failure)}) + "\n", encoding="utf-8"
            )
        self.get_tree().quit(1)
