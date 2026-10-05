"""Capture one fixed-camera world frame without a foreground window."""

from __future__ import annotations

import json
from collections import Counter
from pathlib import Path
from typing import Any, Protocol, TypedDict, cast, runtime_checkable

from py4godot.classes import gdclass
from py4godot.classes.core import Vector3
from py4godot.classes.DisplayServer import DisplayServer
from py4godot.classes.Node import Node
from py4godot.classes.OS import OS
from py4godot.classes.RenderingServer import RenderingServer
from py4godot.utils.smart_cast import (  # type: ignore[import-not-found]
    register_cast_function,
)

register_cast_function("MornleaClientBridge", lambda bridge: bridge)

PORT_VARIABLE = "MORNLEA_GODOT_CAPTURE_PORT"
DIR_VARIABLE = "MORNLEA_GODOT_CAPTURE_DIR"
RUN_ID_VARIABLE = "MORNLEA_GODOT_CAPTURE_RUN_ID"
COMMIT_VARIABLE = "MORNLEA_GIT_COMMIT"
WORKTREE_VARIABLE = "MORNLEA_WORKTREE_STATE"
GODOT_VERSION_VARIABLE = "MORNLEA_GODOT_VERSION"
PY4GODOT_VERSION_VARIABLE = "MORNLEA_PY4GODOT_VERSION"
PY4GODOT_REVISION_VARIABLE = "MORNLEA_PY4GODOT_SOURCE_REVISION"
CPYTHON_VARIABLE = "MORNLEA_CPYTHON_VERSION"
CATALOG_PATH = "res://config/feature_catalog.tres"
IMAGE_RELATIVE = "world/terrain-settled.png"
STAGE_FRAME_BUDGET = 3600
SETTLE_FRAMES = 8
PLAY_TEXT = "Play"


class PlanResult(TypedDict):
    ok: bool
    errors: list[str]
    disabled: list[str]
    order: list[str]


class SectionEntry(TypedDict):
    dimension: int
    x: int
    y: int
    z: int
    revision: int
    surfaces: int


@runtime_checkable
class _DictionaryView(Protocol):
    def __getitem__(self, key: str) -> object: ...


def _word(value: object) -> int:
    if isinstance(value, int) and not isinstance(value, bool):
        return value
    return -1


def _env(name: str) -> str:
    value = OS.instance().get_environment(name)
    return value if isinstance(value, str) else ""


def _visible_terrain_failure(samples: list[tuple[int, int, int]]) -> str:
    # Sample the viewport interior so HUD labels cannot stand in for terrain.
    # Uploaded RID counts prove submission, but only pixel variation proves
    # the fixed world camera actually sees textured geometry.
    colors = Counter(samples)
    if not samples or len(colors) < 8:
        return "the captured world interior is blank or lacks visible terrain texture"
    background_count = colors.most_common(1)[0][1]
    if (len(samples) - background_count) * 20 < len(samples):
        return "the captured world interior is blank: terrain covers less than five percent"
    return ""


def _hide_window() -> None:
    # Headless display only exposes the dummy renderer, which cannot produce a
    # viewport image. Use a no-focus minimized window so capture stays off the
    # foreground without falling back to dummy textures.
    server = DisplayServer.instance()
    server.window_set_flag(4, True)
    server.window_set_mode(1)


@gdclass
class visual_capture(Node):
    """Drive a production catalog to Play, pin a fixed camera, and capture."""

    _failures: list[str]
    _host: Node | None
    _bridge: Node | None
    _session: Node | None
    _camera: Node | None
    _stage: str
    _frames: int
    _settle: int
    _address: str
    _run_dir: Path

    def _ready(self) -> None:
        _hide_window()
        self._failures = []
        self._host = self.get_node("FeatureHost")
        self._bridge = self.get_node_or_null("ClientBridge")
        self._session = None
        self._camera = self.get_node_or_null("CaptureCamera")
        self._stage = "activate"
        self._frames = 0
        self._settle = 0
        self._address = ""
        self._run_dir = Path()
        if self._bridge is None:
            self._failures.append("the typed bridge node is missing from the capture scene")
            self._finish(1)
            return
        if self._camera is None:
            self._failures.append("the fixed CaptureCamera is missing from the capture scene")
            self._finish(1)
            return
        port_text = _env(PORT_VARIABLE)
        if not port_text.isdigit():
            self._failures.append(f"the {PORT_VARIABLE} environment variable is missing")
            self._finish(1)
            return
        run_dir = Path(_env(DIR_VARIABLE))
        if not run_dir.is_absolute():
            self._failures.append("the capture directory must be an absolute path")
            self._finish(1)
            return
        if "testdata/visual-golden" in run_dir.as_posix():
            self._failures.append("pilot evidence must stay outside tracked visual baselines")
            self._finish(1)
            return
        self._run_dir = run_dir
        self._address = f"127.0.0.1:{int(port_text)}"
        self._activate()

    def _activate(self) -> None:
        host = self._host
        if host is None:
            self._failures.append("the feature host node is missing")
            self._finish(1)
            return
        raw = cast(
            str,
            host.call("activate_catalog", CATALOG_PATH, "../ClientBridge", 41),
        )
        result = cast(PlanResult, json.loads(raw))
        if not result["ok"]:
            self._failures.append(f"production catalog activation failed: {result}")
            self._finish(1)
            return
        if "session" not in result["order"] or "world" not in result["order"]:
            self._failures.append(f"session or world is missing from the plan: {result['order']}")
        session = host.get_node_or_null("SessionFeature")
        if session is None or not session.has_method("request_connect"):
            self._failures.append("the active session feature lacks request_connect")
            self._finish(1)
            return
        self._session = session
        begin = self._call_text(session, "request_connect", self._address)
        if begin:
            self._failures.append(f"connection begin failed: {begin}")
        if self._failures:
            self._finish(1)
            return
        self._enter_stage("wait-play")

    def _process(self, _delta: float) -> None:
        session = self._session
        if session is None or self._stage == "done" or self._failures:
            return
        self._frames += 1
        phase = self._call_text(session, "phase_text")
        session.call("refresh")
        summary = self._summary()
        if self._stage == "wait-play":
            if phase == PLAY_TEXT:
                self._enter_stage("wait-terrain")
            elif phase == "Disconnected":
                error_text = self._call_text(session, "error_text")
                self._failures.append(f"disconnected before play: {error_text}")
        elif self._stage == "wait-terrain":
            sections = self._live_sections(summary)
            floor = sections.get((0, 0, 0, 0))
            if floor is not None and floor["surfaces"] >= 1:
                self._enter_stage("wait-settle")
        elif self._stage == "wait-settle":
            self._settle += 1
            self.call_deferred("_pin_camera")
            if self._settle >= SETTLE_FRAMES:
                self._stage = "capturing"
                self.call_deferred("_capture_now")
                return
        elif self._stage == "capturing":
            return
        if self._frames > STAGE_FRAME_BUDGET:
            self._failures.append(f"stage {self._stage} exceeded its frame budget")
        if self._failures:
            self._finish(1)

    def _pin_camera(self) -> None:
        camera = self._camera
        if camera is None:
            self._failures.append("the fixed CaptureCamera vanished before capture")
            return
        camera.call("set_position", Vector3.new3(0.5, -24.0, 12.0))
        camera.call("look_at", Vector3.new3(8.0, -56.0, 8.0), Vector3.new3(0.0, 1.0, 0.0))
        camera.call("set_current", True)

    def _capture_now(self) -> None:
        self._capture(self._summary())

    def _capture(self, summary: dict[str, Any]) -> None:
        if self._stage == "done" or self._failures:
            return
        facts = summary.get("facts")
        floor = self._live_sections(summary).get((0, 0, 0, 0))
        if (
            not isinstance(facts, dict)
            or floor is None
            or floor["surfaces"] < 1
            or _word(facts.get("live_surfaces")) < 1
            or _word(facts.get("uploads_failed")) != 0
        ):
            self._failures.append("the uploaded floor is not live and error-free at capture")
            self._finish(1)
            return
        image_path = self._run_dir / IMAGE_RELATIVE
        image_path.parent.mkdir(parents=True, exist_ok=True)
        viewport = self.get_viewport()
        texture = viewport.call("get_texture")
        if texture is None:
            self._failures.append("the viewport texture is unavailable")
            self._finish(1)
            return
        image = cast(Any, texture).call("get_image")
        if image is None:
            self._failures.append("the viewport image is empty")
            self._finish(1)
            return
        current_camera = viewport.call("get_camera_3d")
        camera = self._camera
        if (
            current_camera is None
            or camera is None
            or cast(Any, current_camera).call("get_instance_id") != camera.call("get_instance_id")
        ):
            self._failures.append("the fixed CaptureCamera is not the viewport's active camera")
            self._finish(1)
            return
        width = _word(cast(Any, image).call("get_width"))
        height = _word(cast(Any, image).call("get_height"))
        if width < 40 or height < 24:
            self._failures.append("the viewport image is too small to prove terrain visibility")
            self._finish(1)
            return
        samples: list[tuple[int, int, int]] = []
        for row in range(24):
            y = height // 8 + row * (height * 3 // 4 - 1) // 23
            for column in range(40):
                x = width // 8 + column * (width * 3 // 4 - 1) // 39
                color = cast(Any, image).call("get_pixel", x, y)
                samples.append((round(color.r * 255), round(color.g * 255), round(color.b * 255)))
        visibility_failure = _visible_terrain_failure(samples)
        if visibility_failure:
            self._failures.append(visibility_failure)
            self._finish(1)
            return
        save_status = cast(Any, image).call("save_png", str(image_path))
        if save_status != 0:
            self._failures.append(f"the captured PNG could not be saved: {save_status}")
            self._finish(1)
            return
        colors = Counter(samples)
        (self._run_dir / "image-metrics.json").write_text(
            json.dumps(
                {
                    "interior_samples": len(samples),
                    "distinct_colors": len(colors),
                    "nonbackground_samples": len(samples) - colors.most_common(1)[0][1],
                    "camera_position": [0.5, -24.0, 12.0],
                    "camera_target": [8.0, -56.0, 8.0],
                    "fixed_camera_current": True,
                },
                indent=2,
                sort_keys=True,
            )
            + "\n",
            encoding="utf-8",
        )
        if not image_path.is_file() or image_path.stat().st_size <= 0:
            self._failures.append("the captured PNG was not written")
            self._finish(1)
            return
        identity = self._identity()
        (self._run_dir / "identity.json").write_text(
            json.dumps(identity, indent=2, sort_keys=True) + "\n",
            encoding="utf-8",
        )
        (self._run_dir / "terrain-summary.json").write_text(
            json.dumps(summary, indent=2, sort_keys=True) + "\n",
            encoding="utf-8",
        )
        session = self._session
        if session is not None:
            session.call("close_connection")
        host = self._host
        if host is not None:
            host.call("deactivate_features")
        print("Python visual capture passed.")
        self._finish(0)

    def _identity(self) -> dict[str, object]:
        families_raw = ""
        bridge = self._bridge
        if bridge is not None:
            families_raw = self._call_text(bridge, "feature_families_json")
        families: object
        try:
            families = json.loads(families_raw) if families_raw else []
        except json.JSONDecodeError:
            families = []
        adapter = RenderingServer.instance()
        gpu_name = str(adapter.get_video_adapter_name())
        gpu_api = str(adapter.get_video_adapter_api_version())
        if not gpu_name.strip():
            gpu_name = "unknown"
        if not gpu_api.strip():
            gpu_api = "unknown"
        os_node = OS.instance()
        return {
            "schema_version": 1,
            "run_id": _env(RUN_ID_VARIABLE),
            "scenario": "world/terrain-settled",
            "git_commit": _env(COMMIT_VARIABLE),
            "worktree_state": _env(WORKTREE_VARIABLE),
            "godot_version": _env(GODOT_VERSION_VARIABLE),
            "py4godot_version": _env(PY4GODOT_VERSION_VARIABLE),
            "py4godot_source_revision": _env(PY4GODOT_REVISION_VARIABLE),
            "cpython_version": _env(CPYTHON_VARIABLE),
            "platform": {
                "os": str(os_node.get_name()).lower(),
                "arch": str(os_node.get_environment("MORNLEA_PLATFORM_ARCH") or "arm64"),
                "version": str(os_node.get_environment("MORNLEA_PLATFORM_VERSION")),
            },
            "gpu": {"name": gpu_name, "api": gpu_api},
            "catalog": CATALOG_PATH,
            "feature_families": families,
            "images": [IMAGE_RELATIVE],
        }

    def _summary(self) -> dict[str, Any]:
        bridge = self._bridge
        if bridge is None:
            return {}
        text = bridge.call("terrain_sections_json")
        if not isinstance(text, str) or not text:
            return {}
        parsed = json.loads(text)
        if not isinstance(parsed, dict) or _word(parsed.get("status")) != 0:
            return {}
        return cast(dict[str, Any], parsed)

    def _live_sections(
        self, summary: dict[str, Any]
    ) -> dict[tuple[int, int, int, int], SectionEntry]:
        entries = summary.get("sections")
        if not isinstance(entries, list):
            return {}
        live: dict[tuple[int, int, int, int], SectionEntry] = {}
        for entry in entries:
            if isinstance(entry, dict):
                key = (
                    _word(entry.get("dimension")),
                    _word(entry.get("x")),
                    _word(entry.get("y")),
                    _word(entry.get("z")),
                )
                live[key] = cast(SectionEntry, entry)
        return live

    def _call_text(self, node: Node, method: str, *arguments: object) -> str:
        value = node.call(method, *arguments) if arguments else node.call(method)
        return value if isinstance(value, str) else ""

    def _enter_stage(self, stage: str) -> None:
        self._stage = stage
        self._frames = 0

    def _finish(self, exit_code: int) -> None:
        self._stage = "done"
        for failure in self._failures:
            print(f"Python visual capture failed: {failure}")
        self.get_tree().quit(exit_code)
