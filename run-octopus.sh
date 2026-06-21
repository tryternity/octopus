#!/usr/bin/env bash
# 启动 octopus-desktop（release）+ 清 WKWebView 缓存，保证前端最新。
set -euo pipefail

# 1. 杀进程并等待真正退出（避免退出时把缓存写回 / 占用文件导致 rm 失败）
pkill -f octopus-desktop 2>/dev/null || true
sleep 1
pkill -9 -f octopus-desktop 2>/dev/null || true   # 强杀残留
sleep 0.5

# 2. 清 WebView 缓存（identifier=com.octopus.desktop）
rm -rf ~/Library/WebKit/com.octopus.desktop
rm -rf ~/Library/Caches/com.octopus.desktop
rm -rf ~/Library/HTTPStorages/com.octopus.desktop

# 3. 切到 desktop crate 目录：frontendDist:"dist" 相对 tauri.conf.json 所在目录，
#    运行时按 CWD 解析，CWD = crates/desktop 最保险。
cd "$(dirname "$0")/crates/desktop"

# 4. 一步到位编译 + 运行（release，省掉重复编译）
# cr cargo build --release -p octopus-desktop   # 平时开发，快编
# cargo build --release -p octopus-desktop      # 打包，走 Cargo.toml 体积优化（无 cr 前缀）
# 必须启用 aliyun feature：云端引擎（Aliyun Fun-ASR）的流式识别依赖此 feature，
# 不启用时 aliyun 引擎无法使用（is_cloud_engine / DispatchEngine 均 cfg gated）。
CARGO_PROFILE_RELEASE_LTO=false \
CARGO_PROFILE_RELEASE_STRIP=false \
CARGO_PROFILE_RELEASE_CODEGEN_UNITS=16 \
cargo run --release -p octopus-desktop --features "embedded aliyun"
# 注意：去掉 --release，debug 模式能打出 panic 栈
#RUST_BACKTRACE=full RUST_LIB_BACKTRACE=1 cargo run --features "embedded aliyun"