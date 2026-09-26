//go:build cgo && (darwin || linux)

package nativeabi

import (
	"crypto/sha256"
	"encoding/binary"
	"encoding/hex"
	"encoding/json"
	"fmt"
	"math"
	"os"
	"path/filepath"
	"runtime"
	"strings"
	"testing"
)

// TestKernelABIRawOracle is the package-local raw producer for the native
// numerical corpus. The public bridge panics on every non-OK status, so the
// malformed and short-capacity observations the corpus must freeze are only
// reachable through the package-private version helpers used here. Each case
// runs the real exported symbol against a canary-filled caller arena, asserts
// the status and the untouched-canary rule locally, and freezes the normalized
// observation. With RUNTIME_ORACLE_EXPORT_DIR set, the same observations are
// published create-exclusively under the nativeabi-kernel child as reviewed
// drafts; an unset variable exports nothing and writes no tracked file.

const kernelOracleExportChild = "nativeabi-kernel"

// kernelDraftAsset pairs a producer-relative path with its exact bytes.
type kernelDraftAsset struct {
	RelativePath string
	Data         []byte
}

// kernelDraftSelection is the manifest fragment one export publishes: the
// case specifications the controller merges by exact case ID.
type kernelDraftSelection struct {
	ID           string          `json:"id"`
	Family       string          `json:"family"`
	Version      string          `json:"version"`
	Operation    string          `json:"operation"`
	Arguments    json.RawMessage `json:"arguments"`
	Input        kernelDraftRef  `json:"input"`
	InputFormat  string          `json:"input_format"`
	Expected     kernelDraftRef  `json:"expected"`
	Checkpoints  []string        `json:"checkpoints"`
	RustConsumer string          `json:"rust_consumer"`
	Category     string          `json:"category"`
}

// kernelDraftRef carries a repository-relative asset path and its digest.
type kernelDraftRef struct {
	Path   string `json:"path"`
	SHA256 string `json:"sha256"`
}

// kernelStatusName normalizes an engine ABI status code to the frozen corpus
// category vocabulary shared with the Rust consumer.
func kernelStatusName(status Status) string {
	switch status {
	case StatusOK:
		return "ok"
	case StatusABIVersion:
		return "abi-version"
	case StatusInvalidArgument:
		return "invalid-argument"
	case StatusInput:
		return "input"
	case StatusScratch:
		return "scratch"
	case StatusRegistry:
		return "registry"
	case StatusEmission:
		return "emission"
	case StatusOutputOverflow:
		return "output-overflow"
	case StatusQueueOverflow:
		return "queue-overflow"
	case StatusPanic:
		return "panic"
	default:
		return "unknown"
	}
}

// kernelDigest renders the corpus digest form of raw bytes.
func kernelDigest(data []byte) string {
	sum := sha256.Sum256(data)
	return "sha256:" + hex.EncodeToString(sum[:])
}

// kernelF32Bits renders ordered 32-bit words as eight-digit lowercase hex
// strings in numeric order, matching the corpus bit-string convention for
// look angles. The numeric order carries negative zero and NaN payloads
// through JSON unchanged.
func kernelF32Bits(data []byte) []string {
	bits := make([]string, 0, len(data)/4)
	for offset := 0; offset+4 <= len(data); offset += 4 {
		bits = append(bits, fmt.Sprintf("%08x", binary.LittleEndian.Uint32(data[offset:offset+4])))
	}
	return bits
}

// marshalKernelExpected renders one normalized outcome the way committed
// corpus expectations render it: recursively sorted keys, two-space
// indentation and one terminal newline.
func marshalKernelExpected(kind, category string, fields map[string]any) []byte {
	raw, err := json.MarshalIndent(map[string]any{
		"kind":     kind,
		"category": category,
		"fields":   fields,
	}, "", "  ")
	if err != nil {
		panic(err)
	}
	return append(raw, '\n')
}

// kernelOracleRepoRoot walks from this file to the go.work checkout root.
func kernelOracleRepoRoot(t *testing.T) string {
	t.Helper()
	_, file, _, ok := runtime.Caller(0)
	if !ok {
		t.Fatal("locate kernel oracle test file")
	}
	dir := filepath.Dir(file)
	for {
		if _, err := os.Stat(filepath.Join(dir, "go.work")); err == nil {
			return dir
		}
		parent := filepath.Dir(dir)
		if parent == dir {
			t.Fatal("go.work not found above nativeabi test")
		}
		dir = parent
	}
}

// exportKernelDrafts publishes one producer child's assets create-exclusively
// below the harness-owned export directory. It resolves the export root
// through its nearest existing ancestor, rejects repository containment and
// any symlink below that ancestor, and validates every asset path, duplicate
// and file-as-parent collision before mutation. A preexisting final child is
// rejected, so reruns never silently replace evidence. An unset
// RUNTIME_ORACLE_EXPORT_DIR exports nothing.
func exportKernelDrafts(t *testing.T, child string, assets []kernelDraftAsset) string {
	t.Helper()
	exportDir := strings.TrimSpace(os.Getenv("RUNTIME_ORACLE_EXPORT_DIR"))
	if exportDir == "" {
		return ""
	}
	if strings.TrimSpace(child) == "" || strings.Contains(child, "\\") || filepath.IsAbs(child) {
		t.Fatalf("producer child must be a clean relative slash path: %q", child)
	}
	for _, part := range strings.Split(child, "/") {
		if part == "" || part == "." || part == ".." {
			t.Fatalf("producer child must be a clean relative slash path: %q", child)
		}
	}
	abs, err := filepath.Abs(exportDir)
	if err != nil {
		t.Fatalf("resolve export root: %v", err)
	}
	ancestor := abs
	for {
		if _, err := os.Lstat(ancestor); err == nil {
			break
		}
		parent := filepath.Dir(ancestor)
		if parent == ancestor {
			t.Fatalf("export root has no existing ancestor: %s", abs)
		}
		ancestor = parent
	}
	resolvedAncestor, err := filepath.EvalSymlinks(ancestor)
	if err != nil {
		t.Fatalf("resolve export ancestor: %v", err)
	}
	tail, err := filepath.Rel(ancestor, abs)
	if err != nil {
		t.Fatalf("relativize export root: %v", err)
	}
	current := resolvedAncestor
	if tail != "." {
		for _, part := range strings.Split(tail, string(filepath.Separator)) {
			if part == "" || part == "." || part == ".." {
				t.Fatalf("export root escapes its ancestor: %s", abs)
			}
			current = filepath.Join(current, part)
			if info, err := os.Lstat(current); err == nil {
				if info.Mode()&os.ModeSymlink != 0 {
					t.Fatalf("export path component is a symlink: %s", current)
				}
				if !info.IsDir() {
					t.Fatalf("export path component is not a directory: %s", current)
				}
			} else if err := os.Mkdir(current, 0o755); err != nil {
				t.Fatalf("create export component: %v", err)
			}
		}
	}
	repo := kernelOracleRepoRoot(t)
	resolvedRepo, err := filepath.EvalSymlinks(repo)
	if err != nil {
		t.Fatalf("resolve repository root: %v", err)
	}
	resolvedExport := current
	if resolvedExport == resolvedRepo || strings.HasPrefix(resolvedExport, resolvedRepo+string(filepath.Separator)) {
		t.Fatalf("export root is inside the repository: %s", resolvedExport)
	}
	seen := make(map[string]struct{}, len(assets))
	for _, asset := range assets {
		if strings.TrimSpace(asset.RelativePath) == "" || strings.Contains(asset.RelativePath, "\\") || filepath.IsAbs(asset.RelativePath) {
			t.Fatalf("asset path must be a clean relative slash path: %q", asset.RelativePath)
		}
		parts := strings.Split(asset.RelativePath, "/")
		for _, part := range parts {
			if part == "" || part == "." || part == ".." {
				t.Fatalf("asset path must be a clean relative slash path: %q", asset.RelativePath)
			}
		}
		if filepath.ToSlash(filepath.Clean(filepath.FromSlash(asset.RelativePath))) != asset.RelativePath {
			t.Fatalf("asset path must be a clean relative slash path: %q", asset.RelativePath)
		}
		if _, dup := seen[asset.RelativePath]; dup {
			t.Fatalf("duplicate asset path: %s", asset.RelativePath)
		}
		seen[asset.RelativePath] = struct{}{}
	}
	childDir := filepath.Join(resolvedExport, filepath.FromSlash(child))
	if err := os.Mkdir(childDir, 0o755); err != nil {
		t.Fatalf("create exclusive producer child %s: %v", childDir, err)
	}
	for _, asset := range assets {
		target := filepath.Join(childDir, filepath.FromSlash(asset.RelativePath))
		parent := filepath.Dir(target)
		relParent, err := filepath.Rel(childDir, parent)
		if err != nil {
			t.Fatalf("relativize asset parent: %v", err)
		}
		if relParent != "." {
			current := childDir
			for _, part := range strings.Split(relParent, string(filepath.Separator)) {
				if part == "" || part == "." || part == ".." {
					t.Fatalf("asset path escapes producer child: %s", asset.RelativePath)
				}
				current = filepath.Join(current, part)
				if info, err := os.Lstat(current); err == nil {
					if info.Mode()&os.ModeSymlink != 0 {
						t.Fatalf("asset parent is a symlink: %s", current)
					}
					if !info.IsDir() {
						t.Fatalf("asset parent is not a directory: %s", current)
					}
				} else if err := os.Mkdir(current, 0o755); err != nil {
					t.Fatalf("create asset parent: %v", err)
				}
			}
		}
		file, err := os.OpenFile(target, os.O_WRONLY|os.O_CREATE|os.O_EXCL, 0o644)
		if err != nil {
			t.Fatalf("create exclusive asset %s: %v", target, err)
		}
		if _, err := file.Write(asset.Data); err != nil {
			file.Close()
			t.Fatalf("write asset %s: %v", target, err)
		}
		if err := file.Close(); err != nil {
			t.Fatalf("close asset %s: %v", target, err)
		}
	}
	return childDir
}

// kernelRawCollisionWall builds the loaded floor-plus-wall scene the accepted
// migration vector pins: position (0.5, 1.0, 0.5), displacement (0.5, 0, 0),
// grounded, step height 0.6, origin (0, 0, 0), dimensions (2, 4, 1), full
// cubes at (0, 0, 0) and (1, 1, 0).
func kernelRawCollisionWall() []byte {
	const headerBytes = 64
	const cellBytes = 196
	input := make([]byte, headerBytes+8*cellBytes)
	copy(input[0:4], "MGC1")
	binary.LittleEndian.PutUint32(input[4:8], 1)
	for offset, value := range map[int]float32{
		8: 0.5, 12: 1.0, 16: 0.5, 20: 0.5, 24: 0.0, 28: 0.0, 36: 0.6,
	} {
		binary.LittleEndian.PutUint32(input[offset:offset+4], math.Float32bits(value))
	}
	input[32] = 1
	binary.LittleEndian.PutUint32(input[52:56], 2)
	binary.LittleEndian.PutUint32(input[56:60], 4)
	binary.LittleEndian.PutUint32(input[60:64], 1)
	for offset := headerBytes; offset < len(input); offset += cellBytes {
		input[offset] = 1
	}
	setCube := func(position [3]int) {
		offset := headerBytes + ((position[1]*2+position[0])*1+position[2])*cellBytes
		input[offset] = 1
		input[offset+1] = 1
		for index, value := range [...]float32{0, 0, 0, 1, 1, 1} {
			binary.LittleEndian.PutUint32(input[offset+4+index*4:offset+8+index*4], math.Float32bits(value))
		}
	}
	setCube([3]int{0, 0, 0})
	setCube([3]int{1, 1, 0})
	return input
}

// kernelPinnedCollisionWallOutput is the accepted migration observation for
// the wall scene: position (0.7, 1.0, 0.5) with X clipping and grounded set.
var kernelPinnedCollisionWallOutput = []byte{
	0x33, 0x33, 0x33, 0x3f, 0x00, 0x00, 0x80, 0x3f,
	0x00, 0x00, 0x00, 0x3f, 0x01, 0x01, 0x00, 0x00,
}

// finishKernelBinaryCase builds the draft asset pair and the manifest
// selection entry for one executed binary observation. The caller supplies
// family, label, executed status and arena plus any success-only normalized
// fields; kind and category derive from the status.
func finishKernelBinaryCase(
	t *testing.T,
	family, label string,
	input []byte,
	outputCapacity int,
	bufferVariant string,
	status Status,
	output []byte,
	extraFields map[string]any,
	assetDir string,
) (kernelDraftAsset, kernelDraftAsset, kernelDraftSelection) {
	t.Helper()
	id := family + "/11/" + label
	fields := map[string]any{
		"status":         int(status),
		"output_len":     outputCapacity,
		"payload_sha256": kernelDigest(output),
	}
	for key, value := range extraFields {
		fields[key] = value
	}
	kind := "error"
	if status == StatusOK {
		kind = "ok"
	}
	arguments, err := json.Marshal(map[string]any{
		"abi_version":     ABIVersion,
		"output_capacity": outputCapacity,
		"buffer_variant":  bufferVariant,
	})
	if err != nil {
		t.Fatalf("marshal arguments: %v", err)
	}
	expected := marshalKernelExpected(kind, kernelStatusName(status), fields)
	inputRel := "cases/kernel/" + assetDir + "/" + label + ".input.bin"
	expectedRel := "cases/kernel/" + assetDir + "/" + label + ".expected.json"
	return kernelDraftAsset{
			RelativePath: strings.ReplaceAll(id, "/", "-") + "/input.bin",
			Data:         append([]byte(nil), input...),
		}, kernelDraftAsset{
			RelativePath: strings.ReplaceAll(id, "/", "-") + "/expected.json",
			Data:         expected,
		}, kernelDraftSelection{
			ID:           id,
			Family:       family,
			Version:      "11",
			Operation:    "kernel",
			Arguments:    arguments,
			Input:        kernelDraftRef{Path: "testdata/runtime-migration/" + inputRel, SHA256: kernelDigest(input)},
			InputFormat:  "binary",
			Expected:     kernelDraftRef{Path: "testdata/runtime-migration/" + expectedRel, SHA256: kernelDigest(expected)},
			Checkpoints:  []string{"0"},
			RustConsumer: "mornlea_engine",
			Category:     kernelStatusName(status),
		}
}

// kernelCollisionCases executes the three collision corpus observations
// against the real exported symbol and returns the draft assets plus the
// manifest selection fragment.
func kernelCollisionCases(t *testing.T) ([]kernelDraftAsset, []kernelDraftSelection) {
	t.Helper()
	wall := kernelRawCollisionWall()

	type observation struct {
		label          string
		input          []byte
		outputCapacity int
		bufferVariant  string
		status         Status
		output         []byte
	}
	run := func(input []byte, outputCapacity int) (Status, []byte) {
		output := make([]byte, outputCapacity)
		for index := range output {
			output[index] = 0xa5
		}
		status := collisionResolveVersion(ABIVersion, input, output)
		return status, output
	}
	nan := append([]byte(nil), wall...)
	nan[65] = 1
	binary.LittleEndian.PutUint32(nan[68:72], math.Float32bits(float32(math.NaN())))

	observations := []observation{
		{label: "floor-wall", input: wall, outputCapacity: 16, bufferVariant: "normal"},
		{label: "invalid-used-nan", input: nan, outputCapacity: 16, bufferVariant: "normal"},
		{label: "short-output-15", input: wall, outputCapacity: 15, bufferVariant: "short"},
	}
	for index := range observations {
		status, output := run(observations[index].input, observations[index].outputCapacity)
		observations[index].status = status
		observations[index].output = output
	}

	if observations[0].status != StatusOK {
		t.Fatalf("floor-wall status=%d, want OK", observations[0].status)
	}
	for index, value := range kernelPinnedCollisionWallOutput {
		if observations[0].output[index] != value {
			t.Fatalf("floor-wall output[%d]=%#x, want %#x", index, observations[0].output[index], value)
		}
	}
	if observations[1].status != StatusInput {
		t.Fatalf("invalid-used-nan status=%d, want input", observations[1].status)
	}
	for index, value := range observations[1].output {
		if value != 0xa5 {
			t.Fatalf("invalid-used-nan output[%d]=%#x modified on failure", index, value)
		}
	}
	if observations[2].status != StatusOutputOverflow {
		t.Fatalf("short-output-15 status=%d, want output-overflow", observations[2].status)
	}
	for index, value := range observations[2].output {
		if value != 0xa5 {
			t.Fatalf("short-output-15 output[%d]=%#x modified on failure", index, value)
		}
	}

	var assets []kernelDraftAsset
	var selections []kernelDraftSelection
	for _, obs := range observations {
		id := "kernel.mornlea_collision_resolve/11/" + obs.label
		child := strings.ReplaceAll(id, "/", "-")
		fields := map[string]any{
			"status":         int(obs.status),
			"output_len":     obs.outputCapacity,
			"payload_sha256": kernelDigest(obs.output),
		}
		kind := "error"
		if obs.status == StatusOK {
			kind = "ok"
			fields["used_payload_sha256"] = kernelDigest(obs.output)
			fields["f32_bits"] = kernelF32Bits(obs.output[:12])
		}
		arguments, err := json.Marshal(map[string]any{
			"abi_version":     ABIVersion,
			"output_capacity": obs.outputCapacity,
			"buffer_variant":  obs.bufferVariant,
		})
		if err != nil {
			t.Fatalf("marshal arguments: %v", err)
		}
		expected := marshalKernelExpected(kind, kernelStatusName(obs.status), fields)
		inputRel := "cases/kernel/collision/" + obs.label + ".input.bin"
		expectedRel := "cases/kernel/collision/" + obs.label + ".expected.json"
		assets = append(assets,
			kernelDraftAsset{RelativePath: child + "/input.bin", Data: append([]byte(nil), obs.input...)},
			kernelDraftAsset{RelativePath: child + "/expected.json", Data: expected},
		)
		selections = append(selections, kernelDraftSelection{
			ID:           id,
			Family:       "kernel.mornlea_collision_resolve",
			Version:      "11",
			Operation:    "kernel",
			Arguments:    arguments,
			Input:        kernelDraftRef{Path: "testdata/runtime-migration/" + inputRel, SHA256: kernelDigest(obs.input)},
			InputFormat:  "binary",
			Expected:     kernelDraftRef{Path: "testdata/runtime-migration/" + expectedRel, SHA256: kernelDigest(expected)},
			Checkpoints:  []string{"0"},
			RustConsumer: "mornlea_engine",
			Category:     kernelStatusName(obs.status),
		})
	}
	return assets, selections
}

// kernelRawPhysicsStep builds a 160-byte header plus four loaded-but-empty
// cells in a (1, 4, 1) prism, mirroring the accepted migration encoder
// structurally. The caller arms individual cells as full cubes.
func kernelRawPhysicsStep(
	position, velocity [3]float32,
	onGround, jump bool,
	moveX, moveZ int8,
	bodyInFluid, sprinting, sneaking bool,
	sweepMin, sweepMax [3]float32,
) []byte {
	const headerBytes = 160
	const cellBytes = 196
	dimensions := [3]uint32{1, 4, 1}
	input := make([]byte, headerBytes+4*cellBytes)
	copy(input[0:4], "MGP1")
	binary.LittleEndian.PutUint32(input[4:8], 4)
	putF32 := func(offset int, value float32) {
		binary.LittleEndian.PutUint32(input[offset:offset+4], math.Float32bits(value))
	}
	for index, value := range position {
		putF32(8+index*4, value)
	}
	for index, value := range velocity {
		putF32(20+index*4, value)
	}
	if onGround {
		input[32] = 1
	}
	if jump {
		input[33] = 1
	}
	input[34] = byte(moveX)
	input[35] = byte(moveZ)
	putF32(36, 0)
	putF32(40, 1)
	putF32(44, 0.05)
	for index, value := range [...]float32{0.6, 4.3, 40, 50, 8, 8.4, 32, 78.4} {
		putF32(48+index*4, value)
	}
	for axis := 0; axis < 3; axis++ {
		putF32(80+axis*8, sweepMin[axis])
		putF32(84+axis*8, sweepMax[axis])
		binary.LittleEndian.PutUint32(input[104+axis*4:108+axis*4], 0)
		binary.LittleEndian.PutUint32(input[116+axis*4:120+axis*4], dimensions[axis])
	}
	if bodyInFluid {
		input[128] = 1
	}
	if sprinting {
		input[129] = 1
	}
	if sneaking {
		input[130] = 1
	}
	for index, value := range [...]float32{6.4, 3, 4, 0.8} {
		putF32(132+index*4, value)
	}
	putF32(148, 1.3)
	putF32(152, 0.3)
	for offset := headerBytes; offset < len(input); offset += cellBytes {
		input[offset] = 1
	}
	return input
}

// kernelArmPhysicsFloorCube arms cell (0, 0, 0) as a full cube in a step
// input built by kernelRawPhysicsStep.
func kernelArmPhysicsFloorCube(input []byte) {
	const headerBytes = 160
	input[headerBytes] = 1
	input[headerBytes+1] = 1
	for index, value := range [...]float32{0, 0, 0, 1, 1, 1} {
		binary.LittleEndian.PutUint32(input[headerBytes+4+index*4:headerBytes+8+index*4], math.Float32bits(value))
	}
}

// kernelNextDown32 returns the largest float32 below the finite nonzero value,
// matching the migration sweep construction arithmetically.
func kernelNextDown32(value float32) float32 {
	bits := math.Float32bits(value)
	if value > 0 {
		bits--
	} else {
		bits++
	}
	return math.Float32frombits(bits)
}

// kernelPinnedPhysicsLandingOutput is the accepted migration observation for
// the floor landing scene: back at y = 1.0 with Y velocity clipped and the
// grounded flag set.
var kernelPinnedPhysicsLandingOutput = []byte{
	0x00, 0x00, 0x00, 0x3f, 0x00, 0x00, 0x80, 0x3f,
	0x00, 0x00, 0x00, 0x3f, 0x00, 0x00, 0x00, 0x00,
	0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
	0x02, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
}

// kernelPinnedPhysicsUlpAcceptOutput is the accepted migration observation
// for the one-ULP sweep scene: free fall to y = 0.92 with velocity -1.6.
var kernelPinnedPhysicsUlpAcceptOutput = []byte{
	0x00, 0x00, 0x00, 0x3f, 0x1f, 0x85, 0x6b, 0x3f,
	0x00, 0x00, 0x00, 0x3f, 0x00, 0x00, 0x00, 0x00,
	0xcd, 0xcc, 0xcc, 0xbf, 0x00, 0x00, 0x00, 0x00,
	0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
}

// kernelPhysicsCases executes the four physics corpus observations against
// the real exported symbol and returns the draft assets plus the manifest
// selection fragment.
func kernelPhysicsCases(t *testing.T) ([]kernelDraftAsset, []kernelDraftSelection) {
	t.Helper()
	landing := kernelRawPhysicsStep(
		[3]float32{0.5, 1, 0.5}, [3]float32{0, 0, 0},
		false, false, 0, 0, false, false, false,
		[3]float32{0, -0.08, 0}, [3]float32{0, 0.05, 0},
	)
	kernelArmPhysicsFloorCube(landing)
	axis := append([]byte(nil), landing...)
	axis[34] = 2
	displacement := float32(-1.6) * float32(0.05)
	ulpAccept := kernelRawPhysicsStep(
		[3]float32{0.5, 1, 0.5}, [3]float32{0, 0, 0},
		false, false, 0, 0, false, false, false,
		[3]float32{0, kernelNextDown32(displacement), 0},
		[3]float32{0, kernelNextDown32(displacement), 0},
	)
	ulpReject := kernelRawPhysicsStep(
		[3]float32{0.5, 1, 0.5}, [3]float32{0, 0, 0},
		false, false, 0, 0, false, false, false,
		[3]float32{0, kernelNextDown32(displacement), 0},
		[3]float32{0, kernelNextDown32(kernelNextDown32(displacement)), 0},
	)

	run := func(input []byte) (Status, []byte) {
		output := make([]byte, 32)
		for index := range output {
			output[index] = 0xa5
		}
		return physicsStepVersion(ABIVersion, input, output), output
	}
	type observation struct {
		label         string
		input         []byte
		want          Status
		pinned        []byte
		bufferVariant string
	}
	observations := []observation{
		{label: "floor-landing", input: landing, want: StatusOK, pinned: kernelPinnedPhysicsLandingOutput, bufferVariant: "normal"},
		{label: "axis-two", input: axis, want: StatusInput, bufferVariant: "normal"},
		{label: "ulp-sweep-accept", input: ulpAccept, want: StatusOK, pinned: kernelPinnedPhysicsUlpAcceptOutput, bufferVariant: "normal"},
		{label: "ulp-sweep-reject", input: ulpReject, want: StatusInput, bufferVariant: "normal"},
	}
	var assets []kernelDraftAsset
	var selections []kernelDraftSelection
	for _, obs := range observations {
		status, output := run(obs.input)
		if status != obs.want {
			t.Fatalf("%s status=%d, want %d", obs.label, status, obs.want)
		}
		extra := map[string]any{}
		if status == StatusOK {
			for index, value := range obs.pinned {
				if output[index] != value {
					t.Fatalf("%s output[%d]=%#x, want %#x", obs.label, index, output[index], value)
				}
			}
			extra["used_payload_sha256"] = kernelDigest(output)
			extra["f32_bits"] = kernelF32Bits(output[:24])
		} else {
			for index, value := range output {
				if value != 0xa5 {
					t.Fatalf("%s output[%d]=%#x modified on failure", obs.label, index, value)
				}
			}
		}
		inputAsset, expectedAsset, selection := finishKernelBinaryCase(
			t, "kernel.mornlea_physics_step", obs.label, obs.input, 32, obs.bufferVariant,
			status, output, extra, "physics",
		)
		assets = append(assets, inputAsset, expectedAsset)
		selections = append(selections, selection)
	}
	return assets, selections
}

// kernelRawRaycastInput builds a 40-byte batch request for an axis-aligned
// ray: origin, direction and maximum travel distance with a zeroed tail.
func kernelRawRaycastInput(origin, direction [3]float32, maximum float32) []byte {
	input := make([]byte, 40)
	copy(input[0:4], "MGR1")
	binary.LittleEndian.PutUint32(input[4:8], 1)
	for index, value := range origin {
		binary.LittleEndian.PutUint32(input[8+index*4:12+index*4], math.Float32bits(value))
	}
	for index, value := range direction {
		binary.LittleEndian.PutUint32(input[20+index*4:24+index*4], math.Float32bits(value))
	}
	binary.LittleEndian.PutUint32(input[32:36], math.Float32bits(maximum))
	return input
}

// kernelFreshRaycastCursor builds the canonical zeroed batch cursor.
func kernelFreshRaycastCursor() []byte {
	cursor := make([]byte, 64)
	copy(cursor[0:4], "MRC1")
	binary.LittleEndian.PutUint32(cursor[4:8], 1)
	return cursor
}

// kernelRaycastBatchJSON renders one executed batch call for the normalized
// observation: record count, done flag, ordered full-record hex, ordered
// distance bits in numeric order and the post-call cursor digest.
func kernelRaycastBatchJSON(count int, done bool, output []byte, cursor []byte) map[string]any {
	records := make([]string, 0, count)
	distances := make([]string, 0, count)
	for index := 0; index < count; index++ {
		record := output[index*20 : (index+1)*20]
		records = append(records, hex.EncodeToString(record))
		distances = append(distances, fmt.Sprintf("%08x", binary.LittleEndian.Uint32(record[16:20])))
	}
	doneFlag := 0
	if done {
		doneFlag = 1
	}
	return map[string]any{
		"count":         count,
		"done":          doneFlag,
		"records":       records,
		"distances":     distances,
		"cursor_sha256": kernelDigest(cursor),
	}
}

// kernelRaycastCases executes the three raycast corpus observations against
// the real exported symbol and returns the draft assets plus the manifest
// selection fragment. Multi-batch rays run the full sequence to done on one
// reused output arena, threading the returned cursor between calls.
func kernelRaycastCases(t *testing.T) ([]kernelDraftAsset, []kernelDraftSelection) {
	t.Helper()
	const outputBytes = 1280
	runSequence := func(input, cursor []byte) (Status, []byte, []byte, []byte, []map[string]any) {
		before := append([]byte(nil), cursor...)
		output := make([]byte, outputBytes)
		for index := range output {
			output[index] = 0xa5
		}
		var batches []map[string]any
		for range 4 {
			status, count, done := raycastBatchVersion(ABIVersion, input, cursor, output)
			if status != StatusOK {
				return status, output, cursor, before, batches
			}
			batches = append(batches, kernelRaycastBatchJSON(int(count), done != 0, output, cursor))
			if done != 0 {
				return StatusOK, output, cursor, before, batches
			}
		}
		t.Fatal("raycast sequence did not finish within four batches")
		return StatusPanic, output, cursor, before, batches
	}

	type observation struct {
		label         string
		input         []byte
		cursor        []byte
		want          Status
		wantBatches   [][2]int
		bufferVariant string
	}
	observations := []observation{
		{
			label:         "second-batch",
			input:         kernelRawRaycastInput([3]float32{0.5, 0.5, 0.5}, [3]float32{1, 0, 0}, 70),
			cursor:        kernelFreshRaycastCursor(),
			want:          StatusOK,
			wantBatches:   [][2]int{{64, 0}, {7, 1}},
			bufferVariant: "normal",
		},
		{
			label:         "tampered-cursor",
			input:         kernelRawRaycastInput([3]float32{0.5, 0.5, 0.5}, [3]float32{1, 0, 0}, 70),
			cursor:        kernelFreshRaycastCursor(),
			want:          StatusInput,
			bufferVariant: "normal",
		},
		{
			label:         "record-65",
			input:         kernelRawRaycastInput([3]float32{0.5, 0.5, 0.5}, [3]float32{0, 0, 1}, 70),
			cursor:        kernelFreshRaycastCursor(),
			want:          StatusOK,
			wantBatches:   [][2]int{{64, 0}, {7, 1}},
			bufferVariant: "normal",
		},
	}
	observations[1].cursor[9] = 1

	var assets []kernelDraftAsset
	var selections []kernelDraftSelection
	for _, obs := range observations {
		status, output, cursor, before, batches := runSequence(obs.input, obs.cursor)
		if status != obs.want {
			t.Fatalf("%s status=%d, want %d", obs.label, status, obs.want)
		}
		// The frozen input asset carries the 40-byte request plus the
		// starting 64-byte cursor, so consumers reproduce the exact batch
		// thread without label-driven behavior.
		assetInput := append(append([]byte(nil), obs.input...), before...)
		extra := map[string]any{}
		if status == StatusOK {
			if len(batches) != len(obs.wantBatches) {
				t.Fatalf("%s batches=%d, want %d", obs.label, len(batches), len(obs.wantBatches))
			}
			for index, want := range obs.wantBatches {
				if batches[index]["count"] != want[0] || batches[index]["done"] != want[1] {
					t.Fatalf("%s batch %d count/done=%v/%v, want %d/%d",
						obs.label, index, batches[index]["count"], batches[index]["done"], want[0], want[1])
				}
			}
			extra["batches"] = batches
			last := batches[len(batches)-1]
			extra["used_payload_sha256"] = kernelDigest(output[:last["count"].(int)*20])
		} else {
			for index, value := range output {
				if value != 0xa5 {
					t.Fatalf("%s output[%d]=%#x modified on failure", obs.label, index, value)
				}
			}
			for index := range cursor {
				if cursor[index] != before[index] {
					t.Fatalf("%s cursor[%d] modified on failure", obs.label, index)
				}
			}
			extra["cursor_sha256"] = kernelDigest(cursor)
		}
		inputAsset, expectedAsset, selection := finishKernelBinaryCase(
			t, "kernel.mornlea_raycast_batch", obs.label, assetInput, outputBytes, obs.bufferVariant,
			status, output, extra, "raycast",
		)
		assets = append(assets, inputAsset, expectedAsset)
		selections = append(selections, selection)
	}
	return assets, selections
}

// kernelRawWorldgenHeader builds a valid 566-byte MGW1 header: layout 3,
// the given seed, Y range [-64, 320), the distinct 1..=15 material table and
// the identity permutation. Worldgen and LOD share this header.
func kernelRawWorldgenHeader(seed uint64) []byte {
	header := make([]byte, 566)
	copy(header[:4], "MGW1")
	binary.LittleEndian.PutUint32(header[4:8], 3)
	binary.LittleEndian.PutUint64(header[8:16], seed)
	minY := int32(-64)
	binary.LittleEndian.PutUint32(header[16:20], uint32(minY))
	binary.LittleEndian.PutUint32(header[20:24], 320)
	for index := 0; index < 15; index++ {
		binary.LittleEndian.PutUint16(header[24+index*2:26+index*2], uint16(index+1))
	}
	for index := 0; index < 512; index++ {
		header[54+index] = byte(index & 255)
	}
	return header
}

// kernelWorldgenChunkCases executes the three chunk corpus observations
// against the real exported symbol and returns the draft assets plus the
// manifest selection fragment.
func kernelWorldgenChunkCases(t *testing.T) ([]kernelDraftAsset, []kernelDraftSelection) {
	t.Helper()
	const outputBytes = 16 * 16 * 384 * 2
	chunkInput := func(seed uint64, chunkX, chunkZ int32) []byte {
		input := kernelRawWorldgenHeader(seed)
		input = binary.LittleEndian.AppendUint32(input, uint32(chunkX))
		return binary.LittleEndian.AppendUint32(input, uint32(chunkZ))
	}
	duplicate := chunkInput(42, 0, 0)
	binary.LittleEndian.PutUint16(duplicate[26:28], 1)
	run := func(input []byte, outputCapacity int) (Status, []byte) {
		output := make([]byte, outputCapacity)
		for index := range output {
			output[index] = 0xa5
		}
		return worldgenChunkVersion(ABIVersion, input, output), output
	}
	type observation struct {
		label          string
		input          []byte
		outputCapacity int
		bufferVariant  string
		want           Status
	}
	observations := []observation{
		{label: "seed-zero-chunk-zero", input: chunkInput(0, 0, 0), outputCapacity: outputBytes, bufferVariant: "normal", want: StatusOK},
		{label: "duplicate-material", input: duplicate, outputCapacity: outputBytes, bufferVariant: "normal", want: StatusInput},
		{label: "signed-extreme", input: chunkInput(0, -134217728, -134217728), outputCapacity: outputBytes, bufferVariant: "normal", want: StatusOK},
	}
	var assets []kernelDraftAsset
	var selections []kernelDraftSelection
	for _, obs := range observations {
		status, output := run(obs.input, obs.outputCapacity)
		if status != obs.want {
			t.Fatalf("%s status=%d, want %d", obs.label, status, obs.want)
		}
		extra := map[string]any{}
		if status == StatusOK {
			if obs.label == "seed-zero-chunk-zero" {
				for index := 0; index < 16*16; index++ {
					if got := binary.LittleEndian.Uint16(output[index*2 : index*2+2]); got != 5 {
						t.Fatalf("seed-zero bedrock layer index=%d got %d, want 5", index, got)
					}
				}
			}
			extra["used_payload_sha256"] = kernelDigest(output)
		} else {
			for index, value := range output {
				if value != 0xa5 {
					t.Fatalf("%s output[%d]=%#x modified on failure", obs.label, index, value)
				}
			}
		}
		inputAsset, expectedAsset, selection := finishKernelBinaryCase(
			t, "kernel.mornlea_worldgen_chunk", obs.label, obs.input, obs.outputCapacity, obs.bufferVariant,
			status, output, extra, "worldgen-chunk",
		)
		assets = append(assets, inputAsset, expectedAsset)
		selections = append(selections, selection)
	}
	return assets, selections
}

// kernelWorldgenProbeCases executes the four probe corpus observations
// against the real exported symbol and returns the draft assets plus the
// manifest selection fragment.
func kernelWorldgenProbeCases(t *testing.T) ([]kernelDraftAsset, []kernelDraftSelection) {
	t.Helper()
	probeInput := func(queries [][4]uint32) []byte {
		input := kernelRawWorldgenHeader(42)
		input = binary.LittleEndian.AppendUint32(input, uint32(len(queries)))
		for _, query := range queries {
			for _, word := range query {
				input = binary.LittleEndian.AppendUint32(input, word)
			}
		}
		return input
	}
	base := [][4]uint32{{0, 3, 0, 5}, {1, 3, 0, 5}, {2, 3, 0, 5}}
	modeThree := [][4]uint32{{3, 3, 0, 5}}
	query64 := make([][4]uint32, 64)
	for index := range query64 {
		query64[index] = [4]uint32{0, uint32(index), 0, 0}
	}
	query65 := make([][4]uint32, 65)
	for index := range query65 {
		query65[index] = [4]uint32{0, uint32(index), 0, 0}
	}
	run := func(input []byte, outputCapacity int) (Status, []byte) {
		output := make([]byte, outputCapacity)
		for index := range output {
			output[index] = 0xa5
		}
		return worldgenProbeVersion(ABIVersion, input, output), output
	}
	type observation struct {
		label          string
		input          []byte
		outputCapacity int
		bufferVariant  string
		want           Status
		wantCount      int
	}
	observations := []observation{
		{label: "height-terrain-base", input: probeInput(base), outputCapacity: 24, bufferVariant: "normal", want: StatusOK, wantCount: 3},
		{label: "mode-three", input: probeInput(modeThree), outputCapacity: 8, bufferVariant: "normal", want: StatusInput},
		{label: "query-64", input: probeInput(query64), outputCapacity: 512, bufferVariant: "normal", want: StatusOK, wantCount: 64},
		{label: "query-65", input: probeInput(query65), outputCapacity: 520, bufferVariant: "normal", want: StatusInput},
	}
	var assets []kernelDraftAsset
	var selections []kernelDraftSelection
	for _, obs := range observations {
		status, output := run(obs.input, obs.outputCapacity)
		if status != obs.want {
			t.Fatalf("%s status=%d, want %d", obs.label, status, obs.want)
		}
		extra := map[string]any{}
		if status == StatusOK {
			records := make([]string, 0, obs.wantCount)
			for index := 0; index < obs.wantCount; index++ {
				records = append(records, hex.EncodeToString(output[index*8:(index+1)*8]))
			}
			extra["output_count"] = obs.wantCount
			extra["records"] = records
			extra["used_payload_sha256"] = kernelDigest(output)
		} else {
			for index, value := range output {
				if value != 0xa5 {
					t.Fatalf("%s output[%d]=%#x modified on failure", obs.label, index, value)
				}
			}
		}
		inputAsset, expectedAsset, selection := finishKernelBinaryCase(
			t, "kernel.mornlea_worldgen_probe", obs.label, obs.input, obs.outputCapacity, obs.bufferVariant,
			status, output, extra, "worldgen-probe",
		)
		assets = append(assets, inputAsset, expectedAsset)
		selections = append(selections, selection)
	}
	return assets, selections
}

// kernelRawTreeBlocksInput builds the 28-byte runtime tree request: layout
// 1, the given seed and root coordinates.
func kernelRawTreeBlocksInput(seed uint64, rootX, rootY, rootZ int32) []byte {
	input := make([]byte, 0, 28)
	input = append(input, "MTB1"...)
	input = binary.LittleEndian.AppendUint32(input, 1)
	input = binary.LittleEndian.AppendUint64(input, seed)
	input = binary.LittleEndian.AppendUint32(input, uint32(rootX))
	input = binary.LittleEndian.AppendUint32(input, uint32(rootY))
	return binary.LittleEndian.AppendUint32(input, uint32(rootZ))
}

// kernelTreeCases executes the three tree corpus observations against the
// real exported symbol and returns the draft assets plus the manifest
// selection fragment.
func kernelTreeCases(t *testing.T) ([]kernelDraftAsset, []kernelDraftSelection) {
	t.Helper()
	const maxOutputBytes = 4 + 128*8
	oak := kernelRawTreeBlocksInput(42, 7, 64, -9)
	below := kernelRawTreeBlocksInput(42, 7, -65, -9)
	run := func(input []byte, outputCapacity int) (Status, int, []byte) {
		output := make([]byte, outputCapacity)
		for index := range output {
			output[index] = 0xa5
		}
		status, count := treeBlocksVersion(ABIVersion, input, output)
		return status, count, output
	}
	status, count, _ := run(oak, maxOutputBytes)
	if status != StatusOK {
		t.Fatalf("oak-order status=%d, want OK", status)
	}
	if count <= 0 || count > 128 {
		t.Fatalf("oak-order count=%d, want 1..128", count)
	}
	short := 4 + count*8 - 1
	observations := []struct {
		label          string
		input          []byte
		outputCapacity int
		bufferVariant  string
		want           Status
		wantCount      int
	}{
		{label: "oak-order", input: oak, outputCapacity: maxOutputBytes, bufferVariant: "normal", want: StatusOK, wantCount: count},
		{label: "root-y-minus-65", input: below, outputCapacity: maxOutputBytes, bufferVariant: "normal", want: StatusInput},
		{label: "short-by-one", input: oak, outputCapacity: short, bufferVariant: "short", want: StatusOutputOverflow},
	}
	var assets []kernelDraftAsset
	var selections []kernelDraftSelection
	for _, obs := range observations {
		status, count, output := run(obs.input, obs.outputCapacity)
		if status != obs.want {
			t.Fatalf("%s status=%d, want %d", obs.label, status, obs.want)
		}
		extra := map[string]any{}
		if status == StatusOK {
			if count != obs.wantCount {
				t.Fatalf("%s count=%d, want %d", obs.label, count, obs.wantCount)
			}
			extra["output_count"] = count
			extra["records_sha256"] = kernelDigest(output[:4+count*8])
			extra["used_payload_sha256"] = kernelDigest(output[:4+count*8])
		} else {
			if count != 0 {
				t.Fatalf("%s failure count=%d, want 0", obs.label, count)
			}
			for index, value := range output {
				if value != 0xa5 {
					t.Fatalf("%s output[%d]=%#x modified on failure", obs.label, index, value)
				}
			}
		}
		inputAsset, expectedAsset, selection := finishKernelBinaryCase(
			t, "kernel.mornlea_tree_blocks", obs.label, obs.input, obs.outputCapacity, obs.bufferVariant,
			status, output, extra, "tree",
		)
		assets = append(assets, inputAsset, expectedAsset)
		selections = append(selections, selection)
	}
	return assets, selections
}

// kernelRawLodShellInput builds a valid 582-byte shell request: the
// identity-material header shared with the engine golden fixture, tile
// coordinates, the fixed 64 column count and the step size.
func kernelRawLodShellInput(tileX, tileZ int32, step uint32) []byte {
	input := make([]byte, 566)
	copy(input[:4], "MGW1")
	binary.LittleEndian.PutUint32(input[4:8], 3)
	binary.LittleEndian.PutUint64(input[8:16], 42)
	minY := int32(-64)
	binary.LittleEndian.PutUint32(input[16:20], uint32(minY))
	binary.LittleEndian.PutUint32(input[20:24], 320)
	for index := uint16(0); index < 15; index++ {
		binary.LittleEndian.PutUint16(input[24+2*int(index):26+2*int(index)], index)
	}
	for index := 0; index < 512; index++ {
		input[54+index] = byte(index & 255)
	}
	input = binary.LittleEndian.AppendUint32(input, uint32(tileX))
	input = binary.LittleEndian.AppendUint32(input, uint32(tileZ))
	input = binary.LittleEndian.AppendUint32(input, 64)
	return binary.LittleEndian.AppendUint32(input, step)
}

// kernelLodCases executes the four LOD shell corpus observations against the
// real exported symbol and returns the draft assets plus the manifest
// selection fragment. The short-capacity probe and its exact retry pin the
// two-phase contract: overflow reports the needed bytes without touching the
// payload, and the retry publishes the identical stream.
func kernelLodCases(t *testing.T) ([]kernelDraftAsset, []kernelDraftSelection) {
	t.Helper()
	stepTwo := kernelRawLodShellInput(-3, 2, 2)
	invalidStep := kernelRawLodShellInput(-3, 2, 3)
	run := func(input []byte, outputCapacity int) (Status, int, []byte) {
		output := make([]byte, outputCapacity)
		for index := range output {
			output[index] = 0xa5
		}
		status, outputLen := lodShellVersion(ABIVersion, input, output)
		return status, outputLen, output
	}
	status, needed, output := run(stepTwo, 1)
	if status != StatusOutputOverflow || needed == 0 || needed%20 != 0 {
		t.Fatalf("step-two probe status/needed=%d/%d, want overflow/multiple of 20", status, needed)
	}
	for index, value := range output {
		if value != 0xa5 {
			t.Fatalf("step-two probe output[%d]=%#x modified on overflow", index, value)
		}
	}
	status, written, output := run(stepTwo, needed)
	if status != StatusOK || written != needed {
		t.Fatalf("step-two status/written=%d/%d, want OK/%d", status, written, needed)
	}
	quads := kernelDigest(output[:written])
	observations := []struct {
		label          string
		input          []byte
		outputCapacity int
		bufferVariant  string
		want           Status
		wantLen        int
		extra          map[string]any
	}{
		{label: "step-two", input: stepTwo, outputCapacity: needed, bufferVariant: "normal", want: StatusOK, wantLen: needed,
			extra: map[string]any{"quads_sha256": quads, "used_payload_sha256": quads, "output_count": needed / 20}},
		{label: "invalid-step", input: invalidStep, outputCapacity: 1, bufferVariant: "normal", want: StatusInput, extra: map[string]any{}},
		{label: "exact-needed-short", input: stepTwo, outputCapacity: needed - 1, bufferVariant: "short", want: StatusOutputOverflow, wantLen: needed,
			extra: map[string]any{"needed_bytes": needed}},
		{label: "exact-needed-retry", input: stepTwo, outputCapacity: needed, bufferVariant: "normal", want: StatusOK, wantLen: needed,
			extra: map[string]any{"quads_sha256": quads, "used_payload_sha256": quads, "output_count": needed / 20}},
	}
	var assets []kernelDraftAsset
	var selections []kernelDraftSelection
	for _, obs := range observations {
		status, outputLen, output := run(obs.input, obs.outputCapacity)
		if status != obs.want {
			t.Fatalf("%s status=%d, want %d", obs.label, status, obs.want)
		}
		if status == StatusOK {
			if outputLen != obs.wantLen {
				t.Fatalf("%s output_len=%d, want %d", obs.label, outputLen, obs.wantLen)
			}
			if got := kernelDigest(output[:outputLen]); got != quads {
				t.Fatalf("%s quads digest=%s, want %s", obs.label, got, quads)
			}
		} else {
			if outputLen != 0 && obs.label == "invalid-step" {
				t.Fatalf("%s output_len=%d, want 0", obs.label, outputLen)
			}
			if obs.label == "exact-needed-short" && outputLen != needed {
				t.Fatalf("%s needed=%d, want %d", obs.label, outputLen, needed)
			}
			for index, value := range output {
				if value != 0xa5 {
					t.Fatalf("%s output[%d]=%#x modified on failure", obs.label, index, value)
				}
			}
		}
		extra := map[string]any{}
		for key, value := range obs.extra {
			extra[key] = value
		}
		inputAsset, expectedAsset, selection := finishKernelBinaryCase(
			t, "kernel.mornlea_lod_shell", obs.label, obs.input, obs.outputCapacity, obs.bufferVariant,
			status, output, extra, "lod",
		)
		assets = append(assets, inputAsset, expectedAsset)
		selections = append(selections, selection)
	}
	return assets, selections
}

// kernelRawFluidEvalInput builds the two-item evaluation scene the binding
// pins: a source over replaceable air below (vertical priority writes one
// level-1 record downward) beside a sealed level-7 cell (no writes).
func kernelRawFluidEvalInput() []byte {
	input := make([]byte, 0, 8+2*14)
	input = binary.LittleEndian.AppendUint32(input, 1)
	input = binary.LittleEndian.AppendUint32(input, 2)
	for _, item := range [][]uint16{
		{27, 2, 0, 2, 2, 2, 2},
		{34, 27, 2, 0, 0, 0, 0},
	} {
		for _, id := range item {
			input = binary.LittleEndian.AppendUint16(input, id)
		}
	}
	return input
}

// kernelPinnedFluidEvalOutput is the accepted binding observation for the
// two-item scene: item 0 writes slot 2 with block 28, every other slot is
// the no-write sentinel.
var kernelPinnedFluidEvalOutput = []byte{
	0x02, 0x1c, 0x00,
	0xff, 0x00, 0x00,
	0xff, 0x00, 0x00,
	0xff, 0x00, 0x00,
	0xff, 0x00, 0x00,
	0xff, 0x00, 0x00,
	0xff, 0x00, 0x00,
	0xff, 0x00, 0x00,
}

// kernelFluidEvalCases executes the three fluid evaluation corpus
// observations against the real exported symbol and returns the draft assets
// plus the manifest selection fragment.
func kernelFluidEvalCases(t *testing.T) ([]kernelDraftAsset, []kernelDraftSelection) {
	t.Helper()
	valid := kernelRawFluidEvalInput()
	malformed := append([]byte(nil), valid[:len(valid)-1]...)
	over := make([]byte, 0, 8+4097*14)
	over = binary.LittleEndian.AppendUint32(over, 1)
	over = binary.LittleEndian.AppendUint32(over, 4097)
	for range 4097 {
		for _, id := range []uint16{27, 2, 0, 2, 2, 2, 2} {
			over = binary.LittleEndian.AppendUint16(over, id)
		}
	}
	run := func(input []byte, outputCapacity int) (Status, []byte) {
		output := make([]byte, outputCapacity)
		for index := range output {
			output[index] = 0xa5
		}
		return fluidEvalBatchVersion(ABIVersion, input, output), output
	}
	type observation struct {
		label          string
		input          []byte
		outputCapacity int
		bufferVariant  string
		want           Status
		pinned         []byte
	}
	observations := []observation{
		{label: "vertical-priority", input: valid, outputCapacity: 24, bufferVariant: "normal", want: StatusOK, pinned: kernelPinnedFluidEvalOutput},
		{label: "malformed-length", input: malformed, outputCapacity: 12, bufferVariant: "normal", want: StatusInput},
		{label: "count-4097", input: over, outputCapacity: 4097 * 12, bufferVariant: "normal", want: StatusOK},
	}
	var assets []kernelDraftAsset
	var selections []kernelDraftSelection
	for _, obs := range observations {
		status, output := run(obs.input, obs.outputCapacity)
		if status != obs.want {
			t.Fatalf("%s status=%d, want %d", obs.label, status, obs.want)
		}
		extra := map[string]any{}
		if status == StatusOK {
			if obs.pinned != nil {
				for index, value := range obs.pinned {
					if output[index] != value {
						t.Fatalf("%s output[%d]=%#x, want %#x", obs.label, index, output[index], value)
					}
				}
				extra["output_hex"] = hex.EncodeToString(output)
			}
			extra["used_payload_sha256"] = kernelDigest(output)
		} else {
			for index, value := range output {
				if value != 0xa5 {
					t.Fatalf("%s output[%d]=%#x modified on failure", obs.label, index, value)
				}
			}
		}
		inputAsset, expectedAsset, selection := finishKernelBinaryCase(
			t, "kernel.mornlea_fluid_eval_batch", obs.label, obs.input, obs.outputCapacity, obs.bufferVariant,
			status, output, extra, "fluid-eval",
		)
		assets = append(assets, inputAsset, expectedAsset)
		selections = append(selections, selection)
	}
	return assets, selections
}

// kernelRawFluidRescanInterior builds the minimal interior scene the binding
// pins: a dense section with one level-3 flowing cell emitting world
// position (-30, 3, 53) beside a sealed uniform source section.
func kernelRawFluidRescanInterior() []byte {
	input := make([]byte, 0, 26+23*4+8194+68*384*2+9*24*3)
	input = binary.LittleEndian.AppendUint32(input, 1)
	centerX := int32(-2)
	input = binary.LittleEndian.AppendUint32(input, uint32(centerX))
	input = binary.LittleEndian.AppendUint32(input, 3)
	input = binary.LittleEndian.AppendUint16(input, 1)
	input = binary.LittleEndian.AppendUint16(input, 16)
	input = binary.LittleEndian.AppendUint16(input, 1)
	input = binary.LittleEndian.AppendUint16(input, 16)
	input = append(input, 0)
	input = append(input, 0)
	input = binary.LittleEndian.AppendUint32(input, 65536)
	for section := 0; section < 24; section++ {
		if section == 4 {
			dense := make([]byte, 4096*2)
			for index := range dense {
				if index%2 == 0 {
					dense[index] = 2
				}
			}
			binary.LittleEndian.PutUint16(dense[850*2:], 30)
			input = append(input, 1, 0)
			input = append(input, dense...)
			continue
		}
		id := uint16(2)
		if section == 6 {
			id = 27
		}
		input = append(input, 0, 0)
		input = binary.LittleEndian.AppendUint16(input, id)
	}
	for range 68 * 384 {
		input = binary.LittleEndian.AppendUint16(input, 2)
	}
	for range 9 * 24 {
		input = append(input, 1, 2, 0)
	}
	return input
}

// kernelRawFluidRescanOuterHalo builds the outer-range scene whose seal check
// actually reads past the owned halo: a dense stone section with one skirt
// column of water sources over the outer box range. The accepted adapter
// observation converges this to the caught-panic status.
func kernelRawFluidRescanOuterHalo() []byte {
	input := make([]byte, 0, 26+23*4+8194+68*384*2+9*24*3)
	input = binary.LittleEndian.AppendUint32(input, 1)
	input = binary.LittleEndian.AppendUint32(input, 0)
	input = binary.LittleEndian.AppendUint32(input, 0)
	input = binary.LittleEndian.AppendUint16(input, 0)
	input = binary.LittleEndian.AppendUint16(input, 2)
	input = binary.LittleEndian.AppendUint16(input, 5)
	input = binary.LittleEndian.AppendUint16(input, 5)
	input = append(input, 0)
	input = append(input, 0)
	input = binary.LittleEndian.AppendUint32(input, 100000)
	for section := 0; section < 24; section++ {
		if section == 0 {
			dense := make([]byte, 4096*2)
			for index := range dense {
				if index%2 == 0 {
					dense[index] = 2
				}
			}
			input = append(input, 1, 0)
			input = append(input, dense...)
			continue
		}
		input = append(input, 0, 0)
		input = binary.LittleEndian.AppendUint16(input, 2)
	}
	for column := 0; column < 68; column++ {
		for range 384 {
			id := uint16(2)
			if column == 4 {
				id = 27
			}
			input = binary.LittleEndian.AppendUint16(input, id)
		}
	}
	for range 9 * 24 {
		input = append(input, 1, 2, 0)
	}
	return input
}

// kernelDecodeRescanSummary reads the trailing spent and done fields.
func kernelDecodeRescanSummary(output []byte) (uint32, bool) {
	tail := output[len(output)-8:]
	return binary.LittleEndian.Uint32(tail[0:4]), tail[4] == 1
}

// kernelDecodeRescanPositions reads the ordered emitted world positions.
func kernelDecodeRescanPositions(output []byte) [][3]int32 {
	var positions [][3]int32
	for offset := 0; offset+12 <= len(output)-8; offset += 12 {
		positions = append(positions, [3]int32{
			int32(binary.LittleEndian.Uint32(output[offset : offset+4])),
			int32(binary.LittleEndian.Uint32(output[offset+4 : offset+8])),
			int32(binary.LittleEndian.Uint32(output[offset+8 : offset+12])),
		})
	}
	return positions
}

// kernelFluidRescanCases executes the four fluid rescan corpus observations
// against the real exported symbol and returns the draft assets plus the
// manifest selection fragment.
func kernelFluidRescanCases(t *testing.T) ([]kernelDraftAsset, []kernelDraftSelection) {
	t.Helper()
	interior := kernelRawFluidRescanInterior()
	halo := kernelRawFluidRescanOuterHalo()
	budget := append([]byte(nil), interior...)
	binary.LittleEndian.PutUint32(budget[22:26], 1)
	run := func(input []byte, outputCapacity int) (Status, int, []byte) {
		output := make([]byte, outputCapacity)
		for index := range output {
			output[index] = 0xa5
		}
		status, outputLen := fluidRescanVersion(ABIVersion, input, output)
		return status, outputLen, output
	}
	status, written, output := run(interior, 20)
	if status != StatusOK || written != 20 {
		t.Fatalf("interior-source status/written=%d/%d, want OK/20", status, written)
	}
	positions := kernelDecodeRescanPositions(output[:written])
	if len(positions) != 1 || positions[0] != [3]int32{-30, 3, 53} {
		t.Fatalf("interior-source positions=%v, want [[-30 3 53]]", positions)
	}
	spent, done := kernelDecodeRescanSummary(output[:written])
	if spent != 4119 || !done {
		t.Fatalf("interior-source spent/done=%d/%v, want 4119/true", spent, done)
	}
	status, needed, _ := run(budget, 1)
	if status != StatusOutputOverflow || needed == 0 {
		t.Fatalf("budget-one probe status/needed=%d/%d, want overflow/nonzero", status, needed)
	}
	observations := []struct {
		label          string
		input          []byte
		outputCapacity int
		bufferVariant  string
		want           Status
	}{
		{label: "interior-source", input: interior, outputCapacity: 20, bufferVariant: "normal", want: StatusOK},
		{label: "outer-missing-halo", input: halo, outputCapacity: 64, bufferVariant: "normal", want: StatusPanic},
		{label: "budget-one-short", input: budget, outputCapacity: needed - 1, bufferVariant: "short", want: StatusOutputOverflow},
		{label: "budget-one-retry", input: budget, outputCapacity: needed, bufferVariant: "normal", want: StatusOK},
	}
	var assets []kernelDraftAsset
	var selections []kernelDraftSelection
	for _, obs := range observations {
		status, outputLen, output := run(obs.input, obs.outputCapacity)
		if status != obs.want {
			t.Fatalf("%s status=%d, want %d", obs.label, status, obs.want)
		}
		extra := map[string]any{}
		if status == StatusOK {
			spent, done := kernelDecodeRescanSummary(output[:outputLen])
			doneFlag := 0
			if done {
				doneFlag = 1
			}
			ordered := make([][3]int32, 0)
			for _, position := range kernelDecodeRescanPositions(output[:outputLen]) {
				ordered = append(ordered, position)
			}
			extra["spent"] = int(spent)
			extra["done"] = doneFlag
			extra["positions"] = ordered
			extra["used_payload_sha256"] = kernelDigest(output[:outputLen])
		} else {
			if outputLen != 0 && obs.label == "outer-missing-halo" {
				t.Fatalf("%s output_len=%d, want 0", obs.label, outputLen)
			}
			if obs.label == "budget-one-short" {
				if outputLen != needed {
					t.Fatalf("%s needed=%d, want %d", obs.label, outputLen, needed)
				}
				extra["needed_bytes"] = needed
			}
			for index, value := range output {
				if value != 0xa5 {
					t.Fatalf("%s output[%d]=%#x modified on failure", obs.label, index, value)
				}
			}
		}
		inputAsset, expectedAsset, selection := finishKernelBinaryCase(
			t, "kernel.mornlea_fluid_rescan", obs.label, obs.input, obs.outputCapacity, obs.bufferVariant,
			status, output, extra, "fluid-rescan",
		)
		assets = append(assets, inputAsset, expectedAsset)
		selections = append(selections, selection)
	}
	return assets, selections
}

// kernelMeshEntry is one 20-byte registry record in wire order.
type kernelMeshEntry struct {
	id               uint16
	opaque           bool
	emission         uint8
	material         [6]uint16
	fluidHeight      uint8
	lightAttenuation uint8
	blockTopRaw      uint8
	model            uint8
}

// kernelMeshSection assembles one raw mesh-section request, mirroring the
// accepted oracle fixture structurally.
type kernelMeshSection struct {
	originY   int32
	airID     uint16
	barrierID uint16
	blocks    []uint16
	entries   []kernelMeshEntry
	visible   []uint64
}

func newKernelMeshSection(entries []kernelMeshEntry) *kernelMeshSection {
	wordsPerRow := (len(entries) + 63) / 64
	return &kernelMeshSection{
		airID:     0,
		barrierID: 1,
		blocks:    make([]uint16, 27*4096),
		entries:   entries,
		visible:   make([]uint64, len(entries)*wordsPerRow),
	}
}

func kernelMeshCell(x, y, z int32) int {
	section := int((x+16)>>4)*3 + int((y+16)>>4)
	section = section*3 + int((z+16)>>4)
	offset := int((y+16)&15)<<8 | int((z+16)&15)<<4 | int((x+16)&15)
	return section*4096 + offset
}

func (s *kernelMeshSection) setBlock(x, y, z int32, id uint16) {
	s.blocks[kernelMeshCell(x, y, z)] = id
}

func (s *kernelMeshSection) fillBlock(id uint16) {
	for index := range s.blocks {
		s.blocks[index] = id
	}
}

func (s *kernelMeshSection) markFaceVisible(row, column int) {
	wordsPerRow := (len(s.entries) + 63) / 64
	s.visible[row*wordsPerRow+column/64] |= 1 << (uint(column) % 64)
}

func (s *kernelMeshSection) encode() []byte {
	wordsPerRow := (len(s.entries) + 63) / 64
	input := make([]byte, 16+27*4096*2+9+9*256*2+len(s.entries)*20+len(s.visible)*8)
	copy(input[0:4], "MGM1")
	binary.LittleEndian.PutUint32(input[4:8], uint32(s.originY))
	binary.LittleEndian.PutUint16(input[8:10], uint16(len(s.entries)))
	binary.LittleEndian.PutUint16(input[10:12], uint16(wordsPerRow))
	binary.LittleEndian.PutUint16(input[12:14], s.airID)
	binary.LittleEndian.PutUint16(input[14:16], s.barrierID)
	offset := 16
	for _, block := range s.blocks {
		binary.LittleEndian.PutUint16(input[offset:], block)
		offset += 2
	}
	offset += 9 + 9*256*2
	for _, entry := range s.entries {
		binary.LittleEndian.PutUint16(input[offset:], entry.id)
		if entry.opaque {
			input[offset+2] = 1
		}
		input[offset+3] = entry.emission
		for face, material := range entry.material {
			binary.LittleEndian.PutUint16(input[offset+4+face*2:], material)
		}
		input[offset+16] = entry.fluidHeight
		input[offset+17] = entry.lightAttenuation
		input[offset+18] = entry.blockTopRaw
		input[offset+19] = entry.model
		offset += 20
	}
	for _, word := range s.visible {
		binary.LittleEndian.PutUint64(input[offset:], word)
		offset += 8
	}
	return input
}

// kernelMeshFacesSection is the isolated-block scene: one opaque stone at the
// center cell whose six axial faces are visible toward air.
func kernelMeshFacesSection() *kernelMeshSection {
	section := newKernelMeshSection([]kernelMeshEntry{
		{id: 0},
		{id: 1, opaque: true, material: [6]uint16{1, 1, 1, 1, 1, 1}},
		{id: 2, opaque: true, material: [6]uint16{10, 11, 12, 13, 14, 15}},
	})
	section.setBlock(0, 0, 0, 2)
	section.markFaceVisible(2, 0)
	return section
}

// kernelMeshRegistry97Section carries 97 registry entries over a non-air
// center cell, so the structural all-air shortcut cannot mask the registry
// capacity rejection.
func kernelMeshRegistry97Section() *kernelMeshSection {
	entries := []kernelMeshEntry{{id: 0}, {id: 1, opaque: true, material: [6]uint16{1, 1, 1, 1, 1, 1}}}
	for id := uint16(2); id < 97; id++ {
		entries = append(entries, kernelMeshEntry{id: id, opaque: true, material: [6]uint16{id, id, id, id, id, id}})
	}
	section := newKernelMeshSection(entries)
	section.setBlock(0, 0, 0, 2)
	section.markFaceVisible(2, 0)
	return section
}

// kernelMeshDenseCombinedSection fills the whole neighborhood with standing
// torches whose plant-layer material makes the plant pass and the model
// dispatcher publish eight quads per cell: 32768 quads, past the
// production-consistent 24576 minimum but inside the typed stage.
func kernelMeshDenseCombinedSection() *kernelMeshSection {
	section := newKernelMeshSection([]kernelMeshEntry{
		{id: 0},
		{id: 1, opaque: true, material: [6]uint16{1, 1, 1, 1, 1, 1}},
		{id: 71, material: [6]uint16{31, 31, 31, 31, 31, 31}, model: 1},
	})
	section.fillBlock(71)
	return section
}

// kernelMeshCases executes the three mesh corpus observations against the
// real exported symbol and returns the draft assets plus the manifest
// selection fragment. Output capacity is counted in u64 slots and the scratch
// arena in u64 words, matching the symbol's capacity units.
func kernelMeshCases(t *testing.T) ([]kernelDraftAsset, []kernelDraftSelection) {
	t.Helper()
	const scratchWords = (48 * 48 * 48 * 5) / 8
	const outputWords = 6 * 4096
	newScratch := func() []uint64 {
		scratch := make([]uint64, scratchWords)
		for index := range scratch {
			scratch[index] = 0xa5a5a5a5a5a5a5a5
		}
		return scratch
	}
	newOutput := func(words int) []uint64 {
		output := make([]uint64, words)
		for index := range output {
			output[index] = 0xa5a5a5a5a5a5a5a5
		}
		return output
	}
	run := func(input []byte, scratch []uint64, output []uint64) (Status, int) {
		return MeshSection(ABIVersion, input, scratch, output)
	}
	facesInput := kernelMeshFacesSection().encode()
	observations := []struct {
		label          string
		input          []byte
		outputCapacity int
		bufferVariant  string
		want           Status
	}{
		{label: "six-faces", input: facesInput, outputCapacity: outputWords, bufferVariant: "normal", want: StatusOK},
		{label: "registry-97", input: kernelMeshRegistry97Section().encode(), outputCapacity: outputWords, bufferVariant: "normal", want: StatusRegistry},
		{label: "model-plant-late-overflow", input: kernelMeshDenseCombinedSection().encode(), outputCapacity: outputWords, bufferVariant: "short", want: StatusOutputOverflow},
	}
	var assets []kernelDraftAsset
	var selections []kernelDraftSelection
	for _, obs := range observations {
		scratch := newScratch()
		output := newOutput(obs.outputCapacity)
		status, count := run(obs.input, scratch, output)
		if status != obs.want {
			t.Fatalf("%s status=%d, want %d", obs.label, status, obs.want)
		}
		extra := map[string]any{}
		if status == StatusOK {
			if count != 6 {
				t.Fatalf("%s count=%d, want 6", obs.label, count)
			}
			for index := 0; index < count; index++ {
				face := uint8(output[index] >> 20 & 7)
				material := uint16(output[index] >> 23)
				if face != uint8(index) || material != uint16(10+index) {
					t.Fatalf("%s quad %d face/material=%d/%d, want %d/%d",
						obs.label, index, face, material, index, 10+index)
				}
			}
			quads := make([]string, 0, count)
			for _, word := range output[:count] {
				quads = append(quads, hex.EncodeToString(wordToBytes(word)))
			}
			extra["output_count"] = count
			extra["quads"] = quads
			extra["quads_sha256"] = kernelDigest(quadsToBytes(output[:count]))
			extra["used_payload_sha256"] = kernelDigest(quadsToBytes(output[:count]))
		} else {
			if count != 0 {
				t.Fatalf("%s failure count=%d, want 0", obs.label, count)
			}
			for index, word := range output {
				if word != 0xa5a5a5a5a5a5a5a5 {
					t.Fatalf("%s output[%d]=%#x modified on failure", obs.label, index, word)
				}
			}
		}
		inputAsset, expectedAsset, selection := finishKernelMeshCase(
			t, obs.label, obs.input, obs.outputCapacity, obs.bufferVariant,
			status, output, extra,
		)
		assets = append(assets, inputAsset, expectedAsset)
		selections = append(selections, selection)
	}
	return assets, selections
}

// wordToBytes renders one packed quad word in little-endian byte order.
func wordToBytes(word uint64) []byte {
	raw := make([]byte, 8)
	binary.LittleEndian.PutUint64(raw, word)
	return raw
}

// quadsToBytes flattens used packed quad words to bytes for digesting.
func quadsToBytes(quads []uint64) []byte {
	raw := make([]byte, 0, len(quads)*8)
	for _, word := range quads {
		raw = append(raw, wordToBytes(word)...)
	}
	return raw
}

// finishKernelMeshCase builds the draft asset pair and selection entry for
// one mesh observation, whose capacities count u64 slots and whose scratch
// capacity rides in the arguments beside them.
func finishKernelMeshCase(
	t *testing.T,
	label string,
	input []byte,
	outputCapacity int,
	bufferVariant string,
	status Status,
	output []uint64,
	extraFields map[string]any,
) (kernelDraftAsset, kernelDraftAsset, kernelDraftSelection) {
	t.Helper()
	arena := quadsToBytes(output)
	id := "kernel.mornlea_mesh_section/11/" + label
	fields := map[string]any{
		"status":          int(status),
		"output_capacity": outputCapacity,
		"output_count":    0,
		"payload_sha256":  kernelDigest(arena),
	}
	for key, value := range extraFields {
		fields[key] = value
	}
	kind := "error"
	if status == StatusOK {
		kind = "ok"
	}
	arguments, err := json.Marshal(map[string]any{
		"abi_version":      ABIVersion,
		"output_capacity":  outputCapacity,
		"scratch_capacity": (48 * 48 * 48 * 5) / 8,
		"buffer_variant":   bufferVariant,
	})
	if err != nil {
		t.Fatalf("marshal arguments: %v", err)
	}
	expected := marshalKernelExpected(kind, kernelStatusName(status), fields)
	inputRel := "cases/kernel/mesh/" + label + ".input.bin"
	expectedRel := "cases/kernel/mesh/" + label + ".expected.json"
	return kernelDraftAsset{
			RelativePath: strings.ReplaceAll(id, "/", "-") + "/input.bin",
			Data:         append([]byte(nil), input...),
		}, kernelDraftAsset{
			RelativePath: strings.ReplaceAll(id, "/", "-") + "/expected.json",
			Data:         expected,
		}, kernelDraftSelection{
			ID:           id,
			Family:       "kernel.mornlea_mesh_section",
			Version:      "11",
			Operation:    "kernel",
			Arguments:    arguments,
			Input:        kernelDraftRef{Path: "testdata/runtime-migration/" + inputRel, SHA256: kernelDigest(input)},
			InputFormat:  "binary",
			Expected:     kernelDraftRef{Path: "testdata/runtime-migration/" + expectedRel, SHA256: kernelDigest(expected)},
			Checkpoints:  []string{"0"},
			RustConsumer: "mornlea_engine",
			Category:     kernelStatusName(status),
		}
}

// TestKernelABIRawOracle executes the corpus raw observations in-package and
// publishes reviewed drafts externally on demand. Without
// RUNTIME_ORACLE_EXPORT_DIR it only asserts current statuses and canaries.
func TestKernelABIRawOracle(t *testing.T) {
	var assets []kernelDraftAsset
	var selections []kernelDraftSelection
	for _, family := range []func(*testing.T) ([]kernelDraftAsset, []kernelDraftSelection){
		kernelCollisionCases,
		kernelPhysicsCases,
		kernelRaycastCases,
		kernelWorldgenChunkCases,
		kernelWorldgenProbeCases,
		kernelTreeCases,
		kernelLodCases,
		kernelFluidEvalCases,
		kernelFluidRescanCases,
		kernelMeshCases,
	} {
		familyAssets, familySelections := family(t)
		assets = append(assets, familyAssets...)
		selections = append(selections, familySelections...)
	}

	selectionRaw, err := json.MarshalIndent(selections, "", "  ")
	if err != nil {
		t.Fatalf("marshal selection: %v", err)
	}
	assets = append(assets, kernelDraftAsset{
		RelativePath: "selection.json",
		Data:         append(selectionRaw, '\n'),
	})
	if dir := exportKernelDrafts(t, kernelOracleExportChild, assets); dir != "" {
		t.Logf("exported %d kernel raw drafts to %s", len(selections), dir)
	}
}
