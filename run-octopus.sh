#!/usr/bin/env bash
# 启动 octopus-desktop + 清 WKWebView 缓存，保证前端最新。
#
# 两阶段：
#   阶段 1（编译打包）：清缓存 + 构建前端 + cargo build（不杀进程，可热构建）
#   阶段 2（运行）：    杀旧进程 + 执行阶段 1 产出的可执行包
#
# 用法：
#   ./run-octopus.sh                默认：阶段 1 + 阶段 2（编译完接着启动）
#   ./run-octopus.sh --not-run      只执行阶段 1（只编译打包，不启动）
#   ./run-octopus.sh --no-compile   只执行阶段 2（直接启动已编译的可执行包）
#
#   build profile 选项（与阶段选择可组合，取第一个匹配）：
#   ./run-octopus.sh --debug        debug build（快链接 + devtools）
#   ./run-octopus.sh --no-lto       --release（无 LTO，开发期迭代最快）
#   ./run-octopus.sh --profiling    --profile profiling（带符号，samply/xctrace 用）
set -euo pipefail

# ── 参数解析 ──
RUN_STAGE1=true   # 编译打包
RUN_STAGE2=true   # 启动运行
RELEASE="--profile optimize"

for arg in "$@"; do
  case "$arg" in
    --no-run)     RUN_STAGE2=false ;;  # 只编译
    --no-compile) RUN_STAGE1=false ;;  # 只运行
    --debug)      RELEASE="" ;;
    --no-lto)     RELEASE="--release" ;;
    --profiling)  RELEASE="--profile profiling" ;;
  esac
done

# ── 定位仓库根 + desktop crate ──
REPO_ROOT="$(cd "$(dirname "$0")" && pwd)"
DESKTOP_DIR="$REPO_ROOT/crates/desktop"

# 根据 profile 推断产物路径 + 二进制名
# optimize/release → target/<profile>/octopus-desktop
# debug            → target/debug/octopus-desktop
# profiling        → target/profiling/octopus-desktop
if [[ -z "$RELEASE" ]]; then
  BIN_PATH="$REPO_ROOT/target/debug/octopus-desktop"
elif [[ "$RELEASE" == "--release" ]]; then
  BIN_PATH="$REPO_ROOT/target/release/octopus-desktop"
elif [[ "$RELEASE" == "--profile optimize" ]]; then
  BIN_PATH="$REPO_ROOT/target/optimize/octopus-desktop"
elif [[ "$RELEASE" == "--profile profiling" ]]; then
  BIN_PATH="$REPO_ROOT/target/profiling/octopus-desktop"
else
  BIN_PATH="$REPO_ROOT/target/optimize/octopus-desktop"
fi

# ════════════════════════════════════════════════════════════════════
# 阶段 1：编译打包（清缓存 + 构建前端 + cargo build；不杀进程，旧应用可继续运行）
# ════════════════════════════════════════════════════════════════════
if [[ "$RUN_STAGE1" == true ]]; then
  echo "┌─ 阶段 1/2：编译打包 ─────────────────────────────────────────"

  # 1.1 清 WebView 缓存（identifier=com.octopus.desktop）
  rm -rf ~/Library/WebKit/com.octopus.desktop
  rm -rf ~/Library/Caches/com.octopus.desktop
  rm -rf ~/Library/HTTPStorages/com.octopus.desktop

  # 1.2 构建前端（React → dist/）
  #     cargo run 不走 Tauri CLI，不会触发 beforeBuildCommand，必须手动 build。
  cd "$DESKTOP_DIR"
  rm -rf ./dist
  cd ./frontend
  # 判断项目本地是否有 tsc
  if [ ! -f "./node_modules/.bin/tsc" ]; then
    echo "未检测到本地typescript，开始安装..."
    npm install typescript --save-dev
  fi
  npm run build

  # 1.4 切到 desktop crate 目录：frontendDist:"dist" 相对 tauri.conf.json 所在目录，
  #     运行时按 CWD 解析，CWD = crates/desktop 最保险。
  cd "$DESKTOP_DIR"

  # 1.5 cargo build（不 run——阶段 2 单独执行产物）
  # 必须启用 cloud feature：云端引擎（Aliyun/ByteDance/Tencent/Baidu）的流式识别依赖此 feature，
  # 不启用时云端引擎无法使用（is_cloud_engine / DispatchEngine 均 cfg gated）。
  # 默认用 --profile optimize（带 LTO + strip + codegen-units=1，生产级 binary）。
  # --debug：debug build 快链接 + devtools。
  # --no-lto：纯 release（无 LTO），开发期迭代最快。
  # --profiling：profiling profile（带符号，samply/xctrace 用）。
  #
  # ⚠️ custom-protocol feature（2026-07-20）：tauri 用 cfg(dev) = !custom-protocol 决定走 devUrl
  # 还是 frontendDist。debug build 想走 vite HMR（devUrl），生产 build 必须启用 custom-protocol
  # 才会走嵌入 dist（否则 WebView 找不到 localhost:1420 崩溃）。
  FEATURES="embedded cloud"
  if [[ "$RELEASE" != "" ]]; then
    FEATURES="$FEATURES custom-protocol"
  fi
  cargo build ${RELEASE} -p octopus-desktop --features "$FEATURES"

  echo "└─ 阶段 1/2 完成：$BIN_PATH"
  echo ""
fi

# ════════════════════════════════════════════════════════════════════
# 阶段 2：运行（执行阶段 1 产出的可执行包）
# ════════════════════════════════════════════════════════════════════
if [[ "$RUN_STAGE2" == true ]]; then
  echo "┌─ 阶段 2/2：启动应用 ─────────────────────────────────────────"
  # 2.1 杀进程并等待真正退出（避免退出时把缓存写回 / 占用文件导致 rm 失败）
  pkill -f octopus-desktop 2>/dev/null || true
  sleep 1
  pkill -9 -f octopus-desktop 2>/dev/null || true   # 强杀残留
  sleep 0.5

  if [[ ! -x "$BIN_PATH" ]]; then
    echo "✗ 未找到可执行包：$BIN_PATH" >&2
    echo "  请先运行 ./run-octopus.sh 或 ./run-octopus.sh --not-run 编译。" >&2
    exit 1
  fi

  # desktop crate 目录：frontendDist:"dist" 按 CWD 解析，CWD = crates/desktop 最保险。
  cd "$DESKTOP_DIR"

  # --debug 模式：能看到 panic 栈 + 自动开 devtools 排查前端
  RUST_BACKTRACE=full RUST_LIB_BACKTRACE=1 "$BIN_PATH"

  echo "└─ 阶段 2/2 结束"
fi
