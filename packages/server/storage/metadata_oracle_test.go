package storage

import (
	"bytes"
	"crypto/sha256"
	"encoding/binary"
	"encoding/json"
	"errors"
	"fmt"
	"hash/crc32"
	"os"
	"path/filepath"
	"sort"
	"strconv"
	"strings"
	"testing"

	"github.com/channing771/mornlea/packages/shared/core"
)

const (
	metadataFamily          = "save.world-metadata"
	metadataVersion         = "6"
	metadataProducerID      = "storage/metadata"
	metadataCorpusRelDir    = "testdata/runtime-migration/cases/storage/world-metadata"
	metadataProducerTestRel = "packages/server/storage/metadata_oracle_test.go"
	metadataCodecSourceRel  = "packages/server/storage/metadata.go"
	metadataPinnedExportDir = "/tmp/runtime-oracle-metadata-2.4"
	metadataSelectionManifest = "selection.json"
	metadataRustConsumer    = "mornlea_storage"
	metadataRuntimeOracleExportDirEnv = "RUNTIME_ORACLE_EXPORT_DIR"

	metadataTotalLenV1 = 36
	metadataTotalLenV2 = 44
	metadataTotalLenV3 = 52
	metadataTotalLenV4 = 57
	metadataTotalLenV5 = 77
	metadataTotalLenV6 = 78
)

type metadataConsumerRoute struct {
	FamilyID  string `json:"family_id"`
	Version   string `json:"version"`
	Operation string `json:"operation"`
}

type metadataAssetRef struct {
	Path   string `json:"path"`
	SHA256 string `json:"sha256"`
}

type metadataCaseSpec struct {
	ID           string          `json:"id"`
	Family       string          `json:"family"`
	Version      string          `json:"version"`
	Operation    string          `json:"operation"`
	Arguments    json.RawMessage `json:"arguments,omitempty"`
	Input        metadataAssetRef `json:"input"`
	InputFormat  string          `json:"input_format"`
	Expected     metadataAssetRef `json:"expected"`
	Encoded      *metadataAssetRef `json:"encoded,omitempty"`
	Checkpoints  []string        `json:"checkpoints"`
	RustConsumer string          `json:"rust_consumer"`
}

type metadataSourceSpec struct {
	Path   string `json:"path"`
	SHA256 string `json:"sha256"`
}

type metadataSelection struct {
	ProducerID string                 `json:"producer_id"`
	Cases      []metadataCaseSpec     `json:"cases"`
	Sources    []metadataSourceSpec   `json:"sources"`
	Routes     []metadataConsumerRoute `json:"routes"`
}

type metadataSaveOutcome struct {
	Kind        string `json:"kind"`
	Category    string `json:"category,omitempty"`
	ValueSHA256 string `json:"value_sha256,omitempty"`
	Length      *int   `json:"length,omitempty"`
	Needed      *int   `json:"needed,omitempty"`
	Available   *int   `json:"available,omitempty"`
}

type metadataCandidate struct {
	Spec   metadataCaseSpec
	Assets map[string][]byte
	Expect metadataSaveOutcome
	Encoded []byte
}

type metadataGeneratedAsset struct {
	RelativePath string
	Data         []byte
}

var metadataValidProducerIDs = map[string]bool{"storage/metadata": true}

const (
	metadataTagNull     = 0x00
	metadataTagFalse    = 0x01
	metadataTagTrue     = 0x02
	metadataTagSigned   = 0x03
	metadataTagUnsigned = 0x04
	metadataTagF32      = 0x05
	metadataTagUTF8     = 0x06
	metadataTagBytes    = 0x07
	metadataTagArray    = 0x08
	metadataTagObject   = 0x09
)

type metadataValueNode struct {
	null     bool
	boolean  *bool
	signed   *int64
	unsigned *uint64
	f32bits  *uint32
	utf8     *string
	bytes    []byte
	array    []metadataValueNode
	object   map[string]metadataValueNode
}

func metadataValueNull() metadataValueNode { return metadataValueNode{null: true} }
func metadataValueBool(v bool) metadataValueNode { return metadataValueNode{boolean: &v} }
func metadataValueSigned(v int64) metadataValueNode { return metadataValueNode{signed: &v} }
func metadataValueUnsigned(v uint64) metadataValueNode { return metadataValueNode{unsigned: &v} }
func metadataValueObject(fields map[string]metadataValueNode) metadataValueNode {
	return metadataValueNode{object: fields}
}

func metadataEncodeValueNode(out *bytes.Buffer, node metadataValueNode) {
	switch {
	case node.null:
		out.WriteByte(metadataTagNull)
	case node.boolean != nil:
		if *node.boolean {
			out.WriteByte(metadataTagTrue)
		} else {
			out.WriteByte(metadataTagFalse)
		}
	case node.signed != nil:
		out.WriteByte(metadataTagSigned)
		_ = binary.Write(out, binary.LittleEndian, *node.signed)
	case node.unsigned != nil:
		out.WriteByte(metadataTagUnsigned)
		_ = binary.Write(out, binary.LittleEndian, *node.unsigned)
	case node.object != nil:
		out.WriteByte(metadataTagObject)
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
			metadataEncodeValueNode(out, node.object[key])
		}
	default:
		panic("empty metadata value node")
	}
}

func metadataValueSHA256(node metadataValueNode) string {
	var out bytes.Buffer
	metadataEncodeValueNode(&out, node)
	sum := sha256.Sum256(out.Bytes())
	return fmt.Sprintf("sha256:%x", sum)
}

func metadataChunkPosValue(pos core.ChunkPos) metadataValueNode {
	return metadataValueObject(map[string]metadataValueNode{
		"x": metadataValueSigned(int64(pos.X)),
		"z": metadataValueSigned(int64(pos.Z)),
	})
}

func metadataValueTree(metadata Metadata) metadataValueNode {
	return metadataValueObject(map[string]metadataValueNode{
		"format_version":           metadataValueUnsigned(uint64(metadata.FormatVersion)),
		"seed":                     metadataValueSigned(metadata.Seed),
		"spawn_dimension":          metadataValueSigned(int64(metadata.SpawnDimension)),
		"spawn_anchor":             metadataChunkPosValue(metadata.SpawnAnchor),
		"world_time_ticks":         metadataValueUnsigned(metadata.WorldTimeTicks),
		"day_phase_offset":         metadataValueUnsigned(metadata.DayPhaseOffset),
		"weather_kind":             metadataValueUnsigned(uint64(metadata.WeatherKind)),
		"weather_ticks_remaining":  metadataValueUnsigned(uint64(metadata.WeatherTicksRemaining)),
		"depths_spawn_anchor":      metadataChunkPosValue(metadata.DepthsSpawnAnchor),
		"depths_seed_salt":         metadataValueUnsigned(metadata.DepthsSeedSalt),
		"difficulty":               metadataValueUnsigned(uint64(metadata.Difficulty)),
	})
}

func metadataArgumentsJSON(capacity *uint32) json.RawMessage {
	if capacity == nil {
		return json.RawMessage(`{}`)
	}
	raw, err := json.Marshal(map[string]uint32{"capacity": *capacity})
	if err != nil {
		panic(err)
	}
	return raw
}

func metadataSchemaVersion(encoded []byte) (uint32, error) {
	if len(encoded) < 8 {
		return 0, fmt.Errorf("input shorter than schema header")
	}
	return binary.LittleEndian.Uint32(encoded[4:8]), nil
}

func metadataStorageErrorCategory(err error) (string, bool) {
	switch {
	case errors.Is(err, ErrCorrupt):
		return "corrupt", true
	case errors.Is(err, ErrFutureVersion):
		return "future_version", true
	default:
		return "", false
	}
}

func metadataRunDecode(spec metadataCaseSpec, input []byte) (metadataSaveOutcome, []byte, error) {
	schema, err := metadataSchemaVersion(input)
	if err != nil {
		return metadataSaveOutcome{}, nil, err
	}
	wantVersion, err := strconv.ParseUint(spec.Version, 10, 32)
	if err != nil {
		return metadataSaveOutcome{}, nil, fmt.Errorf("invalid case version %q", spec.Version)
	}
	decoded, err := decodeMetadata(input)
	if err != nil {
		category, ok := metadataStorageErrorCategory(err)
		if !ok {
			return metadataSaveOutcome{}, nil, fmt.Errorf("unclassified rejection: %w", err)
		}
		return metadataSaveOutcome{Kind: "error", Category: category}, nil, nil
	}
	if uint64(schema) != wantVersion {
		return metadataSaveOutcome{}, nil, fmt.Errorf("accepted input schema %d, want %d", schema, wantVersion)
	}
	digest := metadataValueSHA256(metadataValueTree(decoded))
	return metadataSaveOutcome{Kind: "ok", Category: "save", ValueSHA256: digest}, nil, nil
}

func metadataRunEncode(spec metadataCaseSpec, input []byte) (metadataSaveOutcome, []byte, error) {
	var args struct {
		Capacity *uint32 `json:"capacity"`
	}
	if len(spec.Arguments) > 0 {
		if err := json.Unmarshal(spec.Arguments, &args); err != nil {
			return metadataSaveOutcome{}, nil, err
		}
	}
	decoded, err := decodeMetadata(input)
	if err != nil {
		category, ok := metadataStorageErrorCategory(err)
		if !ok {
			return metadataSaveOutcome{}, nil, fmt.Errorf("unclassified decode rejection: %w", err)
		}
		return metadataSaveOutcome{Kind: "error", Category: category}, nil, nil
	}
	digest := metadataValueSHA256(metadataValueTree(decoded))
	encoded, err := encodeMetadata(decoded)
	if err != nil {
		category, ok := metadataStorageErrorCategory(err)
		if !ok {
			return metadataSaveOutcome{}, nil, fmt.Errorf("unclassified encode rejection: %w", err)
		}
		return metadataSaveOutcome{Kind: "error", Category: category}, nil, nil
	}
	if args.Capacity != nil {
		if int(*args.Capacity) < len(encoded) {
			needed, available := len(encoded), int(*args.Capacity)
			return metadataSaveOutcome{
				Kind: "error", Category: "output_too_small",
				Needed: &needed, Available: &available,
			}, nil, nil
		}
	}
	round, err := decodeMetadata(encoded)
	if err != nil {
		return metadataSaveOutcome{}, nil, err
	}
	if round != decoded {
		return metadataSaveOutcome{}, nil, fmt.Errorf("encode round-trip drift")
	}
	length := len(encoded)
	return metadataSaveOutcome{
		Kind: "ok", Category: "save", ValueSHA256: digest, Length: &length,
	}, encoded, nil
}

func metadataDigestOf(data []byte) string {
	sum := sha256.Sum256(data)
	return fmt.Sprintf("sha256:%x", sum)
}

func metadataHashFile(path string) (string, error) {
	data, err := os.ReadFile(path)
	if err != nil {
		return "", err
	}
	return metadataDigestOf(data), nil
}

func metadataMarshalOutcome(outcome metadataSaveOutcome) ([]byte, error) {
	return json.MarshalIndent(outcome, "", "  ")
}

func metadataSpawnAnchor() core.ChunkPos { return core.ChunkPos{X: 5, Z: -6} }

func metadataLegacyV1Bytes() []byte {
	anchor := metadataSpawnAnchor()
	return encodeLegacyMetadataV1(Metadata{
		FormatVersion:  legacyMetadataVersion,
		Seed:           123_456_789,
		SpawnDimension: core.Overworld,
		SpawnAnchor:    anchor,
	})
}

func metadataLegacyV2Bytes() []byte {
	anchor := metadataSpawnAnchor()
	return encodeLegacyMetadataV2(Metadata{
		FormatVersion:  legacyMetadataV2Version,
		Seed:           123_456_789,
		SpawnDimension: core.Overworld,
		SpawnAnchor:    anchor,
		WorldTimeTicks: 123_456,
	})
}

func metadataLegacyV3Bytes() []byte {
	anchor := metadataSpawnAnchor()
	return encodeLegacyMetadataV3(Metadata{
		FormatVersion:  legacyMetadataV3Version,
		Seed:           123_456_789,
		SpawnDimension: core.Overworld,
		SpawnAnchor:    anchor,
		WorldTimeTicks: 123_456,
		DayPhaseOffset: 678,
	})
}

func metadataLegacyV4Bytes() []byte {
	anchor := metadataSpawnAnchor()
	return encodeLegacyMetadataV4(Metadata{
		FormatVersion:         legacyMetadataV4Version,
		Seed:                  123_456_789,
		SpawnDimension:        core.Overworld,
		SpawnAnchor:           anchor,
		WorldTimeTicks:        123_456,
		DayPhaseOffset:        678,
		WeatherKind:           core.WeatherRain,
		WeatherTicksRemaining: 5000,
	})
}

func metadataLegacyV5Bytes() []byte {
	anchor := metadataSpawnAnchor()
	return encodeLegacyMetadataV5(Metadata{
		FormatVersion:         legacyMetadataV5Version,
		Seed:                  123_456_789,
		SpawnDimension:        core.Overworld,
		SpawnAnchor:           anchor,
		WorldTimeTicks:        123_456,
		DayPhaseOffset:        678,
		WeatherKind:           core.WeatherRain,
		WeatherTicksRemaining: 5000,
		DepthsSpawnAnchor:     anchor,
		DepthsSeedSalt:        depthsSeedSaltDefault,
	})
}

func metadataV6BoundaryMetadata() Metadata {
	anchor := core.ChunkPos{X: 7, Z: -11}
	return Metadata{
		FormatVersion:         currentMetadataVersion,
		Seed:                  -42,
		SpawnDimension:        core.DimensionID(-3),
		SpawnAnchor:           anchor,
		WorldTimeTicks:        0x0102030405060708,
		DayPhaseOffset:        ^uint64(0),
		WeatherKind:           core.WeatherKind(7),
		WeatherTicksRemaining: 5000,
		DepthsSpawnAnchor:     core.ChunkPos{X: -5, Z: 9},
		DepthsSeedSalt:        0x0123456789ABCDEF,
		Difficulty:            core.DifficultyNormal,
	}
}

func metadataV6Weather255Bytes(t *testing.T) []byte {
	t.Helper()
	base := metadataV6BoundaryMetadata()
	base.WeatherKind = core.WeatherKind(255)
	return mustEncodeMetadataForTest(t, base)
}

func metadataWireWithSchema(src []byte, version uint32) []byte {
	out := bytes.Clone(src)
	binary.LittleEndian.PutUint32(out[4:8], version)
	return out
}

func metadataCorruptCRC(src []byte) []byte {
	out := bytes.Clone(src)
	out[len(out)-5] ^= 0xff
	return out
}

func metadataWrongHeader(src []byte) []byte {
	out := bytes.Clone(src)
	out[0] ^= 0xff
	return out
}

func metadataWrongDimensionCountV5() []byte {
	base := metadataLegacyV5Bytes()
	// dimension count sits at payload offset 49 from file start: header 12 + seed 8 + dim 4 + anchor 8 + time 8 + phase 8 + weather 5 = 53? compute from v5 layout
	// After v5 header (12), payload: seed(8)+spawn_dim(4)+anchor(8)+time(8)+phase(8)+weather_kind(1)+weather_rem(4)=41
	// dimension count begins at header(12) + payload offset 41.
	out := bytes.Clone(base)
	binary.LittleEndian.PutUint32(out[53:57], 3)
	checksumOffset := len(out) - 4
	out = out[:checksumOffset]
	out = binary.LittleEndian.AppendUint32(out, crc32.Checksum(out, metadataCRCTable))
	return out
}

func metadataInvalidDifficultyV6(t *testing.T) []byte {
	t.Helper()
	meta := metadataV6BoundaryMetadata()
	encoded := mustEncodeMetadataForTest(t, meta)
	out := bytes.Clone(encoded)
	out[len(out)-5] = 3
	checksumOffset := len(out) - 4
	out = out[:checksumOffset]
	out = binary.LittleEndian.AppendUint32(out, crc32.Checksum(out, metadataCRCTable))
	return out
}

func metadataBuildCandidate(t *testing.T, id, operation, caseVersion string, input []byte, args json.RawMessage, wantEncoded []byte) metadataCandidate {
	t.Helper()
	spec := metadataCaseSpec{
		ID: id, Family: metadataFamily, Version: caseVersion, Operation: operation,
		Arguments: args, InputFormat: "binary", Checkpoints: []string{"0"},
		RustConsumer: metadataRustConsumer,
	}
	var outcome metadataSaveOutcome
	var produced []byte
	var err error
	switch operation {
	case "decode":
		outcome, produced, err = metadataRunDecode(spec, input)
	case "encode":
		outcome, produced, err = metadataRunEncode(spec, input)
	default:
		t.Fatalf("unsupported operation %q", operation)
	}
	if err != nil {
		t.Fatalf("execute %s: %v", id, err)
	}
	expectedBytes, err := metadataMarshalOutcome(outcome)
	if err != nil {
		t.Fatalf("marshal expected %s: %v", id, err)
	}
	encoded := wantEncoded
	if operation == "encode" && outcome.Kind == "ok" {
		if len(produced) == 0 {
			t.Fatalf("encode case %s produced no bytes", id)
		}
		encoded = produced
	}
	stem := strings.ReplaceAll(id, "/", "_")
	inputRel := filepath.ToSlash(filepath.Join(metadataCorpusRelDir, stem+".input.bin"))
	expectedRel := filepath.ToSlash(filepath.Join(metadataCorpusRelDir, stem+".expected.json"))
	assets := map[string][]byte{inputRel: input, expectedRel: expectedBytes}
	spec.Input = metadataAssetRef{Path: inputRel, SHA256: metadataDigestOf(input)}
	spec.Expected = metadataAssetRef{Path: expectedRel, SHA256: metadataDigestOf(expectedBytes)}
	if operation == "encode" && outcome.Kind == "ok" {
		encodedRel := filepath.ToSlash(filepath.Join(metadataCorpusRelDir, stem+".encoded.bin"))
		assets[encodedRel] = encoded
		spec.Encoded = &metadataAssetRef{Path: encodedRel, SHA256: metadataDigestOf(encoded)}
	}
	return metadataCandidate{Spec: spec, Assets: assets, Expect: outcome, Encoded: encoded}
}

func metadataRoutes() []metadataConsumerRoute {
	routes := []metadataConsumerRoute{{FamilyID: metadataFamily, Version: metadataVersion, Operation: "encode"}}
	for version := 6; version >= 1; version-- {
		routes = append(routes, metadataConsumerRoute{
			FamilyID: metadataFamily, Version: strconv.Itoa(version), Operation: "decode",
		})
	}
	return routes
}

func metadataCandidates(t *testing.T) []metadataCandidate {
	t.Helper()
	args := metadataArgumentsJSON(nil)
	v6 := mustEncodeMetadataForTest(t, metadataV6BoundaryMetadata())
	return []metadataCandidate{
		metadataBuildCandidate(t, metadataFamily+"/1/decode/v1-canonical", "decode", "1", metadataLegacyV1Bytes(), args, nil),
		metadataBuildCandidate(t, metadataFamily+"/2/decode/v2-canonical", "decode", "2", metadataLegacyV2Bytes(), args, nil),
		metadataBuildCandidate(t, metadataFamily+"/3/decode/v3-canonical", "decode", "3", metadataLegacyV3Bytes(), args, nil),
		metadataBuildCandidate(t, metadataFamily+"/4/decode/v4-canonical", "decode", "4", metadataLegacyV4Bytes(), args, nil),
		metadataBuildCandidate(t, metadataFamily+"/5/decode/v5-canonical", "decode", "5", metadataLegacyV5Bytes(), args, nil),
		metadataBuildCandidate(t, metadataFamily+"/6/decode/v6-weather-255", "decode", "6", metadataV6Weather255Bytes(t), args, nil),
		metadataBuildCandidate(t, metadataFamily+"/6/encode/v6-boundary", "encode", metadataVersion, v6, args, nil),
		metadataBuildCandidate(t, metadataFamily+"/6/decode/invalid-version-zero", "decode", metadataVersion, metadataWireWithSchema(v6, 0), args, nil),
		metadataBuildCandidate(t, metadataFamily+"/6/decode/invalid-version-future", "decode", metadataVersion, metadataWireWithSchema(v6, currentMetadataVersion+1), args, nil),
		metadataBuildCandidate(t, metadataFamily+"/6/decode/truncated-record", "decode", metadataVersion, v6[:len(v6)-1], args, nil),
		metadataBuildCandidate(t, metadataFamily+"/6/decode/trailing-byte", "decode", metadataVersion, append(bytes.Clone(v6), 0), args, nil),
		metadataBuildCandidate(t, metadataFamily+"/6/decode/corrupt-crc", "decode", metadataVersion, metadataCorruptCRC(v6), args, nil),
		metadataBuildCandidate(t, metadataFamily+"/6/decode/wrong-header", "decode", metadataVersion, metadataWrongHeader(v6), args, nil),
		metadataBuildCandidate(t, metadataFamily+"/5/decode/wrong-dimension-count", "decode", "5", metadataWrongDimensionCountV5(), args, nil),
		metadataBuildCandidate(t, metadataFamily+"/6/decode/invalid-difficulty-3", "decode", metadataVersion, metadataInvalidDifficultyV6(t), args, nil),
	}
}

func buildMetadataOracleSelection(t *testing.T, root string, candidates []metadataCandidate) metadataSelection {
	t.Helper()
	cases := make([]metadataCaseSpec, 0, len(candidates))
	for _, candidate := range candidates {
		cases = append(cases, candidate.Spec)
	}
	sources := []metadataSourceSpec{{Path: metadataCodecSourceRel}, {Path: metadataProducerTestRel}}
	for index := range sources {
		hash, err := metadataHashFile(filepath.Join(root, filepath.FromSlash(sources[index].Path)))
		if err != nil {
			t.Fatalf("hash source %s: %v", sources[index].Path, err)
		}
		sources[index].SHA256 = hash
	}
	sort.Slice(cases, func(i, j int) bool { return cases[i].ID < cases[j].ID })
	sort.Slice(sources, func(i, j int) bool { return sources[i].Path < sources[j].Path })
	return metadataSelection{
		ProducerID: metadataProducerID,
		Cases:      cases,
		Sources:    sources,
		Routes:     metadataRoutes(),
	}
}

func metadataExportSelection(t *testing.T, root string, candidates []metadataCandidate) string {
	t.Helper()
	if strings.TrimSpace(os.Getenv(metadataRuntimeOracleExportDirEnv)) == "" {
		return ""
	}
	selection := buildMetadataOracleSelection(t, root, candidates)
	manifestBytes, err := json.Marshal(selection)
	if err != nil {
		t.Fatalf("marshal selection: %v", err)
	}
	var assets []metadataGeneratedAsset
	assets = append(assets, metadataGeneratedAsset{RelativePath: metadataSelectionManifest, Data: manifestBytes})
	seen := make(map[string]struct{})
	for _, candidate := range candidates {
		for relative, data := range candidate.Assets {
			if _, ok := seen[relative]; ok {
				continue
			}
			seen[relative] = struct{}{}
			assets = append(assets, metadataGeneratedAsset{RelativePath: relative, Data: data})
		}
	}
	exportRoot := strings.TrimSpace(os.Getenv(metadataRuntimeOracleExportDirEnv))
	dir, err := metadataExportGeneratedAssets(root, exportRoot, metadataProducerID, assets)
	if err != nil {
		t.Fatalf("export metadata selection: %v", err)
	}
	return dir
}

func metadataRepoRoot(t *testing.T) string {
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

func metadataOutcomesEqual(left, right metadataSaveOutcome) bool {
	return left.Kind == right.Kind && left.Category == right.Category && left.ValueSHA256 == right.ValueSHA256
}

func TestMetadataOracle(t *testing.T) {
	t.Setenv(metadataRuntimeOracleExportDirEnv, "")
	root := metadataRepoRoot(t)
	candidates := metadataCandidates(t)
	lengths := map[string]int{
		metadataFamily + "/1/decode/v1-canonical": metadataTotalLenV1,
		metadataFamily + "/2/decode/v2-canonical": metadataTotalLenV2,
		metadataFamily + "/3/decode/v3-canonical": metadataTotalLenV3,
		metadataFamily + "/4/decode/v4-canonical": metadataTotalLenV4,
		metadataFamily + "/5/decode/v5-canonical": metadataTotalLenV5,
		metadataFamily + "/6/encode/v6-boundary":    metadataTotalLenV6,
		metadataFamily + "/6/decode/v6-weather-255": metadataTotalLenV6,
	}
	for _, candidate := range candidates {
		input := candidate.Assets[candidate.Spec.Input.Path]
		if wantLen, ok := lengths[candidate.Spec.ID]; ok && len(input) != wantLen {
			t.Fatalf("case %s input length = %d, want %d", candidate.Spec.ID, len(input), wantLen)
		}
		var got metadataSaveOutcome
		var err error
		switch candidate.Spec.Operation {
		case "decode":
			got, _, err = metadataRunDecode(candidate.Spec, input)
			if err != nil {
				t.Fatalf("decode %s: %v", candidate.Spec.ID, err)
			}
		case "encode":
			got, _, err = metadataRunEncode(candidate.Spec, input)
			if err != nil {
				t.Fatalf("encode %s: %v", candidate.Spec.ID, err)
			}
		default:
			t.Fatalf("unknown operation %q", candidate.Spec.Operation)
		}
		if !metadataOutcomesEqual(got, candidate.Expect) {
			t.Fatalf("case %s produced %#v, want %#v", candidate.Spec.ID, got, candidate.Expect)
		}
	}
	// digest sensitivity
	v5 := metadataLegacyV5Bytes()
	decoded, err := decodeMetadata(v5)
	if err != nil {
		t.Fatal(err)
	}
	staleAnchor := decoded
	staleAnchor.DepthsSpawnAnchor.X++
	if metadataValueSHA256(metadataValueTree(staleAnchor)) == candidateDigest(t, v5, "decode", "5") {
		t.Fatal("anchor mutation did not change digest")
	}
	staleSalt := decoded
	staleSalt.DepthsSeedSalt++
	if metadataValueSHA256(metadataValueTree(staleSalt)) == candidateDigest(t, v5, "decode", "5") {
		t.Fatal("salt mutation did not change digest")
	}
	staleWeather := decoded
	staleWeather.WeatherKind = core.WeatherKind(9)
	if metadataValueSHA256(metadataValueTree(staleWeather)) == candidateDigest(t, metadataLegacyV4Bytes(), "decode", "4") {
		t.Fatal("weather mutation did not change digest")
	}
	if metadataExportSelection(t, root, candidates) != "" {
		t.Fatal("export must not run when env unset")
	}
	contained := filepath.Join(root, "metadata-export-probe")
	if _, err := metadataExportGeneratedAssets(root, contained, metadataProducerID, []metadataGeneratedAsset{
		{RelativePath: "probe.json", Data: []byte("{}\n")},
	}); err == nil || !strings.Contains(err.Error(), "live-path") {
		t.Fatalf("expected live-path rejection, got: %v", err)
	}
	handoffRoot := filepath.Join(metadataPinnedExportDir, "handoff")
	t.Setenv(metadataRuntimeOracleExportDirEnv, handoffRoot)
	child := metadataExportSelection(t, root, candidates)
	if child == "" {
		t.Fatal("pinned export directory did not publish a candidate")
	}
	if _, err := os.Stat(filepath.Join(child, metadataSelectionManifest)); err != nil {
		t.Fatalf("selection manifest missing after export: %v", err)
	}
}

func candidateDigest(t *testing.T, input []byte, operation, version string) string {
	t.Helper()
	spec := metadataCaseSpec{Version: version, Operation: operation}
	out, _, err := metadataRunDecode(spec, input)
	if err != nil {
		t.Fatal(err)
	}
	return out.ValueSHA256
}

func metadataValidateProducerID(producerID string) ([]string, error) {
	if strings.TrimSpace(producerID) == "" || strings.Contains(producerID, "\\") || filepath.IsAbs(producerID) {
		return nil, fmt.Errorf("metadata oracle: producer ID must be a clean relative slash path: %q", producerID)
	}
	parts := strings.Split(producerID, "/")
	for _, part := range parts {
		if part == "" || part == "." || part == ".." {
			return nil, fmt.Errorf("metadata oracle: producer ID must be a clean relative slash path: %q", producerID)
		}
	}
	if filepath.ToSlash(filepath.Clean(filepath.FromSlash(producerID))) != producerID {
		return nil, fmt.Errorf("metadata oracle: producer ID must be a clean relative slash path: %q", producerID)
	}
	return parts, nil
}

func metadataValidateGeneratedAssets(assets []metadataGeneratedAsset) error {
	seen := make(map[string]struct{}, len(assets))
	paths := make([]string, 0, len(assets))
	for _, asset := range assets {
		if strings.TrimSpace(asset.RelativePath) == "" {
			return fmt.Errorf("metadata oracle: empty asset relative path")
		}
		if strings.Contains(asset.RelativePath, "\\") {
			return fmt.Errorf("metadata oracle: backslash rejected in relative path: %s", asset.RelativePath)
		}
		if filepath.IsAbs(asset.RelativePath) || strings.HasPrefix(asset.RelativePath, "/") {
			return fmt.Errorf("metadata oracle: absolute path rejected: %s", asset.RelativePath)
		}
		parts := strings.Split(asset.RelativePath, "/")
		for _, part := range parts {
			if part == "." || part == ".." {
				return fmt.Errorf("metadata oracle: relative path contains ./..: %s", asset.RelativePath)
			}
			if part == "" {
				return fmt.Errorf("metadata oracle: asset relative path must be clean: %s", asset.RelativePath)
			}
		}
		cleaned := filepath.Clean(filepath.FromSlash(asset.RelativePath))
		if cleaned == "." || cleaned == ".." || strings.HasPrefix(cleaned, ".."+string(filepath.Separator)) {
			return fmt.Errorf("metadata oracle: path escapes producer directory: %s", asset.RelativePath)
		}
		normalized := filepath.ToSlash(cleaned)
		if normalized != asset.RelativePath {
			return fmt.Errorf("metadata oracle: asset relative path must be clean: %s", asset.RelativePath)
		}
		if _, duplicate := seen[normalized]; duplicate {
			return fmt.Errorf("metadata oracle: duplicate asset relative path: %s", asset.RelativePath)
		}
		seen[normalized] = struct{}{}
		paths = append(paths, normalized)
	}
	for _, relative := range paths {
		parts := strings.Split(relative, "/")
		for index := 1; index < len(parts); index++ {
			parent := strings.Join(parts[:index], "/")
			if _, collision := seen[parent]; collision {
				return fmt.Errorf("metadata oracle: asset path conflicts with another asset: %s", relative)
			}
		}
	}
	return nil
}

func metadataIsLivePath(root, target string) (bool, error) {
	if strings.TrimSpace(target) == "" {
		return false, nil
	}
	absRoot, err := filepath.Abs(root)
	if err != nil {
		return false, fmt.Errorf("metadata oracle: resolve repository root: %w", err)
	}
	absTarget, err := filepath.Abs(target)
	if err != nil {
		return false, fmt.Errorf("metadata oracle: resolve path %s: %w", target, err)
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
		return false, fmt.Errorf("metadata oracle: compare path %s: %w", target, err)
	}
	if rel == "." {
		return true, nil
	}
	if rel == ".." {
		return false, nil
	}
	return !strings.HasPrefix(rel, ".."+string(filepath.Separator)), nil
}

func metadataCreateExportRoot(absExport, existing string, missing []string) error {
	info, err := os.Lstat(existing)
	if err != nil {
		return fmt.Errorf("metadata oracle: stat export ancestor %s: %w", existing, err)
	}
	if !info.IsDir() {
		return fmt.Errorf("metadata oracle: export-prefix non-directory rejected: %s", existing)
	}
	current := existing
	for _, component := range missing {
		current = filepath.Join(current, component)
		info, err := os.Lstat(current)
		switch {
		case err == nil && info.Mode()&os.ModeSymlink != 0:
			return fmt.Errorf("metadata oracle: export-prefix symlink rejected: %s", current)
		case err == nil && !info.IsDir():
			return fmt.Errorf("metadata oracle: export-prefix non-directory rejected: %s", current)
		case err == nil:
			continue
		case !os.IsNotExist(err):
			return fmt.Errorf("metadata oracle: stat export path %s: %w", current, err)
		}
		if err := os.Mkdir(current, 0o755); err != nil {
			return fmt.Errorf("metadata oracle: create export directory %s: %w", current, err)
		}
	}
	if current != absExport {
		return fmt.Errorf("metadata oracle: export root walk ended at %s, want %s", current, absExport)
	}
	return nil
}

func metadataCreateProducerChild(exportRoot string, components []string) (string, error) {
	current := exportRoot
	for index, component := range components {
		current = filepath.Join(current, component)
		final := index == len(components)-1
		info, err := os.Lstat(current)
		switch {
		case err == nil && info.Mode()&os.ModeSymlink != 0:
			return "", fmt.Errorf("metadata oracle: producer-prefix symlink rejected: %s", current)
		case err == nil && final:
			return "", fmt.Errorf("metadata oracle: producer child already exists: %s", current)
		case err == nil && !info.IsDir():
			return "", fmt.Errorf("metadata oracle: producer-prefix non-directory rejected: %s", current)
		case err == nil:
			continue
		case !os.IsNotExist(err):
			return "", fmt.Errorf("metadata oracle: stat producer component %s: %w", current, err)
		}
		if err := os.Mkdir(current, 0o755); err != nil {
			return "", fmt.Errorf("metadata oracle: create producer directory %s: %w", current, err)
		}
	}
	return current, nil
}

func metadataCreateAssetParents(producerChild string, assets []metadataGeneratedAsset) error {
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
				return fmt.Errorf("metadata oracle: asset-prefix symlink rejected: %s", current)
			case err == nil && !info.IsDir():
				return fmt.Errorf("metadata oracle: asset-prefix non-directory rejected: %s", current)
			case err == nil:
				continue
			case !os.IsNotExist(err):
				return fmt.Errorf("metadata oracle: stat asset directory %s: %w", current, err)
			}
			if err := os.Mkdir(current, 0o755); err != nil {
				return fmt.Errorf("metadata oracle: create asset directory %s: %w", current, err)
			}
		}
	}
	return nil
}

func metadataRecheckExportContainment(repoRoot, exportRoot, producerChild string) error {
	resolvedExport, err := filepath.EvalSymlinks(exportRoot)
	if err != nil {
		return fmt.Errorf("metadata oracle: resolve created export root %s: %w", exportRoot, err)
	}
	resolvedProducer, err := filepath.EvalSymlinks(producerChild)
	if err != nil {
		return fmt.Errorf("metadata oracle: resolve created producer child %s: %w", producerChild, err)
	}
	rel, err := filepath.Rel(resolvedExport, resolvedProducer)
	if err != nil || rel == ".." || strings.HasPrefix(rel, ".."+string(filepath.Separator)) {
		return fmt.Errorf("metadata oracle: producer child escaped export root: %s", producerChild)
	}
	live, err := metadataIsLivePath(repoRoot, resolvedProducer)
	if err != nil {
		return err
	}
	if live {
		return fmt.Errorf("metadata oracle: live-path write rejected: producer child %s is inside repository", producerChild)
	}
	return nil
}

func metadataExportGeneratedAssets(repoRoot, exportRoot, producerID string, assets []metadataGeneratedAsset) (string, error) {
	producerComponents, err := metadataValidateProducerID(producerID)
	if err != nil {
		return "", err
	}
	if !metadataValidProducerIDs[producerID] {
		return "", fmt.Errorf("metadata oracle: unrecognized producer ID: %q", producerID)
	}
	if strings.TrimSpace(exportRoot) == "" {
		return "", fmt.Errorf("metadata oracle: export root cannot be empty")
	}
	if err := metadataValidateGeneratedAssets(assets); err != nil {
		return "", err
	}
	absRoot, err := filepath.Abs(repoRoot)
	if err != nil {
		return "", fmt.Errorf("metadata oracle: resolve repository root %s: %w", repoRoot, err)
	}
	if resolved, err := filepath.EvalSymlinks(absRoot); err == nil {
		absRoot = resolved
	}
	absExport, err := filepath.Abs(exportRoot)
	if err != nil {
		return "", fmt.Errorf("metadata oracle: resolve export root %s: %w", exportRoot, err)
	}
	var missing []string
	existing := absExport
	for {
		_, statErr := os.Lstat(existing)
		if statErr == nil {
			break
		}
		if !os.IsNotExist(statErr) {
			return "", fmt.Errorf("metadata oracle: stat export root %s: %w", existing, statErr)
		}
		missing = append(missing, filepath.Base(existing))
		parent := filepath.Dir(existing)
		if parent == existing {
			return "", fmt.Errorf("metadata oracle: export root %s has no existing ancestor", exportRoot)
		}
		existing = parent
	}
	for i, j := 0, len(missing)-1; i < j; i, j = i+1, j-1 {
		missing[i], missing[j] = missing[j], missing[i]
	}
	resolvedExisting, err := filepath.EvalSymlinks(existing)
	if err != nil {
		return "", fmt.Errorf("metadata oracle: resolve export ancestor %s: %w", existing, err)
	}
	resolvedExport := filepath.Join(append([]string{resolvedExisting}, missing...)...)
	live, err := metadataIsLivePath(absRoot, resolvedExport)
	if err != nil {
		return "", err
	}
	if live {
		return "", fmt.Errorf("metadata oracle: live-path write rejected: export root %s is inside repository", exportRoot)
	}
	liveExisting, err := metadataIsLivePath(absRoot, existing)
	if err != nil {
		return "", err
	}
	if liveExisting {
		return "", fmt.Errorf("metadata oracle: live-path write rejected: export ancestor %s is inside repository", existing)
	}
	if err := metadataCreateExportRoot(absExport, existing, missing); err != nil {
		return "", err
	}
	producerChild, err := metadataCreateProducerChild(absExport, producerComponents)
	if err != nil {
		return "", err
	}
	if err := metadataCreateAssetParents(producerChild, assets); err != nil {
		return "", err
	}
	if err := metadataRecheckExportContainment(absRoot, absExport, producerChild); err != nil {
		return "", err
	}
	for _, asset := range assets {
		target := filepath.Join(producerChild, filepath.FromSlash(asset.RelativePath))
		f, err := os.OpenFile(target, os.O_WRONLY|os.O_CREATE|os.O_EXCL, 0o644)
		if err != nil {
			return "", fmt.Errorf("metadata oracle: create exclusive asset %s: %w", asset.RelativePath, err)
		}
		if _, err := f.Write(asset.Data); err != nil {
			f.Close()
			return "", fmt.Errorf("metadata oracle: write asset %s: %w", asset.RelativePath, err)
		}
		if err := f.Close(); err != nil {
			return "", fmt.Errorf("metadata oracle: close asset %s: %w", asset.RelativePath, err)
		}
	}
	return producerChild, nil
}
