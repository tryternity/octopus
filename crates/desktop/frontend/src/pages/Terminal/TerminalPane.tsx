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
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { useTerminalSession } from "./useTerminalSession";
import { SearchOverlay } from "./SearchOverlay";
import { ContextMenu, type MenuPosition, type MenuItem } from "./ContextMenu";
import { relPath } from "./relPath";
import { shellEscape, formatDroppedPaths } from "./shellEscape";
import { takeDragPath } from "./dragStore";
import { pixelToCol, shouldMoveCursor, buildCursorMoveSequence } from "./clickCursor";
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

  // 上报回调用 ref 持有（避免内联回调引用变化导致 effect 循环——AGENTS.md listener 稳定化同款）
  const onPtyIdRef = useRef(onPtyId); onPtyIdRef.current = onPtyId;
  const onCwdRef = useRef(onCwd); onCwdRef.current = onCwd;
  const onConsumeCommandRef = useRef(onConsumeCommand); onConsumeCommandRef.current = onConsumeCommand;

  // 上报 ptyId（session 連接成功后变化）——只依赖 ptyId 值，不依赖回调引用
  useEffect(() => {
    if (session.ptyId != null) {
      onPtyIdRef.current?.(session.ptyId);
    }
  }, [session.ptyId]);

  // 上报 cwd（OSC 7 追踪，cd 后变化）——只依赖 cwd 值
  useEffect(() => {
    if (session.cwd) {
      onCwdRef.current?.(session.cwd);
    }
  }, [session.cwd]);

  // 消费 pendingCommand：ptyId 就绪（shell 可接收输入）后写入 + 回车
  // session 每次渲染新引用，用 sessionRef 持有最新——effect 只依赖 ptyId/pendingCommand
  useEffect(() => {
    if (session.ptyId != null && pendingCommand) {
      sessionRef.current.write(`${pendingCommand}\n`);
      onConsumeCommandRef.current?.();
    }
  }, [session.ptyId, pendingCommand]);

  // ASR 文本回写（spec 2026-07-31-asr-paste-self-webview）：后端检测前台是 terminal
  // webview 时 emit "paste-text" 定向到本窗口。仅活跃 pane 响应——直写 PTY（最可靠，
  // 绕过 xterm/键盘模拟）。限定 target 为当前窗口 label，避免多终端窗口都写。
  //
  // session 对象每次渲染都是新引用（useTerminalSession 返回字面量），不能放 deps——
  // 否则每渲染都 unlisten/listen，间隙丢事件。用 ref 持有最新 session，effect 只挂一次。
  const sessionRef = useRef(session);
  sessionRef.current = session;
  // click vs drag 区分：mousedown 记录起点，移动 <4px 算 click（触发光标移动）
  const mouseDownPos = useRef<{ x: number; y: number } | null>(null);
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

  // 文件树拖拽 + Alt+Click 光标定位（pointer events 方案）：document mouseup 时
  // hit-test 判定鼠标是否在 canvas 内。document 级监听绕开 xterm 内部元素的事件拦截。
  useEffect(() => {
    const isInCanvas = (e: MouseEvent): boolean => {
      const canvas = containerRef.current;
      if (!canvas) return false;
      const rect = canvas.getBoundingClientRect();
      return e.clientX >= rect.left && e.clientX <= rect.right &&
        e.clientY >= rect.top && e.clientY <= rect.bottom;
    };

    // mousedown 记录 Alt+Click 起点（用于 mouseup 判定 click vs drag）
    const handleMouseDown = (e: MouseEvent) => {
      if (e.button === 0 && e.altKey && isInCanvas(e)) {
        mouseDownPos.current = { x: e.clientX, y: e.clientY };
      }
    };

    const handleMouseUp = (e: MouseEvent) => {
      // 优先处理文件拖拽
      const path = takeDragPath();
      if (path !== null) {
        if (!isInCanvas(e)) return;
        const s = sessionRef.current;
        const rel = relPath(path, s.cwd ?? "");
        const escaped = shellEscape(rel);
        s.paste(escaped);
        s.focus();
        return;
      }

      // Alt+Click 光标定位（替代 xterm 内置 altClickMovesCursor，更精确）
      const down = mouseDownPos.current;
      mouseDownPos.current = null;
      if (!down || !e.altKey || e.button !== 0) return;
      // click vs drag：移动 <4px 算 click
      const moved = Math.abs(e.clientX - down.x) + Math.abs(e.clientY - down.y);
      if (moved > 4) return;
      if (!isInCanvas(e)) return;

      const s = sessionRef.current;
      // 坐标换算
      const screen = containerRef.current?.querySelector(".xterm-screen");
      if (!screen) return;
      const rect = (screen as HTMLElement).getBoundingClientRect();
      if (rect.width <= 0 || rect.height <= 0) return;
      const clickCol = pixelToCol(e.clientX, rect.left, rect.width, s.cols);
      const clickRow = Math.max(0, Math.min(s.rows - 1, Math.floor((e.clientY - rect.top) / (rect.height / s.rows))));

      // 门控
      if (!shouldMoveCursor({
        inCommand: !s.isPromptActive(),
        bufferType: s.bufferType,
        clickRow,
        cursorY: s.cursorY,
      })) return;
      if (s.hasSelection()) return; // 有选区时点击是取消选区

      const delta = clickCol - s.cursorX;
      const seq = buildCursorMoveSequence(delta);
      if (seq) s.write(seq);
      s.focus();
    };
    document.addEventListener("mousedown", handleMouseDown);
    document.addEventListener("mouseup", handleMouseUp);
    return () => {
      document.removeEventListener("mousedown", handleMouseDown);
      document.removeEventListener("mouseup", handleMouseUp);
    };
  }, []);

  // OS 文件拖入（Finder → 终端）：Tauri onDragDropEvent。照搬 Terax useTerminalFileDrop。
  // 只活跃 pane 挂（非活跃 tab 的 pane 隐藏，drop 一定落在活跃 pane）。
  // 完整 OS 原生拖拽体验（ghost 影像 + 可靠 drop），补足内部 DOM 拖拽（pointer events）的不足。
  useEffect(() => {
    if (!active) return;
    let disposed = false;
    let unlisten: (() => void) | null = null;
    getCurrentWebview()
      .onDragDropEvent((e) => {
        const p = e.payload;
        if (p.type === "drop") {
          if (!p.paths.length) return;
          const s = sessionRef.current;
          // OS 文件用绝对路径（不经 relPath——Finder 拖入的文件不一定在 cwd 子树）
          s.paste(formatDroppedPaths(p.paths));
          s.focus();
        }
      })
      .then((fn) => {
        if (disposed) fn();
        else unlisten = fn;
      })
      .catch((err) => console.error("[TerminalPane] drag-drop listen failed:", err));
    return () => {
      disposed = true;
      unlisten?.();
    };
  }, [active]);

  return (
    <div className="terminal-pane">
      <div
        ref={containerRef}
        className="terminal-pane-canvas"
        onContextMenu={openContextMenu}
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
