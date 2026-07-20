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
