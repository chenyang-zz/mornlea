package main

import (
	"bytes"
	"crypto/sha256"
	"encoding/json"
	"fmt"
	"io/fs"
	"os"
	"path/filepath"
	"strconv"
	"strings"
	"testing"
)

const (
	// runtimeOracleExportDirEnv names the harness-owned directory explicit
	// fixture export writes into. There is no repository default: an unset
	// variable means nothing is exported, so executed evidence never lands in
	// the tree by accident.
	runtimeOracleExportDirEnv = "RUNTIME_ORACLE_EXPORT_DIR"
)

// generatedAsset pairs a relative slash path with raw bytes.
type generatedAsset struct {
	RelativePath string
	Data         []byte
}

var validProducerIDs = map[string]bool{
	"runtime-oracle/protocol-frame":                 true,
	"runtime-oracle/protocol-negotiation":           true,
	"runtime-oracle/protocol-control":               true,
	"runtime-oracle/protocol-client-control":        true,
	"runtime-oracle/protocol-client-rays":           true,
	"runtime-oracle/protocol-client-inventory":      true,
	"runtime-oracle/protocol-client-stack-views":    true,
	"runtime-oracle/protocol-client-chat":           true,
	"runtime-oracle/protocol-world-delta":           true,
	"runtime-oracle/protocol-snapshot":              true,
	"runtime-oracle/protocol-player-outcomes":       true,
	"runtime-oracle/protocol-inventory-publication": true,
	"runtime-oracle/protocol-remote-players":        true,
	"runtime-oracle/protocol-companions":            true,
	"runtime-oracle/protocol-drops":                 true,
	"runtime-oracle/protocol-hostiles":              true,
	"runtime-oracle/protocol-passives":              true,
	"runtime-oracle/protocol-projectiles":           true,
	"runtime-oracle/protocol-chat-event":            true,
	"runtime-oracle/domain-identity-values":         true,
	"runtime-oracle/domain-values":                  true,
	"runtime-oracle/domain-command-control":         true,
	"runtime-oracle/domain-command-inventory":       true,
	"runtime-oracle/domain-event-player":            true,
	"runtime-oracle/domain-event-world":             true,
	"runtime-oracle/domain-event-inventory":         true,
	"runtime-oracle/domain-event-people":            true,
	"runtime-oracle/domain-event-mobs":              true,
	"runtime-oracle/domain-event-objects":           true,
	"runtime-oracle/domain-event-chat":              true,
	"runtime-oracle/domain-event-manifest":          true,
	"runtime-oracle/storage-player":                 true,
	"runtime-oracle/storage-chunk":                  true,
	"runtime-oracle/storage-region":                 true,
	"runtime-oracle/storage-world-metadata":         true,
	"runtime-oracle/storage-hostile":                true,
	"runtime-oracle/storage-passive":                true,
	"runtime-oracle/storage-companion":              true,
	"storage/metadata":                              true,
	"chunk/migration":                               true,
	"companion/agent-contract":                      true,
}

// validateProducerID and validateGeneratedAssets complete all deterministic
// input checks before export directories can be created.
func validateProducerID(producerID string) ([]string, error) {
	if strings.TrimSpace(producerID) == "" || strings.Contains(producerID, "\\") || filepath.IsAbs(producerID) {
		return nil, fmt.Errorf("runtime-oracle: producer ID must be a clean relative slash path: %q", producerID)
	}
	parts := strings.Split(producerID, "/")
	for _, part := range parts {
		if part == "" || part == "." || part == ".." {
			return nil, fmt.Errorf("runtime-oracle: producer ID must be a clean relative slash path: %q", producerID)
		}
	}
	if filepath.ToSlash(filepath.Clean(filepath.FromSlash(producerID))) != producerID {
		return nil, fmt.Errorf("runtime-oracle: producer ID must be a clean relative slash path: %q", producerID)
	}
	return parts, nil
}

func validateGeneratedAssets(assets []generatedAsset) error {
	seen := make(map[string]struct{}, len(assets))
	paths := make([]string, 0, len(assets))
	for _, asset := range assets {
		if strings.TrimSpace(asset.RelativePath) == "" {
			return fmt.Errorf("runtime-oracle: empty asset relative path")
		}
		if strings.Contains(asset.RelativePath, "\\") {
			return fmt.Errorf("runtime-oracle: backslash rejected in relative path: %s", asset.RelativePath)
		}
		if filepath.IsAbs(asset.RelativePath) || strings.HasPrefix(asset.RelativePath, "/") {
			return fmt.Errorf("runtime-oracle: absolute path rejected: %s", asset.RelativePath)
		}
		parts := strings.Split(asset.RelativePath, "/")
		for _, part := range parts {
			if part == "." || part == ".." {
				return fmt.Errorf("runtime-oracle: relative path contains ./..: %s", asset.RelativePath)
			}
			if part == "" {
				return fmt.Errorf("runtime-oracle: asset relative path must be clean: %s", asset.RelativePath)
			}
		}
		cleaned := filepath.Clean(filepath.FromSlash(asset.RelativePath))
		if cleaned == "." || cleaned == ".." || strings.HasPrefix(cleaned, ".."+string(filepath.Separator)) {
			return fmt.Errorf("runtime-oracle: path escapes producer directory: %s", asset.RelativePath)
		}
		normalized := filepath.ToSlash(cleaned)
		if normalized != asset.RelativePath {
			return fmt.Errorf("runtime-oracle: asset relative path must be clean: %s", asset.RelativePath)
		}
		if _, err := filepath.Localize(asset.RelativePath); err != nil {
			return fmt.Errorf("runtime-oracle: asset relative path is not valid on this platform: %q: %w", asset.RelativePath, err)
		}
		if _, duplicate := seen[normalized]; duplicate {
			return fmt.Errorf("runtime-oracle: duplicate asset relative path: %s", asset.RelativePath)
		}
		seen[normalized] = struct{}{}
		paths = append(paths, normalized)
	}
	for _, relative := range paths {
		parts := strings.Split(relative, "/")
		for index := 1; index < len(parts); index++ {
			parent := strings.Join(parts[:index], "/")
			if _, collision := seen[parent]; collision {
				return fmt.Errorf("runtime-oracle: asset path conflicts with another asset: %s", relative)
			}
		}
	}
	return nil
}

// createExportRoot creates only the missing export-root suffix and refuses any
// component that stops being a real directory during the walk.
func createExportRoot(absExport, existing string, missing []string) error {
	info, err := os.Lstat(existing)
	if err != nil {
		return fmt.Errorf("runtime-oracle: stat export ancestor %s: %w", existing, err)
	}
	if !info.IsDir() {
		return fmt.Errorf("runtime-oracle: export-prefix non-directory rejected: %s", existing)
	}

	current := existing
	for _, component := range missing {
		current = filepath.Join(current, component)
		info, err := os.Lstat(current)
		switch {
		case err == nil && info.Mode()&os.ModeSymlink != 0:
			return fmt.Errorf("runtime-oracle: export-prefix symlink rejected: %s", current)
		case err == nil && !info.IsDir():
			return fmt.Errorf("runtime-oracle: export-prefix non-directory rejected: %s", current)
		case err == nil:
			continue
		case !os.IsNotExist(err):
			return fmt.Errorf("runtime-oracle: stat export path %s: %w", current, err)
		}
		if err := os.Mkdir(current, 0o755); err != nil {
			return fmt.Errorf("runtime-oracle: create export directory %s: %w", current, err)
		}
	}
	if current != absExport {
		return fmt.Errorf("runtime-oracle: export root walk ended at %s, want %s", current, absExport)
	}
	return nil
}

// createProducerChild permits shared real prefix directories but creates the
// final producer directory exclusively as the publication ownership boundary.
func createProducerChild(exportRoot string, components []string) (string, error) {
	current := exportRoot
	for index, component := range components {
		current = filepath.Join(current, component)
		final := index == len(components)-1
		info, err := os.Lstat(current)
		switch {
		case err == nil && info.Mode()&os.ModeSymlink != 0:
			return "", fmt.Errorf("runtime-oracle: producer-prefix symlink rejected: %s", current)
		case err == nil && final:
			return "", fmt.Errorf("runtime-oracle: producer child already exists: %s", current)
		case err == nil && !info.IsDir():
			return "", fmt.Errorf("runtime-oracle: producer-prefix non-directory rejected: %s", current)
		case err == nil:
			continue
		case !os.IsNotExist(err):
			return "", fmt.Errorf("runtime-oracle: stat producer component %s: %w", current, err)
		}
		if err := os.Mkdir(current, 0o755); err != nil {
			return "", fmt.Errorf("runtime-oracle: create producer directory %s: %w", current, err)
		}
	}
	return current, nil
}

// createAssetParents prepares every parent before the first asset file is
// opened, using the same symlink and non-directory checks as producer paths.
func createAssetParents(producerChild string, assets []generatedAsset) error {
	for _, asset := range assets {
		parts := strings.Split(asset.RelativePath, "/")
		current := producerChild
		for _, component := range parts[:len(parts)-1] {
			if component == "" {
				continue
			}
			current = filepath.Join(current, filepath.FromSlash(component))
			info, err := os.Lstat(current)
			switch {
			case err == nil && info.Mode()&os.ModeSymlink != 0:
				return fmt.Errorf("runtime-oracle: asset-prefix symlink rejected: %s", current)
			case err == nil && !info.IsDir():
				return fmt.Errorf("runtime-oracle: asset-prefix non-directory rejected: %s", current)
			case err == nil:
				continue
			case !os.IsNotExist(err):
				return fmt.Errorf("runtime-oracle: stat asset directory %s: %w", current, err)
			}
			if err := os.Mkdir(current, 0o755); err != nil {
				return fmt.Errorf("runtime-oracle: create asset directory %s: %w", current, err)
			}
		}
	}
	return nil
}

// recheckExportContainment closes the component-walk phase by resolving the
// created producer path again immediately before exclusive file creation.
func recheckExportContainment(repoRoot, exportRoot, producerChild string) error {
	resolvedExport, err := filepath.EvalSymlinks(exportRoot)
	if err != nil {
		return fmt.Errorf("runtime-oracle: resolve created export root %s: %w", exportRoot, err)
	}
	resolvedProducer, err := filepath.EvalSymlinks(producerChild)
	if err != nil {
		return fmt.Errorf("runtime-oracle: resolve created producer child %s: %w", producerChild, err)
	}
	rel, err := filepath.Rel(resolvedExport, resolvedProducer)
	if err != nil || rel == ".." || strings.HasPrefix(rel, ".."+string(filepath.Separator)) {
		return fmt.Errorf("runtime-oracle: producer child escaped export root: %s", producerChild)
	}
	live, err := isLivePath(repoRoot, resolvedProducer)
	if err != nil {
		return err
	}
	if live {
		return fmt.Errorf("runtime-oracle: live-path write rejected: producer child %s is inside repository", producerChild)
	}
	return nil
}

func exportGeneratedAssets(
	repoRoot string,
	exportRoot string,
	producerID string,
	assets []generatedAsset,
) (string, error) {
	producerComponents, err := validateProducerID(producerID)
	if err != nil {
		return "", err
	}
	if !validProducerIDs[producerID] {
		return "", fmt.Errorf("runtime-oracle: unrecognized producer ID: %q", producerID)
	}
	if strings.TrimSpace(exportRoot) == "" {
		return "", fmt.Errorf("runtime-oracle: export root cannot be empty")
	}
	if err := validateGeneratedAssets(assets); err != nil {
		return "", err
	}

	absRoot, err := filepath.Abs(repoRoot)
	if err != nil {
		return "", fmt.Errorf("runtime-oracle: resolve repository root %s: %w", repoRoot, err)
	}
	if resolved, err := filepath.EvalSymlinks(absRoot); err == nil {
		absRoot = resolved
	}

	absExport, err := filepath.Abs(exportRoot)
	if err != nil {
		return "", fmt.Errorf("runtime-oracle: resolve export root %s: %w", exportRoot, err)
	}

	// Walk upward from absExport until an existing ancestor is found.
	var missing []string
	existing := absExport
	for {
		_, statErr := os.Lstat(existing)
		if statErr == nil {
			break
		}
		if !os.IsNotExist(statErr) {
			return "", fmt.Errorf("runtime-oracle: stat export root %s: %w", existing, statErr)
		}
		missing = append(missing, filepath.Base(existing))
		parent := filepath.Dir(existing)
		if parent == existing {
			return "", fmt.Errorf("runtime-oracle: export root %s has no existing ancestor", exportRoot)
		}
		existing = parent
	}
	for i, j := 0, len(missing)-1; i < j; i, j = i+1, j-1 {
		missing[i], missing[j] = missing[j], missing[i]
	}

	resolvedExisting, err := filepath.EvalSymlinks(existing)
	if err != nil {
		return "", fmt.Errorf("runtime-oracle: resolve export ancestor %s: %w", existing, err)
	}
	resolvedExport := filepath.Join(append([]string{resolvedExisting}, missing...)...)

	// Check repository containment on both resolved export path and existing ancestor.
	live, err := isLivePath(absRoot, resolvedExport)
	if err != nil {
		return "", err
	}
	if live {
		return "", fmt.Errorf("runtime-oracle: live-path write rejected: export root %s is inside repository", exportRoot)
	}
	liveExisting, err := isLivePath(absRoot, existing)
	if err != nil {
		return "", err
	}
	if liveExisting {
		return "", fmt.Errorf("runtime-oracle: live-path write rejected: export ancestor %s is inside repository", existing)
	}

	// Reject symlink components below the nearest existing ancestor.
	for component := existing; ; component = filepath.Dir(component) {
		if component == "/" || component == "." || component == filepath.Dir(component) {
			break
		}
		if component == "/var" || component == "/tmp" || component == "/etc" {
			break
		}
		info, statErr := os.Lstat(component)
		if statErr != nil {
			return "", fmt.Errorf("runtime-oracle: stat export path %s: %w", component, statErr)
		}
		if info.Mode()&os.ModeSymlink != 0 {
			return "", fmt.Errorf("runtime-oracle: symlink component rejected: %s", component)
		}
	}

	if err := createExportRoot(absExport, existing, missing); err != nil {
		return "", err
	}
	producerChild, err := createProducerChild(absExport, producerComponents)
	if err != nil {
		return "", err
	}
	if err := createAssetParents(producerChild, assets); err != nil {
		return "", err
	}
	if err := recheckExportContainment(absRoot, absExport, producerChild); err != nil {
		return "", err
	}

	for _, asset := range assets {
		target := filepath.Join(producerChild, filepath.FromSlash(asset.RelativePath))
		f, err := os.OpenFile(target, os.O_WRONLY|os.O_CREATE|os.O_EXCL, 0o644)
		if err != nil {
			return "", fmt.Errorf("runtime-oracle: create exclusive asset %s: %w", asset.RelativePath, err)
		}
		if _, err := f.Write(asset.Data); err != nil {
			f.Close()
			return "", fmt.Errorf("runtime-oracle: write asset %s: %w", asset.RelativePath, err)
		}
		if err := f.Close(); err != nil {
			return "", fmt.Errorf("runtime-oracle: close asset %s: %w", asset.RelativePath, err)
		}
	}

	return producerChild, nil
}

func exportGeneratedAssetsFromEnvironment(
	t *testing.T,
	repoRoot string,
	producerID string,
	assets []generatedAsset,
) string {
	t.Helper()
	exportRoot := strings.TrimSpace(os.Getenv(runtimeOracleExportDirEnv))
	if exportRoot == "" {
		return ""
	}
	dir, err := exportGeneratedAssets(repoRoot, exportRoot, producerID, assets)
	if err != nil {
		t.Fatalf("export generated assets for %s: %v", producerID, err)
	}
	return dir
}

// GoOperation executes one corpus case through a real Go producer.
//
// The producer receives only the case specification and the case input bytes.
// The recorded expected outcome deliberately stays outside this signature: a
// producer that could read the expectation could be written to agree with the
// recorded evidence instead of with the production codec, which would turn the
// corpus into a tautology. The returned bytes are the producer's own encoded
// output and are digested into the observation when the family publishes one.
type GoOperation func(CaseSpec, []byte) (Outcome, []byte, error)

// goOperations is the test-only registry keyed by the manifest operation name.
// A manifest naming an operation with no registered producer fails the run
// instead of silently dropping that coverage.
var goOperations = map[string]GoOperation{
	"decode": runFrameDecode,
	"admit":  runDomainAdmit,
}

// goFamilyOperations binds every family this package can execute to the
// operation that executes it. A manifest naming an unsupported family is a hard
// error, so a newly added corpus case cannot pass by being ignored.
var goFamilyOperations = map[string]string{
	"protocol.frame":           "decode",
	"domain.values":            "admit",
	"domain.identity_values":   "admit",
	"domain.command_control":   "admit",
	"domain.command_inventory": "admit",
	"domain.event":             "admit",
}

// runDomainAdmit dispatches one admission case to the producer that owns its
// family. The domain families share one admission operation because each case
// asks a Go authority whether one value is admitted, so the operation name
// registered in `goFamilyOperations` cannot select the family; the family
// itself does, and a family with no producer is a hard error rather than a
// silently skipped case.
//
// The `domain.event` family is shared by two producers, so its case routes
// through `runDomainEvent`, which selects the producer by the rule the case
// names.
func runDomainAdmit(c CaseSpec, input []byte) (Outcome, []byte, error) {
	switch c.Family {
	case domainValuesFamily:
		return runDomainValues(c, input)
	case domainIdentityFamily:
		return runDomainIdentityValues(c, input)
	case domainControlFamily:
		return runDomainControl(c, input)
	case domainCommandInventoryFamily:
		return runDomainCommandInventory(c, input)
	case domainEventPlayerFamily:
		return runDomainEvent(c, input)
	default:
		return Outcome{}, nil, fmt.Errorf("runtime-oracle: case %s names admission family %q, which has no Go producer", c.ID, c.Family)
	}
}

// RunCases executes the manifest selection through the registered Go producers
// and returns one executed observation per declared checkpoint.
//
// The runner owns the evidence boundary: it resolves each case input under the
// corpus byte budget, proves the input digest matches the manifest, invokes the
// registered producer once per declared checkpoint, and derives every
// executed observation from the values the producer returned. Expected outcomes
// are never read or handed to a producer, so an independent execution cannot
// be shaped by the recorded expectation.
//
// This entry keeps the historical single-operation-per-family contract: one
// family is executed as exactly one operation. A family that publishes more
// than one operation, as the framing family now does, is executed through
// `RunProtocolCases`, which resolves each case's route instead of binding the
// whole family to one operation name.
func RunCases(root string, manifest Inventory, operations map[string]GoOperation, familyOperations map[string]string) ([]ExecutedObservation, error) {
	if len(operations) == 0 || len(familyOperations) == 0 {
		return nil, fmt.Errorf("runtime-oracle: no registered Go producers")
	}
	return runCasesByRoute(root, manifest, func(family, version, operation string) (GoOperation, error) {
		bound, supported := familyOperations[family]
		if !supported {
			return nil, fmt.Errorf("family %s has no registered Go producer", family)
		}
		producer, registered := operations[bound]
		if !registered {
			return nil, fmt.Errorf("operation %q has no registered Go producer", bound)
		}
		if operation != bound {
			return nil, fmt.Errorf("case declares operation %q but family %s is executed as %q", operation, family, bound)
		}
		return producer, nil
	})
}

// runCasesByRoute is the shared execution loop every runner uses. The lookup
// resolves one case's family, version and operation to its producer, so the
// route policy lives in the runner's own entry point while the evidence
// boundary, the checkpoint discipline and the observation builder stay in one
// place.
func runCasesByRoute(root string, manifest Inventory, lookup routeLookup) ([]ExecutedObservation, error) {
	if len(manifest.Cases) == 0 {
		return nil, fmt.Errorf("runtime-oracle: manifest selection is empty")
	}

	var produced []ExecutedObservation
	for _, c := range manifest.Cases {
		producer, err := lookup(c.Family, c.Version, c.Operation)
		if err != nil {
			return nil, fmt.Errorf("runtime-oracle: case %s: %w", c.ID, err)
		}
		if len(c.Checkpoints) == 0 {
			return nil, fmt.Errorf("runtime-oracle: case %s declares no checkpoints", c.ID)
		}
		seenCheckpoints := make(map[string]bool, len(c.Checkpoints))
		for _, checkpoint := range c.Checkpoints {
			if seenCheckpoints[checkpoint] {
				return nil, fmt.Errorf("runtime-oracle: case %s declares duplicate checkpoint %q", c.ID, checkpoint)
			}
			seenCheckpoints[checkpoint] = true
		}
		input, err := readCaseInput(root, c)
		if err != nil {
			return nil, fmt.Errorf("runtime-oracle: case %s input: %w", c.ID, err)
		}
		for _, checkpoint := range c.Checkpoints {
			tick, err := strconv.ParseUint(checkpoint, 10, 64)
			if err != nil {
				return nil, fmt.Errorf("runtime-oracle: case %s checkpoint %q: %w", c.ID, checkpoint, err)
			}
			outcome, encoded, err := producer(c, input)
			if err != nil {
				return nil, fmt.Errorf("runtime-oracle: case %s tick %d: %w", c.ID, tick, err)
			}
			if int64(len(encoded)) > MaxBinaryBytes {
				return nil, fmt.Errorf("runtime-oracle: case %s encoded output %d bytes exceeds budget %d", c.ID, len(encoded), MaxBinaryBytes)
			}
			// The encoded digest is derived from what the producer returned, so
			// the observation stays a record of execution rather than of intent.
			if len(encoded) > 0 {
				sum := sha256.Sum256(encoded)
				outcome.EncodedPayloadDigest = fmt.Sprintf("sha256:%x", sum)
			}
			produced = append(produced, ExecutedObservation{Tick: tick, CaseID: c.ID, Outcome: outcome})
		}
	}
	return produced, nil
}

// readCaseInput loads one case input under the corpus byte budget and proves
// its digest matches the manifest, so a producer never executes bytes the
// manifest does not name.
func readCaseInput(root string, c CaseSpec) ([]byte, error) {
	maxBytes, err := caseInputMaxBytes(c.InputFormat)
	if err != nil {
		return nil, err
	}
	if err := validateAsset(root, c.Input, c.InputFormat == "json", maxBytes); err != nil {
		return nil, err
	}
	full := filepath.Join(root, filepath.FromSlash(c.Input.Path))
	data, err := os.ReadFile(full)
	if err != nil {
		return nil, err
	}
	if int64(len(data)) > maxBytes {
		return nil, fmt.Errorf("input %d bytes exceeds budget %d", len(data), maxBytes)
	}
	return data, nil
}

func TestReadCaseInputUsesFormatBudget(t *testing.T) {
	tests := []struct {
		name        string
		inputFormat string
		input       []byte
		wantErr     string
	}{
		{name: "json_exact_limit", inputFormat: "json", input: exactJSONObject(t, MaxCaseJSONBytes)},
		{name: "json_over_limit", inputFormat: "json", input: exactJSONObject(t, MaxCaseJSONBytes+1), wantErr: "exceeds budget"},
		{name: "binary_exact_limit", inputFormat: "binary", input: make([]byte, MaxBinaryBytes)},
	}
	for _, tc := range tests {
		t.Run(tc.name, func(t *testing.T) {
			fixture := newInventoryAssetFixture(t, tc.inputFormat, tc.input, false)
			got, err := readCaseInput(fixture.root, fixture.inventory.Cases[0])
			if tc.wantErr == "" {
				if err != nil {
					t.Fatalf("readCaseInput rejected exact input budget: %v", err)
				}
				if !bytes.Equal(got, tc.input) {
					t.Fatalf("readCaseInput returned %d bytes, want %d", len(got), len(tc.input))
				}
				return
			}
			if err == nil || !strings.Contains(err.Error(), tc.wantErr) {
				t.Fatalf("readCaseInput expected %q, got %v", tc.wantErr, err)
			}
		})
	}
}

// readExpectedOutcome decodes the normalized outcome one case records, which is
// the expectation an independent execution has to reproduce.
func readExpectedOutcome(t *testing.T, root string, c CaseSpec) Outcome {
	t.Helper()
	outcome, err := decodeExpectedOutcome(root, c)
	if err != nil {
		t.Fatalf("read expected outcome for %s: %v", c.ID, err)
	}
	return outcome
}

// decodeExpectedOutcome reads and normalizes the expectation one case records
// without a test handle, so a fixture that has no *testing.T to fail through
// derives its observation from the corpus instead of restating an outcome.
func decodeExpectedOutcome(root string, c CaseSpec) (Outcome, error) {
	full := filepath.Join(root, filepath.FromSlash(c.Expected.Path))
	data, err := os.ReadFile(full)
	if err != nil {
		return Outcome{}, fmt.Errorf("read %s: %w", full, err)
	}
	var outcome Outcome
	dec := json.NewDecoder(bytes.NewReader(data))
	dec.UseNumber()
	if err := dec.Decode(&outcome); err != nil {
		return Outcome{}, fmt.Errorf("decode %s: %w", full, err)
	}
	return outcome, nil
}

// traceFromObservations assembles executed observations into a trace identity
// so the evidence is checked by the same completeness rules a published report
// must satisfy.
func traceFromObservations(manifest Inventory, observations []ExecutedObservation) (Trace, error) {
	return BuildTrace(TraceRequest{}, manifest, observations)
}

// exportExecutedEvidence publishes the executed corpus evidence into the
// harness-owned directory named by RUNTIME_ORACLE_EXPORT_DIR and returns that
// directory. An unset variable exports nothing, so evidence never lands in the
// repository by default; a named directory must be fresh so an export can never
// overwrite evidence an earlier run published. The report itself is published
// through ExportTrace, which gives the export the same containment, symlink and
// no-replace gates the production reports use, and the per-case fixtures are
// written into the directory that publication just created and validated.
func exportExecutedEvidence(t *testing.T, root string, manifest Inventory, observations []ExecutedObservation) (string, error) {
	t.Helper()
	value := strings.TrimSpace(os.Getenv(runtimeOracleExportDirEnv))
	if value == "" {
		return "", nil
	}

	trace, err := traceFromObservations(manifest, observations)
	if err != nil {
		return "", err
	}
	reportTarget := filepath.Join(t.TempDir(), "runtime-corpus-frame.json")
	if err := ExportTrace(root, reportTarget, trace, manifest); err != nil {
		return "", fmt.Errorf("runtime-oracle: export executed evidence: %w", err)
	}
	reportData, err := os.ReadFile(reportTarget)
	if err != nil {
		return "", fmt.Errorf("runtime-oracle: read exported report: %w", err)
	}

	var assets []generatedAsset
	assets = append(assets, generatedAsset{
		RelativePath: "runtime-corpus-frame.json",
		Data:         reportData,
	})

	caseByID := make(map[string]CaseSpec, len(manifest.Cases))
	for _, c := range manifest.Cases {
		caseByID[c.ID] = c
	}
	for _, obs := range observations {
		c, ok := caseByID[obs.CaseID]
		if !ok {
			return "", fmt.Errorf("runtime-oracle: observation names unknown case %s", obs.CaseID)
		}
		input, err := readCaseInput(root, c)
		if err != nil {
			return "", fmt.Errorf("runtime-oracle: export case %s input: %w", c.ID, err)
		}
		expected, err := os.ReadFile(filepath.Join(root, filepath.FromSlash(c.Expected.Path)))
		if err != nil {
			return "", fmt.Errorf("runtime-oracle: export case %s expectation: %w", c.ID, err)
		}
		outcome, err := marshalCanonicalJSON(obs.Outcome)
		if err != nil {
			return "", fmt.Errorf("runtime-oracle: export case %s outcome: %w", c.ID, err)
		}
		assets = append(assets,
			generatedAsset{RelativePath: filepath.ToSlash(filepath.Join(c.ID, "input.bin")), Data: input},
			generatedAsset{RelativePath: filepath.ToSlash(filepath.Join(c.ID, "expected.json")), Data: expected},
			generatedAsset{RelativePath: filepath.ToSlash(filepath.Join(c.ID, "outcome.json")), Data: outcome},
		)
	}

	return exportGeneratedAssets(root, value, "runtime-oracle/protocol-frame", assets)
}

// marshalCanonicalJSON renders one value with recursively sorted keys so two
// exports of the same evidence are byte-identical.
func marshalCanonicalJSON(value any) ([]byte, error) {
	raw, err := json.Marshal(value)
	if err != nil {
		return nil, err
	}
	var generic any
	dec := json.NewDecoder(bytes.NewReader(raw))
	dec.UseNumber()
	if err := dec.Decode(&generic); err != nil {
		return nil, err
	}
	var buf bytes.Buffer
	if err := writeCanonicalJSON(&buf, generic); err != nil {
		return nil, err
	}
	buf.WriteByte('\n')
	return buf.Bytes(), nil
}

// computeTrackedCorpusDigest computes a deterministic SHA256 digest over all
// files under testdata/runtime-migration to prove the tree remains unchanged.
func computeTrackedCorpusDigest(t *testing.T, repoRoot string) string {
	t.Helper()
	casesDir := filepath.Join(repoRoot, "testdata", "runtime-migration")
	hasher := sha256.New()
	err := filepath.WalkDir(casesDir, func(path string, d fs.DirEntry, err error) error {
		if err != nil {
			return err
		}
		if d.IsDir() {
			return nil
		}
		rel, err := filepath.Rel(casesDir, path)
		if err != nil {
			return err
		}
		data, err := os.ReadFile(path)
		if err != nil {
			return err
		}
		fmt.Fprintf(hasher, "%s\x00%x\n", filepath.ToSlash(rel), sha256.Sum256(data))
		return nil
	})
	if err != nil {
		t.Fatalf("compute tracked corpus digest: %v", err)
	}
	return fmt.Sprintf("sha256:%x", hasher.Sum(nil))
}

// assertTrackedCorpusUnchanged asserts the tracked corpus digest matches the baseline.
func assertTrackedCorpusUnchanged(t *testing.T, repoRoot, before string) {
	t.Helper()
	after := computeTrackedCorpusDigest(t, repoRoot)
	if before != after {
		t.Fatalf("tracked corpus was modified: before %s, after %s", before, after)
	}
}

// TestExportGeneratedAssetsRejectsRepositoryContainedRoots pins that an export
// root inside the repository is refused before anything is created.
func TestExportGeneratedAssetsRejectsRepositoryContainedRoots(t *testing.T) {
	root := mustRepoRoot(t)
	before := computeTrackedCorpusDigest(t, root)
	defer assertTrackedCorpusUnchanged(t, root, before)

	exportRoot := filepath.Join(root, "export-contained-probe")
	assets := []generatedAsset{{RelativePath: "test.json", Data: []byte("{}\n")}}
	_, err := exportGeneratedAssets(root, exportRoot, "runtime-oracle/domain-values", assets)
	if err == nil || !strings.Contains(err.Error(), "live-path") {
		t.Fatalf("expected live-path rejection, got: %v", err)
	}
	if _, statErr := os.Lstat(exportRoot); !os.IsNotExist(statErr) {
		t.Fatalf("export directory was created inside repository: %v", statErr)
	}
}

// TestExportGeneratedAssetsRejectsSymlinkedAncestor pins that an export root
// reached through a symlink ancestor is refused.
func TestExportGeneratedAssetsRejectsSymlinkedAncestor(t *testing.T) {
	root := mustRepoRoot(t)
	before := computeTrackedCorpusDigest(t, root)
	defer assertTrackedCorpusUnchanged(t, root, before)

	parent := t.TempDir()
	realDir := filepath.Join(parent, "real")
	if err := os.Mkdir(realDir, 0o755); err != nil {
		t.Fatal(err)
	}
	symlinkDir := filepath.Join(parent, "symlink-ancestor")
	if err := os.Symlink(realDir, symlinkDir); err != nil {
		t.Fatal(err)
	}
	exportRoot := filepath.Join(symlinkDir, "export-target")
	assets := []generatedAsset{{RelativePath: "test.json", Data: []byte("{}\n")}}
	_, err := exportGeneratedAssets(root, exportRoot, "runtime-oracle/domain-values", assets)
	if err == nil || !strings.Contains(err.Error(), "symlink") {
		t.Fatalf("expected symlink rejection, got: %v", err)
	}
	entries, readErr := os.ReadDir(realDir)
	if readErr != nil {
		t.Fatal(readErr)
	}
	if len(entries) != 0 {
		t.Fatalf("export wrote through symlink ancestor: %v", entries)
	}
}

// TestExportGeneratedAssetsRejectsProducerPrefixSymlink pins that a fixed
// producer prefix cannot redirect an export into either the repository or an
// unrelated external directory.
func TestExportGeneratedAssetsRejectsProducerPrefixSymlink(t *testing.T) {
	tests := []struct {
		name       string
		targetRoot func(t *testing.T, repoRoot string) string
	}{
		{
			name: "into_repository",
			targetRoot: func(_ *testing.T, repoRoot string) string {
				return repoRoot
			},
		},
		{
			name: "to_external_directory",
			targetRoot: func(t *testing.T, _ string) string {
				t.Helper()
				return t.TempDir()
			},
		},
	}

	for _, tc := range tests {
		t.Run(tc.name, func(t *testing.T) {
			repoRoot := t.TempDir()
			exportRoot := filepath.Join(t.TempDir(), "export")
			if err := os.Mkdir(exportRoot, 0o755); err != nil {
				t.Fatal(err)
			}
			targetRoot := tc.targetRoot(t, repoRoot)
			if err := os.Symlink(targetRoot, filepath.Join(exportRoot, "runtime-oracle")); err != nil {
				t.Fatal(err)
			}

			sentinel := filepath.Join(targetRoot, "domain-values", "sentinel.json")
			assets := []generatedAsset{{RelativePath: "sentinel.json", Data: []byte("{}\n")}}
			_, err := exportGeneratedAssets(repoRoot, exportRoot, "runtime-oracle/domain-values", assets)
			if err == nil || !strings.Contains(err.Error(), "producer-prefix symlink") {
				t.Fatalf("expected producer-prefix symlink rejection, got: %v", err)
			}
			if _, statErr := os.Lstat(sentinel); !os.IsNotExist(statErr) {
				t.Fatalf("export wrote through producer-prefix symlink: %v", statErr)
			}
		})
	}
}

// TestExportGeneratedAssetsRejectsNonDirectoryProducerPrefix pins that a
// regular file cannot stand in for a shared producer-prefix directory.
func TestExportGeneratedAssetsRejectsNonDirectoryProducerPrefix(t *testing.T) {
	repoRoot := t.TempDir()
	exportRoot := filepath.Join(t.TempDir(), "export")
	if err := os.Mkdir(exportRoot, 0o755); err != nil {
		t.Fatal(err)
	}
	prefix := filepath.Join(exportRoot, "runtime-oracle")
	if err := os.WriteFile(prefix, []byte("sentinel\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	assets := []generatedAsset{{RelativePath: "sentinel.json", Data: []byte("{}\n")}}
	_, err := exportGeneratedAssets(repoRoot, exportRoot, "runtime-oracle/domain-values", assets)
	if err == nil || !strings.Contains(err.Error(), "producer-prefix non-directory") {
		t.Fatalf("expected producer-prefix non-directory rejection, got: %v", err)
	}
	data, readErr := os.ReadFile(prefix)
	if readErr != nil || string(data) != "sentinel\n" {
		t.Fatalf("producer-prefix file changed: %q, err: %v", data, readErr)
	}
}

// TestExportGeneratedAssetsRejectsEscapingRelativePath pins that asset paths
// escaping the producer child, carrying backslashes, or containing ./.. are rejected.
func TestExportGeneratedAssetsRejectsEscapingRelativePath(t *testing.T) {
	root := mustRepoRoot(t)
	before := computeTrackedCorpusDigest(t, root)
	defer assertTrackedCorpusUnchanged(t, root, before)

	badPaths := []struct {
		name    string
		path    string
		wantErr string
	}{
		{name: "parent", path: "../escaped.json", wantErr: "relative path contains ./.."},
		{name: "absolute", path: "/absolute.json", wantErr: "absolute path rejected"},
		{name: "nested_parent_escape", path: "sub/../../escaped.json", wantErr: "relative path contains ./.."},
		{name: "backslash", path: "sub\\backslash.json", wantErr: "backslash rejected"},
		{name: "current", path: "./current.json", wantErr: "relative path contains ./.."},
		{name: "nested_current", path: "sub/./current.json", wantErr: "relative path contains ./.."},
		{name: "nested_parent", path: "sub/../escaped.json", wantErr: "relative path contains ./.."},
		{name: "empty_component", path: "sub//asset.json", wantErr: "asset relative path must be clean"},
		{name: "trailing_slash", path: "sub/", wantErr: "asset relative path must be clean"},
		{name: "nul", path: "sentinel\x00.json", wantErr: "asset relative path is not valid on this platform"},
	}
	for _, tc := range badPaths {
		t.Run(tc.name, func(t *testing.T) {
			exportRoot := filepath.Join(t.TempDir(), "escaping-test")
			producerChild := filepath.Join(exportRoot, "runtime-oracle", "domain-values")
			assets := []generatedAsset{{RelativePath: tc.path, Data: []byte("{}\n")}}
			_, err := exportGeneratedAssets(root, exportRoot, "runtime-oracle/domain-values", assets)
			if err == nil || !strings.Contains(err.Error(), tc.wantErr) {
				t.Fatalf("expected %q rejection for relative path %q, got %v", tc.wantErr, tc.path, err)
			}
			if _, statErr := os.Lstat(producerChild); !os.IsNotExist(statErr) {
				t.Fatalf("producer directory was created before asset validation: %v", statErr)
			}
			if _, statErr := os.Lstat(exportRoot); !os.IsNotExist(statErr) {
				t.Fatalf("export root was created before asset validation: %v", statErr)
			}
		})
	}
}

// TestExportGeneratedAssetsRejectsInvalidAssetSetBeforeCreation pins that
// duplicate and file-as-parent asset sets fail before any export mutation.
func TestExportGeneratedAssetsRejectsInvalidAssetSetBeforeCreation(t *testing.T) {
	root := mustRepoRoot(t)
	tests := []struct {
		name    string
		assets  []generatedAsset
		wantErr string
	}{
		{
			name: "duplicate",
			assets: []generatedAsset{
				{RelativePath: "same.json", Data: []byte("first\n")},
				{RelativePath: "same.json", Data: []byte("second\n")},
			},
			wantErr: "duplicate asset relative path",
		},
		{
			name: "file_is_parent",
			assets: []generatedAsset{
				{RelativePath: "node", Data: []byte("file\n")},
				{RelativePath: "node/child.json", Data: []byte("{}\n")},
			},
			wantErr: "asset path conflicts",
		},
	}

	for _, tc := range tests {
		t.Run(tc.name, func(t *testing.T) {
			exportRoot := filepath.Join(t.TempDir(), "invalid-assets")
			producerChild := filepath.Join(exportRoot, "runtime-oracle", "domain-values")
			_, err := exportGeneratedAssets(root, exportRoot, "runtime-oracle/domain-values", tc.assets)
			if err == nil || !strings.Contains(err.Error(), tc.wantErr) {
				t.Fatalf("expected %q rejection, got %v", tc.wantErr, err)
			}
			if _, statErr := os.Lstat(producerChild); !os.IsNotExist(statErr) {
				t.Fatalf("producer directory was created before asset-set validation: %v", statErr)
			}
		})
	}
}

// TestExportGeneratedAssetsRejectsPreexistingProducerChild pins that a producer
// directory that already exists is refused and cannot be overwritten.
func TestExportGeneratedAssetsRejectsPreexistingProducerChild(t *testing.T) {
	root := mustRepoRoot(t)
	before := computeTrackedCorpusDigest(t, root)
	defer assertTrackedCorpusUnchanged(t, root, before)

	exportRoot := filepath.Join(t.TempDir(), "preexisting-test")
	assets := []generatedAsset{{RelativePath: "initial.txt", Data: []byte("first\n")}}
	published, err := exportGeneratedAssets(root, exportRoot, "runtime-oracle/domain-values", assets)
	if err != nil {
		t.Fatalf("initial export failed: %v", err)
	}
	wantPublished := filepath.Join(exportRoot, "runtime-oracle", "domain-values")
	if published != wantPublished {
		t.Fatalf("published = %s, want %s", published, wantPublished)
	}

	secondAssets := []generatedAsset{{RelativePath: "initial.txt", Data: []byte("second\n")}}
	_, err = exportGeneratedAssets(root, exportRoot, "runtime-oracle/domain-values", secondAssets)
	if err == nil || !strings.Contains(err.Error(), "already exists") {
		t.Fatalf("expected already exists rejection, got: %v", err)
	}

	data, readErr := os.ReadFile(filepath.Join(published, "initial.txt"))
	if readErr != nil {
		t.Fatalf("read asset: %v", readErr)
	}
	if string(data) != "first\n" {
		t.Fatalf("existing asset was overwritten: %q", string(data))
	}
}

// TestExportGeneratedAssetsSuccessfulMultiProducerExport pins that multiple
// distinct producers can export into the same external export root.
func TestExportGeneratedAssetsSuccessfulMultiProducerExport(t *testing.T) {
	root := mustRepoRoot(t)
	before := computeTrackedCorpusDigest(t, root)
	defer assertTrackedCorpusUnchanged(t, root, before)

	exportRoot := filepath.Join(t.TempDir(), "multi-export-test")
	assets1 := []generatedAsset{
		{RelativePath: "values.json", Data: []byte("{\"val\":1}\n")},
	}
	pub1, err := exportGeneratedAssets(root, exportRoot, "runtime-oracle/domain-values", assets1)
	if err != nil {
		t.Fatalf("producer 1 export failed: %v", err)
	}

	assets2 := []generatedAsset{
		{RelativePath: "identity.json", Data: []byte("{\"id\":2}\n")},
	}
	pub2, err := exportGeneratedAssets(root, exportRoot, "runtime-oracle/domain-identity-values", assets2)
	if err != nil {
		t.Fatalf("producer 2 export failed: %v", err)
	}

	wantPub1 := filepath.Join(exportRoot, "runtime-oracle", "domain-values")
	wantPub2 := filepath.Join(exportRoot, "runtime-oracle", "domain-identity-values")
	if pub1 != wantPub1 {
		t.Fatalf("producer 1 path = %s, want %s", pub1, wantPub1)
	}
	if pub2 != wantPub2 {
		t.Fatalf("producer 2 path = %s, want %s", pub2, wantPub2)
	}

	d1, err := os.ReadFile(filepath.Join(pub1, "values.json"))
	if err != nil || string(d1) != "{\"val\":1}\n" {
		t.Fatalf("unexpected asset 1 content: %q, err: %v", string(d1), err)
	}
	d2, err := os.ReadFile(filepath.Join(pub2, "identity.json"))
	if err != nil || string(d2) != "{\"id\":2}\n" {
		t.Fatalf("unexpected asset 2 content: %q, err: %v", string(d2), err)
	}
}
