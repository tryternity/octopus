#!/usr/bin/env bash
# 启动 octopus-desktop + 清 WKWebView 缓存，保证前端最新。
# 用法：
#   ./run-octopus.sh              默认 --profile optimize（带 LTO/strip，生产级 binary）
#   ./run-octopus.sh --debug      debug build（快链接 + devtools）
#   ./run-octopus.sh --no-lto     --release（无 LTO，开发期迭代最快）
#   ./run-octopus.sh --profiling  --profile profiling（带符号，samply/xctrace 用）
set -euo pipefail

RELEASE="--profile optimize"
if [[ "${1:-}" == "--debug" ]]; then
  RELEASE=""
fi
if [[ "${1:-}" == "--profiling" ]]; then
  RELEASE="--profile profiling"
fi
if [[ "${1:-}" == "--no-lto" ]]; then
  RELEASE="--release"
fi

# 1. 杀进程并等待真正退出（避免退出时把缓存写回 / 占用文件导致 rm 失败）
pkill -f octopus-desktop 2>/dev/null || true
sleep 1
pkill -9 -f octopus-desktop 2>/dev/null || true   # 强杀残留
sleep 0.5

# 2. 清 WebView 缓存（identifier=com.octopus.desktop）
rm -rf ~/Library/WebKit/com.octopus.desktop
rm -rf ~/Library/Caches/com.octopus.desktop
rm -rf ~/Library/HTTPStorages/com.octopus.desktop

# 3. 构建前端（React → dist/）
#    cargo run 不走 Tauri CLI，不会触发 beforeBuildCommand，必须手动 build。
cd "$(dirname "$0")/crates/desktop/"
rm -rf ./dist
cd ./frontend
# 判断项目本地是否有 tsc
if [ ! -f "./node_modules/.bin/tsc" ]; then
  echo "未检测到本地typescript，开始安装..."
  npm install typescript --save-dev
fi
npm run build

# 4. 切到 desktop crate 目录：frontendDist:"dist" 相对 tauri.conf.json 所在目录，
#    运行时按 CWD 解析，CWD = crates/desktop 最保险。
cd "../"

# 5. 编译 + 运行（--debug 模式：能看到 panic 栈 + 自动开 devtools 排查前端）
# 必须启用 cloud feature：云端引擎（Aliyun/ByteDance/Tencent/Baidu）的流式识别依赖此 feature，
# 不启用时云端引擎无法使用（is_cloud_engine / DispatchEngine 均 cfg gated）。
# 默认用 --profile optimize（带 LTO + strip + codegen-units=1，生产级 binary）。
# --debug：debug build 快链接 + devtools。
# --no-lto：纯 release（无 LTO），开发期迭代最快。
# --profiling：profiling profile（带符号，samply/xctrace 用）。
RUST_BACKTRACE=full RUST_LIB_BACKTRACE=1 cargo run ${RELEASE} -p octopus-desktop --features "embedded cloud"
