# 客户端域 Go 模块指南

## 作用域

本文件补充仓库根 `AGENTS.md`，适用于 `packages/client/`（客户端域独立 Go 模块，
go.work 成员）。更具体的边界见各子目录 `AGENTS.md`：

- `packages/client/client/AGENTS.md`（镜像、预测与 client C ABI bridge）
- `packages/client/runtime/` 与 `packages/client/presentation/`（平台无关会话步进与呈现值；方向由 `packages/audit` 的 allowed 表与 `TestClientRuntimeBoundaryProhibitions` 强制）
- `packages/client/cmd/mornlea/AGENTS.md`（客户端命令子树总纲；app/capture/
  benchmark/devcapture 各有局部指南）
- `packages/client/cmd/mornlea-godot-core/`（Godot pilot 的过渡 Go client-core；终局由 Rust client-core 替换，不得扩成新的实时权威）

## 包所有权

- `client` 持有只读权威镜像、输入预测、消息接收、client ABI bridge 和渲染侧 CPU 编排，不成为第二个权威模拟器。
- `render`（含 `hud`）、`mesh`、`lod`、`assets`、`audio` 持有呈现侧数据描述、CPU 编码与平台桥接职责。
- `cmd/mornlea` 是图形客户端应用入口（薄 main 加 app/capture/benchmark/devcapture 四个功能域子包）。
- `assets.Registry` 持有全注册物品的 16×16 RGBA 只读图标缓存；完整方块从当前
  顶面/侧面材质生成等距小方块，透明轮廓物品同时占 atlas 追加层供世界薄片采样。
  原创建图与 contact sheet 入口见 `assets/ITEM_ICONS.md`。轮廓物品还在材质刷新时
  生成 `ItemIconPrisms` 只读同行同色棱柱缓存，供第一人称持物复用；帧内不得
  重建像素几何。剑、镐、锄另有 `ItemToolParts` 实心类别缓存，
  柄、连接部与刃缘有独立截面，弯折刃段的 `RotationZ` 与三轴收尖的 `Beveled` 只在 CPU 缓存中保存，颜色从当前图标采样并随覆盖刷新。
  第一人称主手固定八个部件（前臂、袖口、拳掌、拇指与四个局部分面）；空手分面贴合闭合拳掌与袖子；工具以掌、下垂拇指和折指围住柄，额外实体腕部复用原袖口侧面部件，真实连接前置掌握与袖口；所有部件共享同一动作根，握点不随相位漂移，肤色读取当前身份头底面 atlas 后作局部暖色分面，袖口/前臂取同身份袖子中部布料色，避免整张袖纹拉伸。联合预算为 264，96 字节编码与 ABI 不变。
  动作直接消费本地归一化相位，以短预备、快速工作段和较慢回收驱动多轴根变换；空手前冲、剑斜斩、镐锄前伸下劈。首尾精确中立、分段速度连续，完整桌面轨迹保留 HUD 与近裁面安全。

## 依赖边界

- 模块生产代码的兄弟 require 以 tidy 收敛为准：`shared` 由域库直接消费；`server`（及其传递引入的 `contracts`）仅由 `cmd/mornlea` 应用入口消费——普通本地与 benchmark 模式在进程内装配本地权威 Host，与 tools 组合两侧的入口同构。允许 require 边由 `packages/audit/audit/boundaries.py` 强制。
- client 域库包（cmd/mornlea 子树之外的任何包，含测试文件）MUST NOT import `packages/server`；该禁令由 `packages/audit/audit/boundaries.py` 源码守卫强制。
- 包级依赖方向以 `packages/audit/audit/dependencies.py` 的 `allowed` 表为唯一真相，不要在指南中复制该表。新增包或依赖边必须先证明方向合理并同步架构门禁。
- engine C ABI 只能由 `packages/shared/nativeabi` 接触；client C ABI 只能由 `packages/client/client` 接触。领域包通过这两个既有 bridge 调用 Rust，不得直接引入 C ABI 或另建生产 fallback。

## 测试组织

测试与被测代码同目录并按关注点组织。共享 helper 的准入、命名和拆分规则遵循 `docs/test-organization.md`，不要新建平行 helper 中心。

## 定点验证

- 客户端域包示例：`go test ./packages/client/client -race -count=1`；处理其他包时把路径替换为对应真实目录。
- 客户端命令子树：`go test ./packages/client/cmd/mornlea/... -race -count=1`；无窗口视觉 `make visual-check`。
- 依赖边界：`make archcheck`。
- 当前文档入口：`docs/notes/go-rust-division.md`、`docs/test-organization.md`。
