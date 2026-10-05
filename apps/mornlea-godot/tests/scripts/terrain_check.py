"""Drive the production catalog through the transcript terrain scenario.

This is the headless terrain check: it activates the production feature
catalog (including session, input, actors, view, HUD, and world), connects
the pilot session to the transcript
helper started by the gate script, and steps the session while the world
feature's own per-frame typed calls ingest world batches and drive the
terrain stage. After each scenario stage it asserts the bridge's structural
terrain summary: the initial snapshot's section appears at its first
revision, the chunk delta advances revisions and adds the delta section,
the forget stage leaves no live section (the no-stale proof), the server
disconnect plus the feature-owned close resets the stage, and the re-entry
round repopulates the same section from revision one under a fresh session.

Every wait is a bounded frame-budget poll with a named failure line; the
driver never sleeps and never reads raw records.
"""

from __future__ import annotations

import json
from typing import Any, Protocol, TypedDict, cast, runtime_checkable

from py4godot.classes import gdclass
from py4godot.classes.Node import Node
from py4godot.classes.OS import OS
from py4godot.utils.smart_cast import (  # type: ignore[import-not-found]
    register_cast_function,
)

# The generated Py4Godot cast table only knows engine classes; the identity
# cast lets this driver hold the native bridge node for its typed calls,
# mirroring the other check drivers.
register_cast_function("MornleaClientBridge", lambda bridge: bridge)

PORT_VARIABLE = "MORNLEA_TERRAIN_CHECK_PORT"
# Generous but finite per-stage frame budgets: one second of helper pacing
# plus mesh and step convergence fits comfortably inside 3600 frames at the
# engine's frame cadence, and the gate's own deadline bounds the run.
STAGE_FRAME_BUDGET = 3600
STEP_ELAPSED_NS = 16_666_667
STEP_MESSAGE_BUDGET = 64
STEP_MESH_BUDGET = 64

PLAY_TEXT = "Play"
DISCONNECTED_TEXT = "Disconnected"


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


def _word(value: object) -> int:
    if isinstance(value, int) and not isinstance(value, bool):
        return value
    return -1


@runtime_checkable
class _DictionaryView(Protocol):
    """Minimal typed view of the bridge's typed result dictionaries."""

    def __getitem__(self, key: str) -> object: ...


def _section_key(entry: SectionEntry) -> tuple[int, int, int, int]:
    return (entry["dimension"], entry["x"], entry["y"], entry["z"])


def _describe_status(result: _DictionaryView) -> str:
    return (
        f"phase={result['phase']} terminal_cause={result['terminal_cause']}"
        f" steps_completed={result['steps_completed']}"
        f" messages_processed={result['messages_processed']}"
    )


@gdclass
class terrain_check(Node):
    """Frame-driven scenario driver over the production catalog."""

    _failures: list[str]
    _host: Node | None
    _bridge: Node | None
    _session: Node | None
    _feature: Node | None
    _stage: str
    _frames: int
    _address: str
    _initial_revisions: dict[tuple[int, int, int, int], int]
    _last_summary: str
    _steps_ok: int
    _first_step_error: int
    _last_status: str

    def _ready(self) -> None:
        self._failures = []
        self._host = self.get_node("FeatureHost")
        self._bridge = self.get_node_or_null("ClientBridge")
        self._session = None
        self._feature = None
        self._stage = "activate"
        self._frames = 0
        self._address = ""
        self._initial_revisions = {}
        self._last_summary = ""
        self._steps_ok = 0
        self._first_step_error = -1
        self._last_status = ""
        if self._bridge is None:
            self._failures.append("the typed bridge node is missing from the check scene")
            self._finish(1)
            return
        port_text = OS.instance().get_environment(PORT_VARIABLE)
        if not isinstance(port_text, str) or not port_text.isdigit():
            self._failures.append(f"the {PORT_VARIABLE} environment variable is missing")
            self._finish(1)
            return
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
            host.call(
                "activate_catalog",
                "res://config/feature_catalog.tres",
                "../ClientBridge",
                41,
            ),
        )
        result = cast(PlanResult, json.loads(raw))
        if not result["ok"]:
            self._failures.append(f"production catalog activation failed: {result}")
            self._finish(1)
            return
        if result["order"] != [
            "session",
            "actors",
            "platform.desktop.input",
            "player_view",
            "ui",
            "world",
        ]:
            self._failures.append(f"activation order differs: {result['order']}")
        if "world" in result["disabled"]:
            self._failures.append("the world feature was disabled by the plan")
        if cast(int, host.call("active_count")) != 6:
            self._failures.append("the production features did not stay active")
        feature = host.get_node_or_null("WorldFeature")
        if feature is None:
            self._failures.append("the active world feature instance is missing")
            self._finish(1)
            return
        self._feature = feature
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
        # The driver owns the bounded step; the world feature's own
        # `_process` performs its two typed terrain calls in the same frame.
        bridge = self._bridge
        if bridge is not None:
            step_word = _word(
                bridge.call("session_step", STEP_ELAPSED_NS, STEP_MESSAGE_BUDGET, STEP_MESH_BUDGET)
            )
            if step_word == 0:
                self._steps_ok += 1
            elif self._first_step_error == -1:
                self._first_step_error = step_word
            typed = bridge.call("session_status_typed")
            if isinstance(typed, _DictionaryView) and _word(typed["status"]) == 0:
                self._last_status = _describe_status(typed)
        summary = self._summary()
        phase = self._call_text(session, "phase_text")
        session.call("refresh")

        if self._stage == "wait-play":
            if phase == PLAY_TEXT:
                self._enter_stage("wait-initial")
            elif phase == DISCONNECTED_TEXT:
                error_text = self._call_text(session, "error_text")
                self._failures.append(f"the session reached terminal before play: {error_text}")
        elif self._stage == "wait-initial":
            sections = self._live_sections(summary)
            floor = sections.get((0, 0, 0, 0))
            if floor is not None:
                if floor["revision"] != 1:
                    self._failures.append(
                        f"the initial floor section revision is {floor['revision']}, want 1"
                    )
                if floor["surfaces"] < 1:
                    self._failures.append("the initial floor section carries no surface")
                self._initial_revisions = {
                    key: entry["revision"] for key, entry in sections.items()
                }
                self._enter_stage("wait-delta")
        elif self._stage == "wait-delta":
            sections = self._live_sections(summary)
            floor = sections.get((0, 0, 0, 0))
            delta = sections.get((0, 0, 1, 0))
            if floor is not None and delta is not None:
                if floor["revision"] < self._initial_revisions.get((0, 0, 0, 0), 0):
                    self._failures.append("the delta stage lost the floor section revision")
                if delta["revision"] < 1:
                    self._failures.append("the delta section carries no revision")
                self._enter_stage("wait-forget")
        elif self._stage == "wait-forget":
            if not self._live_sections(summary):
                self._enter_stage("wait-disconnect")
        elif self._stage == "wait-disconnect":
            if phase == DISCONNECTED_TEXT:
                self._assert_reset_and_reenter(summary)
                return
        elif self._stage == "wait-reentry-play":
            if phase == PLAY_TEXT:
                self._enter_stage("wait-reentry")
            elif phase == DISCONNECTED_TEXT:
                error_text = self._call_text(session, "error_text")
                self._failures.append(
                    f"the re-entry session reached terminal before play: {error_text}"
                )
        elif self._stage == "wait-reentry":
            sections = self._live_sections(summary)
            floor = sections.get((0, 0, 0, 0))
            if floor is not None:
                if floor["revision"] != 1:
                    self._failures.append(
                        f"the re-entry floor revision is {floor['revision']}, want a fresh 1"
                    )
                self._conclude()
                return

        if self._frames > STAGE_FRAME_BUDGET:
            self._failures.append(f"stage {self._stage} exceeded its frame budget")
        if self._failures:
            self._finish(1)

    def _assert_reset_and_reenter(self, summary: dict[str, Any]) -> None:
        # The feature-owned clean close rides the bridge session close, which
        # resets the terrain stage: no stale section may survive it, the
        # reset counter advances, and the epoch is strictly monotonic.
        session = self._session
        if session is None:
            self._failures.append("the session feature vanished before the reset")
            self._finish(1)
            return
        session.call("close_connection")
        after = self._summary()
        if self._live_sections(after):
            self._failures.append("sections survived the disconnect-and-close reset")
        facts = cast(dict[str, int], after.get("facts", {}))
        if facts.get("resets", 0) < 1:
            self._failures.append("the terrain stage did not record its reset")
        if facts.get("epoch", 0) < 2:
            self._failures.append("the terrain epoch is not strictly monotonic across reset")
        if self._failures:
            self._finish(1)
            return
        begin = self._call_text(session, "request_connect", self._address)
        if begin:
            self._failures.append(f"re-entry connection begin failed: {begin}")
            self._finish(1)
            return
        self._frames = 0
        self._enter_stage("wait-reentry-play")

    def _conclude(self) -> None:
        session = self._session
        if session is not None:
            session.call("close_connection")
        host = self._host
        if host is not None:
            host.call("deactivate_features")
            if cast(int, host.call("active_count")) != 0:
                self._failures.append("deactivation left features active")
        if self._failures:
            self._finish(1)
            return
        print("Python terrain check passed.")
        self._finish(0)

    def _summary(self) -> dict[str, Any]:
        bridge = self._bridge
        if bridge is None:
            return {}
        text = bridge.call("terrain_sections_json")
        if not isinstance(text, str) or not text:
            self._failures.append("the terrain structural summary is unavailable")
            return {}
        parsed = json.loads(text)
        if not isinstance(parsed, dict):
            self._failures.append("the terrain structural summary is not an object")
            return {}
        if _word(parsed.get("status")) != 0:
            self._failures.append("the terrain structural summary reported a failure status")
            return {}
        self._last_summary = text
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
                live[_section_key(cast(SectionEntry, entry))] = cast(SectionEntry, entry)
        return live

    def _enter_stage(self, stage: str) -> None:
        self._stage = stage
        self._frames = 0

    def _finish(self, exit_code: int) -> None:
        self._stage = "done"
        if self._failures:
            for failure in self._failures:
                print(f"Python terrain check failed: {failure}")
            print(f"Python terrain check state: stage={self._stage} frames={self._frames}")
            print(
                f"Python terrain check step history: ok={self._steps_ok}"
                f" first_error={self._first_step_error}"
            )
            print(f"Python terrain check status: {self._last_status}")
            print(f"Python terrain check summary: {self._last_summary}")
        self.call_deferred("_quit", exit_code)

    def _quit(self, exit_code: int) -> None:
        self.get_tree().quit(exit_code)

    def _call_text(self, target: Node, method: str, *arguments: object) -> str:
        value = target.call(method, *arguments)
        return value if isinstance(value, str) else ""
