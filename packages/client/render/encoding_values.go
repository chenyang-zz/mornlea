package render

// `Glyph` describes one cell in the fixed-size glyph atlas.
type Glyph struct {
	Slot                        uint16
	U0, V0, U1, V1              float32
	Advance, BearingX, BearingY float32
	Width, Height               float32
}

// `growEncodeBuffer` reuses the CPU encoding buffer independently of the GPU backend.
func growEncodeBuffer(dst []byte, size int) []byte {
	if cap(dst) < size {
		return make([]byte, size)
	}
	return dst[:size]
}
