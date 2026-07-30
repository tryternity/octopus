/**
 * 终端窗口主组件（Task 7）。
 *
 * 架构（简化版，相对 Terax TerminalStack + TerminalPane + rendererPool）：
 * - 多 tab：tabs 数组，每 tab 一个 TerminalPane（独立 xterm 实例 + PTY session）
 * - tab 切换用 visibility:hidden 保活（不卸载 xterm，切回时 scrollback 保留）
 * - agent 状态徽章：每 tab 标题旁彩色圆点（working 脉冲 / attention bell / finished）
 * - ActionBar 联动：listen "terminal://new-tab" { cwd, command } → 新 tab + 写命令
 *
 * 视觉设计（frontend-design skill）：
 * - 终端画布固定深色 #0c0c0f（终端惯例 + signature 元素，不随主题变）
 * - tab 栏用主题 token（--color-background/border/muted-foreground），浅色/深色自适应
 * - agent 徽章是唯一「活泼」元素：working amber 脉冲，attention 红色，finished 淡出
 */

import { useEffect, useState, useCallback } from "react";
import { Plus, X, Bell } from "lucide-react";
import { listen } from "@tauri-apps/api/event";
import { useT } from "@/lib/i18n";
import { TerminalPane } from "./TerminalPane";
import {
  ensureAgentActivityListener,
  subscribeAgentActivity,
  getAgentActivity,
  type AgentPhase,
} from "./agent-activity";

type Tab = {
  id: number;
  /** shell 启动目录（openPty 用）。 */
  cwd?: string;
  /** 待写入的初始命令（ActionBar 联动；TerminalPane mount 后消费并清空）。 */
  pendingCommand?: string;
  /** TerminalPane 上报的 ptyId（用于 agent 状态映射）。null = 还在连接。 */
  ptyId: number | null;
};

let nextTabId = 1;
function makeTab(cwd?: string, pendingCommand?: string): Tab {
  nextTabId += 1;
  return { id: nextTabId, cwd, pendingCommand, ptyId: null };
}

export default function Terminal() {
  const t = useT();
  const [tabs, setTabs] = useState<Tab[]>(() => [makeTab()]);
  const [activeId, setActiveId] = useState(() => tabs[0]?.id ?? 1);
  const [, forceUpdate] = useState(0);

  // agent 状态变化时强制重渲染（subscribe 模式替代 zustand）
  useEffect(() => subscribeAgentActivity(() => forceUpdate((n) => n + 1)), []);

  // 绑定 agent 信号 listener（幂等）
  useEffect(() => {
    ensureAgentActivityListener();
  }, []);

  const addTab = useCallback((cwd?: string, command?: string) => {
    const tab = makeTab(cwd, command);
    setTabs((prev) => [...prev, tab]);
    setActiveId(tab.id);
  }, []);

  const closeTab = useCallback(
    (id: number) => {
      setTabs((prev) => {
        const next = prev.filter((tb) => tb.id !== id);
        if (next.length === 0) {
          // 最后一个 tab 关了——新建空 tab（窗口不关，保持「终端」始终可用）
          const fresh = makeTab();
          setActiveId(fresh.id);
          return [fresh];
        }
        // 关的是活跃 tab → 切到最后一个
        if (id === activeId) {
          setActiveId(next[next.length - 1].id);
        }
        return next;
      });
    },
    [activeId],
  );

  // ActionBar 联动：listen "terminal://new-tab" { cwd, command }
  useEffect(() => {
    let unlisten: (() => void) | null = null;
    listen<{ cwd?: string; command?: string }>("terminal://new-tab", (e) => {
      addTab(e.payload.cwd, e.payload.command);
    })
      .then((fn) => {
        unlisten = fn;
      })
      .catch(() => {});
    return () => {
      unlisten?.();
    };
  }, [addTab]);

  // 读 URL query 的 cwd（窗口首次打开时 Rust 注入）→ 设为首个 tab 的 cwd
  useEffect(() => {
    const params = new URLSearchParams(window.location.search);
    const cwd = params.get("cwd");
    if (cwd) {
      setTabs((prev) =>
        prev.map((tb, i) => (i === 0 ? { ...tb, cwd: cwd ?? undefined } : tb)),
      );
    }
  }, []);

  const { phases, agents } = getAgentActivity();

  return (
    <div className="terminal-window">
      {/* ── Tab 栏 ── */}
      <div className="terminal-tabbar" role="tablist">
        {tabs.map((tab) => {
          const phase = tab.ptyId != null ? (phases[tab.ptyId] ?? null) : null;
          const agentName = tab.ptyId != null ? (agents[tab.ptyId] ?? null) : null;
          return (
            <TabButton
              key={tab.id}
              active={tab.id === activeId}
              phase={phase}
              label={agentName ?? t("terminal.title")}
              onCloseLabel={t("terminal.closeTab")}
              onClick={() => setActiveId(tab.id)}
              onClose={() => closeTab(tab.id)}
            />
          );
        })}
        <button
          className="terminal-tab-new"
          onClick={() => addTab()}
          title={t("terminal.newTab")}
          aria-label={t("terminal.newTab")}
        >
          <Plus size={14} />
        </button>
      </div>

      {/* ── 终端面板区（多 tab 叠放，visibility 切换保活）── */}
      <div className="terminal-panes">
        {tabs.map((tab) => (
          <div
            key={tab.id}
            className="terminal-pane-wrapper"
            style={{
              visibility: tab.id === activeId ? "visible" : "hidden",
            }}
            aria-hidden={tab.id !== activeId}
          >
            <TerminalPane
              cwd={tab.cwd}
              pendingCommand={tab.pendingCommand}
              onConsumeCommand={() => {
                setTabs((prev) =>
                  prev.map((tb) =>
                    tb.id === tab.id
                      ? { ...tb, pendingCommand: undefined }
                      : tb,
                  ),
                );
              }}
              onPtyId={(ptyId) => {
                setTabs((prev) =>
                  prev.map((tb) =>
                    tb.id === tab.id ? { ...tb, ptyId } : tb,
                  ),
                );
              }}
            />
          </div>
        ))}
      </div>
    </div>
  );
}

// ── Tab 按钮（含 agent 状态徽章）──
function TabButton(props: {
  active: boolean;
  phase: AgentPhase | null;
  label: string;
  onCloseLabel: string;
  onClick: () => void;
  onClose: () => void;
}) {
  const { active, phase, label, onClick, onClose, onCloseLabel } = props;
  return (
    <div
      className={`terminal-tab ${active ? "terminal-tab-active" : ""}`}
      role="tab"
      aria-selected={active}
      onClick={onClick}
    >
      <AgentBadge phase={phase} />
      <span className="terminal-tab-label">{label}</span>
      <button
        className="terminal-tab-close"
        onClick={(e) => {
          e.stopPropagation();
          onClose();
        }}
        aria-label={onCloseLabel}
      >
        <X size={12} />
      </button>
    </div>
  );
}

// ── Agent 状态徽章（signature 元素）──
function AgentBadge({ phase }: { phase: AgentPhase | null }) {
  if (!phase || phase === "idle") return null;
  if (phase === "attention") {
    return (
      <span className="terminal-agent-badge terminal-agent-attention" title="需要关注">
        <Bell size={11} />
      </span>
    );
  }
  // working / finished
  return (
    <span
      className={`terminal-agent-badge terminal-agent-${phase}`}
      title={phase}
    />
  );
}
