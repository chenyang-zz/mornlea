//go:build darwin

package render

import (
	_ "embed"
	"fmt"
	"image"
	"image/draw"
	"strings"
	"sync"

	"golang.org/x/image/font"
	"golang.org/x/image/font/opentype"
	"golang.org/x/image/math/fixed"
)

const (
	glyphAtlasSize       = 1024
	glyphCellSize        = 32
	glyphSlotCount       = 1024
	glyphRequestCapacity = 1024
	glyphResultCapacity  = 32
	glyphUploadBytes     = glyphCellSize * glyphCellSize
	// glyphInkPadding 是 ink 在格内的左上留边（像素）。光栅化按它定位 ink，
	// slot 赋 UV 时也按它定位采样区——两处必须用同一个值，否则采样会整体偏移。
	glyphInkPadding = 2
)

//go:embed assets/NotoSansCJKsc-Regular.otf
var embeddedGlyphFont []byte

//go:embed assets/OFL.txt
var embeddedGlyphLicense string

//go:embed assets/NotoSansCJKsc-Regular.provenance.json
var embeddedGlyphProvenance string

type glyphResult struct {
	char    rune
	glyph   Glyph
	pixels  []byte
	missing bool
	err     error
}

type glyphFaceFactory func() (renderFace, workerFace font.Face, err error)

type glyphRasterizer interface {
	Rasterize(font.Face, rune) (Glyph, []byte, bool, error)
}

// GlyphSink 接收字形图集的 R8 矩形写入。生产实现是 Rust 渲染器的
// 上传入口;gfx 纹理 sink 供切换期的既有 Go renderer 与测试使用。
type GlyphSink interface {
	WriteGlyphRect(x, y, width, height uint32, pixels []byte)
}

// GlyphAtlas owns a bounded 1024x1024 R8 texture split into 32x32 cells.
type GlyphAtlas struct {
	// sink 是唯一的图集写出口(生产 = Rust 渲染器上传入口)。
	sink          GlyphSink
	pendingUpload *glyphResult
	renderFace    font.Face

	mu          sync.Mutex
	requests    chan rune
	results     chan glyphResult
	stop        chan struct{}
	releaseDone chan struct{}
	requested   map[rune]struct{}
	glyphs      map[rune]Glyph
	tofu        Glyph
	nextSlot    uint16
	exhausted   bool
	released    bool
	worker      sync.WaitGroup
}

// NewGlyphAtlasWithSink 创建字形图集:全部矩形写入经 sink
// (生产路径 = Rust 渲染器上传入口)。
func NewGlyphAtlasWithSink(sink GlyphSink) (*GlyphAtlas, error) {
	return newGlyphAtlasSink(sink, embeddedGlyphFaceFactory, opentypeGlyphRasterizer{})
}

func newGlyphAtlasSink(
	sink GlyphSink,
	factory glyphFaceFactory,
	rasterizer glyphRasterizer,
) (*GlyphAtlas, error) {
	renderFace, workerFace, err := factory()
	if err != nil {
		return nil, fmt.Errorf("render: create glyph faces: %w", err)
	}
	if renderFace == nil || workerFace == nil {
		return nil, fmt.Errorf("render: glyph face factory returned nil face")
	}
	return assembleGlyphAtlas(sink, renderFace, workerFace, rasterizer)
}

func assembleGlyphAtlas(
	sink GlyphSink,
	renderFace, workerFace font.Face,
	rasterizer glyphRasterizer,
) (*GlyphAtlas, error) {
	tofu, pixels := tofuGlyph()
	sink.WriteGlyphRect(0, 0, glyphCellSize, glyphCellSize, pixels)

	atlas := &GlyphAtlas{
		sink:        sink,
		renderFace:  renderFace,
		requests:    make(chan rune, glyphRequestCapacity),
		results:     make(chan glyphResult, glyphResultCapacity),
		stop:        make(chan struct{}),
		releaseDone: make(chan struct{}),
		requested:   make(map[rune]struct{}, glyphSlotCount),
		glyphs:      make(map[rune]Glyph, glyphSlotCount),
		tofu:        tofu,
		nextSlot:    1,
	}
	atlas.worker.Add(1)
	go atlas.runWorker(workerFace, rasterizer)
	return atlas, nil
}

func embeddedGlyphFaceFactory() (font.Face, font.Face, error) {
	if err := validateEmbeddedGlyphMetadata(); err != nil {
		return nil, nil, err
	}
	parsed, err := opentype.Parse(embeddedGlyphFontData())
	if err != nil {
		return nil, nil, err
	}
	options := &opentype.FaceOptions{Size: 24, DPI: 72, Hinting: font.HintingFull}
	renderFace, err := opentype.NewFace(parsed, options)
	if err != nil {
		return nil, nil, err
	}
	workerFace, err := opentype.NewFace(parsed, options)
	if err != nil {
		return nil, nil, err
	}
	return renderFace, workerFace, nil
}

func embeddedGlyphFontData() []byte {
	return embeddedGlyphFont
}

// EmbeddedCJKFont 返回内嵌 Noto Sans CJK OTF 字体字节的**只读副本**。菜单
// 字体上传出口已随 client ABI v12 退役,本访问器仅由溯源测试用于锁定嵌入字节
// 的长度与哈希。返回副本而非直接返回嵌入切片,是为了保证调用方无法改写共享
// 的嵌入字节——同一份嵌入字节同时供字形图集生成使用,被改写会污染全部字形。
func EmbeddedCJKFont() []byte {
	out := make([]byte, len(embeddedGlyphFont))
	copy(out, embeddedGlyphFont)
	return out
}

func validateEmbeddedGlyphMetadata() error {
	if !strings.Contains(embeddedGlyphLicense, "SIL OPEN FONT LICENSE Version 1.1 - 26 February 2007") {
		return fmt.Errorf("render: embedded glyph license metadata is missing or invalid")
	}
	if !strings.Contains(embeddedGlyphProvenance, "\"repository\": \"notofonts/noto-cjk\"") {
		return fmt.Errorf("render: embedded glyph provenance metadata is missing or invalid")
	}
	return nil
}

func tofuGlyph() (Glyph, []byte) {
	pixels := make([]byte, glyphUploadBytes)
	for y := 4; y < glyphCellSize-4; y++ {
		for x := 5; x < glyphCellSize-5; x++ {
			if x < 8 || x >= glyphCellSize-8 || y < 7 || y >= glyphCellSize-7 {
				pixels[y*glyphCellSize+x] = 0xff
			}
		}
	}
	return Glyph{
		Slot:     0,
		U0:       0,
		V0:       0,
		U1:       float32(glyphCellSize) / float32(glyphAtlasSize),
		V1:       float32(glyphCellSize) / float32(glyphAtlasSize),
		Advance:  22,
		BearingX: 5,
		BearingY: 25,
		Width:    22,
		Height:   24,
	}, pixels
}

// Request schedules unseen runes without ever blocking the caller.
func (atlas *GlyphAtlas) Request(text string) {
	atlas.mu.Lock()
	defer atlas.mu.Unlock()
	if atlas.released || atlas.exhausted {
		return
	}
	for _, char := range text {
		if _, ok := atlas.glyphs[char]; ok {
			continue
		}
		if _, ok := atlas.requested[char]; ok {
			continue
		}
		select {
		case atlas.requests <- char:
			atlas.requested[char] = struct{}{}
		default:
			// A rejected rune remains unregistered so a later frame can retry it.
		}
	}
}

func (atlas *GlyphAtlas) runWorker(face font.Face, rasterizer glyphRasterizer) {
	defer atlas.worker.Done()
	for {
		select {
		case <-atlas.stop:
			return
		default:
		}

		select {
		case <-atlas.stop:
			return
		case char, ok := <-atlas.requests:
			if !ok {
				return
			}
			glyph, pixels, missing, err := rasterizer.Rasterize(face, char)
			result := glyphResult{char: char, glyph: glyph, pixels: pixels, missing: missing, err: err}
			select {
			case atlas.results <- result:
			case <-atlas.stop:
				return
			}
		}
	}
}

// FlushUploads moves at most one worker result to render-thread state.
func (atlas *GlyphAtlas) FlushUploads(budget *UploadBudget) error {
	atlas.mu.Lock()
	defer atlas.mu.Unlock()
	if atlas.released {
		return nil
	}
	if atlas.pendingUpload == nil {
		select {
		case result := <-atlas.results:
			atlas.pendingUpload = &result
		default:
			return nil
		}
	}

	pending := atlas.pendingUpload
	if pending.err != nil {
		atlas.pendingUpload = nil
		return fmt.Errorf("render: rasterize glyph %q: %w", pending.char, pending.err)
	}
	if pending.missing || atlas.exhausted {
		atlas.glyphs[pending.char] = atlas.tofu
		atlas.pendingUpload = nil
		return nil
	}
	if budget == nil {
		return fmt.Errorf("render: glyph upload budget is nil")
	}
	if !budget.TryConsume(glyphUploadBytes) {
		return nil
	}

	slot := atlas.nextSlot
	x := uint32(slot%32) * glyphCellSize
	y := uint32(slot/32) * glyphCellSize
	atlas.sink.WriteGlyphRect(x, y, glyphCellSize, glyphCellSize, pending.pixels)
	pending.glyph.Slot = slot
	// UV 只覆盖 ink 本身，不覆盖整格。
	//
	// 光栅化把 ink 画在格内 (glyphInkPadding, glyphInkPadding) 处，尺寸为
	// Glyph.Width × Glyph.Height；而全部消费方（hotbar、name tag、调试面板）
	// 都按 Width × Height 画四边形并直接用这里的 UV 采样。若 UV 覆盖整格，
	// 整格（绝大部分是空白）就会被压进 ink 大小的四边形，压缩比为
	// Width/glyphCellSize——'w' 约 0.56 只是变形，'i' 约 0.09 会让 3px 墨迹
	// 缩成 0.26px 而彻底消失，表现为窄字符成片丢失、宽字符偏细。
	// 只覆盖 ink 才能让四边形与图集像素 1:1 对应。
	inkX := float32(x) + glyphInkPadding
	inkY := float32(y) + glyphInkPadding
	pending.glyph.U0 = inkX / float32(glyphAtlasSize)
	pending.glyph.V0 = inkY / float32(glyphAtlasSize)
	pending.glyph.U1 = (inkX + pending.glyph.Width) / float32(glyphAtlasSize)
	pending.glyph.V1 = (inkY + pending.glyph.Height) / float32(glyphAtlasSize)
	atlas.glyphs[pending.char] = pending.glyph
	atlas.pendingUpload = nil
	atlas.nextSlot++
	if atlas.nextSlot >= glyphSlotCount {
		atlas.exhausted = true
	}
	return nil
}

// Glyph returns tofu for runes that are missing, pending, or beyond capacity.
func (atlas *GlyphAtlas) Glyph(char rune) Glyph {
	atlas.mu.Lock()
	defer atlas.mu.Unlock()
	if glyph, ok := atlas.glyphs[char]; ok {
		return glyph
	}
	return atlas.tofu
}

// Kern returns the render-thread face's kerning adjustment in pixels.
func (atlas *GlyphAtlas) Kern(left, right rune) float32 {
	atlas.mu.Lock()
	defer atlas.mu.Unlock()
	if atlas.released || atlas.renderFace == nil {
		return 0
	}
	return float32(atlas.renderFace.Kern(left, right)) / 64
}

// Release stops the worker exactly once.
func (atlas *GlyphAtlas) Release() {
	atlas.mu.Lock()
	if atlas.released {
		done := atlas.releaseDone
		atlas.mu.Unlock()
		<-done
		return
	}
	atlas.released = true
	close(atlas.requests)
	close(atlas.stop)
	atlas.mu.Unlock()

	atlas.worker.Wait()

	atlas.mu.Lock()
	atlas.renderFace = nil
	atlas.pendingUpload = nil
	atlas.mu.Unlock()
	close(atlas.releaseDone)
}

type opentypeGlyphRasterizer struct{}

func (opentypeGlyphRasterizer) Rasterize(face font.Face, char rune) (Glyph, []byte, bool, error) {
	bounds, advance, ok := face.GlyphBounds(char)
	if !ok {
		return Glyph{}, nil, true, nil
	}
	dot := fixed.Point26_6{
		X: fixed.I(glyphInkPadding) - bounds.Min.X,
		Y: fixed.I(glyphInkPadding) - bounds.Min.Y,
	}
	drawRect, mask, maskPoint, _, ok := face.Glyph(dot, char)
	if !ok {
		return Glyph{}, nil, true, nil
	}
	cell := image.NewAlpha(image.Rect(0, 0, glyphCellSize, glyphCellSize))
	clipped := drawRect.Intersect(cell.Bounds())
	if !clipped.Empty() {
		maskPoint = maskPoint.Add(clipped.Min.Sub(drawRect.Min))
		draw.DrawMask(cell, clipped, image.White, image.Point{}, mask, maskPoint, draw.Src)
	}
	return Glyph{
		Advance:  float32(advance) / 64,
		BearingX: float32(bounds.Min.X) / 64,
		BearingY: -float32(bounds.Min.Y) / 64,
		Width:    float32(bounds.Max.X-bounds.Min.X) / 64,
		Height:   float32(bounds.Max.Y-bounds.Min.Y) / 64,
	}, cell.Pix, false, nil
}
