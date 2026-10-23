"""Tests for CI workflows, entrypoints, and local contracts."""

from collections import deque
import os
from pathlib import Path
import re
import shutil
import subprocess
import pytest
import yaml

from audit.common import get_repo_root, run_cmd
from tests.test_companion_agent import companion_agent_workflow_violations

CI_CHECKOUT = "actions/checkout@fbc6f3992d24b796d5a048ff273f7fcc4a7b6c09"
CI_GO = "actions/setup-go@924ae3a1cded613372ab5595356fb5720e22ba16"
CI_NODE = "actions/setup-node@a0853c24544627f65ddf259abe73b1d18a591444"
CI_PYTHON = "actions/setup-python@ece7cb06caefa5fff74198d8649806c4678c61a1"
CI_UPLOAD = "actions/upload-artifact@ea165f8d65b6e75b540449e92b4886f43607fa02"
CI_DOWNLOAD = "actions/download-artifact@634f93cb2916e3fdff6788551b99b062d0335ce0"
CI_UV = "astral-sh/setup-uv@37802adc94f370d6bfd71619e3f0bf239e1f3b78"

CI_ACTIONS: dict[str, str] = {
    CI_CHECKOUT: "v5",
    CI_GO: "v6",
    CI_NODE: "v5",
    CI_PYTHON: "v6",
    CI_UPLOAD: "v4",
    CI_DOWNLOAD: "v5",
    CI_UV: "v7",
}

REQUIRED_CI_JOBS: dict[str, dict] = {
    "preflight": {"runner": "ubuntu-24.04", "timeout": 10, "needs": [], "setups": [CI_GO, CI_NODE, CI_UV]},
    "frontend": {"runner": "ubuntu-24.04", "timeout": 15, "needs": [], "setups": [CI_NODE]},
    "rust-quality": {"runner": "ubuntu-24.04", "timeout": 30, "needs": [], "setups": [CI_GO, CI_PYTHON, CI_UV]},
    "native-linux": {"runner": "ubuntu-24.04", "timeout": 30, "needs": [], "setups": [CI_GO]},
    "native-macos": {"runner": "macos-15", "timeout": 30, "needs": [], "setups": [CI_GO]},
    "linux-quality": {"runner": "ubuntu-24.04", "timeout": 20, "needs": ["native-linux"], "setups": [CI_GO, CI_UV]},
    "race-server": {"runner": "ubuntu-24.04", "timeout": 30, "needs": ["native-linux"], "setups": [CI_GO]},
    "race-rest": {"runner": "ubuntu-24.04", "timeout": 30, "needs": ["native-linux"], "setups": [CI_GO, CI_UV]},
    "race-client": {"runner": "macos-15", "timeout": 45, "needs": ["native-macos"], "setups": [CI_GO]},
    "integration-server": {"runner": "ubuntu-24.04", "timeout": 30, "needs": ["native-linux"], "setups": [CI_GO, CI_PYTHON, CI_UV]},
    "integration-client": {"runner": "macos-15", "timeout": 45, "needs": ["native-macos"], "setups": [CI_GO]},
}

CI_GO_CACHE = """packages/shared/go.sum
packages/server/go.sum
packages/client/go.sum
packages/tools/go.sum
packages/engine/include/mornlea_engine.h
packages/engine/include/mornlea_client.h"""

CI_START = 'echo "CI_START_EPOCH=$(date +%s)" >> "$GITHUB_ENV"'


def ci_summary(name: str) -> str:
    """Format required CI summary run command."""
    return f"""{{\n  echo "{name} seconds: $(( $(date +%s) - CI_START_EPOCH ))"\n  echo "runner: ${{RUNNER_OS}}/$(uname -m)"\n}} >> "$GITHUB_STEP_SUMMARY\""""


def _non_empty_trimmed_lines(value: str) -> list[str]:
    return [line.strip() for line in value.splitlines() if line.strip()]


def required_workflow_violations(source: bytes | str) -> list[str]:
    """Audit executable orchestration independently of package selection and artifact verification."""
    source_str = source.decode("utf-8") if isinstance(source, bytes) else source
    try:
        workflow = yaml.safe_load(source_str)
    except Exception as e:
        return [f"invalid workflow YAML: {e}"]

    if not isinstance(workflow, dict):
        return ["invalid workflow YAML: root must be a mapping"]

    violations: list[str] = []

    def require(ok: bool, message: str) -> None:
        if not ok:
            violations.append(message)

    require(workflow.get("name") == "Required CI", "workflow name must be Required CI")

    on_events = workflow.get("on") if "on" in workflow else workflow.get(True, {})
    require(
        isinstance(on_events, dict) and len(on_events) == 2 and "push" in on_events and "pull_request" in on_events,
        "events must be main push and pull_request only",
    )
    if isinstance(on_events, dict):
        push_event = on_events.get("push")
        if isinstance(push_event, dict):
            require(
                len(push_event) == 1 and push_event.get("branches") == ["main"],
                "push must select only main",
            )
        else:
            require(False, "push must select only main")

        pr_event = on_events.get("pull_request")
        require(pr_event is None, "pull_request must not filter required work")

    concurrency = workflow.get("concurrency", {})
    if isinstance(concurrency, dict):
        require(
            concurrency.get("group") == "required-ci-${{ github.event.pull_request.number || github.ref }}"
            and concurrency.get("cancel-in-progress") is True,
            "candidate concurrency must cancel stale runs",
        )
    else:
        require(False, "candidate concurrency must cancel stale runs")

    require(workflow.get("permissions") == {"contents": "read"}, "workflow permissions must be contents read")
    require(
        workflow.get("env") == {"CARGO_TARGET_DIR": "${{ github.workspace }}/packages/engine/target/cargo"},
        "workflow Cargo target root must be explicit",
    )

    # Check for forbidden overrides and unreviewed actions in raw lines
    for line in source_str.splitlines():
        line_clean = line.strip()
        for forbidden in ["continue-on-error", "strategy", "defaults", "container", "services"]:
            if (
                line_clean == f"{forbidden}:"
                or line_clean.startswith(f"{forbidden}:")
                or f" {forbidden}:" in line
            ):
                require(False, f"forbidden execution override: {forbidden}")

        if "uses:" in line_clean:
            match = re.search(r"uses:\s*([^\s#]+)(?:\s+#\s*([^\s]+))?", line_clean)
            if match:
                action = match.group(1).strip()
                tag = (match.group(2) or "").strip()
                if not action.startswith("./"):
                    expected_tag = CI_ACTIONS.get(action)
                    require(expected_tag is not None, f"unreviewed action: {action}")
                    if expected_tag is not None:
                        require(
                            tag == expected_tag,
                            f"action requires reviewed major comment: {action}",
                        )

    require("godot" not in source_str.lower(), "Godot must remain outside required workflow")

    jobs = workflow.get("jobs", {})
    require(len(jobs) == len(REQUIRED_CI_JOBS) + 1, "required job set must be exact")

    needs_list: list[str] = []
    for name, want in REQUIRED_CI_JOBS.items():
        needs_list.append(name)
        job = jobs.get(name)
        if not job or not isinstance(job, dict):
            violations.append(f"missing required job: {name}")
            continue

        require(job.get("runs-on") == want["runner"], f"{name}: wrong runner")
        require(job.get("timeout-minutes") == want["timeout"], f"{name}: wrong timeout")

        job_needs = job.get("needs")
        if job_needs is None:
            job_needs_list = []
        elif isinstance(job_needs, str):
            job_needs_list = [job_needs]
        else:
            job_needs_list = list(job_needs)
        require(job_needs_list == want["needs"], f"{name}: wrong needs")

        require(job.get("if") is None or job.get("if") == "", f"{name}: required job must not be conditional")
        require(job.get("permissions") is None, f"{name}: job must inherit least privilege permissions")

        job_env = job.get("env")
        expected_env = {"COREPACK_ENABLE_DOWNLOAD_PROMPT": "0"} if name == "frontend" else {}
        if not job_env:
            job_env = {}
        require(job_env == expected_env, f"{name}: unexpected environment override")

        command = f"make ci-{name}"
        if name.startswith("native-") or len(want["needs"]) > 0:
            command += ' CI_CANDIDATE_SHA="$GITHUB_SHA"'

        steps = job.get("steps", [])
        command_indexes = [
            i for i, s in enumerate(steps) if isinstance(s, dict) and s.get("run", "").strip() == command
        ]
        require(len(command_indexes) == 1, f"{name}: missing exact repository command")

        checkout_indexes = [
            i for i, s in enumerate(steps) if isinstance(s, dict) and s.get("uses") == CI_CHECKOUT
        ]
        require(len(checkout_indexes) == 1, f"{name}: candidate checkout required")
        if len(checkout_indexes) == 1:
            require(checkout_indexes[0] == 1, f"{name}: checkout must precede repository-dependent setup")

        setups: list[str] = []
        runs: list[str] = []
        for index, step in enumerate(steps):
            if not isinstance(step, dict):
                continue
            extra_keys = set(step.keys()) - {"name", "uses", "run", "with", "if"}
            require(len(extra_keys) == 0, f"{name}: step has forbidden fields: {extra_keys}")
            step_uses = step.get("uses")
            if step_uses:
                require(
                    (step.get("if") is None or step.get("if") == "") and not step.get("run"),
                    f"{name}: actions must run unconditionally",
                )
                if len(command_indexes) == 1 and step_uses != CI_UPLOAD:
                    require(index < command_indexes[0], f"{name}: setup/download must precede command")
                if step_uses == CI_CHECKOUT:
                    with_block = step.get("with") or {}
                    require(len(with_block) == 0, f"{name}: checkout must use candidate defaults")
                if step_uses not in (CI_CHECKOUT, CI_UPLOAD, CI_DOWNLOAD):
                    setups.append(step_uses)
                if step_uses == CI_GO:
                    with_block = step.get("with", {})
                    require(len(with_block) == 2, f"{name}: Go setup input contract")
                    require(with_block.get("go-version") == "1.26", f"{name}: Go version")
                    require(
                        _non_empty_trimmed_lines(str(with_block.get("cache-dependency-path", "")))
                        == _non_empty_trimmed_lines(CI_GO_CACHE),
                        f"{name}: Go module/header cache",
                    )
                if step_uses == CI_NODE:
                    with_block = step.get("with", {})
                    input_count = 3 if name == "frontend" else 1
                    require(len(with_block) == input_count, f"{name}: Node setup input contract")
                    require(with_block.get("node-version") == "24", f"{name}: Node version")
                    if name == "frontend":
                        require(with_block.get("cache") == "pnpm", f"{name}: pnpm cache")
                        require(
                            with_block.get("cache-dependency-path")
                            == "packages/engine/crates/mornlea_client/frontend/pnpm-lock.yaml",
                            f"{name}: locked pnpm cache",
                        )
                if step_uses == CI_PYTHON:
                    with_block = step.get("with", {})
                    require(len(with_block) == 1, f"{name}: Python setup input contract")
                if step_uses == CI_UV:
                    with_block = step.get("with", {})
                    require(len(with_block) == 3, f"{name}: uv setup input contract")

            step_run = step.get("run")
            if step_run:
                run_clean = step_run.strip()
                runs.append(run_clean)
                if run_clean != ci_summary(name):
                    require(
                        step.get("if") is None or step.get("if") == "",
                        f"{name}: required command must not be conditional",
                    )
                else:
                    require(step.get("if") == "${{ always() }}", f"{name}: summary must always run")

        require(setups == want["setups"], f"{name}: setup actions must match command dependencies")

        want_runs = [CI_START]
        if name in ("preflight", "linux-quality", "race-rest"):
            want_runs.append("sudo apt-get update\nsudo apt-get install --yes ripgrep")
        if name == "frontend":
            want_runs.append(
                "corepack enable\ncd packages/engine/crates/mornlea_client/frontend\n"
                "corepack install --global \"$(node -p 'require(\"./package.json\").packageManager')\"\n"
                "corepack pnpm --version"
            )
        if name == "rust-quality" or name.startswith("native-"):
            want_runs.append("cd packages/engine\nrustup show active-toolchain\nrustc --version\ncargo --version")
        want_runs.append(command)
        want_runs.append(ci_summary(name))
        require(runs == want_runs, f"{name}: run steps must remain thin, bounded orchestration without retries or inline selectors")

        uploads = [i for i, s in enumerate(steps) if isinstance(s, dict) and s.get("uses") == CI_UPLOAD]
        downloads = [i for i, s in enumerate(steps) if isinstance(s, dict) and s.get("uses") == CI_DOWNLOAD]
        if name.startswith("native-"):
            require(len(uploads) == 1 and len(downloads) == 0, f"{name}: producer artifact steps")
            if len(uploads) == 1:
                step = steps[uploads[0]]
                with_block = step.get("with", {})
                require(len(with_block) == 4, f"{name}: upload input contract")
                require(with_block.get("name") == f"{name}-${{{{ github.sha }}}}", f"{name}: same-SHA upload")
                paths = (
                    ["build/ci/native-macos.manifest", "packages/engine/target/release/libmornlea_engine.dylib", "packages/engine/target/release/libmornlea_client.dylib"]
                    if name == "native-macos"
                    else ["build/ci/native-linux.manifest", "bin/mornlea-server", "bin/libmornlea_engine.so", "packages/engine/target/release/libmornlea_engine.so"]
                )
                require(_non_empty_trimmed_lines(str(with_block.get("path", ""))) == paths, f"{name}: artifact paths")
                require(with_block.get("if-no-files-found") == "error", f"{name}: missing files must fail")
                retention = with_block.get("retention-days")
                require(retention == 1 or retention == "1", f"{name}: one day retention")
                if len(command_indexes) == 1:
                    require(uploads[0] > command_indexes[0], f"{name}: upload must follow successful build")
        elif len(want["needs"]) > 0:
            require(len(downloads) == 1 and len(uploads) == 0, f"{name}: consumer artifact steps")
            if len(downloads) == 1:
                step = steps[downloads[0]]
                with_block = step.get("with", {})
                require(len(with_block) == 2, f"{name}: download must use the current run")
                require(with_block.get("name") == f"{want['needs'][0]}-${{{{ github.sha }}}}", f"{name}: same-SHA download")
                require(with_block.get("path") == ".", f"{name}: artifact root must be repository root")
        else:
            require(len(downloads) == 0 and len(uploads) == 0, f"{name}: independent stage must not transfer artifacts")

    needs_list.sort()
    gate = jobs.get("merge-gate")
    require(gate is not None and isinstance(gate, dict), "missing merge-gate")
    if gate and isinstance(gate, dict):
        require(gate.get("if") == "${{ always() }}", "merge-gate must always run")
        gate_needs = gate.get("needs")
        got_needs = sorted(list(gate_needs) if isinstance(gate_needs, list) else ([gate_needs] if gate_needs else []))
        require(got_needs == needs_list, "merge-gate needs must include every required job exactly once")
        require(gate.get("runs-on") == "ubuntu-24.04" and gate.get("timeout-minutes") == 5, "merge-gate runner/timeout")
        require(gate.get("permissions") is None and not gate.get("env"), "merge-gate must inherit permissions and environment")
        steps = gate.get("steps", [])
        require(len(steps) == 1, "merge-gate must be a pure single-step aggregator")
        if len(steps) == 1 and isinstance(steps[0], dict):
            step = steps[0]
            extra_keys = set(step.keys()) - {"name", "uses", "run", "with", "if"}
            require(len(extra_keys) == 0, f"merge-gate: step has forbidden fields: {extra_keys}")
            require(not step.get("uses") and not step.get("if"), "merge-gate must only assert results")
            assertions = sorted([f'test "${{{{ needs.{j}.result }}}}" = success' for j in needs_list])
            got = sorted(_non_empty_trimmed_lines(step.get("run", "")))
            require(got == assertions, "merge-gate must assert success for every need without extra commands")

    violations.extend(companion_agent_workflow_violations(source_str))
    violations.sort()
    return violations


def _make_target_is_phony(makefile: str, target: str) -> bool:
    """Check if target is declared in a .PHONY line."""
    for line in makefile.splitlines():
        if line.startswith(".PHONY:"):
            tokens = line.split(":", 1)[1].split()
            if target in tokens:
                return True
    return False


def _make_target_recipe(makefile: str, target: str) -> str:
    """Extract tab-indented recipe lines for target."""
    lines = makefile.splitlines()
    for idx, line in enumerate(lines):
        if line == f"{target}:" or line.startswith(f"{target}:"):
            recipe_lines: list[str] = []
            for cand in lines[idx + 1 :]:
                if cand.startswith("\t"):
                    recipe_lines.append(cand)
                    continue
                if not cand.strip() or cand.strip().startswith("#"):
                    continue
                break
            return "\n".join(recipe_lines)
    raise ValueError(f"target {target} not found in Makefile")


def _write_executable(path: Path | str, content: str | bytes) -> None:
    p = Path(path)
    p.parent.mkdir(parents=True, exist_ok=True)
    if isinstance(content, str):
        p.write_text(content, encoding="utf-8")
    else:
        p.write_bytes(content)
    p.chmod(0o755)


def _ci_recorder_source(name: str) -> str:
    return f"""#!/usr/bin/env bash
[[ $PWD -ef "$CI_TEST_ROOT" ]] || exit 92
if [[ ${{CI_TEST_LINUX:-}} == 1 ]]; then [[ $GOOS == linux && $GOARCH == amd64 && $CGO_ENABLED == 1 ]] || exit 93; fi
printf '{name}' >> "$CI_TEST_LOG"
printf '\\t%s' "$@" >> "$CI_TEST_LOG"
printf '\\n' >> "$CI_TEST_LOG"
count=$(wc -l < "$CI_TEST_LOG")
(( count != ${{CI_TEST_FAIL_AT:-0}} )) || exit 42
"""


def _ci_command_recorder(tmp_path: Path) -> tuple[Path, Path]:
    bin_dir = tmp_path / "bin"
    log_file = tmp_path / "commands"
    for name in ["make", "go"]:
        _write_executable(bin_dir / name, _ci_recorder_source(name))
    return bin_dir, log_file


def _ci_fixture_bin(tmp_path: Path, commands: list[str]) -> Path:
    bin_dir = tmp_path / "bin"
    bin_dir.mkdir(parents=True, exist_ok=True)
    for cmd in commands:
        if cmd == "bash":
            continue
        _write_executable(bin_dir / cmd, "#!/usr/bin/env bash\nexit 0\n")
    bash_path = shutil.which("bash")
    assert bash_path
    bash_symlink = bin_dir / "bash"
    if not bash_symlink.exists():
        bash_symlink.symlink_to(bash_path)
    return bin_dir


def _ci_environment(path: str, extra_env: list[str] | None = None) -> dict[str, str]:
    env = dict(os.environ)
    env["PATH"] = path
    if extra_env:
        for item in extra_env:
            k, v = item.split("=", 1)
            env[k] = v
    return env


def test_required_ci_workflow(repo_root: Path) -> None:
    """Verify production ci.yml matches all required CI workflow standards."""
    ci_file = repo_root / ".github" / "workflows" / "ci.yml"
    violations = required_workflow_violations(ci_file.read_text(encoding="utf-8"))
    assert not violations, "required CI workflow violations:\n" + "\n".join(violations)


def test_required_ci_workflow_mutations(repo_root: Path) -> None:
    """Verify synthetic mutations to ci.yml are rejected by workflow standard rules."""
    source = (repo_root / ".github" / "workflows" / "ci.yml").read_text(encoding="utf-8")
    assert not required_workflow_violations(source)

    mutations = [
        ("omitted merge need", "needs: [preflight, frontend, rust-quality, native-linux, native-macos, linux-quality, race-server, race-rest, race-client, integration-server, integration-client]", "needs: [frontend, rust-quality, native-linux, native-macos, linux-quality, race-server, race-rest, race-client, integration-server, integration-client]"),
        ("conditional merge", "  merge-gate:\n    if: ${{ always() }}", "  merge-gate:\n    if: ${{ success() }}"),
        ("missing assertion", '          test "${{ needs.preflight.result }}" = success', ""),
        ("Godot job", "jobs:\n", "jobs:\n  godot:\n    runs-on: ubuntu-24.04\n    steps: []\n"),
        ("moving runner", "runs-on: ubuntu-24.04", "runs-on: ubuntu-latest"),
        ("mutable action", CI_CHECKOUT, "actions/checkout@v5"),
        ("missing timeout", "    timeout-minutes: 10\n", ""),
        ("widened permissions", "contents: read", "contents: write"),
        ("artifact sha", "name: native-linux-${{ github.sha }}", "name: native-linux-main"),
        ("artifact root", "          path: .\n", "          path: packages/engine/target/release\n"),
        ("allow failure", "    timeout-minutes: 10", "    timeout-minutes: 10\n    continue-on-error: true"),
        ("inline selector", "run: make ci-preflight", "run: go test ./packages/audit -count=1"),
        ("retry", "run: make ci-preflight", "run: make ci-preflight || make ci-preflight"),
        ("conditional stage", "  preflight:\n", "  preflight:\n    if: ${{ false }}\n"),
        ("missing summary", 'echo "runner: ${RUNNER_OS}/$(uname -m)"', "echo runner"),
        ("wrong Go", "go-version: '1.26'", "go-version: '1.25'"),
        ("wrong Node", "node-version: '24'", "node-version: '20'"),
        ("missing upload file", "            bin/mornlea-server\n", ""),
        ("foreign artifact run", "          path: .\n", "          path: .\n          run-id: 1\n"),
        ("unchecked shell", "      - name: Assert every required result\n", "      - name: Assert every required result\n        shell: bash {0}\n"),
        ("step environment override", "      - name: Run preflight\n", "      - name: Run preflight\n        env:\n          CI_CANDIDATE_SHA: wrong\n"),
        ("filtered required event", "  pull_request:\n", "  pull_request:\n    paths: [README.md]\n"),
        ("stale run cancellation", "cancel-in-progress: true", "cancel-in-progress: false"),
        ("candidate override", f"      - uses: {CI_CHECKOUT} # v5\n", f"      - uses: {CI_CHECKOUT} # v5\n        with:\n          ref: main\n"),
    ]

    for name, old, new in mutations:
        assert old in source, f"mutation sentinel missing: {old!r}"
        mutated = source.replace(old, new, 1)
        violations = required_workflow_violations(mutated)
        assert violations, f"mutation {name!r} was accepted without violations"

    for job, next_job in [("linux-quality", "race-server"), ("race-rest", "race-client")]:
        block = f"  {job}:\n"
        start = source.find(block)
        assert start >= 0
        end = source.find(f"\n  {next_job}:\n", start + len(block))
        assert end >= 0
        end += start + len(block)
        job_source = source[start:end]
        setup = "      - name: Install policy dependencies\n        run: |\n          sudo apt-get update\n          sudo apt-get install --yes ripgrep\n"
        assert setup in job_source
        mutated = source[:start] + job_source.replace(setup, "", 1) + source[end:]
        violations = required_workflow_violations(mutated)
        assert violations, f"missing ripgrep in {job} accepted"


def test_required_ci_workflow_merge_gate_fails_closed(repo_root: Path) -> None:
    """Verify merge-gate assertion script fails closed if any need result is not success."""
    ci_file = repo_root / ".github" / "workflows" / "ci.yml"
    workflow = yaml.safe_load(ci_file.read_text(encoding="utf-8"))
    gate = workflow["jobs"]["merge-gate"]
    assert len(gate["steps"]) == 1
    script_template = gate["steps"][0]["run"]

    for result in ["success", "failure", "cancelled", "skipped", ""]:
        for failed in gate["needs"]:
            script = script_template
            for need in gate["needs"]:
                val = result if need == failed else "success"
                script = script.replace(f"${{{{ needs.{need}.result }}}}", val)
            res = subprocess.run(["bash", "-e", "-c", script], capture_output=True)
            if result == "success":
                assert res.returncode == 0, f"result 'success' failed: {res.stderr}"
            else:
                assert res.returncode != 0, f"non-success result {result!r} for {failed} succeeded unexpectedly"


def test_ci_entrypoints_own_required_commands(repo_root: Path) -> None:
    """Verify Makefile rules, phonies, and commands for all CI targets."""
    makefile = (repo_root / "Makefile").read_text(encoding="utf-8")
    targets = {
        "ci-rust-quality": ("", ["scripts/ci/doctor.sh rust", "$(MAKE) rust", "scripts/ci/run-rust-quality.sh"]),
        "ci-frontend": ("", ["scripts/ci/doctor.sh frontend", "$(MAKE) frontend-check"]),
        "ci-native-linux": ("", [
            "scripts/ci/doctor.sh native-linux",
            "$(MAKE) build-linux-server",
            "$(GO) test ./packages/shared/nativeabi ./packages/shared/core ./packages/shared/physics ./packages/client/mesh -race -count=1",
            'scripts/ci/verify-linux-bundle.sh --root "$(CURDIR)"',
            'scripts/ci/package-native-artifact.sh --platform linux-amd64 --sha "$(CI_CANDIDATE_SHA)" --root "$(CURDIR)" --manifest "$(CI_LINUX_MANIFEST)" --',
            "bin/mornlea-server bin/libmornlea_engine.so packages/engine/target/release/libmornlea_engine.so",
        ]),
        "ci-native-macos": ("", [
            "scripts/ci/doctor.sh native-macos",
            "$(MAKE) rust",
            "scripts/ci/platform-id.sh",
            'scripts/ci/package-native-artifact.sh --platform "$$platform" --sha "$(CI_CANDIDATE_SHA)" --root "$(CURDIR)" --manifest "$(CI_MACOS_MANIFEST)" --',
            "packages/engine/target/release/libmornlea_engine.dylib packages/engine/target/release/libmornlea_client.dylib",
        ]),
        "ci-verify-linux-artifact": ("", [
            'scripts/ci/verify-native-artifact.sh --platform linux-amd64 --sha "$(CI_CANDIDATE_SHA)" --root "$(CURDIR)" --manifest "$(CI_LINUX_MANIFEST)"',
        ]),
        "ci-verify-macos-artifact": ("", [
            "scripts/ci/platform-id.sh",
            'scripts/ci/verify-native-artifact.sh --platform "$$platform" --sha "$(CI_CANDIDATE_SHA)" --root "$(CURDIR)" --manifest "$(CI_MACOS_MANIFEST)"',
        ]),
        "ci-linux-quality": ("ci-verify-linux-artifact", ["scripts/ci/run-linux-quality.sh"]),
        "ci-race-server": ("ci-verify-linux-artifact", ["scripts/ci/run-go-race.sh server"]),
        "ci-race-rest": ("ci-verify-linux-artifact", ["scripts/ci/run-go-race.sh rest"]),
        "ci-race-client": ("ci-verify-macos-artifact", ["scripts/ci/run-go-race.sh client"]),
        "ci-integration-server": ("ci-verify-linux-artifact", ["scripts/ci/run-integration-server.sh"]),
        "ci-integration-client": ("ci-verify-macos-artifact", ["scripts/ci/run-integration-client.sh"]),
    }

    for target, (prereq, commands) in targets.items():
        assert _make_target_is_phony(makefile, target), f"{target} is not phony"
        help_recipe = _make_target_recipe(makefile, "help")
        assert f"make {target} " in help_recipe, f"help omits {target}"

        header = f"{target}: {prereq}" if prereq else f"{target}:"
        assert header in makefile.splitlines(), f"missing exact rule {header!r}"

        body = _make_target_recipe(makefile, target)
        last_idx = -1
        for cmd in commands:
            idx = body.find(cmd)
            assert idx >= 0 and idx > last_idx, f"missing or reordered command {cmd!r} in {target}"
            last_idx = idx

        res = subprocess.run(
            ["make", "-n", target, f"CI_CANDIDATE_SHA={'a'*40}"],
            cwd=repo_root,
            capture_output=True,
            text=True,
        )
        assert res.returncode == 0, f"make -n {target} failed: {res.stderr}"
        if prereq:
            assert res.stdout.count("scripts/ci/verify-native-artifact.sh --platform") == 1
            assert res.stdout.find("verify-native-artifact.sh") < res.stdout.find(commands[0])

    for var in [
        "CI_CANDIDATE_SHA ?= $(shell git rev-parse HEAD)",
        "CI_LINUX_MANIFEST := build/ci/native-linux.manifest",
        "CI_MACOS_MANIFEST := build/ci/native-macos.manifest",
    ]:
        assert var in makefile, f"missing {var}"

    # Check transitive dependency boundaries
    edges: dict[str, list[str]] = {}
    for line in makefile.splitlines():
        if line.startswith("\t") or line.startswith("#") or "=" in line:
            continue
        if ":" in line:
            left, right = line.split(":", 1)
            for name in left.split():
                edges.setdefault(name, []).extend(right.split())

    for target in ["build", "test", "run", "companion-agent-check"]:
        queue = deque([target])
        seen = set()
        while queue:
            node = queue.popleft()
            if node in seen:
                continue
            seen.add(node)
            assert not (node.startswith("ci-") or node.startswith("godot-")), f"legacy {target} reaches {node}"
            queue.extend(edges.get(node, []))


def test_linux_quality_platform_exclusions_are_exact(repo_root: Path, tmp_path: Path) -> None:
    """Verify Linux quality package exclusion set and verify command invocations."""
    bin_dir, log_file = _ci_command_recorder(tmp_path)
    real_go = shutil.which("go")
    assert real_go

    recorder = _ci_recorder_source("go").replace("#!/usr/bin/env bash\n", "")
    fake_go_script = f"""#!/usr/bin/env bash
if [[ $1 == list ]]; then printf 'go: downloading example.com/module v1.0.0\\n' >&2; exec "{real_go}" "$@"; fi
if [[ $1 == work ]]; then exec "{real_go}" "$@"; fi
{recorder}"""
    _write_executable(bin_dir / "go", fake_go_script)

    env_path = f"{bin_dir}:{os.environ.get('PATH', '')}"

    def run_inventory(*args: str) -> list[str]:
        cmd = [str(repo_root / "scripts" / "ci" / "package-inventory.sh"), *args]
        res = subprocess.run(cmd, cwd=repo_root, env=_ci_environment(env_path), capture_output=True, text=True)
        assert res.returncode == 0, f"package inventory failed: {res.stderr}"
        assert "go: downloading example.com/module v1.0.0" in res.stderr
        return res.stdout.split()

    all_pkgs = run_inventory("--all")
    client_pkgs = run_inventory("--slice", "client")

    prefix = "github.com/channing771/mornlea/packages/"
    excluded = [
        "client/cmd/mornlea", "client/cmd/mornlea/app", "client/cmd/mornlea/benchmark",
        "client/cmd/mornlea/capture", "client/cmd/mornlea/devcapture",
        "client/render/hud", "tools/gfxspike",
    ]

    for rel in excluded:
        full_pkg = prefix + rel
        assert full_pkg in all_pkgs, f"union lacks {rel}"
        assert full_pkg in client_pkgs, f"client slice lacks {rel}"

    excluded_set = {prefix + rel for rel in excluded}
    supported = sorted([p for p in all_pkgs if p not in excluded_set])

    quality_script = repo_root / "scripts" / "ci" / "run-linux-quality.sh"
    res = subprocess.run(
        [str(quality_script)],
        cwd=repo_root,
        env=_ci_environment(env_path, [f"CI_TEST_LOG={log_file}", f"CI_TEST_ROOT={repo_root}", "CI_TEST_LINUX=1"]),
        capture_output=True,
        text=True,
    )
    assert res.returncode == 0, f"real quality run failed: {res.stderr}\n{res.stdout}"

    commands = log_file.read_text(encoding="utf-8").strip().splitlines()
    assert len(commands) == 3, f"quality commands = {commands}"
    compile_cmd = commands[0].split()
    vet_cmd = commands[1].split()

    assert compile_cmd[2 : len(compile_cmd) - 3] == supported
    assert vet_cmd[2:] == supported


def test_ci_entrypoints_reject_invalid_sha_and_unverified_consumers(repo_root: Path, tmp_path: Path) -> None:
    """Verify CI entrypoint targets fail closed on invalid SHA and unverified artifact dependencies."""
    targets = [
        "ci-native-linux", "ci-native-macos", "ci-linux-quality",
        "ci-race-server", "ci-race-rest", "ci-race-client",
        "ci-integration-server", "ci-integration-client",
    ]
    for target in targets:
        fixture_root = tmp_path / f"fixture_{target}"
        fixture_root.mkdir(parents=True, exist_ok=True)
        (fixture_root / "Makefile").write_text((repo_root / "Makefile").read_text(encoding="utf-8"), encoding="utf-8")
        called = fixture_root / "called"

        for script in ["doctor.sh", "run-linux-quality.sh", "run-go-race.sh", "run-integration-server.sh", "run-integration-client.sh"]:
            _write_executable(fixture_root / "scripts" / "ci" / script, '#!/usr/bin/env bash\nprintf called > "$CI_TEST_CALLED"\n')

        for script in ["verify-native-artifact.sh", "platform-id.sh"]:
            _write_executable(fixture_root / "scripts" / "ci" / script, (repo_root / "scripts" / "ci" / script).read_text(encoding="utf-8"))

        for sha in ["invalid", "A" * 40, "a" * 39]:
            res = subprocess.run(
                ["make", target, f"CI_CANDIDATE_SHA={sha}"],
                cwd=fixture_root,
                env=_ci_environment(os.environ.get("PATH", ""), [
                    f"CI_TEST_CALLED={called}",
                    "MORNLEA_CI_UNAME_S=Darwin",
                    "MORNLEA_CI_UNAME_M=arm64",
                ]),
                capture_output=True,
                text=True,
            )
            assert res.returncode != 0 or "invalid candidate SHA" in res.stdout + res.stderr
            assert not called.exists(), f"invalid candidate ran dependent work on {target}"

        if target.startswith("ci-native-"):
            continue

        res = subprocess.run(
            ["make", target, f"CI_CANDIDATE_SHA={'a'*40}"],
            cwd=fixture_root,
            env=_ci_environment(os.environ.get("PATH", ""), [
                f"CI_TEST_CALLED={called}",
                "MORNLEA_CI_UNAME_S=Darwin",
                "MORNLEA_CI_UNAME_M=arm64",
            ]),
            capture_output=True,
            text=True,
        )
        assert res.returncode != 0, f"missing manifest succeeded on {target}"
        assert not called.exists(), f"unverified consumer ran on {target}"


def test_ci_integration_ordered_commands_and_failures(repo_root: Path, tmp_path: Path) -> None:
    """Verify integration scripts execute ordered commands and halt at failure points."""
    test_cases = [
        ("server", [
            "doctor\tagent",
            "make\tcompanion-agent-check",
            "make\tcompanion-agent-integration",
            "go\ttest\t./packages/server/server\t-run\tTestTCPPlayerAndWorld|TestMemoryTCPParity\t-race\t-count=10",
        ]),
        ("client", [
            "go\ttest\t./packages/client/cmd/mornlea/benchmark\t-run\t^TestScenarioV7EightSessionServerProbeIsRealAndBounded$\t-count=1",
            "go\ttest\t./packages/client/client\t./packages/server/server\t./packages/client/cmd/mornlea/benchmark\t./packages/tools/perfcheck\t-run\tTest(PerfReportV6|ScenarioV6|PerfcheckV6|PerfcheckV5SameScenario|PerformanceThresholds|InterestObserver|HostStats|BenchmarkServerEpoch|BenchmarkServerMeasuredWindow)\t-count=1",
            "go\ttest\t./packages/contracts/...\t./packages/shared/...\t./packages/server/...\t./packages/client/...\t./packages/tools/...\t-bench=.\t-benchtime=1x\t-run=^$",
            "go\ttest\t./packages/shared/network\t./packages/server/server\t./packages/client/render\t-run\t^$\t-bench\t(RemotePlayerStateCodec|EightPlayerInterest|RemoteAvatarNameTag)\t-benchmem\t-benchtime=100x\t-count=1",
        ]),
    ]

    for name, commands in test_cases:
        for fail_at in range(len(commands) + 1):
            fixture_root = tmp_path / f"int_{name}_{fail_at}"
            fixture_root.mkdir(parents=True, exist_ok=True)
            script_path = fixture_root / "scripts" / "ci" / f"run-integration-{name}.sh"
            _write_executable(script_path, (repo_root / "scripts" / "ci" / f"run-integration-{name}.sh").read_text(encoding="utf-8"))
            bin_dir, log_file = _ci_command_recorder(fixture_root)
            _write_executable(fixture_root / "scripts" / "ci" / "doctor.sh", _ci_recorder_source("doctor"))

            res = subprocess.run(
                [str(script_path)],
                cwd=fixture_root,
                env=_ci_environment(f"{bin_dir}:{os.environ.get('PATH', '')}", [
                    f"CI_TEST_LOG={log_file}",
                    f"CI_TEST_FAIL_AT={fail_at}",
                    f"CI_TEST_ROOT={fixture_root}",
                ]),
                capture_output=True,
                text=True,
            )
            expected_fail = fail_at > 0
            assert (res.returncode != 0) == expected_fail

            want = commands[:fail_at] if fail_at > 0 else commands
            got = log_file.read_text(encoding="utf-8").strip().splitlines() if log_file.exists() else []
            assert got == want, f"{name} fail_at={fail_at}: got {got}, want {want}"


def test_ci_rust_quality_real_fixture_preparation(repo_root: Path, tmp_path: Path) -> None:
    """Verify run-rust-quality.sh workflow preparation across modes."""
    previous = "d042982d33bb1694d768b75b01c297bd02534a08"
    modes = ["pass", "missing-source", "install-error", "fetch-error", "prepare-error", "check-error", "arguments"]

    for mode in modes:
        fixture_root = tmp_path / f"rust_quality_{mode}"
        fixture_root.mkdir(parents=True, exist_ok=True)
        script = fixture_root / "scripts" / "ci" / "run-rust-quality.sh"
        _write_executable(script, (repo_root / "scripts" / "ci" / "run-rust-quality.sh").read_text(encoding="utf-8"))

        bin_dir, log_file = _ci_command_recorder(fixture_root)
        (fixture_root / "packages" / "agent" / ".venv" / "bin").mkdir(parents=True, exist_ok=True)
        (fixture_root / "packages" / "agent" / ".venv" / "bin" / "python").write_text("", encoding="utf-8")
        _write_executable(fixture_root / "packages" / "agent" / ".venv" / "bin" / "python3", "#!/usr/bin/env bash\nexit 0\n")

        uv_src = f"""#!/usr/bin/env bash
[[ $PWD -ef "$CI_TEST_ROOT/packages/agent" ]] || exit 93
cd "$CI_TEST_ROOT"
{_ci_recorder_source('uv').replace('#!/usr/bin/env bash\n', '')}"""
        _write_executable(bin_dir / "uv", uv_src)

        git_src = _ci_recorder_source("git")
        if mode in ("missing-source", "fetch-error"):
            git_src += "[[ $1 != cat-file ]] || exit 1\n"
        _write_executable(bin_dir / "git", git_src)

        prepare_script = f"""#!/usr/bin/env bash
[[ $(command -v python3) == "$CI_TEST_ROOT/packages/agent/.venv/bin/python3" ]] || exit 97
{_ci_recorder_source('prepare').replace('#!/usr/bin/env bash\n', '')}
printf '%s' "$5" > "$CI_TEST_ROOT/prepared-root"
touch "$5/previous-server" "$5/previous-runtime.json"
"""
        _write_executable(fixture_root / "scripts" / "rust-server-opt-in.sh", prepare_script)

        make_src = f"""#!/usr/bin/env bash
[[ $MORNLEA_AGENT_PYTHON == "$CI_TEST_ROOT/packages/agent/.venv/bin/python" ]] || exit 94
[[ $MORNLEA_PREVIOUS_SERVER_BIN == "$(cat "$CI_TEST_ROOT/prepared-root")/previous-server" ]] || exit 95
[[ $MORNLEA_PREVIOUS_PACKAGE == "$(cat "$CI_TEST_ROOT/prepared-root")/previous-runtime.json" ]] || exit 96
{_ci_recorder_source('make').replace('#!/usr/bin/env bash\n', '')}"""
        _write_executable(bin_dir / "make", make_src)

        fail_at_map = {"install-error": 1, "fetch-error": 3, "prepare-error": 3, "check-error": 4}
        fail_at = fail_at_map.get(mode, 0)
        args = ["unexpected"] if mode == "arguments" else []

        res = subprocess.run(
            [str(script), *args],
            cwd=fixture_root,
            env=_ci_environment(f"{bin_dir}:{os.environ.get('PATH', '')}", [
                f"CI_TEST_ROOT={fixture_root}",
                f"CI_TEST_LOG={log_file}",
                f"CI_TEST_FAIL_AT={fail_at}",
            ]),
            capture_output=True,
            text=True,
        )
        expected_fail = mode not in ("pass", "missing-source")
        assert (res.returncode != 0) == expected_fail

        if mode == "arguments":
            assert not log_file.exists()
            continue

        got = log_file.read_text(encoding="utf-8").strip().splitlines()
        want = ["uv\tsync\t--locked", f"git\tcat-file\t-e\t{previous}^{{commit}}"]
        if mode in ("missing-source", "fetch-error"):
            want.append(f"git\tfetch\t--no-tags\torigin\t{previous}")

        if len(got) > len(want):
            prepare_cmd = got[len(want)].split("\t")
            assert len(prepare_cmd) == 6
            assert prepare_cmd[:5] == ["prepare", "prepare-previous", "--source", previous, "--run-dir"]
            assert Path(prepare_cmd[5]).is_absolute()
            want.append(got[len(want)])
            want.append("make\trust-check")

        if fail_at > 0:
            assert len(got) == fail_at
            want = want[:fail_at]

        assert got == want


def test_ci_linux_quality_ordered_commands_and_failures(repo_root: Path, tmp_path: Path) -> None:
    """Verify run-linux-quality.sh execution flow across failure modes."""
    prefix = "github.com/channing771/mornlea/packages/"
    modes = ["pass", "doctor-error", "inventory-error", "empty", "compile-error", "vet-error", "focused-error"]

    for mode in modes:
        fixture_root = tmp_path / f"lq_{mode}"
        fixture_root.mkdir(parents=True, exist_ok=True)
        script = fixture_root / "scripts" / "ci" / "run-linux-quality.sh"
        _write_executable(script, (repo_root / "scripts" / "ci" / "run-linux-quality.sh").read_text(encoding="utf-8"))

        bin_dir, log_file = _ci_command_recorder(fixture_root)
        _write_executable(fixture_root / "scripts" / "ci" / "doctor.sh", _ci_recorder_source("doctor"))

        inv_src = "#!/usr/bin/env bash\n[[ $# == 1 && $1 == --all ]] || exit 92\n"
        if mode == "inventory-error":
            inv_src += "printf 'partial/package\\n'; exit 42\n"
        elif mode == "empty":
            inv_src += "exit 0\n"
        else:
            inv_src += f"printf '%s\\n' '{prefix}audit' '{prefix}client/cmd/mornlea/app' '{prefix}client/cmd/mornlea/capture' '{prefix}client/cmd/mornlea/devcapture' '{prefix}tools/gfxspike' '{prefix}tools/perfcheck'\n"

        _write_executable(fixture_root / "scripts" / "ci" / "package-inventory.sh", inv_src)
        fail_at_map = {"doctor-error": 1, "compile-error": 2, "vet-error": 3, "focused-error": 4}
        fail_at = fail_at_map.get(mode, 0)

        res = subprocess.run(
            [str(script)],
            cwd=fixture_root,
            env=_ci_environment(f"{bin_dir}:{os.environ.get('PATH', '')}", [
                f"CI_TEST_LOG={log_file}",
                f"CI_TEST_FAIL_AT={fail_at}",
                f"CI_TEST_ROOT={fixture_root}",
                "CI_TEST_LINUX=1",
            ]),
            capture_output=True,
            text=True,
        )
        assert (res.returncode != 0) == (mode != "pass")

        want = [
            "doctor\taudit",
            f"go\ttest\t{prefix}audit\t{prefix}tools/perfcheck\t-run\t^$\t-count=1",
            f"go\tvet\t{prefix}audit\t{prefix}tools/perfcheck",
            "go\ttest\t./packages/server/storage/...\t./packages/shared/network/...\t./packages/shared/physics\t-v",
        ]
        if mode in ("inventory-error", "empty"):
            want = want[:1]
        elif fail_at > 0:
            want = want[:fail_at]

        got = log_file.read_text(encoding="utf-8").strip().splitlines()
        assert got == want


def test_ci_audit_entrypoints_reject_missing_ripgrep_before_work(repo_root: Path, tmp_path: Path) -> None:
    """Verify audit CI entrypoints verify ripgrep before doing work."""
    for name, script_name, args in [("linux quality", "run-linux-quality.sh", []), ("rest race", "run-go-race.sh", ["rest"])]:
        fixture_root = tmp_path / f"rg_{script_name}"
        fixture_root.mkdir(parents=True, exist_ok=True)
        script = fixture_root / "scripts" / "ci" / script_name
        _write_executable(script, (repo_root / "scripts" / "ci" / script_name).read_text(encoding="utf-8"))
        _write_executable(fixture_root / "scripts" / "ci" / "doctor.sh", (repo_root / "scripts" / "ci" / "doctor.sh").read_text(encoding="utf-8"))

        called = fixture_root / "inventory-called"
        _write_executable(fixture_root / "scripts" / "ci" / "package-inventory.sh", '#!/usr/bin/env bash\nprintf called > "$CI_TEST_CALLED"\n')

        bin_dir = _ci_fixture_bin(fixture_root, ["bash", "go", "gofmt", "python3", "uv"])
        dirname_path = shutil.which("dirname")
        assert dirname_path
        (bin_dir / "dirname").symlink_to(dirname_path)

        res = subprocess.run(
            [str(script), *args],
            cwd=fixture_root,
            env=_ci_environment(str(bin_dir), [f"CI_TEST_CALLED={called}"]),
            capture_output=True,
            text=True,
        )
        assert res.returncode != 0
        assert "missing required executable for audit: rg" in res.stdout + res.stderr
        assert "passed" not in res.stdout + res.stderr
        assert not called.exists()


def test_ci_race_server_and_client_do_not_require_ripgrep(repo_root: Path, tmp_path: Path) -> None:
    """Verify server and client slices of run-go-race.sh do not require ripgrep."""
    for slice_name in ["server", "client"]:
        fixture_root = tmp_path / f"race_{slice_name}"
        fixture_root.mkdir(parents=True, exist_ok=True)
        script = fixture_root / "scripts" / "ci" / "run-go-race.sh"
        _write_executable(script, (repo_root / "scripts" / "ci" / "run-go-race.sh").read_text(encoding="utf-8"))
        _write_executable(fixture_root / "scripts" / "ci" / "package-inventory.sh", "#!/usr/bin/env bash\nprintf 'example/package\\n'\n")

        bin_dir = _ci_fixture_bin(fixture_root, ["bash", "go", "gofmt"])
        dirname_path = shutil.which("dirname")
        assert dirname_path
        (bin_dir / "dirname").symlink_to(dirname_path)

        called = fixture_root / "go-test-called"
        _write_executable(bin_dir / "go", '#!/usr/bin/env bash\nprintf \'%s\\n\' "$@" > "$CI_TEST_CALLED"\n')

        res = subprocess.run(
            [str(script), slice_name],
            cwd=fixture_root,
            env=_ci_environment(str(bin_dir), [f"CI_TEST_CALLED={called}"]),
            capture_output=True,
            text=True,
        )
        assert res.returncode == 0, f"race {slice_name} failed: {res.stderr}"
        got = called.read_text(encoding="utf-8").split()
        assert len(got) >= 3 and got[0] == "test" and got[1] == "example/package"


def test_ci_linux_bundle_restores_target_and_rejects_mutations(repo_root: Path, tmp_path: Path) -> None:
    """Verify verify-linux-bundle.sh target preservation and failure diagnostics across modes."""
    modes = [
        "pass", "dependency", "go-error", "needed", "origin", "readelf-error",
        "abi", "mesh", "collision", "raycast", "nm-error", "ldd-error",
        "wrong-library", "help-success", "help-text", "loader-error", "loader-open",
    ]

    for mode in modes:
        fixture_root = tmp_path / f"bundle_{mode}"
        fixture_root.mkdir(parents=True, exist_ok=True)
        script = fixture_root / "scripts" / "ci" / "verify-linux-bundle.sh"
        _write_executable(script, (repo_root / "scripts" / "ci" / "verify-linux-bundle.sh").read_text(encoding="utf-8"))

        marker = fixture_root / "packages" / "engine" / "target" / "marker"
        marker.parent.mkdir(parents=True, exist_ok=True)
        marker.write_text("preserve native cache", encoding="utf-8")

        bin_dir = fixture_root / "bin_tools"
        bin_dir.mkdir(parents=True, exist_ok=True)

        _write_executable(bin_dir / "go", """#!/usr/bin/env bash
[[ "$*" == 'list -deps ./packages/server/cmd/mornlea-server' && $GOOS == linux && $GOARCH == amd64 && $CGO_ENABLED == 1 ]] || exit 92
[[ $CI_TEST_MODE != go-error ]] || exit 42
printf 'example/server\n'
[[ $CI_TEST_MODE != dependency ]] || printf 'github.com/channing771/mornlea/packages/client/render\n'
""")
        _write_executable(bin_dir / "readelf", """#!/usr/bin/env bash
[[ "$*" == '-d bin/mornlea-server' ]] || exit 92
[[ $CI_TEST_MODE != readelf-error ]] || exit 42
[[ $CI_TEST_MODE == needed ]] || printf 'NEEDED libmornlea_engine.so\n'
[[ $CI_TEST_MODE == origin ]] || printf 'RUNPATH $ORIGIN\n'
exit 0
""")
        _write_executable(bin_dir / "nm", """#!/usr/bin/env bash
[[ "$*" == '-D --defined-only bin/libmornlea_engine.so' ]] || exit 92
[[ $CI_TEST_MODE != nm-error ]] || exit 42
for entry in abi:mornlea_engine_abi_version mesh:mornlea_mesh_section collision:mornlea_collision_resolve raycast:mornlea_raycast_batch; do
  [[ $CI_TEST_MODE == "${entry%%:*}" ]] || printf '0001 T %s\n' "${entry#*:}"
done
exit 0
""")
        _write_executable(bin_dir / "ldd", """#!/usr/bin/env bash
[[ "$*" == 'bin/mornlea-server' && ! -e packages/engine/target ]] || exit 92
printf called > "$CI_TEST_PROBED"
[[ $CI_TEST_MODE != ldd-error ]] || exit 42
if [[ $CI_TEST_MODE == wrong-library ]]; then printf '/wrong/libmornlea_engine.so\n'; else printf 'libmornlea_engine.so => %s/bin/libmornlea_engine.so (0x1)\n' "$PWD"; fi
""")
        _write_executable(fixture_root / "bin" / "mornlea-server", """#!/usr/bin/env bash
[[ "$*" == '-h' && ! -e packages/engine/target ]] || exit 92
[[ $CI_TEST_MODE != help-success ]] || exit 0
[[ $CI_TEST_MODE == help-text ]] || printf 'flag: help requested\n'
[[ $CI_TEST_MODE != loader-error ]] || printf 'error while loading shared libraries\n'
[[ $CI_TEST_MODE != loader-open ]] || printf 'cannot open shared object\n'
exit 1
""")
        probed = fixture_root / "probed"
        res = subprocess.run(
            [str(script), "--root", str(fixture_root)],
            cwd=fixture_root,
            env=_ci_environment(f"{bin_dir}:{os.environ.get('PATH', '')}", [
                f"CI_TEST_MODE={mode}",
                f"CI_TEST_PROBED={probed}",
            ]),
            capture_output=True,
            text=True,
        )
        assert (res.returncode != 0) == (mode != "pass")
        assert marker.read_text(encoding="utf-8") == "preserve native cache"

        before_detach = mode in ["dependency", "go-error", "needed", "origin", "readelf-error", "abi", "mesh", "collision", "raycast", "nm-error"]
        if before_detach:
            assert not probed.exists()
        else:
            assert probed.exists()


def test_ci_doctor_profiles_and_failures(repo_root: Path, tmp_path: Path) -> None:
    """Verify doctor.sh validation profiles, failure diagnostics, and exit codes."""
    script = repo_root / "scripts" / "ci" / "doctor.sh"
    profiles = {
        "preflight": ["bash", "git", "go", "gofmt", "node", "npx", "python3", "rg", "uv"],
        "audit": ["bash", "python3", "uv", "rg"],
        "frontend": ["bash", "corepack", "git", "node"],
        "rust": ["bash", "cargo", "cc", "git", "go", "gofmt", "make", "python3", "rustc", "rustup", "uv"],
        "go": ["bash", "go", "gofmt"],
        "native-linux": ["bash", "cargo", "cc", "go", "ldd", "make", "nm", "readelf", "rustc", "rustup", "shasum"],
        "native-macos": ["bash", "cargo", "codesign", "go", "install_name_tool", "make", "nm", "rustc", "rustup", "shasum"],
        "agent": ["bash", "go", "python3", "uv"],
        "godot-static": ["bash", "make", "rg"],
        "godot-runtime": ["bash", "uname", "cargo", "cc", "clang++", "codesign", "curl", "ditto", "git", "go", "install_name_tool", "make", "nm", "patch", "perl", "pgrep", "rg", "rustc", "rustup", "sandbox-exec", "shasum", "tar", "unzip", "uv", "xcrun"],
    }

    for profile, commands in profiles.items():
        bin_dir = _ci_fixture_bin(tmp_path / f"doc_{profile}", commands)
        res = subprocess.run([str(script), profile], cwd=repo_root, env=_ci_environment(str(bin_dir)), capture_output=True, text=True)
        assert res.returncode == 0, f"doctor {profile} failed: {res.stderr}"
        assert res.stdout + res.stderr == f"CI dependency profile passed: {profile}\n"

        if profile.startswith("godot-") or profile in ("audit", "rust"):
            for missing in commands:
                if missing == "bash":
                    continue
                selected = [c for c in commands if c != missing]
                missing_bin = _ci_fixture_bin(tmp_path / f"doc_{profile}_miss_{missing}", selected)
                res = subprocess.run([str(script), profile], cwd=repo_root, env=_ci_environment(str(missing_bin)), capture_output=True, text=True)
                want = f"missing required executable for {profile}: {missing}\n"
                assert res.returncode != 0 and (res.stdout + res.stderr) == want

    # Reports in lexical order
    bin_dir = _ci_fixture_bin(tmp_path / "doc_order", ["bash", "git", "go", "gofmt", "node"])
    res = subprocess.run([str(script), "preflight"], cwd=repo_root, env=_ci_environment(str(bin_dir)), capture_output=True, text=True)
    assert res.returncode != 0
    assert (res.stdout + res.stderr) == (
        "missing required executable for preflight: npx\n"
        "missing required executable for preflight: python3\n"
        "missing required executable for preflight: rg\n"
        "missing required executable for preflight: uv\n"
    )

    # Unknown profile
    res = subprocess.run([str(script), "unknown"], cwd=repo_root, env=_ci_environment(str(bin_dir)), capture_output=True, text=True)
    assert res.returncode == 2
    assert "usage: doctor.sh <" in (res.stdout + res.stderr)


def test_ci_package_partitions_reject_mutations(repo_root: Path, tmp_path: Path) -> None:
    """Verify check-package-partitions.sh detects invalid or overlapping module package partitions."""
    script = repo_root / "scripts" / "ci" / "check-package-partitions.sh"

    def make_fixture(mutate_fn=None) -> list[str]:
        fix_dir = tmp_path / "part_fix"
        if fix_dir.exists():
            shutil.rmtree(fix_dir)
        fix_dir.mkdir(parents=True)
        files = {
            "all": "example/a\nexample/b\nexample/c\nexample/d\nexample/e\n",
            "client": "example/a\nexample/b\n",
            "server": "example/c\n",
            "rest": "example/d\nexample/e\n",
        }
        if mutate_fn:
            mutate_fn(files)
        for name, text in files.items():
            (fix_dir / name).write_text(text, encoding="utf-8")
        return [str(fix_dir / "all"), str(fix_dir / "client"), str(fix_dir / "server"), str(fix_dir / "rest")]

    # Valid
    paths = make_fixture()
    res = subprocess.run([str(script), *paths], cwd=repo_root, capture_output=True, text=True)
    assert res.returncode == 0, f"valid partition failed: {res.stderr}"

    cases = [
        ("duplicate package", lambda f: f.update({"client": "example/a\nexample/a\n"}), "duplicate package in client: example/a"),
        ("client server overlap", lambda f: f.update({"server": "example/b\nexample/c\n"}), "overlapping package: example/b"),
        ("missing package", lambda f: f.update({"rest": "example/d\n"}), "missing package: example/e"),
        ("unexpected package", lambda f: f.update({"rest": "example/d\nexample/e\nexample/z\n"}), "unexpected package: example/z"),
        ("unsorted package", lambda f: f.update({"client": "example/b\nexample/a\n"}), "package list is not lexically sorted in client: example/a"),
        ("empty slice", lambda f: f.update({"rest": ""}), "package list is empty: rest"),
    ]

    for name, mutate, want in cases:
        paths = make_fixture(mutate)
        res = subprocess.run([str(script), *paths], cwd=repo_root, capture_output=True, text=True)
        assert res.returncode != 0
        assert want in res.stdout + res.stderr, f"{name}: output {res.stdout} missing {want}"


def test_ci_package_partitions_accept_colon_paths_and_reject_comparison_failure(repo_root: Path, tmp_path: Path) -> None:
    """Verify check-package-partitions.sh supports colon paths and handles comm tool failure."""
    script = repo_root / "scripts" / "ci" / "check-package-partitions.sh"
    colon_dir = tmp_path / "colon:package-lists"
    colon_dir.mkdir(parents=True, exist_ok=True)
    files = {
        "all": "example/a\nexample/b\nexample/c\nexample/d\nexample/e\n",
        "client": "example/a\nexample/b\n",
        "server": "example/c\n",
        "rest": "example/d\nexample/e\n",
    }
    colon_paths = []
    for name, text in files.items():
        p = colon_dir / f"{name}:input"
        p.write_text(text, encoding="utf-8")
        colon_paths.append(str(p))

    res = subprocess.run([str(script), *colon_paths], cwd=repo_root, capture_output=True, text=True)
    assert res.returncode == 0, f"colon path partition failed: {res.stderr}"

    bin_dir = tmp_path / "fail_comm_bin"
    _write_executable(bin_dir / "comm", "#!/usr/bin/env bash\nexit 42\n")
    res = subprocess.run(
        [str(script), *colon_paths],
        cwd=repo_root,
        env=_ci_environment(f"{bin_dir}:{os.environ.get('PATH', '')}"),
        capture_output=True,
        text=True,
    )
    assert res.returncode != 0
    assert "package partition comparison failed" in res.stdout + res.stderr


def test_ci_package_inventory_rejects_unexpected_workspace_path_and_load_errors(repo_root: Path, tmp_path: Path) -> None:
    """Verify package-inventory.sh fails on unexpected module directories or loading errors."""
    script = repo_root / "scripts" / "ci" / "package-inventory.sh"
    real_go = shutil.which("go")
    assert real_go

    # Space path
    bin_space = tmp_path / "bin_space"
    fake_go_space = f"""#!/usr/bin/env bash
if [[ "${{1:-}}" == work && "${{2:-}}" == edit ]]; then
\tprintf '%s\\n' '{{' '  "Use": [' '    {{' '      "DiskPath": "./packages/client"' '    }},' '    {{' '      "DiskPath": "./packages/contracts"' '    }},' '    {{' '      "DiskPath": "./packages/server"' '    }},' '    {{' '      "DiskPath": "./packages/shared"' '    }},' '    {{' '      "DiskPath": "./packages/tools"' '    }},' '    {{' '      "DiskPath": "./packages/extra path"' '    }}' '  ]' '}}'
\texit 0
fi
exec "{real_go}" "$@"
"""
    _write_executable(bin_space / "go", fake_go_space)
    res = subprocess.run(
        [str(script), "--slice", "server"],
        cwd=repo_root,
        env=_ci_environment(f"{bin_space}:{os.environ.get('PATH', '')}"),
        capture_output=True,
        text=True,
    )
    assert res.returncode != 0
    assert "unexpected go.work module directories" in res.stdout + res.stderr

    # Loading error
    bin_err = tmp_path / "bin_err"
    fake_go_err = f"""#!/usr/bin/env bash
if [[ "${{1:-}}" == list ]]; then
\tif [[ "$*" == *'.Error'* ]]; then printf 'example/bad|error\\n'; else printf 'example/bad\\n'; fi
\texit 0
fi
exec "{real_go}" "$@"
"""
    _write_executable(bin_err / "go", fake_go_err)
    res = subprocess.run(
        [str(script), "--slice", "server"],
        cwd=repo_root,
        env=_ci_environment(f"{bin_err}:{os.environ.get('PATH', '')}"),
        capture_output=True,
        text=True,
    )
    assert res.returncode != 0
    assert "package loading error" in res.stdout + res.stderr


def test_ci_repository_package_inventory(repo_root: Path) -> None:
    """Verify package-inventory.sh --check succeeds and inventory slices are sorted and correct."""
    script = repo_root / "scripts" / "ci" / "package-inventory.sh"
    res = subprocess.run([str(script), "--check"], cwd=repo_root, capture_output=True, text=True)
    assert res.returncode == 0, f"package inventory check failed: {res.stderr}"

    def get_slice(name: str) -> str:
        r = subprocess.run([str(script), "--slice", name], cwd=repo_root, capture_output=True, text=True)
        assert r.returncode == 0
        return r.stdout

    client = get_slice("client")
    rest = get_slice("rest")
    server = get_slice("server")

    gfxspike = "github.com/channing771/mornlea/packages/tools/gfxspike"
    assert client.count(gfxspike) == 1
    assert gfxspike not in rest

    native_abi = "github.com/channing771/mornlea/packages/shared/nativeabi"
    assert rest.count(native_abi) == 1

    inv_source = script.read_text(encoding="utf-8")
    for req in ["CGO_ENABLED=1", "list_module linux amd64", "list_module darwin arm64"]:
        assert req in inv_source

    for s_name, contents in [("client", client), ("server", server), ("rest", rest)]:
        lines = contents.split()
        assert len(lines) > 0, f"{s_name} is empty"
        assert lines == sorted(lines), f"{s_name} not sorted"


def test_ci_package_inventory_queries_every_module_on_both_platforms(repo_root: Path, tmp_path: Path) -> None:
    """Verify matrix inventory queries each module across linux/amd64 and darwin/arm64."""
    script = repo_root / "scripts" / "ci" / "package-inventory.sh"
    real_go = shutil.which("go")
    assert real_go

    bin_dir = tmp_path / "bin_query"
    query_log = tmp_path / "queries"
    fake_go = f"""#!/usr/bin/env bash
if [[ "${{1:-}}" == list ]]; then
\tprintf '%s/%s %s\\n' "$GOOS" "$GOARCH" "$PWD" >> "$MORNLEA_CI_QUERY_LOG"
\tprintf 'example/%s/%s|\\n' "$GOOS" "$GOARCH"
\texit 0
fi
exec "{real_go}" "$@"
"""
    _write_executable(bin_dir / "go", fake_go)

    res = subprocess.run(
        [str(script), "--all"],
        cwd=repo_root,
        env=_ci_environment(f"{bin_dir}:{os.environ.get('PATH', '')}", [f"MORNLEA_CI_QUERY_LOG={query_log}"]),
        capture_output=True,
        text=True,
    )
    assert res.returncode == 0, f"matrix inventory failed: {res.stderr}"

    lines = [l.replace(f"{repo_root}/", "") for l in query_log.read_text(encoding="utf-8").strip().splitlines()]
    lines.sort()

    want = []
    for platform in ["linux/amd64", "darwin/arm64"]:
        for module in ["packages/client", "packages/contracts", "packages/server", "packages/shared", "packages/tools"]:
            want.append(f"{platform} {module}")
    want.sort()

    assert lines == want


def test_ci_race_entrypoint_arguments(repo_root: Path, tmp_path: Path) -> None:
    """Verify run-go-race.sh passes arguments and skip filters properly."""
    script = repo_root / "scripts" / "ci" / "run-go-race.sh"
    real_go = shutil.which("go")
    assert real_go

    bin_dir = tmp_path / "bin_race"
    argv_log = tmp_path / "argv"
    fake_go = f"""#!/usr/bin/env bash
if [[ "${{1:-}}" == test ]]; then
\tprintf '%s\\n' "$@" > "$MORNLEA_CI_GO_ARGV"
\texit 0
fi
exec "{real_go}" "$@"
"""
    _write_executable(bin_dir / "go", fake_go)

    cases = [
        ("client skips bounded server probe", "client", "github.com/channing771/mornlea/packages/tools/gfxspike", ["-race", "-p=1", "-skip", "^TestScenarioV7EightSessionServerProbeIsRealAndBounded$"]),
        ("server has no client skip", "server", "github.com/channing771/mornlea/packages/server/server", ["-race", "-p=1"]),
        ("rest has no client skip", "rest", "github.com/channing771/mornlea/packages/shared/nativeabi", ["-race", "-p=1"]),
    ]

    for name, slice_name, want_pkg, want_tail in cases:
        if argv_log.exists():
            argv_log.unlink()
        res = subprocess.run(
            [str(script), slice_name],
            cwd=repo_root,
            env=_ci_environment(f"{bin_dir}:{os.environ.get('PATH', '')}", [f"MORNLEA_CI_GO_ARGV={argv_log}"]),
            capture_output=True,
            text=True,
        )
        assert res.returncode == 0, f"race {slice_name} failed: {res.stderr}"
        got = argv_log.read_text(encoding="utf-8").split()
        assert len(got) >= len(want_tail) + 2
        assert got[0] == "test"
        assert want_pkg in got
        assert got[len(got) - len(want_tail) :] == want_tail

    # Rejects unknown or empty
    for invalid in ["unknown", ""]:
        if argv_log.exists():
            argv_log.unlink()
        res = subprocess.run(
            [str(script), invalid],
            cwd=repo_root,
            env=_ci_environment(f"{bin_dir}:{os.environ.get('PATH', '')}", [f"MORNLEA_CI_GO_ARGV={argv_log}"]),
            capture_output=True,
            text=True,
        )
        assert res.returncode != 0
        assert not argv_log.exists()


def test_ci_race_entrypoint_rejects_empty_inventory_before_go_test(repo_root: Path, tmp_path: Path) -> None:
    """Verify run-go-race.sh stops if package inventory produces no packages."""
    fixture_root = tmp_path / "race_empty"
    fixture_root.mkdir(parents=True, exist_ok=True)
    script = fixture_root / "scripts" / "ci" / "run-go-race.sh"
    _write_executable(script, (repo_root / "scripts" / "ci" / "run-go-race.sh").read_text(encoding="utf-8"))

    bin_dir = _ci_fixture_bin(fixture_root, ["bash"])
    dirname_path = shutil.which("dirname")
    assert dirname_path
    (bin_dir / "dirname").symlink_to(dirname_path)

    argv_log = fixture_root / "argv"
    inv_called = fixture_root / "inventory-called"
    _write_executable(fixture_root / "scripts" / "ci" / "package-inventory.sh", '#!/usr/bin/env bash\nprintf called > "$MORNLEA_CI_INVENTORY_CALLED"\n')

    bash_env = fixture_root / "mapfile.sh"
    mapfile_called = fixture_root / "mapfile-called"
    bash_env.write_text("mapfile() { printf called > \"$MORNLEA_CI_MAPFILE_CALLED\"; eval \"$2=('')\"; }\n", encoding="utf-8")
    _write_executable(bin_dir / "go", '#!/usr/bin/env bash\nprintf \'%s\\n\' "$@" > "$MORNLEA_CI_GO_ARGV"\n')

    res = subprocess.run(
        [str(script), "server"],
        cwd=fixture_root,
        env=_ci_environment(str(bin_dir), [
            f"BASH_ENV={bash_env}",
            f"MORNLEA_CI_GO_ARGV={argv_log}",
            f"MORNLEA_CI_INVENTORY_CALLED={inv_called}",
            f"MORNLEA_CI_MAPFILE_CALLED={mapfile_called}",
        ]),
        capture_output=True,
        text=True,
    )
    assert res.returncode != 0
    assert inv_called.read_text(encoding="utf-8") == "called"
    assert not mapfile_called.exists()
    assert not argv_log.exists()


def test_ci_race_entrypoint_fallback_reader(repo_root: Path, tmp_path: Path) -> None:
    """Verify run-go-race.sh fallback reader works when mapfile is unavailable."""
    fixture_root = tmp_path / "race_fallback"
    fixture_root.mkdir(parents=True, exist_ok=True)
    script = fixture_root / "scripts" / "ci" / "run-go-race.sh"
    _write_executable(script, (repo_root / "scripts" / "ci" / "run-go-race.sh").read_text(encoding="utf-8"))
    _write_executable(fixture_root / "scripts" / "ci" / "package-inventory.sh", "#!/usr/bin/env bash\nprintf 'example/server\\n'\n")

    bin_dir = _ci_fixture_bin(fixture_root, ["bash"])
    dirname_path = shutil.which("dirname")
    assert dirname_path
    (bin_dir / "dirname").symlink_to(dirname_path)

    argv_log = fixture_root / "argv"
    forced_fallback = fixture_root / "forced-fallback"
    primary_reader = fixture_root / "primary-reader"
    bash_env = fixture_root / "fallback.sh"
    bash_env.write_text("""type() { if [[ "$1" == mapfile ]]; then printf forced > "$MORNLEA_CI_FORCED_FALLBACK"; return 1; fi; builtin type "$@"; }
mapfile() { printf called > "$MORNLEA_CI_PRIMARY_READER"; }
""", encoding="utf-8")
    _write_executable(bin_dir / "go", '#!/usr/bin/env bash\nprintf \'%s\\n\' "$@" > "$MORNLEA_CI_GO_ARGV"\n')

    res = subprocess.run(
        [str(script), "server"],
        cwd=fixture_root,
        env=_ci_environment(str(bin_dir), [
            f"BASH_ENV={bash_env}",
            f"MORNLEA_CI_FORCED_FALLBACK={forced_fallback}",
            f"MORNLEA_CI_GO_ARGV={argv_log}",
            f"MORNLEA_CI_PRIMARY_READER={primary_reader}",
        ]),
        capture_output=True,
        text=True,
    )
    assert res.returncode == 0, f"fallback reader failed: {res.stderr}"
    assert forced_fallback.read_text(encoding="utf-8") == "forced"
    assert not primary_reader.exists()
    assert argv_log.read_text(encoding="utf-8").split() == ["test", "example/server", "-race", "-p=1"]


def test_ci_preflight_recipe_order_and_boundary(repo_root: Path) -> None:
    """Verify ci-preflight recipe command order and boundary exclusions."""
    makefile = (repo_root / "Makefile").read_text(encoding="utf-8")
    recipe = _make_target_recipe(makefile, "ci-preflight")

    ordered = [
        "scripts/ci/doctor.sh preflight",
        "gofmt -l",
        "npx --yes @fission-ai/openspec@1.7.0 validate --all --strict --no-interactive",
        "node --test scripts/agent-hooks/guard.test.mjs",
        "$(MAKE) comment-language-check",
        "scripts/ci/package-inventory.sh --check",
        "uv run pytest packages/audit",
    ]

    last = -1
    for cmd in ordered:
        idx = recipe.find(cmd)
        assert idx >= 0, f"ci-preflight is missing {cmd!r}"
        assert idx > last, f"ci-preflight command order is wrong around {cmd!r}"
        last = idx

    audit_cmd = ordered[-1]
    audit_invocations = 0
    for line in recipe.splitlines():
        if line.strip().startswith("uv run pytest packages/audit"):
            audit_invocations += 1
            assert line.strip() == audit_cmd
    assert audit_invocations == 1

    for forbidden in ["cargo", "make rust", "package-native-artifact", "verify-native-artifact", "scripts/godot", "godot-"]:
        assert forbidden not in recipe, f"ci-preflight must not invoke {forbidden!r}"


def test_ci_preflight_stops_before_next_command_when_gofmt_fails(repo_root: Path, tmp_path: Path) -> None:
    """Verify ci-preflight halts immediately if gofmt discovers unformatted files."""
    bin_dir = _ci_fixture_bin(tmp_path, ["bash", "git", "go", "gofmt", "node", "npx", "rg"])
    next_cmd_marker = tmp_path / "next-command-ran"

    _write_executable(bin_dir / "git", "#!/usr/bin/env bash\nprintf 'name with spaces.go\\000'\n")
    _write_executable(bin_dir / "gofmt", "#!/usr/bin/env bash\nexit 42\n")
    _write_executable(bin_dir / "npx", '#!/usr/bin/env bash\nprintf x > "$MORNLEA_CI_NEXT"\n')

    make_bin = shutil.which("make") or "make"
    res = subprocess.run(
        [make_bin, "-f", "Makefile", "ci-preflight"],
        cwd=repo_root,
        env=_ci_environment(str(bin_dir), [f"MORNLEA_CI_NEXT={next_cmd_marker}"]),
        capture_output=True,
        text=True,
    )
    assert res.returncode != 0
    assert not next_cmd_marker.exists(), "preflight ran next command after gofmt failure"
