#!/usr/bin/env bash
# Export the production project and keep the embedded interpreter's filesystem
# layout beside it: CPython cannot import its standard library from the PCK.
set -euo pipefail
script_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd -P)"
repository_root="$(cd -- "${script_dir}/../.." && pwd -P)"
project_root="${repository_root}/apps/mornlea-godot"
source "${script_dir}/version.env"
source "${script_dir}/python-version.env"
source "${script_dir}/py4godot/build-inputs.env"
output_dir="${repository_root}/build/godot-linux"
verify="false"
work_dir=""
previous_output=""
owner_name=".mornlea-linux-distribution"
fail() { printf 'godot export-linux: %s\n' "$*" >&2; exit 1; }
cleanup() {
  local status="$?"
  if [[ -n "${previous_output}" && ! -e "${output_dir}" ]]; then
    if ! mv -- "${previous_output}" "${output_dir}"; then
      printf 'godot export-linux: previous bundle retained at %s\n' "${previous_output}" >&2
      return 1
    fi
  fi
  if [[ -n "${work_dir}" ]]; then
    case "${work_dir}" in
      "${output_parent}/.mornlea-linux-export."*) rm -rf -- "${work_dir}" ;;
      *) printf 'godot export-linux: unexpected staging directory %s\n' "${work_dir}" >&2; return 1 ;;
    esac
  fi
  return "${status}"
}
trap cleanup EXIT
while (($#)); do
  case "$1" in
    --output) (($# >= 2)) || fail '--output requires an absolute path'; output_dir="$2"; shift 2 ;;
    --verify) verify="true"; shift ;;
    --help|-h) printf '%s\n' 'usage: scripts/godot/export-linux.sh [--verify] [--output ABSOLUTE_PATH]'; exit 0 ;;
    *) fail "unsupported argument: $1" ;;
  esac
done
[[ "${output_dir}" == /* ]] || fail 'output must be an absolute path'
[[ ! -L "${output_dir}" ]] || fail 'output must not be a symbolic link'
output_parent="$(dirname -- "${output_dir}")"
mkdir -p -- "${output_parent}"
output_parent="$(cd -- "${output_parent}" && pwd -P)"
output_dir="${output_parent}/$(basename -- "${output_dir}")"
case "${output_dir}/" in
  "${repository_root}/build/"*) ;;
  /|"${repository_root}/"*) fail 'output must stay outside source directories' ;;
esac
if [[ -e "${output_dir}" ]]; then
  [[ -d "${output_dir}" ]] || fail 'output must be a directory'
  if [[ -n "$(find "${output_dir}" -mindepth 1 -maxdepth 1 -print -quit)" ]]; then
    [[ -f "${output_dir}/${owner_name}" && ! -L "${output_dir}/${owner_name}" ]] || fail "unowned nonempty output: ${output_dir}"
    [[ "$(cat "${output_dir}/${owner_name}")" == 'mornlea-linux-x86_64-v1' ]] || fail "unowned nonempty output: ${output_dir}"
  fi
fi
[[ "$(uname -s):$(uname -m)" == Linux:x86_64 ]] || fail 'Linux x86_64 host is required'
source "${script_dir}/py4godot/linux-build-inputs.env"
"${script_dir}/validate-project.sh"
"${script_dir}/sync-assets.sh" --check
native_root="${project_root}/addons/mornlea_bridge/bin/linux-x86_64"
for native in debug/libmornlea_godot.so release/libmornlea_godot.so release/libmornlea_client_core.so release/libmornlea_engine.so; do
  [[ -f "${native_root}/${native}" && ! -L "${native_root}/${native}" ]] || fail "missing native artifact: ${native}; run make godot-build"
done
[[ "$(patchelf --print-rpath "${native_root}/release/libmornlea_client_core.so")" == '$ORIGIN' ]] || fail 'client core RUNPATH must be exactly $ORIGIN'
# Qualification runs create bytecode and import sidecars. Restore a clean
# source-verified runtime before checking its full pinned tree, rather than
# silently excluding generated files from the integrity contract. Run export
# while no other Godot process uses this ignored project runtime.
"${script_dir}/build-python-runtime.sh" --verify --offline --target x86_64-unknown-linux-gnu
python_addon="${project_root}/addons/py4godot"
runtime_directory="cpython-${PY4GODOT_CPYTHON_VERSION}-linux64"
python_runtime="${python_addon}/${runtime_directory}/python"
[[ -f "${python_runtime}/bin/pythonscript.so" ]] || fail 'missing qualified Python runtime; run make godot-build'
loader_digest="$(sha256sum "${python_runtime}/bin/pythonscript.so" | awk '{print $1}')"
[[ "${loader_digest}" == "${PY4GODOT_HARDENED_LOADER_SHA256}" ]] || fail 'embedded Python loader checksum mismatch'
artifact_digest="$({
  find "${python_addon}" -type f -exec shasum -a 256 -- {} + | sed "s#  ${python_addon}/#  #"
  while IFS= read -r artifact_path; do
    relative_path="${artifact_path#${python_addon}/}"
    printf 'symlink:%s  %s\n' "$(readlink "${artifact_path}")" "${relative_path}"
  done < <(find "${python_addon}" -type l -print)
} | LC_ALL=C sort -k2 | sha256sum | awk '{print $1}')"
[[ "${artifact_digest}" == "${PY4GODOT_HARDENED_ARTIFACT_SHA256}" ]] || fail 'embedded Python artifact checksum mismatch'
cache_root="${MORNLEA_GODOT_CACHE_DIR:-/tmp/mornlea-godot-cache}"
template_archive="${cache_root}/${GODOT_VERSION}/linux-x86_64/Godot_v${GODOT_VERSION}_export_templates.tpz"
[[ -f "${template_archive}" ]] || fail 'missing export-template cache; run scripts/godot/fetch.sh --target linux-x86_64'
[[ "$(sha256sum "${template_archive}" | awk '{print $1}')" == "${GODOT_EXPORT_TEMPLATES_SHA256}" ]] || fail 'export-template checksum mismatch'
godot_binary="$("${script_dir}/godot.sh" --print-path)"
work_dir="$(mktemp -d "${output_parent}/.mornlea-linux-export.XXXXXX")"
bundle="${work_dir}/bundle"
mkdir -p -- "${bundle}" "${work_dir}/data/godot/export_templates/${GODOT_VERSION/-stable/.stable}"
unzip -oqj "${template_archive}" templates/linux_debug.x86_64 templates/linux_release.x86_64 -d "${work_dir}/data/godot/export_templates/${GODOT_VERSION/-stable/.stable}"
# The editor discovers debug variants even during a release export. Require
# both profiles before import instead of silently exporting an incomplete PCK.
# Godot 4.7.2's EditorHelp deferred documentation callback can outlive an
# immediate cold-editor shutdown. A bounded 120-frame import lets it settle.
if ! import_output="$("${godot_binary}" --headless --path "${project_root}" --editor --quit-after 120 2>&1)"; then
  printf '%s\n' "${import_output}" >&2; fail 'headless project import failed'
fi
[[ "${import_output}" != *'ERROR:'* ]] || { printf '%s\n' "${import_output}" >&2; fail 'headless import reported an error'; }
if ! export_output="$(XDG_DATA_HOME="${work_dir}/data" "${godot_binary}" --headless --path "${project_root}" --export-release 'Mornlea Linux x86_64' "${bundle}/mornlea.x86_64" 2>&1)"; then
  printf '%s\n' "${export_output}" >&2; fail 'Linux release export failed'
fi
[[ "${export_output}" != *'ERROR:'* ]] || { printf '%s\n' "${export_output}" >&2; fail 'Linux export reported an error'; }
[[ -x "${bundle}/mornlea.x86_64" && -f "${bundle}/mornlea.pck" ]] || fail 'Linux executable or PCK is missing'
mkdir -p -- "${bundle}/addons/py4godot" "${bundle}/addons/mornlea_bridge/bin/linux-x86_64/release"
cp -a -- "${python_addon}/${runtime_directory}" "${bundle}/addons/py4godot/"
cp -a -- "${native_root}/release/." "${bundle}/addons/mornlea_bridge/bin/linux-x86_64/release/"
# Py4Godot's class executor opens Python scripts with importlib's filesystem
# loader, even when Godot discovered their resources inside the PCK. Preserve
# only production sources at their mapped paths; tests and caches stay out.
while IFS= read -r -d '' python_source; do
  relative_source="${python_source#${project_root}/}"
  mkdir -p -- "$(dirname -- "${bundle}/${relative_source}")"
  cp -- "${python_source}" "${bundle}/${relative_source}"
done < <(find "${project_root}/app" "${project_root}/features" "${project_root}/platform/desktop" \
  "${project_root}/addons/mornlea_bridge" -type f -name '*.py' -print0)
# Godot keeps the descriptor's original resource paths but also flattens ELF
# libraries at the bundle root. Require the intact mapped tree to load instead
# so the hardened loader derives CPython's root from its own bin directory.
rm -f -- "${bundle}/pythonscript.so" "${bundle}/libmornlea_godot.so"
if [[ "${verify}" == true ]]; then
  mkdir -p -- "${work_dir}/user-data" "${work_dir}/user-cache"
  if ! run_output="$("${script_dir}/deny-network.sh" /usr/bin/env -u PYTHONPATH -u PYTHONHOME -u LD_LIBRARY_PATH -u LD_PRELOAD \
      XDG_DATA_HOME="${work_dir}/user-data" XDG_CACHE_HOME="${work_dir}/user-cache" \
      "${bundle}/mornlea.x86_64" --headless --quit-after 120 2>&1)"; then
    printf '%s\n' "${run_output}" >&2; fail 'exported production main scene failed'
  fi
  printf '%s\n' "${run_output}"
  [[ "${run_output}" != *'ERROR:'* && "${run_output}" == *'[mornlea-bootstrap] state=ready target=x86_64-unknown-linux-gnu missing=none mismatched=none'* && "${run_output}" == *'[mornlea-host] catalog={"disabled":["platform.desktop.audio","platform.desktop.lifecycle"],"errors":[],"ok":true,"order":["session","actors","platform.desktop.input","player_view","ui","world"]}'* && "${run_output}" == *'[mornlea-lifecycle] python-deinit=bridge'* ]] || fail 'exported production catalog did not activate and close cleanly'
fi
printf '%s\n' 'mornlea-linux-x86_64-v1' > "${bundle}/${owner_name}"
cat > "${bundle}/distribution.env" <<PROVENANCE
GODOT_VERSION=${GODOT_VERSION}
TARGET=x86_64-unknown-linux-gnu
PY4GODOT_VERSION=${PY4GODOT_HARDENED_VERSION}
PY4GODOT_LOADER_SHA256=${PY4GODOT_HARDENED_LOADER_SHA256}
PY4GODOT_ARTIFACT_SHA256=${PY4GODOT_HARDENED_ARTIFACT_SHA256}
SOURCE_REVISION=$(git -C "${repository_root}" rev-parse HEAD)
SOURCE_DIRTY=$(if [[ -n "$(git -C "${repository_root}" status --porcelain)" ]]; then printf 1; else printf 0; fi)
PROVENANCE
if [[ -e "${output_dir}" ]]; then
  previous_output="${work_dir}/previous-bundle"
  mv -- "${output_dir}" "${previous_output}"
fi
mv -- "${bundle}" "${output_dir}"
printf 'Linux production catalog exported: %s/mornlea.x86_64\n' "${output_dir}"
