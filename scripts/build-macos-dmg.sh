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

# ── 1b. 构建录屏 helper（macOS only，Task 15 集成）──────────────────────────
# 必须在 cargo tauri build 之前：Tauri bundler 按 tauri.conf.json resources 配置
# 把 crates/desktop/binaries/octopus-sck-helper 拷进 .app/Contents/Resources/binaries/。
# 缺失会让打包报 resources not found。
echo "[build-dmg] 构建录屏 helper (universal binary)..."
"$REPO_ROOT/scripts/build-macos-helper.sh" || {
  echo "[build-dmg] ⚠️ helper 编译失败，跳过录屏 helper 打包" >&2
  echo "[build-dmg] （录屏功能不可用，其他功能不受影响）" >&2
}
# 验证产物存在——Tauri resources 配置指向 binaries/octopus-sck-helper
if [[ ! -f "$DESKTOP_DIR/binaries/octopus-sck-helper" ]]; then
  echo "[build-dmg] ⚠️ helper 产物未找到：$DESKTOP_DIR/binaries/octopus-sck-helper" >&2
  echo "[build-dmg]    DMG 里的录屏功能将不可用（其他功能正常）" >&2
fi

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

# ── 2b. ad-hoc 签名固定 identifier（稳定 TCC code identity）──────────────────
# 问题：未签名 .app 的 identifier 是 linker 生成的 hash 后缀（如 octopus_desktop-348b5fbda5d6f1ef），
# 每次重新链接 hash 变 → macOS TCC（屏幕录制/麦克风/辅助功能权限）认为是新 app →
# 旧授权失效但条目残留（灰掉无法重勾）= 用户死局。
# 解法：codesign -s - ad-hoc 签名 + 固定 identifier（com.octopus.desktop，与 tauri.conf.json 一致）。
# ad-hoc 不需要 Apple Developer 账号，但 code identity 跨打包稳定，TCC 授权持久。
# --force 覆盖 linker-signed；--deep 签名 bundle 内所有辅助二进制（helper）。
echo "[build-dmg] ad-hoc 签名（固定 identifier=com.octopus.desktop）..."
codesign --force --deep --sign - --identifier com.octopus.desktop "$APP_DIR" 2>&1 | tail -3
# 验证签名
SIGN_ID=$(codesign -dv --verbose=1 "$APP_DIR" 2>&1 | grep '^Identifier=' | head -1)
echo "[build-dmg] 签名: $SIGN_ID"
if [[ "$SIGN_ID" != "Identifier=com.octopus.desktop" ]]; then
  echo "[build-dmg] ⚠️  签名 identifier 异常: $SIGN_ID（TCC 权限可能不稳定）" >&2
fi

# dmg 文件名：octopus_<version>_<arch>.dmg（与 tauri 原生命名一致）
VERSION=$(defaults read "$APP_DIR/Contents/Info" CFBundleVersion 2>/dev/null || echo "0.1.0")
ARCH=$(file "$APP_DIR/Contents/MacOS/"* | grep -oE 'arm64|x86_64' | head -1 || echo "$(uname -m)")
DMG_PATH="$DMG_DIR/octopus_${VERSION}_${ARCH}.dmg"
mkdir -p "$DMG_DIR"
rm -f "$DMG_PATH"

# 构造 dmg staging 目录：.app + Applications 软链接 + .DS_Store（拖拽安装体验）
# 双击 dmg 打开后，Finder 按 .DS_Store 布局显示：octopus.app 在左、Applications 在右，
# 用户把 app 拖到 Applications 即完成安装。
STAGING_DIR="$DMG_DIR/staging"
rm -rf "$STAGING_DIR"
mkdir -p "$STAGING_DIR"
# 拷贝 .app（不能用软链接，否则 dmg 内符号链接断裂）
cp -R "$APP_DIR" "$STAGING_DIR/"
# Applications 软链接——Finder 会显示成带箭头的文件夹图标
ln -s /Applications "$STAGING_DIR/Applications"

# 写 .DS_Store：窗口尺寸 + icon view + 图标位置（app 左 / Applications 右）。
# 用 ds_store python 库写 .DS_Store，比 AppleScript Finder 自动化可靠得多
# （Finder 的 disk/folder AppleScript 语义在多版本下脆弱，常报 -1728/-10010）。
# 三类记录：
#   bwsp — browser window properties（窗口尺寸/位置，去掉 toolbar/sidebar/pathbar）
#   icvp — icon view properties（icon 大小、排列方式、背景色）
#   Iloc — 每个图标的 (x, y) 位置
# 不写 bwsp/icvp 时 Finder 用默认大窗口，图标被挤到下方留大片空白。
echo "[build-dmg] 写 .DS_Store 布局（紧凑窗口 480×160，app 左 / Applications 右）..."
python3 - "$STAGING_DIR/.DS_Store" <<'PYEOF' || echo "[build-dmg] ⚠️  .DS_Store 写入失败（不影响 dmg 功能，仅布局默认）"
import sys
import ds_store
path = sys.argv[1]
bwsp = {
    'ShowStatusBar': False,
    'WindowBounds': '{{200, 200}, {480, 160}}',
    'ContainerShowSidebar': False,
    'PreviewPaneVisibility': False,
    'SidebarWidth': 0,
    'ShowToolbar': False,
    'ShowPathbar': False,
    'ShowTabView': False,
}
icvp = {
    'gridOffsetX': 0.0, 'textSize': 12.0, 'iconSize': 96.0,
    'gridSpacing': 100.0, 'scrollPositionX': 0.0, 'showItemInfo': False,
    'labelOnBottom': True, 'gridOffsetY': 0.0, 'scrollPositionY': 0.0,
    'arrangeBy': 'none', 'showIconPreview': True,
    'backgroundColorBlue': 1.0, 'backgroundType': 0,
    'backgroundColorGreen': 1.0, 'backgroundColorRed': 1.0,
}
with ds_store.DSStore.open(path, 'w+') as d:
    d['.']['bwsp'] = bwsp
    d['.']['icvp'] = icvp
    d['.']['vSrn'] = ('long', 1)
    d['octopus.app']['Iloc'] = (80, 50)
    d['Applications']['Iloc'] = (320, 50)
PYEOF

echo "[build-dmg] hdiutil 打 dmg（UDBZ bzip2 压缩，含 Applications 拖拽安装）..."
# 两步：先打 read-write (UDRW) → 再转只读压缩 (UDBZ)。
# 直接 UDBZ 打 staging 也行，但两步法便于未来插入 Finder 美化（背景图等）。
# -fs HFS+：兼容性好；-volname：挂载后卷名；-srcfolder 指向 staging
RW_DMG="$DMG_DIR/octopus-rw.dmg"
rm -f "$RW_DMG"
hdiutil create \
  -volname "octopus" \
  -srcfolder "$STAGING_DIR" \
  -fs HFS+ \
  -format UDRW \
  -ov \
  "$RW_DMG" 2>&1 | tail -1
hdiutil convert "$RW_DMG" -format UDBZ -ov -o "$DMG_PATH" 2>&1 | tail -1
rm -f "$RW_DMG"

# 清理 staging（.app 副本已打进 dmg，不再需要）
rm -rf "$STAGING_DIR"

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
