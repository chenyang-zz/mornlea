.DEFAULT_GOAL := help

GO := go
CARGO ?= rustup run 1.97.1 cargo
# 默认目标目录隔离到当前 worktree；显式 CARGO_TARGET_DIR 仍可覆盖。
export CARGO_TARGET_DIR ?= $(CURDIR)/packages/engine/target/cargo
RUST_DIR := packages/engine
RUST_DYLIB := $(RUST_DIR)/target/release/libmornlea_engine.dylib
RUST_SO := $(RUST_DIR)/target/release/libmornlea_engine.so
APP := ./packages/client/cmd/mornlea
BINARY := bin/mornlea
SERVER := ./packages/server/cmd/mornlea-server
SERVER_BINARY := bin/mornlea-server
MORNLEA_DYLIB := bin/libmornlea_engine.dylib
MORNLEA_SO := bin/libmornlea_engine.so
PIXEL_PERFECTION_NOTICE_DIR := packages/client/assets/packs/pixel_perfection
PIXEL_PERFECTION_NOTICE_DEST := bin/third-party/pixel-perfection
ARGS ?=

# Artifact identities are explicit inputs shared by local producers and CI consumers.
CI_CANDIDATE_SHA ?= $(shell git rev-parse HEAD)
export CI_CANDIDATE_SHA
CI_LINUX_MANIFEST := build/ci/native-linux.manifest
CI_MACOS_MANIFEST := build/ci/native-macos.manifest
CI_VALIDATE_CANDIDATE_SHA = bash -c '[[ "$$CI_CANDIDATE_SHA" =~ ^([0-9a-f]{40}|[0-9a-f]{64})$$ ]] || { printf "invalid candidate SHA\\n" >&2; exit 1; }'

# go.work 下 `go test ./...` 不跨嵌套模块；全部按模块枚举的入口（test 族、
# dev-check、vet）显式循环该列表，防止新模块成为 ./... 盲区。
GO_TEST_MODULES := ./packages/contracts ./packages/shared ./packages/server ./packages/client ./packages/tools ./packages/audit

.PHONY: help run build build-linux-server test test-race test-race-short test-race-changed test-multiplayer bench-multiplayer archcheck comment-language-check ci-preflight fmt clean visual-check visual-update rust rust-check frontend-check frontend-visual-check frontend-visual-update dev-check companion-agent-check companion-agent-integration agent-planner agent-implementer agent-gates agent-dashboard agent-ui-dev godot-build godot-export-linux godot-check godot-asset-check godot-project-check godot-python-check godot-input-check godot-camera-check godot-target-check godot-entity-check godot-environment-check godot-hud-check godot-disconnect-check godot-smoke godot-terrain-check godot-capability-check godot-playable-smoke godot-visual-evidence godot-visual-compare godot-benchmark
.PHONY: ci-rust-quality ci-frontend ci-native-linux ci-native-macos ci-verify-linux-artifact ci-verify-macos-artifact ci-linux-quality ci-race-server ci-race-rest ci-race-client ci-integration-server ci-integration-client

run test test-multiplayer bench-multiplayer visual-check visual-update: rust
build: rust
test-race: rust

help:
	@printf '%s\n' \
		'常用命令：' \
		'  make run              运行游戏，可通过 ARGS 传递参数' \
		'  make build            构建 bin/mornlea、bin/mornlea-server 与同目录 Rust dylib' \
		'  make build-linux-server 构建 Linux amd64 专服与同目录 Rust .so' \
		'  make test             运行全部测试(按 go.work 六模块逐一循环)' \
		'  make test-race        使用 race detector 运行全部测试(六模块循环,语义与单模块仓库一致)' \
		'  make test-race-short   race detector 快速冒烟(六模块循环 + `-short` 跳过重型测试)' \
		'  make test-race-changed 只对改动包及其反向依赖跑 race(T1 层;RACE_BASE=ref 换基线)' \
		'  make dev-check        迭代期快检:gofmt/六模块 vet+短测试与 Rust 静态检查' \
		'  make ci-preflight     运行平台无关的 CI 前置检查与包分区验证' \
		'  make ci-rust-quality  Run pinned Rust formatting, lint, and unit gates' \
		'  make ci-frontend      Run the frozen frontend dependency and validation gates' \
		'  make ci-native-linux  Build, load-check, and package the Linux server artifact' \
		'  make ci-native-macos  Build and package the macOS native artifact' \
		'  make ci-verify-linux-artifact Verify the candidate Linux artifact before use' \
		'  make ci-verify-macos-artifact Verify the candidate macOS artifact before use' \
		'  make ci-linux-quality Compile and vet supported Linux packages and run focused gates' \
		'  make ci-race-server   Verify Linux artifacts and run the server race slice' \
		'  make ci-race-rest     Verify Linux artifacts and run the neutral race slice' \
		'  make ci-race-client   Verify macOS artifacts and run the client race slice' \
		'  make ci-integration-server Run Agent process and server parity contracts' \
		'  make ci-integration-client Run the independent server probe, reports, and benchmarks' \
		'  make test-multiplayer 运行 M3C 八玩家与 v6 报告测试' \
		'  make bench-multiplayer 运行三组 M3C 多人微基准' \
		'  make archcheck        验证依赖闭包与无图形服务端边界' \
		'  make comment-language-check 验证英文源码注释迁移基线只能下降' \
		'  make rust             构建固定版本的 Rust cdylib' \
		'  make rust-check       运行 Rust 格式、clippy 与单测' \
		'  make frontend-check   菜单 WebView 前端门禁(冻结安装+typecheck+vitest+构建+dist 一致)' \
		'  make companion-agent-check 运行伙伴 Agent locked 安装、格式、静态检查、类型检查与 Python 单测' \
		'  make companion-agent-integration 运行无外网 Go/Python 伙伴 Agent 真进程合同' \
		'  make godot-build         Optional pilot gate: build the offline desktop distribution unit' \
		'  make godot-export-linux  Optional pilot gate: export and verify the Linux production catalog' \
		'  make godot-check         Optional pilot gate: load the Godot project headlessly' \
		'  make godot-asset-check   Optional pilot gate: verify generated Godot assets match their inputs' \
		'  make godot-project-check Optional pilot gate: validate the Godot project structure' \
		'  make godot-python-check  Optional pilot gate: locked Godot Python static and unit checks' \
		'  make godot-input-check   Optional pilot gate: validate desktop semantic input' \
		'  make godot-camera-check  Optional pilot gate: validate typed camera mapping' \
		'  make godot-target-check  Optional pilot gate: validate typed target feedback' \
		'  make godot-entity-check  Optional pilot gate: validate remote-player presentation' \
		'  make godot-environment-check Optional pilot gate: validate typed environment presentation' \
		'  make godot-hud-check      Optional pilot gate: validate confirmed HUD presentation' \
		'  make godot-disconnect-check Optional pilot gate: validate terminal session behavior' \
		'  make godot-smoke         Optional pilot gate: 100-iteration isolated Godot headless smoke' \
		'  make godot-terrain-check  Optional pilot gate: headless transcript terrain scenario end to end' \
		'  make godot-capability-check Optional pilot gate: validate disabled coarse capability reservations' \
		'  make godot-playable-smoke Optional pilot gate: real-server minimum playable scenario' \
		'  make godot-visual-evidence Optional pilot gate: headless Godot visual evidence' \
		'  make godot-visual-compare Optional pilot gate: classify Godot evidence against tracked baselines' \
		'  make godot-benchmark      Optional pilot gate: Godot pilot v23 performance report' \
		'  make fmt              格式化全部 Rust 与 Go 源码' \
		'  make visual-check     跑视觉场景并与 golden 基线比对（SCENES= 只跑场景子集、GIFS=1 生成 GIF 供人工审查）' \
		'  make visual-update    重新生成 golden 基线（VISUAL_OUT 覆盖输出目录，SCENES= 只更新所列场景）' \
		'  make frontend-visual-check    UI 部件视觉基线比对（本机 Chrome，不进 CI）' \
		'  make frontend-visual-update   覆盖 UI 部件视觉基线 PNG（人工确认后使用）' \
		'  make clean            删除 bin 目录' \
		'  make agent-planner    手动运行规划者工作者(docs/agents/planner.md)' \
		'  make agent-gates      运行标准门禁汇总(scripts/agents/gates.sh)' \
		'  make agent-dashboard   构建并启动本地执行状态看板' \
		'  make agent-ui-dev      启动看板前端开发服务器(代理 /api)' \
		'  make help             显示此帮助'

run:
	$(GO) run $(APP) $(ARGS)

rust:
	cd $(RUST_DIR) && $(CARGO) build --locked --release
	@# dylib 部署与 macOS 的 @rpath 改写、codesign 统一收在
	@# scripts/engine/deploy-dylib.sh，意图与边界见该脚本注释。
	scripts/engine/deploy-dylib.sh "$(CARGO_TARGET_DIR)/release" "$(RUST_DIR)/target/release"

rust-check:
	cd $(RUST_DIR) && $(CARGO) fmt --check
	cd $(RUST_DIR) && $(CARGO) clippy --workspace --all-targets -- -D warnings
	cd $(RUST_DIR) && $(CARGO) test --workspace --locked

# frontend-check:菜单 WebView 前端门禁。pnpm 不全局安装,由 package.json 的
# packageManager 字段经 corepack 按钉版自动供给,--frozen-lockfile 是唯一安装
# 姿势;末行校验构建产物与入库 dist 一致,dist 入库后任何漂移都会让门禁变红。
FRONTEND_DIR := packages/engine/crates/mornlea_client/frontend

frontend-check:
	cd $(FRONTEND_DIR) && corepack pnpm install --frozen-lockfile
	cd $(FRONTEND_DIR) && corepack pnpm typecheck && corepack pnpm test && corepack pnpm build
	git diff --exit-code -- $(FRONTEND_DIR)/dist

# frontend-visual-*:UI 部件视觉基线（本机开发工具，不进 CI、零网络、不触
# dist）。管线构成与基线更新纪律见 packages/engine/crates/mornlea_client/
# frontend/AGENTS.md 的「UI 部件视觉基线」小节。
frontend-visual-check:
	cd $(FRONTEND_DIR) && corepack pnpm visual-check

frontend-visual-update:
	cd $(FRONTEND_DIR) && corepack pnpm visual-update

# agent-board 看板前端：与菜单前端同一 corepack pnpm 钉版姿势（版本读
# package.json 的 packageManager 字段），冻结安装保证可复现。
AGENT_BOARD_WEB := packages/tools/agent-board/web

build:
	@mkdir -p $(dir $(BINARY))
	$(GO) build -ldflags='-extldflags=-Wl,-rpath,@loader_path' -o $(BINARY) $(APP)
	$(GO) build -ldflags='-extldflags=-Wl,-rpath,@loader_path' -o $(SERVER_BINARY) $(SERVER)
	cp $(RUST_DYLIB) $(MORNLEA_DYLIB)
	@mkdir -p $(PIXEL_PERFECTION_NOTICE_DEST)
	cp $(PIXEL_PERFECTION_NOTICE_DIR)/ATTRIBUTION.md $(PIXEL_PERFECTION_NOTICE_DEST)/ATTRIBUTION.md
	cp $(PIXEL_PERFECTION_NOTICE_DIR)/LICENSE.txt $(PIXEL_PERFECTION_NOTICE_DEST)/LICENSE.txt
	cp $(PIXEL_PERFECTION_NOTICE_DIR)/PROVENANCE.json $(PIXEL_PERFECTION_NOTICE_DEST)/PROVENANCE.json

build-linux-server: rust
	@mkdir -p $(dir $(SERVER_BINARY))
	CGO_ENABLED=1 GOOS=linux GOARCH=amd64 $(GO) build \
		-ldflags='-extldflags=-Wl,-rpath,$$ORIGIN' \
		-o $(SERVER_BINARY) $(SERVER)
	cp $(RUST_SO) $(MORNLEA_SO)

test:
	for module in $(GO_TEST_MODULES); do $(GO) test $$module/... || exit 1; done

test-race:
	for module in $(GO_TEST_MODULES); do $(GO) test $$module/... -race || exit 1; done

# test-race-short:迭代期 race 冒烟——跳过与 `-short` 相同的重型测试,
# 速度比 `test-race` 快一个数量级;提交前仍跑全量 `test-race`。
test-race-short:
	for module in $(GO_TEST_MODULES); do $(GO) test $$module/... -race -short || exit 1; done

# test-race-changed:测试分层纪律的 T1 层——只对「改动包及其反向依赖」跑
# race(集合由 scripts/agents/race-changed.sh 从 git diff 求,恒含 archcheck),
# 绝大多数改动秒级到分钟级;全量 `test-race` 留给 T3 与 CI。RACE_BASE 覆盖基线。
# Rust 构建不在此处前置:脚本检测到闭包含 cdylib 消费包时才按需 `make rust`。
test-race-changed:
	scripts/agents/race-changed.sh $(if $(RACE_BASE),--base $(RACE_BASE),)

test-multiplayer:
	$(GO) test ./packages/client/client ./packages/server/server ./packages/client/cmd/mornlea/benchmark ./packages/tools/perfcheck \
		-run 'Test(PerfReportV6|ScenarioV6|PerfcheckV6|PerfcheckV5SameScenario|PerformanceThresholds|InterestObserver|HostStats|BenchmarkServerEpoch|BenchmarkServerMeasuredWindow)' -count=1

bench-multiplayer:
	$(GO) test ./packages/shared/network ./packages/server/server ./packages/client/render -run '^$$' \
		-bench '(RemotePlayerStateCodec|EightPlayerInterest|RemoteAvatarNameTag)' -benchmem -count=3

archcheck:
	$(GO) test ./packages/audit -count=1
	test -z "$$($(GO) list -deps $(SERVER) | rg 'packages/client/(client|mesh|render)|gfxspike|glfw|webgpu|x/image/font')"

comment-language-check:
	$(GO) test ./packages/audit -run 'EnglishCommentMigration|CodeCommentLanguage|CommentScanner' -count=1

ci-preflight:
	scripts/ci/doctor.sh preflight
	bash -o pipefail -c 'gofmt_output="$$(git ls-files -z -- '\''*.go'\'' | xargs -0 gofmt -l)" || exit $$?; test -z "$$gofmt_output"'
	npx --yes @fission-ai/openspec@1.7.0 validate --all --strict --no-interactive
	node --test scripts/agent-hooks/guard.test.mjs
	$(MAKE) comment-language-check
	scripts/ci/package-inventory.sh --check
	$(GO) test ./packages/audit -skip '^TestGodotAssetSyncIsDeterministicAndRejectsManualFiles$$' -count=1

ci-rust-quality:
	scripts/ci/doctor.sh rust
	$(MAKE) rust-check

ci-frontend:
	scripts/ci/doctor.sh frontend
	$(MAKE) frontend-check

ci-native-linux:
	@$(CI_VALIDATE_CANDIDATE_SHA)
	scripts/ci/doctor.sh native-linux
	$(MAKE) build-linux-server
	$(GO) test ./packages/shared/nativeabi ./packages/shared/core ./packages/shared/physics ./packages/client/mesh -race -count=1
	scripts/ci/verify-linux-bundle.sh --root "$(CURDIR)"
	mkdir -p build/ci
	scripts/ci/package-native-artifact.sh --platform linux-amd64 --sha "$(CI_CANDIDATE_SHA)" --root "$(CURDIR)" --manifest "$(CI_LINUX_MANIFEST)" -- \
		bin/mornlea-server bin/libmornlea_engine.so packages/engine/target/release/libmornlea_engine.so

ci-native-macos:
	@$(CI_VALIDATE_CANDIDATE_SHA)
	scripts/ci/doctor.sh native-macos
	$(MAKE) rust
	mkdir -p build/ci
	@platform=$$(scripts/ci/platform-id.sh) && \
		case "$$platform" in macos-arm64|macos-x86_64) ;; *) printf 'macOS artifact requires a macOS host\n' >&2; exit 1 ;; esac && \
		scripts/ci/package-native-artifact.sh --platform "$$platform" --sha "$(CI_CANDIDATE_SHA)" --root "$(CURDIR)" --manifest "$(CI_MACOS_MANIFEST)" -- \
		packages/engine/target/release/libmornlea_engine.dylib packages/engine/target/release/libmornlea_client.dylib

ci-verify-linux-artifact:
	scripts/ci/verify-native-artifact.sh --platform linux-amd64 --sha "$(CI_CANDIDATE_SHA)" --root "$(CURDIR)" --manifest "$(CI_LINUX_MANIFEST)"

ci-verify-macos-artifact:
	@platform=$$(scripts/ci/platform-id.sh) && \
		case "$$platform" in macos-arm64|macos-x86_64) ;; *) printf 'macOS artifact requires a macOS host\n' >&2; exit 1 ;; esac && \
		scripts/ci/verify-native-artifact.sh --platform "$$platform" --sha "$(CI_CANDIDATE_SHA)" --root "$(CURDIR)" --manifest "$(CI_MACOS_MANIFEST)"

ci-linux-quality: ci-verify-linux-artifact
	scripts/ci/run-linux-quality.sh

ci-race-server: ci-verify-linux-artifact
	scripts/ci/run-go-race.sh server

ci-race-rest: ci-verify-linux-artifact
	scripts/ci/run-go-race.sh rest

ci-race-client: ci-verify-macos-artifact
	scripts/ci/run-go-race.sh client

ci-integration-server: ci-verify-linux-artifact
	scripts/ci/run-integration-server.sh

ci-integration-client: ci-verify-macos-artifact
	scripts/ci/run-integration-client.sh

# dev-check:迭代期快检——gofmt 检查、vet、全仓短测试(重型测试经 `-short` 跳过)
# 与 Rust fmt/clippy/单测。完整门禁(test/test-race/visual-check/rust-check)
# 仍留给 CI 与提交前,短模式不做任何正确性放宽。
dev-check:
	@unformatted=$$(find . -type f -name '*.go' -not -path './vendor/*' -not -path './.worktrees/*' -not -path './.claude/worktrees/*' -exec gofmt -l {} +); if [ -n "$$unformatted" ]; then echo "gofmt 需要格式化: $$unformatted"; exit 1; fi
	for module in $(GO_TEST_MODULES); do $(GO) vet $$module/... || exit 1; done
	for module in $(GO_TEST_MODULES); do $(GO) test $$module/... -short || exit 1; done
	cd $(RUST_DIR) && $(CARGO) fmt --check
	cd $(RUST_DIR) && $(CARGO) clippy --workspace --all-targets -- -D warnings
	cd $(RUST_DIR) && $(CARGO) test --workspace --locked

AGENT_DIR := packages/agent
AGENT_PYTHON := $(CURDIR)/$(AGENT_DIR)/.venv/bin/python
COMPANION_AGENT_DIR := $(AGENT_DIR)
COMPANION_AGENT_PYTHON := $(AGENT_PYTHON)

companion-agent-check:
	cd $(AGENT_DIR) && uv sync --locked
	cd $(AGENT_DIR) && uv run ruff format --check .
	cd $(AGENT_DIR) && uv run ruff check .
	cd $(AGENT_DIR) && uv run mypy
	cd $(AGENT_DIR) && uv run pytest -q

companion-agent-integration:
	cd $(AGENT_DIR) && uv sync --locked
	cd $(AGENT_DIR) && uv run ruff format --check tests/integration && uv run ruff check tests/integration
	cd $(AGENT_DIR) && uv run mypy
	MORNLEA_AGENT_PYTHON=$(AGENT_PYTHON) MORNLEA_COMPANION_AGENT_PYTHON=$(COMPANION_AGENT_PYTHON) $(GO) test ./packages/shared/companion ./packages/server/server \
		-run 'CompanionAgent.*Integration|CrossLanguage|MCP.*Integration' -race -count=1 -timeout=120s

fmt:
	cd $(RUST_DIR) && $(CARGO) fmt
	find . -type f -name '*.go' \
		-not -path './vendor/*' \
		-not -path './.worktrees/*' \
		-not -path './.claude/worktrees/*' \
		-exec gofmt -w {} +

# visual-check：SCENES= 传逗号分隔场景子集（如 SCENES=mining-crack-early,mining-crack-heavy）
# 只比对所列场景；GIFS=1 让纯比对运行也生成 GIF 剧本到输出目录供人工审查。缺省跑全部场景且不生成 GIF。
visual-check:
	$(GO) run $(APP) --capture $(or $(VISUAL_OUT),build/visual) $(if $(SCENES),--capture-scenes $(SCENES)) $(if $(GIFS),--capture-gifs)

# visual-update：SCENES= 传逗号分隔场景子集，只更新所列场景的 golden；GIF 基线恒生成，不受子集影响。
visual-update:
	$(GO) run $(APP) --capture $(or $(VISUAL_OUT),build/visual) --update-golden $(if $(SCENES),--capture-scenes $(SCENES))

clean:
	rm -rf bin

# agent-*:工作者入口——角色卡见 docs/agents/,开发流程见 docs/development-process.md。
# AGENT_TOOL=claude|codex(默认 claude);AGENT_EXTRA_ARGS 透传 CLI。
agent-planner:
	./scripts/agents/run-agent.sh planner

agent-implementer:
	./scripts/agents/run-agent.sh implementer

agent-gates:
	./scripts/agents/gates.sh

agent-dashboard:
	cd $(AGENT_BOARD_WEB) && corepack pnpm install --frozen-lockfile
	cd $(AGENT_BOARD_WEB) && corepack pnpm run build
	$(GO) run ./packages/tools/agent-board

agent-ui-dev:
	cd $(AGENT_BOARD_WEB) && corepack pnpm run dev

# godot-*: optional pilot gates for the Godot client migration. They touch the
# Godot/Python runtime ONLY when explicitly invoked; legacy targets (build,
# test, run and friends) must not depend on them or probe scripts/godot.
godot-build:
	scripts/godot/build-python-runtime.sh --verify --offline
	scripts/godot/build-core.sh --profile debug --verify
	scripts/godot/build-core.sh --profile release --verify
	scripts/godot/build-extension.sh --profile debug
	scripts/godot/build-extension.sh --profile release --verify

godot-export-linux:
	scripts/godot/export-linux.sh --verify

godot-check:
	scripts/godot/godot.sh --headless --path apps/mornlea-godot --editor --quit

godot-asset-check:
	scripts/godot/sync-assets.sh --check

godot-project-check:
	scripts/godot/validate-project.sh

godot-python-check:
	scripts/godot/python-check.sh --locked

godot-input-check:
	scripts/godot/input-check.sh

godot-camera-check:
	scripts/godot/camera-check.sh

godot-target-check:
	scripts/godot/target-check.sh

godot-entity-check:
	cd $(RUST_DIR) && $(CARGO) test -p mornlea_godot entity_snapshot --locked
	scripts/godot/entity-check.sh

godot-environment-check:
	scripts/godot/environment-check.sh

godot-hud-check:
	scripts/godot/hud-check.sh

godot-disconnect-check:
	go test ./packages/client/runtime -run 'Disconnect|Overflow|Shutdown' -race -count=1

godot-smoke:
	scripts/godot/smoke.sh --iterations 100 --isolated-python

godot-terrain-check:
	scripts/godot/godot-terrain-check.sh

godot-capability-check:
	scripts/godot/capability-check.sh

godot-playable-smoke:
	scripts/godot/playable-smoke.sh --duration 300s

godot-visual-evidence:
	scripts/godot/capture.sh

godot-visual-compare:
	scripts/godot/visual-compare.sh

godot-benchmark:
	scripts/godot/benchmark.sh
