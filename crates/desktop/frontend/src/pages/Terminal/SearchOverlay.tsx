/**
 * 终端搜索浮层——浮在终端区内右上角，Cmd+F 触发。
 *
 * 增量搜索（输入实时高亮）+ 上一个/下一个导航（↑↓ + Enter/Shift+Enter）。
 * Esc 关闭 + clearDecorations + 焦点回终端。
 *
 * 参考 Terax SearchInline（terminal 部分），简化为单一终端搜索。
 */

import { useEffect, useRef, useState } from "react";
import { ChevronUp, ChevronDown, X } from "lucide-react";
import type { SearchAddon } from "@xterm/addon-search";
import { useT } from "@/lib/i18n";

/** 匹配高亮配色（对齐 Terax）。 */
const TERM_DECORATIONS = {
  matchBackground: "#515c6a",
  activeMatchBackground: "#d18616",
  matchOverviewRuler: "#d18616",
  activeMatchColorOverviewRuler: "#d18616",
};

type Props = {
  addon: SearchAddon;
  /** 关闭搜索回调（TerminalPane 设 searchOpen=false）。 */
  onClose: () => void;
  /** 关闭后焦点回终端。 */
  onFocusTerminal: () => void;
};

export function SearchOverlay({ addon, onClose, onFocusTerminal }: Props) {
  const t = useT();
  const [query, setQuery] = useState("");
  const inputRef = useRef<HTMLInputElement>(null);

  // mount 时自动聚焦输入框
  useEffect(() => {
    inputRef.current?.focus();
  }, []);

  // 增量搜索
  const search = (q: string) => {
    setQuery(q);
    if (q) {
      addon.findNext(q, { incremental: true, decorations: TERM_DECORATIONS });
    } else {
      addon.clearDecorations();
    }
  };

  const findNext = () => {
    if (query) addon.findNext(query, { decorations: TERM_DECORATIONS });
  };

  const findPrevious = () => {
    if (query) addon.findPrevious(query, { decorations: TERM_DECORATIONS });
  };

  const close = () => {
    addon.clearDecorations();
    setQuery("");
    onClose();
    onFocusTerminal();
  };

  return (
    <div className="terminal-search-overlay">
      <input
        ref={inputRef}
        className="terminal-search-input"
        value={query}
        placeholder={t("terminal.searchPlaceholder")}
        onChange={(e) => search(e.target.value)}
        onKeyDown={(e) => {
          if (e.key === "Enter") {
            e.preventDefault();
            if (e.shiftKey) findPrevious();
            else findNext();
          } else if (e.key === "Escape") {
            e.preventDefault();
            close();
          }
        }}
      />
      <button
        className="terminal-search-btn"
        onClick={findPrevious}
        title={t("terminal.searchPrev")}
        aria-label={t("terminal.searchPrev")}
        disabled={!query}
      >
        <ChevronUp size={14} />
      </button>
      <button
        className="terminal-search-btn"
        onClick={findNext}
        title={t("terminal.searchNext")}
        aria-label={t("terminal.searchNext")}
        disabled={!query}
      >
        <ChevronDown size={14} />
      </button>
      <button
        className="terminal-search-btn terminal-search-close"
        onClick={close}
        title={t("terminal.searchClose")}
        aria-label={t("terminal.searchClose")}
      >
        <X size={14} />
      </button>
    </div>
  );
}
