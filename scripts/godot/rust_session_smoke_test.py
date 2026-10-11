import hashlib
import importlib.util
import os
import socket
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

SCRIPT = Path(__file__).with_name("rust-session-smoke.py")
SPEC = importlib.util.spec_from_file_location("rust_session_smoke", SCRIPT)
if SCRIPT.exists():
    runner = importlib.util.module_from_spec(SPEC)
    SPEC.loader.exec_module(runner)
else:
    runner = None

SOURCE = "a" * 40
CONTRACT = "b" * 64
REPORT = {
    "ok": True,
    "source_sha": SOURCE,
    "contract_sha": CONTRACT,
    "families": list(range(1, 11)),
    "resets": 1,
    "reconnects": 1,
    "inputs": 1,
    "preparation": True,
    "release_order": True,
    "retained_frame": True,
}
LOG = "\n".join(
    [
        "[mornlea-lifecycle] rust-init=scene",
        "[mornlea-lifecycle] rust-init=main-loop",
        "[mornlea-lifecycle] python-init=host",
        "[mornlea-rust-session] python-release=consumer",
        "[mornlea-rust-session] native-release=core",
        "[mornlea-rust-session] session=complete",
        "[mornlea-lifecycle] python-deinit=features",
        "[mornlea-lifecycle] python-deinit=bridge",
        "[mornlea-lifecycle] rust-deinit=main-loop",
        "[mornlea-lifecycle] rust-deinit=scene",
    ]
)


class RunnerTests(unittest.TestCase):
    def setUp(self):
        self.assertIsNotNone(runner, "Rust session smoke runner is missing")

    def test_numeric_loopback_rejects_injection_and_bad_ports(self):
        self.assertEqual(runner.loopback_address("127.0.0.1:32123\n"), 32123)
        for address in (
            "localhost:1",
            "127.0.0.1:0",
            "127.0.0.1:65536",
            "127.0.0.1:1\n2",
            "127.0.0.1:01",
        ):
            with self.subTest(address=address), self.assertRaises(ValueError):
                runner.loopback_address(address)

    def test_exact_report_rejects_boolean_counts_extra_fields_and_identity(self):
        runner.validate_report(REPORT, SOURCE, CONTRACT)
        for key, value in (
            ("inputs", True),
            ("ok", 1),
            ("families", [True] + list(range(2, 11))),
            ("resets", 0),
            ("source_sha", "c" * 40),
            ("extra", 1),
        ):
            with self.subTest(key=key), self.assertRaises(ValueError):
                runner.validate_report(dict(REPORT, **{key: value}), SOURCE, CONTRACT)
        with self.assertRaises(ValueError):
            runner.validate_report(
                {k: v for k, v in REPORT.items() if k != "preparation"}, SOURCE, CONTRACT
            )

    def test_json_duplicate_and_nonfinite_values_are_refused(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "report.json"
            for value in ('{"ok": true, "ok": true}', '{"value": NaN}'):
                path.write_text(value)
                with self.assertRaises(ValueError):
                    runner.read_json(path)

    def test_lifecycle_rejects_wrong_release_order_and_errors(self):
        runner.validate_log(LOG)
        for value in (
            LOG.replace("native-release=core", "native-release=missing"),
            LOG + "\nERROR: native failed",
            LOG + "\ninstances leaked",
            LOG.replace("python-release=consumer", "session=complete"),
        ):
            with self.subTest(value=value), self.assertRaises(ValueError):
                runner.validate_log(value)

    def test_timeout_reaps_owned_process_and_retains_output(self):
        with tempfile.TemporaryDirectory() as directory:
            log = Path(directory) / "timeout.log"
            with self.assertRaises(TimeoutError):
                runner.run_child(
                    [
                        sys.executable,
                        "-u",
                        "-c",
                        "import os, time; print(os.getpid()); "
                        "print('before timeout'); time.sleep(10)",
                    ],
                    log,
                    0.1,
                )
            self.assertIn("before timeout", log.read_text())
            pid = int(log.read_text().splitlines()[0])
            with self.assertRaises(ProcessLookupError):
                os.kill(pid, 0)

    def test_simulated_scene_writes_actual_result_and_separate_processes(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            script = root / "scene.py"
            script.write_text(
                "import json, os, pathlib, sys\n"
                + "args = dict(item.split('=', 1) for item in sys.argv[1:] "
                "if item.startswith('--mornlea-'))\n"
                + "pathlib.Path(args['--mornlea-rust-report']).write_text(json.dumps("
                + repr(REPORT)
                + "))\n"
                + "print("
                + repr(LOG)
                + ")\nprint('pid=' + str(os.getpid()))\n"
            )
            pids = []
            for index in range(2):
                runner.run_iteration(
                    [sys.executable, str(script)],
                    root,
                    32123,
                    SOURCE,
                    CONTRACT,
                    root,
                    index + 1,
                    {},
                    2,
                )
                pids.append(
                    (root / f"iteration-{index + 1}.log").read_text().split("pid=")[1].strip()
                )
            self.assertNotEqual(*pids)

    def test_source_identity_refuses_dirty_runtime_source(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            engine = root / "packages/engine"
            engine.mkdir(parents=True)
            source = engine / "source.rs"
            source.write_text("original")
            commands = (
                ["init", "-q"],
                ["add", "."],
                [
                    "-c",
                    "user.name=Smoke Test",
                    "-c",
                    "user.email=smoke@example.invalid",
                    "commit",
                    "-q",
                    "-m",
                    "test: prepare source fixture",
                ],
            )
            for command in commands:
                subprocess.run(["git", "-C", str(root), *command], check=True, capture_output=True)
            self.assertEqual(len(runner.source_identity(root)), 40)
            source.write_text("changed")
            with self.assertRaisesRegex(ValueError, "uncommitted"):
                runner.source_identity(root)
            source.write_text("original")
            (engine / "uncommitted.rs").write_text("untracked runtime source")
            with self.assertRaisesRegex(ValueError, "uncommitted"):
                runner.source_identity(root)

    def test_python_identity_separates_generated_cache_from_static_runtime(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            scripts = root / "scripts/godot"
            (scripts / "py4godot").mkdir(parents=True)
            (scripts / "python-version.env").write_text("PY4GODOT_CPYTHON_VERSION=3.14.4\n")
            (scripts / "py4godot/build-inputs.env").write_text(
                "PY4GODOT_HARDENED_LOADER_SHA256=" + hashlib.sha256(b"loader").hexdigest() + "\n"
                "PY4GODOT_HARDENED_ARTIFACT_SHA256=" + "a" * 64 + "\n"
            )
            project = root / "project"
            runtime = project / "addons/py4godot/cpython-3.14.4-darwin64/python"
            (runtime / "bin").mkdir(parents=True)
            (runtime / "lib").mkdir()
            (runtime / "bin/pythonscript.dylib").write_bytes(b"loader")
            (runtime / "bin/python3.14").write_bytes(b"executable")
            (runtime / "lib/libpython3.14.dylib").write_bytes(b"library")
            before = runner.python_identity(root, project)
            cache = runtime / "lib/__pycache__"
            cache.mkdir()
            (cache / "module.pyc").write_bytes(b"generated")
            after = runner.python_identity(root, project)
            self.assertEqual(before["static_sha256"], after["static_sha256"])
            self.assertNotEqual(before["artifact_sha256"], after["artifact_sha256"])
            self.assertEqual(
                after["generated_cache_paths"],
                ["cpython-3.14.4-darwin64/python/lib/__pycache__/module.pyc"],
            )
            (cache / "ordinary.py").write_bytes(b"source")
            self.assertNotEqual(
                after["static_sha256"], runner.python_identity(root, project)["static_sha256"]
            )

    def test_stale_result_is_refused_before_process_launch(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / "iteration-1.json").write_text("{}")
            with self.assertRaisesRegex(ValueError, "stale"):
                runner.run_iteration(
                    ["/usr/bin/true"], root, 32123, SOURCE, CONTRACT, root, 1, {}, 2
                )
            self.assertFalse((root / "iteration-1.log").exists())

    def test_iteration_failure_retains_scene_log(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            with self.assertRaisesRegex(ValueError, "error or leak"):
                runner.run_iteration(
                    [sys.executable, "-c", "print('ERROR: fixture failure')"],
                    root,
                    32123,
                    SOURCE,
                    CONTRACT,
                    root,
                    1,
                    {},
                    2,
                )
            self.assertIn("fixture failure", (root / "iteration-1.log").read_text())

    @unittest.skipUnless(sys.platform == "darwin", "Darwin sandbox qualification")
    def test_sandbox_allows_owned_tcp_and_denies_other_tcp(self):
        with socket.socket() as allowed, tempfile.TemporaryDirectory() as directory:
            allowed.bind(("127.0.0.1", 0))
            allowed.listen(5)
            evidence = runner.qualify_policy(allowed.getsockname()[1], Path(directory))
            self.assertEqual(evidence["allowed"], True)
            self.assertEqual(evidence["denied_loopback"], True)
            self.assertEqual(evidence["denied_external"], True)
            self.assertEqual(evidence["denied_ipv6"], True)
            self.assertEqual(evidence["denied_other_ipv4"], True)

    def test_flags_reject_unpaired_core_and_session_before_build(self):
        smoke = SCRIPT.with_name("smoke.sh")
        for args in (
            ("--core", "rust"),
            ("--exercise-session",),
            ("--core", "pilot", "--exercise-session"),
        ):
            with self.subTest(args=args):
                result = subprocess.run([str(smoke), *args], capture_output=True, text=True)
                self.assertEqual(result.returncode, 2)
                self.assertIn("together", result.stderr)


if __name__ == "__main__":
    unittest.main()
