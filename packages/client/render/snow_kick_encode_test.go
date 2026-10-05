package render

import (
	"bytes"
	"testing"

	"github.com/go-gl/mathgl/mgl32"

	"github.com/channing771/mornlea/packages/shared/core"
)

// TestAppendSnowKickInstancesSharesWeatherBudget 证踢雪尘与降水粒子共享
// `WeatherMaxParticles` 预算：雨天降水占满 256 槽时踢雪尘整组让位、总实例数
// 恰为上限；晴天无降水粒子时踢雪尘完整并入；任何组合都不超限（超限帧会被
// Rust 侧 precip pass 整帧拒绝）。
func TestAppendSnowKickInstancesSharesWeatherBudget(t *testing.T) {
	cam := mgl32.Vec3{0, 40, 0}
	kick := SnowKickInput{Kick: true, Feet: mgl32.Vec3{0.5, 39, 0.5}}

	// 晴天：降水流为空，踢雪尘完整并入。
	clearEncoder := &InstanceEncoder{}
	stream := clearEncoder.EncodeWeatherInstances(nil, cam, 0, 100, core.WeatherClear, 0, 0)
	if len(stream) != 0 {
		t.Fatalf("晴天降水流 = %d 字节，想要空", len(stream))
	}
	stream = clearEncoder.AppendSnowKickInstances(stream, 100, kick)
	if count := len(stream) / avatarInstanceBytes; count != SnowKickParticles {
		t.Fatalf("晴天踢雪尘实例数 = %d，想要 %d", count, SnowKickParticles)
	}

	// 雨天：降水占满 256 槽，踢雪尘让位，总数恰为上限且不超限。
	rainEncoder := &InstanceEncoder{}
	stream = rainEncoder.EncodeWeatherInstances(nil, cam, 0, 100, core.WeatherRain, 0, 0)
	if count := len(stream) / avatarInstanceBytes; count != WeatherMaxParticles {
		t.Fatalf("雨天降水实例数 = %d，想要 %d", count, WeatherMaxParticles)
	}
	stream = rainEncoder.AppendSnowKickInstances(stream, 100, kick)
	if count := len(stream) / avatarInstanceBytes; count != WeatherMaxParticles {
		t.Fatalf("雨天并入踢雪尘后实例数 = %d，想要保持 %d（踢雪尘让位）", count, WeatherMaxParticles)
	}
}

// TestAppendSnowKickInstancesDeterministic 证同输入两帧装配逐字节一致：
// 事件沿与 tick 相同的两个编码器走同一调用序列，输出不因缓冲复用漂移。
func TestAppendSnowKickInstancesDeterministic(t *testing.T) {
	cam := mgl32.Vec3{1, 42, 2}
	run := func() []byte {
		encoder := &InstanceEncoder{}
		kick := SnowKickInput{Kick: true, Feet: mgl32.Vec3{3.5, 41, -2.5}}
		stream := encoder.EncodeWeatherInstances(nil, cam, 0.25, 7, core.WeatherClear, 0, 0)
		return encoder.AppendSnowKickInstances(stream, 7, kick)
	}
	first, second := run(), run()
	if !bytes.Equal(first, second) {
		t.Fatal("同输入两帧踢雪尘装配字节不一致")
	}
	if count := len(first) / avatarInstanceBytes; count != SnowKickParticles {
		t.Fatalf("踢雪尘实例数 = %d，想要 %d", count, SnowKickParticles)
	}
}
