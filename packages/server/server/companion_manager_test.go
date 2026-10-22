// Companion Manager 的 tick 边界编排测试：端到端任务事件序列、FIFO 顺序、
// QueueFull 同步拒绝不调 planner、慢 planner 不阻塞权威 tick、每伙伴单在途与全服
// 四并发、过时结果丢弃、路径不可达与世界时间超时、关服顺序与 Memory/TCP
// parity。全部使用显式 typed planner seam，绝不访问模型服务。
package server

import (
	"context"
	"encoding/json"
	"fmt"
	"log/slog"
	"reflect"
	"slices"
	"sync"
	"testing"
	"time"

	"github.com/channing771/mornlea/packages/server/sim/runtime"
	"github.com/channing771/mornlea/packages/server/storage"
	"github.com/channing771/mornlea/packages/shared/companion"
	"github.com/channing771/mornlea/packages/shared/core"
	"github.com/channing771/mornlea/packages/shared/network"
	"github.com/channing771/mornlea/packages/shared/pathfind"
	"github.com/channing771/mornlea/packages/shared/physics"
)

// fakeCompanionModel 是 typed planner seam 的受控状态：按配置返回固定 go_to 计划，可整体
// 阻塞全部在途请求，并统计请求数、峰值并发与 context 取消次数。配置了
// planScript 时改为逐请求返回脚本条目（耗尽后重复最后一条），供 follow 等
// 需要按请求区分计划形态的测试使用。
type fakeCompanionModel struct {
	mu          sync.Mutex
	requests    int
	inFlight    int
	peak        int
	cancels     int
	block       chan struct{}
	steps       [][3]int32
	script      []string
	served      int
	status      int
	cancelOrder *shutdownOrderLog
}

func newFakeCompanionModel(t *testing.T, steps ...[3]int32) *fakeCompanionModel {
	t.Helper()
	return &fakeCompanionModel{steps: steps}
}

// setPlanScript 配置逐请求计划脚本：第 N 次请求返回 script[N]（完整的计划
// JSON 文本，不含 chat envelope），耗尽后重复最后一条；未配置时沿用 steps
// 构造的固定 go_to 计划。脚本令同一假模型能对「首次 follow、后续 go_to」
// 这类按请求变化的模型行为建模。
func (model *fakeCompanionModel) setPlanScript(script ...string) {
	model.mu.Lock()
	model.script = script
	model.mu.Unlock()
}

// planContentJSON 构造 Agent typed plan seam 使用的候选计划 JSON 文本。
func planContentJSON(steps [][3]int32) string {
	type wireStep struct {
		Kind string `json:"kind"`
		X    int32  `json:"x"`
		Y    int32  `json:"y"`
		Z    int32  `json:"z"`
	}
	type wirePlan struct {
		Summary string     `json:"summary"`
		Steps   []wireStep `json:"steps"`
	}
	plan := wirePlan{Summary: "按指令移动", Steps: make([]wireStep, 0, len(steps))}
	for _, step := range steps {
		plan.Steps = append(plan.Steps, wireStep{Kind: "go_to", X: step[0], Y: step[1], Z: step[2]})
	}
	encoded, _ := json.Marshal(plan)
	return string(encoded)
}

func (model *fakeCompanionModel) holdRequests() {
	model.mu.Lock()
	model.block = make(chan struct{})
	model.mu.Unlock()
}

func (model *fakeCompanionModel) releaseRequests() {
	model.mu.Lock()
	block := model.block
	model.block = nil
	model.mu.Unlock()
	if block != nil {
		close(block)
	}
}

func (model *fakeCompanionModel) snapshotCounts() (requests, peak, inFlight, cancels int) {
	model.mu.Lock()
	defer model.mu.Unlock()
	return model.requests, model.peak, model.inFlight, model.cancels
}

// shutdownOrderLog 记录关服期间跨组件事件的相对顺序。
type shutdownOrderLog struct {
	mu     sync.Mutex
	events []string
}

func (log *shutdownOrderLog) record(event string) {
	log.mu.Lock()
	log.events = append(log.events, event)
	log.mu.Unlock()
}

func (log *shutdownOrderLog) snapshot() []string {
	log.mu.Lock()
	defer log.mu.Unlock()
	return slices.Clone(log.events)
}

// newCompanionManagerHost 构造启用了任务编排的 Host；model 非 nil 时显式替换
// typed planner seam，否则保持生产 Agent planner 缺省。
func newCompanionManagerHost(
	t *testing.T,
	definitions []companion.Definition,
	model *fakeCompanionModel,
	modify func(*Config),
) *Host {
	t.Helper()
	config := hostTestConfig()
	config.Companions = append([]companion.Definition(nil), definitions...)
	config.MaxPlayers = 2
	config.OutboxCapacity = 4096
	config.HeartbeatInterval = time.Hour
	config.HeartbeatTimeout = time.Hour
	if modify != nil {
		modify(&config)
	}
	host := mustNewHost(t, config, flatTestGenerator{}, newHostTestStore())
	if model != nil {
		host.world.companionManager.replacePlannerForTest(t, model)
	}
	t.Cleanup(func() {
		ctx, cancel := context.WithTimeout(context.Background(), longWaitDeadline)
		defer cancel()
		if err := host.Shutdown(ctx); err != nil {
			t.Errorf("Host.Shutdown: %v", err)
		}
	})
	return host
}

// replacePlannerForTest 通过显式 seam 注入受控 planner；生产不保留 direct-model
// client 或 fallback。
func (m *companionManager) replacePlannerForTest(t *testing.T, model *fakeCompanionModel) {
	t.Helper()
	m.planner = fakeCompanionPlanner{model: model}
}

type fakeCompanionPlanner struct {
	model *fakeCompanionModel
}

func (p fakeCompanionPlanner) Plan(ctx context.Context, request companionPlanningRequest) (companionPlanningOutcome, error) {
	model := p.model
	model.mu.Lock()
	model.requests++
	model.inFlight++
	if model.inFlight > model.peak {
		model.peak = model.inFlight
	}
	block := model.block
	status := model.status
	steps := slices.Clone(model.steps)
	content := ""
	if len(model.script) > 0 {
		index := model.served
		if index >= len(model.script) {
			index = len(model.script) - 1
		}
		model.served++
		content = model.script[index]
	}
	model.mu.Unlock()
	defer func() {
		model.mu.Lock()
		model.inFlight--
		model.mu.Unlock()
	}()
	if block != nil {
		select {
		case <-block:
		case <-ctx.Done():
			model.mu.Lock()
			model.cancels++
			cancels := model.cancels
			model.mu.Unlock()
			if model.cancelOrder != nil && cancels == 1 {
				model.cancelOrder.record("model-cancel")
			}
			return companionPlanningOutcome{}, companion.ErrPlannerUnavailable
		}
	}
	if status != 0 {
		return companionPlanningOutcome{}, companion.ErrPlannerUnavailable
	}
	var plan companion.Plan
	if content != "" {
		var candidate companion.AgentPlan
		if err := json.Unmarshal([]byte(content), &candidate); err != nil {
			return companionPlanningOutcome{}, companion.ErrPlannerInvalidPlan
		}
		decoded, err := companion.DecodeAgentPlan(candidate, request.Snapshot)
		if err != nil {
			return companionPlanningOutcome{}, err
		}
		plan = decoded
	} else {
		plan = companion.Plan{Summary: "按指令移动", Steps: make([]companion.PlanStep, 0, len(steps))}
		for _, step := range steps {
			plan.Steps = append(plan.Steps, companion.PlanStep{
				Kind: companion.PlanStepGoTo, X: step[0], Y: step[1], Z: step[2],
			})
		}
	}
	_, digest, err := companion.CanonicalSnapshotDigest(request.Snapshot)
	if err != nil {
		return companionPlanningOutcome{}, companion.ErrPlannerUnavailable
	}
	return companionPlanningOutcome{
		Plan: plan, Generation: request.Generation, Attempt: request.Attempt,
		RunID:          "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa",
		SnapshotID:     "bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb",
		SnapshotDigest: digest,
		requestIdentity: companionPlanningIdentity{
			RunID:          "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa",
			SnapshotID:     "bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb",
			SnapshotDigest: digest,
		},
	}, nil
}

// stepCollectingChatEvents 逐 tick 推进并收集客户端收到的 ChatEvent，直到
// stop 返回 true 或达到 maxTicks。
func stepCollectingChatEvents(
	t *testing.T,
	host *Host,
	client network.ClientEndpoint,
	maxTicks int,
	stop func(events []network.ChatEvent) bool,
) []network.ChatEvent {
	t.Helper()
	var collected []network.ChatEvent
	for range maxTicks {
		result := host.world.StepForTest()
		collected = append(collected,
			companionChatEvents(receiveCompanionChatTick(t, client, result.Tick))...)
		if stop != nil && stop(collected) {
			return collected
		}
	}
	return collected
}

// stepUntilCompanionEvents 逐 tick 推进并收集全部客户端收到的 ChatEvent，
// 直到 stop 命中；上限是 longWaitDeadline 的墙钟而非固定 tick 数。规划/寻路
// worker 的结果只在 tick 边界被应用：non-race 短测里单次 tick 的真实耗时远
// 小于生产节拍（50ms），一轮异步规划在同步快进 tick 下要跨越数百 tick 才
// 落地，固定 tick 上限会把「worker 尚未投递」误判成断言失败（race 模式因
// 每 tick 变慢而掩盖同一时序耦合）。墙钟限界等待的是同一确定性事件流，
// 两种构建模式下断言语义不变；上限只防御真实回归导致的悬挂。每轮推进后
// sleep 一毫秒让 worker 与发布 goroutine 获得调度——与
// stepUntilCompanionManagerReady 的既有让步模式一致，同时避免热轮询放大
// CPU 争用。stop 为 nil 的调用方请继续用固定窗口的 stepCollectingChatEvents
// （静置观察语义），不要用本 helper。
func stepUntilCompanionEvents(
	t *testing.T,
	host *Host,
	clients []network.ClientEndpoint,
	stop func(events []network.ChatEvent) bool,
) []network.ChatEvent {
	t.Helper()
	var collected []network.ChatEvent
	deadline := time.Now().Add(longWaitDeadline)
	for time.Now().Before(deadline) {
		result := host.world.StepForTest()
		for _, endpoint := range clients {
			collected = append(collected,
				companionChatEvents(receiveCompanionChatTick(t, endpoint, result.Tick))...)
		}
		if stop != nil && stop(collected) {
			return collected
		}
		time.Sleep(time.Millisecond)
	}
	return collected
}

func chatEventKinds(events []network.ChatEvent) []network.ChatEventKind {
	kinds := make([]network.ChatEventKind, len(events))
	for index, event := range events {
		kinds[index] = event.Kind
	}
	return kinds
}

func assertStrictlyIncreasingEventIDs(t *testing.T, events []network.ChatEvent) {
	t.Helper()
	for index := 1; index < len(events); index++ {
		if events[index].EventID <= events[index-1].EventID {
			t.Fatalf("EventID 非严格递增：%+v", chatEventKinds(events))
		}
	}
}

func waitForModelRequests(t *testing.T, model *fakeCompanionModel, want int) {
	t.Helper()
	waitIntegrationCondition(t, "假模型请求数", func() bool {
		requests, _, _, _ := model.snapshotCounts()
		return requests >= want
	})
}

// companionManagerHostReady 建好世界并登录发令者，返回 host、客户端与首个
// 伙伴的出生身体事实（供测试按位置构造计划目标）。每个 tick 都同步消费
// 客户端消息，保证返回后的下一步接收不会命中滞留的旧 tick。
func companionManagerHostReady(
	t *testing.T,
	definitions []companion.Definition,
	model *fakeCompanionModel,
) (*Host, network.ClientEndpoint, companion.Body) {
	t.Helper()
	host := newCompanionManagerHost(t, definitions, model, nil)
	client := openCompanionChatClient(t, host, "memory", integrationIdentity(0x71, "发令者"))
	body := stepUntilCompanionManagerReady(
		t, host, []network.ClientEndpoint{client}, definitions[0].ID,
	)
	return host, client, body
}

// stepUntilCompanionManagerReady 推进到玩家 Ready 且目标伙伴激活，逐 tick
// 消费全部客户端消息以保持流同步。
func stepUntilCompanionManagerReady(
	t *testing.T,
	host *Host,
	clients []network.ClientEndpoint,
	wantID companion.ID,
) companion.Body {
	t.Helper()
	deadline := time.Now().Add(longWaitDeadline)
	for time.Now().Before(deadline) {
		result := host.world.StepForTest()
		ready := true
		for _, endpoint := range clients {
			messages := receiveCompanionChatTick(t, endpoint, result.Tick)
			state, ok := messages[len(messages)-1].(network.PlayerState)
			ready = ready && ok && state.Ready
		}
		for _, body := range host.world.engine.CompanionBodies() {
			if body.ID == wantID && ready {
				return body
			}
		}
		time.Sleep(time.Millisecond)
	}
	t.Fatalf("任务测试世界未就绪：companions=%d", len(host.world.engine.CompanionBodies()))
	return companion.Body{}
}

// currentCompanionBody 读取已激活伙伴的当前身体事实，不推进 tick（保持客户端
// 消息流与 tick 同步的测试约定）。
func currentCompanionBody(t *testing.T, host *Host, id companion.ID) companion.Body {
	t.Helper()
	deadline := time.Now().Add(shortWaitDeadline)
	for time.Now().Before(deadline) {
		for _, body := range host.world.engine.CompanionBodies() {
			if body.ID == id {
				return body
			}
		}
		time.Sleep(time.Millisecond)
	}
	t.Fatalf("伙伴 %s 未激活", id)
	return companion.Body{}
}

func TestCompanionManagerTaskLifecycleEvents(t *testing.T) {
	definitions := []companion.Definition{{ID: chatTestCompanionID(1), Name: "阿木"}}
	host, client, body := companionManagerHostReady(t, definitions, nil)
	baseX, baseZ := int32(body.Position[0]), int32(body.Position[2])
	model := newFakeCompanionModel(t,
		[3]int32{baseX + 3, 1, baseZ},
		[3]int32{baseX + 6, 1, baseZ},
	)
	host.world.companionManager.replacePlannerForTest(t, model)

	sendIntegration(t, client, network.ChatCommand{Text: "@阿木 走到那边"})
	events := stepCollectingChatEvents(t, host, client, 400, func(events []network.ChatEvent) bool {
		return slices.Contains(chatEventKinds(events), network.ChatEventTaskCompleted)
	})

	wantKinds := []network.ChatEventKind{
		network.ChatEventAccepted,
		network.ChatEventTaskStarted,
		network.ChatEventTaskProgress,
		network.ChatEventTaskCompleted,
	}
	if !reflect.DeepEqual(chatEventKinds(events), wantKinds) {
		t.Fatalf("事件序列=%v，想要 %v", chatEventKinds(events), wantKinds)
	}
	assertStrictlyIncreasingEventIDs(t, events)
	issuer := integrationIdentity(0x71, "发令者")
	for _, event := range events {
		if err := event.Validate(); err != nil {
			t.Fatalf("事件 %d Validate: %v", event.EventID, err)
		}
		if event.PlayerID != issuer.PlayerID || event.PlayerName != "发令者" ||
			event.CompanionID != definitions[0].ID || event.CompanionName != "阿木" ||
			event.Command != "走到那边" {
			t.Fatalf("事件身份不完整：%+v", event)
		}
	}
	final := currentCompanionBody(t, host, definitions[0].ID)
	if offset := final.Position[0] - body.Position[0]; offset < 5 || offset > 7 {
		t.Fatalf("完成位置偏移=%f，想要约 6 格", offset)
	}
	// 终态后不再有任何任务事件。
	quiet := stepCollectingChatEvents(t, host, client, 3, nil)
	if len(quiet) != 0 {
		t.Fatalf("终态后事件=%v", chatEventKinds(quiet))
	}
}

func TestCompanionManagerFIFOExecutesCommandsInOrder(t *testing.T) {
	definitions := []companion.Definition{{ID: chatTestCompanionID(1), Name: "阿木"}}
	host, client, body := companionManagerHostReady(t, definitions, nil)
	baseX, baseZ := int32(body.Position[0]), int32(body.Position[2])
	model := newFakeCompanionModel(t,
		[3]int32{baseX + 2, 1, baseZ},
		[3]int32{baseX + 4, 1, baseZ},
		[3]int32{baseX + 6, 1, baseZ},
	)
	host.world.companionManager.replacePlannerForTest(t, model)

	for _, text := range []string{"@阿木 第一", "@阿木 第二", "@阿木 第三"} {
		sendIntegration(t, client, network.ChatCommand{Text: text})
	}
	waitForIncomingChatDepth(t, host.world, 3)
	events := stepUntilCompanionEvents(t, host, []network.ClientEndpoint{client}, func(events []network.ChatEvent) bool {
		return countKind(events, network.ChatEventTaskCompleted) == 3
	})

	// 逐条指令的生命周期互不交叠：前一条终态之后下一条才 TaskStarted。
	type lifecycle struct {
		started  int
		finished int
	}
	byCommand := map[string]*lifecycle{}
	order := make([]string, 0, 3)
	for _, event := range events {
		switch event.Kind {
		case network.ChatEventTaskStarted, network.ChatEventTaskProgress,
			network.ChatEventTaskCompleted:
			entry, ok := byCommand[event.Command]
			if !ok {
				entry = &lifecycle{}
				byCommand[event.Command] = entry
				order = append(order, event.Command)
			}
			if event.Kind == network.ChatEventTaskStarted {
				entry.started++
			}
			if event.Kind == network.ChatEventTaskCompleted {
				entry.finished++
			}
		}
	}
	if !reflect.DeepEqual(order, []string{"第一", "第二", "第三"}) {
		t.Fatalf("执行顺序=%v，想要接收顺序 第一/第二/第三", order)
	}
	for _, command := range order {
		entry := byCommand[command]
		if entry.started != 1 || entry.finished != 1 {
			t.Fatalf("指令 %q started=%d finished=%d，想要各 1", command, entry.started, entry.finished)
		}
	}
	assertStrictlyIncreasingEventIDs(t, events)
}

func TestChatCommandQueueFullRejectsWithoutModelCall(t *testing.T) {
	definitions := []companion.Definition{{ID: chatTestCompanionID(1), Name: "阿木"}}
	model := newFakeCompanionModel(t, [3]int32{0, 1, 0})
	model.holdRequests()
	host := newCompanionManagerHost(t, definitions, model, nil)
	sender := openCompanionChatClient(t, host, "memory", integrationIdentity(0x72, "发令者"))
	observer := openCompanionChatClient(t, host, "memory", integrationIdentity(0x73, "旁观者"))
	waitForCompanionChatWorld(t, host, []network.ClientEndpoint{sender, observer}, 1)

	for index := range companion.MaxTaskQueueDepth + 1 {
		sendIntegration(t, sender, network.ChatCommand{Text: fmt.Sprintf("@阿木 指令%d", index)})
	}
	waitForIncomingChatDepth(t, host.world, companion.MaxTaskQueueDepth+1)
	result := host.world.StepForTest()
	senderEvents := companionChatEvents(receiveCompanionChatTick(t, sender, result.Tick))
	observerEvents := companionChatEvents(receiveCompanionChatTick(t, observer, result.Tick))

	accepted := countKind(senderEvents, network.ChatEventAccepted)
	rejected := eventsWithKind(senderEvents, network.ChatEventRejected)
	if accepted != companion.MaxTaskQueueDepth || len(rejected) != 1 {
		t.Fatalf("Accepted=%d QueueFull=%d，想要 %d/1",
			accepted, len(rejected), companion.MaxTaskQueueDepth)
	}
	full := rejected[0]
	if full.RejectReason != network.ChatRejectQueueFull || full.CompanionID != definitions[0].ID ||
		full.Command != fmt.Sprintf("指令%d", companion.MaxTaskQueueDepth) {
		t.Fatalf("QueueFull 事件=%+v", full)
	}
	if err := full.Validate(); err != nil {
		t.Fatalf("QueueFull Validate: %v", err)
	}
	if countKind(observerEvents, network.ChatEventAccepted) != companion.MaxTaskQueueDepth ||
		countKind(observerEvents, network.ChatEventRejected) != 0 {
		t.Fatalf("旁观者事件=%v，QueueFull 不得广播", chatEventKinds(observerEvents))
	}

	waitForModelRequests(t, model, 1)
	requests, _, inFlight, _ := model.snapshotCounts()
	if requests != 1 || inFlight != 1 {
		t.Fatalf("模型调用=%d 在途=%d，QueueFull 必须同步拒绝且只派发队首", requests, inFlight)
	}
	host.world.stepMu.Lock()
	slot := host.world.companionManager.slots[definitions[0].ID]
	depth := slot.queue.Len()
	current, hasCurrent := slot.queue.Current()
	inFlightFlag := slot.planningInFlight
	host.world.stepMu.Unlock()
	if depth != companion.MaxTaskQueueDepth-1 || !hasCurrent ||
		current.State != companion.TaskPlanning || !inFlightFlag {
		t.Fatalf("FIFO depth=%d current=%+v inFlight=%v，既有队列被破坏", depth, current, inFlightFlag)
	}
	if cap(host.world.companionManager.semaphore) != 4 {
		t.Fatalf("模型并发信号量容量=%d，想要 4", cap(host.world.companionManager.semaphore))
	}
}

func TestCompanionManagerSlowModelDoesNotBlockTicks(t *testing.T) {
	definitions := []companion.Definition{{ID: chatTestCompanionID(1), Name: "阿木"}}
	model := newFakeCompanionModel(t, [3]int32{0, 1, 0})
	model.holdRequests()
	host, client, _ := companionManagerHostReady(t, definitions, model)

	sendIntegration(t, client, network.ChatCommand{Text: "@阿木 慢慢想"})
	waitForIncomingChatDepth(t, host.world, 1)
	before := host.world.StepForTest()
	receiveCompanionChatTick(t, client, before.Tick)
	waitForModelRequests(t, model, 1)

	started := time.Now()
	const extraTicks = 20
	for range extraTicks {
		result := host.world.StepForTest()
		receiveCompanionChatTick(t, client, result.Tick)
	}
	elapsed := time.Since(started)
	if after := host.world.TickCount(); after-before.Tick != extraTicks {
		t.Fatalf("tick 推进=%d，想要 %d", after-before.Tick, extraTicks)
	}
	// 挂起的模型请求期间，20 个 tick 必须远快于真实节拍 1 秒；阻塞边界放宽到
	// 2 秒以容纳 race 检测下的抖动。
	if elapsed > 2*time.Second {
		t.Fatalf("挂起模型期间 %d tick 耗时=%v，权威 tick 被阻塞", extraTicks, elapsed)
	}
	if _, _, inFlight, _ := model.snapshotCounts(); inFlight != 1 {
		t.Fatalf("模型在途=%d，想要 1", inFlight)
	}
	model.releaseRequests()
}

func TestCompanionManagerOneInFlightRequestPerCompanion(t *testing.T) {
	definitions := []companion.Definition{{ID: chatTestCompanionID(1), Name: "阿木"}}
	model := newFakeCompanionModel(t,
		[3]int32{1, 1, 0},
		[3]int32{2, 1, 0},
	)
	model.holdRequests()
	host, client, _ := companionManagerHostReady(t, definitions, model)

	sendIntegration(t, client, network.ChatCommand{Text: "@阿木 第一条"})
	sendIntegration(t, client, network.ChatCommand{Text: "@阿木 第二条"})
	waitForIncomingChatDepth(t, host.world, 2)
	for range 8 {
		result := host.world.StepForTest()
		receiveCompanionChatTick(t, client, result.Tick)
	}
	waitForModelRequests(t, model, 1)
	if requests, _, _, _ := model.snapshotCounts(); requests != 1 {
		t.Fatalf("在途期间模型请求数=%d，同一伙伴必须最多一个在途规划请求", requests)
	}

	// 释放后第一条完成，第二条才发起自己的请求；两条转换都发生在后续
	// tick 边界，等待期间必须持续推进世界。等待以墙钟限界：规划 worker
	// 的结果只在 tick 边界被应用，non-race 快进 tick 下一轮异步规划要跨
	// 数百 tick 才能落地，固定 tick 上限会过早放弃（race 模式每 tick 更慢
	// 而掩盖了这一时序）。
	model.releaseRequests()
	dispatchedSecond := false
	deadline := time.Now().Add(longWaitDeadline)
	for time.Now().Before(deadline) {
		result := host.world.StepForTest()
		receiveCompanionChatTick(t, client, result.Tick)
		if requests, _, _, _ := model.snapshotCounts(); requests >= 2 {
			dispatchedSecond = true
			break
		}
		time.Sleep(time.Millisecond)
	}
	if !dispatchedSecond {
		t.Fatal("释放后第二条指令始终未发起规划请求")
	}
	if requests, _, _, _ := model.snapshotCounts(); requests != 2 {
		t.Fatalf("释放后模型请求数=%d，想要 2", requests)
	}
}

func TestCompanionManagerFourConcurrentModelRequests(t *testing.T) {
	definitions := []companion.Definition{
		{ID: chatTestCompanionID(1), Name: "阿木"},
		{ID: chatTestCompanionID(2), Name: "阿木甲"},
		{ID: chatTestCompanionID(3), Name: "小石"},
		{ID: chatTestCompanionID(4), Name: "松果"},
	}
	model := newFakeCompanionModel(t, [3]int32{0, 1, 0})
	model.holdRequests()
	host := newCompanionManagerHost(t, definitions, model, nil)
	sender := openCompanionChatClient(t, host, "memory", integrationIdentity(0x74, "发令者"))
	waitForCompanionChatWorld(t, host, []network.ClientEndpoint{sender}, len(definitions))

	for _, definition := range definitions {
		sendIntegration(t, sender, network.ChatCommand{Text: "@" + definition.Name + " 出发"})
	}
	waitForIncomingChatDepth(t, host.world, len(definitions))
	result := host.world.StepForTest()
	receiveCompanionChatTick(t, sender, result.Tick)

	waitForModelRequests(t, model, len(definitions))
	if _, peak, _, _ := model.snapshotCounts(); peak != len(definitions) {
		t.Fatalf("峰值并发=%d，想要四个伙伴全部并发（上限 4）", peak)
	}
	model.releaseRequests()
}

func TestCompanionManagerStalePlannerResultDiscarded(t *testing.T) {
	definitions := []companion.Definition{{ID: chatTestCompanionID(1), Name: "阿木"}}
	model := newFakeCompanionModel(t, [3]int32{1, 1, 0})
	model.holdRequests()
	host, client, _ := companionManagerHostReady(t, definitions, model)

	sendIntegration(t, client, network.ChatCommand{Text: "@阿木 会过时"})
	waitForIncomingChatDepth(t, host.world, 1)
	result := host.world.StepForTest()
	receiveCompanionChatTick(t, client, result.Tick)
	waitForModelRequests(t, model, 1)

	// 模拟任务在结果在途期间进入终态（关服冻结后的丢弃路径）：直接把当前
	// 任务置为 Failed，晚到的成功计划绝不能让任务复活。
	host.world.stepMu.Lock()
	host.world.companionManager.slots[definitions[0].ID].queue.FailPlanning(
		companion.TaskFailPlannerUnavailable,
	)
	host.world.stepMu.Unlock()
	model.releaseRequests()

	events := stepCollectingChatEvents(t, host, client, 10, nil)
	if slices.Contains(chatEventKinds(events), network.ChatEventTaskStarted) {
		t.Fatalf("过时结果复活了任务：%v", chatEventKinds(events))
	}
	host.world.stepMu.Lock()
	_, hasCurrent := host.world.companionManager.slots[definitions[0].ID].queue.Current()
	host.world.stepMu.Unlock()
	if hasCurrent {
		t.Fatal("过时结果重新占据当前任务槽")
	}
	if requests, _, _, _ := model.snapshotCounts(); requests != 1 {
		t.Fatalf("过时结果触发了新请求=%d", requests)
	}
}

func TestCompanionManagerDistantGoalFailsPathUnreachable(t *testing.T) {
	definitions := []companion.Definition{{ID: chatTestCompanionID(1), Name: "阿木"}}
	host, client, body := companionManagerHostReady(t, definitions, nil)
	model := newFakeCompanionModel(t,
		[3]int32{int32(body.Position[0]) + 1000, 1, int32(body.Position[2])})
	host.world.companionManager.replacePlannerForTest(t, model)

	sendIntegration(t, client, network.ChatCommand{Text: "@阿木 去远方"})
	events := stepUntilCompanionEvents(t, host, []network.ClientEndpoint{client}, func(events []network.ChatEvent) bool {
		return slices.Contains(chatEventKinds(events), network.ChatEventTaskFailed)
	})
	failed := eventsWithKind(events, network.ChatEventTaskFailed)
	if len(failed) != 1 {
		t.Fatalf("TaskFailed=%d，想要 1（事件=%v）", len(failed), chatEventKinds(events))
	}
	if network.TaskFailReason(failed[0].RejectReason) != network.TaskFailPathUnreachable {
		t.Fatalf("失败原因=%d，想要 PathUnreachable", failed[0].RejectReason)
	}
	// 目标在寻路窗口外，伙伴必须原地不动。
	final := currentCompanionBody(t, host, definitions[0].ID)
	if final.Position != body.Position {
		t.Fatalf("不可达任务产生了位移：%v -> %v", body.Position, final.Position)
	}
	// 终态后 FIFO 继续接受新指令。
	sendIntegration(t, client, network.ChatCommand{Text: "@阿木 再试"})
	waitForIncomingChatDepth(t, host.world, 1)
	result := host.world.StepForTest()
	events = companionChatEvents(receiveCompanionChatTick(t, client, result.Tick))
	if len(events) != 1 || events[0].Kind != network.ChatEventAccepted {
		t.Fatalf("终态后新指令事件=%v", chatEventKinds(events))
	}
}

func TestCompanionManagerTaskTimesOutAtWorldTimeDeadline(t *testing.T) {
	definitions := []companion.Definition{{ID: chatTestCompanionID(1), Name: "阿木"}}
	host := newCompanionManagerHost(t, definitions, nil, func(config *Config) {
		config.TaskTimeoutMinutes = 1
	})
	client := openCompanionChatClient(t, host, "memory", integrationIdentity(0x75, "发令者"))
	body := stepUntilCompanionManagerReady(
		t, host, []network.ClientEndpoint{client}, definitions[0].ID,
	)
	baseX, baseZ := int32(body.Position[0]), int32(body.Position[2])
	// 20 步 × 15 格 ≈ 300 格 ≈ 1400+ tick 的行程；1 分钟 deadline（1200 tick）
	// 必然在途中命中，TimedOut 之后移动停在当前位置。
	steps := make([][3]int32, 0, 20)
	for index := 1; index <= 20; index++ {
		steps = append(steps, [3]int32{baseX + int32(index)*15, 1, baseZ})
	}
	model := newFakeCompanionModel(t, steps...)
	host.world.companionManager.replacePlannerForTest(t, model)

	sendIntegration(t, client, network.ChatCommand{Text: "@阿木 长途"})
	events := stepCollectingChatEvents(t, host, client, 4000, func(events []network.ChatEvent) bool {
		return slices.Contains(chatEventKinds(events), network.ChatEventTaskTimedOut)
	})
	timedOut := eventsWithKind(events, network.ChatEventTaskTimedOut)
	if len(timedOut) != 1 {
		t.Fatalf("TaskTimedOut=%d（事件=%v）", len(timedOut), chatEventKinds(events))
	}
	if timedOut[0].RejectReason != network.ChatRejectNone {
		t.Fatalf("TimedOut reason=%d，想要 None", timedOut[0].RejectReason)
	}
	if !slices.Contains(chatEventKinds(events), network.ChatEventTaskStarted) {
		t.Fatalf("缺少 TaskStarted：%v", chatEventKinds(events))
	}
	// 超时后移动必须停在当前位置。
	stop := currentCompanionBody(t, host, definitions[0].ID)
	for range 5 {
		host.world.StepForTest()
	}
	settled := currentCompanionBody(t, host, definitions[0].ID)
	dx := settled.Position[0] - stop.Position[0]
	dz := settled.Position[2] - stop.Position[2]
	if dx*dx+dz*dz > 0.01 {
		t.Fatalf("超时后仍在移动：%v -> %v", stop.Position, settled.Position)
	}
}

func TestCompanionShutdownCancelsPlannerBeforeFinalSaveAndStore(t *testing.T) {
	definitions := []companion.Definition{{ID: chatTestCompanionID(1), Name: "阿木"}}
	order := &shutdownOrderLog{}
	model := newFakeCompanionModel(t, [3]int32{0, 1, 0})
	model.holdRequests()
	model.cancelOrder = order

	config := hostTestConfig()
	config.Companions = definitions
	config.MaxPlayers = 1
	config.OutboxCapacity = 4096
	config.HeartbeatInterval = time.Hour
	config.HeartbeatTimeout = time.Hour
	config.companionPlanner = fakeCompanionPlanner{model: model}
	store := newCompanionManagerOrderStore(order)
	host, err := NewHost(context.Background(), config, flatTestGenerator{}, store)
	if err != nil {
		t.Fatalf("NewHost: %v", err)
	}
	client := openCompanionChatClient(t, host, "memory", integrationIdentity(0x76, "发令者"))
	waitForCompanionChatWorld(t, host, []network.ClientEndpoint{client}, 1)

	sendIntegration(t, client, network.ChatCommand{Text: "@阿木 关服前"})
	waitForIncomingChatDepth(t, host.world, 1)
	result := host.world.StepForTest()
	receiveCompanionChatTick(t, client, result.Tick)
	waitForModelRequests(t, model, 1)

	ctx, cancel := context.WithTimeout(context.Background(), waitDeadline)
	defer cancel()
	if err := host.Shutdown(ctx); err != nil {
		t.Fatalf("Host.Shutdown: %v", err)
	}

	rank := map[string]int{}
	for index, event := range order.snapshot() {
		rank[event] = index
	}
	wantOrder := []string{"model-cancel", "companion-save", "sync", "close"}
	for index := 1; index < len(wantOrder); index++ {
		previous, okPrevious := rank[wantOrder[index-1]]
		current, okCurrent := rank[wantOrder[index]]
		if !okPrevious || !okCurrent || previous > current {
			t.Fatalf("关服顺序=%v，想要 %v 依序出现", order.snapshot(), wantOrder)
		}
	}
	// 冻结后 tick 不再推进，ChatCommand 不再被处理。
	if frozen := host.world.StepForTest(); frozen.Tick != 0 {
		t.Fatalf("冻结后 Step tick=%d，想要空结果", frozen.Tick)
	}
	if _, _, _, cancels := model.snapshotCounts(); cancels == 0 {
		t.Fatal("关服未取消在途模型请求")
	}
}

// companionManagerOrderStore 把伙伴保存与世界存储的关服动作记录进同一顺序
// 日志，供关服顺序断言。
type companionManagerOrderStore struct {
	*hostTestStore
	order *shutdownOrderLog
}

func newCompanionManagerOrderStore(order *shutdownOrderLog) *companionManagerOrderStore {
	return &companionManagerOrderStore{hostTestStore: newHostTestStore(), order: order}
}

func (store *companionManagerOrderStore) SaveCompanions(
	ctx context.Context,
	save storage.CompanionSave,
) error {
	store.order.record("companion-save")
	return store.hostTestStore.MemoryStore.SaveCompanions(ctx, fixtureServerCompanionV5Save(save))
}

func (store *companionManagerOrderStore) Sync(ctx context.Context) error {
	store.order.record("sync")
	return store.hostTestStore.Sync(ctx)
}

func (store *companionManagerOrderStore) Close() error {
	store.order.record("close")
	return store.hostTestStore.Close()
}

func TestChatCommandTaskEventsMemoryTCPParity(t *testing.T) {
	results := make(map[string][]companionChatTranscriptEvent, 2)
	for _, transport := range []string{"memory", "tcp"} {
		transport := transport
		t.Run(transport, func(t *testing.T) {
			results[transport] = runCompanionManagerParity(t, transport)
		})
	}
	if !reflect.DeepEqual(results["memory"], results["tcp"]) {
		t.Fatalf("Memory/TCP 任务事件 transcript 不一致\nMemory=%+v\nTCP=%+v",
			results["memory"], results["tcp"])
	}
	got := results["memory"]
	senderEvents := make([]network.ChatEventKind, 0, 5)
	observerEvents := make([]network.ChatEventKind, 0, 4)
	for _, entry := range got {
		if entry.Recipient == 0 {
			senderEvents = append(senderEvents, entry.Event.Kind)
		} else {
			observerEvents = append(observerEvents, entry.Event.Kind)
		}
	}
	wantSender := []network.ChatEventKind{
		network.ChatEventAccepted,
		network.ChatEventRejected,
		network.ChatEventTaskStarted,
		network.ChatEventTaskProgress,
		network.ChatEventTaskCompleted,
	}
	wantObserver := []network.ChatEventKind{
		network.ChatEventAccepted,
		network.ChatEventTaskStarted,
		network.ChatEventTaskProgress,
		network.ChatEventTaskCompleted,
	}
	if !reflect.DeepEqual(senderEvents, wantSender) {
		t.Fatalf("发令者事件=%v，想要 %v", senderEvents, wantSender)
	}
	if !reflect.DeepEqual(observerEvents, wantObserver) {
		t.Fatalf("旁观者事件=%v，想要 %v", observerEvents, wantObserver)
	}
}

func runCompanionManagerParity(t *testing.T, transport string) []companionChatTranscriptEvent {
	t.Helper()
	definitions := chatTestDefinitions()[:1]
	host := newCompanionManagerHost(t, definitions, nil, nil)
	sender := openCompanionChatClient(t, host, transport, integrationIdentity(0x81, "发送者"))
	observer := openCompanionChatClient(t, host, transport, integrationIdentity(0x82, "观察者"))
	clients := []network.ClientEndpoint{sender, observer}
	body := stepUntilCompanionManagerReady(t, host, clients, definitions[0].ID)
	model := newFakeCompanionModel(t,
		[3]int32{int32(body.Position[0]) + 2, 1, int32(body.Position[2])},
		[3]int32{int32(body.Position[0]) + 4, 1, int32(body.Position[2])},
	)
	host.world.companionManager.replacePlannerForTest(t, model)

	sendIntegration(t, sender, network.ChatCommand{Text: "@阿木 走两步"})
	sendIntegration(t, sender, network.ChatCommand{Text: "@不存在 看看"})
	waitForIncomingChatDepth(t, host.world, 2)

	transcript := make([]companionChatTranscriptEvent, 0, 10)
	quiet := 0
	for range 600 {
		result := host.world.StepForTest()
		tickEvents := 0
		for recipient, endpoint := range clients {
			for _, event := range companionChatEvents(receiveCompanionChatTick(t, endpoint, result.Tick)) {
				transcript = append(transcript, companionChatTranscriptEvent{
					Recipient: recipient,
					Event:     event,
				})
				tickEvents++
			}
		}
		if tickEvents == 0 {
			quiet++
		} else {
			quiet = 0
		}
		completed := false
		for _, entry := range transcript {
			if entry.Event.Kind == network.ChatEventTaskCompleted {
				completed = true
			}
		}
		if completed && quiet >= 3 {
			break
		}
	}
	return transcript
}

func TestCompanionManagerPlanSnapshotBoundedAndOrdered(t *testing.T) {
	definitions := []companion.Definition{{ID: chatTestCompanionID(1), Name: "阿木"}}
	host, _, body := companionManagerHostReady(t, definitions, nil)
	active := activeLoginForPlayer(t, host, integrationIdentity(0x71, "发令者").PlayerID)

	host.world.stepMu.Lock()
	identity := integrationIdentity(0x71, "发令者")
	issuer := host.world.companionManager.captureIssuer(
		identity.PlayerID, "发令者", active.Session, runtime.ActiveTickTunables(),
	)
	snapshot, err := host.world.companionManager.buildPlanSnapshot(
		definitions[0], companion.TaskCommand("环顾四周"), issuer, body,
	)
	worldTime := host.world.engine.WorldTime()
	host.world.stepMu.Unlock()
	if err != nil {
		t.Fatalf("构造快照: %v", err)
	}
	if err := snapshot.Validate(); err != nil {
		t.Fatalf("快照 Validate: %v", err)
	}
	if snapshot.Command != "环顾四周" || snapshot.Companion.ID != definitions[0].ID {
		t.Fatalf("快照身份=%+v", snapshot)
	}
	if snapshot.WorldTimeTicks != worldTime {
		t.Fatalf("快照世界时间=%d，想要 %d", snapshot.WorldTimeTicks, worldTime)
	}
	if snapshot.Issuer.ID != identity.PlayerID {
		t.Fatalf("发令者 ID=%s", snapshot.Issuer.ID)
	}
	if len(snapshot.ExposedBlocks) > companion.MaxPlanExposedBlocks {
		t.Fatalf("暴露方块=%d，超过上限", len(snapshot.ExposedBlocks))
	}
	for index := 1; index < len(snapshot.ExposedBlocks); index++ {
		previous := snapshot.ExposedBlocks[index-1].Pos
		current := snapshot.ExposedBlocks[index].Pos
		if !blockPosAfterForSort(current, previous) {
			t.Fatalf("暴露方块未按 (X,Y,Z) 严格升序：%v 后跟 %v", previous, current)
		}
	}
	if len(snapshot.Heights) > companion.MaxPlanHeightSamples {
		t.Fatalf("高度样本=%d，超过上限", len(snapshot.Heights))
	}
	for index := 1; index < len(snapshot.Heights); index++ {
		previous := snapshot.Heights[index-1]
		current := snapshot.Heights[index]
		if previous.X > current.X || (previous.X == current.X && previous.Z >= current.Z) {
			t.Fatalf("高度样本未按 (X,Z) 严格升序：%+v 后跟 %+v", previous, current)
		}
	}
	if len(snapshot.ChunkRevisions) == 0 || len(snapshot.ChunkRevisions) > pathfind.MaxPlanChunkRevisions {
		t.Fatalf("区块 revision 数=%d", len(snapshot.ChunkRevisions))
	}
}

// taskTickEvent 记录一条任务事件与其被发布的权威 tick，供按 tick 断言
// 「三连失败预算属于单个任务」的可观察节奏（两次 20 tick 冷却窗口）。
type taskTickEvent struct {
	tick  uint64
	event network.ChatEvent
}

// TestCompanionManagerPathFailureBudgetResetsPerTask 验证路径三连失败预算
// 属于单个任务而非槽位：前一条任务以三次寻路失败终结后，同伙伴的下一条
// 不可达任务必须重新经历完整的「三次失败 + 两次 20 tick 冷却」，而不是在
// 第一次寻路失败时立即继承前任务的满额计数而终结（pathfinding spec：
// 「同一任务内连续三次无法得到可用路径」）。
//
// 判定依据是权威 tick 节奏而非墙钟：TaskStarted tick（首次寻路派发）到
// 第三次失败至少要跨过两个完整冷却窗口（2 × PathReplanCooldownTicks）；
// 若预算跨任务泄漏，任务会在第一次失败（Started 后数 tick 内）立即终结。
func TestCompanionManagerPathFailureBudgetResetsPerTask(t *testing.T) {
	definitions := []companion.Definition{{ID: chatTestCompanionID(5), Name: "阿木"}}
	host, client, body := companionManagerHostReady(t, definitions, nil)
	// 两条指令共用同一个不可达目标（寻路窗口外）：各自都要走满三次失败。
	model := newFakeCompanionModel(t,
		[3]int32{int32(body.Position[0]) + 1000, 1, int32(body.Position[2])})
	host.world.companionManager.replacePlannerForTest(t, model)

	// stepUntilTaskEvent 推进世界并按 (tick, event) 收集事件，直到 predicate
	// 命中；上限是 longWaitDeadline 的墙钟（耗尽时返回已收集事件，由调用方
	// 断言失败）。固定 tick 上限在 non-race 快进下会因异步规划 worker 尚未
	// 落地而过早耗尽，墙钟限界的理由见 stepUntilCompanionEvents 的注释。
	stepUntilTaskEvent := func(predicate func(event network.ChatEvent) bool) []taskTickEvent {
		collected := make([]taskTickEvent, 0, 8)
		deadline := time.Now().Add(longWaitDeadline)
		for time.Now().Before(deadline) {
			result := host.world.StepForTest()
			for _, event := range companionChatEvents(receiveCompanionChatTick(t, client, result.Tick)) {
				collected = append(collected, taskTickEvent{tick: result.Tick, event: event})
				if predicate(event) {
					return collected
				}
			}
			time.Sleep(time.Millisecond)
		}
		return collected
	}

	// 任务 A：从零预算出发，按既有语义三次失败后以 PathUnreachable 终结。
	sendIntegration(t, client, network.ChatCommand{Text: "@阿木 第一次去远方"})
	first := stepUntilTaskEvent(func(event network.ChatEvent) bool {
		return event.Kind == network.ChatEventTaskFailed && event.Command == "第一次去远方"
	})
	firstFailed := eventsWithKind(
		networkEventsOf(first), network.ChatEventTaskFailed)
	if len(firstFailed) != 1 ||
		network.TaskFailReason(firstFailed[0].RejectReason) != network.TaskFailPathUnreachable {
		t.Fatalf("任务 A 失败事件=%v（全部事件=%v），想要 1 次 PathUnreachable",
			chatEventKinds(networkEventsOf(first)), chatEventKinds(networkEventsOf(first)))
	}

	// 任务 B：前任务已把槽位计数耗到 3，若预算按任务重置，B 仍要完整走
	// 三次失败；若泄漏，B 会在第一次寻路失败（Started 后数 tick 内）终结。
	sendIntegration(t, client, network.ChatCommand{Text: "@阿木 第二次去远方"})
	second := stepUntilTaskEvent(func(event network.ChatEvent) bool {
		return event.Kind == network.ChatEventTaskFailed && event.Command == "第二次去远方"
	})
	var startedTick, failedTick uint64
	failedCount := 0
	for _, entry := range second {
		if entry.event.Command != "第二次去远方" {
			continue
		}
		switch entry.event.Kind {
		case network.ChatEventTaskStarted:
			startedTick = entry.tick
		case network.ChatEventTaskFailed:
			failedTick = entry.tick
			failedCount++
			if network.TaskFailReason(entry.event.RejectReason) != network.TaskFailPathUnreachable {
				t.Fatalf("任务 B 失败原因=%d，想要 PathUnreachable", entry.event.RejectReason)
			}
		}
	}
	if failedCount != 1 {
		t.Fatalf("任务 B TaskFailed=%d（事件=%v），想要恰好 1 次",
			failedCount, chatEventKinds(networkEventsOf(second)))
	}
	if startedTick == 0 {
		t.Fatalf("任务 B 缺少 TaskStarted：%v", chatEventKinds(networkEventsOf(second)))
	}
	// 三次寻路失败需要两次固定冷却（2 × 20 tick）：Started 到 Failed 至少
	// 跨 2*PathReplanCooldownTicks 个权威 tick。泄漏时该间距只有数 tick。
	if span := failedTick - startedTick; span < 2*pathfind.PathReplanCooldownTicks {
		t.Fatalf("任务 B 从 TaskStarted(tick %d) 到 TaskFailed(tick %d) 仅 %d tick，"+
			"三连失败预算被前任务泄漏（需要 ≥ %d tick 的两次冷却窗口）",
			startedTick, failedTick, span, 2*pathfind.PathReplanCooldownTicks)
	}
}

// networkEventsOf 把带 tick 的事件记录展开为纯事件切片，复用既有过滤助手。
func networkEventsOf(entries []taskTickEvent) []network.ChatEvent {
	events := make([]network.ChatEvent, len(entries))
	for index, entry := range entries {
		events[index] = entry.event
	}
	return events
}

// TestCompanionManagerSnapshotFailureTerminatesTaskAndAdvancesFIFO 验证规划
// 快照构造失败（服务端缺陷的确定性替身：发令者视线命中 Y 越界）时任务真实
// 终结且 FIFO 继续推进，绝不原地死循环。缺陷任务必须在进入 Planning 后以
// TaskFailed(PlannerUnavailable) 终结、绝不抵达模型；队首之后的指令照常
// 被提升、规划与启动。
func TestCompanionManagerSnapshotFailureTerminatesTaskAndAdvancesFIFO(t *testing.T) {
	definitions := []companion.Definition{{ID: chatTestCompanionID(6), Name: "阿木"}}
	host, client, body := companionManagerHostReady(t, definitions, nil)
	baseX, baseZ := int32(body.Position[0]), int32(body.Position[2])
	model := newFakeCompanionModel(t, [3]int32{baseX + 2, 1, baseZ})
	host.world.companionManager.replacePlannerForTest(t, model)

	identity := integrationIdentity(0x78, "发令者")
	// 在 stepMu 下直接构造「队首任务已提升但快照必然构造失败」的现场：
	// lookHit.Y=core.MaxY 落在世界竖直边界之外，PlanSnapshot.Validate 必然
	// 拒绝——它只污染规划输入，不影响 ChatEvent 的事实组装（事件不携带
	// 视线命中），因此失败路径仍能产出可发布的 TaskFailed 事件。
	host.world.stepMu.Lock()
	slot := host.world.companionManager.slots[definitions[0].ID]
	if !slot.queue.Enqueue(companion.TaskCommand("坏快照")) ||
		!slot.queue.Enqueue(companion.TaskCommand("下一条")) ||
		!slot.queue.BeginHead() {
		host.world.stepMu.Unlock()
		t.Fatal("构造缺陷队列状态失败")
	}
	slot.currentCommand = companion.TaskCommand("坏快照")
	slot.currentIssuer = companionTaskIssuer{
		playerID:   identity.PlayerID,
		name:       "发令者",
		position:   [3]float32{0, 1, 0},
		lookHit:    core.BlockPos{Y: core.MaxY},
		hasLookHit: true,
	}
	// 「下一条」被提升时的发令者配对（enqueueCommand 的等价手工路径）。
	slot.issuers = append(slot.issuers, companionTaskIssuer{
		playerID: identity.PlayerID,
		name:     "发令者",
		position: [3]float32{0, 1, 0},
	})
	host.world.stepMu.Unlock()

	// 推进直到缺陷任务终结且后继指令真正启动；死循环实现会在墙钟上限
	// 耗尽时因缺少 TaskFailed 而失败。上限按墙钟而非固定 tick 数（理由见
	// stepUntilCompanionEvents 注释）：「下一条」的 TaskStarted 依赖一轮完整
	// 异步规划落地，non-race 快进下需要数百 tick。
	collected := make([]network.ChatEvent, 0, 8)
	deadline := time.Now().Add(longWaitDeadline)
	for time.Now().Before(deadline) {
		result := host.world.StepForTest()
		collected = append(collected,
			companionChatEvents(receiveCompanionChatTick(t, client, result.Tick))...)
		sawFailed, sawNextStarted := false, false
		for _, event := range collected {
			if event.Kind == network.ChatEventTaskFailed && event.Command == "坏快照" {
				sawFailed = true
			}
			if event.Kind == network.ChatEventTaskStarted && event.Command == "下一条" {
				sawNextStarted = true
			}
		}
		if sawFailed && sawNextStarted {
			break
		}
		time.Sleep(time.Millisecond)
	}

	failed := make([]network.ChatEvent, 0, 1)
	nextStarted := false
	for _, event := range collected {
		switch {
		case event.Kind == network.ChatEventTaskFailed && event.Command == "坏快照":
			failed = append(failed, event)
		case event.Kind == network.ChatEventTaskStarted && event.Command == "下一条":
			nextStarted = true
		}
	}
	if len(failed) != 1 ||
		network.TaskFailReason(failed[0].RejectReason) != network.TaskFailPlannerUnavailable {
		t.Fatalf("坏快照 TaskFailed=%d（事件=%v），想要恰好 1 次 PlannerUnavailable"+
			"——快照失败必须真实终结任务", len(failed), chatEventKinds(collected))
	}
	if !nextStarted {
		t.Fatalf("缺陷任务终结后 FIFO 未推进：「下一条」始终未 TaskStarted（事件=%v）",
			chatEventKinds(collected))
	}
	// 缺陷任务绝不抵达模型：唯一的模型请求属于「下一条」的规划。
	if requests, _, _, _ := model.snapshotCounts(); requests != 1 {
		t.Fatalf("模型请求数=%d，想要 1（坏快照必须失败于快照构造而非模型调用）", requests)
	}
	// 终结后再观察数 tick：缺陷任务不得重复失败（每 tick 重试的迹象）。
	for range 3 {
		result := host.world.StepForTest()
		collected = append(collected,
			companionChatEvents(receiveCompanionChatTick(t, client, result.Tick))...)
	}
	repeated := 0
	for _, event := range collected {
		if event.Kind == network.ChatEventTaskFailed && event.Command == "坏快照" {
			repeated++
		}
	}
	if repeated != 1 {
		t.Fatalf("坏快照 TaskFailed=%d 次，快照失败路径仍在每 tick 重试", repeated)
	}
}

// TestCompanionManagerStopSameTickOrdering 验证同 tick 多条停止按聊天接收
// 顺序处理：第一条作用于 Running 的持续跟随任务并广播 TaskStopped，第二条
// 面对已被清空的当前槽（任务编排尚未提升原队首）按当前状态判定为 NotFollowing
// 单播；队列只推进一次，每条客户端事件流的 EventID 严格递增。
func TestCompanionManagerStopSameTickOrdering(t *testing.T) {
	definitions := []companion.Definition{{ID: chatTestCompanionID(1), Name: "阿木"}}
	model := newFakeCompanionModel(t, [3]int32{0, 1, 0})
	model.holdRequests()
	host := newCompanionManagerHost(t, definitions, model, nil)
	sender := openCompanionChatClient(t, host, "memory", integrationIdentity(0x8d, "停止者"))
	observer := openCompanionChatClient(t, host, "memory", integrationIdentity(0x8e, "旁观者"))
	clients := []network.ClientEndpoint{sender, observer}
	waitForCompanionChatWorld(t, host, clients, 1)

	issuerIdentity := integrationIdentity(0x8f, "原发令者")
	injectRunningCompanionTask(t, host, definitions[0].ID, stopTestIssuer(issuerIdentity),
		"跟着我", []companion.PlanStep{{Kind: companion.PlanStepFollow, PlayerID: issuerIdentity.PlayerID}},
		"下一条")

	// 同一会话顺序发送两条停止：单一读取 goroutine 保证 ingress 顺序与发送
	// 顺序一致，第一条生效、第二条按清空后的槽位判定拒绝。
	sendIntegration(t, sender, network.ChatCommand{Text: "@阿木 停止"})
	sendIntegration(t, sender, network.ChatCommand{Text: "@阿木 停止"})
	waitForIncomingChatDepth(t, host.world, 2)
	result := host.world.StepForTest()
	senderEvents := companionChatEvents(receiveCompanionChatTick(t, sender, result.Tick))
	observerEvents := companionChatEvents(receiveCompanionChatTick(t, observer, result.Tick))

	// 发令者：先收到第二条停止的 NotFollowing 单播（聊天投递先于任务事件
	// 发布，与 Accepted→TaskStarted 的既有顺序一致），随后是第一条停止的
	// TaskStopped 广播；旁观者只看到广播。
	if len(senderEvents) != 2 {
		t.Fatalf("发令者事件=%v，想要 NotFollowing+TaskStopped", chatEventKinds(senderEvents))
	}
	identity := integrationIdentity(0x8d, "停止者")
	if senderEvents[0].Kind != network.ChatEventRejected ||
		senderEvents[0].RejectReason != network.ChatRejectNotFollowing ||
		senderEvents[0].Command != "停止" ||
		senderEvents[0].PlayerID != identity.PlayerID ||
		senderEvents[0].EventID != 1 {
		t.Fatalf("第二条停止事件=%+v，想要 NotFollowing(EventID=1)", senderEvents[0])
	}
	if err := senderEvents[0].Validate(); err != nil {
		t.Fatalf("NotFollowing Validate: %v", err)
	}
	stopped := eventsWithKind(senderEvents, network.ChatEventTaskStopped)
	if len(stopped) != 1 || stopped[0].Command != "跟着我" ||
		stopped[0].PlayerID != issuerIdentity.PlayerID ||
		stopped[0].RejectReason != network.ChatRejectNone {
		t.Fatalf("TaskStopped=%+v，想要唯一广播且携带被停任务事实", stopped)
	}
	if err := stopped[0].Validate(); err != nil {
		t.Fatalf("TaskStopped Validate: %v", err)
	}
	if len(observerEvents) != 1 || observerEvents[0].Kind != network.ChatEventTaskStopped {
		t.Fatalf("旁观者事件=%v，想要唯一 TaskStopped 广播", chatEventKinds(observerEvents))
	}
	if !reflect.DeepEqual(observerEvents[0], stopped[0]) {
		t.Fatalf("广播不一致：observer=%+v sender=%+v", observerEvents[0], stopped[0])
	}
	assertStrictlyIncreasingEventIDs(t, senderEvents)

	// 队列只推进一次：原队首被提升为当前任务并停留在在途规划（挂起模型），
	// 不存在双重停止或双重提升。
	host.world.stepMu.Lock()
	snapshot := host.world.companionManager.slots[definitions[0].ID].queue.Snapshot()
	host.world.stepMu.Unlock()
	if !snapshot.HasCurrent || snapshot.Current.Command != companion.TaskCommand("下一条") ||
		len(snapshot.Pending) != 0 {
		t.Fatalf("同 tick 双停止后队列推进异常：current=%+v has=%v pending=%v",
			snapshot.Current, snapshot.HasCurrent, snapshot.Pending)
	}
}

// TestCompanionManagerPathBlockTableMatchesCollisionOracle 把伙伴寻路阻挡表逐
// 编号对齐到 collision oracle（physics.BlockCollisionBoxes）：零碰撞体的编号
// 必须可通过，有碰撞体的编号必须阻挡。
//
// 循环上界必须覆盖「全部已注册编号」，因此只能写成独占哨兵 core.BlockIDMax。
// 历史上这里写死过具体末项：先是 MossyCobblestoneID，流体追加后退化成子集；
// 改成 WaterLevel7ID 后，农业编号追加又会再退化一次。用哨兵表达之后，新增
// 方块编号自动纳入本门禁，不再需要人手推进上界。
func TestCompanionManagerPathBlockTableMatchesCollisionOracle(t *testing.T) {
	table := pathfind.NewPathBlockTable(productionCompanionPassableBlocks())
	if !table.PassableForTest(core.AirID) {
		t.Fatal("空气必须可通过")
	}
	const shortGrassID core.BlockID = 84
	if !table.PassableForTest(shortGrassID) {
		t.Fatal("短草必须按零碰撞 oracle 对伙伴寻路可通过")
	}
	if boxes := physics.BlockCollisionBoxes(shortGrassID, true); boxes.Count != 0 {
		t.Fatalf("短草碰撞体数=%d，想要 0", boxes.Count)
	}
	for id := core.BlockID(1); id < core.BlockIDMax; id++ {
		if core.IsFluid(id) {
			// 流体的显式豁免：流体在 oracle 下是零碰撞体（实体可自由穿行），
			// 按上面的对齐规则本应可通过；但伙伴寻路刻意继续把它当阻挡。
			// 为什么：伙伴没有任何浮力、屏息或溺水处理，一旦把水面当平地纳入
			// 路径，它会直接走进水里沉底并卡死。宁可让伙伴绕开水域，也不产生
			// 无法自救的路径。
			//
			// 退出条件（**两条都成立才可以删本分支**，缺一条就是制造故障）：
			//
			//  1. 伙伴走与玩家同一套水中积分——即 sim.advanceActiveCompanions
			//     喂给 physics.Step 的 physics.Input 里，BodyInFluid 由
			//     physics.SubmersionFlags 真实算出，而不是像现在这样恒为零值；
			//  2. 伙伴有自己的氧气与溺水结算——即 sim 的 advanceOxygen 那条
			//     结算对伙伴也成立，伙伴在水里待久了会受伤而不是无限静止。
			//
			// 这两条**都还不成立**：spec fluid-survival 的主语全是"玩家"，
			// 浸没物理与氧气/溺水都只接了玩家一侧。原注释写的是笼统的"后续变更
			// 交付浸没物理后删除"，而浸没物理本身（physics.SubmersionFlags、
			// 水中积分、玩家溺水）已经交付，那句话字面已经成立——照它删掉本
			// 分支，A* 会规划出穿水路径而伙伴仍按空气 + 重力积分，正好落进本
			// 豁免当初要防的"走进水里沉底并卡死"。给伙伴接水中物理属 M5 系列
			// 范围，不在变更 fluid-presentation-survival 内。
			if table.PassableForTest(id) {
				t.Fatalf("流体方块 %d 在伙伴接入水中积分与溺水结算之前必须对伙伴寻路保持不可通过", id)
			}
			continue
		}
		boxes := physics.BlockCollisionBoxes(id, true)
		if table.PassableForTest(id) != (boxes.Count == 0) {
			t.Fatalf("方块 %d 可通过=%v，而 collision oracle 的碰撞体数=%d，两者必须一致",
				id, table.PassableForTest(id), boxes.Count)
		}
	}
}

func countKind(events []network.ChatEvent, kind network.ChatEventKind) int {
	count := 0
	for _, event := range events {
		if event.Kind == kind {
			count++
		}
	}
	return count
}

func eventsWithKind(events []network.ChatEvent, kind network.ChatEventKind) []network.ChatEvent {
	matched := make([]network.ChatEvent, 0, len(events))
	for _, event := range events {
		if event.Kind == kind {
			matched = append(matched, event)
		}
	}
	return matched
}

func blockPosAfterForSort(pos, previous core.BlockPos) bool {
	if pos.X != previous.X {
		return pos.X > previous.X
	}
	if pos.Y != previous.Y {
		return pos.Y > previous.Y
	}
	return pos.Z > previous.Z
}

// TestCompanionManagerIssuerMismatchSkipsBeforeBeginHead 锁定 dispatchPlanning
// 发令者失配防御的位次：检查必须先于 BeginHead 触发，缺陷态下队列不得占用
// 当前槽位。「pending 非空而 issuers 为空」正常不可达（Enqueue/restore 保证
// 一一配对），这里直接注入失配态做白盒等价锁——若失配检查被移回 BeginHead
// 之后，队列将提升队首并残留未定义的 currentIssuer 次生态，本测试随之失败。
func TestCompanionManagerIssuerMismatchSkipsBeforeBeginHead(t *testing.T) {
	definitions := []companion.Definition{{ID: chatTestCompanionID(9), Name: "阿玖"}}
	host := newCompanionManagerHost(t, definitions, nil, nil)
	manager := host.world.companionManager
	slot := manager.slots[definitions[0].ID]
	// 注入失配缺陷态：pending 入队但不追加配对的 issuers 条目。
	if !slot.queue.Enqueue(companion.TaskCommand("白盒失配指令")) {
		t.Fatalf("注入 pending 指令失败")
	}
	generation := slot.queue.Generation()
	// 不经 tick 编排直接调用：失配分支在任何模型/寻路派发之前返回，
	// 不会产生在途 worker 或网络请求。
	manager.dispatchPlanning()
	if _, hasCurrent := slot.queue.Current(); hasCurrent {
		t.Fatalf("失配防御必须先于 BeginHead：当前槽位被占用")
	}
	if slot.queue.Len() != 1 {
		t.Fatalf("pending=%d，失配防御不得消费队列", slot.queue.Len())
	}
	if slot.queue.Generation() != generation {
		t.Fatalf("generation=%d，失配防御不得推进世代（BeginHead 未执行）",
			slot.queue.Generation())
	}
}

// mismatchLogCapture 收集经全局 slog 输出的消息文本，供空闲态回归锁断言
// 「不产生失配日志」。只比对 message、不格式化属性，避免断言耦合日志排版。
type mismatchLogCapture struct {
	mu       sync.Mutex
	messages []string
}

func (capture *mismatchLogCapture) Enabled(context.Context, slog.Level) bool {
	return true
}

func (capture *mismatchLogCapture) Handle(_ context.Context, record slog.Record) error {
	capture.mu.Lock()
	capture.messages = append(capture.messages, record.Message)
	capture.mu.Unlock()
	return nil
}

func (capture *mismatchLogCapture) WithAttrs([]slog.Attr) slog.Handler { return capture }
func (capture *mismatchLogCapture) WithGroup(string) slog.Handler      { return capture }

// mismatchLogs 返回捕获到的「任务 FIFO 与发令者队列失配」条数。
func (capture *mismatchLogCapture) mismatchLogs() int {
	capture.mu.Lock()
	defer capture.mu.Unlock()
	count := 0
	for _, message := range capture.messages {
		if message == "任务 FIFO 与发令者队列失配" {
			count++
		}
	}
	return count
}

// TestCompanionManagerIdleSlotNoMismatchLog 锁定失配守卫的空闲态回归：空闲
// 槽位（无 current、pending 与 issuers 同空）是每伙伴的正常状态，步进若干
// tick 绝不产生「任务 FIFO 与发令者队列失配」日志。失配的准确条件是
// 「pending 非空而 issuers 为空」；若守卫退化成只查 issuers 为空（丢掉
// pending 非空前提），空闲态每 tick 误报、本测试以日志洪泛的形式变红。
func TestCompanionManagerIdleSlotNoMismatchLog(t *testing.T) {
	definitions := []companion.Definition{{ID: chatTestCompanionID(1), Name: "阿木"}}
	host := newCompanionManagerHost(t, definitions, nil, nil)
	capture := &mismatchLogCapture{}
	previous := slog.Default()
	slog.SetDefault(slog.New(capture))
	defer slog.SetDefault(previous)
	// 步进真实权威 tick：覆盖伙伴出生扫描在途与激活后的两种空闲形态；
	// 恢复 logger 的 defer 先于 t.Cleanup 的 Shutdown 执行，关服日志不受
	// 捕获影响。
	const idleTicks = 32
	for range idleTicks {
		host.world.StepForTest()
	}
	if count := capture.mismatchLogs(); count != 0 {
		t.Fatalf("空闲槽位 %d 个 tick 产生 %d 条失配日志，空闲态不属失配",
			idleTicks, count)
	}
}

// identityMismatchPlanner wraps the typed planner seam and corrupts the bridge
// identity echo of the first `remaining` successful plans for one companion,
// leaving the local generation and attempt intact.
type identityMismatchPlanner struct {
	inner     companionPlanner
	target    companion.ID
	mu        sync.Mutex
	remaining int
}

func (p *identityMismatchPlanner) Plan(
	ctx context.Context,
	request companionPlanningRequest,
) (companionPlanningOutcome, error) {
	outcome, err := p.inner.Plan(ctx, request)
	p.mu.Lock()
	defer p.mu.Unlock()
	if err == nil && request.CompanionID == p.target && p.remaining > 0 {
		p.remaining--
		outcome.SnapshotID = "cccccccc-cccc-4ccc-8ccc-cccccccccccc"
	}
	return outcome, err
}

// TestCompanionManagerIdentityMismatchFailsOnlyCurrentTaskAndAdvancesFIFO drives
// the real tick loop: one companion's plan comes back with a mismatched bridge
// identity while a second companion plans concurrently. The mismatched task must
// end as TaskFailed(PlannerUnavailable), the planning gate must be released so
// the next queued command dispatches under normal eligibility (in the same tick,
// since dispatch runs after outcomes apply), and the other companion must be
// untouched.
func TestCompanionManagerIdentityMismatchFailsOnlyCurrentTaskAndAdvancesFIFO(t *testing.T) {
	definitions := []companion.Definition{
		{ID: chatTestCompanionID(1), Name: "阿木"},
		{ID: chatTestCompanionID(2), Name: "小石"},
	}
	model := newFakeCompanionModel(t)
	host := newCompanionManagerHost(t, definitions, model, nil)
	client := openCompanionChatClient(t, host, "memory", integrationIdentity(0x75, "发令者"))
	waitForCompanionChatWorld(t, host, []network.ClientEndpoint{client}, len(definitions))
	body := currentCompanionBody(t, host, definitions[0].ID)
	model.mu.Lock()
	model.steps = [][3]int32{{int32(body.Position[0]) + 1, 1, int32(body.Position[2])}}
	model.mu.Unlock()
	planner := &identityMismatchPlanner{
		inner: fakeCompanionPlanner{model: model}, target: definitions[0].ID, remaining: 1,
	}
	host.world.stepMu.Lock()
	host.world.companionManager.planner = planner
	host.world.stepMu.Unlock()
	model.holdRequests()

	for _, text := range []string{"@阿木 第一", "@阿木 第二", "@小石 出发"} {
		sendIntegration(t, client, network.ChatCommand{Text: text})
	}
	waitForIncomingChatDepth(t, host.world, 3)
	result := host.world.StepForTest()
	events := companionChatEvents(receiveCompanionChatTick(t, client, result.Tick))
	// Both companions are planning at the same time before any result lands.
	waitForModelRequests(t, model, 2)
	model.releaseRequests()

	isEvent := func(event network.ChatEvent, id companion.ID, command string, kind network.ChatEventKind) bool {
		return event.CompanionID == id && event.Command == command && event.Kind == kind
	}
	seen := func(events []network.ChatEvent, id companion.ID, command string, kind network.ChatEventKind) bool {
		return slices.ContainsFunc(events, func(event network.ChatEvent) bool {
			return isEvent(event, id, command, kind)
		})
	}
	events = append(events, stepUntilCompanionEvents(t, host, []network.ClientEndpoint{client},
		func(events []network.ChatEvent) bool {
			return seen(events, definitions[0].ID, "第一", network.ChatEventTaskFailed) &&
				seen(events, definitions[0].ID, "第二", network.ChatEventTaskStarted) &&
				seen(events, definitions[1].ID, "出发", network.ChatEventTaskStarted)
		})...)

	host.world.stepMu.Lock()
	inFlight := host.world.companionManager.slots[definitions[0].ID].planningInFlight
	host.world.stepMu.Unlock()
	requests, _, _, _ := model.snapshotCounts()
	if !seen(events, definitions[0].ID, "第一", network.ChatEventTaskFailed) {
		t.Fatalf("identity-mismatched task never reached a terminal state (planningInFlight=%v requests=%d events=%v)",
			inFlight, requests, chatEventKinds(events))
	}
	if !seen(events, definitions[0].ID, "第二", network.ChatEventTaskStarted) {
		t.Fatalf("next FIFO command never dispatched (planningInFlight=%v requests=%d events=%v)",
			inFlight, requests, chatEventKinds(events))
	}
	if !seen(events, definitions[1].ID, "出发", network.ChatEventTaskStarted) {
		t.Fatalf("concurrent companion did not start its task: %v", chatEventKinds(events))
	}
	if requests != 3 {
		t.Fatalf("model requests=%d, want 3 (one per command, no retry of the failed task)", requests)
	}

	var failedIndex, secondStartedIndex int
	for index, event := range events {
		if event.CompanionID == definitions[1].ID && event.Kind == network.ChatEventTaskFailed {
			t.Fatalf("concurrent companion task failed: %+v", event)
		}
		if event.CompanionID == definitions[0].ID && event.Kind == network.ChatEventTaskFailed {
			if event.Command != "第一" {
				t.Fatalf("unexpected failure for %q: %+v", event.Command, event)
			}
			if event.RejectReason != network.ChatRejectReason(network.TaskFailPlannerUnavailable) {
				t.Fatalf("terminal reason=%v, want PlannerUnavailable", event.RejectReason)
			}
			if event.PlayerID != integrationIdentity(0x75, "发令者").PlayerID || event.CompanionName != "阿木" {
				t.Fatalf("terminal event identity incomplete: %+v", event)
			}
			if err := event.Validate(); err != nil {
				t.Fatalf("terminal event Validate: %v", err)
			}
			failedIndex = index
		}
		if isEvent(event, definitions[0].ID, "第二", network.ChatEventTaskStarted) {
			secondStartedIndex = index
		}
	}
	if secondStartedIndex <= failedIndex {
		t.Fatalf("next command started before the failed task terminated: %v", chatEventKinds(events))
	}
	assertStrictlyIncreasingEventIDs(t, events)
}
