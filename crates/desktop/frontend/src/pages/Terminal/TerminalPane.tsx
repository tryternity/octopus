/**
 * 单个终端面板——挂载 xterm 实例 + PTY session + 搜索浮层。
 *
 * 职责：
 * - 调用 useTerminalSession 创建 xterm + PTY
 * - 上报 ptyId 给父组件（用于 agent 状态映射）
 * - 消费 pendingCommand（ActionBar 联动：shell 就绪后写入命令 + 回车）
 * - 终端内搜索（Cmd+F 触发，SearchOverlay 浮层）
 */

import { useEffect, useRef, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWebviewWindow } from "@tauri-apps/api/webviewWindow";
import { useTerminalSession } from "./useTerminalSession";
import { SearchOverlay } from "./SearchOverlay";

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
  const session = useTerminalSession({
    container: containerRef,
    cwd,
    active,
    onSearchOpen: () => setSearchOpen(true),
    onNewTab,
  });

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
  useEffect(() => {
    if (!active) return;
    let unlisten: (() => void) | null = null;
    const currentLabel = getCurrentWebviewWindow().label;
    listen<string>("paste-text", (e) => {
      console.log("[TerminalPane] paste-text received, ptyId=", session.ptyId, "len=", e.payload.length);
      if (session.ptyId != null) session.write(e.payload);
    }, { target: currentLabel })
      .then((fn) => { unlisten = fn; })
      .catch(() => {});
    return () => { unlisten?.(); };
  }, [active, session]);

  return (
    <div className="terminal-pane">
      <div ref={containerRef} className="terminal-pane-canvas" />
      {searchOpen && active && session.searchAddon && (
        <SearchOverlay
          addon={session.searchAddon}
          onClose={() => setSearchOpen(false)}
          onFocusTerminal={() => session.focus()}
        />
      )}
    </div>
  );
}
