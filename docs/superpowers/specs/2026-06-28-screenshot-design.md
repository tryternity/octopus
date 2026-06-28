# 屏幕截图功能设计

**日期**: 2026-06-28
**状态**: ✅ 一期已实现（基础截图 + 框选 + 手柄调整 + 确认 → 剪贴板历史）
**分支**: `feature/clipboard-research`（worktree: `.worktrees/clipboard-research`）

## 0. 概述

为 octopus 新增屏幕截图能力。一期实现基础截图：全局快捷键/托盘菜单触发 → 全屏遮罩 → 鼠标框选 + 8 手柄调整 + 拖拽平移 → Enter 确认 → 自动进剪贴板历史。基于 xcap（跨平台截图引擎）+ Tauri 全屏透明窗口 + React Canvas 选区 UI。

独立 crate `octopus-capx`（目录 `crates/capx/`），封装 xcap 截图 + 裁剪。截图结果作为图片条目进入剪贴板历史，可 OCR / 保存 / 收藏 / 删除。

二期加标注工具栏（矩形/箭头/文字），三期加滚动截图。

## 1. 架构

### 1.1 crate 结构

```
crates/
├── capx/               # octopus-capx — 新增，依赖 infra
│   ├── Cargo.toml      # xcap (path 引用本地), image
│   └── src/
│       ├── lib.rs      # pub use
│       └── capture.rs  # 截图核心：截全屏 → 裁剪选区
└── desktop/            # Tauri 命令 + 前端
    ├── src/
    │   ├── screenshot_commands.rs  # start/confirm/cancel 截图命令
    │   └── screenshot_window.rs    # 全屏透明窗口管理
    └── frontend/src/pages/
        └── Screenshot/index.tsx    # 选区 Canvas UI
```

**依赖关系**：`infra ← capx ← desktop`

### 1.2 为什么直接引用 xcap

xcap 是纯粹的截图引擎（跨平台底层 API 封装），只负责"把屏幕拍下来返回图片"。我们需要的所有功能（框选/裁剪/标注/滚动拼接）都不需要改 xcap：

| 功能 | 实现位置 | 改 xcap？ |
|---|---|---|
| 截全屏 | xcap `Monitor::capture_image()` | 否 |
| 框选矩形 | 前端 Canvas + 鼠标事件 | 否 |
| 裁剪选区 | `image` crate 裁剪 | 否 |
| 标注工具栏（二期） | 前端 Canvas 绘制 | 否 |
| 滚动截图（三期） | 多次调 xcap 截图 → 像素匹配拼接 | 否 |

依赖方式：`xcap = { path = "../../xcap" }`（本地路径引用）。

### 1.3 capture.rs 核心接口

```rust
pub struct ScreenCapture {
    pub png_bytes: Vec<u8>,   // 全屏 PNG
    pub width: u32,
    pub height: u32,
}

/// 截取主显示器全屏
pub fn capture_full_screen() -> Result<ScreenCapture>;

/// 从全屏图中裁剪指定矩形区域（物理像素坐标）
pub fn crop_region(full: &ScreenCapture, x: u32, y: u32, w: u32, h: u32) -> Result<Vec<u8>>;
```

## 2. 截图触发流程

```
用户按快捷键 / 点托盘菜单「截图」
         │
         ▼
┌─────────────────────────────────────┐
│  1. capx::capture_full_screen()     │  xcap 截全屏 → PNG bytes
│     返回 (png_bytes, width, height) │
├─────────────────────────────────────┤
│  2. 创建全屏透明窗口（screenshot）   │  无边框、置顶、透明
│     窗口大小 = 屏幕尺寸              │
├─────────────────────────────────────┤
│  3. emit("screenshot://ready", {    │  传 PNG base64 + 尺寸给前端
│       image, width, height })       │
├─────────────────────────────────────┤
│  4. 前端 Canvas 渲染全屏图           │  作为背景，整体加暗色遮罩
├─────────────────────────────────────┤
│  5. 用户鼠标拖拽框选                 │  mousedown → mousemove → mouseup
│     Canvas 实时更新选区              │  选区内亮 + 选区外暗 + 边框
├─────────────────────────────────────┤
│  6. 8 手柄调整 + 拖拽平移            │  角点/边中点 resize + 内部 move
├─────────────────────────────────────┤
│  7. ESC/右键 取消 → 关窗口           │
│     Enter 确认 → invoke 截图确认     │
├─────────────────────────────────────┤
│  8. 后端裁剪选区                     │  从全屏 PNG 裁剪 (x,y,w,h)
│     → capx::crop_region()            │
├─────────────────────────────────────┤
│  9. 写入剪贴板历史                   │  手动编码 WebP BLOB → DB
│     + 写系统剪贴板                    │  + write_image (suppress flag)
├─────────────────────────────────────┤
│  10. 关闭截图窗口                     │
└─────────────────────────────────────┘
```

### 2.1 选区交互状态机

```
idle（等待框选）→ selecting（拖拽中）→ selected（已确定，可调整）
                                           ↓
                                    resize（拖拽手柄/移动选区）
                                           ↓
                                    selected ←────┘
                                           ↓
                                    Enter → 确认截图
```

- `idle`：鼠标点击任意位置 → 进入 `selecting`
- `selecting`：鼠标拖拽实时更新选区 → mouseup 进入 `selected`
- `selected`：8 手柄可见，鼠标按手柄进入 `resize`，按选区内进入 `move`，按选区外重新 `selecting`
- `resize`/`move`：实时更新 → mouseup 回到 `selected`
- 最小选区 10×10，不超出屏幕边界

### 2.2 选区调整手柄

```
拖拽选区 → mouseup 确定初始选区
         │
         ▼
┌───────────────────────────┐
│  选区四角 + 四边中点显示    │
│  8 个拖拽手柄（小方块）     │
├───────────────────────────┤
│  鼠标移到边框 → 双向箭头   │  cursor: ew-resize / ns-resize
│  鼠标移到角点 → 斜向箭头   │  cursor: nwse-resize / nesw-resize
│  鼠标移到选区内 → 可拖动   │  cursor: move（拖动整个选区位置）
├───────────────────────────┤
│  拖拽手柄 → 实时更新选区   │  最小 10×10
│  拖拽选区内部 → 平移选区   │  不超出屏幕边界
├───────────────────────────┤
│  Enter 确认 / ESC 取消     │
└───────────────────────────┘
```

## 3. 前端选区 Canvas 设计

### 3.1 双层 Canvas

- **底层 Canvas**：全屏原图（xcap 截图）全尺寸渲染
- **上层 Canvas**：遮罩 + 选区框 + 8 手柄（`clearRect` 挖出选区）

```
┌──────────────────────────────────────────────────┐
│  ████████████████████████████████████████████████ │  ← 暗遮罩（选区外）
│  ████████████┌─────────────────────┐████████████ │
│  ████████████│                     │████████████ │
│  ████████████│    选区（清晰）      │████████████ │
│  ████████████│              1280×720│████████████ │  ← 尺寸标注
│  ████████████└─────────────────────┘████████████ │
│  ████████████████████████████████████████████████ │
└──────────────────────────────────────────────────┘
```

### 3.2 选区坐标

```typescript
interface Selection {
  x: number; y: number;   // 左上角
  w: number; h: number;   // 宽高
}
```

归一化处理——支持任意方向拖拽（右下→左上时自动 min/max 换算）。

### 3.3 鼠标状态判定（按优先级）

1. 点在手柄上 → `resize`（按手柄方向调整）
2. 点在选区内 → `move`（平移选区）
3. 点在选区外 → 重新框选（清空旧选区，开始新的 `selecting`）

### 3.4 尺寸标注

选区右下角实时显示像素尺寸（如 `1280 × 720`），半透明白底黑字。

### 3.5 全局快捷键（截图窗口内）

- `Enter` → 确认，调 `invoke("confirm_screenshot", { x, y, w, h })`
- `Esc` / 右键 → 取消，调 `invoke("cancel_screenshot")`

### 3.6 Retina/HiDPI 适配

- xcap 截图尺寸 = 物理像素（如 2880×1800）
- 前端 Canvas / 鼠标坐标 = CSS 像素（如 1440×900）
- 前端按 `devicePixelRatio` 自行换算后传物理坐标给后端（后端无需感知缩放）

## 4. 数据流与存储

### 4.1 截图结果处理（手动写入剪贴板历史）

不走 watcher 自动捕获（避免重复截屏 + 截到截图窗口本身）：

1. 后端从全屏 PNG 裁剪选区 → `capx::crop_region()` → PNG bytes
2. `encode_and_hash()` → SHA-256 去重
3. `encode_to_webp()` → 无损 + 缩略图
4. `insert_image_data()` + `insert_clipboard_item()`（source=clipboard, item_type=image）
5. `ClipboardHandle::write_image()`（设置 suppress flag）

### 4.2 不变量

- 截图在剪贴板历史中表现为普通图片条目（可 OCR / 可保存 / 收藏 / 删除）
- `content` = blob_hash（与 watcher 产生的一致）
- `source` = `clipboard`（截图不是 ASR）

### 4.3 截图配置

```sql
INSERT OR IGNORE INTO app_config (config_key, config_value, description) VALUES
  ('screenshot_shortcut', 'Alt+S', '截图快捷键');
```

AppConfig 新增 `screenshot_shortcut` 字段，设置页快捷键 section 新增「截图」行（ShortcutButton 热重载，与 ASR/剪贴板快捷键一致）。

## 5. Tauri 命令与窗口管理

### 5.1 命令

```rust
/// 启动截图：截全屏 → 创建截图窗口 → emit 图片给前端
#[tauri::command]
pub async fn start_screenshot(app_handle: tauri::AppHandle) -> Result<(), String>;

/// 确认截图：从全屏图裁剪选区 → 写剪贴板历史 → 关窗口
#[tauri::command]
pub async fn confirm_screenshot(
    x: u32, y: u32, w: u32, h: u32,
    app_handle: tauri::AppHandle,
) -> Result<(), String>;

/// 取消截图：关窗口
#[tauri::command]
pub async fn cancel_screenshot(app_handle: tauri::AppHandle) -> Result<(), String>;
```

### 5.2 截图窗口属性

```json
{
  "label": "screenshot_window",
  "title": "",
  "fullscreen": true,
  "decorations": false,
  "alwaysOnTop": true,
  "skipTaskbar": true,
  "resizable": false,
  "transparent": true
}
```

### 5.3 start_screenshot 流程

1. `capx::capture_full_screen()` → 截全屏 PNG + 尺寸
2. 创建截图窗口（如已存在则 destroy 重建）
3. 暂存全屏 PNG 到静态变量 `SCREENSHOT_DATA: Mutex<Option<ScreenCapture>>`
4. 窗口 ready 后 emit `screenshot://ready`（PNG base64 + 尺寸）
5. 前端收到后渲染 Canvas

### 5.4 confirm_screenshot 流程

1. 从 `SCREENSHOT_DATA` 取全屏 PNG
2. `capx::crop_region(full, x, y, w, h)` → 选区 PNG
3. `encode_and_hash` → SHA-256 去重
4. `encode_to_webp` → 无损 + 缩略图
5. `insert_image_data` + `insert_clipboard_item`
6. `ClipboardHandle::write_image`（设置 suppress flag）
7. 关闭截图窗口 + 清空 `SCREENSHOT_DATA`

### 5.5 快捷键注册

main.rs setup 从 config 读 `screenshot_shortcut` 注册全局快捷键。`set_config` 中热重载（与 `clipboard_shortcut` 一致：unregister 旧 + register 新）。

## 6. 错误处理与边界

| 场景 | 处理 |
|---|---|
| macOS 屏幕录制权限未授权 | xcap 返回空/黑屏 → toast「请授予屏幕录制权限」 |
| 多显示器 | ✅ 已实现：每个显示器独立窗口（screenshot_window / screenshot_window_N），用 Tauri monitor API 获取逻辑坐标 + 尺寸，鼠标在哪屏截哪屏 |
| 选区太小（< 10×10） | Enter 无效，不确认 |
| 选区超出屏幕边界 | clamp 到屏幕尺寸内 |
| Retina/HiDPI 缩放 | 前端按 `devicePixelRatio` 换算后传物理坐标 |
| 截图窗口被遮挡 | `alwaysOnTop: true` + 创建后 `set_focus()` |
| 连续触发截图 | 检测窗口已存在 → 先 destroy 重建 |
| 应用崩溃 | `SCREENSHOT_DATA` 内存态，重启自动清空 |
| 快捷键与系统冲突 | `check_shortcut` 冲突检测（与 ASR/剪贴板一致） |

**并发安全**：`SCREENSHOT_DATA` 用 `Mutex<Option<ScreenCapture>>`，互斥保护。

**降级**：截图失败不影响 octopus 其他功能。start_screenshot 返回 Err → 快捷键/托盘回调静默忽略 + 记 error 日志。

## 7. 依赖变更

**新增（Rust）**：
- `xcap = { path = "../../xcap" }`（截图引擎，本地路径引用）
- `image = "0.25"`（裁剪/编码，已有）

**新增（前端）**：无额外依赖（React Canvas 原生 API）。

## 8. 实施分期

| 阶段 | 范围 | 依赖 |
|---|---|---|
| **一期** | 基础截图：xcap 截主屏 + Canvas 框选 + 8 手柄调整 + Enter 确认 → 剪贴板历史 | 无 |
| **1.1 期** | 多显示器支持：每个显示器独立窗口，鼠标在哪屏截哪屏 | ✅ 已实现 |
| **二期** | 标注工具栏（矩形/箭头/文字/马赛克），选区内 Canvas 绘制 | 一期 |
| **三期** | 滚动截图（自动滚动 + 逐行像素匹配拼接） | 一期 |

## 9. 风险

| 风险 | 概率 | 影响 | 缓解 |
|---|---|---|---|
| macOS 屏幕录制权限拒绝 | 中 | 无法截图 | 首次弹窗引导 + toast 提示 |
| xcap Linux Wayland 不完善 | 中 | Linux 截图失效 | 检测 Wayland → fallback `xdg-desktop-portal` 截图 |
| Retina 坐标偏移导致裁剪错位 | 高 | 选区内容偏移 | 前端统一按 `devicePixelRatio` 换算 + 测试验证 |
| 全屏透明窗口在部分 WM 闪烁 | 低 | UX 差 | 测试 macOS/Windows/Linux 三端 |
| xcap 本地路径引用在 CI 环境 | 低 | 构建失败 | CI clone 时包含 xcap 仓库或改为 git 依赖 |
