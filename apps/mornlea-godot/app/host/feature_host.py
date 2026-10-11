"""Assemble coarse Godot features without importing project Python packages.

Godot resource paths are the discovery boundary. The embedded interpreter is
deliberately isolated from the repository root, so this host must remain
independent of ambient ``sys.path`` entries and sibling-module imports.
"""

from __future__ import annotations

import json
import math
import sys
import time
from collections.abc import Callable
from typing import Any, Protocol, cast, runtime_checkable

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
RUST_FRAME_LAYOUT = (1, 0)
RUST_REGISTRY_PATH = "res://catalog/capability_registry.tres"
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
        disabled_reasons: dict[str, str] | None = None,
    ) -> None:
        self.ok = ok
        self.errors = errors
        self.disabled = disabled
        self.order = order
        # Additive diagnostic mapping: why each optionally disabled feature
        # was dropped. The bare-ID `disabled` list stays the consumed
        # contract; this payload never feeds a required-failure rollback.
        self.disabled_reasons = disabled_reasons if disabled_reasons is not None else {}

    def to_json(self) -> str:
        return json.dumps(
            {
                "ok": self.ok,
                "errors": list(self.errors),
                "disabled": list(self.disabled),
                "disabled_reasons": self.disabled_reasons,
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


def _exact_fields(value: object, names: tuple[str, ...]) -> bool:
    return (
        isinstance(value, dict)
        and len(value) == len(names)
        and all(name in value for name in names)
    )


def _bounded_integer(value: object, maximum: int) -> bool:
    return isinstance(value, int) and not isinstance(value, bool) and 0 <= value <= maximum


def _rust_terminal(value: object) -> bool:
    if value is None:
        return True
    if not isinstance(value, dict) or not _exact_fields(value, ("tag", "value")):
        return False
    tag = value["tag"]
    if tag in ("LocalClose", "Timeout", "Capacity", "Internal"):
        return value["value"] is None
    return tag in ("RemoteDisconnect", "LoginRejected") and _bounded_text(value["value"])


# These schemas mirror the owned renderer, without importing project modules.
_ValueCheck = Callable[[object], bool]
_RUST_FRAME_BYTES = 8 << 20


def _word(minimum: int, maximum: int) -> _ValueCheck:
    return lambda value: type(value) is int and minimum <= value <= maximum


def _enum(*names: str) -> _ValueCheck:
    return lambda value: isinstance(value, str) and value in names


def _optional(check: _ValueCheck) -> _ValueCheck:
    return lambda value: value is None or check(value)


def _struct(**fields: _ValueCheck) -> _ValueCheck:
    def check(value: object) -> bool:
        return (
            isinstance(value, dict)
            and _exact_fields(value, tuple(fields))
            and all(validator(value[name]) for name, validator in fields.items())
        )

    return check


def _union(**variants: _ValueCheck) -> _ValueCheck:
    def check(value: object) -> bool:
        if not isinstance(value, dict) or not _exact_fields(value, ("tag", "value")):
            return False
        tag = value["tag"]
        return isinstance(tag, str) and tag in variants and variants[tag](value["value"])

    return check


def _array(check: _ValueCheck, length: int) -> _ValueCheck:
    return lambda value: (
        isinstance(value, list) and len(value) == length and all(check(item) for item in value)
    )


def _float(value: object) -> bool:
    return type(value) is float and math.isfinite(value)


def _f32(value: object) -> bool:
    return _float(value) and abs(cast(float, value)) <= 3.4028234663852886e38


def _hex(length: int) -> _ValueCheck:
    return lambda value: (
        isinstance(value, str)
        and len(value) == length
        and all(char in "0123456789abcdef" for char in value)
    )


def _uuid(value: object) -> bool:
    return _hex(32)(value) and cast(str, value)[12] == "4" and cast(str, value)[16] in "89ab"


def _text(maximum: int, minimum: int = 0, scalars: int | None = None) -> _ValueCheck:
    def check(value: object) -> bool:
        # Python character length bounds encoding work before UTF-8 allocation.
        if not isinstance(value, str) or not minimum <= len(value) <= maximum:
            return False
        if scalars is not None and len(value) > scalars:
            return False
        try:
            return len(value.encode("utf-8")) <= maximum
        except UnicodeEncodeError:
            return False

    return check


_PINNED_SPACE = (
    "\t\n\v\f\r \u0085\u00a0\u1680\u2000\u2001\u2002\u2003\u2004"
    "\u2005\u2006\u2007\u2008\u2009\u200a\u2028\u2029\u202f\u205f\u3000"
)


def _canonical(maximum: int, name: bool = False, companion: bool = False) -> _ValueCheck:
    def check(value: object) -> bool:
        if not _text(maximum, 1, 32 if name else None)(value):
            return False
        text = cast(str, value)
        return (
            text[0] not in _PINNED_SPACE
            and text[-1] not in _PINNED_SPACE
            and all(
                not (ord(char) < 32 or 127 <= ord(char) <= 159)
                and (not companion or char not in _PINNED_SPACE)
                for char in text
            )
        )

    return check


def _u64(value: object) -> bool:
    return _u64_text(value) is not None


def _nonzero_u64(value: object) -> bool:
    return _u64_text(value) not in (None, 0)


def _bool(value: object) -> bool:
    return type(value) is bool


def _null(value: object) -> bool:
    return value is None


_u8 = _word(0, 255)
_u16 = _word(0, 65535)
_u32 = _word(0, 4294967295)
_i32 = _word(-2147483648, 2147483647)
_vec3 = _array(_float, 3)
_chunk = _struct(x=_i32, z=_i32)
_block = _struct(x=_i32, y=_i32, z=_i32)
_pose = _struct(position=_vec3, yaw=_float, pitch=_float)
_look = _struct(yaw=_f32, pitch=_f32)
# G1 erases BoundedText.kind, so generic fields admit its constructor union.
_bounded_text = _text(1024)
_errors = _enum(
    "InvalidInput",
    "InvalidState",
    "IncompatibleVersion",
    "StaleEpoch",
    "Timeout",
    "Capacity",
    "Disconnected",
    "Io",
    "Internal",
)
_reject = _enum(
    "InvalidRay",
    "NoTarget",
    "ChunkNotReady",
    "ProtectedBlock",
    "InvalidBlock",
    "Occupied",
    "InvalidInput",
    "PlayerNotReady",
    "InvalidSlot",
    "HotbarFull",
    "DropCapacity",
    "ContainerCapacity",
    "NotFluidSource",
    "BucketMismatch",
    "NotArmor",
)
_task_state = _union(
    Started=_null,
    Progress=_null,
    Completed=_null,
    TimedOut=_null,
    Stopped=_null,
    Failed=_enum(
        "PlannerUnavailable", "InvalidPlan", "PathUnreachable", "WorldChanged", "InventoryFull"
    ),
)
_companion = _struct(id=_uuid, name=_canonical(128, True, True))
_observation = _struct(epoch=_nonzero_u64, confirmed_revision=_u64, ordinal=_u32)
_terrain_key = _union(Section=_struct(chunk=_chunk, section=_word(0, 23)), LodTile=_chunk)
_resource = _struct(
    epoch=_nonzero_u64,
    dimension=_word(0, 1),
    key=_terrain_key,
    generation=_u64,
    content_revision=_u64,
    job_id=_nonzero_u64,
)
_control = _struct(
    movement=_struct(move_x=_word(-128, 127), move_z=_word(-128, 127), jump=_bool),
    look=_look,
    actions=_struct(primary=_bool, eating=_bool, sprinting=_bool, sneaking=_bool),
)


def _container(value: object) -> bool:
    if not _struct(
        chunk=_chunk, kind=_enum("Furnace", "Chest"), slot=_u8, generation=_word(1, 4294967295)
    )(value):
        return False
    parts = cast(dict[str, object], value)
    return cast(int, parts["slot"]) < (32 if parts["kind"] == "Furnace" else 16)


def _stack(value: object) -> bool:
    if not _struct(item=_u16, count=_u8, durability=_u16)(value):
        return False
    parts = cast(dict[str, int], value)
    item, count, durability = parts["item"], parts["count"], parts["durability"]
    if item == 0:
        return count == 0 and durability == 0
    single = (
        10,
        11,
        12,
        13,
        30,
        31,
        32,
        33,
        47,
        48,
        49,
        50,
        51,
        52,
        55,
        56,
        58,
        59,
        60,
        61,
        62,
        65,
    )
    maxima = {
        10: 131,
        11: 250,
        30: 131,
        31: 250,
        47: 59,
        48: 131,
        49: 250,
        58: 165,
        59: 240,
        60: 225,
        61: 195,
        62: 120,
    }
    return (
        1 <= item < 66
        and 1 <= count <= (1 if item in single else 64)
        and (1 <= durability <= maxima[item] if item in maxima else durability == 0)
    )


def _crafting(value: object) -> bool:
    if not _struct(size=_enum("Personal", "Workbench"), slots=_array(_stack, 9), output=_stack)(
        value
    ):
        return False
    parts = cast(dict[str, Any], value)
    return parts["size"] != "Personal" or all(slot["item"] == 0 for slot in parts["slots"][4:])


def _furnace(value: object) -> bool:
    if not _struct(
        container=_container,
        input=_stack,
        fuel=_stack,
        output=_stack,
        progress_ticks=_word(0, 199),
        burn_ticks=_word(0, 1600),
    )(value):
        return False
    parts = cast(dict[str, Any], value)
    return (
        parts["container"]["kind"] == "Furnace"
        and parts["input"]["item"] in (0, 6, 18, 27, 53)
        and parts["fuel"]["item"] in (0, 5)
        and parts["output"]["item"] in (0, 7, 23, 24, 54)
    )


def _chest(value: object) -> bool:
    return (
        _struct(container=_container, items=_array(_stack, 27))(value)
        and cast(dict[str, Any], value)["container"]["kind"] == "Chest"
    )


def _mining_active(value: object) -> bool:
    return (
        _struct(target=_block, progress=_u16, required=_u16, harvestable=_bool)(value)
        and 0 < cast(dict[str, int], value)["progress"] < cast(dict[str, int], value)["required"]
    )


_inventory_view = _union(
    Inventory=_struct(selected=_word(0, 8), hotbar=_array(_stack, 9), backpack=_array(_stack, 27)),
    Crafting=_crafting,
    Furnace=_furnace,
    Chest=_chest,
    Closed=_container,
    Rejected=_struct(sequence=_u64, reason=_reject),
)
_chat_speaker = _struct(companion=_companion, command=_canonical(1024))
_chat_body = _union(
    Accepted=_chat_speaker,
    InvalidFormat=_null,
    UnknownCompanion=_struct(name=_canonical(128, True, True)),
    QueueFull=_chat_speaker,
    NotFollowing=_chat_speaker,
    Task=_struct(companion=_companion, command=_canonical(1024), state=_task_state),
    Speech=_struct(companion=_companion, text=_canonical(256)),
)
_world_view = _union(
    Environment=_struct(
        day_phase_offset=_word(0, 23999),
        world_time_ticks=_u64,
        weather=_enum("Clear", "Rain", "Thunder"),
        season=_enum("Spring", "Summer", "Autumn", "Winter"),
        season_progress=_u8,
        temperature=_word(-128, 127),
    ),
    Survival=_struct(
        health=_word(0, 20),
        oxygen=_word(0, 300),
        hunger=_word(0, 20),
        saturation_zero=_bool,
        armor_points=_word(0, 20),
    ),
    Chat=_struct(
        event_id=_nonzero_u64, player_id=_uuid, player_name=_canonical(128, True), body=_chat_body
    ),
    Task=_struct(
        observation=_observation, companion=_companion, command=_canonical(1024), state=_task_state
    ),
    Prompt=_optional(_struct(target=_block, label=_bounded_text)),
)
_actor_id = _union(
    RemotePlayer=_uuid,
    Companion=_uuid,
    Hostile=_nonzero_u64,
    Passive=_nonzero_u64,
    Projectile=_nonzero_u64,
    Drop=_struct(dimension=_i32, slot=_word(0, 31), generation=_word(1, 4294967295), chunk=_chunk),
)
_actor_detail = _union(
    RemotePlayer=_struct(display_name=_optional(_bounded_text), reset=_optional(_bool)),
    Companion=_struct(name=_optional(_bounded_text), reset=_optional(_bool)),
    Drop=_struct(block_index=_u32, stack=_stack),
    Hostile=_struct(archetype=_u8, health=_u8),
    Passive=_struct(
        health=_optional(_u8),
        grazing=_optional(_u8),
        despawn_reason=_optional(_enum("Vanished", "Died")),
    ),
    Projectile=_struct(archetype=_optional(_u8)),
)
_provenance = _union(
    Confirmed=_struct(observation=_observation, authoritative_event_id=_optional(_u64)),
    Predicted=_struct(input_sequence=_u64),
    Local=_struct(local_event_sequence=_u64),
)
_resource_order_entry = _union(
    InputJournal=_null,
    PreparationQueue=_null,
    PresentationFrames=_null,
    BridgeHandles=_null,
    Feature=_u64,
)


def _resource_order(value: object) -> bool:
    # Nullary entries cost two semantic bytes; Feature entries cost nine.
    return (
        isinstance(value, list)
        and len(value) <= _RUST_FRAME_BYTES // 2
        and all(_resource_order_entry(entry) for entry in value)
    )


_RECORD_SCHEMAS = {
    1: _struct(
        phase=_enum("Disconnected", "Connecting", "Handshaking", "Admitted", "Closing"),
        player_id=_optional(_uuid),
        terminal=_rust_terminal,
    ),
    2: _struct(
        local_sequence=_optional(_u64),
        intent_kind=_enum(
            "PlayerInput",
            "PlaceBlock",
            "Resync",
            "SelectHotbar",
            "OpenContainer",
            "TillSoil",
            "BoneMeal",
            "CollectWater",
            "PlaceWater",
            "MoveInventory",
            "MoveCrafting",
            "MoveContainer",
            "CloseContainer",
            "DropSelectedItem",
            "TakeCraftingOutput",
            "EquipArmor",
            "MovePartial",
            "QuickMove",
            "DropStack",
            "Chat",
        ),
        receipt=_union(
            Queued=_null, Rejected=_errors, Confirmed=_struct(server_tick=_optional(_u64))
        ),
    ),
    3: _struct(
        dimension=_word(0, 1),
        key=_terrain_key,
        content_revision=_u64,
        generation=_u64,
        material=_enum("Opaque", "Cutout", "Water"),
        visibility=_enum("Near", "Far"),
        light=_struct(sky=_word(0, 15), block=_word(0, 15)),
        resource=_optional(_resource),
    ),
    4: _struct(
        kind=_enum("RemotePlayer", "Drop", "Hostile", "Passive", "Projectile", "Companion"),
        id=_actor_id,
        dimension=_union(Known=_word(0, 1), DropRaw=_i32),
        position=_optional(_vec3),
        yaw=_optional(_float),
        pitch=_optional(_float),
        velocity=_optional(_vec3),
        detail=_optional(_actor_detail),
    ),
    5: _struct(
        confirmed_pose=_pose,
        predicted_pose=_optional(_pose),
        look_ray=_optional(
            _struct(
                origin=_vec3,
                look=_look,
                reach=lambda value: _float(value) and 0 < cast(float, value) <= 6,
            )
        ),
        movement=_struct(control=_optional(_control), on_ground=_bool),
        correction=_optional(
            _struct(
                last_input_sequence=_u64,
                reason=_enum("AuthoritativeReset", "AcknowledgedReplay", "RejectedInput"),
            )
        ),
        mining=_union(Idle=_null, Active=_mining_active),
    ),
    6: _struct(
        view=_inventory_view,
        token=_optional(_struct(epoch=_nonzero_u64, reference=_container, confirmed_revision=_u64)),
        outcome=_optional(
            _union(
                Rejected=_struct(sequence=_u64, reason=_reject),
                PlacementAccepted=_struct(sequence=_u64),
            )
        ),
    ),
    7: _struct(view=_world_view),
    8: _struct(
        cue_id=_word(0, 6),
        provenance=_provenance,
        category=_enum("Ui", "Footstep", "World", "Combat", "Ambient"),
        position=_optional(_vec3),
        gain=lambda value: _float(value) and 0 <= cast(float, value) <= 1,
        pitch=lambda value: _f32(value) and cast(float, value) > 0,
    ),
    9: _struct(
        transition=_enum("Open", "Reset", "Close", "Invalidate"),
        generation=_u64,
        resource_order=_resource_order,
    ),
    10: _struct(
        producer=_struct(source_sha=_hex(40), contract_sha=_hex(64)),
        frame_index=_u64,
        queue_high_water=_struct(
            input=_u64,
            inbound_records=_u64,
            inbound_bytes=_u64,
            outbound_records=_u64,
            outbound_bytes=_u64,
            journal=_u64,
            preparation=_u64,
        ),
        rejected=_struct(
            invalid=_u64, capacity=_u64, stale=_u64, disconnected=_u64, io=_u64, internal=_u64
        ),
    ),
}


def _rust_record_payload(record: dict[str, Any], numeric_id: int) -> bool:
    # The fixed schema graph never traverses undeclared data or recursive values.
    schema = _RECORD_SCHEMAS.get(numeric_id)
    if schema is None or not schema(
        {key: value for key, value in record.items() if key != "header"}
    ):
        return False
    if numeric_id == 3:
        return bool((record["key"]["tag"] == "Section") == (record["visibility"] == "Near"))
    if numeric_id == 4:
        return bool(
            record["kind"] == record["id"]["tag"]
            and (record["detail"] is None or record["kind"] == record["detail"]["tag"])
            and (record["kind"] == "Drop" or record["dimension"]["tag"] == "Known")
        )
    return True


def _utf8_size(text: str) -> int:
    return len(text.encode("utf-8"))


def _terrain_key_size(key: dict[str, Any]) -> int:
    return 10 if key["tag"] == "Section" else 9


def _chat_body_size(body: dict[str, Any]) -> int:
    tag, value = body["tag"], body["value"]
    if tag == "InvalidFormat":
        return 1
    if tag == "UnknownCompanion":
        return 2 + _utf8_size(value["name"])
    return 18 + _utf8_size(value["text"] if tag == "Speech" else value["command"])


def _rust_record_size(record: dict[str, Any], numeric_id: int) -> int:
    """Match C2 validated_size, including its option and vector accounting."""
    header = 18 + (8 if record["header"]["source_tick"] is not None else 0)
    if numeric_id == 1:
        terminal = record["terminal"]
        terminal_bytes = 0 if terminal is None else 1
        if terminal is not None and terminal["value"] is not None:
            terminal_bytes += 1 + _utf8_size(terminal["value"])
        return header + 1 + (17 if record["player_id"] is not None else 0) + terminal_bytes
    if numeric_id == 2:
        receipt = record["receipt"]
        receipt_bytes = 1 if receipt["tag"] == "Queued" else 2
        if receipt["tag"] == "Confirmed" and receipt["value"]["server_tick"] is not None:
            receipt_bytes += 8
        return header + 2 + (8 if record["local_sequence"] is not None else 0) + receipt_bytes
    if numeric_id == 3:
        key_bytes = _terrain_key_size(record["key"])
        return header + 21 + key_bytes + (34 + key_bytes if record["resource"] is not None else 0)
    if numeric_id == 4:
        kind = record["kind"]
        identity = 18 if kind == "Drop" else 17 if kind in ("RemotePlayer", "Companion") else 9
        size = header + 2 + identity + (1 if record["dimension"]["tag"] == "Known" else 4)
        size += sum(
            width
            for field, width in (("position", 25), ("yaw", 9), ("pitch", 9), ("velocity", 25))
            if record[field] is not None
        )
        detail = record["detail"]
        if detail is not None:
            value = detail["value"]
            size += 1
            if kind in ("RemotePlayer", "Companion"):
                name = value["display_name" if kind == "RemotePlayer" else "name"]
                size += 0 if name is None else 1 + _utf8_size(name)
                size += int(value["reset"] is not None)
            else:
                size += {"Drop": 10, "Hostile": 2, "Passive": 3, "Projectile": 1}[kind]
        return size
    if numeric_id == 5:
        return (
            header
            + 43
            + (40 if record["predicted_pose"] is not None else 0)
            + (37 if record["look_ray"] is not None else 0)
            + (19 if record["movement"]["control"] is not None else 0)
            + (10 if record["correction"] is not None else 0)
            + (18 if record["mining"]["tag"] == "Active" else 0)
        )
    if numeric_id == 6:
        view = record["view"]["tag"]
        size = (
            header
            + 1
            + {
                "Inventory": 181,
                "Crafting": 52,
                "Furnace": 19,
                "Chest": 136,
                "Closed": 15,
                "Rejected": 10,
            }[view]
        )
        size += 31 if record["token"] is not None else 0
        if record["outcome"] is not None:
            size += 11 if record["outcome"]["tag"] == "Rejected" else 10
        return size
    if numeric_id == 7:
        tag, value = record["view"]["tag"], record["view"]["value"]
        if tag in ("Environment", "Survival"):
            size = 15 if tag == "Environment" else 7
        elif tag == "Chat":
            size = 25 + _utf8_size(value["player_name"]) + _chat_body_size(value["body"])
        elif tag == "Task":
            size = 38 + _utf8_size(value["command"])
        else:
            size = 1 if value is None else 15 + _utf8_size(value["label"])
        return header + 1 + size
    if numeric_id == 8:
        provenance = record["provenance"]
        size = 9
        if provenance["tag"] == "Confirmed":
            size = 22 + (8 if provenance["value"]["authoritative_event_id"] is not None else 0)
        return header + 11 + size + (25 if record["position"] is not None else 0)
    if numeric_id == 9:
        return (
            header
            + 17
            + sum(9 if entry["tag"] == "Feature" else 2 for entry in record["resource_order"])
        )
    return header + 164


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
    if not isinstance(result, dict) or not _exact_fields(result, ("ok", "value", "error")):
        return None
    if result["ok"] is not True or result["error"] is not None:
        return None
    return result["value"]


def _rust_token_from(result: object) -> dict[str, object] | None:
    """One validated core token, passed to the bridge verbatim thereafter.

    The token is an opaque owned value: the host checks only that it is the
    facade-issued `{slot, generation}` record and then returns the same
    object on every call, never rebuilding or mutating it.
    """
    token = _rust_envelope_value(result)
    if not isinstance(token, dict) or not _exact_fields(token, ("slot", "generation")):
        return None
    slot = token.get("slot")
    if not _bounded_integer(slot, 4_294_967_295) or slot == 0:
        return None
    if _u64_text(token.get("generation")) in (None, 0):
        return None
    return token


def _rust_epoch_text_from(result: object) -> str | None:
    """One canonical session-epoch text answered by connect or reset."""
    epoch = _rust_envelope_value(result)
    if not isinstance(epoch, str) or _u64_text(epoch) in (None, 0):
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
    _rust_token: dict[str, object] | None
    _rust_epoch: str | None
    _rust_frame: dict[str, object] | None
    _rust_families: FamilyTable | None
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
        return _activate_rust(
            self,
            catalog_path,
            bridge,
            epoch,
            decoded[0],
            decoded[1],
            decoded[2],
        ).to_json()

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

    def __init__(
        self,
        versions: dict[str, tuple[int, int]],
        descriptors: dict[str, tuple[int, int, int, int, int]] | None = None,
    ) -> None:
        self.versions = versions
        self.descriptors = descriptors if descriptors is not None else {}


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
    reasons: dict[str, str] = {}
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
            reasons[feature_id] = message
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
            reasons[feature_id] = f"feature {feature_id} {failure}"
            disabled.append(feature_id)
            continue
        if instance is not None:
            host._instances[feature_id] = instance
            host._active_order.append(feature_id)
    return PlanResult(True, (), tuple(sorted(set(disabled))), plan.order, reasons)


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


def _rust_declared_descriptors() -> dict[str, tuple[int, int, int, int, int]] | None:
    """Read producer identity from the existing catalog, never feature identities."""
    registry = _load_resource(RUST_REGISTRY_PATH)
    if registry is None or _metadata(registry, "rust_producer", "") != RUST_PRODUCER_NAME:
        return None
    descriptors: dict[str, tuple[int, int, int, int, int]] = {}
    for token in _metadata_strings(registry, "rust_producer_families"):
        parts = token.split(":")
        if len(parts) != 4:
            return None
        name_version = parts[0].rsplit("@", 1)
        if len(name_version) != 2:
            return None
        name, version_text = name_version
        version = _parse_version(version_text)
        numbers = tuple(_u64_text(part) for part in parts[1:])
        if not name or name in descriptors or version is None or any(n is None for n in numbers):
            return None
        numeric_id, record_limit, record_bytes = cast(tuple[int, int, int], numbers)
        descriptors[name] = (numeric_id, version[0], version[1], record_limit, record_bytes)
    return descriptors if descriptors else None


def _rust_family_table(bridge: Node, token: dict[str, object]) -> tuple[FamilyTable | None, str]:
    """Match the entire producer table to the frozen catalog before activation."""
    result = bridge.call("family_table", token)
    value = _rust_envelope_value(result)
    if value is None:
        return None, f"the rust core did not answer its family table ({_rust_error_class(result)})"
    if (
        not isinstance(value, dict)
        or not _exact_fields(value, ("producer", "descriptors"))
        or value["producer"] != RUST_PRODUCER_NAME
    ):
        return None, "the rust producer identity is not the frozen rust-client-core table"
    declared = _rust_declared_descriptors()
    if declared is None:
        return None, "the declared rust producer table is unavailable"
    descriptors = value["descriptors"]
    if not isinstance(descriptors, list) or not descriptors or len(descriptors) > len(declared):
        return None, "the rust family table has an invalid descriptor count"
    versions: dict[str, tuple[int, int]] = {}
    accepted: dict[str, tuple[int, int, int, int, int]] = {}
    ids: set[int] = set()
    for descriptor in descriptors:
        if not isinstance(descriptor, dict) or not _exact_fields(
            descriptor,
            ("logical_name", "numeric_id", "major", "minor", "record_limit", "record_bytes"),
        ):
            return None, "the rust family table reported an unreadable descriptor"
        name = descriptor["logical_name"]
        if not isinstance(name, str) or not name:
            return None, "the rust family table reported an unnamed descriptor"
        if name in versions:
            return None, f"the rust family table reports family {name} twice"
        major = _u16_word(descriptor["major"])
        minor = _u16_word(descriptor["minor"])
        numeric_id = _u16_word(descriptor["numeric_id"])
        record_limit = _u64_text(descriptor["record_limit"])
        record_bytes = _u64_text(descriptor["record_bytes"])
        if (
            major is None
            or minor is None
            or numeric_id in (None, 0)
            or record_limit is None
            or record_bytes is None
        ):
            return None, f"the rust family table reported invalid fields for {name}"
        identity = (numeric_id, major, minor, record_limit, record_bytes)
        if numeric_id in ids or identity != declared.get(name):
            return None, f"the rust family table disagrees with the declared descriptor for {name}"
        ids.add(numeric_id)
        versions[name] = (major, minor)
        accepted[name] = identity
    if set(accepted) != set(declared):
        return None, "the rust family table is missing a declared descriptor"
    return FamilyTable(versions, accepted), ""


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
    families: FamilyTable | None,
    required: tuple[tuple[str, int, int], ...],
    previous: dict[str, object] | None = None,
) -> dict[str, object] | None:
    """Validate the complete publication before replacing or dispatching any value."""
    frame = _rust_envelope_value(result)
    if not isinstance(frame, dict) or not _exact_fields(
        frame,
        (
            "layout_major",
            "layout_minor",
            "session_epoch",
            "confirmed_revision",
            "frame_index",
            "families",
        ),
    ):
        return None
    layout = (_u16_word(frame["layout_major"]), _u16_word(frame["layout_minor"]))
    if layout != RUST_FRAME_LAYOUT or frame["session_epoch"] != epoch:
        return None
    revision = _u64_text(frame["confirmed_revision"])
    index = _u64_text(frame["frame_index"])
    if _u64_text(frame["session_epoch"]) in (None, 0) or revision is None or index is None:
        return None
    if previous is not None and (
        revision < cast(int, _u64_text(previous["confirmed_revision"]))
        or index < cast(int, _u64_text(previous["frame_index"]))
    ):
        return None
    families_list = frame["families"]
    if (
        families is None
        or not isinstance(families_list, list)
        or not families_list
        or len(families_list) > len(families.descriptors)
    ):
        return None
    seen: set[str] = set()
    total_bytes = 36
    for family in families_list:
        if not isinstance(family, dict) or not _exact_fields(family, ("key", "records")):
            return None
        records = family["records"]
        key = family["key"]
        if not isinstance(key, dict) or not _exact_fields(key, ("logical_name", "major", "minor")):
            return None
        name = key["logical_name"]
        major = _u16_word(key["major"])
        minor = _u16_word(key["minor"])
        if not isinstance(name, str) or name in seen:
            return None
        descriptor = families.descriptors.get(name)
        if descriptor is None or (major, minor) != families.versions[name]:
            return None
        if not isinstance(records, list) or len(records) > descriptor[3]:
            return None
        if revision == 0 and records and descriptor[0] not in (1, 2, 9, 10):
            return None
        total_bytes += _utf8_size(name) + 13
        seen.add(name)
        for record in records:
            if (
                not isinstance(record, dict)
                or len(record) > 11
                or not _rust_record_payload(record, descriptor[0])
            ):
                return None
            header = record.get("header")
            if not isinstance(header, dict) or not _exact_fields(
                header, ("epoch", "revision", "source_tick", "operation")
            ):
                return None
            if header["epoch"] != epoch or header["revision"] != frame["confirmed_revision"]:
                return None
            if header["source_tick"] is not None and _u64_text(header["source_tick"]) is None:
                return None
            if header["operation"] not in ("Upsert", "Remove"):
                return None
            total_bytes += _rust_record_size(record, descriptor[0])
            if total_bytes > _RUST_FRAME_BYTES:
                return None
    if total_bytes > _RUST_FRAME_BYTES or seen != set(families.descriptors):
        return None
    for name, required_major, required_minor in required:
        registered = families.versions.get(name)
        if (
            name not in seen
            or registered is None
            or required_major != registered[0]
            or required_minor > registered[1]
        ):
            return None
    return frame


def _rust_step_from(result: object, epoch: str) -> dict[str, object] | None:
    report = _rust_envelope_value(result)
    if not isinstance(report, dict) or not _exact_fields(
        report,
        (
            "epoch",
            "confirmed_revision",
            "frame_index",
            "processed_messages",
            "processed_meshes",
            "pending_input",
            "pending_inbound",
            "pending_preparation",
            "terminal",
        ),
    ):
        return None
    if report["epoch"] != epoch:
        return None
    if any(
        _u64_text(report[field]) is None for field in ("epoch", "confirmed_revision", "frame_index")
    ):
        return None
    for field, maximum in (
        ("processed_messages", RUST_STEP_MESSAGE_BUDGET),
        ("processed_meshes", RUST_STEP_MESH_BUDGET),
        ("pending_input", 128),
        ("pending_inbound", 8192),
        ("pending_preparation", 4096),
    ):
        if not _bounded_integer(report[field], maximum):
            return None
    return report if _rust_terminal(report["terminal"]) else None


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
    if _rust_step_from(step_result, epoch) is None:
        return 0
    pull_result = bridge.call("pull_typed_frame", token, epoch)
    frame = _rust_frame_from(
        pull_result, epoch, host._rust_families, host._rust_required, host._rust_frame
    )
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
    host._rust_families = table
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
