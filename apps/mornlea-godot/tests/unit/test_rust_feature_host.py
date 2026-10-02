"""Engine-free unit harness for the host's rust-core session ownership.

The harness imports the actual production ``feature_host`` module and runs
its real activation, dispatch, and teardown logic. Only the py4godot module
surface (``gdclass``, the ``Node``/``Object``/resource shells), fake feature
resources, and one scripted rust-facade boundary are substituted: no host
dispatch or rollback behavior is reimplemented here. Nothing imports the
Godot engine or launches a window.
"""

from __future__ import annotations

import importlib.util
import json
import sys
import types
import unittest
from pathlib import Path
from typing import Any

PROJECT_ROOT = Path(__file__).resolve().parents[2]
HOST_MODULE_PATH = PROJECT_ROOT / "app" / "host" / "feature_host.py"

# The scripted producer family table mirrors the frozen rust-client-core
# descriptors: ten logical families, numeric IDs 1..10, every version 1.0.
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
            self.frames.append(args[0])
            return ""
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
        self.step_value: object = {"messages": 0, "meshes": 0}
        self.frame_value: object = frame_value()

    def has_method(self, name: str) -> bool:
        return True

    def call(self, method: str, *args: object) -> object:
        self.calls.append((method, args))
        self._events.append(f"bridge:{method}")
        if method in self.refused:
            return error_envelope()
        if method == "open_core":
            return ok_envelope(self.open_value)
        if method == "connect":
            return ok_envelope(self.connect_value)
        if method == "reset":
            return ok_envelope(self.reset_value)
        if method == "family_table":
            return ok_envelope(self.table_value)
        if method == "step":
            return ok_envelope(self.step_value)
        if method == "pull_typed_frame":
            return ok_envelope(self.frame_value)
        if method == "close":
            return ok_envelope(None)
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

    classes.gdclass = gdclass
    node_module = types.ModuleType("py4godot.classes.Node")
    node_module.Node = StubNode
    object_module = types.ModuleType("py4godot.classes.Object")
    object_module.Object = StubNode
    scene_module = types.ModuleType("py4godot.classes.PackedScene")
    scene_module.PackedScene = StubPackedScene
    resource_module = types.ModuleType("py4godot.classes.Resource")
    resource_module.Resource = StubResource
    loader_module = types.ModuleType("py4godot.classes.ResourceLoader")
    loader_module.ResourceLoader = StubResourceLoader
    utils = types.ModuleType("py4godot.utils")
    smart_cast = types.ModuleType("py4godot.utils.smart_cast")

    def register_cast_function(name: str, function: object) -> None:
        CAST_REGISTRATIONS[name] = function

    smart_cast.register_cast_function = register_cast_function
    py4godot.classes = classes
    utils.smart_cast = smart_cast
    for name, module in {
        "py4godot": py4godot,
        "py4godot.classes": classes,
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

    def activate_rust(
        self, catalog_path: str, epoch: int = 5
    ) -> dict[str, Any]:
        raw = self.host.activate_rust_catalog(
            catalog_path,
            "../ClientBridge",
            epoch,
            CONFIG_JSON,
            ENDPOINT_JSON,
            IDENTITY_JSON,
        )
        return json.loads(raw)

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
        self.assertIs(provider.frames[0], self.bridge.frame_value)

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
                self.assertIs(arguments[0], self.bridge.open_value)
        step_arguments = self.bridge.arguments_of("step")[0]
        self.assertEqual(step_arguments[1], "7")
        self.assertEqual(step_arguments[2], {"messages": 64, "meshes": 32})
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
                {"logical_name": "session", "numeric_id": 1, "major": 1, "minor": 0,
                 "record_limit": "4096", "record_bytes": "0"},
                {"logical_name": "session", "numeric_id": 2, "major": 1, "minor": 0,
                 "record_limit": "4096", "record_bytes": "0"},
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
        self.assertLess(self.events.index("feature[middle]:deactivate_feature"),
                        self.events.index("feature[base]:deactivate_feature"))
        self.assertLess(self.events.index("feature[base]:deactivate_feature"),
                        self.events.index("bridge:close"))
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
        self.bridge.frame_value = frame_value(epoch="9")
        self.host._process(1 / 60)
        self.assertEqual(len(self.active_feature("solo").frames), 2)

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
        self.assertEqual(len(provider.frames), 1)

    def test_refused_close_still_drops_the_token_and_kills_late_ticks(self) -> None:
        self.register_feature("solo")
        catalog = self.register_catalog("closerefused", ("solo",))
        self.assertTrue(self.activate_rust(catalog)["ok"])
        self.host._process(1 / 60)
        steps = self.bridge.count("step")
        self.bridge.refused.add("close")
        self.host.deactivate_features()
        # A refused native release still ends the host's ownership: exactly
        # one release attempt was made and the token is dropped, so a
        # released token can never be replayed as if it were live.
        self.assertEqual(self.bridge.count("close"), 1)
        self.assertIsNone(self.host._rust_token)
        self.assertEqual(self.host.active_count(), 0)
        # Late ticks after the refused close are stale callbacks.
        self.host._process(1 / 60)
        self.assertEqual(self.bridge.count("step"), steps)
        # The dropped token means a repeated teardown makes no second
        # release attempt.
        self.host.deactivate_features()
        self.assertEqual(self.bridge.count("close"), 1)

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


class PilotOnlyBridge(ScriptedBridge):
    """A bridge without the rust facade, like the pilot-era node."""

    def has_method(self, name: str) -> bool:
        return name in ("feature_families_json", "session_create", "session_close")


if __name__ == "__main__":
    unittest.main()
