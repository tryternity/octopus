# 待办：图片复制 IPC 二进制优化

**优先级**: 中（影响所有图片复制场景，不限于滚动截屏）
**前提**: 滚动截屏 e2e 验证通过并同步 main 后再开始

## 问题

预览窗口点「复制」时，前端通过 Tauri IPC 传 base64 编码的 PNG（30MB+ → 40MB+ base64），后端再 base64 解码 → PNG 解码 → 写剪贴板。三层开销导致 3-4 秒延迟。

## 方案

Tauri 2 支持原生二进制传输（`ipc::Request` + Raw body），无需 base64。

### 后端改动

```rust
// crates/desktop/src/clipboard_commands.rs
#[tauri::command]
pub async fn copy_image_to_clipboard(
    request: tauri::ipc::Request,
    handle: State<'_, Arc<ClipboardHandle>>,
    app_handle: tauri::AppHandle,
) -> Result<(), String> {
    let tauri::ipc::InvokeBody::Raw(png_bytes) = request.body() else {
        return Err("expected raw binary body".into());
    };
    handle.write_image(png_bytes).map_err(|e| e.to_string())?;
    // spawn_blocking 落库（已异步）
    let handle_clone = handle.inner().clone();
    tokio::task::spawn_blocking(move || {
        octopus_clipboard::watcher::handle_clipboard_change(handle_clone.as_ref());
    });
    let _ = app_handle.emit("clipboard://changed", ());
    Ok(())
}
```

### 前端改动

```typescript
// 从 base64 改为 Uint8Array
// 旧：await invoke('copy_image_to_clipboard', { pngBase64: b64 })
// 新：
const pngBytes = Uint8Array.from(atob(b64), c => c.charCodeAt(0));
await invoke('copy_image_to_clipboard', pngBytes);
```

### 预期收益

- 省掉前端 base64 编码（~0.5s）
- 省掉 Tauri IPC JSON 反序列化 40MB 字符串（~2-3s）
- 总延迟从 3-4s 降到 < 1s（仅剩 PNG 解码 + 写剪贴板）

### 其他受影响场景

- `save_image_to_file`（同样传 base64，可一并改）
- 标注截图复制（同一路径）

### 参考资料

- [Tauri 2 官方文档：Accessing Raw Request](https://v2.tauri.app/develop/calling-rust/#accessing-raw-request)
- `ipc::Request` + `InvokeBody::Raw` 是官方推荐的二进制传输方式
