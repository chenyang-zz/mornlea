# 容器命令库存发布意图

复用已验收库存意图，仅在现有容器 transfer commit 与 panel drop 的原子 Compound 成功后记录会话，拒绝路径不标记。实际 ray/OpenContainer 建立 lease 后，Chest/Furnace panel36 drop 的库存记录未变但原 Go 仍发布完整 owner InventoryState；先验证容器真实清空，再核实缺失事件 RED，避免夹具拒绝假 RED。第三用例无 lease 拒绝对照。只追加旧4855行后，不改现有库存/合成命令或容器每tick发布频率。

本地 Claude 测试先行与最小实施，Codex 规划、机械格式化、完整必要门禁及独立复核。通过才接受这个原始剩余单元，随后继续原依赖，整体4.1/4.2不宣称完成。禁止推送合并部署。
