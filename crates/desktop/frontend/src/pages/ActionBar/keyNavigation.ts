/** ActionBar 键盘导航纯逻辑 —— 无 React/DOM 依赖，可独立单测。
 *
 * 设计（详见 spec 2026-07-30-actionbar-keydown-extract-design.md）：
 * - decideKeyAction(e, ctx) 给定键盘事件 + refs 快照 → 返回 KeyAction
 * - hook 从 refs 组装 ctx，根据 action 执行副作用
 *
 * 判断顺序严格复刻 index.tsx 重构前 keydown handler（652-863 行）。
 */

import type { ActionBarItem } from "./types";
import type { TabId, View } from "./searchTypes";

// ═══════════════════ 类型 ═══════════════════

/** 纯函数输入——从 refs 同步读取的上下文快照。无 React 依赖。 */
export interface KeyContext {
  /** "search" = hasQuery(query)；"menu" = query 空 */
  mode: "search" | "menu";
  view: View;
  focusLayer: "main" | "sub";
  query: string;
  /** 主菜单项（已过滤 isVisible + isEnabled） */
  mainItems: ActionBarItem[];
  /** 子菜单项（已过滤） */
  subItems: ActionBarItem[];
  /** 全量菜单项（Alt+字母匹配 + submenu 子项展开计算用） */
  menuItems: ActionBarItem[];
  selectedIdx: number;
  subSelectedIdx: number;
  searchSelectedIdx: number;
  activeTab: TabId;
  hasContext: boolean;
  searchEngine: string;
  /** IME 最后按键时刻（Date.now()）；Enter 500ms 内 = 选词确认窗口 */
  lastImeKeyTime: number;
  /** 搜索结果列表长度（↑↓/Enter 用） */
  searchResultsCount: number;
}

/** 纯函数输出——判别联合，枚举所有键盘动作。
 *
 * preventDefault 规则（hook 执行时统一应用）：
 *   - passthrough / ignore / ime-composing / ime-confirm-enter → 不 preventDefault（放行）
 *   - 其余所有 action → preventDefault */
export type KeyAction =
  | { type: "ime-composing" }
  | { type: "ime-confirm-enter" }
  | { type: "passthrough" }
  | { type: "ignore" }
  | { type: "escape-clear-query" }
  | { type: "escape-dismiss" }
  | { type: "search-tab"; dir: 1 | -1 }
  | { type: "search-nav"; dir: 1 | -1 }
  | { type: "search-enter" }
  | { type: "menu-move"; forward: boolean }
  | { type: "menu-toggle-layer" }
  | { type: "menu-enter" }
  | { type: "open-submenu"; parentId: number; subIdx: number }
  | { type: "close-submenu" }
  | { type: "alt-execute"; item: ActionBarItem }
  | { type: "alt-goto-sub"; idx: number }
  | { type: "alt-goto-main"; idx: number; expandSubmenu: boolean; parentId?: number; subIdx?: number };

// ═══════════════════ 子菜单预选 ═══════════════════

/**
 * 给定 submenu 子项列表 + 当前搜索引擎，算初始 subSelectedIdx。
 * 规则（复刻 index.tsx 行 762-767 / 803-808 / 836-841）：首项是 url 类型时，
 * 按 title(小写) 匹配 searchEngine；匹配不到或首项非 url → 0。
 */
export function pickSubIdx(
  subs: ActionBarItem[],
  searchEngine: string,
): number {
  if (subs.length > 0 && subs[0].actionType === "url") {
    const idx = subs.findIndex(
      (s) => s.title.toLowerCase() === searchEngine,
    );
    return idx >= 0 ? idx : 0;
  }
  return 0;
}
