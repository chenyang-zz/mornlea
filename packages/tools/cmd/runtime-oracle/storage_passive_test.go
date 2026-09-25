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
	"slices"
	"sort"
	"strconv"
	"strings"
	"testing"

	"github.com/channing771/mornlea/packages/server/storage/passive"
	"github.com/channing771/mornlea/packages/server/storage/storagedef"
	"github.com/channing771/mornlea/packages/shared/core"
)

const (
	passiveFamily          = "save.passive"
	passiveVersionV1       = "1"
	passiveProducerID      = "runtime-oracle/storage-passive"
	passiveCorpusRelDir    = "testdata/runtime-migration/cases/storage/passive"
	passiveProducerTestRel = "packages/tools/cmd/runtime-oracle/storage_passive_test.go"
	passiveCodecSourceRel  = "packages/server/storage/passive/passive_codec.go"
	passiveExportDir       = "/tmp/runtime-oracle-passive-3.4"

	passiveHeaderLength = 32
	passiveRecordLength = 72

	passiveWireID        = 0
	passiveWireDimension = 8
	passiveWirePosition  = 12
	passiveWireVelocity  = 24
	passiveWireOnGround  = 36
	passiveWireYaw       = 37
	passiveWireHealth    = 41
	passiveWireReserved  = 42

	passiveDecodeV1FixtureID            = passiveFamily + "/" + passiveVersionV1 + "/decode/v1-fixture"
	passiveDecodeEmptyID                = passiveFamily + "/" + passiveVersionV1 + "/decode/empty"
	passiveDecodeMaxRecordsID           = passiveFamily + "/" + passiveVersionV1 + "/decode/max-records"
	passiveDecodeYMinID                 = passiveFamily + "/" + passiveVersionV1 + "/decode/y-min-boundary"
	passiveDecodeYMaxID                 = passiveFamily + "/" + passiveVersionV1 + "/decode/y-max-boundary"
	passiveDecodeHealthMinID            = passiveFamily + "/" + passiveVersionV1 + "/decode/health-min-boundary"
	passiveDecodeHealthMaxID            = passiveFamily + "/" + passiveVersionV1 + "/decode/health-max-boundary"
	passiveDecodeCount33ID              = passiveFamily + "/" + passiveVersionV1 + "/decode/count-33"
	passiveDecodeTruncatedFixtureID     = passiveFamily + "/" + passiveVersionV1 + "/decode/truncated-fixture"
	passiveDecodeInvalidVersionFutureID = passiveFamily + "/" + passiveVersionV1 + "/decode/invalid-version-future"

	passiveEncodeV1ExactID       = passiveFamily + "/" + passiveVersionV1 + "/encode/v1-fixture-exact"
	passiveEncodeEmptyID         = passiveFamily + "/" + passiveVersionV1 + "/encode/empty"
	passiveEncodeMaxRecordsID    = passiveFamily + "/" + passiveVersionV1 + "/encode/max-records"
	passiveEncodeUnsortedID      = passiveFamily + "/" + passiveVersionV1 + "/encode/unsorted-canonical"
	passiveEncodeCapacityMinusID = passiveFamily + "/" + passiveVersionV1 + "/encode/capacity-minus-one"
	passiveEncodeCount33ID       = passiveFamily + "/" + passiveVersionV1 + "/encode/count-33"
)

type passiveCaseArguments struct {
	Capacity *uint32 `json:"capacity,omitempty"`
}

type passiveCandidate struct {
	Spec    CaseSpec
	Assets  map[string][]byte
	Expect  storageSaveOutcome
	Encoded []byte
}

func passiveRecordOffset(index int) int {
	return passiveHeaderLength + index*passiveRecordLength
}

func passiveArgumentsJSON(capacity *uint32) json.RawMessage {
	args := passiveCaseArguments{}
	if capacity != nil {
		args.Capacity = capacity
	}
	raw, err := json.Marshal(args)
	if err != nil {
		panic(err)
	}
	return raw
}

func parsePassiveArguments(c CaseSpec) (passiveCaseArguments, error) {
	var args passiveCaseArguments
	dec := json.NewDecoder(bytes.NewReader(c.Arguments))
	dec.DisallowUnknownFields()
	if err := dec.Decode(&args); err != nil {
		return passiveCaseArguments{}, fmt.Errorf("decode arguments: %w", err)
	}
	var trailing any
	if err := dec.Decode(&trailing); err != io.EOF {
		return passiveCaseArguments{}, fmt.Errorf("arguments carry trailing content")
	}
	return args, nil
}

func passiveSchemaVersion(input []byte) (uint32, error) {
	if len(input) < 12 {
		return 0, fmt.Errorf("input shorter than schema header")
	}
	return binary.LittleEndian.Uint32(input[8:12]), nil
}

func passiveStorageErrorCategory(err error) (string, bool) {
	switch {
	case errors.Is(err, storagedef.ErrCorrupt):
		return "corrupt", true
	case errors.Is(err, storagedef.ErrFutureVersion):
		return "future_version", true
	default:
		return "", false
	}
}

func fixturePassiveRecords() []passive.StoredPassiveMob {
	grazer := passive.StoredPassiveMob{
		ID: 0x8000000000000002, Dimension: core.Overworld,
		Position: [3]float32{-12.5, 70.25, 3.5}, Velocity: [3]float32{-1.25, 0, 0.5},
		OnGround: true, Yaw: 1.25, Health: 17,
	}
	idle := passive.StoredPassiveMob{
		ID: 0x4000000000000001, Dimension: core.Overworld,
		Position: [3]float32{0.5, 64, -9.75}, Velocity: [3]float32{0, -3.25, 0},
		OnGround: false, Yaw: -2.5, Health: core.MaxHealth,
	}
	calf := passive.StoredPassiveMob{
		ID: 1, Dimension: core.Overworld,
		Position: [3]float32{8.5, 65.5, 9.75}, Velocity: [3]float32{2, 0, -2},
		OnGround: true, Yaw: 3, Health: 1,
	}
	return []passive.StoredPassiveMob{grazer, idle, calf}
}

func passiveRecordValueTree(record passive.StoredPassiveMob) storageValueNode {
	vec3 := func(values [3]float32) storageValueNode {
		return storageValueArray([]storageValueNode{
			storageValueF32(values[0]),
			storageValueF32(values[1]),
			storageValueF32(values[2]),
		})
	}
	return storageValueObject(map[string]storageValueNode{
		"id":        storageValueUnsigned(record.ID),
		"dimension": storageValueSigned(int64(record.Dimension)),
		"position":  vec3(record.Position),
		"velocity":  vec3(record.Velocity),
		"on_ground": storageValueBool(record.OnGround),
		"yaw":       storageValueF32(record.Yaw),
		"health":    storageValueUnsigned(uint64(record.Health)),
	})
}

func passiveStoredValueTree(stored passive.StoredPassiveMobs) storageValueNode {
	records := make([]storageValueNode, 0, len(stored.Records))
	for _, record := range stored.Records {
		records = append(records, passiveRecordValueTree(record))
	}
	return storageValueObject(map[string]storageValueNode{
		"revision": storageValueUnsigned(stored.Revision),
		"records":  storageValueArray(records),
	})
}

func storedPassiveToSave(stored passive.StoredPassiveMobs) passive.PassiveMobsSave {
	return passive.PassiveMobsSave{Revision: stored.Revision, Records: slices.Clone(stored.Records)}
}

func repairPassiveCRC(payload []byte) {
	hasher := crc32.New(crc32.MakeTable(crc32.Castagnoli))
	_, _ = hasher.Write(payload[8:28])
	_, _ = hasher.Write(payload[32:])
	binary.LittleEndian.PutUint32(payload[28:], hasher.Sum32())
}

func patchAndRepairPassiveU32(offset int, value uint32) func([]byte) {
	return func(payload []byte) {
		binary.LittleEndian.PutUint32(payload[offset:], value)
		repairPassiveCRC(payload)
	}
}

func putPassiveF32(offset int, value float32) func([]byte) {
	return func(payload []byte) {
		binary.LittleEndian.PutUint32(payload[offset:], math.Float32bits(value))
	}
}

func validPassiveWire(t *testing.T) []byte {
	encoded, err := passive.Encode(passive.PassiveMobsSave{Revision: 9, Records: fixturePassiveRecords()})
	if err != nil {
		t.Fatalf("encode valid v1 wire: %v", err)
	}
	return encoded
}

func readPassiveFixture(t *testing.T, root string) []byte {
	rel := "packages/server/storage/passive/testdata/passive-mobs-v1.bin"
	path := filepath.Join(root, filepath.FromSlash(rel))
	data, err := os.ReadFile(path)
	if err != nil {
		t.Fatalf("read %s: %v", rel, err)
	}
	return data
}

func runPassiveDecode(c CaseSpec, input []byte) (Outcome, []byte, error) {
	schema, err := passiveSchemaVersion(input)
	if err != nil {
		return Outcome{}, nil, fmt.Errorf("runtime-oracle: case %s: %w", c.ID, err)
	}
	wantVersion, err := strconv.ParseUint(c.Version, 10, 32)
	if err != nil {
		return Outcome{}, nil, fmt.Errorf("runtime-oracle: case %s: invalid case version %q", c.ID, c.Version)
	}

	stored, err := passive.Decode(input)
	if err != nil {
		category, ok := passiveStorageErrorCategory(err)
		if !ok {
			return Outcome{}, nil, fmt.Errorf("runtime-oracle: case %s: unclassified rejection: %w", c.ID, err)
		}
		return Outcome{Kind: "error", Category: category}, nil, nil
	}
	if uint64(schema) != wantVersion {
		return Outcome{}, nil, fmt.Errorf("runtime-oracle: case %s: accepted input schema %d, want %s", c.ID, schema, c.Version)
	}
	digest := storageValueSHA256(passiveStoredValueTree(stored))
	return Outcome{Kind: "ok", Category: "save", Fields: map[string]any{"value_sha256": digest}}, nil, nil
}

func runPassiveEncode(c CaseSpec, input []byte) (Outcome, []byte, error) {
	args, err := parsePassiveArguments(c)
	if err != nil {
		return Outcome{}, nil, fmt.Errorf("runtime-oracle: case %s: %w", c.ID, err)
	}

	if c.ID == passiveEncodeCount33ID {
		records := make([]passive.StoredPassiveMob, passive.MaxPassiveMobs+1)
		for index := range records {
			records[index] = passive.StoredPassiveMob{
				ID: uint64(index) + 1, Dimension: core.Overworld, Health: 1,
			}
		}
		_, err := passive.Encode(passive.PassiveMobsSave{Revision: 1, Records: records})
		if err != nil {
			category, ok := passiveStorageErrorCategory(err)
			if !ok {
				return Outcome{}, nil, fmt.Errorf("runtime-oracle: case %s: unclassified rejection: %w", c.ID, err)
			}
			return Outcome{Kind: "error", Category: category}, nil, nil
		}
		return Outcome{}, nil, fmt.Errorf("runtime-oracle: case %s: expected encode rejection", c.ID)
	}

	stored, err := passive.Decode(input)
	if err != nil {
		category, ok := passiveStorageErrorCategory(err)
		if !ok {
			return Outcome{}, nil, fmt.Errorf("runtime-oracle: case %s: unclassified rejection: %w", c.ID, err)
		}
		return Outcome{Kind: "error", Category: category}, nil, nil
	}
	digest := storageValueSHA256(passiveStoredValueTree(stored))
	save := storedPassiveToSave(stored)
	if c.ID == passiveEncodeUnsortedID {
		save.Records = fixturePassiveRecords()
	}

	encoded, err := passive.Encode(save)
	if err != nil {
		category, ok := passiveStorageErrorCategory(err)
		if !ok {
			return Outcome{}, nil, fmt.Errorf("runtime-oracle: case %s: unclassified encode rejection: %w", c.ID, err)
		}
		return Outcome{Kind: "error", Category: category}, nil, nil
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

	round, err := passive.Decode(encoded)
	if err != nil {
		return Outcome{}, nil, fmt.Errorf("runtime-oracle: case %s: re-decode output: %w", c.ID, err)
	}
	if !reflect.DeepEqual(round.Records, stored.Records) {
		return Outcome{}, nil, fmt.Errorf("runtime-oracle: case %s: encoded output record drift", c.ID)
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

func passiveCorpusRoutes() map[ConsumerRoute]GoOperation {
	return map[ConsumerRoute]GoOperation{
		{FamilyID: passiveFamily, Version: passiveVersionV1, Operation: "decode"}: runPassiveDecode,
		{FamilyID: passiveFamily, Version: passiveVersionV1, Operation: "encode"}: runPassiveEncode,
	}
}

func passiveRoutes() []ConsumerRoute {
	return []ConsumerRoute{
		{FamilyID: passiveFamily, Version: passiveVersionV1, Operation: "decode"},
		{FamilyID: passiveFamily, Version: passiveVersionV1, Operation: "encode"},
	}
}

func buildPassiveCandidate(
	t *testing.T,
	id, operation, caseVersion string,
	input []byte,
	args json.RawMessage,
	wantEncoded []byte,
) passiveCandidate {
	t.Helper()
	var producer GoOperation
	switch operation {
	case "decode":
		producer = runPassiveDecode
	case "encode":
		producer = runPassiveEncode
	default:
		t.Fatalf("unsupported operation %q", operation)
	}
	spec := CaseSpec{
		ID:           id,
		Family:       passiveFamily,
		Version:      caseVersion,
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
		if len(producedEncoded) == 0 && saveOutcome.Kind == "ok" {
			t.Fatalf("encode case %s produced no bytes", id)
		}
		if saveOutcome.Kind == "ok" {
			if encoded != nil && !bytes.Equal(producedEncoded, encoded) {
				t.Fatalf("encode case %s bytes mismatch", id)
			}
			encoded = producedEncoded
		}
	}
	stem := strings.ReplaceAll(id, "/", "_")
	inputRel := filepath.ToSlash(filepath.Join(passiveCorpusRelDir, stem+".input.bin"))
	expectedRel := filepath.ToSlash(filepath.Join(passiveCorpusRelDir, stem+".expected.json"))
	assets := map[string][]byte{
		inputRel:    input,
		expectedRel: expectedBytes,
	}
	spec.Input = AssetRef{Path: inputRel, SHA256: digestOf(t, input)}
	spec.Expected = AssetRef{Path: expectedRel, SHA256: digestOf(t, expectedBytes)}
	if operation == "encode" && saveOutcome.Kind == "ok" {
		encodedRel := filepath.ToSlash(filepath.Join(passiveCorpusRelDir, stem+".encoded.bin"))
		assets[encodedRel] = encoded
		spec.Encoded = &AssetRef{Path: encodedRel, SHA256: digestOf(t, encoded)}
	}
	return passiveCandidate{Spec: spec, Assets: assets, Expect: saveOutcome, Encoded: encoded}
}

func passiveMaxRecordsWire(t *testing.T) []byte {
	records := make([]passive.StoredPassiveMob, passive.MaxPassiveMobs)
	for index := range records {
		records[index] = passive.StoredPassiveMob{
			ID: uint64(index) + 1, Dimension: core.Overworld, Health: 1,
		}
	}
	encoded, err := passive.Encode(passive.PassiveMobsSave{Revision: 23, Records: records})
	if err != nil {
		t.Fatalf("encode max records: %v", err)
	}
	return encoded
}

func passiveYBoundaryWire(t *testing.T, y float32) []byte {
	record := passive.StoredPassiveMob{ID: 8, Dimension: core.Overworld, Health: 20}
	record.Position[1] = y
	encoded, err := passive.Encode(passive.PassiveMobsSave{Revision: 2, Records: []passive.StoredPassiveMob{record}})
	if err != nil {
		t.Fatalf("encode y=%v: %v", y, err)
	}
	return encoded
}

func passiveHealthBoundaryWire(t *testing.T, health uint8) []byte {
	record := passive.StoredPassiveMob{
		ID: 9, Dimension: core.Overworld, Position: [3]float32{0, 64, 0}, Health: health,
	}
	encoded, err := passive.Encode(passive.PassiveMobsSave{Revision: 2, Records: []passive.StoredPassiveMob{record}})
	if err != nil {
		t.Fatalf("encode health=%d: %v", health, err)
	}
	return encoded
}

func passiveCount33HeaderWire() []byte {
	oversizedCount := make([]byte, 32)
	copy(oversizedCount, "PMST")
	binary.LittleEndian.PutUint32(oversizedCount[4:], 1)
	binary.LittleEndian.PutUint32(oversizedCount[8:], 1)
	binary.LittleEndian.PutUint64(oversizedCount[12:], 1)
	binary.LittleEndian.PutUint32(oversizedCount[20:], passive.MaxPassiveMobs+1)
	binary.LittleEndian.PutUint32(oversizedCount[24:], (passive.MaxPassiveMobs+1)*passiveRecordLength)
	repairPassiveCRC(oversizedCount)
	return oversizedCount
}

func passiveCorruptCandidates(t *testing.T, valid []byte) []passiveCandidate {
	t.Helper()
	args := passiveArgumentsJSON(nil)
	base0 := passiveRecordOffset(0)
	base1 := passiveRecordOffset(1)
	base2 := passiveRecordOffset(2)

	type corruptSpec struct {
		suffix   string
		category string
		mutate   func([]byte)
	}
	specs := []corruptSpec{
		{"corrupt-magic", "corrupt", func(p []byte) { p[0] ^= 0x01 }},
		{"corrupt-envelope-zero", "corrupt", func(p []byte) {
			binary.LittleEndian.PutUint32(p[4:], 0)
		}},
		{"corrupt-envelope-future", "future_version", func(p []byte) {
			binary.LittleEndian.PutUint32(p[4:], 2)
		}},
		{"invalid-version-zero", "corrupt", func(p []byte) {
			binary.LittleEndian.PutUint32(p[8:], 0)
		}},
		{"invalid-version-future", "future_version", func(p []byte) {
			binary.LittleEndian.PutUint32(p[8:], passive.CurrentSchema+1)
		}},
		{"corrupt-revision-zero", "corrupt", func(p []byte) {
			binary.LittleEndian.PutUint64(p[12:], 0)
		}},
		{"corrupt-payload-length", "corrupt", func(p []byte) {
			binary.LittleEndian.PutUint32(p[24:], 1)
		}},
		{"corrupt-count-payload", "corrupt", func(p []byte) {
			binary.LittleEndian.PutUint32(p[20:], 4)
		}},
		{"corrupt-crc", "corrupt", func(p []byte) { p[28] ^= 0x01 }},
		{"corrupt-duplicate-id", "corrupt", func(p []byte) {
			copy(p[base1+passiveWireID:], p[base0+passiveWireID:base0+passiveWireID+8])
			repairPassiveCRC(p)
		}},
		{"corrupt-descending-ids", "corrupt", func(p []byte) {
			first := bytes.Clone(p[base0+passiveWireID : base0+passiveWireID+8])
			copy(p[base0+passiveWireID:], p[base1+passiveWireID:base1+passiveWireID+8])
			copy(p[base1+passiveWireID:], first)
			repairPassiveCRC(p)
		}},
		{"corrupt-zero-id", "corrupt", func(p []byte) {
			clear(p[base0+passiveWireID : base0+passiveWireID+8])
			repairPassiveCRC(p)
		}},
		{"corrupt-dimension", "corrupt", patchAndRepairPassiveU32(base1+passiveWireDimension, 1)},
		{"corrupt-nan-position", "corrupt", func(p []byte) {
			putPassiveF32(base0+passiveWirePosition+4, float32(math.NaN()))(p)
			repairPassiveCRC(p)
		}},
		{"corrupt-inf-velocity", "corrupt", func(p []byte) {
			putPassiveF32(base2+passiveWireVelocity+8, float32(math.Inf(1)))(p)
			repairPassiveCRC(p)
		}},
		{"corrupt-nan-yaw", "corrupt", func(p []byte) {
			putPassiveF32(base1+passiveWireYaw, float32(math.NaN()))(p)
			repairPassiveCRC(p)
		}},
		{"corrupt-health-zero", "corrupt", func(p []byte) {
			p[base0+passiveWireHealth] = 0
			repairPassiveCRC(p)
		}},
		{"corrupt-health-above", "corrupt", func(p []byte) {
			p[base0+passiveWireHealth] = core.MaxHealth + 1
			repairPassiveCRC(p)
		}},
		{"corrupt-y-below", "corrupt", func(p []byte) {
			putPassiveF32(base0+passiveWirePosition+4, -64.5)(p)
			repairPassiveCRC(p)
		}},
		{"corrupt-y-at-top", "corrupt", func(p []byte) {
			putPassiveF32(base0+passiveWirePosition+4, float32(core.MaxY))(p)
			repairPassiveCRC(p)
		}},
		{"corrupt-bool", "corrupt", func(p []byte) {
			p[base0+passiveWireOnGround] = 2
			repairPassiveCRC(p)
		}},
	}
	for index := range 30 {
		specs = append(specs, corruptSpec{
			suffix:   fmt.Sprintf("corrupt-reserved-%02d", index),
			category: "corrupt",
			mutate: func(p []byte) {
				p[base0+passiveWireReserved+index] ^= 0x01
				repairPassiveCRC(p)
			},
		})
	}

	out := make([]passiveCandidate, 0, len(specs)+4)
	for _, spec := range specs {
		payload := bytes.Clone(valid)
		spec.mutate(payload)
		id := passiveFamily + "/" + passiveVersionV1 + "/decode/" + spec.suffix
		out = append(out, buildPassiveCandidate(t, id, "decode", passiveVersionV1, payload, args, nil))
	}
	for _, trunc := range []struct {
		suffix string
		input  []byte
	}{
		{"truncated-tail", valid[:len(valid)-1]},
		{"truncated-header-only", valid[:passiveHeaderLength]},
		{"truncated-short-record", valid[:passiveHeaderLength+passiveRecordLength-1]},
		{"trailing-byte", append(bytes.Clone(valid), 0)},
	} {
		id := passiveFamily + "/" + passiveVersionV1 + "/decode/" + trunc.suffix
		out = append(out, buildPassiveCandidate(t, id, "decode", passiveVersionV1, trunc.input, args, nil))
	}
	return out
}

func passiveCandidates(t *testing.T) []passiveCandidate {
	t.Helper()
	root := mustRepoRoot(t)
	args := passiveArgumentsJSON(nil)
	v1 := readPassiveFixture(t, root)
	valid := validPassiveWire(t)
	emptyWire, err := passive.Encode(passive.PassiveMobsSave{Revision: 4})
	if err != nil {
		t.Fatalf("encode empty: %v", err)
	}
	maxWire := passiveMaxRecordsWire(t)
	v1Reencoded, err := passive.Encode(storedPassiveToSave(mustPassiveDecode(t, v1)))
	if err != nil {
		t.Fatalf("re-encode v1 fixture: %v", err)
	}
	canonical := buildPassiveCandidate(t, passiveEncodeV1ExactID, "encode", passiveVersionV1, v1, args, v1)
	capacity := uint32(len(canonical.Encoded) - 1)
	capacityCase := buildPassiveCandidate(t, passiveEncodeCapacityMinusID, "encode", passiveVersionV1, v1, passiveArgumentsJSON(&capacity), nil)

	positive := []passiveCandidate{
		buildPassiveCandidate(t, passiveDecodeV1FixtureID, "decode", passiveVersionV1, v1, args, nil),
		buildPassiveCandidate(t, passiveDecodeEmptyID, "decode", passiveVersionV1, emptyWire, args, nil),
		buildPassiveCandidate(t, passiveDecodeMaxRecordsID, "decode", passiveVersionV1, maxWire, args, nil),
		buildPassiveCandidate(t, passiveDecodeYMinID, "decode", passiveVersionV1, passiveYBoundaryWire(t, float32(core.MinY)), args, nil),
		buildPassiveCandidate(t, passiveDecodeYMaxID, "decode", passiveVersionV1, passiveYBoundaryWire(t, 319), args, nil),
		buildPassiveCandidate(t, passiveDecodeHealthMinID, "decode", passiveVersionV1, passiveHealthBoundaryWire(t, 1), args, nil),
		buildPassiveCandidate(t, passiveDecodeHealthMaxID, "decode", passiveVersionV1, passiveHealthBoundaryWire(t, core.MaxHealth), args, nil),
		buildPassiveCandidate(t, passiveDecodeCount33ID, "decode", passiveVersionV1, passiveCount33HeaderWire(), args, nil),
		buildPassiveCandidate(t, passiveDecodeTruncatedFixtureID, "decode", passiveVersionV1, v1[:len(v1)-1], args, nil),
		canonical,
		buildPassiveCandidate(t, passiveEncodeEmptyID, "encode", passiveVersionV1, emptyWire, args, emptyWire),
		buildPassiveCandidate(t, passiveEncodeMaxRecordsID, "encode", passiveVersionV1, maxWire, args, maxWire),
		buildPassiveCandidate(t, passiveEncodeUnsortedID, "encode", passiveVersionV1, v1, args, v1Reencoded),
		capacityCase,
		buildPassiveCandidate(t, passiveEncodeCount33ID, "encode", passiveVersionV1, v1, args, nil),
	}
	out := append(positive, passiveCorruptCandidates(t, valid)...)
	return out
}

func mustPassiveDecode(t *testing.T, wire []byte) passive.StoredPassiveMobs {
	stored, err := passive.Decode(wire)
	if err != nil {
		t.Fatalf("decode: %v", err)
	}
	return stored
}

func passiveSelection(t *testing.T, root string, candidates []passiveCandidate) StorageSelection {
	cases := make([]CaseSpec, 0, len(candidates))
	for _, candidate := range candidates {
		cases = append(cases, candidate.Spec)
	}
	sources := []SourceSpec{
		{Path: passiveCodecSourceRel},
		{Path: passiveProducerTestRel},
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
		ProducerID: passiveProducerID,
		Cases:      cases,
		Sources:    sources,
		Routes:     passiveRoutes(),
	}
}

func passiveRunnerManifest(t *testing.T, root string, candidates []passiveCandidate) Inventory {
	t.Helper()
	base, err := LoadInventory(filepath.Join(root, filepath.FromSlash(InventoryRelPath)))
	if err != nil {
		t.Fatalf("load frozen manifest: %v", err)
	}
	selection := passiveSelection(t, root, candidates)
	merged := base
	passiveCases := append([]CaseSpec(nil), selection.Cases...)
	sort.Slice(passiveCases, func(i, j int) bool { return passiveCases[i].ID < passiveCases[j].ID })
	merged.Cases = passiveCases
	caseIDs := make([]string, 0, len(passiveCases))
	for _, c := range passiveCases {
		caseIDs = append(caseIDs, c.ID)
	}
	for index := range merged.Families {
		if merged.Families[index].ID != passiveFamily {
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

func passiveScratchRoot(t *testing.T, candidates []passiveCandidate) string {
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

func passiveObservation(t *testing.T, observations []ExecutedObservation, id string) ExecutedObservation {
	t.Helper()
	for _, obs := range observations {
		if obs.CaseID == id {
			return obs
		}
	}
	t.Fatalf("no observation for case %s", id)
	return ExecutedObservation{}
}

func validatePassiveExportCandidates(t *testing.T, candidates []passiveCandidate) {
	t.Helper()
	seenIDs := make(map[string]struct{}, len(candidates))
	seenPaths := make(map[string]struct{})
	for _, candidate := range candidates {
		if _, ok := seenIDs[candidate.Spec.ID]; ok {
			t.Fatalf("passive export: duplicate case id %s", candidate.Spec.ID)
		}
		seenIDs[candidate.Spec.ID] = struct{}{}
		for relative := range candidate.Assets {
			if _, ok := seenPaths[relative]; ok {
				t.Fatalf("passive export: duplicate asset path %s", relative)
			}
			seenPaths[relative] = struct{}{}
		}
	}
}

func passivePinnedExportChildIsStaleCollision(t *testing.T, producerChild string) bool {
	t.Helper()
	expectedRoot, err := filepath.Abs(passiveExportDir)
	if err != nil {
		t.Fatalf("abs passive export dir: %v", err)
	}
	absChild, err := filepath.Abs(producerChild)
	if err != nil {
		t.Fatalf("abs producer child: %v", err)
	}
	wantChild := filepath.Join(expectedRoot, filepath.FromSlash("runtime-oracle/storage-passive"))
	if absChild != wantChild {
		t.Fatalf("producer child %s is not the pinned passive export path", producerChild)
	}
	manifestPath := filepath.Join(producerChild, storageSelectionManifest)
	data, err := os.ReadFile(manifestPath)
	if err != nil {
		return false
	}
	var payload struct {
		Cases []struct {
			ID string `json:"id"`
		} `json:"cases"`
	}
	if err := json.Unmarshal(data, &payload); err != nil {
		return false
	}
	staleID := passiveFamily + "/" + passiveVersionV1 + "/decode/truncated-tail"
	count := 0
	for _, spec := range payload.Cases {
		if spec.ID == staleID {
			count++
		}
	}
	return count >= 2
}

func exportPassiveSelectionCandidate(t *testing.T, root string, candidates []passiveCandidate) string {
	t.Helper()
	if strings.TrimSpace(os.Getenv(runtimeOracleExportDirEnv)) == "" {
		return ""
	}
	validatePassiveExportCandidates(t, candidates)
	selection := passiveSelection(t, root, candidates)
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
				t.Fatalf("passive export: duplicate asset path %s", relative)
			}
			seen[relative] = struct{}{}
			assets = append(assets, generatedAsset{RelativePath: relative, Data: data})
		}
	}
	exportRoot, err := exportGeneratedAssets(root, strings.TrimSpace(os.Getenv(runtimeOracleExportDirEnv)), passiveProducerID, assets)
	if err != nil {
		t.Fatalf("export passive selection: %v", err)
	}
	return exportRoot
}

func TestStoragePassiveArgumentsValidate(t *testing.T) {
	if err := validateStorageArguments(passiveFamily, "decode", passiveArgumentsJSON(nil)); err != nil {
		t.Fatalf("validate passive decode arguments: %v", err)
	}
	if err := validateStorageArguments(passiveFamily, "encode", passiveArgumentsJSON(nil)); err != nil {
		t.Fatalf("validate passive encode arguments: %v", err)
	}
}

func TestStoragePassiveProducerExecutesEveryCase(t *testing.T) {
	root := mustRepoRoot(t)
	candidates := passiveCandidates(t)
	const wantCases = 70
	if len(candidates) != wantCases {
		t.Fatalf("candidate count = %d, want %d", len(candidates), wantCases)
	}
	manifest := passiveRunnerManifest(t, root, candidates)
	staged := passiveScratchRoot(t, candidates)

	observations, err := RunStorageCases(staged, manifest, passiveCorpusRoutes())
	if err != nil {
		t.Fatalf("RunStorageCases: %v", err)
	}
	if len(observations) != len(candidates) {
		t.Fatalf("produced %d observations, want %d", len(observations), len(candidates))
	}
	for _, candidate := range candidates {
		obs := passiveObservation(t, observations, candidate.Spec.ID)
		got, err := outcomeToStorageSave(obs.Outcome)
		if err != nil {
			t.Fatalf("case %s: %v", candidate.Spec.ID, err)
		}
		if !storageSaveOutcomesEqual(got, candidate.Expect) {
			t.Fatalf("case %s produced %#v, want %#v", candidate.Spec.ID, got, candidate.Expect)
		}
	}
}

func TestStoragePassiveFutureVersionCategory(t *testing.T) {
	for _, candidate := range passiveCandidates(t) {
		if candidate.Spec.ID != passiveDecodeInvalidVersionFutureID {
			continue
		}
		if candidate.Expect.Kind != "error" || candidate.Expect.Category != "future_version" {
			t.Fatalf("future schema case outcome %#v, want future_version error", candidate.Expect)
		}
		return
	}
	t.Fatal("missing invalid-version-future case")
}

func TestStoragePassivePositionDigestMutationFailsComparison(t *testing.T) {
	root := mustRepoRoot(t)
	v1 := readPassiveFixture(t, root)
	stored := mustPassiveDecode(t, v1)
	stale := stored
	for index := range stale.Records {
		stale.Records[index].Position[0] += 0.25
		break
	}
	spec := CaseSpec{
		ID: passiveDecodeV1FixtureID, Family: passiveFamily, Version: passiveVersionV1,
		Operation: "decode", Arguments: passiveArgumentsJSON(nil), InputFormat: "binary",
		Checkpoints: []string{"0"}, RustConsumer: storageConsumerName,
	}
	outcome, _, err := runPassiveDecode(spec, v1)
	if err != nil {
		t.Fatalf("runPassiveDecode: %v", err)
	}
	got, err := outcomeToStorageSave(outcome)
	if err != nil {
		t.Fatal(err)
	}
	staleExpect := storageSaveOutcome{
		Kind: "ok", Category: "save", ValueSHA256: storageValueSHA256(passiveStoredValueTree(stale)),
	}
	if storageSaveOutcomesEqual(got, staleExpect) {
		t.Fatal("position mutation still matches stale expected digest")
	}
}

func TestStoragePassiveOnGroundDigestMutationFailsComparison(t *testing.T) {
	root := mustRepoRoot(t)
	v1 := readPassiveFixture(t, root)
	stored := mustPassiveDecode(t, v1)
	stale := stored
	for index := range stale.Records {
		if stale.Records[index].OnGround {
			stale.Records[index].OnGround = false
			break
		}
	}
	spec := CaseSpec{
		ID: passiveDecodeV1FixtureID, Family: passiveFamily, Version: passiveVersionV1,
		Operation: "decode", Arguments: passiveArgumentsJSON(nil), InputFormat: "binary",
		Checkpoints: []string{"0"}, RustConsumer: storageConsumerName,
	}
	outcome, _, err := runPassiveDecode(spec, v1)
	if err != nil {
		t.Fatalf("runPassiveDecode: %v", err)
	}
	got, err := outcomeToStorageSave(outcome)
	if err != nil {
		t.Fatal(err)
	}
	staleExpect := storageSaveOutcome{
		Kind: "ok", Category: "save", ValueSHA256: storageValueSHA256(passiveStoredValueTree(stale)),
	}
	if storageSaveOutcomesEqual(got, staleExpect) {
		t.Fatal("on_ground mutation still matches stale expected digest")
	}
}

func TestStoragePassiveExportUnsetWritesNothing(t *testing.T) {
	root := mustRepoRoot(t)
	t.Setenv(runtimeOracleExportDirEnv, "")
	before, err := os.ReadDir(root)
	if err != nil {
		t.Fatal(err)
	}
	_ = exportPassiveSelectionCandidate(t, root, passiveCandidates(t))
	after, err := os.ReadDir(root)
	if err != nil {
		t.Fatal(err)
	}
	if len(before) != len(after) {
		t.Fatal("unset export must not mutate repository tree")
	}
}

func TestStoragePassiveCandidatesExportForReview(t *testing.T) {
	root := mustRepoRoot(t)
	t.Setenv(runtimeOracleExportDirEnv, t.TempDir())
	child := exportPassiveSelectionCandidate(t, root, passiveCandidates(t))
	if child == "" {
		t.Fatal("export root unset after explicit env")
	}
	if _, err := readStorageSelection(child); err != nil {
		t.Fatalf("reload exported selection: %v", err)
	}
}

func TestStoragePassiveExportToPinnedDirectory(t *testing.T) {
	root := mustRepoRoot(t)
	exportRoot := passiveExportDir
	producerChild := filepath.Join(exportRoot, filepath.FromSlash("runtime-oracle/storage-passive"))
	if _, err := os.Lstat(producerChild); err == nil {
		if !passivePinnedExportChildIsStaleCollision(t, producerChild) {
			t.Skip("pinned producer child already exists; reviewed export candidate preserved")
		}
		if err := os.RemoveAll(producerChild); err != nil {
			t.Fatalf("remove stale pinned producer child: %v", err)
		}
	} else if !os.IsNotExist(err) {
		t.Fatalf("stat pinned producer child: %v", err)
	}
	t.Setenv(runtimeOracleExportDirEnv, exportRoot)
	child := exportPassiveSelectionCandidate(t, root, passiveCandidates(t))
	if child == "" {
		t.Fatal("export root unset after explicit env")
	}
	if _, err := readStorageSelection(child); err != nil {
		t.Fatalf("reload exported selection: %v", err)
	}
}
