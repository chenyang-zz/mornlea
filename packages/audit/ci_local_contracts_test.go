package archcheck_test

import (
	"fmt"
	"os"
	"os/exec"
	"path/filepath"
	"slices"
	"strings"
	"testing"
)

func TestCIDoctorProfilesAndFailures(t *testing.T) {
	root := repositoryRoot(t)
	script := filepath.Join(root, "scripts", "ci", "doctor.sh")
	profiles := map[string][]string{
		"preflight":     {"bash", "git", "go", "gofmt", "node", "npx", "rg"},
		"audit":         {"bash", "go", "gofmt", "rg"},
		"frontend":      {"bash", "corepack", "git", "node"},
		"rust":          {"bash", "cargo", "rustc", "rustup"},
		"go":            {"bash", "go", "gofmt"},
		"native-linux":  {"bash", "cargo", "cc", "go", "ldd", "make", "nm", "readelf", "rustc", "rustup", "shasum"},
		"native-macos":  {"bash", "cargo", "codesign", "go", "install_name_tool", "make", "nm", "rustc", "rustup", "shasum"},
		"agent":         {"bash", "go", "python3", "uv"},
		"godot-static":  {"bash", "make", "rg"},
		"godot-runtime": {"bash", "uname", "cargo", "cc", "clang++", "codesign", "curl", "ditto", "git", "go", "install_name_tool", "make", "nm", "patch", "perl", "pgrep", "rg", "rustc", "rustup", "sandbox-exec", "shasum", "tar", "unzip", "uv", "xcrun"},
	}
	for profile, commands := range profiles {
		t.Run(profile, func(t *testing.T) {
			bin := ciFixtureBin(t, commands)
			output, err := ciRun(root, bin, script, profile)
			if err != nil {
				t.Fatalf("doctor %s failed: %v\n%s", profile, err, output)
			}
			if got, want := output, "CI dependency profile passed: "+profile+"\n"; got != want {
				t.Fatalf("doctor output = %q, want %q", got, want)
			}
			if strings.HasPrefix(profile, "godot-") || profile == "audit" {
				for _, missing := range commands {
					if missing == "bash" {
						continue
					}
					t.Run("missing "+missing, func(t *testing.T) {
						selected := slices.DeleteFunc(slices.Clone(commands), func(command string) bool { return command == missing })
						output, err := ciRun(root, ciFixtureBin(t, selected), script, profile)
						want := "missing required executable for " + profile + ": " + missing + "\n"
						if err == nil || output != want {
							t.Fatalf("missing dependency result: %v\n%s\nwant: %s", err, output, want)
						}
					})
				}
			}
		})
	}

	t.Run("reports every missing command in lexical order", func(t *testing.T) {
		bin := ciFixtureBin(t, []string{"bash", "git", "go", "gofmt", "node"})
		output, err := ciRun(root, bin, script, "preflight")
		if err == nil {
			t.Fatalf("doctor unexpectedly succeeded: %s", output)
		}
		want := "missing required executable for preflight: npx\nmissing required executable for preflight: rg\n"
		if output != want {
			t.Fatalf("missing output = %q, want %q", output, want)
		}
	})

	t.Run("rejects unknown profile", func(t *testing.T) {
		output, err := ciRun(root, ciFixtureBin(t, []string{"bash"}), script, "unknown")
		if exitError, ok := err.(*exec.ExitError); !ok || exitError.ExitCode() != 2 {
			t.Fatalf("unknown profile error = %v, output = %s", err, output)
		}
		if !strings.Contains(output, "usage: doctor.sh <") {
			t.Fatalf("unknown profile output = %q", output)
		}
	})
}

func TestCIPackagePartitionsRejectMutations(t *testing.T) {
	root := repositoryRoot(t)
	script := filepath.Join(root, "scripts", "ci", "check-package-partitions.sh")
	for _, test := range []struct {
		name   string
		mutate func(*ciPartitionFixture)
		want   string
	}{
		{"valid partition", func(*ciPartitionFixture) {}, ""},
		{"duplicate package", func(f *ciPartitionFixture) { f.write("client", "example/a\nexample/a\n") }, "duplicate package in client: example/a"},
		{"client server overlap", func(f *ciPartitionFixture) { f.write("server", "example/b\nexample/c\n") }, "overlapping package: example/b"},
		{"missing package", func(f *ciPartitionFixture) { f.write("rest", "example/d\n") }, "missing package: example/e"},
		{"unexpected package", func(f *ciPartitionFixture) { f.write("rest", "example/d\nexample/e\nexample/z\n") }, "unexpected package: example/z"},
		{"unsorted package", func(f *ciPartitionFixture) { f.write("client", "example/b\nexample/a\n") }, "package list is not lexically sorted in client: example/a"},
		{"empty slice", func(f *ciPartitionFixture) { f.write("rest", "") }, "package list is empty: rest"},
	} {
		t.Run(test.name, func(t *testing.T) {
			fixture := newCIPartitionFixture(t)
			test.mutate(fixture)
			output, err := ciRun(root, os.Getenv("PATH"), script, fixture.paths()...)
			if test.want == "" {
				if err != nil {
					t.Fatalf("valid partition failed: %v\n%s", err, output)
				}
				return
			}
			if err == nil {
				t.Fatalf("mutation unexpectedly succeeded: %s", output)
			}
			if !strings.Contains(output, test.want) {
				t.Fatalf("mutation output = %q, want %q", output, test.want)
			}
		})
	}
}

func TestCIPackagePartitionsAcceptColonPathsAndRejectComparisonFailure(t *testing.T) {
	root := repositoryRoot(t)
	script := filepath.Join(root, "scripts", "ci", "check-package-partitions.sh")
	fixture := newCIPartitionFixture(t)
	colonDir := filepath.Join(fixture.dir, "colon:package-lists")
	if err := os.Mkdir(colonDir, 0o755); err != nil {
		t.Fatal(err)
	}
	colonPaths := make([]string, 0, 4)
	for _, name := range []string{"all", "client", "server", "rest"} {
		path := filepath.Join(colonDir, name+":input")
		contents := string(readFile(t, filepath.Join(fixture.dir, name)))
		writeFile(t, path, []byte(contents))
		colonPaths = append(colonPaths, path)
	}
	if output, err := ciRun(root, os.Getenv("PATH"), script, colonPaths...); err != nil {
		t.Fatalf("colon path partition failed: %v\n%s", err, output)
	}

	bin := t.TempDir()
	writeExecutable(t, filepath.Join(bin, "comm"), "#!/usr/bin/env bash\nexit 42\n")
	output, err := ciRun(root, bin+":"+os.Getenv("PATH"), script, fixture.paths()...)
	if err == nil {
		t.Fatalf("comparison tool failure unexpectedly succeeded: %s", output)
	}
	if !strings.Contains(output, "package partition comparison failed") {
		t.Fatalf("comparison tool failure output = %q", output)
	}
}

func TestCIPackageInventoryRejectsUnexpectedWorkspacePathAndLoadErrors(t *testing.T) {
	root := repositoryRoot(t)
	script := filepath.Join(root, "scripts", "ci", "package-inventory.sh")
	realGo, err := exec.LookPath("go")
	if err != nil {
		t.Fatal(err)
	}
	for _, test := range []struct {
		name   string
		fakeGo string
		want   string
	}{
		{
			name:   "workspace path with spaces",
			fakeGo: fmt.Sprintf("#!/usr/bin/env bash\nif [[ \"${1:-}\" == work && \"${2:-}\" == edit ]]; then\n\tprintf '%%s\\n' '{' '  \"Use\": [' '    {' '      \"DiskPath\": \"./packages/audit\"' '    },' '    {' '      \"DiskPath\": \"./packages/client\"' '    },' '    {' '      \"DiskPath\": \"./packages/contracts\"' '    },' '    {' '      \"DiskPath\": \"./packages/server\"' '    },' '    {' '      \"DiskPath\": \"./packages/shared\"' '    },' '    {' '      \"DiskPath\": \"./packages/tools\"' '    },' '    {' '      \"DiskPath\": \"./packages/extra path\"' '    }' '  ]' '}'\n\texit 0\nfi\nexec %q \"$@\"\n", realGo),
			want:   "unexpected go.work module directories",
		},
		{
			name:   "package loading error",
			fakeGo: fmt.Sprintf("#!/usr/bin/env bash\nif [[ \"${1:-}\" == list ]]; then\n\tif [[ \"$*\" == *'.Error'* ]]; then printf 'example/bad|error\\n'; else printf 'example/bad\\n'; fi\n\texit 0\nfi\nexec %q \"$@\"\n", realGo),
			want:   "package loading error",
		},
	} {
		t.Run(test.name, func(t *testing.T) {
			bin := t.TempDir()
			writeExecutable(t, filepath.Join(bin, "go"), test.fakeGo)
			output, err := ciRun(root, bin+":"+os.Getenv("PATH"), script, "--slice", "server")
			if err == nil {
				t.Fatalf("inventory mutation unexpectedly succeeded: %s", output)
			}
			if !strings.Contains(output, test.want) {
				t.Fatalf("inventory mutation output = %q, want %q", output, test.want)
			}
		})
	}
}

func TestCIRepositoryPackageInventory(t *testing.T) {
	root := repositoryRoot(t)
	script := filepath.Join(root, "scripts", "ci", "package-inventory.sh")
	output, err := ciRun(root, os.Getenv("PATH"), script, "--check")
	if err != nil {
		t.Fatalf("package inventory check failed: %v\n%s", err, output)
	}
	wantModules := []string{"packages/audit", "packages/client", "packages/contracts", "packages/server", "packages/shared", "packages/tools"}
	if got := workspaceModules(t); !slices.Equal(got, wantModules) {
		t.Fatalf("workspace modules = %v, want %v", got, wantModules)
	}
	client := ciPackageInventory(t, root, script, "client")
	rest := ciPackageInventory(t, root, script, "rest")
	gfxspike := "github.com/channing771/mornlea/packages/tools/gfxspike"
	if got := strings.Count(client, gfxspike); got != 1 {
		t.Fatalf("client gfxspike occurrences = %d, want 1: %s", got, client)
	}
	if strings.Contains(rest, gfxspike) {
		t.Fatalf("rest includes Darwin-only gfxspike: %s", rest)
	}
	nativeABI := "github.com/channing771/mornlea/packages/shared/nativeabi"
	if got := strings.Count(rest, nativeABI); got != 1 {
		t.Fatalf("rest nativeabi occurrences = %d, want 1: %s", got, rest)
	}
	inventorySource := string(readFile(t, script))
	for _, required := range []string{"CGO_ENABLED=1", "list_module linux amd64", "list_module darwin arm64"} {
		if !strings.Contains(inventorySource, required) {
			t.Fatalf("inventory is missing supported platform selection %q: %q", required, inventorySource)
		}
	}
	for slice, contents := range map[string]string{"client": client, "server": ciPackageInventory(t, root, script, "server"), "rest": rest} {
		lines := strings.Fields(contents)
		if len(lines) == 0 {
			t.Fatalf("%s package inventory is empty", slice)
		}
		if !slices.IsSorted(lines) {
			t.Fatalf("%s package inventory is not sorted: %v", slice, lines)
		}
	}
}

func TestCIPackageInventoryQueriesEveryModuleOnBothPlatforms(t *testing.T) {
	root := repositoryRoot(t)
	script := filepath.Join(root, "scripts", "ci", "package-inventory.sh")
	realGo, err := exec.LookPath("go")
	if err != nil {
		t.Fatal(err)
	}
	bin := t.TempDir()
	queries := filepath.Join(t.TempDir(), "queries")
	writeExecutable(t, filepath.Join(bin, "go"), fmt.Sprintf("#!/usr/bin/env bash\nif [[ \"${1:-}\" == list ]]; then\n\tprintf '%%s/%%s %%s\\n' \"$GOOS\" \"$GOARCH\" \"$PWD\" >> \"$MORNLEA_CI_QUERY_LOG\"\n\tprintf 'example/%%s/%%s|\\n' \"$GOOS\" \"$GOARCH\"\n\texit 0\nfi\nexec %q \"$@\"\n", realGo))
	output, err := ciRunWithEnv(root, bin+":"+os.Getenv("PATH"), []string{"MORNLEA_CI_QUERY_LOG=" + queries}, script, "--all")
	if err != nil {
		t.Fatalf("matrix inventory failed: %v\n%s", err, output)
	}
	got := strings.Split(strings.TrimSpace(string(readFile(t, queries))), "\n")
	for index := range got {
		got[index] = strings.Replace(got[index], root+"/", "", 1)
	}
	slices.Sort(got)
	want := make([]string, 0, 12)
	for _, platform := range []string{"linux/amd64", "darwin/arm64"} {
		for _, module := range []string{"packages/audit", "packages/client", "packages/contracts", "packages/server", "packages/shared", "packages/tools"} {
			want = append(want, platform+" "+module)
		}
	}
	slices.Sort(want)
	if !slices.Equal(got, want) {
		t.Fatalf("inventory platform/module queries = %v, want %v", got, want)
	}
}

func TestCIRaceEntrypointArguments(t *testing.T) {
	root := repositoryRoot(t)
	script := filepath.Join(root, "scripts", "ci", "run-go-race.sh")
	bin := t.TempDir()
	argv := filepath.Join(t.TempDir(), "argv")
	realGo, err := exec.LookPath("go")
	if err != nil {
		t.Fatal(err)
	}
	writeExecutable(t, filepath.Join(bin, "go"), fmt.Sprintf("#!/usr/bin/env bash\nif [[ \"${1:-}\" == test ]]; then\n\tprintf '%%s\\n' \"$@\" > \"$MORNLEA_CI_GO_ARGV\"\n\texit 0\nfi\nexec %q \"$@\"\n", realGo))
	for _, test := range []struct {
		name        string
		slice       string
		wantPackage string
		wantTail    []string
	}{
		{"client skips bounded server probe", "client", "github.com/channing771/mornlea/packages/tools/gfxspike", []string{"-race", "-p=1", "-skip", "^TestScenarioV7EightSessionServerProbeIsRealAndBounded$"}},
		{"server has no client skip", "server", "github.com/channing771/mornlea/packages/server/server", []string{"-race", "-p=1"}},
		{"rest has no client skip", "rest", "github.com/channing771/mornlea/packages/audit", []string{"-race", "-p=1"}},
	} {
		t.Run(test.name, func(t *testing.T) {
			if err := os.Remove(argv); err != nil && !os.IsNotExist(err) {
				t.Fatal(err)
			}
			output, err := ciRunWithEnv(root, bin+":"+os.Getenv("PATH"), []string{"MORNLEA_CI_GO_ARGV=" + argv}, script, test.slice)
			if err != nil {
				t.Fatalf("race %s failed: %v\n%s", test.slice, err, output)
			}
			got := strings.Fields(string(readFile(t, argv)))
			if len(got) < len(test.wantTail)+2 || got[0] != "test" || !slices.Contains(got, test.wantPackage) || !slices.Equal(got[len(got)-len(test.wantTail):], test.wantTail) {
				t.Fatalf("race argv = %q, want package %q and tail %q", got, test.wantPackage, test.wantTail)
			}
		})
	}
	for _, slice := range []string{"unknown", ""} {
		t.Run(fmt.Sprintf("rejects %q before go test", slice), func(t *testing.T) {
			if err := os.Remove(argv); err != nil && !os.IsNotExist(err) {
				t.Fatal(err)
			}
			_, err := ciRunWithEnv(root, bin+":"+os.Getenv("PATH"), []string{"MORNLEA_CI_GO_ARGV=" + argv}, script, slice)
			if err == nil {
				t.Fatal("invalid race slice unexpectedly succeeded")
			}
			if _, statErr := os.Stat(argv); !os.IsNotExist(statErr) {
				t.Fatalf("invalid race slice invoked go: %v", statErr)
			}
		})
	}
}

func TestCIRaceEntrypointRejectsEmptyInventoryBeforeGoTest(t *testing.T) {
	fixtureRoot := t.TempDir()
	script := filepath.Join(fixtureRoot, "scripts", "ci", "run-go-race.sh")
	writeExecutable(t, script, string(readFile(t, filepath.Join(repositoryRoot(t), "scripts", "ci", "run-go-race.sh"))))
	writeExecutable(t, filepath.Join(fixtureRoot, "scripts", "ci", "package-inventory.sh"), "#!/usr/bin/env bash\nexit 0\n")
	bin := ciFixtureBin(t, []string{"bash"})
	directoryTool, err := exec.LookPath("dirname")
	if err != nil {
		t.Fatal(err)
	}
	if err := os.Symlink(directoryTool, filepath.Join(bin, "dirname")); err != nil {
		t.Fatal(err)
	}
	argv := filepath.Join(t.TempDir(), "argv")
	inventoryCalled := filepath.Join(t.TempDir(), "inventory-called")
	writeExecutable(t, filepath.Join(fixtureRoot, "scripts", "ci", "package-inventory.sh"), "#!/usr/bin/env bash\nprintf called > \"$MORNLEA_CI_INVENTORY_CALLED\"\n")
	bashEnvironment := filepath.Join(t.TempDir(), "mapfile.sh")
	mapfileCalled := filepath.Join(t.TempDir(), "mapfile-called")
	writeFile(t, bashEnvironment, []byte("mapfile() { printf called > \"$MORNLEA_CI_MAPFILE_CALLED\"; eval \"$2=('')\"; }\n"))
	writeExecutable(t, filepath.Join(strings.Split(bin, ":")[0], "go"), "#!/usr/bin/env bash\nprintf '%s\\n' \"$@\" > \"$MORNLEA_CI_GO_ARGV\"\n")
	output, err := ciRunWithEnv(fixtureRoot, bin, []string{"BASH_ENV=" + bashEnvironment, "MORNLEA_CI_GO_ARGV=" + argv, "MORNLEA_CI_INVENTORY_CALLED=" + inventoryCalled, "MORNLEA_CI_MAPFILE_CALLED=" + mapfileCalled}, script, "server")
	if err == nil {
		t.Fatalf("empty inventory unexpectedly succeeded: %s", output)
	}
	if got := string(readFile(t, inventoryCalled)); got != "called" {
		t.Fatalf("empty inventory fixture was not reached: %q", got)
	}
	if _, statErr := os.Stat(mapfileCalled); !os.IsNotExist(statErr) {
		t.Fatalf("empty inventory reached mapfile after the guard: %v", statErr)
	}
	if _, statErr := os.Stat(argv); !os.IsNotExist(statErr) {
		t.Fatalf("empty inventory invoked go test: %v", statErr)
	}
}

func TestCIRaceEntrypointFallbackReader(t *testing.T) {
	fixtureRoot := t.TempDir()
	script := filepath.Join(fixtureRoot, "scripts", "ci", "run-go-race.sh")
	writeExecutable(t, script, string(readFile(t, filepath.Join(repositoryRoot(t), "scripts", "ci", "run-go-race.sh"))))
	writeExecutable(t, filepath.Join(fixtureRoot, "scripts", "ci", "package-inventory.sh"), "#!/usr/bin/env bash\nprintf 'example/server\\n'\n")
	bin := ciFixtureBin(t, []string{"bash"})
	directoryTool, err := exec.LookPath("dirname")
	if err != nil {
		t.Fatal(err)
	}
	if err := os.Symlink(directoryTool, filepath.Join(bin, "dirname")); err != nil {
		t.Fatal(err)
	}
	argv := filepath.Join(t.TempDir(), "argv")
	forcedFallback := filepath.Join(t.TempDir(), "forced-fallback")
	primaryReader := filepath.Join(t.TempDir(), "primary-reader")
	bashEnvironment := filepath.Join(t.TempDir(), "fallback.sh")
	writeFile(t, bashEnvironment, []byte("type() { if [[ \"$1\" == mapfile ]]; then printf forced > \"$MORNLEA_CI_FORCED_FALLBACK\"; return 1; fi; builtin type \"$@\"; }\nmapfile() { printf called > \"$MORNLEA_CI_PRIMARY_READER\"; }\n"))
	writeExecutable(t, filepath.Join(bin, "go"), "#!/usr/bin/env bash\nprintf '%s\\n' \"$@\" > \"$MORNLEA_CI_GO_ARGV\"\n")
	output, err := ciRunWithEnv(fixtureRoot, bin, []string{"BASH_ENV=" + bashEnvironment, "MORNLEA_CI_FORCED_FALLBACK=" + forcedFallback, "MORNLEA_CI_GO_ARGV=" + argv, "MORNLEA_CI_PRIMARY_READER=" + primaryReader}, script, "server")
	if err != nil {
		t.Fatalf("fallback reader failed: %v\n%s", err, output)
	}
	if got := string(readFile(t, forcedFallback)); got != "forced" {
		t.Fatalf("fallback reader was not forced: %q", got)
	}
	if _, statErr := os.Stat(primaryReader); !os.IsNotExist(statErr) {
		t.Fatalf("fallback reader invoked mapfile: %v", statErr)
	}
	if got := strings.Fields(string(readFile(t, argv))); !slices.Equal(got, []string{"test", "example/server", "-race", "-p=1"}) {
		t.Fatalf("fallback reader argv = %q", got)
	}
}

func TestCIPreflightRecipeOrderAndBoundary(t *testing.T) {
	makefile := string(readFile(t, filepath.Join(repositoryRoot(t), "Makefile")))
	recipe := makeTargetRecipe(t, makefile, "ci-preflight")
	ordered := []string{
		"scripts/ci/doctor.sh preflight",
		"gofmt -l",
		"npx --yes @fission-ai/openspec@1.7.0 validate --all --strict --no-interactive",
		"node --test scripts/agent-hooks/guard.test.mjs",
		"$(MAKE) comment-language-check",
		"scripts/ci/package-inventory.sh --check",
		"$(GO) test ./packages/audit -skip '^TestGodotAssetSyncIsDeterministicAndRejectsManualFiles$$' -count=1",
	}
	last := -1
	for _, command := range ordered {
		index := strings.Index(recipe, command)
		if index < 0 {
			t.Errorf("ci-preflight is missing %q", command)
			continue
		}
		if index <= last {
			t.Errorf("ci-preflight command order is wrong around %q: %q", command, recipe)
		}
		last = index
	}
	auditCommand := ordered[len(ordered)-1]
	auditInvocations := 0
	for _, line := range strings.Split(recipe, "\n") {
		if !strings.HasPrefix(strings.TrimSpace(line), "$(GO) test ./packages/audit ") {
			continue
		}
		auditInvocations++
		if strings.TrimSpace(line) != auditCommand {
			t.Errorf("ci-preflight audit command = %q, want %q", strings.TrimSpace(line), auditCommand)
		}
	}
	if auditInvocations != 1 {
		t.Errorf("ci-preflight audit invocations = %d, want 1", auditInvocations)
	}
	for _, forbidden := range []string{"cargo", "make rust", "package-native-artifact", "verify-native-artifact", "scripts/godot", "godot-"} {
		if strings.Contains(recipe, forbidden) {
			t.Errorf("ci-preflight must not invoke %q: %q", forbidden, recipe)
		}
	}
}

func TestCIPreflightStopsBeforeNextCommandWhenGofmtFails(t *testing.T) {
	root := repositoryRoot(t)
	bin := ciFixtureBin(t, []string{"bash", "git", "go", "gofmt", "node", "npx", "rg"})
	binDir := strings.Split(bin, ":")[0]
	next := filepath.Join(t.TempDir(), "next-command-ran")
	writeExecutable(t, filepath.Join(binDir, "git"), "#!/usr/bin/env bash\nprintf 'name with spaces.go\\000'\n")
	writeExecutable(t, filepath.Join(binDir, "gofmt"), "#!/usr/bin/env bash\nexit 42\n")
	writeExecutable(t, filepath.Join(binDir, "npx"), "#!/usr/bin/env bash\nprintf x > \"$MORNLEA_CI_NEXT\"\n")
	command := exec.Command("make", "-f", "Makefile", "ci-preflight")
	command.Dir = root
	command.Env = ciEnvironment(bin, []string{"MORNLEA_CI_NEXT=" + next})
	output, err := command.CombinedOutput()
	if err == nil {
		t.Fatalf("gofmt failure unexpectedly succeeded: %s", output)
	}
	if _, statErr := os.Stat(next); !os.IsNotExist(statErr) {
		t.Fatalf("preflight ran its next command after gofmt failure: %v", statErr)
	}
}

func ciFixtureBin(t *testing.T, commands []string) string {
	t.Helper()
	bin := t.TempDir()
	for _, command := range commands {
		if command == "bash" {
			continue
		}
		writeExecutable(t, filepath.Join(bin, command), "#!/usr/bin/env bash\nexit 0\n")
	}
	bash, err := exec.LookPath("bash")
	if err != nil {
		t.Fatal(err)
	}
	if err := os.Symlink(bash, filepath.Join(bin, "bash")); err != nil {
		t.Fatal(err)
	}
	return bin
}

func ciRun(root, path, script string, arguments ...string) (string, error) {
	return ciRunWithEnv(root, path, nil, script, arguments...)
}

func ciRunWithEnv(root, path string, extraEnvironment []string, script string, arguments ...string) (string, error) {
	command := exec.Command(script, arguments...)
	command.Dir = root
	command.Env = ciEnvironment(path, extraEnvironment)
	output, err := command.CombinedOutput()
	return string(output), err
}

func ciEnvironment(path string, extraEnvironment []string) []string {
	environment := make([]string, 0, len(os.Environ())+len(extraEnvironment)+1)
	for _, variable := range os.Environ() {
		if !strings.HasPrefix(variable, "PATH=") {
			environment = append(environment, variable)
		}
	}
	environment = append(environment, "PATH="+path)
	return append(environment, extraEnvironment...)
}

func ciPackageInventory(t *testing.T, root, script, slice string) string {
	t.Helper()
	output, err := ciRun(root, os.Getenv("PATH"), script, "--slice", slice)
	if err != nil {
		t.Fatalf("package inventory %s failed: %v\n%s", slice, err, output)
	}
	return output
}

type ciPartitionFixture struct {
	dir string
	t   *testing.T
}

func newCIPartitionFixture(t *testing.T) *ciPartitionFixture {
	t.Helper()
	fixture := &ciPartitionFixture{dir: t.TempDir(), t: t}
	fixture.write("all", "example/a\nexample/b\nexample/c\nexample/d\nexample/e\n")
	fixture.write("client", "example/a\nexample/b\n")
	fixture.write("server", "example/c\n")
	fixture.write("rest", "example/d\nexample/e\n")
	return fixture
}

func (f *ciPartitionFixture) write(name, contents string) {
	writeFile(f.t, filepath.Join(f.dir, name), []byte(contents))
}

func (f *ciPartitionFixture) paths() []string {
	return []string{filepath.Join(f.dir, "all"), filepath.Join(f.dir, "client"), filepath.Join(f.dir, "server"), filepath.Join(f.dir, "rest")}
}
