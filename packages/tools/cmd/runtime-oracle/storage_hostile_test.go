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

	"github.com/channing771/mornlea/packages/server/storage/hostile"
	"github.com/channing771/mornlea/packages/server/storage/storagedef"
	"github.com/channing771/mornlea/packages/shared/core"
)

const (
	hostileFamily          = "save.hostile"
	hostileVersionV2       = "2"
	hostileVersionV1       = "1"
	hostileProducerID      = "runtime-oracle/storage-hostile"
	hostileCorpusRelDir    = "testdata/runtime-migration/cases/storage/hostile"
	hostileProducerTestRel = "packages/tools/cmd/runtime-oracle/storage_hostile_test.go"
	hostileCodecSourceRel  = "packages/server/storage/hostile/hostile_codec.go"
	hostileExportDir       = "/tmp/runtime-oracle-hostile-3.2"

	hostileHeaderLength    = 32
	hostileRecordLengthV1  = 72
	hostileRecordLengthV2  = 73
	hostileMaxDistantTicks = 600
	hostileCooldownPeriod  = 20

	hostileWireID         = 0
	hostileWireDimension  = 8
	hostileWirePosition   = 12
	hostileWireVelocity   = 24
	hostileWireOnGround   = 36
	hostileWireYaw        = 37
	hostileWireHealth     = 41
	hostileWireAttack     = 42
	hostileWireHurt       = 43
	hostileWireBurn       = 44
	hostileWireHasTarget  = 45
	hostileWirePlayerID   = 46
	hostileWireNextRepath = 62
	hostileWireDistant    = 70
	hostileWireKind       = 72

	hostileDecodeV1FixtureID     = hostileFamily + "/" + hostileVersionV1 + "/decode/v1-fixture"
	hostileDecodeV2FixtureID     = hostileFamily + "/" + hostileVersionV2 + "/decode/v2-fixture"
	hostileDecodeEmptyID           = hostileFamily + "/" + hostileVersionV2 + "/decode/empty"
	hostileDecodeMaxRecordsID      = hostileFamily + "/" + hostileVersionV2 + "/decode/max-records"
	hostileDecodeYMinID            = hostileFamily + "/" + hostileVersionV2 + "/decode/y-min-boundary"
	hostileDecodeYMaxID            = hostileFamily + "/" + hostileVersionV2 + "/decode/y-max-boundary"
	hostileDecodeCount65ID      = hostileFamily + "/" + hostileVersionV2 + "/decode/count-65"
	hostileDecodeCooldown20ID   = hostileFamily + "/" + hostileVersionV2 + "/decode/cooldown-20"
	hostileDecodeV1TruncatedID  = hostileFamily + "/" + hostileVersionV1 + "/decode/truncated-tail"
	hostileDecodeInvalidVersionFutureID = hostileFamily + "/" + hostileVersionV2 + "/decode/invalid-version-future"

	hostileEncodeV2ExactID       = hostileFamily + "/" + hostileVersionV2 + "/encode/v2-fixture-exact"
	hostileEncodeV1ReencodeID    = hostileFamily + "/" + hostileVersionV2 + "/encode/v1-fixture-reencode"
	hostileEncodeEmptyID         = hostileFamily + "/" + hostileVersionV2 + "/encode/empty"
	hostileEncodeMaxRecordsID    = hostileFamily + "/" + hostileVersionV2 + "/encode/max-records"
	hostileEncodeUnsortedID      = hostileFamily + "/" + hostileVersionV2 + "/encode/unsorted-canonical"
	hostileEncodeCapacityMinusID = hostileFamily + "/" + hostileVersionV2 + "/encode/capacity-minus-one"
	hostileEncodeCount65ID       = hostileFamily + "/" + hostileVersionV2 + "/encode/count-65"
)

type hostileCaseArguments struct {
	Capacity *uint32 `json:"capacity,omitempty"`
}

type hostileCandidate struct {
	Spec    CaseSpec
	Assets  map[string][]byte
	Expect  storageSaveOutcome
	Encoded []byte
}

func hostileRecordOffset(index int, recordLength int) int {
	return hostileHeaderLength + index*recordLength
}

func hostileArgumentsJSON(capacity *uint32) json.RawMessage {
	args := hostileCaseArguments{}
	if capacity != nil {
		args.Capacity = capacity
	}
	raw, err := json.Marshal(args)
	if err != nil {
		panic(err)
	}
	return raw
}

func parseHostileArguments(c CaseSpec) (hostileCaseArguments, error) {
	var args hostileCaseArguments
	dec := json.NewDecoder(bytes.NewReader(c.Arguments))
	dec.DisallowUnknownFields()
	if err := dec.Decode(&args); err != nil {
		return hostileCaseArguments{}, fmt.Errorf("decode arguments: %w", err)
	}
	var trailing any
	if err := dec.Decode(&trailing); err != io.EOF {
		return hostileCaseArguments{}, fmt.Errorf("arguments carry trailing content")
	}
	return args, nil
}

func hostileSchemaVersion(input []byte) (uint32, error) {
	if len(input) < 12 {
		return 0, fmt.Errorf("input shorter than schema header")
	}
	return binary.LittleEndian.Uint32(input[8:12]), nil
}

func hostileStorageErrorCategory(err error) (string, bool) {
	switch {
	case errors.Is(err, storagedef.ErrCorrupt):
		return "corrupt", true
	case errors.Is(err, storagedef.ErrFutureVersion):
		return "future_version", true
	default:
		return "", false
	}
}

func fixtureHostileTargetPlayerID() core.PlayerID {
	return core.PlayerID{
		0x6f, 0xce, 0x82, 0x77, 0xa9, 0x33, 0x46, 0xcb,
		0x9a, 0x1f, 0xda, 0x13, 0xb7, 0xee, 0x56, 0x44,
	}
}

func fixtureHostileRecords() []hostile.StoredHostileMob {
	tracking := hostile.StoredHostileMob{
		ID: 0x8000000000000002, Dimension: core.Overworld,
		Position: [3]float32{-12.5, 70.25, 3.5}, Velocity: [3]float32{-1.25, 0, 0.5},
		OnGround: true, Yaw: 1.25,
		Health: 17, AttackCooldown: 3, HurtCooldown: 1, BurnCooldown: 5,
		HasTarget: true, PlayerID: fixtureHostileTargetPlayerID(),
		NextRepathTicks: 905, DistantTicks: 120, Kind: 1,
	}
	idle := hostile.StoredHostileMob{
		ID: 0x4000000000000001, Dimension: core.Overworld,
		Position: [3]float32{0.5, 64, -9.75}, Velocity: [3]float32{0, -3.25, 0},
		OnGround: false, Yaw: -2.5,
		Health: core.MaxHealth,
	}
	far := hostile.StoredHostileMob{
		ID: 1, Dimension: core.Overworld,
		Position: [3]float32{8.5, 65.5, 9.75}, Velocity: [3]float32{2, 0, -2},
		OnGround: true, Yaw: 3,
		Health: 1, BurnCooldown: 19, DistantTicks: hostileMaxDistantTicks,
	}
	return []hostile.StoredHostileMob{tracking, idle, far}
}

func hostileRecordValueTree(record hostile.StoredHostileMob) storageValueNode {
	vec3 := func(values [3]float32) storageValueNode {
		return storageValueArray([]storageValueNode{
			storageValueF32(values[0]),
			storageValueF32(values[1]),
			storageValueF32(values[2]),
		})
	}
	return storageValueObject(map[string]storageValueNode{
		"id":                storageValueUnsigned(record.ID),
		"dimension":         storageValueSigned(int64(record.Dimension)),
		"position":          vec3(record.Position),
		"velocity":          vec3(record.Velocity),
		"on_ground":         storageValueBool(record.OnGround),
		"yaw":               storageValueF32(record.Yaw),
		"health":            storageValueUnsigned(uint64(record.Health)),
		"attack_cooldown":   storageValueUnsigned(uint64(record.AttackCooldown)),
		"hurt_cooldown":     storageValueUnsigned(uint64(record.HurtCooldown)),
		"burn_cooldown":     storageValueUnsigned(uint64(record.BurnCooldown)),
		"has_target":        storageValueBool(record.HasTarget),
		"player_id":         storageValueBytes(record.PlayerID[:]),
		"next_repath_ticks": storageValueUnsigned(record.NextRepathTicks),
		"distant_ticks":     storageValueUnsigned(uint64(record.DistantTicks)),
		"kind":              storageValueUnsigned(uint64(record.Kind)),
	})
}

func hostileStoredValueTree(stored hostile.StoredHostileMobs) storageValueNode {
	records := make([]storageValueNode, 0, len(stored.Records))
	for _, record := range stored.Records {
		records = append(records, hostileRecordValueTree(record))
	}
	return storageValueObject(map[string]storageValueNode{
		"revision": storageValueUnsigned(stored.Revision),
		"records":  storageValueArray(records),
	})
}

func storedHostileToSave(stored hostile.StoredHostileMobs) hostile.HostileMobsSave {
	return hostile.HostileMobsSave{Revision: stored.Revision, Records: slices.Clone(stored.Records)}
}

func repairHostileCRC(payload []byte) {
	hasher := crc32.New(crc32.MakeTable(crc32.Castagnoli))
	_, _ = hasher.Write(payload[8:28])
	_, _ = hasher.Write(payload[32:])
	binary.LittleEndian.PutUint32(payload[28:], hasher.Sum32())
}

func patchAndRepairU32(offset int, value uint32) func([]byte) {
	return func(payload []byte) {
		binary.LittleEndian.PutUint32(payload[offset:], value)
		repairHostileCRC(payload)
	}
}

func putHostileF32(offset int, value float32) func([]byte) {
	return func(payload []byte) {
		binary.LittleEndian.PutUint32(payload[offset:], math.Float32bits(value))
	}
}

func hostileValidV2Wire(t *testing.T) []byte {
	encoded, err := hostile.Encode(hostile.HostileMobsSave{Revision: 9, Records: fixtureHostileRecords()})
	if err != nil {
		t.Fatalf("encode valid v2 wire: %v", err)
	}
	return encoded
}

func readHostileFixture(t *testing.T, root string, version int) []byte {
	rel := fmt.Sprintf("packages/server/storage/hostile/testdata/hostile-mobs-v%d.bin", version)
	path := filepath.Join(root, filepath.FromSlash(rel))
	data, err := os.ReadFile(path)
	if err != nil {
		t.Fatalf("read %s: %v", rel, err)
	}
	return data
}

func runHostileDecode(c CaseSpec, input []byte) (Outcome, []byte, error) {
	schema, err := hostileSchemaVersion(input)
	if err != nil {
		return Outcome{}, nil, fmt.Errorf("runtime-oracle: case %s: %w", c.ID, err)
	}
	wantVersion, err := strconv.ParseUint(c.Version, 10, 32)
	if err != nil {
		return Outcome{}, nil, fmt.Errorf("runtime-oracle: case %s: invalid case version %q", c.ID, c.Version)
	}

	stored, err := hostile.Decode(input)
	if err != nil {
		category, ok := hostileStorageErrorCategory(err)
		if !ok {
			return Outcome{}, nil, fmt.Errorf("runtime-oracle: case %s: unclassified rejection: %w", c.ID, err)
		}
		return Outcome{Kind: "error", Category: category}, nil, nil
	}
	if uint64(schema) != wantVersion {
		return Outcome{}, nil, fmt.Errorf("runtime-oracle: case %s: accepted input schema %d, want %s", c.ID, schema, c.Version)
	}
	digest := storageValueSHA256(hostileStoredValueTree(stored))
	return Outcome{Kind: "ok", Category: "save", Fields: map[string]any{"value_sha256": digest}}, nil, nil
}

func runHostileEncode(c CaseSpec, input []byte) (Outcome, []byte, error) {
	args, err := parseHostileArguments(c)
	if err != nil {
		return Outcome{}, nil, fmt.Errorf("runtime-oracle: case %s: %w", c.ID, err)
	}

	if c.ID == hostileEncodeCount65ID {
		records := make([]hostile.StoredHostileMob, hostile.MaxHostileMobs+1)
		for index := range records {
			records[index] = hostile.StoredHostileMob{
				ID: uint64(index) + 1, Dimension: core.Overworld, Health: 1,
			}
		}
		_, err := hostile.Encode(hostile.HostileMobsSave{Revision: 1, Records: records})
		if err != nil {
			category, ok := hostileStorageErrorCategory(err)
			if !ok {
				return Outcome{}, nil, fmt.Errorf("runtime-oracle: case %s: unclassified rejection: %w", c.ID, err)
			}
			return Outcome{Kind: "error", Category: category}, nil, nil
		}
		return Outcome{}, nil, fmt.Errorf("runtime-oracle: case %s: expected encode rejection", c.ID)
	}

	stored, err := hostile.Decode(input)
	if err != nil {
		category, ok := hostileStorageErrorCategory(err)
		if !ok {
			return Outcome{}, nil, fmt.Errorf("runtime-oracle: case %s: unclassified rejection: %w", c.ID, err)
		}
		return Outcome{Kind: "error", Category: category}, nil, nil
	}
	digest := storageValueSHA256(hostileStoredValueTree(stored))
	save := storedHostileToSave(stored)
	if c.ID == hostileEncodeUnsortedID {
		save.Records = fixtureHostileRecords()
	}

	encoded, err := hostile.Encode(save)
	if err != nil {
		category, ok := hostileStorageErrorCategory(err)
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

	round, err := hostile.Decode(encoded)
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

func hostileCorpusRoutes() map[ConsumerRoute]GoOperation {
	return map[ConsumerRoute]GoOperation{
		{FamilyID: hostileFamily, Version: hostileVersionV1, Operation: "decode"}: runHostileDecode,
		{FamilyID: hostileFamily, Version: hostileVersionV2, Operation: "decode"}: runHostileDecode,
		{FamilyID: hostileFamily, Version: hostileVersionV2, Operation: "encode"}: runHostileEncode,
	}
}

func hostileRoutes() []ConsumerRoute {
	return []ConsumerRoute{
		{FamilyID: hostileFamily, Version: hostileVersionV1, Operation: "decode"},
		{FamilyID: hostileFamily, Version: hostileVersionV2, Operation: "decode"},
		{FamilyID: hostileFamily, Version: hostileVersionV2, Operation: "encode"},
	}
}

func buildHostileCandidate(
	t *testing.T,
	id, operation, caseVersion string,
	input []byte,
	args json.RawMessage,
	wantEncoded []byte,
) hostileCandidate {
	t.Helper()
	var producer GoOperation
	switch operation {
	case "decode":
		producer = runHostileDecode
	case "encode":
		producer = runHostileEncode
	default:
		t.Fatalf("unsupported operation %q", operation)
	}
	spec := CaseSpec{
		ID:           id,
		Family:       hostileFamily,
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
	inputRel := filepath.ToSlash(filepath.Join(hostileCorpusRelDir, stem+".input.bin"))
	expectedRel := filepath.ToSlash(filepath.Join(hostileCorpusRelDir, stem+".expected.json"))
	assets := map[string][]byte{
		inputRel:    input,
		expectedRel: expectedBytes,
	}
	spec.Input = AssetRef{Path: inputRel, SHA256: digestOf(t, input)}
	spec.Expected = AssetRef{Path: expectedRel, SHA256: digestOf(t, expectedBytes)}
	if operation == "encode" && saveOutcome.Kind == "ok" {
		encodedRel := filepath.ToSlash(filepath.Join(hostileCorpusRelDir, stem+".encoded.bin"))
		assets[encodedRel] = encoded
		spec.Encoded = &AssetRef{Path: encodedRel, SHA256: digestOf(t, encoded)}
	}
	return hostileCandidate{Spec: spec, Assets: assets, Expect: saveOutcome, Encoded: encoded}
}

func hostileMaxRecordsWire(t *testing.T) []byte {
	records := make([]hostile.StoredHostileMob, hostile.MaxHostileMobs)
	for index := range records {
		records[index] = hostile.StoredHostileMob{
			ID: uint64(index) + 1, Dimension: core.Overworld, Health: 1,
		}
	}
	encoded, err := hostile.Encode(hostile.HostileMobsSave{Revision: 23, Records: records})
	if err != nil {
		t.Fatalf("encode max records: %v", err)
	}
	return encoded
}

func hostileYBoundaryWire(t *testing.T, y float32) []byte {
	record := hostile.StoredHostileMob{ID: 8, Dimension: core.Overworld, Health: 20}
	record.Position[1] = y
	encoded, err := hostile.Encode(hostile.HostileMobsSave{Revision: 2, Records: []hostile.StoredHostileMob{record}})
	if err != nil {
		t.Fatalf("encode y=%v: %v", y, err)
	}
	return encoded
}

func hostileCooldown20Wire(t *testing.T) []byte {
	record := hostile.StoredHostileMob{
		ID: 0x10, Dimension: core.Overworld, Health: 5,
		AttackCooldown: hostileCooldownPeriod,
	}
	encoded, err := hostile.Encode(hostile.HostileMobsSave{Revision: 11, Records: []hostile.StoredHostileMob{record}})
	if err != nil {
		t.Fatalf("encode cooldown-20 witness: %v", err)
	}
	return encoded
}

func hostileCount65HeaderWire() []byte {
	oversizedCount := make([]byte, 32)
	copy(oversizedCount, "MHST")
	binary.LittleEndian.PutUint32(oversizedCount[4:], 1)
	binary.LittleEndian.PutUint32(oversizedCount[8:], 2)
	binary.LittleEndian.PutUint32(oversizedCount[12:], 1)
	binary.LittleEndian.PutUint32(oversizedCount[20:], hostile.MaxHostileMobs+1)
	binary.LittleEndian.PutUint32(oversizedCount[24:], (hostile.MaxHostileMobs+1)*hostileRecordLengthV2)
	repairHostileCRC(oversizedCount)
	return oversizedCount
}

func hostileCorruptCandidates(t *testing.T, valid []byte) []hostileCandidate {
	t.Helper()
	args := hostileArgumentsJSON(nil)
	base0 := hostileRecordOffset(0, hostileRecordLengthV2)
	base1 := hostileRecordOffset(1, hostileRecordLengthV2)
	base2 := hostileRecordOffset(2, hostileRecordLengthV2)

	type corruptSpec struct {
		suffix   string
		version  string
		category string
		mutate   func([]byte)
	}
	specs := []corruptSpec{
		{"corrupt-magic", hostileVersionV2, "corrupt", func(p []byte) { p[0] ^= 0x01 }},
		{"corrupt-envelope-zero", hostileVersionV2, "corrupt", func(p []byte) {
			binary.LittleEndian.PutUint32(p[4:], 0)
		}},
		{"corrupt-envelope-future", hostileVersionV2, "future_version", func(p []byte) {
			binary.LittleEndian.PutUint32(p[4:], 2)
		}},
		{"invalid-version-zero", hostileVersionV2, "corrupt", func(p []byte) {
			binary.LittleEndian.PutUint32(p[8:], 0)
		}},
		{"invalid-version-future", hostileVersionV2, "future_version", func(p []byte) {
			binary.LittleEndian.PutUint32(p[8:], hostile.CurrentSchema+1)
		}},
		{"corrupt-revision-zero", hostileVersionV2, "corrupt", func(p []byte) {
			binary.LittleEndian.PutUint64(p[12:], 0)
		}},
		{"corrupt-payload-length", hostileVersionV2, "corrupt", func(p []byte) {
			binary.LittleEndian.PutUint32(p[24:], 1)
		}},
		{"corrupt-count-payload", hostileVersionV2, "corrupt", func(p []byte) {
			binary.LittleEndian.PutUint32(p[20:], 4)
		}},
		{"corrupt-crc", hostileVersionV2, "corrupt", func(p []byte) { p[28] ^= 0x01 }},
		{"corrupt-duplicate-id", hostileVersionV2, "corrupt", func(p []byte) {
			copy(p[base1+hostileWireID:], p[base0+hostileWireID:base0+hostileWireID+8])
			repairHostileCRC(p)
		}},
		{"corrupt-descending-ids", hostileVersionV2, "corrupt", func(p []byte) {
			first := bytes.Clone(p[base0+hostileWireID : base0+hostileWireID+8])
			copy(p[base0+hostileWireID:], p[base1+hostileWireID:base1+hostileWireID+8])
			copy(p[base1+hostileWireID:], first)
			repairHostileCRC(p)
		}},
		{"corrupt-zero-id", hostileVersionV2, "corrupt", func(p []byte) {
			clear(p[base0+hostileWireID : base0+hostileWireID+8])
			repairHostileCRC(p)
		}},
		{"corrupt-dimension", hostileVersionV2, "corrupt", patchAndRepairU32(base1+hostileWireDimension, 7)},
		{"corrupt-nan-position", hostileVersionV2, "corrupt", func(p []byte) {
			putHostileF32(base0+hostileWirePosition+4, float32(math.NaN()))(p)
			repairHostileCRC(p)
		}},
		{"corrupt-inf-velocity", hostileVersionV2, "corrupt", func(p []byte) {
			putHostileF32(base2+hostileWireVelocity+8, float32(math.Inf(1)))(p)
			repairHostileCRC(p)
		}},
		{"corrupt-nan-yaw", hostileVersionV2, "corrupt", func(p []byte) {
			putHostileF32(base1+hostileWireYaw, float32(math.NaN()))(p)
			repairHostileCRC(p)
		}},
		{"corrupt-health-zero", hostileVersionV2, "corrupt", func(p []byte) {
			p[base0+hostileWireHealth] = 0
			repairHostileCRC(p)
		}},
		{"corrupt-health-above", hostileVersionV2, "corrupt", func(p []byte) {
			p[base0+hostileWireHealth] = core.MaxHealth + 1
			repairHostileCRC(p)
		}},
		{"corrupt-attack-cooldown", hostileVersionV2, "corrupt", func(p []byte) {
			p[base0+hostileWireAttack] = hostileCooldownPeriod + 1
			repairHostileCRC(p)
		}},
		{"corrupt-hurt-cooldown", hostileVersionV2, "corrupt", func(p []byte) {
			p[base0+hostileWireHurt] = hostileCooldownPeriod + 1
			repairHostileCRC(p)
		}},
		{"corrupt-burn-cooldown", hostileVersionV2, "corrupt", func(p []byte) {
			p[base2+hostileWireBurn] = hostileCooldownPeriod + 1
			repairHostileCRC(p)
		}},
		{"corrupt-distant-above", hostileVersionV2, "corrupt", func(p []byte) {
			binary.LittleEndian.PutUint16(p[base2+hostileWireDistant:], hostileMaxDistantTicks+1)
			repairHostileCRC(p)
		}},
		{"corrupt-kind-above", hostileVersionV2, "corrupt", func(p []byte) {
			binary.LittleEndian.PutUint16(p[base2+hostileWireDistant:], 0)
			p[len(p)-1] = 2
			repairHostileCRC(p)
		}},
		{"corrupt-y-below", hostileVersionV2, "corrupt", func(p []byte) {
			putHostileF32(base0+hostileWirePosition+4, -64.5)(p)
			repairHostileCRC(p)
		}},
		{"corrupt-y-at-top", hostileVersionV2, "corrupt", func(p []byte) {
			putHostileF32(base0+hostileWirePosition+4, float32(core.MaxY))(p)
			repairHostileCRC(p)
		}},
		{"corrupt-bool", hostileVersionV2, "corrupt", func(p []byte) {
			p[base0+hostileWireHasTarget] = 2
			repairHostileCRC(p)
		}},
		{"corrupt-absent-target-keeps-id", hostileVersionV2, "corrupt", func(p []byte) {
			p[base2+hostileWireHasTarget] = 0
			repairHostileCRC(p)
		}},
		{"corrupt-target-zero-id", hostileVersionV2, "corrupt", func(p []byte) {
			clear(p[base2+hostileWirePlayerID : base2+hostileWirePlayerID+16])
			repairHostileCRC(p)
		}},
		{"corrupt-target-bad-version", hostileVersionV2, "corrupt", func(p []byte) {
			p[base2+hostileWirePlayerID+6] = 0x36
			repairHostileCRC(p)
		}},
		{"corrupt-target-bad-variant", hostileVersionV2, "corrupt", func(p []byte) {
			p[base2+hostileWirePlayerID+8] = 0x1f
			repairHostileCRC(p)
		}},
	}

	out := make([]hostileCandidate, 0, len(specs)+4)
	for _, spec := range specs {
		payload := bytes.Clone(valid)
		spec.mutate(payload)
		id := hostileFamily + "/" + spec.version + "/decode/" + spec.suffix
		out = append(out, buildHostileCandidate(t, id, "decode", spec.version, payload, args, nil))
	}
	for _, trunc := range []struct {
		suffix string
		input  []byte
	}{
		{"truncated-tail", valid[:len(valid)-1]},
		{"truncated-header-only", valid[:hostileHeaderLength]},
		{"truncated-short-record", valid[:hostileHeaderLength+hostileRecordLengthV2-1]},
		{"trailing-byte", append(bytes.Clone(valid), 0)},
	} {
		id := hostileFamily + "/" + hostileVersionV2 + "/decode/" + trunc.suffix
		out = append(out, buildHostileCandidate(t, id, "decode", hostileVersionV2, trunc.input, args, nil))
	}
	return out
}

func hostileCandidates(t *testing.T) []hostileCandidate {
	t.Helper()
	root := mustRepoRoot(t)
	args := hostileArgumentsJSON(nil)
	v1 := readHostileFixture(t, root, 1)
	v2 := readHostileFixture(t, root, 2)
	valid := hostileValidV2Wire(t)
	emptyWire, err := hostile.Encode(hostile.HostileMobsSave{Revision: 4})
	if err != nil {
		t.Fatalf("encode empty: %v", err)
	}
	maxWire := hostileMaxRecordsWire(t)
	v2Reencoded, err := hostile.Encode(storedHostileToSave(mustHostileDecode(t, v2)))
	if err != nil {
		t.Fatalf("re-encode v2 fixture: %v", err)
	}
	v1Reencoded, err := hostile.Encode(storedHostileToSave(mustHostileDecode(t, v1)))
	if err != nil {
		t.Fatalf("re-encode v1 fixture: %v", err)
	}
	canonical := buildHostileCandidate(t, hostileEncodeV2ExactID, "encode", hostileVersionV2, v2, args, v2)
	capacity := uint32(len(canonical.Encoded) - 1)
	capacityCase := buildHostileCandidate(t, hostileEncodeCapacityMinusID, "encode", hostileVersionV2, v2, hostileArgumentsJSON(&capacity), nil)

	positive := []hostileCandidate{
		buildHostileCandidate(t, hostileDecodeV1FixtureID, "decode", hostileVersionV1, v1, args, nil),
		buildHostileCandidate(t, hostileDecodeV2FixtureID, "decode", hostileVersionV2, v2, args, nil),
		buildHostileCandidate(t, hostileDecodeEmptyID, "decode", hostileVersionV2, emptyWire, args, nil),
		buildHostileCandidate(t, hostileDecodeMaxRecordsID, "decode", hostileVersionV2, maxWire, args, nil),
		buildHostileCandidate(t, hostileDecodeYMinID, "decode", hostileVersionV2, hostileYBoundaryWire(t, float32(core.MinY)), args, nil),
		buildHostileCandidate(t, hostileDecodeYMaxID, "decode", hostileVersionV2, hostileYBoundaryWire(t, 319.5), args, nil),
		buildHostileCandidate(t, hostileDecodeCooldown20ID, "decode", hostileVersionV2, hostileCooldown20Wire(t), args, nil),
		buildHostileCandidate(t, hostileDecodeCount65ID, "decode", hostileVersionV2, hostileCount65HeaderWire(), args, nil),
		buildHostileCandidate(t, hostileDecodeV1TruncatedID, "decode", hostileVersionV1, v1[:len(v1)-1], args, nil),
		canonical,
		buildHostileCandidate(t, hostileEncodeV1ReencodeID, "encode", hostileVersionV2, v1, args, v1Reencoded),
		buildHostileCandidate(t, hostileEncodeEmptyID, "encode", hostileVersionV2, emptyWire, args, emptyWire),
		buildHostileCandidate(t, hostileEncodeMaxRecordsID, "encode", hostileVersionV2, maxWire, args, maxWire),
		buildHostileCandidate(t, hostileEncodeUnsortedID, "encode", hostileVersionV2, v2, args, v2Reencoded),
		capacityCase,
		buildHostileCandidate(t, hostileEncodeCount65ID, "encode", hostileVersionV2, v2, args, nil),
	}
	out := append(positive, hostileCorruptCandidates(t, valid)...)
	return out
}

func mustHostileDecode(t *testing.T, wire []byte) hostile.StoredHostileMobs {
	stored, err := hostile.Decode(wire)
	if err != nil {
		t.Fatalf("decode: %v", err)
	}
	return stored
}

func hostileSelection(t *testing.T, root string, candidates []hostileCandidate) StorageSelection {
	cases := make([]CaseSpec, 0, len(candidates))
	for _, candidate := range candidates {
		cases = append(cases, candidate.Spec)
	}
	sources := []SourceSpec{
		{Path: hostileCodecSourceRel},
		{Path: hostileProducerTestRel},
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
		ProducerID: hostileProducerID,
		Cases:      cases,
		Sources:    sources,
		Routes:     hostileRoutes(),
	}
}

func hostileRunnerManifest(t *testing.T, root string, candidates []hostileCandidate) Inventory {
	t.Helper()
	base, err := LoadInventory(filepath.Join(root, filepath.FromSlash(InventoryRelPath)))
	if err != nil {
		t.Fatalf("load frozen manifest: %v", err)
	}
	selection := hostileSelection(t, root, candidates)
	merged := base
	hostileCases := append([]CaseSpec(nil), selection.Cases...)
	sort.Slice(hostileCases, func(i, j int) bool { return hostileCases[i].ID < hostileCases[j].ID })
	merged.Cases = hostileCases
	caseIDs := make([]string, 0, len(hostileCases))
	for _, c := range hostileCases {
		caseIDs = append(caseIDs, c.ID)
	}
	for index := range merged.Families {
		if merged.Families[index].ID != hostileFamily {
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

func hostileScratchRoot(t *testing.T, candidates []hostileCandidate) string {
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

func hostileObservation(t *testing.T, observations []ExecutedObservation, id string) ExecutedObservation {
	t.Helper()
	for _, obs := range observations {
		if obs.CaseID == id {
			return obs
		}
	}
	t.Fatalf("no observation for case %s", id)
	return ExecutedObservation{}
}

func exportHostileSelectionCandidate(t *testing.T, root string, candidates []hostileCandidate) string {
	t.Helper()
	if strings.TrimSpace(os.Getenv(runtimeOracleExportDirEnv)) == "" {
		return ""
	}
	selection := hostileSelection(t, root, candidates)
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
	exportRoot, err := exportGeneratedAssets(root, strings.TrimSpace(os.Getenv(runtimeOracleExportDirEnv)), hostileProducerID, assets)
	if err != nil {
		t.Fatalf("export hostile selection: %v", err)
	}
	return exportRoot
}

func TestStorageHostileArgumentsValidate(t *testing.T) {
	if err := validateStorageArguments(hostileFamily, "decode", hostileArgumentsJSON(nil)); err != nil {
		t.Fatalf("validate hostile decode arguments: %v", err)
	}
	if err := validateStorageArguments(hostileFamily, "encode", hostileArgumentsJSON(nil)); err != nil {
		t.Fatalf("validate hostile encode arguments: %v", err)
	}
}

func TestStorageHostileProducerExecutesEveryCase(t *testing.T) {
	root := mustRepoRoot(t)
	candidates := hostileCandidates(t)
	const wantCases = 50
	if len(candidates) != wantCases {
		t.Fatalf("candidate count = %d, want %d", len(candidates), wantCases)
	}
	manifest := hostileRunnerManifest(t, root, candidates)
	staged := hostileScratchRoot(t, candidates)

	observations, err := RunStorageCases(staged, manifest, hostileCorpusRoutes())
	if err != nil {
		t.Fatalf("RunStorageCases: %v", err)
	}
	if len(observations) != len(candidates) {
		t.Fatalf("produced %d observations, want %d", len(observations), len(candidates))
	}
	for _, candidate := range candidates {
		obs := hostileObservation(t, observations, candidate.Spec.ID)
		got, err := outcomeToStorageSave(obs.Outcome)
		if err != nil {
			t.Fatalf("case %s: %v", candidate.Spec.ID, err)
		}
		if !storageSaveOutcomesEqual(got, candidate.Expect) {
			t.Fatalf("case %s produced %#v, want %#v", candidate.Spec.ID, got, candidate.Expect)
		}
	}
}

func TestStorageHostileFutureVersionCategory(t *testing.T) {
	for _, candidate := range hostileCandidates(t) {
		if candidate.Spec.ID != hostileDecodeInvalidVersionFutureID {
			continue
		}
		if candidate.Expect.Kind != "error" || candidate.Expect.Category != "future_version" {
			t.Fatalf("future schema case outcome %#v, want future_version error", candidate.Expect)
		}
		return
	}
	t.Fatal("missing invalid-version-future case")
}

func TestStorageHostileHasTargetDigestMutationFailsComparison(t *testing.T) {
	root := mustRepoRoot(t)
	v2 := readHostileFixture(t, root, 2)
	stored := mustHostileDecode(t, v2)
	stale := stored
	for index := range stale.Records {
		if stale.Records[index].HasTarget {
			stale.Records[index].HasTarget = false
			break
		}
	}
	spec := CaseSpec{
		ID: hostileDecodeV2FixtureID, Family: hostileFamily, Version: hostileVersionV2,
		Operation: "decode", Arguments: hostileArgumentsJSON(nil), InputFormat: "binary",
		Checkpoints: []string{"0"}, RustConsumer: storageConsumerName,
	}
	outcome, _, err := runHostileDecode(spec, v2)
	if err != nil {
		t.Fatalf("runHostileDecode: %v", err)
	}
	got, err := outcomeToStorageSave(outcome)
	if err != nil {
		t.Fatal(err)
	}
	staleExpect := storageSaveOutcome{
		Kind: "ok", Category: "save", ValueSHA256: storageValueSHA256(hostileStoredValueTree(stale)),
	}
	if storageSaveOutcomesEqual(got, staleExpect) {
		t.Fatal("has_target mutation still matches stale expected digest")
	}
}

func TestStorageHostileKindDigestMutationFailsComparison(t *testing.T) {
	root := mustRepoRoot(t)
	v2 := readHostileFixture(t, root, 2)
	stored := mustHostileDecode(t, v2)
	stale := stored
	for index := range stale.Records {
		if stale.Records[index].Kind == 1 {
			stale.Records[index].Kind = 0
			break
		}
	}
	spec := CaseSpec{
		ID: hostileDecodeV2FixtureID, Family: hostileFamily, Version: hostileVersionV2,
		Operation: "decode", Arguments: hostileArgumentsJSON(nil), InputFormat: "binary",
		Checkpoints: []string{"0"}, RustConsumer: storageConsumerName,
	}
	outcome, _, err := runHostileDecode(spec, v2)
	if err != nil {
		t.Fatalf("runHostileDecode: %v", err)
	}
	got, err := outcomeToStorageSave(outcome)
	if err != nil {
		t.Fatal(err)
	}
	staleExpect := storageSaveOutcome{
		Kind: "ok", Category: "save", ValueSHA256: storageValueSHA256(hostileStoredValueTree(stale)),
	}
	if storageSaveOutcomesEqual(got, staleExpect) {
		t.Fatal("kind mutation still matches stale expected digest")
	}
}

func TestStorageHostileExportUnsetWritesNothing(t *testing.T) {
	root := mustRepoRoot(t)
	t.Setenv(runtimeOracleExportDirEnv, "")
	before, err := os.ReadDir(root)
	if err != nil {
		t.Fatal(err)
	}
	_ = exportHostileSelectionCandidate(t, root, hostileCandidates(t))
	after, err := os.ReadDir(root)
	if err != nil {
		t.Fatal(err)
	}
	if len(before) != len(after) {
		t.Fatal("unset export must not mutate repository tree")
	}
}

func TestStorageHostileCandidatesExportForReview(t *testing.T) {
	root := mustRepoRoot(t)
	t.Setenv(runtimeOracleExportDirEnv, t.TempDir())
	child := exportHostileSelectionCandidate(t, root, hostileCandidates(t))
	if child == "" {
		t.Fatal("export root unset after explicit env")
	}
	if _, err := readStorageSelection(child); err != nil {
		t.Fatalf("reload exported selection: %v", err)
	}
}

func TestStorageHostileExportToPinnedDirectory(t *testing.T) {
	root := mustRepoRoot(t)
	exportRoot := hostileExportDir
	producerChild := filepath.Join(exportRoot, filepath.FromSlash("runtime-oracle/storage-hostile"))
	if _, err := os.Lstat(producerChild); err == nil {
		t.Skip("pinned producer child already exists; reviewed export candidate preserved")
	} else if !os.IsNotExist(err) {
		t.Fatalf("stat pinned producer child: %v", err)
	}
	t.Setenv(runtimeOracleExportDirEnv, exportRoot)
	child := exportHostileSelectionCandidate(t, root, hostileCandidates(t))
	if child == "" {
		t.Fatal("export root unset after explicit env")
	}
	if _, err := readStorageSelection(child); err != nil {
		t.Fatalf("reload exported selection: %v", err)
	}
}

