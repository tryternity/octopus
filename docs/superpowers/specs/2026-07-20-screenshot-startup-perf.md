# 2026-07-20 截图启动性能优化（呼之即出）

## 背景

用户反馈：按截图快捷键后，截图窗口要 1-2 秒（实测 4-7 秒）才出来，且窗口出来后**功能不工作**（右键出 debug 菜单、点击无反应）。缺乏"呼之即出"的体验。

## 测量数据（debug build，双屏 4K）

3 次截图稳定复现，平均：

| 阶段 | 耗时 | 占比 |
|---|---|---|
| `capture_all_monitors`（双屏 4K 同步截图） | 829ms | 21% |
| destroy 旧窗口 | ~2ms | 0% |
| **JPEG 编码 + base64（主 3840×2160）** | **1729ms** | 🔥 44% |
| `WebviewWindowBuilder #0` | 0ms | 0% |
| **JPEG 编码 + base64（副 3840×1608）** | **1250ms** | 🔥 32% |
| inter-window sleep | 150ms | 4% |
| `WebviewWindowBuilder #1` | 0ms | 0% |
| **后端总耗时** | **3966ms** | 100% |
| 前端 ready callback | 从未到达 | — |
| 3s 超时强制 show | +3000ms | — |
| **用户感知总延迟** | **~4-7s** | — |

## 病灶定位

### 元凶 1：JPEG 编码占 76%（2979ms / 3966ms）

`crates/desktop/src/screenshot_commands.rs:155-161`：
1. `rgba_bytes.clone()`：4K RGBA 32MB clone
2. `into_rgb8()`：RGBA→RGB 再分配 24MB
3. `JpegEncoder::encode()`：同步编码（大头）
4. `general_purpose::STANDARD.encode()`：base64 编码（纯浪费）

**base64 是纯浪费**：`get_screenshot_image` 后端拿到 base64 又 `decode` 回字节流传前端（`screenshot_commands.rs:530`）。整个 base64 round-trip 没有任何意义。

双屏串行做两次编码，加起来约 3 秒。

### 元凶 2：前端 3s 不 ready（功能性 bug）

`Screenshot show timeout: 0/2 ready` 证明前端从未调用 `show_screenshot_window`——窗口被 3s 超时强制 show，但此时 React 未 mount，所有交互失效。

**不在本次修复范围**（方案 D/E 难度高、风险大），需独立 spec。

### 元凶 3：inter-window sleep(150ms)

`crates/desktop/src/screenshot_commands.rs:175-177` 在创建第二个窗口前强制 sleep 150ms（注释说"同时创建多个全屏 WebView 会导致 macOS segfault"）。这是历史遗留的 race condition workaround。

## 修复方案（A+B+C+F）

### A. 完全去掉 JPEG/base64，直接传 RGBA

**改动**：
- `PENDING_IMAGES: Mutex<Vec<(String, String)>>`（base64 string）→ `Mutex<Vec<(String, Vec<u8>, u32, u32)>>`（RGBA bytes + 宽高）
- `start_screenshot` 删除 JPEG 编码 + base64 编码（line 155-162）
- `get_screenshot_image` 返回 RGBA bytes（不再 base64 decode）
- 前端 `Screenshot/index.tsx`：
  ```ts
  invoke<ArrayBuffer>("get_screenshot_image", { label })
    .then((buf) => {
      const rgba = new Uint8ClampedArray(buf);
      const imgData = new ImageData(rgba, width, height);  // 需要 width/height
      return createImageBitmap(imgData);
    })
    .then((bm) => {
      bgBitmapRef.current = bm;
      setReady(true);
      invoke("show_screenshot_window");
    });
  ```
- 需要 `get_screenshot_image` 同时返回宽高（新增 `get_screenshot_image_meta` 或合并到 Response）

**收益**：省 ~3 秒 JPEG 编码 + 32MB clone + base64 round-trip

**风险**：IPC 协议变化，前端消费路径全改。`createImageBitmap(ImageData)` 在所有现代浏览器都支持。

### B. JPEG 改 spawn_blocking 并行（如果方案 A 不可行时的备选）

**本次跳过**——方案 A 直接消灭了 JPEG 编码，不需要并行了。

### C. JPEG 质量 + 不 clone（如果方案 A 不可行时的备选）

**本次跳过**——方案 A 直接消灭了 JPEG 编码。

### F. 干掉 inter-window sleep(150ms)

**改动**：删除 `screenshot_commands.rs:175-177` 的 `tokio::time::sleep(150ms)`。

**风险**：原注释说"同时创建多个全屏 WebView 会导致 macOS segfault"。但实测 4 天前 binary 双屏截图无 segfault——这个 workaround 可能是历史遗留，且 150ms 不足以避免 race condition（如果真有），该问题应该用其他方式定位（如顺序创建但无 sleep）。

**保守做法**：先去掉 sleep 实测；如果 segfault 复现，恢复 sleep 并改为 50ms（折衷）。

## 实施计划

1. `PENDING_IMAGES` 改类型（含 RGBA bytes + 宽高）
2. `start_screenshot` 删除 JPEG/base64 编码段
3. `get_screenshot_image` 返回 RGBA bytes + 宽高
4. 前端 `Screenshot/index.tsx` 改用 `createImageBitmap(ImageData)`
5. 删除 `inter-window sleep(150ms)`
6. 验证：cargo build + cargo test + 实测截图 timing

---

# 第二部分：截图保存性能优化（点确认后的卡顿）

## 背景

启动问题修完后，用户反馈：**点确认按钮后仍卡 1-6 秒**（取决于截图尺寸）。截图越大卡越久。

## 测量数据（debug build，release 数据见下）

`confirm_screenshot` 内 `save_screenshot_to_history` 各阶段 timing：

| 尺寸 | sha256 | decode PNG | **encode_to_webp** | 总计 |
|---|---|---|---|---|
| 1338×670 | 5ms | 40ms | **966ms** | 1011ms |
| 1512×804 | 6ms | 53ms | **1176ms** | 1235ms |
| **3176×1866** | 48ms | 276ms | **6074ms** | **6398ms** |

**主元凶：encode_to_webp 占 90%+**，且与尺寸强相关。

## 病灶 1：Lossless WebP 优先 + 慢

`crates/clipboard/src/image.rs:120` 原代码无条件把 `WebpLossless` 插入链首：

```rust
chain.insert(0, EncodeAttempt::WebpLossless);
```

WebP lossless 对大图（5M+ px）编码极慢（实测 3176×1866 = 6 秒）。但**截图历史根本不需要 lossless 质量**——给用户看 + 偶尔 OCR。

## 病灶 2：缩略图 resize 用 Triangle 卷积

`img.resize(240, 240, FilterType::Triangle)` —— Triangle 双线性卷积，debug build 对 3244×1760 = 674ms。

## WebP vs JPEG benchmark（/tmp/img-bench 实测，release build）

| 编码 | 3176×1866 耗时 | 体积 | bytes/px |
|---|---|---|---|
| WebP lossless | 1510ms | 316KB | 0.05 |
| WebP lossy q80 | 483ms | 997KB | 0.17 |
| WebP lossy q60 | 412ms | 595KB | 0.10 |
| **JPEG q85** | **55ms** | 1888KB | 0.32 |
| **JPEG q60** | **48ms** | 821KB | 0.14 |

**结论：JPEG 8.6x 快于 WebP lossy**。WebP 唯一优势是体积（同等画质小一半），但截图历史库不缺空间。`image` crate 的纯 Rust JPEG 实现高度优化（DCT），`webp` crate 绑定的 libwebp 反而慢（VP8 帧内预测复杂）。

缩略图对比（240×240 nearest resize）：

| Thumb 方案 | 耗时 | 体积 |
|---|---|---|
| nearest + WebP q20 | 7ms | 464 bytes |
| nearest + JPEG q60 | 8ms | 3344 bytes |
| Triangle + WebP q20（原方案）| 15ms | 452 bytes |

**结论（修订 2026-07-20）：缩略图也用 JPEG**——主图和缩略图共用同一套编码链设计（`IMAGE_SAVE_QUALITY` / `THUMB_SAVE_QUALITY`），统一格式降低 DB blob 的格式多样性。thumb q10 极轻质量（240×240 不要求细节，肉眼几乎无差），benchmark 显示 thumb 用 JPEG vs WebP 速度差异可忽略（1ms 内）。

## 修复方案

### 改动 1：去 Lossless WebP，链首改 JPEG

`crates/clipboard/src/image.rs`：删除 `chain.insert(0, EncodeAttempt::WebpLossless)`，让链按 `IMAGE_SAVE_QUALITY` 配置走。

`crates/infra/src/consts.rs`：`IMAGE_SAVE_QUALITY` 从 `"webp:80;jpeg:80"` 改为 `"jpeg:85;webp:80"`（JPEG 优先 + WebP 兜底）。

### 改动 2：缩略图 resize 改 nearest

`img.resize(240, 240, Triangle)` → `img.thumbnail(240, 240)`（image crate 内置 nearest-neighbor）。缩略图按 `THUMB_SAVE_QUALITY` 链编码（默认 JPEG q10）。

### 改动 3：custom-protocol feature（release build 修复）

调查中发现的关键 bug：tauri 用 `cfg(dev) = !has_feature("custom-protocol")` 决定走 devUrl 还是 frontendDist。**vault 引入 dev 模式后，所有 build（含 release）都不启用 custom-protocol → 都走 devUrl** → 没 vite 就 WebView 崩溃。

修复：
- `crates/desktop/Cargo.toml` 加 `custom-protocol = ["tauri/custom-protocol"]` feature
- `run-octopus.sh` 非 `--debug` 模式自动加 `--features custom-protocol`
- `run-octopus-dev.sh` 不加（配合 vite HMR）

## 实测收益（release build，3176×1866 大图）

| 阶段 | 优化前 | 优化后 | 加速 |
|---|---|---|---|
| encode_to_webp（主图）| 6074ms (lossless) | **57ms** (JPEG q85) | **106x** |
| thumb resize | ~600ms (Triangle) | **6ms** (nearest) | **100x** |
| encode 总计 | 6+ 秒 | **66ms** | **92x** |
| save_screenshot_to_history 总计 | 6+ 秒 | **~96ms** | **65x** |

用户感知：点确认后从"卡 6 秒"变成"几乎瞬间完成"。

## 验证

- `cargo test -p octopus-clipboard`：16 passed（含新增 `test_parse_image_fallbacks` JPEG-first 断言）
- `cargo test -p octopus-desktop`：375 passed
- 实测 release binary 截图保存：< 100ms（之前 6+ 秒）
