package main

import (
	"bytes"
	"crypto/sha256"
	"encoding/json"
	"fmt"
	"io"
	"os"
	"path/filepath"
	"regexp"
	"sort"
	"strconv"
	"strings"
)

const (
	inventorySchemaVersion = 2
	BaselineSourceRevision = "b6043f004176055a2e39a98508b662691c3e4ef7"
	MaxManifestBytes       = 4 * 1024 * 1024
	MaxCaseJSONBytes       = 256 * 1024
	MaxBinaryBytes         = 4 * 1024 * 1024
	MaxCases               = 8192
	MaxObservations        = 32768
)

var hexSha256Pattern = regexp.MustCompile(`^sha256:[0-9a-f]{64}$`)
var sourceRevPattern = regexp.MustCompile(`^[0-9a-f]{40}$`)

// Inventory is the frozen language-neutral contract coverage list.
type Inventory struct {
	SchemaVersion  int        `json:"schema_version"`
	SourceRevision string     `json:"source_revision"`
	Identities     Identities `json:"identities"`
	Families       []Family   `json:"families"`
	Cases          []CaseSpec `json:"cases"`
}

// Identities pins current supported versions. Empty or zero values are
// incomplete evidence and fail acceptance.
type Identities struct {
	Protocol           int    `json:"protocol"`
	ChunkSchema        int    `json:"chunk_schema"`
	PlayerSchema       int    `json:"player_schema"`
	WorldMetadata      int    `json:"world_metadata"`
	CompanionsAISchema int    `json:"companions_ai_schema"`
	HostileMobsSchema  int    `json:"hostile_mobs_schema"`
	PassiveMobsSchema  int    `json:"passive_mobs_schema"`
	EngineABI          int    `json:"engine_abi"`
	RegionFormat       int    `json:"region_format"`
	AgentHTTP          string `json:"agent_http"`
	AgentMCP           string `json:"agent_mcp"`
}

// SourceSpec represents one provenance source file and its sha256 digest.
type SourceSpec struct {
	Path   string `json:"path"`
	SHA256 string `json:"sha256"`
}

// ConsumerKind discriminates between Rust and Go corpus consumers.
type ConsumerKind uint8

const (
	ConsumerRust ConsumerKind = iota + 1
	ConsumerExternalGo
)

// `ConsumerRoute` identifies one exact family version and operation a consumer executes.
type ConsumerRoute struct {
	FamilyID  string
	Version   string
	Operation string
}

// `ConsumerRegistration` binds one consumer kind to its executable routes.
type ConsumerRegistration struct {
	Kind   ConsumerKind
	Routes map[ConsumerRoute]struct{}
}

// `ConsumerRegistry` maps consumer identity names to their closed registrations.
type ConsumerRegistry map[string]ConsumerRegistration

// CoveragePoint identifies one family and supported version pair.
type CoveragePoint struct {
	FamilyID string
	Version  string
}

// CoverageReport separates covered from uncovered points.
type CoverageReport struct {
	Covered   []CoveragePoint
	Uncovered []CoveragePoint
}

// NegativeCoverageExceptions names family/version points for which no invalid
// representation exists. Every entry carries a nonempty reviewed rationale.
type NegativeCoverageExceptions map[CoveragePoint]string

// BaselineConsumerRegistry returns the closed baseline consumer registry.
//
// The corpus_frame consumer is the Rust framing consumer and carries both
// framing operations, because the framing family publishes a decode case and
// an encode case that the frame reader and writer execute independently.
//
// The mornlea_protocol consumer is the Rust packet consumer. It is created
// with the framing routes the packet corpus test already executes; the inbound
// negotiation group then appends the `ClientHello` and `LoginStart` routes,
// and later packet groups append their own family, version and operation
// routes here one node at a time. The closed union is verified at corpus
// closure. A registration never carries an empty route set, so a consumer is
// always born alongside routes that execute under it.
func BaselineConsumerRegistry() ConsumerRegistry {
	return ConsumerRegistry{
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
}

// BaselineNegativeCoverageExceptions returns the baseline negative coverage exceptions.
func BaselineNegativeCoverageExceptions() NegativeCoverageExceptions {
	return make(NegativeCoverageExceptions)
}

// ReconcileWorking checks an in-progress inventory against current registries.
func ReconcileWorking(
	root string,
	inventory Inventory,
	discovered []Family,
	live Identities,
	consumers ConsumerRegistry,
	negativeExceptions NegativeCoverageExceptions,
) (CoverageReport, error) {
	return reconcileInventory(root, inventory, discovered, live, consumers, negativeExceptions, false)
}

// ReconcileComplete checks an inventory for complete acceptance.
func ReconcileComplete(
	root string,
	inventory Inventory,
	discovered []Family,
	live Identities,
	consumers ConsumerRegistry,
	negativeExceptions NegativeCoverageExceptions,
) (CoverageReport, error) {
	return reconcileInventory(root, inventory, discovered, live, consumers, negativeExceptions, true)
}

// Family is one supported protocol, save, kernel, or agent contract.
type Family struct {
	ID                string       `json:"id"`
	Kind              string       `json:"kind"`
	Role              string       `json:"role"`
	CurrentVersion    string       `json:"current_version"`
	SupportedVersions []string     `json:"supported_versions"`
	Source            string       `json:"source"`
	EventualOwner     string       `json:"eventual_owner"`
	NumericSemantics  string       `json:"numeric_semantics"`
	Sources           []SourceSpec `json:"sources"`
	Cases             []string     `json:"cases"`
}

// CaseSpec defines one executable test case in the corpus.
type CaseSpec struct {
	ID           string          `json:"id"`
	Family       string          `json:"family"`
	Version      string          `json:"version"`
	Operation    string          `json:"operation"`
	Arguments    json.RawMessage `json:"arguments,omitempty"`
	PacketKey    *PacketKeySpec  `json:"packet_key,omitempty"`
	Input        AssetRef        `json:"input"`
	InputFormat  string          `json:"input_format"`
	Expected     AssetRef        `json:"expected"`
	Encoded      *AssetRef       `json:"encoded,omitempty"`
	Checkpoints  []string        `json:"checkpoints"`
	RustConsumer string          `json:"rust_consumer"`
}

// PacketKeySpec identifies a wire packet direction, state, and ID.
type PacketKeySpec struct {
	Direction string `json:"direction"`
	State     string `json:"state"`
	ID        uint32 `json:"id"`
}

// AssetRef identifies an input, expected outcome, or encoded binary file.
type AssetRef struct {
	Path   string `json:"path"`
	SHA256 string `json:"sha256"`
}

// InventoryError lists every coverage, version, or fixture failure.
type InventoryError struct {
	Problems []string
}

func (err *InventoryError) Error() string {
	if err == nil || len(err.Problems) == 0 {
		return "runtime-oracle: inventory error"
	}
	return "runtime-oracle: " + strings.Join(err.Problems, "; ")
}

// LoadInventory reads a frozen inventory JSON file.
func LoadInventory(path string) (Inventory, error) {
	info, err := os.Lstat(path)
	if err != nil {
		return Inventory{}, fmt.Errorf("runtime-oracle: stat inventory: %w", err)
	}
	if info.Mode()&os.ModeSymlink != 0 {
		return Inventory{}, fmt.Errorf("runtime-oracle: inventory cannot be a symlink: %s", path)
	}
	if !info.Mode().IsRegular() {
		return Inventory{}, fmt.Errorf("runtime-oracle: inventory must be a regular file: %s", path)
	}
	if info.Size() > MaxManifestBytes {
		return Inventory{}, fmt.Errorf("runtime-oracle: inventory size %d exceeds max %d", info.Size(), MaxManifestBytes)
	}
	data, err := os.ReadFile(path)
	if err != nil {
		return Inventory{}, fmt.Errorf("runtime-oracle: read inventory: %w", err)
	}
	if err := validateNoDuplicateKeys(data); err != nil {
		return Inventory{}, fmt.Errorf("runtime-oracle: inventory duplicate keys: %w", err)
	}
	var inventory Inventory
	dec := json.NewDecoder(bytes.NewReader(data))
	dec.UseNumber()
	if err := dec.Decode(&inventory); err != nil {
		return Inventory{}, fmt.Errorf("runtime-oracle: decode inventory: %w", err)
	}
	if inventory.SchemaVersion != inventorySchemaVersion {
		return Inventory{}, fmt.Errorf("runtime-oracle: inventory schema_version %d, want %d", inventory.SchemaVersion, inventorySchemaVersion)
	}
	if len(inventory.Cases) > MaxCases {
		return Inventory{}, fmt.Errorf("runtime-oracle: cases count %d exceeds maximum %d", len(inventory.Cases), MaxCases)
	}
	return inventory, nil
}

// reconcileInventory is the shared private validator for ReconcileWorking and ReconcileComplete.
func reconcileInventory(
	root string,
	inventory Inventory,
	discovered []Family,
	live Identities,
	consumers ConsumerRegistry,
	negativeExceptions NegativeCoverageExceptions,
	complete bool,
) (CoverageReport, error) {
	var problems []string
	problems = append(problems, identityProblems(inventory.Identities, live)...)

	if inventory.SchemaVersion != inventorySchemaVersion {
		problems = append(problems, fmt.Sprintf("inventory schema_version %d, want %d", inventory.SchemaVersion, inventorySchemaVersion))
	}
	if !sourceRevPattern.MatchString(inventory.SourceRevision) {
		problems = append(problems, fmt.Sprintf("invalid source_revision %q (must be 40 lowercase hex digits)", inventory.SourceRevision))
	}
	var registryProblems []string
	for name, registration := range consumers {
		if strings.TrimSpace(name) == "" {
			registryProblems = append(registryProblems, "consumer registry has empty name")
		}
		if registration.Kind != ConsumerRust && registration.Kind != ConsumerExternalGo {
			registryProblems = append(registryProblems, fmt.Sprintf("consumer registry entry %q has invalid kind %d", name, registration.Kind))
		}
		if len(registration.Routes) == 0 {
			registryProblems = append(registryProblems, fmt.Sprintf("consumer registry entry %q has no routes", name))
		}
	}
	if len(registryProblems) > 0 {
		problems = append(problems, registryProblems...)
		sort.Strings(problems)
		return CoverageReport{}, &InventoryError{Problems: problems}
	}

	inventoryByID := make(map[string]Family, len(inventory.Families))
	for _, family := range inventory.Families {
		if family.ID == "" {
			problems = append(problems, "inventory family has empty id")
			continue
		}
		if _, exists := inventoryByID[family.ID]; exists {
			problems = append(problems, "duplicate inventory family "+family.ID)
			continue
		}
		inventoryByID[family.ID] = family
	}

	discoveredByID := make(map[string]Family, len(discovered))
	for _, family := range discovered {
		discoveredByID[family.ID] = family
		listed, ok := inventoryByID[family.ID]
		if !ok {
			problems = append(problems, "uncovered family "+family.ID)
			continue
		}
		if listed.Kind != family.Kind {
			problems = append(problems, fmt.Sprintf("family %s kind %s does not match code %s", family.ID, listed.Kind, family.Kind))
		}
		if listed.Role != family.Role {
			problems = append(problems, fmt.Sprintf("family %s role %s does not match code %s", family.ID, listed.Role, family.Role))
		}
		if listed.CurrentVersion != family.CurrentVersion {
			problems = append(problems, fmt.Sprintf("family %s version %s does not match code %s", family.ID, listed.CurrentVersion, family.CurrentVersion))
		}
		if !sameStringSet(listed.SupportedVersions, family.SupportedVersions) {
			problems = append(problems, fmt.Sprintf("family %s supported versions %v do not match code %v", family.ID, listed.SupportedVersions, family.SupportedVersions))
		}
		if listed.Source != family.Source {
			problems = append(problems, fmt.Sprintf("family %s source %s does not match code %s", family.ID, listed.Source, family.Source))
		}
		if listed.EventualOwner != family.EventualOwner {
			problems = append(problems, fmt.Sprintf("family %s eventual owner %s does not match code %s", family.ID, listed.EventualOwner, family.EventualOwner))
		}
		if listed.NumericSemantics != family.NumericSemantics {
			problems = append(problems, fmt.Sprintf("family %s numeric semantics do not match code", family.ID))
		}
		if strings.TrimSpace(listed.Source) == "" {
			problems = append(problems, "family "+family.ID+" is missing source provenance")
		}
		if strings.TrimSpace(listed.EventualOwner) == "" {
			problems = append(problems, "family "+family.ID+" is missing eventual owner")
		}
		if strings.TrimSpace(listed.NumericSemantics) == "" {
			problems = append(problems, "family "+family.ID+" is missing numeric semantics")
		}

		if len(listed.Sources) == 0 {
			problems = append(problems, "family "+family.ID+" has no provenance sources")
			continue
		}
		for _, src := range listed.Sources {
			if err := validateCorpusPath(src.Path); err != nil {
				problems = append(problems, fmt.Sprintf("family %s source path: %v", family.ID, err))
				continue
			}
			if !hexSha256Pattern.MatchString(src.SHA256) {
				problems = append(problems, fmt.Sprintf("family %s source %s has invalid sha256 %s", family.ID, src.Path, src.SHA256))
			}
			fullPath := filepath.Join(root, filepath.FromSlash(src.Path))
			if err := checkNoSymlinks(root, src.Path); err != nil {
				if os.IsNotExist(err) {
					problems = append(problems, fmt.Sprintf("family %s source %s is missing", family.ID, src.Path))
				} else {
					problems = append(problems, fmt.Sprintf("family %s source %s has symlink: %v", family.ID, src.Path, err))
				}
				continue
			}
			if _, err := requireRegularFile(fullPath); err != nil {
				problems = append(problems, fmt.Sprintf("family %s source %s must be a regular file: %v", family.ID, src.Path, err))
				continue
			}
			hash, err := hashFile(fullPath)
			if err != nil {
				problems = append(problems, fmt.Sprintf("family %s source %s is missing or unreadable: %v", family.ID, src.Path, err))
			} else if hash != src.SHA256 {
				problems = append(problems, fmt.Sprintf("family %s source %s sha256 %s does not match disk %s", family.ID, src.Path, src.SHA256, hash))
			}
		}
	}

	for id := range inventoryByID {
		if _, ok := discoveredByID[id]; !ok {
			problems = append(problems, "inventory family "+id+" is not in current registries")
		}
	}

	for pt, rationale := range negativeExceptions {
		if strings.TrimSpace(rationale) == "" {
			problems = append(problems, fmt.Sprintf("negative coverage exception for %s/%s has empty rationale", pt.FamilyID, pt.Version))
		}
		fam, ok := inventoryByID[pt.FamilyID]
		if !ok || !containsString(fam.SupportedVersions, pt.Version) {
			problems = append(problems, fmt.Sprintf("negative coverage exception for unknown family/version %s/%s", pt.FamilyID, pt.Version))
		}
	}

	type pointCoverage struct {
		hasOK    bool
		hasError bool
	}
	pointCoverageMap := make(map[CoveragePoint]*pointCoverage)
	for _, family := range inventory.Families {
		for _, ver := range family.SupportedVersions {
			pt := CoveragePoint{FamilyID: family.ID, Version: ver}
			if _, exists := pointCoverageMap[pt]; !exists {
				pointCoverageMap[pt] = &pointCoverage{}
			}
		}
	}

	caseByID := make(map[string]CaseSpec, len(inventory.Cases))
	casesByFamily := make(map[string][]string)
	for _, c := range inventory.Cases {
		if c.ID == "" {
			problems = append(problems, "case has empty id")
			continue
		}
		if _, exists := caseByID[c.ID]; exists {
			problems = append(problems, "duplicate case "+c.ID)
			continue
		}
		caseByID[c.ID] = c
		casesByFamily[c.Family] = append(casesByFamily[c.Family], c.ID)

		kind, err := validateCaseSpecConsumer(root, c, inventoryByID, consumers)
		if err != nil {
			problems = append(problems, fmt.Sprintf("case %s: %v", c.ID, err))
		} else {
			pt := CoveragePoint{FamilyID: c.Family, Version: c.Version}
			if cov := pointCoverageMap[pt]; cov != nil {
				if kind == "ok" {
					cov.hasOK = true
				} else if kind == "error" {
					cov.hasError = true
				}
			}
		}
	}

	for familyID, family := range inventoryByID {
		expectedCases := casesByFamily[familyID]
		if !sameStringSet(family.Cases, expectedCases) {
			problems = append(problems, fmt.Sprintf("family %s cases %v do not match registered cases %v", familyID, family.Cases, expectedCases))
		}
	}

	problems = append(problems, requiredKindProblems(inventory.Families)...)

	var report CoverageReport
	seenPoints := make(map[CoveragePoint]bool)
	for _, family := range inventory.Families {
		for _, ver := range family.SupportedVersions {
			pt := CoveragePoint{FamilyID: family.ID, Version: ver}
			if seenPoints[pt] {
				continue
			}
			seenPoints[pt] = true

			cov := pointCoverageMap[pt]
			rationale, hasNegEx := negativeExceptions[pt]
			validException := hasNegEx && strings.TrimSpace(rationale) != ""

			if cov != nil && cov.hasOK && (cov.hasError || validException) {
				report.Covered = append(report.Covered, pt)
			} else {
				report.Uncovered = append(report.Uncovered, pt)
			}
		}
	}

	sortCoveragePoints(report.Covered)
	sortCoveragePoints(report.Uncovered)

	if complete {
		for _, pt := range report.Uncovered {
			problems = append(problems, fmt.Sprintf("uncovered point %s version %s", pt.FamilyID, pt.Version))
		}
	}

	if len(problems) == 0 {
		return report, nil
	}
	sort.Strings(problems)
	return report, &InventoryError{Problems: problems}
}

// validateCaseSpec validates one case specification against its family and the baseline consumer registry.
func validateCaseSpec(root string, c CaseSpec, families map[string]Family) error {
	_, err := validateCaseSpecConsumer(root, c, families, BaselineConsumerRegistry())
	return err
}

func validateCaseSpecConsumer(root string, c CaseSpec, families map[string]Family, consumers ConsumerRegistry) (string, error) {
	fam, ok := families[c.Family]
	if !ok {
		return "", fmt.Errorf("unknown family %s", c.Family)
	}
	expectedPrefix := c.Family + "/" + c.Version + "/"
	if !strings.HasPrefix(c.ID, expectedPrefix) || len(c.ID) <= len(expectedPrefix) {
		return "", fmt.Errorf("id %q must match %s<label>", c.ID, expectedPrefix)
	}
	if !containsString(fam.SupportedVersions, c.Version) {
		return "", fmt.Errorf("case version %q is absent from family supported_versions", c.Version)
	}
	if strings.TrimSpace(c.RustConsumer) == "" {
		return "", fmt.Errorf("missing rust_consumer")
	}
	registration, ok := consumers[c.RustConsumer]
	if !ok {
		return "", fmt.Errorf("unknown consumer %q", c.RustConsumer)
	}

	switch c.Operation {
	case "decode", "encode", "migrate", "admit", "order", "kernel", "agent-contract":
	default:
		return "", fmt.Errorf("invalid operation %q", c.Operation)
	}

	route := ConsumerRoute{FamilyID: c.Family, Version: c.Version, Operation: c.Operation}
	if _, ok := registration.Routes[route]; !ok {
		return "", fmt.Errorf(
			"consumer %q has unsupported route %s/%s/%s",
			c.RustConsumer,
			route.FamilyID,
			route.Version,
			route.Operation,
		)
	}

	inputMaxBytes, err := caseInputMaxBytes(c.InputFormat)
	if err != nil {
		return "", err
	}
	if len(c.Checkpoints) == 0 {
		return "", fmt.Errorf("empty checkpoints")
	}
	for _, cp := range c.Checkpoints {
		if _, err := strconv.ParseUint(cp, 10, 64); err != nil {
			return "", fmt.Errorf("invalid checkpoint %q (must be u64 decimal string)", cp)
		}
	}
	if strings.HasPrefix(c.Family, "save.") {
		if c.PacketKey != nil {
			return "", fmt.Errorf("save case %s must not carry packet_key", c.ID)
		}
		if c.RustConsumer != "mornlea_storage" {
			return "", fmt.Errorf("save case %s rust_consumer must be mornlea_storage", c.ID)
		}
		if c.InputFormat != "binary" {
			return "", fmt.Errorf("save case %s input_format must be binary", c.ID)
		}
		if len(c.Checkpoints) != 1 || c.Checkpoints[0] != "0" {
			return "", fmt.Errorf("save case %s checkpoints must be [\"0\"]", c.ID)
		}
		if err := validateStorageArguments(c.Family, c.Operation, c.Arguments); err != nil {
			return "", fmt.Errorf("save case %s arguments: %w", c.ID, err)
		}
	}

	// Validate input asset
	if err := validateAsset(root, c.Input, c.InputFormat == "json", inputMaxBytes); err != nil {
		return "", fmt.Errorf("input asset %s: %w", c.Input.Path, err)
	}
	if err := validateSaveCaseInputConstraints(root, c); err != nil {
		return "", fmt.Errorf("input asset %s: %w", c.Input.Path, err)
	}
	// Validate expected asset (always JSON) and extract kind
	kind, err := validateExpectedAsset(root, c.Expected)
	if err != nil {
		return "", fmt.Errorf("expected asset %s: %w", c.Expected.Path, err)
	}
	// Validate encoded asset (optional)
	if c.Encoded != nil {
		if err := validateAsset(root, *c.Encoded, false, MaxBinaryBytes); err != nil {
			return "", fmt.Errorf("encoded asset %s: %w", c.Encoded.Path, err)
		}
	}
	if strings.HasPrefix(c.Family, "save.") {
		switch c.Operation {
		case "decode", "order":
			if c.Encoded != nil {
				return "", fmt.Errorf("save case %s operation %s must not carry encoded asset", c.ID, c.Operation)
			}
		case "encode":
			if kind == "ok" && c.Encoded == nil {
				return "", fmt.Errorf("save case %s successful encode requires encoded asset", c.ID)
			}
			if kind == "error" && c.Encoded != nil {
				return "", fmt.Errorf("save case %s error encode must not carry encoded asset", c.ID)
			}
		}
	}

	return kind, nil
}

func validateExpectedAsset(root string, asset AssetRef) (string, error) {
	if err := validateCorpusPath(asset.Path); err != nil {
		return "", err
	}
	if !hexSha256Pattern.MatchString(asset.SHA256) {
		return "", fmt.Errorf("invalid sha256 %s", asset.SHA256)
	}
	if !strings.HasSuffix(asset.Path, ".json") {
		return "", fmt.Errorf("json asset must have .json extension: %s", asset.Path)
	}
	if err := checkNoSymlinks(root, asset.Path); err != nil {
		return "", err
	}
	fullPath := filepath.Join(root, filepath.FromSlash(asset.Path))
	info, err := os.Stat(fullPath)
	if err != nil {
		return "", fmt.Errorf("stat: %w", err)
	}
	if !info.Mode().IsRegular() {
		return "", fmt.Errorf("path must identify a regular file: %s", asset.Path)
	}
	if info.Size() > MaxCaseJSONBytes {
		return "", fmt.Errorf("file size %d exceeds budget %d", info.Size(), MaxCaseJSONBytes)
	}
	hash, err := hashFile(fullPath)
	if err != nil {
		return "", fmt.Errorf("hash: %w", err)
	}
	if hash != asset.SHA256 {
		return "", fmt.Errorf("sha256 %s does not match disk %s", asset.SHA256, hash)
	}
	content, err := os.ReadFile(fullPath)
	if err != nil {
		return "", err
	}
	if err := validateNoDuplicateKeys(content); err != nil {
		return "", fmt.Errorf("duplicate json keys: %w", err)
	}

	var payload map[string]any
	dec := json.NewDecoder(bytes.NewReader(content))
	dec.UseNumber()
	if err := dec.Decode(&payload); err != nil {
		return "", fmt.Errorf("decode expected json: %w", err)
	}
	var extra any
	if err := dec.Decode(&extra); err != io.EOF {
		return "", fmt.Errorf("unexpected trailing content in expected json")
	}
	rawKind, ok := payload["kind"]
	if !ok {
		return "", fmt.Errorf("expected outcome missing top-level kind")
	}
	kind, ok := rawKind.(string)
	if !ok || (kind != "ok" && kind != "error") {
		return "", fmt.Errorf("expected outcome kind must be \"ok\" or \"error\", got %v", rawKind)
	}
	return kind, nil
}

func containsString(slice []string, s string) bool {
	for _, v := range slice {
		if v == s {
			return true
		}
	}
	return false
}

func sortCoveragePoints(points []CoveragePoint) {
	sort.Slice(points, func(i, j int) bool {
		if points[i].FamilyID != points[j].FamilyID {
			return points[i].FamilyID < points[j].FamilyID
		}
		return points[i].Version < points[j].Version
	})
}

func validateAsset(root string, asset AssetRef, isJSON bool, maxBytes int64) error {
	if err := validateCorpusPath(asset.Path); err != nil {
		return err
	}
	if !hexSha256Pattern.MatchString(asset.SHA256) {
		return fmt.Errorf("invalid sha256 %s", asset.SHA256)
	}
	if isJSON && !strings.HasSuffix(asset.Path, ".json") {
		return fmt.Errorf("json asset must have .json extension: %s", asset.Path)
	}
	if !isJSON && strings.HasSuffix(asset.Path, ".go") {
		return fmt.Errorf("binary asset cannot be a Go source file: %s", asset.Path)
	}
	if err := checkNoSymlinks(root, asset.Path); err != nil {
		return err
	}
	fullPath := filepath.Join(root, filepath.FromSlash(asset.Path))
	info, err := os.Stat(fullPath)
	if err != nil {
		return fmt.Errorf("stat: %w", err)
	}
	if !info.Mode().IsRegular() {
		return fmt.Errorf("path must identify a regular file: %s", asset.Path)
	}
	if info.Size() > maxBytes {
		return fmt.Errorf("file size %d exceeds budget %d", info.Size(), maxBytes)
	}
	hash, err := hashFile(fullPath)
	if err != nil {
		return fmt.Errorf("hash: %w", err)
	}
	if hash != asset.SHA256 {
		return fmt.Errorf("sha256 %s does not match disk %s", asset.SHA256, hash)
	}
	if isJSON {
		content, err := os.ReadFile(fullPath)
		if err != nil {
			return err
		}
		if err := validateNoDuplicateKeys(content); err != nil {
			return fmt.Errorf("duplicate json keys: %w", err)
		}
	}
	return nil
}

func caseInputMaxBytes(inputFormat string) (int64, error) {
	switch inputFormat {
	case "json":
		return MaxCaseJSONBytes, nil
	case "binary":
		return MaxBinaryBytes, nil
	default:
		return 0, fmt.Errorf("invalid input_format %q (must be 'binary' or 'json')", inputFormat)
	}
}

func validateCorpusPath(p string) error {
	if p == "" {
		return fmt.Errorf("path cannot be empty")
	}
	if filepath.IsAbs(p) || strings.HasPrefix(p, "/") || strings.HasPrefix(p, "\\") {
		return fmt.Errorf("path %q cannot be absolute", p)
	}
	if strings.Contains(p, "\\") {
		return fmt.Errorf("path %q must use forward slashes", p)
	}
	for _, part := range strings.Split(p, "/") {
		if part == ".." || part == "." {
			return fmt.Errorf("path %q cannot contain parent or current directory elements", p)
		}
	}
	return nil
}

func checkNoSymlinks(root, rel string) error {
	parts := strings.Split(rel, "/")
	curr := root
	for _, part := range parts {
		curr = filepath.Join(curr, part)
		info, err := os.Lstat(curr)
		if err != nil {
			return err
		}
		if info.Mode()&os.ModeSymlink != 0 {
			return fmt.Errorf("path component %s is a symlink", curr)
		}
	}
	return nil
}

func hashFile(path string) (string, error) {
	if _, err := requireRegularFile(path); err != nil {
		return "", err
	}
	data, err := os.ReadFile(path)
	if err != nil {
		return "", err
	}
	sum := sha256.Sum256(data)
	return fmt.Sprintf("sha256:%x", sum), nil
}

func requireRegularFile(path string) (os.FileInfo, error) {
	info, err := os.Lstat(path)
	if err != nil {
		return nil, err
	}
	if !info.Mode().IsRegular() {
		return nil, fmt.Errorf("path %s is not a regular file", path)
	}
	return info, nil
}

// CanonicalManifestBytes serializes manifest JSON with recursively sorted keys,
// no insignificant whitespace, and one terminal newline.
func CanonicalManifestBytes(inv Inventory) ([]byte, error) {
	raw, err := json.Marshal(inv)
	if err != nil {
		return nil, err
	}
	var generic any
	dec := json.NewDecoder(bytes.NewReader(raw))
	dec.UseNumber()
	if err := dec.Decode(&generic); err != nil {
		return nil, err
	}
	var buf bytes.Buffer
	if err := writeCanonicalJSON(&buf, generic); err != nil {
		return nil, err
	}
	buf.WriteByte('\n')
	return buf.Bytes(), nil
}

// CanonicalCorpusDigest computes the schema-2 canonical corpus digest.
func CanonicalCorpusDigest(inv Inventory) (string, error) {
	manifestBytes, err := CanonicalManifestBytes(inv)
	if err != nil {
		return "", err
	}
	hasher := sha256.New()
	hasher.Write([]byte("mornlea-corpus-v2\n"))
	hasher.Write(manifestBytes)

	sortedCases := append([]CaseSpec(nil), inv.Cases...)
	sort.Slice(sortedCases, func(i, j int) bool {
		return sortedCases[i].ID < sortedCases[j].ID
	})

	for _, c := range sortedCases {
		encodedSHA := ""
		if c.Encoded != nil {
			encodedSHA = c.Encoded.SHA256
		}
		record := fmt.Sprintf("%s\x00%s\x00%s\x00%s\n", c.ID, c.Input.SHA256, c.Expected.SHA256, encodedSHA)
		hasher.Write([]byte(record))
	}
	return fmt.Sprintf("sha256:%x", hasher.Sum(nil)), nil
}

func writeCanonicalJSON(buf *bytes.Buffer, val any) error {
	switch v := val.(type) {
	case map[string]any:
		buf.WriteByte('{')
		keys := make([]string, 0, len(v))
		for k := range v {
			keys = append(keys, k)
		}
		sort.Strings(keys)
		for i, k := range keys {
			if i > 0 {
				buf.WriteByte(',')
			}
			keyBytes, err := json.Marshal(k)
			if err != nil {
				return err
			}
			buf.Write(keyBytes)
			buf.WriteByte(':')
			if err := writeCanonicalJSON(buf, v[k]); err != nil {
				return err
			}
		}
		buf.WriteByte('}')
	case []any:
		buf.WriteByte('[')
		for i, item := range v {
			if i > 0 {
				buf.WriteByte(',')
			}
			if err := writeCanonicalJSON(buf, item); err != nil {
				return err
			}
		}
		buf.WriteByte(']')
	default:
		raw, err := json.Marshal(v)
		if err != nil {
			return err
		}
		buf.Write(raw)
	}
	return nil
}

func validateNoDuplicateKeys(data []byte) error {
	dec := json.NewDecoder(bytes.NewReader(data))
	type objFrame struct {
		isObj     bool
		expectKey bool
		keys      map[string]bool
	}
	var stack []objFrame
	for {
		tok, err := dec.Token()
		if err == io.EOF {
			break
		}
		if err != nil {
			return err
		}
		switch t := tok.(type) {
		case json.Delim:
			switch t {
			case '{':
				if len(stack) > 0 && stack[len(stack)-1].isObj && stack[len(stack)-1].expectKey {
					return fmt.Errorf("unexpected '{' when expecting object key")
				}
				stack = append(stack, objFrame{isObj: true, expectKey: true, keys: make(map[string]bool)})
			case '}':
				if len(stack) == 0 || !stack[len(stack)-1].isObj {
					return fmt.Errorf("unexpected '}'")
				}
				stack = stack[:len(stack)-1]
				if len(stack) > 0 && stack[len(stack)-1].isObj {
					stack[len(stack)-1].expectKey = true
				}
			case '[':
				if len(stack) > 0 && stack[len(stack)-1].isObj && stack[len(stack)-1].expectKey {
					return fmt.Errorf("unexpected '[' when expecting object key")
				}
				stack = append(stack, objFrame{isObj: false})
			case ']':
				if len(stack) == 0 || stack[len(stack)-1].isObj {
					return fmt.Errorf("unexpected ']'")
				}
				stack = stack[:len(stack)-1]
				if len(stack) > 0 && stack[len(stack)-1].isObj {
					stack[len(stack)-1].expectKey = true
				}
			}
		case string:
			if len(stack) > 0 && stack[len(stack)-1].isObj {
				f := &stack[len(stack)-1]
				if f.expectKey {
					if f.keys[t] {
						return fmt.Errorf("duplicate key %q", t)
					}
					f.keys[t] = true
					f.expectKey = false
				} else {
					f.expectKey = true
				}
			}
		default:
			if len(stack) > 0 && stack[len(stack)-1].isObj {
				stack[len(stack)-1].expectKey = true
			}
		}
	}
	return nil
}

func identityProblems(got, live Identities) []string {
	var problems []string
	if live.Protocol == 0 || live.ChunkSchema == 0 || live.PlayerSchema == 0 ||
		live.WorldMetadata == 0 || live.CompanionsAISchema == 0 || live.HostileMobsSchema == 0 ||
		live.PassiveMobsSchema == 0 || live.EngineABI == 0 || live.RegionFormat == 0 ||
		live.AgentHTTP == "" || live.AgentMCP == "" {
		problems = append(problems, "incomplete live contract identity")
	}
	if got.Protocol == 0 || got.ChunkSchema == 0 || got.PlayerSchema == 0 ||
		got.WorldMetadata == 0 || got.CompanionsAISchema == 0 || got.HostileMobsSchema == 0 ||
		got.PassiveMobsSchema == 0 || got.EngineABI == 0 || got.RegionFormat == 0 ||
		got.AgentHTTP == "" || got.AgentMCP == "" {
		problems = append(problems, "incomplete inventory identity")
	}
	if got.Protocol != live.Protocol {
		problems = append(problems, fmt.Sprintf("protocol version %d does not match code %d", got.Protocol, live.Protocol))
	}
	if got.ChunkSchema != live.ChunkSchema {
		problems = append(problems, fmt.Sprintf("chunk schema %d does not match code %d", got.ChunkSchema, live.ChunkSchema))
	}
	if got.PlayerSchema != live.PlayerSchema {
		problems = append(problems, fmt.Sprintf("player schema %d does not match code %d", got.PlayerSchema, live.PlayerSchema))
	}
	if got.WorldMetadata != live.WorldMetadata {
		problems = append(problems, fmt.Sprintf("world metadata %d does not match code %d", got.WorldMetadata, live.WorldMetadata))
	}
	if got.CompanionsAISchema != live.CompanionsAISchema {
		problems = append(problems, fmt.Sprintf("companions.ai schema %d does not match code %d", got.CompanionsAISchema, live.CompanionsAISchema))
	}
	if got.HostileMobsSchema != live.HostileMobsSchema {
		problems = append(problems, fmt.Sprintf("hostile_mobs schema %d does not match code %d", got.HostileMobsSchema, live.HostileMobsSchema))
	}
	if got.PassiveMobsSchema != live.PassiveMobsSchema {
		problems = append(problems, fmt.Sprintf("passive_mobs schema %d does not match code %d", got.PassiveMobsSchema, live.PassiveMobsSchema))
	}
	if got.EngineABI != live.EngineABI {
		problems = append(problems, fmt.Sprintf("engine ABI %d does not match code %d", got.EngineABI, live.EngineABI))
	}
	if got.RegionFormat != live.RegionFormat {
		problems = append(problems, fmt.Sprintf("region format %d does not match code %d", got.RegionFormat, live.RegionFormat))
	}
	if got.AgentHTTP != live.AgentHTTP {
		problems = append(problems, fmt.Sprintf("agent HTTP %s does not match code %s", got.AgentHTTP, live.AgentHTTP))
	}
	if got.AgentMCP != live.AgentMCP {
		problems = append(problems, fmt.Sprintf("agent MCP %s does not match code %s", got.AgentMCP, live.AgentMCP))
	}
	return problems
}

func requiredKindProblems(families []Family) []string {
	seen := map[string]bool{}
	roles := map[string]bool{}
	for _, family := range families {
		seen[family.Kind] = true
		roles[family.Role] = true
	}
	var problems []string
	for _, kind := range []string{"domain", "protocol", "save", "kernel", "agent"} {
		if !seen[kind] {
			problems = append(problems, "inventory is missing kind "+kind)
		}
	}
	for _, role := range []string{"input", "event"} {
		if !roles[role] {
			problems = append(problems, "inventory is missing semantic role "+role)
		}
	}
	return problems
}

func sameStringSet(left, right []string) bool {
	if len(left) != len(right) {
		return false
	}
	counts := make(map[string]int, len(left))
	for _, value := range left {
		counts[value]++
	}
	for _, value := range right {
		counts[value]--
		if counts[value] < 0 {
			return false
		}
	}
	return true
}
