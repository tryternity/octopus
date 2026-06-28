# 屏幕截图功能实施计划（一期）

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development or superpowers:executing-plans.

**Goal:** 实现主屏截图：快捷键/托盘触发 → 全屏遮罩 → 鼠标框选 + 8 手柄调整 → Enter 确认 → 进剪贴板历史

**Architecture:** 独立 crate `octopus-capx`（封装 xcap 截图引擎）+ Tauri 全屏透明窗口 + React Canvas 选区 UI。截图结果手动写入剪贴板历史（绕过 watcher）。

**Tech Stack:** Rust + xcap 0.9.6（本地路径引用）+ image 0.25 + Tauri + React Canvas

**Spec:** `docs/superpowers/specs/2026-06-28-screenshot-design.md`

---

## 文件结构

| 文件 | 变更 | 责任 |
|---|---|---|
| `crates/capx/Cargo.toml` | Create | crate 清单 |
| `crates/capx/src/lib.rs` | Create | 模块入口 |
| `crates/capx/src/capture.rs` | Create | 截全屏 + 裁剪选区 |
| `Cargo.toml` | Modify | workspace members 加 capx |
| `crates/clipboard/src/handle.rs` | Modify | 新增 write_image 方法 |
| `crates/infra/src/config.rs` | Modify | AppConfig 新增 screenshot_shortcut |
| `crates/infra/src/db.rs` | Modify | save/load_app_config 补 screenshot_shortcut |
| `crates/infra/src/db.sql` | Modify | app_config seed screenshot_shortcut |
| `crates/desktop/Cargo.toml` | Modify | 新增 octopus-capx 依赖 |
| `crates/desktop/src/screenshot_commands.rs` | Create | start/confirm/cancel 命令 |
| `crates/desktop/src/main.rs` | Modify | 注册命令 + 快捷键 + 托盘菜单 |
| `crates/desktop/src/settings_commands.rs` | Modify | apply_config_value + 热重载 |
| `crates/desktop/src/tray.rs` | Modify | 托盘菜单加「截图」 |
| `crates/desktop/frontend/src/pages/Screenshot/index.tsx` | Create | 选区 Canvas UI |
| `crates/desktop/frontend/src/main.tsx` | Modify | 路由加 screenshot 页面 |

---

### Task 1: octopus-capx crate

**Files:**
- Create: `crates/capx/Cargo.toml`
- Create: `crates/capx/src/lib.rs`
- Create: `crates/capx/src/capture.rs`
- Modify: `Cargo.toml`（workspace root）

- [ ] **Step 1: 创建 crate**

```bash
mkdir -p crates/capx/src
```

`crates/capx/Cargo.toml`:
```toml
[package]
name = "octopus-capx"
version = "0.1.0"
edition = "2021"

[dependencies]
xcap = { path = "../../xcap" }
image = "0.25"
anyhow = "1"
log = "0.4"
```

`crates/capx/src/lib.rs`:
```rust
pub mod capture;
```

- [ ] **Step 2: 实现 capture.rs**

```rust
use anyhow::{Context, Result};
use xcap::Monitor;

pub struct ScreenCapture {
    pub rgba_bytes: Vec<u8>,
    pub width: u32,
    pub height: u32,
}

/// 截取主显示器全屏（返回 RGBA 像素 + 尺寸）。
/// 主显示器 = 包含鼠标当前位置的显示器。
pub fn capture_full_screen() -> Result<ScreenCapture> {
    let monitors = Monitor::all().context("Failed to list monitors")?;
    let monitor = monitors
        .into_iter()
        .next()
        .context("No monitor found")?;

    let img = monitor
        .capture_image()
        .context("Failed to capture screen")?;

    let width = img.width();
    let height = img.height();
    let rgba_bytes = img.into_raw();

    log::info!(
        "Screen captured: {}x{} ({}KB RGBA)",
        width,
        height,
        rgba_bytes.len() / 1024
    );

    Ok(ScreenCapture {
        rgba_bytes,
        width,
        height,
    })
}

/// 从全屏 RGBA 中裁剪矩形区域，返回 PNG bytes。
/// 坐标为物理像素。
pub fn crop_region(
    full: &ScreenCapture,
    x: u32,
    y: u32,
    w: u32,
    h: u32,
) -> Result<Vec<u8>> {
    let img = ::image::RgbaImage::from_raw(full.width, full.height, full.rgba_bytes.clone())
        .context("Failed to create RgbaImage from full screen")?;

    // clamp
    let x = x.min(full.width.saturating_sub(1));
    let y = y.min(full.height.saturating_sub(1));
    let w = w.min(full.width - x);
    let h = h.min(full.height - y);

    let cropped = ::image::imageops::crop_imm(&img, x, y, w, h).to_image();

    let mut png_bytes = Vec::new();
    cropped
        .write_to(&mut std::io::Cursor::new(&mut png_bytes), ::image::ImageFormat::Png)
        .context("Failed to encode cropped PNG")?;

    Ok(png_bytes)
}
```

- [ ] **Step 3: workspace Cargo.toml 加 member**

在 members 列表末尾加 `"crates/capx"`。

- [ ] **Step 4: 验证编译**

```bash
cargo build -p octopus-capx 2>&1 | tail -5
```

- [ ] **Step 5: Commit**

```bash
git add crates/capx/ Cargo.toml
git commit -m "feat(capx): octopus-capx crate（xcap 截全屏 + 裁剪选区）"
```

---

### Task 2: ClipboardHandle 新增 write_image

**Files:**
- Modify: `crates/clipboard/src/handle.rs`

- [ ] **Step 1: 添加 write_image 方法**

在 `write_text` 方法之后添加：

```rust
/// 写入 PNG 图片到剪贴板（设置 suppress flag）。
pub fn write_image(&self, png_bytes: &[u8]) -> Result<()> {
    self.suppress_flag.store(true, Ordering::SeqCst);
    let ctx = self.ctx.lock().unwrap();
    let img = clipboard_rs::common::RustImageData::from_bytes(png_bytes)
        .map_err(|e| anyhow::anyhow!("Failed to create RustImageData: {}", e))?;
    ctx.set_image(img)
        .map_err(|e| anyhow::anyhow!("Clipboard write image failed: {}", e))?;
    Ok(())
}
```

- [ ] **Step 2: 验证编译 + 测试**

```bash
cargo build -p octopus-clipboard 2>&1 | tail -3
cargo test -p octopus-clipboard 2>&1 | tail -5
```

- [ ] **Step 3: Commit**

```bash
git add crates/clipboard/src/handle.rs
git commit -m "feat(clipboard): ClipboardHandle::write_image"
```

---

### Task 3: AppConfig + DB seed（screenshot_shortcut）

**Files:**
- Modify: `crates/infra/src/config.rs`
- Modify: `crates/infra/src/db.rs`
- Modify: `crates/infra/src/db.sql`

- [ ] **Step 1: config.rs 新增字段**

在 `clipboard_max_age_days` 之后添加：

```rust
    /// 截图全局快捷键（Tauri Accelerator 格式）。默认 "Alt+S"。
    #[serde(default = "default_screenshot_shortcut")]
    pub screenshot_shortcut: String,
```

新增默认值函数：
```rust
fn default_screenshot_shortcut() -> String {
    "Alt+S".into()
}
```

Default impl 末尾加：
```rust
            screenshot_shortcut: default_screenshot_shortcut(),
```

- [ ] **Step 2: db.rs save_app_config_at + load_app_config_at 补字段**

save fields 数组 `[(&str, String); 25]` → `[(&str, String); 26]`，末尾加：
```rust
        ("screenshot_shortcut", cfg.screenshot_shortcut.clone()),
```

load match 分支加：
```rust
            "screenshot_shortcut" => cfg.screenshot_shortcut = value,
```

- [ ] **Step 3: db.sql app_config seed 加**

```sql
    ('screenshot_shortcut',       'Alt+S',                                '截图快捷键'),
```

- [ ] **Step 4: settings_commands apply_config_value 加字段**

```rust
        "screenshot_shortcut" => {
            cfg.screenshot_shortcut = value.as_str().ok_or("screenshot_shortcut 需要字符串")?.to_string();
        }
```

- [ ] **Step 5: set_config 热重载**

在 `clipboard_shortcut` 热重载块之后添加 `screenshot_shortcut` 热重载（同模式：unregister 旧 + register 新 + on_shortcut 回调调 start_screenshot）。

- [ ] **Step 6: 验证编译**

```bash
cargo build -p octopus-infra -p octopus-desktop --features embedded 2>&1 | tail -5
```

- [ ] **Step 7: 手动 seed DB**

```bash
sqlite3 ~/.octopus/octopus.db "INSERT OR IGNORE INTO app_config (config_key, config_value, description) VALUES ('screenshot_shortcut', 'Alt+S', '截图快捷键');"
```

- [ ] **Step 8: Commit**

```bash
git add crates/infra/ crates/desktop/src/settings_commands.rs
git commit -m "feat(infra): screenshot_shortcut 配置 + 热重载"
```

---

### Task 4: screenshot_commands.rs（start/confirm/cancel）

**Files:**
- Create: `crates/desktop/src/screenshot_commands.rs`
- Modify: `crates/desktop/Cargo.toml`（加 octopus-capx + octopus-clipboard 依赖）
- Modify: `crates/desktop/src/main.rs`（注册命令 + 快捷键 + 托盘菜单）

- [ ] **Step 1: Cargo.toml 加依赖**

```toml
octopus-capx = { path = "../capx" }
```

- [ ] **Step 2: 实现 screenshot_commands.rs**

```rust
use std::sync::Mutex;
use tauri::{Emitter, Manager, State};
use octopus_clipboard::ClipboardHandle;
use octopus_clipboard::image;

static SCREENSHOT_DATA: Mutex<Option<octopus_capx::capture::ScreenCapture>> = Mutex::new(None);
const WINDOW_LABEL: &str = "screenshot_window";

#[tauri::command]
pub async fn start_screenshot(app_handle: tauri::AppHandle) -> Result<(), String> {
    // 1. 截全屏
    let capture = octopus_capx::capture::capture_full_screen()
        .map_err(|e| format!("截图失败: {}", e))?;

    // 2. RGBA → PNG base64（前端渲染用）
    let img = ::image::RgbaImage::from_raw(capture.width, capture.height, capture.rgba_bytes.clone())
        .map_err(|e| format!("图像处理失败: {}", e))?;
    let mut png_bytes = Vec::new();
    img.write_to(&mut std::io::Cursor::new(&mut png_bytes), ::image::ImageFormat::Png)
        .map_err(|e| format!("PNG 编码失败: {}", e))?;

    let width = capture.width;
    let height = capture.height;

    // 3. 暂存
    *SCREENSHOT_DATA.lock().unwrap() = Some(capture);

    // 4. 创建/重建截图窗口
    if let Some(old) = app_handle.get_webview_window(WINDOW_LABEL) {
        let _ = old.destroy();
    }

    use tauri::WebviewWindowBuilder;
    let _ = WebviewWindowBuilder::new(
        &app_handle,
        WINDOW_LABEL,
        tauri::WebviewUrl::App("index.html#/screenshot".into()),
    )
    .title("")
    .fullscreen(true)
    .decorations(false)
    .always_on_top(true)
    .skip_taskbar(true)
    .resizable(false)
    .transparent(true)
    .build();

    // 5. 等前端 ready 后 emit 图片数据
    use base64::{Engine, engine::general_purpose};
    let b64 = general_purpose::STANDARD.encode(&png_bytes);
    let ah = app_handle.clone();
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(300));
        let _ = ah.emit("screenshot://ready", serde_json::json!({
            "image": b64,
            "width": width,
            "height": height,
        }));
    });

    Ok(())
}

#[tauri::command]
pub async fn confirm_screenshot(
    x: u32,
    y: u32,
    w: u32,
    h: u32,
    app_handle: tauri::AppHandle,
    handle: State<'_, std::sync::Arc<ClipboardHandle>>,
) -> Result<(), String> {
    // 1. 取全屏数据
    let full = SCREENSHOT_DATA.lock().unwrap().take()
        .ok_or("无截图数据")?;

    // 2. 裁剪选区
    let png_bytes = octopus_capx::capture::crop_region(&full, x, y, w, h)
        .map_err(|e| format!("裁剪失败: {}", e))?;

    // 3. 编码去重 → WebP → DB
    let (png_for_hash, hash) = image::encode_and_hash(
        // crop_region 返回的已经是 PNG，需要先解码为 RGBA 再走 encode_and_hash
        // 实际上 crop_region 返回 PNG，可以直接算 hash
        &{
            // 重新编码 RGBA → PNG for hash consistency
            let img = ::image::load_from_memory(&png_bytes)
                .map_err(|e| format!("decode failed: {}", e))?;
            let rgba = img.to_rgba8();
            let w = rgba.width();
            let h = rgba.height();
            rgba.into_raw().as_slice().to_vec()
            // encode_and_hash 接受 RGBA，这里需要适配
            // 实际上 encode_and_hash 签名是 (rgba: &[u8], width, height)
        },
        w, h,
    ).map_err(|e| format!("编码失败: {}", e))?;
    // 注：crop_region 返回 PNG bytes，不是 RGBA。
    // 需要在 capx 中额外暴露 crop_region_rgba 或在这里解码 PNG → RGBA
    // 简化方案：直接用 png_bytes 算 hash（SHA-256 of PNG bytes）

    // 4. encode_to_webp + insert_image_data + insert_clipboard_item
    // 5. write_image to clipboard (suppress flag)
    // 6. 关窗口

    Ok(())
}

#[tauri::command]
pub async fn cancel_screenshot(app_handle: tauri::AppHandle) -> Result<(), String> {
    *SCREENSHOT_DATA.lock().unwrap() = None;
    if let Some(window) = app_handle.get_webview_window(WINDOW_LABEL) {
        let _ = window.destroy();
    }
    Ok(())
}
```

**注意**：confirm_screenshot 中的去重/WebP/DB 逻辑需要适配——crop_region 返回 PNG bytes，而 `encode_and_hash` 接受 RGBA。需要在 capx 中改为返回 RGBA，或新增 `crop_region_png` + 直接对 PNG bytes 算 SHA-256。

简化方案：在 capx 中新增 `crop_region_png` 返回 `Vec<u8>` PNG，confirm 中直接对 PNG bytes 算 SHA-256（绕过 encode_and_hash），然后解码 PNG → encode_to_webp。

- [ ] **Step 3: main.rs 注册命令 + 快捷键**

mod 声明加 `mod screenshot_commands;`

invoke_handler 加：
```rust
            screenshot_commands::start_screenshot,
            screenshot_commands::confirm_screenshot,
            screenshot_commands::cancel_screenshot,
```

setup 中加截图快捷键注册（从 config 读 `screenshot_shortcut`）。

托盘菜单加「截图」项（tray.rs）。

- [ ] **Step 4: 验证编译**

```bash
cargo build -p octopus-desktop --features embedded 2>&1 | tail -5
```

- [ ] **Step 5: Commit**

```bash
git add crates/desktop/
git commit -m "feat(desktop): screenshot 命令 + 快捷键 + 托盘菜单"
```

---

### Task 5: 前端选区 Canvas UI

**Files:**
- Create: `crates/desktop/frontend/src/pages/Screenshot/index.tsx`
- Modify: `crates/desktop/frontend/src/main.tsx`（路由加 screenshot）

- [ ] **Step 1: main.tsx 路由**

```typescript
// 路由判断：URL hash = #/screenshot 时渲染 Screenshot 组件
```

- [ ] **Step 2: 实现 Screenshot/index.tsx**

核心功能：
- 监听 `screenshot://ready` 事件 → 拿到全屏 PNG base64
- Canvas 渲染全屏图 + 暗遮罩
- 鼠标拖拽框选（mousedown/mousemove/mouseup）
- 8 手柄 resize + 选区 move
- devicePixelRatio 换算
- Enter 确认 → invoke confirm_screenshot
- ESC/右键 → invoke cancel_screenshot
- 选区右下角尺寸标注

组件结构（伪代码）：
```tsx
export default function Screenshot() {
  const canvasRef = useRef<HTMLCanvasElement>(null);
  const [bgImage, setBgImage] = useState<HTMLImageElement | null>(null);
  const [selection, setSelection] = useState<Selection | null>(null);
  const [mode, setMode] = useState<"idle" | "selecting" | "move" | "resize">("idle");
  const [resizeHandle, setResizeHandle] = useState<string | null>(null);
  const dpr = window.devicePixelRatio || 1;

  // 1. listen screenshot://ready → setBgImage
  // 2. Canvas 绘制：bgImage + 遮罩（clearRect 选区）+ 选区边框 + 8 手柄 + 尺寸标注
  // 3. mousedown：判断命中手柄/选区/外部 → 设 mode
  // 4. mousemove：按 mode 更新 selection（归一化 + clamp）
  // 5. mouseup：回 idle/selected
  // 6. keydown：Enter → confirm（dpr 换算），ESC → cancel

  // 全屏 Canvas：position fixed, w/h = window.innerWidth/Height
  // bgImage 按 CSS 像素渲染，confirm 时坐标 × dpr → 物理坐标
}
```

- [ ] **Step 3: 构建前端**

```bash
cd crates/desktop/frontend && npm run build 2>&1 | tail -5
```

- [ ] **Step 4: Commit**

```bash
git add crates/desktop/frontend/
git commit -m "feat(screenshot): 前端选区 Canvas UI（框选 + 手柄调整 + 尺寸标注）"
```

---

### Task 6: 端到端验证

- [ ] **Step 1: 完整构建**

```bash
cd crates/desktop/frontend && npm run build
cd .. && cargo build --features embedded 2>&1 | tail -5
```

- [ ] **Step 2: 运行应用测试截图**

```bash
./run-octopus.sh
```

验证：
1. 按 Alt+S → 全屏变暗 + 十字准星
2. 鼠标拖拽框选 → 选区高亮 + 尺寸标注
3. 拖拽手柄 → 选区调整
4. 拖拽选区内部 → 平移
5. Enter → 截图进剪贴板浮窗（图片条目）
6. ESC → 取消
7. 托盘菜单「截图」→ 同样触发
8. 设置页「截图」快捷键可改 + 热重载

- [ ] **Step 3: 最终 Commit（如有修复）**

---

## Spec Coverage（自审）

| spec 章节 | 实现 task |
|---|---|
| §1 架构（crate 结构） | Task 1 |
| §1.3 capture.rs 接口 | Task 1 |
| §2 截图触发流程 | Task 4 |
| §2.1 选区交互状态机 | Task 5 |
| §2.2 选区调整手柄 | Task 5 |
| §3 前端选区 Canvas | Task 5 |
| §4 数据流（手动写入剪贴板历史） | Task 4（confirm_screenshot）|
| §4.3 截图配置 | Task 3 |
| §5 Tauri 命令 + 窗口 | Task 4 |
| §6 错误处理 | Task 4 + Task 5 |

---

## 实施偏差与补充记录

### 偏差 1：前端拉取模式（替代 emit 延迟）

原设计用 emit + 300ms 延迟发送截图数据给前端，实际 emit 在前端未 ready 时丢失。改为 `get_screenshot_image` 命令——前端 mount 后主动调用，暂存到 `PENDING_IMAGE` 静态变量（与 settings_window 的 `PENDING_PAGE` 同模式）。

### 偏差 2：Monitor::from_point 定位

`Monitor::all().next()` 可能取到错误的显示器。改为 `Monitor::from_point(鼠标位置)` 定位用户当前所在显示器。macOS 用 `core-graphics::CGEvent` 获取鼠标位置。

### 偏差 3：去掉 transparent: true

透明窗口在加载期间闪烁黑色。改为不透明窗口，前端自行渲染全屏 Canvas（黑色 loading 态 → 截图数据就绪后渲染）。

### 偏差 4：base64 从 optional 改为非 optional

screenshot_commands 需要 base64 编码 PNG，将 base64 从 cloud feature 的 optional 依赖改为非 optional。

### 偏差 5：xcap 软链接 + workspace exclude

xcap 声明了 `[workspace]`，导致 octopus workspace 冲突。解决：`exclude = ["xcap"]` + `.gitignore` 排除软链接。

### 偏差 6：macOS 权限

通过 `cargo run` 运行时，屏幕录制权限绑定终端应用（非二进制）。首次截图黑屏 → 授权终端后重启生效。打包 .app 后绑定 octopus 本身。

### 偏差 7：1.1 期多显示器——每屏独立窗口（非拼接）

原设计为「截取所有屏幕拼接为一张全图」，用户澄清为「指定截哪个屏幕」。改为每屏独立窗口：
- `capture_all_monitors()` 截所有显示器
- 每个显示器创建独立 Tauri 窗口（`screenshot_window` / `screenshot_window_N`）
- 窗口坐标用 Tauri `available_monitors()` 逻辑坐标（物理除以 `scale_factor`）
- confirm/cancel 关闭所有 `screenshot_*` 窗口

### 偏差 8：窗口闪烁——延迟显示

窗口创建后立即可见导致白屏闪烁。改为 `visible(false)` + 前端 Canvas 渲染完后调 `show_screenshot_window` 命令显示。
- main.tsx 按 window label 提前设 body 背景为 `rgba(0,0,0,0.5)`
- Loading 态也用 `rgba(0,0,0,0.5)` 和最终遮罩一致
- **TODO**：后续可加窗口过渡动画进一步消除抖动（达到 Xnip 级体验）
