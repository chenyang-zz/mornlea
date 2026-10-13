#!/usr/bin/env bash
# Runs the standard gates from `AGENTS.md`; any failed step makes the final result fail.
# Set `GATES_SKIP_RACE=1` only for iteration. Final validation must run the full race gate.
set -uo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$ROOT"
export PATH="$ROOT/packages/audit/.venv/bin:$PATH"
FAIL=0
STEP=0

step() { STEP=$((STEP+1)); echo; echo "== [${STEP}] $*"; }
run() { local desc="$1"; shift; echo "   --> $*"; if bash -c "$*"; then echo "   [PASS] $desc"; else echo "   [FAIL] $desc"; FAIL=1; fi; }

step "gofmt check (expected output: empty)"
run "gofmt check" 'unformatted=$(find . -type f -name "*.go" -not -path "./vendor/*" -not -path "./.worktrees/*" -not -path "./.claude/worktrees/*" -exec gofmt -l {} +); test -z "$unformatted"'

step "go vet for every workspace module"
run "go vet" 'for module in ./packages/contracts ./packages/shared ./packages/server ./packages/client ./packages/tools; do go vet $module/... || exit 1; done'

step "English source-comment language ratchet"
run "English source-comment language ratchet" 'make comment-language-check'

step "archcheck: dependency, version, and documentation boundaries"
run "archcheck" 'uv run pytest packages/audit'

step "OpenSpec strict validation"
run "OpenSpec strict validation" 'openspec validate --all --strict --no-interactive'

step "Rust build with the pinned toolchain"
run "Rust build" 'make rust'

if [ "${GATES_SKIP_RACE:-0}" != "1" ]; then
  step "Full race tests for every workspace module"
  run "Full race tests" 'for module in ./packages/contracts ./packages/shared ./packages/server ./packages/client ./packages/tools; do go test $module/... -race || exit 1; done'
fi

echo
if [ "$FAIL" = 0 ]; then
  echo "All gates passed ✅"
else
  echo "One or more gates failed ❌ (fix the failures before closeout)"
  exit 1
fi
