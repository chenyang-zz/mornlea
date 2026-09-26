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

// Raw `MGM1` mesh-section framing: 16-byte header (magic, section origin,
// registry count, visibility words per row, air id, barrier id), the 3x3x3
// neighborhood as 27 sections of 4096 little-endian u16 cells, nine
// height-presence bytes, nine 256-entry i16 column tables, 20-byte registry
// entries and the visibility bit rows. Scratch is the fixed 48^3 light volume
// plus its u32 queue; output capacity is 6 quads per center cell.
const (
	kernelMeshViewHeaderBytes        = 16
	kernelMeshViewSections           = 27
	kernelMeshViewSectionCells       = 4096
	kernelMeshViewBlocksBytes        = kernelMeshViewSections * kernelMeshViewSectionCells * 2
	kernelMeshViewHeightColumns      = 9
	kernelMeshViewHeightsBytes       = kernelMeshViewHeightColumns * 256 * 2
	kernelMeshViewRegistryEntryBytes = 20
	kernelMeshViewScratchWords       = (48 * 48 * 48 * 5) / 8
	kernelMeshViewOutputWords        = 6 * 4096
	kernelMeshViewCanary             = uint64(0xd15e_a5ed_f00d_cafe)
)

// Pinned release-ABI observations: the used quad count and the FNV-1a 64-bit
// digest over the used output quads for the three geometry scenes below.
const (
	kernelMeshViewLitCount   = 379
	kernelMeshViewLitDigest  = uint64(0x89808c66dfb9f75b)
	kernelMeshViewSkyCount   = 16384
	kernelMeshViewSkyDigest  = uint64(0x4d730d9a8f458725)
	kernelMeshViewDarkCount  = 14
	kernelMeshViewDarkDigest = uint64(0x37c2bdd706c14798)
)

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

// kernelMeshViewSection assembles one raw mesh-section request and keeps the
// buffers mutable so a case can perturb exactly one field.
type kernelMeshViewSection struct {
	originY        int32
	airID          uint16
	barrierID      uint16
	blocks         []uint16
	heightsPresent []bool
	heights        []int16
	entries        []kernelMeshEntry
	visibility     []uint64
}

func newKernelMeshViewSection(entries []kernelMeshEntry) *kernelMeshViewSection {
	wordsPerRow := (len(entries) + 63) / 64
	return &kernelMeshViewSection{
		airID:          0,
		barrierID:      1,
		blocks:         make([]uint16, kernelMeshViewSections*kernelMeshViewSectionCells),
		heightsPresent: make([]bool, kernelMeshViewHeightColumns),
		heights:        make([]int16, kernelMeshViewHeightColumns*256),
		entries:        entries,
		visibility:     make([]uint64, len(entries)*wordsPerRow),
	}
}

// kernelMeshViewCell mirrors the kernel's neighborhood indexing rule.
func kernelMeshViewCell(x, y, z int32) int {
	section := int((x+16)>>4)*3 + int((y+16)>>4)
	section = section*3 + int((z+16)>>4)
	offset := int((y+16)&15)<<8 | int((z+16)&15)<<4 | int((x+16)&15)
	return section*kernelMeshViewSectionCells + offset
}

func (s *kernelMeshViewSection) setBlock(x, y, z int32, id uint16) {
	s.blocks[kernelMeshViewCell(x, y, z)] = id
}

// markFaceVisible sets one visibility bit, so the id row's faces toward the
// adjacent id are emitted. The zero table emits no geometry at all.
func (s *kernelMeshViewSection) markFaceVisible(row, column int) {
	wordsPerRow := (len(s.entries) + 63) / 64
	s.visibility[row*wordsPerRow+column/64] |= 1 << (uint(column) % 64)
}

// markAirFacesVisible opens the ids' faces toward air, which is what the
// geometry cases need to publish quads.
func (s *kernelMeshViewSection) markAirFacesVisible(rows ...int) {
	for _, row := range rows {
		s.markFaceVisible(row, 0)
	}
}

// fillBlock sets every cell of the 3x3x3 neighborhood.
func (s *kernelMeshViewSection) fillBlock(id uint16) {
	for index := range s.blocks {
		s.blocks[index] = id
	}
}

// setHeight marks one column present and pins its highest block.
func (s *kernelMeshViewSection) setHeight(x, z int32, highest int16) {
	column := int((x+16)>>4)*3 + int((z+16)>>4)
	cell := int((z+16)&15)<<4 | int((x+16)&15)
	s.heightsPresent[column] = true
	s.heights[column*256+cell] = highest
}

// setAllHeights marks all nine columns present, so every neighborhood cell is
// a direct sky candidate above the pinned highest block.
func (s *kernelMeshViewSection) setAllHeights(highest int16) {
	for column := range s.heightsPresent {
		s.heightsPresent[column] = true
		for cell := range 256 {
			s.heights[column*256+cell] = highest
		}
	}
}

func (s *kernelMeshViewSection) encode() []byte {
	wordsPerRow := (len(s.entries) + 63) / 64
	length := kernelMeshViewHeaderBytes + kernelMeshViewBlocksBytes +
		kernelMeshViewHeightColumns + kernelMeshViewHeightsBytes +
		len(s.entries)*kernelMeshViewRegistryEntryBytes + len(s.visibility)*8
	input := make([]byte, length)
	copy(input[0:4], "MGM1")
	binary.LittleEndian.PutUint32(input[4:8], uint32(s.originY))
	binary.LittleEndian.PutUint16(input[8:10], uint16(len(s.entries)))
	binary.LittleEndian.PutUint16(input[10:12], uint16(wordsPerRow))
	binary.LittleEndian.PutUint16(input[12:14], s.airID)
	binary.LittleEndian.PutUint16(input[14:16], s.barrierID)

	offset := kernelMeshViewHeaderBytes
	for _, block := range s.blocks {
		binary.LittleEndian.PutUint16(input[offset:], block)
		offset += 2
	}
	for _, present := range s.heightsPresent {
		if present {
			input[offset] = 1
		}
		offset++
	}
	for _, height := range s.heights {
		binary.LittleEndian.PutUint16(input[offset:], uint16(height))
		offset += 2
	}
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
		offset += kernelMeshViewRegistryEntryBytes
	}
	for _, word := range s.visibility {
		binary.LittleEndian.PutUint64(input[offset:], word)
		offset += 8
	}
	return input
}

// kernelMeshViewRegistry is the shared registry: id 0 air, id 1 opaque
// barrier, id 2 emitter with the requested emission and id 3 a transmissive
// short-grass plant.
func kernelMeshViewRegistry(emission uint8) []kernelMeshEntry {
	return []kernelMeshEntry{
		{id: 0, material: [6]uint16{0, 0, 0, 0, 0, 0}},
		{id: 1, opaque: true, material: [6]uint16{1, 1, 1, 1, 1, 1}},
		{id: 2, emission: emission, material: [6]uint16{3, 3, 3, 3, 3, 3}},
		{id: 3, material: [6]uint16{68, 68, 68, 68, 68, 68}},
	}
}

// kernelMeshViewRun invokes the raw ABI once with caller-owned scratch and
// output buffers.
func kernelMeshViewRun(input []byte, scratch, output []uint64) (nativeabi.Status, int) {
	return nativeabi.MeshSection(nativeabi.ABIVersion, input, scratch, output)
}

// kernelMeshViewPackedQuads decodes the used prefix of the output into packed
// words.
func kernelMeshViewPackedQuads(output []uint64, count int) []uint64 {
	return output[:count]
}

// kernelMeshViewLight returns the 8-bit light field of one packed quad.
func kernelMeshViewLight(packed uint64) uint8 { return uint8(packed >> 47) }

// kernelMeshViewDigest fingerprints the used output bytes with FNV-1a 64-bit.
func kernelMeshViewDigest(output []uint64) uint64 {
	used := make([]byte, len(output)*8)
	for index, word := range output {
		binary.LittleEndian.PutUint64(used[index*8:], word)
	}
	hash := fnv.New64a()
	hash.Write(used)
	return hash.Sum64()
}

// TestKernelMeshView records release-ABI observations of the raw mesh/light
// section entry: the structural-only all-air shortcut, registry capacity and
// semantic ranges, packed light/ambient fields, the exactly filled sky queue
// and scratch reuse after a bright build.
func TestKernelMeshView(t *testing.T) {
	newScratch := func() []uint64 { return make([]uint64, kernelMeshViewScratchWords) }
	newOutput := func() []uint64 {
		output := make([]uint64, kernelMeshViewOutputWords)
		for index := range output {
			output[index] = kernelMeshViewCanary
		}
		return output
	}

	// 1. A semantic-only registry violation (attenuation 2) is accepted while
	// the center section is air: the raw entry only runs the structural parse
	// and the all-air shortcut before that validation, and leaves output
	// untouched.
	shortcut := newKernelMeshViewSection(kernelMeshViewRegistry(15))
	shortcut.entries[2].lightAttenuation = 2
	scratch := newScratch()
	output := newOutput()
	status, count := kernelMeshViewRun(shortcut.encode(), scratch, output)
	if status != nativeabi.StatusOK || count != 0 {
		t.Fatalf("all-air shortcut: status/count=%d/%d, want OK/0", status, count)
	}
	for index, word := range output {
		if word != kernelMeshViewCanary {
			t.Fatalf("all-air shortcut wrote output[%d]=%#x", index, word)
		}
	}

	// 2. The same semantic violation is rejected as soon as the center holds a
	// non-air block, so the shortcut is structural-only.
	shortcut.setBlock(0, 0, 0, 1)
	status, count = kernelMeshViewRun(shortcut.encode(), newScratch(), newOutput())
	if status != nativeabi.StatusRegistry || count != 0 {
		t.Fatalf("non-air semantic violation: status/count=%d/%d, want REGISTRY/0", status, count)
	}

	// 3. Registry capacity: 96 entries are accepted, 97 are rejected.
	atCapacity := make([]kernelMeshEntry, 96)
	for index := range atCapacity {
		atCapacity[index] = kernelMeshEntry{id: uint16(index)}
	}
	capacity := newKernelMeshViewSection(atCapacity)
	capacity.setBlock(0, 0, 0, 1)
	capacity.markAirFacesVisible(1)
	status, count = kernelMeshViewRun(capacity.encode(), newScratch(), newOutput())
	if status != nativeabi.StatusOK || count == 0 {
		t.Fatalf("96 entries: status/count=%d/%d, want OK/nonzero", status, count)
	}
	over := make([]kernelMeshEntry, 97)
	for index := range over {
		over[index] = kernelMeshEntry{id: uint16(index)}
	}
	status, count = kernelMeshViewRun(newKernelMeshViewSection(over).encode(), newScratch(), newOutput())
	if status != nativeabi.StatusRegistry || count != 0 {
		t.Fatalf("97 entries: status/count=%d/%d, want REGISTRY/0", status, count)
	}

	// 4. Semantic entry ranges on the raw entry: emission 16, model 7 and
	// fluid height 15 are rejected; fluid height 1 stays accepted.
	for _, tc := range []struct {
		name   string
		mutate func(*kernelMeshViewSection)
		want   nativeabi.Status
	}{
		{"emission 16", func(s *kernelMeshViewSection) { s.entries[2].emission = 16 }, nativeabi.StatusEmission},
		{"model 7", func(s *kernelMeshViewSection) { s.entries[2].model = 7 }, nativeabi.StatusRegistry},
		{"fluid height 15", func(s *kernelMeshViewSection) { s.entries[2].fluidHeight = 15 }, nativeabi.StatusRegistry},
		{"fluid height 1", func(s *kernelMeshViewSection) { s.entries[2].fluidHeight = 1 }, nativeabi.StatusOK},
	} {
		section := newKernelMeshViewSection(kernelMeshViewRegistry(15))
		section.setBlock(0, 0, 0, 1)
		section.markAirFacesVisible(1, 2, 3)
		tc.mutate(section)
		status, count = kernelMeshViewRun(section.encode(), newScratch(), newOutput())
		if status != tc.want {
			t.Fatalf("%s: status=%d, want %d", tc.name, status, tc.want)
		}
		if tc.want == nativeabi.StatusOK && count == 0 {
			t.Fatalf("%s: accepted but published no quads", tc.name)
		}
	}

	// 5. A stone floor with one emitter publishes ambient/light fields; the
	// emitter's neighbors must carry a nonzero block light.
	lit := newKernelMeshViewSection(kernelMeshViewRegistry(15))
	for x := int32(0); x < 16; x++ {
		for z := int32(0); z < 16; z++ {
			lit.setBlock(x, 0, z, 1)
		}
	}
	lit.setBlock(8, 1, 8, 2)
	lit.markAirFacesVisible(1, 2, 3)
	litOutput := newOutput()
	status, count = kernelMeshViewRun(lit.encode(), newScratch(), litOutput)
	if status != nativeabi.StatusOK || count == 0 {
		t.Fatalf("lit floor: status/count=%d/%d, want OK/nonzero", status, count)
	}
	litQuads := kernelMeshViewPackedQuads(litOutput, count)
	litLight := false
	for _, quad := range litQuads {
		litLight = litLight || kernelMeshViewLight(quad) > 0
	}
	if !litLight {
		t.Fatal("lit floor: no packed quad carries block light")
	}
	if count != kernelMeshViewLitCount || kernelMeshViewDigest(litQuads) != kernelMeshViewLitDigest {
		t.Fatalf(
			"lit floor: count/digest=%d/%#x, want %d/%#x",
			count, kernelMeshViewDigest(litQuads), kernelMeshViewLitCount, kernelMeshViewLitDigest,
		)
	}

	// 6. A fully transmissive center section under a clear sky fills the light
	// queue exactly: every one of the 48^3 cells is a direct sky seed, so an
	// overflow would have to come from duplicated enqueues.
	sky := newKernelMeshViewSection(kernelMeshViewRegistry(0))
	sky.fillBlock(3)
	sky.markAirFacesVisible(1, 2, 3)
	sky.setAllHeights(-17)
	skyOutput := newOutput()
	status, count = kernelMeshViewRun(sky.encode(), newScratch(), skyOutput)
	if status != nativeabi.StatusOK || count == 0 {
		t.Fatalf("sky fill: status/count=%d/%d, want OK/nonzero", status, count)
	}
	skyQuads := kernelMeshViewPackedQuads(skyOutput, count)
	for _, quad := range skyQuads {
		if kernelMeshViewLight(quad)>>4 != 15 {
			t.Fatalf("sky fill: packed sky light=%d, want 15", kernelMeshViewLight(quad)>>4)
		}
	}
	if count != kernelMeshViewSkyCount || kernelMeshViewDigest(skyQuads) != kernelMeshViewSkyDigest {
		t.Fatalf(
			"sky fill: count/digest=%d/%#x, want %d/%#x",
			count, kernelMeshViewDigest(skyQuads), kernelMeshViewSkyCount, kernelMeshViewSkyDigest,
		)
	}

	// 7. Reusing one scratch across a bright and then a dark build must match a
	// fresh dark build exactly, and every dark quadrant must be unlit.
	dark := newKernelMeshViewSection(kernelMeshViewRegistry(0))
	for x := int32(0); x < 16; x++ {
		for z := int32(0); z < 16; z++ {
			dark.setBlock(x, 0, z, 1)
		}
	}
	dark.setBlock(8, 1, 8, 2)
	dark.markAirFacesVisible(1, 2, 3)
	reusedScratch := newScratch()
	reusedOutput := newOutput()
	if status, _ = kernelMeshViewRun(lit.encode(), reusedScratch, reusedOutput); status != nativeabi.StatusOK {
		t.Fatalf("bright reuse pass: status=%d", status)
	}
	reusedOutput = newOutput()
	status, count = kernelMeshViewRun(dark.encode(), reusedScratch, reusedOutput)
	if status != nativeabi.StatusOK || count == 0 {
		t.Fatalf("dark rebuild: status/count=%d/%d, want OK/nonzero", status, count)
	}
	freshOutput := newOutput()
	freshStatus, freshCount := kernelMeshViewRun(dark.encode(), newScratch(), freshOutput)
	if freshStatus != nativeabi.StatusOK || freshCount != count {
		t.Fatalf("fresh dark build: status/count=%d/%d, want OK/%d", freshStatus, freshCount, count)
	}
	if !bytes.Equal(
		flattenKernelMeshViewQuads(reusedOutput[:count]),
		flattenKernelMeshViewQuads(freshOutput[:count]),
	) {
		t.Fatal("dark rebuild on reused scratch diverged from a fresh scratch")
	}
	for _, quad := range reusedOutput[:count] {
		// AO is geometry-driven and legitimately nonzero where the lamp meets
		// the floor; the light field is the part a stale build would leak.
		if kernelMeshViewLight(quad) != 0 {
			t.Fatalf("dark rebuild retained light bits: %#x", quad)
		}
	}
	if count != kernelMeshViewDarkCount ||
		kernelMeshViewDigest(reusedOutput[:count]) != kernelMeshViewDarkDigest {
		t.Fatalf(
			"dark rebuild: count/digest=%d/%#x, want %d/%#x",
			count, kernelMeshViewDigest(reusedOutput[:count]), kernelMeshViewDarkCount, kernelMeshViewDarkDigest,
		)
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
			{"mesh_view_lit", lit.encode(), litQuads},
			{"mesh_view_sky", sky.encode(), skyQuads},
			{"mesh_view_dark", dark.encode(), reusedOutput[:count]},
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

// flattenKernelMeshViewQuads renders packed quads to little-endian bytes for
// byte-exact comparison and export.
func flattenKernelMeshViewQuads(quads []uint64) []byte {
	raw := make([]byte, len(quads)*8)
	for index, word := range quads {
		binary.LittleEndian.PutUint64(raw[index*8:], word)
	}
	return raw
}
