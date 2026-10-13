#!/usr/bin/env bash
set -euo pipefail

[[ $# -eq 0 ]] || { printf 'usage: run-integration-client.sh\n' >&2; exit 2; }
command -v go >/dev/null 2>&1 || { printf 'missing required executable: go\n' >&2; exit 1; }
root=$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd -P)
cd "$root"
# The bounded server probe owns non-race timing; all report failures remain fatal.
go test ./packages/client/cmd/mornlea/benchmark -run '^TestScenarioV7EightSessionServerProbeIsRealAndBounded$' -count=1
go test ./packages/client/client ./packages/server/server ./packages/client/cmd/mornlea/benchmark ./packages/tools/perfcheck -run 'Test(PerfReportV6|ScenarioV6|PerfcheckV6|PerfcheckV5SameScenario|PerformanceThresholds|InterestObserver|HostStats|BenchmarkServerEpoch|BenchmarkServerMeasuredWindow)' -count=1
go test ./packages/contracts/... ./packages/shared/... ./packages/server/... ./packages/client/... ./packages/tools/... -bench=. -benchtime=1x -run='^$'
go test ./packages/shared/network ./packages/server/server ./packages/client/render -run '^$' -bench '(RemotePlayerStateCodec|EightPlayerInterest|RemoteAvatarNameTag)' -benchmem -benchtime=100x -count=1
