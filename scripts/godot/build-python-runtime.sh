#!/usr/bin/env bash
set -euo pipefail

script_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd -P)"
repository_root="$(cd -- "${script_dir}/../.." && pwd -P)"
project_root="${repository_root}/apps/mornlea-godot"
fork_root="${script_dir}/py4godot"

# shellcheck source=python-version.env
source "${script_dir}/python-version.env"
# shellcheck source=py4godot/build-inputs.env
source "${fork_root}/build-inputs.env"

verify="false"
offline="false"
target="${PY4GODOT_BUILD_TARGET}"
if [[ "$(uname -s)-$(uname -m)" == "Linux-x86_64" ]]; then
  target="x86_64-unknown-linux-gnu"
fi
cache_root="${MORNLEA_PY4GODOT_CACHE_DIR:-/tmp/mornlea-py4godot-cache}"
partial_path=""
work_root=""

fail() {
  printf 'Py4Godot build: %s\n' "$*" >&2
  exit 1
}

usage() {
  printf '%s\n' \
    'usage: scripts/godot/build-python-runtime.sh --verify [--offline] [--target darwin-arm64|x86_64-unknown-linux-gnu] [--cache-dir ABSOLUTE_PATH]'
}

safe_remove_generated() {
  local path="$1"
  case "${path}" in
    "${project_root}/addons/py4godot"|"${cache_root}/"*) rm -rf -- "${path}" ;;
    *) fail "refusing to remove path outside generated Python locations: ${path}" ;;
  esac
}

cleanup() {
  local exit_status="$?"
  if [[ -n "${partial_path}" && -f "${partial_path}" ]]; then
    rm -f -- "${partial_path}"
  fi
  if [[ -n "${work_root}" && -d "${work_root}" ]]; then
    safe_remove_generated "${work_root}"
  fi
  return "${exit_status}"
}
trap cleanup EXIT

while (($# > 0)); do
  case "$1" in
    --verify)
      verify="true"
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

[[ "${verify}" == "true" ]] || fail "--verify is required"
# Platform inputs keep native outputs independently pinned; Linux never inherits
# macOS loader or artifact checksums.
case "${target}" in
  darwin-arm64)
    [[ "$(uname -s)-$(uname -m)" == "Darwin-arm64" ]] || \
      fail "unsupported Py4Godot build target on host: $(uname -s)-$(uname -m)"
    runtime_directory="cpython-${PY4GODOT_CPYTHON_VERSION}-darwin64"
    library_suffix="dylib"
    ;;
  x86_64-unknown-linux-gnu)
    # shellcheck source=py4godot/linux-build-inputs.env
    source "${fork_root}/linux-build-inputs.env"
    [[ "$(uname -s)-$(uname -m)" == "Linux-x86_64" ]] || \
      fail "unsupported Py4Godot build target on host: $(uname -s)-$(uname -m)"
    runtime_directory="cpython-${PY4GODOT_CPYTHON_VERSION}-linux64"
    library_suffix="so"
    ;;
  *) fail "unsupported Py4Godot build target: ${target}" ;;
esac
[[ "${target}" == "${PY4GODOT_BUILD_TARGET}" ]] || fail "unsupported Py4Godot build target: ${target}"
[[ "${cache_root}" == /* ]] || fail "cache directory must be an absolute path outside the repository"

mkdir -p -- "${cache_root}"
cache_root="$(cd -- "${cache_root}" && pwd -P)"
case "${cache_root}/" in
  "${repository_root}/"*) fail "cache directory must stay outside the repository: ${cache_root}" ;;
esac

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

sha256_text() {
  if command -v shasum >/dev/null 2>&1; then
    shasum -a 256 | awk '{print $1}'
    return
  fi
  sha256sum | awk '{print $1}'
}

verify_file() {
  local label="$1"
  local file_path="$2"
  local expected="$3"
  local actual
  actual="$(sha256_file "${file_path}")"
  [[ "${actual}" == "${expected}" ]] || fail "${label} checksum mismatch: got ${actual}, want ${expected}"
}

download_file() {
  local label="$1"
  local url="$2"
  local expected="$3"
  local destination="$4"
  if [[ -f "${destination}" ]]; then
    verify_file "${label}" "${destination}" "${expected}"
    return
  fi
  [[ "${offline}" == "false" ]] || fail "offline cache miss: ${destination}"
  partial_path="${destination}.partial.$$"
  curl --fail --show-error --location --retry 3 --retry-all-errors \
    --output "${partial_path}" "${url}"
  verify_file "${label}" "${partial_path}" "${expected}"
  mv -- "${partial_path}" "${destination}"
  partial_path=""
}

release_dir="${cache_root}/${PY4GODOT_VERSION}/${target}"
source_dir="${cache_root}/source/${PY4GODOT_SOURCE_REVISION}"
release_archive="${release_dir}/py4godot.zip"
source_archive="${source_dir}/py4godot.tar.gz"
mkdir -p -- "${release_dir}" "${source_dir}"
download_file "release archive" "${PY4GODOT_RELEASE_URL}" "${PY4GODOT_RELEASE_SHA256}" "${release_archive}"
download_file "source archive" "${PY4GODOT_SOURCE_ARCHIVE_URL}" "${PY4GODOT_SOURCE_ARCHIVE_SHA256}" "${source_archive}"

release_entries="$(unzip -Z1 "${release_archive}")"
if printf '%s\n' "${release_entries}" | grep -Eq '(^/|(^|/)\.\.(/|$))'; then
  fail "release archive contains a path outside its declared root"
fi
if printf '%s\n' "${release_entries}" | grep -Evq "^${PY4GODOT_ARCHIVE_ROOT}(/|$)"; then
  fail "release archive contains a path outside ${PY4GODOT_ARCHIVE_ROOT}"
fi

source_entries="$(tar -tzf "${source_archive}")"
if printf '%s\n' "${source_entries}" | grep -Eq '(^/|(^|/)\.\.(/|$))'; then
  fail "source archive contains a path outside its declared root"
fi
if printf '%s\n' "${source_entries}" | grep -Evq "^${PY4GODOT_SOURCE_ARCHIVE_ROOT}(/|$)"; then
  fail "source archive contains a path outside ${PY4GODOT_SOURCE_ARCHIVE_ROOT}"
fi

series_file="${fork_root}/patches/series"
[[ -f "${series_file}" ]] || fail "patches/series is missing"
series_digest="$({
  while IFS= read -r patch_name || [[ -n "${patch_name}" ]]; do
    [[ -z "${patch_name}" ]] && continue
    [[ "${patch_name}" == "$(basename -- "${patch_name}")" && "${patch_name}" == *.patch ]] || \
      fail "invalid patch-series entry: ${patch_name}"
    patch_path="${fork_root}/patches/${patch_name}"
    [[ -f "${patch_path}" ]] || fail "patch-series file is missing: ${patch_name}"
    printf '%s  %s\n' "$(sha256_file "${patch_path}")" "${patch_name}"
  done < "${series_file}"
} | sha256_text)"
[[ "${series_digest}" == "${PY4GODOT_PATCH_SERIES_SHA256}" ]] || \
  fail "patch-series checksum mismatch: got ${series_digest}, want ${PY4GODOT_PATCH_SERIES_SHA256}"

if [[ "${target}" == "darwin-arm64" ]]; then
  clang_version="$(clang++ --version | sed -n 's/^Apple clang version \([^ ]*\).*/\1/p')"
  [[ "${clang_version}" == "${PY4GODOT_APPLE_CLANG_VERSION}" ]] || \
    fail "Apple clang version mismatch: got ${clang_version}, want ${PY4GODOT_APPLE_CLANG_VERSION}"
  sdk_version="$(xcrun --sdk macosx --show-sdk-version)"
  [[ "${sdk_version}" == "${PY4GODOT_MACOS_SDK_VERSION}" ]] || \
    fail "macOS SDK version mismatch: got ${sdk_version}, want ${PY4GODOT_MACOS_SDK_VERSION}"
else
  gcc_version="$(g++ -dumpfullversion)"
  [[ "${gcc_version}" == "${PY4GODOT_GCC_VERSION}" ]] || \
    fail "GCC version mismatch: got ${gcc_version}, want ${PY4GODOT_GCC_VERSION}"
fi

work_root="$(mktemp -d "${cache_root}/build.XXXXXX")"
tar -xzf "${source_archive}" -C "${work_root}"
source_root="${work_root}/${PY4GODOT_SOURCE_ARCHIVE_ROOT}"
while IFS= read -r patch_name || [[ -n "${patch_name}" ]]; do
  [[ -z "${patch_name}" ]] && continue
  patch -d "${source_root}" -p1 --batch --forward < "${fork_root}/patches/${patch_name}"
done < "${series_file}"

unzip -q "${release_archive}" \
  "${PY4GODOT_ARCHIVE_ROOT}/LICENSE" \
  "${PY4GODOT_ARCHIVE_ROOT}/Python.svg" \
  "${PY4GODOT_ARCHIVE_ROOT}/python.gdextension" \
  "${PY4GODOT_ARCHIVE_ROOT}/signal_script.py" \
  "${PY4GODOT_ARCHIVE_ROOT}/${runtime_directory}/*" \
  -d "${work_root}/materialized"

staged_addon="${work_root}/materialized/${PY4GODOT_ARCHIVE_ROOT}"
python_root="${staged_addon}/${runtime_directory}/python"
python_stdlib="${python_root}/lib/python${PY4GODOT_CPYTHON_VERSION%.*}"
loader_path="${python_root}/bin/pythonscript.${library_suffix}"
[[ -f "${loader_path}" ]] || fail "upstream ${target} Python loader is missing"
[[ -f "${python_root}/bin/main.${library_suffix}" ]] || fail "upstream ${target} Python bridge is missing"
[[ -f "${python_stdlib}/site-packages/py4godot/classes/Node.py" ]] || fail "embedded Py4Godot package is missing"

cp -- "${source_root}/py4godot/utils/smart_cast.py" \
  "${python_stdlib}/site-packages/py4godot/utils/smart_cast.py"
singleton_count=0
while IFS= read -r -d '' generated_class; do
  if ! grep -Eq '^      singleton = [A-Za-z0-9_]+\(\)$' "${generated_class}"; then
    continue
  fi
  perl -0pi -e \
    's/(      singleton = )([A-Za-z0-9_]+)\(\)\n(      singleton\._ptr)/$1$2.construct_without_init()\n$3/g' \
    "${generated_class}"
  singleton_count=$((singleton_count + 1))
done < <(find "${python_stdlib}/site-packages/py4godot/classes" -type f -name '*.py' -print0)
[[ "${singleton_count}" -eq 41 ]] || \
  fail "generated singleton rewrite count mismatch: got ${singleton_count}, want 41"
if grep -R -Eq '^      singleton = [A-Za-z0-9_]+\(\)$' \
  "${python_stdlib}/site-packages/py4godot/classes"; then
  fail "generated singleton rewrite left an owning temporary"
fi

built_loader="${source_root}/build/mornlea/pythonscript.${library_suffix}"
mkdir -p -- "${source_root}/build/mornlea" "${source_root}/python_files"
ln -s -- "${staged_addon}/${runtime_directory}" \
  "${source_root}/python_files/${runtime_directory}"
(
  cd -- "${source_root}"
  if [[ "${target}" == "darwin-arm64" ]]; then
    clang++ -dynamiclib -std=c++17 -O3 -DNDEBUG -Werror -Wall -Wextra -Wpedantic -Wno-unused-parameter \
      -Wl,-install_name,@rpath/pythonscript.dylib \
      -Wl,-rpath,@loader_path/../lib \
      -Wl,-rpath,@loader_path/../Resources/addons/py4godot/${runtime_directory}/python/lib \
      -DMORNLEA_PYTHON_RUNTIME_DIRECTORY=\"${runtime_directory}\" \
      -DMORNLEA_PYTHON_MAIN_LIBRARY=\"main.dylib\" \
      -I. \
      -Ipy4godot/godot_bindings \
      -Ipy4godot/gdextension-api \
      -I"python_files/${runtime_directory}/python/include/python3.14" \
      py4godot/godot_bindings/pythonscript.cpp \
      -L"python_files/${runtime_directory}/python/lib" -lpython3.14 \
      -o build/mornlea/pythonscript.${library_suffix}
  else
    g++ -shared -fPIC -std=c++17 -O3 -DNDEBUG -Werror -Wall -Wextra -Wpedantic -Wno-unused-parameter \
      -Wl,-soname,pythonscript.so '-Wl,-rpath,$ORIGIN/../lib' \
      -DMORNLEA_PYTHON_RUNTIME_DIRECTORY=\"${runtime_directory}\" \
      -DMORNLEA_PYTHON_MAIN_LIBRARY=\"main.so\" \
      -I. -Ipy4godot/godot_bindings -Ipy4godot/gdextension-api \
      -I"python_files/${runtime_directory}/python/include/python3.14" \
      py4godot/godot_bindings/pythonscript.cpp \
      -L"python_files/${runtime_directory}/python/lib" -lpython3.14 -ldl \
      -o build/mornlea/pythonscript.so
  fi
)
verify_file "hardened loader" "${built_loader}" "${PY4GODOT_HARDENED_LOADER_SHA256}"
cp -- "${built_loader}" "${loader_path}"

# Runtime package installers are outside the approved embedded-Python surface.
safe_remove_generated "${python_stdlib}/site-packages/pip"
safe_remove_generated "${python_stdlib}/site-packages/pip-26.0.1.dist-info"
safe_remove_generated "${python_stdlib}/ensurepip"
rm -f -- "${python_root}/bin/pip" "${python_root}/bin/pip3" "${python_root}/bin/pip3.14"
chmod +x "${python_root}/bin/python" "${python_root}/bin/python3" "${python_root}/bin/python3.14"

artifact_digest="$({
  find "${staged_addon}" -type f -exec shasum -a 256 -- {} + | \
    sed "s#  ${staged_addon}/#  #"
  while IFS= read -r artifact_path; do
    relative_path="${artifact_path#${staged_addon}/}"
    printf 'symlink:%s  %s\n' "$(readlink "${artifact_path}")" "${relative_path}"
  done < <(find "${staged_addon}" -type l -print)
} | LC_ALL=C sort -k2 | sha256_text)"
[[ "${artifact_digest}" == "${PY4GODOT_HARDENED_ARTIFACT_SHA256}" ]] || \
  fail "hardened artifact checksum mismatch: got ${artifact_digest}, want ${PY4GODOT_HARDENED_ARTIFACT_SHA256}"

destination="${project_root}/addons/py4godot"
safe_remove_generated "${destination}"
mkdir -p -- "${project_root}/addons"
mv -- "${staged_addon}" "${destination}"

printf 'Py4Godot hardened runtime materialized: version=%s target=%s loader_sha256=%s artifact_sha256=%s\n' \
  "${PY4GODOT_HARDENED_VERSION}" "${target}" "${PY4GODOT_HARDENED_LOADER_SHA256}" \
  "${PY4GODOT_HARDENED_ARTIFACT_SHA256}"
