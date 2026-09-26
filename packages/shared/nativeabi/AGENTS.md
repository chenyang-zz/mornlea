# Engine ABI 桥接

## 唯一入口

- 本包是 Go 调用 `mornlea_engine` C ABI 的唯一入口；mesh、light、collision、raycast、physics 与 worldgen 的生产路径不得新增 Go fallback。
- 领域包负责构造语义输入，本包只负责稳定布局、调用和状态转换，不承载玩法规则。

## FFI 边界

- 进入 C 前校验长度、容量、数值范围、别名与固定布局；Rust 侧仍须独立校验不可信指针和长度。
- 传入 C 的 pointer 和 slice 只在同步调用期间有效。Rust 不得保存 Go 内存地址，Go 也不得在调用返回前移动、复用或释放缓冲。
- 固定容量、overflow 和成功元数据是 ABI 契约；修改上界时同步更新 Go/Rust 常量与跨语言容量测试，不得只放宽一侧。
- ABI 变化必须同批更新 `packages/engine/include/mornlea_engine.h`、Rust FFI、Go bridge、ABI 版本与一致性测试。

## 定点验证与入口

- 测试：`go test ./packages/shared/nativeabi -race -count=1`。
- 当前文档入口：`docs/notes/go-rust-division.md`。

## Package-local raw oracle (`kernel_oracle_test.go`)

- `TestKernelABIRawOracle` is the package-local raw producer for the native
  numerical corpus. The public bridge panics on every non-OK status, so
  malformed and short-capacity observations are reachable only through the
  package-private version helpers used here. Each case runs the real exported
  symbol against a canary-filled caller arena, asserts the status and the
  untouched-canary rule locally, and freezes the normalized observation.
- Test-only code may use the existing cgo bridge but must not add a
  production API, a second `import "C"`, or a tracked write. Drafts publish
  create-exclusively under the `nativeabi-kernel` child of the external
  directory named by `RUNTIME_ORACLE_EXPORT_DIR`; an unset variable exports
  nothing. Export resolves through the nearest existing ancestor, rejects
  repository containment and any symlink below that ancestor, validates every
  asset path and duplicate before mutation, and rejects a preexisting final
  child, so reruns never silently replace evidence.
