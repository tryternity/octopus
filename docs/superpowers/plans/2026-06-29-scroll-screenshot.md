# 滚动截屏实施计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development or superpowers:executing-plans.

**Goal:** 用户框选后点「滚动截图」→ auto 模式自动模拟滚轮 + NCC 拼接 + 实时预览 → 停止后入库（默认 auto，可配 manual）

**Architecture:** capx/stitch.rs ✅ + 录制循环 ✅ + 前端 scrolling ✅。新增：CGEvent 模拟滚轮 + 自动停止 + scroll_mode 配置 + 副屏坐标修正

**Tech Stack:** Rust + imageproc 0.25 + image 0.25 + xcap + Tauri + React

**Spec:** `docs/superpowers/specs/2026-06-29-scroll-screenshot-design.md`

---

## 文件结构

| 文件 | 变更 | 责任 |
|---|---|---|
| `crates/capx/Cargo.toml` | Modify | 加 imageproc 依赖 |
| `crates/capx/src/stitch.rs` | Create | 拼接引擎：Stitcher + NCC + sticky/active cols |
| `crates/capx/src/lib.rs` | Modify | pub mod stitch |
| `crates/desktop/src/screenshot_commands.rs` | Modify | start/stop_scroll_recording 命令 |
| `crates/desktop/src/main.rs` | Modify | 注册命令 |
| `crates/desktop/frontend/src/pages/Screenshot/index.tsx` | Modify | scrolling 模式 + 预览 + 工具栏按钮 |

---

### Task 1: capx/stitch.rs 拼接引擎

**Files:**
- Create: `crates/capx/src/stitch.rs`
- Modify: `crates/capx/src/lib.rs`, `crates/capx/Cargo.toml`

- [ ] **Step 1: Cargo.toml 加 imageproc**

```toml
imageproc = "0.25"
```

- [ ] **Step 2: lib.rs 加 pub mod stitch**

- [ ] **Step 3: 实现 stitch.rs**

```rust
use anyhow::{Context, Result};
use image::{GrayImage, RgbaImage};
use imageproc::gradients::sobel_gradients;
use std::ops::Range;

pub struct StitchConfig {
    pub template_ratio: f32,
    pub min_confidence: f32,
    pub inertia_px: i32,
    pub max_lowconf_streak: u32,
}

impl Default for StitchConfig {
    fn default() -> Self {
        Self {
            template_ratio: 0.2,
            min_confidence: 0.5,
            inertia_px: 100,
            max_lowconf_streak: 8,
        }
    }
}

pub struct Stitcher {
    canvas: RgbaImage,
    last_edges: Option<GrayImage>,
    sticky_top: u32,
    sticky_bottom: u32,
    active_cols: Range<u32>,
    last_delta: i32,
    low_conf_streak: u32,
    config: StitchConfig,
    frame_count: u32,
}

impl Stitcher {
    /// 用首帧初始化。
    pub fn new(first_frame: RgbaImage, config: StitchConfig) -> Self {
        let (w, h) = (first_frame.width(), first_frame.height());
        let edges = compute_edges(&first_frame);
        Self {
            canvas: first_frame,
            last_edges: Some(edges),
            sticky_top: 0,
            sticky_bottom: 0,
            active_cols: 0..w,
            last_delta: 0,
            low_conf_streak: 0,
            config,
            frame_count: 1,
        }
    }

    /// 处理新帧，返回是否拼接了新内容。
    pub fn process_frame(&mut self, frame: &RgbaImage) -> Result<bool> {
        self.frame_count += 1;
        let (w, h) = (frame.width(), frame.height());

        // 第二帧：检测 sticky header/footer + active cols
        if self.frame_count == 2 {
            self.detect_sticky_and_active(&self.canvas, frame);
        }

        // 有效区域（去掉 sticky）
        let eff_top = self.sticky_top;
        let eff_bottom = h.saturating_sub(self.sticky_bottom);
        if eff_bottom <= eff_top { return Ok(false); }
        let eff_h = eff_bottom - eff_top;

        // 重复帧检测
        if self.is_duplicate(frame) {
            return Ok(false);
        }

        let edges = compute_edges(frame);
        let last = match &self.last_edges {
            Some(e) => e,
            None => { self.last_edges = Some(edges); return Ok(false); }
        };

        // 从上一帧底部取模板
        let tpl_h = ((eff_h as f32 * self.config.template_ratio) as u32).max(10);
        let tpl_top = eff_bottom.saturating_sub(tpl_h);

        // 在当前帧搜索（惯性窗口优先）
        let (delta, confidence) = self.match_template(
            last, &edges, tpl_top, eff_top, eff_bottom, tpl_h, w,
        );

        if confidence >= self.config.min_confidence {
            self.low_conf_streak = 0;
            self.last_delta = delta;
        } else {
            self.low_conf_streak += 1;
            if self.low_conf_streak < self.config.max_lowconf_streak {
                // 用上次 delta 硬拼接
            } else {
                // 超过上限，跳过
                self.last_edges = Some(edges);
                return Ok(false);
            }
        }

        // 裁剪新内容（当前帧中超出重叠的部分）
        let new_start = eff_top + (self.last_delta as u32).min(eff_h);
        if new_start >= eff_bottom {
            self.last_edges = Some(edges);
            return Ok(false);
        }

        let new_rows = image::imageops::crop_imm(frame, 0, new_start, w, eff_bottom - new_start).to_image();

        // 追加到 canvas
        let old_h = self.canvas.height();
        let mut combined = RgbaImage::new(w, old_h + new_rows.height());
        combined.copy_from(&self.canvas, 0, 0).context("canvas copy")?;
        combined.copy_from(&new_rows, 0, old_h).context("new_rows copy")?;
        self.canvas = combined;

        self.last_edges = Some(edges);
        Ok(true)
    }

    /// 获取当前拼接结果。
    pub fn canvas(&self) -> &RgbaImage { &self.canvas }

    /// 获取当前高度。
    pub fn height(&self) -> u32 { self.canvas.height() }

    fn detect_sticky_and_active(&mut self, frame_a: &RgbaImage, frame_b: &RgbaImage) {
        let (w, h) = (frame_a.width(), frame_a.height());
        // sticky top：逐行比较，前 N 行完全相同
        let mut sticky_t = 0u32;
        for y in 0..h.min(100) {
            if rows_equal(frame_a, frame_b, y, y, w) { sticky_t = y + 1; }
            else { break; }
        }
        // sticky bottom
        let mut sticky_b = 0u32;
        for y in 0..h.min(100) {
            let ya = h - 1 - y;
            let yb = h - 1 - y;
            if rows_equal(frame_a, frame_b, ya, yb, w) { sticky_b = y + 1; }
            else { break; }
        }
        self.sticky_top = sticky_t;
        self.sticky_bottom = sticky_b;

        // active cols：找变化的列范围
        let mut min_col = w;
        let mut max_col = 0u32;
        for x in 0..w {
            if !cols_equal(frame_a, frame_b, x, h) {
                min_col = min_col.min(x);
                max_col = max_col.max(x);
            }
        }
        if min_col <= max_col {
            self.active_cols = min_col..max_col + 1;
        }
    }

    fn is_duplicate(&self, frame: &RgbaImage) -> bool {
        let last = match &self.last_edges {
            Some(e) => e,
            None => return false,
        };
        let curr = compute_edges(frame);
        let step = 8;
        let mut diff_sum = 0u64;
        let mut count = 0u64;
        for y in (0..last.height()).step_by(step) {
            for x in (0..last.width()).step_by(step) {
                let a = last.get_pixel(x, y)[0] as i32;
                let b = curr.get_pixel(x, y)[0] as i32;
                diff_sum += (a - b).unsigned_abs() as u64;
                count += 1;
            }
        }
        if count == 0 { return false; }
        let mean = diff_sum as f64 / count as f64;
        mean < 2.0
    }

    fn match_template(
        &self,
        last: &GrayImage, curr: &GrayImage,
        tpl_top: u32, eff_top: u32, eff_bottom: u32, tpl_h: u32, w: u32,
    ) -> (i32, f32) {
        // 在惯性窗口内搜索最佳 delta
        let search_start = (self.last_delta - self.config.inertia_px).max(0) as u32;
        let search_end = ((self.last_delta + self.config.inertia_px) as u32).min(eff_bottom - eff_top - tpl_h);

        let mut best_delta = self.last_delta;
        let mut best_score = -1.0f32;

        for d in search_start..=search_end {
            let score = ncc_score(last, curr, tpl_top, eff_top + d, w, tpl_h, &self.active_cols);
            if score > best_score {
                best_score = score;
                best_delta = d as i32;
            }
        }

        // 低置信：全范围重搜
        if best_score < self.config.min_confidence {
            for d in 0..=(eff_bottom - eff_top - tpl_h) {
                let score = ncc_score(last, curr, tpl_top, eff_top + d, w, tpl_h, &self.active_cols);
                if score > best_score {
                    best_score = score;
                    best_delta = d as i32;
                }
            }
        }

        (best_delta, best_score)
    }
}

fn compute_edges(img: &RgbaImage) -> GrayImage {
    let gray = image::imageops::grayscale(img);
    sobel_gradients(&gray)
}

fn rows_equal(a: &RgbaImage, b: &RgbaImage, ya: u32, yb: u32, w: u32) -> bool {
    for x in 0..w {
        if a.get_pixel(x, ya) != b.get_pixel(x, yb) { return false; }
    }
    true
}

fn cols_equal(a: &RgbaImage, b: &RgbaImage, x: u32, h: u32) -> bool {
    for y in 0..h {
        if a.get_pixel(x, y) != b.get_pixel(x, y) { return false; }
    }
    true
}

/// 归一化互相关（NCC）— 在指定列范围内比较模板和目标区域。
fn ncc_score(last: &GrayImage, curr: &GrayImage, tpl_y: u32, tgt_y: u32, w: u32, tpl_h: u32, cols: &Range<u32>) -> f32 {
    if tpl_h == 0 || cols.is_empty() { return 0.0; }
    let col_w = cols.end - cols.start;

    // 计算均值
    let mut sum_t = 0.0f64;
    let mut sum_c = 0.0f64;
    let n = (tpl_h * col_w) as f64;
    for y in 0..tpl_h {
        for x in cols.clone() {
            if x >= w { continue; }
            sum_t += last.get_pixel(x, tpl_y + y)[0] as f64;
            sum_c += curr.get_pixel(x, tgt_y + y)[0] as f64;
        }
    }
    let mean_t = sum_t / n;
    let mean_c = sum_c / n;

    // NCC
    let mut num = 0.0f64;
    let mut den_t = 0.0f64;
    let mut den_c = 0.0f64;
    for y in 0..tpl_h {
        for x in cols.clone() {
            if x >= w { continue; }
            let t = last.get_pixel(x, tpl_y + y)[0] as f64 - mean_t;
            let c = curr.get_pixel(x, tgt_y + y)[0] as f64 - mean_c;
            num += t * c;
            den_t += t * t;
            den_c += c * c;
        }
    }
    let denom = (den_t * den_c).sqrt();
    if denom < 1e-6 { return 0.0; }
    (num / denom) as f32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_stitch_identical_frame() {
        let frame = RgbaImage::from_pixel(100, 200, image::Rgba([255, 0, 0, 255]));
        let mut stitcher = Stitcher::new(frame.clone(), StitchConfig::default());
        let result = stitcher.process_frame(&frame).unwrap();
        assert!(!result, "Identical frame should be skipped as duplicate");
    }

    #[test]
    fn test_stitch_offset() {
        // 创建一个有纹理的图像（渐变），滚动 50px
        let mut frame_a = RgbaImage::new(100, 200);
        for y in 0..200 {
            for x in 0..100 {
                frame_a.put_pixel(x, y, image::Rgba([(x % 256) as u8, (y % 256) as u8, 128, 255]));
            }
        }
        // frame_b = frame_a 向上滚动 50px（底部补新内容）
        let mut frame_b = RgbaImage::new(100, 200);
        for y in 0..200 {
            for x in 0..100 {
                let src_y = if y + 50 < 200 { y + 50 } else { y + 50 };
                frame_b.put_pixel(x, y, frame_a.get_pixel(x, src_y.min(199)).clone());
            }
        }
        let mut stitcher = Stitcher::new(frame_a, StitchConfig::default());
        let result = stitcher.process_frame(&frame_b).unwrap();
        assert!(result, "Offset frame should produce new content");
        assert!(stitcher.height() > 200, "Canvas should grow");
    }
}
```

- [ ] **Step 4: 验证编译 + 测试**

```bash
cargo test -p octopus-capx --lib stitch 2>&1 | tail -10
```

- [ ] **Step 5: Commit**

---

### Task 2: 后端录制循环 + 命令

**Files:**
- Modify: `crates/desktop/src/screenshot_commands.rs`
- Modify: `crates/desktop/src/main.rs`

- [ ] **Step 1: 录制状态 + start/stop 命令**

```rust
use std::sync::atomic::{AtomicBool, Ordering};

static SCROLL_RECORDING: AtomicBool = AtomicBool::new(false);

#[tauri::command]
pub async fn start_scroll_recording(
    x: f64, y: f64, w: f64, h: f64,
    app_handle: tauri::AppHandle,
) -> Result<(), String> {
    SCROLL_RECORDING.store(true, Ordering::SeqCst);

    let ah = app_handle.clone();
    tauri::async_runtime::spawn(async move {
        // 获取截图窗口
        let monitors = tauri::available_monitors(&ah).unwrap_or_default();
        let scale = monitors.first().map(|m| m.scale_factor()).unwrap_or(1.0);

        // 物理坐标
        let px = (x * scale) as i32;
        let py = (y * scale) as i32;
        let pw = (w * scale) as u32;
        let ph = (h * scale) as u32;

        // 首帧
        let monitors = xcap::Monitor::all().unwrap_or_default();
        let monitor = monitors.into_iter().next().unwrap();
        let first = monitor.capture_region(px, py, pw, ph).unwrap();
        let first_rgba = image::RgbaImage::from_raw(pw, ph, first.into_raw()).unwrap();

        let mut stitcher = octopus_capx::stitch::Stitcher::new(first_rgba, Default::default());

        let frame_duration = std::time::Duration::from_millis(66); // ~15fps
        let mut interval = tokio::time::interval(frame_duration);
        interval.tick().await; // skip first

        while SCROLL_RECORDING.load(Ordering::SeqCst) {
            interval.tick().await;

            let monitors = xcap::Monitor::all().unwrap_or_default();
            let monitor = match monitors.into_iter().next() {
                Some(m) => m,
                None => continue,
            };

            let frame = match monitor.capture_region(px, py, pw, ph) {
                Ok(f) => f,
                Err(_) => continue,
            };
            let frame_rgba = match image::RgbaImage::from_raw(pw, ph, frame.into_raw()) {
                Some(img) => img,
                None => continue,
            };

            let added = match stitcher.process_frame(&frame_rgba) {
                Ok(true) => true,
                _ => false,
            };

            if added {
                // 发送预览
                let mut png_bytes = Vec::new();
                let canvas = stitcher.canvas();
                let preview = if canvas.height() > 800 {
                    image::imageops::resize(canvas, 200, (200 * canvas.height() / canvas.width()).min(600), image::imageops::FilterType::Nearest)
                } else {
                    image::imageops::resize(canvas, 200, 200 * canvas.height() / canvas.width(), image::imageops::FilterType::Nearest)
                };
                let _ = preview.write_to(&mut std::io::Cursor::new(&mut png_bytes), image::ImageFormat::Png);
                let b64 = general_purpose::STANDARD.encode(&png_bytes);
                let _ = ah.emit("scroll://frame", serde_json::json!({
                    "image": b64,
                    "height": stitcher.height(),
                    "phys_height": (stitcher.height() as f64 / scale) as u32,
                }));
            }
        }

        // 录制结束：入库
        let canvas = stitcher.canvas().clone();
        let mut png_bytes = Vec::new();
        let _ = canvas.write_to(&mut std::io::Cursor::new(&mut png_bytes), image::ImageFormat::Png);

        // SHA-256 → WebP → 入库（复用现有流程）
        let hash = octopus_clipboard::image::sha256_hex(&png_bytes);
        let img = image::load_from_memory(&png_bytes).unwrap();
        let encoded = octopus_clipboard::image::encode_to_webp_from_image(&img).unwrap();

        let item_id = octopus_clipboard::store::chrono_millis();
        octopus_infra::db::with_db(|conn| {
            octopus_clipboard::store::insert_image_data(conn, &hash, &encoded.webp_blob, &encoded.thumb_blob, img.width() as i64, img.height() as i64)
        }).ok();
        octopus_infra::db::with_db(|conn| {
            octopus_clipboard::store::insert_clipboard_item(conn, &octopus_clipboard::store::NewClipboardItem {
                id: item_id, item_type: octopus_clipboard::ItemType::Image,
                content: hash.clone(), search_text: String::new(),
                created_at: octopus_clipboard::store::iso_now(),
                blob_hash: Some(hash), width: Some(img.width() as i64),
                height: Some(img.height() as i64), has_thumbnail: Some(1),
                file_count: None, is_rich: false,
            })
        }).ok();

        let _ = ah.emit("scroll://done", serde_json::json!({ "id": item_id }));
        let _ = ah.emit("clipboard://changed", ());
    });

    Ok(())
}

#[tauri::command]
pub fn stop_scroll_recording() {
    SCROLL_RECORDING.store(false, Ordering::SeqCst);
}
```

- [ ] **Step 2: main.rs 注册命令**
- [ ] **Step 3: 验证编译**
- [ ] **Step 4: Commit**

---

### Task 3: 前端 scrolling 模式 + 预览 + 工具栏

**Files:**
- Modify: `crates/desktop/frontend/src/pages/Screenshot/index.tsx`

- [ ] **Step 1: Mode 类型加 scrolling**

```typescript
type Mode = "idle" | "selecting" | "selected" | "move" | "resize" | "scrolling";
```

- [ ] **Step 2: 状态 + 预览数据**

```typescript
const [scrollPreview, setScrollPreview] = useState<string | null>(null);
const [scrollHeight, setScrollHeight] = useState(0);
```

- [ ] **Step 3: 监听 scroll://frame + scroll://done**

useEffect 注册 listen，frame 事件更新预览，done 事件回到 selected 模式

- [ ] **Step 4: 工具栏按钮**

撤销之后加「滚动截图」按钮（scroll.svg 图标）
scrolling 模式时变为「停止滚动」按钮

- [ ] **Step 5: 启动/停止逻辑**

```typescript
function startScroll() {
  if (!sel) return;
  setModeSafe("scrolling");
  // 物理坐标传给后端
  invoke("start_scroll_recording", {
    x: sel.x, y: sel.y, w: sel.w, h: sel.h
  });
}

function stopScroll() {
  invoke("stop_scroll_recording");
}
```

- [ ] **Step 6: 预览浮层 DOM**

选区右/左侧 200px 宽浮层，显示 scrollPreview base64 + 高度 + 状态 + 停止按钮

- [ ] **Step 7: scrolling 模式下禁用其他工具**

mouseDown/mouseMove 在 scrolling 模式下直接 return

- [ ] **Step 8: 构建验证**

---

### Task 4: 端到端验证

- [ ] 选区确定 → 点击「滚动截图」→ 预览窗口出现
- [ ] 用触控板滚动网页 → 预览实时更新
- [ ] 点击「停止滚动」→ 长图入库 → 剪贴板浮窗可见
- [ ] 追踪丢失 → 红色警告
- [ ] 重复内容 → 跳过不拼接

---

## Spec Coverage（自审）

| spec 章节 | 实现 task |
|---|---|
| §1 架构 | Task 2 + 3 |
| §2 拼接引擎 | Task 1 |
| §2.3 粘性 header/footer | Task 1（detect_sticky_and_active）|
| §2.4 活跃列检测 | Task 1（active_cols）|
| §2.5 降级策略 | Task 1（low_conf_streak）|
| §3 预览窗口 | Task 3 |
| §3.1 预览位置 | Task 3（右侧优先）|
| §4 状态机 | Task 3（scrolling 模式）|
| §4.2 录制循环 | Task 2 |
| §4.3 Tauri 命令 | Task 2 |
| §4.4 事件 | Task 2 + 3 |
| §5 错误处理 | Task 1 + 2 + 3 |

---

## 实施偏差与重构记录

### 偏差 1：窗口隐藏方案（失败）

原始实现录制时隐藏截图窗口，用户无法看到选区和工具栏。用户反馈"像取消了截图"。
改为：窗口保持显示 + `set_ignore_cursor_events(true)` 滚轮穿透。

### 偏差 2：ignore cursor 导致工具栏不可交互（失败）

整个窗口 ignore cursor 后，工具栏/预览/停止按钮全部无法点击。
改为：区域化 cursor events——鼠标在工具栏/预览区域时 `set_ignore_cursor_events(false)`，离开时恢复 true。

### 偏差 3：spawn_blocking 避免阻塞 event loop

`capture_all_monitors` 每帧 56MB，同步执行阻塞 tokio 导致 ESC 无响应。
改为：每帧用 `tokio::task::spawn_blocking` 在独立线程截图。

### 偏差 4：帧率 15fps → 10fps

每帧 `capture_all_monitors`（56MB RGBA）+ NCC 计算开销大，15fps CPU 紧张。降为 10fps（100ms 间隔）。

### 偏差 5：auto 模式（CGEvent 模拟滚轮）替代 manual 模式

诊断报告确认 macOS `always_on_top` 窗口抢占键盘焦点，导致 `set_ignore_cursor_events(true)` 后滚轮事件不到达底层应用。改方案：
- **auto 模式**（默认）：后端用 `CGEventCreateScrollWheelEvent` 模拟滚轮，不依赖焦点穿透
- **manual 模式**（后续）：用 `tauri-nspanel` NSPanel（NonactivatingPanel 不抢焦点）
- 配置 `scroll_mode`（auto | manual）

### Task 8（Step 4）：auto 模式后端

**Files:**
- Modify: `crates/desktop/src/screenshot_commands.rs`
- Modify: `crates/infra/src/config.rs`, `db.rs`, `db.sql`
- Modify: `crates/desktop/src/settings_commands.rs`

- [ ] **Step 1: scroll_mode 配置**

AppConfig 加 `scroll_mode: String`（默认 `"auto"`），db.sql seed + save/load 补字段。

- [ ] **Step 2: CGEvent 模拟滚轮函数**

```rust
#[cfg(target_os = "macos")]
fn send_scroll_event(delta: i32) {
    use core_graphics::event::{CGEvent, CGEventType, ScrollEventUnit, CGEventTapLocation};
    use core_graphics::event_source::{CGEventSource, CGEventSourceStateID};
    if let Ok(source) = CGEventSource::new(CGEventSourceStateID::HIDSystemState) {
        if let Ok(event) = CGEvent::new_scroll_wheel_event2(
            &source, ScrollEventUnit::Pixel, delta, 0
        ) {
            event.post(CGEventTapLocation::Session);
        }
    }
}
```

- [ ] **Step 3: start_scroll_recording 增加 mode 参数**

auto 模式：录制循环每帧先 `send_scroll_event(40)` → sleep(50ms) → capture → stitch。
去掉 `set_ignore_cursor_events`。
连续 3 帧无新内容 → 自动停止。

manual 模式（占位）：保持现有 ignore cursor 逻辑（后续 NSPanel 替代）。

- [ ] **Step 4: 副屏坐标修正**

start 时记录 monitor 物理偏移 `(target_x, target_y)`。
capture 循环用 `captures.find(|c| c.monitor_x == target_x && c.monitor_y == target_y)` 匹配正确显示器。

- [ ] **Step 5: 验证编译 + 前端 build**

### Task 9（Step 5）：前端适配

- [ ] auto 模式下工具栏显示「停止」按钮（不显示滚动提示）
- [ ] auto 模式预览正常更新
- [ ] auto 模式停止后入库 + 剪贴板刷新
