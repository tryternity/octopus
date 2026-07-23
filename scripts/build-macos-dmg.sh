#!/usr/bin/env bash
# 打包 macOS .dmg（未签名，自用/内测版）。
#
# 用法：
#   ./scripts/build-macos-dmg.sh              默认 --profile optimize（LTO+strip，生产级 binary，体积小）
#   ./scripts/build-macos-dmg.sh --no-lto     纯 release profile（无 LTO，链接快，调试打包流程用）
#   ./scripts/build-macos-dmg.sh --open       构建完 open .app 冒烟测试
#
# 产物路径：
#   target/<profile>/bundle/dmg/octopus_0.1.0_<arch>.dmg
#   target/<profile>/bundle/macos/octopus.app
#
# 前置：
#   1. cargo-tauri CLI 已安装（cargo install tauri-cli --version "^2"）
#   2. icons/icon.icns 已生成（cargo tauri icon icons/icon.png）
#
# feature 组合：embedded,cloud,vault,custom-protocol
#   - embedded : 本地 ASR（octopus-asr-local）
#   - cloud    : 云端 ASR（Aliyun/ByteDance/Tencent/Baidu WSS 流式）
#   - vault    : 密码保险库
#   - custom-protocol : 生产 build 必须启用，让 tauri 走 frontendDist（嵌入 dist）而非 devUrl
#     （cfg(dev) = !has_feature("custom-protocol")，跟 release/debug profile 无关）
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
DESKTOP_DIR="$REPO_ROOT/crates/desktop"

# ── 参数解析 ────────────────────────────────────────────────────────────────
PROFILE="optimize"
SMOKE_OPEN=0
for arg in "$@"; do
  case "$arg" in
    --no-lto)  PROFILE="release" ;;
    --open)    SMOKE_OPEN=1 ;;
    -h|--help)
      sed -n '2,18p' "$0"
      exit 0
      ;;
    *)
      echo "[build-dmg] 未知参数: $arg" >&2
      exit 2
      ;;
  esac
done

FEATURES="embedded,cloud,vault,custom-protocol"

echo "[build-dmg] profile=$PROFILE  features=$FEATURES"
echo "[build-dmg] repo=$REPO_ROOT"

# ── 前置检查 ────────────────────────────────────────────────────────────────
if ! cargo tauri --version >/dev/null 2>&1; then
  echo "[build-dmg] ❌ cargo-tauri CLI 未安装。请先运行：" >&2
  echo "    cargo install tauri-cli --version \"^2\"" >&2
  exit 1
fi
if [[ ! -f "$DESKTOP_DIR/icons/icon.icns" ]]; then
  echo "[build-dmg] ❌ 缺 icons/icon.icns。请先运行：" >&2
  echo "    cd $DESKTOP_DIR && cargo tauri icon icons/icon.png" >&2
  exit 1
fi

# ── 1. 清前端 dist + WebView 缓存 ───────────────────────────────────────────
echo "[build-dmg] 清 dist + WebView 缓存..."
rm -rf "$DESKTOP_DIR/dist"
rm -rf ~/Library/WebKit/com.octopus.desktop
rm -rf ~/Library/Caches/com.octopus.desktop
rm -rf ~/Library/HTTPStorages/com.octopus.desktop

# ── 1b. 手动 build 前端 ─────────────────────────────────────────────────────
# tauri.conf.json beforeBuildCommand=null（CWD/shell 解析在 tauri 2 不可靠，
# 且 worktree checkout 不含 node_modules）。脚本自己 build dist，与 run-octopus.sh
# 做法一致——frontendDist:"dist" 相对 tauri.conf.json（crates/desktop/）解析。
echo "[build-dmg] build 前端 dist..."
FRONTEND_DIR="$DESKTOP_DIR/frontend"
if [[ ! -d "$FRONTEND_DIR/node_modules" ]]; then
  echo "[build-dmg] node_modules 缺失，npm install..."
  (cd "$FRONTEND_DIR" && npm install)
fi
(cd "$FRONTEND_DIR" && npm run build)

# ── 2. cargo tauri build（仅 .app，dmg 用 hdiutil 自己打）───────────────────
# tauri build 原生 -f 传 features；profile 通过 -- 透传给底层 cargo。
# 产物在 target/<profile>/：release profile → target/release/，optimize → target/optimize/。
# ⚠️ Tauri bundler 默认查 target/release/——非 release profile 需较新 tauri-cli（2.x 已支持
#    跟随 cargo profile 定位 binary，但仍是个历史痛点，见 GitHub #15019）。
#    若 optimize profile 报 binary not found，回退 --no-lto 用默认 release。
#
# ⚠️ 不用 tauri 的 dmg bundling（bundle_dmg.sh / create-dmg）——它在部分环境
#    （Finder AppleScript 美化步骤）会失败并吞掉 stderr。改为 -b app 只生成 .app，
#    然后用 macOS 原生 hdiutil 打 dmg（UDBZ bzip2 压缩，~40MB）。
cd "$DESKTOP_DIR"
echo "[build-dmg] 开始 cargo tauri build（仅 .app，预计 3~8 分钟）..."
cargo tauri build \
  -b app \
  -f "$FEATURES" \
  -- --profile "$PROFILE"

# ── 3. 定位 .app + 用 hdiutil 打 dmg ────────────────────────────────────────
APP_DIR="$REPO_ROOT/target/$PROFILE/bundle/macos/octopus.app"
DMG_DIR="$REPO_ROOT/target/$PROFILE/bundle/dmg"

if [[ ! -d "$APP_DIR" ]]; then
  echo "[build-dmg] ❌ 未找到 .app bundle: $APP_DIR" >&2
  exit 1
fi

# dmg 文件名：octopus_<version>_<arch>.dmg（与 tauri 原生命名一致）
VERSION=$(defaults read "$APP_DIR/Contents/Info" CFBundleVersion 2>/dev/null || echo "0.1.0")
ARCH=$(file "$APP_DIR/Contents/MacOS/"* | grep -oE 'arm64|x86_64' | head -1 || echo "$(uname -m)")
DMG_PATH="$DMG_DIR/octopus_${VERSION}_${ARCH}.dmg"
mkdir -p "$DMG_DIR"
rm -f "$DMG_PATH"

echo "[build-dmg] hdiutil 打 dmg（UDBZ bzip2 压缩）..."
# -fs HFS+：兼容性好；-format UDBZ：bzip2 压缩（体积小）；-volname：挂载后卷名
hdiutil create \
  -volname "octopus" \
  -srcfolder "$APP_DIR" \
  -fs HFS+ \
  -format UDBZ \
  -ov \
  "$DMG_PATH" 2>&1 | tail -3

if [[ ! -f "$DMG_PATH" ]]; then
  echo "[build-dmg] ❌ dmg 生成失败: $DMG_PATH" >&2
  exit 1
fi

echo
echo "════════════════════════════════════════════════════════════════"
echo "[build-dmg] ✅ 打包完成"
echo "  .app : $APP_DIR  ($(du -sh "$APP_DIR" | cut -f1))"
echo "  .dmg : $DMG_PATH  ($(du -h "$DMG_PATH" | cut -f1))"
echo "════════════════════════════════════════════════════════════════"

# ── 4. bundle 完整性校验 ────────────────────────────────────────────────────
echo
echo "[build-dmg] bundle 校验："

# 4a. CFBundleIdentifier
BUNDLE_ID=$(defaults read "$APP_DIR/Contents/Info" CFBundleIdentifier 2>/dev/null || echo "")
if [[ "$BUNDLE_ID" == "com.octopus.desktop" ]]; then
  echo "  ✅ CFBundleIdentifier = $BUNDLE_ID"
else
  echo "  ❌ CFBundleIdentifier 异常: '$BUNDLE_ID'（预期 com.octopus.desktop）" >&2
fi

# 4b. seeds resource 映射（seeds_dir() 在 .app 内走 Contents/Resources/seeds）
SEEDS_DIR_IN_APP="$APP_DIR/Contents/Resources/seeds"
if [[ -d "$SEEDS_DIR_IN_APP" && -f "$SEEDS_DIR_IN_APP/llm_providers.json" ]]; then
  echo "  ✅ seeds resource 已映射 ($(find "$SEEDS_DIR_IN_APP" -type f | wc -l | tr -d ' ') 个文件)"
else
  echo "  ⚠️  seeds resource 未找到（app 仍可运行，但首次启动无默认润色 prompt / LLM provider / PPT agent 菜单）" >&2
  echo "      预期路径: $SEEDS_DIR_IN_APP" >&2
fi

# 4c. binary 架构（MacOS/ 下只有一个可执行文件，用 glob 匹配避免硬编码 binary 名）
BIN_ARCH=$(file "$APP_DIR/Contents/MacOS/"* 2>/dev/null | grep -oE 'arm64|x86_64' | head -1 || echo "unknown")
echo "  ✅ binary 架构: $BIN_ARCH"

# ── 5. 可选冒烟测试 ─────────────────────────────────────────────────────────
if [[ "$SMOKE_OPEN" == "1" ]]; then
  echo
  echo "[build-dmg] 启动 .app 冒烟测试（20s 后自动退出）..."
  open "$APP_DIR"
  sleep 20
  pkill -f "octopus.app/Contents/MacOS/octopus" 2>/dev/null || true
  echo "[build-dmg] 冒烟完成——检查 ~/Library/Logs/octopus/ 或 ~/.octopus/logs/ 是否有异常"
fi
