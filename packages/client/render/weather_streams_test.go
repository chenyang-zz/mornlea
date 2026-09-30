//go:build darwin

package render

import (
	"testing"

	"github.com/channing771/mornlea/packages/shared/core"
)

// TestEncodeWeatherStateSteadyFrameZeroAlloc 状态段稳定复用零分配：预热后
// 同一 `dst` 的雨天编码不分配；晴天截断同样不分配。
func TestEncodeWeatherStateSteadyFrameZeroAlloc(t *testing.T) {
	dst := EncodeWeatherState(nil, core.WeatherRain)
	if len(dst) != weatherStateBytes {
		t.Fatalf("雨天状态段长度 = %d，想要 %d", len(dst), weatherStateBytes)
	}
	allocs := testing.AllocsPerRun(20, func() {
		EncodeWeatherState(dst[:weatherStateBytes], core.WeatherRain)
	})
	if allocs != 0 {
		t.Fatalf("状态段稳定复用分配 = %v，想要 0", allocs)
	}
	allocs = testing.AllocsPerRun(20, func() {
		EncodeWeatherState(dst[:weatherStateBytes], core.WeatherClear)
	})
	if allocs != 0 {
		t.Fatalf("晴天状态截断分配 = %v，想要 0", allocs)
	}
}
