package archcheck_test

import (
	"fmt"
	"os"
	"path/filepath"
	"regexp"
	"slices"
	"strings"
	"testing"
)

// compatibilityMatrixDocuments are the bilingual pair that is the only
// explanatory location allowed to state current contract versions. Other notes
// link to it so that a version bump has exactly one prose location to update.
// The table rows use the same English identities as the root `AGENTS.md`
// baseline sentence in both languages, so each row resolves through an existing
// `baselineVersionMappings` document pattern instead of a second label table.
var compatibilityMatrixDocuments = []struct {
	path    string
	heading string
}{
	{path: "docs/notes/compatibility.md", heading: "## Current version matrix"},
	{path: "docs/notes/compatibility.zh.md", heading: "## 当前版本矩阵"},
}

// compatibilityMatrixDocumentID identifies the matrix pair in
// docs/documentation-manifest.json; it is excluded from the hardcoded-version
// guard that covers every other bilingual note.
const compatibilityMatrixDocumentID = "compatibility-guide"

// compatibilityMatrixRustMirrors lists Rust constants that must equal the Go
// or C-header authority for the same contract. The Rust storage and FFI crates
// are independent ports, so a drift between them is a real compatibility bug
// rather than a documentation typo.
var compatibilityMatrixRustMirrors = map[string]struct {
	sourcePath  string
	codePattern string
}{
	"玩家 schema":            {"packages/engine/crates/mornlea_storage/src/player.rs", `pub const CURRENT_SCHEMA: u32 = (\d+);`},
	"区块 schema":            {"packages/engine/crates/mornlea_storage/src/chunk.rs", `pub const CURRENT_SCHEMA: u32 = (\d+);`},
	"companions.ai schema": {"packages/engine/crates/mornlea_storage/src/companion.rs", `pub const CURRENT_SCHEMA: u32 = (\d+);`},
	"hostile_mobs schema":  {"packages/engine/crates/mornlea_storage/src/hostile.rs", `pub const CURRENT_SCHEMA: u32 = (\d+);`},
	"passive_mobs schema":  {"packages/engine/crates/mornlea_storage/src/passive.rs", `pub const CURRENT_SCHEMA: u32 = (\d+);`},
	"engine ABI":           {"packages/engine/crates/mornlea_engine/src/ffi.rs", `pub\(crate\) const ABI_VERSION: u32 = (\d+);`},
	"client ABI":           {"packages/engine/crates/mornlea_client/src/ffi.rs", `pub const CLIENT_ABI_VERSION: u32 = (\d+);`},
}

// hardcodedVersionClaimPattern catches current baseline-version prose in
// English or Chinese: a protocol, ABI, metadata, or scenario keyword followed by
// a version, with or without a copula, and a baseline save name followed by a
// schema version. Schemas outside the baseline matrix, such as the
// configuration-file schema, are not matched. The drift test holds concrete
// examples in both languages.
var hardcodedVersionClaimPattern = regexp.MustCompile("(?i)(protocol|协议|ABI|metadata|scenario)\\s*(为|is)?\\s*v\\d+" +
	"|(player|chunk|玩家|区块|companions\\.ai`?|hostile_mobs`?|passive_mobs`?)[^\\n，。；;]{0,12}schema\\s*(为|is)?\\s*v\\d+")

type compatibilityMatrixRow struct {
	label   string
	version string
	source  string
}

func TestCompatibilityMatrixMatchesSource(t *testing.T) {
	root := repositoryRoot(t)
	var canonical []compatibilityMatrixRow
	for index, document := range compatibilityMatrixDocuments {
		rows, err := parseCompatibilityMatrix(readBaselineDoc(t, root, document.path), document.heading)
		if err != nil {
			t.Fatalf("%s: %v", document.path, err)
		}
		for _, problem := range compatibilityMatrixProblems(root, rows) {
			t.Errorf("%s: %s", document.path, problem)
		}
		if index == 0 {
			canonical = rows
			continue
		}
		if !slices.Equal(compatibilityMatrixClaims(rows), compatibilityMatrixClaims(canonical)) {
			t.Errorf("%s matrix %v differs from %s matrix %v", document.path, compatibilityMatrixClaims(rows), compatibilityMatrixDocuments[0].path, compatibilityMatrixClaims(canonical))
		}
	}
}

// TestBilingualNotesDoNotHardcodeVersions covers every migrated note except the
// matrix pair. Legacy notes are exempt until their mandatory bilingual
// migration, at which point this guard applies without further edits.
func TestBilingualNotesDoNotHardcodeVersions(t *testing.T) {
	root := repositoryRoot(t)
	manifest := readDocumentationManifest(t, root)
	for _, entry := range manifest.Documents {
		if entry.Classification != "bilingual" || entry.ID == compatibilityMatrixDocumentID {
			continue
		}
		for _, relative := range entry.Paths {
			if !strings.HasPrefix(relative, "docs/notes/") {
				continue
			}
			text := stripHistoricalMarkdownSections(readBaselineDoc(t, root, relative))
			for _, problem := range hardcodedVersionClaims(relative, text) {
				t.Error(problem)
			}
		}
	}
}

func TestCompatibilityMatrixGuardDetectsDrift(t *testing.T) {
	root := repositoryRoot(t)
	canonical := compatibilityMatrixDocuments[0]
	valid, err := parseCompatibilityMatrix(readBaselineDoc(t, root, canonical.path), canonical.heading)
	if err != nil {
		t.Fatalf("parse current matrix: %v", err)
	}
	if problems := compatibilityMatrixProblems(root, valid); len(problems) != 0 {
		t.Fatalf("current matrix must be valid before mutation: %v", problems)
	}

	for name := range compatibilityMatrixRustMirrors {
		if !slices.ContainsFunc(baselineVersionMappings, func(mapping baselineVersionMapping) bool { return mapping.name == name }) {
			t.Errorf("Rust mirror %q names no baseline contract and would never be checked", name)
		}
	}

	stale := slices.Clone(valid)
	stale[0].version = "v1"
	if problems := compatibilityMatrixProblems(root, stale); len(problems) != 1 || !strings.Contains(problems[0], "stale") {
		t.Errorf("stale version problems = %v, want exactly one stale diagnostic", problems)
	}

	missing := slices.Clone(valid[1:])
	if problems := compatibilityMatrixProblems(root, missing); len(problems) != 1 || !strings.Contains(problems[0], "missing") {
		t.Errorf("missing row problems = %v, want exactly one missing diagnostic", problems)
	}

	unsourced := slices.Clone(valid)
	unsourced[0].source = "elsewhere"
	if problems := compatibilityMatrixProblems(root, unsourced); len(problems) != 1 || !strings.Contains(problems[0], "authority") {
		t.Errorf("unsourced row problems = %v, want exactly one authority diagnostic", problems)
	}

	if problems := hardcodedVersionClaims("sample.md", "当前线上协议为 v32。\n版本见 [兼容性](compatibility.md)。\nengine ABI v9\n玩家存档为 schema v8\n配置 schema 为 v1\n"); len(problems) != 3 {
		t.Errorf("hardcoded claim problems = %v, want three", problems)
	}
}

// parseCompatibilityMatrix reads the first Markdown table under the matrix
// heading. Each data row is `| identity | vN | authority sources |`.
func parseCompatibilityMatrix(text, heading string) ([]compatibilityMatrixRow, error) {
	lines := strings.Split(text, "\n")
	start := slices.Index(lines, heading)
	if start < 0 {
		return nil, fmt.Errorf("missing %q section", heading)
	}
	var rows []compatibilityMatrixRow
	inTable := false
	for _, line := range lines[start+1:] {
		trimmed := strings.TrimSpace(line)
		if strings.HasPrefix(trimmed, "## ") {
			break
		}
		if !strings.HasPrefix(trimmed, "|") {
			if inTable {
				break
			}
			continue
		}
		inTable = true
		cells := strings.Split(strings.Trim(trimmed, "|"), "|")
		if len(cells) != 3 {
			return nil, fmt.Errorf("matrix row %q has %d cells, want 3", trimmed, len(cells))
		}
		for index := range cells {
			cells[index] = strings.TrimSpace(cells[index])
		}
		if strings.Trim(cells[1], "-: ") == "" || !strings.HasPrefix(cells[1], "v") {
			continue // header or separator row
		}
		rows = append(rows, compatibilityMatrixRow{label: cells[0], version: cells[1], source: cells[2]})
	}
	if len(rows) == 0 {
		return nil, fmt.Errorf("%q section has no matrix rows", heading)
	}
	return rows, nil
}

// compatibilityMatrixProblems resolves every row through the baseline document
// patterns, compares it with the authoritative source constant and any Rust
// mirror, and requires exactly one row per baseline contract.
func compatibilityMatrixProblems(root string, rows []compatibilityMatrixRow) []string {
	var problems []string
	seen := make(map[string]int)
	for _, row := range rows {
		claim := strings.ReplaceAll(row.label, "`", "") + " " + row.version
		var matched *baselineVersionMapping
		for index := range baselineVersionMappings {
			mapping := &baselineVersionMappings[index]
			pattern := strings.ReplaceAll(mapping.docPattern, "`", "")
			if regexp.MustCompile(`^` + pattern + `$`).MatchString(claim) {
				matched = mapping
				break
			}
		}
		if matched == nil {
			problems = append(problems, fmt.Sprintf("matrix row %q matches no baseline contract", row.label))
			continue
		}
		seen[matched.name]++
		if !strings.Contains(row.source, filepath.ToSlash(matched.sourcePath)) {
			problems = append(problems, fmt.Sprintf("matrix row %q does not cite authority %s", row.label, filepath.ToSlash(matched.sourcePath)))
		}
		want, err := resolveSourceConstant(root, filepath.ToSlash(matched.sourcePath), matched.codePattern)
		if err != nil {
			problems = append(problems, err.Error())
			continue
		}
		if row.version != "v"+want {
			problems = append(problems, fmt.Sprintf("matrix row %q is stale: %s, want v%s from %s", row.label, row.version, want, filepath.ToSlash(matched.sourcePath)))
		}
		if mirror, ok := compatibilityMatrixRustMirrors[matched.name]; ok {
			got, err := resolveSourceConstant(root, mirror.sourcePath, mirror.codePattern)
			if err != nil {
				problems = append(problems, err.Error())
			} else if got != want {
				problems = append(problems, fmt.Sprintf("Rust mirror %s has %s v%s, want v%s from %s", mirror.sourcePath, matched.name, got, want, filepath.ToSlash(matched.sourcePath)))
			}
		}
	}
	for _, mapping := range baselineVersionMappings {
		switch seen[mapping.name] {
		case 0:
			problems = append(problems, fmt.Sprintf("matrix is missing %s", mapping.name))
		case 1:
		default:
			problems = append(problems, fmt.Sprintf("matrix repeats %s %d times", mapping.name, seen[mapping.name]))
		}
	}
	return problems
}

func compatibilityMatrixClaims(rows []compatibilityMatrixRow) []string {
	claims := make([]string, 0, len(rows))
	for _, row := range rows {
		claims = append(claims, row.label+"="+row.version)
	}
	return claims
}

func resolveSourceConstant(root, relative, pattern string) (string, error) {
	source, err := os.ReadFile(filepath.Join(root, filepath.FromSlash(relative)))
	if err != nil {
		return "", fmt.Errorf("read authoritative source %s: %v", relative, err)
	}
	value, err := resolveBaselineConstant(string(source), pattern)
	if err != nil {
		return "", fmt.Errorf("resolve %s: %v", relative, err)
	}
	return value, nil
}

func hardcodedVersionClaims(relative, text string) []string {
	var problems []string
	for index, line := range strings.Split(text, "\n") {
		if match := hardcodedVersionClaimPattern.FindString(line); match != "" {
			problems = append(problems, fmt.Sprintf("%s:%d hardcodes current version %q; link to %s instead", relative, index+1, match, compatibilityMatrixDocuments[0].path))
		}
	}
	return problems
}
