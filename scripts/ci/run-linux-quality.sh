#!/usr/bin/env bash
set -euo pipefail

fail() { printf 'Linux quality failed: %s\n' "$*" >&2; exit 1; }
[[ $# -eq 0 ]] || fail 'usage: run-linux-quality.sh'
root=$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd -P)
cd "$root"
export GOOS=linux GOARCH=amd64 CGO_ENABLED=1
scripts/ci/doctor.sh audit
package_output=$(scripts/ci/package-inventory.sh --all) || fail 'package inventory failed'
[[ -n "$package_output" ]] || fail 'package inventory is empty'
if type mapfile >/dev/null 2>&1; then
	mapfile -t all_packages <<< "$package_output"
else
	# macOS contract fixtures use the same reader semantics on its system Bash.
	all_packages=()
	while IFS= read -r package; do all_packages+=("$package"); done <<< "$package_output"
fi

packages=()
for package in "${all_packages[@]}"; do
	# Audit pins this explicit exclusion set against the Linux/Darwin package union.
	case "$package" in
	github.com/channing771/mornlea/packages/client/cmd/mornlea|\
	github.com/channing771/mornlea/packages/client/cmd/mornlea/app|\
	github.com/channing771/mornlea/packages/client/cmd/mornlea/benchmark|\
	github.com/channing771/mornlea/packages/client/cmd/mornlea/capture|\
	github.com/channing771/mornlea/packages/client/cmd/mornlea/devcapture|\
	github.com/channing771/mornlea/packages/client/render/hud|\
	github.com/channing771/mornlea/packages/tools/gfxspike) continue ;;
	esac
	packages+=("$package")
done
((${#packages[@]} > 0)) || fail 'supported package inventory is empty'
go test "${packages[@]}" -run '^$' -count=1
go vet "${packages[@]}"
go test ./packages/audit ./packages/server/storage/... ./packages/shared/network/... ./packages/shared/physics -v
