/**
 * 单个终端面板——挂载 xterm 实例 + PTY session。
 *
 * 职责：
 * - 调用 useTerminalSession 创建 xterm + PTY
 * - 上报 ptyId 给父组件（用于 agent 状态映射）
 * - 消费 pendingCommand（ActionBar 联动：shell 就绪后写入命令 + 回车）
 *
 * 相对 Terax TerminalPane：去掉 forwardRef/blocks/搜索/cwd 回调（Phase 1 不需要）。
 */

import { useEffect, useRef } from "react";
import { useTerminalSession } from "./useTerminalSession";

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
};

export function TerminalPane({
  cwd,
  active = true,
  pendingCommand,
  onConsumeCommand,
  onPtyId,
}: Props) {
  const containerRef = useRef<HTMLDivElement>(null);
  const session = useTerminalSession({ container: containerRef, cwd, active });

  // 上报 ptyId（session 連接成功后变化）
  useEffect(() => {
    if (session.ptyId != null) {
      onPtyId?.(session.ptyId);
    }
  }, [session.ptyId, onPtyId]);

  // 消费 pendingCommand：ptyId 就绪（shell 可接收输入）后写入 + 回车
  useEffect(() => {
    if (session.ptyId != null && pendingCommand) {
      session.write(`${pendingCommand}\n`);
      onConsumeCommand?.();
    }
  }, [session.ptyId, pendingCommand, session, onConsumeCommand]);

  return <div ref={containerRef} className="terminal-pane" />;
}
