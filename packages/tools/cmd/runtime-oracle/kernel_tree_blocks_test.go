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

// kernelTreeBlocksRecord is one decoded runtime tree record: block offsets
// relative to the root cell and the block id. Records arrive in the frozen
// dy/dz/dx emission order; ids are 17 (oak log) and 19 (leaves).
type kernelTreeBlocksRecord struct {
	dx, dy, dz int8
	block      uint16
}

// kernelTreeBlocksInput builds a valid 28-byte `MTB1` runtime tree request:
// magic, layout 1, world seed and the tree root coordinates.
func kernelTreeBlocksInput(seed int64, x, y, z int32) []byte {
	input := make([]byte, nativeabi.TreeBlocksInputBytes)
	copy(input[0:4], "MTB1")
	binary.LittleEndian.PutUint32(input[4:8], 1)
	binary.LittleEndian.PutUint64(input[8:16], uint64(seed))
	binary.LittleEndian.PutUint32(input[16:20], uint32(x))
	binary.LittleEndian.PutUint32(input[20:24], uint32(y))
	binary.LittleEndian.PutUint32(input[24:28], uint32(z))
	return input
}

// kernelTreeBlocksDecode decodes the ordered record list from a successful
// output buffer and reports any non-zero per-record reserved bytes.
func kernelTreeBlocksDecode(t *testing.T, name string, output []byte, count int) []kernelTreeBlocksRecord {
	t.Helper()
	records := make([]kernelTreeBlocksRecord, count)
	for index := 0; index < count; index++ {
		base := nativeabi.TreeBlocksCountBytes + index*nativeabi.TreeBlocksRecordBytes
		if output[base+3] != 0 || output[base+6] != 0 || output[base+7] != 0 {
			t.Errorf("%s: record %d reserved bytes must stay zero", name, index)
		}
		records[index] = kernelTreeBlocksRecord{
			dx:    int8(output[base]),
			dy:    int8(output[base+1]),
			dz:    int8(output[base+2]),
			block: binary.LittleEndian.Uint16(output[base+4 : base+6]),
		}
	}
	return records
}

// TestKernelTreeBlocks records release-ABI observations of
// `nativeabi.TreeBlocks` and locks them as deterministic ordered record
// sequences, so any drift between the Go-visible ABI and the pinned
// expectations fails here first. The Rust numerical-migration tree test pins
// the same ordered tuples for the typed provider.
func TestKernelTreeBlocks(t *testing.T) {
	// Pinned ordered observations per request: seeds and roots cover trunk
	// heights five, six and seven, a fluffy crown, both world-height edges,
	// ordinary positive/negative coordinates and one representable root near
	// each signed horizontal extreme.
	cases := []struct {
		name    string
		seed    int64
		x, y, z int32
		want    []kernelTreeBlocksRecord
	}{
		{
			name: "seed 0 ordinary positive root",
			seed: 0,
			x:    3,
			y:    -64,
			z:    5,
			want: []kernelTreeBlocksRecord{
				{dx: 0, dy: 0, dz: 0, block: 17},
				{dx: 0, dy: 1, dz: 0, block: 17},
				{dx: -1, dy: 2, dz: -2, block: 19},
				{dx: 0, dy: 2, dz: -2, block: 19},
				{dx: 1, dy: 2, dz: -2, block: 19},
				{dx: -2, dy: 2, dz: -1, block: 19},
				{dx: -1, dy: 2, dz: -1, block: 19},
				{dx: 0, dy: 2, dz: -1, block: 19},
				{dx: 1, dy: 2, dz: -1, block: 19},
				{dx: 2, dy: 2, dz: -1, block: 19},
				{dx: -2, dy: 2, dz: 0, block: 19},
				{dx: -1, dy: 2, dz: 0, block: 19},
				{dx: 0, dy: 2, dz: 0, block: 17},
				{dx: 1, dy: 2, dz: 0, block: 19},
				{dx: 2, dy: 2, dz: 0, block: 19},
				{dx: -2, dy: 2, dz: 1, block: 19},
				{dx: -1, dy: 2, dz: 1, block: 19},
				{dx: 0, dy: 2, dz: 1, block: 19},
				{dx: 1, dy: 2, dz: 1, block: 19},
				{dx: 2, dy: 2, dz: 1, block: 19},
				{dx: -1, dy: 2, dz: 2, block: 19},
				{dx: 0, dy: 2, dz: 2, block: 19},
				{dx: 1, dy: 2, dz: 2, block: 19},
				{dx: -1, dy: 3, dz: -2, block: 19},
				{dx: 0, dy: 3, dz: -2, block: 19},
				{dx: 1, dy: 3, dz: -2, block: 19},
				{dx: -2, dy: 3, dz: -1, block: 19},
				{dx: -1, dy: 3, dz: -1, block: 19},
				{dx: 0, dy: 3, dz: -1, block: 19},
				{dx: 1, dy: 3, dz: -1, block: 19},
				{dx: 2, dy: 3, dz: -1, block: 19},
				{dx: -2, dy: 3, dz: 0, block: 19},
				{dx: -1, dy: 3, dz: 0, block: 19},
				{dx: 0, dy: 3, dz: 0, block: 17},
				{dx: 1, dy: 3, dz: 0, block: 19},
				{dx: 2, dy: 3, dz: 0, block: 19},
				{dx: -2, dy: 3, dz: 1, block: 19},
				{dx: -1, dy: 3, dz: 1, block: 19},
				{dx: 0, dy: 3, dz: 1, block: 19},
				{dx: 1, dy: 3, dz: 1, block: 19},
				{dx: 2, dy: 3, dz: 1, block: 19},
				{dx: -1, dy: 3, dz: 2, block: 19},
				{dx: 0, dy: 3, dz: 2, block: 19},
				{dx: 1, dy: 3, dz: 2, block: 19},
				{dx: -1, dy: 4, dz: -1, block: 19},
				{dx: 0, dy: 4, dz: -1, block: 19},
				{dx: 1, dy: 4, dz: -1, block: 19},
				{dx: -1, dy: 4, dz: 0, block: 19},
				{dx: 0, dy: 4, dz: 0, block: 17},
				{dx: 1, dy: 4, dz: 0, block: 19},
				{dx: -1, dy: 4, dz: 1, block: 19},
				{dx: 0, dy: 4, dz: 1, block: 19},
				{dx: 1, dy: 4, dz: 1, block: 19},
				{dx: 0, dy: 5, dz: -1, block: 19},
				{dx: -1, dy: 5, dz: 0, block: 19},
				{dx: 0, dy: 5, dz: 0, block: 19},
				{dx: 1, dy: 5, dz: 0, block: 19},
				{dx: 0, dy: 5, dz: 1, block: 19},
				{dx: 0, dy: 6, dz: -1, block: 19},
				{dx: -1, dy: 6, dz: 0, block: 19},
				{dx: 0, dy: 6, dz: 0, block: 19},
				{dx: 1, dy: 6, dz: 0, block: 19},
				{dx: 0, dy: 6, dz: 1, block: 19},
			},
		},
		{
			name: "seed 1 ordinary negative root",
			seed: 1,
			x:    -7,
			y:    311,
			z:    -13,
			want: []kernelTreeBlocksRecord{
				{dx: 0, dy: 0, dz: 0, block: 17},
				{dx: 0, dy: 1, dz: 0, block: 17},
				{dx: 0, dy: 2, dz: 0, block: 17},
				{dx: 0, dy: 3, dz: 0, block: 17},
				{dx: -1, dy: 4, dz: -2, block: 19},
				{dx: 0, dy: 4, dz: -2, block: 19},
				{dx: 1, dy: 4, dz: -2, block: 19},
				{dx: -2, dy: 4, dz: -1, block: 19},
				{dx: -1, dy: 4, dz: -1, block: 19},
				{dx: 0, dy: 4, dz: -1, block: 19},
				{dx: 1, dy: 4, dz: -1, block: 19},
				{dx: 2, dy: 4, dz: -1, block: 19},
				{dx: -2, dy: 4, dz: 0, block: 19},
				{dx: -1, dy: 4, dz: 0, block: 19},
				{dx: 0, dy: 4, dz: 0, block: 17},
				{dx: 1, dy: 4, dz: 0, block: 19},
				{dx: 2, dy: 4, dz: 0, block: 19},
				{dx: -2, dy: 4, dz: 1, block: 19},
				{dx: -1, dy: 4, dz: 1, block: 19},
				{dx: 0, dy: 4, dz: 1, block: 19},
				{dx: 1, dy: 4, dz: 1, block: 19},
				{dx: 2, dy: 4, dz: 1, block: 19},
				{dx: -1, dy: 4, dz: 2, block: 19},
				{dx: 0, dy: 4, dz: 2, block: 19},
				{dx: 1, dy: 4, dz: 2, block: 19},
				{dx: -1, dy: 5, dz: -2, block: 19},
				{dx: 0, dy: 5, dz: -2, block: 19},
				{dx: 1, dy: 5, dz: -2, block: 19},
				{dx: -2, dy: 5, dz: -1, block: 19},
				{dx: -1, dy: 5, dz: -1, block: 19},
				{dx: 0, dy: 5, dz: -1, block: 19},
				{dx: 1, dy: 5, dz: -1, block: 19},
				{dx: 2, dy: 5, dz: -1, block: 19},
				{dx: -2, dy: 5, dz: 0, block: 19},
				{dx: -1, dy: 5, dz: 0, block: 19},
				{dx: 0, dy: 5, dz: 0, block: 17},
				{dx: 1, dy: 5, dz: 0, block: 19},
				{dx: 2, dy: 5, dz: 0, block: 19},
				{dx: -2, dy: 5, dz: 1, block: 19},
				{dx: -1, dy: 5, dz: 1, block: 19},
				{dx: 0, dy: 5, dz: 1, block: 19},
				{dx: 1, dy: 5, dz: 1, block: 19},
				{dx: 2, dy: 5, dz: 1, block: 19},
				{dx: -1, dy: 5, dz: 2, block: 19},
				{dx: 0, dy: 5, dz: 2, block: 19},
				{dx: 1, dy: 5, dz: 2, block: 19},
				{dx: -1, dy: 6, dz: -1, block: 19},
				{dx: 0, dy: 6, dz: -1, block: 19},
				{dx: 1, dy: 6, dz: -1, block: 19},
				{dx: -1, dy: 6, dz: 0, block: 19},
				{dx: 0, dy: 6, dz: 0, block: 17},
				{dx: 1, dy: 6, dz: 0, block: 19},
				{dx: -1, dy: 6, dz: 1, block: 19},
				{dx: 0, dy: 6, dz: 1, block: 19},
				{dx: 1, dy: 6, dz: 1, block: 19},
				{dx: 0, dy: 7, dz: -1, block: 19},
				{dx: -1, dy: 7, dz: 0, block: 19},
				{dx: 0, dy: 7, dz: 0, block: 19},
				{dx: 1, dy: 7, dz: 0, block: 19},
				{dx: 0, dy: 7, dz: 1, block: 19},
				{dx: 0, dy: 8, dz: -1, block: 19},
				{dx: -1, dy: 8, dz: 0, block: 19},
				{dx: 0, dy: 8, dz: 0, block: 19},
				{dx: 1, dy: 8, dz: 0, block: 19},
				{dx: 0, dy: 8, dz: 1, block: 19},
			},
		},
		{
			name: "seed 0 signed low extreme root",
			seed: 0,
			x:    math.MinInt32 + 2,
			y:    64,
			z:    math.MinInt32 + 2,
			want: []kernelTreeBlocksRecord{
				{dx: 0, dy: 0, dz: 0, block: 17},
				{dx: 0, dy: 1, dz: 0, block: 17},
				{dx: 0, dy: 2, dz: 0, block: 17},
				{dx: 0, dy: 3, dz: 0, block: 17},
				{dx: -1, dy: 4, dz: -2, block: 19},
				{dx: 0, dy: 4, dz: -2, block: 19},
				{dx: 1, dy: 4, dz: -2, block: 19},
				{dx: -2, dy: 4, dz: -1, block: 19},
				{dx: -1, dy: 4, dz: -1, block: 19},
				{dx: 0, dy: 4, dz: -1, block: 19},
				{dx: 1, dy: 4, dz: -1, block: 19},
				{dx: 2, dy: 4, dz: -1, block: 19},
				{dx: -2, dy: 4, dz: 0, block: 19},
				{dx: -1, dy: 4, dz: 0, block: 19},
				{dx: 0, dy: 4, dz: 0, block: 17},
				{dx: 1, dy: 4, dz: 0, block: 19},
				{dx: 2, dy: 4, dz: 0, block: 19},
				{dx: -2, dy: 4, dz: 1, block: 19},
				{dx: -1, dy: 4, dz: 1, block: 19},
				{dx: 0, dy: 4, dz: 1, block: 19},
				{dx: 1, dy: 4, dz: 1, block: 19},
				{dx: 2, dy: 4, dz: 1, block: 19},
				{dx: -1, dy: 4, dz: 2, block: 19},
				{dx: 0, dy: 4, dz: 2, block: 19},
				{dx: 1, dy: 4, dz: 2, block: 19},
				{dx: -1, dy: 5, dz: -2, block: 19},
				{dx: 0, dy: 5, dz: -2, block: 19},
				{dx: 1, dy: 5, dz: -2, block: 19},
				{dx: -2, dy: 5, dz: -1, block: 19},
				{dx: -1, dy: 5, dz: -1, block: 19},
				{dx: 0, dy: 5, dz: -1, block: 19},
				{dx: 1, dy: 5, dz: -1, block: 19},
				{dx: 2, dy: 5, dz: -1, block: 19},
				{dx: -2, dy: 5, dz: 0, block: 19},
				{dx: -1, dy: 5, dz: 0, block: 19},
				{dx: 0, dy: 5, dz: 0, block: 17},
				{dx: 1, dy: 5, dz: 0, block: 19},
				{dx: 2, dy: 5, dz: 0, block: 19},
				{dx: -2, dy: 5, dz: 1, block: 19},
				{dx: -1, dy: 5, dz: 1, block: 19},
				{dx: 0, dy: 5, dz: 1, block: 19},
				{dx: 1, dy: 5, dz: 1, block: 19},
				{dx: 2, dy: 5, dz: 1, block: 19},
				{dx: -1, dy: 5, dz: 2, block: 19},
				{dx: 0, dy: 5, dz: 2, block: 19},
				{dx: 1, dy: 5, dz: 2, block: 19},
				{dx: -1, dy: 6, dz: -1, block: 19},
				{dx: 0, dy: 6, dz: -1, block: 19},
				{dx: 1, dy: 6, dz: -1, block: 19},
				{dx: -1, dy: 6, dz: 0, block: 19},
				{dx: 0, dy: 6, dz: 0, block: 17},
				{dx: 1, dy: 6, dz: 0, block: 19},
				{dx: -1, dy: 6, dz: 1, block: 19},
				{dx: 0, dy: 6, dz: 1, block: 19},
				{dx: 1, dy: 6, dz: 1, block: 19},
				{dx: 0, dy: 7, dz: -1, block: 19},
				{dx: -1, dy: 7, dz: 0, block: 19},
				{dx: 0, dy: 7, dz: 0, block: 19},
				{dx: 1, dy: 7, dz: 0, block: 19},
				{dx: 0, dy: 7, dz: 1, block: 19},
				{dx: 0, dy: 8, dz: -1, block: 19},
				{dx: -1, dy: 8, dz: 0, block: 19},
				{dx: 0, dy: 8, dz: 0, block: 19},
				{dx: 1, dy: 8, dz: 0, block: 19},
				{dx: 0, dy: 8, dz: 1, block: 19},
			},
		},
		{
			name: "seed 0 signed high extreme root",
			seed: 0,
			x:    math.MaxInt32 - 2,
			y:    64,
			z:    math.MaxInt32 - 2,
			want: []kernelTreeBlocksRecord{
				{dx: 0, dy: 0, dz: 0, block: 17},
				{dx: 0, dy: 1, dz: 0, block: 17},
				{dx: 0, dy: 2, dz: 0, block: 17},
				{dx: 0, dy: 3, dz: 0, block: 17},
				{dx: -1, dy: 4, dz: -2, block: 19},
				{dx: 0, dy: 4, dz: -2, block: 19},
				{dx: 1, dy: 4, dz: -2, block: 19},
				{dx: -2, dy: 4, dz: -1, block: 19},
				{dx: -1, dy: 4, dz: -1, block: 19},
				{dx: 0, dy: 4, dz: -1, block: 19},
				{dx: 1, dy: 4, dz: -1, block: 19},
				{dx: 2, dy: 4, dz: -1, block: 19},
				{dx: -2, dy: 4, dz: 0, block: 19},
				{dx: -1, dy: 4, dz: 0, block: 19},
				{dx: 0, dy: 4, dz: 0, block: 17},
				{dx: 1, dy: 4, dz: 0, block: 19},
				{dx: 2, dy: 4, dz: 0, block: 19},
				{dx: -2, dy: 4, dz: 1, block: 19},
				{dx: -1, dy: 4, dz: 1, block: 19},
				{dx: 0, dy: 4, dz: 1, block: 19},
				{dx: 1, dy: 4, dz: 1, block: 19},
				{dx: 2, dy: 4, dz: 1, block: 19},
				{dx: -1, dy: 4, dz: 2, block: 19},
				{dx: 0, dy: 4, dz: 2, block: 19},
				{dx: 1, dy: 4, dz: 2, block: 19},
				{dx: -1, dy: 5, dz: -2, block: 19},
				{dx: 0, dy: 5, dz: -2, block: 19},
				{dx: 1, dy: 5, dz: -2, block: 19},
				{dx: -2, dy: 5, dz: -1, block: 19},
				{dx: -1, dy: 5, dz: -1, block: 19},
				{dx: 0, dy: 5, dz: -1, block: 19},
				{dx: 1, dy: 5, dz: -1, block: 19},
				{dx: 2, dy: 5, dz: -1, block: 19},
				{dx: -2, dy: 5, dz: 0, block: 19},
				{dx: -1, dy: 5, dz: 0, block: 19},
				{dx: 0, dy: 5, dz: 0, block: 17},
				{dx: 1, dy: 5, dz: 0, block: 19},
				{dx: 2, dy: 5, dz: 0, block: 19},
				{dx: -2, dy: 5, dz: 1, block: 19},
				{dx: -1, dy: 5, dz: 1, block: 19},
				{dx: 0, dy: 5, dz: 1, block: 19},
				{dx: 1, dy: 5, dz: 1, block: 19},
				{dx: 2, dy: 5, dz: 1, block: 19},
				{dx: -1, dy: 5, dz: 2, block: 19},
				{dx: 0, dy: 5, dz: 2, block: 19},
				{dx: 1, dy: 5, dz: 2, block: 19},
				{dx: -1, dy: 6, dz: -1, block: 19},
				{dx: 0, dy: 6, dz: -1, block: 19},
				{dx: 1, dy: 6, dz: -1, block: 19},
				{dx: -1, dy: 6, dz: 0, block: 19},
				{dx: 0, dy: 6, dz: 0, block: 17},
				{dx: 1, dy: 6, dz: 0, block: 19},
				{dx: -1, dy: 6, dz: 1, block: 19},
				{dx: 0, dy: 6, dz: 1, block: 19},
				{dx: 1, dy: 6, dz: 1, block: 19},
				{dx: 0, dy: 7, dz: -1, block: 19},
				{dx: -1, dy: 7, dz: 0, block: 19},
				{dx: 0, dy: 7, dz: 0, block: 19},
				{dx: 1, dy: 7, dz: 0, block: 19},
				{dx: 0, dy: 7, dz: 1, block: 19},
				{dx: 0, dy: 8, dz: -1, block: 19},
				{dx: -1, dy: 8, dz: 0, block: 19},
				{dx: 0, dy: 8, dz: 0, block: 19},
				{dx: 1, dy: 8, dz: 0, block: 19},
				{dx: 0, dy: 8, dz: 1, block: 19},
			},
		},
		{
			name: "trunk height five",
			seed: 0,
			x:    0,
			y:    0,
			z:    0,
			want: []kernelTreeBlocksRecord{
				{dx: 0, dy: 0, dz: 0, block: 17},
				{dx: 0, dy: 1, dz: 0, block: 17},
				{dx: -1, dy: 2, dz: -2, block: 19},
				{dx: 0, dy: 2, dz: -2, block: 19},
				{dx: 1, dy: 2, dz: -2, block: 19},
				{dx: -2, dy: 2, dz: -1, block: 19},
				{dx: -1, dy: 2, dz: -1, block: 19},
				{dx: 0, dy: 2, dz: -1, block: 19},
				{dx: 1, dy: 2, dz: -1, block: 19},
				{dx: 2, dy: 2, dz: -1, block: 19},
				{dx: -2, dy: 2, dz: 0, block: 19},
				{dx: -1, dy: 2, dz: 0, block: 19},
				{dx: 0, dy: 2, dz: 0, block: 17},
				{dx: 1, dy: 2, dz: 0, block: 19},
				{dx: 2, dy: 2, dz: 0, block: 19},
				{dx: -2, dy: 2, dz: 1, block: 19},
				{dx: -1, dy: 2, dz: 1, block: 19},
				{dx: 0, dy: 2, dz: 1, block: 19},
				{dx: 1, dy: 2, dz: 1, block: 19},
				{dx: 2, dy: 2, dz: 1, block: 19},
				{dx: -1, dy: 2, dz: 2, block: 19},
				{dx: 0, dy: 2, dz: 2, block: 19},
				{dx: 1, dy: 2, dz: 2, block: 19},
				{dx: -1, dy: 3, dz: -2, block: 19},
				{dx: 0, dy: 3, dz: -2, block: 19},
				{dx: 1, dy: 3, dz: -2, block: 19},
				{dx: -2, dy: 3, dz: -1, block: 19},
				{dx: -1, dy: 3, dz: -1, block: 19},
				{dx: 0, dy: 3, dz: -1, block: 19},
				{dx: 1, dy: 3, dz: -1, block: 19},
				{dx: 2, dy: 3, dz: -1, block: 19},
				{dx: -2, dy: 3, dz: 0, block: 19},
				{dx: -1, dy: 3, dz: 0, block: 19},
				{dx: 0, dy: 3, dz: 0, block: 17},
				{dx: 1, dy: 3, dz: 0, block: 19},
				{dx: 2, dy: 3, dz: 0, block: 19},
				{dx: -2, dy: 3, dz: 1, block: 19},
				{dx: -1, dy: 3, dz: 1, block: 19},
				{dx: 0, dy: 3, dz: 1, block: 19},
				{dx: 1, dy: 3, dz: 1, block: 19},
				{dx: 2, dy: 3, dz: 1, block: 19},
				{dx: -1, dy: 3, dz: 2, block: 19},
				{dx: 0, dy: 3, dz: 2, block: 19},
				{dx: 1, dy: 3, dz: 2, block: 19},
				{dx: -1, dy: 4, dz: -1, block: 19},
				{dx: 0, dy: 4, dz: -1, block: 19},
				{dx: 1, dy: 4, dz: -1, block: 19},
				{dx: -1, dy: 4, dz: 0, block: 19},
				{dx: 0, dy: 4, dz: 0, block: 17},
				{dx: 1, dy: 4, dz: 0, block: 19},
				{dx: -1, dy: 4, dz: 1, block: 19},
				{dx: 0, dy: 4, dz: 1, block: 19},
				{dx: 1, dy: 4, dz: 1, block: 19},
				{dx: 0, dy: 5, dz: -1, block: 19},
				{dx: -1, dy: 5, dz: 0, block: 19},
				{dx: 0, dy: 5, dz: 0, block: 19},
				{dx: 1, dy: 5, dz: 0, block: 19},
				{dx: 0, dy: 5, dz: 1, block: 19},
			},
		},
		{
			name: "trunk height six",
			seed: 1,
			x:    0,
			y:    0,
			z:    0,
			want: []kernelTreeBlocksRecord{
				{dx: 0, dy: 0, dz: 0, block: 17},
				{dx: 0, dy: 1, dz: 0, block: 17},
				{dx: 0, dy: 2, dz: 0, block: 17},
				{dx: -1, dy: 3, dz: -2, block: 19},
				{dx: 0, dy: 3, dz: -2, block: 19},
				{dx: 1, dy: 3, dz: -2, block: 19},
				{dx: -2, dy: 3, dz: -1, block: 19},
				{dx: -1, dy: 3, dz: -1, block: 19},
				{dx: 0, dy: 3, dz: -1, block: 19},
				{dx: 1, dy: 3, dz: -1, block: 19},
				{dx: 2, dy: 3, dz: -1, block: 19},
				{dx: -2, dy: 3, dz: 0, block: 19},
				{dx: -1, dy: 3, dz: 0, block: 19},
				{dx: 0, dy: 3, dz: 0, block: 17},
				{dx: 1, dy: 3, dz: 0, block: 19},
				{dx: 2, dy: 3, dz: 0, block: 19},
				{dx: -2, dy: 3, dz: 1, block: 19},
				{dx: -1, dy: 3, dz: 1, block: 19},
				{dx: 0, dy: 3, dz: 1, block: 19},
				{dx: 1, dy: 3, dz: 1, block: 19},
				{dx: 2, dy: 3, dz: 1, block: 19},
				{dx: -1, dy: 3, dz: 2, block: 19},
				{dx: 0, dy: 3, dz: 2, block: 19},
				{dx: 1, dy: 3, dz: 2, block: 19},
				{dx: -1, dy: 4, dz: -2, block: 19},
				{dx: 0, dy: 4, dz: -2, block: 19},
				{dx: 1, dy: 4, dz: -2, block: 19},
				{dx: -2, dy: 4, dz: -1, block: 19},
				{dx: -1, dy: 4, dz: -1, block: 19},
				{dx: 0, dy: 4, dz: -1, block: 19},
				{dx: 1, dy: 4, dz: -1, block: 19},
				{dx: 2, dy: 4, dz: -1, block: 19},
				{dx: -2, dy: 4, dz: 0, block: 19},
				{dx: -1, dy: 4, dz: 0, block: 19},
				{dx: 0, dy: 4, dz: 0, block: 17},
				{dx: 1, dy: 4, dz: 0, block: 19},
				{dx: 2, dy: 4, dz: 0, block: 19},
				{dx: -2, dy: 4, dz: 1, block: 19},
				{dx: -1, dy: 4, dz: 1, block: 19},
				{dx: 0, dy: 4, dz: 1, block: 19},
				{dx: 1, dy: 4, dz: 1, block: 19},
				{dx: 2, dy: 4, dz: 1, block: 19},
				{dx: -1, dy: 4, dz: 2, block: 19},
				{dx: 0, dy: 4, dz: 2, block: 19},
				{dx: 1, dy: 4, dz: 2, block: 19},
				{dx: -1, dy: 5, dz: -1, block: 19},
				{dx: 0, dy: 5, dz: -1, block: 19},
				{dx: 1, dy: 5, dz: -1, block: 19},
				{dx: -1, dy: 5, dz: 0, block: 19},
				{dx: 0, dy: 5, dz: 0, block: 17},
				{dx: 1, dy: 5, dz: 0, block: 19},
				{dx: -1, dy: 5, dz: 1, block: 19},
				{dx: 0, dy: 5, dz: 1, block: 19},
				{dx: 1, dy: 5, dz: 1, block: 19},
				{dx: 0, dy: 6, dz: -1, block: 19},
				{dx: -1, dy: 6, dz: 0, block: 19},
				{dx: 0, dy: 6, dz: 0, block: 19},
				{dx: 1, dy: 6, dz: 0, block: 19},
				{dx: 0, dy: 6, dz: 1, block: 19},
			},
		},
		{
			name: "trunk height seven",
			seed: 12,
			x:    0,
			y:    0,
			z:    0,
			want: []kernelTreeBlocksRecord{
				{dx: 0, dy: 0, dz: 0, block: 17},
				{dx: 0, dy: 1, dz: 0, block: 17},
				{dx: 0, dy: 2, dz: 0, block: 17},
				{dx: 0, dy: 3, dz: 0, block: 17},
				{dx: -1, dy: 4, dz: -2, block: 19},
				{dx: 0, dy: 4, dz: -2, block: 19},
				{dx: 1, dy: 4, dz: -2, block: 19},
				{dx: -2, dy: 4, dz: -1, block: 19},
				{dx: -1, dy: 4, dz: -1, block: 19},
				{dx: 0, dy: 4, dz: -1, block: 19},
				{dx: 1, dy: 4, dz: -1, block: 19},
				{dx: 2, dy: 4, dz: -1, block: 19},
				{dx: -2, dy: 4, dz: 0, block: 19},
				{dx: -1, dy: 4, dz: 0, block: 19},
				{dx: 0, dy: 4, dz: 0, block: 17},
				{dx: 1, dy: 4, dz: 0, block: 19},
				{dx: 2, dy: 4, dz: 0, block: 19},
				{dx: -2, dy: 4, dz: 1, block: 19},
				{dx: -1, dy: 4, dz: 1, block: 19},
				{dx: 0, dy: 4, dz: 1, block: 19},
				{dx: 1, dy: 4, dz: 1, block: 19},
				{dx: 2, dy: 4, dz: 1, block: 19},
				{dx: -1, dy: 4, dz: 2, block: 19},
				{dx: 0, dy: 4, dz: 2, block: 19},
				{dx: 1, dy: 4, dz: 2, block: 19},
				{dx: -1, dy: 5, dz: -2, block: 19},
				{dx: 0, dy: 5, dz: -2, block: 19},
				{dx: 1, dy: 5, dz: -2, block: 19},
				{dx: -2, dy: 5, dz: -1, block: 19},
				{dx: -1, dy: 5, dz: -1, block: 19},
				{dx: 0, dy: 5, dz: -1, block: 19},
				{dx: 1, dy: 5, dz: -1, block: 19},
				{dx: 2, dy: 5, dz: -1, block: 19},
				{dx: -2, dy: 5, dz: 0, block: 19},
				{dx: -1, dy: 5, dz: 0, block: 19},
				{dx: 0, dy: 5, dz: 0, block: 17},
				{dx: 1, dy: 5, dz: 0, block: 19},
				{dx: 2, dy: 5, dz: 0, block: 19},
				{dx: -2, dy: 5, dz: 1, block: 19},
				{dx: -1, dy: 5, dz: 1, block: 19},
				{dx: 0, dy: 5, dz: 1, block: 19},
				{dx: 1, dy: 5, dz: 1, block: 19},
				{dx: 2, dy: 5, dz: 1, block: 19},
				{dx: -1, dy: 5, dz: 2, block: 19},
				{dx: 0, dy: 5, dz: 2, block: 19},
				{dx: 1, dy: 5, dz: 2, block: 19},
				{dx: -1, dy: 6, dz: -1, block: 19},
				{dx: 0, dy: 6, dz: -1, block: 19},
				{dx: 1, dy: 6, dz: -1, block: 19},
				{dx: -1, dy: 6, dz: 0, block: 19},
				{dx: 0, dy: 6, dz: 0, block: 17},
				{dx: 1, dy: 6, dz: 0, block: 19},
				{dx: -1, dy: 6, dz: 1, block: 19},
				{dx: 0, dy: 6, dz: 1, block: 19},
				{dx: 1, dy: 6, dz: 1, block: 19},
				{dx: 0, dy: 7, dz: -1, block: 19},
				{dx: -1, dy: 7, dz: 0, block: 19},
				{dx: 0, dy: 7, dz: 0, block: 19},
				{dx: 1, dy: 7, dz: 0, block: 19},
				{dx: 0, dy: 7, dz: 1, block: 19},
			},
		},
		{
			name: "fluffy crown",
			seed: 3,
			x:    0,
			y:    0,
			z:    0,
			want: []kernelTreeBlocksRecord{
				{dx: 0, dy: 0, dz: 0, block: 17},
				{dx: 0, dy: 1, dz: 0, block: 17},
				{dx: 0, dy: 2, dz: 0, block: 17},
				{dx: -1, dy: 3, dz: -2, block: 19},
				{dx: 0, dy: 3, dz: -2, block: 19},
				{dx: 1, dy: 3, dz: -2, block: 19},
				{dx: -2, dy: 3, dz: -1, block: 19},
				{dx: -1, dy: 3, dz: -1, block: 19},
				{dx: 0, dy: 3, dz: -1, block: 19},
				{dx: 1, dy: 3, dz: -1, block: 19},
				{dx: 2, dy: 3, dz: -1, block: 19},
				{dx: -2, dy: 3, dz: 0, block: 19},
				{dx: -1, dy: 3, dz: 0, block: 19},
				{dx: 0, dy: 3, dz: 0, block: 17},
				{dx: 1, dy: 3, dz: 0, block: 19},
				{dx: 2, dy: 3, dz: 0, block: 19},
				{dx: -2, dy: 3, dz: 1, block: 19},
				{dx: -1, dy: 3, dz: 1, block: 19},
				{dx: 0, dy: 3, dz: 1, block: 19},
				{dx: 1, dy: 3, dz: 1, block: 19},
				{dx: 2, dy: 3, dz: 1, block: 19},
				{dx: -1, dy: 3, dz: 2, block: 19},
				{dx: 0, dy: 3, dz: 2, block: 19},
				{dx: 1, dy: 3, dz: 2, block: 19},
				{dx: -1, dy: 4, dz: -2, block: 19},
				{dx: 0, dy: 4, dz: -2, block: 19},
				{dx: 1, dy: 4, dz: -2, block: 19},
				{dx: -2, dy: 4, dz: -1, block: 19},
				{dx: -1, dy: 4, dz: -1, block: 19},
				{dx: 0, dy: 4, dz: -1, block: 19},
				{dx: 1, dy: 4, dz: -1, block: 19},
				{dx: 2, dy: 4, dz: -1, block: 19},
				{dx: -2, dy: 4, dz: 0, block: 19},
				{dx: -1, dy: 4, dz: 0, block: 19},
				{dx: 0, dy: 4, dz: 0, block: 17},
				{dx: 1, dy: 4, dz: 0, block: 19},
				{dx: 2, dy: 4, dz: 0, block: 19},
				{dx: -2, dy: 4, dz: 1, block: 19},
				{dx: -1, dy: 4, dz: 1, block: 19},
				{dx: 0, dy: 4, dz: 1, block: 19},
				{dx: 1, dy: 4, dz: 1, block: 19},
				{dx: 2, dy: 4, dz: 1, block: 19},
				{dx: -1, dy: 4, dz: 2, block: 19},
				{dx: 0, dy: 4, dz: 2, block: 19},
				{dx: 1, dy: 4, dz: 2, block: 19},
				{dx: -1, dy: 5, dz: -1, block: 19},
				{dx: 0, dy: 5, dz: -1, block: 19},
				{dx: 1, dy: 5, dz: -1, block: 19},
				{dx: -1, dy: 5, dz: 0, block: 19},
				{dx: 0, dy: 5, dz: 0, block: 17},
				{dx: 1, dy: 5, dz: 0, block: 19},
				{dx: -1, dy: 5, dz: 1, block: 19},
				{dx: 0, dy: 5, dz: 1, block: 19},
				{dx: 1, dy: 5, dz: 1, block: 19},
				{dx: 0, dy: 6, dz: -1, block: 19},
				{dx: -1, dy: 6, dz: 0, block: 19},
				{dx: 0, dy: 6, dz: 0, block: 19},
				{dx: 1, dy: 6, dz: 0, block: 19},
				{dx: 0, dy: 6, dz: 1, block: 19},
				{dx: 0, dy: 7, dz: -1, block: 19},
				{dx: -1, dy: 7, dz: 0, block: 19},
				{dx: 0, dy: 7, dz: 0, block: 19},
				{dx: 1, dy: 7, dz: 0, block: 19},
				{dx: 0, dy: 7, dz: 1, block: 19},
			},
		},
	}
	for _, tc := range cases {
		input := kernelTreeBlocksInput(tc.seed, tc.x, tc.y, tc.z)
		first := make([]byte, nativeabi.TreeBlocksMaxOutputBytes)
		second := make([]byte, nativeabi.TreeBlocksMaxOutputBytes)
		count := nativeabi.TreeBlocks(input, first)
		again := nativeabi.TreeBlocks(input, second)
		if count != again {
			t.Errorf("%s: repeated generation counts differ: %d vs %d", tc.name, count, again)
		}
		if !bytes.Equal(first, second) {
			t.Errorf("%s: repeated generation differs", tc.name)
		}
		got := kernelTreeBlocksDecode(t, tc.name, first, count)
		if len(got) != len(tc.want) {
			t.Errorf("%s: record count got %d want %d", tc.name, len(got), len(tc.want))
			continue
		}
		for index, want := range tc.want {
			if got[index] != want {
				t.Errorf("%s: record %d got %+v want %+v", tc.name, index, got[index], want)
			}
		}
	}

	// Rejected requests panic and leave the caller-owned output untouched.
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
	validInput := kernelTreeBlocksInput(0, 3, -64, 5)

	shortInputOutput := canaryOutput(nativeabi.TreeBlocksMaxOutputBytes)
	assertPanic(t, "short input", shortInputOutput, func() {
		nativeabi.TreeBlocks(validInput[:len(validInput)-1], shortInputOutput)
	})
	badMagicOutput := canaryOutput(nativeabi.TreeBlocksMaxOutputBytes)
	assertPanic(t, "bad magic", badMagicOutput, func() {
		bad := bytes.Clone(validInput)
		bad[0] = 'X'
		nativeabi.TreeBlocks(bad, badMagicOutput)
	})
	wrongLayoutOutput := canaryOutput(nativeabi.TreeBlocksMaxOutputBytes)
	assertPanic(t, "wrong layout", wrongLayoutOutput, func() {
		bad := bytes.Clone(validInput)
		binary.LittleEndian.PutUint32(bad[4:8], 2)
		nativeabi.TreeBlocks(bad, wrongLayoutOutput)
	})
	for _, y := range []int32{-65, 312} {
		rootYOutput := canaryOutput(nativeabi.TreeBlocksMaxOutputBytes)
		assertPanic(t, "out-of-range root y", rootYOutput, func() {
			nativeabi.TreeBlocks(kernelTreeBlocksInput(0, 3, y, 5), rootYOutput)
		})
	}
	for _, x := range []int32{math.MinInt32 + 1, math.MaxInt32 - 1} {
		rootXOutput := canaryOutput(nativeabi.TreeBlocksMaxOutputBytes)
		assertPanic(t, "out-of-range root x", rootXOutput, func() {
			nativeabi.TreeBlocks(kernelTreeBlocksInput(0, x, 64, 5), rootXOutput)
		})
	}
	for _, z := range []int32{math.MinInt32 + 1, math.MaxInt32 - 1} {
		rootZOutput := canaryOutput(nativeabi.TreeBlocksMaxOutputBytes)
		assertPanic(t, "out-of-range root z", rootZOutput, func() {
			nativeabi.TreeBlocks(kernelTreeBlocksInput(0, 3, 64, z), rootZOutput)
		})
	}

	// Capacity semantics: a request needs only its actual output capacity
	// (count header plus produced records). A larger buffer is legal because
	// callers may preallocate the static maximum; the suffix beyond the
	// produced records must stay untouched and the produced bytes must match
	// the exactly sized call byte for byte.
	large := canaryOutput(nativeabi.TreeBlocksMaxOutputBytes)
	count := nativeabi.TreeBlocks(validInput, large)
	needed := nativeabi.TreeBlocksCountBytes + count*nativeabi.TreeBlocksRecordBytes
	exact := make([]byte, needed)
	nativeabi.TreeBlocks(validInput, exact)
	if !bytes.Equal(large[:needed], exact) {
		t.Error("larger output buffer produced different record bytes")
	}
	for index := needed; index < len(large); index++ {
		if large[index] != 0xA5 {
			t.Fatalf("larger output buffer modified the suffix at byte %d", index)
		}
	}

	exportDir := os.Getenv("RUNTIME_ORACLE_EXPORT_DIR")
	if exportDir != "" {
		if err := os.MkdirAll(exportDir, 0755); err != nil {
			t.Fatalf("MkdirAll: %v", err)
		}
		output := make([]byte, needed)
		nativeabi.TreeBlocks(validInput, output)
		if err := os.WriteFile(filepath.Join(exportDir, "tree_blocks_input.bin"), validInput, 0644); err != nil {
			t.Fatalf("WriteFile: %v", err)
		}
		if err := os.WriteFile(filepath.Join(exportDir, "tree_blocks_output.bin"), output, 0644); err != nil {
			t.Fatalf("WriteFile: %v", err)
		}
	}
}
