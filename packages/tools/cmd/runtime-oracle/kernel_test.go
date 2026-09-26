package main

import (
	"bytes"
	"crypto/sha256"
	"encoding/binary"
	"encoding/hex"
	"encoding/json"
	"fmt"
	"os"
	"path/filepath"
	"strings"
	"testing"

	"github.com/channing771/mornlea/packages/shared/nativeabi"
)

// TestKernelOracle executes every imported numerical kernel case through the
// public engine bridge and compares the normalized observation against the
// frozen expectation. Error observations reach this package as bridge panics
// with stable text, which the oracle maps back to the ABI status the
// package-local raw producer observed directly; a panic outside that contract
// fails the case instead of silently mapping. Cases the public bridge cannot
// express raw (the two-phase short-capacity probes) are pinned by case ID,
// category and digest here while their execution lives in the nativeabi raw
// oracle of the same change.

// kernelCaseExpect pins one imported case: its identity, its frozen category
// and whether it is the route's boundary observation.
type kernelCaseExpect struct {
	ID       string
	Category string
	Boundary bool
}

// kernelRouteExpect pins one executable kernel route and its imported cases.
type kernelRouteExpect struct {
	Family  string
	Version string
	Cases   []kernelCaseExpect
}

// kernelClosedRoutes is the eleven-route closure table. Routes land one
// family at a time; a route with no imported cases yet is absent here until
// its import lands.
var kernelClosedRoutes = []kernelRouteExpect{
	{
		Family:  "kernel.mornlea_collision_resolve",
		Version: "11",
		Cases: []kernelCaseExpect{
			{ID: "kernel.mornlea_collision_resolve/11/floor-wall", Category: "ok"},
			{ID: "kernel.mornlea_collision_resolve/11/invalid-used-nan", Category: "input"},
			{ID: "kernel.mornlea_collision_resolve/11/short-output-15", Category: "output-overflow", Boundary: true},
		},
	},
}

// kernelArguments is the frozen binary-case argument vocabulary the Go
// producer and the Rust consumer validate identically.
type kernelArguments struct {
	ABIVersion     int    `json:"abi_version"`
	OutputCapacity int    `json:"output_capacity"`
	BufferVariant  string `json:"buffer_variant"`
}

func parseKernelArguments(t *testing.T, id string, raw json.RawMessage) kernelArguments {
	t.Helper()
	var args kernelArguments
	dec := json.NewDecoder(bytes.NewReader(raw))
	dec.UseNumber()
	if err := dec.Decode(&args); err != nil {
		t.Fatalf("case %s arguments decode: %v", id, err)
	}
	if args.ABIVersion != int(nativeabi.ABIVersion) {
		t.Fatalf("case %s abi_version=%d cannot execute under bridge ABI %d", id, args.ABIVersion, nativeabi.ABIVersion)
	}
	if args.OutputCapacity <= 0 || int64(args.OutputCapacity) > MaxBinaryBytes {
		t.Fatalf("case %s output_capacity=%d outside the binary budget", id, args.OutputCapacity)
	}
	if args.BufferVariant != "normal" && args.BufferVariant != "short" {
		t.Fatalf("case %s buffer_variant=%q is not a supported corpus variant", id, args.BufferVariant)
	}
	return args
}

// kernelPanicStatus maps a recovered bridge panic back to the ABI status the
// raw call returned. The bridge panic texts are stable contract pinned by the
// nativeabi panic-text tests; any other panic value fails the case.
func kernelPanicStatus(recovered any) (uint32, error) {
	text, ok := recovered.(string)
	if !ok {
		return 0, fmt.Errorf("non-string bridge panic %v", recovered)
	}
	switch {
	case strings.Contains(text, "ABI 版本不匹配"):
		return 1, nil
	case strings.Contains(text, "参数非法"):
		return 2, nil
	case strings.Contains(text, "输入非法"):
		return 3, nil
	case strings.Contains(text, "output 过短"):
		return 7, nil
	case strings.Contains(text, "Rust panic"):
		return 9, nil
	default:
		return 0, fmt.Errorf("unmapped bridge panic %q", text)
	}
}

// kernelDigest renders the corpus digest form of raw bytes.
func kernelDigest(data []byte) string {
	sum := sha256.Sum256(data)
	return "sha256:" + hex.EncodeToString(sum[:])
}

// kernelBits renders ordered 32-bit words as eight-digit lowercase hex in
// numeric order, matching the corpus bit-string convention.
func kernelBits(data []byte) []string {
	bits := make([]string, 0, len(data)/4)
	for offset := 0; offset+4 <= len(data); offset += 4 {
		bits = append(bits, fmt.Sprintf("%08x", binary.LittleEndian.Uint32(data[offset:offset+4])))
	}
	return bits
}

// executeKernelCollisionCase runs one binary collision case through the
// public bridge and returns the normalized outcome the frozen expectation
// carries.
func executeKernelCollisionCase(input []byte, args kernelArguments) map[string]any {
	output := make([]byte, args.OutputCapacity)
	for index := range output {
		output[index] = 0xa5
	}
	status := uint32(0)
	func() {
		defer func() {
			if recovered := recover(); recovered != nil {
				mapped, err := kernelPanicStatus(recovered)
				if err != nil {
					panic(err)
				}
				status = mapped
			}
		}()
		nativeabi.CollisionResolve(input, output)
	}()
	fields := map[string]any{
		"status":         int(status),
		"output_len":     len(output),
		"payload_sha256": kernelDigest(output),
	}
	kind := "error"
	if status == 0 {
		kind = "ok"
		fields["used_payload_sha256"] = kernelDigest(output)
		fields["f32_bits"] = kernelBits(output[:12])
	}
	return map[string]any{
		"kind":     kind,
		"category": kernelStatusNameForTest(status),
		"fields":   fields,
	}
}

// kernelStatusNameForTest mirrors the raw oracle's status vocabulary for the
// statuses the public bridge can surface.
func kernelStatusNameForTest(status uint32) string {
	return map[uint32]string{
		0: "ok",
		1: "abi-version",
		2: "invalid-argument",
		3: "input",
		7: "output-overflow",
		9: "panic",
	}[status]
}

func findKernelCase(t *testing.T, cases []CaseSpec, id string) CaseSpec {
	t.Helper()
	for _, c := range cases {
		if c.ID == id {
			return c
		}
	}
	t.Fatalf("case %s is not in the frozen manifest", id)
	return CaseSpec{}
}

// TestKernelOracle executes every closed-route kernel case and pins the
// closure table against the frozen manifest.
func TestKernelOracle(t *testing.T) {
	root := mustRepoRoot(t)
	inventory, err := LoadInventory(filepath.Join(root, filepath.FromSlash(InventoryRelPath)))
	if err != nil {
		t.Fatalf("load frozen inventory: %v", err)
	}
	families := make(map[string]Family, len(inventory.Families))
	for _, family := range inventory.Families {
		families[family.ID] = family
	}
	cases := make(map[string]CaseSpec, len(inventory.Cases))
	for _, c := range inventory.Cases {
		cases[c.ID] = c
	}

	consumers := BaselineConsumerRegistry()
	engine, ok := consumers["mornlea_engine"]
	if !ok {
		t.Fatal("BaselineConsumerRegistry has no mornlea_engine consumer")
	}
	if engine.Kind != ConsumerRust {
		t.Fatalf("mornlea_engine consumer kind=%d, want Rust", engine.Kind)
	}

	var tallies []string
	for _, route := range kernelClosedRoutes {
		wantRoute := ConsumerRoute{FamilyID: route.Family, Version: route.Version, Operation: "kernel"}
		if _, ok := engine.Routes[wantRoute]; !ok {
			t.Fatalf("mornlea_engine registry is missing route %s/%s/kernel", route.Family, route.Version)
		}
		family, ok := families[route.Family]
		if !ok {
			t.Fatalf("manifest has no family %s", route.Family)
		}
		if len(family.Cases) != len(route.Cases) {
			t.Fatalf("family %s carries %d cases, want %d", route.Family, len(family.Cases), len(route.Cases))
		}
		seen := make(map[string]bool, len(route.Cases))
		var okCount, errorCount int
		var boundary bool
		for _, want := range route.Cases {
			spec, ok := cases[want.ID]
			if !ok {
				t.Fatalf("case %s is not in the frozen manifest", want.ID)
			}
			if seen[want.ID] {
				t.Fatalf("duplicate case %s", want.ID)
			}
			seen[want.ID] = true
			if spec.Family != route.Family || spec.Version != route.Version || spec.Operation != "kernel" {
				t.Fatalf("case %s route mismatch: %+v", want.ID, spec)
			}
			if spec.RustConsumer != "mornlea_engine" {
				t.Fatalf("case %s rust_consumer=%s, want mornlea_engine", want.ID, spec.RustConsumer)
			}
			if spec.InputFormat != "binary" {
				t.Fatalf("case %s input_format=%s, want binary", want.ID, spec.InputFormat)
			}
			if len(spec.Checkpoints) != 1 || spec.Checkpoints[0] != "0" {
				t.Fatalf("case %s checkpoints=%v, want [\"0\"]", want.ID, spec.Checkpoints)
			}
			inputPath := filepath.Join(root, filepath.FromSlash(spec.Input.Path))
			input, err := os.ReadFile(inputPath)
			if err != nil {
				t.Fatalf("read input %s: %v", spec.Input.Path, err)
			}
			if int64(len(input)) > MaxBinaryBytes {
				t.Fatalf("case %s input exceeds the binary budget", want.ID)
			}
			if kernelDigest(input) != spec.Input.SHA256 {
				t.Fatalf("case %s input digest drifted from the manifest", want.ID)
			}
			expectedPath := filepath.Join(root, filepath.FromSlash(spec.Expected.Path))
			expectedRaw, err := os.ReadFile(expectedPath)
			if err != nil {
				t.Fatalf("read expected %s: %v", spec.Expected.Path, err)
			}
			if kernelDigest(expectedRaw) != spec.Expected.SHA256 {
				t.Fatalf("case %s expected digest drifted from the manifest", want.ID)
			}
			var expected struct {
				Kind     string `json:"kind"`
				Category string `json:"category"`
			}
			if err := json.Unmarshal(expectedRaw, &expected); err != nil {
				t.Fatalf("decode expected %s: %v", want.ID, err)
			}
			if expected.Category != want.Category {
				t.Fatalf("case %s category=%s, want %s", want.ID, expected.Category, want.Category)
			}
			args := parseKernelArguments(t, want.ID, spec.Arguments)
			var actual map[string]any
			switch route.Family {
			case "kernel.mornlea_collision_resolve":
				actual = executeKernelCollisionCase(input, args)
			default:
				t.Fatalf("no Go producer for family %s", route.Family)
			}
			rendered, err := json.MarshalIndent(actual, "", "  ")
			if err != nil {
				t.Fatalf("render observation %s: %v", want.ID, err)
			}
			rendered = append(rendered, '\n')
			if !bytes.Equal(rendered, expectedRaw) {
				t.Fatalf("case %s observation mismatch:\nactual %s\nexpected %s", want.ID, rendered, expectedRaw)
			}
			if expected.Kind == "ok" {
				okCount++
			} else {
				errorCount++
			}
			if want.Boundary {
				boundary = true
			}
			tallies = append(tallies, want.ID+"="+expected.Category)
		}
		if okCount == 0 || errorCount == 0 {
			t.Fatalf("route %s executed ok=%d error=%d, want at least one of each", route.Family, okCount, errorCount)
		}
		if !boundary {
			t.Fatalf("route %s has no executed boundary case", route.Family)
		}
		t.Logf("route %s/%s executed: %s", route.Family, route.Version, strings.Join(tallies, " "))
		tallies = tallies[:0]
	}

	report, err := ReconcileWorking(root, inventory, mustDiscover(t, root), mustLiveIdentities(t, root), consumers, BaselineNegativeCoverageExceptions())
	if err != nil {
		t.Fatalf("ReconcileWorking: %v", err)
	}
	found := false
	for _, pt := range report.Covered {
		if pt.FamilyID == "kernel.mornlea_collision_resolve" && pt.Version == "11" {
			found = true
		}
	}
	if !found {
		t.Fatal("kernel.mornlea_collision_resolve/11 is not covered after import")
	}
	if _, err := ReconcileComplete(root, inventory, mustDiscover(t, root), mustLiveIdentities(t, root), consumers, BaselineNegativeCoverageExceptions()); err == nil {
		t.Fatal("ReconcileComplete accepted with ten kernel routes still empty")
	} else if !strings.Contains(err.Error(), "kernel.mornlea_physics_step") {
		t.Fatalf("ReconcileComplete rejected for the wrong reason: %v", err)
	}
}

func mustDiscover(t *testing.T, root string) []Family {
	t.Helper()
	families, _, err := Discover(root)
	if err != nil {
		t.Fatalf("discover registries: %v", err)
	}
	return families
}

func mustLiveIdentities(t *testing.T, root string) Identities {
	t.Helper()
	_, live, err := Discover(root)
	if err != nil {
		t.Fatalf("discover identities: %v", err)
	}
	return live
}
