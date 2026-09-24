package main

import (
	"bytes"
	"encoding/binary"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"os"
	"path/filepath"
	"reflect"
	"sort"
	"strconv"
	"strings"
	"testing"

	"github.com/channing771/mornlea/packages/server/storage/region"
	"github.com/channing771/mornlea/packages/server/storage/storagedef"
	"github.com/channing771/mornlea/packages/shared/core"
)

const (
	regionFamily            = "save.region"
	regionVersion           = "1"
	regionProducerID        = "runtime-oracle/storage-region"
	regionCorpusRelDir      = "testdata/runtime-migration/cases/storage/region"
	regionProducerTestRel   = "packages/tools/cmd/runtime-oracle/storage_region_test.go"
	regionFormatSourceRel   = "packages/server/storage/region/region_format.go"

	regionSeedDimension = -3
	regionSeedX         = -1
	regionSeedZ         = 2

	regionFileSizeEmptyBank     = "61440"
	regionFileSizeOccupiedEntry = "65536"

	regionSuperblockDecodeCaseID = regionFamily + "/" + regionVersion + "/decode/superblock-seed"
	regionBankStandbyDecodeID    = regionFamily + "/" + regionVersion + "/decode/bank-standby-gen0"
	regionBankCommittedDecodeID  = regionFamily + "/" + regionVersion + "/decode/bank-committed-gen1"
	regionBankCommittedEncodeID  = regionFamily + "/" + regionVersion + "/encode/bank-committed-gen1"
)

type regionCaseArguments struct {
	Dimension int32   `json:"dimension"`
	X         int32   `json:"x"`
	Z         int32   `json:"z"`
	FileSize  string  `json:"file_size"`
	Component string  `json:"component"`
	Capacity  *uint32 `json:"capacity,omitempty"`
}

type storageSaveOutcome struct {
	Kind        string `json:"kind"`
	Category    string `json:"category,omitempty"`
	ValueSHA256 string `json:"value_sha256,omitempty"`
	Length      *int   `json:"length,omitempty"`
	Needed      *int   `json:"needed,omitempty"`
	Available   *int   `json:"available,omitempty"`
}

type regionCandidate struct {
	Spec    CaseSpec
	Assets  map[string][]byte
	Expect  storageSaveOutcome
	Encoded []byte
}

func regionSeedKey() region.RegionKey {
	return region.RegionKey{
		Dimension: core.DimensionID(regionSeedDimension),
		X:         regionSeedX,
		Z:         regionSeedZ,
	}
}

func regionStandbyBank() region.Bank {
	return region.Bank{}
}

func regionCommittedBank() region.Bank {
	bank := region.Bank{Generation: 1}
	bank.Entries[0] = region.Entry{
		OffsetSector:  15,
		SectorCount:   1,
		PayloadLength: 0,
		Revision:      1,
		PayloadCRC32C: 0,
	}
	return bank
}

func regionEntryValueTree(entry region.Entry) storageValueNode {
	return storageValueObject(map[string]storageValueNode{
		"offset_sector":  storageValueUnsigned(uint64(entry.OffsetSector)),
		"payload_crc32c": storageValueUnsigned(uint64(entry.PayloadCRC32C)),
		"payload_length": storageValueUnsigned(uint64(entry.PayloadLength)),
		"revision":       storageValueUnsigned(entry.Revision),
		"sector_count":   storageValueUnsigned(uint64(entry.SectorCount)),
	})
}

func regionBankValueTree(bank region.Bank) storageValueNode {
	entries := make([]storageValueNode, 0, len(bank.Entries))
	for _, entry := range bank.Entries {
		entries = append(entries, regionEntryValueTree(entry))
	}
	return storageValueObject(map[string]storageValueNode{
		"entries":    storageValueArray(entries),
		"generation": storageValueUnsigned(bank.Generation),
	})
}

func regionSuperblockValueTree() storageValueNode {
	return storageValueObject(map[string]storageValueNode{})
}

func parseRegionArguments(c CaseSpec) (regionCaseArguments, region.RegionKey, int64, error) {
	var args regionCaseArguments
	dec := json.NewDecoder(bytes.NewReader(c.Arguments))
	dec.DisallowUnknownFields()
	if err := dec.Decode(&args); err != nil {
		return regionCaseArguments{}, region.RegionKey{}, 0, fmt.Errorf("decode arguments: %w", err)
	}
	var trailing any
	if err := dec.Decode(&trailing); err != io.EOF {
		return regionCaseArguments{}, region.RegionKey{}, 0, fmt.Errorf("arguments carry trailing content")
	}
	key := region.RegionKey{
		Dimension: core.DimensionID(args.Dimension),
		X:         args.X,
		Z:         args.Z,
	}
	fileSize, err := strconv.ParseInt(args.FileSize, 10, 64)
	if err != nil {
		return regionCaseArguments{}, region.RegionKey{}, 0, fmt.Errorf("file_size must be a signed decimal string")
	}
	return args, key, fileSize, nil
}

func regionSchemaVersion(encoded []byte) (uint32, error) {
	if len(encoded) < 8 {
		return 0, fmt.Errorf("input shorter than schema header")
	}
	return binary.LittleEndian.Uint32(encoded[4:8]), nil
}

func regionStorageErrorCategory(err error) (string, bool) {
	switch {
	case errors.Is(err, storagedef.ErrCorrupt):
		return "corrupt", true
	case errors.Is(err, storagedef.ErrFutureVersion):
		return "future_version", true
	default:
		return "", false
	}
}

func marshalStorageSaveOutcome(outcome storageSaveOutcome) ([]byte, error) {
	rendered, err := json.MarshalIndent(outcome, "", "  ")
	if err != nil {
		return nil, err
	}
	return append(rendered, '\n'), nil
}

func storageSaveOutcomesEqual(a, b storageSaveOutcome) bool {
	aBytes, err1 := json.Marshal(a)
	bBytes, err2 := json.Marshal(b)
	if err1 != nil || err2 != nil {
		return false
	}
	var aMap, bMap any
	decA := json.NewDecoder(bytes.NewReader(aBytes))
	decA.UseNumber()
	decB := json.NewDecoder(bytes.NewReader(bBytes))
	decB.UseNumber()
	if err := decA.Decode(&aMap); err != nil {
		return false
	}
	if err := decB.Decode(&bMap); err != nil {
		return false
	}
	return reflect.DeepEqual(aMap, bMap)
}

func runRegionDecode(c CaseSpec, input []byte) (Outcome, []byte, error) {
	args, key, fileSize, err := parseRegionArguments(c)
	if err != nil {
		return Outcome{}, nil, fmt.Errorf("runtime-oracle: case %s: %w", c.ID, err)
	}
	schema, err := regionSchemaVersion(input)
	if err != nil {
		return Outcome{}, nil, fmt.Errorf("runtime-oracle: case %s: %w", c.ID, err)
	}
	wantVersion, err := strconv.ParseUint(c.Version, 10, 32)
	if err != nil || uint64(schema) != wantVersion {
		return Outcome{}, nil, fmt.Errorf("runtime-oracle: case %s: input schema %d, want %s", c.ID, schema, c.Version)
	}

	switch args.Component {
	case "superblock":
		if err := region.DecodeSuperblock(key, input); err != nil {
			category, ok := regionStorageErrorCategory(err)
			if !ok {
				return Outcome{}, nil, fmt.Errorf("runtime-oracle: case %s: unclassified rejection: %w", c.ID, err)
			}
			return Outcome{Kind: "error", Category: category}, nil, nil
		}
		digest := storageValueSHA256(regionSuperblockValueTree())
		return Outcome{Kind: "ok", Category: "save", Fields: map[string]any{"value_sha256": digest}}, nil, nil
	case "bank":
		bank, err := region.DecodeRegionBank(key, input, fileSize)
		if err != nil {
			category, ok := regionStorageErrorCategory(err)
			if !ok {
				return Outcome{}, nil, fmt.Errorf("runtime-oracle: case %s: unclassified rejection: %w", c.ID, err)
			}
			return Outcome{Kind: "error", Category: category}, nil, nil
		}
		digest := storageValueSHA256(regionBankValueTree(bank))
		return Outcome{Kind: "ok", Category: "save", Fields: map[string]any{"value_sha256": digest}}, nil, nil
	default:
		return Outcome{}, nil, fmt.Errorf("runtime-oracle: case %s: unsupported component %q", c.ID, args.Component)
	}
}

func runRegionEncode(c CaseSpec, input []byte) (Outcome, []byte, error) {
	args, key, fileSize, err := parseRegionArguments(c)
	if err != nil {
		return Outcome{}, nil, fmt.Errorf("runtime-oracle: case %s: %w", c.ID, err)
	}
	if args.Component != "bank" && args.Component != "superblock" {
		return Outcome{}, nil, fmt.Errorf("runtime-oracle: case %s: encode requires bank or superblock component", c.ID)
	}

	var digest string
	var encoded []byte
	switch args.Component {
	case "superblock":
		if err := region.DecodeSuperblock(key, input); err != nil {
			category, ok := regionStorageErrorCategory(err)
			if !ok {
				return Outcome{}, nil, fmt.Errorf("runtime-oracle: case %s: unclassified rejection: %w", c.ID, err)
			}
			return Outcome{Kind: "error", Category: category}, nil, nil
		}
		digest = storageValueSHA256(regionSuperblockValueTree())
		block := region.EncodeSuperblock(key)
		encoded = block[:]
	case "bank":
		bank, err := region.DecodeRegionBank(key, input, fileSize)
		if err != nil {
			category, ok := regionStorageErrorCategory(err)
			if !ok {
				return Outcome{}, nil, fmt.Errorf("runtime-oracle: case %s: unclassified rejection: %w", c.ID, err)
			}
			return Outcome{Kind: "error", Category: category}, nil, nil
		}
		digest = storageValueSHA256(regionBankValueTree(bank))
		block, err := region.EncodeRegionBank(key, bank)
		if err != nil {
			category, ok := regionStorageErrorCategory(err)
			if !ok {
				return Outcome{}, nil, fmt.Errorf("runtime-oracle: case %s: unclassified encode rejection: %w", c.ID, err)
			}
			return Outcome{Kind: "error", Category: category}, nil, nil
		}
		encoded = block[:]
	}

	if args.Capacity != nil {
		if int(*args.Capacity) < len(encoded) {
			needed := len(encoded)
			available := int(*args.Capacity)
			return Outcome{
				Kind:     "error",
				Category: "output_too_small",
				Fields: map[string]any{
					"needed":    needed,
					"available": available,
				},
			}, nil, nil
		}
	}

	length := len(encoded)
	return Outcome{
		Kind:     "ok",
		Category: "save",
		Fields: map[string]any{
			"value_sha256": digest,
			"length":       length,
		},
	}, encoded, nil
}

func regionCorpusRoutes() map[ConsumerRoute]GoOperation {
	return map[ConsumerRoute]GoOperation{
		{FamilyID: regionFamily, Version: regionVersion, Operation: "decode"}: runRegionDecode,
		{FamilyID: regionFamily, Version: regionVersion, Operation: "encode"}: runRegionEncode,
	}
}

func regionSeedRoutes() []ConsumerRoute {
	return []ConsumerRoute{
		{FamilyID: regionFamily, Version: regionVersion, Operation: "decode"},
		{FamilyID: regionFamily, Version: regionVersion, Operation: "encode"},
	}
}

func regionArgumentsJSON(component, fileSize string) json.RawMessage {
	raw, err := json.Marshal(regionCaseArguments{
		Dimension: regionSeedDimension,
		X:         regionSeedX,
		Z:         regionSeedZ,
		FileSize:  fileSize,
		Component: component,
	})
	if err != nil {
		panic(err)
	}
	return raw
}

func outcomeToStorageSave(out Outcome) (storageSaveOutcome, error) {
	result := storageSaveOutcome{Kind: out.Kind, Category: out.Category}
	if out.Kind == "ok" && out.Category == "save" {
		rawDigest, ok := out.Fields["value_sha256"].(string)
		if !ok || rawDigest == "" {
			return storageSaveOutcome{}, fmt.Errorf("missing value_sha256")
		}
		result.ValueSHA256 = rawDigest
		if rawLength, ok := out.Fields["length"]; ok {
			converted, err := intFromAny(rawLength)
			if err != nil {
				return storageSaveOutcome{}, err
			}
			result.Length = &converted
		}
	}
	if out.Kind == "error" && out.Category == "output_too_small" {
		needed, err := intFromAny(out.Fields["needed"])
		if err != nil {
			return storageSaveOutcome{}, err
		}
		available, err := intFromAny(out.Fields["available"])
		if err != nil {
			return storageSaveOutcome{}, err
		}
		result.Needed = &needed
		result.Available = &available
	}
	return result, nil
}

func intFromAny(value any) (int, error) {
	switch typed := value.(type) {
	case int:
		return typed, nil
	case int64:
		return int(typed), nil
	case float64:
		return int(typed), nil
	default:
		return 0, fmt.Errorf("expected integer, got %T", value)
	}
}

func regionSeedCandidates(t *testing.T) []regionCandidate {
	t.Helper()
	key := regionSeedKey()
	superblock := region.EncodeSuperblock(key)
	standbyBytes, err := region.EncodeRegionBank(key, regionStandbyBank())
	if err != nil {
		t.Fatalf("encode standby bank: %v", err)
	}
	committedBytes, err := region.EncodeRegionBank(key, regionCommittedBank())
	if err != nil {
		t.Fatalf("encode committed bank: %v", err)
	}

	build := func(id, operation, component, fileSize string, input []byte, wantEncoded []byte) regionCandidate {
		args := regionArgumentsJSON(component, fileSize)
		var producer GoOperation
		switch operation {
		case "decode":
			producer = runRegionDecode
		case "encode":
			producer = runRegionEncode
		default:
			t.Fatalf("unsupported operation %q", operation)
		}
		spec := CaseSpec{
			ID:           id,
			Family:       regionFamily,
			Version:      regionVersion,
			Operation:    operation,
			Arguments:    args,
			InputFormat:  "binary",
			Checkpoints:  []string{"0"},
			RustConsumer: storageConsumerName,
		}
		outcome, producedEncoded, err := producer(spec, input)
		if err != nil {
			t.Fatalf("execute %s: %v", id, err)
		}
		saveOutcome, err := outcomeToStorageSave(outcome)
		if err != nil {
			t.Fatalf("normalize %s: %v", id, err)
		}
		expectedBytes, err := marshalStorageSaveOutcome(saveOutcome)
		if err != nil {
			t.Fatalf("marshal expected %s: %v", id, err)
		}
		encoded := wantEncoded
		if operation == "encode" {
			if len(producedEncoded) == 0 {
				t.Fatalf("encode case %s produced no bytes", id)
			}
			if encoded != nil && !bytes.Equal(producedEncoded, encoded) {
				t.Fatalf("encode case %s bytes mismatch", id)
			}
			encoded = producedEncoded
		}
		stem := strings.ReplaceAll(id, "/", "_")
		inputRel := filepath.ToSlash(filepath.Join(regionCorpusRelDir, stem+".input.bin"))
		expectedRel := filepath.ToSlash(filepath.Join(regionCorpusRelDir, stem+".expected.json"))
		assets := map[string][]byte{
			inputRel:    input,
			expectedRel: expectedBytes,
		}
		spec.Input = AssetRef{Path: inputRel, SHA256: digestOf(t, input)}
		spec.Expected = AssetRef{Path: expectedRel, SHA256: digestOf(t, expectedBytes)}
		if operation == "encode" && saveOutcome.Kind == "ok" {
			encodedRel := filepath.ToSlash(filepath.Join(regionCorpusRelDir, stem+".encoded.bin"))
			assets[encodedRel] = encoded
			spec.Encoded = &AssetRef{Path: encodedRel, SHA256: digestOf(t, encoded)}
		}
		return regionCandidate{Spec: spec, Assets: assets, Expect: saveOutcome, Encoded: encoded}
	}

	return []regionCandidate{
		build(regionSuperblockDecodeCaseID, "decode", "superblock", regionFileSizeOccupiedEntry, superblock[:], nil),
		build(regionBankStandbyDecodeID, "decode", "bank", regionFileSizeEmptyBank, standbyBytes[:], nil),
		build(regionBankCommittedDecodeID, "decode", "bank", regionFileSizeOccupiedEntry, committedBytes[:], nil),
		build(regionBankCommittedEncodeID, "encode", "bank", regionFileSizeOccupiedEntry, committedBytes[:], committedBytes[:]),
	}
}

func regionSelection(t *testing.T, root string, candidates []regionCandidate) StorageSelection {
	t.Helper()
	cases := make([]CaseSpec, 0, len(candidates))
	for _, candidate := range candidates {
		cases = append(cases, candidate.Spec)
	}
	sources := []SourceSpec{
		{Path: regionProducerTestRel},
		{Path: regionFormatSourceRel},
	}
	for index := range sources {
		hash, err := hashFile(filepath.Join(root, filepath.FromSlash(sources[index].Path)))
		if err != nil {
			t.Fatalf("hash source %s: %v", sources[index].Path, err)
		}
		sources[index].SHA256 = hash
	}
	sort.Slice(cases, func(i, j int) bool { return cases[i].ID < cases[j].ID })
	return StorageSelection{
		ProducerID: regionProducerID,
		Cases:      cases,
		Sources:    sources,
		Routes:     regionSeedRoutes(),
	}
}

func regionManifest(t *testing.T, root string, candidates []regionCandidate) Inventory {
	t.Helper()
	base, err := LoadInventory(filepath.Join(root, filepath.FromSlash(InventoryRelPath)))
	if err != nil {
		t.Fatalf("load frozen manifest: %v", err)
	}
	selection := regionSelection(t, root, candidates)
	integrated := true
	for _, want := range selection.Cases {
		found := false
		for _, existing := range base.Cases {
			if existing.ID == want.ID {
				found = true
				break
			}
		}
		if !found {
			integrated = false
			break
		}
	}
	if integrated {
		return base
	}
	merged, err := mergeStorageSelections(root, base, selection)
	if err != nil {
		t.Fatalf("merge region selection: %v", err)
	}
	return merged
}

func regionScratchRoot(t *testing.T, candidates []regionCandidate) string {
	t.Helper()
	dir := t.TempDir()
	for _, candidate := range candidates {
		for relative, data := range candidate.Assets {
			full := filepath.Join(dir, filepath.FromSlash(relative))
			if err := os.MkdirAll(filepath.Dir(full), 0o755); err != nil {
				t.Fatalf("mkdir asset parent: %v", err)
			}
			if err := os.WriteFile(full, data, 0o644); err != nil {
				t.Fatalf("write asset %s: %v", relative, err)
			}
		}
	}
	return dir
}

func regionObservation(t *testing.T, observations []ExecutedObservation, id string) ExecutedObservation {
	t.Helper()
	for _, obs := range observations {
		if obs.CaseID == id {
			return obs
		}
	}
	t.Fatalf("no observation for case %s", id)
	return ExecutedObservation{}
}

func regionRunnerManifest(t *testing.T, root string, candidates []regionCandidate) Inventory {
	t.Helper()
	merged := regionManifest(t, root, candidates)
	var regionCases []CaseSpec
	for _, c := range merged.Cases {
		if c.Family == regionFamily {
			regionCases = append(regionCases, c)
		}
	}
	sort.Slice(regionCases, func(i, j int) bool { return regionCases[i].ID < regionCases[j].ID })
	merged.Cases = regionCases
	caseIDs := make([]string, 0, len(regionCases))
	for _, c := range regionCases {
		caseIDs = append(caseIDs, c.ID)
	}
	for index := range merged.Families {
		if merged.Families[index].ID == regionFamily {
			merged.Families[index].Cases = caseIDs
		} else {
			merged.Families[index].Cases = nil
		}
	}
	return merged
}

func exportRegionSelectionCandidate(t *testing.T, root string, candidates []regionCandidate) string {
	t.Helper()
	if strings.TrimSpace(os.Getenv(runtimeOracleExportDirEnv)) == "" {
		return ""
	}
	selection := regionSelection(t, root, candidates)
	var assets []generatedAsset
	manifestBytes, err := json.Marshal(encodeStorageSelectionJSON(selection))
	if err != nil {
		t.Fatalf("marshal selection: %v", err)
	}
	assets = append(assets, generatedAsset{RelativePath: storageSelectionManifest, Data: manifestBytes})
	seen := make(map[string]struct{})
	for _, candidate := range candidates {
		for relative, data := range candidate.Assets {
			if _, ok := seen[relative]; ok {
				continue
			}
			seen[relative] = struct{}{}
			assets = append(assets, generatedAsset{RelativePath: relative, Data: data})
		}
	}
	exportRoot, err := exportGeneratedAssets(root, strings.TrimSpace(os.Getenv(runtimeOracleExportDirEnv)), regionProducerID, assets)
	if err != nil {
		t.Fatalf("export region selection: %v", err)
	}
	return exportRoot
}

func TestStorageRegionSeedArgumentsValidate(t *testing.T) {
	args := regionArgumentsJSON("bank", regionFileSizeOccupiedEntry)
	if err := validateStorageArguments(regionFamily, "decode", args); err != nil {
		t.Fatalf("validate region decode arguments: %v", err)
	}
	if err := validateStorageArguments(regionFamily, "encode", args); err != nil {
		t.Fatalf("validate region encode arguments: %v", err)
	}
}

func TestStorageRegionBaselineCarriesRegisteredRoutes(t *testing.T) {
	root := mustRepoRoot(t)
	candidates := regionSeedCandidates(t)
	selection := regionSelection(t, root, candidates)
	registry := BaselineConsumerRegistry()
	for _, route := range selection.Routes {
		if !storageRouteRegistered(registry, route) {
			t.Fatalf("baseline registry missing route %s/%s/%s after integration", route.FamilyID, route.Version, route.Operation)
		}
	}
}

func TestStorageRegionProducerExecutesEverySeedCase(t *testing.T) {
	root := mustRepoRoot(t)
	candidates := regionSeedCandidates(t)
	manifest := regionRunnerManifest(t, root, candidates)
	staged := regionScratchRoot(t, candidates)

	observations, err := RunStorageCases(staged, manifest, regionCorpusRoutes())
	if err != nil {
		t.Fatalf("RunStorageCases: %v", err)
	}
	if len(observations) != len(candidates) {
		t.Fatalf("produced %d observations, want %d", len(observations), len(candidates))
	}
	for _, candidate := range candidates {
		obs := regionObservation(t, observations, candidate.Spec.ID)
		got, err := outcomeToStorageSave(obs.Outcome)
		if err != nil {
			t.Fatalf("case %s: %v", candidate.Spec.ID, err)
		}
		if !storageSaveOutcomesEqual(got, candidate.Expect) {
			t.Fatalf("case %s produced %#v, want %#v", candidate.Spec.ID, got, candidate.Expect)
		}
	}
}

func TestStorageRegionRunnerRejectsUnregisteredRoute(t *testing.T) {
	root := mustRepoRoot(t)
	candidates := regionSeedCandidates(t)
	manifest := regionRunnerManifest(t, root, candidates)
	staged := regionScratchRoot(t, candidates)

	decodeOnly := map[ConsumerRoute]GoOperation{
		{FamilyID: regionFamily, Version: regionVersion, Operation: "decode"}: runRegionDecode,
	}
	if _, err := RunStorageCases(staged, manifest, decodeOnly); err == nil {
		t.Fatal("expected unregistered encode route rejection")
	}
}

func TestStorageRegionSelectionReadRejectsMalformedCandidateHash(t *testing.T) {
	root := mustRepoRoot(t)
	candidates := regionSeedCandidates(t)
	dir := filepath.Join(t.TempDir(), "bad-hash")
	selection := regionSelection(t, root, candidates)
	selection.Cases[0].Input.SHA256 = "sha256:" + strings.Repeat("0", 64)
	writeStorageSelectionCandidate(t, dir, selection)
	for relative, data := range candidates[0].Assets {
		if relative != selection.Cases[0].Input.Path {
			continue
		}
		full := filepath.Join(dir, filepath.FromSlash(relative))
		if err := os.MkdirAll(filepath.Dir(full), 0o755); err != nil {
			t.Fatal(err)
		}
		if err := os.WriteFile(full, data, 0o644); err != nil {
			t.Fatal(err)
		}
	}
	if _, err := readStorageSelection(dir); err == nil {
		t.Fatal("expected digest mismatch before integration")
	}
}

func TestStorageRegionExportUnsetWritesNothing(t *testing.T) {
	root := mustRepoRoot(t)
	t.Setenv(runtimeOracleExportDirEnv, "")
	before, err := os.ReadDir(root)
	if err != nil {
		t.Fatal(err)
	}
	_ = exportRegionSelectionCandidate(t, root, regionSeedCandidates(t))
	after, err := os.ReadDir(root)
	if err != nil {
		t.Fatal(err)
	}
	if len(before) != len(after) {
		t.Fatal("unset export must not mutate repository tree")
	}
}

func TestStorageRegionCandidatesExportForReview(t *testing.T) {
	root := mustRepoRoot(t)
	exportRoot := filepath.Join(t.TempDir(), "region-export-parent")
	t.Setenv(runtimeOracleExportDirEnv, exportRoot)
	candidates := regionSeedCandidates(t)
	child := exportRegionSelectionCandidate(t, root, candidates)
	if child == "" {
		t.Fatal("export root unset after explicit env")
	}
	if _, err := readStorageSelection(child); err != nil {
		t.Fatalf("reload exported selection: %v", err)
	}
}

// TestStorageRegionWriteIntegratedManifest encodes the merged region selection
// into MORNLEA_WRITE_INVENTORY after ReconcileWorking. The controller sets
// that path to the tracked contracts file.
func TestStorageRegionWriteIntegratedManifest(t *testing.T) {
	dest := strings.TrimSpace(os.Getenv("MORNLEA_WRITE_INVENTORY"))
	if dest == "" {
		t.Skip("MORNLEA_WRITE_INVENTORY unset")
	}
	root := mustRepoRoot(t)
	merged := regionManifest(t, root, regionSeedCandidates(t))
	families, live, err := Discover(root)
	if err != nil {
		t.Fatalf("discover: %v", err)
	}
	if _, err := ReconcileWorking(root, merged, families, live, BaselineConsumerRegistry(), BaselineNegativeCoverageExceptions()); err != nil {
		t.Fatalf("reconcile merged manifest: %v", err)
	}
	encoded, err := encodeInventory(merged)
	if err != nil {
		t.Fatalf("encode inventory: %v", err)
	}
	if err := os.WriteFile(dest, append(encoded, '\n'), 0o644); err != nil {
		t.Fatalf("write inventory: %v", err)
	}
}

// TestStorageRegionExportFromEnvironment writes the reviewed selection and
// assets when RUNTIME_ORACLE_EXPORT_DIR is set outside the test. The
// controller uses this hook to integrate tracked manifest assets.
func TestStorageRegionExportFromEnvironment(t *testing.T) {
	exportRoot := strings.TrimSpace(os.Getenv(runtimeOracleExportDirEnv))
	if exportRoot == "" {
		t.Skip("RUNTIME_ORACLE_EXPORT_DIR unset")
	}
	root := mustRepoRoot(t)
	candidates := regionSeedCandidates(t)
	child := exportRegionSelectionCandidate(t, root, candidates)
	if child == "" {
		t.Fatal("export root unset after explicit env")
	}
	if _, err := readStorageSelection(child); err != nil {
		t.Fatalf("reload exported selection: %v", err)
	}
}
