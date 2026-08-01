/**
 * 文件树侧栏——右侧，默认隐藏，工具条切换展开/收缩。
 *
 * 展开态：上方工具条（>> 收起 + 隐藏文件切换）+ 下方文件树
 * 收缩态：小长条（<< 展开）
 *
 * 根目录跟随当前 tab 的 trackedCwd（OSC 7）。懒加载：点击目录展开才
 * invoke terminal_list_dir 加载子项。
 */

import { useCallback, useEffect, useState } from "react";
import { ChevronRight, ChevronDown, Folder, File } from "lucide-react";
import { invoke } from "@tauri-apps/api/core";
import { useT } from "@/lib/i18n";
import { ContextMenu, type MenuPosition, type MenuItem } from "./ContextMenu";
import { PanelResizer } from "./PanelResizer";
import { startDrag } from "./dragStore";

type FileEntry = {
  name: string;
  kind: string; // "dir" | "file"
};

type ChildrenState =
  | { status: "idle" }
  | { status: "loading" }
  | { status: "loaded"; entries: FileEntry[] }
  | { status: "error" };

type Props = {
  cwd: string | null;
  expanded: boolean;
  onToggle: () => void;
  width?: number;
  // resizer 回调（展开态才渲染手柄）
  onResizerStart?: () => void;
  onResizerMove?: (clientX: number) => void;
  onResizerEnd?: () => void;
};

export function FileTreePanel({
  cwd,
  expanded,
  onToggle,
  width,
  onResizerStart,
  onResizerMove,
  onResizerEnd,
}: Props) {
  const t = useT();
  const [showHidden, setShowHidden] = useState(false);
  // 展开的目录路径集合
  const [expandedDirs, setExpandedDirs] = useState<Set<string>>(new Set());
  // 目录路径 → 子项状态（懒加载缓存）
  const [tree, setTree] = useState<Record<string, ChildrenState>>({});
  const [selected, setSelected] = useState<string | null>(null);
  const [menuPos, setMenuPos] = useState<MenuPosition>(null);
  const [menuItems, setMenuItems] = useState<MenuItem[]>([]);

  /** 文件树节点右键：复制路径 / 复制名称 */
  const openNodeMenu = useCallback((e: React.MouseEvent, name: string, fullPath: string) => {
    e.preventDefault();
    e.stopPropagation();
    const copyToClipboard = (text: string) => {
      const ta = document.createElement("textarea");
      ta.value = text;
      ta.style.position = "fixed";
      ta.style.opacity = "0";
      document.body.appendChild(ta);
      ta.select();
      document.execCommand("copy");
      document.body.removeChild(ta);
    };
    setMenuItems([
      { label: t("terminal.ctxCopyPath"), action: () => copyToClipboard(fullPath) },
      { label: t("terminal.ctxCopyName"), action: () => copyToClipboard(name) },
    ]);
    setMenuPos({ x: e.clientX, y: e.clientY });
  }, [t]);

  // cwd 变化时重置树
  useEffect(() => {
    setExpandedDirs(new Set());
    setTree({});
    setSelected(null);
  }, [cwd]);

  // 加载目录子项
  const loadDir = useCallback(async (dirPath: string, currentShowHidden: boolean) => {
    setTree((prev) => ({ ...prev, [dirPath]: { status: "loading" } }));
    try {
      const entries = await invoke<FileEntry[]>("terminal_list_dir", {
        path: dirPath,
        showHidden: currentShowHidden,
      });
      setTree((prev) => ({ ...prev, [dirPath]: { status: "loaded", entries } }));
    } catch {
      setTree((prev) => ({ ...prev, [dirPath]: { status: "error" } }));
    }
  }, []);

  // showHidden 变化时重新加载已展开的目录
  useEffect(() => {
    if (!expanded || !cwd) return;
    // 重新加载根 + 所有展开的目录
    void loadDir(cwd, showHidden);
    for (const dir of expandedDirs) {
      void loadDir(dir, showHidden);
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [showHidden]);

  // 展开时（false→true）自动加载根目录——总是重新加载，不管之前状态
  useEffect(() => {
    if (expanded && cwd) {
      void loadDir(cwd, showHidden);
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [expanded]);

  // cwd 变化时重新加载根目录 + 重置展开状态
  useEffect(() => {
    if (expanded && cwd) {
      void loadDir(cwd, showHidden);
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [cwd]);

  /** 手动刷新：重新加载根目录 + 所有展开的子目录 */
  const refresh = useCallback(() => {
    if (!cwd) return;
    void loadDir(cwd, showHidden);
    for (const dir of expandedDirs) {
      void loadDir(dir, showHidden);
    }
  }, [cwd, showHidden, expandedDirs, loadDir]);

  const toggleDir = (dirPath: string) => {
    setExpandedDirs((prev) => {
      const next = new Set(prev);
      if (next.has(dirPath)) {
        next.delete(dirPath);
      } else {
        next.add(dirPath);
        // 懒加载：未加载过则加载
        if (tree[dirPath]?.status === "idle" || !tree[dirPath]) {
          void loadDir(dirPath, showHidden);
        }
      }
      return next;
    });
  };

  // 递归渲染树节点
  const renderNode = (name: string, fullPath: string, kind: string, depth: number) => {
    const isDir = kind === "dir";
    const isExpandedDir = expandedDirs.has(fullPath);
    const isSelected = selected === fullPath;
    const children = tree[fullPath];

    return (
      <div key={fullPath}>
        <div
          className={`file-tree-row ${isSelected ? "file-tree-row-selected" : ""}`}
          style={{ paddingLeft: `${depth * 12 + 4}px` }}
          // pointer events 方案：mousedown 记录拖拽路径（HTML5 DnD 在 WKWebView 不可靠）。
          // 终端 canvas 的 mouseup 读 dragStore 写入。普通 click 仍走 onClick（mousedown
          // 只设状态，不干扰 click——click 在 mouseup 后触发，dragPath 已被取走/清除）。
          onMouseDown={(e) => {
            if (e.button === 0) {
              startDrag(fullPath, name); // 记录路径 + 创建 ghost + 启动 mousemove 跟踪
              e.preventDefault(); // 禁止浏览器默认文本选择（拖动时涂蓝）
            }
          }}
          onClick={() => {
            if (isDir) toggleDir(fullPath);
            else setSelected(fullPath);
          }}
          onContextMenu={(e) => openNodeMenu(e, name, fullPath)}
        >
          {isDir ? (
            <>
              {isExpandedDir ? (
                <ChevronDown size={12} className="file-tree-chevron" />
              ) : (
                <ChevronRight size={12} className="file-tree-chevron" />
              )}
              <Folder size={13} className="file-tree-icon-dir" />
            </>
          ) : (
            <>
              <span className="file-tree-chevron-spacer" />
              <File size={13} className="file-tree-icon-file" />
            </>
          )}
          <span className="file-tree-name">{name}</span>
        </div>
        {isDir && isExpandedDir && children?.status === "loaded" && (
          <div>
            {children.entries.map((e) =>
              renderNode(e.name, joinPath(fullPath, e.name), e.kind, depth + 1),
            )}
          </div>
        )}
        {isDir && isExpandedDir && children?.status === "loading" && (
          <div className="file-tree-loading" style={{ paddingLeft: `${(depth + 1) * 12 + 4}px` }}>
            ...
          </div>
        )}
      </div>
    );
  };

  // ── 收缩态：小长条 ──
  if (!expanded) {
    return (
      <div className="file-tree-collapsed" onClick={onToggle} title={t("terminal.fileTreeExpand")}>
        <img src="icons/angles-left.svg" alt="expand" className="file-tree-tool-icon" />
      </div>
    );
  }

  // ── 展开态：工具条 + 文件树 ──
  return (
    <div
      className="file-tree-panel"
      style={width !== undefined ? { width: `${width}px` } : undefined}
    >
      {/* 拖拽手柄（左边缘）——仅展开态渲染 */}
      {onResizerStart && onResizerMove && onResizerEnd && (
        <PanelResizer
          side="left"
          onStart={onResizerStart}
          onMove={onResizerMove}
          onEnd={onResizerEnd}
        />
      )}
      <div className="file-tree-toolbar">
        <button
          className="file-tree-tool-btn"
          onClick={onToggle}
          title={t("terminal.fileTreeCollapse")}
        >
          <img src="icons/angles-right.svg" alt="collapse" className="file-tree-tool-icon" />
        </button>
        <button
          className="file-tree-tool-btn"
          onClick={() => setShowHidden(!showHidden)}
          title={t("terminal.fileTreeToggleHidden")}
          style={{ opacity: showHidden ? 1 : 0.5 }}
        >
          <img src="icons/eye.svg" alt="hidden" className="file-tree-tool-icon" />
        </button>
        <button
          className="file-tree-tool-btn"
          onClick={refresh}
          title={t("terminal.fileTreeRefresh")}
        >
          <img src="icons/refresh.svg" alt="refresh" className="file-tree-tool-icon" />
        </button>
      </div>
      <div className="file-tree-body">
        {cwd ? (
          tree[cwd]?.status === "loaded" ? (
            tree[cwd].entries.map((e) =>
              renderNode(e.name, joinPath(cwd, e.name), e.kind, 0),
            )
          ) : tree[cwd]?.status === "loading" ? (
            <div className="file-tree-empty">...</div>
          ) : tree[cwd]?.status === "error" ? (
            <div className="file-tree-empty">{t("terminal.fileTreeError")}</div>
          ) : (
            <div className="file-tree-empty">{t("terminal.fileTreeLoading")}</div>
          )
        ) : (
          <div className="file-tree-empty">{t("terminal.fileTreeWaiting")}</div>
        )}
      </div>
      <ContextMenu position={menuPos} items={menuItems} onClose={() => setMenuPos(null)} />
    </div>
  );
}

/** 拼接路径（处理尾部斜杠）。pure，可复用。 */
function joinPath(parent: string, name: string): string {
  if (parent.endsWith("/")) return `${parent}${name}`;
  return `${parent}/${name}`;
}
