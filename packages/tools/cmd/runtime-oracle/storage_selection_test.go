package main

import (
	"bytes"
	"encoding/json"
	"fmt"
	"os"
	"path/filepath"
	"reflect"
	"sort"
	"strings"
)

// This file holds the test-only storage manifest candidate helpers. Reviewed
// storage selections are merged outside the tracked corpus the same way packet
// producer groups use protocol_manifest_test.go.

const (
	storageSelectionManifest = "selection.json"
	storageConsumerName      = "mornlea_storage"
)

// StorageSelection is one storage producer's reviewed registration handoff.
type StorageSelection struct {
	ProducerID string          `json:"producer_id"`
	Cases      []CaseSpec      `json:"cases"`
	Sources    []SourceSpec    `json:"sources"`
	Routes     []ConsumerRoute `json:"routes"`
}

type consumerRouteJSON struct {
	FamilyID  string `json:"family_id"`
	Version   string `json:"version"`
	Operation string `json:"operation"`
}

// readStorageSelection loads and validates one external candidate directory.
func readStorageSelection(candidateDir string) (StorageSelection, error) {
	repoRoot, err := RepositoryRoot()
	if err != nil {
		return StorageSelection{}, err
	}
	if err := validateExternalCandidateRoot(repoRoot, candidateDir); err != nil {
		return StorageSelection{}, err
	}
	manifestPath := filepath.Join(candidateDir, storageSelectionManifest)
	info, err := os.Lstat(manifestPath)
	if err != nil {
		return StorageSelection{}, fmt.Errorf("runtime-oracle: stat selection manifest: %w", err)
	}
	if info.Mode()&os.ModeSymlink != 0 || !info.Mode().IsRegular() {
		return StorageSelection{}, fmt.Errorf("runtime-oracle: selection manifest must be a regular file")
	}
	if info.Size() > MaxManifestBytes {
		return StorageSelection{}, fmt.Errorf("runtime-oracle: selection manifest exceeds size budget")
	}
	data, err := os.ReadFile(manifestPath)
	if err != nil {
		return StorageSelection{}, fmt.Errorf("runtime-oracle: read selection manifest: %w", err)
	}
	if err := validateNoDuplicateKeys(data); err != nil {
		return StorageSelection{}, fmt.Errorf("runtime-oracle: selection manifest duplicate keys: %w", err)
	}
	selection, err := decodeStorageSelectionJSON(data)
	if err != nil {
		return StorageSelection{}, err
	}
	if _, err := validateProducerID(selection.ProducerID); err != nil {
		return StorageSelection{}, fmt.Errorf("runtime-oracle: selection producer ID: %w", err)
	}
	if !validProducerIDs[selection.ProducerID] {
		return StorageSelection{}, fmt.Errorf("runtime-oracle: unrecognized producer ID %q", selection.ProducerID)
	}
	if len(selection.Cases) == 0 {
		return StorageSelection{}, fmt.Errorf("runtime-oracle: selection registers no case")
	}
	families, _, err := Discover(repoRoot)
	if err != nil {
		return StorageSelection{}, fmt.Errorf("runtime-oracle: discover registries: %w", err)
	}
	familiesByID := make(map[string]Family, len(families))
	for _, family := range families {
		familiesByID[family.ID] = family
	}
	registry := storageRegistryWithRoutes(selection.Routes)
	if err := validateStorageSelectionRoutes(selection, registry); err != nil {
		return StorageSelection{}, err
	}
	for _, src := range selection.Sources {
		if err := validateSelectionSource(repoRoot, src); err != nil {
			return StorageSelection{}, fmt.Errorf("runtime-oracle: selection source %s: %w", src.Path, err)
		}
	}
	seenCase := make(map[string]bool, len(selection.Cases))
	for _, c := range selection.Cases {
		if seenCase[c.ID] {
			return StorageSelection{}, fmt.Errorf("runtime-oracle: selection registers case %s twice", c.ID)
		}
		seenCase[c.ID] = true
		if err := validateStorageCase(c, familiesByID); err != nil {
			return StorageSelection{}, fmt.Errorf("runtime-oracle: selection case %s: %w", c.ID, err)
		}
		if _, err := validateCaseSpecConsumer(candidateDir, c, familiesByID, registry); err != nil {
			return StorageSelection{}, fmt.Errorf("runtime-oracle: selection case %s: %w", c.ID, err)
		}
	}
	return selection, nil
}

// mergeStorageSelections clones the base manifest and merges reviewed storage
// selections. It does not reconcile against the repository while assets remain
// external; the controller reconciles only after integration.
func mergeStorageSelections(root string, base Inventory, selections ...StorageSelection) (Inventory, error) {
	if len(selections) == 0 {
		return Inventory{}, fmt.Errorf("runtime-oracle: merge needs at least one storage selection")
	}
	families, _, err := Discover(root)
	if err != nil {
		return Inventory{}, fmt.Errorf("runtime-oracle: discover registries: %w", err)
	}
	familiesByID := make(map[string]Family, len(families))
	for _, family := range families {
		familiesByID[family.ID] = family
	}
	registry := storageRegistryWithRoutes(flattenSelectionRoutes(selections))

	merged := Inventory{
		SchemaVersion:  base.SchemaVersion,
		SourceRevision: base.SourceRevision,
		Identities:     base.Identities,
		Families:       make([]Family, 0, len(base.Families)),
		Cases:          make([]CaseSpec, 0, len(base.Cases)),
	}
	for _, family := range base.Families {
		family.SupportedVersions = append([]string(nil), family.SupportedVersions...)
		family.Sources = append([]SourceSpec(nil), family.Sources...)
		family.Cases = append([]string(nil), family.Cases...)
		merged.Families = append(merged.Families, family)
	}
	merged.Cases = append(merged.Cases, base.Cases...)

	baseByID := make(map[string]CaseSpec, len(base.Cases))
	for _, c := range base.Cases {
		if _, exists := baseByID[c.ID]; exists {
			return Inventory{}, fmt.Errorf("base manifest registers case %s twice", c.ID)
		}
		baseByID[c.ID] = c
	}

	selectedFamilies := make(map[string]bool)
	pendingByID := make(map[string]CaseSpec)
	for index, selection := range selections {
		if _, err := validateProducerID(selection.ProducerID); err != nil {
			return Inventory{}, fmt.Errorf("selection %d producer ID: %w", index, err)
		}
		if !validProducerIDs[selection.ProducerID] {
			return Inventory{}, fmt.Errorf("selection %d names unrecognized producer ID %q", index, selection.ProducerID)
		}
		if err := validateStorageSelectionRoutes(selection, registry); err != nil {
			return Inventory{}, fmt.Errorf("selection %d: %w", index, err)
		}
		for _, src := range selection.Sources {
			if err := validateSelectionSource(root, src); err != nil {
				return Inventory{}, fmt.Errorf("selection %d source %s: %w", index, src.Path, err)
			}
		}
		if len(selection.Cases) == 0 {
			return Inventory{}, fmt.Errorf("selection %d registers no case", index)
		}
		for _, c := range selection.Cases {
			if err := validateStorageCase(c, familiesByID); err != nil {
				return Inventory{}, fmt.Errorf("selection %d case %s: %w", index, c.ID, err)
			}
			route := ConsumerRoute{FamilyID: c.Family, Version: c.Version, Operation: c.Operation}
			if !storageRouteRegistered(registry, route) {
				return Inventory{}, fmt.Errorf(
					"selection %d case %s names route %s/%s/%s, which no consumer registration carries",
					index, c.ID, route.FamilyID, route.Version, route.Operation,
				)
			}
			if !selectionClaimsRoute(selection, route) {
				return Inventory{}, fmt.Errorf(
					"selection %d case %s names route %s/%s/%s, which the selection does not claim",
					index, c.ID, route.FamilyID, route.Version, route.Operation,
				)
			}
			if _, exists := pendingByID[c.ID]; exists {
				return Inventory{}, fmt.Errorf("selections register case %s twice", c.ID)
			}
			pendingByID[c.ID] = c
			selectedFamilies[c.Family] = true
		}
	}

	addedIDs := make([]string, 0, len(pendingByID))
	for id, c := range pendingByID {
		existing, exists := baseByID[id]
		if exists && !reflect.DeepEqual(existing, c) {
			return Inventory{}, fmt.Errorf("selection case %s conflicts with the registered case", id)
		}
		if !exists {
			addedIDs = append(addedIDs, id)
		}
	}
	sort.Strings(addedIDs)
	for _, id := range addedIDs {
		merged.Cases = append(merged.Cases, pendingByID[id])
	}
	sort.Slice(merged.Cases, func(i, j int) bool { return merged.Cases[i].ID < merged.Cases[j].ID })

	for index := range merged.Families {
		family := merged.Families[index]
		if !selectedFamilies[family.ID] {
			continue
		}
		if !strings.HasPrefix(family.ID, "save.") {
			return Inventory{}, fmt.Errorf("selection registers non-save family %s", family.ID)
		}

		var listed []string
		for _, c := range merged.Cases {
			if c.Family == family.ID {
				listed = append(listed, c.ID)
			}
		}
		sort.Strings(listed)
		registered := make(map[string]bool, len(listed))
		for _, id := range listed {
			registered[id] = true
		}
		for _, id := range family.Cases {
			if !registered[id] {
				return Inventory{}, fmt.Errorf("family %s lists case %s, which is absent from the top-level case list", family.ID, id)
			}
		}
		merged.Families[index].Cases = listed

		paths := make(map[string]string, len(family.Sources))
		for _, source := range family.Sources {
			paths[source.Path] = source.SHA256
		}
		for _, selection := range selections {
			for _, c := range selection.Cases {
				if c.Family != family.ID {
					continue
				}
				for _, src := range selection.Sources {
					if existing, ok := paths[src.Path]; ok && existing != src.SHA256 {
						return Inventory{}, fmt.Errorf("family %s source %s has conflicting sha256", family.ID, src.Path)
					}
					paths[src.Path] = src.SHA256
				}
			}
		}
		relatives := make([]string, 0, len(paths))
		for relative := range paths {
			relatives = append(relatives, relative)
		}
		sort.Strings(relatives)
		sources := make([]SourceSpec, 0, len(relatives))
		for _, relative := range relatives {
			hash, err := hashFile(filepath.Join(root, filepath.FromSlash(relative)))
			if err != nil {
				return Inventory{}, fmt.Errorf("hash provenance source %s: %w", relative, err)
			}
			if paths[relative] != hash {
				return Inventory{}, fmt.Errorf("family %s source %s sha256 %s does not match disk %s", family.ID, relative, paths[relative], hash)
			}
			sources = append(sources, SourceSpec{Path: relative, SHA256: hash})
		}
		merged.Families[index].Sources = sources
	}

	baseFamiliesByID := make(map[string]Family, len(base.Families))
	for _, family := range base.Families {
		baseFamiliesByID[family.ID] = family
	}
	for _, family := range merged.Families {
		if selectedFamilies[family.ID] {
			continue
		}
		if !reflect.DeepEqual(family, baseFamiliesByID[family.ID]) {
			return Inventory{}, fmt.Errorf("selection changed unrelated family %s", family.ID)
		}
	}
	if merged.SourceRevision != base.SourceRevision {
		return Inventory{}, fmt.Errorf("merged source_revision %s, want %s", merged.SourceRevision, base.SourceRevision)
	}
	return merged, nil
}

func validateStorageCase(c CaseSpec, families map[string]Family) error {
	family, ok := families[c.Family]
	if !ok {
		return fmt.Errorf("unknown family %s", c.Family)
	}
	if !strings.HasPrefix(c.Family, "save.") {
		return fmt.Errorf("family %s is not a save family", c.Family)
	}
	prefix := c.Family + "/" + c.Version + "/"
	if !strings.HasPrefix(c.ID, prefix) || len(c.ID) <= len(prefix) {
		return fmt.Errorf("id %q must match %s<label>", c.ID, prefix)
	}
	if !containsString(family.SupportedVersions, c.Version) {
		return fmt.Errorf("version %q is absent from family supported_versions", c.Version)
	}
	if strings.TrimSpace(c.RustConsumer) != storageConsumerName {
		return fmt.Errorf("rust_consumer must be %s", storageConsumerName)
	}
	switch c.Operation {
	case "decode", "encode":
	case "order":
		if c.Family != "save.region" {
			return fmt.Errorf("operation order is permitted only for save.region")
		}
	default:
		return fmt.Errorf("invalid operation %q", c.Operation)
	}
	if c.PacketKey != nil {
		return fmt.Errorf("save case must not carry packet_key")
	}
	if c.InputFormat != "binary" {
		return fmt.Errorf("input_format must be binary")
	}
	if len(c.Checkpoints) != 1 || c.Checkpoints[0] != "0" {
		return fmt.Errorf("checkpoints must be [\"0\"]")
	}
	if err := validateStorageArguments(c.Family, c.Operation, c.Arguments); err != nil {
		return err
	}
	return nil
}

func decodeStorageSelectionJSON(data []byte) (StorageSelection, error) {
	var top map[string]json.RawMessage
	dec := json.NewDecoder(bytes.NewReader(data))
	dec.UseNumber()
	if err := dec.Decode(&top); err != nil {
		return StorageSelection{}, fmt.Errorf("runtime-oracle: decode selection manifest: %w", err)
	}
	allowed := map[string]bool{"producer_id": true, "cases": true, "sources": true, "routes": true}
	for key := range top {
		if !allowed[key] {
			return StorageSelection{}, fmt.Errorf("runtime-oracle: selection manifest has unknown key %q", key)
		}
	}
	var selection StorageSelection
	if raw, ok := top["producer_id"]; ok {
		if err := json.Unmarshal(raw, &selection.ProducerID); err != nil {
			return StorageSelection{}, fmt.Errorf("runtime-oracle: decode producer_id: %w", err)
		}
	}
	if raw, ok := top["cases"]; ok {
		if err := json.Unmarshal(raw, &selection.Cases); err != nil {
			return StorageSelection{}, fmt.Errorf("runtime-oracle: decode cases: %w", err)
		}
	}
	if raw, ok := top["sources"]; ok {
		if err := json.Unmarshal(raw, &selection.Sources); err != nil {
			return StorageSelection{}, fmt.Errorf("runtime-oracle: decode sources: %w", err)
		}
	}
	if raw, ok := top["routes"]; ok {
		var routes []consumerRouteJSON
		if err := json.Unmarshal(raw, &routes); err != nil {
			return StorageSelection{}, fmt.Errorf("runtime-oracle: decode routes: %w", err)
		}
		for _, route := range routes {
			selection.Routes = append(selection.Routes, ConsumerRoute{
				FamilyID:  route.FamilyID,
				Version:   route.Version,
				Operation: route.Operation,
			})
		}
	}
	if strings.TrimSpace(selection.ProducerID) == "" {
		return StorageSelection{}, fmt.Errorf("runtime-oracle: selection manifest missing producer_id")
	}
	return selection, nil
}

func validateStorageSelectionRoutes(selection StorageSelection, registry ConsumerRegistry) error {
	seen := make(map[ConsumerRoute]bool, len(selection.Routes))
	for _, route := range selection.Routes {
		if seen[route] {
			return fmt.Errorf("selection claims route %s/%s/%s twice", route.FamilyID, route.Version, route.Operation)
		}
		seen[route] = true
		if !storageRouteRegistered(registry, route) {
			return fmt.Errorf(
				"selection claims route %s/%s/%s, which no consumer registration carries",
				route.FamilyID, route.Version, route.Operation,
			)
		}
	}
	for _, c := range selection.Cases {
		route := ConsumerRoute{FamilyID: c.Family, Version: c.Version, Operation: c.Operation}
		if !selectionClaimsRoute(selection, route) {
			return fmt.Errorf(
				"selection case %s names route %s/%s/%s, which the selection does not claim",
				c.ID, route.FamilyID, route.Version, route.Operation,
			)
		}
	}
	return nil
}

func selectionClaimsRoute(selection StorageSelection, route ConsumerRoute) bool {
	for _, claimed := range selection.Routes {
		if claimed == route {
			return true
		}
	}
	return false
}

func validateSelectionSource(root string, src SourceSpec) error {
	if err := validateCorpusPath(src.Path); err != nil {
		return err
	}
	if !hexSha256Pattern.MatchString(src.SHA256) {
		return fmt.Errorf("invalid sha256 %s", src.SHA256)
	}
	if err := checkNoSymlinks(root, src.Path); err != nil {
		return err
	}
	fullPath := filepath.Join(root, filepath.FromSlash(src.Path))
	hash, err := hashFile(fullPath)
	if err != nil {
		return fmt.Errorf("source file: %w", err)
	}
	if hash != src.SHA256 {
		return fmt.Errorf("sha256 %s does not match disk %s", src.SHA256, hash)
	}
	return nil
}

func validateExternalCandidateRoot(repoRoot, candidateDir string) error {
	if !filepath.IsAbs(candidateDir) {
		return fmt.Errorf("runtime-oracle: candidate directory must be absolute")
	}
	absRoot, err := filepath.Abs(repoRoot)
	if err != nil {
		return err
	}
	if resolved, err := filepath.EvalSymlinks(absRoot); err == nil {
		absRoot = resolved
	}
	absCandidate, err := filepath.Abs(candidateDir)
	if err != nil {
		return fmt.Errorf("runtime-oracle: resolve candidate directory: %w", err)
	}
	resolvedCandidate, err := filepath.EvalSymlinks(absCandidate)
	if err != nil {
		return fmt.Errorf("runtime-oracle: resolve candidate directory: %w", err)
	}
	live, err := isLivePath(absRoot, resolvedCandidate)
	if err != nil {
		return err
	}
	if live {
		return fmt.Errorf("runtime-oracle: candidate directory is inside repository")
	}
	for component := absCandidate; ; component = filepath.Dir(component) {
		if component == "/" || component == filepath.Dir(component) {
			break
		}
		info, statErr := os.Lstat(component)
		if statErr != nil {
			return fmt.Errorf("runtime-oracle: stat candidate path %s: %w", component, statErr)
		}
		if info.Mode()&os.ModeSymlink != 0 {
			return fmt.Errorf("runtime-oracle: candidate path has symlink ancestor %s", component)
		}
	}
	return nil
}

func flattenSelectionRoutes(selections []StorageSelection) []ConsumerRoute {
	seen := make(map[ConsumerRoute]bool)
	var routes []ConsumerRoute
	for _, selection := range selections {
		for _, route := range selection.Routes {
			if seen[route] {
				continue
			}
			seen[route] = true
			routes = append(routes, route)
		}
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

func storageRegistryWithRoutes(routes []ConsumerRoute) ConsumerRegistry {
	registry := cloneConsumerRegistry(BaselineConsumerRegistry())
	routeSet := make(map[ConsumerRoute]struct{}, len(routes))
	for _, route := range routes {
		routeSet[route] = struct{}{}
	}
	if len(routeSet) == 0 {
		return registry
	}
	registry[storageConsumerName] = ConsumerRegistration{
		Kind:   ConsumerRust,
		Routes: routeSet,
	}
	return registry
}

func storageRouteRegistered(registry ConsumerRegistry, route ConsumerRoute) bool {
	registration, ok := registry[storageConsumerName]
	if !ok {
		return false
	}
	_, ok = registration.Routes[route]
	return ok
}

func cloneConsumerRegistry(base ConsumerRegistry) ConsumerRegistry {
	cloned := make(ConsumerRegistry, len(base))
	for name, registration := range base {
		routes := make(map[ConsumerRoute]struct{}, len(registration.Routes))
		for route := range registration.Routes {
			routes[route] = struct{}{}
		}
		cloned[name] = ConsumerRegistration{Kind: registration.Kind, Routes: routes}
	}
	return cloned
}
