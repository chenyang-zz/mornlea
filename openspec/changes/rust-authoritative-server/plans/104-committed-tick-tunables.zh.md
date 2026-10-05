# 已提交 tick 参数保留

目标：让现有 EnvironmentState 持有的 checked RuleTunables 在真实 advance_tick 中保留。映射原规范 supported outcome 与 tick-start freeze；不宣告配置文件加载、启动组装或整体4.1/4.2完成。

复用唯一环境记录及 TickContext 独占借用。仅移除 freeze_environment 无条件重置参数；缺少环境时的 metadata/default 初始化、next_tick 和既有气候推进保持。无需新 API、全局配置所有者或验证规则。

真实本地 Claude 首先在现有 source_player_restore 测试文件末尾追加两项实际 tick 回归及默认控制，主实现不动。两 tick 保留全部19个 checked 字段；实际耕地耗竭 threshold2000、sat500、ex3999 加 source cost5 应产生 hunger19/sat0/ex4，并核实原生方块、publication、Memory 解码与下一静止 tick。Host 验证真实 RED 后 Claude 最小修复；Codex 仅格式化、门禁与独立审查。

作者仅可改 core/state.rs 的 freeze_environment、上述测试追加与 server AGENTS 的环境所有权说明。主控维护英文计划、tasks 与 ledger。完整 Rust、release、相关 Go 与 OpenSpec 及精确 SHA 独立审核后才更新唯一任务清单 tasks.md；其他原缺口保持明确未完成。现有气候、Snow、生命周期及修改守卫不变。
