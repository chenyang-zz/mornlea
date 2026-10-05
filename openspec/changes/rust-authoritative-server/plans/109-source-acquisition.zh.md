# Source acquisition 调用者计划说明

规范实现接口、算法与验收以 [英文计划](109-source-acquisition.md) 为准；tasks.md 是唯一状态源。

本单元从已验收 companion 恢复生产者继续，显式 library caller 借用原 background scheduler 与 Native GenerationPool，在真实 Acquire 前使用本地恢复 book 判断完成事件是否仍 wanted，在 AdvanceActors 后且 hostile/gameplay 前仅一次 dirty reconcile。手动 advance_tick/replace/start/offer 语义保持。容量不足保留未启动 FIFO 请求；真实 typed failure 不冒充缺失。Pending 出生扫描 dirty 重试与普通 saved-restore 失败不逐 tick 重试分开验收。

实际 Disk saved→Ready 和 Disk missing→NeedsGeneration→Native→Ready 是主验收，双对象或声明编译不接受集成。限制仍为 wanted/候选36660、resident36676、load8、CPU8、staged16，单次 admission 尝试16。Loom 控制器继续固定 b9e4cceb576e0f1f1187d88ae35996e65b727afc；Claude 写 Rust/测试，Codex 只设计、文档、格式化与验收。

不重做 companion，不改 Go/binary/controlplane，不等待新超时功能。原4.1/4.2、自动 save/cache/bootstrap、完整可执行 runtime 与 trusted observer composition 保持开放；本计划没有宣称完成。
