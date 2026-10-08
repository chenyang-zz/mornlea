#!/usr/bin/env bash
# race-changed.sh — 只对「改动包及其反向依赖」跑 race 测试（测试分层纪律的 T1 层）。
#
# 背景：全量 race（`make test-race`，六模块循环）实测约 4.5 分钟且 82% 耗时集中在
# packages/client/cmd/mornlea 与 packages/server/server 两个包（见
# docs/notes/test-quickstart.md）；
# 绝大多数改动只触及叶子包，为它们付全量代价是开发时间的主要浪费之一。
#
# 包集合的构造规则：
#   1. 改动文件集 = `git diff --name-only <base>...HEAD` ∪ 暂存/未暂存改动 ∪
#      未跟踪的 *.go（工作区当前状态全算）；base 默认集成分支 origin/dev，可用 --base 覆盖。
#   2. 文件 → 包：按 `go list` 的包目录前缀映射；不在任何包内的 .go（如 testdata）跳过。
#   3. 反向依赖闭包：沿 `go list` 的 .Imports 传递扩散；.TestImports/.XTestImports
#      只作一层直接依赖（测试对被测包的依赖），不沿测试边继续传递——否则触碰
#      packages/shared/core 这类底座包时闭包近似全仓，工具就失去意义；残余风险由
#      T3 全量门禁与 CI 兜底。
#   4. 恒含 packages/audit（依赖边界/基线版本/文档一致守卫，秒级）。
#   5. 闭包触及 cdylib 消费包（nativeabi/core/physics/mesh/client/sim/server/
#      cmd/mornlea、其 app/capture/benchmark 子包与 cmd/mornlea-server；其中
#      nativeabi/core/physics 在 packages/shared，sim/server 与 cmd/mornlea-server
#      在 packages/server，mesh/client 与 cmd/mornlea 族在 packages/client）时
#      先 `make rust`；纯 Go 叶子改动
#      跳过 Rust 构建，不为无关改动支付构建成本。
#
# 用法:
#   scripts/agents/race-changed.sh            # 计算并运行 go test <集合> -race -count=1
#   scripts/agents/race-changed.sh --diff     # 只打印包集合与依据，不运行
#   scripts/agents/race-changed.sh --base v1  # 换比较基线（默认 origin/dev）
#
# 退出码：包集合为空（无 Go 改动）时 0；测试失败时透传 go test 的退出码。
set -euo pipefail

BASE="origin/dev"
DIFF_ONLY=0
while [ $# -gt 0 ]; do
  case "$1" in
    --diff) DIFF_ONLY=1 ;;
    --base) BASE="$2"; shift ;;
    *) echo "未知参数: $1（支持 --diff / --base <ref>）" >&2; exit 2 ;;
  esac
  shift
done

if ! git rev-parse --verify --quiet "$BASE" >/dev/null; then
  echo "基线 ${BASE} 不存在，回退 HEAD" >&2
  BASE="HEAD"
fi

# 改动文件集：已提交差异 + 暂存 + 未暂存 + 未跟踪，全部过滤出 .go。
changed_files="$( {
  git diff --name-only "${BASE}...HEAD" 2>/dev/null || true
  git diff --name-only
  git diff --cached --name-only
  git ls-files --others --exclude-standard
} | sort -u | grep '\.go$' || true)"

if [ -z "$changed_files" ]; then
  echo "无 Go 改动（相对 ${BASE}），无事可测"
  exit 0
fi

# 文件 → 包：go list 一次给出全部包目录与导入路径。根模块已解散、`./...` 在
# 仓库根不可用，且 go.work 下模式不跨模块：shared、server、client、tools、
# audit 与 contracts 六个模块必须显式列出，否则其改动映射不到包。
WORKSPACE_PACKAGES="./packages/contracts/... ./packages/shared/... ./packages/server/... ./packages/client/... ./packages/tools/... ./packages/audit/..."
pkg_map="$(go list -f '{{.Dir}}|{{.ImportPath}}' ${WORKSPACE_PACKAGES})"
changed_pkgs="$(printf '%s\n' "$changed_files" | while read -r f; do
  dir="$(dirname "$f")"
  abs="$(pwd)/$dir"
  printf '%s\n' "$pkg_map" | awk -F'|' -v d="$abs" '$1 == d { print $2 }'
done | sort -u)"

if [ -z "$changed_pkgs" ]; then
  echo "改动只涉及包外的 .go 文件（如 testdata），无包可测"
  exit 0
fi

# 反向依赖闭包：生产导入边传递扩散；测试导入边只作一层直接依赖。图必须
# 覆盖全部 workspace 模块，否则跨模块的反向依赖断链。
imports_graph="$(go list -f '{{.ImportPath}}|{{join .Imports " "}}|{{join .TestImports " "}}|{{join .XTestImports " "}}' ${WORKSPACE_PACKAGES})"
closure="$(printf '%s\n' "$changed_pkgs")"
frontier="$(printf '%s\n' "$changed_pkgs")"
# 生产边可传递：迭代到不动点（包数有限，最多迭代包总数次）。
while [ -n "$frontier" ]; do
  next=""
  while read -r p; do
    [ -n "$p" ] || continue
    # 找到把 p 列进 .Imports 的包，且尚未入集合的，作为下一层。
    new_dependents="$(printf '%s\n' "$imports_graph" | awk -F'|' -v p="$p" '{
      n = split($2, a, " ")
      for (i = 1; i <= n; i++) if (a[i] == p) print $1
    }')"
    while read -r d; do
      [ -n "$d" ] || continue
      if ! printf '%s\n' "$closure" | grep -qx "$d"; then
        closure="$(printf '%s\n%s\n' "$closure" "$d")"
        next="$(printf '%s\n%s\n' "$next" "$d")"
      fi
    done <<< "$new_dependents"
  done <<< "$frontier"
  frontier="$(printf '%s\n' "$next" | sort -u | grep -v '^$' || true)"
done
# 测试边只作一层直接依赖：测试文件 import 了改动包的包，其测试二进制需要重编重跑。
test_dependents="$(printf '%s\n' "$changed_pkgs" | while read -r p; do
  [ -n "$p" ] || continue
  printf '%s\n' "$imports_graph" | awk -F'|' -v p="$p" '{
    n = split($3, a, " "); for (i = 1; i <= n; i++) if (a[i] == p) print $1
    n = split($4, a, " "); for (i = 1; i <= n; i++) if (a[i] == p) print $1
  }'
done | sort -u)"
closure="$(printf '%s\n%s\n' "$closure" "$test_dependents" | sort -u | grep -v '^$')"
# 恒含 packages/audit：依赖边界、基线版本与文档一致守卫，秒级成本。
closure="$(printf '%s\n%s\n' "$closure" "github.com/channing771/mornlea/packages/audit" | sort -u)"

echo "基线 ${BASE}；改动包："
printf '  %s\n' "$changed_pkgs"
echo "反向依赖闭包（含 packages/audit）："
printf '  %s\n' "$closure"

if [ "$DIFF_ONLY" = 1 ]; then
  exit 0
fi

heavy="$(printf '%s\n' "$closure" | grep -E '/(packages/client/cmd/mornlea/app|packages/client/cmd/mornlea/benchmark|packages/server/server)$' || true)"
if [ -n "$heavy" ]; then
  echo "提示：集合含重型包（${heavy}），预计分钟级；仅迭代验证可改用 --diff 后手动加 -short" >&2
fi

# 运行期消费 cdylib 的包集合（nativeabi/core/physics 已迁入 packages/shared
# 模块，sim/server 与 cmd/mornlea-server 已迁入 packages/server 模块，mesh/
# client 与 cmd/mornlea 族已迁入 packages/client 模块）；触及才前置
# `make rust`，其余改动直接进 Go race 测试。
if printf '%s\n' "$closure" | grep -qE '/(packages/shared/nativeabi|packages/shared/core|packages/shared/physics|packages/client/mesh|packages/client/client|packages/client/cmd/mornlea|packages/client/cmd/mornlea/app|packages/client/cmd/mornlea/capture|packages/client/cmd/mornlea/benchmark|packages/server/sim|packages/server/server|packages/server/cmd/mornlea-server)$'; then
  echo "闭包含 cdylib 消费包，先构建 Rust 动态库（make rust）" >&2
  make rust
fi

# shellcheck disable=SC2086 —— 包列表需要按空白展开为多个参数（不可加引号：
# 引号会把换行分隔的整个列表折叠成单个 malformed import path 参数）。
go test $closure -race -count=1
