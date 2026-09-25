package chunk

import (
	"bytes"
	"crypto/sha256"
	"encoding/binary"
	"encoding/json"
	"errors"
	"fmt"
	"math"
	"os"
	"path/filepath"
	"sort"
	"strconv"
	"strings"
	"testing"

	"github.com/channing771/mornlea/packages/server/storage/region"
	"github.com/channing771/mornlea/packages/server/storage/storagedef"
	"github.com/channing771/mornlea/packages/shared/core"
	"github.com/channing771/mornlea/packages/shared/world"
)

const (
	chunkFamily                     = "save.chunk"
	chunkProducerID                 = "chunk/migration"
	chunkCorpusRelDir               = "testdata/runtime-migration/cases/storage/chunk"
	chunkProducerTestRel            = "packages/server/storage/chunk/chunk_oracle_test.go"
	chunkCodecSourceRel             = "packages/server/storage/chunk/chunk_codec.go"
	chunkPinnedExportDir            = "/tmp/runtime-oracle-chunk-4.3a"
	chunkPinnedExportDirLate        = "/tmp/runtime-oracle-chunk-4.3b-late-fix"
	chunkPinnedExportDirAdversarial = "/tmp/runtime-oracle-chunk-4.3c-adversarial"
	chunkPinnedExportDirCross       = "/tmp/runtime-oracle-chunk-4.3c-cross"
	chunkSelectionManifest          = "selection.json"
	chunkRustConsumer               = "mornlea_storage"
	chunkRuntimeOracleExportDirEnv  = "RUNTIME_ORACLE_EXPORT_DIR"
	chunkRustFrameInputEnv          = "RUST_STORAGE_FRAME_INPUT"

	chunkFixtureRevision = uint64(19)
)

type chunkConsumerRoute struct {
	FamilyID  string `json:"family_id"`
	Version   string `json:"version"`
	Operation string `json:"operation"`
}

type chunkAssetRef struct {
	Path   string `json:"path"`
	SHA256 string `json:"sha256"`
}

type chunkCaseSpec struct {
	ID           string          `json:"id"`
	Family       string          `json:"family"`
	Version      string          `json:"version"`
	Operation    string          `json:"operation"`
	Arguments    json.RawMessage `json:"arguments,omitempty"`
	Input        chunkAssetRef   `json:"input"`
	InputFormat  string          `json:"input_format"`
	Expected     chunkAssetRef   `json:"expected"`
	Checkpoints  []string        `json:"checkpoints"`
	RustConsumer string          `json:"rust_consumer"`
}

type chunkSourceSpec struct {
	Path   string `json:"path"`
	SHA256 string `json:"sha256"`
}

type chunkSelection struct {
	ProducerID string               `json:"producer_id"`
	Cases      []chunkCaseSpec      `json:"cases"`
	Sources    []chunkSourceSpec    `json:"sources"`
	Routes     []chunkConsumerRoute `json:"routes"`
}

type chunkSaveOutcome struct {
	Kind        string `json:"kind"`
	Category    string `json:"category,omitempty"`
	ValueSHA256 string `json:"value_sha256,omitempty"`
}

type chunkCandidate struct {
	Spec   chunkCaseSpec
	Assets map[string][]byte
	Expect chunkSaveOutcome
}

type chunkGeneratedAsset struct {
	RelativePath string
	Data         []byte
}

var chunkOracleValidProducerIDs = map[string]bool{"chunk/migration": true}

const (
	chunkTagNull     = 0x00
	chunkTagFalse    = 0x01
	chunkTagTrue     = 0x02
	chunkTagSigned   = 0x03
	chunkTagUnsigned = 0x04
	chunkTagF32      = 0x05
	chunkTagUTF8     = 0x06
	chunkTagBytes    = 0x07
	chunkTagArray    = 0x08
	chunkTagObject   = 0x09
)

type chunkValueNode struct {
	null     bool
	boolean  *bool
	signed   *int64
	unsigned *uint64
	f32bits  *uint32
	utf8     *string
	bytes    []byte
	array    []chunkValueNode
	object   map[string]chunkValueNode
}

func chunkValueNull() chunkValueNode             { return chunkValueNode{null: true} }
func chunkValueBool(v bool) chunkValueNode       { return chunkValueNode{boolean: &v} }
func chunkValueSigned(v int64) chunkValueNode    { return chunkValueNode{signed: &v} }
func chunkValueUnsigned(v uint64) chunkValueNode { return chunkValueNode{unsigned: &v} }

func chunkValueF32(v float32) chunkValueNode {
	bits := math.Float32bits(v)
	return chunkValueNode{f32bits: &bits}
}

func chunkValueObject(fields map[string]chunkValueNode) chunkValueNode {
	return chunkValueNode{object: fields}
}

func chunkEncodeValueNode(out *bytes.Buffer, node chunkValueNode) {
	switch {
	case node.null:
		out.WriteByte(chunkTagNull)
	case node.boolean != nil:
		if *node.boolean {
			out.WriteByte(chunkTagTrue)
		} else {
			out.WriteByte(chunkTagFalse)
		}
	case node.signed != nil:
		out.WriteByte(chunkTagSigned)
		_ = binary.Write(out, binary.LittleEndian, *node.signed)
	case node.unsigned != nil:
		out.WriteByte(chunkTagUnsigned)
		_ = binary.Write(out, binary.LittleEndian, *node.unsigned)
	case node.f32bits != nil:
		out.WriteByte(chunkTagF32)
		_ = binary.Write(out, binary.LittleEndian, *node.f32bits)
	case node.object != nil:
		out.WriteByte(chunkTagObject)
		keys := make([]string, 0, len(node.object))
		for key := range node.object {
			keys = append(keys, key)
		}
		sort.Strings(keys)
		_ = binary.Write(out, binary.LittleEndian, uint32(len(keys)))
		for _, key := range keys {
			keyBytes := []byte(key)
			_ = binary.Write(out, binary.LittleEndian, uint32(len(keyBytes)))
			out.Write(keyBytes)
			chunkEncodeValueNode(out, node.object[key])
		}
	case node.array != nil:
		out.WriteByte(chunkTagArray)
		_ = binary.Write(out, binary.LittleEndian, uint32(len(node.array)))
		for _, item := range node.array {
			chunkEncodeValueNode(out, item)
		}
	default:
		panic("empty chunk value node")
	}
}

func chunkValueSHA256(node chunkValueNode) string {
	var out bytes.Buffer
	chunkEncodeValueNode(&out, node)
	sum := sha256.Sum256(out.Bytes())
	return fmt.Sprintf("sha256:%x", sum)
}

func chunkFixtureKey() core.ChunkKey {
	return core.ChunkKey{Dimension: core.Overworld, Pos: core.ChunkPos{X: -3, Z: 7}}
}

func chunkArgumentsJSON() json.RawMessage {
	return chunkArgumentsJSONFor(-3, strconv.FormatUint(chunkFixtureRevision, 10))
}

func chunkArgumentsJSONFor(x int32, revision string) json.RawMessage {
	args := map[string]any{
		"dimension": int32(core.Overworld),
		"x":         x,
		"z":         int32(7),
		"revision":  revision,
	}
	raw, err := json.Marshal(args)
	if err != nil {
		panic(err)
	}
	return raw
}

type chunkCaseArguments struct {
	Dimension int32  `json:"dimension"`
	X         int32  `json:"x"`
	Z         int32  `json:"z"`
	Revision  string `json:"revision"`
}

func parseChunkArguments(raw json.RawMessage) (chunkCaseArguments, core.ChunkKey, uint64, error) {
	var args chunkCaseArguments
	if err := json.Unmarshal(raw, &args); err != nil {
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

func chunkSchemaVersion(encoded []byte) (uint32, error) {
	if len(encoded) < 12 {
		return 0, fmt.Errorf("input shorter than schema header")
	}
	return binary.LittleEndian.Uint32(encoded[8:12]), nil
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

func chunkItemStackValue(stack core.ItemStack) chunkValueNode {
	return chunkValueObject(map[string]chunkValueNode{
		"item":       chunkValueUnsigned(uint64(stack.Item)),
		"count":      chunkValueUnsigned(uint64(stack.Count)),
		"durability": chunkValueUnsigned(uint64(stack.Durability)),
	})
}

func chunkSectionValue(snapshot world.ContainerSnapshot) chunkValueNode {
	palette := make([]chunkValueNode, 0, len(snapshot.Palette))
	for _, id := range snapshot.Palette {
		palette = append(palette, chunkValueUnsigned(uint64(id)))
	}
	packed := make([]chunkValueNode, 0, len(snapshot.Packed))
	for _, word := range snapshot.Packed {
		packed = append(packed, chunkValueUnsigned(word))
	}
	return chunkValueObject(map[string]chunkValueNode{
		"kind":    chunkValueUnsigned(uint64(snapshot.Kind)),
		"bits":    chunkValueUnsigned(uint64(snapshot.Bits)),
		"single":  chunkValueUnsigned(uint64(snapshot.Single)),
		"palette": chunkValueNode{array: palette},
		"packed":  chunkValueNode{array: packed},
	})
}

func chunkDropValue(drop world.DropSlot) chunkValueNode {
	return chunkValueObject(map[string]chunkValueNode{
		"generation":         chunkValueUnsigned(uint64(drop.Generation)),
		"active":             chunkValueBool(drop.Active),
		"stack":              chunkItemStackValue(drop.Stack),
		"block_index":        chunkValueUnsigned(uint64(drop.BlockIndex)),
		"age_ticks":          chunkValueUnsigned(uint64(drop.AgeTicks)),
		"pickup_delay_ticks": chunkValueUnsigned(uint64(drop.PickupDelayTicks)),
	})
}

func chunkFurnaceValue(slot world.FurnaceSlot) chunkValueNode {
	return chunkValueObject(map[string]chunkValueNode{
		"generation":     chunkValueUnsigned(uint64(slot.Generation)),
		"active":         chunkValueBool(slot.Active),
		"block_index":    chunkValueUnsigned(uint64(slot.BlockIndex)),
		"input":          chunkItemStackValue(slot.Input),
		"fuel":           chunkItemStackValue(slot.Fuel),
		"output":         chunkItemStackValue(slot.Output),
		"progress_ticks": chunkValueUnsigned(uint64(slot.ProgressTicks)),
		"burn_ticks":     chunkValueUnsigned(uint64(slot.BurnTicks)),
	})
}

func chunkChestValue(slot world.ChestSlot) chunkValueNode {
	items := make([]chunkValueNode, 0, len(slot.Items))
	for _, stack := range slot.Items {
		items = append(items, chunkItemStackValue(stack))
	}
	return chunkValueObject(map[string]chunkValueNode{
		"generation":  chunkValueUnsigned(uint64(slot.Generation)),
		"active":      chunkValueBool(slot.Active),
		"block_index": chunkValueUnsigned(uint64(slot.BlockIndex)),
		"items":       chunkValueNode{array: items},
	})
}

func chunkBodyValue(chunk *world.Chunk) chunkValueNode {
	sections := make([]chunkValueNode, core.SectionsPerChunk)
	for i := 0; i < core.SectionsPerChunk; i++ {
		sections[i] = chunkSectionValue(chunk.Section(i).Blocks.Snapshot())
	}
	drops := make([]chunkValueNode, core.DropsPerChunk)
	for i := 0; i < core.DropsPerChunk; i++ {
		drops[i] = chunkDropValue(chunk.Drop(i))
	}
	furnaces := make([]chunkValueNode, core.FurnacesPerChunk)
	for i := 0; i < core.FurnacesPerChunk; i++ {
		furnaces[i] = chunkFurnaceValue(chunk.Furnace(i))
	}
	chests := make([]chunkValueNode, core.ChestsPerChunk)
	for i := 0; i < core.ChestsPerChunk; i++ {
		chests[i] = chunkChestValue(chunk.Chest(i))
	}
	return chunkValueObject(map[string]chunkValueNode{
		"sections": chunkValueNode{array: sections},
		"drops":    chunkValueNode{array: drops},
		"furnaces": chunkValueNode{array: furnaces},
		"chests":   chunkValueNode{array: chests},
	})
}

func chunkKeyValue(key core.ChunkKey) chunkValueNode {
	return chunkValueObject(map[string]chunkValueNode{
		"dimension": chunkValueSigned(int64(key.Dimension)),
		"x":         chunkValueSigned(int64(key.Pos.X)),
		"z":         chunkValueSigned(int64(key.Pos.Z)),
	})
}

func chunkDecodedValueTree(decoded DecodedPayload) chunkValueNode {
	return chunkValueObject(map[string]chunkValueNode{
		"key":      chunkKeyValue(decoded.Key),
		"revision": chunkValueUnsigned(decoded.Revision),
		"schema":   chunkValueUnsigned(uint64(currentChunkSchema)),
		"migrated": chunkValueBool(decoded.Migrated),
		"chunk":    chunkBodyValue(decoded.Chunk),
	})
}

func chunkRunDecode(spec chunkCaseSpec, input []byte) (chunkSaveOutcome, error) {
	_, key, revision, err := parseChunkArguments(spec.Arguments)
	if err != nil {
		return chunkSaveOutcome{}, err
	}
	wantVersion, err := strconv.ParseUint(spec.Version, 10, 32)
	if err != nil {
		return chunkSaveOutcome{}, fmt.Errorf("invalid case version %q", spec.Version)
	}
	schema, err := chunkSchemaVersion(input)
	if err != nil {
		return chunkSaveOutcome{}, err
	}
	decoded, err := Decode(key, revision, input)
	if err != nil {
		category, ok := chunkStorageErrorCategory(err)
		if !ok {
			return chunkSaveOutcome{}, fmt.Errorf("unclassified rejection: %w", err)
		}
		return chunkSaveOutcome{Kind: "error", Category: category}, nil
	}
	if uint64(schema) != wantVersion {
		return chunkSaveOutcome{}, fmt.Errorf("accepted input schema %d, want %d", schema, wantVersion)
	}
	digest := chunkValueSHA256(chunkDecodedValueTree(decoded))
	return chunkSaveOutcome{Kind: "ok", Category: "save", ValueSHA256: digest}, nil
}

func chunkDigestOf(data []byte) string {
	sum := sha256.Sum256(data)
	return fmt.Sprintf("sha256:%x", sum)
}

func chunkHashFile(path string) (string, error) {
	data, err := os.ReadFile(path)
	if err != nil {
		return "", err
	}
	return chunkDigestOf(data), nil
}

func chunkMarshalOutcome(outcome chunkSaveOutcome) ([]byte, error) {
	return json.MarshalIndent(outcome, "", "  ")
}

func chunkReadFixture(t *testing.T, name string) []byte {
	t.Helper()
	data, err := os.ReadFile(filepath.Join("testdata", name))
	if err != nil {
		t.Fatalf("read fixture %s: %v", name, err)
	}
	return data
}

func chunkTruncatedWire(wire []byte, dropTail int) []byte {
	if dropTail <= 0 || dropTail >= len(wire) {
		return bytes.Clone(wire)
	}
	return bytes.Clone(wire[:len(wire)-dropTail])
}

func chunkCorruptCompressedWire(wire []byte) []byte {
	out := bytes.Clone(wire)
	if len(out) > 44 {
		out[44] ^= 0xff
	}
	return out
}

func chunkWireWithSchema(wire []byte, schema uint32) []byte {
	out := bytes.Clone(wire)
	if len(out) >= 12 {
		binary.LittleEndian.PutUint32(out[8:12], schema)
	}
	return out
}

func chunkBuildCandidate(t *testing.T, id, caseVersion string, input []byte) chunkCandidate {
	return chunkBuildCandidateWithArgs(t, id, caseVersion, input, chunkArgumentsJSON())
}

func chunkBuildCandidateWithArgs(
	t *testing.T,
	id, caseVersion string,
	input []byte,
	args json.RawMessage,
) chunkCandidate {
	t.Helper()
	spec := chunkCaseSpec{
		ID: id, Family: chunkFamily, Version: caseVersion, Operation: "decode",
		Arguments: args, InputFormat: "binary", Checkpoints: []string{"0"},
		RustConsumer: chunkRustConsumer,
	}
	outcome, err := chunkRunDecode(spec, input)
	if err != nil {
		t.Fatalf("execute %s: %v", id, err)
	}
	expectedBytes, err := chunkMarshalOutcome(outcome)
	if err != nil {
		t.Fatalf("marshal expected %s: %v", id, err)
	}
	stem := strings.ReplaceAll(id, "/", "_")
	inputRel := filepath.ToSlash(filepath.Join(chunkCorpusRelDir, stem+".input.bin"))
	expectedRel := filepath.ToSlash(filepath.Join(chunkCorpusRelDir, stem+".expected.json"))
	assets := map[string][]byte{inputRel: input, expectedRel: expectedBytes}
	spec.Input = chunkAssetRef{Path: inputRel, SHA256: chunkDigestOf(input)}
	spec.Expected = chunkAssetRef{Path: expectedRel, SHA256: chunkDigestOf(expectedBytes)}
	return chunkCandidate{Spec: spec, Assets: assets, Expect: outcome}
}

func chunkRoutes() []chunkConsumerRoute {
	var routes []chunkConsumerRoute
	for version := 4; version >= 1; version-- {
		routes = append(routes, chunkConsumerRoute{
			FamilyID: chunkFamily, Version: strconv.Itoa(version), Operation: "decode",
		})
	}
	return routes
}

func chunkLateRoutes() []chunkConsumerRoute {
	var routes []chunkConsumerRoute
	for version := 9; version >= 5; version-- {
		routes = append(routes, chunkConsumerRoute{
			FamilyID: chunkFamily, Version: strconv.Itoa(version), Operation: "decode",
		})
	}
	return routes
}

func chunkAdversarialBaseWire(t *testing.T) []byte {
	t.Helper()
	key := chunkFixtureKey()
	encoded, err := Encode(ChunkSave{
		Key: key, Revision: chunkFixtureRevision, Chunk: world.NewChunk(key.Pos),
	})
	if err != nil {
		t.Fatalf("encode adversarial base: %v", err)
	}
	return encoded
}

func chunkTruncatedEnvelopeWire(wire []byte) []byte {
	if len(wire) >= 44 {
		return bytes.Clone(wire[:43])
	}
	return bytes.Clone(wire)
}

func chunkTruncatedFrameWire(wire []byte) []byte {
	if len(wire) <= 44 {
		return bytes.Clone(wire)
	}
	declared := binary.LittleEndian.Uint32(wire[40:44])
	out := bytes.Clone(wire[:44])
	compressed := wire[44:]
	if len(compressed) > 1 {
		out = append(out, compressed[:len(compressed)-1]...)
	} else if len(compressed) > 0 {
		out = append(out, compressed[0])
	}
	binary.LittleEndian.PutUint32(out[40:44], declared)
	return out
}

func chunkTrailingByteWire(wire []byte) []byte {
	return append(bytes.Clone(wire), 0)
}

func chunkBrokenZstdChecksumWire(wire []byte) []byte {
	out := bytes.Clone(wire)
	if len(out) == 0 {
		return out
	}
	out[len(out)-1] ^= 0xff
	return out
}

func chunkDeclaredLogicalOversizeWire(wire []byte) []byte {
	out := bytes.Clone(wire)
	if len(out) >= 40 {
		binary.LittleEndian.PutUint32(out[36:40], maxDecodedChunk+1)
	}
	return out
}

func chunkDeclaredCompressedOversizeWire(wire []byte) []byte {
	out := bytes.Clone(wire)
	if len(out) >= 44 {
		binary.LittleEndian.PutUint32(out[40:44], region.MaxCompressedChunk+1)
	}
	return out
}

func chunkPaletteErrorWire(t *testing.T, key core.ChunkKey) []byte {
	t.Helper()
	logical := testLogicalChunk(key, chunkFixtureRevision, func(index int) world.ContainerSnapshot {
		if index == 0 {
			packed := make([]uint64, 256)
			packed[0] = 1
			return world.ContainerSnapshot{
				Kind: world.StorageIndexed, Bits: 4,
				Palette: []core.BlockID{0}, Packed: packed,
			}
		}
		return world.ContainerSnapshot{Kind: world.StorageSingle, Single: core.AirID}
	})
	return testEnvelope(key, chunkFixtureRevision, logical)
}

func chunkActiveContainerMismatchWire(t *testing.T, key core.ChunkKey) []byte {
	t.Helper()
	logical := testLogicalChunk(key, chunkFixtureRevision, func(int) world.ContainerSnapshot {
		return world.ContainerSnapshot{Kind: world.StorageSingle, Single: core.AirID}
	})
	for range core.DropsPerChunk {
		logical = appendLogicalDropSlot(logical, world.DropSlot{})
	}
	logical = appendLogicalFurnaceSlot(logical, world.FurnaceSlot{
		Generation: 1, Active: true, BlockIndex: 0,
	})
	for slot := 1; slot < core.FurnacesPerChunk; slot++ {
		logical = appendLogicalFurnaceSlot(logical, world.FurnaceSlot{})
	}
	for range core.ChestsPerChunk {
		logical = appendLogicalChestSlot(logical, world.ChestSlot{})
	}
	return testEnvelope(key, chunkFixtureRevision, logical)
}

func chunkInsufficientDropSlotsWire(t *testing.T, key core.ChunkKey) []byte {
	t.Helper()
	chunk := codecFixtureChunk(key.Pos)
	index, ok := world.ChunkBlockIndex(core.BlockPos{
		X: key.Pos.X << core.SectionShift, Y: 5, Z: key.Pos.Z << core.SectionShift,
	})
	if !ok {
		t.Fatal("fixture block index missing for insufficient drop slots")
	}
	chunk.SetDrop(0, world.DropSlot{
		Generation: 1, Active: true,
		Stack:      core.ItemStack{Item: core.ItemStonePickaxe, Count: 2},
		BlockIndex: index,
	})
	for slot := 1; slot < core.DropsPerChunk; slot++ {
		if slot%2 == 0 {
			chunk.SetDrop(slot, world.DropSlot{Generation: math.MaxUint32})
			continue
		}
		chunk.SetDrop(slot, world.DropSlot{
			Generation: uint32(slot), Active: true,
			Stack:      core.ItemStack{Item: core.ItemStone, Count: 1},
			BlockIndex: index,
		})
	}
	return legacyV4ChunkPayload(t, key, chunkFixtureRevision, chunk)
}

func chunkAdversarialRoutes() []chunkConsumerRoute {
	return []chunkConsumerRoute{
		{FamilyID: chunkFamily, Version: "9", Operation: "decode"},
		{FamilyID: chunkFamily, Version: "4", Operation: "decode"},
	}
}

func chunkAdversarialCandidates(t *testing.T) []chunkCandidate {
	t.Helper()
	key := chunkFixtureKey()
	base := chunkAdversarialBaseWire(t)
	args := chunkArgumentsJSON()
	wrongKeyArgs := chunkArgumentsJSONFor(-2, strconv.FormatUint(chunkFixtureRevision, 10))
	wrongRevisionArgs := chunkArgumentsJSONFor(-3, "20")
	return []chunkCandidate{
		chunkBuildCandidateWithArgs(t, chunkFamily+"/9/decode/invalid-version-zero", "9",
			chunkWireWithSchema(base, 0), args),
		chunkBuildCandidateWithArgs(t, chunkFamily+"/9/decode/invalid-version-future", "9",
			chunkWireWithSchema(base, 10), args),
		chunkBuildCandidateWithArgs(t, chunkFamily+"/9/decode/wrong-key", "9", base, wrongKeyArgs),
		chunkBuildCandidateWithArgs(t, chunkFamily+"/9/decode/wrong-revision", "9", base, wrongRevisionArgs),
		chunkBuildCandidateWithArgs(t, chunkFamily+"/9/decode/truncated-envelope", "9",
			chunkTruncatedEnvelopeWire(base), args),
		chunkBuildCandidateWithArgs(t, chunkFamily+"/9/decode/truncated-frame", "9",
			chunkTruncatedFrameWire(base), args),
		chunkBuildCandidateWithArgs(t, chunkFamily+"/9/decode/trailing-byte", "9",
			chunkTrailingByteWire(base), args),
		chunkBuildCandidateWithArgs(t, chunkFamily+"/9/decode/checksum", "9",
			chunkBrokenZstdChecksumWire(base), args),
		chunkBuildCandidateWithArgs(t, chunkFamily+"/9/decode/declared-logical-oversize", "9",
			chunkDeclaredLogicalOversizeWire(base), args),
		chunkBuildCandidateWithArgs(t, chunkFamily+"/9/decode/compressed-oversize", "9",
			chunkDeclaredCompressedOversizeWire(base), args),
		chunkBuildCandidateWithArgs(t, chunkFamily+"/9/decode/palette-error", "9",
			chunkPaletteErrorWire(t, key), args),
		chunkBuildCandidateWithArgs(t, chunkFamily+"/9/decode/active-container-mismatch", "9",
			chunkActiveContainerMismatchWire(t, key), args),
		chunkBuildCandidateWithArgs(t, chunkFamily+"/4/decode/insufficient-drop-slots", "4",
			chunkInsufficientDropSlotsWire(t, key), args),
	}
}

func chunkCrossCandidateFromFrame(t *testing.T, frame []byte) chunkCandidate {
	t.Helper()
	schema, err := chunkSchemaVersion(frame)
	if err != nil {
		t.Fatalf("cross frame schema: %v", err)
	}
	if schema != 9 {
		t.Fatalf("cross frame schema %d, want 9", schema)
	}
	return chunkBuildCandidate(t, chunkFamily+"/9/decode/rust-v9-frame", "9", frame)
}

func chunkLateCandidates(t *testing.T) []chunkCandidate {
	t.Helper()
	v5 := chunkReadFixture(t, "chunk-v5.bin")
	v6 := chunkReadFixture(t, "chunk-v6.bin")
	v7 := chunkReadFixture(t, "chunk-v7.bin")
	v8 := chunkReadFixture(t, "chunk-v8.bin")
	v9 := chunkReadFixture(t, "chunk-v9.bin")
	return []chunkCandidate{
		chunkBuildCandidate(t, chunkFamily+"/5/decode/v5-fixture", "5", v5),
		chunkBuildCandidate(t, chunkFamily+"/5/decode/corrupt-crc", "5", chunkCorruptCompressedWire(v5)),
		chunkBuildCandidate(t, chunkFamily+"/6/decode/v6-fixture", "6", v6),
		chunkBuildCandidate(t, chunkFamily+"/6/decode/truncated-payload", "6", chunkTruncatedWire(v6, 1)),
		chunkBuildCandidate(t, chunkFamily+"/7/decode/v7-fixture", "7", v7),
		chunkBuildCandidate(t, chunkFamily+"/7/decode/invalid-version-zero", "7", chunkWireWithSchema(v7, 0)),
		chunkBuildCandidate(t, chunkFamily+"/8/decode/v8-fixture", "8", v8),
		chunkBuildCandidate(t, chunkFamily+"/8/decode/invalid-version-future", "8", chunkWireWithSchema(v8, 10)),
		chunkBuildCandidate(t, chunkFamily+"/9/decode/v9-fixture", "9", v9),
		chunkBuildCandidate(t, chunkFamily+"/9/decode/truncated-payload", "9", chunkTruncatedWire(v9, 1)),
	}
}

func chunkEarlyCandidates(t *testing.T) []chunkCandidate {
	t.Helper()
	v1 := chunkReadFixture(t, "chunk-v1.bin")
	v2 := chunkReadFixture(t, "chunk-v2.bin")
	v3 := chunkReadFixture(t, "chunk-v3.bin")
	v4 := chunkReadFixture(t, "chunk-v4.bin")
	return []chunkCandidate{
		chunkBuildCandidate(t, chunkFamily+"/1/decode/v1-fixture", "1", v1),
		chunkBuildCandidate(t, chunkFamily+"/1/decode/truncated-payload", "1", chunkTruncatedWire(v1, 1)),
		chunkBuildCandidate(t, chunkFamily+"/2/decode/v2-fixture", "2", v2),
		chunkBuildCandidate(t, chunkFamily+"/2/decode/corrupt-crc", "2", chunkCorruptCompressedWire(v2)),
		chunkBuildCandidate(t, chunkFamily+"/3/decode/v3-fixture", "3", v3),
		chunkBuildCandidate(t, chunkFamily+"/3/decode/invalid-version-zero", "3", chunkWireWithSchema(v3, 0)),
		chunkBuildCandidate(t, chunkFamily+"/4/decode/v4-fixture", "4", v4),
		chunkBuildCandidate(t, chunkFamily+"/4/decode/invalid-version-future", "4", chunkWireWithSchema(v4, 10)),
	}
}

func buildChunkOracleSelection(t *testing.T, root string, candidates []chunkCandidate, routes []chunkConsumerRoute) chunkSelection {
	t.Helper()
	cases := make([]chunkCaseSpec, 0, len(candidates))
	for _, candidate := range candidates {
		cases = append(cases, candidate.Spec)
	}
	sources := []chunkSourceSpec{{Path: chunkCodecSourceRel}, {Path: chunkProducerTestRel}}
	for index := range sources {
		hash, err := chunkHashFile(filepath.Join(root, filepath.FromSlash(sources[index].Path)))
		if err != nil {
			t.Fatalf("hash source %s: %v", sources[index].Path, err)
		}
		sources[index].SHA256 = hash
	}
	sort.Slice(cases, func(i, j int) bool { return cases[i].ID < cases[j].ID })
	sort.Slice(sources, func(i, j int) bool { return sources[i].Path < sources[j].Path })
	return chunkSelection{
		ProducerID: chunkProducerID,
		Cases:      cases,
		Sources:    sources,
		Routes:     routes,
	}
}

func chunkExportSelection(t *testing.T, root string, candidates []chunkCandidate, routes []chunkConsumerRoute) string {
	t.Helper()
	if strings.TrimSpace(os.Getenv(chunkRuntimeOracleExportDirEnv)) == "" {
		return ""
	}
	selection := buildChunkOracleSelection(t, root, candidates, routes)
	manifestBytes, err := json.Marshal(selection)
	if err != nil {
		t.Fatalf("marshal selection: %v", err)
	}
	var assets []chunkGeneratedAsset
	assets = append(assets, chunkGeneratedAsset{RelativePath: chunkSelectionManifest, Data: manifestBytes})
	for _, candidate := range candidates {
		for relative, data := range candidate.Assets {
			assets = append(assets, chunkGeneratedAsset{RelativePath: relative, Data: data})
		}
	}
	exportRoot := strings.TrimSpace(os.Getenv(chunkRuntimeOracleExportDirEnv))
	dir, err := chunkOracleExportGeneratedAssets(root, exportRoot, chunkProducerID, assets)
	if err != nil {
		t.Fatalf("export chunk selection: %v", err)
	}
	return dir
}

func chunkRepoRoot(t *testing.T) string {
	t.Helper()
	dir, err := os.Getwd()
	if err != nil {
		t.Fatalf("working directory: %v", err)
	}
	for {
		if _, err := os.Stat(filepath.Join(dir, "go.work")); err == nil {
			return dir
		}
		parent := filepath.Dir(dir)
		if parent == dir {
			t.Fatal("repository root not found")
		}
		dir = parent
	}
}

func chunkOutcomesEqual(left, right chunkSaveOutcome) bool {
	return left.Kind == right.Kind && left.Category == right.Category && left.ValueSHA256 == right.ValueSHA256
}

func chunkOracleValidateProducerID(producerID string) ([]string, error) {
	if strings.TrimSpace(producerID) == "" || strings.Contains(producerID, "\\") || filepath.IsAbs(producerID) {
		return nil, fmt.Errorf("chunk oracle: producer ID must be a clean relative slash path: %q", producerID)
	}
	parts := strings.Split(producerID, "/")
	for _, part := range parts {
		if part == "" || part == "." || part == ".." {
			return nil, fmt.Errorf("chunk oracle: producer ID must be a clean relative slash path: %q", producerID)
		}
	}
	if filepath.ToSlash(filepath.Clean(filepath.FromSlash(producerID))) != producerID {
		return nil, fmt.Errorf("chunk oracle: producer ID must be a clean relative slash path: %q", producerID)
	}
	return parts, nil
}

func chunkOracleValidateGeneratedAssets(assets []chunkGeneratedAsset) error {
	seen := make(map[string]struct{}, len(assets))
	paths := make([]string, 0, len(assets))
	for _, asset := range assets {
		if strings.TrimSpace(asset.RelativePath) == "" {
			return fmt.Errorf("chunk oracle: empty asset relative path")
		}
		if strings.Contains(asset.RelativePath, "\\") {
			return fmt.Errorf("chunk oracle: backslash rejected in relative path: %s", asset.RelativePath)
		}
		if filepath.IsAbs(asset.RelativePath) || strings.HasPrefix(asset.RelativePath, "/") {
			return fmt.Errorf("chunk oracle: absolute path rejected: %s", asset.RelativePath)
		}
		parts := strings.Split(asset.RelativePath, "/")
		for _, part := range parts {
			if part == "." || part == ".." {
				return fmt.Errorf("chunk oracle: relative path contains ./..: %s", asset.RelativePath)
			}
			if part == "" {
				return fmt.Errorf("chunk oracle: asset relative path must be clean: %s", asset.RelativePath)
			}
		}
		cleaned := filepath.Clean(filepath.FromSlash(asset.RelativePath))
		if cleaned == "." || cleaned == ".." || strings.HasPrefix(cleaned, ".."+string(filepath.Separator)) {
			return fmt.Errorf("chunk oracle: path escapes producer directory: %s", asset.RelativePath)
		}
		normalized := filepath.ToSlash(cleaned)
		if normalized != asset.RelativePath {
			return fmt.Errorf("chunk oracle: asset relative path must be clean: %s", asset.RelativePath)
		}
		if _, duplicate := seen[normalized]; duplicate {
			return fmt.Errorf("chunk oracle: duplicate asset relative path: %s", asset.RelativePath)
		}
		seen[normalized] = struct{}{}
		paths = append(paths, normalized)
	}
	for _, relative := range paths {
		parts := strings.Split(relative, "/")
		for index := 1; index < len(parts); index++ {
			parent := strings.Join(parts[:index], "/")
			if _, collision := seen[parent]; collision {
				return fmt.Errorf("chunk oracle: asset path conflicts with another asset: %s", relative)
			}
		}
	}
	return nil
}

func chunkOracleIsLivePath(root, target string) (bool, error) {
	if strings.TrimSpace(target) == "" {
		return false, nil
	}
	absRoot, err := filepath.Abs(root)
	if err != nil {
		return false, fmt.Errorf("chunk oracle: resolve repository root: %w", err)
	}
	absTarget, err := filepath.Abs(target)
	if err != nil {
		return false, fmt.Errorf("chunk oracle: resolve path %s: %w", target, err)
	}
	if resolved, err := filepath.EvalSymlinks(absRoot); err == nil {
		absRoot = resolved
	}
	if resolved, err := filepath.EvalSymlinks(absTarget); err == nil {
		absTarget = resolved
	} else {
		parent := filepath.Dir(absTarget)
		if resolvedParent, parentErr := filepath.EvalSymlinks(parent); parentErr == nil {
			absTarget = filepath.Join(resolvedParent, filepath.Base(absTarget))
		}
	}
	rel, err := filepath.Rel(absRoot, absTarget)
	if err != nil {
		return false, fmt.Errorf("chunk oracle: compare path %s: %w", target, err)
	}
	if rel == "." {
		return true, nil
	}
	if rel == ".." {
		return false, nil
	}
	return !strings.HasPrefix(rel, ".."+string(filepath.Separator)), nil
}

func chunkOracleCreateExportRoot(absExport, existing string, missing []string) error {
	info, err := os.Lstat(existing)
	if err != nil {
		return fmt.Errorf("chunk oracle: stat export ancestor %s: %w", existing, err)
	}
	if !info.IsDir() {
		return fmt.Errorf("chunk oracle: export-prefix non-directory rejected: %s", existing)
	}
	current := existing
	for _, component := range missing {
		current = filepath.Join(current, component)
		info, err := os.Lstat(current)
		switch {
		case err == nil && info.Mode()&os.ModeSymlink != 0:
			return fmt.Errorf("chunk oracle: export-prefix symlink rejected: %s", current)
		case err == nil && !info.IsDir():
			return fmt.Errorf("chunk oracle: export-prefix non-directory rejected: %s", current)
		case err == nil:
			continue
		case !os.IsNotExist(err):
			return fmt.Errorf("chunk oracle: stat export path %s: %w", current, err)
		}
		if err := os.Mkdir(current, 0o755); err != nil {
			return fmt.Errorf("chunk oracle: create export directory %s: %w", current, err)
		}
	}
	if current != absExport {
		return fmt.Errorf("chunk oracle: export root walk ended at %s, want %s", current, absExport)
	}
	return nil
}

func chunkOracleCreateProducerChild(exportRoot string, components []string) (string, error) {
	current := exportRoot
	for index, component := range components {
		current = filepath.Join(current, component)
		final := index == len(components)-1
		info, err := os.Lstat(current)
		switch {
		case err == nil && info.Mode()&os.ModeSymlink != 0:
			return "", fmt.Errorf("chunk oracle: producer-prefix symlink rejected: %s", current)
		case err == nil && final:
			return "", fmt.Errorf("chunk oracle: producer child already exists: %s", current)
		case err == nil && !info.IsDir():
			return "", fmt.Errorf("chunk oracle: producer-prefix non-directory rejected: %s", current)
		case err == nil:
			continue
		case !os.IsNotExist(err):
			return "", fmt.Errorf("chunk oracle: stat producer component %s: %w", current, err)
		}
		if err := os.Mkdir(current, 0o755); err != nil {
			return "", fmt.Errorf("chunk oracle: create producer directory %s: %w", current, err)
		}
	}
	return current, nil
}

func chunkOracleCreateAssetParents(producerChild string, assets []chunkGeneratedAsset) error {
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
				return fmt.Errorf("chunk oracle: asset-prefix symlink rejected: %s", current)
			case err == nil && !info.IsDir():
				return fmt.Errorf("chunk oracle: asset-prefix non-directory rejected: %s", current)
			case err == nil:
				continue
			case !os.IsNotExist(err):
				return fmt.Errorf("chunk oracle: stat asset directory %s: %w", current, err)
			}
			if err := os.Mkdir(current, 0o755); err != nil {
				return fmt.Errorf("chunk oracle: create asset directory %s: %w", current, err)
			}
		}
	}
	return nil
}

func chunkOracleVerifyExportedAssets(producerChild string, assets []chunkGeneratedAsset) error {
	var selection chunkSelection
	manifestParsed := false
	for _, asset := range assets {
		target := filepath.Join(producerChild, filepath.FromSlash(asset.RelativePath))
		onDisk, err := os.ReadFile(target)
		if err != nil {
			return fmt.Errorf("chunk oracle: read exported asset %s: %w", asset.RelativePath, err)
		}
		if !bytes.Equal(onDisk, asset.Data) {
			return fmt.Errorf("chunk oracle: exported asset %s bytes differ from payload", asset.RelativePath)
		}
		if asset.RelativePath == chunkSelectionManifest {
			if err := json.Unmarshal(onDisk, &selection); err != nil {
				return fmt.Errorf("chunk oracle: parse exported selection: %w", err)
			}
			manifestParsed = true
		}
	}
	if !manifestParsed {
		return fmt.Errorf("chunk oracle: selection manifest missing from export assets")
	}
	digests := make(map[string]string, len(assets))
	for _, asset := range assets {
		digests[asset.RelativePath] = chunkDigestOf(asset.Data)
	}
	for _, caseSpec := range selection.Cases {
		refs := []chunkAssetRef{caseSpec.Input, caseSpec.Expected}
		for _, ref := range refs {
			digest, ok := digests[ref.Path]
			if !ok {
				return fmt.Errorf("chunk oracle: selection references missing export asset %s", ref.Path)
			}
			if digest != ref.SHA256 {
				return fmt.Errorf("chunk oracle: selection hash mismatch for %s: got %s, want %s", ref.Path, digest, ref.SHA256)
			}
		}
	}
	return nil
}

func chunkOracleRecheckExportContainment(repoRoot, exportRoot, producerChild string) error {
	resolvedExport, err := filepath.EvalSymlinks(exportRoot)
	if err != nil {
		return fmt.Errorf("chunk oracle: resolve created export root %s: %w", exportRoot, err)
	}
	resolvedProducer, err := filepath.EvalSymlinks(producerChild)
	if err != nil {
		return fmt.Errorf("chunk oracle: resolve created producer child %s: %w", producerChild, err)
	}
	rel, err := filepath.Rel(resolvedExport, resolvedProducer)
	if err != nil || rel == ".." || strings.HasPrefix(rel, ".."+string(filepath.Separator)) {
		return fmt.Errorf("chunk oracle: producer child escaped export root: %s", producerChild)
	}
	live, err := chunkOracleIsLivePath(repoRoot, resolvedProducer)
	if err != nil {
		return err
	}
	if live {
		return fmt.Errorf("chunk oracle: live-path write rejected: producer child %s is inside repository", producerChild)
	}
	return nil
}

func chunkOracleExportGeneratedAssets(repoRoot, exportRoot, producerID string, assets []chunkGeneratedAsset) (string, error) {
	producerComponents, err := chunkOracleValidateProducerID(producerID)
	if err != nil {
		return "", err
	}
	if !chunkOracleValidProducerIDs[producerID] {
		return "", fmt.Errorf("chunk oracle: unrecognized producer ID: %q", producerID)
	}
	if strings.TrimSpace(exportRoot) == "" {
		return "", fmt.Errorf("chunk oracle: export root cannot be empty")
	}
	if err := chunkOracleValidateGeneratedAssets(assets); err != nil {
		return "", err
	}
	absRoot, err := filepath.Abs(repoRoot)
	if err != nil {
		return "", fmt.Errorf("chunk oracle: resolve repository root %s: %w", repoRoot, err)
	}
	if resolved, err := filepath.EvalSymlinks(absRoot); err == nil {
		absRoot = resolved
	}
	absExport, err := filepath.Abs(exportRoot)
	if err != nil {
		return "", fmt.Errorf("chunk oracle: resolve export root %s: %w", exportRoot, err)
	}
	var missing []string
	existing := absExport
	for {
		_, statErr := os.Lstat(existing)
		if statErr == nil {
			break
		}
		if !os.IsNotExist(statErr) {
			return "", fmt.Errorf("chunk oracle: stat export root %s: %w", existing, statErr)
		}
		missing = append(missing, filepath.Base(existing))
		parent := filepath.Dir(existing)
		if parent == existing {
			return "", fmt.Errorf("chunk oracle: export root %s has no existing ancestor", exportRoot)
		}
		existing = parent
	}
	for i, j := 0, len(missing)-1; i < j; i, j = i+1, j-1 {
		missing[i], missing[j] = missing[j], missing[i]
	}
	resolvedExisting, err := filepath.EvalSymlinks(existing)
	if err != nil {
		return "", fmt.Errorf("chunk oracle: resolve export ancestor %s: %w", existing, err)
	}
	resolvedExport := filepath.Join(append([]string{resolvedExisting}, missing...)...)
	live, err := chunkOracleIsLivePath(absRoot, resolvedExport)
	if err != nil {
		return "", err
	}
	if live {
		return "", fmt.Errorf("chunk oracle: live-path write rejected: export root %s is inside repository", exportRoot)
	}
	liveExisting, err := chunkOracleIsLivePath(absRoot, existing)
	if err != nil {
		return "", err
	}
	if liveExisting {
		return "", fmt.Errorf("chunk oracle: live-path write rejected: export ancestor %s is inside repository", existing)
	}
	for component := existing; ; component = filepath.Dir(component) {
		if component == "/" || component == "." || component == filepath.Dir(component) {
			break
		}
		if component == "/var" || component == "/tmp" || component == "/etc" {
			break
		}
		info, statErr := os.Lstat(component)
		if statErr != nil {
			return "", fmt.Errorf("chunk oracle: stat export path %s: %w", component, statErr)
		}
		if info.Mode()&os.ModeSymlink != 0 {
			return "", fmt.Errorf("chunk oracle: symlink component rejected: %s", component)
		}
	}
	if err := chunkOracleCreateExportRoot(absExport, existing, missing); err != nil {
		return "", err
	}
	producerChild, err := chunkOracleCreateProducerChild(absExport, producerComponents)
	if err != nil {
		return "", err
	}
	if err := chunkOracleCreateAssetParents(producerChild, assets); err != nil {
		return "", err
	}
	if err := chunkOracleRecheckExportContainment(absRoot, absExport, producerChild); err != nil {
		return "", err
	}
	for _, asset := range assets {
		target := filepath.Join(producerChild, filepath.FromSlash(asset.RelativePath))
		f, err := os.OpenFile(target, os.O_WRONLY|os.O_CREATE|os.O_EXCL, 0o644)
		if err != nil {
			return "", fmt.Errorf("chunk oracle: create exclusive asset %s: %w", asset.RelativePath, err)
		}
		if _, err := f.Write(asset.Data); err != nil {
			f.Close()
			return "", fmt.Errorf("chunk oracle: write asset %s: %w", asset.RelativePath, err)
		}
		if err := f.Close(); err != nil {
			return "", fmt.Errorf("chunk oracle: close asset %s: %w", asset.RelativePath, err)
		}
	}
	if err := chunkOracleVerifyExportedAssets(producerChild, assets); err != nil {
		return "", err
	}
	return producerChild, nil
}

func TestChunkMigrationOracleEarly(t *testing.T) {
	t.Setenv(chunkRuntimeOracleExportDirEnv, "")
	root := chunkRepoRoot(t)
	candidates := chunkEarlyCandidates(t)
	key := chunkFixtureKey()
	for _, candidate := range candidates {
		input := candidate.Assets[candidate.Spec.Input.Path]
		got, err := chunkRunDecode(candidate.Spec, input)
		if err != nil {
			t.Fatalf("decode %s: %v", candidate.Spec.ID, err)
		}
		if !chunkOutcomesEqual(got, candidate.Expect) {
			t.Fatalf("case %s produced %#v, want %#v", candidate.Spec.ID, got, candidate.Expect)
		}
	}
	v1, err := Decode(key, chunkFixtureRevision, chunkReadFixture(t, "chunk-v1.bin"))
	if err != nil {
		t.Fatal(err)
	}
	empty := world.NewChunk(key.Pos)
	if v1.Chunk.DropsHash() != empty.DropsHash() {
		t.Fatal("v1 fixture must migrate to empty drops")
	}
	v2, err := Decode(key, chunkFixtureRevision, chunkReadFixture(t, "chunk-v2.bin"))
	if err != nil {
		t.Fatal(err)
	}
	wantDrops := dropFixtureChunk(t, key.Pos)
	if v2.Chunk.DropsHash() != wantDrops.DropsHash() {
		t.Fatal("v2 fixture drops mismatch")
	}
	for slot := range core.FurnacesPerChunk {
		if v2.Chunk.Furnace(slot) != empty.Furnace(slot) {
			t.Fatalf("v2 fixture furnace slot %d must stay empty", slot)
		}
	}
	for slot := range core.ChestsPerChunk {
		if v2.Chunk.Chest(slot) != empty.Chest(slot) {
			t.Fatalf("v2 fixture chest slot %d must stay empty", slot)
		}
	}
	v3, err := Decode(key, chunkFixtureRevision, chunkReadFixture(t, "chunk-v3.bin"))
	if err != nil {
		t.Fatal(err)
	}
	if v3.Chunk.DropsHash() != wantDrops.DropsHash() {
		t.Fatal("v3 fixture drops mismatch")
	}
	for slot := range core.FurnacesPerChunk {
		if v3.Chunk.Furnace(slot) != empty.Furnace(slot) {
			t.Fatalf("v3 fixture furnace slot %d must stay empty", slot)
		}
	}
	v4, err := Decode(key, chunkFixtureRevision, chunkReadFixture(t, "chunk-v4.bin"))
	if err != nil {
		t.Fatal(err)
	}
	wantFurnace := furnaceFixtureChunk(t, key.Pos)
	if v4.Chunk.Furnace(0) != wantFurnace.Furnace(0) {
		t.Fatal("v4 fixture must preserve furnace state")
	}
	full, _ := core.ItemMaxDurability(core.ItemStonePickaxe)
	wantTool := core.ItemStack{Item: core.ItemStonePickaxe, Count: 1, Durability: full}
	toolChunk := codecFixtureChunk(key.Pos)
	blockIndex, ok := world.ChunkBlockIndex(core.BlockPos{
		X: key.Pos.X << core.SectionShift, Y: 5, Z: key.Pos.Z << core.SectionShift,
	})
	if !ok {
		t.Fatal("fixture block index missing for legacy v4 tool pin")
	}
	toolChunk.SetDrop(5, world.DropSlot{
		Generation: 11, Active: true,
		Stack:      core.ItemStack{Item: core.ItemStonePickaxe, Count: 2},
		BlockIndex: blockIndex, AgeTicks: 101, PickupDelayTicks: 9,
	})
	toolDecoded, err := Decode(key, chunkFixtureRevision, legacyV4ChunkPayload(t, key, chunkFixtureRevision, toolChunk))
	if err != nil {
		t.Fatalf("legacy v4 tool payload: %v", err)
	}
	if !toolDecoded.Migrated {
		t.Fatal("legacy v4 tool stack must migrate")
	}
	witnessedSingles := 0
	for slot := range core.DropsPerChunk {
		stack := toolDecoded.Chunk.Drop(slot).Stack
		if stack.Item == core.ItemStonePickaxe && stack.Count > 1 {
			t.Fatalf("legacy v4 tool stack must be split, slot %d count=%d", slot, stack.Count)
		}
		if stack == wantTool {
			witnessedSingles++
		}
	}
	if witnessedSingles != 2 {
		t.Fatalf("legacy v4 tool split want 2 full-durability singles, got %d", witnessedSingles)
	}
	for _, slot := range []int{0, 5} {
		drop := toolDecoded.Chunk.Drop(slot)
		if drop.Stack != wantTool || drop.BlockIndex != blockIndex ||
			drop.AgeTicks != 101 || drop.PickupDelayTicks != 9 {
			t.Fatalf("legacy v4 tool split slot %d = %+v", slot, drop)
		}
	}
	okCase := candidates[0]
	okInput := okCase.Assets[okCase.Spec.Input.Path]
	okDecoded, err := Decode(key, chunkFixtureRevision, okInput)
	if err != nil {
		t.Fatal(err)
	}
	stale := okDecoded
	stale.Chunk.SetBlock(0, 5, 0, core.StoneID)
	if chunkValueSHA256(chunkDecodedValueTree(stale)) == okCase.Expect.ValueSHA256 {
		t.Fatal("section mutation did not change digest")
	}
	staleDrop := okDecoded
	drop := staleDrop.Chunk.Drop(0)
	drop.AgeTicks++
	staleDrop.Chunk.SetDrop(0, drop)
	if chunkValueSHA256(chunkDecodedValueTree(staleDrop)) == okCase.Expect.ValueSHA256 {
		t.Fatal("drop slot mutation did not change digest")
	}
	if chunkExportSelection(t, root, candidates, chunkRoutes()) != "" {
		t.Fatal("export must not run when env unset")
	}
	contained := filepath.Join(root, "chunk-export-probe")
	if _, err := chunkOracleExportGeneratedAssets(root, contained, chunkProducerID, []chunkGeneratedAsset{
		{RelativePath: "probe.json", Data: []byte("{}\n")},
	}); err == nil || !strings.Contains(err.Error(), "live-path") {
		t.Fatalf("expected live-path rejection, got: %v", err)
	}
	handoffRoot := filepath.Join(t.TempDir(), "chunk-oracle-export")
	t.Setenv(chunkRuntimeOracleExportDirEnv, handoffRoot)
	child := chunkExportSelection(t, root, candidates, chunkRoutes())
	if child == "" {
		t.Fatal("pinned export directory did not publish a candidate")
	}
	selectionPath := filepath.Join(child, chunkSelectionManifest)
	if _, err := os.Stat(selectionPath); err != nil {
		t.Fatalf("selection manifest missing after export: %v", err)
	}
	for _, candidate := range candidates {
		for relative, want := range candidate.Assets {
			got, err := os.ReadFile(filepath.Join(child, filepath.FromSlash(relative)))
			if err != nil {
				t.Fatalf("read exported asset %s: %v", relative, err)
			}
			if !bytes.Equal(got, want) {
				t.Fatalf("exported asset %s bytes differ", relative)
			}
		}
	}
}

func TestChunkMigrationOracleLate(t *testing.T) {
	t.Setenv(chunkRuntimeOracleExportDirEnv, "")
	root := chunkRepoRoot(t)
	candidates := chunkLateCandidates(t)
	key := chunkFixtureKey()
	for _, candidate := range candidates {
		input := candidate.Assets[candidate.Spec.Input.Path]
		got, err := chunkRunDecode(candidate.Spec, input)
		if err != nil {
			t.Fatalf("decode %s: %v", candidate.Spec.ID, err)
		}
		if !chunkOutcomesEqual(got, candidate.Expect) {
			t.Fatalf("case %s produced %#v, want %#v", candidate.Spec.ID, got, candidate.Expect)
		}
	}
	v5, err := Decode(key, chunkFixtureRevision, chunkReadFixture(t, "chunk-v5.bin"))
	if err != nil {
		t.Fatal(err)
	}
	wantFurnace := furnaceFixtureChunk(t, key.Pos)
	if v5.Chunk.Hash() != wantFurnace.Hash() || v5.Chunk.DropsHash() != wantFurnace.DropsHash() {
		t.Fatal("v5 fixture blocks or drops mismatch")
	}
	for slot := range core.FurnacesPerChunk {
		if v5.Chunk.Furnace(slot) != wantFurnace.Furnace(slot) {
			t.Fatalf("v5 fixture furnace slot %d mismatch", slot)
		}
	}
	empty := world.NewChunk(key.Pos)
	for slot := range core.ChestsPerChunk {
		if v5.Chunk.Chest(slot) != empty.Chest(slot) {
			t.Fatalf("v5 fixture chest slot %d must stay empty", slot)
		}
	}
	wantDrops := dropFixtureChunk(t, key.Pos)
	for slot := range core.DropsPerChunk {
		gotDrop := v5.Chunk.Drop(slot)
		wantDrop := wantDrops.Drop(slot)
		if gotDrop != wantDrop {
			t.Fatalf("v5 fixture drop slot %d = %+v, want %+v", slot, gotDrop, wantDrop)
		}
	}
	v6, err := Decode(key, chunkFixtureRevision, chunkReadFixture(t, "chunk-v6.bin"))
	if err != nil {
		t.Fatal(err)
	}
	wantChest := chestFixtureChunk(t, key.Pos)
	if v6.Chunk.Hash() != wantChest.Hash() || v6.Chunk.DropsHash() != wantChest.DropsHash() {
		t.Fatal("v6 fixture blocks or drops mismatch")
	}
	for slot := range core.FurnacesPerChunk {
		if v6.Chunk.Furnace(slot) != wantChest.Furnace(slot) {
			t.Fatalf("v6 fixture furnace slot %d mismatch", slot)
		}
	}
	for slot := range core.ChestsPerChunk {
		if v6.Chunk.Chest(slot) != wantChest.Chest(slot) {
			t.Fatalf("v6 fixture chest slot %d = %+v, want %+v", slot, v6.Chunk.Chest(slot), wantChest.Chest(slot))
		}
	}
	v8, err := Decode(key, chunkFixtureRevision, chunkReadFixture(t, "chunk-v8.bin"))
	if err != nil {
		t.Fatal(err)
	}
	if countFluidCells(v8.Chunk) != 0 {
		t.Fatal("v8 fixture must not inject synthetic water")
	}
	if v8.Chunk.Hash() != wantChest.Hash() {
		t.Fatal("v8 fixture block registry mismatch")
	}
	v9, err := Decode(key, chunkFixtureRevision, chunkReadFixture(t, "chunk-v9.bin"))
	if err != nil {
		t.Fatal(err)
	}
	wantFluid := fluidFixtureChunk(t, key.Pos)
	if countFluidCells(v9.Chunk) != len(fluidBlockIDs) {
		t.Fatalf("v9 fixture fluid cells = %d, want %d", countFluidCells(v9.Chunk), len(fluidBlockIDs))
	}
	if v9.Chunk.Hash() != wantFluid.Hash() {
		t.Fatal("v9 fixture block registry mismatch")
	}
	if v9.Migrated {
		t.Fatal("v9 fixture decode must not mark migrated for current wire")
	}
	okCase := candidates[0]
	okDecoded, err := Decode(key, chunkFixtureRevision, okCase.Assets[okCase.Spec.Input.Path])
	if err != nil {
		t.Fatal(err)
	}
	stale := okDecoded
	stale.Chunk.SetBlock(0, 5, 0, core.StoneID)
	if chunkValueSHA256(chunkDecodedValueTree(stale)) == okCase.Expect.ValueSHA256 {
		t.Fatal("section mutation did not change digest")
	}
	staleDrop := okDecoded
	drop := staleDrop.Chunk.Drop(0)
	drop.AgeTicks++
	staleDrop.Chunk.SetDrop(0, drop)
	if chunkValueSHA256(chunkDecodedValueTree(staleDrop)) == okCase.Expect.ValueSHA256 {
		t.Fatal("drop slot mutation did not change digest")
	}
	if chunkExportSelection(t, root, candidates, chunkLateRoutes()) != "" {
		t.Fatal("export must not run when env unset")
	}
	handoffRoot := filepath.Join(t.TempDir(), "chunk-oracle-late-export")
	t.Setenv(chunkRuntimeOracleExportDirEnv, handoffRoot)
	child := chunkExportSelection(t, root, candidates, chunkLateRoutes())
	if child == "" {
		t.Fatal("export did not publish late candidates")
	}
	if _, err := os.Stat(filepath.Join(child, chunkSelectionManifest)); err != nil {
		t.Fatalf("selection manifest missing: %v", err)
	}
}

func TestChunkMigrationOracleAdversarial(t *testing.T) {
	t.Setenv(chunkRuntimeOracleExportDirEnv, "")
	root := chunkRepoRoot(t)
	candidates := chunkAdversarialCandidates(t)
	if len(candidates) != 13 {
		t.Fatalf("adversarial candidate count = %d, want 13", len(candidates))
	}
	for _, candidate := range candidates {
		input := candidate.Assets[candidate.Spec.Input.Path]
		got, err := chunkRunDecode(candidate.Spec, input)
		if err != nil {
			t.Fatalf("decode %s: %v", candidate.Spec.ID, err)
		}
		if !chunkOutcomesEqual(got, candidate.Expect) {
			t.Fatalf("case %s produced %#v, want %#v", candidate.Spec.ID, got, candidate.Expect)
		}
	}
	future := candidates[1]
	if future.Spec.ID != chunkFamily+"/9/decode/invalid-version-future" ||
		future.Expect.Category != "future_version" {
		t.Fatalf("invalid-version-future outcome %#v", future.Expect)
	}
	if chunkExportSelection(t, root, candidates, chunkAdversarialRoutes()) != "" {
		t.Fatal("export must not run when env unset")
	}
	handoffRoot := filepath.Join(t.TempDir(), "chunk-oracle-adversarial-export")
	t.Setenv(chunkRuntimeOracleExportDirEnv, handoffRoot)
	child := chunkExportSelection(t, root, candidates, chunkAdversarialRoutes())
	if child == "" {
		t.Fatal("export did not publish adversarial candidates")
	}
	if _, err := os.Stat(filepath.Join(child, chunkSelectionManifest)); err != nil {
		t.Fatalf("selection manifest missing: %v", err)
	}
}

func TestChunkMigrationOracleAdversarialExportToPinnedDirectory(t *testing.T) {
	root := chunkRepoRoot(t)
	exportRoot := chunkPinnedExportDirAdversarial
	producerChild := filepath.Join(exportRoot, filepath.FromSlash(chunkProducerID))
	if _, err := os.Lstat(producerChild); err == nil {
		t.Skip("pinned producer child already exists; reviewed export candidate preserved")
	} else if !os.IsNotExist(err) {
		t.Fatalf("stat pinned producer child: %v", err)
	}
	t.Setenv(chunkRuntimeOracleExportDirEnv, exportRoot)
	child := chunkExportSelection(t, root, chunkAdversarialCandidates(t), chunkAdversarialRoutes())
	if child == "" {
		t.Fatal("export root unset after explicit env")
	}
	if _, err := os.Stat(filepath.Join(child, chunkSelectionManifest)); err != nil {
		t.Fatalf("selection manifest missing: %v", err)
	}
}

func TestChunkMigrationOracleCross(t *testing.T) {
	framePath := strings.TrimSpace(os.Getenv(chunkRustFrameInputEnv))
	if framePath == "" {
		return
	}
	if strings.TrimSpace(os.Getenv(chunkRuntimeOracleExportDirEnv)) == "" {
		t.Fatal("RUST_STORAGE_FRAME_INPUT set without RUNTIME_ORACLE_EXPORT_DIR")
	}
	root := chunkRepoRoot(t)
	frame, err := os.ReadFile(framePath)
	if err != nil {
		t.Fatalf("read rust frame %s: %v", framePath, err)
	}
	decoded, err := Decode(chunkFixtureKey(), chunkFixtureRevision, frame)
	if err != nil {
		t.Fatalf("decode rust frame: %v", err)
	}
	if decoded.Schema != currentChunkSchema {
		t.Fatalf("decoded schema %d, want %d", decoded.Schema, currentChunkSchema)
	}
	candidate := chunkCrossCandidateFromFrame(t, frame)
	got, err := chunkRunDecode(candidate.Spec, frame)
	if err != nil {
		t.Fatalf("oracle decode %s: %v", candidate.Spec.ID, err)
	}
	if !chunkOutcomesEqual(got, candidate.Expect) {
		t.Fatalf("case %s produced %#v, want %#v", candidate.Spec.ID, got, candidate.Expect)
	}
	child := chunkExportSelection(t, root, []chunkCandidate{candidate}, chunkLateRoutes())
	if child == "" {
		t.Fatal("export did not publish cross-decode candidate")
	}
	if _, err := os.Stat(filepath.Join(child, chunkSelectionManifest)); err != nil {
		t.Fatalf("selection manifest missing: %v", err)
	}
}

func TestChunkMigrationOracleCrossExportToPinnedDirectory(t *testing.T) {
	framePath := strings.TrimSpace(os.Getenv(chunkRustFrameInputEnv))
	if framePath == "" {
		t.Skip("RUST_STORAGE_FRAME_INPUT unset")
	}
	frame, err := os.ReadFile(framePath)
	if err != nil {
		t.Fatalf("read reviewed frame: %v", err)
	}
	root := chunkRepoRoot(t)
	exportRoot := chunkPinnedExportDirCross
	producerChild := filepath.Join(exportRoot, filepath.FromSlash(chunkProducerID))
	if _, err := os.Lstat(producerChild); err == nil {
		t.Skip("pinned producer child already exists; reviewed export candidate preserved")
	} else if !os.IsNotExist(err) {
		t.Fatalf("stat pinned producer child: %v", err)
	}
	t.Setenv(chunkRuntimeOracleExportDirEnv, exportRoot)
	child := chunkExportSelection(t, root, []chunkCandidate{chunkCrossCandidateFromFrame(t, frame)}, chunkLateRoutes())
	if child == "" {
		t.Fatal("export root unset after explicit env")
	}
	if _, err := os.Stat(filepath.Join(child, chunkSelectionManifest)); err != nil {
		t.Fatalf("selection manifest missing: %v", err)
	}
}

func TestChunkMigrationOracleLateExportToPinnedDirectory(t *testing.T) {
	root := chunkRepoRoot(t)
	exportRoot := chunkPinnedExportDirLate
	producerChild := filepath.Join(exportRoot, filepath.FromSlash(chunkProducerID))
	if _, err := os.Lstat(producerChild); err == nil {
		t.Skip("pinned producer child already exists; reviewed export candidate preserved")
	} else if !os.IsNotExist(err) {
		t.Fatalf("stat pinned producer child: %v", err)
	}
	t.Setenv(chunkRuntimeOracleExportDirEnv, exportRoot)
	child := chunkExportSelection(t, root, chunkLateCandidates(t), chunkLateRoutes())
	if child == "" {
		t.Fatal("export root unset after explicit env")
	}
	if _, err := os.Stat(filepath.Join(child, chunkSelectionManifest)); err != nil {
		t.Fatalf("selection manifest missing: %v", err)
	}
}
