#!/usr/bin/env bash
# 构建 macOS 录屏 helper。
#
# 用法：
#   ./scripts/build-macos-helper.sh                默认 release + host 架构（开发期迭代）
#   ./scripts/build-macos-helper.sh --debug        debug 模式
#   ./scripts/build-macos-helper.sh --arch arm64   单架构
#   ./scripts/build-macos-helper.sh --arch x86_64  单架构
#
# universal binary（DMG 打包用）需显式传两次 --arch：
#   ./scripts/build-macos-helper.sh --arch arm64 --arch x86_64
#
# 产物：crates/desktop/binaries/octopus-sck-helper（拷贝自 .build/release/）
#
# 前置：Xcode + Swift 5.9+（macOS 开发机默认有）

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
HELPER_DIR="$REPO_ROOT/crates/record/native/macos"
OUTPUT_DIR="$REPO_ROOT/crates/desktop/binaries"

CONFIG="release"
# 累积多个 --arch（空数组 = host 架构；1 个 = 单架构；2 个 = universal）
ARCH_FLAGS=()

# 用 while + shift 解析，因为 --arch 需要取下一个参数
while [[ $# -gt 0 ]]; do
  case "$1" in
    --debug)
      CONFIG="debug"
      shift
      ;;
    --arch)
      if [[ $# -lt 2 ]]; then
        echo "[build-helper] ❌ --arch 需要一个参数（arm64 / x86_64）" >&2
        exit 2
      fi
      ARCH_FLAGS+=("--arch" "$2")
      shift 2
      ;;
    -h|--help)
      sed -n '2,14p' "$0"
      exit 0
      ;;
    *)
      echo "[build-helper] 未知参数: $1" >&2
      exit 2
      ;;
  esac
done

if ! command -v swift >/dev/null 2>&1; then
  echo "[build-helper] ❌ swift 命令未找到。请装 Xcode Command Line Tools：xcode-select --install" >&2
  exit 1
fi

if [[ ${#ARCH_FLAGS[@]} -eq 0 ]]; then
  ARCH_DESC="host"
elif [[ ${#ARCH_FLAGS[@]} -eq 2 ]]; then
  ARCH_DESC="${ARCH_FLAGS[1]}"  # 一个 --arch X
else
  ARCH_DESC="universal (${ARCH_FLAGS[1]} + ${ARCH_FLAGS[3]})"
fi
echo "[build-helper] 编译 octopus-sck-helper ($CONFIG / $ARCH_DESC)..."
cd "$HELPER_DIR"
# set -u 下空数组展开会报 unbound（macOS bash 3.2），手动区分空/非空
if [[ ${#ARCH_FLAGS[@]} -eq 0 ]]; then
  swift build -c "$CONFIG"
else
  swift build -c "$CONFIG" "${ARCH_FLAGS[@]}"
fi

# 拷贝产物到 desktop/binaries/（Tauri resources 配置指向这里）
BINARY_NAME="octopus-sck-helper"
SRC_BIN="$HELPER_DIR/.build/$CONFIG/$BINARY_NAME"
DST_BIN="$OUTPUT_DIR/$BINARY_NAME"

mkdir -p "$OUTPUT_DIR"
if [[ ! -f "$SRC_BIN" ]]; then
  echo "[build-helper] ❌ 编译产物未找到: $SRC_BIN" >&2
  exit 1
fi
cp "$SRC_BIN" "$DST_BIN"
chmod +x "$DST_BIN"

echo "[build-helper] ✅ 产物：$DST_BIN"
file "$DST_BIN"
