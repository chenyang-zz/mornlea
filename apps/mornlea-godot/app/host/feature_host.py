"""Assemble coarse Godot features without importing project Python packages.

Godot resource paths are the discovery boundary. The embedded interpreter is
deliberately isolated from the repository root, so this host must remain
independent of ambient ``sys.path`` entries and sibling-module imports.
"""

from __future__ import annotations

import json
import sys
import time
from typing import Protocol, cast, runtime_checkable

from py4godot.classes import gdclass
from py4godot.classes.Node import Node
from py4godot.classes.Object import Object
from py4godot.classes.PackedScene import PackedScene
from py4godot.classes.Resource import Resource
from py4godot.classes.ResourceLoader import ResourceLoader
from py4godot.utils.smart_cast import (  # type: ignore[import-not-found]
    register_cast_function,
)

# The pinned Py4Godot runtime cannot marshal a project-native-class object
# across script-module method boundaries (the callee receives a bare `Object`
# without its pointer), and patching that runtime is out of scope. The host and
# every feature therefore acquire the native bridge node through `get_node`
# with this identity cast, which is the established bridge acquisition pattern;
# only scene paths cross module boundaries.
register_cast_function("MornleaClientBridge", lambda bridge: bridge)

HOST_PROTOCOL_MAJOR = 1
HOST_PROTOCOL_MINOR = 0
BUDGET_CLASSES = frozenset({"bootstrap", "light", "standard", "heavy"})
RESET_POLICIES = frozenset({"recreate", "reset"})

# The rust-producer facade the host drives in rust mode. `connect` is named
# exactly as the facade contract spells it even though it shadows the engine
# Node signal API: the host reaches it only through dynamic `call`, never
# through the signal entry, so the two surfaces cannot alias.
RUST_FACADE_METHODS = (
    "open_core",
    "connect",
    "family_table",
    "step",
    "pull_typed_frame",
    "reset",
    "close",
)
RUST_PRODUCER_NAME = "rust-client-core"
RUST_STEP_MESSAGE_BUDGET = 64
RUST_STEP_MESH_BUDGET = 32
U64_MAX = 18_446_744_073_709_551_615


@runtime_checkable
class StringArrayView(Protocol):
    def size(self) -> int: ...

    def get(self, index: int) -> str: ...


@runtime_checkable
class _DictionaryView(Protocol):
    """Minimal typed view used by the host's one-frame fan-out seam."""

    def __getitem__(self, key: str) -> object: ...


class FeatureManifest:
    """Immutable-in-practice projection of one serialized feature resource."""

    def __init__(
        self,
        feature_id: str,
        host_protocol_major: int,
        host_protocol_minor: int,
        entry_scene_path: str,
        dependencies: tuple[str, ...],
        required_bridge_families: tuple[str, ...],
        required: bool,
        enabled: bool,
        budget_class: str,
        reset_policy: str,
    ) -> None:
        self.feature_id = feature_id
        self.host_protocol_major = host_protocol_major
        self.host_protocol_minor = host_protocol_minor
        self.entry_scene_path = entry_scene_path
        self.dependencies = dependencies
        self.required_bridge_families = required_bridge_families
        self.required = required
        self.enabled = enabled
        self.budget_class = budget_class
        self.reset_policy = reset_policy


class PlanResult:
    """Serializable result shared by planning and activation probes."""

    def __init__(
        self,
        ok: bool,
        errors: tuple[str, ...],
        disabled: tuple[str, ...],
        order: tuple[str, ...],
    ) -> None:
        self.ok = ok
        self.errors = errors
        self.disabled = disabled
        self.order = order

    def to_json(self) -> str:
        return json.dumps(
            {
                "ok": self.ok,
                "errors": list(self.errors),
                "disabled": list(self.disabled),
                "order": list(self.order),
            },
            separators=(",", ":"),
            sort_keys=True,
        )


def _allocated_blocks() -> int:
    getter = getattr(sys, "getallocatedblocks", None)
    if callable(getter):
        count = getter()
        if isinstance(count, int) and not isinstance(count, bool) and count >= 0:
            return count
    return 0


def _u64_text(value: object) -> int | None:
    """Parse one canonical decimal u64 text, or refuse it.

    The facade bridges every unsigned 64-bit value through exactly this
    spelling (digits only, no sign, no leading zero, at most twenty digits),
    so any other shape is a torn or hostile result, never a usable number.
    """
    if not isinstance(value, str) or not value or len(value) > 20:
        return None
    if any(character not in "0123456789" for character in value):
        return None
    if len(value) > 1 and value[0] == "0":
        return None
    number = int(value)
    return number if number <= U64_MAX else None


def _u16_word(value: object) -> int | None:
    if not isinstance(value, int) or isinstance(value, bool):
        return None
    return value if 0 <= value <= 65_535 else None


def _rust_error_class(result: object) -> str:
    """The closed error class name of one facade envelope, for diagnostics."""
    if isinstance(result, dict):
        error = result.get("error")
        if isinstance(error, dict):
            error_class = error.get("class")
            if isinstance(error_class, str) and error_class:
                return error_class
    return "UnknownError"


def _rust_envelope_value(result: object) -> object:
    """The owned value of one closed facade envelope, or None on any refusal.

    A success envelope is exactly `ok` true with a null error; every other
    shape — a failed call, a torn envelope, or a non-dictionary — means "no
    result", and the caller keeps its prior visible state instead of
    inventing a default value.
    """
    if not isinstance(result, dict) or result.get("ok") is not True:
        return None
    return result.get("value")


def _rust_token_from(result: object) -> dict | None:
    """One validated core token, passed to the bridge verbatim thereafter.

    The token is an opaque owned value: the host checks only that it is the
    facade-issued `{slot, generation}` record and then returns the same
    object on every call, never rebuilding or mutating it.
    """
    token = _rust_envelope_value(result)
    if not isinstance(token, dict):
        return None
    slot = token.get("slot")
    if not isinstance(slot, int) or isinstance(slot, bool) or slot <= 0:
        return None
    if _u64_text(token.get("generation")) is None:
        return None
    return token


def _rust_epoch_text_from(result: object) -> str | None:
    """One canonical session-epoch text answered by connect or reset."""
    epoch = _rust_envelope_value(result)
    if not isinstance(epoch, str) or _u64_text(epoch) is None:
        return None
    return epoch


@gdclass
class feature_host(Node):
    """Own the bounded feature lifecycle while remaining feature agnostic.

    The host binds the scene-provided native bridge node as its typed service:
    it acquires the object through `get_node` plus the identity cast above,
    negotiates capability from the bridge's typed family table, and hands the
    bridge's scene path to features so each acquires the same object itself.
    Session ownership is mode explicit: the pilot path leaves session methods
    to the features, while the rust path keeps the core token, session epoch,
    and the single step/pull/fan-out tick here so features only submit
    semantic input and apply output. No code here branches on concrete
    feature identities.
    """

    _instances: dict[str, Node]
    _active_order: list[str]
    _bridge: Node | None
    _bridge_path: str
    _trace: list[str]
    _last_process_ns: int
    _last_apply_ns: int
    _last_allocation_delta: int
    _rust_mode: bool
    _rust_token: dict | None
    _rust_epoch: str | None
    _rust_frame: dict | None
    _rust_families: dict[str, tuple[int, int]] | None
    _rust_required: tuple[tuple[str, int, int], ...]

    def _ready(self) -> None:
        self._instances = {}
        self._active_order = []
        self._bridge = None
        self._bridge_path = ""
        self._trace = []
        self._last_process_ns = 0
        self._last_apply_ns = 0
        self._last_allocation_delta = 0
        # Rust-mode session ownership starts absent: the pilot path stays the
        # default and only an explicit rust activation arms this state.
        self._rust_mode = False
        self._rust_token = None
        self._rust_epoch = None
        self._rust_frame = None
        self._rust_families = None
        self._rust_required = ()

    def _exit_tree(self) -> None:
        _deactivate(self)

    def _process(self, delta: float) -> None:
        """Run one ordered bounded step and fan out one immutable typed frame."""
        started = time.perf_counter_ns()
        allocated_before = _allocated_blocks()
        apply_ns = 0
        try:
            if self._bridge is None:
                return
            if self._rust_mode:
                # Rust mode is an explicit complete path: the host owns the
                # core token and session epoch, so no feature drives the
                # session here — features only receive `apply_typed_frame`.
                apply_ns = _rust_dispatch(self)
                return
            for feature_id in self._active_order:
                instance = self._instances.get(feature_id)
                if instance is not None and instance.has_method("drive_input"):
                    instance.call("drive_input")
            elapsed_ns = max(0, min(int(delta * 1_000_000_000), 100_000_000))
            stepped = False
            for feature_id in self._active_order:
                instance = self._instances.get(feature_id)
                if instance is None or not instance.has_method("drive_session"):
                    continue
                status = instance.call("drive_session", elapsed_ns, 64, 32)
                if not isinstance(status, int) or isinstance(status, bool) or status != 0:
                    return
                stepped = True
            if not stepped:
                return
            for feature_id in self._active_order:
                instance = self._instances.get(feature_id)
                if instance is not None and instance.has_method("drive_world"):
                    instance.call("drive_world")
            typed: _DictionaryView | None = None
            for feature_id in self._active_order:
                instance = self._instances.get(feature_id)
                if instance is None or not instance.has_method("pull_typed_frame"):
                    continue
                candidate = instance.call("pull_typed_frame")
                if isinstance(candidate, _DictionaryView):
                    typed = candidate
                    break
            if typed is None:
                return
            apply_started = time.perf_counter_ns()
            for feature_id in self._active_order:
                instance = self._instances.get(feature_id)
                if instance is not None and instance.has_method("apply_typed_frame"):
                    instance.call("apply_typed_frame", typed)
            apply_ns = time.perf_counter_ns() - apply_started
        finally:
            # Recording-only host cost for the P7 report: interpreter apply
            # duration and allocation pressure stay visible instead of hiding
            # inside aggregate frame time. These counters never gate work.
            self._last_process_ns = time.perf_counter_ns() - started
            self._last_apply_ns = apply_ns
            self._last_allocation_delta = max(0, _allocated_blocks() - allocated_before)

    def last_process_ns(self) -> int:
        return self._last_process_ns

    def last_apply_ns(self) -> int:
        return self._last_apply_ns

    def last_allocation_delta(self) -> int:
        return self._last_allocation_delta

    def plan_catalog(self, catalog_path: str, bridge_path: str) -> str:
        bridge = self.get_node(bridge_path)
        if bridge is None:
            return _missing_bridge_result(bridge_path).to_json()
        result, _ = _build_plan(catalog_path, bridge)
        return result.to_json()

    def activate_catalog(self, catalog_path: str, bridge_path: str, epoch: int) -> str:
        bridge = self.get_node(bridge_path)
        if bridge is None:
            return _missing_bridge_result(bridge_path).to_json()
        return _activate(self, catalog_path, bridge, epoch).to_json()

    def activate_rust_catalog(
        self,
        catalog_path: str,
        bridge_path: str,
        epoch: int,
        config_json: str,
        endpoint_json: str,
        identity_json: str,
    ) -> str:
        """Activate the explicit rust-core path with a host-owned session.

        The pilot `activate_catalog` remains the default complete path; this
        entry never silently substitutes it. The host opens the core, begins
        the connection and negotiates the producer family table itself, then
        instantiates features that only submit semantic input and apply the
        fanned-out frame. Configuration, endpoint, and identity cross as JSON
        text because script modules exchange strings, not live objects.
        """
        bridge = self.get_node(bridge_path)
        if bridge is None:
            return _missing_bridge_result(bridge_path).to_json()
        decoded: list[object] = []
        errors: list[str] = []
        for name, text in (
            ("configuration", config_json),
            ("endpoint", endpoint_json),
            ("identity", identity_json),
        ):
            value, error = _decoded_json(text)
            if error:
                errors.append(f"the rust {name} argument is not valid JSON")
            decoded.append(value)
        if errors:
            return PlanResult(False, tuple(errors), (), ()).to_json()
        return (
            _activate_rust(
                self,
                catalog_path,
                bridge,
                epoch,
                decoded[0],
                decoded[1],
                decoded[2],
            )
            .to_json()
        )

    def rust_session_epoch(self) -> str:
        """The current host-owned session epoch text, or an empty string."""
        return self._rust_epoch if self._rust_epoch is not None else ""

    def reset_features(self, epoch: int) -> None:
        if self._rust_mode:
            _reset_rust(self, epoch)
            return
        _reset(self, epoch)

    def deactivate_features(self) -> None:
        _deactivate(self)

    def active_count(self) -> int:
        return len(self._active_order)

    def trace_json(self) -> str:
        return json.dumps(self._trace, separators=(",", ":"))


def _missing_bridge_result(bridge_path: str) -> PlanResult:
    return PlanResult(False, (f"bridge node is missing at {bridge_path}",), (), ())


def _metadata(resource: Object, name: str, default: object) -> object:
    if not resource.has_meta(name):
        return default
    return resource.get_meta(name, default)


def _metadata_strings(resource: Object, name: str) -> tuple[str, ...]:
    if not resource.has_meta(name):
        return ()
    value = resource.get_meta(name)
    if isinstance(value, StringArrayView):
        return tuple(value.get(index) for index in range(value.size()))
    if isinstance(value, str):
        return (value,)
    return ()


def _load_resource(path: str) -> Resource | None:
    loaded = ResourceLoader.instance().load(path)
    if loaded is None:
        return None
    return cast(Resource, loaded)


def _load_catalog(catalog_path: str) -> tuple[list[FeatureManifest], list[str]]:
    # Loading explicit resource paths keeps product assembly auditable and bounded.
    errors: list[str] = []
    catalog = _load_resource(catalog_path)
    if catalog is None:
        return [], [f"feature catalog is missing: {catalog_path}"]
    major = cast(int, _metadata(catalog, "host_protocol_major", 0))
    minor = cast(int, _metadata(catalog, "host_protocol_minor", 0))
    if major != HOST_PROTOCOL_MAJOR or minor > HOST_PROTOCOL_MINOR:
        errors.append(f"feature catalog has incompatible host protocol {major}.{minor}")
    manifest_paths = _metadata_strings(catalog, "manifest_paths")
    if not manifest_paths:
        errors.append("feature catalog has no manifest paths")
    manifests: list[FeatureManifest] = []
    for manifest_path in manifest_paths:
        manifest_resource = _load_resource(manifest_path)
        if manifest_resource is None:
            errors.append(f"feature manifest is missing: {manifest_path}")
            continue
        manifests.append(
            FeatureManifest(
                feature_id=cast(str, _metadata(manifest_resource, "feature_id", "")),
                host_protocol_major=cast(
                    int, _metadata(manifest_resource, "host_protocol_major", 0)
                ),
                host_protocol_minor=cast(
                    int, _metadata(manifest_resource, "host_protocol_minor", 0)
                ),
                entry_scene_path=cast(str, _metadata(manifest_resource, "entry_scene_path", "")),
                dependencies=_metadata_strings(manifest_resource, "dependencies"),
                required_bridge_families=_metadata_strings(
                    manifest_resource, "required_bridge_families"
                ),
                required=cast(bool, _metadata(manifest_resource, "required", True)),
                enabled=cast(bool, _metadata(manifest_resource, "enabled", True)),
                budget_class=cast(str, _metadata(manifest_resource, "budget_class", "standard")),
                reset_policy=cast(str, _metadata(manifest_resource, "reset_policy", "reset")),
            )
        )
    return manifests, errors


def _parse_version(value: str) -> tuple[int, int] | None:
    parts = value.split(".", 1)
    if len(parts) != 2 or not all(part.isdigit() for part in parts):
        return None
    return int(parts[0]), int(parts[1])


def _decoded_json(text: str) -> tuple[object, str]:
    try:
        return json.loads(text), ""
    except json.JSONDecodeError:
        return None, "invalid JSON"


def _compatible_version(actual: str, required_major: int, required_minor: int) -> bool:
    # Major versions must match; newer compatible minors may satisfy a requirement.
    parsed = _parse_version(actual)
    return parsed is not None and parsed[0] == required_major and parsed[1] >= required_minor


def _call_text(target: Object, method: str, *arguments: object) -> str:
    result = target.call(method, *arguments)
    return result if isinstance(result, str) else ""


class FamilyTable:
    """Parsed projection of a negotiated feature-family table.

    The pilot bridge reports its identity table keyed by the numeric registry
    family identifier as a string (for example ``"2@1.0"`` for the connection
    family, whose single contract version word projects as major with an
    implicit zero minor). The rust producer table is keyed by symbolic logical
    names instead (for example ``"audio-cues@1.0"``); both keyings feed the
    same requirement grammar, so the host keeps one negotiation core.
    """

    def __init__(self, versions: dict[str, tuple[int, int]]) -> None:
        self.versions = versions


def _bridge_family_table(bridge: Node) -> tuple[FamilyTable | None, str]:
    # The bridge identity is its negotiated capability table, reported as one
    # typed JSON value; the host never inspects identity records or packets.
    if not bridge.has_method("feature_families_json"):
        return None, "bridge is missing the feature family table identity"
    table_text = _call_text(bridge, "feature_families_json")
    try:
        entries = json.loads(table_text)
    except json.JSONDecodeError:
        return None, "bridge reported an unreadable feature family table"
    if not isinstance(entries, list):
        return None, "bridge reported an unreadable feature family table"
    versions: dict[str, tuple[int, int]] = {}
    for entry in entries:
        if not isinstance(entry, dict):
            return None, "bridge reported an unreadable feature family table"
        family = entry.get("family")
        version = entry.get("version")
        if (
            not isinstance(family, int)
            or not isinstance(version, int)
            or family <= 0
            or version <= 0
        ):
            return None, "bridge reported an invalid feature family table entry"
        versions[str(family)] = (version, 0)
    return FamilyTable(versions), ""


def _family_incompatibility(table: FamilyTable | None, family_spec: str) -> str:
    family_parts = family_spec.rsplit("@", 1)
    if len(family_parts) != 2:
        return f"has invalid bridge family requirement {family_spec}"
    required = _parse_version(family_parts[1])
    if required is None:
        return f"has invalid bridge family requirement {family_spec}"
    actual = table.versions.get(family_parts[0]) if table is not None else None
    if actual is None:
        return f"requires unavailable bridge family {family_parts[0]}"
    if not _compatible_version(f"{actual[0]}.{actual[1]}", required[0], required[1]):
        return f"requires newer bridge family {family_spec}"
    return ""


def _manifest_incompatibility(manifest: FeatureManifest, table: FamilyTable | None) -> str:
    if not manifest.enabled:
        return "is explicitly disabled"
    if (
        manifest.host_protocol_major != HOST_PROTOCOL_MAJOR
        or manifest.host_protocol_minor > HOST_PROTOCOL_MINOR
    ):
        return (
            "has incompatible host protocol "
            f"{manifest.host_protocol_major}.{manifest.host_protocol_minor}"
        )
    if not manifest.entry_scene_path.startswith("res://"):
        return "has no project-local entry scene"
    if manifest.budget_class not in BUDGET_CLASSES:
        return f"has unknown budget class {manifest.budget_class}"
    if manifest.reset_policy not in RESET_POLICIES:
        return f"has unknown reset policy {manifest.reset_policy}"
    for family_spec in manifest.required_bridge_families:
        incompatibility = _family_incompatibility(table, family_spec)
        if incompatibility:
            return incompatibility
    return ""


def _exclude_or_fail(
    manifest: FeatureManifest,
    reason: str,
    errors: list[str],
    disabled: list[str],
    excluded: set[str],
) -> None:
    if manifest.feature_id in excluded:
        return
    excluded.add(manifest.feature_id)
    if manifest.required:
        errors.append(f"required feature {manifest.feature_id} {reason}")
    else:
        disabled.append(manifest.feature_id)


def _visit_feature(
    feature_id: str,
    manifests: dict[str, FeatureManifest],
    excluded: set[str],
    states: dict[str, int],
    stack: tuple[str, ...],
    order: list[str],
    errors: list[str],
) -> None:
    state = states.get(feature_id, 0)
    if state == 2:
        return
    if state == 1:
        start = stack.index(feature_id)
        cycle = (*stack[start:], feature_id)
        errors.append(f"feature dependency cycle: {' -> '.join(cycle)}")
        return
    states[feature_id] = 1
    next_stack = (*stack, feature_id)
    for dependency in sorted(manifests[feature_id].dependencies):
        if dependency in manifests and dependency not in excluded:
            _visit_feature(
                dependency,
                manifests,
                excluded,
                states,
                next_stack,
                order,
                errors,
            )
    states[feature_id] = 2
    if feature_id not in order:
        order.append(feature_id)


def _build_plan(catalog_path: str, bridge: Node) -> tuple[PlanResult, dict[str, FeatureManifest]]:
    manifests_list, errors = _load_catalog(catalog_path)
    table, bridge_error = _bridge_family_table(bridge)
    if bridge_error:
        errors.append(bridge_error)
    return _plan_from_manifests(manifests_list, errors, table)


def _build_rust_plan(
    catalog_path: str, table: FamilyTable
) -> tuple[PlanResult, dict[str, FeatureManifest]]:
    # Rust manifests spell family requirements as symbolic logical names, so
    # the same planning core negotiates them against the producer table that
    # `family_table` answered instead of the pilot identity table.
    manifests_list, errors = _load_catalog(catalog_path)
    return _plan_from_manifests(manifests_list, errors, table)


def _plan_from_manifests(
    manifests_list: list[FeatureManifest],
    errors: list[str],
    table: FamilyTable | None,
) -> tuple[PlanResult, dict[str, FeatureManifest]]:
    disabled: list[str] = []
    excluded: set[str] = set()
    manifests: dict[str, FeatureManifest] = {}
    # Reject incompatible manifests before traversing dependencies so world state is
    # never partially instantiated from an invalid catalog.
    for manifest in manifests_list:
        if not manifest.feature_id:
            errors.append("feature manifest has an empty ID")
            continue
        if manifest.feature_id in manifests:
            errors.append(f"duplicate feature ID: {manifest.feature_id}")
            continue
        manifests[manifest.feature_id] = manifest
        incompatibility = _manifest_incompatibility(manifest, table)
        if incompatibility:
            _exclude_or_fail(manifest, incompatibility, errors, disabled, excluded)
    changed = True
    # Dependency exclusion is a fixed-point operation: disabling one optional
    # feature may make another feature unavailable on the next pass.
    while changed:
        changed = False
        for feature_id in sorted(manifests):
            if feature_id in excluded:
                continue
            manifest = manifests[feature_id]
            if len(set(manifest.dependencies)) != len(manifest.dependencies):
                _exclude_or_fail(
                    manifest,
                    "has a duplicate dependency",
                    errors,
                    disabled,
                    excluded,
                )
                changed = True
                continue
            unavailable = next(
                (
                    dependency
                    for dependency in manifest.dependencies
                    if dependency not in manifests or dependency in excluded
                ),
                "",
            )
            if unavailable:
                _exclude_or_fail(
                    manifest,
                    f"has missing or disabled dependency {unavailable}",
                    errors,
                    disabled,
                    excluded,
                )
                changed = True
    states: dict[str, int] = {}
    order: list[str] = []
    # Sorted traversal makes activation order independent of resource serialization.
    for feature_id in sorted(manifests):
        if feature_id not in excluded:
            _visit_feature(feature_id, manifests, excluded, states, (), order, errors)
    return (
        PlanResult(
            ok=not errors,
            errors=tuple(errors),
            disabled=tuple(sorted(set(disabled))),
            order=tuple(order),
        ),
        manifests,
    )


def _feature_failure(
    host: feature_host,
    instance: Node,
    manifest: FeatureManifest,
    epoch: int,
) -> str:
    # Lifecycle methods are structural Godot contracts because Py4Godot scripts are
    # loaded as independent resource modules rather than a shared Python package.
    lifecycle = (
        "validate_feature",
        "bind_host",
        "activate_feature",
        "reset_feature",
        "deactivate_feature",
    )
    for method_name in lifecycle:
        if not instance.has_method(method_name):
            return f"is missing lifecycle method {method_name}"
    host._trace.append(f"validate:{manifest.feature_id}")
    failure = _call_text(instance, "validate_feature", manifest.feature_id)
    if failure:
        return f"validate failed: {failure}"
    host._trace.append(f"bind:{manifest.feature_id}")
    # Features receive the bridge's absolute scene path because project-class
    # objects cannot cross script-module method boundaries in the pinned
    # runtime; each feature acquires the typed object through `get_node`.
    failure = _call_text(instance, "bind_host", host._bridge_path)
    if failure:
        return f"bind failed: {failure}"
    host._trace.append(f"activate:{manifest.feature_id}:{epoch}")
    failure = _call_text(instance, "activate_feature", epoch)
    if failure:
        return f"activate failed: {failure}"
    return ""


def _release_instance(instance: Node) -> None:
    instance.queue_free()


def _absolute_path_text(node: Node) -> str:
    # The runtime's `NodePath` wrapper has no usable text conversion, so the
    # path is rebuilt from its concatenated name components.
    path = node.get_path()
    names = str(path.get_concatenated_names())
    if path.is_absolute():
        return f"/{names}"
    return names


def _activate(host: feature_host, catalog_path: str, bridge: Node, epoch: int) -> PlanResult:
    # Each activation begins from a clean host so a failed prior catalog cannot leak
    # instances or bridge state into the next session epoch.
    _deactivate(host)
    host._trace = []
    host._bridge = bridge
    host._bridge_path = _absolute_path_text(bridge)
    plan, manifests = _build_plan(catalog_path, bridge)
    if not plan.ok:
        return plan
    return _instantiate_features(host, plan, manifests, epoch)


def _instantiate_features(
    host: feature_host,
    plan: PlanResult,
    manifests: dict[str, FeatureManifest],
    epoch: int,
) -> PlanResult:
    """Instantiate one accepted plan in dependency order, with rollback.

    Both the pilot and rust activation paths share this loop: providers enter
    before consumers, a required failure rolls the already-active reverse
    trace back through `_deactivate`, and an optional failure is isolated to
    the affected feature and its dependents.
    """
    disabled = list(plan.disabled)
    errors: list[str] = []
    for feature_id in plan.order:
        manifest = manifests[feature_id]
        unavailable = next(
            (
                dependency
                for dependency in manifest.dependencies
                if dependency not in host._instances
            ),
            "",
        )
        if unavailable:
            message = (
                f"feature {feature_id} cannot activate after dependency {unavailable} was disabled"
            )
            if manifest.required:
                errors.append(message)
                _deactivate(host)
                return PlanResult(False, tuple(errors), tuple(sorted(disabled)), plan.order)
            disabled.append(feature_id)
            continue
        scene_resource = _load_resource(manifest.entry_scene_path)
        if scene_resource is None:
            failure = "entry scene could not be loaded"
            instance = None
        else:
            scene = cast(PackedScene, scene_resource)
            instance = scene.instantiate()
            failure = "entry scene could not be instantiated" if instance is None else ""
        if instance is not None:
            host._trace.append(f"instantiate:{feature_id}")
            host.add_child(instance)
            failure = _feature_failure(host, instance, manifest, epoch)
        if failure:
            if instance is not None:
                _release_instance(instance)
            if manifest.required:
                # Required failure rolls back already-active dependencies in reverse
                # order; optional failure is isolated to the affected feature.
                errors.append(f"feature {feature_id} {failure}")
                _deactivate(host)
                return PlanResult(False, tuple(errors), tuple(sorted(disabled)), plan.order)
            disabled.append(feature_id)
            continue
        if instance is not None:
            host._instances[feature_id] = instance
            host._active_order.append(feature_id)
    return PlanResult(True, (), tuple(sorted(set(disabled))), plan.order)


def _reset(host: feature_host, epoch: int) -> None:
    # Reset preserves dependency order so providers refresh before their consumers.
    for feature_id in host._active_order:
        host._trace.append(f"reset:{feature_id}:{epoch}")
        host._instances[feature_id].call("reset_feature", epoch)


def _deactivate(host: feature_host) -> None:
    if not hasattr(host, "_active_order"):
        return
    # Consumers deactivate before providers to preserve dependency lifetime safety.
    for feature_id in reversed(host._active_order):
        host._trace.append(f"deactivate:{feature_id}")
        instance = host._instances[feature_id]
        instance.call("deactivate_feature")
        _release_instance(instance)
    host._instances.clear()
    host._active_order.clear()
    # The host-owned rust core releases only after every feature consumer is
    # down, and `_close_rust` still sees the bound bridge for its one call.
    _close_rust(host)
    host._bridge = None
    host._bridge_path = ""


def _close_rust(host: feature_host) -> None:
    """Release the host-owned rust core token exactly once per issue.

    The facade's close is idempotent for a repeated token, but the host drops
    its copy on the first teardown so no later tick or teardown can replay a
    stale token. The state clears even when the envelope refuses: a token the
    host decided to release must never be reused as if it were still live.
    """
    token = getattr(host, "_rust_token", None)
    bridge = host._bridge
    if token is not None and bridge is not None:
        bridge.call("close", token)
    host._rust_token = None
    host._rust_epoch = None
    host._rust_frame = None
    host._rust_families = None
    host._rust_required = ()
    host._rust_mode = False


def _rust_family_table(bridge: Node, token: dict) -> tuple[FamilyTable | None, str]:
    """Negotiate the rust producer descriptor table for one live core.

    The table is the producer identity the rust manifests negotiate against:
    symbolic logical names with per-family contract versions, so a duplicate
    name, an unnamed or unversioned descriptor, or a non-canonical bound is a
    producer-contract break the host refuses before instantiating features.
    """
    result = bridge.call("family_table", token)
    value = _rust_envelope_value(result)
    if value is None:
        return None, f"the rust core did not answer its family table ({_rust_error_class(result)})"
    if not isinstance(value, dict) or value.get("producer") != RUST_PRODUCER_NAME:
        return None, "the rust producer identity is not the frozen rust-client-core table"
    descriptors = value.get("descriptors")
    if not isinstance(descriptors, list) or not descriptors:
        return None, "the rust family table reported no descriptors"
    versions: dict[str, tuple[int, int]] = {}
    for descriptor in descriptors:
        if not isinstance(descriptor, dict):
            return None, "the rust family table reported an unreadable descriptor"
        name = descriptor.get("logical_name")
        if not isinstance(name, str) or not name:
            return None, "the rust family table reported an unnamed descriptor"
        if name in versions:
            return None, f"the rust family table reports family {name} twice"
        major = _u16_word(descriptor.get("major"))
        minor = _u16_word(descriptor.get("minor"))
        numeric_id = descriptor.get("numeric_id")
        if major is None or minor is None or major < 1:
            return None, f"the rust family table reported an invalid version for {name}"
        if not isinstance(numeric_id, int) or isinstance(numeric_id, bool) or numeric_id <= 0:
            return None, f"the rust family table reported an invalid numeric ID for {name}"
        if _u64_text(descriptor.get("record_limit")) is None or _u64_text(
            descriptor.get("record_bytes")
        ) is None:
            return None, f"the rust family table reported an invalid bound for {name}"
        versions[name] = (major, minor)
    return FamilyTable(versions), ""


def _required_families(
    manifests: dict[str, FeatureManifest], active_order: list[str]
) -> tuple[tuple[str, int, int], ...]:
    """The deduplicated family requirements of the features that activated."""
    required: dict[str, tuple[int, int]] = {}
    for feature_id in active_order:
        manifest = manifests[feature_id]
        for family_spec in manifest.required_bridge_families:
            parts = family_spec.rsplit("@", 1)
            version = _parse_version(parts[1]) if len(parts) == 2 else None
            if version is None:
                continue
            required[parts[0]] = max(required.get(parts[0], (0, 0)), version)
    return tuple((name, major, minor) for name, (major, minor) in sorted(required.items()))


def _rust_frame_from(
    result: object,
    epoch: str,
    families: dict[str, tuple[int, int]] | None,
    required: tuple[tuple[str, int, int], ...],
) -> dict | None:
    """Validate one whole pulled frame, or refuse it without side effects.

    The facade already validates the frame it renders; this is the host's own
    closed-shape check so a torn, mixed, or stale scripted or drifted result
    never reaches a feature: complete scalar fields in canonical spellings,
    the current session epoch, no duplicate family keys, every key known to
    the negotiated table at a compatible version, and every family the active
    manifests declared required present in the same frame.
    """
    frame = _rust_envelope_value(result)
    if not isinstance(frame, dict):
        return None
    if _u16_word(frame.get("layout_major")) is None or _u16_word(frame.get("layout_minor")) is None:
        return None
    if frame.get("session_epoch") != epoch or _u64_text(frame.get("session_epoch")) is None:
        return None
    if _u64_text(frame.get("confirmed_revision")) is None:
        return None
    if _u64_text(frame.get("frame_index")) is None:
        return None
    families_list = frame.get("families")
    if not isinstance(families_list, list) or not families_list:
        return None
    seen: set[str] = set()
    for family in families_list:
        if not isinstance(family, dict) or not isinstance(family.get("records"), list):
            return None
        key = family.get("key")
        if not isinstance(key, dict):
            return None
        name = key.get("logical_name")
        major = _u16_word(key.get("major"))
        minor = _u16_word(key.get("minor"))
        if not isinstance(name, str) or not name or name in seen:
            return None
        if major is None or minor is None:
            return None
        seen.add(name)
        registered = families.get(name) if families is not None else None
        if registered is None:
            return None
        if major != registered[0] or minor > registered[1]:
            return None
    for name, required_major, required_minor in required:
        if name not in seen:
            return None
        registered = families[name] if families is not None else (required_major, required_minor)
        if required_major != registered[0] or required_minor > registered[1]:
            return None
    return frame


def _rust_dispatch(host: feature_host) -> int:
    """One host-owned facade step, pull, validation, and same-frame fan-out.

    Exactly one `step` and one `pull_typed_frame` run per tick regardless of
    the feature count, and every validated frame is handed to each feature as
    the same owned object in activation order. Any refused step or refused,
    torn, mixed, or stale frame returns early: the previous frame and every
    feature resource stay untouched until a later tick succeeds.
    """
    bridge = host._bridge
    token = host._rust_token
    epoch = host._rust_epoch
    if bridge is None or token is None or epoch is None:
        return 0
    work = {"messages": RUST_STEP_MESSAGE_BUDGET, "meshes": RUST_STEP_MESH_BUDGET}
    step_result = bridge.call("step", token, epoch, work)
    if not isinstance(_rust_envelope_value(step_result), dict):
        return 0
    pull_result = bridge.call("pull_typed_frame", token, epoch)
    frame = _rust_frame_from(pull_result, epoch, host._rust_families, host._rust_required)
    if frame is None:
        return 0
    host._rust_frame = frame
    apply_started = time.perf_counter_ns()
    for feature_id in host._active_order:
        instance = host._instances.get(feature_id)
        if instance is not None and instance.has_method("apply_typed_frame"):
            instance.call("apply_typed_frame", frame)
    return time.perf_counter_ns() - apply_started


def _activate_rust(
    host: feature_host,
    catalog_path: str,
    bridge: Node,
    epoch: int,
    config: object,
    endpoint: object,
    identity: object,
) -> PlanResult:
    """Open, connect, negotiate, and instantiate the rust-core host session.

    The host owns the token and epoch for the whole session: it opens the
    core, begins the connection, negotiates the producer family table, and
    only then instantiates features against that table. Any refusal releases
    the opened token, so no half-open session survives a failed activation.
    """
    _deactivate(host)
    host._trace = []
    host._bridge = bridge
    host._bridge_path = _absolute_path_text(bridge)
    missing = next((name for name in RUST_FACADE_METHODS if not bridge.has_method(name)), "")
    if missing:
        return PlanResult(
            False, (f"the bridge is missing the rust facade method {missing}",), (), ()
        )
    open_result = bridge.call("open_core", config)
    token = _rust_token_from(open_result)
    if token is None:
        return PlanResult(
            False,
            (f"the rust core refused to open ({_rust_error_class(open_result)})",),
            (),
            (),
        )
    host._rust_token = token
    host._rust_mode = True
    connect_result = bridge.call("connect", token, endpoint, identity)
    epoch_text = _rust_epoch_text_from(connect_result)
    if epoch_text is None:
        return _rust_abandon(
            host,
            PlanResult(
                False,
                (f"the rust core refused the connection ({_rust_error_class(connect_result)})",),
                (),
                (),
            ),
        )
    host._rust_epoch = epoch_text
    table, table_error = _rust_family_table(bridge, token)
    if table is None:
        return _rust_abandon(host, PlanResult(False, (table_error,), (), ()))
    host._rust_families = table.versions
    plan, manifests = _build_rust_plan(catalog_path, table)
    if not plan.ok:
        return _rust_abandon(host, plan)
    result = _instantiate_features(host, plan, manifests, epoch)
    if not result.ok:
        # The instantiation rollback already deactivated the features and
        # closed the core token through `_deactivate`.
        return result
    host._rust_required = _required_families(manifests, host._active_order)
    return result


def _rust_abandon(host: feature_host, result: PlanResult) -> PlanResult:
    """Release the opened core token of a rust activation that cannot finish."""
    _close_rust(host)
    return result


def _reset_rust(host: feature_host, epoch: int) -> None:
    """Move the host-owned session onto a fresh facade epoch, then features."""
    bridge = host._bridge
    token = host._rust_token
    session_epoch = host._rust_epoch
    if bridge is None or token is None or session_epoch is None:
        return
    result = bridge.call("reset", token, session_epoch)
    fresh = _rust_epoch_text_from(result)
    if fresh is None:
        # A refused reset keeps the live epoch and leaves feature state
        # untouched rather than splitting features from their session epoch.
        return
    host._rust_epoch = fresh
    # Frames of the retired epoch are stale by contract; the retained copy is
    # dropped so only a validated frame of the fresh epoch can reappear.
    host._rust_frame = None
    _reset(host, epoch)
