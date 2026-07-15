/** ActionBar 搜索纯逻辑函数 —— 无 React/DOM 依赖，可独立单测。 */

import {
  TABS,
  DELAYED_SEARCH_MIN_LENGTH,
  EXPAND_THRESHOLD_PX,
  MAX_VISIBLE_RESULTS,
  RESULT_ROW_HEIGHT,
  TAB_BAR_HEIGHT,
  INPUT_HEIGHT,
  type TabId,
  type ExpandDirection,
  type SearchResult,
} from "./searchTypes";

/**
 * 判定展开方向。
 * 输入框下方屏幕空间 > 阈值 → "down"，否则 → "up"。
 */
export function determineExpandDirection(
  inputY: number,
  screenHeight: number,
  threshold: number = EXPAND_THRESHOLD_PX,
): ExpandDirection {
  const spaceBelow = screenHeight - inputY;
  return spaceBelow > threshold ? "down" : "up";
}

/** 按快捷键字符获取 Tab ID。无匹配返回 null。 */
export function getTabByKey(key: string): TabId | null {
  const tab = TABS.find((t) => t.key === key);
  return tab ? tab.id : null;
}

/**
 * 获取下一个/上一个 Tab（循环）。
 * @param direction 1 = 下一个，-1 = 上一个
 */
export function getNextTab(current: TabId, direction: 1 | -1): TabId {
  const idx = TABS.findIndex((t) => t.id === current);
  if (idx === -1) return "all";
  const nextIdx = (idx + direction + TABS.length) % TABS.length;
  return TABS[nextIdx].id;
}

/** Tab 在 TABS 数组中的索引。无效返回 -1。 */
export function getTabIndex(tab: TabId): number {
  return TABS.findIndex((t) => t.id === tab);
}

/** 判定是否应触发延迟搜索（query trim 后 ≥ 2 字符）。 */
export function shouldTriggerDelayedSearch(query: string): boolean {
  return query.trim().length >= DELAYED_SEARCH_MIN_LENGTH;
}

/** 判定是否为 Shell 模式（以 > 开头，忽略前导空格）。 */
export function isShellMode(query: string): boolean {
  return query.trimStart().startsWith(">");
}

/** 从搜索查询中提取 Shell 命令（去掉 > 前缀）。 */
export function extractShellCommand(query: string): string {
  return query.trimStart().slice(1).trim();
}

/**
 * 合并即时结果与延迟结果，去重（按 source+title）。
 * 即时结果优先（同 key 时保留即时结果的 score）。
 */
export function mergeResults(
  instant: SearchResult[],
  delayed: SearchResult[],
): SearchResult[] {
  const seen = new Set<string>();
  const merged: SearchResult[] = [];
  for (const r of instant) {
    const key = `${r.source}:${r.title}`;
    if (!seen.has(key)) {
      seen.add(key);
      merged.push(r);
    }
  }
  for (const r of delayed) {
    const key = `${r.source}:${r.title}`;
    if (!seen.has(key)) {
      seen.add(key);
      merged.push(r);
    }
  }
  return merged;
}

/**
 * 按 Tab 过滤搜索结果。
 * - "all" → 全部
 * - "apps" → source === "app"
 * - "files" → source === "file"
 * - "shell" → source === "shell"
 * - "bookmarks" → source === "bookmark"
 */
export function filterByTab(results: SearchResult[], tab: TabId): SearchResult[] {
  if (tab === "all") return results;
  const sourceMap: Record<string, string> = {
    apps: "app",
    files: "file",
    shell: "shell",
    bookmarks: "bookmark",
  };
  const targetSource = sourceMap[tab];
  if (!targetSource) return results;
  return results.filter((r) => r.source === targetSource);
}

/** 安全解析 actionData JSON。解析失败返回空对象。 */
export function parseActionData(actionData: string): Record<string, unknown> {
  try {
    return JSON.parse(actionData) as Record<string, unknown>;
  } catch {
    return {};
  }
}

/**
 * 计算搜索结果区域需要的高度（px）。
 * 限制在 MAX_VISIBLE_RESULTS 行以内。
 */
export function calcResultsHeight(resultCount: number): number {
  const visible = Math.min(Math.max(resultCount, 0), MAX_VISIBLE_RESULTS);
  return visible * RESULT_ROW_HEIGHT;
}

/**
 * 计算搜索面板总高度（输入框 + Tab 栏 + 结果列表）。
 * 用于动态调整窗口大小。
 */
export function calcPanelHeight(resultCount: number): number {
  return INPUT_HEIGHT + TAB_BAR_HEIGHT + calcResultsHeight(resultCount);
}

/**
 * 选中索引边界保护。
 * 结果列表为空时返回 -1；否则 clamp 到 [0, length-1]。
 */
export function clampSelectedIndex(index: number, length: number): number {
  if (length <= 0) return -1;
  return Math.min(Math.max(index, 0), length - 1);
}

/**
 * 计算上/下导航后的新索引（循环）。
 * 空列表返回 -1。
 */
export function navigateResults(
  currentIndex: number,
  direction: 1 | -1,
  length: number,
): number {
  if (length <= 0) return -1;
  return (currentIndex + direction + length) % length;
}

/** 判定查询是否有内容（trim 后非空）。 */
export function hasQuery(query: string): boolean {
  return query.trim().length > 0;
}
