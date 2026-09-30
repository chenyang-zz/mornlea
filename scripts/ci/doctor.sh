#!/usr/bin/env bash
set -euo pipefail

usage() {
	printf 'usage: doctor.sh <preflight|audit|frontend|rust|go|native-linux|native-macos|agent|godot-static|godot-runtime>\n' >&2
	exit 2
}

profile=${1:-}
[[ $# -eq 1 ]] || usage

case "$profile" in
preflight) required=(bash git go gofmt node npx rg) ;;
audit) required=(bash go gofmt rg) ;;
frontend) required=(bash corepack git node) ;;
rust) required=(bash cargo rustc rustup) ;;
go) required=(bash go gofmt) ;;
native-linux) required=(bash cargo cc go ldd make nm readelf rustc rustup shasum) ;;
native-macos) required=(bash cargo codesign go install_name_tool make nm rustc rustup shasum) ;;
agent) required=(bash go python3 uv) ;;
godot-static) required=(bash make rg) ;;
godot-runtime)
  command -v uname >/dev/null 2>&1 || { printf 'missing required executable for godot-runtime: uname\n' >&2; exit 1; }
  if [[ "$(uname -s)" == Linux ]]; then
    required=(bash uname cargo cc g++ curl git go make nm patch patchelf perl pgrep rg rustc rustup shasum tar unzip uv)
  else
    required=(bash uname cargo cc clang++ codesign curl ditto git go install_name_tool make nm patch perl pgrep rg rustc rustup sandbox-exec shasum tar unzip uv xcrun)
  fi
  ;;
*) usage ;;
esac

missing=0
for executable in "${required[@]}"; do
	if ! command -v "$executable" >/dev/null 2>&1; then
		printf 'missing required executable for %s: %s\n' "$profile" "$executable" >&2
		missing=1
	fi
done
((missing == 0)) || exit 1
printf 'CI dependency profile passed: %s\n' "$profile"
