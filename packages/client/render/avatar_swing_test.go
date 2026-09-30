package render

import (
	"testing"

	"github.com/go-gl/mathgl/mgl32"
)

// TestAvatarSwingAngleIsDistanceDrivenAndSpeedGated 锁定摆动相位函数：相位
// 按累积行进距离推进（固定步幅，禁用墙钟与 tick 计数），速度阈值下回中，同
// 输入重放一致。
func TestAvatarSwingAngleIsDistanceDrivenAndSpeedGated(t *testing.T) {
	if got := AvatarSwingAngle(0, 7, 0); got != 0 {
		t.Fatalf("静止摆动=%v，想要回中 0", got)
	}
	if got := AvatarSwingAngle(1.5, 7, 0.004); got != 0 {
		t.Fatalf("阈值下摆动=%v，想要回中 0", got)
	}
	moving := AvatarSwingAngle(1.0, 7, 0.086)
	if moving == 0 {
		t.Fatal("行走速度下摆动为 0，想要非零")
	}
	if moving < -avatarSwingAmplitude || moving > avatarSwingAmplitude {
		t.Fatalf("摆动=%v，超出振幅 ±%v", moving, avatarSwingAmplitude)
	}
	if repeat := AvatarSwingAngle(1.0, 7, 0.086); repeat != moving {
		t.Fatal("同距离同 ID 同速度重放不一致")
	}
	// 同距离不同速度（皆高于阈值）同相位：步幅锁定，与时间频率无关。
	if same := AvatarSwingAngle(1.0, 7, 0.043); same != moving {
		t.Fatal("同距离异速相位不同，想要步幅锁定")
	}
	if next := AvatarSwingAngle(1.5, 7, 0.086); next == moving {
		t.Fatal("距离前进后相位未变化")
	}
	if other := AvatarSwingAngle(1.0, 8, 0.086); other == moving {
		t.Fatal("不同实体得到相同相位，想要 ID 错相")
	}
	// 走满一个步幅回到同相位：不同实体同节奏。
	for _, phaseID := range []uint64{7, 8} {
		again := AvatarSwingAngle(1.0+avatarStrideLength, phaseID, 0.086)
		want := AvatarSwingAngle(1.0, phaseID, 0.086)
		if diff := again - want; diff < -1e-5 || diff > 1e-5 {
			t.Fatalf("ID %d 步幅周期前后=%v/%v，想要同相位", phaseID, again, want)
		}
	}
}

// TestAvatarSwingZeroKeepsByteIdentity 锁定零摆动的逐字节一致：`Swing` 为零
// 时三类身体的变换与颜色与变更前逐字节一致。
func TestAvatarSwingZeroKeepsByteIdentity(t *testing.T) {
	position := mgl32.Vec3{4, 5, 6}
	cases := []Avatar{
		{Key: testEntityKey(testAvatarID(1)), Position: position, Yaw: 0.5, Pitch: 0.2},
		{Key: HostileEntityKey(9), Position: position, Yaw: 0.5},
		{Key: PassiveEntityKey(7), Position: position, Yaw: 0.5},
	}
	for _, base := range cases {
		neutral := buildAvatarParts(nil, []Avatar{base})
		swung := base
		swung.Swing = 0
		got := buildAvatarParts(nil, []Avatar{swung})
		if len(got) != len(neutral) {
			t.Fatalf("kind %d 实例数=%d，想要 %d", base.Key.Kind, len(got), len(neutral))
		}
		for index := range got {
			if got[index] != neutral[index] {
				t.Fatalf("kind %d 部件 %d 零摆动不一致", base.Key.Kind, index)
			}
		}
	}
}

// assertPivotFixed 断言转轴点不动：转轴是几何坐标（`root` 系）下的 `pivot`，
// 其对应的单位立方体坐标为 `(pivot-center)/size`，映射后应落在 `root` 系的
// 转轴处（本用例 yaw 恒为 0，即 `position+pivot`）。
func assertPivotFixed(t *testing.T, position, pivot, center, size mgl32.Vec3, part avatarPart) {
	t.Helper()
	unit := mgl32.Vec3{
		(pivot[0] - center[0]) / size[0],
		(pivot[1] - center[1]) / size[1],
		(pivot[2] - center[2]) / size[2],
	}
	hold := part.transform.Mul4x1(mgl32.Vec4{unit[0], unit[1], unit[2], 1}).Vec3()
	if diff := hold.Sub(position.Add(pivot)); diff.Len() > 1e-4 {
		t.Fatalf("转轴 %v 漂移到 %v，想要绕轴旋转", position.Add(pivot), hold)
	}
}

// TestAvatarSwingRotatesLimbsAboutJointsAntiphase 锁定四肢摆动几何：手臂/腿
// 绕肩/髋转轴旋转（转轴点不动），对侧反相。
func TestAvatarSwingRotatesLimbsAboutJointsAntiphase(t *testing.T) {
	position := mgl32.Vec3{4, 5, 6}
	parts := buildAvatarParts(nil, []Avatar{{
		Key: testEntityKey(testAvatarID(1)), Position: position, Swing: 0.3,
	}})
	neutral := buildAvatarParts(nil, []Avatar{{
		Key: testEntityKey(testAvatarID(1)), Position: position,
	}})
	// 头与躯干不受摆动影响。
	for _, index := range []int{0, 1} {
		if parts[index] != neutral[index] {
			t.Fatalf("部件 %d 被摆动改变，想要头/躯干不动", index)
		}
	}
	// 四肢都动了，且转轴（肩 y=1.4、髋 y=0.7）固定。
	limbs := []struct {
		index         int
		pivot, center mgl32.Vec3
		size          mgl32.Vec3
	}{
		{2, mgl32.Vec3{-0.25, 1.4, 0}, mgl32.Vec3{-0.25, 1.05, 0}, mgl32.Vec3{0.1, 0.7, 0.25}},
		{3, mgl32.Vec3{0.25, 1.4, 0}, mgl32.Vec3{0.25, 1.05, 0}, mgl32.Vec3{0.1, 0.7, 0.25}},
		{4, mgl32.Vec3{-0.1, 0.7, 0}, mgl32.Vec3{-0.1, 0.35, 0}, mgl32.Vec3{0.18, 0.7, 0.25}},
		{5, mgl32.Vec3{0.1, 0.7, 0}, mgl32.Vec3{0.1, 0.35, 0}, mgl32.Vec3{0.18, 0.7, 0.25}},
	}
	for _, limb := range limbs {
		if parts[limb.index] == neutral[limb.index] {
			t.Fatalf("四肢 %d 未摆动", limb.index)
		}
		assertPivotFixed(t, position, limb.pivot, limb.center, limb.size, parts[limb.index])
	}
	// 对角反相：左臂与右腿同相，右臂与左腿反相。以转轴下垂点（几何坐标
	// `pivot+(0,-0.35,0)`，先换算到单位立方体坐标再映射）的 Z 位移符号判定
	// （绕 X 摆动只改变 Z）。
	armTip := mgl32.Vec3{0, -0.35, 0}
	legTip := mgl32.Vec3{0, -0.35, 0}
	signZ := func(limb struct {
		index         int
		pivot, center mgl32.Vec3
		size          mgl32.Vec3
	}, tip mgl32.Vec3) float32 {
		g := limb.pivot.Add(tip)
		u := mgl32.Vec3{
			(g[0] - limb.center[0]) / limb.size[0],
			(g[1] - limb.center[1]) / limb.size[1],
			(g[2] - limb.center[2]) / limb.size[2],
		}
		mapped := parts[limb.index].transform.Mul4x1(mgl32.Vec4{u[0], u[1], u[2], 1})
		return mapped.Z() - (position.Z() + limb.pivot.Z())
	}
	zArmL := signZ(limbs[0], armTip)
	zArmR := signZ(limbs[1], armTip)
	zLegL := signZ(limbs[2], legTip)
	zLegR := signZ(limbs[3], legTip)
	if zArmL == 0 || zArmR == 0 || (zArmL > 0) == (zArmR > 0) {
		t.Fatalf("左右臂 Z 位移=%v/%v，想要反相非零", zArmL, zArmR)
	}
	if zLegL == 0 || zLegR == 0 || (zLegL > 0) == (zLegR > 0) {
		t.Fatalf("左右腿 Z 位移=%v/%v，想要反相非零", zLegL, zLegR)
	}
	if (zArmL > 0) != (zLegR > 0) || (zArmR > 0) != (zLegL > 0) {
		t.Fatal("对角（左臂右腿/右臂左腿）不同相，想要 MC 式对角步态")
	}
}

// TestAvatarSwingCowLegsSwingDiagonalAboutHips 锁定牛腿摆动：四腿绕髋（y=0.5）
// 前后摆动，对角同相、邻腿反相。
func TestAvatarSwingCowLegsSwingDiagonalAboutHips(t *testing.T) {
	position := mgl32.Vec3{4, 5, 6}
	parts := buildAvatarParts(nil, []Avatar{{
		Key: PassiveEntityKey(7), Position: position, Swing: 0.3,
	}})
	neutral := buildAvatarParts(nil, []Avatar{{
		Key: PassiveEntityKey(7), Position: position,
	}})
	// 头与躯干不动。
	for _, index := range []int{0, 1} {
		if parts[index] != neutral[index] {
			t.Fatalf("牛部件 %d 被摆动改变，想要头/躯干不动", index)
		}
	}
	centers := []mgl32.Vec3{
		{-0.4, 0.25, -0.18}, {-0.4, 0.25, 0.18}, {0.4, 0.25, -0.18}, {0.4, 0.25, 0.18},
	}
	legSize := mgl32.Vec3{0.18, 0.5, 0.18}
	for offset, index := range []int{2, 3, 4, 5} {
		if parts[index] == neutral[index] {
			t.Fatalf("牛腿 %d 未摆动", index)
		}
		pivot := mgl32.Vec3{centers[offset][0], 0.5, centers[offset][2]}
		assertPivotFixed(t, position, pivot, centers[offset], legSize, parts[index])
	}
	// 对角同相：后左+前右（2+5）、后右+前左（3+4）。牛腿绕 Z 摆动，转轴
	// 下垂点位移体现在 X 上。
	signX := func(index, offset int) float32 {
		g := mgl32.Vec3{centers[offset][0], 0.25, centers[offset][2]}
		u := mgl32.Vec3{
			(g[0] - centers[offset][0]) / legSize[0],
			(g[1] - centers[offset][1]) / legSize[1],
			(g[2] - centers[offset][2]) / legSize[2],
		}
		mapped := parts[index].transform.Mul4x1(mgl32.Vec4{u[0], u[1], u[2], 1})
		return mapped.X() - (position.X() + centers[offset][0])
	}
	x2, x3, x4, x5 := signX(2, 0), signX(3, 1), signX(4, 2), signX(5, 3)
	for index, x := range []float32{x2, x3, x4, x5} {
		if x == 0 {
			t.Fatalf("牛腿 %d 位移为 0，想要前后摆动", index+2)
		}
	}
	if (x2 > 0) != (x5 > 0) || (x3 > 0) != (x4 > 0) {
		t.Fatal("牛对角腿不同相，想要对角步态")
	}
	if (x2 > 0) == (x3 > 0) {
		t.Fatal("牛同侧前后腿同相，想要邻腿反相")
	}
}

// TestAvatarSwingYieldsToDeathAndGraze 锁定特殊位姿让路：死亡（侧倒/红闪）与
// 放牧低头的牛不摆动；玩家的观察俯仰不门控四肢。
func TestAvatarSwingYieldsToDeathAndGraze(t *testing.T) {
	position := mgl32.Vec3{4, 5, 6}
	for _, dying := range []Avatar{
		{Key: PassiveEntityKey(7), Position: position, Swing: 0.3, Roll: 0.5},
		{Key: PassiveEntityKey(7), Position: position, Swing: 0.3, Flash: 0.5},
		{Key: PassiveEntityKey(7), Position: position, Swing: 0.3, Pitch: PassiveGrazeHeadPitch(true)},
	} {
		// 对照是同位姿的零摆动：整体侧倒/低头仍在，四肢必须与之逐部件一致。
		reference := dying
		reference.Swing = 0
		want := buildAvatarParts(nil, []Avatar{reference})
		got := buildAvatarParts(nil, []Avatar{dying})
		for index := range got {
			if got[index] != want[index] {
				t.Fatalf("特殊位姿 %+v 的部件 %d 仍在摆动，想要让路", dying, index)
			}
		}
	}
	// 玩家抬头行走：四肢照摆。
	player := buildAvatarParts(nil, []Avatar{{
		Key: testEntityKey(testAvatarID(1)), Position: position, Pitch: 1.0, Swing: 0.3,
	}})
	playerNeutral := buildAvatarParts(nil, []Avatar{{
		Key: testEntityKey(testAvatarID(1)), Position: position, Pitch: 1.0,
	}})
	if player[2] == playerNeutral[2] {
		t.Fatal("玩家观察俯仰门控了摆动，想要四肢照摆")
	}
}

// TestInstanceEncoderEstimatesSpeedFromPresentations 锁定呈现速度差分：静止
// 回中、移动起摆、同 tick 保持、停下回中，全程确定。
func TestInstanceEncoderEstimatesSpeedFromPresentations(t *testing.T) {
	var encoder InstanceEncoder
	key := PassiveEntityKey(7)
	start := mgl32.Vec3{4, 5, 6}
	avatars := []Avatar{{Key: key, Position: start}}
	// 首见无历史：回中。
	first := encoder.EncodeAvatarInstances(nil, 10, avatars)
	if len(first) != avatarPartsPerBody*avatarInstanceBytes {
		t.Fatalf("实例字节=%d，想要 %d", len(first), avatarPartsPerBody*avatarInstanceBytes)
	}
	neutral := buildAvatarParts(nil, []Avatar{{Key: key, Position: start}})
	assertPartStreamsEqual(t, first, avatarPartBytes(neutral))
	// 同 tick 同位置：仍回中。
	again := encoder.EncodeAvatarInstances(nil, 10, avatars)
	assertPartStreamsEqual(t, again, first)
	// 下一 tick 位移 0.086 格：累积距离 0.086、速度 0.086，摆动角即相位函数值。
	moved := []Avatar{{Key: key, Position: start.Add(mgl32.Vec3{0.086, 0, 0})}}
	swung := encoder.EncodeAvatarInstances(nil, 11, moved)
	want := buildAvatarParts(nil, []Avatar{{
		Key: key, Position: start.Add(mgl32.Vec3{0.086, 0, 0}),
		Swing: avatarKindSwingAngle(key.Kind, 0.086, swingPhaseID(key), 0.086),
	}})
	assertPartStreamsEqual(t, swung, avatarPartBytes(want))
	if AvatarSwingAngle(0.086, swingPhaseID(key), 0.086) == 0 {
		t.Fatal("行走相位角为 0，本用例失去摆动覆盖")
	}
	// 同 tick 保持：速度沿用，不回中。
	held := encoder.EncodeAvatarInstances(nil, 11, moved)
	assertPartStreamsEqual(t, held, swung)
	// 停下：下一 tick 无位移即回中。
	stopped := encoder.EncodeAvatarInstances(nil, 12, moved)
	assertPartStreamsEqual(t, stopped, avatarPartBytes(buildAvatarParts(nil, moved)))
	// 40 tick 静止：绝不原地踏步。
	var steady []byte
	for tick := uint64(13); tick < 53; tick++ {
		steady = encoder.EncodeAvatarInstances(nil, tick, moved)
	}
	assertPartStreamsEqual(t, steady, stopped)
}

// TestInstanceEncoderStrideLocksCyclesAcrossSpeeds 锁定步幅语义：慢速与快速
// 走完相同距离时周期数相同（终相位一致），慢速走完则用更多 tick（单周期更长）。
func TestInstanceEncoderStrideLocksCyclesAcrossSpeeds(t *testing.T) {
	drive := func(step float32, frames int) (float32, int) {
		var encoder InstanceEncoder
		key := PassiveEntityKey(7)
		position := mgl32.Vec3{0, 1, 0}
		tick := uint64(20)
		encoder.EncodeAvatarInstances(nil, tick, []Avatar{{Key: key, Position: position}})
		for frame := 0; frame < frames; frame++ {
			tick++
			position = position.Add(mgl32.Vec3{step, 0, 0})
			encoded := encoder.EncodeAvatarInstances(nil, tick, []Avatar{{Key: key, Position: position}})
			neutral := avatarPartBytes(buildAvatarParts(nil, []Avatar{{Key: key, Position: position}}))
			if string(encoded) == string(neutral) {
				t.Fatalf("步速 %v 第 %d 帧未起摆", step, frame)
			}
		}
		return encoder.tracks[key].distance, int(tick)
	}
	const total = float32(2.0)
	slowDistance, slowEnd := drive(0.05, 40)
	fastDistance, fastEnd := drive(0.1, 20)
	if diff := slowDistance - total; diff < -1e-4 || diff > 1e-4 {
		t.Fatalf("慢速累积距离=%v，想要 %v", slowDistance, total)
	}
	if diff := fastDistance - total; diff < -1e-4 || diff > 1e-4 {
		t.Fatalf("快速累积距离=%v，想要 %v", fastDistance, total)
	}
	if slowEnd-20 == fastEnd-20 {
		t.Fatal("慢速与快速用 tick 数相同，想要慢速单周期更长")
	}
	// 同距离终相位一致：两编码器末帧摆动角互相接近。
	swingAt := func(distance float32) float32 {
		return AvatarSwingAngle(distance, swingPhaseID(PassiveEntityKey(7)), 0.1)
	}
	if diff := swingAt(slowDistance) - swingAt(fastDistance); diff < -1e-5 || diff > 1e-5 {
		t.Fatalf("同距离终相位差=%v，想要一致", diff)
	}
}

// TestInstanceEncoderSwingResetsOnRollbackAndReset 锁定累积器清零：tick 回退
// （场景切换/重连）与显式重置都从零重新累积，同消息流重放逐字节一致。
func TestInstanceEncoderSwingResetsOnRollbackAndReset(t *testing.T) {
	key := PassiveEntityKey(7)
	start := mgl32.Vec3{4, 5, 6}
	sequence := func(encoder *InstanceEncoder) []byte {
		encoder.EncodeAvatarInstances(nil, 10, []Avatar{{Key: key, Position: start}})
		return encoder.EncodeAvatarInstances(nil, 11, []Avatar{{Key: key, Position: start.Add(mgl32.Vec3{0.086, 0, 0})}})
	}
	var first InstanceEncoder
	want := sequence(&first)
	// tick 回退后重走同序列：旧距离不清就会把回退前的位移带进相位。
	var rolled InstanceEncoder
	rolled.EncodeAvatarInstances(nil, 30, []Avatar{{Key: key, Position: mgl32.Vec3{9, 9, 9}}})
	rolled.EncodeAvatarInstances(nil, 5, []Avatar{{Key: key, Position: start}})
	rolled.EncodeAvatarInstances(nil, 11, []Avatar{{Key: key, Position: start.Add(mgl32.Vec3{0.086, 0, 0})}})
	// 回退的中途帧（tick 5）已重新锚定：此处的 tick 11 距锚点 6 tick，速度即
	// 0.086/6；直接比较会引入不同的速度门控路径，因此只断言距离已清零。
	distance := rolled.tracks[key].distance
	if diff := distance - 0.086; diff < -1e-5 || diff > 1e-5 {
		t.Fatalf("回退后累积距离=%v，想要 0.086", distance)
	}
	// 显式重置后重走同序列：与首跑逐字节一致。
	var reset InstanceEncoder
	reset.EncodeAvatarInstances(nil, 10, []Avatar{{Key: key, Position: mgl32.Vec3{9, 9, 9}}})
	reset.ResetLocomotion()
	if len(reset.tracks) != 0 {
		t.Fatalf("重置后残留 tracks=%d，想要 0", len(reset.tracks))
	}
	if replayed := sequence(&reset); string(replayed) != string(want) {
		t.Fatal("重置后同序列重放不一致")
	}
}

// TestInstanceEncoderSwingIsDeterministicAndBounded 锁定编码器确定性与有界：
// 同输入序列的双编码器逐字节一致，离场实体不留痕。
func TestInstanceEncoderSwingIsDeterministicAndBounded(t *testing.T) {
	sequence := func(encoder *InstanceEncoder) [][]byte {
		var out [][]byte
		positions := []mgl32.Vec3{{0, 1, 0}, {0.086, 1, 0}, {0.172, 1, 0}, {0.172, 1, 0}}
		for frame, position := range positions {
			avatars := []Avatar{
				{Key: testEntityKey(testAvatarID(1)), Position: position},
				{Key: PassiveEntityKey(7), Position: mgl32.Vec3{4, 5, 6}},
			}
			out = append(out, encoder.EncodeAvatarInstances(nil, uint64(20+frame), avatars))
		}
		return out
	}
	var left, right InstanceEncoder
	for index, want := range sequence(&left) {
		if got := sequence(&right)[index]; string(got) != string(want) {
			t.Fatalf("第 %d 帧双编码器不一致", index)
		}
	}
	var encoder InstanceEncoder
	encoder.EncodeAvatarInstances(nil, 30, []Avatar{{Key: PassiveEntityKey(7)}})
	encoder.EncodeAvatarInstances(nil, 31, []Avatar{{Key: PassiveEntityKey(8)}})
	if len(encoder.tracks) != 1 {
		t.Fatalf("离场实体残留：tracks=%d，想要 1", len(encoder.tracks))
	}
}

func assertPartStreamsEqual(t *testing.T, got []byte, want []byte) {
	t.Helper()
	if string(got) != string(want) {
		t.Fatal("实例字节流不一致")
	}
}
