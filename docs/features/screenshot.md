# 截图系统

> `octopus-capx` crate——区域截图、滚动截图（NCC+Sobel 拼接引擎）、标注工具栏、跨平台贴图浮窗（pin_window）、截图 OCR。依赖 xcap 0.9.6（crates.io 发布版）。

源文件：`crates/capx/src/`、`crates/desktop/src/screenshot_commands.rs`、`crates/desktop/src/screenshot_geometry.rs`。

---

## 1. 模块结构

| 模块 | 职责 |
|------|------|
| `capture` | `capture_all_monitors()` 截取所有显示器（RGBA + 物理像素尺寸 + 显示器坐标）；`crop_region()` 裁剪选区→PNG；`crop_region_rgba()` 直接返回 `RgbaImage`（零 PNG 编解码，滚动截帧热路径专用） |
| `stitch` | 滚动截屏拼接引擎：Canvas-Anchored NCC + Sobel 梯度匹配 |

---

## 2. 区域截图流程

```
start_screenshot
  → capture_all_monitors（截所有显示器）
  → 每屏创建不可见窗口（visible(false)）
  → 前端 get_screenshot_image（ipc::Response 返回原始 JPEG 字节）
      → 前端 URL.createObjectURL 加载
      → Canvas 渲染（原图 + 暗遮罩 + 选区框 + 8 手柄 + 尺寸标注）
  → show_screenshot_window 显示（消除白屏闪烁）
  → 选区下方弹出标注工具栏（矩形/箭头/文字/序号/撤销）
  → 标注在选区内 Canvas clip 绘制
  → Enter 确认：
      Canvas toBlob → Uint8Array Raw body → ipc::Request（不经 base64）
      → PNG SHA-256 去重 → WebP BLOB → DB image_data + clipboard_history + 系统剪贴板
      → 关所有窗口
```

**黑屏检测日志**：权限诊断辅助。

---

## 3. 滚动截图流程

```
用户框选区域
  → 按 Cmd+Shift+D 进入手动滚动模式
  → 后台生产 task 30ms 截帧
  → tokio::sync::watch 通道（丢旧保新）
  → 消费 task NCC 实时拼接
      → preview 编码 fire-and-forget 不阻塞关键路径
  → 截图窗口旁显示拼接预览
  → 用户点绿色「复制」停止
  → 先关截图窗口（用户感知立即停止）
  → 后台并发：
      线程一：PNG → 剪贴板（~1s）
      线程二：canvas → WebP → DB 入库（~2-3s 后台）
  → emit scroll://done { id }（不含 base64，前端不再中转数据）
```

---

## 4. 拼接引擎（Canvas-Anchored NCC + Sobel）

`crates/capx/src/stitch.rs`——**Canvas-Anchored** 消除累积漂移：每帧从画布底部提取 strip → 匹配当前帧 → 追加到画布。

### 流程

1. **strip 提取**：从画布底部提取 `strip_h`（默认 80，`StitchConfig` 字段）px
2. **Sobel 梯度特征图**（`imageproc`，纯色退化回灰度）
3. **NCC 模板匹配**（`imageproc::template_matching::match_template`，CrossCorrelationNormalized）
   - **大屏（帧宽 > `ncc_downsample_width` 默认 1920）走两阶段 refine**：
     - Triangle 降采样域粗定位 dy
     - 原分辨率 ±2px 邻域 `ncc_match_range` refine 恢复亚像素（避免降采样锯齿破坏 response 峰值）
   - 小屏单阶段
4. **验证**：score ≥ `ncc_score_threshold`（默认 0.65）+ response 无区分度拒绝（max-min<0.1）
5. **抛物线亚像素插值**

`strip_h` / `max_scroll` / `ncc_score_threshold` / `ncc_downsample_width` 纳入 `StitchConfig` 字段化（默认值不变行为零变化）。

---

## 5. 降级链

| 级 | 机制 | 触发 |
|----|------|------|
| 1 | **相邻帧参考 fallback**（`prev_gray` + `try_match_prev_frame`） | 内容突变失配时，用前一帧有效区匹配当前帧求正确 dy |
| 2 | **1D 灰度投影 + best-guess** | 历史.dy 中位数，连续 3 次熔断 |
| 3 | **周期性假匹配锁定** | 连续相同 dy≥3 次锁定，dy 变化才解锁 |
| 4 | **NCC stuck 检测** | 连续验证失败≥5 次判静止 |

画布用 `Vec<u8>` 增量追加 + 惰性缓存。

---

## 6. 多显示器

- 每个显示器创建独立 Tauri 窗口（`screenshot_window` / `screenshot_window_N`）
- 用 Tauri `available_monitors()` 获取逻辑坐标 + 尺寸（物理坐标除以 `scale_factor`）
- 定位到对应屏幕
- 窗口初始 `visible(false)`，前端 Canvas 渲染完截图后调 `show_screenshot_window` 显示（消除白屏闪烁）
- 确认/取消时关闭所有 `screenshot_*` 窗口

**窗口串行创建**（间隔 150ms）：macOS WKWebView 同时创建多个全屏窗口会 segfault，故 `start_screenshot` 逐个 `sleep(150ms)` 创建，单窗 build 失败则 `log::error!` + `continue` 跳过该屏。

---

## 7. 标注工具栏

- 工具：选择 / 矩形 / 椭圆 / 菱形 / 直线 / 箭头 / 画笔 / 文字 / 序号 / 马赛克 / OCR / 撤销重做
- 标注在选区内 Canvas clip 绘制
- 命中测试（`hitTestAnnotationPrecise`，`lib/annotation.ts`，Screenshot 与 ImagePreview 共用）：选择工具下点选/拖动标注——空心 rect/oval/diamond/line/arrow/pen 查到线条距离 ≤ `HIT_DIST`(8)；实心 rect/oval/diamond 查鼠标在图形内部（rect 矩形包含 / oval 与 diamond 归一半径 ≤1）；文字/序号用 bounding box。2026-07-07 修正：filled 内部命中 + diamond 独立分支，消除空心菱形误中

---

## 8. 贴图浮窗（pin_window）

截图后「钉住」功能——创建原生浮动窗口置顶显示截图。支持拖拽（左键）、缩放（滚轮，以鼠标位置为锚点）、关闭（hover 右上角红色关闭按钮）。绕过 WebView 直接用原生窗口，单窗内存 < 5MB。

三平台各自实现 `PinWindow` trait（`create(png_data, x, y, w, h)`）：

| 平台 | 实现 |
|------|------|
| macOS | 自定义 `PinNSWindow`（`define_class!`）+ `PinNSImageView`（拖拽经 `performWindowDragWithEvent`、缩放改 frame）+ `PinCloseBtnView`（NSImageView 子类，预渲染 PNG 图标规避 `drawRect:` 崩溃）；`NSTrackingArea` 检测 hover；静态 `PIN_WINDOWS: Mutex<Vec<SendWindow>>` 跟踪窗口 + `setReleasedWhenClosed(false)` 防悬空 + 关闭时延迟 `cleanup` |
| Windows | Win32 `WS_EX_TOPMOST\|LAYERED\|TOOLWINDOW` + `UpdateLayeredWindow`（预乘 BGRA + GDI `StretchBlt` HALFTONE 缩放）；`WM_MOUSEMOVE`+`TrackMouseEvent` 检测 hover；每窗独立线程跑 `GetMessageW` 循环；GDI 资源经 `run_gdi_calls` 闭包 + defer 清理防泄漏 |
| Linux | GTK3 Toplevel + Cairo 自绘（`scroll-event` 缩放锚定 `event.coords()`；`motion-notify`/`leave-notify` 检测 hover） |

右键关闭菜单已三平台移除（hover 按钮体验更优）。

**`screenshot_commands::pin_screenshot` 双路径**：前端 `composeAndCropBytes` 合成带标注/马赛克的 Canvas PNG → `FileReader.readAsDataURL` 转 base64 → 后端解码（`img_base64: Option<String>`）；None 时 fallback 到后端 `ALL_CAPTURES` 裁剪（不含标注）。前端 `isPinningRef` 防重复点击锁。

---

## 9. IPC 二进制传输

所有图片传输已从 base64 改为二进制：

| 方向 | 机制 |
|------|------|
| 前端 → Rust | `ipc::Request` Raw body（`canvas.toBlob → ArrayBuffer → invoke(cmd, arraybuffer)`） |
| Rust → 前端 | `ipc::Response`（原始字节 → 前端 `URL.createObjectURL`） |

消除 base64 编解码 + JSON 序列化开销。

剪贴板历史条目复制（`copy_clipboard_item`）：从 DB 读 WebP → PNG → 剪贴板，移入 `spawn_blocking` 不阻塞 UI。

---

## 10. 截图 OCR

`ocr_screenshot` 后端闭环：

1. 图片入库（截图历史，`save_screenshot_to_history` helper 三处去重）
2. `octopus_paddle_ocr` 识别（入库 + 识别 + OCR 入库均在 `spawn_blocking` 内隔离 CPU 任务）
3. `insert_ocr_item`（item_type='ocr'，meta_info={engine,model,char_count}）
4. 经主线程调 `open_compact_editor_tab(image_id, None)` 开图片 tab
5. `open_compact_editor_tab(ocr_id, None)` 开文本 tab
6. `emit("clipboard://changed")` + `emit("ocr-screenshot://result", { text, blocks })`（推送 OCR 文本块给图片 tab 的 ImagePreview 组件叠加显示）

不再 write_text / update_search_text（编辑保存时才写剪贴板）。

---

## 11. macOS 权限

通过 `cargo run` 运行时，屏幕录制权限需授给终端应用（非二进制）。打包 .app 后绑定 octopus 本身。

---

## 12. screenshot_geometry.rs

`start_scroll_recording` 提取出的纯逻辑——所有函数不依赖 Tauri/Quartz 类型，可独立单测：

| 函数 | 职责 |
|------|------|
| `compute_selection_global` | 坐标换算（窗口原点+CSS 偏移→全局逻辑坐标） |
| `find_monitor_for_point` | 显示器命中 |
| `compute_physical_crop` | 物理像素裁剪（含 `.max(0.0)` 跨显示器边界防御——负中间值防 u32 wrap） |
| `compute_preview_crop` | preview 裁剪参数（消除两处重复） |
