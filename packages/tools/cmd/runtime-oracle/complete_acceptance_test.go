package main

import (
	"fmt"
	"os"
	"path/filepath"
	"reflect"
	"slices"
	"strings"
	"testing"
)

// `foundationInputSources` binds authority observations and independent ordering
// checks to the exact reviewed input contract, including consumer exclusions.
var foundationInputSources = []string{
	"packages/engine/crates/mornlea_domain/src/input/order.rs",
	"packages/engine/crates/mornlea_domain/tests/command_order.rs",
	"packages/engine/crates/mornlea_domain/tests/corpus_domain.rs",
	"packages/server/sim/runtime/command_order_oracle_test.go",
	"packages/server/sim/runtime/engine_step.go",
	"packages/shared/network/protocol/message_command.go",
	"packages/shared/network/protocol/packet_test.go",
	"packages/tools/cmd/runtime-oracle/inventory_test.go",
	"packages/tools/cmd/runtime-oracle/protocol_coverage_test.go",
	"packages/tools/cmd/runtime-oracle/protocol_frame_test.go",
}

func reconcileFoundationAcceptance(root string, inventory Inventory, families []Family, live Identities, consumers ConsumerRegistry) (CoverageReport, error) {
	report, err := ReconcileComplete(root, inventory, families, live, consumers, BaselineNegativeCoverageExceptions())
	if err != nil {
		return report, err
	}
	for _, family := range inventory.Families {
		if family.ID != "domain.input" {
			continue
		}
		paths := make([]string, 0, len(family.Sources))
		for _, source := range family.Sources {
			paths = append(paths, source.Path)
		}
		slices.Sort(paths)
		if !slices.Equal(paths, foundationInputSources) {
			return report, fmt.Errorf("domain.input source bindings: got %v, want %v", paths, foundationInputSources)
		}
		return report, nil
	}
	return report, fmt.Errorf("missing domain.input source bindings")
}

func TestRuntimeFoundationCompleteAcceptance(t *testing.T) {
	root, families, live := discoverLive(t)
	frozen := loadProtocolClosureInventory(t, root)
	report, err := reconcileFoundationAcceptance(root, frozen, families, live, BaselineConsumerRegistry())
	if err != nil {
		t.Fatalf("complete foundation acceptance: %v", err)
	}
	want := make([]CoveragePoint, 0)
	for _, family := range families {
		for _, version := range family.SupportedVersions {
			want = append(want, CoveragePoint{FamilyID: family.ID, Version: version})
		}
	}
	sortCoveragePoints(want)
	if len(want) == 0 || len(report.Uncovered) != 0 || !reflect.DeepEqual(report.Covered, want) {
		t.Fatalf("coverage: covered=%v uncovered=%v discovered=%v", report.Covered, report.Uncovered, want)
	}
	digest, err := CanonicalCorpusDigest(frozen)
	if err != nil {
		t.Fatal(err)
	}
	t.Logf("accepted %d families, %d supported points, %d cases; uncovered=0; corpus=%s", len(families), len(want), len(frozen.Cases), digest)
}

func TestRuntimeFoundationCompleteAcceptanceMutations(t *testing.T) {
	root, families, live := discoverLive(t)
	frozen := loadProtocolClosureInventory(t, root)
	if _, err := reconcileFoundationAcceptance(root, frozen, families, live, BaselineConsumerRegistry()); err != nil {
		t.Fatalf("mutation baseline must first complete: %v", err)
	}
	before := computeTrackedCorpusDigest(t, root)
	t.Cleanup(func() { assertTrackedCorpusUnchanged(t, root, before) })
	removeCases := func(inventory *Inventory, all bool) {
		inventory.Cases = slices.DeleteFunc(inventory.Cases, func(c CaseSpec) bool {
			return c.Family == "domain.input" && (all || c.ID == "domain.input/45/stale-sequence-no-effect")
		})
		for i := range inventory.Families {
			if inventory.Families[i].ID == "domain.input" {
				inventory.Families[i].Cases = slices.DeleteFunc(inventory.Families[i].Cases, func(id string) bool {
					return all || id == "domain.input/45/stale-sequence-no-effect"
				})
			}
		}
	}
	check := func(t *testing.T, candidateRoot string, inventory Inventory, consumers ConsumerRegistry, marker string) {
		t.Helper()
		_, err := reconcileFoundationAcceptance(candidateRoot, inventory, families, live, consumers)
		if err == nil || !strings.Contains(err.Error(), marker) {
			t.Fatalf("want refusal %q, got %v", marker, err)
		}
	}
	t.Run("missing-failure-case", func(t *testing.T) {
		candidate := cloneProtocolClosureInventory(frozen)
		removeCases(&candidate, false)
		check(t, root, candidate, BaselineConsumerRegistry(), "uncovered point domain.input version 45")
	})
	for _, path := range foundationInputSources {
		t.Run("removed-source/"+path, func(t *testing.T) {
			candidate := cloneProtocolClosureInventory(frozen)
			for i := range candidate.Families {
				if candidate.Families[i].ID == "domain.input" {
					candidate.Families[i].Sources = slices.DeleteFunc(candidate.Families[i].Sources, func(source SourceSpec) bool { return source.Path == path })
				}
			}
			check(t, root, candidate, BaselineConsumerRegistry(), "domain.input source bindings")
		})
	}
	t.Run("changed-expected-bytes", func(t *testing.T) {
		candidateRoot := foundationFixtureRoot(t, root, frozen)
		c := frozen.Cases[caseIndexByID(t, frozen.Cases, "domain.input/45/stale-sequence-no-effect")]
		path := filepath.Join(candidateRoot, filepath.FromSlash(c.Expected.Path))
		data, err := os.ReadFile(path)
		if err != nil {
			t.Fatal(err)
		}
		if err := os.WriteFile(path, append(data, '\n'), 0600); err != nil {
			t.Fatal(err)
		}
		check(t, candidateRoot, frozen, BaselineConsumerRegistry(), "sha256")
	})
	t.Run("missing-route", func(t *testing.T) {
		consumers := BaselineConsumerRegistry()
		delete(consumers["external:runtime-authority"].Routes, ConsumerRoute{FamilyID: "domain.input", Version: "45", Operation: "order"})
		check(t, root, frozen, consumers, "consumer registry entry \"external:runtime-authority\" has no routes")
	})
	t.Run("empty-family", func(t *testing.T) {
		candidate := cloneProtocolClosureInventory(frozen)
		removeCases(&candidate, true)
		check(t, root, candidate, BaselineConsumerRegistry(), "uncovered point domain.input version 45")
	})
}

// `foundationFixtureRoot` copies only referenced evidence into temporary storage;
// destructive mutations never touch the tracked corpus or producer sources.
func foundationFixtureRoot(t *testing.T, root string, inventory Inventory) string {
	t.Helper()
	target := t.TempDir()
	paths := make(map[string]struct{})
	for _, family := range inventory.Families {
		for _, source := range family.Sources {
			paths[source.Path] = struct{}{}
		}
	}
	for _, c := range inventory.Cases {
		paths[c.Input.Path] = struct{}{}
		paths[c.Expected.Path] = struct{}{}
		if c.Encoded != nil {
			paths[c.Encoded.Path] = struct{}{}
		}
	}
	for path := range paths {
		data, err := os.ReadFile(filepath.Join(root, filepath.FromSlash(path)))
		if err != nil {
			t.Fatal(err)
		}
		destination := filepath.Join(target, filepath.FromSlash(path))
		if err := os.MkdirAll(filepath.Dir(destination), 0700); err != nil {
			t.Fatal(err)
		}
		if err := os.WriteFile(destination, data, 0600); err != nil {
			t.Fatal(err)
		}
	}
	return target
}
