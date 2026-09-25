package main

import (
	"bytes"
	"encoding/binary"
	"hash/fnv"
	"math"
	"os"
	"path/filepath"
	"testing"

	"github.com/channing771/mornlea/packages/shared/nativeabi"
)

// Frozen `MGW1` chunk frame: 566-byte shared header + chunk coordinates in,
// 98304 little-endian u16 cells out.
const (
	kernelWorldgenHeaderBytes      = 566
	kernelWorldgenChunkInputBytes  = kernelWorldgenHeaderBytes + 8
	kernelWorldgenChunkOutputBytes = 98304 * 2
)

// kernelWorldgenChunkInput builds a valid `MGW1` chunk request: layout 3,
// distinct material table 1..=15 in wire order, identity permutation, and the
// requested chunk coordinates. mutatePerm perturbs one permutation entry to
// prove the kernel consumes the caller-supplied permutation.
func kernelWorldgenChunkInput(seed int64, chunkX, chunkZ int32, mutatePerm bool) []byte {
	input := make([]byte, kernelWorldgenChunkInputBytes)
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
	binary.LittleEndian.PutUint32(input[566:570], uint32(chunkX))
	binary.LittleEndian.PutUint32(input[570:574], uint32(chunkZ))
	return input
}

// kernelWorldgenChunkDigest fingerprints the full 98304-cell dense output with
// FNV-1a 64-bit. The Rust numerical-migration worldgen test implements the
// same algorithm so both sides pin one shared release-ABI digest.
func kernelWorldgenChunkDigest(output []byte) uint64 {
	hash := fnv.New64a()
	hash.Write(output)
	return hash.Sum64()
}

// TestKernelWorldgenChunk records release-ABI observations of
// `nativeabi.WorldgenChunk` and locks them as deterministic digests over the
// complete dense output, so any drift between the Go-visible ABI and the
// pinned expectations fails here first.
func TestKernelWorldgenChunk(t *testing.T) {
	// Pinned FNV-1a-64 digests per observation case.
	cases := []struct {
		name       string
		seed       int64
		chunkX     int32
		chunkZ     int32
		mutatePerm bool
		wantDigest uint64
	}{
		{name: "seed 0 at chunk (0,0)", seed: 0, chunkX: 0, chunkZ: 0, wantDigest: 0x6f674aa8baa4ea79},
		{name: "seed 0 at chunk (-1,2)", seed: 0, chunkX: -1, chunkZ: 2, wantDigest: 0xcfd04d932a54cb6b},
		{name: "seed 1 at chunk (0,0)", seed: 1, chunkX: 0, chunkZ: 0, wantDigest: 0x07c07b897f0c15e0},
		{name: "seed 1 at chunk (-1,2)", seed: 1, chunkX: -1, chunkZ: 2, wantDigest: 0x0dafd95ba0935704},
		{name: "seed -1 at chunk (0,0)", seed: -1, chunkX: 0, chunkZ: 0, wantDigest: 0x4e46cd6ddae06148},
		{name: "seed -1 at chunk (-1,2)", seed: -1, chunkX: -1, chunkZ: 2, wantDigest: 0x4821e20d75d212ea},
		{
			name:       "seed 0 at signed low extreme chunk",
			seed:       0,
			chunkX:     math.MinInt32 / 16,
			chunkZ:     math.MinInt32 / 16,
			wantDigest: 0x8d3e20fa7599b5c6,
		},
		{
			name:       "seed 0 at signed high extreme chunk",
			seed:       0,
			chunkX:     (math.MaxInt32 - 15) / 16,
			chunkZ:     (math.MaxInt32 - 15) / 16,
			wantDigest: 0x7f8806f3505dfbce,
		},
		{
			name:       "seed 0 at chunk (0,0) with mutated permutation",
			seed:       0,
			chunkX:     0,
			chunkZ:     0,
			mutatePerm: true,
			wantDigest: 0x072b63b2bdf3922b,
		},
	}
	for _, tc := range cases {
		input := kernelWorldgenChunkInput(tc.seed, tc.chunkX, tc.chunkZ, tc.mutatePerm)
		first := make([]byte, kernelWorldgenChunkOutputBytes)
		second := make([]byte, kernelWorldgenChunkOutputBytes)
		nativeabi.WorldgenChunk(input, first)
		nativeabi.WorldgenChunk(input, second)
		if !bytes.Equal(first, second) {
			t.Errorf("%s: repeated generation differs", tc.name)
		}
		if got := kernelWorldgenChunkDigest(first); got != tc.wantDigest {
			t.Errorf("%s: digest got %#016x want %#016x", tc.name, got, tc.wantDigest)
		}
	}

	// The caller-supplied permutation is a real kernel input: perturbing one
	// entry must change the generated chunk.
	plain := make([]byte, kernelWorldgenChunkOutputBytes)
	mutated := make([]byte, kernelWorldgenChunkOutputBytes)
	nativeabi.WorldgenChunk(kernelWorldgenChunkInput(0, 0, 0, false), plain)
	nativeabi.WorldgenChunk(kernelWorldgenChunkInput(0, 0, 0, true), mutated)
	if bytes.Equal(plain, mutated) {
		t.Error("permutation change did not affect the generated chunk")
	}

	// Cell observations from the seed 0 / chunk (0,0) run, compared bitwise by
	// the Rust numerical-migration worldgen test. Generation contract: the
	// bottom Y layer is entirely bedrock, material table entry 5 here.
	output := make([]byte, kernelWorldgenChunkOutputBytes)
	nativeabi.WorldgenChunk(kernelWorldgenChunkInput(0, 0, 0, false), output)
	for index := 0; index < 256; index++ {
		if got := binary.LittleEndian.Uint16(output[index*2 : index*2+2]); got != 5 {
			t.Fatalf("bottom layer cell %d got %d want bedrock 5", index, got)
		}
	}
	cells := []struct {
		index int
		want  uint16
	}{
		{index: 16467, want: 2},
		{index: 32768, want: 4},
		{index: 98303, want: 1},
	}
	for _, cell := range cells {
		got := binary.LittleEndian.Uint16(output[cell.index*2 : cell.index*2+2])
		if got != cell.want {
			t.Errorf("cell %d got %d want %d", cell.index, got, cell.want)
		}
	}

	// Failure conditions surface as Go panics; the engine guarantees the
	// caller-owned output is untouched whenever a request is rejected.
	assertPanic := func(t *testing.T, name string, f func()) {
		t.Helper()
		defer func() {
			if recover() == nil {
				t.Errorf("expected panic on %s", name)
			}
		}()
		f()
	}
	validInput := kernelWorldgenChunkInput(0, 0, 0, false)
	validOutput := make([]byte, kernelWorldgenChunkOutputBytes)
	assertPanic(t, "short input", func() {
		nativeabi.WorldgenChunk(validInput[:len(validInput)-1], validOutput)
	})
	assertPanic(t, "short output", func() {
		nativeabi.WorldgenChunk(validInput, validOutput[:len(validOutput)-1])
	})
	assertPanic(t, "long output", func() {
		nativeabi.WorldgenChunk(validInput, make([]byte, kernelWorldgenChunkOutputBytes+1))
	})
	assertPanic(t, "bad magic", func() {
		bad := bytes.Clone(validInput)
		bad[0] = 'X'
		nativeabi.WorldgenChunk(bad, validOutput)
	})
	assertPanic(t, "duplicate material", func() {
		bad := bytes.Clone(validInput)
		// Dirt takes stone's value: the alias pair is not the water-air
		// gate exemption and must be rejected.
		binary.LittleEndian.PutUint16(bad[28:30], 2)
		nativeabi.WorldgenChunk(bad, validOutput)
	})
	assertPanic(t, "short grass alias", func() {
		bad := bytes.Clone(validInput)
		// short_grass takes water's value; it writes into the chunk and is
		// never exempt from the uniqueness rule.
		binary.LittleEndian.PutUint16(bad[52:54], 14)
		nativeabi.WorldgenChunk(bad, validOutput)
	})
	assertPanic(t, "legacy layout", func() {
		bad := bytes.Clone(validInput)
		binary.LittleEndian.PutUint32(bad[4:8], 2)
		nativeabi.WorldgenChunk(bad, validOutput)
	})
	assertPanic(t, "wrong min y", func() {
		bad := bytes.Clone(validInput)
		badMinY := int32(-32)
		binary.LittleEndian.PutUint32(bad[16:20], uint32(badMinY))
		nativeabi.WorldgenChunk(bad, validOutput)
	})

	exportDir := os.Getenv("RUNTIME_ORACLE_EXPORT_DIR")
	if exportDir != "" {
		if err := os.MkdirAll(exportDir, 0755); err != nil {
			t.Fatalf("MkdirAll: %v", err)
		}
		if err := os.WriteFile(filepath.Join(exportDir, "worldgen_chunk_input.bin"), validInput, 0644); err != nil {
			t.Fatalf("WriteFile: %v", err)
		}
		if err := os.WriteFile(filepath.Join(exportDir, "worldgen_chunk_output.bin"), output, 0644); err != nil {
			t.Fatalf("WriteFile: %v", err)
		}
	}
}
