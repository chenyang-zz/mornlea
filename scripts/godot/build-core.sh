#!/usr/bin/env bash
# Build the Go client core as a c-shared dynamic library into the Godot
# bridge distribution tree, colocate the Rust engine dynamic libraries it
# links, and optionally verify the loaded contract.
#
# Rulings pinned by this script (mirror scripts/godot/build-extension.sh and
# apps/mornlea-godot/addons/mornlea_bridge/mornlea_bridge.gdextension):
#   - Target naming: macOS builds are distributed through the established
#     "macos-universal" directory, the same tree the GDExtension descriptor
#     selects for every macOS arch. The Go pilot core is a single host-arch
#     slice inside that directory; a true universal library would need a
#     lipo of two Go builds plus matching universal engine libraries and is
#     reserved for a later change, exactly like the host-only GDExtension
#     slice build-extension.sh places there today.
#   - Generated header: go build -buildmode=c-shared emits a derived header
#     (libmornlea_client_core.h) next to the library. It is deleted here;
#     the canonical ABI contract stays the committed
#     packages/client/cmd/mornlea-godot-core/include/mornlea_client_core.h,
#     and the export-declaration content of the generated file is pinned by
#     the nm symbol verification below instead of a drifting second header.
#   - Linux uses libmornlea_client_core.so and libmornlea_engine.so with a
#     $ORIGIN RUNPATH. Verification loads a detached copy without loader env
#     overrides; the macOS renderer is not part of the Linux Godot bundle.
#   - Engine colocation: the macOS core transitively links libmornlea_engine.dylib
#     (through packages/shared/nativeabi) and libmornlea_client.dylib
#     (through packages/client/client). Both are copied from the canonical
#     engine release directory and the core's link-time absolute rpaths are
#     replaced with @loader_path so both dependencies resolve from the same
#     distribution directory. The engine release directory is produced by
#     "make rust"; this script never rebuilds Rust itself.
#   - Cache discipline: no repository-external persistent cache is needed.
#     The Go build uses the standard user-level Go build cache (GOCACHE),
#     and the verification caller below is compiled inside a mktemp scratch
#     directory that the exit trap removes. The script writes only inside
#     apps/mornlea-godot/addons/mornlea_bridge/bin/, the user-level Go
#     build cache, and that scratch directory; it never writes system
#     directories.
set -euo pipefail

script_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd -P)"
repository_root="$(cd -- "${script_dir}/../.." && pwd -P)"
project_root="${repository_root}/apps/mornlea-godot"
core_package_dir="${repository_root}/packages/client/cmd/mornlea-godot-core"
canonical_header="${core_package_dir}/include/mornlea_client_core.h"
engine_release_dir="${repository_root}/packages/engine/target/release"
core_library_name="libmornlea_client_core.dylib"
profile="debug"
verify="false"
scratch_dir=""

case "$(uname -s):$(uname -m)" in
  Darwin:arm64) host_target="aarch64-apple-darwin" ;;
  Darwin:x86_64) host_target="x86_64-apple-darwin" ;;
  Linux:x86_64) host_target="x86_64-unknown-linux-gnu" ;;
  *) host_target="unsupported" ;;
esac
target="${host_target}"

fail() {
  printf 'godot build-core: %s\n' "$*" >&2
  exit 1
}

usage() {
  printf '%s\n' \
    'usage: scripts/godot/build-core.sh [--verify] [--target aarch64-apple-darwin|x86_64-apple-darwin|x86_64-unknown-linux-gnu] [--profile debug|release]'
}

cleanup() {
  local exit_status="$?"
  if [[ -n "${scratch_dir}" && -d "${scratch_dir}" ]]; then
    case "${scratch_dir}/" in
      "${repository_root}/"*) ;;
      *) rm -rf -- "${scratch_dir}" ;;
    esac
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
    --target)
      (($# >= 2)) || fail "--target requires a value"
      target="$2"
      shift 2
      ;;
    --profile)
      (($# >= 2)) || fail "--profile requires a value"
      profile="$2"
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

[[ "${profile}" == "debug" || "${profile}" == "release" ]] || \
  fail "unsupported client-core profile: ${profile}"

# Every target name outside the approved desktop set is rejected before any
# build runs.
case "${target}" in
  aarch64-apple-darwin|x86_64-apple-darwin)
    platform_dir="macos-universal"
    engine_libraries=(libmornlea_engine.dylib libmornlea_client.dylib)
    ;;
  x86_64-unknown-linux-gnu)
    platform_dir="linux-x86_64"
    core_library_name="libmornlea_client_core.so"
    engine_libraries=(libmornlea_engine.so)
    command -v patchelf >/dev/null 2>&1 || fail "patchelf is required for relocatable Linux libraries"
    ;;
  *) fail "unsupported Godot desktop target: ${target}" ;;
esac

# The Go core, the engine libraries it links, and the GDExtension beside it
# are host-arch pilot builds; cross-target distribution is reserved.
[[ "${target}" == "${host_target}" ]] || \
  fail "cross-target core builds are reserved but not enabled in this pilot: target=${target} host=${host_target}"

destination_dir="${project_root}/addons/mornlea_bridge/bin/${platform_dir}/${profile}"
core_library="${destination_dir}/${core_library_name}"

# The engine release directory is the only sanctioned source of the project
# dynamic libraries; it is produced by "make rust", which also rewrites their
# install names to @rpath and re-signs them.
for engine_library_name in "${engine_libraries[@]}"; do
  [[ -f "${engine_release_dir}/${engine_library_name}" ]] || \
    fail "engine dynamic library is missing: ${engine_release_dir}/${engine_library_name}; run 'make rust' first"
done

mkdir -p -- "${destination_dir}"

# Build. Both profiles keep the symbol table (no -ldflags -s/-w) because the
# verification and crash symbolication rely on it; the debug profile only
# disables Go optimizations for debuggability. CGO is forced on because the
# c-shared surface and the engine bridge are cgo-backed.
build_flags=(-buildmode=c-shared)
if [[ "${profile}" == "debug" ]]; then
  build_flags+=(-gcflags='all=-N -l')
fi
(
  cd -- "${repository_root}"
  CGO_ENABLED=1 go build "${build_flags[@]}" \
    -o "${core_library}" \
    ./packages/client/cmd/mornlea-godot-core
)

# Delete the cgo-generated derived header: the canonical contract stays the
# committed include/mornlea_client_core.h (see the header ruling above).
rm -f -- "${destination_dir}/libmornlea_client_core.h"

# Colocate the engine dynamic libraries and repoint the core's rpaths: the
# cgo directives of packages/shared/nativeabi and packages/client/client
# embed absolute build-tree rpaths; remove every rpath that lives inside the
# repository and resolve the colocated copies through @loader_path instead.
for engine_library_name in "${engine_libraries[@]}"; do
  cp -- "${engine_release_dir}/${engine_library_name}" "${destination_dir}/${engine_library_name}"
done
if [[ "${target}" == x86_64-unknown-linux-gnu ]]; then
  # Replace the cgo build-tree search path with a distribution-relative path.
  # Linux does not link the native macOS client renderer.
  patchelf --set-rpath '$ORIGIN' "${core_library}"
else
  while IFS= read -r rpath_entry; do
    [[ -n "${rpath_entry}" ]] || continue
    case "${rpath_entry}" in
      @*) continue ;;
    esac
    resolved_rpath=""
    [[ -d "${rpath_entry}" ]] && resolved_rpath="$(cd -- "${rpath_entry}" 2>/dev/null && pwd -P || true)"
    if [[ "${rpath_entry}/" == "${repository_root}/"* || "${resolved_rpath}/" == "${repository_root}/"* ]]; then
      install_name_tool -delete_rpath "${rpath_entry}" "${core_library}"
    fi
  done < <(otool -l "${core_library}" | awk '/LC_RPATH/ {grab = 1} grab && $1 == "path" {print $2; grab = 0}')
  if ! otool -l "${core_library}" | awk '/LC_RPATH/ {grab = 1} grab && $1 == "path" {print $2; grab = 0}' | grep -qx '@loader_path'; then
    install_name_tool -add_rpath '@loader_path' "${core_library}"
  fi
  install_name_tool -id '@rpath/libmornlea_client_core.dylib' "${core_library}"
  # install_name_tool invalidates the linker's ad-hoc signature; restore it the
  # same way scripts/engine/deploy-dylib.sh does for the engine libraries.
  codesign --force --sign - "${core_library}" >/dev/null 2>&1

fi

# Deterministic export list; mirrors every //export directive in
# packages/client/cmd/mornlea-godot-core/exports.go. The audit gate pins the
# two lists together, so a new export cannot land without this verification
# learning its name.
exported_symbols="
mornlea_client_core_abi_version
mornlea_client_core_create
mornlea_client_core_destroy
mornlea_client_core_status_identity
mornlea_client_core_connect_begin
mornlea_client_core_connect_poll
mornlea_client_core_disconnect
mornlea_client_core_submit_input
mornlea_client_core_step
mornlea_client_core_world_pull
mornlea_client_core_frame_pull
mornlea_client_core_environment_pull
mornlea_client_core_status_pull
"

header_define() {
  local define_name="$1"
  local value
  # Header defines are unsigned literals, decimal or hexadecimal; both spell
  # unchanged in C. BSD sed has no alternation, so the literal body is matched
  # with one digit-leading character class covering both spellings.
  value="$(sed -n "s/^#define ${define_name} \([0-9][0-9xXa-fA-F]*\)u\$/\1/p" "${canonical_header}" | head -n 1)"
  [[ -n "${value}" ]] || fail "canonical header is missing ${define_name}"
  printf '%s' "${value}"
}

if [[ "${verify}" == "true" ]]; then
  # (a) Exported symbols: every export above must be a defined global symbol
  # of the built library (nm prefixes C symbols with an underscore).
  symbol_prefix="_"
  if [[ "${target}" == x86_64-unknown-linux-gnu ]]; then
    defined_symbols="$(nm -D --defined-only "${core_library}" | awk '{print $NF}')"
    symbol_prefix=""
  else
    defined_symbols="$(nm -gU "${core_library}" | awk '{print $NF}')"
  fi
  while IFS= read -r exported_symbol; do
    [[ -n "${exported_symbol}" ]] || continue
    if ! grep -qx "${symbol_prefix}${exported_symbol}" <<<"${defined_symbols}"; then
      fail "exported symbol is missing from ${core_library}: ${exported_symbol}"
    fi
  done <<<"${exported_symbols}"
  printf 'client-core symbols verified: %s\n' "${core_library}"

  # (b) Header: the committed canonical header is present and still the ABI
  # contract, and the cgo-generated header did not replace or survive beside
  # it in the distribution directory.
  [[ -f "${canonical_header}" ]] || fail "canonical client-core header is missing: ${canonical_header}"
  grep -q 'MORNLEA_CLIENT_ABI_MAJOR' "${canonical_header}" || \
    fail "canonical client-core header lost its ABI identity: ${canonical_header}"
  [[ ! -f "${destination_dir}/libmornlea_client_core.h" ]] || \
    fail "cgo-generated header must not be distributed: ${destination_dir}/libmornlea_client_core.h"
  printf 'client-core canonical header verified: %s\n' "${canonical_header}"

  # (c) Engine dependency: the core must reference both project libraries
  # through @rpath, the colocated copies must carry matching @rpath install
  # names, and the only rpath the core keeps must resolve beside it.
  if [[ "${target}" == x86_64-unknown-linux-gnu ]]; then
    [[ "$(patchelf --print-rpath "${core_library}")" == '$ORIGIN' ]] || \
      fail "Linux client core RUNPATH must be exactly \$ORIGIN"
    dependencies="$(patchelf --print-needed "${core_library}")"
    grep -qx 'libmornlea_engine.so' <<<"${dependencies}" || \
      fail "client core is missing its Linux engine dependency"
    [[ "${dependencies}" != *libmornlea_client* ]] || \
      fail "Linux Godot core must not depend on the macOS renderer"
    while IFS= read -r dependency; do
      [[ "${dependency}" != */* ]] || fail "absolute ELF dependency: ${dependency}"
    done <<<"${dependencies}"
    printf 'client-core dependencies verified: $ORIGIN resolves %s\n' "${destination_dir}/libmornlea_engine.so"
  else
    for dependency in '@rpath/libmornlea_engine.dylib' '@rpath/libmornlea_client.dylib'; do
      otool -L "${core_library}" | grep -qF "${dependency}" || \
        fail "client core is missing dynamic-library dependency ${dependency}"
    done
    for engine_library_name in "${engine_libraries[@]}"; do
      install_name="$(otool -D "${destination_dir}/${engine_library_name}" | awk 'NR == 2 {print $1}')"
      [[ "${install_name}" == "@rpath/${engine_library_name}" ]] || \
        fail "colocated ${engine_library_name} has install name ${install_name}, want @rpath/${engine_library_name}"
    done
    core_rpaths="$(otool -l "${core_library}" | awk '/LC_RPATH/ {grab = 1} grab && $1 == "path" {print $2; grab = 0}')"
    printf '%s\n' "${core_rpaths}" | grep -qx '@loader_path' || \
      fail "client core rpath must include @loader_path for colocated dependencies"
    while IFS= read -r rpath_entry; do
      [[ -n "${rpath_entry}" ]] || continue
      # Verify side: only @-relative rpaths may remain. Build-side deletion
      # removes the repo-internal entries cgo embeds, but verification must not
      # pass any absolute rpath, which could shadow @loader_path by LC_RPATH
      # ordering.
      case "${rpath_entry}" in
        @*) continue ;;
        *) fail "client core carries a non-relative rpath: ${rpath_entry}" ;;
      esac
    done <<<"${core_rpaths}"
    printf 'client-core dependencies verified: @loader_path resolves %s and %s\n' \
      "${destination_dir}/libmornlea_engine.dylib" "${destination_dir}/libmornlea_client.dylib"

  fi

  # (d) Family identity: a throwaway C caller dlopens the built library from
  # a scratch directory outside the repository (never committed), confirms
  # the packed ABI version, runs the create/status-identity/destroy
  # roundtrip, and thereby also proves dyld resolves both colocated engine
  # libraries when loading the core. DYLD search variables are unset so the
  # resolution can only come from @loader_path.
  scratch_root="${TMPDIR:-/tmp}"
  scratch_dir="$(mktemp -d "${scratch_root%/}/mornlea-godot-build-core.XXXXXX")"
  case "${scratch_dir}/" in
    "${repository_root}/"*)
      # The trap cleanup refuses repo-internal paths, so remove this
      # script-owned directory here before failing, or a pathological TMPDIR
      # pointing into the repository would leak it.
      rm -rf -- "${scratch_dir}"
      fail "scratch directory must stay outside the repository: ${scratch_dir}"
      ;;
  esac
  cat >"${scratch_dir}/identity_check.c" <<'C_CALLER'
#include <dlfcn.h>
#include <stdint.h>
#include <stdio.h>

typedef uint32_t (*create_fn)(uint32_t, uint32_t, const uint64_t *, uint32_t, uint64_t *);
typedef uint32_t (*destroy_fn)(uint64_t);
typedef uint32_t (*identity_fn)(uint64_t, uint8_t *, uint32_t, uint32_t *);
typedef uint64_t (*version_fn)(void);

static uint32_t decode32(const uint8_t *bytes) {
  return (uint32_t)bytes[0] | ((uint32_t)bytes[1] << 8) |
         ((uint32_t)bytes[2] << 16) | ((uint32_t)bytes[3] << 24);
}

int main(int argc, char **argv) {
  if (argc != 2) {
    return 2;
  }
  void *handle = dlopen(argv[1], RTLD_NOW | RTLD_LOCAL);
  if (handle == NULL) {
    fprintf(stderr, "dlopen failed: %s\n", dlerror());
    return 1;
  }
  version_fn version = (version_fn)dlsym(handle, "mornlea_client_core_abi_version");
  create_fn create = (create_fn)dlsym(handle, "mornlea_client_core_create");
  destroy_fn destroy = (destroy_fn)dlsym(handle, "mornlea_client_core_destroy");
  identity_fn identity = (identity_fn)dlsym(handle, "mornlea_client_core_status_identity");
  if (version == NULL || create == NULL || destroy == NULL || identity == NULL) {
    fprintf(stderr, "dlsym failed: %s\n", dlerror());
    return 1;
  }
  uint64_t packed = version();
  if (packed != (((uint64_t)EXPECTED_ABI_MAJOR << 32) | (uint64_t)EXPECTED_ABI_MINOR)) {
    fprintf(stderr, "abi version mismatch: got 0x%llx\n", (unsigned long long)packed);
    return 1;
  }
  /* Request every pilot family at contract version 1: the registry accepts a
   * requested version equal to or older than the registered contract, so the
   * roundtrip stays valid across compatible family bumps. */
  uint64_t request[EXPECTED_FAMILY_COUNT];
  for (uint32_t family = 0; family < EXPECTED_FAMILY_COUNT; family++) {
    request[family] = ((uint64_t)1 << 32) | (uint64_t)(family + 1);
  }
  uint64_t session = 0;
  uint32_t status = create(EXPECTED_ABI_MAJOR, EXPECTED_ABI_MINOR, request, EXPECTED_FAMILY_COUNT, &session);
  if (status != 0) {
    fprintf(stderr, "create failed: status=%u\n", status);
    return 1;
  }
  uint32_t required = 0;
  status = identity(session, NULL, 0, &required);
  if (status != EXPECTED_STATUS_INSUFFICIENT_CAPACITY || required == 0) {
    fprintf(stderr, "identity capacity query failed: status=%u required=%u\n", status, required);
    return 1;
  }
  _Alignas(8) uint8_t buffer[512];
  status = identity(session, buffer, required, &required);
  if (status != 0) {
    fprintf(stderr, "identity read failed: status=%u\n", status);
    return 1;
  }
  uint32_t magic = decode32(buffer);
  uint32_t layout = decode32(buffer + 4);
  uint32_t major = decode32(buffer + 8);
  uint32_t minor = decode32(buffer + 12);
  uint32_t families = decode32(buffer + 16);
  if (magic != EXPECTED_MAGIC_IDENTITY || layout != EXPECTED_IDENTITY_LAYOUT ||
      major != EXPECTED_ABI_MAJOR ||
      minor != EXPECTED_ABI_MINOR || families != EXPECTED_FAMILY_COUNT) {
    fprintf(stderr, "identity record mismatch: magic=0x%x layout=%u major=%u minor=%u families=%u\n",
            magic, layout, major, minor, families);
    return 1;
  }
  status = destroy(session);
  if (status != 0) {
    fprintf(stderr, "destroy failed: status=%u\n", status);
    return 1;
  }
  printf("client-core identity check passed: major=%u minor=%u families=%u required=%u\n",
         major, minor, families, required);
  return 0;
}
C_CALLER
  expected_major="$(header_define MORNLEA_CLIENT_ABI_MAJOR)"
  expected_minor="$(header_define MORNLEA_CLIENT_ABI_MINOR)"
  expected_families="$(header_define MORNLEA_CLIENT_FAMILY_COUNT)"
  expected_magic="$(header_define MORNLEA_CLIENT_MAGIC_IDENTITY)"
  expected_layout="$(header_define MORNLEA_CLIENT_IDENTITY_VERSION)"
  expected_capacity="$(header_define MORNLEA_CLIENT_STATUS_INSUFFICIENT_CAPACITY)"
  loader_link_flags=()
  [[ "${target}" != x86_64-unknown-linux-gnu ]] || loader_link_flags=(-ldl)
  cc \
    -DEXPECTED_ABI_MAJOR="${expected_major}" \
    -DEXPECTED_ABI_MINOR="${expected_minor}" \
    -DEXPECTED_FAMILY_COUNT="${expected_families}" \
    -DEXPECTED_MAGIC_IDENTITY="${expected_magic}" \
    -DEXPECTED_IDENTITY_LAYOUT="${expected_layout}" \
    -DEXPECTED_STATUS_INSUFFICIENT_CAPACITY="${expected_capacity}" \
    -o "${scratch_dir}/identity_check" "${scratch_dir}/identity_check.c" "${loader_link_flags[@]}"
  # Load a detached copy, so build-tree libraries cannot satisfy colocation.
  mkdir -p -- "${scratch_dir}/distribution"
  cp -- "${core_library}" "${scratch_dir}/distribution/${core_library_name}"
  for engine_library_name in "${engine_libraries[@]}"; do
    cp -- "${destination_dir}/${engine_library_name}" "${scratch_dir}/distribution/${engine_library_name}"
  done
  identity_output="$(env -u LD_LIBRARY_PATH -u LD_PRELOAD -u DYLD_LIBRARY_PATH -u DYLD_FRAMEWORKS_PATH -u DYLD_FALLBACK_LIBRARY_PATH \
    "${scratch_dir}/identity_check" "${scratch_dir}/distribution/${core_library_name}")"
  printf '%s\n' "${identity_output}"
  [[ "${identity_output}" == *"client-core identity check passed"* ]] || \
    fail "client-core identity verification failed"
fi

printf 'client core built for %s/%s: %s\n' "${target}" "${profile}" "${core_library}"
