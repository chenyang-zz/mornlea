package server

import (
	"context"
	"fmt"
	"math"
	"reflect"
	"runtime"
	"strings"
	"sync/atomic"
	"testing"
	"time"

	"github.com/channing771/mornlea/packages/client/client"
	"github.com/channing771/mornlea/packages/server/sim/contract"
	"github.com/channing771/mornlea/packages/shared/core"
	"github.com/channing771/mornlea/packages/shared/network"
	"github.com/channing771/mornlea/packages/shared/physics"
	"github.com/channing771/mornlea/packages/shared/world"
)

func TestAuthoritativePlayerConvergesAfterThreeTickStateDelay(t *testing.T) {
	h := newDelayedPlayerHarness(t, 3)
	h.waitReady()
	h.hold(client.Movement{MoveZ: 1}, 20) // accelerate and walk one second
	h.hold(client.Movement{}, 10)         // friction stop
	h.hold(client.Movement{MoveZ: 1, Jump: true}, 1)
	h.hold(client.Movement{MoveZ: 1}, 20) // clear the one-block test obstacle without tunneling
	h.clickPlaceDown(0)                   // 空快捷栏栏位无法放置: invalid_block
	h.holdMiningUntilComplete()           // 然后通过权威眼睛射线完成采掘
	h.hold(client.Movement{}, 10)
	h.flushAllStates()
	h.assertConverged(1e-5)
	h.assertWorldHashesEqual()
	h.closeAndAssertNoGoroutineLeak(waitDeadline)
}

func TestAuthoritativePlayerReplayIsDeterministic(t *testing.T) {
	first := runDelayedPlayerScript(t)
	second := runDelayedPlayerScript(t)
	if !reflect.DeepEqual(first, second) {
		t.Fatalf("两次权威玩家 replay 不同:\nfirst=%+v\nsecond=%+v", first, second)
	}
}

func TestDelayedCloseCleanupDoesNotReenterStartedClose(t *testing.T) {
	var gate delayedCloseGate
	var calls atomic.Int32
	entered := make(chan struct{})
	release := make(chan struct{})
	defer func() {
		select {
		case <-release:
		default:
			close(release)
		}
	}()
	done, started := gate.start(func() {
		calls.Add(1)
		close(entered)
		<-release
	})
	if !started {
		t.Fatal("首次异步 close 未启动")
	}
	<-entered

	gate.cleanup(func() {
		calls.Add(1)
	})
	if got := calls.Load(); got != 1 {
		t.Fatalf("close 已启动后 cleanup 调用 closer %d 次，想要 1", got)
	}

	close(release)
	select {
	case <-done:
	case <-time.After(waitDeadline):
		t.Fatal("测试 closer 未退出")
	}
}

func TestDelayedCloseLeakWaitUsesOneDeadline(t *testing.T) {
	started := time.Unix(100, 0)
	deadline := started.Add(time.Second)
	closeDone := make(chan struct{})
	close(closeDone)
	nowCalls := 0
	leakChecks := 0
	result := waitForCloseAndLeakDeadline(
		deadline,
		closeDone,
		func() bool {
			leakChecks++
			return false
		},
		func() time.Time {
			nowCalls++
			if nowCalls == 1 {
				return started.Add(750 * time.Millisecond)
			}
			return deadline
		},
		func() {},
	)
	if result != delayedCloseLeakTimeout {
		t.Fatalf("close 消耗 750ms 后结果=%v，想要原 deadline 的 leak timeout", result)
	}
	if leakChecks != 0 {
		t.Fatalf("原 deadline 已到后仍执行 %d 次 leak probe，想要 0", leakChecks)
	}
}

type delayedCloseWaitResult uint8

const (
	delayedCloseWaitOK delayedCloseWaitResult = iota
	delayedCloseTimeout
	delayedCloseLeakTimeout
)

type delayedCloseGate struct {
	closeStarted bool
}

func (gate *delayedCloseGate) start(closeFunc func()) (<-chan struct{}, bool) {
	if gate.closeStarted {
		return nil, false
	}
	gate.closeStarted = true
	done := make(chan struct{})
	go func() {
		closeFunc()
		close(done)
	}()
	return done, true
}

func (gate *delayedCloseGate) cleanup(closeFunc func()) {
	if gate.closeStarted {
		return
	}
	gate.closeStarted = true
	closeFunc()
}

func waitForCloseAndLeakDeadline(
	deadline time.Time,
	closeDone <-chan struct{},
	leakFree func() bool,
	now func() time.Time,
	yield func(),
) delayedCloseWaitResult {
	remaining := deadline.Sub(now())
	if remaining <= 0 {
		select {
		case <-closeDone:
		default:
			return delayedCloseTimeout
		}
	} else {
		timer := time.NewTimer(remaining)
		select {
		case <-closeDone:
			if !timer.Stop() {
				select {
				case <-timer.C:
				default:
				}
			}
		case <-timer.C:
			return delayedCloseTimeout
		}
	}

	for {
		if !now().Before(deadline) {
			return delayedCloseLeakTimeout
		}
		if leakFree() {
			return delayedCloseWaitOK
		}
		yield()
	}
}

type delayedPlayerState struct {
	deliverAtTick uint64
	state         network.PlayerState
}

type delayedPlayerHarness struct {
	t              *testing.T
	clientEndpoint network.ClientEndpoint
	running        *Server
	mirror         *client.Mirror
	inventory      client.InventoryMirror
	crafting       client.CraftingMirror
	predictor      *client.Predictor
	delayTicks     uint64
	serverTick     uint64
	sequence       uint64
	yaw            float32
	pitch          float32
	movement       client.Movement
	mining         bool
	pendingStates  []delayedPlayerState
	lastApplied    network.PlayerState
	hasApplied     bool
	dimension      core.DimensionID
	rejections     []network.CommandRejected
	deltas         []network.BlockChanges
	placeSequence  uint64
	breakSequence  uint64
	goroutines     int
	closeGate      delayedCloseGate
}

type delayedReplayResult struct {
	Player     contract.PlayerUpdate
	PlayerHash [32]byte
	Chunk      core.ChunkPos
	ChunkHash  [32]byte
	Revision   uint64
	Rejected   []network.CommandRejected
}

// `delayedPlayerGenerator` 保留平坦夹具的碰撞与障碍，只将草地替换成泥土，
// 避免自然刷牛和吃草向玩家回放注入无关方块变化。共享夹具仍保留真实草地。
type delayedPlayerGenerator struct{}

func (delayedPlayerGenerator) GenerateChunk(_ core.DimensionID, position core.ChunkPos) *world.Chunk {
	chunk := flatTestGenerator{}.GenerateChunk(core.Overworld, position)
	for z := 0; z < core.SectionSize; z++ {
		for x := 0; x < core.SectionSize; x++ {
			chunk.SetBlock(x, 0, z, core.DirtID)
		}
	}
	chunk.Compact()
	return chunk
}

func (delayedPlayerGenerator) BaseBlockAt(position core.BlockPos) core.BlockID {
	if position.Y == 0 {
		return core.DirtID
	}
	return flatTestGenerator{}.BaseBlockAt(position)
}

func newDelayedPlayerHarness(t *testing.T, delayTicks uint64) *delayedPlayerHarness {
	t.Helper()
	clientEndpoint, serverEndpoint := network.NewMemoryPair(4096)
	config := DefaultConfig(42)
	config.ViewRadius = 1
	config.Workers = 1
	config.SnapshotChunks = 64
	config.SnapshotBytes = 4 << 20
	config.OutboxCapacity = 4096
	h := &delayedPlayerHarness{
		t:              t,
		clientEndpoint: clientEndpoint,
		mirror:         client.NewMirror(),
		predictor:      client.NewPredictor(),
		delayTicks:     delayTicks,
		goroutines:     runtime.NumGoroutine(),
	}
	h.running = newMemoryAttachedWorldForTest(config, serverEndpoint, delayedPlayerGenerator{})
	t.Cleanup(func() {
		h.closeGate.cleanup(func() {
			shutdownServerForTest(t, h.running)
			_ = h.clientEndpoint.Close()
		})
	})
	return h
}

func (h *delayedPlayerHarness) waitReady() {
	h.t.Helper()
	deadline := time.Now().Add(waitDeadline)
	for {
		nextTick := h.serverTick + 1
		h.applyDueStates(nextTick)
		_, ready := h.predictor.State()
		if ready && h.initialWorldReady() {
			return
		}
		if time.Now().After(deadline) {
			h.t.Fatalf(
				"等待玩家与初始世界 Ready 超过 3 秒: serverTick=%d pendingStates=%d %s",
				h.serverTick,
				len(h.pendingStates),
				h.initialWorldSummary(),
			)
		}
		h.finishTick(0, 0)
		time.Sleep(integrationPollInterval)
	}
}

func (h *delayedPlayerHarness) initialWorldReady() bool {
	for z := int32(-1); z <= 1; z++ {
		for x := int32(-1); x <= 1; x++ {
			if _, ok := h.mirror.Chunk(core.Overworld, core.ChunkPos{X: x, Z: z}); !ok {
				return false
			}
		}
	}
	return true
}

func (h *delayedPlayerHarness) initialWorldSummary() string {
	missing := make([]core.ChunkPos, 0, 9)
	states := make([]string, 0, 9)
	for z := int32(-1); z <= 1; z++ {
		for x := int32(-1); x <= 1; x++ {
			position := core.ChunkPos{X: x, Z: z}
			if _, ok := h.mirror.Chunk(core.Overworld, position); !ok {
				missing = append(missing, position)
			}
			info, ok := h.running.ChunkInfo(core.Overworld, position)
			states = append(states, fmt.Sprintf("%+v=%+v,%v", position, info, ok))
		}
	}
	return fmt.Sprintf(
		"mirrorMissing=%+v server=[%s] pending=%d jobs=%d generated=%d queued=%d",
		missing,
		strings.Join(states, " "),
		len(h.running.pending),
		len(h.running.jobs),
		len(h.running.generated),
		len(h.running.queued),
	)
}

func (h *delayedPlayerHarness) hold(movement client.Movement, ticks int) {
	h.t.Helper()
	h.movement = movement
	for range ticks {
		h.advanceInputTick(nil)
	}
}

func (h *delayedPlayerHarness) clickPlaceDown(slot uint8) {
	h.t.Helper()
	h.pitch = -float32(math.Pi)/2 + 0.01
	h.advanceInputTick(func() network.ClientMessage {
		h.placeSequence = h.nextSequence()
		return network.PlaceBlock{
			Sequence: h.placeSequence,
			Yaw:      h.yaw,
			Pitch:    h.pitch,
			Slot:     slot,
		}
	})
}

func (h *delayedPlayerHarness) holdMiningUntilComplete() {
	h.t.Helper()
	h.pitch = -float32(math.Pi)/2 + 0.01
	h.movement = client.Movement{}
	h.mining = true
	h.breakSequence = h.advanceInputTick(nil)
	for range 4 {
		h.breakSequence = h.advanceInputTick(nil)
	}
	h.mining = false
}

func (h *delayedPlayerHarness) advanceInputTick(
	action func() network.ClientMessage,
) uint64 {
	h.t.Helper()
	h.applyDueStates(h.serverTick + 1)
	sent := 0
	if action != nil {
		h.send(action())
		sent++
	}
	control := client.Control{
		MoveX:  h.movement.MoveX,
		MoveZ:  h.movement.MoveZ,
		Jump:   h.movement.Jump,
		Yaw:    h.yaw,
		Pitch:  h.pitch,
		Mining: h.mining,
	}
	if err := h.predictor.Advance(
		physics.FixedDelta,
		control,
		client.MirrorCollisionSource{Mirror: h.mirror, Dimension: h.dimension},
		h.nextSequence,
		func(input network.PlayerInput) error {
			sent++
			return h.sendError(input)
		},
	); err != nil {
		h.t.Fatalf("Predictor.Advance tick=%d: %v", h.serverTick+1, err)
	}
	lastInputSequence := h.sequence
	h.finishTick(sent, lastInputSequence)
	return lastInputSequence
}

func (h *delayedPlayerHarness) finishTick(sent int, wantInputAck uint64) {
	h.t.Helper()
	if sent != 0 {
		h.waitForIncoming(sent)
	}
	result := h.running.StepForTest()
	wantTick := h.serverTick + 1
	if result.Tick != wantTick {
		h.t.Fatalf("StepForTest tick=%d，想要 %d", result.Tick, wantTick)
	}
	if len(result.Players) != 1 {
		h.t.Fatalf("tick %d Players=%+v，想要恰好一个", result.Tick, result.Players)
	}
	if wantInputAck != 0 && result.Players[0].LastInputSequence != wantInputAck {
		h.t.Fatalf(
			"tick %d 输入未在同 tick 到达: ack=%d want=%d",
			result.Tick,
			result.Players[0].LastInputSequence,
			wantInputAck,
		)
	}
	h.serverTick = result.Tick
	h.drainServerTick(result.Tick)
}

func (h *delayedPlayerHarness) waitForIncoming(want int) {
	h.t.Helper()
	deadline := time.Now().Add(waitDeadline)
	for len(h.running.incoming) < want {
		if time.Now().After(deadline) {
			h.t.Fatalf(
				"等待 MemoryTransport 命令进入 server 超时: got=%d want=%d",
				len(h.running.incoming),
				want,
			)
		}
		time.Sleep(integrationPollInterval)
	}
}

func (h *delayedPlayerHarness) drainServerTick(throughTick uint64) {
	h.t.Helper()
	ctx, cancel := context.WithTimeout(context.Background(), waitDeadline)
	defer cancel()
	for {
		message, err := h.clientEndpoint.Recv(ctx)
		if err != nil {
			h.t.Fatalf("接收 tick %d 服务端消息: %v", throughTick, err)
		}
		switch message := message.(type) {
		case network.PlayerState:
			if message.ServerTick != throughTick {
				h.t.Fatalf(
					"PlayerState tick=%d，想要当前尾消息 tick=%d",
					message.ServerTick,
					throughTick,
				)
			}
			h.pendingStates = append(h.pendingStates, delayedPlayerState{
				deliverAtTick: message.ServerTick + h.delayTicks,
				state:         message,
			})
			return
		case network.InventoryState:
			if err := h.inventory.Apply(message); err != nil {
				h.t.Fatalf("InventoryMirror.Apply: %v", err)
			}
		case network.CraftingState:
			if err := h.crafting.Apply(message); err != nil {
				h.t.Fatalf("CraftingMirror.Apply: %v", err)
			}
		case network.ItemDropUpserts, network.ItemDropRemoves:
			// 掉落物由独立镜像消费，不进入世界镜像。
		case network.PassiveSpawn, network.PassiveState, network.PassiveDespawn:
			// 被动牛由实体镜像消费（随被动牛同步任务装配），不进入世界区块镜像。
		default:
			if delta, ok := message.(network.BlockChanges); ok {
				copied := delta
				copied.Changes = append([]network.BlockChange(nil), delta.Changes...)
				h.deltas = append(h.deltas, copied)
			}
			update, applyErr := h.mirror.Apply(message)
			if applyErr != nil {
				h.t.Fatalf("Mirror.Apply(%T): %v", message, applyErr)
			}
			if update.Resync != nil {
				h.t.Fatalf("延迟收敛场景意外需要 resync: %+v", update.Resync)
			}
			if update.Rejected != nil {
				h.rejections = append(h.rejections, *update.Rejected)
			}
		}
	}
}

func (h *delayedPlayerHarness) applyDueStates(clientTick uint64) {
	h.t.Helper()
	for len(h.pendingStates) != 0 && h.pendingStates[0].deliverAtTick <= clientTick {
		delayed := h.pendingStates[0]
		copy(h.pendingStates, h.pendingStates[1:])
		h.pendingStates = h.pendingStates[:len(h.pendingStates)-1]
		result, err := h.predictor.ApplyPlayerState(
			delayed.state,
			client.MirrorCollisionSource{
				Mirror:    h.mirror,
				Dimension: delayed.state.Dimension,
			},
		)
		if err != nil {
			h.t.Fatalf(
				"ApplyPlayerState(serverTick=%d deliverAt=%d): %v",
				delayed.state.ServerTick,
				delayed.deliverAtTick,
				err,
			)
		}
		h.lastApplied = delayed.state
		h.hasApplied = true
		h.dimension = delayed.state.Dimension
		if result.ResetView {
			h.yaw = result.Yaw
			h.pitch = result.Pitch
		}
	}
}

func (h *delayedPlayerHarness) flushAllStates() {
	h.t.Helper()
	h.movement = client.Movement{}
	flushSequence := h.advanceInputTick(nil)
	for advances := uint64(0); !h.hasApplied || h.lastApplied.LastInputSequence < flushSequence; advances++ {
		if advances > h.delayTicks+1 {
			h.t.Fatalf(
				"neutral input sequence %d 未在 delay=%d 后确认: applied=%d",
				flushSequence,
				h.delayTicks,
				h.lastApplied.LastInputSequence,
			)
		}
		h.applyDueStates(h.serverTick + 1)
		if h.hasApplied && h.lastApplied.LastInputSequence >= flushSequence {
			break
		}
		h.advancePreparedInputTick()
	}
	for range h.delayTicks {
		h.applyDueStates(h.serverTick + 1)
		h.finishTick(0, 0)
	}
}

func (h *delayedPlayerHarness) advancePreparedInputTick() {
	h.t.Helper()
	sent := 0
	if err := h.predictor.Advance(
		physics.FixedDelta,
		client.Control{Yaw: h.yaw, Pitch: h.pitch},
		client.MirrorCollisionSource{Mirror: h.mirror, Dimension: h.dimension},
		h.nextSequence,
		func(input network.PlayerInput) error {
			sent++
			return h.sendError(input)
		},
	); err != nil {
		h.t.Fatalf("flush Predictor.Advance tick=%d: %v", h.serverTick+1, err)
	}
	h.finishTick(sent, h.sequence)
}

func (h *delayedPlayerHarness) assertConverged(tolerance float32) {
	h.t.Helper()
	want, ok := h.running.PlayerStateFor(testSessionID)
	if !ok || !want.Ready {
		h.t.Fatalf("权威玩家最终状态不可用: %+v,%v", want, ok)
	}
	got, ready := h.predictor.State()
	if !ready {
		h.t.Fatal("Predictor flush 后未 Ready")
	}
	if h.predictor.HistoryLen() != 0 {
		h.t.Fatalf("Predictor flush 后 history=%d，想要 0", h.predictor.HistoryLen())
	}
	assertVectorClose(h.t, "Position", got.Position, want.State.Position, tolerance)
	assertVectorClose(h.t, "Velocity", got.Velocity, want.State.Velocity, tolerance)
	if got.OnGround != want.State.OnGround {
		h.t.Fatalf("OnGround=%v，权威=%v", got.OnGround, want.State.OnGround)
	}
	if h.dimension != want.Dimension {
		h.t.Fatalf("dimension=%d，权威=%d", h.dimension, want.Dimension)
	}
	if !h.hasApplied || h.lastApplied.LastInputSequence != want.LastInputSequence {
		h.t.Fatalf(
			"最终输入 ack 未收敛: applied=%d authoritative=%d hasApplied=%v",
			h.lastApplied.LastInputSequence,
			want.LastInputSequence,
			h.hasApplied,
		)
	}
	if want.State.Position.Z()+physics.PlayerWidth/2 >= float32(playerIntegrationObstacle.Z) {
		h.t.Fatalf(
			"玩家未完整越过测试障碍: position=%+v obstacle=%+v",
			want.State.Position,
			playerIntegrationObstacle,
		)
	}
	block, loaded := h.mirror.BlockAt(core.Overworld, playerIntegrationObstacle)
	if !loaded || block != core.StoneID {
		h.t.Fatalf("跳跃后障碍=%d,%v，想要 Stone,true", block, loaded)
	}
}

func (h *delayedPlayerHarness) assertWorldHashesEqual() {
	h.t.Helper()
	wantRejections := []network.CommandRejected{{
		Sequence: h.placeSequence,
		Reason:   network.RejectInvalidBlock,
	}}
	if !reflect.DeepEqual(h.rejections, wantRejections) {
		h.t.Fatalf("rejections=%+v，想要 %+v", h.rejections, wantRejections)
	}
	// M4B：挖掘产生地面掉落物，随后的拾取会追加零方块 revision barrier。
	if len(h.deltas) == 0 {
		h.t.Fatal("没有收到挖掘 delta")
	}
	for index, extra := range h.deltas[1:] {
		if len(extra.Changes) != 0 {
			h.t.Fatalf("挖掘后第 %d 个 delta 含方块变化: %+v", index+1, extra)
		}
	}
	delta := h.deltas[0]
	if delta.BaseRevision != 1 || delta.NewRevision != 2 || len(delta.Changes) != 1 ||
		delta.Changes[0].Block != core.AirID {
		h.t.Fatalf("break delta=%+v，想要一个连续 1→2 Air 变化", delta)
	}
	if h.breakSequence <= h.placeSequence {
		h.t.Fatalf("动作 sequence 顺序错误: place=%d break=%d", h.placeSequence, h.breakSequence)
	}
	authoritativeHash, authoritativeRevision, authoritativeOK := h.running.ChunkHash(
		core.Overworld,
		delta.Chunk,
	)
	mirrorHash, mirrorRevision, mirrorOK := h.mirror.Hash(core.Overworld, delta.Chunk)
	// 权威 revision 取最后一份 delta：拾取掉落物会在挖掘之后追加 barrier。
	lastRevision := h.deltas[len(h.deltas)-1].NewRevision
	if !authoritativeOK || !mirrorOK || authoritativeRevision != mirrorRevision ||
		authoritativeRevision != lastRevision || authoritativeHash != mirrorHash {
		h.t.Fatalf(
			"交互区块不一致: authoritative=(%x,%d,%v) mirror=(%x,%d,%v) delta=%+v",
			authoritativeHash,
			authoritativeRevision,
			authoritativeOK,
			mirrorHash,
			mirrorRevision,
			mirrorOK,
			delta,
		)
	}
}

func (h *delayedPlayerHarness) replayResult() delayedReplayResult {
	h.t.Helper()
	player, ok := h.running.PlayerStateFor(testSessionID)
	if !ok {
		h.t.Fatal("replay 最终 Server.PlayerState 不可用")
	}
	playerHash, ok := h.running.engine.PlayerHash(testSessionID)
	if !ok {
		h.t.Fatal("replay 最终 PlayerHash 不可用")
	}
	// M4B：挖掘 delta 之后可能追加拾取掉落物产生的零方块 barrier。
	if len(h.deltas) == 0 {
		h.t.Fatalf("replay break deltas=%+v，想要至少一个", h.deltas)
	}
	chunk := h.deltas[0].Chunk
	chunkHash, revision, ok := h.running.ChunkHash(core.Overworld, chunk)
	if !ok {
		h.t.Fatalf("replay 最终区块 %+v hash 不可用", chunk)
	}
	// This harness is driven by the server's real-time ticker, so two runs of the
	// same script complete different tick counts. Absolute world time and the
	// values derived from it (season, in-season progress and temperature) are
	// therefore not part of the compared authoritative result. Each run first
	// proves those values match the production derivation for its own world
	// time and seed, then clears them so the replay comparison only covers
	// input-driven state. World time advancement itself is covered by the sim
	// world-time tests. Weather stays in the comparison: it is a seeded clock
	// whose first clear segment lasts far longer than this script.
	h.assertWorldTimeDerivedFields(player)
	player.WorldTimeTicks = 0
	player.Season = 0
	player.SeasonProgress = 0
	player.Temperature = 0
	return delayedReplayResult{
		Player:     player,
		PlayerHash: playerHash,
		Chunk:      chunk,
		ChunkHash:  chunkHash,
		Revision:   revision,
		Rejected:   append([]network.CommandRejected(nil), h.rejections...),
	}
}

// assertWorldTimeDerivedFields recomputes the season fields and temperature
// from the published world time, display offset, weather, height and world
// seed through the shared `core` derivations, so normalizing them for the
// replay comparison cannot hide a wrong or nondeterministic derivation.
func (h *delayedPlayerHarness) assertWorldTimeDerivedFields(player contract.PlayerUpdate) {
	h.t.Helper()
	seasonOffset := core.SeasonOffsetFromSeed(h.running.engine.SeedForTest())
	worldTime := player.WorldTimeTicks
	yearPhase := core.YearPhaseAt(worldTime, seasonOffset)
	wantSeason := core.SeasonAt(worldTime, seasonOffset)
	wantProgress := core.SeasonProgressAt(worldTime, seasonOffset)
	effectivePhase := core.EffectiveDayPhaseAt(worldTime, player.DayPhaseOffset, seasonOffset)
	// The wire temperature is rounded half away from zero and narrowed to int8;
	// `core.TemperatureAt` already clamps it into the int8 range.
	wantTemperature := int8(math.Round(float64(core.TemperatureAt(
		yearPhase, effectivePhase, player.WeatherKind, player.State.Position.Y(),
	))))
	if player.Season != wantSeason || player.SeasonProgress != wantProgress ||
		player.Temperature != wantTemperature {
		h.t.Fatalf(
			"world-time-derived fields at worldTime=%d: season=%v progress=%d temperature=%d, want season=%v progress=%d temperature=%d",
			worldTime,
			player.Season,
			player.SeasonProgress,
			player.Temperature,
			wantSeason,
			wantProgress,
			wantTemperature,
		)
	}
}

func (h *delayedPlayerHarness) closeAndAssertNoGoroutineLeak(timeout time.Duration) {
	h.t.Helper()
	deadline := time.Now().Add(timeout)
	var shutdownErr error
	done, started := h.closeGate.start(func() {
		ctx, cancel := context.WithDeadline(context.Background(), deadline)
		shutdownErr = h.running.Shutdown(ctx)
		cancel()
		_ = h.clientEndpoint.Close()
	})
	if !started {
		h.t.Fatal("delayed player harness 重复关闭")
	}

	var dump string
	result := waitForCloseAndLeakDeadline(
		deadline,
		done,
		func() bool {
			dump = goroutineDump()
			serverLeak := strings.Contains(dump, "(*Server).endpointReader") ||
				strings.Contains(dump, "(*Server).chunkWorker") ||
				strings.Contains(dump, "(*Server).saveWorker") ||
				strings.Contains(dump, "(*persistence.World).saveWorker") ||
				strings.Contains(dump, "(*session).writeLoop")
			return !serverLeak && runtime.NumGoroutine() <= h.goroutines
		},
		time.Now,
		func() {
			// 泄漏等待循环的 yield 步骤：固定 sleep 退避取代热轮询，理由见
			// integrationPollInterval 注释；sleep 让出核心反而能让被观测的
			// 泄漏目标 goroutine 更快退出。
			time.Sleep(integrationPollInterval)
		},
	)
	switch result {
	case delayedCloseWaitOK:
		if shutdownErr != nil {
			h.t.Fatalf("Server Shutdown error=%v", shutdownErr)
		}
		return
	case delayedCloseTimeout:
		h.t.Fatalf("Server/MemoryTransport 关闭超过 %v\n%s", timeout, goroutineDump())
	case delayedCloseLeakTimeout:
		h.t.Fatalf(
			"关闭后 goroutine 未在共享 %v deadline 内回到基线: before=%d after=%d\n%s",
			timeout,
			h.goroutines,
			runtime.NumGoroutine(),
			dump,
		)
	default:
		h.t.Fatalf("未知 close/leak wait result: %d", result)
	}
}

func runDelayedPlayerScript(t *testing.T) delayedReplayResult {
	t.Helper()
	h := newDelayedPlayerHarness(t, 3)
	h.waitReady()
	h.hold(client.Movement{MoveZ: 1}, 20)
	h.hold(client.Movement{}, 10)
	h.hold(client.Movement{MoveZ: 1, Jump: true}, 1)
	h.hold(client.Movement{MoveZ: 1}, 20)
	h.clickPlaceDown(0)
	h.holdMiningUntilComplete()
	h.hold(client.Movement{}, 10)
	h.flushAllStates()
	h.assertConverged(1e-5)
	h.assertWorldHashesEqual()
	result := h.replayResult()
	h.closeAndAssertNoGoroutineLeak(waitDeadline)
	return result
}

func (h *delayedPlayerHarness) nextSequence() uint64 {
	h.sequence++
	return h.sequence
}

func (h *delayedPlayerHarness) send(message network.ClientMessage) {
	h.t.Helper()
	if err := h.sendError(message); err != nil {
		h.t.Fatalf("ClientEndpoint.Send(%T): %v", message, err)
	}
}

func (h *delayedPlayerHarness) sendError(message network.ClientMessage) error {
	ctx, cancel := context.WithTimeout(context.Background(), waitDeadline)
	defer cancel()
	return h.clientEndpoint.Send(ctx, message)
}

func assertVectorClose(
	t *testing.T,
	name string,
	got, want [3]float32,
	tolerance float32,
) {
	t.Helper()
	for axis := range 3 {
		if difference := float32(math.Abs(float64(got[axis] - want[axis]))); difference > tolerance {
			t.Fatalf(
				"%s[%d]=%.9f，权威=%.9f，difference=%.9f tolerance=%g",
				name,
				axis,
				got[axis],
				want[axis],
				difference,
				tolerance,
			)
		}
	}
}

func goroutineDump() string {
	buffer := make([]byte, 1<<20)
	length := runtime.Stack(buffer, true)
	return string(buffer[:length])
}
