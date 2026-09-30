#!/usr/bin/env bash
set -euo pipefail

script_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd -P)"
repository_root="$(cd -- "${script_dir}/../.." && pwd -P)"
project_root="${repository_root}/apps/mornlea-godot"

# shellcheck source=python-version.env
source "${script_dir}/python-version.env"
# shellcheck source=py4godot/build-inputs.env
source "${script_dir}/py4godot/build-inputs.env"
# shellcheck source=version.env
source "${script_dir}/version.env"

mode=""
offline="false"
target="${PY4GODOT_TARGET}"
if [[ "$(uname -s)-$(uname -m)" == "Linux-x86_64" ]]; then
  target="x86_64-unknown-linux-gnu"
fi
cache_root="${MORNLEA_PY4GODOT_CACHE_DIR:-/tmp/mornlea-py4godot-cache}"
godot_cache_root="${MORNLEA_GODOT_CACHE_DIR:-/tmp/mornlea-godot-cache}"
qualification_root=""

fail() {
  printf 'Py4Godot qualification: %s\n' "$*" >&2
  exit 1
}

cleanup() {
  local exit_status="$?"
  if [[ -n "${qualification_root}" && -d "${qualification_root}" ]]; then
    case "${qualification_root}" in
      "${cache_root}/"*) rm -rf -- "${qualification_root}" ;;
      *) printf 'Py4Godot qualification: refusing to remove unexpected path: %s\n' "${qualification_root}" >&2 ;;
    esac
  fi
  return "${exit_status}"
}
trap cleanup EXIT

usage() {
  printf '%s\n' \
    'usage: scripts/godot/python-runtime-check.sh (--qualify|--exported|--coexistence|--bridge-contract|--host-open-session|--session-feature) [--offline] [--target darwin-arm64|x86_64-unknown-linux-gnu] [--cache-dir ABSOLUTE_PATH]'
}

while (($# > 0)); do
  case "$1" in
    --qualify)
      mode="qualify"
      shift
      ;;
    --exported)
      mode="exported"
      shift
      ;;
    --coexistence)
      mode="coexistence"
      shift
      ;;
    --bridge-contract)
      mode="bridge-contract"
      shift
      ;;
    --host-open-session)
      mode="host-open-session"
      shift
      ;;
    --session-feature)
      mode="session-feature"
      shift
      ;;
    --offline)
      offline="true"
      shift
      ;;
    --target)
      (($# >= 2)) || fail "--target requires a value"
      target="$2"
      shift 2
      ;;
    --cache-dir)
      (($# >= 2)) || fail "--cache-dir requires a value"
      cache_root="$2"
      shift 2
      ;;
    --help|-h)
      usage
      exit 0
      ;;
    *)
      fail "unsupported argument: $1"
      ;;
  esac
done

[[ "${mode}" == "qualify" || "${mode}" == "exported" || "${mode}" == "coexistence" || "${mode}" == "bridge-contract" || "${mode}" == "host-open-session" || "${mode}" == "session-feature" ]] || \
  fail "--qualify, --exported, --coexistence, --bridge-contract, --host-open-session, or --session-feature is required"
case "${target}:$(uname -s)-$(uname -m)" in
  darwin-arm64:Darwin-arm64)
    runtime_platform="darwin64"
    runtime_architecture="arm64"
    godot_target="darwin-universal"
    export_kind="macos-app"
    ;;
  x86_64-unknown-linux-gnu:Linux-x86_64)
    # The existing macOS evidence remains pinned independently.
    source "${script_dir}/py4godot/linux-build-inputs.env"
    runtime_platform="linux64"
    runtime_architecture="x86_64"
    godot_target="linux-x86_64"
    export_kind="linux-x86_64"
    ;;
  *) fail "unsupported Py4Godot desktop target: ${target} on $(uname -s)-$(uname -m)" ;;
esac
[[ "${cache_root}" == /* ]] || fail "cache directory must be an absolute path outside the repository"
[[ "${godot_cache_root}" == /* ]] || fail "Godot cache directory must be an absolute path outside the repository"

builder_args=(--verify --target "${target}" --cache-dir "${cache_root}")
if [[ "${offline}" == "true" ]]; then
  builder_args+=(--offline)
fi
"${script_dir}/build-python-runtime.sh" "${builder_args[@]}"
cache_root="$(cd -- "${cache_root}" && pwd -P)"

sha256_file() {
  local file_path="$1"
  if command -v shasum >/dev/null 2>&1; then
    shasum -a 256 -- "${file_path}" | awk '{print $1}'
    return
  fi
  sha256sum -- "${file_path}" | awk '{print $1}'
}

destination="${project_root}/addons/py4godot"
python_root="${destination}/cpython-${PY4GODOT_CPYTHON_VERSION}-${runtime_platform}/python"
bundled_python="${python_root}/bin/python3.14"
python_identity="$(${bundled_python} -I -s -E -c 'import platform, sys; print(platform.machine(), platform.python_version(), sys.flags.isolated)')"
[[ "${python_identity}" == "${runtime_architecture} ${PY4GODOT_CPYTHON_VERSION} 1" ]] || \
  fail "unexpected embedded Python identity: ${python_identity}"
if "${bundled_python}" -I -s -E -m pip --version >/dev/null 2>&1; then
  fail "runtime package installer must be unavailable"
fi

# Resolve the actual editor before entering the sandbox, whose PATH cannot run
# the Bash resolver wrapper. Explicit binary overrides retain their behavior.
if [[ -n "${MORNLEA_GODOT_BIN:-}" ]]; then
  godot_binary="${MORNLEA_GODOT_BIN}"
else
  godot_binary="$("${script_dir}/godot.sh" --print-path)" || \
    fail "Godot executable is unavailable through the repository resolver"
fi
[[ -x "${godot_binary}" ]] || fail "Godot executable is unavailable: ${godot_binary}"
actual_godot_version="$("${godot_binary}" --version)"
[[ "${actual_godot_version}" == 4.7.2.stable* ]] || \
  fail "Godot version mismatch: got ${actual_godot_version}, want 4.7.2-stable"

isolation_bin="${cache_root}/isolated-path"
forbidden_pythonpath="${cache_root}/forbidden-pythonpath"
invocation_marker="${cache_root}/external-runtime-invoked"
mkdir -p -- "${isolation_bin}" "${forbidden_pythonpath}"
touch "${forbidden_pythonpath}/mornlea_external_runtime_poison.py"
rm -f -- "${invocation_marker}"
for external_tool in python python3 python3.14 pip pip3 pip3.14 uv; do
  cat >"${isolation_bin}/${external_tool}" <<'TOOL'
#!/bin/sh
: > "${MORNLEA_EXTERNAL_RUNTIME_MARKER}"
printf 'external Python or package installer invocation is forbidden\n' >&2
exit 93
TOOL
  chmod +x "${isolation_bin}/${external_tool}"
done

if [[ "${runtime_platform}" == "linux64" ]]; then
  # deny-network.sh caches its compiled linux-network-deny.c helper outside the checkout.
  network_denial=("${script_dir}/deny-network.sh")
else
  network_denial=(sandbox-exec -p '(version 1) (allow default) (deny network*)')
fi

run_isolated_executable() {
  "${network_denial[@]}" \
    /usr/bin/env \
      PATH="${isolation_bin}" \
      PYTHONNOUSERSITE=1 \
      PYTHONPATH="${forbidden_pythonpath}" \
      PYTHONUSERBASE="${cache_root}/forbidden-user-base" \
      PIP_CONFIG_FILE=/dev/null \
      MORNLEA_EXTERNAL_RUNTIME_MARKER="${invocation_marker}" \
      MORNLEA_EXPECTED_PYTHON_ROOT="${expected_python_root:-${python_root}}" \
      "$@"
}

run_isolated() {
  run_isolated_executable "${godot_binary}" "$@"
}

# Linux denies the editor's debugger/listener sockets too. Classify only the
# exact paired Godot 4.7.2 diagnostics caused by that deliberate denial; all
# script, extension, export, and other engine errors remain qualification failures.
filter_editor_network_denials() {
  if [[ "${runtime_platform}" != "linux64" ]]; then
    cat
    return
  fi
  awk '
    $0 == "ERROR: Condition \"_sock == -1\" is true. Returning: FAILED" {
      first = $0
      getline second
      getline third
      getline fourth
      if (second == "   at: _inet_open (drivers/unix/net_socket_unix.cpp:288)" &&
          third == "ERROR: Condition \"err != OK\" is true. Returning: ERR_CANT_CREATE" &&
          fourth == "   at: listen (core/io/tcp_server.cpp:56)") {
        next
      }
      print first
      print second
      print third
      print fourth
      next
    }
    { print }
  '
}

if [[ "${mode}" == "bridge-contract" ]]; then
  # Materialize both native units fresh into the project distribution tree
  # (the shared build-python-runtime.sh invocation above already materialized
  # the pinned embedded Python runtime), then exercise the real producer
  # contract end to end under the same isolated, network-denied, poisoned
  # environment as the other modes. The check scene holds the native bridge
  # node and drives it from Python through typed Godot values only. The
  # online connect-success path belongs to the later playable smokes; with
  # network denied this mode exercises the honest offline paths: async begin,
  # bounded poll, cancel-and-join close, and the idempotent teardown.
  "${script_dir}/build-core.sh" --verify >/dev/null
  "${script_dir}/build-extension.sh" --verify >/dev/null
  contract_output="$(run_isolated --headless --path "${project_root}" --quit-after 120 \
    res://tests/scenes/bridge_contract_check.tscn 2>&1)" || {
    printf '%s\n' "${contract_output}" >&2
    fail "mornlea_godot bridge-contract probe failed"
  }
  [[ "${contract_output}" == *"Python bridge contract check passed."* ]] || \
    fail "Python bridge-contract success marker is missing"
  [[ "${contract_output}" != *"ERROR:"* && "${contract_output}" != *"SCRIPT ERROR:"* ]] || {
    printf '%s\n' "${contract_output}" >&2
    fail "bridge-contract probe reported an extension error"
  }
  [[ ! -e "${invocation_marker}" ]] || fail "an external Python or package installer was invoked"
  printf 'mornlea_godot bridge contract verified offline: producer identity, family table, session lifecycle, pulls, and typed values through the scene-held instance.\n'
  exit 0
fi

if [[ "${mode}" == "host-open-session" ]]; then
  # Discharge the bridge-lifecycle forward obligation from the session
  # lifecycle review: unlike the contract scene, this probe drives the real
  # feature host to create a producer session and connect toward a denied
  # address, then quits WITHOUT an explicit close. The bridge node's
  # drop-based release must land during scene teardown, before Rust
  # deinitialization, with a clean exit and the full lifecycle marker order.
  "${script_dir}/build-core.sh" --verify >/dev/null
  "${script_dir}/build-extension.sh" --verify >/dev/null
  host_output="$(run_isolated --headless --path "${project_root}" --quit-after 120 \
    res://tests/scenes/host_open_session_quit_check.tscn 2>&1)" || {
    printf '%s\n' "${host_output}" >&2
    fail "mornlea_godot host open-session probe failed"
  }
  [[ "${host_output}" == *"Python host open-session quit check passed with the session left open."* ]] || \
    fail "host open-session success marker is missing"
  [[ "${host_output}" != *"ERROR:"* && "${host_output}" != *"SCRIPT ERROR:"* ]] || {
    printf '%s\n' "${host_output}" >&2
    fail "host open-session probe reported an extension error"
  }
  main_loop_init="$(awk -v needle="[mornlea-lifecycle] rust-init=main-loop" 'index($0, needle) { print NR; exit }' <<<"${host_output}")"
  session_open="$(awk -v needle="Python host open-session quit check passed" 'index($0, needle) { print NR; exit }' <<<"${host_output}")"
  main_loop_deinit="$(awk -v needle="[mornlea-lifecycle] rust-deinit=main-loop" 'index($0, needle) { print NR; exit }' <<<"${host_output}")"
  scene_deinit="$(awk -v needle="[mornlea-lifecycle] rust-deinit=scene" 'index($0, needle) { print NR; exit }' <<<"${host_output}")"
  [[ -n "${main_loop_init}" && -n "${session_open}" && -n "${main_loop_deinit}" && -n "${scene_deinit}" ]] || {
    printf '%s\n' "${host_output}" >&2
    fail "host open-session probe is missing a lifecycle marker"
  }
  ((main_loop_init < session_open && session_open < main_loop_deinit && main_loop_deinit < scene_deinit)) || {
    printf '%s\n' "${host_output}" >&2
    fail "host open-session lifecycle markers are out of order"
  }
  [[ ! -e "${invocation_marker}" ]] || fail "an external Python or package installer was invoked"
  printf 'mornlea_godot host open-session teardown verified offline: the host-held session closes through bridge drop before Rust deinitialization.\n'
  exit 0
fi

if [[ "${mode}" == "session-feature" ]]; then
  # Drive the real production session feature end to end offline: the check
  # scene activates the production catalog through the real feature host,
  # asserts the initial not-ready display, requests one connection toward a
  # deliberately refused loopback address, observes the Connecting-to-
  # terminal transition, pins the stable dial error display, exercises the
  # latched/reset/clean-close states, and deactivates the feature again.
  "${script_dir}/build-core.sh" --verify >/dev/null
  "${script_dir}/build-extension.sh" --verify >/dev/null
  session_output="$(run_isolated --headless --path "${project_root}" --quit-after 300 \
    res://tests/scenes/session_feature_check.tscn 2>&1)" || {
    printf '%s\n' "${session_output}" >&2
    fail "mornlea_godot session-feature probe failed"
  }
  [[ "${session_output}" == *"Python session feature check passed."* ]] || \
    fail "session-feature success marker is missing"
  [[ "${session_output}" != *"ERROR:"* && "${session_output}" != *"SCRIPT ERROR:"* ]] || {
    printf '%s\n' "${session_output}" >&2
    fail "session-feature probe reported an extension error"
  }
  [[ ! -e "${invocation_marker}" ]] || fail "an external Python or package installer was invoked"
  printf 'mornlea_godot session feature verified offline: production catalog activation, phase display transitions, stable dial error, reset, and clean close.\n'
  exit 0
fi

if [[ "${mode}" == "coexistence" ]]; then
  "${script_dir}/build-extension.sh" --verify >/dev/null
  coexistence_output="$(run_isolated --headless --path "${project_root}" --quit-after 120 \
    res://tests/scenes/bridge_host_check.tscn 2>&1)" || {
    printf '%s\n' "${coexistence_output}" >&2
    fail "Py4Godot and mornlea_godot bridge-host coexistence probe failed"
  }
  [[ "${coexistence_output}" == *"Python bridge host check passed."* ]] || \
    fail "Python bridge-host coexistence marker is missing"
  [[ "${coexistence_output}" != *"ERROR:"* && "${coexistence_output}" != *"SCRIPT ERROR:"* ]] || {
    printf '%s\n' "${coexistence_output}" >&2
    fail "bridge-host coexistence probe reported an extension error"
  }
  [[ ! -e "${invocation_marker}" ]] || fail "an external Python or package installer was invoked"
  printf 'Py4Godot and mornlea_godot coexist through the isolated Python bridge host.\n'
  exit 0
fi

iterations="0"
if [[ "${mode}" != "exported" ]]; then
  editor_shutdown=(--quit)
  if [[ "${runtime_platform}" == "linux64" ]]; then
    # Godot 4.7.2 can execute deferred extension-documentation callbacks after
    # deleting their owner on an immediate cold editor quit. Let startup finish.
    editor_shutdown=(--quit-after 120)
  fi
  editor_output="$(run_isolated --headless --path "${project_root}" --editor "${editor_shutdown[@]}" 2>&1)" || {
    printf '%s\n' "${editor_output}" >&2
    fail "editor failed to load the Python extension"
  }
  checked_editor_output="$(filter_editor_network_denials <<<"${editor_output}")"
  [[ "${checked_editor_output}" != *"ERROR:"* && "${checked_editor_output}" != *"SCRIPT ERROR:"* ]] || {
    printf '%s\n' "${editor_output}" >&2
    fail "editor reported an extension error"
  }

  iterations="${MORNLEA_PY4GODOT_QUALIFY_ITERATIONS:-100}"
  [[ "${iterations}" =~ ^[1-9][0-9]*$ ]] || fail "qualification iteration count must be a positive integer"
  for ((iteration = 1; iteration <= iterations; iteration++)); do
    headless_output="$(run_isolated --headless --path "${project_root}" --quit-after 120 \
      res://tests/scenes/python_runtime_probe.tscn 2>&1)" || {
      printf '%s\n' "${headless_output}" >&2
      fail "headless Python probe failed on iteration ${iteration}"
    }
    [[ "${headless_output}" == *"Py4Godot runtime check passed."* ]] || \
      fail "headless Python success marker is missing on iteration ${iteration}"
    [[ "${headless_output}" != *"ERROR:"* && "${headless_output}" != *"SCRIPT ERROR:"* ]] || {
      printf '%s\n' "${headless_output}" >&2
      fail "headless Python probe reported an error on iteration ${iteration}"
    }
  done

  "${script_dir}/build-extension.sh" --verify >/dev/null
  coexistence_output="$(run_isolated --headless --path "${project_root}" \
    --quit-after 120 res://tests/scenes/bridge_host_check.tscn 2>&1)" || {
    printf '%s\n' "${coexistence_output}" >&2
    fail "Py4Godot and mornlea_godot coexistence probe failed"
  }
  [[ "${coexistence_output}" == *"Python bridge host check passed."* ]] || \
    fail "mornlea_godot coexistence marker is missing"
  [[ "${coexistence_output}" != *"ERROR:"* && "${coexistence_output}" != *"SCRIPT ERROR:"* ]] || {
    printf '%s\n' "${coexistence_output}" >&2
    fail "coexistence probe reported an extension error"
  }
fi

template_archive="${godot_cache_root}/${GODOT_VERSION}/${godot_target}/Godot_v${GODOT_VERSION}_export_templates.tpz"
if [[ ! -f "${template_archive}" ]]; then
  [[ "${offline}" == "false" ]] || fail "offline export-template cache miss: ${template_archive}"
  "${script_dir}/fetch.sh" --target "${godot_target}" --cache-dir "${godot_cache_root}"
fi
[[ "$(sha256_file "${template_archive}")" == "${GODOT_EXPORT_TEMPLATES_SHA256}" ]] || \
  fail "Godot export-template checksum mismatch"

CARGO_NET_OFFLINE=true "${script_dir}/build-extension.sh" --profile release --verify >/dev/null
qualification_root="$(mktemp -d "${cache_root}/qualification.XXXXXX")"
qualification_project="${qualification_root}/project"
qualification_home="${qualification_root}/home"
qualification_output="${qualification_root}/output"
if [[ "${runtime_platform}" == "linux64" ]]; then
  template_dir="${qualification_home}/.local/share/godot/export_templates/4.7.2.stable"
  mkdir -p -- "${template_dir}" "${qualification_output}"
  unzip -oqj "${template_archive}" templates/linux_debug.x86_64 templates/linux_release.x86_64 -d "${template_dir}"
  cp -a -- "${project_root}" "${qualification_project}"
else
  template_dir="${qualification_home}/Library/Application Support/Godot/export_templates/4.7.2.stable"
  mkdir -p -- "${template_dir}" "${qualification_output}"
  unzip -oqj "${template_archive}" templates/macos.zip -d "${template_dir}"
  ditto --norsrc --noextattr "${project_root}" "${qualification_project}"
fi
rm -rf -- "${qualification_project}/.godot" "${qualification_project}/tests/scripts/__pycache__"
if [[ "${runtime_platform}" == "linux64" ]]; then
  cp -- "${script_dir}/fixtures/python-runtime-linux-export-presets.cfg" "${qualification_project}/export_presets.cfg"
  sed -i \
    's#run/main_scene="res://app/bootstrap/bootstrap.tscn"#run/main_scene="res://tests/scenes/python_runtime_probe.tscn"#' \
    "${qualification_project}/project.godot"
  export_preset="Linux Python Qualification"
  export_path="${qualification_output}/MornleaPythonQualification.x86_64"
else
  cp -- "${script_dir}/fixtures/python-runtime-export-presets.cfg" "${qualification_project}/export_presets.cfg"
  sed -i '' \
    's#run/main_scene="res://app/bootstrap/bootstrap.tscn"#run/main_scene="res://tests/scenes/python_runtime_probe.tscn"#' \
    "${qualification_project}/project.godot"
  export_preset="macOS Python Qualification"
  exported_app="${qualification_output}/MornleaPythonQualification.app"
  export_path="${exported_app}"
fi

export_output="$("${network_denial[@]}" \
  /usr/bin/env HOME="${qualification_home}" XDG_DATA_HOME="${qualification_home}/.local/share" \
  "${godot_binary}" --headless --path "${qualification_project}" \
  --export-release "${export_preset}" "${export_path}" 2>&1)" || {
  printf '%s\n' "${export_output}" >&2
  fail "${export_kind} qualification export failed"
}
checked_export_output="$(filter_editor_network_denials <<<"${export_output}")"
[[ "${checked_export_output}" != *"ERROR:"* && "${checked_export_output}" != *"SCRIPT ERROR:"* ]] || {
  printf '%s\n' "${export_output}" >&2
  fail "${export_kind} qualification export reported an error"
}

if [[ "${runtime_platform}" == "linux64" ]]; then
  exported_resources="${qualification_output}"
  exported_python_root="${exported_resources}/addons/py4godot"
  exported_executable="${export_path}"
else
  exported_resources="${exported_app}/Contents/Resources"
  exported_python_root="${exported_app}/Contents/Resources/addons/py4godot"
  exported_executable="${exported_app}/Contents/MacOS/Mornlea Godot Pilot"
fi
mkdir -p -- "${exported_python_root}" "${exported_resources}/tests/scripts"
runtime_directory="cpython-${PY4GODOT_CPYTHON_VERSION}-${runtime_platform}"
if [[ "${runtime_platform}" == "linux64" ]]; then
  cp -a -- "${qualification_project}/addons/py4godot/${runtime_directory}" "${exported_python_root}/${runtime_directory}"
  # Force Godot to load the descriptor's intact filesystem path. The automatic
  # basename copy would make dladdr resolve the executable directory instead.
  rm -f -- "${qualification_output}/pythonscript.so"
  exported_bridge_root="${exported_resources}/addons/mornlea_bridge/bin/linux-x86_64"
  mkdir -p -- "${exported_bridge_root}"
  cp -a -- "${qualification_project}/addons/mornlea_bridge/bin/linux-x86_64/release" "${exported_bridge_root}/release"
  rm -f -- "${qualification_output}/libmornlea_godot.so"
else
  ditto --norsrc --noextattr \
    "${qualification_project}/addons/py4godot/${runtime_directory}" "${exported_python_root}/${runtime_directory}"
fi
cp -- "${qualification_project}/tests/scripts/python_runtime_probe.py" \
  "${exported_resources}/tests/scripts/python_runtime_probe.py"
expected_python_root="${exported_python_root}/${runtime_directory}/python"
[[ -x "${exported_executable}" ]] || fail "exported ${export_kind} executable is missing"
exported_output="$(run_isolated_executable "${exported_executable}" --headless --quit-after 120 2>&1)" || {
  printf '%s\n' "${exported_output}" >&2
  fail "exported ${export_kind} Python probe failed"
}
[[ "${exported_output}" == *"Py4Godot runtime check passed."* ]] || \
  fail "exported ${export_kind} Python success marker is missing"
[[ "${exported_output}" == *"Initialize godot-rust"* ]] || \
  fail "exported ${export_kind} mornlea_godot coexistence marker is missing"
[[ "${exported_output}" != *"ERROR:"* && "${exported_output}" != *"SCRIPT ERROR:"* ]] || {
  printf '%s\n' "${exported_output}" >&2
  fail "exported ${export_kind} Python probe reported an error"
}
[[ ! -e "${invocation_marker}" ]] || fail "an external Python or package installer was invoked"

printf 'Py4Godot %s %s offline: upstream=%s source=%s cpython=%s target=%s cycles=%s exported=%s artifact_sha256=%s\n' \
  "${PY4GODOT_HARDENED_VERSION}" "${mode}" "${PY4GODOT_VERSION}" "${PY4GODOT_SOURCE_REVISION}" \
  "${PY4GODOT_CPYTHON_VERSION}" "${target}" "${iterations}" "${export_kind}" "${PY4GODOT_HARDENED_ARTIFACT_SHA256}"
