package main

import (
	"bytes"
	"encoding/binary"
	"encoding/hex"
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

	"github.com/channing771/mornlea/packages/server/storage/player"
	"github.com/channing771/mornlea/packages/server/storage/storagedef"
	"github.com/channing771/mornlea/packages/shared/core"
)

const (
	playerFamily           = "save.player"
	playerVersion          = "9"
	playerProducerID       = "runtime-oracle/storage-player"
	playerCorpusRelDir     = "testdata/runtime-migration/cases/storage/player"
	playerProducerTestRel  = "packages/tools/cmd/runtime-oracle/storage_player_test.go"
	playerCodecSourceRel   = "packages/server/storage/player/player_codec.go"
	playerFixtureIDHex     = "00112233445546778899aabbccddeeff"
	playerV9FixtureRepoRel = "packages/server/storage/player/testdata/player-v9.bin"

	playerDecodeV9FixtureID        = playerFamily + "/" + playerVersion + "/decode/v9-fixture"
	playerDecodeV9RoundTripAltID   = playerFamily + "/" + playerVersion + "/decode/v9-roundtrip-alt"
	playerDecodeRawArmorTripleID   = playerFamily + "/" + playerVersion + "/decode/raw-armor-triple"
	playerDecodeAbsentRespawnID    = playerFamily + "/" + playerVersion + "/decode/absent-respawn-dirty-tail"
	playerDecodeExhaustionMaxID    = playerFamily + "/" + playerVersion + "/decode/exhaustion-maxi"
	playerEncodeV9CanonicalID      = playerFamily + "/" + playerVersion + "/encode/v9-canonical"
	playerEncodeCapacityMinusOneID = playerFamily + "/" + playerVersion + "/encode/capacity-minus-one"
)

type playerCaseArguments struct {
	RequestedPlayerID string  `json:"requested_player_id"`
	Capacity          *uint32 `json:"capacity,omitempty"`
}

type playerCandidate struct {
	Spec    CaseSpec
	Assets  map[string][]byte
	Expect  storageSaveOutcome
	Encoded []byte
}

func playerFixtureID() core.PlayerID {
	return core.PlayerID{
		0x00, 0x11, 0x22, 0x33, 0x44, 0x55, 0x46, 0x77,
		0x88, 0x99, 0xaa, 0xbb, 0xcc, 0xdd, 0xee, 0xff,
	}
}

func playerIDFromRequestedHex(text string) (core.PlayerID, error) {
	if !playerIDPattern.MatchString(text) {
		return core.PlayerID{}, fmt.Errorf("requested_player_id must be 32 lowercase hex digits")
	}
	var id core.PlayerID
	if _, err := hex.Decode(id[:], []byte(text)); err != nil {
		return core.PlayerID{}, fmt.Errorf("requested_player_id is not hexadecimal")
	}
	return id, nil
}

func playerArgumentsJSON(capacity *uint32) json.RawMessage {
	args := playerCaseArguments{RequestedPlayerID: playerFixtureIDHex}
	if capacity != nil {
		args.Capacity = capacity
	}
	raw, err := json.Marshal(args)
	if err != nil {
		panic(err)
	}
	return raw
}

func parsePlayerArguments(c CaseSpec) (playerCaseArguments, core.PlayerID, error) {
	var args playerCaseArguments
	dec := json.NewDecoder(bytes.NewReader(c.Arguments))
	dec.DisallowUnknownFields()
	if err := dec.Decode(&args); err != nil {
		return playerCaseArguments{}, core.PlayerID{}, fmt.Errorf("decode arguments: %w", err)
	}
	var trailing any
	if err := dec.Decode(&trailing); err != io.EOF {
		return playerCaseArguments{}, core.PlayerID{}, fmt.Errorf("arguments carry trailing content")
	}
	id, err := playerIDFromRequestedHex(args.RequestedPlayerID)
	if err != nil {
		return playerCaseArguments{}, core.PlayerID{}, err
	}
	return args, id, nil
}

func playerSchemaVersion(encoded []byte) (uint32, error) {
	if len(encoded) < 12 {
		return 0, fmt.Errorf("input shorter than schema header")
	}
	return binary.LittleEndian.Uint32(encoded[8:12]), nil
}

func playerStorageErrorCategory(err error) (string, bool) {
	switch {
	case errors.Is(err, storagedef.ErrCorrupt):
		return "corrupt", true
	case errors.Is(err, storagedef.ErrFutureVersion):
		return "future_version", true
	default:
		return "", false
	}
}

func itemStackValueTree(stack core.ItemStack) storageValueNode {
	return storageValueObject(map[string]storageValueNode{
		"count":      storageValueUnsigned(uint64(stack.Count)),
		"durability": storageValueUnsigned(uint64(stack.Durability)),
		"item":       storageValueUnsigned(uint64(stack.Item)),
	})
}

func playerLocationValueTree(loc player.PlayerLocation) storageValueNode {
	return storageValueObject(map[string]storageValueNode{
		"dimension": storageValueSigned(int64(loc.Dimension)),
		"position": storageValueArray([]storageValueNode{
			storageValueF32(loc.Position[0]),
			storageValueF32(loc.Position[1]),
			storageValueF32(loc.Position[2]),
		}),
	})
}

func playerInventoryValueTree(inventory core.Inventory) storageValueNode {
	hotbarSlots := make([]storageValueNode, 0, len(inventory.Hotbar.Slots))
	for _, slot := range inventory.Hotbar.Slots {
		hotbarSlots = append(hotbarSlots, itemStackValueTree(slot))
	}
	backpack := make([]storageValueNode, 0, len(inventory.Backpack))
	for _, slot := range inventory.Backpack {
		backpack = append(backpack, itemStackValueTree(slot))
	}
	return storageValueObject(map[string]storageValueNode{
		"backpack": storageValueArray(backpack),
		"hotbar": storageValueObject(map[string]storageValueNode{
			"selected": storageValueUnsigned(uint64(inventory.Hotbar.Selected)),
			"slots":    storageValueArray(hotbarSlots),
		}),
	})
}

func playerStoredValueTree(stored player.StoredPlayer) storageValueNode {
	var safe storageValueNode = storageValueNull()
	if stored.Safe != nil {
		safe = playerLocationValueTree(*stored.Safe)
	}
	armor := make([]storageValueNode, 0, len(stored.Armor))
	for _, slot := range stored.Armor {
		armor = append(armor, itemStackValueTree(slot))
	}
	return storageValueObject(map[string]storageValueNode{
		"armor":             storageValueArray(armor),
		"current":           playerLocationValueTree(stored.Current),
		"display_name":      storageValueUTF8(stored.DisplayName),
		"exhaustion_milli":  storageValueUnsigned(uint64(stored.ExhaustionMilli)),
		"health":            storageValueUnsigned(uint64(stored.Health)),
		"hunger":            storageValueUnsigned(uint64(stored.Hunger)),
		"inventory":         playerInventoryValueTree(stored.Inventory),
		"needs_rewrite":     storageValueBool(stored.NeedsRewrite),
		"pitch":             storageValueF32(stored.Pitch),
		"player_id":         storageValueBytes(stored.PlayerID[:]),
		"respawn_dimension": storageValueSigned(int64(stored.RespawnDimension)),
		"respawn_position": storageValueArray([]storageValueNode{
			storageValueF32(stored.RespawnPosition[0]),
			storageValueF32(stored.RespawnPosition[1]),
			storageValueF32(stored.RespawnPosition[2]),
		}),
		"respawn_present":  storageValueBool(stored.RespawnPresent),
		"revision":         storageValueUnsigned(stored.Revision),
		"safe":             safe,
		"saturation_milli": storageValueUnsigned(uint64(stored.SaturationMilli)),
		"yaw":              storageValueF32(stored.Yaw),
	})
}

func storedPlayerToSave(stored player.StoredPlayer) player.PlayerSave {
	return player.PlayerSave{
		PlayerID: stored.PlayerID, Revision: stored.Revision, DisplayName: stored.DisplayName,
		Current: stored.Current, Yaw: stored.Yaw, Pitch: stored.Pitch, Safe: stored.Safe,
		Inventory: stored.Inventory, Health: stored.Health,
		Hunger: stored.Hunger, SaturationMilli: stored.SaturationMilli, ExhaustionMilli: stored.ExhaustionMilli,
		RespawnPresent: stored.RespawnPresent, RespawnPosition: stored.RespawnPosition,
		RespawnDimension: stored.RespawnDimension, Armor: stored.Armor,
	}
}

func clonePlayerSave(save player.PlayerSave) player.PlayerSave {
	cloned := save
	if save.Safe != nil {
		safe := *save.Safe
		cloned.Safe = &safe
	}
	return cloned
}

func runPlayerDecode(c CaseSpec, input []byte) (Outcome, []byte, error) {
	_, wantID, err := parsePlayerArguments(c)
	if err != nil {
		return Outcome{}, nil, fmt.Errorf("runtime-oracle: case %s: %w", c.ID, err)
	}
	schema, err := playerSchemaVersion(input)
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

	stored, err := player.Decode(wantID, input)
	if err != nil {
		category, ok := playerStorageErrorCategory(err)
		if !ok {
			return Outcome{}, nil, fmt.Errorf("runtime-oracle: case %s: unclassified rejection: %w", c.ID, err)
		}
		return Outcome{Kind: "error", Category: category}, nil, nil
	}
	if uint64(schema) != wantVersion {
		return Outcome{}, nil, fmt.Errorf("runtime-oracle: case %s: accepted input schema %d, want %s", c.ID, schema, c.Version)
	}
	digest := storageValueSHA256(playerStoredValueTree(stored))
	return Outcome{Kind: "ok", Category: "save", Fields: map[string]any{"value_sha256": digest}}, nil, nil
}

func runPlayerEncode(c CaseSpec, input []byte) (Outcome, []byte, error) {
	args, wantID, err := parsePlayerArguments(c)
	if err != nil {
		return Outcome{}, nil, fmt.Errorf("runtime-oracle: case %s: %w", c.ID, err)
	}

	stored, err := player.Decode(wantID, input)
	if err != nil {
		category, ok := playerStorageErrorCategory(err)
		if !ok {
			return Outcome{}, nil, fmt.Errorf("runtime-oracle: case %s: unclassified rejection: %w", c.ID, err)
		}
		return Outcome{Kind: "error", Category: category}, nil, nil
	}
	digest := storageValueSHA256(playerStoredValueTree(stored))
	save := storedPlayerToSave(stored)

	encoded, err := player.Encode(save)
	if err != nil {
		category, ok := playerStorageErrorCategory(err)
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

	round, err := player.Decode(save.PlayerID, encoded)
	if err != nil {
		return Outcome{}, nil, fmt.Errorf("runtime-oracle: case %s: re-decode output: %w", c.ID, err)
	}
	if round.NeedsRewrite {
		return Outcome{}, nil, fmt.Errorf("runtime-oracle: case %s: encoded output needs rewrite", c.ID)
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

func playerCorpusRoutes() map[ConsumerRoute]GoOperation {
	return map[ConsumerRoute]GoOperation{
		{FamilyID: playerFamily, Version: playerVersion, Operation: "decode"}: runPlayerDecode,
		{FamilyID: playerFamily, Version: playerVersion, Operation: "encode"}: runPlayerEncode,
	}
}

func playerCurrentRoutes() []ConsumerRoute {
	return []ConsumerRoute{
		{FamilyID: playerFamily, Version: playerVersion, Operation: "decode"},
		{FamilyID: playerFamily, Version: playerVersion, Operation: "encode"},
	}
}

func playerRoundTripInventory() core.Inventory {
	stoneFull, _ := core.ItemMaxDurability(core.ItemStonePickaxe)
	ironFull, _ := core.ItemMaxDurability(core.ItemIronPickaxe)
	var inventory core.Inventory
	inventory.Hotbar.Selected = 3
	inventory.Hotbar.Slots[0] = core.ItemStack{Item: core.ItemStone, Count: core.MaxStackCount}
	inventory.Hotbar.Slots[4] = core.ItemStack{Item: core.ItemStonePickaxe, Count: 1, Durability: stoneFull}
	inventory.Hotbar.Slots[6] = core.ItemStack{Item: core.ItemGrass, Count: 1}
	inventory.Backpack[0] = core.ItemStack{Item: core.ItemDirt, Count: 12}
	inventory.Backpack[7] = core.ItemStack{Item: core.ItemIronPickaxe, Count: 1, Durability: ironFull}
	inventory.Backpack[core.BackpackSlots-1] = core.ItemStack{Item: core.ItemStone, Count: 5}
	return inventory
}

func playerRoundTripAltSave() player.PlayerSave {
	id := playerFixtureID()
	safe := player.PlayerLocation{Dimension: core.Overworld, Position: [3]float32{1.5, 65, -2.5}}
	return player.PlayerSave{
		PlayerID: id, Revision: 7, DisplayName: "Chen",
		Current: player.PlayerLocation{Dimension: core.Overworld, Position: [3]float32{2.5, 70, -3.5}},
		Yaw:     1.25, Pitch: -0.5, Safe: &safe, Inventory: playerRoundTripInventory(),
		Health: 13, Hunger: 12, SaturationMilli: 2500, ExhaustionMilli: 1750,
		RespawnPresent: true, RespawnPosition: [3]float32{7, 65, -9}, RespawnDimension: core.Overworld,
		Armor: [core.ArmorSlotCount]core.ItemStack{
			{Item: core.ItemIronHelmet, Count: 1, Durability: 165},
			{Item: core.ItemIronChestplate, Count: 1},
			{Item: core.ItemIronLeggings, Count: 0, Durability: 165},
		},
	}
}

func playerRawArmorSave() player.PlayerSave {
	save := playerRoundTripAltSave()
	save.Armor[0] = core.ItemStack{Item: 4242, Count: 65, Durability: 999}
	return save
}

func playerExhaustionMaxSave() player.PlayerSave {
	save := playerRoundTripAltSave()
	save.ExhaustionMilli = 65535
	return save
}

func playerAbsentRespawnDirtySave() player.PlayerSave {
	save := playerRoundTripAltSave()
	save.RespawnPresent = false
	save.RespawnPosition = [3]float32{1, 2, 3}
	save.RespawnDimension = core.Overworld
	return save
}

func mustEncodePlayerSave(t *testing.T, save player.PlayerSave) []byte {
	t.Helper()
	encoded, err := player.Encode(save)
	if err != nil {
		t.Fatalf("encode player save: %v", err)
	}
	return encoded
}

func readPlayerV9Fixture(t *testing.T, root string) []byte {
	t.Helper()
	full := filepath.Join(root, filepath.FromSlash(playerV9FixtureRepoRel))
	data, err := os.ReadFile(full)
	if err != nil {
		t.Fatalf("read v9 fixture: %v", err)
	}
	return data
}

func buildPlayerCandidate(
	t *testing.T,
	id, operation string,
	input []byte,
	args json.RawMessage,
	wantEncoded []byte,
) playerCandidate {
	t.Helper()
	var producer GoOperation
	switch operation {
	case "decode":
		producer = runPlayerDecode
	case "encode":
		producer = runPlayerEncode
	default:
		t.Fatalf("unsupported operation %q", operation)
	}
	spec := CaseSpec{
		ID:           id,
		Family:       playerFamily,
		Version:      playerVersion,
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
	inputRel := filepath.ToSlash(filepath.Join(playerCorpusRelDir, stem+".input.bin"))
	expectedRel := filepath.ToSlash(filepath.Join(playerCorpusRelDir, stem+".expected.json"))
	assets := map[string][]byte{
		inputRel:    input,
		expectedRel: expectedBytes,
	}
	spec.Input = AssetRef{Path: inputRel, SHA256: digestOf(t, input)}
	spec.Expected = AssetRef{Path: expectedRel, SHA256: digestOf(t, expectedBytes)}
	if operation == "encode" && saveOutcome.Kind == "ok" {
		encodedRel := filepath.ToSlash(filepath.Join(playerCorpusRelDir, stem+".encoded.bin"))
		assets[encodedRel] = encoded
		spec.Encoded = &AssetRef{Path: encodedRel, SHA256: digestOf(t, encoded)}
	}
	return playerCandidate{Spec: spec, Assets: assets, Expect: saveOutcome, Encoded: encoded}
}

func playerCurrentCandidates(t *testing.T) []playerCandidate {
	t.Helper()
	root := mustRepoRoot(t)
	argsDecode := playerArgumentsJSON(nil)
	v9Fixture := readPlayerV9Fixture(t, root)
	roundTripAlt := mustEncodePlayerSave(t, playerRoundTripAltSave())
	rawArmor := mustEncodePlayerSave(t, playerRawArmorSave())
	exhaustionMax := mustEncodePlayerSave(t, playerExhaustionMaxSave())
	absentRespawn := mustEncodePlayerSave(t, playerAbsentRespawnDirtySave())

	canonicalEncode := buildPlayerCandidate(t, playerEncodeV9CanonicalID, "encode", v9Fixture, argsDecode, nil)
	capacity := uint32(len(canonicalEncode.Encoded) - 1)
	capacityArgs := playerArgumentsJSON(&capacity)
	capacityCase := buildPlayerCandidate(t, playerEncodeCapacityMinusOneID, "encode", v9Fixture, capacityArgs, nil)

	return []playerCandidate{
		buildPlayerCandidate(t, playerDecodeV9FixtureID, "decode", v9Fixture, argsDecode, nil),
		buildPlayerCandidate(t, playerDecodeV9RoundTripAltID, "decode", roundTripAlt, argsDecode, nil),
		buildPlayerCandidate(t, playerDecodeRawArmorTripleID, "decode", rawArmor, argsDecode, nil),
		buildPlayerCandidate(t, playerDecodeAbsentRespawnID, "decode", absentRespawn, argsDecode, nil),
		buildPlayerCandidate(t, playerDecodeExhaustionMaxID, "decode", exhaustionMax, argsDecode, nil),
		canonicalEncode,
		capacityCase,
	}
}

func playerSelection(t *testing.T, root string, candidates []playerCandidate) StorageSelection {
	t.Helper()
	cases := make([]CaseSpec, 0, len(candidates))
	for _, candidate := range candidates {
		cases = append(cases, candidate.Spec)
	}
	sources := []SourceSpec{
		{Path: playerCodecSourceRel},
		{Path: playerProducerTestRel},
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
		ProducerID: playerProducerID,
		Cases:      cases,
		Sources:    sources,
		Routes:     playerCurrentRoutes(),
	}
}

func playerScratchRoot(t *testing.T, candidates []playerCandidate) string {
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

func playerManifest(t *testing.T, root string, candidates []playerCandidate) Inventory {
	t.Helper()
	base, err := LoadInventory(filepath.Join(root, filepath.FromSlash(InventoryRelPath)))
	if err != nil {
		t.Fatalf("load frozen manifest: %v", err)
	}
	selection := playerSelection(t, root, candidates)
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
		t.Fatalf("merge player selection: %v", err)
	}
	return merged
}

func playerRunnerManifest(t *testing.T, root string, candidates []playerCandidate) Inventory {
	t.Helper()
	merged := playerManifest(t, root, candidates)
	wantIDs := make(map[string]bool, len(candidates))
	for _, candidate := range candidates {
		wantIDs[candidate.Spec.ID] = true
	}
	var playerCases []CaseSpec
	for _, c := range merged.Cases {
		if c.Family == playerFamily && wantIDs[c.ID] {
			playerCases = append(playerCases, c)
		}
	}
	sort.Slice(playerCases, func(i, j int) bool { return playerCases[i].ID < playerCases[j].ID })
	caseIDs := make([]string, 0, len(playerCases))
	for _, c := range playerCases {
		caseIDs = append(caseIDs, c.ID)
	}
	for index := range merged.Families {
		if merged.Families[index].ID == playerFamily {
			merged.Families[index].Cases = caseIDs
		} else {
			merged.Families[index].Cases = nil
		}
	}
	merged.Cases = playerCases
	return merged
}

func exportPlayerSelectionCandidate(t *testing.T, root string, candidates []playerCandidate) string {
	t.Helper()
	if strings.TrimSpace(os.Getenv(runtimeOracleExportDirEnv)) == "" {
		return ""
	}
	selection := playerSelection(t, root, candidates)
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
	exportRoot, err := exportGeneratedAssets(root, strings.TrimSpace(os.Getenv(runtimeOracleExportDirEnv)), playerProducerID, assets)
	if err != nil {
		t.Fatalf("export player selection: %v", err)
	}
	return exportRoot
}

func playerObservation(t *testing.T, observations []ExecutedObservation, id string) ExecutedObservation {
	t.Helper()
	for _, obs := range observations {
		if obs.CaseID == id {
			return obs
		}
	}
	t.Fatalf("no observation for case %s", id)
	return ExecutedObservation{}
}

func TestStoragePlayerCurrentArgumentsValidate(t *testing.T) {
	if err := validateStorageArguments(playerFamily, "decode", playerArgumentsJSON(nil)); err != nil {
		t.Fatalf("validate player decode arguments: %v", err)
	}
	if err := validateStorageArguments(playerFamily, "encode", playerArgumentsJSON(nil)); err != nil {
		t.Fatalf("validate player encode arguments: %v", err)
	}
}

func TestStoragePlayerCurrentProducerExecutesEveryCase(t *testing.T) {
	root := mustRepoRoot(t)
	candidates := playerCurrentCandidates(t)
	manifest := playerRunnerManifest(t, root, candidates)
	staged := playerScratchRoot(t, candidates)

	observations, err := RunStorageCases(staged, manifest, playerCorpusRoutes())
	if err != nil {
		t.Fatalf("RunStorageCases: %v", err)
	}
	if len(observations) != len(candidates) {
		t.Fatalf("produced %d observations, want %d", len(observations), len(candidates))
	}
	for _, candidate := range candidates {
		obs := playerObservation(t, observations, candidate.Spec.ID)
		got, err := outcomeToStorageSave(obs.Outcome)
		if err != nil {
			t.Fatalf("case %s: %v", candidate.Spec.ID, err)
		}
		if !storageSaveOutcomesEqual(got, candidate.Expect) {
			t.Fatalf("case %s produced %#v, want %#v", candidate.Spec.ID, got, candidate.Expect)
		}
	}
}

func TestStoragePlayerCurrentArmorDigestMutationFailsComparison(t *testing.T) {
	root := mustRepoRoot(t)
	candidates := playerCurrentCandidates(t)
	var armorCase playerCandidate
	for _, candidate := range candidates {
		if candidate.Spec.ID == playerDecodeRawArmorTripleID {
			armorCase = candidate
			break
		}
	}
	if armorCase.Spec.ID == "" {
		t.Fatal("missing raw armor decode case")
	}
	stale := armorCase.Expect
	input := armorCase.Assets[armorCase.Spec.Input.Path]
	stored, err := player.Decode(playerFixtureID(), input)
	if err != nil {
		t.Fatalf("decode armor case: %v", err)
	}
	stored.Armor[0].Item++
	stale.ValueSHA256 = storageValueSHA256(playerStoredValueTree(stored))
	outcome, _, err := runPlayerDecode(armorCase.Spec, input)
	if err != nil {
		t.Fatalf("runPlayerDecode: %v", err)
	}
	got, err := outcomeToStorageSave(outcome)
	if err != nil {
		t.Fatal(err)
	}
	if storageSaveOutcomesEqual(got, stale) {
		t.Fatal("armor item mutation still matches stale expected digest")
	}
	_ = root
}

func TestStoragePlayerCurrentEncodePreservesSourceSave(t *testing.T) {
	candidates := playerCurrentCandidates(t)
	var encodeCase playerCandidate
	for _, candidate := range candidates {
		if candidate.Spec.ID == playerEncodeV9CanonicalID {
			encodeCase = candidate
			break
		}
	}
	if encodeCase.Spec.ID == "" {
		t.Fatal("missing canonical encode case")
	}
	input := encodeCase.Assets[encodeCase.Spec.Input.Path]
	_, wantID, err := parsePlayerArguments(encodeCase.Spec)
	if err != nil {
		t.Fatal(err)
	}
	stored, err := player.Decode(wantID, input)
	if err != nil {
		t.Fatalf("decode encode input: %v", err)
	}
	before := storedPlayerToSave(stored)
	clone := clonePlayerSave(before)
	_, _, err = runPlayerEncode(encodeCase.Spec, input)
	if err != nil {
		t.Fatalf("runPlayerEncode: %v", err)
	}
	if !reflect.DeepEqual(clone, before) {
		t.Fatal("encode path mutated the source PlayerSave")
	}
}

func TestStoragePlayerCurrentDecodeDigestsDiffer(t *testing.T) {
	candidates := playerCurrentCandidates(t)
	var fixtureDigest, altDigest string
	for _, candidate := range candidates {
		switch candidate.Spec.ID {
		case playerDecodeV9FixtureID:
			fixtureDigest = candidate.Expect.ValueSHA256
		case playerDecodeV9RoundTripAltID:
			altDigest = candidate.Expect.ValueSHA256
		}
	}
	if fixtureDigest == "" || altDigest == "" {
		t.Fatal("missing decode digest cases")
	}
	if fixtureDigest == altDigest {
		t.Fatal("v9 fixture and alternate decode digests must differ")
	}
}

func TestStoragePlayerCurrentExportUnsetWritesNothing(t *testing.T) {
	root := mustRepoRoot(t)
	t.Setenv(runtimeOracleExportDirEnv, "")
	before, err := os.ReadDir(root)
	if err != nil {
		t.Fatal(err)
	}
	_ = exportPlayerSelectionCandidate(t, root, playerCurrentCandidates(t))
	after, err := os.ReadDir(root)
	if err != nil {
		t.Fatal(err)
	}
	if len(before) != len(after) {
		t.Fatal("unset export must not mutate repository tree")
	}
}

func TestStoragePlayerCurrentCandidatesExportForReview(t *testing.T) {
	root := mustRepoRoot(t)
	exportRoot := filepath.Join(t.TempDir(), "player-export-parent")
	t.Setenv(runtimeOracleExportDirEnv, exportRoot)
	child := exportPlayerSelectionCandidate(t, root, playerCurrentCandidates(t))
	if child == "" {
		t.Fatal("export root unset after explicit env")
	}
	if _, err := readStorageSelection(child); err != nil {
		t.Fatalf("reload exported selection: %v", err)
	}
}

func TestStoragePlayerCurrentExportFromEnvironment(t *testing.T) {
	exportRoot := strings.TrimSpace(os.Getenv(runtimeOracleExportDirEnv))
	if exportRoot == "" {
		t.Skip("RUNTIME_ORACLE_EXPORT_DIR unset")
	}
	root := mustRepoRoot(t)
	child := exportPlayerSelectionCandidate(t, root, playerCurrentCandidates(t))
	if child == "" {
		t.Fatal("export root unset after explicit env")
	}
	if _, err := readStorageSelection(child); err != nil {
		t.Fatalf("reload exported selection: %v", err)
	}
}
