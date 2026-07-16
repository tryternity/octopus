/**
 * 流式搜索封装：发起 `search_stream` 命令 + 监听 `search://batch` / `search://done` 事件。
 *
 * 防串扰设计（runId 双重校验）：
 * 1. 模块级 currentRunId + 每次 executeSearchStream 生成新 UUID；
 *    新调用立即 unlisten 旧监听器，丢弃未到的旧批次。
 * 2. 即便旧监听器回调已被调度，payload.runId !== myRunId 时仍丢弃。
 *
 * 替换约定：后端 emit 的是全局累积 top-10（已加权+排序+截断），每个 Provider
 * 完成即 emit 一次完整快照。调用方在 onBatch 回调里整体替换即可（见 index.tsx 的 setInstantResults）。
 */
import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import type { SearchBatch, SearchResult, TabId } from "./searchTypes";

let currentRunId: string | null = null;
let unlistenBatch: UnlistenFn | null = null;
let unlistenDone: UnlistenFn | null = null;

/** unlisten 当前所有监听器（内部用 + 组件卸载时调）。幂等。 */
export function cleanupSearchStream(): void {
  unlistenBatch?.();
  unlistenDone?.();
  unlistenBatch = null;
  unlistenDone = null;
  currentRunId = null;
}

/**
 * 发起流式搜索。
 * @param query  查询字符串（后端会 trim）
 * @param tab    当前 Tab（"all" | "apps" | "files" | "shell" | "bookmarks" | "actions"）
 *               —— 后端据此决定哪些 Provider 跑
 * @param onBatch 每个 Provider 批次到达时的回调（payload 已校验 runId 属于本次会话）
 */
export async function executeSearchStream(
  query: string,
  tab: TabId,
  onBatch: (results: SearchResult[]) => void,
): Promise<void> {
  // 取消旧监听 + 重置 runId
  cleanupSearchStream();
  currentRunId = crypto.randomUUID();
  const myRunId = currentRunId;

  unlistenBatch = await listen<SearchBatch>("search://batch", (e) => {
    // runId 不匹配 → 旧批次丢弃（防新搜索结果被旧批次污染）
    if (e.payload.runId !== myRunId) return;
    onBatch(e.payload.results);
  });
  unlistenDone = await listen<{ runId: string }>("search://done", (e) => {
    // done 事件后端 emit { runId }（见 search_commands.rs:36）；旧会话的 done 忽略，避免 tear down 新会话的 batch listener
    if (e.payload.runId !== myRunId) return;
    unlistenBatch?.();
    unlistenDone?.();
    unlistenBatch = null;
    unlistenDone = null;
  });

  try {
    await invoke("search_stream", { query, tab, runId: myRunId });
  } catch (e) {
    // invoke 失败（如引擎未初始化）——清理监听器，避免泄漏；不抛，调用方在 onBatch 里已整体替换
    cleanupSearchStream();
    console.error("[searchStream] search_stream failed:", e);
  }
}
