package archcheck_test

import (
	"fmt"
	"os/exec"
	"path/filepath"
	"reflect"
	"slices"
	"strings"
	"testing"

	"gopkg.in/yaml.v3"
)

const (
	ciCheckout = "actions/checkout@fbc6f3992d24b796d5a048ff273f7fcc4a7b6c09"
	ciGo       = "actions/setup-go@924ae3a1cded613372ab5595356fb5720e22ba16"
	ciNode     = "actions/setup-node@a0853c24544627f65ddf259abe73b1d18a591444"
	ciPython   = "actions/setup-python@ece7cb06caefa5fff74198d8649806c4678c61a1"
	ciUpload   = "actions/upload-artifact@ea165f8d65b6e75b540449e92b4886f43607fa02"
	ciDownload = "actions/download-artifact@634f93cb2916e3fdff6788551b99b062d0335ce0"
	ciUV       = "astral-sh/setup-uv@37802adc94f370d6bfd71619e3f0bf239e1f3b78"
)

var ciActions = map[string]string{ciCheckout: "v5", ciGo: "v6", ciNode: "v5", ciPython: "v6", ciUpload: "v4", ciDownload: "v5", ciUV: "v7"}

type requiredCIJob struct {
	runner  string
	timeout int
	needs   []string
	setups  []string
}

var requiredCIJobs = map[string]requiredCIJob{
	"preflight":          {"ubuntu-24.04", 10, nil, []string{ciGo, ciNode}},
	"frontend":           {"ubuntu-24.04", 15, nil, []string{ciNode}},
	"rust-quality":       {"ubuntu-24.04", 30, nil, []string{ciGo, ciPython, ciUV}},
	"native-linux":       {"ubuntu-24.04", 30, nil, []string{ciGo}},
	"native-macos":       {"macos-15", 30, nil, []string{ciGo}},
	"linux-quality":      {"ubuntu-24.04", 20, []string{"native-linux"}, []string{ciGo}},
	"race-server":        {"ubuntu-24.04", 30, []string{"native-linux"}, []string{ciGo}},
	"race-rest":          {"ubuntu-24.04", 30, []string{"native-linux"}, []string{ciGo}},
	"race-client":        {"macos-15", 45, []string{"native-macos"}, []string{ciGo}},
	"integration-server": {"ubuntu-24.04", 30, []string{"native-linux"}, []string{ciGo, ciPython, ciUV}},
	"integration-client": {"macos-15", 45, []string{"native-macos"}, []string{ciGo}},
}

const ciGoCache = `packages/shared/go.sum
packages/server/go.sum
packages/client/go.sum
packages/tools/go.sum
packages/audit/go.sum
packages/engine/include/mornlea_engine.h
packages/engine/include/mornlea_client.h`

func TestRequiredCIWorkflow(t *testing.T) {
	source := readBaselineDoc(t, repositoryRoot(t), filepath.Join(".github", "workflows", "ci.yml"))
	if violations := requiredWorkflowViolations([]byte(source)); len(violations) > 0 {
		t.Fatal(strings.Join(violations, "\n"))
	}
}

// `requiredWorkflowViolations` audits executable orchestration independently of
// package selection and artifact verification, which remain repository commands.
func requiredWorkflowViolations(source []byte) []string {
	var workflow companionWorkflow
	decoder := yaml.NewDecoder(strings.NewReader(string(source)))
	decoder.KnownFields(true)
	if err := decoder.Decode(&workflow); err != nil {
		return []string{fmt.Sprintf("invalid workflow YAML: %v", err)}
	}
	var tree yaml.Node
	if err := yaml.Unmarshal(source, &tree); err != nil {
		return []string{err.Error()}
	}
	var violations []string
	require := func(ok bool, message string) {
		if !ok {
			violations = append(violations, message)
		}
	}
	require(workflow.Name == "Required CI", "workflow name must be Required CI")
	_, hasPush := workflow.On["push"]
	_, hasPR := workflow.On["pull_request"]
	require(len(workflow.On) == 2 && hasPush && hasPR, "events must be main push and pull_request only")
	if push, ok := workflow.On["push"]; ok {
		var event map[string][]string
		err := push.Decode(&event)
		require(err == nil && len(event) == 1 && slices.Equal(event["branches"], []string{"main"}), "push must select only main")
	}
	if pr, ok := workflow.On["pull_request"]; ok {
		require(pr.Tag == "!!null", "pull_request must not filter required work")
	}
	require(workflow.Concurrency.Group == "required-ci-${{ github.event.pull_request.number || github.ref }}" && workflow.Concurrency.Cancel, "candidate concurrency must cancel stale runs")
	require(reflect.DeepEqual(workflow.Permissions, map[string]string{"contents": "read"}), "workflow permissions must be contents read")
	require(reflect.DeepEqual(workflow.Env, map[string]string{"CARGO_TARGET_DIR": "${{ github.workspace }}/packages/engine/target/cargo"}), "workflow Cargo target root must be explicit")
	// Reject escape hatches even when they live on fields not decoded by the schema.
	var inspect func(*yaml.Node)
	inspect = func(node *yaml.Node) {
		if node.Kind == yaml.MappingNode {
			for i := 0; i < len(node.Content); i += 2 {
				key, value := node.Content[i], node.Content[i+1]
				require(key.Value != "continue-on-error" && key.Value != "strategy" && key.Value != "defaults" && key.Value != "container" && key.Value != "services", "forbidden execution override: "+key.Value)
				if key.Value == "uses" && !strings.HasPrefix(value.Value, "./") {
					tag, ok := ciActions[value.Value]
					require(ok, "unreviewed action: "+value.Value)
					require(ok && strings.TrimSpace(value.LineComment) == "# "+tag, "action requires reviewed major comment: "+value.Value)
				}
			}
		}
		for _, child := range node.Content {
			inspect(child)
		}
	}
	inspect(&tree)
	require(!strings.Contains(strings.ToLower(string(source)), "godot"), "Godot must remain outside required workflow")
	require(len(workflow.Jobs) == len(requiredCIJobs)+1, "required job set must be exact")
	var needs []string
	for name, want := range requiredCIJobs {
		needs = append(needs, name)
		job, ok := workflow.Jobs[name]
		if !ok {
			violations = append(violations, "missing required job: "+name)
			continue
		}
		require(job.RunsOn.Value == want.runner, name+": wrong runner")
		require(job.Timeout == want.timeout, name+": wrong timeout")
		require(slices.Equal([]string(job.Needs), want.needs), name+": wrong needs")
		require(job.If.Value == "", name+": required job must not be conditional")
		require(job.Permissions == nil, name+": job must inherit least privilege permissions")
		require(len(job.Env) == 0 || (name == "frontend" && reflect.DeepEqual(job.Env, map[string]string{"COREPACK_ENABLE_DOWNLOAD_PROMPT": "0"})), name+": unexpected environment override")
		command := "make ci-" + name
		if strings.HasPrefix(name, "native-") || len(want.needs) > 0 {
			command += ` CI_CANDIDATE_SHA="$GITHUB_SHA"`
		}
		commandIndexes := workflowExactCommandStepIndexes(job.Steps, command)
		require(len(commandIndexes) == 1, name+": missing exact repository command")
		checkoutIndexes := workflowUsesStepIndexes(job.Steps, ciCheckout)
		require(len(checkoutIndexes) == 1, name+": candidate checkout required")
		if len(checkoutIndexes) == 1 {
			require(checkoutIndexes[0] == 1, name+": checkout must precede repository-dependent setup")
		}
		var setups []string
		var runs []string
		for index, step := range job.Steps {
			if step.Uses != "" {
				require(step.If.Value == "" && step.Run == "", name+": actions must run unconditionally")
				if len(commandIndexes) == 1 && step.Uses != ciUpload {
					require(index < commandIndexes[0], name+": setup/download must precede command")
				}
				if step.Uses == ciCheckout {
					require(len(step.With) == 0, name+": checkout must use candidate defaults")
				}
				if step.Uses != ciCheckout && step.Uses != ciUpload && step.Uses != ciDownload {
					setups = append(setups, step.Uses)
				}
				if step.Uses == ciGo {
					require(len(step.With) == 2, name+": Go setup input contract")
					value, err := workflowStringWith(step, "go-version")
					require(err == nil && value == "1.26", name+": Go version")
					value, err = workflowStringWith(step, "cache-dependency-path")
					require(err == nil && slices.Equal(nonEmptyTrimmedLines(value), nonEmptyTrimmedLines(ciGoCache)), name+": Go module/header cache")
				}
				if step.Uses == ciNode {
					inputCount := 1
					if name == "frontend" {
						inputCount = 3
					}
					require(len(step.With) == inputCount, name+": Node setup input contract")
					value, err := workflowStringWith(step, "node-version")
					require(err == nil && value == "24", name+": Node version")
					if name == "frontend" {
						value, err = workflowStringWith(step, "cache")
						require(err == nil && value == "pnpm", name+": pnpm cache")
						value, err = workflowStringWith(step, "cache-dependency-path")
						require(err == nil && value == "packages/engine/crates/mornlea_client/frontend/pnpm-lock.yaml", name+": locked pnpm cache")
					}
				}
				if step.Uses == ciPython {
					require(len(step.With) == 1, name+": Python setup input contract")
				}
				if step.Uses == ciUV {
					require(len(step.With) == 3, name+": uv setup input contract")
				}
			}
			if step.Run != "" {
				runs = append(runs, strings.TrimSpace(step.Run))
				if strings.TrimSpace(step.Run) != ciSummary(name) {
					require(step.If.Value == "", name+": required command must not be conditional")
				} else {
					require(step.If.Value == "${{ always() }}", name+": summary must always run")
				}
			}
		}
		require(slices.Equal(setups, want.setups), name+": setup actions must match command dependencies")
		wantRuns := []string{ciStart}
		if name == "preflight" || name == "linux-quality" || name == "race-rest" {
			wantRuns = append(wantRuns, "sudo apt-get update\nsudo apt-get install --yes ripgrep")
		}
		if name == "frontend" {
			wantRuns = append(wantRuns, "corepack enable\ncd packages/engine/crates/mornlea_client/frontend\ncorepack pnpm --version")
		}
		if name == "rust-quality" || strings.HasPrefix(name, "native-") {
			wantRuns = append(wantRuns, "cd packages/engine\nrustup show active-toolchain\nrustc --version\ncargo --version")
		}
		wantRuns = append(wantRuns, command, ciSummary(name))
		require(slices.Equal(runs, wantRuns), name+": run steps must remain thin, bounded orchestration without retries or inline selectors")
		uploads := workflowUsesStepIndexes(job.Steps, ciUpload)
		downloads := workflowUsesStepIndexes(job.Steps, ciDownload)
		if strings.HasPrefix(name, "native-") {
			require(len(uploads) == 1 && len(downloads) == 0, name+": producer artifact steps")
			if len(uploads) == 1 {
				step := job.Steps[uploads[0]]
				require(len(step.With) == 4, name+": upload input contract")
				value, err := workflowStringWith(step, "name")
				require(err == nil && value == name+"-${{ github.sha }}", name+": same-SHA upload")
				value, err = workflowStringWith(step, "path")
				paths := []string{"build/ci/native-macos.manifest", "packages/engine/target/release/libmornlea_engine.dylib", "packages/engine/target/release/libmornlea_client.dylib"}
				if name == "native-linux" {
					paths = []string{"build/ci/native-linux.manifest", "bin/mornlea-server", "bin/libmornlea_engine.so", "packages/engine/target/release/libmornlea_engine.so"}
				}
				require(err == nil && slices.Equal(nonEmptyTrimmedLines(value), paths), name+": artifact paths")
				value, err = workflowStringWith(step, "if-no-files-found")
				require(err == nil && value == "error", name+": missing files must fail")
				retention := step.With["retention-days"].Node
				require(retention != nil && retention.Tag == "!!int" && retention.Value == "1", name+": one day retention")
				if len(commandIndexes) == 1 {
					require(uploads[0] > commandIndexes[0], name+": upload must follow successful build")
				}
			}
		} else if len(want.needs) > 0 {
			require(len(downloads) == 1 && len(uploads) == 0, name+": consumer artifact steps")
			if len(downloads) == 1 {
				step := job.Steps[downloads[0]]
				require(len(step.With) == 2, name+": download must use the current run")
				value, err := workflowStringWith(step, "name")
				require(err == nil && value == want.needs[0]+"-${{ github.sha }}", name+": same-SHA download")
				value, err = workflowStringWith(step, "path")
				require(err == nil && value == ".", name+": artifact root must be repository root")
			}
		} else {
			require(len(downloads) == 0 && len(uploads) == 0, name+": independent stage must not transfer artifacts")
		}
	}
	slices.Sort(needs)
	gate, ok := workflow.Jobs["merge-gate"]
	require(ok, "missing merge-gate")
	require(gate.If.Value == "${{ always() }}", "merge-gate must always run")
	gotNeeds := slices.Clone([]string(gate.Needs))
	slices.Sort(gotNeeds)
	require(slices.Equal(gotNeeds, needs), "merge-gate needs must include every required job exactly once")
	require(gate.RunsOn.Value == "ubuntu-24.04" && gate.Timeout == 5, "merge-gate runner/timeout")
	require(gate.Permissions == nil && len(gate.Env) == 0, "merge-gate must inherit permissions and environment")
	require(len(gate.Steps) == 1, "merge-gate must be a pure single-step aggregator")
	if len(gate.Steps) == 1 {
		step := gate.Steps[0]
		require(step.Uses == "" && step.If.Value == "", "merge-gate must only assert results")
		var assertions []string
		for _, name := range needs {
			assertions = append(assertions, fmt.Sprintf(`test "${{ needs.%s.result }}" = success`, name))
		}
		got := workflowShellStatements(step.Run)
		slices.Sort(got)
		require(slices.Equal(got, assertions), "merge-gate must assert success for every need without extra commands")
	}
	violations = append(violations, companionAgentWorkflowViolations(source)...)
	slices.Sort(violations)
	return violations
}

const ciStart = `echo "CI_START_EPOCH=$(date +%s)" >> "$GITHUB_ENV"`

func ciSummary(name string) string {
	return fmt.Sprintf("{\n  echo \"%s seconds: $(( $(date +%%s) - CI_START_EPOCH ))\"\n  echo \"runner: ${RUNNER_OS}/$(uname -m)\"\n} >> \"$GITHUB_STEP_SUMMARY\"", name)
}

func TestRequiredCIWorkflowMutations(t *testing.T) {
	source := readBaselineDoc(t, repositoryRoot(t), filepath.Join(".github", "workflows", "ci.yml"))
	if violations := requiredWorkflowViolations([]byte(source)); len(violations) > 0 {
		t.Fatalf("baseline workflow: %v", violations)
	}
	mutations := []struct{ name, old, new string }{
		{"omitted merge need", "needs: [preflight, frontend, rust-quality, native-linux, native-macos, linux-quality, race-server, race-rest, race-client, integration-server, integration-client]", "needs: [frontend, rust-quality, native-linux, native-macos, linux-quality, race-server, race-rest, race-client, integration-server, integration-client]"},
		{"conditional merge", "  merge-gate:\n    if: ${{ always() }}", "  merge-gate:\n    if: ${{ success() }}"},
		{"missing assertion", `          test "${{ needs.preflight.result }}" = success`, ""},
		{"Godot job", "jobs:\n", "jobs:\n  godot:\n    runs-on: ubuntu-24.04\n    steps: []\n"},
		{"moving runner", "runs-on: ubuntu-24.04", "runs-on: ubuntu-latest"},
		{"mutable action", ciCheckout, "actions/checkout@v5"},
		{"missing timeout", "    timeout-minutes: 10\n", ""},
		{"widened permissions", "contents: read", "contents: write"},
		{"artifact sha", "name: native-linux-${{ github.sha }}", "name: native-linux-main"},
		{"artifact root", "          path: .\n", "          path: packages/engine/target/release\n"},
		{"allow failure", "    timeout-minutes: 10", "    timeout-minutes: 10\n    continue-on-error: true"},
		{"inline selector", "run: make ci-preflight", "run: go test ./packages/audit -count=1"},
		{"retry", "run: make ci-preflight", "run: make ci-preflight || make ci-preflight"},
		{"conditional stage", "  preflight:\n", "  preflight:\n    if: ${{ false }}\n"},
		{"missing summary", "echo \"runner: ${RUNNER_OS}/$(uname -m)\"", "echo runner"},
		{"wrong Go", "go-version: '1.26'", "go-version: '1.25'"},
		{"wrong Node", "node-version: '24'", "node-version: '20'"},
		{"missing upload file", "            bin/mornlea-server\n", ""},
		{"foreign artifact run", "          path: .\n", "          path: .\n          run-id: 1\n"},
		{"unchecked shell", "      - name: Assert every required result\n", "      - name: Assert every required result\n        shell: bash {0}\n"},
		{"step environment override", "      - name: Run preflight\n", "      - name: Run preflight\n        env:\n          CI_CANDIDATE_SHA: wrong\n"},
		{"filtered required event", "  pull_request:\n", "  pull_request:\n    paths: [README.md]\n"},
		{"stale run cancellation", "cancel-in-progress: true", "cancel-in-progress: false"},
		{"candidate override", "      - uses: " + ciCheckout + " # v5\n", "      - uses: " + ciCheckout + " # v5\n        with:\n          ref: main\n"},
	}
	for _, mutation := range mutations {
		t.Run(mutation.name, func(t *testing.T) {
			if !strings.Contains(source, mutation.old) {
				t.Fatal("mutation sentinel missing")
			}
			mutated := strings.Replace(source, mutation.old, mutation.new, 1)
			if violations := requiredWorkflowViolations([]byte(mutated)); len(violations) == 0 {
				t.Fatal("policy regression accepted")
			}
		})
	}
	for _, test := range []struct{ job, next string }{{"linux-quality", "race-server"}, {"race-rest", "race-client"}} {
		t.Run("missing ripgrep setup in "+test.job, func(t *testing.T) {
			block := "  " + test.job + ":\n"
			start := strings.Index(source, block)
			if start < 0 {
				t.Fatal("job sentinel missing")
			}
			end := strings.Index(source[start+len(block):], "\n  "+test.next+":\n")
			if end < 0 {
				t.Fatal("next job sentinel missing")
			}
			end += start + len(block)
			jobSource := source[start:end]
			setup := "      - name: Install policy dependencies\n        run: |\n          sudo apt-get update\n          sudo apt-get install --yes ripgrep\n"
			if !strings.Contains(jobSource, setup) {
				t.Fatal("setup sentinel missing")
			}
			mutated := source[:start] + strings.Replace(jobSource, setup, "", 1) + source[end:]
			if violations := requiredWorkflowViolations([]byte(mutated)); len(violations) == 0 {
				t.Fatal("missing ripgrep setup accepted")
			}
		})
	}
}

func TestRequiredCIWorkflowMergeGateFailsClosed(t *testing.T) {
	source := readBaselineDoc(t, repositoryRoot(t), filepath.Join(".github", "workflows", "ci.yml"))
	var workflow companionWorkflow
	if err := yaml.Unmarshal([]byte(source), &workflow); err != nil {
		t.Fatal(err)
	}
	gate := workflow.Jobs["merge-gate"]
	if len(gate.Steps) != 1 {
		t.Fatal("merge-gate must have one step")
	}
	for _, result := range []string{"success", "failure", "cancelled", "skipped", ""} {
		for _, failed := range gate.Needs {
			t.Run(failed+"/"+result, func(t *testing.T) {
				script := gate.Steps[0].Run
				for _, need := range gate.Needs {
					value := "success"
					if need == failed {
						value = result
					}
					script = strings.ReplaceAll(script, "${{ needs."+need+".result }}", value)
				}
				err := exec.Command("bash", "-e", "-c", script).Run()
				if (err == nil) != (result == "success") {
					t.Fatalf("result %q returned %v", result, err)
				}
			})
		}
	}
}
