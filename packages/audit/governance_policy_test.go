package archcheck_test

import (
	"path/filepath"
	"strings"
	"testing"
)

func TestProviderAwareOrchestration(t *testing.T) {
	root := repositoryRoot(t)
	for _, path := range []string{"AGENTS.md", filepath.Join("openspec", "config.yaml")} {
		text := readBaselineDoc(t, root, path)
		for _, required := range []string{
			"verified ChatGPT or Codex controller using an OpenAI model",
			"standing project authorization",
			"direct, delegated, or mixed execution",
			"without a separate per-task user request",
			"non-OpenAI or unknown-provider controller",
			"strict `subagent-driven-development`",
			"explicit user prohibition",
			"higher-priority runtime restriction",
			"three subagents",
		} {
			if !strings.Contains(text, required) {
				t.Errorf("%s does not state required provider-aware policy %q", path, required)
			}
		}
	}
}

func TestAgentGuidance(t *testing.T) {
	root := repositoryRoot(t)
	for _, path := range []string{
		filepath.Join("docs", "AGENTS.md"),
		filepath.Join("packages", "engine", "AGENTS.md"),
	} {
		text := readBaselineDoc(t, root, path)
		if !strings.Contains(text, "The root provider-aware orchestration policy applies") {
			t.Errorf("%s must defer to the root provider-aware orchestration policy", path)
		}
	}
}

func TestCodeCommentLanguagePolicy(t *testing.T) {
	root := repositoryRoot(t)
	for _, path := range []string{
		"AGENTS.md",
		filepath.Join("docs", "AGENTS.md"),
		filepath.Join("packages", "engine", "AGENTS.md"),
		filepath.Join("docs", "test-organization.md"),
	} {
		text := readBaselineDoc(t, root, path)
		if !strings.Contains(text, "comments") || !strings.Contains(text, "English") {
			t.Errorf("%s must require English comments", path)
		}
		for _, forbidden := range []string{"comments use Chinese", "Chinese comments", "注释使用中文", "中文注释", "中文 `///`", "中文 doc 注释"} {
			if strings.Contains(text, forbidden) {
				t.Errorf("%s retains obsolete Chinese-comment requirement %q", path, forbidden)
			}
		}
	}
}

func TestDelegationBudgetAndContextIsolation(t *testing.T) {
	root := repositoryRoot(t)
	for _, path := range []string{
		"AGENTS.md",
		filepath.Join(".codex", "skills", "mornlea-implementation-orchestration", "SKILL.md"),
		filepath.Join(".claude", "skills", "mornlea-implementation-orchestration", "SKILL.md"),
	} {
		text := readBaselineDoc(t, root, path)
		if violations := delegationPolicyViolations(text); len(violations) > 0 {
			t.Errorf("%s does not enforce cost-aware context-isolation delegation:\n%s", path, strings.Join(violations, "\n"))
		}
	}
}

func TestDelegationBudgetGuardDetectsDrift(t *testing.T) {
	valid := strings.Join([]string{
		"isolation-first",
		"At most three subagents may run concurrently",
		"main-context retention",
		"Do not delegate merely for parallel speed",
		"concise task brief",
	}, "\n")
	if violations := delegationPolicyViolations(valid); len(violations) != 0 {
		t.Fatalf("valid delegation fixture produced violations: %v", violations)
	}

	for _, fragment := range delegationPolicyFragments() {
		mutated := strings.Replace(valid, fragment, "", 1)
		if violations := delegationPolicyViolations(mutated); len(violations) != 1 {
			t.Errorf("removing %q produced violations %v, want exactly one", fragment, violations)
		}
	}
}

func delegationPolicyViolations(text string) []string {
	var violations []string
	for _, fragment := range delegationPolicyFragments() {
		if !strings.Contains(text, fragment) {
			violations = append(violations, "missing delegation policy fragment: "+fragment)
		}
	}
	return violations
}

func delegationPolicyFragments() []string {
	return []string{
		"isolation-first",
		"At most three subagents may run concurrently",
		"main-context retention",
		"Do not delegate merely for parallel speed",
		"concise task brief",
	}
}

// TestCanonicalGovernanceSpecDelegationBudget keeps the canonical governance
// specification on the same subagent ceiling as AGENTS.md, openspec/config.yaml,
// and the orchestration skills. The specification sits above prose guidance in
// the source-of-truth order, so an unguarded copy could silently override them.
func TestCanonicalGovernanceSpecDelegationBudget(t *testing.T) {
	root := repositoryRoot(t)
	path := filepath.Join("openspec", "specs", "development-governance", "spec.md")
	text := readBaselineDoc(t, root, path)
	if violations := canonicalDelegationBudgetViolations(text); len(violations) > 0 {
		t.Errorf("%s drifted from the three-subagent ceiling:\n%s", path, strings.Join(violations, "\n"))
	}
}

func TestCanonicalGovernanceSpecDelegationBudgetGuardDetectsDrift(t *testing.T) {
	valid := "MUST run no more than three subagents concurrently\n- **AND** MUST NOT start a fourth concurrent subagent\n"
	if violations := canonicalDelegationBudgetViolations(valid); len(violations) != 0 {
		t.Fatalf("valid fixture produced violations: %v", violations)
	}
	stale := "MUST run no more than two subagents concurrently\n- **AND** MUST NOT start a third concurrent subagent\n"
	if violations := canonicalDelegationBudgetViolations(stale); len(violations) != 4 {
		t.Fatalf("stale fixture violations = %v, want four", violations)
	}
}

func canonicalDelegationBudgetViolations(text string) []string {
	var violations []string
	for _, required := range []string{
		"MUST run no more than three subagents concurrently",
		"MUST NOT start a fourth concurrent subagent",
	} {
		if !strings.Contains(text, required) {
			violations = append(violations, "missing canonical ceiling fragment: "+required)
		}
	}
	for _, forbidden := range []string{
		"no more than two subagents",
		"start a third concurrent subagent",
	} {
		if strings.Contains(text, forbidden) {
			violations = append(violations, "stale canonical ceiling fragment: "+forbidden)
		}
	}
	return violations
}
