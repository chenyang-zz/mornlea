import test from "node:test";
import assert from "node:assert/strict";

import {
  changedPaths,
  findBlockedCommand,
  openSpecRequirementReasons,
  runCommand,
  rustValidationRequired,
  stopFailures,
} from "./guard.mjs";

test("blocks destructive git commands", () => {
  assert.match(findBlockedCommand("git reset --hard HEAD"), /reset --hard/);
  assert.match(findBlockedCommand("git clean -fd"), /git clean/);
  assert.match(findBlockedCommand("git push origin main --force-with-lease"), /强制推送/);
});

test("blocks recursive deletion of broad targets", () => {
  assert.match(findBlockedCommand("sudo rm -rf /"), /递归强制删除/);
  assert.match(findBlockedCommand('rm -fr "$HOME"'), /递归强制删除/);
  assert.match(findBlockedCommand("rm -rf ."), /递归强制删除/);
});

test("allows scoped and read-only commands", () => {
  assert.equal(findBlockedCommand("rm -rf ./bin"), null);
  assert.equal(findBlockedCommand("git status --short"), null);
  assert.equal(findBlockedCommand("go test ./... -race"), null);
});

test("requires OpenSpec for contract and architecture changes", () => {
  assert.deepEqual(openSpecRequirementReasons(["packages/shared/network/protocol/packet.go"]), [
    "改动涉及协议、存档格式、性能基线或架构依赖门禁",
  ]);
  assert.deepEqual(openSpecRequirementReasons(["packages/audit/dependency_test.go"]), [
    "改动涉及协议、存档格式、性能基线或架构依赖门禁",
  ]);
});

test("requires OpenSpec for cross-component implementation changes", () => {
  const reasons = openSpecRequirementReasons([
    "packages/client/client/receiver.go",
    "packages/server/server/host.go",
  ]);
  assert.equal(reasons.length, 1);
  assert.match(reasons[0], /packages\/client/);
  assert.match(reasons[0], /packages\/server/);
});

test("does not require OpenSpec for one focused implementation component", () => {
  assert.deepEqual(
    openSpecRequirementReasons([
      "packages/client/client/camera.go",
      "packages/client/client/camera_test.go",
    ]),
    [],
  );
});

test("requires Rust validation for engine and native consumers", () => {
  assert.equal(rustValidationRequired(["packages/engine/crates/mornlea_engine/src/light.rs"]), true);
  assert.equal(rustValidationRequired(["packages/client/mesh/native.go"]), true);
  assert.equal(rustValidationRequired(["packages/shared/physics/collision.go"]), true);
  assert.equal(rustValidationRequired(["packages/shared/core/raycast.go"]), true);
  assert.equal(rustValidationRequired(["packages/server/server/session_ingress.go"]), false);
});

test("runs Rust validation before Go checks for Rust-required changes", () => {
  const calls = [];
  const run = (command, argumentsList) => {
    calls.push([command, argumentsList]);
    return { status: 0, stdout: "" };
  };

  assert.deepEqual(stopFailures(["packages/engine/crates/mornlea_engine/src/light.rs"], run, {}), []);
  assert.deepEqual(calls.slice(1, 4), [
    ["make", ["rust"]],
    ["make", ["rust-check"]],
    [
      "go",
      [
        "test",
        "./packages/shared/nativeabi",
        "./packages/shared/core",
        "./packages/shared/physics",
        "./packages/client/mesh",
        "./packages/client/client",
        "./packages/server/sim/...",
        "./packages/server/server",
        "./packages/client/cmd/mornlea",
        "./packages/server/cmd/mornlea-server",
        "-race",
      ],
    ],
  ]);
  assert.equal(calls.some(([command]) => command === "cargo"), false);
});

test("stop-gate test runs stay cache-friendly with generous timeouts", () => {
  const calls = [];
  const run = (command, argumentsList, timeout) => {
    calls.push({ command, argumentsList, timeout });
    return { status: 0, stdout: "" };
  };

  // guard.mjs 的 changedGoFiles 按真实存在性过滤，fixture 必须指向现存文件；
  // 服务端域已迁 packages/server/server。
  assert.deepEqual(stopFailures(["packages/server/server/host.go"], run, {}), []);
  const affected = calls.find(({ argumentsList }) =>
    argumentsList.includes("./packages/server/server"),
  );
  assert.deepEqual(affected.argumentsList, ["test", "-race", "./packages/server/server"]);
  assert.equal(affected.timeout, 600_000);
});

test("runs the fixed native downstream union for Rust-only bridge changes", () => {
  const calls = [];
  const run = (command, argumentsList) => {
    calls.push([command, argumentsList]);
    return { status: 0, stdout: "" };
  };

  assert.deepEqual(stopFailures(["packages/engine/crates/mornlea_engine/src/ffi.rs"], run, {}), []);
  assert.deepEqual(
    calls.find(([command, argumentsList]) => command === "go" && argumentsList.includes("./packages/shared/nativeabi")),
    [
      "go",
      [
        "test",
        "./packages/shared/nativeabi",
        "./packages/shared/core",
        "./packages/shared/physics",
        "./packages/client/mesh",
        "./packages/client/client",
        "./packages/server/sim/...",
        "./packages/server/server",
        "./packages/client/cmd/mornlea",
        "./packages/server/cmd/mornlea-server",
        "-race",
      ],
    ],
  );
});

test("unions the fixed native downstream packages with mixed bridge Go changes", () => {
  const calls = [];
  const run = (command, argumentsList) => {
    calls.push([command, argumentsList]);
    return { status: 0, stdout: "" };
  };

  assert.deepEqual(
    stopFailures(
      ["packages/engine/crates/mornlea_engine/src/ffi.rs", "packages/shared/nativeabi/native.go"],
      run,
      {},
    ),
    [],
  );
  assert.deepEqual(
    calls.find(([command, argumentsList]) => command === "go" && argumentsList.includes("./packages/shared/nativeabi")),
    [
      "go",
      [
        "test",
        "./packages/shared/nativeabi",
        "./packages/shared/core",
        "./packages/shared/physics",
        "./packages/client/mesh",
        "./packages/client/client",
        "./packages/server/sim/...",
        "./packages/server/server",
        "./packages/client/cmd/mornlea",
        "./packages/server/cmd/mornlea-server",
        "-race",
      ],
    ],
  );
});

test("runs the current identity guard for every identity-only root change", () => {
  for (const path of [
    "go.work",
    ".gitignore",
    "packages/engine/Cargo.toml",
    "scripts/agent-hooks/guard.mjs",
  ]) {
    const calls = [];
    const run = (command, argumentsList) => {
      calls.push([command, argumentsList]);
      return { status: 0, stdout: "" };
    };

    assert.deepEqual(stopFailures([path], run, {}), []);
    assert.equal(
      calls.filter(
        ([command, argumentsList]) =>
          command === "go" &&
          argumentsList.join(" ") ===
            "test ./packages/audit -run ^TestMornleaCurrentIdentity$ -count=1",
      ).length,
      1,
      `${path} did not route exactly once through TestMornleaCurrentIdentity`,
    );
  }
});

test("does not repeat the focused identity guard after the full Go archcheck", () => {
  const calls = [];
  const run = (command, argumentsList) => {
    calls.push([command, argumentsList]);
    return { status: 0, stdout: "" };
  };

  assert.deepEqual(stopFailures(["packages/shared/core/block.go", "go.work"], run, {}), []);
  assert.equal(
    calls.filter(
      ([command, argumentsList]) =>
        command === "go" &&
        argumentsList.join(" ") === "test ./packages/audit -count=1",
    ).length,
    1,
  );
  assert.equal(
    calls.filter(
      ([command, argumentsList]) =>
        command === "go" &&
        argumentsList.includes("^TestMornleaCurrentIdentity$"),
    ).length,
    0,
  );
});

test("passes login-shell Cargo through the full Stop route when PATH is restricted", () => {
  const calls = [];
  const environment = { SHELL: "/bin/zsh", PATH: "/usr/bin:/bin" };
  const spawn = (command, argumentsList) => {
    calls.push([command, argumentsList]);
    if (command === "/bin/zsh") {
      return { status: 0, stdout: "/toolchain/bin/cargo\n" };
    }
    if (
      command === "make" &&
      ["rust", "rust-check"].includes(argumentsList[0]) &&
      !argumentsList.includes("CARGO=/toolchain/bin/cargo")
    ) {
      return { status: 2, stderr: "cargo: command not found" };
    }
    return { status: 0, stdout: "" };
  };
  const run = (command, argumentsList, timeout) =>
    runCommand(command, argumentsList, timeout, spawn, environment);

  assert.deepEqual(
    stopFailures(["packages/engine/crates/mornlea_engine/src/light.rs"], run, environment),
    [],
  );
  assert.deepEqual(
    calls.filter(([command]) => command === "make"),
    [
      ["make", ["rust", "CARGO=/toolchain/bin/cargo"]],
      ["make", ["rust-check", "CARGO=/toolchain/bin/cargo"]],
    ],
  );
});

test("routes deleted Rust and native paths through Rust validation", () => {
  const collect = (command, argumentsList) => {
    assert.equal(command, "git");
    if (argumentsList[0] === "diff") {
      assert.ok(argumentsList.includes("--diff-filter=ACMRD"));
      return {
        status: 0,
        stdout: "packages/engine/crates/mornlea_engine/src/removed.rs\0packages/client/mesh/native_removed.go\0",
      };
    }
    return { status: 0, stdout: "" };
  };
  const paths = changedPaths(collect);
  const calls = [];
  const run = (command, argumentsList) => {
    calls.push([command, argumentsList]);
    return { status: 0, stdout: "" };
  };

  assert.deepEqual(paths, [
    "packages/engine/crates/mornlea_engine/src/removed.rs",
    "packages/client/mesh/native_removed.go",
  ]);
  assert.deepEqual(stopFailures(paths, run, {}), []);
  assert.deepEqual(
    calls.filter(([command]) => command === "make"),
    [
      ["make", ["rust"]],
      ["make", ["rust-check"]],
    ],
  );
  assert.equal(calls.some(([command]) => command === "gofmt"), false);
});

test("builds Rust before existing Go gates without unrelated cargo checks", () => {
  const calls = [];
  const run = (command, argumentsList) => {
    calls.push([command, argumentsList]);
    return { status: 0, stdout: "" };
  };

  assert.deepEqual(stopFailures(["packages/server/server/session_ingress.go"], run, {}), []);
  assert.deepEqual(calls.slice(1, 5), [
    ["gofmt", ["-l", "packages/server/server/session_ingress.go"]],
    ["make", ["rust"]],
    ["go", ["test", "./packages/audit", "-count=1"]],
    ["go", ["test", "-race", "./packages/server/server"]],
  ]);
  assert.equal(calls.some(([command]) => command === "cargo"), false);
});

test("finds tools through the login shell when the hook PATH is incomplete", () => {
  const calls = [];
  const spawn = (command, argumentsList) => {
    calls.push([command, argumentsList]);
    if (
      command === "gofmt" ||
      (command.endsWith("/gofmt") && command !== "/toolchain/bin/gofmt")
    ) {
      return { error: Object.assign(new Error("spawnSync gofmt ENOENT"), { code: "ENOENT" }) };
    }
    if (command === "/bin/zsh") {
      return { status: 0, stdout: "/toolchain/bin/gofmt\n" };
    }
    return { status: 0, stdout: "" };
  };

  const result = runCommand(
    "gofmt",
    ["-l", "packages/server/server/example.go"],
    30_000,
    spawn,
    { SHELL: "/bin/zsh", PATH: "/usr/bin:/bin" },
  );

  assert.equal(result.status, 0);
  assert.deepEqual(calls[0], ["gofmt", ["-l", "packages/server/server/example.go"]]);
  assert.deepEqual(calls.at(-2), ["/bin/zsh", ["-lc", "command -v gofmt"]]);
  assert.deepEqual(calls.at(-1), [
    "/toolchain/bin/gofmt",
    ["-l", "packages/server/server/example.go"],
  ]);
});

test("finds cargo through the login shell when PATH is incomplete", () => {
  const calls = [];
  const spawn = (command, argumentsList) => {
    calls.push([command, argumentsList]);
    if (
      command === "cargo" ||
      (command.endsWith("/cargo") && command !== "/toolchain/bin/cargo")
    ) {
      return { error: Object.assign(new Error("spawnSync cargo ENOENT"), { code: "ENOENT" }) };
    }
    if (command === "/bin/zsh") {
      return { status: 0, stdout: "/toolchain/bin/cargo\n" };
    }
    return { status: 0, stdout: "" };
  };

  const result = runCommand("cargo", ["fmt", "--check"], 30_000, spawn, {
    SHELL: "/bin/zsh",
    PATH: "/usr/bin:/bin",
  });

  assert.equal(result.status, 0);
  assert.deepEqual(calls[0], ["cargo", ["fmt", "--check"]]);
  assert.deepEqual(calls.at(-2), ["/bin/zsh", ["-lc", "command -v cargo"]]);
  assert.deepEqual(calls.at(-1), ["/toolchain/bin/cargo", ["fmt", "--check"]]);
});

test("finds Go tools through GOROOT when the hook PATH is incomplete", () => {
  const calls = [];
  const spawn = (command, argumentsList) => {
    calls.push([command, argumentsList]);
    if (calls.length === 1) {
      return { error: Object.assign(new Error("spawnSync go ENOENT"), { code: "ENOENT" }) };
    }
    return { status: 0, stdout: "" };
  };

  const result = runCommand("go", ["vet", "./..."], 30_000, spawn, {
    GOROOT: "/toolchain",
    PATH: "/usr/bin:/bin",
  });

  assert.equal(result.status, 0);
  assert.deepEqual(calls, [
    ["go", ["vet", "./..."]],
    ["/toolchain/bin/go", ["vet", "./..."]],
  ]);
});

test("finds tools in common installation directories when the hook PATH is incomplete", () => {
  const calls = [];
  const spawn = (command, argumentsList) => {
    calls.push([command, argumentsList]);
    if (calls.length === 1) {
      return { error: Object.assign(new Error("spawnSync openspec ENOENT"), { code: "ENOENT" }) };
    }
    return { status: 0, stdout: "" };
  };

  const result = runCommand("openspec", ["validate", "--all"], 30_000, spawn, {
    PATH: "/usr/bin:/bin",
  });

  assert.equal(result.status, 0);
  assert.deepEqual(calls, [
    ["openspec", ["validate", "--all"]],
    ["/opt/homebrew/bin/openspec", ["validate", "--all"]],
  ]);
});
