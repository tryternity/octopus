import { invoke, listen } from "@/lib/tauri";

interface ResultPayload {
  requestId: string;
  text: string;
}
interface CancelPayload {
  requestId: string;
}

/**
 * 打开精简编辑器编辑一段文本，保存后回调 onResult。
 * 内部注册 result/cancel 两个监听，按 requestId 过滤；任一命中即清理解监听。
 * 取消/X 关窗 → 不调 onResult，仅清理。
 */
export async function openCompactEditor(
  initialText: string,
  onResult: (text: string) => void,
): Promise<void> {
  const requestId = crypto.randomUUID();
  let unlistenResult: (() => void) | undefined;
  let unlistenCancel: (() => void) | undefined;
  const cleanup = () => {
    unlistenResult?.();
    unlistenCancel?.();
  };
  // 先注册监听再开窗（保存需用户操作，无竞态；但顺序正确更稳）
  unlistenResult = await listen("compact-editor://result", (payload) => {
    const p = payload as ResultPayload;
    if (p.requestId !== requestId) return;
    onResult(p.text);
    cleanup();
  });
  unlistenCancel = await listen("compact-editor://cancel", (payload) => {
    const p = payload as CancelPayload;
    if (p.requestId !== requestId) return;
    cleanup();
  });
  await invoke("open_compact_editor", { initialText, requestId });
}
