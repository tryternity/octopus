# macOS DMG 打包实施计划

**日期**：2026-07-23
**分支**：`feat/package-macos-dmg`（worktree `.worktrees/package-macos-dmg`）
**目标**：产出未签名 `.dmg`（自用/内测），feature = `embedded,cloud,vault,custom-protocol`

## 背景

项目此前一直是「裸二进制 cargo run」运行（见 `docs/architecture.md` L257/L362），无 `.app` bundle、无 `.dmg`、无打包脚本。本任务首次建立 macOS 打包链路。

## 任务分解与实施记录

### Task 0 — worktree + 分支 ✅
```
git worktree add -b feat/package-macos-dmg .worktrees/package-macos-dmg main
```
坑：首次 `git worktree add` 时 shell CWD 停留在另一 worktree 内，创建到嵌套位置；修正后从仓库根重新创建。

### Task 1 — 安装 tauri-cli ^2 ✅
```
cargo install tauri-cli --version "^2" --locked
```
版本：`tauri-cli 2.11.4`。

### Task 2 — 生成 macOS 图标集 ✅
```
cd crates/desktop && cargo tauri icon icons/icon.png
```
源 `icon.png`（512×512）→ 生成 `icon.icns`（248KB，含全尺寸）+ macOS PNG（32~1024）+ `icon.ico`（Windows）+ `android/`、`ios/` 全套。
`tauri.conf.json` `bundle.icon` 改为 `["icons/icon.icns", "icons/icon.png"]`。

### Task 3 — `seeds_dir()` 增强 .app bundle 路径解析 ✅
**文件**：`crates/infra/src/seeds.rs:15-44`

原 `seeds_dir()` 两路解析（dev / 裸二进制）无法在 Tauri `.app` bundle 内找到 seeds（exe 在 `Contents/MacOS/`，resources 在 `Contents/Resources/`）。新增第三路：

```rust
// Tauri .app bundle——exe 在 Contents/MacOS/，resources 在 Contents/Resources/
// parent=MacOS → parent.parent()=Contents → join Resources/seeds
if let Some(contents) = parent.parent() {
    let app_bundle = contents.join("Resources").join("seeds");
    if app_bundle.exists() {
        return app_bundle;
    }
}
```

**零回归**：dev 模式第一条 `$CARGO_MANIFEST_DIR/seeds` 命中，新代码不触发。20 个 seeds 相关测试全过。

### Task 4 — `tauri.conf.json` bundle 配置 ✅
**文件**：`crates/desktop/tauri.conf.json`

三处变更：
1. `bundle.icon` 纳入 `icon.icns`
2. `bundle.resources` 用**对象形式** `{ "../infra/seeds/": "seeds/" }` 映射 seeds 到 bundle `Resources/seeds/`（保留子目录结构 `prompts/`、`agent_actions/`）
3. `bundle.macOS`：`minimumSystemVersion: "11.0"`

**坑（已解决）**：
- ❌ 数组形式 `["../infra/seeds/"]`：`..` 被 Tauri 字面编码成 `_up_`，seeds 落到 `Resources/_up_/infra/seeds/`（错误）
- ✅ 对象形式 `{ "../infra/seeds/": "seeds/" }`：key=source（可含 `../`），value=destination，正确落到 `Resources/seeds/`

### Task 4b — `beforeBuildCommand` 改 null + 脚本接管前端构建 ✅
**坑（已解决）**：Tauri 2 的 `beforeBuildCommand`（字符串形式 `"cd frontend && npm run build"`）CWD 行为不可靠——在 workspace 根执行时找不到 `frontend` 目录。改为 `null`，前端构建由 `scripts/build-macos-dmg.sh` 手动完成（与 `run-octopus.sh` 做法一致）。`beforeDevCommand` 同改 null（dev 脚本自己起 vite）。

### Task 4c — 修 main 既有 tsc 错误 ✅
**文件**：`crates/desktop/frontend/src/pages/ActionBar/index.tsx:97`

main 分支既有 unused import（`labelToIndex`，actionbar 改版遗留），主仓库因 tsc 增量缓存不报，worktree 全量 build 暴露。删该 import。

### Task 5 — 打包脚本 `scripts/build-macos-dmg.sh` ✅
**文件**：`scripts/build-macos-dmg.sh`

编排：清 dist/缓存 → 手动 build 前端 → `cargo tauri build -b app`（仅 .app）→ `hdiutil create` 打 dmg → 完整性校验。

**关键决策（dmg bundling 策略）**：
- ❌ 不用 Tauri 自带 dmg bundling（`bundle_dmg.sh` / create-dmg fork）——它在当前环境失败（Finder AppleScript 美化步骤报错，且 Tauri 吞掉 stderr 难诊断）
- ✅ `cargo tauri build -b app` 只生成 `.app`，再用 macOS 原生 `hdiutil create -format UDBZ` 打 dmg（bzip2 压缩）

用法：
```
./scripts/build-macos-dmg.sh              # 默认 optimize profile（LTO+strip，生产级）
./scripts/build-macos-dmg.sh --no-lto     # release profile（无 LTO，调试打包流程用）
./scripts/build-macos-dmg.sh --open       # 构建完冒烟测试
```

### Task 6 — 构建 + 验证 ✅

**no-lto 版（流程验证）**：
- `.app` 101M + `.dmg` 40M
- CFBundleIdentifier = `com.octopus.desktop` ✅
- seeds resource 映射 5 个文件 ✅
- 冒烟测试：进程存活、窗口创建、`~/.octopus/logs/` 无 seeds 报错 ✅

**optimize 版（生产构建）**：进行中（LTO 全量编译 5-8 分钟）。

**Tauri profile 痛点**：`cargo tauri build` 无原生 `--profile`，通过 `--` 透传。bundler 默认查 `target/release/`，非 release profile 需较新 tauri-cli（2.11.4 已支持跟随 cargo profile 定位 binary）。GitHub #15019 跟踪此问题。

## 风险与降级
- **optimize profile bundler 找不到 binary** → 回退 `--no-lto`
- **seeds 映射失败** → app 仍可运行（seeds 缺失为非致命降级，仅少默认 prompt/provider/PPT 菜单）

## 不做（明确排除）
- ❌ Apple 代码签名 + 公证（未签名内测版）
- ❌ Universal Binary（仅 arm64）
- ❌ 自动更新（Tauri updater）
- ❌ 修改 ASR / 业务逻辑
- ❌ push 到 main（等用户明确放行）
