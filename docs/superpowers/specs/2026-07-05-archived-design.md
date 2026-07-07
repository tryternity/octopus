# 归档设计文档（archived-design）

> 归档日期：2026-07-05（实际整理 2026-07-12）
> 范围：2026-06-12 ~ 2026-07-05 的 15 篇设计 spec，按主题同类合并。
> 活跃文档：2026-07-06 及之后的 spec（scroll-stitch 系列、clipboard 系列、streaming-session-manager、vendor-paddle-ocr、clipboard-keyboard-nav 等）仍在 `specs/` 独立维护。

## 归档原则

- **现行实现全量保留**：本归档中所有 spec 均已对照当前代码核实，反映应用现状实现（非过期设计）——保留架构决策、数据结构、接口、不变量、边界用例。
- **同类合并**：同一主题的多篇 spec（图片查看器 4 篇、代码审查 3 篇、DB 2 篇）合并为统一章节，按演进顺序叙述、去除重复背景。
- **演进订正**：实施过程中算法/方案有偏离的（如 CAPX SAD→NCC、双 canvas→SVG overlay），在对应章节标注最终落地方案。
- **代码核实（2026-07-12）**：`crates/notepad` 已移除、`image_preview_window.rs`/`note_commands.rs` 已删、`asr-local/src/feature.rs` 存在、`parking_lot = "0.12"` 在 workspace 依赖、`infra/src/net.rs` 存在、DB `user_version = 18`——均与本归档描述一致。

## 目录

- [一、CAPX 滚动截屏模块优化](#一capx-滚动截屏模块优化)（原 `2026-06-12-capx-optimization-design.md`）
- [二、死代码/重复/超长代码重构](#二死代码重复超长代码重构)（原 `2026-06-12-refactor-deadcode-dup-long-design.md`）
- [三、ASR 光标定位与中间插入/选中替换](#三asr-光标定位与中间插入选中替换)（原 `2026-07-03-asr-cursor-insert-design.md`）
- [四、记事本移除 + 多 tab CompactEditor + OCR 统一](#四记事本移除--多-tab-compacteditor--ocr-统一)（原 `2026-07-03-clean-used-feature-design.md`）
- [五、图片查看器（性能/视口渲染/OCR 文本块/统一查看器）](#五图片查看器)（合并 4 篇）
- [六、外部项目分析参考](#六外部项目分析参考)（原 rapidraw / snow-shot analysis）
- [七、2026-07-05 代码审查批次](#七2026-07-05-代码审查批次)（合并 3 篇）
- [八、DB 表合并 + FTS5 搜索](#八db-表合并--fts5-搜索)（合并 2 篇）

---

## 一、CAPX 滚动截屏模块优化

> 原文：`2026-06-12-capx-optimization-design.md` ｜ 分支 `optimize-capx` ｜ 状态：✅ 实施完成（P1-P5 落地，10 测试全绿，API 零改动）

### 背景

CAPX 模块（`crates/capx/`）提供屏幕捕获（`capture.rs`）与滚动截屏拼接（`stitch.rs`）能力，被 `octopus-desktop` 的 `screenshot_commands.rs` 使用。改造前存在四类问题：

| 类别 | 问题 | 影响 |
|------|------|------|
| 性能 | `find_overlap_spatial_ext` 用 `GrayImage::get_pixel()` 逐像素访问 + f64 累加，模板每 y_offset 重复扫描 | 拼接热路径慢 |
| 性能 | 每次拼接 `RgbaImage::new` + 两次 `copy_from`（旧画布整体复制） | 大画布 O(N²) 内存复制 |
| 重复 | `capture.rs` 三处几乎一样的 CGImage 解析 + BGRA→RGBA 样板 | 维护负担 |
| 健壮性 | 核心匹配 + sticky 检测零测试覆盖；魔法数字散落 | 改参数即可能引入回归 |

### 目标与非目标

- **目标**：SAD 热路径提速（连续内存 + 整数 SAD + 模板预提取）；画布追加从 O(N²) 降到 O(new_rows) 增量 `extend`；消除 capture.rs 重复；提取魔法数字；拆分长函数；补合成图单元测试。
- **非目标**：不改对外 API（`Stitcher::new/process_frame/finalize/canvas/height` 与 `capture::*` 签名语义不变，desktop 零改动）；不引入 SIMD intrinsics；不加 criterion 基准。

### 设计

**新数据结构（`stitch.rs`）**：引入连续 row-major 灰度 buffer `GrayBuf` 替代 `image::GrayImage`，消除 `get_pixel()` 开销，提供 `row(y) -> &[u8]` 整行切片直访。`GrayBuf::from_rgba` 灰度公式 `(2126*R + 7152*G + 722*B)/10000`（整数除法，源自 image 0.25 `SRGB_LUMA`），须与 `image::imageops::grayscale` 逐像素相等。

**画布改造**：`Stitcher` 画布底层从 `RgbaImage` 改为 `canvas_buf: Vec<u8>`（连续 RGBA，真实数据源，增量 `extend_from_slice`）+ `canvas_cache: Option<RgbaImage>`（惰性重建缓存，append 后 invalidate）。`canvas(&self)` 行为不变：cache 为 Some 直接返回，为 None 时从 `canvas_buf` + `canvas_w/h` 用 `RgbaImage::from_raw` 重建。调用端（`screenshot_commands.rs`）对 `canvas()` 总是 `.clone()`，故惰性重建是一次性成本，借用不跨多次 append 存活——desktop 零改动。惰性缓存的内部可变性用 `unsafe`（`&self` 下写 `canvas_cache`，函数式惰性求值标准模式，单线程访问安全）或 `RefCell`。

**画布增量追加（O(N²)→O(new_rows)）**：

```rust
// 旧：分配整块 + 两次 copy_from
let mut combined = RgbaImage::new(w, old_h + new_rows);
combined.copy_from(&self.canvas, 0, 0)?;
combined.copy_from(&new_content, 0, old_h)?;
// 新：直接从 frame 切出 new_rows 行 RGBA，extend
self.canvas_buf.extend_from_slice(&frame_raw[start..end]);
self.canvas_h += new_rows;
self.canvas_cache = None;
```

`process_frame`/`finalize` 中裁剪 `new_content` 时直接操作 `frame` 的底层 `Vec<u8>` 切片，避免 `crop_imm().to_image()` 中间分配。

**capture.rs 去重**：提取 `#[cfg(target_os="macos")] fn cgimage_to_rgba(cg_image) -> Result<(Vec<u8>, u32, u32)>`，三个 macOS 捕获函数（`capture_display_excluding_window` / `capture_region_excluding_window` / `capture_window_region`）的 CGImage 解析 + BGRA→RGBA 统一调用。BGRA→RGBA 字节重排抽为平台无关纯函数 `bgra_to_rgba(raw, &mut rgba)`（非 macOS 也可单测）。

**魔法数字提取为模块常量**：`STRIP_H=80`、`MAX_SCROLL=220`、`STATIONARY_SAD=2.0`、`SAD_ACCEPT=7.5`、`MIN_CONFIDENCE=0.15`、`SPEED_PENALTY=0.04`、`X_START_RATIO=0.10`、`X_END_RATIO=0.80`、`SAMPLE_STEP_X=2`、`STICKY_DETECT_MAX=80`。

### 测试策略

合成图单元测试（不依赖真实截屏、不引入 criterion），内联 `#[cfg(test)] mod tests`：
- `make_frame(w, h, scroll_offset)` 构造带强空间特征的合成帧（y 渐变 + 每 45px 水平线 + 每 7 列亮列 + 确定性格点噪点）；`make_frame_with_sticky` 构造 sticky 顶/底。
- 已知位移检测、静止帧返回 false、sticky 检测、画布高度不变量（连续 process_frame 追加 new_rows 之和 = final_h - initial_h）、周期性内容不误匹配、finalize 补缝、`bgra_to_rgba` 行为、`GrayBuf` 与 `image::grayscale` 逐像素相等。

### ⚠️ 演进订正：SAD → NCC（核心算法已变）

> 本 spec 原描述的「SAD 热路径整数化重写（§3.3）」**未按本 spec 落地**。实施后该函数已整体删除，算法路线从 **2D SAD 空间模板匹配** 改为 **NCC + `row_projection_means` 行投影**，后续 20+ 个 `fix(capx)`/`feat(capx)` commit 围绕新路线迭代（NCC 假匹配 / 周期性假匹配 / 滚动断裂等）。
>
> **仍现行有效的部分**：`cgimage_to_rgba`/`bgra_to_rgba` 去重、魔法数字提取、`GrayBuf` 连续灰度 buffer、画布 `canvas_buf: Vec<u8>` + 惰性 `canvas_cache`、合成图测试网（现 33 处测试标记）。这些是数据结构与质量优化，与匹配算法无关，仍是当前代码。算法细节以 `crates/capx/src/stitch.rs` 实际代码为准（另见 2026-07-06 scroll-stitch 系列 spec）。

---

## 二、死代码/重复/超长代码重构

> 原文：`2026-06-12-refactor-deadcode-dup-long-design.md` ｜ 状态：✅ 已完成（merge `903a66a` + 后续扩展，84+42 tests + tsc 0 error 全绿，0 warning）
> 来源：代码审查报告 `docs/code-review/2026-06-12-code-review.md`
> 决策：Q1=A（分层提取 + 局部 TDD）、Q2=A（coordinator 仅拆内部长函数，先不拆子目录）

### 目标

清理 4 类问题，**行为零变化**为最高约束：
1. paddle-ocr 工具函数集中（`l2` 3 处、`saturate_cast_i16_from_f32` 2 处完全相同）
2. clamp/clip 命名统一（`clip_i32` hi_exclusive vs `clamp_i32` hi_inclusive）
3. `start_scroll_recording`（502 行）拆分
4. `Coordinator::new`（331 行）/ `begin_recording`（228 行）拆分

### 不变量（全任务共用）

- **INV-1 行为保持**：`cargo test --workspace` 全绿是必要非充分条件。
- **INV-2 无 API 变化**：`#[tauri::command]` 签名、`pub fn` 接口签名不变。
- **INV-3 平台守卫保留**：所有 `#[cfg(target_os = "macos")]` / `#[cfg(not(...))]` 分支等价保留。
- **INV-4 无新依赖**：不引入新 crate（允许用已有 `bytemuck`/`ndarray`）。
- **INV-5 单 commit 单任务**。

### TDD 策略矩阵

| 任务 | 类型 | 策略 |
|---|---|---|
| #1 paddle-ocr `l2`/`saturate_cast` 集中 | 纯函数迁移 | **严格 TDD**：先写参数化测试锁住边界，再迁移到 `vision/numeric.rs` |
| #2 clamp/clip 命名统一 | 纯函数改名 | **严格 TDD**：先写 inclusive/exclusive 测试，再改名 |
| #3 `start_scroll_recording` 拆分 | 混合（纯逻辑 + 平台/异步） | **分层**：纯函数（坐标换算、preview crop、显示器命中）严格 TDD；平台/异步编排靠 `cargo check` + 全量测试 + 行为保持 review |
| #4 `Coordinator::new`/`begin_recording` 拆分 | 混合（状态机 + 平台） | **分层**：状态机辅助逻辑提纯函数 TDD；Tauri State/channel/spawn 靠编译器 + 全量测试 |

**TDD 红线**：不引入 trait 抽象 mock 平台 API（YAGNI）；不修改 `#[tauri::command]` 签名；纯逻辑提取后必须立即写测试；每个 commit 后 `cargo test --workspace --exclude octopus-desktop` 全绿（desktop 因前端 dist 缺失无法在 worktree 编译）。

### 各任务设计

**Task 1：paddle-ocr `vision/numeric.rs`**。新建 `crates/paddle-ocr/src/vision/numeric.rs`，集中（均 `pub(crate)`）：`l2`、`cv_round_ties_even_f32`、`saturate_cast_i32_round`、`saturate_cast_i16_from_f32`、`saturate_cast_i16`、`interpolate_cubic_coeffs`、`clip_i32_exclusive_upper`（原 `clip_i32` 改名）、`clamp_i32_inclusive`（原 `clamp_i32` 改名）。`resize.rs`/`rotate_crop.rs`/`word_boxes.rs`/`postprocess/mod.rs` 删本地副本改引用。统一版本含 `is_finite` 检查（NaN/Inf→0，更安全）。

**Task 2**：clamp/clip 改名在 Task 1 迁移过程中一并完成，所有调用点同步更新。

**Task 3：`start_scroll_recording` 拆分**。提取纯逻辑到新建 `crates/desktop/src/screenshot_geometry.rs`（不依赖 Tauri/Quartz 类型）：
- `MonitorRect { x, y, w, h, scale }`、`SelectionGlobal`、`PhysicalCrop { px, py, pw, ph }` struct
- `compute_selection_global`（窗口原点 + CSS 偏移 → 全局逻辑坐标）
- `find_monitor_for_point(monitors, cx, cy) -> Option<usize>`
- `compute_physical_crop(sel, mon) -> PhysicalCrop`
- `compute_preview_crop(canvas_h, canvas_w, preview_w, max_preview_h) -> (crop_src_h, crop_y)`（消除两处重复 preview crop 逻辑）

不提取（保留在主体）：Quartz `CGDisplay`/`get_window_number`、`tokio::spawn` 编排、Tauri `set_ignore_cursor_events`/`emit`、鼠标轮询线程、PNG/WebP 编码 + DB 入库。主体从 502 行降到 ~150 行。

**Task 4：`Coordinator::new`/`begin_recording` 拆分**（文件路径不变，Q2=A 不拆子目录）：
- `new` 提取 `build_coordinator_loop`（`std::thread::spawn(move || { loop {...} })` 内部状态机循环体）为独立函数，`new` 仅做 channel 创建 + config 预处理 + spawn。
- `begin_recording` 按引擎分支拆出 `prepare_streaming_session` / `prepare_cloud_streaming_session`（`#[cfg(feature="cloud")]`）/ `prepare_vad_segmented_session`，主体仅做 `audio.start` + 分支选择。`#[cfg(feature="cloud")]` 守卫在参数和分支上对称。

### 后续扩展（原计划范围外，已实施）

5. **`postprocess/mod.rs`（2226 行）拆分**：拆为 `threshold.rs`/`contour.rs`/`box_score.rs`/`geometry.rs`/`unclip.rs`/`filter.rs`/`tests.rs` 7 个子模块，`mod.rs` 仅留 struct/impl + scratch 类型 + 模块声明。纯文件搬移无逻辑变更。

6. **前端超长组件拆分**：`Screenshot/index.tsx`（1170→960，提取 `ToolButton`/`ToolPropsPopover`/`ScrollPreview`）、`Result/index.tsx`（869→787，提取 `shortcut.ts`/`CaretBlink`）、`ImagePreview/index.tsx`（850→832，提取 `zoom.ts`）。

7. **DB 写入队列提取 + 防御**：`db_queue.rs`（提取 `DbCommand` enum + `DB_SENDER`/`DB_SHUTDOWN`/`DB_HANDLE` static + `process_db_command`/`drain_db_queue`/`shutdown_db`/`get_db_sender`，coordinator.rs 2489→2309 行）；`compute_physical_crop` 加 `.max(0.0)` 防御跨显示器边界负值 wrap + 回归测试。

---

## 三、ASR 光标定位与中间插入/选中替换

> 原文：`2026-07-03-asr-cursor-insert-design.md` ｜ 状态：✅ 全部合入 main
> 相关代码：`crates/desktop/src/transcript.rs`（段模型核心）、`coordinator.rs`（编排）、`pipeline.rs`、`result_window.rs`、`frontend/src/pages/Result/`、`crates/infra/src/db.rs`（v13→v14 迁移）
> 关键 commit：段模型基石 `c20eb35` / `set_caret` `f2ca142` / 选中替换 `b961f8e` / `append_segment` 对称消费 `9d4a654` / 编辑后光标归末尾 `f32f1a9` / vitest 基建 `e797e0f` / 跨会话方案 C `a79ab97` / Bug C cloud 对称 `1f3e162`

### 背景

Result 语音识别窗非编辑态原为 `<div contentEditable={false}>` 无光标，识别文本由 `Transcript` 单一 `full: String` 只从末尾增长。需求：非编辑态也显示闪烁光标可点击定位；点在文本中间后新语音实时从光标处流式插入（原光标后文本右推）；显示/落库/复制顺序一致（真中间插入，非视觉假象）；默认（不点光标）行为与逐字一致（零回归）。

### 关键设计决策

1. **路线：段（segment）模型**。废弃 `raw_text/polished_text/edited_text` 三字段 + `edited≻polished≻raw` 优先级链，改为 `Vec<Segment>`，每段自带类型（`Raw`/`Polished`/`Edited`，后态覆盖前态）。
2. **插入方式：实时流式插入光标处**。点击即更新 `caret_gap`，下一段 delta 立即去新光标；旧活动 Raw 段自然冻结。
3. **光标移动时机：立即切换**。VadSegmented 天然按段边界回填无劈词风险；流式引擎存在「半词被劈」的罕见代价（可接受）。
4. **润色：一次全篇调用**。`edited` = 冻结（preserve verbatim，best-effort，唯一可信源）；`raw → polished`；`polished → 重润`（用最新全篇上下文）。调用后无 `raw` 段。
5. **自动停顿润色（mode=2）无需禁用**：段模型下润色与光标位置无关，中间插入态照常触发。
6. **编辑态**：进编辑显示 `finish_text()`；`commit_edit(flat)` → `segments = [Edited(flat)]`、`caret_gap = 1`；raw/polished 清零。
7. **`finish_text`**：段扁平化纯文本，**派生**（不另存），供 display/落库搜索/clipboard。
8. **编辑后原始 raw ASR 不再保留**（行为变更，已确认接受）。

### 数据结构（`transcript.rs`）

```rust
pub enum SegmentKind { Raw, Polished, Edited }
pub struct Segment { pub kind: SegmentKind, pub text: String }

pub struct Transcript {
    pub id: i64,
    mode: PolishMode,
    segments: Vec<Segment>,           // 结构化真相源
    caret_gap: usize,                 // 新语音生长缝隙 0..=len；==len 即末尾追加（默认/今天行为）
    engine_cumulative: String,        // 引擎累积全量，仅作 delta 提取基准。不显示不落库
    engine_consumed_chars: usize,
    // ... polish_snapshot / polish_caret_offset / pending_delta / polish_pending / db_inserted
    pending_delete: Option<(usize, usize)>,  // 选中替换待删范围（§11），运行时态不入库
}
```

核心方法：
- `finish_text()` / `display_text()`（alias）/ `full()`（alias）/ `db_text()`（alias）→ 段顺序拼接纯文本。
- `apply_engine_full(full) -> bool`：取尾部 delta（`full` 非 `engine_cumulative` 前缀 = diverted → 重算基准、丢弃本次 delta、不回退已展示），在 `caret_gap` 处生长 Raw 段。**delta 追踪与光标位置无关**（`engine_consumed_chars` 跨光标移动连续递增）。
- `append_segment(delta)`：VadSegmented 的 delta 直接生长，走同一 `push_delta_at_caret`。
- `push_delta_at_caret(delta)`：前邻段为 Raw 且光标在其尾 → 追加该段；否则插入一条空 Raw 段到 `caret_gap`。
- `set_caret(char_off)`：遍历段累计 char 定位 gap；落段内 → 劈段（同 kind 一分为二）；`clamp [0,len]`。
- `caret_char_offset()` / `is_inserting()`（`caret_gap < len`）/ `has_raw()`。
- `commit_edit(flat)`：整篇压成单 Edited，raw/polished 清零。
- `polish_apply(full)`：润色结果回填，edited 串匹配定位、间隙 Polished、连续非 edited 合并；恢复 caret 到发起 char offset；flush `pending_delta`。润色失败 `on_polish_failed` 清 pending + flush pending_delta（保留新语音）。

**默认零回归**：`segments=[]`+`caret_gap=0` 等价旧空文档；`caret_gap==len` 等价旧「末尾追加」。**两条类型不变量**：① 润色后无 Raw；② 编辑后只剩 Edited。

### 数据流

**A. 流式插入**：引擎 tick 给累积 `full` → `apply_engine_full` → `push_delta_at_caret` → coordinator emit `update_result(finish_text, insertion)`（`insertion = caret_gap != len`）。

**B. 光标定位（点击）**：前端算点击处在 `finish_text` 的 char offset（code-point 计数）→ `invoke("set_caret", {offset})`。立即切换，后续 delta 从新 gap 生长。

**C. 润色（手动/最终/中间自动，均全篇一次）**：segments 带类型送 LLM（edited 标 preserve，连续非 edited 视为一个 polish 区）→ LLM 全篇一次 → 按 verbatim 定位 edited 段（best-effort 串匹配，LLM 擅改则接受输出、kind 仍 Edited）→ 间隙填 Polished。

**D. 编辑态**：textarea 显示 `finish_text()`；`commit_edit(flat)` → `[Edited(flat)]`；取消恢复快照。

**E. 停止/落库/粘贴**：flush 尾部 → 最终 segments 落库（`segments` JSON + `text` 列）；clipboard/粘贴用 `finish_text()`。下次录音新建 Transcript。

### 前端光标（`Result/index.tsx`）

- 自定义闪烁光标（不用 `contentEditable`——会引入键盘/IME 问题），纯定位指示器。`caret_gap` → `finish_text` char offset → 用 Range 量像素，绝对定位 1px 宽竖条，CSS `@keyframes blink`。
- 点击：`caretRangeFromPoint` 算 char offset（code-point，与 Rust `char` 对齐）→ `invoke("set_caret")`。
- **`update-result` 渲染策略（关键）**：中间插入时 `finish_text` 在中间变化、非尾部延伸 → 会被前端误判 diverted（300ms 延迟）→ 卡顿。修法：后端 `update-result` 附带 `insertion: bool`，前端在插入态直接立即整体渲染（跳过 300ms 延迟）；diverted 延迟仅保留给「光标在末尾 + 引擎纠正」场景。

### DB 迁移（v13 → v14）

`transcriptions` 表新增 `segments TEXT`（JSON `[{kind,text}]`）+ `text TEXT`（= `finish_text` 扁平，denormalize 给 search/clipboard 直接读）。旧记录按 `edited≻polished≻raw` 映射为单段。`search_transcriptions` 改查 `text` 列（`WHERE text LIKE ?`）。`update_edited_text` → `update_segments(segments, text)`。`clipboard_history` 不变（本就存扁平 content）。

> **后续（v15，2026-07-04）**：rusqlite `bundled` feature 自带 SQLite ≥ 3.45 支持 `ALTER TABLE ... DROP COLUMN` 无需重建表，v15 迁移已 DROP 旧三列（信息全在 segments/text）。再后续 DB 合并（见本归档第八节）将 transcriptions 整表并入 clipboard_history。

### §11 追加特性：选中替换（Selection Replace，2026-07-04）

非编辑态**拖选**一段文字 → 在选中处说话 → **首个词到达时**删掉选中文字、识别文字从该处插入。区别于中插（保留原字、新词右推）：选中替换是「删旧换新」。用户原话明确是**开口才删**，非选中即删。

**关键决策：延迟到首词删（`pending_delete`）**。`Transcript` 加运行时态 `pending_delete: Option<(usize, usize)>`（扁平 char 范围 [start,end)）。`set_selection(start,end)` 只记录待删范围 + 把 `caret_gap` 劈到 start（**不删字**，保留浏览器原生高亮反馈）。**两条 delta 入口**——`apply_engine_full`（流式 local：zipformer/paraformer streaming）与 `append_segment`（VadSegmented 离线：sensevoice/firered/qwen3/whisper + cloud partial 拼接）——都在首个非空 delta 插入前消费 `pending_delete`：`delete_range(start,end)` 真删 + 走 `push_delta_at_caret`。不采用「立即删」：与「说话时才删」意图相悖、误操作不可逆、延迟到首词保留高亮反馈/取消容易。

**三个消费点**：`apply_engine_full`、`append_segment`、`take_polish_input`（润色快照基于删后文本）。**清除点**：`set_caret`（取消）、`on_polish_failed`、`commit_edit`、`exit_edit_without_commit`（CancelEdit，c92955e 防「选中→Esc→下次说话幽灵删除」）、`new`。`pending_delete` 是运行时态不入库。

**⚠️ §11.7 通用教训（9d4a654）**：初版漏 `append_segment` 消费 `pending_delete`，致离线/cloud 引擎选中后首词只插不删、选中文本残留。**`Transcript` 有两条 delta 入口，任何「首词触发」型运行时状态都必须在两入口对称消费**，否则只在对应引擎下失效。

**§11.6（f32f1a9）**：编辑保存后闪烁光标错落首位。根因 `CaretBlink` 原把 `container={textRef.current}` 当 prop——render 阶段求值时是旧值，保存时 `editing` true→false 致 textRef `key` 从 `"edit"`→`"view"` 重挂载，effect 去量即将卸载的旧 div，`getBoundingClientRect()` 返回 (0,0)。修复：`CaretBlink` 改接收 `RefObject`，在 effect 内读 `.current`。**通用教训：React 中把 `ref.current` 作为子组件 prop 传递有 render-commit 滞后陷阱，重挂载场景应传 RefObject 在 effect 内读取。**

### §11.8 跨会话选中替换（方案 C）+ Bug C cloud 对称（2026-07-05）

活跃态选中替换（录音中）之外，跨会话维度：录音结束（Idle）后选中已识别文本 → 全局热键 Toggle 开新会话 → 新语音替换选区（而非追加末尾）。

**方案 C（a79ab97）**：移除后端 `idle_selection` 长期缓存（失焦残留 / 编辑后 stale text 指错位 / 拖选后编辑残留三类 bug），改前端 `currentSelectionRef` 缓存 `{start,end,text}`；Toggle 在 Idle **不直接开录音**——`emit("prepare-record", prepare_id)` + 200ms 看门狗 + `pending_prepare` 等待态；前端 listen prepare-record → `invoke("start_recording", {prepare_id, selection: [text,start,end]|null})`（**注意 `selection` 是 tuple `(String,usize,usize)`，serde 按 JSON 数组反序列化，前端必须传数组非对象**）；后端校验 prepare_id 后调 `begin_recording(selection)`：`Some` → `commit_edit(text)`+`set_selection(s,e)` 种子（复用 §11 的 `pending_delete` 机制，首个 delta 删旧插新）；`None` → 普通开。看门狗超时 `FallbackStart` 兜底。

**Bug C（1f3e162）**：`begin_recording` 的三分支（cloud/streaming/vad）必须**对称植入** selection 种子。初版 cloud 分支直接 `Transcript::new` 空实例、漏消费 `selection`，致云端跨会话选中替换退化为末尾追加。根因与 §11.7 **同构**——状态植入/消费须在所有引擎路径对称（活跃态两 delta 入口 + 跨会话三分支），任一漏即在该路径下失效。修复后 `SetSelection` 命令仅携带 `{start,end}`（试行期误加的 `text` 字段同批清理）。

### §12-§15 前端渲染健壮性修复（e2e + 代码审查多轮）

全集中在 `Result/index.tsx`（及其抽出的 `caret.ts`）。

- **§12.1 文字不渲染（最关键）**：React 19 对 `contentEditable` 容器的 children commit 不写 DOM（即使 `contentEditable={false}`、即使 `flushSync`），流式 `setText` 改了 state 但 DOM textNode 始终旧 → 文字空白。修复：`renderResultNow` imperative `textRef.textContent = newText`（非编辑态）绕过 React；`measureCaretPx` 长度改读 DOM `firstText.nodeValue`。
- **§12.2**：闪烁光标 px 是视口相对值随 `scrollTop` 变，加 scroll 监听（passive + rAF 节流）重测；视口外（`px.top < -2 || > clientHeight+2`）`return null` 隐藏。
- **§12.3**：onScroll 恢复 stickToBottom 时立即 `scrollTop = scrollHeight`，不等 tick。
- **§12.4**：textRef div 加 `whitespace-pre-wrap`（否则编辑态 `\n` 折叠成空格）。
- **§12.5**：`measureCaretPx` 末尾态曾优化为 `selectNodeContents+collapse(false)` 锚容器边界，Chrome 常返 zero rect 触发兜底 (0,0) → 光标首位。回退为文本节点内 `setStart(firstText,offset)+collapse(true)`。
- **§13.1**：show-result else 分支（最终/插入态立即渲染）漏清 pending diverted 计时器 → 300ms 后旧回调覆盖最终文本。修复：显式 `clearTimeout(divertedTimer)` + 清 `pendingDiverted`。
- **§13.2**：`CaretBlink` 初始 measure 同步 `getBoundingClientRect` 与同帧 DOM 写叠加 → layout thrashing。改 `requestAnimationFrame`（代价 1 帧 ~16ms 滞后，肉眼无感）。
- **§14.1**：抽 `locateCpOffset(container, pos)`（TreeWalker 收集全部 text node，按 code-point 长度累加定位落点 + UTF-16 offset），`measureCaretPx` 与 `placeCaretAtCodePoint` 共用，修多 text node 错位。
- **§14.2**：进编辑态光标无条件落末尾 → `caretPosRef` 捕获 `restorePos`，`placeCaretAtCodePoint(el, restorePos)` 精准恢复（拖选置 null，故拖选后进编辑仍落末尾——设计如此）。
- **§15 前端拖选三重陷阱**（从右往左选到开头失效）：① `Range.startContainer` 飘移到父容器 → `clampRangeToContainer` 用 `compareBoundaryPoints` 裁剪；② React `onMouseUp` 不在 textRef 外触发 → `onMouseDown` 在 `document` 注册一次性 mouseup listener；③ mouseup 时鼠标在容器外 → `getBoundingClientRect()` 判断 X 坐标方向（左外→offset=0，右外→末尾）；④ `mouseDownOffsetRef` 缓存起点 offset，mouseup 时 min/max 重建选区（不依赖 mouseup 瞬间 DOM Selection）。**教训：WKWebView 中拖选到容器边界是高频踩坑区，不能直接信任 `window.getSelection()` 原始值，须三重防御 + 缓存兜底。**

**通用教训汇总**：① contentEditable 容器的 React children reconcile 不可靠，流式更新须 imperative 同步；② 视口相对像素须随 scroll 重测；③ CSS `white-space` 决定 `\n` 可见性；④ collapsed range 的 `getBoundingClientRect` 锚点敏感。

---

## 四、记事本移除 + 多 tab CompactEditor + OCR 统一

> 原文：`2026-07-03-clean-used-feature-design.md` ｜ 分支 `worktree-clean-used-feature` ｜ 状态：✅ 已合 main
> 主题：移除记事本子系统、CompactEditor 升级为多 tab 常驻编辑器、统一三处 OCR 入口、剪贴板新增 OCR 类别

### 背景与目标

记事本（Notepad）原是独立持久化笔记库（`notes` 表 + FTS5 + 前端页面 + 托盘入口 + 12 个 tauri 命令），与剪贴板历史定位重叠（OCR/ASR 结果既入剪贴板又入笔记，数据双写、入口分散）。CompactEditor 原是无状态单文档编辑器（请求-响应回传一段文本）。

四个目标内在统一：**移除记事本后，CompactEditor 升级为多 tab 常驻编辑器承担「多条目查看/编辑」职责；OCR 文本归宿从「笔记」改为「剪贴板 OCR 类别」，编辑统一在 CompactEditor 的 tab 里完成。**

### 任务 1：记事本清除

- **Rust**：删 `crates/notepad/` 整个 crate（workspace members 移除）；删 `crates/desktop/src/notepad_window.rs`、`note_commands.rs`；`main.rs` `generate_handler!` 移除全部 note/notepad 命令；`tray.rs` 移除 `notepad` 菜单项 + handler；`screenshot_commands.rs` 删 `open_notepad_with_content`，`ocr_screenshot` 改造（见任务 3）。
- **前端**：删 `pages/Notepad/`、`types/note.ts`、`hooks/useNotes.ts`、`lib/notepad.ts`；`App.tsx` 移除 Notepad 路由；`HistoryPanel.tsx` 移除「保存为笔记」按钮。
- **capability**：`capabilities/default.json` `windows` 数组移除 `"notepad_window"`。
- **DB 迁移 v12 → v13**：先 `DROP TABLE notes_fts`（含触发器依赖）再 `DROP TABLE notes`；`clipboard_history` 无 schema 变更（OCR 复用 engine/model 列）。

> ⚠️ **演进**：记事本移除后，CompactEditor 多 tab 形态又被 [统一查看器 spec（本归档第五节）](#五图片查看器)进一步演进——Tab 扩展为 `{ key:'${source}:${itemId}', source:'clipboard'|'transcription', itemId, itemType?, text? }`，`dirty`/`title` 移除。命令签名以演进版为准。

### 任务 2：CompactEditor 多 tab 改造

单例窗口内多 tab，每个 tab 绑定一个剪贴板条目 `item_id`（后演进为多 source）。tab 状态由前端持有。交互：打开 tab（`openCompactEditorTab(itemId)`，已开则 activate）、加载内容（`get_clipboard_item_text(itemId)`）、编辑、Ctrl+S 保存（`set_clipboard_item_text`）、关 tab。

后端命令（`compact_editor_commands.rs`）：
- **新增** `open_compact_editor_tab(item_id, source?, app_handle)`：窗口未开 → store pending + 建窗；窗口已开 → `emit compact-editor://open-tab {item_id, source}`。
- **新增** `get_pending_compact_tab() -> Option<{item_id, source}>`。
- **新增/复用** `get_clipboard_item_text(item_id) -> String`。
- **删除** 旧请求-响应机制：`open_compact_editor(initialText, requestId)`、`get_pending_compact_edit`、`compact-editor://load`/`://result`/`://cancel` 事件、`CompactEditPayload`。
- 保留 `close_compact_editor`、`set_clipboard_item_text`。

### 任务 3：OCR 统一新流程

- **`ocr_screenshot`（`screenshot_commands.rs`）**：改为纯识别返回 `Result<String>`，剥离入库图片/update_search_text/write_text/open_notepad，保留 `close_all_screenshot_windows`。
- **`ocr_image`（`clipboard_commands.rs`）**：不变（已纯识别返回 text，后演进为返回结构化 `{text, blocks}`，见第五节）。
- **新增 `insert_ocr_clipboard_item(text) -> Result<i64>`**：后端读 `ocr_model` config + OCR 引擎信息自填 engine/model，调 `insert_ocr_item` 入 `source='ocr'` 条目返回 `item_id`，`emit clipboard://changed`。
- **删除** `save_ocr_to_note`、`open_notepad_with_note`。
- 三处入口（截图工具栏 `doOcr` / 图片预览 `handleOcr` / 剪贴板图片条目 `handleOcr`）统一：识别 → `insert_ocr_clipboard_item` → `openCompactEditorTab(itemId)`。

**运行时约束（e2e 阶段增强）**：
- **全局并发互斥**：`OcrLockGuard`（`AtomicBool` + `compare_exchange` RAII guard，drop/async cancel 自动释放）在 `ocr_image`/`ocr_screenshot` 入口 `try_acquire`，忙则立即 `Err("前一个 OCR 还未完成，请稍后")` 不进推理。
- **超长图切分**：`height > 1600`px 长截图按块（高 1280、重叠 200）切分逐块识别 + 末行去重合并，避免整图缩放到 det `max_side_len=960` 致短边过小检测不到文本。

### 任务 4：OCR 类别数据结构

- **`clipboard/src/model.rs`**：`Source` 枚举新增 `Ocr`（容错 `from_str` 未知值回落 `Clipboard`）；`OcrMeta { engine, model }`；`ClipboardItem` 加 `ocr_meta: Option<OcrMeta>`。
- **`store.rs`**：`insert_ocr_item(conn, text, ocr_meta) -> Result<i64>`；`build_where` 加 `"ocr" => "source = 'ocr'"`；`row_to_item` 在 `source='ocr'` 时反序列化 `ocr_meta`。
- **前端**：`Source = "clipboard" | "asr" | "ocr"`；`FilterTabs` 加 OCR tab；OCR 条目用 `ScanText` icon。

> ⚠️ **后续演进（DB 合并，见第八节）**：`source` 列后来被 `item_type`（text/voice/ocr/image/file）取代，OCR 类别从 `source=ocr + item_type=text` 改为 `item_type='ocr'`，`OcrMeta` 并入统一 `MetaInfo`。

### CompactEditor 代码审查追加修复（2026-07-04）

多 tab 合入后多轮审查发现的前端健壮性 bug（`pages/CompactEditor/index.tsx`）：
- **replaceOne 焦点跳转**：基于替换后 next 文本重新 `collectMatches`，`matchIdx = Math.min(matchIdx, len-1)`。
- **replaceAll 大小写不匹配**：`new RegExp(escaped, "gi")`（escape 正则元字符）。
- **mount 监听泄露**：加 `cancelled` 标志，防 StrictMode/快速 unmount 下 `unlisten` undefined 泄漏。
- **keydown 监听器每键重建**：`doSaveRef = useRef(doSave)` + 同步赋值 effect，监听器改调 `doSaveRef.current()`，deps 去掉 `doSave`。
- **键盘 undo/redo 失灵（按钮正常）**：受控 textarea 每次 value 同步清空 WebKit 原生 undo 栈 → 键盘失灵；按钮 `execCommand` 走文档级事务栈可用。修复：keydown 拦截 `mod+(z|y)` → `preventDefault` + `document.execCommand(isRedo?"redo":"undo")`，键盘与按钮统一走 `execCommand`。

---

## 五、图片查看器

> 合并 4 篇原 spec（均分支 `image-viewer-perf`，全部 ✅ 已合 main）：
> - `2026-07-03-image-viewer-perf-design.md`（视口渲染 + SVG overlay）
> - `2026-07-03-viewport-rendering-v2-design.md`（视口渲染 v2）
> - `2026-07-04-ocr-text-blocks-design.md`（OCR 文本块可视化）
> - `2026-07-04-unified-viewer-design.md`（统一内容查看器）
>
> 相关代码：`crates/desktop/frontend/src/pages/ImagePreview/`（现为**组件**，非路由页面）、`pages/CompactEditor/`

### 5.1 整体演进脉络

ImagePreview 经历三轮架构演进，最终成为**嵌入 CompactEditor tab 的可控组件**（不再是独立路由窗口）：

1. **单 canvas（整张图）+ 标注全量重绘** → 画笔拖动掉帧。
2. **双 canvas（bgCanvas 底图+已确认标注 / drawCanvas 绘制预览）** → 4K 图仍慢。
3. **单 canvas（底图）+ SVG overlay（标注脱离 canvas）** → 标注变化只更新 SVG DOM，零 canvas 操作。commit `237713a`。
4. **视口渲染（canvas 恒定窗口大小只裁剪可见区域）** → 超大图 GPU 合成 buffer 从 174MB 降到 ~8MB。commit `9bca0de` + `a9faa39`（v2 最终方案）。
5. **统一查看器**：CompactEditor 升级为多 tab 统一查看器，ImagePreview 改为接收 `imageId` props 的组件，独立 `image_preview_window` 废弃删除。

### 5.2 视口渲染 v2（最终方案）

**问题**：2032×15796 超长图 canvas ~20-45M 像素，buffer 80-174MB，GPU 每帧合成 91% 不可见区域。v1 视口渲染尝试失败（canvas 与 scrollContainer 兄弟节点 + flex 居中 + padding + scroll 组合致坐标偏移不可靠）。

**v2 核心原则**：**彻底放弃 CSS flex 居中推导，所有定位用 `absolute` + JS 手算数值**。

```
<scrollContainer overflow-auto absolute inset-0>
  <content relative style="width: scrollW; height: scrollH">   ← 纯撑滚动条
    <wrapper absolute style="left: imgLeft; top: 56; width: dispW; height: dispH; ...棋盘格, ...onMouse">
      <svg overlay absolute inset-0 viewBox="0 0 natW natH" preserveAspectRatio="none" />  ← 标注
    </wrapper>
  </content>
</scrollContainer>
<canvas absolute inset-0 pointer-events:none zIndex:1 />      ← 在 scrollContainer 外，恒定窗口大小
```

**drawBg 完全手算（不依赖 DOM 查询）**：
```ts
const sl = sc.scrollLeft, st = sc.scrollTop, vw = sc.clientWidth, vh = sc.clientHeight;
canvas.width = vw * dpr; canvas.height = vh * dpr;
const imgLeft = Math.max(0, (vw - dispW) / 2);  // 居中（content 无 padding）
const imgTop = 56;
const imgVpX = imgLeft - sl, imgVpY = imgTop - st;
const visL = Math.max(0, -imgVpX), visT = Math.max(0, -imgVpY);
const visR = Math.min(dispW, vw - imgVpX), visB = Math.min(dispH, vh - imgVpY);
ctx.drawImage(bitmap || img, (visL/dispW)*srcW, (visT/dispH)*srcH,
              ((visR-visL)/dispW)*srcW, ((visB-visT)/dispH)*srcH,
              visL+imgVpX, visT+imgVpY, visR-visL, visB-visT);
```

`viewport` state（ResizeObserver）触发 re-render；滚动 RAF 直接调 `drawBg`（不走 React state，避免全组件重渲染）。wrapper 的 `left/top` 在 React render 和 drawBg 中用同一公式。`getBoundingClientRect` 不用于 drawBg（只用于 `canvasCoords` 的 scRect 左上角基准）。棋盘格底在 wrapper，canvas 没画到的不可见区域只显示棋盘格（瞬态，RAF 后画上图）。

**不变量**：① 所有定位 absolute + JS 手算；② canvas 和 scrollContainer 同为 `absolute inset-0`（同坐标系）；③ drawBg 只用 scrollLeft/Top + clientWidth/Height + 已知 imgLeft/imgTop；④ wrapper left/top 在 render 与 drawBg 用同一公式。

### 5.3 createImageBitmap 异步预缩放（缩放优化）

zoom 变化不在主线程对原图 `drawImage` 到超大画布，而先异步生成预缩放 `ImageBitmap`：
```
zoom 变化 → useEffect([zoom]) → setIsScaling → createImageBitmap(img, {resizeWidth, resizeHeight, resizeQuality:"high"})
  （GPU 加速）→ 版本匹配 → scaledBitmapRef = bitmap → drawBg(bitmap) → setIsScaling(false)
  期间 bgCanvas 保持上一帧（不清空，"稍糊但完整"瞬间切清晰，比白屏好）
```
关键细节：`createImageBitmap` 接受 `{resizeWidth, resizeHeight}` 浏览器 GPU 缩放不占主线程；旧 bitmap 必须 `.close()` 释放 GPU 内存；`zoomVersionRef` 防过时 zoom 值覆盖最新帧；快速连续 zoom 中间结果被最新覆盖。150ms debounce（`ebf9426`）期间 drawBg 用原图拉伸占位。

### 5.4 先 thumb 再 full 渐进加载（大图优化）

mount 时并行拉缩略图（`get_image_thumb` ~10ms）和全图（`get_image_full` 100-500ms），缩略图秒开占位，全图就绪无缝替换。`userZoomedRef` 标记用户是否手动缩放（thumb→full 替换时只在未手动缩放时重算 fitZoom）。`fitModeRef` 三态（fitWindow/fitWidth/manual）。ResizeObserver 自适应（窗口 resize 时非 manual 自动重算 zoom）。

**边界**：thumb→full 标注坐标系错位 → `loadingFullRef` 门控，全图加载完成前禁止标注（`tool !== "none"` 时 onMouseDown return）。blob URL 泄漏 → `objectUrlRef` 跟踪，切换/卸载 `revokeObjectURL` + `bitmap.close()`。`fullLoadedRef` 门控防 full 先到 thumb 后到覆盖。

### 5.5 OCR 文本块可视化

**后端**：`ocr_image` 返回值从 `String` 改为结构化 `OcrResult { text, blocks: Vec<OcrTextBlock> }`，`OcrTextBlock { text, x, y, w, h, score }`（自然像素坐标）。`recognize_with_blocks` 不丢弃 bbox；`recognize_long_image_with_blocks` 把每 chunk 的 top offset 加到 y 坐标合并。

**前端**：OCR 后在图片预览叠加文本块可视化层（独立 SVG overlay，与标注 overlay 并列，zIndex 更低）。OCR 按钮三态 toggle（off → overlay 半透蓝边框+蓝字 → mask 白底黑字 → off）；换图重置；双击文本块复制。首次 OCR = 触发识别 + 入库 + 编辑器 + 显示叠加层；后续点击 = toggle（不重新识别、不入库）。

**截图 OCR → 图片预览展示**：截图 OCR 不在全屏透明窗叠加（信息过载），改为关截图窗 → 开图片预览 tab → `emit("ocr-screenshot://result", {text, blocks})` → 图片预览 listen 展示叠加层。

**不变量**：① OCR 文本块坐标在自然像素空间（与标注一致）；② OCR 叠加层与 tool 状态正交。

### 5.6 统一内容查看器（CompactEditor 升级）

**目标**：CompactEditor 升级为统一内容查看器——同一窗口内 tab 切换文本/图片/语音条目，取代独立 ImagePreview 窗口。

**Tab 模型**：
```ts
interface Tab {
  key: string;           // `${source}:${itemId}` 唯一标识
  source: 'clipboard' | 'transcription';
  itemId: number;
  itemType?: 'text' | 'image';  // 仅 source=clipboard，决定渲染 textarea 还是 ImagePreview
  text?: string;
}
```
图片 tab 嵌入 `ImagePreviewComponent`（hidden 保持挂载 `display:none` 切换，≤5 个超过替换最旧）；语音 tab 只读 textarea（读 transcriptions/clipboard_history）；文本 tab 可编辑 textarea。

**ImagePreview 改为可控组件**：保留 `pages/ImagePreview/index.tsx` 作为**组件**（非路由页面），Props `imageId: number`（父传入，替代内部 PENDING/load 逻辑）；去掉 mount 时 `get_pending_image` / `listen("image-preview://load")`；保留 `listen("ocr-screenshot://result")`；`App.tsx` 删除 `image_preview_window` 路由。窗口尺寸默认 880×620 + 可调 + 记忆（WindowState 持久化到 app_config）。

**新后端命令**：`get_transcription_text(id)`、`get_clipboard_item_type(item_id)`；`open_compact_editor_tab` 加 `source` 参数（`"clipboard"` 默认 | `"transcription"`）。

**入口统一**：剪贴板文本「编辑」/剪贴板图片「预览」/图片预览 OCR/截图 OCR/语音管理「查看」/OCR 结果入库 → 全部 `open_compact_editor_tab`。`ocr_screenshot` 改为 `open_compact_editor_tab(image_id)` + emit blocks（不再开预览窗）。`image_preview_window.rs` / `image_preview_commands.rs` 废弃删除（功能合并到 CompactEditor）。

**不变量**：① Tab key 全局唯一；② 图片 tab ≤5（hidden 挂载）；③ 语音 tab 只读；④ 窗口尺寸可调+记忆；⑤ ImagePreviewComponent 由父控制 imageId。

### 5.7 标注工具与 Screenshot 的关系

**共享层**：`lib/annotation.ts`（`Annotation`/`Tool` 类型 + `drawAnnotation`/`drawAnnotationScaled`/`hitTestAnnotationPrecise`/`annBounds` 纯函数）——两端标注数据模型、绘制逻辑、命中检测完全统一。

**渲染层不共享**（各自实现）：ImagePreview 用视口渲染 canvas（窗口大小）+ SVG overlay（标注，自然像素坐标空间）；Screenshot 用单 canvas（屏幕尺寸 ~2M 像素，全量重绘 <1ms 无性能问题，窗口显示空间坐标）。不统一的原因：Screenshot 涉及选区裁剪逻辑改造量大，为 DRY 承担大改动换不到可感知性能提升。当前共享边界（纯函数 DRY + 渲染各自实现）合理。

**标注工具扩展**（后续在 image-viewer-perf 分支累积）：序号工具（自动递增编号）、马赛克工具（色块拼接）、菱形工具、实心填充开关、redo（redoStack + Cmd+Shift+Z），两端（截图 + 图片预览）统一。属性浮窗点击工具弹出、画布操作时自动收起、再点重新弹出；图标统一为截图 SVG 风格。

---

## 六、外部项目分析参考

> 合并 2 篇分析报告（非功能 spec，是借鉴参考）：
> - `2026-07-03-rapidraw-analysis.md`（[CyberTimon/RapidRAW](https://github.com/CyberTimon/RapidRAW)）
> - `2026-07-03-snow-shot-analysis.md`（[mg-chao/snow-shot](https://github.com/mg-chao/snow-shot)）

### 6.1 RapidRAW 可借鉴优化

RapidRAW 是 Tauri 2 + React 19 的专业 RAW 编辑器，架构同栈但功能深度远超 octopus ImagePreview（编辑器 vs 预览+标注）。可迁移到预览场景的设计：

| 优先级 | 改进项 | 来源 | octopus 状态 |
|--------|--------|------|-------------|
| P1 | 前端 ImageBitmap LRU 缓存（最近 N 张） | `ImageLRUCache.ts` | 只存 1 张 |
| P1 | `objectUrlRef` 升级为 protected set | `protectedBlobUrls` | 单 ref |
| P2 | ResizeObserver 自适应 fit-to-window | `useImageRenderSize.ts` | 加载时算一次 |
| P2 | 缩略图批量队列 + debounce | `useThumbnails.ts` | 每项独立 invoke |
| P3 | 后端 metadata 轻量命令（EXIF 早显） | `loadMetadataEarly` | 全图 onload 后才有 |
| P3 | 后端 generation cancel token | `load_image_generation` | 前端 cancelled boolean |

**前端 LRU 关键设计**：`protectedBlobUrls` 集合跟踪哪些 blob URL 在缓存中——LRU 淘汰时才 `revokeObjectURL`，避免误回收正在显示的 URL；`get()` 命中把 key 移到 Map 末尾（JS Map 保持插入序，`keys().next()` 返回最旧）；`cleanupEntry` 淘汰前检查新条目是否复用同一 blob URL。

**不建议借鉴**：wgpu GPU 渲染管线（octopus 不做像素修改，Canvas 2D 已足够，引入 wgpu 增 ~200KB + 复杂度爆炸）；多层 hash 缓存（ImagePreview 无调整参数链）；react-konva 标注框架（标注数量少时原生 canvas 更优，引入 konva 增 ~80KB）。

### 6.2 snow-shot 功能对比

snow-shot 是功能丰富的桌面截图/标注工具（Win/macOS），以 Excalidraw 为绘制核心。功能全景含区域/全屏/滚动（FAST 角点 + 描述子 + HNSW 近邻索引缝合，**非 NCC**）/延时/HDR 截图，17 种标注工具，RapidOCR OCR（文本块可视化 + 翻译 + 视觉模型），视频录制，固定贴图，全屏画板，鼠标穿透，翻译/AI 对话，截图历史，S3 上传，插件系统等。

**octopus 高价值缺失**：模糊/马赛克工具（标注刚需，已实现）、OCR 文本块可视化（已实现）、重做（已实现）。中价值：高亮工具、方向检测 OCR、延时截图。低价值/不建议：视频录制/AI 对话/翻译/Excalidraw 核心（过重 ~500KB，octopus SVG overlay 已够用）/S3/插件系统。

**最有价值借鉴（OCR 增强）**：snow-shot OCR 返回带坐标的 `text_blocks`（`box_points` + `text` + `text_score`），支持可视化显示文本块 + 逐块选择编辑 + 直接翻译/送 LLM。octopus 已据此实现 OCR 文本块可视化（第五节 5.5）。

**架构对比**：snow-shot Excalidraw fork（完整矢量编辑器，WebGL 渲染）vs octopus 视口渲染 + SVG overlay（轻量高效）；Ant Design（重量级）vs Tailwind（轻量）；Rust 后端模块化（tauri-commands 拆 crate）vs 单 crate（desktop）。**octopus 优势**：ASR 是核心能力（snow-shot 没有），图片预览视口渲染 + SVG overlay 在超大图上比 Excalidraw 更轻量。

---

## 七、2026-07-05 代码审查批次

> 合并 3 篇代码审查 spec（均 2026-07-05，全部 ✅ 已落地，对照代码核实：`feature.rs`/`parking_lot`/`net.rs` 均在）：
> - `2026-07-05-code-review-remediation-design.md`（深度审查 P0/P1/P2，子项目 A-I）
> - `2026-07-05-cross-platform-review-design.md`（跨平台复查）
> - `2026-07-05-rust-patterns-review-design.md`（rust-patterns 六大领域专项）
>
> 审查报告：`docs/code-review-2026-07-05.md` ｜ 实施计划：本归档 plans 对应 P0/P1/P2

### 7.1 代码审查修复设计（P0/P1/P2）

2026-07-05 对全 workspace（12 crate ~36K 行）深度审查，发现 14 Critical、~40 Important 及大量 Minor。**总目标**：消除所有 Critical，解决跨模块共性根因（锁毒化、网络超时缺失、重复代码），建立回归测试防线。**不在范围**：架构级重构（如 server 改 per-request engine 隔离，仅做稳定性补丁）、前端视觉重构、新功能。

按「根因主题 + 独立子系统」切分为 9 个子项目，三批执行。

**子项目 A：asr-local 正确性（P0）**。mel filterbank 权重在 Hz 空间计算 bug（paraformer 已在 mel 空间正确，fbank/zipformer 未修）→ 抽 `asr-local/src/feature.rs` 公共模块统一 mel 空间 filterbank（含 `mel_filterbank`/`apply_lfr`/`hz_to_mel`/`mel_to_hz`/`make_window`，`WindowType::Hamming/Povey`）。whisper `Box::leak`（每次 `new` leak 4×n_decoder_layers 字符串）→ 全局 `Lazy<Vec<...>>`（对齐 qwen3 `CACHE_NAMES`）。whisper 归一化 `(v+1e-10).log10()` → `v.max(1e-10).log10()` 对齐 sherpa-onnx。audio.rs hound `.unwrap()` → `Result` 传播。streaming_paraformer `raw_samples` 无界增长 → drain 已消费样本。moonshine `len()-1` → `saturating_sub(1)`。

> **实施偏差**：`compute_fbank` 未统一抽取（fbank.rs 无 DC removal/pre-emphasis，与 paraformer 是不同算法），feature.rs 只统一 mel filterbank + apply_lfr + window + hz_to_mel/mel_to_hz。fbank.rs 用 `high_freq=8000`（Nyquist）、paraformer 用 `high_freq=-400`（7600Hz），不同参数调同一函数。

**子项目 B：全局锁毒化整改（P1）**。引入 `parking_lot = "0.12"` workspace 依赖（不携带毒化标志，持锁 panic 不中毒，API 几乎兼容）。受影响：infra `db.rs` 全局 DB Mutex、clipboard `handle.rs`（9 处）、ocr `engine.rs` INIT_LOCK、asr-local 各引擎 session Mutex、desktop `RwLock<AppConfig>`（runtime_config/settings/coordinator/model_commands）+ screenshot 全局 Mutex + shutdown_db cell。

**子项目 C：全局网络超时（P0/P1）**。建 `infra/src/net.rs` 统一超时常量：`WS_CONNECT_TIMEOUT_SECS=10`、`WS_READ_TIMEOUT_SECS=30`、`HTTP_TIMEOUT_SECS=120`（LLM 慢）、`GRPC_CONNECT_TIMEOUT_SECS=8`、`GRPC_REQUEST_TIMEOUT_SECS=30`、`FILE_DOWNLOAD_TIMEOUT_SECS=300`。asr-cloud 4 provider WS connect_async + ws.next() 全链路包超时；llm `chat_text` 共享 `once_cell::Lazy` Client + 超时；desktop gRPC connect 移入 timeout fut；cloud close 看门狗；dlp 下载超时 + 200MB 大小限制。

**子项目 D：server 稳定化（P1）**。ASR 推理 `spawn_blocking` 包裹 + 请求级锁保护 switch+transcribe 原子性；默认 host 改 `127.0.0.1`；CORS 改可配置（默认同源 `CorsLayer::new()`）；`/transcribe` 加 `DefaultBodyLimit::max(100MB)`；手工 JSON 转义改 `serde_json::Value::String().to_string()`；加 SIGTERM/Ctrl-C graceful shutdown。

> **演进（arch-fixes 2026-07-06）**：`inference_lock` 后被 `AsrEngineManager::get_engine`（只读取 Arc 不改 active）+ `new_with_capacity(8)` 取代——同模型并发受引擎内 `Mutex<Session>` 串行化、跨模型天然并行，不再需要全局 `inference_lock`；仅 `spawn_blocking` 仍生效。

**子项目 E：download + dlp 数据完整性（P0）**。download 多段遇 200 响应数据错位 → 截断写入（仅写 `[seg.begin, seg.end]` 区间，`new_downloaded = seg.end - seg.begin + 1`）；dlp stderr 元数据 JSON 须是首行（之前的日志改 stdout）。

**子项目 F：desktop 协调器健壮性（P0/P1）**。C5 settings_window 非主线程调 AppKit UB → `MainThreadMarker::new()` + `run_on_main_thread` 调度；C6 CloudStreaming 看门狗超时；`unreachable!()` → 穷举 match / 防御性降级；截图 `AtomicBool` CAS 门控 + `BusyGuard` RAII；启动期 `expect` → `Result` 降级（`create_tray` 返回 Result、`home_dir` fallback、config 加载失败用 default）。

**子项目 G：死代码 + clippy + 调试输出（P2）**。删 `infra/image_util.rs` 全文件、desktop 死代码（`unregister_shortcut`/`is_screenshot_active`/`send_scroll`/`close_all_pin_windows`/`Pipeline` trait）、download/capx/asr-cloud 死代码、死依赖（dlp `tempfile`、llm `serde_yaml`）；capx 测试块加 `#[cfg(target_os="macos")]`；`eprintln!`/`console.log` → `log::debug!` 或删除；`cargo clippy --fix` 清 118 lint + 各 crate 加 `#![warn(clippy::all)]`。

**子项目 H：infra DB 健壮性（follow-up）**。`save_app_config_at` 30 条写入包 `unchecked_transaction`；`ensure_db` 打开后设 `PRAGMA journal_mode=WAL; busy_timeout=5000`；voice 历史搜索切 FTS5 MATCH（见第八节）。

**子项目 I：Follow-up Minor 精选**。llm `max_tokens × 1.2 → × 2.0`（中文润色截断）；Aliyun `bearer → Bearer` 统一；选中替换加 `log::debug!("[select]...")` 诊断；`filter_map(|r|r.ok())` → `collect_rows` helper（失败行 warn 跳过非静默）；baidu `Message::Close` 空结果发 Failed；capx `from_raw().expect()` → match 降级；asr-cloud JoinHandle 丢弃评估后**不修**（close_async 30s 超时兜底 + panic task 自动回收）。

**三大技术决策**：① 锁毒化 → parking_lot（无毒化标志，语义消除整类问题）；② 网络超时 → 统一 infra/net.rs 常量（避免散落不一致）；③ asr-local 重复代码 → 先修 bug 再抽取 feature.rs（抽取本身是 C1 修复手段）。

### 7.2 跨平台兼容性复查（2026-07-05）

针对外部跨平台评估报告逐项复查——**不轻信 bug 报告，以代码实际状态为准**。11 个问题点结论：

| # | 指控 | 结论 | 处理 |
|---|------|------|------|
| 1.1 | 非 macOS 点击穿透 poller 为空 | 属实 | 已修复：统一跨平台 poller |
| 1.2 | activation.rs macOS 独占致编译失败 | 不实 | `set_activation_policy` 是 Tauri 跨平台 API；`ns_window()` 已 `#[cfg(macos)]` 门控 |
| 1.3 | 多屏高 DPI 缩放定位错位 | 已处理 | poller 用物理坐标 + scale_factor 换算；截图全程物理像素 |
| 2.1 | 非 macOS 截图热循环 PNG 编解码往返 | 属实 | 已修复：新增 `crop_region_rgba` 直接返回 RgbaImage |
| 3.1 | ONNX Runtime DLL/SO 分发 | 架构问题 | 非代码缺陷，属打包配置 |
| 3.2 | ASR 硬件加速 segfault | 已缓解 | 平台门控按 OS 注册 EP，失败 catch 回退 CPU |
| 3.3 | DF3/OCR SIMD SIGILL | 部分有效 | DF3 失败降级 RNNoise（原直通）；实际用 tract 纯 Rust（运行时 SIMD 检测优雅回退），SIGILL 极低 |
| 4.1 | Linux Wayland 剪贴板后台监听失效 | 属实无解 | Wayland 协议层安全限制，建议 XWayland |
| 4.2 | Windows 剪贴板文件路径格式 | 不实 | 已正确 `cmd /C start "" path` |
| 5.1 | dlp `0o755` 致 Windows 编译失败 | 不实 | 已 `#[cfg(unix)]` 门控 |
| 5.1b | dlp 硬编码斜杠 | 不实 | 已用 `PathBuf::join` + 平台化 URL/扩展名 |

**已实施修复**：① 统一跨平台点击穿透 poller（移除 `#[cfg]` 门控，平台差异只在 `set_result_ignores_mouse`）；② 截图热路径移除 PNG 编解码往返（`crop_region_rgba` 直接返回 RgbaImage）；③ DF3 加载失败降级 RNNoise。未实施（理由充分）：ONNX Runtime 打包（tauri.conf.json 配置非代码缺陷）、EP 注册前置 dlopen 探测（ort 上游应做）、Wayland 剪贴板（协议层无解）。

### 7.3 Rust-Patterns 专项审查（2026-07-05）

用 rust-patterns skill 对 main 做六大领域专项审查（所有权/借用、错误处理、枚举/模式匹配、Trait/泛型、并发、模块可见性）。**总览**：clippy 零警告、269 测试全绿、代码质量较高。发现 3 P1 + 3 P2：

**P1（已完成）**：① Mutex `lock().unwrap()` poisoned panic → `unwrap_or_else(|e| e.into_inner())`（cli/main.rs 10 处 + downloader.rs）；② HeaderValue parse unwrap → `map_err?`（settings_commands.rs:449，secret_key 含非法字符返回错误非 panic）；③ ndarray `as_slice().unwrap()` → `ok_or_else` 传播（streaming_paraformer.rs encoder 输出 2 处 + decoder cache 1 处，非连续内存布局返回错误非 panic）。

**P2（已完成）**：① 14 个零外部调用 `pub fn` 收窄为 `pub(crate)`（clipboard/store.rs 3 个、download 8 个、llm/prompt.rs 3 个）；② paste.rs `thread::sleep` 已在 `spawn_blocking` 内不阻塞 async runtime，无风险；③ cloud_pipeline `block_on` 需架构级重构，暂不动。

**不需要改动**：测试中 unwrap（正常）、ONNX session.run().unwrap()（依赖模型文件）、stitch/zipformer unwrap（均在 `!is_empty()` 守护下）、unsafe 全有 SAFETY 文档、download error.rs thiserror 最佳实践、通配符匹配仅 2 处 UI 边界降级语义正确。

---

## 八、DB 表合并 + FTS5 搜索

> 合并 2 篇 DB 重构 spec（均 2026-07-05，✅ 已实现，DB 现行 `user_version = 18`）：
> - `2026-07-05-db-merge-design.md`（clipboard_history 吞并 transcriptions）
> - `2026-07-05-fts5-search-design.md`（FTS5 搜索切换）

### 8.1 DB 表合并（v16 → v17）

**背景**：`clipboard_history` 和 `transcriptions` 两表大量冗余（每条 ASR 的 text/created_at/engine/polish_status 两表各存一份，通过 `transcription_id` 外键关联，维护复杂）。**目标**：clipboard_history 吞并 transcriptions，精简为 `content` + `ref_data` + `meta_info` 三层数据模型，废弃 transcriptions 表。

**新表结构**：
```sql
CREATE TABLE clipboard_history (
    id INTEGER PRIMARY KEY,           -- 毫秒戳
    item_type TEXT NOT NULL,          -- 'text'|'voice'|'ocr'|'image'|'file'
    content TEXT NOT NULL DEFAULT '', -- voice/ocr/text: 文本全文; image/file: ""
    ref_data TEXT,                    -- image: blob_hash; file: JSON 路径数组; voice/ocr/text: NULL
    meta_info TEXT,                   -- JSON 按 item_type 存不同元数据
    is_favorite INTEGER NOT NULL DEFAULT 0,
    is_rich INTEGER NOT NULL DEFAULT 0,
    created_at TEXT NOT NULL,
    has_thumbnail INTEGER NOT NULL DEFAULT 0,
    segments TEXT                     -- 段 JSON（仅 voice，段模型真相源）
);
```

`item_type` 取代旧 `source` 列：旧 `text`(source=clipboard) 拆为 `text`/`voice`(source=asr)/`ocr`(source=ocr)，image/file 不变。`content`+`ref_data` 分层：text/voice/ocr 的全文在 content、ref_data 为 NULL；image 的 blob_hash 在 ref_data；file 的 JSON 路径数组在 ref_data。

**meta_info JSON**（Option 字段 `skip_serializing_if`，None 不出现在 JSON 避免膨胀）：
- image：`{w, h, size}`
- voice：`{engine, asr_mode, char_count, polished}`（ASR 侧 engine+asr_mode，LLM 侧 polish_model+polished）
- ocr：`{engine, model, char_count}`
- text：`{char_count}`
- file：`{files: [{size, type}]}`

**FTS5 索引**：触发器只用 `content` 做索引源，image/file content 为空字符串 → 不产生索引项。删除 `search_text` 列。`segments` 仅 voice 条目（从 transcriptions.segments 迁移）。

**迁移策略**：不迁移历史数据（用户确认可丢弃），直接 DROP + CREATE。`init_schema` 移除历史迁移链：`user_version >= 17` 跳过，其他跑 db.sql 一次性到 v17，`ensure_db` 不再 loop。

**代码影响**：`clipboard/store.rs` 全部 CRUD 改新表（`insert_asr_item`/`insert_ocr_item`/`insert_clipboard_item`/`row_to_item` 按 item_type 从 content 或 ref_data 取数据 + meta_info JSON 解析）；`watcher.rs` 补全 meta_info（text char_count、image w/h/size、file files）；`coordinator.rs` paste 路径改 `touch_created_at` 顶到列表顶部（不再重复 insert）；`compact_editor_commands.rs` `get_transcription_text` 改读 clipboard_history voice 条目 content；前端 `ClipboardItem` 类型删 source 加 item_type/ref_data/meta_info/has_thumbnail/segments，`MetaInfo` 统一结构，按 item_type 决定图标 + 类型色编码（text=stone/voice=amber/ocr=teal/image=indigo/file=emerald）。

**不变量**：① content 为空仅当 item_type ∈ {image,file}；② ref_data 仅 image/file 有值；③ FTS5 只索引 content；④ meta_info 按 item_type 不同 schema；⑤ segments 仅 voice。

### 8.2 FTS5 搜索切换（v17 → v18）

**背景**：`clipboard_history_fts`（FTS5 trigram tokenizer）已建表 + 触发器在跑，但 voice 历史搜索 `list_transcriptions_search_at` 仍用 `content LIKE '%query%'` 全表扫描——索引建好却白维护。

**目标**：voice 历史搜索走 FTS5 MATCH（>=3 字符），<3 字符回退 LIKE（trigram 无法生成 3-gram）；历史行 backfill 进 FTS5 索引；不破坏子串匹配语义。

**Backfill 迁移（v17→v18）**：
```sql
INSERT INTO clipboard_history_fts(rowid, content)
SELECT id, content FROM clipboard_history
WHERE content != '' AND id NOT IN (SELECT rowid FROM clipboard_history_fts);
```
`content != ''`（空文本不索引，与触发器一致）、`NOT IN`（幂等，已索引不重复插入）。全新库 `INIT_SQL` 建表时触发器自动维护首批 INSERT，无需 backfill。

**搜索函数改造**：`list_transcriptions_search_at` 按查询字符数分流：>=3 字符走 `clipboard_history_fts MATCH ?1`（子查询 `id IN (SELECT rowid ... MATCH)`，trigram MATCH 语义等价子串匹配），<3 字符回退 LIKE。SELECT 列与 row_mapper 不变。

**FTS5 query 转义**：`escape_fts5_match(q)` 用双引号包裹为 phrase（trigram 对 phrase 做连续 3-gram 匹配，语义等价子串匹配），内部双引号双写转义。如 `会议纪要` → `"会议纪要"`（trigram `会议纪`/`议纪要`），`AND` → `"AND"`（字面子串非逻辑运算符），`test*` → `"test*"`（非前缀通配）。

**测试**：backfill 后搜索命中历史行、>=3 字符走 MATCH、<3 字符回退 LIKE、特殊字符查询不报错、空 content 不索引。

**不变量**：① 搜索结果集不变（trigram MATCH 与 `LIKE '%query%'` 等价）；② content="" 不索引；③ 触发器行为不变。

---

> **归档边界说明**：本文件覆盖至 2026-07-05 的设计 spec。2026-07-06 起的活跃 spec（scroll-stitch 系列 6 篇、clipboard 系列 3 篇、streaming-session-manager、vendor-paddle-ocr、scrolling-screenshot-performance-optimization、clipboard-keyboard-nav）仍在 `specs/` 独立维护，不在本归档内。
