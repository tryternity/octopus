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
import { listen } from "@tauri-apps/api/event";
import { getCurrentWebviewWindow } from "@tauri-apps/api/webviewWindow";
import { useTerminalSession } from "./useTerminalSession";
import { SearchOverlay } from "./SearchOverlay";
import { ContextMenu, type MenuPosition, type MenuItem } from "./ContextMenu";
import { relPath } from "./relPath";
import { shellEscape } from "./shellEscape";
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

  // ASR 文本回写（spec 2026-07-31-asr-paste-self-webview）：后端检测前台是 terminal
  // webview 时 emit "paste-text" 定向到本窗口。仅活跃 pane 响应——直写 PTY（最可靠，
  // 绕过 xterm/键盘模拟）。限定 target 为当前窗口 label，避免多终端窗口都写。
  //
  // session 对象每次渲染都是新引用（useTerminalSession 返回字面量），不能放 deps——
  // 否则每渲染都 unlisten/listen，间隙丢事件。用 ref 持有最新 session，effect 只挂一次。
  const sessionRef = useRef(session);
  sessionRef.current = session;
  useEffect(() => {
    if (!active) return;
    let unlisten: (() => void) | null = null;
    const currentLabel = getCurrentWebviewWindow().label;
    listen<string>(
      "paste-text",
      (e) => {
        const s = sessionRef.current;
        if (s.ptyId != null) {
          s.write(e.payload);
        }
      },
      { target: { kind: "WebviewWindow", label: currentLabel } },
    )
      .then((fn) => {
        unlisten = fn;
      })
      .catch((err) => console.error("[TerminalPane] paste-text listener failed:", err));
    return () => {
      unlisten?.();
    };
  }, [active]);

  return (
    <div className="terminal-pane">
      <div
        ref={containerRef}
        className="terminal-pane-canvas"
        onContextMenu={openContextMenu}
        // capture 阶段监听：xterm 在 canvas 内部元素上 attach 的 listener 可能拦截
        // bubble 阶段的 drop（实测 WKWebView 下 onDrop 不触发）。capture 从根向下传播，
        // 在 target 阶段之前触发，确保先于 xterm 拿到 drop 事件。
        onDragOverCapture={(e) => {
          // 允许 drop（否则浏览器/WKWebView 拒绝）
          if (e.dataTransfer.types.includes("text/plain")) {
            e.preventDefault();
            e.dataTransfer.dropEffect = "copy";
          }
        }}
        onDropCapture={(e) => {
          const fullPath = e.dataTransfer.getData("text/plain");
          if (!fullPath) return;
          e.preventDefault();
          e.stopPropagation();
          const s = sessionRef.current;
          const rel = relPath(fullPath, s.cwd ?? "");
          const escaped = shellEscape(rel);
          s.write(escaped); // 插入光标位置，不回车
          s.focus(); // 自动聚焦终端
        }}
      />
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
