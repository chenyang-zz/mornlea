#!/usr/bin/env python3
"""Own the disposable TCP fixture and bounded native Godot qualification processes."""

import argparse
import hashlib
import json
import os
import re
import signal
import socket
import subprocess
import sys
import tempfile
import time
import uuid
from contextlib import suppress
from pathlib import Path

ERRORS = re.compile(
    r"ERROR:|SCRIPT ERROR:|leaked at exit|instances leaked|resources still in use at exit|"
    r"orphan (StringName|NodePath)|interpreter.+not finalized|features?.+still active|"
    r"fatal python error|segmentation fault|abort trap",
    re.IGNORECASE,
)


def positive_integer(text):
    if not re.fullmatch(r"[1-9][0-9]*", text):
        raise argparse.ArgumentTypeError("expected a positive integer")
    return int(text)


def loopback_address(text):
    match = re.fullmatch(r"127\.0\.0\.1:([1-9][0-9]*)", text.strip())
    if not match or not 1 <= int(match[1]) <= 65535:
        raise ValueError("fixture must publish one numeric loopback address and valid port")
    return int(match[1])


def read_json(path):
    def pairs(items):
        result = {}
        for key, value in items:
            if key in result:
                raise ValueError("duplicate JSON field")
            result[key] = value
        return result

    def invalid_constant(value):
        raise ValueError("nonfinite JSON number: " + value)

    if path.stat().st_size > 1024 * 1024:
        raise ValueError("qualification report exceeds one MiB")
    return json.loads(path.read_text(), object_pairs_hook=pairs, parse_constant=invalid_constant)


def validate_report(report, source, contract):
    expected = {
        "ok": True,
        "source_sha": source,
        "contract_sha": contract,
        "families": list(range(1, 11)),
        "resets": 1,
        "reconnects": 1,
        "inputs": 1,
        "preparation": True,
        "release_order": True,
        "retained_frame": True,
    }
    # Exact scalar types prevent JSON booleans from satisfying integer evidence.
    if type(report) is not dict or set(report) != set(expected):
        raise ValueError("missing or extra session evidence fields")
    for key, value in expected.items():
        if type(report[key]) is not type(value) or report[key] != value:
            raise ValueError("session evidence mismatch: " + key)
    if any(type(value) is not int for value in report["families"]):
        raise ValueError("family IDs must be integers")


def validate_log(output):
    if ERRORS.search(output):
        raise ValueError("native process reported an error or leak")
    markers = [
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
    editor_init = "[mornlea-lifecycle] rust-init=editor"
    editor_deinit = "[mornlea-lifecycle] rust-deinit=editor"
    if editor_init in output or editor_deinit in output:
        markers.insert(1, editor_init)
        markers.insert(-1, editor_deinit)
    previous = -1
    for marker in markers:
        if output.count(marker) != 1 or output.index(marker) <= previous:
            raise ValueError("missing, repeated or out-of-order lifecycle marker: " + marker)
        previous = output.index(marker)


def stop_child(child):
    # A separate process group limits cleanup to this runner's descendants.
    if child.poll() is None:
        os.killpg(child.pid, signal.SIGTERM)
        try:
            child.wait(timeout=3)
        except subprocess.TimeoutExpired:
            os.killpg(child.pid, signal.SIGKILL)
            child.wait(timeout=3)
    try:
        os.killpg(child.pid, 0)
    except ProcessLookupError:
        return
    os.killpg(child.pid, signal.SIGKILL)
    raise ValueError("owned child left a process group behind")


def run_child(command, log, timeout, env=None):
    if not isinstance(timeout, (int, float)) or not 0 < timeout <= 45:
        raise ValueError("child wall timeout must be positive and at most 45 seconds")
    with log.open("x") as output:
        child = subprocess.Popen(
            command, stdout=output, stderr=subprocess.STDOUT, env=env, start_new_session=True
        )
        try:
            try:
                status = child.wait(timeout=timeout)
            except subprocess.TimeoutExpired as error:
                raise TimeoutError("owned child exceeded wall timeout") from error
            if status:
                raise ValueError("owned child failed with exit status " + str(status))
        finally:
            stop_child(child)


def policy(port):
    port = loopback_address("127.0.0.1:" + str(port))
    return (
        "(version 1) (allow default) (deny network*) "
        '(allow network-outbound (remote tcp4 "localhost:' + str(port) + '"))'
    )


def qualify_policy(port, directory):
    if sys.platform != "darwin":
        raise ValueError("Rust native session network policy is qualified only on Darwin")
    profile = policy(port)
    (directory / "network.sb").write_text(profile + "\n")
    with socket.socket() as denied, socket.socket(socket.AF_INET6) as denied_ipv6:
        denied.bind(("127.0.0.1", 0))
        denied.listen(1)
        denied_ipv6.bind(("::1", port))
        denied_ipv6.listen(1)
        probes = [
            ("allowed", "127.0.0.1", port, False, "AF_INET"),
            ("denied_loopback", "127.0.0.1", denied.getsockname()[1], True, "AF_INET"),
            ("denied_external", "203.0.113.1", 80, True, "AF_INET"),
            ("denied_ipv6", "::1", port, True, "AF_INET6"),
            ("denied_other_ipv4", "127.0.0.2", port, True, "AF_INET"),
        ]
        for name, host, probe_port, must_deny, family in probes:
            code = (
                "import errno,socket,sys\n"
                "s=socket.socket(getattr(socket,sys.argv[4])); s.settimeout(2)\n"
                "try:\n s.connect((sys.argv[1],int(sys.argv[2])))\n"
                "except OSError as e:\n print('errno='+str(e.errno)); "
                "sys.exit(0 if sys.argv[3]=='deny' "
                "and e.errno in (errno.EPERM,errno.EACCES) else 1)\n"
                "else:\n print('connected'); sys.exit(1 if sys.argv[3]=='deny' else 0)\n"
                "finally:\n s.close()\n"
            )
            run_child(
                [
                    "/usr/bin/sandbox-exec",
                    "-p",
                    profile,
                    sys.executable,
                    "-I",
                    "-c",
                    code,
                    host,
                    str(probe_port),
                    "deny" if must_deny else "allow",
                    family,
                ],
                directory / ("network-" + name + ".log"),
                5,
            )
    return {name: True for name, *_ in probes}


def run_iteration(prefix, project, port, source, contract, directory, iteration, env, timeout=45):
    result = directory / f"iteration-{iteration}.json"
    if result.exists():
        raise ValueError("stale iteration report must not be reused")
    token = "mornlea-smoke-" + uuid.uuid4().hex
    command = [
        *prefix,
        "--headless",
        "--path",
        str(project),
        "--quit-after",
        "1800",
        "res://tests/scenes/rust_session_smoke_check.tscn",
        "--",
        f"--mornlea-rust-server=127.0.0.1:{port}",
        f"--mornlea-rust-source={source}",
        f"--mornlea-rust-contract={contract}",
        f"--mornlea-rust-report={result}",
        f"--mornlea-smoke-token={token}",
    ]
    log = directory / f"iteration-{iteration}.log"
    started = time.time_ns()
    run_child(command, log, timeout, env or None)
    validate_log(log.read_text())
    report = read_json(result)
    validate_report(report, source, contract)
    for crash_root in (
        directory,
        Path(env.get("HOME", str(directory))) / "Library/Logs/DiagnosticReports",
        Path.home() / "Library/Logs/DiagnosticReports",
    ):
        if crash_root.is_dir():
            for path in crash_root.rglob("*"):
                if (
                    path.is_file()
                    and path.stat().st_mtime_ns >= started
                    and (
                        (
                            path.suffix.lower() in (".crash", ".ips")
                            and re.search("godot|mornlea", path.name, re.I)
                        )
                        or path.name == "core"
                        or path.name.startswith("core.")
                    )
                ):
                    raise ValueError("native process left a crash report: " + str(path))
    residue = subprocess.run(["/usr/bin/pgrep", "-f", token], capture_output=True, text=True)
    if residue.returncode != 1:
        raise ValueError("native process residue or failed residue check: " + residue.stdout)
    return report


def sha256(path):
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        for chunk in iter(lambda: stream.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def source_identity(repository):
    dirty = subprocess.check_output(
        [
            "git",
            "-C",
            str(repository),
            "status",
            "--porcelain",
            "--untracked-files=all",
            "--",
            "packages/engine",
            "apps/mornlea-godot",
            "scripts/godot",
        ],
        text=True,
    )
    if dirty:
        raise ValueError("qualification runtime source has uncommitted changes:\n" + dirty)
    source = subprocess.check_output(
        ["git", "-C", str(repository), "rev-parse", "HEAD"], text=True
    ).strip()
    if not re.fullmatch(r"[0-9a-f]{40}", source):
        raise ValueError("qualification requires a full immutable source SHA")
    return source


def contract_sha(repository):
    digest = hashlib.sha256()
    root = repository / "packages/engine/crates/mornlea_client_core"
    for name in ("src/contracts.rs", "src/presentation/frame.rs", "src/presentation/mod.rs"):
        digest.update(name.encode() + b"\0" + (root / name).read_bytes())
    return digest.hexdigest()


def python_identity(repository, project):
    manifests = {}
    for name in ("python-version.env", "py4godot/build-inputs.env"):
        for line in (repository / "scripts/godot" / name).read_text().splitlines():
            if line and not line.startswith("#"):
                key, value = line.split("=", 1)
                manifests[key] = value
    addon = project / "addons/py4godot"
    loader = addon / (
        "cpython-"
        + manifests["PY4GODOT_CPYTHON_VERSION"]
        + "-darwin64/python/bin/pythonscript.dylib"
    )
    actual_loader = sha256(loader)
    if actual_loader != manifests["PY4GODOT_HARDENED_LOADER_SHA256"]:
        raise ValueError("installed Python loader differs from verified manifest")
    lines = []
    generated = set()
    for path in addon.rglob("*"):
        if path.is_symlink():
            lines.append((str(path.relative_to(addon)), "symlink:" + os.readlink(path)))
        elif path.is_file():
            relative = path.relative_to(addon)
            lines.append((str(relative), sha256(path)))
            if "__pycache__" in relative.parts and path.suffix == ".pyc":
                generated.add(str(relative))
    artifact = hashlib.sha256(
        "".join(digest + "  " + name + "\n" for name, digest in sorted(lines)).encode()
    ).hexdigest()
    # The builder verifies canonical bytes before launch; Python may then create bytecode caches.
    static = hashlib.sha256(
        "".join(
            digest + "  " + name + "\n" for name, digest in sorted(lines) if name not in generated
        ).encode()
    ).hexdigest()
    runtime = loader.parent.parent
    return {
        "manifest": manifests,
        "loader_sha256": actual_loader,
        "artifact_sha256": artifact,
        "static_sha256": static,
        "generated_cache_paths": sorted(generated),
        "executable_sha256": sha256(runtime / "bin/python3.14"),
        "library_sha256": sha256(runtime / "lib/libpython3.14.dylib"),
    }


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--repository-root", type=Path, required=True)
    parser.add_argument("--godot", type=Path, required=True)
    parser.add_argument("--iterations", type=positive_integer, default=100)
    args = parser.parse_args()
    if sys.platform != "darwin" or os.uname().machine != "arm64":
        parser.error("Rust native session qualification requires Darwin arm64")
    repository = args.repository_root.resolve()
    project = repository / "apps/mornlea-godot"
    directory = Path(tempfile.mkdtemp(prefix="mornlea-rust-session-smoke.", dir="/private/tmp"))
    print("Rust session qualification report retained: " + str(directory), flush=True)
    server = repository / "packages/engine/target/debug/examples/rust_session_server"
    native = project / "addons/mornlea_bridge/bin/macos-universal/debug/libmornlea_godot.dylib"
    report = {
        "ok": False,
        "source_sha": None,
        "contract_sha": None,
        "features": ["runtime-qualification"],
        "requested_iterations": args.iterations,
        "iterations": [],
    }
    child = None
    try:
        source = source_identity(repository)
        report.update(source_sha=source, contract_sha=contract_sha(repository))
        report.update(
            native_sha256=sha256(native),
            godot_sha256=sha256(args.godot),
            server_sha256=sha256(server),
            python=python_identity(repository, project),
        )
        ready, stop, server_report = (
            directory / name for name in ("server.ready", "server.stop", "server.json")
        )
        with (directory / "server.log").open("x") as output:
            child = subprocess.Popen(
                [
                    str(server),
                    "--listen",
                    "127.0.0.1:0",
                    "--ready-file",
                    str(ready),
                    "--stop-file",
                    str(stop),
                    "--report-file",
                    str(server_report),
                    "--max-seconds",
                    str(args.iterations * 45 + 30),
                ],
                stdout=output,
                stderr=subprocess.STDOUT,
                start_new_session=True,
            )
            deadline = time.monotonic() + 10
            while not ready.exists():
                if child.poll() is not None or time.monotonic() >= deadline:
                    raise TimeoutError("owned fixture failed bounded readiness")
                time.sleep(0.02)
            port = loopback_address(ready.read_text())
            report["server_address"] = f"127.0.0.1:{port}"
            report["network_policy"] = qualify_policy(port, directory)
            isolated = directory / "isolated"
            for name in ("home/Library/Logs/DiagnosticReports", "bin", "poison", "tmp"):
                (isolated / name).mkdir(parents=True, exist_ok=True)
            poison = isolated / "poison"
            (poison / "sitecustomize.py").write_text(
                'raise RuntimeError("ambient Python reached Rust smoke")\n'
            )
            env = dict(
                os.environ,
                HOME=str(isolated / "home"),
                PATH=str(isolated / "bin"),
                TMPDIR=str(isolated / "tmp"),
                PYTHONHOME=str(poison),
                PYTHONPATH=str(poison),
                PYTHONSTARTUP=str(poison / "sitecustomize.py"),
                PYTHONNOUSERSITE="1",
                PYTHONUSERBASE=str(isolated / "user-base"),
                PIP_CONFIG_FILE="/dev/null",
            )
            prefix = ["/usr/bin/sandbox-exec", "-p", policy(port), str(args.godot.resolve())]
            for iteration in range(1, args.iterations + 1):
                if child.poll() is not None:
                    raise ValueError("owned fixture exited before session completion")
                report["iterations"].append(
                    run_iteration(
                        prefix,
                        project,
                        port,
                        source,
                        report["contract_sha"],
                        directory,
                        iteration,
                        env,
                    )
                )
            stop.touch(exist_ok=False)
            if child.wait(timeout=10):
                raise ValueError("owned fixture failed shutdown")
            fixture = read_json(server_report)
            for name in ("sessions", "packets", "ticks"):
                if type(fixture.get(name)) is not int or fixture[name] < (
                    2 * args.iterations if name == "sessions" else 1
                ):
                    raise ValueError("missing actual fixture count: " + name)
            if fixture.get("errors") != []:
                raise ValueError("owned fixture reported failures")
            if "admissions" in fixture and (
                type(fixture["admissions"]) is not int
                or fixture["admissions"] < 2 * args.iterations
            ):
                raise ValueError("fixture did not admit both session generations")
            if ERRORS.search((directory / "server.log").read_text()):
                raise ValueError("owned fixture log contains error or leak")
            report["server"] = fixture
        stop_child(child)
        child = None
        # Recheck static distribution bytes while recording the full mutable cache inventory.
        report["python_final"] = python_identity(repository, project)
        if report["python_final"]["static_sha256"] != report["python"]["static_sha256"]:
            raise ValueError("qualification static Python runtime changed during execution")
        if (
            sha256(native),
            sha256(args.godot),
            sha256(server),
        ) != (
            report["native_sha256"],
            report["godot_sha256"],
            report["server_sha256"],
        ):
            raise ValueError("qualification artifacts changed during execution")
        if (
            source_identity(repository) != source
            or contract_sha(repository) != report["contract_sha"]
        ):
            raise ValueError("qualification source changed during execution")
        report["ok"] = True
    except (OSError, ValueError, TimeoutError, subprocess.SubprocessError) as error:
        report["error"] = str(error)
        print("Rust session qualification failed: " + str(error), file=sys.stderr)
    finally:
        if child is not None:
            try:
                if child.poll() is None:
                    (directory / "server.stop").touch(exist_ok=True)
                    with suppress(subprocess.TimeoutExpired):
                        child.wait(timeout=3)
                stop_child(child)
            except (OSError, ValueError, subprocess.SubprocessError) as error:
                report["ok"] = False
                report["cleanup_error"] = str(error)
        (directory / "report.json").write_text(json.dumps(report, indent=2) + "\n")
    if not report["ok"]:
        return 1
    print(
        f"Rust session qualification passed: iterations={args.iterations} "
        f"report={directory / 'report.json'}"
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
