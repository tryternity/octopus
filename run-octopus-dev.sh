#!/usr/bin/env bash
# dev 模式启动：vite HMR + cargo debug build。
# 改前端 → WebView 秒级热重载，不重编 Rust。
#
# 用法：./run-octopus-dev.sh
#
# 与 run-octopus.sh 区别：
#   - cargo run 不带 --release（debug build，debug_assertions=true）
#   - 不 build dist / 不清 WebView 缓存（从 http://localhost:1420 加载）
#   - 后台启动 vite dev server，cargo 退出时自动 kill
#   - tauri.conf.json 的 devUrl 在 debug build 下生效，所有 WebviewUrl::App(...)
#     自动映射到 http://localhost:1420/...（query string 保留）
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "$0")" && pwd)"
FRONTEND_DIR="$REPO_ROOT/crates/desktop/frontend"
DESKTOP_DIR="$REPO_ROOT/crates/desktop"
DEV_URL="http://localhost:1420"
VITE_PID=""

# 退出时清理 vite
cleanup() {
  if [[ -n "$VITE_PID" ]] && kill -0 "$VITE_PID" 2>/dev/null; then
    echo "[dev] stopping vite (pid=$VITE_PID)..."
    kill "$VITE_PID" 2>/dev/null || true
    wait "$VITE_PID" 2>/dev/null || true
  fi
}
trap cleanup EXIT INT TERM

# 1. 杀旧 octopus-desktop 进程（避免 single-instance plugin 拒启）
pkill -f octopus-desktop 2>/dev/null || true
sleep 1
pkill -9 -f octopus-desktop 2>/dev/null || true

# 2. 启动 vite dev server（后台），等端口可访问
#    bun 启动比 npm 快很多；node_modules/.bin/vite 保证用本地版本。
cd "$FRONTEND_DIR"
if [[ -x "./node_modules/.bin/vite" ]]; then
  "./node_modules/.bin/vite" --host=localhost > /tmp/octopus-vite.log 2>&1 &
else
  # 兜底用 bun run dev（会走 package.json scripts.dev）
  bun run dev > /tmp/octopus-vite.log 2>&1 &
fi
VITE_PID=$!
echo "[dev] vite started (pid=$VITE_PID), log: /tmp/octopus-vite.log"

# 3. 等待 vite 监听端口（最多 30s）
echo "[dev] waiting for $DEV_URL ..."
for i in $(seq 1 60); do
  if curl -sS -o /dev/null --max-time 1 "$DEV_URL" 2>/dev/null; then
    echo "[dev] vite ready after ${i}x0.5s"
    break
  fi
  if ! kill -0 "$VITE_PID" 2>/dev/null; then
    echo "[dev] vite died during startup, last log:"
    cat /tmp/octopus-vite.log
    exit 1
  fi
  sleep 0.5
done

if ! curl -sS -o /dev/null --max-time 1 "$DEV_URL" 2>/dev/null; then
  echo "[dev] vite did not come up at $DEV_URL within 30s, last log:"
  cat /tmp/octopus-vite.log
  exit 1
fi

# 4. cargo run（debug profile，features 与 run-octopus.sh 对齐）
#    RUST_BACKTRACE 保留；CWD 设到 desktop crate（frontendDist 相对路径解析）
cd "$DESKTOP_DIR"
RUST_BACKTRACE=full RUST_LIB_BACKTRACE=1 \
  cargo run -p octopus-desktop --features "embedded cloud"
