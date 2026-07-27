# 二维码识别设计

> **日期**：2026-07-27
> **状态**：设计阶段（待实现）
> **来源**：[竞品分析报告](../../research/2026-07-27-competitive-analysis.md) §4 截屏 P2

---

## 1. 需求

截图选区 + 图文编辑器图片，手动点「QR」按钮识别二维码，就地白卡展示结果 + 写入剪贴板。

**多码识别**：一张图里有多个二维码时全部识别，结果换行分割。

## 2. 方案

用 [rqrr](https://crates.io/crates/rqrr) crate（纯 Rust，依赖 octopus 已有的 `image = "0.25"`），加到 `crates/ocr`。

### 交互流程

```
用户选区 → 点 QR 按钮
  → 前端 composeAndCrop 选区 → PNG raw body
  → invoke("scan_qrcode_screenshot", { body })
  → 后端 spawn_blocking { image::load_from_memory → rqrr scan → 写剪贴板 }
  → 返回 Vec<String>（多码，可能空）
  → 前端白卡展示：
    - 识别中：转圈
    - 成功：文本/链接展示（多个换行分割）+ 已复制提示
    - 失败（空）：「未识别到二维码」
```

参考 snow-shot：选区位置就地白卡覆盖（不关窗，不弹 CompactEditor）。

## 3. 组件

### 3.1 `crates/ocr/src/qrcode.rs`（新建）

```rust
use anyhow::Result;
use image::DynamicImage;

/// 识别图片中的所有二维码，返回内容列表（可能空）。
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

### 3.2 `crates/ocr/Cargo.toml`

加依赖：`rqrr = "0.10"`

### 3.3 后端 Tauri 命令

**`scan_qrcode_screenshot`**（截图入口，`screenshot_commands.rs`）：
- 接收 raw body PNG（同 ocr_screenshot）
- `spawn_blocking`：解码 → `ocr::qrcode::scan` → 多码换行 join 写剪贴板
- 返回 `Vec<String>`（前端展示用）

**`scan_qrcode_image`**（已入库图入口，`clipboard_commands.rs`）：
- 接收 `image_id`
- `spawn_blocking`：从 DB 读 image blob → 解码 → scan → 写剪贴板
- 返回 `Vec<String>`

### 3.4 前端

**QR 按钮**：
- 截图：`AnnotationToolbar` children slot，OCR 按钮旁（`pages/Screenshot/index.tsx`）
- 图文编辑器：`Toolbar.tsx`，OCR 按钮旁

**白卡结果组件**（内联或独立）：
- 位置：选区内 fixed 定位（截图）/ 图片区域（ImagePreview）
- 识别中：`<Spinner>` 或简单「识别中...」
- 成功：文本展示（多码换行）+ URL 可点击（`open_url`）
- 复制：后端已写剪贴板，白卡显示「已复制」
- 失败：「未识别到二维码」
- 关闭：点白卡外 / ESC / 确认按钮

**i18n**：
- `screenshot.tool.qrcode`: 二维码 / QR Code
- `imagePreview.tool.qrcode`: 同上
- `screenshot.qrcode.noResult`: 未识别到二维码
- `screenshot.qrcode.copied`: 已复制到剪贴板

## 4. 不变量

| # | 不变量 | 保证 |
|---|---|---|
| INV-1 | 多码全识别 | rqrr `detect_grids` 返回所有检测到的 QR |
| INV-2 | 剪贴板写入多码换行分割 | join("\n") |
| INV-3 | 截图不关窗 | 白卡就地覆盖，用户可继续标注 |
| INV-4 | 无 QR 时返回空 Vec | 前端显示「未识别到二维码」 |

## 5. 不做

- 自动识别（手动按钮触发）
- 一维条码（rqrr 只 QR）
- CompactEditor tab（内容太短）
- QR 生成（只识别不生成）

## 6. 文件清单

| 文件 | 操作 |
|---|---|
| `crates/ocr/src/qrcode.rs` | 新建：scan 函数 |
| `crates/ocr/src/lib.rs` | 修改：pub mod qrcode |
| `crates/ocr/Cargo.toml` | 修改：加 rqrr |
| `crates/desktop/src/screenshot_commands.rs` | 修改：加 scan_qrcode_screenshot |
| `crates/desktop/src/clipboard_commands.rs` | 修改：加 scan_qrcode_image |
| `crates/desktop/src/main.rs` | 修改：注册命令 |
| `crates/desktop/frontend/src/pages/Screenshot/index.tsx` | 修改：QR 按钮 + 白卡 |
| `crates/desktop/frontend/src/pages/ImagePreview/Toolbar.tsx` | 修改：QR 按钮 |
| `crates/desktop/frontend/src/pages/ImagePreview/index.tsx` | 修改：白卡展示 |
| `crates/desktop/frontend/src/locales/{en,zh-CN}.yaml` | 修改：i18n |
