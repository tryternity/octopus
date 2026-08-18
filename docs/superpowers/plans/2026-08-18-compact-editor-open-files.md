# CompactEditor 打开已存在文件 实施计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** CompactEditor 支持打开磁盘文件——工具栏按钮（文件选择器）+ 拖拽双入口；文本开 file tab（编辑/保存写回），图片入库开图片 tab（ImagePreview 全能力）。

**Architecture:** 新命令 `open_files_in_editor` 纯核心分流（`collect_open_tabs`：图片走 watcher 同款入库镜像、文本走 file tab payload），经泛化的 `open_tabs_batched`（emit-or-pending + mounted 检测 + 一次建窗）批量开 tab；前端双入口收敛同一 invoke。

**Tech Stack:** Rust（Tauri 2、image 0.25 补解码 feature、clipboard 入库 API）、React + plugin-dialog + onDragDropEvent + vitest。

**Spec:** `docs/superpowers/specs/2026-08-18-compact-editor-open-files-design.md`

## Global Constraints

- **开发隔离**：继续在 `.worktree/markdown-conversion` 分支开发，未经明确指令不进 main。
- **TDD**：`is_image_ext` / `collect_open_tabs` / `normalizeDialogSelection` 测试先行。
- **复用**：图片入库逐行镜像 `watcher.rs:165-211`（含 `find_by_content_hash` 历史级去重——比 spec §1 范围外描述更好，白得）；开 tab 走 `open_tabs_batched` 泛化而非新写。
- **DB 测试隔离**：`collect_open_tabs` 图片路径触达 `with_db`——test mod 必须 `init_test_db()` Once 样板（AGENTS.md 测试隔离规约）。
- **casing**：`OpenFilesResult` 与 `PendingTabFull` 均已 camelCase 序列化；前端 interface 一一对应。
- **0 warning**：每 task 编译 0 error 0 warning。
- **依赖**：desktop `image` 补 gif/webp/bmp/tiff 解码 feature（现仅 png/jpeg——`load_from_memory` 解不了其余格式）。

---

### Task 1: 后端纯核心——is_image_ext + ingest_image_file + collect_open_tabs（TDD）

**Files:**
- Modify: `crates/desktop/Cargo.toml:38`（image features 扩展）
- Modify: `crates/desktop/src/commands/compact_editor_commands.rs`（新增 fns + tests）

**Interfaces:**
- Consumes: `octopus_clipboard::image::{hash_rgba, encode_image}`、`octopus_clipboard::store::{find_by_content_hash, touch_created_at, insert_image_data, insert_clipboard_item, NewClipboardItem, iso_now}`、`octopus_clipboard::model::{ItemType, MetaInfo}`、`octopus_sync::store::md5_hex`
- Produces: `fn is_image_ext(ext: &str) -> bool`、`fn ingest_image_file(path: &Path) -> Result<(String, i64, i64), String>`（imageId, w, h）、`fn collect_open_tabs(paths: Vec<String>) -> (Vec<PendingTabFull>, Vec<String>)`。Task 2 消费。

- [ ] **Step 1: image 解码 feature 扩展**

`crates/desktop/Cargo.toml:38` 改：

```toml
image = { version = "0.25", default-features = false, features = ["png", "jpeg", "gif", "webp", "bmp", "tiff"] }
```

- [ ] **Step 2: 写失败测试（compact_editor_commands.rs tests mod 追加）**

test mod 顶部加 DB 隔离样板 + fixture：

```rust
    // ── open_files_in_editor（spec 2026-08-18-compact-editor-open-files）──

    // 图片入库触达 with_db——init_test_db 切 in-memory，防绑开发库（AGENTS.md 测试隔离）
    static OPEN_FILES_DB_SETUP: std::sync::Once = std::sync::Once::new();
    fn setup_open_files_test_db() {
        OPEN_FILES_DB_SETUP.call_once(|| {
            octopus_infra::db::init_test_db();
        });
    }

    /// 1×1 红色 PNG（经典 70 字节 base64）。
    const TINY_PNG_B64: &str =
        "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAEhQGAhKmMIQAAAABJRU5ErkJggg==";

    fn tmp_path(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir()
            .join(format!("octopus-open-files-{}-{}", std::process::id(), name));
        std::fs::create_dir_all(&dir).unwrap();
        dir.join(name)
    }

    #[test]
    fn test_is_image_ext_matrix() {
        for ext in ["png", "jpg", "jpeg", "gif", "webp", "bmp", "tiff", "tif"] {
            assert!(is_image_ext(ext), "ext={}", ext);
            assert!(is_image_ext(&ext.to_uppercase()), "大小写不敏感：{}", ext);
        }
        assert!(is_image_ext(".PNG"), "前导点容忍");
        for ext in ["md", "txt", "pdf", "docx", "", "svg"] {
            assert!(!is_image_ext(ext), "ext={} 应非图片", ext);
        }
    }

    #[test]
    fn test_collect_open_tabs_text_file() {
        let p = tmp_path("note.md");
        std::fs::write(&p, b"# hello").unwrap();
        let (tabs, errors) = collect_open_tabs(vec![p.to_string_lossy().to_string()]);
        assert!(errors.is_empty(), "errors={:?}", errors);
        assert_eq!(tabs.len(), 1);
        assert_eq!(tabs[0].source, "file");
        assert_eq!(tabs[0].text, "# hello");
        assert_eq!(tabs[0].file_path.as_deref(), Some(p.to_string_lossy().as_ref()));
        // itemId = md5(路径) 前 16 hex → i64（与 open_disk_file_in_compact_editor 同规则）
        let hash = octopus_sync::store::md5_hex(p.to_string_lossy().as_bytes());
        let expect = i64::from_str_radix(&hash[..16], 16).unwrap_or(0).to_string();
        assert_eq!(tabs[0].item_id, expect);
    }

    #[test]
    fn test_collect_open_tabs_non_utf8_rejected() {
        let p = tmp_path("bad.bin");
        std::fs::write(&p, [0xFFu8, 0xFE, 0x00, 0x01]).unwrap();
        let (tabs, errors) = collect_open_tabs(vec![p.to_string_lossy().to_string()]);
        assert!(tabs.is_empty());
        assert_eq!(errors.len(), 1);
        assert!(errors[0].contains("非 UTF-8"), "err={}", errors[0]);
        assert!(errors[0].contains("bad.bin"));
    }

    #[test]
    fn test_collect_open_tabs_dir_rejected() {
        let dir = tmp_path("adir");
        std::fs::create_dir_all(&dir).unwrap();
        let (tabs, errors) = collect_open_tabs(vec![dir.to_string_lossy().to_string()]);
        assert!(tabs.is_empty());
        assert!(errors[0].contains("暂不支持文件夹"));
    }

    #[test]
    fn test_collect_open_tabs_image_ingests() {
        setup_open_files_test_db();
        let p = tmp_path("tiny.png");
        std::fs::write(&p, base64_decode(TINY_PNG_B64)).unwrap();
        let (tabs, errors) = collect_open_tabs(vec![p.to_string_lossy().to_string()]);
        assert!(errors.is_empty(), "errors={:?}", errors);
        assert_eq!(tabs.len(), 1);
        assert_eq!(tabs[0].source, "clipboard");
        assert_eq!(tabs[0].item_type, "image");
        assert_eq!(tabs[0].img_width, 1);
        assert_eq!(tabs[0].img_height, 1);
        assert!(!tabs[0].item_id.is_empty());
    }

    #[test]
    fn test_collect_open_tabs_mixed_partial_success() {
        setup_open_files_test_db();
        let ok = tmp_path("ok.md");
        std::fs::write(&ok, b"fine").unwrap();
        let bad = tmp_path("no.txt");
        std::fs::write(&bad, [0xFFu8, 0xFE]).unwrap();
        let img = tmp_path("i.png");
        std::fs::write(&img, base64_decode(TINY_PNG_B64)).unwrap();
        let (tabs, errors) = collect_open_tabs(vec![
            ok.to_string_lossy().to_string(),
            bad.to_string_lossy().to_string(),
            img.to_string_lossy().to_string(),
        ]);
        assert_eq!(tabs.len(), 2, "文本+图片成功");
        assert_eq!(errors.len(), 1);
        assert!(errors[0].contains("no.txt"));
    }

    /// 最小 base64 解码（测试专用，避免引依赖）。
    fn base64_decode(s: &str) -> Vec<u8> {
        const TABLE: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let mut out = Vec::new();
        let mut buf = 0u32;
        let mut bits = 0u32;
        for c in s.bytes().filter(|c| *c != b'=' && !c.is_ascii_whitespace()) {
            let v = TABLE.iter().position(|t| *t == c).expect("非法 base64") as u32;
            buf = (buf << 6) | v;
            bits += 6;
            if bits >= 8 {
                bits -= 8;
                out.push((buf >> bits) as u8);
            }
        }
        out
    }
```

- [ ] **Step 3: 跑测试确认编译失败（红）**

```bash
cargo test -p octopus-desktop test_collect_open_tabs 2>&1 | tail -3
```

Expected: 编译错误（`is_image_ext` / `collect_open_tabs` 未定义）。

- [ ] **Step 4: 实现三个函数（compact_editor_commands.rs，`open_disk_file_in_compact_editor` 之后）**

```rust
// ── 打开已存在文件（spec 2026-08-18-compact-editor-open-files）──

/// 图片扩展名封闭清单（spec §1）；其余一律尝试 UTF-8 文本读。
/// 注：svg 是文本（可编辑），归文本路径。
fn is_image_ext(ext: &str) -> bool {
    matches!(
        ext.trim_start_matches('.').to_ascii_lowercase().as_str(),
        "png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp" | "tiff" | "tif",
    )
}

/// 文件图片入库（镜像 watcher.rs:165-211 的 ingest 组合）：
/// 读 bytes → 解码 → hash_rgba 去重（find_by_content_hash 命中则 touch 已有行）
/// → insert_image_data + insert_clipboard_item(type=image)。返回 (historyId, w, h)。
fn ingest_image_file(path: &std::path::Path) -> Result<(String, i64, i64), String> {
    let bytes = std::fs::read(path).map_err(|e| format!("读取失败: {}", e))?;
    let dyn_img = ::image::load_from_memory(&bytes)
        .map_err(|_| "图片解码失败".to_string())?;
    let rgba_img = dyn_img.to_rgba8();
    let (w, h) = (rgba_img.width(), rgba_img.height());
    let rgba = rgba_img.to_vec();
    let hash = octopus_clipboard::image::hash_rgba(&rgba);

    // 历史级去重（watcher 同款）：同图 touch 已有行直接复用 id，不重复入库
    let existing = octopus_infra::db::with_db(|conn| {
        octopus_clipboard::store::find_by_content_hash(conn, &hash)
    })
    .map_err(|e| format!("DB 查询失败: {}", e))?;
    if let Some(id) = existing {
        let _ = octopus_infra::db::with_db(|conn| {
            octopus_clipboard::store::touch_created_at(conn, &id)
        });
        return Ok((id, w as i64, h as i64));
    }

    let dyn_img = ::image::DynamicImage::ImageRgba8(
        ::image::RgbaImage::from_raw(w, h, rgba).ok_or("RgbaImage::from_raw failed")?,
    );
    let encoded = octopus_clipboard::image::encode_image(&dyn_img)
        .map_err(|e| format!("编码失败: {}", e))?;
    octopus_infra::db::with_db(|conn| {
        octopus_clipboard::store::insert_image_data(
            conn, &hash, &encoded.image_blob, &encoded.thumb_blob, w as i64, h as i64,
        )
    })
    .map_err(|e| format!("图片存储失败: {}", e))?;

    let id = uuid::Uuid::new_v4().to_string();
    octopus_infra::db::with_db(|conn| {
        octopus_clipboard::store::insert_clipboard_item(
            conn,
            &octopus_clipboard::store::NewClipboardItem {
                id: id.clone(),
                item_type: octopus_clipboard::model::ItemType::Image,
                content: String::new(),
                ref_data: Some(hash.clone()),
                meta_info: Some(octopus_clipboard::model::MetaInfo {
                    w: Some(w as i64),
                    h: Some(h as i64),
                    size: Some(format!("{}KB", encoded.image_blob.len() / 1024)),
                    ..Default::default()
                }),
                created_at: octopus_clipboard::store::iso_now(),
                has_thumbnail: Some(1),
                is_rich: false,
            },
        )
    })
    .map_err(|e| format!("历史写入失败: {}", e))?;
    Ok((id, w as i64, h as i64))
}

/// 分流 + 组装 tab（纯核心，无 AppHandle 便于单测，spec §3.3）：
/// 图片 → 入库图片 tab（source="clipboard"，前端 loadAndAddTab 识别）；
/// 其余 → UTF-8 文本读 → file tab（md5 路径 itemId，与 file tab 去重规则一致）。
/// 失败逐个进 errors（`<文件名>（<原因>）`），不中断其他文件。
fn collect_open_tabs(paths: Vec<String>) -> (Vec<PendingTabFull>, Vec<String>) {
    let mut tabs = Vec::new();
    let mut errors = Vec::new();
    for p in paths {
        let path = std::path::PathBuf::from(&p);
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| p.clone());
        if path.is_dir() {
            errors.push(format!("{}（暂不支持文件夹）", name));
            continue;
        }
        if !path.exists() {
            errors.push(format!("{}（文件不存在）", name));
            continue;
        }
        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_string();
        if is_image_ext(&ext) {
            match ingest_image_file(&path) {
                Ok((id, w, h)) => tabs.push(PendingTabFull {
                    item_id: id,
                    source: "clipboard".into(),
                    item_type: "image".into(),
                    text: String::new(),
                    img_width: w,
                    img_height: h,
                    is_temp: false,
                    mode: None,
                    original_text: None,
                    translated_text: None,
                    translate_session_id: None,
                    file_path: None,
                }),
                Err(e) => errors.push(format!("{}（{}）", name, e)),
            }
        } else {
            match std::fs::read_to_string(&path) {
                Ok(text) => {
                    let hash = octopus_sync::store::md5_hex(p.as_bytes());
                    let item_id = i64::from_str_radix(&hash[..16], 16).unwrap_or(0).to_string();
                    tabs.push(PendingTabFull {
                        item_id,
                        source: "file".into(),
                        item_type: "text".into(),
                        text,
                        img_width: 0,
                        img_height: 0,
                        is_temp: false,
                        mode: None,
                        original_text: None,
                        translated_text: None,
                        translate_session_id: None,
                        file_path: Some(p),
                    });
                }
                Err(_) => errors.push(format!("{}（非 UTF-8 文本或读取失败）", name)),
            }
        }
    }
    (tabs, errors)
}
```

注：`PendingTabFull` 若字段可见性不足（private），同文件内可直接构造；`ItemType`/`MetaInfo` 的路径以 `octopus_clipboard::model` 实际导出为准（watcher 用 `crate::model`，desktop 侧走完整路径）。

- [ ] **Step 5: 跑测试确认全绿**

```bash
cargo test -p octopus-desktop test_collect_open_tabs test_is_image_ext 2>&1 | grep "test result"
cargo build -p octopus-desktop 2>&1 | grep -cE "^(error|warning)"
```

Expected: 6 个新测试全过（`test_is_image_ext_matrix` + 5 个 collect）；编译 0 warning。

- [ ] **Step 6: Commit**

```bash
git add crates/desktop/Cargo.toml crates/desktop/Cargo.lock crates/desktop/src/commands/compact_editor_commands.rs
git commit -m "feat(compact-editor): open-files 纯核心——is_image_ext/图片入库镜像/collect_open_tabs（TDD）"
```

---

### Task 2: open_tabs_batched 泛化 + open_files_in_editor 命令 + 注册

**Files:**
- Modify: `crates/desktop/src/commands/compact_editor_commands.rs`
- Modify: `crates/desktop/src/core/invoke_handler.rs:172` 区（注册命令）

**Interfaces:**
- Consumes: `collect_open_tabs`（Task 1）、`PENDING_TABS` / `create_compact_editor_window` / `WINDOW_LABEL`（现有）
- Produces: `#[tauri::command] open_files_in_editor(paths: Vec<String>, app) -> Result<OpenFilesResult, String>`（`OpenFilesResult { errors: Vec<String> }` camelCase）。Task 3 前端 invoke。

- [ ] **Step 1: 泛化 open_tabs_batched**

从 `open_compact_editor_tabs`（:243）抽出批量机制（行为零变化——原函数转调）：

```rust
/// 批量开 tab（完整 payload 直传，不查 DB）。2026-08-18 从 open_compact_editor_tabs
/// 泛化（open-files 复用，spec §3.2）：
/// - 窗口存在且 React 已 mount（PENDING_TABS 空）→ 逐个 emit + show/focus
/// - 窗口存在未 mount → 全部 push pending（emit 会丢——listener 未注册）
/// - 窗口不存在 → push pending + 一次建窗（批量一次，避免连续单开的中间态丢 tab）
fn open_tabs_batched(tabs: Vec<PendingTabFull>, app: &tauri::AppHandle) {
    if tabs.is_empty() {
        return;
    }
    if let Some(window) = app.get_webview_window(WINDOW_LABEL) {
        let react_mounted = PENDING_TABS.lock().is_empty();
        if react_mounted {
            for tab in tabs {
                let _ = window.emit("compact-editor://open-tab", tab);
            }
            let _ = window.show();
            let _ = window.set_focus();
        } else {
            PENDING_TABS.lock().extend(tabs);
        }
    } else {
        PENDING_TABS.lock().extend(tabs);
        create_compact_editor_window(app, None);
    }
}
```

`open_compact_editor_tabs` 改为组装后转调（`push_pending_tab` 的 DB 组装逻辑抽 `fn build_pending_tab(item_id: &str, source: &str) -> PendingTabFull`，`push_pending_tab` 保持原行为供其他调用方）。emit `PendingTabFull` 直接序列化（camelCase，字段覆盖 file/clipboard 两类前端分支）。

- [ ] **Step 2: open_files_in_editor 命令（compact_editor_commands.rs）**

```rust
/// 打开磁盘文件结果（camelCase，spec §3.3）。成功的 tab 经事件/pending 送出。
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OpenFilesResult {
    pub errors: Vec<String>,
}

/// 打开已存在的文件（spec 2026-08-18-compact-editor-open-files）：
/// 图片入库开图片 tab、文本开 file tab；失败逐个收集返回，命令本身不 Err。
#[tauri::command]
pub async fn open_files_in_editor(
    paths: Vec<String>,
    app: AppHandle,
) -> Result<OpenFilesResult, String> {
    // 图片解码 + 多文件 IO——spawn_blocking 防卡 runtime
    let (tabs, errors) = tokio::task::spawn_blocking(move || collect_open_tabs(paths))
        .await
        .map_err(|e| format!("打开任务异常: {}", e))?;
    // create_compact_editor_window 含 set_dock_icon 需主线程（同 markdown 分支模式）
    let ah = app.clone();
    let _ = app.run_on_main_thread(move || {
        open_tabs_batched(tabs, &ah);
    });
    Ok(OpenFilesResult { errors })
}
```

- [ ] **Step 3: invoke_handler 注册（`core/invoke_handler.rs` compact_editor_commands 区，:175 `get_clipboard_item_text` 附近）**

```rust
            crate::commands::compact_editor_commands::open_files_in_editor,
```

- [ ] **Step 4: 编译 + 既有回归**

```bash
cargo build -p octopus-desktop 2>&1 | grep -cE "^(error|warning)"
cargo test -p octopus-desktop compact_editor 2>&1 | grep "test result"
```

Expected: 0 warning；compact_editor 相关测试全过（含 Task 1 的 6 个）。

- [ ] **Step 5: Commit**

```bash
git add crates/desktop/src
git commit -m "feat(compact-editor): open_tabs_batched 泛化 + open_files_in_editor 命令"
```

---

### Task 3: 前端——打开按钮 + 拖拽 + toast + i18n（TDD）

**Files:**
- Create: `crates/desktop/frontend/src/pages/CompactEditor/openFilesUtils.ts` + `openFilesUtils.test.ts`
- Modify: `crates/desktop/frontend/src/pages/CompactEditor/index.tsx`
- Modify: `crates/desktop/frontend/src/locales/zh-CN.yaml` + `en.yaml`（editor 段）

**Interfaces:**
- Consumes: Task 2 的 `open_files_in_editor` 命令（`{ paths }` → `{ errors }`）
- Produces: `normalizeDialogSelection(selected): string[]`、`TEXT_IMAGE_EXTS`

- [ ] **Step 1: 写失败测试（openFilesUtils.test.ts）**

```ts
import { describe, expect, it } from "vitest";
import { normalizeDialogSelection, TEXT_IMAGE_EXTS } from "./openFilesUtils";

describe("openFilesUtils", () => {
  it("单选返回单元素数组", () => {
    expect(normalizeDialogSelection("/tmp/a.md")).toEqual(["/tmp/a.md"]);
  });
  it("多选原样返回", () => {
    expect(normalizeDialogSelection(["/a.md", "/b.png"])).toEqual(["/a.md", "/b.png"]);
  });
  it("null / 取消返回空", () => {
    expect(normalizeDialogSelection(null)).toEqual([]);
  });
  it("扩展名清单含文本与图片", () => {
    for (const ext of ["md", "txt", "json", "py", "png", "jpg", "webp"]) {
      expect(TEXT_IMAGE_EXTS).toContain(ext);
    }
  });
});
```

- [ ] **Step 2: 跑红 → 实现 openFilesUtils.ts**

```bash
cd crates/desktop/frontend && npx vitest run src/pages/CompactEditor/openFilesUtils.test.ts
```

Expected: FAIL（模块不存在）。然后实现：

```ts
// 打开文件入口共用（spec 2026-08-18-compact-editor-open-files §4）。
// 选择器 filter 提示用（后端才是真相源——拖拽不限扩展名，由 collect_open_tabs 分流）。
export const TEXT_IMAGE_EXTS = [
  "md", "markdown", "txt", "log", "json", "yml", "yaml", "toml", "xml", "csv",
  "html", "htm", "js", "jsx", "ts", "tsx", "py", "rs", "sh", "css", "svg",
  "png", "jpg", "jpeg", "gif", "webp", "bmp", "tiff", "tif",
];

/// plugin-dialog open() 返回值归一化为路径数组（null=取消）。
export function normalizeDialogSelection(selected: string | string[] | null): string[] {
  if (selected == null) return [];
  return Array.isArray(selected) ? selected : [selected];
}
```

- [ ] **Step 3: index.tsx 三处改动**

imports（顶部）：

```ts
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { FolderOpen } from "lucide-react";
import { useToast, Toast } from "@/lib/useToast";
import { normalizeDialogSelection, TEXT_IMAGE_EXTS } from "./openFilesUtils";
```

组件内（hooks 区）：

```ts
  const { toast, showToast, dismissToast } = useToast();
  const [dragOver, setDragOver] = useState(false);

  // 打开核心：按钮与拖拽共用。失败 warning toast（不自动消失——需看清单）。
  const openFilesCore = useCallback(async (paths: string[]) => {
    if (paths.length === 0) return;
    try {
      const res = await invoke<{ errors: string[] }>("open_files_in_editor", { paths });
      if (res.errors.length > 0) {
        showToast(
          t("editor.openFailed", { n: String(res.errors.length), detail: res.errors.join("、") }),
          "warning",
        );
      }
    } catch (e) {
      showToast(String(e), "error");
    }
  }, [showToast, t]);
  const openFilesCoreRef = useRef(openFilesCore);
  openFilesCoreRef.current = openFilesCore;

  const handleOpenFiles = useCallback(async () => {
    const selected = await openDialog({
      multiple: true,
      filters: [{ name: "文本与图片", extensions: TEXT_IMAGE_EXTS }],
    });
    await openFilesCoreRef.current(normalizeDialogSelection(selected));
  }, []);

  // OS 文件拖入（Finder → 窗口）：Tauri onDragDropEvent（TerminalPane 同模式——
  // WKWebView 下 HTML5 DnD 不可靠）。listener 一次挂载，回调经 ref 稳定化。
  useEffect(() => {
    let disposed = false;
    let unlisten: (() => void) | null = null;
    getCurrentWebview()
      .onDragDropEvent((e) => {
        const p = e.payload;
        if (p.type === "enter" || p.type === "over") setDragOver(true);
        else if (p.type === "leave") setDragOver(false);
        else if (p.type === "drop") {
          setDragOver(false);
          if (p.paths.length) openFilesCoreRef.current(p.paths);
        }
      })
      .then((fn) => {
        if (disposed) fn();
        else unlisten = fn;
      })
      .catch((err) => console.error("[CompactEditor] drag-drop listen failed:", err));
    return () => {
      disposed = true;
      unlisten?.();
    };
  }, []);
```

tab 栏改动——`{tabs.length > 0 && (` 的条件渲染改为**常驻**（0 tab 时也显示打开按钮），tabs.map 之后追加按钮：

```tsx
      {/* tab 栏（常驻——0 tab 时也要能打开文件） */}
      <div className={`flex-shrink-0 flex items-center gap-0.5 px-1.5 py-1 border-b border-border bg-muted overflow-x-auto thin-scrollbar ${dragOver ? "ring-2 ring-voice ring-inset" : ""}`}>
        {tabs.map((tab, i) => ( /* 原有 map 体不动 */ ))}
        <button
          type="button"
          title={t("editor.openFile")}
          onClick={handleOpenFiles}
          className="flex-shrink-0 flex items-center gap-1 px-2 py-1 rounded-md text-xs text-muted-foreground hover:bg-accent hover:text-foreground transition-colors"
        >
          <FolderOpen className="w-3.5 h-3.5" />
        </button>
      </div>
```

（原 `{tabs.length > 0 && (<div…>…)}` 外层条件去掉；0 tab 时空态提示保留在内容区。）

根 div 加拖拽高亮 + 渲染 Toast（return 尾部）：

```tsx
    <div className={`flex flex-col h-full bg-background ${dragOver ? "ring-2 ring-voice ring-inset" : ""}`}>
      {/* …原有内容… */}
      <Toast toast={toast} onClose={dismissToast} />
    </div>
```

- [ ] **Step 4: i18n（zh-CN.yaml / en.yaml 的 editor 段，`previewTruncated` 后）**

zh-CN：

```yaml
  openFile: 打开文件
  openFailed: ${n} 个文件打开失败：${detail}
```

en：

```yaml
  openFile: Open File
  openFailed: ${n} file(s) failed to open: ${detail}
```

（`t()` 插值参数若只支持单参数对象，`{ n, detail }` 传法对照 `charCount` 的 `{n}` 用法；i18n 实现按 `${name}` 插值支持多键。）

- [ ] **Step 5: 验证**

```bash
cd crates/desktop/frontend
npx vitest run src/pages/CompactEditor/
npx tsc --noEmit
npm run build
```

Expected: CompactEditor 全部测试过（含 openFilesUtils 4 个）；tsc 0 error；build 成功。

- [ ] **Step 6: Commit**

```bash
git add crates/desktop/frontend
git commit -m "feat(compact-editor): 打开文件双入口——工具栏按钮+选择器 / 拖拽 + 失败 toast"
```

---

### Task 4: 全量验证 + 文档同步

**Files:**
- Modify: `docs/features/compact-editor.md`
- Modify: `docs/superpowers/specs/2026-08-18-compact-editor-open-files-design.md`（实施注记）

- [ ] **Step 1: 全量验证**

```bash
cargo build 2>&1 | grep -cE "^(error|warning)"
cargo test 2>&1 | grep -cE "FAILED|error\["
cd crates/desktop/frontend && npm run build
```

Expected: 全 0 / build 成功。

- [ ] **Step 2: 手动 e2e 冒烟（可选推荐）**

1. CompactEditor 工具栏「打开」→ 选一个 .md → file tab 打开、可编辑、Cmd+S 写回
2. 拖一张 .png 进窗口 → 图片 tab（OCR/二维码/缩放可用）、剪贴板历史多一条
3. 拖一个文件夹 → warning toast「暂不支持文件夹」
4. 拖一个非 UTF-8 二进制 → toast「非 UTF-8 文本」

- [ ] **Step 3: 文档同步**

`docs/features/compact-editor.md` 追加「打开已存在文件」段（spec §2 数据流 + 图片入库语义 + 历史级去重 + MAX_IMAGE_TABS 约束 + 拖拽/选择器双入口）。

spec 实施注记：① 图片历史级去重实际启用（`find_by_content_hash` 镜像 watcher，优于 spec §1「范围外」的保守表述）；② desktop `image` crate 补 gif/webp/bmp/tiff 解码 feature；③ 任何其他偏差。

- [ ] **Step 4: Commit**

```bash
git add docs
git commit -m "docs: 同步 CompactEditor 打开文件功能"
```

---

## Self-Review 记录

- **Spec coverage**：§1 双入口/类型/策略 → Task 3 + Task 1 分流；§2 数据流 → Task 1-2；§3.1-3.3 → Task 1-2；§4 前端五点 → Task 3；§5 错误表 → Task 1 实现 + Task 3 toast；§6 测试 → 各 task TDD 步骤；§7 文档 → Task 4。无缺口。
- **占位符**：无 TBD；「原有 map 体不动」给出了精确锚点（tabs.map 结构已在前文核实）。
- **类型一致性**：`PendingTabFull` 十一字段在 Task 1 两处构造与 Task 2 emit 一致；`OpenFilesResult { errors }` 与 Task 3 `invoke<{ errors: string[] }>` 对应；`is_image_ext`/`ingest_image_file`/`collect_open_tabs` 签名跨 task 一致。
- **实现期风险**：① `PendingTabFull`/`store::iso_now` 等可见性以编译器报错为准微调路径；② i18n 多键插值若实现只支持单键，`openFailed` 拆两条文案；③ `emit(PendingTabFull)` 的前端 file 分支依赖 `p.filePath` 字段——`PendingTabFull` 已 camelCase 序列化（`get_pending_compact_tabs` 同源），风险低。
