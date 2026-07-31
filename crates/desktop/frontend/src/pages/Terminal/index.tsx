/**
 * 终端窗口主组件（Task 7 + 改名/布局增强）。
 *
 * 架构（简化版，相对 Terax TerminalStack + TerminalPane + rendererPool）：
 * - 多 tab：tabs 数组，每 tab 一个 TerminalPane（独立 xterm 实例 + PTY session）
 * - tab 切换用 visibility:hidden 保活（不卸载 xterm，切回时 scrollback 保留）
 * - agent 状态徽章：每 tab 标题旁彩色圆点（working 脉冲 / attention bell / finished）
 * - ActionBar 联动：listen "terminal://new-tab" { cwd, command } → 新 tab + 写命令
 * - tab 改名：双击标题内联编辑（customName > agentName > 默认标题）
 * - 布局切换：顶部 tabs ↔ 左侧 sidebar list（localStorage 持久化）
 *
 * 视觉设计（frontend-design skill）：
 * - 终端画布固定深色 #0c0c0f（终端惯例 + signature 元素，不随主题变）
 * - tab/sidebar 栏用主题 token（--color-background/border/muted-foreground），浅色/深色自适应
 * - agent 徽章是唯一「活泼」元素：working amber 脉冲，attention 红色，finished 淡出
 * - sidebar 激活项左侧 2px 强调条（比 border 更有存在感）
 */

import { useEffect, useState, useCallback } from "react";
import { Plus, X, Bell, LayoutPanelLeft, LayoutPanelTop } from "lucide-react";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWebviewWindow } from "@tauri-apps/api/webviewWindow";
import { useT } from "@/lib/i18n";
import { TerminalPane } from "./TerminalPane";
import {
  ensureAgentActivityListener,
  subscribeAgentActivity,
  getAgentActivity,
  displayLabel,
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
  /** 用户自定义名字（双击改名）；空=用默认标题。 */
  customName?: string;
};

type LayoutMode = "tabs" | "sidebar";
const LAYOUT_KEY = "octopus-terminal-layout";
function loadLayout(): LayoutMode {
  return localStorage.getItem(LAYOUT_KEY) === "sidebar" ? "sidebar" : "tabs";
}

let nextTabId = 1;
function makeTab(cwd?: string, pendingCommand?: string): Tab {
  nextTabId += 1;
  return { id: nextTabId, cwd, pendingCommand, ptyId: null };
}

export default function Terminal() {
  const t = useT();
  const [tabs, setTabs] = useState<Tab[]>(() => [makeTab()]);
  const [activeId, setActiveId] = useState(() => tabs[0]?.id ?? 1);
  const [layout, setLayout] = useState<LayoutMode>(() => loadLayout());
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
          const fresh = makeTab();
          setActiveId(fresh.id);
          return [fresh];
        }
        if (id === activeId) {
          setActiveId(next[next.length - 1].id);
        }
        return next;
      });
    },
    [activeId],
  );

  /** 改 tab 名（空字符串 → 清除 customName，回退默认标题）。 */
  const renameTab = useCallback((id: number, name: string) => {
    const trimmed = name.trim();
    setTabs((prev) =>
      prev.map((tb) =>
        tb.id === id ? { ...tb, customName: trimmed || undefined } : tb,
      ),
    );
  }, []);

  const toggleLayout = useCallback(() => {
    setLayout((prev) => {
      const next = prev === "tabs" ? "sidebar" : "tabs";
      localStorage.setItem(LAYOUT_KEY, next);
      return next;
    });
  }, []);

  // ActionBar 联动：listen "terminal://new-tab" { cwd, command }
  // ⚠️ 必须限定 target 为当前窗口 label——否则 listen 默认 {kind:'Any'} 会收到
  // 所有窗口的事件，导致 Rust emit_to 定向失效，每个终端窗口都开 tab。
  // Rust 端 open_terminal_with_command 用 emit_to(label) 定向，前端这里对齐。
  useEffect(() => {
    let unlisten: (() => void) | null = null;
    const currentLabel = getCurrentWebviewWindow().label;
    listen<{ cwd?: string; command?: string }>(
      "terminal://new-tab",
      (e) => {
        addTab(e.payload.cwd, e.payload.command);
      },
      { target: currentLabel },
    )
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
  const defaultTitle = t("terminal.title");

  // 每个 tab 的渲染数据（标题/相位/agent 名）
  const tabMeta = tabs.map((tab) => {
    const phase = tab.ptyId != null ? (phases[tab.ptyId] ?? null) : null;
    const agentName = tab.ptyId != null ? (agents[tab.ptyId] ?? null) : null;
    return {
      tab,
      phase,
      agentName,
      label: displayLabel(tab.customName, agentName, defaultTitle),
      active: tab.id === activeId,
    };
  });

  const panes = (
    <div className="terminal-panes">
      {tabs.map((tab) => (
        <div
          key={tab.id}
          className="terminal-pane-wrapper"
          style={{ visibility: tab.id === activeId ? "visible" : "hidden" }}
          aria-hidden={tab.id !== activeId}
        >
          <TerminalPane
            cwd={tab.cwd}
            active={tab.id === activeId}
            pendingCommand={tab.pendingCommand}
            onNewTab={() => addTab()}
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
  );

  if (layout === "sidebar") {
    return (
      <div className="terminal-window terminal-sidebar-layout">
        <aside className="terminal-sidebar">
          <div className="terminal-sidebar-header">
            <button
              className="terminal-layout-toggle"
              onClick={toggleLayout}
              title={t("terminal.layoutTabs")}
              aria-label={t("terminal.layoutTabs")}
            >
              <LayoutPanelTop size={15} />
            </button>
            <button
              className="terminal-sidebar-new"
              onClick={() => addTab()}
              title={t("terminal.newTab")}
            >
              <Plus size={14} />
              <span>{t("terminal.newTab")}</span>
            </button>
          </div>
          <div className="terminal-sidebar-list" role="tablist">
            {tabMeta.map((m) => (
              <SidebarItem
                key={m.tab.id}
                active={m.active}
                phase={m.phase}
                label={m.label}
                onClick={() => setActiveId(m.tab.id)}
                onClose={() => closeTab(m.tab.id)}
                onRename={(name) => renameTab(m.tab.id, name)}
                closeLabel={t("terminal.closeTab")}
              />
            ))}
          </div>
        </aside>
        {panes}
      </div>
    );
  }

  return (
    <div className="terminal-window">
      <div className="terminal-tabbar" role="tablist">
        <button
          className="terminal-layout-toggle"
          onClick={toggleLayout}
          title={t("terminal.layoutSidebar")}
          aria-label={t("terminal.layoutSidebar")}
        >
          <LayoutPanelLeft size={15} />
        </button>
        {tabMeta.map((m) => (
          <TabButton
            key={m.tab.id}
            active={m.active}
            phase={m.phase}
            label={m.label}
            onCloseLabel={t("terminal.closeTab")}
            onClick={() => setActiveId(m.tab.id)}
            onClose={() => closeTab(m.tab.id)}
            onRename={(name) => renameTab(m.tab.id, name)}
          />
        ))}
        <button
          className="terminal-tab-new"
          onClick={() => addTab()}
          title={t("terminal.newTab")}
          aria-label={t("terminal.newTab")}
        >
          <Plus size={14} />
        </button>
      </div>
      {panes}
    </div>
  );
}

// ── 内联改名 input（双击标题触发）──
function RenameInput(props: {
  initial: string;
  onCommit: (name: string) => void;
  onCancel: () => void;
}) {
  return (
    <input
      className="terminal-rename-input"
      defaultValue={props.initial}
      autoFocus
      // 选中全部文本，方便整体替换
      onFocus={(e) => e.currentTarget.select()}
      onKeyDown={(e) => {
        if (e.key === "Enter") {
          e.preventDefault();
          props.onCommit((e.currentTarget as HTMLInputElement).value);
        } else if (e.key === "Escape") {
          e.preventDefault();
          props.onCancel();
        }
      }}
      onBlur={(e) => props.onCommit(e.target.value)}
      onClick={(e) => e.stopPropagation()}
      onDoubleClick={(e) => e.stopPropagation()}
    />
  );
}

// ── Tab 按钮（含 agent 状态徽章 + 双击改名）──
function TabButton(props: {
  active: boolean;
  phase: AgentPhase | null;
  label: string;
  onCloseLabel: string;
  onClick: () => void;
  onClose: () => void;
  onRename: (name: string) => void;
}) {
  const { active, phase, label, onClick, onClose, onCloseLabel, onRename } =
    props;
  const [editing, setEditing] = useState(false);
  return (
    <div
      className={`terminal-tab ${active ? "terminal-tab-active" : ""}`}
      role="tab"
      aria-selected={active}
      onClick={onClick}
    >
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
      <AgentBadge phase={phase} />
      {editing ? (
        <RenameInput
          initial={label}
          onCommit={(name) => {
            onRename(name);
            setEditing(false);
          }}
          onCancel={() => setEditing(false)}
        />
      ) : (
        <span
          className="terminal-tab-label"
          onDoubleClick={(e) => {
            e.stopPropagation();
            setEditing(true);
          }}
          title={label}
        >
          {label}
        </span>
      )}
    </div>
  );
}

// ── Sidebar list item（含 agent 状态徽章 + 双击改名）──
function SidebarItem(props: {
  active: boolean;
  phase: AgentPhase | null;
  label: string;
  closeLabel: string;
  onClick: () => void;
  onClose: () => void;
  onRename: (name: string) => void;
}) {
  const { active, phase, label, onClick, onClose, closeLabel, onRename } =
    props;
  const [editing, setEditing] = useState(false);
  return (
    <div
      className={`terminal-sidebar-item ${active ? "terminal-sidebar-item-active" : ""}`}
      role="tab"
      aria-selected={active}
      onClick={onClick}
    >
      <button
        className="terminal-tab-close"
        onClick={(e) => {
          e.stopPropagation();
          onClose();
        }}
        aria-label={closeLabel}
      >
        <X size={12} />
      </button>
      <AgentBadge phase={phase} />
      {editing ? (
        <RenameInput
          initial={label}
          onCommit={(name) => {
            onRename(name);
            setEditing(false);
          }}
          onCancel={() => setEditing(false)}
        />
      ) : (
        <span
          className="terminal-sidebar-item-label"
          onDoubleClick={(e) => {
            e.stopPropagation();
            setEditing(true);
          }}
          title={label}
        >
          {label}
        </span>
      )}
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
