#!/usr/bin/env bash
set -euo pipefail

fail() {
	printf 'package inventory failed: %s\n' "$*" >&2
	exit 1
}

root=$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd -P)
expected_modules=$'packages/client\npackages/contracts\npackages/server\npackages/shared\npackages/tools'

workspace_modules() {
	go work edit -json |
		sed -nE 's/^[[:space:]]*"DiskPath":[[:space:]]*"(.*)"[,]?$/\1/p' |
		sed 's#^\./##' |
		LC_ALL=C sort -u
}

modules=$(cd "$root" && workspace_modules) || fail 'cannot parse go.work module directories'
[[ "$modules" == "$expected_modules" ]] || fail "unexpected go.work module directories: ${modules//$'\n'/,}"

# This inventory owns package loading validation, rather than treating `-e` output as usable by default.
platform_excludes_package_error() {
	[[ "$1" == linux && "$2" == amd64 && "$3" == packages/client && "$4" == github.com/channing771/mornlea/packages/client/cmd/mornlea/capture && "$5" == dependency ]]
}

list_module() {
	local goos=$1 goarch=$2 module=$3 output package loading
	shift 3
	output=$(cd "$root/$module" && CGO_ENABLED=1 GOOS="$goos" GOARCH="$goarch" go list -e -f '{{.ImportPath}}|{{if .Error}}direct{{end}}{{if .DepsErrors}}dependency{{end}}' "$@") || fail "cannot list packages for $module"
	while IFS='|' read -r package loading; do
		[[ -n "$package" ]] || fail "package loading error: empty import path in $module"
		if [[ -n "$loading" ]]; then
			# Darwin owns the capture dependency; Linux still enumerates the client module for future portable packages.
			platform_excludes_package_error "$goos" "$goarch" "$module" "$package" "$loading" && continue
			fail "package loading error: $package"
		fi
		printf '%s\n' "$package"
	done <<< "$output"
}

all_packages() {
	local goos goarch module
	# Each supported platform queries every workspace module before the union is partitioned.
	{
		for goos in linux darwin; do
			if [[ "$goos" == linux ]]; then goarch=amd64; else goarch=arm64; fi
			while IFS= read -r module; do
				list_module "$goos" "$goarch" "$module" ./...
			done <<< "$modules"
		done
	} | LC_ALL=C sort -u
}

client_packages() {
	{
		list_module darwin arm64 packages/client ./...
		list_module darwin arm64 packages/tools ./gfxspike
	} | LC_ALL=C sort -u
}

server_packages() {
	list_module linux amd64 packages/server ./... | LC_ALL=C sort -u
}

rest_packages() {
	local module
	for module in packages/contracts packages/shared packages/tools; do
		list_module linux amd64 "$module" ./...
	done | LC_ALL=C sort -u
}

check_partitions() {
	local all client server rest
	temporary=$(mktemp -d "${TMPDIR:-/tmp}/mornlea-package-inventory.XXXXXX") || fail 'cannot create temporary directory'
	trap 'rm -rf "$temporary"' EXIT
	all="$temporary/all"
	client="$temporary/client"
	server="$temporary/server"
	rest="$temporary/rest"
	all_packages > "$all"
	client_packages > "$client"
	server_packages > "$server"
	rest_packages > "$rest"
	"$root/scripts/ci/check-package-partitions.sh" "$all" "$client" "$server" "$rest"
}

case "${1:-}" in
--all)
	[[ $# -eq 1 ]] || fail 'usage: package-inventory.sh --all|--slice <client|server|rest>|--check'
	all_packages
	;;
--slice)
	[[ $# -eq 2 ]] || fail 'usage: package-inventory.sh --all|--slice <client|server|rest>|--check'
	case "$2" in
	client) client_packages ;;
	server) server_packages ;;
	rest) rest_packages ;;
	*) fail "unknown package slice: $2" ;;
	esac
	;;
--check)
	[[ $# -eq 1 ]] || fail 'usage: package-inventory.sh --all|--slice <client|server|rest>|--check'
	check_partitions
	;;
*) fail 'usage: package-inventory.sh --all|--slice <client|server|rest>|--check' ;;
esac
