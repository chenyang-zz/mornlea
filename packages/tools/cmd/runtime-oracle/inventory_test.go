package main

import (
	"os"
	"path/filepath"
	"reflect"
	"strings"
	"testing"
)

func TestContractInventoryReconcilesFrozenCorpus(t *testing.T) {
	root, families, live := discoverLive(t)

	inventory, err := LoadInventory(filepath.Join(root, filepath.FromSlash(InventoryRelPath)))
	if err != nil {
		t.Fatalf("load frozen inventory: %v", err)
	}
	if _, err := ReconcileWorking(root, inventory, families, live, BaselineConsumerRegistry(), BaselineNegativeCoverageExceptions()); err != nil {
		t.Fatalf("frozen inventory drifted from current registries: %v", err)
	}
	digest, err := CanonicalCorpusDigest(inventory)
	if err != nil {
		t.Fatalf("compute canonical corpus digest: %v", err)
	}
	if !strings.HasPrefix(digest, "sha256:") || len(digest) != 71 {
		t.Fatalf("unexpected canonical corpus digest format: %q", digest)
	}
	assertRequiredCoverage(t, inventory.Families)
}

func TestContractInventoryRejectsMissingFamily(t *testing.T) {
	root, families, live := discoverLive(t)
	if len(families) == 0 {
		t.Fatal("discover produced no families")
	}
	cases, err := DiscoverCases(root)
	if err != nil {
		t.Fatalf("discover cases: %v", err)
	}

	inventory := inventoryFrom(live, families[1:], cases)
	_, err = ReconcileWorking(root, inventory, families, live, BaselineConsumerRegistry(), BaselineNegativeCoverageExceptions())
	if err == nil || !strings.Contains(err.Error(), "uncovered family "+families[0].ID) {
		t.Fatalf("missing family %s: error=%v", families[0].ID, err)
	}

	inventory = inventoryFrom(live, families, cases)
	inventory.Families = append(inventory.Families, Family{
		ID: "protocol.missing.Synthetic", Kind: "protocol", Role: "input",
		CurrentVersion: "1", SupportedVersions: []string{"1"},
		Source: "does-not-exist.go", EventualOwner: ownerProtocol,
		NumericSemantics: protocolLE,
		Sources:          []SourceSpec{{Path: "packages/shared/network/codec/frame.go", SHA256: "sha256:e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"}},
	})
	_, err = ReconcileWorking(root, inventory, families, live, BaselineConsumerRegistry(), BaselineNegativeCoverageExceptions())
	if err == nil || !strings.Contains(err.Error(), "inventory family protocol.missing.Synthetic is not in current registries") {
		t.Fatalf("extra inventory family: error=%v", err)
	}
}

func TestContractInventoryRejectsVersionMismatch(t *testing.T) {
	root, families, live := discoverLive(t)
	cases, err := DiscoverCases(root)
	if err != nil {
		t.Fatalf("discover cases: %v", err)
	}

	inventory := inventoryFrom(live, families, cases)
	inventory.Identities.Protocol++
	if inventory.Families[0].CurrentVersion == "" {
		t.Fatal("discovered family is missing a current version")
	}
	inventory.Families[0].CurrentVersion = "0"
	_, err = ReconcileWorking(root, inventory, families, live, BaselineConsumerRegistry(), BaselineNegativeCoverageExceptions())
	if err == nil {
		t.Fatal("version mismatch was accepted")
	}
	text := err.Error()
	if !strings.Contains(text, "protocol version") {
		t.Fatalf("identity version mismatch missing from %v", err)
	}
	if !strings.Contains(text, "family "+families[0].ID+" version") {
		t.Fatalf("family version mismatch missing from %v", err)
	}
}

func TestContractInventoryRejectsMissingProvenanceSource(t *testing.T) {
	root, families, live := discoverLive(t)
	cases, err := DiscoverCases(root)
	if err != nil {
		t.Fatalf("discover cases: %v", err)
	}

	inventory := inventoryFrom(live, families, cases)
	inventory.Families[0].Sources = []SourceSpec{{
		Path:   "testdata/runtime-migration/missing-source.go",
		SHA256: "sha256:e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
	}}
	_, err = ReconcileWorking(root, inventory, families, live, BaselineConsumerRegistry(), BaselineNegativeCoverageExceptions())
	if err == nil || !strings.Contains(err.Error(), "missing") {
		t.Fatalf("missing source: error=%v", err)
	}

	inventory = inventoryFrom(live, families, cases)
	inventory.Families[0].Sources = nil
	_, err = ReconcileWorking(root, inventory, families, live, BaselineConsumerRegistry(), BaselineNegativeCoverageExceptions())
	if err == nil || !strings.Contains(err.Error(), "has no provenance sources") {
		t.Fatalf("empty sources: error=%v", err)
	}
}

func TestContractInventoryRejectsIncompleteIdentity(t *testing.T) {
	root, families, live := discoverLive(t)
	cases, err := DiscoverCases(root)
	if err != nil {
		t.Fatalf("discover cases: %v", err)
	}

	inventory := inventoryFrom(live, families, cases)
	inventory.Identities.AgentHTTP = ""
	inventory.Identities.EngineABI = 0
	_, err = ReconcileWorking(root, inventory, families, live, BaselineConsumerRegistry(), BaselineNegativeCoverageExceptions())
	if err == nil {
		t.Fatal("incomplete identity was accepted")
	}
	text := err.Error()
	if !strings.Contains(text, "incomplete inventory identity") {
		t.Fatalf("incomplete identity missing from %v", err)
	}
}

func discoverLive(t *testing.T) (string, []Family, Identities) {
	t.Helper()
	root, err := RepositoryRoot()
	if err != nil {
		t.Fatal(err)
	}
	families, live, err := Discover(root)
	if err != nil {
		t.Fatalf("discover registries: %v", err)
	}
	if live.Protocol == 0 || live.EngineABI == 0 || live.AgentHTTP == "" {
		t.Fatalf("incomplete live identity: %+v", live)
	}
	return root, families, live
}

func inventoryFrom(live Identities, families []Family, cases []CaseSpec) Inventory {
	cloned := make([]Family, len(families))
	for i, family := range families {
		family.SupportedVersions = append([]string(nil), family.SupportedVersions...)
		family.Sources = append([]SourceSpec(nil), family.Sources...)
		family.Cases = append([]string(nil), family.Cases...)
		cloned[i] = family
	}
	clonedCases := append([]CaseSpec(nil), cases...)
	return Inventory{
		SchemaVersion:  inventorySchemaVersion,
		SourceRevision: BaselineSourceRevision,
		Identities:     live,
		Families:       cloned,
		Cases:          clonedCases,
	}
}

func assertRequiredCoverage(t *testing.T, families []Family) {
	t.Helper()
	seen := map[string]int{}
	roles := map[string]int{}
	owners := map[string]int{}
	for _, family := range families {
		if family.ID == "" || family.Source == "" || family.EventualOwner == "" || family.NumericSemantics == "" {
			t.Fatalf("family %#v is missing provenance", family)
		}
		seen[family.Kind]++
		roles[family.Role]++
		owners[family.EventualOwner]++
	}
	for _, kind := range []string{"domain", "protocol", "save", "kernel", "agent"} {
		if seen[kind] == 0 {
			t.Fatalf("inventory is missing kind %s", kind)
		}
	}
	for _, role := range []string{"input", "event"} {
		if roles[role] == 0 {
			t.Fatalf("inventory is missing semantic role %s", role)
		}
	}
	for _, owner := range []string{ownerDomain, ownerProtocol, ownerStorage, ownerEngine, ownerAgent} {
		if owners[owner] == 0 {
			t.Fatalf("inventory is missing eventual owner %s", owner)
		}
	}
	if seen["kernel"] < 2 {
		t.Fatalf("kernel coverage %d does not include both ABI exports and Go-only pathfind", seen["kernel"])
	}
}

func TestContractInventoryWorkingReportsZeroCaseFamilies(t *testing.T) {
	root, families, live := discoverLive(t)
	inventory, err := LoadInventory(filepath.Join(root, filepath.FromSlash(InventoryRelPath)))
	if err != nil {
		t.Fatalf("load frozen inventory: %v", err)
	}

	report, err := ReconcileWorking(
		root,
		inventory,
		families,
		live,
		BaselineConsumerRegistry(),
		BaselineNegativeCoverageExceptions(),
	)
	if err != nil {
		t.Fatalf("ReconcileWorking failed: %v", err)
	}

	zeroCasePoint := CoveragePoint{
		FamilyID: "kernel.mornlea_collision_resolve",
		Version:  "11",
	}
	foundUncovered := false
	for _, pt := range report.Uncovered {
		if pt == zeroCasePoint {
			foundUncovered = true
			break
		}
	}
	if !foundUncovered {
		t.Fatalf("expected zero-case family %v in Uncovered, got %v", zeroCasePoint, report.Uncovered)
	}
	for _, pt := range report.Covered {
		if pt == zeroCasePoint {
			t.Fatalf("expected zero-case family %v NOT in Covered", zeroCasePoint)
		}
	}

	assertCoveragePointsSorted(t, "Covered", report.Covered)
	assertCoveragePointsSorted(t, "Uncovered", report.Uncovered)
}

func TestContractInventoryCompleteRejectsZeroCaseFamilies(t *testing.T) {
	root, families, live := discoverLive(t)
	inventory, err := LoadInventory(filepath.Join(root, filepath.FromSlash(InventoryRelPath)))
	if err != nil {
		t.Fatalf("load frozen inventory: %v", err)
	}

	report, err := ReconcileComplete(
		root,
		inventory,
		families,
		live,
		BaselineConsumerRegistry(),
		BaselineNegativeCoverageExceptions(),
	)
	if err == nil {
		t.Fatal("ReconcileComplete expected error on zero-case families, got nil")
	}

	invErr, ok := err.(*InventoryError)
	if !ok {
		t.Fatalf("expected *InventoryError, got %T: %v", err, err)
	}

	zeroCasePoint := CoveragePoint{
		FamilyID: "kernel.mornlea_collision_resolve",
		Version:  "11",
	}
	foundInError := false
	for _, prob := range invErr.Problems {
		if strings.Contains(prob, zeroCasePoint.FamilyID) && strings.Contains(prob, zeroCasePoint.Version) {
			foundInError = true
			break
		}
	}
	if !foundInError {
		t.Fatalf("expected uncovered point %v in error problems, got %v", zeroCasePoint, invErr.Problems)
	}

	foundUncovered := false
	for _, pt := range report.Uncovered {
		if pt == zeroCasePoint {
			foundUncovered = true
			break
		}
	}
	if !foundUncovered {
		t.Fatalf("expected zero-case family %v in report.Uncovered, got %v", zeroCasePoint, report.Uncovered)
	}

	assertCoveragePointsSorted(t, "Covered", report.Covered)
	assertCoveragePointsSorted(t, "Uncovered", report.Uncovered)
}

func TestContractInventoryRejectsUnknownConsumer(t *testing.T) {
	root, families, live := discoverLive(t)
	cases, err := DiscoverCases(root)
	if err != nil {
		t.Fatalf("discover cases: %v", err)
	}
	if len(cases) == 0 {
		t.Fatal("no cases discovered")
	}

	inv := inventoryFrom(live, families, cases)
	inv.Cases[0].RustConsumer = "nonexistent_consumer"

	_, err = ReconcileWorking(
		root,
		inv,
		families,
		live,
		BaselineConsumerRegistry(),
		BaselineNegativeCoverageExceptions(),
	)
	if err == nil || !strings.Contains(err.Error(), "unknown consumer") {
		t.Fatalf("ReconcileWorking: expected unknown consumer error, got %v", err)
	}

	_, err = ReconcileComplete(
		root,
		inv,
		families,
		live,
		BaselineConsumerRegistry(),
		BaselineNegativeCoverageExceptions(),
	)
	if err == nil || !strings.Contains(err.Error(), "unknown consumer") {
		t.Fatalf("ReconcileComplete: expected unknown consumer error, got %v", err)
	}
}

func TestContractInventoryRejectsKnownConsumerOnUnsupportedRoute(t *testing.T) {
	root, families, live := discoverLive(t)
	frozen, err := LoadInventory(filepath.Join(root, filepath.FromSlash(InventoryRelPath)))
	if err != nil {
		t.Fatalf("load frozen inventory: %v", err)
	}

	wantRegistry := ConsumerRegistry{
		"corpus_frame": {
			Kind: ConsumerRust,
			Routes: map[ConsumerRoute]struct{}{
				{FamilyID: "protocol.frame", Version: "45", Operation: "decode"}: {},
				{FamilyID: "protocol.frame", Version: "45", Operation: "encode"}: {},
			},
		},
		"mornlea_protocol": {
			Kind: ConsumerRust,
			Routes: map[ConsumerRoute]struct{}{
				{FamilyID: "protocol.frame", Version: "45", Operation: "decode"}:                      {},
				{FamilyID: "protocol.frame", Version: "45", Operation: "encode"}:                      {},
				{FamilyID: "protocol.client.ClientHello", Version: "45", Operation: "decode"}:         {},
				{FamilyID: "protocol.client.ClientHello", Version: "45", Operation: "encode"}:         {},
				{FamilyID: "protocol.client.LoginStart", Version: "45", Operation: "decode"}:          {},
				{FamilyID: "protocol.client.LoginStart", Version: "45", Operation: "encode"}:          {},
				{FamilyID: "protocol.client.PlaceBlock", Version: "45", Operation: "decode"}:          {},
				{FamilyID: "protocol.client.PlaceBlock", Version: "45", Operation: "encode"}:          {},
				{FamilyID: "protocol.client.PlayerInput", Version: "45", Operation: "decode"}:         {},
				{FamilyID: "protocol.client.PlayerInput", Version: "45", Operation: "encode"}:         {},
				{FamilyID: "protocol.client.RequestChunkResync", Version: "45", Operation: "decode"}:  {},
				{FamilyID: "protocol.client.RequestChunkResync", Version: "45", Operation: "encode"}:  {},
				{FamilyID: "protocol.client.SelectHotbar", Version: "45", Operation: "decode"}:        {},
				{FamilyID: "protocol.client.SelectHotbar", Version: "45", Operation: "encode"}:        {},
				{FamilyID: "protocol.client.BoneMeal", Version: "45", Operation: "decode"}:            {},
				{FamilyID: "protocol.client.BoneMeal", Version: "45", Operation: "encode"}:            {},
				{FamilyID: "protocol.client.CollectWater", Version: "45", Operation: "decode"}:        {},
				{FamilyID: "protocol.client.CollectWater", Version: "45", Operation: "encode"}:        {},
				{FamilyID: "protocol.client.OpenContainer", Version: "45", Operation: "decode"}:       {},
				{FamilyID: "protocol.client.OpenContainer", Version: "45", Operation: "encode"}:       {},
				{FamilyID: "protocol.client.PlaceWater", Version: "45", Operation: "decode"}:          {},
				{FamilyID: "protocol.client.PlaceWater", Version: "45", Operation: "encode"}:          {},
				{FamilyID: "protocol.client.TillSoil", Version: "45", Operation: "decode"}:            {},
				{FamilyID: "protocol.client.TillSoil", Version: "45", Operation: "encode"}:            {},
				{FamilyID: "protocol.client.MoveInventoryStack", Version: "45", Operation: "decode"}:  {},
				{FamilyID: "protocol.client.MoveInventoryStack", Version: "45", Operation: "encode"}:  {},
				{FamilyID: "protocol.client.MoveCraftingStack", Version: "45", Operation: "decode"}:   {},
				{FamilyID: "protocol.client.MoveCraftingStack", Version: "45", Operation: "encode"}:   {},
				{FamilyID: "protocol.client.CloseContainer", Version: "45", Operation: "decode"}:      {},
				{FamilyID: "protocol.client.CloseContainer", Version: "45", Operation: "encode"}:      {},
				{FamilyID: "protocol.client.DropSelectedItem", Version: "45", Operation: "decode"}:    {},
				{FamilyID: "protocol.client.DropSelectedItem", Version: "45", Operation: "encode"}:    {},
				{FamilyID: "protocol.client.EquipArmor", Version: "45", Operation: "decode"}:          {},
				{FamilyID: "protocol.client.EquipArmor", Version: "45", Operation: "encode"}:          {},
				{FamilyID: "protocol.client.TakeCraftingOutput", Version: "45", Operation: "decode"}:  {},
				{FamilyID: "protocol.client.TakeCraftingOutput", Version: "45", Operation: "encode"}:  {},
				{FamilyID: "protocol.client.MoveContainerStack", Version: "45", Operation: "decode"}:  {},
				{FamilyID: "protocol.client.MoveContainerStack", Version: "45", Operation: "encode"}:  {},
				{FamilyID: "protocol.client.MoveStackPartial", Version: "45", Operation: "decode"}:    {},
				{FamilyID: "protocol.client.MoveStackPartial", Version: "45", Operation: "encode"}:    {},
				{FamilyID: "protocol.client.QuickMoveStack", Version: "45", Operation: "decode"}:      {},
				{FamilyID: "protocol.client.QuickMoveStack", Version: "45", Operation: "encode"}:      {},
				{FamilyID: "protocol.client.DropStack", Version: "45", Operation: "decode"}:           {},
				{FamilyID: "protocol.client.DropStack", Version: "45", Operation: "encode"}:           {},
				{FamilyID: "protocol.client.ChatCommand", Version: "45", Operation: "decode"}:         {},
				{FamilyID: "protocol.client.ChatCommand", Version: "45", Operation: "encode"}:         {},
				{FamilyID: "protocol.server.BlockChanges", Version: "45", Operation: "decode"}:        {},
				{FamilyID: "protocol.server.BlockChanges", Version: "45", Operation: "encode"}:        {},
				{FamilyID: "protocol.server.ForgetChunks", Version: "45", Operation: "decode"}:        {},
				{FamilyID: "protocol.server.ForgetChunks", Version: "45", Operation: "encode"}:        {},
				{FamilyID: "protocol.server.ChunkSnapshot", Version: "45", Operation: "decode"}:       {},
				{FamilyID: "protocol.server.ChunkSnapshot", Version: "45", Operation: "encode"}:       {},
				{FamilyID: "protocol.server.PlayerState", Version: "45", Operation: "decode"}:         {},
				{FamilyID: "protocol.server.PlayerState", Version: "45", Operation: "encode"}:         {},
				{FamilyID: "protocol.server.CommandRejected", Version: "45", Operation: "decode"}:     {},
				{FamilyID: "protocol.server.CommandRejected", Version: "45", Operation: "encode"}:     {},
				{FamilyID: "protocol.server.PlaceBlockSucceeded", Version: "45", Operation: "decode"}: {},
				{FamilyID: "protocol.server.PlaceBlockSucceeded", Version: "45", Operation: "encode"}: {},
				{FamilyID: "protocol.server.CombatHit", Version: "45", Operation: "decode"}:           {},
				{FamilyID: "protocol.server.CombatHit", Version: "45", Operation: "encode"}:           {},
				{FamilyID: "protocol.server.RemotePlayerSpawn", Version: "45", Operation: "decode"}:   {},
				{FamilyID: "protocol.server.RemotePlayerSpawn", Version: "45", Operation: "encode"}:   {},
				{FamilyID: "protocol.server.RemotePlayerDespawn", Version: "45", Operation: "decode"}: {},
				{FamilyID: "protocol.server.RemotePlayerDespawn", Version: "45", Operation: "encode"}: {},
				{FamilyID: "protocol.server.RemotePlayerStates", Version: "45", Operation: "decode"}:  {},
				{FamilyID: "protocol.server.RemotePlayerStates", Version: "45", Operation: "encode"}:  {},
				{FamilyID: "protocol.server.CompanionSpawn", Version: "45", Operation: "decode"}:      {},
				{FamilyID: "protocol.server.CompanionSpawn", Version: "45", Operation: "encode"}:      {},
				{FamilyID: "protocol.server.CompanionDespawn", Version: "45", Operation: "decode"}:    {},
				{FamilyID: "protocol.server.CompanionDespawn", Version: "45", Operation: "encode"}:    {},
				{FamilyID: "protocol.server.CompanionStates", Version: "45", Operation: "decode"}:     {},
				{FamilyID: "protocol.server.CompanionStates", Version: "45", Operation: "encode"}:     {},
				{FamilyID: "protocol.server.ChatEvent", Version: "45", Operation: "decode"}:           {},
				{FamilyID: "protocol.server.ChatEvent", Version: "45", Operation: "encode"}:           {},
				{FamilyID: "protocol.server.ItemDropUpserts", Version: "45", Operation: "decode"}:     {},
				{FamilyID: "protocol.server.ItemDropUpserts", Version: "45", Operation: "encode"}:     {},
				{FamilyID: "protocol.server.ItemDropRemoves", Version: "45", Operation: "decode"}:     {},
				{FamilyID: "protocol.server.ItemDropRemoves", Version: "45", Operation: "encode"}:     {},
				{FamilyID: "protocol.server.HostileSpawn", Version: "45", Operation: "decode"}:        {},
				{FamilyID: "protocol.server.HostileSpawn", Version: "45", Operation: "encode"}:        {},
				{FamilyID: "protocol.server.HostileState", Version: "45", Operation: "decode"}:        {},
				{FamilyID: "protocol.server.HostileState", Version: "45", Operation: "encode"}:        {},
				{FamilyID: "protocol.server.HostileDespawn", Version: "45", Operation: "decode"}:      {},
				{FamilyID: "protocol.server.HostileDespawn", Version: "45", Operation: "encode"}:      {},
				{FamilyID: "protocol.server.PassiveSpawn", Version: "45", Operation: "decode"}:        {},
				{FamilyID: "protocol.server.PassiveSpawn", Version: "45", Operation: "encode"}:        {},
				{FamilyID: "protocol.server.PassiveState", Version: "45", Operation: "decode"}:        {},
				{FamilyID: "protocol.server.PassiveState", Version: "45", Operation: "encode"}:        {},
				{FamilyID: "protocol.server.PassiveDespawn", Version: "45", Operation: "decode"}:      {},
				{FamilyID: "protocol.server.PassiveDespawn", Version: "45", Operation: "encode"}:      {},
				{FamilyID: "protocol.server.ProjectileSpawn", Version: "45", Operation: "decode"}:     {},
				{FamilyID: "protocol.server.ProjectileSpawn", Version: "45", Operation: "encode"}:     {},
				{FamilyID: "protocol.server.ProjectileState", Version: "45", Operation: "decode"}:     {},
				{FamilyID: "protocol.server.ProjectileState", Version: "45", Operation: "encode"}:     {},
				{FamilyID: "protocol.server.ProjectileDespawn", Version: "45", Operation: "decode"}:   {},
				{FamilyID: "protocol.server.ProjectileDespawn", Version: "45", Operation: "encode"}:   {},
				{FamilyID: "protocol.server.InventoryState", Version: "45", Operation: "decode"}:      {},
				{FamilyID: "protocol.server.InventoryState", Version: "45", Operation: "encode"}:      {},
				{FamilyID: "protocol.server.CraftingState", Version: "45", Operation: "decode"}:       {},
				{FamilyID: "protocol.server.CraftingState", Version: "45", Operation: "encode"}:       {},
				{FamilyID: "protocol.server.FurnaceState", Version: "45", Operation: "decode"}:        {},
				{FamilyID: "protocol.server.FurnaceState", Version: "45", Operation: "encode"}:        {},
				{FamilyID: "protocol.server.ChestState", Version: "45", Operation: "decode"}:          {},
				{FamilyID: "protocol.server.ChestState", Version: "45", Operation: "encode"}:          {},
				{FamilyID: "protocol.server.ContainerClosed", Version: "45", Operation: "decode"}:     {},
				{FamilyID: "protocol.server.ContainerClosed", Version: "45", Operation: "encode"}:     {},
				{FamilyID: "protocol.client.KeepAliveReply", Version: "45", Operation: "decode"}:      {},
				{FamilyID: "protocol.client.KeepAliveReply", Version: "45", Operation: "encode"}:      {},
				{FamilyID: "protocol.server.Disconnect", Version: "45", Operation: "decode"}:          {},
				{FamilyID: "protocol.server.Disconnect", Version: "45", Operation: "encode"}:          {},
				{FamilyID: "protocol.server.HandshakeReject", Version: "45", Operation: "decode"}:     {},
				{FamilyID: "protocol.server.HandshakeReject", Version: "45", Operation: "encode"}:     {},
				{FamilyID: "protocol.server.KeepAlive", Version: "45", Operation: "decode"}:           {},
				{FamilyID: "protocol.server.KeepAlive", Version: "45", Operation: "encode"}:           {},
				{FamilyID: "protocol.server.LoginReject", Version: "45", Operation: "decode"}:         {},
				{FamilyID: "protocol.server.LoginReject", Version: "45", Operation: "encode"}:         {},
				{FamilyID: "protocol.server.LoginSuccess", Version: "45", Operation: "decode"}:        {},
				{FamilyID: "protocol.server.LoginSuccess", Version: "45", Operation: "encode"}:        {},
				{FamilyID: "protocol.server.ServerHello", Version: "45", Operation: "decode"}:         {},
				{FamilyID: "protocol.server.ServerHello", Version: "45", Operation: "encode"}:         {},
			},
		},
		"mornlea_domain": {
			Kind: ConsumerRust,
			Routes: map[ConsumerRoute]struct{}{
				{FamilyID: "domain.identity_values", Version: "current", Operation: "admit"}:   {},
				{FamilyID: "domain.values", Version: "current", Operation: "admit"}:            {},
				{FamilyID: "domain.command_control", Version: "current", Operation: "admit"}:   {},
				{FamilyID: "domain.command_inventory", Version: "current", Operation: "admit"}: {},
				{FamilyID: "domain.event", Version: "1", Operation: "admit"}:                   {},
			},
		},
		"mornlea_storage": {
			Kind: ConsumerRust,
			Routes: map[ConsumerRoute]struct{}{
				{FamilyID: "save.player", Version: "1", Operation: "decode"}: {},
				{FamilyID: "save.player", Version: "2", Operation: "decode"}: {},
				{FamilyID: "save.player", Version: "3", Operation: "decode"}: {},
				{FamilyID: "save.player", Version: "4", Operation: "decode"}: {},
				{FamilyID: "save.player", Version: "5", Operation: "decode"}: {},
				{FamilyID: "save.player", Version: "6", Operation: "decode"}: {},
				{FamilyID: "save.player", Version: "7", Operation: "decode"}: {},
				{FamilyID: "save.player", Version: "8", Operation: "decode"}: {},
				{FamilyID: "save.player", Version: "9", Operation: "decode"}: {},
				{FamilyID: "save.player", Version: "9", Operation: "encode"}: {},
				{FamilyID: "save.region", Version: "1", Operation: "decode"}: {},
				{FamilyID: "save.region", Version: "1", Operation: "encode"}: {},
				{FamilyID: "save.region", Version: "1", Operation: "order"}:  {},
				{FamilyID: "save.world-metadata", Version: "1", Operation: "decode"}: {},
				{FamilyID: "save.world-metadata", Version: "2", Operation: "decode"}: {},
				{FamilyID: "save.world-metadata", Version: "3", Operation: "decode"}: {},
				{FamilyID: "save.world-metadata", Version: "4", Operation: "decode"}: {},
				{FamilyID: "save.world-metadata", Version: "5", Operation: "decode"}: {},
				{FamilyID: "save.world-metadata", Version: "6", Operation: "decode"}: {},
				{FamilyID: "save.world-metadata", Version: "6", Operation: "encode"}: {},
				{FamilyID: "save.hostile", Version: "1", Operation: "decode"}:       {},
				{FamilyID: "save.hostile", Version: "2", Operation: "decode"}:       {},
				{FamilyID: "save.hostile", Version: "2", Operation: "encode"}:       {},
				{FamilyID: "save.passive", Version: "1", Operation: "decode"}:       {},
				{FamilyID: "save.passive", Version: "1", Operation: "encode"}:       {},
				{FamilyID: "save.chunk", Version: "1", Operation: "decode"}:         {},
				{FamilyID: "save.chunk", Version: "2", Operation: "decode"}:         {},
				{FamilyID: "save.chunk", Version: "3", Operation: "decode"}:         {},
				{FamilyID: "save.chunk", Version: "4", Operation: "decode"}:         {},
			},
		},
		"external:agent-contract": {
			Kind: ConsumerExternalGo,
			Routes: map[ConsumerRoute]struct{}{
				{FamilyID: "agent.http", Version: "v1", Operation: "agent-contract"}: {},
				{FamilyID: "agent.mcp", Version: "v1", Operation: "agent-contract"}:  {},
			},
		},
		"external:runtime-authority": {
			Kind: ConsumerExternalGo,
			Routes: map[ConsumerRoute]struct{}{
				{FamilyID: "domain.input", Version: "45", Operation: "order"}: {},
			},
		},
	}
	if got := BaselineConsumerRegistry(); !reflect.DeepEqual(got, wantRegistry) {
		t.Fatalf("BaselineConsumerRegistry() = %#v, want %#v", got, wantRegistry)
	}

	frameIndex := caseIndexByID(t, frozen.Cases, "protocol.frame/45/valid")
	domainIndex := firstCaseIndexForFamily(t, frozen.Cases, "domain.values")
	tests := []struct {
		name      string
		inventory Inventory
		registry  ConsumerRegistry
		wantRoute ConsumerRoute
	}{
		{
			name: "wrong_family",
			inventory: mutateInventoryCase(frozen, domainIndex, func(c *CaseSpec) {
				c.RustConsumer = "corpus_frame"
			}),
			registry: BaselineConsumerRegistry(),
			wantRoute: ConsumerRoute{
				FamilyID: "domain.values", Version: "current", Operation: "admit",
			},
		},
		{
			// `protocol.frame` now publishes both a decode and an encode
			// operation, so the unsupported operation has to be one the closed
			// vocabulary carries but the frame family does not execute.
			name: "wrong_operation",
			inventory: mutateInventoryCase(frozen, frameIndex, func(c *CaseSpec) {
				c.Operation = "admit"
			}),
			registry: BaselineConsumerRegistry(),
			wantRoute: ConsumerRoute{
				FamilyID: "protocol.frame", Version: "45", Operation: "admit",
			},
		},
		{
			name:      "registry_wrong_version",
			inventory: frozen,
			registry: ConsumerRegistry{
				"corpus_frame": {
					Kind: ConsumerRust,
					Routes: map[ConsumerRoute]struct{}{
						{FamilyID: "protocol.frame", Version: "44", Operation: "decode"}: {},
					},
				},
				"mornlea_domain":             wantRegistry["mornlea_domain"],
				"external:agent-contract":    wantRegistry["external:agent-contract"],
				"external:runtime-authority": wantRegistry["external:runtime-authority"],
			},
			wantRoute: ConsumerRoute{
				FamilyID: "protocol.frame", Version: "45", Operation: "decode",
			},
		},
	}

	reconcileModes := []struct {
		name string
		run  func(string, Inventory, []Family, Identities, ConsumerRegistry, NegativeCoverageExceptions) (CoverageReport, error)
	}{
		{name: "working", run: ReconcileWorking},
		{name: "complete", run: ReconcileComplete},
	}
	for _, tc := range tests {
		t.Run(tc.name, func(t *testing.T) {
			wantDiagnostic := tc.wantRoute.FamilyID + "/" + tc.wantRoute.Version + "/" + tc.wantRoute.Operation
			for _, mode := range reconcileModes {
				t.Run(mode.name, func(t *testing.T) {
					_, err := mode.run(root, tc.inventory, families, live, tc.registry, BaselineNegativeCoverageExceptions())
					if err == nil || !strings.Contains(err.Error(), "unsupported route "+wantDiagnostic) {
						t.Fatalf("expected unsupported route %s, got %v", wantDiagnostic, err)
					}
				})
			}
		})
	}
}

func TestContractInventoryRejectsInvalidConsumerRegistry(t *testing.T) {
	root, families, live := discoverLive(t)
	frozen, err := LoadInventory(filepath.Join(root, filepath.FromSlash(InventoryRelPath)))
	if err != nil {
		t.Fatalf("load frozen inventory: %v", err)
	}

	tests := []struct {
		name    string
		mutate  func(ConsumerRegistry)
		wantErr string
	}{
		{
			name: "empty_consumer_name",
			mutate: func(registry ConsumerRegistry) {
				registry[""] = ConsumerRegistration{
					Kind: ConsumerRust,
					Routes: map[ConsumerRoute]struct{}{
						{FamilyID: "protocol.frame", Version: "45", Operation: "decode"}: {},
					},
				}
			},
			wantErr: "consumer registry has empty name",
		},
		{
			name: "invalid_kind",
			mutate: func(registry ConsumerRegistry) {
				registration := registry["corpus_frame"]
				registration.Kind = ConsumerKind(255)
				registry["corpus_frame"] = registration
			},
			wantErr: "consumer registry entry \"corpus_frame\" has invalid kind",
		},
		{
			name: "empty_routes",
			mutate: func(registry ConsumerRegistry) {
				registration := registry["corpus_frame"]
				registration.Routes = map[ConsumerRoute]struct{}{}
				registry["corpus_frame"] = registration
			},
			wantErr: "consumer registry entry \"corpus_frame\" has no routes",
		},
	}

	reconcileModes := []struct {
		name string
		run  func(string, Inventory, []Family, Identities, ConsumerRegistry, NegativeCoverageExceptions) (CoverageReport, error)
	}{
		{name: "working", run: ReconcileWorking},
		{name: "complete", run: ReconcileComplete},
	}
	for _, tc := range tests {
		t.Run(tc.name, func(t *testing.T) {
			for _, mode := range reconcileModes {
				t.Run(mode.name, func(t *testing.T) {
					inventory := mutateInventoryCase(frozen, 0, func(c *CaseSpec) {
						c.Input.Path = "testdata/runtime-migration/missing-before-registry-validation.bin"
					})
					registry := BaselineConsumerRegistry()
					tc.mutate(registry)
					report, err := mode.run(root, inventory, families, live, registry, BaselineNegativeCoverageExceptions())
					if err == nil || !strings.Contains(err.Error(), tc.wantErr) {
						t.Fatalf("expected %q, got %v", tc.wantErr, err)
					}
					if strings.Contains(err.Error(), "missing-before-registry-validation") || strings.Contains(err.Error(), "unsupported route") {
						t.Fatalf("invalid registry reached case validation: %v", err)
					}
					if len(report.Covered) != 0 || len(report.Uncovered) != 0 {
						t.Fatalf("invalid registry reported case coverage: %+v", report)
					}
				})
			}
		})
	}
}

func TestContractInventoryInputAssetBudgets(t *testing.T) {
	tests := []struct {
		name        string
		inputFormat string
		input       []byte
		wantErr     string
	}{
		{name: "json_exact_limit", inputFormat: "json", input: exactJSONObject(t, MaxCaseJSONBytes)},
		{name: "json_over_limit", inputFormat: "json", input: exactJSONObject(t, MaxCaseJSONBytes+1), wantErr: "exceeds budget"},
		{name: "binary_exact_limit", inputFormat: "binary", input: make([]byte, MaxBinaryBytes)},
	}
	for _, tc := range tests {
		t.Run(tc.name, func(t *testing.T) {
			fixture := newInventoryAssetFixture(t, tc.inputFormat, tc.input, true)
			_, err := ReconcileWorking(
				fixture.root,
				fixture.inventory,
				fixture.families,
				fixture.live,
				BaselineConsumerRegistry(),
				BaselineNegativeCoverageExceptions(),
			)
			if tc.wantErr == "" && err != nil {
				t.Fatalf("ReconcileWorking rejected exact input budget: %v", err)
			}
			if tc.wantErr != "" && (err == nil || !strings.Contains(err.Error(), tc.wantErr)) {
				t.Fatalf("ReconcileWorking expected %q, got %v", tc.wantErr, err)
			}
		})
	}
}

func TestContractInventoryRejectsNonRegularAssets(t *testing.T) {
	tests := []struct {
		name   string
		mutate func(*inventoryAssetFixture, string)
		role   string
	}{
		{
			name: "provenance_source",
			mutate: func(fixture *inventoryAssetFixture, rel string) {
				fixture.inventory.Families[0].Sources[0] = assetRefAsSource(rel)
				fixture.families[0].Sources[0] = assetRefAsSource(rel)
			},
			role: "source",
		},
		{
			name: "input",
			mutate: func(fixture *inventoryAssetFixture, rel string) {
				fixture.inventory.Cases[0].Input = directoryAssetRef(rel)
			},
			role: "input asset",
		},
		{
			name: "expected",
			mutate: func(fixture *inventoryAssetFixture, rel string) {
				fixture.inventory.Cases[0].Expected = directoryAssetRef(rel)
			},
			role: "expected asset",
		},
		{
			name: "encoded",
			mutate: func(fixture *inventoryAssetFixture, rel string) {
				asset := directoryAssetRef(rel)
				fixture.inventory.Cases[0].Encoded = &asset
			},
			role: "encoded asset",
		},
	}
	for _, tc := range tests {
		t.Run(tc.name, func(t *testing.T) {
			fixture := newInventoryAssetFixture(t, "json", []byte(`{"value":1}`), true)
			rel := filepath.ToSlash(filepath.Join("assets", "nonregular-"+tc.name+".json"))
			if tc.name == "encoded" {
				rel = filepath.ToSlash(filepath.Join("assets", "nonregular-"+tc.name+".bin"))
			}
			if err := os.Mkdir(filepath.Join(fixture.root, filepath.FromSlash(rel)), 0o755); err != nil {
				t.Fatal(err)
			}
			tc.mutate(&fixture, rel)

			_, err := ReconcileWorking(
				fixture.root,
				fixture.inventory,
				fixture.families,
				fixture.live,
				BaselineConsumerRegistry(),
				BaselineNegativeCoverageExceptions(),
			)
			if err == nil || !strings.Contains(err.Error(), tc.role) || !strings.Contains(err.Error(), "regular file") {
				t.Fatalf("expected %s regular-file rejection, got %v", tc.role, err)
			}
		})
	}
}

func mutateInventoryCase(inventory Inventory, index int, mutate func(*CaseSpec)) Inventory {
	inventory.Cases = append([]CaseSpec(nil), inventory.Cases...)
	mutate(&inventory.Cases[index])
	return inventory
}

func firstCaseIndexForFamily(t *testing.T, cases []CaseSpec, family string) int {
	t.Helper()
	for i := range cases {
		if cases[i].Family == family {
			return i
		}
	}
	t.Fatalf("no case for family %s", family)
	return -1
}

func caseIndexByID(t *testing.T, cases []CaseSpec, id string) int {
	t.Helper()
	for i := range cases {
		if cases[i].ID == id {
			return i
		}
	}
	t.Fatalf("no case with id %s", id)
	return -1
}

func TestContractInventoryRejectsUnsupportedCaseVersion(t *testing.T) {
	root, families, live := discoverLive(t)
	cases, err := DiscoverCases(root)
	if err != nil {
		t.Fatalf("discover cases: %v", err)
	}
	if len(cases) == 0 {
		t.Fatal("no cases discovered")
	}

	inv := inventoryFrom(live, families, cases)
	newID := inv.Cases[0].Family + "/999/unsupported_version_case"
	inv.Cases[0].Version = "999"
	inv.Cases[0].ID = newID
	for i, f := range inv.Families {
		if f.ID == inv.Cases[0].Family {
			for j, id := range f.Cases {
				if id == cases[0].ID {
					inv.Families[i].Cases[j] = newID
				}
			}
		}
	}

	_, err = ReconcileWorking(
		root,
		inv,
		families,
		live,
		BaselineConsumerRegistry(),
		BaselineNegativeCoverageExceptions(),
	)
	if err == nil || (!strings.Contains(err.Error(), "supported_versions") && !strings.Contains(err.Error(), "unsupported case version")) {
		t.Fatalf("ReconcileWorking: expected unsupported version error, got %v", err)
	}

	_, err = ReconcileComplete(
		root,
		inv,
		families,
		live,
		BaselineConsumerRegistry(),
		BaselineNegativeCoverageExceptions(),
	)
	if err == nil || (!strings.Contains(err.Error(), "supported_versions") && !strings.Contains(err.Error(), "unsupported case version")) {
		t.Fatalf("ReconcileComplete: expected unsupported version error, got %v", err)
	}
}

func TestContractInventoryWorkingAndCompleteCoverage(t *testing.T) {
	root, families, live := discoverLive(t)
	frozen, err := LoadInventory(filepath.Join(root, filepath.FromSlash(InventoryRelPath)))
	if err != nil {
		t.Fatalf("load frozen inventory: %v", err)
	}

	// Create an inventory where protocol.frame has only its kind: "ok" case.
	framePoint := CoveragePoint{
		FamilyID: "protocol.frame",
		Version:  "45",
	}
	var filteredCases []CaseSpec
	for _, c := range frozen.Cases {
		if c.Family == "protocol.frame" {
			if strings.HasSuffix(c.ID, "/valid") {
				filteredCases = append(filteredCases, c)
			}
		} else {
			filteredCases = append(filteredCases, c)
		}
	}
	inv := Inventory{
		SchemaVersion:  frozen.SchemaVersion,
		SourceRevision: frozen.SourceRevision,
		Identities:     live,
		Families:       make([]Family, len(frozen.Families)),
		Cases:          filteredCases,
	}
	for i, f := range frozen.Families {
		inv.Families[i] = f
		inv.Families[i].SupportedVersions = append([]string(nil), f.SupportedVersions...)
		inv.Families[i].Sources = append([]SourceSpec(nil), f.Sources...)
		if f.ID == "protocol.frame" {
			inv.Families[i].Cases = []string{"protocol.frame/45/valid"}
		} else {
			inv.Families[i].Cases = append([]string(nil), f.Cases...)
		}
	}

	tests := []struct {
		name               string
		negativeExceptions NegativeCoverageExceptions
		wantCovered        bool
		wantErrSubstring   string
	}{
		{
			name:               "ok_only_without_exception",
			negativeExceptions: NegativeCoverageExceptions{},
			wantCovered:        false,
		},
		{
			name: "ok_only_with_reviewed_exception",
			negativeExceptions: NegativeCoverageExceptions{
				framePoint: "reviewed: frame parsing synthetic valid-only profile has no error representation",
			},
			wantCovered: true,
		},
		{
			name: "exception_with_empty_rationale",
			negativeExceptions: NegativeCoverageExceptions{
				framePoint: "   ",
			},
			wantErrSubstring: "empty rationale",
		},
		{
			name: "exception_for_unknown_point",
			negativeExceptions: NegativeCoverageExceptions{
				CoveragePoint{FamilyID: "unknown.family", Version: "1"}: "some rationale",
			},
			wantErrSubstring: "unknown family/version",
		},
	}

	for _, tc := range tests {
		t.Run(tc.name, func(t *testing.T) {
			workingReport, workingErr := ReconcileWorking(
				root,
				inv,
				families,
				live,
				BaselineConsumerRegistry(),
				tc.negativeExceptions,
			)

			if tc.wantErrSubstring != "" {
				if workingErr == nil || !strings.Contains(workingErr.Error(), tc.wantErrSubstring) {
					t.Fatalf("ReconcileWorking: want error containing %q, got %v", tc.wantErrSubstring, workingErr)
				}
				completeReport, completeErr := ReconcileComplete(
					root,
					inv,
					families,
					live,
					BaselineConsumerRegistry(),
					tc.negativeExceptions,
				)
				_ = completeReport
				if completeErr == nil || !strings.Contains(completeErr.Error(), tc.wantErrSubstring) {
					t.Fatalf("ReconcileComplete: want error containing %q, got %v", tc.wantErrSubstring, completeErr)
				}
				return
			}

			if workingErr != nil {
				t.Fatalf("ReconcileWorking unexpected error: %v", workingErr)
			}

			assertCoveragePointsSorted(t, "working Covered", workingReport.Covered)
			assertCoveragePointsSorted(t, "working Uncovered", workingReport.Uncovered)

			hasPoint := func(pts []CoveragePoint, target CoveragePoint) bool {
				for _, pt := range pts {
					if pt == target {
						return true
					}
				}
				return false
			}

			if tc.wantCovered {
				if !hasPoint(workingReport.Covered, framePoint) {
					t.Fatalf("expected %v in working Covered, got %v", framePoint, workingReport.Covered)
				}
				if hasPoint(workingReport.Uncovered, framePoint) {
					t.Fatalf("expected %v NOT in working Uncovered, got %v", framePoint, workingReport.Uncovered)
				}
			} else {
				if hasPoint(workingReport.Covered, framePoint) {
					t.Fatalf("expected %v NOT in working Covered, got %v", framePoint, workingReport.Covered)
				}
				if !hasPoint(workingReport.Uncovered, framePoint) {
					t.Fatalf("expected %v in working Uncovered, got %v", framePoint, workingReport.Uncovered)
				}

				// In complete mode, without the exception, protocol.frame must be rejected
				completeReport, completeErr := ReconcileComplete(
					root,
					inv,
					families,
					live,
					BaselineConsumerRegistry(),
					tc.negativeExceptions,
				)
				if completeErr == nil {
					t.Fatalf("ReconcileComplete expected rejection of uncovered point %v, got nil", framePoint)
				}
				if !strings.Contains(completeErr.Error(), framePoint.FamilyID) || !strings.Contains(completeErr.Error(), framePoint.Version) {
					t.Fatalf("ReconcileComplete error should identify uncovered point %v, got %v", framePoint, completeErr)
				}
				if !hasPoint(completeReport.Uncovered, framePoint) {
					t.Fatalf("expected %v in complete Uncovered, got %v", framePoint, completeReport.Uncovered)
				}
				assertCoveragePointsSorted(t, "complete Covered", completeReport.Covered)
				assertCoveragePointsSorted(t, "complete Uncovered", completeReport.Uncovered)
			}
		})
	}
}

func assertCoveragePointsSorted(t *testing.T, label string, points []CoveragePoint) {
	t.Helper()
	for i := 0; i < len(points)-1; i++ {
		curr, next := points[i], points[i+1]
		if curr.FamilyID > next.FamilyID || (curr.FamilyID == next.FamilyID && curr.Version >= next.Version) {
			t.Fatalf("%s points not sorted: points[%d]=%+v >= points[%d]=%+v", label, i, curr, i+1, next)
		}
	}
}
