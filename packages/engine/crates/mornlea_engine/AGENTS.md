# Rust Engine 内核

## 生产职责

- 本 crate 是 mesh/light、collision、raycast、physics tick 积分和 worldgen 的唯一生产实现；Go 侧只编码领域输入、调用 ABI 并解码输出。
- 保持无窗口、无 GPU surface、无文件或网络 I/O。engine 不拥有跨调用的权威世界状态，也不启动后台模拟线程。
- 伤害、掉落、合成、库存、权限和 tick 阶段等业务规则留在 Go；Rust 只执行输入描述的确定性批量计算。

## Engine ABI

- 出口或布局变化必须同步 `packages/engine/include/mornlea_engine.h`、本 crate FFI、`packages/shared/nativeabi`、ABI 版本和容量/一致性测试。
- 不可信 pointer 与 length 在创建 slice 前校验，panic 转状态码且不得跨 FFI unwind；失败路径不触碰调用方输出。

## 定点验证与入口

- 测试：`cd packages/engine && cargo test -p mornlea_engine --locked`。
- 当前文档入口：`docs/notes/go-rust-division.md`、`openspec/specs/rust-engine-mesh/spec.md`、`src/native/AGENTS.md`。
