import { useState, useEffect, useRef, useCallback } from "react";
import { invoke } from "@tauri-apps/api/core";
import { getCurrentWindow, LogicalSize } from "@tauri-apps/api/window";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import {
  X, Settings, Mic, Waves, Sparkles, Wand2, Zap, Pencil, Save,
  type LucideIcon,
} from "lucide-react";
import { cn } from "@/lib/utils";

const WIN_W = 520;
const HIDDEN_H = 100;
const TOOLBAR_H = 132;
const DIVERTED_DELAY_MS = 300;

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
  const [text, setText] = useState("");
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
  const toolbarVisibleRef = useRef(false);
  const editingStateRef = useRef(false);

  const win = getCurrentWindow();

  useEffect(() => { editingRef.current = editing; }, [editing]);
  useEffect(() => { toolbarVisibleRef.current = toolbarVisible; }, [toolbarVisible]);
  useEffect(() => { editingStateRef.current = editing; }, [editing]);

  const showToast = useCallback((msg: string) => {
    setToast(msg);
    setTimeout(() => setToast(null), 2000);
  }, []);

  const showToolbar = useCallback(() => {
    if (toolbarVisibleRef.current) return;
    setToolbarVisible(true);
    win.setSize(new LogicalSize(WIN_W, TOOLBAR_H));
  }, [win]);

  const hideToolbar = useCallback(() => {
    if (!toolbarVisibleRef.current || editingStateRef.current) return;
    setToolbarVisible(false);
    setPopupType(null);
    win.setSize(new LogicalSize(WIN_W, HIDDEN_H));
  }, [win]);

  const refreshActive = useCallback(async () => {
    try {
      const st = await invoke<ToolbarState>("toolbar_state");
      setToolbarState(st);
      if (st.hide_toolbar === false) {
        showToolbar();
      }
    } catch { /* ignore */ }
  }, [showToolbar]);

  const renderResultNow = useCallback((newText: string) => {
    displayedRef.current = newText;
    setText(newText);
    if (textRef.current) {
      textRef.current.scrollTop = textRef.current.scrollHeight;
    }
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
          renderResultNow(p as string);
          setVisible(true);
          refreshActive();
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
        }],
        ["hide-result", () => setVisible(false)],
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
    })();

    return () => { cancelled = true; unlistens.forEach((fn) => fn()); };
  }, [renderResultNow, refreshActive]);

  // ── Edit mode ──
  const enterEdit = useCallback(() => {
    if (editingRef.current) return;
    setEditing(true);
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

  const toggleEdit = useCallback(() => {
    editingRef.current ? commitEdit() : enterEdit();
  }, [commitEdit, enterEdit]);

  const updateEditBuffer = useCallback(() => {
    if (!editingRef.current) return;
    invoke("update_edit_buffer", { text: textRef.current?.innerText ?? "" });
  }, []);

  // ── Keyboard shortcuts ──
  useEffect(() => {
    const onKeyDown = (e: KeyboardEvent) => {
      // Esc: cancel + hide (unless popup open or editing)
      if (e.key === "Escape") {
        if (popupType) { setPopupType(null); return; }
        if (editingRef.current) { commitEdit(); return; }
        invoke("cancel_recording");
        win.hide();
        return;
      }
      // Edit shortcut (Cmd+Enter by default)
      const sc = parseShortcut(toolbarState.edit_shortcut);
      if (matchShortcut(e, sc)) {
        e.preventDefault();
        toggleEdit();
      }
    };
    document.addEventListener("keydown", onKeyDown);
    return () => document.removeEventListener("keydown", onKeyDown);
  }, [popupType, toolbarState.edit_shortcut, toggleEdit, win, commitEdit]);

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

  // ── Notify backend window is ready ──
  useEffect(() => {
    invoke("result_window_ready");
    refreshActive();
  }, [refreshActive]);

  // ── Text input handler (edit mode) ──
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

  const openAsrPopup = async () => {
    try {
      const engines = await invoke<{ name: string; label: string; current: boolean }[]>("list_asr_engines");
      setPopupItems(engines.map(e => ({ label: e.label, current: e.current, name: e.name })));
      setPopupType("asr");
    } catch (e) { showToast("读取引擎失败：" + e); }
  };

  const openLlmPopup = async () => {
    try {
      const models = await invoke<{ name: string; label: string; current: boolean }[]>("list_llm_models");
      setPopupItems(models.map(m => ({ label: m.label, current: m.current, name: m.name })));
      setPopupType("llm");
    } catch (e) { showToast("读取润色模型失败：" + e); }
  };

  const handlePopupSelect = async (item: PopupItem) => {
    try {
      if (popupType === "polish" && item.mode !== undefined) {
        await invoke("set_polish_mode", { mode: item.mode });
      } else if (popupType === "denoise" && item.mode !== undefined) {
        await invoke("set_denoise_mode", { mode: item.mode });
      } else if (popupType === "asr" && item.name !== undefined) {
        await invoke("switch_asr_engine", { name: item.name });
        showToast(`已切换：${item.name}（下次录音生效）`);
      } else if (popupType === "llm" && item.name !== undefined) {
        await invoke("switch_polish_llm", { name: item.name });
        showToast(item.name ? `已切换润色模型：${item.name}` : "已关闭润色模型");
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

  const tools: { id: string; icon: LucideIcon; label: string; active?: boolean; disabled?: boolean; onClick: () => void }[] = [
    { id: "close", icon: X, label: "关闭", onClick: () => invoke("discard_recording") },
    { id: "settings", icon: Settings, label: "系统设置", onClick: () => invoke("open_settings") },
    { id: "asr", icon: Mic, label: "语音模型", active: true, onClick: openAsrPopup },
    { id: "denoise", icon: Waves, label: "降噪模式", active: toolbarState.denoise_mode !== 0, onClick: openDenoisePopup },
    { id: "llm", icon: Sparkles, label: "润色模型", active: toolbarState.polish_llm_valid, onClick: openLlmPopup },
    { id: "polish", icon: Wand2, label: "润色模式", active: toolbarState.polish_mode !== 0, onClick: openPolishPopup },
    { id: "polish-now", icon: Zap, label: "立即润色", disabled: polishLoading, onClick: async () => {
      setPolishLoading(true);
      try { await invoke("polish_now"); showToast("润色中…"); }
      catch (e) { setPolishLoading(false); showToast("润色失败：" + e); }
    } },
    { id: "edit", icon: editing ? Save : Pencil, label: editing ? "保存编辑" : "编辑", active: editing, onClick: toggleEdit },
  ];

  return (
    <div
      id="result-container"
      className={cn(
        "w-full h-full bg-white rounded-lg border border-black/10 shadow-lg flex flex-col transition-opacity duration-150",
        visible ? "opacity-100" : "opacity-0",
      )}
    >
      {/* Top bar: drag + toolbar */}
      <div
        className={cn("flex-shrink-0 flex items-center transition-all duration-120", toolbarVisible ? "h-8" : "h-2")}
      >
        <div
          className={cn("flex items-center gap-0.5 px-1.5", toolbarVisible ? "flex" : "hidden")}
        >
          {tools.map(({ id, icon: Icon, label, active, disabled, onClick }) => (
            <button
              key={id}
              className={cn(
                "tool-btn w-[26px] h-[26px] flex items-center justify-center rounded-[5px] transition-colors",
                "text-foreground hover:text-primary hover:bg-black/[0.06]",
                active && "text-primary",
                disabled && "text-black/20 cursor-default hover:bg-transparent hover:text-black/20",
              )}
              title={label}
              aria-label={label}
              disabled={disabled}
              onClick={onClick}
            >
              <Icon className="w-[18px] h-[18px]" />
            </button>
          ))}
        </div>
        <div
          className="flex-1 h-full cursor-grab active:cursor-grabbing flex items-center justify-center"
          onMouseDown={onDragStart}
          data-tauri-drag-region
        >
          {!toolbarVisible && (
            <div className="w-6 h-[3px] bg-black/10 rounded-[1.5px]" />
          )}
        </div>
      </div>

      {/* Text display */}
      <div
        className={cn(
          "flex-1 px-3.5 pb-2 overflow-hidden relative",
          editing && "border border-primary/50 rounded-md bg-primary/[0.04]",
        )}
      >
        <div
          ref={textRef}
          className={cn(
            "text-sm leading-[1.5] text-foreground max-h-[63px] overflow-y-auto",
            "word-break-all outline-none thin-scrollbar",
            !editing && "cursor-text",
          )}
          contentEditable={editing}
          suppressContentEditableWarning
          onInput={onTextInput}
        >
          {text}
        </div>
      </div>

      {/* Popup */}
      {popupType && (
        <div className="popup-content absolute top-[30px] left-1.5 w-[360px] max-h-[200px] overflow-y-auto bg-white rounded-lg border border-black/10 shadow-lg z-10 text-[13px]">
          {popupItems.map((item, i) => (
            <div
              key={i}
              className={cn(
                "px-3 py-1.5 cursor-pointer flex items-center gap-1.5 transition-colors",
                "hover:bg-primary/[0.08]",
                item.current && "text-primary font-medium",
              )}
              onClick={() => handlePopupSelect(item)}
            >
              <span>{item.current ? "●" : "○"}</span>
              <span className="flex-1 min-w-0 truncate">{item.label}</span>
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
