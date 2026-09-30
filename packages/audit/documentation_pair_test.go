package archcheck_test

import (
	"fmt"
	"os"
	"path/filepath"
	"slices"
	"strings"
	"testing"

	"gopkg.in/yaml.v3"
)

type documentationFrontMatter struct {
	DocumentID  string `yaml:"doc_id"`
	Revision    string `yaml:"doc_revision"`
	Language    string `yaml:"language"`
	Counterpart string `yaml:"counterpart"`
}

func TestCompletedDocumentationPairsAreSynchronized(t *testing.T) {
	root := repositoryRoot(t)
	manifest := readDocumentationManifest(t, root)
	for _, entry := range manifest.Documents {
		if entry.Classification != "bilingual" {
			continue
		}
		if !slices.Contains(entry.Paths, entry.EnglishPath) || !slices.Contains(entry.Paths, entry.ChinesePath) {
			// Pair migration is ratcheted by adding both target paths atomically. Once
			// declared current, the pair becomes mandatory and cannot drift.
			continue
		}
		t.Run(entry.ID, func(t *testing.T) {
			for _, problem := range validateDocumentationPair(root, entry) {
				t.Error(problem)
			}
		})
	}
}

func TestDocumentationPairValidationRejectsMissingAndMismatchedMetadata(t *testing.T) {
	root := t.TempDir()
	entry := documentationManifestEntry{
		ID:             "sample",
		Classification: "bilingual",
		Paths:          []string{"docs/sample.md", "docs/sample.zh.md"},
		EnglishPath:    "docs/sample.md",
		ChinesePath:    "docs/sample.zh.md",
		Revision:       "2026-09-15.1",
	}
	writePairFixture(t, root, entry.EnglishPath, `---
doc_id: sample
doc_revision: 2026-09-15.1
language: en
counterpart: sample.zh.md
---
# Sample
`)

	if problems := validateDocumentationPair(root, entry); len(problems) == 0 {
		t.Fatal("missing Chinese counterpart must fail")
	}

	writePairFixture(t, root, entry.ChinesePath, `---
doc_id: other
doc_revision: 2026-09-15.2
language: en
counterpart: missing.md
---
# 样例
`)
	problems := strings.Join(validateDocumentationPair(root, entry), "\n")
	for _, want := range []string{"doc_id", "doc_revision", "language", "counterpart"} {
		if !strings.Contains(problems, want) {
			t.Errorf("mismatched pair problems must mention %s:\n%s", want, problems)
		}
	}
}

func TestPublicRootReadmeOmitsFrontMatter(t *testing.T) {
	root := t.TempDir()
	entry := documentationManifestEntry{
		ID:             "root-readme",
		Classification: "bilingual",
		Paths:          []string{"README.md", "README.zh.md"},
		EnglishPath:    "README.md",
		ChinesePath:    "README.zh.md",
		Revision:       "2026-09-30.1",
	}
	writePairFixture(t, root, entry.EnglishPath, "# Mornlea\n")
	writePairFixture(t, root, entry.ChinesePath, "# Mornlea\n")
	if problems := validateDocumentationPair(root, entry); len(problems) != 0 {
		t.Fatalf("public README pair without front matter was rejected:\n%s", strings.Join(problems, "\n"))
	}
}

func TestDocumentationPairValidationAcceptsRelativeCounterparts(t *testing.T) {
	root := t.TempDir()
	entry := documentationManifestEntry{
		ID:             "sample",
		Classification: "bilingual",
		Paths:          []string{"docs/sample.md", "docs/sample.zh.md"},
		EnglishPath:    "docs/sample.md",
		ChinesePath:    "docs/sample.zh.md",
		Revision:       "2026-09-15.1",
	}
	writePairFixture(t, root, entry.EnglishPath, `---
doc_id: sample
doc_revision: 2026-09-15.1
language: en
counterpart: sample.zh.md
---
# Sample
`)
	writePairFixture(t, root, entry.ChinesePath, `---
doc_id: sample
doc_revision: 2026-09-15.1
language: zh-CN
counterpart: sample.md
---
# 样例
`)
	if problems := validateDocumentationPair(root, entry); len(problems) != 0 {
		t.Fatalf("valid pair rejected:\n%s", strings.Join(problems, "\n"))
	}
}

func TestDocumentationPairValidationRejectsWrongLanguageSuffixes(t *testing.T) {
	root := t.TempDir()
	entry := documentationManifestEntry{
		ID:             "sample",
		Classification: "bilingual",
		Paths:          []string{"docs/sample.en.md", "docs/sample.md"},
		EnglishPath:    "docs/sample.en.md",
		ChinesePath:    "docs/sample.md",
		Revision:       "2026-09-15.1",
	}
	writePairFixture(t, root, entry.EnglishPath, `---
doc_id: sample
doc_revision: 2026-09-15.1
language: en
counterpart: sample.md
---
# Sample
`)
	writePairFixture(t, root, entry.ChinesePath, `---
doc_id: sample
doc_revision: 2026-09-15.1
language: zh-CN
counterpart: sample.en.md
---
# Sample Chinese counterpart
`)
	problems := strings.Join(validateDocumentationPair(root, entry), "\n")
	for _, want := range []string{"unsuffixed *.md", "*.zh.md"} {
		if !strings.Contains(problems, want) {
			t.Errorf("pair problems must mention %q:\n%s", want, problems)
		}
	}
}

func TestDocumentationManifestForbidsEnglishSuffix(t *testing.T) {
	manifest := readDocumentationManifest(t, repositoryRoot(t))
	for _, entry := range manifest.Documents {
		for _, path := range entry.Paths {
			if strings.HasSuffix(path, ".en.md") {
				t.Errorf("English-suffixed documentation path %q is forbidden", path)
			}
		}
	}
}

func validateDocumentationPair(root string, entry documentationManifestEntry) []string {
	var problems []string
	if !isEnglishCanonicalMarkdown(entry.EnglishPath) {
		problems = append(problems, fmt.Sprintf("%s must be an unsuffixed *.md English canonical path", entry.EnglishPath))
	}
	if !strings.HasSuffix(entry.ChinesePath, ".zh.md") {
		problems = append(problems, fmt.Sprintf("%s must use the *.zh.md Chinese counterpart suffix", entry.ChinesePath))
	}
	// The repository landing pages are the public GitHub README. They stay a
	// bilingual pair under semantic checks, but they do not carry the internal
	// YAML header used by long-lived docs.
	if entry.ID == "root-readme" {
		for _, relative := range []string{entry.EnglishPath, entry.ChinesePath} {
			if _, err := os.Stat(filepath.Join(root, filepath.FromSlash(relative))); err != nil {
				problems = append(problems, fmt.Sprintf("read %s: %v", relative, err))
			}
		}
		return problems
	}
	english, englishProblems := readDocumentationFrontMatter(root, entry.EnglishPath)
	chinese, chineseProblems := readDocumentationFrontMatter(root, entry.ChinesePath)
	metadataProblems := append(englishProblems, chineseProblems...)
	problems = append(problems, metadataProblems...)
	if len(metadataProblems) != 0 {
		return problems
	}

	for path, metadata := range map[string]documentationFrontMatter{
		entry.EnglishPath: english,
		entry.ChinesePath: chinese,
	} {
		if metadata.DocumentID != entry.ID {
			problems = append(problems, fmt.Sprintf("%s doc_id = %q, want %q", path, metadata.DocumentID, entry.ID))
		}
		if metadata.Revision != entry.Revision {
			problems = append(problems, fmt.Sprintf("%s doc_revision = %q, want %q", path, metadata.Revision, entry.Revision))
		}
	}
	if english.Language != "en" {
		problems = append(problems, fmt.Sprintf("%s language = %q, want en", entry.EnglishPath, english.Language))
	}
	if chinese.Language != "zh-CN" {
		problems = append(problems, fmt.Sprintf("%s language = %q, want zh-CN", entry.ChinesePath, chinese.Language))
	}
	if resolvedCounterpart(entry.EnglishPath, english.Counterpart) != entry.ChinesePath {
		problems = append(problems, fmt.Sprintf("%s counterpart = %q, want %q", entry.EnglishPath, english.Counterpart, entry.ChinesePath))
	}
	if resolvedCounterpart(entry.ChinesePath, chinese.Counterpart) != entry.EnglishPath {
		problems = append(problems, fmt.Sprintf("%s counterpart = %q, want %q", entry.ChinesePath, chinese.Counterpart, entry.EnglishPath))
	}
	return problems
}

func readDocumentationFrontMatter(root, relative string) (documentationFrontMatter, []string) {
	path := filepath.Join(root, filepath.FromSlash(relative))
	data, err := os.ReadFile(path)
	if err != nil {
		return documentationFrontMatter{}, []string{fmt.Sprintf("read %s: %v", relative, err)}
	}
	text := string(data)
	if !strings.HasPrefix(text, "---\n") {
		return documentationFrontMatter{}, []string{fmt.Sprintf("%s has no YAML front matter", relative)}
	}
	rest := strings.TrimPrefix(text, "---\n")
	end := strings.Index(rest, "\n---\n")
	if end < 0 {
		return documentationFrontMatter{}, []string{fmt.Sprintf("%s has unterminated YAML front matter", relative)}
	}
	var metadata documentationFrontMatter
	if err := yaml.Unmarshal([]byte(rest[:end]), &metadata); err != nil {
		return documentationFrontMatter{}, []string{fmt.Sprintf("decode %s front matter: %v", relative, err)}
	}
	return metadata, nil
}

func resolvedCounterpart(ownerPath, counterpart string) string {
	if counterpart == "" {
		return ""
	}
	if strings.Contains(counterpart, "/") {
		return filepath.ToSlash(filepath.Clean(counterpart))
	}
	return filepath.ToSlash(filepath.Join(filepath.Dir(ownerPath), counterpart))
}

func writePairFixture(t *testing.T, root, relative, content string) {
	t.Helper()
	path := filepath.Join(root, filepath.FromSlash(relative))
	if err := os.MkdirAll(filepath.Dir(path), 0o700); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(path, []byte(content), 0o600); err != nil {
		t.Fatal(err)
	}
}
