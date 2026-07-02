# IPC 二进制传输改造

**日期**: 2026-07-02
**状态**: ✅ 实施完成（3 层全部落地：scroll://done 双向往返消除 + 前端→Rust Raw body + Rust→前端 ipc::Response）
**分支**: `optimize-capx`

---

## 一、改造范围

### 层 1：消除 scroll://done 双向往返

Rust 端已有 `png_bytes`，保存模式下直接弹对话框，不经过前端。

- `scroll://done` payload 移除 `png_base64`，只传 `{ id }`
- 前端保存按钮改为：先 `invoke("stop_scroll_capture", { mode: "save" })`，保存对话框由 Rust 端在停止后直接弹出

### 层 2：前端→Rust 改用 Raw body

| 命令 | 当前 | 改后 |
|------|------|------|
| `copy_image_to_clipboard` | `{ pngBase64: String }` | Raw body + headers |
| `save_image_dialog` | `{ pngBase64: String }` | Raw body + headers |
| `confirm_screenshot_with_data` | `{ pngBase64, label, width, height }` | Raw body + headers（元数据走 headers） |
| `ocr_screenshot` | `{ pngBase64, label }` | Raw body + headers |

前端：`canvas.toDataURL("image/png")` → `canvas.toBlob()` → `arrayBuffer()` → `new Uint8Array()` → `invoke(cmd, uint8array, { headers: { ... } })`

### 层 3：Rust→前端改用二进制返回

| 命令 | 当前 | 改后 |
|------|------|------|
| `get_image_full` | `data:image/webp;base64,...` | `ipc::Response::new(webp_bytes)` |
| `get_screenshot_image` | `{ image: b64 }` | `ipc::Response::new(jpeg_bytes)` + headers 传 width/height |
| `scroll://done` | `{ id, png_base64 }` | `{ id }`（移除 base64） |

保留 base64：`get_image_thumb`（<50KB）、`scroll://frame`（前端需要 data: URL）。

---

## 二、API 兼容性

这是内部 IPC 改造，前端和后端同步修改，无外部 API 变化。
