import { useState, useEffect, useRef, useCallback, useMemo } from "react";
import { invoke } from "@tauri-apps/api/core";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { cn } from "@/lib/utils";
import { SvgIcon, type IconName } from "@/components/SvgIcon";
import { parseShortcut, matchShortcut } from "./shortcut";
import { AsrEditor, type AsrEditorHandle } from "./AsrEditor";
import { TranslationPane } from "./TranslationPane";
import { useT, t as ti18n } from "@/lib/i18n";

const POLISH_MODES = [0, 1, 2];
const DENOISE_MODES = [0, 1, 2];

type TranslateMode = 'off' | 'manual' | '4s' | '8s' | '12s';
const TRANSLATE_MODES: TranslateMode[] = ['manual', '4s', '8s', '12s'];

interface ToolbarState {
  polish_mode: number;
  denoise_mode: number;
  polish_llm_valid: boolean;
  hide_toolbar: boolean;
  edit_shortcut: string;
  translate_mode: string;
}

interface PopupItem {
  label: string;
  current: boolean;
  name?: string;
  mode?: number;
}

type PopupType = "polish" | "denoise" | "asr" | "llm" | "translate" | null;

function Result() {
  const t = useT();
  const [visible, setVisible] = useState(false);
  const [toolbarVisible, setToolbarVisible] = useState(false);
  const [expanded, setExpanded] = useState(false);
  const [text, setText] = useState("");
  const [isRecording, setIsRecording] = useState(false);
  const [isSpeaking, setIsSpeaking] = useState(false);
  const [toolbarState, setToolbarState] = useState<ToolbarState>({
    polish_mode: 0, denoise_mode: 1, polish_llm_valid: false,
    hide_toolbar: true, edit_shortcut: "CmdOrCtrl+Enter",
    translate_mode: "manual",
  });
  const [popupType, setPopupType] = useState<PopupType>(null);
  const [popupItems, setPopupItems] = useState<PopupItem[]>([]);
  const [toast, setToast] = useState<string | null>(null);
  const [polishLoading, setPolishLoading] = useState(false);
  const [translateMode, setTranslateMode] = useState<TranslateMode>('off');
  const [translatedText, setTranslatedText] = useState("");
  const [translating, setTranslating] = useState(false);

  const asrEditorRef = useRef<AsrEditorHandle>(null);
  const caretRef = useRef<number | null>(null);
  const [asrEditorResetKey, setAsrEditorResetKey] = useState(0);
  const speakingTimer = useRef<ReturnType<typeof setTimeout> | null>(null);
  const toolbarVisibleRef = useRef(false);
  const lastTranslatedRef = useRef<string>("");
  const translatingRef = useRef(false);
  const translateModeRef = useRef<TranslateMode>('off');
  const translatedTextRef = useRef("");
  const doTranslateRef = useRef<() => void>(() => {});
  useEffect(() => { translateModeRef.current = translateMode; }, [translateMode]);
  useEffect(() => { translatedTextRef.current = translatedText; }, [translatedText]);

  const win = useMemo(() => getCurrentWindow(), []);

  useEffect(() => { toolbarVisibleRef.current = toolbarVisible; }, [toolbarVisible]);

  const showToast = useCallback((msg: string, ms = 2000) => {
    setToast(msg);
    setTimeout(() => setToast(null), ms);
  }, []);

  const showToolbar = useCallback(() => {
    if (toolbarVisibleRef.current) return;
    setToolbarVisible(true);
  }, []);

  const hideToolbar = useCallback(() => {
    if (!toolbarVisibleRef.current) return;
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

  // VAD 驱动波纹
  useEffect(() => {
    const unlisten = listen<boolean>("update-speaking", (payload) => {
      const speaking = typeof payload === "boolean" ? payload : (payload as any)?.payload ?? false;
      if (speaking) {
        if (speakingTimer.current) clearTimeout(speakingTimer.current);
        setIsSpeaking(true);
      } else {
        if (speakingTimer.current) clearTimeout(speakingTimer.current);
        speakingTimer.current = setTimeout(() => {
          setIsSpeaking(false);
        }, 200);
      }
    });
    return () => { unlisten.then((f) => f()); };
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
          const t = p as string;
          const isPlaceholder = t === "正在聆听…" || t === "正在聆听..." || t === "Listening…" || t === "Listening...";
          setVisible(true);
          setIsRecording(true);
          setTranslateMode('off');
          setTranslatedText("");
          translatingRef.current = false;
          setTranslating(false);
          refreshActive();
          if (isPlaceholder) {
            setText("");
            caretRef.current = null;
            setAsrEditorResetKey(k => k + 1);
          } else {
            setText(t);
            caretRef.current = null;
          }
        }],
        ["update-result", (p) => {
          const payload = p as { text: string; insertion: boolean; caret: number };
          caretRef.current = payload.insertion ? payload.caret : null;
          setText(payload.text);
        }],
        ["clear-result", () => {
          setText("");
          caretRef.current = null;
          setVisible(false);
          setIsRecording(false);
          setAsrEditorResetKey(k => k + 1);
        }],
        ["hide-result", () => {
          setVisible(false);
          setIsRecording(false);
          setAsrEditorResetKey(k => k + 1);
        }],
        ["config-changed", () => refreshActive()],
        ["polish-done", () => setPolishLoading(false)],
        ["prepare-record", (p) => {
          invoke("start_recording", {
            prepareId: p as number,
            selection: null,
          });
        }],
      ];
      await Promise.all(handlers.map(async ([event, handler]) => {
        const fn = await listen(event, (e) => handler(e.payload));
        if (cancelled) { fn(); return; }
        unlistens.push(fn);
      }));
      if (!cancelled) {
        invoke("result_window_ready");
        refreshActive();
      }
    })();

    return () => { cancelled = true; unlistens.forEach((fn) => fn()); };
  }, [refreshActive]);

  const polishLoadingRef = useRef(false);
  useEffect(() => { polishLoadingRef.current = polishLoading; }, [polishLoading]);

  const polishNow = useCallback(async () => {
    if (polishLoadingRef.current) return;
    if (!text.trim()) return;
    // 编辑态先提交——否则后端润色的是旧 transcript，用户编辑被覆盖
    asrEditorRef.current?.commit();
    setPolishLoading(true);
    try { await invoke("polish_now"); showToast(ti18n("result.polishing")); }
    catch (e) { setPolishLoading(false); showToast(ti18n("result.polishFailed") + e); }
  }, [showToast, text]);

  // ── 翻译模式 ──
  const exitTranslateMode = useCallback(() => {
    setTranslateMode('off');
    setTranslatedText("");
    translatingRef.current = false;
    setTranslating(false);
  }, []);

  const doTranslate = useCallback(async () => {
    const source = asrEditorRef.current?.getText() ?? text;
    if (!source.trim()) return;
    if (translatingRef.current) return;
    translatingRef.current = true;
    setTranslating(true);
    lastTranslatedRef.current = source;
    try {
      await invoke("translate_text", { text: source });
    } catch (e) {
      translatingRef.current = false;
      setTranslating(false);
      showToast(ti18n("result.translateFail") + String(e));
    }
  }, [text, showToast]);
  useEffect(() => { doTranslateRef.current = doTranslate; }, [doTranslate]);

  const finalTranslate = useCallback(async (): Promise<string> => {
    const source = asrEditorRef.current?.getText() ?? text;
    if (!source.trim()) return "";
    return new Promise<string>((resolve) => {
      let resolved = false;
      const unlistenPromise = listen("translate-done", (e) => {
        if (resolved) return;
        resolved = true;
        unlistenPromise.then(f => f());
        resolve(e.payload as string);
      });
      invoke("translate_text", { text: source }).catch(() => {
        if (resolved) return;
        resolved = true;
        resolve("");
      });
    });
  }, [text]);

  const enterTranslateMode = useCallback(() => {
    const remembered = toolbarState.translate_mode;
    const mode: TranslateMode = TRANSLATE_MODES.includes(remembered as TranslateMode)
      ? remembered as TranslateMode
      : 'manual';
    setTranslateMode(mode);
    if (!expanded) {
      setExpanded(true);
      invoke("set_result_click_through", { expanded: true });
    }
    setTimeout(() => doTranslateRef.current(), 100);
  }, [toolbarState.translate_mode, expanded]);

  // 翻译事件监听——仅翻译模式下生效
  useEffect(() => {
    if (translateMode === 'off') return;
    let unlistens: UnlistenFn[] = [];
    let cancelled = false;

    (async () => {
      const fnProgress = await listen("translate-progress", (e) => {
        setTranslatedText(e.payload as string);
      });
      if (cancelled) { fnProgress(); return; }
      unlistens.push(fnProgress);

      const fnDone = await listen("translate-done", (e) => {
        setTranslatedText(e.payload as string);
        translatingRef.current = false;
        setTranslating(false);
      });
      if (cancelled) { fnDone(); return; }
      unlistens.push(fnDone);
    })();

    return () => {
      cancelled = true;
      unlistens.forEach(f => f());
    };
  }, [translateMode]);

  // 自动档节流——translateMode 为 8s/12s/15s 时启动定时器
  useEffect(() => {
    if (translateMode === 'off' || translateMode === 'manual') return;
    const secs = parseInt(translateMode, 10);
    if (isNaN(secs)) return;

    const timer = setInterval(() => {
      const current = asrEditorRef.current?.getText() ?? "";
      if (current !== lastTranslatedRef.current && !translatingRef.current && current.trim()) {
        doTranslate();
      }
    }, secs * 1000);

    return () => clearInterval(timer);
  }, [translateMode, doTranslate]);

  const onSave = useCallback(async () => {
    if (translateModeRef.current === 'off') {
      asrEditorRef.current?.commit();
      return;
    }
    asrEditorRef.current?.commit();
    const finalText = await finalTranslate();
    const submitText = finalText && !finalText.startsWith("❌")
      ? finalText
      : translatedTextRef.current;
    invoke("commit_translation", {
      text: submitText,
    });
    exitTranslateMode();
  }, [finalTranslate, exitTranslateMode]);
  const onSaveRef = useRef(onSave);
  useEffect(() => { onSaveRef.current = onSave; }, [onSave]);

  const toggleExpand = useCallback(() => {
    const next = !expanded;
    setExpanded(next);
    invoke("set_result_click_through", { expanded: next });
  }, [expanded]);

  // 全局立即润色快捷键
  useEffect(() => {
    let unlisten: UnlistenFn | undefined;
    let cancelled = false;
    listen("global-polish-trigger", () => polishNow()).then((fn) => {
      if (cancelled) fn();
      else unlisten = fn;
    });
    return () => { cancelled = true; unlisten?.(); };
  }, [polishNow]);

  // ── Keyboard shortcuts ──
  useEffect(() => {
    const onKeyDown = (e: KeyboardEvent) => {
      if (e.key === "Escape") {
        if (popupType) { setPopupType(null); return; }
        invoke("cancel_recording");
        win.hide();
        return;
      }
      if (e.metaKey && e.key === 't') {
        e.preventDefault();
        if (translateModeRef.current !== 'off') doTranslateRef.current();
        return;
      }
      const sc = parseShortcut(toolbarState.edit_shortcut);
      if (matchShortcut(e, sc)) {
        e.preventDefault();
        onSaveRef.current();
      }
    };
    document.addEventListener("keydown", onKeyDown);
    return () => document.removeEventListener("keydown", onKeyDown);
  }, [popupType, toolbarState.edit_shortcut, win]);

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

  const openPolishPopup = async () => {
    if (popupType === "polish") { setPopupType(null); return; }
    const polishLabels = [ti18n("result.polish.off"), ti18n("result.polish.finalOnly"), ti18n("result.polish.intermediate")];
    setPopupItems(POLISH_MODES.map(m => ({
      label: polishLabels[m], current: m === toolbarState.polish_mode, mode: m,
    })));
    setPopupType("polish");
  };

  const openDenoisePopup = async () => {
    if (popupType === "denoise") { setPopupType(null); return; }
    const denoiseLabels = [ti18n("result.denoise.none"), ti18n("result.denoise.light"), ti18n("result.denoise.deep")];
    setPopupItems(DENOISE_MODES.map(m => ({
      label: denoiseLabels[m], current: m === toolbarState.denoise_mode, mode: m,
    })));
    setPopupType("denoise");
  };

  const openTranslatePopup = async () => {
    if (translateMode === 'off') {
      enterTranslateMode();
      return;
    }
    if (popupType === "translate") { setPopupType(null); return; }
    setPopupItems(TRANSLATE_MODES.map(m => ({
      label: m === 'manual' ? t("result.translateManual")
        : m === '4s' ? t("result.translateAuto4")
        : m === '8s' ? t("result.translateAuto8")
        : t("result.translateAuto12"),
      current: m === translateMode,
      name: m,
    })));
    setPopupType("translate");
  };

  const handlePopupSelect = async (item: PopupItem) => {
    try {
      if (popupType === "polish" && item.mode !== undefined) {
        await invoke("set_polish_mode", { mode: item.mode });
      } else if (popupType === "denoise" && item.mode !== undefined) {
        await invoke("set_denoise_mode", { mode: item.mode });
      } else if (popupType === "translate" && item.name) {
        const mode = item.name as TranslateMode;
        setTranslateMode(mode);
        setPopupType(null);
        await invoke("set_translate_mode", { mode });
        return;
      }
      setPopupType(null);
      refreshActive();
    } catch (e) { showToast(ti18n("result.switchFailed") + e); }
  };

  const onDragStart = (e: React.MouseEvent) => {
    e.preventDefault();
    win.startDragging();
  };

  const tools: { id: string; icon: IconName; label: string; active?: boolean; disabled?: boolean; onClick: () => void }[] = [
    { id: "close", icon: "close", label: t("result.close"), onClick: () => invoke("discard_recording") },
    { id: "denoise", icon: "denoise", label: t("result.denoiseMode"), active: toolbarState.denoise_mode !== 0, onClick: openDenoisePopup },
    { id: "polish", icon: "polish", label: t("result.polishMode"), active: toolbarState.polish_mode !== 0, onClick: openPolishPopup },
    { id: "polish-now", icon: "polish-now", label: t("result.polishNow"), disabled: polishLoading, onClick: polishNow },
    { id: "translate", icon: "translate" as IconName, label: t("result.translate"), active: translateMode !== 'off', onClick: openTranslatePopup },
    { id: "translate-now", icon: "redo" as IconName, label: t("result.translateNow"), disabled: translating || translateMode === 'off', onClick: doTranslate },
    { id: "toggle-size", icon: (expanded ? "minimize" : "expand-edit") as IconName, label: expanded ? t("result.zoomOut") : t("result.zoomIn"), disabled: translateMode !== 'off', onClick: toggleExpand },
    { id: "save", icon: "save" as IconName, label: t("result.save"), onClick: onSave },
  ];

  return (
    <div className="relative w-full h-full">
    <div
      id="result-container"
      className={cn(
        "absolute top-0 left-1/2 -translate-x-1/2 rounded-lg border border-black/[0.08] shadow-lg shadow-black/[0.06] flex flex-col overflow-hidden transition-all duration-200 ease-out",
        expanded ? "w-[720px] h-[480px]" : "w-[720px] h-[116px]",
        visible ? "opacity-100" : "opacity-0",
      )}
      style={{ backgroundColor: "var(--color-surface)" }}
    >
      {/* Top bar: toolbar + drag handle + voice line */}
      <div className="flex-shrink-0 flex flex-col relative">
        {/* 录音提示 */}
        {!text.trim() && isRecording && (
          <div className="absolute top-0 left-0 right-0 flex items-center justify-center h-[22px] pointer-events-none z-20">
            <span className="text-[11px] select-none" style={{ color: "var(--color-tool-icon)", opacity: 0.35 }}>{t("result.listening")}</span>
          </div>
        )}
        {/* Toolbar */}
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
                "hover:text-[#007aff] hover:bg-black/[0.05]",
                active && "text-[#007aff]!",
                disabled && "cursor-default hover:bg-transparent",
              )}
              style={{ color: active ? "#007aff" : "var(--color-tool-icon)", opacity: disabled ? 0.35 : 1 }}
              title={label}
              aria-label={label}
              disabled={disabled}
              onMouseDown={(e) => e.stopPropagation()}
              onClick={onClick}
            >
              <SvgIcon name={icon} size={16} />
            </button>
          ))}
        </div>
        {/* Drag handle */}
        <div className="flex items-center justify-center h-2">
          <div
            className="w-6 h-[3px] rounded-[1.5px] cursor-grab active:cursor-grabbing"
            style={{ backgroundColor: "var(--color-tool-icon)", opacity: 0.25 }}
            onMouseDown={onDragStart}
          />
        </div>
        {/* Voice line */}
        {isRecording && (
          <div className={cn("mx-3.5 transition-all duration-300", isSpeaking ? "voice-line-speaking" : "voice-line-idle")} />
        )}
      </div>

      {/* Text display */}
      <div className="flex-1 px-3.5 pt-1 pb-2 overflow-hidden relative">
        {translateMode === 'off' ? (
          <div className="relative h-full">
            <AsrEditor
              key={asrEditorResetKey}
              ref={asrEditorRef}
              text={text}
              caret={caretRef.current}
              expanded={expanded}
              onCommit={(payload) => {
                setText(payload.text);
                if (translateModeRef.current === 'off') {
                  invoke("commit_edit", {
                    text: payload.text,
                    dirtyRanges: payload.dirtyRanges,
                    hasEdited: payload.hasEdited,
                    caret: payload.caret ?? null,
                    selection: payload.selection ?? null,
                  });
                }
              }}
            />
          </div>
        ) : (
          <div className="flex flex-col h-full gap-1">
            {/* 原文区（上） */}
            <div className="flex-1 min-h-0 border-b border-black/[0.06] overflow-hidden">
              <AsrEditor
                key={asrEditorResetKey}
                ref={asrEditorRef}
                text={text}
                caret={caretRef.current}
                expanded={true}
                onCommit={(payload) => {
                  setText(payload.text);
                }}
              />
            </div>
            {/* 译文区（下） */}
            <div className="flex-1 min-h-0 overflow-hidden">
              <TranslationPane
                text={translatedText}
                translating={translating}
                onChange={setTranslatedText}
              />
            </div>
          </div>
        )}
      </div>

      {/* Popup */}
      {popupType && (
        <div className="popup-content absolute top-[28px] left-1.5 w-[360px] bg-background rounded-lg border border-black/[0.10] shadow-lg shadow-black/[0.12] z-30 text-[12px]">
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
              <span className={cn("text-[10px]", item.current ? "text-[#007aff]" : "")} style={item.current ? undefined : { color: "var(--color-tool-icon)", opacity: 0.6 }}>
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
    </div>
  );
}

export default Result;
