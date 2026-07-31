/**
 * 单个终端面板——挂载 xterm 实例 + PTY session + 搜索浮层 + 右键菜单。
 *
 * 职责：
 * - 调用 useTerminalSession 创建 xterm + PTY
 * - 上报 ptyId 给父组件（用于 agent 状态映射）
 * - 消费 pendingCommand（ActionBar 联动：shell 就绪后写入命令 + 回车）
 * - 终端内搜索（Cmd+F 触发，SearchOverlay 浮层）
 * - 终端右键菜单（复制/粘贴/全选/清屏）
 */

import { useEffect, useRef, useState, useCallback } from "react";
import { useTerminalSession } from "./useTerminalSession";
import { SearchOverlay } from "./SearchOverlay";
import { ContextMenu, type MenuPosition, type MenuItem } from "./ContextMenu";
import { useT } from "@/lib/i18n";

type Props = {
  cwd?: string;
  /** tab 是否活跃——活跃 attach WebGL，隐藏 dispose 释放 context。 */
  active?: boolean;
  /** 待写入的初始命令（mount 后消费一次）。 */
  pendingCommand?: string;
  /** 命令消费后回调（父组件清空 pendingCommand，避免重复写）。 */
  onConsumeCommand?: () => void;
  /** PTY 连接成功后上报 ptyId（父组件用于 agent 状态映射）。 */
  onPtyId?: (ptyId: number) => void;
  /** Cmd/Ctrl+T 新建 tab 回调。 */
  onNewTab?: () => void;
  /** OSC 7 cwd 变化时上报（父组件更新 tab.trackedCwd）。 */
  onCwd?: (cwd: string) => void;
};

export function TerminalPane({
  cwd,
  active = true,
  pendingCommand,
  onConsumeCommand,
  onPtyId,
  onNewTab,
  onCwd,
}: Props) {
  const containerRef = useRef<HTMLDivElement>(null);
  const [searchOpen, setSearchOpen] = useState(false);
  const [menuPos, setMenuPos] = useState<MenuPosition>(null);
  const t = useT();

  const openContextMenu = useCallback((e: React.MouseEvent) => {
    e.preventDefault();
    setMenuPos({ x: e.clientX, y: e.clientY });
  }, []);

  const session = useTerminalSession({
    container: containerRef,
    cwd,
    active,
    onSearchOpen: () => setSearchOpen(true),
    onNewTab,
  });

  const terminalMenuItems: MenuItem[] = [
    {
      label: t("terminal.ctxCopy"),
      action: () => {
        const sel = session.getSelection();
        if (!sel) return;
        // WKWebView 的 navigator.clipboard 可能受限——用 textarea + execCommand 兜底
        const ta = document.createElement("textarea");
        ta.value = sel;
        ta.style.position = "fixed";
        ta.style.opacity = "0";
        document.body.appendChild(ta);
        ta.select();
        document.execCommand("copy");
        document.body.removeChild(ta);
      },
      disabled: !session.hasSelection(),
    },
    {
      label: t("terminal.ctxPaste"),
      action: () => {
        // WKWebView navigator.clipboard.readText 受限——用 execCommand('paste')
        // 需要 xterm textarea 聚焦，execCommand('paste') 才生效
        session.focus();
        document.execCommand("paste");
      },
    },
    {
      label: t("terminal.ctxSelectAll"),
      action: () => session.selectAll(),
    },
    {
      label: t("terminal.ctxClear"),
      action: () => session.clear(),
    },
  ];

  // 上报 ptyId（session 連接成功后变化）
  useEffect(() => {
    if (session.ptyId != null) {
      onPtyId?.(session.ptyId);
    }
  }, [session.ptyId, onPtyId]);

  // 上报 cwd（OSC 7 追踪，cd 后变化）
  useEffect(() => {
    if (session.cwd) {
      onCwd?.(session.cwd);
    }
  }, [session.cwd, onCwd]);

  // 消费 pendingCommand：ptyId 就绪（shell 可接收输入）后写入 + 回车
  useEffect(() => {
    if (session.ptyId != null && pendingCommand) {
      session.write(`${pendingCommand}\n`);
      onConsumeCommand?.();
    }
  }, [session.ptyId, pendingCommand, session, onConsumeCommand]);

  return (
    <div className="terminal-pane">
      <div ref={containerRef} className="terminal-pane-canvas" onContextMenu={openContextMenu} />
      {searchOpen && active && session.searchAddon && (
        <SearchOverlay
          addon={session.searchAddon}
          onClose={() => setSearchOpen(false)}
          onFocusTerminal={() => session.focus()}
        />
      )}
      <ContextMenu position={menuPos} items={terminalMenuItems} onClose={() => setMenuPos(null)} />
    </div>
  );
}
