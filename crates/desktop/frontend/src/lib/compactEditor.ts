import { invoke } from "@/lib/tauri";

/**
 * 打开精简编辑器并定位到某剪贴板条目的 tab。
 * 后端：窗口已存在则 emit open-tab 切到/新建该 itemId 的 tab，否则建窗（首个 tab 由前端 mount 取 PENDING_TAB）。
 * fire-and-forget：无回调——编辑保存由编辑器内 Ctrl+S → set_clipboard_item_text 自行处理。
 *
 * 文本编辑入口直接传 item.id；三处 OCR 入口先 insert_ocr_clipboard_item 拿到新 id，再传此函数打开绑定 tab。
 */
export async function openCompactEditorTab(itemId: number): Promise<void> {
  await invoke("open_compact_editor_tab", { itemId });
}
