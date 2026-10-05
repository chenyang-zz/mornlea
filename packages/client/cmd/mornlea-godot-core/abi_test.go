package main

import (
	"os"
	"path/filepath"
	"runtime"
	"strconv"
	"strings"
	"testing"

	"github.com/channing771/mornlea/packages/client/presentation"
	clientruntime "github.com/channing771/mornlea/packages/client/runtime"
	"github.com/channing771/mornlea/packages/shared/core"
)

// TestABIIdentityMatchesClientCoreContract pins identity values with explicit
// literals. The cgo-derived constants in abi.go follow the header mechanically,
// so the equalities below are trivially true while both sides sit at one
// version; the pins turn silent co-version drift (header and constants bumped
// together without review) into a failing test, mirroring the engine ABI pin
// pattern in packages/shared/nativeabi.
func TestABIIdentityMatchesClientCoreContract(t *testing.T) {
	if ABIMajor != 1 || ABIMinor != 1 {
		t.Fatalf("client-core ABI = %d.%d, want 1.1", ABIMajor, ABIMinor)
	}

	statuses := []Status{
		StatusOK,
		StatusInvalidArgument,
		StatusABIMismatch,
		StatusInputRejected,
		StatusInsufficientCapacity,
		StatusInvalidHandle,
		StatusInvalidState,
		StatusDisconnected,
		StatusInternal,
		StatusPanic,
	}
	for index, status := range statuses {
		if want := uint32(index); uint32(status) != want {
			t.Fatalf("status %d = %d, want %d", index, uint32(status), want)
		}
	}
	if uint32(StatusCount) != uint32(len(statuses)) {
		t.Fatalf("status count = %d, want %d", StatusCount, len(statuses))
	}

	families := []Family{
		FamilyIdentity,
		FamilyConnection,
		FamilyInput,
		FamilyStep,
		FamilyWorld,
		FamilyFrame,
		FamilyStatus,
		FamilyEnvironment,
	}
	for index, family := range families {
		if want := uint32(index + 1); uint32(family) != want {
			t.Fatalf("family %d = %d, want %d", index, uint32(family), want)
		}
	}
	if uint32(FamilyCount) != uint32(len(families)) {
		t.Fatalf("family count = %d, want %d", FamilyCount, len(families))
	}

	if ABIAlignment != 8 {
		t.Fatalf("ABI alignment = %d, want 8", ABIAlignment)
	}

	// Every magic tag is composed from the shared client-core prefix, one
	// distinct family character, and the format generation digit, so the tag
	// set is reviewable instead of nine unrelated numbers.
	familyMagics := []struct {
		family Family
		magic  Magic
		char   byte
	}{
		{FamilyIdentity, MagicIdentity, 'I'},
		{FamilyConnection, MagicConnection, 'C'},
		{FamilyInput, MagicInput, 'N'},
		{FamilyStep, MagicStep, 'S'},
		{FamilyWorld, MagicWorld, 'W'},
		{FamilyFrame, MagicFrame, 'F'},
		{FamilyStatus, MagicStatus, 'M'},
	}
	seenMagics := make(map[Magic]bool, len(familyMagics))
	for _, tag := range familyMagics {
		want := Magic(uint32(MagicTagPrefix) | uint32(tag.char)<<16 | uint32(MagicGeneration)<<24)
		if tag.magic != want {
			t.Fatalf("family %d magic = %#x, want composed %#x", tag.family, tag.magic, want)
		}
		if seenMagics[tag.magic] {
			t.Fatalf("family %d reuses magic %#x", tag.family, tag.magic)
		}
		seenMagics[tag.magic] = true
	}

	// The generation digit is tied to the ABI major: generation "1" covers
	// every major-1 layout era and moves only with a new major.
	if want := Magic('0' + byte(ABIMajor)); MagicGeneration != want {
		t.Fatalf("magic generation = %#x, want '0' + ABI major (%#x)", MagicGeneration, want)
	}

	// Family contract versions start at one for the client-core pilot. The
	// pairs stay in a slice instead of a constant-keyed map so packages that
	// type-check this file without resolving cgo constants see valid syntax.
	familyVersions := []struct {
		family  Family
		version uint32
	}{
		{FamilyIdentity, IdentityVersion},
		{FamilyConnection, ConnectionVersion},
		{FamilyInput, InputVersion},
		{FamilyStep, StepVersion},
		{FamilyWorld, WorldVersion},
		{FamilyFrame, FrameVersion},
		{FamilyStatus, StatusVersion},
	}
	for _, entry := range familyVersions {
		if entry.version != 1 {
			t.Fatalf("family %d version = %d, want 1", entry.family, entry.version)
		}
	}
}

// TestABIHeaderTextPinsEveryDefine parses every numeric #define in the header
// text and requires the parsed set to equal exactly the cgo-derived constants.
// Value comparisons alone would let a new or removed define slip through while
// both sides still compile, so the name sets are compared in both directions.
func TestABIHeaderTextPinsEveryDefine(t *testing.T) {
	parsed := parseHeaderDefines(t)
	expected := expectedHeaderDefines()
	for name, want := range expected {
		got, ok := parsed[name]
		if !ok {
			t.Errorf("header is missing define %s", name)
			continue
		}
		if got != want {
			t.Errorf("define %s = %#x, want %#x", name, got, want)
		}
	}
	for name := range parsed {
		if _, ok := expected[name]; !ok {
			t.Errorf("header define %s has no pinned Go constant", name)
		}
	}
}

// TestABILayoutMatchesHeaderStructs validates the fixed-header C layout through
// the cgo-derived facts in abi.go: each struct's size equals its declared byte
// constant, every size is a multiple of the ABI alignment, exact member offsets
// hold, 64-bit members sit only at 8-byte offsets, and each struct keeps
// natural C alignment (8 for headers carrying uint64_t members, 4 otherwise).
// The mornlea_godot abi module mirrors the same numbers, so a header edit
// cannot drift from either consumer unnoticed.
func TestABILayoutMatchesHeaderStructs(t *testing.T) {
	facts := headerLayoutFacts()
	sizes := []struct {
		name     string
		declared uint32
	}{
		{"identity header size", IdentityHeaderBytes},
		{"family descriptor size", FamilyDescriptorBytes},
		{"connection header size", ConnectionHeaderBytes},
		{"input header size", InputHeaderBytes},
		{"step request size", StepRequestBytes},
		{"world header size", WorldHeaderBytes},
		{"frame header size", FrameHeaderBytes},
		{"status header size", StatusHeaderBytes},
	}
	for _, size := range sizes {
		got, ok := facts[size.name]
		if !ok {
			t.Errorf("layout facts are missing %s", size.name)
			continue
		}
		if got != uintptr(size.declared) {
			t.Errorf("%s = %d, want %d", size.name, got, size.declared)
		}
		if got%uintptr(ABIAlignment) != 0 {
			t.Errorf("%s %d is not a multiple of the ABI alignment %d",
				size.name, got, ABIAlignment)
		}
	}

	alignments := []struct {
		name string
		want uintptr
	}{
		{"identity header align", 4},
		{"family descriptor align", 4},
		{"connection header align", 4},
		{"input header align", 4},
		{"step request align", 8},
		{"world header align", 8},
		{"frame header align", 8},
		{"status header align", 4},
	}
	for _, alignment := range alignments {
		got, ok := facts[alignment.name]
		if !ok {
			t.Errorf("layout facts are missing %s", alignment.name)
			continue
		}
		if got != alignment.want {
			t.Errorf("%s = %d, want %d", alignment.name, got, alignment.want)
		}
	}

	offsets := []struct {
		name string
		want uintptr
	}{
		{"identity header magic offset", 0},
		{"identity header abi_major offset", 8},
		{"identity header family_count offset", 16},
		{"identity header reserved offset", 20},
		{"family descriptor family offset", 0},
		{"family descriptor record_limit offset", 8},
		{"family descriptor record_bytes offset", 12},
		{"family descriptor reserved offset", 16},
		{"connection header address_len offset", 8},
		{"input header event_count offset", 8},
		{"step request elapsed_ns offset", 8},
		{"step request message_budget offset", 16},
		{"step request mesh_budget offset", 20},
		{"world header operation_count offset", 8},
		{"world header quad_count offset", 12},
		{"world header epoch offset", 16},
		{"world header atlas_revision offset", 24},
		{"frame header frame_version offset", 8},
		{"frame header entity_count offset", 12},
		{"frame header name_len offset", 16},
		{"frame header reserved offset", 20},
		{"frame header revision offset", 24},
		{"frame header epoch offset", 32},
		{"status header record_count offset", 8},
	}
	for _, member := range offsets {
		got, ok := facts[member.name]
		if !ok {
			t.Errorf("layout facts are missing %s", member.name)
			continue
		}
		if got != member.want {
			t.Errorf("%s = %d, want %d", member.name, got, member.want)
		}
	}
}

// TestABILimitsMatchFrozenPresentationContracts keeps the header's bounded
// family limits equal to the frozen Go presentation and runtime constants that
// own them. Any change on either side must be a reviewed contract change, not
// silent drift between the ABI and the platform-independent client core.
func TestABILimitsMatchFrozenPresentationContracts(t *testing.T) {
	if got, want := SectionEdgeBlocks, uint32(core.SectionSize); got != want {
		t.Errorf("section edge blocks = %d, want %d", got, want)
	}
	if got, want := WorldYBiasBlocks, uint32(-core.MinY); got != want {
		t.Errorf("world Y bias blocks = %d, want %d", got, want)
	}
	if got, want := MaxWorldBatchOperations, uint32(presentation.MaxWorldBatchOperations); got != want {
		t.Errorf("world batch operations = %d, want %d", got, want)
	}
	if got, want := MaxSectionMeshQuads, uint32(presentation.MaxSectionMeshQuads); got != want {
		t.Errorf("section mesh quads = %d, want %d", got, want)
	}
	if got, want := MaxWorldBatchQuads, uint32(presentation.MaxWorldBatchPackedQuads); got != want {
		t.Errorf("world batch quads = %d, want %d", got, want)
	}
	if got, want := MaxEntityRecords, uint32(presentation.MaxEntityBatchRecords); got != want {
		t.Errorf("entity records = %d, want %d", got, want)
	}
	if got, want := MaxStepMessageBudget, uint32(clientruntime.MaxStepMessageBudget); got != want {
		t.Errorf("step message budget = %d, want %d", got, want)
	}
	if got, want := MaxStepMeshBudget, uint32(presentation.MaxWorldBatchOperations); got != want {
		t.Errorf("step mesh budget = %d, want %d", got, want)
	}
	if got, want := FrameSnapshotVersion, uint32(presentation.FrameSnapshotVersion); got != want {
		t.Errorf("frame snapshot version = %d, want %d", got, want)
	}
}

// parseHeaderDefines reads include/mornlea_client_core.h next to this test and
// returns every numeric `#define MORNLEA_CLIENT_*` as name to value. Header
// defines stay single-line literals (decimal or 0x hex, optional `u` suffix)
// so both cross-language parsers can read them without a C preprocessor.
func parseHeaderDefines(t *testing.T) map[string]uint32 {
	t.Helper()
	_, file, _, ok := runtime.Caller(0)
	if !ok {
		t.Fatal("cannot locate the abi test file")
	}
	data, err := os.ReadFile(filepath.Join(filepath.Dir(file), "include", "mornlea_client_core.h"))
	if err != nil {
		t.Fatalf("read header: %v", err)
	}
	defines := make(map[string]uint32)
	for number, line := range strings.Split(string(data), "\n") {
		trimmed := strings.TrimSpace(line)
		if !strings.HasPrefix(trimmed, "#define MORNLEA_CLIENT_") {
			continue
		}
		fields := strings.Fields(trimmed)
		if len(fields) == 2 && fields[1] == "MORNLEA_CLIENT_CORE_H" {
			// The include guard carries no value; skip it.
			continue
		}
		if len(fields) != 3 {
			t.Fatalf("header line %d is not a single-line literal define: %q", number+1, trimmed)
		}
		value, err := strconv.ParseUint(strings.TrimSuffix(fields[2], "u"), 0, 32)
		if err != nil {
			t.Fatalf("header line %d define value %q: %v", number+1, fields[2], err)
		}
		if _, exists := defines[fields[1]]; exists {
			t.Fatalf("header defines %s twice", fields[1])
		}
		defines[fields[1]] = uint32(value)
	}
	if len(defines) == 0 {
		t.Fatal("header defines were not parsed")
	}
	return defines
}

// expectedHeaderDefines maps every header define to its cgo-derived Go
// constant. A define added to the header without a Go pin (or vice versa)
// fails TestABIHeaderTextPinsEveryDefine in the corresponding direction.
func expectedHeaderDefines() map[string]uint32 {
	return map[string]uint32{
		"MORNLEA_CLIENT_ABI_MAJOR":                    ABIMajor,
		"MORNLEA_CLIENT_ABI_MINOR":                    ABIMinor,
		"MORNLEA_CLIENT_STATUS_OK":                    uint32(StatusOK),
		"MORNLEA_CLIENT_STATUS_INVALID_ARGUMENT":      uint32(StatusInvalidArgument),
		"MORNLEA_CLIENT_STATUS_ABI_MISMATCH":          uint32(StatusABIMismatch),
		"MORNLEA_CLIENT_STATUS_INPUT_REJECTED":        uint32(StatusInputRejected),
		"MORNLEA_CLIENT_STATUS_INSUFFICIENT_CAPACITY": uint32(StatusInsufficientCapacity),
		"MORNLEA_CLIENT_STATUS_INVALID_HANDLE":        uint32(StatusInvalidHandle),
		"MORNLEA_CLIENT_STATUS_INVALID_STATE":         uint32(StatusInvalidState),
		"MORNLEA_CLIENT_STATUS_DISCONNECTED":          uint32(StatusDisconnected),
		"MORNLEA_CLIENT_STATUS_INTERNAL":              uint32(StatusInternal),
		"MORNLEA_CLIENT_STATUS_PANIC":                 uint32(StatusPanic),
		"MORNLEA_CLIENT_STATUS_COUNT":                 uint32(StatusCount),
		"MORNLEA_CLIENT_MAGIC_TAG_PREFIX":             uint32(MagicTagPrefix),
		"MORNLEA_CLIENT_MAGIC_GENERATION":             uint32(MagicGeneration),
		"MORNLEA_CLIENT_MAGIC_IDENTITY":               uint32(MagicIdentity),
		"MORNLEA_CLIENT_MAGIC_CONNECTION":             uint32(MagicConnection),
		"MORNLEA_CLIENT_MAGIC_INPUT":                  uint32(MagicInput),
		"MORNLEA_CLIENT_MAGIC_STEP":                   uint32(MagicStep),
		"MORNLEA_CLIENT_MAGIC_WORLD":                  uint32(MagicWorld),
		"MORNLEA_CLIENT_MAGIC_FRAME":                  uint32(MagicFrame),
		"MORNLEA_CLIENT_MAGIC_STATUS":                 uint32(MagicStatus),
		"MORNLEA_CLIENT_MAGIC_ENVIRONMENT":            uint32(MagicEnvironment),
		"MORNLEA_CLIENT_ABI_ALIGNMENT":                ABIAlignment,
		"MORNLEA_CLIENT_FAMILY_IDENTITY":              uint32(FamilyIdentity),
		"MORNLEA_CLIENT_FAMILY_CONNECTION":            uint32(FamilyConnection),
		"MORNLEA_CLIENT_FAMILY_INPUT":                 uint32(FamilyInput),
		"MORNLEA_CLIENT_FAMILY_STEP":                  uint32(FamilyStep),
		"MORNLEA_CLIENT_FAMILY_WORLD":                 uint32(FamilyWorld),
		"MORNLEA_CLIENT_FAMILY_FRAME":                 uint32(FamilyFrame),
		"MORNLEA_CLIENT_FAMILY_STATUS":                uint32(FamilyStatus),
		"MORNLEA_CLIENT_FAMILY_ENVIRONMENT":           uint32(FamilyEnvironment),
		"MORNLEA_CLIENT_FAMILY_COUNT":                 uint32(FamilyCount),
		"MORNLEA_CLIENT_IDENTITY_VERSION":             IdentityVersion,
		"MORNLEA_CLIENT_CONNECTION_VERSION":           ConnectionVersion,
		"MORNLEA_CLIENT_INPUT_VERSION":                InputVersion,
		"MORNLEA_CLIENT_STEP_VERSION":                 StepVersion,
		"MORNLEA_CLIENT_WORLD_VERSION":                WorldVersion,
		"MORNLEA_CLIENT_SECTION_EDGE_BLOCKS":          SectionEdgeBlocks,
		"MORNLEA_CLIENT_WORLD_Y_BIAS_BLOCKS":          WorldYBiasBlocks,
		"MORNLEA_CLIENT_FRAME_VERSION":                FrameVersion,
		"MORNLEA_CLIENT_STATUS_VERSION":               StatusVersion,
		"MORNLEA_CLIENT_ENVIRONMENT_VERSION":          EnvironmentVersion,
		"MORNLEA_CLIENT_ENVIRONMENT_BYTES":            EnvironmentBytes,
		"MORNLEA_CLIENT_MAX_INPUT_EVENTS":             MaxInputEvents,
		"MORNLEA_CLIENT_MAX_CONNECTION_ADDRESS_BYTES": MaxConnectionAddressBytes,
		"MORNLEA_CLIENT_MAX_STEP_MESSAGE_BUDGET":      MaxStepMessageBudget,
		"MORNLEA_CLIENT_MAX_STEP_MESH_BUDGET":         MaxStepMeshBudget,
		"MORNLEA_CLIENT_MAX_WORLD_BATCH_OPERATIONS":   MaxWorldBatchOperations,
		"MORNLEA_CLIENT_MAX_SECTION_MESH_QUADS":       MaxSectionMeshQuads,
		"MORNLEA_CLIENT_MAX_WORLD_BATCH_QUADS":        MaxWorldBatchQuads,
		"MORNLEA_CLIENT_MAX_ENTITY_RECORDS":           MaxEntityRecords,
		"MORNLEA_CLIENT_MAX_TARGET_NAME_BYTES":        MaxTargetNameBytes,
		"MORNLEA_CLIENT_MAX_STATUS_RECORDS":           MaxStatusRecords,
		"MORNLEA_CLIENT_FRAME_SNAPSHOT_VERSION":       FrameSnapshotVersion,
		"MORNLEA_CLIENT_IDENTITY_HEADER_BYTES":        IdentityHeaderBytes,
		"MORNLEA_CLIENT_FAMILY_DESCRIPTOR_BYTES":      FamilyDescriptorBytes,
		"MORNLEA_CLIENT_CONNECTION_HEADER_BYTES":      ConnectionHeaderBytes,
		"MORNLEA_CLIENT_INPUT_HEADER_BYTES":           InputHeaderBytes,
		"MORNLEA_CLIENT_STEP_REQUEST_BYTES":           StepRequestBytes,
		"MORNLEA_CLIENT_WORLD_HEADER_BYTES":           WorldHeaderBytes,
		"MORNLEA_CLIENT_FRAME_HEADER_BYTES":           FrameHeaderBytes,
		"MORNLEA_CLIENT_STATUS_HEADER_BYTES":          StatusHeaderBytes,
	}
}
