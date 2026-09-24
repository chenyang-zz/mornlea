package main

import (
	"bytes"
	"encoding/binary"
	"encoding/json"
	"errors"
	"fmt"
	"hash/crc32"
	"io"
	"math"
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

	regionOrderCommittedNewerAID   = regionFamily + "/" + regionVersion + "/order/committed-newer-a"
	regionOrderCommittedNewerBID   = regionFamily + "/" + regionVersion + "/order/committed-newer-b"
	regionOrderCorruptFallbackID   = regionFamily + "/" + regionVersion + "/order/corrupt-fallback"
	regionOrderStandbyBothID       = regionFamily + "/" + regionVersion + "/order/standby-both"
	regionOrderEqualIdenticalID    = regionFamily + "/" + regionVersion + "/order/equal-identical"
	regionOrderEqualDivergentID    = regionFamily + "/" + regionVersion + "/order/equal-divergent"

	regionCorruptFileSizeExtent   = "69632"  // 17 * region.SectorSize
	regionCorruptFileSizeOver1MiB = "1114112" // 272 * region.SectorSize

	regionCorruptBankOffsetInsideHeadersID = regionFamily + "/" + regionVersion + "/decode/bank-offset-inside-headers"
	regionCorruptBankOverlappingExtentsID  = regionFamily + "/" + regionVersion + "/decode/bank-overlapping-extents"
	regionCorruptBankExtentU32OverflowID   = regionFamily + "/" + regionVersion + "/decode/bank-extent-u32-overflow"
	regionCorruptBankPayloadOver1MiBID     = regionFamily + "/" + regionVersion + "/decode/bank-payload-over-1mib"
	regionCorruptBankExtentPastFileSizeID  = regionFamily + "/" + regionVersion + "/decode/bank-extent-past-file-size"
	regionCorruptBankHeaderReservedID      = regionFamily + "/" + regionVersion + "/decode/bank-header-reserved-nonzero"
	regionCorruptBankTrailingPaddingID     = regionFamily + "/" + regionVersion + "/decode/bank-trailing-padding-nonzero"
	regionCorruptBankWrongMagicID          = regionFamily + "/" + regionVersion + "/decode/bank-wrong-magic"
	regionCorruptBankVersionZeroID         = regionFamily + "/" + regionVersion + "/decode/bank-version-zero"
	regionCorruptBankVersionFuture2ID      = regionFamily + "/" + regionVersion + "/decode/bank-version-future-2"
	regionCorruptBankWrongSignedKeyID      = regionFamily + "/" + regionVersion + "/decode/bank-wrong-signed-key"
	regionCorruptBankInvalidChecksumID     = regionFamily + "/" + regionVersion + "/decode/bank-invalid-checksum"
	regionCorruptBankShortID               = regionFamily + "/" + regionVersion + "/decode/bank-short"
	regionCorruptBankTrailingID            = regionFamily + "/" + regionVersion + "/decode/bank-trailing"
	regionCorruptSuperblockShortID         = regionFamily + "/" + regionVersion + "/decode/superblock-short"
	regionCorruptSuperblockTrailingID      = regionFamily + "/" + regionVersion + "/decode/superblock-trailing"
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
	return regionBankWithGeneration(1)
}

func regionBankWithGeneration(generation uint64) region.Bank {
	bank := region.Bank{Generation: generation}
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

func regionOrderValueTree(selectedIndex int, bank region.Bank) storageValueNode {
	return storageValueObject(map[string]storageValueNode{
		"bank":           regionBankValueTree(bank),
		"selected_index": storageValueUnsigned(uint64(selectedIndex)),
	})
}

func regionOrderInput(bankA, bankB []byte) []byte {
	if len(bankA) != region.BankSize || len(bankB) != region.BankSize {
		panic("region order banks must each be BankSize bytes")
	}
	out := make([]byte, 2*region.BankSize)
	copy(out[:region.BankSize], bankA)
	copy(out[region.BankSize:], bankB)
	return out
}

func regionCorruptBankBytes(valid []byte) []byte {
	if len(valid) != region.BankSize {
		panic("corrupt bank input must be BankSize bytes")
	}
	corrupted := bytes.Clone(valid)
	corrupted[len(corrupted)-1] ^= 0xff
	return corrupted
}

func regionCRC32Table() *crc32.Table {
	return crc32.MakeTable(crc32.Castagnoli)
}

func regionRechecksumBank(encoded []byte) {
	clear(encoded[60:64])
	binary.LittleEndian.PutUint32(encoded[60:64], crc32.Checksum(encoded, regionCRC32Table()))
}

func regionMutateBank(valid []byte, offset int, mask byte, rechecksum bool) []byte {
	mutated := bytes.Clone(valid)
	mutated[offset] ^= mask
	if rechecksum {
		regionRechecksumBank(mutated)
	}
	return mutated
}

func regionPutBankU32(valid []byte, offset int, value uint32) []byte {
	mutated := bytes.Clone(valid)
	binary.LittleEndian.PutUint32(mutated[offset:], value)
	regionRechecksumBank(mutated)
	return mutated
}

func regionPutBankEntryU32(valid []byte, slot, fieldOffset int, value uint32) []byte {
	mutated := bytes.Clone(valid)
	binary.LittleEndian.PutUint32(mutated[64+slot*24+fieldOffset:], value)
	regionRechecksumBank(mutated)
	return mutated
}

func regionPutBankEntryU64(valid []byte, slot, fieldOffset int, value uint64) []byte {
	mutated := bytes.Clone(valid)
	binary.LittleEndian.PutUint64(mutated[64+slot*24+fieldOffset:], value)
	regionRechecksumBank(mutated)
	return mutated
}

func regionCorruptOccupiedBank(t *testing.T) []byte {
	t.Helper()
	key := regionSeedKey()
	bank := regionBankWithGeneration(9)
	bank.Entries[0] = region.Entry{
		OffsetSector:  15,
		SectorCount:   2,
		PayloadLength: 5000,
		Revision:      7,
		PayloadCRC32C: 0x12345678,
	}
	return encodeRegionBankBytes(t, key, bank)
}

func regionCorruptDecodeCandidate(
	t *testing.T,
	id string,
	component string,
	fileSize string,
	input []byte,
	args json.RawMessage,
) regionCandidate {
	t.Helper()
	if args == nil {
		args = regionArgumentsJSON(component, fileSize)
	}
	spec := CaseSpec{
		ID:           id,
		Family:       regionFamily,
		Version:      regionVersion,
		Operation:    "decode",
		Arguments:    args,
		InputFormat:  "binary",
		Checkpoints:  []string{"0"},
		RustConsumer: storageConsumerName,
	}
	outcome, _, err := runRegionDecode(spec, input)
	if err != nil {
		t.Fatalf("execute %s: %v", id, err)
	}
	if outcome.Kind != "error" {
		t.Fatalf("case %s expected error outcome, got %#v", id, outcome)
	}
	saveOutcome, err := outcomeToStorageSave(outcome)
	if err != nil {
		t.Fatalf("normalize %s: %v", id, err)
	}
	expectedBytes, err := marshalStorageSaveOutcome(saveOutcome)
	if err != nil {
		t.Fatalf("marshal expected %s: %v", id, err)
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
	return regionCandidate{Spec: spec, Assets: assets, Expect: saveOutcome}
}

func regionCorruptCandidates(t *testing.T) []regionCandidate {
	t.Helper()
	validBank := regionCorruptOccupiedBank(t)
	superblock := region.EncodeSuperblock(regionSeedKey())
	superBytes := superblock[:]

	wrongKeyArgs, err := json.Marshal(regionCaseArguments{
		Dimension: 0,
		X:         regionSeedX,
		Z:         regionSeedZ,
		FileSize:  regionCorruptFileSizeExtent,
		Component: "bank",
	})
	if err != nil {
		t.Fatalf("marshal wrong-key arguments: %v", err)
	}

	overlap := regionPutBankEntryU32(validBank, 1, 0, 16)
	overlap = regionPutBankEntryU32(overlap, 1, 4, 1)
	overlap = regionPutBankEntryU32(overlap, 1, 8, 1)
	overlap = regionPutBankEntryU64(overlap, 1, 12, 8)

	over1MiB := regionPutBankEntryU32(validBank, 0, 4, 257)
	over1MiB = regionPutBankEntryU32(over1MiB, 0, 8, (1<<20)+1)

	return []regionCandidate{
		regionCorruptDecodeCandidate(t, regionCorruptBankOffsetInsideHeadersID, "bank", regionCorruptFileSizeExtent,
			regionPutBankEntryU32(validBank, 0, 0, 14), nil),
		regionCorruptDecodeCandidate(t, regionCorruptBankOverlappingExtentsID, "bank", regionCorruptFileSizeExtent,
			overlap, nil),
		regionCorruptDecodeCandidate(t, regionCorruptBankExtentU32OverflowID, "bank", regionCorruptFileSizeExtent,
			regionPutBankEntryU32(validBank, 0, 0, math.MaxUint32), nil),
		regionCorruptDecodeCandidate(t, regionCorruptBankPayloadOver1MiBID, "bank", regionCorruptFileSizeOver1MiB,
			over1MiB, nil),
		regionCorruptDecodeCandidate(t, regionCorruptBankExtentPastFileSizeID, "bank", regionFileSizeOccupiedEntry,
			regionPutBankEntryU32(validBank, 0, 0, 16), nil),
		regionCorruptDecodeCandidate(t, regionCorruptBankHeaderReservedID, "bank", regionCorruptFileSizeExtent,
			regionMutateBank(validBank, 48, 1, true), nil),
		regionCorruptDecodeCandidate(t, regionCorruptBankTrailingPaddingID, "bank", regionCorruptFileSizeExtent,
			regionMutateBank(validBank, 64+1024*24, 1, true), nil),
		regionCorruptDecodeCandidate(t, regionCorruptBankWrongMagicID, "bank", regionCorruptFileSizeExtent,
			regionMutateBank(validBank, 0, 1, false), nil),
		regionCorruptDecodeCandidate(t, regionCorruptBankVersionZeroID, "bank", regionCorruptFileSizeExtent,
			regionPutBankU32(validBank, 4, 0), nil),
		regionCorruptDecodeCandidate(t, regionCorruptBankVersionFuture2ID, "bank", regionCorruptFileSizeExtent,
			regionPutBankU32(validBank, 4, 2), nil),
		regionCorruptDecodeCandidate(t, regionCorruptBankWrongSignedKeyID, "bank", regionCorruptFileSizeExtent,
			validBank, wrongKeyArgs),
		regionCorruptDecodeCandidate(t, regionCorruptBankInvalidChecksumID, "bank", regionCorruptFileSizeExtent,
			regionMutateBank(validBank, 100, 1, false), nil),
		regionCorruptDecodeCandidate(t, regionCorruptBankShortID, "bank", regionCorruptFileSizeExtent,
			bytes.Clone(validBank[:len(validBank)-1]), nil),
		regionCorruptDecodeCandidate(t, regionCorruptBankTrailingID, "bank", regionCorruptFileSizeExtent,
			append(bytes.Clone(validBank), 0), nil),
		regionCorruptDecodeCandidate(t, regionCorruptSuperblockShortID, "superblock", regionFileSizeOccupiedEntry,
			bytes.Clone(superBytes[:len(superBytes)-1]), nil),
		regionCorruptDecodeCandidate(t, regionCorruptSuperblockTrailingID, "superblock", regionFileSizeOccupiedEntry,
			append(bytes.Clone(superBytes), 0), nil),
	}
}

func regionCorruptSelection(t *testing.T, root string, candidates []regionCandidate) StorageSelection {
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

func exportRegionCorruptSelectionCandidate(t *testing.T, root string, candidates []regionCandidate) string {
	t.Helper()
	if strings.TrimSpace(os.Getenv(runtimeOracleExportDirEnv)) == "" {
		return ""
	}
	selection := regionCorruptSelection(t, root, candidates)
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
		t.Fatalf("export region corrupt selection: %v", err)
	}
	return exportRoot
}

func encodeRegionBankBytes(t *testing.T, key region.RegionKey, bank region.Bank) []byte {
	block, err := region.EncodeRegionBank(key, bank)
	if err != nil {
		t.Fatalf("encode region bank: %v", err)
	}
	return block[:]
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
	if err != nil {
		return Outcome{}, nil, fmt.Errorf("runtime-oracle: case %s: invalid case version %q", c.ID, c.Version)
	}
	if uint64(schema) != wantVersion && schema != 0 && schema <= uint32(wantVersion) {
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

func runRegionOrder(c CaseSpec, input []byte) (Outcome, []byte, error) {
	args, key, fileSize, err := parseRegionArguments(c)
	if err != nil {
		return Outcome{}, nil, fmt.Errorf("runtime-oracle: case %s: %w", c.ID, err)
	}
	if args.Component != "banks" {
		return Outcome{}, nil, fmt.Errorf("runtime-oracle: case %s: order requires component \"banks\"", c.ID)
	}
	if len(input) != 2*region.BankSize {
		return Outcome{}, nil, fmt.Errorf(
			"runtime-oracle: case %s: order input length %d, want %d",
			c.ID, len(input), 2*region.BankSize,
		)
	}
	bankABytes := input[:region.BankSize]
	bankBBytes := input[region.BankSize:]
	bankA, errA := region.DecodeRegionBank(key, bankABytes, fileSize)
	bankB, errB := region.DecodeRegionBank(key, bankBBytes, fileSize)
	selected, index, err := region.SelectRegionBank(bankA, errA, bankB, errB)
	if err != nil {
		category, ok := regionStorageErrorCategory(err)
		if !ok {
			return Outcome{}, nil, fmt.Errorf("runtime-oracle: case %s: unclassified rejection: %w", c.ID, err)
		}
		return Outcome{Kind: "error", Category: category}, nil, nil
	}
	digest := storageValueSHA256(regionOrderValueTree(index, selected))
	return Outcome{Kind: "ok", Category: "save", Fields: map[string]any{"value_sha256": digest}}, nil, nil
}

func regionCorpusRoutes() map[ConsumerRoute]GoOperation {
	return map[ConsumerRoute]GoOperation{
		{FamilyID: regionFamily, Version: regionVersion, Operation: "decode"}: runRegionDecode,
		{FamilyID: regionFamily, Version: regionVersion, Operation: "encode"}: runRegionEncode,
		{FamilyID: regionFamily, Version: regionVersion, Operation: "order"}: runRegionOrder,
	}
}

func regionSeedRoutes() []ConsumerRoute {
	return []ConsumerRoute{
		{FamilyID: regionFamily, Version: regionVersion, Operation: "decode"},
		{FamilyID: regionFamily, Version: regionVersion, Operation: "encode"},
	}
}

func regionOrderRoutes() []ConsumerRoute {
	routes := regionSeedRoutes()
	routes = append(routes, ConsumerRoute{
		FamilyID: regionFamily, Version: regionVersion, Operation: "order",
	})
	return routes
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

func regionOrderArgumentsJSON(fileSize string) json.RawMessage {
	return regionArgumentsJSON("banks", fileSize)
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

func regionOrderCandidates(t *testing.T) []regionCandidate {
	t.Helper()
	key := regionSeedKey()
	fileSize := regionFileSizeOccupiedEntry

	build := func(id string, bankA, bankB []byte) regionCandidate {
		input := regionOrderInput(bankA, bankB)
		args := regionOrderArgumentsJSON(fileSize)
		spec := CaseSpec{
			ID:           id,
			Family:       regionFamily,
			Version:      regionVersion,
			Operation:    "order",
			Arguments:    args,
			InputFormat:  "binary",
			Checkpoints:  []string{"0"},
			RustConsumer: storageConsumerName,
		}
		outcome, _, err := runRegionOrder(spec, input)
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
		stem := strings.ReplaceAll(id, "/", "_")
		inputRel := filepath.ToSlash(filepath.Join(regionCorpusRelDir, stem+".input.bin"))
		expectedRel := filepath.ToSlash(filepath.Join(regionCorpusRelDir, stem+".expected.json"))
		assets := map[string][]byte{
			inputRel:    input,
			expectedRel: expectedBytes,
		}
		spec.Input = AssetRef{Path: inputRel, SHA256: digestOf(t, input)}
		spec.Expected = AssetRef{Path: expectedRel, SHA256: digestOf(t, expectedBytes)}
		return regionCandidate{Spec: spec, Assets: assets, Expect: saveOutcome}
	}

	standby := encodeRegionBankBytes(t, key, regionStandbyBank())
	gen1 := encodeRegionBankBytes(t, key, regionBankWithGeneration(1))
	gen2 := encodeRegionBankBytes(t, key, regionBankWithGeneration(2))
	corruptGen1 := regionCorruptBankBytes(gen1)

	divergentB := regionBankWithGeneration(1)
	divergentB.Entries[0].PayloadLength = 1
	divergentGen1B := encodeRegionBankBytes(t, key, divergentB)

	return []regionCandidate{
		build(regionOrderCommittedNewerAID, gen2, gen1),
		build(regionOrderCommittedNewerBID, gen1, gen2),
		build(regionOrderCorruptFallbackID, corruptGen1, gen1),
		build(regionOrderStandbyBothID, standby, standby),
		build(regionOrderEqualIdenticalID, gen1, gen1),
		build(regionOrderEqualDivergentID, gen1, divergentGen1B),
	}
}

func regionOrderSelection(t *testing.T, root string, candidates []regionCandidate) StorageSelection {
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
		Routes:     regionOrderRoutes(),
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
	wantIDs := make(map[string]bool, len(candidates))
	for _, candidate := range candidates {
		wantIDs[candidate.Spec.ID] = true
	}
	var regionCases []CaseSpec
	for _, c := range merged.Cases {
		if c.Family == regionFamily && wantIDs[c.ID] {
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

func exportRegionOrderSelectionCandidate(t *testing.T, root string, candidates []regionCandidate) string {
	t.Helper()
	if strings.TrimSpace(os.Getenv(runtimeOracleExportDirEnv)) == "" {
		return ""
	}
	selection := regionOrderSelection(t, root, candidates)
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
		t.Fatalf("export region order selection: %v", err)
	}
	return exportRoot
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

func TestStorageRegionOrderArgumentsValidate(t *testing.T) {
	args := regionOrderArgumentsJSON(regionFileSizeOccupiedEntry)
	if err := validateStorageArguments(regionFamily, "order", args); err != nil {
		t.Fatalf("validate region order arguments: %v", err)
	}
}

func TestStorageRegionOrderBaselineCarriesRegisteredRoute(t *testing.T) {
	root := mustRepoRoot(t)
	candidates := regionOrderCandidates(t)
	selection := regionOrderSelection(t, root, candidates)
	registry := BaselineConsumerRegistry()
	for _, route := range selection.Routes {
		if !storageRouteRegistered(registry, route) {
			t.Fatalf("baseline registry missing route %s/%s/%s after integration", route.FamilyID, route.Version, route.Operation)
		}
	}
}

func TestStorageRegionOrderProducerExecutesEveryCase(t *testing.T) {
	root := mustRepoRoot(t)
	candidates := regionOrderCandidates(t)
	selection := regionOrderSelection(t, root, candidates)
	staged := regionScratchRoot(t, candidates)

	observations, err := RunStorageCases(staged, inventoryFromOrderSelection(t, root, selection), regionCorpusRoutes())
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

func inventoryFromOrderSelection(t *testing.T, root string, selection StorageSelection) Inventory {
	t.Helper()
	base, err := LoadInventory(filepath.Join(root, filepath.FromSlash(InventoryRelPath)))
	if err != nil {
		t.Fatalf("load frozen manifest: %v", err)
	}
	merged := base
	regionCases := append([]CaseSpec(nil), selection.Cases...)
	sort.Slice(regionCases, func(i, j int) bool { return regionCases[i].ID < regionCases[j].ID })
	merged.Cases = regionCases
	caseIDs := make([]string, 0, len(regionCases))
	for _, c := range regionCases {
		caseIDs = append(caseIDs, c.ID)
	}
	for index := range merged.Families {
		if merged.Families[index].ID != regionFamily {
			merged.Families[index].Cases = nil
			continue
		}
		merged.Families[index].Cases = caseIDs
		sourceByPath := make(map[string]string, len(merged.Families[index].Sources))
		for _, source := range merged.Families[index].Sources {
			sourceByPath[source.Path] = source.SHA256
		}
		for _, source := range selection.Sources {
			sourceByPath[source.Path] = source.SHA256
		}
		updated := make([]SourceSpec, 0, len(sourceByPath))
		for path, hash := range sourceByPath {
			updated = append(updated, SourceSpec{Path: path, SHA256: hash})
		}
		sort.Slice(updated, func(i, j int) bool { return updated[i].Path < updated[j].Path })
		merged.Families[index].Sources = updated
	}
	return merged
}

func TestStorageRegionOrderRunnerRejectsUnregisteredRoute(t *testing.T) {
	root := mustRepoRoot(t)
	candidates := regionOrderCandidates(t)
	selection := regionOrderSelection(t, root, candidates)
	staged := regionScratchRoot(t, candidates)
	manifest := inventoryFromOrderSelection(t, root, selection)

	decodeOnly := map[ConsumerRoute]GoOperation{
		{FamilyID: regionFamily, Version: regionVersion, Operation: "decode"}: runRegionDecode,
	}
	if _, err := RunStorageCases(staged, manifest, decodeOnly); err == nil {
		t.Fatal("expected unregistered order route rejection")
	}
}

func TestStorageRegionOrderRejectsShortInputBeforeExport(t *testing.T) {
	root := mustRepoRoot(t)
	candidates := regionOrderCandidates(t)
	if len(candidates) == 0 {
		t.Fatal("expected order candidates")
	}
	shortInput := candidates[0].Assets[candidates[0].Spec.Input.Path]
	if len(shortInput) != 2*region.BankSize {
		t.Fatalf("order candidate input length %d", len(shortInput))
	}
	half := shortInput[:region.BankSize]
	selection := regionOrderSelection(t, root, candidates)
	selection.Cases[0].Input.SHA256 = digestOf(t, half)
	dir := t.TempDir()
	writeStorageSelectionCandidate(t, dir, selection)
	full := filepath.Join(dir, filepath.FromSlash(selection.Cases[0].Input.Path))
	if err := os.MkdirAll(filepath.Dir(full), 0o755); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(full, half, 0o644); err != nil {
		t.Fatal(err)
	}
	families, _, err := Discover(root)
	if err != nil {
		t.Fatal(err)
	}
	familiesByID := make(map[string]Family, len(families))
	for _, family := range families {
		familiesByID[family.ID] = family
	}
	registry := storageRegistryWithRoutes(regionOrderRoutes())
	_, err = validateCaseSpecConsumer(dir, selection.Cases[0], familiesByID, registry)
	if err == nil || !strings.Contains(err.Error(), "57344") {
		t.Fatalf("expected short region order input rejection, got %v", err)
	}
}

func TestStorageRegionOrderCandidatesExportForReview(t *testing.T) {
	root := mustRepoRoot(t)
	exportRoot := filepath.Join(t.TempDir(), "region-order-export")
	t.Setenv(runtimeOracleExportDirEnv, exportRoot)
	candidates := regionOrderCandidates(t)
	child := exportRegionOrderSelectionCandidate(t, root, candidates)
	if child == "" {
		t.Fatal("export root unset after explicit env")
	}
	if _, err := readStorageSelection(child); err != nil {
		t.Fatalf("reload exported selection: %v", err)
	}
}

func TestStorageRegionOrderExportFromEnvironment(t *testing.T) {
	exportRoot := strings.TrimSpace(os.Getenv(runtimeOracleExportDirEnv))
	if exportRoot == "" {
		t.Skip("RUNTIME_ORACLE_EXPORT_DIR unset")
	}
	root := mustRepoRoot(t)
	candidates := regionOrderCandidates(t)
	child := exportRegionOrderSelectionCandidate(t, root, candidates)
	if child == "" {
		t.Fatal("export root unset after explicit env")
	}
	if _, err := readStorageSelection(child); err != nil {
		t.Fatalf("reload exported selection: %v", err)
	}
}

func inventoryFromCorruptSelection(t *testing.T, root string, selection StorageSelection) Inventory {
	t.Helper()
	base, err := LoadInventory(filepath.Join(root, filepath.FromSlash(InventoryRelPath)))
	if err != nil {
		t.Fatalf("load frozen manifest: %v", err)
	}
	merged := base
	regionCases := append([]CaseSpec(nil), selection.Cases...)
	sort.Slice(regionCases, func(i, j int) bool { return regionCases[i].ID < regionCases[j].ID })
	merged.Cases = regionCases
	caseIDs := make([]string, 0, len(regionCases))
	for _, c := range regionCases {
		caseIDs = append(caseIDs, c.ID)
	}
	for index := range merged.Families {
		if merged.Families[index].ID != regionFamily {
			merged.Families[index].Cases = nil
			continue
		}
		merged.Families[index].Cases = caseIDs
		sourceByPath := make(map[string]string, len(merged.Families[index].Sources))
		for _, source := range merged.Families[index].Sources {
			sourceByPath[source.Path] = source.SHA256
		}
		for _, source := range selection.Sources {
			sourceByPath[source.Path] = source.SHA256
		}
		updated := make([]SourceSpec, 0, len(sourceByPath))
		for path, hash := range sourceByPath {
			updated = append(updated, SourceSpec{Path: path, SHA256: hash})
		}
		sort.Slice(updated, func(i, j int) bool { return updated[i].Path < updated[j].Path })
		merged.Families[index].Sources = updated
	}
	return merged
}

func TestStorageRegionCorruptProducerExecutesEveryCase(t *testing.T) {
	root := mustRepoRoot(t)
	candidates := regionCorruptCandidates(t)
	selection := regionCorruptSelection(t, root, candidates)
	staged := regionScratchRoot(t, candidates)

	observations, err := RunStorageCases(staged, inventoryFromCorruptSelection(t, root, selection), regionCorpusRoutes())
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

func TestStorageRegionCorruptFutureVersionCategory(t *testing.T) {
	candidates := regionCorruptCandidates(t)
	for _, candidate := range candidates {
		if candidate.Spec.ID != regionCorruptBankVersionFuture2ID {
			continue
		}
		if candidate.Expect.Category != "future_version" {
			t.Fatalf("future version case category %q, want future_version", candidate.Expect.Category)
		}
		return
	}
	t.Fatal("missing future version corruption case")
}

func TestStorageRegionCorruptOrderBankSwapFailsStaleDigest(t *testing.T) {
	root := mustRepoRoot(t)
	orderCases := regionOrderCandidates(t)
	var newerA regionCandidate
	for _, candidate := range orderCases {
		if candidate.Spec.ID == regionOrderCommittedNewerAID {
			newerA = candidate
			break
		}
	}
	if newerA.Spec.ID == "" {
		t.Fatal("missing committed-newer-a order case")
	}
	staleExpected := newerA.Expect
	input := newerA.Assets[newerA.Spec.Input.Path]
	if len(input) != 2*region.BankSize {
		t.Fatalf("order input length %d", len(input))
	}
	key := regionSeedKey()
	gen1 := encodeRegionBankBytes(t, key, regionBankWithGeneration(1))
	swapped := regionOrderInput(gen1, input[region.BankSize:])
	spec := newerA.Spec
	spec.Input = AssetRef{Path: newerA.Spec.Input.Path, SHA256: digestOf(t, swapped)}
	outcome, _, err := runRegionOrder(spec, swapped)
	if err != nil {
		t.Fatalf("runRegionOrder: %v", err)
	}
	got, err := outcomeToStorageSave(outcome)
	if err != nil {
		t.Fatal(err)
	}
	if storageSaveOutcomesEqual(got, staleExpected) {
		t.Fatal("swapped bank A input still matches stale expected digest")
	}
	_ = root
}

func TestStorageRegionCorruptCandidatesExportForReview(t *testing.T) {
	root := mustRepoRoot(t)
	exportRoot := filepath.Join(t.TempDir(), "region-corrupt-export")
	t.Setenv(runtimeOracleExportDirEnv, exportRoot)
	candidates := regionCorruptCandidates(t)
	child := exportRegionCorruptSelectionCandidate(t, root, candidates)
	if child == "" {
		t.Fatal("export root unset after explicit env")
	}
	if _, err := readStorageSelection(child); err != nil {
		t.Fatalf("reload exported selection: %v", err)
	}
}

func TestStorageRegionCorruptExportFromEnvironment(t *testing.T) {
	exportRoot := strings.TrimSpace(os.Getenv(runtimeOracleExportDirEnv))
	if exportRoot == "" {
		t.Skip("RUNTIME_ORACLE_EXPORT_DIR unset")
	}
	root := mustRepoRoot(t)
	candidates := regionCorruptCandidates(t)
	child := exportRegionCorruptSelectionCandidate(t, root, candidates)
	if child == "" {
		t.Fatal("export root unset after explicit env")
	}
	if _, err := readStorageSelection(child); err != nil {
		t.Fatalf("reload exported selection: %v", err)
	}
}
