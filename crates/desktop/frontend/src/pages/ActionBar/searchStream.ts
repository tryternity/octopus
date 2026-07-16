/**
 * 流式搜索封装：发起 `search_stream` 命令 + 监听 `search://batch` / `search://done` 事件。
 *
 * 防串扰设计（runId 双重校验）：
 * 1. 模块级 currentRunId + 每次 executeSearchStream 生成新 UUID；
 *    新调用立即 unlisten 旧监听器，丢弃未到的旧批次。
 * 2. 即便旧监听器回调已被调度，payload.runId !== myRunId 时仍丢弃。
 *
 * 累加约定：后端按 Provider 扇出，每个 Provider 完成即 emit 一个 batch。
 * 调用方在 onBatch 回调里自行累加/去重/排序（见 index.tsx 的 accumulateBatch）。
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
  unlistenDone = await listen("search://done", () => {
    // done 事件不带 runId（search_commands.rs 当前实现）；当前会话监听器收尾即弃
    unlistenBatch?.();
    unlistenDone?.();
    unlistenBatch = null;
    unlistenDone = null;
  });

  try {
    await invoke("search_stream", { query, tab, runId: myRunId });
  } catch (e) {
    // invoke 失败（如引擎未初始化）——清理监听器，避免泄漏；不抛，调用方依赖 onBatch 累加
    cleanupSearchStream();
    console.error("[searchStream] search_stream failed:", e);
  }
}
