# OCR 模块实施计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 为剪贴板图片条目添加 OCR 识别能力（ocr-rs/MNN + PP-OCRv6），识别文本写入 search_text + 系统剪贴板 + 新建文档。

**Architecture:** 独立 crate `octopus-ocr`（依赖 infra），desktop 层编排调用。ocr-rs 封装 det→crop→rec pipeline，MNN 后端推理。模型三件套（det.mnn/rec.mnn/keys.txt）已手动放置于 `~/.octopus/models/ocr/PP-OCRv6-small/`。DB 零 schema 变更，复用 models 表 + app_config。

**Tech Stack:** Rust + ocr-rs 2.3（MNN）+ image 0.25 + Tauri + React + lucide-react

**Spec:** `docs/superpowers/specs/2026-06-27-ocr-module-design.md`

---

## 文件结构

| 文件 | 责任 |
|---|---|
| **Create:** `crates/ocr/Cargo.toml` | crate 清单，依赖 ocr-rs/image/infra |
| **Create:** `crates/ocr/src/lib.rs` | 模块入口，pub use |
| **Create:** `crates/ocr/src/engine.rs` | OcrEngine 封装：单例 + recognize() |
| **Create:** `crates/ocr/src/model.rs` | 模型路径管理 + is_model_ready |
| **Modify:** `Cargo.toml`（workspace） | members 新增 crates/ocr |
| **Modify:** `crates/desktop/Cargo.toml` | 新增 octopus-ocr 依赖 |
| **Modify:** `crates/desktop/src/clipboard_commands.rs` | 新增 ocr_image 命令 |
| **Modify:** `crates/desktop/src/main.rs` | 注册 ocr_image 命令 |
| **Modify:** `crates/clipboard/src/store.rs` | 新增 update_search_text |
| **Modify:** `crates/infra/src/db.sql` | 新增 OCR models seed + app_config seed |
| **Modify:** `crates/desktop/frontend/src/pages/Clipboard/ClipboardItem.tsx` | OCR 按钮 + 状态机 |
| **Modify:** `crates/desktop/frontend/src/pages/Settings/ClipboardPanel.tsx` | 管理页 OCR 按钮 |

---

### Task 1: octopus-ocr crate 骨架

**Files:**
- Create: `crates/ocr/Cargo.toml`
- Create: `crates/ocr/src/lib.rs`
- Modify: `Cargo.toml`（workspace root，line 2）

- [ ] **Step 1: 创建 crate 目录结构**

```bash
mkdir -p crates/ocr/src
```

- [ ] **Step 2: 写 Cargo.toml**

```toml
[package]
name = "octopus-ocr"
version = "0.1.0"
edition = "2021"

[dependencies]
octopus-infra = { path = "../infra" }
ocr-rs = "2.3"
image = "0.25"
anyhow = "1"
log = "0.4"
```

- [ ] **Step 3: 写 lib.rs（最小骨架）**

```rust
pub mod engine;
pub mod model;
```

- [ ] **Step 4: 写 engine.rs 占位（后续 Task 填充）**

```rust
use anyhow::Result;
use std::sync::Arc;

pub struct OcrEngine {
    inner: ocr_rs::OcrEngine,
}

impl OcrEngine {
    pub fn instance() -> Result<Arc<OcrEngine>> {
        anyhow::bail!("not implemented yet")
    }

    pub fn recognize(&self, _png_bytes: &[u8]) -> Result<String> {
        anyhow::bail!("not implemented yet")
    }
}
```

- [ ] **Step 5: 写 model.rs**

```rust
use std::path::PathBuf;

pub const DEFAULT_OCR_MODEL: &str = "PP-OCRv6-small";

/// 模型组目录：~/.octopus/models/ocr/<model_name>/
pub fn model_dir(model_name: &str) -> PathBuf {
    octopus_infra::paths::octopus_config_home()
        .join("models")
        .join("ocr")
        .join(model_name)
}

/// 检查模型组三件套是否就绪：det.mnn + rec.mnn + keys.txt
pub fn is_model_ready(model_name: &str) -> bool {
    let dir = model_dir(model_name);
    dir.join("det.mnn").exists()
        && dir.join("rec.mnn").exists()
        && dir.join("keys.txt").exists()
}
```

- [ ] **Step 6: workspace Cargo.toml 加 member**

在 `Cargo.toml` line 2 的 members 列表末尾加 `"crates/ocr"`：

```toml
members = ["crates/infra", "crates/asr-local", "crates/asr-cloud", "crates/server", "crates/cli", "crates/desktop", "crates/llm", "crates/dlp", "crates/download", "crates/clipboard", "crates/ocr"]
```

- [ ] **Step 7: 验证编译**

```bash
cargo build -p octopus-ocr 2>&1 | tail -5
```

Expected: 编译通过（可能有 unused warning）。如果 ocr-rs 编译失败需排查 cmake/cc 依赖。

- [ ] **Step 8: Commit**

```bash
git add crates/ocr/ Cargo.toml
git commit -m "feat(ocr): octopus-ocr crate 骨架（engine/model 占位）"
```

---

### Task 2: OcrEngine 实现

**Files:**
- Modify: `crates/ocr/src/engine.rs`

- [ ] **Step 1: 实现 OcrEngine（完整）**

```rust
use anyhow::{Context, Result};
use std::sync::{Arc, OnceLock};

use crate::model;

pub struct OcrEngine {
    inner: ocr_rs::OcrEngine,
}

static INSTANCE: OnceLock<Arc<OcrEngine>> = OnceLock::new();

impl OcrEngine {
    /// 全局单例，首次调用时懒加载。
    /// model_name 从 app_config.ocr_model 读取，默认 PP-OCRv6-small。
    pub fn instance() -> Result<Arc<OcrEngine>> {
        if let Some(e) = INSTANCE.get() {
            return Ok(e.clone());
        }

        let model_name = octopus_infra::db::load_config_key("ocr_model")
            .unwrap_or_else(|| model::DEFAULT_OCR_MODEL.to_string());

        if !model::is_model_ready(&model_name) {
            anyhow::bail!("OCR 模型未就绪: {}（请检查 ~/.octopus/models/ocr/{}/）", model_name, model_name);
        }

        let dir = model::model_dir(&model_name);
        let det_path = dir.join("det.mnn");
        let rec_path = dir.join("rec.mnn");
        let keys_path = dir.join("keys.txt");

        log::info!("Loading OCR model: {} from {}", model_name, dir.display());

        let inner = ocr_rs::OcrEngine::new(
            det_path.to_str().context("invalid det path")?,
            rec_path.to_str().context("invalid rec path")?,
            keys_path.to_str().context("invalid keys path")?,
            None,
        ).context("Failed to init ocr_rs::OcrEngine")?;

        let engine = Arc::new(OcrEngine { inner });

        // OnceLock::set 如果已设置则忽略（竞争安全，两个线程都加载只是浪费一次）
        let _ = INSTANCE.set(engine.clone());

        Ok(engine)
    }

    /// 识别图片字节（PNG），返回识别文本（多行用 \n 连接）。
    pub fn recognize(&self, png_bytes: &[u8]) -> Result<String> {
        let img = ::image::load_from_memory_with_format(png_bytes, ::image::ImageFormat::Png)
            .context("Failed to decode PNG")?;

        let results = self.inner.recognize(&img)
            .context("OCR recognize failed")?;

        let text: Vec<String> = results.into_iter()
            .map(|r| r.text)
            .collect();

        Ok(text.join("\n"))
    }
}
```

- [ ] **Step 2: 验证编译**

```bash
cargo build -p octopus-ocr 2>&1 | tail -10
```

Expected: 编译通过。如果 ocr-rs API 签名与预期不符（`OcrEngine::new` 参数或 `recognize` 返回类型），需查阅 `cargo doc -p ocr-rs --open` 调整。

- [ ] **Step 3: 写集成测试验证真实模型**

在 `crates/ocr/tests/ocr_integration.rs`：

```rust
use octopus_ocr::OcrEngine;

#[test]
fn test_recognize_real_model() {
    // 跳过条件：模型未就绪
    if !octopus_ocr::model::is_model_ready("PP-OCRv6-small") {
        eprintln!("Skipping: OCR model not ready");
        return;
    }

    let engine = OcrEngine::instance().expect("Failed to init engine");

    // 用一张含中文的测试图片（如果没有就跳过）
    let test_img = std::env::var("OCTOPUS_OCR_TEST_IMAGE").ok();
    let test_img = match test_img {
        Some(p) => p,
        None => {
            eprintln!("Skipping: set OCTOPUS_OCR_TEST_IMAGE=/path/to/test.png");
            return;
        }
    };

    let png_bytes = std::fs::read(&test_img).expect("read test image");
    let text = engine.recognize(&png_bytes).expect("recognize");
    assert!(!text.is_empty(), "OCR should return text");
    println!("OCR result: {}", text);
}
```

- [ ] **Step 4: 运行测试**

```bash
cargo test -p octopus-ocr --test ocr_integration -- --nocapture
```

Expected: 如果模型就绪且有测试图片，输出识别文本。否则打印 skip 信息。

- [ ] **Step 5: Commit**

```bash
git add crates/ocr/
git commit -m "feat(ocr): OcrEngine 实现（单例懒加载 + recognize）"
```

---

### Task 3: store.rs 新增 update_search_text

**Files:**
- Modify: `crates/clipboard/src/store.rs`（toggle_favorite 函数之后，约 line 249）

- [ ] **Step 1: 添加 update_search_text 函数**

在 `toggle_favorite` 函数之后添加：

```rust
/// 更新条目的 search_text（OCR 场景：识别后让图片可搜索）。
pub fn update_search_text(conn: &Connection, id: i64, search_text: &str) -> Result<()> {
    conn.execute(
        "UPDATE clipboard_history SET search_text = ? WHERE id = ?",
        params![search_text, id],
    )?;
    Ok(())
}
```

- [ ] **Step 2: 验证编译**

```bash
cargo build -p octopus-clipboard 2>&1 | tail -3
```

Expected: PASS

- [ ] **Step 3: 运行测试**

```bash
cargo test -p octopus-clipboard 2>&1 | tail -5
```

Expected: 14 passed; 0 failed

- [ ] **Step 4: Commit**

```bash
git add crates/clipboard/src/store.rs
git commit -m "feat(clipboard): update_search_text（OCR 文本回写）"
```

---

### Task 4: DB seed（models + app_config）

**Files:**
- Modify: `crates/infra/src/db.sql`（OCR models seed + app_config seed）

- [ ] **Step 1: 在 db.sql 的 ASR models seed 之后添加 OCR seed**

找到最后一个 `INSERT OR IGNORE INTO models` 语句之后，添加：

```sql

-- ── OCR 模型（domain='ocr'）─────────────────────────────────────────
-- source = det 下载地址；secret_key = rec 下载地址（本地模型复用字段）
INSERT OR IGNORE INTO models (domain, provider, category, model_name, source, secret_key, language, description, is_local, is_enabled, is_streaming)
VALUES
    ('ocr','paddleocr','ocr','PP-OCRv6-small',
     'https://github.com/zibo-chen/rust-paddle-ocr/raw/next/models/PP-OCRv6_small_det.mnn',
     'https://github.com/zibo-chen/rust-paddle-ocr/raw/next/models/PP-OCRv6_small_rec.mnn',
     'auto','PP-OCRv6 small (det 4.7M + rec 10M + keys 73K)，中/英/繁体/日',
     1,1,0);
```

- [ ] **Step 2: 在 app_config seed 末尾添加 ocr_model**

找到最后一个 app_config seed INSERT 之后，添加：

```sql
INSERT OR IGNORE INTO app_config (key, value, category) VALUES
    ('ocr_model', 'PP-OCRv6-small', 'setting');
```

- [ ] **Step 3: 验证编译 + DB 迁移**

```bash
cargo build -p octopus-infra 2>&1 | tail -3
```

- [ ] **Step 4: 手动验证 seed 生效**

```bash
sqlite3 ~/.octopus/octopus.db "SELECT domain, category, model_name, is_enabled FROM models WHERE domain='ocr';"
sqlite3 ~/.octopus/octopus.db "SELECT key, value FROM app_config WHERE key='ocr_model';"
```

Expected: 一行 OCR 模型记录 + 一行 ocr_model 配置。如果未出现，需检查 db.sql 是否在 init_schema 中被 execute_batch。

- [ ] **Step 5: Commit**

```bash
git add crates/infra/src/db.sql
git commit -m "feat(infra): OCR models + app_config seed"
```

---

### Task 5: desktop 新增 ocr_image 命令

**Files:**
- Modify: `crates/desktop/Cargo.toml`（新增 octopus-ocr 依赖）
- Modify: `crates/desktop/src/clipboard_commands.rs`（新增 ocr_image 命令）
- Modify: `crates/desktop/src/main.rs`（注册命令）

- [ ] **Step 1: Cargo.toml 加依赖**

在 `crates/desktop/Cargo.toml` 的 `[dependencies]` 中找到 `# octopus-asr-local` 附近，添加：

```toml
# OCR
octopus-ocr = { path = "../ocr" }
```

- [ ] **Step 2: 实现 ocr_image 命令**

在 `crates/desktop/src/clipboard_commands.rs` 末尾（open_file_item 之后）添加：

```rust
/// 图片条目 OCR：识别文本 → 写 search_text + 写剪贴板 + 新建文档。
#[tauri::command]
pub async fn ocr_image(
    id: i64,
    handle: State<'_, Arc<ClipboardHandle>>,
) -> Result<String, String> {
    // 1. 从 DB 读条目
    let item = octopus_infra::db::with_db(|conn| {
        let items = octopus_clipboard::store::query_history(conn, &QueryFilter {
            filter: "all".into(),
            search: None,
            page: 1,
            size: 1000,
        })?;
        Ok::<_, anyhow::Error>(items.into_iter().find(|i| i.id == id))
    })
    .map_err(|e| e.to_string())?;

    let item = item.ok_or("条目不存在")?;
    if item.item_type != octopus_clipboard::ItemType::Image {
        return Err("非图片条目".into());
    }

    let blob_hash = item.image_meta.as_ref().map(|m| m.blob_hash.clone())
        .ok_or("图片元数据缺失")?;

    // 2. 读原图 PNG
    let orig_path = octopus_clipboard::image::clipboard_images_dir()
        .join(format!("{}.png", blob_hash));
    let png_bytes = std::fs::read(&orig_path).map_err(|e| e.to_string())?;

    // 3. OCR 识别
    let engine = octopus_ocr::engine::OcrEngine::instance()
        .map_err(|e| e.to_string())?;
    let text = engine.recognize(&png_bytes).map_err(|e| e.to_string())?;

    if text.trim().is_empty() {
        return Err("未识别到文本".into());
    }

    // 4. 写 search_text（FTS5 可搜索）
    octopus_infra::db::with_db(|conn| {
        octopus_clipboard::store::update_search_text(conn, id, &text)
    }).map_err(|e| e.to_string())?;

    // 5. 写系统剪贴板
    handle.write_text(&text).map_err(|e| e.to_string())?;

    // 6. 系统文本编辑器新建无标题文档
    open_text_editor_with_content(&text);

    Ok(text)
}

/// 用系统文本编辑器新建无标题文档（不落盘临时文件）。
fn open_text_editor_with_content(text: &str) {
    #[cfg(target_os = "macos")]
    {
        // 转义双引号
        let escaped = text.replace('\\', "\\\\").replace('"', "\\\"");
        let script = format!(
            r#"tell application "TextEdit"
    activate
    make new document with properties {{text:"{}"}}
end tell"#,
            escaped
        );
        let _ = std::process::Command::new("osascript")
            .args(["-e", &script])
            .spawn();
    }
    #[cfg(target_os = "windows")]
    {
        // 剪贴板已有文本，启动 notepad，用户 Ctrl+V
        let _ = std::process::Command::new("notepad").spawn();
    }
    #[cfg(target_os = "linux")]
    {
        // 剪贴板已有文本，启动文本编辑器
        let _ = std::process::Command::new("xdg-open")
            .arg("text://")
            .spawn();
    }
}
```

- [ ] **Step 3: main.rs 注册命令**

在 `crates/desktop/src/main.rs` 的 `invoke_handler` 中，找到 `clipboard_commands::open_file_item,` 之后添加：

```rust
            clipboard_commands::ocr_image,
```

- [ ] **Step 4: 验证编译**

```bash
cargo build -p octopus-desktop --features embedded 2>&1 | tail -5
```

Expected: PASS（可能有 dead_code warning）

- [ ] **Step 5: Commit**

```bash
git add crates/desktop/Cargo.toml crates/desktop/src/clipboard_commands.rs crates/desktop/src/main.rs
git commit -m "feat(desktop): ocr_image 命令（识别+search_text+剪贴板+新建文档）"
```

---

### Task 6: 前端 OCR 按钮（ClipboardItem）

**Files:**
- Modify: `crates/desktop/frontend/src/pages/Clipboard/ClipboardItem.tsx`

- [ ] **Step 1: 添加 ScanText import + OCR 状态 + handler**

在 import 行添加 `ScanText`：

```typescript
import { Star, Mic, Type, Image as ImageIcon, FileText, Trash2, Download, FolderOpen, Copy, ScanText, Loader2, Check } from "lucide-react";
```

在组件函数体内（deletePending 状态附近）添加：

```typescript
  const [ocrLoading, setOcrLoading] = useState(false);
  const [ocrDone, setOcrDone] = useState(false);
```

添加 handler：

```typescript
  const handleOcr = async (e: React.MouseEvent) => {
    e.stopPropagation();
    if (ocrLoading) return;
    setOcrLoading(true);
    try {
      await invoke("ocr_image", { id: item.id });
      setOcrLoading(false);
      setOcrDone(true);
      setTimeout(() => setOcrDone(false), 1000);
    } catch (e) {
      setOcrLoading(false);
      const msg = String(e);
      if (msg.includes("未识别到文本")) {
        setOcrDone(true);
        setTimeout(() => setOcrDone(false), 1000);
      } else {
        console.error(e);
      }
    }
  };
```

- [ ] **Step 2: 在操作按钮区域添加 OCR 按钮**

找到图片保存按钮（`{item.item_type === "image" && (`）之后，文件打开按钮之前，添加 OCR 按钮：

```tsx
        {item.item_type === "image" && (
          <button
            className={cn(
              "p-0.5 transition-opacity",
              ocrLoading || ocrDone
                ? "opacity-100"
                : "opacity-0 group-hover:opacity-60 hover:!opacity-100",
            )}
            onClick={handleOcr}
            disabled={ocrLoading}
            title="OCR 识别"
          >
            {ocrLoading ? (
              <Loader2 className="w-3.5 h-3.5 text-stone-500 animate-spin" />
            ) : ocrDone ? (
              <Check className="w-3.5 h-3.5 text-emerald-600" />
            ) : (
              <ScanText className="w-3.5 h-3.5 text-stone-500 hover:text-stone-800" />
            )}
          </button>
        )}
```

- [ ] **Step 3: 构建前端**

```bash
cd crates/desktop/frontend && npm run build 2>&1 | tail -5
```

Expected: PASS

- [ ] **Step 4: Commit**

```bash
git add crates/desktop/frontend/ crates/desktop/dist/
git commit -m "feat(clipboard): 剪贴板浮窗 OCR 按钮（ScanText + 三态）"
```

---

### Task 7: 前端 OCR 按钮（ClipboardPanel）

**Files:**
- Modify: `crates/desktop/frontend/src/pages/Settings/ClipboardPanel.tsx`

- [ ] **Step 1: 在 ClipboardRow 子组件添加 OCR 能力**

在 import 添加 `ScanText, Loader2, Check`：

```typescript
import {
  Star, Mic, Type, Image as ImageIcon, FileText,
  LayoutGrid, Search, Trash2, Copy, Download, FolderOpen,
  ScanText, Loader2, Check,
} from "lucide-react";
```

在 ClipboardRow 函数体内（deletePending 状态附近）添加：

```typescript
  const [ocrLoading, setOcrLoading] = useState(false);
  const [ocrDone, setOcrDone] = useState(false);
```

添加 handler（复用 ClipboardItem 逻辑）：

```typescript
  const handleOcr = async (e: React.MouseEvent) => {
    e.stopPropagation();
    if (ocrLoading) return;
    setOcrLoading(true);
    try {
      await invoke("ocr_image", { id: item.id });
      setOcrLoading(false);
      setOcrDone(true);
      setTimeout(() => setOcrDone(false), 1000);
    } catch (e) {
      setOcrLoading(false);
      const msg = String(e);
      if (msg.includes("未识别到文本")) {
        setOcrDone(true);
        setTimeout(() => setOcrDone(false), 1000);
      } else {
        showToast("OCR 失败：" + e);
      }
    }
  };
```

- [ ] **Step 2: 在行操作区域添加 OCR 按钮**

找到图片保存按钮（`{item.item_type === "image" && (` 块）之后，文件打开按钮之前，添加：

```tsx
        {item.item_type === "image" && (
          <button
            className={cn(
              "p-1 rounded transition-opacity",
              ocrLoading || ocrDone
                ? "opacity-100"
                : "opacity-0 group-hover:opacity-50 hover:!opacity-100",
            )}
            onClick={handleOcr}
            disabled={ocrLoading}
            title="OCR 识别"
          >
            {ocrLoading ? (
              <Loader2 className="w-3.5 h-3.5 text-stone-500 animate-spin" />
            ) : ocrDone ? (
              <Check className="w-3.5 h-3.5 text-emerald-600" />
            ) : (
              <ScanText className="w-3.5 h-3.5 text-stone-500 hover:text-stone-800" />
            )}
          </button>
        )}
```

- [ ] **Step 3: 构建前端**

```bash
cd crates/desktop/frontend && npm run build 2>&1 | tail -5
```

Expected: PASS

- [ ] **Step 4: Commit**

```bash
git add crates/desktop/frontend/ crates/desktop/dist/
git commit -m "feat(clipboard): 管理页 OCR 按钮（ClipboardRow + 三态）"
```

---

### Task 8: load_config_key 辅助函数（如不存在）

**Files:**
- Modify: `crates/infra/src/db.rs`

- [ ] **Step 1: 检查 load_config_key 是否已存在**

```bash
grep -n "pub fn load_config_key" crates/infra/src/db.rs
```

如果已存在，跳过此 Task。如果不存在：

- [ ] **Step 2: 添加 load_config_key 函数**

在 `with_db` 函数附近添加：

```rust
/// 读取 app_config 表中某个 key 的值。
pub fn load_config_key(key: &str) -> Option<String> {
    with_db(|conn| {
        conn.query_row(
            "SELECT value FROM app_config WHERE key = ?",
            params![key],
            |row| row.get::<_, String>(0),
        )
    })
    .ok()
}
```

- [ ] **Step 3: 验证编译**

```bash
cargo build -p octopus-infra 2>&1 | tail -3
```

Expected: PASS

- [ ] **Step 4: Commit**

```bash
git add crates/infra/src/db.rs
git commit -m "feat(infra): load_config_key 辅助函数"
```

---

### Task 9: 端到端验证

- [ ] **Step 1: 完整构建**

```bash
cd crates/desktop/frontend && npm run build
cd .. && cargo build --features embedded 2>&1 | tail -5
```

Expected: 全部 PASS

- [ ] **Step 2: 运行应用，复制一张含文字的截图**

```bash
./run-octopus.sh
```

然后：
1. Cmd+Shift+4 截一张含文字的截图
2. 打开剪贴板浮窗（Alt+V）
3. 找到图片条目，点击 OCR 按钮
4. 观察：按钮 spin → ✓ → TextEdit 新建文档弹出含识别文本

- [ ] **Step 3: 验证 search_text 可搜索**

回到剪贴板浮窗，在搜索框输入 OCR 文本中的关键词 → 图片条目应出现在搜索结果中。

- [ ] **Step 4: 验证管理页 OCR**

打开 Settings → 剪贴板 tab → 对图片条目点 OCR 按钮 → 同样行为。

- [ ] **Step 5: 最终 Commit（如有修复）**

```bash
git add -A
git commit -m "feat(ocr): 端到端验证通过"
```

---

## Spec Coverage（自审）

| spec 章节 | 实现 task |
|---|---|
| §1 架构（crate 结构） | Task 1 |
| §1.3 engine.rs 接口 | Task 2 |
| §1.4 model.rs | Task 1 |
| §2 模型管理（models 表 + app_config） | Task 4 |
| §3 OCR 触发流程 | Task 5 |
| §3.2 结果处理（search_text + 剪贴板 + 新建文档） | Task 5 |
| §4 前端集成（按钮 + 状态机） | Task 6 + 7 |
| §5 数据流（update_search_text） | Task 3 |
| §5.2 models seed | Task 4 |
| §5.3 app_config seed | Task 4 |
| §6 错误处理（空文本/模型未就绪） | Task 5 + 6/7 |
| §7 依赖变更（ocr-rs） | Task 1 |
| load_config_key（engine 依赖） | Task 8 |
