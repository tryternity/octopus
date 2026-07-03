# RapidRAW 分析报告 — 可借鉴的优化与设计

> 分析对象：[CyberTimon/RapidRAW](https://github.com/CyberTimon/RapidRAW)
> 本地路径：`/Users/wudarui/workspace/agent/RapidRAW`
> 分析日期：2026-07-03
> 分析目的：提取可借鉴到 octopus ImagePreview 的优化策略

## 1. 项目定位

RapidRAW 是一个基于 Tauri 2 + React 19 的专业 RAW 图片编辑器（对标 Lightroom）。支持 RAW 开发、GPU 渲染（wgpu）、AI 标注、蒙版、批处理导出。架构上与 octopus 同栈（Tauri 2 + Rust 后端 + React 前端），但功能深度和复杂度远超 ImagePreview。

**核心差异**：RapidRAW 是"编辑器"（修改像素、导出新图），octopus ImagePreview 是"预览+标注"（查看原图、轻量标注后导出）。以下分析聚焦**可迁移到预览场景**的设计。

---

## 2. 可借鉴的优化（按优先级）

### 2.1 多级缓存体系（强烈推荐）

RapidRAW 有 4 层缓存，层层命中避免重复计算：

| 缓存层 | 位置 | 作用 | octopus 对应 |
|--------|------|------|-------------|
| `DecodedImageCache` | Rust `cache_utils.rs` | LRU 缓存已解码原图（`Arc<DynamicImage>` + exif），避免重复 decode | **无**——每次切图重新拉 blob + decode |
| `GpuImageCache` | Rust `app_state.rs` | GPU 纹理缓存（`transform_hash` 判断是否需重新上传） | 不适用（我们用 Canvas 2D） |
| `CachedPreview` | Rust `app_state.rs` | 缩放预览图缓存（含 `transform_hash` 判断脏否） | **无** |
| `ImageLRUCache` | TS `utils/ImageLRUCache.ts` | 前端 LRU（maxSize=20），缓存调整结果 + blob URL | **无** |

**前端 LRU 的关键设计**（`ImageLRUCache.ts`）：
- `protectedBlobUrls` 集合跟踪哪些 blob URL 在缓存中——LRU 淘汰时才 `revokeObjectURL`，避免误回收正在显示的 URL
- `get()` 命中时把 key 移到 Map 末尾（JS Map 保持插入序，`keys().next()` 返回最旧）
- `cleanupEntry(old, replacement)` 淘汰旧条目前检查新条目是否复用了同一个 blob URL——避免"换图时 revoke 了正在用的 URL"

**octopus 可借鉴**：
- 前端 LRU 缓存最近 N 张图的 `ImageBitmap`（当前 `scaledBitmapRef` 只存 1 张），切图回切时秒开
- `objectUrlRef` 升级为 `protectedBlobUrls` 集合，防止多图切换时误 revoke

### 2.2 缩略图批量队列 + debounce（推荐）

`useThumbnails.ts` 的设计：
- `pendingQueueRef`（Set）收集可视区域新增的图片路径
- `debounce(150ms, maxWait=300ms)` 合并快速滚动产生的批量请求
- 合并后随机 shuffle 发给后端（避免同方向滚动时后端按顺序处理导致末尾图延迟）
- `generatedRef`（Set）去重已生成的缩略图

**octopus 可借鉴**：剪贴板窗口滚动时的 `get_image_thumb` 请求可改为批量队列模式（当前每个 ClipboardItem 独立 invoke），减少 IPC 开销 + 快速滚动时按优先级处理。

### 2.3 分阶段渐进加载（部分已有）

`useImageLoader.ts` 的加载流程：
1. `loadMetadataEarly()`：先拉 metadata（EXIF + 历史调整参数），轻量、秒回
2. `loadFullImageData()`：后拉全图二进制 → 设 originalSize / previewSize

**对比 octopus 现状**：我们已做了 thumb→full 渐进加载（spec §3.3），比 RapidRAW 多一步（先 thumb 秒显）。但 RapidRAW 多了"先拉 metadata"这一步——octopus 的 EXIF 信息当前只在全图 onload 后才知道。

**octopus 可借鉴**：如果未来需要在缩略图阶段就显示 EXIF（如图片尺寸、格式），可后端加一个轻量 metadata 命令，不传像素数据。

### 2.4 ResizeObserver 自适应渲染尺寸（推荐）

`useImageRenderSize.ts` 用 `ResizeObserver` 监听容器尺寸变化，实时计算图片在容器内的渲染尺寸（width / height / scale / offsetX / offsetY）。拖拽窗口大小时图片自动重新 fit-to-window。

**octopus 现状**：`computeFitZoom` 只在图片加载时算一次，窗口 resize 后不自动重算。

**octopus 可借鉴**：加一个 `ResizeObserver`，用户拖窗口大小时自动重算 fit zoom（仅 `!userZoomedRef` 时）。

### 2.5 `cancel_token` 取消机制（推荐）

`image_loader.rs:173-180` 和 `app_state.rs:135`：
```rust
pub load_image_generation: Arc<AtomicUsize>,
```
每次加载新图时 `generation.fetch_add(1)`，解码过程中检查 `generation != expected` → `Err("Load cancelled")`。防止快速切图时旧解码完成后覆盖新图。

**对比 octopus 现状**：我们用 `cancelled` boolean 标志（effect cleanup 设 true），但这是前端层面的。后端 `get_image_full` 是一次性 IPC，不支持中途取消。

**octopus 可借鉴**：如果未来支持超大图（100MB+），后端解码可加 generation 取消机制。

### 2.6 `createImageBitmap` 的 resizeQuality（已有）

RapidRAW 在 Rust 后端做缩放（`image::resize`），octopus 在前端用 `createImageBitmap({ resizeQuality: "high" })`。两者等价，octopus 方案更轻（GPU 加速）。

---

## 3. 标注 / 蒙版系统对比

### RapidRAW 的做法

用 **react-konva**（Canvas 2D 声明式框架）做标注/蒙版渲染：
- `Stage` + `Layer` 管理 canvas 渲染树
- `OptimizedBrushLine`（`memo` + `Float32Array` 压点）优化笔迹性能
- `MaskOverlay` 支持矩形/椭圆/渐变/笔刷四种蒙版形状
- `Transformer`（Konva 内置）处理选中后的拖拽/缩放

**性能优化**：笔迹点用 `Float32Array` 而非 `{x,y}[]`（内存紧凑、`Array.from` 直转 Konva 格式）。

### octopus 的做法

原生 Canvas 2D + 手动重绘（`drawBg`/`drawActive` 双 canvas）。

**对比**：
- RapidRAW 的 react-konva 在标注形状多时更省心（声明式、自动 diff）
- octopus 的手动双 canvas 在标注少（<20）时更快（无 React 虚拟 canvas 树开销）
- **不需要迁移到 react-konva**——标注数量少时原生 canvas 性能足够，引入 konva 会增加 bundle ~80KB

**可借鉴**：笔迹点用 `Float32Array` 而非 `number[][]`（当前 `Annotation.points: number[][]`）。但标注数量少时收益微乎其微，**不建议改**。

---

## 4. 不建议借鉴的设计

### 4.1 wgpu GPU 渲染管线

RapidRAW 用 wgpu 做 GPU shader 管线（`gpu_processing.rs` + WGSL shaders），实时像素级调整（曝光、白平衡、曲线）。

**不适用于 octopus**：ImagePreview 不做像素修改，Canvas 2D 的 `drawImage` + `createImageBitmap` 已足够。引入 wgpu 会增加 ~200KB Rust 依赖 + 复杂度爆炸。

### 4.2 多层 Hash 缓存

`cache_utils.rs` 有 4 个 hash 函数（`calculate_geometry_hash` / `calculate_visual_hash` / `calculate_transform_hash` / `calculate_full_job_hash`），用于判断调整参数是否变化、是否需要重新渲染。

**不适用于 octopus**：ImagePreview 没有调整参数链，标注变化直接触发重绘即可，无需 hash 判断脏否。

### 4.3 react-konva 标注框架

见上 §3 分析，标注数量少时原生 canvas 更优。

---

## 5. 总结：建议引入的改进

| 优先级 | 改进项 | 来源 | octopus 当前状态 | 预估工作量 |
|--------|--------|------|-----------------|-----------|
| **P1** | 前端 ImageBitmap LRU 缓存（最近 N 张） | `ImageLRUCache.ts` | 只存 1 张（`scaledBitmapRef`） | 2-3h |
| **P1** | `objectUrlRef` 升级为 protected set | `ImageLRUCache.protectedBlobUrls` | 单 ref（已修泄漏） | 1h |
| **P2** | ResizeObserver 自适应 fit-to-window | `useImageRenderSize.ts` | 加载时算一次 | 1-2h |
| **P2** | 缩略图批量队列 + debounce | `useThumbnails.ts` | 每项独立 invoke | 2-3h |
| **P3** | 后端 metadata 轻量命令（EXIF 早显） | `useImageLoader.loadMetadataEarly` | 全图 onload 后才有 | 3-4h |
| **P3** | 后端 generation cancel token | `load_image_generation` | 前端 cancelled boolean | 2-3h |

**不建议引入**：wgpu 管线、多层 hash 缓存、react-konva。
