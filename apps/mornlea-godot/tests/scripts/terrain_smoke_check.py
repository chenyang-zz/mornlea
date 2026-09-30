"""Drive the production catalog against the real TCP dedicated server.

This is the real-server terrain smoke: it activates the production feature
catalog (including session, input, actors, view, HUD, and world), connects
the pilot session to a live
`mornlea-server` started by the gate script on loopback, waits for the core
of the transcript terrain check's initial-snapshot criterion — confirmed
session phase Play plus at least one live terrain section in the bridge's
structural summary — deliberately without that gate's recorded-scenario
revision and section-coordinate pins, which a live authoritative stream
cannot promise, then runs a fixed number of frames before the clean
feature-owned close.

Per driven frame this driver owns the bounded step plus the two typed terrain
calls and reads their typed result words. Reading the results is the point:
per-frame terrain error words are invisible otherwise, so the driver fails
loudly on any error word once the loaded criterion has been reached. Before
that point a transient error word can occur while the login handoff is still
converging, so pre-load words are recorded as bounded evidence and tolerated.

Because the scene tree processes this root node before the feature host
subtree, this driver's `terrain_ingest_world` and `terrain_frame` calls run
ahead of the world feature's own per-frame pair. The feature's later ingest
observes the bridge's documented empty no-batch outcome (the world pull is
consuming), while its later frame call re-serves the identical non-consuming
snapshot and drives the stage once more — harmless by the stage's fixed
per-drive budgets and exactly-once result consumption, but it means the
stage's frame counter runs at roughly twice the rendered-frame rate while
this driver is active.

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

ADDRESS_VARIABLE = "MORNLEA_TERRAIN_SMOKE_ADDRESS"
# Generous but finite login-to-terrain budget: a real server generates the
# spawn area on demand while the pilot's minimum login view distance keeps
# the streamed radius small, so 7200 frames (two minutes at the engine frame
# cadence) covers slow first-generation runs while the gate's own deadline
# bounds the whole scene.
LOADED_FRAME_BUDGET = 7200
# The fixed smoke run: 300 frames at the pilot's 16.67 ms step cadence is
# five simulated seconds of authoritative streaming, enough for many
# ingest/prepare/upload cycles while keeping the smoke bounded and its frame
# count independent of machine speed.
RUN_FRAMES = 300
STEP_ELAPSED_NS = 16_666_667
STEP_MESSAGE_BUDGET = 64
STEP_MESH_BUDGET = 64
# Only the first few transient pre-load error words are kept as evidence so a
# wedged handoff cannot grow an unbounded failure list.
TRANSIENT_EVIDENCE_LIMIT = 4

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


def _section_count(summary: dict[str, Any]) -> int:
    entries = summary.get("sections")
    if not isinstance(entries, list):
        return 0
    return sum(1 for entry in entries if isinstance(entry, dict))


def _describe_status(result: _DictionaryView) -> str:
    return (
        f"phase={result['phase']} terminal_cause={result['terminal_cause']}"
        f" steps_completed={result['steps_completed']}"
        f" messages_processed={result['messages_processed']}"
    )


@gdclass
class terrain_smoke_check(Node):
    """Frame-driven smoke driver over the production catalog and a real server."""

    _failures: list[str]
    _host: Node | None
    _bridge: Node | None
    _session: Node | None
    _feature: Node | None
    _stage: str
    _frames: int
    _address: str
    _last_summary: str
    _steps_ok: int
    _first_step_error: int
    _last_status: str
    _transient_words: list[str]
    _loaded_sections: int
    _ingest_operations: int
    _post_load_error: str

    def _ready(self) -> None:
        self._failures = []
        self._host = self.get_node("FeatureHost")
        self._bridge = self.get_node_or_null("ClientBridge")
        self._session = None
        self._feature = None
        self._stage = "activate"
        self._frames = 0
        self._address = ""
        self._last_summary = ""
        self._steps_ok = 0
        self._first_step_error = -1
        self._last_status = ""
        self._transient_words = []
        self._loaded_sections = 0
        self._ingest_operations = 0
        self._post_load_error = ""
        if self._bridge is None:
            self._failures.append("the typed bridge node is missing from the smoke scene")
            self._finish(1)
            return
        address = OS.instance().get_environment(ADDRESS_VARIABLE)
        if not isinstance(address, str) or ":" not in address:
            self._failures.append(
                f"the {ADDRESS_VARIABLE} environment variable is missing a host:port address"
            )
            self._finish(1)
            return
        self._address = address
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
        phase = self._call_text(session, "phase_text")
        if self._stage == "wait-play":
            if phase == PLAY_TEXT:
                self._enter_stage("wait-loaded")
            elif phase == DISCONNECTED_TEXT:
                error_text = self._call_text(session, "error_text")
                self._failures.append(
                    f"the real-server session reached terminal before play: {error_text}"
                )
        elif self._stage == "wait-loaded":
            if phase == DISCONNECTED_TEXT:
                error_text = self._call_text(session, "error_text")
                self._failures.append(
                    f"the real-server session reached terminal before terrain: {error_text}"
                )
        elif self._stage == "run" and phase != PLAY_TEXT:
            error_text = self._call_text(session, "error_text")
            self._failures.append(
                f"the real-server session left play during the fixed run: {phase} ({error_text})"
            )
        if self._failures:
            self._finish(1)
            return
        self._drive_frame()
        if self._failures:
            self._finish(1)
            return
        summary = self._summary()
        if self._failures:
            self._finish(1)
            return
        session.call("refresh")
        if self._stage == "wait-loaded":
            sections = _section_count(summary)
            # The loaded criterion mirrors the transcript terrain check's
            # initial-snapshot gate: confirmed phase Play plus at least one
            # live terrain section in the structural summary.
            if sections >= 1:
                self._loaded_sections = sections
                self._enter_stage("run")
        elif self._stage == "run":
            if self._frames >= RUN_FRAMES:
                self._conclude()
                return
        if self._frames > LOADED_FRAME_BUDGET:
            self._failures.append(f"stage {self._stage} exceeded its frame budget")
            self._finish(1)

    def _drive_frame(self) -> None:
        # The per-frame surface: one bounded step plus the two typed terrain
        # calls, with every typed status word read. Error words are tolerated
        # (as bounded evidence) only before the loaded criterion is reached;
        # from the loaded frame on, any non-zero word is a loud failure.
        bridge = self._bridge
        if bridge is None:
            return
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
        for name in ("terrain_ingest_world", "terrain_frame"):
            result = bridge.call(name)
            if not isinstance(result, _DictionaryView):
                self._failures.append(f"the {name} result was not typed")
                return
            status = _word(result["status"])
            if status == 0:
                if name == "terrain_ingest_world":
                    self._ingest_operations += _word(result["operations"])
                continue
            if self._stage == "run":
                self._post_load_error = f"{name} status {status}"
                self._failures.append(
                    f"a terrain error word reached the fixed run: {self._post_load_error}"
                )
                return
            if len(self._transient_words) < TRANSIENT_EVIDENCE_LIMIT:
                self._transient_words.append(f"{name} status {status}")

    def _conclude(self) -> None:
        # The clean close rides the session feature's close_connection path;
        # the terrain stage reset must leave no live section (the no-stale
        # pattern), record its reset, and keep the epoch strictly monotonic.
        session = self._session
        if session is not None:
            session.call("close_connection")
        after = self._summary()
        if self._failures:
            self._finish(1)
            return
        if _section_count(after) != 0:
            self._failures.append("sections survived the clean close")
        facts = cast(dict[str, int], after.get("facts", {}))
        if facts.get("resets", 0) < 1:
            self._failures.append("the terrain stage did not record its reset")
        if facts.get("epoch", 0) < 2:
            self._failures.append("the terrain epoch is not strictly monotonic across the close")
        host = self._host
        if host is not None:
            host.call("deactivate_features")
            if cast(int, host.call("active_count")) != 0:
                self._failures.append("deactivation left features active")
        if self._failures:
            self._finish(1)
            return
        print(
            f"Python terrain smoke passed: loaded {self._loaded_sections} sections,"
            f" ran {RUN_FRAMES} frames with {self._ingest_operations} ingested operations."
        )
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

    def _enter_stage(self, stage: str) -> None:
        self._stage = stage
        self._frames = 0

    def _finish(self, exit_code: int) -> None:
        self._stage = "done"
        if self._failures:
            for failure in self._failures:
                print(f"Python terrain smoke failed: {failure}")
            print(f"Python terrain smoke state: stage={self._stage} frames={self._frames}")
            print(
                f"Python terrain smoke step history: ok={self._steps_ok}"
                f" first_error={self._first_step_error}"
            )
            print(
                f"Python terrain smoke transient words: {self._transient_words}"
                f" loaded_sections={self._loaded_sections}"
                f" ingest_operations={self._ingest_operations}"
                f" post_load_error={self._post_load_error}"
            )
            print(f"Python terrain smoke status: {self._last_status}")
            print(f"Python terrain smoke summary: {self._last_summary}")
        self.call_deferred("_quit", exit_code)

    def _quit(self, exit_code: int) -> None:
        self.get_tree().quit(exit_code)

    def _call_text(self, target: Node, method: str, *arguments: object) -> str:
        value = target.call(method, *arguments)
        return value if isinstance(value, str) else ""
