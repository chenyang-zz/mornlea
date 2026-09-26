package main

import (
	"bytes"
	"encoding/binary"
	"hash/fnv"
	"os"
	"path/filepath"
	"testing"

	"github.com/channing771/mornlea/packages/shared/nativeabi"
)

// Frozen fluid-eval framing: an 8-byte header (layout u32 = 1 + item count u32)
// followed by 14-byte input records (7 little-endian block ids in slot order
// self, above, below, +x, -x, +z, -z). Each item produces a 12-byte output
// record of four 3-byte entries (slot u8 + block id u16 LE); a slot byte of
// 0xFF marks an unused entry whose block id bytes are zero.
const (
	kernelFluidEvalItemInputBytes  = 14
	kernelFluidEvalItemOutputBytes = 12
	kernelFluidEvalSlotsPerItem    = 7
	kernelFluidEvalWriteEntries    = 4
	kernelFluidEvalSlotNoWrite     = 0xFF
	kernelFluidEvalLayoutVersion   = 1
)

// Stable block ids of the fluid rule table, matching `internal/core/block.go`.
const (
	kernelFluidAir         = 0
	kernelFluidStone       = 2
	kernelFluidWaterSource = 27
)

// kernelFluidEvalChange is one decoded output entry: the neighbour slot the
// rule wants written and the block id to write there.
type kernelFluidEvalChange struct {
	slot  uint8
	block uint16
}

// kernelFluidEvalItem is one 7-cell neighbourhood in slot order.
type kernelFluidEvalItem [kernelFluidEvalSlotsPerItem]uint16

// kernelFluidEvalInput encodes items with the frozen layout header. The batch
// count is derived from the items, so an empty batch still carries the full
// 8-byte header with a zero count.
func kernelFluidEvalInput(items []kernelFluidEvalItem) []byte {
	input := make([]byte, 8+len(items)*kernelFluidEvalItemInputBytes)
	binary.LittleEndian.PutUint32(input[0:4], kernelFluidEvalLayoutVersion)
	binary.LittleEndian.PutUint32(input[4:8], uint32(len(items)))
	for index, item := range items {
		base := 8 + index*kernelFluidEvalItemInputBytes
		for slot, cell := range item {
			binary.LittleEndian.PutUint16(input[base+slot*2:], cell)
		}
	}
	return input
}

// kernelFluidEvalChanges decodes one 12-byte output record into its ordered
// change list. Unused entries carry the 0xFF sentinel and are skipped, so a
// record with no writes decodes to an empty slice rather than to padding.
func kernelFluidEvalChanges(record []byte) []kernelFluidEvalChange {
	changes := make([]kernelFluidEvalChange, 0, kernelFluidEvalWriteEntries)
	for index := 0; index < kernelFluidEvalWriteEntries; index++ {
		entry := record[index*3 : index*3+3]
		if entry[0] == kernelFluidEvalSlotNoWrite {
			continue
		}
		changes = append(changes, kernelFluidEvalChange{
			slot:  entry[0],
			block: binary.LittleEndian.Uint16(entry[1:3]),
		})
	}
	return changes
}

// kernelFluidEvalDigest fingerprints an output byte stream with FNV-1a 64-bit.
// The Rust numerical-migration fluid test implements the same digest over the
// same encoded records so both sides pin one shared release-ABI value.
func kernelFluidEvalDigest(output []byte) uint64 {
	hash := fnv.New64a()
	hash.Write(output)
	return hash.Sum64()
}

// kernelFluidEvalRuleMatrix is the shared rule-matrix observation batch. It
// covers vertical priority, horizontal spreading, non-source decay, flowing
// survival, the infinite-source upgrade and the unknown-id skip, in that
// order; the Rust numerical-migration fluid test pins the same items and the
// same digests.
func kernelFluidEvalRuleMatrix() []kernelFluidEvalItem {
	return []kernelFluidEvalItem{
		{kernelFluidWaterSource, kernelFluidStone, kernelFluidAir, kernelFluidStone, kernelFluidStone, kernelFluidStone, kernelFluidStone},
		{kernelFluidWaterSource, kernelFluidStone, kernelFluidStone, kernelFluidAir, kernelFluidAir, kernelFluidAir, kernelFluidAir},
		{kernelFluidWaterSource + 2, kernelFluidStone, kernelFluidAir, kernelFluidStone, kernelFluidStone, kernelFluidStone, kernelFluidStone},
		{kernelFluidWaterSource + 3, kernelFluidStone, kernelFluidStone, kernelFluidWaterSource, kernelFluidAir, kernelFluidAir, kernelFluidAir},
		{kernelFluidAir, kernelFluidStone, kernelFluidStone, kernelFluidWaterSource, kernelFluidWaterSource, kernelFluidStone, kernelFluidStone},
		{65535, kernelFluidStone, kernelFluidStone, kernelFluidStone, kernelFluidStone, kernelFluidStone, kernelFluidStone},
	}
}

// kernelFluidEvalRuleChanges is the expected ordered change list per matrix
// item. Slot numbers are the neighbour discriminants: 0 self, 1 above, 2
// below, 3 +x, 4 -x, 5 +z, 6 -z.
func kernelFluidEvalRuleChanges() [][]kernelFluidEvalChange {
	source := uint16(kernelFluidWaterSource)
	return [][]kernelFluidEvalChange{
		{{slot: 2, block: source + 1}},
		{{slot: 3, block: source + 1}, {slot: 4, block: source + 1}, {slot: 5, block: source + 1}, {slot: 6, block: source + 1}},
		{{slot: 0, block: kernelFluidAir}},
		{{slot: 4, block: source + 4}, {slot: 5, block: source + 4}, {slot: 6, block: source + 4}},
		{{slot: 0, block: source}},
		{},
	}
}

// kernelFluidEvalRun evaluates items through the release ABI and returns the
// raw output buffer.
func kernelFluidEvalRun(items []kernelFluidEvalItem) []byte {
	output := make([]byte, len(items)*kernelFluidEvalItemOutputBytes)
	nativeabi.FluidEvalBatch(kernelFluidEvalInput(items), output)
	return output
}

// kernelFluidEvalAssertMatrix checks decoded change records against the shared
// rule matrix and returns the digest over the complete output stream.
func kernelFluidEvalAssertMatrix(t *testing.T, name string, items []kernelFluidEvalItem, want [][]kernelFluidEvalChange) uint64 {
	t.Helper()
	output := kernelFluidEvalRun(items)
	if len(output) != len(items)*kernelFluidEvalItemOutputBytes {
		t.Fatalf("%s: output length got %d want %d", name, len(output), len(items)*kernelFluidEvalItemOutputBytes)
	}
	for index := range items {
		record := output[index*kernelFluidEvalItemOutputBytes : (index+1)*kernelFluidEvalItemOutputBytes]
		got := kernelFluidEvalChanges(record)
		expect := want[index%len(want)]
		if len(got) != len(expect) {
			t.Errorf("%s: item %d change count got %d want %d (%v vs %v)", name, index, len(got), len(expect), got, expect)
			continue
		}
		for entry := range expect {
			if got[entry] != expect[entry] {
				t.Errorf("%s: item %d change %d got %v want %v", name, index, entry, got[entry], expect[entry])
			}
		}
		for entry := len(expect); entry < kernelFluidEvalWriteEntries; entry++ {
			raw := record[entry*3 : entry*3+3]
			if raw[0] != kernelFluidEvalSlotNoWrite || raw[1] != 0 || raw[2] != 0 {
				t.Errorf("%s: item %d unused entry %d got %v want sentinel", name, index, entry, raw)
			}
		}
	}
	return kernelFluidEvalDigest(output)
}

// TestFluidEval records release-ABI observations of `nativeabi.FluidEvalBatch`
// and locks them as ordered change records plus stream digests, so any drift
// between the Go-visible ABI and the pinned expectations fails here first.
// The Rust numerical-migration fluid test pins the same items and digests for
// the typed provider.
func TestFluidEval(t *testing.T) {
	matrix := kernelFluidEvalRuleMatrix()
	want := kernelFluidEvalRuleChanges()

	// Shared rule matrix at its natural batch size: ordered records and the
	// pinned digest over the complete 72-byte stream.
	matrixDigest := kernelFluidEvalAssertMatrix(t, "rule matrix", matrix, want)
	const wantMatrixDigest uint64 = 0x84aca3f0db8d0ae2
	if matrixDigest != wantMatrixDigest {
		t.Errorf("rule matrix digest got %#016x want %#016x", matrixDigest, wantMatrixDigest)
	}

	// Run-to-run determinism over the matrix batch.
	second := kernelFluidEvalRun(matrix)
	if !bytes.Equal(second, kernelFluidEvalRun(matrix)) {
		t.Error("repeated evaluation differs")
	}

	// Batch size 0: the empty batch carries the full header and a zero-length
	// output. The engine must accept it and write nothing.
	empty := kernelFluidEvalInput(nil)
	emptyOutput := make([]byte, 0)
	nativeabi.FluidEvalBatch(empty, emptyOutput)
	if len(emptyOutput) != 0 {
		t.Errorf("empty batch wrote %d bytes", len(emptyOutput))
	}

	// Batch size 1: the first matrix item alone.
	oneDigest := kernelFluidEvalAssertMatrix(t, "single item", matrix[:1], want)
	const wantOneDigest uint64 = 0xa6c17ad39de0f7a8
	if oneDigest != wantOneDigest {
		t.Errorf("single item digest got %#016x want %#016x", oneDigest, wantOneDigest)
	}

	// Batch size 4096: the matrix repeated, with per-item records checked
	// against the same expectations and a pinned stream digest.
	full := make([]kernelFluidEvalItem, 4096)
	for index := range full {
		full[index] = matrix[index%len(matrix)]
	}
	fullDigest := kernelFluidEvalAssertMatrix(t, "full batch", full, want)
	const wantFullDigest uint64 = 0x8227b74d6d5a3f20
	if fullDigest != wantFullDigest {
		t.Errorf("full batch digest got %#016x want %#016x", fullDigest, wantFullDigest)
	}
	if !bytes.Equal(kernelFluidEvalRun(full), kernelFluidEvalRun(full)) {
		t.Error("repeated full-batch evaluation differs")
	}

	// Observation only: the legacy ABI admits a 4097-item batch. The native
	// typed lane rejects that count, so this is recorded as legacy behaviour
	// and must not be read as a native bound.
	over := make([]kernelFluidEvalItem, 4097)
	for index := range over {
		over[index] = matrix[index%len(matrix)]
	}
	overOutput := kernelFluidEvalRun(over)
	if len(overOutput) != 4097*kernelFluidEvalItemOutputBytes {
		t.Errorf("legacy 4097 batch output length got %d want %d", len(overOutput), 4097*kernelFluidEvalItemOutputBytes)
	}

	// Failure conditions surface as Go panics; the engine guarantees the
	// caller-owned output is untouched whenever a request is rejected.
	assertPanic := func(t *testing.T, name string, output []byte, f func()) {
		t.Helper()
		before := bytes.Clone(output)
		defer func() {
			if recover() == nil {
				t.Errorf("expected panic on %s", name)
			}
			if !bytes.Equal(output, before) {
				t.Errorf("%s: rejected request modified the caller-owned output", name)
			}
		}()
		f()
	}
	canaryOutput := func(size int) []byte {
		output := make([]byte, size)
		for index := range output {
			output[index] = 0xA5
		}
		return output
	}

	validInput := kernelFluidEvalInput(matrix[:1])
	validOutputSize := kernelFluidEvalItemOutputBytes

	shortInputOutput := canaryOutput(validOutputSize)
	assertPanic(t, "short input", shortInputOutput, func() {
		nativeabi.FluidEvalBatch(validInput[:len(validInput)-1], shortInputOutput)
	})
	longInputOutput := canaryOutput(validOutputSize)
	assertPanic(t, "long input", longInputOutput, func() {
		nativeabi.FluidEvalBatch(append(bytes.Clone(validInput), 0), longInputOutput)
	})
	shortOutput := canaryOutput(validOutputSize - 1)
	assertPanic(t, "short output", shortOutput, func() {
		nativeabi.FluidEvalBatch(validInput, shortOutput)
	})
	// The layout version word is the framing sentinel of this kernel: an
	// encoding whose version is not the current layout is rejected exactly
	// like a bad magic on the world-lane kernels.
	badVersionOutput := canaryOutput(validOutputSize)
	assertPanic(t, "bad layout version", badVersionOutput, func() {
		bad := bytes.Clone(validInput)
		binary.LittleEndian.PutUint32(bad[0:4], 2)
		nativeabi.FluidEvalBatch(bad, badVersionOutput)
	})

	exportDir := os.Getenv("RUNTIME_ORACLE_EXPORT_DIR")
	if exportDir != "" {
		if err := os.MkdirAll(exportDir, 0755); err != nil {
			t.Fatalf("MkdirAll: %v", err)
		}
		input := kernelFluidEvalInput(matrix)
		output := kernelFluidEvalRun(matrix)
		if err := os.WriteFile(filepath.Join(exportDir, "fluid_eval_input.bin"), input, 0644); err != nil {
			t.Fatalf("WriteFile: %v", err)
		}
		if err := os.WriteFile(filepath.Join(exportDir, "fluid_eval_output.bin"), output, 0644); err != nil {
			t.Fatalf("WriteFile: %v", err)
		}
	}
}
