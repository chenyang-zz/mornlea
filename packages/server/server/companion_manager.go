// 本文件实现 server 侧 Companion Manager：任务 FIFO 与状态机的 tick 边界
// 编排。
//
// 并发模型（权威 tick 是唯一写者）：
//   - slots（队列/世代/路径/在途标记）只在持有 stepMu 的 tick 路径被读写：
//     step() 的 advanceCompanionTasks、drainIncomingChats 的入队、Shutdown
//     的冻结段；
//   - worker goroutine 只持有不可变值（PlanSnapshot、PathGrid），经有界
//     channel 回送结果；channel 与 semaphore 是它们触碰的全部共享状态，
//     模型槽名额在模型调用返回后、结果发送前显式释放（`plannerWorker` 与
//     `dialogueWorker` 的函数内注释有时序论证）；
//   - 结果只在 tick 边界非阻塞接收，世代或状态不符即丢弃。
//
// channel 容量论证：plannerResults 与 pathResults 容量 4——在途上限由
// “每伙伴 ≤1 规划 + 每伙伴 ≤1 寻路、伙伴数 ≤ companion.MaxActive=4”封顶，
// 满容量时 worker 经 ctx.Done 退出，绝不阻塞；结果每 tick 全量排空，容量
// 恰好覆盖峰值。dialogueResults 同容量 4（每伙伴 ≤1 台词，见
// companion_dialogue.go）。
package server

import (
	"bytes"
	"context"
	"errors"
	"log/slog"
	"math"
	"slices"
	"sync"
	"time"

	"github.com/go-gl/mathgl/mgl32"

	"github.com/channing771/mornlea/packages/server/server/persistence"
	"github.com/channing771/mornlea/packages/server/sim/contract"
	"github.com/channing771/mornlea/packages/server/sim/runtime"
	"github.com/channing771/mornlea/packages/server/storage"
	"github.com/channing771/mornlea/packages/shared/companion"
	"github.com/channing771/mornlea/packages/shared/core"
	"github.com/channing771/mornlea/packages/shared/network"
	"github.com/channing771/mornlea/packages/shared/pathfind"
	"github.com/channing771/mornlea/packages/shared/physics"
)

// companionPlanner 是规划器依赖面：生产实现只接受冻结快照与任务身份，
// 测试通过显式 seam 注入受控实现。
type companionPlanner interface {
	Plan(ctx context.Context, request companionPlanningRequest) (companionPlanningOutcome, error)
}

// companionTaskIssuer 是入队时刻冻结的发令者事实。指令的规划输入不随发令者
// 后续移动漂移；身份字段供任务事件回溯“谁下了这条指令”。
type companionTaskIssuer struct {
	playerID   core.PlayerID
	name       string
	restored   bool
	position   [3]float32
	yaw        float32
	pitch      float32
	lookHit    core.BlockPos
	hasLookHit bool
}

// companionTaskSlot 是一个伙伴的全部任务编排状态。只有权威 tick 写。
type companionTaskSlot struct {
	definition companion.Definition
	queue      companion.TaskQueue
	// issuers 与 queue.pending 一一配对：入队时追加，BeginHead 时消费，
	// 使事件能回溯每条指令的发令者。
	issuers               []companionTaskIssuer
	currentIssuer         companionTaskIssuer
	currentCommand        companion.TaskCommand
	idleDialogueAtTick    uint64
	hasIdleDialogueAtTick bool

	// planningInFlight 表示该伙伴有一个规划请求在途；在途期间绝不发起第二个。
	planningInFlight bool
	// planningAttempt 是 tick 为规划请求分配的本地单调身份；只有完整匹配
	// 当前 attempt 的 worker 结果才有权释放 gate。
	planningAttempt uint64

	// dialogueInFlight 表示该伙伴有一个台词请求在途；在途期间新台词节点
	// 直接跳过（不取消、不替换在途请求），对齐 planningInFlight 的每伙伴
	// 单在途纪律。标记只在 tick 边界（requestDialogue 置位、
	// applyDialogueOutcome 清除）读写。
	dialogueInFlight bool
	// dialogueAttempt 是 tick 为台词请求分配的本地单调身份；过时结果不能
	// 清除后来请求持有的每伙伴 gate。
	dialogueAttempt uint64
	// dialogueReservation 保存首次权威重验已接受的终态 proposal。它不再
	// 依赖后续任务 generation，只由 operation/epoch 的 commit 或 reconcile
	// 确认解除。
	dialogueReservation *companionDialogueReservation
	// memoryReady 只在当前 lease 已按 v5 current lifecycle reconcile 且无冲突时
	// 为真；冲突仅暂停本伙伴 Dialogue，不影响任务、Planner 与 FIFO。
	memoryReady bool

	// 以下三个字段是台词触发节点的任务域状态（D6）。全部属于「当前任务」
	// 而非槽位：dispatchPlanning 的 BeginHead 分支与 restoreQueue 都按任务
	// 边界重置/重导出；预算计数刻意不持久化（design.md「触发节点确定性
	// 算法」的裁决——限流是运行期资源约束，不是事实平面状态）。
	//
	// progressSteps 是本任务预计算的进展台词节点步骤索引集合：进入 Running
	// 时由 companion.SelectProgressSteps(len(plan.Steps)) 一次性导出（计划
	// 校验成功后计划不再变化，集合确定性稳定）；follow 尾步任务恒为空集
	//（持续跟随没有步骤完成事实，只有首次到达节点）。
	progressSteps []int
	// followArrivalSpoken 表示 follow 任务的「首次到达跟随距离」节点已经
	// 消费（发起或被跳过均置位——跳过即放弃，绝不补发）。目标此后反复进出
	// 跟随距离不产生新的台词请求。
	followArrivalSpoken bool
	// dialogueRequests 是本任务本进程已发起的台词请求数，上限
	// companion.MaxDialogueRequestsPerTask；结构上 1+≤6+1 恒不越界，计数是
	// 对未来接线缺陷的防御性封顶。
	dialogueRequests int

	// 路径执行状态（仅 Running 有效）。三连失败预算属于单个任务而非槽位：
	// dispatchPlanning 消费新队首与 restoreQueue 成功恢复时都会把 policy
	// 归零，前一个任务的失败计数绝不削减下一个任务的预算。
	policy       pathfind.PathPolicy
	path         *pathfind.PathResult
	waypoint     int
	pathInFlight bool
	replanAtTick uint64
	hasReplanAt  bool

	// followGoal 是当前 follow 路径（或在途寻路请求）计算时的目标站立格。
	// 它只在「当前步骤是 follow 且 slot.path 非空」时被漂移判定消费；路径
	// 在别处被清空（终态/超时/停止）后残留的旧值不参与任何判定，下一次
	// follow 寻路派发会整体覆写。目标玩家持续移动时 advanceRunners 以它
	// 判定「终点漂移超出重算阈值」并丢弃既有路径，令 follow 复用 go_to 的
	// 寻路/冷却/三连失败语义而不必每 tick 重算。
	followGoal    pathfind.PathCell
	hasFollowGoal bool

	// 交互执行状态（仅 mine/place 步骤的 Running 任务有效）。interactStepIndex
	// 记录交互所属的步骤索引，步骤推进或任务更替后与 queue.Current 的
	// StepIndex 不再相等，交互记忆随之作废——跨步骤/跨任务边界因此自愈，
	// 终态路径无需逐一清理；任务边界（BeginHead/restore）仍经 resetInteraction
	// 显式归零，避免「新任务恰好也从 StepIndex 0 开始」时误继承就绪标记。
	interactStepIndex int
	// interactionReady 表示走近段已结束（路径走尽），进入按住采掘/提交放置
	// 的交互段；dispatchPathRequests 以它停止交互期间的寻路派发。
	interactionReady bool
	// miningHeld 表示 sim 侧采掘意图正在按住（已提交 MineHold 且未 Release）。
	// sim 的按住语义跨 tick 保持，步骤离开采掘时必须显式释放（见
	// releaseFinishedMining）。
	miningHeld bool
	// mineProgress/mineRequired 是最近一次活跃观察到的 sim 采掘进度与计时
	// 规则：同一目标/方块/工具的进度单调递增，回退或规则变化是 sim「目标
	// 替换失效」语义的唯一可观察征兆。
	mineProgress uint16
	mineRequired uint16
}

// plannerOutcome 是一次规划请求的结果，携带任务身份供过期判定。
type plannerOutcome struct {
	id         companion.ID
	generation uint64
	attempt    uint64
	// snapshotDigest 由 worker 从不可变冻结快照独立计算，供 tick 核对 bridge
	// 实际注册并关联的 digest，不能由 Agent 响应自行声明。
	snapshotDigest string
	result         companionPlanningOutcome
	snapshot       companion.PlanSnapshot
	err            error
}

// pathOutcome 是一次寻路的结果，同样携带任务身份。
type pathOutcome struct {
	id         companion.ID
	generation uint64
	result     pathfind.PathResult
	err        error
}

// taskEventFact 是一次状态机迁移产出的待发布事件事实：编排层补齐身份后
// 由 Server 转成 ChatEvent 广播。speech 非空表示这是一条台词事实（D6）：
// event/command 保持零值，taskEventDeliveries 按 CompanionSpeech 组装广播
// （伙伴台词是 ChatEvent 中唯一携带模型生成文本的 kind）。
type taskEventFact struct {
	issuer     companionTaskIssuer
	definition companion.Definition
	command    companion.TaskCommand
	event      companion.TaskEvent
	speech     string
}

// companionManager 编排全部伙伴的任务执行。零值不可用，经 newCompanionManager
// 构造；关闭顺序见 beginShutdown/close。
type companionManager struct {
	engine         *runtime.Engine
	planner        companionPlanner
	companions     *persistence.Companions
	timeoutMinutes int
	table          pathfind.PathBlockTable

	// onlinePlayers 返回 tick 边界的在线玩家事实（稳定 ID + 权威位置，已按
	// ID 升序去重归一），由 Server 在构造后注入——会话注册表归 Server 所有，
	// manager 只消费这一权威源。规划快照的 OnlinePlayers 填充与 follow 目标
	// 的在线性/位置解析共用它；nil 是防御缺省（视同无人在线）。调用方必须
	// 持有 stepMu（与 manager 其余状态同一单写者边界）。
	onlinePlayers func() []companion.PlanPlayer
	// playerDimension 返回指定玩家的当前权威维度，供 follow 的跨维保持查询：
	// 目标在另一维度时跟随原地待命而不是跨维寻路（伙伴不跟随传送）。同样由
	// Server 注入、同一调用边界；nil 是防御缺省（视同维度未知、不保持）。
	playerDimension func(core.PlayerID) (core.DimensionID, bool)

	slots      map[companion.ID]*companionTaskSlot
	orderedIDs []companion.ID
	bodies     map[companion.ID]companion.Body
	// mining 缓存每个已激活伙伴在上一次权威 TickResult 中的采掘进度
	//（observeTickResult 回填），与 bodies 同属「上一 tick 末」的一致观察
	// 截面，mine 步骤执行器的完成与失败判定只读这一缓存。
	mining map[companion.ID]contract.MiningUpdate

	semaphore      chan struct{}
	plannerResults chan plannerOutcome
	pathResults    chan pathOutcome
	// dialogueResults 是台词 worker 的结果回送通道，容量 MaxActive=4：
	// 在途台词 ≤ 每伙伴 1 × 伙伴数 ≤ MaxActive，结果每 tick 全量排空，
	// 容量恰好覆盖峰值；关服 cancel 后 worker 经 ctx.Done 放弃结果退出。
	dialogueResults chan dialogueOutcome
	// memoryCommitResults 把已接受 reservation 的 commit 结果送回权威 tick。
	// 容量按每伙伴最多一个 reservation 封顶，网络 worker 不触碰镜像或广播。
	memoryCommitResults      chan memoryCommitOutcome
	memoryReconcileResults   chan memoryReconcileOutcome
	memoryReconcileFence     uint64
	memoryReconcileTarget    uint64
	memoryReconcileInFlight  bool
	memoryReconcileRequested bool
	memoryReconcileRequestID map[companion.ID]struct{}
	memoryReconcilePending   map[companion.ID]struct{}
	memoryReconcileRetryWait uint8
	memoryReconcileAttempts  uint8
	// memoryReconcileArmAfterDrain 只属于当前关服 attempt。新 attempt 接手
	// 上一轮尚未应用的 reconcile 时，在消费该唯一旧结果后补一次
	// re-arm；本轮新派发的失败不会再次设置它，避免同轮无界自旋。
	memoryReconcileArmAfterDrain bool
	// dialogue 是台词模型依赖面（D5 机制；触发节点接线属 D6）。nil 不会出现
	// 于生产构造（server.go 与 Planner 同源构造），防御缺省下 requestDialogue
	// 不应被调用——D6 接线前没有任何生产调用方。
	dialogue companionDialogue
	// dialogueEffects 是有效台词结果进入 applyDialogueEffect 的次数。D6
	// 交付后该方法的可观察真身已是 CompanionSpeech 广播事实与摘要写入，
	// 生产路径只递增不读取；计数存活至今的原因是 companion_dialogue_test
	// 的 dialogueEffectCount 仍以它为断言输入（stepMu 下与 dialogueInFlight
	// 一起判定台词结果落地或被跳过），事件级断言并未完全接管。只在 tick
	// 边界（持有 stepMu）读写。
	dialogueEffects int

	ctx          context.Context
	cancel       context.CancelFunc
	memoryCtx    context.Context
	cancelMemory context.CancelFunc
	waitGroup    sync.WaitGroup

	// events 是本 tick 累积的事件事实，takeEventFacts 排空后归 Server 发布。
	events []taskEventFact
}

// newCompanionManager 构造 Companion Manager。config 必须已含校验过的
// Agent 任务超时与伙伴定义；外部调用依赖由 Host 显式装配。
func newCompanionManager(
	engine *runtime.Engine,
	config Config,
	planner companionPlanner,
	dialogue companionDialogue,
	companions *persistence.Companions,
) *companionManager {
	ctx, cancel := context.WithCancel(context.Background())
	memoryCtx, cancelMemory := context.WithCancel(context.Background())
	manager := &companionManager{
		engine:                   engine,
		planner:                  planner,
		dialogue:                 dialogue,
		companions:               companions,
		timeoutMinutes:           config.TaskTimeoutMinutes,
		table:                    pathfind.NewPathBlockTable(productionCompanionPassableBlocks()),
		slots:                    make(map[companion.ID]*companionTaskSlot, len(config.Companions)),
		orderedIDs:               make([]companion.ID, 0, len(config.Companions)),
		bodies:                   make(map[companion.ID]companion.Body, companion.MaxActive),
		mining:                   make(map[companion.ID]contract.MiningUpdate, companion.MaxActive),
		semaphore:                make(chan struct{}, companion.MaxActive),
		plannerResults:           make(chan plannerOutcome, companion.MaxActive),
		pathResults:              make(chan pathOutcome, companion.MaxActive),
		dialogueResults:          make(chan dialogueOutcome, companion.MaxActive),
		memoryCommitResults:      make(chan memoryCommitOutcome, companion.MaxActive),
		memoryReconcileResults:   make(chan memoryReconcileOutcome, 1),
		memoryReconcileRequestID: make(map[companion.ID]struct{}, companion.MaxActive),
		memoryReconcilePending:   make(map[companion.ID]struct{}, companion.MaxActive),
		ctx:                      ctx,
		cancel:                   cancel,
		memoryCtx:                memoryCtx,
		cancelMemory:             cancelMemory,
	}
	for _, definition := range config.Companions {
		_, needsReconcile := dialogue.(companionMemoryReconciler)
		manager.slots[definition.ID] = &companionTaskSlot{
			definition: definition, memoryReady: !needsReconcile,
		}
		manager.orderedIDs = append(manager.orderedIDs, definition.ID)
	}
	// orderedIDs 按字节序排序：每 tick 的事件产生顺序因此确定，EventID 分配
	// 在同一世界状态下可重放。
	slices.SortFunc(manager.orderedIDs, func(a, b companion.ID) int {
		return bytes.Compare(a[:], b[:])
	})
	return manager
}

// enqueueCommand 在 Accepted 分支把指令入队。返回 false 表示 FIFO 已满
// （QueueFull 同步拒绝）。issuer 由调用方在入队 tick 冻结。
func (m *companionManager) enqueueCommand(
	definition companion.Definition,
	command companion.TaskCommand,
	issuer companionTaskIssuer,
) bool {
	slot := m.slots[definition.ID]
	if slot == nil {
		// companionsByName 与 slots 同源构造（同一份 config.Companions），
		// 寻址成功后槽位仍缺失只可能是构造缺陷，生产不可达。这里按接受
		// 处理但不入队，代价是调用方会广播 ChatEventAccepted——全体玩家
		// 看到任务被接受而它永不执行；换取的是不把配置缺陷伪装成队列满
		//（返回 false 会以 QueueFull 同步拒绝发令者，谎报一个并不存在的
		// 满员事实）。协议没有「服务端配置缺陷」的拒绝原因，两条路都
		// 无法如实表达，取舍偏向不产生虚假拒绝、由 Error 日志承担可诊
		// 断性。
		slog.Error("任务入队找不到伙伴槽位", "companion", definition.ID)
		return true
	}
	if !slot.queue.Enqueue(command) {
		return false
	}
	slot.issuers = append(slot.issuers, issuer)
	return true
}

// stopCompanion 是停止旁路在 tick 边界的唯一入口（drainIncomingChats 调用，
// 持有 stepMu）。当前任务可停（Running 且计划最后一步是 follow）时转入
// Stopped 终态并累积 TaskStopped 广播事实（携带被停任务的原始指令与发令者
// 身份，复用 applyQueueEvents 的既有组装）；返回 false 表示不可停（非跟随
// 或空闲），由聊天层以 NotFollowing 只回发令者。移动清理沿用既有终态语义：
// 丢弃在途路径与重算计划，runner 不再为终态任务提交任何移动输入，伙伴在
// 权威物理作用下自然停下；在途寻路结果由既有世代/状态双重判定拦截。原队首
// 的推进不在这里特判——终态清槽后本 tick 的 dispatchPlanning 按 FIFO 语义
// 立即开始原队首，pending 不清空也不重排。
func (m *companionManager) stopCompanion(definition companion.Definition) bool {
	slot := m.slots[definition.ID]
	if slot == nil {
		// 与 enqueueCommand 的防御一致：配置缺陷按不可停处理并保留可诊断
		// 日志，绝不伪装成停止成功。
		slog.Error("停止指令找不到伙伴槽位", "companion", definition.ID)
		return false
	}
	events := slot.queue.Stop()
	if len(events) == 0 {
		return false
	}
	slot.path = nil
	slot.hasReplanAt = false
	m.applyQueueEvents(slot, events)
	return true
}

// captureIssuer 在入队 tick 冻结发令者事实：位置朝向来自权威玩家状态，
// 视线命中方块用与交互一致的确定性射线（≤交互距离）。玩家尚未出生时保留
// 有界缺省（有限坐标），指令本身仍然合法。
func (m *companionManager) captureIssuer(
	playerID core.PlayerID,
	name string,
	session contract.SessionID,
	tickTunables runtime.TickTunables,
) companionTaskIssuer {
	issuer := companionTaskIssuer{
		playerID: playerID,
		name:     name,
		position: [3]float32{0, 1, 0},
	}
	player, ok := m.engine.Player(session)
	if !ok {
		return issuer
	}
	issuer.position = [3]float32(player.State.Position)
	issuer.yaw = player.Yaw
	issuer.pitch = player.Pitch
	issuer.lookHit, issuer.hasLookHit = m.issuerLookHit(player, tickTunables)
	return issuer
}

// issuerLookHit 用确定性 DDA 求发令者视线命中的第一个实心方块。射线只穿
// 发令者 3×3 兴趣内的已 ready 区块；未加载方块按未命中处理（快照只描述
// 确凿看见的世界）。
func (m *companionManager) issuerLookHit(
	player contract.PlayerUpdate,
	tickTunables runtime.TickTunables,
) (core.BlockPos, bool) {
	view := m.chunkViewAt(player.Dimension, [3]float32(player.State.Position))
	origin := player.State.Position.Add(
		mgl32.Vec3{0, tickTunables.Physics.EyeHeight, 0},
	)
	direction := runtime.LookDirection(player.Yaw, player.Pitch)
	hit, ok, err := core.RaycastBlocks(
		origin,
		direction,
		tickTunables.Simulation.InteractionReach,
		func(position core.BlockPos) (bool, error) {
			block, ready := view.blockAt(position.X, position.Y, position.Z)
			if !ready {
				return false, nil
			}
			return core.InteractionTarget(block), nil
		},
	)
	if err != nil || !ok {
		return core.BlockPos{}, false
	}
	return hit.Block, true
}

// advanceCompanionTasks 是 tick 边界的编排入口，在聊天 drain 之后、
// engine.Step 之前调用（伙伴 action 必须先入 inbox 才能被本 tick 消费）。
// 返回本 tick 产生的任务事件投递。
func (server *Server) advanceCompanionTasks(
	tickTunables runtime.TickTunables,
) []chatDelivery {
	manager := server.companionManager
	if manager == nil {
		return nil
	}
	manager.refreshBodies()
	manager.applyMemoryReconcileOutcomes()
	manager.applyPlannerOutcomes()
	manager.applyPathOutcomes()
	manager.applyDialogueOutcomes()
	manager.applyMemoryCommitOutcomes()
	manager.expireTasks()
	manager.advanceRunners(tickTunables)
	manager.dispatchMemoryReconcile()
	manager.dispatchPlanning()
	manager.dispatchIdleDialogues()
	manager.dispatchPathRequests()
	return server.taskEventDeliveries(manager.takeEventFacts())
}

// refreshBodies 缓存本 tick 的伙伴身体快照，编排各阶段共用，避免重复拷贝。
func (m *companionManager) refreshBodies() {
	clear(m.bodies)
	for _, body := range m.engine.CompanionBodies() {
		m.bodies[body.ID] = body
	}
}

func (m *companionManager) body(id companion.ID) (companion.Body, bool) {
	body, ok := m.bodies[id]
	return body, ok
}

// takeEventFacts 排空本 tick 累积的事件事实。
func (m *companionManager) takeEventFacts() []taskEventFact {
	facts := m.events
	m.events = nil
	return facts
}

// applyQueueEvents 把状态机迁移产出的事件事实补上任务身份后累积，同时在
// 迁移点即时评估台词触发节点（dispatchDialogueNode——D6 的唯一任务域接线
// 点）。currentCommand/currentIssuer 在任务占据当前槽位的全程有效，终态清槽后
// 不再产生事件。
func (m *companionManager) applyQueueEvents(slot *companionTaskSlot, events []companion.TaskEvent) {
	for _, event := range events {
		m.dispatchDialogueNode(slot, event)
		m.events = append(m.events, taskEventFact{
			issuer:     slot.currentIssuer,
			definition: slot.definition,
			command:    slot.currentCommand,
			event:      event,
		})
	}
}

// dispatchDialogueNode 在状态机迁移点评估台词触发节点（持有 stepMu 的 tick
// 路径，调用点唯一：applyQueueEvents）：
//   - TaskStarted：任务进入 Running。预计算进展节点集合（follow 尾步任务为
//     空集），并派发开始节点；
//   - TaskProgress：一个中间计划步骤完成（CompleteStep 已推进 StepIndex，
//     刚完成的步骤索引即 StepIndex-1）。仅当该索引在预计算集合中时派发进展
//     节点（携带完成步骤的 kind）；最后一步的完成迁移产出 TaskCompleted 而
//     非 TaskProgress，其「完成表达」由终止台词承载（dialogue_nodes.go
//     「末段永远被覆盖」的语义）；
//   - 终态事件（Completed/Failed/TimedOut/Stopped）：派发终止节点（携带终态
//     与稳定失败原因）。时序约束：本调用发生在终态迁移的同一 tick、FIFO
//     提升（dispatchPlanning 的 BeginHead）之前，结果因此携带正确世代。
//
// 台词派发受 requestDialogue 的全部守卫约束（在途/槽满/未激活/预算即跳过），
// 跳过绝不改变迁移本身——表达平面与事实平面在这里结构分离。
func (m *companionManager) dispatchDialogueNode(slot *companionTaskSlot, event companion.TaskEvent) {
	switch event.Kind {
	case companion.TaskEventStarted:
		current, ok := slot.queue.Current()
		if !ok || current.State != companion.TaskRunning || len(current.Plan.Steps) == 0 {
			// 空/非 Running 是状态机缺陷：防御性跳过台词，不影响事实事件。
			return
		}
		if steps := current.Plan.Steps; steps[len(steps)-1].Kind != companion.PlanStepFollow {
			slot.progressSteps = companion.SelectProgressSteps(len(steps))
		} else {
			// follow 尾步任务（含混合计划）：持续跟随没有步骤完成事实，
			// 进展集合恒空，节点全集是开始/首次到达/终止。
			slot.progressSteps = nil
		}
		m.requestDialogue(slot.definition.ID, companion.DialogueNode{Kind: companion.DialogueNodeStart})
	case companion.TaskEventProgress:
		current, ok := slot.queue.Current()
		if !ok || current.StepIndex <= 0 || current.StepIndex-1 >= len(current.Plan.Steps) {
			return
		}
		completed := current.StepIndex - 1
		for _, selected := range slot.progressSteps {
			if selected == completed {
				m.requestDialogue(slot.definition.ID, companion.DialogueNode{
					Kind:     companion.DialogueNodeProgress,
					StepKind: current.Plan.Steps[completed].Kind,
				})
				return
			}
		}
	default:
		if state, terminal := taskEventTerminalState(event.Kind); terminal {
			m.requestDialogue(slot.definition.ID, companion.DialogueNode{
				Kind:   companion.DialogueNodeTerminal,
				State:  state,
				Reason: event.Reason,
			})
		}
	}
}

// taskEventTerminalState 把终态事件类别映射回 TaskState（终止节点载荷）；
// 非终态事件返回 false，调用方跳过。
func taskEventTerminalState(kind companion.TaskEventKind) (companion.TaskState, bool) {
	switch kind {
	case companion.TaskEventCompleted:
		return companion.TaskCompleted, true
	case companion.TaskEventFailed:
		return companion.TaskFailed, true
	case companion.TaskEventTimedOut:
		return companion.TaskTimedOut, true
	case companion.TaskEventStopped:
		return companion.TaskStopped, true
	default:
		return 0, false
	}
}

// applyPlannerOutcomes 在 tick 边界非阻塞排空规划结果并应用：世代或状态
// 不符的结果直接丢弃（任务已终态或已被替换）。
func (m *companionManager) applyPlannerOutcomes() {
	for {
		select {
		case outcome := <-m.plannerResults:
			m.applyPlannerOutcome(outcome)
		default:
			return
		}
	}
}

func (m *companionManager) applyPlannerOutcome(outcome plannerOutcome) {
	slot := m.slots[outcome.id]
	if slot == nil || !slot.planningInFlight {
		return
	}
	if slot.queue.Generation() != outcome.generation || slot.planningAttempt != outcome.attempt {
		return
	}
	current, ok := slot.queue.Current()
	if !ok || current.State != companion.TaskPlanning {
		return
	}
	slot.planningInFlight = false
	switch {
	case outcome.err == nil && !validCompanionPlanningIdentity(outcome):
		// The generation and attempt match this dispatch, so no other reply
		// will arrive for it: discard the result and fail only the current
		// task instead of leaving the planning gate set forever.
		m.applyQueueEvents(slot, slot.queue.FailPlanning(companion.TaskFailPlannerUnavailable))
	case outcome.err == nil:
		if !m.plannerOutcomeMatchesCurrentAuthority(slot, current.Command, outcome) {
			m.applyQueueEvents(slot, slot.queue.FailPlanning(companion.TaskFailWorldChanged))
			return
		}
		m.applyQueueEvents(slot, slot.queue.AcceptPlan(outcome.result.Plan))
		// 结构校验是纯值操作，同一 tick 完成校验并进入 Running；失败即
		// 以 InvalidPlan 终止，绝不改写或降级模型计划。
		m.applyQueueEvents(slot, slot.queue.FinishValidation(
			m.engine.WorldTime(), m.timeoutMinutes,
		))
		current, ok = slot.queue.Current()
		if !ok || current.State != companion.TaskRunning {
			return
		}
		// 进入 Running 后立即请求第一步的路径（寻路 worker 异步执行）。
		if body, active := m.body(outcome.id); active {
			m.submitPathRequest(slot, outcome.id, body, current)
		}
	case errors.Is(outcome.err, companion.ErrPlannerInvalidPlan):
		m.applyQueueEvents(slot, slot.queue.FailPlanning(companion.TaskFailInvalidPlan))
	default:
		m.applyQueueEvents(slot, slot.queue.FailPlanning(companion.TaskFailPlannerUnavailable))
	}
}

// validCompanionPlanningIdentity 验证桥接结果携带的远端身份，并把它绑定到
// tick 保存的本地 attempt、generation 与冻结快照 digest。`RunID` 和
// `SnapshotID` 必须是规范 UUIDv4；单个布尔标记不能替代逐字段核对。
func validCompanionPlanningIdentity(outcome plannerOutcome) bool {
	requestIdentity := outcome.result.requestIdentity
	if outcome.attempt == 0 || outcome.result.Attempt != outcome.attempt ||
		outcome.result.Generation != outcome.generation ||
		outcome.result.RunID != requestIdentity.RunID ||
		outcome.result.SnapshotID != requestIdentity.SnapshotID ||
		outcome.result.SnapshotDigest != requestIdentity.SnapshotDigest ||
		requestIdentity.SnapshotDigest == "" ||
		requestIdentity.SnapshotDigest != outcome.snapshotDigest {
		return false
	}
	if _, err := companion.ParseID(outcome.result.RunID); err != nil {
		return false
	}
	if _, err := companion.ParseID(outcome.result.SnapshotID); err != nil {
		return false
	}
	return true
}

// plannerOutcomeMatchesCurrentAuthority 在 Agent 返回后的权威 tick 边界重建必要
// 世界事实。冻结快照只用于约束候选生成；提交前计划相关区块 revision、目标
// 方块、place 背包与 follow 在线位置必须仍成立。方块比较读取 dense projection，
// 不依赖有损的 exposed 摘要。
func (m *companionManager) plannerOutcomeMatchesCurrentAuthority(
	slot *companionTaskSlot,
	command companion.TaskCommand,
	outcome plannerOutcome,
) bool {
	body, active := m.body(outcome.id)
	if !active {
		return false
	}
	currentSnapshot, err := m.buildPlanSnapshot(slot.definition, command, slot.currentIssuer, body)
	if err != nil {
		return false
	}
	if err := companion.ValidatePlanAgainstSnapshot(outcome.result.Plan, currentSnapshot); err != nil {
		return false
	}
	for _, step := range outcome.result.Plan.Steps {
		switch step.Kind {
		case companion.PlanStepMine, companion.PlanStepPlace:
			target := core.BlockPos{X: step.X, Y: step.Y, Z: step.Z}
			frozenBlock, _, frozenReady := outcome.snapshot.Terrain.Lookup(target)
			currentBlock, _, currentReady := currentSnapshot.Terrain.Lookup(target)
			if !frozenReady || !currentReady || frozenBlock != currentBlock ||
				!samePlanChunkRevision(outcome.snapshot, currentSnapshot, target.Chunk()) {
				return false
			}
		case companion.PlanStepFollow:
			frozenTarget, frozenOnline := planSnapshotPlayer(outcome.snapshot, step.PlayerID)
			currentTarget, currentOnline := planSnapshotPlayer(currentSnapshot, step.PlayerID)
			if !frozenOnline || !currentOnline || frozenTarget.Position != currentTarget.Position {
				return false
			}
		}
	}
	return true
}

// samePlanChunkRevision 只比较含计划坐标的区块，不把投影内无关区块漂移扩大为
// 整份快照失效。缺失 revision 代表无法证明冻结事实仍然成立，按 fail closed 拒绝。
func samePlanChunkRevision(frozen, current companion.PlanSnapshot, target core.ChunkPos) bool {
	var frozenRevision, currentRevision uint64
	var frozenFound, currentFound bool
	for _, revision := range frozen.ChunkRevisions {
		if revision.Chunk == target {
			frozenRevision, frozenFound = revision.Revision, true
			break
		}
	}
	for _, revision := range current.ChunkRevisions {
		if revision.Chunk == target {
			currentRevision, currentFound = revision.Revision, true
			break
		}
	}
	return frozenFound && currentFound && frozenRevision == currentRevision
}

func planSnapshotPlayer(snapshot companion.PlanSnapshot, id core.PlayerID) (companion.PlanPlayer, bool) {
	for _, player := range snapshot.OnlinePlayers {
		if player.ID == id {
			return player, true
		}
	}
	return companion.PlanPlayer{}, false
}

// applyPathOutcomes 在 tick 边界非阻塞排空寻路结果并应用。
func (m *companionManager) applyPathOutcomes() {
	for {
		select {
		case outcome := <-m.pathResults:
			m.applyPathOutcome(outcome)
		default:
			return
		}
	}
}

func (m *companionManager) applyPathOutcome(outcome pathOutcome) {
	slot := m.slots[outcome.id]
	if slot == nil || !slot.pathInFlight {
		return
	}
	slot.pathInFlight = false
	if slot.queue.Generation() != outcome.generation {
		return
	}
	current, ok := slot.queue.Current()
	if !ok || current.State != companion.TaskRunning {
		return
	}
	if outcome.err != nil {
		// 重算失败计入三连失败；未达上限按固定冷却重试，绝不无限重算。
		if slot.policy.RecordFailure() {
			slot.path = nil
			slot.hasReplanAt = false
			m.applyQueueEvents(slot, slot.queue.FailRun(companion.TaskFailPathUnreachable))
			return
		}
		slot.path = nil
		slot.replanAtTick = slot.policy.ReplanAfter(m.engine.TickCount())
		slot.hasReplanAt = true
		return
	}
	if step := current.Plan.Steps[current.StepIndex]; step.Kind == companion.PlanStepFollow {
		if body, active := m.body(outcome.id); active &&
			m.followTargetCrossDimension(body, step.PlayerID) {
			// 跨维跟随不消费寻路结果：传送前派发的在途路径落地时目标已在
			// 另一维度，直接丢弃（不计失败、不设冷却），与派发侧保持一致。
			slot.path = nil
			slot.hasReplanAt = false
			return
		}
	}
	result := outcome.result
	slot.path = &result
	slot.waypoint = 0
	slot.hasReplanAt = false
}

// expireTasks 用权威世界时间检查 Running 任务的 deadline。到期转 TimedOut，
// 移动随当前任务一起停止（runner 不再为其提交任何输入）。
func (m *companionManager) expireTasks() {
	worldTime := m.engine.WorldTime()
	for _, id := range m.orderedIDs {
		slot := m.slots[id]
		current, ok := slot.queue.Current()
		if !ok || current.State != companion.TaskRunning {
			continue
		}
		events := slot.queue.Expire(worldTime)
		if len(events) == 0 {
			continue
		}
		slot.path = nil
		slot.hasReplanAt = false
		m.applyQueueEvents(slot, events)
	}
}

// advanceRunners 推进全部 Running 任务的执行：重验路径 revision、消费已到达
// 的路径点并提交至多一个移动输入。follow 步骤先经 advanceFollowRunner 的
// 在线性与距离裁决——持续跟随没有「步骤完成」语义，路径走尽或目标漂移只
// 触发按既有冷却的重算，绝不推进步骤索引；mine/place 步骤交给
// advanceInteractionRunner（走近复用同一移动语义，走尽后转入采掘按住/放置
// 提交）。全部执行器跑完后由 releaseFinishedMining 兜底释放已离开采掘
// 步骤但仍按住的采掘意图。
func (m *companionManager) advanceRunners(tickTunables runtime.TickTunables) {
	for _, id := range m.orderedIDs {
		slot := m.slots[id]
		current, ok := slot.queue.Current()
		if !ok || current.State != companion.TaskRunning {
			continue
		}
		// follow 步骤每 tick 先做在线性与距离裁决；返回 false 表示任务已
		// 终态（目标离线），本 tick 不再有可执行内容。
		follow := followStepOf(current)
		if follow != nil && !m.advanceFollowRunner(slot, id, *follow) {
			continue
		}
		// mine/place 步骤：走近与交互的专用执行器，不复用下方 go_to 的
		// 路径走尽即完成语义。
		if step := interactionStepOf(current); step != nil {
			m.advanceInteractionRunner(slot, id, current, *step, tickTunables)
			continue
		}
		if slot.path == nil {
			// follow 距离内（或路径尚未就绪）：不提交任何移动输入，伙伴在
			// 权威物理作用下保持原地；普通任务缺少路径同样等待派发。
			continue
		}
		body, active := m.body(id)
		if !active {
			continue
		}
		if m.advancePathMovement(slot, id, body) && follow == nil {
			m.applyQueueEvents(slot, slot.queue.CompleteStep())
		}
		// follow：目标仍在距离外时路径才会走尽——清空路径等待
		// dispatchPathRequests 按当前目标重算；绝不 CompleteStep（持续
		// 语义，步骤索引不推进）。
	}
	m.releaseFinishedMining()
}

// advancePathMovement 是 go_to 与 mine/place 走近段共用的路径执行体：重验
// 路径 revision、消费已到达的路径点并提交至多一个移动输入。返回 true 表示
// 路径已走尽（调用方据此决定完成步骤或转入交互段）；路径失效时丢弃并按
// 固定冷却安排重算，返回 false。
func (m *companionManager) advancePathMovement(
	slot *companionTaskSlot,
	id companion.ID,
	body companion.Body,
) bool {
	// 路径点提交前重验：结果携带的每个区块 revision 都必须与当前权威
	// 状态一致，失效即丢弃路径并按固定冷却重算。
	if !slot.policy.ShouldUse(*slot.path, slot.waypoint, m.windowRevisions(body)) {
		slot.path = nil
		slot.replanAtTick = slot.policy.ReplanAfter(m.engine.TickCount())
		slot.hasReplanAt = true
		return false
	}
	// 到达检查先于提交输入：路径点 0（起点）在首个 tick 即被消费。
	for slot.waypoint < len(slot.path.Waypoints) {
		if !arrivedAtWaypoint(body.Position, slot.path.Waypoints[slot.waypoint]) {
			break
		}
		slot.waypoint++
		slot.policy.RecordSuccess()
	}
	if slot.waypoint >= len(slot.path.Waypoints) {
		slot.path = nil
		slot.hasReplanAt = false
		return true
	}
	m.engine.EnqueueCompanionAction(contract.CompanionAction{
		ID:   id,
		Kind: contract.CompanionActionMove,
		Input: movementInputToward(
			body.Position, slot.path.Waypoints[slot.waypoint]),
	})
	return false
}

// companionFollowReplanDriftBlocks 是 follow 动态终点的重算漂移阈值（水平
// 格距）：目标玩家的站立格相对上次寻路终点移动超过该距离才丢弃既有路径
// 重算。取 2（跟随距离的一半）的权衡：阈值 ≤1 时玩家每走一两格就触发重算，
// 寻路 worker 被位移噪声打满、既有冷却机制形同虚设；阈值 ≥ 跟随距离 4 时
// 旧路径的尾段已明显落后目标，伙伴要先走完过时路径才更新方向，跟随滞后
// 可感。2 格让重算频率由真实位移驱动（约每 2 格一次），旧路径仍保有朝向
// 目标的可用前缀；世界变化造成的路径失效继续由既有 revision 重验承担，
// 与漂移判定互不重叠。
const companionFollowReplanDriftBlocks = 2

// companionFollowDistanceSquared 是跟随距离边界的平方。常量的价值在命名与
// 语义单一来源（跟随距离平方只有一个定义点）；两次乘法编译期即可折叠，无
// 性能含义。
const companionFollowDistanceSquared = companion.CompanionFollowDistanceBlocks *
	companion.CompanionFollowDistanceBlocks

// followStepOf 返回任务当前执行步骤的 follow 形态；非 follow 步骤（或步骤
// 索引越界的防御情形）返回 nil。
func followStepOf(task companion.Task) *companion.PlanStep {
	if task.StepIndex < 0 || task.StepIndex >= len(task.Plan.Steps) {
		return nil
	}
	step := &task.Plan.Steps[task.StepIndex]
	if step.Kind != companion.PlanStepFollow {
		return nil
	}
	return step
}

// advanceFollowRunner 是持续跟随步骤的每 tick 裁决：先读目标在线性——离线
// 立即以 TaskFailWorldChanged 失败并广播（FIFO 随终态在下一 tick 推进），
// 新任务与恢复的 Running follow 共用这一先验，天然满足「恢复任务在下一
// 动作前先验在线性」；在线则按水平距离裁决：距离内清空路径与重算意图并
// 保持原地（不提交移动输入，物理照常），距离外以目标站立格为动态终点，
// 终点漂移超出阈值时丢弃既有路径交由 dispatchPathRequests 重算。返回
// false 表示任务已进入终态。
func (m *companionManager) advanceFollowRunner(
	slot *companionTaskSlot,
	id companion.ID,
	step companion.PlanStep,
) bool {
	target, online := m.followTarget(step.PlayerID)
	if !online {
		slot.path = nil
		slot.hasReplanAt = false
		slot.hasFollowGoal = false
		m.applyQueueEvents(slot, slot.queue.FailRun(companion.TaskFailWorldChanged))
		return false
	}
	body, active := m.body(id)
	if !active {
		// 身体尚未激活（出生扫描在途）：不裁决距离也不提交输入，等下一 tick。
		return true
	}
	if m.followTargetCrossDimension(body, step.PlayerID) {
		// 目标在另一维度（传送）：原地待命，不提交移动输入、不发起寻路、
		// 不判任务失败——返回旧维后跟随自然恢复。路径与重算意图清空，
		// 避免 `submitPathRequest` 以旧维网格配新维终点做无谓派发。
		slot.path = nil
		slot.hasReplanAt = false
		slot.hasFollowGoal = false
		return true
	}
	if withinFollowDistance(body.Position, target.Position) {
		// 距离边界内：停止提交移动输入。清空既有路径与重算意图，防止
		// dispatchPathRequests 在距离内反复发起寻路；已派发的在途寻路结果
		// 落地后会被下一 tick 的本分支再次清空，最坏多一次寻路。
		slot.path = nil
		slot.hasReplanAt = false
		slot.hasFollowGoal = false
		// 首次到达跟随距离是 follow 任务的第二个台词节点（持续跟随没有
		// 步骤完成事实，这是唯一的「中途」节点）：仅首次消费，此后目标
		// 反复进出边界不再触发。跳过（在途/槽满）同样置位——跳过即放弃，
		// 绝不补发。
		if !slot.followArrivalSpoken {
			slot.followArrivalSpoken = true
			m.requestDialogue(id, companion.DialogueNode{Kind: companion.DialogueNodeFirstArrival})
		}
		return true
	}
	if slot.path != nil && slot.hasFollowGoal && followGoalDrifted(slot.followGoal, target.Position) {
		// 目标漂移超出重算阈值：旧路径指向过时终点。清空路径令
		// dispatchPathRequests 以当前目标重算；此刻必然没有失败冷却在身
		//（冷却仅在 path 为 nil 时存在），重算立即发起。
		slot.path = nil
		slot.hasReplanAt = false
		slot.hasFollowGoal = false
	}
	return true
}

// followTarget 从在线玩家集合解析 follow 目标的当前位置事实；找不到目标
// （从未登录或已断开）即离线。这是持续跟随在线性判定的唯一权威源。
func (m *companionManager) followTarget(playerID core.PlayerID) (companion.PlanPlayer, bool) {
	if m.onlinePlayers == nil {
		return companion.PlanPlayer{}, false
	}
	for _, player := range m.onlinePlayers() {
		if player.ID == playerID {
			return player, true
		}
	}
	return companion.PlanPlayer{}, false
}

// followTargetCrossDimension 报告跟随目标是否位于与伙伴不同的维度：只有
// 注入了 `playerDimension` 且能解析出目标维度时才做判定，否则一律视为同维
// （保持既有行为，不因缺失注入而误保持）。
func (m *companionManager) followTargetCrossDimension(
	body companion.Body,
	playerID core.PlayerID,
) bool {
	if m.playerDimension == nil {
		return false
	}
	dimension, ok := m.playerDimension(playerID)
	return ok && dimension != body.Dimension
}

// withinFollowDistance 报告伙伴与目标玩家的水平距离是否落在跟随距离内。
// 只用水平分量：垂直分离（跳跃/地形高差）由重力与寻路的 Y 语义处理，不
// 参与停止判定，避免伙伴位于玩家头顶/脚下时因垂直差被误判为距离外而抖动。
func withinFollowDistance(from, to [3]float32) bool {
	dx := from[0] - to[0]
	dz := from[2] - to[2]
	return dx*dx+dz*dz <= companionFollowDistanceSquared
}

// followGoalDrifted 报告目标当前位置相对上次寻路终点的水平漂移是否超出
// 重算阈值（平方比较避免开方）。基准取方块中心（+0.5）与站立格语义一致。
func followGoalDrifted(goal pathfind.PathCell, targetPos [3]float32) bool {
	dx := float32(goal.X) + 0.5 - targetPos[0]
	dz := float32(goal.Z) + 0.5 - targetPos[2]
	const limit = companionFollowReplanDriftBlocks * companionFollowReplanDriftBlocks
	return dx*dx+dz*dz > limit
}

// standingCellOf 把一个位置归一为它占用的站立格（feet 格）：X/Z 取 floor、
// Y 取 floor，与寻路网格的方块中心基准一致。玩家脚下格天然满足寻路端点的
// 站立约束（feet/head 可通过、正下方支撑）；个别悬空/嵌墙瞬间由寻路失败
// 与既有冷却重试语义兜底，不在此特判。
func standingCellOf(position [3]float32) pathfind.PathCell {
	return pathfind.PathCell{
		X: int32(math.Floor(float64(position[0]))),
		Y: int32(math.Floor(float64(position[1]))),
		Z: int32(math.Floor(float64(position[2]))),
	}
}

// dispatchPlanning 为每个空闲槽位派发规划：取队首、迁移到 Planning、获取
// 共享并发名额、构造快照，再由 worker 发起请求。共享名额不可用时任务在同
// tick 以 PlannerUnavailable 终结；伙伴未激活时仍保持 Queued。快照构造失败
// 同样以 PlannerUnavailable 真实终结，FIFO 在下一 tick 推进。
func (m *companionManager) dispatchPlanning() {
	for _, id := range m.orderedIDs {
		slot := m.slots[id]
		if slot.planningInFlight {
			// 每伙伴最多一个在途规划请求：在途期间绝不发起第二个。
			continue
		}
		current, hasCurrent := slot.queue.Current()
		if hasCurrent && current.State != companion.TaskQueued {
			continue
		}
		if !hasCurrent {
			// 发令者失配检查位于 BeginHead 之前。失配的准确定义是
			// 「pending 非空而 issuers 为空」：issuers 与 queue.pending
			// 在 Enqueue/restore 时一一配对追加、仅在下方的消费点成对变
			// 化，且本函数从检查点到 BeginHead 之间没有任何 issuers 写者
			//（全部写点都在权威 tick 串行执行），该失配只可能是入队或恢
			// 复路径的配对缺陷，正常不可达。空闲态（pending 与 issuers 同
			// 空）是每伙伴的正常状态，不属失配，绝不打日志——守卫因此必
			// 须同时检查 pending 非空。若把失配处理放在 BeginHead 之后，
			// 队列已把队首提升为当前任务而 issuers 为空，防御分支触发时
			// 任务以 Queued 滞留、槽位残留上一任务的 currentIssuer（或零
			// 值——零值 PlayerID 过不了 ChatEvent.Validate，后续事件将被
			// 静默丢弃），次生行为未定义；前移使缺陷态下队列从未占用槽
			// 位。检查只读，正常路径（issuers 配对非空）的控制流与后继
			// 语句零变化。
			if slot.queue.Len() > 0 && len(slot.issuers) == 0 {
				slog.Error("任务 FIFO 与发令者队列失配", "companion", id)
				continue
			}
			if !slot.queue.BeginHead() {
				continue
			}
			// 新任务从零开始计预算：三连失败上限约束「同一任务内」的连续
			// 重算失败（pathfinding spec），前一个任务遗留的计数（含已耗尽
			// 到 3 的终态计数）不得泄漏进新任务。交互状态同理归零——上一
			// 任务的就绪标记与采掘进度记忆不属于新任务。台词节点状态同理：
			// 进展集合将在新任务进入 Running 时重导出，follow 首达标志与
			// 台词预算按「属于任务」重新计（design.md 裁决预算不持久化）。
			slot.policy = pathfind.PathPolicy{}
			slot.resetInteraction()
			slot.progressSteps = nil
			slot.followArrivalSpoken = false
			slot.dialogueRequests = 0
			slot.currentIssuer, slot.issuers = slot.issuers[0], slot.issuers[1:]
			current, _ = slot.queue.Current()
		}
		slot.currentCommand = current.Command
		body, active := m.body(id)
		if !active {
			// 伙伴尚未激活（出生扫描在途）：任务保持 Queued，等下一 tick。
			continue
		}
		if !slot.queue.BeginPlanning() {
			continue
		}
		if slot.dialogueInFlight {
			m.applyQueueEvents(slot, slot.queue.FailPlanning(companion.TaskFailPlannerUnavailable))
			continue
		}
		select {
		case m.semaphore <- struct{}{}:
		default:
			// 全服并发名额不可用时，任务已真实进入 Planning，并在同一 tick
			// 以 PlannerUnavailable 终结；权威路径从不排队等待容量。
			m.applyQueueEvents(slot, slot.queue.FailPlanning(companion.TaskFailPlannerUnavailable))
			continue
		}
		// 快照构造放在 BeginPlanning 成功之后：构造失败时任务已真实处于
		// Planning 态，FailPlanning 能令其进入终态并清出当前槽位（下一
		// tick 的 BeginHead 推进 FIFO），而不是在 Queued 态上被守卫拒绝、
		// 每 tick 原地重试。快照是纯值操作、不发起模型请求，失败路径只需
		// 归还刚占用的并发名额。
		snapshot, err := m.buildPlanSnapshot(slot.definition, current.Command, slot.currentIssuer, body)
		if err != nil {
			// 快照构造失败是服务端缺陷：令任务失败并保留可诊断日志，
			// 绝不让队列悬挂。
			slog.Error("构造规划快照失败", "companion", id, "error", err)
			m.applyQueueEvents(slot, slot.queue.FailPlanning(companion.TaskFailPlannerUnavailable))
			<-m.semaphore
			continue
		}
		if slot.planningAttempt == ^uint64(0) {
			slog.Error("规划 attempt 已耗尽", "companion", id)
			m.applyQueueEvents(slot, slot.queue.FailPlanning(companion.TaskFailPlannerUnavailable))
			<-m.semaphore
			continue
		}
		slot.planningAttempt++
		if slot.planningAttempt == 0 {
			slot.planningAttempt++
		}
		slot.planningInFlight = true
		m.waitGroup.Add(1)
		go m.plannerWorker(id, slot.queue.Generation(), slot.planningAttempt, snapshot)
	}
}

// plannerWorker 在 worker goroutine 上调用模型：只读不可变快照，结果经有界
// channel 回 tick 边界；ctx 取消（关服）时放弃结果；并发名额在模型调用返回
// 后、结果发送前释放（时序论证见函数内注释）。
func (m *companionManager) plannerWorker(
	id companion.ID,
	generation uint64,
	attempt uint64,
	snapshot companion.PlanSnapshot,
) {
	defer m.waitGroup.Done()
	_, snapshotDigest, digestErr := companion.CanonicalSnapshotDigest(snapshot)
	var result companionPlanningOutcome
	var err error
	if digestErr != nil {
		err = companion.ErrPlannerUnavailable
	} else {
		result, err = m.planner.Plan(m.ctx, companionPlanningRequest{
			CompanionID: id, Generation: generation, Attempt: attempt,
			Snapshot: snapshot, Instruction: snapshot.Command,
		})
	}
	// 释放先于发送：`m.semaphore` 约束的是在途模型调用数，`Plan` 返回即调用
	// 结束、名额自此可复用，结果投递只是队列簿记。若先发送再经 defer 释放，
	// 两者之间没有屏障，tick 线程 try-acquire 的成败便依赖 goroutine 调度
	// （ns 级残余窗口，M5E 递延 8 记录的成因）；前移后「任何观察者看到结果
	// 之前名额已归还」成为严格事实，ctx 取消路径行为不变（取消时同样先
	// 释放、发送走 `<-m.ctx.Done()` 分支放弃结果）。
	<-m.semaphore
	outcome := plannerOutcome{
		id: id, generation: generation, attempt: attempt,
		snapshotDigest: snapshotDigest, result: result, snapshot: snapshot, err: err,
	}
	select {
	case m.plannerResults <- outcome:
	case <-m.ctx.Done():
	}
}

// dispatchPathRequests 为缺少路径的 Running 任务发起寻路请求：首次请求立即
// 发起；失效重算受固定冷却约束。
func (m *companionManager) dispatchPathRequests() {
	tick := m.engine.TickCount()
	for _, id := range m.orderedIDs {
		slot := m.slots[id]
		if slot.pathInFlight || slot.path != nil {
			continue
		}
		current, ok := slot.queue.Current()
		if !ok || current.State != companion.TaskRunning {
			continue
		}
		// mine/place 的交互阶段不再寻路：走近已结束，重算只会把伙伴拉离
		// 交互位置（采掘的零进度信号与放置的距离先验都以当前位置为基准）。
		if slot.interactionPhaseActive(current) {
			continue
		}
		if slot.hasReplanAt && tick < slot.replanAtTick {
			continue
		}
		body, active := m.body(id)
		if !active {
			continue
		}
		m.submitPathRequest(slot, id, body, current)
	}
}

// submitPathRequest 在 tick 边界构造不可变网格并交给 worker 执行整数 A*。
// go_to 步骤以步骤坐标为固定终点；mine/place 步骤的目标是交互对象本身
// （实心方块/待填充空气格），终点改取目标的相邻站立格（interactionGoal）；
// follow 步骤的终点是动态的——每次派发都以目标玩家的当前站立格为准（目标
// 离线或已在跟随距离内时不发起，交由 advanceFollowRunner 的每 tick 先验
// 裁决）。窗口区块未就绪时返回 false（下一 tick 重试，不计失败）。
func (m *companionManager) submitPathRequest(
	slot *companionTaskSlot,
	id companion.ID,
	body companion.Body,
	current companion.Task,
) {
	if slot.pathInFlight {
		return
	}
	// 寻路窗口中心取伙伴当前站立格：与 standingCellOf 的逐分量 floor 归一
	// 完全同构（X/Y/Z 各自 float32→float64→Floor→int32），复用同一实现
	// 保证寻路起点与 follow 动态终点使用同一套格坐标语义。
	center := standingCellOf(body.Position)
	if current.StepIndex >= len(current.Plan.Steps) {
		return
	}
	step := current.Plan.Steps[current.StepIndex]
	goal := pathfind.PathCell{X: step.X, Y: step.Y, Z: step.Z}
	if step.Kind == companion.PlanStepMine || step.Kind == companion.PlanStepPlace {
		goal = m.interactionGoal(body, step)
	}
	if step.Kind == companion.PlanStepFollow {
		// 先解析动态终点再构造网格：距离内/离线时避免一次无谓的区块深拷贝。
		target, online := m.followTarget(step.PlayerID)
		if !online {
			// 目标离线：寻路无从发起。失败裁决由 advanceRunners 的在线性
			// 先验统一产生（每 tick 必达），这里只静默跳过。
			return
		}
		if m.followTargetCrossDimension(body, step.PlayerID) {
			// 目标在另一维度：`advanceFollowRunner` 的跨维分支已先行保持，
			// 这里只防御派发窗口，不计失败、不设冷却。
			return
		}
		if withinFollowDistance(body.Position, target.Position) {
			// 距离内无需路径：advanceFollowRunner 的距离分支先行裁决，
			// 这里只防御「本 tick 内目标恰好走进边界」的窗口。
			return
		}
		goal = standingCellOf(target.Position)
		slot.followGoal = goal
		slot.hasFollowGoal = true
	}
	grid, ok := m.buildPathGrid(body, pathfind.PathWindow{Center: center})
	if !ok {
		return
	}
	slot.pathInFlight = true
	m.waitGroup.Add(1)
	go m.pathWorker(id, slot.queue.Generation(), grid, center, goal)
}

// pathWorker 在 worker goroutine 上执行确定性寻路并把结果回送 tick 边界。
func (m *companionManager) pathWorker(
	id companion.ID,
	generation uint64,
	grid pathfind.PathGrid,
	start, goal pathfind.PathCell,
) {
	defer m.waitGroup.Done()
	result, err := pathfind.FindPath(grid, start, goal)
	select {
	case m.pathResults <- pathOutcome{id: id, generation: generation, result: result, err: err}:
	case <-m.ctx.Done():
	}
}

// taskStates 返回有任务内容的伙伴的任务域观察输入，经 companionPersistence.
// Observe 参与 dirty 判定并随保存载荷落盘。空闲队列（无当前任务且
// FIFO 为空）没有可持久化的任务事实，跳过它避免「首次观察到空队列」被误判
// 为任务状态变化而触发无意义的存档。
func (m *companionManager) taskStates() []companion.TaskQueueState {
	states := make([]companion.TaskQueueState, 0, len(m.orderedIDs))
	for _, id := range m.orderedIDs {
		slot := m.slots[id]
		if slot.queue.Len() == 0 {
			if _, hasCurrent := slot.queue.Current(); !hasCurrent {
				continue
			}
		}
		state := slot.queue.Snapshot()
		state.ID = id
		states = append(states, state)
	}
	return states
}

// restoredIssuerIdentity 是恢复任务的合成发令者事实：指令的真实发令者
// （玩家 ID/名称/位置）不落盘，重启后无法回溯；任务事件又必须携带合法
// 玩家身份才能通过 ChatEvent.Validate 发布，因此使用固定的「未知发令者」
// 身份。位置沿用 `captureIssuer` 的有界缺省；server-only `restored` 标记阻止
// 该合成身份被空闲台词复用为真实受众。
//
// playerID 的两个非零字节是满足 core.PlayerID.Valid() 的最小合成形态：
// 该不变量要求 ID 非全零、byte[6] 的高半字节为 4（UUIDv4 version 位）且
// byte[8] 的高两位为 10（variant 位），0x40 与 0x80 恰好各自点亮一处
// 判定所需的位、不携带任何多余信息；其余字节保持全零，明示这不是任何
// 真实玩家的身份，仅作为事件校验的通行证。
var restoredIssuerIdentity = companionTaskIssuer{
	playerID: core.PlayerID{0, 0, 0, 0, 0, 0, 0x40, 0, 0x80, 0, 0, 0, 0, 0, 0, 0},
	name:     "未知发令者",
	position: [3]float32{0, 1, 0},
	restored: true,
}

// restoreQueues 把启动加载的任务域载荷恢复进对应槽位（newWorld 在构造
// Manager 后调用一次）。未配置（inactive）的伙伴没有槽位，任务事实不
// 恢复——配置移除的伙伴不参与编排，存档中的 inactive 记录仍只保留身体。
func (m *companionManager) restoreQueues(queues []storage.StoredCompanionQueue) {
	for _, queue := range queues {
		slot := m.slots[queue.ID]
		if slot == nil {
			continue
		}
		m.restoreQueue(slot, queue)
	}
}

// restoreQueue 恢复单个槽位的任务域：当前任务与 FIFO 指令按存档顺序回填。
// 归一纪律（恢复侧）：Planning/Validating 按 Queued
// 恢复并保留原始指令，重启后重新发起规划；Running 保留步骤索引与 deadline，
// 但路径绝不落盘，恢复后 slot.path 为 nil——首个动作前必须经
// dispatchPathRequests 按当前权威世界重算，天然满足「恢复任务在下一动作前
// 重验」的规格约束。恢复的 Running 任务没有 Started 事件可依赖，进展节点
// 集合在这里按「计划校验成功后一次性预计算」的同一规则重导出；台词预算从
// 零开始（不持久化，重启松弛 ≤8 次属可接受上界——design.md 裁决）。
func (m *companionManager) restoreQueue(slot *companionTaskSlot, queue storage.StoredCompanionQueue) {
	if queue.HasCurrent {
		task := companion.Task{
			Command:       companion.TaskCommand(queue.Current.Command),
			Plan:          companion.Plan{Steps: queue.Current.PlanSteps},
			StepIndex:     queue.Current.StepIndex,
			State:         queue.Current.State,
			StartTick:     queue.Current.StartTick,
			DeadlineTicks: queue.Current.DeadlineTicks,
		}
		if task.State == companion.TaskPlanning || task.State == companion.TaskValidating {
			task.State = companion.TaskQueued
			task.Plan = companion.Plan{}
			task.StepIndex = 0
			task.StartTick = 0
			task.DeadlineTicks = 0
		}
		if slot.queue.RestoreCurrent(task) {
			slot.currentIssuer = restoredIssuerIdentity
			slot.currentCommand = task.Command
			// 恢复的任务同样从零开始计预算：槽位此刻是新建零值，这里按
			// 「预算属于任务」的同一不变量补一次显式归零，防止未来的恢复
			// 时机变化把上一段运行期的计数带入恢复任务。交互状态同样按
			// 任务边界归零（恢复的 Running mine/place 任务从走近段重新开始）。
			slot.policy = pathfind.PathPolicy{}
			slot.resetInteraction()
			slot.progressSteps = nil
			slot.followArrivalSpoken = false
			slot.dialogueRequests = 0
			// 恢复的 Running 任务直接重导出进展节点集合（计划已通过校验
			// 落盘，集合确定性稳定）；Planning/Validating 已归一为 Queued，
			// 集合留待重新进入 Running 时导出。
			if task.State == companion.TaskRunning && len(task.Plan.Steps) != 0 {
				if steps := task.Plan.Steps; steps[len(steps)-1].Kind != companion.PlanStepFollow {
					slot.progressSteps = companion.SelectProgressSteps(len(steps))
				}
			}
		}
	}
	for _, command := range queue.Pending {
		if slot.queue.Enqueue(companion.TaskCommand(command)) {
			slot.issuers = append(slot.issuers, restoredIssuerIdentity)
		}
	}
}

// beginShutdown 进入关服序列：取消在途模型请求。调用点（Server.Shutdown
// 冻结段）必须已经停止接受 ChatCommand 且先于最终 AI 保存；此后 tick 编排
// 不再运行，队列与 actor 状态随生命周期冻结保持一致。
func (m *companionManager) beginShutdown() {
	m.cancel()
}

// beginMemoryFinalization 把 memory worker 切到独立的关服 context。它不复用
// 已取消的 run context，并始终受 caller 与固定上界共同约束；重试关服会先
// 取消上一轮 context，再建立新一轮可用窗口。
func (m *companionManager) beginMemoryFinalization(ctx context.Context) {
	m.cancelMemory()
	m.memoryCtx, m.cancelMemory = context.WithTimeout(ctx, companionMemoryFinalizationTimeout)
	m.memoryReconcileArmAfterDrain = m.memoryReconcileInFlight
	if m.memoryReconcileInFlight {
		return
	}
	m.armPendingMemoryReconcileReservations()
}

func (m *companionManager) armPendingMemoryReconcileReservations() {
	armed := false
	for id, slot := range m.slots {
		if slot.dialogueReservation == nil {
			continue
		}
		if _, pending := m.memoryReconcilePending[id]; !pending {
			continue
		}
		m.memoryReconcileRequestID[id] = struct{}{}
		armed = true
	}
	if armed {
		m.memoryReconcileRequested = true
		m.memoryReconcileRetryWait = 0
		m.memoryReconcileAttempts = 0
	}
}

// memoryFinalizationError 在派生下一批关服 worker 前检查当前 finalization
// 窗口。caller 与子 context 截止相同时，子 timer 可能稍早触发；此时不能仅
// 检查 caller，否则会用已经取消的 context 派发新的 reconcile。
func (m *companionManager) memoryFinalizationError() error {
	if err := m.memoryCtx.Err(); err != nil {
		return err
	}
	if deadline, ok := m.memoryCtx.Deadline(); ok && !time.Now().Before(deadline) {
		return context.DeadlineExceeded
	}
	return nil
}

// companionManagerTaskStates 是 Observe 调用的空值安全包装。
func (server *Server) companionManagerTaskStates() []companion.TaskQueueState {
	if server.companionManager == nil {
		return nil
	}
	return server.companionManager.taskStates()
}

// taskEventDeliveries 把事件事实转成可发布的 ChatEvent 投递。任务事件与
// 台词事实共用同一 EventID 计数器循环，保持全服严格递增且分配顺序确定
// （事实按 tick 内产生顺序排列）。任务事件与台词事件全部广播（recipient 0）；
// 构造出的非法事件（服务端缺陷）跳过并记录，绝不发布半成品。
func (server *Server) taskEventDeliveries(facts []taskEventFact) []chatDelivery {
	if len(facts) == 0 {
		return nil
	}
	deliveries := make([]chatDelivery, 0, len(facts))
	for _, fact := range facts {
		if server.nextChatEventID == ^uint64(0) {
			slog.Error("chat event ID 耗尽，丢弃任务事件",
				"companion", fact.definition.ID)
			continue
		}
		server.nextChatEventID++
		event := network.ChatEvent{
			EventID:       server.nextChatEventID,
			PlayerID:      fact.issuer.playerID,
			PlayerName:    fact.issuer.name,
			CompanionID:   fact.definition.ID,
			CompanionName: fact.definition.Name,
		}
		if fact.speech != "" {
			// 台词事实：kind CompanionSpeech、reason None、不复述指令，
			// 文本槽携带台词（D6 表达平面广播）。
			event.Kind = network.ChatEventCompanionSpeech
			event.Speech = fact.speech
		} else {
			event.Kind = taskEventKind(fact.event.Kind)
			event.RejectReason = taskEventRejectReason(fact.event)
			event.Command = string(fact.command)
		}
		if err := event.Validate(); err != nil {
			slog.Error("任务事件非法", "companion", fact.definition.ID, "error", err)
			continue
		}
		deliveries = append(deliveries, chatDelivery{event: event})
	}
	return deliveries
}

// taskEventKind 把任务域事件类别映射为协议事件枚举。
func taskEventKind(kind companion.TaskEventKind) network.ChatEventKind {
	switch kind {
	case companion.TaskEventStarted:
		return network.ChatEventTaskStarted
	case companion.TaskEventProgress:
		return network.ChatEventTaskProgress
	case companion.TaskEventCompleted:
		return network.ChatEventTaskCompleted
	case companion.TaskEventFailed:
		return network.ChatEventTaskFailed
	case companion.TaskEventTimedOut:
		return network.ChatEventTaskTimedOut
	case companion.TaskEventStopped:
		return network.ChatEventTaskStopped
	default:
		return network.ChatEventKind(0)
	}
}

// taskEventRejectReason 把失败原因映射到 ChatEvent 的 reason 槽位
// （network.TaskFailReason 的 wire 枚举 16..20，其中 TaskFailInventoryFull=20
// 是 v18 追加的容量失败原因）；非失败事件保持 None。
func taskEventRejectReason(event companion.TaskEvent) network.ChatRejectReason {
	if event.Kind != companion.TaskEventFailed {
		return network.ChatRejectNone
	}
	switch event.Reason {
	case companion.TaskFailPlannerUnavailable:
		return network.ChatRejectReason(network.TaskFailPlannerUnavailable)
	case companion.TaskFailInvalidPlan:
		return network.ChatRejectReason(network.TaskFailInvalidPlan)
	case companion.TaskFailPathUnreachable:
		return network.ChatRejectReason(network.TaskFailPathUnreachable)
	case companion.TaskFailWorldChanged:
		return network.ChatRejectReason(network.TaskFailWorldChanged)
	case companion.TaskFailInventoryFull:
		return network.ChatRejectReason(network.TaskFailInventoryFull)
	default:
		return network.ChatRejectNone
	}
}

// waypointArrivalRadiusSquared 是路径点到达阈值的平方：0.35 格半径。阈值
// 小于半格（0.5），保证伙伴不会在相邻路径点之间“抄近路”提前跳格；停止
// 语义由输入撤销后的地面减速保证，无需额外的输入死区。
const waypointArrivalRadiusSquared = float32(0.35 * 0.35)

// arrivedAtWaypoint 报告伙伴水平位置是否到达路径点（方块中心 ±0.35 格）。
// 只用水平距离：go_to 的垂直分量由跳跃/下落的物理语义保证。
func arrivedAtWaypoint(position [3]float32, cell pathfind.PathCell) bool {
	dx := position[0] - (float32(cell.X) + 0.5)
	dz := position[2] - (float32(cell.Z) + 0.5)
	return dx*dx+dz*dz <= waypointArrivalRadiusSquared
}

// movementInputToward 构造朝路径点移动的规范输入：yaw 朝向目标（yaw=0 面
// -Z，故 yaw=atan2(-dx,-dz)）、MoveZ=+1 沿视线前进；目标路径点高于当前脚
// 下一格及以上时按住 Jump——StepHeight（0.6）不足以登上整格台阶，跳上一格
// 与跨一格间隙都由权威物理裁决。每 tick 每伙伴最多提交这一个输入，实际
// 位移永远由权威物理决定；寻路结果与物理的分歧以物理为准（伙伴贴墙等待
// 重算或超时，绝不改写世界）。
func movementInputToward(position [3]float32, target pathfind.PathCell) physics.Input {
	dx := float32(target.X) + 0.5 - position[0]
	dz := float32(target.Z) + 0.5 - position[2]
	input := physics.Input{MoveZ: 1, Yaw: float32(math.Atan2(-float64(dx), -float64(dz)))}
	feetY := int32(math.Floor(float64(position[1])))
	if target.Y > feetY {
		input.Jump = true
	}
	return input
}
