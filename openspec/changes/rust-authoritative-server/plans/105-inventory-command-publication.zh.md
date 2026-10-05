# Inventory command 发布意图

复用现有唯一 InventoryRecord，只增加私有 tick-local 会话标记，经现有 TickOutcome 传给最终 owner 发布。inventory::run 现有五类命令变更 patch 成功后标记；合法 EquipArmor 等值交换也按 Go 标记。SelectHotbar 等值不标记；其他移动无吸收/同格/空源沿既有拒绝。原记录 diff fallback、Active 强制条件、发布顺序及其他 provider 不变。

实际 authority 登录/validated submit/advance_tick 追加测试：0→2→0 最终发布一次；2→0→2 command budget1再2，首 tick 冻结 commands3/carry2、下一 commands2/carry0，证明 carried identity 与最终一次；合法铁盔等值互换一次。各项验证 owner 完整 wire InventoryState、foreign 零份、下一 quiet 清除，保持旧文件所有字节。真正 tests-only RED 后由本地 Claude 最小修复；Codex 仅规划、Rustfmt、门禁、审核。

作者限现有 inventory.rs、core/state.rs、publication_project.rs、publication_projection.rs 末尾、server AGENTS；无新公有接口/库存所有者/rollback 策略。对应原 supported outcome 与 sequence/budget 原要求。完整 Rust、release、相关 Go、OpenSpec、独立审查后才在唯一 tasks.md 接受本节点；其他 inventory dirty writers、自动 acquisition 与整体4.1/4.2仍未完成。禁止部署、推送或合并。
