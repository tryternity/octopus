/** ActionBar 搜索纯逻辑函数 —— 无 React/DOM 依赖，可独立单测。 */

import {
  TABS,
  DELAYED_SEARCH_MIN_LENGTH,
  EXPAND_THRESHOLD_PX,
  MAX_VISIBLE_RESULTS,
  RESULT_ROW_HEIGHT,
  TAB_BAR_HEIGHT,
  INPUT_HEIGHT,
  MENU_HEIGHT_MAIN,
  MENU_HEIGHT_SUBMENU,
  MENU_HEIGHT_LOADING,
  type TabId,
  type TabDef,
  type ExpandDirection,
  type View,
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

/** 返回当前可见的 Tab 列表。无选中（launch 模式）时隐藏"动作"Tab——菜单项需要选中内容。 */
export function getVisibleTabs(hasContext: boolean): readonly TabDef[] {
  return hasContext ? TABS : TABS.filter((t) => t.id !== "actions");
}

/**
 * 获取下一个/上一个 Tab（循环）。
 * @param direction 1 = 下一个，-1 = 上一个
 */
export function getNextTab(current: TabId, direction: 1 | -1, hasContext = true): TabId {
  const tabs = getVisibleTabs(hasContext);
  const idx = tabs.findIndex((t) => t.id === current);
  if (idx === -1) return "all";
  const nextIdx = (idx + direction + tabs.length) % tabs.length;
  return tabs[nextIdx].id;
}

/** Tab 在 TABS 数组中的索引。无效返回 -1。 */
export function getTabIndex(tab: TabId): number {
  return TABS.findIndex((t) => t.id === tab);
}

/** 判定是否应触发延迟搜索（query trim 后 ≥ 2 字符）。 */
export function shouldTriggerDelayedSearch(query: string): boolean {
  return query.trim().length >= DELAYED_SEARCH_MIN_LENGTH;
}

/**
 * 按 Tab 过滤搜索结果。
 * - "all" → 全部
 * - "apps" → source === "app"
 * - "files" → source === "file"
 * - "bookmarks" → source === "bookmark"
 */
export function filterByTab(results: SearchResult[], tab: TabId): SearchResult[] {
  if (tab === "all") return results;
  const sourceMap: Record<string, string> = {
    apps: "app",
    files: "file",
    bookmarks: "bookmark",
    actions: "menu",
    commands: "command",
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
 * 0 结果时仍保留 1 行高度（显示"无结果"提示），最多 MAX_VISIBLE_RESULTS 行。
 */
export function calcResultsHeight(resultCount: number): number {
  const visible = Math.min(Math.max(resultCount, 1), MAX_VISIBLE_RESULTS);
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

/**
 * executeItem 展开 submenu 后，焦点层应切换到 "sub"。
 *
 * 设计意图（架构文档「预览不抢焦点」契约）：
 * - executeItem（点击 / Alt+字母 / Enter on main）是终结性动作——用户明确要打开子菜单，
 *   展开后按 Enter 应执行子项，故 focusLayer 应进 "sub"。
 * - Tab / Alt+数字 的「预览展开」不抢焦点（用户可能继续在 main 层移动）。
 *
 * 本函数只处理 executeItem 路径：submenu → "sub"，其他 actionType 保持当前层。
 */
export function nextFocusLayerAfterExecute(
  actionType: string,
  currentLayer: "main" | "sub",
): "main" | "sub" {
  return actionType === "submenu" ? "sub" : currentLayer;
}

/**
 * 判断按键是否是"移动键"——Tab 或（ARROW_AS_TAB 时）←/→。
 * 搜索模式用于切 Tab 页，菜单模式用于移菜单项，逻辑共享。
 */
export function isMoveKey(key: string, arrowAsTab: boolean): boolean {
  if (key === "Tab") return true;
  return arrowAsTab && (key === "ArrowLeft" || key === "ArrowRight");
}

/**
 * 移动方向：Tab 看 shiftKey，→ 正向，← 反向。非移动键返回 null。
 * forward=true → 正向（下一个），forward=false → 反向（上一个）。
 */
export function moveDirection(
  key: string,
  shiftKey: boolean,
  arrowAsTab: boolean,
): boolean | null {
  if (key === "Tab") return !shiftKey;
  if (arrowAsTab && key === "ArrowRight") return true;
  if (arrowAsTab && key === "ArrowLeft") return false;
  return null;
}

/**
 * 菜单条高度（不含输入框）—— resize effect 据此算窗口总高。
 *
 * 关键不变量（防护「首次有选中触发菜单条被窗口裁剪」回归）：
 * - 无选中（hasContext=false）→ 0（仅搜索框，不显示菜单条）
 * - 有选中 + view=main → MENU_HEIGHT_MAIN
 * - 有选中 + view=submenu → MENU_HEIGHT_SUBMENU
 * - 有选中 + view=loading → MENU_HEIGHT_LOADING
 *
 * 此函数从 index.tsx 的 resize useEffect 内联逻辑抽取——原内联表达式的
 * `!context ? 0 : ...` 依赖 context state，但 resize effect 曾遗漏 context 依赖
 * 导致窗口高度不随 context 更新（菜单条 DOM 渲染了但被窗口边界裁剪）。
 * 抽纯函数 + 单测锁定高度规则，resize effect 只需 `calcMenuHeight(!!context, view)`。
 */
export function calcMenuHeight(hasContext: boolean, view: View): number {
  if (!hasContext) return 0;
  if (view === "submenu") return MENU_HEIGHT_SUBMENU;
  if (view === "loading") return MENU_HEIGHT_LOADING;
  return MENU_HEIGHT_MAIN;
}

/**
 * 窗口总高度 = 输入框 + 菜单/搜索结果区。
 * - 搜索模式（inSearch=true）：输入框 + Tab 栏 + 结果区
 * - 菜单模式（inSearch=false）：输入框 + 菜单条（calcMenuHeight）
 */
export function calcTotalHeight(
  inSearch: boolean,
  hasContext: boolean,
  view: View,
  resultsCount: number,
): number {
  if (inSearch) {
    return INPUT_HEIGHT + TAB_BAR_HEIGHT + calcResultsHeight(resultsCount);
  }
  return INPUT_HEIGHT + calcMenuHeight(hasContext, view);
}
