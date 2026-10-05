#!/usr/bin/env bash
set -euo pipefail

# Fail closed before repository discovery so no validation mode does partial work.
if ! command -v rg >/dev/null 2>&1; then
  printf 'missing required executable: rg\n' >&2
  exit 1
fi

script_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd -P)"
repository_root="$(cd -- "${script_dir}/../.." && pwd -P)"
project_root="${MORNLEA_GODOT_PROJECT_ROOT:-${repository_root}/apps/mornlea-godot}"
failures=0
rg_findings=""

usage() {
  printf 'usage: %s [--script-ownership] [--desktop-only-fixtures] [--python-isolation-fixtures]\n' "${0##*/}" >&2
}

reject() {
  printf '%s\n' "$*" >&2
  failures=1
}

# Preserve ripgrep's distinct no-match status without masking scan failures.
scan_rg() {
  local status=0
  rg_findings="$(rg "$@")" || status=$?
  if ((status > 1)); then
    reject "ripgrep scan failed (exit $status)"
  fi
  return "$status"
}

is_generated_or_development_path() {
  case "$1" in
    .godot/*|.venv/*|.ruff_cache/*|addons/py4godot/*)
      return 0
      ;;
  esac
  return 1
}

validate_script_ownership() {
  local source_path relative_path
  local allowed_bootstrap="app/bootstrap/bootstrap.gd"
  local allowed_setup="app/bootstrap/setup_required.gd"

  # Tests may use GDScript to probe the project before Python is available. The
  # production allowlist remains limited to the native bootstrap diagnostics.
  while IFS= read -r -d '' source_path; do
    relative_path="${source_path#${project_root}/}"
    case "${relative_path}" in
      tests/*|addons/py4godot/*|.godot/*|.venv/*|.ruff_cache/*)
        continue
        ;;
      "${allowed_bootstrap}"|"${allowed_setup}")
        continue
        ;;
    esac
    reject "production GDScript is outside the bootstrap allowlist: ${relative_path}"
  done < <(find "${project_root}" -type f -name '*.gd' -print0)
}

validate_project_root() {
  local count source_path relative_path resolved
  [[ -f "${project_root}/project.godot" ]] || reject "Godot project descriptor is missing from the project root"
  count="$(find "${project_root}" \
    \( -path '*/.godot' -o -path '*/.venv' -o -path '*/.ruff_cache' \) -prune -o \
    -type f -name project.godot -print | wc -l | tr -d '[:space:]')"
  [[ "${count}" == "1" ]] || reject "Godot project root must contain exactly one project.godot; found ${count}"

  # Project-local links are portable; broken links and links leaving the root are not.
  while IFS= read -r -d '' source_path; do
    relative_path="${source_path#${project_root}/}"
    if is_generated_or_development_path "${relative_path}"; then
      continue
    fi
    if ! resolved="$(realpath "${source_path}" 2>/dev/null)"; then
      reject "symbolic link is broken: ${relative_path}"
      continue
    fi
    case "${resolved}" in
      "${project_root}"|"${project_root}/"*) ;;
      *) reject "symbolic link escapes the project root: ${relative_path} -> ${resolved}" ;;
    esac
  done < <(find "${project_root}" -type l -print0)
}

collect_project_text_files() {
  local source_path relative_path
  while IFS= read -r -d '' source_path; do
    relative_path="${source_path#${project_root}/}"
    if is_generated_or_development_path "${relative_path}"; then
      continue
    fi
    case "${relative_path}" in
      tests/*)
        continue
        ;;
    esac
    case "${source_path}" in
      *.cfg|*.gd|*.gdextension|*.godot|*.import|*.json|*.py|*.pyi|*.tres|*.tscn)
        printf '%s\0' "${source_path}"
        ;;
    esac
  done < <(find "${project_root}" -type f -print0)
}

validate_resource_closure() {
  local source_path relative_path autoloads descriptor line library_path
  local -a text_files=()
  while IFS= read -r -d '' source_path; do
    text_files+=("${source_path}")
  done < <(collect_project_text_files)

  if ((${#text_files[@]} > 0)); then
    if scan_rg -n --fixed-strings 'res://../' "${text_files[@]}"; then
      printf '%s\n' "${rg_findings}" >&2
      reject "resource path escapes the project root"
    fi
    if scan_rg -n --pcre2 '(?:^|[=\"'"'"'( ])(?:/(?:[A-Za-z0-9._-]+/)+[A-Za-z0-9._-]+|[A-Za-z]:[\\/])' "${text_files[@]}"; then
      printf '%s\n' "${rg_findings}" >&2
      reject "absolute development-machine path is forbidden"
    fi
    if scan_rg -n --pcre2 '\b(?:PYTHONPATH|PYTHONHOME|LD_LIBRARY_PATH|DYLD_LIBRARY_PATH)\b' "${text_files[@]}"; then
      printf '%s\n' "${rg_findings}" >&2
      reject "system Python or dynamic-library search is forbidden"
    fi
  fi

  autoloads="$(awk '
    /^\[autoload\][[:space:]]*$/ { active=1; next }
    /^\[/ { active=0 }
    active && /^[[:space:]]*[^;#[:space:]][^=]*=/ { print }
  ' "${project_root}/project.godot")"
  if [[ -n "${autoloads}" ]]; then
    printf '%s\n' "${autoloads}" >&2
    reject "unregistered autoload is forbidden; use the explicit Python feature catalog"
  fi

  while IFS= read -r -d '' descriptor; do
    relative_path="${descriptor#${project_root}/}"
    case "${relative_path}" in
      addons/py4godot/*)
        continue
        ;;
    esac
    while IFS= read -r line; do
      [[ "${line}" == *"="* ]] || continue
      library_path="${line#*=}"
      library_path="${library_path#*\"}"
      library_path="${library_path%%\"*}"
      case "${library_path}" in
        res://*) ;;
        /*|[A-Za-z]:[\\/]*|*../*) reject "GDExtension library is not project-local: ${relative_path}: ${library_path}" ;;
      esac
    done < <(awk '/^\[libraries\]/{active=1; next} /^\[/{active=0} active && /=/{print}' "${descriptor}")
  done < <(find "${project_root}" -type f -name '*.gdextension' -print0)
}

validate_uid_policy() {
  local ignore_file="${project_root}/.gitignore"
  local source_path relative_path sidecar
  [[ -f "${ignore_file}" ]] || {
    reject "Godot project .gitignore is missing"
    return
  }
  grep -Eq '^[[:space:]]*/?\.godot/[[:space:]]*$' "${ignore_file}" || reject ".godot/ must be ignored"
  if grep -En '^[[:space:]]*[^#].*\.uid' "${ignore_file}" >/dev/null; then
    reject "UID sidecars must remain tracked"
  fi
  if grep -En '^[[:space:]]*[^#[:space:]].*\.import([[:space:]]|$)' "${ignore_file}" >/dev/null; then
    reject "Godot import sidecars must remain tracked"
  fi

  while IFS= read -r -d '' source_path; do
    relative_path="${source_path#${project_root}/}"
    case "${relative_path}" in
      tests/*|addons/py4godot/*|.godot/*|.venv/*|.ruff_cache/*)
        continue
        ;;
    esac
    sidecar="${source_path}.uid"
    [[ -f "${sidecar}" ]] || reject "Godot identity sidecar is missing: ${relative_path}.uid"
  done < <(find "${project_root}" -type f \( -name '*.gd' -o -name '*.gdshader' -o -name '*.gdextension' \) -print0)
}

validate_comment_language() {
  local source_path relative_path
  local -a architecture_sources=()
  while IFS= read -r -d '' source_path; do
    relative_path="${source_path#${project_root}/}"
    case "${relative_path}" in
      tests/*|addons/py4godot/*|typing/*|.godot/*|.venv/*|.ruff_cache/*)
        continue
        ;;
    esac
    architecture_sources+=("${source_path}")
  done < <(find "${project_root}" -type f \( -name '*.gd' -o -name '*.py' -o -name '*.pyi' \) -print0)
  if ((${#architecture_sources[@]} > 0)) && \
    scan_rg -n --pcre2 '#[^\r\n]*\p{Han}' "${architecture_sources[@]}"; then
    printf '%s\n' "${rg_findings}" >&2
    reject "non-English source comment is forbidden in Godot architecture code"
  fi
}

validate_python_boundary() {
  local source_path relative_path
  local -a production_python=()
  while IFS= read -r -d '' source_path; do
    relative_path="${source_path#${project_root}/}"
    case "${relative_path}" in
      tests/*|addons/py4godot/*|typing/*|.godot/*|.venv/*|.ruff_cache/*)
        continue
        ;;
    esac
    production_python+=("${source_path}")
  done < <(find "${project_root}" -type f \( -name '*.py' -o -name '*.pyi' \) -print0)
  ((${#production_python[@]} > 0)) || return

  if scan_rg -n --pcre2 '^\s*(?:from|import)\s+(?:aiohttp|cffi|ctypes|ensurepip|ftplib|http|mornlea_client_core|mornlea_engine|packages\.agent|pip|requests|sitecustomize|socket|subprocess|urllib|usercustomize|uv|venv|websockets)(?:\b|\.)' "${production_python[@]}"; then
    printf '%s\n' "${rg_findings}" >&2
    reject "forbidden Python runtime dependency"
  fi
  if scan_rg -n --pcre2 "\\b(?:sys\\.path\\.(?:append|extend|insert)|site\\.(?:addsitedir|getusersitepackages)|site\\.(?:USER_SITE|ENABLE_USER_SITE)|os\\.(?:popen|system|exec\\w*|spawn\\w*)|(?:pip|ensurepip|venv)\\.|importlib\\.import_module\\s*\\(\\s*['\\\"](?:pip|ensurepip|uv)['\\\"]|(?:urlopen|urlretrieve)\\s*\\(|requests\\.(?:get|post|put|patch)\\s*\\()" "${production_python[@]}"; then
    printf '%s\n' "${rg_findings}" >&2
    reject "forbidden Python runtime search, installer, process, or download call"
  fi
}

validate_desktop_only() {
  local source_path relative_path input_map

  # Source closure rejects unsupported device and lifecycle declarations before
  # any build or export tool can select platform artifacts.
  if [[ -d "${project_root}/platform" ]]; then
    while IFS= read -r -d '' source_path; do
      relative_path="${source_path#${project_root}/}"
      [[ "${relative_path}" == "platform/desktop" ]] || \
        reject "unsupported mobile lifecycle or platform adapter: ${relative_path}"
    done < <(find "${project_root}/platform" -mindepth 1 -maxdepth 1 -type d -print0)
  fi

  input_map="$(awk '
    /^\[input\][[:space:]]*$/ { active=1; next }
    /^\[/ { active=0 }
    active { print }
  ' "${project_root}/project.godot")"
  if scan_rg -n -i '(?:touch(?:screen)?|screen_(?:touch|drag)|accelerometer|gyroscope|magnetometer|gravity_sensor|virtual_joystick)' <<<"${input_map}"; then
    printf '%s\n' "${rg_findings}" >&2
    reject "unsupported touch or mobile-sensor input"
  fi

  while IFS= read -r -d '' source_path; do
    relative_path="${source_path#${project_root}/}"
    case "${relative_path}" in
      tests/*|addons/py4godot/*|.godot/*|.venv/*|.ruff_cache/*)
        continue
        ;;
    esac
    if [[ "${relative_path}" =~ (^|/)(mobile|android|ios|web|console)[_/-]?(lifecycle|pause|resume)?\.(gd|py)$ ]]; then
      reject "unsupported mobile lifecycle or platform adapter: ${relative_path}"
    fi
  done < <(find "${project_root}" -type f \( -name '*.gd' -o -name '*.py' \) -print0)
}

validate_gdscript_runtime_boundary() {
  local source_path relative_path
  local -a production_gdscript=()
  while IFS= read -r -d '' source_path; do
    relative_path="${source_path#${project_root}/}"
    case "${relative_path}" in
      tests/*|addons/py4godot/*|.godot/*|.venv/*|.ruff_cache/*)
        continue
        ;;
    esac
    production_gdscript+=("${source_path}")
  done < <(find "${project_root}" -type f -name '*.gd' -print0)
  ((${#production_gdscript[@]} > 0)) || return

  if scan_rg -n --pcre2 '\b(?:HTTPClient|HTTPRequest|WebSocketPeer|StreamPeerTCP)\b|\bOS\.(?:execute|create_process)\s*\(' "${production_gdscript[@]}"; then
    printf '%s\n' "${rg_findings}" >&2
    reject "forbidden Godot runtime network, download, or process call"
  fi
}

validate_export_preset() {
  local preset_name="$1"
  local platform="$2"
  local exclusions="$3"
  local include_filter="$4"
  local required
  local -a common_exclusions=(
    'tests/**'
    'typing/**'
    '.venv/**'
    '.ruff_cache/**'
    'pyproject.toml'
    'uv.lock'
    'README*'
    'assets/provenance/**'
    'assets/generated/**/*.provenance.json'
    'assets/generated/**/PROVENANCE.json'
  )
  [[ -n "${platform}" ]] || {
    reject "export preset is missing a platform: ${preset_name}"
    return
  }
  case "${platform}" in
    macOS)
      common_exclusions+=(
        'addons/mornlea_bridge/bin/linux-x86_64/**'
        'addons/mornlea_bridge/bin/windows-x86_64/**'
        'addons/py4godot/cpython-*-linux*/**'
        'addons/py4godot/cpython-*-windows*/**'
      )
      ;;
    'Windows Desktop')
      common_exclusions+=(
        'addons/mornlea_bridge/bin/macos-universal/**'
        'addons/mornlea_bridge/bin/linux-x86_64/**'
        'addons/py4godot/cpython-*-darwin*/**'
        'addons/py4godot/cpython-*-linux*/**'
      )
      ;;
    'Linux/X11')
      common_exclusions+=(
        'addons/mornlea_bridge/bin/macos-universal/**'
        'addons/mornlea_bridge/bin/windows-x86_64/**'
        'addons/py4godot/cpython-*-darwin*/**'
        'addons/py4godot/cpython-*-windows*/**'
      )
      ;;
    *)
      reject "unsupported export platform: ${platform}"
      return
      ;;
  esac
  for required in "${common_exclusions[@]}"; do
    [[ ",${exclusions}," == *",${required},"* ]] || reject "export exclusion is missing: ${required} (${preset_name})"
  done
  # Bootstrap preloads its diagnostic scene even when every native dependency
  # is available. Excluding that scene or its script prevents release startup.
  local -a exclusion_patterns=()
  local dependency pattern
  IFS=',' read -r -a exclusion_patterns <<<"${exclusions}"
  for dependency in app/bootstrap/bootstrap.gd app/bootstrap/setup_required.tscn app/bootstrap/setup_required.gd; do
    for pattern in "${exclusion_patterns[@]}"; do
      if [[ "${dependency}" == ${pattern} ]]; then
        reject "export excludes required Bootstrap dependency: ${dependency} (${preset_name})"
      fi
    done
  done
  if [[ "${include_filter}" =~ (tests/|typing/|assets/provenance/) ]]; then
    reject "export include filter contains development or provenance content (${preset_name})"
  fi
}

validate_export_closure() {
  local presets="${project_root}/export_presets.cfg"
  local line preset_name="" platform="" exclusions="" include_filter="" custom_template="" lower_template=""
  local preset_count=0 relative_path catalog capability_registry
  [[ -f "${presets}" ]] || {
    reject "export_presets.cfg is missing"
    return
  }

  while IFS= read -r line || [[ -n "${line}" ]]; do
    if [[ "${line}" =~ ^\[preset\.[0-9]+\]$ ]]; then
      if ((preset_count > 0)); then
        validate_export_preset "${preset_name}" "${platform}" "${exclusions}" "${include_filter}"
      fi
      preset_count=$((preset_count + 1))
      preset_name="${line}"
      platform=""
      exclusions=""
      include_filter=""
      continue
    fi
    case "${line}" in
      name=\"*\") preset_name="${line#name=\"}"; preset_name="${preset_name%\"}" ;;
      platform=\"*\") platform="${line#platform=\"}"; platform="${platform%\"}" ;;
      exclude_filter=\"*\") exclusions="${line#exclude_filter=\"}"; exclusions="${exclusions%\"}" ;;
      include_filter=\"*\") include_filter="${line#include_filter=\"}"; include_filter="${include_filter%\"}" ;;
      custom_template/*=\"*\")
        custom_template="${line#*=\"}"
        custom_template="${custom_template%\"}"
        lower_template="$(printf '%s' "${custom_template}" | tr '[:upper:]' '[:lower:]')"
        if [[ "${lower_template}" =~ (android|ios|web|wasm|console|xbox|playstation|switch) ]]; then
          reject "unsupported export template dependency: ${custom_template}"
        fi
        ;;
    esac
  done < "${presets}"
  ((preset_count > 0)) || reject "export_presets.cfg contains no export preset"
  if ((preset_count > 0)); then
    validate_export_preset "${preset_name}" "${platform}" "${exclusions}" "${include_filter}"
  fi

  if scan_rg -n -i '^\s*(?:android|ios|web|wasm|console|xbox|playstation|switch)\.' "${project_root}/addons/mornlea_bridge" -g '*.gdextension'; then
    printf '%s\n' "${rg_findings}" >&2
    reject "unsupported platform selector in GDExtension descriptor"
  fi

  catalog="${project_root}/config/feature_catalog.tres"
  [[ -f "${catalog}" ]] || return
  capability_registry="${project_root}/catalog/capability_registry.tres"
  while IFS= read -r -d '' manifest; do
    relative_path="${manifest#${project_root}/}"
    if grep -Fq "res://${relative_path}" "${catalog}"; then
      continue
    fi
    # Reserved coarse capabilities use their own registry and remain disabled;
    # they are not product features and must not be forced into the activation catalog.
    if [[ -f "${capability_registry}" ]] && grep -Fq "res://${relative_path}" "${capability_registry}"; then
      continue
    fi
    reject "feature is not selected by the export catalog: ${relative_path}"
  done < <(find "${project_root}/features" "${project_root}/platform/desktop" -type f -name feature.tres -print0 2>/dev/null || true)
}

run_full_validation() {
  failures=0
  validate_project_root
  validate_resource_closure
  validate_uid_policy
  validate_script_ownership
  validate_comment_language
  validate_python_boundary
  validate_gdscript_runtime_boundary
  validate_desktop_only
  validate_export_closure
  ((failures == 0))
}

new_fixture_root() {
  local fixture
  # Mutation probes synthesize only the valid source closure, so the gate stays
  # independent from generated native and embedded-Python artifacts.
  fixture="$(mktemp -d "${TMPDIR:-/tmp}/mornlea-godot-validate.XXXXXX")"
  mkdir -p "${fixture}/app/bootstrap" "${fixture}/app/host" "${fixture}/addons/mornlea_bridge"
  printf '%s\n' \
    '.godot/' \
    '.venv/' \
    '.ruff_cache/' \
    'addons/mornlea_bridge/bin/' \
    'addons/py4godot/' \
    '__pycache__/' \
    '*.py[cod]' > "${fixture}/.gitignore"
  printf '%s\n' \
    'config_version=5' \
    '' \
    '[application]' \
    'run/main_scene="res://app/bootstrap/bootstrap.tscn"' > "${fixture}/project.godot"
  printf '%s\n' \
    '[preset.0]' \
    'name="Mornlea macOS"' \
    'platform="macOS"' \
    'runnable=true' \
    'export_filter="all_resources"' \
    'include_filter="app/**/*.py,features/**/*.py,platform/desktop/**/*.py,addons/mornlea_bridge/*.py,assets/generated/**"' \
    'exclude_filter="tests/**,typing/**,.venv/**,.ruff_cache/**,pyproject.toml,uv.lock,README*,assets/provenance/**,assets/generated/**/*.provenance.json,assets/generated/**/PROVENANCE.json,addons/mornlea_bridge/bin/linux-x86_64/**,addons/mornlea_bridge/bin/windows-x86_64/**,addons/py4godot/cpython-*-linux*/**,addons/py4godot/cpython-*-windows*/**"' \
    'export_path=""' \
    '' \
    '[preset.0.options]' > "${fixture}/export_presets.cfg"
  printf '%s\n' 'extends Node' '# Bootstrap owns dependency diagnostics before Python can load.' > "${fixture}/app/bootstrap/bootstrap.gd"
  printf '%s\n' 'uid://bootstrapfixture' > "${fixture}/app/bootstrap/bootstrap.gd.uid"
  printf '%s\n' 'extends Control' '# Setup diagnostics never own gameplay behavior.' > "${fixture}/app/bootstrap/setup_required.gd"
  printf '%s\n' 'uid://setupfixture' > "${fixture}/app/bootstrap/setup_required.gd.uid"
  printf '%s\n' \
    '[configuration]' \
    'entry_symbol = "gdext_rust_init"' \
    '' \
    '[libraries]' \
    'macos.debug.arm64 = "res://addons/mornlea_bridge/bin/macos-universal/debug/libmornlea_godot.dylib"' > "${fixture}/addons/mornlea_bridge/mornlea_bridge.gdextension"
  printf '%s\n' 'uid://bridgefixture' > "${fixture}/addons/mornlea_bridge/mornlea_bridge.gdextension.uid"
  printf '%s\n' 'from __future__ import annotations' '# Python owns feature lifecycle after Bootstrap hands off.' > "${fixture}/app/host/app_root.py"
  printf '%s\n' "${fixture}"
}

expect_fixture_rejection() {
  local name="$1"
  local expected="$2"
  local mutation="$3"
  local fixture output status
  fixture="$(new_fixture_root)"
  case "${mutation}" in
    export:*)
      printf '\n[preset.1]\nname="fixture"\nplatform="%s"\n' "${mutation#export:}" >> "${fixture}/export_presets.cfg"
      ;;
    action:*)
      printf '\n[input]\n%s={\n}\n' "${mutation#action:}" >> "${fixture}/project.godot"
      ;;
    lifecycle)
      mkdir -p "${fixture}/platform/mobile"
      printf 'from __future__ import annotations\n' > "${fixture}/platform/mobile/lifecycle.py"
      ;;
    selector:*)
      printf '\n%s = "res://addons/mornlea_bridge/bin/unsupported/library"\n' "${mutation#selector:}" >> "${fixture}/addons/mornlea_bridge/mornlea_bridge.gdextension"
      ;;
    python:system-path)
      mkdir -p "${fixture}/features/world"
      printf "from __future__ import annotations\nimport sys\nsys.path.append('/usr/local/lib/python3.14')\n" > "${fixture}/features/world/fixture.py"
      ;;
    python:user-site)
      mkdir -p "${fixture}/features/world"
      printf 'from __future__ import annotations\nimport site\nsite.getusersitepackages()\n' > "${fixture}/features/world/fixture.py"
      ;;
    python:installer)
      mkdir -p "${fixture}/features/world"
      printf "from __future__ import annotations\nimport importlib\nimportlib.import_module('pip').main(['install', 'fixture'])\n" > "${fixture}/features/world/fixture.py"
      ;;
    *)
      rm -rf "${fixture}"
      printf 'unknown validation fixture mutation: %s\n' "${mutation}" >&2
      return 2
      ;;
  esac
  project_root="${fixture}"
  set +e
  output="$(run_full_validation 2>&1)"
  status=$?
  set -e
  rm -rf "${fixture}"
  if ((status == 0)) || [[ "${output}" != *"${expected}"* ]]; then
    printf 'fixture %s was not rejected as expected:\n%s\n' "${name}" "${output}" >&2
    return 1
  fi
  printf 'fixture rejected: %s\n' "${name}"
}

validate_desktop_only_fixtures() {
  local platform selector action
  for platform in Android iOS Web 'Nintendo Switch'; do
    expect_fixture_rejection "${platform} export preset" "unsupported export platform" "export:${platform}"
  done
  for selector in android.debug.arm64 ios.debug.arm64 web.debug.wasm32 switch.debug.arm64; do
    expect_fixture_rejection "${selector} selector" "unsupported platform selector" "selector:${selector}"
  done
  for action in touch_primary accelerometer_x; do
    expect_fixture_rejection "${action} input" "unsupported touch or mobile-sensor input" "action:${action}"
  done
  expect_fixture_rejection "mobile lifecycle" "unsupported mobile lifecycle or platform adapter" lifecycle
}

validate_python_isolation_fixtures() {
  expect_fixture_rejection "system Python path" "forbidden Python runtime search" python:system-path
  expect_fixture_rejection "user site-package dependency" "forbidden Python runtime search" python:user-site
  expect_fixture_rejection "runtime installer call" "forbidden Python runtime search" python:installer
}

mode="full"
desktop_only_fixtures=0
python_isolation_fixtures=0
if (($# > 2)); then
  usage
  exit 2
fi
for argument in "$@"; do
  case "${argument}" in
    --script-ownership)
      [[ "${mode}" == "full" && ${desktop_only_fixtures} == 0 && ${python_isolation_fixtures} == 0 ]] || {
        usage
        exit 2
      }
      mode="script-ownership"
      ;;
    --desktop-only-fixtures)
      [[ "${mode}" == "full" && ${desktop_only_fixtures} == 0 ]] || {
        usage
        exit 2
      }
      desktop_only_fixtures=1
      ;;
    --python-isolation-fixtures)
      [[ "${mode}" == "full" && ${python_isolation_fixtures} == 0 ]] || {
        usage
        exit 2
      }
      python_isolation_fixtures=1
      ;;
    *)
      usage
      exit 2
      ;;
  esac
done

[[ -d "${project_root}" ]] || {
  printf 'Godot project root is missing: %s\n' "${project_root}" >&2
  exit 1
}
project_root="$(cd -- "${project_root}" && pwd -P)"

if [[ "${mode}" == "script-ownership" ]]; then
  validate_script_ownership
  ((failures == 0)) || exit 1
  printf 'Godot script ownership validation passed.\n'
  exit 0
fi

if ((desktop_only_fixtures)); then
  validate_desktop_only_fixtures
fi
if ((python_isolation_fixtures)); then
  validate_python_isolation_fixtures
fi
if ((desktop_only_fixtures || python_isolation_fixtures)); then
  exit 0
fi

run_full_validation || exit 1
printf 'Godot project closure validation passed.\n'
