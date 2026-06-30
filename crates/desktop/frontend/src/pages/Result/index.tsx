import { useState, useEffect, useRef, useCallback } from "react";
import { invoke } from "@tauri-apps/api/core";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { LogicalSize } from "@tauri-apps/api/dpi";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { cn } from "@/lib/utils";
import { SvgIcon, type IconName } from "@/components/SvgIcon";

const DIVERTED_DELAY_MS = 300;

// ── 编辑框尺寸双模式（精简 520×116 / 长篇 720×480，长篇可拖拽且记忆）──
const COMPACT_SIZE = { w: 520, h: 116 };
const EXPANDED_DEFAULT = { w: 720, h: 480 };
const EXPANDED_SIZE_KEY = "result-expanded-size";

function loadExpandedSize(): { w: number; h: number } {
  const saved = localStorage.getItem(EXPANDED_SIZE_KEY);
  if (saved) {
    const [w, h] = saved.split(",").map(Number);
    if (w > 0 && h > 0) return { w, h };
  }
  return EXPANDED_DEFAULT;
}

const POLISH_OPTIONS = [
  { mode: 0, label: "关闭" },
  { mode: 1, label: "仅最终润色" },
  { mode: 2, label: "中间 + 最终润色" },
];

const DENOISE_OPTIONS = [
  { mode: 0, label: "无降噪" },
  { mode: 1, label: "轻度降噪" },
  { mode: 2, label: "深度降噪" },
];

interface ToolbarState {
  polish_mode: number;
  denoise_mode: number;
  polish_llm_valid: boolean;
  hide_toolbar: boolean;
  edit_shortcut: string;
}

interface PopupItem {
  label: string;
  current: boolean;
  name?: string;
  mode?: number;
}

type PopupType = "polish" | "denoise" | "asr" | "llm" | null;

function Result() {
  const [visible, setVisible] = useState(false);
  const [toolbarVisible, setToolbarVisible] = useState(false);
  const [editing, setEditing] = useState(false);
  const [expanded, setExpanded] = useState(false);
  const [text, setText] = useState("");
  const [isRecording, setIsRecording] = useState(false);
  const [isSpeaking, setIsSpeaking] = useState(false);
  const [toolbarState, setToolbarState] = useState<ToolbarState>({
    polish_mode: 0, denoise_mode: 1, polish_llm_valid: false,
    hide_toolbar: true, edit_shortcut: "Cmd+Enter",
  });
  const [popupType, setPopupType] = useState<PopupType>(null);
  const [popupItems, setPopupItems] = useState<PopupItem[]>([]);
  const [toast, setToast] = useState<string | null>(null);
  const [polishLoading, setPolishLoading] = useState(false);

  const textRef = useRef<HTMLDivElement>(null);
  const editingRef = useRef(false);
  const displayedRef = useRef("");
  const pendingDiverted = useRef<string | null>(null);
  const divertedTimer = useRef<ReturnType<typeof setTimeout> | null>(null);
  const editBufTimer = useRef<ReturnType<typeof setTimeout> | null>(null);
  const speakingTimer = useRef<ReturnType<typeof setTimeout> | null>(null);
  const toolbarVisibleRef = useRef(false);
  const editingStateRef = useRef(false);
  const editSnapshotRef = useRef(""); // 编辑前原始文本快照
  const expandedRef = useRef(false); // 同步 expanded 给 onResized 闭包（防读旧值）
  const expandedSizeRef = useRef(loadExpandedSize()); // 长篇模式记忆的逻辑尺寸

  const win = getCurrentWindow();

  useEffect(() => { editingRef.current = editing; }, [editing]);
  useEffect(() => { toolbarVisibleRef.current = toolbarVisible; }, [toolbarVisible]);
  useEffect(() => { editingStateRef.current = editing; }, [editing]);
  useEffect(() => { expandedRef.current = expanded; }, [expanded]);

  const showToast = useCallback((msg: string) => {
    setToast(msg);
    setTimeout(() => setToast(null), 2000);
  }, []);

  const showToolbar = useCallback(() => {
    if (toolbarVisibleRef.current) return;
    setToolbarVisible(true);
  }, []);

  const hideToolbar = useCallback(() => {
    if (!toolbarVisibleRef.current || editingStateRef.current) return;
    setToolbarVisible(false);
    setPopupType(null);
  }, []);

  const refreshActive = useCallback(async () => {
    try {
      const st = await invoke<ToolbarState>("toolbar_state");
      setToolbarState(st);
      if (st.hide_toolbar === false) {
        showToolbar();
      } else {
        hideToolbar();
      }
    } catch { /* ignore */ }
  }, [showToolbar, hideToolbar]);

  const renderResultNow = useCallback((newText: string) => {
    displayedRef.current = newText;
    setText(newText);
    if (textRef.current) {
      textRef.current.scrollTop = textRef.current.scrollHeight;
    }
    // 标记正在说话
    setIsSpeaking(true);
    if (speakingTimer.current) clearTimeout(speakingTimer.current);
    speakingTimer.current = setTimeout(() => setIsSpeaking(false), 1500);
  }, []);

  // ── Toolbar hover ──
  useEffect(() => {
    const container = document.getElementById("result-container");
    if (!container) return;
    if (toolbarState.hide_toolbar === false) return;

    const onMove = () => showToolbar();
    const onLeave = () => hideToolbar();
    container.addEventListener("mousemove", onMove);
    container.addEventListener("mouseleave", onLeave);
    return () => {
      container.removeEventListener("mousemove", onMove);
      container.removeEventListener("mouseleave", onLeave);
    };
  }, [toolbarState.hide_toolbar, showToolbar, hideToolbar]);

  // ── Tauri events ──
  useEffect(() => {
    let unlistens: UnlistenFn[] = [];
    let cancelled = false;

    (async () => {
      const handlers: [string, (payload: unknown) => void][] = [
        ["show-result", (p) => {
          const text = p as string;
          const isPlaceholder = text === "正在聆听…" || text === "正在聆听...";
          setVisible(true);
          setIsRecording(true);
          refreshActive();
          if (isPlaceholder) {
            // 新录音开始：清空上次残留
            setText("");
            displayedRef.current = "";
            pendingDiverted.current = null;
            if (divertedTimer.current) { clearTimeout(divertedTimer.current); divertedTimer.current = null; }
          } else {
            renderResultNow(text);
          }
        }],
        ["update-result", (p) => {
          if (editingRef.current) return;
          const newText = p as string;
          if (newText === displayedRef.current || newText === pendingDiverted.current) return;
          if (newText.startsWith(displayedRef.current)) {
            if (divertedTimer.current) { clearTimeout(divertedTimer.current); divertedTimer.current = null; }
            pendingDiverted.current = null;
            renderResultNow(newText);
          } else {
            pendingDiverted.current = newText;
            if (!divertedTimer.current) {
              divertedTimer.current = setTimeout(() => {
                divertedTimer.current = null;
                if (pendingDiverted.current !== null) {
                  renderResultNow(pendingDiverted.current);
                  pendingDiverted.current = null;
                }
              }, DIVERTED_DELAY_MS);
            }
          }
        }],
        ["clear-result", () => {
          setText("");
          displayedRef.current = "";
          setVisible(false);
          setIsRecording(false);
        }],
        ["hide-result", () => { setVisible(false); setIsRecording(false); }],
        ["config-changed", () => refreshActive()],
        ["polish-done", () => setPolishLoading(false)],
        ["edit-force-exit", () => {
          if (editingRef.current) {
            setEditing(false);
            if (editBufTimer.current) clearTimeout(editBufTimer.current);
          }
        }],
      ];
      for (const [event, handler] of handlers) {
        const fn = await listen(event, (e) => handler(e.payload));
        if (cancelled) { fn(); return; }
        unlistens.push(fn);
      }
      if (!cancelled) {
        invoke("result_window_ready");
      }
    })();

    return () => { cancelled = true; unlistens.forEach((fn) => fn()); };
  }, [renderResultNow, refreshActive]);

  // ── Edit mode ──
  const enterEdit = useCallback(() => {
    if (editingRef.current) return;
    if (!displayedRef.current.trim()) return;
    editSnapshotRef.current = displayedRef.current; // 保存快照
    setEditing(true);
    setIsRecording(false);
    showToolbar();
    invoke("enter_edit_mode");
    setTimeout(() => {
      const el = textRef.current;
      if (!el) return;
      el.focus();
      const sel = window.getSelection();
      sel?.removeAllRanges();
      const range = document.createRange();
      range.selectNodeContents(el);
      range.collapse(false);
      sel?.addRange(range);
    }, 0);
  }, [showToolbar]);

  const commitEdit = useCallback(() => {
    if (!editingRef.current) return;
    if (editBufTimer.current) clearTimeout(editBufTimer.current);
    const el = textRef.current;
    const editedText = el?.innerText ?? "";
    setEditing(false);
    invoke("commit_edit", { text: editedText });
  }, []);

  const cancelEdit = useCallback(() => {
    if (!editingRef.current) return;
    if (editBufTimer.current) clearTimeout(editBufTimer.current);
    const original = editSnapshotRef.current;
    setEditing(false);
    // 恢复 contentEditable DOM 到编辑前文本
    displayedRef.current = original;
    setText(original);
    if (textRef.current) {
      textRef.current.innerText = original;
    }
    // 只退出编辑态，不 commit（不写 edited_text 到 DB）
    invoke("exit_edit_without_commit");
  }, []);

  const toggleEdit = useCallback(() => {
    editingRef.current ? commitEdit() : enterEdit();
  }, [commitEdit, enterEdit]);

  // 立即润色：工具栏按钮 + 全局 polish_global_shortcut 共用。
  // polishLoading 门控（幂等，与按钮 disabled 一致）+ 空文本判空（无结果静默）。
  const polishNow = useCallback(async () => {
    if (polishLoading) return;
    if (!displayedRef.current.trim()) return;
    setPolishLoading(true);
    try { await invoke("polish_now"); showToast("润色中…"); }
    catch (e) { setPolishLoading(false); showToast("润色失败：" + e); }
  }, [polishLoading, showToast]);

  // 存入记事本：把当前显示文本存为新笔记（内容由前端传入，根治 current_transcription_id
  // 全局值与显示文本的跨信道竞态），transcription_id 仅作溯源。无活动记录时静默返回。
  const saveToNote = useCallback(async () => {
    try {
      const tid = await invoke<number | null>("current_transcription_id");
      if (tid == null) return;
      await invoke<number>("save_transcription_to_note", { transcriptionId: tid, text });
      showToast("已存入记事本");
    } catch (e) {
      console.error(e);
      showToast("存入记事本失败：" + e);
    }
  }, [showToast, text]);

  // 放大/缩小开关：切换编辑框精简(520×116) ↔ 长篇(记忆尺寸或默认 720×480)。
  // 先同步 expandedRef，防 setSize 触发的 onResized 读到旧值污染长篇记忆。
  // 尺寸与编辑态解耦——任一模式均可编辑（toggleEdit）。
  const toggleExpand = useCallback(async () => {
    const next = !expanded;
    expandedRef.current = next;
    setExpanded(next);
    await win.setResizable(next);
    if (next) {
      const { w, h } = expandedSizeRef.current;
      await win.setSize(new LogicalSize(w, h));
    } else {
      await win.setSize(new LogicalSize(COMPACT_SIZE.w, COMPACT_SIZE.h));
    }
  }, [expanded, win]);

  // 全局编辑快捷键（edit_global_shortcut）：后端唤起窗口+focus 后 emit 此事件，
  // 复用 toggleEdit——未编辑则进入、已编辑则保存，与窗口内 Cmd+Enter 同语义。
  // 独立 useEffect（而非并入上面的事件数组）：toggleEdit 在此声明，前置使用会触发 TS2448。
  useEffect(() => {
    let unlisten: UnlistenFn | undefined;
    let cancelled = false;
    listen("global-edit-toggle", () => toggleEdit()).then((fn) => {
      if (cancelled) fn();
      else unlisten = fn;
    });
    return () => { cancelled = true; unlisten?.(); };
  }, [toggleEdit]);

  // 全局立即润色快捷键（polish_global_shortcut）：后端 show 结果窗（不聚焦）后 emit 此事件，
  // 复用 polishNow——空文本静默、进行中幂等，与工具栏「立即润色」按钮同语义。
  // 独立 useEffect（同 global-edit-toggle）：polishNow 在此声明，前置使用触发 TS2448。
  useEffect(() => {
    let unlisten: UnlistenFn | undefined;
    let cancelled = false;
    listen("global-polish-trigger", () => polishNow()).then((fn) => {
      if (cancelled) fn();
      else unlisten = fn;
    });
    return () => { cancelled = true; unlisten?.(); };
  }, [polishNow]);

  // 长篇模式拖拽调整窗口 → 记忆逻辑尺寸到 localStorage，下次切长篇恢复。
  // 精简模式（expandedRef=false）的 setSize 也会触发，但被门控跳过，不污染长篇记忆。
  useEffect(() => {
    let unlisten: UnlistenFn | undefined;
    let cancelled = false;
    win.onResized(async () => {
      if (!expandedRef.current) return;
      const factor = await win.scaleFactor();
      const s = await win.outerSize();
      const w = s.width / factor;
      const h = s.height / factor;
      expandedSizeRef.current = { w, h };
      localStorage.setItem(EXPANDED_SIZE_KEY, `${w},${h}`);
    }).then((fn) => { if (cancelled) fn(); else unlisten = fn; });
    return () => { cancelled = true; unlisten?.(); };
  }, [win]);

  const updateEditBuffer = useCallback(() => {
    if (!editingRef.current) return;
    invoke("update_edit_buffer", { text: textRef.current?.innerText ?? "" });
  }, []);

  // ── Keyboard shortcuts ──
  useEffect(() => {
    const onKeyDown = (e: KeyboardEvent) => {
      if (e.key === "Escape") {
        // 分层：弹窗 > 编辑态 > 录音。编辑态 ESC 放弃编辑（不保存，还原原文），
        // 再按一次 ESC（此时已退出编辑态）才放弃录音。
        if (popupType) { setPopupType(null); return; }
        if (editingRef.current) { cancelEdit(); return; }
        invoke("cancel_recording");
        win.hide();
        return;
      }
      const sc = parseShortcut(toolbarState.edit_shortcut);
      if (matchShortcut(e, sc)) {
        e.preventDefault();
        toggleEdit();
      }
    };
    document.addEventListener("keydown", onKeyDown);
    return () => document.removeEventListener("keydown", onKeyDown);
  }, [popupType, toolbarState.edit_shortcut, toggleEdit, win, cancelEdit]);

  // ── Popup close on outside click ──
  useEffect(() => {
    if (!popupType) return;
    const onDown = (e: MouseEvent) => {
      const target = e.target as HTMLElement;
      if (!target.closest(".popup-content") && !target.closest(".tool-btn")) {
        setPopupType(null);
      }
    };
    document.addEventListener("mousedown", onDown);
    return () => document.removeEventListener("mousedown", onDown);
  }, [popupType]);

  // ── Text input handler ──
  const onTextInput = () => {
    if (editBufTimer.current) clearTimeout(editBufTimer.current);
    editBufTimer.current = setTimeout(updateEditBuffer, 150);
  };

  // ── Popup actions ──
  const openPolishPopup = async () => {
    setPopupItems(POLISH_OPTIONS.map(o => ({
      label: o.label, current: o.mode === toolbarState.polish_mode, mode: o.mode,
    })));
    setPopupType("polish");
  };

  const openDenoisePopup = async () => {
    setPopupItems(DENOISE_OPTIONS.map(o => ({
      label: o.label, current: o.mode === toolbarState.denoise_mode, mode: o.mode,
    })));
    setPopupType("denoise");
  };

  const handlePopupSelect = async (item: PopupItem) => {
    try {
      if (popupType === "polish" && item.mode !== undefined) {
        await invoke("set_polish_mode", { mode: item.mode });
      } else if (popupType === "denoise" && item.mode !== undefined) {
        await invoke("set_denoise_mode", { mode: item.mode });
      }
      setPopupType(null);
      refreshActive();
    } catch (e) { showToast("切换失败：" + e); }
  };

  // ── Drag handle ──
  const onDragStart = (e: React.MouseEvent) => {
    e.preventDefault();
    win.startDragging();
  };

  const tools: { id: string; icon: IconName; label: string; active?: boolean; disabled?: boolean; onClick: () => void }[] = [
    { id: "close", icon: "close", label: "关闭", onClick: () => invoke("discard_recording") },
    { id: "settings", icon: "settings", label: "系统设置", onClick: () => invoke("open_settings") },
    { id: "denoise", icon: "denoise", label: "降噪模式", active: toolbarState.denoise_mode !== 0, onClick: openDenoisePopup },
    { id: "polish", icon: "polish", label: "润色模式", active: toolbarState.polish_mode !== 0, onClick: openPolishPopup },
    { id: "polish-now", icon: "polish-now", label: "立即润色", disabled: polishLoading, onClick: polishNow },
    { id: "note", icon: "note", label: "存入记事本", disabled: !text.trim(), onClick: saveToNote },
    { id: "toggle-size", icon: (expanded ? "minimize" : "expand-edit") as IconName, label: expanded ? "缩小" : "放大", onClick: toggleExpand },
    ...(editing
      ? [
          { id: "cancel-edit", icon: "cancel-editor" as IconName, label: "取消编辑", onClick: cancelEdit },
          { id: "save", icon: "save" as IconName, label: "保存编辑", active: true, onClick: commitEdit },
        ]
      : [
          { id: "edit", icon: "edit" as IconName, label: "编辑", disabled: !text.trim(), onClick: toggleEdit },
        ]
    ),
  ];

  return (
    <div
      id="result-container"
      className={cn(
        "w-full h-full bg-background rounded-lg border border-black/[0.08] shadow-lg shadow-black/[0.06] flex flex-col transition-opacity duration-150 overflow-hidden",
        visible ? "opacity-100" : "opacity-0",
      )}
    >
      {/* Top bar: toolbar + drag handle + voice line */}
      <div className="flex-shrink-0 flex flex-col relative">
        {/* 录音提示——独立于工具栏 opacity，常显居中 */}
        {!text.trim() && isRecording && (
          <div className="absolute top-0 left-0 right-0 flex items-center justify-center h-[22px] pointer-events-none z-20">
            <span className="text-[11px] text-black/[0.28] select-none">正在聆听…</span>
          </div>
        )}
        {/* Toolbar — 纯图标，hover 变蓝，整行可拖拽 */}
        <div
          className={cn(
            "flex items-center gap-[2px] px-1.5 pt-0.5 transition-opacity duration-150 cursor-grab active:cursor-grabbing",
            toolbarState.hide_toolbar === false
              ? "opacity-100"
              : toolbarVisible ? "opacity-100" : "opacity-0",
          )}
          onMouseDown={onDragStart}
        >
          {tools.map(({ id, icon, label, active, disabled, onClick }) => (
            <button
              key={id}
              className={cn(
                "tool-btn w-[20px] h-[20px] flex items-center justify-center rounded-[4px] transition-colors cursor-default",
                "text-black/[0.55] hover:text-[#007aff] hover:bg-black/[0.05]",
                active && "text-[#007aff]",
                disabled && "text-black/[0.18] cursor-default hover:bg-transparent hover:text-black/[0.18]",
              )}
              title={label}
              aria-label={label}
              disabled={disabled}
              onClick={onClick}
            >
              <SvgIcon name={icon} size={16} />
            </button>
          ))}
        </div>
        {/* Drag handle */}
        <div className="flex items-center justify-center h-2">
          <div
            className="w-6 h-[3px] rounded-[1.5px] bg-black/[0.12] cursor-grab active:cursor-grabbing"
            onMouseDown={onDragStart}
          />
        </div>
        {/* Voice line: 说话时绿色流动 / 静音时静态灰线 / 编辑态 voice 底线 */}
        {isRecording && !editing && (
          <div className={cn("mx-3.5 transition-all duration-300", isSpeaking ? "voice-line-speaking" : "voice-line-idle")} />
        )}
        {editing && (
          <div className="h-0.5 bg-voice/30 mx-0" />
        )}
      </div>

      {/* Text display */}
      <div
        className={cn(
          "flex-1 px-3.5 pt-1 pb-2 overflow-hidden relative transition-colors",
          editing && "bg-voice/[0.06]",
        )}
      >
        <div
          ref={textRef}
          className={cn(
            "text-sm leading-[1.6] text-foreground overflow-y-auto",
            expanded ? "h-full" : "max-h-[63px]",
            "break-words outline-none thin-scrollbar",
            !editing && "cursor-text",
          )}
          contentEditable={editing}
          suppressContentEditableWarning
          onInput={onTextInput}
        >
          {text}
        </div>
      </div>

      {/* Bottom toolbar removed — moved to top */}

      {/* Popup */}
      {popupType && (
        <div className="popup-content absolute top-[28px] left-1.5 w-[360px] bg-white rounded-lg border border-black/[0.10] shadow-lg shadow-black/[0.12] z-30 text-[12px]">
          {popupItems.map((item, i) => (
            <div
              key={i}
              className={cn(
                "px-3 py-1 cursor-pointer flex items-center gap-1.5 transition-colors",
                "hover:bg-[#007aff]/[0.08]",
                item.current && "text-[#007aff] font-medium",
              )}
              onClick={() => handlePopupSelect(item)}
            >
              <span className={cn("text-[10px]", item.current ? "text-[#007aff]" : "text-black/40")}>
                {item.current ? "●" : "○"}
              </span>
              <span className="flex-1 min-w-0 truncate text-foreground">{item.label}</span>
            </div>
          ))}
        </div>
      )}

      {/* Toast */}
      {toast && (
        <div className="absolute bottom-1.5 left-1/2 -translate-x-1/2 bg-black/78 text-white text-xs px-2.5 py-1 rounded-md z-20 pointer-events-none">
          {toast}
        </div>
      )}
    </div>
  );
}

export default Result;

// ── Shortcut parsing ──
function parseShortcut(s: string) {
  const parts = s.toLowerCase().split("+").map((p) => p.trim());
  const key = parts.pop();
  const cmdOrCtrl = parts.includes("cmdorctrl");
  return {
    key,
    cmdOrCtrl,
    meta: parts.includes("cmd") || parts.includes("super") || parts.includes("meta"),
    ctrl: parts.includes("control") || parts.includes("ctrl"),
    alt: parts.includes("alt") || parts.includes("option"),
    shift: parts.includes("shift"),
  };
}

function matchShortcut(e: KeyboardEvent, sc: ReturnType<typeof parseShortcut>) {
  if (!sc || !sc.key || e.key.toLowerCase() !== sc.key) return false;
  if (sc.cmdOrCtrl) {
    if (!(e.metaKey || e.ctrlKey)) return false;
  } else {
    if (e.metaKey !== sc.meta) return false;
    if (e.ctrlKey !== sc.ctrl) return false;
  }
  return e.altKey === sc.alt && e.shiftKey === sc.shift;
}
