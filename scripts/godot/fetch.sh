#!/usr/bin/env bash
set -euo pipefail

script_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd -P)"
repository_root="$(cd -- "${script_dir}/../.." && pwd -P)"

# shellcheck source=version.env
source "${script_dir}/version.env"

mode="fetch"
case "$(uname -s):$(uname -m)" in
  Linux:x86_64) target="linux-x86_64" ;;
  *) target="darwin-universal" ;;
esac
cache_root="${MORNLEA_GODOT_CACHE_DIR:-/tmp/mornlea-godot-cache}"
partial_path=""
materialization_dir=""
previous_editor=""

fail() {
  printf 'godot fetch: %s\n' "$*" >&2
  exit 1
}

usage() {
  printf '%s\n' \
    'usage: scripts/godot/fetch.sh [--verify-only] [--target darwin-universal|linux-x86_64] [--cache-dir ABSOLUTE_PATH]'
}

cleanup() {
  local status="$?"
  if [[ -n "${partial_path}" && -f "${partial_path}" ]]; then
    rm -f -- "${partial_path}"
  fi
  if [[ -n "${materialization_dir}" ]]; then
    # A failed publication must leave the previously qualified editor usable.
    if [[ -n "${previous_editor}" && ! -e "${artifact_dir}/${editor_name}" && ! -L "${artifact_dir}/${editor_name}" ]]; then
      if ! mv -- "${previous_editor}" "${artifact_dir}/${editor_name}"; then
        printf 'godot fetch: editor recovery remains at %s\n' "${previous_editor}" >&2
        return 1
      fi
    fi
    case "${materialization_dir}" in
      "${artifact_dir}/.godot-extract."*) rm -rf -- "${materialization_dir}" ;;
      *) printf 'godot fetch: refused unexpected staging cleanup: %s\n' "${materialization_dir}" >&2; return 1 ;;
    esac
  fi
  return "${status}"
}
trap cleanup EXIT

while (($# > 0)); do
  case "$1" in
    --verify-only)
      mode="verify"
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

case "${target}" in
  darwin-universal)
    engine_name="Godot_v${GODOT_VERSION}_macos.universal.zip"
    engine_url="${GODOT_MACOS_UNIVERSAL_URL}"
    engine_sha256="${GODOT_MACOS_UNIVERSAL_SHA256}"
    editor_name="Godot.app"
    editor_relative="Godot.app/Contents/MacOS/Godot"
    ;;
  linux-x86_64)
    engine_name="Godot_v${GODOT_VERSION}_linux.x86_64.zip"
    engine_url="${GODOT_LINUX_X86_64_URL}"
    engine_sha256="${GODOT_LINUX_X86_64_SHA256}"
    editor_name="Godot_v${GODOT_VERSION}_linux.x86_64"
    editor_relative="${editor_name}"
    ;;
  *) fail "unsupported Godot desktop target: ${target}" ;;
esac
[[ "${cache_root}" == /* ]] || fail "cache directory must be an absolute path outside the repository"

mkdir -p -- "${cache_root}"
cache_root="$(cd -- "${cache_root}" && pwd -P)"
case "${cache_root}/" in
  "${repository_root}/"*) fail "cache directory must stay outside the repository: ${cache_root}" ;;
esac

artifact_dir="${cache_root}/${GODOT_VERSION}/${target}"
mkdir -p -- "${artifact_dir}"
engine_path="${artifact_dir}/${engine_name}"
templates_path="${artifact_dir}/Godot_v${GODOT_VERSION}_export_templates.tpz"

sha256_file() {
  local file_path="$1"
  if command -v shasum >/dev/null 2>&1; then
    shasum -a 256 -- "${file_path}" | awk '{print $1}'
    return
  fi
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum -- "${file_path}" | awk '{print $1}'
    return
  fi
  fail "neither shasum nor sha256sum is available"
}

verify_file() {
  local label="$1"
  local file_path="$2"
  local expected="$3"
  local actual
  actual="$(sha256_file "${file_path}")"
  [[ "${actual}" == "${expected}" ]] || fail "${label} checksum mismatch: got ${actual}, want ${expected}"
}

verify_remote() {
  local label="$1"
  local url="$2"
  curl --fail --silent --show-error --location --head --retry 2 --retry-all-errors "${url}" >/dev/null || \
    fail "${label} official release URL is not reachable"
}

download_artifact() {
  local label="$1"
  local url="$2"
  local expected="$3"
  local destination="$4"
  if [[ -f "${destination}" ]]; then
    verify_file "${label}" "${destination}" "${expected}"
    printf '%s already verified: %s\n' "${label}" "${destination}"
    return
  fi
  partial_path="${destination}.partial.$$"
  curl --fail --show-error --location --retry 3 --retry-all-errors --output "${partial_path}" "${url}"
  verify_file "${label}" "${partial_path}" "${expected}"
  mv -- "${partial_path}" "${destination}"
  partial_path=""
  printf '%s downloaded and verified: %s\n' "${label}" "${destination}"
}

materialize_editor() {
  # Stage beside the destination so only a complete, verified application is
  # published; downloaded bytes alone cannot satisfy headless runtime checks.
  materialization_dir="$(mktemp -d "${artifact_dir}/.godot-extract.XXXXXX")"
  if [[ "${target}" == "darwin-universal" ]]; then
    ditto -x -k "${engine_path}" "${materialization_dir}" || fail "could not extract the verified Godot editor"
    [[ -d "${materialization_dir}/Godot.app" && ! -L "${materialization_dir}/Godot.app" ]] || fail "verified archive is missing a regular Godot.app directory"
  else
    unzip -q "${engine_path}" -d "${materialization_dir}" || fail "could not extract the verified Godot editor"
  fi
  local editor="${materialization_dir}/${editor_relative}"
  [[ -f "${editor}" && -x "${editor}" && ! -L "${editor}" ]] || fail "verified archive is missing an executable ${editor_relative}"
  if [[ -e "${artifact_dir}/${editor_name}" || -L "${artifact_dir}/${editor_name}" ]]; then
    mv -- "${artifact_dir}/${editor_name}" "${materialization_dir}/previous-${editor_name}"
    previous_editor="${materialization_dir}/previous-${editor_name}"
  fi
  mv -- "${materialization_dir}/${editor_name}" "${artifact_dir}/${editor_name}"
  printf 'Godot editor materialized: %s\n' "${artifact_dir}/${editor_relative}"
}

if [[ "${mode}" == "verify" ]]; then
  if [[ -f "${engine_path}" ]]; then
    verify_file "Godot Standard" "${engine_path}" "${engine_sha256}"
  else
    verify_remote "Godot Standard" "${engine_url}"
  fi
  if [[ -f "${templates_path}" ]]; then
    verify_file "Godot export templates" "${templates_path}" "${GODOT_EXPORT_TEMPLATES_SHA256}"
  else
    verify_remote "Godot export templates" "${GODOT_EXPORT_TEMPLATES_URL}"
  fi
  printf 'Godot %s metadata verified for %s; cache=%s\n' "${GODOT_VERSION}" "${target}" "${artifact_dir}"
  exit 0
fi

download_artifact "Godot Standard" "${engine_url}" "${engine_sha256}" "${engine_path}"
download_artifact "Godot export templates" "${GODOT_EXPORT_TEMPLATES_URL}" "${GODOT_EXPORT_TEMPLATES_SHA256}" "${templates_path}"
materialize_editor
