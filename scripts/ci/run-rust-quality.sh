#!/usr/bin/env bash
set -euo pipefail

[[ $# -eq 0 ]] || { printf 'usage: run-rust-quality.sh\n' >&2; exit 2; }
root=$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd -P)
cd "$root"
# The migration verifier rejects symlink ancestors, including macOS /var.
TMPDIR=$(cd "${TMPDIR:-/tmp}" && pwd -P)
export TMPDIR
(cd packages/agent && uv sync --locked)

previous_source=d042982d33bb1694d768b75b01c297bd02534a08
if ! git cat-file -e "$previous_source^{commit}"; then
	git fetch --no-tags origin "$previous_source"
fi
quality_run=$(mktemp -d "$TMPDIR/mornlea-rust-quality.XXXXXX")
# Failed packages remain available for inspection; success removes only our copy.
trap 'printf "Rust quality fixture directory: %s\n" "$quality_run" >&2' ERR
scripts/rust-server-opt-in.sh prepare-previous --source "$previous_source" --run-dir "$quality_run"
export MORNLEA_PREVIOUS_SERVER_BIN="$quality_run/previous-server"
export MORNLEA_PREVIOUS_PACKAGE="$quality_run/previous-runtime.json"
export MORNLEA_AGENT_PYTHON="$root/packages/agent/.venv/bin/python"
make rust-check
rm -rf "$quality_run"
