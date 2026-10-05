"""Reject empty viewport evidence independently from native upload counts."""

from __future__ import annotations

import ast
import unittest
from collections import Counter
from collections.abc import Callable
from pathlib import Path
from typing import cast


def load_visibility_check() -> Callable[[list[tuple[int, int, int]]], str]:
    # Load only the pure image predicate; importing the scene requires Godot.
    source = Path(__file__).parent / "scripts" / "visual_capture.py"
    tree = ast.parse(source.read_text(encoding="utf-8"))
    functions: list[ast.stmt] = [
        node
        for node in tree.body
        if isinstance(node, ast.FunctionDef) and node.name == "_visible_terrain_failure"
    ]
    namespace: dict[str, object] = {"Counter": Counter}
    exec(compile(ast.Module(body=functions, type_ignores=[]), str(source), "exec"), namespace)
    return cast(Callable[[list[tuple[int, int, int]]], str], namespace["_visible_terrain_failure"])


class VisualCaptureVisibilityTests(unittest.TestCase):
    def test_blank_frame_is_rejected(self) -> None:
        check = load_visibility_check()
        self.assertIn("blank", check([(5, 8, 20)] * 960))

    def test_hud_or_tiny_noisy_region_cannot_count_as_terrain(self) -> None:
        check = load_visibility_check()
        samples = [(5, 8, 20)] * 940 + [(value, value, value) for value in range(20)]
        self.assertIn("blank", check(samples))

    def test_textured_terrain_region_is_visible(self) -> None:
        check = load_visibility_check()
        samples = [(5, 8, 20)] * 480
        samples += [(32 + index % 16,) * 3 for index in range(480)]
        self.assertEqual("", check(samples))
