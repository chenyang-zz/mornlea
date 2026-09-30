package render

import (
	"bytes"
	"testing"

	"github.com/channing771/mornlea/packages/shared/core"
)

// TestEncodeBreakBurstInstancesMatchesDropEncoding 钉住 burst 字节流与 avatar
// pass 的实例契约一致:新掉落物首帧恰 8 实例、每实例 96 字节;同输入逐帧字节一致。
func TestEncodeBreakBurstInstancesMatchesDropEncoding(t *testing.T) {
	encoder := &InstanceEncoder{}
	drops := []ItemDrop{breakTestDrop(3, core.BlockPos{X: 0, Y: 3, Z: 0}, core.ItemDirt)}
	first := encoder.EncodeBreakBurstInstances(nil, 10, drops)
	if len(first) != 8*avatarInstanceBytes {
		t.Fatalf("burst 流 = %d 字节，想要 %d", len(first), 8*avatarInstanceBytes)
	}
	repeat := encoder.EncodeBreakBurstInstances(nil, 10, drops)
	if !bytes.Equal(first, repeat) {
		t.Fatal("同一 tick 两次 burst 编码字节不一致")
	}
	if later := encoder.EncodeBreakBurstInstances(nil, 11, drops); bytes.Equal(first, later) {
		t.Fatal("tick 推进后 burst 编码字节未变化")
	}
}

// TestAppendBreakBurstInstancesRespectsAvatarBudget 钉住 avatar 段预算：
// 身体占满时 burst 让路、总段恒不超过 450 实例（超限帧会被 Rust 侧整体拒绝）；
// 空位不足一个整 burst 时并入零字节，身体实例恒优先保留。
func TestAppendBreakBurstInstancesRespectsAvatarBudget(t *testing.T) {
	encoder := &InstanceEncoder{}
	drops := []ItemDrop{breakTestDrop(3, core.BlockPos{X: 0, Y: 3, Z: 0}, core.ItemDirt)}
	full := make([]byte, maxAvatarParts*avatarInstanceBytes)
	if got := encoder.AppendBreakBurstInstances(full, 10, drops); len(got) != len(full) {
		t.Fatalf("占满段并入后=%d 字节，想要不变 %d", len(got), len(full))
	}
	roomForOne := make([]byte, (maxAvatarParts-breakBurstParticlesPerBurst)*avatarInstanceBytes)
	if got := encoder.AppendBreakBurstInstances(roomForOne, 11, drops); len(got) != maxAvatarParts*avatarInstanceBytes {
		t.Fatalf("留 8 空位并入后=%d 字节，想要装满 %d", len(got), maxAvatarParts*avatarInstanceBytes)
	}
	roomForPartial := make([]byte, (maxAvatarParts-breakBurstParticlesPerBurst+1)*avatarInstanceBytes)
	if got := encoder.AppendBreakBurstInstances(roomForPartial, 12, drops); len(got) != len(roomForPartial) {
		t.Fatalf("留 7 空位并入后=%d 字节，想要不变 %d", len(got), len(roomForPartial))
	}
}

// TestEncodeBreakBurstInstancesStaysWithinInstanceCap 钉住编码上限:17 个全活
// burst 只输出 64 实例,不做无界工作。
func TestEncodeBreakBurstInstancesStaysWithinInstanceCap(t *testing.T) {
	encoder := &InstanceEncoder{}
	var drops []ItemDrop
	for tick := uint64(0); tick < 17; tick++ {
		drops = append(drops, breakTestDrop(uint8(tick),
			core.BlockPos{X: int32(tick), Y: 3, Z: 0}, core.ItemDirt))
		encoder.EncodeBreakBurstInstances(nil, tick, drops)
	}
	stream := encoder.EncodeBreakBurstInstances(nil, 16, drops)
	if len(stream) != 64*avatarInstanceBytes {
		t.Fatalf("burst 流 = %d 字节，想要上限 %d", len(stream), 64*avatarInstanceBytes)
	}
}
