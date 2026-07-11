# 截图系统

> `octopus-capx` crate——区域截图、滚动截图（NCC+Sobel 拼接引擎）、标注工具栏、跨平台贴图浮窗（pin_window）、截图 OCR。依赖 xcap 0.9.6（crates.io 发布版）。

源文件：`crates/capx/src/`、`crates/desktop/src/screenshot_commands.rs`、`crates/desktop/src/screenshot_geometry.rs`。

---

## 1. 模块结构

| 模块 | 职责 |
|------|------|
| `capture` | `capture_all_monitors()` 截取所有显示器（RGBA + 物理像素尺寸 + 显示器坐标）；`capture_single_monitor(mon_x, mon_y)` 仅截指定物理坐标的单显示器（非 macOS 滚动热路径用，避免多屏冗余捕获）；`crop_region()` 裁剪选区→PNG；`crop_region_rgba()` 直接返回 `RgbaImage`（零 PNG 编解码）；`crop_region_rgba_direct(full_w, full_h, &rgba_bytes, x, y, w, h)` 从只读 `&[u8]` slice 逐行 `copy_from_slice` 裁剪（零全屏克隆，内存省 98%+，滚动热路径专用） |
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
  → 后台生产 task 30ms 截帧（非 macOS：`capture_single_monitor` 仅截目标屏 + `crop_region_rgba_direct` 零全屏克隆裁剪 + 热路径日志降级 info→debug/trace；macOS 走 `capture_all_monitors` + `crop_region_rgba`）
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

**底部暗常数尾裁剪（`content_tail`，每帧动态）**：每帧检测当前帧底部"无内容暗常数尾"——逐行判 max-min ≤ `CONTENT_ROW_MAXMIN`(30) **且** 最亮像素 luma < `CONTENT_TAIL_MAX_LUMA`(40)（双条件：暗 + 常数）。覆盖选区下半截恒定纯黑、滚动后期内容上移后选区底部露出的暗背景。`sticky_bottom` 仅首帧一次且依赖逐像素相等，无法应对动态暗尾；`content_tail` **每帧基于当前帧**检测（非首帧缓存），eff_bottom 每帧止于真实内容底 → append 永不带入暗尾 → 画布底部 strip 始终有特征，避免 canvas-anchored 锚点动态退化（常数模板假匹配 score≈1.0 或失配 stuck 死锁——release 实测「拼接一部分后停止」：前期内容填满无暗尾、后期暗尾动态出现时首帧 content_tail 已失效）。双判定的亮度条件防误判：高 luma 低对比渐变行（每行常数但亮）不会被当成暗尾。finalize 按 `sticky_bottom + content_tail`（最后一帧值）补回尾部。

**画布种子（首帧）用自身暗尾裁剪（init）**：`content_tail` 每帧检测的是**当前帧**（用于 curr ROI），但画布种子 = 首帧，首帧在 app 聚焦/滚动开始前由 setup 单独捕获、暗尾常大于已滚动后的第二帧（内容上移、暗尾缩小）。若 init 用第二帧的小暗尾裁首帧画布 → 残余暗尾留画布底部 → canvas strip 常数 → `canvas_has=false` 首帧即死锁（release 实测 296×160 矮选区「滚动不拼接」：画布全程不增长，finalize 只拼 170 行）。故 init 裁剪读**画布种子缓冲自身**的暗尾（`scan_content_tail_in(canvas_buf)`），保证画布底部停在首帧真实内容底、锚点不退化。检测核心抽出为 `scan_content_tail_in(buf, h)`（帧/画布缓冲共用），每帧 curr ROI 仍读当前帧。

**画布常数尾每帧自愈（trim + reseed）**：canvas-anchored 要求画布底部=真实内容，但底部可能变常数——首帧在 app **聚焦/前置之前**捕获为**整帧空白**（content_tail 无暗尾可裁）、滚动到内容末尾露出纯色背景、或 1D 假匹配 append 常数块。底部常数 → Sobel 退化 → 锚点失效。根治：**每帧**（非一次性闸门——滚动中底部可能【再次】变常数）先用 `canvas_bottom_constant()`（采样画布底 strip 的 max-min）轻量判定；常数则 `scan_canvas_constant_tail()`（逐行往上累加抽样像素运行 min/max，diff≥阈值即停——运行 min/max 而非单行 max-min，防垂直渐变被误判常数）测常数尾高度 `tail`：裁后仍 ≥ `keep_min`（eff_strip_h）→ **非破坏性裁掉常数尾**（只丢空白/纯色背景，不丢内容），锚点回到真实内容底、本帧继续匹配；画布几乎全常数（无内容可留）→ `reseed_canvas_from` 用当前内容帧**重建**锚点（破坏性，仅此极端情况）。日志：`[stitch] canvas constant tail trimmed: N rows`（自愈）或 `[stitch] canvas reseeded ...`（重建）。第 6 次回归根因：旧 `canvas_content_confirmed` 一次性闸门确认后终身跳过检查，滚动中底部再次变常数时永久死锁（NCC stuck=5 stationary 到 finalize，finalize 灰度兜底对常数画布 score≈1.0 假匹配拼错）；改为每帧自愈——"治"而非一次"防"。

### 流程

1. **strip 提取**：从画布底部提取 `eff_strip_h` px（**自适应**：`min(strip_h, content_h/3)`，矮选区缩小留搜索范围，见下）
2. **Sobel 梯度特征图**（`imageproc`，纯色退化回灰度）
3. **NCC 模板匹配**（`best_ncc_match` → `imageproc::template_matching::match_template`，CrossCorrelationNormalized）
   - **双候选**：双侧均有 Sobel 特征时优先 Sobel NCC；Sobel validate 失配再追加**灰度 NCC 兜底**（灰度对比度有时比梯度更稳）
   - **退化不兜底**：任一侧 Sobel 退化（底部 strip 落暗色纯黑空白，`max_gradient==0` = 常数）时**跳过灰度**直接进降级链——常数模板灰度 NCC 必然 score≈1.0 假匹配（release 实测 `dy=-644.4` 重复假帧 append 污染画布 + `periodic false match sad=0.0`）
   - **大屏（帧宽 > `ncc_downsample_width` 默认 1920）走两阶段 refine**：
     - Triangle 降采样域粗定位 dy
     - 原分辨率 ±2px 邻域 `ncc_match_range` refine 恢复亚像素（避免降采样锯齿破坏 response 峰值）
   - 小屏单阶段
4. **验证**：score ≥ `ncc_score_threshold`（默认 0.65）+ response 无区分度拒绝（max-min<0.1）
5. **抛物线亚像素插值**

`strip_h` / `max_scroll` / `ncc_score_threshold` / `ncc_downsample_width` 纳入 `StitchConfig` 字段化（默认值不变行为零变化）。

**strip 高度自适应（`eff_strip_h`，矮选区）**：固定 `strip_h`(80) 对矮选区（如物理 162px 高含 80px 暗尾 → 内容 82px）会吃光 ROI 使 NCC 搜索范围≈0 → 首帧即失配死锁（release 实测「滚动没拼接」：画布几乎不增长）。每帧基于 content_h 算 `eff_strip_h = min(strip_h, content_h/3).max(MIN_STRIP=8)`（字段 `eff_strip_h`，模板提取 + 匹配几何 + 降级链全部读它而非 `config.strip_h`），留 2/3 content_h 作搜索范围。正常选区（content_h ≥ 240）eff_strip_h=80 不变；矮选区自动缩小（82 内容 → 27）。`detect_content_tail` 不再 clamp（自适应后 content_h≥3*strip 天然满足，整帧纯黑退化由 `eff_bottom<=eff_top` / ROI<strip 跳帧兜底）。ROI 不足一个 strip（sticky_top + content_tail 几乎吃光整帧）时跳帧，防 `quick_stationary_check` 越界。

---

## 5. 降级链

> 主匹配（`best_ncc_match`）已是 Sobel 特征 + 灰度双候选 NCC；以下降级链在双候选均 validate 失配后触发。

| 级 | 机制 | 触发 |
|----|------|------|
| 1 | **相邻帧参考 fallback**（`prev_gray` + `try_match_prev_frame`） | 内容突变失配时，用前一帧有效区匹配当前帧求正确 dy；prev 底部 strip 退化（常数，如选区下半截纯黑）时返回 None 不采纳——常数模板同样 score≈1.0 假匹配（release 实测 `dy=-247.5` 画布疯涨） |
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
