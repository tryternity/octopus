# 二维码识别设计

> **日期**：2026-07-27
> **状态**：✅ 已实现（zxing-cpp C++ FFI bundled + 多码全识别 + 就地白卡 + 单个复制/复制所有；e2e 用户验证通过）
> **来源**：[竞品分析报告](../../research/2026-07-27-competitive-analysis.md) §4 截屏 P2

---

## 1. 需求

截图选区 + 图文编辑器图片，手动点「QR」按钮识别二维码，就地白卡展示结果。

**多码识别**：一张图里有多个二维码时全部识别。

## 2. 方案

用 [zxing-cpp](https://crates.io/crates/zxing-cpp) crate（C++ FFI，`bundled` feature vendored 编译）。

**crate 演进**（2026-07-27 三轮换库）：
1. rqrr（纯 Rust）→ 微信 QR DataEcc 失败，1/2 识别
2. quircs（纯 Rust quirc port）→ 同样 DataEcc 失败
3. **zxing-cpp**（C++ FFI Apache-2.0 bundled）→ JPEG q85 两个微信 QR 全部成功 ✅

纯 Rust QR 库的 Reed-Solomon 纠错实现不够鲁棒，对 JPEG 有损压缩后的高纠错级别 QR 会出现 DataEcc 失败。zxing-cpp 是工业级 C++ 实现，bundled feature 用 cmake 静态编译（无系统依赖），跨平台支持（macOS c++/Linux stdc++/Windows MSVC 自动链接）。

### 交互流程

```
用户选区 → 点 QR 按钮
  → 前端 composeAndCrop 选区 → PNG raw body
  → invoke("scan_qrcode_screenshot", { body })
  → 后端 spawn_blocking { image::load_from_memory → qrcode::scan }
  → 返回 Vec<String>（多码，可能空）
  → 前端白卡展示：
    - 识别中：转圈
    - 成功：文本/链接展示（多个换行分割），每个 QR 后有 📋 复制按钮
    - 多码时底部「复制所有」按钮
    - 失败（空）：「未识别到二维码」
```

## 3. 组件

### 3.1 `crates/ocr/src/qrcode.rs`

```rust
use zxingcpp::{BarcodeFormat, ImageView, ImageFormat};

/// 不依赖 zxing-cpp 的 `image` feature（该 feature 会连带拉入 image crate 的
/// default features → avif → rav1e 整条重依赖链）。改为直接用 ImageView::from_slice
/// 从灰度像素构造，QR 识别只需亮度通道，无需彩色格式。
pub fn scan(image: &DynamicImage) -> Result<Vec<String>> {
    let reader = zxingcpp::read()
        .formats(zxingcpp::BarcodeFormats::list(BarcodeFormat::QRCode));
    let luma = image.to_luma8();
    let iv = ImageView::from_slice(luma.as_raw(), luma.width(), luma.height(), ImageFormat::Lum)?;
    let results = reader.from(&iv)?;
    Ok(results.into_iter()
        .map(|r| r.text().to_string())
        .filter(|s| !s.is_empty())
        .collect())
}
```

### 3.2 `crates/ocr/Cargo.toml`

```toml
zxing-cpp = { version = "0.5", features = ["bundled"] }
```

> **注**：不启用 zxing-cpp 的 `image` feature——它会拉入 image crate 的 default features（含 avif → rav1e 重依赖链）。改用 `ImageView::from_slice` + lum 灰度构造（§3.1）。

### 3.3 后端 Tauri 命令

**`scan_qrcode_screenshot`**（截图入口）：raw body PNG → `load_from_memory` → `qrcode::scan`。不自动写剪贴板（前端控制复制）。

**`scan_qrcode_image`**（已入库图入口）：image_id → DB blob → `load_from_memory`（自动检测格式，不硬编码 WebP/JPEG）→ `qrcode::scan`。

### 3.4 前端

- QR 按钮：AnnotationToolbar children（截图）+ Toolbar.tsx（ImagePreview），OCR 按钮旁
- 白卡：fixed 定位（截图）/ absolute（ImagePreview），识别中转圈、成功展示文本+复制按钮、失败提示
- 单个复制：每个 QR 文本后有 📋（icons/copy.svg）按钮
- 复制所有：多码时底部按钮，join("\n") 写剪贴板
- 单码时不显示「复制所有」（只有单个复制按钮）

## 4. 不变量

| # | 不变量 | 保证 |
|---|---|---|
| INV-1 | 多码全识别 | zxing-cpp reader 返回所有检测到的 QR |
| INV-2 | 不自动写剪贴板 | 前端控制复制（单个/所有） |
| INV-3 | 截图不关窗 | 白卡就地覆盖 |

## 5. 不做

- 自动识别（手动按钮触发）
- 一维条码（只 QR）
- CompactEditor tab（内容太短）
- QR 生成（只识别不生成）

## 6. 文件清单

| 文件 | 操作 |
|---|---|
| `crates/ocr/src/qrcode.rs` | scan 函数（zxing-cpp） |
| `crates/ocr/src/lib.rs` | pub mod qrcode |
| `crates/ocr/Cargo.toml` | zxing-cpp 依赖 |
| `crates/desktop/src/screenshot_commands.rs` | scan_qrcode_screenshot |
| `crates/desktop/src/clipboard_commands.rs` | scan_qrcode_image |
| `crates/desktop/src/main.rs` | 注册命令 |
| `crates/desktop/frontend/src/pages/Screenshot/index.tsx` | QR 按钮 + 白卡 |
| `crates/desktop/frontend/src/pages/ImagePreview/Toolbar.tsx` | QR 按钮 |
| `crates/desktop/frontend/src/pages/ImagePreview/index.tsx` | 白卡展示 |
| `crates/desktop/frontend/src/locales/{en,zh-CN}.yaml` | i18n |
