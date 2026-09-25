package main

import (
	"bytes"
	"encoding/binary"
	"math"
	"os"
	"path/filepath"
	"testing"

	"github.com/channing771/mornlea/packages/shared/nativeabi"
)

// Frozen `MGW1` probe framing on top of the shared 566-byte worldgen header:
// u32 record count, then 16-byte query records (mode u32 + x/y/z i32), with
// 8-byte output records (height i32 + block u16 + reserved u16).
const (
	kernelWorldgenProbeRecordBytes       = 16
	kernelWorldgenProbeOutputRecordBytes = 8
)

// kernelWorldgenProbeRecord is one raw query record. mode: 0 = height at
// (x, z), 1 = terrain block, 2 = base block; mode 0 ignores y.
type kernelWorldgenProbeRecord struct {
	mode    uint32
	x, y, z int32
}

// kernelWorldgenProbeValue is one raw output record: the height field of a
// mode 0 result or the block field of a mode 1/2 result; the unused field and
// the reserved bytes stay zero.
type kernelWorldgenProbeValue struct {
	height int32
	block  uint16
}

// kernelWorldgenProbeInput builds a valid `MGW1` probe request: layout 3,
// distinct material table 1..=15 in wire order, identity permutation, and the
// requested query records. mutatePerm perturbs one permutation entry to prove
// the kernel consumes the caller-supplied permutation.
func kernelWorldgenProbeInput(seed int64, mutatePerm bool, records []kernelWorldgenProbeRecord) []byte {
	input := make([]byte, kernelWorldgenHeaderBytes+4+len(records)*kernelWorldgenProbeRecordBytes)
	copy(input[0:4], "MGW1")
	binary.LittleEndian.PutUint32(input[4:8], 3)
	binary.LittleEndian.PutUint64(input[8:16], uint64(seed))
	minY := int32(-64)
	binary.LittleEndian.PutUint32(input[16:20], uint32(minY))
	binary.LittleEndian.PutUint32(input[20:24], 320)
	for index := 0; index < 15; index++ {
		binary.LittleEndian.PutUint16(input[24+index*2:26+index*2], uint16(index+1))
	}
	for index := 0; index < 512; index++ {
		input[54+index] = byte(index & 255)
	}
	if mutatePerm {
		input[55] = 0x20
	}
	binary.LittleEndian.PutUint32(input[kernelWorldgenHeaderBytes:], uint32(len(records)))
	for index, record := range records {
		offset := kernelWorldgenHeaderBytes + 4 + index*kernelWorldgenProbeRecordBytes
		binary.LittleEndian.PutUint32(input[offset:], record.mode)
		binary.LittleEndian.PutUint32(input[offset+4:], uint32(record.x))
		binary.LittleEndian.PutUint32(input[offset+8:], uint32(record.y))
		binary.LittleEndian.PutUint32(input[offset+12:], uint32(record.z))
	}
	return input
}

// kernelWorldgenProbeHeight reads the height field of output record `index`.
func kernelWorldgenProbeHeight(output []byte, index int) int32 {
	return int32(binary.LittleEndian.Uint32(output[index*kernelWorldgenProbeOutputRecordBytes : index*kernelWorldgenProbeOutputRecordBytes+4]))
}

// kernelWorldgenProbeBlock reads the block field of output record `index`.
func kernelWorldgenProbeBlock(output []byte, index int) uint16 {
	return binary.LittleEndian.Uint16(output[index*kernelWorldgenProbeOutputRecordBytes+4 : index*kernelWorldgenProbeOutputRecordBytes+6])
}

// kernelWorldgenProbeRecords is the shared ordinary-coordinate observation
// batch: all modes at positive and negative X/Z, two height lookups at one
// column that differ only in their caller-supplied Y, a terrain/base pair at a
// seeded decoration column (-64, 64, -64: sea water below sea level over a
// below-sea surface), and a terrain cell (39, 0, -57) whose block id depends
// on the seed.
func kernelWorldgenProbeRecords() []kernelWorldgenProbeRecord {
	return []kernelWorldgenProbeRecord{
		{mode: 0, x: 3, z: 5},
		{mode: 1, x: 3, y: 0, z: 5},
		{mode: 2, x: 3, y: 0, z: 5},
		{mode: 0, x: -7, z: -13},
		{mode: 1, x: -7, y: -64, z: -13},
		{mode: 2, x: -7, y: -64, z: -13},
		{mode: 1, x: -7, y: 319, z: -13},
		{mode: 2, x: -7, y: 319, z: -13},
		{mode: 0, x: 20, y: 12345, z: -40},
		{mode: 0, x: 20, y: -12345, z: -40},
		{mode: 1, x: -64, y: 64, z: -64},
		{mode: 2, x: -64, y: 64, z: -64},
		{mode: 1, x: 39, y: 0, z: -57},
	}
}

// kernelWorldgenProbeExtremes is the signed-extreme observation batch: point
// probes at i32 extremes, including base probes whose oak-tree fringe crosses
// i32::MIN and i32::MAX. Values are frozen release-ABI observations; full
// debug/release parity at signed extremes is checked at the adapter layer, so
// these pins are Go-side observations only.
func kernelWorldgenProbeExtremes() []kernelWorldgenProbeRecord {
	return []kernelWorldgenProbeRecord{
		{mode: 0, x: math.MinInt32, z: math.MinInt32},
		{mode: 0, x: math.MaxInt32, z: math.MaxInt32},
		{mode: 1, x: math.MinInt32, y: 0, z: math.MinInt32},
		{mode: 2, x: math.MinInt32, y: 64, z: math.MinInt32},
		{mode: 2, x: math.MinInt32 + 2, y: 64, z: math.MinInt32 + 2},
		{mode: 2, x: math.MaxInt32 - 1, y: 64, z: math.MaxInt32 - 1},
		{mode: 1, x: math.MaxInt32, y: 0, z: math.MaxInt32},
	}
}

// TestKernelWorldgenProbe records release-ABI observations of
// `nativeabi.WorldgenProbe` and locks them as deterministic per-query values,
// so any drift between the Go-visible ABI and the pinned expectations fails
// here first. The Rust numerical-migration probe test pins the same values for
// the typed provider.
func TestKernelWorldgenProbe(t *testing.T) {
	// Pinned per-query observations and FNV-1a-64 digests over the complete
	// 8-byte-per-record output, recorded from the release ABI.
	cases := []struct {
		name       string
		seed       int64
		mutatePerm bool
		records    []kernelWorldgenProbeRecord
		want       []kernelWorldgenProbeValue
		wantDigest uint64
	}{
		{
			name:    "seed 0 ordinary",
			seed:    0,
			records: kernelWorldgenProbeRecords(),
			want: []kernelWorldgenProbeValue{
				{height: 67}, {block: 2}, {block: 2},
				{height: 56}, {block: 5}, {block: 5},
				{block: 1}, {block: 1},
				{height: 53}, {height: 53},
				{block: 1}, {block: 14},
				{block: 11},
			},
			wantDigest: 0x57d1f3def5022eba,
		},
		{
			name:    "seed 1 ordinary",
			seed:    1,
			records: kernelWorldgenProbeRecords(),
			want: []kernelWorldgenProbeValue{
				{height: 67}, {block: 2}, {block: 2},
				{height: 56}, {block: 5}, {block: 5},
				{block: 1}, {block: 1},
				{height: 53}, {height: 53},
				{block: 1}, {block: 14},
				{block: 2},
			},
			wantDigest: 0xf7f73fa4998a06c3,
		},
		{
			name:    "seed 0 signed extremes",
			seed:    0,
			records: kernelWorldgenProbeExtremes(),
			want: []kernelWorldgenProbeValue{
				{height: 64}, {height: 63},
				{block: 2}, {block: 4}, {block: 3}, {block: 14}, {block: 2},
			},
			wantDigest: 0x72119860a6d5d143,
		},
		{
			name:       "seed 0 ordinary mutated permutation",
			seed:       0,
			mutatePerm: true,
			records:    kernelWorldgenProbeRecords(),
			want: []kernelWorldgenProbeValue{
				{height: 67}, {block: 2}, {block: 2},
				{height: 56}, {block: 5}, {block: 5},
				{block: 1}, {block: 1},
				{height: 55}, {height: 55},
				{block: 1}, {block: 14},
				{block: 11},
			},
			wantDigest: 0x254b481755497a7a,
		},
	}
	digests := map[string]uint64{}
	for _, tc := range cases {
		input := kernelWorldgenProbeInput(tc.seed, tc.mutatePerm, tc.records)
		first := make([]byte, len(tc.records)*kernelWorldgenProbeOutputRecordBytes)
		second := make([]byte, len(tc.records)*kernelWorldgenProbeOutputRecordBytes)
		nativeabi.WorldgenProbe(input, first)
		nativeabi.WorldgenProbe(input, second)
		if !bytes.Equal(first, second) {
			t.Errorf("%s: repeated probing differs", tc.name)
		}
		for index, want := range tc.want {
			if got := kernelWorldgenProbeHeight(first, index); got != want.height {
				t.Errorf("%s: record %d height got %d want %d", tc.name, index, got, want.height)
			}
			if got := kernelWorldgenProbeBlock(first, index); got != want.block {
				t.Errorf("%s: record %d block got %d want %d", tc.name, index, got, want.block)
			}
			reserved := first[index*kernelWorldgenProbeOutputRecordBytes+6 : index*kernelWorldgenProbeOutputRecordBytes+8]
			if reserved[0] != 0 || reserved[1] != 0 {
				t.Errorf("%s: record %d reserved bytes got %v want zero", tc.name, index, reserved)
			}
		}
		gotDigest := kernelWorldgenChunkDigest(first)
		if gotDigest != tc.wantDigest {
			t.Errorf("%s: digest got %#016x want %#016x", tc.name, gotDigest, tc.wantDigest)
		}
		digests[tc.name] = gotDigest
	}
	// The caller-supplied permutation is a real kernel input: perturbing one
	// entry must change the probed value sequence.
	if digests["seed 0 ordinary"] == digests["seed 0 ordinary mutated permutation"] {
		t.Error("permutation change did not affect the probed values")
	}

	// The raw mode 0 record carries a Y coordinate that the kernel ignores:
	// two height records at one column with different Y bytes must agree.
	yPair := []kernelWorldgenProbeRecord{
		{mode: 0, x: 20, y: 12345, z: -40},
		{mode: 0, x: 20, y: -12345, z: -40},
	}
	yOutput := make([]byte, 2*kernelWorldgenProbeOutputRecordBytes)
	nativeabi.WorldgenProbe(kernelWorldgenProbeInput(0, false, yPair), yOutput)
	if kernelWorldgenProbeHeight(yOutput, 0) != kernelWorldgenProbeHeight(yOutput, 1) {
		t.Error("mode 0 must ignore the caller-supplied y")
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
	oneRecord := []kernelWorldgenProbeRecord{{mode: 2, x: 3, y: 0, z: 5}}
	validInput := kernelWorldgenProbeInput(0, false, oneRecord)

	emptyOutput := canaryOutput(kernelWorldgenProbeOutputRecordBytes)
	assertPanic(t, "empty record list", emptyOutput, func() {
		nativeabi.WorldgenProbe(kernelWorldgenProbeInput(0, false, nil), emptyOutput)
	})
	overCountOutput := canaryOutput(65 * kernelWorldgenProbeOutputRecordBytes)
	assertPanic(t, "65 records", overCountOutput, func() {
		records := make([]kernelWorldgenProbeRecord, 65)
		for index := range records {
			records[index] = kernelWorldgenProbeRecord{mode: 0, x: int32(index), z: 0}
		}
		nativeabi.WorldgenProbe(kernelWorldgenProbeInput(0, false, records), overCountOutput)
	})
	shortInputOutput := canaryOutput(kernelWorldgenProbeOutputRecordBytes)
	assertPanic(t, "short input", shortInputOutput, func() {
		nativeabi.WorldgenProbe(validInput[:len(validInput)-1], shortInputOutput)
	})
	shortOutput := canaryOutput(kernelWorldgenProbeOutputRecordBytes - 1)
	assertPanic(t, "short output", shortOutput, func() {
		nativeabi.WorldgenProbe(validInput, shortOutput)
	})
	longOutput := canaryOutput(kernelWorldgenProbeOutputRecordBytes + 1)
	assertPanic(t, "long output", longOutput, func() {
		nativeabi.WorldgenProbe(validInput, longOutput)
	})
	badMagicOutput := canaryOutput(kernelWorldgenProbeOutputRecordBytes)
	assertPanic(t, "bad magic", badMagicOutput, func() {
		bad := bytes.Clone(validInput)
		bad[0] = 'X'
		nativeabi.WorldgenProbe(bad, badMagicOutput)
	})
	invalidModeOutput := canaryOutput(kernelWorldgenProbeOutputRecordBytes)
	assertPanic(t, "invalid mode", invalidModeOutput, func() {
		bad := bytes.Clone(validInput)
		binary.LittleEndian.PutUint32(bad[kernelWorldgenHeaderBytes+4:kernelWorldgenHeaderBytes+8], 3)
		nativeabi.WorldgenProbe(bad, invalidModeOutput)
	})
	// The mode word is a closed set 0..=2: an encoding whose unused high
	// bytes are set is not a reserved extension point and must be rejected
	// like any other out-of-range mode.
	highModeOutput := canaryOutput(kernelWorldgenProbeOutputRecordBytes)
	assertPanic(t, "out-of-range mode word", highModeOutput, func() {
		bad := bytes.Clone(validInput)
		binary.LittleEndian.PutUint32(bad[kernelWorldgenHeaderBytes+4:kernelWorldgenHeaderBytes+8], 0x01000002)
		nativeabi.WorldgenProbe(bad, highModeOutput)
	})

	exportDir := os.Getenv("RUNTIME_ORACLE_EXPORT_DIR")
	if exportDir != "" {
		if err := os.MkdirAll(exportDir, 0755); err != nil {
			t.Fatalf("MkdirAll: %v", err)
		}
		input := kernelWorldgenProbeInput(0, false, kernelWorldgenProbeRecords())
		output := make([]byte, len(kernelWorldgenProbeRecords())*kernelWorldgenProbeOutputRecordBytes)
		nativeabi.WorldgenProbe(input, output)
		if err := os.WriteFile(filepath.Join(exportDir, "worldgen_probe_input.bin"), input, 0644); err != nil {
			t.Fatalf("WriteFile: %v", err)
		}
		if err := os.WriteFile(filepath.Join(exportDir, "worldgen_probe_output.bin"), output, 0644); err != nil {
			t.Fatalf("WriteFile: %v", err)
		}
	}
}
