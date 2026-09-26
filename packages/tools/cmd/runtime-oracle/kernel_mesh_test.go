package main

import (
	"encoding/binary"
	"hash/fnv"
	"os"
	"path/filepath"
	"testing"

	"github.com/channing771/mornlea/packages/shared/nativeabi"
)

// Geometry scenes of the raw mesh/light section entry, built on the shared
// MGM1 fixture from `kernel_mesh_view_test.go`. The output is the ordered
// stream of packed `u64` quads the typed provider stages, so the Rust
// numerical-migration mesh test pins the same counts, digests and ordered
// fields. `kernelMeshStageWords` is the typed provider's fixed staging
// capacity: a dense accepted registry stays inside it even though it exceeds
// the production-consistent 6-quads-per-cell minimum.
const kernelMeshStageWords = 40960

// Captured release-ABI digests, shared verbatim with the Rust migration test.
const (
	wantFacesDigest uint64 = 0x105f552ad3c431f5
	wantOrderDigest uint64 = 0x7f0174fc3fdcee12
	wantDenseDigest uint64 = 0x99aaa0c381505725
)

// Geometry literals shared with the Rust migration fixtures.
const (
	kernelMeshAirID         = 0
	kernelMeshBarrierID     = 1
	kernelMeshStoneID       = 2
	kernelMeshWheatID       = 20
	kernelMeshStandingTorch = 71
	kernelMeshWallTorchPosX = 72
	kernelMeshBedID         = 76

	kernelMeshWheatMaterial     = 31
	kernelMeshTorchMaterial     = 90
	kernelMeshBedTopMaterial    = 700
	kernelMeshBedSideMaterial   = 91
	kernelMeshNearTopRaw        = 8
	kernelMeshFarTopRaw         = 13
	kernelMeshStandingPerCell   = 4
	kernelMeshWallPerCell       = 3
	kernelMeshBedPerCell        = 5
	kernelMeshPlantPerCell      = 4
	kernelMeshCombinedPerCell   = 8
	kernelMeshDenseCombinedQuad = kernelMeshCombinedPerCell * 4096
)

// kernelMeshExpectedQuad is one decoded packed quad in the order the mesher
// publishes it. Field extraction mirrors the documented 8-byte layout and is
// deliberately independent of the Rust decoder.
type kernelMeshExpectedQuad struct {
	x, y, z   uint8
	face      uint8
	back      bool
	material  uint16
	corners   [4]uint8
	ao, light uint8
}

// kernelMeshDecodeQuad decodes one packed ABI quad word. The corner-height
// form is distinguished by the nonzero corner-2 field, exactly like the frozen
// unpacking rule.
func kernelMeshDecodeQuad(word uint64) kernelMeshExpectedQuad {
	quad := kernelMeshExpectedQuad{
		x:        uint8(word & 0xf),
		y:        uint8(word >> 4 & 0xf),
		z:        uint8(word >> 8 & 0xf),
		face:     uint8(word >> 20 & 7),
		back:     (word>>12)&1 == 1,
		material: uint16(word >> 23),
		ao:       uint8(word >> 39),
		light:    uint8(word >> 47),
	}
	if corner2 := uint8(word >> 55 & 0xf); corner2 != 0 {
		quad.corners = [4]uint8{
			uint8(word >> 12 & 0xf),
			uint8(word >> 16 & 0xf),
			corner2,
			uint8(word >> 59 & 0xf),
		}
	}
	return quad
}

// kernelMeshDigest fingerprints the used quads as little-endian bytes with
// FNV-1a 64-bit, matching the Rust migration test's digest over packed words.
func kernelMeshDigest(quads []uint64) uint64 {
	raw := make([]byte, len(quads)*8)
	for index, word := range quads {
		binary.LittleEndian.PutUint64(raw[index*8:], word)
	}
	hash := fnv.New64a()
	hash.Write(raw)
	return hash.Sum64()
}

// kernelMeshFacesSection is the isolated-block scene: one opaque stone at the
// center cell whose six axial faces are visible toward air.
func kernelMeshFacesSection() *kernelMeshViewSection {
	section := newKernelMeshViewSection([]kernelMeshEntry{
		{id: kernelMeshAirID},
		{id: kernelMeshBarrierID, opaque: true, material: [6]uint16{1, 1, 1, 1, 1, 1}},
		{
			id:       kernelMeshStoneID,
			opaque:   true,
			material: [6]uint16{10, 11, 12, 13, 14, 15},
		},
	})
	section.setBlock(0, 0, 0, kernelMeshStoneID)
	section.markFaceVisible(2, 0)
	return section
}

// kernelMeshOrderSection is the plant/torch/bed ordering scene: one plant, one
// standing torch, one wall torch and one bed in a single row, with no axial
// faces anywhere.
func kernelMeshOrderSection() *kernelMeshViewSection {
	section := newKernelMeshViewSection([]kernelMeshEntry{
		{id: kernelMeshAirID},
		{id: kernelMeshBarrierID, opaque: true, material: [6]uint16{1, 1, 1, 1, 1, 1}},
		{id: kernelMeshWheatID, material: [6]uint16{31, 31, 31, 31, 31, 31}},
		{
			id:       kernelMeshStandingTorch,
			material: [6]uint16{90, 90, 90, 90, 90, 90},
			model:    1,
		},
		{
			id:       kernelMeshWallTorchPosX,
			material: [6]uint16{90, 90, 90, 90, 90, 90},
			model:    2,
		},
		{
			id:       kernelMeshBedID,
			material: [6]uint16{91, 91, 91, 700, 91, 91},
			model:    6,
		},
	})
	section.setBlock(1, 8, 8, kernelMeshWheatID)
	section.setBlock(2, 8, 8, kernelMeshStandingTorch)
	section.setBlock(3, 8, 8, kernelMeshWallTorchPosX)
	section.setBlock(4, 8, 8, kernelMeshBedID)
	return section
}

// kernelMeshCombinedSection is the model+plant scene: a standing torch whose
// entry material is a plant layer, so the plant pass and the model dispatcher
// each publish four quads for the same cell. `dense` fills the whole section.
func kernelMeshCombinedSection(dense bool) *kernelMeshViewSection {
	section := newKernelMeshViewSection([]kernelMeshEntry{
		{id: kernelMeshAirID},
		{id: kernelMeshBarrierID, opaque: true, material: [6]uint16{1, 1, 1, 1, 1, 1}},
		{
			id:       kernelMeshStandingTorch,
			material: [6]uint16{31, 31, 31, 31, 31, 31},
			model:    1,
		},
	})
	if dense {
		section.fillBlock(kernelMeshStandingTorch)
	} else {
		section.setBlock(8, 8, 8, kernelMeshStandingTorch)
	}
	return section
}

// kernelMeshInvariantSection is the late packing-violation scene: the stone
// entry's +X face material is a plant layer, so the first axial quad (NegX)
// is valid and the second (PosX) violates the packing rule. The raw entry
// catches the resulting panic as status 9 with no published count.
func kernelMeshInvariantSection() *kernelMeshViewSection {
	section := newKernelMeshViewSection([]kernelMeshEntry{
		{id: kernelMeshAirID},
		{id: kernelMeshBarrierID, opaque: true, material: [6]uint16{1, 1, 1, 1, 1, 1}},
		{
			id:       kernelMeshStoneID,
			opaque:   true,
			material: [6]uint16{10, 31, 12, 13, 14, 15},
		},
	})
	section.setBlock(0, 0, 0, kernelMeshStoneID)
	section.markFaceVisible(2, 0)
	return section
}

// kernelMeshOrderQuads is the frozen emission order of the ordering scene:
// the plant pass runs before the model dispatcher, and the model dispatcher
// walks the section in (y, z, x) order.
func kernelMeshOrderQuads() []kernelMeshExpectedQuad {
	plant := func(x uint8) []kernelMeshExpectedQuad {
		quads := make([]kernelMeshExpectedQuad, 0, kernelMeshPlantPerCell)
		for _, cross := range []struct {
			face uint8
			back bool
		}{{6, false}, {6, true}, {7, false}, {7, true}} {
			quads = append(quads, kernelMeshExpectedQuad{
				x: x, y: 8, z: 8, face: cross.face, back: cross.back,
				material: kernelMeshWheatMaterial, ao: 0xff,
			})
		}
		return quads
	}
	torch := plant(2)
	for index := range torch {
		torch[index].material = kernelMeshTorchMaterial
	}
	near, far := uint8(kernelMeshNearTopRaw), uint8(kernelMeshFarTopRaw)
	wall := []kernelMeshExpectedQuad{
		{x: 3, y: 8, z: 8, face: 4, material: kernelMeshTorchMaterial, corners: [4]uint8{0, 0, far, near}, ao: 0xff},
		{x: 3, y: 8, z: 8, face: 5, material: kernelMeshTorchMaterial, corners: [4]uint8{0, 0, far, near}, ao: 0xff},
		{x: 3, y: 8, z: 8, face: 0, material: kernelMeshTorchMaterial, ao: 0xff},
	}
	side, top := uint16(kernelMeshBedSideMaterial), uint16(kernelMeshBedTopMaterial)
	bed := []kernelMeshExpectedQuad{
		{x: 4, y: 8, z: 8, face: 0, material: side, corners: [4]uint8{0, near, near, 0}, ao: 0xff},
		{x: 4, y: 8, z: 8, face: 1, material: side, corners: [4]uint8{0, near, near, 0}, ao: 0xff},
		{x: 4, y: 8, z: 8, face: 4, material: side, corners: [4]uint8{0, 0, near, near}, ao: 0xff},
		{x: 4, y: 8, z: 8, face: 5, material: side, corners: [4]uint8{0, 0, near, near}, ao: 0xff},
		{x: 4, y: 8, z: 8, face: 3, material: top, corners: [4]uint8{near, near, near, near}, ao: 0xff},
	}
	quads := append([]kernelMeshExpectedQuad{}, plant(1)...)
	quads = append(quads, torch...)
	quads = append(quads, wall...)
	return append(quads, bed...)
}

// TestKernelMesh records release-ABI observations of the raw mesh/light
// section geometry: isolated axial faces, plant/torch/bed emission order, a
// model+plant combination above the production-consistent per-cell bound, the
// dense accepted section that only the typed 40960-quad stage covers, and the
// late packing violation that the raw entry answers as status 9.
func TestKernelMesh(t *testing.T) {
	newScratch := func() []uint64 { return make([]uint64, kernelMeshViewScratchWords) }
	newOutput := func(words int) []uint64 {
		output := make([]uint64, words)
		for index := range output {
			output[index] = kernelMeshViewCanary
		}
		return output
	}

	// 1. Six isolated axial faces of one opaque block, in face order, with the
	// per-face material routing and a fully occluded-ambient value.
	faces := newOutput(kernelMeshViewOutputWords)
	status, count := kernelMeshViewRun(
		kernelMeshFacesSection().encode(), newScratch(), faces,
	)
	if status != nativeabi.StatusOK || count != 6 {
		t.Fatalf("six faces: status/count=%d/%d, want OK/6", status, count)
	}
	wantFaces := make([]kernelMeshExpectedQuad, 6)
	for index := range wantFaces {
		wantFaces[index] = kernelMeshExpectedQuad{
			x: 0, y: 0, z: 0,
			face:     uint8(index),
			material: uint16(10 + index),
			ao:       0xff,
		}
	}
	for index, want := range wantFaces {
		if got := kernelMeshDecodeQuad(faces[index]); got != want {
			t.Fatalf("six faces: quad %d got %+v want %+v", index, got, want)
		}
	}
	if count != 6 || kernelMeshDigest(faces[:count]) != wantFacesDigest {
		t.Fatalf(
			"six faces: count/digest=%d/%#x, want 6/%#x",
			count, kernelMeshDigest(faces[:count]), wantFacesDigest,
		)
	}

	// 2. Plant, torch and bed emission order: the plant pass precedes the model
	// dispatcher, which walks the section in (y, z, x) order and emits the
	// standing torch's four cross quads, the wall torch's two leaning plates
	// plus its cap, and the bed's four sides plus its top.
	order := newOutput(kernelMeshViewOutputWords)
	status, count = kernelMeshViewRun(
		kernelMeshOrderSection().encode(), newScratch(), order,
	)
	if status != nativeabi.StatusOK || count != len(kernelMeshOrderQuads()) {
		t.Fatalf(
			"ordering scene: status/count=%d/%d, want OK/%d",
			status, count, len(kernelMeshOrderQuads()),
		)
	}
	for index, want := range kernelMeshOrderQuads() {
		if got := kernelMeshDecodeQuad(order[index]); got != want {
			t.Fatalf("ordering scene: quad %d got %+v want %+v", index, got, want)
		}
	}
	if kernelMeshDigest(order[:count]) != wantOrderDigest {
		t.Fatalf(
			"ordering scene: digest=%#x, want %#x",
			kernelMeshDigest(order[:count]), wantOrderDigest,
		)
	}

	// 3. A valid model+plant combination: the standing torch's plant-layer
	// material makes both the plant pass and the model dispatcher publish four
	// cross quads for the same cell, so one cell exceeds the six-quad plain
	// block bound.
	combined := newOutput(kernelMeshViewOutputWords)
	status, count = kernelMeshViewRun(
		kernelMeshCombinedSection(false).encode(), newScratch(), combined,
	)
	if status != nativeabi.StatusOK || count != kernelMeshCombinedPerCell {
		t.Fatalf(
			"model+plant cell: status/count=%d/%d, want OK/%d",
			status, count, kernelMeshCombinedPerCell,
		)
	}
	for index, word := range combined[:count] {
		quad := kernelMeshDecodeQuad(word)
		if quad.face != 6 && quad.face != 7 || quad.material != kernelMeshWheatMaterial {
			t.Fatalf("model+plant cell: quad %d got %+v, want a plant cross quad", index, quad)
		}
	}

	// 4. The dense accepted model+plant section: 4096 cells at the combined
	// bound, which no longer fits the production-consistent 6-per-cell output
	// capacity but stays inside the typed 40960-quad stage.
	dense := newOutput(kernelMeshStageWords)
	status, count = kernelMeshViewRun(
		kernelMeshCombinedSection(true).encode(), newScratch(), dense,
	)
	if status != nativeabi.StatusOK || count != kernelMeshDenseCombinedQuad {
		t.Fatalf(
			"dense model+plant: status/count=%d/%d, want OK/%d",
			status, count, kernelMeshDenseCombinedQuad,
		)
	}
	if kernelMeshDigest(dense[:count]) != wantDenseDigest {
		t.Fatalf(
			"dense model+plant: digest=%#x, want %#x",
			kernelMeshDigest(dense[:count]), wantDenseDigest,
		)
	}
	// The used prefix is a complete geometry stream: the plant pass and the
	// model dispatcher each cover the whole section in (y, z, x) order.
	first, last := kernelMeshDecodeQuad(dense[0]), kernelMeshDecodeQuad(dense[count-1])
	if first.x != 0 || first.y != 0 || first.z != 0 || first.face != 6 || first.back {
		t.Fatalf("dense model+plant: first quad got %+v", first)
	}
	if last.x != 15 || last.y != 15 || last.z != 15 || last.face != 7 || !last.back {
		t.Fatalf("dense model+plant: last quad got %+v", last)
	}
	// The production-consistent minimum capacity is genuinely short here, which
	// is the late-publication case the ABI adapter owns.
	if count <= kernelMeshViewOutputWords {
		t.Fatalf("dense model+plant: count %d does not exceed the 6-per-cell bound", count)
	}

	// 5. A late packing violation: the first axial quad is valid, the second
	// carries a plant material on an axial face. The raw entry reports the
	// caught panic and publishes no count.
	invariant := newOutput(kernelMeshViewOutputWords)
	status, count = kernelMeshViewRun(
		kernelMeshInvariantSection().encode(), newScratch(), invariant,
	)
	if status != nativeabi.StatusPanic || count != 0 {
		t.Fatalf("late invariant: status/count=%d/%d, want PANIC/0", status, count)
	}

	exportDir := os.Getenv("RUNTIME_ORACLE_EXPORT_DIR")
	if exportDir != "" {
		if err := os.MkdirAll(exportDir, 0o755); err != nil {
			t.Fatalf("MkdirAll: %v", err)
		}
		exported := []struct {
			label  string
			input  []byte
			output []uint64
		}{
			{"mesh_faces", kernelMeshFacesSection().encode(), faces[:6]},
			{"mesh_order", kernelMeshOrderSection().encode(), order[:len(kernelMeshOrderQuads())]},
			{"mesh_combined", kernelMeshCombinedSection(false).encode(), combined[:kernelMeshCombinedPerCell]},
			{"mesh_dense", kernelMeshCombinedSection(true).encode(), dense[:kernelMeshDenseCombinedQuad]},
		}
		for _, fixture := range exported {
			if err := os.WriteFile(
				filepath.Join(exportDir, fixture.label+"_input.bin"),
				fixture.input,
				0o644,
			); err != nil {
				t.Fatalf("WriteFile(%s): %v", fixture.label+"_input.bin", err)
			}
			if err := os.WriteFile(
				filepath.Join(exportDir, fixture.label+"_output.bin"),
				flattenKernelMeshViewQuads(fixture.output),
				0o644,
			); err != nil {
				t.Fatalf("WriteFile(%s): %v", fixture.label+"_output.bin", err)
			}
		}
	}
}
