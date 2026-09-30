#!/usr/bin/env bash
set -euo pipefail

script_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd -P)"
repository_root="$(cd -- "${script_dir}/../.." && pwd -P)"
[[ $# -gt 0 ]] || { printf 'usage: scripts/godot/deny-network.sh EXECUTABLE [ARGUMENTS...]\n' >&2; exit 2; }

case "$(uname -s)-$(uname -m)" in
  Darwin-*)
    exec sandbox-exec -p '(version 1) (allow default) (deny network*)' "$@"
    ;;
  Linux-x86_64)
    # Compile outside the checkout and reuse only a helper for this source hash.
    cache_root="${MORNLEA_PY4GODOT_CACHE_DIR:-/tmp/mornlea-py4godot-cache}/network-denial"
    [[ "${cache_root}" == /* ]] || { printf 'network-denial cache must be absolute\n' >&2; exit 1; }
    mkdir -p -- "${cache_root}"
    cache_root="$(cd -- "${cache_root}" && pwd -P)"
    case "${cache_root}/" in
      "${repository_root}/"*) printf 'network-denial cache must be outside the repository\n' >&2; exit 1 ;;
    esac
    source_hash="$(sha256sum "${script_dir}/linux-network-deny.c" | awk '{print $1}')"
    helper="${cache_root}/linux-network-deny-${source_hash}"
    if [[ ! -x "${helper}" ]]; then
      command -v cc >/dev/null 2>&1 || { printf 'Linux network denial requires a C compiler\n' >&2; exit 1; }
      staging_helper="$(mktemp "${cache_root}/building.XXXXXX")"
      trap 'rm -f -- "${staging_helper}"' EXIT
      cc -std=c11 -Wall -Wextra -Werror -O2 "${script_dir}/linux-network-deny.c" -o "${staging_helper}"
      mv -- "${staging_helper}" "${helper}"
    fi
    exec "${helper}" "$@"
    ;;
  *) printf 'unsupported network-denial host: %s\n' "$(uname -s)-$(uname -m)" >&2; exit 1 ;;
esac
