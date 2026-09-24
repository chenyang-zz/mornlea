package main

import (
	"os"
	"os/exec"
	"path/filepath"
	"sort"
	"strconv"
	"strings"
	"testing"
)

// This file is the protocol-only zero-gap evidence for the corpus closure. The
// corpus is already integrated with every protocol family carrying executed
// cases, so nothing here registers or rewrites a case: the tests prove the
// closed state from three independent sides and pin that every drift class the
// change names fails instead of passing silently.
//
// The three sides are live discovery (the Go registry scan), the tracked
// manifest, and the closed consumer route union. A family, a route or a case
// that only one side names is a closure failure rather than a shared name.
//
// The one-time source revision refresh is staged here as an external
// candidate export: the tracked manifest keeps its recorded revision until the
// controller integrates the reviewed candidate, so no ordinary run writes to
// the corpus tree.

const (
	// protocolFamilyPrefix selects the protocol slice of every family set.
	protocolFamilyPrefix = "protocol."
	// protocolFrameFamily is the one framing family beside the packet families.
	protocolFrameFamily = "protocol.frame"
	// minPacketFamilies is the reviewed packet family count: 23 client and 36
	// server keys. The frame family sits beside them.
	minPacketFamilies = 59
	// minProtocolFamilies is the reviewed protocol family count: 59 packet
	// families plus the framing family.
	minProtocolFamilies = 60
	// minPacketCases is the reviewed minimum packet case count: every packet
	// family carries at least one valid decode, one valid encode and one
	// invalid or boundary case.
	minPacketCases = 177
	// minFrameCases is the reviewed minimum framing case count: two decode
	// cases plus the real encode case.
	minFrameCases = 3
	// minProtocolCases is the reviewed minimum protocol case total.
	minProtocolCases = 180
)

// protocolFamilyTally counts one protocol family's cases by operation and
// recorded outcome kind, so the per-family minimum is checked from the frozen
// evidence rather than from a name.
type protocolFamilyTally struct {
	decodeOK    int
	decodeError int
	encodeOK    int
	encodeError int
}

// loadProtocolClosureInventory loads the tracked corpus manifest.
func loadProtocolClosureInventory(t *testing.T, root string) Inventory {
	t.Helper()
	frozen, err := LoadInventory(filepath.Join(root, filepath.FromSlash(InventoryRelPath)))
	if err != nil {
		t.Fatalf("load frozen inventory: %v", err)
	}
	return frozen
}

// cloneProtocolClosureInventory deep-copies a manifest so a mutation test can
// rewrite one field without touching the frozen value or a shared slice.
func cloneProtocolClosureInventory(frozen Inventory) Inventory {
	cloned := Inventory{
		SchemaVersion:  frozen.SchemaVersion,
		SourceRevision: frozen.SourceRevision,
		Identities:     frozen.Identities,
		Families:       make([]Family, len(frozen.Families)),
		Cases:          append([]CaseSpec(nil), frozen.Cases...),
	}
	for index, family := range frozen.Families {
		family.SupportedVersions = append([]string(nil), family.SupportedVersions...)
		family.Sources = append([]SourceSpec(nil), family.Sources...)
		family.Cases = append([]string(nil), family.Cases...)
		cloned.Families[index] = family
	}
	return cloned
}

// protocolFamilyIDs returns the sorted protocol family identities one family
// set carries.
func protocolFamilyIDs(families []Family) []string {
	ids := make([]string, 0, len(families))
	for _, family := range families {
		if strings.HasPrefix(family.ID, protocolFamilyPrefix) {
			ids = append(ids, family.ID)
		}
	}
	sort.Strings(ids)
	return ids
}

// protocolRouteFamilies returns the sorted, deduplicated family/version
// points the closed consumer route union registers for the protocol slice.
// The framing routes are registered for both the frame consumer and the
// packet consumer, so the union deduplicates before it is compared.
func protocolRouteFamilies(consumers ConsumerRegistry) []CoveragePoint {
	seen := make(map[CoveragePoint]bool)
	points := make([]CoveragePoint, 0)
	for _, registration := range consumers {
		for route := range registration.Routes {
			if !strings.HasPrefix(route.FamilyID, protocolFamilyPrefix) {
				continue
			}
			pt := CoveragePoint{FamilyID: route.FamilyID, Version: route.Version}
			if seen[pt] {
				continue
			}
			seen[pt] = true
			points = append(points, pt)
		}
	}
	sortCoveragePoints(points)
	return points
}

// protocolRouteOperations returns the deduplicated operations one family's
// routes carry across the closed consumer registry. The framing family is
// registered for both the frame consumer and the packet consumer, so the
// union deduplicates before it is compared.
func protocolRouteOperations(consumers ConsumerRegistry, familyID, version string) []string {
	seen := make(map[string]bool, 2)
	operations := make([]string, 0, 2)
	for _, registration := range consumers {
		for route := range registration.Routes {
			if route.FamilyID != familyID || route.Version != version {
				continue
			}
			if seen[route.Operation] {
				continue
			}
			seen[route.Operation] = true
			operations = append(operations, route.Operation)
		}
	}
	sort.Strings(operations)
	return operations
}

// tallyProtocolFamilies counts the frozen protocol cases per family, proving
// each family's minimum from the recorded outcome kinds.
func tallyProtocolFamilies(t *testing.T, root string, frozen Inventory) (map[string]*protocolFamilyTally, int, int) {
	t.Helper()
	tallies := make(map[string]*protocolFamilyTally)
	packetCases, frameCases := 0, 0
	for _, c := range frozen.Cases {
		if !strings.HasPrefix(c.Family, protocolFamilyPrefix) {
			continue
		}
		if c.Family == protocolFrameFamily {
			frameCases++
		} else {
			packetCases++
		}
		tally, exists := tallies[c.Family]
		if !exists {
			tally = &protocolFamilyTally{}
			tallies[c.Family] = tally
		}
		if len(c.Checkpoints) != 1 || c.Checkpoints[0] != "0" {
			t.Fatalf("case %s carries checkpoints %v, want exactly one zero checkpoint", c.ID, c.Checkpoints)
		}
		kind, err := decodeExpectedOutcome(root, c)
		if err != nil {
			t.Fatalf("read expected outcome for %s: %v", c.ID, err)
		}
		switch {
		case c.Operation == "decode" && kind.Kind == "ok":
			tally.decodeOK++
		case c.Operation == "decode" && kind.Kind == "error":
			tally.decodeError++
		case c.Operation == "encode" && kind.Kind == "ok":
			tally.encodeOK++
		case c.Operation == "encode" && kind.Kind == "error":
			tally.encodeError++
		default:
			t.Fatalf("case %s records operation %s with unexpected kind %q", c.ID, c.Operation, kind.Kind)
		}
	}
	return tallies, packetCases, frameCases
}

// TestProtocolCorpusComplete pins the protocol-only zero-gap state: every
// protocol family and version point the registries name is covered by executed
// Go producer and Rust consumer evidence, and every other family's remaining
// gap stays visible without being claimed.
func TestProtocolCorpusComplete(t *testing.T) {
	root, families, live := discoverLive(t)
	before := computeTrackedCorpusDigest(t, root)
	defer assertTrackedCorpusUnchanged(t, root, before)

	frozen := loadProtocolClosureInventory(t, root)
	protocolVersion := strconv.Itoa(live.Protocol)

	// The exact protocol family set is identical across the three
	// independent sides: live discovery, the tracked manifest, and the closed
	// consumer route union.
	discovered := protocolFamilyIDs(families)
	manifest := protocolFamilyIDs(frozen.Families)
	routes := protocolRouteFamilies(BaselineConsumerRegistry())
	if len(discovered) != minProtocolFamilies {
		t.Fatalf("live discovery names %d protocol families, want %d", len(discovered), minProtocolFamilies)
	}
	if len(discovered)-1 != minPacketFamilies {
		t.Fatalf("live discovery names %d packet families, want %d beside the frame family", len(discovered)-1, minPacketFamilies)
	}
	if len(manifest) != minProtocolFamilies {
		t.Fatalf("tracked manifest names %d protocol families, want %d", len(manifest), minProtocolFamilies)
	}
	if len(routes) != minProtocolFamilies {
		t.Fatalf("closed route union names %d protocol points, want %d", len(routes), minProtocolFamilies)
	}
	for index, id := range discovered {
		if manifest[index] != id {
			t.Fatalf("manifest protocol family %d is %s, discovery names %s", index, manifest[index], id)
		}
		if routes[index].FamilyID != id || routes[index].Version != protocolVersion {
			t.Fatalf("route union point %d is %s/%s, discovery names %s/%s", index, routes[index].FamilyID, routes[index].Version, id, protocolVersion)
		}
	}
	consumers := BaselineConsumerRegistry()
	for _, id := range discovered {
		operations := protocolRouteOperations(consumers, id, protocolVersion)
		if len(operations) != 2 || operations[0] != "decode" || operations[1] != "encode" {
			t.Fatalf("family %s carries routes %v, want decode and encode", id, operations)
		}
	}

	// Every protocol case is one checkpoint of executed evidence whose
	// consumer and packet-key direction agree with its family.
	for _, c := range frozen.Cases {
		if !strings.HasPrefix(c.Family, protocolFamilyPrefix) {
			continue
		}
		wantConsumer := "mornlea_protocol"
		if c.Family == protocolFrameFamily {
			wantConsumer = "corpus_frame"
		}
		if c.RustConsumer != wantConsumer {
			t.Fatalf("case %s names consumer %s, want %s", c.ID, c.RustConsumer, wantConsumer)
		}
		if c.Version != protocolVersion {
			t.Fatalf("case %s carries version %s, want %s", c.ID, c.Version, protocolVersion)
		}
		if c.PacketKey != nil {
			wantDirection := packetDirectionClient
			if strings.HasPrefix(c.Family, "protocol.server.") {
				wantDirection = packetDirectionServer
			}
			if c.PacketKey.Direction != wantDirection {
				t.Fatalf("case %s names direction %s, want %s", c.ID, c.PacketKey.Direction, wantDirection)
			}
		}
	}

	// Per-family minimums and the reviewed totals come from the frozen
	// evidence: every family carries a valid decode, a valid encode and at
	// least one invalid or boundary case.
	tallies, packetCases, frameCases := tallyProtocolFamilies(t, root, frozen)
	if len(tallies) != minProtocolFamilies {
		t.Fatalf("frozen protocol cases cover %d families, want %d", len(tallies), minProtocolFamilies)
	}
	for id, tally := range tallies {
		if tally.decodeOK < 1 {
			t.Fatalf("family %s carries %d valid decode cases", id, tally.decodeOK)
		}
		if tally.encodeOK < 1 {
			t.Fatalf("family %s carries %d valid encode cases", id, tally.encodeOK)
		}
		if tally.decodeError+tally.encodeError < 1 {
			t.Fatalf("family %s carries no invalid or boundary case", id)
		}
	}
	if packetCases < minPacketCases {
		t.Fatalf("frozen packet cases %d below the reviewed minimum %d", packetCases, minPacketCases)
	}
	if frameCases < minFrameCases {
		t.Fatalf("frozen frame cases %d below the reviewed minimum %d", frameCases, minFrameCases)
	}
	if packetCases+frameCases < minProtocolCases {
		t.Fatalf("frozen protocol cases %d below the reviewed minimum %d", packetCases+frameCases, minProtocolCases)
	}
	t.Logf("protocol corpus: %d families, %d packet cases, %d frame cases, %d checkpoints",
		len(tallies), packetCases, frameCases, packetCases+frameCases)

	// The closed route union reconciles: every protocol family/version point
	// is covered and none remains uncovered.
	report, err := ReconcileWorking(root, frozen, families, live, consumers, BaselineNegativeCoverageExceptions())
	if err != nil {
		t.Fatalf("frozen inventory does not reconcile under the closed route union: %v", err)
	}
	covered := make(map[CoveragePoint]bool, len(report.Covered))
	for _, pt := range report.Covered {
		covered[pt] = true
	}
	for _, id := range discovered {
		pt := CoveragePoint{FamilyID: id, Version: protocolVersion}
		if !covered[pt] {
			t.Fatalf("protocol point %s/%s is not covered", id, protocolVersion)
		}
	}
	nonProtocolUncovered := make([]string, 0, len(report.Uncovered))
	for _, pt := range report.Uncovered {
		if strings.HasPrefix(pt.FamilyID, protocolFamilyPrefix) {
			t.Fatalf("protocol point %s/%s remains uncovered", pt.FamilyID, pt.Version)
		}
		nonProtocolUncovered = append(nonProtocolUncovered, pt.FamilyID+"/"+pt.Version)
	}
	if len(nonProtocolUncovered) == 0 {
		t.Fatal("no uncovered point remains outside the protocol slice; complete acceptance must stay unclaimed")
	}
	t.Logf("non-protocol uncovered points remain visible: %s", strings.Join(nonProtocolUncovered, ", "))

	// Complete acceptance still refuses: the remaining gaps are exactly the
	// non-protocol points logged above, so this node never claims F1.
	_, completeErr := ReconcileComplete(root, frozen, families, live, consumers, BaselineNegativeCoverageExceptions())
	if completeErr == nil {
		t.Fatal("ReconcileComplete accepted the manifest; the protocol-only closure must not claim complete F1")
	}
	invErr, ok := completeErr.(*InventoryError)
	if !ok {
		t.Fatalf("ReconcileComplete error type %T, want *InventoryError", completeErr)
	}
	for _, problem := range invErr.Problems {
		if strings.Contains(problem, "uncovered point "+protocolFamilyPrefix) {
			t.Fatalf("complete acceptance reports a protocol gap: %s", problem)
		}
	}
	if frozen.SourceRevision != BaselineSourceRevision {
		t.Fatalf("frozen source revision %s does not match the Go baseline %s", frozen.SourceRevision, BaselineSourceRevision)
	}
}

// TestProtocolCorpusClosureMutationsFail pins that every drift class the
// closure names fails instead of passing silently: a zero-case family, a
// duplicate case ID, a missing route, a missing source hash, a mismatched
// digest, an unsupported version, and a case no producer executes.
func TestProtocolCorpusClosureMutationsFail(t *testing.T) {
	root, families, live := discoverLive(t)
	before := computeTrackedCorpusDigest(t, root)
	defer assertTrackedCorpusUnchanged(t, root, before)
	frozen := loadProtocolClosureInventory(t, root)
	consumers := BaselineConsumerRegistry()

	protocolCase := func(family string) (CaseSpec, int) {
		for index, c := range frozen.Cases {
			if c.Family == family {
				return c, index
			}
		}
		t.Fatalf("frozen corpus registers no case for family %s", family)
		return CaseSpec{}, -1
	}
	removeFamilyCases := func(inv *Inventory, family string) {
		kept := make([]CaseSpec, 0, len(inv.Cases))
		for _, c := range inv.Cases {
			if c.Family != family {
				kept = append(kept, c)
			}
		}
		inv.Cases = kept
		for index := range inv.Families {
			if inv.Families[index].ID == family {
				inv.Families[index].Cases = nil
			}
		}
	}

	tests := []struct {
		name string
		run  func(t *testing.T)
	}{
		{
			name: "zero_case_family",
			run: func(t *testing.T) {
				inv := cloneProtocolClosureInventory(frozen)
				removeFamilyCases(&inv, "protocol.server.KeepAlive")
				point := CoveragePoint{FamilyID: "protocol.server.KeepAlive", Version: strconv.Itoa(live.Protocol)}
				report, err := ReconcileWorking(root, inv, families, live, consumers, BaselineNegativeCoverageExceptions())
				if err != nil {
					t.Fatalf("zero-case family rejected the working reconciliation: %v", err)
				}
				for _, pt := range report.Covered {
					if pt == point {
						t.Fatalf("zero-case family %s reported covered", point.FamilyID)
					}
				}
				found := false
				for _, pt := range report.Uncovered {
					if pt == point {
						found = true
					}
				}
				if !found {
					t.Fatalf("zero-case family %s absent from the uncovered report", point.FamilyID)
				}
				_, err = ReconcileComplete(root, inv, families, live, consumers, BaselineNegativeCoverageExceptions())
				if err == nil || !strings.Contains(err.Error(), "uncovered point protocol.server.KeepAlive version") {
					t.Fatalf("complete reconciliation accepted a zero-case family: %v", err)
				}
			},
		},
		{
			name: "duplicate_case_id",
			run: func(t *testing.T) {
				inv := cloneProtocolClosureInventory(frozen)
				_, index := protocolCase("protocol.server.KeepAlive")
				inv.Cases = append(inv.Cases, inv.Cases[index])
				_, err := ReconcileWorking(root, inv, families, live, consumers, BaselineNegativeCoverageExceptions())
				if err == nil || !strings.Contains(err.Error(), "duplicate case "+inv.Cases[index].ID) {
					t.Fatalf("duplicate case ID accepted: %v", err)
				}
			},
		},
		{
			name: "missing_route",
			run: func(t *testing.T) {
				inv := cloneProtocolClosureInventory(frozen)
				for index := range inv.Cases {
					if inv.Cases[index].Family == "protocol.server.KeepAlive" {
						inv.Cases[index].Operation = "migrate"
						break
					}
				}
				_, err := ReconcileWorking(root, inv, families, live, consumers, BaselineNegativeCoverageExceptions())
				if err == nil || !strings.Contains(err.Error(), "unsupported route protocol.server.KeepAlive/45/migrate") {
					t.Fatalf("unrouted operation accepted: %v", err)
				}
			},
		},
		{
			name: "missing_source_hash",
			run: func(t *testing.T) {
				inv := cloneProtocolClosureInventory(frozen)
				for index := range inv.Families {
					if inv.Families[index].ID == "protocol.server.KeepAlive" {
						inv.Families[index].Sources[0].SHA256 = ""
						break
					}
				}
				_, err := ReconcileWorking(root, inv, families, live, consumers, BaselineNegativeCoverageExceptions())
				if err == nil || !strings.Contains(err.Error(), "invalid sha256") {
					t.Fatalf("missing source hash accepted: %v", err)
				}
			},
		},
		{
			name: "mismatched_digest",
			run: func(t *testing.T) {
				inv := cloneProtocolClosureInventory(frozen)
				for index := range inv.Cases {
					if inv.Cases[index].Family == "protocol.server.KeepAlive" {
						inv.Cases[index].Expected.SHA256 = "sha256:" + strings.Repeat("0", 64)
						break
					}
				}
				_, err := ReconcileWorking(root, inv, families, live, consumers, BaselineNegativeCoverageExceptions())
				if err == nil || !strings.Contains(err.Error(), "does not match disk") {
					t.Fatalf("mismatched expected digest accepted: %v", err)
				}
			},
		},
		{
			name: "wrong_version",
			run: func(t *testing.T) {
				inv := cloneProtocolClosureInventory(frozen)
				caseSpec, _ := protocolCase("protocol.server.KeepAlive")
				newID := "protocol.server.KeepAlive/44/unsupported-version"
				for index := range inv.Cases {
					if inv.Cases[index].ID == caseSpec.ID {
						inv.Cases[index].ID = newID
						inv.Cases[index].Version = "44"
						break
					}
				}
				for index := range inv.Families {
					if inv.Families[index].ID == "protocol.server.KeepAlive" {
						for position, id := range inv.Families[index].Cases {
							if id == caseSpec.ID {
								inv.Families[index].Cases[position] = newID
							}
						}
						break
					}
				}
				_, err := ReconcileWorking(root, inv, families, live, consumers, BaselineNegativeCoverageExceptions())
				if err == nil || !strings.Contains(err.Error(), "supported_versions") {
					t.Fatalf("unsupported case version accepted: %v", err)
				}
			},
		},
		{
			name: "unexecuted_case",
			run: func(t *testing.T) {
				caseSpec, _ := protocolCase("protocol.server.KeepAlive")
				observations, err := runOneProtocolCase(t, root, frozen, caseSpec, negotiationCorpusRoutes())
				if err == nil || !strings.Contains(err.Error(), "has no registered Go producer") {
					t.Fatalf("case without an executing producer accepted: %v", err)
				}
				if len(observations) != 0 {
					t.Fatalf("unexecuted case produced %d observations", len(observations))
				}
			},
		},
	}
	for _, tc := range tests {
		t.Run(tc.name, func(t *testing.T) {
			tc.run(t)
		})
	}
}

// protocolClosureGroup pairs one packet producer group with the reviewed case
// this closure executes for its key and value evidence.
type protocolClosureGroup struct {
	name   string
	caseID string
	routes func() map[ConsumerRoute]GoOperation
}

// protocolClosureGroups names every packet producer group exactly once, so the
// key-mutation evidence and the Rust value-mutation evidence cover the same
// closed set of groups.
func protocolClosureGroups() []protocolClosureGroup {
	return []protocolClosureGroup{
		{name: "negotiation", caseID: "protocol.client.ClientHello/45/decode-current-version", routes: negotiationCorpusRoutes},
		{name: "control", caseID: "protocol.client.KeepAliveReply/45/decode-valid", routes: controlCorpusRoutes},
		{name: "client-control", caseID: "protocol.client.PlaceBlock/45/decode-valid", routes: clientControlCorpusRoutes},
		{name: "client-rays", caseID: "protocol.client.BoneMeal/45/decode-valid", routes: clientRayCorpusRoutes},
		{name: "client-inventory", caseID: "protocol.client.CloseContainer/45/decode-valid", routes: clientInventoryCorpusRoutes},
		{name: "client-stack-views", caseID: "protocol.client.DropStack/45/decode-valid", routes: clientStackViewsCorpusRoutes},
		{name: "client-chat", caseID: "protocol.client.ChatCommand/45/decode-max-text", routes: clientChatCorpusRoutes},
		{name: "world-delta", caseID: "protocol.server.BlockChanges/45/decode-empty-barrier", routes: worldDeltaCorpusRoutes},
		{name: "snapshot", caseID: "protocol.server.ChunkSnapshot/45/decode-fixture", routes: snapshotCorpusRoutes},
		{name: "player-outcomes", caseID: "protocol.server.CombatHit/45/decode-kind-three", routes: playerOutcomesCorpusRoutes},
		{name: "inventory-publication", caseID: "protocol.server.CraftingState/45/decode-valid", routes: inventoryPublicationCorpusRoutes},
		{name: "remote-players", caseID: "protocol.server.RemotePlayerDespawn/45/decode-valid", routes: remotePlayersCorpusRoutes},
		{name: "companions", caseID: "protocol.server.CompanionDespawn/45/decode-valid", routes: companionsCorpusRoutes},
		{name: "drops", caseID: "protocol.server.ItemDropRemoves/45/decode-count-one", routes: itemDropsCorpusRoutes},
		{name: "hostiles", caseID: "protocol.server.HostileDespawn/45/decode-count-sixty-four", routes: hostilesCorpusRoutes},
		{name: "passives", caseID: "protocol.server.PassiveDespawn/45/decode-valid", routes: passivesCorpusRoutes},
		{name: "projectiles", caseID: "protocol.server.ProjectileDespawn/45/decode-count-one-hundred-twenty-eight", routes: projectilesCorpusRoutes},
		{name: "chat-event", caseID: "protocol.server.ChatEvent/45/decode-accepted", routes: chatEventCorpusRoutes},
	}
}

// packetKeyRefusalMarkers lists the producer-group refusal messages a mutated
// packet key can answer with. The negotiation group resolves its two states
// through state-named messages while every play group resolves through the
// registered-key message, so the closure accepts the reviewed set rather than
// one wording.
var packetKeyRefusalMarkers = []string{
	"names key",
	"names handshake key",
	"names login key",
	"names unknown state",
}

// errorNamesAnyMarker reports whether one error names any reviewed refusal
// marker, so a mutated case is refused at its key boundary rather than
// accepted.
func errorNamesAnyMarker(err error, markers []string) bool {
	if err == nil {
		return false
	}
	text := err.Error()
	for _, marker := range markers {
		if strings.Contains(text, marker) {
			return true
		}
	}
	return false
}

// TestProtocolCorpusGroupKeysGateExecution pins that every producer group
// resolves its case's own packet key and refuses an altered key, a wrong
// direction and a wrong state, so a case can never be executed under a key its
// family does not own. The unmutated case executes first and reproduces the
// frozen expectation, which proves the refusals come from the mutation rather
// than from a case that never ran.
func TestProtocolCorpusGroupKeysGateExecution(t *testing.T) {
	root, _, _ := discoverLive(t)
	before := computeTrackedCorpusDigest(t, root)
	defer assertTrackedCorpusUnchanged(t, root, before)
	frozen := loadProtocolClosureInventory(t, root)

	byID := make(map[string]CaseSpec, len(frozen.Cases))
	for _, c := range frozen.Cases {
		byID[c.ID] = c
	}
	for _, group := range protocolClosureGroups() {
		t.Run(group.name, func(t *testing.T) {
			caseSpec, owned := byID[group.caseID]
			if !owned {
				t.Fatalf("frozen corpus registers no case %s", group.caseID)
			}
			if caseSpec.PacketKey == nil {
				t.Fatalf("case %s carries no packet key", group.caseID)
			}
			routes := group.routes()
			observations, err := runOneProtocolCase(t, root, frozen, caseSpec, routes)
			if err != nil {
				t.Fatalf("unmutated case %s failed execution: %v", group.caseID, err)
			}
			if len(observations) != 1 {
				t.Fatalf("case %s produced %d observations, want 1", group.caseID, len(observations))
			}
			expected := readExpectedOutcome(t, root, caseSpec)
			if !outcomesEqual(observations[0].Outcome, expected) {
				t.Fatalf("case %s produced %#v, want %#v", group.caseID, observations[0].Outcome, expected)
			}

			mutations := []struct {
				name   string
				mutate func(key *PacketKeySpec)
				want   []string
			}{
				{
					name: "altered_key",
					mutate: func(key *PacketKeySpec) {
						key.ID++
					},
					want: packetKeyRefusalMarkers,
				},
				{
					name: "wrong_direction",
					mutate: func(key *PacketKeySpec) {
						if key.Direction == packetDirectionClient {
							key.Direction = packetDirectionServer
						} else {
							key.Direction = packetDirectionClient
						}
					},
					want: packetKeyRefusalMarkers,
				},
				{
					name: "wrong_state",
					mutate: func(key *PacketKeySpec) {
						if key.State == packetStatePlay {
							key.State = "login"
						} else {
							key.State = packetStatePlay
						}
					},
					want: packetKeyRefusalMarkers,
				},
			}
			for _, mutation := range mutations {
				mutated := caseSpec
				key := *caseSpec.PacketKey
				mutation.mutate(&key)
				mutated.PacketKey = &key
				_, err := runOneProtocolCase(t, root, frozen, mutated, routes)
				if err == nil || !errorNamesAnyMarker(err, mutation.want) {
					t.Fatalf("%s accepted a %s mutation: %v", group.name, mutation.name, err)
				}
			}
		})
	}
}

// TestProtocolCorpusNonProtocolEvidenceUnchanged pins that the protocol
// closure left every domain and agent case exactly where the reviewed corpus
// carries it: the exact totals hold, and every case still reconciles under the
// closed consumer registry.
func TestProtocolCorpusNonProtocolEvidenceUnchanged(t *testing.T) {
	root, families, live := discoverLive(t)
	before := computeTrackedCorpusDigest(t, root)
	defer assertTrackedCorpusUnchanged(t, root, before)
	frozen := loadProtocolClosureInventory(t, root)

	domainCases, agentCases, protocolCases, saveCases := 0, 0, 0, 0
	for _, c := range frozen.Cases {
		switch {
		case strings.HasPrefix(c.Family, "domain."):
			domainCases++
		case strings.HasPrefix(c.Family, "agent."):
			agentCases++
		case strings.HasPrefix(c.Family, protocolFamilyPrefix):
			protocolCases++
		case strings.HasPrefix(c.Family, "save."):
			saveCases++
			if c.Family != "save.region" && c.Family != "save.player" && c.Family != "save.world-metadata" {
				t.Fatalf("save case %s has family %s, want save.region, save.player, or save.world-metadata", c.ID, c.Family)
			}
		default:
			t.Fatalf("case %s belongs to no reviewed corpus slice", c.ID)
		}
	}
	if domainCases != 534 || agentCases != 154 || protocolCases != 436 || saveCases != 78 {
		t.Fatalf("corpus totals drifted: domain %d, agent %d, protocol %d, save %d", domainCases, agentCases, protocolCases, saveCases)
	}
	if _, err := ReconcileWorking(root, frozen, families, live, BaselineConsumerRegistry(), BaselineNegativeCoverageExceptions()); err != nil {
		t.Fatalf("frozen inventory no longer reconciles the non-protocol evidence: %v", err)
	}
}

// TestProtocolCorpusCompleteRejectsAlteredGoSourceByte pins the production
// source-hash-content branch: a working-manifest fixture whose family
// provenance sources reconcile clean must refuse once one byte of a staged
// source changes on disk while the recorded hash stays, so a silently edited
// Go source can never keep its frozen corpus coverage. The staged source lives
// in the fixture's temporary directory, so no tracked file is touched.
func TestProtocolCorpusCompleteRejectsAlteredGoSourceByte(t *testing.T) {
	fixture := newInventoryAssetFixture(t, "binary", []byte{0x01}, false)
	source := fixture.inventory.Families[0].Sources[0]
	if source.Path == "" {
		t.Fatal("fixture registers no family provenance source")
	}
	if _, err := ReconcileWorking(
		fixture.root,
		fixture.inventory,
		fixture.families,
		fixture.live,
		BaselineConsumerRegistry(),
		BaselineNegativeCoverageExceptions(),
	); err != nil {
		t.Fatalf("unmutated fixture does not reconcile: %v", err)
	}

	staged := filepath.Join(fixture.root, filepath.FromSlash(source.Path))
	data, err := os.ReadFile(staged)
	if err != nil {
		t.Fatalf("read staged source %s: %v", source.Path, err)
	}
	if len(data) == 0 {
		t.Fatalf("staged source %s carries no byte to alter", source.Path)
	}
	data[0] ^= 0x01
	if err := os.WriteFile(staged, data, 0o644); err != nil {
		t.Fatalf("rewrite staged source %s: %v", source.Path, err)
	}

	_, err = ReconcileWorking(
		fixture.root,
		fixture.inventory,
		fixture.families,
		fixture.live,
		BaselineConsumerRegistry(),
		BaselineNegativeCoverageExceptions(),
	)
	if err == nil {
		t.Fatal("altered source byte accepted by the working reconciliation")
	}
	text := err.Error()
	if !strings.Contains(text, source.Path) || !strings.Contains(text, "does not match disk") {
		t.Fatalf("altered source byte refusal %v does not name %s with its disk mismatch", text, source.Path)
	}
}

// TestProtocolCorpusClosureCandidateExport stages the one-time source revision
// refresh as an external candidate. The tracked manifest keeps its recorded
// revision; the candidate names the current HEAD revision so the controller
// can verify it at integration and then integrate both sides together. An
// unset export variable writes nothing.
// The candidate publishes through the reviewed create-exclusive exporter
// `writeDomainEventManifestCandidate`, reusing its registered producer path
// (runtime-oracle/domain-event-manifest) so this node adds no exporter identity.
func TestProtocolCorpusClosureCandidateExport(t *testing.T) {
	root := mustRepoRoot(t)
	before := computeTrackedCorpusDigest(t, root)
	defer assertTrackedCorpusUnchanged(t, root, before)
	frozen := loadProtocolClosureInventory(t, root)

	exportRoot := strings.TrimSpace(os.Getenv(runtimeOracleExportDirEnv))
	if exportRoot == "" {
		return
	}
	revision := gitHeadRevision(t, root)
	refreshed := cloneProtocolClosureInventory(frozen)
	refreshed.SourceRevision = revision

	candidate := writeDomainEventManifestCandidate(t, root, exportRoot, refreshed)
	if candidate == "" {
		t.Fatal("manifest candidate export was rejected")
	}
	reloaded, err := LoadInventory(candidate)
	if err != nil {
		t.Fatalf("reload manifest candidate: %v", err)
	}
	if reloaded.SourceRevision != revision {
		t.Fatalf("candidate source revision %s does not match HEAD %s", reloaded.SourceRevision, revision)
	}
	families, discovered, err := Discover(root)
	if err != nil {
		t.Fatalf("discover registries: %v", err)
	}
	if _, err := ReconcileWorking(root, reloaded, families, discovered, BaselineConsumerRegistry(), BaselineNegativeCoverageExceptions()); err != nil {
		t.Fatalf("manifest candidate does not reconcile: %v", err)
	}
	t.Logf("protocol closure candidate %s carries source revision %s", candidate, revision)
}

// gitHeadRevision reads the current repository HEAD revision.
func gitHeadRevision(t *testing.T, root string) string {
	t.Helper()
	command := exec.Command("git", "-C", root, "rev-parse", "HEAD")
	output, err := command.Output()
	if err != nil {
		t.Fatalf("read git HEAD revision: %v", err)
	}
	revision := strings.TrimSpace(string(output))
	if len(revision) != 40 || strings.TrimLeft(revision, "0123456789abcdef") != "" {
		t.Fatalf("git HEAD revision %q is not 40 lowercase hexadecimal digits", revision)
	}
	return revision
}

// stageProtocolCaseRoot copies one case's input asset into a scratch root so a
// producer executes exactly the bytes the manifest names.
func stageProtocolCaseRoot(t *testing.T, root string, c CaseSpec) string {
	t.Helper()
	staged := t.TempDir()
	target := filepath.Join(staged, filepath.FromSlash(c.Input.Path))
	if err := os.MkdirAll(filepath.Dir(target), 0o755); err != nil {
		t.Fatalf("create staged input parent: %v", err)
	}
	data, err := os.ReadFile(filepath.Join(root, filepath.FromSlash(c.Input.Path)))
	if err != nil {
		t.Fatalf("read frozen input %s: %v", c.Input.Path, err)
	}
	if err := os.WriteFile(target, data, 0o644); err != nil {
		t.Fatalf("stage frozen input %s: %v", c.Input.Path, err)
	}
	return staged
}

// runOneProtocolCase executes exactly one frozen case through one producer
// group's route map and returns the executed observations.
func runOneProtocolCase(
	t *testing.T,
	root string,
	frozen Inventory,
	c CaseSpec,
	routes map[ConsumerRoute]GoOperation,
) ([]ExecutedObservation, error) {
	t.Helper()
	staged := stageProtocolCaseRoot(t, root, c)
	manifest := Inventory{
		SchemaVersion:  frozen.SchemaVersion,
		SourceRevision: frozen.SourceRevision,
		Identities:     frozen.Identities,
		Families:       make([]Family, 0, 1),
		Cases:          []CaseSpec{c},
	}
	for _, family := range frozen.Families {
		if family.ID != c.Family {
			continue
		}
		family.SupportedVersions = append([]string(nil), family.SupportedVersions...)
		family.Sources = append([]SourceSpec(nil), family.Sources...)
		family.Cases = []string{c.ID}
		manifest.Families = append(manifest.Families, family)
	}
	if len(manifest.Families) != 1 {
		t.Fatalf("frozen manifest carries no family row for case %s", c.ID)
	}
	return RunProtocolCases(staged, manifest, routes)
}

// TestProtocolCorpusClosureGroupsAreClosed pins that the closure group table
// names every packet producer group exactly once, so the key and value
// mutation evidence cannot silently drop a group.
func TestProtocolCorpusClosureGroupsAreClosed(t *testing.T) {
	groups := protocolClosureGroups()
	if len(groups) != 18 {
		t.Fatalf("closure names %d producer groups, want 18", len(groups))
	}
	seen := make(map[string]bool, len(groups))
	for _, group := range groups {
		if seen[group.name] {
			t.Fatalf("producer group %s is registered twice", group.name)
		}
		seen[group.name] = true
		if group.routes == nil {
			t.Fatalf("producer group %s registers no route map", group.name)
		}
		if !strings.HasPrefix(group.caseID, "protocol.") {
			t.Fatalf("producer group %s names case %s outside the protocol slice", group.name, group.caseID)
		}
	}
}
