package archcheck_test

import (
	"bytes"
	"fmt"
	"os"
	"os/exec"
	"path/filepath"
	"slices"
	"strings"
	"testing"
)

func TestCIEntrypointsOwnRequiredCommands(t *testing.T) {
	root := repositoryRoot(t)
	makefile := string(readFile(t, filepath.Join(root, "Makefile")))
	targets := map[string]struct {
		prerequisite string
		commands     []string
	}{
		"ci-rust-quality":          {"", []string{"scripts/ci/doctor.sh rust", "$(MAKE) rust-check"}},
		"ci-frontend":              {"", []string{"scripts/ci/doctor.sh frontend", "$(MAKE) frontend-check"}},
		"ci-native-linux":          {"", []string{"scripts/ci/doctor.sh native-linux", "$(MAKE) build-linux-server", "$(GO) test ./packages/shared/nativeabi ./packages/shared/core ./packages/shared/physics ./packages/client/mesh -race -count=1", "scripts/ci/verify-linux-bundle.sh --root \"$(CURDIR)\"", "scripts/ci/package-native-artifact.sh --platform linux-amd64 --sha \"$(CI_CANDIDATE_SHA)\" --root \"$(CURDIR)\" --manifest \"$(CI_LINUX_MANIFEST)\" --", "bin/mornlea-server bin/libmornlea_engine.so packages/engine/target/release/libmornlea_engine.so"}},
		"ci-native-macos":          {"", []string{"scripts/ci/doctor.sh native-macos", "$(MAKE) rust", "scripts/ci/platform-id.sh", "scripts/ci/package-native-artifact.sh --platform \"$$platform\" --sha \"$(CI_CANDIDATE_SHA)\" --root \"$(CURDIR)\" --manifest \"$(CI_MACOS_MANIFEST)\" --", "packages/engine/target/release/libmornlea_engine.dylib packages/engine/target/release/libmornlea_client.dylib"}},
		"ci-verify-linux-artifact": {"", []string{"scripts/ci/verify-native-artifact.sh --platform linux-amd64 --sha \"$(CI_CANDIDATE_SHA)\" --root \"$(CURDIR)\" --manifest \"$(CI_LINUX_MANIFEST)\""}},
		"ci-verify-macos-artifact": {"", []string{"scripts/ci/platform-id.sh", "scripts/ci/verify-native-artifact.sh --platform \"$$platform\" --sha \"$(CI_CANDIDATE_SHA)\" --root \"$(CURDIR)\" --manifest \"$(CI_MACOS_MANIFEST)\""}},
		"ci-linux-quality":         {"ci-verify-linux-artifact", []string{"scripts/ci/run-linux-quality.sh"}},
		"ci-race-server":           {"ci-verify-linux-artifact", []string{"scripts/ci/run-go-race.sh server"}},
		"ci-race-rest":             {"ci-verify-linux-artifact", []string{"scripts/ci/run-go-race.sh rest"}},
		"ci-race-client":           {"ci-verify-macos-artifact", []string{"scripts/ci/run-go-race.sh client"}},
		"ci-integration-server":    {"ci-verify-linux-artifact", []string{"scripts/ci/run-integration-server.sh"}},
		"ci-integration-client":    {"ci-verify-macos-artifact", []string{"scripts/ci/run-integration-client.sh"}},
	}
	for target, want := range targets {
		t.Run(target, func(t *testing.T) {
			if !makeTargetIsPhony(makefile, target) {
				t.Errorf("%s is not phony", target)
			}
			if !strings.Contains(makeTargetRecipe(t, makefile, "help"), "make "+target+" ") {
				t.Errorf("help omits %s", target)
			}
			header := target + ":"
			if want.prerequisite != "" {
				header += " " + want.prerequisite
			}
			if !slices.Contains(strings.Split(makefile, "\n"), header) {
				t.Fatalf("missing exact rule %q", header)
			}
			body := makeTargetRecipe(t, strings.Replace(makefile, header, target+":", 1), target)
			last := -1
			for _, command := range want.commands {
				index := strings.Index(body, command)
				if index < 0 || index <= last {
					t.Errorf("missing or reordered command %q in %s", command, body)
				}
				last = index
			}
			cmd := exec.Command("make", "-n", target, "CI_CANDIDATE_SHA="+strings.Repeat("a", 40))
			cmd.Dir = root
			output, err := cmd.CombinedOutput()
			if err != nil {
				t.Fatalf("dry run: %v\n%s", err, output)
			}
			if want.prerequisite != "" {
				if strings.Count(string(output), "scripts/ci/verify-native-artifact.sh --platform") != 1 {
					t.Errorf("consumer must verify exactly once: %s", output)
				}
				if strings.Index(string(output), "verify-native-artifact.sh") > strings.Index(string(output), want.commands[0]) {
					t.Errorf("consumer precedes verification: %s", output)
				}
			}
		})
	}
	for _, variable := range []string{"CI_CANDIDATE_SHA ?= $(shell git rev-parse HEAD)", "CI_LINUX_MANIFEST := build/ci/native-linux.manifest", "CI_MACOS_MANIFEST := build/ci/native-macos.manifest"} {
		if !strings.Contains(makefile, variable) {
			t.Errorf("missing %s", variable)
		}
	}
	// Follow prerequisites transitively so a hidden helper cannot pull CI into local targets.
	edges := map[string][]string{}
	for _, line := range strings.Split(makefile, "\n") {
		if strings.HasPrefix(line, "\t") || strings.HasPrefix(line, "#") || strings.Contains(line, "=") {
			continue
		}
		left, right, ok := strings.Cut(line, ":")
		if !ok {
			continue
		}
		for _, name := range strings.Fields(left) {
			edges[name] = append(edges[name], strings.Fields(right)...)
		}
	}
	for _, target := range []string{"build", "test", "run", "companion-agent-check"} {
		queue, seen := []string{target}, map[string]bool{}
		for len(queue) > 0 {
			name := queue[0]
			queue = queue[1:]
			if seen[name] {
				continue
			}
			seen[name] = true
			if strings.HasPrefix(name, "ci-") || strings.HasPrefix(name, "godot-") {
				t.Errorf("legacy %s reaches %s", target, name)
			}
			queue = append(queue, edges[name]...)
		}
	}
}

func TestLinuxQualityPlatformExclusionsAreExact(t *testing.T) {
	root := repositoryRoot(t)
	bin, log := ciCommandRecorder(t)
	realGo, err := exec.LookPath("go")
	if err != nil {
		t.Fatal(err)
	}
	recorder := strings.TrimPrefix(ciRecorderSource("go"), "#!/usr/bin/env bash\n")
	writeExecutable(t, filepath.Join(bin, "go"), fmt.Sprintf("#!/usr/bin/env bash\nif [[ $1 == list ]]; then printf 'go: downloading example.com/module v1.0.0\\n' >&2; exec %q \"$@\"; fi\nif [[ $1 == work ]]; then exec %q \"$@\"; fi\n%s", realGo, realGo, recorder))
	path := bin + ":" + os.Getenv("PATH")
	inventory := func(args ...string) []string {
		t.Helper()
		cmd := exec.Command(filepath.Join(root, "scripts/ci/package-inventory.sh"), args...)
		cmd.Dir = root
		cmd.Env = ciEnvironment(path, nil)
		var stderr bytes.Buffer
		cmd.Stderr = &stderr
		output, err := cmd.Output()
		if err != nil {
			t.Fatalf("package inventory %v: %v\n%s", args, err, stderr.String())
		}
		if !strings.Contains(stderr.String(), "go: downloading example.com/module v1.0.0") {
			t.Fatalf("package inventory %v did not exercise stderr notice", args)
		}
		return strings.Fields(string(output))
	}
	all, client := inventory("--all"), inventory("--slice", "client")
	const prefix = "github.com/channing771/mornlea/packages/"
	excluded := []string{
		"client/cmd/mornlea", "client/cmd/mornlea/app", "client/cmd/mornlea/benchmark",
		"client/cmd/mornlea/capture", "client/cmd/mornlea/devcapture",
		"client/render/hud", "tools/gfxspike",
	}
	for _, relative := range excluded {
		for name, packages := range map[string][]string{"union": all, "client slice": client} {
			if !slices.Contains(packages, prefix+relative) {
				t.Errorf("%s lacks Darwin-owned package %s", name, relative)
			}
		}
	}
	excludedSet := make(map[string]bool, len(excluded))
	for _, relative := range excluded {
		excludedSet[prefix+relative] = true
	}
	var supported []string
	for _, packagePath := range all {
		if !excludedSet[packagePath] {
			supported = append(supported, packagePath)
		}
	}
	slices.Sort(supported)
	output, err := ciRunWithEnv(root, path, []string{"CI_TEST_LOG=" + log, "CI_TEST_ROOT=" + root, "CI_TEST_LINUX=1"}, filepath.Join(root, "scripts/ci/run-linux-quality.sh"))
	if err != nil {
		t.Fatalf("real quality package selection: %v\n%s", err, output)
	}
	commands := strings.Split(strings.TrimSpace(string(readFile(t, log))), "\n")
	if len(commands) != 3 {
		t.Fatalf("quality commands = %q", commands)
	}
	compile, vet := strings.Fields(commands[0]), strings.Fields(commands[1])
	if len(compile) < 5 || !slices.Equal(compile[2:len(compile)-3], supported) || !slices.Equal(vet[2:], supported) {
		t.Fatalf("compile/vet differ from supported inventory: compile=%q vet=%q supported=%q", compile, vet, supported)
	}
}

func TestCIEntrypointsRejectInvalidSHAAndUnverifiedConsumers(t *testing.T) {
	for _, target := range []string{"ci-native-linux", "ci-native-macos", "ci-linux-quality", "ci-race-server", "ci-race-rest", "ci-race-client", "ci-integration-server", "ci-integration-client"} {
		t.Run(target, func(t *testing.T) {
			root := t.TempDir()
			writeFile(t, filepath.Join(root, "Makefile"), readFile(t, filepath.Join(repositoryRoot(t), "Makefile")))
			called := filepath.Join(root, "called")
			for _, script := range []string{"doctor.sh", "run-linux-quality.sh", "run-go-race.sh", "run-integration-server.sh", "run-integration-client.sh"} {
				writeExecutable(t, filepath.Join(root, "scripts/ci", script), "#!/usr/bin/env bash\nprintf called > \"$CI_TEST_CALLED\"\n")
			}
			for _, script := range []string{"verify-native-artifact.sh", "platform-id.sh"} {
				writeExecutable(t, filepath.Join(root, "scripts/ci", script), string(readFile(t, filepath.Join(repositoryRoot(t), "scripts/ci", script))))
			}
			for _, sha := range []string{"invalid", strings.Repeat("A", 40), strings.Repeat("a", 39)} {
				cmd := exec.Command("make", target, "CI_CANDIDATE_SHA="+sha)
				cmd.Dir = root
				cmd.Env = ciEnvironment(os.Getenv("PATH"), []string{"CI_TEST_CALLED=" + called, "MORNLEA_CI_UNAME_S=Darwin", "MORNLEA_CI_UNAME_M=arm64"})
				output, err := cmd.CombinedOutput()
				if err == nil || !strings.Contains(string(output), "invalid candidate SHA") {
					t.Fatalf("invalid SHA result: %v\n%s", err, output)
				}
				if _, err := os.Stat(called); !os.IsNotExist(err) {
					t.Fatalf("invalid candidate ran dependent work: %v", err)
				}
			}
			if strings.HasPrefix(target, "ci-native-") {
				return
			}
			cmd := exec.Command("make", target, "CI_CANDIDATE_SHA="+strings.Repeat("a", 40))
			cmd.Dir = root
			cmd.Env = ciEnvironment(os.Getenv("PATH"), []string{"CI_TEST_CALLED=" + called, "MORNLEA_CI_UNAME_S=Darwin", "MORNLEA_CI_UNAME_M=arm64"})
			output, err := cmd.CombinedOutput()
			if err == nil {
				t.Fatalf("missing manifest succeeded: %s", output)
			}
			if _, err := os.Stat(called); !os.IsNotExist(err) {
				t.Fatalf("unverified consumer ran: %v", err)
			}
		})
	}
}

func TestCIIntegrationOrderedCommandsAndFailures(t *testing.T) {
	for _, test := range []struct {
		name     string
		commands []string
	}{
		{"server", []string{"doctor\tagent", "make\tcompanion-agent-check", "make\tcompanion-agent-integration", "go\ttest\t./packages/server/server\t-run\tTestTCPPlayerAndWorld|TestMemoryTCPParity\t-race\t-count=10"}},
		{"client", []string{
			"go\ttest\t./packages/client/cmd/mornlea/benchmark\t-run\t^TestScenarioV7EightSessionServerProbeIsRealAndBounded$\t-count=1",
			"go\ttest\t./packages/client/client\t./packages/server/server\t./packages/client/cmd/mornlea/benchmark\t./packages/tools/perfcheck\t-run\tTest(PerfReportV6|ScenarioV6|PerfcheckV6|PerfcheckV5SameScenario|PerformanceThresholds|InterestObserver|HostStats|BenchmarkServerEpoch|BenchmarkServerMeasuredWindow)\t-count=1",
			"go\ttest\t./packages/contracts/...\t./packages/shared/...\t./packages/server/...\t./packages/client/...\t./packages/tools/...\t./packages/audit/...\t-bench=.\t-benchtime=1x\t-run=^$",
			"go\ttest\t./packages/shared/network\t./packages/server/server\t./packages/client/render\t-run\t^$\t-bench\t(RemotePlayerStateCodec|EightPlayerInterest|RemoteAvatarNameTag)\t-benchmem\t-benchtime=100x\t-count=1",
		}},
	} {
		t.Run(test.name, func(t *testing.T) {
			for failAt := 0; failAt <= len(test.commands); failAt++ {
				t.Run(fmt.Sprintf("fail-at-%d", failAt), func(t *testing.T) {
					root, script := ciEntrypointFixture(t, "run-integration-"+test.name+".sh")
					bin, log := ciCommandRecorder(t)
					writeExecutable(t, filepath.Join(root, "scripts/ci/doctor.sh"), ciRecorderSource("doctor"))
					output, err := ciRunWithEnv(t.TempDir(), bin+":"+os.Getenv("PATH"), []string{"CI_TEST_LOG=" + log, fmt.Sprintf("CI_TEST_FAIL_AT=%d", failAt), "CI_TEST_ROOT=" + root}, script)
					if (err != nil) != (failAt > 0) {
						t.Fatalf("unexpected result: %v\n%s", err, output)
					}
					want := test.commands
					if failAt > 0 {
						want = want[:failAt]
					}
					got := strings.Split(strings.TrimSpace(string(readFile(t, log))), "\n")
					if !slices.Equal(got, want) {
						t.Fatalf("commands = %q, want %q", got, want)
					}
				})
			}
		})
	}
}

func TestCILinuxQualityOrderedCommandsAndFailures(t *testing.T) {
	const prefix = "github.com/channing771/mornlea/packages/"
	for _, mode := range []string{"pass", "doctor-error", "inventory-error", "empty", "compile-error", "vet-error", "focused-error"} {
		t.Run(mode, func(t *testing.T) {
			root, script := ciEntrypointFixture(t, "run-linux-quality.sh")
			bin, log := ciCommandRecorder(t)
			writeExecutable(t, filepath.Join(root, "scripts/ci/doctor.sh"), ciRecorderSource("doctor"))
			inventory := "#!/usr/bin/env bash\n[[ $# == 1 && $1 == --all ]] || exit 92\n"
			switch mode {
			case "inventory-error":
				inventory += "printf 'partial/package\\n'; exit 42\n"
			case "empty":
				inventory += "exit 0\n"
			default:
				inventory += "printf '%s\\n' '" + prefix + "audit' '" + prefix + "client/cmd/mornlea/app' '" + prefix + "client/cmd/mornlea/capture' '" + prefix + "client/cmd/mornlea/devcapture' '" + prefix + "tools/gfxspike' '" + prefix + "tools/perfcheck'\n"
			}
			writeExecutable(t, filepath.Join(root, "scripts/ci/package-inventory.sh"), inventory)
			failAt := map[string]int{"doctor-error": 1, "compile-error": 2, "vet-error": 3, "focused-error": 4}[mode]
			output, err := ciRunWithEnv(t.TempDir(), bin+":"+os.Getenv("PATH"), []string{"CI_TEST_LOG=" + log, fmt.Sprintf("CI_TEST_FAIL_AT=%d", failAt), "CI_TEST_ROOT=" + root, "CI_TEST_LINUX=1"}, script)
			if (err != nil) != (mode != "pass") {
				t.Fatalf("unexpected result: %v\n%s", err, output)
			}
			want := []string{
				"doctor\taudit",
				"go\ttest\t" + prefix + "audit\t" + prefix + "tools/perfcheck\t-run\t^$\t-count=1",
				"go\tvet\t" + prefix + "audit\t" + prefix + "tools/perfcheck",
				"go\ttest\t./packages/audit\t./packages/server/storage/...\t./packages/shared/network/...\t./packages/shared/physics\t-v",
			}
			if mode == "inventory-error" || mode == "empty" {
				want = want[:1]
			} else if failAt > 0 {
				want = want[:failAt]
			}
			got := strings.Split(strings.TrimSpace(string(readFile(t, log))), "\n")
			if !slices.Equal(got, want) {
				t.Fatalf("commands = %q, want %q", got, want)
			}
		})
	}
}

func TestCIAuditEntrypointsRejectMissingRipgrepBeforeWork(t *testing.T) {
	for _, test := range []struct {
		name   string
		script string
		args   []string
	}{
		{"linux quality", "run-linux-quality.sh", nil},
		{"rest race", "run-go-race.sh", []string{"rest"}},
	} {
		t.Run(test.name, func(t *testing.T) {
			root, script := ciEntrypointFixture(t, test.script)
			writeExecutable(t, filepath.Join(root, "scripts/ci/doctor.sh"), string(readFile(t, filepath.Join(repositoryRoot(t), "scripts/ci/doctor.sh"))))
			called := filepath.Join(t.TempDir(), "inventory-called")
			writeExecutable(t, filepath.Join(root, "scripts/ci/package-inventory.sh"), "#!/usr/bin/env bash\nprintf called > \"$CI_TEST_CALLED\"\n")
			bin := ciFixtureBin(t, []string{"bash", "go", "gofmt"})
			dirname, err := exec.LookPath("dirname")
			if err != nil {
				t.Fatal(err)
			}
			if err := os.Symlink(dirname, filepath.Join(bin, "dirname")); err != nil {
				t.Fatal(err)
			}
			output, err := ciRunWithEnv(root, bin, []string{"CI_TEST_CALLED=" + called}, script, test.args...)
			if err == nil || !strings.Contains(output, "missing required executable for audit: rg") || strings.Contains(output, "passed") {
				t.Fatalf("missing rg result: %v\n%s", err, output)
			}
			if _, err := os.Stat(called); !os.IsNotExist(err) {
				t.Fatalf("inventory ran without rg: %v", err)
			}
		})
	}
}

func TestCIRaceServerAndClientDoNotRequireRipgrep(t *testing.T) {
	for _, slice := range []string{"server", "client"} {
		t.Run(slice, func(t *testing.T) {
			root, script := ciEntrypointFixture(t, "run-go-race.sh")
			writeExecutable(t, filepath.Join(root, "scripts/ci/package-inventory.sh"), "#!/usr/bin/env bash\nprintf 'example/package\\n'\n")
			bin := ciFixtureBin(t, []string{"bash", "go", "gofmt"})
			dirname, err := exec.LookPath("dirname")
			if err != nil {
				t.Fatal(err)
			}
			if err := os.Symlink(dirname, filepath.Join(bin, "dirname")); err != nil {
				t.Fatal(err)
			}
			called := filepath.Join(t.TempDir(), "go-test-called")
			writeExecutable(t, filepath.Join(bin, "go"), "#!/usr/bin/env bash\nprintf '%s\\n' \"$@\" > \"$CI_TEST_CALLED\"\n")
			output, err := ciRunWithEnv(root, bin, []string{"CI_TEST_CALLED=" + called}, script, slice)
			if err != nil {
				t.Fatalf("race %s requires an unrelated tool: %v\n%s", slice, err, output)
			}
			if got := strings.Fields(string(readFile(t, called))); len(got) < 3 || got[0] != "test" || got[1] != "example/package" {
				t.Fatalf("race %s Go test args = %v", slice, got)
			}
		})
	}
}

func TestCILinuxBundleRestoresTargetAndRejectsMutations(t *testing.T) {
	for _, mode := range []string{"pass", "dependency", "go-error", "needed", "origin", "readelf-error", "abi", "mesh", "collision", "raycast", "nm-error", "ldd-error", "wrong-library", "help-success", "help-text", "loader-error", "loader-open"} {
		t.Run(mode, func(t *testing.T) {
			root, script := ciEntrypointFixture(t, "verify-linux-bundle.sh")
			marker := filepath.Join(root, "packages/engine/target/marker")
			writeFile(t, marker, []byte("preserve native cache"))
			bin := t.TempDir()
			writeExecutable(t, filepath.Join(bin, "go"), `#!/usr/bin/env bash
[[ "$*" == 'list -deps ./packages/server/cmd/mornlea-server' && $GOOS == linux && $GOARCH == amd64 && $CGO_ENABLED == 1 ]] || exit 92
[[ $CI_TEST_MODE != go-error ]] || exit 42
printf 'example/server\n'
[[ $CI_TEST_MODE != dependency ]] || printf 'github.com/channing771/mornlea/packages/client/render\n'
`)
			writeExecutable(t, filepath.Join(bin, "readelf"), `#!/usr/bin/env bash
[[ "$*" == '-d bin/mornlea-server' ]] || exit 92
[[ $CI_TEST_MODE != readelf-error ]] || exit 42
[[ $CI_TEST_MODE == needed ]] || printf 'NEEDED libmornlea_engine.so\n'
[[ $CI_TEST_MODE == origin ]] || printf 'RUNPATH $ORIGIN\n'
exit 0
`)
			writeExecutable(t, filepath.Join(bin, "nm"), `#!/usr/bin/env bash
[[ "$*" == '-D --defined-only bin/libmornlea_engine.so' ]] || exit 92
[[ $CI_TEST_MODE != nm-error ]] || exit 42
for entry in abi:mornlea_engine_abi_version mesh:mornlea_mesh_section collision:mornlea_collision_resolve raycast:mornlea_raycast_batch; do
  [[ $CI_TEST_MODE == "${entry%%:*}" ]] || printf '0001 T %s\n' "${entry#*:}"
done
exit 0
`)
			writeExecutable(t, filepath.Join(bin, "ldd"), `#!/usr/bin/env bash
[[ "$*" == 'bin/mornlea-server' && ! -e packages/engine/target ]] || exit 92
printf called > "$CI_TEST_PROBED"
[[ $CI_TEST_MODE != ldd-error ]] || exit 42
if [[ $CI_TEST_MODE == wrong-library ]]; then printf '/wrong/libmornlea_engine.so\n'; else printf 'libmornlea_engine.so => %s/bin/libmornlea_engine.so (0x1)\n' "$PWD"; fi
`)
			writeExecutable(t, filepath.Join(root, "bin/mornlea-server"), `#!/usr/bin/env bash
[[ "$*" == '-h' && ! -e packages/engine/target ]] || exit 92
[[ $CI_TEST_MODE != help-success ]] || exit 0
[[ $CI_TEST_MODE == help-text ]] || printf 'flag: help requested\n'
[[ $CI_TEST_MODE != loader-error ]] || printf 'error while loading shared libraries\n'
[[ $CI_TEST_MODE != loader-open ]] || printf 'cannot open shared object\n'
exit 1
`)
			probed := filepath.Join(t.TempDir(), "probed")
			output, err := ciRunWithEnv(t.TempDir(), bin+":"+os.Getenv("PATH"), []string{"CI_TEST_MODE=" + mode, "CI_TEST_PROBED=" + probed}, script, "--root", root)
			if (err != nil) != (mode != "pass") {
				t.Fatalf("unexpected result: %v\n%s", err, output)
			}
			if string(readFile(t, marker)) != "preserve native cache" {
				t.Fatal("target was not restored")
			}
			beforeDetach := slices.Contains([]string{"dependency", "go-error", "needed", "origin", "readelf-error", "abi", "mesh", "collision", "raycast", "nm-error"}, mode)
			_, statErr := os.Stat(probed)
			if beforeDetach && !os.IsNotExist(statErr) {
				t.Fatal("invalid bundle reached detached load")
			}
			if !beforeDetach && statErr != nil {
				t.Fatalf("detached probe not reached: %v\n%s", statErr, output)
			}
		})
	}
}

func ciEntrypointFixture(t *testing.T, name string) (string, string) {
	t.Helper()
	root := t.TempDir()
	script := filepath.Join(root, "scripts/ci", name)
	writeExecutable(t, script, string(readFile(t, filepath.Join(repositoryRoot(t), "scripts/ci", name))))
	return root, script
}

func ciRecorderSource(name string) string {
	return `#!/usr/bin/env bash
[[ $PWD -ef "$CI_TEST_ROOT" ]] || exit 92
if [[ ${CI_TEST_LINUX:-} == 1 ]]; then [[ $GOOS == linux && $GOARCH == amd64 && $CGO_ENABLED == 1 ]] || exit 93; fi
printf '` + name + `' >> "$CI_TEST_LOG"
printf '\t%s' "$@" >> "$CI_TEST_LOG"
printf '\n' >> "$CI_TEST_LOG"
count=$(wc -l < "$CI_TEST_LOG")
(( count != ${CI_TEST_FAIL_AT:-0} )) || exit 42
`
}

func ciCommandRecorder(t *testing.T) (string, string) {
	t.Helper()
	bin, log := t.TempDir(), filepath.Join(t.TempDir(), "commands")
	for _, name := range []string{"make", "go"} {
		writeExecutable(t, filepath.Join(bin, name), ciRecorderSource(name))
	}
	return bin, log
}
