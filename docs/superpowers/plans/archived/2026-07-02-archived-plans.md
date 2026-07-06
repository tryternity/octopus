# 已归档实施计划（2026-06-29 ~ 2026-07-02）

> 以下功能均已实现并合并 main（或已下线归档）。本文件由 9 份原独立 plan 文件合并归档，原文件已删除。
> plan↔spec 旧路径交叉引用随归档失效，按主题在 specs/2026-07-02-archived-specs.md 内查同名章节。

## 目录

- 2026-06-29-scroll-screenshot.md
- 2026-06-30-compact-editor.md
- 2026-06-30-notepad.md
- 2026-07-01-image-preview.md
- 2026-07-01-pin-screenshot.md
- 2026-07-02-capx-canvas-anchored.md
- 2026-07-02-capx-ncc-sobel.md
- 2026-07-02-capx-stitch-robustness.md
- 2026-07-02-notepad-type-migration.md


---

## 来自原文件 `2026-06-29-scroll-screenshot.md`

# 滚动截屏实施计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development or superpowers:executing-plans.

**Goal:** 用户框选后点「滚动截图」→ 手动滚动 → 33fps 截帧 + 1D FFT 相位相关亚像素拼接 → 停止后长图入库

**Architecture:** 透明覆盖层 + 焦点让出 + CGWindowList 排除 + 1D FFT 相位相关拼接

**Tech Stack:** Rust + rustfft + imageproc + objc2/objc2-app-kit + core-graphics + core-foundation + Tauri 2 + React

**Spec:** `docs/superpowers/specs/2026-06-29-scroll-screenshot-design.md`

---

## Task 1: FFT 相位相关拼接引擎 ✅

- [x] Cargo.toml 加 `rustfft = "6.2"`
- [x] Stitcher + StitchConfig（min_scroll_px=2.0, min_confidence=0.15）
- [x] project_vertical_range：Sobel 边缘 → 每行平均边缘强度 → 1D 信号
- [x] phase_correlation_dy：FFT → 归一化互功率谱 → IFFT → 峰值 + 抛物线亚像素
- [x] process_frame：detect_sticky → 裁 canvas → FFT 求位移 → 追加底部行 → 更新参考投影
- [x] finalize：补全最后一帧 sticky footer
- [x] detect_sticky：sticky header/footer 检测

## Task 2: capx/capture.rs 区域截图 ✅

- [x] capture_region_excluding_window（CGWindowList + BGRA→RGBA）
- [x] capture_window_region（指定窗口 ID 截图）

## Task 3: 后端录制循环 ✅

- [x] start_scroll_recording / stop_scroll_recording
- [x] save_frontmost_app / activate_prev_app
- [x] target_wid 改用选区中心检测（get_window_pid_at_point），非 PREV_ACTIVE_APP
- [x] 33fps 截图循环
- [x] 30ms 鼠标监视线程（预览区域切换 ignore + 自动激活选区下方应用）
- [x] 预览：底部裁剪 + 400px 宽 + CatmullRom
- [x] 停止：先恢复鼠标 → finalize → spawn_blocking 预览 → 入库
- [x] WebP 入库

## Task 4: 前端 scrolling 模式 ✅

- [x] clearRect 挖透明孔（Canvas 只画边框）
- [x] 选区外遮罩用 DOM div（不经过 Canvas）
- [x] scrolling 模式工具栏隐藏
- [x] startScroll / stopScroll
- [x] scroll://frame 事件监听
- [x] 预览面板 HUD 风格（毛玻璃 + 脉冲 REC + 2:1 按钮 + 底部对齐选区）

## Task 5: 托盘菜单 ✅

- [x] 「截图」→「开始截图」
- [x] 去掉「停止滚动截图」菜单
- [x] 截图进行中灰掉菜单（不改文字）
- [x] 分组分隔线 + 「剪  贴  板」双半角空格对齐
- [x] 引擎信息格式简化

## Task 6: 端到端验证 ✅

- [x] Cmd+Shift+D → 框选 → 滚动截图 → 停止 → 长图入库
- [x] 拼接结果无重叠、无缺失、无模糊
- [x] 预览清晰 + 底部最新内容可见
- [x] 停止时无鼠标假死
- [x] 选区下方应用自动激活
- [x] 预览面板按钮可点击

---

## 实施偏差记录

### 偏差 1-8：早期失败方案 ❌

NSPanel 崩溃、auto 模式体验差、简单 deactivate 不穿透、always_on_top 关闭被遮挡、Occlusion Throttling、Key Window 焦点锁定。

### 偏差 9：NCC 模板匹配（三种变体全部失败）❌

1. 双模板 PLL → delta 剧烈跳动，周期性假匹配
2. 底部 strip 固定 → 整数像素累积误差 → 模糊
3. 动态模板位置 → 帧间不一致 → delta 波动

### 偏差 10：FFT 相位相关（最终方案）✅

1D FFT 相位相关 + 抛物线亚像素拟合。

### 偏差 11：FFT 实现调试 ✅

- dy 方向反了 → 修正
- 投影长度不匹配 → detect_sticky 后重算
- 首帧 sticky 重复 → 初始化时裁掉 sticky
- 最后一帧缺失 → finalize 补全

### 偏差 12：预览体验优化 ✅

- 预览模糊 → 400px CatmullRom
- 看不到最后一行 → 底部裁剪 + finalize 后 emit
- 预览底部固定 → bottom 对齐选区 + justifyContent flex-end

### 偏差 13：停止时鼠标假死 ✅

先恢复鼠标事件 → 再 finalize → spawn_blocking 预览。

### 偏差 14：选区遮罩变暗 ✅

Canvas 像素遮罩 → DOM div 遮罩（选区内不经过 Canvas）。

### 偏差 15：target_wid 黑屏 ✅

PREV_ACTIVE_APP PID → 选区中心检测（get_window_pid_at_point）。

### 偏差 16：选区下方应用自动激活 ✅

CGWindowListCopyWindowInfo + bounds 命中 → activateWithOptions（run_on_main_thread）。
跳过 kCGWindowLayer != 0（桌面壁纸等）。

### 偏差 17：工具栏停止按钮不可点击 ✅

scrolling 模式下隐藏工具栏，停止/取消按钮移到预览面板中。
预览区域 interactiveRects 传给后端监视线程。

### 偏差 18：预览面板 HUD 重新设计 ✅

毛玻璃面板 + 琥珀色脉冲 REC + 等宽数字 + 2:1 按钮 + hover 过渡。

### 偏差 19：托盘菜单设计 ✅

- 分组分隔线
- 「剪  贴  板」「记  事  本」双半角空格对齐
- 截图灰掉菜单（不改文字）
- 引擎信息格式简化
- 菜单文案带快捷键（⌘⇧A 格式，从 config 动态读取）

### 偏差 20：UI 细节优化 ✅

- 滚动截屏按钮使用 `icons/scroll.svg` 图标
- 取消按钮图标 CSS filter 染红色
- tiptap 依赖拆分为独立 chunk（消除 bundle 过大警告）
- 清理 `[window-diag]` 诊断日志

### 偏差 21：保存/复制/取消三按钮 ✅

- `stop_scroll_recording_with_mode(save/copy/cancel)` 后端处理停止模式
- 保存：入库 + 写系统剪贴板 + 弹 `blocking_save_file` 对话框（后端执行，不依赖前端窗口存活）
- 复制：入库 + 写系统剪贴板（`handle.write_image`）
- 取消：不入库，直接关窗口
- Enum 值对齐：Copy=0, Save=1, Cancel=2（`#[repr(u8)]`）

### 偏差 22：监视线程提速 ✅

- 鼠标监视线程从 30ms 降到 16ms（60fps），降低首次点击穿透概率
- macOS `setIgnoresMouseEvents` 仍有异步延迟，彻底解决需多窗口架构（暂不实施）

---

## Spec Coverage

| spec 章节 | 实现 |
|---|---|
| §1.1 透明窗口 | Task 4 |
| §1.2 焦点让出 | Task 3 |
| §1.3 坐标映射 | Task 3 |
| §1.4 自动激活选区下方应用 | Task 3 |
| §2.1 FFT 核心算法 | Task 1 |
| §2.4 Sticky 处理 | Task 1 |
| §3 录制循环 | Task 3 |
| §3.1 停止流程 | 偏差 13 |
| §4.1 滚动模式 UI | Task 4 |
| §4.2 预览面板 | Task 4 |
| §5 托盘菜单 | Task 5 |


---

## 来自原文件 `2026-06-30-compact-editor.md`

# 精简编辑器（Compact Editor）Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 新建一个纯文本精简编辑器窗口（工具栏 + textarea），编辑结果通过事件返回给调用方，并接入语音 Result、OCR、剪贴板文本条目三处。

**Architecture:** 独立 Tauri 窗口 `compact_editor_window`（原生标题栏、关窗即销毁）。后端 `compact_editor_commands.rs` 用静态 `PENDING` 暂存 `{text, request_id}`，前端 mount 时拉取。调用方生成 `requestId` → `open_compact_editor` → 监听 `compact-editor://result`（按 rid 过滤）→ 应用文本。共享命令 `set_clipboard_item_text` 供 OCR/剪贴板写回。

**Tech Stack:** Rust + Tauri 2（`WebviewWindowBuilder`、`emit`、`#[tauri::command]`）；React 19 + TypeScript + Vite 8 + Tailwind 4 + lucide-react；SQLite（rusqlite，`with_db`）。

---

## ✅ 实现状态（2026-06-30 同步）

本计划 **Task 1-6、8-11 已全部实现并提交**（git log：`b2e45c0`…`0d7a061`），后端 `cargo test` 全绿（55 passed, 0 failed）。

- **Task 1-6** ✅：store `update_content` / `set_clipboard_item_text` 命令 / `compact_editor_window` 窗口模块 / `compact_editor_commands` 命令层 + 单测 / `generate_handler!` 注册 4 命令 / CompactEditor 组件 + App 路由 + `compactEditor.ts` helper。
- **Task 7** ⚠️ **废弃**：旧方案（Result 弹独立编辑器窗）曾以 `85660ef` 实现，后因设计改为原地双模式，被 **Task 11 覆盖移除**（Result 不再 `openCompactEditor`）。checkbox 保持未勾。
- **Task 8-9** ✅：OCR 接入（移除系统 TextEdit）+ 剪贴板文本条目「编辑」按钮（`SquarePen`）。
- **Task 10** ✅：`architecture.md` 同步 + 全量后端 `cargo test` 绿。
- **Task 11** ✅：语音 Result 编辑框尺寸双模式——**CSS 伪装方案**（物理固定 720×480 + CSS 切容器尺寸 + Rust 轮询点击穿透），已替换原 setSize/setMaxSize/localStorage 方案（用户实测 setSize 在透明悬浮窗被 NSWindow 拒绝、ACL 补全后仍无效）。

**唯一剩余**：验收 e2e（手动，见文末）——CSS 伪装方案 e2e 已通过（精简态穿透到后方应用确认），其余项需用户跑 `./run-octopus.sh` 逐项确认。

**⚠️ 已修 bug（最终结论）**：Result「放大」切换窗口未变大。**完整踩坑链**：①误判 resizable（`2195c80`，未解决）；②补全 ACL 5 权限（`93f58a2`）——**真实但不足**；③ACL 补全后 setSize 仍读回 520×116（min/max 已放宽、720×480 在区间内）→ **真根因：透明+无边框悬浮窗 NSWindow 拒绝 setFrame，setSize 路径 100% 失效**。最终方案：放弃 setSize，转 CSS 伪装 + Rust 轮询点击穿透。详见文末「已修 bug」节。

---

## ⚠️ 执行环境约束（每个任务都必须遵守）

1. **worktree cwd 陷阱**：Bash 的 cwd 实测可能是**主仓库**而非本 worktree。所有 cargo/npm/git 命令必须显式指向 worktree 绝对路径：
   - **worktree 根**：`/Users/wudarui/workspace/agent/octopus/.claude/worktrees/feature-notepad`
   - cargo：`cargo test --manifest-path <worktree>/Cargo.toml -p <crate>` 或 `cargo build --manifest-path <worktree>/Cargo.toml -p octopus-desktop`
   - npm：`npm --prefix <worktree>/crates/desktop/frontend run build`
   - git：`git -C <worktree> ...`
   - Edit/Write 用绝对路径不受影响。
2. **dist 被跟踪**：`crates/desktop/dist`（36 文件）在 git 内、**非** gitignored。任何前端源码变更的任务，最后一步必须 `npm run build` 重建 dist 并把 `crates/desktop/dist` 一起提交（否则下游 `cargo run` 跑的是旧前端）。
3. **前端无单测框架**（无 vitest）。前端任务的「门」是 `npm run build`（= `tsc -b && vite build`，tsc 类型检查）。行为靠手动 e2e。
4. **分支**：当前已在 `worktree-feature-notepad` 分支（非 main），所有提交落在此分支。
5. **中文交互**：对话/注释/文档用中文。

---

## File Structure

**新建：**
- `crates/desktop/src/compact_editor_window.rs` — 窗口构建 + macOS 激活策略 + `on_compact_editor_closed`。镜像 `notepad_window.rs`。
- `crates/desktop/src/compact_editor_commands.rs` — 静态 `PENDING` + 3 个命令（`open_compact_editor` / `get_pending_compact_edit` / `close_compact_editor`）+ 单测。
- `crates/desktop/frontend/src/pages/CompactEditor/index.tsx` — 编辑器组件（textarea + 工具栏 + 事件收发）。
- `crates/desktop/frontend/src/lib/compactEditor.ts` — 调用方共享 helper（`openCompactEditor(text, onResult)`）。
- `crates/desktop/frontend/public/icons/expand-edit.svg` — Result「展开编辑」按钮图标。

**修改：**
- `crates/clipboard/src/store.rs` — 新增 `update_content`。
- `crates/desktop/src/clipboard_commands.rs` — 新增 `set_clipboard_item_text`；`ocr_image` 移除 TextEdit 调用。
- `crates/desktop/src/main.rs` — `generate_handler!` 注册 4 个新命令；`RunEvent::WindowEvent::Destroyed` 分支挂 `on_compact_editor_closed`；`mod` 声明。
- `crates/desktop/frontend/src/App.tsx` — 路由 `compact_editor_window`。
- `crates/desktop/frontend/src/components/SvgIcon.tsx` — 新增 `"expand-edit"` 图标。
- `crates/desktop/frontend/src/pages/Result/index.tsx` — 「展开编辑」按钮 + `applyResultText`。
- `crates/desktop/frontend/src/pages/Clipboard/ClipboardItem.tsx` — 文本条目「编辑」按钮 + `handleOcr` 改造。
- `docs/architecture.md` — 窗口/命令清单同步。

---

## Task 1: clipboard store 新增 `update_content`

**Files:**
- Modify: `crates/clipboard/src/store.rs`（在 `update_search_text` 后，约 L359 后新增函数）
- Test: `crates/clipboard/src/store.rs` 的 `mod tests`（约 L594+）

- [x] **Step 1: 写失败测试**

在 `crates/clipboard/src/store.rs` 的 `mod tests {` 内（参考 L605 `test_find_by_text_file_dedup` 的写法）新增：

```rust
    #[test]
    fn test_update_content() {
        // update_content 同时改写 content 与 search_text（OCR/剪贴板文本编辑后回写）。
        let conn = open_test_db();
        let id: i64 = 1700;
        insert_clipboard_item(&conn, &NewClipboardItem {
            id, item_type: ItemType::Text, content: "原始文本".into(),
            search_text: "原始文本".into(), created_at: iso_now(),
            blob_hash: None, width: None, height: None, has_thumbnail: None,
            file_count: None, is_rich: false,
        }).unwrap();

        update_content(&conn, id, "改后文本").unwrap();

        // content 经 ClipboardItem 暴露
        let item = get_item_by_id(&conn, id).unwrap().unwrap();
        assert_eq!(item.content, "改后文本");
        // search_text 不在 ClipboardItem 上，直接 SQL 断言
        let search: String = conn.query_row(
            "SELECT search_text FROM clipboard_history WHERE id = ?",
            params![id], |r| r.get(0),
        ).unwrap();
        assert_eq!(search, "改后文本");
    }
```

- [x] **Step 2: 跑测试确认失败**

Run:
```bash
cargo test --manifest-path /Users/wudarui/workspace/agent/octopus/.claude/worktrees/feature-notepad/Cargo.toml -p octopus-clipboard test_update_content
```
Expected: 编译失败，`cannot find function update_content`。

- [x] **Step 3: 实现 `update_content`**

在 `update_search_text` 函数后（L359 之后）新增：

```rust
/// 更新条目的 content 与 search_text（精简编辑器：用户编辑文本后回写剪贴板条目）。
/// 两列同写：content 是展示/粘贴源，search_text 是 FTS5 索引源，编辑后须同步以保搜索命中。
pub fn update_content(conn: &Connection, id: i64, text: &str) -> Result<()> {
    conn.execute(
        "UPDATE clipboard_history SET content = ?, search_text = ? WHERE id = ?",
        params![text, text, id],
    )?;
    Ok(())
}
```

- [x] **Step 4: 跑测试确认通过**

Run:
```bash
cargo test --manifest-path /Users/wudarui/workspace/agent/octopus/.claude/worktrees/feature-notepad/Cargo.toml -p octopus-clipboard test_update_content
```
Expected: PASS。

- [x] **Step 5: 提交**

```bash
git -C /Users/wudarui/workspace/agent/octopus/.claude/worktrees/feature-notepad add crates/clipboard/src/store.rs
git -C /Users/wudarui/workspace/agent/octopus/.claude/worktrees/feature-notepad commit -m "feat(clipboard): store 新增 update_content（content+search_text 同写）"
```

---

## Task 2: `set_clipboard_item_text` 命令

**Files:**
- Modify: `crates/desktop/src/clipboard_commands.rs`（新增命令，参考 `ocr_image` L379 的 `State<'_, Arc<ClipboardHandle>>` + `with_db` + `handle.write_text` 写法）

- [x] **Step 1: 新增命令**

在 `crates/desktop/src/clipboard_commands.rs` 中（`ocr_image` 函数之后）新增：

```rust
/// 精简编辑器回写：更新剪贴板条目文本（content + search_text）并同步系统剪贴板。
/// OCR 编辑、剪贴板文本条目编辑两处共用。
#[tauri::command]
pub async fn set_clipboard_item_text(
    item_id: i64,
    text: String,
    handle: State<'_, Arc<ClipboardHandle>>,
) -> Result<(), String> {
    octopus_infra::db::with_db(|conn| {
        octopus_clipboard::store::update_content(conn, item_id, &text)
    })
    .map_err(|e| e.to_string())?;

    handle.write_text(&text).map_err(|e| e.to_string())?;
    Ok(())
}
```

> 命令是 `ClipboardHandle` + `with_db` 的薄封装，逻辑已在 Task 1 的 `update_content` 单测覆盖；本命令不另写单测（无 Tauri 运行时单测基建），靠 Task 8/9 的 e2e 验证。

- [x] **Step 2: 编译确认（命令注册在 Task 5，此处仅确认本文件编译）**

Run:
```bash
cargo build --manifest-path /Users/wudarui/workspace/agent/octopus/.claude/worktrees/feature-notepad/Cargo.toml -p octopus-desktop 2>&1 | tail -5
```
Expected: 编译通过（可能有 `unused` 警告，因尚未注册/调用，正常）。

- [x] **Step 3: 提交**

```bash
git -C /Users/wudarui/workspace/agent/octopus/.claude/worktrees/feature-notepad add crates/desktop/src/clipboard_commands.rs
git -C /Users/wudarui/workspace/agent/octopus/.claude/worktrees/feature-notepad commit -m "feat(desktop): 新增 set_clipboard_item_text 命令（编辑器回写剪贴板）"
```

---

## Task 3: `compact_editor_window.rs`（窗口生命周期）

**Files:**
- Create: `crates/desktop/src/compact_editor_window.rs`
- Modify: `crates/desktop/src/main.rs`（`mod compact_editor_window;` 声明 + Destroyed 分支挂 `on_compact_editor_closed`）

- [x] **Step 1: 创建窗口模块**

创建 `crates/desktop/src/compact_editor_window.rs`：

```rust
//! 精简编辑器窗口：独立 Tauri 窗口，原生标题栏，720×560 可调大小，居中。
//!
//! 单例 + 关窗即销毁：open 时已存在则 show+focus（由 commands 层额外 emit load 推送新文本），
//! 否则创建。macOS：开窗切 Regular（Dock 显图标），关窗切回 Accessory，与 notepad/settings 对称。

use tauri::{Manager, WebviewUrl, WebviewWindowBuilder};

const WIDTH: f64 = 720.0;
const HEIGHT: f64 = 560.0;
const MIN_WIDTH: f64 = 480.0;
const MIN_HEIGHT: f64 = 360.0;
pub const WINDOW_LABEL: &str = "compact_editor_window";

/// 创建精简编辑器窗口（调用方已确保当前不存在同名窗口）。
pub fn create_compact_editor_window(app_handle: &tauri::AppHandle) {
    // macOS：编辑窗口切 Regular 让 Dock 显示图标（与 settings/notepad 一致）。
    #[cfg(target_os = "macos")]
    {
        let _ = app_handle.set_activation_policy(tauri::ActivationPolicy::Regular);
    }
    let _ = WebviewWindowBuilder::new(
        app_handle,
        WINDOW_LABEL,
        WebviewUrl::default(),
    )
    .title("编辑")
    .inner_size(WIDTH, HEIGHT)
    .min_inner_size(MIN_WIDTH, MIN_HEIGHT)
    .decorations(true)
    .resizable(true)
    .center()
    .visible(true)
    .build();
}

/// macOS: 精简编辑器窗口关闭时切回 Accessory（仅托盘）。
/// 与 notepad_window::on_notepad_closed 对称。
#[cfg(target_os = "macos")]
pub fn on_compact_editor_closed(app_handle: &tauri::AppHandle) {
    let _ = app_handle.set_activation_policy(tauri::ActivationPolicy::Accessory);
}
```

- [x] **Step 2: 在 main.rs 声明模块 + 挂载关窗回调**

在 `crates/desktop/src/main.rs` 顶部 `mod` 声明区（找 `mod notepad_window;` 那一行，在其旁）新增：

```rust
mod compact_editor_window;
```

在 `app.run` 的 `RunEvent::WindowEvent { Destroyed, label, .. }` 分支（main.rs 约 L477-488，已有 `settings_window` / `notepad_window` 两个分支）追加一个 `else if`：

```rust
                } else if label == "compact_editor_window" {
                    compact_editor_window::on_compact_editor_closed(app);
```

（即整体变成 `if label == "settings_window" {...} else if label == "notepad_window" {...} else if label == "compact_editor_window" {...}`。注意此块在 `#[cfg(target_os = "macos")]` 下，与非 mac 平台的 `on_compact_editor_closed` 缺省一致——该函数仅 mac 定义，非 mac 不引用，编译通过。）

- [x] **Step 3: 编译确认**

Run:
```bash
cargo build --manifest-path /Users/wudarui/workspace/agent/octopus/.claude/worktrees/feature-notepad/Cargo.toml -p octopus-desktop 2>&1 | tail -5
```
Expected: 编译通过。

- [x] **Step 4: 提交**

```bash
git -C /Users/wudarui/workspace/agent/octopus/.claude/worktrees/feature-notepad add crates/desktop/src/compact_editor_window.rs crates/desktop/src/main.rs
git -C /Users/wudarui/workspace/agent/octopus/.claude/worktrees/feature-notepad commit -m "feat(desktop): 精简编辑器窗口模块（创建+macOS 激活策略）"
```

---

## Task 4: `compact_editor_commands.rs`（PENDING + 3 命令 + 单测）

**Files:**
- Create: `crates/desktop/src/compact_editor_commands.rs`
- Modify: `crates/desktop/src/main.rs`（`mod compact_editor_commands;` 声明；命令注册放 Task 5 统一做）

- [x] **Step 1: 写失败测试（先建文件含测试 + helper，命令后补）**

创建 `crates/desktop/src/compact_editor_commands.rs`：

```rust
//! 精简编辑器命令层：PENDING 暂存 + 开/取/关三个命令。
//!
//! PENDING 模式参考 result_window：open 时「先写 PENDING 再建窗」，前端 mount 调
//! get_pending_compact_edit 取走。编辑器是按需创建（非预建隐藏窗），故无需 ready 握手——
//! mount 必然在 create_window 之后，get 必读到。

use std::sync::Mutex;
use tauri::{Emitter, Manager};

use crate::compact_editor_window::{create_compact_editor_window, WINDOW_LABEL};

/// 跨窗口传递的编辑载荷。rename_all=camelCase：事件 payload 与命令返回都给前端 {text, requestId}。
#[derive(Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CompactEditPayload {
    pub text: String,
    pub request_id: String,
}

/// 待载入的初始文本。open 时写入，前端 mount/并发再开时 take 或 load 推送。
static PENDING: Mutex<Option<CompactEditPayload>> = Mutex::new(None);

fn store_pending(text: String, request_id: String) {
    *PENDING.lock().unwrap() = Some(CompactEditPayload { text, request_id });
}

fn take_pending() -> Option<CompactEditPayload> {
    PENDING.lock().unwrap().take()
}

/// 打开精简编辑器：写 PENDING；已存在则 emit load 推送新文本 + 聚焦，否则建窗。
#[tauri::command]
pub fn open_compact_editor(
    initial_text: String,
    request_id: String,
    app_handle: tauri::AppHandle,
) {
    store_pending(initial_text.clone(), request_id.clone());
    if let Some(window) = app_handle.get_webview_window(WINDOW_LABEL) {
        // 并发再开：窗口已 mount，PENDING 已被首次 take，改用事件推送新 {text, requestId}。
        let _ = window.emit(
            "compact-editor://load",
            CompactEditPayload { text: initial_text, request_id },
        );
        let _ = window.show();
        let _ = window.set_focus();
    } else {
        create_compact_editor_window(&app_handle);
    }
}

/// 前端 mount 时拉取初始文本（take 清空）。
#[tauri::command]
pub fn get_pending_compact_edit() -> Option<CompactEditPayload> {
    take_pending()
}

/// 关闭精简编辑器窗口（触发 Destroyed → macOS 切 Accessory）。
#[tauri::command]
pub fn close_compact_editor(app_handle: tauri::AppHandle) {
    if let Some(window) = app_handle.get_webview_window(WINDOW_LABEL) {
        let _ = window.close();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pending_store_and_take_roundtrip() {
        // 清空可能的残留（全局静态，防并行测试污染）。
        let _ = take_pending();
        store_pending("你好".into(), "rid-1".into());
        let got = take_pending().expect("take 应返回刚写入的载荷");
        assert_eq!(got.text, "你好");
        assert_eq!(got.request_id, "rid-1");
        assert!(take_pending().is_none(), "第二次 take 应为空");
    }
}
```

- [x] **Step 2: 跑测试确认通过（helper 与测试同批落地，故直接验证）**

在 `main.rs` 顶部 `mod` 区加：

```rust
mod compact_editor_commands;
```

Run:
```bash
cargo test --manifest-path /Users/wudarui/workspace/agent/octopus/.claude/worktrees/feature-notepad/Cargo.toml -p octopus-desktop compact_editor_commands
```
Expected: PASS（`pending_store_and_take_roundtrip`）。命令本体是 Tauri 集成层，不单测。

- [x] **Step 3: 提交**

```bash
git -C /Users/wudarui/workspace/agent/octopus/.claude/worktrees/feature-notepad add crates/desktop/src/compact_editor_commands.rs crates/desktop/src/main.rs
git -C /Users/wudarui/workspace/agent/octopus/.claude/worktrees/feature-notepad commit -m "feat(desktop): 精简编辑器命令层（PENDING+开/取/关+单测）"
```

---

## Task 5: main.rs 注册命令

**Files:**
- Modify: `crates/desktop/src/main.rs` 的 `generate_handler!`（约 L218-258）

- [x] **Step 1: 注册 4 个新命令**

在 `generate_handler![ ... ]` 数组中：
- 紧跟 `clipboard_commands::ocr_image,`（L228）后加：`clipboard_commands::set_clipboard_item_text,`
- 紧跟 `notepad_window::open_notepad,`（L256）后加三行：
  ```rust
            compact_editor_commands::open_compact_editor,
            compact_editor_commands::get_pending_compact_edit,
            compact_editor_commands::close_compact_editor,
  ```

- [x] **Step 2: 编译确认**

Run:
```bash
cargo build --manifest-path /Users/wudarui/workspace/agent/octopus/.claude/worktrees/feature-notepad/Cargo.toml -p octopus-desktop 2>&1 | tail -5
```
Expected: 编译通过，无 warning（所有命令已注册+被前端待用）。

- [x] **Step 3: 提交**

```bash
git -C /Users/wudarui/workspace/agent/octopus/.claude/worktrees/feature-notepad add crates/desktop/src/main.rs
git -C /Users/wudarui/workspace/agent/octopus/.claude/worktrees/feature-notepad commit -m "feat(desktop): 注册精简编辑器 4 个命令到 generate_handler"
```

---

## Task 6: CompactEditor 前端组件 + App 路由 + 共享 helper

**Files:**
- Create: `crates/desktop/frontend/src/pages/CompactEditor/index.tsx`
- Create: `crates/desktop/frontend/src/lib/compactEditor.ts`
- Modify: `crates/desktop/frontend/src/App.tsx`（L46 switch 加 case）

- [x] **Step 1: 创建共享 helper `lib/compactEditor.ts`**

创建 `crates/desktop/frontend/src/lib/compactEditor.ts`：

```ts
import { invoke, listen } from "@/lib/tauri";

interface ResultPayload {
  requestId: string;
  text: string;
}
interface CancelPayload {
  requestId: string;
}

/**
 * 打开精简编辑器编辑一段文本，保存后回调 onResult。
 * 内部注册 result/cancel 两个监听，按 requestId 过滤；任一命中即清理解监听。
 * 取消/X 关窗 → 不调 onResult，仅清理。
 */
export async function openCompactEditor(
  initialText: string,
  onResult: (text: string) => void,
): Promise<void> {
  const requestId = crypto.randomUUID();
  let unlistenResult: (() => void) | undefined;
  let unlistenCancel: (() => void) | undefined;
  const cleanup = () => {
    unlistenResult?.();
    unlistenCancel?.();
  };
  // 先注册监听再开窗（保存需用户操作，无竞态；但顺序正确更稳）
  unlistenResult = await listen("compact-editor://result", (payload) => {
    const p = payload as ResultPayload;
    if (p.requestId !== requestId) return;
    onResult(p.text);
    cleanup();
  });
  unlistenCancel = await listen("compact-editor://cancel", (payload) => {
    const p = payload as CancelPayload;
    if (p.requestId !== requestId) return;
    cleanup();
  });
  await invoke("open_compact_editor", { initialText, requestId });
}
```

- [x] **Step 2: 创建编辑器组件 `pages/CompactEditor/index.tsx`**

创建 `crates/desktop/frontend/src/pages/CompactEditor/index.tsx`：

```tsx
import { useState, useRef, useEffect, useCallback, type ReactNode } from "react";
import { invoke, listen } from "@/lib/tauri";
import { emit } from "@tauri-apps/api/event";
import {
  Undo2, Redo2, ZoomIn, ZoomOut, Search, Eraser, Save, X,
  ChevronUp, ChevronDown, Replace, Check,
} from "lucide-react";

interface PendingEdit {
  text: string;
  requestId: string;
}

const FONT_KEY = "compact-editor-font-size";
const FONT_MIN = 12;
const FONT_MAX = 24;

function CompactEditor() {
  const [text, setText] = useState("");
  const [fontSize, setFontSize] = useState(() => {
    const saved = Number(localStorage.getItem(FONT_KEY));
    return saved >= FONT_MIN && saved <= FONT_MAX ? saved : 15;
  });
  const [showFind, setShowFind] = useState(false);
  const [findQuery, setFindQuery] = useState("");
  const [replaceQuery, setReplaceQuery] = useState("");
  const [matchIdx, setMatchIdx] = useState(-1);
  const [matches, setMatches] = useState<number[]>([]);

  const taRef = useRef<HTMLTextAreaElement>(null);
  const requestIdRef = useRef<string>("");
  const savedRef = useRef(false); // 区分 unmount 时该发 result 还是 cancel

  // ── mount：拉取初始文本 + 监听并发再开 ──
  useEffect(() => {
    let unlisten: (() => void) | undefined;
    (async () => {
      const pending = await invoke<PendingEdit | null>("get_pending_compact_edit");
      if (pending) {
        setText(pending.text);
        requestIdRef.current = pending.requestId;
        setTimeout(() => taRef.current?.focus(), 0);
      }
      unlisten = await listen("compact-editor://load", (payload) => {
        const p = payload as PendingEdit;
        setText(p.text);
        requestIdRef.current = p.requestId;
        savedRef.current = false;
        setMatches([]);
        setMatchIdx(-1);
        setTimeout(() => taRef.current?.focus(), 0);
      });
    })();
    return () => {
      unlisten?.();
      // 兜底：未保存的卸载（X 关窗/系统关闭）发 cancel，防调用方监听悬空。
      if (!savedRef.current && requestIdRef.current) {
        emit("compact-editor://cancel", { requestId: requestIdRef.current });
      }
    };
  }, []);

  const charCount = [...text].length;

  const doSave = useCallback(() => {
    if (!requestIdRef.current) return;
    savedRef.current = true;
    emit("compact-editor://result", { requestId: requestIdRef.current, text });
    invoke("close_compact_editor");
  }, [text]);

  const doCancel = useCallback(() => {
    if (requestIdRef.current) {
      savedRef.current = true; // 已显式发 cancel，别让 unmount 再发
      emit("compact-editor://cancel", { requestId: requestIdRef.current });
    }
    invoke("close_compact_editor");
  }, []);

  // ── 字号 ──
  const decFont = () => setFontSize((f) => Math.max(FONT_MIN, f - 1));
  const incFont = () => setFontSize((f) => Math.min(FONT_MAX, f + 1));
  useEffect(() => { localStorage.setItem(FONT_KEY, String(fontSize)); }, [fontSize]);

  // ── 撤销/重做：execCommand 触发 textarea 原生栈（Cmd+Z/Y 原生亦生效，作可靠兜底）──
  const undo = () => { taRef.current?.focus(); document.execCommand("undo"); };
  const redo = () => { taRef.current?.focus(); document.execCommand("redo"); };

  // ── 清空（二次确认）──
  const [clearPending, setClearPending] = useState(false);
  const clearAll = () => {
    if (!clearPending) { setClearPending(true); setTimeout(() => setClearPending(false), 2000); return; }
    setText(""); setClearPending(false); setMatches([]); setMatchIdx(-1);
  };

  // ── 查找/替换 ──
  const runFind = useCallback(() => {
    const q = findQuery;
    if (!q) { setMatches([]); setMatchIdx(-1); return; }
    const ta = taRef.current;
    if (!ta) return;
    const lower = text.toLowerCase();
    const needle = q.toLowerCase();
    const idxs: number[] = [];
    let from = 0;
    while (true) {
      const i = lower.indexOf(needle, from);
      if (i === -1) break;
      idxs.push(i);
      from = i + needle.length;
    }
    setMatches(idxs);
    setMatchIdx(idxs.length > 0 ? 0 : -1);
    if (idxs.length > 0) selectRange(idxs[0], q.length);
  }, [findQuery, text]);

  useEffect(() => { if (showFind) runFind(); }, [runFind, showFind]);

  const selectRange = (start: number, len: number) => {
    const ta = taRef.current;
    if (!ta) return;
    ta.focus();
    ta.setSelectionRange(start, start + len);
    // 滚动到选中处
    const lineHeight = fontSize * 1.6;
    const lineNum = text.slice(0, start).split("\n").length;
    ta.scrollTop = Math.max(0, (lineNum - 2) * lineHeight);
  };

  const gotoMatch = (delta: number) => {
    if (matches.length === 0) return;
    const next = (matchIdx + delta + matches.length) % matches.length;
    setMatchIdx(next);
    selectRange(matches[next], findQuery.length);
  };

  const replaceOne = () => {
    if (matchIdx < 0 || !findQuery) return;
    const start = matches[matchIdx];
    const next = text.slice(0, start) + replaceQuery + text.slice(start + findQuery.length);
    setText(next);
    // 替换后重算
    setTimeout(runFind, 0);
  };

  const replaceAll = () => {
    if (!findQuery) return;
    setText(text.split(findQuery).join(replaceQuery));
    setTimeout(runFind, 0);
  };

  // ── 快捷键 ──
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      const mod = e.metaKey || e.ctrlKey;
      if (mod && e.key === "Enter") { e.preventDefault(); doSave(); return; }
      if (e.key === "Escape") {
        if (showFind) { setShowFind(false); return; }
        doCancel(); return;
      }
      if (mod && e.key.toLowerCase() === "f") { e.preventDefault(); setShowFind(true); return; }
    };
    document.addEventListener("keydown", onKey);
    return () => document.removeEventListener("keydown", onKey);
  }, [doSave, doCancel, showFind]);

  const ToolBtn = ({ onClick, title, disabled, children }: {
    onClick: () => void; title: string; disabled?: boolean; children: ReactNode;
  }) => (
    <button
      type="button"
      disabled={disabled}
      title={title}
      onClick={onClick}
      className="p-1.5 rounded-md text-stone-600 hover:bg-stone-100 hover:text-stone-900 disabled:opacity-30 disabled:hover:bg-transparent transition-colors"
    >{children}</button>
  );

  return (
    <div className="flex flex-col h-full bg-background">
      {/* 工具栏 */}
      <div className="flex-shrink-0 flex items-center gap-0.5 px-2 py-1.5 border-b border-border bg-stone-50">
        <ToolBtn onClick={undo} title="撤销 (Cmd+Z)"><Undo2 className="w-4 h-4" /></ToolBtn>
        <ToolBtn onClick={redo} title="重做 (Cmd+Shift+Z)"><Redo2 className="w-4 h-4" /></ToolBtn>
        <span className="w-px h-4 bg-stone-200 mx-1" />
        <ToolBtn onClick={decFont} title="缩小字号" disabled={fontSize <= FONT_MIN}><ZoomOut className="w-4 h-4" /></ToolBtn>
        <span className="text-[11px] text-stone-500 w-7 text-center tabular-nums">{fontSize}</span>
        <ToolBtn onClick={incFont} title="放大字号" disabled={fontSize >= FONT_MAX}><ZoomIn className="w-4 h-4" /></ToolBtn>
        <span className="w-px h-4 bg-stone-200 mx-1" />
        <ToolBtn onClick={() => setShowFind((v) => !v)} title="查找/替换 (Cmd+F)"><Search className="w-4 h-4" /></ToolBtn>
        <ToolBtn onClick={clearAll} title="清空">
          {clearPending ? <Check className="w-4 h-4 text-red-500" /> : <Eraser className="w-4 h-4" />}
        </ToolBtn>
        <div className="flex-1" />
        <span className="text-[11px] text-stone-400 mr-2 tabular-nums">{charCount} 字</span>
        <button
          type="button"
          onClick={doCancel}
          className="flex items-center gap-1 px-2.5 py-1 rounded-md text-xs text-stone-600 hover:bg-stone-200 transition-colors"
        >
          <X className="w-3.5 h-3.5" /> 取消
        </button>
        <button
          type="button"
          onClick={doSave}
          className="flex items-center gap-1 px-2.5 py-1 rounded-md text-xs text-white bg-[#007aff] hover:bg-[#0066d6] transition-colors"
        >
          <Save className="w-3.5 h-3.5" /> 保存
          <span className="text-[10px] opacity-70">⌘↵</span>
        </button>
      </div>

      {/* 查找/替换条 */}
      {showFind && (
        <div className="flex-shrink-0 flex flex-wrap items-center gap-1.5 px-2 py-1.5 border-b border-border bg-stone-100">
          <input
            autoFocus
            value={findQuery}
            onChange={(e) => setFindQuery(e.target.value)}
            onKeyDown={(e) => { if (e.key === "Enter") gotoMatch(e.shiftKey ? -1 : 1); }}
            placeholder="查找"
            className="w-32 px-2 py-0.5 text-xs border border-stone-300 rounded bg-white outline-none focus:border-[#007aff]"
          />
          <span className="text-[10px] text-stone-500 w-12 tabular-nums">
            {matches.length > 0 ? `${matchIdx + 1}/${matches.length}` : "0/0"}
          </span>
          <ToolBtn onClick={() => gotoMatch(-1)} title="上一个" disabled={matches.length === 0}><ChevronUp className="w-3.5 h-3.5" /></ToolBtn>
          <ToolBtn onClick={() => gotoMatch(1)} title="下一个" disabled={matches.length === 0}><ChevronDown className="w-3.5 h-3.5" /></ToolBtn>
          <input
            value={replaceQuery}
            onChange={(e) => setReplaceQuery(e.target.value)}
            placeholder="替换"
            className="w-32 px-2 py-0.5 text-xs border border-stone-300 rounded bg-white outline-none focus:border-[#007aff]"
          />
          <button type="button" onClick={replaceOne} className="px-2 py-0.5 text-[11px] rounded border border-stone-300 hover:bg-stone-200">替换</button>
          <button type="button" onClick={replaceAll} className="flex items-center gap-0.5 px-2 py-0.5 text-[11px] rounded border border-stone-300 hover:bg-stone-200">
            <Replace className="w-3 h-3" /> 全替
          </button>
        </div>
      )}

      {/* 文本区 */}
      <textarea
        ref={taRef}
        value={text}
        onChange={(e) => setText(e.target.value)}
        style={{ fontSize: `${fontSize}px`, lineHeight: 1.6 }}
        spellCheck={false}
        className="flex-1 w-full resize-none outline-none p-4 bg-background text-foreground thin-scrollbar"
        placeholder="在此编辑…"
      />
    </div>
  );
}

export default CompactEditor;
```

> 注：`emit` 用 `@tauri-apps/api/event` 的原版（broadcast 到所有窗口，调用方按 rid 过滤）；`invoke`/`listen` 用 `@/lib/tauri` 封装（listen 已解包 payload）。

- [x] **Step 3: App.tsx 加路由**

在 `crates/desktop/frontend/src/App.tsx`：
- 顶部 import 区加：`import CompactEditor from "@/pages/CompactEditor";`
- `switch (label)` 内（L47-54 之间）加一个 case：

```tsx
          case "compact_editor_window":
            return <CompactEditor />;
```

- [x] **Step 4: 类型检查 + 构建（含 dist）**

Run:
```bash
npm --prefix /Users/wudarui/workspace/agent/octopus/.claude/worktrees/feature-notepad/crates/desktop/frontend run build
```
Expected: `tsc -b` 无类型错误，`vite build` 产出新 dist。

- [x] **Step 5: 提交（含 dist）**

```bash
git -C /Users/wudarui/workspace/agent/octopus/.claude/worktrees/feature-notepad add crates/desktop/frontend/src/pages/CompactEditor/index.tsx crates/desktop/frontend/src/lib/compactEditor.ts crates/desktop/frontend/src/App.tsx crates/desktop/dist
git -C /Users/wudarui/workspace/agent/octopus/.claude/worktrees/feature-notepad commit -m "feat(desktop-frontend): CompactEditor 组件 + App 路由 + 共享 helper"
```

---

## ⚠️ 设计修订（2026-06-30）：Task 7 废弃 → 改为 Task 11（Result 原地双模式）

用户反馈（5 条逐步明确）：语音 Result **不弹独立编辑器窗**，改为**编辑框原地尺寸双模式**（精简 520×116 / 长篇 720×480）+ 工具栏「放大/缩小」开关切换，长篇模式可拖拽调整且记忆尺寸。

- **Task 7（Result 接入独立窗「展开编辑」）废弃**——其 `applyResultText`/`openExpandEdit`/`openCompactEditor` 调用全部移除。
- **Task 6/8/9 不变**——精简编辑器独立窗保留给 OCR 与剪贴板文本。
- 替换为下方 **Task 11**。详见 spec §3.5① 重写。

### Task 11: 语音 Result 编辑框尺寸双模式——CSS 伪装 + 点击穿透（替换 Task 7）

**Files:**
- Modify: `crates/desktop/src/result_window.rs`（固定 720×480 + 删 `set_result_window_mode` + 加 `set_result_click_through` + `start_click_through_poller` + `set_result_ignores_mouse`）
- Modify: `crates/desktop/src/main.rs`（注册命令 `set_result_window_mode` → `set_result_click_through`）
- Modify: `crates/desktop/frontend/src/pages/Result/index.tsx`（纯 CSS 双模式）
- Create: `crates/desktop/frontend/public/icons/minimize.svg`（缩小态，四角向内）
- Modify: `crates/desktop/frontend/src/components/SvgIcon.tsx`（ICONS 加 `"minimize"`）

**背景**：setSize/setMaxSize/localStorage 方案（min/max 放宽、ACL 补全）实测无效——`transparent`+`decorations(false)` 悬浮窗上 NSWindow 拒绝 setFrame，`outerSize()` 恒读回 520×116。改 CSS 伪装：物理窗口固定大尺寸，前端按模式切可见容器尺寸，透明区用 Rust 轮询点击穿透。详见 spec §3.5①。

**实现要点（关键代码骨架）：**

`result_window.rs`：
```rust
const RESULT_WIDTH: f64 = 720.0;
const RESULT_HEIGHT: f64 = 480.0;
const BAR_W: f64 = 520.0; const BAR_H: f64 = 116.0;
const BAR_OFFSET_X: f64 = (RESULT_WIDTH - BAR_W) / 2.0; // =100，与前端居中一致
static RESULT_CLICK_THROUGH: AtomicBool = AtomicBool::new(true); // 精简态=true

// 创建：.inner_size(RESULT_WIDTH, RESULT_HEIGHT).resizable(true)
//       .transparent(true).decorations(false).always_on_top(true).accept_first_mouse(true)
// Ok 分支：start_click_through_poller(app.clone());

#[tauri::command]
pub fn set_result_click_through(app: tauri::AppHandle, expanded: bool) {
    RESULT_CLICK_THROUGH.store(!expanded, Ordering::Relaxed);
    if expanded { // 长篇：立即关穿透
        if let Some(win) = app.get_webview_window(WINDOW_LABEL) { set_result_ignores_mouse(&win, false); }
    }
    // 精简：交由轮询线程按光标位置决定
}

// start_click_through_poller（仅 macOS）：~33ms tokio interval，读 CGEvent.location()，
// 按窗口 outer_position()/scale_factor 算小条屏幕矩形 [wx+100..wx+620]×[wy..wy+116]，
// 光标在矩形外 → 穿透、在内 → 可交互；仅在 want != cur_ignore 时切 setIgnoresMouseEvents。
// 窗口隐藏或长篇态（!need_through）时不穿透。

// set_result_ignores_mouse：macOS → run_on_main_thread + ns_win.setIgnoresMouseEvents(ignore)
//   （objc2_app_kit::NSWindow，比 Tauri set_ignore_cursor_events 封装可靠）；其他平台 → set_ignore_cursor_events
```

`Result/index.tsx`：
```tsx
const win = useMemo(() => getCurrentWindow(), []);
const toggleExpand = useCallback(() => {
  const next = !expanded;
  setExpanded(next);
  invoke("set_result_click_through", { expanded: next }); // 通知后端切穿透模式
}, [expanded]);

// 外层透明包裹 + 内层条件尺寸容器
<div className="relative w-full h-full">
  <div id="result-container" className={cn(
    "absolute top-0 left-1/2 -translate-x-1/2 bg-background rounded-lg border ... transition-all duration-200",
    expanded ? "w-[720px] h-[480px]" : "w-[520px] h-[116px]",
    visible ? "opacity-100" : "opacity-0",
  )}>...</div>
</div>
// 文本区 className：expanded ? "h-full" : "max-h-[63px]"
// tools：{ id: "toggle-size", icon: expanded ? "minimize" : "expand-edit", ... }
// 移除：LogicalSize import / saveToNote / note 工具条目 / setSize 诊断 toast / onResized 监听 / expandedSizeRef
```

- 编辑逻辑零改动（仍走 `toggleEdit`/contentEditable）。
- 「存入记事本」工具按钮已移除（大窗口原地编辑已够用，无需导入记事本）；后端 `save_transcription_to_note`/`current_transcription_id` 命令保留作基础设施。
- 边界：长篇态向下展开占满 720×480，若原位置近屏幕底可能部分超出——MVP 不重算位置，e2e 观察。

- [x] Step 1: 新建 `minimize.svg` + `SvgIcon` 加 `"minimize"` 映射
- [x] Step 2: `result_window.rs` 固定 720×480 + `start_click_through_poller` + `set_result_ignores_mouse` + `set_result_click_through` 命令；删 `set_result_window_mode`
- [x] Step 3: `main.rs` 注册 `set_result_click_through`（替换 `set_result_window_mode`）
- [x] Step 4: `Result/index.tsx` 纯 CSS 双模式 + `toggleExpand` 调 `set_result_click_through`；移除 setSize/saveToNote/note 按钮/onResized
- [x] Step 5: 重建 dist（`npm run build`）+ 验证（`tsc -b`、`cargo test`）
- [x] Step 6: commit + 同步文档（spec/plan/architecture/memory）

---

## Task 7: 语音 Result 接入「展开编辑」 ⚠️ 已废弃（见上方修订节 → Task 11）

**Files:**
- Create: `crates/desktop/frontend/public/icons/expand-edit.svg`
- Modify: `crates/desktop/frontend/src/components/SvgIcon.tsx`（ICONS 加项）
- Modify: `crates/desktop/frontend/src/pages/Result/index.tsx`（import + `applyResultText` + `openExpandEdit` + tools 加按钮）

- [ ] **Step 1: 新增图标 svg**

创建 `crates/desktop/frontend/public/icons/expand-edit.svg`（一个「方框+向外箭头」的展开图标，currentColor 填充，与现有 svg 风格一致——单色、`viewBox="0 0 24 24"`）：

```svg
<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
  <path d="M15 3h6v6"/><path d="M9 21H3v-6"/><path d="M21 3l-7 7"/><path d="M3 21l7-7"/>
</svg>
```

> 与现有 `edit.svg` / `note.svg` 同为 stroke 风格（`fill="none" stroke="currentColor"`，24×24），SvgIcon 的 mask 渲染对 stroke 图标已验证可用（现有图标即如此），无需特判。

- [ ] **Step 2: SvgIcon 注册图标名**

在 `crates/desktop/frontend/src/components/SvgIcon.tsx` 的 `ICONS` 对象（L3-15）加一行：

```ts
  "expand-edit": "/icons/expand-edit.svg",
```

- [ ] **Step 3: Result 加 `applyResultText` + `openExpandEdit`**

在 `crates/desktop/frontend/src/pages/Result/index.tsx`：
- 顶部 import 加：
  ```tsx
  import { openCompactEditor } from "@/lib/compactEditor";
  ```
- 在 `saveToNote`（约 L263）之后新增 `applyResultText` 与 `openExpandEdit`：

```tsx
  // 展开编辑回写：更新展示态 + 落库（enter_edit_mode 置 editing=true 后 commit_edit 才生效；
  // 二者均门控于活跃 stage，与现有 toggleEdit 同窗口——会话结束后不落库，沿用既有契约）。
  const applyResultText = useCallback((newText: string) => {
    displayedRef.current = newText;
    setText(newText);
    invoke("enter_edit_mode");
    invoke("commit_edit", { text: newText });
  }, []);

  // 「展开编辑」：用当前显示文本打开精简编辑器，保存后回写。
  const openExpandEdit = useCallback(() => {
    if (!text.trim()) return;
    openCompactEditor(text, applyResultText);
  }, [text, applyResultText]);
```

- [ ] **Step 4: 工具栏加「展开编辑」按钮**

在 `tools` 数组（约 L380-396）中，「存入记事本」项（`{ id: "note", ... }`）之后、`...(editing ? [...] : [...])` 之前，插入：

```tsx
    { id: "expand-edit", icon: "expand-edit" as IconName, label: "展开编辑", disabled: !text.trim(), onClick: openExpandEdit },
```

- [ ] **Step 5: 类型检查 + 构建（含 dist）**

Run:
```bash
npm --prefix /Users/wudarui/workspace/agent/octopus/.claude/worktrees/feature-notepad/crates/desktop/frontend run build
```
Expected: tsc 通过，dist 更新。

- [ ] **Step 6: 提交（含 dist）**

```bash
git -C /Users/wudarui/workspace/agent/octopus/.claude/worktrees/feature-notepad add crates/desktop/frontend/public/icons/expand-edit.svg crates/desktop/frontend/src/components/SvgIcon.tsx crates/desktop/frontend/src/pages/Result/index.tsx crates/desktop/dist
git -C /Users/wudarui/workspace/agent/octopus/.claude/worktrees/feature-notepad commit -m "feat(desktop-frontend): Result 接入「展开编辑」打开精简编辑器"
```

---

## Task 8: OCR 接入（移除系统 TextEdit）

**Files:**
- Modify: `crates/desktop/src/clipboard_commands.rs`（`ocr_image` 删 TextEdit 调用 + 删 `open_text_editor_with_content` 函数）
- Modify: `crates/desktop/frontend/src/pages/Clipboard/ClipboardItem.tsx`（`handleOcr` 取返回值 + 开编辑器 + 回写）

- [x] **Step 1: 后端 `ocr_image` 移除 TextEdit**

在 `crates/desktop/src/clipboard_commands.rs`：
- 删除 `ocr_image` 中的 `open_text_editor_with_content(&text);`（约 L416）这一行。
- 删除现已无引用的 `fn open_text_editor_with_content(text: &str) { ... }` 整个函数（约 L421-447）。删除前先确认无其他调用方：

Run:
```bash
grep -rn "open_text_editor_with_content" /Users/wudarui/workspace/agent/octopus/.claude/worktrees/feature-notepad/crates
```
Expected: 仅 `clipboard_commands.rs` 内的定义处出现（调用已在上一行删除）→ 安全删除整个函数。

删除后 `ocr_image` 末尾变为：

```rust
    octopus_infra::db::with_db(|conn| {
        octopus_clipboard::store::update_search_text(conn, id, &text)
    }).map_err(|e| e.to_string())?;

    handle.write_text(&text).map_err(|e| e.to_string())?;

    Ok(text)
}
```

- [x] **Step 2: 后端编译确认**

Run:
```bash
cargo build --manifest-path /Users/wudarui/workspace/agent/octopus/.claude/worktrees/feature-notepad/Cargo.toml -p octopus-desktop 2>&1 | tail -5
```
Expected: 编译通过，无 dead_code 警告。

- [x] **Step 3: 前端 `handleOcr` 改造**

在 `crates/desktop/frontend/src/pages/Clipboard/ClipboardItem.tsx`：
- 顶部 import 加：`import { openCompactEditor } from "@/lib/compactEditor";`
- 改写 `handleOcr`（约 L87-106）——取 `ocr_image` 返回的文本，开编辑器，保存后 `set_clipboard_item_text` 回写 + 刷新：

```tsx
  const handleOcr = async (e: React.MouseEvent) => {
    e.stopPropagation();
    if (ocrLoading) return;
    setOcrLoading(true);
    try {
      const text = await invoke<string>("ocr_image", { id: item.id });
      setOcrLoading(false);
      setOcrDone(true);
      setTimeout(() => setOcrDone(false), 1000);
      // 识别成功 → 打开精简编辑器，保存后回写剪贴板条目 + 刷新列表
      openCompactEditor(text, (edited) => {
        invoke("set_clipboard_item_text", { itemId: item.id, text: edited })
          .then(onChanged)
          .catch(console.error);
      });
    } catch (err) {
      setOcrLoading(false);
      const msg = String(err);
      if (msg.includes("未识别到文本")) {
        setOcrDone(true);
        setTimeout(() => setOcrDone(false), 1000);
      } else {
        console.error(err);
      }
    }
  };
```

- [x] **Step 4: 类型检查 + 构建（含 dist）**

Run:
```bash
npm --prefix /Users/wudarui/workspace/agent/octopus/.claude/worktrees/feature-notepad/crates/desktop/frontend run build
```
Expected: tsc 通过，dist 更新。

- [x] **Step 5: 提交（含 dist）**

```bash
git -C /Users/wudarui/workspace/agent/octopus/.claude/worktrees/feature-notepad add crates/desktop/src/clipboard_commands.rs crates/desktop/frontend/src/pages/Clipboard/ClipboardItem.tsx crates/desktop/dist
git -C /Users/wudarui/workspace/agent/octopus/.claude/worktrees/feature-notepad commit -m "feat(ocr): OCR 识别后打开精简编辑器（移除系统 TextEdit）+ 回写剪贴板"
```

---

## Task 9: 剪贴板文本条目「编辑」按钮

**Files:**
- Modify: `crates/desktop/frontend/src/pages/Clipboard/ClipboardItem.tsx`（import + `handleEditText` + 操作区加按钮）

- [x] **Step 1: 加文本编辑处理 + 按钮**

在 `crates/desktop/frontend/src/pages/Clipboard/ClipboardItem.tsx`：
- 顶部 lucide import 中加 `SquarePen`（即 `import { ..., NotebookPen, SquarePen } from "lucide-react";`）。
- 在 `handleSaveToNote`（约 L127-138）之后新增：

```tsx
  const handleEditText = (e: React.MouseEvent) => {
    e.stopPropagation();
    if (item.item_type === "image" || item.item_type === "file") return;
    openCompactEditor(item.content, (edited) => {
      invoke("set_clipboard_item_text", { itemId: item.id, text: edited })
        .then(onChanged)
        .catch(console.error);
    });
  };
```

- 在右侧操作区，「存入记事本」按钮（`onClick={handleSaveToNote}`，约 L198-204）之后插入「编辑」按钮，仅对文本/语音文本显示：

```tsx
        {item.item_type !== "image" && item.item_type !== "file" && (
          <button
            className="p-0.5 opacity-0 group-hover:opacity-60 hover:!opacity-100 transition-opacity"
            onClick={handleEditText}
            title="编辑"
          >
            <SquarePen className="w-3.5 h-3.5 text-muted-foreground hover:text-foreground" />
          </button>
        )}
```

- [x] **Step 2: 类型检查 + 构建（含 dist）**

Run:
```bash
npm --prefix /Users/wudarui/workspace/agent/octopus/.claude/worktrees/feature-notepad/crates/desktop/frontend run build
```
Expected: tsc 通过，dist 更新。

- [x] **Step 3: 提交（含 dist）**

```bash
git -C /Users/wudarui/workspace/agent/octopus/.claude/worktrees/feature-notepad add crates/desktop/frontend/src/pages/Clipboard/ClipboardItem.tsx crates/desktop/dist
git -C /Users/wudarui/workspace/agent/octopus/.claude/worktrees/feature-notepad commit -m "feat(clipboard): 文本条目新增「编辑」按钮（打开精简编辑器回写）"
```

---

## Task 10: 文档同步 + 全量验证

**Files:**
- Modify: `docs/architecture.md`

- [x] **Step 1: 同步 architecture.md**

在 `docs/architecture.md`：
- 窗口列表（已有 `notepad_window` 等的位置）加一行：`compact_editor_window` — 精简文本编辑器（工具栏+textarea，关窗即销毁，编辑结果事件返回调用方）。
- 命令清单（已有 `open_notepad` 等的位置）加：`open_compact_editor` / `get_pending_compact_edit` / `close_compact_editor`（精简编辑器）/ `set_clipboard_item_text`（编辑器回写剪贴板）。
- 若有「Tauri 窗口」小结，补一句：精简编辑器与记事本并列，纯编辑工具不持久化。

（具体小标题与行号以文件实际结构为准，新增条目风格对齐已有 `notepad` 条目。）

- [x] **Step 2: 全量后端测试**

Run:
```bash
cargo test --manifest-path /Users/wudarui/workspace/agent/octopus/.claude/worktrees/feature-notepad/Cargo.toml -p octopus-clipboard -p octopus-desktop 2>&1 | tail -15
```
Expected: 全绿（含 Task 1 的 `test_update_content`、Task 4 的 `pending_store_and_take_roundtrip`，且未破坏既有测试）。

- [x] **Step 3: desktop 整体编译**

Run:
```bash
cargo build --manifest-path /Users/wudarui/workspace/agent/octopus/.claude/worktrees/feature-notepad/Cargo.toml -p octopus-desktop 2>&1 | tail -5
```
Expected: 编译通过。

- [x] **Step 4: 提交文档**

```bash
git -C /Users/wudarui/workspace/agent/octopus/.claude/worktrees/feature-notepad add docs/architecture.md
git -C /Users/wudarui/workspace/agent/octopus/.claude/worktrees/feature-notepad commit -m "docs(architecture): 同步精简编辑器窗口与命令清单"
```

---

## 验收 e2e（手动——**本计划唯一剩余项**，交给用户跑 `./run-octopus.sh` 后逐项确认）

1. **Result 双模式**（CSS 伪装方案，**e2e 已通过**——精简态穿透确认）：识别中文 → 点「放大」→ 可见容器撑满 720×480、编辑区撑满 → 编辑 → 保存 → 文本落库 → 点「缩小」切回 520×116 小条；精简态小条下方透明区点击穿透到后方应用。
2. **OCR**：剪贴板图片点 OCR → 编辑器自动开 → 改 → 保存 → 该条目内容 + 系统剪贴板更新；不再弹系统 TextEdit。
3. **剪贴板文本**：文本/语音条目 hover 点「编辑」→ 编辑器开 → 改 → 保存 → 列表 + 系统剪贴板更新。
4. **边界**：取消/Esc/X 关窗不回写；字号记忆生效；查找替换命中数与跳转正确；字符计数对中文按字计；并发开窗（Result + 剪贴板同时开）不串扰。

## ✅ 已修 bug：Result「放大」切换无响应（最终结论：CSS 伪装方案）

**现象**：语音结果窗工具栏点「放大」按钮，图标切到「缩小」但窗口尺寸没变（双模式切换失效）。

**完整踩坑链（三段误判 → 真根因 → 最终方案）：**

1. **误判 resizable**（`2195c80`）：据 Tauri 文档「`resizable(false)` 时 `setSize` 被忽略」改 `.resizable(true)`。未解决。

2. **误判 ACL（真实但不足，`93f58a2`）**：诊断 toast 报 `Command plugin:window|set_max_size not allowed by ACL`，在 `capabilities/default.json` 补 `allow-set-min-size`/`allow-set-max-size`/`allow-set-resizable`/`allow-outer-size`/`allow-scale-factor`。**ACL 缺失是真实的**（确实抛错中断 setSize），但补全后 **setSize 仍失效**——ACL 不是终点。

3. **真根因（NSWindow 拒绝 setFrame）**：ACL 补全 + min/max 全放宽到 [100,4000]、目标 720×480 完全在区间内，`outerSize()` 仍读回 520×116。证明在 `transparent(true)`+`decorations(false)` 悬浮窗上，NSWindow **根本拒绝** setFrame/setSize——不是约束、不是权限、是 frame 不可变。setSize 路径 100% 失效，无解。

**最终方案**：放弃运行时 setSize，改 **CSS 伪装 + 点击穿透**——窗口物理固定 720×480，前端 CSS 切可见容器尺寸（精简 520×116 小条 / 长篇撑满），透明区由 Rust 后台轮询线程（`CGEvent` 读全局鼠标）在 NSWindow 直调 `setIgnoresMouseEvents` 切穿透。详见 Task 11。e2e 已通过（精简态穿透确认）。

**教训**：
- 前端 `await` 窗口命令无 try/catch 时 ACL 错误被默默吞掉——诊断 toast（捕获并显示错误 + 读回 outerSize）是定位关键。
- **别把「ACL 补全」当 setSize 失效的终点**：透明无边框悬浮窗上 ACL 齐全 setSize 仍会被 NSWindow 拒绝；ACL 补全后若读回尺寸仍不变，即命中此真凶，应立即转 CSS 伪装。
- setSize 读回旧值（而非抛错）是「NSWindow 拒绝 setFrame」区别于 ACL（抛错）的判别信号。

## 不做（明确排除）

- 不合并到 main（记事本 e2e 仍待用户确认；合并是外向难逆动作，等用户回来显式授权走 `superpowers:finishing-a-development-branch`）。
- 不接入富文本（TipTap）/标题/分类/收藏——属于完整版记事本。
- 不加 vitest 前端测试框架（YAGNI，`tsc -b` + e2e 足够）。


---

## 来自原文件 `2026-06-30-notepad.md`

# 记事本（内容收集箱）实施计划

> **For agentic workers:** REQUIRED SUB-SKILL: 使用 `superpowers:subagent-driven-development`（推荐）或 `superpowers:executing-plans` 逐任务实现本计划。步骤用 checkbox（`- [ ]`）语法跟踪。

**Goal:** 为 octopus 新增「内容收集箱」式记事本：ASR / OCR / 剪贴板结果一键存入笔记并在富文本编辑器中整理，独立 crate `octopus-notepad` 承载业务逻辑，desktop 加薄命令层 + 窗口 + 前端页面。

**Architecture:** 三层依赖 `infra ← notepad ← desktop`。`infra/db` 加 `notes` + `notes_fts` 表与 v9 迁移（幂等）；新建 `octopus-notepad` crate（仅依赖 `infra` + `scraper`）承载 model / store（CRUD+FTS）/ serialize（HTML→text）/ export（文件 I/O）；`desktop` 加 `note_commands.rs`（薄命令转调 + `emit("notepad://changed")`）+ `notepad_window.rs`（独立窗口）+ 托盘入口 + 前端 `pages/Notepad`（TipTap 富文本）。笔记富文本为内部模型，md/txt/html 为序列化格式。

**Tech Stack:** Rust（rusqlite bundled, anyhow, serde, scraper）/ Tauri 2 / React 19 + TypeScript 6 + Vite 8 + Tailwind 4 + lucide-react / TipTap v3（ProseMirror）+ tiptap-markdown。

**对应规格文档：** `docs/superpowers/specs/2026-06-30-notepad-design.md`

> **2026-07-01 撤销说明**：`save_clipboard_to_note` 命令 + `saveClipboardToNote` helper 已移除——剪贴板浮窗条目（`ClipboardItem.tsx`）+ Settings 剪贴板管理页（`ClipboardPanel.tsx`）+ 语音结果窗（`Result/index.tsx`）的「存入记事本」按钮均已删除。下方涉及 `save_clipboard_to_note` 的 Task（接口定义/注册/前端按钮/e2e）为历史实施记录，实际代码已不含该命令；记事本「存入」入口现仅剩 `HistoryPanel`（`save_transcription_to_note`）+ OCR（`save_ocr_to_note`）。`NoteSource::Clipboard` 变体保留供 DB 历史数据反序列化。

**关键约束（来自 CLAUDE.md / 记忆）：**
- 工作树 cwd 陷阱：`cargo`/`npm`/`grep`/`git` 必须显式指定工作树（`--manifest-path` / `--prefix` / `-C` / 绝对路径）。所有命令默认在工作树根 `/Users/wudarui/workspace/agent/octopus/.claude/worktrees/feature-notepad` 下执行。
- `config/` 是 `~/.octopus/` 软链接：读写配置必须用绝对路径 `/Users/wudarui/.octopus/`，不走 `config/` 相对路径。
- 测试 DB：内存 DB + `include_str!("../../infra/src/db.sql")` + `execute_batch`（仿 clipboard `open_test_db`）。
- 中文交互（注释/文档/commit message 用中文，但 crate/标识符用英文）。

---

## File Structure

**新建文件：**
- `crates/notepad/Cargo.toml` — crate 清单（octopus-infra, scraper, rusqlite bundled, anyhow, serde, serde_json, log, dirs, sha2）
- `crates/notepad/src/lib.rs` — `pub use model/store/serialize/export`
- `crates/notepad/src/model.rs` — `NoteSource` / `Note` / `NoteFilter`
- `crates/notepad/src/serialize.rs` — `extract_text(html)`（scraper）
- `crates/notepad/src/store.rs` — CRUD + FTS + count + toggle + delete（经 `infra::db::with_db`）
- `crates/notepad/src/export.rs` — `write_export` / `read_import`（`~/Documents/octopus/notes/`）
- `crates/desktop/src/note_commands.rs` — 薄 Tauri 命令层 + 集成入口命令 + `get_note_image` 桥接
- `crates/desktop/src/notepad_window.rs` — notepad 窗口管理（仿 `settings_window.rs`）
- `crates/desktop/frontend/src/lib/notepad.ts` — invoke 封装
- `crates/desktop/frontend/src/hooks/useNotes.ts` — 列表 + filter + 分页 + `notepad://changed` 监听
- `crates/desktop/frontend/src/types/note.ts` — TS 类型
- `crates/desktop/frontend/src/pages/Notepad/index.tsx` — 三栏布局
- `crates/desktop/frontend/src/pages/Notepad/NoteList.tsx` — 列表 + 搜索 + 来源筛选 + 加载更多
- `crates/desktop/frontend/src/pages/Notepad/NoteEditor.tsx` — TipTap 编辑器 + 工具栏 + 自动保存
- `crates/desktop/frontend/src/pages/Notepad/extensions.ts` — TipTap 扩展 + Image NodeView

**修改文件：**
- `Cargo.toml`（workspace members 加 `crates/notepad`）
- `crates/desktop/Cargo.toml`（加 `octopus-notepad` 依赖 + `tauri-plugin-dialog` 已有）
- `crates/infra/src/db.sql`（加 `notes` + `notes_fts` + 3 触发器）
- `crates/infra/src/db.rs`（v8→v9 迁移分支 + `get_transcription_display_text(id)`）
- `crates/desktop/src/main.rs`（`mod note_commands; mod notepad_window;` + invoke_handler 注册 + macOS 关窗策略）
- `crates/desktop/src/tray.rs`（菜单加「记事本」项 + 事件处理）
- `crates/desktop/src/coordinator.rs`（`CURRENT_TRANSCRIPTION_ID` 静态 + setter + 3 处会话起点写入 + `current_transcription_id` 命令）
- `crates/desktop/frontend/src/App.tsx`（`case "notepad_window"` 路由）
- `crates/desktop/frontend/src/pages/Result/index.tsx`（工具栏加「存入记事本」按钮 + 图标）
- `crates/desktop/frontend/src/pages/Clipboard/ClipboardItem.tsx`（操作区加「存入记事本」按钮）
- `crates/desktop/frontend/src/components/SvgIcon.tsx`（加 `note` 图标）
- `crates/desktop/frontend/package.json`（TipTap 依赖）
- `crates/desktop/frontend/src/pages/Settings/HistoryPanel.tsx`（行操作加「存入记事本」）
- `crates/desktop/frontend/src/pages/Settings/ClipboardPanel.tsx`（行操作加「存入记事本」）

---

## Task 1: octopus-notepad crate 脚手架

**Files:**
- Create: `crates/notepad/Cargo.toml`
- Create: `crates/notepad/src/lib.rs`
- Modify: `Cargo.toml:2`（workspace members）
- Modify: `crates/desktop/Cargo.toml:81`（加 notepad 依赖）

- [ ] **Step 1: 创建 crate 清单**

`crates/notepad/Cargo.toml`：
```toml
[package]
name = "octopus-notepad"
version = "0.1.0"
edition = "2021"

[dependencies]
octopus-infra = { path = "../infra" }
scraper = "0.20"
rusqlite = { version = "0.31", features = ["bundled"] }
anyhow = "1"
serde = { version = "1", features = ["derive"] }
serde_json = "1"
log = "0.4"
dirs = "5"
```

- [ ] **Step 2: 创建 lib.rs 占位（后续 task 填充模块）**

`crates/notepad/src/lib.rs`：
```rust
//! octopus-notepad：内容收集箱式记事本业务逻辑。
//! 仅依赖 octopus-infra（DB 访问）；序列化用 scraper，文件 I/O 用 std + dirs。

pub mod export;
pub mod model;
pub mod serialize;
pub mod store;

pub use model::{Note, NoteFilter, NoteSource};
```

（各模块文件在后续 task 创建；本步先写 lib.rs，编译会因缺模块失败——先不编译，Task 5 末尾统一编译。）

- [ ] **Step 3: 注册 workspace member**

`Cargo.toml:2`，把 `crates/notepad` 加入 members：
```toml
members = ["crates/infra", "crates/asr-local", "crates/asr-cloud", "crates/server", "crates/cli", "crates/desktop", "crates/llm", "crates/dlp", "crates/download", "crates/clipboard", "crates/ocr", "crates/capx", "crates/notepad"]
```

- [ ] **Step 4: desktop 依赖 notepad**

`crates/desktop/Cargo.toml`，在 `octopus-clipboard` 依赖行（第 81 行附近）后追加：
```toml
# 记事本（内容收集箱）业务逻辑
octopus-notepad = { path = "../notepad" }
```

- [ ] **Step 5: 暂存，不单独编译**

crate 模块尚未创建，跳过编译验证（Task 5 完成后统一编译）。提交留到 Task 5。

---

## Task 2: db.sql notes 表 + notes_fts + v9 迁移

**Files:**
- Modify: `crates/infra/src/db.sql`（末尾追加 notes 表块）
- Modify: `crates/infra/src/db.rs:142-213`（init_schema 加 v8→v9 分支 + fresh install 改 v9）
- Test: `crates/infra/src/db.rs` 测试模块

**id 策略说明（重要）：** `notes` 用 `INTEGER PRIMARY KEY AUTOINCREMENT`（与 models/prompts 一致），**不用** transcriptions/clipboard_history 的毫秒戳 id。原因：notes 有独立 `created_at`/`updated_at` 列承载时间，id 无需兼任时间戳；AUTOINCREMENT 无碰撞，FTS content_rowid 兼容。新建用 `conn.last_insert_rowid()` 取 id。

- [ ] **Step 1: 写失败测试 — notes 表 + FTS 迁移**

在 `crates/infra/src/db.rs` 的 `mod tests`（约第 1105 行 `#[cfg(test)] mod tests`）末尾追加测试。先写测试，验证当前（未加表）会失败：
```rust
    #[test]
    fn notes_table_and_fts_created() {
        let conn = open_init();
        // notes 表存在
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM notes", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 0);
        // notes_fts 虚表存在
        let fts_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM notes_fts", [], |r| r.get(0))
            .unwrap();
        assert_eq!(fts_count, 0);
    }
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test --manifest-path crates/infra/Cargo.toml notes_table_and_fts_created`
Expected: FAIL（`no such table: notes`）

- [ ] **Step 3: db.sql 追加 notes 表 + FTS + 触发器**

在 `crates/infra/src/db.sql` 末尾（第 287 行 `screenshot_shortcut` seed 之后）追加：
```sql

-- ── 记事本（notes 表）─────────────────────────────────────────────────────
-- 内容收集箱：ASR/OCR/剪贴板结果一键存入 + 富文本整理。
-- 富文本 content_html 为内部模型（TipTap getHTML），content_text 为抽取纯文本（FTS + 列表预览）。
CREATE TABLE IF NOT EXISTS notes (
    id            INTEGER PRIMARY KEY AUTOINCREMENT,
    title         TEXT,                             -- 可空，空则列表显示正文截取
    content_html  TEXT    NOT NULL DEFAULT '',      -- 富文本内部格式（TipTap getHTML）
    content_text  TEXT    NOT NULL DEFAULT '',      -- 纯文本抽取，FTS 索引 + 列表预览
    source        TEXT    NOT NULL DEFAULT 'manual', -- asr/ocr/clipboard/manual
    source_ref_id INTEGER,                          -- 关联 transcription_id 或 clipboard_history.id（应用层校验，无 DB 级约束）
    is_pinned     INTEGER NOT NULL DEFAULT 0,
    is_favorite   INTEGER NOT NULL DEFAULT 0,
    created_at    TEXT    NOT NULL,
    updated_at    TEXT    NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_notes_updated ON notes(updated_at DESC, id DESC);
CREATE INDEX IF NOT EXISTS idx_notes_source  ON notes(source);

-- FTS5 全文索引（trigram，CJK 子串匹配，仿 clipboard_history_fts）
CREATE VIRTUAL TABLE IF NOT EXISTS notes_fts USING fts5(
    title, content_text,
    content='notes', content_rowid='id', tokenize='trigram'
);

CREATE TRIGGER IF NOT EXISTS note_fts_ai AFTER INSERT ON notes BEGIN
    INSERT INTO notes_fts(rowid, title, content_text) VALUES (new.id, new.title, new.content_text);
END;
CREATE TRIGGER IF NOT EXISTS note_fts_ad AFTER DELETE ON notes BEGIN
    INSERT INTO notes_fts(notes_fts, rowid, title, content_text)
    VALUES('delete', old.id, old.title, old.content_text);
END;
CREATE TRIGGER IF NOT EXISTS note_fts_au AFTER UPDATE OF title, content_text ON notes BEGIN
    INSERT INTO notes_fts(notes_fts, rowid, title, content_text)
    VALUES('delete', old.id, old.title, old.content_text);
    INSERT INTO notes_fts(rowid, title, content_text) VALUES (new.id, new.title, new.content_text);
END;
```

- [ ] **Step 4: db.rs init_schema 加 v8→v9 分支**

`crates/infra/src/db.rs`，在 `init_schema` 的 `else if v == 7 { ... }` 块（约第 193-211 行）之后、`Ok(())`（第 212 行）之前，插入 v8 分支：
```rust
    } else if v == 8 {
        // v8 → v9：notes / notes_fts 表 + 触发器（记事本功能）。
        // INIT_SQL 已含 notes 建表（幂等 CREATE ... IF NOT EXISTS），重跑即给现存 v8 库补建。
        log::info!("DB migrating v8 → v9: adding notes + notes_fts...");
        conn.execute_batch(INIT_SQL).context("v8→v9: 建 notes + notes_fts 表")?;
        conn.execute("PRAGMA user_version = 9", [])?;
        log::info!("DB migrated to v9: notes + notes_fts");
    }
```

- [ ] **Step 5: fresh install 直接落到 v9**

`crates/infra/src/db.rs`，把 `if v < 2` 分支里的 `conn.execute("PRAGMA user_version = 8", [])?;`（约第 153 行）改为 `= 9`，并更新其上一行 log：
```rust
        conn.execute("PRAGMA user_version = 9", [])?;
        log::info!("DB initialized (v9): schema + app_config(setting) + prompts + clipboard_history + image_data + notes + yaml migration");
```

> 说明：v2-v7 分支仍设 `= 8`（这些是极旧的 legacy 库），下次启动会命中新的 `else if v == 8` 分支补建 notes 落到 v9，两步自愈。dev 阶段策略是删 `~/.octopus/octopus.db` 重建（见 db.sql 头注释），当前开发库都在 v8，本迁移一步到位。

- [ ] **Step 6: 跑测试确认通过**

Run: `cargo test --manifest-path crates/infra/Cargo.toml notes_table_and_fts_created`
Expected: PASS

- [ ] **Step 7: 跑 infra 全量测试确认无回归**

Run: `cargo test --manifest-path crates/infra/Cargo.toml`
Expected: 全绿（既有 init_sql_is_idempotent 等不受影响——notes 是纯新增）

- [ ] **Step 8: 提交**

```bash
git add Cargo.toml crates/notepad/Cargo.toml crates/notepad/src/lib.rs crates/desktop/Cargo.toml crates/infra/src/db.sql crates/infra/src/db.rs
git commit -m "feat(notepad): 新建 octopus-notepad crate 骨架 + notes/notes_fts 表与 v9 迁移"
```

---

## Task 3: notepad model.rs

**Files:**
- Create: `crates/notepad/src/model.rs`

- [ ] **Step 1: 写失败测试 — NoteSource 往返**

`crates/notepad/src/model.rs`：
```rust
use serde::{Deserialize, Serialize};

/// 笔记来源（决定徽标 + 溯源回溯目标）。
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum NoteSource {
    Asr,
    Ocr,
    Clipboard,
    #[default]
    Manual,
}

impl NoteSource {
    pub fn as_str(&self) -> &'static str {
        match self {
            NoteSource::Asr => "asr",
            NoteSource::Ocr => "ocr",
            NoteSource::Clipboard => "clipboard",
            NoteSource::Manual => "manual",
        }
    }

    pub fn from_str(s: &str) -> Self {
        match s {
            "asr" => NoteSource::Asr,
            "ocr" => NoteSource::Ocr,
            "clipboard" => NoteSource::Clipboard,
            _ => NoteSource::Manual,
        }
    }
}

/// 一条笔记（DB notes 表一行）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Note {
    pub id: i64,
    pub title: Option<String>,
    pub content_html: String,
    pub content_text: String,
    pub source: NoteSource,
    pub source_ref_id: Option<i64>,
    pub is_pinned: bool,
    pub is_favorite: bool,
    pub created_at: String,
    pub updated_at: String,
}

/// 列表查询过滤 + 分页。
#[derive(Debug, Clone, Default)]
pub struct NoteFilter {
    pub source: Option<NoteSource>,
    pub favorite: bool,
    pub pinned: bool,
    /// None 或 <3 字符 → LIKE 子串；≥3 字符 → FTS5 phrase MATCH
    pub search: Option<String>,
    pub limit: i64,
    pub offset: i64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn note_source_roundtrip() {
        for s in [NoteSource::Asr, NoteSource::Ocr, NoteSource::Clipboard, NoteSource::Manual] {
            assert_eq!(NoteSource::from_str(s.as_str()), s);
        }
    }

    #[test]
    fn note_source_from_unknown_defaults_manual() {
        assert_eq!(NoteSource::from_str("???"), NoteSource::Manual);
    }

    #[test]
    fn note_source_default_is_manual() {
        assert_eq!(NoteSource::default(), NoteSource::Manual);
    }
}
```

- [ ] **Step 2: 跑测试确认通过**

Run: `cargo test --manifest-path crates/notepad/Cargo.toml model::`
Expected: PASS（model.rs 自包含，可独立编译——但 lib.rs 还引用未创建的 serialize/store/export，需先建空模块文件）

> 若编译报 `file not found for module`，临时建空文件 `serialize.rs`/`store.rs`/`export.rs`（内容 `// placeholder, filled in later tasks`），Task 4/5/6 会覆写。

- [ ] **Step 3: 提交**

```bash
git add crates/notepad/src/model.rs crates/notepad/src/serialize.rs crates/notepad/src/store.rs crates/notepad/src/export.rs
git commit -m "feat(notepad): model.rs — NoteSource/Note/NoteFilter 数据结构"
```

---

## Task 4: notepad serialize.rs（HTML→纯文本）

**Files:**
- Create（覆写）: `crates/notepad/src/serialize.rs`

- [ ] **Step 1: 写失败测试**

`crates/notepad/src/serialize.rs`：
```rust
//! content_html → 纯文本抽取：scraper 解析，块级元素间加换行，<img> 转「[图片]」。
//! 后端为 content_text 的 source of truth（前端 update_note 只传 content_html）。

use scraper::{Html, Selector};

/// 把富文本 HTML 抽取为纯文本（FTS 索引 + 列表预览用）。
///
/// 规则：按 DOM 顺序遍历，块级元素（p/h1-6/li/blockquote/div）之间插入换行；
/// `<br>` 转换行；`<img>` 转「[图片]」；其余取 `.text()` 拼接；折叠多余空白。
pub fn extract_text(html: &str) -> String {
    if html.trim().is_empty() {
        return String::new();
    }
    let fragment = Html::parse_fragment(html);
    // TipTap 块级输出为扁平兄弟（p/h1-6/li/blockquote），不含 div 嵌套，
    // 故按这些标签逐块取文本、块间换行即可。<img> 转「[图片]」，<br> 忽略（块间已换行）。
    let block_sel = Selector::parse("p, h1, h2, h3, h4, h5, h6, li, blockquote, br, img").unwrap();

    let mut blocks: Vec<String> = Vec::new();
    for el in fragment.select(&block_sel) {
        let tag = el.value().name();
        if tag == "img" {
            blocks.push("[图片]".to_string());
        } else if tag == "br" {
            // 块间已是 \n 分隔，br 不额外产生空块
        } else {
            let text: String = el.text().collect::<Vec<_>>().join("");
            if !text.trim().is_empty() {
                blocks.push(text);
            }
        }
    }

    // 无任何块级命中（裸文本 HTML）→ 取整段文本
    let joined = if blocks.is_empty() {
        fragment.root_element().text().collect::<Vec<_>>().join("")
    } else {
        blocks.join("\n")
    };

    // 折叠连续空白（保留换行），trim 首尾
    collapse_whitespace(&joined)
}

fn collapse_whitespace(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut prev_nl = false;
    for line in s.lines() {
        let trimmed: String = line.split_whitespace().collect::<Vec<_>>().join(" ");
        if trimmed.is_empty() {
            if !prev_nl && !out.is_empty() {
                out.push('\n');
                prev_nl = true;
            }
        } else {
            if !out.is_empty() {
                out.push('\n');
            }
            out.push_str(&trimmed);
            prev_nl = false;
        }
    }
    out.trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_html_returns_empty() {
        assert_eq!(extract_text(""), "");
        assert_eq!(extract_text("   "), "");
    }

    #[test]
    fn single_paragraph() {
        assert_eq!(extract_text("<p>你好世界</p>"), "你好世界");
    }

    #[test]
    fn multiple_blocks_get_newlines() {
        let html = "<h1>标题</h1><p>第一段</p><p>第二段</p>";
        assert_eq!(extract_text(html), "标题\n第一段\n第二段");
    }

    #[test]
    fn img_becomes_placeholder() {
        assert_eq!(extract_text(r#"<p>前</p><img src="note-img:abc" alt="x"><p>后</p>"#), "前\n[图片]\n后");
    }

    #[test]
    fn bare_text_html() {
        assert_eq!(extract_text("裸文本内容"), "裸文本内容");
    }

    #[test]
    fn nested_list_items() {
        let html = "<ul><li>一项</li><li>二项</li></ul>";
        assert_eq!(extract_text(html), "一项\n二项");
    }

    #[test]
    fn list_and_paragraph_mix() {
        let html = "<p>引言</p><ul><li>A</li><li>B</li></ul><p>结语</p>";
        assert_eq!(extract_text(html), "引言\nA\nB\n结语");
    }
}
```

> 注意：`scraper` 的 `ElementRef::text()` 返回 `&str` 迭代器（`.text()`），`.collect::<Vec<_>>().join("")` 取块内文本。`Selector::parse` 在 `"tag1, tag2, ..."` 多选择器下按文档序返回。Step 2 跑测试确认编译与行为。

- [ ] **Step 2: 跑测试确认通过**

Run: `cargo test --manifest-path crates/notepad/Cargo.toml serialize::`
Expected: PASS。若编译失败（scraper API），按报错调整 trait 实现，重跑直到 7 个测试全绿。

- [ ] **Step 3: 提交**

```bash
git add crates/notepad/src/serialize.rs
git commit -m "feat(notepad): serialize.rs — HTML→纯文本抽取（scraper）"
```

---

## Task 5: notepad store.rs（CRUD + FTS + count + toggle + delete）

**Files:**
- Create（覆写）: `crates/notepad/src/store.rs`

- [ ] **Step 1: 写 store.rs 实现**

`crates/notepad/src/store.rs`：
```rust
//! notes 表 CRUD + FTS5 搜索 + 排序分页。全部经 `octopus_infra::db::with_db`。
//! 时间戳助手复用 infra 风格（手写，无 chrono 依赖）。

use anyhow::{Context, Result};
use rusqlite::{params, Connection};

use crate::model::{Note, NoteFilter, NoteSource};
use crate::serialize::extract_text;

// ── 时间辅助（与 infra/clipboard 一致的手写实现，避免 chrono 依赖）──

pub fn iso_now() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let (y, mo, d, h, mi, s) = epoch_to_ymd_hms(secs);
    format!("{:04}-{:02}-{:02} {:02}:{:02}:{:02}", y, mo, d, h, mi, s)
}

pub fn epoch_to_ymd_hms(secs: u64) -> (u32, u32, u32, u32, u32, u32) {
    let days = (secs / 86400) as u32;
    let remainder = (secs % 86400) as u32;
    let h = remainder / 3600;
    let mi = (remainder % 3600) / 60;
    let s = remainder % 60;

    let mut year = 1970u32;
    let mut remaining_days = days;
    loop {
        let leap = (year % 4 == 0 && year % 100 != 0) || year % 400 == 0;
        let year_days = if leap { 366 } else { 365 };
        if remaining_days >= year_days {
            remaining_days -= year_days;
            year += 1;
        } else {
            break;
        }
    }
    let leap = (year % 4 == 0 && year % 100 != 0) || year % 400 == 0;
    let month_days = [31, if leap { 29 } else { 28 }, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
    let mut month = 1u32;
    for &md in &month_days {
        if remaining_days < md { break; }
        remaining_days -= md;
        month += 1;
    }
    (year, month, remaining_days + 1, h, mi, s)
}

// ── CRUD ──

/// 列表查询（filter + FTS/LIKE 搜索 + 排序 + 分页）。
pub fn list_notes(filter: &NoteFilter) -> Result<Vec<Note>> {
    octopus_infra::db::with_db(|conn| list_notes_at(conn, filter))
}

pub fn list_notes_at(conn: &Connection, filter: &NoteFilter) -> Result<Vec<Note>> {
    let limit = if filter.limit > 0 { filter.limit } else { 50 };
    let offset = filter.offset.max(0);
    let where_clause = build_where(filter);

    if let Some(ref search) = filter.search {
        if !search.is_empty() {
            return query_with_search(conn, search, &where_clause, limit, offset);
        }
    }

    let sql = format!(
        "SELECT id, title, content_html, content_text, source, source_ref_id,
                is_pinned, is_favorite, created_at, updated_at
         FROM notes
         {}
         ORDER BY is_pinned DESC, updated_at DESC, id DESC
         LIMIT ? OFFSET ?",
        if where_clause.is_empty() { String::new() } else { format!("WHERE {}", where_clause) }
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(params![limit, offset], row_to_note)?;
    Ok(rows.filter_map(|r| r.ok()).collect())
}

fn query_with_search(
    conn: &Connection,
    search: &str,
    extra_where: &str,
    limit: i64,
    offset: i64,
) -> Result<Vec<Note>> {
    // <3 字符（trigram 无法成 token）→ LIKE fallback（title 或 content_text 子串）
    if search.chars().count() < 3 {
        let pattern = format!("%{}%", search);
        let sql = format!(
            "SELECT id, title, content_html, content_text, source, source_ref_id,
                    is_pinned, is_favorite, created_at, updated_at
             FROM notes
             WHERE (content_text LIKE ? OR IFNULL(title,'') LIKE ?)
             {}
             ORDER BY is_pinned DESC, updated_at DESC, id DESC
             LIMIT ? OFFSET ?",
            if extra_where.is_empty() { String::new() } else { format!("AND {}", extra_where) }
        );
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map(params![pattern, pattern, limit, offset], row_to_note)?;
        return Ok(rows.filter_map(|r| r.ok()).collect());
    }

    // ≥3 字符 → FTS5 phrase MATCH（title + content_text 联合索引）
    let phrase = format!("\"{}\"", search.replace('"', "\"\""));
    let sql = format!(
        "SELECT n.id, n.title, n.content_html, n.content_text, n.source, n.source_ref_id,
                n.is_pinned, n.is_favorite, n.created_at, n.updated_at
         FROM notes_fts f JOIN notes n ON n.id = f.rowid
         WHERE notes_fts MATCH ?
         {}
         ORDER BY n.is_pinned DESC, n.updated_at DESC, n.id DESC
         LIMIT ? OFFSET ?",
        if extra_where.is_empty() { String::new() } else { format!("AND {}", extra_where) }
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(params![phrase, limit, offset], row_to_note)?;
    Ok(rows.filter_map(|r| r.ok()).collect())
}

/// 计数（与 list_notes 同 filter/搜索逻辑，保证「共 N 条」一致）。
pub fn count_notes(filter: &NoteFilter) -> Result<i64> {
    octopus_infra::db::with_db(|conn| count_notes_at(conn, filter))
}

pub fn count_notes_at(conn: &Connection, filter: &NoteFilter) -> Result<i64> {
    let where_clause = build_where(filter);
    if let Some(ref search) = filter.search {
        if !search.is_empty() {
            return count_with_search(conn, search, &where_clause);
        }
    }
    let sql = if where_clause.is_empty() {
        "SELECT COUNT(*) FROM notes".to_string()
    } else {
        format!("SELECT COUNT(*) FROM notes WHERE {}", where_clause)
    };
    let count: i64 = conn.query_row(&sql, [], |r| r.get(0))?;
    Ok(count)
}

fn count_with_search(conn: &Connection, search: &str, extra_where: &str) -> Result<i64> {
    if search.chars().count() < 3 {
        let pattern = format!("%{}%", search);
        let sql = if extra_where.is_empty() {
            "SELECT COUNT(*) FROM notes WHERE (content_text LIKE ? OR IFNULL(title,'') LIKE ?)".to_string()
        } else {
            format!("SELECT COUNT(*) FROM notes WHERE (content_text LIKE ? OR IFNULL(title,'') LIKE ?) AND {}", extra_where)
        };
        let count: i64 = conn.query_row(&sql, params![pattern, pattern], |r| r.get(0))?;
        return Ok(count);
    }
    let phrase = format!("\"{}\"", search.replace('"', "\"\""));
    let sql = if extra_where.is_empty() {
        "SELECT COUNT(*) FROM notes_fts f JOIN notes n ON n.id = f.rowid WHERE notes_fts MATCH ?".to_string()
    } else {
        format!("SELECT COUNT(*) FROM notes_fts f JOIN notes n ON n.id = f.rowid WHERE notes_fts MATCH ? AND {}", extra_where)
    };
    let count: i64 = conn.query_row(&sql, params![phrase], |r| r.get(0))?;
    Ok(count)
}

fn build_where(filter: &NoteFilter) -> String {
    let mut conds: Vec<String> = Vec::new();
    if let Some(src) = filter.source {
        conds.push(format!("source = '{}'", src.as_str()));
    }
    if filter.favorite {
        conds.push("is_favorite = 1".to_string());
    }
    if filter.pinned {
        conds.push("is_pinned = 1".to_string());
    }
    conds.join(" AND ")
}

fn row_to_note(row: &rusqlite::Row) -> rusqlite::Result<Note> {
    let source_str: String = row.get(4)?;
    Ok(Note {
        id: row.get(0)?,
        title: row.get(1)?,
        content_html: row.get(2)?,
        content_text: row.get(3)?,
        source: NoteSource::from_str(&source_str),
        source_ref_id: row.get(5)?,
        is_pinned: row.get::<_, i64>(6)? != 0,
        is_favorite: row.get::<_, i64>(7)? != 0,
        created_at: row.get(8)?,
        updated_at: row.get(9)?,
    })
}

/// 按 id 读取单条。
pub fn get_note(id: i64) -> Result<Option<Note>> {
    octopus_infra::db::with_db(|conn| get_note_at(conn, id))
}

pub fn get_note_at(conn: &Connection, id: i64) -> Result<Option<Note>> {
    let mut stmt = conn.prepare(
        "SELECT id, title, content_html, content_text, source, source_ref_id,
                is_pinned, is_favorite, created_at, updated_at
         FROM notes WHERE id = ?",
    )?;
    match stmt.query_row(params![id], row_to_note) {
        Ok(note) => Ok(Some(note)),
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
        Err(e) => Err(e.into()),
    }
}

/// 新建笔记。content_text 由 initial_html 抽取；title 初始为 NULL。
/// 返回新 id（AUTOINCREMENT last_insert_rowid）。
pub fn create_note(source: NoteSource, source_ref_id: Option<i64>, initial_html: &str) -> Result<i64> {
    octopus_infra::db::with_db(|conn| create_note_at(conn, source, source_ref_id, initial_html))
}

pub fn create_note_at(
    conn: &Connection,
    source: NoteSource,
    source_ref_id: Option<i64>,
    initial_html: &str,
) -> Result<i64> {
    let content_text = extract_text(initial_html);
    let now = iso_now();
    conn.execute(
        "INSERT INTO notes (title, content_html, content_text, source, source_ref_id, is_pinned, is_favorite, created_at, updated_at)
         VALUES (NULL, ?, ?, ?, ?, 0, 0, ?, ?)",
        params![initial_html, content_text, source.as_str(), source_ref_id, now, now],
    )
    .context("insert note")?;
    Ok(conn.last_insert_rowid())
}

/// 更新正文/标题。content_text 由 content_html 重抽；updated_at = now。
/// title 空串 → 存 NULL（列表显示用 content_text 截取）。
pub fn update_note(id: i64, title: &str, content_html: &str) -> Result<()> {
    octopus_infra::db::with_db(|conn| update_note_at(conn, id, title, content_html))
}

pub fn update_note_at(conn: &Connection, id: i64, title: &str, content_html: &str) -> Result<()> {
    let content_text = extract_text(content_html);
    let title_db: Option<&str> = if title.trim().is_empty() { None } else { Some(title) };
    conn.execute(
        "UPDATE notes SET title = ?, content_html = ?, content_text = ?, updated_at = ? WHERE id = ?",
        params![title_db, content_html, content_text, iso_now(), id],
    )?;
    Ok(())
}

/// 批量删除。返回实际删除行数。
pub fn delete_notes(ids: &[i64]) -> Result<usize> {
    if ids.is_empty() {
        return Ok(0);
    }
    octopus_infra::db::with_db(|conn| delete_notes_at(conn, ids))
}

pub fn delete_notes_at(conn: &Connection, ids: &[i64]) -> Result<usize> {
    if ids.is_empty() {
        return Ok(0);
    }
    let placeholders = ids.iter().map(|_| "?").collect::<Vec<_>>().join(",");
    let params: Vec<&dyn rusqlite::ToSql> = ids.iter().map(|id| id as &dyn rusqlite::ToSql).collect();
    let n = conn.execute(
        &format!("DELETE FROM notes WHERE id IN ({})", placeholders),
        params.as_slice(),
    )?;
    Ok(n)
}

pub fn toggle_pinned(id: i64) -> Result<()> {
    octopus_infra::db::with_db(|conn| {
        conn.execute(
            "UPDATE notes SET is_pinned = CASE is_pinned WHEN 0 THEN 1 ELSE 0 END WHERE id = ?",
            params![id],
        )?;
        Ok(())
    })
}

pub fn toggle_favorite(id: i64) -> Result<()> {
    octopus_infra::db::with_db(|conn| {
        conn.execute(
            "UPDATE notes SET is_favorite = CASE is_favorite WHEN 0 THEN 1 ELSE 0 END WHERE id = ?",
            params![id],
        )?;
        Ok(())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn open_test_db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        let sql = include_str!("../../infra/src/db.sql");
        conn.execute_batch(sql).unwrap();
        conn
    }

    fn f() -> NoteFilter {
        NoteFilter { source: None, favorite: false, pinned: false, search: None, limit: 50, offset: 0 }
    }

    #[test]
    fn create_and_get_roundtrip() {
        let conn = open_test_db();
        let id = create_note_at(&conn, NoteSource::Asr, Some(123), "<p>识别文本</p>").unwrap();
        assert!(id > 0);
        let note = get_note_at(&conn, id).unwrap().unwrap();
        assert_eq!(note.content_html, "<p>识别文本</p>");
        assert_eq!(note.content_text, "识别文本");
        assert_eq!(note.source, NoteSource::Asr);
        assert_eq!(note.source_ref_id, Some(123));
        assert!(note.title.is_none());
    }

    #[test]
    fn update_rextracts_text_and_handles_title() {
        let conn = open_test_db();
        let id = create_note_at(&conn, NoteSource::Manual, None, "").unwrap();
        update_note_at(&conn, id, "我的标题", "<p>第一段</p><p>第二段</p>").unwrap();
        let note = get_note_at(&conn, id).unwrap().unwrap();
        assert_eq!(note.title.as_deref(), Some("我的标题"));
        assert_eq!(note.content_text, "第一段\n第二段");
        // 空标题 → NULL
        update_note_at(&conn, id, "   ", "<p>x</p>").unwrap();
        let note = get_note_at(&conn, id).unwrap().unwrap();
        assert!(note.title.is_none());
    }

    #[test]
    fn fts_search_three_chars() {
        let conn = open_test_db();
        create_note_at(&conn, NoteSource::Manual, None, "<p>今天天气很好</p>").unwrap();
        create_note_at(&conn, NoteSource::Manual, None, "<p>不相关内容</p>").unwrap();
        let mut filter = f();
        filter.search = Some("今天天气".into()); // ≥3 字符 → FTS
        let rows = list_notes_at(&conn, &filter).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].content_text, "今天天气很好");
        assert_eq!(count_notes_at(&conn, &filter).unwrap(), 1);
    }

    #[test]
    fn like_fallback_short_query() {
        let conn = open_test_db();
        create_note_at(&conn, NoteSource::Manual, None, "<p>hello world</p>").unwrap();
        let mut filter = f();
        filter.search = Some("el".into()); // <3 字符 → LIKE
        let rows = list_notes_at(&conn, &filter).unwrap();
        assert_eq!(rows.len(), 1);
    }

    #[test]
    fn filter_by_source_and_favorite() {
        let conn = open_test_db();
        let a = create_note_at(&conn, NoteSource::Asr, None, "<p>a</p>").unwrap();
        let _b = create_note_at(&conn, NoteSource::Ocr, None, "<p>b</p>").unwrap();
        toggle_favorite(a).unwrap();

        let mut sf = f();
        sf.source = Some(NoteSource::Asr);
        assert_eq!(list_notes_at(&conn, &sf).unwrap().len(), 1);

        let mut ff = f();
        ff.favorite = true;
        assert_eq!(list_notes_at(&conn, &ff).unwrap().len(), 1);
    }

    #[test]
    fn pinned_sorts_first() {
        let conn = open_test_db();
        // 同一秒写入（iso_now 秒级精度），靠 is_pinned DESC 优先
        let first = create_note_at(&conn, NoteSource::Manual, None, "<p>first</p>").unwrap();
        let second = create_note_at(&conn, NoteSource::Manual, None, "<p>second</p>").unwrap();
        toggle_pinned(first).unwrap();
        let rows = list_notes_at(&conn, &f()).unwrap();
        // pinned 的 first 应在 second 之前（即便 second 更新更晚）
        assert_eq!(rows[0].id, first);
        assert_eq!(rows[1].id, second);
    }

    #[test]
    fn delete_batch_and_empty() {
        let conn = open_test_db();
        let ids: Vec<i64> = (0..3).map(|_| create_note_at(&conn, NoteSource::Manual, None, "<p>x</p>").unwrap()).collect();
        let n = delete_notes_at(&conn, &ids[0..2]).unwrap();
        assert_eq!(n, 2);
        assert_eq!(count_notes_at(&conn, &f()).unwrap(), 1);
        assert_eq!(delete_notes_at(&conn, &[]).unwrap(), 0);
    }

    #[test]
    fn fts_triggers_sync_on_update_and_delete() {
        let conn = open_test_db();
        let id = create_note_at(&conn, NoteSource::Manual, None, "<p>旧内容关键字</p>").unwrap();
        let mut filter = f();
        filter.search = Some("关键字".into());
        assert_eq!(count_notes_at(&conn, &filter).unwrap(), 1);
        // update 改掉关键字 → FTS 不再命中
        update_note_at(&conn, id, "", "<p>全新内容</p>").unwrap();
        assert_eq!(count_notes_at(&conn, &filter).unwrap(), 0);
        // delete → 计数归零
        delete_notes_at(&conn, &[id]).unwrap();
        let mut filter2 = f();
        filter2.search = Some("全新内容".into());
        assert_eq!(count_notes_at(&conn, &filter2).unwrap(), 0);
    }
}
```

- [ ] **Step 2: 跑 notepad 全量测试**

Run: `cargo test --manifest-path crates/notepad/Cargo.toml`
Expected: 全绿（model + serialize + store 共约 20 个测试）

- [ ] **Step 3: 编译整个 crate（确认 lib.rs 模块齐全）**

Run: `cargo build --manifest-path crates/notepad/Cargo.toml`
Expected: 编译通过（export 还是占位空文件，Task 6 填充）

- [ ] **Step 4: 提交**

```bash
git add crates/notepad/src/store.rs
git commit -m "feat(notepad): store.rs — notes CRUD + FTS5 搜索 + 排序分页 + toggle/delete"
```

---

## Task 6: notepad export.rs（文件 I/O）

**Files:**
- Create（覆写）: `crates/notepad/src/export.rs`

- [ ] **Step 1: 写 export.rs 实现**

`crates/notepad/src/export.rs`：
```rust
//! 导入/导出文件 I/O。落盘到 ~/Documents/octopus/notes/。
//! 格式转换（HTML↔md↔txt）在前端 TipTap，后端只读/写文件。

use anyhow::{Context, Result};
use std::path::PathBuf;

/// 导出根目录：~/Documents/octopus/notes/（跨平台 dirs::document_dir）。
pub fn notes_dir() -> Result<PathBuf> {
    let docs = dirs::document_dir().context("无法定位 Documents 目录")?;
    Ok(docs.join("octopus").join("notes"))
}

/// 把内容写到 ~/Documents/octopus/notes/<safe_stem>.<ext>。
/// stem 中的路径分隔符/非法字符替换为 `_`，避免目录穿越。
/// 文件名冲突时追加 `-2/-3`。返回写入的绝对路径。
pub fn write_export(filename_stem: &str, ext: &str, content: &str) -> Result<PathBuf> {
    let dir = notes_dir()?;
    std::fs::create_dir_all(&dir).with_context(|| format!("创建目录失败: {}", dir.display()))?;
    let safe_stem = sanitize_stem(filename_stem);
    let safe_ext = ext.trim_start_matches('.').to_lowercase();
    let path = unique_path(&dir, &safe_stem, &safe_ext);
    std::fs::write(&path, content)
        .with_context(|| format!("写入文件失败: {}", path.display()))?;
    Ok(path)
}

/// 读 .md 文件原文返回（md→HTML 解析在前端）。
pub fn read_import(path: &std::path::Path) -> Result<String> {
    std::fs::read_to_string(path)
        .with_context(|| format!("读取文件失败: {}", path.display()))
}

fn sanitize_stem(s: &str) -> String {
    let trimmed = s.trim();
    let stem = if trimmed.is_empty() { "note" } else { trimmed };
    stem.chars()
        .map(|c| {
            if c.is_ascii_control() || matches!(c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|') {
                '_'
            } else {
                c
            }
        })
        .collect::<String>()
        .trim()
        .to_string()
}

fn unique_path(dir: &std::path::Path, stem: &str, ext: &str) -> PathBuf {
    let first = dir.join(format!("{}.{}", stem, ext));
    if !first.exists() {
        return first;
    }
    for i in 2..1000 {
        let cand = dir.join(format!("{}-{}.{}", stem, i, ext));
        if !cand.exists() {
            return cand;
        }
    }
    dir.join(format!("{}.{}", stem, ext))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir() -> PathBuf {
        let d = std::env::temp_dir().join(format!("octopus-notepad-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    #[test]
    fn sanitize_replaces_path_chars() {
        assert_eq!(sanitize_stem("a/b\\c:d"), "a_b_c_d");
        assert_eq!(sanitize_stem("   "), "note");
        assert_eq!(sanitize_stem("正常标题"), "正常标题");
    }

    #[test]
    fn write_and_read_roundtrip() {
        let dir = temp_dir();
        std::fs::create_dir_all(&dir).unwrap();
        std::env::set_var("HOME", &dir); // dirs::document_dir 在 mac/linux 读 HOME
        // 注意：dirs::document_dir 在 macOS 读 ~/Documents，单测无法改HOME精准控制；
        // 故直接测 unique_path + sanitize 的组合逻辑，write_export 的端到端在集成测试覆盖。
        let p = unique_path(&dir, "我的笔记", "md");
        std::fs::write(&p, "# 标题\n正文").unwrap();
        assert_eq!(read_import(&p).unwrap(), "# 标题\n正文");
        // 冲突 → -2
        let p2 = unique_path(&dir, "我的笔记", "md");
        assert_ne!(p, p2);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
```

> 单测聚焦纯函数（sanitize / unique_path / read_import）；`write_export` 跨平台 Documents 目录定位在 Task 17 集成验证。

- [ ] **Step 2: 跑测试**

Run: `cargo test --manifest-path crates/notepad/Cargo.toml export::`
Expected: PASS

- [ ] **Step 3: 提交**

```bash
git add crates/notepad/src/export.rs
git commit -m "feat(notepad): export.rs — 导入读文件/导出写文件（~/Documents/octopus/notes/）"
```

---

## Task 7: infra get_transcription_display_text（save_transcription_to_note 用）

**Files:**
- Modify: `crates/infra/src/db.rs`（加 `get_transcription_display_text`）
- Test: 同文件测试模块

- [ ] **Step 1: 写失败测试**

在 `crates/infra/src/db.rs` 测试模块末尾追加：
```rust
    #[test]
    fn get_transcription_display_text_priority() {
        let conn = open_init();
        // 只有 raw → 返回 raw
        conn.execute(
            "INSERT INTO transcriptions (id, created_at, engine, raw_text, polish_status)
             VALUES (100, '2026-06-30 10:00:00', 'sensevoice', 'raw原文', 'off')",
            [],
        ).unwrap();
        assert_eq!(get_transcription_display_text_at(&conn, 100).unwrap(), Some("raw原文".to_string()));

        // 有 polished → 返回 polished
        conn.execute(
            "UPDATE transcriptions SET polished_text='润色稿' WHERE id=100", [],
        ).unwrap();
        assert_eq!(get_transcription_display_text_at(&conn, 100).unwrap(), Some("润色稿".to_string()));

        // 有 edited → edited 优先
        conn.execute(
            "UPDATE transcriptions SET edited_text='手改稿' WHERE id=100", [],
        ).unwrap();
        assert_eq!(get_transcription_display_text_at(&conn, 100).unwrap(), Some("手改稿".to_string()));

        // 不存在 → None
        assert_eq!(get_transcription_display_text_at(&conn, 9999).unwrap(), None);
    }
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test --manifest-path crates/infra/Cargo.toml get_transcription_display_text_priority`
Expected: FAIL（函数未定义）

- [ ] **Step 3: 实现**

在 `crates/infra/src/db.rs`，紧跟 `fn list_transcriptions_at`（约第 1050 行）之后插入：
```rust
/// 取某条识别记录的展示文本：edited_text ?? polished_text ?? raw_text。
/// 供 save_transcription_to_note 把语音结果转成笔记正文。不存在返回 None。
pub fn get_transcription_display_text(id: i64) -> Result<Option<String>> {
    with_db(|conn| get_transcription_display_text_at(conn, id))
}

fn get_transcription_display_text_at(conn: &Connection, id: i64) -> Result<Option<String>> {
    let row = conn.query_row(
        "SELECT edited_text, polished_text, raw_text FROM transcriptions WHERE id=?1",
        params![id],
        |r| {
            let edited: Option<String> = r.get(0)?;
            let polished: Option<String> = r.get(1)?;
            let raw: String = r.get(2)?;
            Ok(edited.or(polished).unwrap_or(raw))
        },
    );
    match row {
        Ok(text) => Ok(Some(text)),
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
        Err(e) => Err(e.into()),
    }
}
```

- [ ] **Step 4: 跑测试确认通过**

Run: `cargo test --manifest-path crates/infra/Cargo.toml get_transcription_display_text_priority`
Expected: PASS

- [ ] **Step 5: 提交**

```bash
git add crates/infra/src/db.rs
git commit -m "feat(infra): get_transcription_display_text — 笔记溯源取识别记录展示文本"
```

---

## Task 8: desktop note_commands.rs（薄命令层 + 集成入口 + get_note_image 桥接）

**Files:**
- Create: `crates/desktop/src/note_commands.rs`
- Modify: `crates/desktop/src/main.rs:5`（`mod note_commands;`）

- [ ] **Step 1: 写 note_commands.rs**

`crates/desktop/src/note_commands.rs`：
```rust
//! 记事本 Tauri 命令层：薄封装转调 octopus-notepad，写操作成功后 emit("notepad://changed")。
//! 图片 BLOB 桥接：notepad 不依赖 clipboard，图片获取/入库由本层桥接。

use base64::{engine::general_purpose, Engine};
use tauri::{Emitter, State};
use std::sync::Arc;

use octopus_clipboard::ClipboardHandle;
use octopus_notepad::{Note, NoteFilter, NoteSource};

// ── 基础 CRUD ──

#[tauri::command]
pub async fn list_notes(
    source: Option<String>,
    favorite: Option<bool>,
    pinned: Option<bool>,
    search: Option<String>,
    limit: Option<i64>,
    offset: Option<i64>,
) -> Result<Vec<Note>, String> {
    let filter = NoteFilter {
        source: source.as_deref().map(NoteSource::from_str),
        favorite: favorite.unwrap_or(false),
        pinned: pinned.unwrap_or(false),
        search,
        limit: limit.unwrap_or(50),
        offset: offset.unwrap_or(0),
    };
    octopus_infra::db::with_db(|conn| octopus_notepad::store::list_notes_at(conn, &filter))
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn count_notes(
    source: Option<String>,
    favorite: Option<bool>,
    pinned: Option<bool>,
    search: Option<String>,
) -> Result<i64, String> {
    let filter = NoteFilter {
        source: source.as_deref().map(NoteSource::from_str),
        favorite: favorite.unwrap_or(false),
        pinned: pinned.unwrap_or(false),
        search,
        limit: 1,
        offset: 0,
    };
    octopus_infra::db::with_db(|conn| octopus_notepad::store::count_notes_at(conn, &filter))
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn get_note(id: i64) -> Result<Option<Note>, String> {
    octopus_infra::db::with_db(|conn| octopus_notepad::store::get_note_at(conn, id))
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn create_note(
    source: String,
    source_ref_id: Option<i64>,
    initial_html: String,
    app_handle: tauri::AppHandle,
) -> Result<i64, String> {
    let id = octopus_infra::db::with_db(|conn| {
        octopus_notepad::store::create_note_at(conn, NoteSource::from_str(&source), source_ref_id, &initial_html)
    })
    .map_err(|e| e.to_string())?;
    let _ = app_handle.emit("notepad://changed", ());
    Ok(id)
}

#[tauri::command]
pub async fn update_note(
    id: i64,
    title: String,
    content_html: String,
    app_handle: tauri::AppHandle,
) -> Result<(), String> {
    octopus_infra::db::with_db(|conn| {
        octopus_notepad::store::update_note_at(conn, id, &title, &content_html)
    })
    .map_err(|e| e.to_string())?;
    let _ = app_handle.emit("notepad://changed", ());
    Ok(())
}

#[tauri::command]
pub async fn delete_notes(
    ids: Vec<i64>,
    app_handle: tauri::AppHandle,
) -> Result<usize, String> {
    let n = octopus_infra::db::with_db(|conn| octopus_notepad::store::delete_notes_at(conn, &ids))
        .map_err(|e| e.to_string())?;
    let _ = app_handle.emit("notepad://changed", ());
    Ok(n)
}

#[tauri::command]
pub async fn toggle_note_pinned(id: i64, app_handle: tauri::AppHandle) -> Result<(), String> {
    octopus_notepad::store::toggle_pinned(id).map_err(|e| e.to_string())?;
    let _ = app_handle.emit("notepad://changed", ());
    Ok(())
}

#[tauri::command]
pub async fn toggle_note_favorite(id: i64, app_handle: tauri::AppHandle) -> Result<(), String> {
    octopus_notepad::store::toggle_favorite(id).map_err(|e| e.to_string())?;
    let _ = app_handle.emit("notepad://changed", ());
    Ok(())
}

// ── 导入/导出 ──

#[tauri::command]
pub async fn export_note(stem: String, ext: String, content: String) -> Result<String, String> {
    let path = octopus_notepad::export::write_export(&stem, &ext, &content)
        .map_err(|e| e.to_string())?;
    Ok(path.to_string_lossy().to_string())
}

#[tauri::command]
pub async fn import_note_from_file(path: String) -> Result<String, String> {
    octopus_notepad::export::read_import(std::path::Path::new(&path))
        .map_err(|e| e.to_string())
}

// ── 图片桥接（notepad 不依赖 clipboard）──

/// 取笔记内嵌图片：hash → image_data BLOB → data:image/webp;base64,...（仿 get_image_thumb 手法，避免 IPC 字节数组膨胀）。
#[tauri::command]
pub async fn get_note_image(hash: String) -> Result<String, String> {
    let blob = octopus_infra::db::with_db(|conn| {
        octopus_clipboard::store::get_image_blob(conn, &hash)
    })
    .map_err(|e| e.to_string())?
    .ok_or("图片数据不存在")?;
    Ok(format!("data:image/webp;base64,{}", general_purpose::STANDARD.encode(&blob)))
}

/// 编辑器插入图片：选中的图片文件 → 编码 WebP + 缩略图 + sha256(PNG) 入库 → 返回 hash。
/// 前端拿到 hash 后插入 `<img src="note-img:<hash>">` 节点。
#[tauri::command]
pub async fn insert_note_image(path: String) -> Result<String, String> {
    let bytes = std::fs::read(&path).map_err(|e| format!("读取图片失败: {}", e))?;
    let img = ::image::load_from_memory(&bytes).map_err(|e| format!("解码图片失败: {}", e))?;
    // image_data.hash 约定 = sha256(PNG bytes)（见 db.sql image_data 注释 + clipboard encode_and_hash）。
    // encode_to_webp 取 WebP 原图+缩略图 BLOB；encode_and_hash 取 PNG sha256 作去重键。
    let encoded = octopus_clipboard::image::encode_to_webp(&img).map_err(|e| e.to_string())?;
    let rgba = img.to_rgba8();
    let (_png_bytes, hash) = octopus_clipboard::image::encode_and_hash(
        rgba.as_raw(),          // RgbaImage.as_raw() -> &Vec<u8>，自动 deref 成 &[u8]
        rgba.width(),
        rgba.height(),
    ).map_err(|e| e.to_string())?;
    let width = img.width() as i64;
    let height = img.height() as i64;
    octopus_infra::db::with_db(|conn| {
        octopus_clipboard::store::insert_image_data(conn, &hash, &encoded.webp_blob, &encoded.thumb_blob, width, height)
    })
    .map_err(|e| e.to_string())?;
    Ok(hash)
}

// ── 集成入口：识别结果 → 笔记 ──

/// 语音结果 → 新建笔记：查 transcriptions 取展示文本 → <p> 包裹 → create_note(Asr, Some(id))。
#[tauri::command]
pub async fn save_transcription_to_note(
    transcription_id: i64,
    app_handle: tauri::AppHandle,
) -> Result<i64, String> {
    let text = octopus_infra::db::get_transcription_display_text(transcription_id)
        .map_err(|e| e.to_string())?
        .ok_or("原识别记录不存在")?;
    let html = format!("<p>{}</p>", html_escape(&text));
    let id = octopus_notepad::store::create_note(NoteSource::Asr, Some(transcription_id), &html)
        .map_err(|e| e.to_string())?;
    let _ = app_handle.emit("notepad://changed", ());
    Ok(id)
}

/// 剪贴板条目 → 新建笔记：文本转 <p>；图片转 <img src="note-img:<hash>">。
#[tauri::command]
pub async fn save_clipboard_to_note(
    item_id: i64,
    app_handle: tauri::AppHandle,
) -> Result<i64, String> {
    let item = octopus_infra::db::with_db(|conn| octopus_clipboard::store::get_item_by_id(conn, item_id))
        .map_err(|e| e.to_string())?
        .ok_or("原剪贴板记录不存在")?;
    let html = match item.item_type {
        octopus_clipboard::ItemType::Image => {
            let hash = item.image_meta.as_ref().map(|m| m.blob_hash.as_str()).unwrap_or("");
            format!(r#"<img src="note-img:{}" alt="剪贴板图片">"#, hash)
        }
        _ => format!("<p>{}</p>", html_escape(&item.content)),
    };
    let id = octopus_notepad::store::create_note(NoteSource::Clipboard, Some(item_id), &html)
        .map_err(|e| e.to_string())?;
    let _ = app_handle.emit("notepad://changed", ());
    Ok(id)
}

/// OCR 结果 → 新建笔记：text → <p> 包裹 → create_note(Ocr, None)。
#[tauri::command]
pub async fn save_ocr_to_note(text: String, app_handle: tauri::AppHandle) -> Result<i64, String> {
    let html = format!("<p>{}</p>", html_escape(&text));
    let id = octopus_notepad::store::create_note(NoteSource::Ocr, None, &html)
        .map_err(|e| e.to_string())?;
    let _ = app_handle.emit("notepad://changed", ());
    Ok(id)
}

/// 转义 HTML 特殊字符（识别文本/剪贴板内容插入笔记时，避免被当 HTML 解析）。
fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('\n', "</p><p>")
}

// 抑制未用警告（ClipboardHandle 在本模块导入是为未来扩展预留；当前未直接用）。
#[allow(dead_code)]
type _UnusedHandle = Arc<ClipboardHandle>;
```

> 说明：`insert_note_image` 里 `hash` 计算——image_data.hash 约定 = sha256(PNG bytes)（见 db.sql `image_data` 注释 + clipboard `encode_and_hash`）。复用 `encode_and_hash(rgba)` 取 hash，保证与剪贴板图片去重一致（同一张图在剪贴板和笔记间共享一份 BLOB）。`encode_to_webp` 取 WebP/缩略图 blob。两步编码（PNG 求 hash + WebP 求 blob）与 clipboard watcher 路径一致。

- [ ] **Step 2: main.rs 注册模块**

`crates/desktop/src/main.rs`，在 `mod clipboard_commands;`（第 5 行）后加：
```rust
mod note_commands;
```

- [ ] **Step 3: 编译 note_commands（暂不注册到 invoke_handler，Task 11 统一注册）**

Run: `cargo build --manifest-path crates/desktop/Cargo.toml`
Expected: 编译通过（命令函数已定义但未注册不影响编译；未注册的命令调用会运行时报错，Task 11 注册）

> 若报 `ClipboardHandle` 未用等 warning 不影响。若报 `encode_and_hash`/`encode_to_webp` 可见性错误，确认 `octopus_clipboard::image` 模块为 `pub`（clipboard lib.rs 已 `pub mod image`）。

- [ ] **Step 4: 提交**

```bash
git add crates/desktop/src/note_commands.rs crates/desktop/src/main.rs
git commit -m "feat(notepad): note_commands.rs — 薄命令层 + 集成入口 + get_note_image/insert_note_image 桥接"
```

---

## Task 9: coordinator current_transcription_id（Result 窗口溯源）

**Files:**
- Modify: `crates/desktop/src/coordinator.rs`（静态 + setter + 3 处会话起点 + 命令）
- Modify: `crates/desktop/src/main.rs`（`use` 原子类型，如需）

> 背景：Result 窗口前端只收到文本事件（`show-result`/`update-result`），拿不到 transcription_id。方案：coordinator 用 `AtomicI64` 记录当前会话 id，暴露 `current_transcription_id` 命令；Result 窗口存入按钮先取 id 再调 `save_transcription_to_note`。id 在 3 个会话起点（Streaming×2 / VadSegmented）写入，不在 mem::replace 重置（id=0 sentinel）处写，故保存「最近一次有效 id」。

- [ ] **Step 1: coordinator 加静态 + setter**

`crates/desktop/src/coordinator.rs`，在文件顶部 `use` 区附近（约第 1-15 行 `use` 之后）加：
```rust
use std::sync::atomic::AtomicI64;

/// 当前/最近一次录音会话的 transcription_id。
/// 在会话起点（Transcript::new）写入，供 Result 窗口「存入记事本」溯源。
/// 不在 mem::replace（id=0 sentinel）处清除 → 保留最近有效 id，粘贴后短时间内仍可保存。
static CURRENT_TRANSCRIPTION_ID: AtomicI64 = AtomicI64::new(0);

pub(crate) fn set_current_transcription_id(id: i64) {
    CURRENT_TRANSCRIPTION_ID.store(id, std::sync::atomic::Ordering::Relaxed);
}

/// Result 窗口取当前/最近 transcription_id（无会话返回 None）。
#[tauri::command]
pub async fn current_transcription_id() -> Option<i64> {
    let id = CURRENT_TRANSCRIPTION_ID.load(std::sync::atomic::Ordering::Relaxed);
    if id > 0 { Some(id) } else { None }
}
```

> 若 `AtomicI64` 已在文件其他 `use` 中导入，避免重复导入；按编译器提示合并。

- [ ] **Step 2: 3 处会话起点写入 id**

`crates/desktop/src/coordinator.rs`，定位 3 处 `transcript: Transcript::new(now_millis(), config.polish_mode)`（约第 590、678、701 行）。每处改为先取 id、写静态、再构造。例如第 588-592 行（cloud streaming）：

改前：
```rust
                            *stage = Stage::Streaming {
                                pipeline,
                                transcript: Transcript::new(now_millis(), config.polish_mode),
                                streaming_active: tick_active,
                            };
```
改后：
```rust
                            let tid = now_millis();
                            set_current_transcription_id(tid);
                            *stage = Stage::Streaming {
                                pipeline,
                                transcript: Transcript::new(tid, config.polish_mode),
                                streaming_active: tick_active,
                            };
```

对第 676-680 行（local streaming）和第 699-703 行（VadSegmented）做同样改动（变量名用 `tid`，各自替换 `now_millis()` → `tid` 并在 `*stage = ...` 前加两行）。

> 不改第 769/789/815 等处的 `Transcript::new(0, ...)`（id=0 是 discard sentinel，不应写入静态）。

- [ ] **Step 3: 编译**

Run: `cargo build --manifest-path crates/desktop/Cargo.toml`
Expected: 编译通过

- [ ] **Step 4: 提交**

```bash
git add crates/desktop/src/coordinator.rs
git commit -m "feat(notepad): coordinator 记录 current_transcription_id 供 Result 窗口溯源"
```

---

## Task 10: notepad_window.rs + main.rs 窗口注册

**Files:**
- Create: `crates/desktop/src/notepad_window.rs`
- Modify: `crates/desktop/src/main.rs`（`mod notepad_window;` + macOS 关窗策略可选）

- [ ] **Step 1: 写 notepad_window.rs（仿 settings_window.rs）**

`crates/desktop/src/notepad_window.rs`：
```rust
//! 记事本窗口：独立 Tauri 窗口，原生标题栏，1000×680 可调大小，单例。
//! 仿 settings_window：已打开则 set_focus，不重复创建。
//! 位置记忆复用 settings_window 的窗口位置机制（MVP 暂用固定尺寸，位置记忆后续接 window_position）。

use tauri::{Manager, WebviewUrl, WebviewWindowBuilder};

const WINDOW_LABEL: &str = "notepad_window";
const WIDTH: f64 = 1000.0;
const HEIGHT: f64 = 680.0;
const MIN_WIDTH: f64 = 720.0;
const MIN_HEIGHT: f64 = 480.0;

/// 打开记事本窗口（单例：已存在则 set_focus）。
#[tauri::command]
pub fn open_notepad(app_handle: tauri::AppHandle) {
    if let Some(window) = app_handle.get_webview_window(WINDOW_LABEL) {
        let _ = window.set_focus();
        let _ = window.show();
        return;
    }
    // macOS: 记事本是内容编辑窗口，切到 Regular 让 Dock 显示图标（与 settings 一致）。
    #[cfg(target_os = "macos")]
    {
        let _ = app_handle.set_activation_policy(tauri::ActivationPolicy::Regular);
    }
    let _ = WebviewWindowBuilder::new(
        &app_handle,
        WINDOW_LABEL,
        WebviewUrl::default(),
    )
    .title("记事本")
    .inner_size(WIDTH, HEIGHT)
    .min_inner_size(MIN_WIDTH, MIN_HEIGHT)
    .decorations(true)
    .visible(true)
    .build();
}
```

- [ ] **Step 2: main.rs 注册模块**

`crates/desktop/src/main.rs`，在 `mod note_commands;`（Task 8 加的）后加：
```rust
mod notepad_window;
```

- [ ] **Step 3: 编译**

Run: `cargo build --manifest-path crates/desktop/Cargo.toml`
Expected: 编译通过

- [ ] **Step 4: 提交**

```bash
git add crates/desktop/src/notepad_window.rs crates/desktop/src/main.rs
git commit -m "feat(notepad): notepad_window.rs — 独立记事本窗口（单例，仿 settings_window）"
```

---

## Task 11: tray 菜单 + invoke_handler 注册 + App.tsx 路由

**Files:**
- Modify: `crates/desktop/src/tray.rs`（菜单项 + 事件）
- Modify: `crates/desktop/src/main.rs:181-239`（invoke_handler 注册所有命令）
- Modify: `crates/desktop/frontend/src/App.tsx`（路由）

- [ ] **Step 1: tray.rs 加「记事本」菜单项**

`crates/desktop/src/tray.rs`，在 `let clipboard = MenuItem::with_id(...)`（第 42-43 行）后加：
```rust
    let notepad = MenuItem::with_id(app, "notepad", "记事本", true, None::<&str>)
        .expect("failed to create notepad menu item");
```

并把 `Menu::with_items`（第 51 行）的数组加入 `&notepad`：
```rust
    let menu = Menu::with_items(app, &[&toggle, &engine_info, &clipboard, &notepad, &screenshot, &stop_scroll, &settings, &quit])
        .expect("failed to create tray menu");
```

在 `on_menu_event` 的 `"clipboard" => { ... }` 分支（第 79-82 行）后加：
```rust
            "notepad" => {
                info!("Tray: open notepad");
                crate::notepad_window::open_notepad(app.clone());
            }
```

- [ ] **Step 2: main.rs invoke_handler 注册全部 note 命令**

`crates/desktop/src/main.rs`，在 `screenshot_commands::set_cursor_passthrough,`（第 238 行）后、`]`（第 239 行）前追加：
```rust
            note_commands::list_notes,
            note_commands::count_notes,
            note_commands::get_note,
            note_commands::create_note,
            note_commands::update_note,
            note_commands::delete_notes,
            note_commands::toggle_note_pinned,
            note_commands::toggle_note_favorite,
            note_commands::export_note,
            note_commands::import_note_from_file,
            note_commands::get_note_image,
            note_commands::insert_note_image,
            note_commands::save_transcription_to_note,
            note_commands::save_clipboard_to_note,
            note_commands::save_ocr_to_note,
            notepad_window::open_notepad,
            coordinator::current_transcription_id,
```

- [ ] **Step 3: App.tsx 路由**

`crates/desktop/frontend/src/App.tsx`，在 import 区（第 5 行 `import Clipboard from "@/pages/Clipboard";` 后）加：
```tsx
import Notepad from "@/pages/Notepad";
```

在 switch（第 50-51 行 `case "clipboard_window": return <Clipboard />;` 后）加：
```tsx
          case "notepad_window":
            return <Notepad />;
```

- [ ] **Step 4: 编译 desktop（后端）**

Run: `cargo build --manifest-path crates/desktop/Cargo.toml`
Expected: 编译通过（前端 Notepad 页面尚未创建，vite build 会失败——前端在 Task 13 创建后才能 build。此处只验证后端编译）

- [ ] **Step 5: 提交**

```bash
git add crates/desktop/src/tray.rs crates/desktop/src/main.rs crates/desktop/frontend/src/App.tsx
git commit -m "feat(notepad): 托盘菜单「记事本」+ invoke_handler 注册命令 + App.tsx 路由"
```

---

## Task 12: 前端 lib/notepad.ts + types/note.ts + hooks/useNotes.ts

**Files:**
- Create: `crates/desktop/frontend/src/types/note.ts`
- Create: `crates/desktop/frontend/src/lib/notepad.ts`
- Create: `crates/desktop/frontend/src/hooks/useNotes.ts`

- [ ] **Step 1: types/note.ts**

`crates/desktop/frontend/src/types/note.ts`：
```ts
export type NoteSource = "asr" | "ocr" | "clipboard" | "manual";

export interface Note {
  id: number;
  title: string | null;
  content_html: string;
  content_text: string;
  source: NoteSource;
  source_ref_id: number | null;
  is_pinned: boolean;
  is_favorite: boolean;
  created_at: string;
  updated_at: string;
}

export interface NoteListParams {
  source?: NoteSource | null;
  favorite?: boolean;
  pinned?: boolean;
  search?: string | null;
  limit?: number;
  offset?: number;
}
```

- [ ] **Step 2: lib/notepad.ts**

`crates/desktop/frontend/src/lib/notepad.ts`：
```ts
import { invoke } from "@/lib/tauri";
import type { Note, NoteListParams, NoteSource } from "@/types/note";

export async function listNotes(params: NoteListParams): Promise<Note[]> {
  return invoke<Note[]>("list_notes", {
    source: params.source ?? null,
    favorite: params.favorite ?? false,
    pinned: params.pinned ?? false,
    search: params.search ?? null,
    limit: params.limit ?? 50,
    offset: params.offset ?? 0,
  });
}

export async function countNotes(params: NoteListParams): Promise<number> {
  return invoke<number>("count_notes", {
    source: params.source ?? null,
    favorite: params.favorite ?? false,
    pinned: params.pinned ?? false,
    search: params.search ?? null,
  });
}

export const getNote = (id: number) => invoke<Note | null>("get_note", { id });

export const createNote = (source: NoteSource, sourceRefId: number | null, initialHtml: string) =>
  invoke<number>("create_note", { source, sourceRefId, initialHtml });

export const updateNote = (id: number, title: string, contentHtml: string) =>
  invoke<void>("update_note", { id, title, contentHtml });

export const deleteNotes = (ids: number[]) => invoke<number>("delete_notes", { ids });

export const toggleNotePinned = (id: number) => invoke<void>("toggle_note_pinned", { id });
export const toggleNoteFavorite = (id: number) => invoke<void>("toggle_note_favorite", { id });

export const exportNote = (stem: string, ext: string, content: string) =>
  invoke<string>("export_note", { stem, ext, content });

export const importNoteFromFile = (path: string) =>
  invoke<string>("import_note_from_file", { path });

export const getNoteImage = (hash: string) => invoke<string>("get_note_image", { hash });
export const insertNoteImage = (path: string) => invoke<string>("insert_note_image", { path });

// 集成入口
export const currentTranscriptionId = () => invoke<number | null>("current_transcription_id");
export const saveTranscriptionToNote = (transcriptionId: number) =>
  invoke<number>("save_transcription_to_note", { transcriptionId });
export const saveClipboardToNote = (itemId: number) =>
  invoke<number>("save_clipboard_to_note", { itemId });
export const saveOcrToNote = (text: string) => invoke<number>("save_ocr_to_note", { text });
```

- [ ] **Step 3: hooks/useNotes.ts**

`crates/desktop/frontend/src/hooks/useNotes.ts`：
```ts
import { useState, useEffect, useCallback } from "react";
import { listNotes, countNotes } from "@/lib/notepad";
import { useTauriEvent } from "@/hooks/useTauriEvent";
import { useDebouncedValue } from "@/hooks/useClipboardHistory";
import type { Note, NoteSource } from "@/types/note";

const PAGE_SIZE = 30;

export function useNotes(source: NoteSource | null, search: string, favoriteOnly: boolean) {
  const [items, setItems] = useState<Note[]>([]);
  const [total, setTotal] = useState(0);
  const [offset, setOffset] = useState(0); // 加载更多累计 offset
  const debouncedSearch = useDebouncedValue(search, 300);

  const fetchFirst = useCallback(async () => {
    const [rows, count] = await Promise.all([
      listNotes({ source, search: debouncedSearch || null, favorite: favoriteOnly, limit: PAGE_SIZE, offset: 0 }),
      countNotes({ source, search: debouncedSearch || null, favorite: favoriteOnly }),
    ]);
    setItems(rows);
    setTotal(count);
    setOffset(PAGE_SIZE);
  }, [source, debouncedSearch, favoriteOnly]);

  useEffect(() => {
    fetchFirst().catch(console.error);
  }, [fetchFirst]);

  // notepad://changed → 刷新（保存/编辑/删除后后端 emit）
  useTauriEvent("notepad://changed", () => {
    fetchFirst().catch(console.error);
  });

  const loadMore = useCallback(async () => {
    const rows = await listNotes({ source, search: debouncedSearch || null, favorite: favoriteOnly, limit: PAGE_SIZE, offset });
    setItems((prev) => [...prev, ...rows]);
    setOffset((o) => o + PAGE_SIZE);
  }, [source, debouncedSearch, favoriteOnly, offset]);

  return { items, total, refresh: fetchFirst, loadMore, hasMore: items.length < total };
}
```

> 注：`useDebouncedValue` 从 `useClipboardHistory.ts` 复导出（该文件已 export，第 39-46 行）。

- [ ] **Step 4: 提交（前端类型检查留到 Task 13 后统一）**

```bash
git add crates/desktop/frontend/src/types/note.ts crates/desktop/frontend/src/lib/notepad.ts crates/desktop/frontend/src/hooks/useNotes.ts
git commit -m "feat(notepad): 前端 types/note.ts + lib/notepad.ts + hooks/useNotes.ts"
```

---

## Task 13: 前端 pages/Notepad（TipTap 编辑器）+ package.json 依赖

**Files:**
- Modify: `crates/desktop/frontend/package.json`（TipTap 依赖）
- Create: `crates/desktop/frontend/src/pages/Notepad/extensions.ts`
- Create: `crates/desktop/frontend/src/pages/Notepad/NoteList.tsx`
- Create: `crates/desktop/frontend/src/pages/Notepad/NoteEditor.tsx`
- Create: `crates/desktop/frontend/src/pages/Notepad/index.tsx`

- [ ] **Step 1: package.json 加 TipTap 依赖**

`crates/desktop/frontend/package.json`，在 `dependencies`（第 12-24 行）内追加：
```json
    "@tiptap/core": "^3.0.0",
    "@tiptap/react": "^3.0.0",
    "@tiptap/pm": "^3.0.0",
    "@tiptap/starter-kit": "^3.0.0",
    "@tiptap/extension-link": "^3.0.0",
    "@tiptap/extension-image": "^3.0.0",
    "tiptap-markdown": "^0.8.0",
```

Run: `cd crates/desktop/frontend && npm install`（或 pnpm/yarn，按项目实际）
Expected: 安装成功。若 `tiptap-markdown` peer dep 报 React 19 / TipTap v3 不兼容，改用 `--legacy-peer-deps` 或锁定 `tiptap-markdown@next`；记下实际可用版本。

- [ ] **Step 2: extensions.ts（TipTap 扩展 + Image NodeView）**

`crates/desktop/frontend/src/pages/Notepad/extensions.ts`：
```ts
import { useEditor, ReactNodeViewRenderer, NodeViewWrapper, type Editor } from "@tiptap/react";
import StarterKit from "@tiptap/starter-kit";
import Link from "@tiptap/extension-link";
import Image from "@tiptap/extension-image";
import { Markdown } from "tiptap-markdown";
import { useEffect, useRef, useState, type FC } from "react";
import { getNoteImage } from "@/lib/notepad";

// 抑制未用 import 警告（Editor 类型由 useEditor 返回值隐式使用）
type _Editor = Editor;

/**
 * 自定义 Image NodeView：src 形如 `note-img:<hash>` 时，解析 hash → invoke get_note_image
 * 取 WebP data URL → 直接作 <img src>（data URL 无需 blob/revoke）。
 * getHTML 仍输出稳定的 `note-img:<hash>` 协议（不存临时 blob URL），笔记可持久化、跨会话还原。
 */
const NoteImageView: FC<{ node: { attrs: { src: string; alt?: string | null } } }> = ({ node }) => {
  const { src, alt } = node.attrs;
  const [url, setUrl] = useState<string | null>(null);

  useEffect(() => {
    let cancelled = false;
    const prefix = "note-img:";
    if (!src || !src.startsWith(prefix)) {
      setUrl(src || null); // 外部 URL 原样用
      return;
    }
    getNoteImage(src.slice(prefix.length))
      .then((dataUrl) => { if (!cancelled) setUrl(dataUrl); })
      .catch(() => { if (!cancelled) setUrl(null); });
    return () => { cancelled = true; };
  }, [src]);

  if (!url) {
    return (
      <span className="inline-block w-16 h-10 bg-muted rounded text-[10px] text-muted-foreground flex items-center justify-center">
        {alt || "[图片]"}
      </span>
    );
  }
  return <img src={url} alt={alt || ""} className="max-w-full rounded my-1" />;
};

/** 创建编辑器实例。onUpdate 由调用方传入（debounce 后 update_note）。 */
export function useNoteEditor(content: string, onUpdate: (html: string) => void) {
  const editor = useEditor({
    extensions: [
      StarterKit,
      Link.configure({ openOnClick: false }),
      Image.extend({
        addNodeView() {
          return ReactNodeViewRenderer(NoteImageView);
        },
      }),
      Markdown, // md 序列化（editor.storage.markdown.getMarkdown()）
    ],
    content,
    onUpdate: ({ editor }) => onUpdate(editor.getHTML()),
  });

  // 切换 note 时重设 content
  useEffect(() => {
    if (editor && !editor.isDestroyed) editor.commands.setContent(content || "", false);
  }, [content, editor]);

  return editor;
}
```

> TipTap v3 的 `Image.extend({ addNodeView })` + `ReactNodeViewRenderer(NodeView)` 语法：NodeView 组件收 `{ node }` props，`node.attrs.src`/`alt` 即图片属性。若 v3 改了 API（如 NodeView props 结构），按编译器提示调整。核心契约不变：节点 attrs.src = `note-img:<hash>`，渲染时解析取图，getHTML 仍输出协议 src。

- [ ] **Step 3: NoteList.tsx**

`crates/desktop/frontend/src/pages/Notepad/NoteList.tsx`：
```tsx
import { useState } from "react";
import { cn } from "@/lib/utils";
import { Search, Pin, Star, Plus, Mic, ScanText, Clipboard as ClipIcon } from "lucide-react";
import type { Note, NoteSource } from "@/types/note";
import { useNotes } from "@/hooks/useNotes";
import { createNote, toggleNotePinned, toggleNoteFavorite, saveTranscriptionToNote } from "@/lib/notepad";

const SOURCE_TABS: { key: NoteSource | null; label: string }[] = [
  { key: null, label: "全部" },
  { key: "asr", label: "语音" },
  { key: "ocr", label: "OCR" },
  { key: "clipboard", label: "剪贴板" },
];

export default function NoteList({
  selectedId,
  onSelect,
}: {
  selectedId: number | null;
  onSelect: (id: number) => void;
}) {
  const [tab, setTab] = useState<NoteSource | null>(null);
  const [search, setSearch] = useState("");
  const [favOnly, setFavOnly] = useState(false);
  const { items, total, loadMore, hasMore, refresh } = useNotes(tab, search, favOnly);

  const handleNew = async () => {
    const id = await createNote("manual", null, "");
    onSelect(id);
  };

  const handlePin = async (e: React.MouseEvent, id: number) => {
    e.stopPropagation();
    await toggleNotePinned(id);
  };
  const handleFav = async (e: React.MouseEvent, id: number) => {
    e.stopPropagation();
    await toggleNoteFavorite(id);
  };

  return (
    <div className="flex flex-col h-full border-r border-border bg-card">
      {/* 搜索 + 新建 */}
      <div className="p-2 flex items-center gap-1.5 border-b border-border">
        <div className="relative flex-1">
          <Search className="absolute left-2 top-1/2 -translate-y-1/2 w-3.5 h-3.5 text-muted-foreground" />
          <input
            className="w-full pl-7 pr-2 py-1 text-sm rounded bg-background border border-border focus:outline-none focus:ring-1 focus:ring-ring"
            placeholder="搜索笔记"
            value={search}
            onChange={(e) => setSearch(e.target.value)}
          />
        </div>
        <button
          className="p-1 rounded hover:bg-accent text-foreground"
          onClick={handleNew}
          title="新建笔记"
        >
          <Plus className="w-4 h-4" />
        </button>
      </div>
      {/* 来源 tab + 收藏 */}
      <div className="px-2 py-1.5 flex items-center gap-1 border-b border-border overflow-x-auto">
        {SOURCE_TABS.map((t) => (
          <button
            key={t.label}
            className={cn(
              "px-2 py-0.5 text-xs rounded whitespace-nowrap",
              tab === t.key ? "bg-primary text-primary-foreground" : "hover:bg-accent text-muted-foreground"
            )}
            onClick={() => setTab(t.key)}
          >
            {t.label}
          </button>
        ))}
        <button
          className={cn(
            "ml-auto p-1 rounded",
            favOnly ? "text-amber-400" : "text-muted-foreground hover:bg-accent"
          )}
          onClick={() => setFavOnly((v) => !v)}
          title="仅看收藏"
        >
          <Star className={cn("w-3.5 h-3.5", favOnly && "fill-amber-400")} />
        </button>
      </div>
      {/* 列表 */}
      <div className="flex-1 overflow-y-auto">
        {items.map((n) => (
          <NoteRow key={n.id} note={n} active={n.id === selectedId} onSelect={onSelect} onPin={handlePin} onFav={handleFav} />
        ))}
        {items.length === 0 && (
          <div className="p-4 text-center text-xs text-muted-foreground">暂无笔记</div>
        )}
        {hasMore && (
          <button className="w-full py-2 text-xs text-muted-foreground hover:bg-accent" onClick={loadMore}>
            加载更多（共 {total} 条）
          </button>
        )}
      </div>
    </div>
  );
}

function NoteRow({
  note,
  active,
  onSelect,
  onPin,
  onFav,
}: {
  note: Note;
  active: boolean;
  onSelect: (id: number) => void;
  onPin: (e: React.MouseEvent, id: number) => void;
  onFav: (e: React.MouseEvent, id: number) => void;
}) {
  const preview = note.title || note.content_text.slice(0, 60) || "（空笔记）";
  const SourceIcon = note.source === "asr" ? Mic : note.source === "ocr" ? ScanText : note.source === "clipboard" ? ClipIcon : null;
  return (
    <div
      className={cn(
        "group px-3 py-2 cursor-pointer border-b border-border/50",
        active ? "bg-accent" : "hover:bg-accent/50"
      )}
      onClick={() => onSelect(note.id)}
    >
      <div className="flex items-center gap-1.5">
        {SourceIcon && <SourceIcon className="w-3 h-3 flex-shrink-0 text-muted-foreground" />}
        <span className="flex-1 truncate text-sm font-medium">{preview}</span>
        <button className="p-0.5 opacity-0 group-hover:opacity-60 hover:!opacity-100" onClick={(e) => onPin(e, note.id)} title={note.is_pinned ? "取消置顶" : "置顶"}>
          <Pin className={cn("w-3 h-3", note.is_pinned ? "fill-foreground text-foreground" : "text-muted-foreground")} />
        </button>
        <button className="p-0.5 opacity-0 group-hover:opacity-60 hover:!opacity-100" onClick={(e) => onFav(e, note.id)} title="收藏">
          <Star className={cn("w-3 h-3", note.is_favorite ? "fill-amber-400 text-amber-400" : "text-muted-foreground")} />
        </button>
      </div>
      <div className="mt-0.5 text-[10px] text-muted-foreground">{note.updated_at}</div>
    </div>
  );
}
```

- [ ] **Step 4: NoteEditor.tsx**

`crates/desktop/frontend/src/pages/Notepad/NoteEditor.tsx`：
```tsx
import { useState, useEffect, useCallback, useRef } from "react";
import { cn } from "@/lib/utils";
import { Bold, Italic, Heading1, Heading2, List, ListOrdered, Quote, Code, Minus, Link as LinkIcon, Image as ImageIcon, Undo, Redo, Download, Upload, Star, Pin } from "lucide-react";
import { invoke } from "@tauri-apps/api/core";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import type { Note } from "@/types/note";
import { getNote, updateNote, toggleNotePinned, toggleNoteFavorite, exportNote, importNoteFromFile, insertNoteImage } from "@/lib/notepad";
import { useNoteEditor } from "./extensions";

export default function NoteEditor({ noteId }: { noteId: number | null }) {
  const [note, setNote] = useState<Note | null>(null);
  const [title, setTitle] = useState("");
  const [toast, setToast] = useState<string | null>(null);
  const saveTimer = useRef<ReturnType<typeof setTimeout> | null>(null);
  const currentId = useRef<number | null>(null);

  // 加载笔记
  useEffect(() => {
    if (noteId == null) { setNote(null); return; }
    currentId.current = noteId;
    getNote(noteId).then((n) => {
      if (currentId.current !== noteId) return; // 切换防竞态
      setNote(n ?? null);
      setTitle(n?.title ?? "");
    });
  }, [noteId]);

  const doSave = useCallback((html: string) => {
    const id = currentId.current;
    if (id == null) return;
    updateNote(id, title, html).catch(console.error);
  }, [title]);

  const editor = useNoteEditor(note?.content_html ?? "", (html) => {
    // debounce 800ms 自动保存
    if (saveTimer.current) clearTimeout(saveTimer.current);
    saveTimer.current = setTimeout(() => doSave(html), 800);
  });

  // 标题变更也 debounce 保存
  useEffect(() => {
    if (noteId == null) return;
    const t = setTimeout(() => {
      if (editor && !editor.isDestroyed) updateNote(noteId, title, editor.getHTML()).catch(console.error);
    }, 800);
    return () => clearTimeout(t);
  }, [title, noteId, editor]);

  const flash = (msg: string) => { setToast(msg); setTimeout(() => setToast(null), 2000); };

  if (!note) {
    return <div className="flex-1 flex items-center justify-center text-muted-foreground text-sm">选择或新建一条笔记</div>;
  }

  const cmd = editor?.bind?.();
  const exec = (fn: () => void) => () => { if (editor && !editor.isDestroyed) fn(); };

  const insertImage = async () => {
    const selected = await openDialog({ filters: [{ name: "图片", extensions: ["png", "jpg", "jpeg", "webp"] }] });
    if (!selected || Array.isArray(selected)) return;
    try {
      const hash = await insertNoteImage(selected as string);
      editor?.chain().focus().setImage({ src: `note-img:${hash}`, alt: "图片" }).run();
    } catch (e) { flash("插入图片失败: " + String(e)); }
  };

  const doExport = async (ext: "md" | "txt" | "html") => {
    if (!editor) return;
    let content: string;
    if (ext === "md") content = (editor.storage as any).markdown?.getMarkdown() ?? editor.getText();
    else if (ext === "txt") content = editor.getText();
    else {
      // html：把 note-img:<hash> 替换为 data URL（自包含）
      content = editor.getHTML();
      const re = /note-img:([a-f0-9]+)/g;
      const hashes = [...new Set([...content.matchAll(re)].map((m) => m[1]))];
      for (const h of hashes) {
        try {
          const dataUrl = await (await import("@/lib/notepad")).getNoteImage(h);
          content = content.split(`note-img:${h}`).join(dataUrl);
        } catch { /* 替换失败保留占位 */ }
      }
    }
    const stem = (title || note.content_text.slice(0, 20) || "note").replace(/\s+/g, "_");
    const path = await exportNote(stem, ext, content);
    flash("已导出: " + path);
  };

  const doImport = async () => {
    const selected = await openDialog({ filters: [{ name: "Markdown", extensions: ["md", "txt"] }] });
    if (!selected || Array.isArray(selected)) return;
    const md = await importNoteFromFile(selected as string);
    // tiptap-markdown 解析 md → setContent
    editor?.commands.setContent(md, false);
    flash("已导入");
  };

  const tools = [
    { icon: Bold, title: "粗体", onClick: exec(() => editor?.chain().focus().toggleBold().run()) },
    { icon: Italic, title: "斜体", onClick: exec(() => editor?.chain().focus().toggleItalic().run()) },
    { icon: Heading1, title: "标题1", onClick: exec(() => editor?.chain().focus().toggleHeading({ level: 1 }).run()) },
    { icon: Heading2, title: "标题2", onClick: exec(() => editor?.chain().focus().toggleHeading({ level: 2 }).run()) },
    { icon: List, title: "无序列表", onClick: exec(() => editor?.chain().focus().toggleBulletList().run()) },
    { icon: ListOrdered, title: "有序列表", onClick: exec(() => editor?.chain().focus().toggleOrderedList().run()) },
    { icon: Quote, title: "引用", onClick: exec(() => editor?.chain().focus().toggleBlockquote().run()) },
    { icon: Code, title: "代码块", onClick: exec(() => editor?.chain().focus().toggleCodeBlock().run()) },
    { icon: Minus, title: "分割线", onClick: exec(() => editor?.chain().focus().setHorizontalRule().run()) },
    { icon: LinkIcon, title: "链接", onClick: exec(() => { const url = prompt("链接 URL"); if (url) editor?.chain().focus().setLink({ href: url }).run(); }) },
    { icon: ImageIcon, title: "图片", onClick: insertImage },
    { icon: Undo, title: "撤销", onClick: exec(() => editor?.chain().focus().undo().run()) },
    { icon: Redo, title: "重做", onClick: exec(() => editor?.chain().focus().redo().run()) },
  ];

  return (
    <div className="flex-1 flex flex-col bg-background">
      {/* 工具栏 */}
      <div className="flex items-center gap-0.5 px-2 py-1 border-b border-border flex-wrap">
        {tools.map(({ icon: Icon, title, onClick }, i) => (
          <button key={i} className="p-1 rounded hover:bg-accent text-muted-foreground hover:text-foreground" title={title} onClick={onClick}>
            <Icon className="w-4 h-4" />
          </button>
        ))}
        <div className="ml-auto flex items-center gap-0.5">
          <button className="p-1 rounded hover:bg-accent text-muted-foreground" title="导入 md" onClick={doImport}><Upload className="w-4 h-4" /></button>
          <button className="p-1 rounded hover:bg-accent text-muted-foreground" title="导出 md" onClick={() => doExport("md")}><Download className="w-4 h-4" /></button>
          <button className={cn("p-1 rounded", note.is_favorite ? "text-amber-400" : "text-muted-foreground hover:bg-accent")} title="收藏"
            onClick={async () => { await toggleNoteFavorite(note.id); const n = await getNote(note.id); if (n) setNote(n); }}><Star className={cn("w-4 h-4", note.is_favorite && "fill-amber-400")} /></button>
          <button className={cn("p-1 rounded", note.is_pinned ? "text-foreground" : "text-muted-foreground hover:bg-accent")} title="置顶"
            onClick={async () => { await toggleNotePinned(note.id); const n = await getNote(note.id); if (n) setNote(n); }}><Pin className={cn("w-4 h-4", note.is_pinned && "fill-foreground")} /></button>
        </div>
      </div>
      {/* 标题 */}
      <input
        className="px-4 pt-3 pb-1 text-lg font-semibold bg-transparent focus:outline-none"
        placeholder="无标题"
        value={title}
        onChange={(e) => setTitle(e.target.value)}
      />
      {/* 编辑器 */}
      <div className="flex-1 overflow-y-auto px-4 pb-4">
        <div className="prose prose-sm max-w-none [&_img]:max-w-full">
          <EditorContentBridge editor={editor} />
        </div>
      </div>
      {toast && <div className="absolute bottom-3 right-3 px-3 py-1.5 rounded bg-foreground text-background text-xs">{toast}</div>}
    </div>
  );
}

/** 桥接：useEditor 返回的 editor 通过 EditorContent 渲染。 */
import { EditorContent } from "@tiptap/react";
function EditorContentBridge({ editor }: { editor: ReturnType<typeof useNoteEditor> }) {
  if (!editor) return null;
  return <EditorContent editor={editor} />;
}
```

> 注：`@tauri-apps/plugin-dialog` 的 `open` 需 `tauri-plugin-dialog`（desktop Cargo.toml 已有第 18 行）。前端需 `npm install @tauri-apps/plugin-dialog`。若未装，Step 1 的 npm install 一起装上（在 package.json dependencies 加 `"@tauri-apps/plugin-dialog": "^2"`）。

- [ ] **Step 5: index.tsx（三栏布局）**

`crates/desktop/frontend/src/pages/Notepad/index.tsx`：
```tsx
import { useState } from "react";
import NoteList from "./NoteList";
import NoteEditor from "./NoteEditor";

export default function Notepad() {
  const [selectedId, setSelectedId] = useState<number | null>(null);
  return (
    <div className="flex h-screen w-screen overflow-hidden bg-background text-foreground">
      <div className="w-64 flex-shrink-0">
        <NoteList selectedId={selectedId} onSelect={setSelectedId} />
      </div>
      <NoteEditor noteId={selectedId} />
    </div>
  );
}
```

- [ ] **Step 6: 前端类型检查 + 构建**

Run（在工作树 frontend 目录）: `cd crates/desktop/frontend && npm run build`
Expected: tsc + vite build 通过。TipTap/NodeView/dialog 报错按编译器提示修正（见各步备注）。

- [ ] **Step 7: 提交**

```bash
git add crates/desktop/frontend/package.json crates/desktop/frontend/src/pages/Notepad/
git commit -m "feat(notepad): 前端 Notepad 页面 — TipTap 编辑器 + 列表 + 自动保存 + 导入导出"
```

---

## Task 14: Result 窗口「存入记事本」集成

**Files:**
- Modify: `crates/desktop/frontend/src/components/SvgIcon.tsx`（加 note 图标）
- Modify: `crates/desktop/frontend/src/pages/Result/index.tsx`（工具栏按钮）

- [ ] **Step 1: SvgIcon 加 note 图标**

`crates/desktop/frontend/src/components/SvgIcon.tsx`，在 ICONS map（第 3-14 行）加一项。需先在工作树放图标文件 `/public/icons/note.svg`（一个笔记本轮廓 SVG）。若无现成图标，用内联 SVG：改 SvgIcon 支持内联，或在 ICONS 加路径指向新建的 `public/icons/note.svg`。

创建 `crates/desktop/frontend/public/icons/note.svg`：
```xml
<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M14 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V8z"/><polyline points="14 2 14 8 20 8"/><line x1="9" y1="13" x2="15" y2="13"/><line x1="9" y1="17" x2="13" y2="17"/></svg>
```

`SvgIcon.tsx` ICONS 加：
```ts
  note: "/icons/note.svg",
```

- [ ] **Step 2: Result/index.tsx 工具栏加按钮**

`crates/desktop/frontend/src/pages/Result/index.tsx`，在 tools 数组（第 366-381 行）的 `edit` 分支前插入 note 按钮。在 `import` 区顶部（第 2 行 `import { invoke } from "@tauri-apps/api/core";` 已有）确保能用。在 tools 数组中，紧挨 `{ id: "polish-now", ... }` 之后、`...(editing ? ...)` 之前加：

```tsx
    { id: "note", icon: "note", label: "存入记事本", disabled: !text.trim(), onClick: saveToNote },
```

并在组件内（约第 100 行 `renderResultNow` 附近）加 handler：
```tsx
  const saveToNote = async () => {
    try {
      const tid = await invoke<number | null>("current_transcription_id");
      if (tid == null) return;
      await invoke<number>("save_transcription_to_note", { transcriptionId: tid });
      // toast：复用现有 toast 机制（Result 窗口已有 toast state，第 52 行）
      setToast("已存入记事本");
      setTimeout(() => setToast(null), 1500);
    } catch (e) {
      console.error(e);
    }
  };
```

> `setToast` 在 Result 组件第 52 行已存在。若 toast 渲染位置需确认（grep `toast`），无则用 `console.log` 占位 + 提示。

- [ ] **Step 3: 前端构建验证**

Run: `cd crates/desktop/frontend && npm run build`
Expected: 通过

- [ ] **Step 4: 提交**

```bash
git add crates/desktop/frontend/public/icons/note.svg crates/desktop/frontend/src/components/SvgIcon.tsx crates/desktop/frontend/src/pages/Result/index.tsx
git commit -m "feat(notepad): Result 窗口工具栏「存入记事本」按钮 + note 图标"
```

---

## Task 15: Clipboard 条目「存入记事本」集成

**Files:**
- Modify: `crates/desktop/frontend/src/pages/Clipboard/ClipboardItem.tsx`（操作区加按钮）

- [ ] **Step 1: ClipboardItem.tsx 加按钮**

`crates/desktop/frontend/src/pages/Clipboard/ClipboardItem.tsx`，在 import 行（第 3 行 lucide-react）加入 `NotebookPen`：
```tsx
import { Star, Mic, Type, Image as ImageIcon, FileText, Trash2, Download, FolderOpen, Copy, ScanText, Loader2, Check, NotebookPen } from "lucide-react";
```

在组件内加 handler（约第 117 行 `handleCopy` 之后）：
```tsx
  const [noteSaving, setNoteSaving] = useState(false);
  const handleSaveToNote = async (e: React.MouseEvent) => {
    e.stopPropagation();
    if (noteSaving) return;
    setNoteSaving(true);
    try {
      await invoke("save_clipboard_to_note", { itemId: item.id });
      setNoteSaving(false);
    } catch (err) {
      setNoteSaving(false);
      console.error(err);
    }
  };
```

在操作区（第 176 行 `<div className="flex-shrink-0 flex items-center gap-0.5">` 内），在 `Copy` 按钮之后、`Star` 按钮之前插入：
```tsx
        <button
          className="p-0.5 opacity-0 group-hover:opacity-60 hover:!opacity-100 transition-opacity"
          onClick={handleSaveToNote}
          title="存入记事本"
        >
          <NotebookPen className="w-3.5 h-3.5 text-muted-foreground hover:text-foreground" />
        </button>
```

- [ ] **Step 2: 前端构建验证**

Run: `cd crates/desktop/frontend && npm run build`
Expected: 通过

- [ ] **Step 3: 提交**

```bash
git add crates/desktop/frontend/src/pages/Clipboard/ClipboardItem.tsx
git commit -m "feat(notepad): 剪贴板条目「存入记事本」按钮"
```

---

## Task 16: Settings 面板 + OCR 二次集成（可选入口）

**Files:**
- Modify: `crates/desktop/frontend/src/pages/Settings/HistoryPanel.tsx`
- Modify: `crates/desktop/frontend/src/pages/Settings/ClipboardPanel.tsx`

> 这是 spec §5.3 的另两处入口。与 Result/Clipboard 同一逻辑，仅位置不同。每处加一个 `NotebookPen` 按钮调对应命令。OCR 的 `save_ocr_to_note` 命令已在 Task 8 注册；MVP 不在 OCR 流程强制弹按钮（OCR 当前自动开 TextEdit），命令已可用，后续按需在 OCR 结果 UI 加按钮。

- [ ] **Step 1: HistoryPanel 行操作加按钮**

`crates/desktop/frontend/src/pages/Settings/HistoryPanel.tsx`，定位每行的操作按钮区（grep `lucide-react` 或 `按钮`），在删除/复制按钮旁加：
```tsx
import { NotebookPen } from "lucide-react";
// ...
<button
  className="p-1 rounded hover:bg-accent text-muted-foreground"
  title="存入记事本"
  onClick={async (e) => {
    e.stopPropagation();
    try { await invoke("save_transcription_to_note", { transcriptionId: row.id }); } catch (err) { console.error(err); }
  }}
>
  <NotebookPen className="w-3.5 h-3.5" />
</button>
```

> `row.id` 为该历史记录的 transcription_id（HistoryPanel 列表项字段，参考 `list_transcriptions` 返回的 `TranscriptionRecord.id`）。确认字段名（grep `\.id`）。

- [ ] **Step 2: ClipboardPanel 行操作加按钮**

`crates/desktop/frontend/src/pages/Settings/ClipboardPanel.tsx`，同样加按钮调 `save_clipboard_to_note`：
```tsx
<button
  className="p-1 rounded hover:bg-accent text-muted-foreground"
  title="存入记事本"
  onClick={async (e) => {
    e.stopPropagation();
    try { await invoke("save_clipboard_to_note", { itemId: item.id }); } catch (err) { console.error(err); }
  }}
>
  <NotebookPen className="w-3.5 h-3.5" />
</button>
```

> 确认 ClipboardPanel 列表项字段名（`item.id`，与 ClipboardItem 一致）。

- [ ] **Step 3: 前端构建验证**

Run: `cd crates/desktop/frontend && npm run build`
Expected: 通过

- [ ] **Step 4: 提交**

```bash
git add crates/desktop/frontend/src/pages/Settings/HistoryPanel.tsx crates/desktop/frontend/src/pages/Settings/ClipboardPanel.tsx
git commit -m "feat(notepad): Settings 历史记录/剪贴板管理页「存入记事本」入口"
```

---

## Task 17: 全量编译 + e2e 验证 + 文档同步

**Files:**
- 验证: 全工作树
- Modify: `docs/architecture.md`（加 notepad crate 说明）

- [ ] **Step 1: 全量后端编译 + 测试**

Run:
```bash
cargo build --manifest-path Cargo.toml
cargo test --manifest-path crates/infra/Cargo.toml
cargo test --manifest-path crates/notepad/Cargo.toml
```
Expected: 全部编译通过，测试全绿。

- [ ] **Step 2: 前端构建**

Run: `cd crates/desktop/frontend && npm run build`
Expected: tsc + vite 通过。

- [ ] **Step 3: 桌面端运行 + 手动 e2e**

Run（用户侧，需真实环境）: `cargo run --manifest-path crates/desktop/Cargo.toml`（或项目既有 run 命令），逐项验证：
1. 托盘「记事本」→ 打开记事本窗口（左侧列表空 + 右侧编辑器占位）
2. 新建笔记 → 输入标题/正文 → 停顿 800ms 自动保存 → 关窗重开内容在
3. 录音识别 → 结果窗工具栏「存入记事本」→ 记事本列表出现该条 + 来源徽标=语音
4. 复制文本 → 剪贴板浮窗条目「存入记事本」→ 笔记正文为该文本
5. 复制图片 → 剪贴板图片条目「存入记事本」→ 笔记含 `<img>` → 编辑器渲染图片
6. 搜索笔记（≥3 字符走 FTS，<3 走 LIKE）→ 命中正确
7. 导出 md/txt/html → `~/Documents/octopus/notes/` 下生成文件，html 内嵌图片可外部打开
8. 溯源徽标点击 → 跳到 Settings 对应历史/剪贴板页（MVP 可仅打开 Settings，定位后续完善）

- [ ] **Step 4: 文档同步**

更新 `docs/architecture.md`，在 crate 列表/模块说明加：
```markdown
- `octopus-notepad` — 内容收集箱式记事本业务逻辑（notes CRUD + FTS + HTML→text 序列化 + 文件 I/O），仅依赖 infra。
```

并在桌面端窗口/功能章节加「记事本（内容收集箱）」条目，关联 spec `docs/superpowers/specs/2026-06-30-notepad-design.md`。

- [ ] **Step 5: 最终提交**

```bash
git add docs/architecture.md
git commit -m "docs(notepad): architecture.md 同步 notepad crate 与记事本窗口说明"
```

- [ ] **Step 6: 收尾（分支整合由 finishing-a-development-branch 决定）**

功能完成后，运行 `superpowers:finishing-a-development-branch` 决定合并/PR/清理。

---

## 备注：规格偏差与决策

1. **id 策略**：规格 §2.1 `AUTOINCREMENT` 保留（与 models/prompts 一致），不用 clipboard/transcriptions 的毫秒戳 id。notes 有独立 created_at/updated_at 列，id 无需兼任时间戳。新建用 `last_insert_rowid()`。
2. **Result 窗口溯源**：规格 §5.3 说 Result 工具栏调 `save_transcription_to_note(transcription_id)`，但 Result 前端原本只收文本无 id。新增 `current_transcription_id` 命令（coordinator AtomicI64 记录会话起点 id）解决，无需改文本事件 payload。
3. **图片桥接**：规格 §6.2 明确 notepad 不依赖 clipboard，图片 BLOB 由 desktop command 层桥接（`get_note_image`/`insert_note_image`）。insert 时 hash = sha256(PNG)，与 clipboard image_data 去重键一致，同图共享一份 BLOB。
4. **OCR 集成**：`save_ocr_to_note` 命令已实现注册；MVP 不在 OCR 自动流程强制加按钮（OCR 现自动开 TextEdit），命令可用，后续按需接 UI。
5. **TipTap 版本**：锁定 v3（兼容 React 19）。`tiptap-markdown` 与 v3 的兼容性在 Task 13 Step 1 安装时验证，必要时用 `@next` 或 legacy-peer-deps。
6. **溯源回溯定位（§4.2）**：MVP 阶段笔记列表/编辑器的来源徽标点击仅打开 Settings 对应页（`open_settings(initial_page)`）；精确滚动高亮到 `source_ref_id` 行留作后续完善。`source_ref_id` 失效（原记录已删）时徽标灰显——因 `get_note` 返回 ref_id 但应用层未校验其有效性，MVP 统一提供「打开来源」按钮（点击若 Settings 找不到对应行，用户可见无定位，体验可接受），失效校验留作后续。
7. **全局快捷键（§5.2）**：默认不绑（octopus 快捷键已拥挤），spec 明确 MVP 不做。


---

## 来自原文件 `2026-07-01-image-preview.md`

# 图片预览（剪贴板图片项）实施计划

> **For agentic workers:** REQUIRED SUB-SKILL: 用 superpowers:subagent-driven-development（推荐）或 superpowers:executing-plans 逐任务实施。步骤用 checkbox（`- [ ]`）跟踪。

**Goal:** 为剪贴板图片项新增一个轻工具栏预览窗口（含画圈/直线/矩形/文字标注 + 撤销 + 保存/复制/OCR/置顶），并为未来「贴图钉屏」模式打好共享基础。

**Architecture:** 镜像 `compact_editor_window` 的动态窗口 + PENDING 暂存模式（open 写 PENDING → 建窗/聚焦；前端 mount 调 `get_pending_image` 取走）。标注核心从 `Screenshot/index.tsx` 抽取到共享 `frontend/src/lib/annotation.ts`（纯函数，DRY）。预览用**自然像素坐标空间**（窗口可缩放，标注存图像本征分辨率，resize 不错位）；合成保存/复制时在自然尺寸画布 1:1 重绘。

**Tech Stack:** Rust + Tauri 2（`#[tauri::command]`、`generate_handler!`、ACL capabilities）、React 19 + TypeScript + Vite 8 + Tailwind 4 + lucide-react。前端无 vitest（项目惯例：后端 `#[cfg(test)]` + `npm run build` 类型检查 + 手动 e2e）。

---

## 关键约束（贯穿所有任务）

- **不往 main 同步**：功能完整完成前，所有提交留在 `worktree-feature-notepad` 分支。
- **worktree cwd 陷阱**：Bash cwd 是主仓库；cargo 用 `--manifest-path <WT>/Cargo.toml`，npm 用 `--prefix <WT>/crates/desktop/frontend`，git 用 `git -C <WT>`，读写用绝对路径。
- **dist 已纳入 git**：前端变更必须 `npm run build` 并提交 `crates/desktop/dist`。
- **绝对路径根**（下文 `<WT>` = `/Users/wudarui/workspace/agent/octopus/.claude/worktrees/feature-notepad`）。

## 文件结构

| 文件 | 责任 | 动作 |
|------|------|------|
| `crates/desktop/frontend/src/lib/annotation.ts` | 共享标注纯函数（Tool/Annotation 类型 + draw/hit/bounds） | **新建** |
| `crates/desktop/frontend/src/pages/Screenshot/index.tsx` | 改为从 annotation.ts import（去重） | 改 |
| `crates/desktop/src/image_preview_window.rs` | 预览窗口创建 + macOS 激活策略 | **新建** |
| `crates/desktop/src/image_preview_commands.rs` | PENDING 暂存 + open/get/close 三命令 | **新建** |
| `crates/desktop/src/clipboard_commands.rs` | 加 get_image_full / save_image_dialog / copy_image_to_clipboard | 改 |
| `crates/desktop/src/main.rs` | mod 声明 + generate_handler! 注册 6 命令 + RunEvent 路由 | 改 |
| `crates/desktop/capabilities/default.json` | windows 数组加 `image_preview_window` | 改 |
| `crates/desktop/frontend/src/pages/ImagePreview/index.tsx` | 预览主组件（画布 + 标注交互） | **新建** |
| `crates/desktop/frontend/src/pages/ImagePreview/Toolbar.tsx` | 工具栏（工具按钮 + 颜色粗细浮窗 + 动作按钮） | **新建** |
| `crates/desktop/frontend/src/App.tsx` | 路由 case `image_preview_window` | 改 |
| `crates/desktop/frontend/src/pages/Clipboard/ClipboardItem.tsx` | 图片项加「预览」入口 | 改 |
| `docs/architecture.md` | 同步新模块说明 | 改 |

---

## Task 1: 抽取共享标注核心到 lib/annotation.ts

**Files:**
- Create: `<WT>/crates/desktop/frontend/src/lib/annotation.ts`
- Modify: `<WT>/crates/desktop/frontend/src/pages/Screenshot/index.tsx`（删除 6 个内联函数 + Tool/Annotation 定义，改为 import）

**背景**：Screenshot 现把 `Tool`/`Annotation`/`drawAnnotation`/`drawAnnotationScaled`/`drawMultilineText`/`annBounds`/`hitTestAnnotationPrecise`/`pointToSegmentDist` 全部内联在组件里（`index.tsx` 约 11-23 行类型、212-494 行函数）。它们除 `hitTestAnnotationPrecise` 闭包了 `annotations` 外都是纯函数。抽到共享模块后，Screenshot 与 ImagePreview 共用，避免双份维护。

- [ ] **Step 1：新建 `lib/annotation.ts`，写入类型与纯函数**

完整内容（drawAnnotation / drawAnnotationScaled 从 Screenshot `index.tsx:212-390` **逐字搬迁**，不改逻辑；`drawMultilineText` 从 `index.tsx:286-313` 搬迁作为模块私有函数；`annBounds`/`pointToSegmentDist` 从 `:416-494` 搬迁；`hitTestAnnotationPrecise` 从 `:443-483` 搬迁但**加 `anns: Annotation[]` 参数**取代闭包）：

```ts
// 共享标注类型 + 纯绘制/命中函数。Screenshot 与 ImagePreview 共用，坐标空间由调用方决定。

export type Tool = "none" | "rect" | "oval" | "line" | "arrow" | "pen" | "text" | "number";

export interface Annotation {
  type: "rect" | "oval" | "line" | "arrow" | "pen" | "text" | "number";
  x1: number; y1: number; x2: number; y2: number;
  text?: string;
  points?: number[][];
  color?: string;
  lineWidth?: number;
  fontSize?: number;
  number?: number;
  circleSize?: number;
}

// —— 以下 drawAnnotation / drawAnnotationScaled / drawMultilineText / annBounds /
//    pointToSegmentDist / hitTestAnnotationPrecise 逐字来自 Screenshot/index.tsx ——
//    （实现见该文件；搬迁时保持完全一致，仅 hitTestAnnotationPrecise 改签名）
```

`hitTestAnnotationPrecise` 新签名（其余函数体逐字搬迁）：

```ts
const HIT_DIST = 8;

export function hitTestAnnotationPrecise(
  mx: number,
  my: number,
  anns: Annotation[],
): number | null {
  for (let i = anns.length - 1; i >= 0; i--) {
    const ann = anns[i];
    if (ann.type === "rect") {
      const x = Math.min(ann.x1, ann.x2);
      const y = Math.min(ann.y1, ann.y2);
      const w = Math.abs(ann.x2 - ann.x1);
      const h = Math.abs(ann.y2 - ann.y1);
      const onEdge = (Math.abs(mx - x) <= HIT_DIST || Math.abs(mx - (x + w)) <= HIT_DIST) && my >= y - HIT_DIST && my <= y + h + HIT_DIST
        || (Math.abs(my - y) <= HIT_DIST || Math.abs(my - (y + h)) <= HIT_DIST) && mx >= x - HIT_DIST && mx <= x + w + HIT_DIST;
      if (onEdge) return i;
    } else if (ann.type === "oval") {
      const cx = (ann.x1 + ann.x2) / 2;
      const cy = (ann.y1 + ann.y2) / 2;
      const rx = Math.abs(ann.x2 - ann.x1) / 2;
      const ry = Math.abs(ann.y2 - ann.y1) / 2;
      if (rx < 1 || ry < 1) continue;
      const dx = (mx - cx) / rx;
      const dy = (my - cy) / ry;
      const dist = Math.abs(Math.sqrt(dx * dx + dy * dy) - 1) * Math.min(rx, ry);
      if (dist <= HIT_DIST) return i;
    } else if (ann.type === "line" || ann.type === "arrow") {
      if (pointToSegmentDist(mx, my, ann.x1, ann.y1, ann.x2, ann.y2) <= HIT_DIST) return i;
    } else if (ann.type === "pen" && ann.points) {
      for (let j = 1; j < ann.points.length; j++) {
        const [px1, py1] = ann.points[j - 1];
        const [px2, py2] = ann.points[j];
        if (pointToSegmentDist(mx, my, px1, py1, px2, py2) <= HIT_DIST) return i;
      }
    } else {
      const b = annBounds(ann);
      if (mx >= b.x && mx <= b.x + b.w && my >= b.y && my <= b.y + b.h) return i;
    }
  }
  return null;
}
```

导出清单：`Tool`, `Annotation`, `drawAnnotation`, `drawAnnotationScaled`, `annBounds`, `hitTestAnnotationPrecise`, `pointToSegmentDist`。`drawMultilineText`/`HIT_DIST` 模块私有不导出（仅 drawAnnotation 内部用）。

- [ ] **Step 2：Screenshot 改为 import**

在 `Screenshot/index.tsx` 顶部加：

```ts
import { type Annotation, type Tool, drawAnnotation, drawAnnotationScaled, annBounds, hitTestAnnotationPrecise, pointToSegmentDist } from "@/lib/annotation";
```

删除组件内 `type Tool = ...`、`interface Annotation {...}`（11-23 行）及 6 个内联函数（212-494 行区间内的 `drawAnnotation`/`drawAnnotationScaled`/`drawMultilineText`/`annBounds`/`hitTestAnnotationPrecise`/`pointToSegmentDist`/`HIT_DIST`）。

- [ ] **Step 3：更新 hitTestAnnotationPrecise 调用点**

`hitTestAnnotationPrecise` 现需 `anns` 参数。grep 出所有调用点并补参：

```bash
cd <WT> && grep -rn "hitTestAnnotationPrecise(" crates/desktop/frontend/src/pages/Screenshot/
```

每个 `hitTestAnnotationPrecise(mx, my)` → `hitTestAnnotationPrecise(mx, my, annotations)`。

- [ ] **Step 4：类型检查 + 构建验证（截图回归不破）**

Run: `npm --prefix <WT>/crates/desktop/frontend run build`
Expected: tsc + vite 构建成功，无 unused / type error。

- [ ] **Step 5：提交**

```bash
git -C <WT> add crates/desktop/frontend/src/lib/annotation.ts crates/desktop/frontend/src/pages/Screenshot/index.tsx
git -C <WT> commit -m "refactor(desktop): 抽取标注核心到 lib/annotation.ts 供截图与图片预览共用"
```

---

## Task 2: 后端 — 预览窗口 + PENDING 命令 + 注册 + ACL

**Files:**
- Create: `<WT>/crates/desktop/src/image_preview_window.rs`
- Create: `<WT>/crates/desktop/src/image_preview_commands.rs`
- Modify: `<WT>/crates/desktop/src/main.rs`（mod 声明 + generate_handler! + RunEvent）
- Modify: `<WT>/crates/desktop/capabilities/default.json`（windows 数组）

- [ ] **Step 1：新建 `image_preview_window.rs`**（镜像 `compact_editor_window.rs`）

```rust
//! 图片预览窗口：动态创建（非预建隐藏窗）。
//! 打开 → macOS 切 Regular（Dock 出现）；关闭 → RunEvent::Destroyed 路由回 Accessory。

use tauri::{ActivationPolicy, Manager, WebviewUrl, WebviewWindowBuilder};

const WIDTH: f64 = 880.0;
const HEIGHT: f64 = 620.0;
const MIN_WIDTH: f64 = 400.0;
const MIN_HEIGHT: f64 = 320.0;

pub const WINDOW_LABEL: &str = "image_preview_window";

pub fn create_image_preview_window(app_handle: &tauri::AppHandle) {
    #[cfg(target_os = "macos")]
    {
        let _ = app_handle.set_activation_policy(ActivationPolicy::Regular);
    }
    let _ = WebviewWindowBuilder::new(app_handle, WINDOW_LABEL, WebviewUrl::default())
        .title("图片预览")
        .inner_size(WIDTH, HEIGHT)
        .min_inner_size(MIN_WIDTH, MIN_HEIGHT)
        .decorations(true)
        .resizable(true)
        .center()
        .visible(true)
        .build();
}

/// 窗口销毁后恢复 Accessory（Dock 图标隐藏），与 compact_editor 一致。
#[cfg(target_os = "macos")]
pub fn on_image_preview_closed(app_handle: &tauri::AppHandle) {
    let _ = app_handle.set_activation_policy(ActivationPolicy::Accessory);
}
```

- [ ] **Step 2：新建 `image_preview_commands.rs`**（镜像 `compact_editor_commands.rs` 的 PENDING 模式）

```rust
//! 图片预览命令层：PENDING 暂存 + 开/取/关三个命令。
//! 模式同 compact_editor：open 先写 PENDING 再建窗/聚焦；前端 mount 调 get_pending_image 取走。

use std::sync::Mutex;
use tauri::{Emitter, Manager};

use crate::image_preview_window::{create_image_preview_window, WINDOW_LABEL};

/// 跨窗口传递的预览载荷。rename_all=camelCase → 前端拿到 { imageId }。
#[derive(Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PendingImage {
    pub image_id: i64,
}

static PENDING: Mutex<Option<PendingImage>> = Mutex::new(None);

fn store_pending(image_id: i64) {
    *PENDING.lock().unwrap() = Some(PendingImage { image_id });
}

fn take_pending() -> Option<PendingImage> {
    PENDING.lock().unwrap().take()
}

/// 打开图片预览：写 PENDING；已存在则 emit load 推送新 id + 聚焦，否则建窗。
#[tauri::command]
pub fn open_image_preview(image_id: i64, app_handle: tauri::AppHandle) {
    store_pending(image_id);
    if let Some(window) = app_handle.get_webview_window(WINDOW_LABEL) {
        let _ = window.emit("image-preview://load", PendingImage { image_id });
        let _ = window.show();
        let _ = window.set_focus();
    } else {
        create_image_preview_window(&app_handle);
    }
}

/// 前端 mount 时拉取（take 清空）。
#[tauri::command]
pub fn get_pending_image() -> Option<PendingImage> {
    take_pending()
}

/// 关闭预览窗口（触发 Destroyed → macOS 切 Accessory）。
#[tauri::command]
pub fn close_image_preview(app_handle: tauri::AppHandle) {
    if let Some(window) = app_handle.get_webview_window(WINDOW_LABEL) {
        let _ = window.close();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pending_store_and_take_roundtrip() {
        let _ = take_pending(); // 清残留
        store_pending(42);
        let got = take_pending().expect("take 应返回刚写入的载荷");
        assert_eq!(got.image_id, 42);
        assert!(take_pending().is_none(), "第二次 take 应为空");
    }
}
```

- [ ] **Step 3：main.rs 注册 mod + 命令 + RunEvent 路由**

定位锚点：

```bash
cd <WT> && grep -n "mod compact_editor_commands\|mod compact_editor_window\|open_compact_editor\|compact_editor_window =>" crates/desktop/src/main.rs
```

3a. mod 声明区（紧跟 compact_editor 两行后）加：

```rust
mod image_preview_commands;
mod image_preview_window;
```

3b. `generate_handler!` 宏里，compact_editor 三命令后加：

```rust
image_preview_commands::open_image_preview,
image_preview_commands::get_pending_image,
image_preview_commands::close_image_preview,
```

3c. RunEvent::WindowEvent { Destroyed } 的 match 里，`compact_editor_window` 分支旁加：

```rust
"image_preview_window" => image_preview_window::on_image_preview_closed(&app_handle),
```

（macOS 分支内；非 macOS 不需调用，`on_image_preview_closed` 本身 `#[cfg(target_os="macos")]`。若 RunEvent 处理器在非 macOS 编译报「未使用」，参考 compact_editor 的同位置写法保持一致——它在 main.rs 已通过 `#[cfg]` 处理。）

- [ ] **Step 4：capabilities ACL — windows 数组加 label**

`<WT>/crates/desktop/capabilities/default.json` 第 4 行 windows 数组追加 `"image_preview_window"`：

```json
"windows": ["main", "result_window", "settings_window", "clipboard_window", "notepad_window", "compact_editor_window", "image_preview_window", "screenshot_*"],
```

> 理由：动态窗口 label 未列入 capability → 前端 invoke/emit/listen 全被 ACL 静默拦（见 memory `tauri-dynamic-window-capability`）。诊断信号：后端 emit 能收、前端 emit 回不来。

- [ ] **Step 5：测试 + 编译验证**

Run: `cargo test --manifest-path <WT>/Cargo.toml -p octopus-desktop image_preview_commands`
Expected: `pending_store_and_take_roundtrip` PASS。

Run: `cargo build --manifest-path <WT>/Cargo.toml -p octopus-desktop`
Expected: 编译成功（含 RunEvent 处理器）。

- [ ] **Step 6：提交**

```bash
git -C <WT> add crates/desktop/src/image_preview_window.rs crates/desktop/src/image_preview_commands.rs crates/desktop/src/main.rs crates/desktop/capabilities/default.json
git -C <WT> commit -m "feat(desktop): 图片预览窗口 + PENDING 命令 + ACL 注册"
```

---

## Task 3: 后端 — 图片获取/保存/复制命令

**Files:**
- Modify: `<WT>/crates/desktop/src/clipboard_commands.rs`（加 3 命令）
- Modify: `<WT>/crates/desktop/src/main.rs`（generate_handler! 注册 3 命令）

**背景**：`clipboard_commands.rs` 顶部已 `use base64::{Engine, engine::general_purpose};` + `use octopus_clipboard::{ClipboardHandle, ...}` + `State<'_, Arc<ClipboardHandle>>` 模式已建立。`ClipboardHandle::write_image(&[u8])`（handle.rs:50）内部已 `from_bytes`+`set_image`，故复制无需碰 `RustImageData`。

- [ ] **Step 1：加 `get_image_full`**（镜像 `get_image_thumb`，读 `blob` 而非 `thumb`）

定位：`grep -n "pub async fn get_image_thumb\|get_image_blob" <WT>/crates/desktop/src/clipboard_commands.rs`。在 `get_image_thumb` 旁加：

```rust
/// 取图片全分辨率（image_data.blob）→ data URL（base64 + WebP 前缀）。
/// 前端 ImagePreview 用它加载到 <img>/canvas。
#[tauri::command]
pub async fn get_image_full(id: i64) -> Result<String, String> {
    let hash = octopus_infra::db::with_db(|conn| {
        octopus_clipboard::store::get_image_blob(conn, &image_hash_for(conn, id)?)
            .ok_or_else(|| "图片数据缺失".to_string())
    })
    .map_err(|e| e.to_string())??;
    // 注意：上面的闭包返回需对齐 get_image_thumb 的写法（见现有实现）。
    // 简化版：直接复用 get_image_thumb 的取 hash → 取 blob → encode 逻辑，仅把 get_image_thumb 换成 get_image_blob。
    Ok(format!("data:image/webp;base64,{}", general_purpose::STANDARD.encode(&hash)))
}
```

> **落地注意**：上面伪表达仅为示意。实现时**逐字对照现有 `get_image_thumb` 的函数体**（它已正确处理「取 hash + 取 thumb + encode + 返回 data URL」），把其中 `get_image_thumb(conn, &hash)` 换成 `get_image_blob(conn, &hash)`、mime 仍是 `image/webp`（blob 存的是 WebP）。保持错误处理、Option 展开方式与之一致，避免类型不齐。

- [ ] **Step 2：加 `save_image_dialog`**（镜像 `screenshot_commands::save_screenshot_dialog`，**去掉截图专属清理**）

```rust
/// 弹系统保存对话框，把前端合成的标注 PNG（base64）存到用户指定路径。
#[tauri::command]
pub async fn save_image_dialog(
    png_base64: String,
    app_handle: tauri::AppHandle,
) -> Result<(), String> {
    let png_bytes = general_purpose::STANDARD
        .decode(&png_base64)
        .map_err(|e| format!("base64 解码失败: {}", e))?;

    use tauri_plugin_dialog::DialogExt;
    let save_path = app_handle
        .dialog()
        .file()
        .add_filter("PNG 图片", &["png"])
        .set_file_name("image.png")
        .blocking_save_file();

    if let Some(path) = save_path {
        let path = path.as_path().ok_or("无效路径")?;
        std::fs::write(path, &png_bytes).map_err(|e| e.to_string())?;
        log::info!("Image preview saved to {}", path.display());
    }
    Ok(())
}
```

- [ ] **Step 3：加 `copy_image_to_clipboard`**（decode → `handle.write_image`）

```rust
/// 把前端合成的标注 PNG（base64）写入系统剪贴板。
#[tauri::command]
pub async fn copy_image_to_clipboard(
    png_base64: String,
    handle: State<'_, Arc<ClipboardHandle>>,
) -> Result<(), String> {
    let png_bytes = general_purpose::STANDARD
        .decode(&png_base64)
        .map_err(|e| format!("base64 解码失败: {}", e))?;
    handle.write_image(&png_bytes).map_err(|e| e.to_string())
}
```

- [ ] **Step 4：main.rs generate_handler! 注册 3 命令**

定位 `get_image_thumb` 注册处（`grep -n "get_image_thumb" <WT>/crates/desktop/src/main.rs`），其旁加：

```rust
clipboard_commands::get_image_full,
clipboard_commands::save_image_dialog,
clipboard_commands::copy_image_to_clipboard,
```

- [ ] **Step 5：编译验证**

Run: `cargo build --manifest-path <WT>/Cargo.toml -p octopus-desktop`
Expected: 成功。若 `get_image_full` 闭包返回类型报错，回到 Step 1 对齐 `get_image_thumb` 的写法。

- [ ] **Step 6：提交**

```bash
git -C <WT> add crates/desktop/src/clipboard_commands.rs crates/desktop/src/main.rs
git -C <WT> commit -m "feat(desktop): get_image_full / save_image_dialog / copy_image_to_clipboard 命令"
```

---

## Task 4: 前端 — ImagePreview 组件（画布 + 标注交互）

**Files:**
- Create: `<WT>/crates/desktop/frontend/src/pages/ImagePreview/index.tsx`

**坐标空间约定**（自然像素）：
- 标注 `Annotation` 的坐标/线宽/字号均为**图像本征像素**（与显示尺寸无关，resize 不错位）。
- `dispW/dispH` = contain-fit 后的显示尺寸；`natW/natH` = 图像本征尺寸。
- 鼠标 CSS 坐标 → 自然：`nx = cssX / dispW * natW`，`ny = cssY / dispH * natH`。
- 绘制：`ctx.save(); ctx.scale(dispW/natW, dispH/natH); drawAnnotation(ctx, ann); ctx.restore();`
- 合成保存/复制：离屏画布 natW×natH，`drawImage` + `drawAnnotation` 1:1（无 scale）。

- [ ] **Step 1：新建组件骨架 + 加载图片**

```tsx
import { useState, useRef, useEffect, useCallback } from "react";
import { invoke } from "@/lib/tauri";
import { listen } from "@tauri-apps/api/event";
import {
  type Annotation,
  type Tool,
  drawAnnotation,
  annBounds,
  hitTestAnnotationPrecise,
} from "@/lib/annotation";
import Toolbar from "./Toolbar";

export default function ImagePreview() {
  const canvasRef = useRef<HTMLCanvasElement>(null);
  const imgRef = useRef<HTMLImageElement | null>(null);

  const [imageId, setImageId] = useState<number | null>(null);
  const [dataUrl, setDataUrl] = useState<string | null>(null);
  const [natW, setNatW] = useState(0);
  const [natH, setNatH] = useState(0);
  // dispW/dispH 由 contain-fit 在 draw 时算（依赖窗口尺寸），存 ref 避免重渲染抖动
  const dispRef = useRef({ w: 0, h: 0, ox: 0, oy: 0 });

  const [tool, setTool] = useState<Tool>("none");
  const [toolColor, setToolColor] = useState("#ef4444");
  const [toolWidth, setToolWidth] = useState(3);
  const [toolFontSize, setToolFontSize] = useState(20);
  const [annotations, setAnnotations] = useState<Annotation[]>([]);
  const [alwaysOnTop, setAlwaysOnTop] = useState(false);

  // 交互 refs
  const drawingRef = useRef<Annotation | null>(null);
  const dragRef = useRef<{ idx: number; dx: number; dy: number } | null>(null);
  const startRef = useRef({ x: 0, y: 0 });
  const textDraftRef = useRef<{ nx: number; ny: number } | null>(null);
  const [textDraft, setTextDraft] = useState<{ nx: number; ny: number; val: string } | null>(null);

  // —— mount：取 PENDING + 监听并发再开的 load 事件 ——
  useEffect(() => {
    invoke<{ imageId: number } | null>("get_pending_image").then((p) => {
      if (p) setImageId(p.imageId);
    });
    const unlisten = listen<{ imageId: number }>("image-preview://load", (e) => {
      setImageId(e.payload.imageId);
      setAnnotations([]); // 切图清空标注
    });
    return () => { unlisten.then((f) => f()); };
  }, []);

  // —— imageId 变 → 拉全图 ——
  useEffect(() => {
    if (imageId == null) return;
    invoke<string>("get_image_full", { id: imageId })
      .then((url) => {
        setDataUrl(url);
        setAnnotations([]);
      })
      .catch((e) => console.error(e));
  }, [imageId]);
  // ...（继续 Step 2 draw、Step 3 鼠标、Step 4 工具栏接线、Step 5 compose）
}
```

- [ ] **Step 2：`draw` —— contain-fit + 图片 + 标注 + 草稿**

```tsx
  const draw = useCallback(() => {
    const canvas = canvasRef.current;
    const img = imgRef.current;
    if (!canvas || !img || !natW || !natH) return;
    const cssW = canvas.clientWidth;
    const cssH = canvas.clientHeight;
    const dpr = window.devicePixelRatio || 1;
    canvas.width = Math.round(cssW * dpr);
    canvas.height = Math.round(cssH * dpr);
    const ctx = canvas.getContext("2d")!;
    ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
    ctx.clearRect(0, 0, cssW, cssH);

    // contain-fit
    const scale = Math.min(cssW / natW, cssH / natH);
    const dispW = natW * scale;
    const dispH = natH * scale;
    const ox = (cssW - dispW) / 2;
    const oy = (cssH - dispH) / 2;
    dispRef.current = { w: dispW, h: dispH, ox, oy };
    ctx.drawImage(img, ox, oy, dispW, dispH);

    // 标注：自然坐标 → 先平移到显示原点 + 缩放到 disp
    ctx.save();
    ctx.translate(ox, oy);
    ctx.scale(scale, scale);
    for (const ann of annotations) drawAnnotation(ctx, ann);
    if (drawingRef.current) drawAnnotation(ctx, drawingRef.current);
    ctx.restore();

    // 文字草稿（DOM <textarea> 叠加，此处不画）
    void textDraft; void tool;
  }, [natW, natH, annotations, textDraft, tool]);

  useEffect(() => { draw(); }, [draw]);
  // 窗口 resize 重绘
  useEffect(() => {
    const onResize = () => draw();
    window.addEventListener("resize", onResize);
    return () => window.removeEventListener("resize", onResize);
  }, [draw]);
```

`<img>` 元素：渲染一个隐藏 `<img>`（`display:none`，仅作解码源），`onLoad` 记录 `natW/natH`：

```tsx
  {dataUrl && (
    <img
      ref={imgRef}
      src={dataUrl}
      alt=""
      style={{ display: "none" }}
      onLoad={(e) => {
        setNatW(e.currentTarget.naturalWidth);
        setNatH(e.currentTarget.naturalHeight);
      }}
    />
  )}
```

- [ ] **Step 3：鼠标交互（自然坐标转换 + 各工具）**

```tsx
  // CSS 坐标（相对 canvas）→ 自然坐标
  const toNatural = (cssX: number, cssY: number) => {
    const { w, h, ox, oy } = dispRef.current;
    const scale = w && h ? (natW / w) : 1; // nat/disp
    return { nx: (cssX - ox) * scale, ny: (cssY - oy) * scale };
  };

  const canvasCoords = (e: React.MouseEvent) => {
    const rect = canvasRef.current!.getBoundingClientRect();
    return { cssX: e.clientX - rect.left, cssY: e.clientY - rect.top };
  };

  const onMouseDown = (e: React.MouseEvent) => {
    if (e.button !== 0) return;
    const { cssX, cssY } = canvasCoords(e);
    const { nx, ny } = toNatural(cssX, cssY);
    startRef.current = { x: nx, y: ny };

    // 文字草稿确认
    if (textDraftRef.current && textDraftRef.current.val.trim()) {
      commitText();
    } else {
      setTextDraft(null);
      textDraftRef.current = null;
    }

    if (tool === "none") {
      // 选择/移动：命中已有标注
      const idx = hitTestAnnotationPrecise(nx, ny, annotations);
      if (idx != null) {
        dragRef.current = { idx, dx: nx - annotations[idx].x1, dy: ny - annotations[idx].y1 };
      }
      return;
    }

    if (tool === "text") {
      textDraftRef.current = { nx, ny };
      setTextDraft({ nx, ny, val: "" });
      return;
    }

    // rect/oval/line 开始绘制
    drawingRef.current = {
      type: tool as Annotation["type"],
      x1: nx, y1: ny, x2: nx, y2: ny,
      color: toolColor, lineWidth: toolWidth,
    };
  };

  const onMouseMove = (e: React.MouseEvent) => {
    const { cssX, cssY } = canvasCoords(e);
    const { nx, ny } = toNatural(cssX, cssY);
    if (dragRef.current) {
      const { idx, dx, dy } = dragRef.current;
      setAnnotations((prev) => prev.map((a, i) => {
        if (i !== idx) return a;
        const mx = nx - dx, my = ny - dy;
        const w = a.x2 - a.x1, h = a.y2 - a.y1;
        return { ...a, x1: mx, y1: my, x2: mx + w, y2: my + h };
      }));
      return;
    }
    if (drawingRef.current) {
      drawingRef.current = { ...drawingRef.current, x2: nx, y2: ny };
      draw();
    }
  };

  const onMouseUp = () => {
    if (drawingRef.current) {
      const ann = drawingRef.current;
      drawingRef.current = null;
      // 过滤误触（过小）
      if (Math.abs(ann.x2 - ann.x1) > 3 || Math.abs(ann.y2 - ann.y1) > 3) {
        setAnnotations((prev) => [...prev, ann]);
      } else {
        draw();
      }
    }
    dragRef.current = null;
  };

  const commitText = () => {
    const d = textDraftRef.current;
    if (d && d.val.trim()) {
      setAnnotations((prev) => [...prev, {
        type: "text", x1: d.nx, y1: d.ny, x2: d.nx, y2: d.ny,
        text: d.val, color: toolColor, fontSize: toolFontSize,
      }]);
    }
    textDraftRef.current = null;
    setTextDraft(null);
  };

  const undo = () => setAnnotations((prev) => prev.slice(0, -1));
```

- [ ] **Step 4：compose 出口（保存/复制/OCR/置顶）**

```tsx
  // 把 图像 + 标注 合成到自然尺寸 PNG → base64（不含 data: 前缀）
  const composePngBase64 = async (): Promise<string> => {
    const img = imgRef.current!;
    const c = document.createElement("canvas");
    c.width = natW; c.height = natH;
    const ctx = c.getContext("2d")!;
    ctx.drawImage(img, 0, 0, natW, natH);
    for (const ann of annotations) drawAnnotation(ctx, ann);
    const dataUrl = c.toDataURL("image/png");
    return dataUrl.substring(dataUrl.indexOf(",") + 1);
  };

  const handleSave = async () => {
    try {
      const b64 = await composePngBase64();
      await invoke("save_image_dialog", { pngBase64: b64 });
    } catch (e) { console.error(e); }
  };

  const handleCopy = async () => {
    try {
      const b64 = await composePngBase64();
      await invoke("copy_image_to_clipboard", { pngBase64: b64 });
    } catch (e) { console.error(e); }
  };

  const handleOcr = async () => {
    if (imageId == null) return;
    try {
      // 复用现有 ocr_image（对原图 OCR）；标注仅用于视觉，不影响识别
      const text = await invoke<string>("ocr_image", { id: imageId });
      await navigator.clipboard.writeText(text).catch(() => {});
      // 简单反馈：把识别文本作为文字标注贴到画面左上
      if (text) {
        setAnnotations((prev) => [...prev, {
          type: "text", x1: 16, y1: 16, x2: 16, y2: 16,
          text, color: "#f59e0b", fontSize: 24,
        }]);
      }
    } catch (e) { console.error(e); }
  };

  const toggleAlwaysOnTop = async () => {
    const next = !alwaysOnTop;
    setAlwaysOnTop(next);
    try {
      const { getCurrentWindow } = await import("@tauri-apps/api/window");
      await getCurrentWindow().setAlwaysOnTop(next);
    } catch (e) { console.error(e); }
  };

  const close = async () => {
    try { await invoke("close_image_preview"); } catch (e) { console.error(e); }
  };
  // Esc 关闭
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") close();
      if ((e.metaKey || e.ctrlKey) && e.key === "z") undo();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [annotations]);
```

- [ ] **Step 5：渲染（灯箱暗场 + 棋盘格画布 canvas + 文字草稿 textarea + 浮动工具栏 + 底部 EXIF 条）**

> 2026-07-01 按 frontend-design 重做：外层从「贴顶深条 flex-col」改为「灯箱暗场 `#1c1917` + 全屏滚动画布 + 浮动白卡工具栏（fixed 居中）+ 底部 EXIF 状态条」。canvas 加棋盘格 CSS 底（透明 PNG 可读）。默认 1:1（zoom=1），缩放/平移见 §3.4。

```tsx
  return (
    // 灯箱暗场：工具卡与底部 EXIF 条均 fixed 浮于其上
    <div className="relative h-screen overflow-hidden select-none" style={{ background: "#1c1917" }}>
      <Toolbar
        tool={tool} setTool={setTool}
        toolColor={toolColor} setToolColor={setToolColorSync}
        toolWidth={toolWidth} setToolWidth={setToolWidthSync}
        toolFontSize={toolFontSize} setToolFontSize={setToolFontSizeSync}
        alwaysOnTop={alwaysOnTop} onToggleTop={toggleAlwaysOnTop}
        onSave={handleSave} onCopy={handleCopy} onOcr={handleOcr}
        onUndo={undo} canUndo={annotations.length > 0}
      />
      <div className="relative flex-1 overflow-hidden">
        <canvas
          ref={canvasRef}
          className="absolute inset-0 w-full h-full"
          style={{ cursor: tool === "none" ? "default" : "crosshair" }}
          onMouseDown={onMouseDown}
          onMouseMove={onMouseMove}
          onMouseUp={onMouseUp}
        />
        {/* 文字草稿：DOM textarea 叠在画布上，输入完点别处 commit */}
        {textDraft && (() => {
          const { w, ox, oy } = dispRef.current;
          const scale = w / natW;
          const left = ox + textDraft.nx * scale;
          const top = oy + textDraft.ny * scale;
          return (
            <textarea
              autoFocus
              value={textDraft.val}
              onChange={(e) => {
                const v = e.target.value;
                setTextDraft({ ...textDraft, val: v });
                textDraftRef.current = { nx: textDraft.nx, ny: textDraft.ny, val: v } as any;
                // 注意：textDraftRef 需同步 val，见下注
              }}
              onBlur={commitText}
              className="absolute bg-white/90 text-black outline-none resize-none px-1"
              style={{ left, top, fontSize: toolFontSize * scale, minWidth: 120 }}
            />
          );
        })()}
      </div>
    </div>
  );
}
```

> **注**：`textDraftRef` 同步——为保证 `commitText` 读到最新 val，`onChange` 里把 ref 也更新。上面的 `as any` 占位需在实现时用正确类型（`{nx,ny,val}` 全字段）。落地时确保 `textDraftRef.current` 与 `textDraft.val` 同步（参考 Screenshot 的 `textDraftRef`/`editTextOrigRef` 双写模式）。

- [ ] **Step 6：类型检查验证**

Run: `npm --prefix <WT>/crates/desktop/frontend run build`
Expected: tsc 通过（Toolbar 尚未建会有 import 报错 → 先建 Task 5 的 Toolbar 骨架再 build，或把 Step 6 放到 Task 5 后）。本任务内可先 `npx tsc -b` 查类型。

- [ ] **Step 7：提交**

```bash
git -C <WT> add crates/desktop/frontend/src/pages/ImagePreview/index.tsx
git -C <WT> commit -m "feat(desktop): ImagePreview 组件（画布 + 标注交互 + compose 出口）"
```

---

## Task 5: 前端 — 工具栏 Toolbar 组件

**Files:**
- Create: `<WT>/crates/desktop/frontend/src/pages/ImagePreview/Toolbar.tsx`

**设计**（2026-07-01 按 frontend-design 重做：浮动白卡对齐截图主工具栏，属性浮窗 1:1 复刻截图 `ToolPropsPopover`；内联 style 与截图同出处）：
- 工具卡：`position:fixed; left:50%; top:8; translateX(-50%)`，白底 r8 + `box-shadow:0 4px 16px rgba(0,0,0,0.3)`（截图同款）。
- `ToolButton`：32×32 r6，激活 `#3b82f6` 蓝底白字、否则透明 `#44403c` hover `rgba(0,0,0,0.06)`，图标 18px。`Divider` 竖线 `rgba(0,0,0,0.08)`。
- 布局分组（左→右）：操作(保存/复制/OCR) ｜ 标注(选择/矩形/椭圆/直线/文字/撤销) ｜ 缩放(缩小/百分比/放大) ｜ 置顶。
- 属性浮窗：`tool !== "none"` 时从工具卡左下 `absolute top:calc(100%+6px)` 自动浮出（无单独调色板按钮）；白卡 r10 + 两行（滑轨+当前色圆 / 8 预设色 active 蓝环）；文字→字号 10–48、其余→粗细 1–10；不放 `<input type="color">` 调色板（YAGNI）。
- 缩放百分比等宽 `SF Mono` + `tabular-nums`，点击重置 100%；OCR 成功后按钮换绿勾 1.5s。
- 无关闭按钮（用窗口右上角 × 或 Esc）

- [x] **Step 1：新建 Toolbar.tsx**

> 已实现（`crates/desktop/frontend/src/pages/ImagePreview/Toolbar.tsx`）。结构见本 Task 顶部「设计」段：浮动白卡（fixed 居中）+ `ToolButton`(32×32/激活 `#3b82f6`) + `Divider`，分组 操作｜标注｜缩放｜置顶，属性浮窗 `tool!=="none"` 时自动浮出。
>
> **演进**：初版是贴顶 `neutral-800` 横条 + 单独 `Palette` 按钮触发浮窗；2026-07-01 按 frontend-design 重做为浮动白卡 + 自动浮出（对齐截图），旧版代码已废弃不再保留于此。
```

- [ ] **Step 2：类型检查 + 构建**

Run: `npm --prefix <WT>/crates/desktop/frontend run build`
Expected: tsc + vite 成功（ImagePreview + Toolbar 一起编译通过）。

- [ ] **Step 3：提交**

```bash
git -C <WT> add crates/desktop/frontend/src/pages/ImagePreview/Toolbar.tsx
git -C <WT> commit -m "feat(desktop): ImagePreview 工具栏（工具 + 颜色粗细浮窗 + 动作）"
```

---

## Task 6: 前端 — 路由 + 剪贴板入口

**Files:**
- Modify: `<WT>/crates/desktop/frontend/src/App.tsx`
- Modify: `<WT>/crates/desktop/frontend/src/pages/Clipboard/ClipboardItem.tsx`

- [ ] **Step 1：App.tsx 加路由 case**

定位 `switch (label)`（`grep -n "compact_editor_window\|switch (label)\|case \"" <WT>/crates/desktop/frontend/src/App.tsx`）。在 `case "compact_editor_window"` 旁加：

```tsx
case "image_preview_window": return <ImagePreview />;
```

并在文件顶部 import：

```tsx
import ImagePreview from "./pages/ImagePreview";
```

- [ ] **Step 2：ClipboardItem.tsx 加「预览」入口**

图片项操作组（save/ocr 按钮所在 `<div className="flex-shrink-0 flex items-center gap-0.5">`）最前加一个预览按钮。import 加 `Maximize2`：

```tsx
import { Star, Mic, Type, Image as ImageIcon, FileText, Trash2, Download, FolderOpen, ScanText, Loader2, Check, SquarePen, Maximize2 } from "lucide-react";
```

在 `{item.item_type === "image" && (` 的保存按钮**之前**插入：

```tsx
{item.item_type === "image" && (
  <button
    className="p-0.5 opacity-0 group-hover:opacity-60 hover:!opacity-100 transition-opacity"
    onClick={(e) => {
      e.stopPropagation();
      invoke("open_image_preview", { imageId: item.id }).catch(console.error);
    }}
    title="预览"
  >
    <Maximize2 className="w-3.5 h-3.5 text-muted-foreground hover:text-foreground" />
  </button>
)}
```

> 双击仍走 `paste_clipboard_item`（不变）；预览走独立按钮，互不冲突。

- [ ] **Step 3：构建验证 + 提交 dist**

Run: `npm --prefix <WT>/crates/desktop/frontend run build`
Expected: 成功，`crates/desktop/dist` 更新。

```bash
git -C <WT> add crates/desktop/frontend/src/App.tsx crates/desktop/frontend/src/pages/Clipboard/ClipboardItem.tsx crates/desktop/dist
git -C <WT> commit -m "feat(desktop): 图片预览路由 + 剪贴板图片项预览入口"
```

---

## Task 7: 构建总验 + 文档同步

**Files:**
- Modify: `<WT>/docs/architecture.md`

- [ ] **Step 1：后端总编译 + 测试**

Run: `cargo build --manifest-path <WT>/Cargo.toml -p octopus-desktop && cargo test --manifest-path <WT>/Cargo.toml -p octopus-desktop`
Expected: 编译成功，所有测试通过（含新 `pending_store_and_take_roundtrip`）。

- [ ] **Step 2：前端总构建**

Run: `npm --prefix <WT>/crates/desktop/frontend run build`
Expected: 成功。

- [ ] **Step 3：architecture.md 同步**

在桌面模块说明里补：图片预览窗口（`image_preview_window` / `image_preview_commands` / `ImagePreview` 组件）、共享标注核心 `frontend/src/lib/annotation.ts`、新增命令（`open_image_preview`/`get_pending_image`/`close_image_preview`/`get_image_full`/`save_image_dialog`/`copy_image_to_clipboard`）。

- [ ] **Step 4：提交 + 交 e2e**

```bash
git -C <WT> add docs/architecture.md
git -C <WT> commit -m "docs(desktop): 同步图片预览模块到 architecture.md"
```

至此代码完整、构建全绿。**交用户做 e2e**（功能完成且 e2e 通过后再考虑合并 main）。

---

## Spec Coverage

| Spec（2026-07-01-image-preview-design.md）section | 覆盖 Task |
|---|---|
| 轻工具栏预览（窗口 + 工具栏） | T2, T5 |
| 标注：圆/线/矩形/文字 | T1, T4 |
| 选择(移动)/撤销/颜色·粗细浮窗 | T4(选择/撤销), T5(浮窗) |
| 保存/复制/OCR/置顶 | T3(命令), T4(compose/接线), T5(按钮) |
| 共享核心抽取（不重复） | T1 |
| 数据流：open→PENDING→get_pending→get_image_full | T2, T3, T4 |
| 动态窗口 ACL | T2 Step 4 |
| macOS 激活策略 Regular/Accessory | T2 Step 1/3 |
| 贴图钉屏（未来，仅打基础） | T1 共享核心 + T4 compose 复用（本期不建贴图窗口） |

## 风险提示

- **`hitTestAnnotationPrecise` 调用点**：抽取后签名变 `(mx,my,anns)`，Screenshot 内所有调用必须补参（Task 1 Step 3 grep 全覆盖）。漏改 → tsc 报错（能拦住）。
- **`get_image_full` 闭包返回类型**：须对齐 `get_image_thumb` 现有写法，别凭空写返回逻辑。
- **textDraft ref 同步**：textarea 受控 + ref 双写（参考 Screenshot 模式），否则 commit 读不到最新输入。
- **dist 提交**：Task 6/任何前端变更后必须 build 并提交 dist，否则 Tauri 跑旧前端。
- **不合并 main**：所有提交留 worktree 分支，e2e 通过后再议。


---

## 来自原文件 `2026-07-01-pin-screenshot.md`

# 贴图功能（Pin to Desktop）实施计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 截图工具栏新增钉子按钮，点击后选区图片以原生 NSWindow 钉在桌面，支持拖拽/缩放/右键关闭

**Architecture:** objc2 创建原生 NSWindow + NSImageView（不创建 WebView，~3MB/个），PinWindow trait 跨平台抽象

**Tech Stack:** Rust + objc2/objc2-app-kit + objc2-foundation + Tauri 2

**Spec:** `docs/superpowers/specs/2026-07-01-pin-screenshot-design.md`

---

## 文件结构

| 文件 | 变更 | 责任 |
|---|---|---|
| `crates/desktop/src/pin_window.rs` | Create | PinWindow trait + macOS 实现（NSWindow 子类 + 拖拽/缩放/右键） |
| `crates/desktop/src/main.rs` | Modify | `mod pin_window` + 注册 `pin_screenshot` 命令 |
| `crates/desktop/src/screenshot_commands.rs` | Modify | 新增 `pin_screenshot` 命令（从 ALL_CAPTURES 裁剪选区 → PNG → PinWindow::create） |
| `crates/desktop/frontend/src/pages/Screenshot/index.tsx` | Modify | 工具栏钉子按钮 |
| `crates/desktop/frontend/public/icons/pin.svg` | Create | 钉子图标 |

---

### Task 1: pin_window.rs — PinWindow trait + macOS 基础窗口

**Files:**
- Create: `crates/desktop/src/pin_window.rs`
- Modify: `crates/desktop/src/main.rs`（加 `mod pin_window`）

- [ ] **Step 1: 创建 pin_window.rs 基本结构**

```rust
// crates/desktop/src/pin_window.rs
// 贴图功能：原生窗口钉在桌面，支持拖拽/缩放/右键关闭。
// 一期 macOS（NSWindow + NSImageView），二期 Win/Linux 替换实现。

/// 跨平台贴图窗口抽象。
pub trait PinWindow {
    /// 创建贴图窗口。
    /// png_data: PNG 字节
    /// x, y: 选区全局 Quartz 逻辑坐标（原点左下）
    /// width, height: 逻辑像素尺寸
    fn create(png_data: &[u8], x: f64, y: f64, width: f64, height: f64);
}

#[cfg(target_os = "macos")]
mod macos;
```

- [ ] **Step 2: main.rs 加 mod pin_window**

在 `crates/desktop/src/main.rs` 的 `mod` 声明区加：

```rust
mod pin_window;
```

- [ ] **Step 3: 验证编译**

Run: `cargo build -p octopus-desktop --features embedded`
Expected: 编译通过（macos 模块还没内容，先加空壳）

---

### Task 2: macOS NSWindow 创建 + NSImageView 显示图片

**Files:**
- Create: `crates/desktop/src/pin_window/macos.rs`（或在 pin_window.rs 内）

- [ ] **Step 1: 实现 macOS PinWindow**

在 `pin_window.rs` 中实现 macOS 版本：

```rust
#[cfg(target_os = "macos")]
mod macos_impl {
    use objc2::rc::Retained;
    use objc2::runtime::AnyObject;
    use objc2::{class, msg_send, sel, msg_send_id};
    use objc2_app_kit::{NSWindow, NSView, NSImageView, NSImage, NSWindowStyleMask};
    use objc2_foundation::{NSRect, NSPoint, NSSize, NSData, mainQueue};

    pub fn create_pin_window(png_data: &[u8], x: f64, y: f64, width: f64, height: f64) {
        unsafe {
            // 1. 创建 NSImage from PNG data
            let nsdata = NSData::with_bytes(png_data);
            let image: Retained<NSImage> = msg_send_id![
                msg_send_id![class!(NSImage), alloc],
                initWithData: &nsdata
            ].unwrap();

            // 2. 创建 NSImageView
            let frame = NSRect {
                origin: NSPoint::ZERO,
                size: NSSize { width, height },
            };
            let image_view: Retained<NSImageView> = msg_send_id![
                msg_send_id![class!(NSImageView), alloc],
                initWithFrame: frame
            ].unwrap();
            let _: () = msg_send![&image_view, setImage: &image];

            // 3. 创建 NSWindow（borderless + floating）
            let window_frame = NSRect {
                origin: NSPoint { x, y },
                size: NSSize { width, height },
            };
            let window: Retained<NSWindow> = msg_send_id![
                msg_send_id![class!(NSWindow), alloc],
                initWithContentRect: window_frame,
                styleMask: NSWindowStyleMask::Borderless,
                backing: 2, // NSBackingStoreBuffered
                defer: false
            ].unwrap();

            let _: () = msg_send![&window, setLevel: 3]; // NSFloatingWindowLevel = 3
            let _: () = msg_send![&window, setHasShadow: true];
            let _: () = msg_send![&window, setOpaque: false];
            let _: () = msg_send![&window, setBackgroundColor: msg_send_id![class!(NSColor), clearColor]];

            // 4. 设置 contentView 为 image_view
            let content_view: Retained<NSView> = msg_send_id![&window, contentView];
            let _: () = msg_send![&content_view, addSubview: &image_view];

            // 5. 显示窗口
            let _: () = msg_send![&window, makeKeyAndOrderFront: None];
        }
    }
}
```

- [ ] **Step 2: 实现 PinWindow trait**

```rust
#[cfg(target_os = "macos")]
impl PinWindow for () {
    fn create(png_data: &[u8], x: f64, y: f64, width: f64, height: f64) {
        macos_impl::create_pin_window(png_data, x, y, width, height);
    }
}
```

- [ ] **Step 3: 验证编译**

Run: `cargo build -p octopus-desktop --features embedded`
Expected: 编译通过（可能有 unused warning）

---

### Task 3: pin_screenshot 后端命令

**Files:**
- Modify: `crates/desktop/src/screenshot_commands.rs`
- Modify: `crates/desktop/src/main.rs`（注册命令）

- [ ] **Step 1: 新增 pin_screenshot 命令**

在 `screenshot_commands.rs` 中新增：

```rust
/// 贴图：从 ALL_CAPTURES 裁剪选区 → PNG → 创建贴图窗口 → 关闭截图窗口
#[tauri::command]
pub async fn pin_screenshot(
    label: String,
    x: f64, y: f64, w: f64, h: f64,
    app_handle: tauri::AppHandle,
) -> Result<(), String> {
    // 从 ALL_CAPTURES 获取截图数据
    let capture = {
        let captures = ALL_CAPTURES.lock().unwrap();
        captures.iter()
            .find(|(l, _)| l == &label)
            .map(|(_, c)| c.clone())
            .ok_or("找不到截图数据")?
    };

    // 裁剪选区（物理坐标）
    let scale = app_handle.get_webview_window(&label)
        .and_then(|w| w.scale_factor().ok())
        .unwrap_or(1.0) as f64;
    let px = (x * scale) as u32;
    let py = (y * scale) as u32;
    let pw = (w * scale) as u32;
    let ph = (h * scale) as u32;

    let png_bytes = octopus_capx::capture::crop_region(&ScreenCapture {
        rgba_bytes: capture.rgba_bytes,
        width: capture.width,
        height: capture.height,
        monitor_x: 0, // 裁剪不需要
        monitor_y: 0,
    }, px, py, pw, ph).map_err(|e| e.to_string())?;

    // 获取选区全局 Quartz 坐标
    let win = app_handle.get_webview_window(&label)
        .ok_or("窗口不存在")?;
    #[cfg(target_os = "macos")]
    let (qx, qy) = {
        let primary_h = get_primary_screen_height();
        if let Some((cx, cy, _, ch)) = get_window_cocoa_frame(&win) {
            (cx + x, primary_h - (cy + ch) + y) // Cocoa 左下 → Quartz 左上
        } else {
            (x, y)
        }
    };
    #[cfg(not(target_os = "macos"))]
    let (qx, qy) = (x, y);

    // 创建贴图窗口
    <() as crate::pin_window::PinWindow>::create(&png_bytes, qx, qy, w, h);

    // 关闭截图窗口
    close_all_screenshot_windows(&app_handle);

    Ok(())
}
```

- [ ] **Step 2: main.rs 注册命令**

在 `tauri::generate_handler!` 中加：

```rust
screenshot_commands::pin_screenshot,
```

- [ ] **Step 3: 验证编译**

Run: `cargo build -p octopus-desktop --features embedded`
Expected: 编译通过

---

### Task 4: 前端钉子按钮

**Files:**
- Create: `crates/desktop/frontend/public/icons/pin.svg`
- Modify: `crates/desktop/frontend/src/pages/Screenshot/index.tsx`

- [ ] **Step 1: 创建 pin.svg 图标**

一个简单的钉子图标 SVG。

- [ ] **Step 2: 前端工具栏加钉子按钮**

在 OCR 按钮后面、保存按钮前面加：

```tsx
<button onClick={doPin} title="贴图" style={{ padding: "4px", width: 32, height: 32, display: "flex", alignItems: "center", justifyContent: "center", borderRadius: 6, border: "none", background: "transparent", cursor: "pointer" }}>
  <img src="icons/pin.svg" alt="贴图" className="w-[18px] h-[18px]" />
</button>
```

新增 `doPin` 函数：

```tsx
function doPin() {
  if (!sel) return;
  invoke("pin_screenshot", {
    label: winLabel,
    x: sel.x, y: sel.y, w: sel.w, h: sel.h,
  }).catch(() => {});
}
```

- [ ] **Step 3: 验证前端编译**

Run: `cd crates/desktop/frontend && npm run build`
Expected: 编译通过

---

### Task 5: 拖拽移动

**Files:** `crates/desktop/src/pin_window.rs`

- [x] **PinNSImageView（继承 NSImageView）处理 mouseDown**

`PinNSImageView` 重写 `mouseDown:` → 调用 `window.performWindowDragWithEvent(event)`。
系统原生拖拽，零抖动、跨屏正确，不需要手动记录坐标。

---

### Task 6: 滚轮缩放

**Files:** `crates/desktop/src/pin_window.rs`

- [x] **PinNSWindow 重写 scrollWheel:**

`scrollWheel:` → `scrollingDeltaY` × 0.01 缩放因子 → 以鼠标为中心 `setFrame_display`。
限制 20~10000px，NSImageView autoresizingMask 自动同步。

---

### Task 7: 右键菜单关闭

**Files:** `crates/desktop/src/pin_window.rs`

- [x] **PinNSWindow 重写 rightMouseDown: 弹出 NSMenu**

`rightMouseDown:` → `NSMenu` + `NSMenuItem("关闭", action: close)` → `popUpContextMenu`。
target-action 模式，点击后窗口 close。

---

### Task 8: 端到端验证

- [x] **全流程测试通过**

贴图创建 + 拖拽 + 缩放 + 右键关闭全部正常工作。

---

## Spec Coverage

| spec 章节 | 实现 task |
|---|---|
| §1 触发入口 | Task 4 |
| §2.2 PinWindow trait | Task 1 |
| §2.3 数据流 | Task 3 |
| §3.1 窗口创建 | Task 2 |
| §3.2 交互-拖拽 | Task 5 |
| §3.2 交互-缩放 | Task 6 |
| §3.2 交互-右键关闭 | Task 7 |
| §3.2 交互-Esc 关闭 | Task 7 |
| §3.3 事件处理 | Task 5-7 |
| §4 多实例 | Task 2（ARC 自动管理） |
| §5 坐标系 | Task 3 |


---

## 来自原文件 `2026-07-02-capx-canvas-anchored.md`

# Canvas-Anchored 匹配 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task.

**Goal:** 匹配输入源从上一帧改为画布底部 strip，根治累积漂移导致的丢内容。

**Architecture:** 移除 `self.reference` 字段，每帧从 `canvas_buf` 底部提取 STRIP_H 行 RGBA 转灰度作为匹配模板。`find_overlap_spatial_ext` 的 ref_buf 从完整帧变为 strip_h 高度的短灰度图，简化模板提取（ref_buf 本身即模板）。三级降级链同步改造。

**关联文档:** [spec](../specs/2026-07-02-capx-canvas-anchored-design.md)

---

## 关键约束

1. **API 零改动**：`new/process_frame/finalize/canvas/height` 签名不变
2. **灰度公式不变**：`(2126*R + 7152*G + 722*B) / 10000`
3. **现有 16 测试必须保持全绿**
4. **worktree**: `/Users/wudarui/workspace/agent/octopus/.worktrees/optimize-capx`

---

## Task 1: 新增 `extract_canvas_bottom_gray` + 移除 `self.reference` 字段

**Files:** `crates/capx/src/stitch.rs`

- [ ] **Step 1: 在 `impl Stitcher` 中（`is_stationary` 之前）新增 `extract_canvas_bottom_gray` 方法**

```rust
    /// 从画布底部提取 strip_h 行 RGBA 转灰度，作为 Canvas-Anchored 匹配模板。
    /// 无论多少帧匹配失败，画布底部始终是最新已确认内容 → 消除累积漂移。
    fn extract_canvas_bottom_gray(&self, strip_h: u32) -> GrayBuf {
        let row_bytes = self.canvas_w as usize * 4;
        let start_row = self.canvas_h.saturating_sub(strip_h);
        let mut data = Vec::with_capacity(strip_h as usize * self.canvas_w as usize);
        for y in start_row..self.canvas_h {
            let row_start = y as usize * row_bytes;
            for x in 0..self.canvas_w as usize {
                let off = row_start + x * 4;
                let r = self.canvas_buf[off] as u32;
                let g = self.canvas_buf[off + 1] as u32;
                let b = self.canvas_buf[off + 2] as u32;
                let luma = (2126 * r + 7152 * g + 722 * b) / 10000;
                data.push(luma as u8);
            }
        }
        GrayBuf { data, width: self.canvas_w as usize }
    }
```

- [ ] **Step 2: 移除 `self.reference` 字段**

从 `Stitcher` struct 中删除 `reference: GrayBuf` 字段及其文档注释。从 `new()` 中删除 `reference: GrayBuf { data: Vec::new(), width: 0 }` 初始化。

- [ ] **Step 3: 暂时注释掉所有 `self.reference` 引用（编译会报错，逐一处理）**

此时编译会有多处 `self.reference` 报错。**暂时不加 `#[allow(dead_code)]`**——Task 2-4 会逐一替换为 `self.extract_canvas_bottom_gray(STRIP_H)`。

先用 `grep -n "self.reference" crates/capx/src/stitch.rs` 列出所有引用点，了解范围。

- [ ] **Step 4: Commit（WIP，允许编译不过——但实际我们会在 Task 2 立即修复）**

> 实际上不要提交编译不过的代码。改为：Task 1 和 Task 2 一起完成后再提交。Task 1 只做 Step 1（加方法）+ Step 2（移除字段），然后立即进 Task 2 替换引用。

---

## Task 2: 改造 `process_frame` 主匹配为 Canvas-Anchored

**Files:** `crates/capx/src/stitch.rs`

- [ ] **Step 1: 修改 `process_frame` 初始化分支——移除 `self.reference = GrayBuf::from_rgba(frame)`**

初始化分支中删除 `self.reference = GrayBuf::from_rgba(frame);` 这一行。Canvas-Anchored 不需要在初始化时存 reference——下一帧直接从 canvas 底部提取。

- [ ] **Step 2: 修改 `process_frame` 主匹配分支——用画布底部替代 reference**

旧：
```rust
        let curr_buf = GrayBuf::from_rgba(frame);
        // ...
        let texture = estimate_texture_density(&curr_buf, &sample_cols, template_y);
        let sad_accept = self.dynamic_sad_accept(texture);
        let (dy, confidence, best_sad) = match find_overlap_spatial_ext(
            &self.reference,
            &curr_buf,
            ...
```

新：
```rust
        let curr_buf = GrayBuf::from_rgba(frame);
        let canvas_ref = self.extract_canvas_bottom_gray(STRIP_H);
        // ...
        let texture = estimate_texture_density(&canvas_ref, &sample_cols, 0);
        let sad_accept = self.dynamic_sad_accept(texture);
        let (dy, confidence, best_sad) = match find_overlap_spatial_ext(
            &canvas_ref,
            &curr_buf,
            ...
```

> **注意 `estimate_texture_density` 的 `template_y` 参数**：canvas_ref 只有 strip_h 行，template_y 应为 0（整个 canvas_ref 就是模板条）。

- [ ] **Step 3: 修改主匹配成功后的状态更新——移除 `self.reference = curr_buf`**

删除 `self.reference = curr_buf;` 这一行。Canvas-Anchored 不需要存 curr_buf 作为 reference。

但注意 `curr_buf` 在降级链中仍需使用（借用），且 `apply_fallback_match` 中也不再需要 `self.reference = curr_buf.clone()`。检查 `curr_buf` 的所有权——主匹配成功后 `curr_buf` 不再被 move，可以继续借用给后续代码（但主匹配成功直接 return，不会执行降级链）。

- [ ] **Step 4: 修改降级链——传入 `&canvas_ref` 替代 `self.reference`**

降级链中 `try_match` 和 `try_match_1d_projection` 内部引用 `&self.reference`。改为在 `process_frame` 中把 `canvas_ref` 传给降级链。

由于 `try_match` / `try_match_1d_projection` 是 `&self` 方法，无法接收外部参数。两个选择：
- **A**（推荐）：改为接收 `ref_buf: &GrayBuf` 参数
- **B**：改为 `&mut self` 并存 `canvas_ref` 到临时字段

选 A。修改 `try_match` 和 `try_match_1d_projection` 签名，新增 `ref_buf: &GrayBuf` 参数：

```rust
    fn try_match(
        &self,
        ref_buf: &GrayBuf,  // 新增
        curr: &GrayBuf,
        ...
    ) -> Option<(f64, f64, f64)> {
        find_overlap_spatial_ext(ref_buf, curr, ...)
    }
```

`try_match_1d_projection` 同理，把内部 `&self.reference` 替换为 `ref_buf`。

降级链调用处传入 `&canvas_ref`。

- [ ] **Step 5: 修改 `apply_fallback_match`——移除 `self.reference = curr_buf.clone()`**

删除 `self.reference = curr_buf.clone();` 这一行。

- [ ] **Step 6: 编译验证**

Run: `cargo check -p octopus-capx 2>&1 | tail -5`
Expected: `Finished`（无 `self.reference` 引用残留）

若有 `self.reference` 残留报错，用 `grep -n "self.reference" crates/capx/src/stitch.rs` 定位并修复。

- [ ] **Step 7: 运行测试**

Run: `cargo test -p octopus-capx 2>&1 | tail -8`
Expected: 16 passed

- [ ] **Step 8: Commit**

```bash
git add crates/capx/src/stitch.rs
git commit -m "feat(capx): process_frame 改为 Canvas-Anchored 匹配（画布底部 strip 替代 reference 帧）"
```

---

## Task 3: 改造 `find_overlap_spatial_ext` 适配短 ref_buf + 简化模板提取

**Files:** `crates/capx/src/stitch.rs`

> Canvas-Anchored 后，ref_buf 只有 strip_h 行（画布底部），不再需要 `extract_template` 单独提取模板——ref_buf 本身就是模板。

- [ ] **Step 1: 修改 `find_overlap_spatial_ext` 内部——ref_buf 即模板，简化 extract_template 调用**

当前 `extract_template` 从 ref_buf 的 `[template_y, template_y+strip_h)` 行提取模板。Canvas-Anchored 后 ref_buf 本身就只有 strip_h 行，template_y 恒为 0。

把 `extract_template(ref_buf, template_y, &sample_cols, strip_h)` 改为直接从 ref_buf 第 0 行开始提取（template_y = 0）。

或者更简洁：直接传 `template_y = 0` 给 `extract_template`。但 `template_y` 在 `find_overlap_spatial_ext` 中还用于计算 `min_y_offset`/`max_y_offset` 和最终 dy——这些值是 curr_buf 坐标系下的，与 ref_buf 的内部行号无关。

**关键理解**：`template_y` 是 curr_buf 坐标系下的"模板底部位置"（`eff_bottom - strip_h`）。ref_buf 的行号 0..strip_h 对应 curr_buf 中 `template_y..template_y+strip_h` 的期望对齐位置。所以 `extract_template(ref_buf, 0, &sample_cols, strip_h)` 是正确的——从 ref_buf 第 0 行提取。

修改 `find_overlap_spatial_ext` 中：
```rust
    // 旧
    let tpl = extract_template(ref_buf, template_y, &sample_cols, strip_h);
    // 新（ref_buf 行号从 0 开始）
    let tpl = extract_template(ref_buf, 0, &sample_cols, strip_h);
```

- [ ] **Step 2: 修改 `estimate_confidence` 和 `sparse_sad_at_offset` 中的 ref_buf 行号引用**

这些函数内部用 `ref_buf.row((template_y as usize) + dy)` 访问 ref_buf。Canvas-Anchored 后 ref_buf 只有 strip_h 行，行号应从 0 开始。

修改 `estimate_confidence` 和 `sparse_sad_at_offset`：把 `ref_buf.row((template_y as usize) + dy)` 改为 `ref_buf.row(dy)`。

但这两个函数需要知道 ref_buf 的行号映射。**最简洁方案**：给这两个函数也传 `template_y_for_ref = 0`，或者直接在调用时把 ref_buf 视为从第 0 行开始。

实际上 `sparse_sad_at_offset` 和 `estimate_confidence` 中 `template_y` 用于定位 ref_buf 的行。改为：
- `sparse_sad_at_offset(ref_buf, curr_buf, sparse_cols, ref_offset, y_offset, strip_h)`，其中 `ref_offset` 是 ref_buf 内部的行号偏移（Canvas-Anchored 时为 0 + dy）

> **简化决策**：由于 ref_buf 的行号 0..strip_h 恰好对应原来的 `template_y..template_y+strip_h`，只需把所有 `ref_buf.row(template_y + dy)` 改为 `ref_buf.row(dy)`。`search_best_offset` 不访问 ref_buf（它用预提取的 tpl），所以不受影响。`estimate_confidence` 和 `sparse_sad_at_offset` 需要改。

具体修改 `sparse_sad_at_offset`：
```rust
fn sparse_sad_at_offset(
    ref_buf: &GrayBuf,
    curr_buf: &GrayBuf,
    sparse_cols: &[usize],
    strip_h: u32,
    y_offset: u32,  // curr_buf 中的 y_offset
) -> f64 {
    let strip_h = strip_h as usize;
    let mut sad: u64 = 0;
    let mut count = 0u64;
    for dy in (0..strip_h).step_by(2) {
        let ref_row = ref_buf.row(dy);  // 旧：ref_buf.row(template_y + dy)；新：ref_buf.row(dy)
        let curr_row = curr_buf.row(y_offset as usize + dy);
        ...
    }
}
```

移除 `template_y` 参数（ref_buf 行号从 0 开始，不需要偏移）。

同步修改 `estimate_confidence` 的调用和内部逻辑。

- [ ] **Step 3: 编译 + 测试**

Run: `cargo test -p octopus-capx 2>&1 | tail -8`
Expected: 16 passed

- [ ] **Step 4: Commit**

```bash
git add crates/capx/src/stitch.rs
git commit -m "refactor(capx): find_overlap_spatial_ext 适配短 ref_buf（画布底部 strip）"
```

---

## Task 4: 改造 `finalize` + `try_match_1d_projection` 为 Canvas-Anchored

**Files:** `crates/capx/src/stitch.rs`

- [ ] **Step 1: 修改 `finalize`——用画布底部替代 `self.reference`**

旧：
```rust
        let last_buf = GrayBuf::from_rgba(last_frame);
        ...
        if let Some((dy, confidence, _)) = find_overlap_spatial_ext(
            &self.reference,
            &last_buf,
            ...
```

新：
```rust
        let last_buf = GrayBuf::from_rgba(last_frame);
        let canvas_ref = self.extract_canvas_bottom_gray(STRIP_H);
        ...
        if let Some((dy, confidence, _)) = find_overlap_spatial_ext(
            &canvas_ref,
            &last_buf,
            ...
```

- [ ] **Step 2: 修改 `try_match_1d_projection`——接收 `ref_buf: &GrayBuf` 参数替代 `&self.reference`**

把内部所有 `&self.reference` 替换为 `ref_buf`。签名新增 `ref_buf: &GrayBuf`。

- [ ] **Step 3: 修改降级链中 `try_match_1d_projection` 的调用——传入 `&canvas_ref`**

`process_frame` 降级链中：
```rust
                if let Some((dy, conf, sad)) = self.try_match_1d_projection(
                    &canvas_ref,  // 新增
                    &curr_buf, x_start, x_end, eff_top, eff_bottom, max_scroll, sad_accept,
                ) {
```

- [ ] **Step 4: 编译 + 测试**

Run: `cargo test -p octopus-capx 2>&1 | tail -8`
Expected: 16 passed

`cargo check -p octopus-desktop` 确认 API 兼容。

- [ ] **Step 5: Commit**

```bash
git add crates/capx/src/stitch.rs
git commit -m "feat(capx): finalize + try_match_1d_projection 改为 Canvas-Anchored"
```

---

## Task 5: 新增 Canvas-Anchored 测试

**Files:** `crates/capx/src/stitch.rs`（测试模块）

- [ ] **Step 1: 新增"连续失败后恢复"测试（核心验证）**

```rust
    #[test]
    fn test_canvas_anchored_recovers_after_failures() {
        // 构造 5 帧序列，中间帧匹配失败（相同帧模拟静止→无追加）
        // 验证后续帧能与画布底部正确对齐，不位移突变
        let f0 = make_frame(TW, TH, 0);
        let mut s = Stitcher::new(f0, StitchConfig::default());
        let f1 = make_frame(TW, TH, 0);
        s.process_frame(&f1).unwrap(); // init

        // 帧 2: 滚动 30px，成功追加
        let f2 = make_frame(TW, TH, 30);
        let added2 = s.process_frame(&f2).unwrap();
        assert!(added2);
        let h_after_2 = s.height();

        // 帧 3: 相同帧（静止），不追加
        let f3 = make_frame(TW, TH, 30);
        s.process_frame(&f3).unwrap();

        // 帧 4: 滚动到 60px，应能与画布底部（~30px 位置）正确对齐
        let f4 = make_frame(TW, TH, 60);
        let added4 = s.process_frame(&f4).unwrap();
        assert!(added4, "Canvas-Anchored 应在中间静止帧后恢复匹配");
        let h_after_4 = s.height();
        assert!(h_after_4 > h_after_2, "恢复后画布应继续增长");
    }
```

- [ ] **Step 2: 新增"画布底部提取正确性"测试**

```rust
    #[test]
    fn test_extract_canvas_bottom_gray() {
        // 构造已知画布内容，验证提取的底部 strip 灰度正确
        let f0 = make_frame(TW, TH, 0);
        let mut s = Stitcher::new(f0.clone(), StitchConfig::default());
        // 初始化（裁掉 sticky 后画布 = 首帧有效区域）
        let f1 = make_frame(TW, TH, 0);
        s.process_frame(&f1).unwrap();

        // 提取底部 strip
        let bottom_gray = s.extract_canvas_bottom_gray(STRIP_H);
        assert_eq!(bottom_gray.width, TW as usize);
        // data 长度 = strip_h × width
        // 底部 strip 对应画布最后 STRIP_H 行
        // 验证：手动从 canvas 计算底部 strip 灰度，与 extract 结果比对
        let canvas = s.canvas();
        let canvas_h = canvas.height();
        let mut expected = Vec::new();
        for y in (canvas_h - STRIP_H)..canvas_h {
            for x in 0..TW {
                let px = canvas.get_pixel(x, y);
                let luma = (2126 * px[0] as u32 + 7152 * px[1] as u32 + 722 * px[2] as u32) / 10000;
                expected.push(luma as u8);
            }
        }
        // 只比对抽样列（estimate_texture_density 用 sample_cols）
        assert_eq!(bottom_gray.data.len(), STRIP_H as usize * TW as usize);
        // 比对前几行确认一致
        for i in 0..TW as usize {
            assert_eq!(bottom_gray.row(0)[i], expected[i], "底部 strip 首行不一致 @ x={}", i);
        }
    }
```

- [ ] **Step 3: 编译 + 运行全部测试**

Run: `cargo test -p octopus-capx 2>&1 | tail -15`
Expected: 16 + 2 = 18 passed

- [ ] **Step 4: Commit**

```bash
git add crates/capx/src/stitch.rs
git commit -m "test(capx): 新增 Canvas-Anchored 恢复 + 画布底部提取正确性测试"
```

---

## Task 6: 文档同步

**Files:** spec + architecture.md

- [ ] **Step 1: 更新 spec 状态为实施完成**

- [ ] **Step 2: 更新 architecture.md stitch 描述——标注 Canvas-Anchored**

- [ ] **Step 3: Commit**

```bash
git add docs/
git commit -m "docs(capx): 同步 Canvas-Anchored 匹配实施记录"
```

---

## 验收清单

- [ ] `cargo test -p octopus-capx` 全绿（≥18 个测试）
- [ ] `cargo check -p octopus-capx -p octopus-desktop` 无错误
- [ ] API 零改动
- [ ] `self.reference` 字段完全移除，无残留引用
- [ ] 文档同步


---

## 来自原文件 `2026-07-02-capx-ncc-sobel.md`

# NCC + Sobel 梯度匹配引擎重写 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development to implement this plan task-by-task.

**Goal:** 用 `imageproc` 库的 NCC + Sobel 梯度替换手写 SAD + 灰度，根治周期性假匹配。

**Architecture:** 保留 Canvas-Anchored 架构。每帧提取画布底部 strip → Sobel 梯度特征图 → `match_template` NCC 匹配 → 多道验证 → 抛物线亚像素插值。移除手写 SAD/纹理密度/动态阈值等调参补丁。

**关联文档:** [spec](../specs/2026-07-02-capx-ncc-sobel-design.md)

---

## 关键约束

1. **API 零改动**：`new/process_frame/finalize/canvas/height` 签名不变
2. **现有 18 测试必须保持全绿**（或调整测试以适应 NCC 特性）
3. **禁止同步到 main**，直到 e2e 实测通过
4. **imageproc 0.25 API 已确认**：`match_template`、`find_extremes`、`sobel_gradients` 均可用
5. **worktree**: `/Users/wudarui/workspace/agent/octopus/.worktrees/optimize-capx`

### imageproc API 关键细节

- `match_template(image: &GrayImage, template: &GrayImage, method)` → `Image<Luma<f32>>`
  - `image` = 搜索区域（大），`template` = 模板（小）
  - response 尺寸 = `(image.w - template.w + 1, image.h - template.h + 1)`
  - 模板和搜索区域宽度相同时 response 只有 1 列
- `MatchTemplateMethod::CrossCorrelationNormalized` = NCC，越大越好（1.0 完美）
- `find_extremes(&Image<Luma<T>>)` → `Extremes { max_value_location, min_value_location, ... }`
- `sobel_gradients(&GrayImage)` → `Image<Luma<u16>>`（注意是 u16 不是 u8）

---

## Task 1: GrayBuf 增强 + Sobel 特征图生成

**Files:** `crates/capx/src/stitch.rs`

- [ ] **Step 1: 在 `GrayBuf` impl 中新增 `to_gray_image` 方法**

```rust
impl GrayBuf {
    // ... 现有方法 ...

    /// 转为 image::GrayImage（供 imageproc 使用）
    fn to_gray_image(&self) -> image::GrayImage {
        let h = (self.data.len() / self.width) as u32;
        image::GrayImage::from_raw(self.width as u32, h, self.data.clone())
            .expect("GrayBuf → GrayImage 失败")
    }
}
```

- [ ] **Step 2: 在自由函数区新增 `to_feature_map`（Sobel + 归一化 + 纯色退化）**

```rust
use imageproc::gradients::sobel_gradients;

/// 将 GrayBuf 转为 Sobel 梯度特征图 + 归一化。
/// 纯色区域（max_gradient=0）返回 (空白, false)，调用方退回灰度。
fn to_feature_map(gray: &GrayBuf) -> (image::GrayImage, bool) {
    let luma_img = gray.to_gray_image();
    let gradients = sobel_gradients(&luma_img);

    let max_gradient = gradients.iter().map(|p| p[0]).max().unwrap_or(0);
    if max_gradient == 0 {
        return (image::GrayImage::new(luma_img.width(), luma_img.height()), false);
    }

    // 归一化：mean + 3σ
    let (mean, stddev) = mean_stddev(&gradients);
    let normalizer = (mean + 3.0 * stddev).max(1.0);

    let normalized = image::GrayImage::from_fn(gradients.width(), gradients.height(), |x, y| {
        let g = gradients.get_pixel(x, y)[0] as f32;
        let scaled = (g / normalizer) * 255.0;
        image::Luma([scaled.round().clamp(0.0, 255.0) as u8])
    });
    (normalized, true)
}

/// 计算灰度图的均值和标准差。
fn mean_stddev(img: &imageproc::definitions::Image<image::Luma<u16>>) -> (f32, f32) {
    let n = (img.width() * img.height()) as f32;
    let sum: f32 = img.iter().map(|p| p[0] as f32).sum();
    let mean = sum / n;
    let var: f32 = img.iter().map(|p| {
        let d = p[0] as f32 - mean;
        d * d
    }).sum::<f32>() / n;
    (mean, var.sqrt())
}
```

- [ ] **Step 3: 新增常量**

在常量块中追加：

```rust
// ===== NCC 匹配参数 =====
/// 最低 NCC 分数阈值
const NCC_SCORE_THRESHOLD: f32 = 0.75;
/// 局部置信度 delta：best vs 次优差值
const LOCAL_CONFIDENCE_DELTA: f32 = 0.005;
/// 全局置信度 delta：best vs 距离≥4px 的差值
const GLOBAL_CONFIDENCE_DELTA: f32 = 0.002;
/// 全局置信度最小距离（像素）
const GLOBAL_CONFIDENCE_MIN_DIST: usize = 4;
```

- [ ] **Step 4: 编译验证**

Run: `cargo check -p octopus-capx 2>&1 | tail -5`
Expected: `Finished`（可能有 unused warning，后续 task 消费）

- [ ] **Step 5: 测试验证**

Run: `cargo test -p octopus-capx 2>&1 | grep "test result"`
Expected: 18 passed

- [ ] **Step 6: Commit**

```bash
git add crates/capx/src/stitch.rs
git commit -m "feat(capx): GrayBuf::to_gray_image + Sobel 特征图生成 + NCC 常量"
```

---

## Task 2: NCC 匹配 + 多道验证

**Files:** `crates/capx/src/stitch.rs`

- [ ] **Step 1: 新增 `ncc_match` 自由函数**

```rust
use imageproc::template_matching::{match_template, find_extremes, MatchTemplateMethod};
use imageproc::definitions::Image;
use image::Luma;

/// NCC 匹配结果。
struct NccResult {
    best_y: f64,        // 最佳偏移（response 坐标）
    best_score: f64,    // NCC 分数 [0, 1]
    response: Image<Luma<f32>>,  // 完整 response map
}

/// NCC 匹配：在搜索区域中找模板的最佳对齐位置。
fn ncc_match(
    template: &image::GrayImage,
    search_region: &image::GrayImage,
) -> Option<NccResult> {
    // 模板必须严格小于搜索区域
    if template.width() > search_region.width() || template.height() >= search_region.height() {
        return None;
    }
    let response = match_template(
        search_region,
        template,
        MatchTemplateMethod::CrossCorrelationNormalized,
    );
    let extremes = find_extremes(&response);
    let best_y = extremes.max_value_location.1 as f64;
    let best_score = extremes.max_value as f64;
    Some(NccResult { best_y, best_score, response })
}
```

- [ ] **Step 2: 新增 `validate_ncc_match` 多道验证函数**

```rust
/// 多道验证 NCC 匹配结果。
/// 返回 true 表示匹配可信。
fn validate_ncc_match(response: &Image<Luma<f32>>, best_y: usize, best_score: f32) -> bool {
    // 1. 最低分数
    if best_score < NCC_SCORE_THRESHOLD {
        return false;
    }

    let h = response.height() as usize;

    // 2. 局部置信度：best vs best±1 的最大值差
    let local_alt = {
        let mut alt = 0.0f32;
        if best_y > 0 {
            alt = alt.max(response.get_pixel(0, best_y as u32 - 1)[0]);
        }
        if best_y + 1 < h {
            alt = alt.max(response.get_pixel(0, best_y as u32 + 1)[0]);
        }
        alt
    };
    if best_score - local_alt < LOCAL_CONFIDENCE_DELTA {
        return false;
    }

    // 3. 全局置信度：best vs 距离≥GLOBAL_CONFIDENCE_MIN_DIST 的最大值差
    let distant_alt = {
        let mut alt = 0.0f32;
        for y in 0..h {
            if (y as isize - best_y as isize).unsigned_abs() >= GLOBAL_CONFIDENCE_MIN_DIST as isize {
                alt = alt.max(response.get_pixel(0, y as u32)[0]);
            }
        }
        alt
    };
    if best_score - distant_alt < GLOBAL_CONFIDENCE_DELTA {
        return false;
    }

    true
}
```

- [ ] **Step 3: 新增 `parabolic_refine_from_response` 亚像素插值**

```rust
/// 从 NCC response map 在最佳 y 处做抛物线拟合，返回亚像素偏移。
fn parabolic_refine_from_response(response: &Image<Luma<f32>>, best_y: f64) -> f64 {
    let by = best_y as usize;
    if by == 0 || by + 1 >= response.height() as usize {
        return best_y;
    }
    let left = response.get_pixel(0, by as u32 - 1)[0] as f64;
    let center = response.get_pixel(0, by as u32)[0] as f64;
    let right = response.get_pixel(0, by as u32 + 1)[0] as f64;
    let denom = left - 2.0 * center + right;
    if denom.abs() > 1e-10 {
        let delta = 0.5 * (left - right) / denom;
        best_y + delta.clamp(-0.5, 0.5)
    } else {
        best_y
    }
}
```

- [ ] **Step 4: 编译 + 测试**

Run: `cargo test -p octopus-capx 2>&1 | grep "test result"`
Expected: 18 passed（新函数未调用，unused warning 预期）

- [ ] **Step 5: Commit**

```bash
git add crates/capx/src/stitch.rs
git commit -m "feat(capx): NCC 匹配 + 多道验证 + 亚像素插值（imageproc 库）"
```

---

## Task 3: process_frame 接入 NCC 匹配

**Files:** `crates/capx/src/stitch.rs`

> 这是核心改造——替换 `process_frame` 中的主匹配从 SAD 到 NCC。

- [ ] **Step 1: 修改 `process_frame` 主匹配分支**

找到当前的匹配代码段（`let curr_buf = GrayBuf::from_rgba_roi(...)` 到 `let (dy, confidence, best_sad) = match find_overlap_spatial_ext(...)`），替换为 NCC 流程：

```rust
        // ROI 灰度转换：覆盖最大可能搜索范围
        let roi_top = eff_top.max(eff_bottom.saturating_sub(STRIP_H + MAX_SCROLL * 2)) as usize;
        let roi_bottom = eff_bottom as usize;
        let curr_gray = GrayBuf::from_rgba_roi(frame, roi_top, roi_bottom);
        let canvas_gray = self.extract_canvas_bottom_gray(STRIP_H);

        // Sobel 特征图 + 纯色退化
        let (canvas_feat, canvas_has_feat) = to_feature_map(&canvas_gray);
        let (curr_feat, curr_has_feat) = to_feature_map(&curr_gray);
        let (template, search_region) = if canvas_has_feat && curr_has_feat {
            (canvas_feat, curr_feat)
        } else {
            (canvas_gray.to_gray_image(), curr_gray.to_gray_image())
        };

        // NCC 匹配
        let ncc = match ncc_match(&template, &search_region) {
            Some(r) => r,
            None => {
                // 尺寸不合法，进入降级链
                log::info!("[stitch] ncc_match returned None (size mismatch)");
                return self.try_fallback(frame, &curr_gray, w, eff_top, eff_bottom);
            }
        };

        // 多道验证
        if !validate_ncc_match(&ncc.response, ncc.best_y as usize, ncc.best_score as f32) {
            log::info!("[stitch] NCC match failed validation (score={:.4})", ncc.best_score);
            return self.try_fallback(frame, &curr_gray, w, eff_top, eff_bottom);
        }

        // 亚像素插值
        let refined_y = parabolic_refine_from_response(&ncc.response, ncc.best_y);

        // 计算 dy：template 顶部在 curr 坐标系中的位置 - roi_top
        // response 的 y=0 对应 search_region 顶部 = roi_top
        let dy = -(refined_y + STRIP_H as f64); // 负号：用户向下滚动
```

> **注意 dy 计算**：NCC response 的 y 坐标表示模板在搜索区域中的对齐位置。response y=0 对应搜索区域顶部（roi_top）。模板高度 = STRIP_H。如果模板在搜索区域 y=dy_offset 处匹配，则意味着当前帧的 `[roi_top + dy_offset, roi_top + dy_offset + STRIP_H)` 行与画布底部一致 → 新增内容 = `[roi_top + dy_offset + STRIP_H, eff_bottom)` → new_rows = eff_bottom - (roi_top + dy_offset + STRIP_H)。但我们的 dy 约定是位移量（负值=向下滚），所以 dy = -(new_rows)。需要仔细推导坐标关系。

**坐标推导**（关键）：
- 画布底部 strip = canvas 最后 STRIP_H 行，在 canvas 坐标系中是 `[canvas_h - STRIP_H, canvas_h)`
- 当前帧 ROI = `[roi_top, eff_bottom)`
- NCC 搜索：模板（canvas strip）在搜索区域（curr ROI）中滑动
- response y = 模板顶部在 curr ROI 中的偏移量
- response y = 0：模板对齐 curr ROI 顶部 → canvas 底部 = curr ROI 顶部 → 无新内容
- response y = eff_bottom - roi_top - STRIP_H：模板对齐 curr ROI 底部 → 全是新内容
- **new_rows = (eff_bottom - roi_top) - response_y - STRIP_H**
- **dy = -(new_rows)**（负值=向下滚动）

```rust
        let roi_height = (eff_bottom - roi_top as u32) as f64;
        let new_rows_raw = roi_height - refined_y - STRIP_H as f64;
        let dy = -new_rows_raw;
```

- [ ] **Step 2: 移除 `find_overlap_spatial_ext` 调用及相关变量**

移除 `find_overlap_spatial_ext`、`decide_match`、`estimate_confidence`、`search_best_offset`、`extract_template`、`sparse_sad_at_offset` 的调用。但**先不删除函数定义**（Task 5 清理），只是不再调用。

- [ ] **Step 3: 调整后续检查逻辑**

主匹配成功后，dy 方向 + 幅度检查保留（`dy >= 0.0` 跳过、`new_rows` 范围检查），但：
- **移除 `is_stationary()` 双重校验**——NCC 在静止帧上会返回 score≈1.0 且 y 对齐正确（dy≈0），自然被处理
- **保留 dy_history 更新**

```rust
        // dy 方向检查
        if dy >= 0.0 {
            log::info!("[stitch] skipped frame: dy={:.1} >= 0.0 (ncc={:.4})", dy, ncc.best_score);
            self.dy_history.push_back(dy);
            if self.dy_history.len() > DY_HISTORY_LEN { self.dy_history.pop_front(); }
            return Ok(false);
        }

        let new_rows = (-dy).round() as u32;
        let max_scroll_limit = (eff_bottom - eff_top) * 4 / 5;

        if new_rows < self.config.min_scroll_px as u32 || new_rows >= max_scroll_limit {
            log::info!("[stitch] skipped frame: new_rows={} invalid (min={}, max={}) (ncc={:.4})",
                new_rows, self.config.min_scroll_px, max_scroll_limit, ncc.best_score);
            self.dy_history.push_back(dy);
            if self.dy_history.len() > DY_HISTORY_LEN { self.dy_history.pop_front(); }
            return Ok(false);
        }

        log::info!("[stitch] ncc={:.4} dy={:.1} new_rows={} canvas_h={}",
            ncc.best_score, dy, new_rows, self.canvas_h);

        // 主匹配成功：重置 best-guess 计数
        self.best_guess_streak = 0;

        // 画布追加 + 状态更新（不变）
        ...
```

- [ ] **Step 4: 抽取 `try_fallback` 方法**

把当前降级链（降级 1/2/3 + best-guess）封装为方法：

```rust
    fn try_fallback(
        &mut self,
        frame: &RgbaImage,
        curr_gray: &GrayBuf,
        w: u32,
        eff_top: u32,
        eff_bottom: u32,
    ) -> Result<bool> {
        // 保留 1D 投影降级 + best-guess
        // 移除降级 1（扩大搜索范围）和降级 2（缩小模板）——NCC 已覆盖

        // 降级：1D 灰度投影匹配
        let canvas_ref = self.extract_canvas_bottom_gray(STRIP_H);
        let x_start = (w as f64 * X_START_RATIO) as u32;
        let x_end = (w as f64 * X_END_RATIO) as u32;
        if let Some((dy, conf, sad)) = self.try_match_1d_projection(
            &canvas_ref, curr_gray, x_start, x_end, eff_top, eff_bottom, MAX_SCROLL, 0.0,
        ) {
            log::info!("[stitch] fallback: 1D projection match, dy={:.1} conf={:.4}", dy, conf);
            self.best_guess_streak = 0;
            return self.apply_fallback_match(dy, conf, sad, frame, curr_gray, w, eff_top, eff_bottom);
        }

        // 静止检测 + best-guess
        let sample_cols: Vec<usize> = (x_start as usize..x_end as usize)
            .step_by(SAMPLE_STEP_X)
            .collect();
        let stationary_sad = self.quick_stationary_check(curr_gray, &canvas_ref, &sample_cols);
        if stationary_sad < STATIONARY_SAD {
            log::info!("[stitch] stationary detected before best-guess (sad={:.2})", stationary_sad);
            self.dy_history.clear();
            self.best_guess_streak = 0;
            self.last_dy = None;
            return Ok(false);
        }

        if self.best_guess_streak < 3 {
            if let Some(dy) = self.estimate_dy_hint() {
                log::info!("[stitch] best-guess dy={:.1} (streak={})", dy, self.best_guess_streak + 1);
                self.best_guess_streak += 1;
                return self.apply_fallback_match(dy, 0.0, 0.0, frame, curr_gray, w, eff_top, eff_bottom);
            }
        } else {
            log::info!("[stitch] best-guess circuit breaker tripped");
        }

        log::info!("[stitch] all fallbacks exhausted, skipping frame");
        self.last_dy = None;
        Ok(false)
    }
```

- [ ] **Step 5: 编译 + 测试**

Run: `cargo test -p octopus-capx 2>&1 | tail -15`
Expected: 可能有些测试失败——NCC 在合成图上的行为可能不同。

**如果测试失败**：
- `test_known_scroll_appends_rows`：NCC 在渐变+条纹合成图上应能匹配。检查 dy 计算是否正确（坐标关系）。
- `test_stationary_frame_returns_false`：静止帧 NCC 应返回高 score 但 dy≈0，被 `dy >= 0.0` 跳过。
- 如果合成图缺乏 Sobel 特征（纯渐变无边缘），`to_feature_map` 会退化到灰度——这应该仍然工作。

- [ ] **Step 6: Commit**

```bash
git add crates/capx/src/stitch.rs
git commit -m "feat(capx): process_frame 接入 NCC + Sobel 匹配（替换 SAD）"
```

---

## Task 4: finalize 接入 NCC 匹配

**Files:** `crates/capx/src/stitch.rs`

- [ ] **Step 1: 修改 `finalize` 中的匹配为 NCC**

```rust
        // ROI 灰度转换
        let roi_top = eff_top as usize;
        let last_gray = GrayBuf::from_rgba_roi(last_frame, roi_top, eff_bottom as usize);
        let canvas_gray = self.extract_canvas_bottom_gray(STRIP_H);

        // Sobel 特征图 + NCC 匹配
        let (canvas_feat, canvas_has_feat) = to_feature_map(&canvas_gray);
        let (last_feat, last_has_feat) = to_feature_map(&last_gray);
        let (template, search_region) = if canvas_has_feat && last_has_feat {
            (canvas_feat, last_feat)
        } else {
            (canvas_gray.to_gray_image(), last_gray.to_gray_image())
        };

        if let Some(ncc) = ncc_match(&template, &search_region) {
            if validate_ncc_match(&ncc.response, ncc.best_y as usize, ncc.best_score as f32) {
                let refined_y = parabolic_refine_from_response(&ncc.response, ncc.best_y);
                let roi_height = (eff_bottom - eff_top) as f64;
                let new_rows_raw = roi_height - refined_y - STRIP_H as f64;
                let dy = -new_rows_raw;

                if dy < 0.0 {
                    let new_rows = (-dy).round() as u32;
                    if new_rows < eff_bottom - eff_top {
                        log::info!("[stitch] finalize: stitching remaining {} rows (ncc={:.4})", new_rows, ncc.best_score);
                        let crop_y = eff_bottom - new_rows;
                        let row_bytes = w as usize * 4;
                        let start = crop_y as usize * row_bytes;
                        let end = start + new_rows as usize * row_bytes;
                        let frame_raw = last_frame.as_raw();
                        self.canvas_buf.extend_from_slice(&frame_raw[start..end]);
                        self.canvas_h += new_rows;
                        self.invalidate_cache();
                    }
                }
            }
        }
```

- [ ] **Step 2: 编译 + 测试**

Run: `cargo test -p octopus-capx 2>&1 | grep "test result"`

- [ ] **Step 3: Commit**

```bash
git add crates/capx/src/stitch.rs
git commit -m "feat(capx): finalize 接入 NCC 匹配"
```

---

## Task 5: 清理废弃代码

**Files:** `crates/capx/src/stitch.rs`

- [ ] **Step 1: 删除不再调用的函数**

- `find_overlap_spatial_ext`（旧 SAD 主搜索）
- `decide_match`
- `search_best_offset`（整数 SAD 搜索）
- `extract_template`
- `estimate_confidence`
- `sparse_sad_at_offset`
- `estimate_texture_density`（Sobel 替代）
- `dynamic_sad_accept`（NCC 固定阈值）

- [ ] **Step 2: 删除不再使用的常量**

- `SAD_ACCEPT`
- `MIN_CONFIDENCE`（被 NCC_SCORE_THRESHOLD 替代）
- `SPEED_PENALTY`
- `TEXTURE_EDGE_THRESHOLD`
- `TEXTURE_BONUS_FACTOR`
- `SAD_BASELINE_MULTIPLIER`
- `SAD_BASELINE_PADDING`
- `SAD_BASELINE_ALPHA`
- `FALLBACK_STRIP_H`
- `FALLBACK_SAD_MULTIPLIER`
- `sad_baseline` 字段（不再需要 EMA 基线）

> **注意**：`STATIONARY_SAD`、`STATIONARY_DY_THRESHOLD`、`DY_HISTORY_LEN`、`MAX_SCROLL`、`STRIP_H`、`SAMPLE_STEP_X`、`X_START_RATIO`、`X_END_RATIO`、`STICKY_DETECT_MAX` 保留（仍在使用）。

- [ ] **Step 3: 编译 + 测试 + 零 warning**

Run: `cargo test -p octopus-capx 2>&1 | grep -E "test result|warning"`
Expected: 18+ passed，0 warning

- [ ] **Step 4: Commit**

```bash
git add crates/capx/src/stitch.rs
git commit -m "refactor(capx): 清理 SAD 废弃代码（search_best_offset/decide_match/estimate_confidence 等）"
```

---

## Task 6: 新增 NCC 特性测试

**Files:** `crates/capx/src/stitch.rs`

- [ ] **Step 1: 新增 Sobel 特征图测试**

```rust
    #[test]
    fn test_sobel_pure_color_degrades() {
        // 纯色帧：Sobel 无梯度 → 返回 (blank, false)
        let f = make_frame_textured(TW, TH, 0, 0); // texture_level=0 纯色
        let gray = GrayBuf::from_rgba_roi(&f, 0, TH as usize);
        let (feat, has_feat) = to_feature_map(&gray);
        assert!(!has_feat, "纯色帧应无 Sobel 特征");
    }

    #[test]
    fn test_sobel_textured_has_features() {
        // 密集条纹帧：Sobel 有梯度 → 返回 (有内容, true)
        let f = make_frame_textured(TW, TH, 0, 2); // texture_level=2 密集
        let gray = GrayBuf::from_rgba_roi(&f, 0, TH as usize);
        let (feat, has_feat) = to_feature_map(&gray);
        assert!(has_feat, "密集条纹帧应有 Sobel 特征");
    }
```

- [ ] **Step 2: 新增 NCC 匹配精度测试**

```rust
    #[test]
    fn test_ncc_matches_known_offset() {
        // 构造已知位移帧，验证 NCC 返回正确偏移
        let f0 = make_frame(TW, TH, 0);
        let f1 = make_frame(TW, TH, 30); // 滚动 30px
        let gray0 = GrayBuf::from_rgba_roi(&f0, 0, TH as usize);
        let gray1 = GrayBuf::from_rgba_roi(&f1, 0, TH as usize);
        let template_gray = gray0.to_gray_image(); // 整帧作为模板太大，用底部 strip
        // 提取 f0 底部 80 行作为模板
        let canvas_strip = GrayBuf::from_rgba_roi(&f0, (TH - STRIP_H) as usize, TH as usize);
        let template = canvas_strip.to_gray_image();
        let search_region = gray1.to_gray_image();
        let result = ncc_match(&template, &search_region);
        assert!(result.is_some(), "NCC 应返回匹配结果");
        let ncc = result.unwrap();
        assert!(ncc.best_score > 0.75, "NCC 分数应 > 0.75: {}", ncc.best_score);
    }
```

- [ ] **Step 3: 编译 + 运行全部测试**

Run: `cargo test -p octopus-capx 2>&1 | tail -15`
Expected: 20+ passed

- [ ] **Step 4: Commit**

```bash
git add crates/capx/src/stitch.rs
git commit -m "test(capx): 新增 Sobel 特征图 + NCC 匹配精度测试"
```

---

## Task 7: desktop 集成编译 + 文档同步

**Files:** `docs/architecture.md`

- [ ] **Step 1: desktop 编译验证**

Run: `cargo check -p octopus-desktop 2>&1 | grep -E "error|Finished"`

- [ ] **Step 2: 更新 architecture.md**

stitch 描述更新为 NCC + Sobel 版本。

- [ ] **Step 3: Commit**

```bash
git add docs/architecture.md
git commit -m "docs(capx): 同步 NCC + Sobel 匹配引擎到 architecture"
```

---

## 验收清单（e2e 实测前）

- [ ] `cargo test -p octopus-capx` 全绿（≥20）
- [ ] `cargo check -p octopus-desktop` 无错误
- [ ] API 零改动
- [ ] 0 warning
- [ ] 废弃 SAD 代码全部清理


---

## 来自原文件 `2026-07-02-capx-stitch-robustness.md`

# 滚动拼接健壮性优化 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 通过时序平滑、动态自适应阈值、三级兜底降级链提升滚动截屏拼接健壮性，解决错位/丢内容/容易断，API 零改动。

**Architecture:** Stitcher 新增 `dy_history: VecDeque<f64>` 和 `sad_baseline: f64` 两个字段。`process_frame` 重构为主匹配 + 三级降级链（扩大范围→缩小模板→1D 投影）。`find_overlap_spatial_ext` 参数化 `sad_accept` 和 `strip_h`。`decide_match` 移除 `stationary < best + 1.0` 硬覆盖，静止判断改为 dy 时序双重校验上移到 `process_frame`。

**Tech Stack:** Rust 2021、image 0.25、std::collections::VecDeque。

**关联文档:** [spec](../specs/2026-07-02-capx-stitch-robustness-design.md)

---

## 关键约束（所有任务必须遵守）

1. **API 零改动**：`Stitcher::new/process_frame/finalize/canvas/height` 与 `capture::*` 签名不变。`desktop` 零改动。
2. **灰度公式不变**：`GrayBuf::from_rgba` 保持 `(2126*R + 7152*G + 722*B) / 10000`。
3. **dy 符号约定**：`dy < 0` = 用户向下滚动（内容上移）。
4. **现有 12 测试必须保持全绿**：每次改造后 `cargo test -p octopus-capx` 必须通过。
5. **worktree 路径**：所有命令在 `/Users/wudarui/workspace/agent/octopus/.worktrees/optimize-capx` 下执行。

---

## 文件结构

| 文件 | 职责 | 本次改动 |
|------|------|---------|
| `crates/capx/src/stitch.rs` | 滚动拼接 | 新增字段、常量、降级链、1D 投影、测试用例 |

---

## Task 1: 新增常量 + Stitcher 字段

**Files:**
- Modify: `crates/capx/src/stitch.rs`（常量块 + struct + new）

- [ ] **Step 1: 在常量块末尾（`STICKY_DETECT_MAX` 之后）追加新常量**

```rust
/// 时序平滑：静止判断的 dy 均值阈值（近 N 帧 |dy| 均值 < 此值 → 静止）
const STATIONARY_DY_THRESHOLD: f64 = 2.0;
/// dy 历史长度
const DY_HISTORY_LEN: usize = 8;
/// 纹理密度评估：水平梯度阈值
const TEXTURE_EDGE_THRESHOLD: i32 = 20;
/// 动态阈值：纹理密度奖励系数（texture ∈ [0,1] × 30 → 最多加 30）
const TEXTURE_BONUS_FACTOR: f64 = 30.0;
/// 动态阈值：历史基线倍数（sad_baseline × 1.5 + 5）
const SAD_BASELINE_MULTIPLIER: f64 = 1.5;
/// 动态阈值：历史基线 padding
const SAD_BASELINE_PADDING: f64 = 5.0;
/// 动态阈值：EMA 平滑系数
const SAD_BASELINE_ALPHA: f64 = 0.3;
/// 降级 2：缩小模板高度
const FALLBACK_STRIP_H: u32 = 40;
/// 降级 2：阈值放宽倍数
const FALLBACK_SAD_MULTIPLIER: f64 = 1.5;
```

- [ ] **Step 2: 在文件顶部 `use` 语句中添加 `std::collections::VecDeque`**

旧：
```rust
use anyhow::Result;
use image::RgbaImage;
```

新：
```rust
use anyhow::Result;
use image::RgbaImage;
use std::collections::VecDeque;
```

- [ ] **Step 3: Stitcher struct 新增两个字段**

旧：
```rust
pub struct Stitcher {
    canvas_w: u32,
    canvas_h: u32,
    canvas_buf: Vec<u8>,
    canvas_cache: std::cell::UnsafeCell<Option<RgbaImage>>,
    reference: GrayBuf,
    sticky_top: u32,
    sticky_bottom: u32,
    detected: bool,
    config: StitchConfig,
    last_dy: Option<f64>,
}
```

新：
```rust
pub struct Stitcher {
    canvas_w: u32,
    canvas_h: u32,
    canvas_buf: Vec<u8>,
    canvas_cache: std::cell::UnsafeCell<Option<RgbaImage>>,
    reference: GrayBuf,
    sticky_top: u32,
    sticky_bottom: u32,
    detected: bool,
    config: StitchConfig,
    last_dy: Option<f64>,
    /// 最近若干帧的 dy 历史，用于时序平滑判断静止。
    dy_history: VecDeque<f64>,
    /// 历史成功匹配的 SAD 均值（EMA）。
    sad_baseline: f64,
}
```

- [ ] **Step 4: `new()` 初始化新字段**

旧：
```rust
    pub fn new(first_frame: RgbaImage, config: StitchConfig) -> Self {
        let w = first_frame.width();
        let h = first_frame.height();
        Self {
            canvas_w: w,
            canvas_h: h,
            canvas_buf: first_frame.into_raw(),
            canvas_cache: std::cell::UnsafeCell::new(None),
            reference: GrayBuf { data: Vec::new(), width: 0 },
            sticky_top: 0,
            sticky_bottom: 0,
            detected: false,
            config,
            last_dy: None,
        }
    }
```

新：
```rust
    pub fn new(first_frame: RgbaImage, config: StitchConfig) -> Self {
        let w = first_frame.width();
        let h = first_frame.height();
        Self {
            canvas_w: w,
            canvas_h: h,
            canvas_buf: first_frame.into_raw(),
            canvas_cache: std::cell::UnsafeCell::new(None),
            reference: GrayBuf { data: Vec::new(), width: 0 },
            sticky_top: 0,
            sticky_bottom: 0,
            detected: false,
            config,
            last_dy: None,
            dy_history: VecDeque::with_capacity(DY_HISTORY_LEN),
            sad_baseline: 0.0,
        }
    }
```

- [ ] **Step 5: 编译验证（新字段未使用会 warning，预期）**

Run:
```bash
cd /Users/wudarui/workspace/agent/octopus/.worktrees/optimize-capx && cargo check -p octopus-capx 2>&1 | tail -5
```
Expected: `Finished`，可能有 `unused` warning（后续 task 消费）。

- [ ] **Step 6: Commit**

```bash
cd /Users/wudarui/workspace/agent/octopus/.worktrees/optimize-capx
git add crates/capx/src/stitch.rs
git commit -m "feat(capx): 新增健壮性优化常量与 Stitcher 字段（dy_history/sad_baseline）"
```

---

## Task 2: 纹理密度评估 + 动态阈值

**Files:**
- Modify: `crates/capx/src/stitch.rs`（新增 `estimate_texture_density`、`dynamic_sad_accept`）

- [ ] **Step 1: 在 `GrayBuf` impl 块之后、`pub struct StitchConfig` 之前新增 `estimate_texture_density` 自由函数**

```rust
/// 评估模板条区域的纹理密度（边缘像素占比）。
/// 复用 sample_cols 的相邻列对做水平差分，O(STRIP_H × n_cols)，开销极低。
fn estimate_texture_density(buf: &GrayBuf, sample_cols: &[usize], template_y: u32) -> f64 {
    let mut edge_count = 0u32;
    let mut total = 0u32;
    for dy in 0..STRIP_H {
        let row = buf.row((template_y + dy) as usize);
        for w in sample_cols.windows(2) {
            total += 1;
            if (row[w[0]] as i32 - row[w[1]] as i32).abs() > TEXTURE_EDGE_THRESHOLD {
                edge_count += 1;
            }
        }
    }
    if total == 0 { return 0.0; }
    edge_count as f64 / total as f64
}
```

- [ ] **Step 2: 在 `impl Stitcher` 块内（`invalidate_cache` 之后、`canvas` 之前）新增 `dynamic_sad_accept` 方法**

```rust
    /// 根据当前帧纹理密度 + 历史 SAD 基线动态计算 SAD 接受阈值。
    fn dynamic_sad_accept(&self, texture: f64) -> f64 {
        // 纹理越丰富 → 绝对 SAD 天然更高 → 允许更高阈值
        let texture_bonus = texture * TEXTURE_BONUS_FACTOR;
        // 历史基线浮动：EMA 均值的倍数 + padding 作为上界
        let baseline_cap = self.sad_baseline * SAD_BASELINE_MULTIPLIER + SAD_BASELINE_PADDING;
        (SAD_ACCEPT + texture_bonus).min(baseline_cap).max(SAD_ACCEPT)
    }
```

- [ ] **Step 3: 编译验证**

Run:
```bash
cd /Users/wudarui/workspace/agent/octopus/.worktrees/optimize-capx && cargo check -p octopus-capx 2>&1 | tail -5
```
Expected: `Finished`，`estimate_texture_density` 和 `dynamic_sad_accept` 可能有 unused warning（后续 task 消费）。

- [ ] **Step 4: 现有测试验证无回归**

Run:
```bash
cd /Users/wudarui/workspace/agent/octopus/.worktrees/optimize-capx && cargo test -p octopus-capx 2>&1 | grep "test result"
```
Expected: 12 passed。

- [ ] **Step 5: Commit**

```bash
cd /Users/wudarui/workspace/agent/octopus/.worktrees/optimize-capx
git add crates/capx/src/stitch.rs
git commit -m "feat(capx): 新增纹理密度评估与动态 SAD 阈值计算"
```

---

## Task 3: 时序平滑静止判断

**Files:**
- Modify: `crates/capx/src/stitch.rs`（新增 `is_stationary` + 修改 `decide_match`）

- [ ] **Step 1: 在 `impl Stitcher` 块内（`dynamic_sad_accept` 之后）新增 `is_stationary` 方法**

```rust
    /// 判断当前是否为静止状态（基于历史 dy 均值）。
    /// 回弹帧 dy 可能抖动到 -3，但历史 [-15,-12,-10,-3] 均值 -10，不判静止。
    fn is_stationary(&self) -> bool {
        if self.dy_history.len() < 3 {
            return false; // 不足 3 帧，不判静止（让 SAD 主匹配决定）
        }
        let n = self.dy_history.len().min(5);
        let recent: f64 = self.dy_history.iter().rev().take(n).sum::<f64>() / n as f64;
        recent.abs() < STATIONARY_DY_THRESHOLD
    }
```

- [ ] **Step 2: 修改 `decide_match` 签名和逻辑——移除 `stationary < best + 1.0` 硬覆盖，新增 `sad_accept` 参数**

旧：
```rust
fn decide_match(
    best_y_offset: u32,
    best_sad_avg: f64,
    stationary_sad_avg: f64,
    confidence: f64,
    template_y: u32,
) -> Option<(f64, f64)> {
    if stationary_sad_avg < STATIONARY_SAD || stationary_sad_avg < best_sad_avg + 1.0 {
        return Some((0.0, 1.0));
    }
    if best_sad_avg < SAD_ACCEPT && confidence > MIN_CONFIDENCE {
        let dy = best_y_offset as f64 - template_y as f64;
        Some((dy, confidence))
    } else {
        None
    }
}
```

新：
```rust
fn decide_match(
    best_y_offset: u32,
    best_sad_avg: f64,
    stationary_sad_avg: f64,
    confidence: f64,
    template_y: u32,
    sad_accept: f64,
) -> Option<(f64, f64)> {
    // 保留绝对静止快速路径（画面完全没动时 stationary_sad 极低）
    if stationary_sad_avg < STATIONARY_SAD {
        return Some((0.0, 1.0));
    }
    // 移除 stationary < best + 1.0 硬覆盖——交由 is_stationary() 时序判断
    if best_sad_avg < sad_accept && confidence > MIN_CONFIDENCE {
        let dy = best_y_offset as f64 - template_y as f64;
        Some((dy, confidence))
    } else {
        None
    }
}
```

- [ ] **Step 3: 修改 `find_overlap_spatial_ext` 签名——新增 `sad_accept` 和 `strip_h` 参数**

旧签名：
```rust
fn find_overlap_spatial_ext(
    ref_buf: &GrayBuf,
    curr_buf: &GrayBuf,
    x_start: u32,
    x_end: u32,
    eff_top: u32,
    eff_bottom: u32,
    max_scroll: u32,
    last_dy: Option<f64>,
) -> Option<(f64, f64)> {
```

新签名：
```rust
fn find_overlap_spatial_ext(
    ref_buf: &GrayBuf,
    curr_buf: &GrayBuf,
    x_start: u32,
    x_end: u32,
    eff_top: u32,
    eff_bottom: u32,
    max_scroll: u32,
    last_dy: Option<f64>,
    sad_accept: f64,
    strip_h: u32,
) -> Option<(f64, f64)> {
```

- [ ] **Step 4: 修改 `find_overlap_spatial_ext` 函数体——用 `strip_h` 替换 `STRIP_H`，用 `sad_accept` 传给 `decide_match`**

在函数体内，所有使用 `STRIP_H` 的地方改为参数 `strip_h`。具体替换点：
- `if eff_bottom <= eff_top + strip_h + 10`（原来是 `STRIP_H + 10`）
- `let template_y = eff_bottom - strip_h;`（原来是 `STRIP_H`）
- `extract_template(ref_buf, template_y, &sample_cols)` 内部仍用 `STRIP_H`——改为传 `strip_h` 参数（见 Step 5）

`decide_match` 调用处改为传入 `sad_accept`：
```rust
    // 旧
    let confidence = estimate_confidence(...);
    decide_match(best_y_offset, best_sad_avg, stationary_sad_avg, confidence, template_y)
    // 新
    let confidence = estimate_confidence(...);
    decide_match(best_y_offset, best_sad_avg, stationary_sad_avg, confidence, template_y, sad_accept)
```

- [ ] **Step 5: `extract_template`、`search_best_offset`、`estimate_confidence`、`sparse_sad_at_offset` 也参数化 `strip_h`**

这四个函数内部都用 `STRIP_H` 常量。改为接受 `strip_h: u32` 参数，调用时传入。逐个修改：

`extract_template`：
```rust
fn extract_template(ref_buf: &GrayBuf, template_y: u32, sample_cols: &[usize], strip_h: u32) -> Vec<u8> {
    let mut tpl = Vec::with_capacity(strip_h as usize * sample_cols.len());
    for dy in 0..strip_h {
```

`search_best_offset`：签名加 `strip_h: u32`，函数体 `let strip_h = STRIP_H as usize;` 改为 `let strip_h = strip_h as usize;`

`estimate_confidence`：签名加 `strip_h: u32`，函数体 `let strip_h = STRIP_H as usize;` 改为 `let strip_h = strip_h as usize;`

`sparse_sad_at_offset`：签名加 `strip_h: u32`，函数体 `let strip_h = STRIP_H as usize;` 改为 `let strip_h = strip_h as usize;`

所有调用点在 `find_overlap_spatial_ext` 内部，传入 `strip_h`。

- [ ] **Step 6: 临时修改 `process_frame` 和 `finalize` 的调用点以适配新签名**

`process_frame` 中的调用改为（临时直接传常量，Task 4 重构为降级链）：

旧调用：
```rust
        let (dy, confidence) = match find_overlap_spatial_ext(
            &self.reference,
            &curr_buf,
            x_start,
            x_end,
            eff_top,
            eff_bottom,
            max_scroll,
            self.last_dy,
        ) {
```

新调用：
```rust
        let (dy, confidence) = match find_overlap_spatial_ext(
            &self.reference,
            &curr_buf,
            x_start,
            x_end,
            eff_top,
            eff_bottom,
            max_scroll,
            self.last_dy,
            SAD_ACCEPT,  // 临时用硬编码，Task 4 改为动态阈值
            STRIP_H,     // 默认模板高度
        ) {
```

`finalize` 中的调用同样加 `SAD_ACCEPT, STRIP_H` 两个参数。

- [ ] **Step 7: 编译验证**

Run:
```bash
cd /Users/wudarui/workspace/agent/octopus/.worktrees/optimize-capx && cargo check -p octopus-capx 2>&1 | tail -5
```
Expected: `Finished`。

- [ ] **Step 8: 运行测试验证无回归**

Run:
```bash
cd /Users/wudarui/workspace/agent/octopus/.worktrees/optimize-capx && cargo test -p octopus-capx 2>&1 | tail -8
```
Expected: 12 passed。

**如果测试失败**：可能是参数化 `strip_h` 时漏改了某处 `STRIP_H` 引用。用 `grep -n "STRIP_H" crates/capx/src/stitch.rs` 确认所有引用都已参数化（常量定义本身除外）。

- [ ] **Step 9: Commit**

```bash
cd /Users/wudarui/workspace/agent/octopus/.worktrees/optimize-capx
git add crates/capx/src/stitch.rs
git commit -m "feat(capx): 时序平滑静止判断 + find_overlap_spatial_ext 参数化（strip_h/sad_accept）"
```

---

## Task 4: process_frame 重构——动态阈值 + 静止双重校验 + dy_history 更新

**Files:**
- Modify: `crates/capx/src/stitch.rs`（`process_frame` 主匹配分支）

- [ ] **Step 1: 修改 `process_frame` 主匹配分支——引入动态阈值、静止双重校验、dy_history 更新**

在 `process_frame` 中，找到主匹配调用（Task 3 Step 6 的临时版本），替换为：

旧（Task 3 临时版本）：
```rust
        let curr_buf = GrayBuf::from_rgba(frame);

        let x_start = (w as f64 * X_START_RATIO) as u32;
        let x_end = (w as f64 * X_END_RATIO) as u32;

        let max_scroll = MAX_SCROLL;
        let (dy, confidence) = match find_overlap_spatial_ext(
            &self.reference,
            &curr_buf,
            x_start,
            x_end,
            eff_top,
            eff_bottom,
            max_scroll,
            self.last_dy,
            SAD_ACCEPT,  // 临时用硬编码，Task 4 改为动态阈值
            STRIP_H,     // 默认模板高度
        ) {
            Some(v) => v,
            None => {
                log::info!("[stitch] find_overlap_spatial returned None");
                self.last_dy = None;
                return Ok(false);
            }
        };
```

新：
```rust
        let curr_buf = GrayBuf::from_rgba(frame);

        let x_start = (w as f64 * X_START_RATIO) as u32;
        let x_end = (w as f64 * X_END_RATIO) as u32;

        let max_scroll = MAX_SCROLL;
        let sample_cols: Vec<usize> = (x_start as usize..x_end as usize)
            .step_by(SAMPLE_STEP_X)
            .collect();

        // 动态阈值：根据当前帧纹理密度 + 历史基线计算
        let template_y = eff_bottom.saturating_sub(STRIP_H);
        let texture = estimate_texture_density(&curr_buf, &sample_cols, template_y);
        let sad_accept = self.dynamic_sad_accept(texture);

        let (dy, confidence) = match find_overlap_spatial_ext(
            &self.reference,
            &curr_buf,
            x_start,
            x_end,
            eff_top,
            eff_bottom,
            max_scroll,
            self.last_dy,
            sad_accept,
            STRIP_H,
        ) {
            Some(v) => v,
            None => {
                // 降级链在 Task 5 实现
                log::info!("[stitch] main match failed, entering fallback (Task 5)");
                self.last_dy = None;
                return Ok(false);
            }
        };

        // 静止双重校验：dy ≈ 0 且时序也确认静止才跳过
        if dy.abs() < 0.5 && self.is_stationary() {
            log::info!("[stitch] stationary confirmed by temporal smoothing");
            return Ok(false);
        }
```

紧接在现有的 `dy >= 0.0` 检查和 `new_rows` 限制检查之后，追加内容追加成功后的 `dy_history` 和 `sad_baseline` 更新。找到现有的画布追加代码段：

旧（画布追加之后、`Ok(true)` 之前）：
```rust
        // 更新参考灰度与速度缓存
        self.reference = curr_buf;
        self.last_dy = Some(dy);

        Ok(true)
```

新：
```rust
        // 更新参考灰度与速度缓存
        self.reference = curr_buf;
        self.last_dy = Some(dy);

        // 更新 dy_history（时序平滑）和 sad_baseline（动态阈值 EMA）
        self.dy_history.push_back(dy);
        if self.dy_history.len() > DY_HISTORY_LEN {
            self.dy_history.pop_front();
        }
        if self.sad_baseline == 0.0 {
            self.sad_baseline = best_sad;  // 首次直接赋值
        } else {
            self.sad_baseline = SAD_BASELINE_ALPHA * best_sad + (1.0 - SAD_BASELINE_ALPHA) * self.sad_baseline;
        }

        Ok(true)
```

> **注意**：`best_sad` 变量名需要从 `find_overlap_spatial_ext` 的返回值获取。当前 `find_overlap_spatial_ext` 返回 `Option<(f64, f64)>` = `(dy, confidence)`，不含 `best_sad`。需要改为返回 `(dy, confidence, best_sad)` 三元组。

- [ ] **Step 2: 修改 `find_overlap_spatial_ext` 返回值包含 `best_sad_avg`**

旧返回类型：`Option<(f64, f64)>` = `(dy, confidence)`

新返回类型：`Option<(f64, f64, f64)>` = `(dy, confidence, best_sad_avg)`

`decide_match` 返回值改为 `Option<(f64, f64, f64)>`：

```rust
fn decide_match(
    best_y_offset: u32,
    best_sad_avg: f64,
    stationary_sad_avg: f64,
    confidence: f64,
    template_y: u32,
    sad_accept: f64,
) -> Option<(f64, f64, f64)> {
    if stationary_sad_avg < STATIONARY_SAD {
        return Some((0.0, 1.0, 0.0));  // 静止时 best_sad=0
    }
    if best_sad_avg < sad_accept && confidence > MIN_CONFIDENCE {
        let dy = best_y_offset as f64 - template_y as f64;
        Some((dy, confidence, best_sad_avg))
    } else {
        None
    }
}
```

`process_frame` 中的 match 改为解构三元组：
```rust
        let (dy, confidence, best_sad) = match find_overlap_spatial_ext(...) { ... };
```

`finalize` 中的调用也同步解构（只取 dy 和 confidence，忽略 best_sad）：
```rust
        if let Some((dy, confidence, _)) = find_overlap_spatial_ext(...) {
```

- [ ] **Step 3: 编译验证**

Run:
```bash
cd /Users/wudarui/workspace/agent/octopus/.worktrees/optimize-capx && cargo check -p octopus-capx 2>&1 | tail -5
```
Expected: `Finished`。

- [ ] **Step 4: 运行测试验证无回归**

Run:
```bash
cd /Users/wudarui/workspace/agent/octopus/.worktrees/optimize-capx && cargo test -p octopus-capx 2>&1 | tail -8
```
Expected: 12 passed。

**如果 `test_stationary_frame_returns_false` 失败**：因为静止判断现在需要 `dy_history` 攒够 3 帧。init 阶段（第一帧）`process_frame` 返回 false 不更新 `dy_history`，第二帧（真正静止帧）`dy_history` 仍空 → `is_stationary()` 返回 false → dy=0 但不判静止 → 但 `dy >= 0.0` 检查会 return false（dy=0 不追加）。确认：测试中 `dy.abs() < 0.5 && self.is_stationary()` 在 `dy_history` 为空时，`is_stationary()` 返回 false，所以不会进入静止分支；但 `dy >= 0.0` 会在后面 return false——这是正确行为，测试应通过。

- [ ] **Step 5: Commit**

```bash
cd /Users/wudarui/workspace/agent/octopus/.worktrees/optimize-capx
git add crates/capx/src/stitch.rs
git commit -m "feat(capx): process_frame 动态阈值 + 静止双重校验 + dy_history/sad_baseline 更新"
```

---

## Task 5: 三级兜底降级链

**Files:**
- Modify: `crates/capx/src/stitch.rs`（`process_frame` 降级分支 + `try_match` / `try_match_strip` / `try_match_1d_projection`）

- [ ] **Step 1: 在 `impl Stitcher` 块内（`is_stationary` 之后）新增 `try_match` 封装方法**

```rust
    /// 主匹配封装：调用 find_overlap_spatial_ext。
    fn try_match(
        &self,
        curr: &GrayBuf,
        x_start: u32,
        x_end: u32,
        eff_top: u32,
        eff_bottom: u32,
        max_scroll: u32,
        sad_accept: f64,
        strip_h: u32,
    ) -> Option<(f64, f64, f64)> {
        find_overlap_spatial_ext(
            &self.reference,
            curr,
            x_start,
            x_end,
            eff_top,
            eff_bottom,
            max_scroll,
            self.last_dy,
            sad_accept,
            strip_h,
        )
    }
```

- [ ] **Step 2: 新增 `try_match_1d_projection` 方法**

在 `try_match` 之后追加：

```rust
    /// 降级 3：1D 灰度投影匹配。
    /// 将每行像素按抽样列取均值降为一维信号，对一维信号做 SAD 搜索。
    /// 对纯色/低纹理场景（2D SAD 缺乏特征）更鲁棒。
    fn try_match_1d_projection(
        &self,
        curr: &GrayBuf,
        x_start: u32,
        x_end: u32,
        eff_top: u32,
        eff_bottom: u32,
        max_scroll: u32,
        sad_accept: f64,
    ) -> Option<(f64, f64, f64)> {
        let strip_h = STRIP_H;
        if eff_bottom <= eff_top + strip_h + 10 {
            return None;
        }
        let template_y = eff_bottom - strip_h;

        // 构建抽样列索引
        let cols: Vec<usize> = (x_start as usize..x_end as usize)
            .step_by(SAMPLE_STEP_X)
            .collect();
        if cols.is_empty() {
            return None;
        }

        // 计算行均值信号
        let ref_proj = row_projection_means(&self.reference, &cols, template_y, template_y + strip_h);
        let search_start = (template_y as i32 - max_scroll as i32).max(eff_top as i32) as u32;

        // 一维 SAD 搜索
        let mut best_offset = template_y;
        let mut min_sad = f64::MAX;
        let total = strip_h as f64;

        for y_offset in search_start..=template_y {
            let curr_proj = row_projection_means(curr, &cols, y_offset, y_offset + strip_h);
            let mut sad = 0.0f64;
            for i in 0..strip_h as usize {
                sad += (ref_proj[i] - curr_proj[i]).abs();
            }
            let sad_avg = sad / total;
            if sad_avg < min_sad {
                min_sad = sad_avg;
                best_offset = y_offset;
            }
        }

        // 静止检查
        let stationary_sad = {
            let curr_proj = row_projection_means(curr, &cols, template_y, template_y + strip_h);
            let mut sad = 0.0f64;
            for i in 0..strip_h as usize {
                sad += (ref_proj[i] - curr_proj[i]).abs();
            }
            sad / total
        };
        if stationary_sad < STATIONARY_SAD {
            return Some((0.0, 1.0, 0.0));
        }

        // 置信度（简化版：1D 最佳与均值比）
        let mut sum_sad = 0.0f64;
        let mut count = 0.0f64;
        for y_offset in (search_start..=template_y).step_by(10) {
            let curr_proj = row_projection_means(curr, &cols, y_offset, y_offset + strip_h);
            let mut sad = 0.0f64;
            for i in 0..strip_h as usize {
                sad += (ref_proj[i] - curr_proj[i]).abs();
            }
            sum_sad += sad / total;
            count += 1.0;
        }
        let mean_sad = sum_sad / count;
        let confidence = if mean_sad > 1e-5 {
            1.0 - (min_sad / mean_sad)
        } else {
            0.0
        };

        // 1D 投影置信度要求更严（0.25 vs 0.15）
        if min_sad < sad_accept && confidence > 0.25 {
            let dy = best_offset as f64 - template_y as f64;
            Some((dy, confidence, min_sad))
        } else {
            None
        }
    }
```

- [ ] **Step 3: 在自由函数区（`estimate_texture_density` 附近）新增 `row_projection_means` helper**

```rust
/// 计算灰度 buffer 指定行范围 [y_start, y_end) 的每行抽样列均值，降为一维信号。
fn row_projection_means(buf: &GrayBuf, cols: &[usize], y_start: u32, y_end: u32) -> Vec<f64> {
    let n = (y_end - y_start) as usize;
    let mut proj = Vec::with_capacity(n);
    for y in y_start..y_end {
        let row = buf.row(y as usize);
        let sum: u64 = cols.iter().map(|&x| row[x] as u64).sum();
        proj.push(sum as f64 / cols.len() as f64);
    }
    proj
}
```

- [ ] **Step 4: 修改 `process_frame` 的 `None` 分支——替换为三级降级链**

旧（Task 4 版本的 None 分支）：
```rust
            None => {
                // 降级链在 Task 5 实现
                log::info!("[stitch] main match failed, entering fallback (Task 5)");
                self.last_dy = None;
                return Ok(false);
            }
```

新：
```rust
            None => {
                // 进入三级降级链
                log::info!("[stitch] main match failed, entering fallback chain");

                // 降级 1：扩大搜索范围 ×2（快速滚动可能超出 MAX_SCROLL）
                if let Some((dy, conf, sad)) = self.try_match(
                    &curr_buf, x_start, x_end, eff_top, eff_bottom, max_scroll * 2, sad_accept, STRIP_H,
                ) {
                    log::info!("[stitch] fallback 1: expanded search range, dy={:.1} conf={:.4}", dy, conf);
                    return self.apply_fallback_match(dy, conf, sad, frame, &curr_buf, w, eff_top, eff_bottom);
                }

                // 降级 2：缩小模板到 FALLBACK_STRIP_H + 放宽阈值
                if let Some((dy, conf, sad)) = self.try_match(
                    &curr_buf, x_start, x_end, eff_top, eff_bottom, max_scroll, sad_accept * FALLBACK_SAD_MULTIPLIER, FALLBACK_STRIP_H,
                ) {
                    log::info!("[stitch] fallback 2: reduced strip height, dy={:.1} conf={:.4}", dy, conf);
                    return self.apply_fallback_match(dy, conf, sad, frame, &curr_buf, w, eff_top, eff_bottom);
                }

                // 降级 3：1D 灰度投影匹配
                if let Some((dy, conf, sad)) = self.try_match_1d_projection(
                    &curr_buf, x_start, x_end, eff_top, eff_bottom, max_scroll, sad_accept,
                ) {
                    log::info!("[stitch] fallback 3: 1D projection match, dy={:.1} conf={:.4}", dy, conf);
                    return self.apply_fallback_match(dy, conf, sad, frame, &curr_buf, w, eff_top, eff_bottom);
                }

                // 全部失败：不停止，等下一帧
                log::info!("[stitch] all fallbacks exhausted, skipping frame");
                self.last_dy = None;
                return Ok(false);
            }
```

- [ ] **Step 5: 新增 `apply_fallback_match` 方法——复用主匹配的 dy 检查 + 追加逻辑**

在 `try_match_1d_projection` 之后追加：

```rust
    /// 降级匹配结果的处理（复用主匹配的 dy 检查 + 画布追加 + 状态更新）。
    fn apply_fallback_match(
        &mut self,
        dy: f64,
        _confidence: f64,
        best_sad: f64,
        frame: &RgbaImage,
        curr_buf: &GrayBuf,
        w: u32,
        eff_top: u32,
        eff_bottom: u32,
    ) -> Result<bool> {
        // 与主匹配相同的 dy 方向 + 幅度检查
        if dy >= 0.0 {
            self.last_dy = None;
            return Ok(false);
        }
        let new_rows = (-dy).round() as u32;
        let max_scroll_limit = (eff_bottom - eff_top) * 4 / 5;
        if new_rows < self.config.min_scroll_px as u32 || new_rows >= max_scroll_limit {
            self.last_dy = None;
            return Ok(false);
        }

        // 画布追加
        let crop_y = eff_bottom - new_rows;
        let row_bytes = w as usize * 4;
        let start = crop_y as usize * row_bytes;
        let end = start + new_rows as usize * row_bytes;
        let frame_raw = frame.as_raw();
        self.canvas_buf.extend_from_slice(&frame_raw[start..end]);
        self.canvas_h += new_rows;
        self.invalidate_cache();

        // 更新状态
        self.reference = curr_buf.clone_buf();
        self.last_dy = Some(dy);
        self.dy_history.push_back(dy);
        if self.dy_history.len() > DY_HISTORY_LEN {
            self.dy_history.pop_front();
        }
        if self.sad_baseline == 0.0 {
            self.sad_baseline = best_sad;
        } else {
            self.sad_baseline = SAD_BASELINE_ALPHA * best_sad + (1.0 - SAD_BASELINE_ALPHA) * self.sad_baseline;
        }

        Ok(true)
    }
```

> **注意**：`apply_fallback_match` 中 `self.reference = curr_buf.clone_buf()` 需要 `GrayBuf` 支持 clone。因为 `curr_buf` 在主匹配中已被 `self.reference = curr_buf` 消费，但降级链中 `curr_buf` 是借用的。需要在 `GrayBuf` 上加 `clone_buf` 方法或 derive Clone。见 Step 6。

- [ ] **Step 6: 为 `GrayBuf` 添加 `clone_buf` 方法（或 derive Clone）**

在 `GrayBuf` struct 定义上方加 `#[derive(Clone)]`：
```rust
#[derive(Clone)]
struct GrayBuf {
    data: Vec<u8>,
    width: usize,
}
```

然后 `apply_fallback_match` 中的 `self.reference = curr_buf.clone_buf()` 改为 `self.reference = curr_buf.clone()`。

同时主匹配的 `process_frame` 中 `self.reference = curr_buf` 也需调整为 `self.reference = curr_buf.clone()`，因为降级链也要用 `curr_buf`。但主匹配成功时降级链不执行，`curr_buf` 只被赋值一次。**检查所有权**：

实际上在主匹配 `Some` 分支中，`curr_buf` 被 `self.reference = curr_buf` move 了。降级链在 `None` 分支中，`curr_buf` 仍可用（主匹配的 `find_overlap_spatial_ext` 只借用 `&curr_buf`）。所以：
- 主匹配 `Some`：`self.reference = curr_buf`（move，OK）
- 降级链：`curr_buf` 仍可用，`apply_fallback_match` 内 `self.reference = curr_buf.clone()`（clone，OK）

- [ ] **Step 7: 修改 `finalize` 的调用点——适配新签名（sad_accept + strip_h + 三元组返回）**

`finalize` 中的调用：

旧：
```rust
        if let Some((dy, confidence)) = find_overlap_spatial_ext(
            &self.reference,
            &last_buf,
            x_start,
            x_end,
            eff_top,
            eff_bottom,
            max_finalize_scroll,
            None,
        ) {
```

新：
```rust
        if let Some((dy, confidence, _)) = find_overlap_spatial_ext(
            &self.reference,
            &last_buf,
            x_start,
            x_end,
            eff_top,
            eff_bottom,
            max_finalize_scroll,
            None,
            SAD_ACCEPT,
            STRIP_H,
        ) {
```

- [ ] **Step 8: 编译验证**

Run:
```bash
cd /Users/wudarui/workspace/agent/octopus/.worktrees/optimize-capx && cargo check -p octopus-capx 2>&1 | tail -5
```
Expected: `Finished`。

- [ ] **Step 9: 运行测试验证无回归**

Run:
```bash
cd /Users/wudarui/workspace/agent/octopus/.worktrees/optimize-capx && cargo test -p octopus-capx 2>&1 | tail -8
```
Expected: 12 passed。

- [ ] **Step 10: Commit**

```bash
cd /Users/wudarui/workspace/agent/octopus/.worktrees/optimize-capx
git add crates/capx/src/stitch.rs
git commit -m "feat(capx): 三级兜底降级链（扩大范围→缩小模板→1D 投影）"
```

---

## Task 6: 增强测试用例

**Files:**
- Modify: `crates/capx/src/stitch.rs`（测试模块 + `make_frame` 工具增强）

- [ ] **Step 1: 增强 `make_frame` 支持可控纹理密度**

在 `make_frame` 函数之前新增 `make_frame_textured`：

```rust
    /// 合成不同纹理密度的测试帧。
    /// texture_level: 0=纯色背景, 1=稀疏文字, 2=密集条纹
    fn make_frame_textured(width: u32, height: u32, scroll_offset: u32, texture_level: u32) -> RgbaImage {
        let mut img: ImageBuffer<Rgba<u8>, Vec<u8>> = ImageBuffer::new(width, height);
        for y in 0..height {
            for x in 0..width {
                let mut v = ((y + scroll_offset) % 256) as u8;
                match texture_level {
                    0 => {}, // 纯色，仅渐变
                    1 => { // 稀疏文字：每 20 行、每 50 列一个亮点
                        if y % 20 == 0 && x % 50 == 0 { v = v.saturating_add(100); }
                    }
                    2 => { // 密集条纹：每 5 行强对比
                        if (y + scroll_offset) % 5 == 0 { v = 255 - v; }
                        if x % 3 == 0 { v = v.saturating_add(60); }
                    }
                    _ => {},
                }
                let px = Rgba([v, v, v, 255]);
                img.put_pixel(x, y, px);
            }
        }
        img
    }
```

- [ ] **Step 2: 新增时序平滑测试**

在测试模块末尾（最后一个测试之后、`}` 之前）追加：

```rust
    #[test]
    fn test_is_stationary_with_history() {
        let f0 = make_frame(TW, TH, 0);
        let mut s = Stitcher::new(f0, StitchConfig::default());
        let f1 = make_frame(TW, TH, 0);
        s.process_frame(&f1).unwrap(); // init

        // 无 dy_history → 不静止
        assert!(!s.is_stationary(), "空 history 不应判静止");

        // 手动注入 dy_history 模拟持续滚动
        s.dy_history.extend([−15.0, −12.0, −10.0, −3.0]);
        assert!(!s.is_stationary(), "回弹帧 history 均值 -10 不应判静止");

        // 手动注入接近静止的 history
        s.dy_history.clear();
        s.dy_history.extend([−1.0, 0.0, −0.5, 1.0, 0.0]);
        assert!(s.is_stationary(), "均值接近 0 应判静止");
    }
```

> **注意**：测试中直接操作 `s.dy_history` 需要 `dy_history` 在测试模块可访问。由于测试模块是 `mod tests` 在同一文件内，可以访问私有字段。

- [ ] **Step 3: 新增动态阈值测试**

```rust
    #[test]
    fn test_dynamic_sad_accept_scales_with_texture() {
        let f0 = make_frame(TW, TH, 0);
        let mut s = Stitcher::new(f0, StitchConfig::default());

        // sad_baseline = 0 时，baseline_cap = 5.0
        // 低纹理：texture=0.05 → bonus=1.5 → (7.5+1.5).min(5.0).max(7.5) = 7.5
        let low = s.dynamic_sad_accept(0.05);
        assert_eq!(low, SAD_ACCEPT, "低纹理且 baseline=0 应返回基础阈值");

        // 设定 baseline 后
        s.sad_baseline = 10.0;
        // baseline_cap = 10*1.5+5 = 20
        // 高纹理：texture=0.5 → bonus=15 → (7.5+15).min(20).max(7.5) = 20
        let high = s.dynamic_sad_accept(0.5);
        assert!(high > SAD_ACCEPT, "高纹理应放宽阈值: {}", high);
        assert!(high <= 20.0, "不应超过 baseline_cap: {}", high);
    }
```

- [ ] **Step 4: 新增降级链测试**

```rust
    #[test]
    fn test_fallback_expanded_search_range() {
        // 构造超出 MAX_SCROLL 的快速滚动：init 后直接跳 300px
        let f0 = make_frame(TW, TH, 0);
        let mut s = Stitcher::new(f0, StitchConfig::default());
        let f1 = make_frame(TW, TH, 0);
        s.process_frame(&f1).unwrap(); // init
        // 300px 超出 MAX_SCROLL=220，主匹配应失败，降级 1 扩大到 440 应成功
        let f2 = make_frame(TW, TH, 300);
        let added = s.process_frame(&f2).unwrap();
        assert!(added, "快速滚动应通过降级 1（扩大搜索范围）匹配");
    }

    #[test]
    fn test_fallback_1d_projection_low_texture() {
        // 低纹理场景：纯色背景 + 稀疏文字
        let f0 = make_frame_textured(TW, TH, 0, 0); // 纯色
        let mut s = Stitcher::new(f0, StitchConfig::default());
        let f1 = make_frame_textured(TW, TH, 0, 0);
        s.process_frame(&f1).unwrap(); // init
        let f2 = make_frame_textured(TW, TH, 30, 0); // 滚动 30px
        // 2D SAD 在纯色页可能失败，降级 3 的 1D 投影应能匹配
        let added = s.process_frame(&f2).unwrap();
        // 注意：纯色背景可能 2D SAD 也能匹配（渐变特征），这个测试验证至少不报错
        let _ = added; // 不强制 assert，验证不 panic
    }
```

- [ ] **Step 5: 编译 + 运行全部测试**

Run:
```bash
cd /Users/wudarui/workspace/agent/octopus/.worktrees/optimize-capx && cargo test -p octopus-capx 2>&1 | tail -15
```
Expected: 现有 12 + 新增 ≥4 = ≥16 passed。

**如果 `test_fallback_expanded_search_range` 失败**：合成图在 300px 偏移下可能因渐变周期性（256）导致匹配混乱。尝试减小偏移到 250px 或增大 `make_frame` 的特征密度。

**如果 `test_is_stationary_with_history` 编译失败**：检查 `dy_history` 字段名拼写，以及 `extend` 方法接受 `VecDeque` 还是数组。可能需要 `s.dy_history.extend([−15.0, −12.0, −10.0, −3.0].into_iter())` 或 `s.dy_history.extend(vec![−15.0, −12.0, −10.0, −3.0])`。

- [ ] **Step 6: Commit**

```bash
cd /Users/wudarui/workspace/agent/octopus/.worktrees/optimize-capx
git add crates/capx/src/stitch.rs
git commit -m "test(capx): 新增时序平滑/动态阈值/降级链单元测试"
```

---

## Task 7: 文档同步

**Files:**
- Modify: `docs/superpowers/specs/2026-07-02-capx-stitch-robustness-design.md`（标注实施记录）
- Modify: `docs/architecture.md`（stitch 模块描述更新）

- [ ] **Step 1: 更新 spec 状态为实施完成**

在 spec 文件头部的状态字段后追加实施记录：
```
**状态**: ✅ 实施完成（3 改造 + 测试全部落地）
```

并在文件末尾追加实施记录段落（标注偏差，若有）。

- [ ] **Step 2: 更新 architecture.md 的 stitch 描述**

找到 architecture.md 中 stitch 模块描述行（包含"2D SAD 空间模板匹配"），追加健壮性优化的关键点：
- 时序平滑静止判断（替代单帧静态校验硬覆盖）
- 动态自适应 SAD 阈值（纹理密度 + EMA 基线）
- 三级兜底降级链（扩大范围→缩小模板→1D 投影）

- [ ] **Step 3: Commit**

```bash
cd /Users/wudarui/workspace/agent/octopus/.worktrees/optimize-capx
git add docs/
git commit -m "docs(capx): 同步拼接健壮性优化实施记录与 architecture 更新"
```

---

## 验收清单（全部任务完成后核对）

- [ ] `cargo test -p octopus-capx` 全绿（≥16 个测试）
- [ ] `cargo check -p octopus-capx -p octopus-desktop` 无错误
- [ ] API 零改动：`git diff main -- crates/capx/src/lib.rs` 为空，公开签名不变
- [ ] 源码无新增裸魔法数字
- [ ] 降级链有日志输出（`[stitch] fallback N: ...`）
- [ ] 文档同步完成


---

## 来自原文件 `2026-07-02-notepad-type-migration.md`

# 记事本 type 迁移 Implementation Plan

> ⚠️ **本计划已全部落地。** Task 1-13（原 html/text/markdown 三类型方案）+ 富文本追加移除均已实现，**e2e 通过后于 2026-07-02 双向同步合并 main（merge `6e004ac`）**。富文本（Html/TipTap）下线：`NoteType` 收窄为 `text`/`markdown`、删 TipTap 依赖、DB 迁移 v11→v12 删历史 html 笔记。下文保留原任务留档（设计演进考古），凡涉及 `Html`/TipTap/`extract_text`/图片桥接的 step 描述均已被后续移除覆盖；**当前真相以重写后的 spec（双类型最终设计）与 `docs/architecture.md` §octopus-notepad 为准**。
>
> **富文本移除的追加变更**（不在下文 Task 内）：
> - `crates/notepad/src/model.rs`：`NoteType` 去 `Html`（默认 `Text`，`from_str` 未知→Text）；删 `serialize.rs` + `Cargo.toml` 的 `scraper`。
> - `crates/notepad/src/store.rs`：`split_body` 简化（content_html 恒空），测试 `NoteType::Html`→`Text`。
> - `crates/infra/src/db.rs` + `db.sql`：v11→v12 迁移删 html 笔记；notes.type 默认 `'text'`。
> - `crates/desktop/src/note_commands.rs` + `main.rs`：删 `get_note_image`/`insert_note_image` + 注销。
> - 前端：删 `extensions.tsx`、`NoteEditor.tsx` 去 html 分支/工具栏/linkInput、`NoteList.tsx` 去 html tab/选项、`index.css` 删 `.ProseMirror`、`package.json` 删 `@tiptap/*`+`tiptap-markdown`。
> - 提交：`b86f53d`。

---

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 在 webview（TipTap）现役记事本上采纳 egui 分支的 `content_text + type` 表结构（保留 `content_html`），把 `type` 放开到 text/markdown/html 三态，提供安全迁移与三类型编辑器。

**Architecture:** DB 层加 `type` 列（v9→v10 幂等 ALTER，不丢数据）；后端 `NoteType` enum + `Note.note_type`，store 按 type 分发抽取（仅 html 抽取纯文本）；IPC create/update 透传 type；前端按 `note_type` 分发 TipTap/textarea/markdown 编辑器，新建时选 type、已建锁定。

**Tech Stack:** Rust（rusqlite, serde）、React + TypeScript + TipTap、`marked`（md 预览）、Tauri IPC、SQLite FTS5。

**Spec:** `docs/superpowers/specs/2026-07-02-notepad-type-migration-design.md`

**关键约束（来自 CLAUDE.md / 记忆）：**
- worktree 内 cargo/git 必须显式指 worktree 路径（`--manifest-path` / `-C` / 绝对路径）—— worktree cwd 陷阱。
- 前端改完必须 `npm run build` 并提交 `crates/desktop/dist/*`（dist 已跟踪）。
- `config/` 用绝对路径 `~/.octopus/`（本任务不涉及 config）。

---

## Task 1: `NoteType` enum + `Note.note_type` 字段

**Files:**
- Modify: `crates/notepad/src/model.rs`
- Modify: `crates/notepad/src/lib.rs`

- [x] **Step 1: 写失败测试** — 在 `model.rs` 的 `#[cfg(test)] mod tests` 内追加：

```rust
    #[test]
    fn note_type_roundtrip() {
        for t in [NoteType::Html, NoteType::Text, NoteType::Markdown] {
            assert_eq!(NoteType::from_str(t.as_str()), t);
        }
    }

    #[test]
    fn note_type_from_unknown_defaults_html() {
        // 未知值 → Html（保守：历史/异常值保持富文本不丢格式）
        assert_eq!(NoteType::from_str("???"), NoteType::Html);
        assert_eq!(NoteType::from_str(""), NoteType::Html);
    }
```

- [x] **Step 2: 跑测试确认失败** — `cargo test --manifest-path crates/notepad/Cargo.toml -p octopus-notepad note_type` → 编译失败（`NoteType` 未定义）。

- [x] **Step 3: 实现 NoteType** — 在 `model.rs` 的 `NoteSource` impl 之后、`Note` struct 之前插入：

```rust
/// 笔记内容格式（DB `notes.type` 列）。
/// - `Html`：TipTap 富文本（content_html 存原始，content_text 存抽取纯文本）。
/// - `Text`：纯文本（content_text 存原文，content_html 空）。
/// - `Markdown`：md 源码（content_text 存源码，content_html 空，预览端渲染）。
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum NoteType {
    #[default]
    Html,
    Text,
    Markdown,
}

impl NoteType {
    pub fn as_str(&self) -> &'static str {
        match self {
            NoteType::Html => "html",
            NoteType::Text => "text",
            NoteType::Markdown => "markdown",
        }
    }

    pub fn from_str(s: &str) -> Self {
        match s {
            "text" => NoteType::Text,
            "markdown" => NoteType::Markdown,
            // "html" 及未知值 → Html（历史数据 DEFAULT 'html'，容错偏富文本）
            _ => NoteType::Html,
        }
    }
}
```

- [x] **Step 4: Note struct 加字段** — 把 `Note` struct 改为（在 `content_text` 后加 `note_type`）：

```rust
pub struct Note {
    pub id: i64,
    pub title: Option<String>,
    pub content_html: String,
    pub content_text: String,
    pub note_type: NoteType,
    pub source: NoteSource,
    pub source_ref_id: Option<i64>,
    pub is_pinned: bool,
    pub is_favorite: bool,
    pub created_at: String,
    pub updated_at: String,
}
```

- [x] **Step 5: lib.rs 导出 NoteType** — `pub use model::{Note, NoteFilter, NoteSource};` 改为：

```rust
pub use model::{Note, NoteFilter, NoteSource, NoteType};
```

- [x] **Step 6: 跑测试确认通过** — `cargo test --manifest-path crates/notepad/Cargo.toml -p octopus-notepad` → NoteType 测试 PASS（store 测试此时可能编译失败，Task 4 修，本步只看 model 测试通过即可；若 store 编译错阻碍，临时 `cargo test -p octopus-notepad --lib model::tests`）。

- [x] **Step 7: 提交** — `git add crates/notepad/src/model.rs crates/notepad/src/lib.rs && git commit -m "feat(notepad): NoteType enum (html/text/markdown) + Note.note_type 字段"`

---

## Task 2: schema `db.sql` notes 加 `type` 列

**Files:**
- Modify: `crates/infra/src/db.sql`

- [x] **Step 1: 改 notes 建表** — 把 `db.sql` 中 notes 建表改为（加 `type` 列 + 更新注释）：

```sql
-- ── 记事本（notes 表）─────────────────────────────────────────────────────
-- 内容收集箱：ASR/OCR/剪贴板结果一键存入 + 富文本/markdown/纯文本整理。
-- type: 'html'(TipTap 富文本，默认) | 'text'(纯文本) | 'markdown'(md 源码)。
-- content_html = 富文本原始（仅 type=html）；content_text = 纯文本/md源码/html抽取（FTS + 列表预览）。
CREATE TABLE IF NOT EXISTS notes (
    id            INTEGER PRIMARY KEY AUTOINCREMENT,
    title         TEXT,
    content_text  TEXT    NOT NULL DEFAULT '',
    content_html  TEXT    NOT NULL DEFAULT '',
    type          TEXT    NOT NULL DEFAULT 'html',
    source        TEXT    NOT NULL DEFAULT 'manual',
    source_ref_id INTEGER,
    is_pinned     INTEGER NOT NULL DEFAULT 0,
    is_favorite   INTEGER NOT NULL DEFAULT 0,
    created_at    TEXT    NOT NULL,
    updated_at    TEXT    NOT NULL
);
```

> FTS5 表 + 触发器不变（仍索引 `content_text`，`type` 不进 FTS）。

- [x] **Step 2: 提交** — `git add crates/infra/src/db.sql && git commit -m "feat(infra): notes 表加 type 列 (html/text/markdown)"`

---

## Task 3: v9→v10 迁移（幂等 ALTER ADD type）

**Files:**
- Modify: `crates/infra/src/db.rs`
- Test: `crates/infra/src/db.rs`（同文件 `#[cfg(test)]`）

- [x] **Step 1: 写失败测试** — 在 db.rs 测试模块追加（参考 egui 分支 `migrate_v9_to_v10_rebuilds_notes_schema` 结构，但断言**保留数据**）：

```rust
    #[test]
    fn migrate_v9_to_v10_adds_type_column_keeps_data() {
        let dir = std::env::temp_dir().join(format!("octopus-type-mig-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("mig.db");
        let conn = Connection::open(&path).unwrap();
        apply_wal_pragmas(&conn);
        // 模拟旧 v9 库：notes 有 content_html/content_text，无 type
        conn.execute_batch(
            "CREATE TABLE notes (
                id INTEGER PRIMARY KEY AUTOINCREMENT, title TEXT,
                content_html TEXT NOT NULL DEFAULT '', content_text TEXT NOT NULL DEFAULT '',
                source TEXT NOT NULL DEFAULT 'manual', source_ref_id INTEGER,
                is_pinned INTEGER NOT NULL DEFAULT 0, is_favorite INTEGER NOT NULL DEFAULT 0,
                created_at TEXT NOT NULL, updated_at TEXT NOT NULL);
             INSERT INTO notes (title, content_html, content_text, source, created_at, updated_at)
                VALUES ('旧富文本', '<p>你好</p>', '你好', 'manual', '2026-01-01 00:00:00', '2026-01-01 00:00:00');
             PRAGMA user_version = 9;",
        ).unwrap();

        init_schema(&conn).unwrap();

        // type 列存在
        let cols: Vec<String> = conn.prepare("PRAGMA table_info(notes)").unwrap()
            .query_map([], |r| r.get::<_, String>(1)).unwrap()
            .filter_map(|r| r.ok()).collect();
        assert!(cols.contains(&"type".to_string()), "应有 type 列");
        assert!(cols.contains(&"content_html".to_string()), "content_html 应保留");

        // 旧数据保留，type 默认 html
        let row: (String, String, String) = conn.query_row(
            "SELECT content_html, content_text, type FROM notes WHERE title='旧富文本'", [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        ).unwrap();
        assert_eq!(row.0, "<p>你好</p>");
        assert_eq!(row.1, "你好");
        assert_eq!(row.2, "html", "历史笔记默认 type=html");

        let v: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0)).unwrap();
        assert_eq!(v, 10);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn migrate_v9_to_v10_is_idempotent() {
        // 重复 init_schema 不应崩溃（type 列已存在时跳过 ALTER）
        let dir = std::env::temp_dir().join(format!("octopus-type-mig-idem-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let conn = Connection::open(dir.join("mig.db")).unwrap();
        apply_wal_pragmas(&conn);
        conn.execute_batch(
            "CREATE TABLE notes (id INTEGER PRIMARY KEY, title TEXT, content_html TEXT DEFAULT '', content_text TEXT DEFAULT '', type TEXT DEFAULT 'html', source TEXT DEFAULT 'manual', source_ref_id INTEGER, is_pinned INTEGER DEFAULT 0, is_favorite INTEGER DEFAULT 0, created_at TEXT, updated_at TEXT);
             PRAGMA user_version = 9;",
        ).unwrap();
        // type 列已存在 → init_schema 应跳过 ALTER，不报 duplicate column
        init_schema(&conn).unwrap();
        let v: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0)).unwrap();
        assert_eq!(v, 10);
        let _ = std::fs::remove_dir_all(&dir);
    }
```

- [x] **Step 2: 跑测试确认失败** — `cargo test --manifest-path crates/infra/Cargo.toml -p octopus-infra migrate_v9_to_v10` → 失败（无 v9→v10 分支，user_version 停在 9）。

- [x] **Step 3: 实现迁移** — 在 db.rs `init_schema` 的 `} else if v == 8 { ... }` 分支之后、函数结束前追加：

```rust
    } else if v == 9 {
        // v9 → v10：notes 加 type 列（html/text/markdown）。
        // 幂等：v8→v9 重跑 INIT_SQL 建 notes 时已含 type（db.sql 已改），此处先查列存在再 ALTER。
        let has_type: bool = conn
            .prepare("PRAGMA table_info(notes)")?
            .query_map([], |r| r.get::<_, String>(1))?
            .filter_map(|r| r.ok())
            .any(|c| c == "type");
        if !has_type {
            conn.execute(
                "ALTER TABLE notes ADD COLUMN type TEXT NOT NULL DEFAULT 'html'",
                [],
            )
            .context("v9→v10: ALTER notes ADD type")?;
            log::info!("DB migrated to v10: notes.type 列已加（历史笔记默认 html）");
        }
        conn.execute("PRAGMA user_version = 10", [])?;
    }
```

- [x] **Step 4: v0/v1 新库直跳 v10** — 把 v0/v1 分支（`if v < 2 { ... conn.execute("PRAGMA user_version = 9", [])?; }`）的 `= 9` 改为 `= 10`（INIT_SQL 建的 notes 已带 type，新库直接 v10）。

- [x] **Step 5: 更新顶部 version 流转注释** — 在 init_schema 文档注释的版本流转说明里补一行 `/// - v9 → v10: notes 加 type 列（ALTER ADD，幂等）`，并把 v0/v1 注释里的 → v9 改 → v10。

- [x] **Step 6: 跑测试确认通过** — `cargo test --manifest-path crates/infra/Cargo.toml -p octopus-infra` → migrate_v9_to_v10_adds_type / _is_idempotent 均 PASS，且不破坏现有 db 测试。

- [x] **Step 7: 提交** — `git add crates/infra/src/db.rs && git commit -m "feat(infra): v9→v10 迁移 notes 加 type 列（幂等 ALTER，保留历史数据）"`

---

## Task 4: `store.rs` 适配 type（create/update/row/SELECT + 分发抽取）

**Files:**
- Modify: `crates/notepad/src/store.rs`

- [x] **Step 1: 写失败测试** — 在 store.rs 测试模块追加（覆盖三类型 create + 抽取分发 + update）：

```rust
    #[test]
    fn create_note_html_extracts_text() {
        // html 类型：content_text 由 html 抽取
        let (conn, _dir) = test_db();  // 见下：测试 helper
        let id = create_note_at(&conn, NoteSource::Manual, None, "<p>你好</p><p>世界</p>", NoteType::Html).unwrap();
        let n = get_note_at(&conn, id).unwrap().unwrap();
        assert_eq!(n.note_type, NoteType::Html);
        assert_eq!(n.content_html, "<p>你好</p><p>世界</p>");
        assert_eq!(n.content_text, "你好\n世界");  // extract_text 抽取
    }

    #[test]
    fn create_note_text_stores_raw_no_extract() {
        let (conn, _dir) = test_db();
        let id = create_note_at(&conn, NoteSource::Manual, None, "纯文本 <不抽取>", NoteType::Text).unwrap();
        let n = get_note_at(&conn, id).unwrap().unwrap();
        assert_eq!(n.note_type, NoteType::Text);
        assert_eq!(n.content_text, "纯文本 <不抽取>");  // 原文，不经抽取
        assert_eq!(n.content_html, "");                 // text 无 html
    }

    #[test]
    fn create_note_markdown_stores_source() {
        let (conn, _dir) = test_db();
        let id = create_note_at(&conn, NoteSource::Manual, None, "# 标题\n正文", NoteType::Markdown).unwrap();
        let n = get_note_at(&conn, id).unwrap().unwrap();
        assert_eq!(n.note_type, NoteType::Markdown);
        assert_eq!(n.content_text, "# 标题\n正文");
        assert_eq!(n.content_html, "");
    }

    #[test]
    fn update_note_by_type() {
        let (conn, _dir) = test_db();
        let id = create_note_at(&conn, NoteSource::Manual, None, "x", NoteType::Text).unwrap();
        update_note_at(&conn, id, "标题", "<p>新</p>", NoteType::Html).unwrap();
        let n = get_note_at(&conn, id).unwrap().unwrap();
        assert_eq!(n.title.as_deref(), Some("标题"));
        assert_eq!(n.note_type, NoteType::Html);
        assert_eq!(n.content_text, "新");
    }
```

> 若 store.rs 已有 `test_db()` helper 则复用；若无，在测试模块加：
> ```rust
> fn test_db() -> (rusqlite::Connection, std::path::PathBuf) {
>     let dir = std::env::temp_dir().join(format!("octopus-store-test-{}", std::process::id()));
>     let _ = std::fs::remove_dir_all(&dir);
>     std::fs::create_dir_all(&dir).unwrap();
>     let conn = rusqlite::Connection::open(dir.join("t.db")).unwrap();
>     octopus_infra::db::init_for_test(&conn);  // 见 Step 4 备注
>     (conn, dir)
> }
> ```

- [x] **Step 2: 跑测试确认失败** — `cargo test --manifest-path crates/notepad/Cargo.toml -p octopus-notepad create_note` → 编译失败（签名不匹配）。

- [x] **Step 3: 改 create 签名 + 分发抽取** — 替换 `create_note` / `create_note_at`：

```rust
/// 新建笔记。type=Html 时 content_text 由 body(html) 抽取；text/markdown 时 content_text=body 原文。
pub fn create_note(
    source: NoteSource,
    source_ref_id: Option<i64>,
    body: &str,
    note_type: NoteType,
) -> Result<i64> {
    octopus_infra::db::with_db(|conn| create_note_at(conn, source, source_ref_id, body, note_type))
}

pub fn create_note_at(
    conn: &Connection,
    source: NoteSource,
    source_ref_id: Option<i64>,
    body: &str,
    note_type: NoteType,
) -> Result<i64> {
    let (content_html, content_text) = split_body(body, note_type);
    let now = iso_now();
    conn.execute(
        "INSERT INTO notes (title, content_text, content_html, type, source, source_ref_id, is_pinned, is_favorite, created_at, updated_at)
         VALUES (NULL, ?, ?, ?, ?, ?, 0, 0, ?, ?)",
        params![content_text, content_html, note_type.as_str(), source.as_str(), source_ref_id, now, now],
    )
    .context("insert note")?;
    Ok(conn.last_insert_rowid())
}

/// 按 type 拆 body → (content_html, content_text)。
/// Html：html 存原始，text 存抽取纯文本。Text/Markdown：text 存原文/源码，html 空。
fn split_body(body: &str, note_type: NoteType) -> (String, String) {
    match note_type {
        NoteType::Html => (body.to_string(), extract_text(body)),
        NoteType::Text | NoteType::Markdown => (String::new(), body.to_string()),
    }
}
```

- [x] **Step 4: 改 update 签名** — 替换 `update_note` / `update_note_at`：

```rust
pub fn update_note(id: i64, title: &str, body: &str, note_type: NoteType) -> Result<()> {
    octopus_infra::db::with_db(|conn| update_note_at(conn, id, title, body, note_type))
}

pub fn update_note_at(conn: &Connection, id: i64, title: &str, body: &str, note_type: NoteType) -> Result<()> {
    let (content_html, content_text) = split_body(body, note_type);
    let title_db: Option<&str> = if title.trim().is_empty() { None } else { Some(title) };
    conn.execute(
        "UPDATE notes SET title = ?, content_text = ?, content_html = ?, type = ?, updated_at = ? WHERE id = ?",
        params![title_db, content_text, content_html, note_type.as_str(), iso_now(), id],
    )?;
    Ok(())
}
```

- [x] **Step 5: row_to_note + 3 处 SELECT 加 type** — `row_to_note` 改为（SELECT 多一列 `type`，索引顺移）：

```rust
fn row_to_note(row: &rusqlite::Row) -> rusqlite::Result<Note> {
    let source_str: String = row.get(4)?;
    let type_str: String = row.get(10)?;
    Ok(Note {
        id: row.get(0)?,
        title: row.get(1)?,
        content_html: row.get(2)?,
        content_text: row.get(3)?,
        note_type: NoteType::from_str(&type_str),
        source: NoteSource::from_str(&source_str),
        source_ref_id: row.get(5)?,
        is_pinned: row.get::<_, i64>(6)? != 0,
        is_favorite: row.get::<_, i64>(7)? != 0,
        created_at: row.get(8)?,
        updated_at: row.get(9)?,
    })
}
```

把 3 处 SELECT 列表（`list_notes_at` 的 sql、`query_with_search` 的两个 sql、`get_note_at` 的 prepare）统一在 `updated_at` 后加 `, type`：
- `list_notes_at`: `... is_pinned, is_favorite, created_at, updated_at, type FROM notes ...`
- `query_with_search` LIKE 分支: `... updated_at, type FROM notes ...`
- `query_with_search` FTS 分支: `... n.updated_at, n.type FROM notes_fts ...`
- `get_note_at`: `... updated_at, type FROM notes WHERE id = ?`

- [x] **Step 6: import NoteType** — store.rs 顶部 `use` 加 `NoteType`（如 `use crate::model::{Note, NoteFilter, NoteSource, NoteType};` 或现有 import 风格）。

- [x] **Step 7: 跑测试确认通过** — `cargo test --manifest-path crates/notepad/Cargo.toml -p octopus-notepad` → 全 PASS。

> **备注（test_db helper）**：若 `infra::db` 无 `init_for_test` 公开入口，测试 helper 改为直接执行建表 SQL：在 `test_db()` 内 `conn.execute_batch(include_str!("../../infra/src/db.sql 的 notes 部分"))`。实现时按 store.rs 现有测试模式对齐（读现有 store 测试怎么建临时库，照搬）。

- [x] **Step 8: 提交** — `git add crates/notepad/src/store.rs && git commit -m "feat(notepad): store 按 type 分发 create/update/读取（html 抽取，text/md 直存）"`

---

## Task 5: clipboard 剪贴板存入改调 notepad（type=text）

> **实际结论（实现时重新判定 → 本 task 无需改动，下方 step 仅作审查标记）**：plan 误把 `clipboard/src/store.rs:976` 的 INSERT 当生产逻辑，实为 `cleanup_preserves_images_referenced_by_notes` 测试代码（语义=html 笔记嵌 `note-img:` 图片）。clipboard crate 生产代码不写 notes（只读检查 image 引用），剪贴板存笔记的真实入口是前端 IPC `create_note`（Task 6 已透传 type）。若照原 step 改 type=text 会清空 content_html、破坏 image 引用测试，且违反架构边界（note_commands.rs:2 明示 notepad 不依赖 clipboard，反向亦不应依赖）。`:976` 测试 INSERT 显式列名 + `type` DEFAULT 'html' 已兼容新 schema，clipboard 测试通过（`test_delete_by_transcription_ids` 的 `clipboard_history.id` 失败为 pre-existing flaky，main 同样失败，与本次无关）。

**Files:**
- Modify: `crates/clipboard/src/store.rs`（约 :976）
- Modify: `crates/clipboard/Cargo.toml`（加 octopus-notepad 依赖，若未有）

- [x] **Step 1: 读现状** — 读 `clipboard/src/store.rs:960-990` 确认当前 INSERT notes 的完整上下文（标题来源、source 值、是否事务内）。

- [x] **Step 2: 改调 notepad 统一入口** — 把直写 SQL 的 INSERT 替换为：

```rust
use octopus_notepad::{NoteSource, NoteType};
// ...
let id = octopus_notepad::store::create_note_at(
    conn,
    NoteSource::Clipboard,
    None,
    &text,            // 剪贴板纯文本
    NoteType::Text,   // 剪贴板来源固定纯文本
)?;
```

> 保留原函数的 conn 传递（若原代码在 `with_db` 闭包内，调 `create_note_at(conn, ...)` 即可）。删除原 `extract_text` 调用（notepad 内部按 type=text 直存，无需抽取）。

- [x] **Step 3: Cargo.toml 加依赖** — 若 `crates/clipboard/Cargo.toml` 未列 `octopus-notepad`，加 `octopus-notepad = { path = "../notepad" }`。

- [x] **Step 4: 编译 + 测试** — `cargo build --manifest-path crates/clipboard/Cargo.toml -p octopus-clipboard` 通过；跑 clipboard 现有测试不破坏。

- [x] **Step 5: 提交** — `git add crates/clipboard/src/store.rs crates/clipboard/Cargo.toml && git commit -m "refactor(clipboard): 剪贴板存笔记改调 notepad create_note_at (type=text 统一入口)"`

---

## Task 6: IPC `note_commands.rs` 透传 type

**Files:**
- Modify: `crates/desktop/src/note_commands.rs`

- [x] **Step 1: create_note / update_note 命令加参数** —

```rust
use octopus_notepad::{Note, NoteFilter, NoteSource, NoteType};

#[tauri::command]
pub async fn create_note(
    source: String,
    source_ref_id: Option<i64>,
    body: String,
    note_type: String,
    app_handle: tauri::AppHandle,
) -> Result<i64, String> {
    let id = octopus_infra::db::with_db(|conn| {
        octopus_notepad::store::create_note_at(
            conn,
            NoteSource::from_str(&source),
            source_ref_id,
            &body,
            NoteType::from_str(&note_type),
        )
    })
    .map_err(|e| e.to_string())?;
    let _ = app_handle.emit("notepad://changed", ());
    Ok(id)
}

#[tauri::command]
pub async fn update_note(
    id: i64,
    title: String,
    body: String,
    note_type: String,
    app_handle: tauri::AppHandle,
) -> Result<(), String> {
    octopus_infra::db::with_db(|conn| {
        octopus_notepad::store::update_note_at(conn, id, &title, &body, NoteType::from_str(&note_type))
    })
    .map_err(|e| e.to_string())?;
    let _ = app_handle.emit("notepad://changed", ());
    Ok(())
}
```

- [x] **Step 2: save_transcription_to_note / save_ocr_to_note 固定 type=text** — 找到这两个命令内调 `create_note_at` / `<p>` 包裹处，改为传 `NoteType::Text`（ASR/OCR 是纯文本来源，不再 `<p>` 包裹成 html）：

```rust
// 原：let html = format!("<p>{}</p>", text); create_note_at(conn, Asr, id, &html)
// 改：
octopus_notepad::store::create_note_at(conn, NoteSource::Asr, transcription_id, &text, NoteType::Text)?;
// OCR 同理：NoteSource::Ocr, NoteType::Text
```

- [x] **Step 3: 检查 invoke_handler 注册** — create_note/update_note 签名变了，但命令名不变，`invoke_handler!` 注册处无需改（参数由前端按名传）。

- [x] **Step 4: 编译** — `cargo build --manifest-path crates/desktop/Cargo.toml -p octopus-desktop` 通过。

- [x] **Step 5: 提交** — `git add crates/desktop/src/note_commands.rs && git commit -m "feat(desktop): create/update_note IPC 透传 type；ASR/OCR 存入固定 type=text"`

---

## Task 7: 后端整体编译 + 测试收口

- [x] **Step 1: workspace 编译** — `cargo build --manifest-path Cargo.toml` 全 workspace 通过。

- [x] **Step 2: workspace 测试** — `cargo test --manifest-path Cargo.toml -p octopus-notepad -p octopus-infra -p octopus-clipboard` 全 PASS。

- [x] **Step 3: 修复回归** — 若其他 crate 因 Note 字段/签名变更编译失败（如 capx/server 引用 Note），按编译错误逐一适配（grep `create_note`/`update_note`/`Note {` 调用点）。

- [x] **Step 4: 提交（若有修复）** — `git add -A && git commit -m "fix: 适配 NoteType 透传的下游调用点"`

---

## Task 8: 前端类型 + IPC 封装

**Files:**
- Modify: `crates/desktop/frontend/src/types/note.ts`
- Modify: `crates/desktop/frontend/src/lib/notepad.ts`

- [x] **Step 1: types/note.ts 加 NoteType** —

```ts
export type NoteSource = "asr" | "ocr" | "clipboard" | "manual";
export type NoteType = "html" | "text" | "markdown";

export interface Note {
  id: number;
  title: string | null;
  content_text: string;
  content_html: string;
  note_type: NoteType;
  source: NoteSource;
  source_ref_id: number | null;
  is_pinned: boolean;
  is_favorite: boolean;
  created_at: string;
  updated_at: string;
}
// NoteListParams 不变
```

- [x] **Step 2: lib/notepad.ts create/update 加 noteType** —

```ts
import type { Note, NoteListParams, NoteSource, NoteType } from "@/types/note";

export const createNote = (
  source: NoteSource,
  sourceRefId: number | null,
  body: string,
  noteType: NoteType,
) => invoke<number>("create_note", { source, sourceRefId, body, noteType });

export const updateNote = (
  id: number,
  title: string,
  body: string,
  noteType: NoteType,
) => invoke<void>("update_note", { id, title, body, noteType });
```

> 其余导出（list/count/get/delete/pin/favorite/export/import/image）不变。

- [x] **Step 3: 提交** — `git add crates/desktop/frontend/src/types/note.ts crates/desktop/frontend/src/lib/notepad.ts && git commit -m "feat(frontend): NoteType 类型 + createNote/updateNote 透传 noteType"`

---

## Task 9: `MarkdownEditor` 组件 + marked 依赖

**Files:**
- Create: `crates/desktop/frontend/src/pages/Notepad/MarkdownEditor.tsx`
- Modify: `crates/desktop/frontend/package.json`

- [x] **Step 1: 加 marked 依赖** — `cd crates/desktop/frontend && npm install marked`，确认 `package.json` 出现 `"marked"`。

- [x] **Step 2: 创建 MarkdownEditor.tsx** —

```tsx
import { useState, useMemo } from "react";
import { marked } from "marked";
import {
  Bold, Italic, Heading1, List, Code, Link as LinkIcon, Quote,
} from "lucide-react";

interface Props {
  value: string;
  onChange: (md: string) => void;
}

/** markdown 编辑器：左源码 textarea + 工具栏，右可折叠预览（marked 渲染）。 */
export default function MarkdownEditor({ value, onChange }: Props) {
  const [showPreview, setShowPreview] = useState(true);

  const html = useMemo(() => marked.parse(value || "", { async: false }) as string, [value]);

  // 在 textarea 选区/光标处插入语法
  const wrap = (before: string, after: string = before) => {
    const ta = document.getElementById("md-textarea") as HTMLTextAreaElement | null;
    if (!ta) return;
    const { selectionStart: s, selectionEnd: e } = ta;
    const sel = value.slice(s, e);
    const next = value.slice(0, s) + before + sel + after + value.slice(e);
    onChange(next);
    requestAnimationFrame(() => {
      ta.focus();
      ta.selectionStart = s + before.length;
      ta.selectionEnd = e + before.length;
    });
  };

  const linePrefix = (prefix: string) => {
    const ta = document.getElementById("md-textarea") as HTMLTextAreaElement | null;
    if (!ta) return;
    const s = ta.selectionStart;
    const lineStart = value.lastIndexOf("\n", s - 1) + 1;
    const next = value.slice(0, lineStart) + prefix + value.slice(lineStart);
    onChange(next);
  };

  const tools = [
    { icon: Heading1, title: "标题", onClick: () => linePrefix("# ") },
    { icon: Bold, title: "粗体", onClick: () => wrap("**") },
    { icon: Italic, title: "斜体", onClick: () => wrap("*") },
    { icon: List, title: "列表", onClick: () => linePrefix("- ") },
    { icon: Quote, title: "引用", onClick: () => linePrefix("> ") },
    { icon: Code, title: "代码", onClick: () => wrap("`") },
    { icon: LinkIcon, title: "链接", onClick: () => {
        const url = prompt("链接 URL"); if (url) wrap("[", `](${url})`); } },
  ];

  return (
    <div className="flex-1 flex flex-col">
      <div className="flex items-center gap-0.5 px-2 py-1 border-b border-border">
        {tools.map(({ icon: Icon, title, onClick }, i) => (
          <button key={i} title={title} onClick={onClick}
            className="p-1 rounded hover:bg-accent text-muted-foreground hover:text-foreground">
            <Icon className="w-4 h-4" />
          </button>
        ))}
        <button onClick={() => setShowPreview((v) => !v)}
          className="ml-auto px-2 py-1 text-xs rounded hover:bg-accent text-muted-foreground">
          {showPreview ? "隐藏预览" : "显示预览"}
        </button>
      </div>
      <div className={`flex-1 flex ${showPreview ? "flex-row" : "flex-col"} overflow-hidden`}>
        <textarea
          id="md-textarea"
          value={value}
          onChange={(e) => onChange(e.target.value)}
          className={`flex-1 p-4 font-mono text-sm bg-transparent resize-none focus:outline-none border-0 ${showPreview ? "border-r border-border" : ""}`}
          placeholder="输入 markdown..."
        />
        {showPreview && (
          <div className="flex-1 overflow-y-auto px-4 py-2 prose prose-sm max-w-none"
               dangerouslySetInnerHTML={{ __html: html }} />
        )}
      </div>
    </div>
  );
}
```

- [x] **Step 3: 提交** — `git add crates/desktop/frontend/src/pages/Notepad/MarkdownEditor.tsx crates/desktop/frontend/package.json crates/desktop/frontend/package-lock.json && git commit -m "feat(frontend): MarkdownEditor 组件（源码+工具栏+marked 可折叠预览）"`

---

## Task 10: `NoteEditor` 按 type 分发编辑器

**Files:**
- Modify: `crates/desktop/frontend/src/pages/Notepad/NoteEditor.tsx`

- [x] **Step 1: doSave 透传 type** — 把 `doSave` 改为按当前 note 的 type 调用：

```tsx
  const doSave = useCallback(
    (body: string) => {
      const id = currentId.current;
      if (id == null || !note) return;
      updateNote(id, title, body, note.note_type).catch(console.error);
    },
    [title, note],
  );
```

标题 debounce 保存同理：`updateNote(noteId, title, editor.getHTML(), note.note_type)`。

- [x] **Step 2: 编辑区分发** — 在 return 的编辑区（`{/* 编辑器 */}` 处）按 `note.note_type` 分发。保留现有 TipTap 工具栏仅在 html 时显示；text/markdown 用各自编辑器：

```tsx
      {/* 编辑区：按 type 分发 */}
      <div className="flex-1 overflow-hidden flex flex-col">
        {note.note_type === "html" && (
          <>
            {/* 现有 TipTap 工具栏 + EditorContent 保持原样 */}
            <div className="flex items-center gap-0.5 px-2 py-1 border-b border-border flex-wrap">
              {tools.map(({ icon: Icon, title, onClick }, i) => ( /* ... 原样 ... */ ))}
              <div className="ml-auto flex items-center gap-0.5">{/* 导入/导出/收藏/置顶 原样 */}</div>
            </div>
            <input /* 标题 input 原样 */ />
            <div className="flex-1 overflow-y-auto px-4 pb-4">
              <div className="prose prose-sm max-w-none [&_img]:max-w-full">
                <EditorContent editor={editor} />
              </div>
            </div>
          </>
        )}
        {note.note_type === "text" && (
          <TextEditor note={note} title={title} onTitle={setTitle} onSave={doSave} />
        )}
        {note.note_type === "markdown" && (
          <MarkdownEditorOuter note={note} title={title} onTitle={setTitle} onSave={doSave} />
        )}
      </div>
```

> 收藏/置顶/导出按钮在 text/markdown 也需要：抽出公共 Header 组件或在每个分支重复。为控制 scope，建议把标题 + 收藏/置顶/导出抽成 `NoteHeader` 子组件（接收 note + setters），三种编辑器共用。实现时按现有结构重构。

- [x] **Step 3: 内联 TextEditor（纯 textarea）** — 在 NoteEditor.tsx 内或新建 `TextEditor.tsx`：

```tsx
function TextEditor({ note, title, onTitle, onSave }: EditorProps) {
  const [text, setText] = useState(note.content_text);
  const timer = useRef<ReturnType<typeof setTimeout> | null>(null);
  useEffect(() => setText(note.content_text), [note.id]);  // 切换笔记重置
  const onChange = (v: string) => {
    setText(v);
    if (timer.current) clearTimeout(timer.current);
    timer.current = setTimeout(() => onSave(v), 800);
  };
  return (
    <>
      <NoteHeader note={note} title={title} onTitle={onTitle} />
      <textarea
        value={text}
        onChange={(e) => onChange(e.target.value)}
        className="flex-1 p-4 font-mono text-sm bg-transparent resize-none focus:outline-none border-0"
        placeholder="输入纯文本..."
      />
    </>
  );
}
```

- [x] **Step 4: MarkdownEditor 接入（外层包 debounce + header）** —

```tsx
function MarkdownEditorOuter({ note, title, onTitle, onSave }: EditorProps) {
  const [md, setMd] = useState(note.content_text);
  const timer = useRef<ReturnType<typeof setTimeout> | null>(null);
  useEffect(() => setMd(note.content_text), [note.id]);
  const onChange = (v: string) => {
    setMd(v);
    if (timer.current) clearTimeout(timer.current);
    timer.current = setTimeout(() => onSave(v), 800);
  };
  return (
    <>
      <NoteHeader note={note} title={title} onTitle={onTitle} />
      <MarkdownEditor value={md} onChange={onChange} />
    </>
  );
}
```

> `EditorProps = { note: Note; title: string; onTitle: (t: string) => void; onSave: (body: string) => void }`。`NoteHeader` 抽出标题 input + 收藏/置顶/导出按钮（从现有 html 分支搬出，三种复用）。

- [x] **Step 5: 类型检查 + 构建** — `cd crates/desktop/frontend && npm run build` → 通过（dist 产出）。修复 TS 报错（如 doSave 依赖、未用 import）。

- [x] **Step 6: 提交** — `git add crates/desktop/frontend/src/pages/Notepad/ && git commit -m "feat(frontend): NoteEditor 按 note_type 分发 html/text/markdown 编辑器"`

---

## Task 11: 新建笔记 type 选择 UX

**Files:**
- Modify: `crates/desktop/frontend/src/pages/Notepad/NoteList.tsx`（新建按钮处）

- [x] **Step 1: 读 NoteList 新建逻辑** — 确认新建按钮当前如何调 `createNote`（默认 source=manual）。

- [x] **Step 2: 新建按钮加 type 选择** — 新建时默认 `html`（与现状一致），并提供 type 切换。实现为：新建按钮旁加一个 type 下拉（`select`），或新建按钮改为弹出三个选项（富文本/纯文本/Markdown）。推荐下拉：

```tsx
const [newType, setNewType] = useState<NoteType>("html");

const handleCreate = async () => {
  const id = await createNote("manual", null, "", newType);
  onSelect(id);
};

// UI：新建按钮 + type 下拉并排
<div className="flex gap-1">
  <select value={newType} onChange={(e) => setNewType(e.target.value as NoteType)}
          className="text-xs border border-border rounded px-1">
    <option value="html">富文本</option>
    <option value="text">纯文本</option>
    <option value="markdown">Markdown</option>
  </select>
  <button onClick={handleCreate}>新建</button>
</div>
```

> 已建笔记 type 锁定：编辑器内不提供 type 切换（NoteEditor 只读 `note.note_type`）。新建空笔记 body="" 按 type 存（html 空、text 空、md 空）。

- [x] **Step 3: 构建检查** — `npm run build` 通过。

- [x] **Step 4: 提交** — `git add crates/desktop/frontend/src/pages/Notepad/NoteList.tsx && git commit -m "feat(frontend): 新建笔记可选 type（富文本/纯文本/Markdown），已建锁定"`

---

## Task 12: 列表 type 标记

**Files:**
- Modify: `crates/desktop/frontend/src/pages/Notepad/NoteList.tsx`

- [x] **Step 1: 列表项加 type 角标** — 在每条笔记标题旁，非 html 类型显示小标记：

```tsx
{note.note_type === "markdown" && <span className="text-[10px] text-blue-500">MD</span>}
{note.note_type === "text" && <span className="text-[10px] text-muted-foreground">TXT</span>}
{/* html 不标（默认） */}
```

- [x] **Step 2: 构建检查** — `npm run build` 通过。

- [x] **Step 3: 提交** — `git add crates/desktop/frontend/src/pages/Notepad/NoteList.tsx && git commit -m "feat(frontend): 列表项显示 md/txt 类型标记"`

---

## Task 13: dist rebuild + 提交（历史留档：dist 已移出 git）

> main 后续已把 `crates/desktop/dist/` 移出 git 跟踪（commit `f543511`，加入 `.gitignore`）。本 task 当时的「rebuild + 提交 dist」不再适用——前端构建产物不再入库；富文本移除时已 `git rm --cached` 让残留 dist 退出跟踪。

- [x] **Step 1: 完整 rebuild** — `cd crates/desktop/frontend && npm run build`，确认 `crates/desktop/dist/assets/*` 产出新 hash 文件。

- [x] **Step 2: 提交 dist** — `git add crates/desktop/dist/ && git commit -m "chore: rebuild dist（notepad type 三类型编辑器）"`

---

## Task 14: e2e 验证（✅ 2026-07-02 通过，已合并 main）

- [x] **迁移验证** — 真实库启动：v11→v12 迁移删除历史 4 条 html 笔记无崩溃；记事本正常打开；`SELECT type FROM notes` 仅剩 text/markdown。
- [x] **双类型新建/编辑** — 新建纯文本 / Markdown 笔记 → 对应编辑器渲染 → 输入 → 800ms 自动保存 → 重开内容正确。
- [x] **type 锁定** — 已建笔记编辑器无 type 切换入口，锁定生效。
- [x] **搜索** — text/markdown 内容可被 FTS 命中。
- [x] **来源存入** — 剪贴板 / ASR / OCR 存入为纯文本（无 `<p>` 包裹），type='text'。
- [x] **markdown 预览** — `# 标题` / `**粗体**` 预览正确，折叠/展开正常。

---

## Spec Coverage

> 以重写后的 spec（双类型最终设计）章节为准。

| Spec section（双类型最终设计） | Task |
|--------------|------|
| §2 Schema（content_text + content_html 保留恒空 + type DEFAULT 'text'） | Task 2 |
| §3 迁移链 v9→v10→v11→v12 | Task 3（v9→v10）+ 富文本移除（v11→v12） |
| §4.1 NoteType（text/markdown） | Task 1 + 富文本移除（去 Html） |
| §4.2 Note.note_type 字段 | Task 1 |
| §4.3 store（split_body 恒空，无抽取） | Task 4 + 富文本移除（简化 split_body、删 serialize.rs） |
| §4.4 IPC 透传 type + 删图片桥接 | Task 6 + 富文本移除（删 get/insert_note_image） |
| §5 前端双类型（textarea / MarkdownEditor，无 TipTap） | Task 8, 9, 10 + 富文本移除（删 extensions.tsx / TipTap） |
| §6 type 选择 UX（已建锁定）+ 列表角标 | Task 11, 12 |
| §7 测试（NoteType / 迁移 v11→v12 / store） | Task 1, 3, 4 + 富文本移除迁移测试 |

