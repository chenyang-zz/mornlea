package main

import (
	"bytes"
	"encoding/binary"
	"encoding/json"
	"errors"
	"fmt"
	"os"
	"path/filepath"
	"sort"
	"strconv"
	"strings"
	"testing"

	"github.com/klauspost/compress/zstd"

	"github.com/channing771/mornlea/packages/server/storage/chunk"
	"github.com/channing771/mornlea/packages/server/storage/storagedef"
	"github.com/channing771/mornlea/packages/shared/core"
	"github.com/channing771/mornlea/packages/shared/world"
)

const (
	chunkFamily             = "save.chunk"
	chunkVersionV9          = "9"
	chunkProducerID         = "runtime-oracle/storage-chunk"
	chunkCorpusRelDir       = "testdata/runtime-migration/cases/storage/chunk"
	chunkProducerTestRel    = "packages/tools/cmd/runtime-oracle/storage_chunk_test.go"
	chunkCodecSourceRel     = "packages/server/storage/chunk/chunk_codec.go"
	chunkExportDir          = "/tmp/runtime-oracle-chunk-4.3b-current"
	chunkFixtureRevision    = uint64(19)
	chunkEnvelopeHeaderSize = 44

	chunkDecodeV9FluidFixtureID  = chunkFamily + "/" + chunkVersionV9 + "/decode/v9-fluid-fixture"
	chunkDecodeV9ChestRegistryID = chunkFamily + "/" + chunkVersionV9 + "/decode/v9-chest-registry"
	chunkEncodeV9CanonicalID     = chunkFamily + "/" + chunkVersionV9 + "/encode/v9-fixture-exact"
)

type chunkCaseArguments struct {
	Dimension int32  `json:"dimension"`
	X         int32  `json:"x"`
	Z         int32  `json:"z"`
	Revision  string `json:"revision"`
}

type chunkStorageSaveOutcome struct {
	Kind          string `json:"kind"`
	Category      string `json:"category,omitempty"`
	ValueSHA256   string `json:"value_sha256,omitempty"`
	LogicalLength *int   `json:"logical_length,omitempty"`
}

type chunkCandidate struct {
	Spec    CaseSpec
	Assets  map[string][]byte
	Expect  chunkStorageSaveOutcome
	Encoded []byte
}

func chunkFixtureKey() core.ChunkKey {
	return core.ChunkKey{Dimension: core.Overworld, Pos: core.ChunkPos{X: -3, Z: 7}}
}

func chunkArgumentsJSON() json.RawMessage {
	args := chunkCaseArguments{
		Dimension: int32(core.Overworld),
		X:         -3,
		Z:         7,
		Revision:  strconv.FormatUint(chunkFixtureRevision, 10),
	}
	raw, err := json.Marshal(args)
	if err != nil {
		panic(err)
	}
	return raw
}

func parseChunkCaseArguments(c CaseSpec) (chunkCaseArguments, core.ChunkKey, uint64, error) {
	var args chunkCaseArguments
	if err := json.Unmarshal(c.Arguments, &args); err != nil {
		return chunkCaseArguments{}, core.ChunkKey{}, 0, err
	}
	rev, err := strconv.ParseUint(args.Revision, 10, 64)
	if err != nil || rev == 0 {
		return chunkCaseArguments{}, core.ChunkKey{}, 0, fmt.Errorf("invalid revision")
	}
	key := core.ChunkKey{
		Dimension: core.DimensionID(args.Dimension),
		Pos:       core.ChunkPos{X: args.X, Z: args.Z},
	}
	return args, key, rev, nil
}

func chunkSchemaVersion(input []byte) (uint32, error) {
	if len(input) < 12 {
		return 0, fmt.Errorf("input shorter than schema header")
	}
	return binary.LittleEndian.Uint32(input[8:12]), nil
}

func chunkStorageErrorCategory(err error) (string, bool) {
	switch {
	case errors.Is(err, storagedef.ErrCorrupt):
		return "corrupt", true
	case errors.Is(err, storagedef.ErrFutureVersion):
		return "future_version", true
	default:
		return "", false
	}
}

func chunkItemStackValue(stack core.ItemStack) storageValueNode {
	return storageValueObject(map[string]storageValueNode{
		"item":       storageValueUnsigned(uint64(stack.Item)),
		"count":      storageValueUnsigned(uint64(stack.Count)),
		"durability": storageValueUnsigned(uint64(stack.Durability)),
	})
}

func chunkSectionValue(snapshot world.ContainerSnapshot) storageValueNode {
	palette := make([]storageValueNode, 0, len(snapshot.Palette))
	for _, id := range snapshot.Palette {
		palette = append(palette, storageValueUnsigned(uint64(id)))
	}
	packed := make([]storageValueNode, 0, len(snapshot.Packed))
	for _, word := range snapshot.Packed {
		packed = append(packed, storageValueUnsigned(word))
	}
	return storageValueObject(map[string]storageValueNode{
		"kind":    storageValueUnsigned(uint64(snapshot.Kind)),
		"bits":    storageValueUnsigned(uint64(snapshot.Bits)),
		"single":  storageValueUnsigned(uint64(snapshot.Single)),
		"palette": storageValueArray(palette),
		"packed":  storageValueArray(packed),
	})
}

func chunkDropValue(drop world.DropSlot) storageValueNode {
	return storageValueObject(map[string]storageValueNode{
		"generation":         storageValueUnsigned(uint64(drop.Generation)),
		"active":             storageValueBool(drop.Active),
		"stack":              chunkItemStackValue(drop.Stack),
		"block_index":        storageValueUnsigned(uint64(drop.BlockIndex)),
		"age_ticks":          storageValueUnsigned(uint64(drop.AgeTicks)),
		"pickup_delay_ticks": storageValueUnsigned(uint64(drop.PickupDelayTicks)),
	})
}

func chunkFurnaceValue(slot world.FurnaceSlot) storageValueNode {
	return storageValueObject(map[string]storageValueNode{
		"generation":     storageValueUnsigned(uint64(slot.Generation)),
		"active":         storageValueBool(slot.Active),
		"block_index":    storageValueUnsigned(uint64(slot.BlockIndex)),
		"input":          chunkItemStackValue(slot.Input),
		"fuel":           chunkItemStackValue(slot.Fuel),
		"output":         chunkItemStackValue(slot.Output),
		"progress_ticks": storageValueUnsigned(uint64(slot.ProgressTicks)),
		"burn_ticks":     storageValueUnsigned(uint64(slot.BurnTicks)),
	})
}

func chunkChestValue(slot world.ChestSlot) storageValueNode {
	items := make([]storageValueNode, 0, len(slot.Items))
	for _, stack := range slot.Items {
		items = append(items, chunkItemStackValue(stack))
	}
	return storageValueObject(map[string]storageValueNode{
		"generation":  storageValueUnsigned(uint64(slot.Generation)),
		"active":      storageValueBool(slot.Active),
		"block_index": storageValueUnsigned(uint64(slot.BlockIndex)),
		"items":       storageValueArray(items),
	})
}

func chunkBodyValue(body *world.Chunk) storageValueNode {
	sections := make([]storageValueNode, core.SectionsPerChunk)
	for i := 0; i < core.SectionsPerChunk; i++ {
		sections[i] = chunkSectionValue(body.Section(i).Blocks.Snapshot())
	}
	drops := make([]storageValueNode, core.DropsPerChunk)
	for i := 0; i < core.DropsPerChunk; i++ {
		drops[i] = chunkDropValue(body.Drop(i))
	}
	furnaces := make([]storageValueNode, core.FurnacesPerChunk)
	for i := 0; i < core.FurnacesPerChunk; i++ {
		furnaces[i] = chunkFurnaceValue(body.Furnace(i))
	}
	chests := make([]storageValueNode, core.ChestsPerChunk)
	for i := 0; i < core.ChestsPerChunk; i++ {
		chests[i] = chunkChestValue(body.Chest(i))
	}
	return storageValueObject(map[string]storageValueNode{
		"sections": storageValueArray(sections),
		"drops":    storageValueArray(drops),
		"furnaces": storageValueArray(furnaces),
		"chests":   storageValueArray(chests),
	})
}

func chunkKeyValue(key core.ChunkKey) storageValueNode {
	return storageValueObject(map[string]storageValueNode{
		"dimension": storageValueSigned(int64(key.Dimension)),
		"x":         storageValueSigned(int64(key.Pos.X)),
		"z":         storageValueSigned(int64(key.Pos.Z)),
	})
}

func chunkDecodedValueTree(decoded chunk.DecodedPayload) storageValueNode {
	return storageValueObject(map[string]storageValueNode{
		"key":      chunkKeyValue(decoded.Key),
		"revision": storageValueUnsigned(decoded.Revision),
		"schema":   storageValueUnsigned(9),
		"migrated": storageValueBool(decoded.Migrated),
		"chunk":    chunkBodyValue(decoded.Chunk),
	})
}

func chunkLogicalBytesFromFrame(c CaseSpec, frame []byte) ([]byte, error) {
	if len(frame) < chunkEnvelopeHeaderSize {
		return nil, fmt.Errorf("runtime-oracle: case %s: frame shorter than chunk envelope", c.ID)
	}
	declaredLogical := binary.LittleEndian.Uint32(frame[36:40])
	declaredCompressed := binary.LittleEndian.Uint32(frame[40:44])
	if int(declaredCompressed) != len(frame)-chunkEnvelopeHeaderSize {
		return nil, fmt.Errorf(
			"runtime-oracle: case %s: envelope declares %d compressed bytes, frame carries %d",
			c.ID, declaredCompressed, len(frame)-chunkEnvelopeHeaderSize,
		)
	}
	decoder, err := zstd.NewReader(nil)
	if err != nil {
		return nil, fmt.Errorf("runtime-oracle: create zstd reader: %w", err)
	}
	defer decoder.Close()
	logical, err := decoder.DecodeAll(frame[chunkEnvelopeHeaderSize:], nil)
	if err != nil {
		return nil, fmt.Errorf("runtime-oracle: case %s: decompress own output: %w", c.ID, err)
	}
	if uint32(len(logical)) != declaredLogical {
		return nil, fmt.Errorf(
			"runtime-oracle: case %s: decoded %d logical bytes, envelope declares %d",
			c.ID, len(logical), declaredLogical,
		)
	}
	return logical, nil
}

func runChunkDecode(c CaseSpec, input []byte) (Outcome, []byte, error) {
	_, key, revision, err := parseChunkCaseArguments(c)
	if err != nil {
		return Outcome{}, nil, fmt.Errorf("runtime-oracle: case %s: %w", c.ID, err)
	}
	schema, err := chunkSchemaVersion(input)
	if err != nil {
		return Outcome{}, nil, fmt.Errorf("runtime-oracle: case %s: %w", c.ID, err)
	}
	wantVersion, err := strconv.ParseUint(c.Version, 10, 32)
	if err != nil {
		return Outcome{}, nil, fmt.Errorf("runtime-oracle: case %s: invalid case version %q", c.ID, c.Version)
	}
	decoded, err := chunk.Decode(key, revision, input)
	if err != nil {
		category, ok := chunkStorageErrorCategory(err)
		if !ok {
			return Outcome{}, nil, fmt.Errorf("runtime-oracle: case %s: unclassified rejection: %w", c.ID, err)
		}
		return Outcome{Kind: "error", Category: category}, nil, nil
	}
	if uint64(schema) != wantVersion {
		return Outcome{}, nil, fmt.Errorf(
			"runtime-oracle: case %s: accepted input schema %d, want %s", c.ID, schema, c.Version,
		)
	}
	digest := storageValueSHA256(chunkDecodedValueTree(decoded))
	return Outcome{
		Kind:     "ok",
		Category: "save",
		Fields:   map[string]any{"value_sha256": digest},
	}, nil, nil
}

func runChunkEncode(c CaseSpec, input []byte) (Outcome, []byte, error) {
	_, key, revision, err := parseChunkCaseArguments(c)
	if err != nil {
		return Outcome{}, nil, fmt.Errorf("runtime-oracle: case %s: %w", c.ID, err)
	}
	decoded, err := chunk.Decode(key, revision, input)
	if err != nil {
		category, ok := chunkStorageErrorCategory(err)
		if !ok {
			return Outcome{}, nil, fmt.Errorf("runtime-oracle: case %s: unclassified rejection: %w", c.ID, err)
		}
		return Outcome{Kind: "error", Category: category}, nil, nil
	}
	digest := storageValueSHA256(chunkDecodedValueTree(decoded))
	frame, err := chunk.Encode(chunk.ChunkSave{Key: key, Revision: revision, Chunk: decoded.Chunk})
	if err != nil {
		category, ok := chunkStorageErrorCategory(err)
		if !ok {
			return Outcome{}, nil, fmt.Errorf("runtime-oracle: case %s: unclassified encode rejection: %w", c.ID, err)
		}
		return Outcome{Kind: "error", Category: category}, nil, nil
	}
	logical, err := chunkLogicalBytesFromFrame(c, frame)
	if err != nil {
		return Outcome{}, nil, err
	}
	round, err := chunk.Decode(key, revision, frame)
	if err != nil {
		return Outcome{}, nil, fmt.Errorf("runtime-oracle: case %s: re-decode output: %w", c.ID, err)
	}
	if round.Migrated {
		return Outcome{}, nil, fmt.Errorf("runtime-oracle: case %s: encoded output marked migrated", c.ID)
	}
	return Outcome{
		Kind:     "ok",
		Category: "save",
		Fields: map[string]any{
			"value_sha256":   digest,
			"logical_length": len(logical),
		},
	}, logical, nil
}

func outcomeToChunkStorageSave(out Outcome) (chunkStorageSaveOutcome, error) {
	result := chunkStorageSaveOutcome{Kind: out.Kind, Category: out.Category}
	if out.Kind == "ok" && out.Category == "save" {
		rawDigest, ok := out.Fields["value_sha256"].(string)
		if !ok || rawDigest == "" {
			return chunkStorageSaveOutcome{}, fmt.Errorf("missing value_sha256")
		}
		result.ValueSHA256 = rawDigest
		if rawLength, ok := out.Fields["logical_length"]; ok {
			converted, err := intFromAny(rawLength)
			if err != nil {
				return chunkStorageSaveOutcome{}, err
			}
			result.LogicalLength = &converted
		}
	}
	return result, nil
}

func marshalChunkStorageSaveOutcome(outcome chunkStorageSaveOutcome) ([]byte, error) {
	return json.MarshalIndent(outcome, "", "  ")
}

func chunkSaveOutcomesEqual(left, right chunkStorageSaveOutcome) bool {
	if left.Kind != right.Kind || left.Category != right.Category || left.ValueSHA256 != right.ValueSHA256 {
		return false
	}
	if left.LogicalLength == nil && right.LogicalLength == nil {
		return true
	}
	if left.LogicalLength == nil || right.LogicalLength == nil {
		return false
	}
	return *left.LogicalLength == *right.LogicalLength
}

func chunkCorpusRoutes() map[ConsumerRoute]GoOperation {
	return map[ConsumerRoute]GoOperation{
		{FamilyID: chunkFamily, Version: chunkVersionV9, Operation: "decode"}: runChunkDecode,
		{FamilyID: chunkFamily, Version: chunkVersionV9, Operation: "encode"}: runChunkEncode,
	}
}

func chunkRoutes() []ConsumerRoute {
	return []ConsumerRoute{
		{FamilyID: chunkFamily, Version: chunkVersionV9, Operation: "decode"},
		{FamilyID: chunkFamily, Version: chunkVersionV9, Operation: "encode"},
	}
}

func buildChunkCandidate(
	t *testing.T,
	id, operation, caseVersion string,
	input []byte,
	wantLogical []byte,
) chunkCandidate {
	t.Helper()
	var producer GoOperation
	switch operation {
	case "decode":
		producer = runChunkDecode
	case "encode":
		producer = runChunkEncode
	default:
		t.Fatalf("unsupported operation %q", operation)
	}
	spec := CaseSpec{
		ID:           id,
		Family:       chunkFamily,
		Version:      caseVersion,
		Operation:    operation,
		Arguments:    chunkArgumentsJSON(),
		InputFormat:  "binary",
		Checkpoints:  []string{"0"},
		RustConsumer: storageConsumerName,
	}
	outcome, producedLogical, err := producer(spec, input)
	if err != nil {
		t.Fatalf("execute %s: %v", id, err)
	}
	saveOutcome, err := outcomeToChunkStorageSave(outcome)
	if err != nil {
		t.Fatalf("normalize %s: %v", id, err)
	}
	expectedBytes, err := marshalChunkStorageSaveOutcome(saveOutcome)
	if err != nil {
		t.Fatalf("marshal expected %s: %v", id, err)
	}
	logical := wantLogical
	if operation == "encode" {
		if len(producedLogical) == 0 && saveOutcome.Kind == "ok" {
			t.Fatalf("encode case %s produced no logical bytes", id)
		}
		if saveOutcome.Kind == "ok" {
			if logical != nil && !bytes.Equal(producedLogical, logical) {
				t.Fatalf("encode case %s logical bytes mismatch", id)
			}
			logical = producedLogical
		}
	}
	stem := strings.ReplaceAll(id, "/", "_")
	inputRel := filepath.ToSlash(filepath.Join(chunkCorpusRelDir, stem+".input.bin"))
	expectedRel := filepath.ToSlash(filepath.Join(chunkCorpusRelDir, stem+".expected.json"))
	assets := map[string][]byte{
		inputRel:    input,
		expectedRel: expectedBytes,
	}
	spec.Input = AssetRef{Path: inputRel, SHA256: digestOf(t, input)}
	spec.Expected = AssetRef{Path: expectedRel, SHA256: digestOf(t, expectedBytes)}
	if operation == "encode" && saveOutcome.Kind == "ok" {
		encodedRel := filepath.ToSlash(filepath.Join(chunkCorpusRelDir, stem+".encoded.bin"))
		assets[encodedRel] = logical
		spec.Encoded = &AssetRef{Path: encodedRel, SHA256: digestOf(t, logical)}
	}
	return chunkCandidate{Spec: spec, Assets: assets, Expect: saveOutcome, Encoded: logical}
}

func readChunkFixture(t *testing.T, root string, name string) []byte {
	rel := filepath.Join("packages/server/storage/chunk/testdata", name)
	path := filepath.Join(root, filepath.FromSlash(rel))
	data, err := os.ReadFile(path)
	if err != nil {
		t.Fatalf("read %s: %v", rel, err)
	}
	return data
}

func mustEncodeChunkSave(t *testing.T, save chunk.ChunkSave) []byte {
	encoded, err := chunk.Encode(save)
	if err != nil {
		t.Fatalf("encode chunk save: %v", err)
	}
	return encoded
}

func chunkEncodedAfterDecodeFixture(t *testing.T, fixtureName string) []byte {
	key := chunkFixtureKey()
	wire := readChunkFixture(t, mustRepoRoot(t), fixtureName)
	decoded, err := chunk.Decode(key, chunkFixtureRevision, wire)
	if err != nil {
		t.Fatalf("decode %s: %v", fixtureName, err)
	}
	return mustEncodeChunkSave(t, chunk.ChunkSave{Key: key, Revision: chunkFixtureRevision, Chunk: decoded.Chunk})
}

func chunkChestRegistryWire(t *testing.T) []byte {
	return chunkEncodedAfterDecodeFixture(t, "chunk-v6.bin")
}

func chunkCurrentCandidates(t *testing.T) []chunkCandidate {
	t.Helper()
	root := mustRepoRoot(t)
	v9Fixture := readChunkFixture(t, root, "chunk-v9.bin")
	return []chunkCandidate{
		buildChunkCandidate(t, chunkDecodeV9FluidFixtureID, "decode", chunkVersionV9, v9Fixture, nil),
		buildChunkCandidate(t, chunkDecodeV9ChestRegistryID, "decode", chunkVersionV9, chunkChestRegistryWire(t), nil),
		buildChunkCandidate(t, chunkEncodeV9CanonicalID, "encode", chunkVersionV9, v9Fixture, nil),
	}
}

func chunkSelection(t *testing.T, root string, candidates []chunkCandidate) StorageSelection {
	cases := make([]CaseSpec, 0, len(candidates))
	for _, candidate := range candidates {
		cases = append(cases, candidate.Spec)
	}
	sources := []SourceSpec{
		{Path: chunkCodecSourceRel},
		{Path: chunkProducerTestRel},
	}
	for index := range sources {
		hash, err := hashFile(filepath.Join(root, filepath.FromSlash(sources[index].Path)))
		if err != nil {
			t.Fatalf("hash source %s: %v", sources[index].Path, err)
		}
		sources[index].SHA256 = hash
	}
	sort.Slice(cases, func(i, j int) bool { return cases[i].ID < cases[j].ID })
	sort.Slice(sources, func(i, j int) bool { return sources[i].Path < sources[j].Path })
	return StorageSelection{
		ProducerID: chunkProducerID,
		Cases:      cases,
		Sources:    sources,
		Routes:     chunkRoutes(),
	}
}

func chunkRunnerManifest(t *testing.T, root string, candidates []chunkCandidate) Inventory {
	t.Helper()
	base, err := LoadInventory(filepath.Join(root, filepath.FromSlash(InventoryRelPath)))
	if err != nil {
		t.Fatalf("load frozen manifest: %v", err)
	}
	selection := chunkSelection(t, root, candidates)
	merged := base
	chunkCases := append([]CaseSpec(nil), selection.Cases...)
	sort.Slice(chunkCases, func(i, j int) bool { return chunkCases[i].ID < chunkCases[j].ID })
	merged.Cases = chunkCases
	caseIDs := make([]string, 0, len(chunkCases))
	for _, c := range chunkCases {
		caseIDs = append(caseIDs, c.ID)
	}
	for index := range merged.Families {
		if merged.Families[index].ID != chunkFamily {
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

func chunkScratchRoot(t *testing.T, candidates []chunkCandidate) string {
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

func exportChunkSelectionCandidate(t *testing.T, root string, candidates []chunkCandidate) string {
	t.Helper()
	if strings.TrimSpace(os.Getenv(runtimeOracleExportDirEnv)) == "" {
		return ""
	}
	selection := chunkSelection(t, root, candidates)
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
				t.Fatalf("chunk export: duplicate asset path %s", relative)
			}
			seen[relative] = struct{}{}
			assets = append(assets, generatedAsset{RelativePath: relative, Data: data})
		}
	}
	exportRoot, err := exportGeneratedAssets(root, strings.TrimSpace(os.Getenv(runtimeOracleExportDirEnv)), chunkProducerID, assets)
	if err != nil {
		t.Fatalf("export chunk selection: %v", err)
	}
	return exportRoot
}

func TestStorageChunkCurrentProducerExecutesEveryCase(t *testing.T) {
	root := mustRepoRoot(t)
	candidates := chunkCurrentCandidates(t)
	const wantCases = 3
	if len(candidates) != wantCases {
		t.Fatalf("candidate count = %d, want %d", len(candidates), wantCases)
	}
	manifest := chunkRunnerManifest(t, root, candidates)
	staged := chunkScratchRoot(t, candidates)
	observations, err := RunStorageCases(staged, manifest, chunkCorpusRoutes())
	if err != nil {
		t.Fatalf("RunStorageCases: %v", err)
	}
	if len(observations) != len(candidates) {
		t.Fatalf("produced %d observations, want %d", len(observations), len(candidates))
	}
	for _, candidate := range candidates {
		var obs ExecutedObservation
		for _, item := range observations {
			if item.CaseID == candidate.Spec.ID {
				obs = item
				break
			}
		}
		if obs.CaseID == "" {
			t.Fatalf("no observation for case %s", candidate.Spec.ID)
		}
		got, err := outcomeToChunkStorageSave(obs.Outcome)
		if err != nil {
			t.Fatalf("case %s: %v", candidate.Spec.ID, err)
		}
		if !chunkSaveOutcomesEqual(got, candidate.Expect) {
			t.Fatalf("case %s produced %#v, want %#v", candidate.Spec.ID, got, candidate.Expect)
		}
	}
}

func TestStorageChunkCurrentSectionDigestMutationFailsComparison(t *testing.T) {
	candidates := chunkCurrentCandidates(t)
	var wire []byte
	for _, candidate := range candidates {
		if candidate.Spec.ID == chunkDecodeV9FluidFixtureID {
			wire = candidate.Assets[candidate.Spec.Input.Path]
			break
		}
	}
	if len(wire) == 0 {
		t.Fatal("missing fluid fixture wire")
	}
	spec := CaseSpec{
		ID: chunkDecodeV9FluidFixtureID, Family: chunkFamily, Version: chunkVersionV9,
		Operation: "decode", Arguments: chunkArgumentsJSON(), InputFormat: "binary",
		Checkpoints: []string{"0"}, RustConsumer: storageConsumerName,
	}
	outcome, _, err := runChunkDecode(spec, wire)
	if err != nil {
		t.Fatal(err)
	}
	got, err := outcomeToChunkStorageSave(outcome)
	if err != nil {
		t.Fatal(err)
	}
	_, key, revision, err := parseChunkCaseArguments(spec)
	if err != nil {
		t.Fatal(err)
	}
	decoded, err := chunk.Decode(key, revision, wire)
	if err != nil {
		t.Fatal(err)
	}
	stale := decoded
	stale.Chunk.SetBlock(0, int32(core.MinY+5), 0, core.StoneID)
	staleExpect := chunkStorageSaveOutcome{
		Kind: "ok", Category: "save", ValueSHA256: storageValueSHA256(chunkDecodedValueTree(stale)),
	}
	if chunkSaveOutcomesEqual(got, staleExpect) {
		t.Fatal("section mutation still matches stale expected digest")
	}
}

func TestStorageChunkCurrentDecodeDigestsDiffer(t *testing.T) {
	candidates := chunkCurrentCandidates(t)
	var chestDigest, fluidDigest string
	for _, candidate := range candidates {
		if candidate.Spec.Operation != "decode" {
			continue
		}
		if candidate.Spec.ID == chunkDecodeV9ChestRegistryID {
			chestDigest = candidate.Expect.ValueSHA256
		}
		if candidate.Spec.ID == chunkDecodeV9FluidFixtureID {
			fluidDigest = candidate.Expect.ValueSHA256
		}
	}
	if chestDigest == "" || fluidDigest == "" {
		t.Fatal("missing decode digests")
	}
	if chestDigest == fluidDigest {
		t.Fatal("distinct v9 decode cases must publish different digests")
	}
}

func TestStorageChunkCurrentExportUnsetWritesNothing(t *testing.T) {
	root := mustRepoRoot(t)
	t.Setenv(runtimeOracleExportDirEnv, "")
	before, err := os.ReadDir(root)
	if err != nil {
		t.Fatal(err)
	}
	_ = exportChunkSelectionCandidate(t, root, chunkCurrentCandidates(t))
	after, err := os.ReadDir(root)
	if err != nil {
		t.Fatal(err)
	}
	if len(before) != len(after) {
		t.Fatal("unset export must not mutate repository tree")
	}
}

func TestStorageChunkCurrentCandidatesExportForReview(t *testing.T) {
	root := mustRepoRoot(t)
	t.Setenv(runtimeOracleExportDirEnv, t.TempDir())
	child := exportChunkSelectionCandidate(t, root, chunkCurrentCandidates(t))
	if child == "" {
		t.Fatal("export root unset after explicit env")
	}
	if _, err := readStorageSelection(child); err != nil {
		t.Fatalf("reload exported selection: %v", err)
	}
}

func TestStorageChunkCurrentExportToPinnedDirectory(t *testing.T) {
	root := mustRepoRoot(t)
	t.Setenv(runtimeOracleExportDirEnv, filepath.Join(t.TempDir(), "chunk-current-export"))
	child := exportChunkSelectionCandidate(t, root, chunkCurrentCandidates(t))
	if child == "" {
		t.Fatal("export root unset after explicit env")
	}
	if _, err := readStorageSelection(child); err != nil {
		t.Fatalf("reload exported selection: %v", err)
	}
}
