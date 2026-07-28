# 二维码识别实施计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 截图+图文编辑器加二维码识别按钮，rqrr 多码识别，就地白卡展示结果+写剪贴板。

**Architecture:** crates/ocr 加 rqrr 依赖 + scan 函数 → desktop 两个 Tauri 命令 → 前端 QR 按钮 + 白卡组件。

**Tech Stack:** Rust + rqrr（纯 Rust QR 解码）+ React + Tauri 2

**Spec:** `docs/superpowers/specs/2026-07-27-qrcode-scan-design.md`

## Global Constraints

- rqrr = "0.10"（纯 Rust，依赖已有 image = "0.25"）
- 多码全识别，返回 Vec<String>，换行分割写剪贴板
- 就地白卡展示（不关窗、不开 CompactEditor）
- 手动按钮触发（不自动）
- 截图 + 图文编辑器两处都要

---

## File Structure

| 文件 | 操作 | 职责 |
|---|---|---|
| `crates/ocr/src/qrcode.rs` | 新建 | scan 函数 |
| `crates/ocr/src/lib.rs` | 修改 | pub mod qrcode |
| `crates/ocr/Cargo.toml` | 修改 | 加 rqrr |
| `crates/desktop/src/screenshot_commands.rs` | 修改 | scan_qrcode_screenshot 命令 |
| `crates/desktop/src/clipboard_commands.rs` | 修改 | scan_qrcode_image 命令 |
| `crates/desktop/src/main.rs` | 修改 | 注册命令 |
| `crates/desktop/frontend/src/pages/Screenshot/index.tsx` | 修改 | QR 按钮 + 白卡 |
| `crates/desktop/frontend/src/pages/ImagePreview/Toolbar.tsx` | 修改 | QR 按钮 |
| `crates/desktop/frontend/src/pages/ImagePreview/index.tsx` | 修改 | 白卡展示 |
| `crates/desktop/frontend/src/locales/{en,zh-CN}.yaml` | 修改 | i18n |

---

### Task 1: rqrr 依赖 + scan 函数（crates/ocr）

**Files:**
- Create: `crates/ocr/src/qrcode.rs`
- Modify: `crates/ocr/src/lib.rs`, `crates/ocr/Cargo.toml`

**Interfaces:**
- Produces: `crate::qrcode::scan(image: &DynamicImage) -> Result<Vec<String>>`

- [ ] **Step 1: 加 rqrr 依赖**

Read `crates/ocr/Cargo.toml`，在 `[dependencies]` 段加：

```toml
rqrr = "0.10"
```

- [ ] **Step 2: 创建 qrcode.rs**

```rust
//! 二维码识别（rqrr，纯 Rust）。
//!
//! 详见 spec 2026-07-27-qrcode-scan-design.md。

use anyhow::Result;
use image::DynamicImage;

/// 识别图片中的所有二维码，返回内容列表（可能空）。
///
/// 多码全识别——rqrr `detect_grids` 返回所有检测到的 QR grid。
pub fn scan(image: &DynamicImage) -> Result<Vec<String>> {
    let luma = image.to_luma8();
    let mut prepared = rqrr::PreparedImage::prepare(luma);
    let grids = prepared.detect_grids();
    let mut results = Vec::new();
    for grid in grids {
        if let Ok((content, _meta)) = grid.decode() {
            if !content.is_empty() {
                results.push(content);
            }
        }
    }
    Ok(results)
}
```

- [ ] **Step 3: 在 lib.rs 注册**

Read `crates/ocr/src/lib.rs`，加 `pub mod qrcode;`。

- [ ] **Step 4: 编译验证**

Run: `cargo build -p octopus-ocr`
Expected: 0 error

- [ ] **Step 5: Commit**

```bash
git add crates/ocr/src/qrcode.rs crates/ocr/src/lib.rs crates/ocr/Cargo.toml
git commit -m "feat(ocr): 二维码识别 scan 函数（rqrr 多码全识别）"
```

---

### Task 2: 后端 Tauri 命令

**Files:**
- Modify: `crates/desktop/src/screenshot_commands.rs`
- Modify: `crates/desktop/src/clipboard_commands.rs`
- Modify: `crates/desktop/src/main.rs`

**Interfaces:**
- Consumes: Task 1 的 `ocr::qrcode::scan`
- Produces: `scan_qrcode_screenshot(body) -> Vec<String>` + `scan_qrcode_image(image_id) -> Vec<String>`

- [ ] **Step 1: scan_qrcode_screenshot 命令**

Read `crates/desktop/src/screenshot_commands.rs`。参考 `ocr_screenshot` 命令的模式（raw body PNG → spawn_blocking → image::load_from_memory）。

新增命令：

```rust
#[tauri::command]
pub async fn scan_qrcode_screenshot(
    app_handle: tauri::AppHandle,
    request: tauri::ipc::Request,
) -> Result<Vec<String>, String> {
    let body = request.body();
    // 从 raw body 取 PNG bytes（同 ocr_screenshot 的模式）
    let png_bytes = body.as_binary().ok_or("expected binary body")?;

    let results = tokio::task::spawn_blocking(move || -> Result<Vec<String>, String> {
        let img = image::load_from_memory(png_bytes).map_err(|e| e.to_string())?;
        let codes = octopus_ocr::qrcode::scan(&img).map_err(|e| e.to_string())?;
        // 写剪贴板（多码换行分割）
        if !codes.is_empty() {
            let text = codes.join("\n");
            let _ = crate::clipboard::write_text(&text);
        }
        Ok(codes)
    })
    .await
    .map_err(|e| e.to_string())?;

    Ok(results)
}
```

注意：
- 参考现有 `ocr_screenshot` 的 body 提取方式（可能用 `tauri::ipc::Request` 或直接 `Vec<u8>` 参数，按现有代码模式调整）
- 剪贴板写入用项目现有的 clipboard 写文本方法（grep `write_text` 或 `set_text`）
- 不需要 OcrLockGuard（QR 不撞推理模型，秒级）

- [ ] **Step 2: scan_qrcode_image 命令**

Read `crates/desktop/src/clipboard_commands.rs`。参考 `ocr_image` 命令的模式（从 DB 读 image blob → 解码）。

新增命令：

```rust
#[tauri::command]
pub async fn scan_qrcode_image(image_id: i64) -> Result<Vec<String>, String> {
    let results = tokio::task::spawn_blocking(move || -> Result<Vec<String>, String> {
        // 从 DB 读 image blob（参考 ocr_image 的模式）
        let blob = octopus_infra::db::with_db_blocking(|conn| {
            octopus_infra::db::get_image_data_blob(conn, image_id)
        }).map_err(|e| e.to_string())?;
        let img = image::load_from_memory(&blob).map_err(|e| e.to_string())?;
        let codes = octopus_ocr::qrcode::scan(&img).map_err(|e| e.to_string())?;
        if !codes.is_empty() {
            let text = codes.join("\n");
            let _ = crate::clipboard::write_text(&text);
        }
        Ok(codes)
    })
    .await
    .map_err(|e| e.to_string())?;

    Ok(results)
}
```

注意：按现有的 DB image blob 读取方式调整（grep `get_image_data` 或 `image_data` 的读取路径）。

- [ ] **Step 3: 在 main.rs 注册命令**

Read `crates/desktop/src/main.rs`。找到 `ocr_screenshot` / `ocr_image` 注册的位置，加：

```rust
screenshot_commands::scan_qrcode_screenshot,
clipboard_commands::scan_qrcode_image,
```

- [ ] **Step 4: 编译验证**

Run: `cargo build -p octopus-desktop --features embedded`
Expected: 0 error

- [ ] **Step 5: Commit**

```bash
git add crates/desktop/src/screenshot_commands.rs crates/desktop/src/clipboard_commands.rs crates/desktop/src/main.rs
git commit -m "feat(desktop): scan_qrcode_screenshot + scan_qrcode_image 命令"
```

---

### Task 3: 前端 QR 按钮 + 白卡 + i18n

**Files:**
- Modify: `crates/desktop/frontend/src/pages/Screenshot/index.tsx`
- Modify: `crates/desktop/frontend/src/pages/ImagePreview/Toolbar.tsx`
- Modify: `crates/desktop/frontend/src/pages/ImagePreview/index.tsx`
- Modify: `crates/desktop/frontend/src/locales/en.yaml`
- Modify: `crates/desktop/frontend/src/locales/zh-CN.yaml`

**Interfaces:**
- Consumes: Task 2 的 Tauri 命令

- [ ] **Step 1: i18n 文案**

`zh-CN.yaml` screenshot.tool 下加：
```yaml
qrcode: 二维码
```
screenshot 下加：
```yaml
qrNoResult: 未识别到二维码
qrCopied: 已复制到剪贴板
qrScanning: 识别中...
```

`en.yaml` 对应：
```yaml
qrcode: QR Code
qrNoResult: No QR code found
qrCopied: Copied to clipboard
qrScanning: Scanning...
```

imagePreview.tool 下也加 `qrcode`（或复用 screenshot key）。

- [ ] **Step 2: 截图 QR 按钮 + 白卡**

Read `crates/desktop/frontend/src/pages/Screenshot/index.tsx`。

在 AnnotationToolbar children 里，OCR 按钮旁加 QR 按钮：

```tsx
{/* QR 二维码识别 */}
<ToolButton
  onClick={() => doQrScan()}
  label={t("screenshot.tool.qrcode")}
  icon={<img src="icons/qr-code.svg" ... />}
/>
```

加 QR 状态 + 白卡渲染：
```tsx
const [qrResult, setQrResult] = useState<string[] | null>(null);
const [qrScanning, setQrScanning] = useState(false);

const doQrScan = async () => {
  // composeAndCrop 选区 → raw body（参考 doOcr 的模式）
  const body = await composeAndCrop();
  if (!body) return;
  setQrScanning(true);
  setQrResult(null);
  try {
    const codes = await invoke<string[]>("scan_qrcode_screenshot", { ... });
    setQrResult(codes);
  } catch (e) {
    setQrResult([]);
  } finally {
    setQrScanning(false);
  }
};
```

白卡渲染（fixed 定位在选区位置，参考 snow-shot 的 ScanQrcodeTool）：
```tsx
{(qrScanning || qrResult) && (
  <div style={{ position: "fixed", /* 选区位置 */, zIndex: 200, ... 白卡样式 }}>
    {qrScanning ? t("screenshot.qrScanning") :
     qrResult && qrResult.length > 0 ? (
      <div>
        {qrResult.map((code, i) => (
          <div key={i}>
            {code.startsWith("http") ?
              <a href={code} onClick={() => openUrl(code)}>{code}</a> :
              <span>{code}</span>}
          </div>
        ))}
        <span style={{ /* "已复制" 提示 */ }}>{t("screenshot.qrCopied")}</span>
      </div>
    ) : t("screenshot.qrNoResult")}
    <button onClick={() => { setQrResult(null); setQrScanning(false); }}>✕</button>
  </div>
)}
```

- [ ] **Step 3: ImagePreview QR 按钮 + 白卡**

Read `crates/desktop/frontend/src/pages/ImagePreview/Toolbar.tsx` 和 `index.tsx`。

Toolbar 加 QR 按钮（OCR 按钮旁），onClick 调 `props.onQrScan()`。

index.tsx 加 QR 状态 + 白卡渲染（同截图的模式，但调 `scan_qrcode_image(imageId)`，白卡定位在图片区域内）。

- [ ] **Step 4: 图标**

检查 `crates/desktop/frontend/public/icons/` 有没有 `qr-code.svg`。如果缺失，从 lucide.dev 下载 QR Code 图标放入。

- [ ] **Step 5: tsc + vite build**

Run: `cd crates/desktop/frontend && npx tsc --noEmit && npx vite build`
Expected: 0 error

- [ ] **Step 6: Commit**

```bash
git add crates/desktop/frontend/ crates/desktop/frontend/public/icons/
git commit -m "feat(frontend): QR 按钮 + 就地白卡展示 + i18n（截图+图文编辑器）"
```

---

### Task 4: 全量验证 + 文档

- [ ] **Step 1: cargo build + test**

Run: `cargo build -p octopus-desktop --features embedded && cargo test -p octopus-ocr -p octopus-desktop`
Expected: 0 error, all pass

- [ ] **Step 2: architecture.md 更新**

在截图工具栏描述处补 QR 按钮。

- [ ] **Step 3: Commit**

```bash
git add docs/architecture.md
git commit -m "docs: architecture.md 补二维码识别"
```
