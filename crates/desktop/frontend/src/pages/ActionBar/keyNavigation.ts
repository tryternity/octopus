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
import { moveDirection } from "./searchLogic";
import { codeToChar } from "./label";

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

// ═══════════════════ 核心：键盘动作判定 ═══════════════════

/**
 * 核心：给定键盘事件 + ctx 快照，决定要执行的动作。
 *
 * 判断顺序严格复刻 index.tsx 重构前 keydown handler（652-860 行）——
 * 顺序不可调换，前置条件改变会导致后续分支行为偏移。
 *
 * 常量（与 index.tsx 一致）：ARROW_AS_TAB = true。
 */
const ARROW_AS_TAB = true;

export function decideKeyAction(
  e: KeyboardEvent,
  ctx: KeyContext,
): KeyAction {
  // 1. IME 组合中（keyCode 229 / isComposing）→ 放行
  if (e.keyCode === 229 || e.isComposing) {
    return { type: "ime-composing" };
  }
  // 2. Enter 在 IME 后 500ms 内 → 选词确认，放行
  if (e.key === "Enter" && Date.now() - ctx.lastImeKeyTime < 500) {
    return { type: "ime-confirm-enter" };
  }
  // 3-4. Escape（任何视图）
  if (e.key === "Escape") {
    return ctx.mode === "search"
      ? { type: "escape-clear-query" }
      : { type: "escape-dismiss" };
  }
  // 5. loading 视图不拦截
  if (ctx.view === "loading") {
    return { type: "ignore" };
  }
  // 6. 无修饰可打印字符 → 放行（IME 已在顶部拦截）
  if (!e.altKey && !e.metaKey && !e.ctrlKey) {
    const navKeys = ARROW_AS_TAB
      ? ["ArrowUp", "ArrowDown", "ArrowLeft", "ArrowRight", "Tab", "Enter", " "]
      : ["ArrowUp", "ArrowDown", "Tab", "Enter", " "];
    if (!navKeys.includes(e.key)) {
      return { type: "passthrough" };
    }
  }

  // 7-11. 搜索模式（query 非空）
  if (ctx.mode === "search") {
    const dir = moveDirection(e.key, e.shiftKey, ARROW_AS_TAB);
    if (dir !== null) return { type: "search-tab", dir: dir ? 1 : -1 };
    if (e.key === "ArrowDown") return { type: "search-nav", dir: 1 };
    if (e.key === "ArrowUp") return { type: "search-nav", dir: -1 };
    if (e.key === "Enter") return { type: "search-enter" };
    return { type: "passthrough" }; // 兜底
  }

  // 12-16. 菜单模式 - Alt 快捷键
  if (e.altKey) {
    const ch = codeToChar(e.code);
    if (ch) {
      // Alt+字母 → 执行局部快捷键
      if (/^[a-z]$/.test(ch)) {
        const found = ctx.menuItems.find((i) => i.isEnabled && i.shortcut === ch);
        if (found) return { type: "alt-execute", item: found };
        return { type: "passthrough" }; // 未命中，放行（复刻行 744-746：find 无果 fallthrough 到 return）
      }
      // Alt+数字 → 定位菜单项
      if (/^[1-9]$/.test(ch)) {
        const idx = parseInt(ch, 10) - 1;
        if (ctx.focusLayer === "sub") {
          return { type: "alt-goto-sub", idx };
        }
        // 焦点 main
        if (idx < ctx.mainItems.length) {
          const m = ctx.mainItems[idx];
          if (m.actionType === "submenu") {
            const subs = ctx.menuItems.filter((i) => i.isEnabled && i.parentId === m.id);
            const subIdx = pickSubIdx(subs, ctx.searchEngine);
            return { type: "alt-goto-main", idx, expandSubmenu: true, parentId: m.id, subIdx };
          }
          return { type: "alt-goto-main", idx, expandSubmenu: false };
        }
        // idx 越界 → 原代码进 if 但啥也不做，仍 preventDefault（return 在 if 外）
        return { type: "ignore" }; // 复刻行 770-772：越界时 setSelectedIdx 不调，但仍 preventDefault
      }
    }
    // Alt 但 codeToChar 无效 → 放行（§PRESERVE 行 778）
    return { type: "passthrough" };
  }

  // 17-19. 菜单模式 - Tab/←→ 移动
  const menuDir = moveDirection(e.key, e.shiftKey, ARROW_AS_TAB);
  if (menuDir !== null) {
    if (ctx.focusLayer === "sub") {
      return { type: "menu-move", forward: menuDir };
    }
    // 焦点 main：实际移位由 hook 计算（forward + prev），展开判断也在
    // hook 的 setSelectedIdx(prev => ...) 回调里复刻原 795-815（方案 A）。
    // 纯函数拿不到 prev，无法在此算 next，故统一返回 menu-move。
    return { type: "menu-move", forward: menuDir };
  }

  // 20-22. 菜单模式 - ↑↓ 切层
  if (e.key === "ArrowUp" || e.key === "ArrowDown") {
    return { type: "menu-toggle-layer" };
  }

  // 23. Enter/Space → 执行
  if (e.key === "Enter" || e.key === " ") {
    return { type: "menu-enter" };
  }

  // 24. 其他
  return { type: "passthrough" };
}
