package main

import (
	"crypto/sha256"
	"encoding/json"
	"errors"
	"fmt"
	"os"
	"path/filepath"
	"strings"
	"testing"

	"github.com/channing771/mornlea/packages/server/storage/storagedef"
)

func TestStorageSelectionValidateStorageArguments(t *testing.T) {
	t.Run("player rejects missing id", func(t *testing.T) {
		err := validateStorageArguments("save.player", "decode", json.RawMessage(`{}`))
		if err == nil || !strings.Contains(err.Error(), "requested_player_id") {
			t.Fatalf("expected missing player id error, got %v", err)
		}
	})
	t.Run("player rejects bad id", func(t *testing.T) {
		err := validateStorageArguments("save.player", "decode", json.RawMessage(`{"requested_player_id":"UPPERCASE-NOT-HEX-32-CHARS!!!!"}`))
		if err == nil {
			t.Fatal("expected invalid player id")
		}
	})
	t.Run("chunk rejects numeric revision", func(t *testing.T) {
		raw := json.RawMessage(`{"dimension":0,"x":0,"z":0,"revision":1}`)
		err := validateStorageArguments("save.chunk", "decode", raw)
		if err == nil || !strings.Contains(err.Error(), "revision") {
			t.Fatalf("expected revision string error, got %v", err)
		}
	})
	t.Run("region order rejects wrong component", func(t *testing.T) {
		raw := json.RawMessage(`{"dimension":0,"x":0,"z":0,"file_size":"61440","component":"bank"}`)
		err := validateStorageArguments("save.region", "order", raw)
		if err == nil || !strings.Contains(err.Error(), "banks") {
			t.Fatalf("expected banks component error, got %v", err)
		}
	})
	t.Run("region rejects numeric file_size", func(t *testing.T) {
		raw := json.RawMessage(`{"dimension":0,"x":0,"z":0,"file_size":61440,"component":"bank"}`)
		err := validateStorageArguments("save.region", "decode", raw)
		if err == nil || !strings.Contains(err.Error(), "file_size") {
			t.Fatalf("expected file_size string error, got %v", err)
		}
	})
	t.Run("rejects unknown key", func(t *testing.T) {
		raw := json.RawMessage(`{"requested_player_id":"0123456789abcdef0123456789abcdef","extra":1}`)
		err := validateStorageArguments("save.player", "decode", raw)
		if err == nil || !strings.Contains(err.Error(), "unknown argument key") {
			t.Fatalf("expected unknown key error, got %v", err)
		}
	})
	t.Run("rejects capacity on decode", func(t *testing.T) {
		raw := json.RawMessage(`{"requested_player_id":"0123456789abcdef0123456789abcdef","capacity":64}`)
		err := validateStorageArguments("save.player", "decode", raw)
		if err == nil || !strings.Contains(err.Error(), "capacity") {
			t.Fatalf("expected capacity on decode error, got %v", err)
		}
	})
	t.Run("accepts valid player decode", func(t *testing.T) {
		raw := json.RawMessage(`{"requested_player_id":"0123456789abcdef0123456789abcdef"}`)
		if err := validateStorageArguments("save.player", "decode", raw); err != nil {
			t.Fatalf("valid player arguments: %v", err)
		}
	})
}

func TestStorageSelectionValidateCaseSpecRejectsMalformedSaveCase(t *testing.T) {
	root, err := RepositoryRoot()
	if err != nil {
		t.Fatalf("repository root: %v", err)
	}
	frozen, err := LoadInventory(filepath.Join(root, filepath.FromSlash(InventoryRelPath)))
	if err != nil {
		t.Fatalf("load frozen manifest: %v", err)
	}
	families := make(map[string]Family, len(frozen.Families))
	for _, family := range frozen.Families {
		families[family.ID] = family
	}
	bad := storageSampleCase(t, root, "save.player", "decode")
	bad.Arguments = json.RawMessage(`{"requested_player_id":"not-a-valid-player-id"}`)
	if err := validateCaseSpec(root, bad, families); err == nil {
		t.Fatal("expected malformed save arguments to fail validateCaseSpec")
	}
}

func TestStorageSelectionBaselineHasNoSaveCases(t *testing.T) {
	root, err := RepositoryRoot()
	if err != nil {
		t.Fatalf("repository root: %v", err)
	}
	frozen, err := LoadInventory(filepath.Join(root, filepath.FromSlash(InventoryRelPath)))
	if err != nil {
		t.Fatalf("load frozen manifest: %v", err)
	}
	for _, c := range frozen.Cases {
		if strings.HasPrefix(c.Family, "save.") {
			t.Fatalf("baseline unexpectedly carries save case %s", c.ID)
		}
	}
}

func TestStorageSelectionReadRejectsInvalidCandidate(t *testing.T) {
	root, err := RepositoryRoot()
	if err != nil {
		t.Fatalf("repository root: %v", err)
	}
	external := t.TempDir()

	t.Run("repo contained root", func(t *testing.T) {
		_, err := readStorageSelection(root)
		if err == nil || !strings.Contains(err.Error(), "inside repository") {
			t.Fatalf("expected repo containment error, got %v", err)
		}
	})

	t.Run("relative path", func(t *testing.T) {
		_, err := readStorageSelection("relative/candidate")
		if err == nil || !strings.Contains(err.Error(), "absolute") {
			t.Fatalf("expected absolute path error, got %v", err)
		}
	})

	t.Run("missing producer id", func(t *testing.T) {
		dir := filepath.Join(external, "missing-producer")
		writeStorageSelectionDir(t, dir, `{"cases":[],"sources":[],"routes":[]}`)
		_, err := readStorageSelection(dir)
		if err == nil || !strings.Contains(err.Error(), "producer_id") {
			t.Fatalf("expected missing producer_id error, got %v", err)
		}
	})

	t.Run("stale source hash", func(t *testing.T) {
		dir := filepath.Join(external, "stale-source")
		selection := storageSampleSelection(t, root, "save.player", "decode")
		selection.Sources[0].SHA256 = "sha256:0000000000000000000000000000000000000000000000000000000000000000"
		writeStorageSelectionCandidate(t, dir, selection)
		_, err := readStorageSelection(dir)
		if err == nil || !strings.Contains(err.Error(), "does not match disk") {
			t.Fatalf("expected stale source hash error, got %v", err)
		}
	})

	t.Run("duplicate case id", func(t *testing.T) {
		dir := filepath.Join(external, "duplicate-case")
		selection := storageSampleSelection(t, root, "save.player", "decode")
		selection.Cases = append(selection.Cases, selection.Cases[0])
		writeStorageSelectionCandidate(t, dir, selection)
		_, err := readStorageSelection(dir)
		if err == nil || !strings.Contains(err.Error(), "twice") {
			t.Fatalf("expected duplicate case error, got %v", err)
		}
	})

	t.Run("asset path escape", func(t *testing.T) {
		dir := filepath.Join(external, "escape-asset")
		selection := storageSampleSelection(t, root, "save.player", "decode")
		selection.Cases[0].Input.Path = "../outside.bin"
		writeStorageSelectionCandidate(t, dir, selection)
		_, err := readStorageSelection(dir)
		if err == nil {
			t.Fatal("expected asset path escape to fail")
		}
	})

	t.Run("unknown route", func(t *testing.T) {
		dir := filepath.Join(external, "unknown-route")
		selection := storageSampleSelection(t, root, "save.player", "decode")
		selection.Routes = []ConsumerRoute{{FamilyID: "save.player", Version: selection.Cases[0].Version, Operation: "migrate"}}
		writeStorageSelectionCandidate(t, dir, selection)
		_, err := readStorageSelection(dir)
		if err == nil || !strings.Contains(err.Error(), "route") {
			t.Fatalf("expected unknown route error, got %v", err)
		}
	})

	t.Run("bad checkpoints", func(t *testing.T) {
		dir := filepath.Join(external, "bad-checkpoints")
		selection := storageSampleSelection(t, root, "save.player", "decode")
		selection.Cases[0].Checkpoints = []string{"0", "1"}
		writeStorageSelectionCandidate(t, dir, selection)
		_, err := readStorageSelection(dir)
		if err == nil || !strings.Contains(err.Error(), "checkpoints") {
			t.Fatalf("expected checkpoint error, got %v", err)
		}
	})
}

func TestStorageSelectionReadAcceptsValidCandidate(t *testing.T) {
	root, err := RepositoryRoot()
	if err != nil {
		t.Fatalf("repository root: %v", err)
	}
	dir := filepath.Join(t.TempDir(), "valid")
	selection := storageSampleSelection(t, root, "save.player", "decode")
	writeStorageSelectionCandidate(t, dir, selection)
	got, err := readStorageSelection(dir)
	if err != nil {
		t.Fatalf("read valid selection: %v", err)
	}
	if got.ProducerID != selection.ProducerID || len(got.Cases) != 1 {
		t.Fatalf("unexpected selection: %+v", got)
	}
}

func TestStorageSelectionMergeRejects(t *testing.T) {
	root, err := RepositoryRoot()
	if err != nil {
		t.Fatalf("repository root: %v", err)
	}
	base, err := LoadInventory(filepath.Join(root, filepath.FromSlash(InventoryRelPath)))
	if err != nil {
		t.Fatalf("load frozen manifest: %v", err)
	}
	selection := storageSampleSelection(t, root, "save.player", "decode")

	t.Run("unregistered producer", func(t *testing.T) {
		bad := selection
		bad.ProducerID = "runtime-oracle/not-a-storage-producer"
		_, err := mergeStorageSelections(root, base, bad)
		if err == nil || !strings.Contains(err.Error(), "unrecognized producer") {
			t.Fatalf("expected unrecognized producer error, got %v", err)
		}
	})

	t.Run("duplicate conflicting case", func(t *testing.T) {
		registered := base
		registered.Cases = append(append([]CaseSpec(nil), base.Cases...), selection.Cases[0])
		conflict := selection
		conflict.Cases[0].Expected.SHA256 = "sha256:ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff"
		_, err := mergeStorageSelections(root, registered, conflict)
		if err == nil || !strings.Contains(err.Error(), "conflicts") {
			t.Fatalf("expected conflicting case error, got %v", err)
		}
	})

	t.Run("conflicting family source", func(t *testing.T) {
		bad := selection
		bad.Sources[0].SHA256 = "sha256:0000000000000000000000000000000000000000000000000000000000000000"
		_, err := mergeStorageSelections(root, base, bad)
		if err == nil || !strings.Contains(err.Error(), "does not match disk") {
			t.Fatalf("expected conflicting source hash error, got %v", err)
		}
	})

	t.Run("unknown route", func(t *testing.T) {
		bad := selection
		bad.Routes = []ConsumerRoute{{FamilyID: "save.player", Version: bad.Cases[0].Version, Operation: "kernel"}}
		_, err := mergeStorageSelections(root, base, bad)
		if err == nil || !strings.Contains(err.Error(), "route") {
			t.Fatalf("expected unknown route error, got %v", err)
		}
	})
}

func TestStorageSelectionMergePreservesUnrelatedFamilies(t *testing.T) {
	root, err := RepositoryRoot()
	if err != nil {
		t.Fatalf("repository root: %v", err)
	}
	base, err := LoadInventory(filepath.Join(root, filepath.FromSlash(InventoryRelPath)))
	if err != nil {
		t.Fatalf("load frozen manifest: %v", err)
	}
	selection := storageSampleSelection(t, root, "save.player", "decode")
	merged, err := mergeStorageSelections(root, base, selection)
	if err != nil {
		t.Fatalf("merge storage selection: %v", err)
	}
	for _, familyID := range []string{"protocol.frame", "domain.event", "save.chunk"} {
		if !reflectFamilyEqual(base, merged, familyID) {
			t.Fatalf("family %s changed during unrelated merge", familyID)
		}
	}
	if len(merged.Cases) != len(base.Cases)+1 {
		t.Fatalf("expected one added case, got %d total", len(merged.Cases))
	}
}

func TestStorageSelectionExportUnsetWritesNothing(t *testing.T) {
	root, err := RepositoryRoot()
	if err != nil {
		t.Fatalf("repository root: %v", err)
	}
	t.Setenv(runtimeOracleExportDirEnv, "")
	before, err := os.ReadDir(root)
	if err != nil {
		t.Fatalf("read repo root: %v", err)
	}
	_ = exportStorageSelectionCandidateFromEnvironment(t, root, storageSampleSelection(t, root, "save.player", "decode"))
	after, err := os.ReadDir(root)
	if err != nil {
		t.Fatalf("read repo root after: %v", err)
	}
	if len(before) != len(after) {
		t.Fatal("export with unset env must not mutate repository tree")
	}
}

func TestStorageSelectionStoragedefImportIsAvailable(t *testing.T) {
	if storagedef.ErrCorrupt == nil {
		t.Fatal("storagedef.ErrCorrupt must be available to test-only producers")
	}
	if !errors.Is(storagedef.ErrCorrupt, storagedef.ErrCorrupt) {
		t.Fatal("storagedef error identity must be stable")
	}
}

func storageSampleSelection(t *testing.T, root, family, operation string) StorageSelection {
	t.Helper()
	caseSpec := storageSampleCase(t, root, family, operation)
	producer := storageProducerID(family)
	sourcePath := storageProducerSourcePath(family)
	hash, err := hashFile(filepath.Join(root, filepath.FromSlash(sourcePath)))
	if err != nil {
		t.Fatalf("hash source %s: %v", sourcePath, err)
	}
	return StorageSelection{
		ProducerID: producer,
		Cases:      []CaseSpec{caseSpec},
		Sources:    []SourceSpec{{Path: sourcePath, SHA256: hash}},
		Routes: []ConsumerRoute{{
			FamilyID:  family,
			Version:   caseSpec.Version,
			Operation: operation,
		}},
	}
}

func storageSampleCase(t *testing.T, root, family, operation string) CaseSpec {
	t.Helper()
	families, _, err := Discover(root)
	if err != nil {
		t.Fatalf("discover: %v", err)
	}
	var saveFamily Family
	for _, familyRow := range families {
		if familyRow.ID == family {
			saveFamily = familyRow
			break
		}
	}
	if saveFamily.ID == "" {
		t.Fatalf("unknown save family %s", family)
	}
	version := saveFamily.CurrentVersion
	label := "sample"
	id := family + "/" + version + "/" + operation + "/" + label
	inputData := []byte{0x00}
	expectedData := []byte(`{"kind":"error","category":"corrupt"}`)
	inputRel := "testdata/runtime-migration/cases/storage/_candidate/" + strings.ReplaceAll(id, "/", "_") + ".input.bin"
	expectedRel := "testdata/runtime-migration/cases/storage/_candidate/" + strings.ReplaceAll(id, "/", "_") + ".expected.json"
	inputHash, err := hashBytes(inputData)
	if err != nil {
		t.Fatalf("hash input: %v", err)
	}
	expectedHash, err := hashBytes(expectedData)
	if err != nil {
		t.Fatalf("hash expected: %v", err)
	}
	args := storageSampleArguments(family, operation)
	return CaseSpec{
		ID:           id,
		Family:       family,
		Version:      version,
		Operation:    operation,
		Arguments:    args,
		Input:        AssetRef{Path: inputRel, SHA256: inputHash},
		InputFormat:  "binary",
		Expected:     AssetRef{Path: expectedRel, SHA256: expectedHash},
		Checkpoints:  []string{"0"},
		RustConsumer: storageConsumerName,
	}
}

func storageSampleArguments(family, operation string) json.RawMessage {
	switch family {
	case "save.player":
		return json.RawMessage(`{"requested_player_id":"0123456789abcdef0123456789abcdef"}`)
	case "save.chunk":
		return json.RawMessage(`{"dimension":0,"x":0,"z":0,"revision":"0"}`)
	case "save.region":
		if operation == "order" {
			return json.RawMessage(`{"dimension":0,"x":0,"z":0,"file_size":"61440","component":"banks"}`)
		}
		return json.RawMessage(`{"dimension":0,"x":0,"z":0,"file_size":"61440","component":"bank"}`)
	default:
		return json.RawMessage(`{}`)
	}
}

func storageProducerID(family string) string {
	switch family {
	case "save.world-metadata":
		return "storage/metadata"
	default:
		suffix := strings.TrimPrefix(family, "save.")
		return "runtime-oracle/storage-" + suffix
	}
}

func storageProducerSourcePath(family string) string {
	switch family {
	case "save.player":
		return "packages/server/storage/player/player_codec.go"
	case "save.chunk":
		return "packages/server/storage/chunk/chunk_codec.go"
	case "save.region":
		return "packages/server/storage/region/region_format.go"
	case "save.world-metadata":
		return "packages/server/storage/metadata.go"
	case "save.hostile":
		return "packages/server/storage/hostile/hostile_codec.go"
	case "save.passive":
		return "packages/server/storage/passive/passive_codec.go"
	case "save.companion":
		return "packages/server/storage/companion/companion_codec.go"
	default:
		return "packages/server/storage/player/player_codec.go"
	}
}

func writeStorageSelectionCandidate(t *testing.T, candidateDir string, selection StorageSelection) {
	t.Helper()
	for _, c := range selection.Cases {
		writeCandidateAsset(t, candidateDir, c.Input.Path, []byte{0x00})
		writeCandidateAsset(t, candidateDir, c.Expected.Path, []byte(`{"kind":"error","category":"corrupt"}`))
	}
	payload, err := json.Marshal(encodeStorageSelectionJSON(selection))
	if err != nil {
		t.Fatalf("marshal selection: %v", err)
	}
	if err := os.MkdirAll(candidateDir, 0o755); err != nil {
		t.Fatalf("mkdir candidate: %v", err)
	}
	if err := os.WriteFile(filepath.Join(candidateDir, storageSelectionManifest), payload, 0o644); err != nil {
		t.Fatalf("write selection manifest: %v", err)
	}
}

func writeStorageSelectionDir(t *testing.T, candidateDir string, manifest string) {
	t.Helper()
	if err := os.MkdirAll(candidateDir, 0o755); err != nil {
		t.Fatalf("mkdir candidate: %v", err)
	}
	if err := os.WriteFile(filepath.Join(candidateDir, storageSelectionManifest), []byte(manifest), 0o644); err != nil {
		t.Fatalf("write selection manifest: %v", err)
	}
}

func writeCandidateAsset(t *testing.T, candidateDir, rel string, data []byte) {
	t.Helper()
	full := filepath.Join(candidateDir, filepath.FromSlash(rel))
	if err := os.MkdirAll(filepath.Dir(full), 0o755); err != nil {
		t.Fatalf("mkdir asset parent: %v", err)
	}
	if err := os.WriteFile(full, data, 0o644); err != nil {
		t.Fatalf("write asset: %v", err)
	}
}

func hashBytes(data []byte) (string, error) {
	sum := sha256.Sum256(data)
	return fmt.Sprintf("sha256:%x", sum), nil
}

func reflectFamilyEqual(base, merged Inventory, familyID string) bool {
	var baseFamily, mergedFamily *Family
	for i := range base.Families {
		if base.Families[i].ID == familyID {
			baseFamily = &base.Families[i]
			break
		}
	}
	for i := range merged.Families {
		if merged.Families[i].ID == familyID {
			mergedFamily = &merged.Families[i]
			break
		}
	}
	if baseFamily == nil || mergedFamily == nil {
		return false
	}
	return reflectDeepFamilyEqual(*baseFamily, *mergedFamily)
}

func reflectDeepFamilyEqual(left, right Family) bool {
	if left.ID != right.ID || left.Kind != right.Kind || left.Role != right.Role ||
		left.CurrentVersion != right.CurrentVersion || left.Source != right.Source ||
		left.EventualOwner != right.EventualOwner || left.NumericSemantics != right.NumericSemantics {
		return false
	}
	if !sameStringSet(left.SupportedVersions, right.SupportedVersions) {
		return false
	}
	if !sameStringSet(left.Cases, right.Cases) {
		return false
	}
	if len(left.Sources) != len(right.Sources) {
		return false
	}
	for i := range left.Sources {
		if left.Sources[i] != right.Sources[i] {
			return false
		}
	}
	return true
}

type storageSelectionDocument struct {
	ProducerID string              `json:"producer_id"`
	Cases      []CaseSpec          `json:"cases"`
	Sources    []SourceSpec        `json:"sources"`
	Routes     []consumerRouteJSON `json:"routes"`
}

func encodeStorageSelectionJSON(selection StorageSelection) storageSelectionDocument {
	routes := make([]consumerRouteJSON, 0, len(selection.Routes))
	for _, route := range selection.Routes {
		routes = append(routes, consumerRouteJSON{
			FamilyID:  route.FamilyID,
			Version:   route.Version,
			Operation: route.Operation,
		})
	}
	return storageSelectionDocument{
		ProducerID: selection.ProducerID,
		Cases:      selection.Cases,
		Sources:    selection.Sources,
		Routes:     routes,
	}
}

func exportStorageSelectionCandidateFromEnvironment(t *testing.T, repoRoot string, selection StorageSelection) string {
	t.Helper()
	exportRoot := strings.TrimSpace(os.Getenv(runtimeOracleExportDirEnv))
	if exportRoot == "" {
		return ""
	}
	assets := make([]generatedAsset, 0, len(selection.Cases)*2)
	for _, c := range selection.Cases {
		assets = append(assets,
			generatedAsset{RelativePath: c.Input.Path, Data: []byte{0x00}},
			generatedAsset{RelativePath: c.Expected.Path, Data: []byte(`{"kind":"error","category":"corrupt"}`)},
		)
	}
	manifestBytes, err := json.Marshal(encodeStorageSelectionJSON(selection))
	if err != nil {
		t.Fatalf("marshal selection: %v", err)
	}
	assets = append(assets, generatedAsset{RelativePath: storageSelectionManifest, Data: manifestBytes})
	dir, err := exportGeneratedAssets(repoRoot, exportRoot, selection.ProducerID, assets)
	if err != nil {
		t.Fatalf("export storage selection: %v", err)
	}
	return dir
}
