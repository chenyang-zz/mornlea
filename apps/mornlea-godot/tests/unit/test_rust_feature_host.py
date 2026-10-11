"""Engine-free unit harness for the host's rust-core session ownership.

The harness imports the actual production ``feature_host`` module and runs
its real activation, dispatch, and teardown logic. Only the py4godot module
surface (``gdclass``, the ``Node``/``Object``/resource shells), fake feature
resources, and one scripted rust-facade boundary are substituted: no host
dispatch or rollback behavior is reimplemented here. Nothing imports the
Godot engine or launches a window.
"""

from __future__ import annotations

import copy
import importlib.util
import json
import sys
import types
import unittest
from collections.abc import Callable
from pathlib import Path
from typing import Any, cast
from unittest.mock import patch

PROJECT_ROOT = Path(__file__).resolve().parents[2]
HOST_MODULE_PATH = PROJECT_ROOT / "app" / "host" / "feature_host.py"

# The scripted producer family table mirrors the frozen rust-client-core
# descriptors: ten logical families, numeric IDs 1..10, every version 1.0.
_FRAME_BYTES = 8 << 20
PRODUCER_FAMILIES = (
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
PILOT_FAMILY_JSON = json.dumps(
    [
        {"family": family, "record_bytes": 0, "record_limit": 64, "version": 1}
        for family in range(1, 9)
    ]
)

CAST_REGISTRATIONS: dict[str, Any] = {}
RESOURCE_REGISTRY: dict[str, Any] = {}


class NativeArray:
    """Use the installed Array API without accepting Python containers."""

    def __init__(self) -> None:
        self.values: list[object] = []

    @staticmethod
    def new0() -> NativeArray:
        return NativeArray()

    def size(self) -> int:
        return len(self.values)

    def get(self, index: int) -> object:
        return self.values[index]

    def append(self, value: object) -> None:
        assert_native_argument(value)
        self.values.append(value)


class NativeDictionary:
    """Use the installed Dictionary API, including native keys arrays."""

    def __init__(self) -> None:
        self.values: dict[object, object] = {}

    @staticmethod
    def new0() -> NativeDictionary:
        return NativeDictionary()

    def size(self) -> int:
        return len(self.values)

    def keys(self) -> NativeArray:
        result = NativeArray.new0()
        for key in self.values:
            result.append(key)
        return result

    def get(self, key: object, default: object = None) -> object:
        return self.values.get(key, default)

    def set(self, key: object, value: object) -> bool:
        assert_native_argument(key)
        assert_native_argument(value)
        self.values[key] = value
        return True


def assert_native_argument(value: object) -> None:
    if isinstance(value, (dict, list)):
        raise TypeError("Object.call cannot marshal a built-in container")


def native_value(value: object) -> object:
    if isinstance(value, dict):
        dictionary = NativeDictionary.new0()
        for key, entry in value.items():
            dictionary.set(key, native_value(entry))
        return dictionary
    if isinstance(value, list):
        array = NativeArray.new0()
        for entry in value:
            array.append(native_value(entry))
        return array
    return value


def plain_value(value: object) -> Any:
    if isinstance(value, NativeDictionary):
        return {key: plain_value(entry) for key, entry in value.values.items()}
    if isinstance(value, NativeArray):
        return [plain_value(entry) for entry in value.values]
    return value


class StubNodePath:
    """The NodePath surface `_absolute_path_text` consumes."""

    def __init__(self, names: str, absolute: bool) -> None:
        self._names = names
        self._absolute = absolute

    def get_concatenated_names(self) -> str:
        return self._names

    def is_absolute(self) -> bool:
        return self._absolute


class StubNode:
    """The Node shell the production host logic actually touches."""

    def __init__(self, name: str, parent: StubNode | None = None) -> None:
        self.name = name
        self.parent = parent
        self.children: dict[str, StubNode] = {}
        self.freed = False

    def add_child(self, child: StubNode) -> None:
        child.parent = self
        self.children[child.name] = child

    def get_path(self) -> StubNodePath:
        names: list[str] = []
        node: StubNode | None = self
        while node is not None:
            names.append(node.name)
            node = node.parent
        return StubNodePath("/".join(reversed(names)), True)

    def get_node(self, path: str) -> StubNode | None:
        if path.startswith("/"):
            node: StubNode | None = self
            while node is not None and node.name != path.strip("/").split("/")[0]:
                node = node.parent
            if node is None:
                return None
            remaining = path.strip("/").split("/")[1:]
        else:
            node = self
            remaining = path.split("/")
        for step in remaining:
            if step == ".":
                continue
            if step == "..":
                node = node.parent if node is not None else None
                continue
            node = node.children.get(step) if node is not None else None
            if node is None:
                return None
        return node

    def get_node_or_null(self, path: str) -> StubNode | None:
        return self.get_node(path)

    def has_method(self, name: str) -> bool:
        return False

    def call(self, method: str, *_args: object) -> object:
        raise AssertionError(f"unexpected call on plain stub node: {method}")

    def queue_free(self) -> None:
        self.freed = True

    def connect(self, *_args: object) -> int:
        # The Node signal API shares the facade's `connect` name; the host
        # must reach the rust facade through dynamic `call` only.
        raise AssertionError("the host aliased the Node signal connect API")


class StubResource:
    """The Resource metadata surface the catalog reader consumes."""

    def __init__(self, meta: dict[str, Any]) -> None:
        self._meta = meta

    def has_meta(self, name: str) -> bool:
        return name in self._meta

    def get_meta(self, name: str, default: object = None) -> object:
        return self._meta.get(name, default)


class StubPackedScene:
    """Instantiates one fake feature per call, like the real scene wrapper."""

    def __init__(self, factory: Any) -> None:
        self._factory = factory

    def instantiate(self) -> object:
        return self._factory()


class StubResourceLoader:
    @classmethod
    def instance(cls) -> StubResourceLoader:
        return cls()

    def load(self, path: str) -> object:
        return RESOURCE_REGISTRY.get(path)


class StringArray:
    """The PackedStringArray surface `_metadata_strings` consumes."""

    def __init__(self, *items: str) -> None:
        self._items = tuple(items)

    def size(self) -> int:
        return len(self._items)

    def get(self, index: int) -> str:
        return self._items[index]


class FakeFeature(StubNode):
    """A feature double that only records what the production host asks of it."""

    def __init__(
        self,
        name: str,
        events: list[str],
        *,
        validate_failure: str = "",
        bind_failure: str = "",
        activate_failure: str = "",
    ) -> None:
        super().__init__(name)
        self._events = events
        self._failures = {
            "validate_feature": validate_failure,
            "bind_host": bind_failure,
            "activate_feature": activate_failure,
        }
        self.frames: list[object] = []
        self.calls: list[tuple[str, tuple[object, ...]]] = []

    def has_method(self, name: str) -> bool:
        return True

    def call(self, method: str, *args: object) -> object:
        self.calls.append((method, args))
        self._events.append(f"feature[{self.name}]:{method}")
        if method in self._failures:
            return self._failures[method]
        if method == "apply_typed_frame":
            if args[0] != {"pilot-frame": True}:
                assert_native_argument(args[0])
            self.frames.append(args[0])
            return ""
        if method == "deactivate_feature":
            self.frames.clear()
        if method == "drive_session":
            return 0
        if method == "pull_typed_frame":
            return {"pilot-frame": True}
        return None

    def method_names(self) -> list[str]:
        return [name for name, _ in self.calls]


def ok_envelope(value: object) -> dict[str, object]:
    return {"ok": True, "value": value, "error": None}


def error_envelope(error_class: str = "InvalidState") -> dict[str, object]:
    return {
        "ok": False,
        "value": None,
        "error": {"class": error_class, "resource": None, "limit": None, "observed": None},
    }


def family_table_value(names: tuple[str, ...] = PRODUCER_FAMILIES) -> dict[str, object]:
    return {
        "producer": "rust-client-core",
        "descriptors": [
            {
                "logical_name": name,
                "numeric_id": index + 1,
                "major": 1,
                "minor": 0,
                "record_limit": "4096",
                "record_bytes": "0",
            }
            for index, name in enumerate(names)
        ],
    }


def frame_value(
    epoch: str = "7",
    frame_index: str = "1",
    families: tuple[object, ...] = PRODUCER_FAMILIES,
) -> dict[str, object]:
    entries = [
        entry
        if isinstance(entry, dict)
        else {"key": {"logical_name": entry, "major": 1, "minor": 0}, "records": []}
        for entry in families
    ]
    return {
        "layout_major": 1,
        "layout_minor": 0,
        "session_epoch": epoch,
        "confirmed_revision": "3",
        "frame_index": frame_index,
        "families": entries,
    }


def representative_records() -> list[list[dict[str, Any]]]:
    header = {"epoch": "7", "revision": "3", "source_tick": None, "operation": "Upsert"}
    uuid = "00000000000040008000000000000001"
    empty = {"item": 0, "count": 0, "durability": 0}
    chunk = {"x": -1, "z": 2}
    block = {**chunk, "y": -64}
    pose = {"position": [0.0, 0.0, 0.0], "yaw": 0.0, "pitch": 0.0}
    companion = {"id": uuid, "name": "Nova"}
    observation = {"epoch": "7", "confirmed_revision": "3", "ordinal": 0}

    def tagged(tag: str, value: object = None) -> dict[str, Any]:
        return {"tag": tag, "value": value}

    def record(**parts: object) -> dict[str, Any]:
        return {"header": copy.deepcopy(header), **copy.deepcopy(parts)}

    reference = {"chunk": chunk, "kind": "Furnace", "slot": 31, "generation": 1}
    section = tagged("Section", {"chunk": chunk, "section": 23})
    resource = {
        "epoch": "7",
        "dimension": 0,
        "key": section,
        "generation": "1",
        "content_revision": "2",
        "job_id": "1",
    }
    terrain = record(
        dimension=0,
        key=section,
        content_revision="2",
        generation="1",
        material="Opaque",
        visibility="Near",
        light={"sky": 15, "block": 0},
        resource=resource,
    )
    far = copy.deepcopy(terrain)
    far.update(key=tagged("LodTile", chunk), visibility="Far", resource=None)
    actor_records = []
    for kind, identity, detail in (
        ("RemotePlayer", uuid, {"display_name": "Ada", "reset": False}),
        (
            "Drop",
            {"dimension": -9, "slot": 31, "generation": 1, "chunk": chunk},
            {"block_index": 98303, "stack": {"item": 10, "count": 1, "durability": 131}},
        ),
        ("Hostile", "1", {"archetype": 255, "health": 255}),
        ("Passive", "2", {"health": None, "grazing": 0, "despawn_reason": "Died"}),
        ("Projectile", "3", {"archetype": None}),
        ("Companion", uuid, {"name": "Nova", "reset": None}),
    ):
        actor_records.append(
            record(
                kind=kind,
                id=tagged(kind, identity),
                dimension=tagged("DropRaw", -9) if kind == "Drop" else tagged("Known", 0),
                position=[0.0, 0.0, 0.0],
                yaw=0.0,
                pitch=None,
                velocity=None,
                detail=tagged(kind, detail),
            )
        )
    inventory = []
    value: Any
    for tag, value in (
        ("Inventory", {"selected": 8, "hotbar": [empty] * 9, "backpack": [empty] * 27}),
        ("Crafting", {"size": "Personal", "slots": [empty] * 9, "output": empty}),
        ("Crafting", {"size": "Workbench", "slots": [empty] * 9, "output": empty}),
        (
            "Furnace",
            {
                "container": reference,
                "input": empty,
                "fuel": empty,
                "output": empty,
                "progress_ticks": 199,
                "burn_ticks": 1600,
            },
        ),
        ("Chest", {"container": {**reference, "kind": "Chest", "slot": 15}, "items": [empty] * 27}),
        ("Closed", reference),
        ("Rejected", {"sequence": "0", "reason": "InvalidRay"}),
    ):
        inventory.append(record(view=tagged(tag, value), token=None, outcome=None))
    inventory[3]["token"] = {"epoch": "7", "reference": reference, "confirmed_revision": "3"}
    inventory[3]["outcome"] = tagged("PlacementAccepted", {"sequence": "1"})
    inventory[-1]["outcome"] = tagged("Rejected", {"sequence": "2", "reason": "NotArmor"})
    worlds = [
        record(
            view=tagged(
                "Environment",
                {
                    "day_phase_offset": 23999,
                    "world_time_ticks": "18446744073709551615",
                    "weather": "Thunder",
                    "season": "Winter",
                    "season_progress": 255,
                    "temperature": -128,
                },
            )
        ),
        record(
            view=tagged(
                "Survival",
                {
                    "health": 20,
                    "oxygen": 300,
                    "hunger": 20,
                    "saturation_zero": False,
                    "armor_points": 20,
                },
            )
        ),
        record(
            view=tagged(
                "Task",
                {
                    "observation": observation,
                    "companion": companion,
                    "command": "work",
                    "state": tagged("Started"),
                },
            )
        ),
        record(view=tagged("Prompt")),
        record(view=tagged("Prompt", {"target": block, "label": "stone"})),
    ]
    for tag, value in (
        ("InvalidFormat", None),
        ("UnknownCompanion", {"name": "Nova"}),
        ("Accepted", {"companion": companion, "command": "work"}),
        ("QueueFull", {"companion": companion, "command": "work"}),
        ("NotFollowing", {"companion": companion, "command": "work"}),
        (
            "Task",
            {"companion": companion, "command": "work", "state": tagged("Failed", "WorldChanged")},
        ),
        ("Speech", {"companion": companion, "text": "hello"}),
    ):
        worlds.append(
            record(
                view=tagged(
                    "Chat",
                    {
                        "event_id": "1",
                        "player_id": uuid,
                        "player_name": "Ada Lovelace",
                        "body": tagged(tag, value),
                    },
                )
            )
        )
    return [
        [
            record(phase="Admitted", player_id=uuid, terminal=None),
            record(phase="Closing", player_id=None, terminal=tagged("RemoteDisconnect", "bye")),
        ],
        [
            record(local_sequence="0", intent_kind="PlayerInput", receipt=tagged("Queued")),
            record(local_sequence=None, intent_kind="Chat", receipt=tagged("Rejected", "Capacity")),
            record(
                local_sequence="1",
                intent_kind="PlaceBlock",
                receipt=tagged("Confirmed", {"server_tick": None}),
            ),
            record(
                local_sequence="2",
                intent_kind="PlaceBlock",
                receipt=tagged("Confirmed", {"server_tick": "9"}),
            ),
        ],
        [terrain, far],
        actor_records,
        [
            record(
                confirmed_pose=pose,
                predicted_pose=pose,
                look_ray={
                    "origin": [0.0, 0.0, 0.0],
                    "look": {"yaw": 0.0, "pitch": 0.0},
                    "reach": 6.0,
                },
                movement={
                    "control": {
                        "movement": {"move_x": -128, "move_z": 127, "jump": False},
                        "look": {"yaw": 0.0, "pitch": 0.0},
                        "actions": {
                            "primary": True,
                            "eating": False,
                            "sprinting": False,
                            "sneaking": False,
                        },
                    },
                    "on_ground": True,
                },
                correction={"last_input_sequence": "0", "reason": "AcknowledgedReplay"},
                mining=tagged(
                    "Active", {"target": block, "progress": 1, "required": 2, "harvestable": True}
                ),
            ),
            record(
                confirmed_pose=pose,
                predicted_pose=None,
                look_ray=None,
                movement={"control": None, "on_ground": False},
                correction=None,
                mining=tagged("Idle"),
            ),
        ],
        inventory,
        worlds,
        [
            record(
                cue_id=6,
                provenance=tagged(tag, value),
                category="World",
                position=None,
                gain=1.0,
                pitch=1.0,
            )
            for tag, value in (
                ("Confirmed", {"observation": observation, "authoritative_event_id": None}),
                ("Predicted", {"input_sequence": "0"}),
                ("Local", {"local_event_sequence": "0"}),
            )
        ],
        [
            record(
                transition="Reset",
                generation="0",
                resource_order=[
                    tagged(tag)
                    for tag in (
                        "InputJournal",
                        "PreparationQueue",
                        "PresentationFrames",
                        "BridgeHandles",
                    )
                ]
                + [tagged("Feature", "0")],
            )
        ],
        [
            record(
                producer={"source_sha": "0" * 40, "contract_sha": "f" * 64},
                frame_index="0",
                queue_high_water={
                    key: "0"
                    for key in (
                        "input",
                        "inbound_records",
                        "inbound_bytes",
                        "outbound_records",
                        "outbound_bytes",
                        "journal",
                        "preparation",
                    )
                },
                rejected={
                    key: "0"
                    for key in ("invalid", "capacity", "stale", "disconnected", "io", "internal")
                },
            )
        ],
    ]


class ScriptedBridge(StubNode):
    """The scripted rust-facade boundary with call recording.

    Every method the host may call answers a closed envelope; tests swap the
    scripted values or mark a method as refused to exercise host validation.
    """

    def __init__(self, name: str, events: list[str]) -> None:
        super().__init__(name)
        self._events = events
        self.calls: list[tuple[str, tuple[object, ...]]] = []
        self.refused: set[str] = set()
        self.open_value: object = {"slot": 1, "generation": "4"}
        self.connect_value: object = "7"
        self.reset_value: object = "9"
        self.table_value: object = family_table_value()
        self.step_value: object = {
            "epoch": "7",
            "confirmed_revision": "3",
            "frame_index": "1",
            "processed_messages": 0,
            "processed_meshes": 0,
            "pending_input": 0,
            "pending_inbound": 0,
            "pending_preparation": 0,
            "terminal": None,
        }
        self.frame_value: object = frame_value()
        self.before_close: Callable[[], None] = lambda: None

    def has_method(self, name: str) -> bool:
        return True

    def call(self, method: str, *args: object) -> object:
        for argument in args:
            assert_native_argument(argument)
        self.calls.append((method, args))
        self._events.append(f"bridge:{method}")
        if method == "close":
            self.before_close()
        if method in self.refused:
            return native_value(error_envelope())
        if method == "open_core":
            return native_value(ok_envelope(self.open_value))
        if method == "connect":
            return native_value(ok_envelope(self.connect_value))
        if method == "reset":
            return native_value(ok_envelope(self.reset_value))
        if method == "family_table":
            return native_value(ok_envelope(self.table_value))
        if method == "step":
            return native_value(ok_envelope(self.step_value))
        if method == "pull_typed_frame":
            return native_value(ok_envelope(self.frame_value))
        if method == "close":
            return native_value(ok_envelope(None))
        if method == "feature_families_json":
            return PILOT_FAMILY_JSON
        return None

    def count(self, method: str) -> int:
        return sum(1 for name, _ in self.calls if name == method)

    def arguments_of(self, method: str) -> list[tuple[object, ...]]:
        return [arguments for name, arguments in self.calls if name == method]


def install_py4godot_stubs() -> None:
    """Install the py4godot module shells the host module imports."""
    py4godot = types.ModuleType("py4godot")
    classes = types.ModuleType("py4godot.classes")

    def gdclass(cls: type) -> type:
        return cls

    classes.__dict__["gdclass"] = gdclass
    core_module = types.ModuleType("py4godot.classes.core")
    core_module.__dict__.update(Dictionary=NativeDictionary, Array=NativeArray)
    node_module = types.ModuleType("py4godot.classes.Node")
    node_module.__dict__["Node"] = StubNode
    object_module = types.ModuleType("py4godot.classes.Object")
    object_module.__dict__["Object"] = StubNode
    scene_module = types.ModuleType("py4godot.classes.PackedScene")
    scene_module.__dict__["PackedScene"] = StubPackedScene
    resource_module = types.ModuleType("py4godot.classes.Resource")
    resource_module.__dict__["Resource"] = StubResource
    loader_module = types.ModuleType("py4godot.classes.ResourceLoader")
    loader_module.__dict__["ResourceLoader"] = StubResourceLoader
    utils = types.ModuleType("py4godot.utils")
    smart_cast = types.ModuleType("py4godot.utils.smart_cast")

    def register_cast_function(name: str, function: object) -> None:
        CAST_REGISTRATIONS[name] = function

    smart_cast.__dict__["register_cast_function"] = register_cast_function
    py4godot.__dict__["classes"] = classes
    utils.__dict__["smart_cast"] = smart_cast
    for name, module in {
        "py4godot": py4godot,
        "py4godot.classes": classes,
        "py4godot.classes.core": core_module,
        "py4godot.classes.Node": node_module,
        "py4godot.classes.Object": object_module,
        "py4godot.classes.PackedScene": scene_module,
        "py4godot.classes.Resource": resource_module,
        "py4godot.classes.ResourceLoader": loader_module,
        "py4godot.utils": utils,
        "py4godot.utils.smart_cast": smart_cast,
    }.items():
        sys.modules.setdefault(name, module)


def load_host_module() -> types.ModuleType:
    spec = importlib.util.spec_from_file_location("feature_host_under_test", HOST_MODULE_PATH)
    if spec is None or spec.loader is None:
        raise AssertionError(f"the production host module could not be located: {HOST_MODULE_PATH}")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


install_py4godot_stubs()
HOST = load_host_module()

CONFIG_JSON = '{"limits": {}}'
ENDPOINT_JSON = '{"tag": "Memory", "value": {"connector_id": "1"}}'
IDENTITY_JSON = '{"login": {}, "requested_view_distance": 8}'


class RustFeatureHostTest(unittest.TestCase):
    def setUp(self) -> None:
        RESOURCE_REGISTRY.clear()
        RESOURCE_REGISTRY["res://catalog/capability_registry.tres"] = StubResource(
            {
                "rust_producer": "rust-client-core",
                "rust_producer_families": StringArray(
                    *(
                        f"{name}@1.0:{index + 1}:4096:0"
                        for index, name in enumerate(PRODUCER_FAMILIES)
                    )
                ),
            }
        )
        CAST_REGISTRATIONS.clear()
        # Re-running the production module import refreshes the identity-cast
        # registration the host performs at import time.
        globals()["HOST"] = load_host_module()
        self.events: list[str] = []
        self.root = StubNode("root")
        self.host: Any = HOST.feature_host(name="FeatureHost", parent=self.root)
        self.root.add_child(self.host)
        self.host._ready()
        self.bridge = ScriptedBridge("ClientBridge", self.events)
        self.root.add_child(self.bridge)

    def register_feature(
        self,
        feature_id: str,
        *,
        depends: tuple[str, ...] = (),
        families: tuple[str, ...] = ("session@1.0",),
        required: bool = True,
        enabled: bool = True,
        validate_failure: str = "",
        bind_failure: str = "",
        activate_failure: str = "",
    ) -> None:
        scene_path = f"res://unit/{feature_id}.tscn"
        manifest_path = f"res://unit/{feature_id}_manifest.tres"

        def factory() -> FakeFeature:
            return FakeFeature(
                feature_id,
                self.events,
                validate_failure=validate_failure,
                bind_failure=bind_failure,
                activate_failure=activate_failure,
            )

        RESOURCE_REGISTRY[scene_path] = StubPackedScene(factory)
        RESOURCE_REGISTRY[manifest_path] = StubResource(
            {
                "feature_id": feature_id,
                "host_protocol_major": 1,
                "host_protocol_minor": 0,
                "entry_scene_path": scene_path,
                "dependencies": StringArray(*depends),
                "required_bridge_families": StringArray(*families),
                "required": required,
                "enabled": enabled,
                "budget_class": "standard",
                "reset_policy": "reset",
            }
        )

    def register_catalog(self, name: str, feature_ids: tuple[str, ...]) -> str:
        catalog_path = f"res://unit/{name}.tres"
        RESOURCE_REGISTRY[catalog_path] = StubResource(
            {
                "host_protocol_major": 1,
                "host_protocol_minor": 0,
                "manifest_paths": StringArray(
                    *(f"res://unit/{feature_id}_manifest.tres" for feature_id in feature_ids)
                ),
            }
        )
        return catalog_path

    def activate_rust(self, catalog_path: str, epoch: int = 5) -> dict[str, Any]:
        raw = self.host.activate_rust_catalog(
            catalog_path,
            "../ClientBridge",
            epoch,
            CONFIG_JSON,
            ENDPOINT_JSON,
            IDENTITY_JSON,
        )
        return cast(dict[str, Any], json.loads(raw))

    def active_feature(self, feature_id: str) -> FakeFeature:
        instance = self.host._instances[feature_id]
        assert isinstance(instance, FakeFeature)
        return instance

    # ------------------------------------------------------------------
    # Session ownership and single dispatch
    # ------------------------------------------------------------------

    def test_two_features_share_one_step_one_pull_and_one_frame_object(self) -> None:
        self.register_feature("provider", families=("session@1.0",))
        self.register_feature("consumer", depends=("provider",), families=("input@1.0",))
        catalog = self.register_catalog("two", ("provider", "consumer"))
        result = self.activate_rust(catalog)
        self.assertTrue(result["ok"], result)
        for _ in range(3):
            self.host._process(1 / 60)
        self.assertEqual(self.bridge.count("step"), 3)
        self.assertEqual(self.bridge.count("pull_typed_frame"), 3)
        provider = self.active_feature("provider")
        consumer = self.active_feature("consumer")
        self.assertEqual(len(provider.frames), 3)
        self.assertEqual(len(consumer.frames), 3)
        for tick in range(3):
            self.assertIs(provider.frames[tick], consumer.frames[tick])
        self.assertEqual(plain_value(provider.frames[0]), self.bridge.frame_value)
        self.assertIsNot(provider.frames[0], self.bridge.frame_value)

    def test_features_never_drive_or_pull_the_session(self) -> None:
        self.register_feature("alpha")
        self.register_feature("beta", depends=("alpha",))
        catalog = self.register_catalog("nodrive", ("alpha", "beta"))
        self.assertTrue(self.activate_rust(catalog)["ok"])
        self.host._process(1 / 60)
        for feature_id in ("alpha", "beta"):
            names = self.active_feature(feature_id).method_names()
            for forbidden in ("drive_session", "drive_world", "pull_typed_frame", "drive_input"):
                self.assertNotIn(forbidden, names)

    def test_host_passes_the_issued_token_and_epoch_verbatim(self) -> None:
        self.register_feature("solo")
        catalog = self.register_catalog("token", ("solo",))
        self.assertTrue(self.activate_rust(catalog)["ok"])
        self.host._process(1 / 60)
        for method in ("connect", "family_table", "step", "pull_typed_frame"):
            arguments_list = self.bridge.arguments_of(method)
            self.assertTrue(arguments_list, method)
            for arguments in arguments_list:
                self.assertEqual(plain_value(arguments[0]), self.bridge.open_value)
        step_arguments = self.bridge.arguments_of("step")[0]
        self.assertEqual(step_arguments[1], "7")
        self.assertEqual(plain_value(step_arguments[2]), {"messages": 64, "meshes": 32})
        self.assertEqual(self.host.rust_session_epoch(), "7")
        # Features never observe the token: only the bridge path and the
        # lifecycle epoch cross the feature boundary.
        for _, arguments in self.active_feature("solo").calls:
            for argument in arguments:
                self.assertIsNot(argument, self.bridge.open_value)

    def test_activation_uses_scene_path_identity_cast_for_the_bridge(self) -> None:
        self.assertIn("MornleaClientBridge", CAST_REGISTRATIONS)
        self.assertIsNotNone(self.host.get_node("../ClientBridge"))

    # ------------------------------------------------------------------
    # Result, frame, and descriptor validation
    # ------------------------------------------------------------------

    def assert_retains_last_frame_and_resources(
        self, feature_ids: tuple[str, ...], first_frame: object
    ) -> None:
        """A refused dispatch keeps features, epoch, and the last frame."""
        self.assertEqual(self.host.active_count(), len(feature_ids))
        self.assertEqual(self.host.rust_session_epoch(), "7")
        self.assertIs(self.host._rust_frame, first_frame)
        for feature_id in feature_ids:
            feature = self.active_feature(feature_id)
            self.assertEqual(len(feature.frames), 1, feature_id)
            self.assertFalse(feature.freed, feature_id)

    def test_failed_step_and_pull_call_no_apply_and_retain_state(self) -> None:
        self.register_feature("provider")
        self.register_feature("consumer", depends=("provider",))
        catalog = self.register_catalog("failstep", ("provider", "consumer"))
        self.assertTrue(self.activate_rust(catalog)["ok"])
        self.host._process(1 / 60)
        first_frame = self.host._rust_frame
        self.assertIsNotNone(first_frame)
        self.bridge.refused.add("step")
        self.host._process(1 / 60)
        self.assertEqual(self.bridge.count("pull_typed_frame"), 1)
        self.assert_retains_last_frame_and_resources(("provider", "consumer"), first_frame)
        self.bridge.refused.remove("step")
        self.bridge.refused.add("pull_typed_frame")
        self.host._process(1 / 60)
        self.assert_retains_last_frame_and_resources(("provider", "consumer"), first_frame)
        self.bridge.refused.remove("pull_typed_frame")
        self.host._process(1 / 60)
        self.assertEqual(len(self.active_feature("consumer").frames), 2)

    def test_incomplete_frame_shapes_call_no_apply(self) -> None:
        self.register_feature("provider")
        catalog = self.register_catalog("incomplete", ("provider",))
        self.assertTrue(self.activate_rust(catalog)["ok"])
        self.host._process(1 / 60)
        first_frame = self.host._rust_frame
        incomplete_frames = (
            {"layout_major": 1, "layout_minor": 0, "session_epoch": "7"},
            frame_value(families=()),
            {**frame_value(), "confirmed_revision": "03"},
            {**frame_value(), "frame_index": 1},
            {**frame_value(), "families": ()},
            {**frame_value(), "families": ({"key": {"logical_name": "session"}, "records": []},)},
            {**frame_value(), "families": ({"key": "session", "records": []},)},
            "not-a-frame",
        )
        for frame in incomplete_frames:
            self.bridge.frame_value = frame
            self.host._process(1 / 60)
            self.assert_retains_last_frame_and_resources(("provider",), first_frame)

    def test_mixed_and_stale_frames_call_no_apply(self) -> None:
        self.register_feature("provider", families=("session@1.0", "input@1.0"))
        catalog = self.register_catalog("mixed", ("provider",))
        self.assertTrue(self.activate_rust(catalog)["ok"])
        self.host._process(1 / 60)
        first_frame = self.host._rust_frame
        mixed_frames = (
            # Duplicate family keys inside one frame.
            frame_value(families=("session", "session")),
            # A family the negotiated producer table never registered.
            frame_value(families=("session", "unregistered")),
            # A family key whose major disagrees with the descriptor table.
            frame_value(
                families=(
                    "session",
                    {"key": {"logical_name": "input", "major": 2, "minor": 0}, "records": []},
                )
            ),
            # A frame of a different session epoch (stale callback).
            frame_value(epoch="6"),
        )
        for frame in mixed_frames:
            self.bridge.frame_value = frame
            self.host._process(1 / 60)
            self.assert_retains_last_frame_and_resources(("provider",), first_frame)

    def test_missing_mandatory_family_calls_no_apply(self) -> None:
        self.register_feature("provider", families=("session@1.0", "audio-cues@1.0"))
        catalog = self.register_catalog("mandatory", ("provider",))
        self.assertTrue(self.activate_rust(catalog)["ok"])
        self.host._process(1 / 60)
        first_frame = self.host._rust_frame
        # The active manifest requires the audio-cues family; a frame that
        # drops it is incomplete for this host even though it is well formed.
        self.bridge.frame_value = frame_value(families=("session", "input"))
        self.host._process(1 / 60)
        self.assert_retains_last_frame_and_resources(("provider",), first_frame)

    def test_success_envelope_requires_exact_fields_and_null_error(self) -> None:
        for result in (
            {"ok": True, "value": {"visible": True}, "error": {"class": "Internal"}},
            {"ok": True, "value": {"visible": True}},
            {"ok": True, "value": {"visible": True}, "error": None, "extra": 0},
        ):
            with self.subTest(result=result):
                self.assertIsNone(HOST._rust_envelope_value(result))

    def test_incompatible_mixed_and_regressed_publications_retain_resources(self) -> None:
        self.register_feature("consumer")
        catalog = self.register_catalog("strict-frames", ("consumer",))
        self.assertTrue(self.activate_rust(catalog)["ok"])
        self.bridge.frame_value = frame_value(frame_index="9")
        self.host._process(0.01)
        first_frame = self.host._rust_frame
        bad_frames: list[dict[str, object]] = [
            {**frame_value(), "layout_major": 65535},
            {**frame_value(), "layout_minor": 1},
            frame_value(frame_index="1"),
            {**frame_value(frame_index="10"), "confirmed_revision": "2"},
            {**frame_value(frame_index="10"), "extra": None},
        ]
        child = {
            "header": {"epoch": "6", "revision": "99", "source_tick": None, "operation": "Upsert"},
            "phase": "Admitted",
            "player_id": None,
            "terminal": None,
        }
        for header_change in (
            {},
            {"epoch": "7", "revision": "3", "source_tick": "01"},
            {"epoch": "7", "revision": "3", "operation": "Unknown"},
        ):
            bad = frame_value(frame_index="10")
            cast(list[dict[str, object]], bad["families"])[0]["records"] = [
                {**child, "header": {**cast(dict[str, object], child["header"]), **header_change}}
            ]
            bad_frames.append(bad)
        bad = frame_value(frame_index="10")
        cast(list[dict[str, object]], bad["families"])[0]["records"] = [child] * 4097
        bad_frames.append(bad)
        for frame in bad_frames:
            with self.subTest(frame=frame.get("frame_index")):
                self.bridge.frame_value = frame
                self.host._process(0.01)
                self.assert_retains_last_frame_and_resources(("consumer",), first_frame)
        self.bridge.frame_value = first_frame
        self.host._process(0.01)
        self.assertEqual(len(self.active_feature("consumer").frames), 2)

    def test_complete_payload_refusal_preserves_frame_and_resources(self) -> None:
        self.register_feature("consumer")
        catalog = self.register_catalog("payload-refusal", ("consumer",))
        self.assertTrue(self.activate_rust(catalog)["ok"])
        self.host._process(0.01)
        previous = self.host._rust_frame
        header = {"epoch": "7", "revision": "3", "source_tick": None, "operation": "Upsert"}
        malformed = (
            {"header": header},
            {"header": header, "phase": 123, "player_id": [], "terminal": "bad", "extra": None},
            {
                "header": header,
                "phase": "Admitted",
                "player_id": None,
                "terminal": None,
                "extra": [0] * 100000,
            },
        )
        for record in malformed:
            candidate = frame_value(frame_index="2")
            cast(list[dict[str, object]], candidate["families"])[0]["records"] = [record]
            self.bridge.frame_value = candidate
            self.host._process(0.01)
            self.assert_retains_last_frame_and_resources(("consumer",), previous)
        for missing in range(10):
            candidate = frame_value(frame_index="2")
            cast(list[dict[str, object]], candidate["families"]).pop(missing)
            self.bridge.frame_value = candidate
            self.host._process(0.01)
            self.assert_retains_last_frame_and_resources(("consumer",), previous)
        self.bridge.frame_value = frame_value(frame_index="2", families=("session",))
        self.host._process(0.01)
        self.assert_retains_last_frame_and_resources(("consumer",), previous)

    def test_renderer_payload_variants_are_dispatched_as_one_frame(self) -> None:
        self.register_feature("consumer")
        catalog = self.register_catalog("payload-success", ("consumer",))
        self.assertTrue(self.activate_rust(catalog)["ok"])
        candidate = frame_value()
        for family, records in zip(
            cast(list[dict[str, Any]], candidate["families"]), representative_records(), strict=True
        ):
            family["records"] = records
        self.bridge.frame_value = candidate
        self.host._process(0.01)
        self.assertEqual(self.host._rust_frame, candidate)
        self.assertIsNot(self.host._rust_frame, candidate)
        self.assertEqual(plain_value(self.active_feature("consumer").frames[0]), candidate)

    def test_every_payload_field_is_required_and_extra_data_is_refused(self) -> None:
        self.register_feature("consumer")
        catalog = self.register_catalog("closed-payloads", ("consumer",))
        self.assertTrue(self.activate_rust(catalog)["ok"])
        self.host._process(0.01)
        previous = self.host._rust_frame
        for index, records in enumerate(representative_records()):
            for record in records:
                for field in (*record, "extra"):
                    with self.subTest(family=index + 1, field=field):
                        malformed = copy.deepcopy(record)
                        if field == "extra":
                            malformed[field] = [0] * 100000
                        else:
                            del malformed[field]
                        candidate = frame_value(frame_index="2")
                        cast(list[dict[str, Any]], candidate["families"])[index]["records"] = [
                            malformed
                        ]
                        self.bridge.frame_value = candidate
                        self.host._process(0.01)
                        self.assert_retains_last_frame_and_resources(("consumer",), previous)

    def test_nested_shapes_types_and_domain_bounds_preserve_consumers(self) -> None:
        self.register_feature("consumer")
        catalog = self.register_catalog("nested-payloads", ("consumer",))
        self.assertTrue(self.activate_rust(catalog)["ok"])
        self.host._process(0.01)
        previous = self.host._rust_frame
        records = representative_records()

        def mutations(value: Any) -> list[tuple[tuple[object, ...], object]]:
            results: list[tuple[tuple[object, ...], object]] = []
            if isinstance(value, dict):
                results.append(((), {**value, "extra": [0] * 100000}))
                for key, child in value.items():
                    removed = dict(value)
                    del removed[key]
                    results.append(((), removed))
                    results.extend(((key, *path), changed) for path, changed in mutations(child))
            elif isinstance(value, list):
                results.append(((), value + [None] * 100000))
                for index, child in enumerate(value):
                    results.extend(((index, *path), changed) for path, changed in mutations(child))
            elif value is not None:
                results.append(((), {} if isinstance(value, str) else "wrong-type"))
                if type(value) is int:
                    results.append(((), True))
                if type(value) is float:
                    results.extend(((), changed) for changed in (float("nan"), float("inf"), 0))
            return results

        for family_index, family_records in enumerate(records):
            for source in family_records:
                for path, changed in mutations(source):
                    with self.subTest(family=family_index + 1, path=path):
                        malformed: Any = copy.deepcopy(source)
                        if not path:
                            malformed = changed
                        else:
                            target: Any = malformed
                            for key in path[:-1]:
                                target = target[key]
                            target[path[-1]] = changed
                        candidate = frame_value(frame_index="2")
                        cast(list[dict[str, Any]], candidate["families"])[family_index][
                            "records"
                        ] = [malformed]
                        self.bridge.frame_value = candidate
                        self.host._process(0.01)
                        self.assert_retains_last_frame_and_resources(("consumer",), previous)

        cases: tuple[tuple[int, int, tuple[str, ...], object], ...] = (
            (0, 0, ("player_id",), "0" * 32),
            (0, 0, ("player_id",), "00000000000050008000000000000001"),
            (1, 0, ("local_sequence",), "01"),
            (1, 0, ("local_sequence",), "18446744073709551616"),
            (1, 0, ("receipt", "tag"), "Invented"),
            (2, 0, ("key", "value", "section"), 24),
            (2, 0, ("light", "sky"), 16),
            (2, 0, ("resource", "job_id"), "0"),
            (2, 0, ("visibility",), "Far"),
            (3, 0, ("id", "tag"), "Companion"),
            (3, 0, ("detail", "tag"), "Companion"),
            (3, 0, ("dimension",), {"tag": "DropRaw", "value": 0}),
            (3, 1, ("id", "value", "slot"), 32),
            (3, 1, ("detail", "value", "stack", "durability"), 132),
            (4, 0, ("mining", "value", "progress"), 2),
            (4, 0, ("look_ray", "reach"), 6.1),
            (5, 0, ("view", "value", "selected"), 9),
            (5, 1, ("view", "value", "slots"), [{"item": 1, "count": 1, "durability": 0}] * 9),
            (5, 3, ("view", "value", "progress_ticks"), 200),
            (5, 3, ("view", "value", "burn_ticks"), 1601),
            (5, 3, ("view", "value", "fuel"), {"item": 1, "count": 1, "durability": 0}),
            (5, 4, ("view", "value", "container", "slot"), 16),
            (6, 0, ("view", "value", "day_phase_offset"), 24000),
            (6, 1, ("view", "value", "oxygen"), 301),
            (6, 2, ("view", "value", "companion", "name"), "two names"),
            (6, 2, ("view", "value", "command"), " work "),
            (6, 2, ("view", "value", "command"), "界" * 342),
            (7, 0, ("cue_id",), 7),
            (7, 0, ("gain",), 1.1),
            (7, 0, ("pitch",), 0.0),
            (
                8,
                0,
                ("resource_order",),
                [{"tag": "Feature", "value": "0"}] * ((_FRAME_BYTES // 2) + 1),
            ),
            (9, 0, ("producer", "source_sha"), "A" * 40),
        )
        for index, record_index, path, changed in cases:
            with self.subTest(family=index + 1, domain=path):
                malformed = copy.deepcopy(records[index][record_index])
                target = malformed
                for key in path[:-1]:
                    target = target[key]
                target[path[-1]] = changed
                candidate = frame_value(frame_index="2")
                cast(list[dict[str, Any]], candidate["families"])[index]["records"] = [malformed]
                self.bridge.frame_value = candidate
                self.host._process(0.01)
                self.assert_retains_last_frame_and_resources(("consumer",), previous)

    def test_all_renderer_error_classes_and_legal_opaque_relationships_are_admitted(self) -> None:
        table = HOST.FamilyTable(
            {name: (1, 0) for name in PRODUCER_FAMILIES},
            {name: (index + 1, 1, 0, 4096, 0) for index, name in enumerate(PRODUCER_FAMILIES)},
        )
        for error_class in (
            "InvalidInput",
            "InvalidState",
            "IncompatibleVersion",
            "StaleEpoch",
            "Capacity",
            "Timeout",
            "Disconnected",
            "Io",
            "Internal",
        ):
            candidate = frame_value()
            record = representative_records()[1][1]
            record["receipt"]["value"] = error_class
            cast(list[dict[str, Any]], candidate["families"])[1]["records"] = [record]
            self.assertIs(HOST._rust_frame_from(ok_envelope(candidate), "7", table, ()), candidate)

        candidate = frame_value()
        records = representative_records()
        records[2][0]["resource"].update(
            epoch="99",
            generation="0",
            content_revision="0",
            key={"tag": "LodTile", "value": {"x": 0, "z": 0}},
        )
        records[3][1]["detail"]["value"]["block_index"] = 98304
        records[3][1]["dimension"] = {"tag": "Known", "value": 1}
        records[5][0]["token"] = {
            "epoch": "99",
            "confirmed_revision": "0",
            "reference": {"chunk": {"x": 0, "z": 0}, "kind": "Chest", "slot": 0, "generation": 1},
        }
        for family, values in zip(
            cast(list[dict[str, Any]], candidate["families"]), records, strict=True
        ):
            family["records"] = values
        self.assertIs(HOST._rust_frame_from(ok_envelope(candidate), "7", table, ()), candidate)

    def test_f32_fields_reject_finite_double_overflow(self) -> None:
        table = HOST.FamilyTable(
            {name: (1, 0) for name in PRODUCER_FAMILIES},
            {name: (index + 1, 1, 0, 4096, 0) for index, name in enumerate(PRODUCER_FAMILIES)},
        )
        for family_index, path in (
            (4, ("look_ray", "look", "yaw")),
            (4, ("movement", "control", "look", "pitch")),
            (7, ("pitch",)),
        ):
            candidate = frame_value()
            record = representative_records()[family_index][0]
            target = record
            for key in path[:-1]:
                target = target[key]
            target[path[-1]] = 1e300
            cast(list[dict[str, Any]], candidate["families"])[family_index]["records"] = [record]
            self.assertTrue(HOST._rust_frame_from(ok_envelope(candidate), "7", table, ()) is None)

    def test_erased_text_kind_accepts_constructor_union_without_widening_domain_text(self) -> None:
        table = HOST.FamilyTable(
            {name: (1, 0) for name in PRODUCER_FAMILIES},
            {name: (index + 1, 1, 0, 4096, 0) for index, name in enumerate(PRODUCER_FAMILIES)},
        )
        for text in ("", "x" * 1024, "\n", "界" * 341):
            candidate = frame_value()
            records = representative_records()
            records[0][1]["terminal"]["value"] = text
            records[3][0]["detail"]["value"]["display_name"] = text
            records[6][4]["view"]["value"]["label"] = text
            for family, values in zip(
                cast(list[dict[str, Any]], candidate["families"]), records, strict=True
            ):
                family["records"] = values
            self.assertIs(HOST._rust_frame_from(ok_envelope(candidate), "7", table, ()), candidate)
        for text in ("x" * 1025, "界" * 342, "\ud800"):
            for numeric_id, record_index, path in (
                (1, 1, ("terminal", "value")),
                (4, 0, ("detail", "value", "display_name")),
                (7, 4, ("view", "value", "label")),
            ):
                candidate = frame_value()
                record = representative_records()[numeric_id - 1][record_index]
                target = record
                for key in path[:-1]:
                    target = target[key]
                target[path[-1]] = text
                cast(list[dict[str, Any]], candidate["families"])[numeric_id - 1]["records"] = [
                    record
                ]
                self.assertTrue(
                    HOST._rust_frame_from(ok_envelope(candidate), "7", table, ()) is None
                )

    def test_oversized_values_refuse_before_iteration_or_encoding(self) -> None:
        class UnvisitedArray(list[Any]):
            def __iter__(self) -> Any:
                raise AssertionError("an oversized array was traversed")

        class UnencodedText(str):
            def encode(self, encoding: str = "utf-8", errors: str = "strict") -> bytes:
                raise AssertionError("an oversized text was encoded")

        class UnvisitedRecord(dict[str, Any]):
            def items(self) -> Any:
                raise AssertionError("an oversized record was traversed")

        table = HOST.FamilyTable(
            {name: (1, 0) for name in PRODUCER_FAMILIES},
            {name: (index + 1, 1, 0, 4096, 0) for index, name in enumerate(PRODUCER_FAMILIES)},
        )
        records = representative_records()
        records[0][1]["terminal"]["value"] = UnencodedText("x" * 1025)
        records[3][0]["position"] = UnvisitedArray([0.0] * 4)
        records[5][0]["view"]["value"]["hotbar"] = UnvisitedArray([{}] * 10)
        records[9][0] = UnvisitedRecord({str(index): None for index in range(12)})
        for index in (0, 3, 5, 9):
            candidate = frame_value()
            cast(list[dict[str, Any]], candidate["families"])[index]["records"] = records[index]
            self.assertTrue(HOST._rust_frame_from(ok_envelope(candidate), "7", table, ()) is None)

    def test_semantic_frame_bytes_use_actual_nullary_and_feature_charges(self) -> None:
        table = HOST.FamilyTable(
            {name: (1, 0) for name in PRODUCER_FAMILIES},
            {name: (index + 1, 1, 0, 4096, 0) for index, name in enumerate(PRODUCER_FAMILIES)},
        )
        candidate = frame_value()
        base = 36 + sum(len(name) + 13 for name in PRODUCER_FAMILIES) + 35
        for tag, width in (("InputJournal", 2), ("Feature", 9)):
            with self.subTest(tag=tag):
                record = representative_records()[8][0]
                entry = {"tag": tag, "value": "0" if tag == "Feature" else None}
                count = (_FRAME_BYTES - base) // width
                record["resource_order"] = [entry] * count
                cast(list[dict[str, Any]], candidate["families"])[8]["records"] = [record]
                self.assertIs(
                    HOST._rust_frame_from(ok_envelope(candidate), "7", table, ()), candidate
                )
                record["resource_order"] = [entry] * (count + 1)
                self.assertTrue(
                    HOST._rust_frame_from(ok_envelope(candidate), "7", table, ()) is None
                )
        empty_revision = {**frame_value(), "confirmed_revision": "0"}
        for index in range(2, 8):
            record = representative_records()[index][0]
            record["header"]["revision"] = "0"
            candidate = copy.deepcopy(empty_revision)
            cast(list[dict[str, Any]], candidate["families"])[index]["records"] = [record]
            self.assertTrue(HOST._rust_frame_from(ok_envelope(candidate), "7", table, ()) is None)

    def test_declared_descriptor_identity_and_bounds_are_required(self) -> None:
        mutations: tuple[Callable[[dict[str, Any]], object], ...] = (
            lambda table: table["descriptors"][1].update(numeric_id=1),
            lambda table: table["descriptors"].append(
                {
                    "logical_name": "invented",
                    "numeric_id": 70000,
                    "major": 1,
                    "minor": 0,
                    "record_limit": "0",
                    "record_bytes": "0",
                }
            ),
            lambda table: table["descriptors"][0].update(record_limit="4095"),
            lambda table: table["descriptors"][0].update(record_bytes="1"),
            lambda table: table["descriptors"][0].update(major=True),
            lambda table: table["descriptors"][0].update(extra=None),
            lambda table: table["descriptors"].pop(),
            lambda table: table.update(extra=None),
        )
        for index, mutate in enumerate(mutations):
            with self.subTest(index=index):
                self.setUp()
                self.register_feature("consumer")
                catalog = self.register_catalog("strict-table", ("consumer",))
                table = copy.deepcopy(family_table_value())
                mutate(table)
                self.bridge.table_value = table
                self.assertFalse(self.activate_rust(catalog)["ok"])
                self.assertEqual(self.host.active_count(), 0)
                self.assertEqual(self.bridge.count("close"), 1)

    def test_step_report_accepts_actual_retained_work_ceilings(self) -> None:
        self.register_feature("consumer")
        catalog = self.register_catalog("full-step", ("consumer",))
        self.assertTrue(self.activate_rust(catalog)["ok"])
        self.bridge.step_value = {
            **cast(dict[str, object], self.bridge.step_value),
            "pending_input": 128,
            "pending_inbound": 8192,
            "pending_preparation": 4096,
        }
        self.host._process(0.01)
        self.assertEqual(self.bridge.count("pull_typed_frame"), 1)
        self.assertEqual(len(self.active_feature("consumer").frames), 1)

    def test_step_report_is_validated_before_pull(self) -> None:
        self.register_feature("consumer")
        catalog = self.register_catalog("strict-step", ("consumer",))
        self.assertTrue(self.activate_rust(catalog)["ok"])
        self.host._process(0.01)
        first_frame = self.host._rust_frame
        report = cast(dict[str, object], self.bridge.step_value)
        for bad in (
            {"messages": 0, "meshes": 0},
            {**report, "epoch": "6"},
            {**report, "processed_messages": True},
            {**report, "pending_input": -1},
            {**report, "terminal": {"tag": "Unknown", "value": None}},
        ):
            self.bridge.step_value = bad
            self.host._process(0.01)
            self.assertEqual(self.bridge.count("pull_typed_frame"), 1)
            self.assert_retains_last_frame_and_resources(("consumer",), first_frame)

    def test_symbolic_audio_cues_and_lifecycle_families_resolve(self) -> None:
        self.register_feature("cues", families=("audio-cues@1.0",))
        self.register_feature("cycle", depends=("cues",), families=("lifecycle@1.0",))
        catalog = self.register_catalog("symbolic", ("cues", "cycle"))
        result = self.activate_rust(catalog)
        self.assertTrue(result["ok"], result)
        self.assertEqual(result["order"], ["cues", "cycle"])
        self.host._process(1 / 60)
        self.assertEqual(len(self.active_feature("cycle").frames), 1)

    def test_duplicate_table_descriptor_fails_activation(self) -> None:
        self.bridge.table_value = {
            "producer": "rust-client-core",
            "descriptors": [
                {
                    "logical_name": "session",
                    "numeric_id": 1,
                    "major": 1,
                    "minor": 0,
                    "record_limit": "4096",
                    "record_bytes": "0",
                },
                {
                    "logical_name": "session",
                    "numeric_id": 2,
                    "major": 1,
                    "minor": 0,
                    "record_limit": "4096",
                    "record_bytes": "0",
                },
            ],
        }
        self.register_feature("solo")
        catalog = self.register_catalog("duplicate", ("solo",))
        result = self.activate_rust(catalog)
        self.assertFalse(result["ok"])
        self.assertIn("family session twice", " ".join(result["errors"]))
        self.assertEqual(self.host.active_count(), 0)
        self.assertEqual(self.bridge.count("close"), 1)

    def test_unknown_family_requirement_fails_activation(self) -> None:
        self.register_feature("solo", families=("not-a-family@1.0",))
        catalog = self.register_catalog("unknown", ("solo",))
        result = self.activate_rust(catalog)
        self.assertFalse(result["ok"])
        self.assertIn("requires unavailable bridge family not-a-family", " ".join(result["errors"]))

    def test_version_and_major_mismatch_requirements_fail_activation(self) -> None:
        self.register_feature("newer", families=("session@2.0",))
        self.register_feature("older-major", families=("terrain@0.9",))
        catalog = self.register_catalog("versions", ("newer", "older-major"))
        result = self.activate_rust(catalog)
        self.assertFalse(result["ok"])
        joined = " ".join(result["errors"])
        self.assertIn("requires newer bridge family session@2.0", joined)
        self.assertIn("requires newer bridge family terrain@0.9", joined)

    def test_refused_open_connect_and_table_fail_activation_cleanly(self) -> None:
        for refused, expected, closes in (
            ("open_core", "refused to open", 0),
            ("connect", "refused the connection", 1),
            ("family_table", "family table", 1),
        ):
            with self.subTest(refused=refused):
                self.setUp()
                self.register_feature("solo")
                catalog = self.register_catalog(f"refusal-{refused}", ("solo",))
                self.bridge.refused.add(refused)
                result = self.activate_rust(catalog)
                self.assertFalse(result["ok"])
                self.assertIn(expected, " ".join(result["errors"]))
                self.assertEqual(self.host.active_count(), 0)
                # Whatever token survived a late refusal was released.
                self.assertEqual(self.bridge.count("close"), closes)

    # ------------------------------------------------------------------
    # Catalog planning and activation order
    # ------------------------------------------------------------------

    def test_dependency_cycle_fails_without_instances(self) -> None:
        self.register_feature("alpha", depends=("beta",))
        self.register_feature("beta", depends=("alpha",))
        catalog = self.register_catalog("cycle", ("alpha", "beta"))
        result = self.activate_rust(catalog)
        self.assertFalse(result["ok"])
        self.assertIn("feature dependency cycle", " ".join(result["errors"]))
        self.assertEqual(self.host.active_count(), 0)

    def test_provider_activates_before_consumer_and_frame_follows_order(self) -> None:
        self.register_feature("provider")
        self.register_feature("consumer", depends=("provider",))
        catalog = self.register_catalog("order", ("provider", "consumer"))
        self.assertTrue(self.activate_rust(catalog)["ok"])
        trace = json.loads(self.host.trace_json())
        self.assertLess(trace.index("activate:provider:5"), trace.index("activate:consumer:5"))
        self.host._process(1 / 60)
        provider_at = self.events.index("feature[provider]:apply_typed_frame")
        consumer_at = self.events.index("feature[consumer]:apply_typed_frame")
        self.assertLess(provider_at, consumer_at)

    def test_required_failure_rolls_back_the_exact_reverse_trace(self) -> None:
        self.register_feature("base")
        self.register_feature("middle", depends=("base",))
        self.register_feature("top", depends=("middle",), activate_failure="boom")
        catalog = self.register_catalog("rollback", ("base", "middle", "top"))
        result = self.activate_rust(catalog)
        self.assertFalse(result["ok"])
        self.assertIn("feature top activate failed: boom", " ".join(result["errors"]))
        trace = json.loads(self.host.trace_json())
        expected = [
            "instantiate:base",
            "validate:base",
            "bind:base",
            "activate:base:5",
            "instantiate:middle",
            "validate:middle",
            "bind:middle",
            "activate:middle:5",
            "instantiate:top",
            "validate:top",
            "bind:top",
            # The activate marker is traced before the feature call, so the
            # failed activation of the top feature still leaves its marker.
            "activate:top:5",
            "deactivate:middle",
            "deactivate:base",
        ]
        self.assertEqual(trace, expected)
        # Consumers release before providers and before the core token.
        self.assertLess(
            self.events.index("feature[middle]:deactivate_feature"),
            self.events.index("feature[base]:deactivate_feature"),
        )
        self.assertLess(
            self.events.index("feature[base]:deactivate_feature"), self.events.index("bridge:close")
        )
        self.assertEqual(self.host.active_count(), 0)
        self.assertEqual(self.bridge.count("close"), 1)
        self.assertIsNone(self.host._rust_token)

    def test_optional_failure_disables_dependents_with_reason(self) -> None:
        self.register_feature("provider", required=False, activate_failure="nope")
        self.register_feature("dependent", depends=("provider",), required=False)
        catalog = self.register_catalog("optional", ("provider", "dependent"))
        result = self.activate_rust(catalog)
        self.assertTrue(result["ok"], result)
        self.assertEqual(result["disabled"], ["dependent", "provider"])
        # The additive reason mapping names why each optional feature was
        # disabled, without moving anything into the required-error list.
        self.assertEqual(
            result["disabled_reasons"],
            {
                "provider": "feature provider activate failed: nope",
                "dependent": (
                    "feature dependent cannot activate after dependency provider was disabled"
                ),
            },
        )
        self.assertEqual(self.host.active_count(), 0)
        # The optional feature stays absent from both the active set and the
        # required-failure error list: isolation, not a required rollback.
        self.assertEqual(result["errors"], [])

    # ------------------------------------------------------------------
    # Reset and close
    # ------------------------------------------------------------------

    def test_reset_adopts_the_fresh_epoch_and_rejects_stale_frames(self) -> None:
        self.register_feature("solo")
        catalog = self.register_catalog("reset", ("solo",))
        self.assertTrue(self.activate_rust(catalog)["ok"])
        self.host._process(1 / 60)
        self.assertEqual(len(self.active_feature("solo").frames), 1)
        self.host.reset_features(6)
        self.assertEqual(self.bridge.count("reset"), 1)
        self.assertEqual(self.host.rust_session_epoch(), "9")
        self.assertEqual(self.active_feature("solo").method_names().count("reset_feature"), 1)
        # A late frame of the retired epoch is a stale callback: no apply.
        self.bridge.frame_value = frame_value(epoch="7")
        self.host._process(1 / 60)
        self.assertEqual(len(self.active_feature("solo").frames), 1)
        self.assertIsNone(self.host._rust_frame)
        # The fresh epoch applies again.
        self.bridge.step_value = {**cast(dict[str, object], self.bridge.step_value), "epoch": "9"}
        self.bridge.frame_value = frame_value(epoch="9")
        self.host._process(1 / 60)
        self.assertEqual(len(self.active_feature("solo").frames), 2)

    def test_retained_owned_frame_survives_producer_mutation_reset_and_close(self) -> None:
        self.register_feature("solo")
        catalog = self.register_catalog("owned", ("solo",))
        self.assertTrue(self.activate_rust(catalog)["ok"])
        self.host._process(1 / 60)
        retained = self.active_feature("solo").frames[0]
        owned = self.host._rust_frame
        cast(dict[str, object], self.bridge.frame_value)["frame_index"] = "2"
        self.host._process(1 / 60)
        self.assertEqual(plain_value(retained)["frame_index"], "1")
        self.assertEqual(owned["frame_index"], "1")
        self.host.reset_features(6)
        self.host.deactivate_features()
        self.assertEqual(plain_value(retained)["session_epoch"], "7")
        self.assertEqual(owned["session_epoch"], "7")

    def test_activation_rollback_releases_the_bridge_after_confirmed_close(self) -> None:
        self.register_feature("solo")
        catalog = self.register_catalog("rollback-owner", ("solo",))
        self.bridge.refused.add("connect")
        self.assertFalse(self.activate_rust(catalog)["ok"])
        self.assertIsNone(self.host._bridge)
        self.assertIsNone(self.host._rust_token)
        self.assertFalse(self.host._rust_retiring)
        self.host._process(1 / 60)
        self.assertEqual(self.bridge.count("step"), 0)

    def test_malformed_or_throwing_close_retains_token_for_retry(self) -> None:
        for result in (None, {"ok": True, "value": 1, "error": None}, {"ok": True}, "torn"):
            with self.subTest(result=result):
                self.setUp()
                self.register_feature("solo")
                catalog = self.register_catalog("malformed-close", ("solo",))
                self.assertTrue(self.activate_rust(catalog)["ok"])
                token = self.host._rust_token
                original = self.bridge.call

                def malformed_close(
                    method: str,
                    *args: object,
                    close_result: object = result,
                    bridge_call: Callable[..., object] = original,
                ) -> object:
                    return (
                        native_value(close_result)
                        if method == "close"
                        else bridge_call(method, *args)
                    )

                with patch.object(self.bridge, "call", side_effect=malformed_close):
                    self.host.deactivate_features()
                self.assertIs(self.host._rust_token, token)
                self.host.deactivate_features()
                self.assertIsNone(self.host._rust_token)
        self.setUp()
        self.register_feature("solo")
        catalog = self.register_catalog("throwing-close", ("solo",))
        self.assertTrue(self.activate_rust(catalog)["ok"])
        token = self.host._rust_token
        with patch.object(self.bridge, "call", side_effect=RuntimeError("native refusal")):
            self.host.deactivate_features()
        self.assertIs(self.host._rust_token, token)
        self.host.deactivate_features()
        self.assertIsNone(self.host._rust_token)

    def test_refused_reset_keeps_the_epoch_and_skips_feature_resets(self) -> None:
        self.register_feature("solo")
        catalog = self.register_catalog("resetfail", ("solo",))
        self.assertTrue(self.activate_rust(catalog)["ok"])
        self.bridge.refused.add("reset")
        self.host.reset_features(6)
        self.assertEqual(self.host.rust_session_epoch(), "7")
        self.assertNotIn("reset_feature", self.active_feature("solo").method_names())

    def test_close_releases_consumers_first_and_exactly_once(self) -> None:
        self.register_feature("provider")
        self.register_feature("consumer", depends=("provider",))
        catalog = self.register_catalog("close", ("provider", "consumer"))
        self.assertTrue(self.activate_rust(catalog)["ok"])
        self.host._process(1 / 60)
        provider = self.active_feature("provider")
        consumer = self.active_feature("consumer")

        def assert_released() -> None:
            self.assertIsNone(self.host._rust_frame)
            self.assertEqual(provider.frames, [])
            self.assertEqual(consumer.frames, [])

        self.bridge.before_close = assert_released
        self.host.deactivate_features()
        self.assertEqual(self.bridge.count("close"), 1)
        self.assertLess(
            self.events.index("feature[consumer]:deactivate_feature"),
            self.events.index("feature[provider]:deactivate_feature"),
        )
        self.assertLess(
            self.events.index("feature[provider]:deactivate_feature"),
            self.events.index("bridge:close"),
        )
        self.assertEqual(self.host.active_count(), 0)
        self.assertIsNone(self.host._rust_token)
        # A repeated teardown performs no second native release.
        self.host.deactivate_features()
        self.assertEqual(self.bridge.count("close"), 1)
        # Ticks after close are stale callbacks: no step, no pull, no apply.
        steps = self.bridge.count("step")
        self.host._process(1 / 60)
        self.assertEqual(self.bridge.count("step"), steps)
        self.assertEqual(len(provider.frames), 0)

    def test_refused_close_retains_retiring_owner_until_explicit_retry(self) -> None:
        self.register_feature("solo")
        catalog = self.register_catalog("closerefused", ("solo",))
        self.assertTrue(self.activate_rust(catalog)["ok"])
        self.host._process(1 / 60)
        steps = self.bridge.count("step")
        token = self.host._rust_token
        self.bridge.refused.add("close")
        self.host.deactivate_features()
        self.assertEqual(self.bridge.count("close"), 1)
        self.assertIs(self.host._rust_token, token)
        self.assertIs(self.host._bridge, self.bridge)
        self.assertIsNone(self.host._rust_frame)
        self.assertEqual(self.host.active_count(), 0)
        self.host._process(1 / 60)
        self.host.reset_features(6)
        self.assertEqual(self.bridge.count("step"), steps)
        self.assertEqual(self.bridge.count("reset"), 0)
        self.assertFalse(self.activate_rust(catalog)["ok"])
        pilot = json.loads(self.host.activate_catalog(catalog, "../ClientBridge", 6))
        self.assertFalse(pilot["ok"])
        self.assertEqual(self.bridge.count("open_core"), 1)
        self.assertEqual(self.bridge.count("close"), 1)
        self.bridge.refused.remove("close")
        self.host.deactivate_features()
        self.assertEqual(self.bridge.count("close"), 2)
        self.assertIsNone(self.host._rust_token)
        self.assertIsNone(self.host._bridge)
        self.host.deactivate_features()
        self.assertEqual(self.bridge.count("close"), 2)

    # ------------------------------------------------------------------
    # The pilot path stays an explicit, unaliased complete path
    # ------------------------------------------------------------------

    def test_pilot_activation_still_drives_feature_owned_sessions(self) -> None:
        self.register_feature("solo", families=("2@1.0",))
        catalog = self.register_catalog("pilot", ("solo",))
        raw = self.host.activate_catalog(catalog, "../ClientBridge", 3)
        result = json.loads(raw)
        self.assertTrue(result["ok"], result)
        self.host._process(1 / 60)
        names = self.active_feature("solo").method_names()
        self.assertIn("drive_session", names)
        self.assertIn("pull_typed_frame", names)
        self.assertIn("apply_typed_frame", names)
        # The pilot tick never touches the rust facade.
        self.assertEqual(self.bridge.count("step"), 0)
        self.assertEqual(self.bridge.count("pull_typed_frame"), 0)
        self.assertEqual(self.host.rust_session_epoch(), "")

    def test_rust_mode_requires_the_bridge_rust_facade(self) -> None:
        pilot_only = PilotOnlyBridge("PilotBridge", self.events)
        self.root.add_child(pilot_only)
        self.register_feature("solo")
        catalog = self.register_catalog("nofacade", ("solo",))
        raw = self.host.activate_rust_catalog(
            catalog,
            "../PilotBridge",
            5,
            CONFIG_JSON,
            ENDPOINT_JSON,
            IDENTITY_JSON,
        )
        result = json.loads(raw)
        self.assertFalse(result["ok"])
        self.assertIn("missing the rust facade method", " ".join(result["errors"]))
        self.assertEqual(self.host.active_count(), 0)


class EchoBridge(StubNode):
    """Echo native values to exercise the actual host conversion seam."""

    def __init__(self) -> None:
        super().__init__("Echo")
        self.calls = 0
        self.result: object = None

    def call(self, method: str, *args: object) -> object:
        for value in args:
            assert_native_argument(value)
        self.calls += 1
        return args[0] if args else self.result


class NativeConversionTest(unittest.TestCase):
    def setUp(self) -> None:
        self.bridge = EchoBridge()

    def test_owned_copy_preserves_exact_scalar_and_union_values(self) -> None:
        original = {
            "null": None,
            "bool": True,
            "int": -17,
            "float": -0.0,
            "text": "é🌿",
            "u64": "18446744073709551615",
            "hex": "00000000000040008000000000000001",
            "union": {"tag": "Known", "value": 0},
            "array": [None, False, 1.5],
        }
        copied = HOST._call_rust(self.bridge, "echo", original)
        self.assertEqual(copied, original)
        self.assertIsNot(copied, original)
        self.assertIsNot(copied["array"], original["array"])
        for key in ("null", "bool", "int", "float", "text"):
            self.assertIs(type(copied[key]), type(original[key]))
        self.assertEqual(str(copied["float"]), "-0.0")

    def test_depth_and_cycles_are_refused_without_dispatch(self) -> None:
        admitted: object = None
        for _ in range(64):
            admitted = [admitted]
        self.assertEqual(HOST._call_rust(self.bridge, "echo", admitted), admitted)
        calls = self.bridge.calls
        cycle: list[object] = []
        cycle.append(cycle)
        dictionary: dict[str, object] = {}
        dictionary["cycle"] = dictionary
        for value in ([admitted], cycle, dictionary, {1: "not-a-string"}, (1,), object()):
            with self.subTest(kind=type(value).__name__):
                self.assertIsNone(HOST._call_rust(self.bridge, "echo", value))
                self.assertEqual(self.bridge.calls, calls)
        native_cycle = NativeArray.new0()
        native_cycle.append(native_cycle)
        self.bridge.result = native_cycle
        self.assertIsNone(HOST._call_rust(self.bridge, "result"))

    def test_oversized_native_containers_are_refused_before_reading(self) -> None:
        class OversizedArray(NativeArray):
            def size(self) -> int:
                return (8 << 20) + 1

            def get(self, index: int) -> object:
                raise AssertionError("oversized array was traversed")

        class OversizedDictionary(NativeDictionary):
            def size(self) -> int:
                return (8 << 20) + 1

            def keys(self) -> NativeArray:
                raise AssertionError("oversized dictionary was traversed")

        for value in (OversizedArray(), OversizedDictionary()):
            self.bridge.result = value
            self.assertIsNone(HOST._call_rust(self.bridge, "result"))

    def test_node_and_utf8_totals_are_bounded_across_arguments_and_keys(self) -> None:
        with patch.object(HOST, "_RUST_VALUE_NODES", 8):
            self.assertEqual(HOST._call_rust(self.bridge, "echo", [None] * 7), [None] * 7)
            self.assertIsNone(HOST._call_rust(self.bridge, "echo", [None] * 8))
            self.assertIsNone(HOST._call_rust(self.bridge, "echo", [None] * 4, [None] * 4))
        with patch.object(HOST, "_RUST_VALUE_TEXT_BYTES", 8):
            self.assertEqual(HOST._call_rust(self.bridge, "echo", "é" * 4), "é" * 4)
            self.assertIsNone(HOST._call_rust(self.bridge, "echo", "é" * 5))
            self.assertIsNone(HOST._call_rust(self.bridge, "echo", {"abcde": "éé"}))
            self.assertIsNone(HOST._call_rust(self.bridge, "echo", ["abcde", "fghi"]))
            self.bridge.result = native_value(["abcde", "fghi"])
            self.assertIsNone(HOST._call_rust(self.bridge, "result"))

    def test_malformed_native_sizes_keys_and_scalars_are_refused(self) -> None:
        class BadSizeArray(NativeArray):
            def size(self) -> int:
                return -1

        class TornDictionary(NativeDictionary):
            def size(self) -> int:
                return 1

        native_key = NativeDictionary.new0()
        native_key.set(1, "invalid")
        for value in (BadSizeArray(), TornDictionary(), native_key, float("nan")):
            self.bridge.result = value
            self.assertIsNone(HOST._call_rust(self.bridge, "result"))


class PilotOnlyBridge(ScriptedBridge):
    """A bridge without the rust facade, like the pilot-era node."""

    def has_method(self, name: str) -> bool:
        return name in ("feature_families_json", "session_create", "session_close")


if __name__ == "__main__":
    unittest.main()
