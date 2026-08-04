import { invoke } from "@/lib/tauri";

/**
 * 打开统一查看器并定位到某条目的 tab。
 * source: 'clipboard'（默认）| 'transcription'
 */
export async function openCompactEditorTab(itemId: string, source?: string): Promise<void> {
  await invoke("open_compact_editor_tab", { itemId, source: source ?? "clipboard" });
}
