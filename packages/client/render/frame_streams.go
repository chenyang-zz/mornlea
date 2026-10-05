//go:build darwin

package render

// EncodeProjectileInstances 把投射物镜像呈现编码为 96 字节/实例的字节流
// （与 avatar 同布局）：每枚投射物恰一个 cuboid 实例，长轴沿权威速度估计
// 取向，配色按弹种固定。输入应为镜像的 ID 升序呈现列表；段预算
// `MaxProjectileInstances` 之外的部分按输入顺序丢弃尾部。投射物无跨帧跟
// 踪表，会话重置无需清理。dst 会被重置复用。
func (e *InstanceEncoder) EncodeProjectileInstances(dst []byte, projectiles []Projectile) []byte {
	if len(projectiles) > MaxProjectileInstances {
		projectiles = projectiles[:MaxProjectileInstances]
	}
	e.parts = e.parts[:0]
	for _, projectile := range projectiles {
		e.parts = buildProjectileParts(e.parts, projectile)
	}
	dst = growEncodeBuffer(dst, len(e.parts)*avatarInstanceBytes)
	encodeAvatarPartsInto(dst, e.parts)
	return dst
}

// FrameStreams 返回 Prepare 之后已编码的名牌背景与字形实例字节
// (只读视图,下一次 Prepare 前有效)。
func (renderer *NameTagRenderer) FrameStreams() (backgrounds, glyphs []byte) {
	backgrounds = renderer.upload[nameTagBackgroundOffset : nameTagBackgroundOffset+
		len(renderer.layout.backgrounds)*nameTagInstanceBytes]
	glyphs = renderer.upload[nameTagGlyphOffset : nameTagGlyphOffset+
		len(renderer.layout.glyphs)*nameTagInstanceBytes]
	return backgrounds, glyphs
}

// GlyphAtlasSize 导出字形图集边长,供装配方校验。
const GlyphAtlasSize = glyphAtlasSize

// NewNameTagLayouter 创建 layout-only 的名牌 renderer:只支持 Prepare 与
// FrameStreams,不创建任何 GPU 资源(生产切换后由 Rust 渲染器绘制)。
func NewNameTagLayouter(atlas GlyphSource) *NameTagRenderer {
	return &NameTagRenderer{
		atlas:   atlas,
		ordered: make([]NameTag, 0, maxNameTags),
		upload:  make([]byte, nameTagUploadBytes),
		layout: nameTagLayout{
			glyphs:      make([]nameTagGlyph, 0, maxNameTagGlyphs),
			backgrounds: make([]nameTagBackground, 0, maxNameTags),
		},
	}
}
