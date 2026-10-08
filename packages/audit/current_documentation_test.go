package archcheck_test

import (
	"fmt"
	"os"
	"path/filepath"
	"regexp"
	"slices"
	"strconv"
	"strings"
	"testing"
)

var markdownLinkPattern = regexp.MustCompile(`\[[^\]]*\]\(([^)]+)\)`)
var inlineCodePattern = regexp.MustCompile("`([^`\\n]+)`")

func TestCurrentDocumentationVersions(t *testing.T) {
	root := repositoryRoot(t)
	for _, relative := range currentNormativeDocumentation(t, root) {
		data, err := os.ReadFile(filepath.Join(root, filepath.FromSlash(relative)))
		if err != nil {
			t.Fatal(err)
		}
		currentText := stripHistoricalMarkdownSections(string(data))
		for _, mapping := range baselineVersionMappings {
			source, err := os.ReadFile(filepath.Join(root, mapping.sourcePath))
			if err != nil {
				t.Fatalf("read authoritative source %s: %v", mapping.sourcePath, err)
			}
			want, err := resolveBaselineConstant(string(source), mapping.codePattern)
			if err != nil {
				t.Fatalf("resolve authoritative %s: %v", mapping.name, err)
			}
			for _, pattern := range currentDocumentationVersionPatterns(mapping) {
				for _, match := range regexp.MustCompile(pattern).FindAllStringSubmatch(currentText, -1) {
					if match[1] != want {
						t.Errorf("%s has stale %s v%s, want v%s from %s", relative, mapping.name, match[1], want, mapping.sourcePath)
					}
				}
			}
		}
	}
}

func TestCurrentDocumentationVersionGuardIgnoresExplicitHistory(t *testing.T) {
	text := "## Current\nengine ABI v11\n\n## Historical note\nengine ABI v10\n\n## Next\nclient ABI v19\n"
	stripped := stripHistoricalMarkdownSections(text)
	if strings.Contains(stripped, "engine ABI v10") {
		t.Fatalf("historical section was not stripped:\n%s", stripped)
	}
	for _, want := range []string{"engine ABI v11", "client ABI v19"} {
		if !strings.Contains(stripped, want) {
			t.Errorf("current claim %q was stripped", want)
		}
	}
}

func TestDocumentationLinks(t *testing.T) {
	root := repositoryRoot(t)
	for _, relative := range documentationLinkScope(t, root) {
		data, err := os.ReadFile(filepath.Join(root, filepath.FromSlash(relative)))
		if err != nil {
			t.Fatal(err)
		}
		for _, problem := range documentationLinkProblems(root, relative, string(data)) {
			t.Error(problem)
		}
	}
}

// TestDocumentationLinkScopeCoversActiveChangesButNotArchive pins the link
// gate's scope: in-progress OpenSpec changes are current planning inputs and
// must keep resolvable links, while archived changes are frozen records whose
// historical links are never rewritten.
func TestDocumentationLinkScopeCoversActiveChangesButNotArchive(t *testing.T) {
	root := t.TempDir()
	files := map[string]string{
		documentationManifestPath:                                 `{"schema_version":1,"documents":[]}`,
		"openspec/changes/sample-change/proposal.md":              "[gone](missing.md)\n",
		"openspec/changes/sample-change/plans/01-plan.md":         "[ok](../proposal.md)\n",
		"openspec/changes/archive/2026-01-01-old/proposal.md":     "[gone](missing.md)\n",
		"openspec/changes/archive/2026-01-01-old/plans/01-old.md": "[gone](missing.md)\n",
	}
	for relative, content := range files {
		path := filepath.Join(root, filepath.FromSlash(relative))
		if err := os.MkdirAll(filepath.Dir(path), 0o755); err != nil {
			t.Fatal(err)
		}
		if err := os.WriteFile(path, []byte(content), 0o644); err != nil {
			t.Fatal(err)
		}
	}
	scope := documentationLinkScope(t, root)
	want := []string{"openspec/changes/sample-change/plans/01-plan.md", "openspec/changes/sample-change/proposal.md"}
	if !slices.Equal(scope, want) {
		t.Fatalf("documentation link scope = %v, want %v", scope, want)
	}
	var problems []string
	for _, relative := range scope {
		problems = append(problems, documentationLinkProblems(root, relative, files[relative])...)
	}
	if len(problems) != 1 || !strings.Contains(problems[0], "openspec/changes/sample-change/proposal.md") {
		t.Fatalf("active change link problems = %v, want exactly the broken active proposal link", problems)
	}
}

func TestDocumentationLinkValidationRejectsMissingTarget(t *testing.T) {
	root := t.TempDir()
	problems := documentationLinkProblems(root, "docs/sample.md", "[missing](missing.md)\n")
	if len(problems) != 1 || !strings.Contains(problems[0], "missing.md") {
		t.Fatalf("missing local link problems = %v", problems)
	}
	if problems := documentationLinkProblems(root, "docs/sample.md", "[external](https://example.com)\n"); len(problems) != 0 {
		t.Fatalf("external link must not be treated as a repository path: %v", problems)
	}
}

func TestDocumentationSemanticClaims(t *testing.T) {
	root := repositoryRoot(t)
	manifest := readDocumentationManifest(t, root)
	for _, entry := range manifest.Documents {
		if entry.Classification != "bilingual" ||
			!slices.Contains(entry.Paths, entry.EnglishPath) ||
			!slices.Contains(entry.Paths, entry.ChinesePath) {
			continue
		}
		english := readBaselineDoc(t, root, entry.EnglishPath)
		chinese := readBaselineDoc(t, root, entry.ChinesePath)
		for _, problem := range validateDocumentationSemanticPair(entry.ID, entry.EnglishPath, entry.ChinesePath, english, chinese) {
			t.Error(problem)
		}
	}
}

func TestDocumentationSemanticPairRejectsCommandPathAndVersionDrift(t *testing.T) {
	english := "Run `make dev-check`, then inspect `packages/client/runtime` and [architecture](architecture.md). Current protocol v44.\n"
	chinese := "Run `make test-race`, then inspect `packages/server/runtime` and [architecture](other.zh.md). Current protocol v43.\n"
	problems := strings.Join(validateDocumentationSemanticPair("sample", "docs/sample.md", "docs/sample.zh.md", english, chinese), "\n")
	for _, want := range []string{"command claims differ", "repository path claims differ", "local link claims differ", "version claims differ"} {
		if !strings.Contains(problems, want) {
			t.Errorf("semantic pair problems must mention %q:\n%s", want, problems)
		}
	}
}

func validateDocumentationSemanticPair(id, englishPath, chinesePath, english, chinese string) []string {
	checks := []struct {
		name    string
		extract func(string) []string
	}{
		{name: "command", extract: documentationCommandClaims},
		{name: "repository path", extract: documentationRepositoryPathClaims},
		{name: "local link", extract: documentationLocalLinkClaims},
		{name: "version", extract: documentationVersionClaims},
	}
	var problems []string
	for _, check := range checks {
		englishClaims := check.extract(english)
		chineseClaims := check.extract(chinese)
		if !slices.Equal(englishClaims, chineseClaims) {
			problems = append(problems, fmt.Sprintf("%s %s claims differ between %s and %s:\nEnglish: %v\nChinese: %v", id, check.name, englishPath, chinesePath, englishClaims, chineseClaims))
		}
	}
	return problems
}

func documentationLinkProblems(root, relative, text string) []string {
	var problems []string
	for _, match := range markdownLinkPattern.FindAllStringSubmatch(text, -1) {
		target := localDocumentationLinkTarget(match[1])
		if target == "" {
			continue
		}
		absolute := filepath.Join(root, filepath.Dir(filepath.FromSlash(relative)), filepath.FromSlash(target))
		if _, err := os.Stat(absolute); err != nil {
			problems = append(problems, fmt.Sprintf("%s has broken local link %q: %v", relative, match[1], err))
		}
	}
	return problems
}

func documentationLocalLinkClaims(text string) []string {
	text = stripHistoricalMarkdownSections(text)
	var claims []string
	for _, match := range markdownLinkPattern.FindAllStringSubmatch(text, -1) {
		if target := localDocumentationLinkTarget(match[1]); target != "" {
			claims = append(claims, normalizeLocalizedDocumentationPath(target))
		}
	}
	slices.Sort(claims)
	return slices.Compact(claims)
}

func localDocumentationLinkTarget(raw string) string {
	target := strings.TrimSpace(strings.SplitN(raw, "#", 2)[0])
	if target == "" || strings.Contains(target, "://") || strings.HasPrefix(target, "mailto:") {
		return ""
	}
	if strings.HasPrefix(target, "<") && strings.HasSuffix(target, ">") {
		target = strings.TrimSuffix(strings.TrimPrefix(target, "<"), ">")
	}
	return filepath.ToSlash(filepath.Clean(target))
}

func documentationRepositoryPathClaims(text string) []string {
	text = stripHistoricalMarkdownSections(text)
	var claims []string
	for _, match := range inlineCodePattern.FindAllStringSubmatch(text, -1) {
		for _, field := range strings.Fields(match[1]) {
			candidate := strings.Trim(field, "'\"(),:;[]")
			candidate = strings.TrimPrefix(candidate, "./")
			if !isRepositoryPathClaim(candidate) {
				continue
			}
			claims = append(claims, normalizeLocalizedDocumentationPath(candidate))
		}
	}
	slices.Sort(claims)
	return slices.Compact(claims)
}

func isRepositoryPathClaim(candidate string) bool {
	if candidate == "" || strings.Contains(candidate, "://") {
		return false
	}
	for _, prefix := range []string{"apps/", "docs/", "openspec/", "packages/", "scripts/", "testdata/", ".codex/", ".claude/", ".github/"} {
		if strings.HasPrefix(candidate, prefix) {
			return true
		}
	}
	for _, exact := range []string{"AGENTS.md", "CLAUDE.md", "Makefile", "README.md", "README.zh.md", "go.work"} {
		if candidate == exact {
			return true
		}
	}
	return false
}

func normalizeLocalizedDocumentationPath(path string) string {
	if strings.HasSuffix(path, ".zh.md") {
		return strings.TrimSuffix(path, ".zh.md") + ".md"
	}
	return path
}

func documentationVersionClaims(text string) []string {
	text = stripHistoricalMarkdownSections(text)
	var claims []string
	for _, mapping := range baselineVersionMappings {
		for _, pattern := range currentDocumentationVersionPatterns(mapping) {
			for _, match := range regexp.MustCompile(pattern).FindAllStringSubmatch(text, -1) {
				claims = append(claims, mapping.name+"=v"+match[1])
			}
		}
	}
	slices.Sort(claims)
	return slices.Compact(claims)
}

func currentNormativeDocumentation(t *testing.T, root string) []string {
	t.Helper()
	manifest := readDocumentationManifest(t, root)
	var paths []string
	for _, entry := range manifest.Documents {
		switch entry.Classification {
		case "machine":
			paths = append(paths, entry.Paths...)
		case "bilingual":
			if slices.Contains(entry.Paths, entry.EnglishPath) && slices.Contains(entry.Paths, entry.ChinesePath) {
				paths = append(paths, entry.EnglishPath, entry.ChinesePath)
			}
		}
	}
	slices.Sort(paths)
	return slices.Compact(paths)
}

// documentationLinkScope lists the Markdown files whose local links must
// resolve: completed bilingual documentation pairs plus every in-progress
// OpenSpec change artifact. `activeOpenSpecMarkdown` skips
// openspec/changes/archive/ because archived changes are frozen evidence.
func documentationLinkScope(t *testing.T, root string) []string {
	t.Helper()
	paths := append(currentCompletedDocumentationPairs(t, root), activeOpenSpecMarkdown(t, root)...)
	slices.Sort(paths)
	return slices.Compact(paths)
}

func currentCompletedDocumentationPairs(t *testing.T, root string) []string {
	t.Helper()
	manifest := readDocumentationManifest(t, root)
	var paths []string
	for _, entry := range manifest.Documents {
		if entry.Classification != "bilingual" {
			continue
		}
		if slices.Contains(entry.Paths, entry.EnglishPath) && slices.Contains(entry.Paths, entry.ChinesePath) {
			paths = append(paths, entry.EnglishPath, entry.ChinesePath)
		}
	}
	slices.Sort(paths)
	return paths
}

func currentDocumentationVersionPatterns(mapping baselineVersionMapping) []string {
	patterns := []string{mapping.docPattern}
	switch mapping.name {
	case "协议版本":
		patterns = append(patterns, `协议 v(\d+)`)
	case "区块 schema":
		patterns = append(patterns, `区块 schema v(\d+)`)
	case "玩家 schema":
		patterns = append(patterns, `玩家 schema v(\d+)`)
	case "世界 metadata 版本":
		patterns = append(patterns, `世界 metadata v(\d+)`)
	case "engine ABI":
		patterns = append(patterns, `engine C ABI (?:is currently |currently |当前为 )?v(\d+)`)
	case "client ABI":
		patterns = append(patterns, `client C ABI (?:is currently |currently |当前为 )?v(\d+)`)
	case "benchmark scenario":
		patterns = append(patterns, `benchmark scenario 为 v(\d+)`)
	}
	return patterns
}

func documentationCommandClaims(text string) []string {
	text = stripHistoricalMarkdownSections(text)
	var commands []string
	inFence := false
	for _, line := range strings.Split(text, "\n") {
		trimmed := strings.TrimSpace(line)
		if strings.HasPrefix(trimmed, "```") {
			inFence = !inFence
			continue
		}
		if inFence && isDocumentationCommand(trimmed) {
			commands = append(commands, normalizeDocumentationCommand(trimmed))
		}
		for _, match := range inlineCodePattern.FindAllStringSubmatch(line, -1) {
			candidate := strings.TrimSpace(match[1])
			if isDocumentationCommand(candidate) {
				commands = append(commands, normalizeDocumentationCommand(candidate))
			}
		}
	}
	slices.Sort(commands)
	return slices.Compact(commands)
}

func normalizeDocumentationCommand(text string) string {
	if command, _, found := strings.Cut(text, " #"); found {
		text = command
	}
	fields := strings.Fields(text)
	for index := 0; index+1 < len(fields); index++ {
		if fields[index] == "--name" {
			fields[index+1] = "<localized-name>"
		}
	}
	return strings.Join(fields, " ")
}

func isDocumentationCommand(text string) bool {
	for _, prefix := range []string{
		"$openspec-",
		"./scripts/",
		"GATES_",
		"VISUAL_OUT=",
		"corepack ",
		"gh ",
		"git ",
		"go ",
		"make ",
		"node ",
		"npm ",
		"npx ",
		"openspec ",
		"scripts/",
	} {
		if strings.HasPrefix(text, prefix) {
			return true
		}
	}
	return false
}

func stripHistoricalMarkdownSections(text string) string {
	lines := strings.Split(text, "\n")
	var output []string
	skipLevel := 0
	for _, line := range lines {
		level, heading := markdownHeading(line)
		if level > 0 {
			if skipLevel > 0 && level <= skipLevel {
				skipLevel = 0
			}
			lower := strings.ToLower(heading)
			if strings.Contains(lower, "historical") || strings.Contains(heading, "历史") {
				skipLevel = level
				continue
			}
		}
		if skipLevel == 0 {
			output = append(output, line)
		}
	}
	return strings.Join(output, "\n")
}

func markdownHeading(line string) (int, string) {
	trimmed := strings.TrimLeft(line, " ")
	count := 0
	for count < len(trimmed) && trimmed[count] == '#' {
		count++
	}
	if count == 0 || count >= len(trimmed) || trimmed[count] != ' ' {
		return 0, ""
	}
	return count, strings.TrimSpace(trimmed[count+1:])
}

func TestCurrentDocumentationVersionPatternCapturesOneValue(t *testing.T) {
	for _, mapping := range baselineVersionMappings {
		for _, pattern := range currentDocumentationVersionPatterns(mapping) {
			compiled := regexp.MustCompile(pattern)
			if compiled.NumSubexp() != 1 {
				t.Errorf("%s pattern %q has %d capture groups, want 1", mapping.name, pattern, compiled.NumSubexp())
			}
		}
	}
}

func Example_stripHistoricalMarkdownSections() {
	text := "## Current\nprotocol v44\n\n## Historical\nprotocol v1\n"
	stripped := stripHistoricalMarkdownSections(text)
	fmt.Println(strings.Contains(stripped, "v44"), strings.Contains(stripped, "v1"))
	// Output: true false
}

func parsePositiveVersion(text string) (int, error) {
	value, err := strconv.Atoi(text)
	if err != nil || value <= 0 {
		return 0, fmt.Errorf("invalid positive version %q", text)
	}
	return value, nil
}
