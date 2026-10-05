package render

import (
	"testing"

	"github.com/go-gl/mathgl/mgl32"

	"github.com/channing771/mornlea/packages/shared/core"
)

// `TestEncodeWeatherInstancesSteadyFrameZeroAlloc` pins the production weather
// encoder's zero-allocation behavior after its reusable buffers are warmed.
// `TestBuildWeatherPartsSteadyFrameZeroAlloc` in weather_test.go covers the
// platform-independent parts; this test covers their production encoding path.
func TestEncodeWeatherInstancesSteadyFrameZeroAlloc(t *testing.T) {
	var encoder InstanceEncoder
	cam := mgl32.Vec3{0, 40, 0}
	dst := encoder.EncodeWeatherInstances(nil, cam, 0, 99, core.WeatherRain, 0, 6000)
	if len(dst) != WeatherMaxParticles*avatarInstanceBytes {
		t.Fatalf("雨天字节数 = %d，想要 %d", len(dst), WeatherMaxParticles*avatarInstanceBytes)
	}
	allocs := testing.AllocsPerRun(20, func() {
		encoder.EncodeWeatherInstances(dst[:0], cam, 0, 99, core.WeatherRain, 0, 6000)
	})
	if allocs != 0 {
		t.Fatalf("生产入口稳定天气帧分配 = %v，想要 0", allocs)
	}
}
