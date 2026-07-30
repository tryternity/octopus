/** ActionBar 键盘导航副作用 hook —— 绑定 keydown，从 refs 组装 ctx，
 * 调 decideKeyAction，switch(action) 执行 setState/invoke。
 *
 * 纯逻辑在 keyNavigation.ts（可单测）；本文件只做副作用编排。
 * 生命周期：mount 绑定、unmount 解绑（空依赖 []，与原 effect 一致）。
 */

import { useEffect, type RefObject, type MutableRefObject } from "react";
import { invoke } from "@tauri-apps/api/core";
import { decideKeyAction, pickSubIdx, type KeyContext, type KeyAction } from "./keyNavigation";
import { navigateResults, getNextTab, hasQuery } from "./searchLogic";
import type { ActionBarItem, Context } from "./types";
import type { TabId, View, SearchResult } from "./searchTypes";

// ARROW_AS_TAB 常量由 decideKeyAction 内部持有——hook 只需 ctx 快照 + action 分发，
// 不再自己判定导航键，故本文件不需要该常量。

export interface ActionBarKeydownParams {
  // refs（组装 ctx 用）
  queryRef: MutableRefObject<string>;
  viewRef: MutableRefObject<View>;
  focusLayerRef: MutableRefObject<"main" | "sub">;
  contextRef: MutableRefObject<Context | null>;
  selectedIdxRef: MutableRefObject<number>;
  subSelectedIdxRef: MutableRefObject<number>;
  searchSelectedIdxRef: MutableRefObject<number>;
  activeTabRef: MutableRefObject<TabId>;
  mainItemsRef: MutableRefObject<ActionBarItem[]>;
  subItemsRef: MutableRefObject<ActionBarItem[]>;
  menuItemsRef: MutableRefObject<ActionBarItem[]>;
  searchEngineRef: MutableRefObject<string>;
  filteredResultsRef: MutableRefObject<SearchResult[]>;
  inputRef: RefObject<HTMLInputElement | null>;
  lastImeKeyTime: MutableRefObject<number>;
  submenuParentIdRef: MutableRefObject<number | null>;
  // setters
  setQuery: (v: string) => void;
  setActiveTab: (v: TabId) => void;
  setSearchSelectedIdx: (v: number | ((p: number) => number)) => void;
  setSelectedIdx: (v: number | ((p: number) => number)) => void;
  setSubSelectedIdx: (v: number | ((p: number) => number)) => void;
  setView: (v: View) => void;
  setFocusLayer: (v: "main" | "sub") => void;
  // 命令回调
  executeItem: (item: ActionBarItem) => void;
  executeSearchResult: (r: SearchResult) => void;
}

export function useActionBarKeydown(p: ActionBarKeydownParams): void {
  useEffect(() => {
    const handler = (e: KeyboardEvent) => {
      // 组装 ctx 快照
      const ctx: KeyContext = {
        mode: hasQuery(p.queryRef.current) ? "search" : "menu",
        view: p.viewRef.current,
        focusLayer: p.focusLayerRef.current,
        query: p.queryRef.current,
        mainItems: p.mainItemsRef.current,
        subItems: p.subItemsRef.current,
        menuItems: p.menuItemsRef.current,
        selectedIdx: p.selectedIdxRef.current,
        subSelectedIdx: p.subSelectedIdxRef.current,
        searchSelectedIdx: p.searchSelectedIdxRef.current,
        activeTab: p.activeTabRef.current,
        hasContext: !!p.contextRef.current,
        searchEngine: p.searchEngineRef.current,
        lastImeKeyTime: p.lastImeKeyTime.current,
        searchResultsCount: p.filteredResultsRef.current.length,
      };

      const action = decideKeyAction(e, ctx);

      // preventDefault 规则：放行类不 preventDefault，其余都 preventDefault。
      // swallow 刻意排除——它需要 preventDefault 但不执行其他副作用。
      const passthroughTypes: KeyAction["type"][] = ["passthrough", "ignore", "ime-composing", "ime-confirm-enter"];
      if (!passthroughTypes.includes(action.type)) {
        e.preventDefault();
      }

      // IME 时间记录（ime-composing 写 / ime-confirm-enter 清）
      if (action.type === "ime-composing") {
        p.lastImeKeyTime.current = Date.now();
        return;
      }
      if (action.type === "ime-confirm-enter") {
        p.lastImeKeyTime.current = 0;
        return;
      }

      switch (action.type) {
        case "passthrough":
        case "ignore":
        case "swallow":
          return;

        case "escape-clear-query":
          p.setQuery("");
          p.inputRef.current?.focus();
          return;

        case "escape-dismiss":
          invoke("action_bar_dismiss", { reason: "escape" });
          return;

        case "search-tab":
          p.setActiveTab(getNextTab(p.activeTabRef.current, action.dir, !!p.contextRef.current));
          return;

        case "search-nav":
          p.setSearchSelectedIdx(
            navigateResults(p.searchSelectedIdxRef.current, action.dir, p.filteredResultsRef.current.length),
          );
          return;

        case "search-enter": {
          const results = p.filteredResultsRef.current;
          const selected = results[p.searchSelectedIdxRef.current] ?? results[0];
          if (selected) p.executeSearchResult(selected);
          return;
        }

        case "menu-move": {
          // 方案 A：展开判断在 setSelectedIdx/setSubSelectedIdx 回调内（复刻原 786-816）
          if (p.focusLayerRef.current === "sub") {
            p.setSubSelectedIdx((prev) => {
              const items = p.subItemsRef.current;
              if (items.length === 0) return 0;
              return action.forward ? (prev + 1) % items.length : (prev - 1 + items.length) % items.length;
            });
          } else {
            p.setSelectedIdx((prev) => {
              const items = p.mainItemsRef.current;
              if (items.length === 0) return 0;
              const next = action.forward ? (prev + 1) % items.length : (prev - 1 + items.length) % items.length;
              const it = items[next];
              if (it && it.actionType === "submenu") {
                p.submenuParentIdRef.current = it.id;
                const subs = p.menuItemsRef.current.filter((i) => i.isEnabled && i.parentId === it.id);
                p.setSubSelectedIdx(pickSubIdx(subs, p.searchEngineRef.current));
                p.setView("submenu");
              } else {
                p.submenuParentIdRef.current = null;
                p.setView("main");
              }
              return next;
            });
          }
          return;
        }

        case "menu-toggle-layer": {
          // ↑↓ 切层（复刻原 820-846）。preventDefault 已在上面统一处理。
          if (p.focusLayerRef.current === "sub") {
            p.setFocusLayer("main");
          } else {
            const cur = p.mainItemsRef.current[p.selectedIdxRef.current];
            if (cur && cur.actionType === "submenu") {
              p.setFocusLayer("sub");
              if (p.viewRef.current !== "submenu") {
                p.submenuParentIdRef.current = cur.id;
                const subs = p.menuItemsRef.current.filter((i) => i.isEnabled && i.parentId === cur.id);
                p.setSubSelectedIdx(pickSubIdx(subs, p.searchEngineRef.current));
                p.setView("submenu");
              }
            }
            // 当前项非 submenu → preventDefault 了但不切层（§PRESERVE）
          }
          return;
        }

        case "menu-enter": {
          if (p.focusLayerRef.current === "sub") {
            const it = p.subItemsRef.current[p.subSelectedIdxRef.current];
            if (it) p.executeItem(it);
          } else {
            const it = p.mainItemsRef.current[p.selectedIdxRef.current];
            if (it) p.executeItem(it);
          }
          return;
        }

        case "alt-execute":
          p.executeItem(action.item);
          return;

        case "alt-goto-sub":
          if (action.idx < p.subItemsRef.current.length) {
            p.setSubSelectedIdx(action.idx);
          }
          return;

        case "alt-goto-main":
          if (action.idx < p.mainItemsRef.current.length) {
            p.setSelectedIdx(action.idx);
            if (action.expandSubmenu && action.parentId !== undefined) {
              p.submenuParentIdRef.current = action.parentId;
              // subIdx 已由 decideKeyAction 经 pickSubIdx 算好（keyNavigation.ts），hook 不重算
              p.setSubSelectedIdx(action.subIdx ?? 0);
              p.setView("submenu");
            } else {
              p.submenuParentIdRef.current = null;
              p.setView("main");
            }
          }
          return;

        default: {
          // 穷尽性保护：若新增 KeyAction 成员且未在 switch 前拦截、又忘加 case，
          // action 此处类型非 never → tsc 报错，避免按键静默 no-op。
          const _exhaustive: never = action;
          return _exhaustive;
        }
      }
    };
    window.addEventListener("keydown", handler);
    return () => window.removeEventListener("keydown", handler);
  }, []); // 空依赖——与原 effect 一致，handler 闭包通过 refs 读最新值
}
