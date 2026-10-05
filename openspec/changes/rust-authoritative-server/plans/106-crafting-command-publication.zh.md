# 合成命令发布意图

仅接入现有 MoveCrafting 与 TakeCraftingOutput 成功结算，复用已验收库存意图并新增私有 tick-local 合成会话标记，由 TickOutcome 传到末尾 owner 发布。保留唯一 InventoryRecord、记录 diff fallback、Active/owner/顺序/拒绝与 quiet 规则；不改生命周期 open/close、其他 provider 或已通过库存命令。

真实本地 Claude 先追加三个实际 authority/native tick 回归：pack↔grid 同tick往返、先实际入grid再grid0↔1往返、空源/无配方拒绝对照。两个完整 InventoryState/CraftingState 漏发 RED 后最小修复；Codex 规划、机械格式化和门禁，独立 GPT6.1sol 审核。完整必要 Rust/release/相关 Go/OpenSpec 与独立复核通过才接受此单元。继续原依赖，不把本单元当整体4.1/4.2完成；阶段耗尽只开新有界记录，不重置历史。禁止推送合并部署。
