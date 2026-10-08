package archcheck_test

import (
	"crypto/sha256"
	"encoding/hex"
	"encoding/json"
	"fmt"
	"io/fs"
	"os"
	"os/exec"
	"path/filepath"
	"slices"
	"strings"
	"testing"
	"unicode"
)

const (
	openSpecLanguageMigrationPath = "testdata/audit/english-openspec-migration.json"
	updateOpenSpecLanguageDebtEnv = "MORNLEA_UPDATE_OPENSPEC_LANGUAGE_BASELINE"
)

type planningLanguageMigration struct {
	SchemaVersion int                        `json:"schema_version"`
	TotalFiles    int                        `json:"total_files"`
	TotalLines    int                        `json:"total_lines"`
	Files         []planningLanguageDebtFile `json:"files"`
}

type planningLanguageDebtFile struct {
	Path   string `json:"path"`
	Count  int    `json:"count"`
	Digest string `json:"digest"`
}

func TestOpenSpecLanguage(t *testing.T) {
	root := repositoryRoot(t)
	paths := activeOpenSpecMarkdown(t, root)
	assertEnglishPlanningProse(t, root, paths)
}

func TestPlanLanguage(t *testing.T) {
	root := repositoryRoot(t)
	manifest := readDocumentationManifest(t, root)
	changed := changedRepositoryPaths(t, root)
	var paths []string
	for _, entry := range manifest.Documents {
		if entry.Classification != "plan" {
			continue
		}
		for _, path := range entry.Paths {
			if changed[path] {
				paths = append(paths, path)
			}
		}
	}
	assertEnglishPlanningProse(t, root, paths)
}

func TestOpenSpecLanguageClassifierExcludesOnlyCodeAndLocalizedLiterals(t *testing.T) {
	text := "English prose with `中文标识符`.\n\n```text\n中文 fixture\n```\n\nlocalized(\"中文界面文字\")\n"
	if lines := nonEnglishMarkdownProseLines(text); len(lines) != 0 {
		t.Fatalf("code and explicit localized literals must be excluded: %v", lines)
	}
	text += "Normative 中文 prose must fail.\n"
	lines := nonEnglishMarkdownProseLines(text)
	if len(lines) != 1 || !strings.Contains(lines[0], "Normative") {
		t.Fatalf("normative Han prose lines = %v", lines)
	}
}

func TestOpenSpecLanguageDebt(t *testing.T) {
	root := repositoryRoot(t)
	current := currentOpenSpecLanguageDebt(t, root)
	path := filepath.Join(root, filepath.FromSlash(openSpecLanguageMigrationPath))
	baseline, err := readPlanningLanguageMigration(path)
	if err != nil && !os.IsNotExist(err) {
		t.Fatal(err)
	}
	if os.Getenv(updateOpenSpecLanguageDebtEnv) == "1" {
		if err == nil {
			if problems := comparePlanningLanguageDebt(baseline, current, true); len(problems) > 0 {
				t.Fatalf("refuse to increase or replace canonical OpenSpec language debt:\n%s", strings.Join(problems, "\n"))
			}
		}
		writePlanningLanguageMigration(t, path, current)
		return
	}
	if os.IsNotExist(err) {
		t.Fatalf("%s is missing; create the initial inventory only with %s=1", openSpecLanguageMigrationPath, updateOpenSpecLanguageDebtEnv)
	}
	if problems := comparePlanningLanguageDebt(baseline, current, false); len(problems) > 0 {
		t.Fatalf("canonical OpenSpec language debt changed:\n%s\ntranslate the affected specification and update a decreasing baseline with %s=1", strings.Join(problems, "\n"), updateOpenSpecLanguageDebtEnv)
	}
}

func TestOpenSpecLanguageDebtRejectsGrowthAndReplacement(t *testing.T) {
	baseline := planningLanguageMigration{
		SchemaVersion: 1,
		TotalFiles:    1,
		TotalLines:    2,
		Files:         []planningLanguageDebtFile{{Path: "openspec/specs/example/spec.md", Count: 2, Digest: "old"}},
	}
	tests := []struct {
		name    string
		current planningLanguageMigration
		update  bool
		want    string
	}{
		{
			name: "new debt path",
			current: planningLanguageMigration{SchemaVersion: 1, TotalFiles: 2, TotalLines: 3, Files: []planningLanguageDebtFile{
				{Path: "openspec/specs/example/spec.md", Count: 2, Digest: "old"},
				{Path: "openspec/specs/new/spec.md", Count: 1, Digest: "new"},
			}},
			update: true,
			want:   "new non-English planning debt",
		},
		{
			name:    "same count replacement",
			current: planningLanguageMigration{SchemaVersion: 1, TotalFiles: 1, TotalLines: 2, Files: []planningLanguageDebtFile{{Path: "openspec/specs/example/spec.md", Count: 2, Digest: "different"}}},
			update:  true,
			want:    "changed without decreasing",
		},
		{
			name:    "decrease requires update",
			current: planningLanguageMigration{SchemaVersion: 1, TotalFiles: 1, TotalLines: 1, Files: []planningLanguageDebtFile{{Path: "openspec/specs/example/spec.md", Count: 1, Digest: "smaller"}}},
			want:    "decreased from 2 to 1",
		},
	}
	for _, test := range tests {
		t.Run(test.name, func(t *testing.T) {
			problems := strings.Join(comparePlanningLanguageDebt(baseline, test.current, test.update), "\n")
			if !strings.Contains(problems, test.want) {
				t.Fatalf("problems = %q, want fragment %q", problems, test.want)
			}
		})
	}
	decreased := planningLanguageMigration{SchemaVersion: 1, TotalFiles: 1, TotalLines: 1, Files: []planningLanguageDebtFile{{Path: "openspec/specs/example/spec.md", Count: 1, Digest: "smaller"}}}
	if problems := comparePlanningLanguageDebt(baseline, decreased, true); len(problems) != 0 {
		t.Fatalf("decreasing update rejected: %v", problems)
	}
}

func TestOpenSpecLanguageGrandfatherAllowsUnchangedDebt(t *testing.T) {
	baseline := planningLanguageMigration{
		SchemaVersion: 1,
		TotalFiles:    1,
		TotalLines:    2,
		Files:         []planningLanguageDebtFile{{Path: "openspec/specs/example/spec.md", Count: 2, Digest: "unchanged-prose"}},
	}
	for _, allowDecrease := range []bool{false, true} {
		if problems := comparePlanningLanguageDebt(baseline, baseline, allowDecrease); len(problems) != 0 {
			t.Fatalf("unchanged grandfathered debt rejected with allowDecrease=%t: %v", allowDecrease, problems)
		}
	}
}

func TestHistoricalPlanningExemption(t *testing.T) {
	root := t.TempDir()
	writePlanningFixture(t, root, "openspec/changes/active/proposal.md", "English active prose.\n")
	writePlanningFixture(t, root, "openspec/changes/archive/old/proposal.md", "历史中文证据。\n")
	paths := activeOpenSpecMarkdown(t, root)
	if !slices.Equal(paths, []string{"openspec/changes/active/proposal.md"}) {
		t.Fatalf("active paths = %v", paths)
	}
	if findings := planningLanguageFindings(t, root, paths); len(findings) != 0 {
		t.Fatalf("archived evidence entered the active scan: %v", findings)
	}
}

func assertEnglishPlanningProse(t *testing.T, root string, paths []string) {
	t.Helper()
	if findings := planningLanguageFindings(t, root, paths); len(findings) != 0 {
		t.Fatalf("planning artifacts contain non-English prose:\n%s", strings.Join(findings, "\n"))
	}
}

func planningLanguageFindings(t *testing.T, root string, paths []string) []string {
	t.Helper()
	var findings []string
	for _, relative := range paths {
		data, err := os.ReadFile(filepath.Join(root, filepath.FromSlash(relative)))
		if err != nil {
			t.Fatal(err)
		}
		for _, line := range nonEnglishMarkdownProseLines(string(data)) {
			findings = append(findings, relative+": "+line)
		}
	}
	slices.Sort(findings)
	return findings
}

func nonEnglishMarkdownProseLines(text string) []string {
	var findings []string
	inFence := false
	for lineNumber, line := range strings.Split(text, "\n") {
		trimmed := strings.TrimSpace(line)
		if strings.HasPrefix(trimmed, "```") || strings.HasPrefix(trimmed, "~~~") {
			inFence = !inFence
			continue
		}
		if inFence {
			continue
		}
		prose := inlineCodePattern.ReplaceAllString(line, "")
		prose = removeExplicitLocalizedLiterals(prose)
		if containsHanRune(prose) {
			findings = append(findings, fmt.Sprintf("line %d: %s", lineNumber+1, strings.TrimSpace(line)))
		}
	}
	return findings
}

func removeExplicitLocalizedLiterals(line string) string {
	for {
		start := strings.Index(line, "localized(\"")
		if start < 0 {
			return line
		}
		rest := line[start+len("localized(\""):]
		end := strings.Index(rest, "\")")
		if end < 0 {
			return line
		}
		line = line[:start] + rest[end+2:]
	}
}

func containsHanRune(text string) bool {
	for _, char := range text {
		if unicode.In(char, unicode.Han) {
			return true
		}
	}
	return false
}

func activeOpenSpecMarkdown(t *testing.T, root string) []string {
	t.Helper()
	changesRoot := filepath.Join(root, "openspec", "changes")
	// Only the top-level archive holds frozen changes. Directories that merely
	// share the name, such as `archive-foo` or an `archive/` folder inside an
	// active change, remain current planning content and stay in scope.
	archiveRoot := filepath.Join(changesRoot, "archive")
	var paths []string
	err := filepath.WalkDir(changesRoot, func(path string, entry fs.DirEntry, walkErr error) error {
		if walkErr != nil {
			return walkErr
		}
		if entry.IsDir() {
			if path == archiveRoot {
				return filepath.SkipDir
			}
			return nil
		}
		if strings.HasSuffix(entry.Name(), ".md") {
			relative, err := filepath.Rel(root, path)
			if err != nil {
				return err
			}
			paths = append(paths, filepath.ToSlash(relative))
		}
		return nil
	})
	if err != nil {
		t.Fatalf("enumerate active OpenSpec artifacts: %v", err)
	}
	slices.Sort(paths)
	return paths
}

func changedRepositoryPaths(t *testing.T, root string) map[string]bool {
	t.Helper()
	changed := make(map[string]bool)
	for _, args := range [][]string{{"diff", "--name-only", "HEAD", "--"}, {"ls-files", "--others", "--exclude-standard"}} {
		command := exec.Command("git", args...)
		command.Dir = root
		output, err := command.Output()
		if err != nil {
			t.Fatalf("git %s: %v", strings.Join(args, " "), err)
		}
		for _, path := range strings.Split(strings.TrimSpace(string(output)), "\n") {
			if path != "" {
				changed[filepath.ToSlash(path)] = true
			}
		}
	}
	return changed
}

func currentOpenSpecLanguageDebt(t *testing.T, root string) planningLanguageMigration {
	t.Helper()
	specRoot := filepath.Join(root, "openspec", "specs")
	var paths []string
	if err := filepath.WalkDir(specRoot, func(path string, entry fs.DirEntry, walkErr error) error {
		if walkErr != nil {
			return walkErr
		}
		if !entry.IsDir() && strings.HasSuffix(entry.Name(), ".md") {
			relative, err := filepath.Rel(root, path)
			if err != nil {
				return err
			}
			paths = append(paths, filepath.ToSlash(relative))
		}
		return nil
	}); err != nil {
		t.Fatalf("enumerate canonical OpenSpec artifacts: %v", err)
	}
	slices.Sort(paths)
	migration := planningLanguageMigration{SchemaVersion: 1}
	for _, path := range paths {
		data, err := os.ReadFile(filepath.Join(root, filepath.FromSlash(path)))
		if err != nil {
			t.Fatal(err)
		}
		lines := nonEnglishMarkdownProseLines(string(data))
		if len(lines) == 0 {
			continue
		}
		digest := sha256.Sum256([]byte(strings.Join(lines, "\n")))
		migration.Files = append(migration.Files, planningLanguageDebtFile{Path: path, Count: len(lines), Digest: hex.EncodeToString(digest[:])})
		migration.TotalLines += len(lines)
	}
	migration.TotalFiles = len(migration.Files)
	return migration
}

func comparePlanningLanguageDebt(baseline, current planningLanguageMigration, allowDecrease bool) []string {
	if baseline.SchemaVersion != 1 {
		return []string{fmt.Sprintf("baseline schema_version = %d, want 1", baseline.SchemaVersion)}
	}
	if current.SchemaVersion != 1 {
		return []string{fmt.Sprintf("current schema_version = %d, want 1", current.SchemaVersion)}
	}
	baselineFiles := planningDebtByPath(baseline.Files)
	currentFiles := planningDebtByPath(current.Files)
	var problems []string
	for path, currentFile := range currentFiles {
		baselineFile, exists := baselineFiles[path]
		if !exists {
			problems = append(problems, fmt.Sprintf("%s has new non-English planning debt (%d lines)", path, currentFile.Count))
			continue
		}
		switch {
		case currentFile.Count > baselineFile.Count:
			problems = append(problems, fmt.Sprintf("%s increased from %d to %d lines", path, baselineFile.Count, currentFile.Count))
		case currentFile.Count == baselineFile.Count && currentFile.Digest != baselineFile.Digest:
			problems = append(problems, fmt.Sprintf("%s planning debt changed without decreasing", path))
		case currentFile.Count < baselineFile.Count && !allowDecrease:
			problems = append(problems, fmt.Sprintf("%s decreased from %d to %d lines but the checked-in baseline was not ratcheted", path, baselineFile.Count, currentFile.Count))
		}
	}
	for path, baselineFile := range baselineFiles {
		if _, exists := currentFiles[path]; !exists && !allowDecrease {
			problems = append(problems, fmt.Sprintf("%s decreased from %d to 0 lines but the checked-in baseline was not ratcheted", path, baselineFile.Count))
		}
	}
	if !allowDecrease {
		if baseline.TotalFiles != current.TotalFiles {
			problems = append(problems, fmt.Sprintf("total_files changed from %d to %d", baseline.TotalFiles, current.TotalFiles))
		}
		if baseline.TotalLines != current.TotalLines {
			problems = append(problems, fmt.Sprintf("total_lines changed from %d to %d", baseline.TotalLines, current.TotalLines))
		}
	}
	slices.Sort(problems)
	return problems
}

func planningDebtByPath(files []planningLanguageDebtFile) map[string]planningLanguageDebtFile {
	byPath := make(map[string]planningLanguageDebtFile, len(files))
	for _, file := range files {
		byPath[file.Path] = file
	}
	return byPath
}

func readPlanningLanguageMigration(path string) (planningLanguageMigration, error) {
	data, err := os.ReadFile(path)
	if err != nil {
		return planningLanguageMigration{}, err
	}
	var migration planningLanguageMigration
	if err := json.Unmarshal(data, &migration); err != nil {
		return planningLanguageMigration{}, fmt.Errorf("decode %s: %w", path, err)
	}
	return migration, nil
}

func writePlanningLanguageMigration(t *testing.T, path string, migration planningLanguageMigration) {
	t.Helper()
	data, err := json.MarshalIndent(migration, "", "  ")
	if err != nil {
		t.Fatal(err)
	}
	data = append(data, '\n')
	if err := os.MkdirAll(filepath.Dir(path), 0o755); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(path, data, 0o644); err != nil {
		t.Fatal(err)
	}
}

func writePlanningFixture(t *testing.T, root, relative, content string) {
	t.Helper()
	path := filepath.Join(root, filepath.FromSlash(relative))
	if err := os.MkdirAll(filepath.Dir(path), 0o755); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(path, []byte(content), 0o644); err != nil {
		t.Fatal(err)
	}
}
