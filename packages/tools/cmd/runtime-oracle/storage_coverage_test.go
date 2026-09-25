package main

import (
	"path/filepath"
	"sort"
	"strings"
	"testing"
)

const closedSaveSourceRevision = "138183c5cf7e1aac4974233e303a2d9df0c202d6"

var closedSaveFamilyCounts = map[string]int{
	"save.region":         27,
	"save.player":         37,
	"save.world-metadata": 21,
	"save.hostile":        50,
	"save.passive":        70,
	"save.chunk":          35,
	"save.companion":      29,
}

const closedSaveCaseTotal = 269

// loadStorageClosureInventory loads the tracked corpus manifest for save closure.
func loadStorageClosureInventory(t *testing.T, root string) Inventory {
	t.Helper()
	frozen, err := LoadInventory(filepath.Join(root, filepath.FromSlash(InventoryRelPath)))
	if err != nil {
		t.Fatalf("load frozen inventory: %v", err)
	}
	return frozen
}

// cloneStorageClosureInventory deep-copies a manifest for mutation tests.
func cloneStorageClosureInventory(frozen Inventory) Inventory {
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

func storageConsumerRoutes(consumers ConsumerRegistry) []ConsumerRoute {
	registration := consumers["mornlea_storage"]
	routes := make([]ConsumerRoute, 0, len(registration.Routes))
	for route := range registration.Routes {
		routes = append(routes, route)
	}
	sort.Slice(routes, func(i, j int) bool {
		if routes[i].FamilyID != routes[j].FamilyID {
			return routes[i].FamilyID < routes[j].FamilyID
		}
		if routes[i].Version != routes[j].Version {
			return routes[i].Version < routes[j].Version
		}
		return routes[i].Operation < routes[j].Operation
	})
	return routes
}

type routeKindTally struct {
	ok    bool
	error bool
}

func flipLastSHA256HexDigit(digest string) string {
	if len(digest) != len("sha256:")+64 {
		return digest + "x"
	}
	last := digest[len(digest)-1]
	replacement := 'a'
	if last == 'a' {
		replacement = 'b'
	}
	return digest[:len(digest)-1] + string(replacement)
}

func saveCaseByFilter(t *testing.T, frozen Inventory, match func(CaseSpec) bool) CaseSpec {
	t.Helper()
	for _, c := range frozen.Cases {
		if !strings.HasPrefix(c.Family, "save.") {
			continue
		}
		if match(c) {
			return c
		}
	}
	t.Fatal("frozen corpus has no matching save case")
	return CaseSpec{}
}

func reconcileExpectError(t *testing.T, root string, inv Inventory, families []Family, live Identities, consumers ConsumerRegistry, wantSubstrings ...string) {
	t.Helper()
	_, err := ReconcileWorking(root, inv, families, live, consumers, BaselineNegativeCoverageExceptions())
	if err == nil {
		t.Fatalf("mutation was accepted: want error containing %v", wantSubstrings)
	}
	text := err.Error()
	for _, want := range wantSubstrings {
		if !strings.Contains(text, want) {
			t.Fatalf("mutation error %q missing %q", text, want)
		}
	}
}

// TestStorageCorpus pins the save-only zero-gap state from the frozen manifest.
func TestStorageCorpus(t *testing.T) {
	root, families, live := discoverLive(t)
	frozen := loadStorageClosureInventory(t, root)

	if frozen.SourceRevision != BaselineSourceRevision {
		t.Fatalf("frozen source revision %s does not match Go baseline %s", frozen.SourceRevision, BaselineSourceRevision)
	}
	if frozen.SourceRevision != closedSaveSourceRevision {
		t.Fatalf("frozen source revision %s does not match closed save revision %s", frozen.SourceRevision, closedSaveSourceRevision)
	}
	if BaselineSourceRevision != closedSaveSourceRevision {
		t.Fatalf("Go baseline %s does not match closed save revision %s", BaselineSourceRevision, closedSaveSourceRevision)
	}

	consumers := BaselineConsumerRegistry()
	if _, err := ReconcileWorking(root, frozen, families, live, consumers, BaselineNegativeCoverageExceptions()); err != nil {
		t.Fatalf("frozen inventory drifted from current registries: %v", err)
	}

	reloaded, err := LoadInventory(filepath.Join(root, filepath.FromSlash(InventoryRelPath)))
	if err != nil {
		t.Fatalf("reload frozen inventory: %v", err)
	}
	if reloaded.SourceRevision != frozen.SourceRevision {
		t.Fatalf("reloaded source revision %s != in-memory %s", reloaded.SourceRevision, frozen.SourceRevision)
	}
	if reloaded.SourceRevision != BaselineSourceRevision {
		t.Fatalf("reloaded source revision %s != Go baseline %s", reloaded.SourceRevision, BaselineSourceRevision)
	}

	report, err := ReconcileWorking(root, frozen, families, live, consumers, BaselineNegativeCoverageExceptions())
	if err != nil {
		t.Fatalf("reconcile for coverage report: %v", err)
	}
	for _, pt := range report.Uncovered {
		if !strings.HasPrefix(pt.FamilyID, "save.") {
			continue
		}
		t.Fatalf("save point %s/%s remains uncovered", pt.FamilyID, pt.Version)
	}

	counts := make(map[string]int)
	for _, c := range frozen.Cases {
		if !strings.HasPrefix(c.Family, "save.") {
			continue
		}
		counts[c.Family]++
	}
	total := 0
	for family, want := range closedSaveFamilyCounts {
		got := counts[family]
		if got != want {
			t.Fatalf("family %s carries %d cases, want %d", family, got, want)
		}
		total += got
	}
	if total != closedSaveCaseTotal {
		t.Fatalf("save case total %d, want %d", total, closedSaveCaseTotal)
	}

	routes := storageConsumerRoutes(consumers)
	routeCases := make(map[ConsumerRoute]int)
	routeKinds := make(map[ConsumerRoute]*routeKindTally)
	for _, route := range routes {
		routeKinds[route] = &routeKindTally{}
	}
	for _, c := range frozen.Cases {
		if !strings.HasPrefix(c.Family, "save.") {
			continue
		}
		route := ConsumerRoute{FamilyID: c.Family, Version: c.Version, Operation: c.Operation}
		routeCases[route]++
		kind, err := decodeExpectedOutcome(root, c)
		if err != nil {
			t.Fatalf("read expected outcome for %s: %v", c.ID, err)
		}
		tally := routeKinds[route]
		if tally == nil {
			continue
		}
		switch kind.Kind {
		case "ok":
			tally.ok = true
		case "error":
			tally.error = true
		}
	}
	for _, route := range routes {
		if routeCases[route] == 0 {
			t.Fatalf("route %s/%s/%s has no case", route.FamilyID, route.Version, route.Operation)
		}
		tally := routeKinds[route]
		switch route.Operation {
		case "decode":
			if !tally.ok || !tally.error {
				t.Fatalf("route %s/%s/decode missing ok=%v error=%v evidence", route.FamilyID, route.Version, tally.ok, tally.error)
			}
		case "encode":
			if route.FamilyID == "save.chunk" && route.Version == "9" {
				if !tally.ok {
					t.Fatalf("route save.chunk/9/encode missing kind=ok evidence")
				}
				continue
			}
			if !tally.ok || !tally.error {
				t.Fatalf("route %s/%s/encode missing ok=%v error=%v evidence", route.FamilyID, route.Version, tally.ok, tally.error)
			}
		}
	}
}

// TestStorageCorpusMutationsFail pins every save inventory drift class named by closure.
func TestStorageCorpusMutationsFail(t *testing.T) {
	root, families, live := discoverLive(t)
	frozen := loadStorageClosureInventory(t, root)
	consumers := BaselineConsumerRegistry()

	tests := []struct {
		name string
		run  func(t *testing.T)
	}{
		{
			name: "input_digest",
			run: func(t *testing.T) {
				inv := cloneStorageClosureInventory(frozen)
				target := saveCaseByFilter(t, frozen, func(c CaseSpec) bool { return strings.HasPrefix(c.Family, "save.") })
				for index := range inv.Cases {
					if inv.Cases[index].ID != target.ID {
						continue
					}
					inv.Cases[index].Input.SHA256 = flipLastSHA256HexDigit(inv.Cases[index].Input.SHA256)
					break
				}
				reconcileExpectError(t, root, inv, families, live, consumers, "sha256", target.ID)
			},
		},
		{
			name: "encoded_reference",
			run: func(t *testing.T) {
				inv := cloneStorageClosureInventory(frozen)
				target := saveCaseByFilter(t, frozen, func(c CaseSpec) bool {
					if c.Family == "save.chunk" || c.Operation != "encode" || c.Encoded == nil {
						return false
					}
					kind, err := decodeExpectedOutcome(root, c)
					return err == nil && kind.Kind == "ok"
				})
				for index := range inv.Cases {
					if inv.Cases[index].ID != target.ID {
						continue
					}
					inv.Cases[index].Encoded.SHA256 = flipLastSHA256HexDigit(inv.Cases[index].Encoded.SHA256)
					break
				}
				reconcileExpectError(t, root, inv, families, live, consumers, "sha256", target.ID)
			},
		},
		{
			name: "version",
			run: func(t *testing.T) {
				inv := cloneStorageClosureInventory(frozen)
				target := saveCaseByFilter(t, frozen, func(c CaseSpec) bool { return c.Family == "save.player" })
				if !strings.HasPrefix(target.ID, "save.player/"+target.Version+"/") {
					t.Fatalf("unexpected player case id %s", target.ID)
				}
				suffix := strings.TrimPrefix(target.ID, "save.player/"+target.Version+"/")
				newID := "save.player/10/" + suffix
				for index := range inv.Cases {
					if inv.Cases[index].ID != target.ID {
						continue
					}
					inv.Cases[index].ID = newID
					inv.Cases[index].Version = "10"
					break
				}
				for index := range inv.Families {
					if inv.Families[index].ID != "save.player" {
						continue
					}
					for position, id := range inv.Families[index].Cases {
						if id == target.ID {
							inv.Families[index].Cases[position] = newID
						}
					}
				}
				reconcileExpectError(t, root, inv, families, live, consumers, "supported_versions")
			},
		},
		{
			name: "route",
			run: func(t *testing.T) {
				inv := cloneStorageClosureInventory(frozen)
				target := saveCaseByFilter(t, frozen, func(c CaseSpec) bool {
					return c.Family == "save.world-metadata" && c.Version == "1" && c.Operation == "decode"
				})
				for index := range inv.Cases {
					if inv.Cases[index].ID != target.ID {
						continue
					}
					inv.Cases[index].Operation = "encode"
					break
				}
				reconcileExpectError(t, root, inv, families, live, consumers, "unsupported route")
			},
		},
		{
			name: "delete",
			run: func(t *testing.T) {
				inv := cloneStorageClosureInventory(frozen)
				target := saveCaseByFilter(t, frozen, func(c CaseSpec) bool { return strings.HasPrefix(c.Family, "save.") })
				kept := make([]CaseSpec, 0, len(inv.Cases)-1)
				for _, c := range inv.Cases {
					if c.ID != target.ID {
						kept = append(kept, c)
					}
				}
				inv.Cases = kept
				reconcileExpectError(t, root, inv, families, live, consumers, "do not match")
			},
		},
		{
			name: "checkpoints_two",
			run: func(t *testing.T) {
				inv := cloneStorageClosureInventory(frozen)
				target := saveCaseByFilter(t, frozen, func(c CaseSpec) bool { return strings.HasPrefix(c.Family, "save.") })
				for index := range inv.Cases {
					if inv.Cases[index].ID != target.ID {
						continue
					}
					inv.Cases[index].Checkpoints = []string{"0", "1"}
					break
				}
				reconcileExpectError(t, root, inv, families, live, consumers, "checkpoints", target.ID)
			},
		},
		{
			name: "checkpoints_nonzero_only",
			run: func(t *testing.T) {
				inv := cloneStorageClosureInventory(frozen)
				target := saveCaseByFilter(t, frozen, func(c CaseSpec) bool { return strings.HasPrefix(c.Family, "save.") })
				for index := range inv.Cases {
					if inv.Cases[index].ID != target.ID {
						continue
					}
					inv.Cases[index].Checkpoints = []string{"1"}
					break
				}
				reconcileExpectError(t, root, inv, families, live, consumers, "checkpoints", target.ID)
			},
		},
	}
	for _, tc := range tests {
		t.Run(tc.name, func(t *testing.T) {
			tc.run(t)
		})
	}
}
