package main

import (
	"bytes"
	"encoding/binary"
	"os"
	"path/filepath"
	"testing"

	"github.com/channing771/mornlea/packages/shared/nativeabi"
)

// Frozen MFL1 rescan framing on top of the 26-byte header: 24 center-chunk
// section records (kind u8 + pad u8 + a uniform id or 4096 cell ids), the
// 68-column skirt halo and the 3x3 metadata table. The engine crate's rescan
// module doc owns the layout; this builder encodes exactly it.
const (
	kernelFluidRescanLayoutVersion  = 1
	kernelFluidRescanHeaderBytes    = 26
	kernelFluidRescanSections       = 24
	kernelFluidRescanSectionEdge    = 16
	kernelFluidRescanSectionCells   = 4096
	kernelFluidRescanWorldHeight    = 384
	kernelFluidRescanSkirtColumns   = 68
	kernelFluidRescanMetadataChunks = 9
	kernelFluidRescanPositionBytes  = 12
	kernelFluidRescanSummaryBytes   = 8
)

// Stable block ids of the fluid rule table, matching internal/core/block.go.
const (
	kernelFluidRescanAir         = 0
	kernelFluidRescanStone       = 2
	kernelFluidRescanWaterSource = 27
)

// kernelFluidRescanSkirtColumn mirrors the kernel's frozen skirt column order:
// four sides in (x=-1, z=0..15), (x=16, z=0..15), (z=-1, x=0..15),
// (z=16, x=0..15) main order, then the four corners.
func kernelFluidRescanSkirtColumn(bx, bz int32) int {
	switch {
	case bx == 0 && bz >= 1 && bz <= 16:
		return int(bz - 1)
	case bx == 17 && bz >= 1 && bz <= 16:
		return kernelFluidRescanSectionEdge + int(bz-1)
	case bz == 0 && bx >= 1 && bx <= 16:
		return 2*kernelFluidRescanSectionEdge + int(bx-1)
	case bz == 17 && bx >= 1 && bx <= 16:
		return 3*kernelFluidRescanSectionEdge + int(bx-1)
	case bx == 0 && bz == 0:
		return 4 * kernelFluidRescanSectionEdge
	case bx == 17 && bz == 0:
		return 4*kernelFluidRescanSectionEdge + 1
	case bx == 0 && bz == 17:
		return 4*kernelFluidRescanSectionEdge + 2
	case bx == 17 && bz == 17:
		return 4*kernelFluidRescanSectionEdge + 3
	}
	panic("runtime-oracle: skirt column outside the rescan box")
}

// kernelFluidRescanMetaSlot mirrors the metadata table's chunk order: center
// first, then the eight neighbours in row-major (-1..1) order.
func kernelFluidRescanMetaSlot(dx, dz int32) int {
	switch {
	case dx == 0 && dz == 0:
		return 0
	case dx == -1 && dz == -1:
		return 1
	case dx == 0 && dz == -1:
		return 2
	case dx == 1 && dz == -1:
		return 3
	case dx == -1 && dz == 0:
		return 4
	case dx == 1 && dz == 0:
		return 5
	case dx == -1 && dz == 1:
		return 6
	case dx == 0 && dz == 1:
		return 7
	case dx == 1 && dz == 1:
		return 8
	}
	panic("runtime-oracle: metadata slot outside the 3x3 neighborhood")
}

// kernelFluidRescanBox is one fixture box: a solid default whose sections,
// skirt cells and metadata entries the scenario builders override.
type kernelFluidRescanBox struct {
	centerX, centerZ int32
	x0, x1, z0, z1   uint16
	startSection     uint8
	reserved         uint8
	budget           uint32
	layoutVersion    uint32
	uniform          [kernelFluidRescanSections]uint16
	isUniform        [kernelFluidRescanSections]bool
	dense            [kernelFluidRescanSections][kernelFluidRescanSectionCells]uint16
	skirt            [kernelFluidRescanSkirtColumns][kernelFluidRescanWorldHeight]uint16
	metaFlag         [kernelFluidRescanMetadataChunks][kernelFluidRescanSections]uint8
	metaID           [kernelFluidRescanMetadataChunks][kernelFluidRescanSections]uint16
}

// kernelFluidRescanSolidBox is the all-stone default: 24 uniform stone
// sections, a stone skirt and uniform stone metadata, scanning the full
// center-column range from section 0 with a generous budget.
func kernelFluidRescanSolidBox() *kernelFluidRescanBox {
	box := &kernelFluidRescanBox{
		x0:            1,
		x1:            kernelFluidRescanSectionEdge,
		z0:            1,
		z1:            kernelFluidRescanSectionEdge,
		budget:        100000,
		layoutVersion: kernelFluidRescanLayoutVersion,
	}
	for section := range box.uniform {
		box.isUniform[section] = true
		box.uniform[section] = kernelFluidRescanStone
	}
	for column := range box.skirt {
		for y := range box.skirt[column] {
			box.skirt[column][y] = kernelFluidRescanStone
		}
	}
	for chunk := range box.metaFlag {
		for section := range box.metaFlag[chunk] {
			box.metaFlag[chunk][section] = 1
			box.metaID[chunk][section] = kernelFluidRescanStone
		}
	}
	return box
}

func (b *kernelFluidRescanBox) uniformSection(section int, id uint16) {
	b.isUniform[section] = true
	b.uniform[section] = id
}

func (b *kernelFluidRescanBox) denseSection(section int) {
	b.isUniform[section] = false
	for index := range b.dense[section] {
		b.dense[section][index] = kernelFluidRescanStone
	}
}

func (b *kernelFluidRescanBox) setCenterCell(section, lx, y16, lz int, id uint16) {
	b.dense[section][lx+kernelFluidRescanSectionEdge*lz+kernelFluidRescanSectionEdge*kernelFluidRescanSectionEdge*y16] = id
}

func (b *kernelFluidRescanBox) setSkirtCell(bx int32, y int, bz int32, id uint16) {
	b.skirt[kernelFluidRescanSkirtColumn(bx, bz)][y] = id
}

func (b *kernelFluidRescanBox) setMeta(dx, dz int32, section int, flag uint8, id uint16) {
	chunk := kernelFluidRescanMetaSlot(dx, dz)
	b.metaFlag[chunk][section] = flag
	b.metaID[chunk][section] = id
}

// build encodes the box as one complete MFL1 input byte stream.
func (b *kernelFluidRescanBox) build() []byte {
	out := make([]byte, 0, kernelFluidRescanHeaderBytes+
		kernelFluidRescanSections*kernelFluidRescanDenseRecordBytes()+
		kernelFluidRescanSkirtColumns*kernelFluidRescanWorldHeight*2+
		kernelFluidRescanMetadataChunks*kernelFluidRescanSections*3)
	out = binary.LittleEndian.AppendUint32(out, b.layoutVersion)
	out = binary.LittleEndian.AppendUint32(out, uint32(b.centerX))
	out = binary.LittleEndian.AppendUint32(out, uint32(b.centerZ))
	out = binary.LittleEndian.AppendUint16(out, b.x0)
	out = binary.LittleEndian.AppendUint16(out, b.x1)
	out = binary.LittleEndian.AppendUint16(out, b.z0)
	out = binary.LittleEndian.AppendUint16(out, b.z1)
	out = append(out, b.startSection, b.reserved)
	out = binary.LittleEndian.AppendUint32(out, b.budget)
	for section := 0; section < kernelFluidRescanSections; section++ {
		if b.isUniform[section] {
			out = append(out, 0, 0)
			out = binary.LittleEndian.AppendUint16(out, b.uniform[section])
			continue
		}
		out = append(out, 1, 0)
		for _, id := range b.dense[section] {
			out = binary.LittleEndian.AppendUint16(out, id)
		}
	}
	for column := 0; column < kernelFluidRescanSkirtColumns; column++ {
		for y := 0; y < kernelFluidRescanWorldHeight; y++ {
			out = binary.LittleEndian.AppendUint16(out, b.skirt[column][y])
		}
	}
	for chunk := 0; chunk < kernelFluidRescanMetadataChunks; chunk++ {
		for section := 0; section < kernelFluidRescanSections; section++ {
			out = append(out, b.metaFlag[chunk][section])
			out = binary.LittleEndian.AppendUint16(out, b.metaID[chunk][section])
		}
	}
	return out
}

// kernelFluidRescanDenseRecordBytes is the byte length of one dense section
// record: kind + pad + 4096 little-endian cell ids.
func kernelFluidRescanDenseRecordBytes() int {
	return 2 + kernelFluidRescanSectionCells*2
}

// kernelFluidRescanSealedSourceBox carries one world-bottom source that the
// barrier below seals, so nothing is emitted.
func kernelFluidRescanSealedSourceBox() *kernelFluidRescanBox {
	box := kernelFluidRescanSolidBox()
	box.denseSection(0)
	box.setCenterCell(0, 2, 0, 2, kernelFluidRescanWaterSource)
	return box
}

// kernelFluidRescanUnsealedEdgeBox is a uniform source section whose
// section-level fixed point one air neighbour breaks, with one air skirt
// column so the sixteen edge sources on that column emit through the halo.
func kernelFluidRescanUnsealedEdgeBox() *kernelFluidRescanBox {
	box := kernelFluidRescanSolidBox()
	box.uniformSection(3, kernelFluidRescanWaterSource)
	box.setMeta(0, 1, 3, 1, kernelFluidRescanAir)
	for y := 0; y < kernelFluidRescanWorldHeight; y++ {
		box.setSkirtCell(0, y, 5, kernelFluidRescanAir)
	}
	return box
}

// kernelFluidRescanDenseMixedBox is one dense mixed section at center (-3, 2):
// flowing water, one unsealed source, and three sources sealed by their
// neighbours, the skirt corner and the section below.
func kernelFluidRescanDenseMixedBox() *kernelFluidRescanBox {
	box := kernelFluidRescanSolidBox()
	box.centerX = -3
	box.centerZ = 2
	box.denseSection(2)
	box.setCenterCell(2, 3, 1, 4, kernelFluidRescanWaterSource+2)
	box.setCenterCell(2, 5, 2, 6, kernelFluidRescanWaterSource)
	box.setCenterCell(2, 5, 1, 6, kernelFluidRescanAir)
	box.setCenterCell(2, 8, 3, 9, kernelFluidRescanWaterSource)
	box.setCenterCell(2, 0, 5, 0, kernelFluidRescanWaterSource)
	box.setCenterCell(2, 15, 0, 7, kernelFluidRescanWaterSource)
	return box
}

// kernelFluidRescanInteriorHaloBox scans one interior column whose source is
// unsealed only through the -x halo column.
func kernelFluidRescanInteriorHaloBox() *kernelFluidRescanBox {
	box := kernelFluidRescanSolidBox()
	box.x0, box.x1, box.z0, box.z1 = 1, 1, 1, 1
	box.denseSection(5)
	box.setCenterCell(5, 0, 7, 0, kernelFluidRescanWaterSource)
	box.setSkirtCell(0, 87, 1, kernelFluidRescanAir)
	return box
}

// kernelFluidRescanPositions decodes the world position stream of one output
// buffer (its trailing summary excluded).
func kernelFluidRescanPositions(output []byte) [][3]int32 {
	body := output[:len(output)-kernelFluidRescanSummaryBytes]
	positions := make([][3]int32, 0, len(body)/kernelFluidRescanPositionBytes)
	for len(body) > 0 {
		positions = append(positions, [3]int32{
			int32(binary.LittleEndian.Uint32(body[0:4])),
			int32(binary.LittleEndian.Uint32(body[4:8])),
			int32(binary.LittleEndian.Uint32(body[8:12])),
		})
		body = body[kernelFluidRescanPositionBytes:]
	}
	return positions
}

// kernelFluidRescanEdgePositions is the shared unsealed-edge observation: one
// emitted world position per y16 step of the source column, in scan order.
func kernelFluidRescanEdgePositions() [][3]int32 {
	positions := make([][3]int32, 0, kernelFluidRescanSectionEdge)
	for y16 := 0; y16 < kernelFluidRescanSectionEdge; y16++ {
		positions = append(positions, [3]int32{0, 48 + int32(y16) - 64, 4})
	}
	return positions
}

// TestFluidRescan records release-ABI observations of `nativeabi.FluidRescan`
// and locks them as ordered world positions plus spent/done summaries, so any
// drift between the Go-visible ABI and the pinned expectations fails here
// first. The Rust numerical-migration fluid test pins the same scenarios for
// the typed provider.
func TestFluidRescan(t *testing.T) {
	scenarios := []struct {
		name      string
		box       *kernelFluidRescanBox
		positions [][3]int32
		spent     uint32
		done      bool
	}{
		{
			name:  "uniform nonfluid",
			box:   kernelFluidRescanSolidBox(),
			spent: 24,
			done:  true,
		},
		{
			name:  "sealed source",
			box:   kernelFluidRescanSealedSourceBox(),
			spent: 4119,
			done:  true,
		},
		{
			name:      "unsealed edge source",
			box:       kernelFluidRescanUnsealedEdgeBox(),
			positions: kernelFluidRescanEdgePositions(),
			spent:     4119,
			done:      true,
		},
		{
			name: "dense mixed",
			box:  kernelFluidRescanDenseMixedBox(),
			positions: [][3]int32{
				{-45, -31, 36},
				{-43, -30, 38},
			},
			spent: 4119,
			done:  true,
		},
		{
			name: "interior halo edge",
			box:  kernelFluidRescanInteriorHaloBox(),
			positions: [][3]int32{
				{0, 23, 0},
			},
			spent: 39,
			done:  true,
		},
		{
			name: "budget zero",
			box: func() *kernelFluidRescanBox {
				box := kernelFluidRescanSolidBox()
				box.budget = 0
				return box
			}(),
		},
		{
			name: "budget one",
			box: func() *kernelFluidRescanBox {
				box := kernelFluidRescanSolidBox()
				box.budget = 1
				return box
			}(),
			spent: 1,
		},
		{
			name: "start section 23 completes",
			box: func() *kernelFluidRescanBox {
				box := kernelFluidRescanSolidBox()
				box.startSection = 23
				box.budget = 1
				return box
			}(),
			spent: 1,
			done:  true,
		},
		{
			name: "start section 23 budget break",
			box: func() *kernelFluidRescanBox {
				box := kernelFluidRescanSolidBox()
				box.startSection = 23
				box.budget = 0
				return box
			}(),
		},
	}
	for _, tc := range scenarios {
		input := tc.box.build()
		need := kernelFluidRescanSummaryBytes + kernelFluidRescanPositionBytes*len(tc.positions)
		output := make([]byte, need)
		status, written := nativeabi.FluidRescan(input, output)
		if status != nativeabi.StatusOK {
			t.Fatalf("%s: status got %v want OK", tc.name, status)
		}
		if written != need {
			t.Fatalf("%s: written got %d want %d", tc.name, written, need)
		}
		positions := kernelFluidRescanPositions(output[:written])
		if len(positions) != len(tc.positions) {
			t.Errorf("%s: position count got %d want %d", tc.name, len(positions), len(tc.positions))
		} else {
			for index := range positions {
				if positions[index] != tc.positions[index] {
					t.Errorf("%s: position %d got %v want %v", tc.name, index, positions[index], tc.positions[index])
				}
			}
		}
		spent := binary.LittleEndian.Uint32(output[written-kernelFluidRescanSummaryBytes:])
		if spent != tc.spent {
			t.Errorf("%s: spent got %d want %d", tc.name, spent, tc.spent)
		}
		doneFlag := output[written-kernelFluidRescanSummaryBytes+4]
		if (doneFlag == 1) != tc.done {
			t.Errorf("%s: done got %v want %v", tc.name, doneFlag == 1, tc.done)
		}
		for _, pad := range output[written-kernelFluidRescanSummaryBytes+5 : written] {
			if pad != 0 {
				t.Errorf("%s: summary pad byte got %d want 0", tc.name, pad)
			}
		}

		// Run-to-run determinism over the exact output stream.
		second := make([]byte, need)
		status, written = nativeabi.FluidRescan(input, second)
		if status != nativeabi.StatusOK || written != need {
			t.Fatalf("%s: repeated call status %v written %d", tc.name, status, written)
		}
		if !bytes.Equal(output[:written], second[:written]) {
			t.Errorf("%s: repeated scan differs", tc.name)
		}
	}

	// Failure conditions return a non-OK status and leave the caller-owned
	// output untouched; a short output additionally reports the exact
	// required capacity through the returned byte count.
	canaryOutput := func(size int) []byte {
		output := make([]byte, size)
		for index := range output {
			output[index] = 0xA5
		}
		return output
	}
	assertUntouched := func(t *testing.T, name string, output, before []byte) {
		t.Helper()
		if !bytes.Equal(output, before) {
			t.Errorf("%s: rejected request modified the caller-owned output", name)
		}
	}

	validInput := kernelFluidRescanDenseMixedBox().build()
	validNeed := kernelFluidRescanSummaryBytes + 2*kernelFluidRescanPositionBytes

	shortInput := canaryOutput(validNeed)
	shortInputBefore := bytes.Clone(shortInput)
	status, written := nativeabi.FluidRescan(validInput[:len(validInput)-1], shortInput)
	if status == nativeabi.StatusOK {
		t.Error("short input: status got OK want input rejection")
	}
	if written != 0 {
		t.Errorf("short input: written got %d want 0", written)
	}
	assertUntouched(t, "short input", shortInput, shortInputBefore)

	shortOutput := canaryOutput(validNeed - 1)
	shortOutputBefore := bytes.Clone(shortOutput)
	status, written = nativeabi.FluidRescan(validInput, shortOutput)
	if status != nativeabi.StatusOutputOverflow {
		t.Errorf("short output: status got %v want OUTPUT_OVERFLOW", status)
	}
	if written != validNeed {
		t.Errorf("short output: required capacity got %d want %d", written, validNeed)
	}
	assertUntouched(t, "short output", shortOutput, shortOutputBefore)

	badVersion := bytes.Clone(validInput)
	binary.LittleEndian.PutUint32(badVersion[0:4], kernelFluidRescanLayoutVersion+1)
	badVersionOutput := canaryOutput(validNeed)
	badVersionBefore := bytes.Clone(badVersionOutput)
	status, written = nativeabi.FluidRescan(badVersion, badVersionOutput)
	if status == nativeabi.StatusOK {
		t.Error("bad layout version: status got OK want input rejection")
	}
	if written != 0 {
		t.Errorf("bad layout version: written got %d want 0", written)
	}
	assertUntouched(t, "bad layout version", badVersionOutput, badVersionBefore)

	exportDir := os.Getenv("RUNTIME_ORACLE_EXPORT_DIR")
	if exportDir != "" {
		if err := os.MkdirAll(exportDir, 0755); err != nil {
			t.Fatalf("MkdirAll: %v", err)
		}
		input := kernelFluidRescanDenseMixedBox().build()
		output := make([]byte, validNeed)
		status, written = nativeabi.FluidRescan(input, output)
		if status != nativeabi.StatusOK {
			t.Fatalf("export scan: status %v written %d", status, written)
		}
		output = output[:written]
		if err := os.WriteFile(filepath.Join(exportDir, "fluid_rescan_input.bin"), input, 0644); err != nil {
			t.Fatalf("WriteFile: %v", err)
		}
		if err := os.WriteFile(filepath.Join(exportDir, "fluid_rescan_output.bin"), output, 0644); err != nil {
			t.Fatalf("WriteFile: %v", err)
		}
	}
}
