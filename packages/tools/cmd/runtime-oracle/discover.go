package main

import (
	"encoding/json"
	"fmt"
	"os"
	"path/filepath"
	"regexp"
	"sort"
	"strconv"
	"strings"
)

const (
	ownerDomain            = "mornlea_domain"
	ownerProtocol          = "mornlea_protocol"
	ownerStorage           = "mornlea_storage"
	ownerEngine            = "mornlea_engine"
	ownerAgent             = "companion-agent-service"
	protocolLE             = "little-endian integers; IEEE-754 binary32; canonical uvarint; reject NaN/Inf and unknown IDs"
	saveLE                 = "little-endian integers; CRC integrity; exact byte round-trip; no implicit repair"
	replayIdentity         = "source revision, contract versions, fixture digest, seed, ordered input, tick schedule, normalized observations"
	domainValues           = "registered item numbering; fixed per-chunk slot counts; nonzero generation; reject unregistered items and out-of-range values"
	domainIdentityValues   = "UUIDv4 identity; canonical display and companion names; bounded command and speech text; reject zero, wrong version, wrong variant, out-of-range and untrimmed values"
	domainCommandControl   = "finite look angles only; full i8 movement axes; hotbar slot 0..8; resync dimension 0/1; independent held flags; reject non-finite rotation and out-of-range slots"
	domainCommandInventory = "inventory 0..35; crafting view 0..44 with one grid end; furnace view 0..38 output source only; chest view 0..62; partial/quick/drop share view bounds without the stricter crafting and furnace-output rules; reject same slot, malformed container reference and unknown view; chat text 1..1024 bytes"
)

var (
	uintConstPattern    = regexp.MustCompile(`(?m)^\s*(?:const\s+)?(\w+)\s+uint32\s*=\s*(\w+)`)
	packetReturnPattern = regexp.MustCompile(`return ([A-Z][A-Za-z0-9]+)\{\}, true`)
	engineABIPattern    = regexp.MustCompile(`#define\s+MORNLEA_ENGINE_ABI_VERSION\s+(\d+)u`)
	engineFnPattern     = regexp.MustCompile(`(?m)^uint32_t (mornlea_[a-z_]+)\(`)
	agentVersionPattern = regexp.MustCompile(`"application_contract_version"\s*:\s*"([^"]+)"`)
)

// Discover reads current registries and sources without importing production
// packages. The oracle stays a leaf: it observes files, it does not load
// live authority, native ABI, or network stacks.
func Discover(root string) ([]Family, Identities, error) {
	protocolVersion, err := discoverProtocolVersion(root)
	if err != nil {
		return nil, Identities{}, err
	}
	chunkCurrent, chunkSupported, err := discoverSchemaRange(root,
		"packages/server/storage/chunk/chunk_codec.go", "currentChunkSchema",
		"packages/server/storage/chunk/migration.go", "oldestChunkSchema")
	if err != nil {
		return nil, Identities{}, err
	}
	playerCurrent, playerSupported, err := discoverSchemaRange(root,
		"packages/server/storage/player/player_codec.go", "CurrentSchema",
		"packages/server/storage/player/player_migration.go", "oldestPlayerSchema")
	if err != nil {
		return nil, Identities{}, err
	}
	companionCurrent, companionSupported, err := discoverNamedSchemas(root,
		"packages/server/storage/companion/companion_codec.go", "CurrentSchema", "companionSchemaV")
	if err != nil {
		return nil, Identities{}, err
	}
	hostileCurrent, hostileSupported, err := discoverNamedSchemas(root,
		"packages/server/storage/hostile/hostile_codec.go", "CurrentSchema", "hostileSchemaV")
	if err != nil {
		return nil, Identities{}, err
	}
	passiveCurrent, passiveSupported, err := discoverNamedSchemas(root,
		"packages/server/storage/passive/passive_codec.go", "CurrentSchema", "passiveSchemaV")
	if err != nil {
		return nil, Identities{}, err
	}
	metadataCurrent, metadataSupported, err := discoverMetadataVersions(root)
	if err != nil {
		return nil, Identities{}, err
	}
	regionCurrent, err := discoverUintFile(root, "packages/server/storage/region/region_format.go", "currentRegionVersion")
	if err != nil {
		return nil, Identities{}, err
	}
	engineABI, err := discoverEngineABI(root)
	if err != nil {
		return nil, Identities{}, err
	}
	agentHTTP, err := discoverAgentVersion(root, "packages/contracts/companion-agent/http-v1/manifest.json")
	if err != nil {
		return nil, Identities{}, err
	}
	agentMCP, err := discoverAgentVersion(root, "packages/contracts/companion-agent/mcp-v1/manifest.json")
	if err != nil {
		return nil, Identities{}, err
	}

	live := Identities{
		Protocol:           protocolVersion,
		ChunkSchema:        chunkCurrent,
		PlayerSchema:       playerCurrent,
		WorldMetadata:      metadataCurrent,
		CompanionsAISchema: companionCurrent,
		HostileMobsSchema:  hostileCurrent,
		PassiveMobsSchema:  passiveCurrent,
		EngineABI:          engineABI,
		RegionFormat:       regionCurrent,
		AgentHTTP:          agentHTTP,
		AgentMCP:           agentMCP,
	}

	clientPackets, serverPackets, err := discoverProtocolPackets(root)
	if err != nil {
		return nil, Identities{}, err
	}
	kernels, err := discoverEngineKernels(root, engineABI)
	if err != nil {
		return nil, Identities{}, err
	}

	protocolVersionText := strconv.Itoa(protocolVersion)
	families := []Family{
		protocolFamily(root, "protocol.frame", "input", protocolVersionText, "packages/shared/network/codec/frame.go",
			[]string{"packages/shared/network/codec/frame.go", "packages/shared/network/codec/frame_test.go"}),
		saveFamily(root, "save.chunk", strconv.Itoa(chunkCurrent), intsToStrings(chunkSupported),
			"packages/server/storage/chunk/chunk_codec.go", append(
				versionedBins("packages/server/storage/chunk/testdata", "chunk", chunkSupported),
				"packages/server/storage/chunk/chunk_oracle_test.go",
				"packages/tools/cmd/runtime-oracle/storage_chunk_test.go",
			)),
		saveFamily(root, "save.player", strconv.Itoa(playerCurrent), intsToStrings(playerSupported),
			"packages/server/storage/player/player_codec.go", append(
				versionedBins("packages/server/storage/player/testdata", "player", playerSupported),
				"packages/tools/cmd/runtime-oracle/storage_player_test.go",
			)),
		saveFamily(root, "save.companion", strconv.Itoa(companionCurrent), intsToStrings(companionSupported),
			"packages/server/storage/companion/companion_codec.go", versionedBins("packages/server/storage/companion/testdata", "companions", companionSupported)),
		saveFamily(root, "save.hostile", strconv.Itoa(hostileCurrent), intsToStrings(hostileSupported),
			"packages/server/storage/hostile/hostile_codec.go", append(
				versionedBins("packages/server/storage/hostile/testdata", "hostile-mobs", hostileSupported),
				"packages/tools/cmd/runtime-oracle/storage_hostile_test.go",
			)),
		saveFamily(root, "save.passive", strconv.Itoa(passiveCurrent), intsToStrings(passiveSupported),
			"packages/server/storage/passive/passive_codec.go", append(
				versionedBins("packages/server/storage/passive/testdata", "passive-mobs", passiveSupported),
				"packages/tools/cmd/runtime-oracle/storage_passive_test.go",
			)),
		saveFamily(root, "save.world-metadata", strconv.Itoa(metadataCurrent), intsToStrings(metadataSupported),
			"packages/server/storage/metadata.go",
			[]string{
				"packages/server/storage/metadata.go",
				"packages/server/storage/metadata_oracle_test.go",
				"packages/server/storage/metadata_test.go",
			}),
		saveFamily(root, "save.region", strconv.Itoa(regionCurrent), []string{strconv.Itoa(regionCurrent)},
			"packages/server/storage/region/region_format.go",
			[]string{
				"packages/server/storage/region/region_format.go",
				"packages/server/storage/region/region_format_test.go",
				"packages/tools/cmd/runtime-oracle/storage_region_test.go",
			}),
		{
			ID: "kernel.pathfind", Kind: "kernel", Role: "event",
			CurrentVersion: "1", SupportedVersions: []string{"1"},
			Source: "packages/shared/pathfind/pathfind.go", EventualOwner: ownerEngine,
			NumericSemantics: "integer path costs; fixed neighbor expansion order; bounded node budget; distinct unreachable and budget failures",
			Sources:          makeSourceSpecs(root, []string{"packages/shared/pathfind/pathfind.go", "packages/shared/pathfind/pathfind_test.go"}),
		},
		{
			ID: "agent.http", Kind: "agent", Role: "input",
			CurrentVersion: agentHTTP, SupportedVersions: []string{agentHTTP},
			Source: "packages/contracts/companion-agent/http-v1/manifest.json", EventualOwner: ownerAgent,
			NumericSemantics: "JSON request/response contracts; bounded bodies; loopback-only service",
			Sources: makeSourceSpecs(root, []string{
				"packages/contracts/companion-agent/http-v1/manifest.json",
				"packages/contracts/companion-agent/http-v1/golden/valid.json",
				"packages/contracts/companion-agent/http-v1/golden/invalid.json",
			}),
		},
		{
			ID: "agent.mcp", Kind: "agent", Role: "event",
			CurrentVersion: agentMCP, SupportedVersions: []string{agentMCP},
			Source: "packages/contracts/companion-agent/mcp-v1/manifest.json", EventualOwner: ownerAgent,
			NumericSemantics: "JSON tool contracts; deterministic array order; bounded result bytes",
			Sources: makeSourceSpecs(root, []string{
				"packages/contracts/companion-agent/mcp-v1/manifest.json",
				"packages/contracts/companion-agent/mcp-v1/golden/valid.json",
				"packages/contracts/companion-agent/mcp-v1/golden/invalid.json",
			}),
		},
		{
			ID: "domain.input", Kind: "domain", Role: "input",
			CurrentVersion: protocolVersionText, SupportedVersions: []string{protocolVersionText},
			Source: "packages/shared/network/protocol/message_command.go", EventualOwner: ownerDomain,
			NumericSemantics: protocolLE,
			Sources:          makeSourceSpecs(root, []string{"packages/shared/network/protocol/message_command.go", "packages/shared/network/protocol/packet_test.go"}),
		},
		{
			ID: "domain.event", Kind: "domain", Role: "event",
			CurrentVersion: "1", SupportedVersions: []string{"1"},
			Source: "packages/client/presentation/transcript_corpus_test.go", EventualOwner: ownerDomain,
			NumericSemantics: replayIdentity,
			Sources:          makeSourceSpecs(root, []string{"packages/client/presentation/transcript_corpus_test.go"}),
		},
		{
			ID: "domain.values", Kind: "domain", Role: "input",
			CurrentVersion: "current", SupportedVersions: []string{"current"},
			Source: "packages/shared/core/item.go", EventualOwner: ownerDomain,
			NumericSemantics: domainValues,
			Sources: makeSourceSpecs(root, []string{
				"packages/shared/core/item.go",
				"packages/shared/core/smelting.go",
				"packages/shared/core/drop.go",
				"packages/shared/core/container.go",
				"packages/shared/core/furnace.go",
				"packages/shared/core/chest.go",
				"packages/shared/network/protocol/message_container.go",
			}),
		},
		{
			ID: "domain.identity_values", Kind: "domain", Role: "input",
			CurrentVersion: "current", SupportedVersions: []string{"current"},
			Source: "packages/shared/core/player_id.go", EventualOwner: ownerDomain,
			NumericSemantics: domainIdentityValues,
			Sources: makeSourceSpecs(root, []string{
				"packages/shared/core/player_id.go",
				"packages/shared/companion/identity.go",
				"packages/shared/network/protocol/message_companion.go",
			}),
		},
		{
			ID: "domain.command_control", Kind: "domain", Role: "input",
			CurrentVersion: "current", SupportedVersions: []string{"current"},
			Source: "packages/shared/network/protocol/message_command.go", EventualOwner: ownerDomain,
			NumericSemantics: domainCommandControl,
			Sources: makeSourceSpecs(root, []string{
				"packages/shared/network/protocol/message_command.go",
				"packages/shared/network/protocol/message_container.go",
				"packages/shared/network/protocol/packet.go",
				"packages/shared/network/codec/codec_client.go",
				"packages/shared/core/item.go",
				"packages/shared/core/block.go",
			}),
		},
		{
			ID: "domain.command_inventory", Kind: "domain", Role: "input",
			CurrentVersion: "current", SupportedVersions: []string{"current"},
			Source: "packages/shared/network/protocol/message_inventory.go", EventualOwner: ownerDomain,
			NumericSemantics: domainCommandInventory,
			Sources: makeSourceSpecs(root, []string{
				"packages/shared/network/protocol/message_inventory.go",
				"packages/shared/network/protocol/message_container.go",
				"packages/shared/network/protocol/message_stack_splitting.go",
				"packages/shared/network/protocol/message_drop_stack.go",
				"packages/shared/network/protocol/message_companion.go",
				"packages/shared/network/protocol/packet.go",
				"packages/shared/network/codec/codec_client.go",
				"packages/shared/core/inventory.go",
				"packages/shared/core/furnace.go",
				"packages/shared/core/chest.go",
				"packages/shared/core/container.go",
			}),
		},
	}
	for _, name := range clientPackets {
		families = append(families, protocolFamily(root, "protocol.client."+name, "input", protocolVersionText,
			"packages/shared/network/protocol/registry.go", protocolPacketFixtures(name)))
	}
	for _, name := range serverPackets {
		families = append(families, protocolFamily(root, "protocol.server."+name, "event", protocolVersionText,
			"packages/shared/network/protocol/registry.go", protocolPacketFixtures(name)))
	}
	families = append(families, kernels...)
	sort.Slice(families, func(i, j int) bool { return families[i].ID < families[j].ID })
	return families, live, nil
}

func makeSourceSpecs(root string, paths []string) []SourceSpec {
	specs := make([]SourceSpec, 0, len(paths))
	for _, p := range paths {
		full := filepath.Join(root, filepath.FromSlash(p))
		hash, err := hashFile(full)
		if err != nil {
			hash = ""
		}
		specs = append(specs, SourceSpec{Path: p, SHA256: hash})
	}
	return specs
}

func protocolFamily(root, id, role, version, source string, fixtures []string) Family {
	var cases []string
	if id == "protocol.frame" {
		cases = []string{"protocol.frame/45/valid"}
	}
	return Family{
		ID: id, Kind: "protocol", Role: role,
		CurrentVersion: version, SupportedVersions: []string{version},
		Source: source, EventualOwner: ownerProtocol,
		NumericSemantics: protocolLE,
		Sources:          makeSourceSpecs(root, fixtures),
		Cases:            cases,
	}
}

func saveFamily(root, id, current string, supported []string, source string, fixtures []string) Family {
	return Family{
		ID: id, Kind: "save", Role: "event",
		CurrentVersion: current, SupportedVersions: supported,
		Source: source, EventualOwner: ownerStorage,
		NumericSemantics: saveLE,
		Sources:          makeSourceSpecs(root, fixtures),
	}
}

func protocolPacketFixtures(name string) []string {
	fixtures := []string{
		"packages/shared/network/codec/codec_golden_test.go",
		"packages/shared/network/protocol/packet_test.go",
	}
	if name == "ChunkSnapshot" {
		fixtures = append([]string{"packages/shared/network/codec/testdata/chunk-snapshot-v1.bin"}, fixtures...)
	}
	return fixtures
}

func versionedBins(dir, prefix string, versions []int) []string {
	fixtures := make([]string, 0, len(versions))
	for _, version := range versions {
		fixtures = append(fixtures, fmt.Sprintf("%s/%s-v%d.bin", dir, prefix, version))
	}
	return fixtures
}

func discoverProtocolVersion(root string) (int, error) {
	return discoverUintFile(root, "packages/shared/network/protocol/packet.go", "ProtocolVersion")
}

func discoverProtocolPackets(root string) (client, server []string, err error) {
	source, err := readRepoFile(root, "packages/shared/network/protocol/registry.go")
	if err != nil {
		return nil, nil, err
	}
	clientSrc, ok := sliceBetween(source, "func ClientPacketForID", "func ServerPacketID")
	if !ok {
		return nil, nil, fmt.Errorf("runtime-oracle: ClientPacketForID not found")
	}
	serverSrc, ok := sliceBetween(source, "func ServerPacketForID", "func CommandRejectReasonID")
	if !ok {
		return nil, nil, fmt.Errorf("runtime-oracle: ServerPacketForID not found")
	}
	return uniqueSorted(packetReturnPattern.FindAllStringSubmatch(clientSrc, -1)),
		uniqueSorted(packetReturnPattern.FindAllStringSubmatch(serverSrc, -1)),
		nil
}

func discoverEngineKernels(root string, abi int) ([]Family, error) {
	source, err := readRepoFile(root, "packages/engine/include/mornlea_engine.h")
	if err != nil {
		return nil, err
	}
	var families []Family
	seen := map[string]bool{}
	for _, match := range engineFnPattern.FindAllStringSubmatch(source, -1) {
		name := match[1]
		if name == "mornlea_engine_abi_version" || seen[name] {
			continue
		}
		seen[name] = true
		version := strconv.Itoa(abi)
		families = append(families, Family{
			ID: "kernel." + name, Kind: "kernel", Role: "event",
			CurrentVersion: version, SupportedVersions: []string{version},
			Source: "packages/engine/include/mornlea_engine.h", EventualOwner: ownerEngine,
			NumericSemantics: kernelSemantics(name),
			Sources:          makeSourceSpecs(root, kernelFixtures(name)),
			Cases:            nil,
		})
	}
	if len(families) == 0 {
		return nil, fmt.Errorf("runtime-oracle: no engine kernel exports found")
	}
	return families, nil
}

func kernelSemantics(name string) string {
	switch {
	case strings.Contains(name, "fluid"):
		return "deterministic integer fluid ranks; little-endian packed records; fail closed on overflow"
	case strings.Contains(name, "physics"):
		return "IEEE-754 binary32 little-endian; layout-versioned MGP1 header; reject non-finite used inputs"
	case strings.Contains(name, "collision"):
		return "IEEE-754 binary32 little-endian; swept AABB; fail closed on invalid layout"
	case strings.Contains(name, "raycast"):
		return "IEEE-754 binary32 little-endian; bounded batch; fail closed on invalid layout"
	case strings.Contains(name, "worldgen"), strings.Contains(name, "tree"), strings.Contains(name, "lod"):
		return "deterministic integer worldgen; little-endian packed voxels; layout-versioned MGW1/MTB1"
	default:
		return "little-endian packed records; fail closed on invalid layout, aliasing, or overflow"
	}
}

func kernelFixtures(name string) []string {
	rust := map[string]string{
		"mornlea_mesh_section":      "packages/engine/crates/mornlea_engine/src/greedy/mod.rs",
		"mornlea_collision_resolve": "packages/engine/crates/mornlea_engine/src/collision.rs",
		"mornlea_raycast_batch":     "packages/engine/crates/mornlea_engine/src/raycast.rs",
		"mornlea_physics_step":      "packages/engine/crates/mornlea_engine/src/step.rs",
		"mornlea_worldgen_chunk":    "packages/engine/crates/mornlea_engine/src/worldgen.rs",
		"mornlea_worldgen_probe":    "packages/engine/crates/mornlea_engine/src/worldgen.rs",
		"mornlea_tree_blocks":       "packages/engine/crates/mornlea_engine/src/worldgen.rs",
		"mornlea_lod_shell":         "packages/engine/crates/mornlea_engine/src/lod.rs",
		"mornlea_fluid_eval_batch":  "packages/engine/crates/mornlea_engine/src/fluid_eval.rs",
		"mornlea_fluid_rescan":      "packages/engine/crates/mornlea_engine/src/fluid_rescan.rs",
	}
	fixtures := []string{
		"packages/engine/include/mornlea_engine.h",
		"packages/shared/nativeabi/native_test.go",
	}
	if path, ok := rust[name]; ok {
		fixtures = append(fixtures, path)
	}
	return fixtures
}

func discoverEngineABI(root string) (int, error) {
	source, err := readRepoFile(root, "packages/engine/include/mornlea_engine.h")
	if err != nil {
		return 0, err
	}
	match := engineABIPattern.FindStringSubmatch(source)
	if match == nil {
		return 0, fmt.Errorf("runtime-oracle: engine ABI version not found")
	}
	return strconv.Atoi(match[1])
}

func discoverAgentVersion(root, rel string) (string, error) {
	source, err := readRepoFile(root, rel)
	if err != nil {
		return "", err
	}
	match := agentVersionPattern.FindStringSubmatch(source)
	if match == nil || match[1] == "" {
		return "", fmt.Errorf("runtime-oracle: agent contract version missing in %s", rel)
	}
	return match[1], nil
}

func discoverSchemaRange(root, currentFile, currentName, oldestFile, oldestName string) (int, []int, error) {
	current, err := discoverUintFile(root, currentFile, currentName)
	if err != nil {
		return 0, nil, err
	}
	oldest, err := discoverUintFile(root, oldestFile, oldestName)
	if err != nil {
		return 0, nil, err
	}
	if oldest < 1 || current < oldest {
		return 0, nil, fmt.Errorf("runtime-oracle: invalid schema range %s %d..%d", currentName, oldest, current)
	}
	versions := make([]int, 0, current-oldest+1)
	for version := oldest; version <= current; version++ {
		versions = append(versions, version)
	}
	return current, versions, nil
}

func discoverNamedSchemas(root, rel, currentName, prefix string) (int, []int, error) {
	consts, err := uintConstsFile(root, rel)
	if err != nil {
		return 0, nil, err
	}
	current, err := resolveUint(consts, currentName)
	if err != nil {
		return 0, nil, fmt.Errorf("runtime-oracle: %s: %w", rel, err)
	}
	var versions []int
	for name := range consts {
		if !strings.HasPrefix(name, prefix) {
			continue
		}
		value, err := resolveUint(consts, name)
		if err != nil {
			return 0, nil, err
		}
		versions = append(versions, value)
	}
	if len(versions) == 0 {
		return 0, nil, fmt.Errorf("runtime-oracle: no %s* schemas in %s", prefix, rel)
	}
	sort.Ints(versions)
	return current, versions, nil
}

func discoverMetadataVersions(root string) (int, []int, error) {
	consts, err := uintConstsFile(root, "packages/server/storage/metadata.go")
	if err != nil {
		return 0, nil, err
	}
	current, err := resolveUint(consts, "currentMetadataVersion")
	if err != nil {
		return 0, nil, err
	}
	seen := map[int]bool{}
	var versions []int
	for name := range consts {
		if !strings.Contains(name, "Metadata") || !strings.HasSuffix(name, "Version") {
			continue
		}
		value, err := resolveUint(consts, name)
		if err != nil {
			return 0, nil, err
		}
		if seen[value] {
			continue
		}
		seen[value] = true
		versions = append(versions, value)
	}
	sort.Ints(versions)
	if len(versions) == 0 {
		return 0, nil, fmt.Errorf("runtime-oracle: no world metadata versions found")
	}
	return current, versions, nil
}

func discoverUintFile(root, rel, name string) (int, error) {
	consts, err := uintConstsFile(root, rel)
	if err != nil {
		return 0, err
	}
	value, err := resolveUint(consts, name)
	if err != nil {
		return 0, fmt.Errorf("runtime-oracle: %s: %w", rel, err)
	}
	return value, nil
}

func uintConstsFile(root, rel string) (map[string]string, error) {
	source, err := readRepoFile(root, rel)
	if err != nil {
		return nil, err
	}
	consts := map[string]string{}
	for _, match := range uintConstPattern.FindAllStringSubmatch(source, -1) {
		consts[match[1]] = match[2]
	}
	return consts, nil
}

func resolveUint(consts map[string]string, name string) (int, error) {
	seen := map[string]bool{}
	current := name
	for {
		if seen[current] {
			return 0, fmt.Errorf("cyclic uint constant %s", name)
		}
		seen[current] = true
		raw, ok := consts[current]
		if !ok {
			if n, err := strconv.Atoi(current); err == nil {
				return n, nil
			}
			return 0, fmt.Errorf("uint constant %s not found", name)
		}
		if n, err := strconv.Atoi(raw); err == nil {
			return n, nil
		}
		current = raw
	}
}

func readRepoFile(root, rel string) (string, error) {
	data, err := os.ReadFile(filepath.Join(root, filepath.FromSlash(rel)))
	if err != nil {
		return "", fmt.Errorf("runtime-oracle: read %s: %w", rel, err)
	}
	return string(data), nil
}

func sliceBetween(source, start, end string) (string, bool) {
	begin := strings.Index(source, start)
	if begin < 0 {
		return "", false
	}
	rest := source[begin:]
	stop := strings.Index(rest, end)
	if stop < 0 {
		return rest, true
	}
	return rest[:stop], true
}

func uniqueSorted(matches [][]string) []string {
	seen := map[string]bool{}
	var names []string
	for _, match := range matches {
		name := match[1]
		if seen[name] {
			continue
		}
		seen[name] = true
		names = append(names, name)
	}
	sort.Strings(names)
	return names
}

func intsToStrings(values []int) []string {
	out := make([]string, len(values))
	for i, value := range values {
		out[i] = strconv.Itoa(value)
	}
	return out
}

func encodeInventory(inventory Inventory) ([]byte, error) {
	cloned := inventory
	cloned.Families = append([]Family(nil), inventory.Families...)
	sort.Slice(cloned.Families, func(i, j int) bool { return cloned.Families[i].ID < cloned.Families[j].ID })
	return json.MarshalIndent(cloned, "", "  ")
}

// DiscoverCases returns all current test cases in the corpus.
func DiscoverCases(root string) ([]CaseSpec, error) {
	frameInput := "testdata/runtime-migration/cases/frame/valid.bin"
	frameInputHash, err := hashFile(filepath.Join(root, filepath.FromSlash(frameInput)))
	if err != nil {
		return nil, fmt.Errorf("hash frame input: %w", err)
	}
	frameExpected := "testdata/runtime-migration/cases/frame/valid.expected.json"
	frameExpectedHash, err := hashFile(filepath.Join(root, filepath.FromSlash(frameExpected)))
	if err != nil {
		return nil, fmt.Errorf("hash frame expected: %w", err)
	}

	return []CaseSpec{
		{
			ID:           "protocol.frame/45/valid",
			Family:       "protocol.frame",
			Version:      "45",
			Operation:    "decode",
			Input:        AssetRef{Path: frameInput, SHA256: frameInputHash},
			InputFormat:  "binary",
			Expected:     AssetRef{Path: frameExpected, SHA256: frameExpectedHash},
			Checkpoints:  []string{"0"},
			RustConsumer: "corpus_frame",
		},
	}, nil
}
