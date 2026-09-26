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

// TestKernelABIRawOracle executes the corpus raw observations in-package and
// publishes reviewed drafts externally on demand. Without
// RUNTIME_ORACLE_EXPORT_DIR it only asserts current statuses and canaries.
func TestKernelABIRawOracle(t *testing.T) {
	assets, selections := kernelCollisionCases(t)

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
