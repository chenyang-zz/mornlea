package archcheck_test

import (
	"fmt"
	"os"
	"os/exec"
	"path/filepath"
	"reflect"
	"regexp"
	"slices"
	"strings"
	"testing"

	"gopkg.in/yaml.v3"
)

// The Godot pilot entry points are opt-in gates: they may touch the
// Godot/Python runtime only when a developer or the dedicated CI job invokes
// them explicitly. This guard pins both halves of that boundary: the entry
// points must exist with the exact verified script invocations, and every
// legacy make/CI entry (build, test, run, companion-agent-check) must stay
// free of any scripts/godot probe or godot-* dependency edge.
var godotEntrypointTargets = []string{
	"godot-export-linux",
	"godot-build",
	"godot-check",
	"godot-asset-check",
	"godot-project-check",
	"godot-python-check",
	"godot-input-check",
	"godot-camera-check",
	"godot-target-check",
	"godot-entity-check",
	"godot-environment-check",
	"godot-hud-check",
	"godot-disconnect-check",
	"godot-smoke",
	"godot-terrain-check",
	"godot-capability-check",
	"godot-playable-smoke",
	"godot-visual-evidence",
	"godot-visual-compare",
	"godot-benchmark",
}

// godotEntrypointRecipes pins the exact script invocation each gate must run;
// godot-smoke additionally requires the isolated Python flag so the smoke
// cycle can never silently fall back to a host interpreter.
var godotEntrypointRecipes = map[string][]string{
	"godot-export-linux": {"scripts/godot/export-linux.sh --verify"},
	"godot-build": {
		"scripts/godot/build-python-runtime.sh --verify --offline",
		"scripts/godot/build-core.sh --profile debug --verify",
		"scripts/godot/build-core.sh --profile release --verify",
		"scripts/godot/build-extension.sh --profile debug",
		"scripts/godot/build-extension.sh --profile release --verify",
	}, "godot-check": {"scripts/godot/godot.sh --headless --path apps/mornlea-godot --editor --quit"},
	"godot-asset-check":   {"scripts/godot/sync-assets.sh --check"},
	"godot-project-check": {"scripts/godot/validate-project.sh"},
	"godot-python-check":  {"scripts/godot/python-check.sh --locked"},
	"godot-input-check":   {"scripts/godot/input-check.sh"},
	"godot-camera-check":  {"scripts/godot/camera-check.sh"},
	"godot-target-check":  {"scripts/godot/target-check.sh"},
	"godot-entity-check": {
		"$(CARGO) test -p mornlea_godot entity_snapshot --locked",
		"scripts/godot/entity-check.sh",
	},
	"godot-environment-check": {"scripts/godot/environment-check.sh"},
	"godot-hud-check":         {"scripts/godot/hud-check.sh"},
	"godot-disconnect-check":  {"go test ./packages/client/runtime", "Disconnect|Overflow|Shutdown"},
	"godot-smoke":             {"scripts/godot/smoke.sh --iterations 100", "--isolated-python"},
	"godot-terrain-check":     {"scripts/godot/godot-terrain-check.sh"},
	"godot-capability-check":  {"scripts/godot/capability-check.sh"},
	"godot-playable-smoke":    {"scripts/godot/playable-smoke.sh --duration 300s"},
	"godot-visual-evidence":   {"scripts/godot/capture.sh"},
	"godot-visual-compare":    {"scripts/godot/visual-compare.sh"},
	"godot-benchmark":         {"scripts/godot/benchmark.sh"},
}

const godotEntrypointTestRootEnv = "MORNLEA_GODOT_ENTRYPOINT_TEST_ROOT"

func TestGodotIsOptionalForLegacyBuild(t *testing.T) {
	// Mutation re-runs redirect all baseline reads to a temporary root, so
	// each probe below verifies that this guard rejects one isolated
	// regression instead of re-reading the real repository.
	root := os.Getenv(godotEntrypointTestRootEnv)
	mutationRun := root != ""
	if !mutationRun {
		root = repositoryRoot(t)
	}
	makefile := readBaselineDoc(t, root, "Makefile")
	workflow := readBaselineDoc(t, root, filepath.Join(".github", "workflows", "ci.yml"))
	optional := readBaselineDoc(t, root, filepath.Join(".github", "workflows", "godot.yml"))
	if violations := godotMakefileEntrypointViolations(t, makefile); len(violations) > 0 {
		t.Fatalf("Godot Makefile entry-point contract has %d violations:\n%s", len(violations), strings.Join(violations, "\n"))
	}
	if violations := godotWorkflowEntrypointViolations(workflow, optional); len(violations) > 0 {
		t.Fatalf("Godot CI entry-point contract has %d violations:\n%s", len(violations), strings.Join(violations, "\n"))
	}
	if !mutationRun {
		testGodotEntrypointMutations(t, makefile, workflow, optional)
	}
}

func godotMakefileEntrypointViolations(t *testing.T, makefile string) []string {
	t.Helper()
	var violations []string
	rules := parseMakeRules(makefile)
	declared := make(map[string]bool)
	for _, rule := range rules {
		for _, target := range rule.targets {
			declared[target] = true
		}
	}
	for _, target := range godotEntrypointTargets {
		if !declared[target] {
			violations = append(violations, fmt.Sprintf("Makefile is missing the optional godot gate %s", target))
			continue
		}
		if !makeTargetIsPhony(makefile, target) {
			violations = append(violations, fmt.Sprintf("Makefile .PHONY does not register %s", target))
		}
		if !strings.Contains(makefile, "make "+target) {
			violations = append(violations, fmt.Sprintf("make help does not list %s", target))
		}
		recipe := makeTargetRecipe(t, makefile, target)
		if target == "godot-build" {
			lines := strings.Split(strings.TrimSpace(recipe), "\n")
			for index := range lines {
				lines[index] = strings.TrimSpace(lines[index])
			}
			if !reflect.DeepEqual(lines, godotEntrypointRecipes[target]) {
				violations = append(violations, fmt.Sprintf("godot-build recipe order = %q, want %q", lines, godotEntrypointRecipes[target]))
			}
		}
		for _, required := range godotEntrypointRecipes[target] {
			if !strings.Contains(recipe, required) {
				violations = append(violations, fmt.Sprintf("godot gate %s must invoke %q", target, required))
			}
		}
	}

	godotGate := make(map[string]bool, len(godotEntrypointTargets))
	for _, target := range godotEntrypointTargets {
		godotGate[target] = true
	}
	for _, rule := range rules {
		for _, target := range rule.targets {
			if strings.HasPrefix(target, ".") {
				// Special make targets such as .PHONY register bookkeeping
				// metadata, not build-order edges; the mandatory .PHONY
				// registration of the godot gates must not be read as a
				// dependency.
				continue
			}
			for _, prerequisite := range rule.prerequisites {
				if strings.HasPrefix(prerequisite, "godot-") {
					if godotGate[target] {
						violations = append(violations, fmt.Sprintf("godot gate %s must stay a leaf target, but depends on %s", target, prerequisite))
					} else {
						violations = append(violations, fmt.Sprintf("legacy target %s must not depend on the optional godot gate %s", target, prerequisite))
					}
				}
			}
			if godotGate[target] {
				continue
			}
			for _, recipeLine := range rule.recipe {
				if strings.Contains(recipeLine, "scripts/godot") {
					violations = append(violations, fmt.Sprintf("legacy target %s probes the Godot runtime in its recipe: %s", target, strings.TrimSpace(recipeLine)))
				}
			}
		}
	}
	violations = append(violations, godotMakefileScriptReferenceViolations(makefile)...)
	slices.Sort(violations)
	return violations
}

// godotMakefileScriptReferenceViolations is a whole-file invariant over every
// non-comment Makefile line: the substring scripts/godot may appear only in a
// rule line of exactly one registered godot gate target or in the
// tab-indented recipe lines directly following such a rule. It deliberately
// stays grammar-light instead of teaching the rule parser more make syntax —
// double-colon rules, inline semicolon recipes, and especially parse-time
// $(shell ...) assignments (which execute on every make invocation) must not
// open a silent probe path into the Godot runtime.
func godotMakefileScriptReferenceViolations(makefile string) []string {
	godotGate := make(map[string]bool, len(godotEntrypointTargets))
	for _, target := range godotEntrypointTargets {
		godotGate[target] = true
	}
	var violations []string
	inGodotRecipe := false
	for _, line := range strings.Split(makefile, "\n") {
		if strings.HasPrefix(strings.TrimSpace(line), "#") {
			// Comment lines are inert prose and never terminate a recipe.
			continue
		}
		if strings.HasPrefix(line, "\t") {
			if !inGodotRecipe && strings.Contains(line, "scripts/godot") {
				violations = append(violations, fmt.Sprintf("tab recipe line outside the godot gate recipes references scripts/godot: %s", strings.TrimSpace(line)))
			}
			continue
		}
		if strings.TrimSpace(line) == "" {
			// Blank lines do not terminate a recipe, mirroring
			// `makeTargetRecipe`.
			continue
		}
		targets, _, ok := splitMakeRuleLine(line)
		inGodotRecipe = ok && len(targets) == 1 && godotGate[targets[0]]
		if !inGodotRecipe && strings.Contains(line, "scripts/godot") {
			violations = append(violations, fmt.Sprintf("Makefile line references scripts/godot outside the registered godot gate rules: %s", strings.TrimSpace(line)))
		}
	}
	return violations
}

type makeRule struct {
	targets       []string
	prerequisites []string
	recipe        []string
}

// parseMakeRules extracts one entry per rule line: the target names (a rule
// may declare several targets), the raw prerequisite tokens, and the
// tab-indented recipe that follows it. Variable assignments ("name := value")
// are skipped. Recipe scanning mirrors `makeTargetRecipe`: blank and comment
// lines do not terminate a recipe.
func parseMakeRules(makefile string) []makeRule {
	lines := strings.Split(makefile, "\n")
	var rules []makeRule
	for index := 0; index < len(lines); index++ {
		line := lines[index]
		targets, prerequisites, ok := splitMakeRuleLine(line)
		if !ok {
			continue
		}
		rule := makeRule{targets: targets, prerequisites: prerequisites}
		next := index + 1
		for next < len(lines) {
			candidate := lines[next]
			if strings.HasPrefix(candidate, "\t") {
				rule.recipe = append(rule.recipe, candidate)
				next++
				continue
			}
			if candidate == "" || strings.HasPrefix(candidate, "#") {
				next++
				continue
			}
			break
		}
		rules = append(rules, rule)
		index = next - 1
	}
	return rules
}

func splitMakeRuleLine(line string) ([]string, []string, bool) {
	if line == "" || strings.HasPrefix(line, "\t") || strings.HasPrefix(line, "#") {
		return nil, nil, false
	}
	colon := strings.Index(line, ":")
	if colon <= 0 {
		return nil, nil, false
	}
	if colon+1 < len(line) && (line[colon+1] == '=' || line[colon+1] == ':') {
		return nil, nil, false
	}
	targets := strings.Fields(line[:colon])
	if len(targets) == 0 {
		return nil, nil, false
	}
	return targets, strings.Fields(line[colon+1:]), true
}

var workflowJobHeader = regexp.MustCompile(`^ {2}[A-Za-z0-9][A-Za-z0-9_-]*:$`)

// parseWorkflowJobs splits the workflow into top-level job blocks keyed by job
// name. A job header is a two-space-indented "name:" line under `jobs:`; the
// block extends to the next job header or end of file. Only lines after the
// top-level `jobs:` key participate, so sibling keys such as `on: push:` are
// never mistaken for jobs.
func parseWorkflowJobs(workflow string) map[string]string {
	jobs := make(map[string]string)
	current := ""
	var block []string
	flush := func() {
		if current != "" {
			jobs[current] = strings.Join(block, "\n")
		}
	}
	inJobs := false
	for _, line := range strings.Split(workflow, "\n") {
		if !inJobs {
			inJobs = line == "jobs:"
			continue
		}
		if workflowJobHeader.MatchString(line) {
			flush()
			current = strings.TrimSpace(strings.TrimSuffix(line, ":"))
			block = nil
			continue
		}
		if current != "" {
			block = append(block, line)
		}
	}
	flush()
	return jobs
}

var godotWorkflowPaths = []string{
	".github/workflows/godot.yml",
	"Makefile",
	"go.work",
	"go.work.sum",
	"apps/mornlea-godot/**",
	"packages/audit/godot_*_test.go",
	"packages/audit/ci_workflow_standard_test.go",
	"packages/audit/go.mod",
	"packages/audit/go.sum",
	"packages/client/assets/**",
	"packages/client/client/**",
	"packages/client/cmd/mornlea-godot-assets/**",
	"packages/client/cmd/mornlea-godot-core/**",
	"packages/client/go.mod",
	"packages/client/go.sum",
	"packages/client/mesh/**",
	"packages/client/presentation/**",
	"packages/client/render/**",
	"packages/client/runtime/**",
	"packages/contracts/**",
	"packages/engine/**",
	"packages/server/go.mod",
	"packages/server/go.sum",
	"packages/shared/**",
	"packages/tools/go.mod",
	"packages/tools/go.sum",
	"scripts/ci/**",
	"scripts/engine/**",
	"scripts/godot/**",
}

const godotGoCache = "go.work.sum\n" + ciGoCache

const godotRustActivation = "cd packages/engine\nrustup show active-toolchain\nrustc --version\ncargo --version"

const godotModulePrefetch = `for module in packages/contracts packages/shared packages/server packages/client packages/tools packages/audit; do
  (cd "$module" && GOWORK=off go mod download)
done`

// Workflow decoding checks executable fields rather than comments or formatting.
// Exact step ordering prevents setup, retry, and artifact paths from hiding gates.
func godotWorkflowEntrypointViolations(required, optional string) []string {
	var violations []string
	require := func(ok bool, message string) {
		if !ok {
			violations = append(violations, message)
		}
	}
	var requiredTree yaml.Node
	if err := yaml.Unmarshal([]byte(required), &requiredTree); err != nil {
		violations = append(violations, "invalid required workflow: "+err.Error())
	} else {
		var inspect func(*yaml.Node)
		inspect = func(node *yaml.Node) {
			if node.Kind == yaml.ScalarNode {
				require(!strings.Contains(strings.ToLower(node.Value), "godot"), "required CI merge gate must stay independent of Godot: "+node.Value)
			}
			for _, child := range node.Content {
				inspect(child)
			}
		}
		inspect(&requiredTree)
	}
	var workflow companionWorkflow
	decoder := yaml.NewDecoder(strings.NewReader(optional))
	decoder.KnownFields(true)
	if err := decoder.Decode(&workflow); err != nil {
		return append(violations, "invalid optional workflow: "+err.Error())
	}
	var tree yaml.Node
	if err := yaml.Unmarshal([]byte(optional), &tree); err != nil {
		return append(violations, err.Error())
	}
	var inspect func(*yaml.Node)
	inspect = func(node *yaml.Node) {
		if node.Kind == yaml.MappingNode {
			for i := 0; i < len(node.Content); i += 2 {
				key, value := node.Content[i], node.Content[i+1]
				if key.Value == "uses" {
					tag, ok := ciActions[value.Value]
					require(ok && strings.TrimSpace(value.LineComment) == "# "+tag, "action requires reviewed immutable revision and major comment: "+value.Value)
				}
			}
		}
		for _, child := range node.Content {
			inspect(child)
		}
	}
	inspect(&tree)
	require(workflow.Name == "Godot CI", "optional workflow name must be Godot CI")
	require(len(workflow.On) == 3, "optional events must be push, pull_request, and workflow_dispatch")
	for _, name := range []string{"push", "pull_request"} {
		event, ok := workflow.On[name]
		var filters map[string][]string
		err := event.Decode(&filters)
		wantKeys := 1
		if name == "push" {
			wantKeys = 2
			require(slices.Equal(filters["branches"], []string{"main"}), "optional push must select main")
		}
		paths := slices.Clone(filters["paths"])
		wantPaths := slices.Clone(godotWorkflowPaths)
		slices.Sort(paths)
		slices.Sort(wantPaths)
		require(ok && err == nil && len(filters) == wantKeys && slices.Equal(paths, wantPaths), name+": optional path filters must cover the exact Godot input set")
	}
	dispatch, ok := workflow.On["workflow_dispatch"]
	require(ok && dispatch.Tag == "!!null", "optional workflow must support manual dispatch")
	require(workflow.Concurrency.Group == "godot-ci-${{ github.event.pull_request.number || github.ref }}" && workflow.Concurrency.Cancel, "optional concurrency must cancel stale candidates")
	require(reflect.DeepEqual(workflow.Permissions, map[string]string{"contents": "read"}), "optional permissions must be contents read")
	require(len(workflow.Env) == 0, "optional workflow must not override gate environments")
	require(len(workflow.Jobs) == 2, "optional workflow must have exactly two jobs without merge authority")
	for name, want := range map[string]requiredCIJob{
		"godot-static":  {"ubuntu-24.04", 20, nil, []string{ciCheckout}},
		"godot-runtime": {"macos-26", 90, []string{"godot-static"}, []string{ciCheckout, ciGo, ciUV}},
	} {
		job, ok := workflow.Jobs[name]
		require(ok, "missing optional job: "+name)
		require(job.RunsOn.Value == want.runner && job.Timeout == want.timeout, name+": runner and timeout must match qualification environment")
		require(slices.Equal([]string(job.Needs), want.needs), name+": dependency must remain inside the optional workflow")
		require(job.If.Value == "" && len(job.Permissions) == 0, name+": job cannot override execution or permissions")
		wantOrder := append([]string{ciStart}, want.setups...)
		if name == "godot-static" {
			require(len(job.Env) == 0, name+": static job cannot override its environment")
			wantOrder = append(wantOrder, "sudo apt-get update\nsudo apt-get install --yes ripgrep", "scripts/ci/doctor.sh godot-static", "make godot-project-check")
		} else {
			require(reflect.DeepEqual(job.Env, map[string]string{"DEVELOPER_DIR": "/Applications/Xcode_26.5.app/Contents/Developer"}), name+": runtime must select the qualified Xcode environment")
			wantOrder = append(wantOrder, "brew install ripgrep", godotRustActivation, "scripts/ci/doctor.sh godot-runtime", godotModulePrefetch, "make rust", "make godot-asset-check", "scripts/godot/fetch.sh", "scripts/godot/build-python-runtime.sh --verify", "make godot-build", "make godot-python-check", "make godot-smoke", "scripts/godot/python-runtime-check.sh --exported --offline")
		}
		wantOrder = append(wantOrder, ciSummary(name))
		var order []string
		for _, step := range job.Steps {
			if step.Uses != "" {
				order = append(order, step.Uses)
				require(step.Run == "" && step.If.Value == "", name+": setup must run unconditionally")
				switch step.Uses {
				case ciCheckout:
					require(len(step.With) == 0, name+": checkout must use the candidate defaults")
				case ciGo:
					value, err := workflowStringWith(step, "go-version")
					require(len(step.With) == 2 && err == nil && value == "1.26", name+": Go setup must pin 1.26")
					value, err = workflowStringWith(step, "cache-dependency-path")
					require(err == nil && slices.Equal(nonEmptyTrimmedLines(value), nonEmptyTrimmedLines(godotGoCache)), name+": Go cache must cover workspace, module, and native ABI inputs")
				case ciUV:
					value, err := workflowStringWith(step, "version")
					require(len(step.With) == 1 && err == nil && value == "0.12.5", name+": uv setup must pin 0.12.5")
				default:
					require(false, name+": unexpected action or required artifact transfer: "+step.Uses)
				}
			} else {
				command := strings.TrimSpace(step.Run)
				order = append(order, command)
				require(len(step.With) == 0, name+": commands cannot have action inputs")
				if command == ciSummary(name) {
					require(step.If.Value == "${{ always() }}", name+": duration and runner summary must always run")
				} else {
					require(step.If.Value == "", name+": validation cannot be conditional")
				}
			}
		}
		require(slices.Equal(order, wantOrder), name+": setup and gates must run in order without retries, artifacts, or inline validation")
	}
	slices.Sort(violations)
	return violations
}

func testGodotEntrypointMutations(t *testing.T, makefile, workflow, optional string) {
	t.Helper()
	type mutationCase struct {
		name string
		// Each mutator returns the mutated copy or fails the subtest when its
		// anchor drifted, so a silent no-op mutation can never look like a
		// rejection.
		mutateMakefile func(*testing.T, string) string
		mutateWorkflow func(*testing.T, string) string
		mutateOptional func(*testing.T, string) string
	}
	mutations := []mutationCase{
		{
			name: "legacy build recipe probes the godot runtime",
			mutateMakefile: func(t *testing.T, source string) string {
				return mutateGodotAnchorOnce(t, source,
					"\tcp $(PIXEL_PERFECTION_NOTICE_DIR)/PROVENANCE.json $(PIXEL_PERFECTION_NOTICE_DEST)/PROVENANCE.json\n",
					"\tcp $(PIXEL_PERFECTION_NOTICE_DIR)/PROVENANCE.json $(PIXEL_PERFECTION_NOTICE_DEST)/PROVENANCE.json\n\tscripts/godot/fetch.sh\n")
			},
		},
		{
			name: "legacy test target depends on a godot gate",
			mutateMakefile: func(t *testing.T, source string) string {
				return mutateGodotAnchorOnce(t, source, "\ntest:\n", "\ntest: godot-project-check\n")
			},
		},
		{
			name: "godot smoke loses the isolated python flag",
			mutateMakefile: func(t *testing.T, source string) string {
				return mutateGodotAnchorOnce(t, source,
					"scripts/godot/smoke.sh --iterations 100 --isolated-python",
					"scripts/godot/smoke.sh --iterations 100")
			},
		},
		{
			name: "double-colon legacy rule runs a godot script",
			mutateMakefile: func(t *testing.T, source string) string {
				return mutateGodotAnchorOnce(t, source,
					"\ngodot-smoke:\n\tscripts/godot/smoke.sh --iterations 100 --isolated-python\n",
					"\ngodot-smoke:\n\tscripts/godot/smoke.sh --iterations 100 --isolated-python\n\nbuild::\n\tscripts/godot/fetch.sh\n")
			},
		},
		{
			name: "make parse-time assignment probes the godot runtime",
			mutateMakefile: func(t *testing.T, source string) string {
				return mutateGodotAnchorOnce(t, source,
					"\ngodot-smoke:\n\tscripts/godot/smoke.sh --iterations 100 --isolated-python\n",
					"\ngodot-smoke:\n\tscripts/godot/smoke.sh --iterations 100 --isolated-python\n\nGODOT_PROBE := $(shell scripts/godot/fetch.sh)\n")
			},
		},
		{
			name: "godot smoke reduces the lifecycle count",
			mutateMakefile: func(t *testing.T, source string) string {
				return mutateGodotAnchorOnce(t, source, "scripts/godot/smoke.sh --iterations 100 --isolated-python", "scripts/godot/smoke.sh --iterations 1 --isolated-python")
			},
		},
	}
	for _, mutation := range []struct{ name, old, replacement string }{
		{"smoke moved into required CI", "run: make ci-preflight", "run: make godot-smoke"},
		{"required script probe", "run: make ci-preflight", "run: scripts/godot/fetch.sh"},
		{"required Godot job", "jobs:\n", "jobs:\n  godot:\n    runs-on: ubuntu-24.04\n    steps: []\n"},
		{"block-style merge dependency", "needs: [preflight, frontend, rust-quality, native-linux, native-macos, linux-quality, race-server, race-rest, race-client, integration-server, integration-client]", "needs:\n      - preflight\n      - godot-static"},
		{"required path-filter indirection", "  pull_request:\n", "  pull_request:\n    paths: [apps/mornlea-godot/**]\n"},
	} {
		mutations = append(mutations, mutationCase{name: mutation.name, mutateWorkflow: func(t *testing.T, source string) string {
			return mutateGodotAnchorOnce(t, source, mutation.old, mutation.replacement)
		}})
	}
	for _, mutation := range []struct{ name, old, replacement string }{
		{"deleted path", "      - 'packages/shared/**'", ""},
		{"deleted workspace definition", "      - 'go.work'", ""},
		{"deleted workspace checksums", "      - 'go.work.sum'", ""},
		{"deleted client dependencies", "      - 'packages/client/go.mod'", ""},
		{"deleted client core dependency", "      - 'packages/client/client/**'", ""},
		{"deleted client renderer dependency", "      - 'packages/client/render/**'", ""},
		{"deleted client checksums", "      - 'packages/client/go.sum'", ""},
		{"missing manual dispatch", "  workflow_dispatch:\n", ""},
		{"moving runner", "runs-on: macos-26", "runs-on: macos-latest"},
		{"wrong Xcode", "DEVELOPER_DIR: /Applications/Xcode_26.5.app/Contents/Developer", "DEVELOPER_DIR: /Applications/Xcode.app/Contents/Developer"},
		{"missing Xcode", "      DEVELOPER_DIR: /Applications/Xcode_26.5.app/Contents/Developer", ""},
		{"missing module prefetch", "packages/tools packages/audit; do", "packages/tools; do"},
		{"prefetch mutates workspace", "GOWORK=off go mod download", "go mod download"},
		{"missing native build", "run: make rust", "run: true"},
		{"missing Rust activation", "      - name: Activate repository-pinned Rust toolchain\n        run: |\n          cd packages/engine\n          rustup show active-toolchain\n          rustc --version\n          cargo --version\n", ""},
		{"missing workspace cache input", "            go.work.sum\n", ""},
		{"allow failure", "    timeout-minutes: 90", "    timeout-minutes: 90\n    continue-on-error: true"},
		{"optional needs merge gate", "needs: godot-static", "needs: [godot-static, merge-gate]"},
		{"optional produces merge gate", "  godot-runtime:\n", "  merge-gate:\n"},
		{"omitted asset gate", "run: make godot-asset-check", "run: true"},
		{"omitted Python gate", "run: make godot-python-check", "run: true"},
		{"omitted client core build", "run: make godot-build", "run: true"},
		{"omitted exported runtime", "run: scripts/godot/python-runtime-check.sh --exported --offline", "run: true"},
		{"Python checked before runtime materialization", "run: scripts/godot/build-python-runtime.sh --verify", "run: make godot-python-check"},
		{"automatic retry", "run: make godot-smoke", "run: make godot-smoke || make godot-smoke"},
		{"mutable action", ciCheckout, "actions/checkout@v5"},
		{"required artifact consumer", "run: make godot-smoke", "uses: " + ciDownload + " # v5"},
		{"wrong uv", "version: '0.12.5'", "version: '0.12.4'"},
		{"runtime doctor before provisioning", "run: brew install ripgrep", "run: scripts/ci/doctor.sh godot-runtime"},
		{"skipped gate", "      - name: Run lifecycle smoke\n", "      - name: Run lifecycle smoke\n        if: ${{ false }}\n"},
		{"missing duration evidence", "godot-runtime seconds:", "elapsed:"},
	} {
		mutations = append(mutations, mutationCase{name: mutation.name, mutateOptional: func(t *testing.T, source string) string {
			t.Helper()
			if !strings.Contains(source, mutation.old) {
				t.Fatal("mutation anchor missing: " + mutation.old)
			}
			return strings.Replace(source, mutation.old, mutation.replacement, 1)
		}})
	}
	for _, mutation := range mutations {
		t.Run(mutation.name, func(t *testing.T) {
			root := t.TempDir()
			mutatedMakefile, mutatedWorkflow, mutatedOptional := makefile, workflow, optional
			if mutation.mutateMakefile != nil {
				mutatedMakefile = mutation.mutateMakefile(t, mutatedMakefile)
			}
			if mutation.mutateWorkflow != nil {
				mutatedWorkflow = mutation.mutateWorkflow(t, mutatedWorkflow)
			}
			if mutation.mutateOptional != nil {
				mutatedOptional = mutation.mutateOptional(t, mutatedOptional)
			}
			writeGodotEntrypointMutationFile(t, root, "Makefile", mutatedMakefile)
			writeGodotEntrypointMutationFile(t, root, filepath.Join(".github", "workflows", "ci.yml"), mutatedWorkflow)
			writeGodotEntrypointMutationFile(t, root, filepath.Join(".github", "workflows", "godot.yml"), mutatedOptional)
			command := exec.Command(os.Args[0], "-test.run=^TestGodotIsOptionalForLegacyBuild$")
			command.Env = append(os.Environ(), godotEntrypointTestRootEnv+"="+root)
			output, err := command.CombinedOutput()
			if err == nil {
				t.Errorf("godot entry-point guard accepted the mutation\n%s", output)
			}
		})
	}
}

func mutateGodotAnchorOnce(t *testing.T, source, old, replacement string) string {
	t.Helper()
	if count := strings.Count(source, old); count != 1 {
		t.Fatalf("mutation anchor must occur exactly once, got %d: %q", count, old)
	}
	return strings.Replace(source, old, replacement, 1)
}

func writeGodotEntrypointMutationFile(t *testing.T, root, relative, source string) {
	t.Helper()
	path := filepath.Join(root, relative)
	if err := os.MkdirAll(filepath.Dir(path), 0o700); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(path, []byte(source), 0o600); err != nil {
		t.Fatal(err)
	}
}

func TestGodotRollbackRejectsMissingOptionalWorkflow(t *testing.T) {
	root := godotRollbackFixture(t)
	command := exec.Command("bash", filepath.Join(root, "scripts/godot/rollback-check.sh"))
	output, err := command.CombinedOutput()
	if err == nil || !strings.Contains(string(output), "optional Godot workflow is missing") || strings.Contains(string(output), "check passed") {
		t.Fatalf("rollback accepted a missing optional workflow or lost its diagnostic: %v\n%s", err, output)
	}
}

func TestGodotRollbackWorkflowIsolation(t *testing.T) {
	root := repositoryRoot(t)
	required := readBaselineDoc(t, root, ".github/workflows/ci.yml")
	optional := readBaselineDoc(t, root, ".github/workflows/godot.yml")
	for _, test := range []struct {
		name, file, old, replacement string
		wantSuccess                  bool
	}{
		{name: "baseline", wantSuccess: true},
		{"inert required comment", "ci.yml", "jobs:\n", "jobs:\n  # godot remains optional\n", true},
		{"required job", "ci.yml", "  preflight:\n", "  godot-preflight:\n", false},
		{"required direct probe", "ci.yml", "run: make ci-preflight", "run: scripts/godot/fetch.sh", false},
		{"required block need", "ci.yml", "  preflight:\n", "  preflight:\n    needs:\n      - godot-static\n", false},
		{"missing optional gate", "godot.yml", "        run: make godot-smoke", "        # run: make godot-smoke", false},
		{"optional merge dependency", "godot.yml", "needs: godot-static", "needs: [godot-static, merge-gate]", false},
		{"optional merge job", "godot.yml", "  godot-runtime:", "  merge-gate:", false},
		{"optional allow failure", "godot.yml", "    timeout-minutes: 90", "    timeout-minutes: 90\n    continue-on-error: true", false},
	} {
		t.Run(test.name, func(t *testing.T) {
			fixture := godotRollbackFixture(t)
			sources := map[string]string{"ci.yml": required, "godot.yml": optional}
			if test.file != "" {
				sources[test.file] = mutateGodotAnchorOnce(t, sources[test.file], test.old, test.replacement)
			}
			for name, source := range sources {
				writeGodotEntrypointMutationFile(t, fixture, ".github/workflows/"+name, source)
			}
			command := exec.Command("bash", filepath.Join(fixture, "scripts/godot/rollback-check.sh"))
			output, err := command.CombinedOutput()
			if (err == nil) != test.wantSuccess || strings.Contains(string(output), "check passed") != test.wantSuccess {
				t.Fatalf("rollback success=%t: %v\n%s", test.wantSuccess, err, output)
			}
		})
	}
}

// Only workflow and Makefile copies are mutable; authority checks observe the
// real repository through read-only fixture links.
func godotRollbackFixture(t *testing.T) string {
	t.Helper()
	sourceRoot, root := repositoryRoot(t), t.TempDir()
	for _, relative := range []string{"Makefile", ".github/workflows/ci.yml", "scripts/godot/rollback-check.sh"} {
		writeGodotEntrypointMutationFile(t, root, relative, readBaselineDoc(t, sourceRoot, relative))
	}
	for _, relative := range []string{"apps", "docs", "packages"} {
		if err := os.Symlink(filepath.Join(sourceRoot, relative), filepath.Join(root, relative)); err != nil {
			t.Fatal(err)
		}
	}
	return root
}
