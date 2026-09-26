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

// Frozen `MGW1` lod-shell framing on top of the shared 566-byte worldgen
// header: tile_x i32 + tile_z i32 + columns u32 (fixed 64) + lod_step u32 in
// {2, 4, 8}. The output is an ordered stream of 20-byte little-endian quads:
// x/z/y i32 + w/d u16 + face u8 + material u16 + shade u8.
const (
	kernelLodQuadBytes   = 20
	kernelLodTileColumns = 64
	kernelLodInputBytes  = kernelWorldgenHeaderBytes + 16
)

// kernelLodQuad is one decoded ABI shell quad.
type kernelLodQuad struct {
	x, z, y  int32
	w, d     uint16
	face     uint8
	material uint16
	shade    uint8
}

// kernelLodInput builds a valid lod-shell request with the release-ABI
// observation header: layout 3, material table ids 1..=15 in wire order,
// identity permutation and the requested tile coordinates. gatedFluid sets the
// water entry to the air entry, the wire form of the fluid-disabled gate. The
// shell path reads only terrain heights and surface materials, so the header
// seed never reaches the output.
func kernelLodInput(tileX, tileZ int32, step uint32, gatedFluid bool) []byte {
	input := make([]byte, kernelLodInputBytes)
	copy(input[0:4], "MGW1")
	binary.LittleEndian.PutUint32(input[4:8], 3)
	binary.LittleEndian.PutUint64(input[8:16], 0)
	minY := int32(-64)
	binary.LittleEndian.PutUint32(input[16:20], uint32(minY))
	binary.LittleEndian.PutUint32(input[20:24], 320)
	for index := 0; index < 15; index++ {
		binary.LittleEndian.PutUint16(input[24+index*2:26+index*2], uint16(index+1))
	}
	if gatedFluid {
		binary.LittleEndian.PutUint16(input[24+13*2:26+13*2], 1)
	}
	for index := 0; index < 512; index++ {
		input[54+index] = byte(index & 255)
	}
	binary.LittleEndian.PutUint32(input[kernelWorldgenHeaderBytes:], uint32(tileX))
	binary.LittleEndian.PutUint32(input[kernelWorldgenHeaderBytes+4:], uint32(tileZ))
	binary.LittleEndian.PutUint32(input[kernelWorldgenHeaderBytes+8:], kernelLodTileColumns)
	binary.LittleEndian.PutUint32(input[kernelWorldgenHeaderBytes+12:], step)
	return input
}

// kernelLodDecode decodes the ordered quads of a shell byte stream.
func kernelLodDecode(output []byte) []kernelLodQuad {
	count := len(output) / kernelLodQuadBytes
	quads := make([]kernelLodQuad, count)
	for index := 0; index < count; index++ {
		base := index * kernelLodQuadBytes
		quads[index] = kernelLodQuad{
			x:        int32(binary.LittleEndian.Uint32(output[base:])),
			z:        int32(binary.LittleEndian.Uint32(output[base+4:])),
			y:        int32(binary.LittleEndian.Uint32(output[base+8:])),
			w:        binary.LittleEndian.Uint16(output[base+12:]),
			d:        binary.LittleEndian.Uint16(output[base+14:]),
			face:     output[base+16],
			material: binary.LittleEndian.Uint16(output[base+17:]),
			shade:    output[base+19],
		}
	}
	return quads
}

// kernelLodDigest fingerprints a shell byte stream with FNV-1a 64-bit. The
// Rust numerical-migration lod test implements the same digest over the same
// encoded quads so both sides pin one shared release-ABI value.
func kernelLodDigest(output []byte) uint64 {
	hash := fnv.New64a()
	hash.Write(output)
	return hash.Sum64()
}

// kernelLodTopCovering finds the top quad covering a world column.
func kernelLodTopCovering(tops []kernelLodQuad, x, z int32) (kernelLodQuad, bool) {
	for _, quad := range tops {
		if quad.x <= x && x < quad.x+int32(quad.w) && quad.z <= z && z < quad.z+int32(quad.d) {
			return quad, true
		}
	}
	return kernelLodQuad{}, false
}

// kernelLodCheckOrdered walks the ordered shell contract: tops first in
// strictly increasing (z, x) start order, then X skirts, then Z skirts. Every
// skirt closes its height step exactly and the taller side owns the wall
// (face, y, w, d, material and shade follow from the tops it connects). At a
// tile boundary the lower side is the neighbor's window and has no top here,
// so only the taller side is checked; that is exactly the case where the
// taller side keeps the skirt.
func kernelLodCheckOrdered(t *testing.T, name string, tileX, tileZ int32, quads []kernelLodQuad) {
	t.Helper()
	baseX := tileX * kernelLodTileColumns
	baseZ := tileZ * kernelLodTileColumns
	var tops []kernelLodQuad
	phase := 0
	prevX, prevZ := int32(0), int32(0)
	havePrev := false
	for index, quad := range quads {
		var nextPhase int
		var keyX, keyZ int32
		switch quad.face {
		case 0:
			if quad.shade != 255 {
				t.Errorf("%s: quad %d top shade %d want 255", name, index, quad.shade)
			}
			tops = append(tops, quad)
			nextPhase, keyX, keyZ = 0, quad.z, quad.x
		case 1, 2:
			if quad.shade != 153 {
				t.Errorf("%s: quad %d x-skirt shade %d want 153", name, index, quad.shade)
			}
			nextPhase, keyX, keyZ = 1, quad.z, quad.x
		default:
			if quad.shade != 204 {
				t.Errorf("%s: quad %d z-skirt shade %d want 204", name, index, quad.shade)
			}
			nextPhase, keyX, keyZ = 2, quad.x, quad.z
		}
		if nextPhase < phase {
			t.Fatalf("%s: quad %d breaks the tops, X skirts, Z skirts grouping", name, index)
		}
		if nextPhase != phase {
			phase, havePrev = nextPhase, false
		}
		// Inside a group the start key is (z, x) for tops and X skirts and
		// (x, z) for Z skirts; strict monotonicity pins the emission order.
		if havePrev && !(prevX < keyX || (prevX == keyX && prevZ < keyZ)) {
			t.Errorf("%s: quad %d is not strictly ordered after (%d,%d)", name, index, prevX, prevZ)
		}
		prevX, prevZ, havePrev = keyX, keyZ, true
	}

	for index, quad := range quads {
		if quad.face == 0 {
			continue
		}
		high, ok := kernelLodTopCovering(tops, quad.x, quad.z)
		if !ok {
			t.Fatalf("%s: quad %d wall column is not covered by a top", name, index)
		}
		lowX, lowZ := quad.x, quad.z
		switch quad.face {
		case 2:
			lowX = quad.x + 1
		case 1:
			lowX = quad.x - 1
		case 4:
			lowZ = quad.z + 1
		default:
			lowZ = quad.z - 1
		}
		if int32(quad.d) < 1 || quad.y+int32(quad.d) != high.y+1 {
			t.Errorf("%s: quad %d does not reach the taller top plane", name, index)
		}
		if quad.material != high.material {
			t.Errorf("%s: quad %d material %d want taller side %d", name, index, quad.material, high.material)
		}
		low, covered := kernelLodTopCovering(tops, lowX, lowZ)
		if !covered {
			// The lower side is the neighbor's window; the wall must sit on
			// the tile boundary for this tile to own the skirt.
			onEdge := false
			switch quad.face {
			case 2:
				onEdge = quad.x+1 == baseX+kernelLodTileColumns
			case 1:
				onEdge = quad.x == baseX
			case 4:
				onEdge = quad.z+1 == baseZ+kernelLodTileColumns
			default:
				onEdge = quad.z == baseZ
			}
			if !onEdge {
				t.Errorf("%s: quad %d loses its lower side inside the tile", name, index)
			}
			switch quad.face {
			case 2:
				if high.x+int32(high.w) != quad.x+1 {
					t.Errorf("%s: quad %d wall plane does not meet the taller top", name, index)
				}
			case 1:
				if high.x != quad.x {
					t.Errorf("%s: quad %d wall plane does not meet the taller top", name, index)
				}
			case 4:
				if high.z+int32(high.d) != quad.z+1 {
					t.Errorf("%s: quad %d wall plane does not meet the taller top", name, index)
				}
			default:
				if high.z != quad.z {
					t.Errorf("%s: quad %d wall plane does not meet the taller top", name, index)
				}
			}
			continue
		}
		if high.y <= low.y {
			t.Errorf("%s: quad %d is not owned by the taller side", name, index)
		}
		if quad.y != low.y+1 {
			t.Errorf("%s: quad %d y %d want lower top plane %d", name, index, quad.y, low.y+1)
		}
		if int32(quad.d) != high.y-low.y {
			t.Errorf("%s: quad %d span %d want height step %d", name, index, quad.d, high.y-low.y)
		}
		switch quad.face {
		case 2:
			if high.x+int32(high.w) != quad.x+1 || low.x != quad.x+1 {
				t.Errorf("%s: quad %d wall plane does not meet both tops", name, index)
			}
		case 1:
			if high.x != quad.x || low.x+int32(low.w) != quad.x {
				t.Errorf("%s: quad %d wall plane does not meet both tops", name, index)
			}
		case 4:
			if high.z+int32(high.d) != quad.z+1 || low.z != quad.z+1 {
				t.Errorf("%s: quad %d wall plane does not meet both tops", name, index)
			}
		default:
			if high.z != quad.z || low.z+int32(low.d) != quad.z {
				t.Errorf("%s: quad %d wall plane does not meet both tops", name, index)
			}
		}
	}
}

// TestLodShell records release-ABI observations of `nativeabi.LodShell` and
// locks them as ordered quad fields, exact counts and stream digests, so any
// drift between the Go-visible ABI and the pinned expectations fails here
// first. The Rust numerical-migration lod test pins the same values for the
// typed provider. Empty shells (zero quads) are not expressible on this ABI:
// every sampled window is non-air, so a shell always contains at least its
// merged tops; that shape stays pinned at the shared seams in `src/lod.rs`.
func TestLodShell(t *testing.T) {
	// Pinned observation cases: one tile per step size plus the fluid-disabled
	// counterpart of the sea tile and the all-basin tile that clamps to one
	// uniform top quad. Literals are captured from the Go-oracle observation
	// run and shared verbatim with the Rust numerical-migration lod test.
	cases := []struct {
		name       string
		tileX      int32
		tileZ      int32
		step       uint32
		gatedFluid bool
		wantCount  int
		wantDigest uint64
		wantFirst  kernelLodQuad
		wantLast   kernelLodQuad
	}{
		{
			name: "step two at tile (0,0)", tileX: 0, tileZ: 0, step: 2,
			wantCount: 1284, wantDigest: 0xd84930ff4cee9f89,
			wantFirst: kernelLodQuad{x: 0, z: 0, y: 64, w: 2, d: 2, face: 0, material: 4, shade: 255},
			wantLast:  kernelLodQuad{x: 62, z: 61, y: 77, w: 2, d: 1, face: 4, material: 4, shade: 204},
		},
		{
			name: "step four at tile (-3,2)", tileX: -3, tileZ: 2, step: 4,
			wantCount: 112, wantDigest: 0xaddba620c8cb5bd8,
			wantFirst: kernelLodQuad{x: -192, z: 128, y: 64, w: 64, d: 24, face: 0, material: 14, shade: 255},
			wantLast:  kernelLodQuad{x: -132, z: 168, y: 66, w: 4, d: 2, face: 3, material: 4, shade: 204},
		},
		{
			name: "step eight at tile (7,-5)", tileX: 7, tileZ: -5, step: 8,
			wantCount: 82, wantDigest: 0x23714f969c54b4f7,
			wantFirst: kernelLodQuad{x: 448, z: -320, y: 71, w: 8, d: 8, face: 0, material: 4, shade: 255},
			wantLast:  kernelLodQuad{x: 504, z: -289, y: 65, w: 8, d: 1, face: 4, material: 4, shade: 204},
		},
		{
			name: "sea clamp gated off at tile (-3,2)", tileX: -3, tileZ: 2, step: 4, gatedFluid: true,
			wantCount: 548, wantDigest: 0x4f33d86bc6a16f8b,
			wantFirst: kernelLodQuad{x: -192, z: 128, y: 47, w: 4, d: 8, face: 0, material: 7, shade: 255},
			wantLast:  kernelLodQuad{x: -132, z: 168, y: 66, w: 4, d: 2, face: 3, material: 4, shade: 204},
		},
		{
			name: "uniform single top at tile (-4,1)", tileX: -4, tileZ: 1, step: 4,
			wantCount: 1, wantDigest: 0x94ed4971f2c989c9,
			wantFirst: kernelLodQuad{x: -256, z: 64, y: 64, w: 64, d: 64, face: 0, material: 14, shade: 255},
			wantLast:  kernelLodQuad{x: -256, z: 64, y: 64, w: 64, d: 64, face: 0, material: 14, shade: 255},
		},
	}
	for _, tc := range cases {
		input := kernelLodInput(tc.tileX, tc.tileZ, tc.step, tc.gatedFluid)
		first := nativeabi.LodShell(input)
		second := nativeabi.LodShell(input)
		if !bytes.Equal(first, second) {
			t.Errorf("%s: repeated generation differs", tc.name)
		}
		quads := kernelLodDecode(first)
		kernelLodCheckOrdered(t, tc.name, tc.tileX, tc.tileZ, quads)
		if len(quads) != tc.wantCount {
			t.Errorf("%s: count got %d want %d", tc.name, len(quads), tc.wantCount)
		}
		if got := kernelLodDigest(first); got != tc.wantDigest {
			t.Errorf("%s: digest got %#016x want %#016x", tc.name, got, tc.wantDigest)
		}
		if len(quads) > 0 {
			if quads[0] != tc.wantFirst {
				t.Errorf("%s: first quad got %+v want %+v", tc.name, quads[0], tc.wantFirst)
			}
			if quads[len(quads)-1] != tc.wantLast {
				t.Errorf("%s: last quad got %+v want %+v", tc.name, quads[len(quads)-1], tc.wantLast)
			}
		}
	}

	// Taller-side boundary ownership pin: the first skirt of the step-eight
	// case, with the exact ordered fields observed on the release ABI.
	quads := kernelLodDecode(nativeabi.LodShell(kernelLodInput(7, -5, 8, false)))
	var firstSkirt kernelLodQuad
	found := false
	for _, quad := range quads {
		if quad.face != 0 {
			firstSkirt, found = quad, true
			break
		}
	}
	if !found {
		t.Fatal("the step-eight case must contain a height step")
	}
	wantSkirt := kernelLodQuad{x: 448, z: -320, y: 69, w: 8, d: 3, face: 1, material: 4, shade: 153}
	if firstSkirt != wantSkirt {
		t.Errorf("first skirt got %+v want %+v", firstSkirt, wantSkirt)
	}

	// Sea-clamp pin pair: the clamped water surface of the basin tile and the
	// unclamped terrain top covering the same window once the fluid gate is
	// off.
	clamped := kernelLodDecode(nativeabi.LodShell(kernelLodInput(-3, 2, 4, false)))
	var waterTop kernelLodQuad
	found = false
	for _, quad := range clamped {
		if quad.face == 0 && quad.y == 64 && quad.material == 14 {
			waterTop, found = quad, true
			break
		}
	}
	if !found {
		t.Fatal("the basin tile must contain a clamped water surface")
	}
	wantWater := kernelLodQuad{x: -192, z: 128, y: 64, w: 64, d: 24, face: 0, material: 14, shade: 255}
	if waterTop != wantWater {
		t.Errorf("clamped water top got %+v want %+v", waterTop, wantWater)
	}
	gated := kernelLodDecode(nativeabi.LodShell(kernelLodInput(-3, 2, 4, true)))
	terrainTop, ok := kernelLodTopCovering(gated, waterTop.x, waterTop.z)
	if !ok {
		t.Fatal("the gate-off shell must cover the clamped window")
	}
	wantTerrain := kernelLodQuad{x: -192, z: 128, y: 47, w: 4, d: 8, face: 0, material: 7, shade: 255}
	if terrainTop != wantTerrain {
		t.Errorf("unclamped terrain top got %+v want %+v", terrainTop, wantTerrain)
	}

	// Rejected requests panic before any output is produced. `nativeabi.LodShell`
	// owns its output buffer (static per-step bound plus one overflow retry), so
	// there is no caller-owned buffer to keep untouched here; untouched-output
	// and exact short-output semantics are pinned at the raw layer in
	// `packages/shared/nativeabi/native_test.go`.
	assertPanic := func(t *testing.T, name string, f func()) {
		t.Helper()
		defer func() {
			if recover() == nil {
				t.Errorf("expected panic on %s", name)
			}
		}()
		f()
	}
	validInput := kernelLodInput(-3, 2, 4, false)
	assertPanic(t, "short input", func() {
		nativeabi.LodShell(validInput[:len(validInput)-1])
	})
	assertPanic(t, "bad magic", func() {
		bad := bytes.Clone(validInput)
		bad[0] = 'X'
		nativeabi.LodShell(bad)
	})
	assertPanic(t, "wrong columns", func() {
		bad := bytes.Clone(validInput)
		binary.LittleEndian.PutUint32(bad[574:578], 63)
		nativeabi.LodShell(bad)
	})
	assertPanic(t, "disallowed step", func() {
		bad := bytes.Clone(validInput)
		binary.LittleEndian.PutUint32(bad[578:582], 3)
		nativeabi.LodShell(bad)
	})
	assertPanic(t, "tile overflow", func() {
		bad := bytes.Clone(validInput)
		binary.LittleEndian.PutUint32(bad[566:570], uint32(int32(math.MaxInt32/64+1)))
		nativeabi.LodShell(bad)
	})

	exportDir := os.Getenv("RUNTIME_ORACLE_EXPORT_DIR")
	if exportDir != "" {
		if err := os.MkdirAll(exportDir, 0755); err != nil {
			t.Fatalf("MkdirAll: %v", err)
		}
		if err := os.WriteFile(filepath.Join(exportDir, "lod_input.bin"), validInput, 0644); err != nil {
			t.Fatalf("WriteFile: %v", err)
		}
		if err := os.WriteFile(filepath.Join(exportDir, "lod_output.bin"), nativeabi.LodShell(validInput), 0644); err != nil {
			t.Fatalf("WriteFile: %v", err)
		}
	}
}
