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
	"sort"
	"strconv"
	"strings"
	"testing"

	"github.com/channing771/mornlea/packages/server/storage/companion"
	"github.com/channing771/mornlea/packages/server/storage/storagedef"
	sharedcompanion "github.com/channing771/mornlea/packages/shared/companion"
	"github.com/channing771/mornlea/packages/shared/core"
)

const (
	companionFamily          = "save.companion"
	companionProducerID      = "runtime-oracle/storage-companion"
	companionCorpusRelDir    = "testdata/runtime-migration/cases/storage/companion"
	companionProducerTestRel = "packages/tools/cmd/runtime-oracle/storage_companion_test.go"
	companionCodecSourceRel  = "packages/server/storage/companion/companion_codec.go"
	companionExportDir       = "/tmp/runtime-oracle-companion-4.6a"

	companionHeaderLength = 32
	companionRecordLength = 221

	companionLegacyFlagHasTask    = 1 << 0
	companionLegacyFlagHasFIFO    = 1 << 1
	companionLegacyFlagHasSummary = 1 << 2

	companionDecodeV1FixtureID              = companionFamily + "/1/decode/v1-fixture"
	companionDecodeV1TruncatedPayloadID     = companionFamily + "/1/decode/truncated-payload"
	companionDecodeV2FixtureID              = companionFamily + "/2/decode/v2-fixture"
	companionDecodeV2CorruptCRCID           = companionFamily + "/2/decode/corrupt-crc"
	companionDecodeV3FixtureID              = companionFamily + "/3/decode/v3-fixture"
	companionDecodeV3InvalidVersionZeroID   = companionFamily + "/3/decode/invalid-version-zero"
	companionDecodeV4FixtureID              = companionFamily + "/4/decode/v4-fixture"
	companionDecodeV4InvalidVersionFutureID = companionFamily + "/4/decode/invalid-version-future"
)

type companionCaseArguments struct{}

type companionCandidate struct {
	Spec    CaseSpec
	Assets  map[string][]byte
	Expect  storageSaveOutcome
	Encoded []byte
}

func companionArgumentsJSON() json.RawMessage {
	raw, err := json.Marshal(companionCaseArguments{})
	if err != nil {
		panic(err)
	}
	return raw
}

func parseCompanionArguments(c CaseSpec) (companionCaseArguments, error) {
	var args companionCaseArguments
	dec := json.NewDecoder(bytes.NewReader(c.Arguments))
	dec.DisallowUnknownFields()
	if err := dec.Decode(&args); err != nil {
		return companionCaseArguments{}, fmt.Errorf("decode arguments: %w", err)
	}
	var trailing any
	if err := dec.Decode(&trailing); err != io.EOF {
		return companionCaseArguments{}, fmt.Errorf("arguments carry trailing content")
	}
	return args, nil
}

func companionSchemaVersion(input []byte) (uint32, error) {
	if len(input) < 12 {
		return 0, fmt.Errorf("input shorter than schema header")
	}
	return binary.LittleEndian.Uint32(input[8:12]), nil
}

func companionStorageErrorCategory(err error) (string, bool) {
	switch {
	case errors.Is(err, storagedef.ErrCorrupt):
		return "corrupt", true
	case errors.Is(err, storagedef.ErrFutureVersion):
		return "future_version", true
	default:
		return "", false
	}
}

func companionIdentityBytes(id companion.Identity) storageValueNode {
	return storageValueBytes(id[:])
}

func companionItemStackValue(stack core.ItemStack) storageValueNode {
	return storageValueObject(map[string]storageValueNode{
		"count":      storageValueUnsigned(uint64(stack.Count)),
		"durability": storageValueUnsigned(uint64(stack.Durability)),
		"item":       storageValueUnsigned(uint64(stack.Item)),
	})
}

func companionInventoryValue(inventory core.Inventory) storageValueNode {
	hotbarSlots := make([]storageValueNode, 0, len(inventory.Hotbar.Slots))
	for _, slot := range inventory.Hotbar.Slots {
		hotbarSlots = append(hotbarSlots, companionItemStackValue(slot))
	}
	backpack := make([]storageValueNode, 0, len(inventory.Backpack))
	for _, slot := range inventory.Backpack {
		backpack = append(backpack, companionItemStackValue(slot))
	}
	return storageValueObject(map[string]storageValueNode{
		"backpack": storageValueArray(backpack),
		"hotbar": storageValueObject(map[string]storageValueNode{
			"selected": storageValueUnsigned(uint64(inventory.Hotbar.Selected)),
			"slots":    storageValueArray(hotbarSlots),
		}),
	})
}

func companionBodyValue(body companion.Body) storageValueNode {
	return storageValueObject(map[string]storageValueNode{
		"id":        companionIdentityBytes(companion.Identity(body.ID)),
		"dimension": storageValueSigned(int64(body.Dimension)),
		"position": storageValueArray([]storageValueNode{
			storageValueF32(body.Position[0]),
			storageValueF32(body.Position[1]),
			storageValueF32(body.Position[2]),
		}),
		"yaw":       storageValueF32(body.Yaw),
		"pitch":     storageValueF32(body.Pitch),
		"inventory": companionInventoryValue(body.Inventory),
	})
}

func companionPlanStepValue(step sharedcompanion.PlanStep) storageValueNode {
	return storageValueObject(map[string]storageValueNode{
		"block":     storageValueUnsigned(uint64(step.Block)),
		"kind":      storageValueUnsigned(uint64(step.Kind)),
		"player_id": storageValueBytes(step.PlayerID[:]),
		"x":         storageValueSigned(int64(step.X)),
		"y":         storageValueSigned(int64(step.Y)),
		"z":         storageValueSigned(int64(step.Z)),
	})
}

func companionTaskValue(task companion.StoredCompanionTask) storageValueNode {
	steps := make([]storageValueNode, 0, len(task.PlanSteps))
	for _, step := range task.PlanSteps {
		steps = append(steps, companionPlanStepValue(step))
	}
	return storageValueObject(map[string]storageValueNode{
		"command":         storageValueUTF8(task.Command),
		"deadline_ticks":  storageValueUnsigned(task.DeadlineTicks),
		"fail_reason":     storageValueUnsigned(uint64(task.FailReason)),
		"plan_steps":      storageValueArray(steps),
		"start_tick":      storageValueUnsigned(task.StartTick),
		"state":           storageValueUnsigned(uint64(task.State)),
		"step_index":      storageValueSigned(int64(task.StepIndex)),
	})
}

func companionQueueValue(queue companion.StoredCompanionQueue) storageValueNode {
	pending := make([]storageValueNode, 0, len(queue.Pending))
	for _, entry := range queue.Pending {
		pending = append(pending, storageValueUTF8(entry))
	}
	var current storageValueNode = storageValueNull()
	if queue.HasCurrent {
		current = companionTaskValue(queue.Current)
	}
	return storageValueObject(map[string]storageValueNode{
		"current":   current,
		"has_current": storageValueBool(queue.HasCurrent),
		"id":        companionIdentityBytes(companion.Identity(queue.ID)),
		"pending":   storageValueArray(pending),
		"summary":   storageValueUTF8(queue.Summary),
	})
}

func companionLifecycleValue(lifecycle companion.StoredCompanionLifecycle) storageValueNode {
	return storageValueObject(map[string]storageValueNode{
		"active":                  storageValueBool(lifecycle.Active),
		"id":                      companionIdentityBytes(companion.Identity(lifecycle.ID)),
		"memory_epoch":            storageValueUnsigned(lifecycle.MemoryEpoch),
		"memory_operation_id":     companionIdentityBytes(lifecycle.MemoryOperationID),
		"memory_revision":         storageValueUnsigned(lifecycle.MemoryRevision),
		"summary":                 storageValueUTF8(lifecycle.Summary),
		"tombstone_operation_id":  companionIdentityBytes(lifecycle.TombstoneOperationID),
	})
}

func companionStoredValueTree(stored companion.StoredCompanions) storageValueNode {
	records := make([]storageValueNode, 0, len(stored.Records))
	for _, record := range stored.Records {
		records = append(records, companionBodyValue(record))
	}
	lifecycles := make([]storageValueNode, 0, len(stored.Lifecycles))
	for _, lifecycle := range stored.Lifecycles {
		lifecycles = append(lifecycles, companionLifecycleValue(lifecycle))
	}
	queues := make([]storageValueNode, 0, len(stored.Queues))
	for _, queue := range stored.Queues {
		queues = append(queues, companionQueueValue(queue))
	}
	return storageValueObject(map[string]storageValueNode{
		"agent_namespace_id": companionIdentityBytes(stored.AgentNamespaceID),
		"lifecycles":         storageValueArray(lifecycles),
		"queues":             storageValueArray(queues),
		"records":            storageValueArray(records),
		"revision":           storageValueUnsigned(stored.Revision),
		"source_schema":      storageValueUnsigned(uint64(stored.SourceSchema)),
	})
}

func companionResealCRC(wire []byte) {
	table := crc32.MakeTable(crc32.Castagnoli)
	hasher := crc32.New(table)
	_, _ = hasher.Write(wire[8:28])
	if len(wire) > companionHeaderLength {
		_, _ = hasher.Write(wire[companionHeaderLength:])
	}
	binary.LittleEndian.PutUint32(wire[28:32], hasher.Sum32())
}

func companionTruncatedWire(wire []byte, dropTail int) []byte {
	if dropTail <= 0 || dropTail >= len(wire) {
		return bytes.Clone(wire)
	}
	return bytes.Clone(wire[:len(wire)-dropTail])
}

func companionCorruptCRCWire(wire []byte) []byte {
	out := bytes.Clone(wire)
	if len(out) >= 32 {
		out[28] ^= 0xff
	}
	return out
}

func companionWireWithSchema(wire []byte, schema uint32) []byte {
	out := bytes.Clone(wire)
	if len(out) >= 12 {
		binary.LittleEndian.PutUint32(out[8:12], schema)
	}
	return out
}

func readCompanionLegacyFixture(t *testing.T, root string, schema int) []byte {
	t.Helper()
	rel := fmt.Sprintf("packages/server/storage/companion/testdata/companions-v%d.bin", schema)
	full := filepath.Join(root, filepath.FromSlash(rel))
	data, err := os.ReadFile(full)
	if err != nil {
		t.Fatalf("read companion v%d fixture: %v", schema, err)
	}
	got, err := companionSchemaVersion(data)
	if err != nil {
		t.Fatalf("companion v%d fixture schema: %v", schema, err)
	}
	if got != uint32(schema) {
		t.Fatalf("companion v%d fixture schema u32 = %d, want %d", schema, got, schema)
	}
	return data
}

func appendCompanionPlanStep(dst []byte, step sharedcompanion.PlanStep) []byte {
	dst = append(dst, byte(step.Kind))
	switch step.Kind {
	case sharedcompanion.PlanStepFollow:
		dst = append(dst, step.PlayerID[:]...)
	default:
		dst = binary.LittleEndian.AppendUint32(dst, uint32(step.X))
		dst = binary.LittleEndian.AppendUint32(dst, uint32(step.Y))
		dst = binary.LittleEndian.AppendUint32(dst, uint32(step.Z))
		if step.Kind == sharedcompanion.PlanStepPlace {
			dst = binary.LittleEndian.AppendUint16(dst, uint16(step.Block))
		}
	}
	return dst
}

func appendCompanionTask(dst []byte, task companion.StoredCompanionTask) []byte {
	dst = binary.LittleEndian.AppendUint16(dst, uint16(len(task.Command)))
	dst = append(dst, task.Command...)
	dst = binary.LittleEndian.AppendUint16(dst, uint16(len(task.PlanSteps)))
	for _, step := range task.PlanSteps {
		dst = appendCompanionPlanStep(dst, step)
	}
	dst = binary.LittleEndian.AppendUint32(dst, uint32(task.StepIndex))
	dst = append(dst, byte(task.State), byte(task.FailReason))
	dst = binary.LittleEndian.AppendUint64(dst, task.StartTick)
	dst = binary.LittleEndian.AppendUint64(dst, task.DeadlineTicks)
	return dst
}

func appendCompanionFIFO(dst []byte, pending []string) []byte {
	dst = binary.LittleEndian.AppendUint16(dst, uint16(len(pending)))
	for _, command := range pending {
		dst = binary.LittleEndian.AppendUint16(dst, uint16(len(command)))
		dst = append(dst, command...)
	}
	return dst
}

func appendCompanionBody(dst []byte, body companion.Body) []byte {
	dst = append(dst, body.ID[:]...)
	dst = binary.LittleEndian.AppendUint32(dst, uint32(body.Dimension))
	for _, value := range body.Position {
		dst = binary.LittleEndian.AppendUint32(dst, math.Float32bits(value))
	}
	dst = binary.LittleEndian.AppendUint32(dst, math.Float32bits(body.Yaw))
	dst = binary.LittleEndian.AppendUint32(dst, math.Float32bits(body.Pitch))
	dst = append(dst, body.Inventory.Hotbar.Selected)
	for _, stack := range body.Inventory.Hotbar.Slots {
		dst = binary.LittleEndian.AppendUint16(dst, uint16(stack.Item))
		dst = append(dst, stack.Count)
		dst = binary.LittleEndian.AppendUint16(dst, stack.Durability)
	}
	for _, stack := range body.Inventory.Backpack {
		dst = binary.LittleEndian.AppendUint16(dst, uint16(stack.Item))
		dst = append(dst, stack.Count)
		dst = binary.LittleEndian.AppendUint16(dst, stack.Durability)
	}
	return dst
}

func appendCompanionLegacyQueue(dst []byte, schema uint32, queue companion.StoredCompanionQueue) []byte {
	var flags uint8
	if queue.HasCurrent {
		flags |= companionLegacyFlagHasTask
	}
	if len(queue.Pending) != 0 {
		flags |= companionLegacyFlagHasFIFO
	}
	if schema >= 4 && queue.Summary != "" {
		flags |= companionLegacyFlagHasSummary
	}
	dst = append(dst, flags)
	if flags&companionLegacyFlagHasTask != 0 {
		dst = appendCompanionTask(dst, queue.Current)
	}
	if flags&companionLegacyFlagHasFIFO != 0 {
		dst = appendCompanionFIFO(dst, queue.Pending)
	}
	if flags&companionLegacyFlagHasSummary != 0 {
		dst = binary.LittleEndian.AppendUint16(dst, uint16(len(queue.Summary)))
		dst = append(dst, queue.Summary...)
	}
	return dst
}

func encodeCompanionLegacyWire(
	schema uint32,
	revision uint64,
	records []companion.Body,
	perRecord []companion.StoredCompanionQueue,
) ([]byte, error) {
	if schema == 1 {
		perRecord = nil
	}
	payload := make([]byte, 0, len(records)*companionRecordLength)
	for index, body := range records {
		payload = appendCompanionBody(payload, body)
		if schema == 1 {
			continue
		}
		queue := companion.StoredCompanionQueue{}
		if index < len(perRecord) {
			queue = perRecord[index]
		}
		if queue.HasCurrent || len(queue.Pending) != 0 || queue.Summary != "" {
			payload = appendCompanionLegacyQueue(payload, schema, queue)
		} else {
			payload = append(payload, 0)
		}
	}
	if schema == 1 && len(payload) != len(records)*companionRecordLength {
		return nil, fmt.Errorf("v1 payload length mismatch")
	}
	header := make([]byte, 0, companionHeaderLength+len(payload))
	header = append(header, 'M', 'C', 'A', 'I')
	header = binary.LittleEndian.AppendUint32(header, 1)
	header = binary.LittleEndian.AppendUint32(header, schema)
	header = binary.LittleEndian.AppendUint64(header, revision)
	header = binary.LittleEndian.AppendUint32(header, uint32(len(records)))
	header = binary.LittleEndian.AppendUint32(header, uint32(len(payload)))
	header = binary.LittleEndian.AppendUint32(header, 0)
	header = append(header, payload...)
	companionResealCRC(header)
	return header, nil
}

func companionV3TwoDistinctQueuesWire(t *testing.T, root string) []byte {
	t.Helper()
	v3 := readCompanionLegacyFixture(t, root, 3)
	v4 := readCompanionLegacyFixture(t, root, 4)
	decV3, err := companion.Decode(v3)
	if err != nil {
		t.Fatalf("decode v3 fixture: %v", err)
	}
	decV4, err := companion.Decode(v4)
	if err != nil {
		t.Fatalf("decode v4 fixture: %v", err)
	}
	if len(decV3.Queues) != 1 || len(decV4.Queues) < 2 {
		t.Fatalf("fixture queue shape changed")
	}
	perRecord := []companion.StoredCompanionQueue{decV3.Queues[0], decV4.Queues[1]}
	wire, err := encodeCompanionLegacyWire(3, decV3.Revision, decV3.Records, perRecord)
	if err != nil {
		t.Fatalf("encode v3 dual-queue wire: %v", err)
	}
	decoded, err := companion.Decode(wire)
	if err != nil {
		t.Fatalf("decode constructed v3 dual-queue wire: %v", err)
	}
	if len(decoded.Queues) != 2 {
		t.Fatalf("constructed wire queues = %d, want 2", len(decoded.Queues))
	}
	return wire
}

func runCompanionDecode(c CaseSpec, input []byte) (Outcome, []byte, error) {
	if _, err := parseCompanionArguments(c); err != nil {
		return Outcome{}, nil, fmt.Errorf("runtime-oracle: case %s: %w", c.ID, err)
	}
	schema, err := companionSchemaVersion(input)
	if err != nil {
		return Outcome{}, nil, fmt.Errorf("runtime-oracle: case %s: %w", c.ID, err)
	}
	wantVersion, err := strconv.ParseUint(c.Version, 10, 32)
	if err != nil {
		return Outcome{}, nil, fmt.Errorf("runtime-oracle: case %s: invalid case version %q", c.ID, c.Version)
	}

	stored, err := companion.Decode(input)
	if err != nil {
		category, ok := companionStorageErrorCategory(err)
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
	digest := storageValueSHA256(companionStoredValueTree(stored))
	return Outcome{Kind: "ok", Category: "save", Fields: map[string]any{"value_sha256": digest}}, nil, nil
}

func buildCompanionCandidate(
	t *testing.T,
	id, operation, caseVersion string,
	input []byte,
	args json.RawMessage,
) companionCandidate {
	t.Helper()
	spec := CaseSpec{
		ID:           id,
		Family:       companionFamily,
		Version:      caseVersion,
		Operation:    operation,
		Arguments:    args,
		InputFormat:  "binary",
		Checkpoints:  []string{"0"},
		RustConsumer: storageConsumerName,
	}
	outcome, _, err := runCompanionDecode(spec, input)
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
	inputRel := filepath.ToSlash(filepath.Join(companionCorpusRelDir, stem+".input.bin"))
	expectedRel := filepath.ToSlash(filepath.Join(companionCorpusRelDir, stem+".expected.json"))
	assets := map[string][]byte{
		inputRel:    input,
		expectedRel: expectedBytes,
	}
	spec.Input = AssetRef{Path: inputRel, SHA256: digestOf(t, input)}
	spec.Expected = AssetRef{Path: expectedRel, SHA256: digestOf(t, expectedBytes)}
	return companionCandidate{Spec: spec, Assets: assets, Expect: saveOutcome}
}

func companionLegacyCandidates(t *testing.T) []companionCandidate {
	t.Helper()
	root := mustRepoRoot(t)
	args := companionArgumentsJSON()
	v1 := readCompanionLegacyFixture(t, root, 1)
	v2 := readCompanionLegacyFixture(t, root, 2)
	v3 := readCompanionLegacyFixture(t, root, 3)
	v4 := readCompanionLegacyFixture(t, root, 4)

	return []companionCandidate{
		buildCompanionCandidate(t, companionDecodeV1FixtureID, "decode", "1", v1, args),
		buildCompanionCandidate(t, companionDecodeV1TruncatedPayloadID, "decode", "1", companionTruncatedWire(v1, 1), args),
		buildCompanionCandidate(t, companionDecodeV2FixtureID, "decode", "2", v2, args),
		buildCompanionCandidate(t, companionDecodeV2CorruptCRCID, "decode", "2", companionCorruptCRCWire(v2), args),
		buildCompanionCandidate(t, companionDecodeV3FixtureID, "decode", "3", v3, args),
		buildCompanionCandidate(t, companionDecodeV3InvalidVersionZeroID, "decode", "3", companionWireWithSchema(v3, 0), args),
		buildCompanionCandidate(t, companionDecodeV4FixtureID, "decode", "4", v4, args),
		buildCompanionCandidate(t, companionDecodeV4InvalidVersionFutureID, "decode", "4", companionWireWithSchema(v4, 10), args),
	}
}

func companionLegacyRoutes() []ConsumerRoute {
	routes := make([]ConsumerRoute, 0, 4)
	for _, version := range []string{"1", "2", "3", "4"} {
		routes = append(routes, ConsumerRoute{
			FamilyID:  companionFamily,
			Version:   version,
			Operation: "decode",
		})
	}
	return routes
}

func companionCorpusRoutes() map[ConsumerRoute]GoOperation {
	routes := make(map[ConsumerRoute]GoOperation, len(companionLegacyRoutes()))
	for _, route := range companionLegacyRoutes() {
		routes[route] = runCompanionDecode
	}
	return routes
}

func companionSelection(t *testing.T, root string, candidates []companionCandidate, routes []ConsumerRoute) StorageSelection {
	t.Helper()
	cases := make([]CaseSpec, 0, len(candidates))
	for _, candidate := range candidates {
		cases = append(cases, candidate.Spec)
	}
	sources := []SourceSpec{
		{Path: companionCodecSourceRel},
		{Path: companionProducerTestRel},
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
		ProducerID: companionProducerID,
		Cases:      cases,
		Sources:    sources,
		Routes:     routes,
	}
}

func companionScratchRoot(t *testing.T, candidates []companionCandidate) string {
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

func inventoryFromCompanionSelection(t *testing.T, root string, selection StorageSelection) Inventory {
	t.Helper()
	base, err := LoadInventory(filepath.Join(root, filepath.FromSlash(InventoryRelPath)))
	if err != nil {
		t.Fatalf("load frozen manifest: %v", err)
	}
	merged := base
	companionCases := append([]CaseSpec(nil), selection.Cases...)
	sort.Slice(companionCases, func(i, j int) bool { return companionCases[i].ID < companionCases[j].ID })
	merged.Cases = companionCases
	caseIDs := make([]string, 0, len(companionCases))
	for _, c := range companionCases {
		caseIDs = append(caseIDs, c.ID)
	}
	for index := range merged.Families {
		if merged.Families[index].ID != companionFamily {
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

func companionRunnerManifest(t *testing.T, root string, candidates []companionCandidate) Inventory {
	t.Helper()
	selection := companionSelection(t, root, candidates, companionLegacyRoutes())
	merged := inventoryFromCompanionSelection(t, root, selection)
	wantIDs := make(map[string]bool, len(candidates))
	for _, candidate := range candidates {
		wantIDs[candidate.Spec.ID] = true
	}
	var companionCases []CaseSpec
	for _, c := range merged.Cases {
		if c.Family == companionFamily && wantIDs[c.ID] {
			companionCases = append(companionCases, c)
		}
	}
	sort.Slice(companionCases, func(i, j int) bool { return companionCases[i].ID < companionCases[j].ID })
	caseIDs := make([]string, 0, len(companionCases))
	for _, c := range companionCases {
		caseIDs = append(caseIDs, c.ID)
	}
	for index := range merged.Families {
		if merged.Families[index].ID == companionFamily {
			merged.Families[index].Cases = caseIDs
		} else {
			merged.Families[index].Cases = nil
		}
	}
	merged.Cases = companionCases
	return merged
}

func companionObservation(t *testing.T, observations []ExecutedObservation, id string) ExecutedObservation {
	t.Helper()
	for _, obs := range observations {
		if obs.CaseID == id {
			return obs
		}
	}
	t.Fatalf("no observation for case %s", id)
	return ExecutedObservation{}
}

func exportCompanionSelectionCandidate(t *testing.T, root string, candidates []companionCandidate) string {
	t.Helper()
	if strings.TrimSpace(os.Getenv(runtimeOracleExportDirEnv)) == "" {
		return ""
	}
	selection := companionSelection(t, root, candidates, companionLegacyRoutes())
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
				t.Fatalf("companion export: duplicate asset path %s", relative)
			}
			seen[relative] = struct{}{}
			assets = append(assets, generatedAsset{RelativePath: relative, Data: data})
		}
	}
	exportRoot, err := exportGeneratedAssets(root, strings.TrimSpace(os.Getenv(runtimeOracleExportDirEnv)), companionProducerID, assets)
	if err != nil {
		t.Fatalf("export companion selection: %v", err)
	}
	return exportRoot
}

func TestStorageCompanionLegacyArgumentsValidate(t *testing.T) {
	if err := validateStorageArguments(companionFamily, "decode", companionArgumentsJSON()); err != nil {
		t.Fatalf("validate companion decode arguments: %v", err)
	}
}

func TestStorageCompanionLegacyProducerExecutesEveryCase(t *testing.T) {
	root := mustRepoRoot(t)
	candidates := companionLegacyCandidates(t)
	const wantCases = 8
	if len(candidates) != wantCases {
		t.Fatalf("candidate count = %d, want %d", len(candidates), wantCases)
	}
	manifest := companionRunnerManifest(t, root, candidates)
	staged := companionScratchRoot(t, candidates)

	observations, err := RunStorageCases(staged, manifest, companionCorpusRoutes())
	if err != nil {
		t.Fatalf("RunStorageCases: %v", err)
	}
	if len(observations) != len(candidates) {
		t.Fatalf("produced %d observations, want %d", len(observations), len(candidates))
	}
	for _, candidate := range candidates {
		obs := companionObservation(t, observations, candidate.Spec.ID)
		got, err := outcomeToStorageSave(obs.Outcome)
		if err != nil {
			t.Fatalf("case %s: %v", candidate.Spec.ID, err)
		}
		if !storageSaveOutcomesEqual(got, candidate.Expect) {
			t.Fatalf("case %s produced %#v, want %#v", candidate.Spec.ID, got, candidate.Expect)
		}
	}
}

func TestStorageCompanionLegacyQueueDigestMutationFailsComparison(t *testing.T) {
	root := mustRepoRoot(t)
	v4 := readCompanionLegacyFixture(t, root, 4)
	stored, err := companion.Decode(v4)
	if err != nil {
		t.Fatalf("decode v4: %v", err)
	}
	stale := stored
	if len(stale.Queues) > 0 && len(stale.Queues[0].Pending) > 0 {
		stale.Queues[0].Pending[0] = stale.Queues[0].Pending[0] + "x"
	}
	spec := CaseSpec{
		ID: companionDecodeV4FixtureID, Family: companionFamily, Version: "4",
		Operation: "decode", Arguments: companionArgumentsJSON(), InputFormat: "binary",
		Checkpoints: []string{"0"}, RustConsumer: storageConsumerName,
	}
	outcome, _, err := runCompanionDecode(spec, v4)
	if err != nil {
		t.Fatalf("runCompanionDecode: %v", err)
	}
	got, err := outcomeToStorageSave(outcome)
	if err != nil {
		t.Fatal(err)
	}
	staleExpect := storageSaveOutcome{
		Kind: "ok", Category: "save", ValueSHA256: storageValueSHA256(companionStoredValueTree(stale)),
	}
	if storageSaveOutcomesEqual(got, staleExpect) {
		t.Fatal("queue mutation still matches stale expected digest")
	}
}

func TestStorageCompanionLegacyInputSwapDigestMutationFailsComparison(t *testing.T) {
	root := mustRepoRoot(t)
	v3 := readCompanionLegacyFixture(t, root, 3)
	dual := companionV3TwoDistinctQueuesWire(t, root)
	spec := CaseSpec{
		ID: companionDecodeV3FixtureID, Family: companionFamily, Version: "3",
		Operation: "decode", Arguments: companionArgumentsJSON(), InputFormat: "binary",
		Checkpoints: []string{"0"}, RustConsumer: storageConsumerName,
	}
	outcome, _, err := runCompanionDecode(spec, v3)
	if err != nil {
		t.Fatalf("runCompanionDecode v3 fixture: %v", err)
	}
	want, err := outcomeToStorageSave(outcome)
	if err != nil {
		t.Fatal(err)
	}
	swapped, _, err := runCompanionDecode(spec, dual)
	if err != nil {
		t.Fatalf("runCompanionDecode dual-queue v3: %v", err)
	}
	got, err := outcomeToStorageSave(swapped)
	if err != nil {
		t.Fatal(err)
	}
	if storageSaveOutcomesEqual(got, want) {
		t.Fatal("swapped v3 dual-queue input still matches v3 fixture digest")
	}
}

func TestStorageCompanionLegacyExportUnsetWritesNothing(t *testing.T) {
	root := mustRepoRoot(t)
	t.Setenv(runtimeOracleExportDirEnv, "")
	_ = exportCompanionSelectionCandidate(t, root, companionLegacyCandidates(t))
}

func TestStorageCompanionLegacyCandidatesExportForReview(t *testing.T) {
	root := mustRepoRoot(t)
	exportRoot := filepath.Join(t.TempDir(), "companion-legacy-export-parent")
	t.Setenv(runtimeOracleExportDirEnv, exportRoot)
	child := exportCompanionSelectionCandidate(t, root, companionLegacyCandidates(t))
	if child == "" {
		t.Fatal("export returned empty path with RUNTIME_ORACLE_EXPORT_DIR set")
	}
	if _, err := os.Stat(filepath.Join(child, storageSelectionManifest)); err != nil {
		t.Fatalf("missing selection manifest: %v", err)
	}
}

func TestStorageCompanionLegacyExportToPinnedDirectory(t *testing.T) {
	root := mustRepoRoot(t)
	if _, err := os.Stat(companionExportDir); err == nil {
		t.Fatalf("pinned export child %s already exists", companionExportDir)
	}
	t.Setenv(runtimeOracleExportDirEnv, companionExportDir)
	child := exportCompanionSelectionCandidate(t, root, companionLegacyCandidates(t))
	if child == "" {
		t.Fatal("export returned empty path")
	}
	if _, err := os.Stat(filepath.Join(child, storageSelectionManifest)); err != nil {
		t.Fatalf("missing selection manifest under pinned export: %v", err)
	}
}
